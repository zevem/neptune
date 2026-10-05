//! Public session operations; grid mutations and revisions share the terminal lock.
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(test)]
use alacritty_terminal::event::Event;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Processor, Rgb};
use anyhow::{Context, Result, bail};
use parking_lot::{Mutex, MutexGuard};

#[path = "runtime/mod.rs"]
mod runtime;
use runtime::{engine_loop, read_loop, write_loop};
#[path = "runtime/state.rs"]
mod state;
use state::*;
#[path = "limits.rs"]
mod limits;
pub use limits::MAX_GRID_CELLS;
use limits::check_grid_budget;
#[path = "shell_integration/mod.rs"]
mod shell_integration;
use shell_integration::{CwdTracker, PromptScanner, PromptState};
#[cfg(test)]
use shell_integration::{PromptMark, decode_cwd, decode_path, row_identity};
#[path = "engine.rs"]
mod engine;
#[path = "notifications.rs"]
mod notifications;
use crate::{SessionError, SessionErrorKind};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};

const READ_SIZE: usize = 64 * 1024;
const PARSE_SLICE: usize = 16 * 1024;
const OUTPUT_QUEUE: usize = 8;
const INPUT_QUEUE: usize = 64;
const MAX_INPUT: usize = 1024 * 1024;
const MAX_QUEUED_INPUT: usize = 2 * 1024 * 1024;
const EVENT_QUEUE: usize = 64;
const EVENT_BYTE_QUEUE: usize = 1024 * 1024;
const MAX_CLIPBOARD_EVENT: usize = 64 * 1024;

/// Conservative transport allocation reservation (not a strict RSS bound).
/// Includes queued and active writes, output/pool buffers, VTE's 2 MiB sync
/// allocation and the bounded clipboard payload queue. Grid extras and allocator
/// overhead are separate from this estimate.
pub const TRANSPORT_BUFFER_BUDGET: usize = MAX_QUEUED_INPUT
    + MAX_INPUT
    + (2 * OUTPUT_QUEUE + 2) * READ_SIZE
    + 2 * 1024 * 1024
    + EVENT_BYTE_QUEUE
    + 16 * (2 * 4096 + 2 * 8192 + 128)
    + 8192;
const MAX_OSC: usize = 8192;

pub type Repaint = Arc<dyn Fn() + Send + Sync>;

/// Options for a real shell session. Inherited environment variables are kept.
#[derive(Debug, Clone)]
pub struct SessionOptions {
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
    pub scrollback: usize,
    /// An explicit executable; `None` uses the system's default shell.
    pub shell: Option<String>,
    /// Arguments require an explicit shell. An empty vector is valid.
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            cols: 100,
            rows: 30,
            scrollback: 10_000,
            shell: None,
            args: Vec::new(),
            env: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Running,
    Exited { code: u32, signal: Option<String> },
    Error(String),
}

/// An on-demand OS observation, independent of terminal output and titles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessActivity {
    /// Only a recognized shell remains, with no foreground or child job.
    Idle,
    /// A foreground job, child process, or non-shell program is running.
    Running,
    /// The operating system could not establish whether the session is idle.
    Unknown,
}

#[derive(Debug, Clone)]
pub struct SessionMetadata {
    pub title: String,
    pub shell: String,
    pub cwd: PathBuf,
    /// Last directory explicitly reported by the shell with OSC 7. Unlike
    /// `cwd`, this never comes from polling the local child process.
    pub reported_cwd: Option<PathBuf>,
    pub process_id: Option<u32>,
    /// Shell PID reported by Neptune's remote bootstrap; never a local PID.
    pub remote_process_id: Option<u32>,
    pub status: SessionStatus,
    pub bell_count: u64,
}

/// Counters are cumulative. Parser timing excludes mutex wait time.
#[derive(Debug, Clone, Copy, Default)]
pub struct SessionMetrics {
    pub bytes_received: u64,
    pub bytes_parsed: u64,
    pub parse_nanoseconds: u64,
    pub revision: u64,
    pub active_workers: usize,
    pub queued_input_bytes: usize,
    pub input_backpressure: u64,
    pub dropped_events: u64,
    pub queued_event_bytes: usize,
    pub spawn_nanoseconds: u64,
    pub cleanup_nanoseconds: u64,
    pub snapshot_nanoseconds: u64,
    pub snapshot_lock_nanoseconds: u64,
    pub snapshot_cells: u64,
    pub snapshot_rows: u64,
    pub snapshot_count: u64,
}

/// One session owns one shell, one VT grid, and three I/O workers.
///
/// All UI-facing operations are bounded and nonblocking apart from the short
/// grid mutex. Output backpressure is applied to the PTY rather than accumulating
/// an unbounded event log. Dropping a session terminates its child and workers.
pub struct TerminalSession {
    terminal: Arc<Mutex<Term<EventProxy>>>,
    shared: Arc<Shared>,
    input: SyncSender<Input>,
    viewport_cache: Mutex<Option<crate::ViewportSnapshot>>,
}

impl TerminalSession {
    /// Ask the session worker to inspect processes without blocking a frame.
    /// Only one pending request is retained; a newer request disconnects the
    /// previous receiver. Callers must treat disconnection/timeouts as unknown.
    pub fn check_process_activity(&self) -> Receiver<ProcessActivity> {
        let (sender, receiver) = mpsc::sync_channel(1);
        if matches!(self.metadata().status, SessionStatus::Exited { .. }) {
            let _ = sender.try_send(ProcessActivity::Idle);
        } else if self.shared.stopped.load(Ordering::Acquire) {
            let _ = sender.try_send(ProcessActivity::Unknown);
        } else {
            *self.shared.process_check.lock() = Some(sender);
        }
        receiver
    }

    pub fn spawn(
        options: SessionOptions,
        repaint: Repaint,
    ) -> std::result::Result<Self, SessionError> {
        validate_options(&options)?;
        let started = Instant::now();
        let session = Self::spawn_impl(options, repaint).map_err(SessionError::spawn)?;
        session
            .shared
            .spawn_nanoseconds
            .store(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        Ok(session)
    }

    fn spawn_impl(options: SessionOptions, repaint: Repaint) -> Result<Self> {
        if options.cols == 0 || options.rows == 0 {
            bail!("Terminal dimensions must be nonzero");
        }
        if options.cols > 4096 || options.rows > 4096 {
            bail!("Terminal dimensions exceed 4096 cells per axis");
        }
        if options.scrollback > 1_000_000 {
            bail!("Scrollback exceeds one million lines");
        }
        check_grid_budget(options.cols, options.rows, options.scrollback)?;
        if options.shell.is_none() && !options.args.is_empty() {
            bail!("Shell arguments require an explicit shell executable");
        }
        if !options.cwd.is_dir() {
            bail!(
                "Working directory does not exist: {}",
                options.cwd.display()
            );
        }

        let size = Size {
            cols: options.cols,
            rows: options.rows,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = portable_pty::native_pty_system()
            .openpty(size.pty())
            .context("Open pseudoterminal")?;
        let mut command = match &options.shell {
            Some(shell) => {
                let mut command = CommandBuilder::new(shell);
                command.args(&options.args);
                command
            }
            None => CommandBuilder::new_default_prog(),
        };
        command.cwd(&options.cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("TERM_PROGRAM", "neptune");
        command.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        for (key, value) in &options.env {
            command.env(key, value);
        }
        let shell = options.shell.clone().unwrap_or_else(|| command.get_shell());
        // Explicit arguments mean a caller-chosen program, such as SSH.
        #[cfg(windows)]
        if options.args.is_empty() {
            shell_integration::report_cwd(&mut command, &shell);
        }
        let mut child_guard = ChildGuard(Some(
            pair.slave.spawn_command(command).context("Start shell")?,
        ));
        let child = child_guard.0.as_ref().unwrap();
        // Keeping the slave alive prevents EOF after the child exits.
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().context("Clone PTY reader")?;
        let writer = pair.master.take_writer().context("Take PTY writer")?;

        #[cfg(unix)]
        let poll_fd = runtime::prepare_nonblocking(&*pair.master)?;

        let (input, input_rx) = mpsc::sync_channel(INPUT_QUEUE);
        let (output_tx, output_rx) = mpsc::sync_channel(OUTPUT_QUEUE);
        let (pool_tx, pool_rx) = mpsc::sync_channel(OUTPUT_QUEUE);
        let config = Config {
            scrolling_history: options.scrollback,
            kitty_keyboard: true,
            ..Config::default()
        };
        let shared = Arc::new(Shared::new(
            SessionMetadata {
                title: String::new(),
                shell,
                cwd: options.cwd,
                reported_cwd: None,
                process_id: child.process_id(),
                remote_process_id: None,
                status: SessionStatus::Running,
                bell_count: 0,
            },
            size,
            config.clone(),
            repaint,
        ));
        let proxy = EventProxy {
            shared: shared.clone(),
            input: input.clone(),
        };
        let terminal = Arc::new(Mutex::new(Term::new(config, &size, proxy)));
        let master = Arc::new(Mutex::new(Some(pair.master)));
        let session = Self {
            terminal: terminal.clone(),
            shared: shared.clone(),
            input,
            viewport_cache: Mutex::new(None),
        };

        let reader_shared = shared.clone();
        let reader_worker = runtime::Worker::new(reader_shared.clone());
        thread::Builder::new()
            .name("terminal-reader".into())
            .spawn(move || {
                let _worker = reader_worker;
                read_loop(
                    reader,
                    output_tx,
                    pool_rx,
                    reader_shared,
                    #[cfg(unix)]
                    poll_fd,
                );
            })
            .context("Start PTY reader worker")?;

        let writer_shared = shared.clone();
        let writer_worker = runtime::Worker::new(writer_shared.clone());
        let writer_master = master.clone();
        thread::Builder::new()
            .name("terminal-writer".into())
            .spawn(move || {
                let _worker = writer_worker;
                write_loop(
                    writer,
                    input_rx,
                    writer_master,
                    writer_shared,
                    #[cfg(unix)]
                    poll_fd,
                );
            })
            .context("Start PTY writer worker")?;

        let engine_worker = runtime::Worker::new(shared.clone());
        let notification_input = session.input.clone();
        thread::Builder::new()
            .name("terminal-engine".into())
            .spawn(move || {
                let _worker = engine_worker;
                engine_loop(
                    terminal,
                    output_rx,
                    pool_tx,
                    notification_input,
                    child_guard.0.take().unwrap(),
                    shared,
                    master,
                );
            })
            .context("Start VT parser worker")?;
        Ok(session)
    }

    /// Send exact bytes. On backpressure, return an error instead of blocking UI.
    /// A single call accepts at most one MiB (including paste delimiters).
    pub fn write(&self, bytes: &[u8]) -> std::result::Result<(), SessionError> {
        if bytes.is_empty() {
            return Ok(());
        }
        if bytes.len() > MAX_INPUT {
            return Err(SessionError::new(
                SessionErrorKind::Backpressure,
                "Input exceeds one MiB; split it into smaller writes",
            ));
        }
        if self.shared.stopped.load(Ordering::Acquire) {
            return Err(SessionError::new(
                SessionErrorKind::Closed,
                "Terminal session is closed",
            ));
        }
        self.shared
            .enqueue(&self.input, Input::Write(bytes.to_vec()))
    }

    /// Honor bracketed-paste mode, normalize line endings, and remove escape
    /// characters so pasted content cannot close the bracketed-paste sequence.
    pub fn paste(&self, text: &str) -> std::result::Result<(), SessionError> {
        if text.len() > MAX_INPUT - 16 {
            return Err(SessionError::new(
                SessionErrorKind::Backpressure,
                "Paste exceeds one MiB",
            ));
        }
        let bracketed = self.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let clean = text
            .replace('\u{1b}', "")
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        if bracketed {
            self.write(format!("\u{1b}[200~{clean}\u{1b}[201~").as_bytes())
        } else {
            self.write(clean.replace('\n', "\r").as_bytes())
        }
    }

    /// Resize both the VT grid and the operating-system PTY. Pixel dimensions
    /// are the complete text area, not one cell. Repeated identical calls are free.
    pub fn resize(
        &self,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> std::result::Result<(), SessionError> {
        if cols == 0 || rows == 0 || cols > 4096 || rows > 4096 {
            return Err(SessionError::new(
                SessionErrorKind::InvalidGeometry,
                "Invalid terminal dimensions",
            ));
        }
        let size = Size {
            cols,
            rows,
            pixel_width,
            pixel_height,
        };
        if *self.shared.size.lock() == size {
            return Ok(());
        }
        let config = self.shared.config.lock();
        check_grid_budget(cols, rows, config.scrolling_history)?;
        // Lock the VT grid before notifying the PTY. A child may redraw as soon
        // as TIOCSWINSZ/ResizePseudoConsole returns, so parsing that redraw must
        // wait until both the grid and size-query metadata use its new geometry.
        let mut terminal = self.terminal.lock();
        self.shared.enqueue(&self.input, Input::Resize(size))?;
        self.shared.prompt.lock().resize(&mut *terminal, size);
        *self.shared.size.lock() = size;
        self.shared.changed();
        drop(terminal);
        drop(config);
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, Term<EventProxy>> {
        self.terminal.lock()
    }
    pub fn revision(&self) -> u64 {
        self.shared.revision.load(Ordering::Acquire)
    }

    /// Acknowledge the pending redraw before reading any metadata or terminal
    /// grid for a UI frame. Call once at the start of the frame for visible
    /// sessions, never after their snapshot: output arriving after this call
    /// then requests the next frame without losing its final update. Hidden
    /// sessions can remain unacknowledged until shown; lifecycle and metadata
    /// notifications bypass coalescing so their badges still update.
    pub fn acknowledge_repaint(&self) {
        self.shared.repaint_pending.store(false, Ordering::Release);
    }
    pub fn metadata(&self) -> SessionMetadata {
        self.shared.metadata.lock().clone()
    }

    /// Drain bounded clipboard and process notification events. OSC 52 clipboard reads are disabled by
    /// the parser's default policy; renderers may handle copy events explicitly.
    pub fn drain_events(&self) -> Vec<crate::TerminalEvent> {
        let mut events = self.shared.events.lock();
        self.shared.queued_event_bytes.store(0, Ordering::Relaxed);
        events.drain(..).collect()
    }

    pub fn set_color(&self, index: usize, color: crate::Rgb) {
        if let Some(slot) = self.shared.palette.lock().get_mut(index) {
            *slot = Rgb {
                r: color.r,
                g: color.g,
                b: color.b,
            };
        }
    }

    pub fn metrics(&self) -> SessionMetrics {
        SessionMetrics {
            bytes_received: self.shared.bytes_received.load(Ordering::Relaxed),
            bytes_parsed: self.shared.bytes_parsed.load(Ordering::Relaxed),
            parse_nanoseconds: self.shared.parse_nanoseconds.load(Ordering::Relaxed),
            revision: self.revision(),
            active_workers: self.shared.active_workers.load(Ordering::Acquire),
            queued_input_bytes: self.shared.queued_input_bytes.load(Ordering::Relaxed),
            input_backpressure: self.shared.input_backpressure.load(Ordering::Relaxed),
            dropped_events: self.shared.dropped_events.load(Ordering::Relaxed),
            queued_event_bytes: self.shared.queued_event_bytes.load(Ordering::Relaxed),
            spawn_nanoseconds: self.shared.spawn_nanoseconds.load(Ordering::Relaxed),
            cleanup_nanoseconds: self.shared.cleanup_nanoseconds.load(Ordering::Relaxed),
            snapshot_nanoseconds: self.shared.snapshot_nanoseconds.load(Ordering::Relaxed),
            snapshot_lock_nanoseconds: self
                .shared
                .snapshot_lock_nanoseconds
                .load(Ordering::Relaxed),
            snapshot_cells: self.shared.snapshot_cells.load(Ordering::Relaxed),
            snapshot_rows: self.shared.snapshot_rows.load(Ordering::Relaxed),
            snapshot_count: self.shared.snapshot_count.load(Ordering::Relaxed),
        }
    }

    /// Positive deltas scroll into history; negative deltas scroll toward output.
    pub fn scroll(&self, delta: i32) {
        let mut terminal = self.lock();
        terminal.scroll_display(Scroll::Delta(delta));
        self.shared.changed();
    }

    pub fn scroll_to_bottom(&self) {
        let mut terminal = self.lock();
        terminal.scroll_display(Scroll::Bottom);
        self.shared.changed();
    }

    /// Coordinates are viewport-relative. Scrolled history is accounted for.
    pub fn start_selection(&self, row: usize, col: usize, kind: crate::SelectionType) {
        let kind = engine::selection_type(kind);
        let mut terminal = self.lock();
        let point = viewport_point(&terminal, row, col);
        terminal.selection = Some(Selection::new(kind, point, Side::Left));
        if kind == SelectionType::Semantic || kind == SelectionType::Lines {
            terminal.selection.as_mut().unwrap().include_all();
        }
        self.shared.changed();
        drop(terminal);
    }

    pub fn update_selection(&self, row: usize, col: usize) {
        let mut terminal = self.lock();
        let point = viewport_point(&terminal, row, col);
        if let Some(selection) = &mut terminal.selection {
            selection.update(point, Side::Right);
        }
        self.shared.changed();
        drop(terminal);
    }

    pub fn clear_selection(&self) {
        let mut terminal = self.lock();
        terminal.selection = None;
        self.shared.changed();
    }

    pub fn selected_text(&self) -> Option<String> {
        self.lock().selection_to_string()
    }

    pub fn set_scrollback(&self, lines: usize) -> std::result::Result<(), SessionError> {
        if lines > 1_000_000 {
            return Err(SessionError::new(
                SessionErrorKind::InvalidGeometry,
                "Scrollback exceeds one million lines",
            ));
        }
        let mut config = self.shared.config.lock();
        let size = *self.shared.size.lock();
        check_grid_budget(size.cols, size.rows, lines)?;
        config.scrolling_history = lines;
        let mut terminal = self.lock();
        terminal.set_options(config.clone());
        self.shared.changed();
        drop(terminal);
        drop(config);
        Ok(())
    }

    /// Stop without joining workers or closing ConPTY on the UI thread. The
    /// engine reaps the shell and escalates if it ignores SIGHUP; closing the
    /// last Unix PTY descriptor also hangs up its foreground process group.
    pub fn shutdown(&self) {
        let mut started = self.shared.shutdown_started.lock();
        if self.shared.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        *started = Some(Instant::now());
        drop(started);
        self.shared.force_repaint();
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Failed startup must terminate and reap a shell that was already created.
struct ChildGuard(Option<Box<dyn Child + Send + Sync>>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn viewport_point(terminal: &Term<EventProxy>, row: usize, col: usize) -> Point {
    Point::new(
        Line(row.min(terminal.screen_lines() - 1) as i32 - terminal.grid().display_offset() as i32),
        Column(col.min(terminal.columns() - 1)),
    )
}

fn default_palette() -> [Rgb; 269] {
    let mut palette = [Rgb {
        r: 211,
        g: 214,
        b: 220,
    }; 269];
    let ansi = [
        (22, 24, 29),
        (230, 126, 128),
        (160, 194, 134),
        (221, 189, 130),
        (143, 171, 217),
        (190, 158, 206),
        (129, 188, 194),
        (198, 202, 209),
        (88, 94, 105),
        (238, 154, 156),
        (180, 212, 153),
        (237, 210, 154),
        (167, 192, 232),
        (210, 180, 226),
        (156, 212, 215),
        (237, 239, 244),
    ];
    for (slot, (r, g, b)) in palette.iter_mut().zip(ansi) {
        *slot = Rgb { r, g, b };
    }
    let cube = [0, 95, 135, 175, 215, 255];
    for (index, slot) in palette.iter_mut().enumerate().take(232).skip(16) {
        let n = index - 16;
        *slot = Rgb {
            r: cube[n / 36],
            g: cube[n / 6 % 6],
            b: cube[n % 6],
        };
    }
    for (index, slot) in palette.iter_mut().enumerate().take(256).skip(232) {
        let value = 8 + (index - 232) as u8 * 10;
        *slot = Rgb {
            r: value,
            g: value,
            b: value,
        };
    }
    palette[257] = Rgb {
        r: 22,
        g: 24,
        b: 29,
    };
    palette
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

fn validate_options(options: &SessionOptions) -> std::result::Result<(), SessionError> {
    if options.cols == 0
        || options.rows == 0
        || options.cols > 4096
        || options.rows > 4096
        || options.scrollback > 1_000_000
    {
        return Err(SessionError::new(
            SessionErrorKind::InvalidGeometry,
            "Invalid terminal dimensions or history",
        ));
    }
    check_grid_budget(options.cols, options.rows, options.scrollback)?;
    if options.shell.is_none() && !options.args.is_empty() {
        return Err(SessionError::new(
            SessionErrorKind::InvalidOptions,
            "Shell arguments require an explicit shell executable",
        ));
    }
    if !options.cwd.is_dir() {
        return Err(SessionError::new(
            SessionErrorKind::InvalidOptions,
            format!(
                "Working directory does not exist: {}",
                options.cwd.display()
            ),
        ));
    }
    Ok(())
}

/// Shared lifecycle observation survives releasing the live session handle.
#[derive(Clone)]
pub struct ShutdownCompletion {
    shared: Arc<Shared>,
}
impl ShutdownCompletion {
    pub fn is_complete(&self) -> bool {
        self.shared.stopped.load(Ordering::Acquire) && self.active_workers() == 0
    }
    pub fn active_workers(&self) -> usize {
        self.shared.active_workers.load(Ordering::Acquire)
    }
    pub fn cleanup_nanoseconds(&self) -> u64 {
        self.shared.cleanup_nanoseconds.load(Ordering::Acquire)
    }
}
impl TerminalSession {
    pub fn shutdown_completion(&self) -> ShutdownCompletion {
        ShutdownCompletion {
            shared: self.shared.clone(),
        }
    }
}
