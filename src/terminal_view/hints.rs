//! Bounded keyboard hints over the visible owned viewport. A hinted viewport
//! stays still until selection/cancel; the live terminal continues processing.
use super::{
    Cache,
    links::{LinkTarget, plain_candidate},
};
use crate::{
    platform::editor::path_like,
    theme::{self, Palette},
};
use eframe::egui::{self, Rect};
use terminal_core::Point;

const ALPHABET: &[u8] = b"asdfghjklqwertyuiopzxcvbnm";
const MAX_HINTS: usize = 256;
const MAX_SCAN_CELLS: usize = 65_536;

pub(super) struct Hints {
    targets: Vec<Target>,
    prefix: String,
}

struct Target {
    text: String,
    start: usize,
    end: usize,
    label: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum HintInput {
    Pending,
    Copy(String),
    Cancel,
}

impl Hints {
    pub(super) fn estimated_bytes(&self) -> usize {
        self.targets.capacity() * std::mem::size_of::<Target>()
            + self.prefix.capacity()
            + self
                .targets
                .iter()
                .map(|target| target.text.capacity() + target.label.capacity())
                .sum::<usize>()
    }
}

impl Cache {
    pub fn begin_hints(&mut self) -> bool {
        let columns = usize::from(self.columns);
        if columns == 0 {
            return false;
        }
        let limit = (self.sources.len() * columns).min(MAX_SCAN_CELLS);
        let mut targets = Vec::new();
        let mut index = 0;
        while index < limit && targets.len() < MAX_HINTS {
            let point = Point::new(
                (index / columns) as i32 - self.display_offset as i32,
                index % columns,
            );
            if let Some(cell) = self.source_cell(index)
                && cell.hyperlink.is_some()
                && let Some(link) = self.link_at(point)
            {
                let text = match link.target {
                    LinkTarget::Web(url) => url.as_str().to_owned(),
                    LinkTarget::File(file) => file.path,
                };
                targets.push(Target {
                    text,
                    start: link.start,
                    end: link.end,
                    label: String::new(),
                });
                index = link.end + 1;
                continue;
            }
            let Some((text, positions, end)) = self.token_at(index) else {
                let rejected_token = self
                    .source_cell(index)
                    .is_some_and(|c| !super::links::delimiter(c));
                index += 1;
                // Skip a rejected overlong/clipped token once, rather than
                // retrying an 8 KiB read at each of its cells.
                while rejected_token
                    && index < limit
                    && self.connected(index - 1, index)
                    && self
                        .source_cell(index)
                        .is_some_and(|c| !super::links::delimiter(c))
                {
                    index += 1;
                }
                continue;
            };
            let (offset, candidate) = plain_candidate(&text);
            let candidate = super::links::trim_url(candidate);
            let hex = (7..=64).contains(&candidate.len())
                && candidate.bytes().all(|b| b.is_ascii_hexdigit());
            // Use the click detector to preserve link boundaries. Ordinary
            // prose needs no quoted-line scan at every word in the viewport.
            let link_byte = text
                .find("https://")
                .or_else(|| text.find("http://"))
                .unwrap_or(offset);
            let link = if text.contains("://")
                || crate::platform::editor::FileLocation::parse(candidate).is_some()
            {
                positions
                    .iter()
                    .find(|(byte, _)| *byte == link_byte)
                    .and_then(|(_, at)| {
                        self.link_at(Point::new(
                            (*at / columns) as i32 - self.display_offset as i32,
                            *at % columns,
                        ))
                    })
            } else {
                None
            };
            if let Some(link) = link {
                let text = match link.target {
                    LinkTarget::Web(url) => url.as_str().to_owned(),
                    // Copy the printed path:line[:column] spelling, useful in
                    // prompts and bug reports as well as plain file paths.
                    LinkTarget::File(_) => {
                        let start = positions
                            .iter()
                            .find(|(_, cell)| *cell == link.start)
                            .map(|(byte, _)| *byte);
                        let end = positions
                            .iter()
                            .find(|(_, cell)| *cell > link.end)
                            .map_or(text.len(), |(byte, _)| *byte);
                        start
                            .and_then(|start| text.get(start..end))
                            .unwrap_or(candidate)
                            .to_owned()
                    }
                };
                targets.push(Target {
                    text,
                    start: link.start,
                    end: link.end,
                    label: String::new(),
                });
            } else if (hex || path_like(candidate) || candidate.starts_with("file://"))
                && let (Some((_, start)), Some((_, end))) = (
                    positions.iter().find(|(byte, _)| *byte == offset),
                    positions
                        .iter()
                        .rev()
                        .find(|(byte, _)| *byte < offset + candidate.len()),
                )
            {
                targets.push(Target {
                    text: candidate.into(),
                    start: *start,
                    end: *end,
                    label: String::new(),
                });
            }
            index = end + 1;
        }
        if targets.is_empty() {
            return false;
        }
        let two = targets.len() > ALPHABET.len();
        for (i, target) in targets.iter_mut().enumerate() {
            if two {
                target.label.push(ALPHABET[i / ALPHABET.len()] as char);
            }
            target.label.push(ALPHABET[i % ALPHABET.len()] as char);
        }
        self.hints = Some(Hints {
            targets,
            prefix: String::new(),
        });
        self.pressed_link = None;
        true
    }

    pub fn hinting(&self) -> bool {
        self.hints.is_some()
    }

    pub fn cancel_hints(&mut self) {
        self.hints = None;
        self.invalidate();
    }

    pub fn hint_text(&mut self, text: &str) -> HintInput {
        let Some(hints) = &mut self.hints else {
            return HintInput::Cancel;
        };
        for c in text.chars() {
            if !c.is_ascii_alphabetic() {
                continue;
            }
            hints.prefix.push(c.to_ascii_lowercase());
            if let Some(target) = hints.targets.iter().find(|t| t.label == hints.prefix) {
                return HintInput::Copy(target.text.clone());
            }
            if !hints
                .targets
                .iter()
                .any(|t| t.label.starts_with(&hints.prefix))
            {
                hints.prefix.clear();
            }
        }
        HintInput::Pending
    }

    pub fn hint_backspace(&mut self) {
        if let Some(hints) = &mut self.hints {
            hints.prefix.pop();
        }
    }

    pub(super) fn paint_hints(&self, ui: &egui::Ui, rect: Rect, p: Palette) {
        let Some(hints) = &self.hints else {
            return;
        };
        let painter = ui.painter().with_clip_rect(rect);
        let columns = usize::from(self.columns);
        for target in hints
            .targets
            .iter()
            .filter(|t| t.label.starts_with(&hints.prefix))
        {
            for row in target.start / columns..=target.end / columns {
                let left = if row == target.start / columns {
                    target.start % columns
                } else {
                    0
                };
                let right = if row == target.end / columns {
                    target.end % columns + 1
                } else {
                    columns
                };
                painter.rect_filled(
                    Rect::from_min_max(
                        rect.min + egui::vec2(left as f32, row as f32) * self.cell,
                        rect.min + egui::vec2(right as f32, (row + 1) as f32) * self.cell,
                    ),
                    1,
                    p.accent.gamma_multiply(0.22),
                );
            }
            let pos = rect.min
                + egui::vec2(
                    (target.start % columns) as f32,
                    (target.start / columns) as f32,
                ) * self.cell;
            let galley = painter.layout_no_wrap(
                target.label.to_uppercase(),
                egui::FontId::monospace(12.0),
                p.bg,
            );
            let size = galley.size() + egui::vec2(8.0, 2.0);
            let pos = egui::pos2(pos.x.min(rect.right() - size.x).max(rect.left()), pos.y);
            painter.rect_filled(Rect::from_min_size(pos, size), 3, p.terminal_fg);
            painter.galley(pos + egui::vec2(4.0, 1.0), galley, p.bg);
        }
        let caption = if hints.prefix.is_empty() {
            "Type a label to copy · Esc cancels".to_owned()
        } else {
            format!(
                "{} · Backspace edits · Esc cancels",
                hints.prefix.to_uppercase()
            )
        };
        let galley = painter.layout(
            caption,
            theme::regular(12.0),
            p.secondary,
            (rect.width() - 16.0).max(1.0),
        );
        let size = galley.size() + egui::vec2(16.0, 8.0);
        let bar = Rect::from_min_size(rect.right_bottom() - size, size);
        painter.rect_filled(bar, 4, p.bg);
        painter.galley(bar.min + egui::vec2(8.0, 4.0), galley, p.secondary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use terminal_core::{Cell, Flags};

    fn cache(text: &str, columns: usize) -> Cache {
        let cells: Vec<_> = text
            .chars()
            .enumerate()
            .map(|(i, c)| Cell {
                c,
                column: i % columns,
                ..Default::default()
            })
            .collect();
        let mut cache = Cache::default();
        cache.columns = columns as u16;
        cache.sources = cells
            .chunks(columns)
            .map(|cells| {
                let mut row = cells.to_vec();
                row.resize_with(columns, Cell::default);
                Arc::from(row)
            })
            .collect();
        cache.lines = cache.sources.len() as u16;
        cache
    }

    #[test]
    fn hints_copy_paths_locations_hashes_and_urls_without_prose() {
        let mut cache = cache(
            "at src/foo.rs:42:3 /tmp/out.txt deadbee (https://example.com/a_(b)). noise",
            100,
        );
        assert!(cache.begin_hints());
        let texts: Vec<_> = cache
            .hints
            .as_ref()
            .unwrap()
            .targets
            .iter()
            .map(|t| t.text.as_str())
            .collect();
        assert_eq!(
            texts,
            [
                "src/foo.rs:42:3",
                "/tmp/out.txt",
                "deadbee",
                "https://example.com/a_(b)"
            ]
        );
        assert_eq!(cache.hint_text("d"), HintInput::Copy("deadbee".into()));
    }

    #[test]
    fn hints_include_plain_extensionless_names_and_decoded_file_hyperlinks() {
        let mut cache = cache("AGENTS.md .env README Makefile open", 80);
        for cell in &mut Arc::make_mut(&mut cache.sources[0])[31..35] {
            cell.hyperlink = Some(Arc::from("file:///tmp/my%20notes"));
        }
        assert!(cache.begin_hints());
        let texts: Vec<_> = cache
            .hints
            .as_ref()
            .unwrap()
            .targets
            .iter()
            .map(|target| target.text.as_str())
            .collect();
        assert_eq!(
            texts,
            ["AGENTS.md", ".env", "README", "Makefile", "/tmp/my notes"]
        );
        let mut cache = self::cache("\"punctuation.md,\" \"ends.md.\" \"src/foo.rs:42:3\"", 80);
        assert!(cache.begin_hints());
        let texts: Vec<_> = cache
            .hints
            .as_ref()
            .unwrap()
            .targets
            .iter()
            .map(|target| target.text.as_str())
            .collect();
        assert_eq!(texts, ["punctuation.md,", "ends.md.", "src/foo.rs:42:3"]);
    }

    #[test]
    fn labels_are_unique_prefix_free_and_bounded_and_backspace_refines() {
        let mut cache = cache(
            &(0..400).map(|i| format!("file{i}.rs ")).collect::<String>(),
            5000,
        );
        assert!(cache.begin_hints());
        assert_eq!(cache.hints.as_ref().unwrap().targets.len(), MAX_HINTS);
        let labels: Vec<_> = cache
            .hints
            .as_ref()
            .unwrap()
            .targets
            .iter()
            .map(|t| &t.label)
            .collect();
        for (i, label) in labels.iter().enumerate() {
            assert!(
                labels
                    .iter()
                    .enumerate()
                    .all(|(j, other)| i == j || !other.starts_with(label.as_str()))
            );
        }
        assert_eq!(cache.hint_text("a"), HintInput::Pending);
        cache.hint_backspace();
        assert_eq!(cache.hint_text("as"), HintInput::Copy("file1.rs".into()));
        cache.cancel_hints();
        assert!(!cache.hinting());
    }

    #[test]
    fn hints_follow_soft_wraps_scrollback_and_ignore_hidden_cells() {
        let mut cache = cache("src/long_file.rs:42", 10);
        cache.display_offset = 9;
        Arc::make_mut(&mut cache.sources[0])[9]
            .flags
            .insert(Flags::WRAPLINE);
        assert!(cache.begin_hints());
        assert_eq!(
            cache.hint_text("a"),
            HintInput::Copy("src/long_file.rs:42".into())
        );
        cache.cancel_hints();
        for row in &mut cache.sources {
            for cell in Arc::make_mut(row) {
                cell.flags.insert(Flags::HIDDEN);
            }
        }
        assert!(!cache.begin_hints());
    }
}
