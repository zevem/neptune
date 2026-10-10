//! Bounded process supervision and owned browser snapshots. No Chromium code
//! is linked into the terminal executable or called during interactive frames.
mod input;
mod pixels;
pub(crate) mod protocol;

use eframe::egui;
use neptune_model::{Completion, PaneId};
use protocol::{Command, Event, State, Target};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, BufReader, Read, Write},
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
    pub failed: bool,
    /// This system allowed Chromium no sandbox, so the host runs without one.
    pub unsandboxed: bool,
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
    failed: bool,
    unsandboxed: bool,
}
struct Bridge {
    sender: Option<mpsc::SyncSender<Command>>,
    incoming: Arc<Mutex<Incoming>>,
    targets: Arc<Mutex<BTreeSet<Target>>>,
    stopping: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}
impl Bridge {
    fn launch(ctx: egui::Context) -> Result<Self, &'static str> {
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
                if run(receiver, updates.clone(), live, stop, ctx.clone()).is_err()
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
    opened: bool,
    completed: bool,
    failed: bool,
    visible: Option<bool>,
    focused: Option<bool>,
    geometry: Option<(u32, u32, f32)>,
    frame_rate: Option<u32>,
    navigation: Option<String>,
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
}
impl Browsers {
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
    ) -> Result<(), &'static str> {
        let replacement =
            self.entries.contains_key(&pane) || self.closing.keys().any(|t| t.pane == pane.get());
        if !replacement && self.entries.len() + self.closing.len() >= protocol::MAX_BROWSERS {
            return Err(
                "Browser capacity is reserved while previews are closing. Try again in a moment.",
            );
        }
        self.close(pane);
        if self
            .bridge
            .as_ref()
            .is_some_and(|b| b.done.load(Ordering::Acquire))
        {
            self.bridge = None;
            self.closing.clear();
        }
        if self.bridge.is_none() {
            self.bridge = Some(Bridge::launch(ctx.clone())?);
        }
        self.entries.insert(
            pane,
            Entry {
                generation,
                state: State::default(),
                texture: None,
                popup: None,
                opened: false,
                completed: false,
                failed: false,
                visible: None,
                focused: None,
                geometry: None,
                frame_rate: None,
                navigation: None,
                buttons: 0,
                pointer_inside: false,
                click: None,
            },
        );
        ctx.request_repaint();
        Ok(())
    }
    pub fn close(&mut self, pane: PaneId) {
        if let Some(entry) = self.entries.remove(&pane)
            && entry.opened
        {
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
            failed: entry.failed,
            unsandboxed: self.unsandboxed,
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
        let width = (size.x.max(1.0) as u32).min(protocol::MAX_SIDE);
        let height = (size.y.max(1.0) as u32).min(protocol::MAX_SIDE);
        let scale = scale
            .min((protocol::MAX_PIXELS as f32 / (width as f32 * height as f32)).sqrt() * 0.999)
            .min(protocol::MAX_SIDE as f32 / width.max(height) as f32)
            .clamp(0.25, 4.0);
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
        if self
            .bridge
            .as_ref()
            .is_some_and(|b| b.done.load(Ordering::Acquire) && b.stopping.load(Ordering::Acquire))
        {
            self.bridge = None;
        }
        if self.bridge.is_none() && !self.entries.is_empty() {
            match Bridge::launch(ctx.clone()) {
                Ok(bridge) => {
                    self.bridge = Some(bridge);
                    self.unsandboxed = false;
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
                        egui::pos2(frame.position[0] as f32, frame.position[1] as f32),
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
                        entry.state.loading = false;
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
                if bridge.send(Command::Open { target }) {
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
    let profile = profile_builder.tempdir()?;
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
    process
        .env("NEPTUNE_BROWSER_PROFILE", profile.path())
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
        .spawn(move || read(stdout, incoming, targets, reader_ctx))?;
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
            Event::State { target, state } => {
                if state.title.len() > 2048
                    || state.url.len() > protocol::MAX_URL
                    || state.error.as_ref().is_some_and(|e| e.len() > 2048)
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
        browsers.start(&ctx, pane, 1).unwrap();
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
                    ctx.clone()
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
        browsers.start(&ctx, pane, 2).unwrap();
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
        read(wire.as_slice(), incoming.clone(), targets, ctx).unwrap();
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
                ctx.clone()
            )
            .is_err()
        );
        let bytes = vec![b' '; protocol::MAX_MESSAGE + 1];
        assert!(read(bytes.as_slice(), incoming, targets, ctx).is_err());
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
