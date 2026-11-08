//! Pixel-smooth scrolling (SPEC §7.3).
//!
//! Precise (trackpad) deltas accumulate into a pixel offset. Whole lines go to alacritty's
//! `display_offset`; the remaining fraction of a line is applied as a sub-line paint offset.
//! Line deltas (mouse wheels) are animated over `smooth_scroll_duration` with ease-out using the
//! same offset.

use std::time::{Duration, Instant};

#[derive(Debug, Default, Clone)]
pub struct ScrollState {
    /// Sub-line offset in pixels, `0 ≤ frac < line_height`: content is drawn shifted down by
    /// this much (revealing part of the line above the top row).
    pub frac: f32,
    /// Pending animated scroll (wheel notches).
    anim: Option<Anim>,
    /// Fractional lines accumulated on the alternate screen (sent as arrow keys / reports).
    pub alt_accum: f32,
    /// Last time the user scrolled (for the overlay scrollbar fade).
    pub last_scroll: Option<Instant>,
}

#[derive(Debug, Clone)]
struct Anim {
    start: Instant,
    duration: Duration,
    /// Total pixels to move (positive = up into history).
    total: f32,
    /// Pixels already applied.
    applied: f32,
}

/// Result of applying a pixel delta: whole lines to scroll the display by (positive = up into
/// history), the new fraction already stored.
pub struct Applied {
    pub lines: i32,
}

impl ScrollState {
    /// Apply `dy` pixels (positive = towards history, i.e. content moves down). `offset` is the
    /// current display offset, `max_offset` the history size.
    pub fn apply_pixels(&mut self, dy: f32, line_h: f32, offset: usize, max_offset: usize) -> Applied {
        self.last_scroll = Some(Instant::now());
        let total = self.frac + dy;
        let mut lines = (total / line_h).floor() as i32;
        let mut frac = total - lines as f32 * line_h;
        // Clamp to history bounds.
        let target = offset as i64 + i64::from(lines);
        if target <= 0 && (target < 0 || frac < 0.0) {
            lines = -(offset as i32);
            frac = 0.0;
        } else if target >= max_offset as i64 {
            lines = (max_offset - offset) as i32;
            frac = 0.0;
        }
        self.frac = frac.clamp(0.0, line_h - 0.001);
        if offset as i64 + i64::from(lines) == 0 && self.frac > 0.0 && dy < 0.0 {
            // Snapping to the bottom.
            self.frac = self.frac.max(0.0);
        }
        Applied { lines }
    }

    /// Start (or extend) an animated scroll of `lines` lines.
    pub fn animate_lines(&mut self, lines: f32, line_h: f32, duration: Duration) {
        let px = lines * line_h;
        let now = Instant::now();
        match &mut self.anim {
            Some(a) => {
                let remaining = a.total - a.applied;
                *a = Anim { start: now, duration, total: remaining + px, applied: 0.0 };
            }
            None => self.anim = Some(Anim { start: now, duration, total: px, applied: 0.0 }),
        }
    }

    /// Pixels to apply this frame for a running animation; `None` when idle.
    pub fn animation_step(&mut self, now: Instant) -> Option<f32> {
        let a = self.anim.as_mut()?;
        let t = if a.duration.is_zero() {
            1.0
        } else {
            (now.duration_since(a.start).as_secs_f32() / a.duration.as_secs_f32()).min(1.0)
        };
        let eased = 1.0 - (1.0 - t).powi(3);
        let target = a.total * eased;
        let step = target - a.applied;
        a.applied = target;
        if t >= 1.0 {
            self.anim = None;
        }
        Some(step)
    }

    #[must_use]
    pub fn animating(&self) -> bool {
        self.anim.is_some()
    }

    pub fn reset(&mut self) {
        self.frac = 0.0;
        self.anim = None;
        self.alt_accum = 0.0;
    }

    /// Opacity of the overlay scrollbar: visible while scrolling, fades out after 800 ms.
    #[must_use]
    pub fn scrollbar_alpha(&self, now: Instant) -> f32 {
        let Some(t) = self.last_scroll else { return 0.0 };
        let dt = now.duration_since(t).as_secs_f32();
        if dt < 0.8 {
            1.0
        } else if dt < 1.0 {
            1.0 - (dt - 0.8) / 0.2
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_fraction_and_whole_lines() {
        let mut s = ScrollState::default();
        let a = s.apply_pixels(10.0, 16.0, 0, 100);
        assert_eq!(a.lines, 0);
        assert!((s.frac - 10.0).abs() < 1e-3);
        let a = s.apply_pixels(10.0, 16.0, 0, 100);
        assert_eq!(a.lines, 1);
        assert!((s.frac - 4.0).abs() < 1e-3);
    }

    #[test]
    fn clamps_at_bottom_and_top() {
        let mut s = ScrollState::default();
        let a = s.apply_pixels(-50.0, 16.0, 1, 100);
        assert_eq!(a.lines, -1);
        assert_eq!(s.frac, 0.0);
        let a = s.apply_pixels(5000.0, 16.0, 90, 100);
        assert_eq!(a.lines, 10);
        assert_eq!(s.frac, 0.0);
    }

    #[test]
    fn animation_completes() {
        let mut s = ScrollState::default();
        s.animate_lines(3.0, 10.0, Duration::from_millis(100));
        let start = Instant::now();
        let mut total = 0.0;
        total += s.animation_step(start + Duration::from_millis(50)).unwrap();
        total += s.animation_step(start + Duration::from_millis(200)).unwrap();
        assert!((total - 30.0).abs() < 0.01);
        assert!(s.animation_step(start + Duration::from_millis(300)).is_none());
    }

    #[test]
    fn wheel_notches_extend_a_running_animation() {
        let mut s = ScrollState::default();
        s.animate_lines(2.0, 10.0, Duration::from_millis(100));
        let now = Instant::now();
        let first = s.animation_step(now + Duration::from_millis(50)).unwrap();
        assert!(first > 0.0 && first < 20.0, "ease-out: part of the way");
        // Another notch: the remainder plus the new distance, restarted.
        s.animate_lines(1.0, 10.0, Duration::from_millis(100));
        let rest = s.animation_step(Instant::now() + Duration::from_secs(1)).unwrap();
        assert!((first + rest - 30.0).abs() < 0.01);
        assert!(!s.animating());
        // Zero duration completes at once.
        s.animate_lines(-1.0, 10.0, Duration::ZERO);
        assert_eq!(s.animation_step(Instant::now()), Some(-10.0));
        assert!(s.animation_step(Instant::now()).is_none());
    }

    #[test]
    fn reset_clears_everything_but_the_scrollbar_timer() {
        let mut s = ScrollState::default();
        s.apply_pixels(5.0, 16.0, 0, 10);
        s.animate_lines(1.0, 16.0, Duration::from_millis(100));
        s.alt_accum = 0.5;
        s.reset();
        assert_eq!(s.frac, 0.0);
        assert_eq!(s.alt_accum, 0.0);
        assert!(!s.animating());
        assert!(s.last_scroll.is_some());
    }

    #[test]
    fn scrollbar_fades_after_800ms() {
        let mut s = ScrollState::default();
        let t = Instant::now();
        assert_eq!(s.scrollbar_alpha(t), 0.0, "hidden until scrolling");
        s.last_scroll = Some(t);
        assert_eq!(s.scrollbar_alpha(t + Duration::from_millis(500)), 1.0);
        assert!((s.scrollbar_alpha(t + Duration::from_millis(900)) - 0.5).abs() < 1e-3);
        assert_eq!(s.scrollbar_alpha(t + Duration::from_millis(1500)), 0.0);
    }

    #[test]
    fn scrolling_down_at_the_bottom_snaps_to_zero() {
        let mut s = ScrollState::default();
        s.apply_pixels(8.0, 16.0, 0, 10);
        let a = s.apply_pixels(-20.0, 16.0, 0, 10);
        assert_eq!(a.lines, 0);
        assert_eq!(s.frac, 0.0);
        // No history: nothing to scroll.
        let a = s.apply_pixels(40.0, 16.0, 0, 0);
        assert_eq!((a.lines, s.frac), (0, 0.0));
    }
}
