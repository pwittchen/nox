//! Known agent title patterns (SPEC 4.6 step 3, 7.12 source 3), next to the icon table.
//!
//! | agent  | working                                   | needs input | idle |
//! |--------|-------------------------------------------|-------------|------|
//! | claude | braille spinner (`⠂ ⠐ …`), `✻ ✽ ✶ ✢ ·` prefix | —        | `✳`  |
//! | gemini | `✦` prefix                                 | `✋` prefix  | `◇`  |
//!
//! Codex, Copilot and the others don't set telling titles; they fall back to the activity
//! heuristic unless they report explicitly.

use crate::state::AgentState;

struct TitleRule {
    kind: &'static str,
    /// Does this title identify the agent (for icons / detection when the process is unknown)?
    identifies: fn(&str) -> bool,
    /// State and message (title without its status prefix).
    state: fn(&str) -> Option<(AgentState, &str)>,
}

const RULES: &[TitleRule] = &[
    TitleRule { kind: "claude", identifies: claude_identifies, state: claude_state },
    TitleRule { kind: "gemini", identifies: gemini_identifies, state: gemini_state },
];

fn split_prefix(title: &str) -> Option<(char, &str)> {
    let mut chars = title.chars();
    let first = chars.next()?;
    Some((first, chars.as_str().trim_start()))
}

fn is_braille_spinner(c: char) -> bool {
    ('\u{2801}'..='\u{28FF}').contains(&c)
}

fn claude_identifies(title: &str) -> bool {
    title.starts_with("✳ ") || title.contains("Claude Code")
}

fn claude_state(title: &str) -> Option<(AgentState, &str)> {
    let (c, rest) = split_prefix(title)?;
    match c {
        '✳' => Some((AgentState::Idle, rest)),
        '✻' | '✽' | '✶' | '✢' | '·' => Some((AgentState::Working, rest)),
        c if is_braille_spinner(c) => Some((AgentState::Working, rest)),
        _ => None,
    }
}

fn gemini_identifies(title: &str) -> bool {
    title.contains("Gemini CLI")
        || title.starts_with("◇  Ready")
        || title.starts_with("✦  Working")
        || title.starts_with("✋  Action Required")
}

fn gemini_state(title: &str) -> Option<(AgentState, &str)> {
    let (c, rest) = split_prefix(title)?;
    match c {
        '✦' => Some((AgentState::Working, rest)),
        '✋' => Some((AgentState::NeedsInput, rest)),
        '◇' => Some((AgentState::Idle, rest)),
        _ => None,
    }
}

/// The agent kind a title identifies (`✳ Claude Code` → `claude`).
#[must_use]
pub fn title_agent_kind(title: &str) -> Option<&'static str> {
    let title = title.trim();
    RULES.iter().find(|r| (r.identifies)(title)).map(|r| r.kind)
}

/// The state an agent of `kind` signals with `title`, with the rest of the title as message.
#[must_use]
pub fn title_state<'t>(kind: &str, title: &'t str) -> Option<(AgentState, &'t str)> {
    let rule = RULES.iter().find(|r| r.kind == kind)?;
    (rule.state)(title.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_titles() {
        assert_eq!(title_agent_kind("✳ Claude Code"), Some("claude"));
        assert_eq!(title_agent_kind("vim README.md"), None);
        assert_eq!(title_state("claude", "✳ Claude Code"), Some((AgentState::Idle, "Claude Code")));
        assert_eq!(title_state("claude", "⠂ Fix parser"), Some((AgentState::Working, "Fix parser")));
        assert_eq!(title_state("claude", "⠐ Fix parser"), Some((AgentState::Working, "Fix parser")));
        assert_eq!(title_state("claude", "✻ Fix parser"), Some((AgentState::Working, "Fix parser")));
        assert_eq!(title_state("claude", "plain"), None);
        assert_eq!(title_state("codex", "✳ x"), None);
    }

    #[test]
    fn gemini_titles() {
        assert_eq!(title_agent_kind("◇  Ready (nox)"), Some("gemini"));
        assert_eq!(title_state("gemini", "✦  Working… (nox)").map(|s| s.0), Some(AgentState::Working));
        assert_eq!(
            title_state("gemini", "✋  Action Required (nox)").map(|s| s.0),
            Some(AgentState::NeedsInput)
        );
        assert_eq!(title_state("gemini", "◇  Ready (nox)").map(|s| s.0), Some(AgentState::Idle));
    }
}
