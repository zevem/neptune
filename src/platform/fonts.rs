//! Bundled fonts and bounded platform font discovery.

use eframe::egui::{self, FontFamily, FontId};
use std::{
    io::{self, Read},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender},
};

pub const DEFAULT_FAMILY: &str = "JetBrains Mono";
const REGULAR: &str = "Terminal";
const BOLD: &str = "TerminalBold";
const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;
const MAX_FAMILIES: usize = 512;

pub fn install(ctx: &egui::Context) {
    // The first frame needs no filesystem access. Installed fonts and fallbacks
    // arrive from the worker while the shell starts independently.
    ctx.set_fonts(bundled_definitions());
}

pub fn terminal_font(size: f32, bold: bool) -> FontId {
    FontId::new(
        size,
        FontFamily::Name(if bold { BOLD } else { REGULAR }.into()),
    )
}

struct Loaded {
    family: String,
    definitions: egui::FontDefinitions,
    available: bool,
    resolved: Option<String>,
    families: Option<Vec<String>>,
}

/// One font worker and bounded request/result slots. Rapid changes coalesce;
/// only the latest choice can install fonts. No font I/O runs during a frame.
#[derive(Default)]
pub struct Fonts {
    worker: Option<(SyncSender<String>, Receiver<Loaded>)>,
    requested: Option<String>,
    applied: Option<(String, bool)>,
    resolved: Option<String>,
    families: Option<Vec<String>>,
    invalidate: bool,
    failed: bool,
}

impl Fonts {
    pub fn families(&self) -> Option<&[String]> {
        self.families.as_deref()
    }

    pub fn loading(&self, family: &str) -> bool {
        !self.failed
            && self
                .applied
                .as_ref()
                .is_none_or(|(applied, _)| applied != family)
    }

    pub fn unavailable(&self, family: &str) -> bool {
        (self.failed && !family.eq_ignore_ascii_case(DEFAULT_FAMILY))
            || self
                .applied
                .as_ref()
                .is_some_and(|(applied, available)| applied == family && !available)
    }

    /// The installed family behind a shorthand or PostScript name, once loaded.
    pub fn resolved_family(&self, family: &str) -> Option<&str> {
        self.applied
            .as_ref()
            .filter(|(applied, available)| !self.failed && applied == family && *available)
            .and(self.resolved.as_deref())
    }

    #[cfg(test)]
    pub(crate) fn bundled() -> Self {
        Self {
            failed: true,
            families: Some(Vec::new()),
            applied: Some((DEFAULT_FAMILY.into(), true)),
            ..Default::default()
        }
    }

    /// Returns true on the pass after new definitions take effect, so every
    /// visible or hidden terminal can discard galleys and remeasure its grid.
    pub fn poll(&mut self, ctx: &egui::Context, family: &str) -> bool {
        let changed = std::mem::take(&mut self.invalidate);
        if self.failed {
            return changed;
        }
        if self.worker.is_none() {
            let (requests, receiver) = mpsc::sync_channel::<String>(1);
            let (sender, results) = mpsc::sync_channel(1);
            let wake = ctx.clone();
            let started = std::thread::Builder::new()
                .name("neptune-fonts".into())
                .spawn(move || {
                    let mut database = fontdb::Database::new();
                    database.load_system_fonts();
                    let catalog = catalog(&database);
                    drop(database);
                    let base = font_definitions();
                    let mut families = Some(catalog.iter().map(|font| font.name.clone()).collect());
                    while let Ok(mut family) = receiver.recv() {
                        while let Ok(latest) = receiver.try_recv() {
                            family = latest;
                        }
                        // Typed aliases and families beyond the bounded menu are
                        // resolved on demand, without retaining an unbounded database.
                        let extra = if !family.eq_ignore_ascii_case(DEFAULT_FAMILY)
                            && !catalog.iter().any(|font| font.matches(&family))
                        {
                            let mut database = fontdb::Database::new();
                            database.load_system_fonts();
                            resolve_family(&database, &family)
                        } else {
                            None
                        };
                        let candidates =
                            extra.as_ref().map(std::slice::from_ref).unwrap_or(&catalog);
                        let lookup = extra
                            .as_ref()
                            .map_or(family.as_str(), |font| font.name.as_str());
                        let (definitions, available) = definitions_for(&base, candidates, lookup);
                        let resolved = find_family(candidates, lookup)
                            .filter(|font| available && !font.name.eq_ignore_ascii_case(&family))
                            .map(|font| font.name.clone());
                        if sender
                            .send(Loaded {
                                family,
                                definitions,
                                available,
                                resolved,
                                families: families.take(),
                            })
                            .is_err()
                        {
                            break;
                        }
                        wake.request_repaint();
                    }
                });
            if started.is_err() {
                self.failed = true;
                self.families = Some(Vec::new());
                return changed;
            }
            self.worker = Some((requests, results));
        }
        let Some((requests, results)) = &self.worker else {
            return changed;
        };
        if self.requested.as_deref() != Some(family) {
            match requests.try_send(family.to_owned()) {
                Ok(()) => self.requested = Some(family.to_owned()),
                Err(mpsc::TrySendError::Full(_)) => {}
                Err(mpsc::TrySendError::Disconnected(_)) => self.failed = true,
            }
        }
        match results.try_recv() {
            Ok(loaded) => {
                if let Some(families) = loaded.families {
                    self.families = Some(families);
                }
                if loaded.family == family {
                    ctx.set_fonts(loaded.definitions);
                    self.applied = Some((loaded.family, loaded.available));
                    self.resolved = loaded.resolved;
                    self.invalidate = true;
                }
                // Retry a coalesced choice once the worker frees its slot.
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.failed = true;
                self.families.get_or_insert_default();
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if self.failed {
            ctx.set_fonts(bundled_definitions());
            self.invalidate = true;
            self.worker = None;
            self.families.get_or_insert_default();
        }
        changed
    }
}

struct InstalledFamily {
    name: String,
    regular: fontdb::FaceInfo,
    bold: fontdb::FaceInfo,
}

impl InstalledFamily {
    fn matches(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
            || self
                .regular
                .families
                .iter()
                .any(|(family, _)| family.eq_ignore_ascii_case(name))
            || self.regular.post_script_name.eq_ignore_ascii_case(name)
            || self.bold.post_script_name.eq_ignore_ascii_case(name)
    }
}

fn nerd_font_alias(name: &str) -> Option<String> {
    let (family, suffix) = name.rsplit_once(' ')?;
    let expanded = if suffix.eq_ignore_ascii_case("NF") {
        "Nerd Font"
    } else if suffix.eq_ignore_ascii_case("NFM") {
        "Nerd Font Mono"
    } else {
        return None;
    };
    Some(format!("{family} {expanded}"))
}

fn find_family<'a>(catalog: &'a [InstalledFamily], name: &str) -> Option<&'a InstalledFamily> {
    // An actual family or PostScript name always wins over an abbreviation.
    catalog.iter().find(|font| font.matches(name)).or_else(|| {
        let alias = nerd_font_alias(name)?;
        catalog.iter().find(|font| font.matches(&alias))
    })
}

fn is_terminal_face(face: &fontdb::FaceInfo) -> bool {
    face.style == fontdb::Style::Normal
        && (face.monospaced
            || face.families.iter().any(|(name, _)| {
                [" Nerd Font", " Nerd Font Mono", " NF", " NFM"]
                    .iter()
                    .any(|suffix| {
                        name.get(name.len().saturating_sub(suffix.len())..)
                            .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
                    })
            }))
}

fn resolve_family(database: &fontdb::Database, name: &str) -> Option<InstalledFamily> {
    let find = |name: &str| {
        database.faces().find(|face| {
            is_terminal_face(face)
                && (face
                    .families
                    .iter()
                    .any(|(family, _)| family.eq_ignore_ascii_case(name))
                    || face.post_script_name.eq_ignore_ascii_case(name))
        })
    };
    let face = find(name).or_else(|| find(&nerd_font_alias(name)?))?;
    let canonical = &face.families.first()?.0;
    let families = [fontdb::Family::Name(canonical)];
    let query = fontdb::Query {
        families: &families,
        ..Default::default()
    };
    let regular = database.face(database.query(&query)?)?;
    if !is_terminal_face(regular) {
        return None;
    }
    let bold = database
        .query(&fontdb::Query {
            weight: fontdb::Weight::BOLD,
            ..query
        })
        .and_then(|id| database.face(id))
        .filter(|face| is_terminal_face(face))
        .unwrap_or(regular);
    Some(InstalledFamily {
        name: canonical.clone(),
        regular: regular.clone(),
        bold: bold.clone(),
    })
}

fn catalog(database: &fontdb::Database) -> Vec<InstalledFamily> {
    let names: std::collections::BTreeSet<_> = database
        .faces()
        .filter(|face| is_terminal_face(face))
        .filter_map(|face| face.families.first().map(|(name, _)| name))
        .filter(|name| crate::config::valid_font_family(name) && name.as_str() != DEFAULT_FAMILY)
        .collect();
    names
        .into_iter()
        .filter_map(|name| resolve_family(database, name))
        .take(MAX_FAMILIES)
        .collect()
}

fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_FONT_BYTES as u64 {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FONT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FONT_BYTES {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    Ok(bytes)
}

fn face_data(face: &fontdb::FaceInfo) -> Option<egui::FontData> {
    let bytes = match &face.source {
        fontdb::Source::File(path) => read_bounded(path).ok()?,
        fontdb::Source::Binary(bytes) if bytes.as_ref().as_ref().len() <= MAX_FONT_BYTES => {
            bytes.as_ref().as_ref().to_vec()
        }
        _ => return None,
    };
    // A font may have been removed or replaced since discovery. Re-parse before
    // handing it to egui, which expects valid font bytes and collection indices.
    let current = ttf_parser::Face::parse(&bytes, face.index).ok()?;
    // Nerd Font icons can be wider than a cell and unset isFixedPitch. The
    // terminal's text still needs a complete, uniform ASCII grid.
    if !current.is_monospaced() && !fixed_text_metrics(&current) {
        return None;
    }
    let mut data = egui::FontData::from_owned(bytes);
    data.index = face.index;
    Some(data)
}

fn fixed_text_metrics(face: &ttf_parser::Face<'_>) -> bool {
    let advance = |character| {
        face.glyph_index(character)
            .and_then(|glyph| face.glyph_hor_advance(glyph))
    };
    let Some(width) = advance('M').filter(|width| *width > 0) else {
        return false;
    };
    (b' '..=b'~').all(|character| advance(char::from(character)) == Some(width))
}

fn definitions_for(
    base: &egui::FontDefinitions,
    catalog: &[InstalledFamily],
    family: &str,
) -> (egui::FontDefinitions, bool) {
    let mut definitions = base.clone();
    if family.eq_ignore_ascii_case(DEFAULT_FAMILY) {
        return (definitions, true);
    }
    let Some(font) = find_family(catalog, family) else {
        return (definitions, false);
    };
    let Some(regular) = face_data(&font.regular) else {
        return (definitions, false);
    };
    let bold = if font.bold.id == font.regular.id {
        regular.clone()
    } else {
        face_data(&font.bold).unwrap_or_else(|| regular.clone())
    };
    for (family, name, mut data, weight) in [
        (REGULAR, "SelectedTerminal", regular, 400.0_f32),
        (BOLD, "SelectedTerminalBold", bold, 700.0),
    ] {
        // Variable families can store regular and bold in the same face.
        if let Some(axis) = data
            .variation_axes()
            .into_iter()
            .find(|axis| axis.tag.to_be_bytes() == *b"wght")
        {
            data.tweak
                .coords
                .push(axis.tag, weight.clamp(axis.range.min, axis.range.max));
        }
        definitions.font_data.insert(name.into(), data.into());
        definitions
            .families
            .entry(FontFamily::Name(family.into()))
            .or_default()
            .insert(0, name.into());
    }
    (definitions, true)
}

/// Uses only bundled bytes, giving tests the same metrics on every desktop.
pub fn bundled_definitions() -> egui::FontDefinitions {
    font_definitions_with(&mut |_| Err(io::Error::from(io::ErrorKind::NotFound)))
}

fn font_definitions() -> egui::FontDefinitions {
    font_definitions_with(&mut read_bounded)
}

fn font_definitions_with(
    read: &mut impl FnMut(&Path) -> io::Result<Vec<u8>>,
) -> egui::FontDefinitions {
    let mut definitions = egui::FontDefinitions::default();
    crate::icons::register_fonts(&mut definitions);
    for (name, bytes) in [
        (
            "Geist",
            include_bytes!("../../assets/fonts/Geist-Regular.ttf").as_slice(),
        ),
        (
            "GeistMedium",
            include_bytes!("../../assets/fonts/Geist-Medium.ttf").as_slice(),
        ),
        (
            "GeistSemiBold",
            include_bytes!("../../assets/fonts/Geist-SemiBold.ttf").as_slice(),
        ),
        (
            "JetBrains",
            include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf").as_slice(),
        ),
        (
            "JetBrainsBold",
            include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf").as_slice(),
        ),
    ] {
        definitions
            .font_data
            .insert(name.into(), egui::FontData::from_static(bytes).into());
    }

    let mut terminal_fallbacks = Vec::new();
    let mut shared_fallbacks = Vec::new();
    if let Some(name) = load_font(&mut definitions, "SystemNerd", nerd_candidates(), 0, read) {
        terminal_fallbacks.push(name);
    }
    for (name, candidates, index) in system_fallback_candidates() {
        if let Some(name) = load_font(&mut definitions, name, candidates, index, read) {
            terminal_fallbacks.push(name.clone());
            shared_fallbacks.push(name);
        }
    }

    let mono = definitions
        .families
        .entry(FontFamily::Monospace)
        .or_default();
    mono.insert(0, "JetBrains".into());
    // The bundled terminal font owns Latin metrics. Installed Nerd symbols and
    // CJK faces are fallback only, so prompts do not change the normal grid.
    mono.splice(1..1, terminal_fallbacks);
    let terminal = mono.clone();
    definitions
        .families
        .insert(FontFamily::Name(REGULAR.into()), terminal);
    let mono = &definitions.families[&FontFamily::Monospace];
    let mut bold = mono.clone();
    bold.insert(0, "JetBrainsBold".into());
    definitions
        .families
        .insert(FontFamily::Name(BOLD.into()), bold);

    let proportional = definitions
        .families
        .entry(FontFamily::Proportional)
        .or_default();
    proportional.insert(0, "Geist".into());
    proportional.extend(shared_fallbacks);
    // Interface weights share the regular family's fallbacks, so emphasis never
    // changes which glyphs are available.
    let proportional = proportional.clone();
    for (family, face) in [
        (crate::theme::MEDIUM, "GeistMedium"),
        (crate::theme::SEMIBOLD, "GeistSemiBold"),
    ] {
        let mut fonts = proportional.clone();
        fonts[0] = face.into();
        definitions
            .families
            .insert(FontFamily::Name(family.into()), fonts);
    }
    definitions
}

/// Load installed fonts only. Patched Nerd fonts are never redistributed.
fn load_font(
    definitions: &mut egui::FontDefinitions,
    name: &str,
    candidates: Vec<PathBuf>,
    preferred_index: u32,
    read: &mut impl FnMut(&Path) -> io::Result<Vec<u8>>,
) -> Option<String> {
    for path in candidates {
        let Ok(bytes) = read(&path) else {
            continue;
        };
        let Some(signature) = bytes.get(..4) else {
            continue;
        };
        let index = match signature {
            b"ttcf" => {
                let Some(count) = bytes
                    .get(8..12)
                    .and_then(|data| data.try_into().ok())
                    .map(u32::from_be_bytes)
                else {
                    continue;
                };
                if count == 0 {
                    continue;
                }
                if preferred_index < count {
                    preferred_index
                } else {
                    0
                }
            }
            b"\x00\x01\x00\x00" | b"OTTO" | b"true" => 0,
            _ => continue,
        };
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        definitions.font_data.insert(name.into(), data.into());
        return Some(name.into());
    }
    None
}

fn nerd_candidates() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Some(user_font_home) = std::env::var_os("HOME") {
        let user_font_home = PathBuf::from(user_font_home);
        #[cfg(target_os = "linux")]
        roots.extend([
            user_font_home.join(".local/share/fonts"),
            user_font_home.join(".fonts"),
        ]);
        #[cfg(target_os = "macos")]
        roots.push(user_font_home.join("Library/Fonts"));
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(local_data) = std::env::var_os("LOCALAPPDATA") {
            roots.push(PathBuf::from(local_data).join("Microsoft/Windows/Fonts"));
        }
        roots.push(windows_fonts());
    }
    #[cfg(target_os = "linux")]
    roots.extend([
        PathBuf::from("/usr/local/share/fonts"),
        PathBuf::from("/usr/share/fonts/truetype"),
    ]);
    #[cfg(target_os = "macos")]
    roots.push(PathBuf::from("/Library/Fonts"));

    let names = [
        "MesloLGSNerdFontMono-Regular.ttf",
        "MesloLGMNerdFontMono-Regular.ttf",
        "MesloLGS NF Regular.ttf",
        "JetBrainsMonoNerdFontMono-Regular.ttf",
        "SymbolsNerdFontMono-Regular.ttf",
    ];
    let mut candidates = Vec::new();
    for root in roots {
        for directory in ["", "Meslo", "JetBrainsMono", "NerdFonts", "SymbolsNerdFont"] {
            for name in names {
                candidates.push(root.join(directory).join(name));
            }
        }
    }
    candidates
}

type SystemFallback = (&'static str, Vec<PathBuf>, u32);

fn system_fallback_candidates() -> Vec<SystemFallback> {
    #[cfg(target_os = "linux")]
    {
        vec![
            (
                "SystemSymbols",
                paths(&[
                    "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
                    "/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf",
                    "/usr/share/fonts/google-noto-sans-symbols-2-fonts/NotoSansSymbols2-Regular.ttf",
                ]),
                0,
            ),
            (
                "SystemSymbolsLegacy",
                paths(&[
                    "/usr/share/fonts/truetype/noto/NotoSansSymbols-Regular.ttf",
                    "/usr/share/fonts/noto/NotoSansSymbols-Regular.ttf",
                ]),
                0,
            ),
            (
                "SystemCjk",
                paths(&[
                    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
                    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
                    "/usr/share/fonts/google-noto-sans-cjk-fonts/NotoSansCJK-Regular.ttc",
                ]),
                2,
            ),
        ]
    }
    #[cfg(target_os = "macos")]
    {
        vec![
            (
                "SystemSymbols",
                paths(&["/System/Library/Fonts/Apple Symbols.ttf"]),
                0,
            ),
            (
                "SystemCjk",
                paths(&[
                    "/System/Library/Fonts/PingFang.ttc",
                    "/System/Library/Fonts/STHeiti Light.ttc",
                    "/Library/Fonts/NotoSansCJK-Regular.ttc",
                ]),
                0,
            ),
        ]
    }
    #[cfg(target_os = "windows")]
    {
        let root = windows_fonts();
        vec![
            ("SystemSymbols", vec![root.join("seguisym.ttf")], 0),
            (
                "SystemCjk",
                ["msyh.ttc", "Deng.ttf", "simhei.ttf", "msgothic.ttc"]
                    .map(|name| root.join(name))
                    .into(),
                0,
            ),
        ]
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    Vec::new()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn paths(candidates: &[&str]) -> Vec<PathBuf> {
    candidates.iter().map(PathBuf::from).collect()
}

#[cfg(target_os = "windows")]
fn windows_fonts() -> PathBuf {
    PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed_database() -> fontdb::Database {
        let mut database = fontdb::Database::new();
        for bytes in [
            include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf").as_slice(),
            include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf").as_slice(),
            include_bytes!("../../assets/fonts/Geist-Regular.ttf").as_slice(),
        ] {
            database.load_font_data(bytes.to_vec());
        }
        database
    }

    fn without_fixed_pitch(bytes: &[u8]) -> Vec<u8> {
        let mut bytes = bytes.to_vec();
        let tables = u16::from_be_bytes(bytes[4..6].try_into().unwrap());
        for table in 0..usize::from(tables) {
            let record = 12 + table * 16;
            if &bytes[record..record + 4] == b"post" {
                let offset = u32::from_be_bytes(bytes[record + 8..record + 12].try_into().unwrap());
                bytes[offset as usize + 12..offset as usize + 16].fill(0);
                return bytes;
            }
        }
        panic!("fixture has a post table");
    }

    fn nerd_database() -> fontdb::Database {
        let mut database = fontdb::Database::new();
        for bytes in [
            include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf").as_slice(),
            include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf").as_slice(),
        ] {
            database.load_font_data(without_fixed_pitch(bytes));
        }
        let faces: Vec<_> = database.faces().cloned().collect();
        for mut face in faces {
            assert!(
                !face.monospaced,
                "the real font bytes have the Nerd Font flag"
            );
            database.remove_face(face.id);
            face.id = fontdb::ID::dummy();
            face.families[0].0 = "MesloLGS Nerd Font".into();
            database.push_face_info(face);
        }
        database
    }

    #[test]
    fn nerd_font_short_name_loads_fixed_text_without_the_fixed_pitch_flag() {
        let database = nerd_database();
        let fonts = catalog(&database);
        assert_eq!(fonts.len(), 1, "terminal Nerd Font appears in the picker");
        assert_eq!(fonts[0].name, "MesloLGS Nerd Font");
        let base = bundled_definitions();
        for name in ["MesloLGS NF", "meslolgs nf", "MesloLGS Nerd Font"] {
            let font = resolve_family(&database, name).expect("installed Meslo font resolves");
            assert_eq!(font.name, "MesloLGS Nerd Font");
            assert_eq!(font.regular.weight, fontdb::Weight::NORMAL);
            assert!(font.bold.weight > font.regular.weight);
            assert_ne!(font.regular.id, font.bold.id);
            assert!(
                definitions_for(&base, &fonts, name).1,
                "selected font loads"
            );
        }
        assert!(resolve_family(&database, "Missing NF").is_none());
    }

    #[test]
    fn exact_font_names_take_priority_over_nerd_font_abbreviations() {
        let mut database = nerd_database();
        let expanded = resolve_family(&database, "MesloLGS Nerd Font").unwrap();
        let mut face = expanded.regular.clone();
        face.id = fontdb::ID::dummy();
        face.families[0].0 = "MesloLGS NF".into();
        database.push_face_info(face);
        let exact = resolve_family(&database, "MesloLGS NF").unwrap();
        assert_eq!(exact.name, "MesloLGS NF");
        // Put the expanded family first, so preference cannot depend on order.
        let fonts = [expanded, exact];
        assert_eq!(
            find_family(&fonts, "MesloLGS NF").unwrap().name,
            "MesloLGS NF"
        );
    }

    #[test]
    fn nfm_abbreviation_selects_the_mono_variant_and_excludes_propo() {
        let mut database = nerd_database();
        let faces: Vec<_> = database.faces().cloned().collect();
        for mut face in faces {
            database.remove_face(face.id);
            face.id = fontdb::ID::dummy();
            face.families[0].0 = "MesloLGS Nerd Font Mono".into();
            database.push_face_info(face);
        }
        let font = resolve_family(&database, "meslolgs nfm").unwrap();
        assert_eq!(font.name, "MesloLGS Nerd Font Mono");
        assert!(definitions_for(&bundled_definitions(), &[font], "MesloLGS NFM").1);

        database.load_font_data(include_bytes!("../../assets/fonts/Geist-Regular.ttf").to_vec());
        let mut face = database
            .faces()
            .find(|face| face.post_script_name == "Geist-Regular")
            .unwrap()
            .clone();
        database.remove_face(face.id);
        face.id = fontdb::ID::dummy();
        face.families[0].0 = "MesloLGS Nerd Font Propo".into();
        database.push_face_info(face);
        assert!(resolve_family(&database, "MesloLGS Nerd Font Propo").is_none());
    }

    #[test]
    fn proportional_fonts_cannot_bypass_validation_with_a_nerd_font_name() {
        let mut database = fontdb::Database::new();
        database.load_font_data(include_bytes!("../../assets/fonts/Geist-Regular.ttf").to_vec());
        let mut face = database.faces().next().unwrap().clone();
        database.remove_face(face.id);
        face.id = fontdb::ID::dummy();
        face.families[0].0 = "Fake Nerd Font".into();
        let regular = face.clone();
        database.push_face_info(face);
        let font = InstalledFamily {
            name: "Fake Nerd Font".into(),
            bold: regular.clone(),
            regular,
        };
        assert!(!definitions_for(&bundled_definitions(), &[font], "Fake Nerd Font").1);
    }

    #[test]
    fn typed_font_names_resolve_aliases_and_postscript_names_without_menu_entries() {
        let mut database = installed_database();
        let faces: Vec<_> = database
            .faces()
            .filter(|face| face.monospaced)
            .cloned()
            .collect();
        for mut face in faces {
            database.remove_face(face.id);
            face.id = fontdb::ID::dummy();
            face.families[0].0 = "Installed Mono".into();
            let language = face.families[0].1;
            face.families.push(("Alternate Mono".into(), language));
            database.push_face_info(face);
        }
        let base = bundled_definitions();
        for name in ["installed mono", "ALTERNATE MONO", "JetBrainsMono-Regular"] {
            let font = resolve_family(&database, name)
                .expect("typed name resolves independently of the menu");
            assert_eq!(font.name, "Installed Mono");
            assert!(definitions_for(&base, std::slice::from_ref(&font), name).1);
        }
        assert!(resolve_family(&database, "Geist").is_none());
        assert!(resolve_family(&database, "Missing Mono").is_none());
        assert!(definitions_for(&base, &[], "jetbrains mono").1);
    }

    #[test]
    fn catalog_excludes_proportional_fonts_and_pairs_regular_with_bold() {
        let mut database = installed_database();
        // Give the fixture an installed name distinct from the bundled default.
        let faces: Vec<_> = database
            .faces()
            .filter(|face| face.monospaced)
            .cloned()
            .collect();
        for mut face in faces {
            database.remove_face(face.id);
            face.id = fontdb::ID::dummy();
            face.families[0].0 = "Installed Mono".into();
            database.push_face_info(face);
        }
        let catalog = catalog(&database);
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].name, "Installed Mono");
        assert_eq!(catalog[0].regular.weight, fontdb::Weight::NORMAL);
        assert!(catalog[0].bold.weight > catalog[0].regular.weight);
        assert_ne!(catalog[0].bold.id, catalog[0].regular.id);
        let (definitions, available) =
            definitions_for(&bundled_definitions(), &catalog, "Installed Mono");
        assert!(available);
        for (family, selected) in [
            (REGULAR, "SelectedTerminal"),
            (BOLD, "SelectedTerminalBold"),
        ] {
            assert_eq!(
                definitions.families[&FontFamily::Name(family.into())][0],
                selected
            );
        }
        assert_eq!(definitions.families[&FontFamily::Proportional][0], "Geist");
        assert_eq!(definitions.families[&FontFamily::Monospace][0], "JetBrains");
    }

    #[test]
    fn unavailable_family_and_default_restore_bundled_fonts() {
        let base = bundled_definitions();
        let (missing, available) = definitions_for(&base, &[], "Removed Mono");
        assert!(!available);
        assert_eq!(
            missing.families[&FontFamily::Name(REGULAR.into())][0],
            "JetBrains"
        );
        let (default, available) = definitions_for(&base, &[], DEFAULT_FAMILY);
        assert!(available);
        assert_eq!(
            default.families[&FontFamily::Name(BOLD.into())][0],
            "JetBrainsBold"
        );
    }

    #[test]
    fn removed_font_and_invalid_collection_index_are_rejected() {
        let database = installed_database();
        let mut face = database.faces().next().unwrap().clone();
        face.index = u32::MAX;
        assert!(face_data(&face).is_none());
        let root = tempfile::tempdir().unwrap();
        face.index = 0;
        face.source = fontdb::Source::File(root.path().join("removed.ttf"));
        assert!(face_data(&face).is_none());
    }

    #[test]
    fn stale_worker_completion_keeps_catalog_without_installing_fonts() {
        let ctx = egui::Context::default();
        let (requests, _receiver) = mpsc::sync_channel(1);
        let (sender, results) = mpsc::sync_channel(1);
        let mut fonts = Fonts {
            worker: Some((requests, results)),
            requested: Some("Latest".into()),
            ..Default::default()
        };
        sender
            .send(Loaded {
                family: "Stale".into(),
                definitions: bundled_definitions(),
                available: true,
                resolved: Some("Stale installed family".into()),
                families: Some(vec!["Latest".into()]),
            })
            .unwrap();
        assert!(!fonts.poll(&ctx, "Latest"));
        assert_eq!(fonts.families(), Some(["Latest".to_owned()].as_slice()));
        assert!(fonts.loading("Latest"));
        assert_eq!(fonts.resolved_family("Latest"), None);
        sender
            .send(Loaded {
                family: "Latest".into(),
                definitions: bundled_definitions(),
                available: true,
                resolved: Some("Actual installed family".into()),
                families: None,
            })
            .unwrap();
        assert!(!fonts.poll(&ctx, "Latest"));
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        assert!(fonts.poll(&ctx, "Latest"));
        assert!(!fonts.poll(&ctx, "Latest"));
        assert!(!fonts.loading("Latest"));
        assert_eq!(
            fonts.resolved_family("Latest"),
            Some("Actual installed family")
        );
        assert_eq!(fonts.resolved_family("Another"), None);
    }

    #[test]
    fn full_request_slot_retries_the_latest_choice() {
        let ctx = egui::Context::default();
        let (requests, receiver) = mpsc::sync_channel(1);
        let (_sender, results) = mpsc::sync_channel(1);
        requests.send("First".into()).unwrap();
        let mut fonts = Fonts {
            worker: Some((requests, results)),
            requested: Some("First".into()),
            ..Default::default()
        };
        fonts.poll(&ctx, "Intermediate");
        fonts.poll(&ctx, "Latest");
        assert_eq!(receiver.recv().unwrap(), "First");
        fonts.poll(&ctx, "Latest");
        assert_eq!(receiver.recv().unwrap(), "Latest");
    }

    #[test]
    fn bundled_fonts_do_not_depend_on_installed_fallbacks() {
        let definitions = bundled_definitions();
        assert_eq!(definitions.families[&FontFamily::Monospace][0], "JetBrains");
        assert_eq!(
            definitions.families[&FontFamily::Name("TerminalBold".into())][0],
            "JetBrainsBold"
        );
        for (family, face) in [
            (crate::theme::MEDIUM, "GeistMedium"),
            (crate::theme::SEMIBOLD, "GeistSemiBold"),
        ] {
            assert_eq!(
                definitions.families[&FontFamily::Name(family.into())][0],
                face
            );
        }
        assert!(!definitions.font_data.contains_key("SystemNerd"));
        assert!(!definitions.font_data.contains_key("SystemCjk"));
    }

    #[test]
    fn tabler_families_survive_font_selection_without_entering_text_fallbacks() {
        let base = bundled_definitions();
        for selected in [DEFAULT_FAMILY, "Unavailable family"] {
            let (definitions, _) = definitions_for(&base, &[], selected);
            for name in ["Tabler Outline", "Tabler Filled"] {
                let icon_family = FontFamily::Name(name.into());
                assert!(definitions.font_data.contains_key(name));
                assert_eq!(definitions.families[&icon_family], [name]);
                for (family, fallbacks) in &definitions.families {
                    if *family != icon_family {
                        assert!(!fallbacks.iter().any(|fallback| fallback == name));
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_candidates_are_skipped_before_valid_font() {
        let mut definitions = egui::FontDefinitions::default();
        let mut read = |path: &Path| {
            Ok(if path == Path::new("broken") {
                vec![0; 16]
            } else {
                include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf").to_vec()
            })
        };
        assert_eq!(
            load_font(
                &mut definitions,
                "Candidate",
                vec!["broken".into(), "valid".into()],
                0,
                &mut read
            ),
            Some("Candidate".into())
        );
        assert_eq!(definitions.font_data["Candidate"].index, 0);
    }
}
