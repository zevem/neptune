//! Reads cookies out of the user's other browsers, so that Neptune's browser
//! can start signed in. Read-only: a source profile is never opened for
//! writing, and nothing here names a cookie, its value or its host in an error.

mod chromium;
mod discovery;
mod firefox;
mod keyring;

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use chromium::Keys;
use discovery::Profile;

/// More than any browser profile holds in practice; bounds one import's memory.
const MAX_COOKIES: usize = 20_000;
/// Chromium rejects a cookie whose name and value exceed 4 KiB; twice that
/// keeps every cookie a browser accepted and still bounds a damaged row.
const MAX_NAME_AND_VALUE: usize = 8 * 1024;
const MAX_HOST_OR_PATH: usize = 2 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SameSite {
    Unspecified,
    None,
    Lax,
    Strict,
}

impl SameSite {
    /// Chromium and Firefox number the three policies alike; every other
    /// stored value (-1, NULL, Firefox's 256) means none was given.
    fn stored(value: Option<i64>) -> Self {
        match value {
            Some(0) => Self::None,
            Some(1) => Self::Lax,
            Some(2) => Self::Strict,
            _ => Self::Unspecified,
        }
    }
}

/// One cookie as another browser stored it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cookie {
    /// Host as stored: a leading '.' marks a domain cookie, none a host-only cookie.
    pub host: String,
    pub name: String,
    pub value: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: SameSite,
    /// Unix seconds; None for a session cookie.
    pub expires: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Source {
    pub id: &'static str,
    pub name: &'static str,
    pub profiles: Vec<SourceProfile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceProfile {
    pub directory: String,
    pub name: String,
    pub cookies: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    NotFound,
    BrowserRunning,
    KeyringLocked,
    KeyringMissing,
    KeyringUnavailable,
    Unsupported,
    ReadFailed,
}

impl Failure {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::NotFound => "That browser profile is no longer there. Refresh the list.",
            Self::BrowserRunning => "Quit the other browser, then import again.",
            Self::KeyringLocked => "Unlock your keyring or keychain, then import again.",
            Self::KeyringMissing => {
                "Your keyring has no key for that browser. Open the browser once, then import again."
            }
            Self::KeyringUnavailable => {
                "Your keyring could not be reached, so that browser's cookies cannot be decrypted."
            }
            Self::Unsupported => "Importing from that browser is not supported on this system.",
            Self::ReadFailed => "That browser's cookies could not be read.",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Read {
    pub cookies: Vec<Cookie>,
    pub skipped: usize,
}

impl Read {
    /// Once full, a reader counts the remaining rows without decoding them.
    fn full(&self) -> bool {
        self.cookies.len() >= MAX_COOKIES
    }

    fn push(&mut self, cookie: Cookie) {
        if self.full()
            || cookie.name.len() + cookie.value.len() > MAX_NAME_AND_VALUE
            || cookie.host.len() > MAX_HOST_OR_PATH
            || cookie.path.len() > MAX_HOST_OR_PATH
        {
            self.skipped += 1;
        } else {
            self.cookies.push(cookie);
        }
    }
}

/// A Chromium-family browser: where it keeps profiles and how its key is named.
pub(super) struct Chromium {
    id: &'static str,
    name: &'static str,
    /// Under `~/Library/Application Support`.
    macos: &'static str,
    /// Under `~/.config`; None where the browser has no Linux build.
    linux: Option<&'static str>,
    /// Keychain account; the service is this name followed by " Safe Storage".
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    keychain: &'static str,
    /// The Secret Service item's `application` attribute.
    application: &'static str,
}

impl Chromium {
    /// None omits the browser here. Windows encrypts with AES-GCM under a
    /// DPAPI-wrapped key, which this reader does not implement.
    fn directory(&self) -> Option<&'static str> {
        if cfg!(windows) {
            None
        } else if cfg!(target_os = "macos") {
            Some(self.macos)
        } else {
            self.linux
        }
    }
}

const fn chromium(
    id: &'static str,
    name: &'static str,
    macos: &'static str,
    linux: &'static str,
    keychain: &'static str,
    application: &'static str,
) -> Chromium {
    Chromium {
        id,
        name,
        macos,
        linux: Some(linux),
        keychain,
        application,
    }
}

const CHROMIUM: &[Chromium] = &[
    chromium(
        "chrome",
        "Chrome",
        "Google/Chrome",
        "google-chrome",
        "Chrome",
        "chrome",
    ),
    chromium(
        "edge",
        "Microsoft Edge",
        "Microsoft Edge",
        "microsoft-edge",
        "Microsoft Edge",
        "msedge",
    ),
    chromium(
        "brave",
        "Brave",
        "BraveSoftware/Brave-Browser",
        "BraveSoftware/Brave-Browser",
        "Brave",
        "brave",
    ),
    chromium(
        "vivaldi", "Vivaldi", "Vivaldi", "vivaldi", "Vivaldi", "vivaldi",
    ),
    chromium(
        "opera",
        "Opera",
        "com.operasoftware.Opera",
        "opera",
        "Opera",
        "opera",
    ),
    Chromium {
        id: "arc",
        name: "Arc",
        macos: "Arc/User Data",
        linux: None,
        keychain: "Arc",
        application: "",
    },
    chromium(
        "chromium", "Chromium", "Chromium", "chromium", "Chromium", "chromium",
    ),
];

const FIREFOX: &str = "firefox";

/// Where browsers keep their profiles; a parameter so tests never read the user's.
struct Roots {
    /// Parent of every Chromium-family directory.
    chromium: PathBuf,
    firefox: Vec<PathBuf>,
}

impl Roots {
    fn host() -> Option<Self> {
        let dirs = directories::BaseDirs::new()?;
        let home = dirs.home_dir();
        Some(if cfg!(windows) {
            Self {
                chromium: dirs.data_local_dir().to_path_buf(),
                firefox: vec![dirs.config_dir().join("Mozilla").join("Firefox")],
            }
        } else if cfg!(target_os = "macos") {
            let support = home.join("Library").join("Application Support");
            Self {
                firefox: vec![support.join("Firefox")],
                chromium: support,
            }
        } else {
            Self {
                chromium: home.join(".config"),
                firefox: vec![
                    home.join(".mozilla/firefox"),
                    home.join("snap/firefox/common/.mozilla/firefox"),
                ],
            }
        })
    }
}

struct Found {
    id: &'static str,
    name: &'static str,
    /// None for Firefox.
    browser: Option<&'static Chromium>,
    profiles: Vec<Profile>,
}

fn discover(roots: &Roots) -> Vec<Found> {
    let mut found: Vec<Found> = CHROMIUM
        .iter()
        .filter_map(|browser| {
            let root = roots.chromium.join(browser.directory()?);
            Some(Found {
                id: browser.id,
                name: browser.name,
                browser: Some(browser),
                profiles: discovery::chromium_profiles(&root),
            })
        })
        .collect();
    let mut firefox: Vec<Profile> = Vec::new();
    for profile in roots
        .firefox
        .iter()
        .flat_map(|root| discovery::firefox_profiles(root))
    {
        // `directory` is what a caller hands back, so it names one profile.
        if firefox
            .iter()
            .all(|known| known.directory != profile.directory)
        {
            firefox.push(profile);
        }
    }
    found.push(Found {
        id: FIREFOX,
        name: "Firefox",
        browser: None,
        profiles: firefox,
    });
    found.retain(|source| !source.profiles.is_empty());
    found
}

/// Browsers that have at least one profile with a cookie database. Blocking; call on a worker.
pub(crate) fn sources() -> Vec<Source> {
    Roots::host().map_or_else(Vec::new, |roots| sources_in(&roots))
}

fn sources_in(roots: &Roots) -> Vec<Source> {
    discover(roots)
        .into_iter()
        .map(|found| {
            let sql = if found.browser.is_some() {
                chromium::COUNT
            } else {
                firefox::COUNT
            };
            Source {
                id: found.id,
                name: found.name,
                profiles: found
                    .profiles
                    .into_iter()
                    .map(|profile| SourceProfile {
                        cookies: count(&profile.db, sql),
                        directory: profile.directory,
                        name: profile.name,
                    })
                    .collect(),
            }
        })
        .collect()
}

/// All cookies of one listed profile. `directory` must equal one that `sources()` lists for `id`.
/// Blocking; call on a worker.
pub(crate) fn read(id: &str, directory: &str) -> Result<Read, Failure> {
    let roots = Roots::host().ok_or(Failure::NotFound)?;
    read_in(&roots, id, directory, keyring::keys)
}

fn read_in(
    roots: &Roots,
    id: &str,
    directory: &str,
    keys: impl FnOnce(&Chromium) -> Result<Keys, Failure>,
) -> Result<Read, Failure> {
    if CHROMIUM
        .iter()
        .any(|browser| browser.id == id && browser.directory().is_none())
    {
        return Err(Failure::Unsupported);
    }
    // Listing again is the traversal guard: only a directory that discovery
    // itself produced is opened, never a path assembled from the argument.
    let found = discover(roots)
        .into_iter()
        .find(|found| found.id == id)
        .ok_or(Failure::NotFound)?;
    let profile = found
        .profiles
        .iter()
        .find(|profile| profile.directory == directory)
        .ok_or(Failure::NotFound)?;
    match found.browser {
        Some(browser) => {
            if discovery::chromium_running(&profile.lock) {
                return Err(Failure::BrowserRunning);
            }
            chromium::read(&profile.db, &keys(browser)?)
        }
        None => {
            if discovery::firefox_running(&profile.lock) {
                return Err(Failure::BrowserRunning);
            }
            firefox::read(&profile.db)
        }
    }
}

fn open(db: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// Best effort: a running browser holds its database locked.
fn count(db: &Path, sql: &str) -> Option<u64> {
    let connection = open(db).ok()?;
    // Listing must not wait on each locked profile in turn.
    connection.busy_timeout(std::time::Duration::ZERO).ok()?;
    let count: i64 = connection.query_row(sql, [], |row| row.get(0)).ok()?;
    u64::try_from(count).ok()
}

/// A private copy of a cookie database, removed when dropped.
struct Snapshot {
    // Declared first: the connection closes before its directory is removed.
    db: Connection,
    _directory: tempfile::TempDir,
}

/// Copies through SQLite rather than the filesystem, so that a write-ahead
/// log is included and the source is only ever opened read-only.
fn snapshot(source: &Path) -> Result<Snapshot, Failure> {
    let copy = || -> Option<Snapshot> {
        let directory = tempfile::Builder::new()
            .prefix("neptune-cookie-import-")
            .tempdir()
            .ok()?;
        let path = directory.path().join("cookies.sqlite");
        let live = open(source).ok()?;
        live.busy_timeout(std::time::Duration::from_secs(2)).ok()?;
        live.execute("vacuum into ?1", [path.to_str()?]).ok()?;
        drop(live);
        Some(Snapshot {
            db: open(&path).ok()?,
            _directory: directory,
        })
    };
    copy().ok_or(Failure::ReadFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn firefox_db(profile: &Path, cookies: &[&str]) {
        std::fs::create_dir_all(profile).unwrap();
        let db = Connection::open(profile.join("cookies.sqlite")).unwrap();
        db.execute_batch(firefox::tests::SCHEMA).unwrap();
        for name in cookies {
            db.execute(
                "insert into moz_cookies (originAttributes, name, value, host, path, expiry,
                 isSecure, isHttpOnly, sameSite) values ('', ?1, 'v', 'example.test', '/', 0, 0, 0, 0)",
                [name],
            )
            .unwrap();
        }
    }

    fn roots(home: &Path) -> Roots {
        Roots {
            chromium: home.join("config"),
            firefox: vec![home.join("firefox"), home.join("snap")],
        }
    }

    fn no_keys(_: &Chromium) -> Result<Keys, Failure> {
        Err(Failure::KeyringUnavailable)
    }

    #[test]
    fn sources_list_only_browsers_that_have_a_cookie_database() {
        let home = tempfile::tempdir().unwrap();
        let roots = roots(home.path());
        assert_eq!(sources_in(&roots), []);

        // A browser directory without any cookie database is not a source.
        std::fs::create_dir_all(roots.chromium.join("vivaldi/Default")).unwrap();
        let chrome = roots.chromium.join("google-chrome");
        chromium::tests::database(&chrome.join("Default/Network/Cookies"), 24);
        firefox_db(&roots.firefox[0].join("one.default"), &["a", "b"]);
        firefox_db(&roots.firefox[1].join("two.snap"), &["a"]);
        // The same directory under a second root would be unreachable by name.
        firefox_db(&roots.firefox[1].join("one.default"), &[]);

        let profile = |directory: &str, cookies| SourceProfile {
            directory: directory.into(),
            name: directory.into(),
            cookies: Some(cookies),
        };
        assert_eq!(
            sources_in(&roots),
            [
                Source {
                    id: "chrome",
                    name: "Chrome",
                    profiles: vec![profile("Default", 0)],
                },
                Source {
                    id: "firefox",
                    name: "Firefox",
                    profiles: vec![profile("one.default", 2), profile("two.snap", 1)],
                },
            ]
        );
    }

    #[test]
    fn read_opens_only_a_directory_that_the_listing_names() {
        let home = tempfile::tempdir().unwrap();
        let roots = roots(home.path());
        firefox_db(&roots.firefox[0].join("one.default"), &["a"]);
        // A real profile that the listing does not reach.
        firefox_db(&home.path().join("outside"), &["secret"]);
        chromium::tests::database(&roots.chromium.join("chromium/Default/Cookies"), 24);

        let outside = home.path().join("outside");
        let read = |id, directory| read_in(&roots, id, directory, no_keys).map(|read| read.cookies);
        assert_eq!(read("firefox", "one.default").unwrap().len(), 1);
        for directory in [
            "",
            ".",
            "../outside",
            "one.default/../../outside",
            "one.default/",
            outside.to_str().unwrap(),
        ] {
            assert_eq!(read("firefox", directory), Err(Failure::NotFound));
        }
        assert_eq!(read("chrome", "Default"), Err(Failure::NotFound));
        assert_eq!(read("safari", "Default"), Err(Failure::NotFound));
        assert_eq!(
            read("chromium", "../chromium/Default"),
            Err(Failure::NotFound)
        );
        // Listed: the read proceeds as far as asking for the browser's key.
        assert_eq!(
            read("chromium", "Default"),
            Err(Failure::KeyringUnavailable)
        );
        // Arc has no Linux build, so it is never listed here.
        let arc = read("arc", "Default");
        if cfg!(target_os = "macos") {
            assert_eq!(arc, Err(Failure::NotFound));
        } else {
            assert_eq!(arc, Err(Failure::Unsupported));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_running_browser_is_not_read() {
        let home = tempfile::tempdir().unwrap();
        let roots = roots(home.path());
        let profile = roots.firefox[0].join("one.default");
        firefox_db(&profile, &["a"]);
        let chromium = roots.chromium.join("chromium");
        chromium::tests::database(&chromium.join("Default/Cookies"), 24);
        let me = std::process::id();

        std::os::unix::fs::symlink(format!("127.0.1.1:+{me}"), profile.join("lock")).unwrap();
        // Another host's lock cannot be tested for liveness, so it holds.
        std::os::unix::fs::symlink(format!("elsewhere-{me}"), chromium.join("SingletonLock"))
            .unwrap();
        for (id, directory) in [("firefox", "one.default"), ("chromium", "Default")] {
            assert_eq!(
                read_in(&roots, id, directory, no_keys).map(|read| read.cookies),
                Err(Failure::BrowserRunning)
            );
        }
    }

    #[test]
    fn a_read_bounds_cookie_count_and_size() {
        let cookie = |name: &str, host: &str, path: &str| Cookie {
            host: host.into(),
            name: name.into(),
            value: "v".into(),
            path: path.into(),
            secure: false,
            http_only: false,
            same_site: SameSite::Unspecified,
            expires: None,
        };
        let mut read = Read::default();
        read.push(cookie(&"n".repeat(MAX_NAME_AND_VALUE - 1), "a.test", "/"));
        read.push(cookie(&"n".repeat(MAX_NAME_AND_VALUE), "a.test", "/"));
        read.push(cookie("n", &"h".repeat(MAX_HOST_OR_PATH + 1), "/"));
        read.push(cookie("n", "a.test", &"p".repeat(MAX_HOST_OR_PATH + 1)));
        assert_eq!((read.cookies.len(), read.skipped), (1, 3));
        while !read.full() {
            read.push(cookie("n", "a.test", "/"));
        }
        read.push(cookie("n", "a.test", "/"));
        assert_eq!((read.cookies.len(), read.skipped), (MAX_COOKIES, 4));
    }

    #[test]
    fn a_missing_or_damaged_database_fails_without_detail() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Cookies");
        assert_eq!(snapshot(&path).err(), Some(Failure::ReadFailed));
        std::fs::write(&path, b"not a database").unwrap();
        assert_eq!(snapshot(&path).err(), Some(Failure::ReadFailed));
        assert_eq!(count(&path, chromium::COUNT), None);
    }
}
