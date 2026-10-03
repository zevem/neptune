//! Bounded, asynchronous handoff of files to the desktop: showing one in the
//! native file manager, or opening it with its default application. Paths
//! never enter diagnostics or saved state.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use eframe::egui;

/// What the file manager is called in menus on this platform.
pub const REVEAL_LABEL: &str = if cfg!(target_os = "macos") {
    "Reveal in Finder"
} else if cfg!(windows) {
    "Reveal in File Explorer"
} else {
    "Reveal in file manager"
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Handoff {
    /// Show the file manager with this file or folder selected.
    Reveal(PathBuf),
    /// Open with the default application.
    Open(PathBuf),
}

#[derive(Default)]
pub struct FileOpener {
    pending: Option<mpsc::Receiver<Result<(), &'static str>>>,
}

impl FileOpener {
    /// Only one launcher can be in flight.
    pub fn start(&mut self, handoff: Handoff, ctx: egui::Context) -> Result<(), &'static str> {
        if self.pending.is_some() {
            return Err("A file is already opening. Try again in a moment.");
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("neptune-open-file".into())
            .spawn(move || {
                let _ = sender.send(launch(&handoff));
                ctx.request_repaint();
            })
            .map_err(|_| "Could not start the file launcher.")?;
        self.pending = Some(receiver);
        Ok(())
    }

    pub fn poll(&mut self) -> Option<Result<(), &'static str>> {
        let result = match self.pending.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err("The file launcher stopped unexpectedly."),
        };
        self.pending = None;
        Some(result)
    }
}

fn quiet(mut command: Command) -> Command {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    command
}

/// A `file://` URI with every byte outside the unreserved set escaped, so it
/// carries no quotes, commas or spaces into a launcher's argument.
#[cfg(not(any(target_os = "macos", windows)))]
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(byte) {
            uri.push(*byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

/// The commands to try in order; the first that succeeds ends the handoff.
fn commands(handoff: &Handoff) -> Vec<Command> {
    let open = |path: &Path| {
        #[cfg(target_os = "macos")]
        let mut command = Command::new("/usr/bin/open");
        #[cfg(windows)]
        let mut command = Command::new("explorer.exe");
        #[cfg(not(any(target_os = "macos", windows)))]
        let mut command = Command::new("xdg-open");
        command.arg(path);
        quiet(command)
    };
    match handoff {
        Handoff::Open(path) => vec![open(path)],
        Handoff::Reveal(path) => {
            #[cfg(target_os = "macos")]
            {
                let mut command = Command::new("/usr/bin/open");
                command.arg("-R").arg(path);
                vec![quiet(command)]
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                // Explorer reads its own command line: only the path may be
                // quoted, which ordinary argument quoting would not do.
                let mut select = std::ffi::OsString::from("/select,\"");
                select.push(path);
                select.push("\"");
                let mut command = Command::new("explorer.exe");
                command.raw_arg(select);
                vec![quiet(command)]
            }
            #[cfg(not(any(target_os = "macos", windows)))]
            {
                // File managers that select an item answer this interface; the
                // rest can still open the folder that holds it.
                let mut show = Command::new("dbus-send");
                show.args([
                    "--session",
                    "--print-reply",
                    "--reply-timeout=5000",
                    "--dest=org.freedesktop.FileManager1",
                    "--type=method_call",
                    "/org/freedesktop/FileManager1",
                    "org.freedesktop.FileManager1.ShowItems",
                ])
                .arg(format!("array:string:{}", file_uri(path)))
                .arg("string:");
                let folder = path.parent().unwrap_or(path);
                vec![quiet(show), open(folder)]
            }
        }
    }
}

fn launch(handoff: &Handoff) -> Result<(), &'static str> {
    let commands = commands(handoff);
    let last = commands.len().saturating_sub(1);
    for (index, mut command) in commands.into_iter().enumerate() {
        let Ok(mut child) = command.spawn() else {
            continue;
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match child.try_wait() {
                // Explorer reports failure even when it shows the file.
                Ok(Some(status)) if status.success() || cfg!(windows) => return Ok(()),
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    if index == last {
                        return Err("The file launcher timed out.");
                    }
                    break;
                }
            }
        }
    }
    Err(match handoff {
        Handoff::Reveal(_) => "Could not show the file. Check that a file manager is installed.",
        Handoff::Open(_) => "Could not open the file. No application is set to open it.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn revealing_passes_an_escaped_uri_and_falls_back_to_the_folder() {
        let path = PathBuf::from("/tmp/a folder/it's, \"x\" é.txt");
        let commands = commands(&Handoff::Reveal(path.clone()));
        assert_eq!(commands[0].get_program(), "dbus-send");
        let uri = commands[0]
            .get_args()
            .find_map(|arg| arg.to_str()?.strip_prefix("array:string:"))
            .unwrap();
        assert_eq!(
            uri,
            "file:///tmp/a%20folder/it%27s%2C%20%22x%22%20%C3%A9.txt"
        );
        assert_eq!(commands[1].get_program(), "xdg-open");
        assert_eq!(
            commands[1].get_args().next().unwrap(),
            path.parent().unwrap()
        );
    }

    #[test]
    fn opening_passes_the_path_as_one_argument_without_a_shell() {
        let path = PathBuf::from("/tmp/$(echo) && rm.txt");
        let commands = commands(&Handoff::Open(path.clone()));
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].get_args().last().unwrap(), path.as_os_str());
    }

    #[test]
    fn handoffs_are_bounded_and_completion_allows_retry() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut opener = FileOpener {
            pending: Some(receiver),
        };
        assert!(
            opener
                .start(Handoff::Open("/tmp".into()), egui::Context::default())
                .is_err()
        );
        assert_eq!(opener.poll(), None);
        sender.send(Err("failed")).unwrap();
        assert_eq!(opener.poll(), Some(Err("failed")));
        assert!(opener.pending.is_none());
    }
}
