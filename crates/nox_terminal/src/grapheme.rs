//! Grapheme-aware text input (SPEC §7.1, `terminal.grapheme_width`).
//!
//! `alacritty_terminal` sizes every code point on its own: a zero-width one is appended to the
//! previous cell, everything else gets a new cell of `wcwidth` columns. That splits emoji that
//! are one grapheme cluster but several code points — 👨‍👩‍👧 becomes three wide cells, 🇵🇱 two narrow
//! ones, 👍🏽 two wide ones — and leaves ❤️ (a narrow base + VS16) in one column.
//!
//! [`GraphemeInput`] wraps the `Term` as the parser's [`Handler`], forwards everything and
//! intercepts only `input`: a code point that continues the emoji cluster in the previous cell
//! is appended to that cell instead, and a cluster that turns into an emoji presentation is
//! widened to two columns, as Ghostty, kitty and `WezTerm` do. Only emoji sequences are merged;
//! other scripts keep alacritty's per-code-point widths. In `legacy` mode it forwards `input`
//! untouched.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Point};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::cursor_icon::CursorIcon;
use alacritty_terminal::vte::ansi::{
    Attr, CharsetIndex, ClearMode, CursorShape, CursorStyle, Handler, Hyperlink, KeyboardModes,
    KeyboardModesApplyBehavior, LineClearMode, Mode, ModifyOtherKeys, PrivateMode, Rgb, ScpCharPath,
    ScpUpdateMode, StandardCharset, TabulationClearMode,
};
use unicode_segmentation::GraphemeCursor;
use unicode_width::UnicodeWidthChar;

const ZWJ: char = '\u{200D}';
const VS16: char = '\u{FE0F}';
/// Any wide character; written to let alacritty do the wrapping when a cell is widened, then
/// replaced by the real cluster.
const WIDE_PLACEHOLDER: char = '\u{1F600}';

fn is_regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

fn is_skin_tone(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

/// Can `base` take a skin-tone modifier? Every `Emoji_Modifier_Base` is at U+1F000 or above
/// except these few narrow ones (☝ ⛹ ✊ ✋ ✌ ✍).
fn takes_skin_tone(base: char) -> bool {
    base >= '\u{1F000}' || matches!(base, '\u{261D}' | '\u{26F9}' | '\u{270A}'..='\u{270D}')
}

/// Can `base` + VS16 be an emoji? Excludes ASCII except the keycap bases (`1️⃣`, `#️⃣`, `*️⃣`).
fn takes_vs16(base: char) -> bool {
    !base.is_ascii() || matches!(base, '#' | '*' | '0'..='9')
}

/// Is there no grapheme boundary between `cluster` and `next` (UAX #29, extended clusters)?
fn continues_cluster(cluster: &str, next: char) -> bool {
    let mut s = String::with_capacity(cluster.len() + next.len_utf8());
    s.push_str(cluster);
    s.push(next);
    let mut cursor = GraphemeCursor::new(cluster.len(), s.len(), true);
    cursor.is_boundary(&s, 0) == Ok(false)
}

/// What to do with an input character, given the cell before the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Let alacritty handle it.
    Default,
    /// Append to the previous cell, widening it to two columns first if `widen`.
    Attach { widen: bool },
}

fn classify(cell: &Cell, c: char) -> Action {
    let base = cell.c;
    let zw = cell.zerowidth().unwrap_or(&[]);
    let wide = cell.flags.contains(Flags::WIDE_CHAR);
    if base == ' ' || base == '\0' || base == '\t' {
        return Action::Default;
    }
    if c == VS16 {
        let widen = !wide && takes_vs16(base) && !zw.contains(&VS16);
        return if widen { Action::Attach { widen } } else { Action::Default };
    }
    if is_regional_indicator(c) && is_regional_indicator(base) && zw.is_empty() {
        // The first of a pair: 🇵 + 🇱 → 🇵🇱. A third indicator starts a new flag.
        return Action::Attach { widen: !wide };
    }
    if is_skin_tone(c) && takes_skin_tone(base) && !zw.iter().any(|&z| is_skin_tone(z) || z == ZWJ) {
        return Action::Attach { widen: !wide };
    }
    if zw.last() == Some(&ZWJ) && c.width().unwrap_or(0) > 0 {
        let cluster: String = std::iter::once(base).chain(zw.iter().copied()).collect();
        if continues_cluster(&cluster, c) {
            return Action::Attach { widen: !wide };
        }
    }
    Action::Default
}

/// The parser's handler: the `Term` with grapheme-aware `input` (see the module docs).
pub struct GraphemeInput<'a, L: EventListener> {
    term: &'a mut Term<L>,
    enabled: bool,
}

impl<'a, L: EventListener> GraphemeInput<'a, L> {
    /// `enabled = false` is `terminal.grapheme_width = "legacy"`: a plain pass-through.
    pub fn new(term: &'a mut Term<L>, enabled: bool) -> Self {
        Self { term, enabled }
    }

    /// The cell a zero-width character would attach to (same rule as alacritty), if any on
    /// this line.
    fn prev_cell(&self) -> Option<Point> {
        let grid = self.term.grid();
        let cur = grid.cursor.point;
        let mut col = cur.column;
        if !grid.cursor.input_needs_wrap {
            if col.0 == 0 {
                return None;
            }
            col -= 1;
        }
        if grid[cur.line][col].flags.contains(Flags::WIDE_CHAR_SPACER) {
            if col.0 == 0 {
                return None;
            }
            col -= 1;
        }
        Some(Point::new(cur.line, col))
    }

    /// Turn the narrow cell at `at` into a wide one, wrapping it to the next line if it is in the
    /// last column. Returns where the cell is now, or `None` if it can't be widened (insert mode,
    /// autowrap off at the right margin, a one-column terminal).
    fn widen(&mut self, at: Point) -> Option<Point> {
        let columns = self.term.columns();
        let mode = *self.term.mode();
        let last = at.column.0 + 1 >= columns;
        if columns < 2 || mode.contains(TermMode::INSERT) || (last && !mode.contains(TermMode::LINE_WRAP)) {
            return None;
        }
        let saved = self.term.grid()[at].clone();
        let mut template = Cell {
            fg: saved.fg,
            bg: saved.bg,
            flags: saved.flags
                - (Flags::WIDE_CHAR | Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
            ..Cell::default()
        };
        template.set_underline_color(saved.underline_color());
        template.set_hyperlink(saved.hyperlink());

        // Rewrite the cell as a wide character with the cell's own attributes; alacritty places
        // the spacer, or the leading spacer and the wrap.
        let grid = self.term.grid_mut();
        let old_template = std::mem::replace(&mut grid.cursor.template, template);
        grid.cursor.point = at;
        grid.cursor.input_needs_wrap = false;
        Handler::input(self.term, WIDE_PLACEHOLDER);
        let grid = self.term.grid_mut();
        grid.cursor.template = old_template;

        let cur = grid.cursor.point;
        let back = if grid.cursor.input_needs_wrap { 1 } else { 2 };
        let pos = Point::new(cur.line, Column(cur.column.0.checked_sub(back)?));
        let cell = &mut grid[pos];
        cell.c = saved.c;
        for &z in saved.zerowidth().unwrap_or(&[]) {
            cell.push_zerowidth(z);
        }
        Some(pos)
    }
}

/// Forward `Handler` methods to the `Term` unchanged.
macro_rules! forward {
    ($( fn $name:ident(&mut self $(, $arg:ident : $ty:ty)* ); )*) => {
        $(
            #[inline]
            fn $name(&mut self $(, $arg: $ty)*) {
                Handler::$name(self.term $(, $arg)*);
            }
        )*
    };
}

impl<L: EventListener> Handler for GraphemeInput<'_, L> {
    fn input(&mut self, c: char) {
        if !self.enabled {
            return Handler::input(self.term, c);
        }
        let Some(prev) = self.prev_cell() else {
            return Handler::input(self.term, c);
        };
        match classify(&self.term.grid()[prev], c) {
            Action::Default => Handler::input(self.term, c),
            Action::Attach { widen } => {
                let at = if widen { self.widen(prev).unwrap_or(prev) } else { prev };
                self.term.grid_mut()[at].push_zerowidth(c);
            }
        }
    }

    forward! {
        fn set_title(&mut self, title: Option<String>);
        fn set_cursor_style(&mut self, style: Option<CursorStyle>);
        fn set_cursor_shape(&mut self, shape: CursorShape);
        fn goto(&mut self, line: i32, col: usize);
        fn goto_line(&mut self, line: i32);
        fn goto_col(&mut self, col: usize);
        fn insert_blank(&mut self, count: usize);
        fn move_up(&mut self, count: usize);
        fn move_down(&mut self, count: usize);
        fn identify_terminal(&mut self, intermediate: Option<char>);
        fn device_status(&mut self, arg: usize);
        fn move_forward(&mut self, col: usize);
        fn move_backward(&mut self, col: usize);
        fn move_down_and_cr(&mut self, row: usize);
        fn move_up_and_cr(&mut self, row: usize);
        fn put_tab(&mut self, count: u16);
        fn backspace(&mut self);
        fn carriage_return(&mut self);
        fn linefeed(&mut self);
        fn bell(&mut self);
        fn substitute(&mut self);
        fn newline(&mut self);
        fn set_horizontal_tabstop(&mut self);
        fn scroll_up(&mut self, count: usize);
        fn scroll_down(&mut self, count: usize);
        fn insert_blank_lines(&mut self, count: usize);
        fn delete_lines(&mut self, count: usize);
        fn erase_chars(&mut self, count: usize);
        fn delete_chars(&mut self, count: usize);
        fn move_backward_tabs(&mut self, count: u16);
        fn move_forward_tabs(&mut self, count: u16);
        fn save_cursor_position(&mut self);
        fn restore_cursor_position(&mut self);
        fn clear_line(&mut self, mode: LineClearMode);
        fn clear_screen(&mut self, mode: ClearMode);
        fn clear_tabs(&mut self, mode: TabulationClearMode);
        fn set_tabs(&mut self, interval: u16);
        fn reset_state(&mut self);
        fn reverse_index(&mut self);
        fn terminal_attribute(&mut self, attr: Attr);
        fn set_mode(&mut self, mode: Mode);
        fn unset_mode(&mut self, mode: Mode);
        fn report_mode(&mut self, mode: Mode);
        fn set_private_mode(&mut self, mode: PrivateMode);
        fn unset_private_mode(&mut self, mode: PrivateMode);
        fn report_private_mode(&mut self, mode: PrivateMode);
        fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>);
        fn set_keypad_application_mode(&mut self);
        fn unset_keypad_application_mode(&mut self);
        fn set_active_charset(&mut self, index: CharsetIndex);
        fn configure_charset(&mut self, index: CharsetIndex, charset: StandardCharset);
        fn set_color(&mut self, index: usize, color: Rgb);
        fn dynamic_color_sequence(&mut self, prefix: String, index: usize, terminator: &str);
        fn reset_color(&mut self, index: usize);
        fn clipboard_store(&mut self, clipboard: u8, data: &[u8]);
        fn clipboard_load(&mut self, clipboard: u8, terminator: &str);
        fn decaln(&mut self);
        fn push_title(&mut self);
        fn pop_title(&mut self);
        fn text_area_size_pixels(&mut self);
        fn text_area_size_chars(&mut self);
        fn set_hyperlink(&mut self, link: Option<Hyperlink>);
        fn set_mouse_cursor_icon(&mut self, icon: CursorIcon);
        fn report_keyboard_mode(&mut self);
        fn push_keyboard_mode(&mut self, mode: KeyboardModes);
        fn pop_keyboard_modes(&mut self, to_pop: u16);
        fn set_keyboard_mode(&mut self, mode: KeyboardModes, behavior: KeyboardModesApplyBehavior);
        fn set_modify_other_keys(&mut self, mode: ModifyOtherKeys);
        fn report_modify_other_keys(&mut self);
        fn set_scp(&mut self, char_path: ScpCharPath, update_mode: ScpUpdateMode);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::index::Line;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

    struct Size(usize, usize);
    impl Dimensions for Size {
        fn total_lines(&self) -> usize {
            self.1
        }
        fn screen_lines(&self) -> usize {
            self.1
        }
        fn columns(&self) -> usize {
            self.0
        }
    }

    fn run(cols: usize, enabled: bool, input: &str) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &Size(cols, 4), VoidListener);
        let mut p: Processor<StdSyncHandler> = Processor::new();
        p.advance(&mut GraphemeInput::new(&mut term, enabled), input.as_bytes());
        term
    }

    /// `(cluster, wide)` per cell of a line, skipping spacers and trailing blanks.
    fn cells(term: &Term<VoidListener>, line: i32) -> Vec<(String, bool)> {
        let row = &term.grid()[Line(line)];
        let mut out: Vec<(String, bool)> = (0..term.columns())
            .map(|c| &row[Column(c)])
            .filter(|c| !c.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER))
            .map(|c| {
                let s = std::iter::once(c.c).chain(c.zerowidth().unwrap_or(&[]).iter().copied()).collect();
                (s, c.flags.contains(Flags::WIDE_CHAR))
            })
            .collect();
        while out.last().is_some_and(|(s, _)| s == " ") {
            out.pop();
        }
        out
    }

    fn col(term: &Term<VoidListener>) -> usize {
        term.grid().cursor.point.column.0
    }

    #[test]
    fn zwj_family_is_one_wide_cell() {
        let t = run(20, true, "👨\u{200D}👩\u{200D}👧x");
        assert_eq!(cells(&t, 0), vec![("👨\u{200D}👩\u{200D}👧".into(), true), ("x".into(), false)]);
        assert_eq!(col(&t), 3);
    }

    #[test]
    fn zwj_with_narrow_component_and_vs16() {
        // 👩‍⚕️ = 👩 ZWJ ⚕ VS16; 🏳️‍🌈 = 🏳 VS16 ZWJ 🌈.
        let t = run(20, true, "👩\u{200D}⚕\u{FE0F}🏳\u{FE0F}\u{200D}🌈");
        assert_eq!(
            cells(&t, 0),
            vec![("👩\u{200D}⚕\u{FE0F}".into(), true), ("🏳\u{FE0F}\u{200D}🌈".into(), true)]
        );
        assert_eq!(col(&t), 4);
    }

    #[test]
    fn flags_pair_regional_indicators() {
        let t = run(20, true, "🇵🇱🇩🇪🇺");
        assert_eq!(cells(&t, 0), vec![("🇵🇱".into(), true), ("🇩🇪".into(), true), ("🇺".into(), false)]);
        assert_eq!(col(&t), 5);
    }

    #[test]
    fn skin_tone_attaches() {
        let t = run(20, true, "👍🏽✌🏻 🏽");
        assert_eq!(
            cells(&t, 0),
            vec![("👍🏽".into(), true), ("✌🏻".into(), true), (" ".into(), false), ("🏽".into(), true)]
        );
    }

    #[test]
    fn vs16_widens_narrow_base() {
        let t = run(20, true, "❤\u{FE0F}a1\u{FE0F}\u{20E3}b\u{FE0F}");
        assert_eq!(
            cells(&t, 0),
            vec![
                ("❤\u{FE0F}".into(), true),
                ("a".into(), false),
                ("1\u{FE0F}\u{20E3}".into(), true),
                ("b\u{FE0F}".into(), false),
            ]
        );
        assert_eq!(col(&t), 6);
    }

    #[test]
    fn widening_in_last_column_wraps() {
        let t = run(4, true, "abc❤\u{FE0F}d");
        assert_eq!(cells(&t, 0), vec![("a".into(), false), ("b".into(), false), ("c".into(), false)]);
        assert!(t.grid()[Line(0)][Column(3)].flags.contains(Flags::LEADING_WIDE_CHAR_SPACER));
        assert_eq!(cells(&t, 1), vec![("❤\u{FE0F}".into(), true), ("d".into(), false)]);
    }

    #[test]
    fn widening_keeps_attributes() {
        let t = run(20, true, "\x1b[31m❤\u{FE0F}\x1b[0mx");
        let cell = &t.grid()[Line(0)][Column(0)];
        assert!(cell.flags.contains(Flags::WIDE_CHAR));
        assert_ne!(cell.fg, t.grid()[Line(0)][Column(2)].fg);
        assert_eq!(cells(&t, 0)[1], ("x".into(), false));
    }

    #[test]
    fn zwj_between_letters_does_not_merge() {
        let t = run(20, true, "a\u{200D}b");
        assert_eq!(cells(&t, 0), vec![("a\u{200D}".into(), false), ("b".into(), false)]);
    }

    #[test]
    fn legacy_keeps_per_code_point_widths() {
        let t = run(20, false, "🇵🇱👍🏽❤\u{FE0F}");
        assert_eq!(
            cells(&t, 0),
            vec![
                ("🇵".into(), false),
                ("🇱".into(), false),
                ("👍".into(), true),
                ("🏽".into(), true),
                ("❤\u{FE0F}".into(), false),
            ]
        );
    }
}
