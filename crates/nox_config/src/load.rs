//! Lenient loading: a TOML syntax error fails the file; an invalid value only resets that key.

use std::collections::BTreeMap;
use std::path::Path;

use toml::{Table, Value};

use crate::model::{Config, ConfigEnum, KeyBinding};
use crate::schema::{SettingKind, find_setting};

/// A non-fatal problem in the config file: the key fell back to its default (or was ignored).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// Dotted key, e.g. `terminal.scrollback`.
    pub key: String,
    pub message: String,
}

impl std::fmt::Display for Notice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.key, self.message)
    }
}

/// The file could not be read or is not valid TOML; the last good config should be kept.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{}", match .line { Some(l) => format!("line {l}: {message}"), None => message.clone() })]
pub struct ParseError {
    /// 1-based line of the error, when known.
    pub line: Option<usize>,
    pub message: String,
}

/// Parses `config.toml` contents.
pub fn load_from_str(s: &str) -> Result<(Config, Vec<Notice>), ParseError> {
    let table: Table = s.parse().map_err(|e: toml::de::Error| ParseError {
        line: e.span().map(|sp| line_of(s, sp.start)),
        message: e.message().trim().to_string(),
    })?;
    let mut notices = Vec::new();
    let config = from_table(&table, &mut notices);
    Ok((config, notices))
}

/// Reads and parses a config file. A missing file yields the defaults.
pub fn load_from_path(path: &Path) -> Result<(Config, Vec<Notice>), ParseError> {
    match std::fs::read_to_string(path) {
        Ok(s) => load_from_str(&s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Config::default(), Vec::new())),
        Err(e) => Err(ParseError { line: None, message: format!("cannot read {}: {e}", path.display()) }),
    }
}

fn line_of(s: &str, byte: usize) -> usize {
    let byte = byte.min(s.len());
    s.as_bytes()[..byte].split(|&b| b == b'\n').count()
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::String(_) => "a string",
        Value::Integer(_) => "an integer",
        Value::Float(_) => "a float",
        Value::Boolean(_) => "a boolean",
        Value::Datetime(_) => "a datetime",
        Value::Array(_) => "an array",
        Value::Table(_) => "a table",
    }
}

/// Reads keys of one table, recording which were consumed and every problem found.
struct Sec<'a> {
    table: Option<&'a Table>,
    prefix: String,
    notices: &'a mut Vec<Notice>,
    seen: Vec<String>,
}

impl<'a> Sec<'a> {
    fn new(root: &'a Table, name: &str, notices: &'a mut Vec<Notice>) -> Self {
        let table = match root.get(name) {
            None => None,
            Some(Value::Table(t)) => Some(t),
            Some(other) => {
                notices.push(Notice {
                    key: name.to_string(),
                    message: format!("expected a table, found {}; using defaults", type_name(other)),
                });
                None
            }
        };
        Self::over(table, name.to_string(), notices)
    }

    fn over(table: Option<&'a Table>, prefix: String, notices: &'a mut Vec<Notice>) -> Self {
        Self { table, prefix, notices, seen: Vec::new() }
    }

    fn full(&self, key: &str) -> String {
        format!("{}.{}", self.prefix, key)
    }

    fn get(&mut self, key: &str) -> Option<&'a Value> {
        self.seen.push(key.to_string());
        self.table.and_then(|t| t.get(key))
    }

    fn notice(&mut self, key: &str, message: String) {
        let key = self.full(key);
        self.notices.push(Notice { key, message });
    }

    fn wrong_type(&mut self, key: &str, expected: &str, v: &Value) {
        self.notice(key, format!("expected {expected}, found {}; using the default", type_name(v)));
    }

    fn bool(&mut self, key: &str, out: &mut bool) {
        match self.get(key) {
            None => {}
            Some(Value::Boolean(b)) => *out = *b,
            Some(v) => self.wrong_type(key, "true or false", v),
        }
    }

    fn string(&mut self, key: &str, out: &mut String) {
        match self.get(key) {
            None => {}
            Some(Value::String(s)) => out.clone_from(s),
            Some(v) => self.wrong_type(key, "a string", v),
        }
    }

    fn opt_string(&mut self, key: &str, out: &mut Option<String>) {
        match self.get(key) {
            None => {}
            Some(Value::String(s)) => *out = Some(s.clone()),
            Some(v) => self.wrong_type(key, "a string", v),
        }
    }

    fn int_range(&self, key: &str) -> (i64, i64) {
        match find_setting(&self.full(key)).map(|m| &m.kind) {
            Some(SettingKind::Int { min, max, .. } | SettingKind::IntPair { min, max, .. }) => (*min, *max),
            _ => (i64::MIN, i64::MAX),
        }
    }

    fn check_int(&mut self, key: &str, i: i64, min: i64, max: i64) -> Option<i64> {
        if i < min || i > max {
            self.notice(key, format!("{i} is out of range ({min}–{max}); using the default"));
            None
        } else {
            Some(i)
        }
    }

    fn int<T: TryFrom<i64>>(&mut self, key: &str, out: &mut T) {
        let (min, max) = self.int_range(key);
        match self.get(key) {
            None => {}
            Some(Value::Integer(i)) => {
                if let Some(i) = self.check_int(key, *i, min, max) {
                    match T::try_from(i) {
                        Ok(v) => *out = v,
                        Err(_) => self.notice(key, format!("{i} is out of range; using the default")),
                    }
                }
            }
            Some(v) => self.wrong_type(key, "an integer", v),
        }
    }

    fn int_pair(&mut self, key: &str, out: &mut [u32; 2]) {
        let (min, max) = self.int_range(key);
        match self.get(key) {
            None => {}
            Some(Value::Array(a)) if a.len() == 2 => {
                let mut res = [0u32; 2];
                for (slot, v) in res.iter_mut().zip(a) {
                    let Some(i) = v.as_integer() else {
                        self.notice(key, "expected two integers; using the default".into());
                        return;
                    };
                    let Some(i) = self.check_int(key, i, min, max) else {
                        return;
                    };
                    *slot = i as u32;
                }
                *out = res;
            }
            Some(v) => self.wrong_type(key, "an array of two integers", v),
        }
    }

    fn float(&mut self, key: &str, out: &mut f64) {
        let (min, max) = match find_setting(&self.full(key)).map(|m| &m.kind) {
            Some(SettingKind::Float { min, max, .. }) => (*min, *max),
            _ => (f64::MIN, f64::MAX),
        };
        let f = match self.get(key) {
            None => return,
            Some(Value::Float(f)) => *f,
            Some(Value::Integer(i)) => *i as f64,
            Some(v) => return self.wrong_type(key, "a number", v),
        };
        if !f.is_finite() || f < min || f > max {
            self.notice(key, format!("{f} is out of range ({min}–{max}); using the default"));
        } else {
            *out = f;
        }
    }

    fn enumeration<E: ConfigEnum>(&mut self, key: &str, out: &mut E) {
        match self.get(key) {
            None => {}
            Some(Value::String(s)) => {
                if let Some(e) = E::parse(s) {
                    *out = e;
                } else {
                    let names = E::NAMES.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(", ");
                    self.notice(
                        key,
                        format!("unknown value \"{s}\" (expected one of {names}); using the default"),
                    );
                }
            }
            Some(v) => self.wrong_type(key, "a string", v),
        }
    }

    fn string_list(&mut self, key: &str, out: &mut Vec<String>) {
        match self.get(key) {
            None => {}
            Some(Value::Array(a)) => {
                let list: Option<Vec<String>> = a.iter().map(|v| v.as_str().map(str::to_string)).collect();
                match list {
                    Some(l) => *out = l,
                    None => self.notice(key, "expected an array of strings; using the default".into()),
                }
            }
            Some(v) => self.wrong_type(key, "an array of strings", v),
        }
    }

    /// A table of string values; bad entries are skipped individually.
    fn string_map(&mut self, key: &str, out: &mut BTreeMap<String, String>) {
        match self.get(key) {
            None => {}
            Some(Value::Table(t)) => {
                let mut map = BTreeMap::new();
                for (k, v) in t {
                    match v {
                        Value::String(s) => {
                            map.insert(k.clone(), s.clone());
                        }
                        other => {
                            let full = format!("{}.{}", self.full(key), k);
                            self.notices.push(Notice {
                                key: full,
                                message: format!("expected a string, found {}; ignored", type_name(other)),
                            });
                        }
                    }
                }
                *out = map;
            }
            Some(v) => self.wrong_type(key, "a table", v),
        }
    }

    /// Reports keys that were never read.
    fn finish(self) {
        let Some(t) = self.table else { return };
        for k in t.keys() {
            if !self.seen.iter().any(|s| s == k) {
                self.notices.push(Notice {
                    key: format!("{}.{}", self.prefix, k),
                    message: "unknown key; ignored".into(),
                });
            }
        }
    }
}

const TOP_LEVEL: &[&str] =
    &["ui", "tabs", "workspaces", "agents", "icons", "font", "terminal", "shell", "app", "keys", "window"];

fn from_table(root: &Table, notices: &mut Vec<Notice>) -> Config {
    let mut c = Config::default();

    for k in root.keys() {
        if !TOP_LEVEL.contains(&k.as_str()) {
            notices.push(Notice { key: k.clone(), message: "unknown key; ignored".into() });
        }
    }

    {
        let u = &mut c.ui;
        let mut s = Sec::new(root, "ui", notices);
        s.string("theme", &mut u.theme);
        s.enumeration("mode", &mut u.mode);
        s.string("font_family", &mut u.font_family);
        s.bool("pane_header", &mut u.pane_header);
        s.float("dim_inactive_panes", &mut u.dim_inactive_panes);
        s.bool("focus_follows_mouse", &mut u.focus_follows_mouse);
        s.bool("focus_click_through", &mut u.focus_click_through);
        s.int("window_padding", &mut u.window_padding);
        s.bool("show_git_branch", &mut u.show_git_branch);
        s.finish();
    }
    {
        let t = &mut c.tabs;
        let mut s = Sec::new(root, "tabs", notices);
        s.enumeration("position", &mut t.position);
        s.bool("sidebar_visible", &mut t.sidebar_visible);
        s.int("sidebar_width", &mut t.sidebar_width);
        s.bool("show_index", &mut t.show_index);
        s.enumeration("title", &mut t.title);
        s.bool("icons", &mut t.icons);
        s.enumeration("sidebar_workspaces", &mut t.sidebar_workspaces);
        s.finish();
    }
    {
        let w = &mut c.workspaces;
        let mut s = Sec::new(root, "workspaces", notices);
        s.enumeration("new_tab_cwd", &mut w.new_tab_cwd);
        s.bool("close_when_empty", &mut w.close_when_empty);
        s.string("worktree_dir", &mut w.worktree_dir);
        s.enumeration("worktree_opens", &mut w.worktree_opens);
        s.bool("lazy_restore", &mut w.lazy_restore);
        s.bool("rerun_startup_commands", &mut w.rerun_startup_commands);
        s.finish();
    }
    {
        let a = &mut c.agents;
        let mut s = Sec::new(root, "agents", notices);
        s.bool("keep_awake", &mut a.keep_awake);
        s.enumeration("restore", &mut a.restore);
        s.bool("cooperation", &mut a.cooperation);
        s.enumeration("scope", &mut a.scope);
        s.enumeration("deliver", &mut a.deliver);
        s.bool("allow_send_to_shell", &mut a.allow_send_to_shell);
        s.int("claim_ttl", &mut a.claim_ttl);
        if let Some(t) = s.table {
            for (name, v) in t {
                if let Value::Table(def_table) = v {
                    s.seen.push(name.clone());
                    let base = a.defs.get(name).cloned();
                    let known = base.is_some();
                    let mut def = base.unwrap_or_default();
                    let mut d = Sec::over(Some(def_table), format!("agents.{name}"), s.notices);
                    d.string("command", &mut def.command);
                    d.opt_string("resume", &mut def.resume);
                    d.opt_string("resume_last", &mut def.resume_last);
                    d.string_list("match", &mut def.match_);
                    d.opt_string("icon", &mut def.icon);
                    d.finish();
                    if def.command.trim().is_empty() && !known {
                        if def_table.contains_key("command") {
                            // Wrong type or empty: already reported or obviously empty.
                            s.notices.push(Notice {
                                key: format!("agents.{name}.command"),
                                message: "must not be empty; agent ignored".into(),
                            });
                        } else {
                            s.notices.push(Notice {
                                key: format!("agents.{name}.command"),
                                message: "missing; agent ignored".into(),
                            });
                        }
                        continue;
                    }
                    if def.command.trim().is_empty() {
                        def.command.clone_from(&a.defs[name].command);
                    }
                    a.defs.insert(name.clone(), def);
                }
            }
        }
        s.finish();
    }
    {
        let mut s = Sec::over(Some(root), String::new(), notices);
        s.string_map("icons", &mut c.icons);
        fix_root_prefix(notices);
    }
    {
        let f = &mut c.font;
        let mut s = Sec::new(root, "font", notices);
        s.string("family", &mut f.family);
        s.float("size", &mut f.size);
        s.float("line_height", &mut f.line_height);
        s.float("letter_spacing", &mut f.letter_spacing);
        s.string("weight", &mut f.weight);
        s.string("weight_bold", &mut f.weight_bold);
        s.enumeration("size_applies_to", &mut f.size_applies_to);
        s.finish();
    }
    {
        let t = &mut c.terminal;
        let mut s = Sec::new(root, "terminal", notices);
        s.int("scrollback", &mut t.scrollback);
        s.float("scroll_multiplier", &mut t.scroll_multiplier);
        s.int("smooth_scroll_duration", &mut t.smooth_scroll_duration);
        s.bool("scroll_on_input", &mut t.scroll_on_input);
        s.enumeration("cursor_style", &mut t.cursor_style);
        s.bool("cursor_blink", &mut t.cursor_blink);
        s.int_pair("padding", &mut t.padding);
        s.bool("copy_on_select", &mut t.copy_on_select);
        s.bool("confirm_multiline_paste", &mut t.confirm_multiline_paste);
        s.string("word_separators", &mut t.word_separators);
        s.enumeration("option_as_meta", &mut t.option_as_meta);
        s.bool("bold_is_bright", &mut t.bold_is_bright);
        s.enumeration("bell", &mut t.bell);
        s.bool("attention_sound", &mut t.attention_sound);
        s.float("attention_sound_volume", &mut t.attention_sound_volume);
        s.bool("notifications", &mut t.notifications);
        s.int("attention_min_duration", &mut t.attention_min_duration);
        s.enumeration("close_on_exit", &mut t.close_on_exit);
        s.enumeration("osc52", &mut t.osc52);
        s.enumeration("grapheme_width", &mut t.grapheme_width);
        s.bool("kitty_keyboard", &mut t.kitty_keyboard);
        s.float("minimum_contrast", &mut t.minimum_contrast);
        s.finish();
    }
    {
        let sh = &mut c.shell;
        let mut s = Sec::new(root, "shell", notices);
        s.string("program", &mut sh.program);
        s.string_list("args", &mut sh.args);
        s.string("working_directory", &mut sh.working_directory);
        s.bool("inherit_cwd", &mut sh.inherit_cwd);
        s.bool("integration", &mut sh.integration);
        s.string_map("env", &mut sh.env);
        s.finish();
    }
    {
        let a = &mut c.app;
        let mut s = Sec::new(root, "app", notices);
        s.bool("restore_session", &mut a.restore_session);
        s.bool("confirm_close", &mut a.confirm_close);
        s.bool("quit_when_last_tab_closed", &mut a.quit_when_last_tab_closed);
        s.bool("dock_bounce_on_bell", &mut a.dock_bounce_on_bell);
        s.bool("check_for_updates", &mut a.check_for_updates);
        s.bool("auto_install_updates", &mut a.auto_install_updates);
        s.string("update_channel", &mut a.update_channel);
        s.finish();
    }
    match root.get("keys") {
        None => {}
        Some(Value::Table(t)) => {
            for (action, v) in t {
                match parse_key_binding(v) {
                    Some(b) => {
                        c.keys.insert(action.clone(), b);
                    }
                    None => notices.push(Notice {
                        key: format!("keys.{action}"),
                        message: "expected a shortcut string or { send = \"…\" }; ignored".into(),
                    }),
                }
            }
        }
        Some(other) => notices.push(Notice {
            key: "keys".into(),
            message: format!("expected a table, found {}; using defaults", type_name(other)),
        }),
    }
    {
        let w = &mut c.window;
        let mut s = Sec::new(root, "window", notices);
        s.int("width", &mut w.width);
        s.int("height", &mut w.height);
        s.int("x", &mut w.x);
        s.int("y", &mut w.y);
        s.finish();
    }
    c
}

/// `Sec::over(root, "")` produces keys like `.icons`; strip the leading dot.
fn fix_root_prefix(notices: &mut [Notice]) {
    for n in notices {
        if let Some(stripped) = n.key.strip_prefix('.') {
            n.key = stripped.to_string();
        }
    }
}

fn parse_key_binding(v: &Value) -> Option<KeyBinding> {
    match v {
        Value::String(s) => Some(KeyBinding::Shortcut(s.clone())),
        Value::Table(t) if t.len() == 1 => match t.get("send") {
            Some(Value::String(s)) => Some(KeyBinding::Send { send: s.clone() }),
            _ => None,
        },
        _ => None,
    }
}
