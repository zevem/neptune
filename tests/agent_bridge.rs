//! While Neptune is closed, an agent that kept running is never held up by
//! its hooks, and its Neptune tools say why they do nothing.
#![cfg(unix)]

use std::{
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Runs a private agent mode against the bridge at `socket`, with `input`.
fn run(mode: &[&str], socket: &std::path::Path, input: &str) -> (std::process::Output, Duration) {
    let started = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_neptune"))
        .args(mode)
        .env("NEPTUNE_AGENT_ENDPOINT", socket)
        .env("NEPTUNE_AGENT_TOKEN", "a-terminal-of-an-earlier-launch")
        .env("NEPTUNE_AGENT_RUN", "an-invocation")
        .env_remove("NEPTUNE_AGENT_ROLE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (output, started.elapsed())
}

#[test]
fn while_neptune_is_closed_hooks_give_up_and_tools_say_it_is_not_open() {
    let root = tempfile::tempdir().unwrap();
    // The socket a Neptune that quit or crashed left, with nobody listening,
    // and one that is gone.
    let stale = root.path().join("bridge.sock");
    drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
    let missing = root.path().join("missing.sock");
    for socket in [&stale, &missing] {
        let (hook, took) = run(
            &["--agent-hook", "claude"],
            socket,
            r#"{"hook_event_name":"PreToolUse","tool_name":"Bash"}"#,
        );
        assert!(hook.status.success(), "{socket:?}");
        assert!(hook.stdout.is_empty());
        assert!(took < Duration::from_secs(2), "the hook took {took:?}");

        let calls = [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_agents","arguments":{}}}"#,
        ];
        let (tools, took) = run(&["--agent-mcp"], socket, &(calls.join("\n") + "\n"));
        assert!(tools.status.success(), "{socket:?}");
        assert!(took < Duration::from_secs(3), "the tool call took {took:?}");
        let answer = String::from_utf8(tools.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .find(|message| message["id"] == 2)
            .expect("the tool call is answered");
        assert_eq!(answer["result"]["isError"], true);
        let text = answer["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("Neptune isn't open"), "{text}");
    }
}
