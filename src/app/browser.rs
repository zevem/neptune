//! What a browser tab does besides showing a page: its profile, cookies from
//! another browser, a picked element and a recording. Other browsers' files
//! are read on workers; cookie names and values stay out of diagnostics.
use super::*;
use crate::{
    config::BrowserProfile,
    platform::browser_import::{self, Failure, Read, SameSite, Source},
    runtime::browser::{Outcome, protocol},
    ui::browser::Tool,
};

#[derive(Default)]
pub(super) struct BrowserTools {
    /// The browsers found on this machine, once looked for.
    sources: Option<Arc<Vec<Source>>>,
    looking: Option<mpsc::Receiver<Vec<Source>>>,
    /// The profile an import is for, while its cookies are read.
    reading: Option<(String, mpsc::Receiver<Result<Read, Failure>>)>,
    /// Cookies the read left out, to count with those Chromium refuses.
    left_out: usize,
    /// The profile the next browser pane starts with, when it was opened
    /// from a browser: it stays in that one's profile.
    pub(super) inherit: Option<Option<String>>,
}

impl BrowserTools {
    pub(super) fn sources(&self) -> Option<Arc<Vec<Source>>> {
        self.sources.clone()
    }
    pub(super) fn busy(&self) -> bool {
        self.reading.is_some()
    }
}

/// A name no profile has yet: "Profile 2", "Chrome 3".
fn unused_name(config: &Config, base: &str) -> String {
    let taken = |name: &str| {
        name.eq_ignore_ascii_case("Default")
            || name.eq_ignore_ascii_case("Private")
            || config
                .browser_profiles
                .iter()
                .any(|profile| profile.name.eq_ignore_ascii_case(name))
    };
    (1..)
        .map(|n| {
            if n == 1 {
                base.to_owned()
            } else {
                format!("{base} {n}")
            }
        })
        .find(|name| !taken(name))
        .unwrap_or_else(|| base.to_owned())
}

/// A directory name for a new profile, unlike those in use or once removed.
fn new_id(config: &Config) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    (0..)
        .map(|n| format!("profile-{:x}", now.wrapping_add(n)))
        .find(|id| config.browser_profiles.iter().all(|p| &p.id != id))
        .unwrap_or_default()
}

fn wire(cookie: browser_import::Cookie) -> protocol::Cookie {
    protocol::Cookie {
        host: cookie.host,
        name: cookie.name,
        value: cookie.value,
        path: cookie.path,
        secure: cookie.secure,
        http_only: cookie.http_only,
        same_site: match cookie.same_site {
            // Chromium refuses a cookie for every site that is not also
            // secure. Browsers that keep one meant no restriction at all.
            SameSite::None if !cookie.secure => -1,
            SameSite::None => 0,
            SameSite::Lax => 1,
            SameSite::Strict => 2,
            SameSite::Unspecified => -1,
        },
        expires: cookie.expires,
    }
}

impl App {
    fn restart_in(&mut self, ctx: &egui::Context, pane: PaneId, profile: Option<String>) {
        self.browsers.switch(pane, profile);
        self.action(ctx, Action::Restart(pane));
    }

    pub(super) fn browser_tool(
        &mut self,
        ctx: &egui::Context,
        pane: PaneId,
        generation: u64,
        tool: Tool,
    ) {
        if !self.controller.model().pane(pane).is_some_and(|p| {
            p.generation() == generation && p.kind() == neptune_model::PaneKind::Browser
        }) {
            return;
        }
        let result = match tool {
            Tool::Pick => self.browsers.pick(pane, generation),
            Tool::Record => self.browsers.record(pane, generation),
            Tool::Profile(profile) => {
                if self.browsers.profile(pane) != profile.as_deref() {
                    self.restart_in(ctx, pane, profile);
                }
                Ok(())
            }
            Tool::DefaultProfile(profile) => {
                let mut config = self.config.clone();
                config.browser_profile = profile.unwrap_or_else(|| BrowserProfile::PRIVATE.into());
                self.action(ctx, Action::Preferences(config));
                Ok(())
            }
            Tool::NewProfile => {
                if self.config.browser_profiles.len() >= BrowserProfile::MAX {
                    Err("Neptune keeps at most 24 browser profiles. Remove one first.")
                } else {
                    let mut config = self.config.clone();
                    let id = new_id(&config);
                    config.browser_profiles.push(BrowserProfile {
                        name: unused_name(&config, "Profile"),
                        id: id.clone(),
                    });
                    self.action(ctx, Action::Preferences(config));
                    if self.config.browser_profiles.iter().any(|p| p.id == id) {
                        self.restart_in(ctx, pane, Some(id));
                    }
                    Ok(())
                }
            }
            Tool::RemoveProfile => {
                let Some(id) = self.browsers.profile(pane).map(str::to_owned) else {
                    return;
                };
                let mut config = self.config.clone();
                config.browser_profiles.retain(|profile| profile.id != id);
                if config.browser_profiles.len() == self.config.browser_profiles.len() {
                    return;
                }
                self.action(ctx, Action::Preferences(config));
                if self.config.browser_profiles.iter().all(|p| p.id != id) {
                    self.browsers.remove_profile(&id);
                    // Every tab that was in it goes on in the default one.
                    let panes: Vec<PaneId> = self
                        .controller
                        .model()
                        .workspaces()
                        .iter()
                        .flat_map(|workspace| workspace.panes())
                        .map(|pane| pane.id())
                        .filter(|pane| self.browsers.profile(*pane) == Some(id.as_str()))
                        .collect();
                    for pane in panes {
                        self.restart_in(ctx, pane, Some(BrowserProfile::DEFAULT.into()));
                    }
                }
                Ok(())
            }
            Tool::ClearCookies => match self.browsers.profile(pane).map(str::to_owned) {
                Some(profile) => self.browsers.clear_cookies(&profile).map(|()| {
                    self.ui.say(
                        ctx,
                        format!(
                            "Cleared the cookies of {}.",
                            self.config.browser_profile_name(Some(&profile))
                        ),
                    );
                }),
                None => Ok(()),
            },
            Tool::Sources => {
                if self.browser_tools.looking.is_none() {
                    let (sender, receiver) = mpsc::channel();
                    let wake = ctx.clone();
                    let started = std::thread::Builder::new()
                        .name("neptune-browser-sources".into())
                        .spawn(move || {
                            let _ = sender.send(browser_import::sources());
                            wake.request_repaint();
                        });
                    if started.is_ok() {
                        self.browser_tools.looking = Some(receiver);
                    }
                }
                Ok(())
            }
            Tool::Import { source, directory } => {
                let Some(profile) = self.browsers.profile(pane).map(str::to_owned) else {
                    return;
                };
                if self.browser_tools.busy() || self.browsers.importing() {
                    Err("An import is already under way.")
                } else {
                    let (sender, receiver) = mpsc::channel();
                    let wake = ctx.clone();
                    let started = std::thread::Builder::new()
                        .name("neptune-browser-import".into())
                        .spawn(move || {
                            let _ = sender.send(browser_import::read(&source, &directory));
                            wake.request_repaint();
                        });
                    if started.is_ok() {
                        self.browser_tools.reading = Some((profile, receiver));
                    }
                    Ok(())
                }
            }
        };
        if let Err(error) = result {
            self.ui.error = Some(error.into());
        }
    }

    /// Hand over what browsers and their workers finished since last frame.
    pub(super) fn poll_browser_tools(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.browser_tools.looking {
            match receiver.try_recv() {
                Ok(sources) => {
                    self.browser_tools.sources = Some(Arc::new(sources));
                    self.browser_tools.looking = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.browser_tools.sources = Some(Default::default());
                    self.browser_tools.looking = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some((profile, receiver)) = &self.browser_tools.reading {
            match receiver.try_recv() {
                Ok(Ok(read)) => {
                    self.browser_tools.left_out = read.skipped;
                    self.browsers
                        .import(profile, read.cookies.into_iter().map(wire).collect());
                    self.browser_tools.reading = None;
                    ctx.request_repaint();
                }
                Ok(Err(failure)) => {
                    self.ui.error = Some(failure.message().into());
                    self.browser_tools.reading = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.ui.error = Some(Failure::ReadFailed.message().into());
                    self.browser_tools.reading = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        for outcome in self.browsers.outcomes() {
            let said = match outcome {
                Outcome::Picked(said) => {
                    crate::platform::clipboard::copy(ctx, said);
                    "Element copied. Paste it where you describe the change.".into()
                }
                Outcome::Recorded(Ok(path)) => {
                    let said =
                        format!("Recording saved to {}. Its path is copied.", path.display());
                    crate::platform::clipboard::copy(ctx, path.to_string_lossy().into_owned());
                    said
                }
                Outcome::Recorded(Err(error)) => {
                    self.ui.error = Some(error);
                    continue;
                }
                Outcome::Imported { imported, skipped } => {
                    let skipped =
                        skipped as usize + std::mem::take(&mut self.browser_tools.left_out);
                    let cookies = |count: usize| {
                        format!("{count} cookie{}", if count == 1 { "" } else { "s" })
                    };
                    match (imported as usize, skipped) {
                        (0, 0) => "That browser profile has no cookies to import.".into(),
                        (0, skipped) => format!(
                            "No cookie was imported: {} could not be read or kept.",
                            cookies(skipped)
                        ),
                        (imported, 0) => {
                            format!("Imported {}. Reload a page to use them.", cookies(imported))
                        }
                        (imported, skipped) => format!(
                            "Imported {} and left out {} that could not be read or kept. Reload a page to use them.",
                            cookies(imported),
                            cookies(skipped)
                        ),
                    }
                }
            };
            self.ui.say(ctx, said);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_browser_profile_gets_a_name_and_directory_of_its_own() {
        let mut config = Config::default();
        assert_eq!(unused_name(&config, "Profile"), "Profile");
        assert_eq!(unused_name(&config, "Default"), "Default 2");
        for name in ["Profile", "profile 2"] {
            let id = new_id(&config);
            assert!(protocol::valid_profile(&id));
            config.browser_profiles.push(BrowserProfile {
                id,
                name: name.into(),
            });
        }
        assert_eq!(unused_name(&config, "Profile"), "Profile 3");
        assert_ne!(config.browser_profiles[0].id, config.browser_profiles[1].id);
        config.validate().unwrap();
    }
}
