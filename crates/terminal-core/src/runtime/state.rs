//! Session synchronization and bounded event delivery. Lock ordering: config, terminal, parser, prompt/size/metadata; viewport extraction locks terminal then its cache.
use super::*;
use alacritty_terminal::event::{Event, WindowSize};
use parking_lot::Condvar;
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
    /// The pane's focus, for a session whose program another engine answers.
    Focus(bool),
    /// The history limit and palette of a replica, for the engine that
    /// answers its program.
    Scrollback(usize),
    Color(usize, crate::Rgb),
    /// Make the program redraw its screen, as after a resize.
    Redraw,
    /// The session stopped: the writer, which waits only on this queue, ends.
    Stop,
}

impl Input {
    /// Bytes counted against the pending input budget.
    pub(super) fn queued_bytes(&self) -> usize {
        match self {
            Input::Write(bytes) | Input::Reply(bytes) => bytes.len(),
            Input::Resize(_)
            | Input::Focus(_)
            | Input::Scrollback(_)
            | Input::Color(..)
            | Input::Redraw
            | Input::Stop => 0,
        }
    }
}

pub(super) enum Output {
    Bytes(Vec<u8>, usize),
    End,
    Error(io::Error),
    /// Nothing was read: the session stopped or a process check is waiting,
    /// and the engine, which waits only on this queue, must look.
    Wake,
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
    /// Another engine (the session daemon's) is authoritative for the program;
    /// this one is its replica. Set once, before the workers start.
    pub(super) replica: AtomicBool,
    /// A replica stopping because its handle was dropped leaves the program running.
    pub(super) detached: AtomicBool,
    pub(super) output_tap: Mutex<OutputTap>,
    /// Locked after the terminal, by the engine and by snapshots.
    pub(super) parser: Mutex<Parser>,
    /// The workers' queues and descriptor waits have no timeout while a
    /// session is quiet, so whatever else needs a worker wakes it through
    /// these.
    engine_wake: SyncSender<Output>,
    writer_wake: SyncSender<Input>,
    #[cfg(unix)]
    pub(super) stop_signal: transport::StopSignal,
}

impl Shared {
    pub(super) fn new(
        metadata: SessionMetadata,
        size: Size,
        config: Config,
        repaint: Repaint,
        engine_wake: SyncSender<Output>,
        writer_wake: SyncSender<Input>,
    ) -> io::Result<Self> {
        Ok(Self {
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
            replica: AtomicBool::new(false),
            detached: AtomicBool::new(false),
            output_tap: Mutex::new(OutputTap::default()),
            parser: Mutex::new(Parser::default()),
            engine_wake,
            writer_wake,
            #[cfg(unix)]
            stop_signal: transport::StopSignal::new()?,
        })
    }

    /// Stop the session and end every worker's wait. Returns whether it had
    /// already stopped.
    pub(super) fn stop(&self) -> bool {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return true;
        }
        #[cfg(unix)]
        self.stop_signal.raise();
        // A full queue has a worker that is not waiting, and each rechecks
        // `stopped` before it waits again.
        let _ = self.writer_wake.try_send(Input::Stop);
        self.wake_engine();
        false
    }

    /// Have the engine run the checks that follow its wait for output.
    pub(super) fn wake_engine(&self) {
        // As in `stop`, a full queue needs no wake.
        let _ = self.engine_wake.try_send(Output::Wake);
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

/// Where an authoritative engine copies the output it has parsed. Chunks are
/// pushed under the terminal lock, so a snapshot taken under that lock and the
/// chunks that follow it neither overlap nor leave a gap.
#[derive(Default)]
pub(super) struct OutputTap {
    pub(super) subscriber: Option<Arc<Tap>>,
    /// The engine has stopped; nothing more will be pushed.
    pub(super) ended: bool,
}

impl OutputTap {
    /// Replace any earlier subscriber, which is closed.
    #[cfg(unix)]
    pub(super) fn subscribe(&mut self) -> Arc<Tap> {
        let tap = Arc::new(Tap::default());
        if self.ended {
            tap.end();
        }
        if let Some(earlier) = self.subscriber.replace(tap.clone()) {
            earlier.close();
        }
        tap
    }

    #[cfg(unix)]
    pub(super) fn unsubscribe(&mut self, tap: &Arc<Tap>) {
        if self
            .subscriber
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, tap))
        {
            self.subscriber = None;
        }
        tap.close();
    }

    pub(super) fn end(&mut self) {
        self.ended = true;
        if let Some(tap) = &self.subscriber {
            tap.end();
        }
    }
}

/// Output queued for one attached client, drained by its connection writer
/// in batches: the writer is woken once [`TAP_BATCH`] bytes are queued or the
/// engine has parsed everything it was given, never by a timer, so a
/// keystroke's echo leaves as soon as it is parsed while sustained output
/// crosses in few large frames.
#[derive(Default)]
pub(super) struct Tap {
    state: Mutex<TapState>,
    ready: Condvar,
}

#[derive(Default)]
struct TapState {
    bytes: Vec<u8>,
    /// The writer may take what is queued.
    flush: bool,
    ended: bool,
    closed: bool,
}

#[cfg(unix)]
pub(super) enum Tapped {
    Bytes(Vec<u8>),
    /// Every byte was taken and the engine has stopped.
    Ended,
    /// The subscriber was replaced or went away.
    Closed,
}

/// Queued output at which the writer is woken while more is still to parse.
pub(super) const TAP_BATCH: usize = READ_SIZE;

/// Queued output above which the engine waits for the client to catch up, so
/// a slow client applies backpressure to the program as a local pane does.
pub(super) const TAP_HIGH_WATER: usize = 2 * READ_SIZE;

impl Tap {
    pub(super) fn push(&self, chunk: &[u8]) {
        let mut state = self.state.lock();
        if state.closed {
            return;
        }
        state.bytes.extend_from_slice(chunk);
        if state.bytes.len() >= TAP_BATCH && !state.flush {
            state.flush = true;
            drop(state);
            self.ready.notify_all();
        }
    }

    /// Hand what is queued to the writer now. The engine calls this once it
    /// has parsed all the output read so far.
    pub(super) fn flush(&self) {
        let mut state = self.state.lock();
        if !state.bytes.is_empty() && !state.flush {
            state.flush = true;
            drop(state);
            self.ready.notify_all();
        }
    }

    fn end(&self) {
        self.state.lock().ended = true;
        self.ready.notify_all();
    }

    #[cfg(unix)]
    pub(super) fn close(&self) {
        let mut state = self.state.lock();
        state.closed = true;
        state.bytes = Vec::new();
        drop(state);
        self.ready.notify_all();
    }

    /// Everything queued, once it is due: as one batch, in order.
    #[cfg(unix)]
    pub(super) fn next(&self) -> Tapped {
        let mut state = self.state.lock();
        loop {
            if state.closed {
                return Tapped::Closed;
            }
            if !state.bytes.is_empty() && (state.flush || state.ended) {
                state.flush = false;
                let bytes = std::mem::take(&mut state.bytes);
                drop(state);
                // The engine may be waiting for room.
                self.ready.notify_all();
                return Tapped::Bytes(bytes);
            }
            if state.ended {
                return Tapped::Ended;
            }
            self.ready.wait(&mut state);
        }
    }

    /// Called by the engine without the terminal lock.
    pub(super) fn wait_for_room(&self, shared: &Shared) {
        let mut state = self.state.lock();
        while state.bytes.len() > TAP_HIGH_WATER
            && !state.closed
            && !shared.stopped.load(Ordering::Acquire)
        {
            self.ready.wait_for(&mut state, Duration::from_millis(100));
        }
    }
}
