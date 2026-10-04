//! What each running CLI agent is doing. Ephemeral and bounded by the panes
//! that run one: nothing here is saved, logged or sent to diagnostics.
//!
//! The agent's hooks say what it is doing. Neither CLI has a hook for every
//! way a turn or a question can end, so the terminal title, which both keep
//! current, corrects a report that it has contradicted for a while.
use neptune_model::{AgentKind, Lifecycle, Model, PaneId};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

/// A title outweighs a report it has contradicted for this long; shorter
/// disagreements are the two arriving in either order.
const HOLD: Duration = Duration::from_secs(2);
/// A request the agent's own reviewer answers at once is not shown.
const DEBOUNCE: Duration = Duration::from_millis(400);

/// What an agent is waiting on a person for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Attention {
    Permission,
    Question,
    Plan,
    Input,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Working,
    Idle,
    NeedsInput(Attention),
}

/// A key that can answer or dismiss what an agent asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    /// Escape leaves a question, a plan or a permission request.
    Dismiss,
    /// Enter or a digit picks an option, which may refuse the request.
    Choose,
}

impl Reply {
    /// The reply a terminal write carries, when it is exactly one such key
    /// pressed alone, in the plain encoding or the kitty keyboard protocol's.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let code = match bytes {
            [byte] => u32::from(*byte),
            [0x1b, b'[', report @ .., b'u'] => {
                let mut fields = std::str::from_utf8(report).ok()?.split(';');
                let code = fields.next()?.parse().ok()?;
                // No modifiers, and a press rather than a repeat or a release.
                if !matches!(fields.next(), None | Some("1" | "1:1")) || fields.next().is_some() {
                    return None;
                }
                code
            }
            _ => return None,
        };
        match code {
            0x1b => Some(Self::Dismiss),
            0x0d | 0x31..=0x39 => Some(Self::Choose),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Title {
    Busy,
    Calm,
    /// The title itself says a person is needed.
    Blocked,
}

/// What a title says, or `None` when it carries no activity mark.
fn classify(kind: AgentKind, title: &str) -> Option<Title> {
    let first = title.chars().next()?;
    let spinner = matches!(first, '\u{2800}'..='\u{28ff}' | '\u{25d0}'..='\u{25d3}');
    match kind {
        AgentKind::Claude if spinner => Some(Title::Busy),
        AgentKind::Claude => (first == '\u{2733}').then_some(Title::Calm),
        AgentKind::Codex if title.contains("Action Required") => Some(Title::Blocked),
        AgentKind::Codex if spinner => Some(Title::Busy),
        AgentKind::Codex => Some(Title::Calm),
    }
}

/// What the agent calls its conversation: its title without the activity
/// mark. `None` for a title that is not the agent's or names nothing.
pub fn conversation(kind: AgentKind, title: &str) -> Option<&str> {
    let class = classify(kind, title)?;
    let name = match (kind, class) {
        (_, Title::Blocked) => return None,
        (AgentKind::Codex, Title::Calm) => title,
        _ => title.strip_prefix(|_: char| true)?,
    }
    .trim();
    (!name.is_empty()).then_some(name)
}

#[derive(Clone, Debug)]
struct Entry {
    generation: u64,
    /// As the agent's hooks last reported it.
    reported: Activity,
    reported_at: Instant,
    /// What was shown when that report arrived.
    previous: Activity,
    title: Option<(Title, Instant)>,
    /// The title the terminal had when the agent opened: its shell's.
    shell_title: Option<String>,
    /// The agent has set a title of its own.
    titled: bool,
    /// The title has shown work during this run, so its calm means rest.
    live: bool,
    replied: Option<(Reply, Instant)>,
    shown: Activity,
    since: Instant,
}

impl Entry {
    /// What to show at `now`, and when that could next change without a new
    /// report or title.
    fn resolve(&self, now: Instant) -> (Activity, Option<Instant>) {
        let title = self.title.map(|(title, _)| title);
        if title == Some(Title::Blocked) {
            let attention = match self.reported {
                Activity::NeedsInput(attention) => attention,
                _ => Attention::Input,
            };
            return (Activity::NeedsInput(attention), None);
        }
        // Evidence counts from the later of its arrival and the report's.
        let held = |wanted: Title| {
            self.title
                .filter(|(title, _)| *title == wanted)
                .map(|(_, since)| since.max(self.reported_at) + HOLD)
        };
        let overruled = |deadline: Option<Instant>, by: Activity| match deadline {
            Some(at) if now >= at => (by, None),
            deadline => (self.reported, deadline),
        };
        match self.reported {
            Activity::Working => overruled(held(Title::Calm).filter(|_| self.live), Activity::Idle),
            Activity::Idle => overruled(held(Title::Busy), Activity::Working),
            Activity::NeedsInput(attention) => {
                let shown_at = self.reported_at + DEBOUNCE;
                if now < shown_at {
                    return (self.previous, Some(shown_at));
                }
                let resumed = held(Title::Busy);
                if resumed.is_some_and(|at| now >= at) {
                    return (Activity::Working, None);
                }
                // A choice can also allow the work, which the title then shows;
                // without a live title the two cannot be told apart.
                let refused = self.replied.and_then(|(reply, at)| {
                    let certain = reply == Reply::Dismiss
                        || (self.live
                            && matches!(attention, Attention::Permission | Attention::Plan));
                    (certain && title != Some(Title::Busy))
                        .then(|| at.max(self.title.map_or(at, |(_, since)| since)) + HOLD)
                });
                overruled(refused.or(resumed), Activity::Idle)
            }
        }
    }
}

#[derive(Default)]
pub struct AgentActivities {
    entries: BTreeMap<PaneId, Entry>,
}

impl AgentActivities {
    /// A report from the agent's hooks; `None` when the agent left.
    pub fn report(
        &mut self,
        pane: PaneId,
        generation: u64,
        activity: Option<Activity>,
        now: Instant,
    ) {
        let Some(activity) = activity else {
            self.entries.remove(&pane);
            return;
        };
        match self.entries.get_mut(&pane) {
            Some(entry) if entry.generation == generation => {
                entry.previous = entry.shown;
                entry.reported = activity;
                entry.reported_at = now;
                entry.replied = None;
            }
            _ => {
                self.entries.insert(
                    pane,
                    Entry {
                        generation,
                        reported: activity,
                        reported_at: now,
                        previous: activity,
                        title: None,
                        shell_title: None,
                        titled: false,
                        live: false,
                        replied: None,
                        shown: activity,
                        since: now,
                    },
                );
            }
        }
    }

    /// The title of a terminal, as often as it is read.
    pub fn title(&mut self, pane: PaneId, kind: AgentKind, title: &str, now: Instant) {
        let Some(entry) = self.entries.get_mut(&pane) else {
            return;
        };
        match &entry.shell_title {
            None => entry.shell_title = Some(title.to_owned()),
            Some(shell) => entry.titled |= shell != title,
        }
        // Until the agent draws, the title is the shell's and says nothing.
        let title = classify(kind, title).filter(|_| entry.titled);
        let was = entry.title.map(|(title, _)| title);
        if was != title {
            // Work straight after a request in the title is its answer, not
            // a disagreement to wait out.
            let since = if was == Some(Title::Blocked) && title == Some(Title::Busy) {
                now.checked_sub(HOLD).unwrap_or(now)
            } else {
                now
            };
            entry.title = title.map(|title| (title, since));
        }
        entry.live |= title == Some(Title::Busy);
    }

    /// A key typed into a terminal whose agent is waiting.
    pub fn reply(&mut self, pane: PaneId, reply: Reply, now: Instant) {
        // Once a reply has ended the wait, later keys are the next prompt's.
        if let Some(entry) = self.entries.get_mut(&pane)
            && matches!(entry.reported, Activity::NeedsInput(_))
            && (now < entry.reported_at + DEBOUNCE
                || matches!(entry.resolve(now).0, Activity::NeedsInput(_)))
        {
            entry.replied = Some((reply, now));
        }
    }

    /// Whether a key typed into this terminal is worth reporting.
    pub fn waiting(&self, pane: PaneId) -> bool {
        self.entries
            .get(&pane)
            .is_some_and(|entry| matches!(entry.reported, Activity::NeedsInput(_)))
    }

    /// Drops agents whose terminal closed, restarted or stopped. The agent's
    /// own exit is reported; its reference can reach the model a frame after
    /// its first report, so a missing one is not a reason to drop it.
    pub fn retain(&mut self, model: &Model) {
        self.entries.retain(|pane, entry| {
            model.pane(*pane).is_some_and(|pane| {
                pane.generation() == entry.generation
                    && matches!(pane.lifecycle(), Lifecycle::Starting | Lifecycle::Running)
            })
        });
    }

    /// Brings what is shown up to `now`. Returns whether anything changed and
    /// how long until it could change again by time alone.
    pub fn settle(&mut self, now: Instant) -> (bool, Option<Duration>) {
        let mut changed = false;
        let mut next: Option<Instant> = None;
        for entry in self.entries.values_mut() {
            let (shown, deadline) = entry.resolve(now);
            if shown != entry.shown {
                entry.shown = shown;
                entry.since = now;
                changed = true;
            }
            next = match (next, deadline) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
        }
        (changed, next.map(|at| at.saturating_duration_since(now)))
    }

    /// Whether the terminal's title is the agent's own by now.
    pub fn titled(&self, pane: PaneId) -> bool {
        self.entries.get(&pane).is_some_and(|entry| entry.titled)
    }

    /// What the agent in `pane` is doing and since when.
    pub fn get(&self, pane: PaneId) -> Option<(Activity, Instant)> {
        self.entries
            .get(&pane)
            .map(|entry| (entry.shown, entry.since))
    }

    pub fn waiting_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| matches!(entry.shown, Activity::NeedsInput(_)))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: AgentKind = AgentKind::Claude;
    const CODEX: AgentKind = AgentKind::Codex;
    const ASK: Activity = Activity::NeedsInput(Attention::Question);

    fn pane() -> PaneId {
        PaneId::new(1)
    }
    /// An agent at `activity` whose terminal still shows its shell's title.
    fn opened(kind: AgentKind, activity: Activity, now: Instant) -> AgentActivities {
        let mut agents = AgentActivities::default();
        agents.report(pane(), 1, Some(activity), now);
        agents.title(pane(), kind, "user@host: ~/project", now);
        agents
    }
    fn shown(agents: &mut AgentActivities, now: Instant) -> Activity {
        agents.settle(now);
        agents.get(pane()).unwrap().0
    }
    fn secs(value: f64) -> Duration {
        Duration::from_millis((value * 1000.0).round() as u64)
    }

    #[test]
    fn titles_are_read_per_agent_and_other_titles_say_nothing() {
        assert_eq!(
            classify(CLAUDE, "\u{2733} Fix the build"),
            Some(Title::Calm)
        );
        assert_eq!(
            classify(CLAUDE, "\u{25d0} Fix the build"),
            Some(Title::Busy)
        );
        assert_eq!(
            classify(CLAUDE, "\u{2819} Fix the build"),
            Some(Title::Busy)
        );
        assert_eq!(classify(CLAUDE, "zsh"), None);
        assert_eq!(classify(CLAUDE, ""), None);
        assert_eq!(classify(CODEX, "\u{280b} neptune"), Some(Title::Busy));
        assert_eq!(
            classify(CODEX, "[ ! ] Action Required"),
            Some(Title::Blocked)
        );
        assert_eq!(classify(CODEX, "neptune"), Some(Title::Calm));
        assert_eq!(classify(CODEX, ""), None);
    }

    #[test]
    fn a_conversation_is_named_by_its_title_without_the_mark() {
        assert_eq!(
            conversation(CLAUDE, "\u{2733} Fix the build"),
            Some("Fix the build")
        );
        assert_eq!(
            conversation(CLAUDE, "\u{25d0} Fix the build"),
            Some("Fix the build")
        );
        assert_eq!(conversation(CLAUDE, "\u{2733} "), None);
        assert_eq!(conversation(CLAUDE, "zsh"), None);
        assert_eq!(conversation(CODEX, "\u{280b} neptune"), Some("neptune"));
        assert_eq!(conversation(CODEX, "neptune"), Some("neptune"));
        assert_eq!(conversation(CODEX, "[ ! ] Action Required"), None);
    }

    #[test]
    fn a_report_is_shown_at_once_and_leaves_with_its_agent() {
        let mut agents = AgentActivities::default();
        let start = Instant::now();
        assert!(agents.get(pane()).is_none());
        agents.report(pane(), 3, Some(Activity::Idle), start);
        assert_eq!(agents.get(pane()), Some((Activity::Idle, start)));
        agents.report(pane(), 3, Some(Activity::Working), start + secs(1.0));
        assert_eq!(agents.settle(start + secs(1.0)), (true, None));
        assert_eq!(
            agents.get(pane()),
            Some((Activity::Working, start + secs(1.0)))
        );
        // The same report again changes nothing that is shown.
        agents.report(pane(), 3, Some(Activity::Working), start + secs(5.0));
        assert_eq!(agents.settle(start + secs(5.0)), (false, None));
        assert_eq!(agents.get(pane()).unwrap().1, start + secs(1.0));
        agents.report(pane(), 3, None, start + secs(6.0));
        assert!(agents.get(pane()).is_none());
    }

    #[test]
    fn an_interrupted_turn_rests_once_the_title_has_stayed_calm() {
        let start = Instant::now();
        let mut agents = opened(CLAUDE, Activity::Working, start);
        // A title that never showed work says nothing about rest.
        agents.title(pane(), CLAUDE, "\u{2733} Task", start);
        assert_eq!(shown(&mut agents, start + secs(60.0)), Activity::Working);
        agents.title(pane(), CLAUDE, "\u{25d0} Task", start + secs(60.0));
        agents.title(pane(), CLAUDE, "\u{25d1} Task", start + secs(61.0));
        agents.title(pane(), CLAUDE, "\u{2733} Task", start + secs(70.0));
        let (changed, wait) = agents.settle(start + secs(70.5));
        assert!(!changed);
        assert_eq!(wait, Some(secs(1.5)));
        assert_eq!(shown(&mut agents, start + secs(72.0)), Activity::Idle);
        // A new prompt is believed again, however calm the title still is.
        agents.report(pane(), 1, Some(Activity::Working), start + secs(80.0));
        assert_eq!(shown(&mut agents, start + secs(80.0)), Activity::Working);
        assert_eq!(shown(&mut agents, start + secs(81.9)), Activity::Working);
        agents.title(pane(), CLAUDE, "\u{25d0} Task", start + secs(81.95));
        assert_eq!(shown(&mut agents, start + secs(200.0)), Activity::Working);
    }

    #[test]
    fn a_busy_title_outweighs_rest_and_an_answered_request() {
        let start = Instant::now();
        let mut agents = opened(CODEX, Activity::Idle, start);
        agents.title(pane(), CODEX, "\u{280b} neptune", start + secs(1.0));
        assert_eq!(shown(&mut agents, start + secs(2.0)), Activity::Idle);
        assert_eq!(shown(&mut agents, start + secs(3.0)), Activity::Working);
        agents.title(pane(), CODEX, "neptune", start + secs(4.0));
        assert_eq!(shown(&mut agents, start + secs(4.0)), Activity::Idle);

        agents.report(pane(), 1, Some(ASK), start + secs(10.0));
        assert_eq!(shown(&mut agents, start + secs(11.0)), ASK);
        agents.title(pane(), CODEX, "\u{2819} neptune", start + secs(20.0));
        assert_eq!(shown(&mut agents, start + secs(21.0)), ASK);
        assert_eq!(shown(&mut agents, start + secs(22.0)), Activity::Working);
    }

    #[test]
    fn a_request_answered_at_once_is_never_shown() {
        let mut agents = AgentActivities::default();
        let start = Instant::now();
        agents.report(pane(), 1, Some(Activity::Working), start);
        agents.report(pane(), 1, Some(ASK), start + secs(1.0));
        let (changed, wait) = agents.settle(start + secs(1.1));
        assert!(!changed);
        assert_eq!(wait, Some(secs(0.3)));
        agents.report(pane(), 1, Some(Activity::Working), start + secs(1.2));
        assert_eq!(agents.settle(start + secs(1.5)), (false, None));
        agents.report(pane(), 1, Some(ASK), start + secs(2.0));
        assert_eq!(shown(&mut agents, start + secs(2.4)), ASK);
        assert_eq!(agents.waiting_count(), 1);
    }

    #[test]
    fn a_dismissed_or_refused_request_rests_and_an_allowed_one_works() {
        let start = Instant::now();
        let permission = Activity::NeedsInput(Attention::Permission);
        let asked = |activity| {
            let mut agents = opened(CLAUDE, Activity::Working, start);
            agents.title(pane(), CLAUDE, "\u{25d0} Task", start);
            agents.report(pane(), 1, Some(activity), start + secs(1.0));
            agents.title(pane(), CLAUDE, "\u{2733} Task", start + secs(1.0));
            agents
        };
        // Escape leaves a question; nothing reports it.
        let mut agents = asked(ASK);
        assert!(agents.waiting(pane()));
        agents.reply(pane(), Reply::Dismiss, start + secs(5.0));
        assert_eq!(shown(&mut agents, start + secs(6.0)), ASK);
        assert_eq!(shown(&mut agents, start + secs(7.0)), Activity::Idle);
        // Enter moves between the questions of one request.
        let mut agents = asked(ASK);
        agents.reply(pane(), Reply::Choose, start + secs(5.0));
        assert_eq!(shown(&mut agents, start + secs(60.0)), ASK);
        // A refused permission leaves the title calm.
        let mut agents = asked(permission);
        agents.reply(pane(), Reply::Choose, start + secs(5.0));
        assert_eq!(shown(&mut agents, start + secs(7.0)), Activity::Idle);
        // An allowed one runs, long before its tool reports back.
        let mut agents = asked(permission);
        agents.reply(pane(), Reply::Choose, start + secs(5.0));
        agents.title(pane(), CLAUDE, "\u{25d1} Task", start + secs(5.1));
        assert_eq!(shown(&mut agents, start + secs(6.0)), permission);
        assert_eq!(shown(&mut agents, start + secs(7.1)), Activity::Working);
        // Keys typed after the wait has ended belong to the next prompt.
        let mut agents = asked(ASK);
        agents.reply(pane(), Reply::Dismiss, start + secs(5.0));
        assert_eq!(shown(&mut agents, start + secs(7.0)), Activity::Idle);
        agents.reply(pane(), Reply::Choose, start + secs(10.0));
        agents.reply(pane(), Reply::Dismiss, start + secs(10.0));
        assert_eq!(shown(&mut agents, start + secs(10.0)), Activity::Idle);
        assert_eq!(shown(&mut agents, start + secs(11.0)), Activity::Idle);
        // A key typed while nothing is asked is not a reply.
        let mut agents = AgentActivities::default();
        agents.report(pane(), 1, Some(Activity::Working), start);
        agents.reply(pane(), Reply::Dismiss, start);
        assert!(!agents.waiting(pane()));
        assert_eq!(shown(&mut agents, start + secs(9.0)), Activity::Working);
    }

    #[test]
    fn a_title_that_asks_for_action_needs_input_without_any_hook() {
        let start = Instant::now();
        let mut agents = opened(CODEX, Activity::Idle, start);
        assert!(!agents.titled(pane()));
        // The shell's title is not the agent's, whatever it reads like.
        agents.title(pane(), CODEX, "user@host: ~/project", start + secs(0.5));
        assert_eq!(shown(&mut agents, start + secs(60.0)), Activity::Idle);
        assert!(!agents.titled(pane()));
        agents.title(pane(), CODEX, "[ . ] Action Required", start + secs(1.0));
        assert!(agents.titled(pane()));
        assert_eq!(
            shown(&mut agents, start + secs(1.0)),
            Activity::NeedsInput(Attention::Input)
        );
        // Work after the request is its answer, shown without a pause at rest.
        agents.title(pane(), CODEX, "\u{280b} neptune", start + secs(2.0));
        assert_eq!(shown(&mut agents, start + secs(2.0)), Activity::Working);
        agents.title(pane(), CODEX, "neptune", start + secs(3.0));
        assert_eq!(shown(&mut agents, start + secs(3.0)), Activity::Idle);
    }

    #[test]
    fn replies_are_single_keys() {
        for escape in [&b"\x1b"[..], b"\x1b[27u", b"\x1b[27;1u", b"\x1b[27;1:1u"] {
            assert_eq!(Reply::from_bytes(escape), Some(Reply::Dismiss));
        }
        for choice in [&b"\r"[..], b"2", b"\x1b[13u", b"\x1b[13;1u", b"\x1b[50;1u"] {
            assert_eq!(Reply::from_bytes(choice), Some(Reply::Choose));
        }
        // Arrows, letters, chords, releases and pasted text answer nothing.
        for bytes in [
            &b"\x1b[A"[..],
            b"y",
            b"0",
            b"12",
            b"",
            b"\x1b[27;5u",
            b"\x1b[27;1:3u",
            b"\x1b[97;1u",
            b"\x1b[27;1;1u",
            b"\x1b[u",
        ] {
            assert_eq!(Reply::from_bytes(bytes), None, "{bytes:?}");
        }
    }

    #[test]
    fn agents_leave_with_their_terminal_generation_and_reference() {
        use neptune_model::{AgentSession, Command, Controller};
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspace {
                cwd: std::env::temp_dir(),
                name: "a".into(),
                remote: None,
                group: None,
            })
            .unwrap();
        let id = controller.model().active_pane().unwrap();
        let generation = controller.model().pane(id).unwrap().generation();
        let mut agents = AgentActivities::default();
        let now = Instant::now();
        let open = |controller: &mut Controller, generation| {
            controller
                .dispatch(Command::PaneAgentChanged {
                    pane: id,
                    generation,
                    agent: Some(AgentSession {
                        kind: CLAUDE,
                        session_id: None,
                        cwd: std::env::temp_dir(),
                    }),
                })
                .unwrap();
        };
        // A report can precede the reference by a frame; it is kept for it.
        agents.report(id, generation, Some(Activity::Working), now);
        agents.retain(controller.model());
        assert!(agents.get(id).is_some());
        open(&mut controller, generation);
        agents.report(id, generation, Some(Activity::Working), now);
        agents.retain(controller.model());
        assert!(agents.get(id).is_some());
        controller.dispatch(Command::RestartPane(id)).unwrap();
        agents.retain(controller.model());
        assert!(agents.get(id).is_none());
        // A report from the replaced shell does not describe the new one.
        open(&mut controller, generation + 1);
        agents.report(id, generation, Some(Activity::Working), now);
        agents.retain(controller.model());
        assert!(agents.get(id).is_none());
        // A terminal whose shell has exited runs no agent, whatever it keeps.
        agents.report(id, generation + 1, Some(Activity::Working), now);
        controller
            .dispatch(Command::SessionStarted {
                pane: id,
                generation: generation + 1,
            })
            .unwrap();
        agents.retain(controller.model());
        assert!(agents.get(id).is_some());
        controller
            .dispatch(Command::SessionExited {
                pane: id,
                generation: generation + 1,
            })
            .unwrap();
        assert!(!matches!(
            controller.model().pane(id).unwrap().lifecycle(),
            Lifecycle::Starting | Lifecycle::Running
        ));
        agents.retain(controller.model());
        assert!(agents.get(id).is_none());
    }
}
