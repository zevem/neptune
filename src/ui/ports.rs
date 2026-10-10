//! Dev-server chips shared by terminal tabs and the single-pane toolbar.
use super::{Action, helpers};
use crate::{
    icons::{self, Icon},
    runtime::ports::{Forward, Port},
    theme::{self, Palette},
};
use eframe::egui::{
    self, CursorIcon, Rect, Sense, Stroke, StrokeKind, WidgetInfo, WidgetType, pos2, vec2,
};
use neptune_model::PaneId;

pub struct Chips<'a> {
    chips: Vec<(Rect, std::sync::Arc<egui::Galley>, egui::Response, usize)>,
    ports: &'a [Port],
    target: Option<(PaneId, u64)>,
    menu: bool,
    pub left: f32,
}
impl<'a> Chips<'a> {
    pub fn layout(
        ui: &egui::Ui,
        id: egui::Id,
        ports: &'a [Port],
        target: Option<(PaneId, u64)>,
        (limit, right, middle): (f32, f32, f32),
        p: Palette,
    ) -> Self {
        let mut result = Self {
            chips: Vec::new(),
            ports,
            target,
            menu: false,
            left: right,
        };
        if ports.is_empty() || target.is_none() {
            return result;
        }
        let labels: Vec<_> = ports
            .iter()
            .map(|port| {
                ui.painter()
                    .layout_no_wrap(port.label(), theme::medium(11.5), p.secondary)
            })
            .collect();
        let width: f32 = labels.iter().map(|l| l.size().x + 24.0).sum();
        result.menu = ports.len() > 2 || width > right - limit;
        if result.menu {
            // Even a narrow tab retains a reachable list of every listener.
            let mut label = ui.painter().layout_no_wrap(
                format!("{} ports", ports.len()),
                theme::medium(11.5),
                p.secondary,
            );
            if label.size().x + 24.0 > right - limit {
                label = ui.painter().layout_no_wrap(
                    ports.len().to_string(),
                    theme::medium(11.5),
                    p.secondary,
                );
            }
            if label.size().x + 24.0 > right - limit {
                label =
                    ui.painter()
                        .layout_no_wrap(String::new(), theme::medium(11.5), p.secondary);
            }
            if label.size().x + 24.0 > right - limit {
                return result;
            }
            result.add(ui, id.with((target, "list")), label, 0, middle);
        } else {
            for (index, label) in labels.into_iter().enumerate().rev() {
                result.add(
                    ui,
                    id.with((target, ports[index].listener)),
                    label,
                    index,
                    middle,
                );
            }
        }
        result
    }
    fn add(
        &mut self,
        ui: &egui::Ui,
        id: egui::Id,
        label: std::sync::Arc<egui::Galley>,
        index: usize,
        middle: f32,
    ) {
        let rect = Rect::from_min_max(
            pos2(self.left - label.size().x - 23.0, middle - 10.0),
            pos2(self.left, middle + 10.0),
        );
        let response = ui.interact(rect, id, Sense::click());
        response.widget_info(|| {
            WidgetInfo::labeled(
                WidgetType::Button,
                true,
                if self.menu {
                    "Listening ports".into()
                } else {
                    hint(&self.ports[index])
                },
            )
        });
        self.left = rect.left() - 3.0;
        self.chips.push((rect, label, response, index));
    }
    pub fn is_empty(&self) -> bool {
        self.chips.is_empty()
    }
    pub fn hovered(&self) -> bool {
        self.chips
            .iter()
            .any(|(_, _, response, _)| response.hovered())
    }
    pub fn paint(self, painter: &egui::Painter, p: Palette, actions: &mut Vec<Action>) {
        let Some((pane, generation)) = self.target else {
            return;
        };
        for (rect, label, response, index) in self.chips {
            let port = &self.ports[index];
            let busy = matches!(port.forward, Forward::Starting | Forward::Stopping);
            let color = if matches!(port.forward, Forward::Failed { .. }) {
                p.red
            } else if matches!(port.forward, Forward::On(_)) {
                p.accent
            } else {
                p.secondary
            };
            painter.rect_filled(
                rect,
                6,
                if response.hovered() {
                    p.hover
                } else {
                    p.control
                },
            );
            if response.has_focus() {
                painter.rect_stroke(rect, 6, Stroke::new(1.0, p.accent), StrokeKind::Inside);
            }
            icons::paint(
                painter,
                Rect::from_center_size(pos2(rect.left() + 11.0, rect.center().y), vec2(12.0, 12.0)),
                if self.menu {
                    Icon::ChevronDown
                } else if busy {
                    Icon::Ellipsis
                } else {
                    Icon::Globe
                },
                color,
            );
            helpers::galley_at(
                painter,
                pos2(rect.left() + 21.0, rect.center().y + 0.5),
                label,
            );
            let response = response.on_hover_cursor(CursorIcon::PointingHand);
            if self.menu {
                egui::Popup::menu(&response).show(|ui| {
                    helpers::menu_layout(ui, 240.0);
                    egui::ScrollArea::vertical()
                        .id_salt((pane, generation, "ports-list"))
                        .max_height((ui.ctx().content_rect().height() - 32.0).max(40.0))
                        .show(ui, |ui| {
                            for port in self.ports {
                                port_menu(ui, p, pane, generation, port, actions);
                            }
                        });
                });
                response.on_hover_text("Listening ports");
            } else {
                if response.clicked() && !busy {
                    actions.push(Action::Port {
                        pane,
                        generation,
                        listener: port.listener,
                        stop: false,
                    });
                }
                response.context_menu(|ui| {
                    helpers::menu_layout(ui, 240.0);
                    port_menu(ui, p, pane, generation, port, actions);
                });
                response.on_hover_text(hint(port));
            }
        }
    }
}
fn hint(port: &Port) -> String {
    match &port.forward {
        Forward::Starting => format!("Forwarding {}…", port.listener.label()),
        Forward::Stopping => format!("Stopping forward for {}…", port.listener.label()),
        Forward::Failed {
            message,
            local: None,
        } => format!("Forward {} — retry\n{message}", port.listener.label()),
        Forward::Failed {
            message,
            local: Some(local),
        } => format!(
            "Preview localhost:{local}\nCould not stop forwarding: {message}\nRight-click to retry stopping"
        ),
        Forward::On(local) => format!(
            "Preview localhost:{local} (forwarded from {})\nRight-click to stop forwarding",
            port.listener.label()
        ),
        _ if port.remote => format!("Forward {} to this computer", port.listener.label()),
        _ => format!("Preview {}", port.label()),
    }
}
fn port_menu(
    ui: &mut egui::Ui,
    p: Palette,
    pane: PaneId,
    generation: u64,
    port: &Port,
    actions: &mut Vec<Action>,
) {
    let busy = matches!(port.forward, Forward::Starting | Forward::Stopping);
    if busy {
        ui.label(hint(port));
        return;
    }
    let label = if port.remote && port.url().is_none() {
        format!("Forward {}", port.listener.label())
    } else {
        format!("Preview {}", port.label())
    };
    if helpers::menu_item(ui, p, Icon::Globe, &label, "", false) {
        actions.push(Action::Port {
            pane,
            generation,
            listener: port.listener,
            stop: false,
        });
        ui.close();
    }
    if let Some(url) = port.url()
        && let Some(link) = crate::platform::links::WebLink::new(&url)
        && helpers::menu_item(
            ui,
            p,
            Icon::ArrowUpRight,
            "Open in external browser",
            "",
            false,
        )
    {
        actions.push(Action::OpenLink(link));
        ui.close();
    }
    if matches!(
        port.forward,
        Forward::On(_) | Forward::Failed { local: Some(_), .. }
    ) && helpers::menu_item(ui, p, Icon::Close, "Stop forwarding", "", false)
    {
        actions.push(Action::Port {
            pane,
            generation,
            listener: port.listener,
            stop: true,
        });
        ui.close();
    }
    if let Forward::Failed { message, .. } = &port.forward {
        ui.label(egui::RichText::new(message).color(p.red));
    }
}
