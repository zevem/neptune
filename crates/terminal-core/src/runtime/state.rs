//! Session synchronization and bounded event delivery. Lock ordering: config, terminal, prompt/size; viewport extraction locks terminal then its cache.
use super::*;
use alacritty_terminal::event::{Event, WindowSize};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Size {
    pub(super) cols: u16,
    pub(super) rows: u16,
    pub(super) pixel_width: u16,
    pub(super) pixel_height: u16,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }
    fn screen_lines(&self) -> usize {
        self.rows as usize
    }
    fn columns(&self) -> usize {
        self.cols as usize
    }
}

impl Size {
    pub(super) fn pty(self) -> PtySize {
        PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: self.pixel_width,
            pixel_height: self.pixel_height,
        }
    }

    pub(super) fn window(self) -> WindowSize {
        WindowSize {
            num_lines: self.rows,
            num_cols: self.cols,
            cell_width: self.pixel_width / self.cols.max(1),
            cell_height: self.pixel_height / self.rows.max(1),
        }
    }
}

pub(super) enum Input {
    /// Bytes typed or pasted into the session.
    Write(Vec<u8>),
    /// Bytes the engine generated in answer to the program's output.
    Reply(Vec<u8>),
    Resize(Size),
}

impl Input {
    /// Bytes counted against the pending input budget.
    pub(super) fn queued_bytes(&self) -> usize {
        match self {
            Input::Write(bytes) | Input::Reply(bytes) => bytes.len(),
            Input::Resize(_) => 0,
        }
    }
}

pub(super) enum Output {
    Bytes(Vec<u8>, usize),
    End,
    Error(io::Error),
}

pub(super) struct Shared {
    pub(super) process_check: Mutex<Option<SyncSender<ProcessActivity>>>,
    pub(super) metadata: Mutex<SessionMetadata>,
    pub(super) size: Mutex<Size>,
    pub(super) events: Mutex<VecDeque<crate::TerminalEvent>>,
    pub(super) palette: Mutex<[Rgb; 269]>,
    pub(super) config: Mutex<Config>,
    pub(super) stopped: AtomicBool,
    pub(super) revision: AtomicU64,
    pub(super) bytes_received: AtomicU64,
    pub(super) bytes_parsed: AtomicU64,
    pub(super) parse_nanoseconds: AtomicU64,
    pub(super) active_workers: AtomicUsize,
    pub(super) queued_input_bytes: AtomicUsize,
    pub(super) repaint_pending: AtomicBool,
    pub(super) prompt: Mutex<PromptState>,
    pub(super) repaint: Repaint,
    pub(super) input_backpressure: AtomicU64,
    pub(super) dropped_events: AtomicU64,
    pub(super) queued_event_bytes: AtomicUsize,
    pub(super) spawn_nanoseconds: AtomicU64,
    pub(super) cleanup_nanoseconds: AtomicU64,
    pub(super) snapshot_nanoseconds: AtomicU64,
    pub(super) snapshot_lock_nanoseconds: AtomicU64,
    pub(super) snapshot_cells: AtomicU64,
    pub(super) snapshot_rows: AtomicU64,
    pub(super) snapshot_count: AtomicU64,
    pub(super) shutdown_started: Mutex<Option<Instant>>,
}

impl Shared {
    pub(super) fn new(
        metadata: SessionMetadata,
        size: Size,
        config: Config,
        repaint: Repaint,
    ) -> Self {
        Self {
            process_check: Mutex::new(None),
            metadata: Mutex::new(metadata),
            size: Mutex::new(size),
            config: Mutex::new(config),
            events: Mutex::new(VecDeque::new()),
            palette: Mutex::new(default_palette()),
            stopped: AtomicBool::new(false),
            revision: AtomicU64::new(1),
            bytes_received: AtomicU64::new(0),
            bytes_parsed: AtomicU64::new(0),
            parse_nanoseconds: AtomicU64::new(0),
            active_workers: AtomicUsize::new(0),
            queued_input_bytes: AtomicUsize::new(0),
            repaint_pending: AtomicBool::new(false),
            prompt: Mutex::new(PromptState::default()),
            repaint,
            input_backpressure: AtomicU64::new(0),
            dropped_events: AtomicU64::new(0),
            queued_event_bytes: AtomicUsize::new(0),
            spawn_nanoseconds: AtomicU64::new(0),
            cleanup_nanoseconds: AtomicU64::new(0),
            snapshot_nanoseconds: AtomicU64::new(0),
            snapshot_lock_nanoseconds: AtomicU64::new(0),
            snapshot_cells: AtomicU64::new(0),
            snapshot_rows: AtomicU64::new(0),
            snapshot_count: AtomicU64::new(0),
            shutdown_started: Mutex::new(None),
        }
    }

    pub(super) fn enqueue(
        &self,
        sender: &SyncSender<Input>,
        input: Input,
    ) -> std::result::Result<(), SessionError> {
        let bytes = input.queued_bytes();
        if self
            .queued_input_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                (queued + bytes <= MAX_QUEUED_INPUT).then_some(queued + bytes)
            })
            .is_err()
        {
            self.input_backpressure.fetch_add(1, Ordering::Relaxed);
            return Err(SessionError::new(
                SessionErrorKind::Backpressure,
                "PTY input exceeds the two MiB pending budget",
            ));
        }
        match sender.try_send(input) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.queued_input_bytes.fetch_sub(bytes, Ordering::AcqRel);
                match error {
                    TrySendError::Full(_) => {
                        self.input_backpressure.fetch_add(1, Ordering::Relaxed);
                        Err(SessionError::new(
                            SessionErrorKind::Backpressure,
                            "PTY input queue is full; retry after pending input is written",
                        ))
                    }
                    TrySendError::Disconnected(_) => Err(SessionError::new(
                        SessionErrorKind::Closed,
                        "Terminal session is closed",
                    )),
                }
            }
        }
    }

    pub(super) fn changed(&self) {
        self.revision.fetch_add(1, Ordering::Release);
        // The renderer clears this before taking its frame snapshot. Changes
        // after that acknowledgement schedule another frame, while an output
        // burst before it needs only one wakeup regardless of PTY read sizes.
        if !self.repaint_pending.swap(true, Ordering::AcqRel) {
            (self.repaint)();
        }
    }

    pub(super) fn force_repaint(&self) {
        // Lifecycle and metadata events remain observable for hidden panes,
        // whose output notification can stay pending until they are activated.
        self.repaint_pending.store(true, Ordering::Release);
        (self.repaint)();
    }

    pub(super) fn changed_force(&self) {
        self.revision.fetch_add(1, Ordering::Release);
        self.force_repaint();
    }

    pub(super) fn push_event(&self, event: crate::TerminalEvent) {
        let mut events = self.events.lock();
        let wake = events.is_empty();
        let mut bytes = self.queued_event_bytes.load(Ordering::Relaxed);
        let size = event.payload_len();
        while events.len() >= EVENT_QUEUE || bytes + size > EVENT_BYTE_QUEUE {
            if let Some(old) = events.pop_front() {
                bytes -= old.payload_len();
            }
            self.dropped_events.fetch_add(1, Ordering::Relaxed);
        }
        events.push_back(event);
        self.queued_event_bytes
            .store(bytes + size, Ordering::Relaxed);
        drop(events);
        if wake {
            self.force_repaint();
        }
    }

    pub(super) fn error(&self, error: impl std::fmt::Display) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        self.metadata.lock().status = SessionStatus::Error(error.to_string());
        self.changed_force();
    }
}

/// Engine replies never run on the UI thread or recursively lock the terminal.
#[derive(Clone)]
pub(super) struct EventProxy {
    pub(super) shared: Arc<Shared>,
    pub(super) input: SyncSender<Input>,
}

impl EventProxy {
    fn reply(&self, text: String) {
        if let Err(error) = self
            .shared
            .enqueue(&self.input, Input::Reply(text.into_bytes()))
        {
            self.shared.error(error);
        }
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(title) => {
                let mut metadata = self.shared.metadata.lock();
                if metadata.title != title {
                    metadata.title = title;
                    drop(metadata);
                    self.shared.force_repaint();
                }
            }
            Event::ResetTitle => {
                let mut metadata = self.shared.metadata.lock();
                if !metadata.title.is_empty() {
                    metadata.title.clear();
                    drop(metadata);
                    self.shared.force_repaint();
                }
            }
            Event::Bell => {
                self.shared.metadata.lock().bell_count += 1;
                // Codex and other TUIs use BEL when their OSC auto-detection
                // does not recognize the host terminal. BEL has no message.
                self.shared
                    .push_event(crate::TerminalEvent::Notification(crate::Notification {
                        title: "Terminal bell".into(),
                        ..Default::default()
                    }));
            }
            Event::PtyWrite(text) => self.reply(text),
            Event::ColorRequest(index, formatter) => {
                if let Some(color) = self.shared.palette.lock().get(index).copied() {
                    self.reply(formatter(color));
                }
            }
            Event::TextAreaSizeRequest(formatter) => {
                let size = self.shared.size.lock().window();
                self.reply(formatter(size));
            }
            Event::ClipboardStore(kind, text) => {
                if text.len() > MAX_CLIPBOARD_EVENT {
                    self.shared.dropped_events.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                self.shared
                    .push_event(crate::TerminalEvent::ClipboardStore {
                        selection: matches!(
                            kind,
                            alacritty_terminal::term::ClipboardType::Selection
                        ),
                        text,
                    });
            }
            // OSC 52 reads are disabled by parser policy and never reach the desktop.
            Event::ClipboardLoad(_, _) => {}
            Event::MouseCursorDirty
            | Event::CursorBlinkingChange
            | Event::Wakeup
            | Event::Exit
            | Event::ChildExit(_) => {}
        }
    }
}
