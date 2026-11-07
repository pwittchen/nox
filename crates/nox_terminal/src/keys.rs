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
    fn keypad() {
        assert_eq!(encode_keypad('1', KeyModes::default()), b"1");
        assert_eq!(encode_keypad('1', KeyModes { app_cursor: false, app_keypad: true }), b"\x1bOq");
    }
}
