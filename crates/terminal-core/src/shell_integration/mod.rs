//! Portable OSC 7 / OSC 9;9 cwd and OSC 133 prompt tracking, independent of
//! /proc polling, plus the prompt hooks that make Windows shells report.
use super::*;

/// PowerShell's location is not its process directory, so only the shell can
/// report it. Wrap whichever prompt the user's profile defined; the report is
/// written beside the prompt string so PSReadLine's prompt width is unchanged.
#[cfg(windows)]
const POWERSHELL_REPORT: &str = "$global:__neptune_prompt = $function:prompt; \
function global:prompt { \
$l = $executionContext.SessionState.Path.CurrentLocation; \
if ($l.Provider.Name -eq 'FileSystem') { \
[Console]::Write([string][char]27 + ']9;9;' + $l.ProviderPath + [string][char]27 + '\\') }; \
& $global:__neptune_prompt }";

/// Windows shells report no directory by default. Add an OSC 9;9 report to
/// each prompt of a plain local cmd or PowerShell without editing user files.
#[cfg(windows)]
pub(super) fn report_cwd(command: &mut CommandBuilder, shell: &str) {
    let name = std::path::Path::new(shell)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_ascii_lowercase());
    match name.as_deref() {
        // A default-program builder cannot take arguments; it is ComSpec.
        Some("pwsh" | "powershell") if !command.is_default_prog() => {
            command.args(["-NoExit", "-Command", POWERSHELL_REPORT]);
        }
        Some("cmd") => {
            let prompt = command
                .get_env("PROMPT")
                .map(|prompt| prompt.to_string_lossy().into_owned())
                .unwrap_or_else(|| "$P$G".into());
            command.env("PROMPT", format!("$E]9;9;$P$E\\{prompt}"));
        }
        _ => {}
    }
}

/// A bounded observer of OSC 7 and ConEmu/Windows Terminal OSC 9;9 directory
/// reports. Ordinary printable runs are skipped using memchr; only a matching
/// sequence allocates. VT parsing still belongs to Alacritty.
#[derive(Default)]
pub(super) struct CwdTracker {
    state: u8,
    pub(super) bytes: Vec<u8>,
    pub(super) remote_pid: Option<u32>,
}

impl CwdTracker {
    pub(super) fn advance(&mut self, mut bytes: &[u8]) -> Option<PathBuf> {
        let mut path = None;
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
            match self.state {
                1 => {
                    self.state = if byte == b']' {
                        self.bytes.clear();
                        2
                    } else if byte == 0x1b {
                        1
                    } else {
                        0
                    }
                }
                2 if byte == 7 => {
                    self.observe_pid();
                    path = decode_report(&self.bytes).or(path);
                    self.state = 0;
                }
                2 if byte == 0x1b => self.state = 3,
                2 => {
                    if self.bytes.len() < MAX_OSC {
                        self.bytes.push(byte);
                    }
                    if self.bytes.len() == MAX_OSC || !is_report_prefix(&self.bytes) {
                        self.bytes.clear();
                        self.state = 4;
                    }
                }
                3 => {
                    if byte == b'\\' {
                        self.observe_pid();
                        path = decode_report(&self.bytes).or(path);
                    }
                    self.state = 0;
                }
                4 => {
                    self.state = if byte == 7 {
                        0
                    } else if byte == 0x1b {
                        5
                    } else {
                        4
                    }
                }
                5 => self.state = if byte == b'\\' { 0 } else { 4 },
                _ => self.state = 0,
            }
        }
        path
    }

    fn observe_pid(&mut self) {
        if let Some(bytes) = self.bytes.strip_prefix(REPORTS[2]) {
            self.remote_pid = std::str::from_utf8(bytes)
                .ok()
                .filter(|text| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|text| text.parse::<u32>().ok())
                .filter(|pid| *pid > 1);
        }
    }
}

const REPORTS: [&[u8]; 3] = [b"7;", b"9;9;", b"777;neptune;pid;"];

fn is_report_prefix(bytes: &[u8]) -> bool {
    REPORTS
        .iter()
        .any(|prefix| bytes.iter().zip(*prefix).all(|(a, b)| a == b))
}

fn decode_report(bytes: &[u8]) -> Option<PathBuf> {
    match bytes.strip_prefix(REPORTS[0]) {
        Some(uri) => decode_cwd(uri),
        None => decode_path(bytes.strip_prefix(REPORTS[1])?),
    }
    .filter(|path| is_local_report(path))
}

/// Any program's output can claim a directory, and splits/restoration later
/// probe it and start shells there. Refuse Windows UNC and device paths so
/// terminal output cannot make Neptune connect to a network share. Drive paths
/// and rooted paths (including remote hosts' Unix paths) remain valid.
fn is_local_report(path: &std::path::Path) -> bool {
    use std::path::{Component, Prefix};
    match path.components().next() {
        Some(Component::Prefix(prefix)) => matches!(prefix.kind(), Prefix::Disk(_)),
        _ => true,
    }
}

/// OSC 9;9 carries a native path, optionally quoted, rather than a URI.
pub(super) fn decode_path(bytes: &[u8]) -> Option<PathBuf> {
    let text = std::str::from_utf8(bytes).ok()?;
    let text = text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or(text);
    let path = PathBuf::from(text);
    (!text.contains('\0') && path.is_absolute()).then_some(path)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PromptMark {
    Start { redraw: Option<bool> },
    Output,
}

/// Only OSC 133 is observed; the VT processor remains authoritative for every
/// escape and glyph. Completed marker offsets let it process text up to each
/// marker before its exact cursor position is recorded, even across PTY reads.
#[derive(Default)]
pub(super) struct PromptScanner {
    state: u8,
    pub(super) bytes: Vec<u8>,
}

impl PromptScanner {
    pub(super) fn advance(&mut self, bytes: &[u8]) -> Vec<(usize, PromptMark)> {
        let mut marks = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() {
            if self.state == 0 {
                let Some(next) = memchr::memchr(0x1b, &bytes[offset..]) else {
                    break;
                };
                offset += next;
            }
            let byte = bytes[offset];
            offset += 1;
            match self.state {
                0 => self.state = 1,
                1 => {
                    if byte == b']' {
                        self.bytes.clear();
                        self.state = 2;
                    } else if byte == b'c' {
                        marks.push((offset, PromptMark::Output));
                        self.state = 0;
                    } else if byte != 0x1b {
                        self.state = 0;
                    }
                }
                2 => {
                    if byte == 7 {
                        if let Some(mark) = self.decode() {
                            marks.push((offset, mark));
                        }
                        self.state = 0;
                    } else if byte == 0x1b {
                        self.state = 3;
                    } else if self.bytes.len() < 256 {
                        self.bytes.push(byte);
                        let prefix = b"133;";
                        if self.bytes.len() <= prefix.len()
                            && self.bytes != prefix[..self.bytes.len()]
                        {
                            self.state = 4;
                        }
                    } else {
                        self.bytes.clear();
                        self.state = 4;
                    }
                }
                3 => {
                    if byte == b'\\' {
                        if let Some(mark) = self.decode() {
                            marks.push((offset, mark));
                        }
                        self.state = 0;
                    } else {
                        self.state = 4;
                    }
                }
                4 => {
                    self.state = if byte == 7 {
                        0
                    } else if byte == 0x1b {
                        5
                    } else {
                        4
                    };
                }
                5 => self.state = if byte == b'\\' { 0 } else { 4 },
                _ => unreachable!(),
            }
        }
        marks
    }

    pub(super) fn decode(&self) -> Option<PromptMark> {
        let text = std::str::from_utf8(&self.bytes)
            .ok()?
            .strip_prefix("133;")?;
        let mut parts = text.split(';');
        let action = parts.next()?;
        let mut continuation = false;
        let mut redraw = None;
        for option in parts {
            match option {
                "k=r" | "k=s" | "k=c" => continuation = true,
                "redraw=0" => redraw = Some(false),
                "redraw=1" => redraw = Some(true),
                _ => {}
            }
        }
        match action {
            "A" | "N" | "P" if !continuation => Some(PromptMark::Start { redraw }),
            "C" | "D" => Some(PromptMark::Output),
            _ => None,
        }
    }
}

pub(super) struct PromptAnchor {
    /// Row allocations stay with their content when the grid rotates. This is
    /// an identity value only: no raw pointer is retained or dereferenced, and
    /// no sentinel attributes are inserted into the application's cells.
    pub(super) row_identity: usize,
    column: usize,
}

pub(super) struct PromptState {
    pub(super) anchor: Option<PromptAnchor>,
    redraw: bool,
}

impl Default for PromptState {
    fn default() -> Self {
        Self {
            anchor: None,
            redraw: true,
        }
    }
}

impl PromptState {
    pub(super) fn mark<T: EventListener>(&mut self, terminal: &Term<T>, mark: PromptMark) {
        match mark {
            PromptMark::Start { redraw } => {
                if let Some(redraw) = redraw {
                    self.redraw = redraw;
                }
                if terminal.mode().contains(TermMode::ALT_SCREEN) {
                    return;
                }
                let point = terminal.grid().cursor.point;
                self.anchor = Some(PromptAnchor {
                    row_identity: row_identity(terminal, point.line),
                    column: point.column.0,
                });
            }
            PromptMark::Output => self.anchor = None,
        }
    }

    pub(super) fn resize<T: EventListener>(&mut self, terminal: &mut Term<T>, size: Size) {
        if terminal.columns() == size.cols as usize && terminal.screen_lines() == size.rows as usize
        {
            terminal.resize(size);
            return;
        }
        let start = self.anchor.as_ref().and_then(|anchor| {
            if !self.redraw || terminal.mode().contains(TermMode::ALT_SCREEN) {
                return None;
            }
            // The visible area is most likely; a long input can have scrolled
            // its prompt into history. Row identity also follows rotations at
            // the scrollback cap, where history_size no longer increases.
            (0..terminal.screen_lines() as i32)
                .chain((terminal.topmost_line().0..0).rev())
                .map(Line)
                .find(|line| row_identity(terminal, *line) == anchor.row_identity)
        });
        let mut distance = None;
        if let Some(start) = start {
            let cursor = terminal.grid().cursor.point;
            if start <= cursor.line {
                distance = Some(cursor.line.0 - start.0);
                let column = self.anchor.as_ref().unwrap().column;
                let columns = terminal.columns();
                for line in start.0..terminal.screen_lines() as i32 {
                    let row = &mut terminal.grid_mut()[Line(line)];
                    if line == start.0 && column > 0 {
                        for col in column..columns {
                            row[Column(col)] = Cell::default();
                        }
                    } else {
                        row.reset(&Cell::default());
                    }
                }
                // Zsh redraws its own input buffer after SIGWINCH. Reflowing
                // cleared whitespace before an old cursor beyond the new edge
                // would still change its remembered prompt-to-cursor distance.
                let cursor = &mut terminal.grid_mut().cursor;
                cursor.point.column.0 = cursor.point.column.0.min(size.cols as usize - 1);
                cursor.input_needs_wrap = false;
            }
        }
        terminal.resize(size);
        if let Some(distance) = distance {
            let line =
                Line(terminal.grid().cursor.point.line.0 - distance).max(terminal.topmost_line());
            let column = self
                .anchor
                .as_ref()
                .unwrap()
                .column
                .min(size.cols as usize - 1);
            self.anchor = Some(PromptAnchor {
                row_identity: row_identity(terminal, line),
                column,
            });
        } else {
            // Grid reallocation can change row identities. Without a verified
            // active prompt, preserve content until the next explicit marker.
            self.anchor = None;
        }
    }
}

pub(super) fn row_identity<T: EventListener>(terminal: &Term<T>, line: Line) -> usize {
    std::ptr::from_ref(&terminal.grid()[Point::new(line, Column(0))]) as usize
}

pub(super) fn decode_cwd(bytes: &[u8]) -> Option<PathBuf> {
    let uri = std::str::from_utf8(bytes).ok()?.strip_prefix("file://")?;
    let path = &uri[uri.find('/')?..];
    let mut decoded = Vec::with_capacity(path.len());
    let mut bytes = path.as_bytes().iter().copied();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = (bytes.next()? as char).to_digit(16)?;
            let low = (bytes.next()? as char).to_digit(16)?;
            let value = (high * 16 + low) as u8;
            if value == 0 {
                return None;
            }
            decoded.push(value);
        } else if byte == 0 {
            return None;
        } else {
            decoded.push(byte);
        }
    }
    let path = String::from_utf8(decoded).ok()?;
    #[cfg(windows)]
    let path = if path.as_bytes().get(2) == Some(&b':') {
        path[1..].to_owned()
    } else {
        path
    };
    Some(PathBuf::from(path))
}
