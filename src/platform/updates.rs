//! Explicit user-approved in-place installation, relaunch and the manual
//! installer/file-manager handoff.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
use anyhow::Context;
use anyhow::Result;
#[cfg(not(windows))]
use anyhow::ensure;
#[cfg(not(windows))]
use std::time::{Duration, Instant};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// What this installation can replace with a verified download: the app bundle
/// on macOS and the AppImage file on Linux. Package-managed, Windows and source
/// builds have none and keep the manual handoff.
pub fn install_target() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    return app_bundle(&std::env::current_exe().ok()?);
    #[cfg(target_os = "linux")]
    return super::host_env::appimage_path()
        .filter(|path| path.is_file())
        .map(Into::into);
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    None
}

/// The `.app` bundle whose `Contents/MacOS` holds `executable`.
#[cfg(any(target_os = "macos", test))]
fn app_bundle(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.ends_with("MacOS")
        && contents.ends_with("Contents")
        && bundle
            .extension()
            .is_some_and(|extension| extension == "app"))
    .then(|| bundle.into())
}

/// Replace `target` with the verified `download`. The running application keeps
/// the files it has open; the new version starts on the next launch. On failure
/// the installed application is left as it was.
pub fn install(download: &Path, target: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    return install_bundle(download, target);
    #[cfg(target_os = "linux")]
    return install_file(download, target);
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (download, target);
        anyhow::bail!("In-place installation is not supported on this platform")
    }
}

/// Copy beside the target, then rename over it: the old file is replaced in one
/// step and stays readable through the mount of the running AppImage.
#[cfg(target_os = "linux")]
fn install_file(download: &Path, target: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let directory = target.parent().context("AppImage has no directory")?;
    let mut staged = tempfile::Builder::new()
        .prefix(".neptune-update-")
        .tempfile_in(directory)?;
    std::io::copy(&mut std::fs::File::open(download)?, staged.as_file_mut())?;
    // Browser downloads lose the execute bit; this one inherits the user's.
    let mode = std::fs::metadata(target)?.permissions().mode();
    staged
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(mode | 0o100))?;
    staged.as_file().sync_all()?;
    staged.persist(target)?;
    Ok(())
}

/// Copy the application out of the verified disk image into a hidden folder
/// beside the installed bundle, check its signature, then swap the two.
#[cfg(target_os = "macos")]
fn install_bundle(image: &Path, bundle: &Path) -> Result<()> {
    let run = |program: &str, args: &[&std::ffi::OsStr]| -> Result<()> {
        let status = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        ensure!(status.success(), "{program} failed");
        Ok(())
    };
    let parent = bundle.parent().context("App bundle has no directory")?;
    let work = tempfile::Builder::new()
        .prefix(".neptune-update-")
        .tempdir_in(parent)?;
    let mount = work.path().join("image");
    std::fs::create_dir(&mount)?;
    let staged = work.path().join("Neptune.app");
    run(
        "/usr/bin/hdiutil",
        &[
            "attach".as_ref(),
            "-nobrowse".as_ref(),
            "-readonly".as_ref(),
            "-noautoopen".as_ref(),
            "-quiet".as_ref(),
            "-mountpoint".as_ref(),
            mount.as_os_str(),
            image.as_os_str(),
        ],
    )?;
    let copied = run(
        "/usr/bin/ditto",
        &[mount.join("Neptune.app").as_os_str(), staged.as_os_str()],
    );
    let _ = run(
        "/usr/bin/hdiutil",
        &[
            "detach".as_ref(),
            mount.as_os_str(),
            "-force".as_ref(),
            "-quiet".as_ref(),
        ],
    );
    copied?;
    ensure!(
        staged.join("Contents/MacOS/neptune").is_file(),
        "Disk image does not contain Neptune"
    );
    run(
        "/usr/bin/codesign",
        &[
            "--verify".as_ref(),
            "--deep".as_ref(),
            "--strict".as_ref(),
            staged.as_os_str(),
        ],
    )?;
    // Renamed by Neptune itself: macOS lets an application update its own
    // bundle, but not a helper tool acting on it. Dropping `work` removes the
    // previous version and the mount point.
    replace_bundle(&staged, bundle, &work.path().join("Previous.app"))
}

/// Put `staged` where `bundle` is, keeping the old bundle at `previous`. A
/// failure leaves `bundle` in place.
#[cfg(any(target_os = "macos", test))]
fn replace_bundle(staged: &Path, bundle: &Path, previous: &Path) -> Result<()> {
    std::fs::rename(bundle, previous).context("Could not move the installed application")?;
    if let Err(error) = std::fs::rename(staged, bundle) {
        std::fs::rename(previous, bundle).context("Could not restore the application")?;
        return Err(error.into());
    }
    Ok(())
}

/// Start the installed application again as this one exits. `arguments` are
/// the storage options this run was launched with.
pub fn relaunch(target: &Path, arguments: &[OsString]) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        // A new instance: the exiting one would otherwise only be activated.
        let mut command = Command::new("/usr/bin/open");
        command.arg("-n").arg(target);
        if !arguments.is_empty() {
            command.arg("--args").args(arguments);
        }
        command
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = {
        let mut command = Command::new(target);
        command.args(arguments);
        // Outlive the terminal or launcher session the old process belonged to.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

pub fn open_installer(path: &Path) -> Result<()> {
    // A disk image the user opens is a download like any other to Gatekeeper.
    #[cfg(target_os = "macos")]
    ensure!(
        Command::new("/usr/bin/xattr")
            .args(["-w", "com.apple.quarantine", "0083;00000000;Neptune;"])
            .arg(path)
            .status()?
            .success(),
        "Could not apply macOS download quarantine"
    );
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("/usr/bin/open");
        command.arg(path);
        command
    };
    #[cfg(windows)]
    let mut command = Command::new(path);
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(path.parent().unwrap_or(path));
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = command.spawn()?;
    // The Windows installer is a separate interactive application. Do not wait
    // for installation or close terminal sessions on the user's behalf.
    #[cfg(windows)]
    {
        drop(child);
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        let mut child = child;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait()? {
                ensure!(status.success(), "Installer handoff failed");
                return Ok(());
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("Installer handoff timed out");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_executable_inside_an_app_bundle_names_one() {
        assert_eq!(
            app_bundle(Path::new(
                "/Applications/Neptune.app/Contents/MacOS/neptune"
            )),
            Some("/Applications/Neptune.app".into())
        );
        assert_eq!(app_bundle(Path::new("/usr/local/bin/neptune")), None);
        assert_eq!(
            app_bundle(Path::new("/src/target/release/MacOS/neptune")),
            None
        );
    }

    #[test]
    fn a_bundle_is_swapped_whole_and_survives_a_failed_swap() {
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("Neptune.app");
        let staged = root.path().join("staged.app");
        let previous = root.path().join("Previous.app");
        for (directory, version) in [(&bundle, "old"), (&staged, "new")] {
            std::fs::create_dir(directory).unwrap();
            std::fs::write(directory.join("version"), version).unwrap();
        }
        replace_bundle(&staged, &bundle, &previous).unwrap();
        assert_eq!(std::fs::read(bundle.join("version")).unwrap(), b"new");
        assert_eq!(std::fs::read(previous.join("version")).unwrap(), b"old");
        assert!(!staged.exists());

        std::fs::remove_dir_all(&previous).unwrap();
        assert!(replace_bundle(&staged, &bundle, &previous).is_err());
        assert_eq!(std::fs::read(bundle.join("version")).unwrap(), b"new");
        assert!(!previous.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_appimage_is_replaced_in_place_and_stays_executable() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let download = root.path().join("Neptune-2.AppImage");
        std::fs::write(&download, b"new").unwrap();
        let installed = tempfile::tempdir().unwrap();
        let target = installed.path().join("Neptune.AppImage");
        std::fs::write(&target, b"old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o750)).unwrap();
        // The running AppImage stays readable through its open file.
        let mut running = std::fs::File::open(&target).unwrap();

        install_file(&download, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o750
        );
        let mut old = Vec::new();
        std::io::Read::read_to_end(&mut running, &mut old).unwrap();
        assert_eq!(old, b"old");
        assert_eq!(std::fs::read_dir(installed.path()).unwrap().count(), 1);
        assert!(download.exists());

        // An unwritable or missing location leaves nothing behind.
        assert!(install_file(&download, &installed.path().join("gone/Neptune.AppImage")).is_err());
        assert_eq!(std::fs::read_dir(installed.path()).unwrap().count(), 1);
    }
}
