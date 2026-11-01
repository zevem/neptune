//! The session daemon keeps sessions running without a client, and the client
//! that starts, lists and attaches to them. Unix only.
//!
//! The daemon owns each program's terminal and runs its authoritative engine,
//! which answers terminal queries whether or not a client is attached. A
//! client's [`TerminalSession`] is a replica: on attach it is rebuilt from a
//! snapshot, then parses the same output. Input, paste, resize and focus go
//! through the daemon; the replica's own query replies are dropped.
//!
//! Dropping a daemon-backed session detaches and leaves its program running;
//! [`TerminalSession::shutdown`] or [`DaemonClient::close`] closes it.
//!
//! The daemon listens on a socket named for its protocol version, inside a
//! directory only its user can open, and rejects peers of other users. It
//! exits [`DaemonOptions::idle_exit`] after it last had neither a session nor
//! a connected client. Its diagnostics name an operation and an error kind,
//! never terminal contents, input, titles or paths.
use super::*;
use std::fmt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use transport::Exit;

mod client;
pub(super) mod protocol;
mod server;
pub use client::{Attached, DaemonClient, SessionEntry, Takeover};
pub use server::run;

/// The protocol this build's daemon speaks, and the version new sessions
/// start in.
pub const PROTOCOL_VERSION: u32 = 1;

/// Released protocol versions before [`PROTOCOL_VERSION`], oldest first,
/// whose daemons this build's client still attaches to. Every release attaches
/// to daemons of every earlier released version, so none is ever removed: when
/// `PROTOCOL_VERSION` moves on, the version it had joins this list, its
/// encodings stay in `protocol`, and its pinned fixtures stay in
/// `tests/daemon_protocol.rs`.
pub const EARLIER_PROTOCOL_VERSIONS: &[u32] = &[];

/// Whether this build's client speaks protocol `version`.
fn attaches_to(version: u32) -> bool {
    version == PROTOCOL_VERSION
        || EARLIER_PROTOCOL_VERSIONS.contains(&version)
        || simulated::serves(version)
}

/// Test support: this build's daemon under another protocol version's name,
/// standing in for a daemon of another release, so that tests can run several
/// daemons for one directory. It speaks this build's encodings; the client in
/// the same process attaches to it. Never part of a released build.
#[cfg(any(test, feature = "simulated-daemon-versions"))]
pub mod simulated {
    use super::*;

    static VERSIONS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

    /// Run a daemon as [`super::run`] does, named and speaking as version
    /// `version`.
    pub fn run(
        dir: &Path,
        options: DaemonOptions,
        version: u32,
    ) -> std::result::Result<(), SessionError> {
        {
            let mut versions = VERSIONS.lock();
            if !versions.contains(&version) {
                versions.push(version);
            }
        }
        server::serve(dir, options, version)
    }

    /// Run a daemon of `version` on a thread of this process, as [`run`]
    /// does, and return once it listens.
    pub fn spawn(
        dir: &Path,
        options: DaemonOptions,
        version: u32,
    ) -> thread::JoinHandle<std::result::Result<(), SessionError>> {
        let serving = dir.to_owned();
        let daemon = thread::spawn(move || run(&serving, options, version));
        let deadline = Instant::now() + Duration::from_secs(5);
        while let Err(error) = DaemonClient::connect_to(dir, version) {
            assert!(
                !daemon.is_finished() && Instant::now() < deadline,
                "the session daemon never listened: {error}"
            );
            thread::sleep(Duration::from_millis(5));
        }
        daemon
    }

    pub(super) fn serves(version: u32) -> bool {
        VERSIONS.lock().contains(&version)
    }
}

#[cfg(not(any(test, feature = "simulated-daemon-versions")))]
mod simulated {
    pub(super) fn serves(_: u32) -> bool {
        false
    }
}

/// A session's identity within its session daemon. It stays the same across
/// detach and reattach; its text form round-trips through [`str::parse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(u64);

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl std::str::FromStr for SessionId {
    type Err = SessionError;
    /// Exactly 16 hex digits, as [`SessionId`] displays.
    fn from_str(text: &str) -> std::result::Result<Self, SessionError> {
        Some(text)
            .filter(|text| text.len() == 16 && text.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .and_then(|text| u64::from_str_radix(text, 16).ok())
            .map(Self)
            .ok_or_else(|| SessionError::new(SessionErrorKind::NotFound, "Invalid session id"))
    }
}

#[derive(Debug, Clone)]
pub struct DaemonOptions {
    /// How long the daemon waits, with no session and no client, before exiting.
    pub idle_exit: Duration,
    /// The most sessions it runs at once, running or ended but not yet taken.
    /// Starting another fails.
    pub max_sessions: usize,
    /// The most connections it serves at once: each attached session streams
    /// over one, besides each client's own. Another is refused.
    pub max_connections: usize,
}

impl Default for DaemonOptions {
    fn default() -> Self {
        Self {
            idle_exit: Duration::from_secs(10),
            // Neptune runs at most 64 sessions.
            max_sessions: 64,
            max_connections: 64 + 16,
        }
    }
}

/// Short names, so that the socket path fits in its address with room to
/// spare for long data roots and user names (see [`fits`]).
fn socket_path(dir: &Path, version: u32) -> PathBuf {
    dir.join(format!("v{version}"))
}

fn lock_path(dir: &Path, version: u32) -> PathBuf {
    dir.join(format!("v{version}.lock"))
}

/// Whether a session daemon can listen in `dir`: a socket's path must fit
/// in its address, whose size the platform sets (103 bytes on macOS).
pub fn fits(dir: &Path) -> bool {
    // SAFETY: an all-zero sockaddr_un is valid.
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    // One byte for the terminating NUL.
    socket_path(dir, PROTOCOL_VERSION).as_os_str().len() < address.sun_path.len()
}

fn effective_uid() -> u32 {
    unsafe { libc::geteuid() }
}

/// The directory must belong to this user and be closed to everyone else.
fn check_private_dir(dir: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(dir)?;
    if !metadata.is_dir() || metadata.uid() != effective_uid() || metadata.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "The session daemon directory must belong to this user and be private",
        ));
    }
    Ok(())
}

/// Create `dir` (and any missing parent) so that only this user can open
/// it, or close an existing directory of this user's to others. Fails if
/// `dir` is anything else, or belongs to another user. Session daemons and
/// Neptune keep their sockets in such directories.
pub fn make_private_dir(dir: &Path) -> io::Result<()> {
    if let Some(parent) = dir.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(dir)?;
            if metadata.is_dir() && metadata.uid() == effective_uid() {
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Err(error) => return Err(error),
    }
    check_private_dir(dir)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(credentials.uid)
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut uid = 0;
    let mut gid = 0;
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

/// Only processes of `owner` may be on the other end.
fn check_peer(stream: &UnixStream, owner: u32) -> io::Result<()> {
    if peer_uid(stream)? != owner {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "The peer belongs to another user",
        ));
    }
    Ok(())
}

fn unavailable(detail: impl fmt::Display) -> SessionError {
    SessionError::new(
        SessionErrorKind::Unavailable,
        format!("Session daemon unavailable: {detail}"),
    )
}

#[cfg(test)]
mod tests;
