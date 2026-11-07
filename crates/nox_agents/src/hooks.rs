//! Agent integration helpers: Claude Code hooks (SPEC 7.12 "Install Agent Hooks…") and MCP
//! setup commands (7.13 "MCP server").

use serde_json::{Map, Value, json};

use crate::{Error, Result};

/// Claude Code hook events and the `nox` command each runs.
pub const CLAUDE_HOOKS: &[(&str, &str)] = &[
    ("UserPromptSubmit", "nox status working --session-from-stdin"),
    ("Notification", "nox status needs-input --session-from-stdin"),
    ("Stop", "nox status idle --session-from-stdin"),
];

fn has_nox_hook(groups: &[Value], command: &str) -> bool {
    groups.iter().any(|g| {
        g.get("hooks").and_then(Value::as_array).is_some_and(|hooks| {
            hooks
                .iter()
                .any(|h| h.get("command").and_then(Value::as_str).is_some_and(|c| c.trim() == command))
        })
    })
}

/// Adds nox's hooks to a Claude Code `settings.json` (user `~/.claude/settings.json` or project
/// `.claude/settings.json`), returning the new content (pretty-printed, trailing newline).
/// Idempotent; everything else in the file (other hooks, keys, key order) is preserved. `None`
/// or blank content starts from `{}`. Show the diff to the user before writing it.
pub fn claude_hooks_patch(existing_settings_json: Option<&str>) -> Result<String> {
    let mut root: Value = match existing_settings_json.map(str::trim) {
        None | Some("") => Value::Object(Map::new()),
        Some(s) => serde_json::from_str(s)?,
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| Error::Invalid("settings.json: the top level is not an object".into()))?;
    let hooks = obj.entry("hooks").or_insert_with(|| Value::Object(Map::new()));
    if hooks.is_null() {
        *hooks = Value::Object(Map::new());
    }
    let hooks = hooks
        .as_object_mut()
        .ok_or_else(|| Error::Invalid("settings.json: \"hooks\" is not an object".into()))?;
    for (event, command) in CLAUDE_HOOKS {
        let groups = hooks.entry(*event).or_insert_with(|| Value::Array(Vec::new()));
        if groups.is_null() {
            *groups = Value::Array(Vec::new());
        }
        let groups = groups
            .as_array_mut()
            .ok_or_else(|| Error::Invalid(format!("settings.json: \"hooks.{event}\" is not an array")))?;
        if !has_nox_hook(groups, command) {
            groups.push(json!({
                "matcher": "",
                "hooks": [{ "type": "command", "command": command }]
            }));
        }
    }
    let mut out = serde_json::to_string_pretty(&root)?;
    out.push('\n');
    Ok(out)
}

/// The command that registers `nox mcp` with an agent (Copy MCP Setup Command).
pub fn mcp_setup_command(agent_kind: &str) -> Option<String> {
    Some(
        match agent_kind {
            "claude" => "claude mcp add nox -- nox mcp",
            "codex" => "codex mcp add nox -- nox mcp",
            "gemini" => "gemini mcp add nox nox mcp",
            _ => return None,
        }
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_hooks_from_nothing() {
        let out = claude_hooks_patch(None).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["hooks"]["Stop"],
            json!([{ "matcher": "", "hooks": [{ "type": "command", "command": "nox status idle --session-from-stdin" }] }])
        );
        assert_eq!(
            v["hooks"]["Notification"][0]["hooks"][0]["command"],
            "nox status needs-input --session-from-stdin"
        );
        assert_eq!(
            v["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
            "nox status working --session-from-stdin"
        );
        assert!(out.ends_with("}\n"));
        assert_eq!(claude_hooks_patch(Some("  ")).unwrap(), out);
    }

    #[test]
    fn idempotent_and_preserving() {
        let existing = r#"{
  "model": "opus",
  "permissions": { "allow": ["Bash(ls:*)"] },
  "hooks": {
    "Stop": [{ "matcher": "", "hooks": [{ "type": "command", "command": "say done" }] }],
    "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "check.sh" }] }]
  },
  "env": { "A": "1" }
}"#;
        let once = claude_hooks_patch(Some(existing)).unwrap();
        let twice = claude_hooks_patch(Some(&once)).unwrap();
        assert_eq!(once, twice);
        let v: Value = serde_json::from_str(&once).unwrap();
        assert_eq!(v["model"], "opus");
        assert_eq!(v["env"]["A"], "1");
        assert_eq!(v["permissions"]["allow"][0], "Bash(ls:*)");
        assert_eq!(v["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "check.sh");
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "say done");
        // Key order is kept.
        let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["model", "permissions", "hooks", "env"]);
    }

    #[test]
    fn rejects_non_objects() {
        assert!(claude_hooks_patch(Some("[1]")).is_err());
        assert!(claude_hooks_patch(Some(r#"{"hooks": 3}"#)).is_err());
        assert!(claude_hooks_patch(Some(r#"{"hooks": {"Stop": {}}}"#)).is_err());
        assert!(claude_hooks_patch(Some("{not json")).is_err());
        assert!(claude_hooks_patch(Some(r#"{"hooks": null}"#)).is_ok());
    }

    #[test]
    fn mcp_commands() {
        assert_eq!(mcp_setup_command("claude").as_deref(), Some("claude mcp add nox -- nox mcp"));
        assert_eq!(mcp_setup_command("codex").as_deref(), Some("codex mcp add nox -- nox mcp"));
        assert_eq!(mcp_setup_command("gemini").as_deref(), Some("gemini mcp add nox nox mcp"));
        assert_eq!(mcp_setup_command("aider"), None);
    }
}
