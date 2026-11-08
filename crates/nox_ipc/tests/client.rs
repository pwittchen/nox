//! `Client` against hand-written servers: malformed, partial and unrelated response lines.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::Duration;

use nox_ipc::{Client, ErrorCode, Method};
use serde_json::json;

/// A server that accepts one connection, reads one request line and writes `reply`. With
/// `close`, it then closes the connection; otherwise it keeps it open until the client hangs up
/// (on macOS `setsockopt` fails with `EINVAL` once the peer is gone, see `Client::call`).
fn fake(reply: &'static [u8], close: bool) -> (tempfile::TempDir, PathBuf, JoinHandle<String>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fake.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let thread = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request = String::new();
        reader.read_line(&mut request).unwrap();
        let mut stream = stream;
        if close {
            // Let the client get into its blocking read first.
            std::thread::sleep(Duration::from_millis(100));
        }
        stream.write_all(reply).unwrap();
        if !close {
            let mut rest = Vec::new();
            let _ = reader.read_to_end(&mut rest);
        }
        request
    });
    (dir, path, thread)
}

fn ls() -> Method {
    Method::Ls { all: false }
}

#[test]
fn request_line_carries_version_id_and_token() {
    let (_dir, path, server) = fake(b"{\"v\":1,\"id\":1,\"ok\":{}}\n", false);
    let mut client = Client::connect(&path, Some("tok".into())).unwrap();
    assert_eq!(client.call(ls(), None).unwrap(), json!({}));
    drop(client);
    let request: serde_json::Value = serde_json::from_str(&server.join().unwrap()).unwrap();
    assert_eq!(request, json!({"v":1,"id":1,"token":"tok","method":"ls","params":{"all":false}}));
}

#[test]
fn skips_garbage_and_other_ids() {
    let (_dir, path, _server) =
        fake(b"garbage\n{\"v\":1,\"id\":99,\"ok\":\"other\"}\n{\"v\":1,\"id\":1,\"ok\":\"mine\"}\n", false);
    let mut client = Client::connect(&path, None).unwrap();
    assert!(!client.has_token());
    assert_eq!(client.call(ls(), Some(Duration::from_secs(5))).unwrap(), json!("mine"));
}

#[test]
fn id_zero_error_fails_the_call() {
    let (_dir, path, _server) =
        fake(b"{\"v\":1,\"id\":0,\"error\":{\"code\":\"usage\",\"message\":\"invalid JSON\"}}\n", false);
    let mut client = Client::connect(&path, None).unwrap();
    let err = client.call(ls(), None).unwrap_err();
    assert_eq!((err.code, err.message.as_str()), (ErrorCode::Usage, "invalid JSON"));
}

#[test]
fn id_zero_ok_is_skipped() {
    let (_dir, path, _server) = fake(b"{\"v\":1,\"id\":0,\"ok\":1}\n{\"v\":1,\"id\":1,\"ok\":2}\n", false);
    let mut client = Client::connect(&path, None).unwrap();
    assert_eq!(client.call(ls(), None).unwrap(), json!(2));
}

#[test]
fn closed_or_truncated_connection() {
    let (_dir, path, _server) = fake(b"", true);
    let mut client = Client::connect(&path, None).unwrap();
    let err = client.call(ls(), None).unwrap_err();
    assert_eq!((err.code, err.message.as_str()), (ErrorCode::Internal, "nox closed the connection"));

    let (_dir, path, _server) = fake(b"{\"v\":1,\"id\":1", true);
    let mut client = Client::connect(&path, None).unwrap();
    let err = client.call(ls(), None).unwrap_err();
    assert_eq!((err.code, err.message.as_str()), (ErrorCode::Internal, "nox closed the connection"));
}

#[test]
fn answer_after_other_id_is_read_when_the_server_closed() {
    // Both lines arrive, then the server closes; re-arming the read timeout for the second
    // (already buffered) line then fails with `EINVAL` on macOS, which must not fail the call.
    let (_dir, path, _server) =
        fake(b"{\"v\":1,\"id\":99,\"ok\":\"other\"}\n{\"v\":1,\"id\":1,\"ok\":\"mine\"}\n", true);
    let mut client = Client::connect(&path, None).unwrap();
    assert_eq!(client.call(ls(), Some(Duration::from_secs(5))).unwrap(), json!("mine"));
}

#[test]
fn unexpected_result_shape() {
    let (_dir, path, _server) = fake(b"{\"v\":1,\"id\":1,\"ok\":\"not an ls result\"}\n", false);
    let mut client = Client::connect(&path, None).unwrap();
    let err = client.call_typed::<nox_ipc::LsResult>(ls(), None).unwrap_err();
    assert_eq!(err.code, ErrorCode::Internal);
    assert!(err.message.starts_with("unexpected result:"), "{}", err.message);
}

#[test]
fn zero_timeout_fails_without_waiting() {
    let (_dir, path, _server) = fake(b"", true);
    let mut client = Client::connect(&path, None).unwrap();
    let err = client.call(ls(), Some(Duration::ZERO)).unwrap_err();
    assert_eq!(err.code, ErrorCode::Timeout);
}

#[test]
fn send_to_closed_peer_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fake.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let mut client = Client::connect(&path, None).unwrap();
    // Accept and close at once, then drop the listener too.
    drop(listener.accept().unwrap());
    drop(listener);
    // The first write may still be buffered; at the latest the read sees EOF.
    let err = client.call(ls(), Some(Duration::from_secs(5))).unwrap_err();
    assert_eq!(err.code, ErrorCode::Internal);
    let err = client.call(ls(), Some(Duration::from_secs(5))).unwrap_err();
    assert_eq!(err.code, ErrorCode::Internal);
    assert!(err.message.starts_with("send request:"), "{}", err.message);
}

#[test]
fn connect_to_missing_socket_fails() {
    let dir = tempfile::tempdir().unwrap();
    let err = Client::connect(&dir.path().join("none.sock"), None).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}
