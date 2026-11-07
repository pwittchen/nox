//! Agent state machine (SPEC 7.12 "Agent state").
//!
//! One [`AgentTracker`] per agent pane (create it when the pane's foreground program becomes an
//! agent; replace it when a new agent starts). The app feeds it events; every mutator returns the
//! [`Transition`] it caused, if any, so the app can update dots and fire attention (7.11):
//! `transition.wants_attention(min_duration)`.
//!
//! **Source priority** ("the first available wins"):
//! 1. explicit report (`nox status`, hooks, MCP) — once a pane has reported, OSC notifications,
//!    titles and activity no longer change its state (the agent tells us precisely; an OSC 9
//!    still chimes through the app's generic *attention* trigger, so nothing is lost);
//! 2. OSC 9 / 777 notification → needs input (until a report has been seen);
//! 3. title patterns ([`crate::titles`]) — edge-triggered: only a *change* of the state the title
//!    signals counts, so an animated spinner doesn't pull a pane out of needs-input;
//! 4. activity (only while neither a report nor a known title has been seen): output → working,
//!    [`ACTIVITY_WINDOW`] quiet (see [`AgentTracker::tick`]) → idle. Output never leaves
//!    needs-input (TUIs redraw while they wait).
//!
//! In addition, user input in a pane that needs input means the user answered it →
//! working ([`AgentTracker::on_input`]), whatever the source. After exit every event is ignored.
//!
//! A **turn** starts when the agent enters working from idle and ends when it goes idle or exits;
//! needs-input in the middle (a permission prompt) doesn't end it. `turn_duration` is reported on
//! transitions to idle, needs-input and exited.

use std::fmt;
use std::str::FromStr;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::titles::title_state;

/// Output within this window counts as working for the activity heuristic.
pub const ACTIVITY_WINDOW: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentState {
    Working,
    NeedsInput,
    Idle,
    Exited,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentState::Working => "working",
            AgentState::NeedsInput => "needs-input",
            AgentState::Idle => "idle",
            AgentState::Exited => "exited",
        }
    }
}

impl fmt::Display for AgentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AgentState {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "working" => Ok(AgentState::Working),
            "needs-input" | "needs_input" | "needsinput" => Ok(AgentState::NeedsInput),
            "idle" => Ok(AgentState::Idle),
            "exited" => Ok(AgentState::Exited),
            _ => Err(format!("unknown agent state {s:?} (working, needs-input, idle, exited)")),
        }
    }
}

/// What set the current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StateSource {
    /// Initial idle state of a new tracker.
    Initial,
    Report,
    Notify,
    Title,
    Activity,
    /// The user typed into a pane that needed input.
    Input,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub from: AgentState,
    pub to: AgentState,
    /// Length of the turn so far, on transitions out of a turn (to idle / needs-input / exited).
    pub turn_duration: Option<Duration>,
}

impl Transition {
    /// Attention rule of 7.11: needs-input always; idle after a turn of at least `min`.
    pub fn wants_attention(&self, min: Duration) -> bool {
        match self.to {
            AgentState::NeedsInput => true,
            AgentState::Idle => self.turn_duration.is_some_and(|d| d >= min),
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentTracker {
    kind: String,
    state: AgentState,
    since: Instant,
    message: Option<String>,
    session: Option<String>,
    turn_started: Option<Instant>,
    source: StateSource,
    has_report: bool,
    has_title: bool,
    last_title_state: Option<AgentState>,
    last_output: Option<Instant>,
}

impl AgentTracker {
    /// A tracker for an agent of `kind` (`claude`, `codex`, a configured name) that just started.
    /// It starts idle (waiting for its first prompt).
    pub fn new(kind: impl Into<String>, now: Instant) -> Self {
        Self {
            kind: kind.into(),
            state: AgentState::Idle,
            since: now,
            message: None,
            session: None,
            turn_started: None,
            source: StateSource::Initial,
            has_report: false,
            has_title: false,
            last_title_state: None,
            last_output: None,
        }
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }
    pub fn state(&self) -> AgentState {
        self.state
    }
    /// When the current state was entered.
    pub fn since(&self) -> Instant {
        self.since
    }
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
    pub fn session_id(&self) -> Option<&str> {
        self.session.as_deref()
    }
    /// Start of the current turn, while one is in progress.
    pub fn turn_started(&self) -> Option<Instant> {
        self.turn_started
    }
    pub fn source(&self) -> StateSource {
        self.source
    }
    /// Has the agent ever reported explicitly (so heuristics are off)?
    pub fn has_report(&self) -> bool {
        self.has_report
    }

    /// Sets the session id learned elsewhere (e.g. [`session_id_from_cmdline`]).
    pub fn set_session_id(&mut self, session: Option<String>) {
        self.session = session;
    }

    fn exited(&self) -> bool {
        self.state == AgentState::Exited
    }

    fn set_state(&mut self, to: AgentState, source: StateSource, now: Instant) -> Option<Transition> {
        let from = self.state;
        if from == to {
            return None;
        }
        let turn_duration = match to {
            AgentState::Working => {
                self.turn_started.get_or_insert(now);
                None
            }
            AgentState::NeedsInput => self.turn_started.map(|t| now.saturating_duration_since(t)),
            AgentState::Idle | AgentState::Exited => {
                self.turn_started.take().map(|t| now.saturating_duration_since(t))
            }
        };
        self.state = to;
        self.since = now;
        self.source = source;
        Some(Transition { from, to, turn_duration })
    }

    /// An explicit report (`nox status <state> [-m msg] [--session id]`). Always wins.
    pub fn on_report(
        &mut self,
        state: AgentState,
        msg: Option<String>,
        session: Option<String>,
        now: Instant,
    ) -> Option<Transition> {
        if self.exited() {
            return None;
        }
        if session.is_some() {
            self.session = session;
        }
        self.has_report = true;
        if msg.is_some() || state != self.state {
            self.message = msg;
        }
        self.set_state(state, StateSource::Report, now)
    }

    /// An OSC 9 / 777 notification from the agent → needs input with `text` as message, unless
    /// the agent reports explicitly.
    pub fn on_osc_notify(&mut self, text: &str, now: Instant) -> Option<Transition> {
        if self.exited() || self.has_report {
            return None;
        }
        let text = text.trim();
        self.message = (!text.is_empty()).then(|| text.to_string());
        self.set_state(AgentState::NeedsInput, StateSource::Notify, now)
    }

    /// The pane title changed (OSC 0/2).
    pub fn on_title(&mut self, title: &str, now: Instant) -> Option<Transition> {
        if self.exited() || self.has_report {
            return None;
        }
        let (state, rest) = title_state(&self.kind, title)?;
        self.has_title = true;
        let edge = self.last_title_state != Some(state);
        self.last_title_state = Some(state);
        if !edge {
            if self.state == state && !rest.is_empty() {
                self.message = Some(rest.to_string());
            }
            return None;
        }
        self.message = (!rest.is_empty()).then(|| rest.to_string());
        self.set_state(state, StateSource::Title, now)
    }

    /// The pane produced output.
    pub fn on_output(&mut self, now: Instant) -> Option<Transition> {
        self.last_output = Some(now);
        if self.exited() || self.has_report || self.has_title || self.state != AgentState::Idle {
            return None;
        }
        self.message = None;
        self.set_state(AgentState::Working, StateSource::Activity, now)
    }

    /// The user typed into the pane: answering a pane that needs input resumes its turn.
    pub fn on_input(&mut self, now: Instant) -> Option<Transition> {
        if self.state != AgentState::NeedsInput {
            return None;
        }
        self.message = None;
        self.set_state(AgentState::Working, StateSource::Input, now)
    }

    /// The agent's process exited.
    pub fn on_exit(&mut self, now: Instant) -> Option<Transition> {
        self.set_state(AgentState::Exited, StateSource::Exit, now)
    }

    /// Periodic check for the activity heuristic (call it at least every ~500 ms while the pane
    /// is working, or schedule it at [`AgentTracker::next_tick`]).
    pub fn tick(&mut self, now: Instant) -> Option<Transition> {
        if self.state != AgentState::Working || self.has_report || self.has_title {
            return None;
        }
        let last = self.last_output.map_or(self.since, |o| o.max(self.since));
        if now.saturating_duration_since(last) >= ACTIVITY_WINDOW {
            self.set_state(AgentState::Idle, StateSource::Activity, now)
        } else {
            None
        }
    }

    /// When [`AgentTracker::tick`] could next change the state, if it can.
    pub fn next_tick(&self) -> Option<Instant> {
        if self.state != AgentState::Working || self.has_report || self.has_title {
            return None;
        }
        let last = self.last_output.map_or(self.since, |o| o.max(self.since));
        Some(last + ACTIVITY_WINDOW)
    }
}

/// The session id an agent was started with, from its arguments (after the executable or
/// script): `--resume <id>` / `--resume=<id>`, `-r <id>`, `--session-id <id>`, and for codex
/// `resume <id>` (not `resume --last`).
pub fn session_id_from_cmdline(kind: &str, args: &[String]) -> Option<String> {
    let valid = |s: &str| !s.is_empty() && !s.starts_with('-');
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        for flag in ["--resume", "--session-id"] {
            if let Some(v) = a.strip_prefix(flag).and_then(|r| r.strip_prefix('='))
                && valid(v)
            {
                return Some(v.to_string());
            }
        }
        let takes_id = matches!(a, "--resume" | "-r" | "--session-id") || (a == "resume" && kind == "codex");
        if takes_id
            && let Some(next) = args.get(i + 1)
            && valid(next)
        {
            return Some(next.clone());
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(10);

    fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn claude_without_hooks_titles_osc_and_input() {
        let t0 = Instant::now();
        let mut a = AgentTracker::new("claude", t0);
        // Startup: idle title, splash output — no activity heuristic once a title is known.
        assert_eq!(a.on_title("✳ Claude Code", at(t0, 100)), None);
        assert_eq!(a.on_output(at(t0, 120)), None);
        assert_eq!(a.state(), AgentState::Idle);
        // The user submits a prompt; the spinner animates.
        let tr = a.on_title("⠂ Fix the parser", at(t0, 1_000)).unwrap();
        assert_eq!((tr.from, tr.to, tr.turn_duration), (AgentState::Idle, AgentState::Working, None));
        assert_eq!(a.message(), Some("Fix the parser"));
        assert_eq!(a.turn_started(), Some(at(t0, 1_000)));
        for (i, frame) in ["⠐", "⠂", "⠐", "⠂"].iter().enumerate() {
            assert_eq!(a.on_title(&format!("{frame} Fix the parser"), at(t0, 1_100 + i as u64 * 100)), None);
            assert_eq!(a.on_output(at(t0, 1_150 + i as u64 * 100)), None);
        }
        assert_eq!(a.tick(at(t0, 5_900)), None);
        // Permission prompt.
        let tr = a.on_osc_notify("Claude needs your permission to use Bash", at(t0, 6_000)).unwrap();
        assert_eq!(tr.to, AgentState::NeedsInput);
        assert_eq!(tr.turn_duration, Some(Duration::from_secs(5)));
        assert!(tr.wants_attention(MIN));
        assert_eq!(a.message(), Some("Claude needs your permission to use Bash"));
        // Spinner keeps animating: no change.
        assert_eq!(a.on_title("⠐ Fix the parser", at(t0, 6_100)), None);
        assert_eq!(a.state(), AgentState::NeedsInput);
        // The user approves.
        let tr = a.on_input(at(t0, 8_000)).unwrap();
        assert_eq!(tr.to, AgentState::Working);
        assert_eq!(a.turn_started(), Some(at(t0, 1_000)), "the turn continues");
        // Done.
        let tr = a.on_title("✳ Fix the parser", at(t0, 21_000)).unwrap();
        assert_eq!(tr.to, AgentState::Idle);
        assert_eq!(tr.turn_duration, Some(Duration::from_secs(20)));
        assert!(tr.wants_attention(MIN));
        assert_eq!(a.turn_started(), None);
        assert_eq!(a.since(), at(t0, 21_000));
    }

    #[test]
    fn claude_with_hooks_reports_win() {
        let t0 = Instant::now();
        let mut a = AgentTracker::new("claude", t0);
        let tr = a.on_report(AgentState::Working, None, Some("s-1".into()), at(t0, 500)).unwrap();
        assert_eq!(tr.to, AgentState::Working);
        assert_eq!(a.session_id(), Some("s-1"));
        assert_eq!(a.source(), StateSource::Report);
        // Lower sources are ignored from now on.
        assert_eq!(a.on_title("✳ Claude Code", at(t0, 600)), None);
        assert_eq!(a.on_osc_notify("Claude is waiting", at(t0, 700)), None);
        assert_eq!(a.tick(at(t0, 5_000)), None);
        assert_eq!(a.state(), AgentState::Working);
        let tr = a
            .on_report(AgentState::NeedsInput, Some("Permission: Bash".into()), None, at(t0, 2_500))
            .unwrap();
        assert_eq!(tr.turn_duration, Some(Duration::from_secs(2)));
        assert_eq!(a.message(), Some("Permission: Bash"));
        assert_eq!(a.session_id(), Some("s-1"));
        let tr = a.on_report(AgentState::Idle, None, Some("s-1".into()), at(t0, 3_500)).unwrap();
        assert_eq!(tr.turn_duration, Some(Duration::from_secs(3)));
        assert!(!tr.wants_attention(MIN), "short turn stays quiet");
        assert_eq!(a.message(), None);
        // Same state again: no transition.
        assert_eq!(a.on_report(AgentState::Idle, None, None, at(t0, 4_000)), None);
    }

    #[test]
    fn codex_activity_heuristic() {
        let t0 = Instant::now();
        let mut a = AgentTracker::new("codex", t0);
        assert_eq!(a.on_title("codex", at(t0, 10)), None, "no title rule for codex");
        let tr = a.on_output(at(t0, 1_000)).unwrap();
        assert_eq!(tr.to, AgentState::Working);
        assert_eq!(a.on_output(at(t0, 1_200)), None);
        assert_eq!(a.tick(at(t0, 2_000)), None);
        assert_eq!(a.next_tick(), Some(at(t0, 2_700)));
        assert_eq!(a.on_output(at(t0, 2_500)), None);
        assert_eq!(a.tick(at(t0, 3_999)), None);
        let tr = a.tick(at(t0, 4_000)).unwrap();
        assert_eq!(tr.to, AgentState::Idle);
        assert_eq!(tr.turn_duration, Some(Duration::from_secs(3)));
        assert_eq!(a.next_tick(), None);
        // A burst, then an OSC notification: output doesn't leave needs-input, ticks neither.
        a.on_output(at(t0, 5_000)).unwrap();
        let tr = a.on_osc_notify("Approve command?", at(t0, 5_500)).unwrap();
        assert_eq!(tr.from, AgentState::Working);
        assert_eq!(a.on_output(at(t0, 5_600)), None);
        assert_eq!(a.tick(at(t0, 60_000)), None);
        assert_eq!(a.state(), AgentState::NeedsInput);
        assert_eq!(a.on_input(at(t0, 61_000)).unwrap().to, AgentState::Working);
        let tr = a.tick(at(t0, 62_500)).unwrap();
        assert_eq!(tr.to, AgentState::Idle);
        assert_eq!(tr.turn_duration, Some(Duration::from_millis(57_500)));
        assert!(tr.wants_attention(MIN));
    }

    #[test]
    fn gemini_titles_including_needs_input() {
        let t0 = Instant::now();
        let mut a = AgentTracker::new("gemini", t0);
        assert_eq!(a.on_title("◇  Ready (nox)", at(t0, 10)), None);
        assert_eq!(a.on_title("✦  Working… (nox)", at(t0, 1_000)).unwrap().to, AgentState::Working);
        let tr = a.on_title("✋  Action Required (nox)", at(t0, 2_000)).unwrap();
        assert_eq!(tr.to, AgentState::NeedsInput);
        assert!(tr.wants_attention(MIN));
        assert_eq!(a.on_title("✦  Working… (nox)", at(t0, 3_000)).unwrap().to, AgentState::Working);
        assert_eq!(
            a.on_title("◇  Ready (nox)", at(t0, 4_000)).unwrap().turn_duration,
            Some(Duration::from_secs(3))
        );
    }

    #[test]
    fn exit_ends_everything() {
        let t0 = Instant::now();
        let mut a = AgentTracker::new("claude", t0);
        a.on_output(at(t0, 100)).unwrap();
        let tr = a.on_exit(at(t0, 1_100)).unwrap();
        assert_eq!((tr.from, tr.to), (AgentState::Working, AgentState::Exited));
        assert_eq!(tr.turn_duration, Some(Duration::from_secs(1)));
        assert!(!tr.wants_attention(Duration::ZERO));
        assert_eq!(a.on_report(AgentState::Working, None, None, at(t0, 2_000)), None);
        assert_eq!(a.on_osc_notify("x", at(t0, 2_000)), None);
        assert_eq!(a.on_title("⠂ x", at(t0, 2_000)), None);
        assert_eq!(a.on_output(at(t0, 2_000)), None);
        assert_eq!(a.on_exit(at(t0, 3_000)), None);
        assert_eq!(a.state(), AgentState::Exited);
    }

    #[test]
    fn state_parsing() {
        assert_eq!("needs-input".parse::<AgentState>(), Ok(AgentState::NeedsInput));
        assert_eq!("Idle".parse::<AgentState>(), Ok(AgentState::Idle));
        assert!("busy".parse::<AgentState>().is_err());
        assert_eq!(serde_json::to_string(&AgentState::NeedsInput).unwrap(), "\"needs-input\"");
        assert_eq!(AgentState::NeedsInput.to_string(), "needs-input");
    }

    #[test]
    fn session_ids_from_command_lines() {
        assert_eq!(session_id_from_cmdline("claude", &args("--resume abc-123")).as_deref(), Some("abc-123"));
        assert_eq!(session_id_from_cmdline("claude", &args("--model opus -r abc")).as_deref(), Some("abc"));
        assert_eq!(session_id_from_cmdline("claude", &args("--resume=xyz")).as_deref(), Some("xyz"));
        assert_eq!(session_id_from_cmdline("claude", &args("--session-id 9f")).as_deref(), Some("9f"));
        assert_eq!(session_id_from_cmdline("claude", &args("--resume")), None);
        assert_eq!(session_id_from_cmdline("claude", &args("--resume --verbose")), None);
        assert_eq!(session_id_from_cmdline("claude", &args("--continue")), None);
        assert_eq!(session_id_from_cmdline("claude", &args("resume abc")), None);
        assert_eq!(session_id_from_cmdline("codex", &args("resume 0199-aa")).as_deref(), Some("0199-aa"));
        assert_eq!(session_id_from_cmdline("codex", &args("resume --last")), None);
        assert_eq!(session_id_from_cmdline("codex", &args("")), None);
    }
}
