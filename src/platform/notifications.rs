//! Native OS notification delivery stays outside interactive frames.
use eframe::egui;
use neptune_model::PaneId;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

struct Request {
    title: String,
    body: String,
    live: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct DesktopNotifier {
    sender: Option<mpsc::SyncSender<Request>>,
    sessions: BTreeMap<PaneId, Arc<AtomicBool>>,
    results: Option<mpsc::Receiver<bool>>,
    last_sent: Option<Instant>,
    pub unavailable: bool,
}

impl DesktopNotifier {
    /// At most one OS banner per second globally; the in-app history is independent.
    pub fn show(&mut self, pane: PaneId, title: String, body: String, ctx: &egui::Context) {
        if self
            .last_sent
            .is_some_and(|last| last.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        if self.sender.is_none() {
            let (sender, receiver) = mpsc::sync_channel::<Request>(8);
            let (result_sender, results) = mpsc::sync_channel(8);
            let wake = ctx.clone();
            let worker = std::thread::Builder::new()
                .name("neptune-notifications".into())
                .spawn(move || {
                    #[cfg(target_os = "linux")]
                    let mut delivery = linux::Delivery::default();
                    #[cfg(not(target_os = "linux"))]
                    let initialized = initialize();
                    while let Ok(request) = receiver.recv() {
                        if !request.live.load(Ordering::Acquire) {
                            continue;
                        }
                        #[cfg(target_os = "linux")]
                        let delivered = delivery.send(&request.title, &request.body).is_ok();
                        #[cfg(not(target_os = "linux"))]
                        let delivered = initialized && deliver(&request.title, &request.body);
                        let _ = result_sender.try_send(delivered);
                        wake.request_repaint();
                    }
                });
            if worker.is_err() {
                self.unavailable = true;
                return;
            }
            self.sender = Some(sender);
            self.results = Some(results);
        }
        if let Some(sender) = &self.sender {
            // On Linux the notification service interprets body markup. Escape
            // terminal-supplied text rather than allowing links or image markup.
            #[cfg(target_os = "linux")]
            let body = body
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            let live = self
                .sessions
                .entry(pane)
                .or_insert_with(|| Arc::new(AtomicBool::new(true)))
                .clone();
            if sender.try_send(Request { title, body, live }).is_ok() {
                self.last_sent = Some(Instant::now());
            }
        }
    }

    /// Invalidate queued work before a pane is removed or its session replaced.
    pub fn cancel(&mut self, pane: PaneId) {
        if let Some(live) = self.sessions.remove(&pane) {
            live.store(false, Ordering::Release);
        }
    }

    pub fn cancel_all(&mut self) {
        for (_, live) in std::mem::take(&mut self.sessions) {
            live.store(false, Ordering::Release);
        }
    }

    pub fn poll(&mut self) {
        if let Some(results) = &self.results {
            for delivered in results.try_iter() {
                self.unavailable = !delivered;
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn initialize() -> bool {
    // Match the installed Neptune.app bundle, rather than impersonating Finder.
    mac_notification_sys::set_application("rs.neptune.terminal").is_ok()
}

#[cfg(windows)]
fn initialize() -> bool {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    let result = (|| -> std::io::Result<()> {
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(r"Software\Classes\AppUserModelId\rs.neptune.terminal")?;
        key.set_value("DisplayName", &"Neptune")?;
        key.set_value("ShowInSettings", &1_u32)?;
        Ok(())
    })();
    result.is_ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn initialize() -> bool {
    true
}

#[cfg(target_os = "macos")]
fn deliver(title: &str, body: &str) -> bool {
    // notify-rust's legacy macOS handle sends on Drop and hides send errors.
    // Call its native backend directly so the popover can report a failure.
    mac_notification_sys::Notification::new()
        .title(title)
        .message(body)
        .asynchronous(true)
        .send()
        .is_ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn deliver(title: &str, body: &str) -> bool {
    let mut notification = notify_rust::Notification::new();
    notification.appname("Neptune").summary(title).body(body);
    #[cfg(windows)]
    notification.app_id("rs.neptune.terminal");
    notification.show().is_ok()
}

#[cfg(target_os = "linux")]
mod linux {
    use std::{collections::HashMap, time::Duration};
    use zbus::{blocking::Connection, zvariant::Value};

    #[derive(Default)]
    pub(super) struct Delivery {
        connection: Option<Connection>,
    }

    impl Delivery {
        pub(super) fn send(&mut self, title: &str, body: &str) -> zbus::Result<()> {
            if self.connection.is_none() {
                self.connection = Some(
                    zbus::blocking::connection::Builder::session()?
                        .max_queued(8)
                        .method_timeout(Duration::from_secs(5))
                        .build()?,
                );
            }
            // Each alert has a fresh ID but the same sender. Dropping a
            // notify-rust handle after show() closes its per-alert connection;
            // GNOME then destroys the application's entire notification source.
            let result = self.connection.as_ref().unwrap().call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "Notify",
                &(
                    "Neptune",
                    0_u32,
                    "neptune",
                    title,
                    body,
                    Vec::<&str>::new(),
                    HashMap::from([("desktop-entry", Value::from("rs.neptune.terminal"))]),
                    -1_i32,
                ),
            );
            match result.and_then(|reply| reply.body().deserialize::<u32>()) {
                Ok(_) => Ok(()),
                Err(error) => {
                    // A disconnected bus or a restarted service must be able
                    // to recover on the next alert, without blocking a frame.
                    self.connection = None;
                    Err(error)
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{
            io::{BufRead, BufReader},
            process::{Child, Command, Stdio},
            sync::{
                Arc,
                atomic::{AtomicBool, Ordering},
                mpsc,
            },
        };
        use zbus::{blocking::connection::Builder, message::Header, zvariant::OwnedValue};

        struct PrivateBus(Child);

        impl Drop for PrivateBus {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        struct Received {
            sender: String,
            replaces: u32,
            title: String,
            body: String,
            desktop_entry: String,
        }

        struct NotificationService {
            received: mpsc::SyncSender<Received>,
            next_id: u32,
            fail: Arc<AtomicBool>,
        }

        #[zbus::interface(name = "org.freedesktop.Notifications")]
        impl NotificationService {
            // The freedesktop Notify signature has eight arguments.
            #[allow(clippy::too_many_arguments)]
            fn notify(
                &mut self,
                _app: &str,
                replaces: u32,
                _icon: &str,
                title: &str,
                body: &str,
                _actions: Vec<String>,
                hints: HashMap<String, OwnedValue>,
                _timeout: i32,
                #[zbus(header)] header: Header<'_>,
            ) -> zbus::fdo::Result<u32> {
                if self.fail.load(Ordering::Acquire) {
                    return Err(zbus::fdo::Error::Failed("Synthetic failure".into()));
                }
                self.received
                    .try_send(Received {
                        sender: header.sender().unwrap().to_string(),
                        replaces,
                        title: title.into(),
                        body: body.into(),
                        desktop_entry: String::try_from(
                            hints.get("desktop-entry").unwrap().try_clone().unwrap(),
                        )
                        .unwrap(),
                    })
                    .unwrap();
                self.next_id += 1;
                Ok(self.next_id)
            }
        }

        #[test]
        fn notifications_keep_the_same_sender_alive_between_distinct_alerts() {
            // A private daemon avoids the user's notifications and concurrent
            // tests; no process-wide environment variables are changed.
            let mut bus = PrivateBus(
                Command::new("dbus-daemon")
                    .args(["--session", "--nofork", "--print-address=1"])
                    .stdout(Stdio::piped())
                    .spawn()
                    .expect("Linux notification regression requires dbus-daemon"),
            );
            let mut address = String::new();
            BufReader::new(bus.0.stdout.take().unwrap())
                .read_line(&mut address)
                .unwrap();
            let address = address.trim();
            let (received, requests) = mpsc::sync_channel(8);
            let fail = Arc::new(AtomicBool::new(false));
            let server = Builder::address(address)
                .unwrap()
                .name("org.freedesktop.Notifications")
                .unwrap()
                .serve_at(
                    "/org/freedesktop/Notifications",
                    NotificationService {
                        received,
                        next_id: 0,
                        fail: fail.clone(),
                    },
                )
                .unwrap()
                .build()
                .unwrap();
            let observer = zbus::blocking::fdo::DBusProxy::new(&server).unwrap();
            let mut delivery = Delivery {
                connection: Some(Builder::address(address).unwrap().build().unwrap()),
            };
            let mut original_sender = None;
            for title in ["First completion", "Second completion"] {
                delivery.send(title, "Synthetic body &lt;tag&gt;").unwrap();
                let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
                assert_eq!(
                    request.replaces, 0,
                    "each alert gets its own OS history entry"
                );
                assert_eq!(request.title, title);
                assert_eq!(request.body, "Synthetic body &lt;tag&gt;");
                assert_eq!(request.desktop_entry, "rs.neptune.terminal");
                assert!(
                    observer
                        .name_has_owner(request.sender.as_str().try_into().unwrap())
                        .unwrap(),
                    "GNOME destroys app notifications when this sender disappears"
                );
                if let Some(sender) = &original_sender {
                    assert_eq!(sender, &request.sender);
                } else {
                    original_sender = Some(request.sender);
                }
            }
            // Failed delivery releases a broken transport so the next request
            // can reconnect. Backend errors never contain terminal text in UI.
            fail.store(true, Ordering::Release);
            assert!(delivery.send("Service gone", "").is_err());
            assert!(delivery.connection.is_none());
        }
    }
}

impl Drop for DesktopNotifier {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notifications_cancel_queued_delivery_without_affecting_other_panes() {
        let mut notifier = DesktopNotifier::default();
        let old = Arc::new(AtomicBool::new(true));
        let other = Arc::new(AtomicBool::new(true));
        notifier.sessions.insert(PaneId::new(1), old.clone());
        notifier.sessions.insert(PaneId::new(2), other.clone());
        notifier.cancel(PaneId::new(1));
        assert!(!old.load(Ordering::Acquire));
        assert!(other.load(Ordering::Acquire));
        drop(notifier);
        assert!(!other.load(Ordering::Acquire));
    }
}
