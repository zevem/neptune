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
mod protocol;
mod server;
pub use client::{Attached, DaemonClient, SessionEntry};
pub use server::run;

/// The protocol this build's client and daemon speak.
pub const PROTOCOL_VERSION: u32 = 1;

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
    fn from_str(text: &str) -> std::result::Result<Self, SessionError> {
        u64::from_str_radix(text, 16)
            .ok()
            .filter(|_| text.len() == 16)
            .map(Self)
            .ok_or_else(|| SessionError::new(SessionErrorKind::NotFound, "Invalid session id"))
    }
}

#[derive(Debug, Clone)]
pub struct DaemonOptions {
    /// How long the daemon waits, with no session and no client, before exiting.
    pub idle_exit: Duration,
}

impl Default for DaemonOptions {
    fn default() -> Self {
        Self {
            idle_exit: Duration::from_secs(10),
        }
    }
}

fn socket_path(dir: &Path, version: u32) -> PathBuf {
    dir.join(format!("session-daemon-v{version}.sock"))
}

fn lock_path(dir: &Path, version: u32) -> PathBuf {
    dir.join(format!("session-daemon-v{version}.lock"))
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

/// Create the directory, or close an existing one of this user's to others.
fn make_private_dir(dir: &Path) -> io::Result<()> {
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
