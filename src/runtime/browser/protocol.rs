//! Private, bounded wire contract between Neptune and its browser child.
//! This file is also compiled by the isolated browser executable.
use serde::{Deserialize, Serialize};

pub const MAX_BROWSERS: usize = 8;
pub const MAX_SIDE: u32 = 4096;
pub const MAX_PIXELS: usize = 4 * 1024 * 1024;
pub const MAX_TEXT: usize = 64 * 1024;
// JSON can escape each input byte into six bytes; leave room for its envelope.
pub const MAX_MESSAGE: usize = 512 * 1024;
pub const MAX_URL: usize = 8192;
/// The longest side of a page's icon, in pixels.
pub const MAX_ICON: u32 = 64;
pub const DEFAULT_FRAME_RATE: u32 = 60;
pub const MAX_FRAME_RATE: u32 = 240;
/// A picked element's description, as the page script bounds each part of it.
pub const MAX_PICK: usize = 16 * 1024;
/// Cookies one command carries; an import is sent as several.
pub const MAX_COOKIES: usize = 64;

/// The directory of a saved profile under the browser root.
pub fn profile_directory(id: &str) -> String {
    format!("profile.{id}")
}

/// A saved profile's id: `default` or one Neptune generated.
pub fn valid_profile(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Physical pixels in an already validated, bounded browser surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Damage {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
impl Damage {
    pub fn full(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }
    pub fn valid(self, width: u32, height: u32) -> bool {
        self.width > 0
            && self.height > 0
            && self
                .x
                .checked_add(self.width)
                .is_some_and(|end| end <= width)
            && self
                .y
                .checked_add(self.height)
                .is_some_and(|end| end <= height)
    }
    pub fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self {
            x,
            y,
            width: (self.x + self.width).max(other.x + other.width) - x,
            height: (self.y + self.height).max(other.y + other.height) - y,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Target {
    pub pane: u64,
    pub generation: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    Open {
        target: Target,
        /// The saved profile whose cookies and storage this browser uses, or
        /// none for one kept in memory and gone with the browser.
        #[serde(default)]
        profile: Option<String>,
    },
    /// Start picking an element in the page, or cancel a pick under way.
    Pick {
        target: Target,
        active: bool,
    },
    /// Record the page to this WebM file, or with no path end the recording.
    Record {
        target: Target,
        path: Option<String>,
    },
    /// Cookies read from another browser, for a saved profile. `last` ends
    /// the import and asks for its count.
    SetCookies {
        profile: String,
        cookies: Vec<Cookie>,
        last: bool,
    },
    ClearCookies {
        profile: String,
    },
    Close {
        target: Target,
    },
    Navigate {
        target: Target,
        url: String,
    },
    Back {
        target: Target,
    },
    Forward {
        target: Target,
    },
    Reload {
        target: Target,
    },
    Stop {
        target: Target,
    },
    DevTools {
        target: Target,
    },
    Find {
        target: Target,
        text: String,
        reverse: bool,
        next: bool,
    },
    StopFind {
        target: Target,
    },
    /// The pane in physical pixels, and the pixels of one logical point.
    /// Pointer positions and wheel deltas stay in logical points.
    Resize {
        target: Target,
        width: u32,
        height: u32,
        scale: f32,
    },
    FrameRate {
        target: Target,
        frames_per_second: u32,
    },
    Visible {
        target: Target,
        visible: bool,
    },
    Focus {
        target: Target,
        focused: bool,
    },
    Key {
        target: Target,
        code: i32,
        native: i32,
        pressed: bool,
        modifiers: u32,
    },
    Text {
        target: Target,
        text: String,
    },
    Ime {
        target: Target,
        text: String,
        commit: bool,
    },
    Mouse {
        target: Target,
        x: i32,
        y: i32,
        modifiers: u32,
        kind: Mouse,
    },
    Copy {
        target: Target,
    },
    Cut {
        target: Target,
    },
    Paste {
        target: Target,
        /// Desktop clipboard text for the size guard. CEF performs native
        /// paste so page handlers and clipboard formats retain browser behavior.
        text: String,
    },
    Shutdown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Mouse {
    Move {
        leave: bool,
    },
    Button {
        button: u8,
        pressed: bool,
        clicks: i32,
    },
    Wheel {
        dx: i32,
        dy: i32,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cookie {
    /// As the other browser stored it: a leading dot marks a domain cookie.
    pub host: String,
    pub name: String,
    pub value: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    /// 0 none, 1 lax, 2 strict; anything else leaves it to the browser.
    pub same_site: i8,
    /// Unix seconds, or none for a cookie that ends with the session.
    pub expires: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct State {
    pub title: String,
    pub url: String,
    pub loading: bool,
    pub back: bool,
    pub forward: bool,
    pub error: Option<String>,
    pub ready: bool,
    pub closed: bool,
    #[serde(default)]
    pub caret: Option<[i32; 4]>,
    /// The page waits for the user to click an element.
    #[serde(default)]
    pub picking: bool,
    #[serde(default)]
    pub recording: bool,
    /// The pointer the page asks for where it is: a `cef_cursor_type_t`.
    #[serde(default)]
    pub cursor: u32,
    /// What the element under the pointer says about itself.
    #[serde(default)]
    pub tooltip: Option<String>,
    /// Where the link under the pointer leads.
    #[serde(default)]
    pub link: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    State {
        target: Target,
        state: State,
    },
    /// Followed by tightly packed BGRA pixels for `damage`, or the entire
    /// surface when absent. Surface dimensions retain their resource bounds.
    Frame {
        target: Target,
        width: u32,
        height: u32,
        popup: bool,
        x: i32,
        y: i32,
        #[serde(default)]
        damage: Option<Damage>,
    },
    /// The element the user clicked, as JSON the page script wrote. A page
    /// can say anything here: it is text for the user, never a command.
    Picked {
        target: Target,
        pick: String,
    },
    /// A recording ended: the bytes in its file, or why there is none.
    Recorded {
        target: Target,
        bytes: u64,
        error: Option<String>,
    },
    Imported {
        imported: u32,
        skipped: u32,
    },
    /// The page's icon, followed by its tightly packed RGBA pixels with
    /// alpha premultiplied; no size when the page has none.
    Icon {
        target: Target,
        width: u32,
        height: u32,
    },
}

pub fn frame_len(width: u32, height: u32) -> Option<usize> {
    let pixels = (width as usize).checked_mul(height as usize)?;
    (width > 0 && height > 0 && width <= MAX_SIDE && height <= MAX_SIDE && pixels <= MAX_PIXELS)
        .then(|| pixels * 4)
}
