//! End-to-end: the built `nox` binary against a `nox_ipc` server with a mock handler.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nox_ipc::{
    Caller, ClaimConflictWire, ClaimConflictsData, ErrorCode, Handler, InboxResult, IpcError, LsResult,
    MessageWire, Method, MsgResult, PaneInfo, Responder, Server, ServerHandle, TabInfo, WsInfo,
};
use serde_json::json;

#[derive(Default)]
struct Mock {
    seen: Mutex<Vec<(Option<Caller>, Method)>>,
}

impl Handler for Mock {
    fn handle(&self, caller: Option<Caller>, method: Method, reply: Responder) {
        self.seen.lock().unwrap().push((caller, method.clone()));
        match method {
            Method::Ls { .. } => reply.ok_typed(&LsResult {
                workspaces: vec![WsInfo {
                    id: 1,
                    name: "nox".into(),
                    root: Some("/r/nox".into()),
                    active: true,
                    tabs: vec![TabInfo {
                        id: 2,
                        title: "agent".into(),
                        panes: vec![PaneInfo {
                            id: 3,
                            name: "backend".into(),
                            program: Some("claude".into()),
                            agent_state: Some("working".into()),
                            cwd: Some("/r/nox".into()),
                            branch: Some("main".into()),
                            focused: true,
                        }],
                    }],
                }],
            }),
            Method::Msg { to, .. } => reply.ok_typed(&MsgResult { ids: vec![1], recipients: vec![to] }),
            Method::Inbox { wait: true, .. } => {
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(300));
                    reply.ok_typed(&InboxResult {
                        messages: vec![MessageWire {
                            id: 9,
                            from: 3,
                            from_name: "backend".into(),
                            text: "feat/x is ready".into(),
                            ts_ms: 0,
                        }],
                    });
                });
            }
            Method::Claim { .. } => reply.error(
                IpcError::new(ErrorCode::Conflict, "claim conflict").with_data(
                    serde_json::to_value(ClaimConflictsData {
                        conflicts: vec![ClaimConflictWire {
                            path: "src/parser/lexer.rs".into(),
                            holder: 3,
                            holder_name: "backend".into(),
                            reason: Some("lexer rewrite".into()),
                        }],
                    })
                    .unwrap(),
                ),
            ),
            Method::Wait { .. } => reply.err(ErrorCode::Timeout, "pane did not become idle"),
            Method::Read { .. } => reply.ok(json!({"text": "hello\nworld"})),
            Method::Open { path, .. } => reply.ok(json!({"workspace_id": path.len()})),
            _ => reply.ok(json!({})),
        }
    }
}

// Field order matters: the server stops (and removes its socket) before the dir goes away.
struct Fixture {
    handle: ServerHandle,
    mock: Arc<Mock>,
    path: PathBuf,
    token: String,
    _dir: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nox.sock");
    let mock = Arc::new(Mock::default());
    let handle = Server::start(&path, mock.clone()).unwrap();
    let token = handle.tokens().issue(Caller { pane_id: 7, tab_id: 2, workspace_id: 1 });
    Fixture { _dir: dir, handle, mock, path, token }
}

fn nox(socket: &Path, token: Option<&str>, args: &[&str], stdin: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nox"));
    cmd.args(args)
        .env("NOX_SOCKET", socket)
        .env_remove("NOX_TOKEN")
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(t) = token {
        cmd.env("NOX_TOKEN", t);
    }
    let mut child = cmd.spawn().unwrap();
    if let Some(input) = stdin {
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    }
    child.wait_with_output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn ls_human_and_json() {
    let f = fixture();
    let o = nox(&f.path, Some(&f.token), &["ls"], None);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let s = stdout(&o);
    assert!(s.starts_with("1 nox  /r/nox  (active)\n  2 agent\n"), "{s}");
    assert!(s.contains("*3  backend  claude  working  /r/nox  main"), "{s}");

    let o = nox(&f.path, Some(&f.token), &["ls", "--json"], None);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["workspaces"][0]["tabs"][0]["panes"][0]["name"], "backend");

    // Without a token: workspaces only.
    let o = nox(&f.path, None, &["--json", "ls"], None);
    assert_eq!(o.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["workspaces"][0]["tabs"], json!([]));
}

#[test]
fn msg_sends_with_identity() {
    let f = fixture();
    let o = nox(&f.path, Some(&f.token), &["msg", "reviewer", "please review"], None);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(stdout(&o), "sent to reviewer\n");
    let seen = f.mock.seen.lock().unwrap();
    let (caller, method) = seen.last().unwrap();
    assert_eq!(caller.unwrap().pane_id, 7);
    assert_eq!(method, &Method::Msg { to: "reviewer".into(), text: "please review".into() });
}

#[test]
fn msg_from_stdin_and_unauthorized() {
    let f = fixture();
    let o = nox(&f.path, Some(&f.token), &["msg", "all", "-"], Some("multi\nline"));
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(
        f.mock.seen.lock().unwrap().last().unwrap().1,
        Method::Msg { to: "all".into(), text: "multi\nline".into() }
    );
    let o = nox(&f.path, None, &["msg", "all", "x"], None);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("token"), "{}", stderr(&o));
}

#[test]
fn inbox_wait_is_answered_later() {
    let f = fixture();
    let t = Instant::now();
    let o = nox(&f.path, Some(&f.token), &["inbox", "--wait"], None);
    assert!(t.elapsed() >= Duration::from_millis(300));
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let s = stdout(&o);
    assert!(s.starts_with("[backend · ") && s.ends_with("] feat/x is ready\n"), "{s}");
}

#[test]
fn claim_conflict_exits_3() {
    let f = fixture();
    let o = nox(&f.path, Some(&f.token), &["claim", "src/parser/", "-m", "parser work"], None);
    assert_eq!(o.status.code(), Some(3));
    let e = stderr(&o);
    assert!(e.contains("src/parser/lexer.rs held by backend (3): lexer rewrite"), "{e}");

    let o = nox(&f.path, Some(&f.token), &["--json", "claim", "x"], None);
    assert_eq!(o.status.code(), Some(3));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["error"]["code"], "conflict");
}

#[test]
fn wait_timeout_exits_4_and_read_prints_text() {
    let f = fixture();
    let o = nox(&f.path, Some(&f.token), &["wait", "backend", "--timeout", "0.1"], None);
    assert_eq!(o.status.code(), Some(4), "{}", stderr(&o));
    let o = nox(&f.path, Some(&f.token), &["read", "backend", "--last-output"], None);
    assert_eq!(stdout(&o), "hello\nworld\n");
}

#[test]
fn status_session_from_stdin() {
    let f = fixture();
    let o = nox(
        &f.path,
        Some(&f.token),
        &["status", "idle", "--session-from-stdin"],
        Some(r#"{"session_id":"sess-42","hook_event_name":"Stop"}"#),
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(
        f.mock.seen.lock().unwrap().last().unwrap().1,
        Method::Status {
            state: nox_ipc::AgentStateWire::Idle,
            message: None,
            session: Some("sess-42".into())
        }
    );
}

#[test]
fn dot_alias_opens_canonical_path() {
    let f = fixture();
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nox"));
    let o = cmd
        .arg(".")
        .current_dir(dir.path())
        .env("NOX_SOCKET", &f.path)
        .env_remove("NOX_TOKEN")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let expected = std::fs::canonicalize(dir.path()).unwrap();
    let seen = f.mock.seen.lock().unwrap();
    let (caller, method) = seen.last().unwrap();
    assert!(caller.is_none());
    assert_eq!(method, &Method::Open { path: expected.to_string_lossy().into_owned(), force_new: false });
}

#[test]
fn usage_and_not_running() {
    let f = fixture();
    let o = nox(&f.path, Some(&f.token), &["status", "busy"], None);
    assert_eq!(o.status.code(), Some(2));
    let o = nox(&f.path, Some(&f.token), &["frobnicate"], None);
    assert_eq!(o.status.code(), Some(2));
    let o = nox(&f.path, Some(&f.token), &["--help"], None);
    assert_eq!(o.status.code(), Some(0));

    let path = f.path.clone();
    f.handle.stop();
    let o = nox(&path, Some("t"), &["ls"], None);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("not running"), "{}", stderr(&o));
}

#[test]
fn mcp_over_stdio() {
    let f = fixture();
    let input = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"send_message","arguments":{"to":"backend","text":"hi"}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"claim_paths","arguments":{"paths":["src/"]}}}),
    ]
    .iter()
    .map(|v| v.to_string() + "\n")
    .collect::<String>();
    let o = nox(&f.path, Some(&f.token), &["mcp"], Some(&input));
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let lines: Vec<serde_json::Value> =
        stdout(&o).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 4, "notification gets no response");
    assert_eq!(lines[0]["result"]["serverInfo"]["name"], "nox");
    assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 11);
    assert_eq!(lines[2]["result"]["isError"], false);
    assert_eq!(lines[3]["result"]["isError"], true);
    assert_eq!(
        f.mock.seen.lock().unwrap().iter().find(|(_, m)| matches!(m, Method::Msg { .. })).unwrap().1,
        Method::Msg { to: "backend".into(), text: "hi".into() }
    );
}
