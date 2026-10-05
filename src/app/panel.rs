//! The trailing panel's toggle and tabs, and the agents its second tab lists.
use super::App;
use crate::ui::{self, panel::Event, panel::Tab};
use eframe::egui;
use std::time::{Duration, Instant};

impl App {
    /// How far the panel has slid in, 0 to 1. In a window too narrow for it
    /// the panel is gone at once, whatever its toggle says.
    pub(super) fn panel_reveal(&mut self, ctx: &egui::Context, available: bool) -> f32 {
        let open = self.ui.panel.open && available;
        let sliding = self
            .ui
            .panel
            .slide
            .and_then(|slide| slide.reveal(open, ctx.input(|input| input.time)))
            .filter(|_| available);
        if sliding.is_some() {
            ctx.request_repaint();
        } else {
            self.ui.panel.slide = None;
        }
        sliding.unwrap_or(if open { 1.0 } else { 0.0 })
    }

    pub(super) fn panel_event(&mut self, ctx: &egui::Context, event: Event) {
        let state = &mut self.ui.panel;
        let (open, tab) = match event {
            Event::Toggle => (!state.open, state.tab),
            Event::Show(tab) => (true, tab),
        };
        if open != state.open {
            state.slide = Some(ui::chrome::SidebarSlide::toggled(
                state.slide,
                state.open,
                ctx.input(|input| input.time),
            ));
            state.open = open;
        }
        state.tab = tab;
        if !open || tab != Tab::Files {
            self.leave_explorer(ctx);
        }
        ctx.request_repaint();
    }

    /// Takes what agents reported and what their titles say, whether or not
    /// the panel is in view, and wakes for the next change time alone brings.
    pub(super) fn poll_agents(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        for (pane, generation, activity) in self.sessions.agent_activity() {
            self.agents.report(pane, generation, activity, now);
        }
        let model = self.controller.model();
        self.agents.retain(model);
        for (id, session) in self.sessions.iter() {
            if let Some(kind) = model
                .pane(id)
                .and_then(|pane| pane.agent())
                .map(|agent| agent.kind)
                && self.agents.get(id).is_some()
            {
                self.agents.title(id, kind, &session.metadata().title, now);
            }
        }
        if let (_, Some(wait)) = self.agents.settle(now) {
            ctx.request_repaint_after(wait);
        }
    }

    /// Every running agent, in the order of the workspaces and their terminals.
    pub(super) fn agent_rows(&self) -> Vec<ui::agents::Row> {
        let now = Instant::now();
        let model = self.controller.model();
        let focused = model.active_pane();
        let mut rows = Vec::new();
        for workspace in model.workspaces() {
            for pane in workspace.panes() {
                let (Some(agent), Some((activity, since))) =
                    (pane.agent(), self.agents.get(pane.id()))
                else {
                    continue;
                };
                let title = self
                    .sessions
                    .get(pane.id())
                    .filter(|_| self.agents.titled(pane.id()))
                    .and_then(|session| {
                        crate::agent_activity::conversation(agent.kind, &session.metadata().title)
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| ui::agents::kind_name(agent.kind).to_owned());
                rows.push(ui::agents::Row {
                    pane: pane.id(),
                    generation: pane.generation(),
                    kind: agent.kind,
                    activity,
                    elapsed: now.saturating_duration_since(since),
                    title,
                    workspace: workspace.name().to_owned(),
                    folder: ui::helpers::compact_path(&agent.cwd),
                    focused: focused == Some(pane.id()),
                });
            }
        }
        rows
    }

    /// The agents that the agent in `parent` started, for its tab's list.
    pub(super) fn spawned_agents(&self, parent: neptune_model::PaneId) -> Vec<ui::agents::Spawned> {
        self.controller
            .model()
            .spawned(parent)
            .map(|pane| {
                let kind = pane.agent().map(|agent| agent.kind);
                let title = kind
                    .zip(self.sessions.get(pane.id()))
                    .filter(|_| self.agents.titled(pane.id()))
                    .and_then(|(kind, session)| {
                        crate::agent_activity::conversation(kind, &session.metadata().title)
                            .map(|title| format!("{} · {title}", ui::agents::kind_name(kind)))
                    })
                    .or_else(|| kind.map(|kind| ui::agents::kind_name(kind).to_owned()))
                    .unwrap_or_else(|| "Agent".to_owned());
                ui::agents::Spawned {
                    pane: pane.id(),
                    title,
                    activity: self.agents.get(pane.id()).map(|(activity, _)| activity),
                    starting: self.delegation.starting(pane.id()),
                }
            })
            .collect()
    }

    /// Ages are shown in minutes: a list in view is drawn again as they pass.
    pub(super) fn tick_agents(ctx: &egui::Context) {
        ctx.request_repaint_after(Duration::from_secs(30));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_activity::{Activity, Attention};
    use crate::ui::Action;
    use eframe::egui::{Pos2, Rect, Vec2};
    use neptune_model::{AgentKind, AgentSession, Command, PaneId};

    const WINDOW: Vec2 = Vec2::new(900.0, 640.0);

    fn frame(app: &mut App, ctx: &egui::Context) {
        let mut host = eframe::Frame::_new_kittest();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, WINDOW)),
                ..Default::default()
            },
            |ui| {
                app.poll_explorer(ui.ctx());
                eframe::App::ui(app, ui, &mut host);
            },
        );
        output.textures_delta.clear();
    }

    fn settle(app: &mut App, ctx: &egui::Context, what: &str, done: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            frame(app, ctx);
            if done(app) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// An application with a workspace per name, each terminal running an agent.
    fn with_agents(root: &std::path::Path, names: &[&str]) -> (App, egui::Context, Vec<PaneId>) {
        let (mut app, _sender) = super::super::tests::fixture(root);
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        app.startup = None;
        let mut panes = Vec::new();
        for (index, name) in names.iter().enumerate() {
            app.controller
                .dispatch(Command::AddWorkspace {
                    group: None,
                    cwd: root.into(),
                    name: (*name).into(),
                    remote: None,
                })
                .unwrap();
            let pane = app.controller.model().active_pane().unwrap();
            let generation = app.controller.model().pane(pane).unwrap().generation();
            app.controller
                .dispatch(Command::PaneAgentChanged {
                    pane,
                    generation,
                    agent: Some(AgentSession {
                        kind: if index % 2 == 0 {
                            AgentKind::Claude
                        } else {
                            AgentKind::Codex
                        },
                        session_id: None,
                        cwd: root.into(),
                    }),
                })
                .unwrap();
            panes.push(pane);
        }
        (app, ctx, panes)
    }

    fn generation(app: &App, pane: PaneId) -> u64 {
        app.controller.model().pane(pane).unwrap().generation()
    }

    #[test]
    fn agents_of_every_workspace_are_listed_and_a_row_reveals_its_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, panes) = with_agents(dir.path(), &["One", "Two", "Three"]);
        let now = Instant::now();
        // A reference alone is a conversation to resume, not a running agent.
        assert!(app.agent_rows().is_empty());
        // A report that reaches a frame before its reference waits for it.
        app.controller
            .dispatch(Command::PaneAgentChanged {
                pane: panes[2],
                generation: generation(&app, panes[2]),
                agent: None,
            })
            .unwrap();
        let third = generation(&app, panes[2]);
        app.agents
            .report(panes[2], third, Some(Activity::Idle), now);
        app.poll_agents(&ctx);
        assert!(app.agent_rows().is_empty());
        assert!(app.agents.get(panes[2]).is_some());
        app.agents.report(panes[2], third, None, now);
        let asked = Activity::NeedsInput(Attention::Question);
        for (pane, activity) in [(panes[0], Activity::Working), (panes[1], asked)] {
            let generation = generation(&app, pane);
            app.agents.report(pane, generation, Some(activity), now);
        }
        app.poll_agents(&ctx);
        let rows = app.agent_rows();
        let listed: Vec<_> = rows
            .iter()
            .map(|row| {
                (
                    row.workspace.as_str(),
                    row.title.as_str(),
                    row.activity,
                    row.focused,
                )
            })
            .collect();
        assert_eq!(
            listed,
            [
                ("One", "Claude Code", Activity::Working, false),
                ("Two", "Codex", asked, false),
            ]
        );
        assert_eq!(app.agents.waiting_count(), 1);
        // A row whose terminal has been replaced since it was drawn does nothing.
        app.action(&ctx, Action::OpenAgent(panes[0], rows[0].generation + 1));
        assert_eq!(app.controller.model().active_pane(), Some(panes[2]));
        app.action(&ctx, Action::OpenAgent(panes[0], rows[0].generation));
        assert_eq!(app.controller.model().active_pane(), Some(panes[0]));
        assert_eq!(
            app.controller.model().active_workspace(),
            app.controller.model().workspace_for_pane(panes[0])
        );
        assert!(app.agent_rows()[0].focused);
        // The agent leaves the list with its terminal's shell and with its close.
        app.controller
            .dispatch(Command::RestartPane(panes[0]))
            .unwrap();
        app.poll_agents(&ctx);
        assert_eq!(app.agent_rows().len(), 1);
        let generation = generation(&app, panes[1]);
        app.agents.report(panes[1], generation, None, now);
        app.poll_agents(&ctx);
        assert!(app.agent_rows().is_empty());
        assert_eq!(app.agents.waiting_count(), 0);
    }

    #[test]
    fn the_agents_tab_opens_the_panel_and_rests_the_explorer() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), "text").unwrap();
        let (mut app, ctx, _) = with_agents(dir.path(), &["One"]);
        let show = |app: &mut App, tab| app.action(&ctx, Action::Panel(Event::Show(tab)));
        show(&mut app, Tab::Files);
        assert!(app.ui.panel.open && app.ui.panel.tab == Tab::Files);
        settle(&mut app, &ctx, "the folder to be read", |app| {
            app.explorer_rows() > 0
        });
        // A name being typed does not outlive its tab.
        app.action(
            &ctx,
            Action::Explorer(ui::explorer::Event::BeginCreate {
                parent: None,
                folder: false,
            }),
        );
        assert!(app.ui.explorer.edit.is_some());
        show(&mut app, Tab::Agents);
        assert!(app.ui.panel.open && app.ui.panel.tab == Tab::Agents);
        assert!(app.ui.explorer.edit.is_none());
        settle(&mut app, &ctx, "the explorer to rest", |app| {
            app.explorer_resting()
        });
        // Showing the tab in view again changes nothing; the toggle closes it.
        show(&mut app, Tab::Agents);
        assert!(app.ui.panel.open);
        app.action(&ctx, Action::Panel(Event::Toggle));
        assert!(!app.ui.panel.open);
        // A closed panel opens on the tab asked for.
        show(&mut app, Tab::Files);
        assert!(app.ui.panel.open && app.ui.panel.tab == Tab::Files);
        settle(&mut app, &ctx, "the folder to be read again", |app| {
            !app.explorer_resting() && app.explorer_rows() > 0
        });
    }
}
