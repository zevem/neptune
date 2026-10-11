//! What a preview does beyond showing a page: saved profiles, cookies brought
//! from another browser, picking an element and recording. Each drives
//! Chromium through its DevTools protocol or cookie store from this process;
//! a page is given nothing it can call.
use super::{
    Entry, Output,
    handlers::{self, Reply},
    protocol::{self, Event, Target},
};
use cef::*;
use std::{
    collections::BTreeMap,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicBool, atomic::Ordering},
    time::{Duration, Instant},
};

/// Resolves once with the picked element as JSON, or null when cancelled.
const PICK: &str = include_str!("pick.js");
const CANCEL_PICK: &str = "window[Symbol.for('neptune.pick.cancel')]?.()";
/// Defines `__neptuneRecorder` in the blank page that encodes a recording.
const RECORDER: &str = include_str!("recorder.js");
/// The encoder is given at most this many page frames a second.
const FRAME_INTERVAL: Duration = Duration::from_millis(30);
/// A recording ends itself here, with what it has.
const MAX_RECORDING: Duration = Duration::from_secs(10 * 60);
/// Seconds Unix time is behind Chromium's, which counts from 1601.
const CHROMIUM_EPOCH: i64 = 11_644_473_600;

/// What a DevTools reply is for, by the id its request was sent with.
pub(super) enum Purpose {
    Pick,
    Stop,
    Chunk(u32),
}

/// A recording under way: the page's frames go to an encoder in a blank page
/// of its own, so the page under view is never changed to record it.
pub(super) struct Recording {
    scratch: Browser,
    _observer: Option<Registration>,
    closed: Arc<AtomicBool>,
    path: PathBuf,
    /// The encoder page has loaded and frames are being sent to it.
    ready: bool,
    stopping: bool,
    started: Instant,
    last_frame: Option<Instant>,
    chunks: u32,
    file: Option<std::fs::File>,
    bytes: u64,
    pending: BTreeMap<i32, Purpose>,
    next: i32,
}

#[derive(Default)]
pub(super) struct Tally {
    pending: u32,
    imported: u32,
    skipped: u32,
    last: bool,
}

pub(super) struct Tools {
    pub replies: handlers::Replies,
    /// Encoder pages that were told to close and have yet to.
    pub closing: Vec<(Browser, Arc<AtomicBool>)>,
    root: PathBuf,
    /// Neptune gave this host the saved profiles, rather than a temporary root.
    saved: bool,
    contexts: BTreeMap<String, (RequestContext, Arc<AtomicBool>)>,
    /// Browsers to open once their saved profile's files are.
    pub waiting: Vec<(Target, String)>,
    /// Commands for a browser whose profile is still opening.
    pub held: Vec<(Target, protocol::Command)>,
    tally: Arc<Mutex<Tally>>,
    output: Arc<Output>,
}

fn send(host: &BrowserHost, id: i32, method: &str, params: serde_json::Value) -> bool {
    let message = serde_json::json!({ "id": id, "method": method, "params": params });
    serde_json::to_vec(&message).is_ok_and(|bytes| host.send_dev_tools_message(Some(&bytes)) != 0)
}

fn evaluate(host: &BrowserHost, id: i32, expression: &str) -> bool {
    send(
        host,
        id,
        "Runtime.evaluate",
        serde_json::json!({ "expression": expression, "awaitPromise": true, "returnByValue": true }),
    )
}

/// The value a `Runtime.evaluate` reply carries, unless the script threw.
fn value(body: &[u8]) -> Option<serde_json::Value> {
    let mut reply: serde_json::Value = serde_json::from_slice(body).ok()?;
    if reply.get("exceptionDetails").is_some() {
        return None;
    }
    Some(reply.get_mut("result")?.get_mut("value")?.take())
}

fn run(browser: &Browser, code: &str) {
    if let Some(frame) = browser.main_frame() {
        frame.execute_java_script(Some(&code.into()), None, 0);
    }
}

impl Entry {
    fn id(&mut self) -> i32 {
        self.next = self.next.wrapping_add(1).max(1);
        self.next
    }

    pub(super) fn pick(&mut self, active: bool) {
        let Some(host) = self.browser.host() else {
            return;
        };
        let id = self.id();
        if !active {
            evaluate(&host, id, CANCEL_PICK);
        } else if !self.data.picking() && evaluate(&host, id, PICK) {
            self.pending.insert(id, Purpose::Pick);
            self.data.update(|s| s.picking = true);
        }
    }

    /// A reply from the page's own DevTools session.
    fn page_reply(&mut self, output: &Output, id: i32, success: bool, body: &[u8]) {
        if !matches!(self.pending.remove(&id), Some(Purpose::Pick)) {
            return;
        }
        self.data.update(|s| s.picking = false);
        // A navigation ends the script without a value, as a cancel does.
        if success
            && let Some(serde_json::Value::String(pick)) = value(body)
            && pick.len() <= protocol::MAX_PICK
        {
            let target = self.data.target();
            output.put(target, 3, Event::Picked { target, pick }, Vec::new());
        }
    }

    pub(super) fn pointer(&self, x: i32, y: i32, kind: &protocol::Mouse) {
        let Some(recording) = self.recording.as_ref().filter(|r| r.ready && !r.stopping) else {
            return;
        };
        let code = match kind {
            protocol::Mouse::Move { leave: true } => "__neptuneRecorder.cursor(-1,-1,false)".into(),
            protocol::Mouse::Button {
                button: 0, pressed, ..
            } => format!("__neptuneRecorder.cursor({x},{y},{pressed})"),
            _ => format!("__neptuneRecorder.cursor({x},{y})"),
        };
        run(&recording.scratch, &code);
    }
}

impl Tools {
    pub(super) fn new(root: PathBuf, output: Arc<Output>) -> Self {
        Self {
            replies: Default::default(),
            closing: Vec::new(),
            root,
            saved: std::env::var_os("NEPTUNE_BROWSER_SAVED").is_some(),
            contexts: BTreeMap::new(),
            waiting: Vec::new(),
            held: Vec::new(),
            tally: Default::default(),
            output,
        }
    }

    /// Whether a browser asked to use this profile gets a saved one.
    pub(super) fn saves(&self, profile: Option<&str>) -> bool {
        self.saved && profile.is_some_and(protocol::valid_profile)
    }

    /// A saved profile's cookies and storage, kept in a directory of its own
    /// and shared by its browsers, and whether Chromium has opened it.
    pub(super) fn context(&mut self, id: &str) -> Option<(RequestContext, bool)> {
        if !self.saves(Some(id)) {
            return None;
        }
        if let Some((context, ready)) = self.contexts.get(id) {
            return Some((context.clone(), ready.load(Ordering::Acquire)));
        }
        // Chromium keeps a profile only in a direct child of its root.
        let directory = self.root.join(protocol::profile_directory(id));
        std::fs::create_dir_all(&directory).ok()?;
        let ready = Arc::new(AtomicBool::new(false));
        let context = cef::request_context_create_context(
            Some(&RequestContextSettings {
                cache_path: directory.to_string_lossy().as_ref().into(),
                ..Default::default()
            }),
            Some(&mut handlers::Opened::new(ready.clone())),
        )?;
        self.contexts
            .insert(id.to_owned(), (context.clone(), ready.clone()));
        Some((context, ready.load(Ordering::Acquire)))
    }

    pub(super) fn set_cookies(
        &mut self,
        profile: &str,
        cookies: Vec<protocol::Cookie>,
        last: bool,
    ) {
        let manager = self
            .context(profile)
            .and_then(|(context, _)| context.cookie_manager(None));
        let mut settled = Vec::new();
        for cookie in cookies.into_iter().take(protocol::MAX_COOKIES) {
            let host = cookie.host.trim_start_matches('.');
            let url = format!(
                "{}://{}{}",
                if cookie.secure { "https" } else { "http" },
                if host.contains(':') {
                    format!("[{host}]")
                } else {
                    host.to_owned()
                },
                cookie.path
            );
            let stored = Cookie {
                name: cookie.name.as_str().into(),
                value: cookie.value.as_str().into(),
                // A host-only cookie names no domain, or it would widen to
                // the host's subdomains.
                domain: if cookie.host.starts_with('.') {
                    cookie.host.as_str().into()
                } else {
                    CefString::default()
                },
                path: cookie.path.as_str().into(),
                secure: i32::from(cookie.secure),
                httponly: i32::from(cookie.http_only),
                has_expires: i32::from(cookie.expires.is_some()),
                expires: Basetime {
                    val: cookie
                        .expires
                        .unwrap_or(0)
                        .saturating_add(CHROMIUM_EPOCH)
                        .saturating_mul(1_000_000),
                },
                same_site: match cookie.same_site {
                    0 => CookieSameSite::NO_RESTRICTION,
                    1 => CookieSameSite::LAX_MODE,
                    2 => CookieSameSite::STRICT_MODE,
                    _ => CookieSameSite::UNSPECIFIED,
                },
                ..Default::default()
            };
            let mut counted = handlers::Counted::new(self.tally.clone(), self.output.clone());
            // Count before asking: the answer may come within the call.
            if let Ok(mut tally) = self.tally.lock() {
                tally.pending += 1;
            }
            let asked = manager.as_ref().is_some_and(|manager| {
                manager.set_cookie(
                    Some(&url.as_str().into()),
                    Some(&stored),
                    Some(&mut counted),
                ) != 0
            });
            settled.push(asked);
        }
        if let Ok(mut tally) = self.tally.lock() {
            for asked in settled {
                if !asked {
                    tally.pending -= 1;
                    tally.skipped += 1;
                }
            }
            tally.last |= last;
            tally.finish(&self.output);
        }
        if last && let Some(manager) = manager {
            manager.flush_store(None);
        }
    }

    pub(super) fn clear_cookies(&mut self, profile: &str) {
        if let Some(manager) = self
            .context(profile)
            .and_then(|(context, _)| context.cookie_manager(None))
        {
            manager.delete_cookies(None, None, None);
            manager.flush_store(None);
        }
    }

    pub(super) fn record(&mut self, entry: &mut Entry, path: Option<String>) {
        let Some(path) = path else {
            self.stop(entry);
            return;
        };
        let path = PathBuf::from(path);
        if entry.recording.is_some()
            || path.extension().is_none_or(|e| e != "webm")
            || !path.is_absolute()
        {
            return;
        }
        let closed = Arc::new(AtomicBool::new(false));
        let target = entry.data.target();
        let mut client = handlers::EncoderClient::new(target, self.replies.clone(), closed.clone());
        let info = WindowInfo {
            windowless_rendering_enabled: 1,
            ..Default::default()
        };
        let Some(scratch) = cef::browser_host_create_browser_sync(
            Some(&info),
            Some(&mut client),
            Some(&"about:blank".into()),
            Some(&BrowserSettings::default()),
            None,
            None,
        ) else {
            self.recorded(target, 0, Some("Could not start the recorder."));
            return;
        };
        let observer = scratch.host().and_then(|host| {
            host.add_dev_tools_message_observer(Some(&mut handlers::Observer::new(
                target,
                true,
                self.replies.clone(),
            )))
        });
        entry.recording = Some(Recording {
            scratch,
            _observer: observer,
            closed,
            path,
            ready: false,
            stopping: false,
            started: Instant::now(),
            last_frame: None,
            chunks: 0,
            file: None,
            bytes: 0,
            pending: BTreeMap::new(),
            next: 0,
        });
        entry.data.update(|s| s.recording = true);
    }

    fn recorded(&self, target: Target, bytes: u64, error: Option<&str>) {
        self.output.put(
            target,
            4,
            Event::Recorded {
                target,
                bytes,
                error: error.map(str::to_owned),
            },
            Vec::new(),
        );
    }

    /// End a recording and say how it went; its encoder page closes.
    fn finish(&mut self, entry: &mut Entry, error: Option<&str>) {
        let Some(mut recording) = entry.recording.take() else {
            return;
        };
        drop(recording.file.take());
        if error.is_some() {
            let _ = std::fs::remove_file(&recording.path);
        }
        self.recorded(entry.data.target(), recording.bytes, error);
        entry.data.update(|s| s.recording = false);
        self.close(recording);
    }

    fn close(&mut self, recording: Recording) {
        if let Some(host) = recording.scratch.host() {
            host.close_browser(1);
        }
        self.closing.push((recording.scratch, recording.closed));
    }

    /// A browser going away takes its recording with it, unsaved.
    pub(super) fn abandon(&mut self, entry: &mut Entry) {
        if let Some(recording) = entry.recording.take() {
            self.close(recording);
        }
    }

    pub(super) fn stop(&mut self, entry: &mut Entry) {
        let Some(recording) = entry.recording.as_mut().filter(|r| !r.stopping) else {
            return;
        };
        recording.stopping = true;
        if let Some(host) = entry.browser.host() {
            send(&host, 0, "Page.stopScreencast", serde_json::json!({}));
        }
        recording.next += 1;
        let id = recording.next;
        if recording.ready
            && recording
                .scratch
                .host()
                .is_some_and(|host| evaluate(&host, id, "__neptuneRecorder.stop()"))
        {
            recording.pending.insert(id, Purpose::Stop);
        } else {
            self.finish(
                entry,
                Some("The recording ended before the page was shown."),
            );
        }
    }

    /// Recordings end themselves at their time limit.
    pub(super) fn tick(&mut self, entries: &mut BTreeMap<Target, Entry>) {
        for entry in entries.values_mut() {
            if entry
                .recording
                .as_ref()
                .is_some_and(|r| r.started.elapsed() > MAX_RECORDING)
            {
                self.stop(entry);
            }
        }
        self.closing
            .retain(|(_, closed)| !closed.load(Ordering::Acquire));
    }

    pub(super) fn reply(&mut self, entries: &mut BTreeMap<Target, Entry>, reply: Reply) {
        match reply {
            Reply::Result {
                target,
                encoder: false,
                id,
                success,
                body,
            } => {
                if let Some(entry) = entries.get_mut(&target) {
                    entry.page_reply(&self.output, id, success, &body);
                }
            }
            Reply::Result {
                target,
                encoder: true,
                id,
                success,
                body,
            } => {
                if let Some(entry) = entries.get_mut(&target) {
                    self.encoded(entry, id, success, &body);
                }
            }
            Reply::Loaded { target } => {
                let Some(entry) = entries.get_mut(&target) else {
                    return;
                };
                let Some(recording) = entry.recording.as_mut().filter(|r| !r.ready && !r.stopping)
                else {
                    return;
                };
                recording.ready = true;
                run(&recording.scratch, RECORDER);
                if let Some(host) = entry.browser.host() {
                    send(
                        &host,
                        0,
                        "Page.startScreencast",
                        serde_json::json!({ "format": "jpeg", "quality": 80, "everyNthFrame": 1 }),
                    );
                }
            }
            Reply::Frame { target, params } => {
                let Some(entry) = entries.get_mut(&target) else {
                    return;
                };
                let Ok(frame) = serde_json::from_slice::<serde_json::Value>(&params) else {
                    return;
                };
                // Chromium sends the next frame once this one is acknowledged.
                if let (Some(host), Some(session)) = (entry.browser.host(), frame.get("sessionId"))
                {
                    send(
                        &host,
                        0,
                        "Page.screencastFrameAck",
                        serde_json::json!({ "sessionId": session }),
                    );
                }
                let Some(recording) = entry.recording.as_mut().filter(|r| r.ready && !r.stopping)
                else {
                    return;
                };
                let now = Instant::now();
                if recording
                    .last_frame
                    .is_some_and(|last| now.duration_since(last) < FRAME_INTERVAL)
                {
                    return;
                }
                let width = frame["metadata"]["deviceWidth"].as_f64().unwrap_or(0.0);
                if let Some(data) = frame["data"].as_str()
                    && width >= 1.0
                    && data
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
                {
                    recording.last_frame = Some(now);
                    run(
                        &recording.scratch,
                        &format!("__neptuneRecorder.frame(\"{data}\",{width})"),
                    );
                }
            }
        }
    }

    /// A reply from the encoder page: how much it holds, then each part.
    fn encoded(&mut self, entry: &mut Entry, id: i32, success: bool, body: &[u8]) {
        let Some(recording) = entry.recording.as_mut() else {
            return;
        };
        let Some(purpose) = recording.pending.remove(&id) else {
            return;
        };
        let Some(value) = value(body).filter(|_| success) else {
            self.finish(entry, Some("The recording could not be encoded."));
            return;
        };
        let next = match purpose {
            Purpose::Pick => return,
            Purpose::Stop => {
                recording.chunks = value["chunks"].as_u64().unwrap_or(0).min(4096) as u32;
                if recording.chunks == 0 {
                    self.finish(entry, Some("The page showed nothing to record."));
                    return;
                }
                let Ok(file) = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&recording.path)
                else {
                    self.finish(entry, Some("The recording could not be saved."));
                    return;
                };
                recording.file = Some(file);
                0
            }
            Purpose::Chunk(index) => {
                let written = value
                    .as_str()
                    .and_then(decode)
                    .zip(recording.file.as_mut())
                    .and_then(|(bytes, file)| file.write_all(&bytes).ok().map(|()| bytes.len()));
                let Some(written) = written else {
                    self.finish(entry, Some("The recording could not be saved."));
                    return;
                };
                recording.bytes += written as u64;
                index + 1
            }
        };
        if next >= recording.chunks {
            self.finish(entry, None);
            return;
        }
        recording.next += 1;
        let id = recording.next;
        if recording
            .scratch
            .host()
            .is_some_and(|host| evaluate(&host, id, &format!("__neptuneRecorder.chunk({next})")))
        {
            recording.pending.insert(id, Purpose::Chunk(next));
        } else {
            self.finish(entry, Some("The recording could not be saved."));
        }
    }
}

impl Tally {
    fn finish(&mut self, output: &Output) {
        if self.last && self.pending == 0 {
            let target = Target {
                pane: 0,
                generation: 0,
            };
            output.put(
                target,
                5,
                Event::Imported {
                    imported: self.imported,
                    skipped: self.skipped,
                },
                Vec::new(),
            );
            *self = Self::default();
        }
    }
    pub(super) fn settle(&mut self, output: &Output, success: bool) {
        self.pending = self.pending.saturating_sub(1);
        if success {
            self.imported += 1;
        } else {
            self.skipped += 1;
        }
        self.finish(output);
    }
}

/// Standard base64, as `btoa` writes it.
fn decode(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(text.len() / 4 * 3);
    let (mut bits, mut count) = (0u32, 0);
    for byte in text.bytes() {
        let six = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        bits = bits << 6 | u32::from(six);
        count += 6;
        if count >= 8 {
            count -= 8;
            bytes.push((bits >> count) as u8);
        }
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    #[test]
    fn recording_parts_decode_as_the_page_encoded_them() {
        assert_eq!(super::decode("").unwrap(), b"");
        assert_eq!(super::decode("TQ==").unwrap(), b"M");
        assert_eq!(super::decode("TWE=").unwrap(), b"Ma");
        assert_eq!(
            super::decode("GkXfo/8A").unwrap(),
            [0x1a, 0x45, 0xdf, 0xa3, 0xff, 0]
        );
        assert!(super::decode("a b").is_none());
    }

    #[test]
    fn a_script_that_threw_or_navigated_away_has_no_value() {
        assert_eq!(
            super::value(br#"{"result":{"type":"string","value":"{}"}}"#),
            Some(serde_json::json!("{}"))
        );
        assert_eq!(
            super::value(br#"{"result":{"type":"object","subtype":"null","value":null}}"#),
            Some(serde_json::Value::Null)
        );
        assert!(super::value(br#"{"result":{"type":"object"},"exceptionDetails":{}}"#).is_none());
        assert!(super::value(b"not json").is_none());
    }
}
