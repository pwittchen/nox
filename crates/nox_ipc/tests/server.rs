use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nox_ipc::{
    Caller, Client, ErrorCode, Handler, LsResult, Method, Responder, Response, Server, TabInfo, WsInfo,
};
use serde_json::json;

struct Mock;

impl Handler for Mock {
    fn handle(&self, caller: Option<Caller>, method: Method, reply: Responder) {
        match method {
            Method::Ls { .. } => reply.ok_typed(&LsResult {
                workspaces: vec![WsInfo {
                    id: 1,
                    name: "nox".into(),
                    root: None,
                    active: true,
                    tabs: vec![TabInfo { id: 2, title: "t".into(), panes: vec![] }],
                }],
            }),
            Method::Open { .. } => reply.ok(json!({"workspace_id": 1})),
            Method::Name { name } => {
                let c = caller.expect("authenticated");
                reply.ok(json!({"pane": c.pane_id, "name": name}));
            }
            Method::Inbox { .. } => {
                // Deferred: answered from another thread later.
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(100));
                    reply.ok(json!({"messages": []}));
                });
            }
            Method::Wait { timeout_s, .. } => {
                let wait = Duration::from_secs_f64(timeout_s.unwrap_or(0.0).max(0.0));
                std::thread::spawn(move || {
                    std::thread::sleep(wait);
                    reply.ok(json!({"state": "idle"}));
                });
            }
            Method::Claims => drop(reply), // dropped responder → internal error
            _ => reply.err(ErrorCode::NotFound, "unsupported in mock"),
        }
    }
}

fn start() -> (tempfile::TempDir, nox_ipc::ServerHandle, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run").join("nox.sock");
    let handle = Server::start(&path, Arc::new(Mock)).unwrap();
    (dir, handle, path)
}

#[test]
fn socket_and_dir_permissions() {
    let (dir, handle, path) = start();
    let sock_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(sock_mode, 0o600);
    let dir_mode = std::fs::metadata(dir.path().join("run")).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700);
    handle.stop();
    assert!(!path.exists(), "socket removed on stop");
}

#[test]
fn stale_socket_is_replaced_and_live_one_refused() {
    let (_dir, handle, path) = start();
    let err = Server::start(&path, Arc::new(Mock)).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
    drop(handle);
    // Leave a stale socket file behind (bound, never listened on by a server).
    let stale = std::os::unix::net::UnixListener::bind(&path).unwrap();
    drop(stale);
    assert!(path.exists());
    let handle = Server::start(&path, Arc::new(Mock)).unwrap();
    assert!(path.exists());
    drop(handle);
    assert!(!path.exists());
}

#[test]
fn authenticated_call_works() {
    let (_dir, handle, path) = start();
    let caller = Caller { pane_id: 5, tab_id: 6, workspace_id: 1 };
    let token = handle.tokens().issue(caller);
    let mut client = Client::connect(&path, Some(token.clone())).unwrap();
    let v = client.call(Method::Name { name: "backend".into() }, None).unwrap();
    assert_eq!(v, json!({"pane": 5, "name": "backend"}));
    // Authenticated ls keeps tabs.
    let ls: LsResult = client.call_typed(Method::Ls { all: false }, None).unwrap();
    assert_eq!(ls.workspaces[0].tabs.len(), 1);

    // Revoked token → unauthorized.
    handle.tokens().revoke_pane(5);
    let err = client.call(Method::Name { name: "x".into() }, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::Unauthorized);
}

#[test]
fn without_token_only_open_and_ls() {
    let (_dir, _handle, path) = start();
    let mut client = Client::connect(&path, None).unwrap();
    let ls: LsResult = client.call_typed(Method::Ls { all: true }, None).unwrap();
    assert_eq!(ls.workspaces.len(), 1);
    assert!(ls.workspaces[0].tabs.is_empty(), "tabs stripped without a token");
    let open = client.call(Method::Open { path: "/tmp".into(), force_new: false }, None).unwrap();
    assert_eq!(open, json!({"workspace_id": 1}));
    for method in [
        Method::Name { name: "x".into() },
        Method::Claims,
        Method::Msg { to: "all".into(), text: "hi".into() },
    ] {
        let err = client.call(method, None).unwrap_err();
        assert_eq!(err.code, ErrorCode::Unauthorized);
    }
    let mut bad = Client::connect(&path, Some("deadbeef".into())).unwrap();
    let err = bad.call(Method::Claims, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::Unauthorized);
    assert!(err.message.contains("invalid token"));
}

#[test]
fn deferred_responder_answers_later() {
    let (_dir, handle, path) = start();
    let token = handle.tokens().issue(Caller { pane_id: 1, tab_id: 1, workspace_id: 1 });
    let mut client = Client::connect(&path, Some(token)).unwrap();
    let t = Instant::now();
    let v =
        client.call(Method::Inbox { wait: true, from: None, peek: false, timeout_s: None }, None).unwrap();
    assert!(t.elapsed() >= Duration::from_millis(100));
    assert_eq!(v, json!({"messages": []}));
}

#[test]
fn dropped_responder_answers_internal() {
    let (_dir, handle, path) = start();
    let token = handle.tokens().issue(Caller { pane_id: 1, tab_id: 1, workspace_id: 1 });
    let mut client = Client::connect(&path, Some(token)).unwrap();
    let err = client.call(Method::Claims, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::Internal);
}

#[test]
fn client_timeout() {
    let (_dir, handle, path) = start();
    let token = handle.tokens().issue(Caller { pane_id: 1, tab_id: 1, workspace_id: 1 });
    let mut client = Client::connect(&path, Some(token)).unwrap();
    let wait =
        |s: f64| Method::Wait { pane: "x".into(), state: nox_ipc::WaitState::Idle, timeout_s: Some(s) };
    let err = client.call(wait(0.5), Some(Duration::from_millis(50))).unwrap_err();
    assert_eq!(err.code, ErrorCode::Timeout);
    // The late answer to the timed-out request is skipped; the next call still matches.
    let v = client.call(wait(0.6), Some(Duration::from_secs(5))).unwrap();
    assert_eq!(v, json!({"state": "idle"}));
}

#[test]
fn concurrent_requests_answered_out_of_order() {
    let (_dir, handle, path) = start();
    let token = handle.tokens().issue(Caller { pane_id: 1, tab_id: 1, workspace_id: 1 });
    let mut stream = UnixStream::connect(&path).unwrap();
    let slow = json!({"v":1,"id":1,"token":token,"method":"wait",
        "params":{"pane":"x","state":"idle","timeout_s":0.3}});
    let fast = json!({"v":1,"id":2,"token":token,"method":"name","params":{"name":"n"}});
    writeln!(stream, "{slow}").unwrap();
    writeln!(stream, "{fast}").unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut ids = Vec::new();
    for _ in 0..2 {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let r: Response = serde_json::from_str(&line).unwrap();
        assert!(r.into_result().is_ok());
        ids.push(serde_json::from_str::<serde_json::Value>(&line).unwrap()["id"].as_u64().unwrap());
    }
    assert_eq!(ids, vec![2, 1]);
}

#[test]
fn malformed_and_wrong_version() {
    let (_dir, _handle, path) = start();
    let mut stream = UnixStream::connect(&path).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut next = |line: &str| {
        writeln!(stream, "{line}").unwrap();
        let mut out = String::new();
        reader.read_line(&mut out).unwrap();
        serde_json::from_str::<Response>(&out).unwrap()
    };
    let r = next("not json");
    assert_eq!(r.into_result().unwrap_err().code, ErrorCode::Usage);
    let r = next(r#"{"v":2,"id":9,"method":"ls"}"#);
    assert_eq!(r.id, 9);
    assert_eq!(r.into_result().unwrap_err().code, ErrorCode::Usage);
    let r = next(r#"{"v":1,"id":10,"method":"bogus"}"#);
    assert_eq!(r.id, 10);
    assert_eq!(r.into_result().unwrap_err().code, ErrorCode::Usage);
}

#[test]
fn stop_returns_even_if_socket_file_was_deleted() {
    let (_dir, handle, path) = start();
    std::fs::remove_file(&path).unwrap();
    let t = Instant::now();
    handle.stop();
    assert!(t.elapsed() < Duration::from_secs(2));
}
