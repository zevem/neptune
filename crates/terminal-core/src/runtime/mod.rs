//! Bounded PTY transport and parser workers. No desktop thread performs process cleanup.
use super::*;
#[path = "../platform/mod.rs"]
mod platform;
#[cfg(windows)]
use platform::close_conpty;
#[cfg(unix)]
use platform::poll_pty;
use platform::process_cwd;

pub(super) struct Worker(Arc<Shared>);

impl Worker {
    pub(super) fn new(shared: Arc<Shared>) -> Self {
        shared.active_workers.fetch_add(1, Ordering::AcqRel);
        Self(shared)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let mut workers = self.0.active_workers.load(Ordering::Acquire);
        loop {
            if workers == 1
                && let Some(started) = *self.0.shutdown_started.lock()
            {
                self.0
                    .cleanup_nanoseconds
                    .store(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
            }
            match self.0.active_workers.compare_exchange_weak(
                workers,
                workers - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    if workers == 1 {
                        self.0.force_repaint();
                    }
                    break;
                }
                Err(current) => workers = current,
            }
        }
    }
}

pub(super) fn read_loop(
    mut reader: Box<dyn Read + Send>,
    output: SyncSender<Output>,
    pool: Receiver<Vec<u8>>,
    shared: Arc<Shared>,
    #[cfg(unix)] poll_fd: libc::c_int,
) {
    let mut buffer = vec![0; READ_SIZE];
    #[cfg(windows)]
    let mut output_disconnected = false;
    loop {
        #[cfg(not(windows))]
        if shared.stopped.load(Ordering::Acquire) {
            break;
        }
        match reader.read(&mut buffer) {
            Ok(0) => {
                let _ = output.send(Output::End);
                break;
            }
            Ok(len) => {
                shared
                    .bytes_received
                    .fetch_add(len as u64, Ordering::Relaxed);
                #[cfg(windows)]
                if output_disconnected {
                    // ClosePseudoConsole may wait for its output pipe to drain.
                    // Even after an engine failure, finish draining that pipe.
                    continue;
                }
                match output.send(Output::Bytes(buffer, len)) {
                    Ok(()) => buffer = pool.try_recv().unwrap_or_else(|_| vec![0; READ_SIZE]),
                    Err(mpsc::SendError(Output::Bytes(returned, _))) => {
                        #[cfg(windows)]
                        {
                            buffer = returned;
                            output_disconnected = true;
                        }
                        #[cfg(not(windows))]
                        {
                            drop(returned);
                            break;
                        }
                    }
                    Err(_) => unreachable!("Only an output chunk was sent"),
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            #[cfg(unix)]
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if !poll_pty(poll_fd, libc::POLLIN, &shared) {
                    break;
                }
            }
            Err(error) => {
                let _ = output.send(Output::Error(error));
                break;
            }
        }
    }
}

pub(super) fn write_loop(
    mut writer: Box<dyn Write + Send>,
    input: Receiver<Input>,
    master: Master,
    shared: Arc<Shared>,
    #[cfg(unix)] poll_fd: libc::c_int,
) {
    while !shared.stopped.load(Ordering::Acquire) {
        let command = match input.recv_timeout(Duration::from_millis(100)) {
            Ok(command) => command,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let result = match command {
            Input::Resize(size) => master
                .lock()
                .as_ref()
                .map(|master| master.resize(size.pty()))
                .unwrap_or(Ok(())),
            Input::Write(bytes) => {
                shared
                    .queued_input_bytes
                    .fetch_sub(bytes.len(), Ordering::AcqRel);
                let mut remaining = bytes.as_slice();
                let mut result = Ok(());
                while !remaining.is_empty() && !shared.stopped.load(Ordering::Acquire) {
                    match writer.write(&remaining[..remaining.len().min(8192)]) {
                        Ok(0) => {
                            result = Err(io::Error::from(io::ErrorKind::WriteZero).into());
                            break;
                        }
                        Ok(len) => remaining = &remaining[len..],
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        #[cfg(unix)]
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            if !poll_pty(poll_fd, libc::POLLOUT, &shared) {
                                break;
                            }
                        }
                        Err(error) => {
                            result = Err(error.into());
                            break;
                        }
                    }
                }
                result
            }
        };
        if let Err(error) = result {
            shared.error(format!("PTY write: {error}"));
            break;
        }
    }
    for command in input.try_iter() {
        if let Input::Write(bytes) = command {
            shared
                .queued_input_bytes
                .fetch_sub(bytes.len(), Ordering::AcqRel);
        }
    }
}

pub(super) fn engine_loop(
    terminal: Arc<Mutex<Term<EventProxy>>>,
    output: Receiver<Output>,
    pool: SyncSender<Vec<u8>>,
    notification_input: SyncSender<Input>,
    mut child: Box<dyn Child + Send + Sync>,
    shared: Arc<Shared>,
    master: Master,
) {
    let mut processor: Processor = Processor::new();
    let mut cwd_tracker = CwdTracker::default();
    let mut prompt_scanner = PromptScanner::default();
    let mut notifications = notifications::Scanner::default();
    let mut eof = false;
    let mut exit = None;
    let mut last_cwd = Instant::now();
    let mut last_child_poll = Instant::now();
    #[cfg(windows)]
    let mut conpty_close_started = false;
    loop {
        if shared.stopped.load(Ordering::Acquire) {
            #[cfg(not(windows))]
            break;
            #[cfg(windows)]
            if exit.is_none() {
                let _ = child.kill();
                if let Ok(status) = child.wait() {
                    exit = Some(status);
                }
            }
        }
        #[cfg(windows)]
        if exit.is_some() && !conpty_close_started {
            close_conpty(&master, &shared);
            conpty_close_started = true;
        }
        let timeout = processor
            .sync_timeout()
            .sync_timeout()
            .map(|deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(100))
            })
            .unwrap_or(Duration::from_millis(100));
        match output.recv_timeout(timeout) {
            Ok(Output::Bytes(buffer, len)) => {
                for chunk in buffer[..len].chunks(PARSE_SLICE) {
                    notifications.advance(chunk, |command| match command {
                        notifications::Command::Event(event) => shared.push_event(event),
                        notifications::Command::Reply(text) => {
                            // Use the same bounded writer as terminal protocol replies.
                            let _ = shared
                                .enqueue(&notification_input, Input::Write(text.into_bytes()));
                        }
                    });
                    let marks = prompt_scanner.advance(chunk);
                    let mut grid = terminal.lock();
                    let was_reporting_focus = grid.mode().contains(TermMode::FOCUS_IN_OUT);
                    let started = Instant::now();
                    let mut offset = 0;
                    for (end, mark) in marks {
                        processor.advance(&mut *grid, &chunk[offset..end]);
                        let mut prompt = shared.prompt.lock();
                        if processor.sync_bytes_count() == 0 {
                            prompt.mark(&*grid, mark);
                        } else {
                            // A synchronized frame has not reached the grid
                            // yet. Its raw marker cannot identify a grid row.
                            prompt.anchor = None;
                        }
                        offset = end;
                    }
                    processor.advance(&mut *grid, &chunk[offset..]);
                    report_focus_on_enable(
                        &grid,
                        was_reporting_focus,
                        &shared,
                        &notification_input,
                    );
                    // Commit visible changes before releasing their grid lock.
                    if processor.sync_bytes_count() < chunk.len() {
                        shared.changed();
                    }
                    shared
                        .parse_nanoseconds
                        .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
                }
                if let Some(cwd) = cwd_tracker.advance(&buffer[..len]) {
                    let mut metadata = shared.metadata.lock();
                    if metadata.reported_cwd.as_ref() != Some(&cwd) || metadata.cwd != cwd {
                        metadata.reported_cwd = Some(cwd.clone());
                        metadata.cwd = cwd;
                        drop(metadata);
                        shared.force_repaint();
                    }
                }
                if let Some(pid) = cwd_tracker.remote_pid.take() {
                    let mut metadata = shared.metadata.lock();
                    if metadata.remote_process_id != Some(pid) {
                        metadata.remote_process_id = Some(pid);
                        drop(metadata);
                        shared.force_repaint();
                    }
                }
                shared.bytes_parsed.fetch_add(len as u64, Ordering::Relaxed);
                let _ = pool.try_send(buffer);
                // Follow Alacritty's wakeup policy: a synchronized application
                // frame should not rebuild visible rows for every buffered chunk.
            }
            Ok(Output::End) => eof = true,
            Ok(Output::Error(error)) => {
                shared.error(format!("PTY read: {error}"));
                eof = true;
            }
            Err(RecvTimeoutError::Disconnected) => eof = true,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if processor
            .sync_timeout()
            .sync_timeout()
            .is_some_and(|deadline| deadline <= Instant::now())
        {
            let mut grid = terminal.lock();
            let was_reporting_focus = grid.mode().contains(TermMode::FOCUS_IN_OUT);
            processor.stop_sync(&mut *grid);
            report_focus_on_enable(&grid, was_reporting_focus, &shared, &notification_input);
            shared.changed();
        }
        // Small PTY reads should not each cause a waitpid/WaitForSingleObject
        // syscall. EOF still polls immediately, and busy or idle streams detect
        // exit within the same 100ms bound used by the receive timeout.
        if exit.is_none() && (eof || last_child_poll.elapsed() >= Duration::from_millis(100)) {
            match child.try_wait() {
                Ok(status) => exit = status,
                Err(error) => {
                    shared.error(format!("Wait for shell: {error}"));
                    break;
                }
            }
            last_child_poll = Instant::now();
        }
        #[cfg(windows)]
        if exit.is_some() && !conpty_close_started {
            close_conpty(&master, &shared);
            conpty_close_started = true;
        }
        if exit.is_some() && eof {
            break;
        }
        let check = shared.process_check.lock().take();
        if let Some(reply) = check {
            // This worker owns/reaps the child. Never inspect a PID after it
            // has been reaped and could have been reused by the OS.
            let activity = if exit.is_some() {
                ProcessActivity::Unknown
            } else {
                let pid = shared.metadata.lock().process_id;
                let master = master.lock();
                match (pid, master.as_ref()) {
                    (Some(pid), Some(master)) => platform::process_activity(pid, &**master),
                    _ => ProcessActivity::Unknown,
                }
            };
            let _ = reply.try_send(activity);
            shared.force_repaint();
        }
        if last_cwd.elapsed() >= Duration::from_secs(1) {
            let pid = shared.metadata.lock().process_id;
            if let Some(pid) = pid
                && let Some(cwd) = process_cwd(pid)
            {
                let mut metadata = shared.metadata.lock();
                if metadata.cwd != cwd {
                    metadata.cwd = cwd;
                    drop(metadata);
                    shared.changed_force();
                }
            }
            last_cwd = Instant::now();
        }
        // EOF can precede the wait result by a few milliseconds. Avoid spinning
        // on a disconnected output receiver while the OS reaps the child.
        if eof && exit.is_none() {
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[cfg(not(windows))]
    {
        // BSD slave close can wait for terminal output to drain. Once parsing
        // stops, release blocked sends and every master handle before reaping
        // the child; otherwise wait() can outlive the retiring I/O workers.
        shared.stopped.store(true, Ordering::Release);
        drop(output);
        master.lock().take();
    }
    if exit.is_none() {
        // The worker owns the authoritative child handle, so a stale cloned
        // process id can never signal a process after the shell was reaped.
        let _ = child.kill();
        if let Ok(status) = child.wait() {
            exit = Some(status);
        }
    }
    {
        let mut grid = terminal.lock();
        processor.stop_sync(&mut *grid);
        shared.changed();
    }
    if let Some(status) = exit {
        shared.metadata.lock().status = SessionStatus::Exited {
            code: status.exit_code(),
            signal: status.signal().map(str::to_owned),
        };
    }
    shared.stopped.store(true, Ordering::Release);
    if let Some(reply) = shared.process_check.lock().take() {
        let _ = reply.try_send(ProcessActivity::Idle);
    }
    let mut shutdown_started = shared.shutdown_started.lock();
    if shutdown_started.is_none() {
        *shutdown_started = Some(Instant::now());
    }
    drop(shutdown_started);
    // A reader blocked on the bounded output channel must be released before
    // ClosePseudoConsole, which can wait for that reader to drain its pipe.
    #[cfg(windows)]
    drop(output);
    #[cfg(windows)]
    if !conpty_close_started {
        close_conpty(&master, &shared);
    }
    shared.changed_force();
}

fn report_focus_on_enable(
    terminal: &Term<EventProxy>,
    was_reporting: bool,
    shared: &Shared,
    input: &SyncSender<Input>,
) {
    if !was_reporting && terminal.mode().contains(TermMode::FOCUS_IN_OUT) {
        // TUIs start assuming focus. A pane may have lost it before the
        // program requested reports, so supply its actual state on enable.
        let bytes = if terminal.is_focused {
            b"\x1b[I"
        } else {
            b"\x1b[O"
        };
        if let Err(error) = shared.enqueue(input, Input::Write(bytes.to_vec())) {
            shared.error(error);
        }
    }
}

#[cfg(unix)]
pub(super) fn prepare_nonblocking(master: &dyn MasterPty) -> Result<libc::c_int> {
    platform::prepare_nonblocking(master)
}
