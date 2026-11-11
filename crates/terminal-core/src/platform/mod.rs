//! Platform transport, process metadata, and ConPTY teardown.
use super::*;

mod processes;
pub(super) use processes::process_activity;
#[cfg(unix)]
pub(in crate::session) use processes::shell_activity;

/// Becomes readable, and stays so, once its session stops: what ends a
/// worker's wait on a descriptor that has nothing for it.
#[cfg(unix)]
pub(in crate::session) struct StopSignal {
    raised: io::PipeReader,
    raise: io::PipeWriter,
}

#[cfg(unix)]
impl StopSignal {
    pub(in crate::session) fn new() -> io::Result<Self> {
        let (raised, raise) = io::pipe()?;
        Ok(Self { raised, raise })
    }

    pub(in crate::session) fn raise(&self) {
        // One byte always fits, and is never read back.
        let _ = (&self.raise).write(&[0]);
    }
}

/// Wait, for as long as it takes, until `fd` is ready for `events`.
/// `Ok(false)` means the session stopped first.
#[cfg(unix)]
pub(in crate::session) fn poll_pty(
    fd: libc::c_int,
    events: libc::c_short,
    shared: &Shared,
) -> io::Result<bool> {
    while !shared.stopped.load(Ordering::Acquire) {
        let mut descriptors = [
            libc::pollfd {
                fd,
                events,
                revents: 0,
            },
            libc::pollfd {
                fd: std::os::fd::AsRawFd::as_raw_fd(&shared.stop_signal.raised),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: two valid descriptors, both open for as long as the call.
        let result = unsafe { libc::poll(descriptors.as_mut_ptr(), 2, -1) };
        if result > 0 {
            return Ok(!shared.stopped.load(Ordering::Acquire));
        }
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
    Ok(false)
}

/// ConPTY remains open after its final child exits. Closing it can wait for VT
/// output draining, so neither the parser nor the reader can perform that close.
/// See <https://learn.microsoft.com/en-us/windows/console/closepseudoconsole>.
#[cfg(windows)]
pub(super) fn close_conpty(master: &Master, shared: &Arc<Shared>) {
    let Some(master) = master.lock().take() else {
        return;
    };
    let shared = shared.clone();
    let error_shared = shared.clone();
    let worker = Worker::new(shared.clone());
    if let Err(error) = thread::Builder::new()
        .name("terminal-conpty-close".into())
        .spawn(move || {
            let _worker = worker;
            drop(master);
        })
    {
        error_shared.error(format!("Start ConPTY close worker: {error}"));
    }
}

#[cfg(target_os = "linux")]
pub(in crate::session) fn process_cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}
#[cfg(not(target_os = "linux"))]
pub(in crate::session) fn process_cwd(_pid: u32) -> Option<PathBuf> {
    None
}

#[cfg(unix)]
pub(super) fn prepare_nonblocking(master: &dyn MasterPty) -> Result<libc::c_int> {
    let fd = master.as_raw_fd().context("PTY has no native descriptor")?;
    // Clones share the open-file description. Polling makes idle reads/pastes
    // interruptible without a second reader or a signal from the GUI thread.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        bail!("Set nonblocking PTY: {}", io::Error::last_os_error());
    }
    Ok(fd)
}
