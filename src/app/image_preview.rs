//! A picture named in a terminal is read and shown while the pointer rests on
//! its path. Finding and decoding the file happens on a worker; paths and
//! pictures stay out of diagnostics and saved state.
use super::*;
use crate::terminal_view::ImagePath;
use std::path::Path;

/// How long the pointer rests on a path before its file is read.
const DWELL: f64 = 0.25;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DECODED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PIXELS_PER_SIDE: u32 = 16_384;
/// The longest side of the full view's copy of a picture.
const VIEW_PIXELS: usize = 4096;

#[derive(Default)]
pub(super) struct ImagePreview {
    hover: Option<Hover>,
    /// Where the card was last frame; the pointer may move from the path
    /// onto it.
    pub(super) card: Option<Rect>,
    /// The picture shown at full size while its overlay is open.
    view: Option<View>,
    channel: Option<(mpsc::Sender<Read>, mpsc::Receiver<Read>)>,
    /// One file is read at a time; a newer path waits for the worker.
    reading: bool,
    requests: u64,
}

/// A path the pointer rests on, kept while its spellings stay the same.
struct Hover {
    pane: PaneId,
    texts: Vec<String>,
    /// The cells of each spelling, as of the last frame the pointer was on
    /// the path.
    rows: Vec<Vec<Rect>>,
    since: f64,
    request: Option<u64>,
    shown: Option<Shown>,
}

struct Shown {
    /// Which spelling named the file.
    reading: usize,
    path: PathBuf,
    name: String,
    pixels: [u32; 2],
    texture: egui::TextureHandle,
}

struct View {
    path: PathBuf,
    name: String,
    pixels: [u32; 2],
    /// The card's small picture until the file has been read at this size.
    texture: egui::TextureHandle,
    request: Option<u64>,
    zoom: ui::image_preview::Zoom,
}

struct Read {
    request: u64,
    picture: Option<Picture>,
}

struct Picture {
    reading: usize,
    path: PathBuf,
    pixels: [u32; 2],
    image: egui::ColorImage,
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl ImagePreview {
    /// Reads a picture on a worker and returns the request it will answer,
    /// or `None` while another file is being read or no worker can start.
    fn request(
        &mut self,
        ctx: &egui::Context,
        read: impl FnOnce() -> Option<Picture> + Send + 'static,
    ) -> Option<u64> {
        if self.reading {
            return None;
        }
        let (sender, _) = self.channel.get_or_insert_with(mpsc::channel);
        let sender = sender.clone();
        let request = self.requests + 1;
        let wake = ctx.clone();
        std::thread::Builder::new()
            .name("neptune-image-preview".into())
            .spawn(move || {
                let _ = sender.send(Read {
                    request,
                    picture: read(),
                });
                wake.request_repaint();
            })
            .ok()?;
        self.requests = request;
        self.reading = true;
        Some(request)
    }

    /// Hands finished pictures to whichever of the card and the full view
    /// still waits for them.
    fn poll(&mut self, ctx: &egui::Context) {
        let Some((_, receiver)) = &self.channel else {
            return;
        };
        while let Ok(read) = receiver.try_recv() {
            self.reading = false;
            let Some(picture) = read.picture else {
                continue;
            };
            // The pointer may have moved on to another path meanwhile.
            if let Some(hover) = &mut self.hover
                && hover.request == Some(read.request)
            {
                hover.shown = Some(Shown {
                    reading: picture.reading,
                    name: file_name(&picture.path),
                    path: picture.path,
                    pixels: picture.pixels,
                    texture: ctx.load_texture(
                        "image-preview",
                        picture.image,
                        egui::TextureOptions::LINEAR,
                    ),
                });
            } else if let Some(view) = &mut self.view
                && view.request == Some(read.request)
            {
                view.pixels = picture.pixels;
                // Mipmaps keep a large picture clean while it is shown small.
                view.texture = ctx.load_texture(
                    "image-view",
                    picture.image,
                    egui::TextureOptions {
                        mipmap_mode: Some(egui::TextureFilter::Linear),
                        ..egui::TextureOptions::LINEAR
                    },
                );
            }
        }
    }
}

impl App {
    /// Follows the path under the pointer and shows the picture it names:
    /// in a card beside the path, and at full size once the card is clicked.
    pub(super) fn preview_image(
        &mut self,
        ui: &egui::Ui,
        p: Palette,
        bounds: Rect,
        hover: Option<(PaneId, Vec<ImagePath>)>,
    ) {
        let ctx = ui.ctx();
        self.image_preview.poll(ctx);
        if self.ui.overlay != OverlayState::None {
            // A sheet owns the window; the card does not float over it.
            self.image_preview.hover = None;
            self.image_preview.card = None;
            ui::image_preview::show(ui, p, bounds, None);
            self.view_image(ui, p, bounds);
            return;
        }
        self.image_preview.view = None;
        let now = ctx.input(|input| input.time);
        let preview = &mut self.image_preview;
        // On the card, or crossing to it, the pointer is still on the path.
        let held = preview
            .hover
            .as_ref()
            .is_some_and(|hover| hover.shown.is_some())
            && preview
                .card
                .zip(ctx.input(|input| input.pointer.hover_pos()))
                .is_some_and(|(card, pointer)| {
                    card.expand(ui::image_preview::REACH).contains(pointer)
                });
        if !held {
            match hover {
                None => preview.hover = None,
                Some((pane, paths)) => {
                    let same = preview.hover.as_ref().is_some_and(|hover| {
                        hover.pane == pane
                            && hover.texts.iter().eq(paths.iter().map(|path| &path.text))
                    });
                    if !same {
                        preview.hover = Some(Hover {
                            pane,
                            texts: paths.iter().map(|path| path.text.clone()).collect(),
                            rows: Vec::new(),
                            since: now,
                            request: None,
                            shown: None,
                        });
                    }
                    if let Some(hover) = &mut preview.hover {
                        hover.rows = paths.into_iter().map(|path| path.rows).collect();
                    }
                }
            }
        }
        if let Some(hover) = &preview.hover
            && hover.request.is_none()
            && !preview.reading
        {
            let rested = now - hover.since;
            if rested < DWELL {
                ctx.request_repaint_after(Duration::from_secs_f64(DWELL - rested));
            } else {
                let pane = hover.pane;
                let texts = hover.texts.clone();
                // Relative paths are read from where the agent runs, then
                // from the shell's directory.
                let mut directories = Vec::new();
                if let Some(agent) = self.controller.model().pane(pane).and_then(|p| p.agent()) {
                    directories.push(agent.cwd.clone());
                }
                if let Some(session) = self.sessions.get(pane) {
                    let metadata = session.metadata();
                    directories.extend(metadata.reported_cwd);
                    directories.push(metadata.cwd);
                }
                directories.dedup();
                let limit = texture_limit(ctx, ui::image_preview::MAX_SIZE);
                let request = preview.request(ctx, move || {
                    let home = directories::BaseDirs::new();
                    let home = home.as_ref().map(|dirs| dirs.home_dir());
                    read(&texts, &directories, home, limit)
                });
                if let Some(hover) = &mut preview.hover {
                    hover.request = request;
                }
            }
        }
        let shown = preview.hover.as_ref().and_then(|hover| {
            let shown = hover.shown.as_ref()?;
            Some(ui::image_preview::Preview {
                texture: &shown.texture,
                name: &shown.name,
                pixels: shown.pixels,
                rows: hover.rows.get(shown.reading)?,
            })
        });
        let card = ui::image_preview::show(ui, p, bounds, shown);
        preview.card = card.card;
        if card.clicked
            && let Some(shown) = preview.hover.take().and_then(|hover| hover.shown)
        {
            preview.card = None;
            preview.view = Some(View {
                path: shown.path,
                name: shown.name,
                pixels: shown.pixels,
                texture: shown.texture,
                request: None,
                zoom: Default::default(),
            });
            self.ui.overlay = OverlayState::Image;
        }
    }

    /// The clicked picture over the dimmed window, read again in more
    /// detail when the card's copy is smaller than the file.
    fn view_image(&mut self, ui: &egui::Ui, p: Palette, bounds: Rect) {
        let ctx = ui.ctx();
        let preview = &mut self.image_preview;
        if self.ui.overlay != OverlayState::Image {
            preview.view = None;
            return;
        }
        let Some(view) = &preview.view else {
            return;
        };
        let size = view.texture.size();
        if view.request.is_none() && size != view.pixels.map(|side| side as usize) {
            // Enough to stay sharp when zoomed in, whatever the window's size.
            let side = ctx.input(|input| input.max_texture_side).min(VIEW_PIXELS) as u32;
            let limit = [side, side];
            let path = view.path.clone();
            let request = preview.request(ctx, move || {
                let (pixels, image) = decode(&path, limit)?;
                Some(Picture {
                    reading: 0,
                    path,
                    pixels,
                    image,
                })
            });
            if let Some(view) = &mut preview.view {
                view.request = request;
            }
        }
        let Some(view) = &mut preview.view else {
            return;
        };
        let full = ui::image_preview::View {
            texture: &view.texture,
            name: &view.name,
            pixels: view.pixels,
        };
        if ui::image_preview::view(ui, p, bounds, full, &mut view.zoom) {
            self.action(ctx, Action::CloseOverlay);
        }
    }
}

/// The pixels a picture shown in `size` points can use on this display.
fn texture_limit(ctx: &egui::Context, size: Vec2) -> [u32; 2] {
    let size = size * ctx.pixels_per_point();
    let side = ctx.input(|input| input.max_texture_side) as f32;
    [
        size.x.min(side).ceil() as u32,
        size.y.min(side).ceil() as u32,
    ]
}

/// The first spelling that names a readable picture, scaled to fit `limit`.
fn read(
    texts: &[String],
    directories: &[PathBuf],
    home: Option<&Path>,
    limit: [u32; 2],
) -> Option<Picture> {
    texts.iter().enumerate().find_map(|(reading, text)| {
        locations(text, directories, home)
            .into_iter()
            .find_map(|path| {
                let (pixels, image) = decode(&path, limit)?;
                Some(Picture {
                    reading,
                    path,
                    pixels,
                    image,
                })
            })
    })
}

/// Where a spelling may point: `file://` targets, `~`, shell-escaped spaces
/// and paths relative to each of the terminal's directories.
fn locations(text: &str, directories: &[PathBuf], home: Option<&Path>) -> Vec<PathBuf> {
    let text = if text
        .get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file://"))
    {
        // The host of a file URL is this machine, or nothing useful.
        let Some(path) = text[7..].find('/').map(|slash| &text[7 + slash..]) else {
            return Vec::new();
        };
        let path = percent_decoded(path);
        // `file:///C:/shot.png` names `C:/shot.png`, not a path under a root.
        match path.as_bytes() {
            [b'/', drive, b':', ..] if cfg!(windows) && drive.is_ascii_alphabetic() => {
                path[1..].to_owned()
            }
            _ => path,
        }
    } else if cfg!(windows) {
        text.to_owned()
    } else {
        text.replace("\\ ", " ")
    };
    if let Some(rest) = text.strip_prefix("~/") {
        return home.map(|home| home.join(rest)).into_iter().collect();
    }
    let path = Path::new(&text);
    if path.is_absolute() {
        vec![path.into()]
    } else {
        directories.iter().map(|base| base.join(path)).collect()
    }
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = text
            .get(index + 1..index + 3)
            .filter(|_| bytes[index] == b'%')
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        decoded.push(byte.unwrap_or(bytes[index]));
        index += if byte.is_some() { 3 } else { 1 };
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Decodes a regular file by its contents, whatever its name claims, within
/// fixed bounds. Returns the file's own pixel size with the scaled picture.
pub(super) fn decode(path: &Path, limit: [u32; 2]) -> Option<([u32; 2], egui::ColorImage)> {
    use image::ImageDecoder as _;
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    let mut reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_PIXELS_PER_SIDE);
    limits.max_image_height = Some(MAX_PIXELS_PER_SIDE);
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().ok();
    let mut picture = image::DynamicImage::from_decoder(decoder).ok()?;
    if let Some(orientation) = orientation {
        picture.apply_orientation(orientation);
    }
    let pixels = [picture.width(), picture.height()];
    if pixels[0] > limit[0] || pixels[1] > limit[1] {
        picture = picture.thumbnail(limit[0].max(1), limit[1].max(1));
    }
    let picture = picture.into_rgba8();
    let size = [picture.width() as usize, picture.height() as usize];
    Some((
        pixels,
        egui::ColorImage::from_rgba_unmultiplied(size, picture.as_raw()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(path: &Path, width: u32, height: u32) {
        image::RgbaImage::from_pixel(width, height, image::Rgba([200, 40, 40, 255]))
            .save(path)
            .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn file_urls_and_drive_paths_name_files_on_their_drive() {
        let directories = [PathBuf::from(r"C:\agent")];
        let at = |text| locations(text, &directories, None);
        assert_eq!(
            at("file:///C:/shots/a%20b.png"),
            [PathBuf::from("C:/shots/a b.png")]
        );
        assert_eq!(at(r"C:\shots\a.png"), [PathBuf::from(r"C:\shots\a.png")]);
        assert_eq!(at(r"out\a.png"), [PathBuf::from(r"C:\agent\out\a.png")]);
    }

    #[cfg(not(windows))]
    #[test]
    fn spellings_resolve_against_home_file_urls_and_each_directory() {
        let home = Path::new("/home/me");
        let directories = [PathBuf::from("/agent"), PathBuf::from("/shell")];
        let at = |text| locations(text, &directories, Some(home));
        assert_eq!(at("/tmp/a.png"), [PathBuf::from("/tmp/a.png")]);
        assert_eq!(at("~/a.png"), [PathBuf::from("/home/me/a.png")]);
        assert_eq!(
            at("out/a.png"),
            [
                PathBuf::from("/agent/out/a.png"),
                PathBuf::from("/shell/out/a.png")
            ]
        );
        assert_eq!(at("file:///tmp/a%20b.png"), [PathBuf::from("/tmp/a b.png")]);
        assert_eq!(at("FILE://host/tmp/a.png"), [PathBuf::from("/tmp/a.png")]);
        assert!(at("file://host").is_empty());
        assert_eq!(at("/tmp/a\\ b.png"), [PathBuf::from("/tmp/a b.png")]);
        assert!(locations("~/a.png", &directories, None).is_empty());
    }

    #[test]
    fn the_first_spelling_that_is_a_picture_is_read_and_scaled_to_fit() {
        let directory = tempfile::tempdir().unwrap();
        picture(&directory.path().join("wide shot.png"), 400, 100);
        // A name alone proves nothing: this one holds text.
        std::fs::write(directory.path().join("shot.png"), "not a picture").unwrap();
        let read = read(
            &[
                "shot.png".into(),
                "missing.png".into(),
                "wide shot.png".into(),
            ],
            &[directory.path().into()],
            None,
            [100, 100],
        )
        .unwrap();
        assert_eq!(read.reading, 2);
        assert_eq!(file_name(&read.path), "wide shot.png");
        assert_eq!(read.pixels, [400, 100]);
        assert_eq!(read.image.size, [100, 25]);
    }

    #[test]
    fn contents_decide_the_format_and_directories_are_not_pictures() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("really-a.jpg");
        image::RgbImage::from_pixel(8, 6, image::Rgb([10, 200, 90]))
            .save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        let (pixels, image) = decode(&path, [64, 64]).unwrap();
        assert_eq!((pixels, image.size), ([8, 6], [8, 6]));
        for format in [
            image::ImageFormat::Jpeg,
            image::ImageFormat::Gif,
            image::ImageFormat::WebP,
            image::ImageFormat::Bmp,
        ] {
            image::RgbaImage::from_pixel(8, 6, image::Rgba([10, 200, 90, 255]))
                .save_with_format(&path, format)
                .or_else(|_| {
                    image::RgbImage::from_pixel(8, 6, image::Rgb([10, 200, 90]))
                        .save_with_format(&path, format)
                })
                .unwrap();
            assert_eq!(decode(&path, [64, 64]).unwrap().0, [8, 6], "{format:?}");
        }
        std::fs::create_dir(directory.path().join("folder.png")).unwrap();
        assert!(decode(&directory.path().join("folder.png"), [64, 64]).is_none());
        assert!(decode(&directory.path().join("missing.png"), [64, 64]).is_none());
    }
}
