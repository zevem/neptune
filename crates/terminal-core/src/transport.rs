//! Where a session engine's bytes come from and go to. The engine parses what a
//! transport's source reads, sends input, resize and its own replies to the
//! transport's sink, and reaps through its process. The local PTY (or ConPTY)
//! is the first transport. Backend types stay private to this crate.
use super::*;
use portable_pty::{Child, CommandBuilder, MasterPty};
#[cfg(windows)]
use runtime::Worker;
#[path = "platform/mod.rs"]
mod platform;
#[cfg(windows)]
use platform::close_conpty;
#[cfg(unix)]
pub(super) use platform::poll_pty;
pub(super) use platform::process_cwd;
#[cfg(unix)]
pub(super) use platform::shell_activity;

/// One session's transport, split across the reader, writer and engine workers.
pub(super) struct Transport {
    pub(super) source: Box<dyn Source>,
    pub(super) sink: Box<dyn Sink>,
    pub(super) process: Box<dyn Process>,
    /// The program shown in metadata.
    pub(super) shell: String,
    pub(super) process_id: Option<u32>,
    /// Set when another engine, the session daemon's, is authoritative for
    /// the program: the state its output continues from. This engine is then
    /// a replica, and dropping its handle detaches instead of closing.
    pub(super) replica: Option<Snapshot>,
}

/// Output from the program, read by the reader worker.
pub(super) trait Source: Send {
    /// `Ok(0)` is end of output. A nonblocking source reports `WouldBlock`.
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize>;
    /// After `WouldBlock`, wait for output. `Ok(false)` stops reading quietly
    /// because the session is stopping; an error is reported as a read failure.
    fn wait_readable(&mut self, shared: &Shared) -> io::Result<bool>;
}

/// Bytes and geometry for the program, sent by the writer worker in queue order.
pub(super) trait Sink: Send {
    /// Input typed or pasted into the session. A nonblocking sink reports `WouldBlock`.
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize>;
    /// After `WouldBlock`, wait until writable. `Ok(false)` abandons the write
    /// because the session is stopping; an error is reported as a write failure.
    fn wait_writable(&mut self, shared: &Shared) -> io::Result<bool>;
    fn resize(&mut self, size: Size) -> Result<()>;
    /// Answers the engine generated itself: cursor position, device attributes,
    /// colours, text area size, notification capabilities and focus state on
    /// enable. A transport whose program is already answered by another engine
    /// drops them, so the program never receives a reply twice.
    fn reply(&mut self, bytes: &[u8], shared: &Shared) -> Result<()> {
        write_all(self, bytes, shared)
    }
    /// The pane's focus, sent only by replicas: the authoritative engine
    /// tracks focus and reports it to programs that ask.
    fn focus(&mut self, _focused: bool) -> Result<()> {
        Ok(())
    }
}

/// The program's lifetime, owned and reaped by the engine worker.
pub(super) trait Process: Send {
    fn try_wait(&mut self) -> io::Result<Option<Exit>>;
    /// End the program and reap it. `None` when reaping failed; the engine may retry.
    fn terminate(&mut self) -> Option<Exit>;
    fn activity(&self, pid: u32) -> ProcessActivity;
    /// Close the terminal device. The engine calls this once: on Unix before
    /// terminating, so BSD output draining cannot block reaping; on Windows
    /// after exit, because closing ConPTY can wait for output to drain.
    fn release(&mut self, shared: &Arc<Shared>);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Exit {
    pub(super) code: u32,
    pub(super) signal: Option<String>,
}

/// Write every byte in bounded chunks unless the session stops first.
pub(super) fn write_all<S: Sink + ?Sized>(
    sink: &mut S,
    bytes: &[u8],
    shared: &Shared,
) -> Result<()> {
    let mut remaining = bytes;
    while !remaining.is_empty() && !shared.stopped.load(Ordering::Acquire) {
        match sink.write(&remaining[..remaining.len().min(8192)]) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero).into()),
            Ok(len) => remaining = &remaining[len..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if !sink.wait_writable(shared)? {
                    break;
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// Start `options`' program on a new pseudoterminal of `size`.
pub(super) fn spawn_pty(options: &SessionOptions, size: Size) -> Result<Transport> {
    let pair = portable_pty::native_pty_system()
        .openpty(size.pty())
        .context("Open pseudoterminal")?;
    let mut command = match &options.shell {
        Some(shell) => {
            let mut command = CommandBuilder::new(shell);
            command.args(&options.args);
            command
        }
        None => CommandBuilder::new_default_prog(),
    };
    command.cwd(&options.cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("TERM_PROGRAM", "neptune");
    command.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
    for (key, value) in &options.env {
        command.env(key, value);
    }
    let shell = options.shell.clone().unwrap_or_else(|| command.get_shell());
    // Explicit arguments mean a caller-chosen program, such as SSH.
    #[cfg(windows)]
    if options.args.is_empty() {
        shell_integration::report_cwd(&mut command, &shell);
    }
    let mut child_guard = ChildGuard(Some(
        pair.slave.spawn_command(command).context("Start shell")?,
    ));
    let process_id = child_guard.0.as_ref().unwrap().process_id();
    // Keeping the slave alive prevents EOF after the child exits.
    drop(pair.slave);
    let reader = pair.master.try_clone_reader().context("Clone PTY reader")?;
    let writer = pair.master.take_writer().context("Take PTY writer")?;

    #[cfg(unix)]
    let poll_fd = platform::prepare_nonblocking(&*pair.master)?;

    let master: Master = Arc::new(Mutex::new(Some(pair.master)));
    Ok(Transport {
        source: Box::new(PtySource {
            reader,
            #[cfg(unix)]
            poll_fd,
        }),
        sink: Box::new(PtySink {
            writer,
            master: master.clone(),
            #[cfg(unix)]
            poll_fd,
        }),
        process: Box::new(PtyProcess {
            child: child_guard.0.take().unwrap(),
            master,
        }),
        shell,
        process_id,
        replica: None,
    })
}

type Master = Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>;

/// Failed startup must terminate and reap a shell that was already created.
struct ChildGuard(Option<Box<dyn Child + Send + Sync>>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct PtySource {
    reader: Box<dyn Read + Send>,
    #[cfg(unix)]
    poll_fd: libc::c_int,
}

impl Source for PtySource {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buffer)
    }

    #[cfg(unix)]
    fn wait_readable(&mut self, shared: &Shared) -> io::Result<bool> {
        Ok(poll_pty(self.poll_fd, libc::POLLIN, shared))
    }

    /// ConPTY pipes block; `WouldBlock` is an ordinary read failure there.
    #[cfg(not(unix))]
    fn wait_readable(&mut self, _: &Shared) -> io::Result<bool> {
        Err(io::ErrorKind::WouldBlock.into())
    }
}

struct PtySink {
    writer: Box<dyn Write + Send>,
    master: Master,
    #[cfg(unix)]
    poll_fd: libc::c_int,
}

impl Sink for PtySink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writer.write(bytes)
    }

    #[cfg(unix)]
    fn wait_writable(&mut self, shared: &Shared) -> io::Result<bool> {
        Ok(poll_pty(self.poll_fd, libc::POLLOUT, shared))
    }

    /// ConPTY pipes block; `WouldBlock` is an ordinary write failure there.
    #[cfg(not(unix))]
    fn wait_writable(&mut self, _: &Shared) -> io::Result<bool> {
        Err(io::ErrorKind::WouldBlock.into())
    }

    fn resize(&mut self, size: Size) -> Result<()> {
        self.master
            .lock()
            .as_ref()
            .map(|master| master.resize(size.pty()))
            .unwrap_or(Ok(()))
    }
}

struct PtyProcess {
    child: Box<dyn Child + Send + Sync>,
    master: Master,
}

fn exit(status: portable_pty::ExitStatus) -> Exit {
    Exit {
        code: status.exit_code(),
        signal: status.signal().map(str::to_owned),
    }
}

impl Process for PtyProcess {
    fn try_wait(&mut self) -> io::Result<Option<Exit>> {
        Ok(self.child.try_wait()?.map(exit))
    }

    fn terminate(&mut self) -> Option<Exit> {
        let _ = self.child.kill();
        self.child.wait().ok().map(exit)
    }

    fn activity(&self, pid: u32) -> ProcessActivity {
        match self.master.lock().as_ref() {
            Some(master) => platform::process_activity(pid, &**master),
            None => ProcessActivity::Unknown,
        }
    }

    #[cfg(not(windows))]
    fn release(&mut self, _: &Arc<Shared>) {
        self.master.lock().take();
    }

    #[cfg(windows)]
    fn release(&mut self, shared: &Arc<Shared>) {
        close_conpty(&self.master, shared);
    }
}

/// Failed startup must terminate and reap a process that was already created.
/// The engine worker disarms this once it owns the process.
pub(super) struct ProcessGuard(pub(super) Option<Box<dyn Process>>);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if let Some(process) = &mut self.0 {
            let _ = process.terminate();
        }
    }
}
