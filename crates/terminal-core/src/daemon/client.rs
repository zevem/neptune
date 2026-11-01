//! The client: one control connection for requests, and one connection per
//! attached session, whose replica engine reads the session's output from it.
use super::*;
use protocol::{FrameBuffer, MAX_DAEMON_FRAME, Reply, Request};

/// Requests that take longer than this mean the daemon is not answering.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// A connection to the session daemon serving one directory.
pub struct DaemonClient {
    dir: PathBuf,
    control: Mutex<UnixStream>,
}

/// A session the daemon runs, attached or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    pub id: SessionId,
    /// `Running`, or `Exited` once its program ended while detached.
    pub status: SessionStatus,
    /// What a client last stored with [`DaemonClient::set_data`].
    pub data: Vec<u8>,
}

/// A daemon-backed session and the data stored with it.
pub struct Attached {
    pub session: TerminalSession,
    pub data: Vec<u8>,
}

fn unexpected() -> SessionError {
    unavailable("unexpected reply")
}

fn reply_error(reply: Reply) -> SessionError {
    match reply {
        Reply::Failed { kind, detail } => SessionError::new(kind, detail),
        _ => unexpected(),
    }
}

/// Connect to `dir`'s daemon of protocol `version` and say hello.
fn open(dir: &Path, version: u32) -> std::result::Result<UnixStream, SessionError> {
    check_private_dir(dir).map_err(unavailable)?;
    let mut stream = UnixStream::connect(socket_path(dir, version)).map_err(unavailable)?;
    check_peer(&stream, effective_uid()).map_err(unavailable)?;
    stream
        .set_read_timeout(Some(REPLY_TIMEOUT))
        .map_err(unavailable)?;
    match exchange(&mut stream, &Request::Hello { version })? {
        Reply::Welcome { version: agreed } if agreed == version => Ok(stream),
        Reply::Refused { version: theirs } => Err(SessionError::new(
            SessionErrorKind::Incompatible,
            format!(
                "The session daemon speaks protocol version {theirs}; this client speaks version {version}"
            ),
        )),
        _ => Err(unexpected()),
    }
}

fn exchange(
    stream: &mut UnixStream,
    request: &Request,
) -> std::result::Result<Reply, SessionError> {
    stream.write_all(&request.encode()).map_err(unavailable)?;
    match protocol::read_frame(stream, MAX_DAEMON_FRAME).map_err(unavailable)? {
        Some(frame) => Reply::decode(&frame).map_err(unavailable),
        None => Err(unavailable("the connection closed")),
    }
}

impl DaemonClient {
    /// Connect to the daemon serving `dir`, which must be user-only.
    pub fn connect(dir: &Path) -> std::result::Result<Self, SessionError> {
        Ok(Self {
            dir: dir.to_owned(),
            control: Mutex::new(open(dir, PROTOCOL_VERSION)?),
        })
    }

    fn request(&self, request: &Request) -> std::result::Result<Reply, SessionError> {
        exchange(&mut self.control.lock(), request)
    }

    pub fn list(&self) -> std::result::Result<Vec<SessionEntry>, SessionError> {
        match self.request(&Request::List)? {
            Reply::Sessions(entries) => Ok(entries
                .into_iter()
                .map(|entry| SessionEntry {
                    id: SessionId(entry.id),
                    status: match entry.exit {
                        None => SessionStatus::Running,
                        Some(exit) => SessionStatus::Exited {
                            code: exit.code,
                            signal: exit.signal,
                        },
                    },
                    data: entry.data,
                })
                .collect()),
            reply => Err(reply_error(reply)),
        }
    }

    /// Store opaque data (at most 64 KiB) with a session, returned on attach and list.
    pub fn set_data(&self, id: SessionId, data: &[u8]) -> std::result::Result<(), SessionError> {
        match self.request(&Request::SetData {
            id: id.0,
            data: data.to_vec(),
        })? {
            Reply::Done => Ok(()),
            reply => Err(reply_error(reply)),
        }
    }

    /// End a session's processes, attached or not.
    pub fn close(&self, id: SessionId) -> std::result::Result<(), SessionError> {
        match self.request(&Request::Close { id: id.0 })? {
            Reply::Done => Ok(()),
            reply => Err(reply_error(reply)),
        }
    }

    /// Start a session in the daemon with the same options as
    /// [`TerminalSession::spawn`], attached to this client.
    pub fn start(
        &self,
        options: SessionOptions,
        repaint: Repaint,
    ) -> std::result::Result<(TerminalSession, SessionId), SessionError> {
        validate_options(&options)?;
        let mut stream = open(&self.dir, PROTOCOL_VERSION)?;
        match exchange(&mut stream, &Request::Start(options))? {
            Reply::Attached(attached) => {
                let id = SessionId(attached.id);
                Ok((replica(stream, *attached, repaint)?, id))
            }
            reply => Err(reply_error(reply)),
        }
    }

    /// Attach to a session, which takes this pane size. Its screen,
    /// scrollback and cursor are those of the running session.
    pub fn attach(
        &self,
        id: SessionId,
        cols: u16,
        rows: u16,
        repaint: Repaint,
    ) -> std::result::Result<Attached, SessionError> {
        if cols == 0 || rows == 0 || cols > 4096 || rows > 4096 {
            return Err(SessionError::new(
                SessionErrorKind::InvalidGeometry,
                "Invalid terminal dimensions",
            ));
        }
        let mut stream = open(&self.dir, PROTOCOL_VERSION)?;
        match exchange(
            &mut stream,
            &Request::Attach {
                id: id.0,
                cols,
                rows,
            },
        )? {
            Reply::Attached(attached) => {
                let data = attached.data.clone();
                Ok(Attached {
                    session: replica(stream, *attached, repaint)?,
                    data,
                })
            }
            reply => Err(reply_error(reply)),
        }
    }
}

/// A session engine continuing from the daemon's snapshot.
fn replica(
    stream: UnixStream,
    attached: protocol::Attached,
    repaint: Repaint,
) -> std::result::Result<TerminalSession, SessionError> {
    let setup = || -> io::Result<_> {
        stream.set_read_timeout(None)?;
        stream.set_nonblocking(true)?;
        Ok((stream.try_clone()?, stream.try_clone()?))
    };
    let (reader, writer) = setup().map_err(unavailable)?;
    let link = Arc::new(Link::default());
    let writer = Arc::new(Mutex::new(writer));
    let mut snapshot = attached.snapshot;
    // The engine parses the daemon's unfinished output before the stream.
    let pending = std::mem::take(&mut snapshot.pending);
    let size = Size {
        cols: snapshot.cols,
        rows: snapshot.rows,
        pixel_width: 0,
        pixel_height: 0,
    };
    let transport = Transport {
        source: Box::new(ReplicaSource {
            stream: reader,
            frames: FrameBuffer::default(),
            scratch: vec![0; READ_SIZE],
            pending,
            offset: 0,
            ended: None,
            link: link.clone(),
        }),
        sink: Box::new(ReplicaSink {
            writer: writer.clone(),
        }),
        process: Box::new(ReplicaProcess {
            link,
            writer,
            socket: stream,
        }),
        shell: attached.shell,
        process_id: attached.process_id,
        replica: Some(snapshot),
    };
    TerminalSession::start(
        transport,
        attached.cwd,
        size,
        attached.scrollback as usize,
        repaint,
    )
    .map_err(SessionError::spawn)
}

/// What the reader learned about the program, for the engine's process.
#[derive(Default)]
struct Link {
    exit: Mutex<Option<Exit>>,
    /// The daemon went away without reporting an exit.
    lost: AtomicBool,
}

fn lost() -> io::Error {
    io::Error::other("Session ended unexpectedly")
}

struct ReplicaSource {
    stream: UnixStream,
    frames: FrameBuffer,
    scratch: Vec<u8>,
    /// Output received but not yet handed to the engine.
    pending: Vec<u8>,
    offset: usize,
    /// The daemon's final frame, once received.
    ended: Option<Option<Exit>>,
    link: Arc<Link>,
}

impl transport::Source for ReplicaSource {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut filled = 0;
        loop {
            let available = &self.pending[self.offset..];
            let len = available.len().min(buffer.len() - filled);
            buffer[filled..filled + len].copy_from_slice(&available[..len]);
            self.offset += len;
            filled += len;
            if filled == buffer.len() {
                return Ok(filled);
            }
            if let Some(exit) = &self.ended {
                if filled > 0 {
                    return Ok(filled);
                }
                match exit {
                    Some(exit) => *self.link.exit.lock() = Some(exit.clone()),
                    None => self.link.lost.store(true, Ordering::Release),
                }
                return Ok(0);
            }
            match self.frames.next(MAX_DAEMON_FRAME)? {
                Some(frame) => match Reply::decode(&frame)? {
                    Reply::Output(bytes) => {
                        self.pending = bytes;
                        self.offset = 0;
                    }
                    Reply::Ended(exit) => self.ended = Some(exit),
                    _ => return Err(io::ErrorKind::InvalidData.into()),
                },
                None if filled > 0 => return Ok(filled),
                None => match self.stream.read(&mut self.scratch) {
                    Ok(0) => self.ended = Some(None),
                    Ok(len) => self.frames.extend(&self.scratch[..len]),
                    Err(error) => return Err(error),
                },
            }
        }
    }

    fn wait_readable(&mut self, shared: &Shared) -> io::Result<bool> {
        Ok(transport::poll_pty(
            self.stream.as_raw_fd(),
            libc::POLLIN,
            shared,
        ))
    }
}

/// Write one whole frame to the nonblocking connection. Shutting the
/// connection down ends a write the daemon never reads.
fn send_frame(stream: &mut UnixStream, request: &Request) -> io::Result<()> {
    let frame = request.encode();
    let mut remaining = &frame[..];
    while !remaining.is_empty() {
        match stream.write(remaining) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(len) => remaining = &remaining[len..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let mut descriptor = libc::pollfd {
                    fd: stream.as_raw_fd(),
                    events: libc::POLLOUT,
                    revents: 0,
                };
                unsafe { libc::poll(&mut descriptor, 1, 1000) };
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

struct ReplicaSink {
    writer: Arc<Mutex<UnixStream>>,
}

impl ReplicaSink {
    fn send(&self, request: &Request) -> io::Result<()> {
        send_frame(&mut self.writer.lock(), request)
    }
}

impl transport::Sink for ReplicaSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.send(&Request::Input(bytes.to_vec()))?;
        Ok(bytes.len())
    }

    fn wait_writable(&mut self, _: &Shared) -> io::Result<bool> {
        Ok(true)
    }

    fn resize(&mut self, size: Size) -> Result<()> {
        Ok(self.send(&Request::Resize {
            cols: size.cols,
            rows: size.rows,
            pixel_width: size.pixel_width,
            pixel_height: size.pixel_height,
        })?)
    }

    /// The daemon's engine already answered the program.
    fn reply(&mut self, _: &[u8], _: &Shared) -> Result<()> {
        Ok(())
    }

    fn focus(&mut self, focused: bool) -> Result<()> {
        Ok(self.send(&Request::Focus(focused))?)
    }
}

struct ReplicaProcess {
    link: Arc<Link>,
    writer: Arc<Mutex<UnixStream>>,
    socket: UnixStream,
}

impl transport::Process for ReplicaProcess {
    fn try_wait(&mut self) -> io::Result<Option<Exit>> {
        if self.link.lost.load(Ordering::Acquire) {
            return Err(lost());
        }
        Ok(self.link.exit.lock().clone())
    }

    /// The daemon ends the program; its exit is known only if it was reported.
    fn terminate(&mut self) -> Option<Exit> {
        self.link.exit.lock().clone()
    }

    fn activity(&self, _: u32) -> ProcessActivity {
        ProcessActivity::Unknown
    }

    /// Close asks the daemon to end the session; detach only disconnects.
    fn release(&mut self, shared: &Arc<Shared>) {
        let running = self.link.exit.lock().is_none() && !self.link.lost.load(Ordering::Acquire);
        if running
            && !shared.detached.load(Ordering::Acquire)
            && let Some(mut writer) = self.writer.try_lock_for(Duration::from_secs(1))
        {
            let _ = send_frame(&mut writer, &Request::End);
        }
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }
}
