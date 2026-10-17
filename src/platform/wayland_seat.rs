//! The window's own Wayland connection, on its own queue and thread, in the
//! manner of the toolkit's clipboard. The toolkit does not take part in
//! drag-and-drop on Wayland, so this one data device carries both files dropped
//! from other applications and workspaces dragged between Neptune's windows.
//!
//! A compositor gives drag-and-drop to one data device per client, the newest,
//! so the process has a single seat. Each window's clipboard has a data device
//! of its own, and the seat renews its device after them. Only button presses
//! and a held workspace's position are kept; no terminal input or contents
//! pass through it.

use eframe::egui::{self, Pos2};
use std::{
    io::Read as _,
    os::fd::AsFd as _,
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc,
    },
};
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::{
    Connection, Dispatch, Proxy as _, QueueHandle, WEnum, delegate_noop, event_created_child,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_data_device::{self, WlDataDevice},
        wl_data_device_manager::{DndAction, WlDataDeviceManager},
        wl_data_offer::{self, WlDataOffer},
        wl_data_source::{self, WlDataSource},
        wl_pointer::{self, WlPointer},
        wl_registry::{self, WlRegistry},
        wl_seat::WlSeat,
        wl_surface::WlSurface,
    },
};
use wayland_protocols::xdg::{
    shell::client::xdg_toplevel::XdgToplevel,
    toplevel_drag::v1::client::{
        xdg_toplevel_drag_manager_v1::XdgToplevelDragManagerV1,
        xdg_toplevel_drag_v1::XdgToplevelDragV1,
    },
};

const URI_LIST: &str = "text/uri-list";
/// More paths than this are not a drop worth typing into a terminal.
const MAX_TRANSFER: u64 = 1024 * 1024;
/// Signals the window has yet to draw a frame for.
const PENDING_SIGNALS: usize = 64;
/// Linux's primary pointer button.
const BTN_LEFT: u32 = 0x110;

/// Workspaces move only between this process's windows.
fn workspace_mime() -> &'static str {
    static MIME: OnceLock<String> = OnceLock::new();
    MIME.get_or_init(|| format!("application/x-neptune-workspace-{}", std::process::id()))
}

pub enum FileSignal {
    /// Surface coordinates of files held over the root window, if any are.
    Hover(Option<(f64, f64)>),
    Dropped(Vec<PathBuf>),
}

/// The held workspace, as the seat sees it, in surface-local logical pixels.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkspaceSignal {
    /// Held over one of Neptune's surfaces.
    Over { surface: ObjectId, position: Pos2 },
    /// Held over no surface of Neptune's.
    Away,
    /// Released over one of Neptune's surfaces.
    Released,
    /// The drag-and-drop transfer ended. After its origin surface closes, the
    /// same press continues as pointer motion.
    Ended,
}

/// What the seat's thread records for the window.
#[derive(Default)]
struct Shared {
    /// The last primary press: its serial above its surface's protocol id.
    press: AtomicU64,
    /// Whether the primary button is still held, as far as the pointer knows.
    held: AtomicBool,
    /// Pointer motion is forwarded only while a held workspace needs it.
    follow_pointer: AtomicBool,
    /// The workspace transfer in progress, by number; 0 when none is.
    transfer: AtomicU32,
}

/// A workspace transfer, as its data source knows it.
pub struct Transfer {
    number: u32,
    /// The window move, until the drop or cancellation ends it.
    drag: OnceLock<XdgToplevelDragV1>,
    dropped: AtomicBool,
}

impl Transfer {
    fn end(source: &WlDataSource) {
        if let Some(drag) = source
            .data::<Self>()
            .and_then(|transfer| transfer.drag.get())
        {
            drag.destroy();
        }
        source.destroy();
    }
}

pub struct Seat {
    connection: Connection,
    queue: QueueHandle<State>,
    manager: WlDataDeviceManager,
    seat: WlSeat,
    device: WlDataDevice,
    /// A transfer that entered a window before the renewal still reaches it.
    previous: Option<WlDataDevice>,
    toplevel_drags: Option<XdgToplevelDragManagerV1>,
    shared: Arc<Shared>,
    transfer: Option<WlDataSource>,
    transfers: u32,
    files: Option<mpsc::Receiver<FileSignal>>,
    workspace: mpsc::Receiver<WorkspaceSignal>,
}

/// The `wl_surface` of a toolkit window.
pub fn surface_of(window: &eframe::wgpu::rwh::RawWindowHandle) -> Option<ObjectId> {
    let eframe::wgpu::rwh::RawWindowHandle::Wayland(window) = window else {
        return None;
    };
    // SAFETY: the toolkit's window owns this live surface proxy.
    unsafe { ObjectId::from_ptr(WlSurface::interface(), window.surface.as_ptr().cast()).ok() }
}

impl Seat {
    /// The seat of the root window's Wayland connection.
    pub fn connect(display: usize, root: usize, wake: &egui::Context) -> Option<Self> {
        // SAFETY: both pointers belong to the root window, which lives until
        // the process exits.
        let (backend, root) = unsafe {
            (
                Backend::from_foreign_display(display as *mut _),
                ObjectId::from_ptr(WlSurface::interface(), root as *mut _).ok()?,
            )
        };
        let connection = Connection::from_backend(backend);
        let (globals, mut queue) = registry_queue_init::<State>(&connection).ok()?;
        let handle = queue.handle();
        let manager = globals
            .bind::<WlDataDeviceManager, _, _>(&handle, 1..=3, ())
            .ok()?;
        let seat = globals.bind::<WlSeat, _, _>(&handle, 1..=1, ()).ok()?;
        let toplevel_drags = globals
            .bind::<XdgToplevelDragManagerV1, _, _>(&handle, 1..=1, ())
            .ok();
        let device = manager.get_data_device(&seat, &handle, ());
        let _pointer = seat.get_pointer(&handle, ());
        let (files, file_signals) = mpsc::sync_channel(PENDING_SIGNALS);
        let (workspace, workspace_signals) = mpsc::sync_channel(PENDING_SIGNALS);
        let shared = Arc::new(Shared::default());
        let mut state = State {
            files,
            workspace,
            wake: wake.clone(),
            root,
            offers: Vec::new(),
            drag: None,
            shared: shared.clone(),
            pointer: None,
        };
        std::thread::Builder::new()
            .name("neptune-wayland-seat".into())
            .spawn(move || while queue.blocking_dispatch(&mut state).is_ok() {})
            .ok()?;
        Some(Self {
            connection,
            queue: handle,
            manager,
            seat,
            device,
            previous: None,
            toplevel_drags,
            shared,
            transfer: None,
            transfers: 0,
            files: Some(file_signals),
            workspace: workspace_signals,
        })
    }

    /// Take drag-and-drop back from the data devices of windows created since.
    pub fn renew_device(&mut self) {
        let device = self.manager.get_data_device(&self.seat, &self.queue, ());
        let previous = std::mem::replace(&mut self.device, device);
        if let Some(retired) = self.previous.replace(previous)
            && retired.version() >= 2
        {
            retired.release();
        }
        let _ = self.connection.flush();
    }

    /// Files dropped on the root window, for their one reader.
    pub fn take_files(&mut self) -> Option<mpsc::Receiver<FileSignal>> {
        self.files.take()
    }

    /// Whether held windows can follow the pointer between Neptune's windows.
    pub fn moves_windows(&self) -> bool {
        self.toplevel_drags.is_some()
    }

    /// Start carrying a workspace with the held press on `origin`, or with a
    /// `continuing` press that began on a window since closed. The compositor
    /// sends the press's later motion to the data device only.
    pub fn start_workspace_drag(&mut self, origin: &ObjectId, continuing: bool) -> bool {
        let press = self.shared.press.load(Ordering::Acquire);
        if !self.shared.held.load(Ordering::Acquire)
            || !(continuing || press as u32 == origin.protocol_id())
        {
            return false;
        }
        let Ok(origin) = WlSurface::from_id(&self.connection, origin.clone()) else {
            return false;
        };
        if let Some(stale) = self.transfer.take() {
            // A drag the compositor never ended for us cannot be in progress.
            Transfer::end(&stale);
        }
        self.transfers = self.transfers.wrapping_add(1).max(1);
        let source = self.manager.create_data_source(
            &self.queue,
            Transfer {
                number: self.transfers,
                drag: OnceLock::new(),
                dropped: AtomicBool::new(false),
            },
        );
        source.offer(workspace_mime().to_owned());
        if source.version() >= 3 {
            source.set_actions(DndAction::Move);
        }
        if let (Some(manager), Some(transfer)) = (&self.toplevel_drags, source.data::<Transfer>()) {
            let _ = transfer
                .drag
                .set(manager.get_xdg_toplevel_drag(&source, &self.queue, ()));
        }
        self.shared
            .transfer
            .store(self.transfers, Ordering::Release);
        self.shared.follow_pointer.store(true, Ordering::Release);
        self.device
            .start_drag(Some(&source), &origin, None, (press >> 32) as u32);
        self.transfer = Some(source);
        self.connection.flush().is_ok()
    }

    /// Move a window with the held workspace. `offset` places the pointer in
    /// the window, in logical pixels.
    pub fn attach(&self, window: &winit::window::Window, offset: egui::Vec2) -> bool {
        use winit::platform::wayland::WindowExtWayland as _;
        let Some(toplevel) = window.xdg_toplevel() else {
            return false;
        };
        // SAFETY: the toolkit's window owns this live toplevel proxy.
        let Ok(id) =
            (unsafe { ObjectId::from_ptr(XdgToplevel::interface(), toplevel.as_ptr().cast()) })
        else {
            return false;
        };
        let Ok(toplevel) = XdgToplevel::from_id(&self.connection, id) else {
            return false;
        };
        let Some(drag) = self
            .transfer
            .as_ref()
            .and_then(|source| source.data::<Transfer>())
            .filter(|transfer| !transfer.dropped.load(Ordering::Acquire))
            .and_then(|transfer| transfer.drag.get())
        else {
            return false;
        };
        drag.attach(&toplevel, offset.x.round() as i32, offset.y.round() as i32);
        self.connection.flush().is_ok()
    }

    /// Stop following a held workspace.
    pub fn finish_workspace_drag(&self) {
        self.shared.follow_pointer.store(false, Ordering::Release);
        while self.workspace.try_recv().is_ok() {}
    }

    pub fn workspace_signals(&self) -> Vec<WorkspaceSignal> {
        self.workspace.try_iter().collect()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Other,
    Files,
    Workspace,
}

struct State {
    files: mpsc::SyncSender<FileSignal>,
    workspace: mpsc::SyncSender<WorkspaceSignal>,
    wake: egui::Context,
    root: ObjectId,
    /// Offers introduced so far, and what each carries.
    offers: Vec<(WlDataOffer, Kind)>,
    /// The offer over a window, and what it was accepted as.
    drag: Option<(WlDataOffer, Kind)>,
    shared: Arc<Shared>,
    /// The surface under the pointer, while it is over one of Neptune's.
    pointer: Option<ObjectId>,
}

impl State {
    /// Motion may be skipped while the window falls behind; the end of a drag
    /// and its files may not.
    fn file(&self, signal: FileSignal) {
        let _ = match signal {
            FileSignal::Hover(Some(_)) => self.files.try_send(signal).ok(),
            signal => self.files.send(signal).ok(),
        };
        self.wake.request_repaint();
    }

    fn workspace(&self, signal: WorkspaceSignal) {
        let _ = match signal {
            WorkspaceSignal::Over { .. } => self.workspace.try_send(signal).ok(),
            signal => self.workspace.send(signal).ok(),
        };
        self.wake.request_repaint_of(egui::ViewportId::ROOT);
    }

    fn transferring(&self) -> bool {
        self.shared.transfer.load(Ordering::Acquire) != 0
    }

    fn following(&self) -> bool {
        self.shared.follow_pointer.load(Ordering::Acquire)
    }

    fn release(&mut self, offer: WlDataOffer) {
        self.offers.retain(|(known, _)| *known != offer);
        offer.destroy();
    }
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
delegate_noop!(State: XdgToplevelDragManagerV1);
delegate_noop!(State: XdgToplevelDragV1);
delegate_noop!(State: ignore XdgToplevel);
delegate_noop!(State: ignore WlSurface);

impl Dispatch<WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                surface,
                surface_x,
                surface_y,
                ..
            } => {
                state.pointer = Some(surface.id());
                if state.following() {
                    state.workspace(WorkspaceSignal::Over {
                        surface: surface.id(),
                        position: Pos2::new(surface_x as f32, surface_y as f32),
                    });
                }
            }
            wl_pointer::Event::Leave { surface, .. } => {
                // Focus returns from a drag by leaving the window pressed
                // before it, which the pointer is no longer over.
                if state.pointer.as_ref() != Some(&surface.id()) {
                    return;
                }
                state.pointer = None;
                if state.following() {
                    state.workspace(WorkspaceSignal::Away);
                }
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                if state.following()
                    && let Some(surface) = state.pointer.clone()
                {
                    state.workspace(WorkspaceSignal::Over {
                        surface,
                        position: Pos2::new(surface_x as f32, surface_y as f32),
                    });
                }
            }
            wl_pointer::Event::Button {
                serial,
                button: BTN_LEFT,
                state: button,
                ..
            } => {
                if matches!(button, WEnum::Value(wl_pointer::ButtonState::Pressed)) {
                    if let Some(surface) = &state.pointer {
                        let press = u64::from(serial) << 32 | u64::from(surface.protocol_id());
                        state.shared.press.store(press, Ordering::Release);
                        state.shared.held.store(true, Ordering::Release);
                    }
                } else {
                    state.shared.held.store(false, Ordering::Release);
                    if state.following() {
                        state.workspace(WorkspaceSignal::Released);
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlDataSource, Transfer> for State {
    fn event(
        state: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        transfer: &Transfer,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wl_data_source::Event;
        let current = state.shared.transfer.load(Ordering::Acquire) == transfer.number;
        match event {
            // Releasing a workspace leaves no data to read.
            Event::Send { fd, .. } => drop(fd),
            Event::DndDropPerformed if current => {
                // The drop consumed the release; the pointer never sees it.
                transfer.dropped.store(true, Ordering::Release);
                if let Some(drag) = transfer.drag.get() {
                    drag.destroy();
                }
                state.shared.held.store(false, Ordering::Release);
                state.workspace(WorkspaceSignal::Ended);
            }
            Event::DndFinished | Event::Cancelled => {
                let dropped = transfer.dropped.swap(true, Ordering::AcqRel);
                Transfer::end(source);
                if current {
                    let _ = state.shared.transfer.compare_exchange(
                        transfer.number,
                        0,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                    if !dropped {
                        state.workspace(WorkspaceSignal::Ended);
                    }
                }
            }
            _ => {}
        }
    }
}

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
            && let Some(known) = state.offers.iter_mut().find(|(known, _)| known == offer)
        {
            if mime_type == workspace_mime() {
                known.1 = Kind::Workspace;
            } else if mime_type == URI_LIST && known.1 == Kind::Other {
                known.1 = Kind::Files;
            }
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
            Event::DataOffer { id } => state.offers.push((id, Kind::Other)),
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
                let kind = state
                    .offers
                    .iter()
                    .find(|(known, _)| *known == offer)
                    .map_or(Kind::Other, |(_, kind)| *kind);
                // Files land only in the root window; window decorations are
                // surfaces of their own.
                let kind = match kind {
                    Kind::Files if surface.id() != state.root => Kind::Other,
                    kind => kind,
                };
                let (mime, action) = match kind {
                    Kind::Files => (Some(URI_LIST), DndAction::Copy),
                    Kind::Workspace => (Some(workspace_mime()), DndAction::Move),
                    Kind::Other => (None, DndAction::None),
                };
                offer.accept(serial, mime.map(str::to_owned));
                if mime.is_some() && offer.version() >= 3 {
                    offer.set_actions(action, action);
                }
                match kind {
                    Kind::Files => state.file(FileSignal::Hover(Some((x, y)))),
                    Kind::Workspace => state.workspace(WorkspaceSignal::Over {
                        surface: surface.id(),
                        position: Pos2::new(x as f32, y as f32),
                    }),
                    Kind::Other => {}
                }
                state.pointer = Some(surface.id());
                state.drag = Some((offer, kind));
            }
            Event::Motion { x, y, .. } => match &state.drag {
                Some((_, Kind::Files)) => state.file(FileSignal::Hover(Some((x, y)))),
                Some((_, Kind::Workspace)) => {
                    if let Some(surface) = state.pointer.clone() {
                        state.workspace(WorkspaceSignal::Over {
                            surface,
                            position: Pos2::new(x as f32, y as f32),
                        });
                    }
                }
                _ => {}
            },
            Event::Leave => {
                state.pointer = None;
                if let Some((offer, kind)) = state.drag.take() {
                    state.release(offer);
                    match kind {
                        Kind::Files => state.file(FileSignal::Hover(None)),
                        // Cancelling a transfer leaves its window too; the
                        // press it carried may continue over that window.
                        Kind::Workspace if state.transferring() => {
                            state.workspace(WorkspaceSignal::Away)
                        }
                        Kind::Workspace | Kind::Other => {}
                    }
                }
            }
            Event::Drop => {
                let Some((offer, kind)) = state.drag.take() else {
                    return;
                };
                match kind {
                    Kind::Files => {
                        if let Ok((mut reader, writer)) = std::io::pipe() {
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
                            state
                                .file(FileSignal::Dropped(super::file_drag::uri_list_paths(&list)));
                        }
                        state.file(FileSignal::Hover(None));
                    }
                    Kind::Workspace => {
                        if offer.version() >= 3 {
                            offer.finish();
                        }
                        state.workspace(WorkspaceSignal::Released);
                    }
                    Kind::Other => {}
                }
                state.release(offer);
            }
            _ => {}
        }
    }

    event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}
