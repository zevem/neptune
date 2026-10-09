//! The Markdown release notes and a lead's replies are written in, laid out
//! natively: headings, lists, quotes, tables, rules, fenced code and the marks
//! inside a line. File documents can also draw pictures decoded by their
//! worker; links lead somewhere only when pressed. Nothing executes markup.
//! What is not finished, such as a mark without its pair or half
//! a table, stays the text it is.
use super::{Action, helpers::focus_ring, helpers::place};
use crate::{
    icons::{self, Icon},
    platform::links::WebLink,
    theme::{self, Palette},
};
use eframe::egui::{
    self, Align, Color32, CursorIcon, FontId, Frame, Id, Layout, Margin, Pos2, Rect, Sense, Stroke,
    TextFormat, Ui, WidgetInfo, WidgetType, text::LayoutJob, vec2,
};
use std::{
    cell::Cell,
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
};

/// The height of a line of body text.
pub const LINE: f32 = 19.0;
/// How far a level of a list sets its items in: a nested marker stands
/// under the text of the item it belongs to.
const INDENT: f32 = 18.0;
/// The room a quote's bar takes.
const QUOTE: f32 = 12.0;
/// Quotes and lists deeper than this are drawn at this depth: a side panel
/// has no room for more.
const DEEPEST: u8 = 4;

#[derive(Debug, PartialEq, Eq)]
pub enum Block {
    /// A heading and its level, from one to six.
    Heading(u8, String),
    Bullet(String),
    /// An item of a numbered list, with its number as written.
    Numbered(String, String),
    /// An item with a box, ticked or not.
    Task(bool, String),
    /// A fenced or indented block, its lines as written.
    Code(String),
    Image(Image),
    Rule,
    Table(Table),
    Text(String),
}
/// A document image; decoding is owned by the file-preview worker.
#[derive(Debug, PartialEq, Eq)]
pub struct Image {
    pub source: String,
    pub alt: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub link: Option<String>,
}

#[derive(Clone)]
pub struct Picture {
    pub texture: egui::TextureHandle,
    pub pixels: [u32; 2],
}

pub struct Document {
    pub blocks: Vec<Placed>,
    pub pictures: HashMap<usize, Picture>,
    pub loading_images: bool,
    code_controls: HashMap<usize, CodeControls>,
}

struct CodeControls {
    wrap: Cell<bool>,
    copied_until: Cell<f64>,
}

impl Default for CodeControls {
    fn default() -> Self {
        Self {
            wrap: Cell::new(true),
            copied_until: Cell::new(0.0),
        }
    }
}

impl Document {
    pub fn new(blocks: Vec<Placed>) -> Self {
        let loading_images = blocks
            .iter()
            .any(|block| matches!(block.block, Block::Image(_)));
        let code_controls = blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| matches!(block.block, Block::Code(_)))
            .map(|(index, _)| (index, CodeControls::default()))
            .collect();
        Self {
            blocks,
            pictures: HashMap::new(),
            loading_images,
            code_controls,
        }
    }

    /// Unchanged blocks keep their controls and image layout on a file reload.
    /// State is released when its preview is replaced or closed.
    pub fn retain_state_from(&mut self, previous: &Self) {
        for (&index, controls) in &self.code_controls {
            if previous.blocks.get(index) == self.blocks.get(index)
                && let Some(old) = previous.code_controls.get(&index)
            {
                controls.wrap.set(old.wrap.get());
                controls.copied_until.set(old.copied_until.get());
            }
        }
        // Keep layout stable while unchanged images are being read again.
        // A failed replacement removes its cached picture on the UI thread.
        for (&index, picture) in &previous.pictures {
            if self.blocks.get(index).is_some()
                && self.blocks.get(index) == previous.blocks.get(index)
            {
                self.pictures.insert(index, picture.clone());
            }
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct Table {
    /// How each column is set, which is also how many there are.
    pub aligns: Vec<Align>,
    pub head: Vec<String>,
    /// Every row has a cell for each column.
    pub rows: Vec<Vec<String>>,
}
/// A block and where it stands.
#[derive(Debug, PartialEq, Eq)]
pub struct Placed {
    /// How many quotes it is inside.
    pub quote: u8,
    /// How many levels of a list it is set in by.
    pub inset: u8,
    pub block: Block,
}
/// What a line break inside a paragraph means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lines {
    /// Hard-wrapped release notes: continuation lines join their block and
    /// the surface decides where text wraps. Their headings are the labels
    /// of a sheet that has a title of its own.
    Joined,
    /// Conversation: a line break was meant.
    Kept,
}

/// The number that opens a list item, and the item.
fn numbered(line: &str) -> Option<(&str, &str)> {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    let rest = line.get(digits..)?;
    let item = rest
        .strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))?;
    (1..=3).contains(&digits).then(|| (&line[..digits], item))
}

/// The column `line`'s text starts at; a tab is four.
fn column(line: &str) -> usize {
    line.bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .map(|byte| if byte == b'\t' { 4 } else { 1 })
        .sum()
}
/// `line` without its first `columns` of white space.
fn undent(line: &str, columns: usize) -> &str {
    let (mut left, mut at) = (columns, 0);
    for byte in line.bytes() {
        let wide = match byte {
            b' ' => 1,
            b'\t' => 4,
            _ => break,
        };
        if left == 0 {
            break;
        }
        left = left.saturating_sub(wide);
        at += 1;
    }
    &line[at..]
}
/// How many quote marks open `line`, `most` at most, and what follows them.
fn quoted(line: &str, most: u8) -> (u8, &str) {
    let (mut depth, mut rest) = (0, line);
    while depth < most {
        let inner = rest.trim_start_matches(' ');
        let Some(inner) = inner
            .strip_prefix('>')
            .filter(|_| rest.len() - inner.len() <= 3)
        else {
            break;
        };
        rest = inner.strip_prefix(' ').unwrap_or(inner);
        depth += 1;
    }
    (depth, rest)
}
/// How long the run of one byte at `at` is.
fn run_of(bytes: &[u8], at: usize) -> usize {
    bytes[at..]
        .iter()
        .take_while(|byte| **byte == bytes[at])
        .count()
}
/// The mark and length of the fence `line` is, and what it says after it.
fn fence_of(line: &str) -> Option<(u8, usize, &str)> {
    let mark = *line.as_bytes().first()?;
    if mark != b'`' && mark != b'~' {
        return None;
    }
    let length = run_of(line.as_bytes(), 0);
    let info = line[length..].trim();
    // Backticks after the run make it a span of code inside a line.
    (length >= 3 && !(mark == b'`' && info.contains('`'))).then_some((mark, length, info))
}
/// The level and text of a heading: its signs are followed by a space, so
/// "#123" and "#tag" are text.
fn heading(line: &str) -> Option<(u8, &str)> {
    let level = line.bytes().take_while(|byte| *byte == b'#').count();
    let rest = &line[level..];
    if !(1..=6).contains(&level) || !rest.starts_with([' ', '\t']) {
        return None;
    }
    // A closing run of signs is part of the mark.
    let text = rest.trim();
    let closed = text.trim_end_matches('#');
    let text = if closed.is_empty() || closed.ends_with([' ', '\t']) {
        closed.trim_end()
    } else {
        text
    };
    (!text.is_empty()).then_some((level as u8, text))
}
/// The level of the heading a line of "=" or "-" makes of the paragraph
/// over it.
fn underline(line: &str) -> Option<u8> {
    let level = match line.as_bytes()[0] {
        b'=' => 1,
        b'-' => 2,
        _ => return None,
    };
    (line.len() >= 2 && run_of(line.as_bytes(), 0) == line.len()).then_some(level)
}
fn rule(line: &str) -> bool {
    let mut marks = line.bytes().filter(|byte| !matches!(byte, b' ' | b'\t'));
    let Some(mark @ (b'-' | b'*' | b'_')) = marks.next() else {
        return false;
    };
    let mut count = 1;
    marks.all(|byte| {
        count += 1;
        byte == mark
    }) && count >= 3
}
/// The item `line` opens, and how wide its marker is.
fn item(line: &str) -> Option<(Block, usize)> {
    if let Some((number, text)) = numbered(line) {
        let block = Block::Numbered(number.to_owned(), text.trim().to_owned());
        return Some((block, number.len() + 2));
    }
    let text = line
        .strip_prefix(['-', '*', '+'])?
        .strip_prefix([' ', '\t'])?
        .trim();
    for (mark, done) in [("[ ]", false), ("[x]", true), ("[X]", true)] {
        if let Some(rest) = text.strip_prefix(mark)
            && (rest.is_empty() || rest.starts_with(' '))
        {
            return Some((Block::Task(done, rest.trim().to_owned()), 2));
        }
    }
    Some((Block::Bullet(text.to_owned()), 2))
}
/// The cells of a table's row. An escaped bar is a bar inside a cell.
fn cells(line: &str) -> Vec<String> {
    let line = line.trim();
    let inner = line.strip_prefix('|').unwrap_or(line);
    let mut cells = vec![String::new()];
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                chars.next();
                cells.last_mut().expect("a cell").push('|');
            }
            '|' => cells.push(String::new()),
            c => cells.last_mut().expect("a cell").push(c),
        }
    }
    // The bar that ends the row opens no cell.
    if cells.len() > 1 && cells.last().is_some_and(|cell| cell.trim().is_empty()) {
        cells.pop();
    }
    cells.iter().map(|cell| cell.trim().to_owned()).collect()
}
/// How the columns are set, if `line` is the row under a table's header.
fn aligns(line: &str) -> Option<Vec<Align>> {
    if !line.contains('|') {
        return None;
    }
    cells(line)
        .iter()
        .map(|cell| {
            let (left, right) = (cell.starts_with(':'), cell.ends_with(':'));
            let dashes = cell.strip_prefix(':').unwrap_or(cell);
            let dashes = dashes.strip_suffix(':').unwrap_or(dashes);
            (!dashes.is_empty() && dashes.bytes().all(|byte| byte == b'-')).then_some(
                match (left, right) {
                    (true, true) => Align::Center,
                    (false, true) => Align::Max,
                    _ => Align::Min,
                },
            )
        })
        .collect()
}
/// The table whose header is `line`, where the line at `at` is the row of
/// dashes under it. Takes the lines of the table. A header whose dashes
/// have not come yet is text until they do.
fn table(source: &[&str], at: &mut usize, line: &str, quote: u8) -> Option<Table> {
    if !line.contains('|') {
        return None;
    }
    let below = |at: usize| {
        let (depth, rest) = quoted(source.get(at)?, DEEPEST);
        (depth == quote).then(|| rest.trim())
    };
    let aligns = aligns(below(*at)?)?;
    let head = cells(line);
    if head.len() != aligns.len() {
        return None;
    }
    *at += 1;
    let mut rows = Vec::new();
    while let Some(row) = below(*at).filter(|row| row.contains('|')) {
        // A row has a cell for each column, whatever was written.
        let mut row = cells(row);
        row.resize(head.len(), String::new());
        rows.push(row);
        *at += 1;
    }
    Some(Table { aligns, head, rows })
}
/// How many levels of a list a line that starts at `indent` is in: it is in
/// an item when it starts under that item's text. The levels it is past are
/// closed.
fn inside(lists: &mut Vec<(usize, usize)>, indent: usize) -> u8 {
    while lists.last().is_some_and(|(marker, _)| indent < marker + 2) {
        lists.pop();
    }
    lists.len().min(usize::from(DEEPEST)) as u8
}

/// A fence that is open: its mark, and where it stands.
#[derive(Clone, Copy)]
struct Fence {
    mark: u8,
    length: usize,
    indent: usize,
    quote: u8,
}

pub fn blocks(text: &str, lines: Lines) -> Vec<Placed> {
    parse_blocks(text, lines, false)
}

fn code_line(text: &str, indent: usize, verbatim: bool) -> &str {
    let text = undent(text, indent);
    if verbatim {
        text.trim_end_matches('\r')
    } else {
        text.trim_end()
    }
}

fn parse_blocks(text: &str, lines: Lines, verbatim_code: bool) -> Vec<Placed> {
    let source: Vec<&str> = text.lines().collect();
    let mut blocks: Vec<Placed> = Vec::new();
    // Whether the last block is still open to continuation lines.
    let mut open = false;
    // The lists a line can be in: where each one's markers and text start.
    let mut lists: Vec<(usize, usize)> = Vec::new();
    let mut fence = None::<Fence>;
    // How many quotes the line above was in.
    let mut within = 0;
    let mut at = 0;
    while let Some(raw) = source.get(at).copied() {
        at += 1;
        if let Some(fenced) = fence {
            let (depth, rest) = quoted(raw, fenced.quote);
            if depth == fenced.quote {
                let ends = fence_of(rest.trim()).is_some_and(|(mark, length, info)| {
                    mark == fenced.mark && length >= fenced.length && info.is_empty()
                });
                if ends {
                    fence = None;
                } else if let Some(Placed {
                    block: Block::Code(code),
                    ..
                }) = blocks.last_mut()
                {
                    if !code.is_empty() {
                        code.push('\n');
                    }
                    code.push_str(code_line(rest, fenced.indent, verbatim_code));
                }
                continue;
            }
            // The quote it stood in ended, and it with the quote.
            fence = None;
        }
        let (quote, rest) = quoted(raw, DEEPEST);
        if quote != within {
            open = false;
            lists.clear();
        }
        within = quote;
        let (indent, line) = (column(rest), rest.trim());
        if line.is_empty() {
            open = false;
            continue;
        }
        // Where a block that begins on this line stands.
        let mut place = |block| Placed {
            quote,
            inset: inside(&mut lists, indent),
            block,
        };
        if let Some((mark, length, _)) = fence_of(line) {
            blocks.push(place(Block::Code(String::new())));
            fence = Some(Fence {
                mark,
                length,
                indent,
                quote,
            });
            open = false;
        } else if let Some((level, text)) = heading(line) {
            blocks.push(place(Block::Heading(level, text.to_owned())));
            open = false;
        } else if let Some(table) = table(&source, &mut at, line, quote) {
            blocks.push(place(Block::Table(table)));
            open = false;
        } else if open
            && let Some(level) = underline(line)
            && let Some(Placed { block, .. }) = blocks.last_mut()
            && let Block::Text(text) = block
        {
            *block = Block::Heading(level, text.replace('\n', " "));
            open = false;
        } else if rule(line) {
            blocks.push(place(Block::Rule));
            open = false;
        } else if let Some((block, marker)) = item(line) {
            blocks.push(place(block));
            lists.push((indent, indent + marker));
            open = true;
        } else if open
            && let Some(Placed {
                block:
                    Block::Bullet(text)
                    | Block::Numbered(_, text)
                    | Block::Task(_, text)
                    | Block::Text(text),
                ..
            }) = blocks.last_mut()
        {
            text.push(match lines {
                Lines::Joined => ' ',
                Lines::Kept => '\n',
            });
            text.push_str(line);
        } else {
            let inset = inside(&mut lists, indent);
            let strip = lists.last().map_or(0, |(_, text)| *text) + 4;
            if indent < strip {
                blocks.push(Placed {
                    quote,
                    inset,
                    block: Block::Text(line.to_owned()),
                });
                open = true;
                continue;
            }
            // Set in by four under where it would start: code, to the
            // first line that is not.
            let mut code = code_line(rest, strip, verbatim_code).to_owned();
            let mut blank = 0;
            while let Some(next) = source.get(at) {
                let (depth, next) = quoted(next, DEEPEST);
                if depth != quote {
                    break;
                }
                if next.trim().is_empty() {
                    blank += 1;
                } else if column(next) >= strip {
                    code.extend(std::iter::repeat_n('\n', blank + 1));
                    code.push_str(code_line(next, strip, verbatim_code));
                    blank = 0;
                } else {
                    break;
                }
                at += 1;
            }
            blocks.push(Placed {
                quote,
                inset,
                block: Block::Code(code),
            });
            open = false;
        }
    }
    blocks
}

/// File documents also show Markdown images and HTML `img` elements.
/// Other HTML stays literal; this parser never executes markup.
pub fn document_blocks(text: &str) -> Vec<Placed> {
    let mut result = Vec::new();
    for placed in parse_blocks(text, Lines::Joined, true) {
        let text = match &placed.block {
            Block::Text(text)
            | Block::Bullet(text)
            | Block::Numbered(_, text)
            | Block::Task(_, text)
            | Block::Heading(_, text) => text,
            _ => {
                result.push(placed);
                continue;
            }
        };
        let listed = matches!(
            placed.block,
            Block::Bullet(_) | Block::Numbered(..) | Block::Task(..)
        );
        let part = |text: &str, first: bool| {
            let block = match (&placed.block, first) {
                (Block::Bullet(_), true) => Block::Bullet(text.into()),
                (Block::Numbered(number, _), true) => Block::Numbered(number.clone(), text.into()),
                (Block::Task(done, _), true) => Block::Task(*done, text.into()),
                (Block::Heading(level, _), true) => Block::Heading(*level, text.into()),
                _ => Block::Text(text.into()),
            };
            Placed {
                quote: placed.quote,
                inset: placed.inset + u8::from(listed && !first),
                block,
            }
        };
        let (mut start, mut at) = (0, 0);
        while at < text.len() {
            if text.as_bytes()[at] == b'`' {
                let run = run_of(text.as_bytes(), at);
                if let Some(end) = code_end(text.as_bytes(), at + run, run) {
                    at = end + run;
                    continue;
                }
            }
            if text.as_bytes()[at] == b'\\' {
                at += 1;
                at += text[at..].chars().next().map_or(0, char::len_utf8);
                continue;
            }
            if let Some((image, end)) = image_at(text, at) {
                if !text[start..at].trim().is_empty() {
                    result.push(part(text[start..at].trim(), start == 0));
                }
                result.push(Placed {
                    quote: placed.quote,
                    inset: placed.inset + u8::from(listed),
                    block: Block::Image(image),
                });
                at = end;
                start = end;
            } else {
                at += text[at..].chars().next().map_or(1, char::len_utf8);
            }
        }
        if start == 0 {
            result.push(placed);
        } else if !text[start..].trim().is_empty() {
            result.push(part(text[start..].trim(), false));
        }
    }
    result
}

fn image_at(text: &str, at: usize) -> Option<(Image, usize)> {
    let rest = &text[at..];
    if rest.starts_with("[![") {
        let outer = link_at(text, at)?;
        let (mut image, end) = image_at(text, at + 1)?;
        if end != outer.label {
            return None;
        }
        image.link = Some(outer.to);
        return Some((image, outer.end));
    }
    if rest.starts_with("![") {
        let found = link_at(text, at + 1)?;
        return Some((
            Image {
                source: found.to,
                alt: unescaped(&text[at + 2..found.label]),
                width: None,
                height: None,
                link: None,
            },
            found.end,
        ));
    }
    if !rest.get(..4)?.eq_ignore_ascii_case("<img")
        || !rest.as_bytes().get(4)?.is_ascii_whitespace()
    {
        return None;
    }
    let mut attributes = HashMap::new();
    let mut rest = &rest[4..];
    loop {
        rest = rest.trim_start();
        if rest.starts_with('>') || rest.starts_with("/>") {
            break;
        }
        let end = rest.find(|c: char| c.is_whitespace() || matches!(c, '=' | '>' | '/'))?;
        if end == 0 {
            return None;
        }
        let name = rest[..end].to_ascii_lowercase();
        rest = rest[end..].trim_start();
        if let Some(value) = rest.strip_prefix('=') {
            rest = value.trim_start();
            let (value, after) = if rest.starts_with(['\'', '"']) {
                let quote = rest.as_bytes()[0] as char;
                let end = rest[1..].find(quote)? + 1;
                (&rest[1..end], &rest[end + 1..])
            } else {
                let end = rest.find(|c: char| c.is_whitespace() || c == '>')?;
                (&rest[..end], &rest[end..])
            };
            attributes.insert(name, html_value(value));
            rest = after;
        }
    }
    let dimension = |key| {
        attributes
            .get(key)
            .and_then(|value: &String| value.parse::<u32>().ok())
            .filter(|value| (1..=16_384).contains(value))
    };
    let image = Image {
        source: attributes.get("src")?.clone(),
        alt: attributes.get("alt").cloned().unwrap_or_default(),
        width: dimension("width"),
        height: dimension("height"),
        link: None,
    };
    let end = text.len() - rest.len() + if rest.starts_with("/>") { 2 } else { 1 };
    Some((image, end))
}

fn html_value(mut text: &str) -> String {
    let mut value = String::with_capacity(text.len());
    while let Some(at) = text.find('&') {
        value.push_str(&text[..at]);
        text = &text[at..];
        if let Some((entity, character)) =
            ENTITIES.iter().find(|(entity, _)| text.starts_with(entity))
        {
            value.push(*character);
            text = &text[entity.len()..];
        } else {
            value.push('&');
            text = &text[1..];
        }
    }
    value.push_str(text);
    value
}

/// How a run of text inside a line is set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Style {
    strong: bool,
    emphasis: bool,
    strike: bool,
    code: bool,
    /// Which of the line's links it is the text of.
    link: Option<u16>,
}
#[derive(Debug, PartialEq, Eq)]
struct Span {
    text: String,
    style: Style,
}
/// A line as its runs of text, and where its links lead as written.
#[derive(Debug, Default, PartialEq, Eq)]
struct Inline {
    spans: Vec<Span>,
    links: Vec<String>,
}

enum Token {
    Text {
        text: String,
        code: bool,
        link: Option<u16>,
    },
    /// A run of one mark of strong text, emphasis or a strike.
    Mark {
        mark: u8,
        /// How long the run was written.
        length: usize,
        /// How much of it no pair has used yet.
        left: usize,
        opens: bool,
        closes: bool,
        /// How many strong, emphasised and struck runs begin after it, and
        /// how many end before it.
        on: [u8; 3],
        off: [u8; 3],
        link: Option<u16>,
    },
}

const ENTITIES: [(&str, char); 7] = [
    ("&amp;", '&'),
    ("&lt;", '<'),
    ("&gt;", '>'),
    ("&quot;", '"'),
    ("&#39;", '\''),
    ("&apos;", '\''),
    ("&nbsp;", '\u{a0}'),
];

/// Where the run of `run` backticks that closes a span of code begins.
fn code_end(bytes: &[u8], mut at: usize, run: usize) -> Option<usize> {
    while at < bytes.len() {
        if bytes[at] == b'`' {
            let length = run_of(bytes, at);
            if length == run {
                return Some(at);
            }
            at += length;
        } else {
            at += 1;
        }
    }
    None
}
/// How long the tag at the start of `rest` is, if it breaks the line.
fn line_break(rest: &str) -> Option<usize> {
    ["<br>", "<br/>", "<br />"].into_iter().find_map(|tag| {
        rest.get(..tag.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(tag))
            .then_some(tag.len())
    })
}
/// The address written out at the start of `rest`, without the punctuation
/// of the sentence it is in.
fn bare(rest: &str) -> &str {
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '`' | '"'))
        .unwrap_or(rest.len());
    let mut url = &rest[..end];
    loop {
        let mut trimmed =
            url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '*', '_', '~', '|']);
        for (open, close) in [('(', ')'), ('[', ']')] {
            if trimmed.ends_with(close)
                && trimmed.matches(close).count() > trimmed.matches(open).count()
            {
                trimmed = &trimmed[..trimmed.len() - 1];
            }
        }
        if trimmed.len() == url.len() {
            return url;
        }
        url = trimmed;
    }
}
/// `text` without the backslashes that escape its punctuation.
fn unescaped(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match chars.peek() {
            Some(next) if c == '\\' && next.is_ascii_punctuation() => {}
            _ => plain.push(c),
        }
    }
    plain
}

/// A link written as `[text](address "title")`.
struct Linked {
    /// Where the bracket that closes its text is.
    label: usize,
    to: String,
    /// Where what follows it begins.
    end: usize,
}
/// The link whose text opens with the bracket at `open`. Every index it
/// stops at is a mark of one byte, so slices between them are whole.
fn link_at(text: &str, open: usize) -> Option<Linked> {
    let bytes = text.as_bytes();
    let (mut depth, mut at) = (0_usize, open);
    let label = loop {
        match *bytes.get(at)? {
            b'\\' => at += 1,
            // A bracket inside code is the code's.
            b'`' => {
                let run = run_of(bytes, at);
                at = code_end(bytes, at + run, run).unwrap_or(at) + run - 1;
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break at;
                }
            }
            _ => {}
        }
        at += 1;
    };
    if bytes.get(label + 1) != Some(&b'(') {
        return None;
    }
    let space = |at: &mut usize| {
        while matches!(bytes.get(*at), Some(b' ' | b'\t' | b'\n')) {
            *at += 1;
        }
    };
    let mut at = label + 2;
    space(&mut at);
    let to = if bytes.get(at) == Some(&b'<') {
        let end = at + 1 + text[at + 1..].find(['>', '\n'])?;
        if bytes[end] != b'>' {
            return None;
        }
        let to = &text[at + 1..end];
        at = end + 1;
        to
    } else {
        let (start, mut nested) = (at, 0_usize);
        loop {
            match *bytes.get(at)? {
                b' ' | b'\t' | b'\n' => break,
                b'(' => nested += 1,
                b')' if nested == 0 => break,
                b')' => nested -= 1,
                b'\\' => at += 1,
                _ => {}
            }
            at += 1;
        }
        text.get(start..at)?
    };
    space(&mut at);
    if let Some(quote @ (b'"' | b'\'' | b'(')) = bytes.get(at).copied() {
        let closing = if quote == b'(' { b')' } else { quote };
        at += 1;
        loop {
            match *bytes.get(at)? {
                b'\\' => at += 1,
                byte if byte == closing => break,
                _ => {}
            }
            at += 1;
        }
        at += 1;
        space(&mut at);
    }
    (bytes.get(at) == Some(&b')')).then(|| Linked {
        label,
        to: unescaped(to),
        end: at + 1,
    })
}

/// Reads `text` into `tokens`: what is certainly text or code, and the
/// marks `pair` has yet to match. `link` is the link it is the text of.
fn scan(text: &str, link: Option<u16>, tokens: &mut Vec<Token>, links: &mut Vec<String>) {
    fn flush(plain: &mut String, link: Option<u16>, tokens: &mut Vec<Token>) {
        if !plain.is_empty() {
            tokens.push(Token::Text {
                text: std::mem::take(plain),
                code: false,
                link,
            });
        }
    }
    /// Adds a link and returns which it is; none where a line has too many.
    fn linked(links: &mut Vec<String>, to: String) -> Option<u16> {
        let index = u16::try_from(links.len()).ok()?;
        links.push(to);
        Some(index)
    }
    let bytes = text.as_bytes();
    let mut plain = String::new();
    // Always at the start of a character.
    let mut at = 0;
    while at < bytes.len() {
        let rest = &text[at..];
        match bytes[at] {
            b'\\' => match rest[1..].chars().next() {
                Some(c) if c.is_ascii_punctuation() || c == '\n' => {
                    plain.push(c);
                    at += 2;
                }
                _ => {
                    plain.push('\\');
                    at += 1;
                }
            },
            b'`' => {
                let run = run_of(bytes, at);
                let Some(end) = code_end(bytes, at + run, run) else {
                    plain.push_str(&rest[..run]);
                    at += run;
                    continue;
                };
                flush(&mut plain, link, tokens);
                let code = &text[at + run..end];
                // A space each side sets a backtick apart from the marks.
                let code = code
                    .strip_prefix(' ')
                    .and_then(|code| code.strip_suffix(' '))
                    .filter(|code| !code.trim().is_empty())
                    .unwrap_or(code);
                tokens.push(Token::Text {
                    text: code.to_owned(),
                    code: true,
                    link,
                });
                at = end + run;
            }
            mark @ (b'*' | b'_' | b'~') => {
                let run = run_of(bytes, at);
                let before = text[..at].chars().next_back();
                let after = text[at + run..].chars().next();
                let space = |c: Option<char>| c.is_none_or(char::is_whitespace);
                let sign = |c: Option<char>| c.is_some_and(|c| c.is_ascii_punctuation());
                // A run opens what follows it when it leans on it, and
                // closes what it follows; an underscore inside a word is
                // part of the word.
                let left = !space(after) && (!sign(after) || space(before) || sign(before));
                let right = !space(before) && (!sign(before) || space(after) || sign(after));
                let (opens, closes) = match mark {
                    b'_' => (
                        left && (!right || sign(before)),
                        right && (!left || sign(after)),
                    ),
                    _ => (left, right),
                };
                if (mark == b'~' && run != 2) || !(opens || closes) {
                    plain.push_str(&rest[..run]);
                } else {
                    flush(&mut plain, link, tokens);
                    tokens.push(Token::Mark {
                        mark,
                        length: run,
                        left: run,
                        opens,
                        closes,
                        on: [0; 3],
                        off: [0; 3],
                        link,
                    });
                }
                at += run;
            }
            b'[' if link.is_none() => {
                let Some(found) = link_at(text, at) else {
                    plain.push('[');
                    at += 1;
                    continue;
                };
                flush(&mut plain, link, tokens);
                let label = &text[at + 1..found.label];
                at = found.end;
                if label.trim().is_empty() {
                    let link = linked(links, found.to.clone());
                    tokens.push(Token::Text {
                        text: found.to,
                        code: false,
                        link,
                    });
                } else {
                    let link = linked(links, found.to);
                    scan(label, link, tokens, links);
                }
            }
            // A picture is never loaded: it is the link its words are.
            b'!' if bytes.get(at + 1) == Some(&b'[') => {
                let Some(found) = link_at(text, at + 1) else {
                    plain.push('!');
                    at += 1;
                    continue;
                };
                flush(&mut plain, link, tokens);
                let words = &text[at + 2..found.label];
                let words = if words.trim().is_empty() {
                    found.to.clone()
                } else {
                    unescaped(words)
                };
                let link = link.or_else(|| linked(links, found.to));
                tokens.push(Token::Text {
                    text: words,
                    code: false,
                    link,
                });
                at = found.end;
            }
            b'<' => {
                if let Some(length) = line_break(rest) {
                    plain.push('\n');
                    at += length;
                } else if link.is_none()
                    && let Some(end) = rest.find('>')
                    && WebLink::new(&rest[1..end]).is_some()
                {
                    flush(&mut plain, link, tokens);
                    let link = linked(links, rest[1..end].to_owned());
                    tokens.push(Token::Text {
                        text: rest[1..end].to_owned(),
                        code: false,
                        link,
                    });
                    at += end + 1;
                } else {
                    // Any other tag is shown as it was written.
                    plain.push('<');
                    at += 1;
                }
            }
            b'h' if link.is_none()
                && (rest.starts_with("http://") || rest.starts_with("https://"))
                && !text[..at]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_alphanumeric) =>
            {
                let url = bare(rest);
                if WebLink::new(url).is_some() {
                    flush(&mut plain, link, tokens);
                    let link = linked(links, url.to_owned());
                    tokens.push(Token::Text {
                        text: url.to_owned(),
                        code: false,
                        link,
                    });
                } else {
                    plain.push_str(url);
                }
                at += url.len();
            }
            b'&' => match ENTITIES.iter().find(|(name, _)| rest.starts_with(name)) {
                Some((name, c)) => {
                    plain.push(*c);
                    at += name.len();
                }
                None => {
                    plain.push('&');
                    at += 1;
                }
            },
            _ => {
                // To the next byte that can begin a mark: all of those
                // are one byte long, so the slice is whole.
                let length = 1 + bytes[at + 1..]
                    .iter()
                    .position(|byte| {
                        matches!(
                            byte,
                            b'\\' | b'`' | b'*' | b'_' | b'~' | b'[' | b'!' | b'<' | b'h' | b'&'
                        )
                    })
                    .unwrap_or(bytes.len() - at - 1);
                plain.push_str(&rest[..length]);
                at += length;
            }
        }
    }
    flush(&mut plain, link, tokens);
}

/// Matches the marks of `tokens` one pair at a time, from the left: a mark
/// closes the nearest one before it that it fits. One without a partner is
/// left as it was written, and takes no other pair with it.
fn pair(tokens: &mut [Token]) {
    // The marks that can still be closed, by kind, the nearest last.
    let mut open: [Vec<usize>; 3] = Default::default();
    for at in 0..tokens.len() {
        let Token::Mark {
            mark,
            length,
            opens,
            closes,
            ..
        } = tokens[at]
        else {
            continue;
        };
        let kind = match mark {
            b'*' => 0,
            b'_' => 1,
            _ => 2,
        };
        // While some of it is left to close with.
        while closes
            && let Token::Mark {
                left: left @ 1.., ..
            } = tokens[at]
        {
            // Not far back: a text is not searched whole for every mark.
            let back = open[kind].iter().rev().take(8).position(|opener| {
                let Token::Mark {
                    length: other,
                    closes: either,
                    ..
                } = tokens[*opener]
                else {
                    return false;
                };
                // Where one of the two could be either end, they pair
                // only if their lengths do not add up to a multiple of
                // three: "*a**b*" is one emphasis around two marks.
                kind == 2
                    || !((opens || either)
                        && (length + other).is_multiple_of(3)
                        && !(length.is_multiple_of(3) && other.is_multiple_of(3)))
            });
            let Some(back) = back else {
                break;
            };
            let slot = open[kind].len() - 1 - back;
            let from = open[kind][slot];
            // What opened inside the pair and did not close stays text.
            open[kind].truncate(slot + 1);
            for (other, open) in open.iter_mut().enumerate() {
                while other != kind && open.last().is_some_and(|opener| *opener > from) {
                    open.pop();
                }
            }
            let Token::Mark {
                left: opener_left, ..
            } = tokens[from]
            else {
                break;
            };
            // Two marks each side are strong text, one is emphasis.
            let (used, style) = match kind {
                2 => (2, 2),
                _ if opener_left >= 2 && left >= 2 => (2, 0),
                _ => (1, 1),
            };
            if let Token::Mark { left, on, .. } = &mut tokens[from] {
                *left -= used;
                on[style] = on[style].saturating_add(1);
                if *left == 0 {
                    open[kind].pop();
                }
            }
            if let Token::Mark { left, off, .. } = &mut tokens[at] {
                *left -= used;
                off[style] = off[style].saturating_add(1);
            }
        }
        if opens && matches!(tokens[at], Token::Mark { left, .. } if left > 0) {
            open[kind].push(at);
        }
    }
}

/// The runs of `text`. A mark without its pair is shown as written, or left
/// out where `unpaired` is false, for words that are read without marks.
fn inline(text: &str, unpaired: bool) -> Inline {
    let (mut tokens, mut links) = (Vec::new(), Vec::new());
    scan(text, None, &mut tokens, &mut links);
    pair(&mut tokens);
    let mut spans: Vec<Span> = Vec::new();
    let mut add = |text: &str, style: Style| match spans.last_mut() {
        Some(last) if last.style == style => last.text.push_str(text),
        _ => spans.push(Span {
            text: text.to_owned(),
            style,
        }),
    };
    // How many strong, emphasised and struck runs the text is inside.
    let mut depth = [0_u8; 3];
    let style = |depth: [u8; 3], code, link| Style {
        strong: depth[0] > 0,
        emphasis: depth[1] > 0,
        strike: depth[2] > 0,
        code,
        link,
    };
    for token in &tokens {
        match token {
            Token::Text { text, code, link } => add(text, style(depth, *code, *link)),
            Token::Mark {
                mark,
                left,
                on,
                off,
                link,
                ..
            } => {
                for (depth, off) in depth.iter_mut().zip(off) {
                    *depth = depth.saturating_sub(*off);
                }
                if *left > 0 && unpaired {
                    let marks = char::from(*mark).to_string().repeat(*left);
                    add(&marks, style(depth, false, *link));
                }
                for (depth, on) in depth.iter_mut().zip(on) {
                    *depth = depth.saturating_add(*on);
                }
            }
        }
    }
    Inline { spans, links }
}

/// Where a pressed link leads: a page in the browser, or a file of this
/// computer with the application that opens it. Nothing else a text names
/// is opened.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    Web(WebLink),
    File(PathBuf),
    Document(PathBuf, Option<String>),
}

impl Target {
    fn action(&self) -> Action {
        match self {
            Self::Web(link) => Action::OpenLink(link.clone()),
            Self::File(path) => Action::Explorer(super::explorer::Event::Open(path.clone())),
            Self::Document(path, anchor) => Action::Explorer(super::explorer::Event::FollowLink {
                path: path.clone(),
                anchor: anchor.clone(),
            }),
        }
    }
}

fn target_at(link: &str, document: Option<&Path>) -> Option<Target> {
    let Some(document) = document else {
        return target(link);
    };
    if let Some(web) = WebLink::new(link) {
        return Some(Target::Web(web));
    }
    if link.len() > 4096 || link.chars().any(char::is_control) {
        return None;
    }
    let (name, anchor) = link.split_once('#').map_or((link, None), |(name, anchor)| {
        (name, Some(anchor.to_owned()))
    });
    let path = if name.is_empty() {
        document.to_path_buf()
    } else if name
        .get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file://"))
    {
        let Target::File(path) = target(name)? else {
            return None;
        };
        path
    } else {
        // A URL scheme is not a relative file, including script/data URLs.
        if name.contains(':') && !Path::new(name).is_absolute() {
            return None;
        }
        let mut bytes = Vec::with_capacity(name.len());
        let mut source = name.bytes();
        while let Some(byte) = source.next() {
            bytes.push(if byte == b'%' {
                let digits = [source.next()?, source.next()?];
                u8::from_str_radix(std::str::from_utf8(&digits).ok()?, 16).ok()?
            } else {
                byte
            });
        }
        let name = String::from_utf8(bytes).ok()?;
        if name.chars().any(char::is_control) {
            return None;
        }
        document.parent()?.join(name)
    };
    Some(Target::Document(path, anchor))
}

pub fn picture_path(source: &str, document: &Path) -> Option<PathBuf> {
    match target_at(source, Some(document))? {
        Target::Document(path, _) => Some(path),
        _ => None,
    }
}

fn heading_anchor(text: &str) -> String {
    inline(text, false)
        .spans
        .into_iter()
        .flat_map(|span| span.text.chars().collect::<Vec<_>>())
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || matches!(c, '-' | '_'))
        .flat_map(char::to_lowercase)
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .collect()
}

fn target(link: &str) -> Option<Target> {
    if let Some(web) = WebLink::new(link) {
        return Some(Target::Web(web));
    }
    let path = match link.get(..7) {
        Some(scheme) if scheme.eq_ignore_ascii_case("file://") => {
            let rest = &link[7..];
            let rest = rest.strip_prefix("localhost").unwrap_or(rest);
            // A drive of Windows follows the slash that begins the path.
            let rest = if cfg!(windows) {
                rest.strip_prefix('/').unwrap_or(rest)
            } else {
                rest
            };
            let mut bytes = Vec::with_capacity(rest.len());
            let mut source = rest.bytes();
            while let Some(byte) = source.next() {
                if byte == b'%' {
                    let digits = [source.next()?, source.next()?];
                    let digits = std::str::from_utf8(&digits).ok()?;
                    bytes.push(u8::from_str_radix(digits, 16).ok()?);
                } else {
                    bytes.push(byte);
                }
            }
            String::from_utf8(bytes).ok()?
        }
        _ => link.to_owned(),
    };
    // The line a link points at is not part of the file's name.
    let mut name = path.as_str();
    if let Some((file, anchor)) = name.rsplit_once("#L")
        && anchor
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'-' | b'L' | b'C'))
    {
        name = file;
    }
    for _ in 0..2 {
        if let Some((file, number)) = name.rsplit_once(':')
            && !number.is_empty()
            && number.bytes().all(|byte| byte.is_ascii_digit())
        {
            name = file;
        }
    }
    let path = PathBuf::from(name);
    (path.is_absolute() && name.len() <= 4096 && !name.chars().any(char::is_control))
        .then_some(Target::File(path))
}

/// The faces the text of a block is set in.
struct Face {
    plain: FontId,
    strong: FontId,
    code: FontId,
    ink: Color32,
    /// The height of its lines; the font's own without one.
    line: Option<f32>,
}
impl Face {
    fn body(ink: Color32) -> Self {
        Self {
            plain: theme::regular(13.0),
            strong: theme::medium(13.0),
            code: FontId::monospace(12.0),
            ink,
            line: Some(LINE),
        }
    }
    /// A heading of a reply. The first two levels are a little larger than
    /// the text and the rest are told apart by weight and ink: a side panel
    /// has no room for a page's hierarchy. The headings of release notes
    /// are all the label they have always been.
    fn heading(level: u8, lines: Lines, ink: Color32, quiet: Color32) -> Self {
        let (size, ink, line) = match (lines, level) {
            (Lines::Joined, _) => (11.5, quiet, None),
            (_, 1) => (16.0, ink, Some(23.0)),
            (_, 2) => (14.5, ink, Some(21.0)),
            (_, 3) => (13.0, ink, Some(LINE)),
            (_, 4) => (13.0, quiet, Some(LINE)),
            _ => (11.5, quiet, Some(LINE)),
        };
        Self {
            plain: theme::medium(size),
            strong: theme::semibold(size),
            code: FontId::monospace(size - 1.0),
            ink,
            line,
        }
    }
    /// A cell of a table, which is set a little smaller than the text
    /// around it; its header in the medium weight.
    fn cell(head: bool, ink: Color32) -> Self {
        Self {
            plain: if head {
                theme::medium(12.0)
            } else {
                theme::regular(12.0)
            },
            strong: theme::medium(12.0),
            code: FontId::monospace(11.0),
            ink,
            line: Some(17.0),
        }
    }
}

/// A line ready to be laid out, and its links: which characters of it each
/// one is, and where it leads as written.
struct Laid {
    job: LayoutJob,
    links: Vec<(Range<usize>, String)>,
}

/// Sets `text` in `face`: `code` in the terminal's face, **strong** in the
/// heavier weight, *emphasis* slanted, ~~struck~~ text with a line through
/// it. A link that can be pressed, which is what `live` asks for, is
/// underlined.
fn lay(text: &str, face: &Face, live: bool) -> Laid {
    lay_at(text, face, live, None)
}

fn lay_at(text: &str, face: &Face, live: bool, document: Option<&Path>) -> Laid {
    let Inline { spans, links } = inline(text, true);
    let pressed: Vec<bool> = links
        .iter()
        .map(|link| live && target_at(link, document).is_some())
        .collect();
    let mut ranges = vec![None::<Range<usize>>; links.len()];
    let mut job = LayoutJob::default();
    let mut chars = 0;
    for Span { text, style } in &spans {
        let font = if style.code {
            &face.code
        } else if style.strong {
            &face.strong
        } else {
            &face.plain
        };
        let mut format = TextFormat {
            font_id: font.clone(),
            color: face.ink,
            line_height: face.line,
            valign: Align::Center,
            italics: style.emphasis,
            ..Default::default()
        };
        if style.strike {
            format.strikethrough = Stroke::new(1.0, face.ink);
        }
        let end = chars + text.chars().count();
        if let Some(link) = style.link.map(usize::from) {
            if pressed[link] {
                format.underline = Stroke::new(1.0, face.ink.gamma_multiply(0.55));
            }
            let start = ranges[link].as_ref().map_or(chars, |range| range.start);
            ranges[link] = Some(start..end);
        }
        job.append(text, 0.0, format);
        chars = end;
    }
    Laid {
        job,
        links: ranges
            .into_iter()
            .zip(links)
            .filter_map(|(range, link)| Some((range?, link)))
            .collect(),
    }
}

/// Body text with `code` spans set in the terminal face, **strong** ones in
/// the medium weight, and the other marks of a line. A mark without its
/// pair is ordinary text.
pub fn body(text: &str, color: Color32) -> LayoutJob {
    lay(text, &Face::body(color), false).job
}

/// The words of `text` on one line, without the marks that set them: what
/// a preview shows of a report.
pub fn plain(text: &str) -> String {
    fn add(words: &mut String, text: &str) {
        for span in inline(text, false).spans {
            words.push_str(&span.text);
        }
        words.push(' ');
    }
    let mut words = String::new();
    for placed in blocks(text, Lines::Joined) {
        match &placed.block {
            Block::Heading(_, text)
            | Block::Bullet(text)
            | Block::Numbered(_, text)
            | Block::Task(_, text)
            | Block::Text(text) => add(&mut words, text),
            Block::Code(code) => {
                words.push_str(code);
                words.push(' ');
            }
            Block::Table(table) => {
                for cell in std::iter::once(&table.head).chain(&table.rows).flatten() {
                    add(&mut words, cell);
                }
            }
            Block::Rule => {}
            Block::Image(image) => add(&mut words, &image.alt),
        }
    }
    words.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A label of `job`, wrapped at the available width as any label is. Its
/// text can be selected. Returns where its last line is, so that a control
/// can sit at the end of what was written.
pub fn wrapped(ui: &mut Ui, job: LayoutJob) -> Rect {
    let laid = Laid {
        job,
        links: Vec::new(),
    };
    label(ui, laid, 0, None)
}

/// `wrapped`, with links: one that leads somewhere is pressed where its
/// text is, which shows where it leads under the pointer. `salt` tells the
/// labels of one surface apart. Without `actions` no link is pressed.
fn label(
    ui: &mut Ui,
    laid: Laid,
    salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    actions: Option<(&mut Vec<Action>, Palette, Option<&Path>)>,
) -> Rect {
    let Laid { mut job, links } = laid;
    job.wrap.max_width = ui.available_width();
    job.halign = ui.layout().horizontal_placement();
    job.justify = ui.layout().horizontal_justify();
    let galley = ui.painter().layout_job(job);
    let rect = ui.add(egui::Label::new(galley.clone())).rect;
    let origin = match galley.job.halign {
        Align::Min => rect.left_top(),
        Align::Center => rect.center_top(),
        Align::Max => rect.right_top(),
    };
    if let Some((actions, p, document)) =
        actions.filter(|_| !links.is_empty() && ui.is_rect_visible(rect))
    {
        for (index, (range, link)) in links.iter().enumerate() {
            let target = target_at(link, document);
            let words = || -> String {
                let text = galley.job.text.chars();
                text.skip(range.start).take(range.len()).collect()
            };
            // How many characters the rows above hold.
            let mut above = 0;
            for (line, row) in galley.rows.iter().enumerate() {
                let count = row.row.glyphs.len();
                let (from, to) = (range.start.max(above), range.end.min(above + count));
                let first = above;
                above += count + usize::from(row.ends_with_newline);
                if from >= to {
                    continue;
                }
                let x = |at: usize| {
                    let glyph = row.row.glyphs.get(at - first);
                    row.pos.x + glyph.map_or(row.row.size.x, |glyph| glyph.pos.x)
                };
                let part = Rect::from_min_max(
                    Pos2::new(x(from), row.pos.y),
                    Pos2::new(x(to), row.pos.y + row.row.size.y),
                )
                .translate(origin.to_vec2());
                let id = ui.id().with(("markup-link", salt, index, line));
                let Some(target) = &target else {
                    // Where it would lead is shown; it leads nowhere.
                    if !link.is_empty() {
                        ui.interact(part, id, Sense::hover())
                            .on_hover_text(link.as_str());
                    }
                    continue;
                };
                let response = ui.interact(part, id, Sense::click());
                response.widget_info(|| WidgetInfo::labeled(WidgetType::Link, true, words()));
                if response.has_focus() {
                    focus_ring(ui.painter(), part.expand2(vec2(2.0, 0.0)), 3, p);
                }
                if response
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_text(link.as_str())
                    .clicked()
                {
                    actions.push(target.action());
                }
            }
        }
    }
    galley
        .rows
        .last()
        .map_or(rect, |row| row.rect().translate(origin.to_vec2()))
}

/// How far a block is set in by the quotes and lists it stands in.
fn inset(placed: &Placed) -> f32 {
    f32::from(placed.quote) * QUOTE + f32::from(placed.inset) * INDENT
}

/// The width `text` takes when `show` lays it out in no more than `limit`:
/// a short message is as wide as its longest line, not as its column.
pub fn width(ui: &Ui, text: &str, lines: Lines, limit: f32) -> f32 {
    let line = |text: &str, face: Face, room: f32| {
        let mut job = lay(text, &face, false).job;
        job.wrap.max_width = room.max(0.0);
        ui.painter().layout_job(job).size().x
    };
    let ink = Color32::PLACEHOLDER;
    let mut widest = 0.0_f32;
    for placed in blocks(text, lines) {
        let left = inset(&placed);
        let room = limit - left;
        widest = widest.max(
            left + match &placed.block {
                Block::Heading(level, text) => {
                    line(text, Face::heading(*level, lines, ink, ink), room)
                }
                Block::Bullet(text) | Block::Numbered(_, text) | Block::Task(_, text) => {
                    let marker = 16.0 + ui.spacing().item_spacing.x;
                    marker + line(text, Face::body(ink), room - marker)
                }
                // A fenced block and a table fill their column.
                Block::Code(_) | Block::Table(_) | Block::Image(_) => room,
                Block::Rule => 0.0,
                Block::Text(text) => line(text, Face::body(ink), room),
            },
        );
    }
    // A point to spare, so that laying it out again breaks no line anew.
    (widest + 1.0).ceil().min(limit)
}

/// Lays `text` out as blocks down the available width. Its text can be
/// selected and copied. Returns where its last line is; nothing for a text
/// without a block. Its links are text: see `show_in`.
pub fn show(ui: &mut Ui, p: Palette, text: &str, lines: Lines) -> Option<Rect> {
    render(ui, p, text, lines, p.fg, None)
}
/// The same in `ink`, for text that is quoted rather than said, with links
/// that are pressed: a web address opens in the browser and a file of this
/// computer with its application, as `actions` is asked to. The blocks of
/// one `ui` share their names, so a surface that shows several texts gives
/// each a `ui` of its own.
pub fn show_in(
    ui: &mut Ui,
    p: Palette,
    text: &str,
    lines: Lines,
    ink: Color32,
    actions: &mut Vec<Action>,
) -> Option<Rect> {
    render(ui, p, text, lines, ink, Some(actions))
}

fn render(
    ui: &mut Ui,
    p: Palette,
    text: &str,
    lines: Lines,
    ink: Color32,
    actions: Option<&mut Vec<Action>>,
) -> Option<Rect> {
    render_blocks(ui, p, &blocks(text, lines), lines, ink, actions, None)
}

/// Draws a document already parsed by a file-preview worker.
pub fn show_document(
    ui: &mut Ui,
    p: Palette,
    document: &Document,
    path: &Path,
    anchor: Option<&str>,
    actions: &mut Vec<Action>,
) -> Option<Rect> {
    // Files have headings of their own; the small section labels of release
    // notes would flatten a document's hierarchy. Paragraphs are already joined.
    render_blocks(
        ui,
        p,
        &document.blocks,
        Lines::Kept,
        p.fg,
        Some(actions),
        Some((path, document, anchor)),
    )
}

fn render_blocks(
    ui: &mut Ui,
    p: Palette,
    blocks: &[Placed],
    lines: Lines,
    ink: Color32,
    mut actions: Option<&mut Vec<Action>>,
    document: Option<(&Path, &Document, Option<&str>)>,
) -> Option<Rect> {
    let marker = |ui: &mut Ui, block: &Block| {
        let (_, marker) = ui.allocate_space(vec2(16.0, LINE));
        match block {
            Block::Numbered(number, _) => {
                ui.painter().text(
                    Pos2::new(marker.right() - 4.0, marker.center().y),
                    egui::Align2::RIGHT_CENTER,
                    format!("{number}."),
                    theme::regular(12.0),
                    p.muted,
                );
            }
            Block::Task(done, _) => {
                let tick = Rect::from_center_size(
                    Pos2::new(marker.left() + 6.0, marker.center().y),
                    vec2(11.0, 11.0),
                );
                ui.painter().rect_stroke(
                    tick,
                    3,
                    Stroke::new(1.0, p.muted),
                    egui::StrokeKind::Inside,
                );
                if *done {
                    icons::paint(ui.painter(), tick.shrink(1.5), Icon::Check, p.secondary);
                }
            }
            _ => {
                ui.painter().circle_filled(
                    Pos2::new(marker.left() + 5.0, marker.center().y),
                    1.75,
                    p.muted,
                );
            }
        }
    };
    let mut last = None;
    // How many quotes the block above stood in, and where it ended: their
    // bars run on through the room between two blocks.
    let mut above = (0_u8, 0.0_f32);
    for (index, placed) in blocks.iter().enumerate() {
        let gap = if index == 0 { 0.0 } else { 6.0 };
        ui.add_space(match &placed.block {
            Block::Heading(level, _) if index > 0 => {
                if *level <= 2 || lines == Lines::Joined {
                    14.0
                } else {
                    10.0
                }
            }
            Block::Heading(..) => 0.0,
            // The first item of release notes follows its heading closely.
            Block::Bullet(_) | Block::Numbered(..) | Block::Task(..) => 6.0,
            Block::Text(_) if lines == Lines::Joined => 6.0,
            Block::Rule if index > 0 => 10.0,
            _ => gap,
        });
        let (left, top) = (ui.cursor().left(), ui.cursor().top());
        let base = document.map(|(path, _, _)| path);
        let links = actions.as_deref_mut().map(|actions| (actions, p, base));
        let block = |ui: &mut Ui| match &placed.block {
            Block::Heading(level, text) => {
                let face = Face::heading(*level, lines, ink, p.secondary);
                let rect = label(ui, lay_at(text, &face, links.is_some(), base), index, links);
                if document
                    .and_then(|(_, _, anchor)| anchor)
                    .is_some_and(|anchor| anchor == heading_anchor(text))
                {
                    ui.scroll_to_rect(rect, Some(Align::Min));
                }
                rect
            }
            Block::Bullet(text) | Block::Numbered(_, text) | Block::Task(_, text) => {
                ui.horizontal_top(|ui| {
                    marker(ui, &placed.block);
                    let laid = lay_at(text, &Face::body(ink), links.is_some(), base);
                    label(ui, laid, index, links)
                })
                .inner
            }
            Block::Code(code) => {
                if let Some((_, document, _)) = document {
                    return code_block(ui, p, ink, code, index, &document.code_controls[&index]);
                }
                Frame::new()
                    .fill(p.control)
                    .corner_radius(theme::metrics::CONTROL_RADIUS)
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        // Long lines wrap: a panel does not scroll sideways.
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(code)
                                    .font(FontId::monospace(12.0))
                                    .color(ink),
                            )
                            .wrap(),
                        );
                    })
                    .response
                    .rect
            }
            Block::Image(image) => {
                let picture = document.and_then(|(_, document, _)| document.pictures.get(&index));
                let rect = if let Some(picture) = picture {
                    let mut size = vec2(picture.pixels[0] as f32, picture.pixels[1] as f32);
                    match (image.width, image.height) {
                        (Some(width), Some(height)) => size = vec2(width as f32, height as f32),
                        (Some(width), None) => size *= width as f32 / size.x,
                        (None, Some(height)) => size *= height as f32 / size.y,
                        _ => {}
                    }
                    size *= (ui.available_width() / size.x).min(1.0);
                    let response =
                        ui.add(egui::Image::new(&picture.texture).fit_to_exact_size(size));
                    response
                        .widget_info(|| WidgetInfo::labeled(WidgetType::Image, true, &image.alt));
                    ui.painter().rect_stroke(
                        response.rect,
                        0,
                        Stroke::new(
                            1.0,
                            if p.dark {
                                Color32::from_white_alpha(25)
                            } else {
                                Color32::from_black_alpha(25)
                            },
                        ),
                        egui::StrokeKind::Inside,
                    );
                    response.on_hover_text(&image.alt).rect
                } else {
                    let loading = document.is_some_and(|(_, document, _)| document.loading_images);
                    let status = if loading {
                        "Loading image…"
                    } else {
                        "Image unavailable"
                    };
                    let caption = if image.alt.is_empty() {
                        status.to_owned()
                    } else {
                        format!("{status} · {}", image.alt)
                    };
                    ui.label(egui::RichText::new(caption).color(p.muted))
                        .on_hover_text(if loading {
                            "Pictures load in the background"
                        } else {
                            "Could not load this picture or the document's image limit was reached"
                        })
                        .rect
                };
                if let Some((actions, _, _)) = links
                    && let Some(target) =
                        image.link.as_deref().and_then(|link| target_at(link, base))
                {
                    let response = ui.interact(
                        rect,
                        ui.id().with(("document-image-link", index)),
                        Sense::click(),
                    );
                    response
                        .widget_info(|| WidgetInfo::labeled(WidgetType::Link, true, &image.alt));
                    if response.has_focus() {
                        focus_ring(ui.painter(), rect, 3, p);
                    }
                    if response
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text(image.link.as_deref().unwrap_or_default())
                        .clicked()
                    {
                        actions.push(target.action());
                    }
                }
                rect
            }
            Block::Rule => {
                let (_, rule) = ui.allocate_space(vec2(ui.available_width(), 1.0));
                ui.painter()
                    .line_segment([rule.left_center(), rule.right_center()], p.hairline());
                rule
            }
            Block::Table(table) => grid(ui, p, ink, table, index, links),
            Block::Text(text) => {
                let laid = lay_at(text, &Face::body(ink), links.is_some(), base);
                label(ui, laid, index, links)
            }
        };
        let room = inset(placed);
        let rect = if room == 0.0 {
            block(ui)
        } else {
            ui.horizontal_top(|ui| {
                ui.add_space(room);
                ui.vertical(block).inner
            })
            .inner
        };
        let bottom = ui.min_rect().bottom();
        for level in 0..placed.quote {
            let x = left + f32::from(level) * QUOTE + 1.0;
            let from = if level < above.0 { above.1 } else { top };
            ui.painter().rect_filled(
                Rect::from_min_max(Pos2::new(x, from), Pos2::new(x + 2.0, bottom)),
                1,
                p.border,
            );
        }
        above = (placed.quote, bottom);
        match &placed.block {
            Block::Heading(..) => ui.add_space(2.0),
            Block::Rule => ui.add_space(4.0),
            _ => {}
        }
        last = Some(rect);
    }
    last
}

fn code_block(
    ui: &mut Ui,
    p: Palette,
    ink: Color32,
    code: &str,
    index: usize,
    controls: &CodeControls,
) -> Rect {
    let id = ui.id().with(("document-code", index));
    let mut wrap = controls.wrap.get();
    let copied = controls.copied_until.get();
    let now = ui.input(|input| input.time);
    let rect = Frame::new()
        .fill(p.control)
        .corner_radius(theme::metrics::CONTROL_RADIUS)
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Code")
                        .font(theme::regular(11.0))
                        .color(p.muted),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if icons::button(
                        ui,
                        if copied > now {
                            Icon::Check
                        } else {
                            Icon::Copy
                        },
                        "Copy code block",
                    )
                    .clicked()
                    {
                        crate::platform::clipboard::copy(ui.ctx(), code.to_owned());
                        controls.copied_until.set(now + 1.2);
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(1200));
                    }
                    let surface = ui.painter().add(egui::Shape::Noop);
                    let response = icons::button(ui, Icon::Wrap, "Toggle code line wrapping");
                    if wrap {
                        ui.painter().set(
                            surface,
                            egui::Shape::rect_filled(response.rect.shrink(1.0), 7, p.pressed),
                        );
                    }
                    if response.clicked() {
                        wrap = !wrap;
                    }
                });
            });
            let text = egui::RichText::new(code)
                .font(FontId::monospace(12.0))
                .color(ink);
            if wrap {
                ui.add(egui::Label::new(text).wrap());
            } else {
                egui::ScrollArea::horizontal()
                    .id_salt(id.with("scroll"))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend));
                    });
            }
        })
        .response
        .rect;
    controls.wrap.set(wrap);
    rect
}

/// A table: its header in the medium weight over a line, its rows parted
/// by hairlines. A column is as wide as its longest cell while the table
/// fits; where it does not, cells wrap, and a table too wide even so is
/// moved sideways inside its own width.
fn grid(
    ui: &mut Ui,
    p: Palette,
    ink: Color32,
    table: &Table,
    salt: usize,
    mut actions: Option<(&mut Vec<Action>, Palette, Option<&Path>)>,
) -> Rect {
    /// The room around a cell's text.
    const PAD: egui::Vec2 = vec2(8.0, 4.0);
    /// What a column that wraps keeps, and gets where nothing fits.
    const NARROW: f32 = 56.0;
    const WRAPPED: f32 = 120.0;
    let live = actions.is_some();
    let document = actions.as_ref().and_then(|(_, _, path)| *path);
    let rows = || std::iter::once(&table.head).chain(&table.rows);
    let cell = |head: bool, text: &str, width: f32, align: Align| {
        let mut laid = lay_at(text, &Face::cell(head, ink), live, document);
        laid.job.wrap.max_width = width;
        laid.job.halign = align;
        laid
    };
    let columns = table.aligns.len();
    let wanted: Vec<f32> = (0..columns)
        .map(|column| {
            rows()
                .enumerate()
                .map(|(row, cells)| {
                    let laid = cell(row == 0, &cells[column], f32::INFINITY, Align::Min);
                    ui.painter().layout_job(laid.job).size().x.ceil()
                })
                .fold(0.0, f32::max)
        })
        .collect();
    let room = ui.available_width() - columns as f32 * 2.0 * PAD.x;
    let least: Vec<f32> = wanted.iter().map(|width| width.min(NARROW)).collect();
    let (all, kept): (f32, f32) = (wanted.iter().sum(), least.iter().sum());
    let widths: Vec<f32> = if all <= room {
        wanted
    } else if kept <= room {
        // What is left after each column's least goes to those that want
        // more, by how much more they want.
        let share = (room - kept) / (all - kept);
        wanted
            .iter()
            .zip(&least)
            .map(|(wanted, least)| (least + (wanted - least) * share).floor())
            .collect()
    } else {
        wanted.iter().map(|width| width.min(WRAPPED)).collect()
    };
    let width = widths.iter().sum::<f32>() + columns as f32 * 2.0 * PAD.x;
    let mut draw = |ui: &mut Ui| {
        let heights: Vec<f32> = rows()
            .enumerate()
            .map(|(row, cells)| {
                let tallest = (0..columns)
                    .map(|column| {
                        let laid = cell(row == 0, &cells[column], widths[column], Align::Min);
                        ui.painter().layout_job(laid.job).size().y
                    })
                    .fold(0.0, f32::max);
                tallest.ceil() + 2.0 * PAD.y
            })
            .collect();
        let (_, rect) = ui.allocate_space(vec2(width, heights.iter().sum()));
        let mut top = rect.top();
        for (row, cells) in rows().enumerate() {
            let mut left = rect.left();
            for column in 0..columns {
                let align = table.aligns[column];
                let inner = Rect::from_min_size(
                    Pos2::new(left + PAD.x, top + PAD.y),
                    vec2(widths[column], heights[row] - 2.0 * PAD.y),
                );
                place(
                    ui,
                    inner,
                    Layout::top_down(align),
                    ("markup-cell", salt, row, column),
                    |ui| {
                        let laid = cell(row == 0, &cells[column], widths[column], align);
                        let links = actions
                            .as_mut()
                            .map(|(actions, p, path)| (&mut **actions, *p, *path));
                        label(ui, laid, (salt, row, column), links);
                    },
                );
                left += widths[column] + 2.0 * PAD.x;
            }
            top += heights[row];
            if row + 1 < heights.len() {
                // The line under the header is the stronger one.
                let ink = if row == 0 { p.border } else { p.separator };
                ui.painter().line_segment(
                    [Pos2::new(rect.left(), top), Pos2::new(rect.right(), top)],
                    Stroke::new(1.0, ink),
                );
            }
        }
        rect
    };
    if width <= ui.available_width() + 0.5 {
        return draw(ui);
    }
    egui::ScrollArea::horizontal()
        .id_salt(Id::new(("markup-table", salt)))
        .show(ui, |ui| draw(ui))
        .inner_rect
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_controls_belong_to_the_current_document_and_survive_unchanged_reload() {
        let before = Document::new(document_blocks("```sh\nfirst\n```\n\n```sh\nsecond\n```"));
        before.code_controls[&0].wrap.set(false);
        before.code_controls[&0].copied_until.set(1.2);
        let mut after = Document::new(document_blocks("```sh\nfirst\n```"));
        after.retain_state_from(&before);
        assert_eq!(
            after.code_controls.len(),
            1,
            "removed blocks retain no controls"
        );
        assert!(!after.code_controls[&0].wrap.get());
        assert_eq!(after.code_controls[&0].copied_until.get(), 1.2);
        let mut changed = Document::new(document_blocks("```sh\nchanged\n```"));
        changed.retain_state_from(&before);
        assert!(changed.code_controls[&0].wrap.get());
        assert_eq!(changed.code_controls[&0].copied_until.get(), 0.0);
    }

    #[test]
    fn unchanged_images_keep_their_layout_while_a_reload_is_pending() {
        let ctx = egui::Context::default();
        let mut before = Document::new(document_blocks("![Logo](logo.png)"));
        let texture = ctx.load_texture(
            "reload-test",
            egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let id = texture.id();
        before.pictures.insert(
            0,
            Picture {
                texture,
                pixels: [96, 96],
            },
        );
        let mut after = Document::new(document_blocks("![Logo](logo.png)"));
        after.retain_state_from(&before);
        assert_eq!(after.pictures[&0].texture.id(), id);
        assert_eq!(after.pictures[&0].pixels, [96, 96]);
        assert!(
            after.loading_images,
            "the retained picture is still refreshed"
        );
        let mut changed = Document::new(document_blocks("![Logo](replacement.png)"));
        changed.retain_state_from(&before);
        assert!(changed.pictures.is_empty());
    }

    #[test]
    fn document_images_parse_markdown_and_html_without_interpreting_code() {
        let parsed = document_blocks(
            "Before ![Plot](assets/a%20b.png) after.\n\n<img HEIGHT='96' src=assets/logo.png alt=\"Neptune &amp; logo\" width=96>\n\n`![literal](code.png)`\n\n```html\n<img src=code.png>\n```\n",
        );
        assert!(
            matches!(&parsed[1].block, Block::Image(image) if image.source == "assets/a%20b.png" && image.alt == "Plot")
        );
        assert!(
            matches!(&parsed[3].block, Block::Image(image) if image.width == Some(96) && image.height == Some(96) && image.alt == "Neptune & logo")
        );
        assert_eq!(
            parsed
                .iter()
                .filter(|block| matches!(block.block, Block::Image(_)))
                .count(),
            2
        );
        let list = document_blocks(
            "- A [![Logo](logo.png)](https://neptune.rs) after\n\n## ![Heading image](heading.png)",
        );
        assert!(
            matches!(&list[1].block, Block::Image(image) if image.link.as_deref() == Some("https://neptune.rs"))
        );
        assert_eq!(list[1].inset, 1);
        assert!(matches!(&list[3].block, Block::Image(image) if image.source == "heading.png"));
        for cut in "<img src=\"broken.png\" alt='still typing'>"
            .char_indices()
            .map(|(at, _)| at)
        {
            document_blocks(&"<img src=\"broken.png\" alt='still typing'>"[..cut]);
        }
    }

    #[test]
    fn document_links_resolve_from_the_file_and_reject_executable_schemes() {
        let file = Path::new("/project/README.md");
        assert_eq!(
            target_at("docs/user%20guide.md#settings", Some(file)),
            Some(Target::Document(
                "/project/docs/user guide.md".into(),
                Some("settings".into())
            ))
        );
        assert_eq!(
            target_at("#quick-start", Some(file)),
            Some(Target::Document(file.into(), Some("quick-start".into())))
        );
        assert!(matches!(
            target_at("https://neptune.rs", Some(file)),
            Some(Target::Web(_))
        ));
        for unsafe_link in [
            "javascript:alert(1)",
            "data:text/html,test",
            "relative%00name",
            "https://",
            "broken%xy",
        ] {
            assert!(
                target_at(unsafe_link, Some(file)).is_none(),
                "{unsafe_link}"
            );
        }
        assert_eq!(
            heading_anchor("Quick **start** & setup!"),
            "quick-start--setup"
        );
    }

    /// The blocks of `text` without where they stand.
    fn kinds(text: &str, lines: Lines) -> Vec<Block> {
        blocks(text, lines)
            .into_iter()
            .map(|placed| placed.block)
            .collect()
    }
    /// The blocks of a reply as (quotes, levels in, block).
    fn placed(text: &str) -> Vec<(u8, u8, Block)> {
        blocks(text, Lines::Kept)
            .into_iter()
            .map(|placed| (placed.quote, placed.inset, placed.block))
            .collect()
    }
    /// The runs of `text` with what sets them written as tags: b for
    /// strong, i for emphasis, s for a strike, c for code and a for a link.
    fn runs(text: &str) -> String {
        let mut out = String::new();
        for Span { text, style } in inline(text, true).spans {
            let tags = [
                (style.strong, "b"),
                (style.emphasis, "i"),
                (style.strike, "s"),
                (style.code, "c"),
                (style.link.is_some(), "a"),
            ];
            for (_, tag) in tags.iter().filter(|(on, _)| *on) {
                out.push_str(&format!("<{tag}>"));
            }
            out.push_str(&text);
            for (_, tag) in tags.iter().rev().filter(|(on, _)| *on) {
                out.push_str(&format!("</{tag}>"));
            }
        }
        out
    }
    fn links(text: &str) -> Vec<String> {
        inline(text, true).links
    }

    #[test]
    fn hard_wrapped_notes_become_whole_blocks() {
        let notes = "### What's New\n\n- Open a tab with\n  Ctrl+Shift+T, and drag tabs\n  between splits.\n- Shorter rows.\n\nA closing line\nin two parts.\n\nSeparate.";
        assert_eq!(
            kinds(notes, Lines::Joined),
            [
                Block::Heading(3, "What's New".into()),
                Block::Bullet("Open a tab with Ctrl+Shift+T, and drag tabs between splits.".into()),
                Block::Bullet("Shorter rows.".into()),
                Block::Text("A closing line in two parts.".into()),
                Block::Text("Separate.".into()),
            ]
        );
    }

    #[test]
    fn a_reply_keeps_its_line_breaks_its_numbers_and_its_code() {
        let reply = "Plan:\nthree steps\n\n1. auth\n2) tests\n   for the API\n2026. not a list\n\n```sh\ncargo test\n  --locked\n```\ndone\n```\nunclosed";
        assert_eq!(
            kinds(reply, Lines::Kept),
            [
                Block::Text("Plan:\nthree steps".into()),
                Block::Numbered("1".into(), "auth".into()),
                Block::Numbered("2".into(), "tests\nfor the API\n2026. not a list".into()),
                Block::Code("cargo test\n  --locked".into()),
                Block::Text("done".into()),
                Block::Code("unclosed".into()),
            ]
        );
        assert_eq!(numbered("12. x"), Some(("12", "x")));
        assert_eq!(numbered(". x"), None);
        assert_eq!(numbered("1.x"), None);
    }

    #[test]
    fn headings_have_levels_and_rules_part_what_they_stand_between() {
        let text = "# One\n## Two ##\n###### Six\n####### seven\n#123\n#tag and text\n#\n\nTitle\nof two lines\n===\n\nSecond\n---\n\n---\n***\n_ _ _\n--\n\n=== alone";
        assert_eq!(
            kinds(text, Lines::Kept),
            [
                Block::Heading(1, "One".into()),
                Block::Heading(2, "Two".into()),
                Block::Heading(6, "Six".into()),
                // Not headings: seven signs, and signs with no space after.
                Block::Text("####### seven\n#123\n#tag and text\n#".into()),
                Block::Heading(1, "Title of two lines".into()),
                Block::Heading(2, "Second".into()),
                Block::Rule,
                Block::Rule,
                Block::Rule,
                Block::Text("--".into()),
                Block::Text("=== alone".into()),
            ]
        );
        // A sign inside a heading's text stays, a closing run does not.
        assert_eq!(heading("## C# and F#"), Some((2, "C# and F#")));
        assert_eq!(heading("## Done ###"), Some((2, "Done")));
    }

    #[test]
    fn lists_nest_by_how_far_they_are_set_in_and_tasks_have_boxes() {
        let text = "- one\n  - two\n    1. three\n       - four\n  + back to two\n* top\n\n  under top\n\n- [ ] open\n- [x] done\n- [X] done too\n- [] not a task\n\n1. first\n   - child\n\n     of the child\n2. second\n\nout";
        let bullet = |text: &str| Block::Bullet(text.into());
        assert_eq!(
            placed(text),
            [
                (0, 0, bullet("one")),
                (0, 1, bullet("two")),
                (0, 2, Block::Numbered("1".into(), "three".into())),
                (0, 3, bullet("four")),
                (0, 1, bullet("back to two")),
                (0, 0, bullet("top")),
                // A paragraph of an item stands under that item's text.
                (0, 1, Block::Text("under top".into())),
                (0, 0, Block::Task(false, "open".into())),
                (0, 0, Block::Task(true, "done".into())),
                (0, 0, Block::Task(true, "done too".into())),
                (0, 0, bullet("[] not a task")),
                (0, 0, Block::Numbered("1".into(), "first".into())),
                (0, 1, bullet("child")),
                (0, 2, Block::Text("of the child".into())),
                (0, 0, Block::Numbered("2".into(), "second".into())),
                (0, 0, Block::Text("out".into())),
            ]
        );
        // Deeper than a panel has room for is drawn at the deepest level.
        let deep = (0..9).fold(String::new(), |text, level| {
            format!("{text}{}- {level}\n", "  ".repeat(level))
        });
        let insets: Vec<u8> = blocks(&deep, Lines::Kept)
            .iter()
            .map(|placed| placed.inset)
            .collect();
        assert_eq!(insets, [0, 1, 2, 3, 4, 4, 4, 4, 4]);
        // Marks that are not items.
        assert_eq!(kinds("-no\n*no*\n+1", Lines::Kept).len(), 1);
    }

    #[test]
    fn quotes_hold_blocks_of_their_own_and_nest() {
        let text = "> said\n> on two lines\n>\n> - item\n>   - nested\n> > deeper\n> ```\n> code\n>   kept\n> ```\n> | a | b |\n> |---|---|\n> | 1 | 2 |\nout\n>not far in";
        assert_eq!(
            placed(text),
            [
                (1, 0, Block::Text("said\non two lines".into())),
                (1, 0, Block::Bullet("item".into())),
                (1, 1, Block::Bullet("nested".into())),
                (2, 0, Block::Text("deeper".into())),
                (1, 0, Block::Code("code\n  kept".into())),
                (
                    1,
                    0,
                    Block::Table(Table {
                        aligns: vec![Align::Min, Align::Min],
                        head: vec!["a".into(), "b".into()],
                        rows: vec![vec!["1".into(), "2".into()]],
                    })
                ),
                (0, 0, Block::Text("out".into())),
                (1, 0, Block::Text("not far in".into())),
            ]
        );
        // A fence ends with the quote it stands in.
        assert_eq!(
            placed("> ```\n> a\nb"),
            [
                (1, 0, Block::Code("a".into())),
                (0, 0, Block::Text("b".into()))
            ]
        );
    }

    #[test]
    fn code_is_kept_as_written_whatever_fences_it() {
        // Its marks are not read, and its lines keep how far in they are.
        let text = "```rust\nfn f() {\n    **x**\n}\n# not a heading\n- not a list\n```\n~~~\n```\ninner\n```\n~~~\n````md\n```\nstill code\n```\n````\n```` not closed by three\n```\n````";
        assert_eq!(
            kinds(text, Lines::Kept),
            [
                Block::Code("fn f() {\n    **x**\n}\n# not a heading\n- not a list".into()),
                Block::Code("```\ninner\n```".into()),
                Block::Code("```\nstill code\n```".into()),
                Block::Code("```".into()),
            ]
        );
        // A fence of an item stands under the item, without the item's
        // own indentation in its lines.
        assert_eq!(
            placed("1. run\n   ```sh\n   cargo test\n     --locked\n   ```\n   then"),
            [
                (0, 0, Block::Numbered("1".into(), "run".into())),
                (0, 1, Block::Code("cargo test\n  --locked".into())),
                (0, 1, Block::Text("then".into())),
            ]
        );
        // Four in after a blank line is code too, to the line that is not.
        assert_eq!(
            kinds(
                "Run:\n\n    cargo test\n\n      --locked\n\nthen",
                Lines::Kept
            ),
            [
                Block::Text("Run:".into()),
                Block::Code("cargo test\n\n  --locked".into()),
                Block::Text("then".into()),
            ]
        );
        // Backticks that open a line and close on it are a span, not a fence.
        assert_eq!(
            kinds("```not a fence``` here", Lines::Kept),
            [Block::Text("```not a fence``` here".into())]
        );
        // One that is being written is code as far as it has come.
        assert_eq!(kinds("```", Lines::Kept), [Block::Code(String::new())]);
    }

    #[test]
    fn tables_have_a_cell_for_each_column_and_half_a_table_is_text() {
        let text = "Stack:\n| Area | Stack | Notes |\n|:---|:---:|---:|\n| Mobile | Expo 57, **RN** | a \\| b |\n| API | `hono` |\n| Web | Next | x | extra |\nafter";
        assert_eq!(
            kinds(text, Lines::Kept),
            [
                Block::Text("Stack:".into()),
                Block::Table(Table {
                    aligns: vec![Align::Min, Align::Center, Align::Max],
                    head: vec!["Area".into(), "Stack".into(), "Notes".into()],
                    rows: vec![
                        vec!["Mobile".into(), "Expo 57, **RN**".into(), "a | b".into()],
                        vec!["API".into(), "`hono`".into(), String::new()],
                        vec!["Web".into(), "Next".into(), "x".into()],
                    ],
                }),
                Block::Text("after".into()),
            ]
        );
        // No bars at the ends, and one column.
        assert_eq!(
            kinds(
                "a | b\n- | -\n1 | 2\n\n| one |\n| --- |\n| 1 |",
                Lines::Kept
            )
            .len(),
            2
        );
        // Until its dashes come a header is text, and so is a table whose
        // dashes do not match its header.
        for half in [
            "| Area | Stack |",
            "| Area | Stack |\n|---",
            "| Area | Stack |\n|---|",
            "| Area | Stack |\n| 1 | 2 |",
            "a | b\n---",
        ] {
            let blocks = kinds(half, Lines::Kept);
            assert!(
                !blocks.iter().any(|block| matches!(block, Block::Table(_))),
                "{half:?} is {blocks:?}"
            );
        }
        assert!(matches!(
            kinds("| Area | Stack |\n|---|---|", Lines::Kept).as_slice(),
            [Block::Table(table)] if table.rows.is_empty()
        ));
    }

    #[test]
    fn marks_pair_one_by_one_and_one_without_a_partner_is_text() {
        for (text, wanted) in [
            (
                "a **strong** and __strong__ word",
                "a <b>strong</b> and <b>strong</b> word",
            ),
            ("*em* and _em_", "<i>em</i> and <i>em</i>"),
            (
                "***both*** and **_both_**",
                "<b><i>both</i></b> and <b><i>both</i></b>",
            ),
            (
                "~~gone~~ and ~not~ and ~~~no~~~",
                "<s>gone</s> and ~not~ and ~~~no~~~",
            ),
            // A mark that the card cut the partner of takes no pair with it.
            (
                "**P2, measured:** discovery took **",
                "<b>P2, measured:</b> discovery took **",
            ),
            ("**open and **shut**", "**open and <b>shut</b>"),
            ("*a **b** c*", "<i>a </i><b><i>b</i></b><i> c</i>"),
            (
                "**bold with `code` inside**",
                "<b>bold with </b><b><c>code</c></b><b> inside</b>",
            ),
            // Arithmetic and names are not marks.
            ("2 * 3 * 4 and 2 ** 3", "2 * 3 * 4 and 2 ** 3"),
            (
                "snake_case_name and __init__.py",
                "snake_case_name and <b>init</b>.py",
            ),
            ("a_b_c _d_", "a_b_c <i>d</i>"),
            ("5*3*2", "5<i>3</i>2"),
            // Escaped marks are the characters themselves.
            (r"\*no\* \_no\_ \# \| \\ \a", r"*no* _no_ # | \ \a"),
            // Code: its marks are its own, and two backticks hold one.
            (
                "`a ** b` and `` a ` b `` and ` alone",
                "<c>a ** b</c> and <c>a ` b</c> and ` alone",
            ),
            ("``unclosed ` here", "``unclosed ` here"),
            // Tags are shown, never read; a break breaks the line.
            (
                "<b>x</b> a<br>b<BR/>c &lt;p&gt; &amp; &unknown;",
                "<b>x</b> a\nb\nc <p> & &unknown;",
            ),
            // An emphasis does not cross the one it would have to cut.
            ("*a _b* c_", "<i>a _b</i> c_"),
        ] {
            assert_eq!(runs(text), wanted, "{text:?}");
        }
    }

    #[test]
    fn links_are_their_text_and_lead_only_where_a_person_may_go() {
        for (text, wanted, to) in [
            (
                "see [the docs](https://a.example/x) now",
                "see <a>the docs</a> now",
                vec!["https://a.example/x"],
            ),
            (
                "[**Full** report](/tmp/audit/report.md \"The report\")",
                "<b><a>Full</a></b><a> report</a>",
                vec!["/tmp/audit/report.md"],
            ),
            (
                "[a](<with space.md>) [b](x(1).md) [](https://e.example)",
                "<a>a</a> <a>b</a> <a>https://e.example</a>",
                vec!["with space.md", "x(1).md", "https://e.example"],
            ),
            (
                "<https://auto.example/a_b> and <div> and <mailto:x@y.z>",
                "<a>https://auto.example/a_b</a> and <div> and <mailto:x@y.z>",
                vec!["https://auto.example/a_b"],
            ),
            (
                "(https://bare.example/a_(b)). Then https://c.example/x_y, ok",
                "(<a>https://bare.example/a_(b)</a>). Then <a>https://c.example/x_y</a>, ok",
                vec!["https://bare.example/a_(b)", "https://c.example/x_y"],
            ),
            (
                "![a chart](https://img.example/c.png) ![](pic.png)",
                "<a>a chart</a> <a>pic.png</a>",
                vec!["https://img.example/c.png", "pic.png"],
            ),
            (
                "[`code` link](a.md) xhttps://no.example",
                "<c><a>code</a></c><a> link</a> xhttps://no.example",
                vec!["a.md"],
            ),
            // Half a link is text.
            (
                "[text](https://unclosed and [text] (x) and [t]",
                "[text](<a>https://unclosed</a> and [text] (x) and [t]",
                vec!["https://unclosed"],
            ),
            ("a[i] = b[j](k", "a[i] = b[j](k", vec![]),
        ] {
            assert_eq!(runs(text), wanted, "{text:?}");
            assert_eq!(links(text), to, "{text:?}");
        }
        let web = |url: &str| Some(Target::Web(WebLink::new(url).unwrap()));
        let file = |path: &str| Some(Target::File(PathBuf::from(path)));
        assert_eq!(
            target("https://a.example/x?y#z"),
            web("https://a.example/x?y#z")
        );
        assert_eq!(target("HTTP://a.example"), web("HTTP://a.example"));
        if cfg!(unix) {
            assert_eq!(target("/tmp/audit/report.md"), file("/tmp/audit/report.md"));
            assert_eq!(target("/src/ui/chat.rs:42:7"), file("/src/ui/chat.rs"));
            assert_eq!(target("/src/ui/chat.rs#L42-L50"), file("/src/ui/chat.rs"));
            assert_eq!(target("file:///tmp/a%20b.md"), file("/tmp/a b.md"));
            assert_eq!(target("file://localhost/tmp/a.md"), file("/tmp/a.md"));
        }
        // Nothing else is ever opened: other schemes, a path that is not
        // whole, an address with something hidden in it.
        for inert in [
            "javascript:alert(1)",
            "data:text/html,x",
            "vscode://file/x",
            "mailto:a@b.c",
            "src/ui/chat.rs",
            "./a.md",
            "~/a.md",
            "#anchor",
            "",
            "file://host/share/a.md%",
            "file:///tmp/%zz",
            "https://",
            "https://a.example/x y",
            "/tmp/a\u{7}b",
        ] {
            assert_eq!(target(inert), None, "{inert:?}");
        }
    }

    #[test]
    fn spans_keep_every_character_of_text_with_unpaired_marks() {
        let text = |job: LayoutJob| job.text;
        assert_eq!(
            text(body("run `chmod u+x` first", Color32::WHITE)),
            "run chmod u+x first"
        );
        assert_eq!(text(body("a ` alone", Color32::WHITE)), "a ` alone");
        assert_eq!(
            text(body("a **strong** word", Color32::WHITE)),
            "a strong word"
        );
        assert_eq!(text(body("2 ** 3", Color32::WHITE)), "2 ** 3");
        // Marks inside code are the code's.
        assert_eq!(text(body("`a ** b`", Color32::WHITE)), "a ** b");
        let job = body("x **y** `z`", Color32::WHITE);
        let fonts: Vec<_> = job
            .sections
            .iter()
            .filter(|section| !section.byte_range.is_empty())
            .map(|section| section.format.font_id.clone())
            .collect();
        assert_eq!(
            fonts,
            [
                theme::regular(13.0),
                theme::medium(13.0),
                theme::regular(13.0),
                FontId::monospace(12.0)
            ]
        );
        // Emphasis is slanted, a strike is struck, and a link is underlined
        // only where it can be pressed.
        let job = lay(
            "*e* ~~s~~ [l](https://a.example) [r](rel.md)",
            &Face::body(Color32::WHITE),
            true,
        );
        let set: Vec<_> = job
            .job
            .sections
            .iter()
            .map(|section| {
                let format = &section.format;
                (
                    &job.job.text[section.byte_range.start.0..section.byte_range.end.0],
                    format.italics,
                    format.strikethrough != Stroke::NONE,
                    format.underline != Stroke::NONE,
                )
            })
            .collect();
        assert_eq!(
            set,
            [
                ("e", true, false, false),
                (" ", false, false, false),
                ("s", false, true, false),
                (" ", false, false, false),
                ("l", false, false, true),
                (" r", false, false, false),
            ]
        );
        // Which characters a link is, in the text as it is read.
        assert_eq!(
            job.links,
            [
                (4..5, "https://a.example".to_owned()),
                (6..7, "rel.md".to_owned())
            ]
        );
        // Where no link is pressed none is underlined.
        let inert = body("[l](https://a.example)", Color32::WHITE);
        assert!(
            inert
                .sections
                .iter()
                .all(|section| section.format.underline == Stroke::NONE)
        );
    }

    #[test]
    fn a_preview_is_the_words_without_their_marks() {
        let report = "## Tech stack\n\n**P2, measured:** discovery of `expo` took *long*.\n\n| Area | Stack |\n|---|---|\n| Mobile | Expo 57 |\n\n- [x] done\n1. [Full report](/tmp/report.md)\n\n---\n```sh\ncargo test\n```\n> quoted **and cut";
        assert_eq!(
            plain(report),
            "Tech stack P2, measured: discovery of expo took long. Area Stack Mobile Expo 57 done Full report cargo test quoted and cut"
        );
        // What is no mark stays.
        assert_eq!(plain("2 * 3 and a_b"), "2 * 3 and a_b");
        assert_eq!(plain("  \n\n"), "");
    }

    const REPORT: &str = "# Títle ünïcode 日本語\n\nSome **bold**, *em*, ~~gone~~, `code` and [a link](https://a.example/x \"t\") — ok.\n\n> quote with **mark\n> > deeper `tick\n\n- one\n  - two [ ] \\* \\\n    1) three ![alt](x.png)\n- [x] task <br> <https://b.example>\n\n| Área | Stack |\n|:--|--:|\n| a \\| b | `c` |\n| é |\n\n~~~~py\nprint('```')\n~~~~\n\n    indented\n\nTitle\n===\n***\nhttps://c.example/a_(b)). &amp; &#39; end\\";

    #[test]
    fn a_text_cut_anywhere_is_laid_out_without_a_fault() {
        // As it comes while it is written, and as a card would cut it:
        // every beginning of it, and every end.
        let cuts: Vec<usize> = REPORT.char_indices().map(|(at, _)| at).collect();
        for &cut in &cuts {
            for part in [&REPORT[..cut], &REPORT[cut..]] {
                for lines in [Lines::Kept, Lines::Joined] {
                    for placed in blocks(part, lines) {
                        let texts: Vec<&String> = match &placed.block {
                            Block::Heading(_, text)
                            | Block::Bullet(text)
                            | Block::Numbered(_, text)
                            | Block::Task(_, text)
                            | Block::Text(text) => vec![text],
                            Block::Table(table) => {
                                assert!(
                                    table.rows.iter().all(|row| row.len() == table.aligns.len())
                                );
                                table
                                    .head
                                    .iter()
                                    .chain(table.rows.iter().flatten())
                                    .collect()
                            }
                            _ => Vec::new(),
                        };
                        for text in texts {
                            let laid = lay(text, &Face::body(Color32::WHITE), true);
                            let chars = laid.job.text.chars().count();
                            assert!(laid.links.iter().all(|(range, _)| range.end <= chars));
                        }
                    }
                }
                plain(part);
            }
        }
        // Marks alone, in runs, and against characters of several bytes.
        for odd in [
            "*",
            "**",
            "***",
            "_",
            "~~",
            "`",
            "``",
            "[",
            "![",
            "[]",
            "[](",
            "[]()",
            "![]()",
            "<",
            "<>",
            "\\",
            "&",
            "|",
            "|\n|-",
            "|\n-|-",
            ">",
            ">>>>>>>>",
            "#",
            "- ",
            "-",
            "1.",
            "```",
            "~~~\n",
            "h",
            "http://",
            "é*é*é",
            "*é",
            "é_",
            "`é",
            "[é](é)",
            "<é>",
            "\\é",
            "日本語**日本語**",
            "**日本語",
            "~~é",
            "|é|\n|-|\n|é",
            "\u{feff}# x",
            "a\r\nb\r\n- c\r\n",
            "\t- tab\n\t\tcode",
            "[a](b \"c",
            "[a](<b",
            "[a]( b ",
            "[[a]](b)",
            "[a](b)(c)[d](e)",
            "](",
            "*_*_*_*_",
            "**a*b**c*",
            "~~a~~~~b~~",
            "`` ` ``` ` ``",
            "- - - -",
            "> > > >",
        ] {
            for lines in [Lines::Kept, Lines::Joined] {
                blocks(odd, lines);
            }
            lay(odd, &Face::body(Color32::WHITE), true);
            plain(odd);
        }
        // Many marks without a partner do not make the work grow with
        // their square, and their counts do not overflow.
        let many = format!("{}{}", "*a ".repeat(4000), " b*".repeat(4000));
        let started = std::time::Instant::now();
        inline(&many, true);
        inline(&format!("{}x{}", "*".repeat(3000), "*".repeat(3000)), true);
        inline(&"[".repeat(3000), true);
        inline(&"`a ".repeat(3000), true);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    const WINDOW: Rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(400.0, 3000.0));

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        theme::apply(&ctx, &crate::config::Config::default());
        ctx
    }
    /// Lays `text` out as a reply in a column of `width`, and returns what
    /// its links asked for and what was drawn.
    fn frame(
        ctx: &egui::Context,
        text: &str,
        width: f32,
        events: Vec<egui::Event>,
    ) -> (Vec<Action>, egui::FullOutput) {
        let mut actions = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(WINDOW),
                events,
                ..Default::default()
            },
            |ui| {
                let p = Palette::for_config(&crate::config::Config::default());
                ui.set_width(width);
                show_in(ui, p, text, Lines::Kept, p.fg, &mut actions);
            },
        );
        output.textures_delta.clear();
        (actions, output)
    }
    /// Every text drawn, with where it is and the face it begins in.
    fn drawn(output: &egui::FullOutput) -> Vec<(String, Rect, FontId)> {
        fn collect(shape: &egui::Shape, found: &mut Vec<(String, Rect, FontId)>) {
            match shape {
                egui::Shape::Text(text) => found.push((
                    text.galley.text().to_owned(),
                    text.visual_bounding_rect(),
                    text.galley
                        .job
                        .sections
                        .first()
                        .map_or(FontId::default(), |section| section.format.font_id.clone()),
                )),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect(shape, found)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut found);
        }
        found
    }

    #[test]
    fn document_links_emit_browser_and_relative_preview_actions_when_clicked() {
        let ctx = context();
        let document = Document::new(document_blocks(
            "[Website](https://neptune.rs)\n\n[Guide](docs/usage.md#settings)\n\n[![Logo](logo.png)](https://neptune.rs/download)",
        ));
        let path = Path::new("/project/README.md");
        let run = |events| {
            let mut actions = Vec::new();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(WINDOW),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(300.0);
                    show_document(
                        ui,
                        Palette::for_config(&crate::config::Config::default()),
                        &document,
                        path,
                        None,
                        &mut actions,
                    );
                },
            );
            output.textures_delta.clear();
            (actions, output)
        };
        run(Vec::new());
        let (_, output) = run(Vec::new());
        let press = |label: &str| {
            let pos = drawn(&output)
                .into_iter()
                .find(|(text, ..)| text == label)
                .unwrap()
                .1
                .center();
            run(vec![egui::Event::PointerMoved(pos)]);
            run(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }]);
            run(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }])
            .0
        };
        assert!(
            matches!(press("Website").as_slice(), [Action::OpenLink(link)] if link.as_str() == "https://neptune.rs")
        );
        assert!(
            matches!(press("Guide").as_slice(), [Action::Explorer(super::super::explorer::Event::FollowLink { path, anchor })] if path == Path::new("/project/docs/usage.md") && anchor.as_deref() == Some("settings"))
        );
        assert!(
            matches!(press("Loading image… · Logo").as_slice(), [Action::OpenLink(link)] if link.as_str() == "https://neptune.rs/download")
        );
    }

    #[test]
    fn a_reply_is_laid_out_inside_a_narrow_panel_with_a_hierarchy() {
        let ctx = context();
        let text = "# First\n## Second\n### Third\n#### Fourth\n\nBody text.\n\n- one\n  - two\n    - three\n\n> quoted\n\n| Area | Stack | Notes |\n|:---|:---:|---:|\n| Mobile | Expo 57 with React Native and a long list of libraries | left |\n| API | Hono | a much longer note that has to wrap in its column |\n\n```\nlet a_very_long_line_of_code_that_does_not_fit = in_a_panel_of_three_hundred_points();\n```";
        frame(&ctx, text, 300.0, Vec::new());
        let (_, output) = frame(&ctx, text, 300.0, Vec::new());
        let drawn = drawn(&output);
        let of = |wanted: &str| {
            drawn
                .iter()
                .find(|(text, ..)| text == wanted)
                .unwrap_or_else(|| panic!("{wanted} in {drawn:?}"))
        };
        // The first levels are larger than the text, the others its size.
        let size = |wanted: &str| of(wanted).2.size;
        assert!(size("First") > size("Second") && size("Second") > size("Third"));
        assert_eq!(
            (size("Third"), size("Fourth"), size("Body text.")),
            (13.0, 13.0, 13.0)
        );
        assert_eq!(of("Third").2, theme::medium(13.0));
        // Each level of a list is further in, and a quote is set in too.
        let left = |wanted: &str| of(wanted).1.left();
        assert!(left("one") < left("two") && left("two") < left("three"));
        assert!(left("quoted") > left("Body text."));
        // Nothing runs out of the panel: the cells of the table wrap, each
        // in its column, and so does the code.
        for (text, rect, _) in &drawn {
            assert!(
                rect.right() <= left("Body text.") + 300.5,
                "{text:?} at {rect:?}"
            );
        }
        let (area, stack, notes) = (of("Area").1, of("Stack").1, of("Notes").1);
        assert!(area.right() < stack.left() && stack.right() < notes.left());
        assert_eq!(of("Area").2, theme::medium(12.0));
        let long = of("a much longer note that has to wrap in its column").1;
        assert!(long.height() > 20.0 && long.left() >= stack.right());
        // The last column is set against its trailing edge.
        assert!((of("left").1.right() - notes.right()).abs() < 1.5);
        // The bubble of a message is as wide as what it holds, and no wider
        // than it may be.
        let mut widths = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(WINDOW),
                ..Default::default()
            },
            |ui| {
                for text in [
                    "hi",
                    "- one\n  - nested item",
                    "# A heading",
                    "| a | b |\n|---|---|",
                    text,
                ] {
                    widths.push(width(ui, text, Lines::Kept, 240.0));
                }
            },
        );
        output.textures_delta.clear();
        assert!(
            widths[0] < 40.0 && widths[0] < widths[1] && widths[1] < 240.0,
            "{widths:?}"
        );
        assert!(widths[2] > widths[0] && widths[2] < 240.0);
        assert_eq!((widths[3], widths[4]), (240.0, 240.0));
    }

    #[test]
    fn a_table_too_wide_for_its_panel_moves_sideways_inside_it() {
        let ctx = context();
        let text = "before\n\n| One column | Two columns | Three columns | Four columns | Five columns | Six columns |\n|---|---|---|---|---|---|\n| aaaaaaaaaa | bbbbbbbbbbb | ccccccccccccc | dddddddddddd | eeeeeeeeeeee | fffffffffff |\n\nafter";
        frame(&ctx, text, 240.0, Vec::new());
        let (_, output) = frame(&ctx, text, 240.0, Vec::new());
        // What does not fit is clipped to the panel's width, not drawn
        // over what is beside it.
        let mut cells = 0;
        for clipped in &output.shapes {
            if let egui::Shape::Text(text) = &clipped.shape
                && text.galley.text().contains("columns")
            {
                cells += 1;
                assert!(
                    clipped.clip_rect.width() <= 240.5,
                    "{:?}",
                    clipped.clip_rect
                );
            }
        }
        // Only the columns in view are drawn at all.
        assert!((2..5).contains(&cells), "{cells} cells drawn");
        let drawn = drawn(&output);
        let of = |wanted: &str| drawn.iter().find(|(text, ..)| text == wanted).unwrap().1;
        assert!(of("after").top() > of("One column").bottom());
        assert!(of("after").top() - of("before").bottom() < 120.0);
    }

    #[test]
    fn a_pressed_link_asks_for_its_page_or_its_file_and_nothing_else_is_pressed() {
        let ctx = context();
        let text = "Read [the page](https://a.example/x) or [the file](/tmp/report.md), not [this](javascript:alert(1)) or [that](rel.md).\n\n| Where |\n|---|\n| <https://b.example> |";
        frame(&ctx, text, 300.0, Vec::new());
        let (_, output) = frame(&ctx, text, 300.0, Vec::new());
        // Where the words of a link are: under the galley they are part of.
        let at = |wanted: &str| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) => {
                        let galley = &text.galley;
                        let start = galley.text().find(wanted)?;
                        let chars = galley.text()[..start].chars().count() + 1;
                        let cursor = egui::text::CCursor::new(chars);
                        Some(text.pos + galley.pos_from_cursor(cursor).center().to_vec2())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{wanted} is not drawn"))
        };
        let press = |pos: Pos2| {
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            let mut actions = Vec::new();
            for events in [
                vec![egui::Event::PointerMoved(pos)],
                vec![button(true)],
                vec![button(false)],
            ] {
                actions.extend(frame(&ctx, text, 300.0, events).0);
            }
            actions
        };
        assert!(matches!(
            press(at("the page")).as_slice(),
            [Action::OpenLink(link)] if link.as_str() == "https://a.example/x"
        ));
        if cfg!(unix) {
            assert!(matches!(
                press(at("the file")).as_slice(),
                [Action::Explorer(crate::ui::explorer::Event::Open(path))]
                    if path == std::path::Path::new("/tmp/report.md")
            ));
        }
        assert!(matches!(
            press(at("https://b.example")).as_slice(),
            [Action::OpenLink(link)] if link.as_str() == "https://b.example"
        ));
        // What is not a page or a file of this computer, and plain words.
        for inert in ["this", "that", "Read", "Where"] {
            assert!(press(at(inert)).is_empty(), "{inert}");
        }
        // The text reads without its marks, which is what is copied.
        let texts: Vec<String> = drawn(&output).into_iter().map(|(text, ..)| text).collect();
        assert!(
            texts.contains(&"Read the page or the file, not this or that.".to_owned()),
            "{texts:?}"
        );
    }
}
