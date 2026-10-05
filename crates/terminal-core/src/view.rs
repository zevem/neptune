//! Engine-independent cells, coordinates and immutable visible rows.
use std::sync::Arc;

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct Mode: u32 {
        const NONE                    = 0;
        const SHOW_CURSOR             = 1;
        const APP_CURSOR              = 1 << 1;
        const APP_KEYPAD              = 1 << 2;
        const MOUSE_REPORT_CLICK      = 1 << 3;
        const BRACKETED_PASTE         = 1 << 4;
        const SGR_MOUSE               = 1 << 5;
        const MOUSE_MOTION            = 1 << 6;
        const LINE_WRAP               = 1 << 7;
        const LINE_FEED_NEW_LINE      = 1 << 8;
        const ORIGIN                  = 1 << 9;
        const INSERT                  = 1 << 10;
        const FOCUS_IN_OUT            = 1 << 11;
        const ALT_SCREEN              = 1 << 12;
        const MOUSE_DRAG              = 1 << 13;
        const UTF8_MOUSE              = 1 << 14;
        const ALTERNATE_SCROLL        = 1 << 15;
        const VI                      = 1 << 16;
        const URGENCY_HINTS           = 1 << 17;
        const DISAMBIGUATE_ESC_CODES  = 1 << 18;
        const REPORT_EVENT_TYPES      = 1 << 19;
        const REPORT_ALTERNATE_KEYS   = 1 << 20;
        const REPORT_ALL_KEYS_AS_ESC  = 1 << 21;
        const REPORT_ASSOCIATED_TEXT  = 1 << 22;
        const MOUSE_MODE              = Self::MOUSE_REPORT_CLICK.bits() | Self::MOUSE_MOTION.bits() | Self::MOUSE_DRAG.bits();
        const KITTY_KEYBOARD_PROTOCOL = Self::DISAMBIGUATE_ESC_CODES.bits()
                                      | Self::REPORT_EVENT_TYPES.bits()
                                      | Self::REPORT_ALTERNATE_KEYS.bits()
                                      | Self::REPORT_ALL_KEYS_AS_ESC.bits()
                                      | Self::REPORT_ASSOCIATED_TEXT.bits();
         const ANY                    = u32::MAX;
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct Flags: u16 {
        const INVERSE                   = 0b0000_0000_0000_0001;
        const BOLD                      = 0b0000_0000_0000_0010;
        const ITALIC                    = 0b0000_0000_0000_0100;
        const BOLD_ITALIC               = 0b0000_0000_0000_0110;
        const UNDERLINE                 = 0b0000_0000_0000_1000;
        const WRAPLINE                  = 0b0000_0000_0001_0000;
        const WIDE_CHAR                 = 0b0000_0000_0010_0000;
        const WIDE_CHAR_SPACER          = 0b0000_0000_0100_0000;
        const DIM                       = 0b0000_0000_1000_0000;
        const DIM_BOLD                  = 0b0000_0000_1000_0010;
        const HIDDEN                    = 0b0000_0001_0000_0000;
        const STRIKEOUT                 = 0b0000_0010_0000_0000;
        const LEADING_WIDE_CHAR_SPACER  = 0b0000_0100_0000_0000;
        const DOUBLE_UNDERLINE          = 0b0000_1000_0000_0000;
        const UNDERCURL                 = 0b0001_0000_0000_0000;
        const DOTTED_UNDERLINE          = 0b0010_0000_0000_0000;
        const DASHED_UNDERLINE          = 0b0100_0000_0000_0000;
        const ALL_UNDERLINES            = Self::UNDERLINE.bits() | Self::DOUBLE_UNDERLINE.bits()
                                        | Self::UNDERCURL.bits() | Self::DOTTED_UNDERLINE.bits()
                                        | Self::DASHED_UNDERLINE.bits();
    }
}

/// Input protocol modes owned by Neptune.
pub type TermMode = Mode;

/// Grid coordinates: negative lines are history, line zero starts the live screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    pub line: i32,
    pub column: usize,
}
impl Point {
    pub const fn new(line: i32, column: usize) -> Self {
        Self { line, column }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionType {
    Simple,
    Block,
    Semantic,
    Lines,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Beam,
    HollowBlock,
    Hidden,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
    pub point: Point,
    pub shape: CursorShape,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum NamedColor {
    Black = 0,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
    Foreground = 256,
    Background,
    Cursor,
    DimBlack,
    DimRed,
    DimGreen,
    DimYellow,
    DimBlue,
    DimMagenta,
    DimCyan,
    DimWhite,
    BrightForeground,
    DimForeground,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    Named(NamedColor),
    Spec(Rgb),
    Indexed(u8),
}
pub type Colors = [Option<Rgb>; 269];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub column: usize,
    pub c: char,
    pub extra: Vec<char>,
    pub fg: Color,
    pub bg: Color,
    pub flags: Flags,
    /// OSC 8 target, shared by cells belonging to the same hyperlink.
    pub hyperlink: Option<Arc<str>>,
}
impl Default for Cell {
    fn default() -> Self {
        Self {
            column: 0,
            c: ' ',
            extra: Vec::new(),
            fg: Color::Named(NamedColor::Foreground),
            bg: Color::Named(NamedColor::Background),
            flags: Flags::empty(),
            hyperlink: None,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionRange {
    pub start: Point,
    pub end: Point,
    pub is_block: bool,
}
impl SelectionRange {
    pub fn contains(self, point: Point) -> bool {
        self.start.line <= point.line
            && self.end.line >= point.line
            && (self.start.column <= point.column
                || (self.start.line != point.line && !self.is_block))
            && (self.end.column >= point.column || (self.end.line != point.line && !self.is_block))
    }
}

/// Damage is relative to the preceding snapshot taken from this session.
/// Shared rows remain unchanged by future parser activity; new consumers may
/// simply use every row irrespective of the damage hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewportDamage {
    Full,
    Rows(Vec<usize>),
}
#[derive(Debug, Clone)]
pub struct ViewportSnapshot {
    pub revision: u64,
    pub columns: usize,
    pub screen_lines: usize,
    pub display_offset: usize,
    pub rows: Vec<Arc<[Cell]>>,
    pub cursor: Cursor,
    pub mode: Mode,
    pub selection: Option<SelectionRange>,
    pub colors: Colors,
    pub damage: ViewportDamage,
}
impl ViewportSnapshot {
    pub fn blank(columns: usize, screen_lines: usize) -> Self {
        let row: Arc<[Cell]> = (0..columns)
            .map(|column| Cell {
                column,
                ..Cell::default()
            })
            .collect();
        Self {
            revision: 0,
            columns,
            screen_lines,
            display_offset: 0,
            rows: vec![row; screen_lines],
            cursor: Cursor::default(),
            mode: Mode::SHOW_CURSOR,
            selection: None,
            colors: [None; 269],
            damage: ViewportDamage::Full,
        }
    }
    pub fn text(&self) -> String {
        let mut text = String::new();
        for row in &self.rows {
            let start = text.len();
            for cell in row.iter().filter(|cell| {
                !cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            }) {
                text.push(cell.c);
                text.extend(&cell.extra);
            }
            while text.len() > start && text.ends_with(' ') {
                text.pop();
            }
            if !row
                .last()
                .is_some_and(|cell| cell.flags.contains(Flags::WRAPLINE))
            {
                text.push('\n');
            }
        }
        text
    }
}

/// Supported terminal events are bounded to 64 pending entries and one MiB of
/// payload; any individual event above 64 KiB is discarded. On overflow the
/// oldest notification is dropped and metrics record the drop. Clipboard
/// reads remain disabled; the desktop runtime explicitly owns copy delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    ClipboardStore {
        selection: bool,
        text: String,
    },
    Notification(Notification),
    CloseNotification {
        id: String,
    },
    /// A line addressed to the hosting application by an integration it
    /// installed (`OSC 7717 ; neptune ; line`): at most 512 printable ASCII
    /// bytes. What it means, and whether to believe it, is the application's.
    Report(String),
}

/// An explicit terminal attention request. BEL uses a generic title; OSC text
/// comes from the program, never from inspecting ordinary output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notification {
    pub id: Option<String>,
    pub title: String,
    pub body: String,
    pub occasion: NotificationOccasion,
}

/// OSC 99's optional visibility condition.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NotificationOccasion {
    #[default]
    Always,
    Unfocused,
    Invisible,
}

impl TerminalEvent {
    pub(crate) fn payload_len(&self) -> usize {
        match self {
            Self::ClipboardStore { text, .. } => text.len(),
            Self::Notification(n) => {
                n.title.len() + n.body.len() + n.id.as_ref().map_or(0, String::len)
            }
            Self::CloseNotification { id } => id.len(),
            Self::Report(line) => line.len(),
        }
    }
}
