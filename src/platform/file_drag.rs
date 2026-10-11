//! Files dragged over the window from another application.
//!
//! The window toolkit reports that files hover and which were dropped, but not
//! where they are held, and on Wayland it reports nothing at all. Linux fills
//! both gaps from the display server.

use eframe::egui::{self, Pos2};
use std::path::PathBuf;

/// What a file drag has done by this frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileDrag {
    pub hovering: bool,
    /// Where the files are held, on platforms that say.
    pub pointer: Option<Pos2>,
    /// Files released over the window since the last frame.
    pub dropped: Vec<PathBuf>,
}

#[derive(Default)]
pub struct FileDragSource {
    #[cfg(target_os = "linux")]
    display: Option<linux::Display>,
    #[cfg(target_os = "linux")]
    attached: bool,
}

impl FileDragSource {
    /// A source that never asks the display server, for native test windows.
    #[cfg(test)]
    pub fn detached() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            display: None,
            #[cfg(target_os = "linux")]
            attached: true,
        }
    }

    /// Connects to the window's display server once the window exists.
    pub fn attach(&mut self, frame: &eframe::Frame, ctx: &egui::Context) {
        #[cfg(target_os = "linux")]
        if !self.attached {
            use eframe::wgpu::rwh::{HasDisplayHandle as _, HasWindowHandle as _};
            let (Ok(display), Ok(window)) = (frame.display_handle(), frame.window_handle()) else {
                return;
            };
            self.attached = true;
            self.display = linux::Display::of(display.as_raw(), window.as_raw(), ctx);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (frame, ctx);
    }

    pub fn poll(&mut self, ctx: &egui::Context) -> FileDrag {
        #[allow(unused_mut)]
        let mut drag = ctx.input(|input| FileDrag {
            hovering: !input.raw.hovered_files.is_empty(),
            pointer: input.pointer.latest_pos(),
            dropped: input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect(),
        });
        #[cfg(target_os = "linux")]
        if ctx.viewport_id() == egui::ViewportId::ROOT
            && let Some(display) = &mut self.display
        {
            // This display source is attached to the root's native surface;
            // its coordinates and Wayland transfers belong only to that
            // window. Child windows use their own toolkit events above.
            display.poll(ctx, &mut drag);
        }
        drag
    }
}

/// The local files named by a `text/uri-list` transfer.
#[cfg(target_os = "linux")]
fn uri_list_paths(list: &[u8]) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStringExt as _;
    list.split(|byte| matches!(byte, b'\r' | b'\n'))
        .filter_map(|line| {
            // An optional host precedes the absolute path.
            let rest = line.strip_prefix(b"file://")?;
            let path = &rest[rest.iter().position(|byte| *byte == b'/')?..];
            let mut bytes = Vec::with_capacity(path.len());
            let mut index = 0;
            while index < path.len() {
                let hex = |offset: usize| {
                    path.get(index + offset)
                        .and_then(|byte| (*byte as char).to_digit(16))
                };
                if path[index] == b'%'
                    && let (Some(high), Some(low)) = (hex(1), hex(2))
                {
                    bytes.push((high * 16 + low) as u8);
                    index += 3;
                } else {
                    bytes.push(path[index]);
                    index += 1;
                }
            }
            Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
        })
        .collect()
}

#[cfg(target_os = "linux")]
mod linux {
    use super::FileDrag;
    use eframe::egui::{self, Pos2};
    use eframe::wgpu::rwh::{RawDisplayHandle, RawWindowHandle};
    use std::{path::PathBuf, sync::mpsc, time::Duration};

    pub enum Display {
        Wayland {
            signals: mpsc::Receiver<wayland::Signal>,
            /// Surface coordinates of files held over the window.
            hover: Option<(f64, f64)>,
        },
        X11 {
            connection: Box<x11rb::rust_connection::RustConnection>,
            window: u32,
        },
    }

    impl Display {
        pub fn of(
            display: RawDisplayHandle,
            window: RawWindowHandle,
            ctx: &egui::Context,
        ) -> Option<Self> {
            let x11 = |window| {
                let (connection, _) = x11rb::connect(None).ok()?;
                Some(Self::X11 {
                    connection: Box::new(connection),
                    window,
                })
            };
            match (display, window) {
                (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(window)) => {
                    wayland::listen(
                        display.display.as_ptr() as usize,
                        window.surface.as_ptr() as usize,
                        ctx.clone(),
                    )
                    .map(|signals| Self::Wayland {
                        signals,
                        hover: None,
                    })
                }
                (_, RawWindowHandle::Xlib(window)) => x11(window.window as u32),
                (_, RawWindowHandle::Xcb(window)) => x11(window.window.get()),
                _ => None,
            }
        }

        pub fn poll(&mut self, ctx: &egui::Context, drag: &mut FileDrag) {
            match self {
                Self::Wayland { signals, hover } => {
                    let (held, mut dropped) = wayland::settle(hover, signals.try_iter());
                    drag.dropped.append(&mut dropped);
                    drag.hovering = hover.is_some();
                    // Surface coordinates ignore the app's own zoom.
                    let zoom = ctx.zoom_factor();
                    drag.pointer = held.map(|(x, y)| Pos2::new(x as f32 / zoom, y as f32 / zoom));
                }
                Self::X11 { connection, window } => {
                    // The drop ends the hover, but its files still land where
                    // the pointer is.
                    if !drag.hovering && drag.dropped.is_empty() {
                        return;
                    }
                    use x11rb::protocol::xproto::ConnectionExt as _;
                    if let Ok(cookie) = connection.query_pointer(*window)
                        && let Ok(pointer) = cookie.reply()
                    {
                        let scale = ctx.pixels_per_point();
                        drag.pointer = Some(Pos2::new(
                            f32::from(pointer.win_x) / scale,
                            f32::from(pointer.win_y) / scale,
                        ));
                    }
                    // A drag sends the window no pointer motion to wake it.
                    if drag.hovering {
                        ctx.request_repaint_after(Duration::from_millis(16));
                    }
                }
            }
        }
    }

    /// A data device of the window's own Wayland connection, on its own queue
    /// and thread, in the manner of the toolkit's clipboard.
    pub(super) mod wayland {
        use super::{PathBuf, egui, mpsc};
        use std::{io::Read as _, os::fd::AsFd as _};
        use wayland_backend::client::{Backend, ObjectId};
        use wayland_client::{
            Connection, Dispatch, Proxy as _, QueueHandle, delegate_noop, event_created_child,
            globals::{GlobalListContents, registry_queue_init},
            protocol::{
                wl_data_device::{self, WlDataDevice},
                wl_data_device_manager::{DndAction, WlDataDeviceManager},
                wl_data_offer::{self, WlDataOffer},
                wl_registry::{self, WlRegistry},
                wl_seat::WlSeat,
                wl_surface::WlSurface,
            },
        };

        const URI_LIST: &str = "text/uri-list";
        /// More paths than this are not a drop worth typing into a terminal.
        const MAX_TRANSFER: u64 = 1024 * 1024;

        pub enum Signal {
            /// Surface coordinates of files held over the window, if any are.
            Hover(Option<(f64, f64)>),
            Dropped(Vec<PathBuf>),
        }
        /// Applies a frame's signals to the hover state. Returns where the
        /// files are held, or were when they were dropped: the drop ends the
        /// hover in the same frame, and its files belong where they were let go.
        pub fn settle(
            hover: &mut Option<(f64, f64)>,
            signals: impl Iterator<Item = Signal>,
        ) -> (Option<(f64, f64)>, Vec<PathBuf>) {
            let mut held = *hover;
            let mut dropped = Vec::new();
            for signal in signals {
                match signal {
                    Signal::Hover(position) => {
                        *hover = position;
                        held = position.or(held);
                    }
                    Signal::Dropped(mut paths) => dropped.append(&mut paths),
                }
            }
            (if dropped.is_empty() { *hover } else { held }, dropped)
        }

        /// Signals the window has yet to draw a frame for.
        const PENDING_SIGNALS: usize = 64;

        struct State {
            signals: mpsc::SyncSender<Signal>,
            wake: egui::Context,
            surface: ObjectId,
            /// Offers introduced so far, and whether each carries files.
            offers: Vec<(WlDataOffer, bool)>,
            /// The offer over the window, and whether it was accepted.
            drag: Option<(WlDataOffer, bool)>,
        }

        impl State {
            /// Motion may be skipped while the window falls behind; the end
            /// of a drag and its files may not.
            fn signal(&self, signal: Signal) {
                let _ = match signal {
                    Signal::Hover(Some(_)) => self.signals.try_send(signal).ok(),
                    signal => self.signals.send(signal).ok(),
                };
                self.wake.request_repaint();
            }

            fn release(&mut self, offer: WlDataOffer) {
                self.offers.retain(|(known, _)| *known != offer);
                offer.destroy();
            }
        }

        pub fn listen(
            display: usize,
            surface: usize,
            wake: egui::Context,
        ) -> Option<mpsc::Receiver<Signal>> {
            let (signals, received) = mpsc::sync_channel(PENDING_SIGNALS);
            std::thread::Builder::new()
                .name("neptune-file-drag".into())
                .spawn(move || {
                    // SAFETY: both pointers belong to the window, which lives
                    // until the process exits; nothing joins this thread.
                    let (backend, surface) = unsafe {
                        (
                            Backend::from_foreign_display(display as *mut _),
                            ObjectId::from_ptr(WlSurface::interface(), surface as *mut _),
                        )
                    };
                    let Ok(surface) = surface else {
                        return;
                    };
                    let connection = Connection::from_backend(backend);
                    let Ok((globals, mut queue)) = registry_queue_init::<State>(&connection) else {
                        return;
                    };
                    let handle = queue.handle();
                    let (Ok(manager), Ok(seat)) = (
                        globals.bind::<WlDataDeviceManager, _, _>(&handle, 1..=3, ()),
                        globals.bind::<WlSeat, _, _>(&handle, 1..=1, ()),
                    ) else {
                        return;
                    };
                    let _device = manager.get_data_device(&seat, &handle, ());
                    let mut state = State {
                        signals,
                        wake,
                        surface,
                        offers: Vec::new(),
                        drag: None,
                    };
                    while queue.blocking_dispatch(&mut state).is_ok() {}
                })
                .ok()?;
            Some(received)
        }

        impl Dispatch<WlRegistry, GlobalListContents> for State {
            fn event(
                _: &mut Self,
                _: &WlRegistry,
                _: wl_registry::Event,
                _: &GlobalListContents,
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
        delegate_noop!(State: ignore WlSeat);
        delegate_noop!(State: WlDataDeviceManager);

        impl Dispatch<WlDataOffer, ()> for State {
            fn event(
                state: &mut Self,
                offer: &WlDataOffer,
                event: wl_data_offer::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                if let wl_data_offer::Event::Offer { mime_type } = event
                    && mime_type == URI_LIST
                    && let Some(known) = state.offers.iter_mut().find(|(known, _)| known == offer)
                {
                    known.1 = true;
                }
            }
        }

        impl Dispatch<WlDataDevice, ()> for State {
            fn event(
                state: &mut Self,
                _: &WlDataDevice,
                event: wl_data_device::Event,
                _: &(),
                connection: &Connection,
                _: &QueueHandle<Self>,
            ) {
                use wl_data_device::Event;
                match event {
                    Event::DataOffer { id } => state.offers.push((id, false)),
                    // The clipboard is read through the platform clipboard.
                    Event::Selection { id: Some(offer) } => state.release(offer),
                    Event::Enter {
                        serial,
                        surface,
                        x,
                        y,
                        id,
                    } => {
                        let Some(offer) = id else {
                            return;
                        };
                        // Window decorations are surfaces of their own.
                        let accepted = surface.id() == state.surface
                            && state
                                .offers
                                .iter()
                                .any(|(known, files)| *known == offer && *files);
                        offer.accept(serial, accepted.then(|| URI_LIST.to_owned()));
                        if accepted {
                            if offer.version() >= 3 {
                                offer.set_actions(DndAction::Copy, DndAction::Copy);
                            }
                            state.signal(Signal::Hover(Some((x, y))));
                        }
                        state.drag = Some((offer, accepted));
                    }
                    Event::Motion { x, y, .. } => {
                        if matches!(state.drag, Some((_, true))) {
                            state.signal(Signal::Hover(Some((x, y))));
                        }
                    }
                    Event::Leave => {
                        if let Some((offer, _)) = state.drag.take() {
                            state.release(offer);
                        }
                        state.signal(Signal::Hover(None));
                    }
                    Event::Drop => {
                        let Some((offer, accepted)) = state.drag.take() else {
                            return;
                        };
                        if accepted && let Ok((mut reader, writer)) = std::io::pipe() {
                            offer.receive(URI_LIST.to_owned(), writer.as_fd());
                            let _ = connection.flush();
                            // The source sees the end of the pipe only once
                            // this copy of it is closed.
                            drop(writer);
                            let mut list = Vec::new();
                            let _ = (&mut reader).take(MAX_TRANSFER).read_to_end(&mut list);
                            if offer.version() >= 3 {
                                offer.finish();
                            }
                            state.signal(Signal::Dropped(super::super::uri_list_paths(&list)));
                        }
                        state.release(offer);
                        state.signal(Signal::Hover(None));
                    }
                    _ => {}
                }
            }

            event_created_child!(State, WlDataDevice, [
                wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
            ]);
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn child_file_drags_do_not_consume_the_roots_wayland_transfer() {
        let ctx = egui::Context::default();
        let child = egui::ViewportId::from_hash_of("file-drag-child");
        let (sender, signals) = std::sync::mpsc::sync_channel(1);
        let root_file = PathBuf::from("/tmp/root.png");
        sender
            .send(linux::wayland::Signal::Dropped(vec![root_file.clone()]))
            .unwrap();
        let mut source = FileDragSource {
            display: Some(linux::Display::Wayland {
                signals,
                hover: Some((15.0, 25.0)),
            }),
            attached: true,
        };
        let pointer = egui::pos2(300.0, 80.0);
        let mut input = egui::RawInput {
            viewport_id: child,
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(640.0, 400.0),
            )),
            hovered_files: vec![egui::HoveredFile {
                path: Some(PathBuf::from("/tmp/child.png")),
                ..Default::default()
            }],
            events: vec![egui::Event::PointerMoved(pointer)],
            ..Default::default()
        };
        input.viewports.insert(child, egui::ViewportInfo::default());
        let mut drag = FileDrag::default();
        let mut output = ctx.run_ui(input, |ui| drag = source.poll(ui.ctx()));
        output.textures_delta.clear();
        assert!(drag.hovering);
        assert_eq!(drag.pointer, Some(pointer));
        assert!(drag.dropped.is_empty());

        let mut output = ctx.run_ui(Default::default(), |ui| drag = source.poll(ui.ctx()));
        output.textures_delta.clear();
        assert_eq!(drag.dropped, vec![root_file]);
        assert_eq!(drag.pointer, Some(egui::pos2(15.0, 25.0)));
    }

    #[test]
    fn wayland_files_are_dropped_where_they_were_last_held() {
        use linux::wayland::{Signal, settle};
        let file = PathBuf::from("/tmp/shot.png");
        let mut hover = None;
        let frame = [Signal::Hover(Some((10.0, 20.0)))];
        assert_eq!(
            settle(&mut hover, frame.into_iter()),
            (Some((10.0, 20.0)), vec![])
        );

        // The drop and the end of the hover arrive together.
        let frame = [
            Signal::Hover(Some((300.0, 40.0))),
            Signal::Dropped(vec![file.clone()]),
            Signal::Hover(None),
        ];
        assert_eq!(
            settle(&mut hover, frame.into_iter()),
            (Some((300.0, 40.0)), vec![file.clone()])
        );
        assert_eq!(hover, None);

        // Without motion in the drop's frame, the earlier position stands.
        let mut hover = Some((5.0, 6.0));
        let frame = [Signal::Dropped(vec![file.clone()]), Signal::Hover(None)];
        assert_eq!(
            settle(&mut hover, frame.into_iter()),
            (Some((5.0, 6.0)), vec![file])
        );

        // Files that leave the window are held nowhere.
        let mut hover = Some((5.0, 6.0));
        assert_eq!(
            settle(&mut hover, [Signal::Hover(None)].into_iter()),
            (None, vec![])
        );
    }

    #[test]
    fn uri_lists_name_local_files_and_decode_their_escapes() {
        let list = b"# from a file manager\r\nfile:///home/me/Screenshot%20from%202026.png\r\n\
            file://host/tmp/caf%C3%A9.txt\r\nhttps://example.com/a.png\r\nfile:///tmp/100%\r\n";
        assert_eq!(
            uri_list_paths(list),
            [
                PathBuf::from("/home/me/Screenshot from 2026.png"),
                PathBuf::from("/tmp/café.txt"),
                PathBuf::from("/tmp/100%"),
            ]
        );
    }
}
