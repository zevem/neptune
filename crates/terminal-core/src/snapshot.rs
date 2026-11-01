//! A terminal's state as terminal-core describes it, independent of the
//! backend's own serialization, so a replica engine can continue from it.
//!
//! Today it carries the scrollback and visible screen as text, oldest line
//! first, and the cursor. Styles, wrapped lines, modes, the alternate screen,
//! title and directory are not carried yet.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub(super) cols: u16,
    pub(super) rows: u16,
    /// Scrollback then screen, one entry per grid line, without trailing blanks.
    pub(super) lines: Vec<String>,
    /// Screen-relative cursor line and column.
    pub(super) cursor: (u16, u16),
}

impl Snapshot {
    #[cfg(unix)]
    pub(super) fn capture<T: EventListener>(terminal: &Term<T>) -> Self {
        let grid = terminal.grid();
        let lines = (terminal.topmost_line().0..=terminal.bottommost_line().0)
            .map(|line| {
                let row = &grid[Line(line)];
                let mut text = String::new();
                for column in 0..terminal.columns() {
                    let cell = &row[Column(column)];
                    if cell.flags.intersects(
                        alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER
                            | alacritty_terminal::term::cell::Flags::LEADING_WIDE_CHAR_SPACER,
                    ) {
                        continue;
                    }
                    text.push(cell.c);
                    text.extend(cell.zerowidth().unwrap_or_default());
                }
                text.truncate(text.trim_end_matches(' ').len());
                text
            })
            .collect();
        let cursor = grid.cursor.point;
        Self {
            cols: terminal.columns() as u16,
            rows: terminal.screen_lines() as u16,
            lines,
            cursor: (cursor.line.0.max(0) as u16, cursor.column.0 as u16),
        }
    }

    /// Rebuild a fresh terminal of the snapshot's size. Lines beyond the
    /// screen scroll into its history.
    pub(super) fn restore<T: EventListener>(&self, terminal: &mut Term<T>) {
        let mut bytes = self.lines.join("\r\n").into_bytes();
        bytes.extend(format!("\x1b[{};{}H", self.cursor.0 + 1, self.cursor.1 + 1).as_bytes());
        let mut processor: Processor = Processor::new();
        processor.advance(terminal, &bytes);
    }
}
