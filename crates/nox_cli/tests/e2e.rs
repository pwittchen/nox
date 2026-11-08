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

/// `nox` without a controlling terminal, with a custom environment (`None` removes a variable).
fn nox_detached(env: &[(&str, Option<&str>)], args: &[&str]) -> Output {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nox"));
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env {
        match v {
            Some(v) => cmd.env(k, v),
            None => cmd.env_remove(k),
        };
    }
    // SAFETY: setsid is async-signal-safe; it detaches the child from our terminal so the
    // notification fallback can't reach the developer's real /dev/tty.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.output().unwrap()
}

#[test]
fn notify_falls_back_to_osc777_without_nox() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("none.sock");
    let missing = missing.to_str().unwrap();
    let o = nox_detached(
        &[("NOX_SOCKET", Some(missing)), ("NOX_TOKEN", None)],
        &["notify", "Build", "all green"],
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(stdout(&o), "\x1b]777;notify;Build;all green\x07");
}

#[test]
fn notify_falls_back_to_osc777_without_token() {
    let f = fixture();
    let o = nox_detached(
        &[("NOX_SOCKET", Some(f.path.to_str().unwrap())), ("NOX_TOKEN", None)],
        &["notify", "Done"],
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(stdout(&o), "\x1b]777;notify;Done;\x07");
    assert!(f.mock.seen.lock().unwrap().is_empty(), "rejected before the handler");
}

#[test]
fn connect_errors_explain_the_situation() {
    let f = fixture();
    let path = f.path.clone();
    f.handle.stop();
    // Inside a nox pane (NOX_SOCKET set) the hint mentions agents.cooperation.
    let o = nox(&path, Some("t"), &["claims"], None);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("cannot reach nox at") && stderr(&o).contains("agents.cooperation"),
        "{}",
        stderr(&o)
    );

    // Outside nox: the default socket under NOX_HOME.
    let home = tempfile::tempdir().unwrap();
    let o = nox_detached(&[("NOX_SOCKET", None), ("NOX_HOME", Some(home.path().to_str().unwrap()))], &["ls"]);
    assert_eq!(o.status.code(), Some(1));
    let err = stderr(&o);
    assert!(err.starts_with("nox: nox is not running (no control socket at "), "{err}");
    assert!(err.contains(&home.path().join("run/nox.sock").display().to_string()), "{err}");

    // Any other connect error is passed through.
    let long = format!("/{}", "x".repeat(200));
    let o = nox_detached(&[("NOX_SOCKET", Some(&long))], &["ls"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).starts_with(&format!("nox: cannot connect to nox at {long}: ")), "{}", stderr(&o));
}

#[test]
fn invalid_timeout_is_a_usage_error() {
    let f = fixture();
    let o = nox(&f.path, Some(&f.token), &["wait", "backend", "--timeout=-1"], None);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert_eq!(stderr(&o), "nox: invalid --timeout -1\n");
    assert!(f.mock.seen.lock().unwrap().is_empty());
}

#[test]
fn json_errors_go_to_stdout_too() {
    let f = fixture();
    let o = nox(&f.path, Some(&f.token), &["--json", "wait", "backend", "--timeout", "1"], None);
    assert_eq!(o.status.code(), Some(4));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["error"]["code"], "timeout");
    assert_eq!(stderr(&o), "nox: pane did not become idle\n");
}

#[test]
fn mcp_reports_ipc_errors_and_reconnects() {
    let f = fixture();
    let input = [
        // Timeout from nox: a tool error; the client reconnects for the next call.
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"wait_for_pane","arguments":{"pane":"backend"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"read_pane","arguments":{"pane":"backend"}}}),
    ]
    .iter()
    .map(|v| v.to_string() + "\n")
    .collect::<String>();
    let o = nox(&f.path, Some(&f.token), &["mcp"], Some(&input));
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let lines: Vec<serde_json::Value> =
        stdout(&o).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines[0]["result"]["isError"], true);
    assert_eq!(lines[0]["result"]["content"][0]["text"], "pane did not become idle (timeout)");
    assert_eq!(lines[1]["result"]["content"][0]["text"], "hello\nworld");
}

#[test]
fn mcp_without_nox_still_answers() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("none.sock");
    let input = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_claims"}})
        .to_string()
        + "\n";
    let o = nox(&missing, Some("t"), &["mcp"], Some(&input));
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let r: serde_json::Value = serde_json::from_str(stdout(&o).trim()).unwrap();
    assert_eq!(r["result"]["isError"], true);
    assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("cannot reach nox"), "{r}");
}

#[test]
fn mcp_fails_when_stdout_is_closed() {
    let f = fixture();
    let mut child = Command::new(env!("CARGO_BIN_EXE_nox"))
        .arg("mcp")
        .env("NOX_SOCKET", &f.path)
        .env("NOX_TOKEN", &f.token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n").unwrap();
    drop(stdin);
    let o = child.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).starts_with("nox mcp: "), "{}", stderr(&o));
}
