//! Bounded observer for notification OSCs. It never consumes terminal input.
use crate::{Notification, NotificationOccasion, TerminalEvent};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use std::collections::VecDeque;

const MAX_SEQUENCE: usize = 8192;
const MAX_TEXT: usize = 4096;
const MAX_PENDING: usize = 16;
const MAX_REPORT: usize = 512;

pub(super) enum Command {
    Event(TerminalEvent),
    Reply(String),
}

#[derive(Default)]
struct Pending {
    notification: Notification,
    encoded: [String; 2],
}

#[derive(Default)]
pub(super) struct Scanner {
    state: u8,
    bytes: Vec<u8>,
    pending: VecDeque<Pending>,
}

impl Scanner {
    pub(super) fn advance(&mut self, mut bytes: &[u8], mut emit: impl FnMut(Command)) {
        while !bytes.is_empty() {
            if self.state == 0 {
                let Some(offset) = memchr::memchr(0x1b, bytes) else {
                    break;
                };
                bytes = &bytes[offset + 1..];
                self.state = 1;
                continue;
            }
            let byte = bytes[0];
            bytes = &bytes[1..];
            // CAN/SUB cancel any escape/string, including discarded strings.
            if matches!(byte, 0x18 | 0x1a) {
                self.state = 0;
                self.bytes.clear();
                continue;
            }
            match self.state {
                1 => match byte {
                    b']' => {
                        self.bytes.clear();
                        self.state = 2;
                    }
                    // Do not interpret embedded OSCs inside other control strings.
                    b'P' | b'X' | b'^' | b'_' => self.state = 6,
                    0x1b => {}
                    _ => self.state = 0,
                },
                2 if byte == 7 => {
                    self.finish(&mut emit);
                    self.state = 0;
                }
                2 if byte == 0x1b => self.state = 3,
                2 => {
                    if self.bytes.len() < MAX_SEQUENCE {
                        self.bytes.push(byte);
                    } else {
                        self.bytes.clear();
                        self.state = 4;
                    }
                }
                3 => {
                    if byte == b'\\' {
                        self.finish(&mut emit);
                        self.state = 0;
                    } else {
                        self.bytes.clear();
                        self.state = 4;
                    }
                }
                4 => {
                    if byte == 7 {
                        self.state = 0;
                    } else if byte == 0x1b {
                        self.state = 5;
                    }
                }
                5 => self.state = if byte == b'\\' { 0 } else { 4 },
                6 => {
                    if byte == 0x1b {
                        self.state = 7;
                    }
                }
                7 => self.state = if byte == b'\\' { 0 } else { 6 },
                _ => self.state = 0,
            }
        }
    }

    fn finish(&mut self, emit: &mut impl FnMut(Command)) {
        let bytes = std::mem::take(&mut self.bytes);
        if let Ok(text) = std::str::from_utf8(&bytes) {
            self.decode(text, emit);
        }
        self.bytes = bytes;
        self.bytes.clear();
    }

    fn decode(&mut self, text: &str, emit: &mut impl FnMut(Command)) {
        if let Some(line) = text.strip_prefix("7717;neptune;") {
            // Passed on as it came: the application holds what it expects.
            if !line.is_empty()
                && line.len() <= MAX_REPORT
                && line.bytes().all(|byte| byte.is_ascii_graphic())
            {
                emit(Command::Event(TerminalEvent::Report(line.to_owned())));
            }
        } else if let Some(title) = text.strip_prefix("9;") {
            // ConEmu uses numeric OSC 9 subcommands (notably 9;4 progress).
            if title
                .split_once(';')
                .is_some_and(|(v, _)| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
            {
                return;
            }
            send(
                Notification {
                    title: clean(title),
                    ..Default::default()
                },
                emit,
            );
        } else if let Some(text) = text.strip_prefix("777;notify;") {
            let (title, body) = text.split_once(';').unwrap_or((text, ""));
            send(
                Notification {
                    title: clean(title),
                    body: clean(body),
                    ..Default::default()
                },
                emit,
            );
        } else if let Some(text) = text.strip_prefix("99;") {
            let Some((metadata, payload)) = text.split_once(';') else {
                return;
            };
            let mut id = None;
            let mut kind = "title";
            let mut done = true;
            let mut encoded = false;
            let mut occasion = None;
            for field in metadata.split(':') {
                let Some((key, value)) = field.split_once('=') else {
                    continue;
                };
                match key {
                    "i" if value.len() <= 128
                        && value
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_-+.".contains(&b)) =>
                    {
                        id = Some(value.to_owned())
                    }
                    "p" => kind = value,
                    "d" => done = value != "0",
                    "e" => encoded = value == "1",
                    "o" => {
                        occasion = Some(match value {
                            "unfocused" => NotificationOccasion::Unfocused,
                            "invisible" => NotificationOccasion::Invisible,
                            _ => NotificationOccasion::Always,
                        })
                    }
                    _ => {}
                }
            }
            if kind == "?" {
                emit(Command::Reply(format!(
                    "\x1b]99;i={}:p=?;p=title,body:o=always,unfocused,invisible\x1b\\",
                    id.as_deref().unwrap_or("0")
                )));
                return;
            }
            if kind == "close" {
                if let Some(id) = id {
                    self.pending
                        .retain(|n| n.notification.id.as_ref() != Some(&id));
                    emit(Command::Event(TerminalEvent::CloseNotification { id }));
                }
                return;
            }
            if !matches!(kind, "title" | "body") {
                return;
            }
            let position = self.pending.iter().position(|n| n.notification.id == id);
            let mut pending = position
                .and_then(|index| self.pending.remove(index))
                .unwrap_or_else(|| Pending {
                    notification: Notification {
                        id,
                        ..Default::default()
                    },
                    ..Default::default()
                });
            if let Some(occasion) = occasion {
                pending.notification.occasion = occasion;
            }
            let index = usize::from(kind == "body");
            if encoded {
                // Keep encoded fragments until completion: chunk boundaries may
                // split base64 quartets or a multi-byte UTF-8 character.
                if pending.encoded[index].len() + payload.len() > MAX_SEQUENCE {
                    return;
                }
                pending.encoded[index].push_str(payload);
            } else {
                let target = if index == 1 {
                    &mut pending.notification.body
                } else {
                    &mut pending.notification.title
                };
                if !pending.encoded[index].is_empty() {
                    let Some(decoded) = decode_base64(&pending.encoded[index]) else {
                        return;
                    };
                    target.push_str(&decoded);
                    *target = clean(target);
                    pending.encoded[index].clear();
                }
                for c in payload.chars().filter(|c| !c.is_control()) {
                    if target.len() + c.len_utf8() > MAX_TEXT {
                        break;
                    }
                    target.push(c);
                }
            }
            if done {
                for (index, encoded) in pending.encoded.iter().enumerate() {
                    if encoded.is_empty() {
                        continue;
                    }
                    let Some(decoded) = decode_base64(encoded) else {
                        return;
                    };
                    let target = if index == 1 {
                        &mut pending.notification.body
                    } else {
                        &mut pending.notification.title
                    };
                    target.push_str(&decoded);
                    *target = clean(target);
                }
                send(pending.notification, emit);
            } else {
                if self.pending.len() == MAX_PENDING {
                    self.pending.pop_front();
                }
                self.pending.push_back(pending);
            }
        }
    }
}

fn decode_base64(mut text: &str) -> Option<String> {
    let mut bytes = Vec::new();
    while !text.is_empty() {
        // Independently encoded chunks may include padding between segments.
        let end = text
            .find('=')
            .map(|start| start + text[start..].bytes().take_while(|b| *b == b'=').count())
            .unwrap_or(text.len());
        let (part, rest) = text.split_at(end);
        bytes.extend(
            STANDARD
                .decode(part)
                .or_else(|_| STANDARD_NO_PAD.decode(part))
                .ok()?,
        );
        text = rest;
    }
    String::from_utf8(bytes).ok()
}

fn clean(text: &str) -> String {
    let mut result = String::new();
    for c in text.chars().filter(|c| !c.is_control()) {
        if result.len() + c.len_utf8() > MAX_TEXT {
            break;
        }
        result.push(c);
    }
    result
}

fn send(mut notification: Notification, emit: &mut impl FnMut(Command)) {
    if notification.title.is_empty() {
        notification.title = std::mem::take(&mut notification.body);
    }
    if !notification.title.is_empty() {
        emit(Command::Event(TerminalEvent::Notification(notification)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn events(bytes: &[u8], chunk: usize) -> Vec<TerminalEvent> {
        let mut scanner = Scanner::default();
        let mut events = Vec::new();
        for bytes in bytes.chunks(chunk) {
            scanner.advance(bytes, |command| {
                if let Command::Event(event) = command {
                    events.push(event)
                }
            });
        }
        events
    }
    #[test]
    fn notifications_survive_every_chunk_boundary_and_terminator() {
        let bytes = b"text\x1b]9;Ready\x07\x1b]777;notify;Build;Done;really\x1b\\\x1b]99;i=a:d=0;Review\x1b\\\x1b]99;i=a:p=body:e=1;SGVsbG8=\x1b\\";
        for chunk in 1..=bytes.len() {
            let result = events(bytes, chunk);
            assert_eq!(result.len(), 3);
            assert!(
                matches!(&result[2], TerminalEvent::Notification(n) if n.title == "Review" && n.body == "Hello")
            );
        }
    }
    #[test]
    fn malformed_progress_and_embedded_strings_are_not_notifications() {
        assert!(events(b"\x1b]9;4;1;50\x07\x1b]99;p=icon;data\x07\x1b]99;e=1;!bad\x07\x1bP\x1b]9;fake\x07\x1b\\\x1b]9;cancel\x18", 1).is_empty());
    }
    #[test]
    fn notifications_decode_base64_split_inside_a_quartet_or_utf8_character() {
        let result = events(b"\x1b]99;i=x:e=1:d=0;5\x07\x1b]99;i=x:e=1;L2g5aW9\x07", 1);
        assert!(matches!(&result[0], TerminalEvent::Notification(n) if n.title == "你好"));
        assert_eq!(decode_base64("SGVsbG8=V29ybGQ="), Some("HelloWorld".into()));
        assert_eq!(decode_base64("SGVsbG8"), Some("Hello".into()));
    }

    #[test]
    fn application_reports_are_short_printable_lines() {
        let bytes = b"\x1b]7717;neptune;abc;run;hook;Stop\x07\x1b]7717;neptune;second\x1b\\";
        for chunk in 1..=bytes.len() {
            assert_eq!(
                events(bytes, chunk),
                [
                    TerminalEvent::Report("abc;run;hook;Stop".into()),
                    TerminalEvent::Report("second".into())
                ]
            );
        }
        let mut long = b"\x1b]7717;neptune;".to_vec();
        long.extend(vec![b'x'; MAX_REPORT + 1]);
        long.extend(b"\x07\x1b]7717;neptune;with space\x07\x1b]7717;neptune;\x07");
        // Another vendor's line, and one inside another control string.
        long.extend(b"\x1b]7717;other;line\x07\x1bP\x1b]7717;neptune;fake\x07\x1b\\");
        assert!(events(&long, 7).is_empty());
    }
    #[test]
    fn oversized_sequences_are_discarded_and_parser_recovers() {
        let mut bytes = b"\x1b]9;".to_vec();
        bytes.extend(vec![b'x'; MAX_SEQUENCE + 1]);
        bytes.extend(b"\x07\x1b]9;Valid\x07");
        assert_eq!(events(&bytes, 13).len(), 1);
    }
    #[test]
    fn pending_chunks_and_text_are_bounded_and_close_discards_them() {
        let mut scanner = Scanner::default();
        for i in 0..100 {
            scanner.advance(format!("\x1b]99;i={i}:d=0;pending\x07").as_bytes(), |_| {});
        }
        assert_eq!(scanner.pending.len(), MAX_PENDING);
        scanner.advance(b"\x1b]99;i=99:p=close;\x07", |_| {});
        assert!(
            !scanner
                .pending
                .iter()
                .any(|n| n.notification.id.as_deref() == Some("99"))
        );
        assert!(clean(&"界".repeat(MAX_TEXT)).len() <= MAX_TEXT);
    }
}
