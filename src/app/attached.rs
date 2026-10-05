//! The files agents attached to their terminals, as their tabs list them.
//! Each is looked at once on a worker, for a small copy of a picture; paths
//! and pictures stay out of diagnostics.
use super::*;
use std::{collections::HashMap, path::Path};

/// What a worker found where an attached file should be.
enum Found {
    Picture {
        pixels: [u32; 2],
        image: egui::ColorImage,
    },
    Other,
    Missing,
}

enum Seen {
    /// A worker is looking.
    Reading,
    Picture {
        /// Pixels of the file itself.
        pixels: [u32; 2],
        texture: egui::TextureHandle,
    },
    Other,
    Missing,
}

/// An attached picture, for the full view.
pub(super) struct Picture {
    pub(super) path: PathBuf,
    pub(super) label: String,
    pub(super) pixels: [u32; 2],
    pub(super) texture: egui::TextureHandle,
}

pub(super) struct Attached {
    seen: HashMap<PathBuf, Seen>,
    sender: mpsc::Sender<(PathBuf, Found)>,
    receiver: mpsc::Receiver<(PathBuf, Found)>,
    /// One file is read at a time.
    reading: bool,
}

impl Default for Attached {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            seen: HashMap::new(),
            sender,
            receiver,
            reading: false,
        }
    }
}

impl Attached {
    /// A file attached again may have changed: it is looked at afresh.
    pub(super) fn forget(&mut self, path: &Path) {
        self.seen.remove(path);
    }

    fn kind(&self, path: &Path) -> ui::attached::Kind {
        use ui::attached::Kind;
        match self.seen.get(path) {
            None | Some(Seen::Reading) => Kind::Unread,
            Some(Seen::Picture { texture, .. }) => Kind::Picture(texture.clone()),
            Some(Seen::Other) => Kind::Other,
            Some(Seen::Missing) => Kind::Missing,
        }
    }
}

fn look(path: &Path, limit: [u32; 2]) -> Found {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => match image_preview::decode(path, limit) {
            Some((pixels, image)) => Found::Picture { pixels, image },
            None => Found::Other,
        },
        _ => Found::Missing,
    }
}

impl App {
    /// Takes what the worker found and sends it to the next file of the
    /// workspace in view that has not been looked at.
    pub(super) fn poll_attached(&mut self, ctx: &egui::Context) {
        let mut found = false;
        while let Ok((path, file)) = self.attached.receiver.try_recv() {
            self.attached.reading = false;
            found = true;
            // Forgotten meanwhile: attached again, or no longer listed.
            if !matches!(self.attached.seen.get(&path), Some(Seen::Reading)) {
                continue;
            }
            let seen = match file {
                Found::Picture { pixels, image } => Seen::Picture {
                    pixels,
                    texture: ctx.load_texture("attached-file", image, egui::TextureOptions::LINEAR),
                },
                Found::Other => Seen::Other,
                Found::Missing => Seen::Missing,
            };
            self.attached.seen.insert(path, seen);
        }
        let model = self.controller.model();
        if found {
            // Small copies are kept only for files still on some list.
            let listed: std::collections::HashSet<&Path> = model
                .workspaces()
                .iter()
                .flat_map(|workspace| workspace.panes())
                .flat_map(|pane| pane.attachments())
                .map(|file| file.path())
                .collect();
            self.attached
                .seen
                .retain(|path, _| listed.contains(path.as_path()));
        }
        if self.attached.reading {
            return;
        }
        let next = model
            .active_workspace()
            .and_then(|id| model.workspace(id))
            .into_iter()
            .flat_map(|workspace| workspace.panes())
            .flat_map(|pane| pane.attachments().iter().rev())
            .find(|file| !self.attached.seen.contains_key(file.path()));
        let Some(path) = next.map(|file| file.path().to_path_buf()) else {
            return;
        };
        // Twice the size it is drawn at stays sharp where a row is scaled.
        let side = (ui::attached::THUMB * ctx.pixels_per_point() * 2.0).ceil() as u32;
        let sender = self.attached.sender.clone();
        let wake = ctx.clone();
        let target = path.clone();
        let spawned = std::thread::Builder::new()
            .name("neptune-attached-file".into())
            .spawn(move || {
                let found = look(&target, [side, side]);
                let _ = sender.send((target, found));
                wake.request_repaint();
            });
        // Without a worker the file is listed by its name alone.
        self.attached.reading = spawned.is_ok();
        self.attached.seen.insert(
            path,
            if spawned.is_ok() {
                Seen::Reading
            } else {
                Seen::Other
            },
        );
    }

    /// A terminal's attached files, oldest first, for its tab's list.
    pub(super) fn attached_files(&self, pane: &neptune_model::Pane) -> Vec<ui::attached::File> {
        pane.attachments()
            .iter()
            .map(|file| ui::attached::File::new(file, self.attached.kind(file.path())))
            .collect()
    }

    /// The pictures among a terminal's attached files, newest first as its
    /// list shows them.
    pub(super) fn attached_pictures(&self, pane: PaneId) -> Vec<Picture> {
        let Some(pane) = self.controller.model().pane(pane) else {
            return Vec::new();
        };
        pane.attachments()
            .iter()
            .rev()
            .filter_map(|file| match self.attached.seen.get(file.path()) {
                Some(Seen::Picture { pixels, texture }) => Some(Picture {
                    path: file.path().into(),
                    label: file.title().map_or_else(|| file.name(), str::to_owned),
                    pixels: *pixels,
                    texture: texture.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    /// A picture opens at full size over the window, where its neighbours on
    /// the list are a step away; any other file opens with its application.
    pub(super) fn open_attachment(&mut self, ctx: &egui::Context, pane: PaneId, path: PathBuf) {
        let picture = self
            .attached_pictures(pane)
            .into_iter()
            .find(|picture| picture.path == path);
        match picture {
            Some(picture) => self.view_attached(pane, picture),
            None => self.explorer_event(ctx, ui::explorer::Event::Open(path)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neptune_model::{AgentKind, AgentSession, Attachment};

    fn settle(app: &mut App, ctx: &egui::Context, what: &str, done: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app.poll_attached(ctx);
            if done(app) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn attached_files_are_looked_at_listed_viewed_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _sender) = super::super::tests::fixture(dir.path());
        let ctx = egui::Context::default();
        app.startup = None;
        app.controller
            .dispatch(Command::AddWorkspace {
                group: None,
                cwd: dir.path().into(),
                name: "One".into(),
                remote: None,
            })
            .unwrap();
        let pane = app.controller.model().active_pane().unwrap();
        let generation = app.controller.model().pane(pane).unwrap().generation();
        app.controller
            .dispatch(Command::PaneAgentChanged {
                pane,
                generation,
                agent: Some(AgentSession {
                    kind: AgentKind::Claude,
                    session_id: None,
                    cwd: dir.path().into(),
                }),
            })
            .unwrap();
        let file = |name: &str| dir.path().join(name);
        for name in ["before.png", "after.png"] {
            image::RgbaImage::from_pixel(400, 200, image::Rgba([20, 120, 220, 255]))
                .save(file(name))
                .unwrap();
        }
        std::fs::write(file("report.md"), "# Report").unwrap();
        for (name, title) in [
            ("before.png", Some("Before")),
            ("report.md", None),
            ("gone.png", None),
            ("after.png", Some("After")),
        ] {
            app.controller
                .dispatch(Command::PaneFileAttached {
                    pane,
                    generation,
                    attachment: Attachment::new(file(name), title).unwrap(),
                })
                .unwrap();
        }
        let kinds = |app: &App| -> Vec<&'static str> {
            let pane = app.controller.model().pane(pane).unwrap();
            app.attached_files(pane)
                .iter()
                .map(|file| match file.kind {
                    ui::attached::Kind::Unread => "unread",
                    ui::attached::Kind::Picture(_) => "picture",
                    ui::attached::Kind::Other => "other",
                    ui::attached::Kind::Missing => "missing",
                })
                .collect()
        };
        assert_eq!(kinds(&app), ["unread"; 4]);
        settle(&mut app, &ctx, "the files to be looked at", |app| {
            !kinds(app).contains(&"unread")
        });
        assert_eq!(kinds(&app), ["picture", "other", "missing", "picture"]);
        let pictures = app.attached_pictures(pane);
        let labels: Vec<_> = pictures.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["After", "Before"]);
        assert_eq!(pictures[0].pixels, [400, 200]);
        // A picture opens over the window and steps to its neighbour.
        app.action(&ctx, Action::OpenAttachment(pane, file("after.png")));
        assert_eq!(app.ui.overlay, OverlayState::Image);
        assert_eq!(app.viewed_image(), Some(file("after.png").as_path()));
        app.step_attached(1);
        assert_eq!(app.viewed_image(), Some(file("before.png").as_path()));
        app.step_attached(1);
        assert_eq!(app.viewed_image(), Some(file("after.png").as_path()));
        app.action(&ctx, Action::CloseOverlay);
        // Taken off the list one at a time, then all at once.
        app.action(&ctx, Action::RemoveAttachment(pane, file("gone.png")));
        assert_eq!(kinds(&app), ["picture", "other", "picture"]);
        // A file attached again is looked at again.
        std::fs::remove_file(file("before.png")).unwrap();
        app.attached.forget(&file("before.png"));
        settle(&mut app, &ctx, "the file to be looked at again", |app| {
            kinds(app)[0] == "missing"
        });
        app.action(&ctx, Action::ClearAttachments(pane));
        assert!(kinds(&app).is_empty());
        assert!(app.controller.model().pane(pane).unwrap().agent().is_some());
    }
}
