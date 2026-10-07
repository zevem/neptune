//! The files of a project's context folder, `context` inside its own, as
//! the store's worker reads and writes them. Who may write what is decided
//! before a request reaches the worker; what a file may hold, and that
//! decisions and notes are only ever added to, is kept here as well, where
//! the one writer is.
use crate::projects::context::{
    self as rules, ARCHIVE, ARCHIVE_BEFORE, DECISIONS, DECISIONS_HEAD, Digest, FOLDER, FileInfo,
    INDEX, INSTRUCTIONS, MAX_ARCHIVE, MAX_DECISIONS, MAX_FILES, MAX_FOLDER, MAX_INSTRUCTIONS,
    MAX_NOTES, MAX_READ, MAX_STATUS, NOTES, Place, STATUS, STATUS_BEFORE,
};
use std::{
    io::{self, Read as _, Seek as _, Write as _},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Entries of a folder that are looked at before the rest is left unread.
const MAX_LISTED: usize = 4 * MAX_FILES;
/// How far past the clock an entry's time may be and still be taken as the
/// time it was recorded: entries of one second each get the next, and a
/// clock is set back now and then.
const AHEAD: u64 = 24 * 60 * 60;
const READ_ONLY: &str = "This project is read-only here, so nothing of its context is changed.";
const LINKED: &str = "The project's context folder, or its notes, is a link to somewhere else, \
                      which Neptune does not read or write through. Tell the user.";
const FULL: &str = "The project's context is full (4 MB or 64 files). Tell the user: they \
                    delete files in the Context part of the Project tab.";

/// One thing asked of a project's context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Nothing but what the folder holds now.
    Look,
    /// A file's text from `offset`, for a tool.
    Read { place: Place, offset: u64 },
    /// An entry for DECISIONS.md, as `rules::decision` wrote it.
    Record { entry: String },
    /// An entry for `notes/<topic>.md`, as `rules::note` wrote it.
    Note { topic: String, entry: String },
    /// STATUS.md, replaced or added to by the lead.
    Status { content: String, append: bool },
    /// INSTRUCTIONS.md as the user wrote it; empty removes it.
    Instructions { content: String },
    /// The user deletes a file.
    Delete { place: Place },
}
impl Op {
    fn writes(&self) -> bool {
        !matches!(self, Self::Look | Self::Read { .. })
    }
}

fn failed(error: &io::Error) -> String {
    format!(
        "Neptune could not write the project's context ({}).",
        error.kind()
    )
}
/// The file at `path`, where it is one: a link or a folder in its place is
/// not followed.
fn regular(path: &Path) -> Option<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(std::fs::Metadata::is_file)
}
/// Whether a link stands at `path`: what is behind it is not the project's,
/// so a folder that is one is neither listed nor written into.
fn linked(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|found| found.file_type().is_symlink())
}
/// At most `max` bytes of the file at `path`, from its start or its end.
fn bytes(path: &Path, max: u64, end: bool) -> Vec<u8> {
    let Some(found) = regular(path) else {
        return Vec::new();
    };
    let read = || -> io::Result<Vec<u8>> {
        let mut file = std::fs::File::open(path)?;
        if end && found.len() > max {
            file.seek(io::SeekFrom::Start(found.len() - max))?;
        }
        let mut bytes = Vec::new();
        file.take(max).read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    read().unwrap_or_default()
}
fn text(path: &Path, max: usize) -> String {
    String::from_utf8_lossy(&bytes(path, max as u64, false)).into_owned()
}
fn tail(path: &Path, max: usize) -> String {
    String::from_utf8_lossy(&bytes(path, max as u64, true)).into_owned()
}
fn modified(found: &std::fs::Metadata) -> Option<SystemTime> {
    found.modified().ok()
}

/// The files of the context folder `dir` that Neptune knows by name, in
/// the order of their names.
fn listed(dir: &Path) -> Vec<(Place, PathBuf)> {
    let mut found = Vec::new();
    let mut scan = |at: &Path, prefix: &str| {
        if linked(at) {
            return;
        }
        let Ok(entries) = std::fs::read_dir(at) else {
            return;
        };
        for entry in entries.flatten().take(MAX_LISTED) {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let name = format!("{prefix}{name}");
            // Only a file whose name is the one its place is written by.
            if let Ok(place) = Place::parse(&name)
                && place.name() == name
            {
                found.push((place, entry.path()));
            }
        }
    };
    scan(dir, "");
    scan(&dir.join(NOTES), "notes/");
    found.sort_by_key(|(place, _)| place.name());
    found.truncate(MAX_FILES);
    found
}

/// What the context folder of the project in `folder` holds now.
pub(super) fn look(folder: &Path) -> Digest {
    let dir = folder.join(FOLDER);
    if linked(&dir) {
        return Digest::default();
    }
    let files = listed(&dir)
        .into_iter()
        .filter_map(|(place, path)| {
            let found = regular(&path)?;
            Some(FileInfo {
                name: place.name(),
                heading: rules::heading(&text(&path, 2048)),
                size: found.len(),
                author: rules::author(&place, &tail(&path, 1024)),
                modified: modified(&found)
                    .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |since| since.as_secs()),
            })
        })
        .collect();
    let mut digest = Digest::new(
        &text(&dir.join(INSTRUCTIONS), MAX_INSTRUCTIONS),
        &text(&dir.join(STATUS), MAX_STATUS),
        &tail(&dir.join(DECISIONS), MAX_DECISIONS),
        files,
    );
    // A time that has not come is no record of Neptune's: taken for one,
    // nothing recorded after it would ever count as newer. Such an entry
    // is read as one written by hand.
    let soon = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(u64::MAX, |since| since.as_secs() + AHEAD);
    for decision in &mut digest.decisions {
        decision.at = decision.at.filter(|at| *at <= soon);
    }
    digest
}

/// How much the folder holds, in bytes and in files, whatever they are.
fn used(dir: &Path) -> (u64, usize) {
    let mut total = (0, 0);
    for at in [dir.to_path_buf(), dir.join(NOTES)] {
        if linked(&at) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(at) else {
            continue;
        };
        for entry in entries.flatten().take(MAX_LISTED) {
            if let Some(found) = regular(&entry.path()) {
                total.0 += found.len();
                total.1 += 1;
            }
        }
    }
    total
}
/// Whether `adding` more bytes, in a new file or not, still fit the folder.
fn room(dir: &Path, adding: usize, new: bool) -> Result<(), String> {
    let (bytes, files) = used(dir);
    if bytes + adding as u64 > MAX_FOLDER || files + usize::from(new) > MAX_FILES {
        Err(FULL.into())
    } else {
        Ok(())
    }
}

/// Writes a whole file in one step, so that it is never seen half written.
fn replace(path: &Path, text: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        super::ensure(parent)?;
    }
    crate::config::atomic_write(path, text.as_bytes())
        .map_err(|error| io::Error::other(error.to_string()))
}
/// Adds `entry` to the end of the file at `path` with one write. A new file
/// begins with `head`. What is there is never read back and written again.
fn append(path: &Path, head: &str, entry: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        super::ensure(parent)?;
    }
    // Only a file of the folder is added to: what a link there points at
    // is someone else's.
    if std::fs::symlink_metadata(path).is_ok_and(|found| !found.is_file()) {
        return Err(io::Error::other("not a file"));
    }
    let size = regular(path).map_or(0, |found| found.len());
    let mut out = String::new();
    if size == 0 {
        out.push_str(head);
    } else if bytes(path, 1, true) != b"\n" {
        // Its last line was left without an end, as by a hand edit.
        out.push('\n');
    }
    if !out.is_empty() || size > 0 {
        out.push('\n');
    }
    out.push_str(entry);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(out.as_bytes())?;
    file.sync_data()
}

fn record(dir: &Path, entry: &str) -> Result<String, String> {
    let path = dir.join(DECISIONS);
    if regular(&path).is_some_and(|found| found.len() > MAX_FOLDER) {
        // Not a file Neptune wrote; none of it is moved or cut.
        return Err(FULL.into());
    }
    let existing = text(&path, MAX_FOLDER as usize);
    // The oldest entries make room in the archive first: were this cut
    // short there, they would be in both files and lost from neither.
    if let Some((old, kept)) = rules::overflow(&existing, entry.len() + 1) {
        let archive = dir.join(ARCHIVE);
        let held = regular(&archive).map_or(0, |found| found.len());
        if held > 0 && held + old.len() as u64 > MAX_ARCHIVE as u64 {
            std::fs::rename(&archive, dir.join(ARCHIVE_BEFORE)).map_err(|error| failed(&error))?;
        }
        append(
            &archive,
            "# Decisions archive\n\nOlder decisions, moved here from DECISIONS.md as it filled.\n",
            &old,
        )
        .and_then(|()| {
            replace(
                &path,
                if kept.trim().is_empty() {
                    DECISIONS_HEAD
                } else {
                    &kept
                },
            )
        })
        .map_err(|error| failed(&error))?;
    }
    room(dir, entry.len() + 1, regular(&path).is_none())?;
    append(&path, DECISIONS_HEAD, entry).map_err(|error| failed(&error))?;
    Ok(format!("Recorded in {DECISIONS}."))
}

fn note(dir: &Path, topic: &str, entry: &str) -> Result<String, String> {
    let place = Place::Note(rules::slug(topic)?);
    let (name, path) = (place.name(), place.path(dir));
    let held = regular(&path).map_or(0, |found| found.len());
    if held + entry.len() as u64 + 1 > MAX_NOTES as u64 {
        return Err(format!(
            "{name} is full ({} KB). Add to another topic, such as {topic}-2.",
            MAX_NOTES / 1024
        ));
    }
    room(dir, entry.len() + 1, held == 0)?;
    append(&path, &rules::notes_head(topic), entry).map_err(|error| failed(&error))?;
    Ok(format!("Added to {name}."))
}

fn status(dir: &Path, content: &str, add: bool, now: SystemTime) -> Result<String, String> {
    let path = dir.join(STATUS);
    let content = content.trim();
    let found = regular(&path);
    let held = found.as_ref().map_or(0, |found| found.len());
    if add {
        if held + content.len() as u64 + 2 > MAX_STATUS as u64 {
            return Err(format!(
                "{STATUS} would pass {} KB. Replace it with a shorter one instead.",
                MAX_STATUS / 1024
            ));
        }
        room(dir, content.len() + 2, held == 0)?;
        append(&path, "", content).map_err(|error| failed(&error))?;
        return Ok(format!("Added to {STATUS}."));
    }
    if content.len() + 1 > MAX_STATUS {
        return Err(format!(
            "{STATUS} holds at most {} KB. Keep it to the goal, what is in progress, what is \
             next, what is done and links.",
            MAX_STATUS / 1024
        ));
    }
    // A replacement that follows another closely corrects it: the one kept
    // beside it stays the last that stood for a while.
    let settled = found
        .as_ref()
        .and_then(modified)
        .is_none_or(|at| rules::status_settled(now.duration_since(at).unwrap_or_default()));
    let before = dir.join(STATUS_BEFORE);
    let first = regular(&before).is_none();
    let keep = found.is_some() && (settled || first);
    room(dir, content.len() + 1, found.is_none() || (keep && first))?;
    if keep {
        // The one before it stays beside it.
        replace(&before, &text(&path, 8 * MAX_STATUS)).map_err(|error| failed(&error))?;
    }
    replace(&path, &format!("{content}\n")).map_err(|error| failed(&error))?;
    Ok(if keep {
        format!("Replaced {STATUS}. The one before it is kept as {STATUS_BEFORE}.")
    } else if found.is_some() {
        format!("Replaced {STATUS}.")
    } else {
        format!("Wrote {STATUS}.")
    })
}

fn instructions(dir: &Path, content: &str) -> Result<String, String> {
    let path = dir.join(INSTRUCTIONS);
    let content = content.trim();
    if content.len() + 1 > MAX_INSTRUCTIONS {
        return Err(format!(
            "The instructions hold at most {} KB.",
            MAX_INSTRUCTIONS / 1024
        ));
    }
    if content.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(failed(&error)),
            _ => Ok("Cleared.".into()),
        };
    }
    room(dir, content.len() + 1, regular(&path).is_none())?;
    replace(&path, &format!("{content}\n")).map_err(|error| failed(&error))?;
    Ok("Saved.".into())
}

fn read(folder: &Path, place: &Place, offset: u64) -> Result<String, String> {
    if *place == Place::Index {
        // Always what is there now, whatever the file says.
        let index = rules::index(&look(folder).files);
        let from = usize::try_from(offset).map_or(index.len(), |from| from.min(index.len()));
        return Ok(rules::window(
            &index.as_bytes()[from..],
            offset,
            index.len() as u64,
        ));
    }
    let path = place.path(&folder.join(FOLDER));
    let name = place.name();
    let missing = || {
        format!("There is no {name} yet. read_context without a path lists what the project keeps.")
    };
    let found = regular(&path).ok_or_else(missing)?;
    let read = || -> io::Result<Vec<u8>> {
        let mut file = std::fs::File::open(&path)?;
        file.seek(io::SeekFrom::Start(offset.min(found.len())))?;
        let mut bytes = Vec::new();
        file.take(MAX_READ as u64 + 4).read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    let bytes =
        read().map_err(|error| format!("Neptune could not read {name} ({}).", error.kind()))?;
    Ok(rules::window(&bytes, offset, found.len()))
}

fn delete(dir: &Path, place: &Place) -> Result<String, String> {
    let path = place.path(dir);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok("Deleted.".into()),
        Err(error) => Err(failed(&error)),
        Ok(found) if found.is_dir() => Err(format!("{} is a folder.", place.name())),
        // A link in its place is removed, never followed.
        Ok(_) => std::fs::remove_file(&path)
            .map(|()| "Deleted.".into())
            .map_err(|error| failed(&error)),
    }
}

/// Writes INDEX.md where it no longer says what the folder holds. Returns
/// whether it was written.
fn index(dir: &Path, digest: &Digest) -> bool {
    let path = dir.join(INDEX);
    let there = regular(&path).is_some();
    // A folder with nothing in it gets no index of nothing.
    if !there && digest.files.is_empty() {
        return false;
    }
    let wanted = rules::index(&digest.files);
    if there && text(&path, wanted.len() + 1) == wanted {
        return false;
    }
    replace(&path, &wanted).is_ok()
}

/// Carries out `op` for the project in `folder`, and says what its context
/// holds afterwards. A project that is not `writable` is only read.
pub(super) fn carry(folder: &Path, writable: bool, op: Op) -> (Result<String, String>, Digest) {
    let dir = folder.join(FOLDER);
    // A link where the folder or its notes should be leads out of the
    // project: nothing is read or written through it.
    let notes = match &op {
        Op::Note { .. } => true,
        Op::Read { place, .. } | Op::Delete { place } => matches!(place, Place::Note(_)),
        _ => false,
    };
    let astray = linked(&dir) || (notes && linked(&dir.join(NOTES)));
    let result = if op.writes() && !writable {
        Err(READ_ONLY.into())
    } else if astray && op != Op::Look {
        Err(LINKED.into())
    } else {
        match &op {
            Op::Look => Ok(String::new()),
            Op::Read { place, offset } => read(folder, place, *offset),
            Op::Record { entry } => record(&dir, entry),
            Op::Note { topic, entry } => note(&dir, topic, entry),
            Op::Status { content, append } => status(&dir, content, *append, SystemTime::now()),
            Op::Instructions { content } => instructions(&dir, content),
            Op::Delete { place } => delete(&dir, place),
        }
    };
    let mut digest = look(folder);
    if writable && !linked(&dir) && index(&dir, &digest) {
        digest = look(folder);
    }
    (result, digest)
}
