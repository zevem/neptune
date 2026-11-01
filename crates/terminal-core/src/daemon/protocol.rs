//! Session daemon protocol, version 1. Every frame is a little-endian `u32`
//! length, then a kind byte and its payload. A connection opens with a hello;
//! the daemon refuses any version but its own. Strings are UTF-8 and byte
//! strings are length-prefixed. Released encodings never change: a new
//! encoding needs a new protocol version and socket name.
use super::*;
use crate::view;
use std::os::unix::ffi::{OsStrExt, OsStringExt};

pub(super) const MAGIC: &[u8; 16] = b"NEPTUNE-SESSIONS";
/// Frames a client may send: input is at most one MiB.
pub(super) const MAX_CLIENT_FRAME: usize = 2 * 1024 * 1024;
/// Frames the daemon may send, bounded by a snapshot of the largest grid.
pub(super) const MAX_DAEMON_FRAME: usize = 256 * 1024 * 1024;
pub(super) const MAX_DATA: usize = 64 * 1024;

/// What a client sends.
#[derive(Debug, Clone)]
pub(super) enum Request {
    Hello {
        version: u32,
    },
    List,
    SetData {
        id: u64,
        data: Vec<u8>,
    },
    Close {
        id: u64,
    },
    /// Start a session; the connection then streams it.
    Start(SessionOptions),
    /// Attach to a session at this size; the connection then streams it.
    Attach {
        id: u64,
        cols: u16,
        rows: u16,
    },
    /// Become the daemon's client in place of any earlier one, which is told
    /// and let go of. The connection then stays open while this client holds
    /// the daemon.
    TakeOver,
    // Sent only on a session's stream.
    Input(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    },
    Focus(bool),
    /// Close the streamed session.
    End,
}

/// What the daemon sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Reply {
    Welcome {
        version: u32,
    },
    Refused {
        version: u32,
    },
    Sessions(Vec<Entry>),
    Done,
    Failed {
        kind: SessionErrorKind,
        detail: String,
    },
    Attached(Box<Attached>),
    Output(Vec<u8>),
    /// The streamed session ended; `None` when it failed without an exit status.
    Ended(Option<Exit>),
    /// Sent on a client's takeover connection: a newer client took over.
    Replaced,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) id: u64,
    pub(super) exit: Option<Exit>,
    pub(super) data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Attached {
    pub(super) id: u64,
    pub(super) process_id: Option<u32>,
    pub(super) shell: String,
    pub(super) cwd: PathBuf,
    pub(super) scrollback: u32,
    pub(super) data: Vec<u8>,
    pub(super) snapshot: Snapshot,
}

mod kind {
    pub(super) const HELLO: u8 = 1;
    pub(super) const LIST: u8 = 2;
    pub(super) const SET_DATA: u8 = 3;
    pub(super) const CLOSE: u8 = 4;
    pub(super) const START: u8 = 5;
    pub(super) const ATTACH: u8 = 6;
    pub(super) const TAKE_OVER: u8 = 7;
    pub(super) const INPUT: u8 = 16;
    pub(super) const RESIZE: u8 = 17;
    pub(super) const FOCUS: u8 = 18;
    pub(super) const END: u8 = 19;
    pub(super) const WELCOME: u8 = 64;
    pub(super) const REFUSED: u8 = 65;
    pub(super) const SESSIONS: u8 = 66;
    pub(super) const DONE: u8 = 67;
    pub(super) const FAILED: u8 = 68;
    pub(super) const ATTACHED: u8 = 69;
    pub(super) const OUTPUT: u8 = 70;
    pub(super) const ENDED: u8 = 71;
    pub(super) const REPLACED: u8 = 72;
}

#[derive(Default)]
struct Encoder(Vec<u8>);

impl Encoder {
    fn frame(kind: u8) -> Self {
        Self(vec![0, 0, 0, 0, kind])
    }
    fn u8(&mut self, value: u8) -> &mut Self {
        self.0.push(value);
        self
    }
    fn u16(&mut self, value: u16) -> &mut Self {
        self.0.extend(value.to_le_bytes());
        self
    }
    fn u32(&mut self, value: u32) -> &mut Self {
        self.0.extend(value.to_le_bytes());
        self
    }
    fn u64(&mut self, value: u64) -> &mut Self {
        self.0.extend(value.to_le_bytes());
        self
    }
    fn bytes(&mut self, value: &[u8]) -> &mut Self {
        self.u32(value.len() as u32);
        self.0.extend(value);
        self
    }
    fn str(&mut self, value: &str) -> &mut Self {
        self.bytes(value.as_bytes())
    }
    fn exit(&mut self, exit: &Option<Exit>) -> &mut Self {
        match exit {
            None => self.u8(0),
            Some(exit) => {
                self.u8(1).u32(exit.code);
                match &exit.signal {
                    None => self.u8(0),
                    Some(signal) => self.u8(1).str(signal),
                }
            }
        }
    }
    fn color(&mut self, color: view::Color) -> &mut Self {
        match color {
            view::Color::Named(named) => self.u8(0).u16(named as usize as u16),
            view::Color::Indexed(index) => self.u8(1).u8(index),
            view::Color::Spec(rgb) => self.u8(2).u8(rgb.r).u8(rgb.g).u8(rgb.b),
        }
    }
    fn cursor(&mut self, cursor: &snapshot::Cursor) -> &mut Self {
        self.u16(cursor.line)
            .u16(cursor.column)
            .u8(cursor.pending_wrap as u8)
            .u32(cursor.style)
    }
    fn screen(&mut self, screen: &snapshot::Screen) -> &mut Self {
        self.u32(screen.keyboard.bits())
            .cursor(&screen.cursor)
            .cursor(&screen.saved_cursor)
            .u32(screen.lines.len() as u32);
        for row in &screen.lines {
            self.u32(row.runs.len() as u32);
            for run in &row.runs {
                self.u32(run.style).str(&run.text);
            }
            self.u32(row.combining.len() as u32);
            for (column, text) in &row.combining {
                self.u16(*column).str(text);
            }
        }
        self
    }
    /// A snapshot's encoding in version 1.
    fn snapshot(&mut self, snapshot: &Snapshot) -> &mut Self {
        self.u16(snapshot.cols)
            .u16(snapshot.rows)
            .u32(snapshot.hyperlinks.len() as u32);
        for link in &snapshot.hyperlinks {
            self.str(&link.id).str(&link.uri);
        }
        self.u32(snapshot.styles.len() as u32);
        for style in &snapshot.styles {
            self.color(style.fg).color(style.bg);
            match style.underline {
                None => self.u8(0),
                Some(color) => self.u8(1).color(color),
            };
            self.u16(style.flags.bits());
            match style.hyperlink {
                None => self.u8(0),
                Some(index) => self.u8(1).u32(index),
            };
        }
        self.screen(&snapshot.primary);
        match &snapshot.alternate {
            None => self.u8(0),
            Some(screen) => self.u8(1).screen(screen),
        };
        let shape = CURSOR_SHAPES
            .iter()
            .position(|shape| *shape == snapshot.cursor_style.shape)
            .unwrap_or_default() as u8;
        self.u32(snapshot.modes.bits())
            .u8(shape)
            .u8(snapshot.cursor_style.blinking as u8)
            .u32(snapshot.colors.len() as u32);
        for (index, rgb) in &snapshot.colors {
            self.u16(*index).u8(rgb.r).u8(rgb.g).u8(rgb.b);
        }
        self.str(&snapshot.title);
        match &snapshot.reported_cwd {
            None => self.u8(0),
            Some(path) => self.u8(1).bytes(path.as_os_str().as_bytes()),
        };
        match snapshot.remote_process_id {
            None => self.u8(0),
            Some(pid) => self.u8(1).u32(pid),
        };
        match snapshot.prompt {
            None => self.u8(0),
            Some(prompt) => self.u8(1).u32(prompt.line as u32).u16(prompt.column),
        };
        self.u8(snapshot.prompt_redraw as u8)
            .bytes(&snapshot.pending)
    }
    fn finish(mut self) -> Vec<u8> {
        let len = (self.0.len() - 4) as u32;
        self.0[..4].copy_from_slice(&len.to_le_bytes());
        self.0
    }
}

struct Decoder<'a>(&'a [u8]);

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Malformed session daemon frame")
}

impl<'a> Decoder<'a> {
    fn take(&mut self, len: usize) -> io::Result<&'a [u8]> {
        if self.0.len() < len {
            return Err(invalid());
        }
        let (head, tail) = self.0.split_at(len);
        self.0 = tail;
        Ok(head)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn bool(&mut self) -> io::Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid()),
        }
    }
    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> io::Result<Vec<u8>> {
        let len = self.u32()? as usize;
        Ok(self.take(len)?.to_vec())
    }
    fn string(&mut self) -> io::Result<String> {
        String::from_utf8(self.bytes()?).map_err(|_| invalid())
    }
    fn count(&mut self) -> io::Result<usize> {
        let count = self.u32()? as usize;
        // Every element takes at least four bytes.
        if count > self.0.len() / 4 {
            return Err(invalid());
        }
        Ok(count)
    }
    fn exit(&mut self) -> io::Result<Option<Exit>> {
        Ok(match self.bool()? {
            false => None,
            true => Some(Exit {
                code: self.u32()?,
                signal: match self.bool()? {
                    false => None,
                    true => Some(self.string()?),
                },
            }),
        })
    }
    fn color(&mut self) -> io::Result<view::Color> {
        Ok(match self.u8()? {
            0 => {
                let code = usize::from(self.u16()?);
                view::Color::Named(
                    *NAMED_COLORS
                        .iter()
                        .find(|named| **named as usize == code)
                        .ok_or_else(invalid)?,
                )
            }
            1 => view::Color::Indexed(self.u8()?),
            2 => view::Color::Spec(view::Rgb {
                r: self.u8()?,
                g: self.u8()?,
                b: self.u8()?,
            }),
            _ => return Err(invalid()),
        })
    }
    fn cursor(&mut self) -> io::Result<snapshot::Cursor> {
        Ok(snapshot::Cursor {
            line: self.u16()?,
            column: self.u16()?,
            pending_wrap: self.bool()?,
            style: self.u32()?,
        })
    }
    fn screen(&mut self) -> io::Result<snapshot::Screen> {
        let keyboard =
            view::Mode::from_bits_retain(self.u32()?) & view::Mode::KITTY_KEYBOARD_PROTOCOL;
        let cursor = self.cursor()?;
        let saved_cursor = self.cursor()?;
        let lines = (0..self.count()?)
            .map(|_| {
                let runs = (0..self.count()?)
                    .map(|_| {
                        Ok(snapshot::Run {
                            style: self.u32()?,
                            text: self.string()?,
                        })
                    })
                    .collect::<io::Result<_>>()?;
                let combining = (0..self.count()?)
                    .map(|_| Ok((self.u16()?, self.string()?)))
                    .collect::<io::Result<_>>()?;
                Ok(snapshot::Row { runs, combining })
            })
            .collect::<io::Result<_>>()?;
        Ok(snapshot::Screen {
            lines,
            cursor,
            saved_cursor,
            keyboard,
        })
    }
    fn snapshot(&mut self) -> io::Result<Snapshot> {
        let cols = self.u16()?;
        let rows = self.u16()?;
        let hyperlinks = (0..self.count()?)
            .map(|_| {
                Ok(snapshot::Hyperlink {
                    id: self.string()?,
                    uri: self.string()?,
                })
            })
            .collect::<io::Result<_>>()?;
        let styles = (0..self.count()?)
            .map(|_| {
                Ok(snapshot::Style {
                    fg: self.color()?,
                    bg: self.color()?,
                    underline: match self.bool()? {
                        false => None,
                        true => Some(self.color()?),
                    },
                    flags: view::Flags::from_bits_retain(self.u16()?),
                    hyperlink: match self.bool()? {
                        false => None,
                        true => Some(self.u32()?),
                    },
                })
            })
            .collect::<io::Result<_>>()?;
        let primary = self.screen()?;
        let alternate = match self.bool()? {
            false => None,
            true => Some(self.screen()?),
        };
        let modes = view::Mode::from_bits_retain(self.u32()?);
        let cursor_style = snapshot::CursorStyle {
            shape: *CURSOR_SHAPES
                .get(usize::from(self.u8()?))
                .ok_or_else(invalid)?,
            blinking: self.bool()?,
        };
        let colors = (0..self.count()?)
            .map(|_| {
                Ok((
                    self.u16()?,
                    view::Rgb {
                        r: self.u8()?,
                        g: self.u8()?,
                        b: self.u8()?,
                    },
                ))
            })
            .collect::<io::Result<_>>()?;
        let title = self.string()?;
        let reported_cwd = match self.bool()? {
            false => None,
            true => Some(PathBuf::from(std::ffi::OsString::from_vec(self.bytes()?))),
        };
        let remote_process_id = match self.bool()? {
            false => None,
            true => Some(self.u32()?),
        };
        let prompt = match self.bool()? {
            false => None,
            true => Some(snapshot::Prompt {
                line: self.u32()? as i32,
                column: self.u16()?,
            }),
        };
        let snapshot = Snapshot {
            cols,
            rows,
            styles,
            hyperlinks,
            primary,
            alternate,
            modes,
            cursor_style,
            colors,
            title,
            reported_cwd,
            remote_process_id,
            prompt,
            prompt_redraw: self.bool()?,
            pending: self.bytes()?,
        };
        valid(&snapshot).then_some(snapshot).ok_or_else(invalid)
    }
    fn end(self) -> io::Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(invalid())
        }
    }
}

const NAMED_COLORS: [view::NamedColor; 29] = {
    use view::NamedColor::*;
    [
        Black,
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
        Foreground,
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
    ]
};

/// Cursor shapes by their code.
const CURSOR_SHAPES: [view::CursorShape; 5] = [
    view::CursorShape::Block,
    view::CursorShape::Underline,
    view::CursorShape::Beam,
    view::CursorShape::HollowBlock,
    view::CursorShape::Hidden,
];

/// Whether a decoded snapshot describes a grid a replica can be built from:
/// every reference resolves and every cell and cursor lies inside it.
fn valid(snapshot: &Snapshot) -> bool {
    let (cols, rows) = (usize::from(snapshot.cols), usize::from(snapshot.rows));
    let styles = snapshot.styles.len() as u32;
    let cursor = |cursor: &snapshot::Cursor| {
        usize::from(cursor.line) < rows
            && usize::from(cursor.column) < cols
            && cursor.style < styles
    };
    let screen = |screen: &snapshot::Screen| {
        cursor(&screen.cursor)
            && cursor(&screen.saved_cursor)
            && screen.lines.iter().all(|row| {
                row.runs.iter().all(|run| run.style < styles)
                    && row
                        .runs
                        .iter()
                        .map(|run| run.text.chars().count())
                        .sum::<usize>()
                        <= cols
                    && row
                        .combining
                        .iter()
                        .all(|(column, _)| usize::from(*column) < cols)
            })
    };
    (1..=4096).contains(&cols)
        && (1..=4096).contains(&rows)
        && snapshot.primary.lines.len() >= rows
        && screen(&snapshot.primary)
        && snapshot
            .alternate
            .as_ref()
            .is_none_or(|alternate| alternate.lines.len() == rows && screen(alternate))
        && snapshot.styles.iter().all(|style| {
            style
                .hyperlink
                .is_none_or(|index| (index as usize) < snapshot.hyperlinks.len())
        })
        && snapshot.colors.iter().all(|(index, _)| *index < 269)
}

/// The snapshot part of an attach reply, for its contract tests.
#[cfg(test)]
pub(in crate::session) fn encode_snapshot(snapshot: &Snapshot) -> Vec<u8> {
    let mut encoder = Encoder::default();
    encoder.snapshot(snapshot);
    encoder.0
}

#[cfg(test)]
pub(in crate::session) fn decode_snapshot(bytes: &[u8]) -> io::Result<Snapshot> {
    let mut decoder = Decoder(bytes);
    let snapshot = decoder.snapshot()?;
    decoder.end()?;
    Ok(snapshot)
}

fn error_kind_code(kind: SessionErrorKind) -> u8 {
    match kind {
        SessionErrorKind::Backpressure => 1,
        SessionErrorKind::Closed => 2,
        SessionErrorKind::InvalidGeometry => 3,
        SessionErrorKind::InvalidOptions => 4,
        SessionErrorKind::Spawn => 5,
        SessionErrorKind::InvalidSearch => 6,
        SessionErrorKind::Unavailable => 7,
        SessionErrorKind::Incompatible => 8,
        SessionErrorKind::NotFound => 9,
    }
}

fn error_kind(code: u8) -> io::Result<SessionErrorKind> {
    Ok(match code {
        1 => SessionErrorKind::Backpressure,
        2 => SessionErrorKind::Closed,
        3 => SessionErrorKind::InvalidGeometry,
        4 => SessionErrorKind::InvalidOptions,
        5 => SessionErrorKind::Spawn,
        6 => SessionErrorKind::InvalidSearch,
        7 => SessionErrorKind::Unavailable,
        8 => SessionErrorKind::Incompatible,
        9 => SessionErrorKind::NotFound,
        _ => return Err(invalid()),
    })
}

impl Request {
    pub(super) fn encode(&self) -> Vec<u8> {
        let mut frame;
        match self {
            Request::Hello { version } => {
                frame = Encoder::frame(kind::HELLO);
                frame.0.extend(MAGIC);
                frame.u32(*version);
            }
            Request::List => frame = Encoder::frame(kind::LIST),
            Request::TakeOver => frame = Encoder::frame(kind::TAKE_OVER),
            Request::SetData { id, data } => {
                frame = Encoder::frame(kind::SET_DATA);
                frame.u64(*id).bytes(data);
            }
            Request::Close { id } => {
                frame = Encoder::frame(kind::CLOSE);
                frame.u64(*id);
            }
            Request::Start(options) => {
                frame = Encoder::frame(kind::START);
                frame
                    .bytes(options.cwd.as_os_str().as_bytes())
                    .u16(options.cols)
                    .u16(options.rows)
                    .u32(options.scrollback.min(u32::MAX as usize) as u32);
                match &options.shell {
                    None => frame.u8(0),
                    Some(shell) => frame.u8(1).str(shell),
                };
                frame.u32(options.args.len() as u32);
                for arg in &options.args {
                    frame.str(arg);
                }
                frame.u32(options.env.len() as u32);
                for (key, value) in &options.env {
                    frame.str(key).str(value);
                }
            }
            Request::Attach { id, cols, rows } => {
                frame = Encoder::frame(kind::ATTACH);
                frame.u64(*id).u16(*cols).u16(*rows);
            }
            Request::Input(bytes) => {
                frame = Encoder::frame(kind::INPUT);
                frame.bytes(bytes);
            }
            Request::Resize {
                cols,
                rows,
                pixel_width,
                pixel_height,
            } => {
                frame = Encoder::frame(kind::RESIZE);
                frame
                    .u16(*cols)
                    .u16(*rows)
                    .u16(*pixel_width)
                    .u16(*pixel_height);
            }
            Request::Focus(focused) => {
                frame = Encoder::frame(kind::FOCUS);
                frame.u8(*focused as u8);
            }
            Request::End => frame = Encoder::frame(kind::END),
        }
        frame.finish()
    }

    pub(super) fn decode(frame: &[u8]) -> io::Result<Self> {
        let (&kind, payload) = frame.split_first().ok_or_else(invalid)?;
        let mut data = Decoder(payload);
        let request = match kind {
            kind::HELLO => {
                if data.take(MAGIC.len())? != MAGIC {
                    return Err(invalid());
                }
                Request::Hello {
                    version: data.u32()?,
                }
            }
            kind::LIST => Request::List,
            kind::TAKE_OVER => Request::TakeOver,
            kind::SET_DATA => Request::SetData {
                id: data.u64()?,
                data: data.bytes()?,
            },
            kind::CLOSE => Request::Close { id: data.u64()? },
            kind::START => {
                let cwd = PathBuf::from(std::ffi::OsString::from_vec(data.bytes()?));
                let cols = data.u16()?;
                let rows = data.u16()?;
                let scrollback = data.u32()? as usize;
                let shell = match data.bool()? {
                    false => None,
                    true => Some(data.string()?),
                };
                let args = (0..data.count()?)
                    .map(|_| data.string())
                    .collect::<io::Result<_>>()?;
                let env = (0..data.count()?)
                    .map(|_| Ok((data.string()?, data.string()?)))
                    .collect::<io::Result<_>>()?;
                Request::Start(SessionOptions {
                    cwd,
                    cols,
                    rows,
                    scrollback,
                    shell,
                    args,
                    env,
                })
            }
            kind::ATTACH => Request::Attach {
                id: data.u64()?,
                cols: data.u16()?,
                rows: data.u16()?,
            },
            kind::INPUT => Request::Input(data.bytes()?),
            kind::RESIZE => Request::Resize {
                cols: data.u16()?,
                rows: data.u16()?,
                pixel_width: data.u16()?,
                pixel_height: data.u16()?,
            },
            kind::FOCUS => Request::Focus(data.bool()?),
            kind::END => Request::End,
            _ => return Err(invalid()),
        };
        data.end()?;
        Ok(request)
    }
}

impl Reply {
    pub(super) fn encode(&self) -> Vec<u8> {
        let mut frame;
        match self {
            Reply::Welcome { version } => {
                frame = Encoder::frame(kind::WELCOME);
                frame.u32(*version);
            }
            Reply::Refused { version } => {
                frame = Encoder::frame(kind::REFUSED);
                frame.u32(*version);
            }
            Reply::Sessions(entries) => {
                frame = Encoder::frame(kind::SESSIONS);
                frame.u32(entries.len() as u32);
                for entry in entries {
                    frame.u64(entry.id).exit(&entry.exit).bytes(&entry.data);
                }
            }
            Reply::Done => frame = Encoder::frame(kind::DONE),
            Reply::Replaced => frame = Encoder::frame(kind::REPLACED),
            Reply::Failed { kind, detail } => {
                frame = Encoder::frame(kind::FAILED);
                frame.u8(error_kind_code(*kind)).str(detail);
            }
            Reply::Attached(attached) => {
                frame = Encoder::frame(kind::ATTACHED);
                frame.u64(attached.id);
                match attached.process_id {
                    None => frame.u8(0),
                    Some(pid) => frame.u8(1).u32(pid),
                };
                frame
                    .str(&attached.shell)
                    .bytes(attached.cwd.as_os_str().as_bytes())
                    .u32(attached.scrollback)
                    .bytes(&attached.data);
                frame.snapshot(&attached.snapshot);
            }
            Reply::Output(bytes) => {
                frame = Encoder::frame(kind::OUTPUT);
                frame.bytes(bytes);
            }
            Reply::Ended(exit) => {
                frame = Encoder::frame(kind::ENDED);
                frame.exit(exit);
            }
        }
        frame.finish()
    }

    pub(super) fn decode(frame: &[u8]) -> io::Result<Self> {
        let (&kind, payload) = frame.split_first().ok_or_else(invalid)?;
        let mut data = Decoder(payload);
        let reply = match kind {
            kind::WELCOME => Reply::Welcome {
                version: data.u32()?,
            },
            kind::REFUSED => Reply::Refused {
                version: data.u32()?,
            },
            kind::SESSIONS => {
                let entries = (0..data.count()?)
                    .map(|_| {
                        Ok(Entry {
                            id: data.u64()?,
                            exit: data.exit()?,
                            data: data.bytes()?,
                        })
                    })
                    .collect::<io::Result<_>>()?;
                Reply::Sessions(entries)
            }
            kind::DONE => Reply::Done,
            kind::REPLACED => Reply::Replaced,
            kind::FAILED => Reply::Failed {
                kind: error_kind(data.u8()?)?,
                detail: data.string()?,
            },
            kind::ATTACHED => {
                let id = data.u64()?;
                let process_id = match data.bool()? {
                    false => None,
                    true => Some(data.u32()?),
                };
                let shell = data.string()?;
                let cwd = PathBuf::from(std::ffi::OsString::from_vec(data.bytes()?));
                let scrollback = data.u32()?;
                let session_data = data.bytes()?;
                Reply::Attached(Box::new(Attached {
                    id,
                    process_id,
                    shell,
                    cwd,
                    scrollback,
                    data: session_data,
                    snapshot: data.snapshot()?,
                }))
            }
            kind::OUTPUT => Reply::Output(data.bytes()?),
            kind::ENDED => Reply::Ended(data.exit()?),
            _ => return Err(invalid()),
        };
        data.end()?;
        Ok(reply)
    }
}

/// Reads whole frames from a blocking stream.
pub(super) fn read_frame(stream: &mut impl Read, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0; 4];
    match stream.read_exact(&mut len) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > limit {
        return Err(invalid());
    }
    let mut frame = vec![0; len];
    stream.read_exact(&mut frame)?;
    Ok(Some(frame))
}

/// Splits frames out of bytes read from a nonblocking stream.
#[derive(Default)]
pub(super) struct FrameBuffer {
    bytes: Vec<u8>,
    start: usize,
}

impl FrameBuffer {
    pub(super) fn extend(&mut self, bytes: &[u8]) {
        if self.start > 0 && self.start == self.bytes.len() {
            self.bytes.clear();
            self.start = 0;
        }
        self.bytes.extend_from_slice(bytes);
    }

    pub(super) fn next(&mut self, limit: usize) -> io::Result<Option<Vec<u8>>> {
        let available = &self.bytes[self.start..];
        if available.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_le_bytes(available[..4].try_into().unwrap()) as usize;
        if len == 0 || len > limit {
            return Err(invalid());
        }
        if available.len() < 4 + len {
            return Ok(None);
        }
        let frame = available[4..4 + len].to_vec();
        self.start += 4 + len;
        if self.start > 64 * 1024 && self.start * 2 > self.bytes.len() {
            self.bytes.drain(..self.start);
            self.start = 0;
        }
        Ok(Some(frame))
    }
}
