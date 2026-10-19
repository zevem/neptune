//! The light or dark mode the operating system asks applications to use.
//! macOS and Windows report it through the window toolkit, which repaints
//! when it changes. Linux desktops report it through the XDG desktop portal,
//! which is read and followed on a worker so a slow bus never holds a frame.
use crate::config::Appearance;
use eframe::egui;

#[derive(Default)]
pub struct SystemAppearance {
    /// The latest mode the portal reported; the worker lives for the process.
    #[cfg(target_os = "linux")]
    portal: Option<std::sync::Arc<std::sync::atomic::AtomicU8>>,
    /// What a headless test has the system report, in place of the desktop.
    #[cfg(test)]
    pub reporting: Option<Option<Appearance>>,
    /// Windows left to follow the system rather than a theme of their own.
    released: std::collections::HashSet<egui::ViewportId>,
}

impl SystemAppearance {
    /// The mode the system asks for now, if it says. On Linux the first call
    /// starts listening, and the answer arrives with a repaint.
    pub fn current(&mut self, ctx: &egui::Context) -> Option<Appearance> {
        #[cfg(test)]
        if let Some(reported) = self.reporting {
            return reported;
        }
        #[cfg(target_os = "linux")]
        {
            let latest = self.portal.get_or_insert_with(|| linux::watch(ctx.clone()));
            linux::decode(latest.load(std::sync::atomic::Ordering::Acquire))
        }
        #[cfg(not(target_os = "linux"))]
        ctx.system_theme().map(|theme| match theme {
            egui::Theme::Light => Appearance::Light,
            egui::Theme::Dark => Appearance::Dark,
        })
    }

    /// egui sets each window's theme to match its style. On macOS and Windows
    /// a window with a theme of its own hears nothing more of the system's
    /// mode, so while themes follow the system each window is left to it,
    /// once. Afterwards each window takes the theme in use again. Linux
    /// windows hear nothing either way, and keep theirs.
    pub fn leave_windows_to_system(&mut self, ctx: &egui::Context, following: bool, dark: bool) {
        if !cfg!(target_os = "linux") {
            self.hand_over(ctx, following, dark);
        }
    }

    fn hand_over(&mut self, ctx: &egui::Context, following: bool, dark: bool) {
        if ctx.options(|options| options.sync_window_theme) == following {
            ctx.options_mut(|options| options.sync_window_theme = !following);
        }
        if following {
            if self.released.insert(ctx.viewport_id()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(
                    egui::SystemTheme::SystemDefault,
                ));
            }
            return;
        }
        let theme = if dark {
            egui::SystemTheme::Dark
        } else {
            egui::SystemTheme::Light
        };
        for viewport in self.released.drain() {
            ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::SetTheme(theme));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window themes one pass of the main window sets.
    fn pass(ctx: &egui::Context, mut run: impl FnMut(&egui::Context)) -> Vec<egui::SystemTheme> {
        let mut output = ctx.run_ui(Default::default(), |ui| run(ui.ctx()));
        output.textures_delta.clear();
        output
            .viewport_output
            .remove(&egui::ViewportId::ROOT)
            .map_or_else(Vec::new, |output| output.commands)
            .into_iter()
            .filter_map(|command| match command {
                egui::ViewportCommand::SetTheme(theme) => Some(theme),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn windows_are_left_to_the_system_once_while_following_it() {
        let ctx = egui::Context::default();
        let mut appearance = SystemAppearance::default();
        // egui sets the window's theme to match its style.
        let styled = pass(&ctx, |ctx| ctx.set_theme(egui::Theme::Dark));
        assert_eq!(styled, [egui::SystemTheme::Dark]);

        let mut follow = |ctx: &egui::Context| {
            appearance.hand_over(ctx, true, false);
            ctx.set_theme(egui::Theme::Light);
        };
        assert_eq!(pass(&ctx, &mut follow), [egui::SystemTheme::SystemDefault]);
        // Not again, and egui does not pin it while the style changes.
        assert!(pass(&ctx, &mut follow).is_empty());

        // Afterwards the window takes the theme in use again.
        let unfollow = pass(&ctx, |ctx| appearance.hand_over(ctx, false, false));
        assert!(!unfollow.is_empty());
        assert!(
            unfollow
                .iter()
                .all(|theme| *theme == egui::SystemTheme::Light)
        );
        assert!(ctx.options(|options| options.sync_window_theme));
        assert!(appearance.released.is_empty());
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use crate::config::Appearance;
    use eframe::egui;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU8, Ordering},
        },
        time::Duration,
    };
    use zbus::{
        blocking::{Connection, Proxy},
        zvariant::{OwnedValue, Value},
    };

    const NAMESPACE: &str = "org.freedesktop.appearance";
    const KEY: &str = "color-scheme";
    const UNKNOWN: u8 = 0;
    const LIGHT: u8 = 1;
    const DARK: u8 = 2;

    pub(super) fn decode(value: u8) -> Option<Appearance> {
        match value {
            LIGHT => Some(Appearance::Light),
            DARK => Some(Appearance::Dark),
            _ => None,
        }
    }

    /// Follows the portal on a worker. Without a session bus or a portal
    /// that knows the colour scheme, the mode stays unknown.
    pub(super) fn watch(ctx: egui::Context) -> Arc<AtomicU8> {
        let latest = Arc::new(AtomicU8::new(UNKNOWN));
        let report = latest.clone();
        let _ = std::thread::Builder::new()
            .name("neptune-appearance".into())
            .spawn(move || {
                let Ok(connection) = zbus::blocking::connection::Builder::session()
                    .and_then(|builder| builder.method_timeout(Duration::from_secs(10)).build())
                else {
                    return;
                };
                let _ = follow(&connection, |mode| {
                    let value = match mode {
                        Some(Appearance::Light) => LIGHT,
                        Some(Appearance::Dark) => DARK,
                        None => UNKNOWN,
                    };
                    if report.swap(value, Ordering::AcqRel) != value {
                        ctx.request_repaint();
                    }
                });
            });
        latest
    }

    /// Reports the colour scheme, then each change to it, until the bus closes.
    fn follow(
        connection: &Connection,
        mut report: impl FnMut(Option<Appearance>),
    ) -> zbus::Result<()> {
        let portal = Proxy::new(
            connection,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
        )?;
        // Listen before reading, so a change between the two is not lost.
        let changes =
            portal.receive_signal_with_args("SettingChanged", &[(0, NAMESPACE), (1, KEY)])?;
        // Portals older than version 2 offer only the deprecated Read.
        let value: OwnedValue = portal
            .call("ReadOne", &(NAMESPACE, KEY))
            .or_else(|_| portal.call("Read", &(NAMESPACE, KEY)))?;
        report(scheme(&value));
        for message in changes {
            if let Ok((_, _, value)) = message.body().deserialize::<(String, String, OwnedValue)>()
            {
                report(scheme(&value));
            }
        }
        Ok(())
    }

    /// 1 asks for dark and 2 for light. 0, no preference, is what GNOME's
    /// light mode reports, so it is light, as GTK and libadwaita take it.
    fn scheme(value: &Value) -> Option<Appearance> {
        match value {
            // The deprecated Read wraps the setting in one more variant.
            Value::Value(inner) => scheme(inner),
            Value::U32(1) => Some(Appearance::Dark),
            Value::U32(0 | 2) => Some(Appearance::Light),
            _ => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{
            io::{BufRead, BufReader},
            process::{Child, Command, Stdio},
            sync::mpsc,
        };
        use zbus::blocking::connection::Builder;

        struct PrivateBus {
            daemon: Child,
            /// Holds the daemon's configuration and socket.
            _root: tempfile::TempDir,
        }

        impl Drop for PrivateBus {
            fn drop(&mut self) {
                let _ = self.daemon.kill();
                let _ = self.daemon.wait();
            }
        }

        fn private_bus() -> (PrivateBus, String) {
            // A private daemon avoids the user's desktop and concurrent tests;
            // no process-wide environment variables are changed. It lists no
            // service directories: the session's would start the real portal.
            let root = tempfile::tempdir().unwrap();
            let config = root.path().join("bus.conf");
            std::fs::write(
                &config,
                format!(
                    r#"<busconfig>
  <type>session</type>
  <listen>unix:dir={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
                    root.path().display()
                ),
            )
            .unwrap();
            let mut bus = PrivateBus {
                daemon: Command::new("dbus-daemon")
                    .arg(format!("--config-file={}", config.display()))
                    .args(["--nofork", "--print-address=1"])
                    .stdout(Stdio::piped())
                    .spawn()
                    .expect("Linux appearance regression requires dbus-daemon"),
                _root: root,
            };
            let mut address = String::new();
            BufReader::new(bus.daemon.stdout.take().unwrap())
                .read_line(&mut address)
                .unwrap();
            (bus, address.trim().to_owned())
        }

        struct Portal {
            scheme: u32,
            /// Only the deprecated Read, as portals before version 2 offer.
            old: bool,
        }

        #[zbus::interface(name = "org.freedesktop.portal.Settings")]
        impl Portal {
            fn read_one(&self, namespace: &str, key: &str) -> zbus::fdo::Result<OwnedValue> {
                if self.old {
                    return Err(zbus::fdo::Error::UnknownMethod("ReadOne".into()));
                }
                self.setting(namespace, key, Value::U32(self.scheme))
            }

            fn read(&self, namespace: &str, key: &str) -> zbus::fdo::Result<OwnedValue> {
                let wrapped = Value::Value(Box::new(Value::U32(self.scheme)));
                self.setting(namespace, key, wrapped)
            }
        }

        impl Portal {
            fn setting(
                &self,
                namespace: &str,
                key: &str,
                value: Value,
            ) -> zbus::fdo::Result<OwnedValue> {
                if (namespace, key) != (NAMESPACE, KEY) {
                    return Err(zbus::fdo::Error::Failed(
                        "Requested setting not found".into(),
                    ));
                }
                Ok(OwnedValue::try_from(value).unwrap())
            }
        }

        fn serve(address: &str, portal: Portal) -> Connection {
            Builder::address(address)
                .unwrap()
                .name("org.freedesktop.portal.Desktop")
                .unwrap()
                .serve_at("/org/freedesktop/portal/desktop", portal)
                .unwrap()
                .build()
                .unwrap()
        }

        fn change(server: &Connection, namespace: &str, key: &str, scheme: u32) {
            server
                .emit_signal(
                    None::<&str>,
                    "/org/freedesktop/portal/desktop",
                    "org.freedesktop.portal.Settings",
                    "SettingChanged",
                    &(namespace, key, Value::U32(scheme)),
                )
                .unwrap();
        }

        /// Follows the portal at `address` on a thread, reporting each mode.
        fn follower(address: &str) -> mpsc::Receiver<Option<Appearance>> {
            let client = Builder::address(address).unwrap().build().unwrap();
            let (sender, modes) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = follow(&client, |mode| {
                    let _ = sender.send(mode);
                });
            });
            modes
        }

        fn next(modes: &mpsc::Receiver<Option<Appearance>>) -> Option<Appearance> {
            modes.recv_timeout(Duration::from_secs(5)).unwrap()
        }

        #[test]
        fn the_portal_colour_scheme_is_read_then_followed() {
            let (_bus, address) = private_bus();
            let server = serve(
                &address,
                Portal {
                    scheme: 1,
                    old: false,
                },
            );
            let modes = follower(&address);
            assert_eq!(next(&modes), Some(Appearance::Dark));
            // Another setting of the portal is not the colour scheme.
            change(&server, NAMESPACE, "accent-color", 2);
            change(&server, "org.gnome.desktop.interface", KEY, 2);
            change(&server, NAMESPACE, KEY, 2);
            assert_eq!(next(&modes), Some(Appearance::Light));
            // GNOME's light mode states no preference.
            change(&server, NAMESPACE, KEY, 0);
            assert_eq!(next(&modes), Some(Appearance::Light));
            change(&server, NAMESPACE, KEY, 1);
            assert_eq!(next(&modes), Some(Appearance::Dark));
            // A value the specification does not define is no mode.
            change(&server, NAMESPACE, KEY, 7);
            assert_eq!(next(&modes), None);
        }

        #[test]
        fn an_older_portal_is_read_through_its_deprecated_method() {
            let (_bus, address) = private_bus();
            let _server = serve(
                &address,
                Portal {
                    scheme: 2,
                    old: true,
                },
            );
            assert_eq!(next(&follower(&address)), Some(Appearance::Light));
        }

        #[test]
        fn without_a_portal_the_mode_stays_unknown() {
            let (_bus, address) = private_bus();
            let client = Builder::address(address.as_str()).unwrap().build().unwrap();
            let mut reported = Vec::new();
            assert!(follow(&client, |mode| reported.push(mode)).is_err());
            assert!(reported.is_empty());
        }
    }
}
