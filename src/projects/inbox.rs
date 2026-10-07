//! What waits for a project's lead. The user's messages go first; what its
//! agents did is gathered per agent and sent as one turn once it has been
//! quiet for a moment, with what its watches have to say. Nothing is written into a running turn: the owner
//! takes a turn from here only while the lead is idle.
use super::transcript::{Attachment, MAX_ATTACHMENTS, Origin};
use crate::agent_activity::Attention;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// Messages and agents with news that can wait at once.
pub const MAX_ITEMS: usize = 32;
/// Replies of one agent that wait; older ones are counted, not kept.
pub const MAX_REPLIES: usize = 16;
/// Events are sent once nothing new has come for this long,
pub const QUIET: Duration = Duration::from_secs(2);
/// and no later than this after the first of them.
pub const MAX_WAIT: Duration = Duration::from_secs(10);
/// The user's messages that go in one turn, by their length together. A
/// longer queue takes more turns, and so does one with more files than a
/// message may have.
pub const MAX_USER_BYTES: usize = 128 * 1024;
/// Turns Neptune may begin by itself in any hour,
pub const MAX_AUTO_PER_HOUR: usize = 30;
/// and in a row without a word from the user.
pub const MAX_AUTO_STREAK: u32 = 20;
const HOUR: Duration = Duration::from_secs(3600);

/// What the person sent the lead: words, files, or both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    pub attachments: Vec<Attachment>,
}
impl From<String> for Message {
    fn from(text: String) -> Self {
        Self {
            text,
            attachments: Vec::new(),
        }
    }
}
impl From<&str> for Message {
    fn from(text: &str) -> Self {
        text.to_owned().into()
    }
}

/// What became of an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum What {
    /// Its turn ended.
    Finished,
    /// It waits for a person in its terminal.
    NeedsPerson(Attention),
    /// Its CLI asks something before taking its task.
    Asking,
    Ended(String),
    /// It said something while still working.
    Progress,
    /// Its CLI has not taken its task in good time.
    Stalled,
    /// A message for it was never typed.
    Undelivered,
    /// Something the chat holds from before Neptune was closed, in the
    /// chat's words, which the lead was never told.
    Past(String),
}
impl What {
    /// A state, which a later remark does not replace.
    fn state(&self) -> bool {
        !matches!(self, Self::Progress | Self::Undelivered)
    }
    /// As the chat and the lead are told.
    pub fn words(&self) -> String {
        match self {
            Self::Finished => "finished its turn".into(),
            Self::NeedsPerson(Attention::Permission) => "needs a permission answer".into(),
            Self::NeedsPerson(Attention::Question) => "asked a question".into(),
            Self::NeedsPerson(Attention::Plan) => "has a plan to approve".into(),
            Self::NeedsPerson(Attention::Input) => "waits for input".into(),
            Self::Asking => "asks something before starting".into(),
            Self::Ended(reason) if reason.is_empty() => "ended".into(),
            Self::Ended(reason) => format!("ended: {reason}"),
            Self::Progress => "sent a message".into(),
            Self::Stalled => "has not started yet".into(),
            Self::Undelivered => "did not receive the lead's message".into(),
            Self::Past(words) => format!("{words} {}", super::transcript::RESTARTED),
        }
    }
    /// A person has to act in the agent's terminal.
    pub fn needs_person(&self) -> bool {
        matches!(self, Self::NeedsPerson(_) | Self::Asking)
    }
}

/// The news of one agent since the lead was last told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEvent {
    pub agent: u64,
    pub title: Option<String>,
    pub what: What,
    /// What it said, oldest first.
    pub replies: Vec<String>,
    /// Earlier replies that were not kept.
    pub omitted: usize,
    /// What its terminal shows while it asks before starting. For the lead
    /// only: never an entry of the chat.
    pub screen: Option<String>,
    /// A message for it was never typed.
    pub undelivered: bool,
}
impl AgentEvent {
    pub fn new(agent: u64, title: Option<String>, what: What) -> Self {
        Self {
            agent,
            title,
            undelivered: what == What::Undelivered,
            what,
            replies: Vec::new(),
            omitted: 0,
            screen: None,
        }
    }
    fn merge(&mut self, newer: AgentEvent) {
        if newer.what.state() || !self.what.state() {
            self.what = newer.what;
            self.screen = newer.screen;
        }
        if newer.title.is_some() {
            self.title = newer.title;
        }
        self.undelivered |= newer.undelivered;
        self.omitted += newer.omitted;
        self.replies.extend(newer.replies);
        if self.replies.len() > MAX_REPLIES {
            let over = self.replies.len() - MAX_REPLIES;
            self.replies.drain(..over);
            self.omitted += over;
        }
    }
}

/// Something a watch of the project has to say: a schedule that came due, a
/// pull request that changed, a worktree that was merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fire {
    /// The watch it comes from. One fire of a watch waits at a time: a
    /// later one is not stacked on it.
    pub watch: Option<u64>,
    /// As the lead is told it, in whole lines.
    pub text: String,
}

/// Why Neptune stops beginning turns by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    Hourly,
    Streak,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    Nothing,
    /// Events wait for a quiet moment that comes then.
    Wait(Instant),
    /// A turn can be taken once the lead is idle.
    Ready(Origin),
    /// An automatic turn is due and one too many: the project pauses.
    Guard(Guard),
}
/// One turn's worth of what waited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub origin: Origin,
    pub users: Vec<Message>,
    pub events: Vec<AgentEvent>,
    pub fires: Vec<Fire>,
}
/// A turn handed to the lead that it has not begun.
#[derive(Debug)]
struct Sent {
    id: String,
    origin: Origin,
    users: usize,
    events: Vec<AgentEvent>,
    fires: Vec<Fire>,
    window: Option<(Instant, Instant)>,
}

#[derive(Debug, Default)]
pub struct Inbox {
    users: VecDeque<Message>,
    events: Vec<AgentEvent>,
    fires: Vec<Fire>,
    /// When the first and the newest of the waiting events came.
    window: Option<(Instant, Instant)>,
    sent: Option<Sent>,
    /// When Neptune began turns by itself, within the last hour.
    auto: VecDeque<Instant>,
    streak: u32,
}
impl Inbox {
    /// The user's messages that have not reached the lead.
    pub fn queued(&self) -> usize {
        self.users.len()
    }
    pub fn has_events(&self) -> bool {
        !self.events.is_empty() || !self.fires.is_empty()
    }
    fn items(&self) -> usize {
        self.users.len() + self.events.len() + self.fires.len()
    }
    /// A fire of `watch` waits here or was handed to the lead and not begun.
    pub fn awaits(&self, watch: u64) -> bool {
        self.fires
            .iter()
            .chain(self.sent.iter().flat_map(|sent| &sent.fires))
            .any(|fire| fire.watch == Some(watch))
    }
    /// Adds what a watch has to say. False, and nothing added, where an
    /// earlier fire of the same watch still waits or too much waits already:
    /// a watch that fires again says it again.
    pub fn push_fire(&mut self, fire: Fire, now: Instant) -> bool {
        if fire.watch.is_some_and(|watch| self.awaits(watch)) || self.items() >= MAX_ITEMS {
            return false;
        }
        self.window = Some((self.window.map_or(now, |(first, _)| first), now));
        self.fires.push(fire);
        true
    }
    /// A turn was handed to the lead and has not begun.
    pub fn sending(&self) -> bool {
        self.sent.is_some()
    }
    /// False when too much waits already; nothing is dropped for it.
    pub fn push_user(&mut self, message: Message) -> bool {
        if self.items() >= MAX_ITEMS {
            return false;
        }
        self.users.push_back(message);
        true
    }
    /// Adds an agent's news to what already waits for that agent.
    pub fn push_event(&mut self, event: AgentEvent, now: Instant) {
        self.window = Some((self.window.map_or(now, |(first, _)| first), now));
        if let Some(waiting) = self
            .events
            .iter_mut()
            .find(|waiting| waiting.agent == event.agent)
        {
            return waiting.merge(event);
        }
        if self.items() >= MAX_ITEMS && !self.events.is_empty() {
            // The oldest news gives way; the lead's next turn names every
            // agent's state whatever was dropped.
            self.events.remove(0);
        }
        let mut first = AgentEvent::new(event.agent, None, What::Progress);
        first.merge(event);
        self.events.push(first);
    }
    /// What the lead is owed now. `limited` holds everything, `paused` what
    /// Neptune would send by itself.
    pub fn next(&mut self, now: Instant, paused: bool, limited: bool) -> Next {
        if self.sent.is_some() || limited {
            return Next::Nothing;
        }
        if !self.users.is_empty() {
            return Next::Ready(Origin::User);
        }
        let Some((first, newest)) = self.window.filter(|_| !paused && self.has_events()) else {
            return Next::Nothing;
        };
        let due = (newest + QUIET).min(first + MAX_WAIT);
        if now < due {
            return Next::Wait(due);
        }
        while self
            .auto
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= HOUR)
        {
            self.auto.pop_front();
        }
        if self.auto.len() >= MAX_AUTO_PER_HOUR {
            Next::Guard(Guard::Hourly)
        } else if self.streak >= MAX_AUTO_STREAK {
            Next::Guard(Guard::Streak)
        } else {
            Next::Ready(Origin::Events)
        }
    }
    /// Hands over one turn under `id`. The user's messages stay here until
    /// the lead begins the turn; a paused project's events stay as well.
    pub fn take(&mut self, id: &str, paused: bool) -> Turn {
        let origin = if self.users.is_empty() {
            Origin::Events
        } else {
            Origin::User
        };
        let (mut bytes, mut files) = (0, 0);
        let users: Vec<Message> = self
            .users
            .iter()
            .enumerate()
            .take_while(|(index, message)| {
                bytes += message.text.len();
                files += message.attachments.len();
                *index == 0 || (bytes <= MAX_USER_BYTES && files <= MAX_ATTACHMENTS)
            })
            .map(|(_, message)| message.clone())
            .collect();
        let (events, fires, window) = if paused && origin == Origin::User {
            (Vec::new(), Vec::new(), None)
        } else {
            (
                std::mem::take(&mut self.events),
                std::mem::take(&mut self.fires),
                self.window.take(),
            )
        };
        self.sent = Some(Sent {
            id: id.to_owned(),
            origin,
            users: users.len(),
            events: events.clone(),
            fires: fires.clone(),
            window,
        });
        Turn {
            origin,
            users,
            events,
            fires,
        }
    }
    /// The lead began turn `id`: what it carried no longer waits.
    pub fn started(&mut self, id: &str, now: Instant) -> Option<Turn> {
        let sent = self.sent.take_if(|sent| sent.id == id)?;
        let users = self
            .users
            .drain(..sent.users.min(self.users.len()))
            .collect();
        match sent.origin {
            Origin::User => self.streak = 0,
            Origin::Events => {
                self.streak += 1;
                if self.auto.len() == MAX_AUTO_PER_HOUR * 2 {
                    self.auto.pop_front();
                }
                self.auto.push_back(now);
            }
        }
        Some(Turn {
            origin: sent.origin,
            users,
            events: sent.events,
            fires: sent.fires,
        })
    }
    /// The lead never began the turn it was handed: everything waits again.
    pub fn abandoned(&mut self) {
        let Some(sent) = self.sent.take() else {
            return;
        };
        let newer = std::mem::replace(&mut self.events, sent.events);
        for event in newer {
            match self
                .events
                .iter_mut()
                .find(|waiting| waiting.agent == event.agent)
            {
                Some(waiting) => waiting.merge(event),
                None => self.events.push(event),
            }
        }
        // What was handed over is older than what came since.
        let newer = std::mem::replace(&mut self.fires, sent.fires);
        self.fires.extend(newer);
        self.window = match (sent.window, self.window) {
            (Some((first, newest)), Some((_, later))) => Some((first, newest.max(later))),
            (window, later) => window.or(later),
        };
    }
    /// The user spoke or resumed: Neptune may begin turns by itself again.
    pub fn reset_guard(&mut self) {
        self.streak = 0;
        self.auto.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A message of words alone.
    impl PartialEq<&str> for Message {
        fn eq(&self, text: &&str) -> bool {
            self.text == *text && self.attachments.is_empty()
        }
    }

    fn event(agent: u64, what: What, replies: &[&str]) -> AgentEvent {
        AgentEvent {
            replies: replies.iter().map(|reply| (*reply).to_owned()).collect(),
            ..AgentEvent::new(agent, Some(format!("task {agent}")), what)
        }
    }
    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn the_users_messages_go_first_and_leave_only_once_the_lead_begins_the_turn() {
        let start = Instant::now();
        let mut inbox = Inbox::default();
        assert_eq!(inbox.next(start, false, false), Next::Nothing);
        inbox.push_event(event(7, What::Finished, &["done"]), start);
        assert!(inbox.push_user("first".into()));
        assert!(inbox.push_user("second".into()));
        // A message does not wait for the quiet moment events wait for.
        assert_eq!(inbox.next(start, false, false), Next::Ready(Origin::User));
        let turn = inbox.take("t1", false);
        assert_eq!(turn.origin, Origin::User);
        assert_eq!(turn.users, ["first", "second"]);
        assert_eq!(turn.events.len(), 1, "what waited rides along");
        // Handed over is not begun: the messages still count as waiting, and
        // nothing else is offered meanwhile.
        assert!(inbox.sending() && inbox.queued() == 2);
        assert!(inbox.push_user("third".into()));
        assert_eq!(inbox.next(start + secs(60), false, false), Next::Nothing);
        // Another turn's start is not this one's.
        assert!(inbox.started("other", start).is_none());
        let begun = inbox.started("t1", start).unwrap();
        assert_eq!(begun.users, ["first", "second"]);
        assert_eq!((inbox.queued(), inbox.sending()), (1, false));
        assert!(!inbox.has_events());
        assert_eq!(inbox.next(start, false, false), Next::Ready(Origin::User));

        // A lead that ended before beginning the turn never had it.
        inbox.push_event(event(7, What::Finished, &["again"]), start);
        let turn = inbox.take("t2", false);
        assert_eq!((turn.users.len(), turn.events.len()), (1, 1));
        inbox.push_event(event(7, What::Progress, &["more"]), start + secs(1));
        inbox.push_event(event(8, What::Finished, &[]), start + secs(1));
        inbox.abandoned();
        assert_eq!(inbox.queued(), 1);
        let turn = inbox.take("t3", false);
        assert_eq!(turn.events.len(), 2);
        assert_eq!(turn.events[0].replies, ["again", "more"]);
        assert_eq!(turn.events[0].what, What::Finished);
    }

    #[test]
    fn events_gather_per_agent_and_go_as_one_turn_after_a_quiet_moment() {
        let start = Instant::now();
        let mut inbox = Inbox::default();
        inbox.push_event(event(1, What::Progress, &["a"]), start);
        assert_eq!(inbox.next(start, false, false), Next::Wait(start + QUIET));
        // News keeps the window open, up to its limit.
        inbox.push_event(event(2, What::Finished, &["b"]), start + secs(1));
        inbox.push_event(event(1, What::Finished, &["c"]), start + secs(1));
        assert_eq!(
            inbox.next(start + secs(2), false, false),
            Next::Wait(start + secs(3))
        );
        for n in 2..=9 {
            inbox.push_event(event(1, What::Progress, &["tick"]), start + secs(n));
        }
        assert_eq!(
            inbox.next(start + secs(9), false, false),
            Next::Wait(start + MAX_WAIT)
        );
        // Paused or limited, nothing is sent however long it waits.
        assert_eq!(inbox.next(start + secs(99), true, false), Next::Nothing);
        assert_eq!(inbox.next(start + secs(99), false, true), Next::Nothing);
        assert_eq!(
            inbox.next(start + MAX_WAIT, false, false),
            Next::Ready(Origin::Events)
        );
        let turn = inbox.take("t1", false);
        assert_eq!(turn.origin, Origin::Events);
        assert!(turn.users.is_empty());
        // One entry per agent: the newest state, a remark not replacing it.
        assert_eq!(turn.events.len(), 2);
        assert_eq!(
            (turn.events[0].agent, &turn.events[0].what),
            (1, &What::Finished)
        );
        assert_eq!(turn.events[0].replies.len(), 10);
        assert_eq!(turn.events[0].title.as_deref(), Some("task 1"));
        assert!(inbox.started("t1", start + MAX_WAIT).is_some());
        assert_eq!(inbox.next(start + secs(99), false, false), Next::Nothing);

        // Only the newest replies of a talkative agent are kept, and counted.
        for n in 0..MAX_REPLIES + 5 {
            inbox.push_event(event(3, What::Progress, &[&n.to_string()]), start);
        }
        inbox.push_event(event(3, What::Undelivered, &[]), start);
        let turn = inbox.take("t2", false);
        assert_eq!(turn.events[0].replies.len(), MAX_REPLIES);
        assert_eq!(turn.events[0].replies[0], "5");
        assert_eq!(turn.events[0].omitted, 5);
        assert!(turn.events[0].undelivered);

        // A paused project's events stay behind when the user speaks.
        inbox.abandoned();
        inbox.push_user("hello".into());
        let turn = inbox.take("t3", true);
        assert!(turn.events.is_empty() && inbox.has_events());
        inbox.started("t3", start);
        assert_eq!(inbox.next(start + secs(99), true, false), Next::Nothing);
    }

    #[test]
    fn no_more_waits_than_the_limit() {
        let start = Instant::now();
        let mut inbox = Inbox::default();
        for n in 0..MAX_ITEMS {
            assert!(inbox.push_user(n.to_string().into()));
        }
        assert!(!inbox.push_user("one too many".into()));
        assert_eq!(inbox.queued(), MAX_ITEMS);
        let mut inbox = Inbox::default();
        for agent in 0..MAX_ITEMS as u64 + 4 {
            inbox.push_event(event(agent, What::Finished, &[]), start);
        }
        assert!(!inbox.push_user("full".into()));
        let turn = inbox.take("t", false);
        assert_eq!(turn.events.len(), MAX_ITEMS);
        assert_eq!(turn.events[0].agent, 4, "the oldest news gave way");
    }

    #[test]
    fn a_long_queue_of_messages_takes_more_than_one_turn() {
        let start = Instant::now();
        let mut inbox = Inbox::default();
        for _ in 0..3 {
            inbox.push_user("x".repeat(MAX_USER_BYTES / 2 + 1).into());
        }
        assert_eq!(inbox.take("t1", false).users.len(), 1);
        inbox.started("t1", start);
        assert_eq!(inbox.queued(), 2);
        // One message is a turn whatever its length.
        let mut inbox = Inbox::default();
        inbox.push_user("x".repeat(MAX_USER_BYTES * 2).into());
        inbox.push_user("next".into());
        assert_eq!(inbox.take("t1", false).users.len(), 1);
        inbox.started("t1", start);
        assert_eq!(inbox.take("t2", false).users, ["next"]);
    }

    #[test]
    fn a_messages_files_wait_with_it_and_a_turn_takes_no_more_than_a_message_may_have() {
        let start = Instant::now();
        let file = |n: usize| Attachment {
            path: format!("/tmp/{n}.png"),
            bytes: 10,
        };
        let message = |text: &str, files: std::ops::Range<usize>| Message {
            text: text.into(),
            attachments: files.map(file).collect(),
        };
        let mut inbox = Inbox::default();
        // Files alone are a message.
        assert!(inbox.push_user(message("", 0..2)));
        assert!(inbox.push_user(message("and these", 2..MAX_ATTACHMENTS)));
        assert!(inbox.push_user(message("one more", 10..11)));
        assert_eq!(inbox.queued(), 3);
        let turn = inbox.take("t1", false);
        assert_eq!(
            turn.users,
            [message("", 0..2), message("and these", 2..MAX_ATTACHMENTS)]
        );
        // A lead that never began the turn leaves them waiting, whole.
        inbox.abandoned();
        assert_eq!(inbox.take("t2", false).users, turn.users);
        assert_eq!(inbox.started("t2", start).unwrap().users, turn.users);
        assert_eq!(inbox.take("t3", false).users, [message("one more", 10..11)]);
    }

    #[test]
    fn neptune_stops_beginning_turns_by_itself_past_its_limits() {
        let start = Instant::now();
        let mut inbox = Inbox::default();
        let mut now = start;
        let automatic = |inbox: &mut Inbox, now: Instant| {
            inbox.push_event(event(1, What::Finished, &[]), now);
            let next = inbox.next(now + QUIET, false, false);
            if next == Next::Ready(Origin::Events) {
                inbox.take("t", false);
                inbox.started("t", now + QUIET);
            }
            next
        };
        // Twenty in a row without a word from the user.
        for _ in 0..MAX_AUTO_STREAK {
            assert_eq!(automatic(&mut inbox, now), Next::Ready(Origin::Events));
            now += secs(600);
        }
        assert_eq!(automatic(&mut inbox, now), Next::Guard(Guard::Streak));
        // The user's own turn is always taken, and starts the count again.
        inbox.push_user("go on".into());
        assert_eq!(inbox.next(now, false, false), Next::Ready(Origin::User));
        inbox.take("u", false);
        inbox.started("u", now);
        // Thirty within an hour, however often the user speaks between.
        let mut inbox = Inbox::default();
        let mut now = start;
        for n in 0..MAX_AUTO_PER_HOUR {
            assert_eq!(
                automatic(&mut inbox, now),
                Next::Ready(Origin::Events),
                "{n}"
            );
            inbox.push_user("and".into());
            inbox.take("u", false);
            inbox.started("u", now);
            now += secs(60);
        }
        assert_eq!(automatic(&mut inbox, now), Next::Guard(Guard::Hourly));
        // The hour passing lets the oldest go.
        assert_eq!(
            inbox.next(start + HOUR + QUIET + secs(1), false, false),
            Next::Ready(Origin::Events)
        );
        // Resuming is the user's word that it may go on.
        let mut inbox = Inbox::default();
        for _ in 0..MAX_AUTO_STREAK {
            automatic(&mut inbox, start);
        }
        assert_eq!(automatic(&mut inbox, start), Next::Guard(Guard::Streak));
        inbox.reset_guard();
        assert_eq!(
            inbox.next(start + QUIET, false, false),
            Next::Ready(Origin::Events)
        );
    }

    #[test]
    fn every_state_is_said_in_words() {
        let all = [
            What::Finished,
            What::NeedsPerson(Attention::Permission),
            What::NeedsPerson(Attention::Question),
            What::NeedsPerson(Attention::Plan),
            What::NeedsPerson(Attention::Input),
            What::Asking,
            What::Ended(String::new()),
            What::Ended("its terminal was closed".into()),
            What::Progress,
            What::Stalled,
            What::Undelivered,
        ];
        let words: std::collections::BTreeSet<String> = all.iter().map(What::words).collect();
        assert_eq!(words.len(), all.len());
        assert_eq!(all.iter().filter(|what| what.needs_person()).count(), 5);
    }
}
