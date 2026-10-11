//! A picked element as text for the clipboard, and the picture of it. The
//! page wrote the description, so every part is bounded and treated as text.
use eframe::egui;
use std::path::{Path, PathBuf};

/// The longest value of each part, in characters, as the page script cuts them.
const LIMITS: [(&str, &str, usize); 7] = [
    ("url", "url", 2048),
    ("selector", "selector", 512),
    ("component", "component", 128),
    ("source", "source", 512),
    ("text", "text", 200),
    ("html", "html", 2000),
    ("styles", "styles", 2000),
];

fn bounded(value: &serde_json::Value, limit: usize) -> Option<String> {
    let text: String = value
        .as_str()?
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .take(limit)
        .collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// What a person pastes to an agent or an issue: where the element is, what
/// it is and how it looks. Parts the page left out are left out.
pub(super) fn describe(pick: &str, screenshot: Option<&Path>) -> Option<String> {
    let pick: serde_json::Value = serde_json::from_str(pick).ok()?;
    let tag = bounded(pick.get("tag")?, 64)?;
    let mut said = format!("Element picked in the browser preview\ntag: {tag}\n");
    for (key, label, limit) in LIMITS {
        let Some(mut value) = pick.get(key).and_then(|value| bounded(value, limit)) else {
            continue;
        };
        if key == "component" {
            let owners: Vec<String> = pick
                .get("owners")
                .and_then(|owners| owners.as_array())
                .into_iter()
                .flatten()
                .take(4)
                .filter_map(|owner| bounded(owner, 128))
                .collect();
            if !owners.is_empty() {
                value = format!("{value} (in {})", owners.join(" > "));
            }
        }
        if value.contains('\n') || matches!(key, "html" | "styles") {
            said.push_str(label);
            said.push_str(":\n");
            for line in value.lines() {
                said.push_str("  ");
                said.push_str(line);
                said.push('\n');
            }
        } else {
            said.push_str(&format!("{label}: {value}\n"));
        }
    }
    if let Some(path) = screenshot {
        said.push_str(&format!("screenshot: {}\n", path.display()));
    }
    Some(said)
}

/// The element's place in the frame, in physical pixels with a margin, or
/// none when the page named no place inside it.
fn region(pick: &str, size: [usize; 2]) -> Option<[usize; 4]> {
    let pick: serde_json::Value = serde_json::from_str(pick).ok()?;
    let scale = pick
        .get("dpr")?
        .as_f64()
        .filter(|s| (0.25..=4.0).contains(s))?;
    let rect = pick.get("rect")?;
    let part = |key: &str| rect.get(key)?.as_f64().filter(|v| v.is_finite());
    let (x, y, width, height) = (part("x")?, part("y")?, part("width")?, part("height")?);
    const MARGIN: f64 = 20.0;
    let left = ((x - MARGIN) * scale).floor().clamp(0.0, size[0] as f64) as usize;
    let top = ((y - MARGIN) * scale).floor().clamp(0.0, size[1] as f64) as usize;
    let right = ((x + width + MARGIN) * scale)
        .ceil()
        .clamp(0.0, size[0] as f64) as usize;
    let bottom = ((y + height + MARGIN) * scale)
        .ceil()
        .clamp(0.0, size[1] as f64) as usize;
    (right > left && bottom > top).then_some([left, top, right - left, bottom - top])
}

/// Pictures kept; a new one takes the place of the oldest.
const KEPT: usize = 32;

fn prune(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut pictures: Vec<(u128, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let taken = name
                .to_str()?
                .strip_prefix("element-")?
                .strip_suffix(".png")?;
            Some((taken.parse().ok()?, entry.path()))
        })
        .collect();
    pictures.sort();
    let over = (pictures.len() + 1).saturating_sub(KEPT);
    for (_, path) in pictures.into_iter().take(over) {
        let _ = std::fs::remove_file(path);
    }
}

/// Save the picked element's part of the page as it was last painted. Runs on
/// the browser's reader, which owns the frame.
pub(super) fn capture(
    pick: &str,
    image: &egui::ColorImage,
    directory: &Path,
    name: u128,
) -> Option<PathBuf> {
    let [left, top, width, height] = region(pick, image.size)?;
    let mut pixels = Vec::with_capacity(width * height * 4);
    for row in top..top + height {
        let start = row * image.size[0] + left;
        for pixel in &image.pixels[start..start + width] {
            pixels.extend_from_slice(&pixel.to_srgba_unmultiplied());
        }
    }
    std::fs::create_dir_all(directory).ok()?;
    prune(directory);
    let path = directory.join(format!("element-{name}.png"));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .ok()?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let written = encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&pixels));
    if written.is_err() {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pick_is_described_with_only_the_parts_the_page_gave() {
        let pick = serde_json::json!({
            "url": "http://localhost:3000/settings",
            "tag": "button",
            "selector": "[data-testid=\"save\"]",
            "text": "Save",
            "html": "<button data-testid=\"save\">Save</button>",
            "styles": "display: flex;\ncolor: rgb(0, 0, 0);",
            "component": "SaveButton",
            "owners": ["Toolbar", "App"],
            "source": null,
            "rect": {"x": 10, "y": 20, "width": 80, "height": 24},
            "dpr": 2,
        })
        .to_string();
        assert_eq!(
            describe(&pick, Some(Path::new("/tmp/element-1.png"))).unwrap(),
            "Element picked in the browser preview\n\
             tag: button\n\
             url: http://localhost:3000/settings\n\
             selector: [data-testid=\"save\"]\n\
             component: SaveButton (in Toolbar > App)\n\
             text: Save\n\
             html:\n  <button data-testid=\"save\">Save</button>\n\
             styles:\n  display: flex;\n  color: rgb(0, 0, 0);\n\
             screenshot: /tmp/element-1.png\n"
        );
    }

    #[test]
    fn a_page_cannot_overrun_or_forge_lines_of_a_pick() {
        let pick = serde_json::json!({
            "tag": "div",
            "url": "http://a/\u{1b}[31m\rb",
            "selector": "x".repeat(5000),
            "text": 7,
            "owners": "App",
        })
        .to_string();
        let said = describe(&pick, None).unwrap();
        assert!(said.contains("url: http://a/[31mb\n"));
        assert!(said.contains(&format!("selector: {}\n", "x".repeat(512))));
        assert!(!said.contains("text:"));
        assert!(describe("{\"url\":\"http://a/\"}", None).is_none());
        assert!(describe("not json", None).is_none());
    }

    #[test]
    fn the_picture_is_the_element_with_a_margin_inside_the_frame() {
        let pick = |rect: [f64; 4], dpr: f64| {
            serde_json::json!({"rect": {"x": rect[0], "y": rect[1], "width": rect[2], "height": rect[3]}, "dpr": dpr})
                .to_string()
        };
        assert_eq!(
            region(&pick([100.0, 50.0, 40.0, 10.0], 2.0), [800, 600]),
            Some([160, 60, 160, 100])
        );
        // Clipped to the frame, and nothing for a place outside it.
        assert_eq!(
            region(&pick([0.0, 0.0, 4000.0, 10.0], 1.0), [800, 600]),
            Some([0, 0, 800, 30])
        );
        assert_eq!(
            region(&pick([900.0, 0.0, 10.0, 10.0], 1.0), [800, 600]),
            None
        );
        assert_eq!(
            region(&pick([0.0, 0.0, 10.0, 10.0], 100.0), [800, 600]),
            None
        );
        assert_eq!(
            region(&pick([f64::NAN, 0.0, 10.0, 10.0], 1.0), [800, 600]),
            None
        );
    }

    #[test]
    fn the_picture_is_saved_as_the_frame_showed_it() {
        let directory = tempfile::tempdir().unwrap();
        let mut image = egui::ColorImage::filled([64, 64], egui::Color32::from_rgb(1, 2, 3));
        image.pixels[0] = egui::Color32::from_rgb(9, 8, 7);
        let pick = serde_json::json!({"rect": {"x": 0, "y": 0, "width": 4, "height": 4}, "dpr": 1})
            .to_string();
        let path = capture(&pick, &image, directory.path(), 5).unwrap();
        let mut reader =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()))
                .read_info()
                .unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!((info.width, info.height), (24, 24));
        assert_eq!(&pixels[..8], [9, 8, 7, 255, 1, 2, 3, 255]);
        // Only the newest pictures are kept, and nothing else is touched.
        std::fs::write(directory.path().join("notes.txt"), "kept").unwrap();
        for name in 6..6 + KEPT as u128 {
            capture(&pick, &image, directory.path(), name).unwrap();
        }
        assert_eq!(
            std::fs::read_dir(directory.path()).unwrap().count(),
            KEPT + 1
        );
        assert!(!path.exists());
        assert!(
            directory
                .path()
                .join(format!("element-{}.png", 5 + KEPT))
                .exists()
        );
    }
}
