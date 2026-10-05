//! Bounded background update discovery, authenticated installer downloads and
//! in-place installation the user asks for. No shell access, terminal telemetry
//! or implicit installation.

use anyhow::{Context, Result, ensure};
use ed25519_dalek::{Signature, VerifyingKey};
use eframe::egui;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const REPOSITORY: &str = "zevem/neptune";
const API: &str = "https://api.github.com/repos/zevem/neptune/releases";
const DOWNLOADS: &str = "https://github.com/zevem/neptune/releases/download";
const PUBLIC_KEY: &str = include_str!("../../packaging/update-public-key.hex");
const MAX_METADATA: u64 = 128 * 1024;
const MAX_INSTALLER: u64 = 1024 * 1024 * 1024;
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReleaseChannel {
    #[default]
    Stable,
    /// Accept both prereleases and stable versions, always by SemVer precedence.
    Beta,
}

impl ReleaseChannel {
    pub fn accepts(self, version: &Version) -> bool {
        self == Self::Beta || version.pre.is_empty()
    }
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}

fn release_version(release: &GithubRelease) -> Option<Version> {
    if release.draft {
        return None;
    }
    let version = Version::parse(release.tag_name.strip_prefix('v')?).ok()?;
    (release.prerelease != version.pre.is_empty()).then_some(version)
}

fn select_release(
    releases: &[GithubRelease],
    current: &Version,
    channel: ReleaseChannel,
) -> Option<String> {
    releases
        .iter()
        .filter_map(|release| {
            let version = release_version(release)?;
            (channel.accepts(&version) && version.cmp_precedence(current).is_gt())
                .then_some((version, release.tag_name.clone()))
        })
        .max_by(|a, b| a.0.cmp_precedence(&b.0))
        .map(|(_, tag)| tag)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseAsset {
    pub platform: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedRelease {
    schema: u32,
    repository: String,
    pub version: String,
    pub tag: String,
    commit: String,
    pub notes: String,
    assets: Vec<ReleaseAsset>,
}

impl VerifiedRelease {
    pub fn release_url(&self) -> String {
        format!("https://github.com/{REPOSITORY}/releases/tag/{}", self.tag)
    }

    pub fn asset(&self) -> Result<&ReleaseAsset> {
        self.assets
            .iter()
            .find(|asset| asset.platform == platform())
            .context("No installer for this operating system/architecture")
    }
}

fn platform() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "macos-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "macos-x64"
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        "windows-x64"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        #[cfg(target_os = "linux")]
        let appimage = crate::platform::host_env::appimage();
        #[cfg(not(target_os = "linux"))]
        let appimage = false;
        if !appimage
            && std::env::current_exe()
                .ok()
                .is_some_and(|path| path == Path::new("/usr/bin/neptune"))
        {
            "linux-x64-deb"
        } else {
            "linux-x64-appimage"
        }
    } else {
        "unsupported"
    }
}

fn verify_manifest(
    bytes: &[u8],
    signature: &str,
    public_key: &str,
    tag: &str,
    current: &Version,
    channel: ReleaseChannel,
) -> Result<VerifiedRelease> {
    ensure!(
        bytes.len() <= MAX_METADATA as usize,
        "Update metadata is too large"
    );
    let public_key: [u8; 32] = hex::decode(public_key.trim())?
        .try_into()
        .map_err(|_| anyhow::anyhow!("Invalid updater trust key"))?;
    let signature = Signature::from_slice(&hex::decode(signature.trim())?)?;
    VerifyingKey::from_bytes(&public_key)?
        .verify_strict(bytes, &signature)
        .context("Update signature is invalid")?;
    let release: VerifiedRelease = serde_json::from_slice(bytes)?;
    ensure!(
        release.schema == 1
            && release.repository == REPOSITORY
            && release.tag == tag
            && release.tag == format!("v{}", release.version),
        "Update metadata identity mismatch"
    );
    let version = Version::parse(&release.version)?;
    ensure!(
        channel.accepts(&version) && version.cmp_precedence(current).is_gt(),
        "Update would violate channel or downgrade policy"
    );
    ensure!(
        release.commit.len() == 40 && release.commit.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid source identity"
    );
    ensure!(
        release.notes.len() <= 32000 && !release.notes.trim().is_empty(),
        "Invalid release notes"
    );
    let expected = [
        ("macos-arm64", "macos-arm64.dmg"),
        ("macos-x64", "macos-x64.dmg"),
        ("windows-x64", "windows-x64.exe"),
        ("linux-x64-appimage", "linux-x64.AppImage"),
        ("linux-x64-deb", "linux-x64.deb"),
    ];
    ensure!(
        release.assets.len() == expected.len(),
        "Incomplete update manifest"
    );
    for (platform, suffix) in expected {
        let matches: Vec<_> = release
            .assets
            .iter()
            .filter(|asset| asset.platform == platform)
            .collect();
        ensure!(matches.len() == 1, "Missing or duplicate update artifact");
        let asset = matches[0];
        ensure!(
            asset.name == format!("Neptune-{}-{suffix}", release.version)
                && asset.size > 0
                && asset.size <= MAX_INSTALLER
                && asset.sha256.len() == 64
                && asset.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid update artifact"
        );
    }
    release.asset()?;
    Ok(release)
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(5)
        .timeout_global(Some(timeout))
        .user_agent(concat!("Neptune/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn fetch(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>> {
    let mut response = agent
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .call()?;
    Ok(response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()?)
}

fn discover(channel: ReleaseChannel, cancel: &AtomicBool) -> Result<Option<VerifiedRelease>> {
    let agent = agent(Duration::from_secs(20));
    let current = Version::parse(env!("CARGO_PKG_VERSION"))?;
    let releases = if channel == ReleaseChannel::Stable {
        match fetch(&agent, &format!("{API}/latest"), MAX_METADATA) {
            Ok(bytes) => vec![serde_json::from_slice::<GithubRelease>(&bytes)?],
            Err(error)
                if error
                    .downcast_ref::<ureq::Error>()
                    .is_some_and(|error| matches!(error, ureq::Error::StatusCode(404))) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        }
    } else {
        // Bounded pagination supports older stable releases beyond a page of
        // betas. Refuse incomplete discovery rather than guess when over budget.
        let mut releases = Vec::new();
        for page in 1..=10 {
            ensure!(!cancel.load(Ordering::Relaxed), "Update check cancelled");
            let bytes = fetch(
                &agent,
                &format!("{API}?per_page=100&page={page}"),
                4 * 1024 * 1024,
            )?;
            let batch: Vec<GithubRelease> = serde_json::from_slice(&bytes)?;
            let done = batch.len() < 100;
            releases.extend(batch);
            if done {
                break;
            }
            ensure!(page < 10, "Release history exceeds update discovery budget");
        }
        releases
    };
    let Some(tag) = select_release(&releases, &current, channel) else {
        return Ok(None);
    };
    ensure!(!cancel.load(Ordering::Relaxed), "Update check cancelled");
    let bytes = fetch(
        &agent,
        &format!("{DOWNLOADS}/{tag}/update-manifest.json"),
        MAX_METADATA,
    )?;
    let signature = fetch(
        &agent,
        &format!("{DOWNLOADS}/{tag}/update-manifest.sig"),
        256,
    )?;
    verify_manifest(
        &bytes,
        std::str::from_utf8(&signature)?,
        PUBLIC_KEY,
        &tag,
        &current,
        channel,
    )
    .map(Some)
}

pub struct PreparedUpdate {
    directory: Option<tempfile::TempDir>,
    pub path: std::path::PathBuf,
}

fn download(release: &VerifiedRelease, cancel: &AtomicBool) -> Result<PreparedUpdate> {
    let asset = release.asset()?;
    let mut response = agent(Duration::from_secs(5 * 60))
        .get(format!("{DOWNLOADS}/{}/{}", release.tag, asset.name))
        .call()?;
    save_download(response.body_mut().as_reader(), asset, cancel)
}

fn save_download(
    reader: impl Read,
    asset: &ReleaseAsset,
    cancel: &AtomicBool,
) -> Result<PreparedUpdate> {
    let directory = tempfile::Builder::new()
        .prefix("neptune-update-")
        .tempdir()?;
    let path = directory.path().join(&asset.name);
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)?;
    let mut reader = reader.take(asset.size + 1);
    let mut hash = Sha256::new();
    let mut size = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        ensure!(!cancel.load(Ordering::Relaxed), "Download cancelled");
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size += count as u64;
        ensure!(size <= asset.size, "Installer exceeds signed size");
        hash.update(&buffer[..count]);
        file.write_all(&buffer[..count])?;
    }
    ensure!(
        size == asset.size && hex::encode(hash.finalize()).eq_ignore_ascii_case(&asset.sha256),
        "Installer checksum/size mismatch"
    );
    file.sync_all()?;
    Ok(PreparedUpdate {
        directory: Some(directory),
        path,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    Current,
    Available,
    Downloading,
    Ready,
    Opening,
    Opened,
    Installing,
    /// The new version is on disk; this process is still the old one.
    Installed,
    Error(String),
}

/// What the user can do next with the release on offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextStep {
    Download,
    /// Replace this installation with the verified download and restart.
    Install,
    /// Hand the verified download to the installer or file manager.
    Open,
    Restart,
}

fn recheck(prepared: &PreparedUpdate, asset: &ReleaseAsset, cancel: &AtomicBool) -> Result<()> {
    ensure!(
        !cancel.load(Ordering::Relaxed),
        "Installer handoff cancelled"
    );
    let mut file = std::fs::File::open(&prepared.path)?.take(asset.size + 1);
    let mut hash = Sha256::new();
    let size = std::io::copy(&mut file, &mut hash)?;
    ensure!(
        size == asset.size && hex::encode(hash.finalize()).eq_ignore_ascii_case(&asset.sha256),
        "Installer changed after download"
    );
    ensure!(
        !cancel.load(Ordering::Relaxed),
        "Installer handoff cancelled"
    );
    Ok(())
}

/// Install the verified download in place. A download that cannot be installed
/// here is handed back for the manual handoff; one that changed is dropped.
fn install(
    prepared: PreparedUpdate,
    asset: &ReleaseAsset,
    cancel: &AtomicBool,
    install: impl FnOnce(&Path) -> Result<()>,
) -> Result<Completion> {
    recheck(&prepared, asset, cancel)?;
    Ok(match install(&prepared.path) {
        Ok(()) => Completion::Installed,
        Err(_) => Completion::NotInstalled(prepared),
    })
}

fn handoff(
    mut prepared: PreparedUpdate,
    asset: &ReleaseAsset,
    cancel: &AtomicBool,
    open: impl FnOnce(&Path) -> Result<()>,
) -> Result<PreparedUpdate> {
    recheck(&prepared, asset, cancel)?;
    open(&prepared.path)?;
    // The installer/file manager can outlive Neptune. Retain only after a
    // successful explicit handoff; failed or cancelled private files are dropped.
    if let Some(directory) = prepared.directory.take() {
        let _ = directory.keep();
    }
    Ok(prepared)
}

enum Completion {
    Checked(Option<VerifiedRelease>),
    Downloaded(PreparedUpdate),
    Opened(PreparedUpdate),
    Installed,
    NotInstalled(PreparedUpdate),
}
struct Pending {
    channel: ReleaseChannel,
    receiver: mpsc::Receiver<Result<Completion>>,
    cancel: Arc<AtomicBool>,
}

pub struct Updates {
    pub status: UpdateStatus,
    pub release: Option<VerifiedRelease>,
    prepared: Option<PreparedUpdate>,
    /// What an in-place installation replaces; `None` where the download is
    /// handed to the installer or file manager instead.
    target: Option<PathBuf>,
    pending: Option<Pending>,
    next_check: Instant,
    channel: ReleaseChannel,
    dismissed: VecDeque<String>,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            status: UpdateStatus::Idle,
            release: None,
            prepared: None,
            target: crate::platform::updates::install_target(),
            pending: None,
            next_check: Instant::now() + Duration::from_secs(15),
            channel: Default::default(),
            dismissed: VecDeque::new(),
        }
    }
}

impl Updates {
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn notification(&self) -> Option<&VerifiedRelease> {
        self.release
            .as_ref()
            .filter(|release| !self.dismissed.contains(&release.version))
    }
    pub fn dismiss(&mut self) {
        if let Some(release) = &self.release {
            if self.dismissed.len() == 16 {
                self.dismissed.pop_front();
            }
            self.dismissed.push_back(release.version.clone());
        }
    }
    pub fn next_step(&self) -> NextStep {
        if self.status == UpdateStatus::Installed {
            NextStep::Restart
        } else if self.prepared.is_none() && self.status != UpdateStatus::Installing {
            NextStep::Download
        } else if self.target.is_some() {
            NextStep::Install
        } else {
            NextStep::Open
        }
    }
    /// The installation to start again once the new version is on disk.
    pub fn installed(&self) -> Option<&Path> {
        self.target
            .as_deref()
            .filter(|_| self.status == UpdateStatus::Installed)
    }
    /// An installation cannot be interrupted halfway, and afterwards only a
    /// restart is left to do.
    fn settled(&self) -> bool {
        matches!(
            self.status,
            UpdateStatus::Installing | UpdateStatus::Installed
        )
    }
    pub fn cancel(&mut self) {
        if self.settled() {
            return;
        }
        // Keep the slot reserved until the worker exits: channel switching and
        // retry cannot accumulate background network/download workers.
        if let Some(pending) = &self.pending {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        self.status = if self.prepared.is_some() {
            UpdateStatus::Ready
        } else if self.release.is_some() {
            UpdateStatus::Available
        } else {
            UpdateStatus::Idle
        };
    }
    pub fn configure(&mut self, channel: ReleaseChannel) {
        if self.channel != channel && !self.settled() {
            self.cancel();
            self.channel = channel;
            self.release = None;
            self.prepared = None;
            self.status = UpdateStatus::Idle;
            self.next_check = Instant::now() + Duration::from_secs(1);
        }
    }
    fn start(
        &mut self,
        ctx: &egui::Context,
        status: UpdateStatus,
        work: impl FnOnce(Arc<AtomicBool>) -> Result<Completion> + Send + 'static,
    ) {
        if self.busy() {
            return;
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let wake = ctx.clone();
        let result = std::thread::Builder::new()
            .name("neptune-update".into())
            .spawn(move || {
                let result = work(worker_cancel);
                let _ = sender.send(result);
                wake.request_repaint();
            });
        if result.is_err() {
            self.status = UpdateStatus::Error("Could not start update worker".into());
            return;
        }
        self.status = status;
        self.pending = Some(Pending {
            channel: self.channel,
            receiver,
            cancel,
        });
    }
    pub fn check(&mut self, ctx: &egui::Context) {
        if self.settled() {
            return;
        }
        self.next_check = Instant::now() + CHECK_INTERVAL;
        let channel = self.channel;
        self.start(ctx, UpdateStatus::Checking, move |cancel| {
            discover(channel, &cancel).map(Completion::Checked)
        });
    }
    pub fn download(&mut self, ctx: &egui::Context) {
        if let Some(release) = self.release.clone() {
            self.start(ctx, UpdateStatus::Downloading, move |cancel| {
                download(&release, &cancel).map(Completion::Downloaded)
            });
        }
    }
    pub fn open(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        if let Some(prepared) = self.prepared.take() {
            let asset = self
                .release
                .as_ref()
                .and_then(|release| release.asset().ok())
                .cloned();
            self.start(ctx, UpdateStatus::Opening, move |cancel| {
                let asset = asset.context("Missing verified installer identity")?;
                handoff(
                    prepared,
                    &asset,
                    &cancel,
                    crate::platform::updates::open_installer,
                )
                .map(Completion::Opened)
            });
        }
    }
    pub fn install(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let Some(target) = self.target.clone() else {
            return;
        };
        if let Some(prepared) = self.prepared.take() {
            let asset = self
                .release
                .as_ref()
                .and_then(|release| release.asset().ok())
                .cloned();
            self.start(ctx, UpdateStatus::Installing, move |cancel| {
                let asset = asset.context("Missing verified installer identity")?;
                install(prepared, &asset, &cancel, |download| {
                    crate::platform::updates::install(download, &target)
                })
            });
        }
    }
    /// Returns whether an installation has just finished and a restart is due.
    pub fn poll(&mut self, ctx: &egui::Context, automatic: bool) -> bool {
        let mut installed = false;
        let completion =
            self.pending
                .as_ref()
                .and_then(|pending| match pending.receiver.try_recv() {
                    Ok(result) => Some((
                        pending.channel,
                        pending.cancel.load(Ordering::Relaxed),
                        result,
                    )),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => Some((
                        pending.channel,
                        pending.cancel.load(Ordering::Relaxed),
                        Err(anyhow::anyhow!("Update worker stopped unexpectedly")),
                    )),
                });
        if let Some((channel, cancelled, result)) = completion {
            self.pending = None;
            if channel == self.channel && !cancelled {
                match result {
                    Ok(Completion::Checked(release)) => {
                        // Keep the verified installer tied to its original release.
                        if self.release.as_ref().map(|r| &r.version)
                            != release.as_ref().map(|r| &r.version)
                        {
                            self.prepared = None;
                        }
                        self.status = if release.is_none() {
                            UpdateStatus::Current
                        } else if self.prepared.is_some() {
                            UpdateStatus::Ready
                        } else {
                            UpdateStatus::Available
                        };
                        self.release = release;
                    }
                    Ok(Completion::Downloaded(prepared)) => {
                        self.prepared = Some(prepared);
                        self.status = UpdateStatus::Ready;
                    }
                    Ok(Completion::Opened(prepared)) => {
                        self.prepared = Some(prepared);
                        self.dismiss();
                        self.status = UpdateStatus::Opened;
                    }
                    Ok(Completion::Installed) => {
                        self.dismiss();
                        self.status = UpdateStatus::Installed;
                        installed = true;
                    }
                    Ok(Completion::NotInstalled(prepared)) => {
                        // Read-only or unowned locations keep the manual path.
                        self.prepared = Some(prepared);
                        self.target = None;
                        self.status = UpdateStatus::Error("Neptune could not replace itself in this location. Open the verified download to install it yourself.".into());
                    }
                    Err(_) => self.status = UpdateStatus::Error("Could not complete the update. Check your connection and try again. Unverified downloads are never opened.".into()),
                }
            }
        }
        if automatic && !self.busy() && !self.settled() {
            if Instant::now() >= self.next_check {
                self.check(ctx);
            }
            ctx.request_repaint_after(self.next_check.saturating_duration_since(Instant::now()));
        }
        installed
    }
}

impl Drop for Updates {
    fn drop(&mut self) {
        if let Some(pending) = &self.pending {
            pending.cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn releases(tags: &[(&str, bool, bool)]) -> Vec<GithubRelease> {
        tags.iter()
            .map(|(tag, prerelease, draft)| GithubRelease {
                tag_name: (*tag).into(),
                prerelease: *prerelease,
                draft: *draft,
            })
            .collect()
    }

    pub(crate) fn signed(version: &str) -> (Vec<u8>, String, String) {
        // Deterministic test-only key, unrelated to the production trust anchor.
        let key = SigningKey::from_bytes(&[7; 32]);
        let assets: Vec<_> = [("macos-arm64", "macos-arm64.dmg"), ("macos-x64", "macos-x64.dmg"), ("windows-x64", "windows-x64.exe"), ("linux-x64-appimage", "linux-x64.AppImage"), ("linux-x64-deb", "linux-x64.deb")].into_iter().map(|(platform, suffix)| serde_json::json!({ "platform": platform, "name": format!("Neptune-{version}-{suffix}"), "size": 12, "sha256": "11".repeat(32) })).collect();
        let bytes = serde_json::to_vec(&serde_json::json!({ "schema": 1, "repository": REPOSITORY, "version": version, "tag": format!("v{version}"), "commit": "a".repeat(40), "notes": "### What's New\n- Better terminal updates.", "assets": assets })).unwrap();
        let signature = hex::encode(key.sign(&bytes).to_bytes());
        (
            bytes,
            signature,
            hex::encode(key.verifying_key().to_bytes()),
        )
    }

    pub(crate) fn visual_release() -> VerifiedRelease {
        let (bytes, signature, key) = signed("0.2.0");
        verify_manifest(
            &bytes,
            &signature,
            &key,
            "v0.2.0",
            &Version::parse("0.1.0").unwrap(),
            ReleaseChannel::Stable,
        )
        .unwrap()
    }

    #[test]
    fn stable_never_offers_prereleases_drafts_or_version_metadata_changes() {
        let list = releases(&[
            ("v0.4.0-beta.1", true, false),
            ("v0.3.0", false, true),
            ("v0.2.1+build.2", false, false),
            ("v0.2.2", false, false),
        ]);
        assert_eq!(
            select_release(
                &list,
                &Version::parse("0.2.1").unwrap(),
                ReleaseChannel::Stable
            ),
            Some("v0.2.2".into())
        );
        assert_eq!(
            select_release(
                &list[..3],
                &Version::parse("0.2.1").unwrap(),
                ReleaseChannel::Stable
            ),
            None
        );
    }

    #[test]
    fn beta_moves_to_stable_then_to_newer_betas_without_downgrades() {
        let list = releases(&[
            ("v0.2.0-beta.2", true, false),
            ("v0.2.0-rc.1", true, false),
            ("v0.2.0", false, false),
        ]);
        assert_eq!(
            select_release(
                &list,
                &Version::parse("0.2.0-beta.10").unwrap(),
                ReleaseChannel::Beta
            ),
            Some("v0.2.0".into())
        );
        let list = releases(&[
            ("v0.3.0-beta.2", true, false),
            ("v0.3.0-beta.10", true, false),
        ]);
        assert_eq!(
            select_release(
                &list,
                &Version::parse("0.2.0").unwrap(),
                ReleaseChannel::Beta
            ),
            Some("v0.3.0-beta.10".into())
        );
        assert_eq!(
            select_release(
                &list,
                &Version::parse("0.3.0").unwrap(),
                ReleaseChannel::Beta
            ),
            None
        );
    }

    #[test]
    fn malformed_and_misclassified_github_tags_are_rejected() {
        let list = releases(&[
            ("v01.2.3", false, false),
            ("v0.2.0-beta.01", true, false),
            ("v0.2.0-beta.1", false, false),
            ("v0.2.0", true, false),
            ("0.2.0", false, false),
        ]);
        assert_eq!(
            select_release(
                &list,
                &Version::parse("0.1.0").unwrap(),
                ReleaseChannel::Beta
            ),
            None
        );
    }

    #[test]
    fn metadata_requires_valid_signature_identity_channel_and_complete_artifacts() {
        let current = Version::parse("0.1.0").unwrap();
        let (bytes, signature, key) = signed("0.2.0");
        let release = verify_manifest(
            &bytes,
            &signature,
            &key,
            "v0.2.0",
            &current,
            ReleaseChannel::Stable,
        )
        .unwrap();
        assert_eq!(release.version, "0.2.0");
        let mut tampered = bytes.clone();
        tampered[10] ^= 1;
        assert!(
            verify_manifest(
                &tampered,
                &signature,
                &key,
                "v0.2.0",
                &current,
                ReleaseChannel::Stable
            )
            .is_err()
        );
        assert!(
            verify_manifest(
                &bytes,
                &signature,
                &"ff".repeat(32),
                "v0.2.0",
                &current,
                ReleaseChannel::Stable
            )
            .is_err()
        );
        assert!(
            verify_manifest(
                &bytes,
                &signature,
                &key,
                "v0.2.1",
                &current,
                ReleaseChannel::Stable
            )
            .is_err()
        );
        assert!(
            verify_manifest(
                &bytes,
                &signature,
                &key,
                "v0.2.0",
                &Version::parse("0.3.0").unwrap(),
                ReleaseChannel::Stable
            )
            .is_err()
        );
        let (bytes, signature, key) = signed("0.4.0-beta.1");
        assert!(
            verify_manifest(
                &bytes,
                &signature,
                &key,
                "v0.4.0-beta.1",
                &current,
                ReleaseChannel::Stable
            )
            .is_err()
        );
        assert!(
            verify_manifest(
                &bytes,
                &signature,
                &key,
                "v0.4.0-beta.1",
                &current,
                ReleaseChannel::Beta
            )
            .is_ok()
        );
        let mut incomplete: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        incomplete["assets"].as_array_mut().unwrap().pop();
        let bytes = serde_json::to_vec(&incomplete).unwrap();
        let signing = SigningKey::from_bytes(&[7; 32]);
        let signature = hex::encode(signing.sign(&bytes).to_bytes());
        assert!(
            verify_manifest(
                &bytes,
                &signature,
                &key,
                "v0.4.0-beta.1",
                &current,
                ReleaseChannel::Beta
            )
            .is_err()
        );
    }

    #[test]
    fn dismissed_updates_stay_quiet_and_manual_review_remains_available() {
        let (bytes, signature, key) = signed("0.2.0");
        let release = verify_manifest(
            &bytes,
            &signature,
            &key,
            "v0.2.0",
            &Version::parse("0.1.0").unwrap(),
            ReleaseChannel::Stable,
        )
        .unwrap();
        let mut updates = Updates::default();
        updates.release = Some(release);
        assert!(updates.notification().is_some());
        updates.dismiss();
        assert!(updates.notification().is_none());
        assert!(updates.release.is_some());
    }

    #[test]
    fn cancelled_checks_reserve_worker_slot_and_stale_completions_cannot_change_channel() {
        let ctx = egui::Context::default();
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut updates = Updates::default();
        updates.pending = Some(Pending {
            channel: ReleaseChannel::Stable,
            receiver,
            cancel: cancel.clone(),
        });
        updates.configure(ReleaseChannel::Beta);
        assert!(updates.busy());
        assert!(cancel.load(Ordering::Relaxed));
        sender.send(Ok(Completion::Checked(None))).unwrap();
        updates.poll(&ctx, false);
        assert!(!updates.busy());
        assert_eq!(updates.status, UpdateStatus::Idle);
        assert_eq!(updates.channel, ReleaseChannel::Beta);
    }

    #[test]
    fn worker_failure_is_recoverable_without_repaint_polling() {
        let ctx = egui::Context::default();
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut updates = Updates::default();
        updates.pending = Some(Pending {
            channel: ReleaseChannel::Stable,
            receiver,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        drop(sender);
        updates.poll(&ctx, false);
        assert!(matches!(updates.status, UpdateStatus::Error(_)));
        assert!(!updates.busy());
    }

    #[test]
    fn installer_bytes_require_exact_signed_hash_size_and_uncancelled_download() {
        let bytes = b"verified installer";
        let asset = ReleaseAsset {
            platform: platform().into(),
            name: "Neptune-test.AppImage".into(),
            size: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(bytes)),
        };
        let cancel = AtomicBool::new(false);
        let prepared = save_download(&bytes[..], &asset, &cancel).unwrap();
        let path = prepared.path.clone();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        drop(prepared);
        assert!(!path.exists());
        assert!(save_download(&bytes[..bytes.len() - 1], &asset, &cancel).is_err());
        assert!(save_download(&b"verified installer extra"[..], &asset, &cancel).is_err());
        assert!(save_download(&b"tampered installer"[..], &asset, &cancel).is_err());
        cancel.store(true, Ordering::Relaxed);
        assert!(save_download(&bytes[..], &asset, &cancel).is_err());
    }

    /// A verified download as the sheet sees it, installable in place or not.
    pub(crate) fn prepare(updates: &mut Updates, in_place: bool) {
        updates.target = in_place.then(|| "/opt/Neptune.AppImage".into());
        updates.prepared = Some(PreparedUpdate {
            directory: None,
            path: PathBuf::new(),
        });
    }

    /// The new version on disk, with this process still the old one.
    pub(crate) fn installed(updates: &mut Updates) {
        updates.release = Some(visual_release());
        updates.target = Some("/opt/Neptune.AppImage".into());
        updates.status = UpdateStatus::Installed;
    }

    fn ready(target: Option<&str>) -> (Updates, mpsc::SyncSender<Result<Completion>>) {
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut updates = Updates::default();
        updates.release = Some(visual_release());
        updates.target = target.map(Into::into);
        updates.status = UpdateStatus::Installing;
        updates.pending = Some(Pending {
            channel: ReleaseChannel::Stable,
            receiver,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        (updates, sender)
    }

    #[test]
    fn an_installation_cannot_be_cancelled_and_leaves_only_a_restart() {
        let ctx = egui::Context::default();
        let (mut updates, sender) = ready(Some("/opt/Neptune.AppImage"));
        assert_eq!(updates.next_step(), NextStep::Install);
        assert_eq!(updates.installed(), None);
        // Neither Cancel nor a channel change may lose track of a replacement
        // that is already under way.
        updates.cancel();
        updates.configure(ReleaseChannel::Beta);
        assert_eq!(updates.status, UpdateStatus::Installing);
        sender.send(Ok(Completion::Installed)).unwrap();
        assert!(updates.poll(&ctx, true));
        assert_eq!(updates.status, UpdateStatus::Installed);
        assert_eq!(updates.next_step(), NextStep::Restart);
        assert_eq!(
            updates.installed(),
            Some(Path::new("/opt/Neptune.AppImage"))
        );
        assert!(updates.notification().is_none());
        // The old process stops offering the release it has just installed.
        updates.check(&ctx);
        updates.configure(ReleaseChannel::Beta);
        assert!(!updates.poll(&ctx, true));
        assert!(!updates.busy());
        assert_eq!(updates.status, UpdateStatus::Installed);
    }

    #[test]
    fn a_location_neptune_cannot_replace_falls_back_to_the_manual_handoff() {
        let ctx = egui::Context::default();
        let bytes = b"verified installer";
        let asset = ReleaseAsset {
            platform: platform().into(),
            name: "Neptune-test.AppImage".into(),
            size: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(bytes)),
        };
        let cancel = AtomicBool::new(false);
        let prepared = save_download(&bytes[..], &asset, &cancel).unwrap();
        let path = prepared.path.clone();
        let completion = install(prepared, &asset, &cancel, |_| {
            anyhow::bail!("Read-only location")
        })
        .unwrap();
        let (mut updates, sender) = ready(Some("/Volumes/Neptune/Neptune.app"));
        sender.send(Ok(completion)).unwrap();
        assert!(!updates.poll(&ctx, false));
        assert!(matches!(updates.status, UpdateStatus::Error(_)));
        assert_eq!(updates.next_step(), NextStep::Open);
        assert_eq!(updates.installed(), None);
        assert!(path.exists());
        drop(updates);
        assert!(!path.exists());

        // A download that changed is never installed, and is not kept.
        let prepared = save_download(&bytes[..], &asset, &cancel).unwrap();
        let path = prepared.path.clone();
        std::fs::write(&path, b"tampered installer").unwrap();
        assert!(
            install(prepared, &asset, &cancel, |_| panic!(
                "Tampered installer installed"
            ))
            .is_err()
        );
        assert!(!path.exists());

        let prepared = save_download(&bytes[..], &asset, &cancel).unwrap();
        let path = prepared.path.clone();
        assert!(matches!(
            install(prepared, &asset, &cancel, |download| {
                assert_eq!(std::fs::read(download).unwrap(), bytes);
                Ok(())
            }),
            Ok(Completion::Installed)
        ));
        assert!(!path.exists());
    }

    #[test]
    fn handoff_rechecks_bytes_respects_cancel_and_retains_only_successful_downloads() {
        let bytes = b"verified installer";
        let asset = ReleaseAsset {
            platform: platform().into(),
            name: "Neptune-test.AppImage".into(),
            size: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(bytes)),
        };
        let cancel = AtomicBool::new(false);
        let prepared = save_download(&bytes[..], &asset, &cancel).unwrap();
        let path = prepared.path.clone();
        cancel.store(true, Ordering::Relaxed);
        assert!(
            handoff(prepared, &asset, &cancel, |_| panic!(
                "Cancelled installer opened"
            ))
            .is_err()
        );
        assert!(!path.exists());

        cancel.store(false, Ordering::Relaxed);
        let prepared = save_download(&bytes[..], &asset, &cancel).unwrap();
        let path = prepared.path.clone();
        std::fs::write(&path, b"tampered installer").unwrap();
        assert!(
            handoff(prepared, &asset, &cancel, |_| panic!(
                "Tampered installer opened"
            ))
            .is_err()
        );
        assert!(!path.exists());

        let prepared = save_download(&bytes[..], &asset, &cancel).unwrap();
        let path = prepared.path.clone();
        assert!(
            handoff(prepared, &asset, &cancel, |_| anyhow::bail!(
                "Handoff failed"
            ))
            .is_err()
        );
        assert!(!path.exists());

        let prepared = save_download(&bytes[..], &asset, &cancel).unwrap();
        let prepared = handoff(prepared, &asset, &cancel, |path| {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
            Ok(())
        })
        .unwrap();
        assert!(prepared.path.exists());
        assert!(prepared.directory.is_none());
        std::fs::remove_dir_all(prepared.path.parent().unwrap()).unwrap();
    }
}
