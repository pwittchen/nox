//! Wire protocol of the control socket (SPEC §7.13): newline-delimited JSON, versioned `"v": 1`.
//!
//! A request line looks like
//! `{"v":1,"id":7,"token":"…","method":"read","params":{"pane":"backend","mode":"screen"}}`
//! and its response like `{"v":1,"id":7,"ok":{"text":"…"}}` or
//! `{"v":1,"id":7,"error":{"code":"not_found","message":"no pane named backend"}}`.
//! Responses on one connection may arrive out of order; clients match them by `id`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Current protocol version.
pub const PROTOCOL_VERSION: u32 = 1;

/// One request from a client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub v: u32,
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(flatten)]
    pub method: Method,
}

/// Every operation of the control socket, serialized as `{"method": "...", "params": {...}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Method {
    /// Workspaces, tabs and panes. Without a token only workspaces (no tabs) are returned.
    Ls {
        #[serde(default)]
        all: bool,
    },
    /// Name the calling pane.
    Name { name: String },
    /// Report the calling agent's state and session id (SPEC §7.12).
    Status {
        state: AgentStateWire,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
    },
    /// Plain text of another pane.
    Read {
        /// Pane id or name.
        pane: String,
        #[serde(default)]
        mode: ReadMode,
    },
    /// Type text into another (agent) pane as a bracketed paste.
    Send {
        pane: String,
        text: String,
        #[serde(default)]
        submit: bool,
    },
    /// Post a message to a pane (name / id) or to `all` in the workspace.
    Msg { to: String, text: String },
    /// Read (and mark read unless `peek`) the calling pane's messages.
    Inbox {
        /// Block until at least one message arrives (a subscription, answered later).
        #[serde(default)]
        wait: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        #[serde(default)]
        peek: bool,
        /// With `wait`: give up with `timeout` after this many seconds (none = forever).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_s: Option<f64>,
    },
    /// Block until the pane reaches `state`.
    Wait {
        pane: String,
        #[serde(default)]
        state: WaitState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_s: Option<f64>,
    },
    /// Take advisory claims on paths.
    Claim {
        paths: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Wait until conflicting claims are released instead of failing with `conflict`.
        #[serde(default)]
        wait: bool,
        /// With `wait`: give up with `timeout` after this many seconds (none = forever).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_s: Option<f64>,
    },
    /// Release own claims; an empty list releases all of them.
    Release {
        #[serde(default)]
        paths: Vec<String>,
    },
    /// List claims in the workspace.
    Claims,
    /// Open a new pane in the caller's workspace.
    Spawn {
        #[serde(default)]
        placement: Placement,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        /// Empty = the user's shell.
        #[serde(default)]
        command: Vec<String>,
    },
    /// Attention sound + notification.
    Notify {
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
    },
    /// Open or switch to the workspace for a folder (absolute path).
    Open {
        path: String,
        #[serde(default)]
        force_new: bool,
    },
}

impl Method {
    /// The wire name of the method (`"ls"`, `"read"`, …).
    pub fn name(&self) -> &'static str {
        match self {
            Method::Ls { .. } => "ls",
            Method::Name { .. } => "name",
            Method::Status { .. } => "status",
            Method::Read { .. } => "read",
            Method::Send { .. } => "send",
            Method::Msg { .. } => "msg",
            Method::Inbox { .. } => "inbox",
            Method::Wait { .. } => "wait",
            Method::Claim { .. } => "claim",
            Method::Release { .. } => "release",
            Method::Claims => "claims",
            Method::Spawn { .. } => "spawn",
            Method::Notify { .. } => "notify",
            Method::Open { .. } => "open",
        }
    }

    /// Methods allowed without a valid token.
    pub fn allowed_without_token(&self) -> bool {
        matches!(self, Method::Ls { .. } | Method::Open { .. })
    }
}

/// Agent state as reported by `nox status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentStateWire {
    Working,
    NeedsInput,
    Idle,
}

impl AgentStateWire {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentStateWire::Working => "working",
            AgentStateWire::NeedsInput => "needs-input",
            AgentStateWire::Idle => "idle",
        }
    }
}

/// What part of a pane `read` returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadMode {
    /// The visible screen.
    #[default]
    Screen,
    /// The last N lines (screen + scrollback).
    Lines(u32),
    /// The whole scrollback plus the screen.
    Scrollback,
    /// The output of the last finished command (OSC 133 C..D).
    LastOutput,
}

/// State `wait` blocks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WaitState {
    #[default]
    Idle,
    NeedsInput,
    Exited,
}

impl WaitState {
    pub fn as_str(self) -> &'static str {
        match self {
            WaitState::Idle => "idle",
            WaitState::NeedsInput => "needs-input",
            WaitState::Exited => "exited",
        }
    }
}

/// Where `spawn` opens the new pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Placement {
    #[default]
    Tab,
    SplitRight,
    SplitBelow,
}

/// Error categories; each maps to a CLI exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Malformed request / bad arguments.
    Usage,
    /// Unknown pane, workspace, …
    NotFound,
    /// Missing or invalid token.
    Unauthorized,
    /// Out of the caller's scope, or a send into a plain shell.
    Forbidden,
    /// Claim conflict.
    Conflict,
    Timeout,
    /// Loop guard (SPEC §7.13 Safety).
    RateLimited,
    Internal,
}

impl ErrorCode {
    /// CLI exit code: `0` ok, `1` error, `2` usage, `3` conflict, `4` timeout.
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::Usage => 2,
            ErrorCode::Conflict => 3,
            ErrorCode::Timeout => 4,
            ErrorCode::NotFound
            | ErrorCode::Unauthorized
            | ErrorCode::Forbidden
            | ErrorCode::RateLimited
            | ErrorCode::Internal => 1,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Usage => "usage",
            ErrorCode::NotFound => "not_found",
            ErrorCode::Unauthorized => "unauthorized",
            ErrorCode::Forbidden => "forbidden",
            ErrorCode::Conflict => "conflict",
            ErrorCode::Timeout => "timeout",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::Internal => "internal",
        }
    }
}

/// Error payload of a response. `data` carries structured details, e.g. for `conflict` a
/// [`ClaimConflictsData`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct IpcError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl IpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

/// `{"ok": …}` or `{"error": {…}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseResult {
    Ok(Value),
    Error(IpcError),
}

/// One response line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub v: u32,
    pub id: u64,
    #[serde(flatten)]
    pub result: ResponseResult,
}

impl Response {
    pub fn ok(id: u64, value: Value) -> Self {
        Self { v: PROTOCOL_VERSION, id, result: ResponseResult::Ok(value) }
    }

    pub fn err(id: u64, error: IpcError) -> Self {
        Self { v: PROTOCOL_VERSION, id, result: ResponseResult::Error(error) }
    }

    pub fn into_result(self) -> Result<Value, IpcError> {
        match self.result {
            ResponseResult::Ok(v) => Ok(v),
            ResponseResult::Error(e) => Err(e),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Typed results
// ---------------------------------------------------------------------------------------------

/// Result of `ls`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LsResult {
    pub workspaces: Vec<WsInfo>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WsInfo {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub root: Option<String>,
    #[serde(default)]
    pub active: bool,
    /// Empty for unauthenticated callers.
    #[serde(default)]
    pub tabs: Vec<TabInfo>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TabInfo {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub panes: Vec<PaneInfo>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PaneInfo {
    pub id: u64,
    pub name: String,
    /// Foreground program (e.g. `claude`, `zsh`).
    #[serde(default)]
    pub program: Option<String>,
    /// `working`, `needs-input`, `idle`, `exited`; none for non-agent panes.
    #[serde(default)]
    pub agent_state: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub focused: bool,
}

/// Result of `read`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ReadResult {
    pub text: String,
}

/// Result of `msg`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MsgResult {
    /// Ids of the queued messages (one per recipient).
    #[serde(default)]
    pub ids: Vec<u64>,
    /// Names of the recipient panes.
    #[serde(default)]
    pub recipients: Vec<String>,
}

/// Result of `inbox`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct InboxResult {
    pub messages: Vec<MessageWire>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MessageWire {
    pub id: u64,
    /// Sender pane id.
    pub from: u64,
    pub from_name: String,
    pub text: String,
    /// Unix time in milliseconds.
    pub ts_ms: u64,
}

/// Result of `wait`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaitResult {
    pub state: WaitState,
}

/// Result of a successful `claim`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ClaimResult {
    /// The claimed paths, as stored (resolved against the workspace root).
    pub claimed: Vec<String>,
}

/// `data` of a `conflict` error from `claim`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ClaimConflictsData {
    pub conflicts: Vec<ClaimConflictWire>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ClaimConflictWire {
    /// The held path that overlaps the requested one.
    pub path: String,
    /// Holder pane id.
    pub holder: u64,
    pub holder_name: String,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Result of `release`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ReleaseResult {
    pub released: Vec<String>,
}

/// Result of `claims`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ClaimsResult {
    pub claims: Vec<ClaimWire>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ClaimWire {
    pub path: String,
    pub holder: u64,
    pub holder_name: String,
    #[serde(default)]
    pub reason: Option<String>,
    /// Unix time in milliseconds of the last (re)claim.
    #[serde(default)]
    pub since_ms: u64,
}

/// Result of `spawn`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SpawnResult {
    pub pane_id: u64,
}

/// Result of `open`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OpenResult {
    pub workspace_id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn all_methods() -> Vec<Method> {
        vec![
            Method::Ls { all: true },
            Method::Name { name: "backend".into() },
            Method::Status {
                state: AgentStateWire::NeedsInput,
                message: Some("permission".into()),
                session: Some("abc-123".into()),
            },
            Method::Status { state: AgentStateWire::Idle, message: None, session: None },
            Method::Read { pane: "backend".into(), mode: ReadMode::Screen },
            Method::Read { pane: "3".into(), mode: ReadMode::Lines(40) },
            Method::Read { pane: "3".into(), mode: ReadMode::Scrollback },
            Method::Read { pane: "3".into(), mode: ReadMode::LastOutput },
            Method::Send { pane: "reviewer".into(), text: "hi\n".into(), submit: true },
            Method::Msg { to: "all".into(), text: "ready".into() },
            Method::Inbox { wait: true, from: Some("backend".into()), peek: false, timeout_s: None },
            Method::Inbox { wait: false, from: None, peek: true, timeout_s: Some(2.5) },
            Method::Wait { pane: "backend".into(), state: WaitState::Exited, timeout_s: Some(1.5) },
            Method::Wait { pane: "backend".into(), state: WaitState::NeedsInput, timeout_s: None },
            Method::Claim {
                paths: vec!["src/parser/".into(), "a.rs".into()],
                reason: Some("refactor".into()),
                wait: true,
                timeout_s: None,
            },
            Method::Release { paths: vec![] },
            Method::Release { paths: vec!["a.rs".into()] },
            Method::Claims,
            Method::Spawn {
                placement: Placement::SplitBelow,
                name: Some("helper".into()),
                cwd: Some("/tmp".into()),
                command: vec!["claude".into(), "--fast".into()],
            },
            Method::Spawn { placement: Placement::Tab, name: None, cwd: None, command: vec![] },
            Method::Notify { title: "Done".into(), body: Some("all green".into()) },
            Method::Open { path: "/Users/me/x".into(), force_new: true },
        ]
    }

    #[test]
    fn every_method_round_trips() {
        for (i, method) in all_methods().into_iter().enumerate() {
            let req = Request { v: 1, id: i as u64, token: Some("t".into()), method };
            let line = serde_json::to_string(&req).unwrap();
            assert!(!line.contains('\n'));
            let back: Request = serde_json::from_str(&line).unwrap();
            assert_eq!(back, req, "{line}");
        }
    }

    #[test]
    fn wire_shape() {
        let req = Request {
            v: 1,
            id: 7,
            token: None,
            method: Method::Read { pane: "backend".into(), mode: ReadMode::Lines(5) },
        };
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            json!({"v":1,"id":7,"method":"read","params":{"pane":"backend","mode":{"lines":5}}})
        );
        let claims: Request = serde_json::from_value(json!({"v":1,"id":1,"method":"claims"})).unwrap();
        assert_eq!(claims.method, Method::Claims);
        let ls: Request = serde_json::from_value(json!({"v":1,"id":1,"method":"ls","params":{}})).unwrap();
        assert_eq!(ls.method, Method::Ls { all: false });
        let st: Request = serde_json::from_value(
            json!({"v":1,"id":1,"token":"x","method":"status","params":{"state":"needs-input"}}),
        )
        .unwrap();
        assert!(matches!(st.method, Method::Status { state: AgentStateWire::NeedsInput, .. }));
        let sp: Request = serde_json::from_value(
            json!({"v":1,"id":1,"method":"spawn","params":{"placement":"split-right"}}),
        )
        .unwrap();
        assert!(matches!(sp.method, Method::Spawn { placement: Placement::SplitRight, .. }));
    }

    #[test]
    fn response_round_trips() {
        let ok = Response::ok(3, json!({"text": "hello"}));
        let s = serde_json::to_string(&ok).unwrap();
        assert_eq!(s, r#"{"v":1,"id":3,"ok":{"text":"hello"}}"#);
        assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), ok);

        let err = Response::err(
            4,
            IpcError::new(ErrorCode::Conflict, "held").with_data(
                serde_json::to_value(ClaimConflictsData {
                    conflicts: vec![ClaimConflictWire {
                        path: "src/a.rs".into(),
                        holder: 2,
                        holder_name: "backend".into(),
                        reason: Some("x".into()),
                    }],
                })
                .unwrap(),
            ),
        );
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains(r#""error":{"code":"conflict","message":"held""#), "{s}");
        assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), err);

        for code in [
            ErrorCode::Usage,
            ErrorCode::NotFound,
            ErrorCode::Unauthorized,
            ErrorCode::Forbidden,
            ErrorCode::Conflict,
            ErrorCode::Timeout,
            ErrorCode::RateLimited,
            ErrorCode::Internal,
        ] {
            let v = serde_json::to_value(code).unwrap();
            assert_eq!(v, json!(code.as_str()));
            assert_eq!(serde_json::from_value::<ErrorCode>(v).unwrap(), code);
        }
    }

    #[test]
    fn exit_codes() {
        assert_eq!(ErrorCode::Usage.exit_code(), 2);
        assert_eq!(ErrorCode::Conflict.exit_code(), 3);
        assert_eq!(ErrorCode::Timeout.exit_code(), 4);
        assert_eq!(ErrorCode::NotFound.exit_code(), 1);
        assert_eq!(ErrorCode::Internal.exit_code(), 1);
    }

    #[test]
    fn results_round_trip() {
        let ls = LsResult {
            workspaces: vec![WsInfo {
                id: 1,
                name: "nox".into(),
                root: Some("/p/nox".into()),
                active: true,
                tabs: vec![TabInfo {
                    id: 2,
                    title: "agent".into(),
                    panes: vec![PaneInfo {
                        id: 3,
                        name: "claude".into(),
                        program: Some("claude".into()),
                        agent_state: Some("working".into()),
                        cwd: Some("/p/nox".into()),
                        branch: Some("main".into()),
                        focused: true,
                    }],
                }],
            }],
        };
        let v = serde_json::to_value(&ls).unwrap();
        assert_eq!(serde_json::from_value::<LsResult>(v).unwrap(), ls);
        let inbox = InboxResult {
            messages: vec![MessageWire {
                id: 1,
                from: 3,
                from_name: "backend".into(),
                text: "hi".into(),
                ts_ms: 10,
            }],
        };
        let v = serde_json::to_value(&inbox).unwrap();
        assert_eq!(serde_json::from_value::<InboxResult>(v).unwrap(), inbox);
        let w = WaitResult { state: WaitState::NeedsInput };
        assert_eq!(serde_json::to_value(&w).unwrap(), json!({"state":"needs-input"}));
    }
}
