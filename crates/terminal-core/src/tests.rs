use super::*;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::Color;

fn wait_for(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "Session condition timed out");
        thread::sleep(Duration::from_millis(10));
    }
}
#[cfg(unix)]
fn screen(session: &TerminalSession) -> String {
    session.screen_text()
}

#[cfg(unix)]
#[test]
fn kernel_resize_waits_until_the_parser_grid_lock_is_available() {
    let directory = tempfile::tempdir().unwrap();
    let probe_file = directory.path().join("observed-size");
    let session = Arc::new(
        TerminalSession::spawn(
            SessionOptions {
                shell: Some("/bin/sh".into()),
                args: vec![
                    "-c".into(),
                    "printf 'READY\\r\\n'; while IFS= read -r probe; do stty size > \"$1\"; done"
                        .into(),
                    "resize-probe".into(),
                    probe_file.to_string_lossy().into_owned(),
                ],
                cols: 80,
                rows: 12,
                scrollback: 128,
                ..SessionOptions::default()
            },
            Arc::new(|| {}),
        )
        .unwrap(),
    );
    wait_for(|| screen(&session).contains("READY"));

    // A renderer can briefly hold this lock while a different thread requests
    // resize. The kernel must keep the old geometry until that request owns the
    // grid lock, otherwise SIGWINCH output can overtake the VT grid resize.
    let grid = session.lock();
    let resizing = session.clone();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let resize = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        resizing.resize(91, 17, 910, 340)
    });
    started_rx.recv().unwrap();
    std::thread::sleep(Duration::from_millis(50));
    session.write(b"probe\r").unwrap();
    wait_for(|| std::fs::read_to_string(&probe_file).is_ok_and(|size| !size.trim().is_empty()));
    let observed_while_locked = std::fs::read_to_string(&probe_file).unwrap();
    drop(grid);
    resize.join().unwrap().unwrap();
    assert_eq!(
        observed_while_locked.trim(),
        "12 80",
        "The kernel changed size before the parser grid lock was available"
    );

    std::fs::remove_file(&probe_file).unwrap();
    session.write(b"probe\r").unwrap();
    wait_for(|| std::fs::read_to_string(&probe_file).is_ok_and(|size| size.trim() == "17 91"));
    assert_eq!(session.lock().columns(), 91);
    assert_eq!(session.lock().screen_lines(), 17);
    session.shutdown();
    wait_for(|| session.metrics().active_workers == 0);
}

fn terminal(scrollback: usize) -> Term<VoidListener> {
    let size = Size {
        cols: 12,
        rows: 4,
        pixel_width: 0,
        pixel_height: 0,
    };
    Term::new(
        Config {
            scrolling_history: scrollback,
            kitty_keyboard: true,
            ..Config::default()
        },
        &size,
        VoidListener,
    )
}

fn advance_prompt(
    terminal: &mut Term<VoidListener>,
    parser: &mut Processor,
    scanner: &mut PromptScanner,
    prompt: &mut PromptState,
    bytes: &[u8],
) {
    let mut offset = 0;
    for (end, mark) in scanner.advance(bytes) {
        parser.advance(terminal, &bytes[offset..end]);
        prompt.mark(terminal, mark);
        offset = end;
    }
    parser.advance(terminal, &bytes[offset..]);
}

#[test]
fn semantic_prompt_scanner_handles_fragmentation_options_reset_and_bounded_osc() {
    let mut scanner = PromptScanner::default();
    let mut marks = Vec::new();
    for byte in b"\x1b]133;A;redraw=0\x1b\\text\x1b]133;P;k=r\x07\x1b]133;B\x07\x1b]133;C\x07\x1b]133;A;redraw=1\x07\x1bc" {
            marks.extend(scanner.advance(&[*byte]).into_iter().map(|(_, mark)| mark));
        }
    assert_eq!(
        marks,
        vec![
            PromptMark::Start {
                redraw: Some(false)
            },
            PromptMark::Output,
            PromptMark::Start { redraw: Some(true) },
            PromptMark::Output,
        ]
    );
    assert!(scanner.advance(b"\x1b]133;").is_empty());
    assert!(scanner.advance(&vec![b'x'; 100_000]).is_empty());
    assert!(scanner.bytes.len() <= 256);
    assert_eq!(
        scanner.advance(b"\x07\x1b]133;A\x07"),
        vec![(9, PromptMark::Start { redraw: None })]
    );
}

#[test]
fn active_prompt_row_follows_scroll_at_the_history_cap_and_survives_height_shrink() {
    let mut terminal = terminal(10_000);
    terminal.resize(Size {
        cols: 80,
        rows: 4,
        pixel_width: 0,
        pixel_height: 0,
    });
    let mut parser: Processor = Processor::new();
    let mut scanner = PromptScanner::default();
    let mut prompt = PromptState::default();
    for line in 0..10_050 {
        parser.advance(&mut terminal, format!("kept-{line:05}\r\n").as_bytes());
    }
    assert_eq!(terminal.history_size(), 10_000);
    assert_eq!(terminal.grid().cursor.point.line, Line(3));
    advance_prompt(
        &mut terminal,
        &mut parser,
        &mut scanner,
        &mut prompt,
        b"\x1b]133;A\x07LEFT\x1b[70GRIGHT\r\n> \x1b]133;B\x07",
    );
    let identity = prompt.anchor.as_ref().unwrap().row_identity;
    assert_eq!(row_identity(&terminal, Line(2)), identity);
    assert_eq!(terminal.history_size(), 10_000);
    prompt.resize(
        &mut terminal,
        Size {
            cols: 33,
            rows: 3,
            pixel_width: 0,
            pixel_height: 0,
        },
    );
    let content = terminal.bounds_to_string(
        Point::new(Line(0), Column(0)),
        Point::new(terminal.bottommost_line(), terminal.last_column()),
    );
    assert!(content.contains("kept-10049"), "{content}");
    assert!(
        !content.contains("LEFT") && !content.contains("RIGHT"),
        "{content}"
    );
    assert_eq!(terminal.grid().cursor.point.line, Line(2));
    assert_eq!(
        prompt.anchor.as_ref().unwrap().row_identity,
        row_identity(&terminal, Line(1))
    );
}

#[test]
fn nonredrawable_prompts_output_and_alternate_screen_are_preserved() {
    for ending in [
        b"\x1b]133;C\x07".as_slice(),
        b"\x1b[?1049h".as_slice(),
        b"\x1bc".as_slice(),
    ] {
        let mut terminal = terminal(16);
        let mut parser: Processor = Processor::new();
        let mut scanner = PromptScanner::default();
        let mut prompt = PromptState::default();
        advance_prompt(
            &mut terminal,
            &mut parser,
            &mut scanner,
            &mut prompt,
            b"\x1b]133;A\x07prompt",
        );
        advance_prompt(
            &mut terminal,
            &mut parser,
            &mut scanner,
            &mut prompt,
            ending,
        );
        parser.advance(&mut terminal, b"VISIBLE");
        prompt.resize(
            &mut terminal,
            Size {
                cols: 16,
                rows: 4,
                pixel_width: 0,
                pixel_height: 0,
            },
        );
        let content = terminal.bounds_to_string(
            Point::new(Line(0), Column(0)),
            Point::new(terminal.bottommost_line(), terminal.last_column()),
        );
        assert!(content.contains("VISIBLE"), "{content}");
    }
    let mut terminal = terminal(16);
    let mut parser: Processor = Processor::new();
    let mut scanner = PromptScanner::default();
    let mut prompt = PromptState::default();
    advance_prompt(
        &mut terminal,
        &mut parser,
        &mut scanner,
        &mut prompt,
        b"\x1b]133;A;redraw=0\x07KEEP",
    );
    prompt.resize(
        &mut terminal,
        Size {
            cols: 16,
            rows: 4,
            pixel_width: 0,
            pixel_height: 0,
        },
    );
    assert_eq!(terminal.grid()[Point::new(Line(0), Column(0))].c, 'K');
}

#[test]
fn fragmented_utf8_truecolor_and_wide_cells_are_preserved() {
    let mut terminal = terminal(16);
    let mut parser: Processor = Processor::new();
    let input = "\x1b[38;2;91;166;201m界e\u{301}\x1b[0m";
    for byte in input.as_bytes() {
        parser.advance(&mut terminal, &[*byte]);
    }
    let grid = terminal.grid();
    assert_eq!(grid[Point::new(Line(0), Column(0))].c, '界');
    assert!(
        grid[Point::new(Line(0), Column(0))]
            .flags
            .contains(Flags::WIDE_CHAR)
    );
    assert!(
        grid[Point::new(Line(0), Column(1))]
            .flags
            .contains(Flags::WIDE_CHAR_SPACER)
    );
    assert_eq!(
        grid[Point::new(Line(0), Column(0))].fg,
        Color::Spec(Rgb {
            r: 91,
            g: 166,
            b: 201
        })
    );
    assert_eq!(grid[Point::new(Line(0), Column(2))].c, 'e');
    assert_eq!(
        grid[Point::new(Line(0), Column(2))].zerowidth(),
        Some(&['\u{301}'][..])
    );
    assert_eq!(grid.cursor.point.column, Column(3));
}

#[test]
fn alternate_screen_restores_primary_content_and_cursor() {
    let mut terminal = terminal(16);
    let mut parser: Processor = Processor::new();
    parser.advance(&mut terminal, b"primary\x1b[?1049h\x1b[Halternate");
    assert!(terminal.mode().contains(TermMode::ALT_SCREEN));
    assert_eq!(terminal.grid()[Point::new(Line(0), Column(0))].c, 'a');
    parser.advance(&mut terminal, b"\x1b[?1049l");
    assert!(!terminal.mode().contains(TermMode::ALT_SCREEN));
    assert_eq!(terminal.grid()[Point::new(Line(0), Column(0))].c, 'p');
    assert_eq!(terminal.grid().cursor.point.column, Column(7));
}

#[test]
fn scrolling_history_is_bounded_and_runtime_reduction_is_applied() {
    let mut terminal = terminal(8);
    let mut parser: Processor = Processor::new();
    parser.advance(&mut terminal, "line\r\n".repeat(100).as_bytes());
    assert_eq!(terminal.history_size(), 8);
    terminal.set_options(Config {
        scrolling_history: 3,
        ..Config::default()
    });
    assert_eq!(terminal.history_size(), 3);
    assert_eq!(terminal.total_lines(), 7);
}

#[test]
fn incomplete_synchronized_update_can_be_flushed_at_timeout() {
    let mut terminal = terminal(8);
    let mut parser: Processor = Processor::new();
    parser.advance(&mut terminal, b"\x1b[?2026hbuffered");
    assert_eq!(terminal.grid()[Point::new(Line(0), Column(0))].c, ' ');
    assert!(parser.sync_timeout().sync_timeout().is_some());
    parser.stop_sync(&mut terminal);
    assert_eq!(terminal.grid()[Point::new(Line(0), Column(0))].c, 'b');
    assert!(parser.sync_timeout().sync_timeout().is_none());
}

#[test]
fn kitty_keyboard_modes_activate_and_the_stack_restores_previous_flags() {
    let mut terminal = terminal(8);
    let mut parser: Processor = Processor::new();
    parser.advance(&mut terminal, b"\x1b[>1u");
    assert!(terminal.mode().contains(TermMode::DISAMBIGUATE_ESC_CODES));
    assert!(!terminal.mode().contains(TermMode::REPORT_ALL_KEYS_AS_ESC));
    parser.advance(&mut terminal, b"\x1b[>31u");
    assert!(terminal.mode().contains(TermMode::KITTY_KEYBOARD_PROTOCOL));
    parser.advance(&mut terminal, b"\x1b[<u");
    assert!(terminal.mode().contains(TermMode::DISAMBIGUATE_ESC_CODES));
    assert!(!terminal.mode().contains(TermMode::REPORT_EVENT_TYPES));
    parser.advance(&mut terminal, b"\x1b[<u");
    assert!(
        !terminal
            .mode()
            .intersects(TermMode::KITTY_KEYBOARD_PROTOCOL)
    );
}

#[test]
fn osc7_survives_fragmentation_and_rejects_invalid_uris() {
    let mut tracker = CwdTracker::default();
    assert!(
        tracker
            .advance(b"normal text\x1b]0;title\x07\x1b]7;file://local")
            .is_none()
    );
    assert!(tracker.advance(b"host/home/dev/my%20project\x1b").is_none());
    assert_eq!(
        tracker.advance(b"\\").map(|report| report.path),
        Some(PathBuf::from("/home/dev/my project"))
    );
    assert!(decode_cwd(b"file://host/tmp/%00").is_none());
    assert!(decode_cwd(b"file://host/tmp/%GG").is_none());
    assert!(decode_cwd(b"https://host/path").is_none());
}

#[test]
fn osc7_from_another_host_is_not_a_local_directory() {
    let local = |uri: &str| decode_cwd(uri.as_bytes()).map(|report| report.local);
    assert_eq!(local("file:///home/dev"), Some(true));
    assert_eq!(local("file://localhost/home/dev"), Some(true));
    assert_eq!(local("file://LocalHost/home/dev"), Some(true));
    // A shell reached over SSH, and Neptune's own SSH bootstrap.
    assert_eq!(
        local("file://devbox-that-is-not-here/home/dev"),
        Some(false)
    );
    assert_eq!(local("file://neptune/home/dev"), Some(false));
    if let Some(name) = local_hostname() {
        assert_eq!(local(&format!("file://{name}/home/dev")), Some(true));
        let upper = name.to_ascii_uppercase();
        assert_eq!(local(&format!("file://{upper}/home/dev")), Some(true));
        // `hostname -s` and `$HOST` can disagree about the domain.
        let short = name.split('.').next().unwrap();
        assert_eq!(local(&format!("file://{short}.local/home/dev")), Some(true));
        assert_eq!(local(&format!("file://{short}/home/dev")), Some(true));
    }
}

#[test]
fn oversized_osc7_has_bounded_storage_and_recovers() {
    let mut tracker = CwdTracker::default();
    let mut input = b"\x1b]7;file://host/".to_vec();
    input.extend(vec![b'x'; MAX_OSC * 2]);
    input.extend_from_slice(b"\x07\x1b]7;file://host/tmp\x07");
    assert_eq!(
        tracker.advance(&input).map(|report| report.path),
        Some(PathBuf::from("/tmp"))
    );
    assert!(tracker.bytes.capacity() <= MAX_OSC);
}

#[test]
fn remote_shell_pid_survives_fragmentation_without_becoming_a_directory() {
    let mut tracker = CwdTracker::default();
    for byte in b"\x1b]777;neptune;pid;12345\x1b\\" {
        assert!(tracker.advance(&[*byte]).is_none());
    }
    assert_eq!(tracker.remote_pid.take(), Some(12345));
    for invalid in ["0", "1", "-2", "12;other", "4294967296"] {
        tracker.advance(format!("\x1b]777;neptune;pid;{invalid}\x07").as_bytes());
        assert_eq!(tracker.remote_pid.take(), None);
    }
}

#[test]
fn osc9_9_reports_native_paths_and_ignores_other_osc9() {
    let dir = if cfg!(windows) {
        r"C:\Work dir"
    } else {
        "/work dir"
    };
    let mut tracker = CwdTracker::default();
    assert!(
        tracker
            .advance(b"\x1b]9;4;1;50\x07\x1b]9;Done\x07\x1b]9;9")
            .is_none()
    );
    let local = |path: &str| {
        Some(CwdReport {
            path: PathBuf::from(path),
            local: true,
        })
    };
    assert_eq!(
        tracker.advance(format!(";\"{dir}\"\x1b\\").as_bytes()),
        local(dir)
    );
    assert_eq!(
        tracker.advance(format!("\x1b]9;9;{dir}\x07").as_bytes()),
        local(dir)
    );
    assert!(decode_path(b"relative").is_none());
    assert!(decode_path(b"/tmp\0").is_none());
    if cfg!(windows) {
        // Output must not steer later splits/restores onto a network share.
        for report in [
            &b"\x1b]9;9;\\\\evil\\share\x07"[..],
            b"\x1b]9;9;\\\\?\\UNC\\evil\\share\x07",
            b"\x1b]7;file://x//evil/share\x07",
        ] {
            assert!(tracker.advance(report).is_none());
        }
        assert_eq!(
            tracker
                .advance(b"\x1b]7;file://neptune/home/dev\x07")
                .map(|report| report.path),
            Some(PathBuf::from("/home/dev"))
        );
    }
}

/// A parser-backed fixture exercises the public contract without PTYs,
/// platform shells, fonts or a GUI. Backend mutation remains internal.
fn fixture(cols: u16, rows: u16, history: usize) -> TerminalSession {
    let size = Size {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    };
    let config = Config {
        scrolling_history: history,
        kitty_keyboard: true,
        ..Config::default()
    };
    let (input, _) = mpsc::sync_channel(INPUT_QUEUE);
    let (output, _) = mpsc::sync_channel(OUTPUT_QUEUE);
    let shared = Arc::new(
        Shared::new(
            SessionMetadata {
                title: String::new(),
                shell: "fake".into(),
                cwd: PathBuf::from("."),
                reported_cwd: None,
                process_id: None,
                remote_process_id: None,
                status: SessionStatus::Running,
                bell_count: 0,
            },
            size,
            config.clone(),
            Arc::new(|| {}),
            output,
            input.clone(),
        )
        .unwrap(),
    );
    let proxy = EventProxy {
        shared: shared.clone(),
        input: input.clone(),
    };
    TerminalSession {
        terminal: Arc::new(Mutex::new(Term::new(config, &size, proxy))),
        shared,
        input,
        viewport_cache: Mutex::new(None),
    }
}

/// What a session sent to its transport, in order.
#[derive(Debug, PartialEq, Eq)]
enum Sent {
    Input(Vec<u8>),
    Reply(Vec<u8>),
    Resize(u16, u16),
    Release,
    Terminate,
}

struct FakeSource(mpsc::Receiver<Vec<u8>>);
impl transport::Source for FakeSource {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.0.recv() {
            Ok(bytes) => {
                buffer[..bytes.len()].copy_from_slice(&bytes);
                Ok(bytes.len())
            }
            Err(_) => Ok(0),
        }
    }
    fn wait_readable(&mut self, _: &Shared) -> io::Result<bool> {
        Ok(false)
    }
}

struct FakeSink(mpsc::Sender<Sent>);
impl transport::Sink for FakeSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let _ = self.0.send(Sent::Input(bytes.to_vec()));
        Ok(bytes.len())
    }
    fn wait_writable(&mut self, _: &Shared) -> io::Result<bool> {
        Ok(false)
    }
    fn resize(&mut self, size: Size) -> Result<()> {
        let _ = self.0.send(Sent::Resize(size.cols, size.rows));
        Ok(())
    }
    fn reply(&mut self, bytes: &[u8], _: &Shared) -> Result<()> {
        let _ = self.0.send(Sent::Reply(bytes.to_vec()));
        Ok(())
    }
}

struct FakeProcess(mpsc::Sender<Sent>);
impl transport::Process for FakeProcess {
    fn try_wait(&mut self) -> io::Result<Option<transport::Exit>> {
        Ok(None)
    }
    fn terminate(&mut self) -> Option<transport::Exit> {
        let _ = self.0.send(Sent::Terminate);
        Some(transport::Exit {
            code: 1,
            signal: Some("Hangup".into()),
        })
    }
    fn activity(&self, _: u32) -> ProcessActivity {
        ProcessActivity::Unknown
    }
    fn release(&mut self, _: &Arc<Shared>) {
        let _ = self.0.send(Sent::Release);
    }
}

/// A session engine fed by a fake transport: the test writes the program's
/// output and observes everything the engine sends back.
fn fake_session(cols: u16, rows: u16) -> (mpsc::Sender<Vec<u8>>, Receiver<Sent>, TerminalSession) {
    let (output, source) = mpsc::channel();
    let (sent_tx, sent) = mpsc::channel();
    let transport = transport::Transport {
        source: Box::new(FakeSource(source)),
        sink: Box::new(FakeSink(sent_tx.clone())),
        process: Box::new(FakeProcess(sent_tx)),
        shell: "fake".into(),
        process_id: None,
        replica: None,
    };
    let size = Size {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    };
    let session =
        TerminalSession::start(transport, PathBuf::from("."), size, 16, Arc::new(|| {})).unwrap();
    (output, sent, session)
}

fn next_sent(sent: &Receiver<Sent>) -> Sent {
    sent.recv_timeout(Duration::from_secs(5))
        .expect("the session sent nothing to its transport")
}

#[test]
fn engine_replies_input_and_resize_all_go_through_the_transport() {
    let (output, sent, session) = fake_session(12, 4);

    // A cursor-position query is answered by the engine, not typed by the user.
    output.send(b"\x1b[6n".to_vec()).unwrap();
    assert_eq!(next_sent(&sent), Sent::Reply(b"\x1b[1;1R".to_vec()));
    // So is a notification capability query.
    output.send(b"\x1b]99;i=7:p=?;\x1b\\".to_vec()).unwrap();
    assert!(matches!(
        next_sent(&sent),
        Sent::Reply(reply) if reply.starts_with(b"\x1b]99;i=7:p=?;")
    ));
    // And the focus state a program receives when it turns on focus reports.
    output.send(b"\x1b[?1004h".to_vec()).unwrap();
    assert_eq!(next_sent(&sent), Sent::Reply(b"\x1b[O".to_vec()));

    session.write(b"ls\r").unwrap();
    assert_eq!(next_sent(&sent), Sent::Input(b"ls\r".to_vec()));
    session.paste("a\nb").unwrap();
    assert_eq!(next_sent(&sent), Sent::Input(b"a\rb".to_vec()));
    session.resize(20, 6, 200, 120).unwrap();
    assert_eq!(next_sent(&sent), Sent::Resize(20, 6));
    assert!(sent.try_recv().is_err());
}

#[test]
fn teardown_releases_the_device_and_ends_the_process_in_platform_order() {
    let (output, sent, session) = fake_session(12, 4);
    let completion = session.shutdown_completion();
    session.shutdown();
    // The source reaches end of output once its process is gone.
    drop(output);
    wait_for(|| completion.is_complete());

    let teardown: Vec<Sent> = sent.try_iter().collect();
    // Unix closes the terminal device before ending the child; Windows ends
    // the child first and then closes ConPTY on a counted worker.
    #[cfg(unix)]
    assert_eq!(teardown, vec![Sent::Release, Sent::Terminate]);
    #[cfg(windows)]
    assert_eq!(teardown, vec![Sent::Terminate, Sent::Release]);
    assert_eq!(
        session.metadata().status,
        SessionStatus::Exited {
            code: 1,
            signal: Some("Hangup".into())
        }
    );
    assert_eq!(session.metrics().active_workers, 0);
}

fn feed(session: &TerminalSession, bytes: &[u8]) {
    let mut terminal = session.terminal.lock();
    session.shared.parser.lock().advance(&mut *terminal, bytes);
    session.shared.changed();
}

#[test]
fn owned_snapshots_preserve_revision_unicode_attributes_and_unchanged_rows() {
    let session = fixture(12, 4, 16);
    feed(&session, "\x1b[38;2;91;166;201m界e\u{301}".as_bytes());
    let first = session.viewport();
    assert_eq!(first.revision, session.revision());
    assert_eq!((first.columns, first.screen_lines), (12, 4));
    assert_eq!(
        first.rows[0][0].fg,
        crate::Color::Spec(crate::Rgb {
            r: 91,
            g: 166,
            b: 201
        })
    );
    assert_eq!(first.rows[0][2].extra, vec!['\u{301}']);
    feed(&session, b"\x1b[3;1Hchanged");
    let next = session.viewport();
    assert!(Arc::ptr_eq(&first.rows[1], &next.rows[1]));
    assert!(!Arc::ptr_eq(&first.rows[2], &next.rows[2]));
    assert_eq!(first.rows[2][0].c, ' ');
    assert_eq!(next.rows[2][0].c, 'c');
    assert!(matches!(next.damage, crate::ViewportDamage::Rows(_)));
    let repeated = session.viewport();
    assert!(matches!(repeated.damage,crate::ViewportDamage::Rows(rows) if rows.is_empty()));
    assert!(Arc::ptr_eq(&next.rows[2], &repeated.rows[2]));
}

#[test]
fn owned_snapshots_preserve_osc8_targets_across_wrap_resize_and_replacement() {
    let session = fixture(12, 4, 16);
    feed(
        &session,
        b"\x1b]8;id=docs;https://example.com/docs\x1b\\documentation link\x1b]8;;\x1b\\ plain",
    );
    let first = session.viewport();
    let target = first.rows[0][0].hyperlink.as_ref().unwrap();
    assert_eq!(target.as_ref(), "https://example.com/docs");
    assert!(Arc::ptr_eq(
        target,
        first.rows[1][0].hyperlink.as_ref().unwrap()
    ));
    assert!(first.rows[1][6].hyperlink.is_none());
    session.terminal.lock().resize(Size {
        cols: 24,
        rows: 4,
        pixel_width: 240,
        pixel_height: 80,
    });
    session.shared.changed();
    let resized = session.viewport();
    assert_eq!(
        resized.rows[0][17].hyperlink.as_deref(),
        Some("https://example.com/docs")
    );
    feed(
        &session,
        b"\x1b[1;1H\x1b]8;;https://other.example/\x07new\x1b]8;;\x07",
    );
    let replaced = session.viewport();
    assert_eq!(
        replaced.rows[0][0].hyperlink.as_deref(),
        Some("https://other.example/")
    );
    assert_eq!(
        first.rows[0][0].hyperlink.as_deref(),
        Some("https://example.com/docs")
    );
}

#[test]
fn dragging_selection_left_to_column_zero_includes_the_leading_character() {
    let session = fixture(12, 4, 0);
    feed(&session, b"/a");
    session.start_selection_at(crate::Point::new(0, 2), crate::SelectionType::Simple);
    session.update_selection_at(crate::Point::new(0, 0));
    assert_eq!(session.selected_text().as_deref(), Some("/a"));
}

#[test]
fn drag_selection_includes_endpoint_cells_when_direction_changes() {
    for viewport_coordinates in [false, true] {
        for kind in [crate::SelectionType::Simple, crate::SelectionType::Block] {
            let session = fixture(12, 4, 0);
            feed(&session, b"/abcdef");
            if viewport_coordinates {
                session.start_selection(0, 3, kind);
            } else {
                session.start_selection_at(crate::Point::new(0, 3), kind);
            }
            for (column, expected) in [(5, "cde"), (0, "/abc"), (3, "c"), (1, "abc"), (6, "cdef")] {
                if viewport_coordinates {
                    session.update_selection(0, column);
                } else {
                    session.update_selection_at(crate::Point::new(0, column));
                }
                assert_eq!(
                    session.selected_text().as_deref(),
                    Some(expected),
                    "viewport_coordinates={viewport_coordinates}, kind={kind:?}, column={column}"
                );
            }
        }
    }
}

#[test]
fn reverse_selection_preserves_newlines_and_unicode_in_scrollback() {
    for viewport_coordinates in [false, true] {
        let session = fixture(12, 2, 16);
        feed(&session, "/abc\r\n/界e\u{301}\r\nlast".as_bytes());
        session.scroll(1);
        if viewport_coordinates {
            session.start_selection(1, 4, crate::SelectionType::Simple);
            session.update_selection(0, 0);
        } else {
            session.start_selection_at(crate::Point::new(0, 4), crate::SelectionType::Simple);
            session.update_selection_at(crate::Point::new(-1, 0));
        }
        assert_eq!(
            session.selected_text().as_deref(),
            Some("/abc\n/界e\u{301}")
        );
        let selection = session.viewport().selection.unwrap();
        assert_eq!(selection.start, crate::Point::new(-1, 0));
        assert_eq!(selection.end, crate::Point::new(0, 4));
    }
}

#[test]
fn reverse_selection_preserves_soft_wrapped_text() {
    let session = fixture(4, 3, 0);
    feed(&session, b"/abcdef");
    session.start_selection_at(crate::Point::new(1, 3), crate::SelectionType::Simple);
    session.update_selection_at(crate::Point::new(0, 0));
    assert_eq!(session.selected_text().as_deref(), Some("/abcdef"));
}

#[test]
fn viewport_scroll_and_selection_use_history_coordinates() {
    let session = fixture(12, 4, 16);
    feed(&session, b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
    session.scroll(1);
    let snapshot = session.viewport();
    assert_eq!(snapshot.display_offset, 1);
    session.start_selection(0, 0, crate::SelectionType::Simple);
    session.update_selection(0, 2);
    assert_eq!(session.selected_text().as_deref(), Some("one"));
    let snapshot = session.viewport();
    let selection = snapshot.selection.unwrap();
    assert_eq!(selection.start, crate::Point::new(-1, 0));
    assert!(selection.contains(crate::Point::new(-1, 1)));
    session.select_all();
    assert!(session.selected_text().unwrap().contains("five"));
    session.clear_history();
    assert_eq!(session.history_size(), 0);
    assert_eq!(session.viewport().display_offset, 0);
}

#[test]
fn clipboard_notifications_have_project_owned_bounded_delivery() {
    let session = fixture(12, 4, 16);
    for index in 0..70 {
        EventProxy {
            shared: session.shared.clone(),
            input: session.input.clone(),
        }
        .send_event(Event::ClipboardStore(
            alacritty_terminal::term::ClipboardType::Clipboard,
            format!("copy-{index}"),
        ));
    }
    let events = session.drain_events();
    assert_eq!(events.len(), 64);
    assert_eq!(
        events[0],
        crate::TerminalEvent::ClipboardStore {
            selection: false,
            text: "copy-6".into()
        }
    );
    assert_eq!(session.metrics().dropped_events, 6);
    assert!(session.drain_events().is_empty());
}

#[test]
fn owned_modes_follow_kitty_and_alternate_screen_state() {
    let session = fixture(12, 4, 16);
    feed(&session, b"\x1b[?1049h\x1b[>31u");
    assert!(
        session
            .modes()
            .contains(crate::Mode::KITTY_KEYBOARD_PROTOCOL | crate::Mode::ALT_SCREEN)
    );
    feed(&session, b"\x1b[<u\x1b[?1049l");
    assert!(
        !session
            .modes()
            .intersects(crate::Mode::KITTY_KEYBOARD_PROTOCOL | crate::Mode::ALT_SCREEN)
    );
}

fn finish_search(session: &TerminalSession, task: &mut crate::SearchTask) -> crate::SearchProgress {
    let budget = crate::SearchBudget {
        max_rows: 1,
        max_duration: Duration::from_secs(1),
    };
    for _ in 0..100 {
        let progress = session.search_step(task, budget);
        if progress != crate::SearchProgress::Pending {
            return progress;
        }
    }
    panic!("Budgeted search did not complete")
}
#[test]
fn search_is_budgeted_cancellable_and_rejects_stale_revisions() {
    let session = fixture(12, 4, 16);
    feed(&session, b"zero\r\none\r\nneedle");
    let query = crate::SearchQuery::compile("needle").unwrap();
    let mut task = session.begin_search(
        query.clone(),
        crate::Point::new(0, 0),
        crate::Direction::Right,
    );
    assert_eq!(
        session.search_step(
            &mut task,
            crate::SearchBudget {
                max_rows: 1,
                max_duration: Duration::from_secs(1)
            }
        ),
        crate::SearchProgress::Pending
    );
    assert_eq!(
        finish_search(&session, &mut task),
        crate::SearchProgress::Found(crate::SelectionRange {
            start: crate::Point::new(2, 0),
            end: crate::Point::new(2, 5),
            is_block: false
        })
    );
    let mut cancelled = session.begin_search(
        query.clone(),
        crate::Point::default(),
        crate::Direction::Right,
    );
    cancelled.cancel();
    assert_eq!(
        finish_search(&session, &mut cancelled),
        crate::SearchProgress::Cancelled
    );
    let mut stale = session.begin_search(query, crate::Point::default(), crate::Direction::Right);
    feed(&session, b"!");
    assert_eq!(
        finish_search(&session, &mut stale),
        crate::SearchProgress::Stale
    );
}
#[test]
fn search_maps_unicode_wrapped_matches_to_grid_columns() {
    let session = fixture(6, 4, 16);
    feed(&session, "ab界cdef".as_bytes());
    let query = crate::SearchQuery::compile("界cde").unwrap();
    let mut task = session.begin_search(query, crate::Point::default(), crate::Direction::Right);
    assert_eq!(
        finish_search(&session, &mut task),
        crate::SearchProgress::Found(crate::SelectionRange {
            start: crate::Point::new(0, 2),
            end: crate::Point::new(1, 0),
            is_block: false
        })
    );
}
#[test]
fn public_errors_distinguish_options_geometry_and_closed_sessions() {
    let invalid = TerminalSession::spawn(
        SessionOptions {
            cols: 0,
            ..SessionOptions::default()
        },
        Arc::new(|| {}),
    )
    .err()
    .unwrap();
    assert_eq!(invalid.kind(), crate::SessionErrorKind::InvalidGeometry);
    let session = fixture(12, 4, 16);
    let completion = session.shutdown_completion();
    session.shutdown();
    assert!(completion.is_complete());
    assert_eq!(
        session.write(b"closed").unwrap_err().kind(),
        crate::SessionErrorKind::Closed
    );
    assert_eq!(
        session.resize(0, 1, 0, 0).unwrap_err().kind(),
        crate::SessionErrorKind::InvalidGeometry
    );
}

#[test]
fn search_wraps_to_earlier_matches_and_retains_logical_line_anchors() {
    let session = fixture(6, 4, 16);
    feed(&session, b"abcdefghij");
    let mut anchored = session.begin_search(
        crate::SearchQuery::compile("^abcdefghi").unwrap(),
        crate::Point::new(1, 1),
        crate::Direction::Right,
    );
    assert_eq!(
        finish_search(&session, &mut anchored),
        crate::SearchProgress::Found(crate::SelectionRange {
            start: crate::Point::new(0, 0),
            end: crate::Point::new(1, 2),
            is_block: false
        })
    );
    let mut backwards = session.begin_search(
        crate::SearchQuery::compile("fgh").unwrap(),
        crate::Point::new(0, 2),
        crate::Direction::Left,
    );
    assert_eq!(
        finish_search(&session, &mut backwards),
        crate::SearchProgress::Found(crate::SelectionRange {
            start: crate::Point::new(0, 5),
            end: crate::Point::new(1, 1),
            is_block: false
        })
    );
}

#[test]
fn clipboard_delivery_is_bounded_by_payload_bytes_as_well_as_event_count() {
    let session = fixture(12, 4, 16);
    let proxy = EventProxy {
        shared: session.shared.clone(),
        input: session.input.clone(),
    };
    for _ in 0..18 {
        proxy.send_event(Event::ClipboardStore(
            alacritty_terminal::term::ClipboardType::Clipboard,
            "x".repeat(MAX_CLIPBOARD_EVENT),
        ));
    }
    proxy.send_event(Event::ClipboardStore(
        alacritty_terminal::term::ClipboardType::Clipboard,
        "x".repeat(MAX_CLIPBOARD_EVENT + 1),
    ));
    assert_eq!(session.metrics().queued_event_bytes, EVENT_BYTE_QUEUE);
    assert_eq!(session.metrics().dropped_events, 3);
    assert_eq!(session.drain_events().len(), 16);
    assert_eq!(session.metrics().queued_event_bytes, 0);
}

#[test]
fn successive_single_cell_searches_advance_and_wrap_in_both_directions() {
    let session = fixture(6, 4, 16);
    feed(&session, b"x x");
    let query = crate::SearchQuery::compile("x").unwrap();
    let mut task = session.begin_search(
        query.clone(),
        crate::Point::new(0, 0),
        crate::Direction::Right,
    );
    let crate::SearchProgress::Found(first) = finish_search(&session, &mut task) else {
        panic!("first match missing");
    };
    let mut task = session.begin_search(
        query.clone(),
        session.next_search_point(first.end, crate::Direction::Right),
        crate::Direction::Right,
    );
    let crate::SearchProgress::Found(second) = finish_search(&session, &mut task) else {
        panic!("second match missing");
    };
    assert_eq!(second.start, crate::Point::new(0, 2));
    let mut task = session.begin_search(
        query.clone(),
        session.next_search_point(second.start, crate::Direction::Left),
        crate::Direction::Left,
    );
    let crate::SearchProgress::Found(previous) = finish_search(&session, &mut task) else {
        panic!("reverse match missing");
    };
    assert_eq!(previous.start, first.start);
    let mut task = session.begin_search(
        query,
        session.next_search_point(first.start, crate::Direction::Left),
        crate::Direction::Left,
    );
    let crate::SearchProgress::Found(last) = finish_search(&session, &mut task) else {
        panic!("wrapped match missing");
    };
    assert_eq!(last.start, second.start);
}

/// Rebuild a second engine from `source`'s snapshot, as a replica is: the
/// snapshot crosses the daemon's wire encoding, then the replica parses the
/// output its engine had read but not finished parsing.
fn rebuild(source: &TerminalSession) -> TerminalSession {
    let (snapshot, ()) = source.snapshot(|| ());
    #[cfg(unix)]
    let snapshot = daemon::protocol::decode_snapshot(&daemon::protocol::encode_snapshot(&snapshot))
        .expect("the snapshot survives its wire encoding");
    let history = source.shared.config.lock().scrolling_history;
    let replica = fixture(snapshot.cols, snapshot.rows, history);
    snapshot.restore(&mut replica.terminal.lock(), &replica.shared);
    feed(&replica, &snapshot.pending);
    replica
}

/// Everything a person or a program can observe of the two engines matches.
fn assert_same_terminal(case: &str, expected: &TerminalSession, actual: &TerminalSession) {
    let (want, got) = (expected.viewport(), actual.viewport());
    assert_eq!(
        (want.columns, want.screen_lines, want.display_offset),
        (got.columns, got.screen_lines, got.display_offset),
        "{case}: geometry"
    );
    assert_eq!(want.rows, got.rows, "{case}: viewport rows");
    assert_eq!(want.cursor, got.cursor, "{case}: cursor");
    assert_eq!(want.mode, got.mode, "{case}: modes");
    assert_eq!(want.colors, got.colors, "{case}: colours");
    assert_eq!(
        expected.history_size(),
        actual.history_size(),
        "{case}: history size"
    );
    assert_eq!(
        expected.history_text(),
        actual.history_text(),
        "{case}: history text"
    );
    let (want_revision, top, bottom, _) = expected.search_bounds();
    let (got_revision, got_top, got_bottom, _) = actual.search_bounds();
    assert_eq!((top, bottom), (got_top, got_bottom), "{case}: grid bounds");
    for line in top..=bottom {
        assert_eq!(
            expected.search_row(line, want_revision),
            actual.search_row(line, got_revision),
            "{case}: cells, styles and wraps of line {line}"
        );
    }
    // The backend's own cells and cursors, including what the owned view
    // leaves out: underline colours, hyperlink ids, pending wraps and the
    // cursor's current attributes.
    let (want, got) = (expected.terminal.lock(), actual.terminal.lock());
    for line in top..=bottom {
        for column in 0..want.columns() {
            let point = Point::new(Line(line), Column(column));
            assert_eq!(
                want.grid()[point],
                got.grid()[point],
                "{case}: cell {point:?}"
            );
        }
    }
    assert_eq!(
        want.grid().cursor,
        got.grid().cursor,
        "{case}: backend cursor"
    );
    assert_eq!(
        want.grid().saved_cursor,
        got.grid().saved_cursor,
        "{case}: saved cursor"
    );
    assert_eq!(want.mode(), got.mode(), "{case}: backend modes");
    assert_eq!(
        want.cursor_style(),
        got.cursor_style(),
        "{case}: cursor style"
    );
    drop((want, got));
    let (want, got) = (expected.metadata(), actual.metadata());
    assert_eq!(want.title, got.title, "{case}: title");
    assert_eq!(
        want.reported_cwd, got.reported_cwd,
        "{case}: reported directory"
    );
    assert_eq!(
        want.remote_process_id, got.remote_process_id,
        "{case}: remote process id"
    );
}

/// Output that exercises cursor state, saved cursors, pending wraps, styles
/// and screens after the snapshot, so drift a static comparison misses shows.
const FOLLOW_UP: &[u8] = b"\x1b8after\x1b[?1049l\x1b8back\r\nnext \x1b[1;31mred\x1b[m line\r\n";

#[test]
fn a_rebuilt_engine_matches_its_source_across_the_fixture_corpus() {
    let mut history = String::new();
    for line in 0..40 {
        history.push_str(&format!("history line {line}\r\n"));
    }
    let corpus: &[(&str, &[u8])] = &[
        ("history and cursor", history.as_bytes()),
        (
            "SGR and truecolor",
            b"\x1b[1;3;4mbold italic\x1b[0m \x1b[38;2;91;166;201;48;5;17mtrue\x1b[7;9;2mattrs\x1b[58;2;1;2;3;4:3mcurl\x1b[m\r\n\x1b[44m    \x1b[m\r\n",
        ),
        (
            "alternate screen",
            b"primary\r\nshell \x1b[>1u\x1b[?1049h\x1b[>5u\x1b[2;3Hfull\x1b[33mscreen\x1b7\x1b[5;1Hstatus",
        ),
        (
            "wide and combining characters",
            "\u{754c}e\u{301} \u{1f600}x\u{20dd}\r\n0123456789012345678\u{754c}tail\r\n".as_bytes(),
        ),
        (
            "wrapped lines",
            b"a line long enough to wrap twice around twenty\r\nexact twenty columns\r\nfull",
        ),
        (
            "keyboard protocol flags",
            b"\x1b[>1u\x1b[>3u\x1b[=8;2u",
        ),
        (
            "mouse modes",
            b"\x1b[?1002h\x1b[?1006h\x1b[?1004h",
        ),
        (
            "bracketed paste and input modes",
            b"\x1b[?2004h\x1b[?1h\x1b=\x1b[4h\x1b[?7l",
        ),
        (
            "hyperlinks",
            b"\x1b]8;id=docs;https://example.com/a\x1b\\docs\x1b]8;;\x1b\\ \x1b]8;;file:///tmp\x07tmp\x1b]8;;\x07 open\x1b]8;;https://open.example\x07",
        ),
        (
            "title, colours and cursor style",
            b"\x1b]0;build: running\x07\x1b]4;1;rgb:12/34/56\x07\x1b]11;rgb:01/02/03\x07\x1b[5 q\x1b[?12h",
        ),
        (
            "pending wrap at the last column",
            b"01234567890123456789",
        ),
        (
            "saved cursor and origin mode",
            b"top\x1b[3;5H\x1b[1;32m\x1b7\x1b[m\x1b[?6h\x1b[2;2Hfrom origin",
        ),
    ];
    for (case, bytes) in corpus {
        let source = fixture(20, 6, 64);
        feed(&source, bytes);
        let replica = rebuild(&source);
        assert_same_terminal(case, &source, &replica);
        feed(&source, FOLLOW_UP);
        feed(&replica, FOLLOW_UP);
        assert_same_terminal(&format!("{case}, then more output"), &source, &replica);
    }
}

#[test]
fn output_cut_anywhere_by_a_snapshot_is_parsed_once_by_the_replica() {
    let stream = "a\x1b]0;tit le\x07b\x1b]2;st\x1b\\c\x1b[1;38;2;1;2;3mD\x1b[m\u{1f600}\u{754c}e\
        \x1bPq#0;2;0;0;0~\x1b\\f\x1b_apc\x1b\\g\x1b(0q\x1b(Bh\x1b]8;;http://x\x07link\x1b]8;;\x07\x1b[?1049h\x1b[2Jalt";
    let bytes = stream.as_bytes();
    for cut in 0..=bytes.len() {
        let source = fixture(40, 4, 16);
        feed(&source, &bytes[..cut]);
        let replica = rebuild(&source);
        feed(&source, &bytes[cut..]);
        feed(&replica, &bytes[cut..]);
        assert_same_terminal(&format!("output cut at byte {cut}"), &source, &replica);
    }
}

#[test]
fn a_synchronized_update_in_progress_is_applied_before_the_snapshot() {
    let source = fixture(20, 4, 16);
    feed(&source, b"old\x1b[?2026h\r\x1b[Knew frame");
    assert!(
        source.screen_text().starts_with("old"),
        "the update is held"
    );
    let replica = rebuild(&source);
    assert!(source.screen_text().starts_with("new frame"));
    assert_same_terminal("update applied", &source, &replica);
    feed(&source, b" ends\x1b[?2026l");
    feed(&replica, b" ends\x1b[?2026l");
    assert_same_terminal("update ended", &source, &replica);
}

fn resize_grid(session: &TerminalSession, cols: u16, rows: u16) {
    let size = Size {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    };
    let mut terminal = session.terminal.lock();
    session.shared.prompt.lock().resize(&mut *terminal, size);
    session.shared.changed();
}

#[test]
fn a_rebuilt_engine_keeps_the_prompt_mark_its_shell_redraws_from() {
    let source = fixture(20, 6, 16);
    feed(&source, b"output\r\n$ ");
    {
        let terminal = source.terminal.lock();
        source
            .shared
            .prompt
            .lock()
            .mark(&*terminal, PromptMark::Start { redraw: None });
    }
    feed(&source, b"a command long enough to wrap");
    let replica = rebuild(&source);
    // A shell redraws its prompt after a resize, so the old one is cleared.
    resize_grid(&source, 12, 6);
    resize_grid(&replica, 12, 6);
    assert!(
        !source.screen_text().contains("command"),
        "the typed command was cleared"
    );
    assert_same_terminal("after resize", &source, &replica);
}

#[test]
fn a_rebuilt_engine_keeps_only_the_newest_history_its_limit_allows() {
    let source = fixture(20, 4, 100);
    for line in 0..200 {
        feed(&source, format!("line {line}\r\n").as_bytes());
    }
    let (snapshot, ()) = source.snapshot(|| ());
    let replica = fixture(20, 4, 10);
    snapshot.restore(&mut replica.terminal.lock(), &replica.shared);
    assert_eq!(replica.history_size(), 10);
    let history = replica.history_text();
    let first = history.lines().next().unwrap();
    assert_eq!(first, "line 187");
    assert_eq!(replica.screen_text(), source.screen_text());
}

/// What a tap's writer gets within a moment, or `None` while it is left waiting.
#[cfg(unix)]
fn taken(tap: &Arc<Tap>) -> Option<Tapped> {
    let (sender, receiver) = mpsc::channel();
    let writer = tap.clone();
    let waiting = thread::spawn(move || {
        let _ = sender.send(writer.next());
    });
    let taken = receiver.recv_timeout(Duration::from_millis(100)).ok();
    if taken.is_none() {
        // Release the writer left waiting; a closed tap holds nothing.
        tap.close();
    }
    waiting.join().unwrap();
    taken
}

#[cfg(unix)]
fn bytes_of(tapped: Option<Tapped>) -> Vec<u8> {
    match tapped {
        Some(Tapped::Bytes(bytes)) => bytes,
        Some(Tapped::Ended) => panic!("the tap ended"),
        Some(Tapped::Closed) => panic!("the tap closed"),
        None => panic!("the writer was left waiting"),
    }
}

#[test]
#[cfg(unix)]
fn a_tap_holds_output_for_its_writer_until_the_engine_has_caught_up() {
    let tap = Arc::new(Tap::default());
    tap.push(b"echo");
    tap.push(b"ed");
    tap.flush();
    // Both chunks leave as one batch, in order, as soon as they are flushed.
    assert_eq!(bytes_of(taken(&tap)), b"echoed");

    // Without a flush, less than a batch waits for the output that follows.
    let held = Arc::new(Tap::default());
    held.push(b"more to come");
    assert!(taken(&held).is_none());
}

#[test]
#[cfg(unix)]
fn a_tap_wakes_its_writer_for_a_full_batch_while_the_engine_is_still_behind() {
    let tap = Arc::new(Tap::default());
    let slice = vec![b'x'; PARSE_SLICE];
    let mut pushed = 0;
    while pushed < TAP_BATCH {
        tap.push(&slice);
        pushed += slice.len();
    }
    assert_eq!(bytes_of(taken(&tap)).len(), pushed);
    // The batch was taken whole: what follows starts another.
    tap.push(b"tail");
    tap.flush();
    assert_eq!(bytes_of(taken(&tap)), b"tail");
}

#[test]
#[cfg(unix)]
fn a_tap_delivers_unflushed_output_before_its_end_and_drops_it_when_closed() {
    let tap = Arc::new(Tap::default());
    tap.push(b"final words");
    let mut output = OutputTap {
        subscriber: Some(tap.clone()),
        ended: false,
    };
    output.end();
    assert_eq!(bytes_of(taken(&tap)), b"final words");
    assert!(matches!(taken(&tap), Some(Tapped::Ended)));

    // A client that detaches takes nothing more; the engine keeps the output.
    let detached = Arc::new(Tap::default());
    detached.push(b"unsent");
    detached.close();
    detached.push(b"later");
    detached.flush();
    assert!(matches!(taken(&detached), Some(Tapped::Closed)));
}

#[test]
#[cfg(unix)]
fn a_full_tap_holds_the_engine_until_its_writer_takes_the_batch() {
    let session = fixture(20, 4, 0);
    let tap = Arc::new(Tap::default());
    tap.push(&vec![b'x'; TAP_HIGH_WATER + 1]);
    let (sender, receiver) = mpsc::channel();
    let (engine, shared) = (tap.clone(), session.shared.clone());
    let waiting = thread::spawn(move || {
        engine.wait_for_room(&shared);
        let _ = sender.send(());
    });
    assert!(receiver.recv_timeout(Duration::from_millis(100)).is_err());
    assert_eq!(bytes_of(taken(&tap)).len(), TAP_HIGH_WATER + 1);
    receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    waiting.join().unwrap();
}
