use std::sync::mpsc;
use std::time::Duration;

use super::*;

#[test]
fn defaults_match_spec() {
    let c = Config::default();
    assert_eq!(c.ui.theme, "nox");
    assert_eq!(c.ui.mode, Mode::System);
    assert_eq!(c.ui.dim_inactive_panes, 0.15);
    assert_eq!(c.tabs.position, TabsPosition::Top);
    assert_eq!(c.tabs.sidebar_width, 220);
    assert_eq!(c.agents.claim_ttl, 1800);
    assert_eq!(c.agents.defs["claude"].resume_last.as_deref(), Some("claude --continue"));
    assert_eq!(c.agents.defs["codex"].resume.as_deref(), Some("codex resume {session}"));
    assert_eq!(c.font.family, "JetBrains Mono");
    assert_eq!(c.font.size, 14.0);
    assert_eq!(c.terminal.scrollback, 10_000);
    assert_eq!(c.terminal.padding, [8, 6]);
    assert_eq!(c.terminal.word_separators, " ()[]{}',\"`│");
    assert_eq!(c.terminal.osc52, Osc52::Write);
    assert_eq!(c.terminal.grapheme_width, GraphemeWidth::Unicode);
    assert_eq!(c.shell.args, vec!["-l".to_string()]);
    assert_eq!(c.app.update_channel, "stable");
    assert_eq!(c.window.width, 1000);
}

#[test]
fn empty_file_is_defaults() {
    let (c, n) = load_from_str("").unwrap();
    assert_eq!(c, Config::default());
    assert_eq!(n, [] as [load::Notice; 0]);
}

#[test]
fn default_file_parses_to_defaults_without_notices() {
    let (c, n) = load_from_str(DEFAULT_CONFIG_TOML).unwrap();
    assert!(n.is_empty(), "{n:?}");
    assert_eq!(c, Config::default());
}

#[test]
fn partial_file() {
    let src = r#"
[tabs]
position = "left"
sidebar_width = 300

[terminal]
osc52 = "read-write"
grapheme_width = "legacy"
padding = [2, 4]
attention_sound_volume = 1

[agents.claude]
command = "claude --model opus"

[agents.my-agent]
command = "my-agent --fast"
match = ["my-agent", "my-agent-dev"]
icon = "~/.nox/icons/my-agent.svg"

[shell.env]
RUST_LOG = "debug"

[icons]
"jbang" = "java"

[keys]
split_vertical = "cmd-d"
command_palette = ""
"alt-left" = { send = "\u001bb" }
"#;
    let (c, n) = load_from_str(src).unwrap();
    assert!(n.is_empty(), "{n:?}");
    assert_eq!(c.tabs.position, TabsPosition::Left);
    assert_eq!(c.tabs.sidebar_width, 300);
    assert!(c.tabs.show_index);
    assert_eq!(c.terminal.osc52, Osc52::ReadWrite);
    assert_eq!(c.terminal.grapheme_width, GraphemeWidth::Legacy);
    assert_eq!(c.terminal.padding, [2, 4]);
    assert_eq!(c.terminal.attention_sound_volume, 1.0);
    let claude = &c.agents.defs["claude"];
    assert_eq!(claude.command, "claude --model opus");
    // Unspecified fields of a built-in agent keep their defaults.
    assert_eq!(claude.resume.as_deref(), Some("claude --resume {session}"));
    let mine = &c.agents.defs["my-agent"];
    assert_eq!(mine.match_, vec!["my-agent", "my-agent-dev"]);
    assert_eq!(mine.icon.as_deref(), Some("~/.nox/icons/my-agent.svg"));
    assert!(c.agents.defs.contains_key("codex"));
    assert_eq!(c.shell.env["RUST_LOG"], "debug");
    assert_eq!(c.icons["jbang"], "java");
    assert_eq!(c.keys["split_vertical"], KeyBinding::Shortcut("cmd-d".into()));
    assert!(c.keys["command_palette"].is_unbind());
    assert_eq!(c.keys["alt-left"], KeyBinding::Send { send: "\u{1b}b".into() });
}

#[test]
fn invalid_values_fall_back_with_notices() {
    let src = r#"
[ui]
mode = "sepia"
pane_header = "yes"
dim_inactive_panes = 2.0

[terminal]
scrollback = 200000
attention_sound_volume = 1.5
cursor_style = "bar"
padding = [1]

[tabs]
sidebar_width = -5
bogus = 1

[agents.broken]
resume = "x"
"#;
    let (c, n) = load_from_str(src).unwrap();
    let keys: Vec<&str> = n.iter().map(|n| n.key.as_str()).collect();
    for k in [
        "ui.mode",
        "ui.pane_header",
        "ui.dim_inactive_panes",
        "terminal.scrollback",
        "terminal.attention_sound_volume",
        "terminal.padding",
        "tabs.sidebar_width",
        "tabs.bogus",
        "agents.broken.command",
    ] {
        assert!(keys.contains(&k), "missing notice for {k}: {keys:?}");
    }
    let d = Config::default();
    assert_eq!(c.ui.mode, d.ui.mode);
    assert_eq!(c.ui.pane_header, d.ui.pane_header);
    assert_eq!(c.ui.dim_inactive_panes, d.ui.dim_inactive_panes);
    assert_eq!(c.terminal.scrollback, d.terminal.scrollback);
    assert_eq!(c.terminal.attention_sound_volume, d.terminal.attention_sound_volume);
    assert_eq!(c.terminal.padding, d.terminal.padding);
    assert_eq!(c.tabs.sidebar_width, d.tabs.sidebar_width);
    // Valid neighbours still apply.
    assert_eq!(c.terminal.cursor_style, CursorStyle::Bar);
    assert!(!c.agents.defs.contains_key("broken"));
}

#[test]
fn section_of_wrong_type_is_a_notice() {
    let (c, n) = load_from_str("ui = 3\nfoo = 1\n").unwrap();
    assert_eq!(c.ui, UiConfig::default());
    assert!(n.iter().any(|n| n.key == "ui"));
    assert!(n.iter().any(|n| n.key == "foo"));
}

#[test]
fn syntax_error_reports_line() {
    let src = "[ui]\ntheme = \"nox\"\nmode = \n";
    let err = load_from_str(src).unwrap_err();
    assert_eq!(err.line, Some(3));
    assert_ne!(err.message, "");
    assert!(err.to_string().starts_with("line 3:"));
}

#[test]
fn ensure_default_file_writes_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub/config.toml");
    assert!(ensure_default_file(&path).unwrap());
    std::fs::write(&path, "# mine\n").unwrap();
    assert!(!ensure_default_file(&path).unwrap());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mine\n");
}

#[test]
fn set_and_remove_preserve_comments_and_unknown_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let src = "# top comment\n\n[tabs]\nposition = \"top\"   # where tabs go\nmystery = 42\n\n[ui]\ntheme = \"nox\"\n";
    std::fs::write(&path, src).unwrap();

    set_value(&path, "tabs.position", "left").unwrap();
    let out = std::fs::read_to_string(&path).unwrap();
    assert!(out.contains("# top comment"));
    assert!(out.contains("position = \"left\"   # where tabs go"), "{out}");
    assert!(out.contains("mystery = 42"));

    // New key in an existing table, new key in a new table, and a nested agent table.
    set_value(&path, "ui.mode", "dark").unwrap();
    set_value(&path, "terminal.scrollback", 5000i64).unwrap();
    set_value(&path, "agents.my-agent.command", "my-agent").unwrap();
    let (c, _) = load_from_path(&path).unwrap();
    assert_eq!(c.tabs.position, TabsPosition::Left);
    assert_eq!(c.ui.mode, Mode::Dark);
    assert_eq!(c.terminal.scrollback, 5000);
    assert_eq!(c.agents.defs["my-agent"].command, "my-agent");

    remove_value(&path, "tabs.position").unwrap();
    remove_value(&path, "does.not.exist").unwrap();
    let out = std::fs::read_to_string(&path).unwrap();
    assert!(!out.contains("position"));
    assert!(out.contains("mystery = 42"));
    assert!(out.starts_with("# top comment\n"));
    let (c, _) = load_from_path(&path).unwrap();
    assert_eq!(c.tabs.position, TabsPosition::Top);

    // Writing to the default file keeps its comments.
    let path2 = dir.path().join("c2.toml");
    ensure_default_file(&path2).unwrap();
    ConfigWriter::new(&path2).set("ui.theme", "nord").unwrap();
    let out = std::fs::read_to_string(&path2).unwrap();
    assert!(out.contains("theme = \"nord\"                # palette"), "{out}");
    assert_eq!(out.lines().count(), DEFAULT_CONFIG_TOML.lines().count());
}

#[test]
fn set_value_at_handles_dotted_names() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    set_value_at(&path, &["icons", "my.tool"], "java").unwrap();
    let (c, _) = load_from_path(&path).unwrap();
    assert_eq!(c.icons["my.tool"], "java");
}

#[test]
fn refuses_to_write_unparsable_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let broken = "[ui\ntheme = \n";
    std::fs::write(&path, broken).unwrap();
    let err = set_value(&path, "ui.theme", "nord").unwrap_err();
    assert!(matches!(err, WriteError::Unparsable(_)));
    assert!(remove_value(&path, "ui.theme").is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
}

#[test]
fn writing_into_scalar_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[ui]\ntheme = \"nox\"\n").unwrap();
    assert!(matches!(set_value(&path, "ui.theme.x", 1i64), Err(WriteError::NotATable(_))));
    assert!(matches!(set_value(&path, "ui..x", 1i64), Err(WriteError::InvalidKey(_))));
}

#[test]
fn self_write_is_recognized() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    set_value(&path, "ui.theme", "nord").unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(is_self_write(&path, &bytes));
    assert!(!is_self_write(&path, b"[ui]\ntheme = \"other\"\n"));

    let mut filter = ChangeFilter::new(None);
    assert!(!filter.should_deliver(&path, &bytes), "own write must be ignored");
    let external = b"[ui]\ntheme = \"gruvbox\"\n";
    assert!(filter.should_deliver(&path, external));
    assert!(!filter.should_deliver(&path, external), "duplicate event");
}

#[test]
fn watcher_reports_external_changes_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "").unwrap();
    let (tx, rx) = mpsc::channel();
    let _handle = watch(&path, move |r| {
        let _ = tx.send(r.map(|(c, _)| c.ui.theme));
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(300));

    set_value(&path, "ui.theme", "nord").unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(800)).is_err(), "self-write should be ignored");

    std::fs::write(&path, "[ui]\ntheme = \"gruvbox\"\n").unwrap();
    let got = rx.recv_timeout(Duration::from_secs(5)).expect("external change");
    assert_eq!(got.unwrap(), "gruvbox");

    std::fs::write(&path, "[ui\n").unwrap();
    let got = rx.recv_timeout(Duration::from_secs(5)).expect("broken change");
    assert_eq!(got.unwrap_err().line, Some(1));
}

#[test]
fn schema_and_values() {
    let schema = settings_schema();
    assert!(schema.iter().any(|m| m.key == "tabs.position"));
    for m in &schema {
        assert!(default_value(m.key).is_some(), "schema key {} has no value", m.key);
    }
    // Every Section has at least one row.
    for s in Section::ALL {
        assert!(schema.iter().any(|m| m.section == *s), "{s:?}");
    }
    // Ranges in the schema match the defaults.
    let d = Config::default();
    for m in &schema {
        let v = get_value(&d, m.key).unwrap();
        match &m.kind {
            SettingKind::Int { min, max, .. } => {
                let i = v.as_integer().unwrap();
                assert!(*min <= i && i <= *max, "{}", m.key);
            }
            SettingKind::Float { min, max, .. } => {
                let f = v.as_float().or(v.as_integer().map(|i| i as f64)).unwrap();
                assert!(*min <= f && f <= *max, "{}", m.key);
            }
            SettingKind::Enum(names) => assert!(names.contains(&v.as_str().unwrap()), "{}", m.key),
            _ => {}
        }
    }

    assert_eq!(default_value("tabs.position"), Some(toml::Value::String("top".into())));
    assert_eq!(default_value("agents.claude.command"), Some(toml::Value::String("claude".into())));
    assert_eq!(default_value("nope.nothing"), None);
    let (c, _) = load_from_str("[tabs]\nposition = \"left\"\n").unwrap();
    assert!(differs_from_default(&c, "tabs.position"));
    assert!(!differs_from_default(&c, "tabs.title"));
    assert_eq!(get_value(&c, "terminal.padding"), Some(toml::Value::Array(vec![8.into(), 6.into()])));
}

#[test]
fn paths_follow_nox_home() {
    // SAFETY: tests in this crate don't read NOX_HOME concurrently elsewhere.
    unsafe { std::env::set_var("NOX_HOME", "/tmp/nox-test-home") };
    assert_eq!(nox_dir(), std::path::PathBuf::from("/tmp/nox-test-home"));
    assert_eq!(config_path(), std::path::PathBuf::from("/tmp/nox-test-home/config.toml"));
    assert_eq!(themes_dir(), std::path::PathBuf::from("/tmp/nox-test-home/themes"));
    assert_eq!(icons_dir(), std::path::PathBuf::from("/tmp/nox-test-home/icons"));
    assert_eq!(run_dir(), std::path::PathBuf::from("/tmp/nox-test-home/run"));
    unsafe { std::env::remove_var("NOX_HOME") };
}
