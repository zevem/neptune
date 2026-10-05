//! What git says about the folders in view: each workspace's branch for the
//! sidebar, and the changed files and one diff for the changes tab. A worker
//! asks git on a timer and reports only what changed, so an unchanged
//! repository wakes no frame. Paths and file contents stay out of diagnostics
//! and saved state.
use super::git::{self, Failure, Listing, Summary};
use super::*;
use crate::ui::changes::{DiffBody, Event, Repository, Scope};
use std::{collections::HashMap, path::Path};

/// How often the listed repository and the shown diff are read again.
const DETAIL_INTERVAL: Duration = if cfg!(test) {
    Duration::from_millis(60)
} else {
    Duration::from_secs(2)
};
/// How often the sidebar's branches are read again.
const SUMMARY_INTERVAL: Duration = if cfg!(test) {
    Duration::from_millis(60)
} else {
    Duration::from_secs(5)
};
/// A slow repository is asked less often: git gets at most this share of
/// the worker's time.
const WORK_SHARE: u32 = 10;
/// A window in the background is read this many times less often.
const BACKGROUND: u32 = 5;
/// The most folders whose branch is followed.
const MAX_FOLDERS: usize = 32;

/// The repository the tab lists, what it is compared with and the file whose
/// diff is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Detail {
    dir: PathBuf,
    scope: Scope,
    file: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Watch {
    /// Folders whose branch the sidebar shows.
    folders: Vec<PathBuf>,
    detail: Option<Detail>,
    /// Another window is the active one: what is watched may be in view
    /// still, and is read less often.
    background: bool,
}

enum Request {
    Watch(Watch),
    /// Read everything watched again now, and report it whether it changed.
    Refresh,
}

enum Reply {
    Summary {
        dir: PathBuf,
        summary: Option<Summary>,
    },
    Listed {
        dir: PathBuf,
        scope: Scope,
        listing: Result<Listing, Failure>,
    },
    Diff {
        dir: PathBuf,
        scope: Scope,
        path: String,
        body: DiffBody,
    },
}

type Listed = ((PathBuf, Scope), Result<Listing, Failure>);

pub(super) struct Changes {
    replies: (mpsc::Sender<Reply>, mpsc::Receiver<Reply>),
    worker: Option<mpsc::Sender<Request>>,
    /// What the worker was last told to watch; nothing while the window is
    /// minimized.
    sent: Watch,
    summaries: HashMap<PathBuf, Option<Summary>>,
    /// What the tab shows, whether or not it is being read.
    detail: Option<Detail>,
    /// Why there is no folder to ask git about.
    notice: &'static str,
    listed: Option<Listed>,
    /// The diff of a file, by what it was compared with and its path.
    diff: Option<((Scope, String), DiffBody)>,
}

impl Default for Changes {
    fn default() -> Self {
        Self {
            replies: mpsc::channel(),
            worker: None,
            sent: Watch::default(),
            summaries: HashMap::new(),
            detail: None,
            notice: "",
            listed: None,
            diff: None,
        }
    }
}

/// Asks git about what is watched: at once for anything new, then on the
/// timers. Requests made while git ran are taken together, newest last.
fn worker(requests: mpsc::Receiver<Request>, replies: mpsc::Sender<Reply>, wake: egui::Context) {
    let mut watch = Watch::default();
    let mut summaries: HashMap<PathBuf, Option<Summary>> = HashMap::new();
    let mut listed: Option<Listed> = None;
    let mut diffed: Option<(String, DiffBody)> = None;
    let mut next_summaries = Instant::now();
    let mut next_detail = Instant::now();
    loop {
        let due = [
            (!watch.folders.is_empty()).then_some(next_summaries),
            watch.detail.is_some().then_some(next_detail),
        ]
        .into_iter()
        .flatten()
        .min();
        let mut request = match due {
            None => requests
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            Some(due) => requests.recv_timeout(due.saturating_duration_since(Instant::now())),
        };
        loop {
            match request {
                Ok(Request::Watch(new)) => {
                    summaries.retain(|dir, _| new.folders.contains(dir));
                    if new.detail != watch.detail {
                        let key = |detail: &Option<Detail>| {
                            detail
                                .as_ref()
                                .map(|detail| (detail.dir.clone(), detail.scope))
                        };
                        if key(&new.detail) != key(&watch.detail) {
                            listed = None;
                        }
                        diffed = None;
                        next_detail = Instant::now();
                    }
                    // A window that returns to the front is brought up to date.
                    if watch.background && !new.background {
                        next_summaries = Instant::now();
                        next_detail = Instant::now();
                    }
                    watch = new;
                }
                Ok(Request::Refresh) => {
                    summaries.clear();
                    listed = None;
                    diffed = None;
                    next_summaries = Instant::now();
                    next_detail = Instant::now();
                }
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
        let slower = if watch.background { BACKGROUND } else { 1 };
        let started = Instant::now();
        let every = started >= next_summaries;
        for dir in &watch.folders {
            // Between timers, only a folder not asked about yet is read.
            if !every && summaries.contains_key(dir) {
                continue;
            }
            let summary = git::summary(dir);
            if summaries.get(dir) != Some(&summary) {
                summaries.insert(dir.clone(), summary.clone());
                reports.push(Reply::Summary {
                    dir: dir.clone(),
                    summary,
                });
            }
        }
        if every {
            next_summaries =
                Instant::now() + (SUMMARY_INTERVAL * slower).max(started.elapsed() * WORK_SHARE);
        }
        if let Some(detail) = &watch.detail
            && Instant::now() >= next_detail
        {
            let started = Instant::now();
            let key = (detail.dir.clone(), detail.scope);
            let listing = git::listing(&detail.dir, detail.scope);
            let file = detail.file.as_ref().and_then(|path| {
                let listing = listing.as_ref().ok()?;
                let file = listing.files.iter().find(|file| file.path == *path)?;
                Some((path, git::diff(&listing.root, detail.scope, file)))
            });
            if listed
                .as_ref()
                .is_none_or(|(known, was)| *known != key || *was != listing)
            {
                listed = Some((key.clone(), listing.clone()));
                reports.push(Reply::Listed {
                    dir: key.0.clone(),
                    scope: key.1,
                    listing,
                });
            }
            if let Some((path, body)) = file
                && diffed
                    .as_ref()
                    .is_none_or(|(known, was)| known != path || *was != body)
            {
                diffed = Some((path.clone(), body.clone()));
                reports.push(Reply::Diff {
                    dir: key.0,
                    scope: key.1,
                    path: path.clone(),
                    body,
                });
            }
            next_detail =
                Instant::now() + (DETAIL_INTERVAL * slower).max(started.elapsed() * WORK_SHARE);
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

impl Changes {
    fn request(&mut self, ctx: &egui::Context, request: Request) -> bool {
        if self.worker.is_none() {
            let (sender, requests) = mpsc::channel();
            let replies = self.replies.0.clone();
            let wake = ctx.clone();
            let spawned = std::thread::Builder::new()
                .name("neptune-changes".into())
                .spawn(move || worker(requests, replies, wake));
            if spawned.is_err() {
                return false;
            }
            self.worker = Some(sender);
        }
        self.worker
            .as_ref()
            .is_some_and(|worker| worker.send(request).is_ok())
    }

    /// The listing of what the tab shows, once it has been read.
    fn listing(&self) -> Option<&Result<Listing, Failure>> {
        let detail = self.detail.as_ref()?;
        self.listed
            .as_ref()
            .filter(|((dir, scope), _)| *dir == detail.dir && *scope == detail.scope)
            .map(|(_, listing)| listing)
    }

    /// The branch of the repository `dir` is in, and whether it has changes
    /// that are not committed.
    pub(super) fn branch(&self, dir: &Path) -> Option<ui::Branch> {
        let summary = self.summaries.get(dir)?.as_ref()?;
        Some(ui::Branch {
            name: summary.branch.clone(),
            dirty: summary.dirty,
        })
    }

    pub(super) fn view<'a>(
        &'a self,
        (scope, selected): (Scope, Option<&str>),
        reveal: f32,
        window: Rect,
    ) -> ui::changes::View<'a> {
        static LOADING: DiffBody = DiffBody::Loading;
        let (repository, notice) = match self.listing() {
            // Nothing is said while git is first asked.
            None => (None, self.notice),
            Some(Err(Failure::Missing)) => (
                None,
                "Git was not found. Install Git to see what changed here.",
            ),
            Some(Err(Failure::NotRepository)) => (None, "This folder is not in a Git repository."),
            Some(Err(Failure::Other(message))) => (None, message.as_str()),
            Some(Ok(listing)) => {
                let diff = selected
                    .and_then(|path| listing.files.iter().find(|file| file.path == path))
                    .map(|file| {
                        let body = self
                            .diff
                            .as_ref()
                            .filter(|((of, path), _)| *of == scope && *path == file.path)
                            .map_or(&LOADING, |(_, body)| body);
                        (file, body)
                    });
                let repository = Repository {
                    root: &listing.root,
                    branch: &listing.summary.branch,
                    detached: listing.summary.detached,
                    ahead: listing.summary.ahead,
                    behind: listing.summary.behind,
                    base: listing.base.as_deref(),
                    problem: listing.no_base.then_some(
                        "There is no main branch to compare with. Neptune looks for origin's default branch, then main and master.",
                    ),
                    files: &listing.files,
                    more: listing.more,
                    diff,
                };
                (Some(repository), "")
            }
        };
        ui::changes::View {
            repository,
            notice,
            reveal,
            window,
        }
    }
}

impl App {
    /// Whether the tab's files have been read, for a test that waits for them.
    #[cfg(test)]
    pub(super) fn changes_listed(&self) -> Option<Vec<String>> {
        let listing = self.changes.listing()?.as_ref().ok()?;
        Some(listing.files.iter().map(|file| file.path.clone()).collect())
    }

    /// What the worker reported, taken whether or not anything shows it.
    pub(super) fn poll_changes(&mut self, ctx: &egui::Context) {
        // A minimized window draws no frame that could say so.
        if ctx.input(|input| input.viewport().minimized == Some(true))
            && self.changes.sent != Watch::default()
        {
            self.changes.sent = Watch::default();
            self.changes.request(ctx, Request::Watch(Watch::default()));
        }
        while let Ok(reply) = self.changes.replies.1.try_recv() {
            match reply {
                Reply::Summary { dir, summary } => {
                    if self.changes.sent.folders.contains(&dir) {
                        self.changes.summaries.insert(dir, summary);
                    }
                }
                Reply::Listed {
                    dir,
                    scope,
                    listing,
                } => {
                    // The tab and the sidebar say the same of one folder.
                    if let (Ok(listing), Some(summary)) =
                        (&listing, self.changes.summaries.get_mut(&dir))
                    {
                        *summary = Some(listing.summary.clone());
                    }
                    self.changes.listed = Some(((dir, scope), listing));
                    // A file that is no longer listed has no diff to show.
                    if let Some(listing) = self.changes.listing()
                        && let Some(selected) = &self.ui.changes.selected
                        && !listing.as_ref().is_ok_and(|listing| {
                            listing.files.iter().any(|file| file.path == *selected)
                        })
                    {
                        self.ui.changes.selected = None;
                        self.changes.diff = None;
                    }
                }
                Reply::Diff {
                    dir,
                    scope,
                    path,
                    body,
                } => {
                    if self.changes.detail.as_ref().is_some_and(|detail| {
                        detail.dir == dir && detail.file.as_ref() == Some(&path)
                    }) {
                        self.changes.diff = Some(((scope, path), body));
                    }
                }
            }
        }
    }

    /// Tells the worker what this frame shows: the sidebar's folders and the
    /// tab's repository. They are read less often while another window is the
    /// active one, and not at all while this one is minimized.
    pub(super) fn sync_changes(&mut self, ctx: &egui::Context, sidebar: bool, tab: bool) {
        let model = self.controller.model();
        let folder = |workspace: &neptune_model::Workspace| {
            PathBuf::from(
                workspace
                    .pane(workspace.active())
                    .map_or(workspace.cwd(), |pane| pane.cwd()),
            )
        };
        let mut folders = Vec::new();
        if sidebar {
            for workspace in model.workspaces() {
                let dir = folder(workspace);
                if workspace.remote().is_none()
                    && folders.len() < MAX_FOLDERS
                    && !folders.contains(&dir)
                {
                    folders.push(dir);
                }
            }
        }
        let workspace = model.active_workspace().and_then(|id| model.workspace(id));
        let (dir, notice) = match workspace {
            None => (None, "Open a workspace to see what changed in its folder."),
            Some(workspace) if workspace.remote().is_some() => (
                None,
                "This workspace is connected over SSH. Changes are shown for folders on this computer only.",
            ),
            Some(workspace) => (Some(folder(workspace)), ""),
        };
        self.changes.notice = notice;
        // Another folder's file is not this one's.
        if self
            .changes
            .detail
            .as_ref()
            .is_some_and(|detail| Some(&detail.dir) != dir.as_ref())
        {
            self.ui.changes.selected = None;
            self.changes.diff = None;
        }
        self.changes.detail = dir.map(|dir| Detail {
            dir,
            scope: self.ui.changes.scope,
            file: self.ui.changes.selected.clone(),
        });
        self.changes
            .summaries
            .retain(|dir, _| folders.contains(dir));
        let watch = if ctx.input(|input| input.viewport().minimized == Some(true)) {
            Watch::default()
        } else {
            Watch {
                folders,
                detail: self.changes.detail.clone().filter(|_| tab),
                background: self.window_focused == Some(false),
            }
        };
        if watch != self.changes.sent {
            self.changes.sent = watch.clone();
            self.changes.request(ctx, Request::Watch(watch));
        }
    }

    pub(super) fn changes_event(&mut self, ctx: &egui::Context, event: Event) {
        match event {
            Event::Scope(scope) => self.ui.changes.scope = scope,
            Event::Select(path) => self.ui.changes.selected = Some(path),
            Event::CloseDiff => self.ui.changes.selected = None,
            Event::Refresh => {
                self.changes.request(ctx, Request::Refresh);
            }
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{
        Action,
        changes::{LineKind, Status},
        panel::{Event as Panel, Tab},
    };
    use eframe::egui::Vec2;

    const WINDOW: Vec2 = Vec2::new(900.0, 640.0);

    fn frame(app: &mut App, ctx: &egui::Context) {
        let mut host = eframe::Frame::_new_kittest();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, WINDOW)),
                ..Default::default()
            },
            |ui| {
                app.poll_changes(ui.ctx());
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

    fn opened(root: &Path) -> (App, egui::Context) {
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
        (app, ctx)
    }

    fn shown(app: &App) -> ui::changes::View<'_> {
        let state = &app.ui.changes;
        app.changes
            .view((state.scope, state.selected.as_deref()), 1.0, Rect::NOTHING)
    }

    fn branch(app: &App, dir: &Path) -> Option<(String, bool)> {
        app.changes
            .branch(dir)
            .map(|branch| (branch.name, branch.dirty))
    }

    #[test]
    fn the_sidebar_follows_each_folders_branch_and_the_tab_lists_and_diffs_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git::tests::repository(root);
        let (mut app, ctx) = opened(root);
        // The sidebar alone reads the branch, not the files.
        settle(&mut app, &ctx, "the branch", |app| {
            branch(app, root) == Some(("main".into(), false))
        });
        assert!(app.changes.sent.detail.is_none() && app.changes.listed.is_none());

        git::tests::run(root, &["checkout", "--quiet", "-b", "feat/review"]);
        std::fs::write(root.join("kept.txt"), "one\n2\nthree\n").unwrap();
        settle(&mut app, &ctx, "the new branch and its change", |app| {
            branch(app, root) == Some(("feat/review".into(), true))
        });

        app.action(&ctx, Action::Panel(Panel::Show(Tab::Changes)));
        settle(&mut app, &ctx, "the changed files", |app| {
            app.changes_listed() == Some(vec!["kept.txt".into()])
        });
        let view = shown(&app);
        let repository = view.repository.expect("a repository");
        assert_eq!(repository.branch, "feat/review");
        assert_eq!(repository.files[0].status, Status::Modified);
        assert!(repository.diff.is_none() && repository.base.is_none());

        app.action(&ctx, Action::Changes(Event::Select("kept.txt".into())));
        let lines = |app: &App| -> Option<Vec<(LineKind, String)>> {
            let view = shown(app);
            match view.repository?.diff? {
                (_, DiffBody::Lines { lines, .. }) => Some(
                    lines
                        .iter()
                        .filter(|line| matches!(line.kind, LineKind::Added | LineKind::Removed))
                        .map(|line| (line.kind, line.text.clone()))
                        .collect(),
                ),
                _ => None,
            }
        };
        settle(&mut app, &ctx, "the diff", |app| {
            lines(app)
                == Some(vec![
                    (LineKind::Removed, "two".into()),
                    (LineKind::Added, "2".into()),
                ])
        });
        // The diff follows the file as it is edited.
        std::fs::write(root.join("kept.txt"), "one\n2\nthree\nfour\n").unwrap();
        settle(&mut app, &ctx, "the edited diff", |app| {
            lines(app).is_some_and(|lines| lines.len() == 3)
        });

        // Committed, the work leaves the working tree and stays on the branch.
        git::tests::run(root, &["commit", "--quiet", "-am", "second"]);
        settle(&mut app, &ctx, "a clean working tree", |app| {
            app.changes_listed() == Some(Vec::new()) && branch(app, root).is_some_and(|b| !b.1)
        });
        assert_eq!(app.ui.changes.selected, None, "its diff closed with it");
        app.action(&ctx, Action::Changes(Event::Scope(Scope::Branch)));
        settle(&mut app, &ctx, "the branch's files", |app| {
            app.changes_listed() == Some(vec!["kept.txt".into()])
        });
        let view = shown(&app);
        assert_eq!(view.repository.unwrap().base, Some("main"));

        // Out of view, the repository is no longer read.
        app.action(&ctx, Action::Panel(Panel::Show(Tab::Agents)));
        frame(&mut app, &ctx);
        assert!(app.changes.sent.detail.is_none());
        assert_eq!(app.changes.sent.folders, [root.to_path_buf()]);
        // In the background the branch is still followed, less often.
        app.window_focused = Some(false);
        frame(&mut app, &ctx);
        assert!(app.changes.sent.background);
        git::tests::run(root, &["checkout", "--quiet", "main"]);
        settle(&mut app, &ctx, "the branch checked out meanwhile", |app| {
            branch(app, root) == Some(("main".into(), false))
        });
    }

    #[test]
    fn folders_without_a_repository_and_remote_workspaces_say_why_nothing_is_listed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        if git::listing(root, Scope::WorkingTree) != Err(Failure::NotRepository) {
            // The temporary folder is inside a repository, or git is missing.
            return;
        }
        let (mut app, ctx) = opened(root);
        app.action(&ctx, Action::Panel(Panel::Show(Tab::Changes)));
        settle(&mut app, &ctx, "git's answer", |app| {
            app.changes.listing().is_some()
        });
        let view = shown(&app);
        assert!(view.repository.is_none());
        assert_eq!(view.notice, "This folder is not in a Git repository.");
        assert_eq!(branch(&app, root), None);

        app.controller
            .dispatch(Command::AddWorkspace {
                group: None,
                cwd: root.into(),
                name: "Remote".into(),
                remote: Some("me@devbox".into()),
            })
            .unwrap();
        frame(&mut app, &ctx);
        let view = shown(&app);
        assert!(view.repository.is_none() && view.notice.contains("SSH"));
        assert!(app.changes.sent.detail.is_none());
    }
}
