//! Observe a held workspace gesture across sidebar owners. Its pressed native
//! surface stays alive through each transfer; widget drag identities change.
//! Only an active gesture observes the desktop pointer; no terminal input or
//! contents are collected.

use eframe::egui::{self, Pos2};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

#[derive(Clone, Copy)]
pub(crate) struct Pointer {
    /// Desktop coordinates in physical pixels.
    pub position: Pos2,
    pub primary_down: bool,
}

#[derive(Default)]
pub(crate) struct Monitor {
    #[cfg(target_os = "linux")]
    window: Option<u32>,
}

pub(crate) struct Watch {
    samples: mpsc::Receiver<Result<Pointer, ()>>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Watch {
    pub fn poll(&self) -> Result<Option<Pointer>, ()> {
        match self.samples.try_recv() {
            Ok(sample) => sample.map(Some),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(()),
        }
    }
}

impl Monitor {
    pub fn attach(&mut self, frame: &eframe::Frame) {
        #[cfg(target_os = "linux")]
        if self.window.is_none() {
            use eframe::wgpu::rwh::{HasWindowHandle as _, RawWindowHandle};
            self.window = frame
                .window_handle()
                .ok()
                .and_then(|handle| match handle.as_raw() {
                    RawWindowHandle::Xlib(window) => u32::try_from(window.window).ok(),
                    RawWindowHandle::Xcb(window) => Some(window.window.get()),
                    _ => None,
                });
        }
        #[cfg(not(target_os = "linux"))]
        let _ = frame;
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
            Some(Watch { samples, cancelled })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = ctx;
            None
        }
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
