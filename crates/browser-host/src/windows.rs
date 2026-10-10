//! CEF's signed sandbox bootstrap loads this DLL before initializing Chromium.
#[cfg(windows)]
#[path = "main.rs"]
mod engine;

#[cfg(windows)]
#[unsafe(export_name = "RunConsoleMain")]
unsafe extern "C" fn run_console_main(
    _argc: i32,
    _argv: *mut *mut std::ffi::c_char,
    sandbox_info: *mut u8,
    _version_info: *mut std::ffi::c_void,
) -> i32 {
    if sandbox_info.is_null() {
        return 1;
    }
    // No panic crosses the bootstrap ABI. Details and browsing contents never
    // enter Neptune's diagnostics or stdout (which is the private wire pipe).
    match std::panic::catch_unwind(|| engine::run(sandbox_info)) {
        Ok(Ok(())) => 0,
        _ => 1,
    }
}
