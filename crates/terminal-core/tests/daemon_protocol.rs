//! Pinned session daemon protocol fixtures: a scripted daemon that speaks a
//! released protocol version byte for byte, and the public client attaching
//! to it.
//!
//! Each released version has a module here. A released version's frames
//! never change, and its module stays for as long as Neptune attaches to
//! daemons of that version, so a client that drifts from what those daemons
//! send and expect fails here. A new version adds its own module with its own
//! frames; it doesn't edit an earlier one.
#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use terminal_core::daemon::{DaemonClient, PROTOCOL_VERSION, SessionEntry, SessionId};
use terminal_core::{SessionOptions, SessionStatus};

mod support;
use support::Frame;

/// What a scripted connection does next.
enum Step {
    /// Read exactly these bytes from the client.
    Expect(Vec<u8>),
    Send(Vec<u8>),
    /// The client closes the connection, sending nothing more.
    Closed,
    /// Wait until the test says to go on.
    Await(std::sync::mpsc::Receiver<()>),
}

/// A daemon of one protocol version that plays a script per connection, in
/// the order the client connects. Every step must happen as written.
struct Scripted {
    root: tempfile::TempDir,
    connections: Vec<JoinHandle<()>>,
    listener: Option<JoinHandle<Vec<JoinHandle<()>>>>,
}

impl Scripted {
    fn serve(version: u32, scripts: Vec<Vec<Step>>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("sessions");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let listener = UnixListener::bind(support::socket(&dir, version)).unwrap();
        let listener = std::thread::spawn(move || {
            scripts
                .into_iter()
                .map(|script| {
                    let (stream, _) = listener.accept().unwrap();
                    std::thread::spawn(move || play(stream, script))
                })
                .collect()
        });
        Self {
            root,
            connections: Vec::new(),
            listener: Some(listener),
        }
    }

    fn dir(&self) -> PathBuf {
        self.root.path().join("sessions")
    }

    /// Every connection played its whole script.
    fn finish(mut self) {
        self.connections = self.listener.take().unwrap().join().unwrap();
        for connection in self.connections.drain(..) {
            connection.join().unwrap();
        }
    }
}

fn play(mut stream: UnixStream, script: Vec<Step>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    for (index, step) in script.into_iter().enumerate() {
        match step {
            Step::Expect(expected) => {
                let mut received = vec![0; expected.len()];
                stream
                    .read_exact(&mut received)
                    .unwrap_or_else(|error| panic!("step {index}: {error}"));
                assert_eq!(received, expected, "step {index}");
            }
            Step::Send(bytes) => stream.write_all(&bytes).unwrap(),
            Step::Await(go) => go.recv().unwrap(),
            Step::Closed => {
                let mut rest = Vec::new();
                match stream.read_to_end(&mut rest) {
                    Ok(_) => {}
                    // A client closing with what it only peeked at unread
                    // resets the connection.
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
                    Err(error) => panic!("step {index}: {error}"),
                }
                assert_eq!(rest, [], "step {index}: the client sent more");
            }
        }
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

fn id(text: &str) -> SessionId {
    text.parse().unwrap()
}

/// Protocol version 1, the first released version.
mod v1 {
    use super::*;
    use Step::{Await, Closed, Expect, Send};

    const VERSION: u32 = 1;

    fn hello() -> Vec<u8> {
        support::hello(VERSION)
    }
    fn welcome() -> Vec<u8> {
        support::welcome(VERSION)
    }
    fn done() -> Vec<u8> {
        Frame::new(67).done()
    }

    /// A snapshot of a 20x4 terminal with one line of history: styled text,
    /// a beam cursor after the prompt, a changed palette colour, a title, a
    /// reported directory and a prompt mark, and the first two bytes of a
    /// character the output stream completes.
    fn snapshot(frame: Frame) -> Frame {
        let cursor = |frame: Frame, line: u16, column: u16, style: u32| {
            frame.u16(line).u16(column).u8(0).u32(style)
        };
        // Size, then no hyperlinks.
        let frame = frame.u16(20).u16(4).u32(0);
        // Styles: the default, then bold red text.
        let frame = frame
            .u32(2)
            .u8(0)
            .u16(256)
            .u8(0)
            .u16(257)
            .u8(0)
            .u16(0)
            .u8(0)
            .u8(2)
            .u8(255)
            .u8(0)
            .u8(0)
            .u8(0)
            .u16(257)
            .u8(0)
            .u16(0b10)
            .u8(0);
        // The primary screen: keyboard flags, cursor, saved cursor, lines.
        let frame = cursor(frame.u32(0), 1, 7, 0);
        let frame = cursor(frame, 0, 0, 0).u32(5);
        let line = |frame: Frame, runs: &[(u32, &str)]| {
            let mut frame = frame.u32(runs.len() as u32);
            for (style, text) in runs {
                frame = frame.u32(*style).str(text);
            }
            // No combining characters.
            frame.u32(0)
        };
        let frame = line(frame, &[(0, "old line")]);
        let frame = line(frame, &[(1, "red"), (0, " text")]);
        let frame = line(frame, &[(0, "prompt$")]);
        let frame = line(frame, &[]);
        let frame = line(frame, &[]);
        // No alternate screen; modes: cursor shown, line wrap.
        let frame = frame.u8(0).u32(1 | 1 << 7);
        // A blinking beam cursor, then palette colour 1 changed.
        let frame = frame.u8(2).u8(1).u32(1).u16(1).u8(1).u8(2).u8(3);
        frame
            .str("pinned title")
            .u8(1)
            .bytes(b"/tmp/pinned")
            // No remote process; the prompt starts the third line.
            .u8(0)
            .u8(1)
            .u32(2)
            .u16(0)
            .u8(0)
            // Pending: the start of "€".
            .bytes(&[0xe2, 0x82])
    }

    fn attached(id: u64) -> Vec<u8> {
        let frame = Frame::new(69)
            .u64(id)
            // No process id.
            .u8(0)
            .str("/bin/sh")
            .bytes(b"/tmp")
            .u32(100)
            .bytes(b"{\"kept\":1}");
        snapshot(frame).done()
    }

    fn output(bytes: &[u8]) -> Vec<u8> {
        Frame::new(70).bytes(bytes).done()
    }

    #[test]
    fn a_client_lists_keeps_data_with_and_ends_sessions_of_a_version_1_daemon() {
        let failure = Frame::new(68)
            .u8(9)
            .str("The session daemon has no such session")
            .done();
        let daemon = Scripted::serve(
            VERSION,
            vec![vec![
                Expect(hello()),
                Send(welcome()),
                Expect(Frame::new(2).done()),
                Send(
                    Frame::new(66)
                        .u32(3)
                        .u64(0xa1)
                        .u8(0)
                        .bytes(b"{\"kept\":1}")
                        .u64(0xa2)
                        .u8(1)
                        .u32(3)
                        .u8(0)
                        .bytes(b"")
                        .u64(0xa3)
                        .u8(1)
                        .u32(0)
                        .u8(1)
                        .str("SIGKILL")
                        .bytes(b"")
                        .done(),
                ),
                Expect(Frame::new(3).u64(0xa1).bytes(b"{\"kept\":2}").done()),
                Send(done()),
                Expect(Frame::new(4).u64(0xa2).done()),
                Send(done()),
                Expect(Frame::new(4).u64(0xa4).done()),
                Send(failure),
                // End all.
                Expect(Frame::new(8).done()),
                Send(done()),
                Closed,
            ]],
        );

        let client = DaemonClient::connect_to(&daemon.dir(), VERSION).unwrap();
        assert_eq!(client.version(), VERSION);
        assert_eq!(
            client.list().unwrap(),
            vec![
                SessionEntry {
                    id: id("00000000000000a1"),
                    status: SessionStatus::Running,
                    data: b"{\"kept\":1}".to_vec(),
                },
                SessionEntry {
                    id: id("00000000000000a2"),
                    status: SessionStatus::Exited {
                        code: 3,
                        signal: None
                    },
                    data: Vec::new(),
                },
                SessionEntry {
                    id: id("00000000000000a3"),
                    status: SessionStatus::Exited {
                        code: 0,
                        signal: Some("SIGKILL".into())
                    },
                    data: Vec::new(),
                },
            ]
        );
        client
            .set_data(id("00000000000000a1"), b"{\"kept\":2}")
            .unwrap();
        client.close(id("00000000000000a2")).unwrap();
        let error = client.close(id("00000000000000a4")).unwrap_err();
        assert_eq!(error.kind(), terminal_core::SessionErrorKind::NotFound);
        assert_eq!(error.to_string(), "The session daemon has no such session");
        client.end_all().unwrap();
        drop(client);
        daemon.finish();
    }

    #[test]
    fn a_client_attaches_to_a_version_1_session_and_streams_it_until_it_ends() {
        let daemon = Scripted::serve(
            VERSION,
            vec![
                // The client's control connection.
                vec![Expect(hello()), Send(welcome()), Closed],
                // The session's stream.
                vec![
                    Expect(hello()),
                    Send(welcome()),
                    Expect(Frame::new(6).u64(0xa1).u16(20).u16(4).done()),
                    Send(attached(0xa1)),
                    Send(output(&[0xac, b' ', b'm', b'o', b'r', b'e'])),
                    Expect(Frame::new(16).bytes(b"typed").done()),
                    Expect(Frame::new(17).u16(30).u16(5).u16(300).u16(100).done()),
                    Expect(Frame::new(18).u8(1).done()),
                    // The history limit, then palette colour 1.
                    Expect(Frame::new(21).u32(50).done()),
                    Expect(Frame::new(22).u16(1).u8(4).u8(5).u8(6).done()),
                    // Ended with code 7 and no signal.
                    Send(Frame::new(71).u8(1).u32(7).u8(0).done()),
                    // The client shows the exit, so the daemon may let it go.
                    Expect(Frame::new(20).done()),
                    Closed,
                ],
            ],
        );

        let client = DaemonClient::connect_to(&daemon.dir(), VERSION).unwrap();
        let attached = client
            .attach(id("00000000000000a1"), 20, 4, repaint())
            .unwrap();
        drop(client);
        assert_eq!(attached.data, b"{\"kept\":1}");
        let session = attached.session;
        wait_for(|| session.screen_text().contains("prompt$€ more"));
        let screen = session.viewport();
        assert_eq!(
            session.screen_text().lines().next(),
            Some("red text"),
            "{:?}",
            session.screen_text()
        );
        assert_eq!(
            screen.rows[0][0].fg,
            terminal_core::Color::Spec(terminal_core::Rgb { r: 255, g: 0, b: 0 })
        );
        assert!(screen.rows[0][0].flags.contains(terminal_core::Flags::BOLD));
        assert_eq!(screen.cursor.shape, terminal_core::CursorShape::Beam);
        assert_eq!(
            screen.colors[1],
            Some(terminal_core::Rgb { r: 1, g: 2, b: 3 })
        );
        assert!(session.history_text().starts_with("old line"));
        let metadata = session.metadata();
        assert_eq!(metadata.title, "pinned title");
        assert_eq!(metadata.shell, "/bin/sh");
        assert_eq!(metadata.reported_cwd, Some(PathBuf::from("/tmp/pinned")));

        session.write(b"typed").unwrap();
        session.resize(30, 5, 300, 100).unwrap();
        session.focus(true).unwrap();
        session.set_scrollback(50).unwrap();
        session.set_color(1, terminal_core::Rgb { r: 4, g: 5, b: 6 });
        wait_for(|| {
            session.metadata().status
                == SessionStatus::Exited {
                    code: 7,
                    signal: None,
                }
        });
        session.shutdown();
        wait_for(|| session.shutdown_completion().is_complete());
        drop(session);
        daemon.finish();
    }

    #[test]
    fn a_client_starts_a_session_in_a_version_1_daemon() {
        let cwd = tempfile::tempdir().unwrap();
        let cwd_bytes = cwd.path().as_os_str().as_encoded_bytes().to_vec();
        let daemon = Scripted::serve(
            VERSION,
            vec![
                vec![Expect(hello()), Send(welcome()), Closed],
                vec![
                    Expect(hello()),
                    Send(welcome()),
                    Expect(
                        Frame::new(5)
                            .bytes(&cwd_bytes)
                            .u16(20)
                            .u16(4)
                            .u32(100)
                            .u8(1)
                            .str("/bin/sh")
                            .u32(2)
                            .str("-c")
                            .str("exec cat")
                            .u32(1)
                            .str("PINNED")
                            .str("1")
                            .done(),
                    ),
                    Send(attached(0xb7)),
                    Expect(Frame::new(19).done()),
                    Closed,
                ],
            ],
        );

        let client = DaemonClient::connect_to(&daemon.dir(), VERSION).unwrap();
        let (session, started) = client
            .start(
                SessionOptions {
                    cwd: cwd.path().into(),
                    cols: 20,
                    rows: 4,
                    scrollback: 100,
                    shell: Some("/bin/sh".into()),
                    args: vec!["-c".into(), "exec cat".into()],
                    env: vec![("PINNED".into(), "1".into())],
                },
                repaint(),
            )
            .unwrap();
        assert_eq!(started, id("00000000000000b7"));
        wait_for(|| session.screen_text().contains("prompt$"));
        // Closing a running session ends it in the daemon.
        session.shutdown();
        wait_for(|| session.shutdown_completion().is_complete());
        drop((session, client));
        daemon.finish();
    }

    #[test]
    fn a_client_takes_over_a_version_1_daemon_and_hears_when_a_newer_one_does() {
        let (go, wait) = std::sync::mpsc::channel();
        let daemon = Scripted::serve(
            VERSION,
            vec![
                vec![Expect(hello()), Send(welcome()), Closed],
                vec![
                    Expect(hello()),
                    Send(welcome()),
                    Expect(Frame::new(7).done()),
                    Send(done()),
                    Await(wait),
                    // Replaced: a newer client took over.
                    Send(Frame::new(72).done()),
                    Closed,
                ],
            ],
        );

        let client = DaemonClient::connect_to(&daemon.dir(), VERSION).unwrap();
        let (told, heard) = std::sync::mpsc::channel();
        let told = std::sync::Mutex::new(told);
        let takeover = client
            .take_over(Arc::new(move || told.lock().unwrap().send(()).unwrap()))
            .unwrap();
        drop(client);
        assert!(takeover.held());
        assert!(!takeover.replaced());
        go.send(()).unwrap();
        heard.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(takeover.replaced());
        assert!(!takeover.held());
        drop(takeover);
        daemon.finish();
    }
}

/// Not a version's fixture: no client attaches to a version that isn't
/// released yet, or to none.
#[test]
fn a_client_reaches_only_daemons_of_versions_it_attaches_to() {
    let root = tempfile::tempdir().unwrap();
    for version in [0, PROTOCOL_VERSION + 1] {
        let error = DaemonClient::connect_to(root.path(), version)
            .err()
            .expect("connected to an unknown version");
        assert_eq!(error.kind(), terminal_core::SessionErrorKind::Incompatible);
    }
}
