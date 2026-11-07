//! Attention: a soft chime and a macOS notification when a job in a pane the user isn't
//! looking at finishes or needs them (SPEC §7.11).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use nox_agents::AgentState;
use nox_layout::PaneId;

#[derive(Default)]
pub struct Attention {
    last_chime: Option<Instant>,
    /// Panes that chimed and haven't been visited since.
    chimed: HashSet<PaneId>,
}

pub fn notification_id(pane: PaneId) -> String {
    format!("pane-{pane}")
}

pub fn pane_from_notification(id: &str) -> Option<PaneId> {
    id.strip_prefix("pane-")?.parse().ok().map(PaneId)
}

impl Attention {
    /// The pane was visited: it may chime again, and its notification goes away.
    pub fn visited(&mut self, pane: PaneId) {
        if self.chimed.remove(&pane) {
            nox_mac::remove_notification(&notification_id(pane));
        }
    }

    pub fn trigger(&mut self, pane: PaneId, title: &str, body: &str, cfg: &nox_config::Config) {
        let now = Instant::now();
        if cfg.terminal.attention_sound && self.should_chime(pane, now) {
            let path = crate::assets::resources_dir().join("sounds/attention.caf");
            if path.exists() {
                nox_mac::play_sound(&path, cfg.terminal.attention_sound_volume as f32);
            }
        }
        self.chimed.insert(pane);
        if cfg.terminal.notifications {
            nox_mac::notify(&notification_id(pane), title, body);
        }
    }

    /// Several triggers within 3 s play one chime; a pane chimes once until visited.
    pub fn should_chime(&mut self, pane: PaneId, now: Instant) -> bool {
        if self.chimed.contains(&pane) {
            return false;
        }
        if self.last_chime.is_some_and(|t| now.duration_since(t) < Duration::from_secs(3)) {
            return false;
        }
        self.last_chime = Some(now);
        true
    }
}

/// Notification text for a finished command.
pub fn done_text(command: &str, exit: Option<i32>, tab_title: &str, duration: Duration) -> (String, String) {
    let title = match exit {
        Some(code) if code != 0 => format!("{command} failed (exit {code})"),
        _ => format!("{command} finished"),
    };
    (title, format!("{tab_title} · {}", crate::util::short_duration(duration)))
}

/// Notification text for an agent state change.
pub fn agent_text(
    name: &str,
    to: AgentState,
    message: Option<&str>,
    workspace: &str,
    tab: &str,
) -> (String, String) {
    let title = match to {
        AgentState::NeedsInput => format!("{name} needs input"),
        _ => format!("{name} finished"),
    };
    let place = format!("{workspace} · {tab}");
    let body = match message {
        Some(m) if !m.is_empty() => format!("{m}\n{place}"),
        _ => place,
    };
    (title, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chime_dedupe() {
        let mut a = Attention::default();
        let t = Instant::now();
        assert!(a.should_chime(PaneId(1), t));
        a.chimed.insert(PaneId(1));
        assert!(!a.should_chime(PaneId(2), t + Duration::from_secs(1)), "within 3 s");
        assert!(a.should_chime(PaneId(2), t + Duration::from_secs(4)));
        a.chimed.insert(PaneId(2));
        assert!(!a.should_chime(PaneId(1), t + Duration::from_secs(10)), "not visited yet");
        a.chimed.remove(&PaneId(1));
        assert!(a.should_chime(PaneId(1), t + Duration::from_secs(10)));
    }

    #[test]
    fn texts() {
        let (t, b) = done_text("cargo build", Some(1), "2 ~/nox", Duration::from_secs(134));
        assert_eq!(t, "cargo build failed (exit 1)");
        assert_eq!(b, "2 ~/nox · 2m 14s");
        assert_eq!(done_text("make", Some(0), "x", Duration::from_secs(1)).0, "make finished");
        assert_eq!(pane_from_notification(&notification_id(PaneId(42))), Some(PaneId(42)));
    }
}
