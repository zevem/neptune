//! What the person chose for a project: the model and effort its lead runs
//! with, and what the agents its lead starts run with, for each CLI. A value
//! that is not set leaves the choice where it was: with the lead's CLI for
//! the lead, and with the lead for its agents.
use neptune_model::AgentKind;
use serde::{Deserialize, Serialize};

/// The longest model name kept, as a CLI takes one.
pub const MAX_MODEL: usize = 80;
/// The longest effort level kept.
const MAX_EFFORT: usize = 16;

/// A model name as a CLI takes one: a short word, never an option. The
/// same rule for a name the person types and for one an agent passes.
pub fn model_name(value: &str) -> Option<&str> {
    let value = value.trim();
    (value.len() <= MAX_MODEL
        && value.starts_with(|first: char| first.is_ascii_alphanumeric())
        && value
            .chars()
            .all(|letter| letter.is_ascii_alphanumeric() || "._:/-[]@".contains(letter)))
    .then_some(value)
}
/// A model or an effort level as a list shows it: "xhigh" is "Extra high",
/// and a name nobody listed stays as it was typed.
pub fn label(value: &str) -> String {
    match value {
        "xhigh" => "Extra high".to_owned(),
        value if value.contains(['-', '.', '/', ':']) => value.to_owned(),
        value => {
            let mut letters = value.chars();
            letters.next().map_or(String::new(), |first| {
                first.to_uppercase().chain(letters).collect()
            })
        }
    }
}
/// Models offered by name for each CLI, as the installed CLIs named them
/// when this was written. Any other is typed: these are a convenience, not
/// the list of what a CLI takes.
pub fn models(kind: AgentKind) -> &'static [&'static str] {
    match kind {
        AgentKind::Claude => &["fable", "opus", "sonnet", "haiku"],
        AgentKind::Codex => &["gpt-6.1-sol", "gpt-6-astra", "gpt-6-sol", "gpt-6-luna"],
        _ => &[],
    }
}

/// What a project's lead runs with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeadSettings {
    /// None for the CLI's own choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}
/// What the agents a project's lead starts with one CLI run with. Each
/// value that is set wins over what the lead asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSettings {
    /// None where the lead decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Whether Claude Code agents run with ultracode on: always, never, or
    /// as the lead asks where it is None. Codex has no such switch: its
    /// Ultra is an effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ultracode: Option<bool>,
}
impl LeadSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}
impl AgentSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    /// What an agent of `kind` is started with: each value the person set,
    /// and otherwise what its lead asked for. `effort` and `ultracode` are
    /// the lead's as an agent's task holds them: Codex's Ultra is an effort.
    pub fn over(
        &self,
        kind: AgentKind,
        model: Option<String>,
        effort: Option<String>,
        ultracode: bool,
    ) -> (Option<String>, Option<String>, bool) {
        let model = self.model.clone().or(model);
        let effort = self.effort.clone().or(effort);
        match kind {
            AgentKind::Claude => (model, effort, self.ultracode.unwrap_or(ultracode)),
            _ => (model, effort, false),
        }
    }
    /// The same in a few words for the lead, or nothing where it decides.
    fn words(&self, kind: AgentKind) -> Option<String> {
        let mut said = Vec::new();
        if let Some(model) = &self.model {
            said.push(format!("model {model}"));
        }
        if let Some(effort) = &self.effort {
            said.push(format!("effort {effort}"));
        }
        if let Some(on) = self.ultracode.filter(|_| kind == AgentKind::Claude) {
            said.push(format!("ultracode {}", if on { "on" } else { "off" }));
        }
        (!said.is_empty()).then(|| said.join(", "))
    }
}

/// Everything the person chose for one project, as `project.json` keeps it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default, skip_serializing_if = "LeadSettings::is_default")]
    pub lead: LeadSettings,
    /// For the Claude Code agents its lead starts,
    #[serde(default, skip_serializing_if = "AgentSettings::is_default")]
    pub claude: AgentSettings,
    /// and for the Codex ones.
    #[serde(default, skip_serializing_if = "AgentSettings::is_default")]
    pub codex: AgentSettings,
}
impl Settings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    /// What the person set for the agents of `kind`; nothing for a CLI a
    /// lead does not start.
    pub fn agents(&self, kind: AgentKind) -> Option<&AgentSettings> {
        match kind {
            AgentKind::Claude => Some(&self.claude),
            AgentKind::Codex => Some(&self.codex),
            _ => None,
        }
    }
    /// Within the bounds this build writes. What each CLI takes is checked
    /// where the settings are made, by the rules of the CLIs.
    pub fn is_valid(&self) -> bool {
        let word = |value: &Option<String>, most: usize| {
            value
                .as_ref()
                .is_none_or(|value| !value.is_empty() && value.len() <= most)
        };
        [
            (&self.lead.model, &self.lead.effort),
            (&self.claude.model, &self.claude.effort),
            (&self.codex.model, &self.codex.effort),
        ]
        .iter()
        .all(|(model, effort)| word(model, MAX_MODEL) && word(effort, MAX_EFFORT))
    }
    /// What a lead's turn says of them, as whole lines: nothing where the
    /// lead decides everything.
    pub fn lines(&self) -> String {
        let said: Vec<String> = [
            (AgentKind::Claude, "claude", &self.claude),
            (AgentKind::Codex, "codex", &self.codex),
        ]
        .into_iter()
        .filter_map(|(kind, name, agents)| Some(format!("{name}: {}", agents.words(kind)?)))
        .collect();
        if said.is_empty() {
            return String::new();
        }
        format!(
            "agent settings (the user's; they win over what you pass to spawn_agent): {}\n",
            said.join(" | ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agents(model: Option<&str>, effort: Option<&str>, ultracode: Option<bool>) -> AgentSettings {
        AgentSettings {
            model: model.map(str::to_owned),
            effort: effort.map(str::to_owned),
            ultracode,
        }
    }
    fn some(word: &str) -> Option<String> {
        Some(word.to_owned())
    }

    #[test]
    fn what_the_person_set_wins_and_what_they_left_is_the_leads() {
        // Nothing set: the lead's own arguments stand.
        assert_eq!(
            AgentSettings::default().over(AgentKind::Claude, some("opus"), some("high"), true),
            (some("opus"), some("high"), true)
        );
        assert_eq!(
            AgentSettings::default().over(AgentKind::Codex, None, some("ultra"), false),
            (None, some("ultra"), false)
        );
        // Each value by itself: a set one wins, the others stay the lead's.
        assert_eq!(
            agents(Some("sonnet"), None, None).over(
                AgentKind::Claude,
                some("opus"),
                some("high"),
                false
            ),
            (some("sonnet"), some("high"), false)
        );
        assert_eq!(
            agents(None, Some("low"), None).over(AgentKind::Claude, some("opus"), None, true),
            (some("opus"), some("low"), true)
        );
        // Ultracode is beside the effort, and the person turns it on or off
        // whatever the lead asked.
        assert_eq!(
            agents(None, None, Some(true)).over(AgentKind::Claude, None, some("medium"), false),
            (None, some("medium"), true)
        );
        assert_eq!(
            agents(None, None, Some(false)).over(AgentKind::Claude, None, None, true),
            (None, None, false)
        );
        // Codex's Ultra is an effort like any other, and it has no ultracode.
        assert_eq!(
            agents(None, Some("ultra"), Some(true)).over(
                AgentKind::Codex,
                None,
                some("low"),
                false
            ),
            (None, some("ultra"), false)
        );
        assert_eq!(
            agents(None, Some("high"), None).over(AgentKind::Codex, None, some("ultra"), false),
            (None, some("high"), false)
        );
    }

    #[test]
    fn a_lead_is_told_only_what_was_set_and_older_files_read_as_nothing_set() {
        assert_eq!(Settings::default().lines(), "");
        let settings = Settings {
            lead: LeadSettings {
                model: some("opus"),
                effort: None,
            },
            claude: agents(Some("sonnet"), Some("high"), Some(true)),
            codex: agents(None, Some("ultra"), None),
        };
        assert_eq!(
            settings.lines(),
            "agent settings (the user's; they win over what you pass to spawn_agent): claude: \
             model sonnet, effort high, ultracode on | codex: effort ultra\n"
        );
        assert!(
            Settings {
                claude: agents(None, None, Some(false)),
                ..Settings::default()
            }
            .lines()
            .contains("claude: ultracode off")
        );
        // Only what is set is written, and it reads back as it was.
        let written = serde_json::to_value(&settings).unwrap();
        assert_eq!(
            written,
            serde_json::json!({
                "lead": {"model": "opus"},
                "claude": {"model": "sonnet", "effort": "high", "ultracode": true},
                "codex": {"effort": "ultra"},
            })
        );
        assert_eq!(
            serde_json::from_value::<Settings>(written).unwrap(),
            settings
        );
        // A group nothing was chosen in is not written at all.
        let one = Settings {
            lead: settings.lead.clone(),
            ..Settings::default()
        };
        assert_eq!(
            serde_json::to_string(&one).unwrap(),
            r#"{"lead":{"model":"opus"}}"#
        );
        let empty: Settings = serde_json::from_str("{}").unwrap();
        assert!(empty.is_default() && empty.is_valid());
        assert!(serde_json::from_str::<Settings>(r#"{"lead":{"speed":1}}"#).is_err());
        // Bounds: nothing empty and nothing longer than a CLI's name for it.
        let long = Settings {
            lead: LeadSettings {
                model: Some("m".repeat(MAX_MODEL + 1)),
                effort: None,
            },
            ..Settings::default()
        };
        assert!(!long.is_valid());
        let blank = Settings {
            codex: agents(None, Some(""), None),
            ..Settings::default()
        };
        assert!(!blank.is_valid());
        assert!(settings.is_valid());
        assert_eq!(settings.agents(AgentKind::Gemini), None);
        assert_eq!(models(AgentKind::Gemini), &[] as &[&str]);
        assert_eq!(
            ["opus", "xhigh", "gpt-6.1-sol", "max"].map(label),
            ["Opus", "Extra high", "gpt-6.1-sol", "Max"]
        );
    }
}
