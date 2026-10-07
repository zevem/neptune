//! Validated application shortcuts. Terminal protocol encoding remains in terminal-core.
use anyhow::{Result, ensure};
use eframe::egui::{Key, Modifiers};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BindingAction {
    NewWorkspace,
    NewWorktree,
    NewTab,
    SplitRight,
    SplitBelow,
    ClosePane,
    Find,
    CommandPalette,
    ToggleSidebar,
    ToggleRightPanel,
    ZoomPane,
    Copy,
    CopyHints,
    Paste,
    NextTab,
    PreviousTab,
    Preferences,
    NextWorkspace,
    PreviousWorkspace,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    ZoomIn,
    ZoomOut,
    ResetZoom,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    ClearScrollback,
    RestartPane,
    BackgroundPane,
    CloseWorkspace,
    Notifications,
    ShowFiles,
    ShowAgents,
    ShowProject,
    BrowseThemes,
    #[serde(rename = "workspace-1")]
    Workspace1,
    #[serde(rename = "workspace-2")]
    Workspace2,
    #[serde(rename = "workspace-3")]
    Workspace3,
    #[serde(rename = "workspace-4")]
    Workspace4,
    #[serde(rename = "workspace-5")]
    Workspace5,
    #[serde(rename = "workspace-6")]
    Workspace6,
    #[serde(rename = "workspace-7")]
    Workspace7,
    #[serde(rename = "workspace-8")]
    Workspace8,
    #[serde(rename = "workspace-9")]
    Workspace9,
}

const CTRL: Modifiers = Modifiers {
    ctrl: true,
    ..Modifiers::NONE
};
const CMD: Modifiers = Modifiers {
    mac_cmd: true,
    ..Modifiers::NONE
};
const CTRL_SHIFT: Modifiers = Modifiers {
    shift: true,
    ..CTRL
};
const CMD_SHIFT: Modifiers = Modifiers { shift: true, ..CMD };
const HOST: Modifiers = if cfg!(target_os = "macos") {
    CMD
} else {
    CTRL_SHIFT
};
const fn chord(key: Key, modifiers: Modifiers) -> KeyChord {
    KeyChord { key, modifiers }
}
const fn host(key: Key) -> KeyChord {
    chord(key, HOST)
}

impl BindingAction {
    pub const ALL: &'static [Self] = &[
        Self::NewWorkspace,
        Self::NewWorktree,
        Self::NewTab,
        Self::SplitRight,
        Self::SplitBelow,
        Self::ClosePane,
        Self::Find,
        Self::CommandPalette,
        Self::ToggleSidebar,
        Self::ToggleRightPanel,
        Self::ZoomPane,
        Self::Copy,
        Self::CopyHints,
        Self::Paste,
        Self::NextTab,
        Self::PreviousTab,
        Self::Preferences,
        Self::NextWorkspace,
        Self::PreviousWorkspace,
        Self::FocusLeft,
        Self::FocusRight,
        Self::FocusUp,
        Self::FocusDown,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::ResetZoom,
        Self::IncreaseFontSize,
        Self::DecreaseFontSize,
        Self::ResetFontSize,
        Self::ClearScrollback,
        Self::RestartPane,
        Self::BackgroundPane,
        Self::CloseWorkspace,
        Self::Notifications,
        Self::ShowFiles,
        Self::ShowAgents,
        Self::ShowProject,
        Self::BrowseThemes,
        Self::Workspace1,
        Self::Workspace2,
        Self::Workspace3,
        Self::Workspace4,
        Self::Workspace5,
        Self::Workspace6,
        Self::Workspace7,
        Self::Workspace8,
        Self::Workspace9,
    ];
    fn defaults(self) -> &'static [KeyChord] {
        match self {
            Self::NewWorkspace => const { &[host(Key::N)] },
            Self::NewWorktree => const { &[host(Key::G)] },
            Self::NewTab => const { &[host(Key::T)] },
            Self::SplitRight => const { &[host(Key::D)] },
            Self::SplitBelow => const { &[host(Key::E)] },
            Self::ClosePane => const { &[host(Key::W)] },
            Self::Find => const { &[host(Key::F)] },
            Self::CommandPalette => const { &[host(Key::P)] },
            Self::ToggleSidebar => const { &[host(Key::B)] },
            Self::ToggleRightPanel => const { &[host(Key::O)] },
            Self::ZoomPane => const { &[host(Key::Enter)] },
            Self::Copy => const { &[host(Key::C), chord(Key::Copy, Modifiers::NONE)] },
            Self::CopyHints => {
                const {
                    &[chord(
                        Key::H,
                        if cfg!(target_os = "macos") {
                            CMD_SHIFT
                        } else {
                            CTRL_SHIFT
                        },
                    )]
                }
            }
            Self::Paste => {
                #[cfg(windows)]
                {
                    const {
                        &[
                            host(Key::V),
                            chord(Key::Paste, Modifiers::NONE),
                            chord(Key::Insert, Modifiers::SHIFT),
                        ]
                    }
                }
                #[cfg(not(windows))]
                {
                    const { &[host(Key::V), chord(Key::Paste, Modifiers::NONE)] }
                }
            }
            Self::NextTab => const { &[host(Key::PageDown)] },
            Self::PreviousTab => const { &[host(Key::PageUp)] },
            Self::Preferences => const { &[chord(Key::Comma, CTRL), chord(Key::Comma, CMD)] },
            Self::NextWorkspace => const { &[chord(Key::Tab, CTRL)] },
            Self::PreviousWorkspace => const { &[chord(Key::Tab, CTRL_SHIFT)] },
            Self::FocusLeft => const { &[chord(Key::ArrowLeft, CTRL_SHIFT)] },
            Self::FocusRight => const { &[chord(Key::ArrowRight, CTRL_SHIFT)] },
            Self::FocusUp => const { &[chord(Key::ArrowUp, CTRL_SHIFT)] },
            Self::FocusDown => const { &[chord(Key::ArrowDown, CTRL_SHIFT)] },
            Self::ZoomIn => {
                const {
                    &[
                        chord(Key::Plus, CTRL),
                        chord(Key::Equals, CTRL),
                        chord(Key::Plus, CMD),
                        chord(Key::Equals, CMD),
                    ]
                }
            }
            Self::ZoomOut => const { &[chord(Key::Minus, CTRL), chord(Key::Minus, CMD)] },
            Self::ResetZoom => const { &[chord(Key::Num0, CTRL), chord(Key::Num0, CMD)] },
            Self::IncreaseFontSize => {
                const {
                    &[
                        chord(Key::Plus, CTRL_SHIFT),
                        chord(Key::Equals, CTRL_SHIFT),
                        chord(Key::Plus, CMD_SHIFT),
                        chord(Key::Equals, CMD_SHIFT),
                    ]
                }
            }
            Self::DecreaseFontSize => {
                const { &[chord(Key::Minus, CTRL_SHIFT), chord(Key::Minus, CMD_SHIFT)] }
            }
            Self::ResetFontSize => {
                const { &[chord(Key::Num0, CTRL_SHIFT), chord(Key::Num0, CMD_SHIFT)] }
            }
            Self::ClearScrollback => const { &[] },
            Self::RestartPane => const { &[] },
            Self::BackgroundPane => const { &[] },
            Self::CloseWorkspace => const { &[] },
            Self::Notifications => const { &[] },
            Self::ShowFiles => const { &[] },
            Self::ShowAgents => const { &[] },
            Self::ShowProject => const { &[] },
            Self::BrowseThemes => const { &[] },
            Self::Workspace1 => const { &[host(Key::Num1)] },
            Self::Workspace2 => const { &[host(Key::Num2)] },
            Self::Workspace3 => const { &[host(Key::Num3)] },
            Self::Workspace4 => const { &[host(Key::Num4)] },
            Self::Workspace5 => const { &[host(Key::Num5)] },
            Self::Workspace6 => const { &[host(Key::Num6)] },
            Self::Workspace7 => const { &[host(Key::Num7)] },
            Self::Workspace8 => const { &[host(Key::Num8)] },
            Self::Workspace9 => const { &[host(Key::Num9)] },
        }
    }
    pub fn workspace(index: usize) -> Option<Self> {
        use BindingAction::*;
        [
            Workspace1, Workspace2, Workspace3, Workspace4, Workspace5, Workspace6, Workspace7,
            Workspace8, Workspace9,
        ]
        .get(index)
        .copied()
    }
    pub fn workspace_index(self) -> Option<usize> {
        (0..9).find(|index| Self::workspace(*index) == Some(self))
    }
    pub fn global(self) -> bool {
        self.is_font()
            || matches!(
                self,
                Self::ZoomIn
                    | Self::ZoomOut
                    | Self::ResetZoom
                    | Self::Preferences
                    | Self::CommandPalette
                    | Self::ToggleSidebar
                    | Self::ToggleRightPanel
            )
    }
    pub fn is_font(self) -> bool {
        matches!(
            self,
            Self::IncreaseFontSize | Self::DecreaseFontSize | Self::ResetFontSize
        )
    }
}

/// A logical key and exact native modifiers. `command` is derived by egui and
/// deliberately excluded: `ctrl` and `super` must remain distinct on macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct KeyChord {
    key: Key,
    modifiers: Modifiers,
}
impl KeyChord {
    fn bindable(key: Key) -> bool {
        !matches!(
            key,
            Key::Escape
                | Key::ShiftLeft
                | Key::ShiftRight
                | Key::ControlLeft
                | Key::ControlRight
                | Key::AltLeft
                | Key::AltRight
                | Key::SuperLeft
                | Key::SuperRight
        )
    }
    fn matches(self, key: Key, modifiers: Modifiers) -> bool {
        self.key == key
            && self.modifiers.alt == modifiers.alt
            && self.modifiers.ctrl == modifiers.ctrl
            && self.modifiers.shift == modifiers.shift
            && self.modifiers.mac_cmd == modifiers.mac_cmd
    }
    pub fn hint(self) -> String {
        let mut parts = Vec::new();
        if self.modifiers.ctrl {
            parts.push("Ctrl");
        }
        if self.modifiers.mac_cmd {
            parts.push(if cfg!(target_os = "macos") {
                "⌘"
            } else {
                "Super"
            });
        }
        if self.modifiers.alt {
            parts.push("Alt");
        }
        if self.modifiers.shift {
            parts.push("Shift");
        }
        parts.push(match self.key {
            Key::ArrowLeft => "←",
            Key::ArrowRight => "→",
            Key::ArrowUp => "↑",
            Key::ArrowDown => "↓",
            Key::Plus => "+",
            Key::Minus => "-",
            Key::Equals => "=",
            Key::Comma => ",",
            Key::PageUp => "PgUp",
            Key::PageDown => "PgDn",
            _ => self.key.name(),
        });
        if cfg!(target_os = "macos") && parts.first() == Some(&"⌘") {
            format!("⌘{}", parts[1..].join("+"))
        } else {
            parts.join("+")
        }
    }
}
impl TryFrom<String> for KeyChord {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() > 96 {
            return Err("Keybinding chords must be at most 96 bytes".into());
        }
        let mut parts: Vec<_> = value.split('+').map(str::trim).collect();
        let name = parts.pop().ok_or("A keybinding needs a key")?;
        let key = Key::ALL
            .iter()
            .copied()
            .find(|key| key.name().eq_ignore_ascii_case(name))
            .or_else(|| match name.to_ascii_lowercase().as_str() {
                "left" | "arrowleft" => Some(Key::ArrowLeft),
                "right" | "arrowright" => Some(Key::ArrowRight),
                "up" | "arrowup" => Some(Key::ArrowUp),
                "down" | "arrowdown" => Some(Key::ArrowDown),
                "esc" => Some(Key::Escape),
                "return" => Some(Key::Enter),
                "pgup" => Some(Key::PageUp),
                "pgdn" => Some(Key::PageDown),
                _ => Key::from_name(name),
            })
            .ok_or_else(|| {
                format!("Unknown key '{name}' in keybinding '{value}'; use Plus for the + key")
            })?;
        if !Self::bindable(key) {
            return Err(format!(
                "Key {name} cannot be bound; Escape is reserved for cancelling UI"
            ));
        }
        let mut modifiers = Modifiers::NONE;
        for part in parts {
            let flag = match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut modifiers.ctrl,
                "shift" => &mut modifiers.shift,
                "alt" | "option" => &mut modifiers.alt,
                "super" | "cmd" | "command" | "meta" => &mut modifiers.mac_cmd,
                "primary" if cfg!(target_os = "macos") => &mut modifiers.mac_cmd,
                "primary" => &mut modifiers.ctrl,
                _ => return Err(format!("Unknown modifier '{part}' in keybinding '{value}'")),
            };
            if *flag {
                return Err(format!(
                    "Repeated modifier '{part}' in keybinding '{value}'"
                ));
            }
            *flag = true;
        }
        if matches!(key, Key::Copy | Key::Cut | Key::Paste) && modifiers != Modifiers::NONE {
            return Err("Dedicated Copy/Cut/Paste keys can only be bound without modifiers".into());
        }
        Ok(Self { key, modifiers })
    }
}
impl From<KeyChord> for String {
    fn from(chord: KeyChord) -> Self {
        let mut parts = Vec::new();
        if chord.modifiers.ctrl {
            parts.push("Ctrl");
        }
        if chord.modifiers.mac_cmd {
            parts.push("Super");
        }
        if chord.modifiers.alt {
            parts.push("Alt");
        }
        if chord.modifiers.shift {
            parts.push("Shift");
        }
        parts.push(chord.key.name());
        parts.join("+")
    }
}

/// Only overrides are saved. An empty list disables the action's shortcuts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Keybindings(BTreeMap<BindingAction, Vec<KeyChord>>);
impl Keybindings {
    /// A copyable catalog of action names, platform defaults and accepted keys.
    /// Keep it commented so it can accompany a config without adding overrides.
    pub(crate) fn reference() -> String {
        let defaults = Self(
            BindingAction::ALL
                .iter()
                .map(|&action| {
                    let chords = action
                        .defaults()
                        .iter()
                        .copied()
                        // Other platforms cannot configure macOS-only modifiers.
                        .filter(|chord| cfg!(target_os = "macos") || !chord.modifiers.mac_cmd)
                        .collect();
                    (action, chords)
                })
                .collect(),
        );
        let mut reference = String::from(
            "# Shortcut reference: action names and platform defaults.\n\
             # Uncomment only the actions you want to change, then restart Neptune.\n\
             # Omitted actions keep defaults; a list replaces them; [] disables them.\n\
             # Example: split-right = [\"Alt+H\", \"Primary+Shift+F10\"]\n",
        );
        reference.push_str("\n[keybindings]\n");
        let entries =
            toml::to_string(&defaults).expect("Keybindings serialize as TOML string lists");
        for line in entries.lines() {
            reference.push_str("# ");
            reference.push_str(line);
            reference.push('\n');
        }
        reference.push_str(
            "\n# Key syntax\n\
             # Edit an existing action entry instead of uncommenting a second entry.\n\
             # Use up to 8 shortcuts per action. A shortcut cannot belong to two actions.\n\
             # Chords: modifier+modifier+key, with exact modifiers and case-insensitive names.\n\
             # Modifiers: Ctrl (Control), Shift, Alt (Option), Primary.\n\
             # Primary means Command on macOS and Ctrl on Linux/Windows.\n",
        );
        if cfg!(target_os = "macos") {
            reference.push_str("# macOS also accepts Super, Cmd, Command and Meta for Command.\n");
        }
        if cfg!(windows) {
            reference.push_str(
                "# Ctrl+Insert cannot be rebound on Windows; use a different shortcut.\n",
            );
        }
        reference.push_str(
            "# Keys: A-Z, 0-9, F1-F35, and the names below. Use Plus for the + key.\n\
             # Copy, Cut and Paste are dedicated keyboard keys; use them without modifiers.\n\
             # Escape is reserved for cancelling UI. Other keys:\n",
        );
        let keys: Vec<_> = Key::ALL
            .iter()
            .copied()
            .filter(|&key| KeyChord::bindable(key))
            .map(|key| key.name())
            .filter(|name| {
                !(name.len() == 1 && name.as_bytes()[0].is_ascii_alphanumeric()
                    || name
                        .strip_prefix('F')
                        .is_some_and(|number| number.parse::<u8>().is_ok()))
            })
            .collect();
        for names in keys.chunks(5) {
            reference.push_str("# ");
            reference.push_str(&names.join(", "));
            reference.push('\n');
        }
        reference
    }
    pub fn overridden(&self, action: BindingAction) -> bool {
        self.0.contains_key(&action)
    }
    pub fn bindings(&self, action: BindingAction) -> &[KeyChord] {
        self.0
            .get(&action)
            .map_or_else(|| action.defaults(), Vec::as_slice)
    }
    pub fn hint(&self, action: BindingAction) -> String {
        let bindings = self.bindings(action);
        let binding = if self.0.contains_key(&action) {
            bindings.first()
        } else {
            bindings
                .iter()
                .find(|binding| binding.modifiers.mac_cmd == cfg!(target_os = "macos"))
                .or(bindings.first())
        };
        binding.map_or_else(String::new, |binding| binding.hint())
    }
    pub fn hint_or_unbound(&self, action: BindingAction) -> String {
        let hint = self.hint(action);
        if hint.is_empty() {
            "Unbound".into()
        } else {
            hint
        }
    }
    pub fn resolve(
        &self,
        key: Key,
        physical: Option<Key>,
        modifiers: Modifiers,
        font_key: Key,
    ) -> Option<BindingAction> {
        self.0
            .iter()
            .find_map(|(action, bindings)| {
                bindings
                    .iter()
                    .any(|binding| binding.matches(key, modifiers))
                    .then_some(*action)
            })
            .or_else(|| {
                BindingAction::ALL.iter().copied().find(|action| {
                    if self.overridden(*action) {
                        return false;
                    }
                    let candidate = if action.is_font() {
                        font_key
                    } else if action.workspace_index().is_some() {
                        physical.unwrap_or(key)
                    } else {
                        key
                    };
                    action
                        .defaults()
                        .iter()
                        .any(|binding| binding.matches(candidate, modifiers))
                })
            })
    }

    pub fn validate(&self) -> Result<()> {
        let mut seen = HashSet::new();
        for action in BindingAction::ALL {
            ensure!(
                self.bindings(*action).len() <= 8,
                "At most 8 shortcuts per action are allowed"
            );
            for binding in self.bindings(*action) {
                ensure!(
                    !self.overridden(*action)
                        || !binding.modifiers.mac_cmd
                        || cfg!(target_os = "macos"),
                    "Super/Cmd keybindings are supported on macOS; use Primary or Ctrl on this platform"
                );
                ensure!(
                    !self.overridden(*action)
                        || !cfg!(windows)
                        || binding.key != Key::Insert
                        || !binding.modifiers.ctrl,
                    "Ctrl+Insert cannot be rebound on Windows because the window toolkit reports it as Ctrl+C"
                );
                let identity = (
                    binding.key,
                    binding.modifiers.ctrl,
                    binding.modifiers.mac_cmd,
                    binding.modifiers.alt,
                    binding.modifiers.shift,
                );
                ensure!(
                    seen.insert(identity),
                    "Conflicting keybinding {} for {:?}; remove it from the other action (use [] to disable its defaults)",
                    binding.hint(),
                    action
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn reference_lists_every_action_with_valid_platform_defaults() {
        let reference = Keybindings::reference();
        let (_, entries) = reference.split_once("[keybindings]\n").unwrap();
        let (entries, _) = entries.split_once("\n# Key syntax").unwrap();
        let entries = entries
            .lines()
            .map(|line| line.strip_prefix("# ").unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let catalog: Keybindings = toml::from_str(&entries).unwrap();
        catalog.validate().unwrap();
        assert_eq!(catalog.0.len(), BindingAction::ALL.len());
        for &action in BindingAction::ALL {
            let expected: Vec<_> = action
                .defaults()
                .iter()
                .copied()
                .filter(|chord| cfg!(target_os = "macos") || !chord.modifiers.mac_cmd)
                .collect();
            assert_eq!(catalog.bindings(action), expected, "{action:?}");
        }
        assert!(reference.contains("# clear-scrollback = []"));
        assert!(reference.contains("# workspace-9 = ["));
        assert!(reference.contains("Use Plus for the + key"));
        assert!(reference.contains("Primary means Command on macOS and Ctrl on Linux/Windows"));
    }

    #[test]
    fn reference_names_every_supported_non_alphanumeric_key() {
        let reference = Keybindings::reference();
        let (_, keys) = reference.split_once("Other keys:\n").unwrap();
        for name in keys
            .lines()
            .flat_map(|line| line.trim_start_matches("# ").split(", "))
        {
            let chord = KeyChord::try_from(name.to_owned()).unwrap();
            assert!(KeyChord::bindable(chord.key));
        }
        for &key in Key::ALL {
            let name = key.name();
            if KeyChord::bindable(key) && name.len() > 1 && !name.starts_with('F') {
                assert!(keys.contains(name), "{name}");
            }
        }
    }

    #[test]
    fn keybindings_omitted_keep_the_existing_defaults() {
        let config: Config = toml::from_str("font_size = 16").unwrap();
        config.keybindings.validate().unwrap();
        assert_eq!(
            config.keybindings.resolve(Key::D, None, HOST, Key::D),
            Some(BindingAction::SplitRight)
        );
        assert_eq!(config.keybindings.resolve(Key::C, None, CTRL, Key::C), None);
    }

    #[test]
    fn keybindings_replace_defaults_disable_actions_and_roundtrip() {
        let mut config: Config = toml::from_str(
            r#"
            [keybindings]
            split-right = ["alt+h", "ctrl+shift+F10"]
            split-below = []
            increase-font-size = ["primary+F8"]
        "#,
        )
        .unwrap();
        config.validate().unwrap();
        let bindings = &config.keybindings;
        assert_eq!(
            bindings.resolve(Key::H, None, Modifiers::ALT, Key::H),
            Some(BindingAction::SplitRight)
        );
        assert_eq!(
            bindings.resolve(Key::H, None, Modifiers::ALT | Modifiers::SHIFT, Key::H),
            None
        );
        assert_eq!(bindings.resolve(Key::D, None, HOST, Key::D), None);
        assert_eq!(bindings.resolve(Key::E, None, HOST, Key::E), None);
        assert_eq!(bindings.hint(BindingAction::SplitBelow), "");
        let saved = toml::to_string(&config).unwrap();
        assert_eq!(toml::from_str::<Config>(&saved).unwrap(), config);
    }

    #[test]
    fn keybindings_conflicts_include_defaults_and_aliases() {
        let default_split = String::from(host(Key::D));
        let cases = [
            format!("[keybindings]\nfind = [\"{default_split}\"]"),
            "[keybindings]\nfind = [\"ctrl+f12\", \"control+F12\"]".into(),
            "[keybindings]\nfind = [\"alt+x\"]\ncopy = [\"option+X\"]".into(),
        ];
        for source in cases {
            let mut config: Config = toml::from_str(&source).unwrap();
            assert!(
                config
                    .validate()
                    .unwrap_err()
                    .to_string()
                    .contains("Conflicting keybinding")
            );
        }
    }

    #[test]
    fn keybindings_reject_unknown_actions_keys_modifiers_and_reserved_escape() {
        for source in [
            "[keybindings]\nunknown-action = []",
            "[keybindings]\nfind = [\"ctrl+unknown\"]",
            "[keybindings]\nfind = [\"hyper+F1\"]",
            "[keybindings]\nfind = [\"ctrl+control+F1\"]",
            "[keybindings]\nfind = [\"Escape\"]",
            "[keybindings]\nfind = [\"Ctrl++\"]",
            "[keybindings]\nfind = [\"Ctrl+Copy\"]",
            "[keybindings]\nfind = \"ctrl+f\"",
        ] {
            assert!(toml::from_str::<Config>(source).is_err(), "{source}");
        }
    }

    #[test]
    fn keybindings_numbered_workspace_overrides_use_logical_keys() {
        let mut config: Config = toml::from_str(
            r#"[keybindings]
            workspace-1 = ["Alt+1"]
            workspace-9 = []
        "#,
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(
            config
                .keybindings
                .resolve(Key::Num1, Some(Key::Num2), Modifiers::ALT, Key::Num1),
            Some(BindingAction::Workspace1)
        );
        assert_eq!(
            config.keybindings.resolve(Key::Num9, None, HOST, Key::Num9),
            None
        );
        assert_eq!(
            config.keybindings.resolve(
                Key::Exclamationmark,
                Some(Key::Num1),
                HOST,
                Key::Exclamationmark
            ),
            None
        );
    }

    #[test]
    fn keybindings_bound_the_shortcut_count() {
        let mut config: Config = toml::from_str(
            r#"[keybindings]
            find = ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9"]
        "#,
        )
        .unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn keybindings_can_reassign_a_default_after_disabling_its_owner() {
        let source = format!(
            "[keybindings]\nsplit-right = []\nfind = [\"{}\"]",
            String::from(host(Key::D))
        );
        let mut config: Config = toml::from_str(&source).unwrap();
        config.validate().unwrap();
        assert_eq!(
            config.keybindings.resolve(Key::D, None, HOST, Key::D),
            Some(BindingAction::Find)
        );
    }

    #[test]
    fn keybindings_invalid_load_preserves_the_original_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        let source = "[keybindings]\nfind = [\"invalid-key\"]";
        std::fs::write(&path, source).unwrap();
        assert!(Config::load(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
    }
}
