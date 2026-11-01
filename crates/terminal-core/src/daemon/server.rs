//! The session daemon: one listener thread, and per connection one thread for
//! requests that, once the connection streams a session, also forwards its
//! input while a second thread sends its output.
use super::*;
use protocol::{Attached as AttachedFrame, Entry, MAX_CLIENT_FRAME, MAX_DATA, Reply, Request};
use std::collections::HashMap;
use std::fs::File;
use std::os::unix::net::UnixListener;

/// Run a session daemon of [`PROTOCOL_VERSION`] on `dir` until it has had
/// neither a session nor a client for `options.idle_exit`. `dir` is created
/// user-only, or an existing directory of this user's is closed to others.
/// Fails if another daemon of this protocol version already serves `dir`;
/// daemons of other versions serving it are left alone.
pub fn run(dir: &Path, options: DaemonOptions) -> std::result::Result<(), SessionError> {
    serve(dir, options, PROTOCOL_VERSION)
}

/// Serve `dir` as the daemon of protocol `version`, whose lock and socket
/// carry its version.
pub(super) fn serve(
    dir: &Path,
    options: DaemonOptions,
    version: u32,
) -> std::result::Result<(), SessionError> {
    make_private_dir(dir).map_err(unavailable)?;
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(dir, version))
        .map_err(unavailable)?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(unavailable("another session daemon serves this directory"));
    }
    let socket = socket_path(dir, version);
    match std::fs::remove_file(&socket) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(unavailable(error)),
    }
    let listener = UnixListener::bind(&socket).map_err(unavailable)?;
    listener.set_nonblocking(true).map_err(unavailable)?;
    let (wake, woken) = UnixStream::pair().map_err(unavailable)?;
    wake.set_nonblocking(true).map_err(unavailable)?;
    woken.set_nonblocking(true).map_err(unavailable)?;
    let daemon = Arc::new(Daemon {
        sessions: Mutex::new(HashMap::new()),
        connections: AtomicUsize::new(0),
        wake,
        owner: effective_uid(),
        version,
        client: Mutex::new(None),
        serial: AtomicU64::new(0),
    });

    let mut idle_since = None;
    loop {
        loop {
            match listener.accept() {
                Ok((stream, _)) => daemon.accept(stream),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    diagnose("accept", &error);
                    break;
                }
            }
        }
        let mut drained = [0; 64];
        while matches!((&woken).read(&mut drained), Ok(len) if len > 0) {}
        let timeout = if daemon.idle() {
            let since = *idle_since.get_or_insert_with(Instant::now);
            let left = options.idle_exit.saturating_sub(since.elapsed());
            if left.is_zero() {
                break;
            }
            Some(left)
        } else {
            idle_since = None;
            None
        };
        wait_readable(&[listener.as_raw_fd(), woken.as_raw_fd()], timeout);
    }
    let _ = std::fs::remove_file(&socket);
    drop(lock);
    Ok(())
}

struct Daemon {
    sessions: Mutex<HashMap<u64, Arc<Hosted>>>,
    connections: AtomicUsize,
    /// Wakes the listener to recheck whether the daemon is idle.
    wake: UnixStream,
    owner: u32,
    /// The protocol version it speaks.
    version: u32,
    /// The client that took over last, by its takeover connection.
    client: Mutex<Option<(u64, UnixStream)>>,
    /// Tells connections apart.
    serial: AtomicU64,
}

/// A session the daemon runs with its authoritative engine.
struct Hosted {
    id: u64,
    session: TerminalSession,
    data: Mutex<Vec<u8>>,
    /// The connection that streams the session, if one does.
    streaming: Mutex<Option<(u64, UnixStream)>>,
}

/// Counts a connection until its thread ends.
struct Connection(Arc<Daemon>);

impl Drop for Connection {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::AcqRel);
        self.0.changed();
    }
}

/// Operation and error kind only: never contents, input, titles or paths.
fn diagnose(operation: &str, error: &io::Error) {
    eprintln!("neptune session daemon: {operation}: {:?}", error.kind());
}

fn wait_readable(fds: &[libc::c_int], timeout: Option<Duration>) {
    let mut descriptors: Vec<libc::pollfd> = fds
        .iter()
        .map(|&fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    let timeout = timeout.map_or(-1, |timeout| {
        // Round up so an idle daemon does not wake early and spin.
        (timeout.as_micros().div_ceil(1000)).min(i32::MAX as u128) as libc::c_int
    });
    unsafe { libc::poll(descriptors.as_mut_ptr(), descriptors.len() as _, timeout) };
}

fn new_id(sessions: &HashMap<u64, Arc<Hosted>>) -> u64 {
    // Unpredictable across daemons, so a saved id never names another
    // daemon's session.
    let mut bytes = [0; 8];
    loop {
        if File::open("/dev/urandom")
            .and_then(|mut random| random.read_exact(&mut bytes))
            .is_err()
        {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            bytes = (nanos ^ (u64::from(std::process::id()) << 32)).to_le_bytes();
        }
        let id = u64::from_le_bytes(bytes);
        if id != 0 && !sessions.contains_key(&id) {
            return id;
        }
    }
}

fn exit_of(status: &SessionStatus) -> Option<Exit> {
    match status {
        SessionStatus::Exited { code, signal } => Some(Exit {
            code: *code,
            signal: signal.clone(),
        }),
        _ => None,
    }
}

fn failed(error: SessionError) -> Reply {
    Reply::Failed {
        kind: error.kind(),
        detail: error.to_string(),
    }
}

fn not_found() -> Reply {
    Reply::Failed {
        kind: SessionErrorKind::NotFound,
        detail: "The session daemon has no such session".into(),
    }
}

fn send(stream: &mut UnixStream, reply: &Reply) -> io::Result<()> {
    stream.write_all(&reply.encode())
}

impl Daemon {
    fn changed(&self) {
        let _ = (&self.wake).write(&[1]);
    }

    fn idle(&self) -> bool {
        let sessions = self.sessions.lock();
        sessions.is_empty() && self.connections.load(Ordering::Acquire) == 0
    }

    fn accept(self: &Arc<Self>, stream: UnixStream) {
        if let Err(error) = stream
            .set_nonblocking(false)
            .and_then(|()| check_peer(&stream, self.owner))
        {
            diagnose("accept", &error);
            return;
        }
        self.connections.fetch_add(1, Ordering::AcqRel);
        let connection = Connection(self.clone());
        let daemon = self.clone();
        if let Err(error) = thread::Builder::new()
            .name("session-daemon-client".into())
            .spawn(move || {
                let _connection = connection;
                daemon.serve(stream);
            })
        {
            diagnose("start connection", &error);
        }
    }

    fn get(&self, id: u64) -> Option<Arc<Hosted>> {
        self.sessions.lock().get(&id).cloned()
    }

    fn close(&self, id: u64) -> bool {
        let hosted = self.sessions.lock().remove(&id);
        self.changed();
        match hosted {
            Some(hosted) => {
                hosted.session.shutdown();
                true
            }
            None => false,
        }
    }

    /// An ended session is released once a client has taken its end.
    fn release(&self, hosted: &Arc<Hosted>) {
        let mut sessions = self.sessions.lock();
        if sessions
            .get(&hosted.id)
            .is_some_and(|current| Arc::ptr_eq(current, hosted))
        {
            sessions.remove(&hosted.id);
        }
        drop(sessions);
        self.changed();
    }

    fn entries(&self) -> Vec<Entry> {
        self.sessions
            .lock()
            .values()
            .map(|hosted| Entry {
                id: hosted.id,
                exit: exit_of(&hosted.session.metadata().status),
                data: hosted.data.lock().clone(),
            })
            .collect()
    }

    fn serve(self: &Arc<Self>, mut stream: UnixStream) {
        let request = |stream: &mut UnixStream| -> Option<Request> {
            match protocol::read_frame(stream, MAX_CLIENT_FRAME)
                .and_then(|frame| frame.map(|frame| Request::decode(&frame)).transpose())
            {
                Ok(request) => request,
                Err(error) => {
                    diagnose("read request", &error);
                    None
                }
            }
        };
        let reply = match request(&mut stream) {
            Some(Request::Hello { version }) if version == self.version => {
                Reply::Welcome { version }
            }
            Some(Request::Hello { .. }) => {
                let _ = send(
                    &mut stream,
                    &Reply::Refused {
                        version: self.version,
                    },
                );
                return;
            }
            Some(_) => {
                diagnose("read hello", &io::ErrorKind::InvalidData.into());
                return;
            }
            None => return,
        };
        if let Err(error) = send(&mut stream, &reply) {
            diagnose("write reply", &error);
            return;
        }
        while let Some(request) = request(&mut stream) {
            let reply = match request {
                Request::List => Reply::Sessions(self.entries()),
                Request::SetData { data, .. } if data.len() > MAX_DATA => {
                    failed(SessionError::new(
                        SessionErrorKind::InvalidOptions,
                        "Session data exceeds 64 KiB",
                    ))
                }
                Request::SetData { id, data } => match self.get(id) {
                    Some(hosted) => {
                        *hosted.data.lock() = data;
                        Reply::Done
                    }
                    None => not_found(),
                },
                Request::Close { id } => match self.close(id) {
                    true => Reply::Done,
                    false => not_found(),
                },
                Request::Start(options) => match TerminalSession::spawn(options, Arc::new(|| {})) {
                    Ok(session) => {
                        let mut sessions = self.sessions.lock();
                        let id = new_id(&sessions);
                        let hosted = Arc::new(Hosted {
                            id,
                            session,
                            data: Mutex::new(Vec::new()),
                            streaming: Mutex::new(None),
                        });
                        sessions.insert(id, hosted.clone());
                        drop(sessions);
                        return self.stream(stream, hosted, None);
                    }
                    Err(error) => failed(error),
                },
                Request::Attach { id, cols, rows } => match self.get(id) {
                    Some(hosted) => return self.stream(stream, hosted, Some((cols, rows))),
                    None => not_found(),
                },
                Request::TakeOver => return self.take_over(stream),
                _ => {
                    diagnose("read request", &io::ErrorKind::InvalidData.into());
                    return;
                }
            };
            if let Err(error) = send(&mut stream, &reply) {
                diagnose("write reply", &error);
                return;
            }
        }
    }

    /// Send the session's snapshot, then its output, while forwarding the
    /// client's input. The connection closing detaches.
    fn stream(
        self: &Arc<Self>,
        mut stream: UnixStream,
        hosted: Arc<Hosted>,
        size: Option<(u16, u16)>,
    ) {
        let session = &hosted.session;
        // An ended session keeps its final size: its terminal is gone, also
        // when its program ends just as it is resized.
        if let Some((cols, rows)) = size
            && session.metadata().status == SessionStatus::Running
        {
            let current = *session.shared.size.lock();
            if (current.cols, current.rows) != (cols, rows)
                && let Err(error) = session.resize(cols, rows, 0, 0)
                && error.kind() != SessionErrorKind::Closed
            {
                let _ = send(&mut stream, &failed(error));
                return;
            }
        }
        let (snapshot, tap) = session.snapshot(|| session.shared.output_tap.lock().subscribe());
        let metadata = session.metadata();
        let attached = AttachedFrame {
            id: hosted.id,
            process_id: metadata.process_id,
            shell: metadata.shell,
            cwd: metadata.cwd,
            scrollback: session.shared.config.lock().scrolling_history as u32,
            data: hosted.data.lock().clone(),
            snapshot,
        };
        let serial = self.serial.fetch_add(1, Ordering::Relaxed);
        let writer = match send(&mut stream, &Reply::Attached(Box::new(attached)))
            .and_then(|()| stream.try_clone())
            .and_then(|writer| Ok((writer, stream.try_clone()?)))
        {
            Ok((writer, streaming)) => {
                // An earlier connection lost the session when this one
                // subscribed; one that stopped reading is let go of too.
                if let Some((_, earlier)) = hosted.streaming.lock().replace((serial, streaming)) {
                    let _ = earlier.shutdown(std::net::Shutdown::Both);
                }
                writer
            }
            Err(error) => {
                diagnose("write snapshot", &error);
                session.shared.output_tap.lock().unsubscribe(&tap);
                return;
            }
        };
        let output = {
            let daemon = self.clone();
            let hosted = hosted.clone();
            let tap = tap.clone();
            thread::Builder::new()
                .name("session-daemon-output".into())
                .spawn(move || daemon.send_output(writer, &hosted, &tap))
        };
        match output {
            Ok(output) => {
                self.forward_input(&mut stream, &hosted);
                session.shared.output_tap.lock().unsubscribe(&tap);
                let _ = stream.shutdown(std::net::Shutdown::Both);
                let _ = output.join();
            }
            Err(error) => {
                diagnose("start output", &error);
                session.shared.output_tap.lock().unsubscribe(&tap);
            }
        }
        let mut streaming = hosted.streaming.lock();
        if streaming
            .as_ref()
            .is_some_and(|(current, _)| *current == serial)
        {
            *streaming = None;
        }
    }

    /// Make this connection's client the daemon's client. The earlier one is
    /// told, and every session's stream is let go of, so a client that
    /// stopped reading never holds a program or the daemon. The connection is
    /// held until its client goes.
    fn take_over(self: &Arc<Self>, mut stream: UnixStream) {
        let serial = self.serial.fetch_add(1, Ordering::Relaxed);
        let held = match stream.try_clone() {
            Ok(held) => held,
            Err(error) => return diagnose("take over", &error),
        };
        {
            let mut client = self.client.lock();
            if let Some((_, earlier)) = client.replace((serial, held)) {
                // One short frame, on a connection its client never writes
                // to, so it never waits for a client that stopped reading.
                if let Err(error) = earlier
                    .set_nonblocking(true)
                    .and_then(|()| (&earlier).write_all(&Reply::Replaced.encode()))
                {
                    diagnose("tell replaced", &error);
                }
                let _ = earlier.shutdown(std::net::Shutdown::Read);
            }
            for hosted in self.sessions.lock().values() {
                if let Some((_, streaming)) = hosted.streaming.lock().take() {
                    let _ = streaming.shutdown(std::net::Shutdown::Both);
                }
            }
            if let Err(error) = send(&mut stream, &Reply::Done) {
                diagnose("write reply", &error);
            }
        }
        // Nothing more is read from it; it ends when its client goes or a
        // newer client takes over.
        let mut ignored = [0; 64];
        while matches!(stream.read(&mut ignored), Ok(len) if len > 0) {}
        let mut client = self.client.lock();
        if client
            .as_ref()
            .is_some_and(|(current, _)| *current == serial)
        {
            *client = None;
        }
    }

    fn send_output(&self, mut stream: UnixStream, hosted: &Arc<Hosted>, tap: &Arc<Tap>) {
        loop {
            let result = match tap.next() {
                Tapped::Bytes(bytes) => send(&mut stream, &Reply::Output(bytes)),
                Tapped::Ended => {
                    // The session stays until the client says it took the
                    // end: one that is leaving may never show it. Its input
                    // thread hears that, or the connection closing.
                    let exit = exit_of(&hosted.session.metadata().status);
                    if send(&mut stream, &Reply::Ended(exit)).is_ok() {
                        hosted.session.shared.output_tap.lock().unsubscribe(tap);
                        return;
                    }
                    break;
                }
                Tapped::Closed => break,
            };
            if let Err(error) = result {
                if error.kind() != io::ErrorKind::BrokenPipe {
                    diagnose("write output", &error);
                }
                break;
            }
        }
        hosted.session.shared.output_tap.lock().unsubscribe(tap);
        // Unblock the input thread, whose client may still be connected.
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }

    fn forward_input(&self, stream: &mut UnixStream, hosted: &Arc<Hosted>) {
        let session = &hosted.session;
        loop {
            let request = match protocol::read_frame(stream, MAX_CLIENT_FRAME)
                .and_then(|frame| frame.map(|frame| Request::decode(&frame)).transpose())
            {
                Ok(Some(request)) => request,
                Ok(None) => return,
                Err(error) => {
                    if !matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionReset | io::ErrorKind::NotConnected
                    ) {
                        diagnose("read input", &error);
                    }
                    return;
                }
            };
            let result = match request {
                Request::Input(bytes) => write_input(session, &bytes),
                Request::Resize {
                    cols,
                    rows,
                    pixel_width,
                    pixel_height,
                } => session.resize(cols, rows, pixel_width, pixel_height),
                Request::Focus(focused) => session.focus(focused),
                Request::End => {
                    self.close(hosted.id);
                    return;
                }
                Request::Taken => {
                    if exit_of(&session.metadata().status).is_some() {
                        self.release(hosted);
                    }
                    return;
                }
                _ => {
                    diagnose("read input", &io::ErrorKind::InvalidData.into());
                    return;
                }
            };
            if let Err(error) = result
                && error.kind() != SessionErrorKind::Closed
            {
                eprintln!("neptune session daemon: forward input: {:?}", error.kind());
            }
        }
    }
}

/// The client already bounds its pending input; wait out this engine's queue.
fn write_input(session: &TerminalSession, bytes: &[u8]) -> std::result::Result<(), SessionError> {
    loop {
        match session.write(bytes) {
            Err(error) if error.kind() == SessionErrorKind::Backpressure => {
                thread::sleep(Duration::from_millis(2));
            }
            result => return result,
        }
    }
}
