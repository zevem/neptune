//! Turn running sessions into generation-tagged discovery targets.
use super::*;
use crate::runtime::ports::{RemoteTarget, Target};
impl App {
    pub(super) fn poll_ports(&mut self, ctx: &egui::Context) {
        let targets = self
            .sessions
            .iter()
            .filter_map(|(pane, session)| {
                let item = self.controller.model().pane(pane)?;
                if item.lifecycle() != &Lifecycle::Running
                    || self.sessions.generation(pane) != Some(item.generation())
                {
                    return None;
                }
                let metadata = session.metadata();
                if !matches!(metadata.status, SessionStatus::Running) {
                    return None;
                }
                let remote = self.remote_of(pane).map(|remote| RemoteTarget {
                    client: self.ssh_client.clone(),
                    destination: remote.destination().into(),
                    control: self.sessions.ssh_control(pane),
                });
                let pid = if remote.is_some() {
                    metadata.remote_process_id
                } else {
                    metadata.process_id
                }?;
                Some(Target {
                    pane,
                    generation: item.generation(),
                    pid,
                    remote,
                })
            })
            .collect();
        let context = ctx.clone();
        self.ports
            .sync(targets, Arc::new(move || context.request_repaint()));
    }
    pub(super) fn port_action(
        &mut self,
        ctx: &egui::Context,
        pane: PaneId,
        generation: u64,
        listener: crate::platform::ports::Listener,
        stop: bool,
    ) {
        if self
            .controller
            .model()
            .pane(pane)
            .is_none_or(|p| p.generation() != generation || p.lifecycle() != &Lifecycle::Running)
        {
            return;
        }
        if !stop
            && let Some(url) = self
                .ports
                .view(pane, generation)
                .iter()
                .find(|p| p.listener == listener)
                .and_then(|p| p.url())
        {
            self.action(
                ctx,
                Action::NewBrowser(pane, Some(neptune_model::Axis::Vertical), Some(url)),
            );
        } else if let Err(error) = self.ports.request(pane, generation, listener, stop) {
            self.ui.error = Some(error.into());
        }
        ctx.request_repaint();
    }
}
