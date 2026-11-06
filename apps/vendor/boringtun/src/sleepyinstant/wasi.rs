use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Instant {
    t: std::time::Instant,
}

impl Instant {
    pub(crate) fn now() -> Self {
        Self { t: std::time::Instant::now() }
    }

    pub(crate) fn duration_since(&self, earlier: Instant) -> Duration {
        self.t.saturating_duration_since(earlier.t)
    }
}
