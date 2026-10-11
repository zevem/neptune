//! Finds the profiles that hold a cookie database, and whether their browser
//! still has them open.

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

pub(super) struct Profile {
    /// Identifies the profile to callers; unique within its browser.
    pub directory: String,
    pub name: String,
    pub db: PathBuf,
    /// The symlink through which the browser claims the profile.
    pub lock: PathBuf,
}

/// Profiles under one Chromium user-data directory.
pub(super) fn chromium_profiles(root: &Path) -> Vec<Profile> {
    let profile = |directory: String, name: Option<String>| {
        let db = ["Network/Cookies", "Cookies"]
            .into_iter()
            .map(|candidate| root.join(&directory).join(candidate))
            .find(|candidate| candidate.is_file())?;
        Some(Profile {
            name: name.unwrap_or_else(|| directory.clone()),
            directory,
            db,
            // One browser process owns every profile of the directory.
            lock: root.join("SingletonLock"),
        })
    };
    let mut profiles: Vec<Profile> = local_state(root)
        .into_iter()
        .filter_map(|(directory, name)| profile(directory, name))
        .collect();
    if profiles.is_empty() {
        profiles = directories(root)
            .into_iter()
            .filter_map(|directory| profile(directory, None))
            .collect();
    }
    profiles
}

/// Directory and display name of each profile that `Local State` records.
fn local_state(root: &Path) -> Vec<(String, Option<String>)> {
    let Some(state) = fs::read(root.join("Local State"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
    else {
        return Vec::new();
    };
    let Some(cache) = state["profile"]["info_cache"].as_object() else {
        return Vec::new();
    };
    cache
        .iter()
        .filter(|(directory, _)| safe_segment(directory))
        .map(|(directory, info)| {
            let name = info["name"].as_str().map(str::trim);
            (
                directory.clone(),
                name.filter(|name| !name.is_empty()).map(str::to_owned),
            )
        })
        .collect()
}

/// A profile directory is joined to its root, so it must name a direct child.
fn safe_segment(name: &str) -> bool {
    !matches!(name, "" | "." | "..") && !name.contains(['/', '\\', '\0'])
}

/// Names of the directories directly inside `parent`, sorted.
fn directories(parent: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| safe_segment(name))
        .collect();
    names.sort();
    names
}

/// Profiles under one Firefox root.
pub(super) fn firefox_profiles(root: &Path) -> Vec<Profile> {
    let profile = |directory: String, name: Option<String>, path: PathBuf| {
        let db = path.join("cookies.sqlite");
        db.is_file().then(|| Profile {
            name: name.unwrap_or_else(|| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&directory)
                    .to_owned()
            }),
            directory,
            db,
            lock: path.join("lock"),
        })
    };
    let mut profiles: Vec<Profile> = fs::read_to_string(root.join("profiles.ini"))
        .map(|ini| profiles_ini(&ini))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|listed| {
            let path = if listed.relative {
                root.join(inside(&listed.path)?)
            } else {
                Some(PathBuf::from(&listed.path)).filter(|path| path.is_absolute())?
            };
            profile(listed.path, listed.name, path)
        })
        .collect();
    if profiles.is_empty() {
        // Linux keeps profiles beside profiles.ini; macOS and Windows one level down.
        let nested = cfg!(any(target_os = "macos", windows));
        let parent = if nested {
            root.join("Profiles")
        } else {
            root.to_path_buf()
        };
        profiles = directories(&parent)
            .into_iter()
            .filter_map(|name| {
                let path = parent.join(&name);
                let directory = if nested {
                    format!("Profiles/{name}")
                } else {
                    name
                };
                profile(directory, None, path)
            })
            .collect();
    }
    profiles
}

#[derive(Debug, PartialEq)]
struct Listed {
    name: Option<String>,
    path: String,
    relative: bool,
}

/// The `[ProfileN]` sections of a `profiles.ini`.
fn profiles_ini(ini: &str) -> Vec<Listed> {
    let mut listed = Vec::new();
    // Path, name and IsRelative of the profile section being read.
    let mut section: Option<(Option<String>, Option<String>, bool)> = None;
    let mut finish = |section: Option<(Option<String>, Option<String>, bool)>| {
        if let Some((Some(path), name, relative)) = section {
            listed.push(Listed {
                name,
                path,
                relative,
            });
        }
    };
    for line in ini.lines().map(str::trim) {
        if let Some(header) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            finish(section.take());
            let number = header.strip_prefix("Profile").unwrap_or("x");
            if !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()) {
                section = Some((None, None, true));
            }
        } else if let Some((path, name, relative)) = &mut section
            && let Some((key, value)) = line.split_once('=')
        {
            let value = value.trim();
            match key.trim() {
                "Path" if !value.is_empty() => *path = Some(value.to_owned()),
                "Name" if !value.is_empty() => *name = Some(value.to_owned()),
                "IsRelative" => *relative = value != "0",
                _ => {}
            }
        }
    }
    finish(section);
    listed
}

/// A relative profile path, unless it could leave the root it is joined to.
fn inside(relative: &str) -> Option<PathBuf> {
    let path = Path::new(relative);
    let stays = !relative.contains('\0')
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
        && path
            .components()
            .any(|component| matches!(component, Component::Normal(_)));
    stays.then(|| path.to_path_buf())
}

/// Chromium links `SingletonLock` to `<hostname>-<pid>` while it runs.
pub(super) fn chromium_running(lock: &Path) -> bool {
    held(lock, |target| chromium_owner(target, &hostname()))
}

/// Firefox links `lock` to `<ip>:+<pid>` on Unix; elsewhere there is no link
/// and a locked database fails the read instead.
pub(super) fn firefox_running(lock: &Path) -> bool {
    held(lock, firefox_owner)
}

/// Whether a lock link is held. `owner` yields the local process to test;
/// a link it cannot attribute to one counts as held.
fn held(lock: &Path, owner: impl Fn(&str) -> Option<i32>) -> bool {
    match fs::read_link(lock) {
        Ok(target) => target.to_str().and_then(owner).is_none_or(alive),
        Err(error) => error.kind() != io::ErrorKind::NotFound,
    }
}

fn chromium_owner(target: &str, host: &str) -> Option<i32> {
    // Host names contain dashes; the process id follows the last one.
    let (owner, pid) = target.rsplit_once('-')?;
    (owner == host).then(|| process_id(pid))?
}

fn firefox_owner(target: &str) -> Option<i32> {
    let (_, pid) = target.rsplit_once(':')?;
    process_id(pid.strip_prefix('+').unwrap_or(pid))
}

/// Zero and negative ids address process groups, not one process.
fn process_id(text: &str) -> Option<i32> {
    text.parse().ok().filter(|pid| *pid > 0)
}

#[cfg(unix)]
fn alive(pid: i32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    const ESRCH: i32 = 3;
    // SAFETY: signal 0 delivers nothing; it only reports whether `pid` exists.
    // A process of another user fails with EPERM and is still alive.
    unsafe { kill(pid, 0) == 0 || io::Error::last_os_error().raw_os_error() != Some(ESRCH) }
}

#[cfg(not(unix))]
fn alive(_: i32) -> bool {
    true
}

#[cfg(unix)]
fn hostname() -> String {
    unsafe extern "C" {
        fn gethostname(name: *mut std::ffi::c_char, length: usize) -> i32;
    }
    let mut name = [0_u8; 256];
    // SAFETY: the buffer is live and one byte longer than the length passed,
    // so the name is NUL-terminated even if the system truncated it.
    if unsafe { gethostname(name.as_mut_ptr().cast(), name.len() - 1) } != 0 {
        return String::new();
    }
    let length = name.iter().position(|byte| *byte == 0).unwrap_or(0);
    String::from_utf8_lossy(&name[..length]).into_owned()
}

#[cfg(not(unix))]
fn hostname() -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: PathBuf) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"").unwrap();
    }

    fn listing(profiles: &[Profile]) -> Vec<(&str, &str)> {
        profiles
            .iter()
            .map(|profile| (profile.directory.as_str(), profile.name.as_str()))
            .collect()
    }

    #[test]
    fn chromium_profiles_come_from_local_state() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("chrome");
        touch(root.join("Default/Network/Cookies"));
        // The current location wins over the one older versions used.
        touch(root.join("Default/Cookies"));
        touch(root.join("Profile 1/Cookies"));
        touch(root.join("Profile 2/Cookies"));
        // Recorded without a cookie database, and present without a record.
        fs::create_dir_all(root.join("Profile 3")).unwrap();
        touch(root.join("Unrecorded/Cookies"));
        // A database that only a traversing directory name would reach.
        touch(home.path().join("outside/Cookies"));
        touch(root.join("Default/nested/Cookies"));
        fs::write(
            root.join("Local State"),
            serde_json::json!({ "profile": { "info_cache": {
                "Default": { "name": "  Personal " },
                "Profile 1": { "name": "   " },
                "Profile 2": {},
                "Profile 3": { "name": "Empty" },
                "../outside": { "name": "Escape" },
                "Default/nested": { "name": "Nested" },
                "Default\\nested": { "name": "Nested" },
                "..": { "name": "Parent" },
                ".": { "name": "Self" },
                "": { "name": "Root" },
            } } })
            .to_string(),
        )
        .unwrap();

        let profiles = chromium_profiles(&root);
        assert_eq!(
            listing(&profiles),
            [
                ("Default", "Personal"),
                ("Profile 1", "Profile 1"),
                ("Profile 2", "Profile 2"),
            ]
        );
        assert_eq!(profiles[0].db, root.join("Default/Network/Cookies"));
        assert_eq!(profiles[1].db, root.join("Profile 1/Cookies"));
        assert_eq!(profiles[0].lock, root.join("SingletonLock"));
    }

    #[test]
    fn chromium_profiles_are_scanned_without_a_usable_local_state() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("chrome");
        assert!(chromium_profiles(&root).is_empty());
        touch(root.join("Profile 1/Network/Cookies"));
        touch(root.join("Default/Cookies"));
        fs::create_dir_all(root.join("Crashpad")).unwrap();
        // A cookie database is a file; a directory of that name is not one.
        fs::create_dir_all(root.join("Odd/Cookies")).unwrap();
        touch(root.join("Cookies"));
        let scanned = [("Default", "Default"), ("Profile 1", "Profile 1")];
        assert_eq!(listing(&chromium_profiles(&root)), scanned);

        for state in [
            "not json",
            "{}",
            r#"{"profile":{"info_cache":{"Gone":{}}}}"#,
        ] {
            fs::write(root.join("Local State"), state).unwrap();
            assert_eq!(listing(&chromium_profiles(&root)), scanned);
        }
    }

    #[test]
    fn profiles_ini_lists_profile_sections_only() {
        let ini = "[Install4F96D1932A9F858E]\nDefault=Profiles/a.default\n\n\
                   [Profile1]\nName=Work\nIsRelative=1\nPath=Profiles/b.work\n\n\
                   [Profile0]\n Name = default \r\nPath=/abs/c\nIsRelative=0\n\n\
                   [Profile2]\nName=No path\n\n\
                   [ProfileGroups]\nPath=groups\n\n\
                   [General]\nStartWithLastProfile=1\nPath=general\n\
                   [Profile3]\nPath=d\n";
        let listed = |name: Option<&str>, path: &str, relative| Listed {
            name: name.map(str::to_owned),
            path: path.into(),
            relative,
        };
        assert_eq!(
            profiles_ini(ini),
            [
                listed(Some("Work"), "Profiles/b.work", true),
                listed(Some("default"), "/abs/c", false),
                listed(None, "d", true),
            ]
        );
    }

    #[test]
    fn firefox_profiles_stay_inside_their_root() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("firefox");
        let elsewhere = home.path().join("elsewhere");
        touch(root.join("a.default/cookies.sqlite"));
        touch(root.join("nested/b.work/cookies.sqlite"));
        fs::create_dir_all(root.join("c.empty")).unwrap();
        touch(home.path().join("outside/cookies.sqlite"));
        touch(elsewhere.join("cookies.sqlite"));
        let outside = home.path().join("outside");
        fs::write(
            root.join("profiles.ini"),
            format!(
                "[Profile0]\nName=Main\nIsRelative=1\nPath=a.default\n\
                 [Profile1]\nIsRelative=1\nPath=nested/b.work\n\
                 [Profile2]\nName=Empty\nIsRelative=1\nPath=c.empty\n\
                 [Profile3]\nName=Escape\nIsRelative=1\nPath=../outside\n\
                 [Profile4]\nName=Escape\nIsRelative=1\nPath=a.default/../../outside\n\
                 [Profile5]\nName=Escape\nPath={}\n\
                 [Profile6]\nName=Relative absolute\nIsRelative=0\nPath=a.default\n\
                 [Profile7]\nName=Elsewhere\nIsRelative=0\nPath={}\n",
                outside.display(),
                elsewhere.display(),
            ),
        )
        .unwrap();

        let profiles = firefox_profiles(&root);
        let elsewhere_directory = elsewhere.to_str().unwrap();
        assert_eq!(
            listing(&profiles),
            [
                ("a.default", "Main"),
                ("nested/b.work", "b.work"),
                (elsewhere_directory, "Elsewhere"),
            ]
        );
        assert_eq!(profiles[1].db, root.join("nested/b.work/cookies.sqlite"));
        assert_eq!(profiles[1].lock, root.join("nested/b.work/lock"));
        for escape in ["", ".", "..", "../x", "a/../../x", "/abs", "a/.."] {
            assert_eq!(inside(escape), None, "{escape:?}");
        }
    }

    // Linux keeps profiles beside profiles.ini; the scan differs elsewhere.
    #[cfg(target_os = "linux")]
    #[test]
    fn firefox_profiles_are_scanned_without_a_usable_profiles_ini() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("firefox");
        assert!(firefox_profiles(&root).is_empty());
        touch(root.join("b.work/cookies.sqlite"));
        touch(root.join("a.default/cookies.sqlite"));
        fs::create_dir_all(root.join("Crash Reports")).unwrap();
        let scanned = [("a.default", "a.default"), ("b.work", "b.work")];
        assert_eq!(listing(&firefox_profiles(&root)), scanned);
        fs::write(root.join("profiles.ini"), "[Profile0]\nPath=gone\n").unwrap();
        assert_eq!(listing(&firefox_profiles(&root)), scanned);
    }

    #[test]
    fn lock_targets_name_a_local_process_or_hold() {
        assert_eq!(chromium_owner("host-123", "host"), Some(123));
        assert_eq!(
            chromium_owner("my-dashed-host-123", "my-dashed-host"),
            Some(123)
        );
        for target in [
            "other-123",
            "host",
            "host-",
            "host-abc",
            "host-0",
            "host--5",
            "-123",
            "",
        ] {
            assert_eq!(chromium_owner(target, "host"), None, "{target:?}");
        }
        assert_eq!(firefox_owner("127.0.1.1:+4242"), Some(4242));
        assert_eq!(firefox_owner("192.168.1.7:4242"), Some(4242));
        for target in [
            "127.0.1.1",
            "127.0.1.1:+",
            "127.0.1.1:+x",
            "127.0.1.1:-4",
            "",
        ] {
            assert_eq!(firefox_owner(target), None, "{target:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_lock_is_held_while_its_process_lives() {
        use std::os::unix::fs::symlink;
        let home = tempfile::tempdir().unwrap();
        let lock = home.path().join("SingletonLock");
        let host = hostname();
        assert!(!host.is_empty());
        assert!(!chromium_running(&lock), "no link means not running");

        let relink = |target: String| {
            let _ = fs::remove_file(&lock);
            symlink(target, &lock).unwrap();
        };
        relink(format!("{host}-{}", std::process::id()));
        assert!(chromium_running(&lock));

        // A reaped child's id is not running, barring reuse within this test.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let gone = child.id();
        child.wait().unwrap();
        relink(format!("{host}-{gone}"));
        assert!(!chromium_running(&lock), "a stale lock does not block");
        relink(format!("127.0.1.1:+{gone}"));
        assert!(!firefox_running(&lock));
        relink(format!("127.0.1.1:+{}", std::process::id()));
        assert!(firefox_running(&lock));

        relink(format!("another-host-{gone}"));
        assert!(chromium_running(&lock), "another host cannot be tested");
        relink("unparsable".into());
        assert!(chromium_running(&lock));
        // Not a link at all: the claim cannot be read, so it stands.
        fs::remove_file(&lock).unwrap();
        fs::write(&lock, b"").unwrap();
        assert!(chromium_running(&lock));
    }
}
