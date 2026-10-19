//! Observe a held workspace gesture across sidebar owners. Its pressed native
//! surface stays alive through each transfer; widget drag identities change.
//! Only an active gesture observes the desktop pointer; no terminal input or
//! contents are collected.
//!
//! Wayland reports neither the desktop pointer nor window positions. There
//! the compositor carries the held window as a drag-and-drop transfer, which
//! tells each of Neptune's windows where the workspace is held over it.

use eframe::egui::{self, Pos2};
#[cfg(any(not(target_os = "macos"), test))]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

#[derive(Clone, Copy)]
pub(crate) struct Pointer {
    /// Top-left desktop coordinates: screen points on macOS, physical pixels
    /// on X11. A macOS display scales its window coordinates independently;
    /// multiplying them by Retina scale would break a shared desktop space.
    pub position: Pos2,
    pub primary_down: bool,
}

/// Convert a viewport's egui points into the pointer's desktop coordinates.
pub(crate) fn desktop_scale(native_pixels_per_point: f32, zoom: f32) -> f32 {
    if cfg!(target_os = "macos") {
        zoom
    } else {
        native_pixels_per_point * zoom
    }
}

#[derive(Default)]
pub(crate) struct Monitor {
    #[cfg(target_os = "linux")]
    window: Option<u32>,
    #[cfg(target_os = "linux")]
    seat: Option<crate::platform::wayland_seat::Seat>,
    #[cfg(target_os = "linux")]
    wayland: bool,
    /// Native windows the seat has renewed its data device after.
    #[cfg(target_os = "linux")]
    windows: Vec<winit::window::WindowId>,
}

/// Where the compositor reports a carried workspace, in the viewport's
/// local logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(
    not(target_os = "linux"),
    expect(dead_code, reason = "Only the Wayland seat carries a held workspace")
)]
pub(crate) enum Carried {
    Over {
        viewport: Option<egui::ViewportId>,
        position: Pos2,
    },
    Away,
    Released,
    Ended,
}

#[cfg(target_os = "linux")]
fn surface(viewport: egui::ViewportId) -> Option<wayland_backend::client::ObjectId> {
    use eframe::wgpu::rwh::HasWindowHandle as _;
    let window = eframe::viewport_window(viewport)?;
    let handle = window.window_handle().ok()?;
    crate::platform::wayland_seat::surface_of(&handle.as_raw())
}

pub(crate) struct Watch(Observation);

enum Observation {
    #[cfg(any(not(target_os = "macos"), test))]
    #[cfg_attr(
        not(any(target_os = "linux", target_os = "macos")),
        expect(dead_code, reason = "This backend has no desktop pointer observer")
    )]
    Samples {
        samples: mpsc::Receiver<Result<Pointer, ()>>,
        cancelled: Arc<AtomicBool>,
    },
    #[cfg(target_os = "macos")]
    Desktop,
}

impl Drop for Watch {
    fn drop(&mut self) {
        match &self.0 {
            #[cfg(any(not(target_os = "macos"), test))]
            Observation::Samples { cancelled, .. } => cancelled.store(true, Ordering::Relaxed),
            #[cfg(target_os = "macos")]
            Observation::Desktop => {}
        }
    }
}

impl Watch {
    pub fn poll(&self) -> Result<Option<Pointer>, ()> {
        match &self.0 {
            #[cfg(any(not(target_os = "macos"), test))]
            Observation::Samples { samples, .. } => match samples.try_recv() {
                Ok(sample) => sample.map(Some),
                Err(mpsc::TryRecvError::Empty) => Ok(None),
                Err(mpsc::TryRecvError::Disconnected) => Err(()),
            },
            #[cfg(target_os = "macos")]
            Observation::Desktop => macos::pointer().map(Some),
        }
    }

    #[cfg(all(test, target_os = "macos"))]
    pub(crate) fn test_samples() -> (mpsc::SyncSender<Result<Pointer, ()>>, Self) {
        let (sender, samples) = mpsc::sync_channel(1);
        (
            sender,
            Self(Observation::Samples {
                samples,
                cancelled: Arc::new(AtomicBool::new(false)),
            }),
        )
    }
}

impl Monitor {
    pub fn attach(&mut self, frame: &eframe::Frame, ctx: &egui::Context) {
        #[cfg(target_os = "linux")]
        if self.window.is_none() && !self.wayland {
            use eframe::wgpu::rwh::{
                HasDisplayHandle as _, HasWindowHandle as _, RawDisplayHandle, RawWindowHandle,
            };
            let (Ok(display), Ok(window)) = (frame.display_handle(), frame.window_handle()) else {
                return;
            };
            match (display.as_raw(), window.as_raw()) {
                (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(window)) => {
                    self.wayland = true;
                    self.seat = crate::platform::wayland_seat::Seat::connect(
                        display.display.as_ptr() as usize,
                        window.surface.as_ptr() as usize,
                        ctx,
                    );
                }
                (_, RawWindowHandle::Xlib(window)) => {
                    self.window = u32::try_from(window.window).ok()
                }
                (_, RawWindowHandle::Xcb(window)) => self.window = Some(window.window.get()),
                _ => {}
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (frame, ctx);
    }

    /// Keep drag-and-drop on the seat after the toolkit creates windows.
    pub fn notice_windows(&mut self, viewports: impl IntoIterator<Item = egui::ViewportId>) {
        #[cfg(target_os = "linux")]
        {
            let Some(seat) = &mut self.seat else {
                return;
            };
            let windows: Vec<_> = viewports
                .into_iter()
                .filter_map(eframe::viewport_window)
                .map(|window| window.id())
                .collect();
            if windows.iter().any(|window| !self.windows.contains(window)) {
                seat.renew_device();
            }
            self.windows = windows;
        }
        #[cfg(not(target_os = "linux"))]
        let _ = viewports;
    }

    /// Files the seat receives for the root window, for their one reader.
    pub fn take_file_signals(&mut self) -> crate::platform::file_drag::SeatFiles {
        #[cfg(target_os = "linux")]
        {
            self.seat.as_mut().and_then(|seat| seat.take_files())
        }
        #[cfg(not(target_os = "linux"))]
        None
    }

    /// Whether the compositor carries held windows between Neptune's
    /// windows, where the application cannot position them.
    pub fn carries(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.seat.as_ref().is_some_and(|seat| seat.moves_windows())
        }
        #[cfg(not(target_os = "linux"))]
        false
    }

    /// Carry the workspace held in `origin`, if that window holds the press.
    /// A `continuing` press began on a window that has since closed.
    pub fn start_carry(&mut self, origin: egui::ViewportId, continuing: bool) -> bool {
        #[cfg(target_os = "linux")]
        {
            let (Some(seat), Some(surface)) = (&mut self.seat, surface(origin)) else {
                return false;
            };
            seat.moves_windows() && seat.start_workspace_drag(&surface, continuing)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (origin, continuing);
            false
        }
    }

    /// Move the native window of `viewport` with the carried workspace.
    pub fn attach_carried(&self, viewport: egui::ViewportId, offset: egui::Vec2) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.seat.as_ref().is_some_and(|seat| {
                eframe::viewport_window(viewport).is_some_and(|window| seat.attach(&window, offset))
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (viewport, offset);
            false
        }
    }

    /// What happened to the carried workspace, with surfaces resolved among
    /// `viewports`.
    pub fn carried(&self, viewports: &[egui::ViewportId]) -> Vec<Carried> {
        #[cfg(target_os = "linux")]
        {
            use crate::platform::wayland_seat::WorkspaceSignal;
            let Some(seat) = &self.seat else {
                return Vec::new();
            };
            let signals = seat.workspace_signals();
            if signals.is_empty() {
                return Vec::new();
            }
            let surfaces: Vec<_> = viewports
                .iter()
                .filter_map(|viewport| Some((surface(*viewport)?, *viewport)))
                .collect();
            signals
                .into_iter()
                .map(|signal| match signal {
                    WorkspaceSignal::Over { surface, position } => Carried::Over {
                        viewport: surfaces
                            .iter()
                            .find(|(known, _)| *known == surface)
                            .map(|(_, viewport)| *viewport),
                        position,
                    },
                    WorkspaceSignal::Away => Carried::Away,
                    WorkspaceSignal::Released => Carried::Released,
                    WorkspaceSignal::Ended => Carried::Ended,
                })
                .collect()
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = viewports;
            Vec::new()
        }
    }

    pub fn finish_carry(&self) {
        #[cfg(target_os = "linux")]
        if let Some(seat) = &self.seat {
            seat.finish_workspace_drag();
        }
    }

    /// A Wayland root that could not reach its seat.
    #[cfg(all(test, target_os = "linux"))]
    pub fn wayland_without_seat() -> Self {
        Self {
            wayland: true,
            ..Default::default()
        }
    }

    /// Whether an empty native root may stay hidden beside occupied windows.
    /// Wayland cannot hide it; on macOS ordering it out retains an empty
    /// NSWindow. Reuse the root and destroy the spare viewport on both.
    pub fn keeps_hidden_root(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            !self.wayland
        }
        #[cfg(target_os = "macos")]
        {
            false
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            true
        }
    }

    pub fn watch(&self, ctx: &egui::Context) -> Option<Watch> {
        #[cfg(target_os = "linux")]
        {
            let window = self.window?;
            let (sender, samples) = mpsc::sync_channel(1);
            let cancelled = Arc::new(AtomicBool::new(false));
            let stop = cancelled.clone();
            let wake = ctx.clone();
            std::thread::Builder::new()
                .name("neptune-workspace-drag".into())
                .spawn(move || {
                    let result = linux::observe(window, &sender, &stop, &wake);
                    if result.is_err() && !stop.load(Ordering::Relaxed) {
                        let _ = sender.send(Err(()));
                        wake.request_repaint_of(egui::ViewportId::ROOT);
                    }
                })
                .ok()?;
            Some(Watch(Observation::Samples { samples, cancelled }))
        }
        #[cfg(target_os = "macos")]
        {
            // The gesture's existing 16 ms repaint deadline drives polling.
            // No global event tap, worker or idle pointer observation is needed.
            let _ = ctx;
            Some(Watch(Observation::Desktop))
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = ctx;
            None
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::ffi::c_void;

    #[repr(C)]
    struct Point {
        x: f64,
        y: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreate(source: *const c_void) -> *const c_void;
        fn CGEventGetLocation(event: *const c_void) -> Point;
        fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: *const c_void);
    }

    pub(super) fn pointer() -> Result<Pointer, ()> {
        // SAFETY: A null source requests the current session's default event.
        // Check the owned Create result, read it, and release it exactly once.
        // These read-only queries neither install an event tap nor post input.
        let (position, primary_down) = unsafe {
            let event = CGEventCreate(std::ptr::null());
            if event.is_null() {
                return Err(());
            }
            let position = CGEventGetLocation(event);
            // kCGEventSourceStateCombinedSessionState, kCGMouseButtonLeft.
            let primary_down = CGEventSourceButtonState(0, 0);
            CFRelease(event);
            (
                Pos2::new(position.x as f32, position.y as f32),
                primary_down,
            )
        };
        if !position.x.is_finite() || !position.y.is_finite() {
            return Err(());
        }
        Ok(Pointer {
            position,
            primary_down,
        })
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use x11rb::protocol::xproto::{ConnectionExt as _, KeyButMask};

    pub(super) fn observe(
        window: u32,
        sender: &mpsc::SyncSender<Result<Pointer, ()>>,
        stop: &AtomicBool,
        wake: &egui::Context,
    ) -> Result<(), ()> {
        // Connect and wait for display replies off the interactive thread.
        let (connection, _) = x11rb::connect(None).map_err(|_| ())?;
        let root = connection
            .query_tree(window)
            .map_err(|_| ())?
            .reply()
            .map_err(|_| ())?
            .root;
        while !stop.load(Ordering::Relaxed) {
            let reply = connection
                .query_pointer(root)
                .map_err(|_| ())?
                .reply()
                .map_err(|_| ())?;
            if !reply.same_screen {
                return Err(());
            }
            let pointer = Pointer {
                position: Pos2::new(f32::from(reply.root_x), f32::from(reply.root_y)),
                primary_down: reply.mask.contains(KeyButMask::BUTTON1),
            };
            if !pointer.primary_down {
                // Motion can coalesce; the final release must reach the owner.
                let _ = sender.send(Ok(pointer));
                wake.request_repaint_of(egui::ViewportId::ROOT);
                return Ok(());
            }
            match sender.try_send(Ok(pointer)) {
                Ok(()) => wake.request_repaint_of(egui::ViewportId::ROOT),
                Err(mpsc::TrySendError::Full(_)) => {}
                Err(mpsc::TrySendError::Disconnected(_)) => return Ok(()),
            }
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        Ok(())
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn a_macos_workspace_drag_can_observe_the_press_after_leaving_its_window() {
        let ctx = egui::Context::default();
        let watch = Monitor::default().watch(&ctx);
        assert!(
            watch.is_some(),
            "macOS must observe the desktop press to move a held workspace back into another window"
        );
        let pointer = watch.unwrap().poll().unwrap().unwrap();
        assert!(pointer.position.x.is_finite() && pointer.position.y.is_finite());
    }
}
