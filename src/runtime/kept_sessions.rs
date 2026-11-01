//! Kept sessions run in the data root's session daemon, so they outlive
//! Neptune. Connecting, launching the daemon, starting and attaching all run
//! on startup workers, never on a frame. Unix only; elsewhere every request
//! fails and Neptune keeps its sessions in process.
use neptune_model::SessionRef;
use std::path::{Path, PathBuf};
use terminal_core::{SessionOptions, TerminalSession};

type Repaint = std::sync::Arc<dyn Fn() + Send + Sync>;

/// The private mode that runs a session daemon: `--session-daemon DIR`.
pub const DAEMON_MODE: &str = "--session-daemon";

/// Where a data root's session daemons listen.
pub fn directory(data_root: &Path) -> PathBuf {
    data_root.join("sessions")
}

/// Runs the session daemon mode, if these are its arguments. It leaves the
/// session and process group of whatever launched it, so closing a terminal
/// that launched Neptune doesn't end it.
pub fn cli(args: &[String]) -> Option<i32> {
    if args.first().map(String::as_str) != Some(DAEMON_MODE) {
        return None;
    }
    let Some(dir) = args.get(1) else {
        eprintln!("neptune session daemon: {DAEMON_MODE} needs a directory");
        return Some(2);
    };
    #[cfg(unix)]
    {
        close_inherited();
        // SAFETY: setsid has no memory effects; it fails harmlessly if this
        // process already leads a session.
        unsafe { libc::setsid() };
        Some(
            match terminal_core::daemon::run(
                Path::new(dir),
                terminal_core::daemon::DaemonOptions::default(),
            ) {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("neptune session daemon: run: {:?}", error.kind());
                    1
                }
            },
        )
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        eprintln!("neptune session daemon: this platform keeps sessions in Neptune");
        Some(2)
    }
}

/// The daemon outlives whatever launched Neptune, so it lets go of every
/// descriptor it inherited beyond the standard three (a launcher's lock or
/// terminal, say). Runs before any thread starts.
#[cfg(unix)]
fn close_inherited() {
    let listed = if cfg!(target_os = "linux") {
        "/proc/self/fd"
    } else {
        "/dev/fd"
    };
    let inherited: Vec<i32> = std::fs::read_dir(listed)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .filter(|fd| *fd > 2)
        .collect();
    for fd in inherited {
        // SAFETY: nothing in this process uses these descriptors yet; the
        // one that listed them is already closed, which close reports.
        unsafe { libc::close(fd) };
    }
}

/// The environment a kept session's program starts with. The session daemon
/// may have been started by an earlier launch, so Neptune passes its own
/// environment rather than letting the program inherit the daemon's. The
/// terminal's identity is terminal-core's to set, as for a local session.
fn with_environment(mut options: SessionOptions) -> SessionOptions {
    const TERMINAL: [&str; 4] = ["TERM", "COLORTERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION"];
    let explicit = std::mem::take(&mut options.env);
    options.env = std::env::vars()
        .filter(|(key, _)| !TERMINAL.contains(&key.as_str()))
        .chain(explicit)
        .collect();
    options
}

#[cfg_attr(not(unix), allow(dead_code))]
pub struct KeptSessions {
    dir: PathBuf,
    /// The `neptune` executable that launches a daemon when none serves
    /// `dir`; `None` only connects.
    program: Option<PathBuf>,
    #[cfg(unix)]
    client: std::sync::Mutex<Option<std::sync::Arc<terminal_core::daemon::DaemonClient>>>,
}

impl KeptSessions {
    pub fn new(dir: PathBuf, program: Option<PathBuf>) -> Self {
        Self {
            dir,
            program,
            #[cfg(unix)]
            client: Default::default(),
        }
    }

    /// Start a program in the daemon. The reference names its session there.
    pub fn start(
        &self,
        options: SessionOptions,
        repaint: Repaint,
    ) -> Result<(TerminalSession, SessionRef), String> {
        #[cfg(unix)]
        {
            let options = with_environment(options);
            let (session, id) =
                self.request(|client| client.start(options.clone(), repaint.clone()))?;
            Ok((
                session,
                SessionRef {
                    daemon: terminal_core::daemon::PROTOCOL_VERSION,
                    id: id.to_string(),
                },
            ))
        }
        #[cfg(not(unix))]
        {
            let _ = (options, repaint, with_environment);
            Err(UNSUPPORTED.into())
        }
    }

    /// Reattach to a kept session; it takes this size.
    pub fn attach(
        &self,
        reference: &SessionRef,
        cols: u16,
        rows: u16,
        repaint: Repaint,
    ) -> Result<TerminalSession, String> {
        #[cfg(unix)]
        {
            use terminal_core::daemon::{PROTOCOL_VERSION, SessionId};
            if reference.daemon != PROTOCOL_VERSION {
                return Err(format!(
                    "This session runs in a session daemon of protocol version {}",
                    reference.daemon
                ));
            }
            let id: SessionId = reference.id.parse().map_err(|error| format!("{error:#}"))?;
            self.request(|client| client.attach(id, cols, rows, repaint.clone()))
                .map(|attached| attached.session)
        }
        #[cfg(not(unix))]
        {
            let _ = (reference, cols, rows, repaint);
            Err(UNSUPPORTED.into())
        }
    }
}

impl KeptSessions {
    /// End every session the daemon keeps. Without a running daemon there is
    /// none to end, so this never launches one.
    pub fn end_all(&self) -> Result<(), String> {
        #[cfg(unix)]
        {
            use terminal_core::{SessionErrorKind, daemon::DaemonClient};
            let client = match DaemonClient::connect(&self.dir) {
                Err(error) if error.kind() == SessionErrorKind::Unavailable => return Ok(()),
                client => client.map_err(|error| format!("{error:#}"))?,
            };
            let sessions = client.list().map_err(|error| format!("{error:#}"))?;
            // One that already ended, or ends meanwhile, is gone either way.
            for session in sessions {
                let _ = client.close(session.id);
            }
        }
        Ok(())
    }
}

#[cfg(not(unix))]
const UNSUPPORTED: &str = "Keeping terminals running after quit needs macOS or Linux";

#[cfg(unix)]
mod unix {
    use super::*;
    use std::{
        process::{Child, Command, Stdio},
        sync::Arc,
        time::{Duration, Instant},
    };
    use terminal_core::{SessionError, SessionErrorKind, daemon::DaemonClient};

    /// How long a launched daemon may take to listen.
    const LAUNCH_TIMEOUT: Duration = Duration::from_secs(5);
    const LAUNCH_FAILED: &str = "The session daemon could not be launched";

    impl KeptSessions {
        /// Run a request on the connected client, connecting first if need
        /// be. A daemon that went away since is reconnected, or relaunched,
        /// once. The lock is held only to connect, so that both startup
        /// workers can attach at once.
        pub(super) fn request<T>(
            &self,
            run: impl Fn(&DaemonClient) -> Result<T, SessionError>,
        ) -> Result<T, String> {
            let client = self.client(None)?;
            let result = match run(&client) {
                // Starting and attaching open their own connections; one
                // that failed to connect has done nothing in the daemon.
                Err(error) if error.kind() == SessionErrorKind::Unavailable => {
                    run(&*self.client(Some(&client))?)
                }
                result => result,
            };
            result.map_err(|error| format!("{error:#}"))
        }

        /// The connected client, replacing `stale` if it is still the one.
        fn client(&self, stale: Option<&Arc<DaemonClient>>) -> Result<Arc<DaemonClient>, String> {
            let mut cached = self.client.lock().map_err(|_| LAUNCH_FAILED.to_owned())?;
            match cached.as_ref() {
                Some(client) if !stale.is_some_and(|stale| Arc::ptr_eq(stale, client)) => {
                    Ok(client.clone())
                }
                _ => {
                    let client = Arc::new(self.connect()?);
                    *cached = Some(client.clone());
                    Ok(client)
                }
            }
        }

        fn connect(&self) -> Result<DaemonClient, String> {
            let error = match DaemonClient::connect(&self.dir) {
                Err(error) if error.kind() == SessionErrorKind::Unavailable => error,
                result => return result.map_err(|error| format!("{error:#}")),
            };
            let Some(program) = &self.program else {
                return Err(format!("{error:#}"));
            };
            let mut scoped = cfg!(target_os = "linux");
            let mut child =
                launch(program, &self.dir, scoped).map_err(|_| LAUNCH_FAILED.to_owned())?;
            let deadline = Instant::now() + LAUNCH_TIMEOUT;
            loop {
                std::thread::sleep(Duration::from_millis(20));
                match DaemonClient::connect(&self.dir) {
                    Err(error)
                        if error.kind() == SessionErrorKind::Unavailable
                            && Instant::now() < deadline => {}
                    result => {
                        reap(child);
                        return result.map_err(|error| format!("{error:#}"));
                    }
                }
                // Without a user service manager, launch the daemon directly.
                if scoped && matches!(child.try_wait(), Ok(Some(status)) if !status.success()) {
                    scoped = false;
                    child =
                        launch(program, &self.dir, false).map_err(|_| LAUNCH_FAILED.to_owned())?;
                }
            }
        }
    }

    /// Launch `program` as the session daemon for `dir`. On Linux it runs in
    /// a transient scope of the user's service manager, so the scope of the
    /// app or terminal that launched Neptune can end without ending it.
    fn launch(program: &Path, dir: &Path, scoped: bool) -> std::io::Result<Child> {
        use std::os::unix::process::CommandExt;
        let mut command = if scoped {
            let mut command = Command::new("systemd-run");
            command
                .args(["--user", "--scope", "--collect", "--quiet", "--"])
                .arg(program);
            command
        } else {
            Command::new(program)
        };
        command
            .arg(DAEMON_MODE)
            .arg(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Its own session from the start, so a hangup of the terminal that
        // launched Neptune never reaches it. A process group leader could
        // not start one.
        // SAFETY: setsid is async-signal-safe and touches no memory.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        command.spawn()
    }

    /// The daemon outlives Neptune; while Neptune runs, collect its exit.
    fn reap(mut child: Child) {
        let _ = std::thread::Builder::new()
            .name("neptune-session-daemon".into())
            .spawn(move || child.wait());
    }
}
