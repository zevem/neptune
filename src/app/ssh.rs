//! SSH launch commands. Remote paths are data, never terminal input or local cwd.
use std::path::Path;

use neptune_model::Remote;

const BOOTSTRAP: &str = include_str!("ssh-bootstrap.sh");

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn arguments(remote: &Remote, cwd: Option<&Path>) -> Vec<String> {
    // Explicit commands need a remote PTY. Quote the entire script and the
    // path separately because OpenSSH sends a command string to the host shell.
    let directory = cwd.and_then(Path::to_str).unwrap_or_default();
    vec![
        "-t".into(),
        "--".into(),
        remote.destination().into(),
        format!("sh -c {} neptune {}", quote(BOOTSTRAP), quote(directory)),
    ]
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::PermissionsExt,
        sync::Arc,
        time::{Duration, Instant},
    };
    use terminal_core::{SessionOptions, SessionStatus, TerminalSession};

    fn wait_for(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "SSH bootstrap condition timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn bootstrap(home: &Path, dotdir: Option<&Path>, cwd: Option<&Path>) -> TerminalSession {
        let command = arguments(&Remote::parse("devbox").unwrap(), cwd).remove(3);
        let mut env = vec![
            ("SHELL".into(), "zsh".into()),
            ("HOME".into(), home.to_str().unwrap().into()),
            ("TMPDIR".into(), home.to_str().unwrap().into()),
            (
                "ZDOTDIR".into(),
                dotdir.unwrap_or(home).to_str().unwrap().into(),
            ),
        ];
        // Fixtures load test-owned user startup files.
        env.push(("NEPTUNE_STARTUP".into(), String::new()));
        TerminalSession::spawn(
            SessionOptions {
                shell: Some("/bin/sh".into()),
                args: vec!["-c".into(), command],
                cwd: home.into(),
                env,
                ..Default::default()
            },
            Arc::new(|| {}),
        )
        .unwrap()
    }

    #[test]
    fn zsh_bootstrap_preserves_login_files_and_reports_directory_changes() {
        let root = tempfile::tempdir().unwrap();
        let dotdir = root.path().join("dotfiles");
        let project = root.path().join("project space ' % λ $(touch injected)");
        std::fs::create_dir(&dotdir).unwrap();
        std::fs::create_dir(&project).unwrap();
        // Reproduce runner completion paths that would make global compinit
        // ask an interactive security question before the fixture can start.
        let completions = dotdir.join("insecure-completions");
        std::fs::create_dir(&completions).unwrap();
        std::fs::set_permissions(&completions, std::fs::Permissions::from_mode(0o777)).unwrap();
        for (file, stage) in [(".zprofile", "profile"), (".zshrc", "rc")] {
            std::fs::write(
                dotdir.join(file),
                format!("NEPTUNE_STARTUP+=\"{stage} \"\nPROMPT='NEPTUNE> '\n"),
            )
            .unwrap();
        }
        std::fs::write(
            dotdir.join(".zshenv"),
            // Keep the real bootstrap and user login ordering while excluding
            // unrelated system startup such as Ubuntu's interactive compinit.
            // HISTFILE stands in for macOS /etc/zshrc, which uses the startup ZDOTDIR.
            "unsetopt GLOBAL_RCS\nfpath=(\"$ZDOTDIR/insecure-completions\" $fpath)\nNEPTUNE_STARTUP+=\"env \"\nHISTFILE=$_neptune_dir/.zsh_history\n",
        )
        .unwrap();
        std::fs::write(
            dotdir.join(".zlogin"),
            "printf '%slogin' \"$NEPTUNE_STARTUP\" > \"$HOME/startup\"\nprintf '%s' \"$HISTFILE\" > \"$HOME/histfile\"\ncd \"$HOME\"\n",
        )
        .unwrap();
        let session = bootstrap(root.path(), Some(&dotdir), Some(&project));
        wait_for(|| session.metadata().reported_cwd.as_deref() == Some(project.as_path()));
        assert_eq!(
            std::fs::read_to_string(root.path().join("startup")).unwrap(),
            "env profile rc login"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("histfile")).unwrap(),
            dotdir.join(".zsh_history").to_str().unwrap()
        );
        assert!(!root.path().join("injected").exists());
        assert!(!std::fs::read_dir(root.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("neptune-zsh.")
        }));
        session.write(b"cd -- \"$HOME\"\r").unwrap();
        wait_for(|| session.metadata().reported_cwd.as_deref() == Some(root.path()));
        session
            .write(b"printf '%s' \"$ZDOTDIR\" > \"$HOME/zdotdir\"; exit\r")
            .unwrap();
        wait_for(|| !matches!(session.metadata().status, SessionStatus::Running));
        assert_eq!(
            std::fs::read_to_string(root.path().join("zdotdir")).unwrap(),
            dotdir.to_str().unwrap()
        );
    }

    /// A host whose login files set the search path anew, and a `claude`
    /// there that fires one hook of the adapter's with private input.
    #[test]
    fn agents_on_the_host_report_through_the_terminal_and_say_nothing_private() {
        use crate::agent_activity::{Activity, Attention};
        use crate::runtime::agents::AgentBridge;
        use neptune_model::{AgentKind, PaneId};
        use terminal_core::TerminalEvent;

        for shell in ["bash", "zsh"] {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().join("home with ' quote");
            let fixtures = root.path().join("bin");
            std::fs::create_dir(&home).unwrap();
            std::fs::create_dir(&fixtures).unwrap();
            let claude = fixtures.join("claude");
            std::fs::write(
                &claude,
                r#"#!/bin/sh
[ "$1" = --settings ] && grep -q 'hook\\" Stop"' "$2" || { echo BAD SETTINGS; exit 9; }
printf '%s' '{"session_id":"s","cwd":"/srv/it","permission_mode":"default","hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"PRIVATE \"tool_name\":\"Other\""},"agent_id":"a1"}' |
    sh "$NEPTUNE_AGENT_DIR/hook" PermissionRequest
printf 'FIXTURE DONE %s\n' "$3"
"#,
            )
            .unwrap();
            std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
            let path = format!("PATH='{}':/usr/bin:/bin\n", fixtures.display());
            std::fs::write(home.join(".bash_profile"), format!("{path}PS1='> '\n")).unwrap();
            std::fs::write(home.join(".zshenv"), "unsetopt GLOBAL_RCS\n").unwrap();
            std::fs::write(home.join(".zshrc"), format!("{path}PROMPT='> '\n")).unwrap();

            let bridge = AgentBridge::default();
            let pane = PaneId::new(1);
            let mut options = SessionOptions {
                shell: Some("/bin/sh".into()),
                args: arguments(&Remote::parse("devbox").unwrap(), None),
                cwd: root.path().into(),
                env: vec![
                    ("SHELL".into(), shell.into()),
                    ("HOME".into(), home.to_str().unwrap().into()),
                    ("ZDOTDIR".into(), home.to_str().unwrap().into()),
                    ("TMPDIR".into(), root.path().to_str().unwrap().into()),
                    ("XDG_CACHE_HOME".into(), String::new()),
                    // The test itself may run inside an agent's terminal.
                    ("NEPTUNE_AGENT_RUN".into(), String::new()),
                ],
                ..Default::default()
            };
            bridge
                .prepare_remote(pane, 1, &mut options, Arc::new(|| {}))
                .unwrap();
            // Run the host's side here: the client's last argument is its command.
            options.args = vec!["-c".into(), options.args.pop().unwrap()];
            let session = TerminalSession::spawn(options, Arc::new(|| {})).unwrap();
            session.write(b"claude first\r").unwrap();
            let mut reports = Vec::new();
            let mut collect = |session: &TerminalSession, count: usize| {
                wait_for(|| {
                    reports.extend(session.drain_events().into_iter().filter_map(
                        |event| match event {
                            TerminalEvent::Report(line) => Some(line),
                            _ => None,
                        },
                    ));
                    reports.len() >= count
                });
                reports.clone()
            };
            let seen = collect(&session, 3);
            assert!(seen[0].ends_with(";open;claude"), "{shell}: {seen:?}");
            assert!(
                seen[1].ends_with(
                    ";hook;PermissionRequest;tool_name=Bash;permission_mode=default;agent_id=1"
                ),
                "{shell}: {seen:?}"
            );
            assert!(seen[2].ends_with(";close"), "{shell}: {seen:?}");
            assert!(seen.iter().all(|line| !line.contains("PRIVATE")));
            // The bridge takes them for this terminal and no other.
            assert!(!bridge.report(PaneId::new(2), 1, &seen[0]));
            assert!(bridge.report(pane, 1, &seen[0]));
            assert!(bridge.report(pane, 1, &seen[1]));
            assert_eq!(
                bridge.drain_activity().pop().unwrap(),
                (
                    pane,
                    1,
                    Some(Activity::NeedsInput(Attention::Permission)),
                    Some(AgentKind::Claude)
                )
            );
            // The adapter is installed once, privately, under the cache directory.
            let installed = home.join(".cache/neptune/agents");
            let mode = |path: &Path| path.metadata().unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&installed), 0o700);
            assert_eq!(mode(&installed.join("bin/claude")), 0o700);
            assert!(
                !std::fs::read_to_string(installed.join("stamp"))
                    .unwrap()
                    .is_empty()
            );
            // A batch job, and a CLI opened by another, are left as they are.
            session
                .write(b"claude --print x; NEPTUNE_AGENT_RUN=1 claude nested; exit\r")
                .unwrap();
            wait_for(|| !matches!(session.metadata().status, SessionStatus::Running));
            assert!(collect(&session, 3).len() == 3, "{shell}");
        }
    }

    #[test]
    fn a_missing_remote_directory_fails_without_opening_a_shell_elsewhere() {
        let root = tempfile::tempdir().unwrap();
        let session = bootstrap(root.path(), None, Some(&root.path().join("missing")));
        wait_for(|| !matches!(session.metadata().status, SessionStatus::Running));
        assert!(
            matches!(session.metadata().status, SessionStatus::Exited { code, .. } if code != 0)
        );
        assert!(session.metadata().reported_cwd.is_none());
    }
}
