//! A real session daemon, run as its own process on a socket in a temporary
//! directory, driven through the public client and session handles.
#![cfg(unix)]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use terminal_core::daemon::{self, DaemonClient, DaemonOptions, PROTOCOL_VERSION};
use terminal_core::{SessionOptions, SessionStatus, TerminalSession};

mod support;

/// The test binary doubles as the daemon process: `Daemon::start` runs it
/// again with this variable naming the socket directory.
const DAEMON_DIR: &str = "TERMINAL_CORE_TEST_DAEMON_DIR";
const DAEMON_IDLE_MS: &str = "TERMINAL_CORE_TEST_DAEMON_IDLE_MS";
/// Likewise, `client_process` is a client that starts a session and waits
/// to be killed.
const CLIENT_DIR: &str = "TERMINAL_CORE_TEST_CLIENT_DIR";

#[test]
fn daemon_process() {
    let Ok(dir) = std::env::var(DAEMON_DIR) else {
        return;
    };
    let idle_exit = std::env::var(DAEMON_IDLE_MS)
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(10));
    daemon::run(
        Path::new(&dir),
        DaemonOptions {
            idle_exit,
            ..DaemonOptions::default()
        },
    )
    .unwrap();
}

#[test]
fn client_process() {
    let Ok(dir) = std::env::var(CLIENT_DIR) else {
        return;
    };
    let client = DaemonClient::connect(Path::new(&dir)).unwrap();
    let (session, id) = client
        .start(shell("printf 'ALIVE\\n'; exec cat"), repaint())
        .unwrap();
    wait_for(|| session.screen_text().contains("ALIVE"));
    println!("SESSION {id}");
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

/// A client that takes over, starts a session that writes without pause,
/// and is then stopped by its test.
const HOLDING_CLIENT_DIR: &str = "TERMINAL_CORE_TEST_HOLDING_CLIENT_DIR";
/// Where that session's program records how far it counted.
const HOLDING_CLIENT_COUNT: &str = "TERMINAL_CORE_TEST_HOLDING_CLIENT_COUNT";

#[test]
fn holding_client_process() {
    let Ok(dir) = std::env::var(HOLDING_CLIENT_DIR) else {
        return;
    };
    let client = DaemonClient::connect(Path::new(&dir)).unwrap();
    let _held = client.take_over(repaint()).unwrap();
    let counted = std::env::var(HOLDING_CLIENT_COUNT).unwrap();
    let (session, id) = client
        .start(
            shell(&format!(
                "i=0; while :; do i=$((i+1)); echo \"count $i\"; echo $i > '{counted}'; done"
            )),
            repaint(),
        )
        .unwrap();
    wait_for(|| session.screen_text().contains("count"));
    println!("SESSION {id}");
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

struct Daemon {
    root: tempfile::TempDir,
    child: Child,
}

impl Daemon {
    fn start() -> Self {
        Self::with_idle_exit(Duration::from_secs(10))
    }

    fn with_idle_exit(idle_exit: Duration) -> Self {
        let root = tempfile::tempdir().unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "daemon_process",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(DAEMON_DIR, root.path().join("sessions"))
            .env(DAEMON_IDLE_MS, idle_exit.as_millis().to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Self { root, child }
    }

    fn dir(&self) -> std::path::PathBuf {
        self.root.path().join("sessions")
    }

    /// Connect once the daemon listens.
    fn client(&self) -> DaemonClient {
        Self::connect_until_listening(&self.dir())
    }

    fn connect_until_listening(dir: &Path) -> DaemonClient {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match DaemonClient::connect(dir) {
                Ok(client) => return client,
                Err(error) => {
                    assert!(Instant::now() < deadline, "daemon never listened: {error}");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn shell(script: &str) -> SessionOptions {
    SessionOptions {
        shell: Some("/bin/sh".into()),
        args: vec!["-c".into(), script.into()],
        cols: 80,
        rows: 12,
        scrollback: 128,
        ..SessionOptions::default()
    }
}

fn repaint() -> terminal_core::Repaint {
    Arc::new(|| {})
}

#[track_caller]
fn wait_for(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_session_keeps_running_after_its_handle_is_dropped_and_a_new_client_reattaches() {
    let daemon = Daemon::start();
    let client = daemon.client();
    let (session, id) = client
        .start(
            shell("i=0; while [ $i -lt 30 ]; do echo line$i; i=$((i+1)); done; printf 'READY'; exec cat"),
            repaint(),
        )
        .unwrap();
    wait_for(|| session.screen_text().contains("READY"));
    let screen = session.screen_text();
    let history = session.history_text();
    let cursor = session.viewport().cursor;
    assert!(history.contains("line0"), "{history}");
    drop(session);
    drop(client);

    let client = daemon.client();
    let listed = client.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
    assert_eq!(listed[0].status, SessionStatus::Running);

    let attached = client.attach(id, 80, 12, repaint()).unwrap();
    let session: &TerminalSession = &attached.session;
    assert_eq!(session.screen_text(), screen);
    assert_eq!(session.history_text(), history);
    assert_eq!(session.viewport().cursor, cursor);

    // The same program is still running and reads what this client types.
    session.write(b"again\r").unwrap();
    wait_for(|| session.screen_text().matches("again").count() == 2);
}

/// Probe the terminal once `trigger` exists, recording every reply that
/// arrives within half a second in `result`.
fn cursor_probe(trigger: &Path, result: &Path) -> String {
    format!(
        "while [ ! -e '{trigger}' ]; do sleep 0.02; done; \
         printf '\\033[6n'; sleep 0.5; dd bs=256 count=1 2>/dev/null > '{result}.part'; \
         mv '{result}.part' '{result}'",
        trigger = trigger.display(),
        result = result.display(),
    )
}

#[track_caller]
fn single_cursor_reply(path: &Path) {
    wait_for(|| path.exists());
    let reply = std::fs::read(path).unwrap();
    let text = String::from_utf8_lossy(&reply);
    let fields: Vec<&str> = text
        .strip_prefix("\x1b[")
        .and_then(|rest| rest.strip_suffix('R'))
        .map(|rest| rest.split(';').collect())
        .unwrap_or_default();
    assert!(
        fields.len() == 2 && fields.iter().all(|n| n.parse::<u16>().is_ok()),
        "expected exactly one cursor position reply, got {text:?}"
    );
}

#[test]
fn output_while_detached_is_shown_after_reattach() {
    let daemon = Daemon::start();
    let go = daemon.root.path().join("go");
    let done = daemon.root.path().join("done");
    let client = daemon.client();
    let (session, id) = client
        .start(
            shell(&format!(
                "printf 'BEFORE\\n'; while [ ! -e '{go}' ]; do sleep 0.02; done; \
                 printf 'WHILE-DETACHED\\n'; touch '{done}'; exec cat",
                go = go.display(),
                done = done.display()
            )),
            repaint(),
        )
        .unwrap();
    wait_for(|| session.screen_text().contains("BEFORE"));
    drop(session);
    std::fs::write(&go, b"").unwrap();
    wait_for(|| done.exists());

    let attached = client.attach(id, 80, 12, repaint()).unwrap();
    let screen = attached.session.screen_text();
    assert!(
        screen.contains("BEFORE") && screen.contains("WHILE-DETACHED"),
        "{screen}"
    );
}

#[test]
fn a_cursor_position_query_gets_exactly_one_reply_detached_and_attached() {
    let daemon = Daemon::start();
    let dir = daemon.root.path();
    let client = daemon.client();
    let (session, id) = client
        .start(
            shell(&format!(
                "stty raw -echo min 0; printf 'READY'; {}; {}; exec cat",
                cursor_probe(&dir.join("go-detached"), &dir.join("detached")),
                cursor_probe(&dir.join("go-attached"), &dir.join("attached")),
            )),
            repaint(),
        )
        .unwrap();
    wait_for(|| session.screen_text().contains("READY"));
    drop(session);
    std::fs::write(dir.join("go-detached"), b"").unwrap();
    single_cursor_reply(&dir.join("detached"));

    let attached = client.attach(id, 80, 12, repaint()).unwrap();
    std::fs::write(dir.join("go-attached"), b"").unwrap();
    single_cursor_reply(&dir.join("attached"));
    drop(attached);
}

#[test]
fn data_set_by_one_client_is_returned_to_the_next() {
    let daemon = Daemon::start();
    let client = daemon.client();
    let (session, id) = client.start(shell("exec cat"), repaint()).unwrap();
    client.set_data(id, b"agent-token-1").unwrap();
    drop(session);
    drop(client);

    let client = daemon.client();
    assert_eq!(client.list().unwrap()[0].data, b"agent-token-1");
    let attached = client.attach(id, 80, 12, repaint()).unwrap();
    assert_eq!(attached.data, b"agent-token-1");
}

fn process_alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

/// A session that records its program's pid.
fn recorded_session(
    client: &DaemonClient,
    pid_file: &Path,
) -> (TerminalSession, daemon::SessionId, String) {
    let (session, id) = client
        .start(
            shell(&format!("echo $$ > '{}'; exec cat", pid_file.display())),
            repaint(),
        )
        .unwrap();
    wait_for(|| std::fs::read_to_string(pid_file).is_ok_and(|pid| pid.ends_with('\n')));
    let pid = std::fs::read_to_string(pid_file).unwrap().trim().to_owned();
    assert!(process_alive(&pid));
    (session, id, pid)
}

#[test]
fn close_ends_the_session_processes_and_removes_it_from_the_list() {
    let daemon = Daemon::start();
    let client = daemon.client();

    // Closing an attached handle.
    let (session, _, pid) = recorded_session(&client, &daemon.root.path().join("first"));
    session.shutdown();
    wait_for(|| !process_alive(&pid));
    wait_for(|| client.list().unwrap().is_empty());
    drop(session);

    // Closing a detached session through the client.
    let (session, id, pid) = recorded_session(&client, &daemon.root.path().join("second"));
    drop(session);
    assert_eq!(client.list().unwrap().len(), 1);
    client.close(id).unwrap();
    assert!(client.list().unwrap().is_empty());
    wait_for(|| !process_alive(&pid));
    assert_eq!(
        client
            .attach(id, 80, 12, repaint())
            .err()
            .map(|error| error.kind()),
        Some(terminal_core::SessionErrorKind::NotFound)
    );
}

#[test]
fn the_socket_directory_is_user_only_and_clients_refuse_a_shared_one() {
    use std::os::unix::fs::PermissionsExt;
    let daemon = Daemon::start();
    let dir = daemon.dir();
    let mode = |dir: &Path| std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
    drop(daemon.client());
    assert_eq!(mode(&dir), 0o700);

    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = DaemonClient::connect(&dir)
        .err()
        .expect("connected through a shared directory");
    assert_eq!(error.kind(), terminal_core::SessionErrorKind::Unavailable);
}

/// Every version opens with the same handshake, so the daemon tells a
/// client of another version which one it speaks.
#[test]
fn the_daemon_welcomes_its_own_version_and_refuses_others() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    let daemon = Daemon::start();
    drop(daemon.client());
    let socket = support::socket(&daemon.dir(), PROTOCOL_VERSION);

    let mut stream = UnixStream::connect(&socket).unwrap();
    stream.write_all(&support::hello(PROTOCOL_VERSION)).unwrap();
    let mut welcome = vec![0; support::welcome(PROTOCOL_VERSION).len()];
    stream.read_exact(&mut welcome).unwrap();
    assert_eq!(welcome, support::welcome(PROTOCOL_VERSION));

    let mut stream = UnixStream::connect(&socket).unwrap();
    stream
        .write_all(&support::hello(PROTOCOL_VERSION + 1))
        .unwrap();
    let mut refusal = Vec::new();
    stream.read_to_end(&mut refusal).unwrap();
    assert_eq!(refusal, support::refused(PROTOCOL_VERSION));
}

#[test]
fn a_client_reports_a_daemon_of_another_version_clearly() {
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("sessions");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener =
        std::os::unix::net::UnixListener::bind(support::socket(&dir, PROTOCOL_VERSION)).unwrap();
    let theirs = PROTOCOL_VERSION + 1;
    let daemon = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = vec![0; support::hello(PROTOCOL_VERSION).len()];
        stream.read_exact(&mut request).unwrap();
        assert_eq!(request, support::hello(PROTOCOL_VERSION));
        stream.write_all(&support::refused(theirs)).unwrap();
    });

    let error = DaemonClient::connect(&dir)
        .err()
        .expect("connected to another version");
    assert_eq!(error.kind(), terminal_core::SessionErrorKind::Incompatible);
    assert_eq!(
        error.to_string(),
        format!(
            "The session daemon speaks protocol version {theirs}; this client speaks version {PROTOCOL_VERSION}"
        )
    );
    daemon.join().unwrap();
}

#[test]
fn the_daemon_exits_after_its_grace_period_only_without_sessions_and_clients() {
    let grace = Duration::from_millis(300);
    let mut daemon = Daemon::with_idle_exit(grace);
    let dir = daemon.dir();
    let mut running = || daemon.child.try_wait().unwrap().is_none();

    let client = Daemon::connect_until_listening(&dir);
    std::thread::sleep(grace * 3);
    assert!(running(), "exited while a client was connected");
    let (session, id) = client.start(shell("exec cat"), repaint()).unwrap();
    drop(session);
    drop(client);
    std::thread::sleep(grace * 3);
    assert!(running(), "exited while a session was running");

    let client = Daemon::connect_until_listening(&dir);
    client.close(id).unwrap();
    std::thread::sleep(grace * 3);
    assert!(running(), "exited while a client was connected");
    drop(client);
    wait_for(|| !running());
    assert!(daemon.child.wait().unwrap().success());
    assert!(!support::socket(&dir, PROTOCOL_VERSION).exists());
}

#[test]
fn sessions_outlive_a_client_process_that_is_killed() {
    use std::io::BufRead;
    let daemon = Daemon::start();
    drop(daemon.client());
    let mut client = Command::new(std::env::current_exe().unwrap())
        .args([
            "client_process",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CLIENT_DIR, daemon.dir())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let id: daemon::SessionId = std::io::BufReader::new(client.stdout.take().unwrap())
        .lines()
        .map_while(Result::ok)
        .find_map(|line| line.split_once("SESSION ").map(|(_, id)| id.to_owned()))
        .expect("the client process never started its session")
        .parse()
        .unwrap();
    client.kill().unwrap();
    client.wait().unwrap();

    let client = daemon.client();
    let listed = client.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0].id, &listed[0].status),
        (id, &SessionStatus::Running)
    );
    let attached = client.attach(id, 80, 12, repaint()).unwrap();
    assert!(attached.session.screen_text().contains("ALIVE"));
    attached.session.write(b"still here\r").unwrap();
    wait_for(|| attached.session.screen_text().matches("still here").count() == 2);
}

#[test]
fn daemon_diagnostics_contain_no_contents_input_titles_or_paths() {
    use std::io::{Read, Write};
    let mut daemon = Daemon::with_idle_exit(Duration::from_millis(200));
    let cwd = daemon.root.path().join("path-marker");
    std::fs::create_dir(&cwd).unwrap();
    let client = daemon.client();
    let (session, id) = client
        .start(
            SessionOptions {
                cwd: cwd.clone(),
                ..shell("printf '\\033]0;title-marker\\007output-marker\\n'; exec cat")
            },
            repaint(),
        )
        .unwrap();
    wait_for(|| session.metadata().title == "title-marker");
    session.write(b"input-marker\r").unwrap();
    wait_for(|| session.screen_text().matches("input-marker").count() == 2);
    // Give the daemon something to report: a malformed request carrying a marker.
    let mut stream =
        std::os::unix::net::UnixStream::connect(support::socket(&daemon.dir(), PROTOCOL_VERSION))
            .unwrap();
    stream.write_all(&support::hello(PROTOCOL_VERSION)).unwrap();
    stream.read_exact(&mut [0; 9]).unwrap();
    stream.write_all(b"\x0f\0\0\0\x03request-marker").unwrap();
    let _ = stream.read(&mut [0; 16]);
    drop(stream);
    drop(session);
    client.close(id).unwrap();
    drop(client);

    assert!(daemon.child.wait().unwrap().success());
    let mut diagnostics = String::new();
    daemon
        .child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut diagnostics)
        .unwrap();
    assert!(
        diagnostics.contains("neptune session daemon: read request"),
        "{diagnostics}"
    );
    for marker in [
        "path-marker",
        "title-marker",
        "output-marker",
        "input-marker",
        "request-marker",
    ] {
        assert!(!diagnostics.contains(marker), "{marker} in {diagnostics}");
    }
    assert!(
        !diagnostics.contains(&*daemon.root.path().to_string_lossy()),
        "{diagnostics}"
    );
}

#[test]
fn input_paste_and_resize_reach_the_program_through_the_daemon() {
    let daemon = Daemon::start();
    let client = daemon.client();
    let (session, id) = client
        .start(
            shell("while IFS= read -r line; do echo \"GOT:$line $(stty size)\"; done"),
            repaint(),
        )
        .unwrap();
    session.write(b"typed\r").unwrap();
    wait_for(|| session.screen_text().contains("GOT:typed 12 80"));
    session.paste("one\ntwo").unwrap();
    session.write(b"\r").unwrap();
    wait_for(|| session.screen_text().contains("GOT:two 12 80"));
    session.resize(90, 15, 900, 300).unwrap();
    session.write(b"resized\r").unwrap();
    wait_for(|| session.screen_text().contains("GOT:resized 15 90"));
    drop(session);

    // Attaching applies the attaching pane's size.
    let attached = client.attach(id, 100, 20, repaint()).unwrap();
    attached.session.write(b"attached\r").unwrap();
    wait_for(|| {
        attached
            .session
            .screen_text()
            .contains("GOT:attached 20 100")
    });
}

#[test]
fn the_daemon_reports_the_attached_pane_focus_once() {
    let daemon = Daemon::start();
    let dir = daemon.root.path();
    let (go, enabled, changed) = (dir.join("go"), dir.join("enabled"), dir.join("changed"));
    let client = daemon.client();
    let (session, _) = client
        .start(
            shell(&format!(
                "stty raw -echo min 0; printf 'READY'; \
                 while [ ! -e '{go}' ]; do sleep 0.02; done; \
                 printf '\\033[?1004h'; sleep 0.5; dd bs=64 count=1 2>/dev/null > '{enabled}.part'; \
                 mv '{enabled}.part' '{enabled}'; \
                 sleep 1; dd bs=64 count=1 2>/dev/null > '{changed}.part'; mv '{changed}.part' '{changed}'; \
                 exec cat",
                go = go.display(),
                enabled = enabled.display(),
                changed = changed.display(),
            )),
            repaint(),
        )
        .unwrap();
    wait_for(|| session.screen_text().contains("READY"));
    // A daemon engine starts unfocused; this pane has focus.
    session.focus(true).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    std::fs::write(&go, b"").unwrap();
    // Enabling reports answers with the pane's focus, which the daemon follows.
    wait_for(|| enabled.exists());
    assert_eq!(std::fs::read(&enabled).unwrap(), b"\x1b[I");
    session.focus(false).unwrap();
    wait_for(|| changed.exists());
    assert_eq!(std::fs::read(&changed).unwrap(), b"\x1b[O");
}

#[test]
fn a_full_screen_program_keeps_its_screen_and_input_modes_after_reattach() {
    let daemon = Daemon::start();
    let input = daemon.root.path().join("input");
    let client = daemon.client();
    let (session, id) = client
        .start(
            shell(&format!(
                "printf 'shell prompt\\n'; stty raw -echo; \
                 printf '\\033[?1049h\\033[H\\033[1;33mFULL SCREEN\\033[m\\033[?1002h\\033[?1006h\\033[?2004hREADY'; \
                 head -c 23 > '{input}.part'; mv '{input}.part' '{input}'; \
                 printf '\\033[?1049lBACK'; exec cat",
                input = input.display()
            )),
            repaint(),
        )
        .unwrap();
    wait_for(|| session.screen_text().contains("READY"));
    drop(session);

    let attached = client.attach(id, 80, 12, repaint()).unwrap();
    let session = &attached.session;
    let modes = session.modes();
    for mode in [
        terminal_core::Mode::ALT_SCREEN,
        terminal_core::Mode::MOUSE_DRAG,
        terminal_core::Mode::SGR_MOUSE,
        terminal_core::Mode::BRACKETED_PASTE,
    ] {
        assert!(modes.contains(mode), "{mode:?} is lost: {modes:?}");
    }
    let screen = session.screen_text();
    assert!(
        screen.contains("FULL SCREEN") && !screen.contains("shell prompt"),
        "{screen}"
    );

    // Input is encoded for the program's modes, as the desktop does.
    let click = terminal_core::input::mouse(0, 2, 1, true, Default::default(), modes).unwrap();
    session.write(&click).unwrap();
    session.paste("hi").unwrap();
    wait_for(|| input.exists());
    assert_eq!(
        String::from_utf8_lossy(&std::fs::read(&input).unwrap()),
        "\x1b[<0;3;2M\x1b[200~hi\x1b[201~"
    );
    // Leaving the alternate screen shows the shell's screen from before it.
    wait_for(|| session.screen_text().contains("BACK"));
    let screen = session.screen_text();
    assert!(screen.contains("shell prompt"), "{screen}");
}

/// The number each of `session`'s lines holds, up to the first line that
/// holds none. Any other text there means output was mangled.
fn numbered_lines(session: &TerminalSession) -> Vec<u32> {
    session
        .history_text()
        .lines()
        .map(str::trim)
        .take_while(|line| !line.is_empty() && *line != "DONE")
        .map(|line| {
            line.parse()
                .unwrap_or_else(|_| panic!("mangled output line {line:?}"))
        })
        .collect()
}

#[test]
fn attaching_during_sustained_output_delivers_every_byte_exactly_once() {
    const LINES: u32 = 20_000;
    let daemon = Daemon::start();
    let client = daemon.client();
    // Coloured, numbered lines in bursts, so attaches land inside bursts and
    // inside escape sequences.
    let options = SessionOptions {
        cols: 20,
        scrollback: 25_000,
        ..shell(&format!(
            "awk 'BEGIN {{ for (i = 1; i <= {LINES}; i++) {{ \
             printf \"\\033[3%dm%d\\033[m\\n\", i % 8, i; \
             if (i % 100 == 0) {{ fflush(); system(\"sleep 0.01\") }} }} }}'; \
             printf 'DONE'; exec cat"
        ))
    };
    let (session, id) = client.start(options, repaint()).unwrap();
    drop(session);

    let mut attaches_during_output = 0;
    let last = loop {
        let attached = client.attach(id, 20, 12, repaint()).unwrap();
        if attached.session.screen_text().contains("DONE") {
            break attached;
        }
        attaches_during_output += 1;
        std::thread::sleep(Duration::from_millis(20));
        let mut numbers = numbered_lines(&attached.session);
        // The program may still be writing the last line.
        numbers.pop();
        if let Some(index) = (0..numbers.len()).find(|&index| numbers[index] != index as u32 + 1) {
            panic!(
                "attach {attaches_during_output} lost or repeated output after line {index}: {:?}",
                &numbers[index.saturating_sub(3)..(index + 3).min(numbers.len())]
            );
        }
    };
    assert!(
        attaches_during_output >= 3,
        "only {attaches_during_output} attaches happened during the output"
    );
    let numbers = numbered_lines(&last.session);
    assert_eq!(numbers, (1..=LINES).collect::<Vec<_>>());
}

/// A program that prints READY, then its last words and exits 3 once `go` exists.
fn exits_on(go: &Path) -> SessionOptions {
    shell(&format!(
        "printf 'READY\\n'; while [ ! -e '{go}' ]; do sleep 0.02; done; \
         printf '\\033[1mfinal words\\033[m'; exit 3",
        go = go.display()
    ))
}

const EXITED_3: SessionStatus = SessionStatus::Exited {
    code: 3,
    signal: None,
};

/// A client takes the exited session: its final screen and status, after
/// which the daemon lets the session go. It attaches at another size than
/// the session had, as a reattaching pane often does.
#[track_caller]
fn take_exited(client: &DaemonClient, id: daemon::SessionId) {
    let attached = client.attach(id, 100, 30, repaint()).unwrap();
    let screen = attached.session.screen_text();
    assert!(screen.contains("final words"), "{screen}");
    wait_for(|| attached.session.metadata().status == EXITED_3);
    wait_for(|| client.list().unwrap().is_empty());
    let error = client.attach(id, 80, 12, repaint()).err().unwrap();
    assert_eq!(error.kind(), terminal_core::SessionErrorKind::NotFound);
}

#[test]
fn an_exited_session_keeps_its_final_screen_and_status_until_a_client_takes_them() {
    let daemon = Daemon::start();
    let go = daemon.root.path().join("go");
    let client = daemon.client();
    let (session, id) = client.start(exits_on(&go), repaint()).unwrap();
    wait_for(|| session.screen_text().contains("READY"));
    // The program exits right after its handle is dropped, while the
    // departing replica may still be connected.
    drop(session);
    std::fs::write(&go, b"").unwrap();
    wait_for(|| {
        client
            .list()
            .unwrap()
            .iter()
            .any(|entry| entry.status == EXITED_3)
    });
    take_exited(&client, id);
}

/// One frame from a raw connection: its kind and payload.
fn read_frame(stream: &mut std::os::unix::net::UnixStream) -> (u8, Vec<u8>) {
    use std::io::Read;
    let mut len = [0; 4];
    stream.read_exact(&mut len).unwrap();
    let mut frame = vec![0; u32::from_le_bytes(len) as usize];
    stream.read_exact(&mut frame).unwrap();
    let kind = frame.remove(0);
    (kind, frame)
}

/// Attach over a raw connection, which never takes an exit: the connection
/// and the kind of the daemon's reply.
fn raw_attach(
    daemon: &Daemon,
    id: daemon::SessionId,
    cols: u16,
    rows: u16,
) -> (std::os::unix::net::UnixStream, u8) {
    use std::io::Write;
    let mut stream =
        std::os::unix::net::UnixStream::connect(support::socket(&daemon.dir(), PROTOCOL_VERSION))
            .unwrap();
    stream.write_all(&support::hello(PROTOCOL_VERSION)).unwrap();
    assert_eq!(read_frame(&mut stream).0, 64, "welcome");
    // Attach: kind 6, the id, then the size.
    let mut attach = vec![13, 0, 0, 0, 6];
    attach.extend(
        u64::from_str_radix(&id.to_string(), 16)
            .unwrap()
            .to_le_bytes(),
    );
    attach.extend(cols.to_le_bytes());
    attach.extend(rows.to_le_bytes());
    stream.write_all(&attach).unwrap();
    let reply = read_frame(&mut stream).0;
    (stream, reply)
}

#[test]
fn a_pane_of_any_size_can_attach_long_after_its_session_exited() {
    let daemon = Daemon::start();
    let go = daemon.root.path().join("go");
    let client = daemon.client();
    let (session, id) = client.start(exits_on(&go), repaint()).unwrap();
    wait_for(|| session.screen_text().contains("READY"));
    drop(session);
    std::fs::write(&go, b"").unwrap();
    wait_for(|| {
        client
            .list()
            .unwrap()
            .iter()
            .any(|entry| entry.status == EXITED_3)
    });
    // Long enough for the ended session's workers to stop, after which its
    // terminal can no longer be resized.
    let until = Instant::now() + Duration::from_secs(1);
    let mut cols = 81;
    while Instant::now() < until {
        assert_eq!(
            raw_attach(&daemon, id, cols, 24).1,
            69,
            "attached at {cols}"
        );
        cols ^= 1;
    }
    take_exited(&client, id);
}

#[test]
fn an_exit_sent_to_a_client_that_leaves_without_taking_it_is_kept() {
    let daemon = Daemon::start();
    let go = daemon.root.path().join("go");
    let client = daemon.client();
    let (session, id) = client.start(exits_on(&go), repaint()).unwrap();
    wait_for(|| session.screen_text().contains("READY"));
    drop(session);
    // A client that departs right after the daemon sends it the exit must
    // not be the one that takes it.
    let (mut stream, reply) = raw_attach(&daemon, id, 80, 12);
    assert_eq!(reply, 69, "attached");
    std::fs::write(&go, b"").unwrap();
    loop {
        match read_frame(&mut stream).0 {
            70 => {}
            71 => break,
            kind => panic!("unexpected frame {kind}"),
        }
    }
    drop(stream);
    assert_eq!(
        client
            .list()
            .unwrap()
            .into_iter()
            .map(|entry| (entry.id, entry.status))
            .collect::<Vec<_>>(),
        vec![(id, EXITED_3)],
        "the daemon kept the exited session"
    );
    take_exited(&client, id);
}

#[test]
fn a_reattached_session_keeps_the_history_its_scrollback_allows() {
    let daemon = Daemon::start();
    let client = daemon.client();
    let options = SessionOptions {
        scrollback: 50,
        ..shell(
            "i=1; while [ $i -le 500 ]; do echo line$i; i=$((i+1)); done; printf 'READY'; exec cat",
        )
    };
    let (session, id) = client.start(options, repaint()).unwrap();
    wait_for(|| session.screen_text().contains("READY"));
    let history = session.history_text();
    drop(session);

    let attached = client.attach(id, 80, 12, repaint()).unwrap();
    assert_eq!(attached.session.history_size(), 50);
    assert_eq!(attached.session.history_text(), history);
    assert!(history.starts_with("line440\n"), "{history}");
}

#[test]
fn a_newer_client_takes_over_every_session_and_the_earlier_one_is_told() {
    let daemon = Daemon::start();
    let earlier = daemon.client();
    let (told, notice) = std::sync::mpsc::channel();
    let held = earlier
        .take_over(Arc::new(move || {
            let _ = told.send(());
        }))
        .unwrap();
    let (first, first_id) = earlier
        .start(shell("printf 'FIRST\\n'; exec cat"), repaint())
        .unwrap();
    let (second, second_id) = earlier
        .start(shell("printf 'SECOND\\n'; exec cat"), repaint())
        .unwrap();
    wait_for(|| first.screen_text().contains("FIRST") && second.screen_text().contains("SECOND"));
    assert!(!held.replaced());

    let newer = daemon.client();
    let holding = newer.take_over(repaint()).unwrap();
    // The earlier client knows as soon as the newer one holds the daemon.
    assert!(held.replaced());
    assert!(!holding.replaced());
    notice
        .recv_timeout(Duration::from_secs(5))
        .expect("the earlier client was never told");
    // Its handles were let go of: closing them no longer reaches the daemon.
    wait_for(|| first.metadata().status != SessionStatus::Running);
    first.shutdown();
    second.shutdown();
    drop((first, second));

    let mut listed: Vec<_> = newer
        .list()
        .unwrap()
        .into_iter()
        .map(|entry| (entry.id, entry.status))
        .collect();
    listed.sort_by_key(|(id, _)| id.to_string());
    let mut expected = vec![
        (first_id, SessionStatus::Running),
        (second_id, SessionStatus::Running),
    ];
    expected.sort_by_key(|(id, _)| id.to_string());
    assert_eq!(listed, expected);
    for (id, marker) in [(first_id, "FIRST"), (second_id, "SECOND")] {
        let attached = newer.attach(id, 80, 12, repaint()).unwrap();
        assert!(attached.session.screen_text().contains(marker));
        attached.session.write(b"taken\r").unwrap();
        wait_for(|| attached.session.screen_text().matches("taken").count() == 2);
    }
}

/// The highest count `path` held.
fn recorded_count(path: &Path, highest: &mut u64) -> u64 {
    if let Some(count) = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse().ok())
    {
        *highest = (*highest).max(count);
    }
    *highest
}

/// A stopped test process, continued and ended however its test ends.
struct Stopped(Child);

impl Drop for Stopped {
    fn drop(&mut self) {
        let pid = self.0.id().to_string();
        let _ = Command::new("kill").args(["-CONT", &pid]).status();
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_client_that_stopped_responding_never_holds_back_the_sessions_taken_over_from_it() {
    use std::io::BufRead;
    let daemon = Daemon::start();
    drop(daemon.client());
    let counted = daemon.root.path().join("count");
    let mut holding = Command::new(std::env::current_exe().unwrap())
        .args([
            "holding_client_process",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(HOLDING_CLIENT_DIR, daemon.dir())
        .env(HOLDING_CLIENT_COUNT, &counted)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let output = holding.stdout.take().unwrap();
    let holding = Stopped(holding);
    let id: daemon::SessionId = std::io::BufReader::new(output)
        .lines()
        .map_while(Result::ok)
        .find_map(|line| line.split_once("SESSION ").map(|(_, id)| id.to_owned()))
        .expect("the holding client never started its session")
        .parse()
        .unwrap();
    assert!(
        Command::new("kill")
            .args(["-STOP", &holding.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    // Its program soon fills everything queued for the stopped client, and waits.
    let mut highest = 0;
    let mut last = u64::MAX;
    wait_for(|| {
        std::thread::sleep(Duration::from_millis(200));
        let count = recorded_count(&counted, &mut highest);
        std::mem::replace(&mut last, count) == count
    });
    let stalled = highest;

    let client = daemon.client();
    let _held = client.take_over(repaint()).unwrap();
    // Taken over, it runs on before anything attaches to it.
    wait_for(|| recorded_count(&counted, &mut highest) > stalled + 1000);
    let attached = client.attach(id, 80, 12, repaint()).unwrap();
    attached.session.shutdown();
    wait_for(|| client.list().unwrap().is_empty());
}

/// A snapshot can't carry a full-screen program's scroll region, tab stops
/// or character sets, so attaching at the size it already has makes it
/// redraw, as a resize would.
#[test]
fn a_full_screen_program_redraws_when_a_client_attaches_at_its_size() {
    let daemon = Daemon::start();
    let client = daemon.client();
    let (session, id) = client
        .start(
            shell(
                "printf '\\033[?1049hFULL'; trap 'printf REDRAWN' WINCH; \
                 while :; do sleep 0.05; done",
            ),
            repaint(),
        )
        .unwrap();
    wait_for(|| session.screen_text().contains("FULL"));
    drop(session);

    let attached = client.attach(id, 80, 12, repaint()).unwrap();
    wait_for(|| attached.session.screen_text().contains("REDRAWN"));
    attached.session.shutdown();
}

/// A shell on its own screen is left alone: attaching at its size doesn't
/// make it redraw a prompt it already shows.
#[test]
fn a_shell_on_its_own_screen_is_not_asked_to_redraw_on_attach() {
    let daemon = Daemon::start();
    let client = daemon.client();
    let (session, id) = client
        .start(
            shell("printf 'PLAIN'; trap 'printf REDRAWN' WINCH; while :; do sleep 0.05; done"),
            repaint(),
        )
        .unwrap();
    wait_for(|| session.screen_text().contains("PLAIN"));
    drop(session);

    let attached = client.attach(id, 80, 12, repaint()).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(!attached.session.screen_text().contains("REDRAWN"));
    attached.session.shutdown();
}

/// The history limit and the palette a client sets reach the daemon's
/// engine, which keeps the history and answers colour queries.
#[test]
fn a_clients_history_limit_and_colours_reach_the_daemon() {
    let daemon = Daemon::start();
    let result = daemon.root.path().join("background");
    let client = daemon.client();
    let (session, id) = client
        .start(
            SessionOptions {
                scrollback: 1000,
                ..shell(&format!(
                    "stty raw -echo min 0; i=0; while [ $i -lt 300 ]; do echo line$i; i=$((i+1)); done; \
                     printf 'READY'; while [ \"$(dd bs=1 count=1 2>/dev/null)\" != g ]; do :; done; \
                     printf '\\033]11;?\\033\\\\'; sleep 0.5; dd bs=256 count=1 2>/dev/null > '{result}.part'; \
                     mv '{result}.part' '{result}'; exec sleep 30",
                    result = result.display()
                ))
            },
            repaint(),
        )
        .unwrap();
    wait_for(|| session.screen_text().contains("READY"));
    session.set_scrollback(50).unwrap();
    session.set_color(
        terminal_core::NamedColor::Background as usize,
        terminal_core::Rgb { r: 1, g: 2, b: 3 },
    );
    // Typed after the colour, so the daemon has it before the query.
    session.write(b"g").unwrap();
    wait_for(|| result.exists());
    let reply = String::from_utf8_lossy(&std::fs::read(&result).unwrap()).into_owned();
    assert!(reply.contains("rgb:0101/0202/0303"), "{reply:?}");
    drop(session);

    let attached = client.attach(id, 80, 12, repaint()).unwrap();
    assert_eq!(attached.session.history_size(), 50);
    attached.session.shutdown();
}

/// End all ends attached and detached sessions in one request.
#[test]
fn ending_all_sessions_ends_attached_and_detached_ones_at_once() {
    let daemon = Daemon::start();
    let client = daemon.client();
    let (attached, _, attached_pid) =
        recorded_session(&client, &daemon.root.path().join("attached"));
    let (detached, _, detached_pid) =
        recorded_session(&client, &daemon.root.path().join("detached"));
    drop(detached);
    assert_eq!(client.list().unwrap().len(), 2);

    client.end_all().unwrap();
    assert!(client.list().unwrap().is_empty());
    wait_for(|| !process_alive(&attached_pid) && !process_alive(&detached_pid));
    wait_for(|| attached.metadata().status != SessionStatus::Running);
}
