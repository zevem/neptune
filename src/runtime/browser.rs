//! Bounded process supervision and owned browser snapshots. No Chromium code
//! is linked into the terminal executable or called during interactive frames.
mod input;
mod pick;
mod pixels;
pub(crate) mod protocol;
mod tabs;

use eframe::egui;
use neptune_model::{Completion, PaneId};
use protocol::{Command, Event, State, Target};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command as Process, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(crate) struct View {
    pub state: State,
    pub texture: Option<egui::TextureId>,
    pub popup: Option<(egui::TextureId, egui::Rect)>,
    /// The page's own icon, for its tab.
    pub icon: Option<egui::TextureId>,
    pub failed: bool,
    /// This system allowed Chromium no sandbox, so the host runs without one.
    pub unsandboxed: bool,
    /// The saved profile this browser keeps its cookies in, or none for a
    /// private one.
    pub profile: Option<String>,
    /// This window holds the saved profiles; another Neptune may hold them.
    pub saved: bool,
    /// Other browsers found here to import cookies from, once looked for.
    pub sources: Option<Arc<Vec<crate::platform::browser_import::Source>>>,
    pub importing: bool,
}

/// The profile every Neptune has, kept in its data directory.
pub(crate) const DEFAULT_PROFILE: &str = "default";

/// What a browser finished for its user, for the application to hand over.
pub(crate) enum Outcome {
    /// An element's description for the clipboard, with its picture if kept.
    Picked(String),
    Recorded(Result<PathBuf, String>),
    Imported {
        imported: u32,
        skipped: u32,
    },
}

/// Where a window's browsers keep what outlives them.
#[derive(Clone, Default)]
struct Storage {
    /// Saved profiles, held by one Neptune at a time.
    profiles: Option<PathBuf>,
    /// Pictures of picked elements.
    captures: Option<PathBuf>,
}
#[derive(Clone)]
pub(crate) struct Frame {
    image: Arc<egui::ColorImage>,
    position: [i32; 2],
    size: [usize; 2],
    offset: [usize; 2],
}
#[derive(Default)]
struct Incoming {
    states: BTreeMap<Target, State>,
    frames: BTreeMap<(Target, bool), Frame>,
    /// A page's icon, or none where the page has none.
    icons: BTreeMap<Target, Option<egui::ColorImage>>,
    failed: bool,
    unsandboxed: bool,
    saved: bool,
    picks: Vec<(Target, String, Option<PathBuf>)>,
    recorded: Vec<(Target, Option<String>)>,
    imported: Option<(u32, u32)>,
}
struct Bridge {
    sender: Option<mpsc::SyncSender<Command>>,
    incoming: Arc<Mutex<Incoming>>,
    targets: Arc<Mutex<BTreeSet<Target>>>,
    stopping: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}
impl Bridge {
    fn launch(ctx: egui::Context, storage: Storage) -> Result<Self, &'static str> {
        let (sender, receiver) = mpsc::sync_channel(64);
        let incoming = Arc::new(Mutex::new(Incoming::default()));
        let targets = Arc::new(Mutex::new(BTreeSet::new()));
        let stopping = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let (updates, live, stop, finished) = (
            incoming.clone(),
            targets.clone(),
            stopping.clone(),
            done.clone(),
        );
        std::thread::Builder::new()
            .name("neptune-browser-host".into())
            .spawn(move || {
                if run(receiver, updates.clone(), live, stop, ctx.clone(), storage).is_err()
                    && let Ok(mut incoming) = updates.lock()
                {
                    incoming.failed = true;
                }
                finished.store(true, Ordering::Release);
                ctx.request_repaint();
            })
            .map_err(|_| "Could not start the browser supervisor.")?;
        Ok(Self {
            sender: Some(sender),
            incoming,
            targets,
            stopping,
            done,
        })
    }
    fn send(&self, command: Command) -> bool {
        self.sender
            .as_ref()
            .is_some_and(|sender| sender.try_send(command).is_ok())
    }
    fn stop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        self.sender.take();
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Entry {
    generation: u64,
    state: State,
    texture: Option<egui::TextureHandle>,
    popup: Option<(egui::TextureHandle, egui::Rect)>,
    icon: Option<egui::TextureHandle>,
    /// What a wheel or touchpad moved short of a whole point.
    wheel: egui::Vec2,
    opened: bool,
    completed: bool,
    failed: bool,
    visible: Option<bool>,
    focused: Option<bool>,
    geometry: Option<(u32, u32, f32)>,
    frame_rate: Option<u32>,
    navigation: Option<String>,
    profile: Option<String>,
    /// The file a recording under way is saved to.
    recording: Option<PathBuf>,
    buttons: u32,
    pointer_inside: bool,
    click: Option<(u8, Instant, egui::Pos2, i32)>,
}
#[derive(Default)]
pub(crate) struct Browsers {
    entries: BTreeMap<PaneId, Entry>,
    closing: BTreeMap<Target, bool>,
    bridge: Option<Bridge>,
    frame_rate: Option<u32>,
    unsandboxed: bool,
    saved: bool,
    storage: Storage,
    /// Cookies of an import the host has yet to be sent.
    cookies: VecDeque<Command>,
    outcomes: Vec<Outcome>,
    /// The profile of the pane that last closed, or the one it was told to
    /// change to: a restart opens the same pane again and keeps it.
    resume: Option<(PaneId, Option<String>)>,
    /// Where each tab that keeps its sign-ins is, for the next launch.
    tabs: tabs::Tabs,
}
impl Browsers {
    /// Keep saved profiles and captures under this directory. A window that
    /// was given none keeps every browser in memory, as a test launch does.
    pub fn store(&mut self, root: PathBuf) {
        self.tabs = tabs::Tabs::open(&root);
        self.storage = Storage {
            captures: Some(root.join("captures")),
            profiles: Some(root),
        };
    }
    pub fn outcomes(&mut self) -> Vec<Outcome> {
        std::mem::take(&mut self.outcomes)
    }
    fn entry(&mut self, pane: PaneId, generation: u64) -> Option<(&mut Entry, Target)> {
        let target = Target {
            pane: pane.get(),
            generation,
        };
        self.entries
            .get_mut(&pane)
            .filter(|e| e.generation == generation && e.opened && !e.failed)
            .map(|entry| (entry, target))
    }
    /// Start picking an element, or cancel a pick under way.
    pub fn pick(&mut self, pane: PaneId, generation: u64) -> Result<(), &'static str> {
        let Some((entry, target)) = self.entry(pane, generation) else {
            return Ok(());
        };
        let active = !entry.state.picking;
        self.send(Command::Pick { target, active })
    }
    /// Start recording the page, or end and save a recording under way.
    pub fn record(&mut self, pane: PaneId, generation: u64) -> Result<(), &'static str> {
        let directory = recordings();
        let Some((entry, target)) = self.entry(pane, generation) else {
            return Ok(());
        };
        if entry.state.recording || entry.recording.is_some() {
            return self.send(Command::Record { target, path: None });
        }
        let directory = directory.ok_or("Neptune found no folder to save a recording in.")?;
        std::fs::create_dir_all(&directory)
            .map_err(|_| "The folder for recordings could not be made.")?;
        let path = directory.join(format!("neptune-{}.webm", stamp()));
        let name = path
            .to_str()
            .ok_or("The folder for recordings has a name Neptune cannot use.")?;
        let command = Command::Record {
            target,
            path: Some(name.to_owned()),
        };
        entry.recording = Some(path);
        if let Err(error) = self.send(command) {
            if let Some((entry, _)) = self.entry(pane, generation) {
                entry.recording = None;
            }
            return Err(error);
        }
        Ok(())
    }
    /// Give a saved profile cookies read from another browser. They are sent
    /// as the host takes them; an [`Outcome::Imported`] follows.
    pub fn import(&mut self, profile: &str, cookies: Vec<protocol::Cookie>) {
        let mut parts = cookies.chunks(protocol::MAX_COOKIES).peekable();
        if parts.peek().is_none() {
            self.outcomes.push(Outcome::Imported {
                imported: 0,
                skipped: 0,
            });
        }
        while let Some(part) = parts.next() {
            self.cookies.push_back(Command::SetCookies {
                profile: profile.to_owned(),
                cookies: part.to_vec(),
                last: parts.peek().is_none(),
            });
        }
    }
    pub fn importing(&self) -> bool {
        !self.cookies.is_empty()
    }
    pub fn clear_cookies(&mut self, profile: &str) -> Result<(), &'static str> {
        self.send(Command::ClearCookies {
            profile: profile.to_owned(),
        })
    }
    /// Forget a profile: its cookies go now, and what else it kept when the
    /// browser host next starts, since a running one holds the files.
    pub fn remove_profile(&mut self, profile: &str) {
        if !protocol::valid_profile(profile) || profile == DEFAULT_PROFILE {
            return;
        }
        let _ = self.clear_cookies(profile);
        if let Some(root) = self.storage.profiles.as_ref().filter(|_| self.saved) {
            let removed = root.join("removed");
            let _ = std::fs::create_dir_all(&removed)
                .and_then(|()| std::fs::write(removed.join(profile), []));
        }
    }
    /// The saved profile a browser pane uses, if any.
    pub fn profile(&self, pane: PaneId) -> Option<&str> {
        self.entries.get(&pane)?.profile.as_deref()
    }
    /// Have a pane's next start use another profile. Chromium fixes a
    /// browser's storage when it is made, so the caller restarts the pane.
    pub fn switch(&mut self, pane: PaneId, profile: Option<String>) {
        self.resume = Some((pane, profile));
    }
    pub fn has_previews(&self) -> bool {
        !self.entries.is_empty()
    }
    pub fn display_rate(&mut self, millihertz: Option<u32>) {
        self.frame_rate = Some(
            millihertz
                .filter(|rate| *rate > 0)
                .map(|rate| {
                    ((rate as f64 / 1000.0).round() as u32).clamp(1, protocol::MAX_FRAME_RATE)
                })
                .unwrap_or(protocol::DEFAULT_FRAME_RATE),
        );
    }
    pub fn start(
        &mut self,
        ctx: &egui::Context,
        pane: PaneId,
        generation: u64,
        profile: Option<String>,
    ) -> Result<(), &'static str> {
        let replacement =
            self.entries.contains_key(&pane) || self.closing.keys().any(|t| t.pane == pane.get());
        if !replacement && self.entries.len() + self.closing.len() >= protocol::MAX_BROWSERS {
            return Err(
                "Browser capacity is reserved while previews are closing. Try again in a moment.",
            );
        }
        self.release(pane);
        // A tab the last run left opens where it was, in its profile; one
        // being restarted keeps the profile it had or was changed to.
        let restored = self.tabs.get(pane.get()).cloned();
        let profile = match self.resume.take() {
            Some((resumed, profile)) if resumed == pane => profile,
            _ => restored.as_ref().map(|tab| tab.profile.clone()).or(profile),
        }
        .filter(|id| protocol::valid_profile(id));
        if self
            .bridge
            .as_ref()
            .is_some_and(|b| b.done.load(Ordering::Acquire))
        {
            self.bridge = None;
            self.closing.clear();
        }
        if self.bridge.is_none() {
            self.bridge = Some(Bridge::launch(ctx.clone(), self.storage.clone())?);
            self.saved = false;
        }
        self.entries.insert(
            pane,
            Entry {
                generation,
                state: State::default(),
                texture: None,
                icon: None,
                wheel: egui::Vec2::ZERO,
                popup: None,
                opened: false,
                completed: false,
                failed: false,
                visible: None,
                focused: None,
                geometry: None,
                frame_rate: None,
                navigation: restored.map(|tab| tab.url),
                profile,
                recording: None,
                buttons: 0,
                pointer_inside: false,
                click: None,
            },
        );
        ctx.request_repaint();
        Ok(())
    }
    /// The pane is gone, and with it the page to come back to. A restart
    /// names its page again when it opens.
    pub fn close(&mut self, pane: PaneId) {
        self.release(pane);
        self.tabs.note(pane.get(), None);
    }
    fn release(&mut self, pane: PaneId) {
        if let Some(entry) = self.entries.remove(&pane) {
            if self
                .resume
                .as_ref()
                .is_none_or(|(resumed, _)| *resumed != pane)
            {
                self.resume = Some((pane, entry.profile.clone()));
            }
            if !entry.opened {
                return;
            }
            self.closing.insert(
                Target {
                    pane: pane.get(),
                    generation: entry.generation,
                },
                false,
            );
        }
    }
    pub fn view(&self, pane: PaneId, generation: u64) -> Option<View> {
        let entry = self
            .entries
            .get(&pane)
            .filter(|e| e.generation == generation)?;
        Some(View {
            state: entry.state.clone(),
            texture: entry.texture.as_ref().map(egui::TextureHandle::id),
            popup: entry
                .popup
                .as_ref()
                .map(|(texture, rect)| (texture.id(), *rect)),
            icon: entry.icon.as_ref().map(egui::TextureHandle::id),
            failed: entry.failed,
            unsandboxed: self.unsandboxed,
            profile: entry.profile.clone().filter(|_| self.saved),
            saved: self.saved,
            sources: None,
            importing: !self.cookies.is_empty(),
        })
    }
    pub fn address(&self, pane: PaneId, generation: u64) -> Option<String> {
        let entry = self
            .entries
            .get(&pane)
            .filter(|e| e.generation == generation)?;
        Some(
            entry
                .navigation
                .as_ref()
                .unwrap_or(&entry.state.url)
                .clone(),
        )
    }
    pub fn send(&self, command: Command) -> Result<(), &'static str> {
        match &command {
            Command::Text { text, .. }
            | Command::Paste { text, .. }
            | Command::Ime { text, .. }
            | Command::Find { text, .. }
                if text.len() > protocol::MAX_TEXT =>
            {
                return Err("Browser text input is limited to 64 KiB per operation.");
            }
            _ => {}
        }
        if self
            .bridge
            .as_ref()
            .is_some_and(|bridge| bridge.send(command))
        {
            Ok(())
        } else {
            Err("The browser is busy or has stopped. Try again in a moment.")
        }
    }
    pub fn navigate(
        &mut self,
        pane: PaneId,
        generation: u64,
        address: &str,
    ) -> Result<(), &'static str> {
        let address = address.trim();
        let url = if address.starts_with("http://")
            || address.starts_with("https://")
            || address == "about:blank"
        {
            address.to_owned()
        } else if address.contains("://") {
            return Err("Enter an HTTP or HTTPS address.");
        } else if address.starts_with("localhost")
            || address.starts_with("127.")
            || address.starts_with("[::1]")
        {
            format!("http://{address}")
        } else {
            format!("https://{address}")
        };
        if url.len() > protocol::MAX_URL || url.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err("Enter a valid web address of at most 8192 bytes.");
        }
        let entry = self
            .entries
            .get_mut(&pane)
            .filter(|e| e.generation == generation)
            .ok_or("This browser pane has closed.")?;
        entry.navigation = Some(url);
        Ok(())
    }
    pub fn geometry(&mut self, pane: PaneId, generation: u64, size: egui::Vec2, scale: f32) {
        // The page is painted in the pane's own physical pixels, so a frame
        // meets the screen one to one. A pane too large for a frame is
        // painted at a lower scale and stretched.
        let points = size.max(egui::Vec2::splat(1.0));
        let scale = scale
            .min((protocol::MAX_PIXELS as f32 / (points.x * points.y)).sqrt() * 0.999)
            .min(protocol::MAX_SIDE as f32 / points.max_elem())
            .clamp(0.25, 4.0);
        let width = ((points.x * scale).round() as u32).clamp(1, protocol::MAX_SIDE);
        let height = ((points.y * scale).round() as u32).clamp(1, protocol::MAX_SIDE);
        let target = Target {
            pane: pane.get(),
            generation,
        };
        let geometry = (width, height, scale);
        let Some(entry) = self
            .entries
            .get_mut(&pane)
            .filter(|e| e.generation == generation && e.opened)
        else {
            return;
        };
        if entry.geometry != Some(geometry)
            && self.bridge.as_ref().is_some_and(|b| {
                b.send(Command::Resize {
                    target,
                    width,
                    height,
                    scale,
                })
            })
        {
            entry.geometry = Some(geometry);
        }
    }
    pub fn poll(
        &mut self,
        ctx: &egui::Context,
        shown: &[PaneId],
        focused: Option<PaneId>,
    ) -> Vec<Completion> {
        let shown = if ctx.input(|input| input.viewport().visible()) == Some(false) {
            &[][..]
        } else {
            shown
        };
        let mut completions = Vec::new();
        if let Some(wait) = self.tabs.settle() {
            ctx.request_repaint_after(wait);
        }
        if self
            .bridge
            .as_ref()
            .is_some_and(|b| b.done.load(Ordering::Acquire) && b.stopping.load(Ordering::Acquire))
        {
            self.bridge = None;
        }
        if self.bridge.is_none() && !self.entries.is_empty() {
            match Bridge::launch(ctx.clone(), self.storage.clone()) {
                Ok(bridge) => {
                    self.bridge = Some(bridge);
                    self.unsandboxed = false;
                    self.saved = false;
                }
                Err(error) => {
                    for (pane, entry) in &mut self.entries {
                        if !entry.failed {
                            entry.failed = true;
                            entry.state.error = Some(error.into());
                            completions.push(Completion::Failed {
                                pane: *pane,
                                generation: entry.generation,
                                error: error.into(),
                            });
                        }
                    }
                }
            }
        }
        let Some(bridge) = &self.bridge else {
            return completions;
        };
        let incoming = bridge
            .incoming
            .try_lock()
            .ok()
            .map(|mut data| std::mem::take(&mut *data));
        if let Some(incoming) = incoming {
            self.unsandboxed |= incoming.unsandboxed;
            self.saved |= incoming.saved;
            for (target, pick, screenshot) in incoming.picks {
                if self
                    .entries
                    .get(&PaneId::new(target.pane))
                    .is_some_and(|e| e.generation == target.generation)
                    && let Some(said) = pick::describe(&pick, screenshot.as_deref())
                {
                    self.outcomes.push(Outcome::Picked(said));
                }
            }
            for (target, error) in incoming.recorded {
                if let Some(path) = self
                    .entries
                    .get_mut(&PaneId::new(target.pane))
                    .filter(|e| e.generation == target.generation)
                    .and_then(|e| e.recording.take())
                {
                    self.outcomes
                        .push(Outcome::Recorded(error.map_or(Ok(path), Err)));
                }
            }
            if let Some((imported, skipped)) = incoming.imported {
                self.outcomes.push(Outcome::Imported { imported, skipped });
            }
            for (target, state) in incoming.states {
                if state.closed {
                    self.closing.remove(&target);
                    if let Ok(mut live) = bridge.targets.try_lock() {
                        live.remove(&target);
                    }
                }
                if let Some(entry) = self
                    .entries
                    .get_mut(&PaneId::new(target.pane))
                    .filter(|e| e.generation == target.generation)
                {
                    entry.state = state;
                    // Only this window's saved profiles are remembered: a
                    // private tab leaves no trace of where it went.
                    if self.saved && entry.state.ready && !entry.state.closed {
                        match &entry.profile {
                            // A blank page on the way to one is not a move.
                            Some(profile) if entry.state.url.starts_with("http") => {
                                self.tabs.note(
                                    target.pane,
                                    Some(tabs::Tab {
                                        url: entry.state.url.clone(),
                                        profile: profile.clone(),
                                    }),
                                );
                            }
                            Some(_) => {}
                            None => self.tabs.note(target.pane, None),
                        }
                    }
                    if entry.state.closed && !entry.failed {
                        entry.failed = true;
                        entry.state.error.get_or_insert_with(|| {
                            "The preview closed unexpectedly. Restart to try again.".into()
                        });
                        completions.push(Completion::Failed {
                            pane: PaneId::new(target.pane),
                            generation: target.generation,
                            error: "Browser creation failed".into(),
                        });
                    } else if entry.state.ready && !entry.completed {
                        entry.completed = true;
                        completions.push(Completion::Started {
                            pane: PaneId::new(target.pane),
                            generation: target.generation,
                        });
                    }
                }
            }
            for (target, icon) in incoming.icons {
                if let Some(entry) = self
                    .entries
                    .get_mut(&PaneId::new(target.pane))
                    .filter(|e| e.generation == target.generation)
                {
                    entry.icon = icon.map(|icon| {
                        ctx.load_texture(
                            format!("browser-icon-{}", target.pane),
                            icon,
                            egui::TextureOptions::LINEAR,
                        )
                    });
                }
            }
            for ((target, popup), frame) in incoming.frames {
                let Some(entry) = self
                    .entries
                    .get_mut(&PaneId::new(target.pane))
                    .filter(|e| e.generation == target.generation)
                else {
                    continue;
                };
                if popup {
                    if frame.size == [0, 0] {
                        entry.popup = None;
                        continue;
                    }
                    let scale = entry.geometry.map_or(1.0, |g| g.2);
                    let size = egui::vec2(frame.size[0] as f32, frame.size[1] as f32) / scale;
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(frame.position[0] as f32, frame.position[1] as f32) / scale,
                        size,
                    );
                    entry.popup = frame
                        .texture(
                            ctx,
                            entry.popup.take().map(|p| p.0),
                            format!("browser-popup-{}", target.pane),
                        )
                        .map(|texture| (texture, rect));
                } else if entry.visible == Some(true) && shown.contains(&PaneId::new(target.pane)) {
                    entry.texture = frame.texture(
                        ctx,
                        entry.texture.take(),
                        format!("browser-{}", target.pane),
                    );
                }
            }
            if incoming.failed
                || bridge.done.load(Ordering::Acquire) && !bridge.stopping.load(Ordering::Acquire)
            {
                self.closing.clear();
                for (pane, entry) in &mut self.entries {
                    if !entry.failed {
                        entry.failed = true;
                        entry.opened = false;
                        entry.recording = None;
                        entry.state.loading = false;
                        entry.state.picking = false;
                        entry.state.recording = false;
                        entry.state.error = Some(
                            "The browser host stopped. Restart this preview to try again.".into(),
                        );
                        completions.push(if entry.completed {
                            Completion::Exited {
                                pane: *pane,
                                generation: entry.generation,
                            }
                        } else {
                            Completion::Failed {
                                pane: *pane,
                                generation: entry.generation,
                                error: "Browser host unavailable".into(),
                            }
                        });
                    }
                }
            }
        }
        for (target, sent) in &mut self.closing {
            if !*sent {
                *sent = bridge.send(Command::Close { target: *target });
            }
        }
        let mut reserved = self.entries.values().filter(|e| e.opened).count() + self.closing.len();
        for (pane, entry) in &mut self.entries {
            let target = Target {
                pane: pane.get(),
                generation: entry.generation,
            };
            if !entry.opened
                && !entry.failed
                && reserved < protocol::MAX_BROWSERS
                && let Ok(mut live) = bridge.targets.try_lock()
            {
                live.insert(target);
                // A host without the saved profiles opens it in memory.
                if bridge.send(Command::Open {
                    target,
                    profile: entry.profile.clone(),
                }) {
                    entry.opened = true;
                    reserved += 1;
                }
            }
            if !entry.opened || entry.failed {
                continue;
            }
            let frame_rate = self.frame_rate.unwrap_or(protocol::DEFAULT_FRAME_RATE);
            if entry.frame_rate != Some(frame_rate)
                && bridge.send(Command::FrameRate {
                    target,
                    frames_per_second: frame_rate,
                })
            {
                entry.frame_rate = Some(frame_rate);
            }
            if entry.completed
                && let Some(url) = entry.navigation.take()
                && !bridge.send(Command::Navigate {
                    target,
                    url: url.clone(),
                })
            {
                entry.navigation = Some(url);
            }
            let visible = shown.contains(pane);
            if entry.visible != Some(visible) && bridge.send(Command::Visible { target, visible }) {
                entry.visible = Some(visible);
            }
            let owns_focus = visible && focused == Some(*pane) && ctx.input(|i| i.focused);
            if entry.focused != Some(owns_focus)
                && bridge.send(Command::Focus {
                    target,
                    focused: owns_focus,
                })
            {
                entry.focused = Some(owns_focus);
            }
        }
        // An import goes out as the host's queue has room for it.
        for _ in 0..8 {
            let Some(command) = self.cookies.pop_front() else {
                break;
            };
            if let Some(sender) = &bridge.sender
                && let Err(
                    mpsc::TrySendError::Full(command) | mpsc::TrySendError::Disconnected(command),
                ) = sender.try_send(command)
            {
                self.cookies.push_front(command);
                break;
            }
        }
        if !self.cookies.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(20));
        }
        if let Ok(mut live) = bridge.targets.try_lock() {
            live.retain(|t| {
                self.closing.contains_key(t)
                    || self
                        .entries
                        .get(&PaneId::new(t.pane))
                        .is_some_and(|e| e.generation == t.generation && e.opened)
            });
        }
        if self.entries.values().any(|e| !e.opened && !e.failed) || !self.closing.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(20));
        }
        if self.entries.is_empty()
            && self.closing.is_empty()
            && let Some(bridge) = &mut self.bridge
        {
            bridge.stop();
        }
        completions
    }
    pub fn shutdown(&mut self) {
        self.tabs.flush();
        if let Some(mut bridge) = self.bridge.take() {
            bridge.stop();
            let deadline = Instant::now() + Duration::from_secs(6);
            while !bridge.done.load(Ordering::Acquire) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

fn run(
    receiver: mpsc::Receiver<Command>,
    incoming: Arc<Mutex<Incoming>>,
    targets: Arc<Mutex<BTreeSet<Target>>>,
    stopping: Arc<AtomicBool>,
    ctx: egui::Context,
    storage: Storage,
) -> std::io::Result<()> {
    let executable = std::env::var_os("NEPTUNE_BROWSER_HOST")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::current_exe().ok().and_then(|p| {
                p.parent().map(|parent| {
                    let name = if cfg!(windows) {
                        "neptune_browser.exe"
                    } else {
                        "neptune-browser"
                    };
                    let sibling = parent.join(name);
                    if sibling.is_file() {
                        sibling
                    } else if cfg!(windows) {
                        parent.join("browser").join(name)
                    } else if cfg!(target_os = "linux") {
                        parent.join("../lib/neptune/browser").join(name)
                    } else {
                        sibling
                    }
                })
            })
        });
    let Some(executable) = executable else {
        return Err(std::io::Error::other("Browser helper unavailable"));
    };
    // The supervisor owns this root through process teardown, so renderer or
    // host crashes cannot leave profiles behind. Creation is outside UI frames.
    let mut profile_builder = tempfile::Builder::new();
    profile_builder.prefix("neptune-browser-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        profile_builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    // Chromium lets one process use a profile directory. The Neptune that
    // locks them first keeps saved profiles; any other runs as before, with
    // browsers that forget.
    let held = storage.profiles.as_deref().and_then(hold);
    let profile = match &held {
        Some(_) => None,
        None => Some(profile_builder.tempdir()?),
    };
    let root = match (&profile, &storage.profiles) {
        (Some(temporary), _) => temporary.path().to_path_buf(),
        (None, Some(saved)) => saved.clone(),
        (None, None) => return Err(std::io::Error::other("Browser profile unavailable")),
    };
    if held.is_some()
        && let Ok(mut updates) = incoming.lock()
    {
        updates.saved = true;
    }
    #[cfg(target_os = "linux")]
    let unsandboxed = sandbox_unavailable(&executable);
    let mut process = Process::new(executable);
    // Chromium aborts at startup when the system allows it neither sandbox,
    // as an AppImage on Ubuntu does. Previews then run without it; any other
    // startup failure keeps the sandbox and shows the error screen.
    #[cfg(target_os = "linux")]
    if unsandboxed {
        process.arg("--no-sandbox");
        if let Ok(mut updates) = incoming.lock() {
            updates.unsandboxed = true;
        }
    }
    #[cfg(target_os = "linux")]
    if let Some(libraries) = bundled_libraries(process.get_program().as_ref()) {
        process.env("LD_LIBRARY_PATH", libraries);
    }
    if held.is_some() {
        process.env("NEPTUNE_BROWSER_SAVED", "1");
    }
    process
        .env("NEPTUNE_BROWSER_PROFILE", &root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        process.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        process.creation_flags(0x0800_0000);
    }
    let mut child = ChildGuard {
        child: process.spawn()?,
        terminated: false,
    };
    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(std::io::Error::other("Browser pipe unavailable"));
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(std::io::Error::other("Browser pipe unavailable"));
    };
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        unsafe extern "C" {
            fn fcntl(fd: i32, command: i32, ...) -> i32;
        }
        // Linux F_GETPIPE_SZ / F_SETPIPE_SZ. This worker exclusively owns the
        // new child pipe; a bounded 1 MiB window reduces context switches for
        // large frames. Restricted kernels retain their original capacity.
        // SAFETY: the descriptor is live and both commands take integer args.
        unsafe {
            if fcntl(stdout.as_raw_fd(), 1032) < 1024 * 1024 {
                let _ = fcntl(stdout.as_raw_fd(), 1031, 1024_i32 * 1024);
            }
        }
    }
    let process_done = Arc::new(AtomicBool::new(false));
    let writer_done = process_done.clone();
    let writer = std::thread::Builder::new()
        .name("neptune-browser-input".into())
        .spawn(move || -> std::io::Result<()> {
            while !writer_done.load(Ordering::Acquire) {
                let command = match receiver.recv_timeout(Duration::from_millis(20)) {
                    Ok(command) => command,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                serde_json::to_writer(&mut stdin, &command)?;
                stdin.write_all(b"\n")?;
                stdin.flush()?;
            }
            serde_json::to_writer(&mut stdin, &Command::Shutdown)?;
            stdin.write_all(b"\n")?;
            stdin.flush()
        })?;
    let reader_ctx = ctx.clone();
    let reader = std::thread::Builder::new()
        .name("neptune-browser-output".into())
        .spawn(move || read(stdout, incoming, targets, reader_ctx, storage.captures))?;
    let mut since = None;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if stopping.load(Ordering::Acquire) {
            let since = *since.get_or_insert(Instant::now());
            if since.elapsed() > Duration::from_secs(5) {
                let _ = child.kill();
                break child.wait()?;
            }
        }
        if reader.is_finished() || writer.is_finished() && !stopping.load(Ordering::Acquire) {
            let _ = child.kill();
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    child.terminate();
    // Closing the process releases blocked pipe workers. The supervisor, never
    // the UI, joins them and accounts for teardown.
    process_done.store(true, Ordering::Release);
    let _ = writer.join();
    let _ = reader.join();
    if status.success() && stopping.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err(std::io::Error::other("Browser host stopped"))
    }
}

/// Take the saved profiles for this process, and finish removing those the
/// user removed while a host still had their files open.
fn hold(root: &Path) -> Option<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)
            .ok()?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(root).ok()?;
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("lock"))
        .ok()?;
    lock.try_lock().ok()?;
    for removed in std::fs::read_dir(root.join("removed"))
        .into_iter()
        .flatten()
        .flatten()
    {
        if let Some(id) = removed.file_name().to_str()
            && protocol::valid_profile(id)
            && id != DEFAULT_PROFILE
        {
            let profile = root.join(protocol::profile_directory(id));
            if !profile.exists() || std::fs::remove_dir_all(&profile).is_ok() {
                let _ = std::fs::remove_file(removed.path());
            }
        }
    }
    Some(lock)
}

/// Where recordings are saved: the user's videos, else beside their profile.
fn recordings() -> Option<PathBuf> {
    let user = directories::UserDirs::new()?;
    Some(
        user.video_dir()
            .map_or_else(|| user.home_dir().join("Videos"), Path::to_path_buf)
            .join("Neptune"),
    )
}

/// A name for a file made now, unlike one made a moment ago.
fn stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis())
}

/// Whether this system allows Chromium neither of its Linux sandboxes: a
/// root-owned setuid `chrome-sandbox` beside the host, or a user namespace in
/// which this user may map itself. Runs on the supervisor, outside frames.
#[cfg(target_os = "linux")]
fn sandbox_unavailable(host: &std::path::Path) -> bool {
    use std::os::unix::{fs::MetadataExt, process::CommandExt};
    if std::fs::metadata(host.with_file_name("chrome-sandbox"))
        .is_ok_and(|helper| helper.uid() == 0 && helper.mode() & 0o4001 == 0o4001)
    {
        return false;
    }
    unsafe extern "C" {
        fn getuid() -> u32;
        fn unshare(flags: i32) -> i32;
        fn open(path: *const std::ffi::c_char, flags: i32, ...) -> i32;
        fn write(fd: i32, bytes: *const std::ffi::c_void, count: usize) -> isize;
    }
    const CLONE_NEWUSER: i32 = 0x1000_0000;
    const O_WRONLY: i32 = 1;
    const ECANCELED: i32 = 125;
    // SAFETY: getuid has no preconditions and cannot fail.
    let map = format!("0 {} 1", unsafe { getuid() });
    let mut probe = Process::new(host);
    probe
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The forked child enters a user namespace and maps its own user, as
    // Chromium's check does, then always fails the launch: nothing is executed.
    // Ubuntu's AppArmor restriction allows the namespace and denies the map.
    // SAFETY: only async-signal-safe system calls run in the forked child.
    unsafe {
        probe.pre_exec(move || {
            if unshare(CLONE_NEWUSER) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let fd = open(c"/proc/self/uid_map".as_ptr(), O_WRONLY);
            if fd < 0 || write(fd, map.as_ptr().cast(), map.len()) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Err(std::io::Error::from_raw_os_error(ECANCELED))
        });
    }
    match probe.spawn() {
        Ok(mut child) => {
            let _ = child.kill();
            let _ = child.wait();
            false
        }
        // EPERM, EACCES, EINVAL, ENOSPC, ENOSYS and EUSERS are how a kernel or
        // its policy refuses the namespace. A fork that failed for want of
        // memory or processes says nothing, and must not cost the sandbox.
        Err(error) => matches!(error.raw_os_error(), Some(1 | 13 | 22 | 28 | 38 | 87)),
    }
}

/// The search path that puts an AppImage's own Chromium runtime libraries
/// first, when this system cannot load the host with the libraries it has.
/// Systems that can keep their own, which match their graphics and NSS setup.
#[cfg(target_os = "linux")]
fn bundled_libraries(host: &std::path::Path) -> Option<std::ffi::OsString> {
    if !crate::platform::host_env::appimage() {
        return None;
    }
    // `usr/lib`, three levels above `usr/lib/neptune/browser/neptune-browser`.
    let bundled = host.ancestors().nth(3)?;
    // glibc's loader lists what it would load, and what it cannot find, without
    // running the program. Neptune's own launch environment is already restored.
    let listed = Process::new(host)
        .env("LD_TRACE_LOADED_OBJECTS", "1")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !String::from_utf8_lossy(&listed.stdout).contains("=> not found") {
        return None;
    }
    // An empty entry would name the working directory.
    let host_path = std::env::var_os("LD_LIBRARY_PATH").unwrap_or_default();
    std::env::join_paths(
        std::iter::once(bundled.to_path_buf())
            .chain(std::env::split_paths(&host_path).filter(|entry| !entry.as_os_str().is_empty())),
    )
    .ok()
}

/// A startup or pipe error must also reap the process and its Unix children.
struct ChildGuard {
    child: std::process::Child,
    terminated: bool,
}
impl std::ops::Deref for ChildGuard {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.child
    }
}
impl std::ops::DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}
impl ChildGuard {
    fn terminate(&mut self) {
        if std::mem::replace(&mut self.terminated, true) {
            return;
        }
        #[cfg(unix)]
        {
            unsafe extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            // This process group was created specifically for this child. No
            // application-name or worktree matching is used for teardown.
            if let Ok(pid) = i32::try_from(self.child.id()) {
                unsafe {
                    kill(-pid, 9);
                }
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn read(
    stdout: impl Read,
    incoming: Arc<Mutex<Incoming>>,
    targets: Arc<Mutex<BTreeSet<Target>>>,
    ctx: egui::Context,
    captures: Option<PathBuf>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stdout);
    let mut surfaces: BTreeMap<(Target, bool), pixels::Surface> = BTreeMap::new();
    let mut bytes = Vec::new();
    loop {
        let mut line = Vec::new();
        let count = reader
            .by_ref()
            .take(protocol::MAX_MESSAGE as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if count == 0 {
            return Ok(());
        }
        if line.len() > protocol::MAX_MESSAGE || line.last() != Some(&b'\n') {
            return Err(std::io::Error::other("Invalid browser message"));
        }
        let event: Event = serde_json::from_slice(&line)
            .map_err(|_| std::io::Error::other("Invalid browser message"))?;
        match event {
            Event::Picked { target, pick } => {
                if pick.len() > protocol::MAX_PICK {
                    return Err(std::io::Error::other("Browser pick limit"));
                }
                if targets.lock().is_ok_and(|live| live.contains(&target)) {
                    // The page script removed its marks and let a frame pass
                    // before it answered, so this is the element alone.
                    let screenshot = surfaces
                        .get(&(target, false))
                        .zip(captures.as_deref())
                        .and_then(|(surface, directory)| {
                            pick::capture(&pick, surface.image(), directory, stamp())
                        });
                    if let Ok(mut updates) = incoming.lock() {
                        updates.picks.truncate(protocol::MAX_BROWSERS);
                        updates.picks.push((target, pick, screenshot));
                    }
                }
            }
            Event::Recorded { target, error, .. } => {
                if error.as_ref().is_some_and(|e| e.len() > 2048) {
                    return Err(std::io::Error::other("Browser state limit"));
                }
                if let Ok(mut updates) = incoming.lock() {
                    updates.recorded.truncate(protocol::MAX_BROWSERS);
                    updates.recorded.push((target, error));
                }
            }
            Event::Imported { imported, skipped } => {
                if let Ok(mut updates) = incoming.lock() {
                    updates.imported = Some((imported, skipped));
                }
            }
            Event::Icon {
                target,
                width,
                height,
            } => {
                if width > protocol::MAX_ICON || height > protocol::MAX_ICON {
                    return Err(std::io::Error::other("Browser icon limit"));
                }
                let mut pixels = vec![0; width as usize * height as usize * 4];
                reader.read_exact(&mut pixels)?;
                let icon = (!pixels.is_empty()).then(|| {
                    egui::ColorImage::from_rgba_premultiplied(
                        [width as usize, height as usize],
                        &pixels,
                    )
                });
                if targets.lock().is_ok_and(|live| live.contains(&target))
                    && let Ok(mut updates) = incoming.lock()
                {
                    updates.icons.insert(target, icon);
                }
            }
            Event::State { target, state } => {
                if state.title.len() > 2048
                    || state.url.len() > protocol::MAX_URL
                    || state.error.as_ref().is_some_and(|e| e.len() > 2048)
                    || state.tooltip.as_ref().is_some_and(|t| t.len() > 2048)
                    || state.link.as_ref().is_some_and(|l| l.len() > 2048)
                {
                    return Err(std::io::Error::other("Browser state limit"));
                }
                if targets.lock().is_ok_and(|live| live.contains(&target))
                    && let Ok(mut updates) = incoming.lock()
                {
                    updates.states.insert(target, state);
                }
            }
            Event::Frame {
                target,
                width,
                height,
                popup,
                x,
                y,
                damage,
            } => {
                let cleared = popup && width == 0 && height == 0 && damage.is_none();
                let damage = damage.unwrap_or(protocol::Damage::full(width, height));
                let len = if cleared {
                    0
                } else {
                    protocol::frame_len(width, height)
                        .filter(|_| damage.valid(width, height))
                        .and_then(|_| protocol::frame_len(damage.width, damage.height))
                        .ok_or_else(|| std::io::Error::other("Browser frame limit"))?
                };
                bytes.resize(len, 0);
                reader.read_exact(&mut bytes)?;
                let live = targets
                    .lock()
                    .map_err(|_| std::io::Error::other("Browser targets unavailable"))?;
                surfaces.retain(|(target, _), _| live.contains(target));
                if !live.contains(&target) {
                    continue;
                }
                drop(live);
                let key = (target, popup);
                let size = [width as usize, height as usize];
                if cleared {
                    surfaces.remove(&key);
                    if let Ok(mut updates) = incoming.lock() {
                        updates.frames.insert(
                            key,
                            Frame {
                                image: Arc::new(egui::ColorImage::new([0, 0], Vec::new())),
                                position: [x, y],
                                size,
                                offset: [0, 0],
                            },
                        );
                    }
                } else {
                    if surfaces.get(&key).is_none_or(|s| s.size() != size) {
                        if damage != protocol::Damage::full(width, height) {
                            return Err(std::io::Error::other(
                                "Browser frame missing initial pixels",
                            ));
                        }
                        surfaces.insert(key, pixels::Surface::new(size));
                    }
                    if let Some(surface) = surfaces.get_mut(&key) {
                        surface.update(damage, &bytes);
                        if let Ok(mut updates) = incoming.lock() {
                            let frame = surface.snapshot(damage, updates.frames.get(&key), [x, y]);
                            updates.frames.insert(key, frame);
                        }
                    }
                }
            }
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Browsers, mpsc::Receiver<Command>, egui::Context, PaneId) {
        let (sender, receiver) = mpsc::sync_channel(64);
        let mut browsers = Browsers {
            bridge: Some(Bridge {
                sender: Some(sender),
                incoming: Default::default(),
                targets: Default::default(),
                stopping: Default::default(),
                done: Default::default(),
            }),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let pane = PaneId::new(3);
        browsers.start(&ctx, pane, 1, None).unwrap();
        (browsers, receiver, ctx, pane)
    }
    fn update(browsers: &Browsers, target: Target, state: State) {
        browsers
            .bridge
            .as_ref()
            .unwrap()
            .incoming
            .lock()
            .unwrap()
            .states
            .insert(target, state);
    }
    #[test]
    fn browser_pauses_painting_in_hidden_windows_and_resumes_on_restore() {
        let (mut browsers, receiver, ctx, pane) = fixture();
        browsers.poll(&ctx, &[pane], Some(pane));
        receiver.try_iter().for_each(drop);
        for occluded in [false, true] {
            let mut input = egui::RawInput::default();
            let viewport = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
            viewport.minimized = Some(!occluded);
            viewport.occluded = Some(occluded);
            let _ = ctx.run_logic(&input, |ctx| {
                browsers.poll(ctx, &[pane], Some(pane));
            });
            assert_eq!(browsers.entries[&pane].visible, Some(false));
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .minimized = Some(false);
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .occluded = Some(false);
            let _ = ctx.run_logic(&input, |ctx| {
                browsers.poll(ctx, &[pane], Some(pane));
            });
            assert_eq!(browsers.entries[&pane].visible, Some(true));
            let visible = receiver
                .try_iter()
                .filter_map(|c| match c {
                    Command::Visible { visible, .. } => Some(visible),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(visible, [false, true]);
        }
    }
    #[test]
    fn a_restored_tab_opens_its_page_in_its_profile_until_it_is_closed() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("tabs.json"),
            r#"{"version":1,"tabs":{"7":{"url":"https://example.com/a","profile":"profile-work"}}}"#,
        )
        .unwrap();
        let (mut browsers, _receiver, ctx, _) = fixture();
        browsers.store(root.path().into());
        let (restored, new) = (PaneId::new(7), PaneId::new(8));
        browsers
            .start(&ctx, restored, 1, Some("default".into()))
            .unwrap();
        browsers
            .start(&ctx, new, 1, Some("default".into()))
            .unwrap();
        let entry = &browsers.entries[&restored];
        assert_eq!(entry.profile.as_deref(), Some("profile-work"));
        assert_eq!(entry.navigation.as_deref(), Some("https://example.com/a"));
        let entry = &browsers.entries[&new];
        assert_eq!(entry.profile.as_deref(), Some("default"));
        assert_eq!(entry.navigation, None);
        // A restart keeps the page; closing the tab forgets it.
        browsers.start(&ctx, restored, 2, None).unwrap();
        assert!(browsers.entries[&restored].navigation.is_some());
        browsers.close(restored);
        browsers.tabs.flush();
        assert!(
            !std::fs::read_to_string(root.path().join("tabs.json"))
                .unwrap()
                .contains("example.com")
        );
    }
    #[test]
    fn a_page_is_sized_in_the_physical_pixels_of_its_pane() {
        let (mut browsers, receiver, ctx, pane) = fixture();
        browsers.poll(&ctx, &[pane], None);
        receiver.try_iter().for_each(drop);
        let sized = |browsers: &mut Browsers, size, scale| {
            browsers.geometry(pane, 1, size, scale);
            receiver.try_iter().find_map(|command| match command {
                Command::Resize {
                    width,
                    height,
                    scale,
                    ..
                } => Some((width, height, scale)),
                _ => None,
            })
        };
        // A pane of a fractional size covers whole pixels of the display.
        assert_eq!(
            sized(&mut browsers, egui::vec2(433.4, 300.0), 1.5),
            Some((650, 450, 1.5))
        );
        assert_eq!(sized(&mut browsers, egui::vec2(433.4, 300.0), 1.5), None);
        // One too large for a frame is painted at a lower scale.
        let (width, height, scale) = sized(&mut browsers, egui::vec2(4000.0, 3000.0), 2.0).unwrap();
        assert!(scale < 2.0);
        assert!(protocol::frame_len(width, height).is_some());
    }
    #[test]
    fn a_wheel_and_a_touchpad_scroll_a_page_as_they_do_a_browser() {
        let (mut browsers, receiver, ctx, pane) = fixture();
        browsers.poll(&ctx, &[pane], None);
        receiver.try_iter().for_each(drop);
        let target = Target {
            pane: pane.get(),
            generation: 1,
        };
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        let mut scroll = |unit, y: f32| {
            let event = egui::Event::MouseWheel {
                unit,
                delta: egui::vec2(0.0, y),
                modifiers: Default::default(),
                phase: egui::TouchPhase::Move,
            };
            browsers
                .input(
                    target,
                    rect,
                    &[event],
                    false,
                    Default::default(),
                    Some(egui::pos2(10.0, 10.0)),
                )
                .unwrap();
            receiver.try_iter().find_map(|command| match command {
                Command::Mouse {
                    modifiers,
                    kind: protocol::Mouse::Wheel { dy, .. },
                    ..
                } => Some((dy, modifiers & (1 << 14) != 0)),
                _ => None,
            })
        };
        // A notch is Chromium's own distance, left to it to ease.
        assert_eq!(scroll(egui::MouseWheelUnit::Line, -1.0), Some((-53, false)));
        // A slow stroke on a touchpad adds up rather than being dropped.
        assert_eq!(scroll(egui::MouseWheelUnit::Point, 0.4), None);
        assert_eq!(scroll(egui::MouseWheelUnit::Point, 0.4), Some((1, true)));
        assert_eq!(scroll(egui::MouseWheelUnit::Point, 12.3), Some((12, true)));
    }
    #[test]
    fn browser_frame_rate_follows_display_and_retries_a_full_command_queue() {
        let (mut browsers, receiver, ctx, pane) = fixture();
        browsers.display_rate(Some(143_980));
        browsers.poll(&ctx, &[pane], None);
        assert!(receiver.try_iter().any(|command| matches!(
            command,
            Command::FrameRate {
                frames_per_second: 144,
                ..
            }
        )));
        receiver.try_iter().for_each(drop);
        let target = Target {
            pane: pane.get(),
            generation: 1,
        };
        for _ in 0..64 {
            browsers.send(Command::Copy { target }).unwrap();
        }
        browsers.display_rate(Some(59_940));
        browsers.poll(&ctx, &[pane], None);
        assert_eq!(browsers.entries[&pane].frame_rate, Some(144));
        receiver.try_iter().for_each(drop);
        browsers.poll(&ctx, &[pane], None);
        assert!(receiver.try_iter().any(|command| matches!(
            command,
            Command::FrameRate {
                frames_per_second: 60,
                ..
            }
        )));
        browsers.display_rate(Some(360_070));
        browsers.poll(&ctx, &[pane], None);
        assert_eq!(
            browsers.entries[&pane].frame_rate,
            Some(protocol::MAX_FRAME_RATE)
        );
        browsers.display_rate(None);
        browsers.poll(&ctx, &[pane], None);
        assert_eq!(
            browsers.entries[&pane].frame_rate,
            Some(protocol::DEFAULT_FRAME_RATE)
        );
    }

    #[test]
    fn browser_reader_rejects_outside_or_uninitialized_damage() {
        let ctx = egui::Context::default();
        let incoming = Arc::new(Mutex::new(Incoming::default()));
        let target = Target {
            pane: 1,
            generation: 1,
        };
        let targets = Arc::new(Mutex::new(BTreeSet::from([target])));
        for damage in [
            protocol::Damage {
                x: 1,
                y: 0,
                width: 2,
                height: 1,
            },
            protocol::Damage {
                x: u32::MAX,
                y: 0,
                width: 1,
                height: 1,
            },
            protocol::Damage {
                x: 0,
                y: 0,
                width: 0,
                height: 1,
            },
            protocol::Damage {
                x: 1,
                y: 0,
                width: 1,
                height: 1,
            },
        ] {
            let mut wire = serde_json::to_vec(&Event::Frame {
                target,
                width: 2,
                height: 1,
                popup: false,
                x: 0,
                y: 0,
                damage: Some(damage),
            })
            .unwrap();
            wire.push(b'\n');
            wire.extend([0; 8]);
            assert!(
                read(
                    wire.as_slice(),
                    incoming.clone(),
                    targets.clone(),
                    ctx.clone(),
                    None,
                )
                .is_err()
            );
        }
    }
    #[test]
    fn browser_initial_navigation_waits_for_creation() {
        let (mut browsers, receiver, ctx, pane) = fixture();
        browsers.navigate(pane, 1, "localhost:3000").unwrap();
        browsers.poll(&ctx, &[pane], None);
        let commands: Vec<_> = receiver.try_iter().collect();
        assert!(commands.iter().any(|c| matches!(c, Command::Open { .. })));
        assert!(
            !commands
                .iter()
                .any(|c| matches!(c, Command::Navigate { .. }))
        );
        update(
            &browsers,
            Target {
                pane: 3,
                generation: 1,
            },
            State {
                ready: true,
                ..Default::default()
            },
        );
        assert_eq!(browsers.poll(&ctx, &[pane], None).len(), 1);
        assert!(
            receiver.try_iter().any(
                |c| matches!(c, Command::Navigate { url, .. } if url == "http://localhost:3000")
            )
        );
    }
    #[test]
    fn browser_stale_generation_cannot_replace_restarted_state() {
        let (mut browsers, receiver, ctx, pane) = fixture();
        browsers.poll(&ctx, &[pane], None);
        receiver.try_iter().for_each(drop);
        browsers.start(&ctx, pane, 2, None).unwrap();
        update(
            &browsers,
            Target {
                pane: 3,
                generation: 1,
            },
            State {
                ready: true,
                title: "Old page".into(),
                ..Default::default()
            },
        );
        assert!(browsers.poll(&ctx, &[pane], None).is_empty());
        assert!(browsers.view(pane, 2).unwrap().state.title.is_empty());
        assert_eq!(browsers.closing.len(), 1);
        update(
            &browsers,
            Target {
                pane: 3,
                generation: 1,
            },
            State {
                closed: true,
                ..Default::default()
            },
        );
        browsers.poll(&ctx, &[pane], None);
        assert!(browsers.closing.is_empty());
    }
    #[test]
    fn browser_damage_coalesces_without_losing_pixels_between_updates() {
        let ctx = egui::Context::default();
        let incoming = Arc::new(Mutex::new(Incoming::default()));
        let target = Target {
            pane: 3,
            generation: 1,
        };
        let targets = Arc::new(Mutex::new(BTreeSet::from([target])));
        let mut wire = Vec::new();
        for (damage, pixels) in [
            (
                None,
                vec![10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255],
            ),
            (
                Some(protocol::Damage {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                }),
                vec![1, 2, 3, 255],
            ),
            (
                Some(protocol::Damage {
                    x: 2,
                    y: 0,
                    width: 1,
                    height: 1,
                }),
                vec![4, 5, 6, 255],
            ),
        ] {
            serde_json::to_writer(
                &mut wire,
                &Event::Frame {
                    target,
                    width: 3,
                    height: 1,
                    popup: false,
                    x: 0,
                    y: 0,
                    damage,
                },
            )
            .unwrap();
            wire.push(b'\n');
            wire.extend(pixels);
        }
        read(wire.as_slice(), incoming.clone(), targets, ctx, None).unwrap();
        let updates = incoming.lock().unwrap();
        let frame = &updates.frames[&(target, false)];
        assert_eq!(frame.image.size, [3, 1]);
        assert_eq!(
            frame
                .image
                .pixels
                .iter()
                .map(|p| p.to_array())
                .collect::<Vec<_>>(),
            [[3, 2, 1, 255], [60, 50, 40, 255], [6, 5, 4, 255]]
        );
    }
    #[test]
    fn browser_reader_rejects_oversized_frames_and_messages() {
        let ctx = egui::Context::default();
        let incoming = Arc::new(Mutex::new(Incoming::default()));
        let targets = Arc::new(Mutex::new(BTreeSet::new()));
        let event = Event::Frame {
            target: Target {
                pane: 1,
                generation: 1,
            },
            width: 4096,
            height: 4096,
            damage: None,
            popup: false,
            x: 0,
            y: 0,
        };
        let mut data = serde_json::to_vec(&event).unwrap();
        data.push(b'\n');
        assert!(
            read(
                data.as_slice(),
                incoming.clone(),
                targets.clone(),
                ctx.clone(),
                None,
            )
            .is_err()
        );
        let bytes = vec![b' '; protocol::MAX_MESSAGE + 1];
        assert!(read(bytes.as_slice(), incoming, targets, ctx, None).is_err());
    }
    #[test]
    fn browser_disallowed_navigation_and_text_do_not_kill_transport() {
        let (mut browsers, _receiver, _ctx, pane) = fixture();
        for url in [
            "file:///etc/passwd",
            "javascript://alert(1)",
            "https://a\nb",
        ] {
            assert!(browsers.navigate(pane, 1, url).is_err());
        }
        assert!(
            browsers
                .send(Command::Paste {
                    target: Target {
                        pane: 3,
                        generation: 1
                    },
                    text: "a".repeat(protocol::MAX_TEXT + 1)
                })
                .is_err()
        );
        assert!(
            browsers
                .send(Command::Copy {
                    target: Target {
                        pane: 3,
                        generation: 1
                    }
                })
                .is_ok()
        );
    }
    #[test]
    fn browser_input_reconciles_focus_and_ignores_failed_previews() {
        let (mut browsers, receiver, _ctx, pane) = fixture();
        let target = Target {
            pane: pane.get(),
            generation: 1,
        };
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
        let events = [egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        }];
        browsers
            .input(target, rect, &events, true, egui::Modifiers::CTRL, None)
            .unwrap();
        let commands: Vec<_> = receiver.try_iter().collect();
        assert!(matches!(&commands[0], Command::Focus { focused: true, .. }));
        assert!(matches!(
            &commands[1],
            Command::Key {
                code: 65,
                modifiers: 4,
                ..
            }
        ));
        browsers.entries.get_mut(&pane).unwrap().failed = true;
        browsers
            .input(target, rect, &events, true, egui::Modifiers::CTRL, None)
            .unwrap();
        assert!(receiver.try_iter().next().is_none());
    }
    #[test]
    fn browser_mouse_release_outside_still_reaches_page() {
        let (mut browsers, receiver, _ctx, pane) = fixture();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
        for (position, pressed) in [
            (egui::pos2(20.0, 20.0), true),
            (egui::pos2(120.0, 20.0), false),
        ] {
            browsers
                .input(
                    Target {
                        pane: pane.get(),
                        generation: 1,
                    },
                    rect,
                    &[egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    }],
                    false,
                    Default::default(),
                    Some(position),
                )
                .unwrap();
        }
        let commands: Vec<_> = receiver
            .try_iter()
            .filter(|c| matches!(c, Command::Mouse { .. }))
            .collect();
        assert_eq!(commands.len(), 2);
        assert!(matches!(
            &commands[1],
            Command::Mouse {
                x: 120,
                kind: protocol::Mouse::Button { pressed: false, .. },
                ..
            }
        ));
        assert_eq!(browsers.entries[&pane].buttons, 0);
    }
    #[test]
    fn browser_host_failure_releases_old_capacity_and_reports_once() {
        let (mut browsers, receiver, ctx, pane) = fixture();
        browsers.poll(&ctx, &[pane], None);
        receiver.try_iter().for_each(drop);
        browsers
            .bridge
            .as_ref()
            .unwrap()
            .incoming
            .lock()
            .unwrap()
            .failed = true;
        assert_eq!(browsers.poll(&ctx, &[pane], None).len(), 1);
        assert!(!browsers.entries[&pane].opened);
        assert!(browsers.poll(&ctx, &[pane], None).is_empty());
        browsers.close(pane);
        assert!(browsers.closing.is_empty());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn browser_keeps_its_sandbox_when_a_setuid_helper_or_namespaces_serve() {
        unsafe extern "C" {
            fn getuid() -> u32;
        }
        let dir = tempfile::tempdir().unwrap();
        let host = dir.path().join("neptune-browser");
        std::fs::write(&host, b"").unwrap();
        // No helper: the answer is whether this user may map itself in a new
        // user namespace, which `unshare --map-root-user` also requires.
        let namespaces = Process::new("unshare")
            .args(["--user", "--map-root-user", "true"])
            .stderr(Stdio::null())
            .status();
        if let Ok(status) = namespaces {
            assert_eq!(sandbox_unavailable(&host), !status.success());
        }
        // A helper this user owns is not the setuid-root one Chromium accepts.
        use std::os::unix::fs::PermissionsExt;
        let helper = dir.path().join("chrome-sandbox");
        std::fs::write(&helper, b"").unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o4755)).unwrap();
        // SAFETY: getuid has no preconditions and cannot fail.
        if unsafe { getuid() } != 0
            && let Ok(status) = namespaces
        {
            assert_eq!(sandbox_unavailable(&host), !status.success());
        }
    }
}
