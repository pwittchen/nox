//! `nox_agents` — everything nox knows about coding agents and the programs running in panes,
//! independent of the UI: process icons and agent detection (SPEC 4.6, 4.8), the agent state
//! machine (7.12), session resume planning, the per-workspace message bus, activity log and
//! claims (7.13), the loop guard, git context and worktrees, project files and their trust
//! store (4.7, 11), agent hook installation and agent naming.
//!
//! Nothing here depends on GPUI or other nox crates; time-dependent logic takes `now` as a
//! parameter so it is deterministic under test.

// `cargo lint` runs clippy::pedantic. These lints are allowed on purpose: numeric casts are
// bounded grid / pixel / color math, `# Errors` / `# Panics` sections would only restate the
// signature, hex color literals read better without separators, and long UI builders and
// flag-heavy config structs are clearer kept whole; `assert!(x.is_empty())` reads fine in tests
// and `(n..).find(..)` searches are meant to be unbounded.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::unreadable_literal,
    clippy::too_many_lines,
    clippy::struct_excessive_bools,
    clippy::similar_names,
    clippy::many_single_char_names,
    clippy::assert_is_empty,
    clippy::maybe_infinite_iter
)]

pub mod bus;
pub mod claims;
pub mod git;
pub mod hooks;
pub mod icons;
pub mod project;
pub mod ratelimit;
pub mod resume;
pub mod state;
pub mod titles;

pub use bus::{
    ActivityEntry, ActivityKind, ActivityLog, MAX_MESSAGES, Message, MessageBus, MessageId, PaneRef,
    Recipient,
};
pub use claims::{Claim, ClaimConflict, Claims, DEFAULT_CLAIM_TTL, normalize_claim_path};
pub use git::{
    branch_from_head, git_branch, git_dir, repo_root, sanitize_branch, worktree_add, worktree_is_clean,
    worktree_path, worktree_remove,
};
pub use hooks::{CLAUDE_HOOKS, claude_hooks_patch, mcp_setup_command};
pub use icons::{
    BUILTIN_AGENT_KINDS, Icon, IconTable, ProcessInfo, brand_tint, builtin_icon_names, builtin_icon_svg,
};
pub use project::{
    PROJECT_FILE, ProjectFile, ProjectLayout, ProjectPane, ProjectTab, TrustStore, content_hash,
};
pub use ratelimit::RateLimiter;
pub use resume::{AgentDefLite, ResumeAction, ResumeCandidate, ResumePlan, plan_resume};
pub use state::{
    ACTIVITY_WINDOW, AgentState, AgentTracker, StateSource, Transition, session_id_from_cmdline,
};
pub use titles::{title_agent_kind, title_state};

/// Errors of the fallible operations of this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("cannot serialize TOML: {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The name for a new agent of `kind` in a workspace where `taken` names are in use (SPEC 4.8):
/// `claude`, then `claude-2`, `claude-3`, … (the first free one).
#[must_use]
pub fn unique_agent_name(kind: &str, taken: &[String]) -> String {
    let is_taken = |name: &str| taken.iter().any(|t| t == name);
    if !is_taken(kind) {
        return kind.to_string();
    }
    (2u32..)
        .map(|n| format!("{kind}-{n}"))
        .find(|name| !is_taken(name))
        .expect("unbounded range always yields a free name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_names_are_suffixed() {
        assert_eq!(unique_agent_name("claude", &[]), "claude");
        let taken = vec!["claude".to_string()];
        assert_eq!(unique_agent_name("claude", &taken), "claude-2");
        let taken = vec!["claude".into(), "claude-2".into(), "codex".into()];
        assert_eq!(unique_agent_name("claude", &taken), "claude-3");
        let taken = vec!["claude".into(), "claude-3".into()];
        assert_eq!(unique_agent_name("claude", &taken), "claude-2");
        assert_eq!(unique_agent_name("codex", &taken), "codex");
    }
}
