//! Close-time observations only. No command lines, environment, or output are read.
use super::*;

pub(in crate::session) fn process_activity(pid: u32, master: &dyn MasterPty) -> ProcessActivity {
    #[cfg(unix)]
    let foreground = master.as_raw_fd().and_then(|fd| {
        // The master stays locked/open for the duration of this observation.
        let group = unsafe { libc::tcgetpgrp(fd) };
        let shell_group = unsafe { libc::getpgid(pid as libc::pid_t) };
        (group > 0 && shell_group > 0).then_some(group != shell_group)
    });
    #[cfg(not(unix))]
    let foreground = {
        let _ = master;
        Some(false)
    };
    if foreground == Some(true) {
        return ProcessActivity::Running;
    }
    let (children, executable) = inspect_process(pid);
    if children == Some(true) {
        return ProcessActivity::Running;
    }
    match executable {
        Some(executable) if !is_shell(&executable) => ProcessActivity::Running,
        Some(_) if foreground == Some(false) && children == Some(false) => ProcessActivity::Idle,
        _ => ProcessActivity::Unknown,
    }
}

/// A program on this host whose terminal another process holds, such as a
/// session daemon's. Without its foreground group, a shell with no child
/// process is at its prompt.
#[cfg(unix)]
pub(in crate::session) fn shell_activity(pid: u32) -> ProcessActivity {
    match inspect_process(pid) {
        (Some(true), _) => ProcessActivity::Running,
        (_, Some(executable)) if !is_shell(&executable) => ProcessActivity::Running,
        (Some(false), Some(_)) => ProcessActivity::Idle,
        _ => ProcessActivity::Unknown,
    }
}

fn is_shell(path: &std::path::Path) -> bool {
    let name = path.file_stem().unwrap_or_default().to_string_lossy();
    matches!(
        name.to_ascii_lowercase().as_str(),
        "sh" | "bash"
            | "dash"
            | "zsh"
            | "fish"
            | "ksh"
            | "ksh93"
            | "mksh"
            | "csh"
            | "tcsh"
            | "nu"
            | "pwsh"
            | "powershell"
            | "cmd"
    )
}

#[cfg(target_os = "linux")]
fn inspect_process(pid: u32) -> (Option<bool>, Option<PathBuf>) {
    // Children may be created by any shell thread (for example in fish).
    // Read one byte per task, never allocate an unbounded child PID list.
    let children = linux_children(pid);
    let executable = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
    (children, executable)
}

#[cfg(target_os = "linux")]
fn linux_children(pid: u32) -> Option<bool> {
    let started = Instant::now();
    let tasks = std::fs::read_dir(format!("/proc/{pid}/task")).ok()?;
    let mut complete = true;
    let mut count = 0;
    for task in tasks {
        count += 1;
        if count > 1024 || started.elapsed() > Duration::from_millis(100) {
            return None;
        }
        let children = task.ok().and_then(|task| {
            std::fs::File::open(task.path().join("children"))
                .and_then(|mut file| file.read(&mut [0_u8; 1]))
                .ok()
        });
        match children {
            Some(0) => {}
            Some(_) => return Some(true),
            None => complete = false,
        }
    }
    (complete && count > 0).then_some(false)
}

#[cfg(target_os = "macos")]
fn inspect_process(pid: u32) -> (Option<bool>, Option<PathBuf>) {
    use std::os::unix::ffi::OsStringExt;
    let mut child = 0_i32;
    // libproc can return zero for both errors and an empty list. Clear/check
    // errno so a denied query never silently becomes an idle shell.
    let children = unsafe {
        *libc::__error() = 0;
        let count = libc::proc_listchildpids(
            pid as i32,
            (&mut child as *mut i32).cast(),
            std::mem::size_of_val(&child) as i32,
        );
        if count > 0 {
            Some(true)
        } else if count == 0 && *libc::__error() == 0 {
            Some(false)
        } else {
            None
        }
    };
    let mut buffer = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let length =
        unsafe { libc::proc_pidpath(pid as i32, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    let executable = (length > 0).then(|| {
        buffer.truncate(length as usize);
        PathBuf::from(std::ffi::OsString::from_vec(buffer))
    });
    (children, executable)
}

#[cfg(windows)]
fn inspect_process(pid: u32) -> (Option<bool>, Option<PathBuf>) {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_NO_MORE_FILES, GetLastError, INVALID_HANDLE_VALUE},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
    };
    // A single bounded traversal, only when a close is requested. The snapshot
    // owns the enumeration; it does not open or signal unrelated processes.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return (None, None);
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        let mut found_child = false;
        let mut executable = None;
        let mut present = Process32FirstW(snapshot, &mut entry) != 0;
        let mut inspected = 0;
        while present && inspected < 65_536 {
            inspected += 1;
            found_child |= entry.th32ParentProcessID == pid;
            if entry.th32ProcessID == pid {
                let length = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                executable = Some(PathBuf::from(std::ffi::OsString::from_wide(
                    &entry.szExeFile[..length],
                )));
            }
            present = Process32NextW(snapshot, &mut entry) != 0;
        }
        let complete = !present && GetLastError() == ERROR_NO_MORE_FILES;
        CloseHandle(snapshot);
        (
            if found_child {
                Some(true)
            } else if complete {
                Some(false)
            } else {
                None
            },
            executable,
        )
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn inspect_process(_pid: u32) -> (Option<bool>, Option<PathBuf>) {
    (None, None)
}
