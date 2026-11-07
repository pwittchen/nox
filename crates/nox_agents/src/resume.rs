//! Planning agent resumption after a relaunch (SPEC 7.12 "Resuming agents").

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

/// An agent pane saved in the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeCandidate {
    pub pane: u64,
    /// Agent kind / `[agents.<name>]` key.
    pub kind: String,
    pub cwd: PathBuf,
    /// The original command line the agent was started with.
    pub command: String,
    pub session_id: Option<String>,
}

/// The resume-related part of an `[agents.<name>]` definition.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentDefLite {
    pub command: String,
    /// With a `{session}` placeholder, e.g. `claude --resume {session}`.
    pub resume: Option<String>,
    /// e.g. `claude --continue`.
    pub resume_last: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeAction {
    /// Run this command (through the user's shell, in the saved cwd).
    Run(String),
    /// No resume configured: type this (the original command line) at the prompt, don't run it.
    TypeOnly(String),
    /// Several same-kind agents in one directory without session ids: `resume_last` would open
    /// the same conversation twice. Listed in the resume sheet as "session unknown".
    SessionUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumePlan {
    pub pane: u64,
    pub action: ResumeAction,
}

/// Quotes a session id for the shell when it contains anything beyond `[A-Za-z0-9._:@+=/-]`.
fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "._:@+=/-".contains(c));
    if safe { s.to_string() } else { format!("'{}'", s.replace('\'', r"'\''")) }
}

fn non_empty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Plans how to bring back each agent pane, in order. Rules:
/// - session id known and `resume` set → `resume` with `{session}` replaced;
/// - otherwise `resume_last`, but only for the first pane (in input order) of each
///   (kind, cwd) group without an id; the others get [`ResumeAction::SessionUnknown`];
/// - no `resume` / `resume_last` configured (or kind unknown) → [`ResumeAction::TypeOnly`] with
///   the original command (falling back to the definition's `command`);
/// - only `resume` configured and no id → [`ResumeAction::SessionUnknown`].
pub fn plan_resume(agents: &[ResumeCandidate], defs: &BTreeMap<String, AgentDefLite>) -> Vec<ResumePlan> {
    let mut last_used: HashSet<(&str, &PathBuf)> = HashSet::new();
    agents
        .iter()
        .map(|a| {
            let def = defs.get(&a.kind);
            let resume = def.and_then(|d| non_empty(&d.resume));
            let resume_last = def.and_then(|d| non_empty(&d.resume_last));
            let session = a.session_id.as_deref().filter(|s| !s.is_empty());
            let action = match (resume, resume_last, session) {
                (Some(tpl), _, Some(id)) => ResumeAction::Run(tpl.replace("{session}", &shell_quote(id))),
                (None, None, _) => {
                    let cmd = if a.command.trim().is_empty() {
                        def.map(|d| d.command.clone()).unwrap_or_default()
                    } else {
                        a.command.clone()
                    };
                    ResumeAction::TypeOnly(cmd)
                }
                (_, Some(last), _) => {
                    if last_used.insert((a.kind.as_str(), &a.cwd)) {
                        ResumeAction::Run(last.to_string())
                    } else {
                        ResumeAction::SessionUnknown
                    }
                }
                (Some(_), None, None) => ResumeAction::SessionUnknown,
            };
            ResumePlan { pane: a.pane, action }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(pane: u64, kind: &str, cwd: &str, session: Option<&str>) -> ResumeCandidate {
        ResumeCandidate {
            pane,
            kind: kind.into(),
            cwd: cwd.into(),
            command: format!("{kind} --model x"),
            session_id: session.map(Into::into),
        }
    }

    fn defs() -> BTreeMap<String, AgentDefLite> {
        let mut d = BTreeMap::new();
        d.insert(
            "claude".into(),
            AgentDefLite {
                command: "claude".into(),
                resume: Some("claude --resume {session}".into()),
                resume_last: Some("claude --continue".into()),
            },
        );
        d.insert(
            "codex".into(),
            AgentDefLite {
                command: "codex".into(),
                resume: Some("codex resume {session}".into()),
                resume_last: None,
            },
        );
        d.insert("plain".into(), AgentDefLite { command: "plain".into(), resume: None, resume_last: None });
        d
    }

    fn actions(c: &[ResumeCandidate]) -> Vec<ResumeAction> {
        plan_resume(c, &defs()).into_iter().map(|p| p.action).collect()
    }

    #[test]
    fn session_known() {
        assert_eq!(
            actions(&[cand(1, "claude", "/a", Some("abc-1")), cand(2, "codex", "/a", Some("x y"))]),
            vec![
                ResumeAction::Run("claude --resume abc-1".into()),
                ResumeAction::Run("codex resume 'x y'".into()),
            ]
        );
    }

    #[test]
    fn session_unknown_uses_resume_last_once_per_dir() {
        let plan = plan_resume(
            &[
                cand(1, "claude", "/a", None),
                cand(2, "claude", "/a", None),
                cand(3, "claude", "/b", None),
                cand(4, "claude", "/a", Some("s4")),
            ],
            &defs(),
        );
        assert_eq!(plan.iter().map(|p| p.pane).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        assert_eq!(plan[0].action, ResumeAction::Run("claude --continue".into()));
        assert_eq!(plan[1].action, ResumeAction::SessionUnknown);
        assert_eq!(plan[2].action, ResumeAction::Run("claude --continue".into()));
        assert_eq!(plan[3].action, ResumeAction::Run("claude --resume s4".into()));
    }

    #[test]
    fn resume_without_resume_last_and_no_id() {
        assert_eq!(actions(&[cand(1, "codex", "/a", None)]), vec![ResumeAction::SessionUnknown]);
    }

    #[test]
    fn no_resume_configured_types_the_command() {
        assert_eq!(
            actions(&[cand(1, "plain", "/a", Some("s")), cand(2, "unknown", "/a", None)]),
            vec![
                ResumeAction::TypeOnly("plain --model x".into()),
                ResumeAction::TypeOnly("unknown --model x".into()),
            ]
        );
        let mut c = cand(3, "plain", "/a", None);
        c.command = String::new();
        assert_eq!(actions(&[c]), vec![ResumeAction::TypeOnly("plain".into())]);
    }
}
