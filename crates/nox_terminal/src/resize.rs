//! PTY resize coalescing (SPEC §7.4).
//!
//! While the grid size keeps changing (window corner or divider drag), the PTY is resized at
//! most once per [`INTERVAL`], and once more with the final size [`SETTLE`] after the changes
//! stop. Shells and full-screen programs then redraw a few times, not on every pixel.

use std::time::{Duration, Instant};

pub const INTERVAL: Duration = Duration::from_millis(33);
pub const SETTLE: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_w: u16,
    pub cell_h: u16,
}

#[derive(Debug)]
pub struct ResizeCoalescer {
    /// Size the PTY currently has.
    sent: Option<GridSize>,
    /// Latest requested size, not yet sent.
    pending: Option<GridSize>,
    last_send: Option<Instant>,
    last_request: Option<Instant>,
}

impl Default for ResizeCoalescer {
    fn default() -> Self {
        Self::new()
    }
}

impl ResizeCoalescer {
    #[must_use]
    pub fn new() -> Self {
        Self { sent: None, pending: None, last_send: None, last_request: None }
    }

    /// The size the PTY was created with.
    #[must_use]
    pub fn with_initial(size: GridSize) -> Self {
        Self { sent: Some(size), ..Self::new() }
    }

    /// A new size was requested. Returns a size to send right now, if any.
    pub fn request(&mut self, size: GridSize, now: Instant) -> Option<GridSize> {
        self.last_request = Some(now);
        if self.sent == Some(size) {
            self.pending = None;
            return None;
        }
        self.pending = Some(size);
        let ready = self.last_send.is_none_or(|t| now.duration_since(t) >= INTERVAL);
        if ready { self.take(now) } else { None }
    }

    /// Called when the deadline returned by [`Self::deadline`] passes.
    pub fn poll(&mut self, now: Instant) -> Option<GridSize> {
        let pending = self.pending?;
        let since_send = self.last_send.map(|t| now.duration_since(t));
        let since_req = self.last_request.map(|t| now.duration_since(t));
        let interval_ok = since_send.is_none_or(|d| d >= INTERVAL);
        let settled = since_req.is_none_or(|d| d >= SETTLE);
        if (interval_ok || settled) && self.sent != Some(pending) { self.take(now) } else { None }
    }

    /// When `poll` should be called next, if something is pending.
    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        self.pending?;
        let by_interval = self.last_send.map(|t| t + INTERVAL);
        let by_settle = self.last_request.map(|t| t + SETTLE);
        match (by_interval, by_settle) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    #[must_use]
    pub fn current(&self) -> Option<GridSize> {
        self.pending.or(self.sent)
    }

    fn take(&mut self, now: Instant) -> Option<GridSize> {
        let size = self.pending.take()?;
        self.sent = Some(size);
        self.last_send = Some(now);
        Some(size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sz(cols: u16) -> GridSize {
        GridSize { cols, rows: 24, cell_w: 8, cell_h: 16 }
    }

    #[test]
    fn first_request_is_sent_immediately() {
        let mut c = ResizeCoalescer::with_initial(sz(80));
        let t = Instant::now();
        assert_eq!(c.request(sz(81), t), Some(sz(81)));
        assert_eq!(c.request(sz(81), t), None, "same size is not re-sent");
    }

    #[test]
    fn current_tracks_pending_then_sent() {
        let mut c = ResizeCoalescer::default();
        assert_eq!(c.current(), None);
        assert_eq!(c.deadline(), None);
        assert_eq!(c.poll(Instant::now()), None);
        let t = Instant::now();
        assert_eq!(c.request(sz(90), t), Some(sz(90)));
        assert_eq!(c.current(), Some(sz(90)));
        // Within the interval: held back, but reported as the current size.
        assert_eq!(c.request(sz(91), t + Duration::from_millis(1)), None);
        assert_eq!(c.current(), Some(sz(91)));
        assert_eq!(c.deadline(), Some(t + INTERVAL));
        // Going back to the size the PTY already has cancels the pending one.
        assert_eq!(c.request(sz(90), t + Duration::from_millis(2)), None);
        assert_eq!(c.current(), Some(sz(90)));
        assert_eq!(c.deadline(), None);
    }

    #[test]
    fn poll_before_deadline_sends_nothing() {
        let mut c = ResizeCoalescer::with_initial(sz(80));
        let t = Instant::now();
        assert_eq!(c.request(sz(81), t), Some(sz(81)));
        let t1 = t + Duration::from_millis(10);
        assert_eq!(c.request(sz(82), t1), None);
        assert_eq!(c.poll(t1 + Duration::from_millis(1)), None);
        assert_eq!(c.poll(t + INTERVAL), Some(sz(82)));
    }

    #[test]
    fn rapid_changes_are_throttled_then_settle() {
        let mut c = ResizeCoalescer::with_initial(sz(80));
        let t0 = Instant::now();
        let mut sent = vec![];
        // A drag producing a new size every 4 ms for 200 ms.
        for i in 0..50u16 {
            let t = t0 + Duration::from_millis(4 * u64::from(i));
            if let Some(s) = c.poll(t) {
                sent.push(s);
            }
            if let Some(s) = c.request(sz(81 + i), t) {
                sent.push(s);
            }
        }
        // Roughly one send per 33 ms over 200 ms.
        assert!(sent.len() <= 8 && sent.len() >= 4, "sent {}", sent.len());
        // After the drag stops, the final size goes out once the deadline passes.
        let end = t0 + Duration::from_millis(4 * 49);
        let deadline = c.deadline();
        if let Some(d) = deadline {
            assert!(d <= end + SETTLE);
            assert_eq!(c.poll(end + SETTLE), Some(sz(130)));
        } else {
            assert_eq!(sent.last(), Some(&sz(130)));
        }
        assert_eq!(c.poll(end + SETTLE * 2), None);
        assert_eq!(c.deadline(), None);
    }
}
