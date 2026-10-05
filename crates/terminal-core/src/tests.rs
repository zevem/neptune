use super::*;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::Color;

#[cfg(unix)]
fn wait_for(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "PTY condition timed out");
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
        tracker.advance(b"\\"),
        Some(PathBuf::from("/home/dev/my project"))
    );
    assert!(decode_cwd(b"file://host/tmp/%00").is_none());
    assert!(decode_cwd(b"file://host/tmp/%GG").is_none());
    assert!(decode_cwd(b"https://host/path").is_none());
}

#[test]
fn oversized_osc7_has_bounded_storage_and_recovers() {
    let mut tracker = CwdTracker::default();
    let mut input = b"\x1b]7;file://host/".to_vec();
    input.extend(vec![b'x'; MAX_OSC * 2]);
    input.extend_from_slice(b"\x07\x1b]7;file://host/tmp\x07");
    assert_eq!(tracker.advance(&input), Some(PathBuf::from("/tmp")));
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
    assert_eq!(
        tracker.advance(format!(";\"{dir}\"\x1b\\").as_bytes()),
        Some(PathBuf::from(dir))
    );
    assert_eq!(
        tracker.advance(format!("\x1b]9;9;{dir}\x07").as_bytes()),
        Some(PathBuf::from(dir))
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
            tracker.advance(b"\x1b]7;file://neptune/home/dev\x07"),
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
    let shared = Arc::new(Shared::new(
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
    ));
    let (input, _) = mpsc::sync_channel(INPUT_QUEUE);
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
fn feed(session: &TerminalSession, bytes: &[u8]) {
    let mut terminal = session.terminal.lock();
    let mut parser: Processor = Processor::new();
    parser.advance(&mut *terminal, bytes);
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
