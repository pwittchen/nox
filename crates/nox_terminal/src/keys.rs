//! Key encoder (SPEC §7.9): turns a key press into the bytes a terminal program expects,
//! honoring the terminal's mode flags (application cursor keys, application keypad) and
//! nox's keyboard settings (option-as-meta, iTerm "Natural Text Editing" defaults).
//!
//! Input is platform-neutral and mirrors GPUI's `Keystroke`: `key` is the key name
//! (`"a"`, `"enter"`, `"left"`, `"f5"`, …) and `key_char` the character the key would type
//! with the current modifiers (if any).

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// ⌘
    pub cmd: bool,
}

#[derive(Debug, Clone)]
pub struct KeyInput<'a> {
    pub key: &'a str,
    pub key_char: Option<&'a str>,
    pub mods: Mods,
    /// Whether the pressed Option key acts as Meta (sends ESC-prefixed sequences), already
    /// resolved from `terminal.option_as_meta` and which Option key is down.
    pub alt_is_meta: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyModes {
    pub app_cursor: bool,
    pub app_keypad: bool,
}

/// Encode a key press. `None` means the key produces no terminal input (e.g. a ⌘ shortcut,
/// which is left to the app, or a bare modifier).
#[must_use]
pub fn encode(input: &KeyInput<'_>, modes: KeyModes) -> Option<Vec<u8>> {
    let KeyInput { key, key_char, mods, alt_is_meta } = *input;
    let meta = mods.alt && alt_is_meta;

    // ⌘ combinations: only the natural-text-editing ones reach the program.
    if mods.cmd {
        return match (key, mods.ctrl || mods.alt) {
            ("left", false) => Some(vec![0x01]),      // ^A line start
            ("right", false) => Some(vec![0x05]),     // ^E line end
            ("backspace", false) => Some(vec![0x15]), // ^U delete line
            _ => None,
        };
    }

    let m = modifier_param(mods);

    match key {
        "enter" => {
            // ⇧↵ (newline inside agent prompts, SPEC §7.12) and ⌥↵ send ESC ↵.
            let esc = (mods.shift && !mods.ctrl) || mods.alt;
            return Some(if esc { b"\x1b\r".to_vec() } else { b"\r".to_vec() });
        }
        "backspace" => {
            return Some(if mods.alt {
                b"\x1b\x7f".to_vec() // ⌥⌫ delete word
            } else if mods.ctrl {
                vec![0x08]
            } else {
                vec![0x7f]
            });
        }
        "tab" => {
            return Some(if mods.shift { b"\x1b[Z".to_vec() } else { b"\t".to_vec() });
        }
        "escape" => {
            return Some(if meta { b"\x1b\x1b".to_vec() } else { b"\x1b".to_vec() });
        }
        "space" => {
            if mods.ctrl {
                return Some(prefix_meta(meta, vec![0x00]));
            }
            return Some(prefix_meta(meta, b" ".to_vec()));
        }
        "left" | "right" | "up" | "down" | "home" | "end" => {
            // ⌥←/⌥→: word left / right (natural text editing).
            if mods.alt && !mods.ctrl && !mods.shift {
                match key {
                    "left" => return Some(b"\x1bb".to_vec()),
                    "right" => return Some(b"\x1bf".to_vec()),
                    _ => {}
                }
            }
            let c = match key {
                "up" => 'A',
                "down" => 'B',
                "right" => 'C',
                "left" => 'D',
                "home" => 'H',
                _ => 'F',
            };
            return Some(if m > 1 {
                format!("\x1b[1;{m}{c}").into_bytes()
            } else if modes.app_cursor {
                format!("\x1bO{c}").into_bytes()
            } else {
                format!("\x1b[{c}").into_bytes()
            });
        }
        "pageup" | "pagedown" | "insert" | "delete" => {
            let n = match key {
                "insert" => 2,
                "delete" => 3,
                "pageup" => 5,
                _ => 6,
            };
            return Some(tilde(n, m));
        }
        _ => {}
    }

    if let Some(n) = key.strip_prefix('f').and_then(|s| s.parse::<u8>().ok()) {
        return function_key(n, m);
    }

    // Ctrl + character.
    if mods.ctrl {
        let base = key_char.filter(|c| c.chars().count() == 1).unwrap_or(key);
        if let Some(byte) = ctrl_byte(base, key) {
            return Some(prefix_meta(mods.alt, vec![byte]));
        }
        if base.chars().count() == 1 {
            // Ctrl with a key that has no control code: send the character.
            return Some(prefix_meta(meta, base.as_bytes().to_vec()));
        }
        return None;
    }

    // Option as Meta: ESC + the unmodified character.
    if meta {
        let mut base = key.to_string();
        if base.chars().count() != 1 {
            return None;
        }
        if mods.shift {
            base = base.to_uppercase();
        }
        return Some(prefix_meta(true, base.into_bytes()));
    }

    // Plain text (including Option-composed characters when Option isn't Meta).
    if let Some(text) = key_char
        && !text.is_empty()
    {
        return Some(text.as_bytes().to_vec());
    }
    if key.chars().count() == 1 {
        let s = if mods.shift { key.to_uppercase() } else { key.to_string() };
        return Some(s.into_bytes());
    }
    None
}

/// Whether `terminal.option_as_meta = "auto"` makes the left Option key Meta on the keyboard
/// layout with this input source ID: only on the US / ABC layouts, whose Option layer holds
/// symbols nobody types in a shell. Other layouts (Polish Pro, German, French, …) need Option
/// for their letters, so it types characters there.
#[must_use]
pub fn layout_uses_option_as_meta(layout_id: &str) -> bool {
    matches!(layout_id, "com.apple.keylayout.US" | "com.apple.keylayout.ABC")
}

/// Keypad keys in application keypad mode. GPUI doesn't report keypad keys distinctly, so this
/// is exposed for completeness and used when a platform layer can tell.
#[must_use]
pub fn encode_keypad(c: char, modes: KeyModes) -> Vec<u8> {
    if !modes.app_keypad {
        return c.to_string().into_bytes();
    }
    let code = match c {
        '0'..='9' => (b'p' + (c as u8 - b'0')) as char,
        '.' => 'n',
        '+' => 'k',
        '-' => 'm',
        '*' => 'j',
        '/' => 'o',
        '=' => 'X',
        '\r' | '\n' => 'M',
        _ => return c.to_string().into_bytes(),
    };
    format!("\x1bO{code}").into_bytes()
}

fn modifier_param(mods: Mods) -> u8 {
    1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.ctrl)
}

fn prefix_meta(meta: bool, mut bytes: Vec<u8>) -> Vec<u8> {
    if meta {
        bytes.insert(0, 0x1b);
    }
    bytes
}

fn tilde(n: u8, m: u8) -> Vec<u8> {
    if m > 1 { format!("\x1b[{n};{m}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
}

fn function_key(n: u8, m: u8) -> Option<Vec<u8>> {
    let ss3 = |c: char| {
        if m > 1 { format!("\x1b[1;{m}{c}").into_bytes() } else { format!("\x1bO{c}").into_bytes() }
    };
    Some(match n {
        1 => ss3('P'),
        2 => ss3('Q'),
        3 => ss3('R'),
        4 => ss3('S'),
        5 => tilde(15, m),
        6 => tilde(17, m),
        7 => tilde(18, m),
        8 => tilde(19, m),
        9 => tilde(20, m),
        10 => tilde(21, m),
        11 => tilde(23, m),
        12 => tilde(24, m),
        13..=20 => tilde([25, 26, 28, 29, 31, 32, 33, 34][(n - 13) as usize], m),
        _ => return None,
    })
}

fn ctrl_byte(base: &str, key: &str) -> Option<u8> {
    let mut chars = base.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let c = c.to_ascii_lowercase();
    Some(match c {
        'a'..='z' => c as u8 - b'a' + 1,
        '@' | '2' | ' ' => 0x00,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '-' | '/' | '7' => 0x1f,
        '8' | '?' => 0x7f,
        _ => {
            let _ = key;
            return None;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(key: &str, key_char: Option<&str>, mods: Mods) -> Option<Vec<u8>> {
        encode(&KeyInput { key, key_char, mods, alt_is_meta: true }, KeyModes::default())
    }

    const NONE: Mods = Mods { ctrl: false, alt: false, shift: false, cmd: false };
    const CTRL: Mods = Mods { ctrl: true, ..NONE };
    const ALT: Mods = Mods { alt: true, ..NONE };
    const SHIFT: Mods = Mods { shift: true, ..NONE };
    const CMD: Mods = Mods { cmd: true, ..NONE };

    #[test]
    fn plain_text_and_controls() {
        assert_eq!(enc("a", Some("a"), NONE).unwrap(), b"a");
        assert_eq!(enc("a", Some("A"), SHIFT).unwrap(), b"A");
        assert_eq!(enc("c", Some("c"), CTRL).unwrap(), vec![3]);
        assert_eq!(enc("enter", None, NONE).unwrap(), b"\r");
        assert_eq!(enc("backspace", None, NONE).unwrap(), vec![0x7f]);
        assert_eq!(enc("tab", None, SHIFT).unwrap(), b"\x1b[Z");
        assert_eq!(enc("[", Some("["), CTRL).unwrap(), vec![0x1b]);
        assert_eq!(enc("space", Some(" "), CTRL).unwrap(), vec![0]);
        assert_eq!(enc("ł", Some("ł"), NONE).unwrap(), "ł".as_bytes());
    }

    #[test]
    fn cursor_keys_respect_app_mode() {
        let input = KeyInput { key: "up", key_char: None, mods: NONE, alt_is_meta: true };
        assert_eq!(encode(&input, KeyModes::default()).unwrap(), b"\x1b[A");
        assert_eq!(encode(&input, KeyModes { app_cursor: true, app_keypad: false }).unwrap(), b"\x1bOA");
        let shift_up = KeyInput { mods: SHIFT, ..input };
        assert_eq!(encode(&shift_up, KeyModes::default()).unwrap(), b"\x1b[1;2A");
    }

    #[test]
    fn natural_text_editing() {
        assert_eq!(enc("left", None, ALT).unwrap(), b"\x1bb");
        assert_eq!(enc("right", None, ALT).unwrap(), b"\x1bf");
        assert_eq!(enc("left", None, CMD).unwrap(), vec![0x01]);
        assert_eq!(enc("right", None, CMD).unwrap(), vec![0x05]);
        assert_eq!(enc("backspace", None, ALT).unwrap(), b"\x1b\x7f");
        assert_eq!(enc("backspace", None, CMD).unwrap(), vec![0x15]);
        assert_eq!(enc("t", Some("t"), CMD), None);
    }

    #[test]
    fn option_as_meta() {
        assert_eq!(enc("x", Some("≈"), ALT).unwrap(), b"\x1bx");
        let compose = KeyInput { key: "x", key_char: Some("≈"), mods: ALT, alt_is_meta: false };
        assert_eq!(encode(&compose, KeyModes::default()).unwrap(), "≈".as_bytes());
        assert_eq!(enc("a", Some("A"), Mods { shift: true, ..ALT }).unwrap(), b"\x1bA");
        // Polish Pro: ⌥A types „ą" when Option isn't Meta.
        let polish = KeyInput { key: "a", key_char: Some("ą"), mods: ALT, alt_is_meta: false };
        assert_eq!(encode(&polish, KeyModes::default()).unwrap(), "ą".as_bytes());
    }

    #[test]
    fn auto_option_as_meta_only_on_us_layouts() {
        assert!(layout_uses_option_as_meta("com.apple.keylayout.US"));
        assert!(layout_uses_option_as_meta("com.apple.keylayout.ABC"));
        assert!(!layout_uses_option_as_meta("com.apple.keylayout.PolishPro"));
        assert!(!layout_uses_option_as_meta("com.apple.keylayout.German"));
    }

    #[test]
    fn shift_enter_and_function_keys() {
        assert_eq!(enc("enter", None, SHIFT).unwrap(), b"\x1b\r");
        assert_eq!(enc("f1", None, NONE).unwrap(), b"\x1bOP");
        assert_eq!(enc("f5", None, NONE).unwrap(), b"\x1b[15~");
        assert_eq!(enc("f5", None, CTRL).unwrap(), b"\x1b[15;5~");
        assert_eq!(enc("delete", None, NONE).unwrap(), b"\x1b[3~");
        assert_eq!(enc("pageup", None, SHIFT).unwrap(), b"\x1b[5;2~");
    }

    #[test]
    fn remaining_special_keys() {
        assert_eq!(enc("backspace", None, CTRL).unwrap(), vec![0x08]);
        assert_eq!(enc("escape", None, NONE).unwrap(), b"\x1b");
        assert_eq!(enc("escape", None, ALT).unwrap(), b"\x1b\x1b");
        assert_eq!(enc("space", Some(" "), NONE).unwrap(), b" ");
        assert_eq!(enc("space", Some(" "), ALT).unwrap(), b"\x1b ");
        assert_eq!(enc("space", Some(" "), Mods { ctrl: true, ..ALT }).unwrap(), b"\x1b\x00");
        assert_eq!(enc("enter", None, ALT).unwrap(), b"\x1b\r");
        // ⇧⌃↵ is a plain CR (only ⇧↵ without ⌃ sends ESC CR).
        assert_eq!(enc("enter", None, Mods { ctrl: true, ..SHIFT }).unwrap(), b"\r");
        assert_eq!(enc("tab", None, NONE).unwrap(), b"\t");
        // ⌘ with other modifiers, or ⌘ + a non-editing key, is left to the app.
        assert_eq!(enc("left", None, Mods { alt: true, ..CMD }), None);
        assert_eq!(enc("backspace", None, Mods { ctrl: true, ..CMD }), None);
    }

    #[test]
    fn every_cursor_key() {
        for (key, c) in
            [("up", 'A'), ("down", 'B'), ("right", 'C'), ("left", 'D'), ("home", 'H'), ("end", 'F')]
        {
            assert_eq!(enc(key, None, NONE).unwrap(), format!("\x1b[{c}").into_bytes(), "{key}");
            assert_eq!(enc(key, None, CTRL).unwrap(), format!("\x1b[1;5{c}").into_bytes(), "{key}");
            let app = encode(
                &KeyInput { key, key_char: None, mods: NONE, alt_is_meta: true },
                KeyModes { app_cursor: true, app_keypad: false },
            );
            assert_eq!(app.unwrap(), format!("\x1bO{c}").into_bytes(), "{key}");
        }
        // ⌥↑ / ⌥Home aren't word motions: they get the xterm modifier parameter (3 = Alt).
        assert_eq!(enc("up", None, ALT).unwrap(), b"\x1b[1;3A");
        assert_eq!(enc("home", None, ALT).unwrap(), b"\x1b[1;3H");
        // ⌥⇧← is a modified cursor key, not a word motion.
        assert_eq!(enc("left", None, Mods { shift: true, ..ALT }).unwrap(), b"\x1b[1;4D");
        assert_eq!(enc("left", None, Mods { ctrl: true, ..ALT }).unwrap(), b"\x1b[1;7D");
        // Modified keys ignore application cursor mode.
        let shift_end = KeyInput { key: "end", key_char: None, mods: SHIFT, alt_is_meta: true };
        assert_eq!(
            encode(&shift_end, KeyModes { app_cursor: true, app_keypad: false }).unwrap(),
            b"\x1b[1;2F"
        );
    }

    #[test]
    fn tilde_keys() {
        assert_eq!(enc("insert", None, NONE).unwrap(), b"\x1b[2~");
        assert_eq!(enc("pageup", None, NONE).unwrap(), b"\x1b[5~");
        assert_eq!(enc("pagedown", None, NONE).unwrap(), b"\x1b[6~");
        assert_eq!(
            enc("delete", None, Mods { shift: true, alt: true, ctrl: true, cmd: false }).unwrap(),
            b"\x1b[3;8~"
        );
    }

    #[test]
    fn every_function_key() {
        let expected: [&[u8]; 20] = [
            b"\x1bOP",
            b"\x1bOQ",
            b"\x1bOR",
            b"\x1bOS",
            b"\x1b[15~",
            b"\x1b[17~",
            b"\x1b[18~",
            b"\x1b[19~",
            b"\x1b[20~",
            b"\x1b[21~",
            b"\x1b[23~",
            b"\x1b[24~",
            b"\x1b[25~",
            b"\x1b[26~",
            b"\x1b[28~",
            b"\x1b[29~",
            b"\x1b[31~",
            b"\x1b[32~",
            b"\x1b[33~",
            b"\x1b[34~",
        ];
        for (i, want) in expected.iter().enumerate() {
            let key = format!("f{}", i + 1);
            assert_eq!(enc(&key, None, NONE).unwrap(), *want, "{key}");
        }
        assert_eq!(enc("f2", None, SHIFT).unwrap(), b"\x1b[1;2Q");
        assert_eq!(enc("f12", None, ALT).unwrap(), b"\x1b[24;3~");
        assert_eq!(enc("f21", None, NONE), None);
        assert_eq!(enc("f0", None, NONE), None);
    }

    #[test]
    fn ctrl_symbol_table() {
        for (key, byte) in [
            ("@", 0x00),
            ("2", 0x00),
            ("3", 0x1b),
            ("\\", 0x1c),
            ("4", 0x1c),
            ("]", 0x1d),
            ("5", 0x1d),
            ("^", 0x1e),
            ("6", 0x1e),
            ("_", 0x1f),
            ("-", 0x1f),
            ("/", 0x1f),
            ("7", 0x1f),
            ("8", 0x7f),
            ("?", 0x7f),
        ] {
            assert_eq!(enc(key, Some(key), CTRL).unwrap(), vec![byte], "ctrl+{key}");
        }
        // Upper-case letters map like lower-case ones.
        assert_eq!(enc("C", Some("C"), CTRL).unwrap(), vec![3]);
        // ⌃⌥ prefixes ESC.
        assert_eq!(enc("c", Some("c"), Mods { ctrl: true, ..ALT }).unwrap(), b"\x1b\x03");
        // key_char wins over the key name when it is a single character.
        assert_eq!(enc("x", Some("a"), CTRL).unwrap(), vec![1]);
    }

    #[test]
    fn ctrl_without_control_code_sends_the_character() {
        assert_eq!(enc("1", Some("1"), CTRL).unwrap(), b"1");
        assert_eq!(enc("1", Some("1"), Mods { ctrl: true, ..ALT }).unwrap(), b"\x1b1");
        // Multi-character key names without a control code produce nothing.
        assert_eq!(enc("capslock", None, CTRL), None);
    }

    #[test]
    fn meta_and_fallbacks() {
        // Option as Meta with a named key that has no encoding: nothing.
        assert_eq!(enc("capslock", None, ALT), None);
        // No key_char: fall back to the key name, upper-cased with Shift.
        assert_eq!(enc("q", None, NONE).unwrap(), b"q");
        assert_eq!(enc("q", None, SHIFT).unwrap(), b"Q");
        assert_eq!(enc("q", Some(""), NONE).unwrap(), b"q");
        // Unknown multi-character keys (bare modifiers) produce nothing.
        assert_eq!(enc("shift", None, SHIFT), None);
    }

    #[test]
    fn keypad_application_mode_table() {
        let app = KeyModes { app_cursor: false, app_keypad: true };
        for (c, code) in [
            ('0', 'p'),
            ('9', 'y'),
            ('.', 'n'),
            ('+', 'k'),
            ('-', 'm'),
            ('*', 'j'),
            ('/', 'o'),
            ('=', 'X'),
            ('\r', 'M'),
            ('\n', 'M'),
        ] {
            assert_eq!(encode_keypad(c, app), format!("\x1bO{code}").into_bytes(), "{c:?}");
        }
        assert_eq!(encode_keypad(',', app), b",");
    }

    #[test]
    fn keypad() {
        assert_eq!(encode_keypad('1', KeyModes::default()), b"1");
        assert_eq!(encode_keypad('1', KeyModes { app_cursor: false, app_keypad: true }), b"\x1bOq");
    }
}
