//! Human-readable rendering of IPC results and errors.

use std::time::{SystemTime, UNIX_EPOCH};

use nox_ipc::{
    ClaimConflictsData, ClaimsResult, ErrorCode, InboxResult, IpcError, LsResult, Method, MsgResult,
    OpenResult, ReadResult, SpawnResult, WaitResult,
};
use serde::Deserialize;
use serde_json::Value;

/// Text printed to stdout for a successful result (empty = print nothing).
pub fn render(method: &Method, value: &Value) -> String {
    match method {
        Method::Ls { .. } => match LsResult::deserialize(value) {
            Ok(ls) => render_ls(&ls),
            Err(_) => pretty(value),
        },
        Method::Read { .. } => match ReadResult::deserialize(value) {
            Ok(r) if r.text.is_empty() || r.text.ends_with('\n') => r.text,
            Ok(r) => r.text + "\n",
            Err(_) => pretty(value),
        },
        Method::Inbox { .. } => match InboxResult::deserialize(value) {
            Ok(r) => render_inbox(&r, now_ms()),
            Err(_) => pretty(value),
        },
        Method::Msg { .. } => match MsgResult::deserialize(value) {
            Ok(r) if r.recipients.is_empty() => String::new(),
            Ok(r) => format!("sent to {}\n", r.recipients.join(", ")),
            Err(_) => String::new(),
        },
        Method::Wait { .. } => match WaitResult::deserialize(value) {
            Ok(r) => format!("{}\n", r.state.as_str()),
            Err(_) => pretty(value),
        },
        Method::Claims => match ClaimsResult::deserialize(value) {
            Ok(r) => render_claims(&r),
            Err(_) => pretty(value),
        },
        Method::Spawn { .. } => match SpawnResult::deserialize(value) {
            Ok(r) => format!("{}\n", r.pane_id),
            Err(_) => pretty(value),
        },
        Method::Open { .. } => match OpenResult::deserialize(value) {
            Ok(r) => format!("{}\n", r.workspace_id),
            Err(_) => pretty(value),
        },
        Method::Name { .. }
        | Method::Status { .. }
        | Method::Send { .. }
        | Method::Claim { .. }
        | Method::Release { .. }
        | Method::Notify { .. } => String::new(),
    }
}

/// Text printed to stderr for an error.
pub fn render_error(err: &IpcError) -> String {
    let mut out = format!("nox: {}\n", err.message);
    if err.code == ErrorCode::Conflict
        && let Some(data) = &err.data
        && let Ok(c) = serde_json::from_value::<ClaimConflictsData>(data.clone())
    {
        for c in c.conflicts {
            out.push_str(&format!("  {} held by {} ({})", c.path, c.holder_name, c.holder));
            if let Some(r) = c.reason.filter(|r| !r.is_empty()) {
                out.push_str(&format!(": {r}"));
            }
            out.push('\n');
        }
    }
    out
}

fn pretty(v: &Value) -> String {
    if v.is_null() || v.as_object().is_some_and(|o| o.is_empty()) {
        return String::new();
    }
    serde_json::to_string_pretty(v).unwrap_or_default() + "\n"
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn home_short(path: &str) -> String {
    if let Some(home) = std::env::var_os("HOME").map(|h| h.to_string_lossy().into_owned())
        && !home.is_empty()
        && let Some(rest) = path.strip_prefix(&home)
        && (rest.is_empty() || rest.starts_with('/'))
    {
        return format!("~{rest}");
    }
    path.to_string()
}

/// Indented tree: workspace → tab → pane rows with aligned columns.
pub fn render_ls(ls: &LsResult) -> String {
    let mut rows: Vec<[String; 6]> = Vec::new();
    for ws in &ls.workspaces {
        for tab in &ws.tabs {
            for p in &tab.panes {
                rows.push([
                    format!("{}{}", if p.focused { "*" } else { " " }, p.id),
                    p.name.clone(),
                    p.program.clone().unwrap_or_default(),
                    p.agent_state.clone().unwrap_or_else(|| "-".into()),
                    p.cwd.as_deref().map(home_short).unwrap_or_default(),
                    p.branch.clone().unwrap_or_default(),
                ]);
            }
        }
    }
    let mut widths = [0usize; 6];
    for r in &rows {
        for (w, c) in widths.iter_mut().zip(r) {
            *w = (*w).max(c.chars().count());
        }
    }
    let mut out = String::new();
    let mut row_iter = rows.into_iter();
    for ws in &ls.workspaces {
        out.push_str(&format!("{} {}", ws.id, ws.name));
        if let Some(root) = &ws.root {
            out.push_str(&format!("  {}", home_short(root)));
        }
        if ws.active {
            out.push_str("  (active)");
        }
        out.push('\n');
        for tab in &ws.tabs {
            out.push_str(&format!("  {} {}\n", tab.id, tab.title));
            for _ in &tab.panes {
                let r = row_iter.next().expect("one row per pane");
                let mut line = String::from("   ");
                for (i, (cell, w)) in r.iter().zip(widths).enumerate() {
                    if i > 0 {
                        line.push_str("  ");
                    }
                    line.push_str(cell);
                    line.push_str(&" ".repeat(w - cell.chars().count()));
                }
                out.push_str(line.trim_end());
                out.push('\n');
            }
        }
    }
    out
}

fn ago(ts_ms: u64, now: u64) -> String {
    let s = now.saturating_sub(ts_ms) / 1000;
    match s {
        0..60 => format!("{s}s ago"),
        60..3600 => format!("{}m ago", s / 60),
        3600..86400 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

pub fn render_inbox(r: &InboxResult, now: u64) -> String {
    let mut out = String::new();
    for m in &r.messages {
        out.push_str(&format!("[{} · {}] {}\n", m.from_name, ago(m.ts_ms, now), m.text));
    }
    out
}

pub fn render_claims(r: &ClaimsResult) -> String {
    let w = r.claims.iter().map(|c| c.path.chars().count()).max().unwrap_or(0);
    let mut out = String::new();
    for c in &r.claims {
        let mut line = format!("{}{}  {}", c.path, " ".repeat(w - c.path.chars().count()), c.holder_name);
        if let Some(reason) = c.reason.as_deref().filter(|r| !r.is_empty()) {
            line.push_str(&format!("  {reason}"));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use nox_ipc::{ClaimConflictWire, ClaimWire, MessageWire, PaneInfo, TabInfo, WsInfo};

    #[test]
    fn ls_tree() {
        let ls = LsResult {
            workspaces: vec![
                WsInfo {
                    id: 1,
                    name: "nox".into(),
                    root: Some("/r/nox".into()),
                    active: true,
                    tabs: vec![TabInfo {
                        id: 2,
                        title: "agent".into(),
                        panes: vec![
                            PaneInfo {
                                id: 3,
                                name: "claude".into(),
                                program: Some("claude".into()),
                                agent_state: Some("working".into()),
                                cwd: Some("/r/nox".into()),
                                branch: Some("main".into()),
                                focused: true,
                            },
                            PaneInfo {
                                id: 14,
                                name: "shell".into(),
                                program: Some("zsh".into()),
                                ..Default::default()
                            },
                        ],
                    }],
                },
                WsInfo { id: 5, name: "scratch".into(), ..Default::default() },
            ],
        };
        let s = render_ls(&ls);
        assert_eq!(
            s,
            "1 nox  /r/nox  (active)\n  2 agent\n   *3   claude  claude  working  /r/nox  main\n    14  shell   zsh     -\n5 scratch\n"
        );
    }

    #[test]
    fn inbox_and_claims() {
        let r = InboxResult {
            messages: vec![MessageWire {
                id: 1,
                from: 2,
                from_name: "backend".into(),
                text: "ready".into(),
                ts_ms: 1_000,
            }],
        };
        assert_eq!(render_inbox(&r, 66_000), "[backend · 1m ago] ready\n");
        let c = ClaimsResult {
            claims: vec![
                ClaimWire {
                    path: "src/a.rs".into(),
                    holder: 1,
                    holder_name: "x".into(),
                    reason: Some("why".into()),
                    since_ms: 0,
                },
                ClaimWire { path: "b".into(), holder: 2, holder_name: "y".into(), reason: None, since_ms: 0 },
            ],
        };
        assert_eq!(render_claims(&c), "src/a.rs  x  why\nb         y\n");
    }

    #[test]
    fn conflict_error() {
        let err = IpcError::new(ErrorCode::Conflict, "claim conflict").with_data(
            serde_json::to_value(ClaimConflictsData {
                conflicts: vec![ClaimConflictWire {
                    path: "src/parser/lexer.rs".into(),
                    holder: 4,
                    holder_name: "backend".into(),
                    reason: Some("lexer rewrite".into()),
                }],
            })
            .unwrap(),
        );
        assert_eq!(
            render_error(&err),
            "nox: claim conflict\n  src/parser/lexer.rs held by backend (4): lexer rewrite\n"
        );
    }

    #[test]
    fn silent_methods() {
        assert_eq!(render(&Method::Name { name: "x".into() }, &serde_json::json!({})), "");
        assert_eq!(
            render(
                &Method::Read { pane: "x".into(), mode: Default::default() },
                &serde_json::json!({"text":"a"})
            ),
            "a\n"
        );
    }
}
