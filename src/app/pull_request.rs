//! The pull request the panel's tab shows. A worker reads it through the
//! person's GitHub CLI when it is opened and again as it ages, only while
//! the tab is in view, and reports only what changed. It also does what the
//! person asks of the pull request there. Nothing read is saved, and none of
//! it enters diagnostics.
use super::*;
use crate::runtime::pull_request::{
    self as source, Act, Change, ChangedFile, Choices, Detail, Failure,
};
use crate::runtime::pull_requests::Checks;
use crate::ui::changes::{DiffBody, File, Status};
use crate::ui::pull_request::{Body, Event, FileList, Problem, Shown};
use neptune_model::PullRequest;
use std::sync::atomic::{AtomicU64, Ordering};

/// A slow host is asked less often: the GitHub CLI gets at most this share
/// of the worker's time.
const WORK_SHARE: u32 = 10;
/// A window in the background is read this many times less often.
const BACKGROUND: u32 = 5;

/// How long what was read stands before it is read again.
fn refresh_after(read: &Result<Detail, Failure>) -> Duration {
    Duration::from_secs(match read {
        Ok(detail) if !detail.in_review() => 10 * 60,
        Ok(detail) if detail.checks() == Checks::Pending => 15,
        _ => 60,
    })
}

/// What the tab shows this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Watch {
    link: PullRequest,
    /// Its changed files are in view as well.
    files: bool,
    /// The commit whose files are shown, in place of all it changes.
    commit: Option<String>,
    /// Changes that are only white space are left out of the diffs.
    plain: bool,
    /// Another window is the active one: it is read less often.
    background: bool,
    /// Which reading of it and of its files the tab waits for. Each counts
    /// up when the tab lets go of what it had, so that the worker reports
    /// again what it has already read once.
    detail_turn: u64,
    files_turn: u64,
}

enum Request {
    /// Nothing while the tab is out of view or the window is minimized.
    Watch(Option<Watch>),
    /// Read what is watched again now, and report it whether it changed.
    Refresh,
    /// What is asked of a pull request, with the host's name for it.
    Act(PullRequest, String, Act),
    /// Read what the pull request's repository offers it.
    Choices(PullRequest),
}

enum Reply {
    Detail {
        link: PullRequest,
        read: Result<Box<Detail>, Failure>,
    },
    Files {
        link: PullRequest,
        /// The reading of its files this answers.
        turn: u64,
        read: Result<Files, Failure>,
    },
    Done {
        link: PullRequest,
        act: Act,
        result: Result<(), String>,
    },
    Choices {
        link: PullRequest,
        read: Result<Choices, Failure>,
    },
}

/// The files a pull request changes, each with its diff ready to paint.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Files {
    files: Vec<File>,
    /// The diff of each file, in their order.
    diffs: Vec<DiffBody>,
    /// Files beyond those read.
    more: usize,
}

type Read<T> = Box<dyn Fn(&PullRequest) -> Result<T, Failure> + Send>;
type ReadFiles =
    Box<dyn Fn(&PullRequest, Option<&str>) -> Result<Vec<ChangedFile>, Failure> + Send>;
type Do = Box<dyn Fn(&PullRequest, &str, &Act) -> Result<(), String> + Send>;

/// Where a pull request is read and changed: the GitHub CLI, or a stand-in.
pub(super) struct Source {
    pub detail: Read<Detail>,
    pub files: ReadFiles,
    pub choices: Read<Choices>,
    pub act: Do,
}
impl Default for Source {
    fn default() -> Self {
        // Tests of the application do not run the person's GitHub CLI.
        if cfg!(test) {
            Self {
                detail: Box::new(|_| Err(Failure::Unavailable)),
                files: Box::new(|_, _| Err(Failure::Unavailable)),
                choices: Box::new(|_| Err(Failure::Unavailable)),
                act: Box::new(|_, _, _| Err(String::new())),
            }
        } else {
            Self {
                detail: Box::new(source::read),
                files: Box::new(source::read_files),
                choices: Box::new(source::read_choices),
                act: Box::new(source::act),
            }
        }
    }
}

/// The most pictures shown of one pull request, the most one may weigh and
/// the most points a side of one keeps.
const MAX_PICTURES: usize = 16;
const PICTURE_BYTES: u64 = 8 * 1024 * 1024;
const PICTURE_PIXELS: u32 = 1_024;

type Decoded = ([u32; 2], egui::ColorImage);
type Fetch = Box<dyn Fn(&PullRequest, &str) -> Option<Decoded> + Send>;
/// A picture as its worker reports it: the pull request shown when it was
/// asked for, where it comes from and what was read of it.
type ReadPicture = (u64, String, Option<Decoded>);

/// Whether a picture written in a pull request is fetched: one its host
/// keeps, over an encrypted connection. A picture kept elsewhere would tell
/// whoever keeps it that the person read this; it opens in the browser.
fn fetched(link: &PullRequest, source: &str) -> bool {
    let Some(rest) = source.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let (of, _, _) = link.location();
    host.eq_ignore_ascii_case(of)
        || (of.eq_ignore_ascii_case("github.com")
            && host
                .to_ascii_lowercase()
                .ends_with(".githubusercontent.com"))
}

/// Reads a picture without the person's account, as a public repository
/// serves it, then through their GitHub CLI, as a private one does.
fn fetch_picture(link: &PullRequest, source: &str) -> Option<Decoded> {
    let limit = [PICTURE_PIXELS, PICTURE_PIXELS];
    let open = || {
        let address = crate::platform::links::WebLink::new(source)?;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .max_redirects(3)
            .build()
            .into();
        let bytes = agent
            .get(address.as_str())
            .call()
            .ok()?
            .body_mut()
            .with_config()
            .limit(PICTURE_BYTES)
            .read_to_vec()
            .ok()?;
        image_preview::decode_bytes(&bytes, limit)
    };
    open().or_else(|| {
        // Only what was attached on the pull request's own host is asked
        // for with the person's account.
        let (host, _, _) = link.location();
        let attached = source
            .strip_prefix("https://")
            .and_then(|rest| rest.split_once('/'))
            .is_some_and(|(of, path)| {
                of.eq_ignore_ascii_case(host) && path.starts_with("user-attachments/")
            });
        if !attached {
            return None;
        }
        let bytes = source::read_attachment(source, PICTURE_BYTES)?;
        image_preview::decode_bytes(&bytes, limit)
    })
}

/// The pictures of what is written in the pull request shown. They are read
/// one at a time on a thread of their own, so that none holds up the pull
/// request, and let go with it.
struct Pictures {
    replies: (mpsc::Sender<ReadPicture>, mpsc::Receiver<ReadPicture>),
    worker: Option<mpsc::Sender<(u64, PullRequest, String)>>,
    /// Taken by the worker, which starts with the first picture.
    fetch: Option<Fetch>,
    /// Counts the pull requests shown: a picture of an earlier one is
    /// neither read nor kept.
    turn: Arc<AtomicU64>,
    read: ui::markup::Pictures,
    asked: Vec<String>,
    /// What is shown was read anew: its pictures are looked for.
    due: bool,
}

impl Default for Pictures {
    fn default() -> Self {
        Self {
            replies: mpsc::channel(),
            worker: None,
            // Tests of the application reach no host.
            fetch: (!cfg!(test)).then(|| Box::new(fetch_picture) as Fetch),
            turn: Arc::default(),
            read: ui::markup::Pictures::new(),
            asked: Vec::new(),
            due: false,
        }
    }
}

impl Pictures {
    fn forget(&mut self) {
        self.turn.fetch_add(1, Ordering::Relaxed);
        self.read.clear();
        self.asked.clear();
        self.due = true;
    }

    fn ask(&mut self, ctx: &egui::Context, link: &PullRequest, source: String) -> bool {
        if self.worker.is_none() {
            let Some(fetch) = self.fetch.take() else {
                return false;
            };
            let (sender, requests) = mpsc::channel::<(u64, PullRequest, String)>();
            let (replies, turn, wake) = (self.replies.0.clone(), self.turn.clone(), ctx.clone());
            let spawned = std::thread::Builder::new()
                .name("neptune-pull-request-pictures".into())
                .spawn(move || {
                    for (of, link, source) in requests {
                        if turn.load(Ordering::Relaxed) != of {
                            continue;
                        }
                        let picture = fetch(&link, &source);
                        if replies.send((of, source, picture)).is_err() {
                            return;
                        }
                        wake.request_repaint();
                    }
                });
            if spawned.is_err() {
                return false;
            }
            self.worker = Some(sender);
        }
        let of = self.turn.load(Ordering::Relaxed);
        self.worker
            .as_ref()
            .is_some_and(|worker| worker.send((of, link.clone(), source)).is_ok())
    }
}

pub(super) struct PullRequestTab {
    replies: (mpsc::Sender<Reply>, mpsc::Receiver<Reply>),
    worker: Option<mpsc::Sender<Request>>,
    /// Taken by the worker, which starts with the first pull request opened.
    source: Option<Source>,
    /// What the worker was last told to watch.
    sent: Option<Watch>,
    /// The pull requests opened in the tab, in the order they were.
    opened: Vec<PullRequest>,
    /// What was last read of those not in view, shown at once on return.
    kept: Vec<(PullRequest, Box<Detail>)>,
    /// What the repository of the one shown offers it.
    choices: Option<Choices>,
    /// The pull request the tab shows; without one it lists those linked.
    shown: Option<PullRequest>,
    detail: Option<Result<Box<Detail>, Failure>>,
    files: Option<Result<Files, Failure>>,
    /// Which reading of each the tab waits for: see `Watch`.
    detail_turn: u64,
    files_turn: u64,
    pictures: Pictures,
    /// The person asked for it to be read again, and it has not been yet.
    refreshing: bool,
    /// What is being done to it.
    acting: Option<Act>,
    /// Why the last thing asked of it was not done.
    problem: Option<(&'static str, String)>,
}

impl Default for PullRequestTab {
    fn default() -> Self {
        Self {
            replies: mpsc::channel(),
            worker: None,
            source: Some(Source::default()),
            sent: None,
            opened: Vec::new(),
            kept: Vec::new(),
            choices: None,
            shown: None,
            detail: None,
            files: None,
            detail_turn: 0,
            files_turn: 0,
            pictures: Pictures::default(),
            refreshing: false,
            acting: None,
            problem: None,
        }
    }
}

/// Takes changes that are only white space out of a diff: a run of removed
/// lines followed by as many added ones that say the same without their
/// white space reads as lines that did not change.
fn without_whitespace(body: DiffBody) -> DiffBody {
    use crate::ui::changes::LineKind;
    let DiffBody::Lines {
        mut lines,
        widest,
        truncated,
    } = body
    else {
        return body;
    };
    let bare = |text: &str| -> String { text.split_whitespace().collect() };
    let mut at = 0;
    while at < lines.len() {
        let run = |from: usize, kind| {
            lines[from..]
                .iter()
                .take_while(|line| line.kind == kind)
                .count()
        };
        let removed = run(at, LineKind::Removed);
        let added = run(at + removed, LineKind::Added);
        if removed == 0 {
            at += added.max(1);
            continue;
        }
        let same = removed == added
            && (0..removed)
                .all(|i| bare(&lines[at + i].text) == bare(&lines[at + removed + i].text));
        if same {
            // The lines as they are now stay, numbered on both sides.
            for i in 0..removed {
                let old = lines[at + i].old;
                let now = &mut lines[at + removed + i];
                now.kind = LineKind::Context;
                now.old = old;
            }
            lines.drain(at..at + removed);
            at += removed;
        } else {
            at += removed + added;
        }
    }
    let changed = lines
        .iter()
        .any(|line| matches!(line.kind, LineKind::Added | LineKind::Removed));
    if !changed {
        return DiffBody::Note("Only white space changed in this file.");
    }
    DiffBody::Lines {
        lines,
        widest,
        truncated,
    }
}

fn listed(files: Vec<ChangedFile>, total: u32, plain: bool) -> Files {
    let more = (total as usize).saturating_sub(files.len());
    let (files, diffs) = files
        .into_iter()
        .map(|file| {
            let body = match &file.patch {
                Some(patch) if plain => {
                    without_whitespace(super::git::parse_diff(patch.as_bytes()))
                }
                Some(patch) => super::git::parse_diff(patch.as_bytes()),
                None if file.change == Change::Renamed && file.added + file.removed == 0 => {
                    DiffBody::Same
                }
                None => DiffBody::Note(
                    "GitHub shows no diff for this file: it is not text, or it changed too much.",
                ),
            };
            let listed = File {
                path: file.path,
                from: file.from,
                status: match file.change {
                    Change::Modified => Status::Modified,
                    Change::Added => Status::Added,
                    Change::Deleted => Status::Deleted,
                    Change::Renamed => Status::Renamed,
                },
                added: Some(file.added),
                removed: Some(file.removed),
            };
            (listed, body)
        })
        .unzip();
    Files { files, diffs, more }
}

/// Reads what is watched: at once when it is new or asked for, then as it
/// ages. Requests made while the GitHub CLI ran are taken together, newest
/// last, and what the person asked to be done comes before the next read.
fn worker(
    source: Source,
    requests: mpsc::Receiver<Request>,
    replies: mpsc::Sender<Reply>,
    wake: egui::Context,
) {
    let mut watch: Option<Watch> = None;
    let mut known: Option<(PullRequest, Result<Detail, Failure>)> = None;
    // The reading of it the tab was last told.
    let mut told: Option<u64> = None;
    // The pull request, commit and reading whose files were read.
    let mut files_of: Option<(PullRequest, String)> = None;
    let mut wanted: Vec<PullRequest> = Vec::new();
    let mut next = Instant::now();
    let mut asked = false;
    loop {
        let mut request = match &watch {
            None => requests
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            Some(_) => requests.recv_timeout(next.saturating_duration_since(Instant::now())),
        };
        let mut acts = Vec::new();
        loop {
            match request {
                Ok(Request::Watch(new)) => {
                    let link = |watch: &Option<Watch>| {
                        watch.as_ref().map(|watch| watch.link.url().to_owned())
                    };
                    let front = |watch: &Option<Watch>| {
                        watch.as_ref().is_some_and(|watch| !watch.background)
                    };
                    // Another pull request, one that returns to view and a
                    // window that returns to the front are read at once.
                    if link(&new) != link(&watch) || (front(&new) && !front(&watch)) {
                        next = Instant::now();
                    }
                    watch = new;
                }
                Ok(Request::Refresh) => {
                    files_of = None;
                    asked = true;
                    next = Instant::now();
                }
                Ok(Request::Act(link, id, act)) => acts.push((link, id, act)),
                Ok(Request::Choices(link)) => wanted.push(link),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
            match requests.try_recv() {
                Ok(newer) => request = Ok(newer),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }
        let mut reports = Vec::new();
        for link in wanted.drain(..) {
            let read = (source.choices)(&link);
            reports.push(Reply::Choices { link, read });
        }
        for (link, id, act) in acts {
            let result = (source.act)(&link, &id, &act);
            reports.push(Reply::Done { link, act, result });
            // What it changed is shown as soon as it can be read.
            asked = true;
            next = Instant::now();
        }
        if let Some(current) = &watch {
            // A tab that let go of what it had is told again at once.
            let waits = told != Some(current.detail_turn);
            if waits || Instant::now() >= next {
                let started = Instant::now();
                let read = (source.detail)(&current.link);
                let slower = if current.background { BACKGROUND } else { 1 };
                next = Instant::now()
                    + (refresh_after(&read) * slower).max(started.elapsed() * WORK_SHARE);
                let same = known
                    .as_ref()
                    .is_some_and(|(link, was)| link.same(&current.link) && *was == read);
                if asked || waits || !same {
                    reports.push(Reply::Detail {
                        link: current.link.clone(),
                        read: read.clone().map(Box::new),
                    });
                    known = Some((current.link.clone(), read));
                }
                told = Some(current.detail_turn);
                asked = false;
            }
            // Its files are read once for each commit it comes to end with,
            // or for the one commit chosen.
            let of_commit = current
                .commit
                .clone()
                .or_else(|| match &known {
                    Some((_, Ok(detail))) => Some(detail.head_commit.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            // Read anew when white space is to be left out, or shown again,
            // and when the tab let go of the files it had.
            let of_commit = format!("{of_commit} {} {}", current.plain, current.files_turn);
            let read_for = |of_commit: &str| {
                files_of
                    .as_ref()
                    .is_some_and(|(of, commit)| of.same(&current.link) && commit == of_commit)
            };
            if current.files
                && let Some((link, Err(failure))) = &known
                && link.same(&current.link)
            {
                // Its files cannot be read while it cannot be: the tab is
                // told so once, and they are read when it can be again.
                let of_commit = format!("{of_commit} unread");
                if !read_for(&of_commit) {
                    reports.push(Reply::Files {
                        link: link.clone(),
                        turn: current.files_turn,
                        read: Err(*failure),
                    });
                    files_of = Some((link.clone(), of_commit));
                }
            } else if current.files
                && let Some((link, Ok(detail))) = &known
                && link.same(&current.link)
                && !read_for(&of_commit)
            {
                let read = (source.files)(link, current.commit.as_deref()).map(|files| {
                    // One commit's files are all listed, or cut off unseen.
                    let total = match current.commit {
                        Some(_) => files.len() as u32,
                        None => detail.files,
                    };
                    listed(files, total, current.plain)
                });
                files_of = Some((link.clone(), of_commit));
                reports.push(Reply::Files {
                    link: link.clone(),
                    turn: current.files_turn,
                    read,
                });
            }
        }
        if reports.is_empty() {
            continue;
        }
        for report in reports {
            if replies.send(report).is_err() {
                return;
            }
        }
        wake.request_repaint();
    }
}

impl PullRequestTab {
    fn request(&mut self, ctx: &egui::Context, request: Request) -> bool {
        if self.worker.is_none() {
            let Some(source) = self.source.take() else {
                return false;
            };
            let (sender, requests) = mpsc::channel();
            let replies = self.replies.0.clone();
            let wake = ctx.clone();
            let spawned = std::thread::Builder::new()
                .name("neptune-pull-request".into())
                .spawn(move || worker(source, requests, replies, wake));
            if spawned.is_err() {
                return false;
            }
            self.worker = Some(sender);
        }
        self.worker
            .as_ref()
            .is_some_and(|worker| worker.send(request).is_ok())
    }

    /// The pull request the tab shows, if it shows one.
    pub(super) fn shown(&self) -> Option<&PullRequest> {
        self.shown.as_ref()
    }

    /// The pull requests opened in the tab.
    pub(super) fn opened(&self) -> &[PullRequest] {
        &self.opened
    }

    /// Shows `link`, or the list without one. What was read of the one that
    /// leaves view is kept, and what was kept of the one that comes is shown
    /// at once while it is read again.
    fn turn_to(&mut self, link: Option<PullRequest>) {
        /// The most pull requests whose last reading is kept.
        const KEPT: usize = 8;
        if let (Some(was), Some(Ok(detail))) = (self.shown.take(), self.detail.take()) {
            self.kept.retain(|(of, _)| !of.same(&was));
            if self.kept.len() >= KEPT {
                self.kept.remove(0);
            }
            self.kept.push((was, detail));
        }
        self.detail = link.as_ref().and_then(|link| {
            let place = self.kept.iter().position(|(of, _)| of.same(link))?;
            Some(Ok(self.kept.remove(place).1))
        });
        self.shown = link;
        self.detail_turn += 1;
        self.forget_files();
        self.pictures.forget();
        self.choices = None;
        self.refreshing = false;
        self.acting = None;
        self.problem = None;
    }

    /// Lets go of the files read: they are read again for what is shown now.
    fn forget_files(&mut self) {
        self.files = None;
        self.files_turn += 1;
    }

    /// What the tab draws: the pull request in `shown`, or `linked`, the
    /// pull requests of the workspace in view.
    pub(super) fn view<'a>(
        &'a self,
        linked: &'a [ui::helpers::LinkedPullRequest],
        selected: Option<&str>,
    ) -> Body<'a> {
        let Some(link) = &self.shown else {
            return Body::Linked(linked);
        };
        match &self.detail {
            None => Body::Reading(link),
            Some(Err(failure)) => Body::Failed(link, *failure),
            Some(Ok(detail)) => Body::Shown(Shown {
                link,
                detail,
                files: match &self.files {
                    None => FileList::Reading,
                    Some(Err(failure)) => FileList::Failed(*failure),
                    Some(Ok(files)) => FileList::Listed {
                        files: &files.files,
                        more: files.more,
                        diff: selected
                            .and_then(|path| files.files.iter().position(|file| file.path == path))
                            .map(|place| (&files.files[place], &files.diffs[place])),
                    },
                },
                choices: self.choices.as_ref(),
                pictures: &self.pictures.read,
                refreshing: self.refreshing,
                acting: self.acting.as_ref(),
                problem: self
                    .problem
                    .as_ref()
                    .map(|(title, said)| Problem { title, said }),
            }),
        }
    }
}

impl App {
    /// What the worker reported, taken whether or not anything shows it.
    pub(super) fn poll_pull_request(&mut self, ctx: &egui::Context) {
        while let Ok((of, source, picture)) = self.pull_request.pictures.replies.1.try_recv() {
            let pictures = &mut self.pull_request.pictures;
            if pictures.turn.load(Ordering::Relaxed) != of {
                continue;
            }
            pictures.asked.retain(|asked| *asked != source);
            let picture = picture.map(|(pixels, image)| ui::markup::Picture {
                pixels,
                texture: ctx.load_texture(
                    "pull-request-picture",
                    image,
                    egui::TextureOptions::LINEAR,
                ),
            });
            pictures.read.insert(source, picture);
            ctx.request_repaint();
        }
        while let Ok(reply) = self.pull_request.replies.1.try_recv() {
            let of = |link: &PullRequest| {
                self.pull_request
                    .shown
                    .as_ref()
                    .is_some_and(|shown| shown.same(link))
            };
            match reply {
                Reply::Detail { link, read } if of(&link) => {
                    self.pull_request.refreshing = false;
                    // What was read stays in view while one read fails.
                    if read.is_ok() || !matches!(self.pull_request.detail, Some(Ok(_))) {
                        self.pull_request.detail = Some(read);
                        self.pull_request.pictures.due = true;
                    }
                }
                // Files read for what the tab showed before are not its.
                Reply::Files { link, turn, read }
                    if of(&link) && turn == self.pull_request.files_turn =>
                {
                    if let Ok(files) = &read
                        && let Some(selected) = &self.ui.pull_request.selected
                        && !files.files.iter().any(|file| file.path == *selected)
                    {
                        self.ui.pull_request.selected = None;
                    }
                    self.pull_request.files = Some(read);
                }
                Reply::Done { link, act, result } if of(&link) => {
                    self.pull_request.acting = None;
                    self.ui.pull_request.confirm = None;
                    match result {
                        Ok(()) => self.ui.pull_request.done(link.url(), &act),
                        Err(said) => {
                            let said = if said.is_empty() {
                                "The host refused it. Check that the account signed in to the GitHub CLI may do this, then try again.".to_owned()
                            } else {
                                said
                            };
                            self.pull_request.problem = Some((act.outcome().1, said));
                        }
                    }
                }
                Reply::Choices { link, read } if of(&link) => {
                    // A list that could not be read offers nothing to choose.
                    self.pull_request.choices = Some(read.unwrap_or_default());
                }
                _ => {}
            }
            ctx.request_repaint();
        }
    }

    /// Asks for the pictures of what is shown that were not asked for yet,
    /// as many as one pull request may show.
    fn want_pictures(&mut self, ctx: &egui::Context) {
        let tab = &mut self.pull_request;
        if !std::mem::take(&mut tab.pictures.due) {
            return;
        }
        let (Some(link), Some(Ok(detail))) = (&tab.shown, &tab.detail) else {
            return;
        };
        let written =
            std::iter::once(&detail.body).chain(detail.entries.iter().flat_map(|entry| {
                let replies = match &entry.kind {
                    source::Kind::Thread { replies, .. } => replies.as_slice(),
                    _ => &[],
                };
                std::iter::once(&entry.body).chain(replies.iter().map(|reply| &reply.body))
            }));
        for source in written.flat_map(|text| ui::markup::pictures_in(text)) {
            let pictures = &mut tab.pictures;
            if pictures.read.contains_key(&source) || pictures.asked.contains(&source) {
                continue;
            }
            let room = pictures.read.len() + pictures.asked.len() < MAX_PICTURES;
            if room && fetched(link, &source) && pictures.ask(ctx, link, source.clone()) {
                pictures.asked.push(source);
            } else {
                pictures.read.insert(source, None);
            }
        }
    }

    /// Tells the worker what this frame shows. A pull request is read only
    /// while its tab is in view, less often while another window is the
    /// active one, and not at all while this one is minimized.
    pub(super) fn sync_pull_request(&mut self, ctx: &egui::Context, tab: bool) {
        let minimized = ctx.input(|input| input.viewport().minimized == Some(true));
        let watch = self
            .pull_request
            .shown
            .clone()
            .filter(|_| tab && !minimized)
            .map(|link| Watch {
                link,
                files: self.ui.pull_request.segment == ui::pull_request::Segment::Code,
                commit: self.ui.pull_request.scope.clone(),
                plain: self.ui.pull_request.hide_whitespace,
                background: self.window_focused == Some(false),
                detail_turn: self.pull_request.detail_turn,
                files_turn: self.pull_request.files_turn,
            });
        if watch.is_some() {
            self.want_pictures(ctx);
        }
        if watch != self.pull_request.sent {
            // Nothing is started for a tab that shows no pull request.
            if watch.is_some() || self.pull_request.worker.is_some() {
                self.pull_request
                    .request(ctx, Request::Watch(watch.clone()));
            }
            self.pull_request.sent = watch;
        }
    }

    /// The pull requests linked by the terminals of the workspace in view,
    /// newest first, each once.
    pub(super) fn linked_pull_requests(&self) -> Vec<ui::helpers::LinkedPullRequest> {
        let model = self.controller.model();
        let mut linked: Vec<ui::helpers::LinkedPullRequest> = Vec::new();
        for link in model
            .active_workspace()
            .and_then(|id| model.workspace(id))
            .into_iter()
            .flat_map(|workspace| workspace.panes())
            .flat_map(|pane| pane.pull_requests().iter().rev())
        {
            if !linked.iter().any(|known| known.link.same(link)) {
                linked.push(ui::helpers::LinkedPullRequest {
                    link: link.clone(),
                    lookup: self.pull_requests.lookup(link),
                    preview: self.pull_requests.preview(link),
                });
            }
        }
        linked
    }

    /// The tab is no longer the one in view: its field must not keep the
    /// keyboard. What was typed in it stays.
    pub(super) fn leave_pull_request(&mut self, ctx: &egui::Context) {
        self.ui.pull_request.focus = false;
        ctx.memory_mut(|memory| {
            for id in ui::pull_request::field_ids() {
                if memory.has_focus(id) {
                    memory.surrender_focus(id);
                }
            }
        });
    }

    /// Escape in the tab's field puts the comment away and returns the
    /// keyboard to the terminal. Returns whether the key was the tab's.
    pub(super) fn pull_request_escape(&mut self, ctx: &egui::Context) -> bool {
        // The toolkit has already taken focus from the field for this key.
        let held =
            |id| ctx.memory(|memory| memory.has_focus(id) || memory.had_focus_last_frame(id));
        let Some(id) = ui::pull_request::field_ids()
            .into_iter()
            .find(|id| held(*id))
        else {
            return false;
        };
        let tab = &mut self.ui.pull_request;
        if id == ui::pull_request::composer_id() {
            tab.composer = None;
            tab.line = None;
        } else if id == ui::pull_request::reply_id() {
            tab.replying = None;
        } else if id == ui::pull_request::rewrite_id() {
            tab.rewriting = None;
        } else {
            tab.editing = None;
        }
        tab.focus = false;
        ctx.memory_mut(|memory| memory.surrender_focus(id));
        true
    }

    /// Writes `words` at the prompt of the terminal in front, for the person
    /// to send: a question for the agent that runs there, or a command.
    fn hand_to_terminal(&mut self, ctx: &egui::Context, words: &str, agent: bool) {
        let model = self.controller.model();
        let pane = model.active_pane().and_then(|id| model.pane(id));
        let runs_agent = pane
            .is_some_and(|pane| pane.agent().is_some() || self.agents.host(pane.id()).is_some());
        let remote = model
            .active_workspace()
            .and_then(|id| model.workspace(id))
            .is_some_and(|workspace| workspace.remote().is_some());
        let Some(pane) = pane.map(|pane| pane.id()) else {
            self.ui.error = Some("Open a terminal first.".into());
            return;
        };
        if agent && !runs_agent {
            self.ui.error = Some(
                "The terminal in front runs no agent. Focus one that does, then try again.".into(),
            );
        } else if !agent && (runs_agent || remote) {
            self.ui.error = Some(
                "Focus a terminal with a shell on this computer to check the pull request out."
                    .into(),
            );
        } else {
            self.paste_text(pane, words);
            self.action(ctx, Action::Focus(pane));
        }
    }

    /// Starts an agent in a terminal of its own with `task`, from the agent
    /// of the terminal in front and in its folder, and shows its terminal.
    fn start_agent_on(&mut self, ctx: &egui::Context, task: String) {
        use neptune_model::AgentKind;
        let model = self.controller.model();
        let front = model
            .active_pane()
            .and_then(|id| model.pane(id))
            .and_then(|pane| Some((pane, pane.agent()?)));
        let Some((pane, agent)) = front else {
            self.ui.error = Some(
                "The terminal in front runs no agent. Focus one that runs Claude Code or Codex, which starts the new agent, then try again."
                    .into(),
            );
            return;
        };
        let (parent, generation) = (pane.id(), pane.generation());
        let cwd = agent.cwd.clone();
        let task = crate::runtime::agents::Task {
            // The CLIs Neptune starts with a task; the one in use if it is one.
            kind: match agent.kind {
                AgentKind::Codex => AgentKind::Codex,
                _ => AgentKind::Claude,
            },
            prompt: task,
            model: None,
            effort: None,
            ultracode: false,
            resume: None,
            title: None,
            worktree: None,
        };
        match self.start_from(ctx, parent, generation, task, cwd) {
            Ok(started) => self.action(ctx, Action::Focus(started)),
            Err(why) => self.ui.error = Some(why),
        }
    }

    pub(super) fn pull_request_event(&mut self, ctx: &egui::Context, event: Event) {
        match event.clone() {
            Event::Open(link) => {
                // A window without room for the panel has the browser.
                if !self.ui.panel.available {
                    if let Some(link) = crate::platform::links::WebLink::new(link.url()) {
                        self.action(ctx, Action::OpenLink(link));
                    }
                    return;
                }
                if !self
                    .pull_request
                    .opened
                    .iter()
                    .any(|opened| opened.same(&link))
                {
                    // The tab holds a handful; the oldest gives way.
                    if self.pull_request.opened.len() >= ui::pull_request::MAX_OPEN {
                        self.pull_request.opened.remove(0);
                    }
                    self.pull_request.opened.push(link.clone());
                }
                if !self
                    .pull_request
                    .shown
                    .as_ref()
                    .is_some_and(|shown| shown.same(&link))
                {
                    self.pull_request.turn_to(Some(link));
                    self.ui.pull_request.opened();
                }
                self.panel_event(ctx, ui::panel::Event::Show(ui::panel::Tab::PullRequest));
            }
            Event::Back | Event::Listed => {
                let listed = event == Event::Listed;
                self.pull_request.turn_to(None);
                self.ui.pull_request.opened();
                self.leave_pull_request(ctx);
                if listed {
                    self.panel_event(ctx, ui::panel::Event::Show(ui::panel::Tab::PullRequest));
                }
            }
            Event::Close(link) => {
                let Some(place) = self
                    .pull_request
                    .opened
                    .iter()
                    .position(|opened| opened.same(&link))
                else {
                    return;
                };
                self.pull_request.opened.remove(place);
                self.pull_request.kept.retain(|(of, _)| !of.same(&link));
                // Closing the one in view shows the one that takes its
                // place, or the list after the last.
                if self
                    .pull_request
                    .shown
                    .as_ref()
                    .is_some_and(|shown| shown.same(&link))
                {
                    let opened = &self.pull_request.opened;
                    let next = opened.get(place).or(opened.last()).cloned();
                    self.pull_request.turn_to(next);
                    self.pull_request.kept.retain(|(of, _)| !of.same(&link));
                    self.ui.pull_request.opened();
                    self.leave_pull_request(ctx);
                }
            }
            Event::Choices => {
                if let Some(link) = self.pull_request.shown.clone() {
                    self.pull_request.request(ctx, Request::Choices(link));
                }
            }
            Event::Whitespace(hide) => {
                self.ui.pull_request.hide_whitespace = hide;
                self.pull_request.forget_files();
            }
            Event::Hand { words, agent } => self.hand_to_terminal(ctx, &words, agent),
            Event::Start(task) => self.start_agent_on(ctx, task),
            Event::Worktree => {
                let model = self.controller.model();
                let front = model.active_pane().and_then(|id| model.pane(id));
                let (Some(pane), Some(link)) = (front, self.pull_request.shown.clone()) else {
                    self.ui.error = Some("Open a terminal in the repository first.".into());
                    return;
                };
                // The CLI in use there runs in the new tab; Claude Code
                // where the terminal runs none.
                let agent = pane
                    .agent()
                    .map_or(neptune_model::AgentKind::Claude, |agent| agent.kind);
                let (_, owner, repository) = link.location();
                let of = (format!("{owner}/{repository}"), link.number());
                if let Err(why) = self.checkout_worktree(ctx, pane.id(), of, agent) {
                    self.ui.error = Some(why.into());
                }
            }
            Event::Scope(commit) => {
                self.ui.pull_request.scope = commit;
                self.ui.pull_request.selected = None;
                self.ui.pull_request.segment = ui::pull_request::Segment::Code;
                self.pull_request.forget_files();
            }
            Event::Refresh => {
                if self.pull_request.shown.is_some() {
                    self.pull_request.refreshing = true;
                    // One that failed to be read is read anew.
                    if matches!(self.pull_request.detail, Some(Err(_))) {
                        self.pull_request.detail = None;
                    }
                    if matches!(self.pull_request.files, Some(Err(_))) {
                        self.pull_request.forget_files();
                    }
                    self.pull_request.request(ctx, Request::Refresh);
                }
            }
            Event::Act(act) => {
                // One thing is done at a time.
                if self.pull_request.acting.is_none()
                    && let Some(link) = self.pull_request.shown.clone()
                    && let Some(Ok(detail)) = &self.pull_request.detail
                {
                    let id = detail.id.clone();
                    self.pull_request.problem = None;
                    self.pull_request.acting = Some(act.clone());
                    if !self.pull_request.request(ctx, Request::Act(link, id, act)) {
                        self.pull_request.acting = None;
                    }
                }
            }
            Event::DismissProblem => self.pull_request.problem = None,
            Event::Select(path) => self.ui.pull_request.selected = Some(path),
            Event::CloseDiff => self.ui.pull_request.selected = None,
            Event::Copy(text) => crate::platform::clipboard::copy(ctx, text),
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::pull_request::{Allowed, Method, Verdict};
    use crate::runtime::pull_requests::State;
    use crate::ui::{
        changes::LineKind,
        panel::{Event as Panel, Tab},
        pull_request::Segment,
    };
    use std::sync::Mutex;

    const WINDOW: Vec2 = Vec2::new(900.0, 640.0);

    fn frame(app: &mut App, ctx: &egui::Context) {
        let mut host = eframe::Frame::_new_kittest();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, WINDOW)),
                ..Default::default()
            },
            |ui| {
                app.poll_pull_request(ui.ctx());
                eframe::App::ui(app, ui, &mut host);
            },
        );
        output.textures_delta.clear();
    }

    fn settle(app: &mut App, ctx: &egui::Context, what: &str, done: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            frame(app, ctx);
            if done(app) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn link(number: u64) -> PullRequest {
        PullRequest::parse(&format!("https://github.com/zevem/neptune/pull/{number}")).unwrap()
    }

    pub(in crate::app) fn detail(title: &str) -> Detail {
        Detail {
            id: "PR_1".into(),
            auto_merge: None,
            viewed: Vec::new(),
            reactions: Vec::new(),
            stacked: false,
            stack: Vec::new(),
            title: title.into(),
            state: State::Open,
            author: "ada".into(),
            body: "What it does.".into(),
            base: "main".into(),
            head: "feat/tab".into(),
            opened: 1_791_342_861,
            updated: 1_791_342_861,
            ended: None,
            merged_by: String::new(),
            commits: 1,
            history: Vec::new(),
            head_commit: "8b45e82".into(),
            added: 3,
            removed: 1,
            files: 2,
            merge: source::Merge::Ready,
            decision: None,
            reviewers: Vec::new(),
            labels: Vec::new(),
            assignees: Vec::new(),
            checks: Vec::new(),
            more_checks: 0,
            entries: Vec::new(),
            earlier: 0,
            allowed: Allowed {
                update: true,
                merge: true,
                judge: true,
                methods: vec![Method::Squash],
                ..Allowed::default()
            },
        }
    }

    /// What the stand-in for the GitHub CLI was asked, and what it answers.
    #[derive(Default)]
    struct Host {
        title: String,
        body: String,
        reads: usize,
        file_reads: usize,
        acts: Vec<Act>,
        refuse: Option<String>,
        missing: bool,
    }

    fn opened(root: &std::path::Path) -> (App, egui::Context, Arc<Mutex<Host>>) {
        let (mut app, _sender) = super::super::tests::fixture(root);
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        app.startup = None;
        app.controller
            .dispatch(Command::AddWorkspace {
                group: None,
                cwd: root.into(),
                name: "Work".into(),
                remote: None,
            })
            .unwrap();
        let host = Arc::new(Mutex::new(Host {
            title: "A tab for pull requests".into(),
            ..Host::default()
        }));
        let (reads, lists, acts) = (host.clone(), host.clone(), host.clone());
        app.pull_request.source = Some(Source {
            detail: Box::new(move |_| {
                let mut host = reads.lock().unwrap();
                host.reads += 1;
                if host.missing {
                    return Err(Failure::NotFound);
                }
                let mut detail = detail(&host.title);
                if !host.body.is_empty() {
                    detail.body = host.body.clone();
                }
                if host.acts.contains(&Act::Merge(Method::Squash)) {
                    detail.state = State::Merged;
                }
                Ok(detail)
            }),
            files: Box::new(move |_, _| {
                lists.lock().unwrap().file_reads += 1;
                Ok(vec![
                    ChangedFile {
                        path: "src/a.rs".into(),
                        from: None,
                        change: Change::Modified,
                        added: 1,
                        removed: 1,
                        patch: Some("@@ -1,2 +1,2 @@\n one\n-two\n+2\n".into()),
                    },
                    ChangedFile {
                        path: "logo.png".into(),
                        from: None,
                        change: Change::Added,
                        added: 0,
                        removed: 0,
                        patch: None,
                    },
                ])
            }),
            choices: Box::new(|_| {
                Ok(Choices {
                    labels: vec!["ui".into(), "bug".into()],
                    people: vec!["grace".into()],
                })
            }),
            act: Box::new(move |_, _, act| {
                let mut host = acts.lock().unwrap();
                match host.refuse.clone() {
                    Some(said) => Err(said),
                    None => {
                        host.acts.push(act.clone());
                        Ok(())
                    }
                }
            }),
        });
        (app, ctx, host)
    }

    fn title(app: &App) -> Option<String> {
        match &app.pull_request.detail {
            Some(Ok(detail)) => Some(detail.title.clone()),
            _ => None,
        }
    }

    #[test]
    fn a_pull_request_opens_in_its_tab_and_is_read_only_while_in_view() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, host) = opened(dir.path());
        frame(&mut app, &ctx);
        // The tab without a pull request starts nothing.
        app.action(&ctx, Action::Panel(Panel::Show(Tab::PullRequest)));
        frame(&mut app, &ctx);
        assert!(app.pull_request.worker.is_none());
        assert!(matches!(app.pull_request.view(&[], None), Body::Linked([])));

        app.action(&ctx, Action::Panel(Panel::Toggle));
        app.action(&ctx, Action::PullRequest(Event::Open(link(83))));
        assert!(app.ui.panel.open && app.ui.panel.tab == Tab::PullRequest);
        assert!(matches!(app.pull_request.view(&[], None), Body::Reading(_)));
        settle(&mut app, &ctx, "the pull request", |app| {
            title(app).as_deref() == Some("A tab for pull requests")
        });
        assert_eq!(host.lock().unwrap().reads, 1);
        assert_eq!(
            host.lock().unwrap().file_reads,
            0,
            "its files are not in view"
        );

        // Its files are read when their part is shown, once for a commit.
        app.ui.pull_request.segment = Segment::Code;
        settle(&mut app, &ctx, "its files", |app| {
            app.pull_request.files.is_some()
        });
        app.action(&ctx, Action::PullRequest(Event::Select("src/a.rs".into())));
        let selected = app.ui.pull_request.selected.clone();
        let Body::Shown(shown) = app.pull_request.view(&[], selected.as_deref()) else {
            panic!("the pull request is shown");
        };
        let FileList::Listed { files, more, diff } = shown.files else {
            panic!("its files are listed");
        };
        assert_eq!((files.len(), more), (2, 0));
        let Some((file, DiffBody::Lines { lines, .. })) = diff else {
            panic!("the chosen file has a diff");
        };
        assert_eq!(file.path, "src/a.rs");
        let changed: Vec<_> = lines
            .iter()
            .filter(|line| matches!(line.kind, LineKind::Added | LineKind::Removed))
            .map(|line| (line.kind, line.text.as_str()))
            .collect();
        assert_eq!(
            changed,
            [(LineKind::Removed, "two"), (LineKind::Added, "2")]
        );
        let Body::Shown(shown) = app.pull_request.view(&[], Some("logo.png")) else {
            panic!("the pull request is shown");
        };
        assert!(matches!(
            shown.files,
            FileList::Listed {
                diff: Some((_, DiffBody::Note(_))),
                ..
            }
        ));

        // Asked for again, it is read again and its files with it.
        host.lock().unwrap().title = "Renamed".into();
        app.action(&ctx, Action::PullRequest(Event::Refresh));
        assert!(app.pull_request.refreshing);
        settle(&mut app, &ctx, "the new title", |app| {
            title(app).as_deref() == Some("Renamed") && !app.pull_request.refreshing
        });
        settle(&mut app, &ctx, "its files again", |_| {
            host.lock().unwrap().file_reads == 2
        });

        // Out of view it is not read, and what was read is kept.
        app.action(&ctx, Action::Panel(Panel::Show(Tab::Agents)));
        frame(&mut app, &ctx);
        assert!(app.pull_request.sent.is_none());
        assert_eq!(title(&app).as_deref(), Some("Renamed"));
        let reads = host.lock().unwrap().reads;
        app.action(&ctx, Action::Panel(Panel::Show(Tab::PullRequest)));
        settle(&mut app, &ctx, "a read on return", |_| {
            host.lock().unwrap().reads > reads
        });

        // The way back shows those linked, and another one starts afresh.
        app.action(&ctx, Action::PullRequest(Event::Back));
        assert!(app.pull_request.shown().is_none() && app.pull_request.detail.is_none());
        app.action(&ctx, Action::PullRequest(Event::Open(link(84))));
        assert!(app.pull_request.detail.is_none());
        assert_eq!(app.ui.pull_request.segment, Segment::Summary);
    }

    #[test]
    fn a_pull_request_shown_again_is_told_again_what_was_already_read() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, host) = opened(dir.path());
        let named = |app: &App| title(app).is_some();
        let listed = |app: &App| matches!(app.pull_request.files, Some(Ok(_)));
        app.action(&ctx, Action::PullRequest(Event::Open(link(83))));
        settle(&mut app, &ctx, "the pull request", named);
        app.ui.pull_request.segment = Segment::Code;
        settle(&mut app, &ctx, "its files", listed);

        // Closed and opened again, nothing of it is kept: the worker, which
        // read the same a moment ago, says it again, and its files as well.
        app.action(&ctx, Action::PullRequest(Event::Close(link(83))));
        app.action(&ctx, Action::PullRequest(Event::Open(link(83))));
        assert!(app.pull_request.detail.is_none());
        settle(&mut app, &ctx, "the pull request again", named);
        app.ui.pull_request.segment = Segment::Code;
        settle(&mut app, &ctx, "its files again", listed);

        // So does the way back, which keeps what was read but not its files.
        app.action(&ctx, Action::PullRequest(Event::Back));
        app.action(&ctx, Action::PullRequest(Event::Open(link(83))));
        app.ui.pull_request.segment = Segment::Code;
        settle(&mut app, &ctx, "its files after the way back", listed);

        // One commit chosen and all of them again before the worker looks:
        // the files it read for all of them are the ones still wanted.
        app.action(
            &ctx,
            Action::PullRequest(Event::Scope(Some("8b45e82".into()))),
        );
        app.action(&ctx, Action::PullRequest(Event::Scope(None)));
        assert!(app.pull_request.files.is_none());
        settle(&mut app, &ctx, "the files of all commits", listed);
        app.action(&ctx, Action::PullRequest(Event::Whitespace(true)));
        app.action(&ctx, Action::PullRequest(Event::Whitespace(false)));
        settle(&mut app, &ctx, "its files with their white space", listed);

        // Files read for one commit are not shown as those of all of them:
        // the answer waits unread while all of them are chosen again.
        let files = host.lock().unwrap().file_reads;
        app.action(
            &ctx,
            Action::PullRequest(Event::Scope(Some("8b45e82".into()))),
        );
        frame(&mut app, &ctx);
        let deadline = Instant::now() + Duration::from_secs(20);
        while host.lock().unwrap().file_reads == files {
            assert!(Instant::now() < deadline, "the commit's files are read");
            std::thread::sleep(Duration::from_millis(5));
        }
        // The worker reports a moment after it has read.
        std::thread::sleep(Duration::from_millis(100));
        app.action(&ctx, Action::PullRequest(Event::Scope(None)));
        app.poll_pull_request(&ctx);
        assert!(app.pull_request.files.is_none(), "one commit's files");
        settle(&mut app, &ctx, "all commits after one", |app| {
            listed(app) && host.lock().unwrap().file_reads == files + 2
        });
    }

    #[test]
    fn pictures_written_in_a_pull_request_are_read_from_its_host_only() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, host) = opened(dir.path());
        let attached = "https://github.com/user-attachments/assets/4e0618da";
        let elsewhere = "https://tracker.example/seen.png";
        host.lock().unwrap().body =
            format!("Before\n<img alt=\"The tab\" src=\"{attached}\" />\n![Seen]({elsewhere})");
        let asked = Arc::new(Mutex::new(Vec::new()));
        let fetches = asked.clone();
        app.pull_request.pictures.fetch = Some(Box::new(move |_, source| {
            fetches.lock().unwrap().push(source.to_owned());
            Some((
                [2, 2],
                egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
            ))
        }));
        app.action(&ctx, Action::PullRequest(Event::Open(link(83))));
        settle(&mut app, &ctx, "its pictures", |app| {
            app.pull_request.pictures.read.len() == 2
        });
        let pictures = &app.pull_request.pictures;
        assert!(pictures.read[attached].is_some() && pictures.asked.is_empty());
        assert!(pictures.read[elsewhere].is_none(), "kept by someone else");
        assert_eq!(*asked.lock().unwrap(), [attached]);

        // Read again, it asks for no picture twice; left, it keeps none.
        app.action(&ctx, Action::PullRequest(Event::Refresh));
        settle(&mut app, &ctx, "the refresh", |app| {
            !app.pull_request.refreshing
        });
        frame(&mut app, &ctx);
        assert_eq!(asked.lock().unwrap().len(), 1);
        app.action(&ctx, Action::PullRequest(Event::Back));
        assert!(app.pull_request.pictures.read.is_empty());

        assert!(fetched(
            &link(83),
            "https://private-user-images.githubusercontent.com/1/a.png?jwt=x"
        ));
        assert!(fetched(
            &link(83),
            "https://GitHub.com/zevem/neptune/raw/main/a.png"
        ));
        assert!(!fetched(&link(83), "http://github.com/a.png"));
        assert!(!fetched(
            &link(83),
            "https://github.com.tracker.example/a.png"
        ));
        assert!(!fetched(
            &link(83),
            "https://evilgithubusercontent.com/a.png"
        ));
    }

    #[test]
    fn what_is_asked_of_a_pull_request_is_done_once_and_a_refusal_is_said() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, host) = opened(dir.path());
        frame(&mut app, &ctx);
        app.action(&ctx, Action::PullRequest(Event::Open(link(83))));
        settle(&mut app, &ctx, "the pull request", |app| {
            title(app).is_some()
        });

        // A comment that was posted leaves the field; the composer closes.
        app.ui.pull_request.composer = Some(ui::pull_request::Composer::Comment);
        *app.ui.pull_request.draft_mut(link(83).url()) = "Thanks".into();
        app.action(
            &ctx,
            Action::PullRequest(Event::Act(Act::Comment("Thanks".into()))),
        );
        assert!(app.pull_request.acting.is_some());
        // A second request waits for the first.
        app.action(&ctx, Action::PullRequest(Event::Act(Act::Close)));
        settle(&mut app, &ctx, "the comment", |app| {
            app.pull_request.acting.is_none()
        });
        assert_eq!(host.lock().unwrap().acts, [Act::Comment("Thanks".into())]);
        assert!(app.ui.pull_request.composer.is_none());
        assert_eq!(app.ui.pull_request.draft(link(83).url()), "");

        // A refusal says what the host said, and keeps what was written.
        host.lock().unwrap().refuse = Some("Review cannot be requested from the author".into());
        app.ui.pull_request.composer = Some(ui::pull_request::Composer::Review);
        *app.ui.pull_request.draft_mut(link(83).url()) = "Looks good".into();
        app.action(
            &ctx,
            Action::PullRequest(Event::Act(Act::Review(
                Verdict::Approved,
                "Looks good".into(),
                Vec::new(),
            ))),
        );
        settle(&mut app, &ctx, "the refusal", |app| {
            app.pull_request.problem.is_some()
        });
        assert_eq!(
            app.pull_request.problem,
            Some((
                "Could not submit the review",
                "Review cannot be requested from the author".into()
            ))
        );
        assert_eq!(app.ui.pull_request.draft(link(83).url()), "Looks good");
        assert!(app.ui.pull_request.composer.is_some());
        app.action(&ctx, Action::PullRequest(Event::DismissProblem));
        assert!(app.pull_request.problem.is_none());

        // Merged, it is read again at once and shows where it stands.
        host.lock().unwrap().refuse = None;
        app.action(
            &ctx,
            Action::PullRequest(Event::Act(Act::Merge(Method::Squash))),
        );
        settle(
            &mut app,
            &ctx,
            "the merge",
            |app| matches!(&app.pull_request.detail, Some(Ok(detail)) if detail.state == State::Merged),
        );
    }

    #[test]
    fn a_pull_request_that_cannot_be_read_says_why_and_is_read_anew_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, host) = opened(dir.path());
        host.lock().unwrap().missing = true;
        frame(&mut app, &ctx);
        app.action(&ctx, Action::PullRequest(Event::Open(link(83))));
        settle(&mut app, &ctx, "the failure", |app| {
            app.pull_request.detail.is_some()
        });
        assert!(matches!(
            app.pull_request.view(&[], None),
            Body::Failed(_, Failure::NotFound)
        ));
        // Trying again reads it anew.
        host.lock().unwrap().missing = false;
        app.action(&ctx, Action::PullRequest(Event::Refresh));
        assert!(app.pull_request.detail.is_none());
        settle(&mut app, &ctx, "the pull request", |app| {
            title(app).is_some()
        });
        // One read that fails afterwards leaves what was read in view.
        host.lock().unwrap().missing = true;
        app.action(&ctx, Action::PullRequest(Event::Refresh));
        settle(&mut app, &ctx, "the failed read", |app| {
            !app.pull_request.refreshing
        });
        assert!(title(&app).is_some());
    }

    #[test]
    fn several_pull_requests_stay_open_and_what_is_written_under_one_is_put_away_when_sent() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, host) = opened(dir.path());
        frame(&mut app, &ctx);
        app.action(&ctx, Action::PullRequest(Event::Open(link(83))));
        settle(&mut app, &ctx, "the first", |app| title(app).is_some());
        app.action(&ctx, Action::PullRequest(Event::Open(link(84))));
        assert!(app.pull_request.detail.is_none(), "the second is read anew");
        settle(&mut app, &ctx, "the second", |app| title(app).is_some());
        let numbers = |app: &App| -> Vec<u64> {
            app.pull_request
                .opened()
                .iter()
                .map(PullRequest::number)
                .collect()
        };
        assert_eq!(numbers(&app), [83, 84]);
        // Returning to one shows what was read of it at once.
        app.action(&ctx, Action::PullRequest(Event::Open(link(83))));
        assert!(title(&app).is_some());
        assert_eq!(numbers(&app), [83, 84], "opened again, it is listed once");
        // The tab of the linked ones shows the list and keeps the others open.
        app.action(&ctx, Action::Panel(Panel::Show(Tab::Files)));
        app.action(&ctx, Action::PullRequest(Event::Listed));
        assert!(app.pull_request.shown().is_none() && app.ui.panel.tab == Tab::PullRequest);
        assert_eq!(numbers(&app), [83, 84]);
        app.action(&ctx, Action::PullRequest(Event::Open(link(83))));
        // Closing the one in view shows its neighbour; the last, the list.
        app.action(&ctx, Action::PullRequest(Event::Close(link(83))));
        assert_eq!(app.pull_request.shown().map(PullRequest::number), Some(84));
        app.action(&ctx, Action::PullRequest(Event::Close(link(84))));
        assert!(app.pull_request.shown().is_none() && numbers(&app).is_empty());

        app.action(&ctx, Action::PullRequest(Event::Open(link(83))));
        settle(&mut app, &ctx, "the first again", |app| {
            title(app).is_some()
        });
        // The lists a row's control opens are read when asked for.
        app.action(&ctx, Action::PullRequest(Event::Choices));
        settle(&mut app, &ctx, "the labels", |app| {
            app.pull_request.choices.is_some()
        });
        assert_eq!(app.pull_request.choices.as_ref().unwrap().people, ["grace"]);
        // An answer that was posted leaves its field; the host's name for
        // the pull request went with it.
        app.ui.pull_request.replying = Some("T_1".into());
        app.ui.pull_request.reply = "Done".into();
        let answer = Act::Reply {
            thread: "T_1".into(),
            body: "Done".into(),
        };
        app.action(&ctx, Action::PullRequest(Event::Act(answer.clone())));
        settle(&mut app, &ctx, "the reply", |app| {
            app.pull_request.acting.is_none()
        });
        assert_eq!(host.lock().unwrap().acts, [answer]);
        assert!(app.ui.pull_request.replying.is_none() && app.ui.pull_request.reply.is_empty());
        // A review takes the comments that waited for it.
        let line = source::LineComment {
            path: "src/a.rs".into(),
            line: 2,
            removed: false,
            body: "Why?".into(),
        };
        app.ui
            .pull_request
            .add_pending(link(83).url(), line.clone());
        let review = Act::Review(Verdict::Commented, String::new(), vec![line]);
        app.action(&ctx, Action::PullRequest(Event::Act(review)));
        settle(&mut app, &ctx, "the review", |app| {
            app.pull_request.acting.is_none()
        });
        assert_eq!(app.ui.pull_request.pending(link(83).url()).count(), 0);
        // One commit's files are read in place of all of them.
        app.action(
            &ctx,
            Action::PullRequest(Event::Scope(Some("8b45e82f1a2b".into()))),
        );
        assert_eq!(app.ui.pull_request.segment, Segment::Code);
        settle(&mut app, &ctx, "the commit's files", |app| {
            app.pull_request.files.is_some()
        });
        assert_eq!(
            app.pull_request.sent.as_ref().unwrap().commit.as_deref(),
            Some("8b45e82f1a2b")
        );
    }

    #[test]
    fn words_for_an_agent_go_to_a_terminal_that_runs_one_and_a_command_to_a_shell() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, _) = opened(dir.path());
        frame(&mut app, &ctx);
        let hand = |app: &mut App, agent| {
            app.ui.error = None;
            let words = "About pull request https://github.com/zevem/neptune/pull/83: ".into();
            app.action(&ctx, Action::PullRequest(Event::Hand { words, agent }));
            app.ui.error.clone()
        };
        // A shell takes a command, not a question for an agent.
        assert!(hand(&mut app, true).is_some_and(|said| said.contains("runs no agent")));
        // Nor does it start an agent: one is started by another.
        app.ui.error = None;
        app.action(&ctx, Action::PullRequest(Event::Start("Explain it".into())));
        assert!(
            app.ui
                .error
                .take()
                .is_some_and(|said| said.contains("runs no agent"))
        );
        assert_eq!(hand(&mut app, false), None);
        let pane = app.controller.model().active_pane().unwrap();
        let generation = app.controller.model().pane(pane).unwrap().generation();
        app.controller
            .dispatch(Command::PaneAgentChanged {
                pane,
                generation,
                agent: Some(neptune_model::AgentSession {
                    kind: neptune_model::AgentKind::Claude,
                    session_id: None,
                    cwd: dir.path().into(),
                }),
            })
            .unwrap();
        assert_eq!(hand(&mut app, true), None);
        assert!(hand(&mut app, false).is_some_and(|said| said.contains("shell")));
    }

    #[test]
    fn changes_that_are_only_white_space_are_left_out_when_asked() {
        use crate::ui::changes::LineKind;
        let diff = |patch: &str, plain| {
            let files = vec![ChangedFile {
                path: "a.rs".into(),
                from: None,
                change: Change::Modified,
                added: 0,
                removed: 0,
                patch: Some(patch.into()),
            }];
            listed(files, 1, plain).diffs.remove(0)
        };
        let kinds = |body: &DiffBody| -> Vec<(LineKind, String)> {
            match body {
                DiffBody::Lines { lines, .. } => lines
                    .iter()
                    .filter(|line| line.kind != LineKind::Hunk)
                    .map(|line| (line.kind, line.text.clone()))
                    .collect(),
                other => panic!("no lines: {other:?}"),
            }
        };
        let patch = "@@ -1,3 +1,3 @@\n-if a {\n-b\n+    if a {\n+c\n same\n";
        // The pair that says something else is kept; as written, all are.
        assert_eq!(kinds(&diff(patch, false)).len(), 5);
        assert_eq!(
            kinds(&diff(patch, true)),
            [
                (LineKind::Removed, "if a {".into()),
                (LineKind::Removed, "b".into()),
                (LineKind::Added, "    if a {".into()),
                (LineKind::Added, "c".into()),
                (LineKind::Context, "same".into()),
            ],
            "a run that differs in a line is shown whole"
        );
        let indented = "@@ -1,2 +1,2 @@\n-a\n-b\n+  a\n+\tb\n";
        assert!(matches!(diff(indented, true), DiffBody::Note(_)));
        let mixed = "@@ -1,3 +1,3 @@\n-a\n+  a\n keep\n-old\n+new\n";
        assert_eq!(
            kinds(&diff(mixed, true)),
            [
                (LineKind::Context, "  a".into()),
                (LineKind::Context, "keep".into()),
                (LineKind::Removed, "old".into()),
                (LineKind::Added, "new".into()),
            ]
        );
    }

    /// What a capture shows in place of GitHub: a pull request in review with
    /// a description, reviewers, checks in every outcome, a conversation and
    /// changed files.
    fn pictured(state: &str) -> Detail {
        use source::{Check, Commit, Decision, Entry, Kind, Outcome, Reply, Reviewer};
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let check = |name: &str, workflow: &str, outcome, seconds| Check {
            name: name.into(),
            workflow: workflow.into(),
            outcome,
            seconds,
            url: "https://github.com/zevem/neptune/actions/runs/1".into(),
        };
        let mut pictured = Detail {
            title: "feat(ui): read a pull request in the right panel without leaving the terminal"
                .into(),
            body: "## Problem\n\nA linked pull request opened the browser, away from the terminal that works on it.\n\n## Change\n\n- A **Pull request** tab in the right panel\n- Clicking a number on a terminal's tab opens it there\n- `Ctrl`+click still opens the browser\n\n```sh\ngh pr checkout 118\n```\n\nSee [the design guide](https://neptune.rs/docs/design) for the layout.".into(),
            head: "feat/pull-request-panel".into(),
            opened: now - 26 * 3600,
            updated: now - 9 * 60,
            commits: 4,
            added: 2841,
            removed: 96,
            files: 5,
            history: vec![
                Commit {
                    oid: "1a2b3c4d5e6f".into(),
                    short: "1a2b3c4".into(),
                    headline: "feat(ui): a tab for one pull request".into(),
                    author: "ada".into(),
                    at: now - 25 * 3600,
                },
                Commit {
                    oid: "8b45e82f1a2b".into(),
                    short: "8b45e82".into(),
                    headline: "fix(ui): five names share the narrowest strip".into(),
                    author: "ada".into(),
                    at: now - 40 * 60,
                },
            ],
            decision: Some(Decision::ReviewRequired),
            reviewers: vec![
                Reviewer {
                    name: "grace".into(),
                    verdict: Verdict::Approved,
                },
                Reviewer {
                    name: "linus".into(),
                    verdict: Verdict::ChangesRequested,
                },
                Reviewer {
                    name: "desktop-team".into(),
                    verdict: Verdict::Awaited,
                },
            ],
            labels: vec!["ui".into(), "right panel".into(), "needs-design-review".into()],
            assignees: vec!["ada".into()],
            checks: vec![
                check("Windows build", "CI", Outcome::Failed, Some(312)),
                check("Native visual review", "CI", Outcome::Running, None),
                check("Fast validation", "CI", Outcome::Passed, Some(15)),
                check("Linux build and tests", "CI", Outcome::Passed, Some(642)),
                check("macOS build and tests", "CI", Outcome::Passed, Some(3900)),
                check("Website downloads", "CI", Outcome::Skipped, None),
            ],
            entries: vec![
                Entry {
                    id: "C_1".into(),
                    reactions: Vec::new(),
                    can_edit: true,
                    author: "linus".into(),
                    at: now - 20 * 3600,
                    kind: Kind::Thread {
                        id: "T_1".into(),
                        can_reply: true,
                        can_resolve: true,
                        path: "src/ui/panel.rs".into(),
                        line: Some(241),
                        resolved: false,
                        outdated: false,
                        replies: vec![Reply {
                            author: "ada".into(),
                            at: now - 19 * 3600,
                            body: "It is cut to `PR` only where the strip has no room.".into(),
                        }],
                    },
                    body: "Does the fifth name still fit at the narrowest width?".into(),
                    url: "https://github.com/zevem/neptune/pull/118#discussion_r1".into(),
                },
                Entry {
                    id: "C_2".into(),
                    reactions: Vec::new(),
                    can_edit: true,
                    author: "grace".into(),
                    at: now - 5 * 3600,
                    kind: Kind::Review(Verdict::Approved),
                    body: "Reads well. The checks row is a nice touch.".into(),
                    url: "https://github.com/zevem/neptune/pull/118#pullrequestreview-1".into(),
                },
                Entry {
                    id: "C_3".into(),
                    reactions: Vec::new(),
                    can_edit: true,
                    author: "linus".into(),
                    at: now - 3 * 3600,
                    kind: Kind::Thread {
                        id: "T_2".into(),
                        can_reply: true,
                        can_resolve: true,
                        path: "src/runtime/pull_request.rs".into(),
                        line: Some(52),
                        resolved: true,
                        outdated: true,
                        replies: Vec::new(),
                    },
                    body: "Bound this response.".into(),
                    url: "https://github.com/zevem/neptune/pull/118#discussion_r2".into(),
                },
                Entry {
                    id: "C_4".into(),
                    reactions: Vec::new(),
                    can_edit: true,
                    author: "linus".into(),
                    at: now - 2 * 3600,
                    kind: Kind::Review(Verdict::ChangesRequested),
                    body: "The Windows build has to pass first.".into(),
                    url: "https://github.com/zevem/neptune/pull/118#pullrequestreview-2".into(),
                },
                Entry {
                    id: "C_5".into(),
                    reactions: Vec::new(),
                    can_edit: true,
                    author: "ada".into(),
                    at: now - 12 * 60,
                    kind: Kind::Comment,
                    body: "Pushed a fix for the Windows build:\n\n1. `cli` no longer assumes a POSIX path\n2. the test uses the bundled fonts".into(),
                    url: "https://github.com/zevem/neptune/pull/118#issuecomment-1".into(),
                },
            ],
            allowed: Allowed {
                update: true,
                merge: true,
                judge: true,
                methods: vec![Method::Merge, Method::Squash, Method::Rebase],
                auto_merge: true,
                update_branch: true,
                edit: true,
                react: true,
                label: true,
                request: true,
            },
            ..detail("")
        };
        pictured.reactions = vec![source::Reaction {
            emoji: source::Emoji::Rocket,
            count: 3,
            mine: true,
        }];
        pictured.entries[4].reactions = vec![
            source::Reaction {
                emoji: source::Emoji::ThumbsUp,
                count: 2,
                mine: false,
            },
            source::Reaction {
                emoji: source::Emoji::Heart,
                count: 1,
                mine: true,
            },
        ];
        pictured.viewed = vec!["docs/design.md".into()];
        pictured.stack = vec![source::Neighbour {
            number: 119,
            title: "feat(ui): comment on a line of a diff".into(),
            below: false,
        }];
        match state {
            "ready" => {
                pictured
                    .checks
                    .retain(|check| check.outcome == source::Outcome::Passed);
                pictured.decision = Some(Decision::Approved);
                pictured.reviewers.truncate(1);
            }
            "merged" => {
                pictured.state = State::Merged;
                pictured.merged_by = "grace".into();
                pictured.ended = Some(now - 2 * 86_400);
            }
            "draft" => pictured.state = State::Draft,
            "awaiting" => {
                pictured.checks = vec![Check {
                    name: "Build".into(),
                    workflow: "CI".into(),
                    outcome: Outcome::Awaiting,
                    seconds: None,
                    url: "https://github.com/zevem/neptune/actions/runs/77/job/1".into(),
                }];
                pictured.decision = None;
            }
            "conflicts" => pictured.merge = source::Merge::Conflicts,
            _ => {}
        }
        pictured
    }

    /// A real GPU/native capture of the pull request tab in isolated storage,
    /// with a pull request written here in place of GitHub.
    /// `NEPTUNE_PR_STATE` names what is shown: `summary` (the default),
    /// `ready`, `merged`, `draft`, `conflicts`, `checks`, `confirm`, `close`,
    /// `problem`, `timeline`, `code`, `diff`, `comment`, `review`, `reading`,
    /// `missing`, `signed-out`, `linked`, `empty` and `pictures` (a description
    /// with a picture of its host and one kept elsewhere). `NEPTUNE_PR_NARROW=1`
    /// uses a 640×400 window, `NEPTUNE_PR_WIDTH` sets the panel's width and
    /// `NEPTUNE_PR_THEME` names a theme. `NEPTUNE_PR_LIVE` names a pull
    /// request by its address and reads it with the person's GitHub CLI
    /// instead, pictures included. The desktop's pointer and keyboard are kept out of it:
    /// presses, typing and scrolling need a hand-driven native check.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "Manual native visual QA; needs a desktop and NEPTUNE_PR_CAPTURE"]
    fn capture_pull_request_native() {
        use ui::pull_request::{Composer, Confirm};
        use winit::platform::x11::EventLoopBuilderExtX11;
        let output = PathBuf::from(
            std::env::var("NEPTUNE_PR_CAPTURE").expect("Set a task-owned capture path"),
        );
        let state = std::env::var("NEPTUNE_PR_STATE").unwrap_or_default();
        let size = if std::env::var_os("NEPTUNE_PR_NARROW").is_some() {
            [640.0, 400.0]
        } else {
            [1000.0, 680.0]
        };
        let data = tempfile::tempdir().unwrap();
        let data_path = data.path().to_path_buf();
        let theme = std::env::var("NEPTUNE_PR_THEME").unwrap_or_else(|_| "graphite".into());
        std::fs::write(
            data_path.join("config.toml"),
            format!("shell = \"/bin/sh\"\ntheme = \"{theme}\"\n"),
        )
        .unwrap();
        let project = data_path.join("orbit");
        std::fs::create_dir_all(&project).unwrap();
        let options = eframe::NativeOptions {
            renderer: eframe::Renderer::Wgpu,
            viewport: egui::ViewportBuilder::default()
                .with_inner_size(size)
                .with_decorations(false),
            event_loop_builder: Some(Box::new(|builder| {
                builder.with_any_thread(true);
            })),
            ..Default::default()
        };
        struct NativeCapture {
            app: App,
            state: String,
            staged: bool,
            /// How far a driven run has come, and what it saw on the way.
            step: usize,
            seen: Arc<Mutex<Vec<String>>>,
            /// When the second driven run began pressing.
            began: Option<Instant>,
        }
        /// Where a press of the second driven run lands.
        #[derive(Clone)]
        enum Where {
            /// The middle of the control with this name.
            Control(egui::Id),
            /// A row of the menu that control opened, wherever the menu
            /// found room.
            Menu(egui::Id, usize),
            /// The same for a control known by where it is.
            MenuAt(Pos2, usize),
            At(Pos2),
        }
        #[derive(Clone)]
        enum Step {
            Click(Where),
            Type(&'static str),
            /// The command key with Enter.
            Send,
            /// Enter alone.
            Enter,
        }
        /// What the second driven run does, and when: it changes a label,
        /// takes a reaction back, answers and reopens a review conversation,
        /// writes a comment on a line of a diff, sends the review that
        /// carries it and closes the pull request's pill.
        fn presses() -> Vec<(u64, Step)> {
            use ui::pull_request as tab;
            let shown = pictured("press");
            let reacted = shown.entries[4].id.clone();
            let source::Kind::Thread { id: thread, .. } = &shown.entries[2].kind else {
                panic!("the third entry is a review conversation");
            };
            let labels = egui::Id::new(("pull-request-add", "Labels"));
            let reviewers = egui::Id::new(("pull-request-add", "Reviewers"));
            let words = |name: &str| egui::Id::new(("pull-request-words", name));
            let shown_as = words("Choose what the code part shows");
            let (timeline, code) = (Pos2::new(849.0, 194.0), Pos2::new(944.0, 194.0));
            let more = Pos2::new(978.0, 93.0);
            let control = |id| Step::Click(Where::Control(id));
            let steps = vec![
                // The title rewritten from the menu of the trailing control.
                Step::Click(Where::At(more)),
                Step::Click(Where::MenuAt(more, 1)),
                Step::Type(" v2"),
                Step::Enter,
                // A label from its list, and someone asked to review.
                control(labels),
                Step::Click(Where::Menu(labels, 0)),
                control(reviewers),
                Step::Click(Where::Menu(reviewers, 0)),
                // The description rewritten in place.
                control(words("Edit description")),
                Step::Type(" More."),
                Step::Send,
                Step::Click(Where::At(timeline)),
                // A reaction taken back and a comment rewritten.
                control(egui::Id::new((
                    "pull-request-reaction",
                    reacted.as_str(),
                    "Heart",
                ))),
                control(egui::Id::new(("pull-request-rewrite", reacted.as_str()))),
                Step::Type(" Again."),
                control(tab::rewrite_id().with("send")),
                // A review conversation answered and reopened.
                control(egui::Id::new(("pull-request-answer", thread.as_str()))),
                Step::Type("On it"),
                Step::Send,
                control(egui::Id::new(("pull-request-resolve", thread.as_str()))),
                // White space left out, a line of a diff commented on in
                // the diff, and the review that carries it sent.
                Step::Click(Where::At(code)),
                control(shown_as),
                Step::Click(Where::Menu(shown_as, 0)),
                // What was beside what is, and long lines wrapped.
                control(shown_as),
                Step::Click(Where::Menu(shown_as, 1)),
                control(shown_as),
                Step::Click(Where::Menu(shown_as, 2)),
                Step::Click(Where::At(Pos2::new(850.0, 303.0))),
                Step::Click(Where::At(Pos2::new(900.0, 532.0))),
                Step::Type("Why?"),
                Step::Send,
                control(egui::Id::new((
                    "pull-request-button",
                    "Send what was written",
                ))),
                control(egui::Id::new(("pull-request-pill-close", link(118).url()))),
                Step::Type(""),
            ];
            steps
                .into_iter()
                .enumerate()
                .map(|(place, step)| (300 + place as u64 * 550, step))
                .collect()
        }
        impl NativeCapture {
            /// The second driven run: each press is a pointer move, a press
            /// and a release on frames of their own, where the control is
            /// this frame.
            fn press(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
                use egui::{Event as Input, Key, Modifiers, PointerButton};
                let began = *self.began.get_or_insert_with(Instant::now);
                let elapsed = began.elapsed().as_millis() as u64;
                let script = presses();
                // The capture waits until everything was pressed.
                if self.step / 3 < script.len() {
                    self.app.started = Instant::now() - Duration::from_secs(1);
                }
                let Some((at, step)) = script.get(self.step / 3) else {
                    return;
                };
                let part = (self.step % 3) as u64;
                if elapsed < at + part * 90 {
                    return;
                }
                self.step += 1;
                match step {
                    Step::Click(target) => {
                        // A row of the menu that floats near `anchor`:
                        // below the control first, then above it.
                        let menu = |anchor: Pos2, row: usize| {
                            let floats = |pos: Pos2| {
                                ctx.layer_id_at(pos)
                                    .is_some_and(|layer| layer.order == egui::Order::Foreground)
                            };
                            let below = (4..240).step_by(4).map(|down| down as f32);
                            let above = (4..240).step_by(4).map(|up| -(up as f32));
                            below
                                .chain(above)
                                .flat_map(|dy| {
                                    [-150.0, -100.0, -60.0, 0.0, 60.0]
                                        .map(|dx| anchor + Vec2::new(dx, dy + dy.signum() * 10.0))
                                })
                                .find(|pos| floats(*pos))
                                .map(|edge| edge + Vec2::new(0.0, 12.0 + 28.0 * row as f32))
                        };
                        let found = match target {
                            Where::At(pos) => Some(*pos),
                            Where::Control(id) => {
                                ctx.read_response(*id).map(|found| found.rect.center())
                            }
                            Where::Menu(id, row) => ctx
                                .read_response(*id)
                                .and_then(|found| menu(found.rect.center(), *row)),
                            Where::MenuAt(anchor, row) => menu(*anchor, *row),
                        };
                        let Some(pos) = found else {
                            if part == 0 {
                                self.seen
                                    .lock()
                                    .unwrap()
                                    .push(format!("{at}: nothing to press"));
                            }
                            return;
                        };
                        input.events.push(match part {
                            0 => Input::PointerMoved(pos),
                            part => Input::PointerButton {
                                pos,
                                button: PointerButton::Primary,
                                pressed: part == 1,
                                modifiers: Modifiers::NONE,
                            },
                        });
                    }
                    Step::Type(text) if part == 0 && !text.is_empty() => {
                        input.events.push(Input::Text((*text).into()));
                    }
                    Step::Enter if part == 0 => {
                        let key = |pressed| Input::Key {
                            key: Key::Enter,
                            physical_key: None,
                            pressed,
                            repeat: false,
                            modifiers: Modifiers::NONE,
                        };
                        input.events.extend([key(true), key(false)]);
                    }
                    Step::Send if part == 0 => {
                        let command = Modifiers {
                            ctrl: true,
                            command: true,
                            ..Modifiers::NONE
                        };
                        let key = |pressed| Input::Key {
                            key: Key::Enter,
                            physical_key: None,
                            pressed,
                            repeat: false,
                            modifiers: command,
                        };
                        input.events.extend([
                            Input::ModifiersChanged(command),
                            key(true),
                            key(false),
                            Input::ModifiersChanged(Modifiers::NONE),
                        ]);
                    }
                    _ => {}
                }
            }
        }
        /// What a driven run presses and types, and when, in milliseconds
        /// from the launch: a number on the terminal's toolbar, the control
        /// that opens the comment field, a comment sent with the command
        /// key and Enter, then a second one left with Escape.
        fn script() -> Vec<(u64, Vec<egui::Event>)> {
            use egui::{Event as Input, Key, Modifiers, PointerButton, pos2};
            let command = Modifiers {
                ctrl: true,
                command: true,
                ..Modifiers::NONE
            };
            let press = |pos, pressed| Input::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            let key = |key, modifiers, pressed| Input::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            };
            let (number, compose) = (pos2(410.0, 22.0), pos2(968.0, 648.0));
            let click = |at: u64, pos| {
                vec![
                    (at, vec![Input::PointerMoved(pos)]),
                    (at + 100, vec![press(pos, true)]),
                    (at + 200, vec![press(pos, false)]),
                ]
            };
            let mut script = click(500, number);
            script.extend(click(1100, compose));
            script.push((1500, vec![Input::Text("Looks right".into())]));
            script.push((
                1700,
                vec![
                    Input::ModifiersChanged(command),
                    key(Key::Enter, command, true),
                    key(Key::Enter, command, false),
                    Input::ModifiersChanged(Modifiers::NONE),
                ],
            ));
            script.extend(click(2000, compose));
            script.push((2400, vec![Input::Text("Kept".into())]));
            script.push((
                2600,
                vec![
                    key(Key::Escape, Modifiers::NONE, true),
                    key(Key::Escape, Modifiers::NONE, false),
                ],
            ));
            script
        }
        impl eframe::App for NativeCapture {
            fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
                eframe::App::logic(&mut self.app, ctx, frame);
                let Some(pane) = self.app.controller.model().active_pane() else {
                    return;
                };
                if self.staged || self.app.sessions.get(pane).is_none() {
                    return;
                }
                self.staged = true;
                let live = std::env::var("NEPTUNE_PR_LIVE").ok();
                let shown = live
                    .as_deref()
                    .map_or_else(|| link(118), |url| PullRequest::parse(url).unwrap());
                // The terminal runs an agent that linked two pull requests.
                let generation = self.app.sessions.generation(pane).unwrap();
                let cwd = self.app.controller.model().pane(pane).unwrap().cwd().into();
                self.app.dispatch(
                    ctx,
                    Command::PaneAgentChanged {
                        pane,
                        generation,
                        agent: Some(neptune_model::AgentSession {
                            kind: neptune_model::AgentKind::Claude,
                            session_id: None,
                            cwd,
                        }),
                    },
                );
                if self.state != "empty" {
                    for pull_request in [link(112), shown.clone()] {
                        self.app.dispatch(
                            ctx,
                            Command::PanePullRequestLinked {
                                pane,
                                generation,
                                pull_request,
                            },
                        );
                    }
                }
                if let Ok(width) = std::env::var("NEPTUNE_PR_WIDTH") {
                    self.app.ui.panel.width = width.parse().unwrap();
                }
                if matches!(self.state.as_str(), "drive" | "hover") {
                    // The panel stays closed: a press on a number opens it.
                } else if matches!(self.state.as_str(), "linked" | "empty") {
                    self.app
                        .action(ctx, Action::Panel(ui::panel::Event::Show(Tab::PullRequest)));
                } else {
                    self.app
                        .action(ctx, Action::PullRequest(Event::Open(shown.clone())));
                }
                if self.state == "tabs" {
                    // A second one open beside it, the first in view again.
                    self.app
                        .action(ctx, Action::PullRequest(Event::Open(link(112))));
                    self.app
                        .action(ctx, Action::PullRequest(Event::Open(shown.clone())));
                }
                // Shown at rest, not on its way in.
                self.app.ui.panel.slide = None;
                let tab = &mut self.app.ui.pull_request;
                match self.state.as_str() {
                    "checks" => (tab.description, tab.checks) = (false, true),
                    "confirm" => tab.confirm = Some(Confirm::Merge(Method::Squash)),
                    "close" => tab.confirm = Some(Confirm::Close),
                    "timeline" => tab.segment = Segment::Timeline,
                    "code" | "diff" | "split" | "line" | "wrap" => tab.segment = Segment::Code,
                    "comment" => {
                        tab.composer = Some(Composer::Comment);
                        *tab.draft_mut(shown.url()) =
                            "Thanks, the Windows build passes here now.".into();
                    }
                    "review" => {
                        tab.composer = Some(Composer::Review);
                        tab.verdict = Verdict::Approved;
                    }
                    _ => {}
                }
                if matches!(self.state.as_str(), "diff" | "split" | "line" | "wrap") {
                    tab.selected = Some("src/ui/panel.rs".into());
                }
                tab.split = self.state == "split";
                tab.wrap = self.state == "wrap";
                if self.state == "line" {
                    tab.line = Some(ui::pull_request::Target {
                        path: "src/ui/panel.rs".into(),
                        line: 241,
                        removed: false,
                    });
                    tab.composer = Some(Composer::Line);
                    tab.note = "Does `PR` still fit beside a count?".into();
                }
                if self.state == "problem" {
                    self.app.pull_request.problem = Some((
                        "Could not merge this pull request",
                        "Pull request is not mergeable: the base branch requires all checks to pass."
                            .into(),
                    ));
                }
            }
            fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
                eframe::App::ui(&mut self.app, ui, frame);
            }
            fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
                eframe::App::raw_input_hook(&mut self.app, ctx, input);
                // The desktop's own pointer and keyboard have no part in the capture.
                input.events.retain(|event| {
                    !matches!(
                        event,
                        egui::Event::Key { .. }
                            | egui::Event::Text(_)
                            | egui::Event::Paste(_)
                            | egui::Event::PointerMoved(_)
                            | egui::Event::MouseMoved(_)
                            | egui::Event::PointerButton { .. }
                            | egui::Event::PointerGone
                            | egui::Event::MouseWheel { .. }
                    )
                });
                // The pointer rests on a linked number, for its card.
                if self.state == "hover" && self.staged && self.step == 0 {
                    self.step = 1;
                    input
                        .events
                        .push(egui::Event::PointerMoved(Pos2::new(410.0, 22.0)));
                }
                // An agent started for real is given time to come up.
                if self.state == "start" && self.staged {
                    let began = *self.began.get_or_insert_with(Instant::now);
                    if self.step == 0 {
                        self.step = 1;
                        let task = "Reply with the single word ready and do nothing else.";
                        self.app
                            .action(ctx, Action::PullRequest(Event::Start(task.into())));
                        self.seen
                            .lock()
                            .unwrap()
                            .push(format!("started: error={:?}", self.app.ui.error));
                    }
                    if began.elapsed() < Duration::from_secs(25) {
                        self.app.started = Instant::now() - Duration::from_secs(1);
                    }
                }
                if self.state == "press" && self.staged {
                    self.press(ctx, input);
                }
                if self.state != "drive" || !self.staged {
                    return;
                }
                // What the last step led to is noted before the next one.
                let elapsed = self.app.started.elapsed().as_millis() as u64;
                let script = script();
                while let Some((at, events)) = script.get(self.step) {
                    if elapsed < *at {
                        break;
                    }
                    let tab = &self.app.ui.pull_request;
                    let composer = ui::pull_request::composer_id();
                    self.seen.lock().unwrap().push(format!(
                        "{at}: open={} tab={:?} shown={:?} composer={:?} draft={:?} typing={}",
                        self.app.ui.panel.open,
                        self.app.ui.panel.tab,
                        self.app.pull_request.shown().map(PullRequest::number),
                        tab.composer,
                        tab.draft(link(118).url()),
                        ctx.memory(|memory| memory.has_focus(composer)),
                    ));
                    input.events.extend(events.iter().cloned());
                    self.step += 1;
                }
            }
            fn on_exit(&mut self) {
                eframe::App::on_exit(&mut self.app);
                if self.state == "start" {
                    let model = self.app.controller.model();
                    let front = model.active_pane();
                    let said = front
                        .and_then(|pane| self.app.sessions.get(pane))
                        .map(|session| session.metadata().title);
                    self.seen.lock().unwrap().push(format!(
                        "agent: panes={} front_runs={:?} activity={:?} said={said:?}",
                        model
                            .workspaces()
                            .iter()
                            .map(|w| w.panes().len())
                            .sum::<usize>(),
                        front
                            .and_then(|pane| model.pane(pane))
                            .and_then(|pane| pane.agent().map(|agent| agent.kind)),
                        front
                            .and_then(|pane| self.app.agents.get(pane))
                            .map(|found| found.0),
                    ));
                }
                self.seen.lock().unwrap().push(format!(
                    "closed: shown={:?} plain={} split={} wrap={}",
                    self.app.pull_request.shown().map(PullRequest::number),
                    self.app.ui.pull_request.hide_whitespace,
                    self.app.ui.pull_request.split,
                    self.app.ui.pull_request.wrap,
                ));
                let tab = &self.app.ui.pull_request;
                self.seen.lock().unwrap().push(format!(
                    "end: composer={:?} draft={:?} typing={}",
                    tab.composer,
                    tab.draft(link(118).url()),
                    false,
                ));
            }
        }
        let seen = Arc::new(Mutex::new(Vec::new()));
        let asked = Arc::new(Mutex::new(Vec::<Act>::new()));
        let (noted, done) = (seen.clone(), asked.clone());
        let pictured_state = state.clone();
        eframe::run_native(
            "Neptune pull request visual QA",
            options,
            Box::new(move |cc| {
                let mut app = App::new(
                    cc,
                    Launch {
                        cwd: Some(project),
                        data_root: Some(data_path),
                        screenshot: Some(output),
                        ..Default::default()
                    },
                    window_state::LoadReport::default(),
                );
                app.file_drag = crate::platform::file_drag::FileDragSource::detached();
                if std::env::var_os("NEPTUNE_PR_LIVE").is_some() {
                    app.pull_request.source = Some(Source {
                        detail: Box::new(source::read),
                        files: Box::new(source::read_files),
                        choices: Box::new(source::read_choices),
                        act: Box::new(|_, _, _| Err("A capture changes nothing.".into())),
                    });
                    app.pull_request.pictures.fetch = Some(Box::new(fetch_picture));
                    app.pull_requests = Default::default();
                } else {
                    use crate::runtime::pull_requests::{Checks, Status, Watcher};
                    app.pull_requests = Watcher::with(Box::new(|_, links| {
                        links
                            .iter()
                            .map(|link| {
                                let preview = crate::runtime::pull_requests::Preview {
                                    title: format!("Pull request {}", link.number()),
                                    author: "ada".into(),
                                    opened: std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map_or(0, |since| since.as_secs() as i64)
                                        - 26 * 3600,
                                };
                                let status = if link.number() == 112 {
                                    Status {
                                        state: State::Merged,
                                        checks: Checks::Passing,
                                        unresolved: 0,
                                    }
                                } else {
                                    Status {
                                        state: State::Open,
                                        checks: Checks::Failing,
                                        unresolved: 1,
                                    }
                                };
                                Some((status, preview))
                            })
                            .collect()
                    }));
                    let state = pictured_state.clone();
                    let driven = matches!(state.as_str(), "drive" | "press");
                    app.pull_request.source = Some(Source {
                        detail: Box::new(move |_| match state.as_str() {
                            "reading" => {
                                std::thread::sleep(Duration::from_secs(30));
                                Err(Failure::Unavailable)
                            }
                            "missing" => Err(Failure::NotFound),
                            "signed-out" => Err(Failure::SignedOut),
                            "pictures" => {
                                let mut detail = pictured("summary");
                                detail.body = "A badge kept somewhere else is not fetched:\n\n![Build status](https://ci.example/badge.png)\n\nThe tab, from the running app:\n\n<img alt=\"The tab in the window\" src=\"https://github.com/user-attachments/assets/4e0618da\" />\n\nAnd what follows it.".into();
                                Ok(detail)
                            }
                            state => Ok(pictured(state)),
                        }),
                        files: Box::new(|_, _| {
                            let file = |path: &str, change, added, removed, patch: Option<&str>| {
                                ChangedFile {
                                    path: path.into(),
                                    from: None,
                                    change,
                                    added,
                                    removed,
                                    patch: patch.map(str::to_owned),
                                }
                            };
                            Ok(vec![
                                file("docs/design.md", Change::Modified, 12, 3, Some("@@ -1 +1 @@\n-a\n+b\n")),
                                file("src/app/pull_request.rs", Change::Added, 640, 0, Some("@@ -0,0 +1 @@\n+//! The pull request the panel's tab shows.\n")),
                                file(
                                    "src/ui/panel.rs",
                                    Change::Modified,
                                    41,
                                    9,
                                    Some("@@ -238,9 +238,15 @@ pub fn show(\n     child.multiply_opacity(view.reveal);\n-    let tabs = [Tab::Files, Tab::Agents, Tab::Changes, Tab::Project];\n+    let tabs = [\n+        Tab::Files,\n+        Tab::Agents,\n+        Tab::Changes,\n+        Tab::Project,\n+        Tab::PullRequest,\n+    ];\n     let waiting = |item| match item {\n         Tab::Agents => view.waiting,\n"),
                                ),
                                file("src/ui/pull_request.rs", Change::Added, 2140, 0, None),
                                file("assets/tab.png", Change::Added, 0, 0, None),
                            ])
                        }),
                        choices: Box::new(|_| {
                            Ok(Choices {
                                labels: vec!["ui".into(), "right panel".into(), "bug".into()],
                                people: vec!["grace".into(), "linus".into()],
                            })
                        }),
                        act: Box::new(move |_, _, act| {
                            // A driven run has its stand-in do what is asked.
                            if driven {
                                done.lock().unwrap().push(act.clone());
                                Ok(())
                            } else {
                                Err("A capture changes nothing.".into())
                            }
                        }),
                    });
                }
                // A picture drawn here stands in for one read from the host.
                if app.pull_request.pictures.fetch.is_none() {
                    app.pull_request.pictures.fetch = Some(Box::new(|_, _| {
                        let size = [960, 300];
                        let mut picture = egui::ColorImage::filled(size, egui::Color32::BLACK);
                        for (at, pixel) in picture.pixels.iter_mut().enumerate() {
                            let (x, y) = (at % size[0], at / size[0]);
                            *pixel = egui::Color32::from_rgb(
                                (40 + x * 120 / size[0]) as u8,
                                (60 + y * 120 / size[1]) as u8,
                                160,
                            );
                        }
                        Some(([960, 300], picture))
                    }));
                }
                Ok(Box::new(NativeCapture {
                    app,
                    state: pictured_state,
                    staged: false,
                    step: 0,
                    seen: noted,
                    began: None,
                }))
            }),
        )
        .unwrap();
        if state == "start" {
            println!("{:#?}", seen.lock().unwrap());
            return;
        }
        if state == "press" {
            let asked = asked.lock().unwrap();
            println!("{asked:#?}\n{:?}", seen.lock().unwrap());
            assert!(
                seen.lock()
                    .unwrap()
                    .iter()
                    .all(|line| !line.contains("nothing to press"))
            );
            let [
                retitled,
                label,
                request,
                describe,
                react,
                rewrite,
                reply,
                resolve,
                review,
            ] = &asked[..]
            else {
                panic!("nine things were asked of the pull request: {asked:?}");
            };
            assert!(
                matches!(retitled, Act::Title(title) if title.ends_with(" v2")),
                "{retitled:?}"
            );
            assert!(matches!(label, Act::Label { .. }), "{label:?}");
            assert!(matches!(request, Act::Request { .. }), "{request:?}");
            assert!(
                matches!(describe, Act::Description(body) if body.ends_with("More.")),
                "{describe:?}"
            );
            assert!(
                matches!(rewrite, Act::Edit { body, said: source::Said::Comment, .. } if body.ends_with("Again.")),
                "{rewrite:?}"
            );
            assert!(
                matches!(
                    react,
                    Act::React {
                        emoji: source::Emoji::Heart,
                        on: false,
                        ..
                    }
                ),
                "{react:?}"
            );
            assert!(
                matches!(reply, Act::Reply { body, .. } if body == "On it"),
                "{reply:?}"
            );
            assert!(
                matches!(
                    resolve,
                    Act::Resolve {
                        resolved: false,
                        ..
                    }
                ),
                "{resolve:?}"
            );
            let Act::Review(Verdict::Commented, body, lines) = review else {
                panic!("the last thing asked was a review: {review:?}");
            };
            assert!(body.is_empty());
            assert!(
                matches!(&lines[..], [line] if line.path == "src/ui/panel.rs" && line.body == "Why?"),
                "{lines:?}"
            );
            // Its pill closed, the tab lists the linked pull requests again.
            assert!(
                seen.lock()
                    .unwrap()
                    .iter()
                    .any(|line| line.contains("closed: shown=None plain=true split=true wrap=true"))
            );
            return;
        }
        if state != "drive" {
            return;
        }
        let seen = seen.lock().unwrap();
        println!("{}", seen.join("\n"));
        let at = |step: &str| {
            seen.iter()
                .find(|line| line.starts_with(step))
                .unwrap_or_else(|| panic!("the run did not reach {step}"))
                .clone()
        };
        // The pressed number opened the panel on its pull request.
        assert!(at("500:").contains("open=false"), "{seen:?}");
        let opened = at("1100:");
        assert!(
            opened.contains("open=true tab=PullRequest shown=Some(118) composer=None"),
            "{opened}"
        );
        // The round control opened the field and gave it the keyboard.
        assert!(at("1500:").contains("composer=Some(Comment)"), "{seen:?}");
        let typed = at("1700:");
        assert!(
            typed.contains("draft=\"Looks right\" typing=true"),
            "{typed}"
        );
        // The command key with Enter sent it: the field is put away empty.
        assert_eq!(*asked.lock().unwrap(), [Act::Comment("Looks right".into())]);
        assert!(at("2000:").contains("composer=None draft=\"\""), "{seen:?}");
        // Escape puts the field away and keeps what was written.
        assert!(at("2600:").contains("composer=Some(Comment) draft=\"Kept\" typing=true"));
        assert!(
            at("end:").contains("composer=None draft=\"Kept\""),
            "{seen:?}"
        );
    }
}
