//! A project's chat as its entries, shaped as the saved chat holds them: one
//! entry a line. Memory holds the newest; the store reads older ones when
//! they are asked for. Text that is still streaming is never an entry.
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Entries held in memory; older ones leave first.
pub const MAX_ENTRIES: usize = 400;
/// Older entries read back for "Load earlier", on top of the newest.
pub const MAX_EARLIER: usize = 1200;
/// The text of one entry.
pub const MAX_TEXT: usize = 64 * 1024;
/// One entry as a line of the saved chat.
pub const MAX_LINE: usize = 64 * 1024;
/// The first line of a saved chat this build reads and adds to.
pub const HEADER: &str = r#"{"neptune_chat":1}"#;
/// What is added to an event that waited through a restart.
pub const RESTARTED: &str = "(from before Neptune restarted)";
/// Files that go with one message.
pub const MAX_ATTACHMENTS: usize = 10;
/// A picture the lead is shown. Its provider takes 5 MB of it as text, which
/// is this much of the file.
pub const MAX_PICTURE: u64 = 3_932_160;
/// The path of an attached file, so that a message's line keeps its limit.
pub const MAX_PATH: usize = 1024;

/// A file the person gave the lead with a message. It stays where it is:
/// the chat keeps where that is and how large it was, never what it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub path: String,
    #[serde(default)]
    pub bytes: u64,
}
impl Attachment {
    /// Its file name, as a chip shows it.
    pub fn name(&self) -> &str {
        std::path::Path::new(&self.path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&self.path)
    }
    /// Its name is that of a picture a lead can be shown.
    pub fn picture(&self) -> bool {
        let extension = std::path::Path::new(&self.path)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp")
    }
    /// How large it was, in the unit a person would say it in.
    pub fn size(&self) -> String {
        match self.bytes {
            bytes if bytes < 1024 => format!("{bytes} B"),
            bytes if bytes < 1024 * 1024 => format!("{} KB", bytes.div_ceil(1024)),
            bytes => format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)),
        }
    }
}

/// Who began a turn of the lead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    User,
    /// Neptune, with what happened since the lead last spoke.
    Events,
}
/// Where an event came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Agent,
    Subscription,
    Pr,
    Worktree,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ended {
    Completed,
    Interrupted,
    Failed,
    Limit,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entry {
    User {
        text: String,
        /// The files that went with it.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<Attachment>,
    },
    Turn {
        id: String,
        origin: Origin,
    },
    /// A finished block of the lead's text.
    Lead {
        text: String,
    },
    /// Something Neptune did because the lead asked, or refused to do.
    Tool {
        name: String,
        summary: String,
        ok: bool,
    },
    /// Something that happened, as it is given to the lead.
    Event {
        source: Source,
        /// The agent it is about.
        #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
        agent: Option<u64>,
        what: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        text: String,
    },
    End {
        turn: String,
        outcome: Ended,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost: Option<f64>,
    },
    /// A watch the lead proposed, which the person allows or declines
    /// beside it. It is the person's to answer, not news for the lead.
    Proposal {
        /// The watch's number.
        watch: u64,
        what: String,
        /// What it would tell the lead to do.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        text: String,
    },
    /// Every event up to this entry reached the lead.
    Delivered {
        through: u64,
    },
    Notice {
        text: String,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub seq: u64,
    /// Seconds since the Unix epoch.
    pub at: u64,
    #[serde(flatten)]
    pub entry: Entry,
}

/// Cuts `text` to at most `max` bytes, on a character.
pub fn clip(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `record` as a line of the saved chat, without its line end. An entry
/// whose line would be too long is saved with less of its text.
pub fn line(record: &Record) -> String {
    let mut line = serde_json::to_string(record).unwrap_or_default();
    if line.len() <= MAX_LINE {
        return line;
    }
    let mut shorter = record.clone();
    // Escaping makes a line longer than its text, by an amount that
    // depends on the text.
    for _ in 0..8 {
        let (Entry::User { text, .. }
        | Entry::Lead { text }
        | Entry::Notice { text }
        | Entry::Event { text, .. }
        | Entry::Proposal { text, .. }
        | Entry::Tool { summary: text, .. }) = &mut shorter.entry
        else {
            break;
        };
        let keep = text.len() as u64 * (MAX_LINE as u64 - 512) / line.len() as u64;
        *text = clip(text, keep as usize).to_owned();
        line = serde_json::to_string(&shorter).unwrap_or_default();
        if line.len() <= MAX_LINE {
            return line;
        }
    }
    // Only its number and time are certain to fit.
    serde_json::to_string(&Record {
        seq: record.seq,
        at: record.at,
        entry: Entry::Notice {
            text: "An entry was too long to save.".into(),
        },
    })
    .unwrap_or_default()
}
/// The entry a line of a saved chat holds, if this build reads it.
pub fn parse(line: &str) -> Option<Record> {
    (line.len() <= MAX_LINE)
        .then(|| serde_json::from_str(line).ok())
        .flatten()
}

/// What a chat that was read back says was cut short when Neptune closed.
#[derive(Debug, Default, PartialEq)]
pub struct Recovery {
    /// The turn the lead was in.
    pub unfinished: Option<String>,
    /// Events the lead was never told, oldest first.
    pub undelivered: Vec<Record>,
    /// The user wrote something no turn took.
    pub unsent: bool,
}
/// The notice an unfinished turn becomes.
pub const CLOSED_MID_TURN: &str = "Neptune closed while the lead was replying.";
/// The notice for words of the user that no turn took.
pub const CLOSED_UNSENT: &str =
    "Neptune closed before the lead took your last message. Send it again if you still want it.";

/// Reads what was cut short from the newest entries of a chat.
pub fn recover<'a>(records: impl IntoIterator<Item = &'a Record>) -> Recovery {
    let mut recovery = Recovery::default();
    let mut delivered = 0;
    let mut events = Vec::new();
    for record in records {
        match &record.entry {
            Entry::Turn { id, origin } => {
                recovery.unfinished = Some(id.clone());
                if *origin == Origin::User {
                    recovery.unsent = false;
                }
            }
            Entry::End { turn, .. } => {
                recovery.unfinished.take_if(|open| open == turn);
            }
            Entry::Delivered { through } => delivered = delivered.max(*through),
            Entry::Event { .. } => events.push(record),
            Entry::User { .. } => recovery.unsent = true,
            // Said once: the person has been told.
            Entry::Notice { text } if text == CLOSED_UNSENT => recovery.unsent = false,
            _ => {}
        }
    }
    recovery.undelivered = events
        .into_iter()
        .filter(|record| record.seq > delivered)
        .cloned()
        .collect();
    recovery
}

#[derive(Debug, Default)]
pub struct Transcript {
    records: VecDeque<Record>,
    last: u64,
    /// Older entries at the front that were read back, beyond the newest.
    earlier: usize,
}
impl Transcript {
    /// A chat as it was read back: its newest entries, and the number of
    /// the newest entry ever written.
    pub fn restore(records: Vec<Record>, last: u64) -> Self {
        let skip = records.len().saturating_sub(MAX_ENTRIES);
        let last = records.last().map_or(last, |record| record.seq.max(last));
        Self {
            records: records.into_iter().skip(skip).collect(),
            last,
            earlier: 0,
        }
    }
    /// Puts older entries before the ones held. Returns whether more may
    /// still be read back.
    pub fn prepend(&mut self, older: Vec<Record>) -> bool {
        let first = self.records.front().map_or(u64::MAX, |record| record.seq);
        let room = MAX_EARLIER.saturating_sub(self.earlier);
        let older: Vec<Record> = older
            .into_iter()
            .filter(|record| record.seq < first)
            .collect();
        let skip = older.len().saturating_sub(room);
        for record in older.into_iter().skip(skip).rev() {
            self.records.push_front(record);
            self.earlier += 1;
        }
        self.earlier < MAX_EARLIER
    }
    /// The number of the oldest entry held.
    pub fn first(&self) -> Option<u64> {
        self.records.front().map(|record| record.seq)
    }
    /// Begins a new chat: nothing is held, and numbers go on.
    pub fn clear(&mut self) {
        self.records.clear();
        self.earlier = 0;
    }
    /// Adds an entry at time `at` and returns its number.
    pub fn push(&mut self, mut entry: Entry, at: u64) -> u64 {
        if let Entry::User { text, .. }
        | Entry::Lead { text }
        | Entry::Notice { text }
        | Entry::Proposal { text, .. }
        | Entry::Event { text, .. } = &mut entry
            && text.len() > MAX_TEXT
        {
            *text = clip(text, MAX_TEXT).to_owned();
        }
        self.last += 1;
        if self.records.len() >= MAX_ENTRIES + self.earlier {
            self.records.pop_front();
        }
        self.records.push_back(Record {
            seq: self.last,
            at,
            entry,
        });
        self.last
    }
    pub fn records(&self) -> &VecDeque<Record> {
        &self.records
    }
    /// The number of the newest entry, 0 before the first.
    pub fn last(&self) -> u64 {
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_one_line_each_in_the_shape_the_saved_chat_names() {
        let mut chat = Transcript::default();
        chat.push(user("go"), 10);
        chat.push(
            Entry::Turn {
                id: "t1".into(),
                origin: Origin::User,
            },
            11,
        );
        chat.push(
            Entry::Tool {
                name: "spawn_agent".into(),
                summary: "Started agent 12 · Claude Code · auth".into(),
                ok: true,
            },
            12,
        );
        chat.push(
            Entry::Event {
                source: Source::Agent,
                agent: Some(12),
                what: "finished its turn".into(),
                text: "done\nall".into(),
            },
            13,
        );
        chat.push(
            Entry::End {
                turn: "t1".into(),
                outcome: Ended::Completed,
                cost: None,
            },
            14,
        );
        chat.push(Entry::Delivered { through: 4 }, 15);
        let lines: Vec<String> = chat
            .records()
            .iter()
            .map(|record| serde_json::to_string(record).unwrap())
            .collect();
        assert_eq!(
            lines,
            [
                r#"{"seq":1,"at":10,"kind":"user","text":"go"}"#,
                r#"{"seq":2,"at":11,"kind":"turn","id":"t1","origin":"user"}"#,
                r#"{"seq":3,"at":12,"kind":"tool","name":"spawn_agent","summary":"Started agent 12 · Claude Code · auth","ok":true}"#,
                r#"{"seq":4,"at":13,"kind":"event","source":"agent","ref":12,"what":"finished its turn","text":"done\nall"}"#,
                r#"{"seq":5,"at":14,"kind":"end","turn":"t1","outcome":"completed"}"#,
                r#"{"seq":6,"at":15,"kind":"delivered","through":4}"#,
            ]
        );
        for (line, record) in lines.iter().zip(chat.records()) {
            assert_eq!(&serde_json::from_str::<Record>(line).unwrap(), record);
        }
    }

    #[test]
    fn memory_holds_the_newest_entries_and_no_entry_is_longer_than_its_limit() {
        let mut chat = Transcript::default();
        for n in 0..MAX_ENTRIES + 25 {
            chat.push(
                Entry::Notice {
                    text: n.to_string(),
                },
                0,
            );
        }
        assert_eq!(chat.records().len(), MAX_ENTRIES);
        assert_eq!(chat.records()[0].seq, 26);
        assert_eq!(chat.last(), (MAX_ENTRIES + 25) as u64);
        chat.push(
            Entry::Lead {
                text: "é".repeat(MAX_TEXT),
            },
            0,
        );
        let Some(Entry::Lead { text }) = chat.records().back().map(|record| &record.entry) else {
            panic!("no entry");
        };
        assert_eq!(text.len(), MAX_TEXT);
        assert_eq!(clip("héllo", 2), "h");
    }

    fn user(text: &str) -> Entry {
        Entry::User {
            text: text.into(),
            attachments: Vec::new(),
        }
    }
    fn record(seq: u64, entry: Entry) -> Record {
        Record { seq, at: 0, entry }
    }
    fn event(agent: u64, what: &str) -> Entry {
        Entry::Event {
            source: Source::Agent,
            agent: Some(agent),
            what: what.into(),
            text: String::new(),
        }
    }
    fn turn(id: &str, origin: Origin) -> Entry {
        Entry::Turn {
            id: id.into(),
            origin,
        }
    }
    fn end(id: &str) -> Entry {
        Entry::End {
            turn: id.into(),
            outcome: Ended::Completed,
            cost: None,
        }
    }

    #[test]
    fn a_saved_line_is_never_longer_than_its_limit_and_reads_back() {
        let short = record(3, user("hi"));
        assert_eq!(
            line(&short),
            r#"{"seq":3,"at":0,"kind":"user","text":"hi"}"#
        );
        assert_eq!(parse(&line(&short)), Some(short));
        // Text that doubles when it is escaped still fits, with most of
        // what fits kept.
        for text in [
            "\"".repeat(MAX_TEXT),
            "é\n".repeat(MAX_TEXT / 3),
            "x".repeat(MAX_TEXT),
        ] {
            let long = record(4, Entry::Lead { text: text.clone() });
            let saved = line(&long);
            assert!(saved.len() <= MAX_LINE && saved.len() > MAX_LINE * 9 / 10);
            let Some(Record {
                seq: 4,
                entry: Entry::Lead { text: kept },
                ..
            }) = parse(&saved)
            else {
                panic!("not read back");
            };
            assert!(text.starts_with(&kept) && !kept.is_empty());
        }
        // A line that is no entry, or longer than one may be, is not read.
        assert_eq!(parse("{}"), None);
        assert_eq!(parse(r#"{"seq":1,"at":0,"kind":"hologram"}"#), None);
        assert_eq!(parse(HEADER), None);
        let long = format!(
            r#"{{"seq":1,"at":0,"kind":"user","text":"{}"}}"#,
            "x".repeat(MAX_LINE)
        );
        assert_eq!(parse(&long), None);
    }

    #[test]
    fn a_message_keeps_where_its_files_are_and_a_line_without_any_reads_as_before() {
        let shot = Attachment {
            path: "/home/me/Screenshot from 2026.png".into(),
            bytes: 245_760,
        };
        let log = Attachment {
            path: "/tmp/build.log".into(),
            bytes: 900,
        };
        let with = record(
            3,
            Entry::User {
                text: String::new(),
                attachments: vec![shot.clone(), log.clone()],
            },
        );
        let saved = line(&with);
        assert_eq!(
            saved,
            r#"{"seq":3,"at":0,"kind":"user","text":"","attachments":[{"path":"/home/me/Screenshot from 2026.png","bytes":245760},{"path":"/tmp/build.log","bytes":900}]}"#
        );
        assert_eq!(parse(&saved), Some(with));
        // A line written before messages had files, and one without any.
        let old = r#"{"seq":3,"at":0,"kind":"user","text":"hi"}"#;
        assert_eq!(parse(old), Some(record(3, user("hi"))));
        assert_eq!(line(&record(3, user("hi"))), old);
        // A long message is cut to its line; its files are kept whole.
        let long = record(
            4,
            Entry::User {
                text: "\"".repeat(MAX_TEXT),
                attachments: vec![shot.clone(); MAX_ATTACHMENTS],
            },
        );
        let saved = line(&long);
        assert!(saved.len() <= MAX_LINE);
        let Some(Entry::User { attachments, .. }) = parse(&saved).map(|record| record.entry) else {
            panic!("not read back");
        };
        assert_eq!(attachments.len(), MAX_ATTACHMENTS);

        assert_eq!(
            (shot.name(), shot.picture()),
            ("Screenshot from 2026.png", true)
        );
        assert_eq!((log.name(), log.picture()), ("build.log", false));
        assert_eq!((shot.size(), log.size()), ("240 KB".into(), "900 B".into()));
        let large = Attachment {
            path: "/tmp/SHOT.JPEG".into(),
            bytes: 3 * 1024 * 1024 + 512 * 1024,
        };
        assert_eq!((large.picture(), large.size()), (true, "3.5 MB".into()));
    }

    #[test]
    fn what_was_cut_short_is_read_from_a_chat_and_only_that() {
        // Finished and delivered: nothing was cut short.
        let whole = [
            record(1, user("go")),
            record(2, turn("t1", Origin::User)),
            record(3, end("t1")),
            record(4, event(12, "finished its turn")),
            record(5, turn("t2", Origin::Events)),
            record(6, end("t2")),
            record(7, Entry::Delivered { through: 4 }),
        ];
        assert_eq!(recover(&whole), Recovery::default());
        // Closed in a turn, with news the lead never heard and words of
        // the user no turn took.
        let cut = [
            record(1, event(12, "finished its turn")),
            record(2, turn("t1", Origin::Events)),
            record(3, end("t1")),
            record(4, Entry::Delivered { through: 1 }),
            record(5, event(12, "ended")),
            record(6, event(13, "asked a question")),
            record(7, user("and?")),
            record(8, turn("t2", Origin::Events)),
            record(9, Entry::Lead { text: "Loo".into() }),
        ];
        let recovery = recover(&cut);
        assert_eq!(recovery.unfinished.as_deref(), Some("t2"));
        assert_eq!(
            recovery
                .undelivered
                .iter()
                .map(|record| record.seq)
                .collect::<Vec<_>>(),
            [5, 6]
        );
        assert!(recovery.unsent);
        // Said once: after the notices and the mark, nothing is left.
        let mut told = cut.to_vec();
        told.extend([
            record(10, end("t2")),
            record(
                11,
                Entry::Notice {
                    text: CLOSED_MID_TURN.into(),
                },
            ),
            record(
                12,
                Entry::Notice {
                    text: CLOSED_UNSENT.into(),
                },
            ),
            record(13, turn("t3", Origin::Events)),
            record(14, end("t3")),
            record(15, Entry::Delivered { through: 12 }),
        ]);
        assert_eq!(recover(&told), Recovery::default());
        // A user's turn takes the user's words; a mark never goes back.
        let taken = [
            record(1, user("go")),
            record(2, event(7, "ended")),
            record(3, Entry::Delivered { through: 2 }),
            record(4, Entry::Delivered { through: 1 }),
            record(5, turn("t1", Origin::User)),
            record(6, end("t1")),
        ];
        assert_eq!(recover(&taken), Recovery::default());
    }

    #[test]
    fn a_chat_that_was_read_back_goes_on_and_takes_older_entries_in_front() {
        let notice = |seq: u64| {
            record(
                seq,
                Entry::Notice {
                    text: seq.to_string(),
                },
            )
        };
        let mut chat = Transcript::restore((101..=600).map(notice).collect(), 600);
        assert_eq!(chat.records().len(), MAX_ENTRIES);
        assert_eq!((chat.first(), chat.last()), (Some(201), 600));
        assert_eq!(chat.push(Entry::Notice { text: "new".into() }, 0), 601);
        assert_eq!(chat.first(), Some(202));
        // Older ones go in front; what is held already is not taken twice.
        assert!(chat.prepend((150..=210).map(notice).collect()));
        assert_eq!(chat.first(), Some(150));
        let numbers: Vec<u64> = chat.records().iter().map(|record| record.seq).collect();
        assert_eq!(numbers, (150..=601).collect::<Vec<_>>());
        // What was read back stays as new entries come.
        chat.push(
            Entry::Notice {
                text: "newer".into(),
            },
            0,
        );
        assert_eq!((chat.first(), chat.last()), (Some(151), 602));
        assert_eq!(chat.records().len(), MAX_ENTRIES + 52);
        // No more is read back than its bound, the newest of it kept.
        let mut long = Transcript::restore((5000..5010).map(notice).collect(), 5009);
        assert!(!long.prepend((1..=2000).map(notice).collect()));
        assert_eq!(long.records().len(), 10 + MAX_EARLIER);
        assert_eq!(long.first(), Some(2001 - MAX_EARLIER as u64));
        assert!(!long.prepend((1..=100).map(notice).collect()));
        assert_eq!(long.records().len(), 10 + MAX_EARLIER);
        // A chat whose file is empty takes its numbers from the one set aside.
        let mut fresh = Transcript::restore(Vec::new(), 40);
        assert_eq!(fresh.push(Entry::Notice { text: "x".into() }, 0), 41);
        fresh.clear();
        assert!(fresh.records().is_empty() && fresh.first().is_none());
        assert_eq!(fresh.push(Entry::Notice { text: "y".into() }, 0), 42);
    }
}
