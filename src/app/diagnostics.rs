//! Opt-in structured counters never include terminal contents or input.
use crate::{runtime::sessions::SessionManager, ui::PaneRender};
use neptune_model::PaneId;
use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};
pub(super) struct Diagnostics {
    enabled: bool,
    frames: VecDeque<f64>,
    last: Instant,
    total: u64,
    /// Every failure line, whether or not it was printed, for tests of
    /// what such a line may hold.
    #[cfg(test)]
    pub said: std::cell::RefCell<Vec<String>>,
}
impl Diagnostics {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            frames: VecDeque::with_capacity(2048),
            last: Instant::now(),
            total: 0,
            #[cfg(test)]
            said: Default::default(),
        }
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn failure(
        &self,
        operation: &str,
        pane: Option<PaneId>,
        generation: Option<u64>,
        kind: &str,
    ) {
        let line = || {
            serde_json::json!({"operation":"failure","source_operation":operation,"pane":pane.map(PaneId::get),"generation":generation,"error_kind":kind})
                .to_string()
        };
        #[cfg(test)]
        self.said.borrow_mut().push(line());
        if self.enabled {
            eprintln!("{}", line());
        }
    }
    pub fn operation(&self, operation: &str, pane: PaneId, generation: u64, elapsed: Duration) {
        if self.enabled {
            eprintln!(
                "{}",
                serde_json::json!({"operation":operation,"pane":pane.get(),"generation":generation,"elapsed_ms":elapsed.as_secs_f64()*1000.0})
            );
        }
    }
    pub fn frame(
        &mut self,
        elapsed: Duration,
        sessions: &SessionManager,
        renders: &BTreeMap<PaneId, PaneRender>,
        model: &neptune_model::Model,
    ) {
        if !self.enabled {
            return;
        }
        self.total += 1;
        if self.frames.len() == 2048 {
            self.frames.pop_front();
        }
        self.frames.push_back(elapsed.as_secs_f64() * 1000.0);
        if self.last.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last = Instant::now();
        let mut values = self.frames.iter().copied().collect::<Vec<_>>();
        values.sort_by(f64::total_cmp);
        let percentile = |p: f64| {
            values
                .get(((values.len().saturating_sub(1)) as f64 * p) as usize)
                .copied()
                .unwrap_or(0.0)
        };
        let panes=sessions.iter().map(|(id,session)|{let m=session.metrics();serde_json::json!({"workspace":model.workspace_for_pane(id).map(|w|w.get()),"pane":id.get(),"generation":sessions.generation(id),"received":m.bytes_received,"parsed":m.bytes_parsed,"parser_ns":m.parse_nanoseconds,"workers":m.active_workers,"revision":m.revision,"snapshot_ns":m.snapshot_nanoseconds,"snapshot_lock_ns":m.snapshot_lock_nanoseconds,"snapshot_cells":m.snapshot_cells,"snapshot_rows":m.snapshot_rows,"snapshot_count":m.snapshot_count,"queued_input_bytes":m.queued_input_bytes,"input_backpressure":m.input_backpressure,"dropped_events":m.dropped_events,"spawn_ns":m.spawn_nanoseconds,"cleanup_ns":m.cleanup_nanoseconds,"row_rebuilds":renders.get(&id).map(|r|r.cache.render_rebuilds)})}).collect::<Vec<_>>();
        eprintln!(
            "{}",
            serde_json::json!({"operation":"frames","count":self.total,"p50_ms":percentile(0.5),"p95_ms":percentile(0.95),"p99_ms":percentile(0.99),"resources":sessions.usage(),"cache_estimated_bytes":renders.values().map(|r|r.cache.estimated_bytes()).sum::<usize>(),"panes":panes})
        );
    }
    pub fn shutdown(&self, complete: bool) {
        if self.enabled {
            eprintln!(
                "{}",
                serde_json::json!({"operation":"shutdown","complete":complete})
            );
        }
    }
}
