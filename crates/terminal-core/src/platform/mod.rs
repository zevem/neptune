//! Platform transport, process metadata, and ConPTY teardown.
use super::*;

mod processes;
pub(super) use processes::process_activity;
#[cfg(unix)]
pub(in crate::session) use processes::shell_activity;

#[cfg(unix)]
pub(in crate::session) fn poll_pty(
    fd: libc::c_int,
    events: libc::c_short,
    shared: &Shared,
) -> bool {
    while !shared.stopped.load(Ordering::Acquire) {
        let mut descriptor = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut descriptor, 1, 100) };
        if result > 0 {
            return !shared.stopped.load(Ordering::Acquire);
        }
        if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return false;
        }
    }
    false
}

/// ConPTY remains open after its final child exits. Closing it can wait for VT
/// output draining, so neither the parser nor the reader can perform that close.
/// See <https://learn.microsoft.com/en-us/windows/console/closepseudoconsole>.
#[cfg(windows)]
pub(super) fn close_conpty(master: &Master, shared: &Arc<Shared>) {
    let Some(master) = master.lock().take() else {
        return;
    };
    let shared = shared.clone();
    let error_shared = shared.clone();
    let worker = Worker::new(shared.clone());
    if let Err(error) = thread::Builder::new()
        .name("terminal-conpty-close".into())
        .spawn(move || {
            let _worker = worker;
            drop(master);
        })
    {
        error_shared.error(format!("Start ConPTY close worker: {error}"));
    }
}

#[cfg(target_os = "linux")]
pub(in crate::session) fn process_cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}
#[cfg(not(target_os = "linux"))]
pub(in crate::session) fn process_cwd(_pid: u32) -> Option<PathBuf> {
    None
}

#[cfg(unix)]
pub(super) fn prepare_nonblocking(master: &dyn MasterPty) -> Result<libc::c_int> {
    let fd = master.as_raw_fd().context("PTY has no native descriptor")?;
    // Clones share the open-file description. Polling makes idle reads/pastes
    // interruptible without a second reader or a signal from the GUI thread.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        bail!("Set nonblocking PTY: {}", io::Error::last_os_error());
    }
    Ok(fd)
}
