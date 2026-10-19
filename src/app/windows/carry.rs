//! Wayland lets no application position its windows or see the desktop
//! pointer. There the compositor carries a held workspace's window as a
//! drag-and-drop transfer, and each of Neptune's windows reports where the
//! workspace is held over it. Leaving a sidebar gives the workspace a new
//! window that follows the pointer, on top of the others; entering another
//! sidebar rejoins it there, within the same press.
use super::*;
use crate::platform::workspace_drag::Carried;
use std::time::{Duration, Instant};

/// Closing the window a transfer carried leaves the window under the pointer
/// until the closed window's animation ends; it is entered again afterwards.
const CLOSE_SETTLE: Duration = Duration::from_millis(600);
/// Without a transfer, that closing window can hold the pointer, and even its
/// release, away from Neptune until the pointer moves again. A press that
/// stays away this long ends in the sidebar it joined.
const LEAVE_GRACE: Duration = Duration::from_secs(1);

pub(in crate::app) struct Carry {
    workspace: WorkspaceId,
    /// The window that lists the workspace now.
    window: WindowId,
    /// Original press relative to the row, in egui points.
    grip: Vec2,
    /// Listed in another window's sidebar rather than following the pointer.
    docked: bool,
    /// A new window still to attach to the transfer once it exists.
    attach: Option<WindowId>,
    /// The native surface whose press started the transfer.
    origin: egui::ViewportId,
    /// A transfer is in progress. After its origin window closes, the same
    /// press continues as pointer motion over the window it joined.
    transfer: bool,
    /// Docking closed the origin window; the compositor's cancellation of
    /// its transfer does not end the press.
    origin_closing: bool,
    /// When the held workspace left the window it joined.
    left: Option<Instant>,
    /// When joining a window closed the one the workspace left.
    closed: Option<Instant>,
    /// The press ended, or could not continue as a transfer; the workspace
    /// stays where it is.
    ended: bool,
}

impl App {
    /// Carry a workspace held in `source`'s sidebar out of it. False when the
    /// compositor cannot, so the caller detaches it as before.
    pub(super) fn carry_workspace_out(
        &mut self,
        ctx: &egui::Context,
        workspace: WorkspaceId,
        source: WindowId,
        grip: Vec2,
    ) -> bool {
        if !self.windows.pointer.carries() {
            return false;
        }
        let origin = self.viewport_of(source);
        let sole = self
            .controller
            .model()
            .workspaces()
            .iter()
            .filter(|w| w.window() == source)
            .count()
            == 1;
        // The root's native window cannot close, so it cannot rejoin another
        // sidebar. Its only workspace moves with it as an ordinary window.
        if (sole && origin == egui::ViewportId::ROOT)
            || !self.windows.pointer.start_carry(origin, false)
        {
            return false;
        }
        self.cancel_workspace_drag(ctx);
        self.ui.cancel_sidebar_drag(ctx);
        let mut carry = Carry {
            workspace,
            window: source,
            grip,
            docked: false,
            attach: None,
            origin,
            transfer: true,
            origin_closing: false,
            left: None,
            closed: None,
            ended: false,
        };
        if sole {
            // A window with only this workspace follows the pointer itself,
            // held where it was pressed, from its next frame.
            carry.attach = Some(source);
            ctx.request_repaint_of(origin);
        } else {
            self.carry_into_new_window(ctx, &mut carry);
        }
        self.windows.carried = Some(carry);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
        true
    }

    fn carry_into_new_window(&mut self, ctx: &egui::Context, carry: &mut Carry) {
        let source = carry.window;
        let size = ctx
            .input(|input| {
                input
                    .raw
                    .viewports
                    .get(&self.viewport_of(source))
                    .and_then(|info| info.inner_rect)
            })
            .map_or(ctx.content_rect().size(), |rect| rect.size());
        self.enter_window(source);
        let Some((window, effects)) = self.detach_to_new_window(carry.workspace, source) else {
            return;
        };
        self.windows.launches.insert(
            window,
            LaunchWindow {
                position: None,
                size,
                maximized: false,
                fullscreen: false,
                defer_creation: false,
                first_frame: true,
                fallback: Some(source),
            },
        );
        self.execute(ctx, effects);
        carry.window = window;
        carry.attach = Some(window);
        carry.docked = false;
    }

    /// Attach a carried workspace's new window before its first frame is
    /// shown. The compositor places it under the pointer, above the others.
    pub(super) fn attach_carried_window(&mut self, ctx: &egui::Context, window: WindowId) {
        let Some(carry) = &self.windows.carried else {
            return;
        };
        if carry.attach != Some(window) {
            return;
        }
        let anchor = self
            .ui
            .workspace_rows
            .iter()
            .find(|(id, _)| *id == carry.workspace)
            .map(|(_, row)| row.min + carry.grip)
            .or_else(|| {
                // A narrow window or hidden sidebar has no row to hold.
                self.windows
                    .drop_zones
                    .get(&window)
                    .map(|zones| zones.toolbar.clamp(zones.toolbar.min + carry.grip))
            })
            .unwrap_or(carry.grip.to_pos2());
        // Surface coordinates ignore the app's own zoom.
        if self
            .windows
            .pointer
            .attach_carried(ctx.viewport_id(), anchor.to_vec2() * ctx.zoom_factor())
            && let Some(carry) = &mut self.windows.carried
        {
            carry.attach = None;
        }
    }

    pub(super) fn carrying_workspace(&self) -> bool {
        self.windows.carried.is_some()
    }

    pub(super) fn carries_in(&self, window: WindowId) -> bool {
        self.windows
            .carried
            .as_ref()
            .is_some_and(|carry| carry.window == window)
    }

    pub(super) fn cancel_carried_workspace(&mut self, ctx: &egui::Context) {
        if let Some(carry) = self.windows.carried.take() {
            self.end_carry(ctx, carry, false);
        }
    }

    pub(in crate::app) fn poll_carried_workspace(&mut self, ctx: &egui::Context) {
        let Some(mut carry) = self.windows.carried.take() else {
            return;
        };
        if !self.windows.pointer.carries() {
            self.end_carry(ctx, carry, false);
            return;
        }
        if self.exit_approved
            || self
                .controller
                .model()
                .workspace(carry.workspace)
                .is_none_or(|workspace| workspace.window() != carry.window)
        {
            self.end_carry(ctx, carry, false);
            return;
        }
        let windows: Vec<_> = self
            .controller
            .model()
            .windows()
            .iter()
            .map(|window| (self.viewport_of(window.id()), window.id()))
            .collect();
        let viewports: Vec<_> = windows.iter().map(|(viewport, _)| *viewport).collect();
        let previous = self.presentation_window();
        let zoom = ctx.zoom_factor();
        for signal in self.windows.pointer.carried(&viewports) {
            match signal {
                Carried::Over { viewport, position } => {
                    let over = viewport.and_then(|viewport| {
                        windows
                            .iter()
                            .find(|(known, _)| *known == viewport)
                            .map(|(_, window)| *window)
                    });
                    carry.left = None;
                    self.carry_over(ctx, &mut carry, over, position / zoom);
                }
                Carried::Away
                    if carry.docked
                        && carry.transfer
                        && carry
                            .closed
                            .is_none_or(|closed| closed.elapsed() >= CLOSE_SETTLE) =>
                {
                    self.carry_out_of(ctx, &mut carry);
                }
                Carried::Away if carry.docked => {
                    carry.left.get_or_insert_with(Instant::now);
                }
                Carried::Away => {}
                Carried::Ended if std::mem::take(&mut carry.origin_closing) => {
                    carry.transfer = false;
                }
                Carried::Released | Carried::Ended => carry.ended = true,
            }
            if carry.ended {
                break;
            }
        }
        if let Some(left) = carry.left
            && !carry.ended
        {
            let wait = match carry.closed {
                Some(closed) if carry.transfer => CLOSE_SETTLE.saturating_sub(closed.elapsed()),
                _ => LEAVE_GRACE.saturating_sub(left.elapsed()),
            };
            match Some(wait).filter(|wait| !wait.is_zero()) {
                Some(remaining) => ctx.request_repaint_after(remaining),
                None if carry.transfer => {
                    carry.left = None;
                    self.carry_out_of(ctx, &mut carry);
                }
                None => carry.ended = true,
            }
        }
        if carry.ended {
            self.end_carry(ctx, carry, true);
            self.restore_drag_context(previous);
            return;
        }
        self.windows.carried = Some(carry);
        self.restore_drag_context(previous);
    }

    fn in_drop_zone(&self, window: WindowId, point: Pos2) -> bool {
        self.windows
            .drop_zones
            .get(&window)
            .is_some_and(|zones| zones.sidebar.unwrap_or(zones.toolbar).contains(point))
    }

    fn carry_over(
        &mut self,
        ctx: &egui::Context,
        carry: &mut Carry,
        over: Option<WindowId>,
        point: Pos2,
    ) {
        let joins = over.filter(|target| {
            *target != carry.window
                && self.workspace_drop_available(*target)
                && self.in_drop_zone(*target, point)
        });
        if let Some(target) = joins {
            self.dock_carried(ctx, carry, target, point);
        } else if !carry.docked {
        } else if over == Some(carry.window) && self.in_drop_zone(carry.window, point) {
            self.hold_carried_row(ctx, carry, point);
        } else {
            self.carry_out_of(ctx, carry);
        }
    }

    fn hold_carried_row(&mut self, ctx: &egui::Context, carry: &Carry, point: Pos2) {
        let grouped = self
            .controller
            .model()
            .workspace(carry.workspace)
            .is_some_and(|workspace| workspace.group().is_some());
        if let Some(ui) = self.window_ui_state_mut(carry.window) {
            ui.carry_workspace(carry.workspace, grouped, point, carry.grip);
        }
        ctx.request_repaint_of(self.viewport_of(carry.window));
    }

    fn dock_carried(
        &mut self,
        ctx: &egui::Context,
        carry: &mut Carry,
        target: WindowId,
        point: Pos2,
    ) {
        let from = carry.window;
        let from_viewport = self.viewport_of(from);
        if carry.docked
            && let Some(ui) = self.window_ui_state_mut(from)
        {
            ui.cancel_carried_workspace();
        }
        self.enter_window(from);
        self.ui.cancel_sidebar_drag(ctx);
        // The shared durable transfer keeps drafts, panes and sessions.
        self.dispatch(
            ctx,
            Command::MoveWorkspaceToWindow {
                workspace: carry.workspace,
                window: target,
            },
        );
        if self
            .controller
            .model()
            .workspace(carry.workspace)
            .is_none_or(|workspace| workspace.window() != target)
        {
            return;
        }
        carry.closed = None;
        if self.controller.model().window(from).is_none() {
            // The emptied window closes, which detaches it from the transfer.
            carry.closed = Some(Instant::now());
            if carry.attach == Some(from) {
                carry.attach = None;
            }
            if carry.transfer && from_viewport == carry.origin {
                carry.origin_closing = true;
            }
        }
        carry.window = target;
        carry.docked = true;
        self.hold_carried_row(ctx, carry, point);
    }

    /// The carried workspace left the sidebar it joined: it follows the
    /// pointer in a window of its own again.
    fn carry_out_of(&mut self, ctx: &egui::Context, carry: &mut Carry) {
        let from = carry.window;
        if !carry.transfer {
            // The press continues from the window it joined.
            let origin = self.viewport_of(from);
            if !self.windows.pointer.start_carry(origin, true) {
                carry.ended = true;
                return;
            }
            carry.transfer = true;
            carry.origin = origin;
        }
        if let Some(ui) = self.window_ui_state_mut(from) {
            ui.cancel_carried_workspace();
        }
        ctx.request_repaint_of(self.viewport_of(from));
        self.carry_into_new_window(ctx, carry);
    }

    fn end_carry(&mut self, ctx: &egui::Context, carry: Carry, released: bool) {
        self.windows.pointer.finish_carry();
        if carry.docked
            && let Some(ui) = self.window_ui_state_mut(carry.window)
        {
            if released {
                ui.finish_carried_workspace();
            } else {
                ui.cancel_carried_workspace();
            }
        }
        // The toolkit never saw these surfaces' button release.
        let viewport = self.viewport_of(carry.window);
        self.windows.released.insert(carry.origin);
        self.windows.released.insert(viewport);
        ctx.request_repaint_of(viewport);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    /// Where empty native windows must not linger, the root never stays
    /// empty beside other windows: it shows the most recently used of them,
    /// whose own native window then closes.
    pub(in crate::app) fn rehost_empty_root(&mut self, ctx: &egui::Context) {
        if self.windows.pointer.keeps_hidden_root()
            || self.startup.is_some()
            || self.windows.drag.is_some()
            || self.windows.carried.is_some()
        {
            return;
        }
        let model = self.controller.model();
        let host = self.windows.host;
        if model
            .window(host)
            .is_some_and(|window| window.active_workspace().is_some())
        {
            return;
        }
        let Some(adopted) = model
            .windows()
            .iter()
            .filter(|window| {
                window.active_workspace().is_some()
                    && self.viewport_of(window.id()) != egui::ViewportId::ROOT
            })
            .map(|window| window.id())
            .max_by(|a, b| {
                let focused = |window| {
                    self.windows
                        .drop_zones
                        .get(window)
                        .map_or(0.0, |zones| zones.last_focused)
                };
                focused(a).total_cmp(&focused(b))
            })
        else {
            return;
        };
        let child = self.viewport_of(adopted);
        let shown = ctx.input(|input| {
            input.raw.viewports.get(&child).map(|info| {
                (
                    info.outer_rect.map(|rect| rect.min),
                    info.inner_rect.map(|rect| rect.size()),
                    info.maximized.unwrap_or(false),
                    info.fullscreen.unwrap_or(false),
                    info.minimized.unwrap_or(false),
                )
            })
        });
        self.windows
            .viewports
            .insert(adopted, egui::ViewportId::ROOT);
        if model.window(host).is_some() {
            self.windows.viewports.insert(host, child);
        }
        self.windows.host = adopted;
        self.windows.root_hidden = false;
        // Reuse the surviving window's geometry. Wayland omits its position;
        // macOS reports it in shared desktop points, independent of Retina scale.
        if let Some((position, size, maximized, fullscreen, minimized)) = shown {
            // Leave the old host's special states before applying the survivor's
            // normal geometry, then restore the survivor's window states.
            for command in [
                egui::ViewportCommand::Minimized(false),
                egui::ViewportCommand::Fullscreen(false),
                egui::ViewportCommand::Maximized(false),
            ] {
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, command);
            }
            if let Some(position) = position {
                ctx.send_viewport_cmd_to(
                    egui::ViewportId::ROOT,
                    egui::ViewportCommand::OuterPosition(position),
                );
            }
            if let Some(size) = size.filter(|_| !maximized && !fullscreen) {
                ctx.send_viewport_cmd_to(
                    egui::ViewportId::ROOT,
                    egui::ViewportCommand::InnerSize(size),
                );
            }
            if maximized {
                ctx.send_viewport_cmd_to(
                    egui::ViewportId::ROOT,
                    egui::ViewportCommand::Maximized(true),
                );
            }
            if fullscreen {
                ctx.send_viewport_cmd_to(
                    egui::ViewportId::ROOT,
                    egui::ViewportCommand::Fullscreen(true),
                );
            }
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(true));
            if minimized {
                ctx.send_viewport_cmd_to(
                    egui::ViewportId::ROOT,
                    egui::ViewportCommand::Minimized(true),
                );
            }
        } else {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(true));
        }
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
}
