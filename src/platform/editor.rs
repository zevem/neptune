//! File locations are data, never shell commands. Resolution and editor launch
//! run on the file handoff worker, outside interactive frames.
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Editor {
    pub name: String,
    pub command: Vec<String>,
}

#[derive(Default)]
pub struct Detection {
    receiver: Option<std::sync::mpsc::Receiver<Vec<Editor>>>,
    editors: Option<Vec<Editor>>,
}

impl Detection {
    pub fn refresh(&mut self) {
        // Reopening Preferences reuses an in-flight scan. Dropping its reply
        // would allow repeated opens to accumulate filesystem workers.
        if self.editors.is_some() {
            *self = Self::default();
        }
    }

    pub fn poll(&mut self, ctx: &eframe::egui::Context) -> Option<&[Editor]> {
        if self.editors.is_none() {
            if let Some(receiver) = &self.receiver {
                match receiver.try_recv() {
                    Ok(editors) => self.editors = Some(editors),
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.editors = Some(Vec::new())
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }
            } else {
                let (sender, receiver) = std::sync::mpsc::sync_channel(1);
                let wake = ctx.clone();
                if std::thread::Builder::new()
                    .name("neptune-editors".into())
                    .spawn(move || {
                        let _ = sender.send(detect());
                        wake.request_repaint();
                    })
                    .is_ok()
                {
                    self.receiver = Some(receiver);
                } else {
                    self.editors = Some(Vec::new());
                }
            }
        }
        self.editors.as_deref()
    }
}

fn executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn detected(paths: Vec<PathBuf>) -> Vec<Editor> {
    let mut editors = Vec::new();
    for path in paths {
        if !executable(&path) {
            continue;
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        let (label, args): (&str, &[&str]) = match name
            .trim_end_matches(".exe")
            .trim_end_matches(".cmd")
            .trim_end_matches(".sh")
        {
            "code" => ("Visual Studio Code", &["--goto", "{file}:{line}:{column}"]),
            "code-insiders" => (
                "Visual Studio Code Insiders",
                &["--goto", "{file}:{line}:{column}"],
            ),
            "cursor" => ("Cursor", &["--goto", "{file}:{line}:{column}"]),
            "studio" | "studio64" | "android-studio" => (
                "Android Studio",
                &["--line", "{line}", "--column", "{column}", "{file}"],
            ),
            "idea" | "idea64" => (
                "IntelliJ IDEA",
                &["--line", "{line}", "--column", "{column}", "{file}"],
            ),
            "subl" | "sublime_text" => ("Sublime Text", &["{file}:{line}:{column}"]),
            "zed" => ("Zed", &["{file}:{line}:{column}"]),
            _ => continue,
        };
        if editors.iter().any(|e: &Editor| e.name == label) {
            continue;
        }
        let mut command = vec![path.to_string_lossy().into_owned()];
        command.extend(args.iter().map(|s| (*s).into()));
        editors.push(Editor {
            name: label.into(),
            command,
        });
    }
    editors
}

fn detect() -> Vec<Editor> {
    let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
    let mut folders: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).take(256).collect())
        .unwrap_or_default();
    #[cfg(unix)]
    {
        folders.extend(
            [
                "/usr/local/bin",
                "/opt/homebrew/bin",
                "/snap/bin",
                "/opt/android-studio/bin",
                "/usr/local/android-studio/bin",
            ]
            .map(PathBuf::from),
        );
        if let Some(home) = &home {
            folders.extend([
                home.join(".local/bin"),
                home.join(".local/share/JetBrains/Toolbox/scripts"),
                home.join("android-studio/bin"),
            ]);
        }
    }
    let names = [
        "code",
        "code-insiders",
        "cursor",
        "studio",
        "studio.sh",
        "studio64",
        "android-studio",
        "idea",
        "idea64",
        "subl",
        "sublime_text",
        "zed",
    ];
    let paths: Vec<PathBuf> = folders
        .iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .collect();
    #[cfg(any(target_os = "macos", windows))]
    let mut paths = paths;
    #[cfg(target_os = "macos")]
    {
        for base in [
            Some(PathBuf::from("/Applications")),
            home.as_ref().map(|h| h.join("Applications")),
        ]
        .into_iter()
        .flatten()
        {
            for app in [
                "Visual Studio Code.app/Contents/Resources/app/bin/code",
                "Cursor.app/Contents/Resources/app/bin/cursor",
                "Android Studio.app/Contents/MacOS/studio",
                "IntelliJ IDEA.app/Contents/MacOS/idea",
            ] {
                paths.push(base.join(app));
            }
        }
    }
    #[cfg(windows)]
    {
        let mut bases: Vec<PathBuf> = ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .collect();
        if let Some(home) = home {
            bases.push(home.join("AppData/Local"));
        }
        for base in bases {
            for app in [
                "Programs/Microsoft VS Code/Code.exe",
                "Microsoft VS Code/Code.exe",
                "Programs/cursor/Cursor.exe",
                "Cursor/Cursor.exe",
                "Android/Android Studio/bin/studio64.exe",
            ] {
                paths.push(base.join(app));
            }
        }
        paths.extend(folders.iter().flat_map(|dir| {
            names
                .iter()
                .map(move |name| dir.join(format!("{name}.exe")))
        }));
    }
    detected(paths)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLocation {
    pub path: String,
    pub line: u32,
    pub column: u32,
}

impl FileLocation {
    /// Printed paths optionally end in :line[:column]. Read numbers from the
    /// right so drive letters and colons inside a path remain intact.
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() > 4096 || text.chars().any(char::is_control) {
            return None;
        }
        if text
            .get(..7)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file://"))
        {
            return Self::file_uri(text);
        }
        let Some((before, last)) = text
            .rsplit_once(':')
            .filter(|(_, last)| !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()))
        else {
            return path_like(text).then(|| Self::path(text)).flatten();
        };
        let number = |s: &str| {
            (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                .then(|| s.parse::<u32>().ok())
                .flatten()
                .filter(|n| *n > 0)
        };
        let last = number(last)?;
        let (path, line, column) = match before.rsplit_once(':') {
            Some((path, line)) if !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit()) => {
                (path, number(line)?, last)
            }
            _ => (before, last, 1),
        };
        if !(path_like(path)
            || !path.is_empty()
                && path
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
                && path.chars().any(char::is_alphabetic))
            || path.contains("://")
        {
            return None;
        }
        Some(Self {
            path: path.into(),
            line,
            column,
        })
    }

    /// An explicitly clicked spelling may name an extensionless file with any
    /// name. The worker decides whether it exists; ordinary prose is not made
    /// into hover links or keyboard hints just for looking like a word.
    pub fn path(text: &str) -> Option<Self> {
        if text.is_empty()
            || text.trim().is_empty()
            || text.len() > 4096
            || text.chars().any(char::is_control)
            || text.contains("://")
        {
            return None;
        }
        Some(Self {
            path: text.into(),
            line: 1,
            column: 1,
        })
    }

    fn file_uri(text: &str) -> Option<Self> {
        if text.chars().any(char::is_whitespace) || text.contains(['?', '#']) {
            return None;
        }
        // Like file drops and picture previews, file hyperlinks refer to this
        // terminal's machine. SSH terminals are rejected by the coordinator.
        let rest = &text[7..];
        let path = &rest[rest.find('/')?..];
        let bytes = path.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len());
        let mut at = 0;
        while at < bytes.len() {
            if bytes[at] == b'%' {
                let high = char::from(*bytes.get(at + 1)?).to_digit(16)?;
                let low = char::from(*bytes.get(at + 2)?).to_digit(16)?;
                decoded.push((high * 16 + low) as u8);
                at += 3;
            } else {
                decoded.push(bytes[at]);
                at += 1;
            }
        }
        let path = String::from_utf8(decoded).ok()?;
        let path = match path.as_bytes() {
            [b'/', drive, b':', ..] if cfg!(windows) && drive.is_ascii_alphabetic() => &path[1..],
            _ => &path,
        };
        Self::path(path)
    }
}

/// Conservative text recognition, without touching the filesystem. Bare file
/// names with extensions, dotfiles and directory paths are useful hints too.
pub fn path_like(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 4096
        && !text.chars().any(char::is_control)
        && !text.contains("://")
        && (text.contains('/')
            || text.contains('\\')
            || text.starts_with('.') && text.len() > 1
            || text.rsplit_once('.').is_some_and(|(base, ext)| {
                !base.is_empty() && !ext.chars().any(char::is_whitespace)
            })
            || text.chars().any(|c| c.is_ascii_uppercase())
                && text.chars().all(|c| {
                    c.is_ascii_uppercase() || c.is_ascii_digit() || matches!(c, '_' | '-')
                })
            || text
                .get(text.len().saturating_sub(4)..)
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case("file")))
        && !matches!(text, "/" | "\\" | "./" | "../")
}

pub fn validate_command(args: &[String]) -> anyhow::Result<()> {
    if args.is_empty() {
        return Ok(());
    }
    anyhow::ensure!(
        args.len() <= 64
            && args
                .iter()
                .all(|s| s.len() <= 4096 && !s.chars().any(char::is_control)),
        "editor command is too long or contains control characters"
    );
    anyhow::ensure!(
        !args[0].trim().is_empty() && !args[0].contains('{'),
        "editor must name an executable"
    );
    anyhow::ensure!(
        args[1..].iter().any(|s| s.contains("{file}")),
        "editor arguments must include {{file}}"
    );
    Ok(())
}

pub(super) fn command(
    location: &FileLocation,
    directories: &[PathBuf],
    args: &[String],
) -> Result<Command, &'static str> {
    let path = resolve(location, directories)?;
    let executable = args
        .first()
        .ok_or("Choose an external editor in Preferences.")?;
    validate_command(args)
        .map_err(|_| "The editor command is invalid. Check editor in config.toml.")?;
    let mut command = Command::new(executable);
    for argument in &args[1..] {
        // Substitute each template part once; braces in a printed filename
        // are literal and must not become another placeholder.
        let mut remaining = argument.as_str();
        let mut value = std::ffi::OsString::new();
        while let Some(offset) = remaining.find('{') {
            value.push(&remaining[..offset]);
            remaining = &remaining[offset..];
            if let Some(rest) = remaining.strip_prefix("{file}") {
                value.push(&path);
                remaining = rest;
            } else if let Some(rest) = remaining.strip_prefix("{line}") {
                value.push(location.line.to_string());
                remaining = rest;
            } else if let Some(rest) = remaining.strip_prefix("{column}") {
                value.push(location.column.to_string());
                remaining = rest;
            } else {
                value.push("{");
                remaining = &remaining[1..];
            }
        }
        value.push(remaining);
        command.arg(value);
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_keep_drive_letters_unicode_spaces_and_optional_columns() {
        for (text, path, line, column) in [
            ("AGENTS.md", "AGENTS.md", 1, 1),
            (".gitignore", ".gitignore", 1, 1),
            ("README", "README", 1, 1),
            ("Makefile", "Makefile", 1, 1),
            ("src/tool", "src/tool", 1, 1),
            ("src/foo.rs:42", "src/foo.rs", 42, 1),
            ("C:\\src\\foo.rs:42:9", "C:\\src\\foo.rs", 42, 9),
            ("/tmp/a b/é.rs:2:3", "/tmp/a b/é.rs", 2, 3),
        ] {
            assert_eq!(
                FileLocation::parse(text),
                Some(FileLocation {
                    path: path.into(),
                    line,
                    column
                })
            );
        }
        for text in [
            "https://site.test:42",
            "12:30",
            "foo.rs:0",
            "foo.rs:4294967296",
            "foo.rs:1\n",
        ] {
            assert!(FileLocation::parse(text).is_none(), "{text}");
        }
    }

    #[test]
    fn file_uris_decode_the_actual_path_and_reject_malformed_targets() {
        for (uri, path) in [
            ("file:///tmp/AGENTS.md", "/tmp/AGENTS.md"),
            ("FILE://localhost/tmp/a%20b%23c.%C3%A9", "/tmp/a b#c.é"),
            ("file://devbox/tmp/README", "/tmp/README"),
        ] {
            assert_eq!(FileLocation::parse(uri), FileLocation::path(path), "{uri}");
        }
        for uri in [
            "file://",
            "file://host",
            "file:///tmp/%",
            "file:///tmp/%xx",
            "file:///tmp/%FF",
            "file:///tmp/%00",
            "file:///tmp/%0A",
            "file:///tmp/a b",
            "file:///tmp/file?query",
        ] {
            assert!(FileLocation::parse(uri).is_none(), "{uri}");
        }
    }

    #[test]
    fn resolution_accepts_regular_files_with_any_name_and_preserves_directory_priority() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        for name in [
            "AGENTS.md",
            ".env",
            "README",
            "Makefile",
            "plainword",
            "é 世界",
            "foo.pkg+unknown",
            "foo.any-extension",
        ] {
            std::fs::write(root.path().join(name), "first directory").unwrap();
            std::fs::write(other.path().join(name), "second directory").unwrap();
            let location = FileLocation::path(name).unwrap();
            assert_eq!(
                resolve(&location, &[root.path().into(), other.path().into()]).unwrap(),
                root.path().join(name).canonicalize().unwrap(),
                "{name}"
            );
        }
        std::fs::create_dir(root.path().join("folder.md")).unwrap();
        for name in ["folder.md", "missing.md"] {
            assert!(resolve(&FileLocation::path(name).unwrap(), &[root.path().into()]).is_err());
        }
    }

    #[test]
    fn editor_resolves_the_first_existing_directory_and_preserves_literal_arguments() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("a {line} $(echo) é.rs");
        std::fs::write(&file, "").unwrap();
        let location = FileLocation {
            path: file.file_name().unwrap().to_str().unwrap().into(),
            line: 42,
            column: 7,
        };
        let built = command(
            &location,
            &[root.path().join("missing"), root.path().into()],
            &[
                "code".into(),
                "--goto".into(),
                "{file}:{line}:{column}".into(),
            ],
        )
        .unwrap();
        assert_eq!(built.get_program(), "code");
        let args: Vec<_> = built.get_args().collect();
        assert_eq!(args[0], "--goto");
        assert_eq!(
            args[1].to_str().unwrap(),
            format!("{}:42:7", file.canonicalize().unwrap().display())
        );
        let cursor = command(
            &location,
            &[root.path().into()],
            &[
                "cursor".into(),
                "--goto".into(),
                "{file}:{line}:{column}".into(),
            ],
        )
        .unwrap();
        assert_eq!(cursor.get_program(), "cursor");
        assert!(
            command(
                &FileLocation::parse("missing.rs:1").unwrap(),
                &[root.path().into()],
                &[]
            )
            .is_err()
        );
        assert!(command(&location, &[root.path().into()], &[]).is_err());
    }
}

pub fn resolve(location: &FileLocation, directories: &[PathBuf]) -> Result<PathBuf, &'static str> {
    let path = if let Some(rest) = location.path.strip_prefix("~/") {
        directories::BaseDirs::new().map(|d| d.home_dir().join(rest))
    } else if Path::new(&location.path).is_absolute() {
        Some(PathBuf::from(&location.path))
    } else {
        directories
            .iter()
            .map(|base| base.join(&location.path))
            .find(|path| path.is_file())
    }
    .filter(|path| path.is_file())
    .ok_or("The file was not found in this terminal's directories.")?;
    // An absolute path cannot be mistaken for an editor option. Canonicalizing
    // also gives Windows launchers their native drive spelling.
    path.canonicalize()
        .map_err(|_| "Could not resolve the file.")
}

#[cfg(test)]
mod detection_tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn installed_editor_detection_recognizes_launchers_and_builds_line_arguments() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        for name in ["code", "cursor", "studio.sh", "unknown", "not-executable"] {
            let path = root.path().join(name);
            std::fs::write(&path, "").unwrap();
            std::fs::set_permissions(
                &path,
                std::fs::Permissions::from_mode(if name == "not-executable" {
                    0o644
                } else {
                    0o755
                }),
            )
            .unwrap();
            paths.push(path);
        }
        paths.push(root.path().join("code"));
        let found = detected(paths);
        assert_eq!(
            found.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["Visual Studio Code", "Cursor", "Android Studio"]
        );
        assert_eq!(
            &found[2].command[1..],
            ["--line", "{line}", "--column", "{column}", "{file}"]
        );
        for editor in found {
            validate_command(&editor.command).unwrap();
        }
    }
}
