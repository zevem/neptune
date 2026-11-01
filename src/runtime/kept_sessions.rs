//! Kept sessions run in the data root's session daemon, so they outlive
//! Neptune. Connecting, launching the daemon, starting and attaching all run
//! on startup workers, never on a frame. Unix only; elsewhere every request
//! fails and Neptune keeps its sessions in process.
use neptune_model::SessionRef;
use std::path::{Path, PathBuf};
use terminal_core::{SessionOptions, TerminalSession};

type Repaint = std::sync::Arc<dyn Fn() + Send + Sync>;

/// What Neptune keeps with a session in the daemon, for the launch that
/// reattaches it. Fields a later build adds are ignored by this one.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionData {
    /// What the terminal's agents reach Neptune with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<super::agents::Credential>,
    /// The connection sharing of an SSH terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<SshData>,
}
impl SessionData {
    #[cfg_attr(not(unix), allow(dead_code))]
    fn decode(bytes: &[u8]) -> Self {
        serde_json::from_slice(bytes).unwrap_or_default()
    }
}

/// An SSH terminal's control socket, which lives with its session, and the
/// forwards Neptune made through it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SshData {
    pub control: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwards: Vec<super::ssh_control::KeptForward>,
}

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
    Some(run_daemon(Path::new(dir)))
}

#[cfg(unix)]
fn run_daemon(dir: &Path) -> i32 {
    close_inherited();
    // SAFETY: setsid has no memory effects; it fails harmlessly if this
    // process already leads a session.
    unsafe { libc::setsid() };
    match terminal_core::daemon::run(dir, terminal_core::daemon::DaemonOptions::default()) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("neptune session daemon: run: {:?}", error.kind());
            1
        }
    }
}

/// Whether a session daemon can listen in `dir`, whose socket path must fit
/// in its address. A data root deep enough to break that keeps its terminals
/// in Neptune.
pub fn fits(dir: &Path) -> bool {
    #[cfg(unix)]
    return terminal_core::daemon::fits(dir);
    #[cfg(not(unix))]
    {
        let _ = dir;
        false
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

/// What a launch that was taken over is told when it asks for a session.
const REPLACED: &str = "A newer Neptune took over these terminals";

/// A launch's hold on its data root's session daemon. Holding it takes over
/// from an earlier launch, which closes its windows and saves nothing more;
/// a newer launch takes over from this one in turn.
#[cfg_attr(not(unix), allow(dead_code))]
pub struct Claim {
    /// Runs once a newer launch takes over.
    wake: Repaint,
    replaced: std::sync::Arc<std::sync::atomic::AtomicBool>,
    #[cfg(unix)]
    held: std::sync::Mutex<Option<terminal_core::daemon::Takeover>>,
}

impl Default for Claim {
    fn default() -> Self {
        Self::new(std::sync::Arc::new(|| {}))
    }
}

impl Claim {
    pub fn new(wake: Repaint) -> Self {
        Self {
            wake,
            replaced: Default::default(),
            #[cfg(unix)]
            held: Default::default(),
        }
    }

    /// Whether a newer launch took over. True as soon as that launch holds
    /// the daemon.
    pub fn replaced(&self) -> bool {
        use std::sync::atomic::Ordering;
        #[cfg(unix)]
        if !self.replaced.load(Ordering::Acquire)
            // A hold under way waits for the daemon; until then the flag
            // answers.
            && let Ok(held) = self.held.try_lock()
            && held.as_ref().is_some_and(|held| held.replaced())
        {
            self.replaced.store(true, Ordering::Release);
        }
        self.replaced.load(Ordering::Acquire)
    }
}

/// The sessions running in a data root's session daemons, as a launch finds
/// them when it starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Live {
    /// The sessions of every daemon that could be asked.
    pub sessions: Vec<SessionRef>,
    /// Protocol versions whose daemon is there but couldn't be asked: which
    /// of their sessions run is unknown.
    pub unlisted: Vec<u32>,
}

/// The sessions running in the session daemons that serve `dir`, of this
/// build's protocol version and every earlier one it attaches to; without a
/// daemon there are none. Once every daemon there could be asked, the SSH
/// control folders that none of their sessions refers to any more, left by a
/// logout or by a session that ended unseen, are removed. Never launches a
/// daemon.
pub fn live(dir: &Path) -> Live {
    KeptSessions::new(dir.to_owned(), None, Default::default()).live()
}

/// Kept sessions run in the session daemons of one data root: new ones in the
/// daemon of this build's protocol version, which Neptune launches when need
/// be, while daemons of earlier versions, left by the build Neptune updated
/// from, keep serving the sessions they have until those end.
#[cfg_attr(not(unix), allow(dead_code))]
pub struct KeptSessions {
    dir: PathBuf,
    /// The `neptune` executable that launches a daemon when none serves
    /// `dir`; `None` only connects.
    program: Option<PathBuf>,
    /// Every daemon this launch connects to, it takes over.
    claim: std::sync::Arc<Claim>,
    /// The protocol version new sessions start in.
    #[cfg(unix)]
    current: u32,
    /// Earlier protocol versions whose daemons Neptune still attaches to.
    #[cfg(unix)]
    earlier: &'static [u32],
    /// The current version's daemon. An earlier daemon gets a connection
    /// only per request, so it can exit once its sessions have ended.
    #[cfg(unix)]
    client: std::sync::Mutex<Option<std::sync::Arc<terminal_core::daemon::DaemonClient>>>,
}

impl KeptSessions {
    pub fn new(dir: PathBuf, program: Option<PathBuf>, claim: std::sync::Arc<Claim>) -> Self {
        Self {
            dir,
            program,
            claim,
            #[cfg(unix)]
            current: terminal_core::daemon::PROTOCOL_VERSION,
            #[cfg(unix)]
            earlier: terminal_core::daemon::EARLIER_PROTOCOL_VERSIONS,
            #[cfg(unix)]
            client: Default::default(),
        }
    }

    /// Kept sessions of a build whose protocol version is `current`, which
    /// attaches to daemons of `earlier` versions; never launches a daemon.
    #[cfg(all(test, unix))]
    pub fn with_versions(dir: PathBuf, current: u32, earlier: &'static [u32]) -> Self {
        Self {
            current,
            earlier,
            ..Self::new(dir, None, Default::default())
        }
    }

    /// Where kept SSH terminals' control sockets live, each in a folder of
    /// its own: the daemons' own folder, whose path is short enough for a
    /// socket to fit in its address with a long user name.
    pub fn ssh_directory(&self) -> PathBuf {
        self.dir.clone()
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::{
        process::{Child, Command, Stdio},
        sync::Arc,
        time::{Duration, Instant},
    };
    use terminal_core::{
        SessionError, SessionErrorKind,
        daemon::{DaemonClient, SessionId},
    };

    /// How long a launched daemon may take to listen.
    const LAUNCH_TIMEOUT: Duration = Duration::from_secs(5);
    const LAUNCH_FAILED: &str = "The session daemon could not be launched";

    impl Claim {
        /// Take over the daemons serving `dir` that run; none is launched. An
        /// earlier build may still be open on an earlier version's daemon, so
        /// those are taken over too.
        pub fn take_over_running(&self, dir: &Path) -> Result<(), String> {
            use terminal_core::daemon::{EARLIER_PROTOCOL_VERSIONS, PROTOCOL_VERSION};
            self.take_over_versions(dir, PROTOCOL_VERSION, EARLIER_PROTOCOL_VERSIONS)
        }

        /// Hold the daemon of `current`, and take over those of `earlier`
        /// versions only to tell whoever holds them: holding an earlier daemon
        /// would keep it from exiting once its sessions end. A newer launch
        /// takes over this build's daemon too, which tells this one.
        pub(in crate::runtime) fn take_over_versions(
            &self,
            dir: &Path,
            current: u32,
            earlier: &[u32],
        ) -> Result<(), String> {
            let running = |version| match DaemonClient::connect_to(dir, version) {
                Err(error) if error.kind() == SessionErrorKind::Unavailable => Ok(None),
                client => client.map(Some).map_err(|error| format!("{error:#}")),
            };
            let mut result = match running(current) {
                Ok(Some(client)) => self.hold(&client),
                other => other.map(drop),
            };
            if self.replaced() {
                return Err(REPLACED.into());
            }
            for version in earlier {
                let told = running(*version).and_then(|client| match client {
                    Some(client) => client
                        .take_over(Arc::new(|| {}))
                        .map(drop)
                        .map_err(|error| format!("{error:#}")),
                    None => Ok(()),
                });
                result = result.and(told);
            }
            result
        }

        /// Hold the daemon `client` reaches, unless this launch already holds it.
        fn hold(&self, client: &DaemonClient) -> Result<(), String> {
            use std::sync::atomic::Ordering;
            let mut held = self.held.lock().map_err(|_| REPLACED.to_owned())?;
            if self.replaced.load(Ordering::Acquire)
                || held.as_ref().is_some_and(|held| held.replaced())
            {
                self.replaced.store(true, Ordering::Release);
                return Err(REPLACED.into());
            }
            // A daemon that went away was replaced by the one `client` reaches.
            if held
                .as_ref()
                .is_some_and(terminal_core::daemon::Takeover::held)
            {
                return Ok(());
            }
            let replaced = self.replaced.clone();
            let wake = self.wake.clone();
            *held = Some(
                client
                    .take_over(Arc::new(move || {
                        replaced.store(true, Ordering::Release);
                        wake();
                    }))
                    .map_err(|error| format!("{error:#}"))?,
            );
            Ok(())
        }
    }

    impl KeptSessions {
        /// See [`live`].
        pub fn live(&self) -> Live {
            let mut live = Live::default();
            // The folder of each SSH terminal's control socket that a session
            // still refers to.
            let mut controls = Vec::new();
            for version in self.versions() {
                let listed = match self.connect_to(version) {
                    Err(error) if error.kind() == SessionErrorKind::Unavailable => continue,
                    client => client.and_then(|client| client.list()),
                };
                // A daemon that can't be asked holds back no other.
                let Ok(entries) = listed else {
                    live.unlisted.push(version);
                    continue;
                };
                for entry in entries {
                    let data = SessionData::decode(&entry.data);
                    controls.extend(
                        data.ssh
                            .and_then(|ssh| ssh.control.parent().map(Path::to_owned)),
                    );
                    live.sessions.push(SessionRef {
                        daemon: version,
                        id: entry.id.to_string(),
                    });
                }
            }
            if live.unlisted.is_empty() {
                self.remove_ssh_directories_except(&controls);
            }
            live
        }

        /// Remove every SSH control folder but `kept`. Neptune's SSH folders
        /// are the only folders in the daemons' own; the daemons keep only
        /// files there.
        fn remove_ssh_directories_except(&self, kept: &[PathBuf]) {
            let Ok(entries) = std::fs::read_dir(self.ssh_directory()) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) && !kept.contains(&path) {
                    let _ = std::fs::remove_dir_all(path);
                }
            }
        }

        /// Start a program in the daemon. The reference names its session there.
        pub fn start(
            &self,
            options: SessionOptions,
            repaint: Repaint,
        ) -> Result<(TerminalSession, SessionRef), String> {
            let options = with_environment(options);
            let (session, id) =
                self.request(|client| client.start(options.clone(), repaint.clone()))?;
            Ok((
                session,
                SessionRef {
                    daemon: self.current,
                    id: id.to_string(),
                },
            ))
        }

        /// Keep `data` with a session, in place of what it had.
        pub fn set_data(&self, reference: &SessionRef, data: &SessionData) -> Result<(), String> {
            let bytes = serde_json::to_vec(data).map_err(|error| error.to_string())?;
            self.on(reference, |client, id| client.set_data(id, &bytes))
        }

        /// Reattach to a kept session, with what Neptune kept with it; it
        /// takes this size.
        pub fn attach(
            &self,
            reference: &SessionRef,
            cols: u16,
            rows: u16,
            repaint: Repaint,
        ) -> Result<(TerminalSession, SessionData), String> {
            self.on(reference, |client, id| {
                client.attach(id, cols, rows, repaint.clone())
            })
            .map(|attached| (attached.session, SessionData::decode(&attached.data)))
        }

        /// Connect to the daemon now, launching it if need be, so that this
        /// launch holds it before any terminal starts: a newer launch then
        /// always reaches this one to take over.
        pub fn connect_now(&self) -> Result<(), String> {
            if self.claim.replaced() {
                return Err(REPLACED.into());
            }
            self.client(None).map(drop)
        }

        /// End every session the daemons keep, those of earlier versions too,
        /// each daemon's at once. Without a running daemon there is none to
        /// end, so this never launches one. A daemon that can't be asked
        /// doesn't stop the others' ending.
        pub fn end_all(&self) -> Result<(), String> {
            // They are a newer launch's now.
            if self.claim.replaced() {
                return Err(REPLACED.into());
            }
            let mut failure = Ok(());
            for version in self.versions() {
                let ended = match self.connect_to(version) {
                    Err(error) if error.kind() == SessionErrorKind::Unavailable => continue,
                    client => client.and_then(|client| client.end_all()),
                };
                if let Err(error) = ended {
                    failure = failure.and(Err(format!("{error:#}")));
                }
            }
            failure
        }

        /// This build's protocol version, then the earlier ones.
        pub(super) fn versions(&self) -> impl Iterator<Item = u32> + '_ {
            std::iter::once(self.current).chain(self.earlier.iter().copied())
        }

        pub(super) fn connect_to(&self, version: u32) -> Result<DaemonClient, SessionError> {
            DaemonClient::connect_to(&self.dir, version)
        }

        /// Run a request about a kept session on the daemon that has it. An
        /// earlier version's daemon is never launched: without it, the
        /// session is gone.
        pub(super) fn on<T>(
            &self,
            reference: &SessionRef,
            run: impl Fn(&DaemonClient, SessionId) -> Result<T, SessionError>,
        ) -> Result<T, String> {
            if self.claim.replaced() {
                return Err(REPLACED.into());
            }
            if !self.versions().any(|version| version == reference.daemon) {
                return Err(format!(
                    "This session runs in a session daemon of protocol version {}",
                    reference.daemon
                ));
            }
            let id: SessionId = reference.id.parse().map_err(|error| format!("{error:#}"))?;
            if reference.daemon == self.current {
                return self.request(|client| run(client, id));
            }
            self.connect_to(reference.daemon)
                .and_then(|client| run(&client, id))
                .map_err(|error| format!("{error:#}"))
        }

        /// Run a request on the connected client, connecting first if need
        /// be. A daemon that went away since is reconnected, or relaunched,
        /// once. The lock is held only to connect, so that both startup
        /// workers can attach at once.
        pub(super) fn request<T>(
            &self,
            run: impl Fn(&DaemonClient) -> Result<T, SessionError>,
        ) -> Result<T, String> {
            if self.claim.replaced() {
                return Err(REPLACED.into());
            }
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
                    self.claim.hold(&client)?;
                    *cached = Some(client.clone());
                    Ok(client)
                }
            }
        }

        fn connect(&self) -> Result<DaemonClient, String> {
            let error = match self.connect_to(self.current) {
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
                match self.connect_to(self.current) {
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

/// Elsewhere Neptune keeps every session in process: nothing is kept, found
/// or ended in a daemon.
#[cfg(not(unix))]
mod elsewhere {
    use super::*;

    const UNSUPPORTED: &str = "Keeping terminals running after quit needs macOS or Linux";

    impl Claim {
        pub fn take_over_running(&self, _: &Path) -> Result<(), String> {
            Ok(())
        }
    }

    impl KeptSessions {
        pub fn live(&self) -> Live {
            Live::default()
        }
        pub fn start(
            &self,
            _: SessionOptions,
            _: Repaint,
        ) -> Result<(TerminalSession, SessionRef), String> {
            let _ = with_environment;
            Err(UNSUPPORTED.into())
        }
        pub fn set_data(&self, _: &SessionRef, _: &SessionData) -> Result<(), String> {
            Err(UNSUPPORTED.into())
        }
        pub fn attach(
            &self,
            _: &SessionRef,
            _: u16,
            _: u16,
            _: Repaint,
        ) -> Result<(TerminalSession, SessionData), String> {
            Err(UNSUPPORTED.into())
        }
        pub fn connect_now(&self) -> Result<(), String> {
            Ok(())
        }
        pub fn end_all(&self) -> Result<(), String> {
            Ok(())
        }
    }

    pub(super) fn run_daemon(_: &Path) -> i32 {
        eprintln!("neptune session daemon: this platform keeps sessions in Neptune");
        2
    }
}
#[cfg(not(unix))]
use elsewhere::run_daemon;
