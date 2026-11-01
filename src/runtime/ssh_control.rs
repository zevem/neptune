//! An SSH terminal's connection sharing: the control socket its own ssh
//! client listens on, which port discovery and forwards go through. A kept
//! session's socket lives with the session, so it outlives Neptune, and the
//! forwards made through it are kept with the session too.
use crate::platform::ports::Listener;
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

/// A forward made through a kept session's control socket: its remote
/// listener, and the local port that reaches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeptForward {
    pub listener: Listener,
    pub local: u16,
}

/// Keeps a session's forwards with it, wherever it is kept.
pub type Record = Box<dyn Fn(&[KeptForward]) + Send + Sync>;

pub struct SshControl {
    path: PathBuf,
    owner: Owner,
}
enum Owner {
    /// A session in Neptune: a private directory, removed with the last
    /// handle.
    Neptune(#[allow(dead_code)] tempfile::TempDir),
    /// A kept session: the socket's directory stays until the session
    /// closes.
    Kept {
        directory: PathBuf,
        forwards: Mutex<Vec<KeptForward>>,
        record: Record,
    },
}

/// The name of the socket in its directory. Short, because a socket path
/// has little room (see [`fits`]).
const SOCKET: &str = "s";

impl SshControl {
    /// A control socket in a private temporary directory, for a session that
    /// ends with Neptune.
    pub fn private() -> std::io::Result<Self> {
        let directory = private_directory().prefix("neptune-ssh-").tempdir()?;
        Ok(Self {
            path: directory.path().join(SOCKET),
            owner: Owner::Neptune(directory),
        })
    }

    /// A control socket in a new directory of `parent`, for a kept session.
    /// The directory is the caller's to remove (see [`Self::remove`]); `None`
    /// if its socket path would be too long for ssh to listen on.
    pub fn reserve(parent: &Path) -> std::io::Result<Option<PathBuf>> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        // The socket is `parent/XXXXXX/s`.
        if !fits(&parent.join("XXXXXX").join(SOCKET)) {
            return Ok(None);
        }
        let directory = private_directory()
            .prefix("")
            .rand_bytes(6)
            .tempdir_in(parent)?
            .keep();
        Ok(Some(directory.join(SOCKET)))
    }

    /// A kept session's control socket at `path` (from [`Self::reserve`]),
    /// with the forwards made through it so far. `record` keeps them with the
    /// session whenever they change.
    pub fn kept(path: PathBuf, forwards: Vec<KeptForward>, record: Record) -> Option<Self> {
        let directory = path.parent()?.to_owned();
        (path.file_name()? == SOCKET).then_some(Self {
            path,
            owner: Owner::Kept {
                directory,
                forwards: Mutex::new(forwards),
                record,
            },
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The forwards made through this socket that its session keeps.
    pub fn forwards(&self) -> Vec<KeptForward> {
        match &self.owner {
            Owner::Kept { forwards, .. } => forwards.lock().map_or(Vec::new(), |f| f.clone()),
            Owner::Neptune(_) => Vec::new(),
        }
    }

    /// Keep `current` as the forwards made through this socket. It may block
    /// on the session daemon, so it runs on a worker.
    pub fn record(&self, current: &[KeptForward]) {
        if let Owner::Kept {
            forwards, record, ..
        } = &self.owner
            && let Ok(mut forwards) = forwards.lock()
            && *forwards != current
        {
            *forwards = current.to_vec();
            record(current);
        }
    }

    /// As the session closes: remove a kept socket and its directory. A
    /// socket in Neptune goes with its last handle.
    pub fn remove(&self) {
        if let Owner::Kept { directory, .. } = &self.owner {
            let _ = std::fs::remove_dir_all(directory);
        }
    }
}

/// A directory only this user can enter.
fn private_directory<'a>() -> tempfile::Builder<'a, 'a> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder
}

/// Whether ssh can listen on `path`. While it starts listening, it binds a
/// name 17 bytes longer, and a socket path must fit `sun_path` with its NUL.
fn fits(path: &Path) -> bool {
    const SUN_PATH: usize = if cfg!(any(target_os = "macos", target_os = "ios")) {
        104
    } else {
        108
    };
    path.as_os_str().len() + 17 < SUN_PATH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_path_leaves_room_for_the_name_ssh_binds_first() {
        let at = |length: usize| PathBuf::from("/".repeat(length));
        let limit = if cfg!(target_os = "macos") { 104 } else { 108 };
        assert!(fits(&at(limit - 18)));
        assert!(!fits(&at(limit - 17)));
    }

    #[cfg(unix)]
    #[test]
    fn a_kept_socket_gets_its_own_private_directory_which_close_removes() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("sessions").join("ssh");
        let first = SshControl::reserve(&parent).unwrap().unwrap();
        let second = SshControl::reserve(&parent).unwrap().unwrap();
        assert_ne!(first.parent(), second.parent());
        for directory in [parent.as_path(), first.parent().unwrap()] {
            let mode = std::fs::metadata(directory).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{directory:?}");
        }
        std::fs::write(&first, b"").unwrap();
        let control = SshControl::kept(first.clone(), Vec::new(), Box::new(|_| {})).unwrap();
        assert_eq!(control.path(), first);
        drop(control);
        assert!(first.exists(), "a detached session keeps its socket");
        SshControl::kept(first.clone(), Vec::new(), Box::new(|_| {}))
            .unwrap()
            .remove();
        assert!(!first.parent().unwrap().exists());
        assert!(second.parent().unwrap().exists());

        let long = root.path().join("l".repeat(100));
        assert_eq!(SshControl::reserve(&long).unwrap(), None);
    }
}
