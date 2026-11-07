//! Keystroke display: `cmd-shift-d` → `⌘⇧D`.

use gpui::Keystroke;

#[must_use]
pub fn pretty_keystroke(k: &Keystroke) -> String {
    let mut s = String::new();
    let m = &k.modifiers;
    if m.control {
        s.push('⌃');
    }
    if m.alt {
        s.push('⌥');
    }
    if m.shift {
        s.push('⇧');
    }
    if m.platform {
        s.push('⌘');
    }
    s.push_str(&pretty_key(&k.key));
    s
}

#[must_use]
pub fn pretty_key(key: &str) -> String {
    match key {
        "enter" => "↵".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "tab" => "⇥".into(),
        "escape" => "⎋".into(),
        "backspace" => "⌫".into(),
        "delete" => "⌦".into(),
        "space" => "Space".into(),
        "pageup" => "PgUp".into(),
        "pagedown" => "PgDn".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        k if k.len() == 1 => k.to_uppercase(),
        k => {
            let mut c = k.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect(),
                None => String::new(),
            }
        }
    }
}

/// Parse a config shortcut (`cmd-shift-d`) and render it.
#[must_use]
pub fn pretty_shortcut(s: &str) -> String {
    s.split_whitespace()
        .filter_map(|part| Keystroke::parse(part).ok())
        .map(|k| pretty_keystroke(&k))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A keystroke as config text (`cmd-shift-d`).
#[must_use]
pub fn keystroke_to_config(k: &Keystroke) -> String {
    let mut parts = Vec::new();
    if k.modifiers.control {
        parts.push("ctrl");
    }
    if k.modifiers.alt {
        parts.push("alt");
    }
    if k.modifiers.shift {
        parts.push("shift");
    }
    if k.modifiers.platform {
        parts.push("cmd");
    }
    let mut s = parts.join("-");
    if !s.is_empty() {
        s.push('-');
    }
    s.push_str(&k.key);
    s
}
