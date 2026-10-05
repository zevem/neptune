//! Pictures and files reach a terminal as paths: a shell gets the path to work
//! with, and a CLI agent attaches the file a pasted path names.
use super::*;
use crate::platform::clipboard::{self, Image};
use std::path::Path;

/// Pasted pictures kept for the programs they were given to.
const KEPT_IMAGES: usize = 32;

struct ImagePaste {
    pane: PaneId,
    generation: u64,
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
}

impl Attachments {
    pub(super) fn new(directory: PathBuf) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            directory,
            sender,
            receiver,
            reading: false,
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
                    pane,
                    generation,
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

    pub(super) fn poll_attachments(&mut self) {
        while let Ok(paste) = self.attachments.receiver.try_recv() {
            self.attachments.reading = false;
            // A terminal closed or restarted meanwhile does not get the paste.
            if self.sessions.generation(paste.pane) != Some(paste.generation) {
                continue;
            }
            match paste.result {
                Ok(path) => self.paste_paths(paste.pane, &[path]),
                Err(None) if paste.quiet => {}
                Err(None) => {
                    self.ui.error = Some("The clipboard has no text or image to paste".into());
                }
                Err(Some(error)) => {
                    self.ui.error = Some(format!("Could not save the pasted image: {error}"));
                }
            }
        }
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
            pane: first,
            generation,
            quiet: false,
            result: Ok(root.path().join(name)),
        };
        app.sessions.get(first).unwrap().write(b"touch ").unwrap();
        let sender = app.attachments.sender.clone();
        sender.send(paste(generation + 1, "stale.png")).unwrap();
        sender.send(paste(generation, "pasted.png")).unwrap();
        app.poll_attachments();
        app.sessions.get(first).unwrap().write(b"\r").unwrap();
        created(&root.path().join("pasted.png"));
        assert!(!root.path().join("stale.png").exists());

        // An empty clipboard is reported from the menu, not for a key chord.
        let empty = |quiet| ImagePaste {
            pane: first,
            generation,
            quiet,
            result: Err(None),
        };
        sender.send(empty(true)).unwrap();
        app.poll_attachments();
        assert_eq!(app.ui.error, None);
        sender.send(empty(false)).unwrap();
        app.poll_attachments();
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
