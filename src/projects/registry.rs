//! What a project keeps beside its chat, as `project.json` holds it: the
//! lead's conversation, the agents it started and what became of them, and
//! its counters. The store reads and writes it; the rules are here.
use super::{settings::Settings, subscription::Subscription, transcript::clip};
use neptune_model::{AgentKind, Worktree};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The format this build reads and writes.
pub const VERSION: u32 = 1;
/// Agents a project remembers; the oldest ended ones leave first.
pub const MAX_TASKS: usize = 64;
/// What is kept of an agent's last reply.
pub const MAX_REPORT: usize = 8 * 1024;
pub const MAX_SUBSCRIPTIONS: usize = 16;
/// Agents a project starts in one day.
pub const MAX_SPAWNS_A_DAY: u32 = 40;
const MAX_AUTO_TURNS: usize = 64;
const MAX_TITLE: usize = 512;
const MAX_LINKS: usize = 16;
const DAY: u64 = 24 * 3600;
/// The longest effort level kept with an agent.
const MAX_EFFORT: usize = 16;

/// The lead's conversation with its CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeadRef {
    pub kind: AgentKind,
    #[serde(default)]
    pub session: Option<String>,
    /// A handshake succeeded in it once: it is resumed from then on.
    #[serde(default)]
    pub opened: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Starting,
    Working,
    /// It waits for a person in its terminal.
    Waiting,
    /// Its turn ended: there is something to look at.
    Review,
    Ended,
}
/// One agent a project started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    /// The number its lead and the person know it by.
    pub n: u64,
    pub title: String,
    pub kind: AgentKind,
    /// Its conversation as its CLI named it, to open it again.
    #[serde(default)]
    pub session: Option<String>,
    pub cwd: PathBuf,
    #[serde(default)]
    pub worktree: Option<Worktree>,
    #[serde(default)]
    pub pull_requests: Vec<String>,
    pub state: TaskState,
    #[serde(default)]
    pub last_report: Option<String>,
    /// Seconds since the Unix epoch.
    pub started: u64,
    #[serde(default)]
    pub ended: Option<u64>,
    /// Its worktree's branch was merged, and the lead was told.
    #[serde(default, skip_serializing_if = "is_false")]
    pub merged: bool,
    /// What it was started with, so that one opened again after Neptune
    /// was closed runs with the same. Absent where its CLI chose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub ultracode: bool,
}
fn is_false(value: &bool) -> bool {
    !*value
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counters {
    #[serde(default)]
    pub spawned_today: u32,
    /// The day `spawned_today` counts, in days since the Unix epoch.
    #[serde(default)]
    pub day: u64,
    /// When Neptune began turns by itself. Kept for a later build.
    #[serde(default)]
    pub auto_turns: Vec<u64>,
    /// The numbers given to watches so far: none names two.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub watches: u64,
}
fn is_zero(value: &u64) -> bool {
    *value == 0
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Saved {
    pub version: u32,
    /// What the project is called and where it works, so that it can be
    /// offered again after its workspace was closed.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub directory: PathBuf,
    #[serde(default)]
    pub lead: Option<LeadRef>,
    #[serde(default)]
    pub tasks: Vec<Task>,
    /// Its watches: what wakes its lead beside its agents.
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
    #[serde(default)]
    pub counters: Counters,
    /// What the person chose for its lead and its agents. Absent in a file
    /// written before there was anything to choose.
    #[serde(default, skip_serializing_if = "Settings::is_default")]
    pub settings: Settings,
}
impl Default for Saved {
    fn default() -> Self {
        Self {
            version: VERSION,
            name: String::new(),
            directory: PathBuf::new(),
            lead: None,
            tasks: Vec::new(),
            subscriptions: Vec::new(),
            counters: Counters::default(),
            settings: Settings::default(),
        }
    }
}
impl Saved {
    /// Within the bounds this build writes. A file outside them is damaged.
    pub fn is_valid(&self) -> bool {
        self.version == VERSION
            && self.name.len() <= MAX_TITLE
            && self.tasks.len() <= MAX_TASKS
            && self.subscriptions.len() <= MAX_SUBSCRIPTIONS
            && self.subscriptions.iter().all(Subscription::is_valid)
            && self.counters.auto_turns.len() <= MAX_AUTO_TURNS
            && self.settings.is_valid()
            && self.tasks.iter().all(|task| {
                task.n > 0
                    && task.title.len() <= MAX_TITLE
                    && task.pull_requests.len() <= MAX_LINKS
                    && task
                        .last_report
                        .as_ref()
                        .is_none_or(|report| report.len() <= MAX_REPORT)
                    && task
                        .model
                        .as_deref()
                        .is_none_or(|model| super::settings::model_name(model).is_some())
                    && task
                        .effort
                        .as_ref()
                        .is_none_or(|effort| effort.len() <= MAX_EFFORT)
            })
    }
}

/// The agents a project started, newest last.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    tasks: Vec<Task>,
}
impl Registry {
    /// Tasks as they were read; one number names one task.
    pub fn restore(tasks: Vec<Task>) -> Self {
        let mut registry = Self::default();
        for task in tasks {
            registry.tasks.retain(|known| known.n != task.n);
            registry.tasks.push(task);
        }
        registry.prune();
        registry
    }
    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }
    pub fn get(&self, n: u64) -> Option<&Task> {
        self.tasks.iter().find(|task| task.n == n)
    }
    /// An agent was started. `resumed` is the closed one it goes on from,
    /// whose record it takes over. A number used again names the new agent.
    pub fn started(&mut self, mut task: Task, resumed: Option<u64>) {
        let earlier = resumed
            .and_then(|n| self.tasks.iter().position(|task| task.n == n))
            .map(|at| self.tasks.remove(at));
        if let Some(earlier) = earlier {
            task.last_report = earlier.last_report;
            if task.title.is_empty() {
                task.title = earlier.title;
            }
            // One opened again that was not told what to run with goes on
            // with what the earlier one had.
            if task.model.is_none() && task.effort.is_none() && !task.ultracode {
                task.model = earlier.model;
                task.effort = earlier.effort;
                task.ultracode = earlier.ultracode;
            }
        }
        task.title = clip(&task.title, MAX_TITLE).to_owned();
        self.tasks.retain(|known| known.n != task.n);
        self.tasks.push(task);
        self.prune();
    }
    pub fn get_mut(&mut self, n: u64) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|task| task.n == n)
    }
    /// Keeps `reply` as what the last turn of task `n` ended with, within
    /// its limit. Returns whether that changed anything.
    pub fn reported(&mut self, n: u64, reply: &str) -> bool {
        let reply = clip(reply, MAX_REPORT);
        match self.get_mut(n) {
            Some(task) if task.last_report.as_deref() != Some(reply) => {
                task.last_report = Some(reply.to_owned());
                true
            }
            _ => false,
        }
    }
    /// Ends every task that is open and not among `open`. Returns whether
    /// any ended.
    pub fn end_others(&mut self, open: &[u64], now: u64) -> bool {
        let mut changed = false;
        for task in &mut self.tasks {
            if task.ended.is_none() && !open.contains(&task.n) {
                task.ended = Some(now);
                task.state = TaskState::Ended;
                changed = true;
            }
        }
        changed
    }
    fn prune(&mut self) {
        while self.tasks.len() > MAX_TASKS {
            // The oldest ended one, or the oldest of all where none ended.
            let at = self
                .tasks
                .iter()
                .position(|task| task.ended.is_some())
                .unwrap_or(0);
            self.tasks.remove(at);
        }
    }
}

impl Counters {
    /// Whether another agent may be started on the day of `now`.
    pub fn may_spawn(&self, now: u64) -> bool {
        self.day != now / DAY || self.spawned_today < MAX_SPAWNS_A_DAY
    }
    /// Counts one started agent on the day of `now`.
    pub fn spawned(&mut self, now: u64) {
        let day = now / DAY;
        if self.day != day {
            self.day = day;
            self.spawned_today = 0;
        }
        self.spawned_today = self.spawned_today.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(n: u64, ended: Option<u64>) -> Task {
        Task {
            n,
            title: format!("task {n}"),
            kind: AgentKind::Claude,
            session: None,
            cwd: "/work".into(),
            worktree: None,
            pull_requests: Vec::new(),
            state: if ended.is_some() {
                TaskState::Ended
            } else {
                TaskState::Working
            },
            last_report: None,
            started: n,
            ended,
            merged: false,
            model: None,
            effort: None,
            ultracode: false,
        }
    }

    #[test]
    fn what_a_project_keeps_has_the_shape_its_file_names() {
        let saved = Saved {
            name: "shop".into(),
            directory: "/code/shop".into(),
            lead: Some(LeadRef {
                kind: AgentKind::Claude,
                session: Some("s-1".into()),
                opened: true,
            }),
            tasks: vec![Task {
                session: Some("a-1".into()),
                pull_requests: vec!["https://example.test/o/r/pull/214".into()],
                state: TaskState::Review,
                last_report: Some("done".into()),
                ..task(12, None)
            }],
            counters: Counters {
                spawned_today: 7,
                day: 20366,
                auto_turns: Vec::new(),
                watches: 0,
            },
            ..Saved::default()
        };
        let text = serde_json::to_string(&saved).unwrap();
        assert_eq!(
            text,
            r#"{"version":1,"name":"shop","directory":"/code/shop","lead":{"kind":"claude","session":"s-1","opened":true},"tasks":[{"n":12,"title":"task 12","kind":"claude","session":"a-1","cwd":"/work","worktree":null,"pull_requests":["https://example.test/o/r/pull/214"],"state":"review","last_report":"done","started":12,"ended":null}],"subscriptions":[],"counters":{"spawned_today":7,"day":20366,"auto_turns":[]}}"#
        );
        assert_eq!(serde_json::from_str::<Saved>(&text).unwrap(), saved);
        assert!(saved.is_valid());
        // The least a file can say, and a watch as it is kept.
        let least: Saved = serde_json::from_str(r#"{"version":1}"#).unwrap();
        assert_eq!(least, Saved::default());
        // A file written before a project had settings names none, and
        // reads as nothing chosen; what was chosen is kept as it was set,
        // and a setting this build would not have written is damage.
        assert!(!text.contains("settings"));
        assert!(
            serde_json::from_str::<Saved>(&text)
                .unwrap()
                .settings
                .is_default()
        );
        let chosen: Saved = serde_json::from_str(
            r#"{"version":1,"settings":{"lead":{"model":"opus","effort":"high"},"claude":{"ultracode":false},"codex":{"effort":"ultra"}}}"#,
        )
        .unwrap();
        assert!(chosen.is_valid());
        assert_eq!(chosen.settings.lead.model.as_deref(), Some("opus"));
        assert_eq!(chosen.settings.claude.ultracode, Some(false));
        assert_eq!(chosen.settings.codex.effort.as_deref(), Some("ultra"));
        assert!(serde_json::to_string(&chosen).unwrap().ends_with(
            r#""settings":{"lead":{"model":"opus","effort":"high"},"claude":{"ultracode":false},"codex":{"effort":"ultra"}}}"#
        ));
        let blank: Saved =
            serde_json::from_str(r#"{"version":1,"settings":{"lead":{"model":""}}}"#).unwrap();
        assert!(!blank.is_valid());
        assert!(
            serde_json::from_str::<Saved>(r#"{"version":1,"settings":{"gemini":{}}}"#).is_err()
        );
        let watch = r#"{"id":3,"title":"Nightly","trigger":{"kind":"interval","minutes":60},"instruction":"Check the build","paused":false,"allowed":true,"next":1790003600,"last":null,"fires_today":2,"day":20717,"seen":null,"saved":1790000000}"#;
        let watched: Saved = serde_json::from_str(&format!(
            r#"{{"version":1,"subscriptions":[{watch}],"counters":{{"watches":3}}}}"#
        ))
        .unwrap();
        assert!(watched.is_valid());
        assert_eq!(watched.counters.watches, 3);
        let written = serde_json::to_string(&watched).unwrap();
        assert!(written.contains(&format!(r#""subscriptions":[{watch}]"#)));
        assert!(written.contains(r#""watches":3"#));
        // A watch this build would not have written is damage, and one of
        // a shape it does not know is not dropped silently.
        let often = watch.replace(r#""minutes":60"#, r#""minutes":1"#);
        let often: Saved =
            serde_json::from_str(&format!(r#"{{"version":1,"subscriptions":[{often}]}}"#)).unwrap();
        assert!(!often.is_valid());
        assert!(
            serde_json::from_str::<Saved>(
                r#"{"version":1,"subscriptions":[{"id":3,"title":"Nightly"}]}"#
            )
            .is_err()
        );
        // A field this build does not know is not dropped silently.
        assert!(serde_json::from_str::<Saved>(r#"{"version":1,"chat":[]}"#).is_err());
        assert!(
            serde_json::from_str::<Saved>(r#"{"version":1,"lead":{"kind":"claude","x":1}}"#)
                .is_err()
        );
        // Out of bounds is damaged, not trimmed.
        let many = Saved {
            tasks: (1..=MAX_TASKS as u64 + 1).map(|n| task(n, None)).collect(),
            ..Saved::default()
        };
        assert!(!many.is_valid());
        let long = Saved {
            tasks: vec![Task {
                last_report: Some("x".repeat(MAX_REPORT + 1)),
                ..task(1, None)
            }],
            ..Saved::default()
        };
        assert!(!long.is_valid());
        assert!(
            !Saved {
                version: 2,
                ..Saved::default()
            }
            .is_valid()
        );
    }

    #[test]
    fn a_project_remembers_its_agents_within_bounds_and_counts_a_days_starts() {
        let mut registry = Registry::default();
        for n in 1..=MAX_TASKS as u64 {
            registry.started(task(n, (n % 2 == 0).then_some(n)), None);
        }
        // One more: the oldest ended one leaves, never one still open.
        registry.started(task(100, None), None);
        assert_eq!(registry.tasks().len(), MAX_TASKS);
        assert!(registry.get(2).is_none() && registry.get(1).is_some());
        // A reply is kept within its limit; an unchanged task is not a change.
        let long = "é".repeat(MAX_REPORT);
        assert!(registry.reported(1, &long));
        assert_eq!(
            registry.get(1).unwrap().last_report.as_ref().unwrap().len(),
            MAX_REPORT
        );
        assert!(!registry.reported(1, &long) && !registry.reported(999, "x"));
        registry.get_mut(100).unwrap().state = TaskState::Review;
        // One opened again takes over the record of its closed terminal.
        assert!(registry.end_others(&[100], 500));
        assert!(!registry.end_others(&[100], 600));
        assert_eq!(
            (
                registry.get(1).unwrap().state,
                registry.get(1).unwrap().ended
            ),
            (TaskState::Ended, Some(500))
        );
        let earlier = registry.get_mut(1).unwrap();
        earlier.model = Some("sonnet".into());
        earlier.effort = Some("high".into());
        earlier.ultracode = true;
        registry.started(
            Task {
                title: String::new(),
                ..task(200, None)
            },
            Some(1),
        );
        let reopened = registry.get(200).unwrap();
        assert!(registry.get(1).is_none());
        assert_eq!(reopened.title, "task 1");
        // and runs with what that one ran with, unless it was told otherwise.
        assert_eq!(
            (
                reopened.model.as_deref(),
                reopened.effort.as_deref(),
                reopened.ultracode
            ),
            (Some("sonnet"), Some("high"), true)
        );
        registry.get_mut(200).unwrap().ended = Some(700);
        registry.started(
            Task {
                model: Some("opus".into()),
                ..task(201, None)
            },
            Some(200),
        );
        let told = registry.get(201).unwrap();
        assert_eq!(
            (
                told.model.as_deref(),
                told.effort.as_deref(),
                told.ultracode
            ),
            (Some("opus"), None, false)
        );
        // What an agent ran with is kept with it and read back; a record
        // written before that was kept reads as one whose CLI chose, and a
        // model that is no name is a damaged file.
        let kept = Saved {
            tasks: vec![Task {
                model: Some("sonnet".into()),
                effort: Some("high".into()),
                ultracode: true,
                ..task(7, None)
            }],
            ..Saved::default()
        };
        let text = serde_json::to_string(&kept).unwrap();
        assert!(text.contains(r#""model":"sonnet","effort":"high","ultracode":true"#));
        let back: Saved = serde_json::from_str(&text).unwrap();
        assert!(back == kept && back.is_valid());
        let old = serde_json::to_string(&Saved {
            tasks: vec![task(7, None)],
            ..Saved::default()
        })
        .unwrap();
        assert!(!old.contains("model") && !old.contains("ultracode"));
        let old: Saved = serde_json::from_str(&old).unwrap();
        assert_eq!(
            (old.tasks[0].model.clone(), old.tasks[0].ultracode),
            (None, false)
        );
        let bad = Saved {
            tasks: vec![Task {
                model: Some("--dangerous".into()),
                ..task(7, None)
            }],
            ..Saved::default()
        };
        assert!(!bad.is_valid());
        let reopened = registry.get(201).unwrap();
        assert_eq!(reopened.last_report.as_ref().unwrap().len(), MAX_REPORT);
        // A number used again names the new agent only.
        registry.started(task(3, None), None);
        assert_eq!(
            registry.tasks().iter().filter(|task| task.n == 3).count(),
            1
        );
        assert_eq!(registry.get(3).unwrap().ended, None);
        let restored = Registry::restore(vec![task(5, Some(1)), task(5, None)]);
        assert_eq!(restored.tasks().len(), 1);

        let mut counters = Counters::default();
        let day = 20_000 * DAY;
        for _ in 0..MAX_SPAWNS_A_DAY {
            assert!(counters.may_spawn(day + 5));
            counters.spawned(day + 5);
        }
        assert!(!counters.may_spawn(day + DAY - 1));
        assert_eq!(counters.spawned_today, MAX_SPAWNS_A_DAY);
        assert!(counters.may_spawn(day + DAY));
        counters.spawned(day + DAY);
        assert_eq!((counters.spawned_today, counters.day), (1, 20_001));
    }
}
