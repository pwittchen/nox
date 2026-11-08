//! Human-readable rendering of IPC results and errors.

use std::fmt::Write as _;
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
            let _ = write!(out, "  {} held by {} ({})", c.path, c.holder_name, c.holder);
            if let Some(r) = c.reason.filter(|r| !r.is_empty()) {
                let _ = write!(out, ": {r}");
            }
            out.push('\n');
        }
    }
    out
}

fn pretty(v: &Value) -> String {
    if v.is_null() || v.as_object().is_some_and(serde_json::Map::is_empty) {
        return String::new();
    }
    serde_json::to_string_pretty(v).unwrap_or_default() + "\n"
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
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
        let _ = write!(out, "{} {}", ws.id, ws.name);
        if let Some(root) = &ws.root {
            let _ = write!(out, "  {}", home_short(root));
        }
        if ws.active {
            out.push_str("  (active)");
        }
        out.push('\n');
        for tab in &ws.tabs {
            let _ = writeln!(out, "  {} {}", tab.id, tab.title);
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
        let _ = writeln!(out, "[{} · {}] {}", m.from_name, ago(m.ts_ms, now), m.text);
    }
    out
}

pub fn render_claims(r: &ClaimsResult) -> String {
    let w = r.claims.iter().map(|c| c.path.chars().count()).max().unwrap_or(0);
    let mut out = String::new();
    for c in &r.claims {
        let mut line = format!("{}{}  {}", c.path, " ".repeat(w - c.path.chars().count()), c.holder_name);
        if let Some(reason) = c.reason.as_deref().filter(|r| !r.is_empty()) {
            let _ = write!(line, "  {reason}");
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
    fn render_each_method() {
        use nox_ipc::{ReadMode, WaitState};
        use serde_json::json;
        let read = Method::Read { pane: "x".into(), mode: ReadMode::default() };
        assert_eq!(render(&read, &json!({"text": ""})), "");
        assert_eq!(render(&read, &json!({"text": "a\n"})), "a\n");
        let msg = Method::Msg { to: "all".into(), text: "t".into() };
        assert_eq!(render(&msg, &json!({"ids": [1, 2], "recipients": ["a", "b"]})), "sent to a, b\n");
        assert_eq!(render(&msg, &json!({"ids": [], "recipients": []})), "");
        assert_eq!(render(&msg, &json!("garbage")), "");
        let wait = Method::Wait { pane: "x".into(), state: WaitState::Idle, timeout_s: None };
        assert_eq!(render(&wait, &json!({"state": "needs-input"})), "needs-input\n");
        let claims = Method::Claims;
        assert_eq!(
            render(&claims, &json!({"claims": [{"path": "a", "holder": 1, "holder_name": "n"}]})),
            "a  n\n"
        );
        let spawn =
            Method::Spawn { placement: nox_ipc::Placement::Tab, name: None, cwd: None, command: vec![] };
        assert_eq!(render(&spawn, &json!({"pane_id": 12})), "12\n");
        let open = Method::Open { path: "/x".into(), force_new: false };
        assert_eq!(render(&open, &json!({"workspace_id": 4})), "4\n");
        let inbox = Method::Inbox { wait: false, from: None, peek: false, timeout_s: None };
        assert_eq!(render(&inbox, &json!({"messages": []})), "");
        let ls = Method::Ls { all: false };
        assert_eq!(render(&ls, &json!({"workspaces": []})), "");
        for m in [
            Method::Status { state: nox_ipc::AgentStateWire::Idle, message: None, session: None },
            Method::Send { pane: "x".into(), text: "t".into(), submit: false },
            Method::Claim { paths: vec![], reason: None, wait: false, timeout_s: None },
            Method::Release { paths: vec![] },
            Method::Notify { title: "t".into(), body: None },
        ] {
            assert_eq!(render(&m, &json!({"anything": 1})), "", "{m:?}");
        }
    }

    #[test]
    fn unexpected_results_are_pretty_printed() {
        use nox_ipc::{ReadMode, WaitState};
        use serde_json::json;
        let odd = json!({"unexpected": true});
        let pretty_odd = "{\n  \"unexpected\": true\n}\n";
        for m in [
            Method::Ls { all: false },
            Method::Read { pane: "x".into(), mode: ReadMode::default() },
            Method::Inbox { wait: false, from: None, peek: false, timeout_s: None },
            Method::Wait { pane: "x".into(), state: WaitState::Idle, timeout_s: None },
            Method::Claims,
            Method::Spawn { placement: nox_ipc::Placement::Tab, name: None, cwd: None, command: vec![] },
            Method::Open { path: "/x".into(), force_new: false },
        ] {
            assert_eq!(render(&m, &odd), pretty_odd, "{m:?}");
            // Null and empty objects print nothing.
            assert_eq!(render(&m, &json!(null)), "", "{m:?}");
        }
        assert_eq!(render(&Method::Claims, &json!({})), "");
    }

    #[test]
    fn ago_buckets() {
        assert_eq!(ago(0, 59_999), "59s ago");
        assert_eq!(ago(0, 60_000), "1m ago");
        assert_eq!(ago(0, 3_599_999), "59m ago");
        assert_eq!(ago(0, 3_600_000), "1h ago");
        assert_eq!(ago(0, 86_399_999), "23h ago");
        assert_eq!(ago(0, 86_400_000 * 3), "3d ago");
        // Clock skew: a message from the future is "0s ago".
        assert_eq!(ago(10_000, 0), "0s ago");
        assert!(now_ms() > 1_600_000_000_000);
    }

    #[test]
    fn home_is_shortened_only_on_a_path_boundary() {
        let Some(home) = std::env::var("HOME").ok().filter(|h| !h.is_empty() && h != "/") else {
            return;
        };
        assert_eq!(home_short(&home), "~");
        assert_eq!(home_short(&format!("{home}/src")), "~/src");
        assert_eq!(home_short(&format!("{home}x/src")), format!("{home}x/src"));
        assert_eq!(home_short("/elsewhere"), "/elsewhere");
    }

    #[test]
    fn plain_and_conflict_errors() {
        assert_eq!(
            render_error(&IpcError::new(ErrorCode::NotFound, "no pane named x")),
            "nox: no pane named x\n"
        );
        // Conflict without parsable data: just the message.
        let e = IpcError::new(ErrorCode::Conflict, "held").with_data(serde_json::json!({"other": 1}));
        assert_eq!(render_error(&e), "nox: held\n");
        // An empty reason is not printed.
        let e = IpcError::new(ErrorCode::Conflict, "held").with_data(
            serde_json::to_value(ClaimConflictsData {
                conflicts: vec![ClaimConflictWire {
                    path: "a".into(),
                    holder: 1,
                    holder_name: "b".into(),
                    reason: Some(String::new()),
                }],
            })
            .unwrap(),
        );
        assert_eq!(render_error(&e), "nox: held\n  a held by b (1)\n");
    }

    #[test]
    fn silent_methods() {
        assert_eq!(render(&Method::Name { name: "x".into() }, &serde_json::json!({})), "");
        assert_eq!(
            render(
                &Method::Read { pane: "x".into(), mode: nox_ipc::ReadMode::default() },
                &serde_json::json!({"text":"a"})
            ),
            "a\n"
        );
    }
}
