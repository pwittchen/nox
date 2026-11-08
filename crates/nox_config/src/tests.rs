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

    // Several keys in one write; the rest of the table stays.
    set_value(&path, "keys.new_tab", "cmd-y").unwrap();
    set_value(&path, "keys.close_tab", "").unwrap();
    set_value(&path, "keys.split_right", "cmd-e").unwrap();
    remove_values_at(&path, &[&["keys", "new_tab"], &["keys", "close_tab"], &["keys", "missing"]]).unwrap();
    let (c, _) = load_from_path(&path).unwrap();
    assert_eq!(c.keys.len(), 1);
    assert!(c.keys.contains_key("split_right"));
    assert!(std::fs::read_to_string(&path).unwrap().contains("mystery = 42"));

    // Comments above a removed key stay: on the next key, or before the next table.
    let path3 = dir.path().join("c3.toml");
    std::fs::write(
        &path3,
        "[keys]\n# keep me\nnew_tab = \"cmd-y\"\nclose_tab = \"\"\n\"alt-left\" = { send = \"x\" }\n\n[window]\nwidth = 900\n",
    )
    .unwrap();
    remove_values_at(&path3, &[&["keys", "new_tab"], &["keys", "close_tab"]]).unwrap();
    let out = std::fs::read_to_string(&path3).unwrap();
    assert_eq!(out, "[keys]\n# keep me\n\"alt-left\" = { send = \"x\" }\n\n[window]\nwidth = 900\n", "{out}");
    remove_value(&path3, "keys.alt-left").unwrap();
    let out = std::fs::read_to_string(&path3).unwrap();
    assert!(out.contains("# keep me"), "{out}");
    assert!(load_from_path(&path3).unwrap().0.keys.is_empty());

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

fn notice_for<'a>(notices: &'a [Notice], key: &str) -> &'a str {
    notices
        .iter()
        .find(|n| n.key == key)
        .map_or_else(|| panic!("no notice for {key}: {notices:?}"), |n| n.message.as_str())
}

#[test]
fn wrong_types_name_what_was_found() {
    let src = r#"
[ui]
theme = 3
font_family = 1.5
window_padding = "6"

[font]
size = true
weight = 2024-01-01

[terminal]
padding = 4
word_separators = []

[shell]
args = "-l"
env = "x"

[agents]
keep_awake = { a = 1 }
"#;
    let (c, n) = load_from_str(src).unwrap();
    assert_eq!(notice_for(&n, "ui.theme"), "expected a string, found an integer; using the default");
    assert_eq!(notice_for(&n, "ui.font_family"), "expected a string, found a float; using the default");
    assert_eq!(notice_for(&n, "ui.window_padding"), "expected an integer, found a string; using the default");
    assert_eq!(notice_for(&n, "font.size"), "expected a number, found a boolean; using the default");
    assert_eq!(notice_for(&n, "font.weight"), "expected a string, found a datetime; using the default");
    assert_eq!(
        notice_for(&n, "terminal.padding"),
        "expected an array of two integers, found an integer; using the default"
    );
    assert_eq!(
        notice_for(&n, "terminal.word_separators"),
        "expected a string, found an array; using the default"
    );
    assert_eq!(
        notice_for(&n, "shell.args"),
        "expected an array of strings, found a string; using the default"
    );
    assert_eq!(notice_for(&n, "shell.env"), "expected a table, found a string; using the default");
    assert_eq!(
        notice_for(&n, "agents.keep_awake"),
        "expected true or false, found a table; using the default"
    );
    let d = Config::default();
    assert_eq!((c.ui.theme, c.font.size, c.shell.args), (d.ui.theme, d.font.size, d.shell.args));
}

#[test]
fn malformed_lists_pairs_and_maps() {
    let src = r#"
[terminal]
padding = [1, "2"]
scroll_multiplier = 2

[shell]
args = ["-l", 1]
env = { GOOD = "1", BAD = 2 }

[workspaces]
new_tab_cwd = 7

[icons]
vim = "neovim"
"my.tool" = 1
"#;
    let (c, n) = load_from_str(src).unwrap();
    assert_eq!(notice_for(&n, "terminal.padding"), "expected two integers; using the default");
    assert_eq!(notice_for(&n, "shell.args"), "expected an array of strings; using the default");
    assert_eq!(notice_for(&n, "shell.env.BAD"), "expected a string, found an integer; ignored");
    assert_eq!(
        notice_for(&n, "workspaces.new_tab_cwd"),
        "expected a string, found an integer; using the default"
    );
    // The [icons] notice has no leading dot.
    assert_eq!(notice_for(&n, "icons.my.tool"), "expected a string, found an integer; ignored");
    let d = Config::default();
    assert_eq!(c.terminal.padding, d.terminal.padding);
    assert_eq!(c.shell.args, d.shell.args);
    assert_eq!(c.shell.env.get("GOOD").map(String::as_str), Some("1"));
    assert!(!c.shell.env.contains_key("BAD"));
    assert_eq!(c.icons.get("vim").map(String::as_str), Some("neovim"));
    // Integers are accepted for float settings.
    assert_eq!(c.terminal.scroll_multiplier, 2.0);
}

#[test]
fn icons_of_wrong_type() {
    let (c, n) = load_from_str("icons = 5\n").unwrap();
    assert_eq!(notice_for(&n, "icons"), "expected a table, found an integer; using the default");
    assert_eq!(c.icons, Config::default().icons);
}

#[test]
fn padding_values_out_of_range() {
    let (c, n) = load_from_str("[terminal]\npadding = [1, 100000]\n").unwrap();
    assert!(notice_for(&n, "terminal.padding").contains("out of range"), "{n:?}");
    assert_eq!(c.terminal.padding, Config::default().terminal.padding);
    let (c, n) = load_from_str("[terminal]\npadding = [3, 4]\n").unwrap();
    assert!(n.is_empty(), "{n:?}");
    assert_eq!(c.terminal.padding, [3, 4]);
}

#[test]
fn non_finite_floats_are_rejected() {
    let (c, n) = load_from_str("[font]\nsize = nan\nline_height = inf\n").unwrap();
    assert!(notice_for(&n, "font.size").contains("out of range"));
    assert!(notice_for(&n, "font.line_height").contains("out of range"));
    assert_eq!(c.font.size, Config::default().font.size);
}

#[test]
fn agent_definitions() {
    let src = r#"
[agents.claude]
command = ""
icon = "robot"

[agents.empty]
command = "  "

[agents.typed]
command = 1

[agents.aider]
command = "aider"
match = ["aider", "aider-chat"]
resume_last = "aider --restore"
extra = true
"#;
    let (c, n) = load_from_str(src).unwrap();
    // A known agent with an empty command keeps its default command but takes other keys.
    let claude = &c.agents.defs["claude"];
    assert_eq!(claude.command, Config::default().agents.defs["claude"].command);
    assert_eq!(claude.icon.as_deref(), Some("robot"));
    // Unknown agents need a non-empty string command.
    assert_eq!(notice_for(&n, "agents.empty.command"), "must not be empty; agent ignored");
    assert!(!c.agents.defs.contains_key("empty"));
    assert!(!c.agents.defs.contains_key("typed"));
    assert!(n.iter().any(|x| x.key == "agents.typed.command" && x.message.contains("found an integer")));
    assert!(
        n.iter().any(|x| x.key == "agents.typed.command" && x.message == "must not be empty; agent ignored")
    );
    let aider = &c.agents.defs["aider"];
    assert_eq!(aider.command, "aider");
    assert_eq!(aider.match_, vec!["aider", "aider-chat"]);
    assert_eq!(aider.resume_last.as_deref(), Some("aider --restore"));
    assert_eq!(notice_for(&n, "agents.aider.extra"), "unknown key; ignored");
}

#[test]
fn key_bindings() {
    let src = r#"
[keys]
new_tab = "cmd+t"
clear = { send = "\u000c" }
bad_table = { send = 1 }
two_keys = { send = "a", other = "b" }
number = 5
"#;
    let (c, n) = load_from_str(src).unwrap();
    assert_eq!(c.keys.get("new_tab"), Some(&KeyBinding::Shortcut("cmd+t".into())));
    assert_eq!(c.keys.get("clear"), Some(&KeyBinding::Send { send: "\u{c}".into() }));
    for k in ["keys.bad_table", "keys.two_keys", "keys.number"] {
        assert_eq!(notice_for(&n, k), "expected a shortcut string or { send = \"…\" }; ignored");
    }
    let (_, n) = load_from_str("keys = [1]\n").unwrap();
    assert_eq!(notice_for(&n, "keys"), "expected a table, found an array; using defaults");
}

#[test]
fn unknown_top_level_keys_and_notice_display() {
    let (_, n) = load_from_str("colour = \"red\"\n[ui]\nmode = \"dark\"\n").unwrap();
    assert_eq!(n.len(), 1);
    assert_eq!(n[0].to_string(), "colour: unknown key; ignored");
    let e = load_from_str("[ui]\nmode = \n").unwrap_err();
    assert!(e.to_string().starts_with("line 2: "), "{e}");
    let e = ParseError { line: None, message: "boom".into() };
    assert_eq!(e.to_string(), "boom");
}

#[test]
fn load_from_path_cases() {
    let dir = tempfile::tempdir().unwrap();
    // Missing file: defaults, no notices.
    let (c, n) = load_from_path(&dir.path().join("none.toml")).unwrap();
    assert_eq!(c, Config::default());
    assert!(n.is_empty());
    // A directory can't be read.
    let e = load_from_path(dir.path()).unwrap_err();
    assert_eq!(e.line, None);
    assert!(e.message.starts_with("cannot read "), "{}", e.message);
    let p = dir.path().join("config.toml");
    std::fs::write(&p, "[tabs]\nposition = \"bottom\"\n").unwrap();
    assert_eq!(load_from_path(&p).unwrap().0.tabs.position, TabsPosition::Bottom);
}

macro_rules! check_enum {
    ($($t:ty),+ $(,)?) => {$(
        for name in <$t as ConfigEnum>::NAMES {
            let v = <$t as ConfigEnum>::parse(name).unwrap();
            assert_eq!(v.as_str(), *name);
            assert_eq!(v.to_string(), *name);
            assert_eq!(toml::Value::try_from(v).unwrap(), toml::Value::String((*name).to_string()));
        }
        assert!(<$t as ConfigEnum>::parse("no-such-value").is_none());
        let d = <$t>::default();
        assert!(<$t as ConfigEnum>::NAMES.contains(&d.as_str()), "{}", stringify!($t));
    )+};
}

#[test]
fn config_enums_round_trip() {
    check_enum!(
        TabsPosition,
        Mode,
        TabTitle,
        SidebarWorkspaces,
        NewTabCwd,
        WorktreeOpens,
        AgentRestore,
        AgentScope,
        Deliver,
        SizeAppliesTo,
        CursorStyle,
        OptionAsMeta,
        Bell,
        CloseOnExit,
        Osc52,
        GraphemeWidth,
    );
}

#[test]
fn section_labels_follow_spec() {
    let labels: Vec<&str> = Section::ALL.iter().map(|s| s.label()).collect();
    assert_eq!(
        labels,
        vec![
            "General",
            "Appearance",
            "Tabs",
            "Font",
            "Terminal",
            "Shell",
            "Workspaces",
            "Agents",
            "Icons",
            "Keys"
        ]
    );
}

#[test]
fn get_value_of_root_and_missing_keys() {
    let c = Config::default();
    let root = get_value(&c, "").unwrap();
    assert!(root.as_table().unwrap().contains_key("terminal"));
    assert_eq!(get_value(&c, "terminal.nope"), None);
    assert_eq!(get_value(&c, "terminal.scrollback.deeper"), None);
    assert!(find_setting("terminal.scrollback").is_some());
    assert!(find_setting("terminal.nope").is_none());
}

#[test]
fn config_writer_wraps_the_free_functions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let w = ConfigWriter::new(&path);
    assert_eq!(w.path(), path);
    w.set("tabs.position", "left").unwrap();
    w.set_at(&["icons", "my.tool"], "robot").unwrap();
    w.set("ui.theme", "nord").unwrap();
    let (c, _) = load_from_path(&path).unwrap();
    assert_eq!(c.tabs.position, TabsPosition::Left);
    assert_eq!(c.icons.get("my.tool").map(String::as_str), Some("robot"));
    w.remove("tabs.position").unwrap();
    w.remove_at(&["icons", "my.tool"]).unwrap();
    let (c, _) = load_from_path(&path).unwrap();
    assert_eq!(c.tabs.position, TabsPosition::Top);
    assert!(!c.icons.contains_key("my.tool"));
    assert_eq!(c.ui.theme, "nord");
}

#[test]
fn write_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    assert!(matches!(set_value(&path, "ui..theme", "x"), Err(WriteError::InvalidKey(k)) if k == "ui..theme"));
    assert!(matches!(set_value_at(&path, &[], "x"), Err(WriteError::InvalidKey(k)) if k.is_empty()));
    assert!(matches!(remove_value(&path, ".x"), Err(WriteError::InvalidKey(_))));
    // Nothing was written by the failed calls.
    assert!(!path.exists());
    // A directory in place of the file is an I/O error.
    let e = set_value(dir.path(), "ui.theme", "x").unwrap_err();
    assert!(matches!(e, WriteError::Io(_)), "{e}");
    assert_eq!(WriteError::NotATable("ui.theme".into()).to_string(), "\"ui.theme\" is not a table");
}

#[test]
fn removing_missing_keys_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let original = "# mine\n[ui]\ntheme = \"nord\"\n";
    std::fs::write(&path, original).unwrap();
    remove_values_at(&path, &[&["ui", "mode"], &["tabs", "position"], &["ui", "theme", "deeper"], &[]])
        .unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn removing_the_last_key_keeps_its_comments() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    // Last key of the last table: the comment moves to the end of the file.
    std::fs::write(&path, "[ui]\n# keep me\ntheme = \"nord\"\n").unwrap();
    remove_value(&path, "ui.theme").unwrap();
    let out = std::fs::read_to_string(&path).unwrap();
    assert!(out.contains("# keep me"), "{out}");
    assert!(!out.contains("nord"), "{out}");
    // Last key of a table followed by another table: the comment goes before that table.
    std::fs::write(&path, "[ui]\n# about theme\ntheme = \"nord\"\n\n[tabs]\nposition = \"left\"\n").unwrap();
    remove_value(&path, "ui.theme").unwrap();
    let out = std::fs::read_to_string(&path).unwrap();
    let comment = out.find("# about theme").expect("comment kept");
    assert!(comment < out.find("[tabs]").unwrap(), "{out}");
    assert_eq!(load_from_path(&path).unwrap().0.tabs.position, TabsPosition::Left);
}

#[test]
fn atomic_write_keeps_permissions_and_creates_dirs() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub/dir/config.toml");
    atomic_write(&path, b"a = 1\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    atomic_write(&path, b"a = 2\n").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "a = 2\n");
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o640);
    assert!(is_self_write(&path, b"a = 2\n"));
    assert!(!is_self_write(&path, b"a = 1\n"));
}

#[test]
fn ensure_default_file_creates_parents_and_reports_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a/b/config.toml");
    assert!(ensure_default_file(&path).unwrap());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG_TOML);
    // A parent that is a file can't be created.
    let file = dir.path().join("file");
    std::fs::write(&file, "x").unwrap();
    assert!(ensure_default_file(&file.join("config.toml")).is_err());
}

#[test]
fn watch_needs_a_parent_directory() {
    let e = watch(std::path::Path::new("config.toml"), |_| {}).unwrap_err();
    assert!(matches!(e, WatchError::NoParent(_)), "{e}");
    assert_eq!(e.to_string(), "config path config.toml has no parent directory");
    let dir = tempfile::tempdir().unwrap();
    let handle = watch(&dir.path().join("config.toml"), |_| {}).unwrap();
    assert_eq!(format!("{handle:?}"), "WatchHandle");
}

#[test]
fn watcher_reports_invalid_utf8_and_ignores_other_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let (tx, rx) = mpsc::channel();
    let _handle = watch(&path, move |r| {
        let _ = tx.send(r.map(|(c, _)| c.ui.theme));
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(dir.path().join("other.toml"), "x = 1\n").unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(600)).is_err(), "other files are ignored");
    std::fs::write(&path, b"\xff\xfe").unwrap();
    let got = rx.recv_timeout(Duration::from_secs(5)).expect("change");
    assert_eq!(got.unwrap_err().message, "config file is not valid UTF-8");
}
