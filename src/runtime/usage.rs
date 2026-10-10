//! Read-only subscription probes. One worker owns the CLIs, credentials and
//! network; frames only take bounded, owned readings. No transcript is read.
mod probes;

use eframe::egui;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const INTERVAL: Duration = Duration::from_secs(5 * 60);
const MAX_WINDOWS: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Codex,
    Claude,
    Cursor,
}
impl Provider {
    pub const ALL: [Self; 3] = [Self::Codex, Self::Claude, Self::Cursor];
    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Cursor => "Cursor",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub label: String,
    pub used_percent: f64,
    pub resets_at: Option<u64>,
}

/// Only bounded metadata crosses the worker boundary, never raw provider errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    NotInstalled,
    SignIn,
    Unsupported,
    Failed,
    Keychain,
}
impl Unavailable {
    pub fn message(self, provider: Provider) -> &'static str {
        match self {
            Self::NotInstalled => "CLI not found on this computer.",
            Self::SignIn => match provider {
                Provider::Codex => "Sign in with codex login, then refresh.",
                Provider::Claude => "Sign in with claude auth login, then refresh.",
                Provider::Cursor => "Sign in with agent login, then refresh.",
            },
            Self::Unsupported => "This login or CLI does not report subscription limits.",
            Self::Failed => "Could not read usage. Try refreshing.",
            Self::Keychain => "Allow access to Cursor’s saved Keychain login to read usage.",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Reading {
    pub provider: Provider,
    pub windows: Vec<Window>,
    pub plan: Option<String>,
    pub checked_at: Option<SystemTime>,
    pub unavailable: Option<Unavailable>,
}
impl Reading {
    fn empty(provider: Provider) -> Self {
        Self {
            provider,
            windows: Vec::new(),
            plan: None,
            checked_at: None,
            unavailable: None,
        }
    }
    /// The most consumed allowance is the one closest to blocking work.
    pub fn summary(&self) -> Option<&Window> {
        self.windows
            .iter()
            .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
    }
    fn accept(&mut self, result: Result<(Vec<Window>, Option<String>), Unavailable>) {
        match result {
            Ok((windows, plan)) => {
                self.windows = windows;
                self.plan = plan;
                self.checked_at = Some(SystemTime::now());
                self.unavailable = None;
            }
            Err(error) => {
                // A transient failure retains the last good snapshot, explicitly stale.
                if error != Unavailable::Failed {
                    self.windows.clear();
                    self.plan = None;
                    self.checked_at = None;
                }
                self.unavailable = Some(error);
            }
        }
    }
}

pub struct Usage {
    pub readings: [Reading; 3],
    pub refreshing: bool,
    requests: Option<mpsc::SyncSender<bool>>,
    replies: Option<mpsc::Receiver<Reply>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    keychain: bool,
}
enum Reply {
    Checking,
    Reading(usize, Reading),
    Done,
}
impl Default for Usage {
    fn default() -> Self {
        Self {
            readings: Provider::ALL.map(Reading::empty),
            refreshing: false,
            requests: None,
            replies: None,
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
            keychain: false,
        }
    }
}
impl Usage {
    /// Called after bootstrap, never for ephemeral captures or headless fixtures.
    pub fn start(&mut self, ctx: &egui::Context) {
        if self.worker.is_some() {
            return;
        }
        let (send, requests) = mpsc::sync_channel(1);
        let (replies, receive) = mpsc::sync_channel(5);
        let stop = Arc::clone(&self.stop);
        let wake = ctx.clone();
        let mut keychain = self.keychain;
        let spawned = thread::Builder::new()
            .name("neptune-usage".into())
            .spawn(move || {
                let mut readings = Provider::ALL.map(Reading::empty);
                loop {
                    if stop.load(Ordering::Acquire) || replies.send(Reply::Checking).is_err() {
                        return;
                    }
                    wake.request_repaint();
                    for (index, reading) in readings.iter_mut().enumerate() {
                        if stop.load(Ordering::Acquire) {
                            return;
                        }
                        reading.accept(probes::read(reading.provider, keychain, &stop));
                        // One complete cycle fits the bounded mailbox. Shutdown
                        // drops its receiver, releasing a blocked sender too.
                        if replies
                            .send(Reply::Reading(index, reading.clone()))
                            .is_err()
                        {
                            return;
                        }
                        wake.request_repaint();
                    }
                    if replies.send(Reply::Done).is_err() {
                        return;
                    }
                    wake.request_repaint();
                    // Requests received during the probe are coalesced into one follow-up.
                    match requests.recv_timeout(INTERVAL) {
                        Ok(allow) => keychain |= allow,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
            });
        match spawned {
            Ok(worker) => {
                self.worker = Some(worker);
                self.requests = Some(send);
                self.replies = Some(receive);
                self.refreshing = true;
            }
            Err(_) => {
                for reading in &mut self.readings {
                    reading.unavailable = Some(Unavailable::Failed);
                }
            }
        }
    }
    pub fn poll(&mut self) {
        if let Some(replies) = &self.replies {
            while let Ok(reply) = replies.try_recv() {
                match reply {
                    Reply::Checking => self.refreshing = true,
                    Reply::Reading(index, reading) => self.readings[index] = reading,
                    Reply::Done => self.refreshing = false,
                }
            }
        }
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
        {
            self.refreshing = false;
            for reading in &mut self.readings {
                reading.accept(Err(Unavailable::Failed));
            }
            self.requests = None;
            self.replies = None;
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
    pub fn refresh(&mut self, ctx: &egui::Context, keychain: bool) {
        if self.refreshing && (!keychain || self.keychain) {
            return;
        }
        self.keychain |= keychain;
        if self.worker.is_none() {
            self.start(ctx);
        }
        if let Some(requests) = &self.requests {
            // Repeated refresh clicks do not accumulate jobs or processes.
            if requests.try_send(self.keychain).is_ok() {
                self.refreshing = true;
            }
        }
    }
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.requests = None;
        self.replies = None;
        // Final exit only. Give cancelled CLI probes time to kill and reap
        // their children before the process exits, without waiting on HTTP.
        let deadline = Instant::now() + Duration::from_millis(250);
        while self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(5));
        }
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
            && let Some(worker) = self.worker.take()
        {
            let _ = worker.join();
        }
    }
}
impl Drop for Usage {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// RFC3339 reset instants, using the existing TOML parser for validation.
fn timestamp(text: &str) -> Option<u64> {
    let value: toml::value::Datetime = text.parse().ok()?;
    let date = value.date?;
    let time = value.time?;
    let offset = match value.offset? {
        toml::value::Offset::Z => 0,
        toml::value::Offset::Custom { minutes } => i64::from(minutes) * 60,
    };
    let year = i64::from(date.year) - i64::from(date.month <= 2);
    let era = year.div_euclid(400);
    let y = year - era * 400;
    let month = i64::from(date.month) + if date.month > 2 { -3 } else { 9 };
    let days =
        era * 146097 + y * 365 + y / 4 - y / 100 + (153 * month + 2) / 5 + i64::from(date.day)
            - 1
            - 719468;
    let seconds = days * 86400
        + i64::from(time.hour) * 3600
        + i64::from(time.minute) * 60
        + i64::from(time.second)
        - offset;
    u64::try_from(seconds).ok().filter(|seconds| *seconds > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reset_instants_respect_offsets_and_reject_missing_zones() {
        assert_eq!(timestamp("2026-10-09T12:30:00Z"), Some(1791549000));
        assert_eq!(
            timestamp("2026-10-09T07:30:00-05:00"),
            timestamp("2026-10-09T12:30:00Z")
        );
        assert_eq!(timestamp("2026-10-09T12:30:00"), None);
        assert_eq!(timestamp("broken"), None);
    }
    #[test]
    fn failure_keeps_last_reading_but_logout_clears_it() {
        let mut reading = Reading::empty(Provider::Codex);
        reading.accept(Ok((
            vec![Window {
                label: "Session".into(),
                used_percent: 42.0,
                resets_at: None,
            }],
            None,
        )));
        let checked = reading.checked_at;
        reading.accept(Err(Unavailable::Failed));
        assert_eq!(reading.windows[0].used_percent, 42.0);
        assert_eq!(reading.checked_at, checked);
        assert_eq!(reading.unavailable, Some(Unavailable::Failed));
        reading.accept(Err(Unavailable::SignIn));
        assert!(reading.windows.is_empty());
    }
    #[test]
    fn summary_shows_an_exhausted_weekly_allowance_even_with_session_quota_left() {
        let mut reading = Reading::empty(Provider::Codex);
        reading.accept(Ok((
            vec![
                Window {
                    label: "Session".into(),
                    used_percent: 12.0,
                    resets_at: None,
                },
                Window {
                    label: "Weekly".into(),
                    used_percent: 100.0,
                    resets_at: None,
                },
            ],
            None,
        )));
        assert_eq!(reading.summary().unwrap().label, "Weekly");
    }
}
