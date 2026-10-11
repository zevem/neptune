//! Batch launches must reach the installed CLI even after a child launcher
//! removes Neptune's environment variables but keeps the adapters on PATH.
#![cfg(unix)]

use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Launch(Child);
impl Launch {
    fn wait(&mut self) -> Option<ExitStatus> {
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return Some(status);
            }
            if Instant::now() >= until {
                return None;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Launch {
    fn drop(&mut self) {
        // A regressed launcher repeatedly execs in this same child PID.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn executable(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn batch_without_shim_variable(provider: &str, args: &[&str], installed: bool) {
    let root = tempfile::tempdir().unwrap();
    let shims = root.path().join("adapters");
    let bin = root.path().join("bin");
    std::fs::create_dir(&shims).unwrap();
    std::fs::create_dir(&bin).unwrap();
    // The wrappers shipped before this regression fix have no marker or
    // environment repair. They can remain on PATH in existing terminals.
    executable(
        &shims.join(provider),
        &format!(
            "#!/bin/sh\nexec '{}' --agent-run {provider} \"$@\"\n",
            env!("CARGO_BIN_EXE_neptune").replace('\'', "'\\''")
        ),
    );
    if installed {
        executable(
            &bin.join(provider),
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$NEPTUNE_TEST_ARGUMENTS\"\nexit 23\n",
        );
    }
    let arguments = root.path().join("arguments");
    let error = root.path().join("stderr");
    for real_directory in [bin.as_path(), Path::new("")] {
        let mut command = Command::new(shims.join(provider));
        command
            .args(args)
            .env_clear()
            .env(
                "PATH",
                std::env::join_paths([shims.as_path(), real_directory]).unwrap(),
            )
            .env("NEPTUNE_TEST_ARGUMENTS", &arguments)
            .current_dir(&bin)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&error).unwrap());
        let mut launch = Launch(command.spawn().unwrap());
        let status = launch.wait();
        assert!(status.is_some(), "{provider} recursed into its own adapter");
        if installed {
            assert_eq!(status.unwrap().code(), Some(23));
            assert_eq!(
                std::fs::read_to_string(&arguments).unwrap(),
                args.iter()
                    .map(|arg| format!("{arg}\n"))
                    .collect::<String>()
            );
        } else {
            assert_eq!(status.unwrap().code(), Some(1));
            assert!(!arguments.exists());
            assert_eq!(
                std::fs::read_to_string(&error).unwrap(),
                format!("Error: {provider} is not installed or is not on PATH\n")
            );
        }
    }
}

#[test]
fn codex_app_server_skips_adapter_without_shim_variable() {
    batch_without_shim_variable("codex", &["app-server", "--listen", "stdio://"], true);
}

#[test]
fn claude_batch_skips_adapter_without_shim_variable() {
    for print in ["--print", "-p"] {
        batch_without_shim_variable("claude", &[print, "fixture prompt"], true);
    }
}

#[test]
fn missing_cli_exits_instead_of_recursing_through_an_adapter() {
    batch_without_shim_variable("codex", &["app-server"], false);
    batch_without_shim_variable("claude", &["--print", "fixture prompt"], false);
}
