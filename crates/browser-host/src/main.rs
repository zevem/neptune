//! Chromium lives in a separate, task-owned process. Its event loop, startup,
//! page scripts and pixel copies cannot block Neptune's terminal frames.
#[path = "handlers.rs"]
mod handlers;
#[cfg(target_os = "macos")]
#[path = "mac.rs"]
mod mac;
mod pacing;
#[path = "../../../src/runtime/browser/protocol.rs"]
mod protocol;

use cef::{args::Args, *};
use protocol::{Command, Event, Target};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    sync::{Arc, Condvar, Mutex, mpsc},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Wake {
    next: Option<Instant>,
    input: bool,
}
impl Wake {
    fn begin(&mut self, now: Instant) {
        self.input = false;
        if self.next.is_some_and(|next| next <= now) {
            self.next = None;
        }
    }
}

type PendingOutput = BTreeMap<(Target, u8), (Event, Vec<u8>)>;
#[derive(Default)]
struct Output {
    // One latest state and frame per live pane, never a growing frame queue.
    pending: Mutex<PendingOutput>,
    wake: Condvar,
}
impl Output {
    fn paint(
        &self,
        target: Target,
        popup: bool,
        size: [u32; 2],
        position: [i32; 2],
        mut damage: protocol::Damage,
        pixels: &[u8],
    ) {
        let slot = if popup { 2 } else { 1 };
        let Ok(mut pending) = self.pending.lock() else {
            return;
        };
        if pending.len() >= protocol::MAX_BROWSERS * 3 && !pending.contains_key(&(target, slot)) {
            return;
        }
        // A slow pipe may replace several paints. Include every outstanding
        // change, copying their union from CEF's latest complete buffer.
        if let Some((
            Event::Frame {
                width,
                height,
                damage: old,
                ..
            },
            _,
        )) = pending.get(&(target, slot))
            && [*width, *height] == size
        {
            damage = damage.union(old.unwrap_or(protocol::Damage::full(size[0], size[1])));
        }
        let stride = size[0] as usize * 4;
        let row_len = damage.width as usize * 4;
        let mut bytes = Vec::with_capacity(row_len * damage.height as usize);
        for y in damage.y..damage.y + damage.height {
            let start = y as usize * stride + damage.x as usize * 4;
            bytes.extend_from_slice(&pixels[start..start + row_len]);
        }
        pending.insert(
            (target, slot),
            (
                Event::Frame {
                    target,
                    width: size[0],
                    height: size[1],
                    popup,
                    x: position[0],
                    y: position[1],
                    damage: Some(damage),
                },
                bytes,
            ),
        );
        drop(pending);
        self.wake.notify_one();
    }
    fn put(&self, target: Target, slot: u8, event: Event, bytes: Vec<u8>) {
        if let Ok(mut pending) = self.pending.lock()
            && (pending.len() < protocol::MAX_BROWSERS * 3 || pending.contains_key(&(target, slot)))
        {
            pending.insert((target, slot), (event, bytes));
        }
        self.wake.notify_one();
    }
    fn write(&self) -> std::io::Result<()> {
        // This is a binary pipe, so do not scan megapixel payloads for text
        // newlines through Rust's line-buffered stdout. Clone the owned pipe
        // handle and buffer the small JSON envelopes; explicitly flush batches.
        let standard = std::io::stdout();
        #[cfg(unix)]
        let pipe = {
            use std::os::fd::AsFd;
            std::fs::File::from(standard.as_fd().try_clone_to_owned()?)
        };
        #[cfg(windows)]
        let pipe = {
            use std::os::windows::io::AsHandle;
            std::fs::File::from(standard.as_handle().try_clone_to_owned()?)
        };
        let mut stdout = BufWriter::with_capacity(64 * 1024, pipe);
        loop {
            let Ok(mut pending) = self.pending.lock() else {
                return Ok(());
            };
            while pending.is_empty() {
                let Ok(next) = self.wake.wait(pending) else {
                    return Ok(());
                };
                pending = next;
            }
            let events = std::mem::take(&mut *pending);
            drop(pending);
            for (_, (event, bytes)) in events {
                serde_json::to_writer(&mut stdout, &event)?;
                stdout.write_all(b"\n")?;
                stdout.write_all(&bytes)?;
            }
            stdout.flush()?;
        }
    }
}

struct Entry {
    browser: Browser,
    data: Arc<handlers::Data>,
}

#[cfg(not(target_os = "windows"))]
fn main() -> anyhow::Result<()> {
    run(std::ptr::null_mut())
}
#[cfg(target_os = "windows")]
// This entry is unused when the same engine is included by the sandbox DLL.
#[allow(dead_code)]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("Use the bundled neptune_browser.exe sandbox bootstrap.")
}

pub(crate) fn run(sandbox_info: *mut u8) -> anyhow::Result<()> {
    let args = Args::new();
    #[cfg(target_os = "macos")]
    let helper = std::env::args().any(|a| a.starts_with("--type="));
    #[cfg(target_os = "macos")]
    let _sandbox = if helper {
        let mut s = cef::sandbox::Sandbox::new();
        s.initialize(args.as_main_args());
        Some(s)
    } else {
        None
    };
    #[cfg(target_os = "macos")]
    let _loader = {
        let executable = std::env::current_exe()?;
        let loader = cef::library_loader::LibraryLoader::new(&executable, helper);
        anyhow::ensure!(loader.load(), "Browser framework unavailable");
        loader
    };
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    // Chromium renderer/GPU subprocesses enter here before any worker starts.
    let result = cef::execute_process(Some(args.as_main_args()), None, sandbox_info);
    if result >= 0 {
        std::process::exit(result)
    }
    #[cfg(target_os = "macos")]
    mac::initialize();
    let output = Arc::new(Output::default());
    let writer = output.clone();
    std::thread::Builder::new()
        .name("browser-output".into())
        .spawn(move || {
            // If Neptune has gone away, this child has no work to preserve.
            if writer.write().is_err() {
                std::process::exit(0)
            }
        })?;
    let (sender, receiver) = mpsc::sync_channel(64);
    let deadline = Arc::new((Mutex::new(Wake::default()), Condvar::new()));
    let input_deadline = deadline.clone();
    std::thread::Builder::new()
        .name("browser-input".into())
        .spawn(move || {
            let mut reader = BufReader::new(std::io::stdin());
            loop {
                let mut line = Vec::new();
                match reader
                    .by_ref()
                    .take(protocol::MAX_MESSAGE as u64 + 1)
                    .read_until(b'\n', &mut line)
                {
                    Ok(0) | Err(_) => break,
                    Ok(_) if line.len() > protocol::MAX_MESSAGE || line.last() != Some(&b'\n') => {
                        break;
                    }
                    Ok(_) => {
                        let Ok(command) = serde_json::from_slice(&line) else {
                            break;
                        };
                        if sender.send(command).is_err() {
                            return;
                        }
                        if let Ok(mut next) = input_deadline.0.lock() {
                            next.input = true;
                        }
                        input_deadline.1.notify_one();
                    }
                }
            }
            let _ = sender.send(Command::Shutdown);
            if let Ok(mut next) = input_deadline.0.lock() {
                next.input = true;
            }
            input_deadline.1.notify_one();
        })?;
    let mut app = handlers::HostApp::new(deadline.clone());
    let profile = std::env::var_os("NEPTUNE_BROWSER_PROFILE")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("Missing supervised browser profile"))?;
    anyhow::ensure!(profile.is_dir(), "Browser profile unavailable");
    let settings = Settings {
        windowless_rendering_enabled: 1,
        external_message_pump: 1,
        // Empty cache_path gives in-memory contexts; a unique root isolates
        // CEF's singleton bookkeeping between Neptune windows.
        root_cache_path: profile.to_string_lossy().as_ref().into(),
        #[cfg(target_os = "macos")]
        browser_subprocess_path: std::env::current_exe()?
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Missing helper parent"))?
            .join("../Frameworks/Neptune Browser Helper.app/Contents/MacOS/Neptune Browser Helper")
            .to_string_lossy()
            .as_ref()
            .into(),
        log_severity: LogSeverity::DISABLE,
        ..Default::default()
    };
    anyhow::ensure!(
        cef::initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(&mut app),
            sandbox_info
        ) == 1,
        "Browser initialization failed"
    );
    let mut entries: BTreeMap<Target, Entry> = BTreeMap::new();
    let mut stopping = None;
    loop {
        if let Ok(mut next) = deadline.0.lock() {
            next.begin(Instant::now());
        }
        #[cfg(target_os = "linux")]
        pump_native_events();
        cef::do_message_loop_work();
        for command in receiver.try_iter().take(64) {
            if matches!(command, Command::Shutdown) {
                stopping.get_or_insert(Instant::now());
                for entry in entries.values() {
                    if let Some(host) = entry.browser.host() {
                        host.close_dev_tools();
                        host.close_browser(1);
                    }
                }
            } else if stopping.is_none() {
                handle(command, &mut entries, &output);
            }
        }
        entries.retain(|_, entry| !entry.data.closed());
        if stopping.is_some() && entries.is_empty() {
            break;
        }
        // A stalled engine is killed by Neptune's supervisor; do not tear CEF
        // down while it still has live browsers.
        if stopping.is_some_and(|since| since.elapsed() > Duration::from_secs(4)) {
            std::process::exit(0);
        }
        let (lock, wake) = &*deadline;
        if let Ok(next) = lock.lock() {
            let wait = if next.input {
                Duration::ZERO
            } else {
                next.next.map_or(Duration::from_secs(60), |at| {
                    at.saturating_duration_since(Instant::now())
                })
            }
            .min(if stopping.is_some() {
                Duration::from_millis(10)
            } else if !entries.is_empty() {
                entries
                    .values()
                    .map(|entry| entry.data.pump_interval(Instant::now()))
                    .min()
                    .unwrap_or(Duration::from_millis(33))
            } else {
                Duration::from_secs(60)
            });
            let _ = wake.wait_timeout(next, wait);
        }
    }
    cef::shutdown();
    Ok(())
}

#[cfg(target_os = "linux")]
fn pump_native_events() {
    #[link(name = "glib-2.0")]
    unsafe extern "C" {
        fn g_main_context_iteration(context: *mut std::ffi::c_void, may_block: i32) -> i32;
    }
    // CEF's Linux external-pump example dispatches the default GLib context.
    // Clipboard selection requests and native DevTools events live there,
    // outside CefDoMessageLoopWork. Bound dispatch and never block on GTK.
    for _ in 0..32 {
        // SAFETY: null selects the default context; 0 requests nonblocking
        // dispatch. This is the helper's main thread, which owns CEF and GTK.
        if unsafe { g_main_context_iteration(std::ptr::null_mut(), 0) } == 0 {
            break;
        }
    }
}

fn handle(command: Command, entries: &mut BTreeMap<Target, Entry>, output: &Arc<Output>) {
    if let Command::Open { target } = command {
        if entries.contains_key(&target) || entries.len() >= protocol::MAX_BROWSERS {
            return;
        }
        let data = Arc::new(handlers::Data::new(target, output.clone()));
        let mut client = handlers::PreviewClient::new(data.clone());
        let info = WindowInfo {
            windowless_rendering_enabled: 1,
            ..Default::default()
        };
        let settings = BrowserSettings {
            windowless_frame_rate: protocol::DEFAULT_FRAME_RATE as i32,
            ..Default::default()
        };
        let mut context =
            cef::request_context_create_context(Some(&RequestContextSettings::default()), None);
        if let Some(browser) = cef::browser_host_create_browser_sync(
            Some(&info),
            Some(&mut client),
            Some(&"about:blank".into()),
            Some(&settings),
            None,
            context.as_mut(),
        ) {
            entries.insert(target, Entry { browser, data });
        } else {
            data.update(|state| {
                state.closed = true;
                state.error =
                    Some("Could not start the browser. Retry to open a fresh preview.".into())
            });
        }
        return;
    }
    let Some(target) = command_target(&command) else {
        return;
    };
    let Some(entry) = entries.get_mut(&target) else {
        return;
    };
    if matches!(&command, Command::Text { text, .. } | Command::Paste { text, .. } | Command::Ime { text, .. } | Command::Find { text, .. } if text.len() > protocol::MAX_TEXT)
    {
        entry.data.update(|s| {
            s.error = Some("Browser text input is limited to 64 KiB per operation.".into())
        });
        return;
    }
    let Some(host) = entry.browser.host() else {
        return;
    };
    match command {
        Command::Close { .. } => {
            host.close_dev_tools();
            host.close_browser(1);
        }
        Command::Navigate { url, .. } => {
            if valid_url(&url) {
                entry.data.update(|s| {
                    s.error = None;
                    s.loading = true;
                });
                if let Some(frame) = entry.browser.main_frame() {
                    frame.load_url(Some(&url.as_str().into()));
                }
            } else {
                entry
                    .data
                    .update(|s| s.error = Some("Enter an HTTP or HTTPS address.".into()));
            }
        }
        Command::Back { .. } => {
            entry.data.update(|s| s.error = None);
            entry.browser.go_back();
        }
        Command::Forward { .. } => {
            entry.data.update(|s| s.error = None);
            entry.browser.go_forward();
        }
        Command::Reload { .. } => {
            entry.data.update(|s| s.error = None);
            entry.browser.reload();
        }
        Command::Stop { .. } => entry.browser.stop_load(),
        Command::DevTools { .. } => {
            let mut client = handlers::ToolsClient::new();
            let info = WindowInfo {
                window_name: "Neptune Developer Tools".into(),
                bounds: Rect {
                    x: 80,
                    y: 80,
                    width: 1100,
                    height: 760,
                },
                ..Default::default()
            };
            #[cfg(windows)]
            let info = info.set_as_popup(std::ptr::null_mut(), "Neptune Developer Tools");
            host.show_dev_tools(
                Some(&info),
                Some(&mut client),
                Some(&BrowserSettings::default()),
                None,
            );
        }
        Command::Find {
            text,
            reverse,
            next,
            ..
        } => host.find(
            Some(&text.as_str().into()),
            i32::from(!reverse),
            0,
            i32::from(next),
        ),
        Command::StopFind { .. } => host.stop_finding(1),
        Command::Resize {
            width,
            height,
            scale,
            ..
        } => {
            if width > 0
                && height > 0
                && width <= protocol::MAX_SIDE
                && height <= protocol::MAX_SIDE
                && scale.is_finite()
                && (0.25..=4.0).contains(&scale)
                && protocol::frame_len(
                    (width as f32 * scale).ceil() as u32,
                    (height as f32 * scale).ceil() as u32,
                )
                .is_some()
            {
                entry.data.resize(width, height, scale);
                host.notify_screen_info_changed();
                host.was_resized();
            }
        }
        Command::FrameRate {
            frames_per_second, ..
        } => {
            if (1..=protocol::MAX_FRAME_RATE).contains(&frames_per_second) {
                entry.data.frame_rate(frames_per_second);
                host.set_windowless_frame_rate(frames_per_second as i32);
            }
        }
        Command::Visible { visible, .. } => {
            entry.data.visible(visible);
            if visible {
                entry.data.invalidate();
            }
            host.was_hidden(i32::from(!visible));
        }
        Command::Focus { focused, .. } => host.set_focus(i32::from(focused)),
        Command::Key {
            code,
            native,
            pressed,
            modifiers,
            ..
        } => {
            host.send_key_event(Some(&KeyEvent {
                type_: if pressed {
                    KeyEventType::RAWKEYDOWN
                } else {
                    KeyEventType::KEYUP
                },
                windows_key_code: code,
                native_key_code: native,
                modifiers,
                ..Default::default()
            }));
            if pressed && matches!(code, 8 | 9 | 13) {
                host.send_key_event(Some(&KeyEvent {
                    type_: KeyEventType::CHAR,
                    windows_key_code: code,
                    native_key_code: native,
                    modifiers,
                    character: code as u16,
                    unmodified_character: code as u16,
                    ..Default::default()
                }));
            }
        }
        Command::Text { text, .. } => {
            for character in text.encode_utf16().take(protocol::MAX_MESSAGE / 2) {
                host.send_key_event(Some(&KeyEvent {
                    type_: KeyEventType::CHAR,
                    windows_key_code: i32::from(character),
                    character,
                    unmodified_character: character,
                    ..Default::default()
                }));
            }
        }
        Command::Ime { text, commit, .. } => {
            let length = text.encode_utf16().count() as u32;
            let text: CefString = text.as_str().into();
            if commit {
                host.ime_commit_text(Some(&text), None, 0);
            } else {
                host.ime_set_composition(
                    Some(&text),
                    None,
                    None,
                    Some(&Range {
                        from: length,
                        to: length,
                    }),
                );
            }
        }
        Command::Mouse {
            x,
            y,
            modifiers,
            kind,
            ..
        } => {
            let event = MouseEvent { x, y, modifiers };
            match kind {
                protocol::Mouse::Move { leave } => {
                    host.send_mouse_move_event(Some(&event), i32::from(leave))
                }
                protocol::Mouse::Button {
                    button,
                    pressed,
                    clicks,
                } => host.send_mouse_click_event(
                    Some(&event),
                    match button {
                        1 => MouseButtonType::MIDDLE,
                        2 => MouseButtonType::RIGHT,
                        _ => MouseButtonType::LEFT,
                    },
                    i32::from(!pressed),
                    clicks.clamp(1, 3),
                ),
                protocol::Mouse::Wheel { dx, dy } => {
                    host.send_mouse_wheel_event(Some(&event), dx, dy)
                }
            }
        }
        Command::Copy { .. } => {
            if let Some(frame) = entry.browser.focused_frame() {
                frame.copy();
            }
        }
        Command::Cut { .. } => {
            if let Some(frame) = entry.browser.focused_frame() {
                frame.cut();
            }
        }
        Command::Paste { .. } => {
            // Paste is a browser edit command, distinct from completing an IME
            // composition. CEF reads the native clipboard and emits a normal
            // paste event, preserving the page's clipboard handling.
            if let Some(frame) = entry.browser.focused_frame() {
                frame.paste();
            }
        }
        Command::Open { .. } | Command::Shutdown => {}
    }
}

fn command_target(command: &Command) -> Option<Target> {
    match command {
        Command::Open { target }
        | Command::Close { target }
        | Command::Navigate { target, .. }
        | Command::Back { target }
        | Command::Forward { target }
        | Command::Reload { target }
        | Command::Stop { target }
        | Command::DevTools { target }
        | Command::Find { target, .. }
        | Command::StopFind { target }
        | Command::Resize { target, .. }
        | Command::FrameRate { target, .. }
        | Command::Visible { target, .. }
        | Command::Focus { target, .. }
        | Command::Key { target, .. }
        | Command::Text { target, .. }
        | Command::Ime { target, .. }
        | Command::Mouse { target, .. }
        | Command::Copy { target }
        | Command::Cut { target }
        | Command::Paste { target, .. } => Some(*target),
        Command::Shutdown => None,
    }
}

fn valid_url(value: &str) -> bool {
    value == "about:blank"
        || (value.len() <= protocol::MAX_URL
            && !value.chars().any(char::is_control)
            && url::Url::parse(value)
                .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn coalesced_paints_include_all_damage_from_the_latest_complete_buffer() {
        use super::{
            Output,
            protocol::{Damage, Event, Target},
        };
        let output = Output::default();
        let target = Target {
            pane: 1,
            generation: 1,
        };
        let left = Damage {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let right = Damage {
            x: 2,
            y: 0,
            width: 1,
            height: 1,
        };
        let first = [1, 2, 3, 255, 10, 20, 30, 255, 40, 50, 60, 255];
        let latest = [1, 2, 3, 255, 10, 20, 30, 255, 4, 5, 6, 255];
        output.paint(target, false, [3, 1], [0, 0], left, &first);
        output.paint(target, false, [3, 1], [0, 0], right, &latest);
        let pending = output.pending.lock().unwrap();
        let (event, bytes) = &pending[&(target, 1)];
        assert!(
            matches!(event, Event::Frame { damage: Some(damage), .. } if *damage == Damage::full(3, 1))
        );
        assert_eq!(bytes, &latest);
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn resize_and_popup_close_replace_outstanding_damage() {
        use super::{
            Output,
            protocol::{Damage, Event, Target},
        };
        let output = Output::default();
        let target = Target {
            pane: 1,
            generation: 1,
        };
        output.paint(target, true, [3, 1], [4, 5], Damage::full(3, 1), &[0; 12]);
        output.put(
            target,
            2,
            Event::Frame {
                target,
                width: 0,
                height: 0,
                popup: true,
                x: 0,
                y: 0,
                damage: None,
            },
            Vec::new(),
        );
        output.paint(
            target,
            true,
            [1, 1],
            [6, 7],
            Damage::full(1, 1),
            &[1, 2, 3, 255],
        );
        let pending = output.pending.lock().unwrap();
        let (event, bytes) = &pending[&(target, 2)];
        assert!(matches!(
            event,
            Event::Frame {
                width: 1,
                height: 1,
                x: 6,
                y: 7,
                ..
            }
        ));
        assert_eq!(bytes, &[1, 2, 3, 255]);
    }

    #[test]
    fn input_wake_preserves_future_chromium_deadline() {
        let now = std::time::Instant::now();
        let at = now + std::time::Duration::from_millis(20);
        let mut wake = super::Wake {
            next: Some(at),
            input: true,
        };
        wake.begin(now);
        assert_eq!(wake.next, Some(at));
        assert!(!wake.input);
        wake.begin(at);
        assert_eq!(wake.next, None);
    }

    #[test]
    fn valid_preview_urls_have_no_local_file_or_script_access() {
        for url in [
            "about:blank",
            "http://localhost:3000/",
            "https://example.com/a?b=1",
            "http://[::1]:8080/",
        ] {
            assert!(super::valid_url(url));
        }
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,test",
            "https://",
            "about:settings",
            "http://localhost\n:80",
        ] {
            assert!(!super::valid_url(url));
        }
    }
}
