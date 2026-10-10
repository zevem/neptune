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
pub const DEFAULT_FRAME_RATE: u32 = 60;
pub const MAX_FRAME_RATE: u32 = 240;

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
}

pub fn frame_len(width: u32, height: u32) -> Option<usize> {
    let pixels = (width as usize).checked_mul(height as usize)?;
    (width > 0 && height > 0 && width <= MAX_SIDE && height <= MAX_SIDE && pixels <= MAX_PIXELS)
        .then(|| pixels * 4)
}
