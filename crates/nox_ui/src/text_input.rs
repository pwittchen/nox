//! A small single-line text input (SPEC §5.8): caret, selection, ⌥/⌘ word and line movement,
//! copy / paste, undo, IME marked text. Used by the palette, find bar, inline rename, sheets
//! and the Settings window.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine,
    SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, actions, div, fill, point,
    prelude::*, px, relative, size,
};

use crate::theme::Theme;

actions!(
    nox_input,
    [
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteToStart,
        Left,
        Right,
        WordLeft,
        WordRight,
        SelectLeft,
        SelectRight,
        SelectWordLeft,
        SelectWordRight,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        Paste,
        Cut,
        Copy,
        Undo,
        Submit,
        Cancel,
        ShowCharacterPalette,
    ]
);

pub const CONTEXT: &str = "NoxTextInput";

pub fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("alt-backspace", DeleteWordLeft, c),
        KeyBinding::new("cmd-backspace", DeleteToStart, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("alt-left", WordLeft, c),
        KeyBinding::new("alt-right", WordRight, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("alt-shift-left", SelectWordLeft, c),
        KeyBinding::new("alt-shift-right", SelectWordRight, c),
        KeyBinding::new("cmd-a", SelectAll, c),
        KeyBinding::new("cmd-left", Home, c),
        KeyBinding::new("cmd-right", End, c),
        KeyBinding::new("home", Home, c),
        KeyBinding::new("end", End, c),
        KeyBinding::new("cmd-shift-left", SelectHome, c),
        KeyBinding::new("cmd-shift-right", SelectEnd, c),
        KeyBinding::new("cmd-v", Paste, c),
        KeyBinding::new("cmd-x", Cut, c),
        KeyBinding::new("cmd-c", Copy, c),
        KeyBinding::new("cmd-z", Undo, c),
        KeyBinding::new("enter", Submit, c),
        KeyBinding::new("escape", Cancel, c),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, c),
    ]);
}

#[derive(Debug, Clone)]
pub enum InputEvent {
    Changed,
    Submit,
    Cancel,
    Blur,
}

pub struct TextInput {
    focus: FocusHandle,
    pub content: String,
    placeholder: SharedString,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    undo: Vec<(String, Range<usize>)>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    selecting: bool,
    pub monospace: bool,
    blur_registered: bool,
}

impl EventEmitter<InputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>, placeholder: impl Into<SharedString>) -> Self {
        let focus = cx.focus_handle();
        Self {
            blur_registered: false,
            focus,
            content: String::new(),
            placeholder: placeholder.into(),
            selected: 0..0,
            reversed: false,
            marked: None,
            undo: vec![],
            last_layout: None,
            last_bounds: None,
            selecting: false,
            monospace: false,
        }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.content
    }

    /// Replace the content and select all of it.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.content = text.into();
        self.selected = 0..self.content.len();
        self.reversed = false;
        self.marked = None;
        cx.notify();
    }

    pub fn set_text_cursor_end(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.content = text.into();
        self.selected = self.content.len()..self.content.len();
        self.marked = None;
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    fn push_undo(&mut self) {
        if self.undo.last().map(|(s, _)| s.as_str()) != Some(self.content.as_str()) {
            self.undo.push((self.content.clone(), self.selected.clone()));
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
        }
    }

    fn cursor(&self) -> usize {
        if self.reversed { self.selected.start } else { self.selected.end }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected = offset..offset;
        self.reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.reversed {
            self.selected.start = offset;
        } else {
            self.selected.end = offset;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        cx.notify();
    }

    fn prev_boundary(&self, offset: usize) -> usize {
        self.content[..offset].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content[offset..].chars().next().map_or(offset, |c| offset + c.len_utf8())
    }

    fn word_left(&self, offset: usize) -> usize {
        let s = &self.content[..offset];
        let trimmed = s.trim_end_matches(|c: char| !c.is_alphanumeric());
        trimmed.rfind(|c: char| !c.is_alphanumeric()).map_or(0, |i| i + 1)
    }

    fn word_right(&self, offset: usize) -> usize {
        let s = &self.content[offset..];
        let skip = s.len() - s.trim_start_matches(|c: char| !c.is_alphanumeric()).len();
        let rest = &s[skip..];
        offset + skip + rest.find(|c: char| !c.is_alphanumeric()).unwrap_or(rest.len())
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.move_to(self.prev_boundary(self.cursor()), cx);
        } else {
            self.move_to(self.selected.start, cx);
        }
    }
    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.move_to(self.next_boundary(self.cursor()), cx);
        } else {
            self.move_to(self.selected.end, cx);
        }
    }
    fn word_left_a(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.word_left(self.cursor()), cx);
    }
    fn word_right_a(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.word_right(self.cursor()), cx);
    }
    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.prev_boundary(self.cursor()), cx);
    }
    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor()), cx);
    }
    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.word_left(self.cursor()), cx);
    }
    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.word_right(self.cursor()), cx);
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = 0..self.content.len();
        self.reversed = false;
        cx.notify();
    }
    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }
    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }
    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }
    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), cx);
    }
    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.select_to(self.prev_boundary(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }
    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.select_to(self.next_boundary(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }
    fn delete_word_left(&mut self, _: &DeleteWordLeft, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.select_to(self.word_left(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }
    fn delete_to_start(&mut self, _: &DeleteToStart, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
        self.replace_text_in_range(None, "", window, cx);
    }
    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
            self.replace_text_in_range(None, &text.replace('\n', " "), window, cx);
        }
    }
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected.clone()].to_string()));
        }
    }
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected.clone()].to_string()));
            self.replace_text_in_range(None, "", window, cx);
        }
    }
    fn undo_a(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((text, sel)) = self.undo.pop() {
            self.content = text;
            self.selected = sel;
            cx.emit(InputEvent::Changed);
            cx.notify();
        }
    }
    // GPUI action handlers take `&mut self` even when they only use the context.
    #[allow(clippy::unused_self)]
    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Submit);
    }
    #[allow(clippy::unused_self)]
    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Cancel);
    }
    #[allow(clippy::unused_self)]
    fn char_palette(&mut self, _: &ShowCharacterPalette, window: &mut Window, _: &mut Context<Self>) {
        window.show_character_palette();
    }

    fn index_for_position(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref()) else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left()).min(self.content.len())
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.selecting = true;
        if ev.click_count >= 2 {
            self.selected = 0..self.content.len();
            cx.notify();
            return;
        }
        if ev.modifiers.shift {
            self.select_to(self.index_for_position(ev.position), cx);
        } else {
            self.move_to(self.index_for_position(ev.position), cx);
        }
    }
    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }
    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            self.select_to(self.index_for_position(ev.position), cx);
        }
    }

    fn utf16_to_utf8(&self, offset: usize) -> usize {
        let mut u8o = 0;
        let mut u16o = 0;
        for ch in self.content.chars() {
            if u16o >= offset {
                break;
            }
            u16o += ch.len_utf16();
            u8o += ch.len_utf8();
        }
        u8o
    }
    fn utf8_to_utf16(&self, offset: usize) -> usize {
        let mut u16o = 0;
        let mut u8o = 0;
        for ch in self.content.chars() {
            if u8o >= offset {
                break;
            }
            u8o += ch.len_utf8();
            u16o += ch.len_utf16();
        }
        u16o
    }
    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.utf8_to_utf16(r.start)..self.utf8_to_utf16(r.end)
    }
    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.utf16_to_utf8(r.start)..self.utf16_to_utf8(r.end)
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let r = self.range_from_utf16(&range);
        actual.replace(self.range_to_utf16(&r));
        Some(self.content[r].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_utf16(&self.selected), reversed: self.reversed })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.push_undo();
        let r = range
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        self.content = format!("{}{}{}", &self.content[..r.start], text, &self.content[r.end..]);
        self.selected = r.start + text.len()..r.start + text.len();
        self.reversed = false;
        self.marked = None;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        new_sel: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let r = range
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        self.content = format!("{}{}{}", &self.content[..r.start], text, &self.content[r.end..]);
        self.marked = (!text.is_empty()).then(|| r.start..r.start + text.len());
        // The IME's selection is in UTF-16 units relative to the new marked text.
        let in_text = |u16o: usize| {
            let mut u16_seen = 0;
            text.char_indices()
                .find(|(_, ch)| {
                    let before = u16_seen;
                    u16_seen += ch.len_utf16();
                    before >= u16o
                })
                .map_or(text.len(), |(i, _)| i)
        };
        self.selected = new_sel.map_or_else(
            || r.start + text.len()..r.start + text.len(),
            |s| r.start + in_text(s.start)..r.start + in_text(s.end),
        );
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.last_layout.as_ref()?;
        let r = self.range_from_utf16(&range);
        Some(Bounds::from_corners(
            point(bounds.left() + layout.x_for_index(r.start), bounds.top()),
            point(bounds.left() + layout.x_for_index(r.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        p: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let local = self.last_bounds?.localize(&p)?;
        let layout = self.last_layout.as_ref()?;
        let idx = layout.index_for_x(local.x)?;
        Some(self.utf8_to_utf16(idx))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct Prepaint {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let input = self.input.read(cx);
        let theme = Theme::global(cx);
        let style = window.text_style();
        let (text, color): (SharedString, _) = if input.content.is_empty() {
            (input.placeholder.clone(), theme.text_muted)
        } else {
            (input.content.clone().into(), style.color)
        };
        let run = TextRun {
            len: text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(m) = input.marked.as_ref().filter(|_| !input.content.is_empty()) {
            vec![
                TextRun { len: m.start, ..run.clone() },
                TextRun {
                    len: m.end - m.start,
                    underline: Some(UnderlineStyle { color: Some(color), thickness: px(1.), wavy: false }),
                    ..run.clone()
                },
                TextRun { len: text.len() - m.end, ..run },
            ]
            .into_iter()
            .filter(|r| r.len > 0)
            .collect()
        } else {
            vec![run]
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window.text_system().shape_line(text, font_size, &runs, None);
        let cursor_x = if input.content.is_empty() { px(0.) } else { line.x_for_index(input.cursor()) };
        let (selection, cursor) = if input.selected.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_x, bounds.top()),
                        size(px(1.5), bounds.size.height),
                    ),
                    theme.accent,
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(bounds.left() + line.x_for_index(input.selected.start), bounds.top()),
                        point(bounds.left() + line.x_for_index(input.selected.end), bounds.bottom()),
                    ),
                    theme.selection,
                )),
                None,
            )
        };
        Prepaint { line: Some(line), cursor, selection }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut (),
        pre: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.input.clone()), cx);
        if let Some(sel) = pre.selection.take() {
            window.paint_quad(sel);
        }
        let Some(line) = pre.line.take() else { return };
        let _ = line.paint(bounds.origin, window.line_height(), window, cx);
        if focus.is_focused(window)
            && let Some(c) = pre.cursor.take()
        {
            window.paint_quad(c);
        }
        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.blur_registered {
            self.blur_registered = true;
            cx.on_blur(&self.focus, window, |_, _, cx: &mut Context<Self>| cx.emit(InputEvent::Blur))
                .detach();
        }
        div()
            .flex()
            .flex_1()
            .min_w(px(0.))
            .overflow_hidden()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_to_start))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left_a))
            .on_action(cx.listener(Self::word_right_a))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::undo_a))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::char_palette))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(TextElement { input: cx.entity() })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{Modifiers, TestAppContext, VisualTestContext};

    use super::*;

    type Events = Rc<RefCell<Vec<String>>>;

    /// Lays the input out in a 400 px wide row at (20, 10), as a sheet or the palette would.
    struct Host(Entity<TextInput>);
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().pl(px(20.)).pt(px(10.)).child(div().w(px(400.)).flex().child(self.0.clone()))
        }
    }

    /// A focused input with `text` (caret at the end), recording its events.
    fn input<'a>(
        cx: &'a mut TestAppContext,
        text: &str,
    ) -> (Entity<TextInput>, &'a mut VisualTestContext, Events) {
        crate::test_support::init(cx);
        let (host, cx) = cx.add_window_view(|_, cx| Host(cx.new(|cx| TextInput::new(cx, "Placeholder"))));
        let input = host.read_with(cx, |h, _| h.0.clone());
        let events: Events = Rc::default();
        let ev = events.clone();
        let text = text.to_string();
        input.update_in(cx, |i, window, cx| {
            cx.subscribe(&cx.entity(), move |_, _, e: &InputEvent, _| ev.borrow_mut().push(format!("{e:?}")))
                .detach();
            i.set_text_cursor_end(text, cx);
            i.focus(window);
        });
        cx.run_until_parked();
        (input, cx, events)
    }

    fn state(input: &Entity<TextInput>, cx: &mut VisualTestContext) -> (String, Range<usize>, bool) {
        input.read_with(cx, |i, _| (i.content.clone(), i.selected.clone(), i.reversed))
    }

    #[gpui::test]
    fn typing_and_deleting(cx: &mut TestAppContext) {
        let (input, cx, events) = input(cx, "");
        cx.simulate_input("héllo");
        assert_eq!(state(&input, cx), ("héllo".into(), 6..6, false));
        cx.simulate_keystrokes("backspace");
        assert_eq!(input.read_with(cx, |i, _| i.text().to_string()), "héll");
        cx.simulate_keystrokes("left left left delete");
        assert_eq!(state(&input, cx), ("hll".into(), 1..1, false));
        // Backspace / delete at the edges do nothing harmful.
        cx.simulate_keystrokes("home backspace end delete");
        assert_eq!(state(&input, cx).0, "hll");
        assert!(events.borrow().iter().all(|e| e == "Changed"));
        assert!(events.borrow().len() >= 3);
    }

    #[gpui::test]
    fn word_and_line_movement(cx: &mut TestAppContext) {
        let (input, cx, _) = input(cx, "git commit --amend");
        cx.simulate_keystrokes("alt-left");
        assert_eq!(state(&input, cx).1, 13..13);
        cx.simulate_keystrokes("alt-left alt-left");
        assert_eq!(state(&input, cx).1, 0..0);
        cx.simulate_keystrokes("alt-left");
        assert_eq!(state(&input, cx).1, 0..0);
        cx.simulate_keystrokes("alt-right");
        assert_eq!(state(&input, cx).1, 3..3);
        cx.simulate_keystrokes("alt-right alt-right alt-right");
        assert_eq!(state(&input, cx).1, 18..18);
        cx.simulate_keystrokes("cmd-left");
        assert_eq!(state(&input, cx).1, 0..0);
        cx.simulate_keystrokes("cmd-right");
        assert_eq!(state(&input, cx).1, 18..18);
        cx.simulate_keystrokes("home right");
        assert_eq!(state(&input, cx).1, 1..1);
        cx.simulate_keystrokes("end");
        assert_eq!(state(&input, cx).1, 18..18);
    }

    #[gpui::test]
    fn selection_extends_and_flips(cx: &mut TestAppContext) {
        let (input, cx, _) = input(cx, "abc def");
        cx.simulate_keystrokes("shift-left shift-left");
        assert_eq!(state(&input, cx), ("abc def".into(), 5..7, true));
        // Left / right with a selection collapse it to its edge.
        cx.simulate_keystrokes("left");
        assert_eq!(state(&input, cx).1, 5..5);
        cx.simulate_keystrokes("shift-right shift-right");
        assert_eq!(state(&input, cx), ("abc def".into(), 5..7, false));
        cx.simulate_keystrokes("right");
        assert_eq!(state(&input, cx).1, 7..7);
        // Extending past the anchor flips the direction.
        cx.simulate_keystrokes("left left shift-right shift-alt-left shift-alt-left");
        assert_eq!(state(&input, cx), ("abc def".into(), 0..5, true));
        cx.simulate_keystrokes("alt-shift-right");
        assert_eq!(state(&input, cx), ("abc def".into(), 3..5, true));
        cx.simulate_keystrokes("cmd-shift-right");
        assert_eq!(state(&input, cx), ("abc def".into(), 5..7, false));
        cx.simulate_keystrokes("cmd-shift-left");
        assert_eq!(state(&input, cx), ("abc def".into(), 0..5, true));
        cx.simulate_keystrokes("cmd-a");
        assert_eq!(state(&input, cx), ("abc def".into(), 0..7, false));
        // Typing replaces the selection.
        cx.simulate_input("x");
        assert_eq!(state(&input, cx), ("x".into(), 1..1, false));
    }

    #[gpui::test]
    fn deleting_words_and_lines(cx: &mut TestAppContext) {
        let (input, cx, _) = input(cx, "cd ~/src/nox  ");
        cx.simulate_keystrokes("alt-backspace");
        assert_eq!(state(&input, cx).0, "cd ~/src/");
        cx.simulate_keystrokes("alt-backspace");
        assert_eq!(state(&input, cx).0, "cd ~/");
        cx.simulate_keystrokes("cmd-backspace");
        assert_eq!(state(&input, cx), (String::new(), 0..0, false));
        // With a selection, word delete removes just the selection.
        input.update(cx, |i, cx| i.set_text("one two", cx));
        cx.simulate_keystrokes("alt-backspace");
        assert_eq!(state(&input, cx).0, "");
        input.update(cx, |i, cx| i.set_text_cursor_end("one two", cx));
        cx.simulate_keystrokes("shift-left backspace");
        assert_eq!(state(&input, cx).0, "one tw");
        cx.simulate_keystrokes("home shift-right delete");
        assert_eq!(state(&input, cx).0, "ne tw");
    }

    #[gpui::test]
    fn clipboard(cx: &mut TestAppContext) {
        let (input, cx, _) = input(cx, "hello world");
        // Nothing selected: copy and cut leave the clipboard alone.
        cx.write_to_clipboard(ClipboardItem::new_string("keep".into()));
        cx.simulate_keystrokes("cmd-c cmd-x");
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("keep"));
        assert_eq!(state(&input, cx).0, "hello world");

        cx.simulate_keystrokes("shift-alt-left cmd-c");
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("world"));
        cx.simulate_keystrokes("cmd-x");
        assert_eq!(state(&input, cx).0, "hello ");
        cx.simulate_keystrokes("cmd-v cmd-v");
        assert_eq!(state(&input, cx).0, "hello worldworld");
        // Pasted newlines become spaces (single-line field).
        cx.write_to_clipboard(ClipboardItem::new_string("a\nb".into()));
        cx.simulate_keystrokes("cmd-a cmd-v");
        assert_eq!(state(&input, cx), ("a b".into(), 3..3, false));
    }

    #[gpui::test]
    fn undo_restores_text_and_selection(cx: &mut TestAppContext) {
        let (input, cx, events) = input(cx, "abc");
        cx.simulate_input("d");
        cx.simulate_keystrokes("shift-left shift-left backspace");
        assert_eq!(state(&input, cx).0, "ab");
        events.borrow_mut().clear();
        cx.simulate_keystrokes("cmd-z");
        assert_eq!(state(&input, cx), ("abcd".into(), 2..4, false));
        assert_eq!(*events.borrow(), ["Changed"]);
        cx.simulate_keystrokes("cmd-z");
        assert_eq!(state(&input, cx), ("abc".into(), 3..3, false));
        // An empty undo stack does nothing.
        events.borrow_mut().clear();
        cx.simulate_keystrokes("cmd-z");
        assert_eq!(state(&input, cx).0, "abc");
        assert!(events.borrow().is_empty());
    }

    #[gpui::test]
    fn undo_history_is_bounded(cx: &mut TestAppContext) {
        let (input, cx, _) = input(cx, "");
        for _ in 0..120 {
            cx.simulate_input("x");
        }
        input.read_with(cx, |i, _| assert_eq!(i.undo.len(), 100));
        for _ in 0..150 {
            cx.simulate_keystrokes("cmd-z");
        }
        // The oldest 20 states are gone.
        assert_eq!(state(&input, cx).0, "x".repeat(20));
    }

    #[gpui::test]
    fn submit_cancel_and_blur_events(cx: &mut TestAppContext) {
        let (input, cx, events) = input(cx, "x");
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        cx.simulate_keystrokes("enter escape");
        assert_eq!(*events.borrow(), ["Submit", "Cancel"]);
        cx.update(|window, _| {
            window.blur();
            window.refresh();
        });
        cx.run_until_parked();
        assert_eq!(events.borrow().last().map(String::as_str), Some("Blur"));
        assert_eq!(state(&input, cx).0, "x");
    }

    #[gpui::test]
    fn set_text_selects_all(cx: &mut TestAppContext) {
        let (input, cx, _) = input(cx, "");
        input.update(cx, |i, cx| i.set_text("new name", cx));
        assert_eq!(state(&input, cx), ("new name".into(), 0..8, false));
        cx.simulate_input("z");
        assert_eq!(state(&input, cx).0, "z");
    }

    #[gpui::test]
    fn ime_ranges_are_utf16(cx: &mut TestAppContext) {
        let (input, cx, events) = input(cx, "a😀b");
        input.update_in(cx, |i, window, cx| {
            // UTF-8: a=0, 😀=1..5, b=5; UTF-16: a=0, 😀=1..3, b=3.
            let mut actual = None;
            assert_eq!(i.text_for_range(1..3, &mut actual, window, cx).as_deref(), Some("😀"));
            assert_eq!(actual, Some(1..3));
            let sel = i.selected_text_range(false, window, cx).unwrap();
            assert_eq!(sel.range, 4..4);
            assert!(!sel.reversed);
            assert_eq!(i.marked_text_range(window, cx), None);

            // Composition: mark "ka" in place of the selection, then commit "か".
            i.replace_and_mark_text_in_range(None, "ka", None, window, cx);
            assert_eq!(i.content, "a😀bka");
            assert_eq!(i.marked_text_range(window, cx), Some(4..6));
            assert_eq!(i.selected, 8..8);
            i.replace_and_mark_text_in_range(None, "か", None, window, cx);
            assert_eq!(i.content, "a😀bか");
            assert_eq!(i.marked, Some(6..9));
            assert_eq!(i.selected, 9..9);
            i.replace_text_in_range(None, "か", window, cx);
            assert_eq!(i.content, "a😀bか");
            assert_eq!(i.marked, None);
            assert_eq!(i.selected, 9..9);

            // An explicit UTF-16 range replaces exactly that text.
            i.replace_text_in_range(Some(1..3), "x", window, cx);
            assert_eq!(i.content, "axbか");

            // Unmark keeps the text.
            i.replace_and_mark_text_in_range(Some(0..0), "z", None, window, cx);
            assert_eq!(i.marked_text_range(window, cx), Some(0..1));
            i.unmark_text(window, cx);
            assert_eq!(i.marked_text_range(window, cx), None);
            assert_eq!(i.content, "zaxbか");
            // Marking empty text clears the mark.
            i.replace_and_mark_text_in_range(Some(0..1), "", None, window, cx);
            assert_eq!(i.marked, None);
            assert_eq!(i.content, "axbか");
        });
        assert!(events.borrow().iter().all(|e| e == "Changed"));
    }

    #[gpui::test]
    fn ime_selection_inside_marked_text(cx: &mut TestAppContext) {
        // The IME's selection is relative to the marked text.
        let (input, cx, _) = input(cx, "ab");
        input.update_in(cx, |i, window, cx| {
            i.replace_and_mark_text_in_range(Some(1..1), "xyz", Some(1..2), window, cx);
            assert_eq!(i.content, "axyzb");
            assert_eq!(i.marked, Some(1..4));
            assert_eq!(i.selected, 2..3);
            // The next composition step replaces the marked text, not the selection.
            i.replace_and_mark_text_in_range(None, "q", Some(1..1), window, cx);
            assert_eq!(i.content, "aqb");
            assert_eq!(i.selected, 2..2);
        });
    }

    #[gpui::test]
    fn ime_selection_with_non_ascii_text(cx: &mut TestAppContext) {
        // Non-ASCII before the mark: the selection is offset by the mark's byte start.
        let (input, cx, _) = input(cx, "é");
        input.update_in(cx, |i, window, cx| {
            i.replace_and_mark_text_in_range(None, "ab", Some(1..2), window, cx);
            assert_eq!(i.content, "éab");
            assert_eq!(i.marked, Some(2..4));
            assert_eq!(i.selected, 3..4);
            assert_eq!(&i.content[i.selected.clone()], "b");
            // Non-ASCII inside the mark: UTF-16 offsets map to char boundaries of the new text.
            i.replace_and_mark_text_in_range(None, "か😀な", Some(1..3), window, cx);
            assert_eq!(i.content, "éか😀な");
            assert_eq!(&i.content[i.selected.clone()], "😀");
            // A selection past the end of the marked text clamps to its end.
            i.replace_and_mark_text_in_range(None, "x", Some(5..9), window, cx);
            assert_eq!(i.selected, 3..3);
        });
    }

    #[gpui::test]
    fn marked_text_renders_underlined_run(cx: &mut TestAppContext) {
        let (input, cx, _) = input(cx, "ab");
        input.update_in(cx, |i, window, cx| {
            i.replace_and_mark_text_in_range(Some(1..1), "x", None, window, cx);
        });
        cx.run_until_parked();
        input.read_with(cx, |i, _| {
            assert_eq!(i.content, "axb");
            assert_eq!(i.last_layout.as_ref().map(|l| l.len), Some(3));
        });
    }

    #[gpui::test]
    fn geometry_queries(cx: &mut TestAppContext) {
        let (input, cx, _) = input(cx, "abcd");
        input.update_in(cx, |i, window, cx| {
            let bounds = i.last_bounds.unwrap();
            let layout = i.last_layout.clone().unwrap();
            let b = i.bounds_for_range(1..3, bounds, window, cx).unwrap();
            assert_eq!(b.left(), bounds.left() + layout.x_for_index(1));
            assert_eq!(b.right(), bounds.left() + layout.x_for_index(3));
            // Inside the field: the character under the point.
            for idx in 0..4 {
                let p = point(bounds.left() + layout.x_for_index(idx) + px(1.), bounds.top() + px(2.));
                assert_eq!(i.character_index_for_point(p, window, cx), Some(idx));
            }
            // Outside the field: no index.
            assert_eq!(i.character_index_for_point(point(px(-50.), px(-50.)), window, cx), None);
        });
    }

    #[gpui::test]
    fn geometry_needs_a_layout(cx: &mut TestAppContext) {
        crate::test_support::init(cx);
        let cx = cx.add_empty_window();
        let input = cx.update(|_, cx| cx.new(|cx| TextInput::new(cx, "")));
        input.update_in(cx, |i, window, cx| {
            let b = Bounds::new(point(px(0.), px(0.)), size(px(10.), px(10.)));
            assert_eq!(i.bounds_for_range(0..0, b, window, cx), None);
            assert_eq!(i.character_index_for_point(point(px(1.), px(1.)), window, cx), None);
            assert_eq!(i.index_for_position(point(px(1.), px(1.))), 0);
        });
    }

    #[gpui::test]
    fn mouse_places_caret_and_selects(cx: &mut TestAppContext) {
        let (input, cx, _) = input(cx, "hello world");
        cx.update(|window, _| window.blur());
        let (bounds, layout) =
            input.read_with(cx, |i, _| (i.last_bounds.unwrap(), i.last_layout.clone().unwrap()));
        let at = |idx: usize| {
            point(bounds.left() + layout.x_for_index(idx), bounds.top() + bounds.size.height / 2.)
        };

        cx.simulate_click(at(3), Modifiers::none());
        assert_eq!(state(&input, cx).1, 3..3);
        assert!(cx.update(|window, cx| input.read(cx).focus.is_focused(window)), "a click focuses");

        // Shift-click extends from the caret.
        cx.simulate_click(at(8), Modifiers::shift());
        assert_eq!(state(&input, cx).1, 3..8);

        // Drag selects.
        cx.simulate_mouse_down(at(1), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(at(5), Some(MouseButton::Left), Modifiers::none());
        cx.simulate_mouse_up(at(5), MouseButton::Left, Modifiers::none());
        assert_eq!(state(&input, cx).1, 1..5);
        // Moving without a pressed button after mouse up doesn't change the selection.
        cx.simulate_mouse_move(at(9), None, Modifiers::none());
        assert_eq!(state(&input, cx).1, 1..5);

        // Below the field: the end; above: the start.
        input.read_with(cx, |i, _| {
            assert_eq!(i.index_for_position(point(at(2).x, bounds.bottom() + px(5.))), 11);
            assert_eq!(i.index_for_position(point(at(2).x, bounds.top() - px(5.))), 0);
        });

        // Double click selects everything.
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at(2),
            modifiers: Modifiers::none(),
            click_count: 2,
            first_mouse: false,
        });
        assert_eq!(state(&input, cx).1, 0..11);
    }

    #[gpui::test]
    fn placeholder_renders_when_empty(cx: &mut TestAppContext) {
        let (input, cx, _) = input(cx, "");
        input.read_with(cx, |i, _| {
            assert_eq!(i.text(), "");
            assert_eq!(i.last_layout.as_ref().map(|l| l.len), Some("Placeholder".len()));
        });
        input.update(cx, |i, cx| i.set_text("all", cx));
        cx.run_until_parked();
        input.read_with(cx, |i, _| assert_eq!(i.last_layout.as_ref().map(|l| l.len), Some(3)));
    }

    #[gpui::test]
    fn root_smoke(cx: &mut TestAppContext) {
        crate::test_support::init(cx);
        let (root, cx) =
            cx.add_window_view(|window, cx| crate::root::Root::new(crate::root::Initial::Fresh, window, cx));
        cx.run_until_parked();
        root.read_with(cx, |r, _| {
            assert_eq!(r.ws.list.len(), 1);
            assert_eq!(r.panes.len(), 1);
        });
    }
}
