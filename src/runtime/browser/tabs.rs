//! The page and profile of each browser tab that keeps its sign-ins, so a
//! restored tab opens where it was. A private tab is never written here.
use super::protocol;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Tabs remembered at most; the oldest panes give way.
const KEPT: usize = 64;
/// How long changes gather before they are written.
const SETTLE: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Tab {
    pub url: String,
    pub profile: String,
}
impl Tab {
    fn valid(&self) -> bool {
        self.url.len() <= protocol::MAX_URL
            && (self.url.starts_with("http://") || self.url.starts_with("https://"))
            && !self
                .url
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
            && protocol::valid_profile(&self.profile)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    version: u32,
    tabs: BTreeMap<u64, Tab>,
}

#[derive(Default)]
pub(super) struct Tabs {
    path: Option<PathBuf>,
    tabs: BTreeMap<u64, Tab>,
    changed: Option<Instant>,
}
impl Tabs {
    /// Read what the last run left. A file that cannot be read restores no
    /// page, and the next change replaces it.
    pub(super) fn open(root: &Path) -> Self {
        let path = root.join("tabs.json");
        let tabs = std::fs::read(&path)
            .ok()
            .filter(|bytes| bytes.len() <= 1024 * 1024)
            .and_then(|bytes| serde_json::from_slice::<Saved>(&bytes).ok())
            .filter(|saved| saved.version == 1)
            .map(|saved| saved.tabs)
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, tab)| tab.valid())
            .collect();
        Self {
            path: Some(path),
            tabs,
            changed: None,
        }
    }
    pub(super) fn get(&self, pane: u64) -> Option<&Tab> {
        self.tabs.get(&pane)
    }
    /// A tab is at this page in this profile; none of either forgets it.
    pub(super) fn note(&mut self, pane: u64, tab: Option<Tab>) {
        let tab = tab.filter(Tab::valid);
        if self.tabs.get(&pane) == tab.as_ref() {
            return;
        }
        match tab {
            Some(tab) => {
                self.tabs.insert(pane, tab);
                while self.tabs.len() > KEPT {
                    self.tabs.pop_first();
                }
            }
            None => {
                self.tabs.remove(&pane);
            }
        }
        self.changed.get_or_insert_with(Instant::now);
    }
    /// Write settled changes on a worker, and say when to look again.
    pub(super) fn settle(&mut self) -> Option<Duration> {
        let waited = self.changed?.elapsed();
        if waited < SETTLE {
            return Some(SETTLE - waited);
        }
        self.changed = None;
        if let Some((path, bytes)) = self.encoded() {
            let _ = std::thread::Builder::new()
                .name("neptune-browser-tabs".into())
                .spawn(move || write(&path, &bytes));
        }
        None
    }
    /// Write what is pending before the window goes.
    pub(super) fn flush(&mut self) {
        if self.changed.take().is_some()
            && let Some((path, bytes)) = self.encoded()
        {
            write(&path, &bytes);
        }
    }
    fn encoded(&self) -> Option<(PathBuf, Vec<u8>)> {
        let saved = Saved {
            version: 1,
            tabs: self.tabs.clone(),
        };
        Some((self.path.clone()?, serde_json::to_vec(&saved).ok()?))
    }
}

/// Replace the file whole, so a reader never sees half of it.
fn write(path: &Path, bytes: &[u8]) {
    let partial = path.with_extension("json.partial");
    if std::fs::write(&partial, bytes).is_ok() {
        let _ = std::fs::rename(&partial, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(url: &str) -> Tab {
        Tab {
            url: url.into(),
            profile: "default".into(),
        }
    }

    #[test]
    fn tabs_come_back_where_they_were_and_only_as_pages() {
        let root = tempfile::tempdir().unwrap();
        let mut tabs = Tabs::open(root.path());
        tabs.note(3, Some(tab("https://example.com/a")));
        tabs.note(4, Some(tab("http://localhost:3000/")));
        // Not a page to come back to, and a closed or private tab.
        tabs.note(5, Some(tab("about:blank")));
        tabs.note(4, None);
        tabs.flush();
        let tabs = Tabs::open(root.path());
        assert_eq!(tabs.get(3), Some(&tab("https://example.com/a")));
        assert_eq!(tabs.get(4), None);
        assert_eq!(tabs.get(5), None);
    }

    #[test]
    fn a_damaged_or_foreign_file_restores_nothing() {
        let root = tempfile::tempdir().unwrap();
        for bytes in [
            &b"not json"[..],
            br#"{"version":2,"tabs":{"3":{"url":"https://a/","profile":"default"}}}"#,
            br#"{"version":1,"tabs":{"3":{"url":"file:///etc/passwd","profile":"default"},"4":{"url":"https://a/","profile":"../x"}}}"#,
        ] {
            std::fs::write(root.path().join("tabs.json"), bytes).unwrap();
            let tabs = Tabs::open(root.path());
            assert!(tabs.get(3).is_none() && tabs.get(4).is_none());
        }
    }

    #[test]
    fn only_so_many_tabs_are_remembered_and_unchanged_ones_are_not_rewritten() {
        let root = tempfile::tempdir().unwrap();
        let mut tabs = Tabs::open(root.path());
        for pane in 1..=KEPT as u64 + 2 {
            tabs.note(pane, Some(tab("https://example.com/")));
        }
        assert!(tabs.get(1).is_none() && tabs.get(2).is_none());
        assert!(tabs.get(KEPT as u64 + 2).is_some());
        tabs.flush();
        tabs.note(9, Some(tab("https://example.com/")));
        assert!(tabs.settle().is_none());
        assert!(!root.path().join("tabs.json.partial").exists());
    }
}
