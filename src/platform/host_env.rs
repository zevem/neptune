//! Keeps AppImage launcher state out of the processes Neptune starts.
//!
//! The AppImage runtime exports `ARGV0`, `APPIMAGE`, `APPDIR` and `OWD`, and
//! `AppRun` prepends the bundled window libraries to `LD_LIBRARY_PATH`. Shells
//! and helpers would inherit all of it: zsh uses an exported `ARGV0` as
//! `argv[0]` of every command, which breaks multi-call binaries such as rustup's
//! `cargo`, and host programs would load Neptune's bundled libraries.
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    sync::OnceLock,
};

static APPIMAGE: OnceLock<Option<PathBuf>> = OnceLock::new();

/// The AppImage file this process was launched from, read before [`restore`]
/// takes it out of the environment.
pub fn appimage_path() -> Option<&'static Path> {
    APPIMAGE
        .get_or_init(|| std::env::var_os("APPIMAGE").map(PathBuf::from))
        .as_deref()
}

/// Whether this process was launched from an AppImage.
pub fn appimage() -> bool {
    appimage_path().is_some()
}

/// Restore the environment the user launched Neptune from.
///
/// The dynamic loader reads `LD_LIBRARY_PATH` once at startup, so Neptune keeps
/// resolving its bundled libraries after the variable is restored.
///
/// # Safety
/// Must run before any other thread exists; it mutates the process environment.
pub unsafe fn restore() {
    if !appimage() {
        return;
    }
    let appdir = std::env::var_os("APPDIR");
    let libraries = std::env::var_os("LD_LIBRARY_PATH")
        .zip(appdir.as_deref())
        .map(|(path, appdir)| host_library_path(&path, Path::new(appdir)));
    // SAFETY: the caller guarantees no other thread reads the environment.
    unsafe {
        match libraries {
            Some(Some(path)) => std::env::set_var("LD_LIBRARY_PATH", path),
            Some(None) => std::env::remove_var("LD_LIBRARY_PATH"),
            None => {}
        }
        for name in ["ARGV0", "APPIMAGE", "APPDIR", "OWD"] {
            std::env::remove_var(name);
        }
    }
}

/// `path` without the entries inside `appdir`; `None` when nothing remains.
fn host_library_path(path: &OsStr, appdir: &Path) -> Option<OsString> {
    let host: Vec<_> = std::env::split_paths(path)
        .filter(|entry| !entry.starts_with(appdir))
        .collect();
    if host.is_empty() {
        return None;
    }
    std::env::join_paths(host).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_libraries_leave_the_child_library_path() {
        let appdir = Path::new("/tmp/.mount_neptunAbCdEf");
        let host = |path: &str| host_library_path(OsStr::new(path), appdir);
        assert_eq!(host("/tmp/.mount_neptunAbCdEf/usr/lib"), None);
        assert_eq!(
            host("/tmp/.mount_neptunAbCdEf/usr/lib:/opt/cuda/lib64:/usr/local/lib"),
            Some("/opt/cuda/lib64:/usr/local/lib".into())
        );
        // A sibling mount of another AppImage is the user's own setting.
        assert_eq!(
            host("/tmp/.mount_neptunAbCdEfg/usr/lib"),
            Some("/tmp/.mount_neptunAbCdEfg/usr/lib".into())
        );
    }
}
