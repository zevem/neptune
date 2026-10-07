//! Pictures and files reach a terminal as paths: a shell gets the path to work
//! with, and a CLI agent attaches the file a pasted path names. A project's
//! lead gets them with a message: they wait over its field as the person
//! picked, pasted or dropped them, each looked at once on a worker. Their
//! names and paths stay out of diagnostics.
use super::*;
use crate::{
    platform::clipboard::{self, Image},
    projects::transcript::{Attachment, MAX_ATTACHMENTS, MAX_PATH, MAX_PICTURE},
};
use neptune_model::ProjectId;
use std::path::Path;

/// Pasted pictures kept for the programs they were given to.
const KEPT_IMAGES: usize = 32;

/// Where a pasted picture goes.
#[derive(Clone, Copy)]
enum Target {
    /// A terminal, as it ran when the paste was asked for.
    Pane { pane: PaneId, generation: u64 },
    /// The message being written to a project's lead.
    Lead(ProjectId),
}

struct ImagePaste {
    target: Target,
    quiet: bool,
    /// The saved picture, or why there is none; `None` for an empty clipboard.
    result: Result<PathBuf, Option<String>>,
}

pub(super) struct Attachments {
    directory: PathBuf,
    sender: mpsc::Sender<ImagePaste>,
    receiver: mpsc::Receiver<ImagePaste>,
    /// One picture is read at a time; further requests wait for the user.
    reading: bool,
    /// The file picker is open for a lead's message.
    picking: Option<(ProjectId, mpsc::Receiver<Vec<PathBuf>>)>,
    /// Files for a lead's message as a worker found them.
    looked: (mpsc::Sender<Looked>, mpsc::Receiver<Looked>),
}
/// What a worker found where the files for a lead's message should be:
/// each as it is attached, or why it is not.
type Looked = (ProjectId, Vec<Result<Attachment, String>>);

impl Attachments {
    pub(super) fn new(directory: PathBuf) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            directory,
            sender,
            receiver,
            reading: false,
            picking: None,
            looked: mpsc::channel(),
        }
    }
}

impl App {
    /// Pastes the clipboard's text, or the path of a saved copy of its picture.
    /// `quiet` leaves an empty clipboard unreported, as for a key chord.
    pub(super) fn paste_clipboard(&mut self, ctx: &egui::Context, pane: PaneId, quiet: bool) {
        match clipboard::read() {
            Ok(text) if !text.is_empty() => self.paste_text(pane, &text),
            _ => self.paste_image(ctx, pane, quiet),
        }
    }

    pub(super) fn paste_text(&mut self, pane: PaneId, text: &str) {
        if let Some(session) = self.sessions.get(pane) {
            match session.paste(text) {
                Ok(()) => self.notifications.acknowledge(Some(pane)),
                Err(error) => self.ui.error = Some(error.to_string()),
            }
        }
    }

    /// Decoding and saving a screenshot takes too long for a frame.
    fn paste_image(&mut self, ctx: &egui::Context, pane: PaneId, quiet: bool) {
        let Some(generation) = self.sessions.generation(pane) else {
            return;
        };
        self.read_image(ctx, Target::Pane { pane, generation }, quiet);
    }

    fn read_image(&mut self, ctx: &egui::Context, target: Target, quiet: bool) {
        if self.attachments.reading {
            return;
        }
        let directory = self.attachments.directory.clone();
        let sender = self.attachments.sender.clone();
        let wake = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("neptune-paste-image".into())
            .spawn(move || {
                let result = match clipboard::read_image() {
                    Ok(image) => save_image(&directory, &image).map_err(Some),
                    Err(_) => Err(None),
                };
                let _ = sender.send(ImagePaste {
                    target,
                    quiet,
                    result,
                });
                wake.request_repaint();
            });
        match spawned {
            Ok(_) => self.attachments.reading = true,
            Err(error) => self.ui.error = Some(format!("Could not read the clipboard: {error}")),
        }
    }

    pub(super) fn poll_attachments(&mut self, ctx: &egui::Context) {
        while let Ok(paste) = self.attachments.receiver.try_recv() {
            self.attachments.reading = false;
            // A terminal closed or restarted meanwhile does not get the paste.
            if let Target::Pane { pane, generation } = paste.target
                && self.sessions.generation(pane) != Some(generation)
            {
                continue;
            }
            match (paste.result, paste.target) {
                (Ok(path), Target::Lead(project)) => self.attach_files(ctx, project, vec![path]),
                (Ok(path), Target::Pane { pane, .. }) => self.paste_paths(pane, &[path]),
                (Err(None), _) if paste.quiet => {}
                (Err(None), _) => {
                    self.ui.error = Some("The clipboard has no text or image to paste".into());
                }
                (Err(Some(error)), _) => {
                    self.ui.error = Some(format!("Could not save the pasted image: {error}"));
                }
            }
        }
        match self
            .attachments
            .picking
            .as_ref()
            .map(|(project, picked)| (*project, picked.try_recv()))
        {
            Some((project, Ok(files))) => {
                self.attachments.picking = None;
                self.attach_files(ctx, project, files);
            }
            Some((_, Err(mpsc::TryRecvError::Disconnected))) => self.attachments.picking = None,
            _ => {}
        }
        while let Ok((project, found)) = self.attachments.looked.1.try_recv() {
            self.attached_to_lead(project, found);
        }
    }

    /// A paste chord pressed in the field of a lead's message with no text
    /// on the clipboard attaches the picture there. Text is the field's own
    /// to paste.
    pub(super) fn paste_into_composer(&mut self, ctx: &egui::Context) {
        if self.swallowed_paste.is_none()
            || !ctx.memory(|memory| memory.has_focus(ui::chat::composer_id()))
        {
            return;
        }
        // The field in view is that of the project in front.
        let Some(project) = self.active_project().map(|project| project.id()) else {
            return;
        };
        self.swallowed_paste = None;
        self.read_image(ctx, Target::Lead(project), true);
    }

    /// Files held or released over the chat with a project's lead are for
    /// its message; a terminal gets every other drag as before. The chat
    /// says where it was drawn a frame ago, which is where the files are
    /// seen to be held. Where the platform does not say where they are
    /// held, they are for the message while its field has the keyboard, as
    /// they are for the focused terminal otherwise.
    pub(super) fn drop_on_lead(
        &mut self,
        ctx: &egui::Context,
        drag: crate::platform::file_drag::FileDrag,
    ) -> crate::platform::file_drag::FileDrag {
        let writing = ctx.memory(|memory| memory.has_focus(ui::chat::composer_id()));
        // Another sheet or the palette over the chat owns the window.
        let free = matches!(self.ui.overlay, OverlayState::None | OverlayState::Project);
        let over = self
            .ui
            .project
            .chat_area
            .take()
            .filter(|(_, area)| {
                free && drag
                    .pointer
                    .map_or(writing, |pointer| area.contains(pointer))
            })
            .map(|(project, _)| project);
        self.ui.project.dropping = over.is_some() && drag.hovering;
        let Some(project) = over else {
            return drag;
        };
        self.attach_files(ctx, project, drag.dropped);
        Default::default()
    }

    /// Opens the file picker for a message to `project`'s lead.
    pub(super) fn pick_attachments(&mut self, ctx: &egui::Context, project: ProjectId) {
        if self.attachments.picking.is_some() {
            return;
        }
        if self.ui.project.attached.get(&project).map_or(0, Vec::len) >= MAX_ATTACHMENTS {
            self.ui.error = Some(too_many());
            return;
        }
        match self.folder_picker.open_files(ctx.clone()) {
            Ok(picked) => self.attachments.picking = Some((project, picked)),
            Err(error) => self.ui.error = Some(error),
        }
    }

    /// Has `files` looked at for the message being written to `project`'s
    /// lead: a folder on another machine can take longer than a frame to
    /// answer.
    pub(super) fn attach_files(
        &mut self,
        ctx: &egui::Context,
        project: ProjectId,
        mut files: Vec<PathBuf>,
    ) {
        if files.is_empty() {
            return;
        }
        // Never more looked at than a message could take.
        let over = files.len() > MAX_ATTACHMENTS;
        files.truncate(MAX_ATTACHMENTS);
        let sender = self.attachments.looked.0.clone();
        let wake = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("neptune-attach".into())
            .spawn(move || {
                let mut found: Vec<_> = files.iter().map(|path| look(path)).collect();
                if over {
                    found.push(Err(too_many()));
                }
                let _ = sender.send((project, found));
                wake.request_repaint();
            });
        if let Err(error) = spawned {
            self.ui.error = Some(format!("Could not read the files: {error}"));
        }
    }

    /// Puts what was found over the field of `project`'s message, and says
    /// what was left out and why.
    fn attached_to_lead(&mut self, project: ProjectId, found: Vec<Result<Attachment, String>>) {
        // The project was removed meanwhile.
        if self.controller.model().project(project).is_none() {
            return;
        }
        let attached = self.ui.project.attached.entry(project).or_default();
        let mut refused = Vec::new();
        for file in found {
            match file {
                // Attached twice is attached once.
                Ok(file) if attached.contains(&file) => {}
                Ok(_) if attached.len() >= MAX_ATTACHMENTS => refused.push(too_many()),
                Ok(file) => attached.push(file),
                Err(why) => refused.push(why),
            }
        }
        refused.dedup();
        if !refused.is_empty() {
            self.ui.error = Some(refused.join(" "));
        }
        // What was attached is sent from the field, with Enter.
        self.ui.project.focus = true;
    }

    /// Pastes files as shell words, each followed by a space so that the
    /// next one, or the rest of a command, can follow.
    pub(super) fn paste_paths(&mut self, pane: PaneId, paths: &[PathBuf]) {
        let text: String = paths.iter().map(|path| quoted(path) + " ").collect();
        if !text.is_empty() {
            self.paste_text(pane, &text);
        }
    }
}

fn too_many() -> String {
    format!("A message takes at most {MAX_ATTACHMENTS} files.")
}

/// A file as it is attached to a lead's message, or why it is not. It is
/// opened once, so that one the lead's CLI could not read is refused here.
fn look(path: &Path) -> Result<Attachment, String> {
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    let unreadable = || format!("“{name}” could not be read.");
    // The lead is told the path in a line of its own.
    let text = std::path::absolute(path)
        .ok()
        .and_then(|path| path.into_os_string().into_string().ok())
        .filter(|text| text.len() <= MAX_PATH && !text.chars().any(char::is_control))
        .ok_or_else(|| format!("“{name}” has a path the lead cannot be given."))?;
    let about = std::fs::metadata(path).map_err(|_| unreadable())?;
    if about.is_dir() {
        return Err(format!("“{name}” is a folder. Attach the files in it."));
    }
    if !about.is_file() || std::fs::File::open(path).is_err() {
        return Err(unreadable());
    }
    let file = Attachment {
        path: text,
        bytes: about.len(),
    };
    if file.picture() && file.bytes > MAX_PICTURE {
        return Err(format!(
            "“{name}” is {}. A picture for the lead is at most {:.1} MB.",
            file.size(),
            MAX_PICTURE as f64 / (1024.0 * 1024.0)
        ));
    }
    Ok(file)
}

/// A path as one word of a shell command line.
fn quoted(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        return if text.contains(' ') {
            format!("\"{text}\"")
        } else {
            text.into_owned()
        };
    }
    let plain = |byte: u8| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte);
    if !text.is_empty() && text.bytes().all(plain) {
        text.into_owned()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

/// Saves a clipboard picture as a PNG only its owner can read, and forgets
/// the oldest saved pictures beyond a bound.
fn save_image(directory: &Path, image: &Image) -> Result<PathBuf, String> {
    let pixels = (image.width as usize)
        .checked_mul(image.height as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    if pixels != Some(image.rgba.len()) || image.rgba.is_empty() {
        return Err("the clipboard image is malformed".into());
    }
    let save = || -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(directory)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis());
        let path = directory.join(format!("paste-{stamp:013}.png"));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut encoder = png::Encoder::new(
            std::io::BufWriter::new(options.open(&path)?),
            image.width,
            image.height,
        );
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        encoder.write_header()?.write_image_data(&image.rgba)?;
        let mut saved: Vec<_> = std::fs::read_dir(directory)?
            .filter_map(|entry| Some(entry.ok()?.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("paste-") && name.ends_with(".png"))
            })
            .collect();
        saved.sort();
        for old in saved.iter().rev().skip(KEPT_IMAGES) {
            let _ = std::fs::remove_file(old);
        }
        Ok(path)
    };
    save().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn dropped_files_and_pasted_pictures_reach_the_shell_as_single_words() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, _sender) = crate::app::tests::fixture(root.path());
        let ctx = egui::Context::default();
        app.startup = None;
        app.config.shell = Some("/bin/sh".into());
        app.dispatch(
            &ctx,
            Command::AddWorkspace {
                group: None,
                cwd: root.path().into(),
                name: "Home".into(),
                remote: None,
            },
        );
        let first = app.controller.model().active_pane().unwrap();
        app.action(&ctx, Action::Split(first, neptune_model::Axis::Vertical));
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.sessions.usage().running != 2 {
            app.poll(&ctx);
            assert!(Instant::now() < deadline, "{:?}", app.ui.error);
            std::thread::sleep(Duration::from_millis(10));
        }
        let created = |path: &Path| {
            while !path.exists() {
                assert!(
                    Instant::now() < deadline,
                    "{} was not created",
                    path.display()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        };

        // Files dropped on the unfocused terminal focus it and name the file.
        let dropped = root.path().join("it's a shot.png");
        assert_ne!(app.controller.model().active_pane(), Some(first));
        app.sessions.get(first).unwrap().write(b"touch ").unwrap();
        app.action(&ctx, Action::DropFiles(first, vec![dropped.clone()]));
        assert_eq!(app.controller.model().active_pane(), Some(first));
        app.sessions.get(first).unwrap().write(b"\r").unwrap();
        created(&dropped);

        // A picture saved for a terminal that was since restarted is dropped;
        // one for the running terminal is pasted as its path.
        let generation = app.sessions.generation(first).unwrap();
        let paste = |generation, name: &str| ImagePaste {
            target: Target::Pane {
                pane: first,
                generation,
            },
            quiet: false,
            result: Ok(root.path().join(name)),
        };
        app.sessions.get(first).unwrap().write(b"touch ").unwrap();
        let sender = app.attachments.sender.clone();
        sender.send(paste(generation + 1, "stale.png")).unwrap();
        sender.send(paste(generation, "pasted.png")).unwrap();
        app.poll_attachments(&ctx);
        app.sessions.get(first).unwrap().write(b"\r").unwrap();
        created(&root.path().join("pasted.png"));
        assert!(!root.path().join("stale.png").exists());

        // An empty clipboard is reported from the menu, not for a key chord.
        let empty = |quiet| ImagePaste {
            target: Target::Pane {
                pane: first,
                generation,
            },
            quiet,
            result: Err(None),
        };
        sender.send(empty(true)).unwrap();
        app.poll_attachments(&ctx);
        assert_eq!(app.ui.error, None);
        sender.send(empty(false)).unwrap();
        app.poll_attachments(&ctx);
        assert!(app.ui.error.is_some());
    }

    #[cfg(not(windows))]
    #[test]
    fn paths_are_single_shell_words() {
        let quoted = |path: &str| quoted(Path::new(path));
        assert_eq!(quoted("/home/me/shot-1.png"), "/home/me/shot-1.png");
        assert_eq!(
            quoted("/home/me/Screenshot from 2026.png"),
            "'/home/me/Screenshot from 2026.png'"
        );
        assert_eq!(
            quoted("/tmp/it's; rm -rf $HOME"),
            r"'/tmp/it'\''s; rm -rf $HOME'"
        );
        assert_eq!(quoted("/tmp/café.png"), "'/tmp/café.png'");
    }

    #[test]
    fn a_file_for_a_lead_is_looked_at_once_and_one_that_cannot_go_is_refused_plainly() {
        let directory = tempfile::tempdir().unwrap();
        let notes = directory.path().join("notes.md");
        std::fs::write(&notes, b"plan").unwrap();
        assert_eq!(
            look(&notes),
            Ok(Attachment {
                path: notes.to_str().unwrap().into(),
                bytes: 4,
            })
        );
        // Another kind of file is only named to the lead, whatever its size;
        // a picture is shown to it, up to what its provider takes.
        let sized = |name: &str, bytes: u64| {
            let path = directory.path().join(name);
            std::fs::File::create(&path)
                .unwrap()
                .set_len(bytes)
                .unwrap();
            look(&path)
        };
        assert_eq!(
            sized("video.mp4", 4 * MAX_PICTURE).map(|file| file.bytes),
            Ok(4 * MAX_PICTURE)
        );
        assert!(sized("shot.png", MAX_PICTURE).is_ok());
        assert_eq!(
            sized("large.JPG", 6 * 1024 * 1024),
            Err("“large.JPG” is 6.0 MB. A picture for the lead is at most 3.8 MB.".into())
        );
        assert_eq!(
            look(directory.path()).unwrap_err(),
            format!(
                "“{}” is a folder. Attach the files in it.",
                directory.path().file_name().unwrap().to_str().unwrap()
            )
        );
        assert_eq!(
            look(&directory.path().join("gone.txt")),
            Err("“gone.txt” could not be read.".into())
        );
        // The lead is told a path in a line of its own, of a bounded length.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let broken = directory.path().join("two\nlines.txt");
            std::fs::write(&broken, b"x").unwrap();
            assert!(look(&broken).unwrap_err().ends_with("cannot be given."));
            let deep = directory.path().join("d".repeat(200)).join("e".repeat(200));
            let deep = deep.join("f".repeat(200)).join("g".repeat(200));
            let deep = deep.join("h".repeat(200));
            std::fs::create_dir_all(&deep).unwrap();
            std::fs::write(deep.join("far.txt"), b"x").unwrap();
            assert_eq!(
                look(&deep.join("far.txt")),
                Err("“far.txt” has a path the lead cannot be given.".into())
            );
            let secret = directory.path().join("secret.txt");
            std::fs::write(&secret, b"x").unwrap();
            std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();
            // An administrator reads everything.
            if std::fs::File::open(&secret).is_err() {
                assert_eq!(look(&secret), Err("“secret.txt” could not be read.".into()));
            }
        }
    }

    #[test]
    fn files_held_over_a_leads_chat_are_its_messages_and_others_are_a_terminals() {
        use crate::platform::file_drag::FileDrag;
        let root = tempfile::tempdir().unwrap();
        let (mut app, _sender) = crate::app::tests::fixture(root.path());
        let ctx = egui::Context::default();
        let project = ProjectId::new(3);
        let area = egui::Rect::from_min_size(egui::pos2(600.0, 100.0), egui::vec2(300.0, 400.0));
        let drag = |pointer: Option<egui::Pos2>, dropped: &[&str]| FileDrag {
            hovering: dropped.is_empty(),
            pointer,
            dropped: dropped.iter().map(PathBuf::from).collect(),
        };
        // Held over the chat, the chat says so and no terminal does.
        app.ui.project.chat_area = Some((project, area));
        let over = Some(egui::pos2(700.0, 300.0));
        assert_eq!(app.drop_on_lead(&ctx, drag(over, &[])), FileDrag::default());
        assert!(app.ui.project.dropping);
        // The chat says where it is each frame it is drawn; one that is no
        // longer drawn takes nothing.
        assert!(app.ui.project.chat_area.is_none());
        let held = drag(over, &[]);
        assert_eq!(app.drop_on_lead(&ctx, held.clone()), held);
        assert!(!app.ui.project.dropping);
        // Elsewhere, or where the platform does not say and the message
        // is not being written, files are a terminal's as before; so are
        // they under the palette.
        for (pointer, overlay) in [
            (Some(egui::pos2(100.0, 300.0)), OverlayState::None),
            (None, OverlayState::None),
            (over, OverlayState::Palette),
        ] {
            app.ui.project.chat_area = Some((project, area));
            app.ui.overlay = overlay;
            let dropped = drag(pointer, &["/tmp/a.png"]);
            assert_eq!(app.drop_on_lead(&ctx, dropped.clone()), dropped);
            assert!(!app.ui.project.dropping);
        }
        app.ui.overlay = OverlayState::None;
        // Released over the chat they are taken from the terminals.
        app.ui.project.chat_area = Some((project, area));
        assert_eq!(
            app.drop_on_lead(&ctx, drag(over, &["/tmp/a.png"])),
            FileDrag::default()
        );
        assert!(!app.ui.project.dropping);
        // Each is looked at on a worker; the test takes what it found
        // itself, so that polling does not take it first.
        let looked = |app: &mut App| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok((to, found)) = app.attachments.looked.1.try_recv() {
                    assert!(to == project && found.len() == 1 && found[0].is_err());
                    app.attached_to_lead(to, found);
                    break;
                }
                assert!(Instant::now() < deadline, "the files were not looked at");
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        looked(&mut app);
        // A pasted picture saved for a lead's message is looked at like any
        // other file, and a project that has gone since gets nothing.
        app.attachments
            .sender
            .send(ImagePaste {
                target: Target::Lead(project),
                quiet: true,
                result: Ok(root.path().join("paste-0000000000001.png")),
            })
            .unwrap();
        app.poll_attachments(&ctx);
        looked(&mut app);
        assert!(app.ui.project.attached.is_empty() && app.ui.error.is_none());
    }

    #[test]
    fn pasted_pictures_are_saved_as_png_and_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let image = Image {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 0, 255, 128],
        };
        for index in 0..KEPT_IMAGES + 3 {
            std::fs::write(
                directory.path().join(format!("paste-{index:013}.png")),
                b"old",
            )
            .unwrap();
        }
        std::fs::write(directory.path().join("notes.txt"), b"kept").unwrap();
        let path = save_image(directory.path(), &image).unwrap();
        let mut reader =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()))
                .read_info()
                .unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!((info.width, info.height), (2, 1));
        assert_eq!(&pixels[..info.buffer_size()], image.rgba.as_slice());
        let mut names: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names.len(), KEPT_IMAGES + 1);
        assert_eq!(names[0], "notes.txt");
        assert_eq!(names[1], format!("paste-{:013}.png", 4));
        assert_eq!(
            names.last().map(String::as_str),
            path.file_name().unwrap().to_str()
        );

        let malformed = Image {
            rgba: vec![0; 7],
            ..image
        };
        assert!(save_image(directory.path(), &malformed).is_err());
    }
}
