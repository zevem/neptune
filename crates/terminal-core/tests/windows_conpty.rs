//! Native Windows CI coverage. The ignored fixture is launched as a real child
//! inside ConPTY; it is not a mock shell or a precomputed terminal transcript.
//! No installed shell or PowerShell profile is required.
#![cfg(windows)]

use std::io::{Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use terminal_core::Mode as TermMode;
use terminal_core::{SessionOptions, SessionStatus, TerminalSession};
use windows_sys::Win32::System::Console::{
    CONSOLE_SCREEN_BUFFER_INFO, ENABLE_ECHO_INPUT, ENABLE_EXTENDED_FLAGS, ENABLE_LINE_INPUT,
    ENABLE_PROCESSED_INPUT, ENABLE_PROCESSED_OUTPUT, ENABLE_QUICK_EDIT_MODE,
    ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode,
    GetConsoleScreenBufferInfo, GetStdHandle, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode,
};

const FIXTURE_ENV: &str = "NEPTUNE_CONPTY_TEST_FIXTURE";
const PASTE: &[u8] = b"\x1b[200~alpha\nbeta[201~\x1b[201~";

fn session(scenario: &str) -> TerminalSession {
    TerminalSession::spawn(
        SessionOptions {
            shell: Some(
                std::env::current_exe()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            ),
            args: vec![
                "--ignored".into(),
                "--exact".into(),
                "conpty_child_fixture".into(),
                "--nocapture".into(),
                "--test-threads=1".into(),
            ],
            env: vec![(FIXTURE_ENV.into(), scenario.into())],
            cols: 80,
            rows: 12,
            scrollback: 128,
            ..SessionOptions::default()
        },
        Arc::new(|| {}),
    )
    .expect("Start a native ConPTY child")
}

fn screen(session: &TerminalSession) -> String {
    session.history_text()
}

fn wait_for(session: &TerminalSession, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "ConPTY condition timed out. Metadata: {:?}; workers: {}; screen: {}",
            session.metadata(),
            session.metrics().active_workers,
            screen(session)
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn exited(session: &TerminalSession, code: u32) -> bool {
    matches!(session.metadata().status, SessionStatus::Exited { code: actual, .. } if actual == code)
}

#[test]
fn conpty_raw_input_reaches_child_and_nonzero_exit_is_reaped() {
    let session = session("input");
    wait_for(&session, || screen(&session).contains("READY"));
    session.write(b"hello").unwrap();
    wait_for(&session, || exited(&session, 7));
    assert!(screen(&session).contains("INPUT:hello"));
    assert_eq!(
        session.metrics().bytes_received,
        session.metrics().bytes_parsed
    );
    wait_for(&session, || session.metrics().active_workers == 0);
}

#[test]
fn conpty_resize_changes_the_native_console_window_dimensions() {
    let session = session("resize");
    wait_for(&session, || screen(&session).contains("READY"));
    session.resize(91, 17, 910, 340).unwrap();
    session.write(b"?").unwrap();
    wait_for(&session, || exited(&session, 0));
    assert_eq!(session.viewport().columns, 91);
    assert_eq!(session.viewport().screen_lines, 17);
    assert!(
        screen(&session).contains("SIZE:91x17"),
        "{}",
        screen(&session)
    );
    wait_for(&session, || session.metrics().active_workers == 0);
}

#[test]
fn conpty_bracketed_paste_delivers_the_exact_sanitized_input() {
    let session = session("paste");
    wait_for(&session, || {
        session.modes().contains(TermMode::BRACKETED_PASTE)
    });
    session.paste("alpha\r\nbeta\x1b[201~").unwrap();
    wait_for(&session, || exited(&session, 0));
    let expected: String = PASTE.iter().map(|byte| format!("{byte:02x}")).collect();
    assert!(
        screen(&session).contains(&format!("HEX:{expected}")),
        "{}",
        screen(&session)
    );
    wait_for(&session, || session.metrics().active_workers == 0);
}

#[test]
fn conpty_natural_exit_drains_the_final_frame_before_workers_stop() {
    let session = session("burst");
    wait_for(&session, || exited(&session, 0));
    assert!(
        screen(&session).contains("FINAL-OUTPUT"),
        "Final ConPTY output was lost: {}",
        screen(&session)
    );
    wait_for(&session, || session.metrics().active_workers == 0);
}

#[test]
fn conpty_shutdown_interrupts_idle_input_and_a_pending_large_write() {
    let session = session("idle");
    wait_for(&session, || screen(&session).contains("READY"));
    session.write(&vec![b'x'; 512 * 1024]).unwrap();
    let started = Instant::now();
    session.shutdown();
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "Shutdown blocked the UI"
    );
    wait_for(&session, || session.metrics().active_workers == 0);
    assert!(matches!(
        session.metadata().status,
        SessionStatus::Exited { .. }
    ));
    assert!(session.write(b"closed").is_err());
}

/// Executed only in a subprocess created by the tests above. Uses the same
/// Win32 APIs as a native console application instead of simulating its output.
/// <https://learn.microsoft.com/en-us/windows/console/getconsolemode>
/// <https://learn.microsoft.com/en-us/windows/console/getconsolescreenbufferinfo>
#[test]
#[ignore = "fixture invoked inside a real ConPTY by the parent integration tests"]
fn conpty_child_fixture() {
    let scenario = std::env::var(FIXTURE_ENV).expect("ConPTY fixture scenario");
    unsafe {
        let input = GetStdHandle(STD_INPUT_HANDLE);
        let output = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut input_mode = 0;
        let mut output_mode = 0;
        assert_ne!(
            GetConsoleMode(input, &mut input_mode),
            0,
            "{}",
            std::io::Error::last_os_error()
        );
        assert_ne!(
            GetConsoleMode(output, &mut output_mode),
            0,
            "{}",
            std::io::Error::last_os_error()
        );
        input_mode &= !(ENABLE_ECHO_INPUT
            | ENABLE_LINE_INPUT
            | ENABLE_PROCESSED_INPUT
            | ENABLE_QUICK_EDIT_MODE);
        input_mode |= ENABLE_VIRTUAL_TERMINAL_INPUT | ENABLE_EXTENDED_FLAGS;
        output_mode |= ENABLE_VIRTUAL_TERMINAL_PROCESSING | ENABLE_PROCESSED_OUTPUT;
        assert_ne!(
            SetConsoleMode(input, input_mode),
            0,
            "{}",
            std::io::Error::last_os_error()
        );
        assert_ne!(
            SetConsoleMode(output, output_mode),
            0,
            "{}",
            std::io::Error::last_os_error()
        );
    }
    let mut stdout = std::io::stdout().lock();
    if scenario == "paste" {
        stdout.write_all(b"\x1b[?2004h").unwrap();
    }
    stdout.write_all(b"READY\r\n").unwrap();
    stdout.flush().unwrap();
    let mut stdin = std::io::stdin().lock();
    match scenario.as_str() {
        "input" => {
            let mut bytes = [0; 5];
            stdin.read_exact(&mut bytes).unwrap();
            writeln!(stdout, "INPUT:{}\r", String::from_utf8_lossy(&bytes)).unwrap();
            stdout.flush().unwrap();
            std::process::exit(7);
        }
        "resize" => {
            let mut marker = [0];
            stdin.read_exact(&mut marker).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
                assert_ne!(
                    unsafe {
                        GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &mut info)
                    },
                    0
                );
                let width = info.srWindow.Right - info.srWindow.Left + 1;
                let height = info.srWindow.Bottom - info.srWindow.Top + 1;
                if width == 91 && height == 17 {
                    writeln!(stdout, "SIZE:{width}x{height}\r").unwrap();
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "Native console resize was {width}x{height}"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        "paste" => {
            let mut bytes = vec![0; PASTE.len()];
            stdin.read_exact(&mut bytes).unwrap();
            let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            writeln!(stdout, "HEX:{hex}\r").unwrap();
        }
        "burst" => {
            for index in 0..2048 {
                writeln!(
                    stdout,
                    "output-line-{index:04} abcdefghijklmnopqrstuvwxyz\r"
                )
                .unwrap();
            }
            stdout.write_all(b"FINAL-OUTPUT\r\n").unwrap();
        }
        "idle" => std::thread::sleep(Duration::from_secs(60)),
        _ => panic!("Unknown fixture scenario: {scenario}"),
    }
    stdout.flush().unwrap();
    std::process::exit(0);
}

#[test]
fn process_activity_distinguishes_cmd_prompt_from_a_running_child() {
    use terminal_core::ProcessActivity::{Idle, Running};
    let session = TerminalSession::spawn(
        SessionOptions {
            shell: Some("cmd.exe".into()),
            args: vec!["/D".into(), "/Q".into()],
            env: vec![
                (FIXTURE_ENV.into(), "input".into()),
                ("PROMPT".into(), "ACTIVITY_READY$G".into()),
            ],
            ..SessionOptions::default()
        },
        Arc::new(|| {}),
    )
    .unwrap();
    let activity = || {
        session
            .check_process_activity()
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
    };
    wait_for(&session, || screen(&session).contains("ACTIVITY_READY>"));
    // A process the console starts with may outlive the first prompt briefly.
    wait_for(&session, || activity() == Idle);
    let executable = std::env::current_exe().unwrap();
    session
        .write(
            format!(
                "\"{}\" --ignored --exact conpty_child_fixture --nocapture\r",
                executable.display()
            )
            .as_bytes(),
        )
        .unwrap();
    wait_for(&session, || {
        screen(&session).contains("READY\r\n") || screen(&session).contains("READY\n")
    });
    assert_eq!(activity(), Running);
    session.write(b"hello").unwrap();
    wait_for(&session, || activity() == Idle);
    session.shutdown();
    wait_for(&session, || session.metrics().active_workers == 0);
}

#[test]
fn process_activity_marks_direct_programs_running_and_exited_sessions_idle() {
    use terminal_core::ProcessActivity::{Idle, Running};
    let session = session("input");
    wait_for(&session, || screen(&session).contains("READY"));
    assert_eq!(
        session
            .check_process_activity()
            .recv_timeout(Duration::from_secs(3))
            .unwrap(),
        Running
    );
    session.write(b"hello").unwrap();
    wait_for(&session, || exited(&session, 7));
    assert_eq!(
        session
            .check_process_activity()
            .recv_timeout(Duration::from_secs(3))
            .unwrap(),
        Idle
    );
    wait_for(&session, || session.metrics().active_workers == 0);
}

#[test]
fn cmd_and_powershell_report_directory_changes() {
    let target = std::env::var("ProgramFiles").unwrap();
    for (shell, cd) in [
        ("cmd.exe", format!("cd /d \"{target}\"\r")),
        ("powershell.exe", format!("Set-Location '{target}'\r")),
    ] {
        let session = TerminalSession::spawn(
            SessionOptions {
                shell: Some(shell.into()),
                cwd: std::env::temp_dir(),
                ..SessionOptions::default()
            },
            Arc::new(|| {}),
        )
        .unwrap();
        session.write(cd.as_bytes()).unwrap();
        wait_for(&session, || {
            let metadata = session.metadata();
            metadata.reported_cwd == Some(metadata.cwd.clone())
                && metadata.cwd.to_string_lossy().eq_ignore_ascii_case(&target)
        });
        session.shutdown();
        wait_for(&session, || session.metrics().active_workers == 0);
    }
}
