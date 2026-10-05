use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub keybindings: crate::keybindings::Keybindings,
    pub theme: Theme,
    pub custom_themes: Vec<crate::terminal_theme::CustomTheme>,
    /// Themes starred in the catalog, in the order they were starred.
    pub favorite_themes: Vec<Theme>,
    pub accent: Accent,
    /// Scale of terminal content and window chrome, independent of font size.
    pub window_zoom: f32,
    /// Installed monospace family; unavailable fonts use bundled JetBrains Mono.
    pub font_family: String,
    pub font_size: f32,
    pub line_height: f32,
    pub scrollback: usize,
    pub shell: Option<String>,
    /// Editor executable and arguments, with {file}, {line} and {column}.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub editor: Vec<String>,
    /// Arguments for `shell`, such as `-d Ubuntu` for `wsl.exe`. Omitted when
    /// empty, so settings without arguments still load in older versions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shell_args: Vec<String>,
    pub cursor: Cursor,
    pub cursor_blink: bool,
    pub sidebar_width: f32,
    pub restore_workspaces: bool,
    pub confirm_close: bool,
    /// Independent protection for sessions with active or unknown jobs.
    pub warn_running_processes: bool,
    /// Allow terminal programs to send native OS notifications.
    pub desktop_notifications: bool,
    /// Update checks use only public release metadata, never terminal contents.
    pub check_updates: bool,
    pub release_channel: crate::runtime::updates::ReleaseChannel,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Theme {
    #[default]
    Graphite,
    Dusk,
    Light,
    /// A bundled or custom palette styles both the window and terminal.
    Palette(String),
}

impl Theme {
    pub const BUILTINS: [(Self, &'static str); 3] = [
        (Self::Graphite, "Graphite"),
        (Self::Dusk, "Dusk"),
        (Self::Light, "Light"),
    ];
    pub fn id(&self) -> &str {
        match self {
            Self::Graphite => "graphite",
            Self::Dusk => "dusk",
            Self::Light => "light",
            Self::Palette(id) => id,
        }
    }
}
impl TryFrom<String> for Theme {
    type Error = &'static str;
    fn try_from(id: String) -> Result<Self, Self::Error> {
        match id.as_str() {
            "graphite" => Ok(Self::Graphite),
            "dusk" => Ok(Self::Dusk),
            "light" => Ok(Self::Light),
            _ if id.starts_with("iterm:") || id.starts_with("custom:") => Ok(Self::Palette(id)),
            _ => Err("Unknown theme; use graphite, dusk, light, iterm:<name> or custom:<id>"),
        }
    }
}
impl From<Theme> for String {
    fn from(theme: Theme) -> Self {
        theme.id().to_owned()
    }
}

/// The highlight used for focus, selection and the terminal cursor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Accent {
    #[default]
    Blue,
    Indigo,
    Purple,
    Pink,
    Red,
    Orange,
    Yellow,
    Green,
    Graphite,
}

impl Accent {
    pub const ALL: [(Self, &'static str); 9] = [
        (Self::Blue, "Blue"),
        (Self::Indigo, "Indigo"),
        (Self::Purple, "Purple"),
        (Self::Pink, "Pink"),
        (Self::Red, "Red"),
        (Self::Orange, "Orange"),
        (Self::Yellow, "Yellow"),
        (Self::Green, "Green"),
        (Self::Graphite, "Graphite"),
    ];
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cursor {
    #[default]
    Block,
    Beam,
    Underline,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            keybindings: Default::default(),
            theme: Theme::Graphite,
            custom_themes: Vec::new(),
            favorite_themes: Vec::new(),
            accent: Accent::Blue,
            window_zoom: 1.0,
            font_family: "JetBrains Mono".into(),
            font_size: 14.0,
            line_height: 1.4,
            scrollback: 10_000,
            shell: None,
            editor: Vec::new(),
            shell_args: Vec::new(),
            cursor: Cursor::Block,
            cursor_blink: false,
            sidebar_width: 216.0,
            restore_workspaces: true,
            confirm_close: true,
            warn_running_processes: true,
            desktop_notifications: true,
            check_updates: true,
            release_channel: Default::default(),
        }
    }
}

impl Config {
    // Match the bounds of the existing app zoom shortcuts.
    pub const WINDOW_ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.2..=5.0;

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let mut value: toml::Table = toml::from_str(
            &std::fs::read_to_string(path)
                .with_context(|| format!("Cannot read {}", path.display()))?,
        )
        .with_context(|| format!("Invalid configuration in {}", path.display()))?;
        // Upgrade the short-lived split-theme format without discarding custom
        // palettes. The next normal save writes only the unified theme choice.
        if let Some(legacy) = value.remove("terminal_theme") {
            value.insert("theme".into(), legacy);
        }
        let mut config: Self = value
            .try_into()
            .with_context(|| format!("Invalid configuration in {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&mut self) -> Result<()> {
        use crate::terminal_theme::MAX_CUSTOM_THEMES;
        self.keybindings.validate()?;
        anyhow::ensure!(
            self.custom_themes.len() <= MAX_CUSTOM_THEMES,
            "At most 128 custom themes are allowed"
        );
        let mut ids = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        for theme in &self.custom_themes {
            theme.validate()?;
            anyhow::ensure!(ids.insert(&theme.id), "Duplicate custom theme ID");
            anyhow::ensure!(
                names.insert(theme.name.trim().to_lowercase()),
                "Custom theme names must be unique"
            );
        }
        anyhow::ensure!(
            self.has_theme(&self.theme),
            "Unknown theme: {}",
            self.theme.id()
        );
        // Favorites follow the themes that exist: one whose theme is gone is
        // dropped rather than refused, and none is listed twice.
        let mut seen = std::collections::HashSet::new();
        let mut favorites = std::mem::take(&mut self.favorite_themes);
        favorites.retain(|theme| self.has_theme(theme) && seen.insert(theme.clone()));
        self.favorite_themes = favorites;
        anyhow::ensure!(
            self.window_zoom.is_finite() && Self::WINDOW_ZOOM_RANGE.contains(&self.window_zoom),
            "window_zoom must be between 0.2 and 5"
        );
        anyhow::ensure!(
            valid_font_family(&self.font_family),
            "font_family must be a nonempty name of at most 128 characters without control characters"
        );
        self.font_family = self.font_family.trim().to_owned();
        anyhow::ensure!(
            self.font_size.is_finite() && (9.0..=32.0).contains(&self.font_size),
            "font_size must be between 9 and 32"
        );
        anyhow::ensure!(
            self.line_height.is_finite() && (1.0..=2.0).contains(&self.line_height),
            "line_height must be between 1 and 2"
        );
        anyhow::ensure!(
            self.scrollback <= 1_000_000,
            "scrollback cannot exceed 1,000,000 lines"
        );
        anyhow::ensure!(
            self.sidebar_width.is_finite() && (170.0..=360.0).contains(&self.sidebar_width),
            "sidebar_width must be between 170 and 360"
        );
        crate::platform::editor::validate_command(&self.editor)?;
        if let Some(shell) = &self.shell {
            anyhow::ensure!(!shell.trim().is_empty(), "shell cannot be empty");
        }
        anyhow::ensure!(
            self.shell.is_some() || self.shell_args.is_empty(),
            "shell_args require a shell"
        );
        Ok(())
    }
    /// Whether `theme` is built in, bundled or a saved custom theme.
    fn has_theme(&self, theme: &Theme) -> bool {
        let Theme::Palette(id) = theme else {
            return true;
        };
        id.strip_prefix("iterm:")
            .and_then(crate::terminal_theme::bundled)
            .is_some()
            || self.custom_themes.iter().any(|custom| &custom.id == id)
    }

    pub fn theme_colors(&self) -> Option<crate::terminal_theme::ThemeColors> {
        let Theme::Palette(id) = &self.theme else {
            return None;
        };
        if let Some(name) = id.strip_prefix("iterm:") {
            crate::terminal_theme::bundled(name).map(|theme| theme.colors)
        } else {
            self.custom_themes
                .iter()
                .find(|theme| &theme.id == id)
                .map(|theme| theme.colors)
        }
    }

    pub fn theme_name(&self) -> &str {
        match &self.theme {
            Theme::Graphite => "Graphite",
            Theme::Dusk => "Dusk",
            Theme::Light => "Light",
            Theme::Palette(id) => id.strip_prefix("iterm:").unwrap_or_else(|| {
                self.custom_themes
                    .iter()
                    .find(|theme| &theme.id == id)
                    .map_or("Unknown theme", |theme| theme.name.as_str())
            }),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        atomic_write(path, self.source_for_editing()?.as_bytes())
    }

    fn source_for_editing(&self) -> Result<String> {
        let mut settings = toml::Table::try_from(self)?;
        settings.remove("keybindings");
        let mut source = toml::to_string_pretty(&settings)?;
        source.push('\n');
        source.push_str(&crate::keybindings::Keybindings::reference());
        source.push_str(&toml::to_string_pretty(&self.keybindings)?);
        Ok(source)
    }

    /// Prepare a first config for editing without replacing any existing file,
    /// including an invalid config or a concurrent preferences save.
    /// Run only on a storage worker.
    pub(crate) fn create_if_missing(&self, path: &Path) -> Result<()> {
        use std::io::{ErrorKind, Write};
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => {
                anyhow::ensure!(!metadata.is_dir(), "The config path is a directory");
                return Ok(());
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let mut temporary = tempfile::Builder::new()
            .prefix(".neptune-config-")
            .tempfile_in(parent)?;
        temporary.write_all(self.source_for_editing()?.as_bytes())?;
        temporary.as_file().sync_all()?;
        match temporary.into_temp_path().persist_noclobber(path) {
            Ok(()) => {
                #[cfg(unix)]
                std::fs::File::open(parent)?.sync_all()?;
                Ok(())
            }
            Err(error) if error.error.kind() == ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error.error.into()),
        }
    }
}

pub(crate) fn valid_font_family(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= 128 && !name.chars().any(char::is_control)
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    atomic_write_with(path, |file| file.write_all(bytes))
}

/// Prepare a complete file beside its destination and commit it with one rename.
/// Unique temporary files allow concurrent saves; the last successful commit wins.
/// Preparation/replacement failures preserve the previous destination and remove
/// the temporary file. A directory-sync failure can occur after the commit.
/// Windows access/sharing conflicts retry for up to 250 ms on the storage caller.
fn atomic_write_with(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Cannot create configuration directory {}", parent.display()))?;
    // Open before committing so a failure here also preserves the old file.
    #[cfg(unix)]
    let directory = std::fs::File::open(parent)
        .with_context(|| format!("Cannot open configuration directory {}", parent.display()))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".neptune-save-")
        .tempfile_in(parent)
        .with_context(|| format!("Cannot prepare save for {}", path.display()))?;
    write(temporary.as_file_mut())
        .with_context(|| format!("Cannot write configuration {}", path.display()))?;
    temporary
        .as_file()
        .sync_all()
        .with_context(|| format!("Cannot synchronize configuration {}", path.display()))?;
    // tempfile uses rename on Unix and MoveFileExW(REPLACE_EXISTING) on Windows.
    // Never remove the old destination before this atomic replacement.
    // Close our prepared file before replacement so another writer cannot
    // encounter our still-open, delete-pending destination on Windows.
    persist_temporary(temporary.into_temp_path(), path)
        .with_context(|| format!("Cannot replace configuration {}", path.display()))?;
    #[cfg(unix)]
    directory.sync_all().with_context(|| {
        format!(
            "Saved {}, but cannot synchronize its directory",
            path.display()
        )
    })?;
    Ok(())
}

fn persist_temporary(temporary: tempfile::TempPath, path: &Path) -> std::io::Result<()> {
    #[cfg(not(windows))]
    return temporary.persist(path).map_err(|error| error.error);

    #[cfg(windows)]
    {
        use std::time::{Duration, Instant};
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut temporary = temporary;
        loop {
            match temporary.persist(path) {
                Ok(()) => return Ok(()),
                Err(error)
                    if matches!(error.error.raw_os_error(), Some(5 | 32 | 33))
                        && Instant::now() < deadline =>
                {
                    // MoveFileEx can temporarily reject replacement while a
                    // reader/another rename holds the destination. Keep the
                    // complete temporary file and never unlink the old data.
                    temporary = error.path;
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error.error),
            }
        }
    }
}

pub fn data_dir() -> PathBuf {
    directories::ProjectDirs::from("rs", "Neptune", "neptune")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".neptune"))
}

/// Resolve default storage on a startup worker, preserving all legacy files.
/// Explicit data roots and ephemeral screenshot launches bypass migration.
pub fn prepare_data_dir() -> Result<PathBuf> {
    let destination = data_dir();
    let legacy = directories::ProjectDirs::from("dev", "Pace", "pace")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".pace"));
    migrate_data_dir(&legacy, &destination)?;
    Ok(destination)
}

fn migrate_data_dir(legacy: &Path, destination: &Path) -> Result<()> {
    if destination.try_exists()? || !legacy.try_exists()? {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create storage parent {}", parent.display()))?;
    }
    // Move the complete directory atomically, including recovery copies and
    // unreadable or unsupported state. Never parse or replace saved files here.
    // A failed move stops startup before a fresh directory can hide old work.
    std::fs::rename(legacy, destination).with_context(|| {
        format!(
            "Cannot migrate saved data from {} to {}",
            legacy.display(),
            destination.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn config_for_editing_creates_a_complete_snapshot_when_missing() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("new folder/custom settings.toml");
        let mut config: super::Config =
            toml::from_str("font_size = 18\n[keybindings]\nsplit-right = [\"Alt+H\"]\n").unwrap();
        config.validate().unwrap();
        config.create_if_missing(&path).unwrap();
        assert_eq!(super::Config::load(&path).unwrap(), config);
        let source = std::fs::read_to_string(&path).unwrap();
        assert!(source.contains("restart Neptune"));
        assert!(source.contains("# workspace-9 = ["));
        assert!(source.contains("# clear-scrollback = []"));
        assert!(source.contains("# Chords: modifier+modifier+key"));
        assert!(source.contains("split-right = [\"Alt+H\"]"));
        // Later Preferences saves keep the reference available in the file.
        config.font_size = 20.0;
        config.save(&path).unwrap();
        assert_eq!(super::Config::load(&path).unwrap(), config);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("# workspace-9 = [")
        );
    }

    #[test]
    fn config_for_editing_preserves_existing_invalid_files_and_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        let original = "[keybindings]\nfind = [\"unknown-key\"]\n";
        std::fs::write(&path, original).unwrap();
        super::Config::default().create_if_missing(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        #[cfg(unix)]
        {
            let link = root.path().join("linked.toml");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            super::Config::default().create_if_missing(&link).unwrap();
            assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        }
    }

    #[test]
    fn config_for_editing_concurrent_creation_never_replaces_a_complete_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        let start = std::sync::Barrier::new(4);
        std::thread::scope(|scope| {
            for size in 15..19 {
                let path = &path;
                let start = &start;
                scope.spawn(move || {
                    let config = super::Config {
                        font_size: size as f32,
                        ..super::Config::default()
                    };
                    start.wait();
                    config.create_if_missing(path).unwrap();
                });
            }
        });
        let config = super::Config::load(&path).unwrap();
        assert!((15.0..=18.0).contains(&config.font_size));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    use super::*;

    #[test]
    fn font_family_defaults_for_old_settings_and_roundtrips_unavailable_choices() {
        let legacy: Config = toml::from_str("font_size = 15.0").unwrap();
        assert_eq!(legacy.font_family, "JetBrains Mono");
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        let config = Config {
            font_family: "A font from another computer".into(),
            ..Config::default()
        };
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap().font_family, config.font_family);
    }

    #[test]
    fn font_family_names_are_bounded_and_trimmed() {
        for family in ["".into(), " ".into(), "bad\nfont".into(), "x".repeat(129)] {
            let mut config = Config {
                font_family: family,
                ..Config::default()
            };
            assert!(config.validate().is_err());
        }
        let mut config = Config {
            font_family: "  MesloLGS NF  ".into(),
            ..Config::default()
        };
        config.validate().unwrap();
        assert_eq!(config.font_family, "MesloLGS NF");
    }

    #[test]
    fn terminal_themes_round_trip_and_legacy_appearance_stays_compatible() {
        let mut legacy: Config = toml::from_str("theme = \"dusk\"").unwrap();
        legacy.validate().unwrap();
        assert_eq!(legacy.theme, Theme::Dusk);
        assert_eq!(legacy.theme_name(), "Dusk");
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        legacy
            .custom_themes
            .push(crate::terminal_theme::CustomTheme {
                id: "custom:1".into(),
                name: "My colors".into(),
                colors: crate::terminal_theme::bundled("Dracula").unwrap().colors,
            });
        legacy.theme = Theme::Palette("custom:1".into());
        legacy.favorite_themes = vec![
            Theme::Palette("iterm:Dracula".into()),
            Theme::Light,
            Theme::Palette("custom:1".into()),
        ];
        legacy.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), legacy);
    }

    #[test]
    fn favorites_follow_the_themes_that_exist() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        // A favorite outlives neither its theme nor a second mention, and
        // never keeps the rest of the settings from loading.
        std::fs::write(
            &path,
            r#"favorite_themes = ["dusk", "iterm:missing", "custom:7", "iterm:Dracula", "dusk"]
font_size = 15.0
"#,
        )
        .unwrap();
        let mut config = Config::load(&path).unwrap();
        assert_eq!(config.font_size, 15.0);
        assert_eq!(
            config.favorite_themes,
            [Theme::Dusk, Theme::Palette("iterm:Dracula".into())]
        );
        // Custom IDs are reused, so a deleted theme takes its star with it.
        let custom = crate::terminal_theme::CustomTheme {
            id: "custom:7".into(),
            name: "My colors".into(),
            colors: crate::terminal_theme::bundled("Dracula").unwrap().colors,
        };
        config
            .favorite_themes
            .push(Theme::Palette(custom.id.clone()));
        config.custom_themes.push(custom);
        config.validate().unwrap();
        assert_eq!(config.favorite_themes.len(), 3);
        config.custom_themes.clear();
        config.validate().unwrap();
        assert_eq!(config.favorite_themes.len(), 2);
        // Only a theme ID can be a favorite.
        assert!(toml::from_str::<Config>("favorite_themes = [\"dracula\"]").is_err());
    }

    #[test]
    fn split_theme_settings_migrate_to_one_theme_without_writing_on_load() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        let original = "theme = \"dusk\"\nterminal_theme = \"iterm:Dracula\"\n";
        std::fs::write(&path, original).unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.theme, Theme::Palette("iterm:Dracula".into()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        config.save(&path).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("terminal_theme"));
        assert_eq!(Config::load(&path).unwrap(), config);
        let damaged = original.replace("Dracula", "missing");
        std::fs::write(&path, &damaged).unwrap();
        assert!(Config::load(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), damaged);
    }

    #[test]
    fn terminal_theme_validation_rejects_unknown_duplicate_and_oversized_data() {
        let theme = crate::terminal_theme::CustomTheme {
            id: "custom:1".into(),
            name: "My colors".into(),
            colors: crate::terminal_theme::bundled("Dracula").unwrap().colors,
        };
        let mut config = Config {
            theme: Theme::Palette("iterm:missing".into()),
            ..Config::default()
        };
        assert!(config.validate().is_err());
        config.theme = Theme::Palette(theme.id.clone());
        assert!(config.validate().is_err());
        config.custom_themes.push(theme.clone());
        assert!(config.validate().is_ok());
        config.custom_themes.push(theme.clone());
        assert!(config.validate().is_err());
        config.custom_themes[1].id = "custom:2".into();
        config.custom_themes[1].name = "MY COLORS".into();
        assert!(config.validate().is_err());
        config.custom_themes = vec![theme; 129];
        assert!(config.validate().is_err());
    }

    #[test]
    fn product_rename_preserves_all_saved_bytes_and_recovery_copies() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("legacy");
        let destination = root.path().join("Neptune/neptune/config");
        std::fs::create_dir_all(legacy.join("recovery")).unwrap();
        let files: [(&str, &[u8]); 4] = [
            ("config.toml", b"theme = \"dusk\"\n"),
            ("workspaces.json", b"{\"version\":999}"),
            ("window.json", b"damaged window data\xff"),
            ("recovery/original.json", b"original bytes"),
        ];
        for (name, bytes) in files {
            std::fs::write(legacy.join(name), bytes).unwrap();
        }
        migrate_data_dir(&legacy, &destination).unwrap();
        assert!(!legacy.exists());
        for (name, bytes) in files {
            assert_eq!(std::fs::read(destination.join(name)).unwrap(), bytes);
        }
        migrate_data_dir(&legacy, &destination).unwrap();
        assert_eq!(
            std::fs::read(destination.join("workspaces.json")).unwrap(),
            files[1].1
        );
    }

    #[test]
    fn product_rename_keeps_existing_neptune_storage_and_legacy_storage() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("legacy");
        let destination = root.path().join("neptune");
        std::fs::create_dir(&legacy).unwrap();
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(legacy.join("workspaces.json"), b"legacy work").unwrap();
        std::fs::write(destination.join("workspaces.json"), b"current work").unwrap();
        migrate_data_dir(&legacy, &destination).unwrap();
        assert_eq!(
            std::fs::read(legacy.join("workspaces.json")).unwrap(),
            b"legacy work"
        );
        assert_eq!(
            std::fs::read(destination.join("workspaces.json")).unwrap(),
            b"current work"
        );
    }

    #[test]
    fn product_rename_does_not_create_storage_without_legacy_data() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("neptune");
        migrate_data_dir(&root.path().join("missing"), &destination).unwrap();
        assert!(!destination.exists());
    }

    #[test]
    fn product_rename_failure_preserves_legacy_data() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("legacy");
        std::fs::create_dir(&legacy).unwrap();
        std::fs::write(legacy.join("workspaces.json"), b"original work").unwrap();
        let blocked = root.path().join("blocked");
        std::fs::write(&blocked, b"a file, not a directory").unwrap();
        assert!(migrate_data_dir(&legacy, &blocked.join("neptune")).is_err());
        assert_eq!(
            std::fs::read(legacy.join("workspaces.json")).unwrap(),
            b"original work"
        );
    }

    // Atomic replacement guarantees complete contents, not that a racing
    // Windows open always succeeds. Test this Windows error policy on every
    // host, but apply it only to the Windows stress-test reader below. That
    // reader has observed ERROR_FILE_NOT_FOUND (2) during concurrent replaces;
    // retry it within the same budget, then require the final file to exist.
    fn read_with_windows_retries(
        mut read: impl FnMut() -> std::io::Result<Vec<u8>>,
    ) -> std::io::Result<Vec<u8>> {
        use std::time::{Duration, Instant};

        let deadline = Instant::now() + Duration::from_millis(250);
        loop {
            match read() {
                Err(error)
                    if matches!(error.raw_os_error(), Some(2 | 5 | 32 | 33))
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                result => return result,
            }
        }
    }

    #[test]
    fn concurrent_reader_recovers_from_transient_windows_replacement_conflicts() {
        for code in [2, 5, 32, 33] {
            let mut attempts = 0;
            let bytes = read_with_windows_retries(|| {
                attempts += 1;
                if attempts == 1 {
                    Err(std::io::Error::from_raw_os_error(code))
                } else {
                    Ok(b"complete file".to_vec())
                }
            })
            .unwrap();
            assert_eq!(bytes, b"complete file");
            assert_eq!(attempts, 2);
        }
    }

    #[test]
    fn concurrent_reader_surfaces_unexpected_errors_without_retrying() {
        for error in [
            std::io::Error::from_raw_os_error(3),
            std::io::Error::from_raw_os_error(87),
            std::io::Error::new(std::io::ErrorKind::NotFound, "no Windows error code"),
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "no Windows error code",
            ),
            std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid file"),
        ] {
            let mut error = Some(error);
            let raw_code = error.as_ref().unwrap().raw_os_error();
            let kind = error.as_ref().unwrap().kind();
            let returned = read_with_windows_retries(|| {
                Err(error.take().expect("Unexpected error was retried"))
            })
            .unwrap_err();
            assert_eq!(returned.raw_os_error(), raw_code);
            assert_eq!(returned.kind(), kind);
        }
    }

    #[test]
    fn concurrent_reader_surfaces_persistent_replacement_errors_after_timeout() {
        for code in [2, 5, 32, 33] {
            let started = std::time::Instant::now();
            let mut attempts = 0;
            let error = read_with_windows_retries(|| {
                attempts += 1;
                Err(std::io::Error::from_raw_os_error(code))
            })
            .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(code));
            assert!(attempts > 1);
            // Each retry sleeps at least 5 ms within the 250 ms budget.
            assert!(attempts <= 51);
            assert!(started.elapsed() >= std::time::Duration::from_millis(250));
        }
    }

    #[cfg(windows)]
    #[test]
    fn concurrent_reader_recovers_after_a_windows_handle_releases_read_access() {
        use std::os::windows::fs::OpenOptionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, b"complete file").unwrap();
        let mut blocker = Some(
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&path)
                .unwrap(),
        );
        let bytes = read_with_windows_retries(|| {
            let result = std::fs::read(&path);
            if let Err(error) = &result {
                assert_eq!(error.raw_os_error(), Some(32));
                // Release only after a real sharing conflict, without making
                // recovery depend on another thread meeting the retry deadline.
                drop(blocker.take().expect("Read stayed blocked after release"));
            }
            result
        })
        .unwrap();
        assert!(blocker.is_none());
        assert_eq!(bytes, b"complete file");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn save_retries_a_temporary_windows_sharing_violation() {
        use std::io::Write;
        use std::os::windows::fs::OpenOptionsExt;
        use std::time::Duration;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, b"previous complete file").unwrap();
        // FILE_SHARE_READ | FILE_SHARE_WRITE, deliberately without DELETE.
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1 | 2)
            .open(&path)
            .unwrap();
        let (prepared, ready) = std::sync::mpsc::sync_channel(1);
        std::thread::scope(|scope| {
            let writer = scope.spawn(|| {
                atomic_write_with(&path, |file| {
                    file.write_all(b"replacement complete file")?;
                    prepared.send(()).unwrap();
                    Ok(())
                })
            });
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
            std::thread::sleep(Duration::from_millis(50));
            assert_eq!(std::fs::read(&path).unwrap(), b"previous complete file");
            drop(reader);
            writer.join().unwrap().unwrap();
        });
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement complete file");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn sharing_timeout_preserves_previous_file_and_cleans_tempfile() {
        use std::os::windows::fs::OpenOptionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, b"previous complete file").unwrap();
        // Keep the destination open without FILE_SHARE_DELETE past the retry limit.
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1 | 2)
            .open(&path)
            .unwrap();

        let error = atomic_write(&path, b"replacement complete file").unwrap_err();
        assert!(matches!(
            error
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(5 | 32 | 33)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"previous complete file");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);

        drop(reader);
        atomic_write(&path, b"replacement complete file").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement complete file");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn rejects_invalid_resources() {
        let mut c = Config {
            font_size: f32::NAN,
            ..Config::default()
        };
        assert!(c.validate().is_err());
        c = Config {
            scrollback: usize::MAX,
            ..Config::default()
        };
        assert!(c.validate().is_err());
        assert!(toml::from_str::<Config>("font_szie = 14").is_err());
    }
    #[test]
    fn close_preferences_migrate_and_round_trip_independently() {
        let old: Config = toml::from_str("confirm_close = false").unwrap();
        assert!(!old.confirm_close);
        assert!(old.warn_running_processes);
        for confirm in [false, true] {
            for warn in [false, true] {
                let config = Config {
                    confirm_close: confirm,
                    warn_running_processes: warn,
                    ..Config::default()
                };
                let restored: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
                assert_eq!(
                    (restored.confirm_close, restored.warn_running_processes),
                    (confirm, warn)
                );
            }
        }
    }

    #[test]
    fn shell_arguments_round_trip_and_are_omitted_when_empty() {
        let plain = toml::to_string(&Config::default()).unwrap();
        assert!(!plain.contains("shell_args"));
        let config = Config {
            shell: Some("wsl.exe".into()),
            shell_args: vec!["-d".into(), "Ubuntu 24.04".into()],
            ..Config::default()
        };
        let mut restored: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored, config);
        let mut orphaned: Config = toml::from_str("shell_args = [\"-l\"]").unwrap();
        assert!(orphaned.validate().is_err());
    }

    #[test]
    fn config_round_trip() {
        let c = Config::default();
        let parsed: Config = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
        assert_eq!(parsed.font_size, 14.0);
        assert_eq!(parsed.theme, Theme::Graphite);
        assert_eq!(parsed.accent, Accent::Blue);
    }
    #[test]
    fn configuration_saved_before_accents_still_loads() {
        let parsed: Config = toml::from_str("theme = \"dusk\"\nfont_size = 15.0\n").unwrap();
        assert_eq!(parsed.theme, Theme::Dusk);
        assert_eq!(parsed.accent, Accent::Blue);
        assert_eq!(parsed.window_zoom, 1.0);
        let accent: Config = toml::from_str("accent = \"indigo\"").unwrap();
        assert_eq!(accent.accent, Accent::Indigo);
    }
    #[test]
    fn saved_config_round_trip_replaces_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/config.toml");
        Config::default().save(&path).unwrap();
        let changed = Config {
            theme: Theme::Dusk,
            window_zoom: 1.3,
            font_size: 19.0,
            ..Config::default()
        };
        changed.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.theme, Theme::Dusk);
        assert_eq!(loaded.window_zoom, 1.3);
        assert_eq!(loaded.font_size, 19.0);
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }
    #[test]
    fn window_zoom_rejects_non_finite_and_out_of_range_values() {
        for zoom in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, 0.19, 5.01] {
            let mut config = Config {
                window_zoom: zoom,
                ..Config::default()
            };
            assert!(config.validate().is_err(), "Accepted zoom {zoom}");
        }
        for zoom in [0.2, 1.0, 1.35, 5.0] {
            let mut config = Config {
                window_zoom: zoom,
                ..Config::default()
            };
            assert!(config.validate().is_ok(), "Rejected zoom {zoom}");
        }
    }
    #[test]
    fn failed_preparation_preserves_previous_file() {
        use std::io::Write;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        atomic_write(&path, b"previous complete configuration").unwrap();
        let result = atomic_write_with(&path, |file| {
            file.write_all(b"incomplete replacement")?;
            Err(std::io::Error::other("injected write failure"))
        });
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"previous complete configuration"
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
    #[test]
    fn failed_replacement_preserves_destination_and_cleans_tempfile() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("existing-directory");
        std::fs::create_dir(&destination).unwrap();
        let previous = destination.join("config.toml");
        std::fs::write(&previous, b"previous file").unwrap();
        assert!(atomic_write(&destination, b"replacement").is_err());
        assert_eq!(std::fs::read(&previous).unwrap(), b"previous file");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
    #[test]
    fn concurrent_saves_never_expose_partial_files_or_collide() {
        use std::sync::{
            Arc, Barrier,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        const LENGTH: usize = 32 * 1024;
        atomic_write(&path, &vec![b'A'; LENGTH]).unwrap();
        let complete = AtomicBool::new(false);
        let committed = AtomicUsize::new(0);
        let start = Arc::new(Barrier::new(9));
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                start.wait();
                let mut observed = 0;
                loop {
                    #[cfg(windows)]
                    let bytes = read_with_windows_retries(|| std::fs::read(&path)).unwrap();
                    #[cfg(not(windows))]
                    let bytes = std::fs::read(&path).unwrap();
                    assert_eq!(bytes.len(), LENGTH);
                    assert!((b'A'..=b'H').contains(&bytes[0]));
                    assert!(bytes.iter().all(|byte| *byte == bytes[0]));
                    observed += 1;
                    if complete.load(Ordering::Acquire) {
                        return observed;
                    }
                }
            });
            let writers: Vec<_> = (0..8)
                .map(|index| {
                    let path = &path;
                    let start = start.clone();
                    let committed = &committed;
                    scope.spawn(move || {
                        start.wait();
                        for _ in 0..8 {
                            match atomic_write(path, &vec![b'A' + index; LENGTH]) {
                                Ok(()) => {
                                    committed.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(error) => {
                                    // Continuous readers/writers can exhaust the bounded
                                    // Windows sharing retries. Every other failure is a bug.
                                    #[cfg(windows)]
                                    let sharing_conflict = error
                                        .to_string()
                                        .starts_with("Cannot replace configuration ")
                                        && error.downcast_ref::<std::io::Error>().is_some_and(
                                            |error| {
                                                matches!(error.raw_os_error(), Some(5 | 32 | 33))
                                            },
                                        );
                                    #[cfg(not(windows))]
                                    let sharing_conflict = false;
                                    assert!(sharing_conflict, "Concurrent save failed: {error:#}");
                                }
                            }
                        }
                    })
                })
                .collect();
            let results: Vec<_> = writers.into_iter().map(|writer| writer.join()).collect();
            complete.store(true, Ordering::Release);
            assert!(reader.join().unwrap() > 0);
            for result in results {
                result.unwrap();
            }
        });
        assert!(committed.load(Ordering::Relaxed) > 0);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), LENGTH);
        assert!(bytes.iter().all(|byte| *byte == bytes[0]));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}

#[cfg(test)]
mod editor_tests {
    use super::*;
    #[test]
    fn editor_settings_default_to_explorer_and_validate_and_round_trip_custom_commands() {
        let legacy: Config = toml::from_str("font_size = 14.0").unwrap();
        assert!(legacy.editor.is_empty());
        let mut config = Config {
            editor: vec![
                "cursor".into(),
                "--goto".into(),
                "{file}:{line}:{column}".into(),
            ],
            ..Config::default()
        };
        config.validate().unwrap();
        assert_eq!(
            toml::from_str::<Config>(&toml::to_string(&config).unwrap()).unwrap(),
            config
        );
        for args in [
            vec!["".into(), "{file}".into()],
            vec!["code".into(), "--goto".into()],
            vec!["code".into(), "{file}\n".into()],
        ] {
            config.editor = args;
            assert!(config.validate().is_err());
        }
    }
}
