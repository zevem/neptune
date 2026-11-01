//! A terminal's state as terminal-core describes it, independent of the
//! backend's own serialization, so a replica engine can continue from it.
//!
//! A snapshot carries the primary screen with its history and, while a
//! program uses it, the alternate screen: styled cells (colours, attributes,
//! underline colours, hyperlinks, wide and combining characters, wrapped
//! lines), each screen's cursor and saved cursor with their pending wrap and
//! attributes. It also carries the modes, keyboard protocol flags, cursor
//! style, dynamic colours, title, reported directory, remote process id, the
//! prompt mark and the output the engine had read but not finished parsing.
//!
//! Not carried: the scroll region, tab stops, character sets, the title
//! stack, and keyboard protocol flags below the top of each screen's stack.
//! Programs that rely on them set them again when they redraw.
use super::*;
use crate::view;
use alacritty_terminal::grid::Cursor as GridCursor;
use alacritty_terminal::term::cell::{Flags as CellFlags, Hyperlink as CellHyperlink};
use alacritty_terminal::vte::ansi::{
    self, CursorStyle as AnsiCursorStyle, Handler, KeyboardModes, KeyboardModesApplyBehavior,
    NamedMode, NamedPrivateMode, Processor,
};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub(super) cols: u16,
    pub(super) rows: u16,
    /// Cell attributes, referenced by runs and cursors.
    pub(super) styles: Vec<Style>,
    pub(super) hyperlinks: Vec<Hyperlink>,
    /// History then screen, oldest line first.
    pub(super) primary: Screen,
    /// The screen's lines only; present while the alternate screen is active.
    pub(super) alternate: Option<Screen>,
    /// The active screen's modes, including keyboard protocol flags.
    pub(super) modes: view::Mode,
    pub(super) cursor_style: CursorStyle,
    /// Colours a program changed (OSC 4, 10, 11, 12), by palette index.
    pub(super) colors: Vec<(u16, view::Rgb)>,
    pub(super) title: String,
    pub(super) reported_cwd: Option<PathBuf>,
    pub(super) remote_process_id: Option<u32>,
    /// Where the shell's current prompt starts, as a primary line index.
    pub(super) prompt: Option<Prompt>,
    pub(super) prompt_redraw: bool,
    /// Output read but not yet acted on: an escape sequence or character that
    /// the stream following the snapshot completes. A replica parses it first.
    pub(super) pending: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct Style {
    pub(super) fg: view::Color,
    pub(super) bg: view::Color,
    pub(super) underline: Option<view::Color>,
    pub(super) flags: view::Flags,
    pub(super) hyperlink: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Hyperlink {
    pub(super) id: String,
    pub(super) uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Screen {
    pub(super) lines: Vec<Row>,
    pub(super) cursor: Cursor,
    pub(super) saved_cursor: Cursor,
    /// The keyboard protocol flags in effect on this screen.
    pub(super) keyboard: view::Mode,
}

/// One grid line. Trailing blank cells are left out.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Row {
    pub(super) runs: Vec<Run>,
    /// Zero-width characters drawn over a cell, by column.
    pub(super) combining: Vec<(u16, String)>,
}

/// Consecutive cells of one style, one character per cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Run {
    pub(super) style: u32,
    pub(super) text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Cursor {
    /// Screen-relative.
    pub(super) line: u16,
    pub(super) column: u16,
    /// The next character starts a new line first.
    pub(super) pending_wrap: bool,
    /// The attributes new characters take.
    pub(super) style: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CursorStyle {
    pub(super) shape: view::CursorShape,
    pub(super) blinking: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Prompt {
    pub(super) line: i32,
    pub(super) column: u16,
}

impl Snapshot {
    /// Capture `terminal` with its session state. The caller holds the
    /// terminal lock and has already applied any synchronized update.
    pub(super) fn capture<T: EventListener>(
        terminal: &mut Term<T>,
        shared: &Shared,
        pending: Vec<u8>,
    ) -> Self {
        let mut styles = Styles::default();
        let modes = view::Mode::from_bits_retain(terminal.mode().bits());
        let keyboard = modes & view::Mode::KITTY_KEYBOARD_PROTOCOL;
        let prompt_state = shared.prompt.lock();
        let (primary, alternate, prompt) = if modes.contains(view::Mode::ALT_SCREEN) {
            let alternate = screen(terminal, 0, &mut styles, keyboard);
            // The primary screen is only readable while it is active. Leaving
            // the alternate screen keeps its contents; entering it again
            // clears them, so they are put back from a copy.
            let alternate_grid = terminal.grid().clone();
            terminal.swap_alt();
            let primary_keyboard = view::Mode::from_bits_retain(terminal.mode().bits())
                & view::Mode::KITTY_KEYBOARD_PROTOCOL;
            let primary = screen(
                terminal,
                terminal.topmost_line().0,
                &mut styles,
                primary_keyboard,
            );
            let prompt = prompt_state.anchor_point(terminal);
            terminal.swap_alt();
            *terminal.grid_mut() = alternate_grid;
            // Entering applies the top of the screen's stack, which a program
            // may have changed without pushing.
            Handler::set_keyboard_mode(
                terminal,
                keyboard_modes(keyboard),
                KeyboardModesApplyBehavior::Replace,
            );
            (primary, Some(alternate), prompt)
        } else {
            let primary = screen(terminal, terminal.topmost_line().0, &mut styles, keyboard);
            (primary, None, prompt_state.anchor_point(terminal))
        };
        let style = terminal.cursor_style();
        let metadata = shared.metadata.lock();
        Self {
            cols: terminal.columns() as u16,
            rows: terminal.screen_lines() as u16,
            styles: styles.styles,
            hyperlinks: styles.hyperlinks,
            primary,
            alternate,
            modes,
            cursor_style: CursorStyle {
                shape: engine::cursor_shape(style.shape),
                blinking: style.blinking,
            },
            colors: (0..269)
                .filter_map(|index| {
                    terminal.colors()[index].map(|rgb| {
                        (
                            index as u16,
                            view::Rgb {
                                r: rgb.r,
                                g: rgb.g,
                                b: rgb.b,
                            },
                        )
                    })
                })
                .collect(),
            title: metadata.title.clone(),
            reported_cwd: metadata.reported_cwd.clone(),
            remote_process_id: metadata.remote_process_id,
            prompt: prompt.map(|(line, column)| Prompt {
                line: line.0,
                column: column as u16,
            }),
            prompt_redraw: prompt_state.redraw(),
            pending,
        }
    }

    /// Rebuild a fresh terminal of the snapshot's size and its session state.
    /// History beyond the terminal's own limit is dropped, oldest first.
    /// The caller then parses [`Snapshot::pending`] as the first output.
    pub(super) fn restore<T: EventListener>(&self, terminal: &mut Term<T>, shared: &Shared) {
        let templates: Vec<Cell> = {
            let links: Vec<CellHyperlink> = self
                .hyperlinks
                .iter()
                .map(|link| CellHyperlink::new(Some(&link.id), link.uri.clone()))
                .collect();
            self.styles
                .iter()
                .map(|style| {
                    let mut cell = Cell {
                        fg: engine::backend_color(style.fg),
                        bg: engine::backend_color(style.bg),
                        flags: CellFlags::from_bits_truncate(style.flags.bits()),
                        ..Cell::default()
                    };
                    cell.set_underline_color(style.underline.map(engine::backend_color));
                    cell.set_hyperlink(
                        style
                            .hyperlink
                            .and_then(|index| links.get(index as usize).cloned()),
                    );
                    cell
                })
                .collect()
        };
        let template = |style: u32| templates.get(style as usize).cloned().unwrap_or_default();
        let cursor = |cursor: &Cursor, terminal: &Term<T>| GridCursor {
            point: Point::new(
                Line(i32::from(cursor.line).min(terminal.screen_lines() as i32 - 1)),
                Column(usize::from(cursor.column).min(terminal.columns() - 1)),
            ),
            template: template(cursor.style),
            charsets: Default::default(),
            input_needs_wrap: cursor.pending_wrap,
        };
        let rows = terminal.screen_lines();
        let limit = shared.config.lock().scrolling_history;
        let lines = &self.primary.lines;
        let kept = &lines[lines.len().saturating_sub(limit + rows)..];
        let history = kept.len().saturating_sub(rows);
        let mut scrolled = 0;
        while scrolled < history {
            let count = (history - scrolled).min(rows);
            terminal
                .grid_mut()
                .scroll_up::<ansi::Color>(&(Line(0)..Line(rows as i32)), count);
            scrolled += count;
        }
        for (index, row) in kept.iter().enumerate() {
            write_row(
                terminal,
                Line(index as i32 - history as i32),
                row,
                &template,
            );
        }
        let primary_cursor = cursor(&self.primary.cursor, terminal);
        let primary_saved = cursor(&self.primary.saved_cursor, terminal);
        terminal.grid_mut().cursor = primary_cursor;
        terminal.grid_mut().saved_cursor = primary_saved;
        // Lines keep their numbers: dropping the oldest history leaves the
        // screen at line zero. A prompt in dropped history is gone.
        shared.prompt.lock().restore(
            terminal,
            self.prompt
                .filter(|mark| mark.line >= -(history as i32) && mark.line < rows as i32)
                .map(|mark| (Line(mark.line), usize::from(mark.column))),
            self.prompt_redraw,
        );
        push_keyboard(terminal, self.primary.keyboard);
        let active = match &self.alternate {
            Some(alternate) => {
                terminal.swap_alt();
                for (index, row) in alternate.lines.iter().take(rows).enumerate() {
                    write_row(terminal, Line(index as i32), row, &template);
                }
                push_keyboard(terminal, alternate.keyboard);
                alternate
            }
            None => &self.primary,
        };
        set_modes(terminal, self.modes);
        // Modes such as origin move the cursor, so cursors come last.
        terminal.grid_mut().cursor = cursor(&active.cursor, terminal);
        terminal.grid_mut().saved_cursor = cursor(&active.saved_cursor, terminal);
        let style = AnsiCursorStyle {
            shape: engine::backend_cursor_shape(self.cursor_style.shape),
            blinking: self.cursor_style.blinking,
        };
        if terminal.cursor_style() != style {
            terminal.set_cursor_style(Some(style));
        }
        for &(index, rgb) in &self.colors {
            terminal.set_color(
                usize::from(index),
                Rgb {
                    r: rgb.r,
                    g: rgb.g,
                    b: rgb.b,
                },
            );
        }
        if !self.title.is_empty() {
            terminal.set_title(Some(self.title.clone()));
        }
        let mut metadata = shared.metadata.lock();
        metadata.title.clone_from(&self.title);
        metadata.reported_cwd.clone_from(&self.reported_cwd);
        metadata.remote_process_id = self.remote_process_id;
    }
}

/// Deduplicates cell attributes while a snapshot is captured.
#[derive(Default)]
struct Styles {
    styles: Vec<Style>,
    index: HashMap<Style, u32>,
    hyperlinks: Vec<Hyperlink>,
    /// Keyed by the target's allocation, which the grid keeps alive.
    links: HashMap<(usize, usize), u32>,
}

impl Styles {
    fn get(&mut self, cell: &Cell) -> u32 {
        let hyperlink = cell.hyperlink().map(|link| {
            let key = (link.id().as_ptr() as usize, link.uri().as_ptr() as usize);
            *self.links.entry(key).or_insert_with(|| {
                self.hyperlinks.push(Hyperlink {
                    id: link.id().to_owned(),
                    uri: link.uri().to_owned(),
                });
                self.hyperlinks.len() as u32 - 1
            })
        });
        let style = Style {
            fg: engine::color(cell.fg),
            bg: engine::color(cell.bg),
            underline: cell.underline_color().map(engine::color),
            flags: view::Flags::from_bits_retain(cell.flags.bits()),
            hyperlink,
        };
        if let Some(&index) = self.index.get(&style) {
            return index;
        }
        self.styles.push(style.clone());
        let index = self.styles.len() as u32 - 1;
        self.index.insert(style, index);
        index
    }

    fn cursor(&mut self, cursor: &GridCursor<Cell>) -> Cursor {
        Cursor {
            line: cursor.point.line.0.max(0) as u16,
            column: cursor.point.column.0 as u16,
            pending_wrap: cursor.input_needs_wrap,
            style: self.get(&cursor.template),
        }
    }
}

/// The active grid's lines from `top` to the bottom of the screen.
fn screen<T>(terminal: &Term<T>, top: i32, styles: &mut Styles, keyboard: view::Mode) -> Screen {
    let grid = terminal.grid();
    let columns = terminal.columns();
    let blank = Cell::default();
    let lines = (top..terminal.screen_lines() as i32)
        .map(|line| {
            let cells = &grid[Line(line)];
            let end = (0..columns)
                .rev()
                .find(|&column| cells[Column(column)] != blank)
                .map_or(0, |column| column + 1);
            let mut row = Row::default();
            let mut previous: Option<(&Cell, u32)> = None;
            for column in 0..end {
                let cell = &cells[Column(column)];
                // Neighbouring cells usually share attributes.
                let style = match previous {
                    Some((last, style))
                        if last.fg == cell.fg
                            && last.bg == cell.bg
                            && last.flags == cell.flags
                            && last.extra.is_none()
                            && cell.extra.is_none() =>
                    {
                        style
                    }
                    _ => styles.get(cell),
                };
                previous = Some((cell, style));
                match row.runs.last_mut() {
                    Some(run) if run.style == style => run.text.push(cell.c),
                    _ => row.runs.push(Run {
                        style,
                        text: cell.c.to_string(),
                    }),
                }
                if let Some(extra) = cell.zerowidth().filter(|extra| !extra.is_empty()) {
                    row.combining.push((column as u16, extra.iter().collect()));
                }
            }
            row
        })
        .collect();
    Screen {
        lines,
        cursor: styles.cursor(&grid.cursor),
        saved_cursor: styles.cursor(&grid.saved_cursor),
        keyboard,
    }
}

fn write_row<T>(terminal: &mut Term<T>, line: Line, row: &Row, template: &impl Fn(u32) -> Cell) {
    let columns = terminal.columns();
    let cells = &mut terminal.grid_mut()[line];
    let mut column = 0;
    for run in &row.runs {
        let style = template(run.style);
        for c in run.text.chars() {
            if column >= columns {
                return;
            }
            let mut cell = style.clone();
            cell.c = c;
            cells[Column(column)] = cell;
            column += 1;
        }
    }
    for (column, extra) in &row.combining {
        if usize::from(*column) < columns {
            for c in extra.chars() {
                cells[Column(usize::from(*column))].push_zerowidth(c);
            }
        }
    }
}

fn keyboard_modes(mode: view::Mode) -> KeyboardModes {
    [
        (
            view::Mode::DISAMBIGUATE_ESC_CODES,
            KeyboardModes::DISAMBIGUATE_ESC_CODES,
        ),
        (
            view::Mode::REPORT_EVENT_TYPES,
            KeyboardModes::REPORT_EVENT_TYPES,
        ),
        (
            view::Mode::REPORT_ALTERNATE_KEYS,
            KeyboardModes::REPORT_ALTERNATE_KEYS,
        ),
        (
            view::Mode::REPORT_ALL_KEYS_AS_ESC,
            KeyboardModes::REPORT_ALL_KEYS_AS_ESC,
        ),
        (
            view::Mode::REPORT_ASSOCIATED_TEXT,
            KeyboardModes::REPORT_ASSOCIATED_TEXT,
        ),
    ]
    .into_iter()
    .filter(|(flag, _)| mode.contains(*flag))
    .fold(KeyboardModes::NO_MODE, |modes, (_, flag)| modes | flag)
}

fn push_keyboard<T: EventListener>(terminal: &mut Term<T>, keyboard: view::Mode) {
    let modes = keyboard_modes(keyboard);
    if !modes.is_empty() {
        terminal.push_keyboard_mode(modes);
    }
}

/// Bring every mode but the screen, keyboard protocol and vi mode to `wanted`.
fn set_modes<T: EventListener>(terminal: &mut Term<T>, wanted: view::Mode) {
    use view::Mode;
    let private = [
        (Mode::SHOW_CURSOR, NamedPrivateMode::ShowCursor),
        (Mode::APP_CURSOR, NamedPrivateMode::CursorKeys),
        (Mode::FOCUS_IN_OUT, NamedPrivateMode::ReportFocusInOut),
        (Mode::BRACKETED_PASTE, NamedPrivateMode::BracketedPaste),
        (Mode::ALTERNATE_SCROLL, NamedPrivateMode::AlternateScroll),
        (Mode::URGENCY_HINTS, NamedPrivateMode::UrgencyHints),
        (Mode::LINE_WRAP, NamedPrivateMode::LineWrap),
        (Mode::ORIGIN, NamedPrivateMode::Origin),
        // Mouse reports and encodings are each mutually exclusive.
        (
            Mode::MOUSE_REPORT_CLICK,
            NamedPrivateMode::ReportMouseClicks,
        ),
        (Mode::MOUSE_DRAG, NamedPrivateMode::ReportCellMouseMotion),
        (Mode::MOUSE_MOTION, NamedPrivateMode::ReportAllMouseMotion),
        (Mode::SGR_MOUSE, NamedPrivateMode::SgrMouse),
        (Mode::UTF8_MOUSE, NamedPrivateMode::Utf8Mouse),
    ];
    let current = |terminal: &Term<T>| Mode::from_bits_retain(terminal.mode().bits());
    for (flag, mode) in private {
        let on = wanted.contains(flag);
        if current(terminal).contains(flag) != on {
            if on {
                terminal.set_private_mode(mode.into());
            } else {
                terminal.unset_private_mode(mode.into());
            }
        }
    }
    for (flag, mode) in [
        (Mode::INSERT, NamedMode::Insert),
        (Mode::LINE_FEED_NEW_LINE, NamedMode::LineFeedNewLine),
    ] {
        let on = wanted.contains(flag);
        if current(terminal).contains(flag) != on {
            if on {
                terminal.set_mode(mode.into());
            } else {
                terminal.unset_mode(mode.into());
            }
        }
    }
    if current(terminal).contains(Mode::APP_KEYPAD) != wanted.contains(Mode::APP_KEYPAD) {
        if wanted.contains(Mode::APP_KEYPAD) {
            terminal.set_keypad_application_mode();
        } else {
            terminal.unset_keypad_application_mode();
        }
    }
}

/// A session engine's parser, which also follows how far it has acted on
/// its output, so a snapshot can carry the rest.
#[derive(Default)]
pub(super) struct Parser {
    processor: Processor,
    unfinished: Unfinished,
}

impl Parser {
    pub(super) fn advance<T: EventListener>(&mut self, terminal: &mut Term<T>, bytes: &[u8]) {
        self.processor.advance(terminal, bytes);
        self.unfinished.advance(bytes);
    }

    /// When a synchronized update in progress must be applied anyway.
    pub(super) fn sync_deadline(&self) -> Option<Instant> {
        self.processor.sync_timeout().sync_timeout()
    }

    /// Output held for a synchronized update, not yet on the grid.
    pub(super) fn sync_bytes_count(&self) -> usize {
        self.processor.sync_bytes_count()
    }

    pub(super) fn stop_sync<T: EventListener>(&mut self, terminal: &mut Term<T>) {
        self.processor.stop_sync(terminal);
    }

    /// The bytes of an escape sequence or character the output has begun
    /// but not finished. Any synchronized update must be applied first.
    pub(super) fn unfinished(&self) -> Vec<u8> {
        self.unfinished.bytes.clone()
    }
}

/// Bytes kept of one unfinished sequence; a longer string keeps its start.
const MAX_UNFINISHED: usize = 64 * 1024;

/// Follows the parser's state machine only as far as needed to know where
/// its last sequence started and whether it has ended.
#[derive(Default)]
struct Unfinished {
    state: Scan,
    bytes: Vec<u8>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
enum Scan {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    DcsEntry,
    DcsParam,
    DcsIntermediate,
    DcsPassthrough,
    /// An ignored control string, ended only by ESC, CAN or SUB.
    Ignore,
    Osc,
}

impl Unfinished {
    fn advance(&mut self, bytes: &[u8]) {
        // ESC starts a sequence from any state, and CAN or SUB abandons one,
        // so only what follows the last of them can still be unfinished.
        let rest = match bytes
            .iter()
            .rposition(|&byte| matches!(byte, 0x1b | 0x18 | 0x1a))
        {
            Some(index) => {
                self.clear();
                if bytes[index] == 0x1b {
                    self.state = Scan::Escape;
                    self.bytes.push(0x1b);
                } else {
                    self.state = Scan::Ground;
                }
                &bytes[index + 1..]
            }
            None => bytes,
        };
        if self.state == Scan::Ground {
            self.keep_partial_character(rest);
            return;
        }
        for (index, &byte) in rest.iter().enumerate() {
            self.state = self.state.next(byte);
            if self.state == Scan::Ground {
                self.clear();
                self.keep_partial_character(&rest[index + 1..]);
                return;
            }
            if self.bytes.len() < MAX_UNFINISHED {
                self.bytes.push(byte);
            }
        }
    }

    fn clear(&mut self) {
        self.bytes.clear();
        if self.bytes.capacity() > 4096 {
            self.bytes.shrink_to(64);
        }
    }

    /// In ground state, keep a UTF-8 character that the output cut short.
    fn keep_partial_character(&mut self, bytes: &[u8]) {
        let mut tail = if bytes.len() >= 4 {
            Vec::new()
        } else {
            std::mem::take(&mut self.bytes)
        };
        tail.extend_from_slice(&bytes[bytes.len().saturating_sub(4)..]);
        self.bytes.clear();
        for back in 1..=tail.len().min(3) {
            let byte = tail[tail.len() - back];
            if byte & 0xc0 == 0x80 {
                continue;
            }
            let needed = match byte {
                0xf0.. => 4,
                0xe0.. => 3,
                0xc0.. => 2,
                _ => 1,
            };
            if needed > back {
                self.bytes.extend_from_slice(&tail[tail.len() - back..]);
            }
            return;
        }
    }
}

impl Scan {
    /// The next state for any byte but ESC, CAN and SUB.
    fn next(self, byte: u8) -> Self {
        match (self, byte) {
            (Scan::Escape, 0x20..=0x2f) => Scan::EscapeIntermediate,
            (Scan::Escape, b'P') => Scan::DcsEntry,
            (Scan::Escape, b'X' | b'^' | b'_') => Scan::Ignore,
            (Scan::Escape, b'[') => Scan::Csi,
            (Scan::Escape, b']') => Scan::Osc,
            (Scan::Escape | Scan::EscapeIntermediate, 0x30..=0x7e) => Scan::Ground,
            (Scan::Csi, 0x40..=0x7e) => Scan::Ground,
            (Scan::DcsEntry | Scan::DcsParam, 0x20..=0x2f) => Scan::DcsIntermediate,
            (Scan::DcsEntry, 0x30..=0x3f) => Scan::DcsParam,
            (Scan::DcsParam, 0x3c..=0x3f) | (Scan::DcsIntermediate, 0x30..=0x3f) => Scan::Ignore,
            (Scan::DcsEntry | Scan::DcsParam | Scan::DcsIntermediate, 0x40..=0x7e) => {
                Scan::DcsPassthrough
            }
            (Scan::DcsPassthrough, 0x9c) => Scan::Ground,
            (Scan::Osc, 0x07) => Scan::Ground,
            (state, _) => state,
        }
    }
}
