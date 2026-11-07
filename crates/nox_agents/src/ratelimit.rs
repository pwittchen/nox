//! Loop guard (SPEC 7.13 "Safety"): a pane may send at most 20 messages / sends per 10 s.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct RateLimiter {
    max: usize,
    window: Duration,
    events: HashMap<u64, VecDeque<Instant>>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimiter {
    pub const MAX_EVENTS: usize = 20;
    pub const WINDOW: Duration = Duration::from_secs(10);

    /// 20 events per 10 s per pane.
    #[must_use]
    pub fn new() -> Self {
        Self::with_limit(Self::MAX_EVENTS, Self::WINDOW)
    }

    #[must_use]
    pub fn with_limit(max: usize, window: Duration) -> Self {
        Self { max, window, events: HashMap::new() }
    }

    /// Records an event of `pane` if allowed; `false` means the request must fail (and the user
    /// gets a notice). Rejected events don't count.
    pub fn check(&mut self, pane: u64, now: Instant) -> bool {
        let q = self.events.entry(pane).or_default();
        while q.front().is_some_and(|t| now.saturating_duration_since(*t) >= self.window) {
            q.pop_front();
        }
        if q.len() >= self.max {
            return false;
        }
        q.push_back(now);
        true
    }

    /// Forgets a closed pane.
    pub fn forget(&mut self, pane: u64) {
        self.events.remove(&pane);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_per_ten_seconds_per_pane() {
        let t0 = Instant::now();
        let mut r = RateLimiter::new();
        for i in 0..20 {
            assert!(r.check(1, t0 + Duration::from_millis(i * 100)), "{i}");
        }
        assert!(!r.check(1, t0 + Duration::from_secs(3)));
        assert!(r.check(2, t0 + Duration::from_secs(3)), "other panes are independent");
        // The first event leaves the window at t0 + 10 s.
        assert!(r.check(1, t0 + Duration::from_secs(10)));
        assert!(!r.check(1, t0 + Duration::from_millis(10_050)));
        assert!(r.check(1, t0 + Duration::from_millis(10_100)));
        r.forget(1);
        assert!(r.check(1, t0 + Duration::from_millis(10_150)));
    }
}
