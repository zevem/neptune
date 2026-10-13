//! Bounded, literal UTF-8 text search. Only the explorer's worker reads files.
use crate::ui::explorer::TextMatch;
use std::{fs::File, io::Read, ops::Range, path::Path};

const FILE_BYTES: usize = 8 * 1024 * 1024;
pub(super) const TOTAL_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Query(String);

impl Query {
    pub(super) fn new(text: &str) -> Self {
        Self(text.to_lowercase())
    }

    /// Map the folded byte range back to the original UTF-8 text, including
    /// characters whose lowercase form expands (such as dotted capital I).
    fn find(&self, text: &str) -> Option<Range<usize>> {
        let folded = text.to_lowercase();
        let start = folded.find(&self.0)?;
        let end = start + self.0.len();
        let mut offset = 0;
        let mut original_start = None;
        for (index, ch) in text.char_indices() {
            let next = offset + ch.to_lowercase().map(char::len_utf8).sum::<usize>();
            if original_start.is_none() && next > start {
                original_start = Some(index);
            }
            if next >= end {
                return original_start.map(|start| start..index + ch.len_utf8());
            }
            offset = next;
        }
        None
    }
}

#[derive(Default)]
pub(super) struct Results {
    pub matches: Vec<TextMatch>,
    pub limited: bool,
    pub skipped: bool,
}

fn snippet(text: &str, range: Range<usize>, line: u32) -> TextMatch {
    let start = text[..range.start]
        .char_indices()
        .rev()
        .nth(40)
        .map_or(0, |(index, _)| index);
    let match_end = text[range.start..range.end]
        .char_indices()
        .nth(160)
        .map_or(range.end, |(index, _)| range.start + index);
    let end = text[match_end..]
        .char_indices()
        .nth(80)
        .map_or(text.len(), |(index, _)| match_end + index);
    let prefix = if start > 0 { "…" } else { "" };
    let mut snippet = format!("{prefix}{}", &text[start..end]);
    if end < text.len() {
        snippet.push('…');
    }
    // Keep offsets identical when replacing tabs and other control characters.
    snippet = snippet
        .chars()
        .map(|ch| if ch.is_ascii_control() { ' ' } else { ch })
        .collect();
    TextMatch {
        line,
        column: super::display_line(&text[..range.start]).1 as u32 + 1,
        snippet,
        highlight: prefix.len() + range.start - start..prefix.len() + match_end - start,
    }
}

pub(super) fn search(
    path: &Path,
    query: &Query,
    limit: usize,
    cancelled: &impl Fn() -> bool,
    total: &mut usize,
) -> Results {
    let mut result = Results::default();
    if !std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) {
        result.skipped = true;
        return result;
    }
    let Ok(mut file) = File::open(path) else {
        result.skipped = true;
        return result;
    };
    // Never read devices, sockets or pipes as if they were text files.
    let Ok(metadata) = file.metadata() else {
        result.skipped = true;
        return result;
    };
    if !metadata.is_file() {
        result.skipped = true;
        return result;
    }
    let budget = FILE_BYTES.min(TOTAL_BYTES.saturating_sub(*total));
    result.limited = metadata.len() > budget as u64;
    let mut bytes = Vec::new();
    let mut chunk = [0; 64 * 1024];
    while bytes.len() < budget {
        if cancelled() {
            result.limited = true;
            return result;
        }
        let amount = chunk.len().min(budget - bytes.len());
        match file.read(&mut chunk[..amount]) {
            Ok(0) => break,
            Ok(count) => {
                *total += count;
                if chunk[..count].contains(&0) {
                    result.skipped = true;
                    return result;
                }
                bytes.extend_from_slice(&chunk[..count]);
            }
            Err(_) => {
                result.skipped = true;
                return result;
            }
        }
    }
    // A file can grow after metadata was read. Reaching the byte budget must
    // still be reported even when its earlier size would have fit.
    result.limited |= bytes.len() == budget;
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(error) if result.limited && error.error_len().is_none() => {
            // A bounded read may end in the middle of the last UTF-8 character.
            std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or_default()
        }
        Err(_) => {
            result.skipped = true;
            return result;
        }
    };
    for (index, line) in text.lines().enumerate() {
        if cancelled() {
            result.limited = true;
            return result;
        }
        if let Some(range) = query.find(line) {
            if result.matches.len() == limit {
                result.limited = true;
                break;
            }
            result.matches.push(snippet(line, range, index as u32 + 1));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_matches_preserve_unicode_columns_and_highlight_offsets() {
        let text = "🪐 İSTANBUL has [brackets]";
        let range = Query::new("istanbul").find(text);
        // Unicode lowercase preserves the dot on İ, rather than stripping it.
        assert!(range.is_none());
        let range = Query::new("İstanbul").find(text).unwrap();
        let hit = snippet(text, range, 7);
        assert_eq!((hit.line, hit.column), (7, 3));
        assert_eq!(&hit.snippet[hit.highlight], "İSTANBUL");
        assert!(Query::new("[brackets]").find(text).is_some());
        let text = "\tstart needle";
        assert_eq!(
            snippet(text, Query::new("needle").find(text).unwrap(), 1).column,
            11
        );
    }

    #[test]
    fn snippets_keep_a_distant_match_in_bounded_unicode_context() {
        let text = format!("{}needle{}", "🪐".repeat(400), "尾".repeat(400));
        let hit = snippet(&text, Query::new("needle").find(&text).unwrap(), 1);
        assert_eq!(&hit.snippet[hit.highlight], "needle");
        assert!(hit.snippet.starts_with('…') && hit.snippet.ends_with('…'));
        assert!(hit.snippet.chars().count() < 300);
    }

    #[test]
    fn reads_past_preview_limits_and_bounds_results_files_and_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(
            &path,
            format!("{}NEEDLE\r\nneedle", "line\n".repeat(60_000)),
        )
        .unwrap();
        let run = |limit, cancelled: bool| {
            search(&path, &Query::new("needle"), limit, &|| cancelled, &mut 0)
        };
        let result = run(2, false);
        assert_eq!(
            result
                .matches
                .iter()
                .map(|hit| hit.line)
                .collect::<Vec<_>>(),
            [60_001, 60_002]
        );
        #[cfg(unix)]
        {
            let link = dir.path().join("linked.txt");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert_eq!(
                search(&link, &Query::new("needle"), 2, &|| false, &mut 0)
                    .matches
                    .len(),
                2
            );
        }
        assert!(run(1, false).limited);
        assert!(run(2, true).matches.is_empty());
        std::fs::write(&path, b"needle\0binary").unwrap();
        assert!(run(2, false).skipped);
        std::fs::write(&path, b"\xffneedle").unwrap();
        assert!(run(2, false).skipped);
        std::fs::write(&path, format!("needle\n{}", "x".repeat(FILE_BYTES))).unwrap();
        let result = run(2, false);
        assert!(result.limited);
        assert_eq!(result.matches.len(), 1);
    }
}
