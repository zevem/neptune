//! What `git` says about a folder: its branch, the files that changed and how
//! one of them differs. Everything here blocks on a child process, so it runs
//! on the changes worker, never in a frame. Output is read up to a limit and
//! the rest is left unread.
use crate::ui::changes::{DiffBody, File, Line, LineKind, Scope, Status};
use std::{
    io::Read as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// The most of a status or file list that is read.
const LIST_BYTES: usize = 4 * 1024 * 1024;
/// The most files one list shows; the rest are counted.
pub(super) const MAX_FILES: usize = 2_000;
const DIFF_BYTES: usize = 512 * 1024;
const DIFF_LINES: usize = 10_000;
/// The tree of a commit without files, which a repository without commits is
/// compared with.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// A folder's branch and whether anything in it differs from its last commit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Summary {
    /// The branch, or the short name of the commit checked out without one.
    pub branch: String,
    pub detached: bool,
    /// Commits the branch has that its upstream lacks, and the reverse.
    pub ahead: u32,
    pub behind: u32,
    pub dirty: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Listing {
    pub root: PathBuf,
    pub summary: Summary,
    /// What the branch is compared with, when it is.
    pub base: Option<String>,
    /// The branch was to be compared with the main one, and there is none.
    pub no_base: bool,
    pub files: Vec<File>,
    /// Files beyond the limit.
    pub more: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Failure {
    /// `git` could not be started.
    Missing,
    NotRepository,
    Other(String),
}

struct Output {
    ok: bool,
    stdout: Vec<u8>,
    truncated: bool,
}

/// Runs `git` in `dir` without a terminal, a prompt or a lock on the index,
/// reading at most `limit` bytes of what it prints.
fn git(dir: &Path, args: &[&str], limit: usize) -> std::io::Result<Output> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        // A repository's own configuration must not start programs for a
        // status nobody asked for at a prompt.
        .args(["-c", "core.fsmonitor=false", "-c", "core.quotepath=false"])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn()?;
    let mut stdout = Vec::new();
    let read = child
        .stdout
        .take()
        .map(|pipe| pipe.take(limit as u64 + 1).read_to_end(&mut stdout));
    let truncated = stdout.len() > limit;
    if truncated {
        stdout.truncate(limit);
        let _ = child.kill();
    }
    let status = child.wait()?;
    if let Some(Err(error)) = read {
        return Err(error);
    }
    Ok(Output {
        ok: status.success() || truncated,
        stdout,
        truncated,
    })
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_owned()
}

/// The records of `-z` output, without the empty one after the last.
fn records(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
}

struct StatusReport {
    summary: Summary,
    /// The branch has no commit yet.
    initial: bool,
    untracked: Vec<String>,
}

/// Reads `git status --porcelain=v2 --branch -z`.
fn parse_status(bytes: &[u8]) -> StatusReport {
    let mut summary = Summary::default();
    let mut initial = false;
    let mut oid = String::new();
    let mut untracked = Vec::new();
    let mut records = records(bytes);
    while let Some(record) = records.next() {
        let record = String::from_utf8_lossy(record);
        if let Some(header) = record.strip_prefix("# ") {
            if let Some(head) = header.strip_prefix("branch.head ") {
                summary.detached = head == "(detached)";
                summary.branch = head.to_owned();
            } else if let Some(value) = header.strip_prefix("branch.oid ") {
                initial = value == "(initial)";
                oid = value.to_owned();
            } else if let Some(counts) = header.strip_prefix("branch.ab ") {
                let mut counts = counts.split(' ');
                let count =
                    |part: Option<&str>| part.and_then(|part| part[1..].parse().ok()).unwrap_or(0);
                summary.ahead = count(counts.next());
                summary.behind = count(counts.next());
            }
            continue;
        }
        match record.as_bytes().first() {
            // An ignored file is not a change.
            Some(b'!') => continue,
            Some(b'?') => untracked.push(record[2..].to_owned()),
            // A rename is followed by the path it had.
            Some(b'2') => {
                records.next();
            }
            _ => {}
        }
        summary.dirty = true;
    }
    if summary.detached {
        summary.branch = oid.chars().take(7).collect();
    }
    StatusReport {
        summary,
        initial,
        untracked,
    }
}

fn status_of(code: &str) -> Status {
    match code.as_bytes().first() {
        Some(b'A' | b'C') => Status::Added,
        Some(b'D') => Status::Deleted,
        Some(b'R') => Status::Renamed,
        Some(b'U') => Status::Conflicted,
        _ => Status::Modified,
    }
}

/// Reads `git diff --name-status -z`: a status, then one path or, for a
/// rename or a copy, the path it had and the one it has.
fn parse_names(bytes: &[u8]) -> Vec<File> {
    let mut files = Vec::new();
    let mut records = records(bytes).map(|record| String::from_utf8_lossy(record).into_owned());
    while let Some(code) = records.next() {
        let status = status_of(&code);
        let Some(first) = records.next() else { break };
        let (path, from) = if code.starts_with(['R', 'C']) {
            let Some(path) = records.next() else { break };
            // A copy's source is still there, so only a rename names it.
            (path, (status == Status::Renamed).then_some(first))
        } else {
            (first, None)
        };
        files.push(File {
            path,
            from,
            status,
            added: None,
            removed: None,
        });
    }
    files
}

/// Reads `git diff --numstat -z` into the lines added to and removed from
/// each path. A file that is not text has no counts.
fn parse_counts(bytes: &[u8]) -> Vec<(String, u32, u32)> {
    let mut counts = Vec::new();
    let mut records = records(bytes).map(|record| String::from_utf8_lossy(record).into_owned());
    while let Some(record) = records.next() {
        let mut parts = record.splitn(3, '\t');
        let (added, removed) = (parts.next(), parts.next());
        let path = match parts.next() {
            // A rename's paths follow as records of their own.
            Some("") | None => {
                records.next();
                let Some(path) = records.next() else { break };
                path
            }
            Some(path) => path.to_owned(),
        };
        if let (Some(Ok(added)), Some(Ok(removed))) =
            (added.map(str::parse::<u32>), removed.map(str::parse::<u32>))
        {
            counts.push((path, added, removed));
        }
    }
    counts
}

/// Turns a unified diff of one file into lines to paint.
pub(super) fn parse_diff(bytes: &[u8]) -> DiffBody {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = Vec::new();
    let mut widest = 0;
    let mut truncated = false;
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    let mut all = text.split('\n').peekable();
    while let Some(raw) = all.next() {
        if !in_hunk && !raw.starts_with("@@") {
            if raw.starts_with("Binary files ") || raw.starts_with("GIT binary patch") {
                return DiffBody::Binary;
            }
            continue;
        }
        // What follows the last line break is not a line.
        if raw.is_empty() && all.peek().is_none() {
            break;
        }
        if lines.len() >= DIFF_LINES {
            truncated = true;
            break;
        }
        let (kind, shown) = if let Some(header) = raw.strip_prefix("@@") {
            in_hunk = true;
            let mut ranges = header.split(' ').filter(|part| !part.is_empty());
            let start = |range: Option<&str>| {
                range
                    .and_then(|range| range[1..].split(',').next())
                    .and_then(|start| start.parse().ok())
                    .unwrap_or(0)
            };
            old = start(ranges.next());
            new = start(ranges.next());
            (LineKind::Hunk, raw)
        } else if let Some(rest) = raw.strip_prefix('+') {
            (LineKind::Added, rest)
        } else if let Some(rest) = raw.strip_prefix('-') {
            (LineKind::Removed, rest)
        } else if raw.starts_with('\\') {
            // "\ No newline at end of file" is about the line before it.
            continue;
        } else if raw.starts_with("diff --git ") {
            // One file is asked for; anything further is not its diff.
            break;
        } else {
            (LineKind::Context, raw.strip_prefix(' ').unwrap_or(raw))
        };
        let (text, columns) = super::explorer::display_line(shown);
        widest = widest.max(columns);
        let number = |on: bool, counter: &mut u32| {
            on.then(|| {
                let number = *counter;
                *counter += 1;
                number
            })
        };
        lines.push(Line {
            kind,
            old: number(
                matches!(kind, LineKind::Removed | LineKind::Context),
                &mut old,
            ),
            new: number(
                matches!(kind, LineKind::Added | LineKind::Context),
                &mut new,
            ),
            text,
        });
    }
    if lines.is_empty() {
        return DiffBody::Same;
    }
    DiffBody::Lines {
        lines,
        widest,
        truncated,
    }
}

/// A file git does not track, read as lines that were all added.
fn untracked_diff(path: &Path) -> DiffBody {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => return DiffBody::Failed(error.kind().to_string()),
    };
    // Reading a pipe or a device could wait forever.
    if !metadata.is_file() {
        return DiffBody::Note("Not a regular file");
    }
    let mut bytes = Vec::new();
    let read = std::fs::File::open(path)
        .and_then(|file| file.take(DIFF_BYTES as u64 + 1).read_to_end(&mut bytes));
    if let Err(error) = read {
        return DiffBody::Failed(error.kind().to_string());
    }
    if bytes.iter().take(8 * 1024).any(|byte| *byte == 0) {
        return DiffBody::Binary;
    }
    let mut truncated = bytes.len() > DIFF_BYTES;
    bytes.truncate(DIFF_BYTES);
    let text = String::from_utf8_lossy(&bytes);
    let mut raw: Vec<&str> = text.split('\n').collect();
    // The last piece is what follows the final line break, or a line the
    // limit cut short.
    if raw.len() > 1 && (truncated || raw.last() == Some(&"")) {
        raw.pop();
    }
    if raw.len() > DIFF_LINES {
        raw.truncate(DIFF_LINES);
        truncated = true;
    }
    if raw == [""] {
        return DiffBody::Note("Empty file");
    }
    let mut widest = 0;
    let lines = raw
        .into_iter()
        .zip(1..)
        .map(|(line, number)| {
            let (text, columns) = super::explorer::display_line(line);
            widest = widest.max(columns);
            Line {
                kind: LineKind::Added,
                old: None,
                new: Some(number),
                text,
            }
        })
        .collect();
    DiffBody::Lines {
        lines,
        widest,
        truncated,
    }
}

/// The top folder of the repository `dir` is in.
fn root(dir: &Path) -> Result<PathBuf, Failure> {
    let output =
        git(dir, &["rev-parse", "--show-toplevel"], 16 * 1024).map_err(|_| Failure::Missing)?;
    let root = text(&output.stdout);
    if !output.ok || root.is_empty() {
        return Err(Failure::NotRepository);
    }
    Ok(root.into())
}

/// The branch and dirty state of `dir`, or `None` outside a repository.
pub(super) fn summary(dir: &Path) -> Option<Summary> {
    let output = git(
        dir,
        &["status", "--porcelain=v2", "--branch", "-z"],
        LIST_BYTES,
    )
    .ok()?;
    output.ok.then(|| parse_status(&output.stdout).summary)
}

/// The branch this repository's work is merged into, as git names it.
fn main_branch(root: &Path) -> Option<String> {
    let run = |args: &[&str]| git(root, args, 16 * 1024).ok().filter(|output| output.ok);
    if let Some(output) = run(&[
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ]) {
        let name = text(&output.stdout);
        if !name.is_empty() {
            return Some(name);
        }
    }
    ["origin/main", "origin/master", "main", "master"]
        .into_iter()
        .find(|name| {
            run(&[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{name}^{{commit}}"),
            ])
            .is_some()
        })
        .map(str::to_owned)
}

/// The commit `scope` compares the folder with, and the name of the branch
/// it stands for. `None` when there is no main branch to compare with.
fn base(root: &Path, scope: Scope, initial: bool) -> Option<(String, Option<String>)> {
    match scope {
        Scope::WorkingTree if initial => Some((EMPTY_TREE.into(), None)),
        Scope::WorkingTree => Some(("HEAD".into(), None)),
        Scope::Branch => {
            let name = main_branch(root)?;
            let output = git(root, &["merge-base", "HEAD", &name], 16 * 1024).ok()?;
            let commit = text(&output.stdout);
            (output.ok && !commit.is_empty()).then_some((commit, Some(name)))
        }
    }
}

/// The files of the repository holding `dir` that differ in `scope`.
pub(super) fn listing(dir: &Path, scope: Scope) -> Result<Listing, Failure> {
    let root = root(dir)?;
    let failed = |_| Failure::Other("Git could not be run".into());
    let status = git(
        &root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
        LIST_BYTES,
    )
    .map_err(failed)?;
    if !status.ok {
        return Err(Failure::Other("Git could not read this repository".into()));
    }
    let report = parse_status(&status.stdout);
    let Some((commit, base)) = base(&root, scope, report.initial) else {
        return Ok(Listing {
            root,
            summary: report.summary,
            base: None,
            no_base: true,
            files: Vec::new(),
            more: 0,
        });
    };
    let names = git(
        &root,
        &["diff", "--name-status", "-z", "-M", &commit, "--"],
        LIST_BYTES,
    )
    .map_err(failed)?;
    if !names.ok {
        return Err(Failure::Other("Git could not compare this folder".into()));
    }
    let mut files = parse_names(&names.stdout);
    let counts = git(
        &root,
        &["diff", "--numstat", "-z", "-M", &commit, "--"],
        LIST_BYTES,
    )
    .map_err(failed)?;
    let counts: std::collections::HashMap<_, _> = parse_counts(&counts.stdout)
        .into_iter()
        .map(|(path, added, removed)| (path, (added, removed)))
        .collect();
    for file in &mut files {
        if let Some((added, removed)) = counts.get(&file.path) {
            file.added = Some(*added);
            file.removed = Some(*removed);
        }
    }
    files.extend(report.untracked.into_iter().map(|path| File {
        path,
        from: None,
        status: Status::Untracked,
        added: None,
        removed: None,
    }));
    files.sort_by(|a, b| a.path.cmp(&b.path));
    // A list cut short at the limit has more files than were read.
    let more =
        files.len().saturating_sub(MAX_FILES) + usize::from(status.truncated || names.truncated);
    files.truncate(MAX_FILES);
    Ok(Listing {
        root,
        summary: report.summary,
        base,
        no_base: false,
        files,
        more,
    })
}

/// How `file` of the repository at `root` differs in `scope`.
pub(super) fn diff(root: &Path, scope: Scope, file: &File) -> DiffBody {
    if file.status == Status::Untracked {
        return untracked_diff(&root.join(&file.path));
    }
    let initial = git(root, &["rev-parse", "--verify", "--quiet", "HEAD"], 1024)
        .is_ok_and(|output| !output.ok);
    let Some((commit, _)) = base(root, scope, initial) else {
        return DiffBody::Failed("There is nothing to compare this file with".into());
    };
    let mut args = vec![
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "-M",
        &commit,
        "--",
    ];
    args.extend(file.from.as_deref());
    args.push(&file.path);
    match git(root, &args, DIFF_BYTES) {
        Ok(output) if output.ok => match parse_diff(&output.stdout) {
            DiffBody::Lines { lines, widest, .. } if output.truncated => DiffBody::Lines {
                lines,
                widest,
                truncated: true,
            },
            body => body,
        },
        _ => DiffBody::Failed("Git could not compare this file".into()),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// Runs `git` for a test's own repository, as a person without settings.
    pub(in crate::app) fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git is installed");
        assert!(status.success(), "git {args:?}");
    }

    /// A repository on `main` with one commit: `kept.txt`, `gone.txt`,
    /// `old.txt` and `src/lib.rs`.
    pub(in crate::app) fn repository(dir: &Path) {
        run(dir, &["init", "--quiet"]);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("kept.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(dir.join("gone.txt"), "bye\n").unwrap();
        std::fs::write(dir.join("old.txt"), "a\nb\nc\nd\ne\nf\n").unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
        run(dir, &["add", "."]);
        run(dir, &["commit", "--quiet", "-m", "first"]);
    }

    fn listed(listing: &Listing) -> Vec<(&str, Status, Option<u32>, Option<u32>)> {
        listing
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.status, file.added, file.removed))
            .collect()
    }

    #[test]
    fn status_reports_the_branch_its_upstream_and_whether_anything_changed() {
        let clean = parse_status(
            b"# branch.oid 1234567890abcdef\0# branch.head feat/x\0# branch.upstream origin/feat/x\0# branch.ab +2 -1\0",
        );
        assert_eq!(
            clean.summary,
            Summary {
                branch: "feat/x".into(),
                detached: false,
                ahead: 2,
                behind: 1,
                dirty: false,
            }
        );
        assert!(!clean.initial && clean.untracked.is_empty());
        // A rename carries its old path as a record of its own, which is not
        // an entry; an ignored file is not a change.
        let dirty = parse_status(
            b"# branch.oid abcdef1234567\0# branch.head (detached)\x002 R. N... 100644 100644 100644 a b R100 new name.txt\0? old.txt\0? notes/new file.md\0! target/\0",
        );
        assert_eq!(dirty.summary.branch, "abcdef1");
        assert!(dirty.summary.detached && dirty.summary.dirty);
        assert_eq!(dirty.untracked, ["notes/new file.md"]);
        let ignored = parse_status(b"# branch.oid (initial)\0# branch.head main\0! target/\0");
        assert!(ignored.initial && !ignored.summary.dirty);
    }

    #[test]
    fn names_and_counts_are_read_with_renames_and_files_that_are_not_text() {
        let files =
            parse_names(b"M\0src/a b.rs\0R087\0old.txt\0new.txt\0D\0gone\0C50\0a\0copy\0U\0both\0");
        let seen: Vec<_> = files
            .iter()
            .map(|file| (file.path.as_str(), file.from.as_deref(), file.status))
            .collect();
        assert_eq!(
            seen,
            [
                ("src/a b.rs", None, Status::Modified),
                ("new.txt", Some("old.txt"), Status::Renamed),
                ("gone", None, Status::Deleted),
                ("copy", None, Status::Added),
                ("both", None, Status::Conflicted),
            ]
        );
        assert_eq!(
            parse_counts(b"3\t1\tsrc/a b.rs\x000\t2\t\0old.txt\0new.txt\0-\t-\tlogo.png\0"),
            [("src/a b.rs".into(), 3, 1), ("new.txt".into(), 0, 2)]
        );
    }

    #[test]
    fn a_diff_becomes_numbered_lines_without_its_headers() {
        let diff = b"diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@ fn main\n one\n-two\n+2\n three\n\\ No newline at end of file\n@@ -10 +10,2 @@\n+\tnew\n ten\n";
        let DiffBody::Lines {
            lines,
            widest,
            truncated,
        } = parse_diff(diff)
        else {
            panic!("lines")
        };
        assert!(!truncated);
        let seen: Vec<_> = lines
            .iter()
            .map(|line| (line.kind, line.old, line.new, line.text.as_str()))
            .collect();
        assert_eq!(
            seen,
            [
                (LineKind::Hunk, None, None, "@@ -1,3 +1,3 @@ fn main"),
                (LineKind::Context, Some(1), Some(1), "one"),
                (LineKind::Removed, Some(2), None, "two"),
                (LineKind::Added, None, Some(2), "2"),
                (LineKind::Context, Some(3), Some(3), "three"),
                (LineKind::Hunk, None, None, "@@ -10 +10,2 @@"),
                (LineKind::Added, None, Some(10), "    new"),
                (LineKind::Context, Some(10), Some(11), "ten"),
            ]
        );
        assert_eq!(widest, "@@ -1,3 +1,3 @@ fn main".len());
        assert_eq!(
            parse_diff(b"diff --git a/p b/p\nBinary files a/p and b/p differ\n"),
            DiffBody::Binary
        );
        // A change of mode alone has no lines.
        assert_eq!(
            parse_diff(b"diff --git a/f b/f\nold mode 100644\nnew mode 100755\n"),
            DiffBody::Same
        );
    }

    #[test]
    fn a_folder_outside_a_repository_has_no_branch_and_no_list() {
        let dir = tempfile::tempdir().unwrap();
        // A parent of the temporary folder could be a repository of its own.
        if root(dir.path()).is_ok() {
            return;
        }
        assert_eq!(summary(dir.path()), None);
        assert_eq!(
            listing(dir.path(), Scope::WorkingTree),
            Err(Failure::NotRepository)
        );
    }

    #[test]
    fn the_working_tree_lists_what_differs_from_the_last_commit() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        repository(dir);
        assert_eq!(
            summary(dir),
            Some(Summary {
                branch: "main".into(),
                ..Summary::default()
            })
        );
        let clean = listing(&dir.join("src"), Scope::WorkingTree).unwrap();
        assert!(clean.files.is_empty() && clean.base.is_none());
        assert_eq!(
            std::fs::canonicalize(&clean.root).unwrap(),
            std::fs::canonicalize(dir).unwrap()
        );

        std::fs::write(dir.join("kept.txt"), "one\n2\nthree\nfour\n").unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        run(dir, &["mv", "old.txt", "new.txt"]);
        std::fs::create_dir_all(dir.join("notes")).unwrap();
        std::fs::write(dir.join("notes/todo.md"), "- first\n- second\n").unwrap();
        std::fs::write(dir.join("blob.bin"), [1u8, 0, 2, 0]).unwrap();
        // Staged and unstaged work are one change to review.
        run(dir, &["add", "kept.txt"]);

        assert!(summary(dir).unwrap().dirty);
        // Asked from a folder inside it, the whole repository is listed.
        let listing = listing(&dir.join("src"), Scope::WorkingTree).unwrap();
        assert_eq!(
            listed(&listing),
            [
                ("blob.bin", Status::Untracked, None, None),
                ("gone.txt", Status::Deleted, Some(0), Some(1)),
                ("kept.txt", Status::Modified, Some(2), Some(1)),
                ("new.txt", Status::Renamed, Some(0), Some(0)),
                ("notes/todo.md", Status::Untracked, None, None),
            ]
        );
        let file = |path: &str| listing.files.iter().find(|file| file.path == path).unwrap();
        assert_eq!(file("new.txt").from.as_deref(), Some("old.txt"));
        let lines = |body: DiffBody| match body {
            DiffBody::Lines { lines, .. } => lines
                .into_iter()
                .map(|line| (line.kind, line.text))
                .collect::<Vec<_>>(),
            other => panic!("lines, not {other:?}"),
        };
        let kept = lines(diff(&listing.root, Scope::WorkingTree, file("kept.txt")));
        assert_eq!(
            kept[1..],
            [
                (LineKind::Context, "one".to_owned()),
                (LineKind::Removed, "two".to_owned()),
                (LineKind::Added, "2".to_owned()),
                (LineKind::Context, "three".to_owned()),
                (LineKind::Added, "four".to_owned()),
            ]
        );
        assert_eq!(
            lines(diff(
                &listing.root,
                Scope::WorkingTree,
                file("notes/todo.md")
            )),
            [
                (LineKind::Added, "- first".to_owned()),
                (LineKind::Added, "- second".to_owned()),
            ]
        );
        assert_eq!(
            diff(&listing.root, Scope::WorkingTree, file("blob.bin")),
            DiffBody::Binary
        );
        assert_eq!(
            diff(&listing.root, Scope::WorkingTree, file("new.txt")),
            DiffBody::Same
        );
    }

    #[test]
    fn the_branch_lists_everything_since_it_left_main_committed_or_not() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        repository(dir);
        run(dir, &["checkout", "--quiet", "-b", "feat/review"]);
        std::fs::write(dir.join("kept.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        run(dir, &["commit", "--quiet", "-am", "second"]);
        // Main moves on; the branch is compared with where it left.
        run(dir, &["checkout", "--quiet", "main"]);
        std::fs::write(dir.join("gone.txt"), "still here\n").unwrap();
        run(dir, &["commit", "--quiet", "-am", "elsewhere"]);
        run(dir, &["checkout", "--quiet", "feat/review"]);
        std::fs::write(dir.join("src/lib.rs"), "pub fn g() {}\n").unwrap();

        let tree = listing(dir, Scope::WorkingTree).unwrap();
        assert_eq!(tree.summary.branch, "feat/review");
        assert_eq!(
            listed(&tree),
            [("src/lib.rs", Status::Modified, Some(1), Some(1))]
        );
        let branch = listing(dir, Scope::Branch).unwrap();
        assert_eq!(branch.base.as_deref(), Some("main"));
        assert_eq!(
            listed(&branch),
            [
                ("kept.txt", Status::Modified, Some(1), Some(0)),
                ("src/lib.rs", Status::Modified, Some(1), Some(1)),
            ]
        );
        assert!(matches!(
            diff(&branch.root, Scope::Branch, &branch.files[0]),
            DiffBody::Lines { .. }
        ));
    }

    #[test]
    fn a_repository_without_a_commit_or_a_main_branch_still_answers() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        run(dir, &["init", "--quiet", "--initial-branch", "trunk"]);
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        run(dir, &["add", "a.txt"]);
        let listing = listing(dir, Scope::WorkingTree).unwrap();
        assert_eq!(listing.summary.branch, "trunk");
        assert_eq!(
            listed(&listing),
            [
                ("a.txt", Status::Added, Some(1), Some(0)),
                ("b.txt", Status::Untracked, None, None),
            ]
        );
        assert!(matches!(
            diff(&listing.root, Scope::WorkingTree, &listing.files[0]),
            DiffBody::Lines { .. }
        ));
        let branch = super::listing(dir, Scope::Branch).unwrap();
        assert!(branch.no_base && branch.files.is_empty());
        assert_eq!(branch.summary.branch, "trunk");
    }
}
