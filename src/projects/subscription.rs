//! A project's watches: what wakes its lead beside its agents. One runs on
//! a schedule, with an instruction the person or the lead wrote; one follows
//! a pull request and speaks when its checks, its state or its open review
//! comments change. The rules are here: what may be added, how often one
//! may fire, and which changes of a pull request are worth a turn. The
//! store keeps them in `project.json`; nothing here reads a clock or a file.
use super::schedule;
use neptune_model::PullRequest;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Watches one project holds.
pub const MAX: usize = super::registry::MAX_SUBSCRIPTIONS;
/// A watch's name, in characters.
pub const MAX_TITLE: usize = 80;
/// What a watch tells the lead to do.
pub const MAX_INSTRUCTION: usize = 2 * 1024;
/// Times a schedule wakes the lead in one day,
pub const MAX_FIRES_A_DAY: u32 = 12;
/// and times one pull request does.
pub const MAX_PR_WAKES_A_DAY: u32 = 10;
const DAY: u64 = 24 * 3600;

fn is_false(value: &bool) -> bool {
    !*value
}

/// What sets a watch off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Trigger {
    /// Every so many minutes while Neptune is open.
    Interval { minutes: u32 },
    /// A change of this pull request.
    PullRequest {
        url: String,
        /// Neptune began it because an agent of the project linked the
        /// pull request.
        #[serde(default, skip_serializing_if = "is_false")]
        auto: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Review {
    Open,
    Draft,
    Merged,
    Closed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Checks {
    None,
    Passing,
    Pending,
    Failing,
}
/// A pull request as its watch last told the lead of it. Only what is
/// counted and named: never a word anyone wrote on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Seen {
    pub state: Review,
    pub checks: Checks,
    pub unresolved: u8,
}
impl Seen {
    fn in_review(self) -> bool {
        matches!(self.state, Review::Open | Review::Draft)
    }
    /// "open · checks failing · 2 unresolved review comments".
    pub fn words(self) -> String {
        let mut words = vec![
            match self.state {
                Review::Open => "open",
                Review::Draft => "draft",
                Review::Merged => "merged",
                Review::Closed => "closed",
            }
            .to_owned(),
        ];
        if self.in_review() {
            match self.checks {
                Checks::None => {}
                Checks::Passing => words.push("checks passing".into()),
                Checks::Pending => words.push("checks running".into()),
                Checks::Failing => words.push("checks failing".into()),
            }
            match self.unresolved {
                0 => {}
                1 => words.push("1 unresolved review comment".into()),
                count => words.push(format!("{count} unresolved review comments")),
            }
        }
        words.join(" · ")
    }
}

/// A change of a pull request that its lead is woken for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    ChecksFailing,
    ChecksPassing,
    Merged,
    Closed,
    /// More review comments are open than before.
    Unresolved {
        from: u8,
        to: u8,
    },
}
impl Change {
    pub fn words(self) -> String {
        match self {
            Self::ChecksFailing => "its checks are failing".into(),
            Self::ChecksPassing => "its checks pass".into(),
            Self::Merged => "it was merged".into(),
            Self::Closed => "it was closed without merging".into(),
            Self::Unresolved { from, to } => {
                format!("unresolved review comments went from {from} to {to}")
            }
        }
    }
    /// The pull request is done with: so is its watch.
    pub fn ends(self) -> bool {
        matches!(self, Self::Merged | Self::Closed)
    }
}
/// What changed from `before` to `now` that is worth a turn of the lead.
/// Checks that began to run, comments that were resolved and a draft made
/// ready are not. A pull request that was merged or closed says only that.
pub fn changes(before: Seen, now: Seen) -> Vec<Change> {
    let mut changes = Vec::new();
    if now.state != before.state {
        match now.state {
            Review::Merged => return vec![Change::Merged],
            Review::Closed => return vec![Change::Closed],
            Review::Open | Review::Draft => {}
        }
    }
    if !now.in_review() {
        return changes;
    }
    if now.checks != before.checks || !before.in_review() {
        match now.checks {
            Checks::Failing => changes.push(Change::ChecksFailing),
            Checks::Passing => changes.push(Change::ChecksPassing),
            Checks::None | Checks::Pending => {}
        }
    }
    let was = if before.in_review() {
        before.unresolved
    } else {
        0
    };
    if now.unresolved > was {
        changes.push(Change::Unresolved {
            from: was,
            to: now.unresolved,
        });
    }
    changes
}

/// What became of a watch's last fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// It waits for the lead, or the lead is at it.
    Waiting,
    Done,
    Failed,
    /// The person stopped the lead's turn.
    Stopped,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Last {
    /// When it fired, in seconds since the Unix epoch,
    pub at: u64,
    /// and the run it was: with the watch's number, what names a fire.
    pub due: u64,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subscription {
    /// The number the lead and the chat know it by. Never used twice.
    pub id: u64,
    pub title: String,
    pub trigger: Trigger,
    /// What the lead is told to do when it fires. Empty for a pull request
    /// that is only followed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instruction: String,
    #[serde(default)]
    pub paused: bool,
    /// False while the lead proposed it and the person has not said yes.
    pub allowed: bool,
    /// When a schedule next runs, in seconds since the Unix epoch.
    #[serde(default)]
    pub next: Option<u64>,
    #[serde(default)]
    pub last: Option<Last>,
    /// Times it woke the lead on `day`,
    #[serde(default)]
    pub fires_today: u32,
    /// in days since the Unix epoch.
    #[serde(default)]
    pub day: u64,
    /// The pull request as the lead was last told of it.
    #[serde(default)]
    pub seen: Option<Seen>,
    /// When its instruction was written.
    #[serde(default)]
    pub saved: u64,
}
impl Subscription {
    /// The minutes between its runs, where it runs on a schedule.
    pub fn minutes(&self) -> Option<u32> {
        match self.trigger {
            Trigger::Interval { minutes } => Some(minutes),
            Trigger::PullRequest { .. } => None,
        }
    }
    /// The pull request it follows.
    pub fn link(&self) -> Option<PullRequest> {
        match &self.trigger {
            Trigger::PullRequest { url, .. } => PullRequest::parse(url),
            Trigger::Interval { .. } => None,
        }
    }
    /// Neither paused nor waiting to be allowed.
    pub fn runs(&self) -> bool {
        self.allowed && !self.paused
    }
    /// "Every 30 min", "Pull request zevem/neptune#83".
    pub fn trigger_words(&self) -> String {
        match &self.trigger {
            Trigger::Interval { minutes } => {
                let every = schedule::every(*minutes);
                format!("E{}", &every[1..])
            }
            Trigger::PullRequest { url, .. } => match PullRequest::parse(url) {
                Some(link) => format!("Pull request {}", link.label()),
                None => "Pull request".into(),
            },
        }
    }
    /// Within what this build writes.
    pub fn is_valid(&self) -> bool {
        self.id > 0
            && titled(&self.title).is_ok_and(|title| title == self.title)
            && self.instruction.len() <= MAX_INSTRUCTION
            && match &self.trigger {
                Trigger::Interval { minutes } => {
                    (schedule::MIN_MINUTES..=schedule::MAX_MINUTES).contains(minutes)
                }
                Trigger::PullRequest { url, .. } => {
                    PullRequest::parse(url).is_some_and(|link| link.url() == url)
                }
            }
    }
    fn limit(&self) -> u32 {
        match self.trigger {
            Trigger::Interval { .. } => MAX_FIRES_A_DAY,
            Trigger::PullRequest { .. } => MAX_PR_WAKES_A_DAY,
        }
    }
    /// It woke the lead as often as it may on the day of `now`.
    pub fn spent(&self, now: u64) -> bool {
        self.day == now / DAY && self.fires_today >= self.limit()
    }
    /// It fires for the run that was due at `due`. `counted` is false for a
    /// run the person asked for, which is theirs and not the watch's.
    pub fn fired(&mut self, due: u64, now: u64, counted: bool) {
        if counted {
            if self.day != now / DAY {
                self.day = now / DAY;
                self.fires_today = 0;
            }
            self.fires_today = self.fires_today.saturating_add(1);
        }
        self.last = Some(Last {
            at: now,
            due,
            outcome: Outcome::Waiting,
        });
    }
    /// The run that was due at `due` fired already: a fire is named by its
    /// watch and its time, and happens once.
    pub fn fired_for(&self, due: u64) -> bool {
        self.last.is_some_and(|last| last.due == due)
    }
    /// What its row says under its name, at `now`.
    pub fn detail(&self, now: u64) -> String {
        if !self.allowed {
            return "Proposed by the lead · waits for you".into();
        }
        let mut parts = Vec::new();
        if self.paused {
            parts.push("Paused".to_owned());
        } else if let (Some(next), Some(_)) = (self.next, self.minutes()) {
            parts.push(if next <= now {
                "Due now".to_owned()
            } else {
                format!("Next {}", until(next - now))
            });
        } else if self.minutes().is_none() {
            parts.push(
                self.seen
                    .map_or_else(|| "Checking…".to_owned(), |seen| sentence(&seen.words())),
            );
        }
        if let Some(last) = self.last {
            let ago = ago(now.saturating_sub(last.at));
            parts.push(match last.outcome {
                Outcome::Waiting => "Running".to_owned(),
                Outcome::Done => format!("Last ran {ago}"),
                Outcome::Failed => format!("Last run failed {ago}"),
                Outcome::Stopped => format!("Last run stopped {ago}"),
            });
        }
        parts.join(" · ")
    }
}

/// `words` with a capital at their head.
pub fn sentence(words: &str) -> String {
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}
/// "in 14 min", "in 3 h", "in 2 d": how long until something `seconds`
/// away, a started minute counted whole.
fn until(seconds: u64) -> String {
    match seconds.div_ceil(60) {
        minutes @ 0..=59 => format!("in {} min", minutes.max(1)),
        minutes @ 60..=1439 => format!("in {} h", minutes / 60),
        minutes => format!("in {} d", minutes / 1440),
    }
}
/// "just now", "5 min ago", "3 h ago", "2 d ago".
fn ago(seconds: u64) -> String {
    match seconds {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86_399 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / DAY),
    }
}

/// A watch's name as it is kept: one line, without what only a terminal
/// would show.
fn titled(title: &str) -> Result<String, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("A watch needs a title.".into());
    }
    if title.chars().count() > MAX_TITLE || title.chars().any(char::is_control) {
        return Err(format!(
            "A watch's title is one line of at most {MAX_TITLE} characters."
        ));
    }
    Ok(title.to_owned())
}

/// A watch as it is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct New {
    pub title: String,
    pub trigger: Trigger,
    pub instruction: String,
    /// False for a schedule the lead proposes: the person allows it.
    pub allowed: bool,
}
/// Adds a watch to `list` and returns its number. `given` counts the
/// numbers handed out so far, so that none names two watches.
pub fn add(
    list: &mut Vec<Subscription>,
    given: &mut u64,
    new: New,
    now: SystemTime,
) -> Result<u64, String> {
    let title = titled(&new.title)?;
    let instruction = new.instruction.trim().to_owned();
    if instruction.len() > MAX_INSTRUCTION {
        return Err(format!(
            "What a watch should do is at most {} KB.",
            MAX_INSTRUCTION / 1024
        ));
    }
    let (trigger, next) = match new.trigger {
        Trigger::Interval { minutes } => {
            if minutes < schedule::MIN_MINUTES {
                return Err(format!(
                    "A watch runs every {} minutes at the most often.",
                    schedule::MIN_MINUTES
                ));
            }
            if minutes > schedule::MAX_MINUTES {
                return Err("A watch runs once a week at the least often.".into());
            }
            if instruction.is_empty() {
                return Err("Say what the lead should do when this watch fires.".into());
            }
            // A proposed one has no run until it is allowed.
            let next = new.allowed.then(|| schedule::first(minutes, now));
            (Trigger::Interval { minutes }, next)
        }
        Trigger::PullRequest { url, auto } => {
            let link = PullRequest::parse(&url).ok_or(
                "That is not the address of a pull request, such as \
                 https://github.com/owner/repository/pull/12.",
            )?;
            if let Some(known) = list
                .iter()
                .find(|known| known.link().is_some_and(|other| other.same(&link)))
            {
                return Err(format!(
                    "Watch {} already follows that pull request.",
                    known.id
                ));
            }
            let url = link.url().to_owned();
            (Trigger::PullRequest { url, auto }, None)
        }
    };
    if list.len() >= MAX {
        return Err(format!(
            "A project has at most {MAX} watches. Remove one first."
        ));
    }
    let id = list
        .iter()
        .map(|known| known.id)
        .max()
        .unwrap_or(0)
        .max(*given)
        .checked_add(1)
        .ok_or("No watch can be added.")?;
    *given = id;
    list.push(Subscription {
        id,
        title,
        trigger,
        instruction,
        paused: false,
        allowed: new.allowed,
        next,
        last: None,
        fires_today: 0,
        day: 0,
        seen: None,
        saved: schedule::seconds(now),
    });
    Ok(id)
}

/// Why a watch fires now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Its time came.
    Due(u64),
    /// Its time came while Neptune was closed, this many runs ago.
    Late { at: u64, runs: u64 },
    /// The person pressed "Run now".
    Asked(u64),
}
impl Why {
    /// The time that names this fire.
    pub fn due(self) -> u64 {
        match self {
            Self::Due(at) | Self::Asked(at) | Self::Late { at, .. } => at,
        }
    }
    /// "schedule · due 14:00 UTC", and what else is true of it.
    pub fn words(self) -> String {
        match self {
            Self::Due(at) => format!("schedule · due {}", schedule::clock(at)),
            Self::Late { at, runs: 1 } => format!(
                "schedule · due {} · missed 1 run while Neptune was closed",
                schedule::clock(at)
            ),
            Self::Late { at, runs } => format!(
                "schedule · due {} · missed {runs} runs while Neptune was closed",
                schedule::clock(at)
            ),
            Self::Asked(at) => format!("run by the user · {}", schedule::clock(at)),
        }
    }
}

/// One line for each watch, as the lead is told of them.
pub fn listing(list: &[Subscription], now: u64) -> String {
    if list.is_empty() {
        return "The project has no watches. Agent updates always reach you.".into();
    }
    list.iter()
        .map(|watch| {
            let mut line = format!(
                "Watch {} \"{}\": {}",
                watch.id,
                watch.title,
                watch.trigger_words().to_lowercase()
            );
            if let Trigger::PullRequest { url, .. } = &watch.trigger {
                line = format!("Watch {} \"{}\": {url}", watch.id, watch.title);
            }
            let state = if !watch.allowed {
                "proposed — the user must allow it".to_owned()
            } else {
                watch.detail(now).to_lowercase()
            };
            format!("{line}; {state}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }
    fn schedule(title: &str, minutes: u32, allowed: bool) -> New {
        New {
            title: title.into(),
            trigger: Trigger::Interval { minutes },
            instruction: "Check the nightly build".into(),
            allowed,
        }
    }
    fn follow(url: &str) -> New {
        New {
            title: "PR".into(),
            trigger: Trigger::PullRequest {
                url: url.into(),
                auto: false,
            },
            instruction: String::new(),
            allowed: true,
        }
    }
    fn seen(state: Review, checks: Checks, unresolved: u8) -> Seen {
        Seen {
            state,
            checks,
            unresolved,
        }
    }

    #[test]
    fn a_watch_is_added_within_its_bounds_and_numbered_once() {
        let (mut list, mut given) = (Vec::new(), 0);
        let now = at(1_000_000);
        // One the person adds runs from now on; one the lead proposes has
        // no run until it is allowed.
        assert_eq!(
            add(&mut list, &mut given, schedule(" Nightly ", 60, true), now),
            Ok(1)
        );
        assert_eq!(
            add(&mut list, &mut given, schedule("Hourly", 15, false), now),
            Ok(2)
        );
        assert_eq!(
            (list[0].title.as_str(), list[0].next, list[0].allowed),
            ("Nightly", Some(1_003_600), true)
        );
        assert_eq!((list[1].next, list[1].allowed), (None, false));
        assert_eq!(list[0].saved, 1_000_000);
        assert!(list.iter().all(Subscription::is_valid));
        for (new, why) in [
            (schedule("", 60, true), "needs a title"),
            (schedule("a\nb", 60, true), "one line"),
            (schedule(&"t".repeat(81), 60, true), "one line"),
            (schedule("Often", 14, true), "every 15 minutes"),
            (schedule("Rarely", 7 * 24 * 60 + 1, true), "once a week"),
            (
                New {
                    instruction: "  ".into(),
                    ..schedule("Empty", 60, true)
                },
                "what the lead should do",
            ),
            (
                New {
                    instruction: "i".repeat(MAX_INSTRUCTION + 1),
                    ..schedule("Long", 60, true)
                },
                "at most 2 KB",
            ),
            (follow("https://example.com/not/a/pr"), "not the address"),
            (follow("git@github.com:a/b.git"), "not the address"),
        ] {
            let refused = add(&mut list, &mut given, new, now).unwrap_err();
            assert!(refused.contains(why), "{refused}");
        }
        assert_eq!(list.len(), 2);
        // A pull request is followed once, under its plain address.
        assert_eq!(
            add(
                &mut list,
                &mut given,
                follow("https://GitHub.com/zevem/neptune/pull/83/files?w=1"),
                now
            ),
            Ok(3)
        );
        assert_eq!(
            list[2].trigger,
            Trigger::PullRequest {
                url: "https://github.com/zevem/neptune/pull/83".into(),
                auto: false
            }
        );
        assert_eq!(list[2].trigger_words(), "Pull request zevem/neptune#83");
        assert_eq!(list[0].trigger_words(), "Every 1 h");
        assert!(
            add(
                &mut list,
                &mut given,
                follow("https://github.com/Zevem/Neptune/pull/83"),
                now
            )
            .unwrap_err()
            .contains("Watch 3 already follows")
        );
        // A number is never given twice, also after its watch is gone.
        list.retain(|watch| watch.id != 3);
        assert_eq!(
            add(&mut list, &mut given, schedule("Again", 30, true), now),
            Ok(4)
        );
        // Sixteen and no more.
        for n in 0..MAX {
            let _ = add(
                &mut list,
                &mut given,
                schedule(&format!("w{n}"), 30, true),
                now,
            );
        }
        assert_eq!(list.len(), MAX);
        assert!(
            add(&mut list, &mut given, schedule("One more", 30, true), now)
                .unwrap_err()
                .contains("at most 16 watches")
        );
    }

    #[test]
    fn a_watch_wakes_the_lead_only_so_often_in_a_day_and_once_for_each_run() {
        let (mut list, mut given) = (Vec::new(), 0);
        add(&mut list, &mut given, schedule("Nightly", 15, true), at(0)).unwrap();
        add(
            &mut list,
            &mut given,
            follow("https://github.com/a/b/pull/1"),
            at(0),
        )
        .unwrap();
        let day = 400 * DAY;
        for (watch, limit) in list.iter_mut().zip([12, 10]) {
            for run in 0..limit {
                assert!(!watch.spent(day + run), "{run}");
                assert!(!watch.fired_for(day + run));
                watch.fired(day + run, day + run, true);
                // The same run does not fire again, whoever asks.
                assert!(watch.fired_for(day + run));
            }
            assert!(watch.spent(day + 500));
            assert_eq!(watch.fires_today, limit as u32);
            // A run the person asks for is theirs: it is not counted.
            watch.fired(day + 600, day + 600, false);
            assert_eq!(watch.fires_today, limit as u32);
            // The next day it may again.
            assert!(!watch.spent(day + DAY));
            watch.fired(day + DAY, day + DAY, true);
            assert_eq!((watch.fires_today, watch.day), (1, 401));
            assert_eq!(watch.last.map(|last| last.outcome), Some(Outcome::Waiting));
        }
    }

    #[test]
    fn a_pull_request_speaks_when_its_checks_its_state_or_its_open_comments_change() {
        use Change::*;
        use Checks::{Failing, None as NoChecks, Passing, Pending};
        use Review::{Closed as WasClosed, Draft, Merged as WasMerged, Open};
        let open = |checks, unresolved| seen(Open, checks, unresolved);
        for (before, now, expected) in [
            // Nothing changed, or only what needs no one.
            (open(Passing, 0), open(Passing, 0), vec![]),
            (open(Passing, 0), open(Pending, 0), vec![]),
            (open(Failing, 2), open(Pending, 1), vec![]),
            (seen(Draft, Pending, 0), open(Pending, 0), vec![]),
            (open(NoChecks, 3), open(NoChecks, 3), vec![]),
            // Checks that fail or pass.
            (open(Pending, 0), open(Failing, 0), vec![ChecksFailing]),
            (open(Passing, 0), open(Failing, 0), vec![ChecksFailing]),
            (open(Pending, 0), open(Passing, 0), vec![ChecksPassing]),
            (open(Failing, 0), open(Passing, 0), vec![ChecksPassing]),
            (open(NoChecks, 0), open(Passing, 0), vec![ChecksPassing]),
            // More comments to answer; fewer is no news.
            (
                open(Passing, 1),
                open(Passing, 3),
                vec![Unresolved { from: 1, to: 3 }],
            ),
            (open(Passing, 3), open(Passing, 1), vec![]),
            (
                open(Pending, 0),
                open(Failing, 2),
                vec![ChecksFailing, Unresolved { from: 0, to: 2 }],
            ),
            // Done with: that alone is said, whatever else differs.
            (open(Failing, 2), seen(WasMerged, Passing, 5), vec![Merged]),
            (open(Passing, 0), seen(WasClosed, Failing, 9), vec![Closed]),
            (
                seen(WasMerged, Passing, 0),
                seen(WasMerged, Failing, 4),
                vec![],
            ),
            // Opened again: it is told as it stands.
            (
                seen(WasClosed, NoChecks, 0),
                open(Failing, 1),
                vec![ChecksFailing, Unresolved { from: 0, to: 1 }],
            ),
        ] {
            assert_eq!(changes(before, now), expected, "{before:?} -> {now:?}");
        }
        assert!(Merged.ends() && Closed.ends() && !ChecksFailing.ends());
        assert_eq!(
            open(Failing, 2).words(),
            "open · checks failing · 2 unresolved review comments"
        );
        assert_eq!(seen(WasMerged, Failing, 2).words(), "merged");
        assert_eq!(
            Unresolved { from: 1, to: 3 }.words(),
            "unresolved review comments went from 1 to 3"
        );
    }

    #[test]
    fn a_watch_is_kept_as_its_project_file_holds_it() {
        let (mut list, mut given) = (Vec::new(), 0);
        add(
            &mut list,
            &mut given,
            schedule("Nightly", 60, true),
            at(1_790_000_000),
        )
        .unwrap();
        list[0].fired(1_790_003_600, 1_790_003_601, true);
        list[0].seen = None;
        let json = serde_json::to_value(&list[0]).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "id": 1, "title": "Nightly",
                "trigger": {"kind": "interval", "minutes": 60},
                "instruction": "Check the nightly build",
                "paused": false, "allowed": true,
                "next": 1_790_003_600u64,
                "last": {"at": 1_790_003_601u64, "due": 1_790_003_600u64, "outcome": "waiting"},
                "fires_today": 1, "day": 1_790_003_601u64 / DAY,
                "seen": null, "saved": 1_790_000_000u64,
            })
        );
        assert_eq!(
            serde_json::from_value::<Subscription>(json.clone()).unwrap(),
            list[0]
        );
        // What the baseline wrote, with nothing this build added, reads.
        let plain: Subscription = serde_json::from_value(serde_json::json!({
            "id": 3, "title": "t", "trigger": {"kind": "pull_request",
            "url": "https://github.com/a/b/pull/1"}, "instruction": "", "paused": false,
            "allowed": true, "next": null, "last": null, "fires_today": 2,
            "seen": {"state": "open", "checks": "failing", "unresolved": 2},
        }))
        .unwrap();
        assert!(plain.is_valid());
        assert_eq!(plain.seen, Some(seen(Review::Open, Checks::Failing, 2)));
        // A field this build does not know is not dropped silently, and a
        // watch outside its bounds is damage.
        let mut unknown = json.clone();
        unknown["command"] = "rm".into();
        assert!(serde_json::from_value::<Subscription>(unknown).is_err());
        for (field, value) in [
            ("id", serde_json::json!(0)),
            ("title", serde_json::json!(" padded ")),
            (
                "trigger",
                serde_json::json!({"kind": "interval", "minutes": 5}),
            ),
            (
                "trigger",
                serde_json::json!({"kind": "pull_request", "url": "https://x.y/a/b/pull/1/files"}),
            ),
            (
                "instruction",
                serde_json::json!("i".repeat(MAX_INSTRUCTION + 1)),
            ),
        ] {
            let mut bad = json.clone();
            bad[field] = value;
            let read: Subscription = serde_json::from_value(bad).unwrap();
            assert!(!read.is_valid(), "{field}");
        }
    }

    #[test]
    fn a_watch_says_when_it_runs_next_and_what_became_of_its_last_run() {
        let (mut list, mut given) = (Vec::new(), 0);
        let hour = 3600;
        add(
            &mut list,
            &mut given,
            schedule("Nightly", 60, false),
            at(9 * hour),
        )
        .unwrap();
        add(
            &mut list,
            &mut given,
            follow("https://github.com/a/b/pull/7"),
            at(9 * hour),
        )
        .unwrap();
        let now = 9 * hour + 600;
        assert_eq!(list[0].detail(now), "Proposed by the lead · waits for you");
        assert_eq!(list[1].detail(now), "Checking…");
        assert_eq!(
            listing(&list, now),
            "Watch 1 \"Nightly\": every 1 h; proposed — the user must allow it\n\
             Watch 2 \"PR\": https://github.com/a/b/pull/7; checking…"
        );
        list[0].allowed = true;
        list[0].next = Some(10 * hour);
        // How long until its next run, as its last is said: a started
        // minute is counted whole, then hours, then days.
        assert_eq!(list[0].detail(now), "Next in 50 min");
        assert_eq!(list[0].detail(10 * hour - 61), "Next in 2 min");
        assert_eq!(list[0].detail(10 * hour - 1), "Next in 1 min");
        assert_eq!(list[0].detail(7 * hour), "Next in 3 h");
        assert_eq!(list[0].detail(8 * hour + 60), "Next in 1 h");
        list[0].next = Some(10 * hour + 3 * DAY);
        assert_eq!(list[0].detail(10 * hour), "Next in 3 d");
        list[0].next = Some(10 * hour);
        list[0].fired(10 * hour, 10 * hour, true);
        assert_eq!(list[0].detail(10 * hour + 5), "Due now · Running");
        list[0].next = Some(11 * hour);
        for (outcome, said) in [
            (Outcome::Done, "Next in 55 min · Last ran 5 min ago"),
            (
                Outcome::Failed,
                "Next in 55 min · Last run failed 5 min ago",
            ),
            (
                Outcome::Stopped,
                "Next in 55 min · Last run stopped 5 min ago",
            ),
        ] {
            list[0].last.as_mut().unwrap().outcome = outcome;
            assert_eq!(list[0].detail(10 * hour + 300), said);
        }
        list[0].paused = true;
        assert_eq!(
            list[0].detail(10 * hour + 300),
            "Paused · Last run stopped 5 min ago"
        );
        list[1].seen = Some(seen(Review::Open, Checks::Failing, 1));
        assert_eq!(
            list[1].detail(now),
            "Open · checks failing · 1 unresolved review comment"
        );
        assert_eq!(Why::Due(14 * hour).words(), "schedule · due 14:00 UTC");
        assert_eq!(
            Why::Late {
                at: 14 * hour,
                runs: 3
            }
            .words(),
            "schedule · due 14:00 UTC · missed 3 runs while Neptune was closed"
        );
        assert!(
            Why::Late { at: 0, runs: 1 }
                .words()
                .ends_with("missed 1 run while Neptune was closed")
        );
        assert_eq!(
            Why::Asked(14 * hour + 60).words(),
            "run by the user · 14:01 UTC"
        );
        assert_eq!(
            listing(&[], 0),
            "The project has no watches. Agent updates always reach you."
        );
    }
}
