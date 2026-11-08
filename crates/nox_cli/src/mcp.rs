//! `nox mcp`: an MCP server over stdio (JSON-RPC 2.0, newline-delimited) exposing the
//! cooperation operations of SPEC §7.13 as tools. It talks to nox through the same socket and
//! token as the CLI, so it acts as the agent pane it was started from.

use std::io::{BufRead, Write};
use std::time::Duration;

use nox_ipc::{AgentStateWire, Client, ErrorCode, IpcError, Method, Placement, ReadMode, WaitState};
use serde_json::{Map, Value, json};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
const KNOWN_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// What the MCP server needs from nox; [`LazyClient`] in production, fakes in tests.
pub trait IpcBackend {
    fn call(&mut self, method: Method, timeout: Option<Duration>) -> Result<Value, IpcError>;
}

/// Connects on first use and reconnects after a broken connection, so `nox mcp` can start
/// (and answer `initialize`) even when nox isn't reachable yet.
#[derive(Default)]
pub struct LazyClient {
    client: Option<Client>,
}

impl IpcBackend for LazyClient {
    fn call(&mut self, method: Method, timeout: Option<Duration>) -> Result<Value, IpcError> {
        if self.client.is_none() {
            let client = Client::connect_from_env()
                .map_err(|e| IpcError::new(ErrorCode::Internal, crate::connect_error_message(&e)))?;
            self.client = Some(client);
        }
        let client = self.client.as_mut().expect("connected above");
        let result = client.call(method, timeout);
        if matches!(&result, Err(e) if matches!(e.code, ErrorCode::Internal | ErrorCode::Timeout)) {
            // The stream may be broken or hold a late answer; start fresh next time.
            self.client = None;
        }
        result
    }
}

/// Serve MCP on stdin / stdout until stdin closes.
pub fn serve(backend: &mut impl IpcBackend) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if let Some(out) = handle_jsonrpc(&line, backend) {
            stdout.write_all(out.as_bytes())?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn rpc_result(id: &Value, result: Value) -> String {
    let mut response = json!({"jsonrpc": "2.0", "id": id});
    response["result"] = result;
    response.to_string()
}

fn rpc_error(id: &Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

/// Handle one JSON-RPC line; returns the response line, or `None` for notifications.
pub fn handle_jsonrpc(line: &str, backend: &mut impl IpcBackend) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(rpc_error(&Value::Null, -32700, &format!("parse error: {e}")));
        }
    };
    if msg.is_array() {
        return Some(rpc_error(&Value::Null, -32600, "batches are not supported"));
    }
    let Some(obj) = msg.as_object() else {
        return Some(rpc_error(&Value::Null, -32600, "invalid request"));
    };
    let id = obj.get("id").cloned();
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        // A response to something we sent (we never send requests) or garbage.
        return id.map(|id| rpc_error(&id, -32600, "invalid request: missing method"));
    };
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        // Notifications (`notifications/initialized`, `notifications/cancelled`, …).
        return None;
    };
    Some(match method {
        "initialize" => {
            let requested = params.get("protocolVersion").and_then(Value::as_str);
            let version = match requested {
                Some(v) if KNOWN_VERSIONS.contains(&v) => v,
                _ => PROTOCOL_VERSION,
            };
            rpc_result(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "nox", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": "Cooperate with other agents running in nox: list panes, \
                        exchange messages, wait for other agents, claim files before editing \
                        them, spawn helpers. Everything stays visible to the user."
                }),
            )
        }
        "ping" => rpc_result(&id, json!({})),
        "tools/list" => rpc_result(&id, json!({"tools": tools()})),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(rpc_error(&id, -32602, "tools/call needs a tool name"));
            };
            let empty = Value::Object(Map::new());
            let args = params.get("arguments").filter(|a| !a.is_null()).unwrap_or(&empty);
            if !args.is_object() {
                return Some(rpc_error(&id, -32602, "arguments must be an object"));
            }
            if !TOOL_NAMES.contains(&name) {
                return Some(rpc_error(&id, -32602, &format!("unknown tool: {name}")));
            }
            rpc_result(&id, call_tool(name, args, backend))
        }
        other => rpc_error(&id, -32601, &format!("method not found: {other}")),
    })
}

const TOOL_NAMES: &[&str] = &[
    "list_panes",
    "read_pane",
    "send_to_pane",
    "send_message",
    "read_inbox",
    "wait_for_pane",
    "claim_paths",
    "release_paths",
    "list_claims",
    "spawn_pane",
    "set_status",
];

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    let mut tool = json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "required": required,
            "additionalProperties": false
        }
    });
    tool["inputSchema"]["properties"] = properties;
    tool
}

/// Tool definitions with JSON Schemas.
pub fn tools() -> Vec<Value> {
    let pane = json!({"type": "string", "description": "Pane id or name"});
    vec![
        tool(
            "list_panes",
            "List workspaces, tabs and panes visible to you: id, name, program, agent state, cwd, git branch.",
            json!({"all": {"type": "boolean", "description": "Include other workspaces (if allowed)"}}),
            &[],
        ),
        tool(
            "read_pane",
            "Read the plain text of another pane: the visible screen by default.",
            json!({
                "pane": pane,
                "mode": {"type": "string", "enum": ["screen", "lines", "scrollback", "last_output"],
                         "description": "What to read (default screen)"},
                "lines": {"type": "integer", "minimum": 1, "description": "Number of lines for mode=lines (default 50)"}
            }),
            &["pane"],
        ),
        tool(
            "send_to_pane",
            "Type text into another agent pane as a bracketed paste; submit=true also presses Return.",
            json!({"pane": pane, "text": {"type": "string"}, "submit": {"type": "boolean"}}),
            &["pane", "text"],
        ),
        tool(
            "send_message",
            "Post a message to a pane's inbox (by name or id) or to \"all\" in the workspace.",
            json!({"to": {"type": "string", "description": "Pane name, id or \"all\""}, "text": {"type": "string"}}),
            &["to", "text"],
        ),
        tool(
            "read_inbox",
            "Read (and mark read) your messages; wait=true blocks until one arrives.",
            json!({
                "wait": {"type": "boolean"},
                "from": {"type": "string", "description": "Only messages from this pane"},
                "peek": {"type": "boolean", "description": "Do not mark messages read"},
                "timeout_s": {"type": "number", "minimum": 0, "description": "With wait: give up after this many seconds"}
            }),
            &[],
        ),
        tool(
            "wait_for_pane",
            "Block until a pane reaches a state (default idle, i.e. its agent finished its turn).",
            json!({
                "pane": pane,
                "state": {"type": "string", "enum": ["idle", "needs-input", "exited"]},
                "timeout_s": {"type": "number", "minimum": 0}
            }),
            &["pane"],
        ),
        tool(
            "claim_paths",
            "Take advisory claims on files or directories before editing them. Fails with the holder if another pane holds an overlapping path, unless wait=true.",
            json!({
                "paths": {"type": "array", "items": {"type": "string"}, "minItems": 1},
                "reason": {"type": "string"},
                "wait": {"type": "boolean"},
                "timeout_s": {"type": "number", "minimum": 0, "description": "With wait: give up after this many seconds"}
            }),
            &["paths"],
        ),
        tool(
            "release_paths",
            "Release your claims; all of them when paths is empty or omitted.",
            json!({"paths": {"type": "array", "items": {"type": "string"}}}),
            &[],
        ),
        tool("list_claims", "List claims in the workspace.", json!({}), &[]),
        tool(
            "spawn_pane",
            "Open a new visible pane in your workspace running a command; returns its pane id.",
            json!({
                "command": {"type": "array", "items": {"type": "string"}, "description": "Command and arguments; empty = a shell"},
                "placement": {"type": "string", "enum": ["tab", "split-right", "split-below"]},
                "name": {"type": "string"},
                "cwd": {"type": "string", "description": "Absolute working directory"}
            }),
            &[],
        ),
        tool(
            "set_status",
            "Report your state (working, needs-input, idle) with an optional message and session id.",
            json!({
                "state": {"type": "string", "enum": ["working", "needs-input", "idle"]},
                "message": {"type": "string"},
                "session": {"type": "string"}
            }),
            &["state"],
        ),
    ]
}

struct Args<'a>(&'a Value);

impl Args<'_> {
    fn str(&self, k: &str) -> Result<Option<String>, String> {
        match self.0.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(format!("`{k}` must be a string")),
        }
    }
    fn req_str(&self, k: &str) -> Result<String, String> {
        self.str(k)?.ok_or_else(|| format!("missing `{k}`"))
    }
    fn bool(&self, k: &str) -> Result<bool, String> {
        match self.0.get(k) {
            None | Some(Value::Null) => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => Err(format!("`{k}` must be a boolean")),
        }
    }
    fn f64(&self, k: &str) -> Result<Option<f64>, String> {
        match self.0.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => match v.as_f64() {
                Some(f) if f.is_finite() && f >= 0.0 => Ok(Some(f)),
                _ => Err(format!("`{k}` must be a non-negative number")),
            },
        }
    }
    fn strings(&self, k: &str) -> Result<Vec<String>, String> {
        match self.0.get(k) {
            None | Some(Value::Null) => Ok(vec![]),
            Some(Value::Array(a)) => a
                .iter()
                .map(|v| {
                    v.as_str().map(str::to_string).ok_or_else(|| format!("`{k}` must be an array of strings"))
                })
                .collect(),
            Some(Value::String(s)) => Ok(vec![s.clone()]),
            Some(_) => Err(format!("`{k}` must be an array of strings")),
        }
    }
}

fn client_timeout(timeout_s: Option<f64>) -> Option<Duration> {
    timeout_s.map(|t| Duration::from_secs_f64(t) + Duration::from_secs(5))
}

/// Map a tool call to an IPC request.
pub fn tool_to_method(name: &str, args: &Value) -> Result<(Method, Option<Duration>), String> {
    let a = Args(args);
    let m = match name {
        "list_panes" => Method::Ls { all: a.bool("all")? },
        "read_pane" => {
            let mode = match a.str("mode")?.as_deref() {
                None | Some("screen") => ReadMode::Screen,
                Some("lines") => {
                    let n = match args.get("lines") {
                        None | Some(Value::Null) => 50,
                        Some(v) => v
                            .as_u64()
                            .filter(|n| *n >= 1)
                            .and_then(|n| u32::try_from(n).ok())
                            .ok_or("`lines` must be a positive integer")?,
                    };
                    ReadMode::Lines(n)
                }
                Some("scrollback") => ReadMode::Scrollback,
                Some("last_output") => ReadMode::LastOutput,
                Some(other) => return Err(format!("unknown mode `{other}`")),
            };
            Method::Read { pane: a.req_str("pane")?, mode }
        }
        "send_to_pane" => {
            Method::Send { pane: a.req_str("pane")?, text: a.req_str("text")?, submit: a.bool("submit")? }
        }
        "send_message" => Method::Msg { to: a.req_str("to")?, text: a.req_str("text")? },
        "read_inbox" => {
            let timeout_s = a.f64("timeout_s")?;
            return Ok((
                Method::Inbox {
                    wait: a.bool("wait")?,
                    from: a.str("from")?,
                    peek: a.bool("peek")?,
                    timeout_s,
                },
                client_timeout(timeout_s),
            ));
        }
        "wait_for_pane" => {
            let state = match a.str("state")?.as_deref() {
                None | Some("idle") => WaitState::Idle,
                Some("needs-input") => WaitState::NeedsInput,
                Some("exited") => WaitState::Exited,
                Some(other) => return Err(format!("unknown state `{other}`")),
            };
            let timeout_s = a.f64("timeout_s")?;
            return Ok((
                Method::Wait { pane: a.req_str("pane")?, state, timeout_s },
                client_timeout(timeout_s),
            ));
        }
        "claim_paths" => {
            let paths = a.strings("paths")?;
            if paths.is_empty() {
                return Err("`paths` must not be empty".into());
            }
            let timeout_s = a.f64("timeout_s")?;
            return Ok((
                Method::Claim { paths, reason: a.str("reason")?, wait: a.bool("wait")?, timeout_s },
                client_timeout(timeout_s),
            ));
        }
        "release_paths" => Method::Release { paths: a.strings("paths")? },
        "list_claims" => Method::Claims,
        "spawn_pane" => {
            let placement = match a.str("placement")?.as_deref() {
                None | Some("tab") => Placement::Tab,
                Some("split-right") => Placement::SplitRight,
                Some("split-below") => Placement::SplitBelow,
                Some(other) => return Err(format!("unknown placement `{other}`")),
            };
            let cwd = a.str("cwd")?;
            if let Some(c) = &cwd
                && !c.starts_with('/')
            {
                return Err("`cwd` must be an absolute path".into());
            }
            Method::Spawn { placement, name: a.str("name")?, cwd, command: a.strings("command")? }
        }
        "set_status" => {
            let state = match a.req_str("state")?.as_str() {
                "working" => AgentStateWire::Working,
                "needs-input" => AgentStateWire::NeedsInput,
                "idle" => AgentStateWire::Idle,
                other => return Err(format!("unknown state `{other}`")),
            };
            Method::Status { state, message: a.str("message")?, session: a.str("session")? }
        }
        other => return Err(format!("unknown tool: {other}")),
    };
    Ok((m, None))
}

fn text_result(text: &str, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

fn call_tool(name: &str, args: &Value, backend: &mut impl IpcBackend) -> Value {
    let (method, timeout) = match tool_to_method(name, args) {
        Ok(m) => m,
        Err(e) => return text_result(&format!("invalid arguments: {e}"), true),
    };
    let is_read = matches!(method, Method::Read { .. });
    match backend.call(method, timeout) {
        Ok(v) => {
            let text = match (is_read, v.get("text").and_then(Value::as_str)) {
                (true, Some(t)) => t.to_string(),
                _ if v.is_null() || v.as_object().is_some_and(serde_json::Map::is_empty) => "ok".into(),
                _ => serde_json::to_string_pretty(&v).unwrap_or_default(),
            };
            let mut r = text_result(&text, false);
            if v.is_object() && !is_read {
                r["structuredContent"] = v;
            }
            r
        }
        Err(e) => {
            let mut text = format!("{} ({})", e.message, e.code.as_str());
            if let Some(data) = &e.data {
                text.push('\n');
                text.push_str(&serde_json::to_string_pretty(data).unwrap_or_default());
            }
            text_result(&text, true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        calls: Vec<(Method, Option<Duration>)>,
    }

    impl IpcBackend for Fake {
        fn call(&mut self, method: Method, timeout: Option<Duration>) -> Result<Value, IpcError> {
            self.calls.push((method.clone(), timeout));
            match method {
                Method::Ls { .. } => Ok(json!({"workspaces": []})),
                Method::Read { .. } => Ok(json!({"text": "screen text"})),
                Method::Claim { .. } => Err(IpcError::new(ErrorCode::Conflict, "held by backend")
                    .with_data(json!({"conflicts": [{"path": "a", "holder": 2, "holder_name": "backend"}]}))),
                Method::Status { .. } => Ok(json!({})),
                _ => Ok(json!({"ok": true})),
            }
        }
    }

    fn rpc(line: &Value, fake: &mut Fake) -> Value {
        serde_json::from_str(&handle_jsonrpc(&line.to_string(), fake).expect("response")).unwrap()
    }

    #[test]
    fn handshake_list_call() {
        let mut fake = Fake::default();
        let init = rpc(
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}),
            &mut fake,
        );
        assert_eq!(init["jsonrpc"], "2.0");
        assert_eq!(init["id"], 1);
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(init["result"]["serverInfo"]["name"], "nox");
        assert!(init["result"]["capabilities"]["tools"].is_object());

        assert_eq!(
            handle_jsonrpc(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, &mut fake),
            None
        );

        let list = rpc(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}), &mut fake);
        let tools = list["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, TOOL_NAMES);
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object");
            assert!(t["description"].as_str().is_some_and(|d| !d.is_empty()));
            for r in t["inputSchema"]["required"].as_array().unwrap() {
                assert!(t["inputSchema"]["properties"].get(r.as_str().unwrap()).is_some());
            }
        }

        let call = rpc(
            &json!({"jsonrpc":"2.0","id":"x","method":"tools/call","params":{"name":"read_pane","arguments":{"pane":"backend","mode":"lines","lines":10}}}),
            &mut fake,
        );
        assert_eq!(call["id"], "x");
        assert_eq!(call["result"]["isError"], false);
        assert_eq!(call["result"]["content"][0]["type"], "text");
        assert_eq!(call["result"]["content"][0]["text"], "screen text");
        assert_eq!(
            fake.calls.last().unwrap().0,
            Method::Read { pane: "backend".into(), mode: ReadMode::Lines(10) }
        );

        let ls = rpc(
            &json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_panes"}}),
            &mut fake,
        );
        assert_eq!(ls["result"]["structuredContent"], json!({"workspaces": []}));

        let st = rpc(
            &json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"set_status","arguments":{"state":"needs-input","session":"s"}}}),
            &mut fake,
        );
        assert_eq!(st["result"]["content"][0]["text"], "ok");
        assert_eq!(
            fake.calls.last().unwrap().0,
            Method::Status { state: AgentStateWire::NeedsInput, message: None, session: Some("s".into()) }
        );
    }

    #[test]
    fn older_version_is_accepted_unknown_is_not() {
        let mut fake = Fake::default();
        let r = rpc(
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}),
            &mut fake,
        );
        assert_eq!(r["result"]["protocolVersion"], "2024-11-05");
        let r = rpc(
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}),
            &mut fake,
        );
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    #[test]
    fn errors() {
        let mut fake = Fake::default();
        let r = rpc(&json!({"jsonrpc":"2.0","id":1,"method":"ping"}), &mut fake);
        assert_eq!(r["result"], json!({}));
        let r = rpc(&json!({"jsonrpc":"2.0","id":2,"method":"resources/list"}), &mut fake);
        assert_eq!(r["error"]["code"], -32601);
        let r: Value = serde_json::from_str(&handle_jsonrpc("{oops", &mut fake).unwrap()).unwrap();
        assert_eq!(r["error"]["code"], -32700);
        assert_eq!(r["id"], Value::Null);
        let r =
            rpc(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"nope"}}), &mut fake);
        assert_eq!(r["error"]["code"], -32602);
        // Bad arguments → tool error result, not a protocol error.
        let r = rpc(
            &json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"read_pane","arguments":{}}}),
            &mut fake,
        );
        assert_eq!(r["result"]["isError"], true);
        // IPC error → isError with the code and details.
        let r = rpc(
            &json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"claim_paths","arguments":{"paths":["a"]}}}),
            &mut fake,
        );
        assert_eq!(r["result"]["isError"], true);
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("conflict") && text.contains("backend"), "{text}");
        assert_eq!(handle_jsonrpc("", &mut fake), None);
    }

    #[test]
    fn malformed_requests() {
        let mut fake = Fake::default();
        let raw = |line: &str, fake: &mut Fake| -> Option<Value> {
            handle_jsonrpc(line, fake).map(|s| serde_json::from_str(&s).unwrap())
        };
        let r = raw("[1, 2]", &mut fake).unwrap();
        assert_eq!((r["error"]["code"].clone(), r["id"].clone()), (json!(-32600), Value::Null));
        assert_eq!(r["error"]["message"], "batches are not supported");
        let r = raw("42", &mut fake).unwrap();
        assert_eq!(r["error"]["message"], "invalid request");
        // A message without a method: an error if it has an id, otherwise ignored.
        let r = raw(r#"{"jsonrpc":"2.0","id":9,"result":{}}"#, &mut fake).unwrap();
        assert_eq!((r["id"].clone(), r["error"]["code"].clone()), (json!(9), json!(-32600)));
        assert_eq!(raw(r#"{"jsonrpc":"2.0","result":{}}"#, &mut fake), None);
        let r = rpc(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}}), &mut fake);
        assert_eq!(r["error"]["message"], "tools/call needs a tool name");
        let r = rpc(
            &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_panes","arguments":[1]}}),
            &mut fake,
        );
        assert_eq!(r["error"]["message"], "arguments must be an object");
        // Null arguments mean none.
        let r = rpc(
            &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_claims","arguments":null}}),
            &mut fake,
        );
        assert_eq!(r["result"]["isError"], false);
        assert!(fake.calls.iter().all(|(m, _)| *m == Method::Claims), "{:?}", fake.calls);
    }

    #[test]
    fn tool_results() {
        let mut fake = Fake::default();
        // Non-empty object results are pretty-printed and returned as structured content.
        let r = call_tool("send_message", &json!({"to":"a","text":"t"}), &mut fake);
        assert_eq!(r["isError"], false);
        assert_eq!(r["content"][0]["text"], "{\n  \"ok\": true\n}");
        assert_eq!(r["structuredContent"], json!({"ok": true}));
        // read_pane returns the text itself, without structured content.
        let r = call_tool("read_pane", &json!({"pane":"b"}), &mut fake);
        assert_eq!(r["content"][0]["text"], "screen text");
        assert!(r.get("structuredContent").is_none());
        let r = call_tool("read_pane", &json!({"pane":1}), &mut fake);
        assert_eq!(r["content"][0]["text"], "invalid arguments: `pane` must be a string");
        assert_eq!(r["isError"], true);
    }

    #[test]
    fn argument_validation() {
        let err = |name: &str, args: Value| tool_to_method(name, &args).unwrap_err();
        assert_eq!(err("list_panes", json!({"all": "yes"})), "`all` must be a boolean");
        assert_eq!(err("send_message", json!({"to": "a"})), "missing `text`");
        assert_eq!(err("read_pane", json!({"pane": "b", "mode": "pixels"})), "unknown mode `pixels`");
        assert_eq!(
            err("read_pane", json!({"pane": "b", "mode": "lines", "lines": 0})),
            "`lines` must be a positive integer"
        );
        assert_eq!(
            err("read_pane", json!({"pane": "b", "mode": "lines", "lines": 5_000_000_000_u64})),
            "`lines` must be a positive integer"
        );
        assert_eq!(err("wait_for_pane", json!({"pane": "b", "state": "asleep"})), "unknown state `asleep`");
        assert_eq!(
            err("wait_for_pane", json!({"pane": "b", "timeout_s": "1"})),
            "`timeout_s` must be a non-negative number"
        );
        assert_eq!(err("spawn_pane", json!({"placement": "float"})), "unknown placement `float`");
        assert_eq!(err("claim_paths", json!({"paths": 3})), "`paths` must be an array of strings");
        assert_eq!(err("claim_paths", json!({"paths": ["a", 1]})), "`paths` must be an array of strings");
        assert_eq!(err("teleport", json!({})), "unknown tool: teleport");
    }

    #[test]
    fn argument_defaults_and_shortcuts() {
        let m = |name: &str, args: Value| tool_to_method(name, &args).unwrap();
        let read = |mode: Value| m("read_pane", json!({"pane": "b", "mode": mode, "lines": null})).0;
        assert_eq!(read(json!("lines")), Method::Read { pane: "b".into(), mode: ReadMode::Lines(50) });
        assert_eq!(read(json!("scrollback")), Method::Read { pane: "b".into(), mode: ReadMode::Scrollback });
        assert_eq!(read(json!("last_output")), Method::Read { pane: "b".into(), mode: ReadMode::LastOutput });
        assert_eq!(read(Value::Null), Method::Read { pane: "b".into(), mode: ReadMode::Screen });
        // A single path may be given as a string.
        let (c, t) =
            m("claim_paths", json!({"paths": "src/", "reason": "x", "wait": true, "timeout_s": 2.5}));
        assert_eq!(
            c,
            Method::Claim {
                paths: vec!["src/".into()],
                reason: Some("x".into()),
                wait: true,
                timeout_s: Some(2.5)
            }
        );
        assert_eq!(t, Some(Duration::from_secs_f64(7.5)));
        let (w, t) = m("wait_for_pane", json!({"pane": "b", "state": "needs-input"}));
        assert_eq!(w, Method::Wait { pane: "b".into(), state: WaitState::NeedsInput, timeout_s: None });
        assert_eq!(t, None);
        assert_eq!(
            m("spawn_pane", json!({"placement": "split-right", "name": "h"})).0,
            Method::Spawn {
                placement: Placement::SplitRight,
                name: Some("h".into()),
                cwd: None,
                command: vec![]
            }
        );
        assert_eq!(
            m("spawn_pane", json!({})).0,
            Method::Spawn { placement: Placement::Tab, name: None, cwd: None, command: vec![] }
        );
        for (state, want) in [("working", AgentStateWire::Working), ("idle", AgentStateWire::Idle)] {
            assert_eq!(
                m("set_status", json!({"state": state, "message": "m"})).0,
                Method::Status { state: want, message: Some("m".into()), session: None }
            );
        }
    }

    #[test]
    fn tool_mapping() {
        let m = |name: &str, args: Value| tool_to_method(name, &args).unwrap();
        assert_eq!(
            m("send_to_pane", json!({"pane":"r","text":"t","submit":true})).0,
            Method::Send { pane: "r".into(), text: "t".into(), submit: true }
        );
        assert_eq!(
            m("send_message", json!({"to":"all","text":"t"})).0,
            Method::Msg { to: "all".into(), text: "t".into() }
        );
        let (w, t) = m("wait_for_pane", json!({"pane":"b","state":"exited","timeout_s":1}));
        assert_eq!(w, Method::Wait { pane: "b".into(), state: WaitState::Exited, timeout_s: Some(1.0) });
        assert_eq!(t, Some(Duration::from_secs(6)));
        assert_eq!(
            m("read_inbox", json!({"wait":true,"from":"b"})).0,
            Method::Inbox { wait: true, from: Some("b".into()), peek: false, timeout_s: None }
        );
        assert_eq!(m("release_paths", json!({})).0, Method::Release { paths: vec![] });
        assert_eq!(m("list_claims", json!({})).0, Method::Claims);
        assert_eq!(
            m("spawn_pane", json!({"command":["claude"],"placement":"split-below","cwd":"/x"})).0,
            Method::Spawn {
                placement: Placement::SplitBelow,
                name: None,
                cwd: Some("/x".into()),
                command: vec!["claude".into()]
            }
        );
        assert!(tool_to_method("spawn_pane", &json!({"cwd":"rel"})).is_err());
        assert!(tool_to_method("claim_paths", &json!({"paths":[]})).is_err());
        assert!(tool_to_method("set_status", &json!({"state":"busy"})).is_err());
        assert!(tool_to_method("wait_for_pane", &json!({"pane":"b","timeout_s":-1})).is_err());
    }
}
