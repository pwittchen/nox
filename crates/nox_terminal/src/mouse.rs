//! Mouse reporting (SPEC §7.1): normal / button / any-event tracking, SGR and UTF-8 encodings.

use alacritty_terminal::term::TermMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    /// Motion with no button held (any-event tracking).
    None,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MouseMods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// Whether the program wants mouse reports at all.
#[must_use]
pub fn reporting(mode: TermMode) -> bool {
    mode.intersects(TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
}

/// Encode a mouse event at 0-based `col`/`row`, or `None` if the current mode doesn't report
/// it.
#[must_use]
pub fn encode(
    mode: TermMode,
    button: MouseButton,
    action: MouseAction,
    mods: MouseMods,
    col: usize,
    row: usize,
) -> Option<Vec<u8>> {
    if !reporting(mode) {
        return None;
    }
    let is_wheel = matches!(
        button,
        MouseButton::WheelUp | MouseButton::WheelDown | MouseButton::WheelLeft | MouseButton::WheelRight
    );
    if action == MouseAction::Motion {
        let held = button != MouseButton::None;
        let wanted = mode.contains(TermMode::MOUSE_MOTION) || (held && mode.contains(TermMode::MOUSE_DRAG));
        if !wanted {
            return None;
        }
    }
    let mut code: u32 = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::None => 3,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
        MouseButton::WheelLeft => 66,
        MouseButton::WheelRight => 67,
    };
    if action == MouseAction::Motion {
        code += 32;
    }
    if mods.shift {
        code += 4;
    }
    if mods.alt {
        code += 8;
    }
    if mods.ctrl {
        code += 16;
    }

    if mode.contains(TermMode::SGR_MOUSE) {
        let suffix = if action == MouseAction::Release && !is_wheel { 'm' } else { 'M' };
        return Some(format!("\x1b[<{};{};{}{}", code, col + 1, row + 1, suffix).into_bytes());
    }

    // Legacy encodings: release is button 3, no button identity.
    if action == MouseAction::Release {
        if is_wheel {
            return None;
        }
        code = (code & !0b11) | 3;
    }
    let mut out = b"\x1b[M".to_vec();
    out.push((32 + code) as u8);
    if mode.contains(TermMode::UTF8_MOUSE) {
        for v in [col, row] {
            let c = char::from_u32(32 + 1 + v as u32)?;
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    } else {
        if col > 222 || row > 222 {
            return None;
        }
        out.push((32 + 1 + col) as u8);
        out.push((32 + 1 + row) as u8);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_press_release() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let m = MouseMods::default();
        assert_eq!(encode(mode, MouseButton::Left, MouseAction::Press, m, 4, 9).unwrap(), b"\x1b[<0;5;10M");
        assert_eq!(encode(mode, MouseButton::Left, MouseAction::Release, m, 4, 9).unwrap(), b"\x1b[<0;5;10m");
        assert_eq!(
            encode(mode, MouseButton::WheelUp, MouseAction::Press, m, 0, 0).unwrap(),
            b"\x1b[<64;1;1M"
        );
        assert!(encode(mode, MouseButton::Left, MouseAction::Motion, m, 0, 0).is_none());
    }

    #[test]
    fn drag_and_motion_modes() {
        let drag = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let m = MouseMods::default();
        assert_eq!(encode(drag, MouseButton::Left, MouseAction::Motion, m, 1, 1).unwrap(), b"\x1b[<32;2;2M");
        assert!(encode(drag, MouseButton::None, MouseAction::Motion, m, 1, 1).is_none());
        let any = TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE;
        assert_eq!(encode(any, MouseButton::None, MouseAction::Motion, m, 1, 1).unwrap(), b"\x1b[<35;2;2M");
    }

    #[test]
    fn button_codes_and_modifiers() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let m = MouseMods::default();
        let code = |b, mods| {
            let out = encode(mode, b, MouseAction::Press, mods, 0, 0).unwrap();
            let s = String::from_utf8(out).unwrap();
            s.trim_start_matches("\x1b[<").split(';').next().unwrap().parse::<u32>().unwrap()
        };
        assert_eq!(code(MouseButton::Middle, m), 1);
        assert_eq!(code(MouseButton::Right, m), 2);
        assert_eq!(code(MouseButton::WheelDown, m), 65);
        assert_eq!(code(MouseButton::WheelLeft, m), 66);
        assert_eq!(code(MouseButton::WheelRight, m), 67);
        assert_eq!(code(MouseButton::Left, MouseMods { shift: true, ..m }), 4);
        assert_eq!(code(MouseButton::Left, MouseMods { alt: true, ..m }), 8);
        assert_eq!(code(MouseButton::Right, MouseMods { shift: true, alt: true, ctrl: true }), 30);
        // Wheel "releases" are reported with M in SGR, never m.
        assert_eq!(
            encode(mode, MouseButton::WheelDown, MouseAction::Release, m, 2, 3).unwrap(),
            b"\x1b[<65;3;4M"
        );
    }

    #[test]
    fn reporting_modes() {
        assert!(!reporting(TermMode::empty()));
        assert!(!reporting(TermMode::SGR_MOUSE));
        assert!(reporting(TermMode::MOUSE_REPORT_CLICK));
        assert!(reporting(TermMode::MOUSE_DRAG));
        assert!(reporting(TermMode::MOUSE_MOTION));
        // Click-only tracking reports no motion at all.
        let click = TermMode::MOUSE_REPORT_CLICK;
        let m = MouseMods::default();
        assert!(encode(click, MouseButton::None, MouseAction::Motion, m, 0, 0).is_none());
    }

    #[test]
    fn legacy_limits_and_utf8() {
        let m = MouseMods::default();
        let legacy = TermMode::MOUSE_REPORT_CLICK;
        // Legacy wheel releases aren't reported.
        assert!(encode(legacy, MouseButton::WheelUp, MouseAction::Release, m, 0, 0).is_none());
        assert_eq!(
            encode(legacy, MouseButton::WheelUp, MouseAction::Press, m, 0, 0).unwrap(),
            b"\x1b[M\x60\x21\x21"
        );
        // Right release keeps the modifier bits but becomes button 3.
        let shift = MouseMods { shift: true, ..m };
        assert_eq!(
            encode(legacy, MouseButton::Right, MouseAction::Release, shift, 0, 0).unwrap(),
            b"\x1b[M\x27\x21\x21"
        );
        // X10 coordinates stop at 223.
        assert_eq!(encode(legacy, MouseButton::Left, MouseAction::Press, m, 222, 222).unwrap()[4], 255);
        assert!(encode(legacy, MouseButton::Left, MouseAction::Press, m, 223, 0).is_none());
        assert!(encode(legacy, MouseButton::Left, MouseAction::Press, m, 0, 223).is_none());
        // UTF-8 mode encodes large coordinates as UTF-8 characters.
        let utf8 = TermMode::MOUSE_REPORT_CLICK | TermMode::UTF8_MOUSE;
        let out = encode(utf8, MouseButton::Left, MouseAction::Press, m, 300, 1).unwrap();
        let mut want = b"\x1b[M\x20".to_vec();
        want.extend("\u{14d}".as_bytes()); // 32 + 1 + 300 = 333
        want.push(b'"');
        assert_eq!(out, want);
        // ...but not ones that would be surrogate code points.
        assert!(encode(utf8, MouseButton::Left, MouseAction::Press, m, 0xD800, 0).is_none());
    }

    #[test]
    fn legacy_encoding() {
        let mode = TermMode::MOUSE_REPORT_CLICK;
        let m = MouseMods { ctrl: true, ..Default::default() };
        assert_eq!(
            encode(mode, MouseButton::Left, MouseAction::Press, m, 0, 0).unwrap(),
            b"\x1b[M\x30\x21\x21"
        );
        assert_eq!(
            encode(mode, MouseButton::Left, MouseAction::Release, MouseMods::default(), 0, 0).unwrap(),
            b"\x1b[M\x23\x21\x21"
        );
        assert!(encode(TermMode::empty(), MouseButton::Left, MouseAction::Press, m, 0, 0).is_none());
    }
}
