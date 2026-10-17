//! Neptune patch: the native window of each viewport.
//!
//! Platform integrations sometimes need a child viewport's own native window,
//! for example a Wayland `xdg_toplevel` to attach to a window drag. Upstream
//! eframe exposes only the root window, through [`crate::Frame::winit_window`].

use std::sync::{Arc, Mutex, Weak};

use egui::ViewportId;
use winit::window::Window;

static WINDOWS: Mutex<Vec<(ViewportId, Weak<Window>)>> = Mutex::new(Vec::new());

pub(crate) fn register(viewport: ViewportId, window: &Arc<Window>) {
    let mut windows = WINDOWS.lock().unwrap_or_else(|poison| poison.into_inner());
    windows.retain(|(id, window)| *id != viewport && window.strong_count() > 0);
    windows.push((viewport, Arc::downgrade(window)));
}

/// The native window currently showing `viewport`, if eframe has created it.
///
/// The window is only borrowed: eframe closes it when the viewport is no
/// longer shown, whoever still holds this handle.
pub fn viewport_window(viewport: ViewportId) -> Option<Arc<Window>> {
    let mut windows = WINDOWS.lock().unwrap_or_else(|poison| poison.into_inner());
    windows.retain(|(_, window)| window.strong_count() > 0);
    windows
        .iter()
        .find(|(id, _)| *id == viewport)
        .and_then(|(_, window)| window.upgrade())
}
