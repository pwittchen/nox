//! Control socket server (SPEC §7.13).
//!
//! One background thread accepts connections; each connection gets a thread that reads request
//! lines, authenticates the token and hands the request to the [`Handler`] together with a
//! one-shot [`Responder`]. The handler may answer right away or later from any thread
//! (subscriptions such as `wait`, `inbox --wait`, `claim --wait` — never polling).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;

use serde_json::Value;

use crate::protocol::{ErrorCode, IpcError, Method, PROTOCOL_VERSION, Request, Response};

/// Longest accepted request line (bytes); longer lines are answered with a `usage` error.
pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// The pane a token belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Caller {
    pub pane_id: u64,
    pub tab_id: u64,
    pub workspace_id: u64,
}

/// Per-pane tokens, shared between the app (which issues them when spawning panes) and the
/// server (which resolves them). Cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct Tokens {
    inner: Arc<RwLock<HashMap<String, Caller>>>,
}

impl Tokens {
    pub fn new() -> Self {
        Self::default()
    }

    /// Issue a new random token (32 hex chars) for a pane. Put it in the pane's `NOX_TOKEN`.
    pub fn issue(&self, caller: Caller) -> String {
        let token = uuid::Uuid::new_v4().simple().to_string();
        self.write().insert(token.clone(), caller);
        token
    }

    /// The caller a token belongs to.
    pub fn resolve(&self, token: &str) -> Option<Caller> {
        self.read().get(token).copied()
    }

    /// Revoke every token of a pane (pane closed).
    pub fn revoke_pane(&self, pane_id: u64) {
        self.write().retain(|_, c| c.pane_id != pane_id);
    }

    /// Revoke one token.
    pub fn revoke(&self, token: &str) {
        self.write().remove(token);
    }

    /// The pane moved (its tab moved to another tab / workspace); its token stays valid.
    pub fn update(&self, pane_id: u64, tab_id: u64, workspace_id: u64) {
        for c in self.write().values_mut().filter(|c| c.pane_id == pane_id) {
            c.tab_id = tab_id;
            c.workspace_id = workspace_id;
        }
    }

    pub fn len(&self) -> usize {
        self.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, Caller>> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, Caller>> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }
}

/// Application side of the socket.
///
/// `handle` is called on the connection's thread; it must not block for long. Long operations
/// keep `reply` and answer later from another thread. `caller` is `None` only for methods that
/// are allowed without a token (`ls`, `open`); for `ls` the server strips tabs from the result.
pub trait Handler: Send + Sync {
    fn handle(&self, caller: Option<Caller>, method: Method, reply: Responder);
}

type SharedWriter = Arc<Mutex<UnixStream>>;

/// One-shot reply to a request. `Send`, so it can be moved to another thread. If dropped without
/// answering, an `internal` error is sent so the client never hangs.
pub struct Responder {
    writer: SharedWriter,
    id: u64,
    workspaces_only: bool,
    answered: bool,
}

impl std::fmt::Debug for Responder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Responder").field("id", &self.id).finish_non_exhaustive()
    }
}

impl Responder {
    /// Request id this responder answers.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Answer with a result value.
    pub fn ok(mut self, mut value: Value) {
        if self.workspaces_only {
            strip_tabs(&mut value);
        }
        self.send(Response::ok(self.id, value));
    }

    /// Answer with a serializable result.
    pub fn ok_typed<T: serde::Serialize>(self, value: &T) {
        match serde_json::to_value(value) {
            Ok(v) => self.ok(v),
            Err(e) => self.err(ErrorCode::Internal, format!("serialize result: {e}")),
        }
    }

    /// Answer with an error.
    pub fn err(self, code: ErrorCode, message: impl Into<String>) {
        self.error(IpcError::new(code, message));
    }

    /// Answer with a full error (incl. `data`).
    pub fn error(mut self, error: IpcError) {
        self.send(Response::err(self.id, error));
    }

    /// Answer with `Ok(v)` / `Err(e)`.
    pub fn result(self, result: Result<Value, IpcError>) {
        match result {
            Ok(v) => self.ok(v),
            Err(e) => self.error(e),
        }
    }

    fn send(&mut self, response: Response) {
        self.answered = true;
        write_response(&self.writer, &response);
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        if !self.answered {
            self.answered = true;
            write_response(
                &self.writer,
                &Response::err(self.id, IpcError::new(ErrorCode::Internal, "request dropped")),
            );
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn write_response(writer: &SharedWriter, response: &Response) {
    let mut line = match serde_json::to_string(response) {
        Ok(l) => l,
        Err(e) => {
            log::error!("nox_ipc: cannot serialize response: {e}");
            return;
        }
    };
    line.push('\n');
    let mut stream = lock(writer);
    if let Err(e) = stream.write_all(line.as_bytes()).and_then(|_| stream.flush()) {
        log::debug!("nox_ipc: write response {}: {e}", response.id);
    }
}

/// Unauthenticated `ls`: workspaces only.
fn strip_tabs(value: &mut Value) {
    if let Some(wss) = value.get_mut("workspaces").and_then(Value::as_array_mut) {
        for ws in wss {
            if let Some(obj) = ws.as_object_mut() {
                obj.insert("tabs".into(), Value::Array(vec![]));
            }
        }
    }
}

/// The control socket server.
pub struct Server;

impl Server {
    /// Start serving on `socket_path` with a fresh [`Tokens`] table (get it with
    /// [`ServerHandle::tokens`]).
    pub fn start(socket_path: &Path, handler: Arc<dyn Handler>) -> io::Result<ServerHandle> {
        Self::start_with_tokens(socket_path, Tokens::new(), handler)
    }

    /// Start serving on `socket_path`, authenticating against `tokens`.
    ///
    /// Creates the parent directory (mode `0700`), removes a stale socket (fails with
    /// `AddrInUse` if another server answers on it), binds and sets the socket to `0600`.
    pub fn start_with_tokens(
        socket_path: &Path,
        tokens: Tokens,
        handler: Arc<dyn Handler>,
    ) -> io::Result<ServerHandle> {
        if let Some(dir) = socket_path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
            let mode = std::fs::metadata(dir)?.permissions().mode() & 0o777;
            if mode != 0o700 {
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        if std::fs::symlink_metadata(socket_path).is_ok() {
            if UnixStream::connect(socket_path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("another server is listening on {}", socket_path.display()),
                ));
            }
            std::fs::remove_file(socket_path)?;
        }
        let listener = UnixListener::bind(socket_path)?;
        listener.set_nonblocking(true)?;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;

        let shared = Arc::new(Shared {
            tokens: tokens.clone(),
            handler,
            stopping: AtomicBool::new(false),
            connections: Mutex::new(HashMap::new()),
            next_conn: AtomicU64::new(1),
        });
        let accept_shared = shared.clone();
        let accept = std::thread::Builder::new()
            .name("nox-ipc-accept".into())
            .spawn(move || accept_loop(listener, accept_shared))?;

        Ok(ServerHandle { path: socket_path.to_path_buf(), tokens, shared, accept: Some(accept) })
    }
}

struct Shared {
    tokens: Tokens,
    handler: Arc<dyn Handler>,
    stopping: AtomicBool,
    connections: Mutex<HashMap<u64, UnixStream>>,
    next_conn: AtomicU64,
}

/// How often the accept loop re-checks the stop flag even when nothing wakes it (e.g. the
/// socket file was deleted, so the wake-up connect in `shutdown` cannot reach it).
const ACCEPT_POLL_MS: i32 = 200;

fn accept_loop(listener: UnixListener, shared: Arc<Shared>) {
    let fd = listener.as_raw_fd();
    loop {
        if shared.stopping.load(Ordering::SeqCst) {
            break;
        }
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd, the listener outlives the call.
        let n = unsafe { libc::poll(&mut pfd, 1, ACCEPT_POLL_MS) };
        if n <= 0 {
            continue;
        }
        if shared.stopping.load(Ordering::SeqCst) {
            break;
        }
        let stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {
                continue;
            }
            Err(e) => {
                log::warn!("nox_ipc: accept: {e}");
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
        };
        // Accepted sockets inherit O_NONBLOCK from the listener on BSD/macOS.
        if let Err(e) = stream.set_nonblocking(false) {
            log::warn!("nox_ipc: set_nonblocking: {e}");
            continue;
        }
        let conn_id = shared.next_conn.fetch_add(1, Ordering::Relaxed);
        if let Ok(clone) = stream.try_clone() {
            lock(&shared.connections).insert(conn_id, clone);
        }
        let conn_shared = shared.clone();
        let spawned = std::thread::Builder::new().name(format!("nox-ipc-conn-{conn_id}")).spawn(move || {
            if let Err(e) = serve_connection(stream, &conn_shared) {
                log::debug!("nox_ipc: connection {conn_id}: {e}");
            }
            lock(&conn_shared.connections).remove(&conn_id);
        });
        if let Err(e) = spawned {
            log::error!("nox_ipc: spawn connection thread: {e}");
            lock(&shared.connections).remove(&conn_id);
        }
    }
}

fn serve_connection(stream: UnixStream, shared: &Shared) -> io::Result<()> {
    let writer: SharedWriter = Arc::new(Mutex::new(stream.try_clone()?));
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = (&mut reader).take(MAX_LINE_BYTES as u64 + 1).read_until(b'\n', &mut buf)?;
        if n == 0 || shared.stopping.load(Ordering::SeqCst) {
            return Ok(());
        }
        if buf.len() > MAX_LINE_BYTES && buf.last() != Some(&b'\n') {
            write_response(
                &writer,
                &Response::err(0, IpcError::new(ErrorCode::Usage, "request line too long")),
            );
            return Ok(());
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        dispatch(line, &writer, shared);
    }
}

fn dispatch(line: &str, writer: &SharedWriter, shared: &Shared) {
    let raw: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            let err = IpcError::new(ErrorCode::Usage, format!("invalid JSON: {e}"));
            write_response(writer, &Response::err(0, err));
            return;
        }
    };
    let id = raw.get("id").and_then(Value::as_u64).unwrap_or(0);
    let reply_err = |code, msg: String| {
        write_response(writer, &Response::err(id, IpcError::new(code, msg)));
    };
    match raw.get("v").and_then(Value::as_u64) {
        Some(v) if v == u64::from(PROTOCOL_VERSION) => {}
        Some(v) => {
            return reply_err(
                ErrorCode::Usage,
                format!("unsupported protocol version {v} (this nox speaks {PROTOCOL_VERSION})"),
            );
        }
        None => return reply_err(ErrorCode::Usage, "missing protocol version \"v\"".into()),
    }
    let request: Request = match serde_json::from_value(raw) {
        Ok(r) => r,
        Err(e) => return reply_err(ErrorCode::Usage, format!("invalid request: {e}")),
    };
    let caller = request.token.as_deref().and_then(|t| shared.tokens.resolve(t));
    if caller.is_none() && !request.method.allowed_without_token() {
        let msg = if request.token.is_some() { "invalid token" } else { "missing token" };
        return reply_err(
            ErrorCode::Unauthorized,
            format!("{msg}: `{}` needs NOX_TOKEN (only open and ls work without)", request.method.name()),
        );
    }
    let responder = Responder {
        writer: writer.clone(),
        id,
        workspaces_only: caller.is_none() && matches!(request.method, Method::Ls { .. }),
        answered: false,
    };
    let handler = shared.handler.clone();
    // A panicking handler must not take the connection thread down silently: the responder
    // is dropped during unwinding and answers `internal`.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        handler.handle(caller, request.method, responder);
    }));
    if result.is_err() {
        log::error!("nox_ipc: handler panicked on request {id}");
    }
}

/// Running server. Dropping it (or [`ServerHandle::stop`]) stops accepting, closes open
/// connections and removes the socket file.
pub struct ServerHandle {
    path: PathBuf,
    tokens: Tokens,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for ServerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerHandle").field("path", &self.path).finish_non_exhaustive()
    }
}

impl ServerHandle {
    pub fn socket_path(&self) -> &Path {
        &self.path
    }

    /// The token table this server authenticates against.
    pub fn tokens(&self) -> &Tokens {
        &self.tokens
    }

    /// Stop the server and remove the socket file.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let Some(accept) = self.accept.take() else {
            return;
        };
        self.shared.stopping.store(true, Ordering::SeqCst);
        // Wake the accept loop.
        let _ = UnixStream::connect(&self.path);
        let _ = accept.join();
        for (_, conn) in lock(&self.shared.connections).drain() {
            let _ = conn.shutdown(std::net::Shutdown::Both);
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_issue_resolve_update_revoke() {
        let t = Tokens::new();
        let c = Caller { pane_id: 1, tab_id: 2, workspace_id: 3 };
        let tok = t.issue(c);
        assert_eq!(tok.len(), 32);
        assert!(tok.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_ne!(tok, t.issue(Caller { pane_id: 9, ..c }));
        assert_eq!(t.resolve(&tok), Some(c));
        t.update(1, 5, 6);
        assert_eq!(t.resolve(&tok), Some(Caller { pane_id: 1, tab_id: 5, workspace_id: 6 }));
        t.revoke_pane(1);
        assert_eq!(t.resolve(&tok), None);
        assert_eq!(t.len(), 1);
        assert_eq!(t.resolve("nope"), None);
    }

    #[test]
    fn strip_tabs_keeps_workspaces() {
        let mut v = serde_json::json!({"workspaces":[{"id":1,"name":"a","tabs":[{"id":2}]}]});
        strip_tabs(&mut v);
        assert_eq!(v, serde_json::json!({"workspaces":[{"id":1,"name":"a","tabs":[]}]}));
    }
}
