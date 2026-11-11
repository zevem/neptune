//! A quiet session's workers wait for work instead of polling. This target
//! holds one test, so every `terminal-*` thread of the process is its own.
#![cfg(target_os = "linux")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use terminal_core::{SessionOptions, SessionStatus, TerminalSession};

/// Times this process's session workers have blocked, which is once per wakeup.
fn worker_wakeups() -> u64 {
    let mut total = 0;
    for task in std::fs::read_dir("/proc/self/task").unwrap() {
        let task = task.unwrap().path();
        let Ok(name) = std::fs::read_to_string(task.join("comm")) else {
            continue;
        };
        if !name.starts_with("terminal-") {
            continue;
        }
        let status = std::fs::read_to_string(task.join("status")).unwrap_or_default();
        total += status
            .lines()
            .find_map(|line| line.strip_prefix("voluntary_ctxt_switches:"))
            .map_or(0, |count| count.trim().parse::<u64>().unwrap());
    }
    total
}

#[track_caller]
fn wait_for(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "PTY condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_quiet_session_never_wakes_its_workers_and_still_stops_and_exits_promptly() {
    let spawn = |script: &str| {
        TerminalSession::spawn(
            SessionOptions {
                shell: Some("/bin/sh".into()),
                args: vec!["-c".into(), script.into()],
                ..SessionOptions::default()
            },
            Arc::new(|| {}),
        )
        .unwrap()
    };
    let closed = spawn("printf READY; sleep 30");
    let exiting = spawn("printf READY; IFS= read -r line; exit 7");
    for session in [&closed, &exiting] {
        wait_for(|| session.screen_text().contains("READY"));
        wait_for(|| session.metrics().active_workers == 3);
    }
    // The directory read that follows the output above is the last deadline.
    std::thread::sleep(Duration::from_millis(1500));
    let before = worker_wakeups();
    std::thread::sleep(Duration::from_secs(2));
    let wakeups = worker_wakeups() - before;
    assert_eq!(wakeups, 0, "six quiet workers woke {wakeups} times in 2 s");

    // Nothing polls, so each of these must reach the waiting workers.
    let started = Instant::now();
    closed.shutdown();
    wait_for(|| closed.metrics().active_workers == 0);
    exiting.write(b"go\n").unwrap();
    wait_for(|| exiting.metrics().active_workers == 0);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "Stop and exit took {:?}",
        started.elapsed()
    );
    assert_eq!(
        exiting.metadata().status,
        SessionStatus::Exited {
            code: 7,
            signal: None
        }
    );
}
