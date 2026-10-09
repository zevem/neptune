//! The file explorer's data. Listing folders, searching, reading previews and
//! changing files all happen on workers; frames only read what they reported.
//! Paths and file contents stay out of diagnostics and saved state.
use super::*;
use crate::{
    platform::files::Handoff,
    ui::explorer::{
        Edit, EditKind, Event, Hit, PreviewBody, PreviewView, Row, RowKind, ScrollTarget,
        SearchView,
    },
};
use std::{
    collections::{BTreeSet, HashMap, HashSet, VecDeque},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

/// How often the folders in view are read again for changes made elsewhere.
const WATCH_INTERVAL: Duration = if cfg!(test) {
    Duration::from_millis(60)
} else {
    Duration::from_secs(2)
};
const MAX_WATCHED: usize = 128;
/// The most items read from one folder; the rest are counted, not shown.
const MAX_ENTRIES: usize = 10_000;
const MAX_HITS: usize = 2_000;
/// The most files and folders one search looks at.
const MAX_SEARCHED: usize = 500_000;
/// Typing settles for this long before a search starts.
const SEARCH_DELAY: f64 = 0.12;
const PREVIEW_BYTES: usize = 256 * 1024;
const PREVIEW_LINES: usize = 5_000;
const PREVIEW_COLUMNS: usize = 1_000;
/// The longest side of a previewed picture's texture.
const PREVIEW_PIXELS: u32 = 1_024;
const MAX_DOCUMENT_IMAGES: usize = 16;
const DOCUMENT_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    path: PathBuf,
    dir: bool,
    link: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Listing {
    Loaded {
        entries: Arc<[Entry]>,
        /// Items beyond the limit.
        more: usize,
    },
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Op {
    Create { path: PathBuf, folder: bool },
    Rename { from: PathBuf, to: PathBuf },
    Delete(PathBuf),
}

enum Request {
    List(PathBuf),
    /// The folders to read again periodically; none while the panel is closed.
    Watch(Vec<PathBuf>),
    /// The previewed file, whose changes are reported.
    WatchFile(Option<PathBuf>),
    Apply(Op),
}

enum Loaded {
    Text {
        lines: Vec<String>,
        widest: usize,
        truncated: bool,
        markdown: Option<Vec<ui::markup::Placed>>,
    },
    Image {
        pixels: [u32; 2],
        image: egui::ColorImage,
    },
    Binary,
    Failed(String),
}

enum Reply {
    Listed {
        dir: PathBuf,
        listing: Listing,
    },
    Applied {
        op: Op,
        result: Result<(), String>,
    },
    /// The previewed file is no longer what was read.
    Changed(PathBuf),
    Hits {
        search: u64,
        hits: Vec<Hit>,
        done: bool,
        truncated: bool,
    },
    Preview {
        request: u64,
        size: Option<u64>,
        loaded: Loaded,
    },
    Picture {
        request: u64,
        index: usize,
        pixels: [u32; 2],
        image: egui::ColorImage,
    },
    PictureFailed {
        request: u64,
        index: usize,
    },
    PicturesFinished {
        request: u64,
    },
}

enum Body {
    Loading,
    Text {
        lines: Vec<String>,
        widest: usize,
        truncated: bool,
        markdown: Option<ui::markup::Document>,
    },
    Image {
        texture: egui::TextureHandle,
        pixels: [u32; 2],
    },
    Binary,
    Failed(String),
}

struct Preview {
    path: PathBuf,
    name: String,
    request: u64,
    revision: u64,
    size: Option<u64>,
    body: Body,
}

#[derive(Default)]
struct Search {
    /// The newest search; a worker that reads another number stops.
    current: Arc<AtomicU64>,
    hits: Vec<Hit>,
    running: bool,
    truncated: bool,
    /// When the search for the latest typing starts.
    due: Option<f64>,
}

pub(super) struct Explorer {
    root: Option<PathBuf>,
    /// Why there is no folder to show.
    notice: &'static str,
    dirs: HashMap<PathBuf, Listing>,
    /// Folders being read for the first time.
    requested: HashSet<PathBuf>,
    expanded: BTreeSet<PathBuf>,
    rows: Vec<Row>,
    dirty: bool,
    watched: Vec<PathBuf>,
    /// The previewed file the worker reports changes of.
    watched_file: Option<PathBuf>,
    replies: (mpsc::Sender<Reply>, mpsc::Receiver<Reply>),
    files: Option<mpsc::Sender<Request>>,
    previews: Option<mpsc::Sender<(u64, PathBuf)>>,
    preview_requests: u64,
    preview_current: Arc<AtomicU64>,
    preview_paused: bool,
    preview: Option<Preview>,
    search: Search,
}

impl Default for Explorer {
    fn default() -> Self {
        Self {
            root: None,
            notice: "",
            dirs: HashMap::new(),
            requested: HashSet::new(),
            expanded: BTreeSet::new(),
            rows: Vec::new(),
            dirty: true,
            watched: Vec::new(),
            watched_file: None,
            replies: mpsc::channel(),
            files: None,
            previews: None,
            preview_requests: 0,
            preview_current: Arc::new(AtomicU64::new(0)),
            preview_paused: false,
            preview: None,
            search: Search::default(),
        }
    }
}

fn reason(error: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match error.kind() {
        PermissionDenied => "Permission denied".into(),
        NotFound => "It no longer exists".into(),
        AlreadyExists => "A file or folder with that name already exists".into(),
        DirectoryNotEmpty => "The folder is not empty".into(),
        _ => error.to_string(),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Folders first, then names without regard to case.
fn list(dir: &Path) -> Listing {
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(error) => return Listing::Failed(reason(&error)),
    };
    let mut entries = Vec::new();
    let mut more = 0;
    for entry in read.flatten() {
        if entries.len() >= MAX_ENTRIES {
            more += 1;
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type().ok();
        let link = kind.is_some_and(|kind| kind.is_symlink());
        let dir = if link {
            std::fs::metadata(&path).is_ok_and(|target| target.is_dir())
        } else {
            kind.is_some_and(|kind| kind.is_dir())
        };
        entries.push(Entry {
            name: entry.file_name().to_string_lossy().into_owned(),
            path,
            dir,
            link,
        });
    }
    entries.sort_by_cached_key(|entry| (!entry.dir, entry.name.to_lowercase(), entry.name.clone()));
    Listing::Loaded {
        entries: entries.into(),
        more,
    }
}

fn apply(op: &Op) -> Result<(), String> {
    let exists = |path: &Path| std::fs::symlink_metadata(path).is_ok();
    match op {
        Op::Create { path, folder } => {
            let name = file_name(path);
            let failed =
                |error: std::io::Error| format!("Could not create “{name}”: {}", reason(&error));
            if exists(path) {
                return Err(format!(
                    "Could not create “{name}”: a file or folder with that name already exists"
                ));
            }
            if *folder {
                return std::fs::create_dir_all(path).map_err(failed);
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(failed)?;
            }
            // Never replaces a file that appeared meanwhile.
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map(drop)
                .map_err(failed)
        }
        Op::Rename { from, to } => {
            let name = file_name(from);
            // A change of case names the same file where case is ignored.
            let link = std::fs::symlink_metadata(to).is_ok_and(|to| to.is_symlink());
            let same = !link
                && name.to_lowercase() == file_name(to).to_lowercase()
                && std::fs::canonicalize(from)
                    .ok()
                    .zip(std::fs::canonicalize(to).ok())
                    .is_some_and(|(from, to)| from == to);
            if exists(to) && !same {
                return Err(format!(
                    "Could not rename “{name}”: “{}” already exists",
                    file_name(to)
                ));
            }
            std::fs::rename(from, to)
                .map_err(|error| format!("Could not rename “{name}”: {}", reason(&error)))
        }
        Op::Delete(path) => {
            let name = file_name(path);
            let failed =
                |error: std::io::Error| format!("Could not delete “{name}”: {}", reason(&error));
            let kind = std::fs::symlink_metadata(path).map_err(failed)?.file_type();
            if kind.is_dir() {
                std::fs::remove_dir_all(path).map_err(failed)
            } else {
                // A link is removed, never what it points at. On Windows a
                // link to a folder is itself removed as a folder.
                std::fs::remove_file(path)
                    .or_else(|error| {
                        if kind.is_symlink() {
                            std::fs::remove_dir(path)
                        } else {
                            Err(error)
                        }
                    })
                    .map_err(failed)
            }
        }
    }
}

/// Lists and changes files, and reads the watched folders again on a timer.
/// A folder read on the timer is reported only when it changed, so an
/// unchanged tree wakes no frame.
fn file_worker(
    requests: mpsc::Receiver<Request>,
    replies: mpsc::Sender<Reply>,
    wake: egui::Context,
) {
    let mut watched: Vec<PathBuf> = Vec::new();
    let mut known: HashMap<PathBuf, Listing> = HashMap::new();
    // A file's length and modification time stand for its contents.
    let stamp = |path: &Path| {
        std::fs::metadata(path)
            .ok()
            .map(|file| (file.len(), file.modified().ok()))
    };
    let mut file: Option<(PathBuf, _)> = None;
    let mut next_scan = Instant::now() + WATCH_INTERVAL;
    loop {
        let request = if watched.is_empty() && file.is_none() {
            requests
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        } else {
            requests.recv_timeout(next_scan.saturating_duration_since(Instant::now()))
        };
        let sent = match request {
            Ok(Request::List(dir)) => {
                let listing = list(&dir);
                known.insert(dir.clone(), listing.clone());
                replies.send(Reply::Listed { dir, listing })
            }
            Ok(Request::Watch(dirs)) => {
                known.retain(|dir, _| dirs.contains(dir));
                watched = dirs;
                next_scan = Instant::now() + WATCH_INTERVAL;
                continue;
            }
            Ok(Request::WatchFile(path)) => {
                file = path.map(|path| {
                    let seen = stamp(&path);
                    (path, seen)
                });
                continue;
            }
            Ok(Request::Apply(op)) => {
                let result = apply(&op);
                replies.send(Reply::Applied { op, result })
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let mut changed = false;
                for dir in &watched {
                    let listing = list(dir);
                    if known.get(dir) != Some(&listing) {
                        known.insert(dir.clone(), listing.clone());
                        changed |= replies
                            .send(Reply::Listed {
                                dir: dir.clone(),
                                listing,
                            })
                            .is_ok();
                    }
                }
                if let Some((path, seen)) = &mut file {
                    let now = stamp(path);
                    if now != *seen {
                        *seen = now;
                        changed |= replies.send(Reply::Changed(path.clone())).is_ok();
                    }
                }
                next_scan = Instant::now() + WATCH_INTERVAL;
                if changed {
                    wake.request_repaint();
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        if sent.is_err() {
            return;
        }
        wake.request_repaint();
    }
}

/// Reads the newest requested file; requests made while reading are skipped.
fn preview_worker(
    requests: mpsc::Receiver<(u64, PathBuf)>,
    replies: mpsc::Sender<Reply>,
    wake: egui::Context,
    current: Arc<AtomicU64>,
) {
    let mut pending = None;
    loop {
        let Some(mut newest) = pending.take().or_else(|| requests.recv().ok()) else {
            return;
        };
        while let Ok(newer) = requests.try_recv() {
            newest = newer;
        }
        let (request, path) = newest;
        if current.load(Ordering::Relaxed) != request {
            continue;
        }
        let (size, loaded) = read_preview(&path);
        if current.load(Ordering::Relaxed) != request {
            continue;
        }
        let images: Vec<_> = match &loaded {
            Loaded::Text {
                markdown: Some(blocks),
                ..
            } => blocks
                .iter()
                .enumerate()
                .filter_map(|(index, block)| {
                    if let ui::markup::Block::Image(image) = &block.block {
                        Some((index, image.source.clone()))
                    } else {
                        None
                    }
                })
                .take(MAX_DOCUMENT_IMAGES)
                .collect(),
            _ => Vec::new(),
        };
        if replies
            .send(Reply::Preview {
                request,
                size,
                loaded,
            })
            .is_err()
        {
            return;
        }
        wake.request_repaint();
        if images.is_empty() {
            continue;
        }
        // Text appears immediately. Pictures arrive one at a time, and a new
        // file request cancels the remaining work for this document.
        let deadline = Instant::now() + Duration::from_secs(8);
        for (index, source) in images {
            if current.load(Ordering::Relaxed) != request {
                break;
            }
            match requests.try_recv() {
                Ok(newer) => {
                    pending = Some(newer);
                    break;
                }
                Err(mpsc::TryRecvError::Disconnected) => return,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if Instant::now() >= deadline {
                break;
            }
            let picture = document_picture(&source, &path);
            if current.load(Ordering::Relaxed) != request {
                break;
            }
            let reply = match picture {
                Some((pixels, image)) => Reply::Picture {
                    request,
                    index,
                    pixels,
                    image,
                },
                None => Reply::PictureFailed { request, index },
            };
            if replies.send(reply).is_err() {
                return;
            }
            wake.request_repaint();
        }
        if current.load(Ordering::Relaxed) != request {
            continue;
        }
        if replies.send(Reply::PicturesFinished { request }).is_err() {
            return;
        }
        wake.request_repaint();
    }
}

fn document_picture(source: &str, document: &Path) -> Option<([u32; 2], egui::ColorImage)> {
    let limit = [PREVIEW_PIXELS, PREVIEW_PIXELS];
    if let Some(link) = crate::platform::links::WebLink::new(source) {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(2)))
            .max_redirects(3)
            .build()
            .into();
        let bytes = agent
            .get(link.as_str())
            .call()
            .ok()?
            .body_mut()
            .with_config()
            .limit(DOCUMENT_IMAGE_BYTES)
            .read_to_vec()
            .ok()?;
        return image_preview::decode_bytes(&bytes, limit);
    }
    let path = ui::markup::picture_path(source, document)?;
    image_preview::decode(&path, limit)
}

fn read_preview(path: &Path) -> (Option<u64>, Loaded) {
    use std::io::Read as _;
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => return (None, Loaded::Failed(reason(&error))),
    };
    // Reading a pipe or a device could wait forever.
    if !metadata.is_file() {
        return (None, Loaded::Failed("Not a regular file".into()));
    }
    let size = Some(metadata.len());
    if let Some((pixels, image)) = image_preview::decode(path, [PREVIEW_PIXELS, PREVIEW_PIXELS]) {
        return (size, Loaded::Image { pixels, image });
    }
    let mut bytes = Vec::new();
    let read = std::fs::File::open(path)
        .and_then(|file| file.take(PREVIEW_BYTES as u64 + 1).read_to_end(&mut bytes));
    if let Err(error) = read {
        return (size, Loaded::Failed(reason(&error)));
    }
    let markdown = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
    (size, text_preview(&bytes, markdown))
}

/// A line of a file as it is painted: tabs become spaces, control characters
/// blanks, and what is beyond the widest line shown an ellipsis. Returns the
/// columns it takes as well.
pub(super) fn display_line(line: &str) -> (String, usize) {
    let mut shown = String::with_capacity(line.len());
    let mut columns = 0;
    for character in line.trim_end_matches('\r').chars() {
        if columns >= PREVIEW_COLUMNS {
            shown.push('…');
            columns += 1;
            break;
        }
        match character {
            '\t' => {
                let stop = 4 - columns % 4;
                shown.extend(std::iter::repeat_n(' ', stop));
                columns += stop;
            }
            character if character.is_control() => {
                shown.push(' ');
                columns += 1;
            }
            character => {
                shown.push(character);
                columns += 1;
            }
        }
    }
    (shown, columns)
}

/// Text as lines ready to paint, or `Binary` for anything else.
fn text_preview(bytes: &[u8], markdown: bool) -> Loaded {
    if bytes.iter().take(8 * 1024).any(|byte| *byte == 0) {
        return Loaded::Binary;
    }
    let mut truncated = bytes.len() > PREVIEW_BYTES;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(PREVIEW_BYTES)]);
    let mut lines: Vec<&str> = text.split('\n').collect();
    if truncated {
        // The cut may have split the last line, or a character in it. A file
        // that is one long line still shows what was read of it.
        if lines.len() > 1 {
            lines.pop();
        }
    } else if lines.last() == Some(&"") {
        lines.pop();
    }
    if lines.len() > PREVIEW_LINES {
        lines.truncate(PREVIEW_LINES);
        truncated = true;
    }
    // Code-copy controls need the original bounded text, including tabs and
    // long command lines. Source rows are sanitized separately for display.
    let markdown = markdown.then(|| ui::markup::document_blocks(&lines.join("\n")));
    let mut widest = 0;
    let lines = lines
        .into_iter()
        .map(|line| {
            let (shown, columns) = display_line(line);
            widest = widest.max(columns);
            truncated |= columns > PREVIEW_COLUMNS;
            shown
        })
        .collect();
    Loaded::Text {
        lines,
        widest,
        truncated,
        markdown,
    }
}

/// Patterns naming files to leave out, as typed: separated by commas, with
/// `*`, `?`, `**` for any folders and `{a,b}` for alternatives. A pattern
/// without a folder applies at any depth.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Globs(Vec<Vec<String>>);

/// Splits at `separator` outside braces.
fn split_outside_braces(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (index, character) in text.char_indices() {
        match character {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ if character == separator && depth == 0 => {
                parts.push(&text[start..index]);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Every pattern a `{a,b}` group stands for, within a fixed bound.
fn expand_braces(pattern: &str) -> Vec<String> {
    // Alternatives multiply, so the work is bounded as well as the result.
    let mut budget = 256;
    expand_within(pattern, &mut budget)
}

fn expand_within(pattern: &str, budget: &mut usize) -> Vec<String> {
    const MAX_ALTERNATIVES: usize = 64;
    let literal = || vec![pattern.to_owned()];
    if *budget == 0 {
        return literal();
    }
    *budget -= 1;
    let Some(open) = pattern.find('{') else {
        return literal();
    };
    let mut depth = 0usize;
    let close = pattern[open..]
        .char_indices()
        .find_map(|(index, character)| {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(open + index);
                    }
                }
                _ => {}
            }
            None
        });
    // An unclosed brace is an ordinary character.
    let Some(close) = close else {
        return literal();
    };
    let (head, tail) = (&pattern[..open], &pattern[close + 1..]);
    let mut expanded = Vec::new();
    for alternative in split_outside_braces(&pattern[open + 1..close], ',') {
        for pattern in expand_within(&format!("{head}{alternative}{tail}"), budget) {
            if expanded.len() >= MAX_ALTERNATIVES {
                return expanded;
            }
            expanded.push(pattern);
        }
    }
    expanded
}

/// Names differ by case only where the platform's file systems say so.
fn fold(text: &str) -> String {
    if cfg!(any(windows, target_os = "macos")) {
        text.to_lowercase()
    } else {
        text.to_owned()
    }
}

fn segments(pattern: &str) -> Vec<String> {
    let pattern = pattern.trim().replace('\\', "/");
    let pattern = pattern.strip_prefix("./").unwrap_or(&pattern);
    let mut segments: Vec<String> = Vec::new();
    for segment in pattern.split('/').filter(|segment| !segment.is_empty()) {
        if segment == "**" && segments.last().is_some_and(|last| last == "**") {
            continue;
        }
        segments.push(segment.to_owned());
    }
    if segments.is_empty() {
        return segments;
    }
    // `*.log` and `dist` apply in every folder; `dist/` covers its contents.
    if !pattern.trim_end_matches('/').contains('/') && segments[0] != "**" {
        segments.insert(0, "**".into());
    }
    if pattern.ends_with('/') && segments.last().is_some_and(|last| last != "**") {
        segments.push("**".into());
    }
    segments
}

impl Globs {
    pub(super) fn parse(text: &str) -> Self {
        Self(
            split_outside_braces(text, ',')
                .into_iter()
                .flat_map(expand_braces)
                .map(|pattern| segments(&fold(&pattern)))
                .filter(|segments| !segments.is_empty())
                .collect(),
        )
    }

    /// Whether the file or folder at `path`, given by the names leading to it
    /// from the root, is covered. `**` also stands for no folder at all, so a
    /// pattern for a folder's contents covers the folder itself.
    pub(super) fn matches(&self, path: &[String]) -> bool {
        self.0.iter().any(|pattern| glob(pattern, path))
    }
}

fn glob(pattern: &[String], path: &[String]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((first, rest)) if first == "**" => {
            (0..=path.len()).any(|skipped| glob(rest, &path[skipped..]))
        }
        Some((first, rest)) => path
            .split_first()
            .is_some_and(|(name, tail)| wildcard(first, name) && glob(rest, tail)),
    }
}

/// `*` stands for any run of characters in a name and `?` for one.
fn wildcard(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, n));
            p += 1;
        } else if p < pattern.len() && (pattern[p] == '?' || pattern[p] == name[n]) {
            p += 1;
            n += 1;
        } else if let Some((at, taken)) = star {
            p = at + 1;
            n = taken + 1;
            star = Some((at, taken + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|character| *character == '*')
}

/// What typed text looks for: a pattern when it has wildcards, otherwise
/// words that must all be in the path, at least one of them in the name.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Query {
    Glob(Vec<String>),
    Words(Vec<String>),
}

impl Query {
    fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_lowercase();
        if text.is_empty() {
            return None;
        }
        Some(if text.contains(['*', '?']) {
            Self::Glob(segments(&text))
        } else {
            Self::Words(text.split_whitespace().map(str::to_owned).collect())
        })
    }

    /// `path` holds the lowercased names from the root to the item.
    fn matches(&self, path: &[String]) -> bool {
        match self {
            Self::Glob(pattern) => glob(pattern, path),
            Self::Words(words) => {
                let Some(name) = path.last() else {
                    return false;
                };
                let whole = path.join("/");
                // A word with a slash can only be part of the path.
                let mut named = words.iter().filter(|word| !word.contains('/')).peekable();
                words.iter().all(|word| whole.contains(word.as_str()))
                    && (named.peek().is_none() || named.any(|word| name.contains(word.as_str())))
            }
        }
    }
}

/// Walks the folders under `root` nearest first, reporting matches in
/// batches. Links to folders are listed but not followed.
fn search_worker(
    root: PathBuf,
    query: Query,
    exclude: Globs,
    search: u64,
    current: Arc<AtomicU64>,
    replies: mpsc::Sender<Reply>,
    wake: egui::Context,
) {
    let mut queue = VecDeque::from([(root, Vec::<String>::new())]);
    let mut batch = Vec::new();
    let mut sent = Instant::now();
    let (mut found, mut seen) = (0, 0);
    let mut truncated = false;
    let cancelled = || current.load(Ordering::Relaxed) != search;
    'walk: while let Some((dir, names)) = queue.pop_front() {
        let Listing::Loaded { entries, .. } = list(&dir) else {
            continue;
        };
        for entry in entries.iter() {
            if cancelled() {
                return;
            }
            seen += 1;
            if found >= MAX_HITS || seen > MAX_SEARCHED {
                truncated = true;
                break 'walk;
            }
            let mut path = names.clone();
            path.push(entry.name.clone());
            let folded: Vec<String> = path.iter().map(|name| fold(name)).collect();
            if exclude.matches(&folded) {
                continue;
            }
            let lowered: Vec<String> = path.iter().map(|name| name.to_lowercase()).collect();
            if query.matches(&lowered) {
                found += 1;
                batch.push(Hit {
                    path: entry.path.clone(),
                    name: entry.name.clone(),
                    folder: names.join("/"),
                    dir: entry.dir,
                });
            }
            if entry.dir && !entry.link {
                queue.push_back((entry.path.clone(), path));
            }
        }
        if !batch.is_empty() && (batch.len() >= 64 || sent.elapsed() > Duration::from_millis(80)) {
            let hits = std::mem::take(&mut batch);
            if replies
                .send(Reply::Hits {
                    search,
                    hits,
                    done: false,
                    truncated: false,
                })
                .is_err()
            {
                return;
            }
            sent = Instant::now();
            wake.request_repaint();
        }
    }
    if !cancelled() {
        let _ = replies.send(Reply::Hits {
            search,
            hits: batch,
            done: true,
            truncated,
        });
        wake.request_repaint();
    }
}

fn is_picture(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["png", "jpg", "jpeg", "gif", "webp", "bmp"]
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

/// Why `text` cannot name a new or renamed item, if it cannot.
fn invalid_name(kind: EditKind, text: &str) -> Option<&'static str> {
    let nested = text.contains('/') || (cfg!(windows) && text.contains('\\'));
    if kind == EditKind::Rename && nested {
        return Some("A name cannot contain a slash");
    }
    if text.contains('\0') || (cfg!(windows) && text.contains(['<', '>', ':', '"', '|', '?', '*']))
    {
        return Some("A name cannot contain this character");
    }
    let path = Path::new(text);
    let plain = path
        .components()
        .all(|component| matches!(component, std::path::Component::Normal(_)));
    if path.is_absolute() || !plain || text.ends_with('/') {
        return Some("Enter a name, or a path inside this folder");
    }
    None
}

impl Explorer {
    /// A listing is useful only while its branch is open under the current
    /// root. Worker replies can arrive after collapse, navigation or hiding.
    fn needs_listing(&self, dir: &Path) -> bool {
        let Some(root) = self.root.as_deref() else {
            return false;
        };
        dir.starts_with(root)
            && dir
                .ancestors()
                .take_while(|ancestor| *ancestor != root)
                .all(|ancestor| self.expanded.contains(ancestor))
    }

    fn files(&mut self, ctx: &egui::Context) -> Option<&mpsc::Sender<Request>> {
        if self.files.is_none() {
            let (sender, requests) = mpsc::channel();
            let replies = self.replies.0.clone();
            let wake = ctx.clone();
            std::thread::Builder::new()
                .name("neptune-explorer".into())
                .spawn(move || file_worker(requests, replies, wake))
                .ok()?;
            self.files = Some(sender);
        }
        self.files.as_ref()
    }

    fn request(&mut self, ctx: &egui::Context, request: Request) -> bool {
        self.files(ctx)
            .is_some_and(|files| files.send(request).is_ok())
    }

    fn list(&mut self, ctx: &egui::Context, dir: &Path) {
        self.request(ctx, Request::List(dir.into()));
    }

    /// Reads the root and every folder in view again.
    fn refresh(&mut self, ctx: &egui::Context) {
        for dir in self.watched.clone() {
            self.list(ctx, &dir);
        }
        if let Some(preview) = &self.preview {
            let path = preview.path.clone();
            self.show_preview(ctx, path, false);
        }
    }

    fn set_root(&mut self, ctx: &egui::Context, root: Option<PathBuf>) {
        if let Some(root) = &root {
            self.dirs.retain(|dir, _| dir.starts_with(root));
            self.list(ctx, root);
        } else {
            self.dirs.clear();
        }
        self.requested.clear();
        self.root = root;
        self.dirty = true;
    }

    /// Shows `path` in the preview. A file already shown keeps its content
    /// until the new reading arrives.
    pub(super) fn show_preview(&mut self, ctx: &egui::Context, path: PathBuf, reset: bool) {
        if self.previews.is_none() {
            let (sender, requests) = mpsc::channel();
            let replies = self.replies.0.clone();
            let wake = ctx.clone();
            let current = self.preview_current.clone();
            if std::thread::Builder::new()
                .name("neptune-explorer-preview".into())
                .spawn(move || preview_worker(requests, replies, wake, current))
                .is_ok()
            {
                self.previews = Some(sender);
            }
        }
        self.preview_requests += 1;
        let request = self.preview_requests;
        self.preview_current.store(request, Ordering::Relaxed);
        self.preview_paused = false;
        match &mut self.preview {
            Some(preview) if preview.path == path && !reset => preview.request = request,
            _ => {
                self.preview = Some(Preview {
                    name: file_name(&path),
                    path: path.clone(),
                    request,
                    revision: 0,
                    size: None,
                    body: Body::Loading,
                });
            }
        }
        let sent = self
            .previews
            .as_ref()
            .is_some_and(|previews| previews.send((request, path)).is_ok());
        if !sent && let Some(preview) = &mut self.preview {
            preview.body = Body::Failed("Could not read the file".into());
        }
    }

    fn cancel_preview_work(&mut self) {
        self.preview_current.store(0, Ordering::Relaxed);
        self.preview_paused = self.preview.is_some();
        if let Some(preview) = &mut self.preview {
            preview.request = 0;
        }
    }

    fn cancel_search(&mut self) {
        self.search.current.fetch_add(1, Ordering::Relaxed);
        self.search.hits.clear();
        self.search.running = false;
        self.search.truncated = false;
        self.search.due = None;
    }

    fn start_search(&mut self, ctx: &egui::Context, query: &str, exclude: &str) {
        self.cancel_search();
        let (Some(root), Some(query)) = (self.root.clone(), Query::parse(query)) else {
            return;
        };
        let search = self.search.current.load(Ordering::Relaxed);
        let current = self.search.current.clone();
        let exclude = Globs::parse(exclude);
        let replies = self.replies.0.clone();
        let wake = ctx.clone();
        self.search.running = std::thread::Builder::new()
            .name("neptune-explorer-search".into())
            .spawn(move || search_worker(root, query, exclude, search, current, replies, wake))
            .is_ok();
    }

    /// Opens every folder between the root and `path`, so its row exists.
    fn uncover(&mut self, ctx: &egui::Context, path: &Path) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let mut dir = path.parent();
        while let Some(parent) = dir
            && parent != root
            && parent.starts_with(&root)
        {
            if self.expanded.insert(parent.into()) {
                self.list(ctx, parent);
            }
            dir = parent.parent();
        }
        self.dirty = true;
    }

    /// The tree as rows, with the folders shown and those still to be read.
    fn rebuild(&mut self, edit: Option<&Edit>) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let mut rows = Vec::new();
        let mut shown = Vec::new();
        let mut missing = Vec::new();
        if let Some(root) = &self.root {
            let mut tree = Tree {
                dirs: &self.dirs,
                expanded: &self.expanded,
                edit,
                rows: &mut rows,
                shown: &mut shown,
                missing: &mut missing,
            };
            tree.folder(root, 0);
        }
        // Keep expansion choices, but release the entries of collapsed
        // branches. Reopening them already asks the worker for a fresh listing.
        let visible: HashSet<&Path> = shown.iter().map(PathBuf::as_path).collect();
        self.dirs.retain(|dir, _| visible.contains(dir.as_path()));
        self.rows = rows;
        self.dirty = false;
        (shown, missing)
    }

    pub(super) fn view<'a>(
        &'a self,
        searching: bool,
        reveal: f32,
        window: Rect,
    ) -> ui::explorer::View<'a> {
        ui::explorer::View {
            root: self.root.as_deref(),
            notice: self.notice,
            rows: &self.rows,
            search: searching.then_some(SearchView {
                hits: &self.search.hits,
                running: self.search.running || self.search.due.is_some(),
                truncated: self.search.truncated,
            }),
            preview: self.preview.as_ref().map(|preview| PreviewView {
                path: &preview.path,
                name: &preview.name,
                revision: preview.revision,
                size: preview.size,
                body: match &preview.body {
                    Body::Loading => PreviewBody::Loading,
                    Body::Text {
                        lines,
                        widest,
                        truncated,
                        markdown,
                    } => PreviewBody::Text {
                        lines,
                        widest: *widest,
                        truncated: *truncated,
                        markdown: markdown.as_ref(),
                    },
                    Body::Image { texture, pixels } => PreviewBody::Image {
                        texture,
                        pixels: *pixels,
                    },
                    Body::Binary => PreviewBody::Binary,
                    Body::Failed(message) => PreviewBody::Failed(message),
                },
            }),
            reveal,
            window,
        }
    }
}

struct Tree<'a> {
    dirs: &'a HashMap<PathBuf, Listing>,
    expanded: &'a BTreeSet<PathBuf>,
    edit: Option<&'a Edit>,
    rows: &'a mut Vec<Row>,
    shown: &'a mut Vec<PathBuf>,
    missing: &'a mut Vec<PathBuf>,
}

impl Tree<'_> {
    fn note(&mut self, dir: &Path, depth: usize, text: String, error: bool) {
        self.rows.push(Row {
            path: dir.into(),
            name: text,
            depth,
            link: false,
            kind: RowKind::Note { error },
        });
    }

    fn editor(&mut self, edit: &Edit, path: &Path, depth: usize, folder: bool) {
        self.rows.push(Row {
            path: path.into(),
            name: String::new(),
            depth,
            link: false,
            kind: RowKind::Edit { folder },
        });
        if let Some(error) = &edit.error {
            self.note(path, depth, error.clone(), true);
        }
    }

    fn folder(&mut self, dir: &Path, depth: usize) {
        self.shown.push(dir.into());
        let creating = self
            .edit
            .filter(|edit| edit.kind != EditKind::Rename && edit.parent == dir);
        if let Some(edit) = creating {
            self.editor(edit, dir, depth, edit.kind == EditKind::NewFolder);
        }
        match self.dirs.get(dir) {
            None => {
                self.missing.push(dir.into());
                self.note(dir, depth, "Loading…".into(), false);
            }
            Some(Listing::Failed(message)) => self.note(dir, depth, message.clone(), true),
            Some(Listing::Loaded { entries, more }) => {
                if entries.is_empty() && creating.is_none() {
                    self.note(dir, depth, "Empty folder".into(), false);
                }
                for entry in entries.iter() {
                    let renaming = self.edit.filter(|edit| {
                        edit.kind == EditKind::Rename && edit.target.as_deref() == Some(&entry.path)
                    });
                    if let Some(edit) = renaming {
                        self.editor(edit, &entry.path, depth, entry.dir);
                        continue;
                    }
                    let expanded = entry.dir && self.expanded.contains(&entry.path);
                    self.rows.push(Row {
                        path: entry.path.clone(),
                        name: entry.name.clone(),
                        depth,
                        link: entry.link,
                        kind: if entry.dir {
                            RowKind::Folder { expanded }
                        } else {
                            RowKind::File {
                                picture: is_picture(&entry.name),
                            }
                        },
                    });
                    if expanded {
                        self.folder(&entry.path, depth + 1);
                    }
                }
                if *more > 0 {
                    self.note(
                        dir,
                        depth,
                        format!("{more} more items are not shown"),
                        false,
                    );
                }
            }
        }
    }
}

impl App {
    pub(super) fn explorer_text_selected(&self) -> bool {
        self.ui.overlay == OverlayState::None
            && self.ui.panel.open
            && self.ui.panel.tab == ui::panel::Tab::Files
            && self.explorer.preview.as_ref().is_some_and(|preview| {
                matches!(&preview.body, Body::Text { markdown, .. } if markdown.is_none() || !self.ui.explorer.markdown_preview)
                    && self.ui.explorer.source_selection.has_selection(&preview.path, preview.revision)
            })
    }

    /// How many rows the tree has, for a test that waits for them.
    #[cfg(test)]
    pub(super) fn explorer_rows(&self) -> usize {
        self.explorer.rows.len()
    }

    /// No folder is read or watched, as while the explorer is out of view.
    #[cfg(test)]
    pub(super) fn explorer_resting(&self) -> bool {
        self.explorer.root.is_none() && self.explorer.watched.is_empty()
    }

    /// Results of workers, taken whether or not the panel is in view.
    pub(super) fn poll_explorer(&mut self, ctx: &egui::Context) {
        while let Ok(reply) = self.explorer.replies.1.try_recv() {
            match reply {
                Reply::Listed { dir, listing } => {
                    self.explorer.requested.remove(&dir);
                    if self.explorer.needs_listing(&dir) {
                        self.explorer.dirs.insert(dir, listing);
                        self.explorer.dirty = true;
                    }
                }
                Reply::Applied { op, result } => self.applied(ctx, op, result),
                Reply::Changed(path) => {
                    if self
                        .explorer
                        .preview
                        .as_ref()
                        .is_some_and(|preview| preview.path == path)
                    {
                        self.explorer.show_preview(ctx, path, false);
                    }
                }
                Reply::Hits {
                    search,
                    hits,
                    done,
                    truncated,
                } => {
                    let explorer = &mut self.explorer;
                    if explorer.search.current.load(Ordering::Relaxed) == search {
                        explorer.search.hits.extend(hits);
                        explorer.search.running = !done;
                        explorer.search.truncated = truncated;
                    }
                }
                Reply::Preview {
                    request,
                    size,
                    loaded,
                } => {
                    if let Some(preview) = &mut self.explorer.preview
                        && preview.request == request
                    {
                        preview.size = size;
                        preview.revision = request;
                        preview.body = match loaded {
                            Loaded::Text {
                                lines,
                                widest,
                                truncated,
                                markdown,
                            } => Body::Text {
                                lines,
                                widest,
                                truncated,
                                markdown: markdown.map(|blocks| {
                                    let mut document = ui::markup::Document::new(blocks);
                                    if let Body::Text {
                                        markdown: Some(previous),
                                        ..
                                    } = &preview.body
                                    {
                                        document.retain_state_from(previous);
                                    }
                                    document
                                }),
                            },
                            Loaded::Image { pixels, image } => Body::Image {
                                pixels,
                                texture: ctx.load_texture(
                                    "explorer-preview",
                                    image,
                                    egui::TextureOptions::LINEAR,
                                ),
                            },
                            Loaded::Binary => Body::Binary,
                            Loaded::Failed(message) => Body::Failed(message),
                        };
                    }
                }
                Reply::Picture {
                    request,
                    index,
                    pixels,
                    image,
                } => {
                    if let Some(preview) = &mut self.explorer.preview
                        && preview.request == request
                        && let Body::Text {
                            markdown: Some(document),
                            ..
                        } = &mut preview.body
                    {
                        document.pictures.insert(
                            index,
                            ui::markup::Picture {
                                pixels,
                                texture: ctx.load_texture(
                                    "explorer-document-image",
                                    image,
                                    egui::TextureOptions::LINEAR,
                                ),
                            },
                        );
                    }
                }
                Reply::PictureFailed { request, index } => {
                    if let Some(preview) = &mut self.explorer.preview
                        && preview.request == request
                        && let Body::Text {
                            markdown: Some(document),
                            ..
                        } = &mut preview.body
                    {
                        document.pictures.remove(&index);
                    }
                }
                Reply::PicturesFinished { request } => {
                    if let Some(preview) = &mut self.explorer.preview
                        && preview.request == request
                        && let Body::Text {
                            markdown: Some(document),
                            ..
                        } = &mut preview.body
                    {
                        document.loading_images = false;
                    }
                }
            }
        }
    }

    /// Brings the panel's data up to date for a frame that shows it.
    pub(super) fn sync_explorer(&mut self, ctx: &egui::Context) {
        if self.explorer.preview_paused
            && let Some(preview) = &self.explorer.preview
        {
            let path = preview.path.clone();
            self.explorer.show_preview(ctx, path, false);
        }
        let model = self.controller.model();
        let workspace = model.active_workspace().and_then(|id| model.workspace(id));
        let (root, notice) = match workspace {
            None => (None, "Open a workspace to browse its folder."),
            Some(workspace) if workspace.remote().is_some() => (
                None,
                "This workspace is connected over SSH. The explorer shows folders on this computer only.",
            ),
            Some(workspace) => (
                Some(PathBuf::from(
                    workspace
                        .pane(workspace.active())
                        .map_or(workspace.cwd(), |pane| pane.cwd()),
                )),
                "",
            ),
        };
        self.explorer.notice = notice;
        if root != self.explorer.root {
            self.explorer.set_root(ctx, root);
            self.ui.explorer.edit = None;
            self.ui.explorer.scroll_to = None;
            if !self.ui.explorer.query.trim().is_empty() {
                self.explorer.search.due = Some(ctx.input(|input| input.time));
            }
        }
        if let Some(due) = self.explorer.search.due {
            let now = ctx.input(|input| input.time);
            if now >= due {
                let state = &self.ui.explorer;
                self.explorer
                    .start_search(ctx, &state.query, &state.exclude);
            } else {
                ctx.request_repaint_after(Duration::from_secs_f64(due - now));
            }
        }
        if self.explorer.dirty {
            let (mut shown, missing) = self.explorer.rebuild(self.ui.explorer.edit.as_ref());
            // A name cannot be typed for a folder the tree no longer shows.
            let placed = self
                .explorer
                .rows
                .iter()
                .any(|row| matches!(row.kind, RowKind::Edit { .. }));
            if !placed
                && let Some(edit) = &self.ui.explorer.edit
                && self.explorer.dirs.contains_key(&edit.parent)
                && missing.is_empty()
            {
                self.ui.explorer.edit = None;
                self.ui.explorer.scroll_to = None;
            }
            for dir in missing {
                if self.explorer.requested.insert(dir.clone()) {
                    self.explorer.list(ctx, &dir);
                }
            }
            shown.truncate(MAX_WATCHED);
            if shown != self.explorer.watched {
                self.explorer.watched = shown.clone();
                self.explorer.request(ctx, Request::Watch(shown));
            }
        }
        let previewed = self
            .explorer
            .preview
            .as_ref()
            .map(|preview| preview.path.clone());
        if previewed != self.explorer.watched_file {
            self.explorer.watched_file = previewed.clone();
            self.explorer.request(ctx, Request::WatchFile(previewed));
        }
    }

    /// The panel left view: nothing is read again until it returns.
    pub(super) fn rest_explorer(&mut self, ctx: &egui::Context) {
        self.explorer.cancel_preview_work();
        if self.explorer.watched_file.take().is_some() {
            self.explorer.request(ctx, Request::WatchFile(None));
        }
        if !self.explorer.watched.is_empty() {
            self.explorer.watched.clear();
            self.explorer.request(ctx, Request::Watch(Vec::new()));
            // What is shown next is read afresh.
            self.explorer.root = None;
            self.explorer.dirs = HashMap::new();
            self.explorer.rows = Vec::new();
            self.explorer.requested = HashSet::new();
            self.explorer.cancel_search();
            self.explorer.dirty = true;
        }
    }

    /// Escape leaves an edit, then a search, then the field itself. Returns
    /// whether the key was the explorer's.
    pub(super) fn explorer_escape(&mut self, ctx: &egui::Context) -> bool {
        use ui::explorer::{edit_id, exclude_id, query_id};
        // The toolkit has already taken focus from the field for this key.
        let held =
            |id| ctx.memory(|memory| memory.has_focus(id) || memory.had_focus_last_frame(id));
        if self.ui.explorer.edit.is_some() {
            // A name field that scrolled out of view lost the keyboard; the
            // key is then the shell's, and the unfinished name is dropped.
            let typing = held(edit_id())
                || self
                    .ui
                    .explorer
                    .edit
                    .as_ref()
                    .is_some_and(|edit| edit.focus);
            self.ui.explorer.edit = None;
            self.explorer.dirty = true;
            ctx.memory_mut(|memory| memory.surrender_focus(edit_id()));
            if !typing {
                return false;
            }
        } else if held(query_id()) && !self.ui.explorer.query.is_empty() {
            self.ui.explorer.query.clear();
            self.explorer_event(ctx, Event::SearchChanged);
            ctx.memory_mut(|memory| memory.request_focus(query_id()));
        } else if let Some(id) = [query_id(), exclude_id()].into_iter().find(|id| held(*id)) {
            ctx.memory_mut(|memory| memory.surrender_focus(id));
        } else if self.explorer_text_selected() {
            self.ui.explorer.source_selection.reset();
        } else {
            return false;
        }
        true
    }

    fn applied(&mut self, ctx: &egui::Context, op: Op, result: Result<(), String>) {
        let parent = |path: &Path| path.parent().map(Path::to_path_buf);
        let touched = match &op {
            Op::Create { path, .. } | Op::Delete(path) => vec![parent(path)],
            Op::Rename { from, to } => vec![parent(from), parent(to)],
        };
        for dir in touched.into_iter().flatten() {
            self.explorer.list(ctx, &dir);
        }
        if let Err(error) = result {
            self.ui.error = Some(error);
            return;
        }
        let explorer = &mut self.explorer;
        let state = &mut self.ui.explorer;
        match op {
            Op::Create { path, folder } => {
                // A name with folders in it made them too.
                explorer.uncover(ctx, &path);
                if let Some(root) = explorer.root.clone() {
                    let mut dir = path.parent();
                    while let Some(made) = dir.filter(|dir| dir.starts_with(&root)) {
                        explorer.list(ctx, made);
                        dir = made.parent();
                    }
                }
                state.selected = Some((path.clone(), folder));
                state.scroll_to = Some(ScrollTarget::Path(path.clone()));
                if !folder {
                    explorer.show_preview(ctx, path, true);
                }
            }
            Op::Rename { from, to } => {
                let moved = |path: &Path| path.strip_prefix(&from).ok().map(|rest| to.join(rest));
                explorer.expanded = std::mem::take(&mut explorer.expanded)
                    .into_iter()
                    .map(|dir| moved(&dir).unwrap_or(dir))
                    .collect();
                explorer.dirs.retain(|dir, _| !dir.starts_with(&from));
                if let Some((selected, _)) = &mut state.selected
                    && let Some(path) = moved(selected)
                {
                    *selected = path;
                }
                if let Some(path) = explorer
                    .preview
                    .as_ref()
                    .and_then(|preview| moved(&preview.path))
                {
                    explorer.show_preview(ctx, path, true);
                }
            }
            Op::Delete(path) => {
                explorer.expanded.retain(|dir| !dir.starts_with(&path));
                explorer.dirs.retain(|dir, _| !dir.starts_with(&path));
                if state
                    .selected
                    .as_ref()
                    .is_some_and(|(selected, _)| selected.starts_with(&path))
                {
                    state.selected = None;
                }
                if explorer
                    .preview
                    .as_ref()
                    .is_some_and(|preview| preview.path.starts_with(&path))
                {
                    explorer.preview = None;
                }
            }
        }
        explorer.dirty = true;
        if !state.query.trim().is_empty() {
            explorer.search.due = Some(ctx.input(|input| input.time));
        }
    }

    /// Leaves a search so the tree, where items are made and renamed, shows.
    fn leave_search(&mut self) {
        self.ui.explorer.query.clear();
        self.explorer.cancel_search();
    }

    fn commit(&mut self, ctx: &egui::Context, explicit: bool) {
        let Some(mut edit) = self.ui.explorer.edit.take() else {
            return;
        };
        self.explorer.dirty = true;
        let text = edit.text.trim().to_owned();
        let unchanged = edit
            .target
            .as_deref()
            .is_some_and(|target| file_name(target) == text);
        if text.is_empty() || unchanged {
            return;
        }
        let taken = |name: &str| {
            matches!(
                self.explorer.dirs.get(&edit.parent),
                Some(Listing::Loaded { entries, .. })
                    if entries.iter().any(|entry| entry.name == name)
            )
        };
        let problem = invalid_name(edit.kind, &text)
            .map(str::to_owned)
            .or_else(|| taken(&text).then(|| format!("“{text}” already exists in this folder")));
        if let Some(problem) = problem {
            // Clicking away abandons a name that cannot be used.
            if explicit {
                edit.error = Some(problem);
                edit.focus = true;
                self.ui.explorer.edit = Some(edit);
            }
            return;
        }
        let path = edit.parent.join(&text);
        let op = match (edit.kind, edit.target) {
            (EditKind::Rename, Some(from)) => Op::Rename { from, to: path },
            (EditKind::Rename, None) => return,
            (kind, _) => Op::Create {
                path,
                folder: kind == EditKind::NewFolder,
            },
        };
        if !self.explorer.request(ctx, Request::Apply(op)) {
            self.ui.error = Some("Could not start the file worker.".into());
        }
    }

    /// The explorer is no longer the tab in view: a name being typed is
    /// dropped, and a field that is leaving must not keep the keyboard.
    pub(super) fn leave_explorer(&mut self, ctx: &egui::Context) {
        self.ui.explorer.edit = None;
        ctx.memory_mut(|memory| {
            if let Some(id) = memory.focused().filter(|id| {
                use ui::explorer::{edit_id, exclude_id, query_id};
                [query_id(), exclude_id(), edit_id()].contains(id)
            }) {
                memory.surrender_focus(id);
            }
        });
        self.explorer.dirty = true;
    }

    pub(super) fn explorer_event(&mut self, ctx: &egui::Context, event: Event) {
        match event {
            Event::Expand(path, expanded) => {
                if expanded {
                    self.explorer.expanded.insert(path.clone());
                    self.explorer.list(ctx, &path);
                } else {
                    self.explorer.expanded.remove(&path);
                }
                self.ui.explorer.selected = Some((path, true));
                self.ui.explorer.scroll_to = None;
                self.explorer.dirty = true;
            }
            Event::Select(path) => {
                self.ui.explorer.markdown_anchor = None;
                self.ui.explorer.source_selection.reset();
                self.ui.explorer.preview_location = None;
                self.ui.explorer.preview_jump = false;
                self.ui.explorer.selected = Some((path.clone(), false));
                self.ui.explorer.scroll_to = None;
                self.explorer.show_preview(ctx, path, false);
            }
            Event::FollowLink { path, anchor } => {
                let loaded = self.explorer.preview.as_ref().is_some_and(|preview| {
                    preview.path == path
                        && matches!(
                            &preview.body,
                            Body::Text {
                                markdown: Some(_),
                                ..
                            }
                        )
                });
                if !loaded {
                    self.explorer_event(ctx, Event::Select(path));
                }
                self.ui.explorer.markdown_preview = true;
                self.ui.explorer.markdown_anchor = anchor;
            }
            Event::ShowInTree(path, dir) => {
                self.leave_search();
                self.explorer.uncover(ctx, &path);
                if dir && self.explorer.expanded.insert(path.clone()) {
                    self.explorer.list(ctx, &path);
                }
                self.ui.explorer.selected = Some((path.clone(), dir));
                self.ui.explorer.scroll_to = Some(ScrollTarget::Path(path));
            }
            Event::ClosePreview => {
                self.ui.explorer.source_selection.reset();
                self.explorer.preview = None;
                self.explorer.cancel_preview_work();
            }
            Event::BeginCreate { parent, folder } => {
                let Some(root) = self.explorer.root.clone() else {
                    return;
                };
                // Without a folder named, the new item joins the selection.
                let parent = parent
                    .or_else(|| match &self.ui.explorer.selected {
                        Some((path, true)) => Some(path.clone()),
                        Some((path, false)) => path.parent().map(Path::to_path_buf),
                        None => None,
                    })
                    .filter(|parent| parent.starts_with(&root))
                    .unwrap_or(root.clone());
                self.leave_search();
                if parent != root {
                    self.explorer.uncover(ctx, &parent);
                    self.explorer.expanded.insert(parent.clone());
                }
                self.explorer.list(ctx, &parent);
                self.ui.explorer.edit = Some(Edit {
                    kind: if folder {
                        EditKind::NewFolder
                    } else {
                        EditKind::NewFile
                    },
                    parent,
                    target: None,
                    text: String::new(),
                    focus: true,
                    error: None,
                });
                self.ui.explorer.scroll_to = Some(ScrollTarget::Edit);
                self.explorer.dirty = true;
            }
            Event::BeginRename(path) => {
                let Some(parent) = path.parent().map(Path::to_path_buf) else {
                    return;
                };
                self.leave_search();
                self.explorer.uncover(ctx, &path);
                self.ui.explorer.edit = Some(Edit {
                    kind: EditKind::Rename,
                    parent,
                    text: file_name(&path),
                    target: Some(path),
                    focus: true,
                    error: None,
                });
                self.ui.explorer.scroll_to = Some(ScrollTarget::Edit);
                self.explorer.dirty = true;
            }
            Event::Commit { explicit } => self.commit(ctx, explicit),
            Event::CancelEdit => {
                self.ui.explorer.edit = None;
                self.explorer.dirty = true;
            }
            Event::Retyped => self.explorer.dirty = true,
            Event::Delete(path) => {
                self.ui.explorer.edit = None;
                self.explorer.dirty = true;
                self.ui.explorer.delete = Some(path);
                self.ui.overlay = OverlayState::DeleteFile;
            }
            Event::ConfirmDelete => {
                self.ui.overlay = OverlayState::None;
                if let Some(path) = self.ui.explorer.delete.take()
                    && !self.explorer.request(ctx, Request::Apply(Op::Delete(path)))
                {
                    self.ui.error = Some("Could not start the file worker.".into());
                }
            }
            Event::Reveal(path) => self.hand_off(ctx, Handoff::Reveal(path)),
            Event::Open(path) => self.hand_off(ctx, Handoff::Open(path)),
            Event::OpenLink(link) => {
                if let Err(error) = self.link_opener.open(link, ctx.clone()) {
                    self.ui.error = Some(error.into());
                }
            }
            Event::CopyPath(path) => {
                crate::platform::clipboard::copy(ctx, path.display().to_string());
            }
            Event::CopyRelativePath(path) => {
                let relative = self
                    .explorer
                    .root
                    .as_deref()
                    .and_then(|root| path.strip_prefix(root).ok())
                    .map(|relative| {
                        if relative.as_os_str().is_empty() {
                            ".".into()
                        } else {
                            relative.display().to_string()
                        }
                    })
                    .unwrap_or_else(|| path.display().to_string());
                crate::platform::clipboard::copy(ctx, relative);
            }
            Event::Refresh => {
                self.explorer.refresh(ctx);
                if !self.ui.explorer.query.trim().is_empty() {
                    self.explorer.search.due = Some(ctx.input(|input| input.time));
                }
            }
            Event::CollapseAll => {
                self.explorer.expanded.clear();
                self.explorer.dirty = true;
            }
            Event::SearchChanged => {
                self.ui.explorer.scroll_to = None;
                self.ui.explorer.edit = None;
                self.explorer.dirty = true;
                if self.ui.explorer.query.trim().is_empty() {
                    self.explorer.cancel_search();
                } else {
                    self.explorer.search.due = Some(ctx.input(|input| input.time) + SEARCH_DELAY);
                    ctx.request_repaint();
                }
            }
        }
    }

    fn hand_off(&mut self, ctx: &egui::Context, handoff: Handoff) {
        if let Err(error) = self.file_opener.start(handoff, ctx.clone()) {
            self.ui.error = Some(error.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_fixture(root: &Path) -> (App, egui::Context, mpsc::Receiver<Request>) {
        let (mut app, _) = super::super::tests::fixture(root);
        app.startup = None;
        app.controller
            .dispatch(Command::AddWorkspace {
                group: None,
                cwd: root.into(),
                name: "Memory fixture".into(),
                remote: None,
            })
            .unwrap();
        // Exercise the real reply/rebuild boundary without filesystem timing.
        let (files, requests) = mpsc::channel();
        app.explorer.files = Some(files);
        let ctx = egui::Context::default();
        app.sync_explorer(&ctx);
        (app, ctx, requests)
    }

    fn cached_listing(app: &mut App, ctx: &egui::Context, dir: &Path, entries: Arc<[Entry]>) {
        app.explorer
            .replies
            .0
            .send(Reply::Listed {
                dir: dir.into(),
                listing: Listing::Loaded { entries, more: 0 },
            })
            .unwrap();
        app.poll_explorer(ctx);
        app.sync_explorer(ctx);
    }

    #[test]
    fn collapsed_folders_release_listings_and_reopening_reads_them_again() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, ctx, requests) = cache_fixture(root.path());
        let folders: Arc<[Entry]> = (0..32)
            .map(|index| Entry {
                name: format!("folder-{index}"),
                path: root.path().join(format!("folder-{index}")),
                dir: true,
                link: false,
            })
            .collect();
        cached_listing(&mut app, &ctx, root.path(), folders.clone());
        for folder in folders.iter() {
            app.explorer_event(&ctx, Event::Expand(folder.path.clone(), true));
            let entries: Arc<[Entry]> = (0..1000)
                .map(|index| Entry {
                    name: format!("file-{index}"),
                    path: folder.path.join(format!("file-{index}")),
                    dir: false,
                    link: false,
                })
                .collect();
            let released = Arc::downgrade(&entries);
            cached_listing(&mut app, &ctx, &folder.path, entries);
            assert_eq!(app.explorer.rows.len(), 1032);
            app.explorer_event(&ctx, Event::Expand(folder.path.clone(), false));
            app.sync_explorer(&ctx);
            assert!(released.upgrade().is_none(), "collapsed listing retained");
            assert_eq!(app.explorer.dirs.len(), 1);
            assert_eq!(app.explorer.rows.len(), 32);
        }
        let folder = &folders[0].path;
        app.explorer_event(&ctx, Event::Expand(folder.clone(), true));
        app.sync_explorer(&ctx);
        assert!(!app.explorer.dirs.contains_key(folder));
        assert!(
            requests
                .try_iter()
                .any(|request| matches!(request, Request::List(path) if &path == folder))
        );
        cached_listing(&mut app, &ctx, folder, Arc::from([]));
        assert!(app.explorer.dirs.contains_key(folder));
    }

    #[test]
    fn late_listings_do_not_restore_collapsed_or_hidden_folder_allocations() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, ctx, _requests) = cache_fixture(root.path());
        let folder = root.path().join("folder");
        cached_listing(
            &mut app,
            &ctx,
            root.path(),
            Arc::from([Entry {
                name: "folder".into(),
                path: folder.clone(),
                dir: true,
                link: false,
            }]),
        );
        app.explorer_event(&ctx, Event::Expand(folder.clone(), true));
        app.explorer_event(&ctx, Event::Expand(folder.clone(), false));
        app.sync_explorer(&ctx);
        app.explorer
            .replies
            .0
            .send(Reply::Listed {
                dir: folder.clone(),
                listing: Listing::Loaded {
                    entries: Arc::from([]),
                    more: 0,
                },
            })
            .unwrap();
        app.poll_explorer(&ctx);
        assert!(!app.explorer.dirs.contains_key(&folder));
        app.rest_explorer(&ctx);
        app.explorer
            .replies
            .0
            .send(Reply::Listed {
                dir: root.path().into(),
                listing: Listing::Loaded {
                    entries: Arc::from([]),
                    more: 0,
                },
            })
            .unwrap();
        app.poll_explorer(&ctx);
        assert!(app.explorer.dirs.is_empty());
        assert_eq!(app.explorer.rows.capacity(), 0);
    }

    fn names(path: &str) -> Vec<String> {
        path.split('/').map(str::to_owned).collect()
    }

    #[test]
    fn default_exclusions_cover_those_folders_at_any_depth_and_nothing_else() {
        let globs = Globs::parse(ui::explorer::DEFAULT_EXCLUDE);
        for path in [
            "node_modules",
            "node_modules/react/index.js",
            "web/node_modules",
            ".git",
            ".git/HEAD",
            "crates/core/target/debug/app",
            "dist",
            "site/build/index.html",
        ] {
            assert!(globs.matches(&names(path)), "{path}");
        }
        for path in [
            "src/main.rs",
            ".github/workflows/ci.yml",
            ".gitignore",
            "targets/a",
            "node_modules_backup/a",
            "rebuild/a",
        ] {
            assert!(!globs.matches(&names(path)), "{path}");
        }
    }

    #[test]
    fn patterns_support_wildcards_alternatives_and_bare_names() {
        let globs = Globs::parse(" *.log , tmp/, **/*.{png,jp?g}, docs/**/draft-*.md, {a,b}{1,2} ");
        for path in [
            "debug.log",
            "deep/er/x.log",
            "tmp",
            "a/tmp/file",
            "shot.png",
            "img/photo.jpeg",
            "docs/draft-1.md",
            "docs/x/y/draft-final.md",
            "a1",
            "x/b2",
        ] {
            assert!(globs.matches(&names(path)), "{path}");
        }
        for path in [
            "log",
            "debug.log.txt",
            "tmpx",
            "photo.jpg.bak",
            "notes/draft-1.md",
            "a3",
        ] {
            assert!(!globs.matches(&names(path)), "{path}");
        }
        assert_eq!(Globs::parse(" , ,"), Globs::default());
        assert!(!Globs::default().matches(&names("anything")));
        // An unclosed brace is an ordinary character, not a dropped pattern.
        assert!(Globs::parse("{odd").matches(&names("{odd")));
        // A star stands for a run of characters even before a literal star.
        assert!(wildcard("*", "*abc") && wildcard("a*c", "a*bc"));
        // Many groups stay within a fixed amount of work.
        let started = Instant::now();
        assert!(expand_braces(&"{a,b}".repeat(40)).len() <= 64);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn queries_match_words_in_the_path_with_one_in_the_name() {
        let words = Query::parse("  Main  SRC ").unwrap();
        assert!(words.matches(&names("src/main.rs")));
        assert!(!words.matches(&names("lib/main.rs")));
        let one = Query::parse("src").unwrap();
        assert!(one.matches(&names("a/src")));
        assert!(
            !one.matches(&names("src/lib.rs")),
            "only its folder matches"
        );
        let nested = Query::parse("app/expl").unwrap();
        assert!(nested.matches(&names("src/app/explorer.rs")));
        assert!(!nested.matches(&names("src/ui/explorer.rs")));
        let glob = Query::parse("*.RS").unwrap();
        assert!(glob.matches(&names("src/app/mod.rs")));
        assert!(!glob.matches(&names("src/app/mod.rs.bak")));
        assert_eq!(Query::parse("   "), None);
    }

    #[test]
    fn names_for_new_and_renamed_items_stay_inside_their_folder() {
        for name in ["a.txt", ".env", "notes/today.md", "a b"] {
            assert_eq!(invalid_name(EditKind::NewFile, name), None, "{name}");
        }
        for name in ["..", ".", "../x", "/etc/passwd", "a/../b", "a/"] {
            assert!(invalid_name(EditKind::NewFolder, name).is_some(), "{name}");
        }
        assert!(invalid_name(EditKind::Rename, "a/b").is_some());
        assert_eq!(invalid_name(EditKind::Rename, "b.txt"), None);
    }

    #[test]
    fn listings_show_hidden_files_and_put_folders_first() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["b.txt", ".env", "A.txt"] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        for name in ["src", ".git"] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
        }
        let Listing::Loaded { entries, more } = list(dir.path()) else {
            panic!("the folder was not read");
        };
        assert_eq!(more, 0);
        let shown: Vec<_> = entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.dir))
            .collect();
        assert_eq!(
            shown,
            [
                (".git", true),
                ("src", true),
                (".env", false),
                ("A.txt", false),
                ("b.txt", false)
            ]
        );
        assert!(matches!(
            list(&dir.path().join("missing")),
            Listing::Failed(_)
        ));
    }

    #[test]
    fn changes_never_replace_what_exists_and_deleting_a_link_keeps_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let file = root.join("a/b/new.txt");
        apply(&Op::Create {
            path: file.clone(),
            folder: false,
        })
        .unwrap();
        assert!(file.is_file());
        std::fs::write(&file, "kept").unwrap();
        assert!(
            apply(&Op::Create {
                path: file.clone(),
                folder: false
            })
            .is_err()
        );
        assert!(
            apply(&Op::Create {
                path: file.clone(),
                folder: true
            })
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "kept");

        apply(&Op::Create {
            path: root.join("made"),
            folder: true,
        })
        .unwrap();
        assert!(root.join("made").is_dir());

        let other = root.join("a/b/other.txt");
        std::fs::write(&other, "other").unwrap();
        let taken = apply(&Op::Rename {
            from: file.clone(),
            to: other.clone(),
        });
        assert!(taken.unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "other");
        let renamed = root.join("a/b/renamed.txt");
        apply(&Op::Rename {
            from: file.clone(),
            to: renamed.clone(),
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&renamed).unwrap(), "kept");
        assert!(!file.exists());

        #[cfg(unix)]
        {
            let link = root.join("link");
            std::os::unix::fs::symlink(root.join("a"), &link).unwrap();
            apply(&Op::Delete(link.clone())).unwrap();
            assert!(std::fs::symlink_metadata(&link).is_err());
            assert!(renamed.is_file(), "the link's target was deleted");
        }
        apply(&Op::Delete(root.join("a"))).unwrap();
        assert!(!root.join("a").exists());
        assert!(apply(&Op::Delete(root.join("a"))).is_err());
    }

    #[test]
    fn searching_skips_excluded_folders_and_reports_every_match_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for path in [
            "src/main.rs",
            "src/app/main_window.rs",
            "node_modules/pkg/main.js",
            "target/debug/main",
            ".git/main",
            "README.md",
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(root, root.join("src/loop")).unwrap();
        let run = |exclude: &str| {
            let (replies, results) = mpsc::channel();
            search_worker(
                root.into(),
                Query::parse("main").unwrap(),
                Globs::parse(exclude),
                7,
                Arc::new(AtomicU64::new(7)),
                replies,
                egui::Context::default(),
            );
            let mut found = Vec::new();
            let mut finished = false;
            for reply in results.try_iter() {
                let Reply::Hits {
                    search, hits, done, ..
                } = reply
                else {
                    panic!("a search only reports hits");
                };
                assert_eq!(search, 7);
                finished |= done;
                found.extend(hits.into_iter().map(|hit| {
                    hit.path
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/")
                }));
            }
            assert!(finished);
            found.sort();
            found
        };
        assert_eq!(
            run(ui::explorer::DEFAULT_EXCLUDE),
            ["src/app/main_window.rs", "src/main.rs"]
        );
        assert_eq!(
            run(""),
            [
                ".git/main",
                "node_modules/pkg/main.js",
                "src/app/main_window.rs",
                "src/main.rs",
                "target/debug/main"
            ]
        );

        // A search that was superseded reports nothing more.
        let (replies, results) = mpsc::channel();
        search_worker(
            root.into(),
            Query::parse("main").unwrap(),
            Globs::default(),
            7,
            Arc::new(AtomicU64::new(8)),
            replies,
            egui::Context::default(),
        );
        assert!(results.try_iter().next().is_none());
    }

    #[test]
    fn markdown_previews_are_parsed_on_read_and_keep_bounded_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.MD");
        std::fs::write(
            &path,
            "# Heading\n\n- **bold**\n\n```rust\nlet answer = 42;\n```\n",
        )
        .unwrap();
        let Loaded::Text {
            lines,
            markdown: Some(blocks),
            ..
        } = read_preview(&path).1
        else {
            panic!("Markdown is parsed on the worker's read path");
        };
        assert_eq!(lines[0], "# Heading");
        assert!(
            matches!(&blocks[0].block, ui::markup::Block::Heading(1, text) if text == "Heading")
        );
        assert!(matches!(&blocks[1].block, ui::markup::Block::Bullet(text) if text == "**bold**"));
        assert!(
            matches!(&blocks[2].block, ui::markup::Block::Code(text) if text == "let answer = 42;")
        );

        std::fs::write(&path, "x".repeat(PREVIEW_COLUMNS + 20)).unwrap();
        assert!(matches!(
            read_preview(&path).1,
            Loaded::Text {
                truncated: true,
                markdown: Some(_),
                ..
            }
        ));
        let plain = dir.path().join("notes.txt");
        std::fs::write(&plain, "# Heading").unwrap();
        assert!(matches!(
            read_preview(&plain).1,
            Loaded::Text { markdown: None, .. }
        ));
    }

    #[test]
    fn markdown_code_keeps_original_tabs_and_long_commands_within_file_limits() {
        let command = format!("printf 'literal\ttab' {}", "argument ".repeat(160));
        let source = format!("```sh\n{command}\n```\n");
        let Loaded::Text {
            lines,
            markdown: Some(blocks),
            truncated,
            ..
        } = text_preview(source.as_bytes(), true)
        else {
            panic!("Markdown document");
        };
        assert!(truncated);
        assert!(lines[1].ends_with('…'));
        assert!(matches!(&blocks[0].block, ui::markup::Block::Code(code) if code == &command));

        let source = format!(
            "{}\n```sh\nnot included\n```",
            "paragraph\n".repeat(PREVIEW_LINES)
        );
        let Loaded::Text {
            markdown: Some(blocks),
            truncated,
            ..
        } = text_preview(source.as_bytes(), true)
        else {
            panic!("bounded Markdown document");
        };
        assert!(truncated);
        assert!(
            !blocks
                .iter()
                .any(|block| matches!(block.block, ui::markup::Block::Code(_)))
        );
    }

    #[test]
    fn cancelling_a_preview_skips_pending_images_and_allows_the_next_file() {
        use std::io::{Read as _, Write as _};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("README.md");
        let next = dir.path().join("next.txt");
        std::fs::write(&next, "next file").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::fs::write(
            &path,
            format!("![One](http://{address}/one.png)\n\n![Two](http://{address}/two.png)"),
        )
        .unwrap();
        let current = Arc::new(AtomicU64::new(1));
        let cancellation = current.clone();
        let (cancelled, wait) = mpsc::channel();
        let server = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => panic!("image request did not arrive: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 1024];
            stream.read(&mut request).unwrap();
            cancellation.store(0, Ordering::Relaxed);
            cancelled.send(()).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        });
        let (send, requests) = mpsc::channel();
        let (replies, receive) = mpsc::channel();
        let running = current.clone();
        let worker = std::thread::spawn(move || {
            preview_worker(requests, replies, egui::Context::default(), running)
        });
        send.send((1, path)).unwrap();
        assert!(matches!(
            receive.recv_timeout(Duration::from_secs(5)).unwrap(),
            Reply::Preview { request: 1, .. }
        ));
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        current.store(2, Ordering::Relaxed);
        send.send((2, next)).unwrap();
        assert!(matches!(
            receive.recv_timeout(Duration::from_secs(5)).unwrap(),
            Reply::Preview { request: 2, .. }
        ));
        drop(send);
        worker.join().unwrap();
        server.join().unwrap();
        assert!(receive.try_iter().next().is_none());
    }

    #[test]
    fn markdown_image_worker_reports_text_before_bounded_pictures() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("README.md");
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        image::RgbaImage::from_pixel(1200, 600, image::Rgba([80, 120, 240, 255]))
            .save(dir.path().join("assets/logo.png"))
            .unwrap();
        std::fs::write(
            &path,
            "# Neptune\n\n<img src=\"assets/logo.png\" width=96 height=48>\n",
        )
        .unwrap();
        let (send, requests) = mpsc::channel();
        let (replies, receive) = mpsc::channel();
        send.send((9, path)).unwrap();
        let worker = std::thread::spawn(move || {
            preview_worker(
                requests,
                replies,
                egui::Context::default(),
                Arc::new(AtomicU64::new(9)),
            )
        });
        assert!(matches!(
            receive.recv_timeout(Duration::from_secs(5)).unwrap(),
            Reply::Preview {
                request: 9,
                loaded: Loaded::Text {
                    markdown: Some(_),
                    ..
                },
                ..
            }
        ));
        let Reply::Picture {
            request,
            index,
            pixels,
            image,
        } = receive.recv_timeout(Duration::from_secs(5)).unwrap()
        else {
            panic!("image follows text");
        };
        assert_eq!((request, index, pixels), (9, 1, [1200, 600]));
        assert_eq!(image.size, [1024, 512]);
        drop(send);
        worker.join().unwrap();
    }

    #[test]
    fn remote_markdown_images_use_the_bounded_native_decoder() {
        use std::io::{Read as _, Write as _};
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(4, 2, image::Rgba([60, 80, 100, 255]))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let bytes = png.into_inner();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => panic!("image request did not arrive: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 1024];
            stream.read(&mut request).unwrap();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).unwrap();
            stream.write_all(&bytes).unwrap();
        });
        let picture = document_picture(
            &format!("http://{address}/logo.png"),
            Path::new("/project/README.md"),
        );
        server.join().unwrap();
        assert_eq!(picture.unwrap().0, [4, 2]);
    }

    #[test]
    fn previews_read_text_as_lines_and_recognise_other_files() {
        let Loaded::Text {
            lines,
            widest,
            truncated,
            ..
        } = text_preview(b"fn main() {\r\n\tprintln!(\"hi\");\n}\n", false)
        else {
            panic!("source is text");
        };
        assert_eq!(lines, ["fn main() {", "    println!(\"hi\");", "}"]);
        assert_eq!(widest, 19);
        assert!(!truncated);
        assert!(matches!(
            text_preview(b"\x7fELF\0\0\0", false),
            Loaded::Binary
        ));
        assert!(matches!(
            text_preview(b"", false),
            Loaded::Text { lines, .. } if lines.is_empty()
        ));
        let long = "x\n".repeat(PREVIEW_BYTES);
        assert!(matches!(
            text_preview(&long.as_bytes()[..PREVIEW_BYTES + 1], false),
            Loaded::Text { lines, truncated: true, .. } if lines.len() == PREVIEW_LINES
        ));

        let minified = "x".repeat(PREVIEW_BYTES + 1);
        assert!(matches!(
            text_preview(minified.as_bytes(), false),
            Loaded::Text { lines, truncated: true, .. } if lines.len() == 1
        ));

        let dir = tempfile::tempdir().unwrap();
        let (size, loaded) = read_preview(dir.path());
        assert_eq!(size, None);
        assert!(matches!(loaded, Loaded::Failed(_)));
        let picture = dir.path().join("dot.png");
        image::RgbaImage::from_pixel(3, 2, image::Rgba([255, 0, 0, 255]))
            .save(&picture)
            .unwrap();
        assert!(matches!(
            read_preview(&picture).1,
            Loaded::Image { pixels: [3, 2], .. }
        ));
    }

    #[test]
    fn the_tree_lists_open_folders_with_their_edit_rows_in_place() {
        let root = PathBuf::from("/project");
        let entry = |name: &str, dir: bool| Entry {
            name: name.into(),
            path: root.join(name),
            dir,
            link: false,
        };
        let mut explorer = Explorer {
            root: Some(root.clone()),
            ..Default::default()
        };
        explorer.dirs.insert(
            root.clone(),
            Listing::Loaded {
                entries: vec![entry("src", true), entry("a.txt", false)].into(),
                more: 0,
            },
        );
        explorer.expanded.insert(root.join("src"));
        let (shown, missing) = explorer.rebuild(None);
        assert_eq!(shown, [root.clone(), root.join("src")]);
        assert_eq!(missing, [root.join("src")]);
        let labels = |explorer: &Explorer| -> Vec<String> {
            explorer
                .rows
                .iter()
                .map(|row| match row.kind {
                    RowKind::Edit { folder } => format!("{}edit:{folder}", row.depth),
                    _ => format!("{}{}", row.depth, row.name),
                })
                .collect()
        };
        assert_eq!(labels(&explorer), ["0src", "1Loading…", "0a.txt"]);

        explorer.dirs.insert(
            root.join("src"),
            Listing::Loaded {
                entries: Vec::new().into(),
                more: 0,
            },
        );
        let create = Edit {
            kind: EditKind::NewFolder,
            parent: root.join("src"),
            target: None,
            text: String::new(),
            focus: true,
            error: None,
        };
        explorer.rebuild(Some(&create));
        assert_eq!(labels(&explorer), ["0src", "1edit:true", "0a.txt"]);

        let rename = Edit {
            kind: EditKind::Rename,
            parent: root.clone(),
            target: Some(root.join("a.txt")),
            text: "a.txt".into(),
            focus: true,
            error: Some("Taken".into()),
        };
        explorer.rebuild(Some(&rename));
        assert_eq!(
            labels(&explorer),
            ["0src", "1Empty folder", "0edit:false", "0Taken"]
        );
    }

    const WINDOW: egui::Vec2 = egui::vec2(900.0, 640.0);

    /// One whole frame of the application, as the window would run it.
    fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> Vec<String> {
        let mut host = eframe::Frame::_new_kittest();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, WINDOW)),
                events,
                ..Default::default()
            },
            |ui| {
                app.poll_explorer(ui.ctx());
                eframe::App::ui(app, ui, &mut host);
            },
        );
        output.textures_delta.clear();
        output
            .platform_output
            .commands
            .into_iter()
            .filter_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text),
                _ => None,
            })
            .collect()
    }

    /// Runs frames until the workers have brought about `done`.
    fn settle(app: &mut App, ctx: &egui::Context, what: &str, done: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            frame(app, ctx, Vec::new());
            if done(app) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn shown(app: &App) -> Vec<String> {
        app.explorer
            .rows
            .iter()
            .map(|row| format!("{}{}", "  ".repeat(row.depth), row.name))
            .collect()
    }

    fn opened(root: &Path) -> (App, egui::Context) {
        let (mut app, _sender) = super::super::tests::fixture(root);
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        app.startup = None;
        app.controller
            .dispatch(Command::AddWorkspace {
                group: None,
                cwd: root.into(),
                name: "Project".into(),
                remote: None,
            })
            .unwrap();
        app.action(&ctx, Action::Panel(ui::panel::Event::Toggle));
        assert!(app.ui.panel.open);
        (app, ctx)
    }

    fn act(app: &mut App, ctx: &egui::Context, event: Event) {
        app.action(ctx, Action::Explorer(event));
    }

    fn type_name(app: &mut App, ctx: &egui::Context, name: &str) {
        app.ui
            .explorer
            .edit
            .as_mut()
            .expect("a name is being typed")
            .text = name.into();
        act(app, ctx, Event::Commit { explicit: true });
    }

    #[test]
    fn copying_file_text_reserves_the_chord_until_the_preview_closes() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, _sender) = super::super::tests::fixture(root.path());
        app.ui.panel.open = true;
        app.explorer.root = Some(root.path().into());
        app.explorer.preview = Some(Preview {
            path: root.path().join("source.rs"),
            name: "source.rs".into(),
            request: 1,
            revision: 1,
            size: None,
            body: Body::Text {
                lines: vec!["copy this text".into()],
                widest: 14,
                truncated: false,
                markdown: None,
            },
        });
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        let mut tick = 0;
        let mut run = |app: &mut App, events: Vec<egui::Event>| {
            tick += 1;
            let mut writes = 0;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(tick as f64 * 0.02),
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, WINDOW)),
                    events,
                    ..Default::default()
                },
                |ui| {
                    app.shortcuts(ui.ctx());
                    let events = app.terminal_events(ui.ctx());
                    let normalized =
                        crate::input::normalize_events(&events, ui.input(|input| input.modifiers));
                    writes = crate::input::route_events(
                        crate::input::RoutingContext::TerminalPane(1),
                        &normalized,
                        terminal_core::Mode::empty(),
                    )
                    .iter()
                    .filter(|event| matches!(event.action, crate::input::InputAction::Write(_)))
                    .count();
                    let view =
                        app.explorer
                            .view(false, 1.0, Rect::from_min_size(Pos2::ZERO, WINDOW));
                    ui::explorer::show(
                        ui,
                        Rect::from_min_max(Pos2::new(580.0, 44.0), WINDOW.to_pos2()),
                        Palette::for_config(&app.config),
                        &view,
                        &mut app.ui.explorer,
                        &mut Vec::new(),
                    );
                },
            );
            output.textures_delta.clear();
            (output, writes)
        };
        for _ in 0..12 {
            run(&mut app, Vec::new());
        }
        let (output, _) = run(&mut app, Vec::new());
        let rect = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "copy this text" => {
                    Some(text.visual_bounding_rect())
                }
                _ => None,
            })
            .unwrap();
        let (from, to) = (
            rect.left_center(),
            rect.right_center() + egui::vec2(1.0, 0.0),
        );
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [
            vec![egui::Event::PointerMoved(from)],
            vec![button(from, true)],
            vec![egui::Event::PointerMoved(to)],
            vec![button(to, false)],
        ] {
            run(&mut app, events);
        }
        assert!(app.explorer_text_selected());
        let ctrl = egui::Modifiers::CTRL;
        let (output, writes) = run(
            &mut app,
            vec![egui::Event::ModifiersChanged(ctrl), egui::Event::Copy],
        );
        assert_eq!(writes, 0, "copying the file must not interrupt the shell");
        assert!(output.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "copy this text")));
        if !cfg!(target_os = "macos") {
            let modifiers = ctrl | egui::Modifiers::SHIFT;
            let (output, writes) = run(
                &mut app,
                vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::Key {
                        key: egui::Key::C,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                ],
            );
            assert_eq!(writes, 0);
            assert!(output.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "copy this text")));
        }
        app.ui.overlay = OverlayState::Palette;
        app.release_closed_overlay_focus(&ctx);
        assert!(
            !app.explorer_text_selected(),
            "a dialog owns its clipboard commands"
        );
        app.ui.overlay = OverlayState::None;
        app.release_closed_overlay_focus(&ctx);
        act(&mut app, &ctx, Event::ClosePreview);
        let (_, writes) = run(
            &mut app,
            vec![egui::Event::ModifiersChanged(ctrl), egui::Event::Copy],
        );
        assert_eq!(
            writes, 1,
            "Ctrl+C returns to the shell after closing the preview"
        );
    }

    #[test]
    fn hiding_and_closing_cancel_preview_work_and_files_resume_on_return() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("README.md");
        std::fs::write(&path, "# Heading\n\n[Top](#heading)").unwrap();
        let (mut app, _) = super::super::tests::fixture(root.path());
        let ctx = egui::Context::default();
        app.explorer.show_preview(&ctx, path.clone(), false);
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.explorer.preview.as_ref().unwrap().revision == 0 {
            assert!(Instant::now() < deadline);
            app.poll_explorer(&ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
        let request = app.explorer.preview_requests;
        act(
            &mut app,
            &ctx,
            Event::FollowLink {
                path,
                anchor: Some("heading".into()),
            },
        );
        assert_eq!(
            app.explorer.preview_requests, request,
            "same-file anchors use the loaded document"
        );
        app.rest_explorer(&ctx);
        assert_eq!(app.explorer.preview_current.load(Ordering::Relaxed), 0);
        assert!(app.explorer.preview_paused);
        assert_eq!(app.explorer.preview.as_ref().unwrap().request, 0);
        app.sync_explorer(&ctx);
        assert!(app.explorer.preview_requests > request);
        assert!(!app.explorer.preview_paused);
        act(&mut app, &ctx, Event::ClosePreview);
        assert!(app.explorer.preview.is_none());
        assert!(!app.explorer.preview_paused);
        assert_eq!(app.explorer.preview_current.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn late_markdown_pictures_cannot_update_a_replacement_or_closed_preview() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, _sender) = super::super::tests::fixture(root.path());
        let ctx = egui::Context::default();
        app.explorer.preview = Some(Preview {
            path: root.path().join("notes.md"),
            name: "notes.md".into(),
            request: 2,
            revision: 2,
            size: None,
            body: Body::Text {
                lines: Vec::new(),
                widest: 0,
                truncated: false,
                markdown: Some(ui::markup::Document::new(Vec::new())),
            },
        });
        let reply = |request| Reply::Picture {
            request,
            index: 0,
            pixels: [1, 1],
            image: egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
        };
        app.explorer.replies.0.send(reply(1)).unwrap();
        app.poll_explorer(&ctx);
        let pictures = |app: &App| match &app.explorer.preview.as_ref().unwrap().body {
            Body::Text {
                markdown: Some(document),
                ..
            } => document.pictures.len(),
            _ => panic!("document stays open"),
        };
        assert_eq!(pictures(&app), 0);
        app.explorer.replies.0.send(reply(2)).unwrap();
        app.poll_explorer(&ctx);
        assert_eq!(pictures(&app), 1);
        app.explorer
            .replies
            .0
            .send(Reply::PictureFailed {
                request: 1,
                index: 0,
            })
            .unwrap();
        app.poll_explorer(&ctx);
        assert_eq!(
            pictures(&app),
            1,
            "a stale failure cannot remove a current picture"
        );
        app.explorer
            .replies
            .0
            .send(Reply::PictureFailed {
                request: 2,
                index: 0,
            })
            .unwrap();
        app.poll_explorer(&ctx);
        assert_eq!(
            pictures(&app),
            0,
            "a failed refresh removes its retained picture"
        );
        app.explorer_event(&ctx, Event::ClosePreview);
        app.explorer.replies.0.send(reply(2)).unwrap();
        app.poll_explorer(&ctx);
        assert!(app.explorer.preview.is_none());
    }

    #[test]
    fn the_panel_shows_the_terminals_folder_with_hidden_files_and_follows_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
        std::fs::write(root.join(".env"), "KEY=1\n").unwrap();
        let (mut app, ctx) = opened(root);
        settle(&mut app, &ctx, "the folder to be read", |app| {
            shown(app) == [".git", "src", ".env"]
        });

        act(&mut app, &ctx, Event::Expand(root.join("src"), true));
        settle(&mut app, &ctx, "the opened folder", |app| {
            shown(app) == [".git", "src", "  lib.rs", ".env"]
        });

        // A file made by a program in the terminal appears by itself.
        std::fs::write(root.join("src/made-elsewhere.rs"), "").unwrap();
        settle(&mut app, &ctx, "a file made elsewhere", |app| {
            shown(app) == [".git", "src", "  lib.rs", "  made-elsewhere.rs", ".env"]
        });

        act(&mut app, &ctx, Event::Select(root.join("src/lib.rs")));
        settle(&mut app, &ctx, "the preview", |app| {
            matches!(
                app.explorer.preview.as_ref().map(|preview| &preview.body),
                Some(Body::Text { lines, .. }) if lines == &["pub fn f() {}"]
            )
        });
        assert_eq!(app.explorer.preview.as_ref().unwrap().size, Some(14));
        // An edit made elsewhere reaches the preview by itself.
        std::fs::write(root.join("src/lib.rs"), "pub fn g() -> u8 {\n    1\n}\n").unwrap();
        settle(&mut app, &ctx, "the edited file's preview", |app| {
            matches!(
                app.explorer.preview.as_ref().map(|preview| &preview.body),
                Some(Body::Text { lines, .. }) if lines.len() == 3
            )
        });

        // Named as the tree names it, with the platform's separators.
        let copied = root.join("src").join("lib.rs");
        act(&mut app, &ctx, Event::CopyPath(copied.clone()));
        assert_eq!(
            frame(&mut app, &ctx, Vec::new()),
            [copied.clone().display().to_string()]
        );
        act(&mut app, &ctx, Event::CopyRelativePath(copied.clone()));
        assert_eq!(
            frame(&mut app, &ctx, Vec::new()),
            [Path::new("src").join("lib.rs").display().to_string()]
        );

        // The terminal changing directory changes the folder shown.
        let pane = app.controller.model().active_pane().unwrap();
        let generation = app.controller.model().pane(pane).unwrap().generation();
        app.controller
            .dispatch(Command::PaneCwdChanged {
                pane,
                generation,
                cwd: root.join("src"),
            })
            .unwrap();
        settle(&mut app, &ctx, "the terminal's new folder", |app| {
            shown(app) == ["lib.rs", "made-elsewhere.rs"]
        });

        // Closed, it reads nothing; opened again, it is current.
        app.action(&ctx, Action::Panel(ui::panel::Event::Toggle));
        settle(&mut app, &ctx, "the panel to close", |app| {
            app.ui.panel.slide.is_none() && app.explorer.watched.is_empty()
        });
        std::fs::remove_file(root.join("src/made-elsewhere.rs")).unwrap();
        app.action(&ctx, Action::Panel(ui::panel::Event::Toggle));
        settle(&mut app, &ctx, "the reopened panel", |app| {
            shown(app) == ["lib.rs"]
        });
    }

    #[test]
    fn items_are_created_renamed_and_deleted_from_the_panel() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("keep.txt"), "kept").unwrap();
        let (mut app, ctx) = opened(root);
        settle(&mut app, &ctx, "the folder to be read", |app| {
            shown(app) == ["keep.txt"]
        });

        act(
            &mut app,
            &ctx,
            Event::BeginCreate {
                parent: None,
                folder: false,
            },
        );
        frame(&mut app, &ctx, Vec::new());
        assert!(matches!(
            app.explorer.rows[0].kind,
            RowKind::Edit { folder: false }
        ));
        assert_eq!(
            ctx.memory(|memory| memory.focused()),
            Some(ui::explorer::edit_id()),
            "the name field takes the keyboard"
        );
        // It keeps it, and the edit, over the frames that follow.
        for _ in 0..3 {
            frame(&mut app, &ctx, Vec::new());
        }
        assert!(app.ui.explorer.edit.is_some());
        assert_eq!(
            ctx.memory(|memory| memory.focused()),
            Some(ui::explorer::edit_id())
        );

        // A name that is taken keeps the field open and says why.
        type_name(&mut app, &ctx, "keep.txt");
        let edit = app.ui.explorer.edit.as_ref().expect("the field stays open");
        assert!(edit.error.as_deref().unwrap().contains("already exists"));
        frame(&mut app, &ctx, Vec::new());
        assert!(matches!(
            app.explorer.rows[1].kind,
            RowKind::Note { error: true }
        ));
        type_name(&mut app, &ctx, "../escape.txt");
        assert!(app.ui.explorer.edit.as_ref().unwrap().error.is_some());
        assert!(!root.parent().unwrap().join("escape.txt").exists());

        type_name(&mut app, &ctx, "notes/today.md");
        let made = root.join("notes/today.md");
        settle(&mut app, &ctx, "the new file", |app| {
            shown(app) == ["notes", "  today.md", "keep.txt"]
        });
        assert!(made.is_file() && app.ui.explorer.edit.is_none());
        assert_eq!(app.ui.explorer.selected, Some((made.clone(), false)));
        assert_eq!(
            std::fs::read_to_string(root.join("keep.txt")).unwrap(),
            "kept"
        );

        act(
            &mut app,
            &ctx,
            Event::BeginCreate {
                parent: Some(root.join("notes")),
                folder: true,
            },
        );
        type_name(&mut app, &ctx, "drafts");
        settle(&mut app, &ctx, "the new folder", |app| {
            shown(app) == ["notes", "  drafts", "  today.md", "keep.txt"]
        });
        assert!(root.join("notes/drafts").is_dir());

        // Renaming keeps what was open, selected and previewed on the item.
        act(&mut app, &ctx, Event::BeginRename(root.join("notes")));
        assert_eq!(app.ui.explorer.edit.as_ref().unwrap().text, "notes");
        type_name(&mut app, &ctx, "journal");
        let moved = root.join("journal/today.md");
        settle(&mut app, &ctx, "the renamed folder", |app| {
            shown(app) == ["journal", "  drafts", "  today.md", "keep.txt"]
        });
        assert!(moved.is_file() && !root.join("notes").exists());
        assert_eq!(
            app.ui.explorer.selected,
            Some((root.join("journal/drafts"), true))
        );
        assert_eq!(app.explorer.preview.as_ref().unwrap().path, moved);

        act(&mut app, &ctx, Event::BeginRename(moved.clone()));
        type_name(&mut app, &ctx, "drafts");
        assert!(app.ui.explorer.edit.as_ref().unwrap().error.is_some());
        act(&mut app, &ctx, Event::CancelEdit);
        assert!(moved.is_file());

        // Deleting waits for the sheet; leaving the sheet deletes nothing.
        act(&mut app, &ctx, Event::Delete(root.join("journal")));
        assert_eq!(app.ui.overlay, OverlayState::DeleteFile);
        frame(&mut app, &ctx, Vec::new());
        app.action(&ctx, Action::CloseOverlay);
        assert_eq!(app.ui.overlay, OverlayState::None);
        assert!(app.ui.explorer.delete.is_none());
        act(&mut app, &ctx, Event::ConfirmDelete);
        frame(&mut app, &ctx, Vec::new());
        std::thread::sleep(Duration::from_millis(50));
        assert!(moved.is_file(), "a dismissed sheet must not delete");

        act(&mut app, &ctx, Event::Delete(root.join("journal")));
        act(&mut app, &ctx, Event::ConfirmDelete);
        settle(&mut app, &ctx, "the deletion", |app| {
            shown(app) == ["keep.txt"]
        });
        assert!(!root.join("journal").exists());
        assert!(app.ui.explorer.selected.is_none() && app.explorer.preview.is_none());
        assert!(app.ui.error.is_none(), "{:?}", app.ui.error);
    }

    #[test]
    fn searching_uses_the_exclusions_and_escape_leaves_it_a_step_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for path in [
            "src/config.rs",
            "node_modules/pkg/config.js",
            "build/config.h",
        ] {
            std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
            std::fs::write(root.join(path), "").unwrap();
        }
        let (mut app, ctx) = opened(root);
        assert_eq!(app.ui.explorer.exclude, ui::explorer::DEFAULT_EXCLUDE);
        let found = |app: &App| -> Vec<String> {
            let mut found: Vec<_> = app
                .explorer
                .search
                .hits
                .iter()
                .map(|hit| format!("{}/{}", hit.folder, hit.name))
                .collect();
            found.sort();
            found
        };
        let finished =
            |app: &App| !app.explorer.search.running && app.explorer.search.due.is_none();

        app.ui.explorer.query = "config".into();
        act(&mut app, &ctx, Event::SearchChanged);
        settle(&mut app, &ctx, "the search", |app| {
            finished(app) && !app.explorer.search.hits.is_empty()
        });
        assert_eq!(found(&app), ["src/config.rs"]);

        app.ui.explorer.exclude = "**/build/**".into();
        act(&mut app, &ctx, Event::SearchChanged);
        settle(&mut app, &ctx, "the search with other exclusions", |app| {
            finished(app) && app.explorer.search.hits.len() == 2
        });
        assert_eq!(found(&app), ["node_modules/pkg/config.js", "src/config.rs"]);

        // A result that is a file previews; one in a folder shows in the tree.
        act(
            &mut app,
            &ctx,
            Event::ShowInTree(root.join("src/config.rs"), false),
        );
        assert!(app.ui.explorer.query.is_empty());
        settle(&mut app, &ctx, "the tree around the result", |app| {
            shown(app) == ["build", "node_modules", "src", "  config.rs"]
        });

        let escape = || egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        // Whether the application kept the key from the shell.
        let press = |app: &mut App| {
            let mut consumed = false;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events: vec![escape()],
                    ..Default::default()
                },
                |ui| {
                    app.shortcuts(ui.ctx());
                    consumed = !ui.input(|input| input.key_pressed(egui::Key::Escape));
                },
            );
            output.textures_delta.clear();
            consumed
        };
        let focused = || ctx.memory(|memory| memory.focused());
        assert!(
            !press(&mut app),
            "with nothing of the panel's focused, Escape is the shell's"
        );
        app.ui.explorer.query = "config".into();
        ctx.memory_mut(|memory| memory.request_focus(ui::explorer::query_id()));
        frame(&mut app, &ctx, Vec::new());
        assert_eq!(focused(), Some(ui::explorer::query_id()));
        frame(&mut app, &ctx, vec![escape()]);
        assert!(
            app.ui.explorer.query.is_empty(),
            "first the search is cleared"
        );
        assert_eq!(focused(), Some(ui::explorer::query_id()));
        frame(&mut app, &ctx, vec![escape()]);
        assert_eq!(focused(), None, "then the field is left");
        frame(&mut app, &ctx, Vec::new());
        assert!(!press(&mut app));

        act(
            &mut app,
            &ctx,
            Event::BeginRename(root.join("src/config.rs")),
        );
        assert!(press(&mut app));
        assert!(app.ui.explorer.edit.is_none());
        assert!(root.join("src/config.rs").is_file());
    }

    #[test]
    fn a_remote_workspace_shows_no_local_folder_and_the_shortcut_toggles_the_panel() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx) = opened(dir.path());
        settle(&mut app, &ctx, "the local folder", |app| {
            app.explorer.root.is_some()
        });
        app.controller
            .dispatch(Command::AddWorkspace {
                group: None,
                cwd: dir.path().into(),
                name: "Server".into(),
                remote: Some("user@example.com".into()),
            })
            .unwrap();
        frame(&mut app, &ctx, Vec::new());
        assert!(app.explorer.root.is_none() && app.explorer.rows.is_empty());
        assert!(app.explorer.notice.contains("SSH"));
        // Nothing can be made where no folder is shown.
        act(
            &mut app,
            &ctx,
            Event::BeginCreate {
                parent: None,
                folder: false,
            },
        );
        assert!(app.ui.explorer.edit.is_none());

        let chord = if cfg!(target_os = "macos") {
            egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND
        } else {
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT
        };
        let toggle = egui::Event::Key {
            key: egui::Key::O,
            physical_key: Some(egui::Key::O),
            pressed: true,
            repeat: false,
            modifiers: chord,
        };
        frame(&mut app, &ctx, vec![toggle.clone()]);
        assert!(!app.ui.panel.open);
        assert!(app.ui.panel.slide.is_some(), "the panel slides away");
        frame(&mut app, &ctx, vec![toggle]);
        assert!(app.ui.panel.open);
    }

    #[test]
    fn the_toolbar_button_toggles_the_panel_and_its_edge_resizes_it() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx) = opened(dir.path());
        let button = |pos: Pos2, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let click = |app: &mut App, pos: Pos2| {
            frame(app, &ctx, vec![egui::Event::PointerMoved(pos)]);
            frame(app, &ctx, vec![button(pos, true)]);
            frame(app, &ctx, vec![button(pos, false)]);
        };
        settle(&mut app, &ctx, "the panel to open", |app| {
            app.ui.panel.slide.is_none()
        });
        // The trailing control of the toolbar, with the sidebar showing.
        let toggle = Pos2::new(WINDOW.x - 22.0, metrics::TOOLBAR_HEIGHT * 0.5);
        click(&mut app, toggle);
        assert!(!app.ui.panel.open, "the toolbar button closes the panel");
        settle(&mut app, &ctx, "the panel to close", |app| {
            app.ui.panel.slide.is_none()
        });
        click(&mut app, toggle);
        assert!(app.ui.panel.open, "and opens it again");
        settle(&mut app, &ctx, "the panel to open", |app| {
            app.ui.panel.slide.is_none() && app.explorer.root.is_some()
        });

        let width = app.ui.panel.width;
        let edge = Pos2::new(WINDOW.x - width, 300.0);
        frame(&mut app, &ctx, vec![egui::Event::PointerMoved(edge)]);
        frame(&mut app, &ctx, vec![button(edge, true)]);
        let dragged = edge - egui::vec2(80.0, 0.0);
        frame(&mut app, &ctx, vec![egui::Event::PointerMoved(dragged)]);
        frame(&mut app, &ctx, vec![button(dragged, false)]);
        assert_eq!(app.ui.panel.width, width + 80.0);
        // However far it is dragged, the terminals keep their room.
        let far = Pos2::new(40.0, 300.0);
        let edge = Pos2::new(WINDOW.x - app.ui.panel.width, 300.0);
        frame(&mut app, &ctx, vec![egui::Event::PointerMoved(edge)]);
        frame(&mut app, &ctx, vec![button(edge, true)]);
        frame(&mut app, &ctx, vec![egui::Event::PointerMoved(far)]);
        frame(&mut app, &ctx, vec![button(far, false)]);
        assert_eq!(app.ui.panel.width, *ui::panel::WIDTH.end());
    }
}
