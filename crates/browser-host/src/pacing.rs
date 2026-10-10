//! CEF can receive native/GPU work without an external-pump deadline.
use super::protocol;
use std::time::{Duration, Instant};

pub(super) struct Pacing {
    visible: bool,
    frame_rate: u32,
    painted: Option<Instant>,
}
impl Default for Pacing {
    fn default() -> Self {
        Self {
            visible: true,
            frame_rate: protocol::DEFAULT_FRAME_RATE,
            painted: Some(Instant::now()),
        }
    }
}
impl Pacing {
    pub(super) fn paint(&mut self, now: Instant) {
        self.painted = Some(now);
    }
    pub(super) fn visible(&mut self, visible: bool) {
        self.visible = visible;
        self.painted = visible.then(Instant::now);
    }
    pub(super) fn frame_rate(&mut self, rate: u32) {
        self.frame_rate = rate;
    }
    pub(super) fn interval(&self, now: Instant) -> Duration {
        if self.visible
            && self
                .painted
                .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_millis(100))
        {
            // Leave room to service asynchronous capture/readback between
            // frames. Sleeping for an entire frame still introduces judder.
            Duration::from_nanos(1_000_000_000 / u64::from(self.frame_rate) / 4)
                .clamp(Duration::from_millis(1), Duration::from_millis(8))
        } else {
            Duration::from_millis(33)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_work_is_serviced_between_frames_then_returns_to_idle() {
        let now = Instant::now();
        let mut pacing = Pacing::default();
        pacing.frame_rate(144);
        pacing.paint(now);
        assert!(pacing.interval(now) < Duration::from_millis(2));
        assert_eq!(
            pacing.interval(now + Duration::from_millis(100)),
            Duration::from_millis(33)
        );
        pacing.visible(false);
        pacing.paint(now); // A late callback must not reactivate a hidden pane.
        assert_eq!(pacing.interval(now), Duration::from_millis(33));
        pacing.visible(true);
        assert!(pacing.interval(Instant::now()) < Duration::from_millis(2));
    }
}
