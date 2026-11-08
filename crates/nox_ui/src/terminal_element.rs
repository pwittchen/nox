//! The terminal grid as a custom GPUI element (SPEC §7.2).
//!
//! 1. In prepaint, compute the grid size from the bounds (resizing the `Term` in the same frame),
//!    lock the `Term` briefly and copy the visible cells (plus one extra row above for the
//!    sub-line scroll offset) into a frame buffer, then release the lock.
//! 2. Merge adjacent cells with the same background into quads, and adjacent cells with the same
//!    style into text runs, shaped by GPUI's text system and snapped to the cell grid. GPUI
//!    caches shaped lines between frames, so unchanged rows are not re-shaped.
//! 3. Paint backgrounds, selection / search highlights, text, decorations, cursor, link hover,
//!    IME marked text and the overlay scrollbar.

use std::time::Instant;

use gpui::{
    App, Bounds, ContentMask, Element, ElementId, ElementInputHandler, Entity, Font, FontFeatures, FontStyle,
    FontWeight, GlobalElementId, Hsla, LayoutId, Pixels, Point, ShapedLine, SharedString, Size,
    StrikethroughStyle, Style, TextRun, UnderlineStyle, Window, fill, point, prelude::*, px, quad, relative,
    size, transparent_black,
};
use nox_terminal::GridSize;
use nox_terminal::alacritty_terminal::grid::Dimensions;
use nox_terminal::alacritty_terminal::index::{Column, Line, Point as GridPoint};
use nox_terminal::alacritty_terminal::term::TermMode;
use nox_terminal::alacritty_terminal::term::cell::Flags;
use nox_terminal::alacritty_terminal::term::color::Colors;
use nox_terminal::alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};

use crate::pane::{GridGeom, Pane};
use crate::settings_state::Settings;
use crate::theme::{Theme, to_hsla};

/// Terminal font settings resolved from the config.
#[derive(Clone)]
pub struct TermFont {
    pub family: SharedString,
    pub fallbacks: Vec<String>,
    pub size: f32,
    pub line_height: f32,
    pub letter_spacing: f32,
    pub weight: FontWeight,
    pub weight_bold: FontWeight,
}

impl TermFont {
    #[must_use]
    pub fn font(&self, bold: bool, italic: bool) -> Font {
        Font {
            family: self.family.clone(),
            // The grid is drawn cell by cell: no ligatures (SPEC §2).
            features: FontFeatures(std::sync::Arc::new(vec![("calt".into(), 0), ("liga".into(), 0)])),
            fallbacks: if self.fallbacks.is_empty() {
                None
            } else {
                Some(gpui::FontFallbacks::from_fonts(self.fallbacks.clone()))
            },
            weight: if bold { self.weight_bold } else { self.weight },
            style: if italic { FontStyle::Italic } else { FontStyle::Normal },
        }
    }
}

/// Cell metrics for a font at a size: width from the advance of `0`, height from the font's
/// natural line height (ascent + descent) × `line_height` — like iTerm's line spacing
/// percentage — rounded to whole pixels so rows never blur.
#[must_use]
pub fn cell_metrics(window: &Window, font: &TermFont, size: f32) -> (Pixels, Pixels) {
    let ts = window.text_system();
    let f = font.font(false, false);
    let id = ts.resolve_font(&f);
    let sz = px(size);
    let w = ts.advance(id, sz, '0').map_or(px(size * 0.6), |s| s.width);
    let w = w + px(font.letter_spacing);
    let natural = f32::from(ts.ascent(id, sz)) + f32::from(ts.descent(id, sz)).abs();
    let natural = if natural > 0.0 { natural } else { size * 1.2 };
    let h = (natural * font.line_height).round().max(1.0);
    (w, px(h))
}

pub struct TerminalElement {
    pane: Entity<Pane>,
    focused: bool,
    window_active: bool,
}

impl TerminalElement {
    #[must_use]
    pub fn new(pane: Entity<Pane>, focused: bool, window_active: bool) -> Self {
        Self { pane, focused, window_active }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct RunStyle {
    fg: Hsla,
    bold: bool,
    italic: bool,
    underline: Option<(Hsla, bool /*wavy*/)>,
    strike: bool,
}

struct TextSeg {
    col: usize,
    row: usize,
    line: ShapedLine,
}

struct Quad {
    bounds: Bounds<Pixels>,
    color: Hsla,
}

pub struct Prepaint {
    geom: GridGeom,
    bg_quads: Vec<Quad>,
    highlight_quads: Vec<Quad>,
    texts: Vec<TextSeg>,
    deco_quads: Vec<Quad>,
    cursor: Option<CursorPaint>,
    marked: Option<(Point<Pixels>, ShapedLine, Pixels)>,
    scrollbar: Option<Quad>,
    new_output_pill: bool,
}

struct CursorPaint {
    bounds: Bounds<Pixels>,
    shape: CursorShape,
    hollow: bool,
    color: Hsla,
    text: Option<ShapedLine>,
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

fn resolve_color(c: Color, colors: &Colors, theme: &Theme, bold_bright: bool) -> Hsla {
    match c {
        Color::Spec(rgb) => Hsla::from(gpui::Rgba {
            r: f32::from(rgb.r) / 255.,
            g: f32::from(rgb.g) / 255.,
            b: f32::from(rgb.b) / 255.,
            a: 1.,
        }),
        Color::Indexed(i) => {
            let mut idx = i as usize;
            if bold_bright && idx < 8 {
                idx += 8;
            }
            indexed(idx, colors, theme)
        }
        Color::Named(n) => named(n, colors, theme, bold_bright),
    }
}

fn indexed(idx: usize, colors: &Colors, theme: &Theme) -> Hsla {
    if let Some(rgb) = colors[idx] {
        return Hsla::from(gpui::Rgba {
            r: f32::from(rgb.r) / 255.,
            g: f32::from(rgb.g) / 255.,
            b: f32::from(rgb.b) / 255.,
            a: 1.,
        });
    }
    theme.palette[idx.min(255)]
}

fn named(n: NamedColor, colors: &Colors, theme: &Theme, bold_bright: bool) -> Hsla {
    let over = |i: usize| {
        colors[i].map(|rgb| {
            Hsla::from(gpui::Rgba {
                r: f32::from(rgb.r) / 255.,
                g: f32::from(rgb.g) / 255.,
                b: f32::from(rgb.b) / 255.,
                a: 1.,
            })
        })
    };
    let i = n as usize;
    match n {
        NamedColor::Foreground | NamedColor::BrightForeground => over(256).unwrap_or(theme.text),
        NamedColor::Background => over(257).unwrap_or(theme.bg),
        NamedColor::Cursor => over(258).unwrap_or(theme.caret),
        NamedColor::DimForeground => {
            let c = over(256).unwrap_or(theme.text);
            c.opacity(0.66)
        }
        _ if (259..=266).contains(&i) => {
            // Dim ANSI colors.
            indexed(i - 259, colors, theme).opacity(0.66)
        }
        _ => {
            let mut idx = i.min(255);
            if bold_bright && idx < 8 {
                idx += 8;
            }
            indexed(idx, colors, theme)
        }
    }
}

fn hsla_rgba(h: Hsla) -> nox_theme::Rgba {
    let r: gpui::Rgba = h.into();
    nox_theme::Rgba { r: r.r, g: r.g, b: r.b, a: r.a }
}

impl Element for TerminalElement {
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
        style.size.height = relative(1.).into();
        style.flex_grow = 1.;
        style.min_size.height = px(0.).into();
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
        let settings = Settings::global(cx).clone();
        let theme = Theme::global(cx).clone();
        let cfg = &settings.config;
        let font = settings.term_font.clone();

        // Advance a running wheel animation.
        let now = Instant::now();
        self.pane.update(cx, |pane, cx| pane.step_scroll_animation(now, window, cx));

        let pane = self.pane.read(cx);
        let font_size = pane.font_size(&settings);
        let (cell_w, cell_h) = cell_metrics(window, &font, font_size);
        let pad_x = px(cfg.terminal.padding[0] as f32);
        let pad_y = px(cfg.terminal.padding[1] as f32);
        let inner_w = (bounds.size.width - pad_x * 2.).max(px(0.));
        let inner_h = (bounds.size.height - pad_y * 2.).max(px(0.));
        let cols = ((inner_w / cell_w).floor() as usize).max(2);
        let rows = ((inner_h / cell_h).floor() as usize).max(1);
        let origin = point(bounds.origin.x + pad_x, bounds.origin.y + pad_y);
        let geom = GridGeom { bounds, origin, cell_w, cell_h, cols, rows };

        let Some(session) = pane.session.as_ref() else {
            return Prepaint {
                geom,
                bg_quads: vec![],
                highlight_quads: vec![],
                texts: vec![],
                deco_quads: vec![],
                cursor: None,
                marked: None,
                scrollbar: None,
                new_output_pill: false,
            };
        };

        // Resize the Term in this frame; the PTY resize is coalesced (SPEC §7.4).
        session.resize(GridSize {
            cols: cols as u16,
            rows: rows as u16,
            cell_w: f32::from(cell_w).round() as u16,
            cell_h: f32::from(cell_h).round() as u16,
        });

        let frac = px(pane.scroll.frac);
        let hovered = pane.hovered_link.clone();
        let search_matches = pane.find_highlights();
        let marked_text = pane.marked_text.clone();
        let cursor_on = pane.cursor_visible(now);
        let bold_bright = cfg.terminal.bold_is_bright;
        let min_contrast = cfg.terminal.minimum_contrast as f32;
        let scroll_alpha = pane.scroll.scrollbar_alpha(now);
        let new_output = pane.new_output_below;

        let text_system = window.text_system().clone();
        let fsz = px(font_size);

        let mut bg_quads = Vec::new();
        let mut highlight_quads = Vec::new();
        let mut texts = Vec::new();
        let mut deco_quads = Vec::new();
        let mut cursor = None;
        let mut scrollbar = None;

        {
            let term = session.term().lock();
            let grid = term.grid();
            let offset = grid.display_offset() as i32;
            let history = grid.history_size() as i32;
            let screen = term.screen_lines() as i32;
            let tcols = term.columns();
            let colors = term.colors();
            let mode = *term.mode();
            let selection = term.selection.as_ref().and_then(|s| s.to_range(&term));

            // Viewport rows to draw: an extra row above when there is a sub-line offset.
            let extra = i32::from(frac > px(0.) && offset < history);
            let first_vrow: i32 = -extra;
            for vrow in first_vrow..screen.min(rows as i32) {
                let line = Line(vrow - offset);
                if line.0 < -history || line.0 >= screen {
                    continue;
                }
                let y = origin.y + cell_h * vrow as f32 + frac;
                let row = &grid[line];

                // Backgrounds.
                let mut run_start = 0usize;
                let mut run_color: Option<Hsla> = None;
                let flush_bg = |start: usize, end: usize, color: Option<Hsla>, quads: &mut Vec<Quad>| {
                    if let Some(c) = color
                        && end > start
                    {
                        quads.push(Quad {
                            bounds: Bounds::new(
                                point(origin.x + cell_w * start as f32, y),
                                size(cell_w * (end - start) as f32, cell_h),
                            ),
                            color: c,
                        });
                    }
                };

                // Text segments.
                let mut seg_text = String::new();
                let mut seg_start = 0usize;
                let mut seg_style: Option<RunStyle> = None;
                let mut seg_len_cells = 0usize;

                let flush_text = |text: &mut String,
                                  start: usize,
                                  style: Option<RunStyle>,
                                  texts: &mut Vec<TextSeg>,
                                  force: bool| {
                    let trimmed_len = text.trim_end_matches(' ').len();
                    if let Some(st) = style
                        && trimmed_len > 0
                    {
                        text.truncate(trimmed_len);
                        let run = TextRun {
                            len: text.len(),
                            font: font.font(st.bold, st.italic),
                            color: st.fg,
                            background_color: None,
                            underline: st.underline.map(|(c, wavy)| UnderlineStyle {
                                color: Some(c),
                                thickness: px(1.),
                                wavy,
                            }),
                            strikethrough: st
                                .strike
                                .then_some(StrikethroughStyle { color: Some(st.fg), thickness: px(1.) }),
                        };
                        let shaped = text_system.shape_line(
                            SharedString::from(std::mem::take(text)),
                            fsz,
                            &[run],
                            if force { Some(cell_w) } else { None },
                        );
                        texts.push(TextSeg { col: start, row: (vrow - first_vrow) as usize, line: shaped });
                    }
                    text.clear();
                };

                for col in 0..tcols.min(cols) {
                    let cell = &row[Column(col)];
                    let flags = cell.flags;
                    if flags.contains(Flags::WIDE_CHAR_SPACER)
                        || flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
                    {
                        // Covered by the wide char to its left (its bg run extends here).
                        continue;
                    }
                    let mut fg =
                        resolve_color(cell.fg, colors, &theme, bold_bright && flags.contains(Flags::BOLD));
                    let mut bg = resolve_color(cell.bg, colors, &theme, false);
                    let mut bg_default = matches!(cell.bg, Color::Named(NamedColor::Background));
                    if flags.contains(Flags::INVERSE) {
                        std::mem::swap(&mut fg, &mut bg);
                        bg_default = false;
                    }
                    if flags.contains(Flags::DIM) {
                        fg = fg.opacity(0.66);
                    }
                    if flags.contains(Flags::HIDDEN) {
                        fg = bg;
                    }
                    if min_contrast > 1.0 {
                        let fixed = nox_theme::ensure_contrast(hsla_rgba(fg), hsla_rgba(bg), min_contrast);
                        fg = to_hsla(fixed);
                    }
                    let width = if flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };

                    // Background run.
                    let this_bg = if bg_default { None } else { Some(bg) };
                    if this_bg != run_color {
                        flush_bg(run_start, col, run_color, &mut bg_quads);
                        run_start = col;
                        run_color = this_bg;
                    }
                    if width == 2 && col + 1 == tcols.min(cols) {
                        // fallthrough
                    }

                    // Text.
                    let underline = if flags.intersects(Flags::ALL_UNDERLINES) {
                        let uc =
                            cell.underline_color().map_or(fg, |c| resolve_color(c, colors, &theme, false));
                        Some((uc, flags.contains(Flags::UNDERCURL)))
                    } else {
                        None
                    };
                    let style = RunStyle {
                        fg,
                        bold: flags.contains(Flags::BOLD),
                        italic: flags.contains(Flags::ITALIC),
                        underline,
                        strike: flags.contains(Flags::STRIKEOUT),
                    };
                    let zw = cell.zerowidth();
                    let special = width == 2 || zw.is_some() || (cell.c as u32) > 0x2fff;
                    if special || Some(style) != seg_style {
                        flush_text(&mut seg_text, seg_start, seg_style, &mut texts, true);
                        seg_start = col;
                        seg_style = Some(style);
                        seg_len_cells = 0;
                    }
                    let ch = if cell.c == '\0' || cell.c == '\t' { ' ' } else { cell.c };
                    seg_text.push(ch);
                    if let Some(zw) = zw {
                        seg_text.extend(zw.iter());
                    }
                    seg_len_cells += width;
                    if special {
                        flush_text(&mut seg_text, seg_start, seg_style, &mut texts, false);
                        seg_style = None;
                        seg_start = col + width;
                        seg_len_cells = 0;
                    }
                    let _ = seg_len_cells;
                }
                flush_bg(run_start, tcols.min(cols), run_color, &mut bg_quads);
                flush_text(&mut seg_text, seg_start, seg_style, &mut texts, true);

                // Selection on this line.
                if let Some(sel) = &selection {
                    let (start, end) = (sel.start, sel.end);
                    if line >= start.line && line <= end.line {
                        let (c0, c1) = if sel.is_block {
                            (start.column.0.min(end.column.0), start.column.0.max(end.column.0))
                        } else {
                            let c0 = if line == start.line { start.column.0 } else { 0 };
                            let c1 = if line == end.line { end.column.0 } else { tcols - 1 };
                            (c0, c1)
                        };
                        highlight_quads.push(Quad {
                            bounds: Bounds::new(
                                point(origin.x + cell_w * c0 as f32, y),
                                size(cell_w * (c1 + 1 - c0) as f32, cell_h),
                            ),
                            color: theme.selection,
                        });
                    }
                }

                // Search matches on this line.
                for (i, m) in search_matches.iter().enumerate() {
                    let current = Some(i) == pane.find_current_index();
                    let (s, e) = (m.start(), m.end());
                    if line < s.line || line > e.line {
                        continue;
                    }
                    let c0 = if line == s.line { s.column.0 } else { 0 };
                    let c1 = if line == e.line { e.column.0 } else { tcols - 1 };
                    let color = if current { theme.accent.opacity(0.45) } else { theme.selection };
                    highlight_quads.push(Quad {
                        bounds: Bounds::new(
                            point(origin.x + cell_w * c0 as f32, y),
                            size(cell_w * (c1 + 1 - c0) as f32, cell_h),
                        ),
                        color,
                    });
                }

                // Hovered link underline.
                if let Some(link) = &hovered {
                    for (lline, c0, c1) in &link.spans {
                        if *lline == line {
                            deco_quads.push(Quad {
                                bounds: Bounds::new(
                                    point(origin.x + cell_w * *c0 as f32, y + cell_h - px(2.)),
                                    size(cell_w * (*c1 + 1 - *c0) as f32, px(1.)),
                                ),
                                color: theme.accent,
                            });
                        }
                    }
                }
            }

            // Cursor.
            let cpoint = grid.cursor.point;
            let shape = if mode.contains(TermMode::SHOW_CURSOR) {
                term.cursor_style().shape
            } else {
                CursorShape::Hidden
            };
            let vrow = cpoint.line.0 + offset;
            if shape != CursorShape::Hidden && vrow >= 0 && vrow < rows as i32 && cpoint.column.0 < cols {
                let cell = &grid[GridPoint::new(cpoint.line, cpoint.column)];
                let wide = cell.flags.contains(Flags::WIDE_CHAR);
                let cw = if wide { cell_w * 2. } else { cell_w };
                let cb = Bounds::new(
                    point(origin.x + cell_w * cpoint.column.0 as f32, origin.y + cell_h * vrow as f32 + frac),
                    size(cw, cell_h),
                );
                let hollow = !self.focused || !self.window_active;
                let color = colors[NamedColor::Cursor as usize].map_or(theme.caret, |rgb| {
                    Hsla::from(gpui::Rgba {
                        r: f32::from(rgb.r) / 255.,
                        g: f32::from(rgb.g) / 255.,
                        b: f32::from(rgb.b) / 255.,
                        a: 1.,
                    })
                });
                let text = if shape == CursorShape::Block && !hollow && cell.c != ' ' && cell.c != '\0' {
                    let s: String = std::iter::once(cell.c)
                        .chain(cell.zerowidth().unwrap_or(&[]).iter().copied())
                        .collect();
                    let run = TextRun {
                        len: s.len(),
                        font: font.font(cell.flags.contains(Flags::BOLD), cell.flags.contains(Flags::ITALIC)),
                        color: theme.caret_text,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    Some(text_system.shape_line(s.into(), fsz, &[run], None))
                } else {
                    None
                };
                if cursor_on || hollow {
                    cursor = Some(CursorPaint { bounds: cb, shape, hollow, color, text });
                }
            }

            // Overlay scrollbar.
            if scroll_alpha > 0.0 && history > 0 {
                let total = (history + screen) as f32;
                let h = bounds.size.height;
                let thumb_h = (h * (screen as f32 / total)).max(px(24.));
                let top_frac = (history - offset) as f32 / total;
                let y = bounds.origin.y
                    + (h - thumb_h) * (top_frac / ((history as f32) / total).max(1e-6)).min(1.0);
                scrollbar = Some(Quad {
                    bounds: Bounds::new(point(bounds.right() - px(8.), y), size(px(6.), thumb_h)),
                    color: theme.text_muted.opacity(0.4 * scroll_alpha),
                });
            }
        }

        // IME marked text at the cursor.
        let marked = marked_text.filter(|t| !t.is_empty()).and_then(|t| {
            let c = cursor.as_ref()?;
            let run = TextRun {
                len: t.len(),
                font: font.font(false, false),
                color: theme.text,
                background_color: Some(theme.bg),
                underline: Some(UnderlineStyle { color: Some(theme.accent), thickness: px(1.), wavy: false }),
                strikethrough: None,
            };
            let shaped = text_system.shape_line(t.into(), fsz, &[run], None);
            Some((c.bounds.origin, shaped, cell_h))
        });

        self.pane.update(cx, |p, _| p.geom = Some(geom.clone()));

        Prepaint {
            geom,
            bg_quads,
            highlight_quads,
            texts,
            deco_quads,
            cursor,
            marked,
            scrollbar,
            new_output_pill: new_output,
        }
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
        let focus = self.pane.read(cx).focus_handle.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.pane.clone()), cx);

        let geom = pre.geom.clone();
        let theme = Theme::global(cx).clone();
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for q in pre.bg_quads.drain(..) {
                window.paint_quad(fill(q.bounds, q.color));
            }
            for q in pre.highlight_quads.drain(..) {
                window.paint_quad(fill(q.bounds, q.color));
            }
            // Rows are counted from the first drawn row, which is the extra one above the
            // viewport when there is a sub-line offset.
            let frac = px(self.pane.read(cx).scroll.frac);
            let extra = if frac > px(0.) { 1. } else { 0. };
            for seg in pre.texts.drain(..) {
                let x = geom.origin.x + geom.cell_w * seg.col as f32;
                let y = geom.origin.y + geom.cell_h * (seg.row as f32 - extra) + frac;
                let _ = seg.line.paint(point(x, y), geom.cell_h, window, cx);
            }
            for q in pre.deco_quads.drain(..) {
                window.paint_quad(fill(q.bounds, q.color));
            }
            if let Some(c) = pre.cursor.take() {
                let b = c.bounds;
                if c.hollow {
                    window.paint_quad(quad(
                        b,
                        px(0.),
                        transparent_black(),
                        px(1.),
                        c.color,
                        gpui::BorderStyle::default(),
                    ));
                } else {
                    match c.shape {
                        CursorShape::Beam => window
                            .paint_quad(fill(Bounds::new(b.origin, size(px(2.), b.size.height)), c.color)),
                        CursorShape::Underline => window.paint_quad(fill(
                            Bounds::new(point(b.origin.x, b.bottom() - px(2.)), size(b.size.width, px(2.))),
                            c.color,
                        )),
                        CursorShape::HollowBlock => window.paint_quad(quad(
                            b,
                            px(0.),
                            transparent_black(),
                            px(1.),
                            c.color,
                            gpui::BorderStyle::default(),
                        )),
                        _ => {
                            window.paint_quad(fill(b, c.color));
                            if let Some(t) = c.text {
                                let _ = t.paint(b.origin, geom.cell_h, window, cx);
                            }
                        }
                    }
                }
            }
            if let Some((origin, line, h)) = pre.marked.take() {
                window.paint_quad(fill(Bounds::new(origin, size(line.width, h)), theme.bg));
                let _ = line.paint(origin, h, window, cx);
            }
            if let Some(q) = pre.scrollbar.take() {
                window.paint_quad(quad(
                    q.bounds,
                    px(3.),
                    q.color,
                    px(0.),
                    transparent_black(),
                    gpui::BorderStyle::default(),
                ));
            }
        });
        let _ = pre.new_output_pill;
    }
}

/// Convert a window position to a grid point (viewport-relative line adjusted by the display
/// offset) and the side of the cell it falls on.
#[must_use]
pub fn position_to_point(
    geom: &GridGeom,
    pos: Point<Pixels>,
    frac: f32,
    display_offset: usize,
) -> (GridPoint, nox_terminal::alacritty_terminal::index::Side) {
    use nox_terminal::alacritty_terminal::index::Side;
    let x = f32::from(pos.x - geom.origin.x).max(0.0);
    let y = f32::from(pos.y - geom.origin.y) - frac;
    let cw = f32::from(geom.cell_w);
    let ch = f32::from(geom.cell_h);
    let col = ((x / cw) as usize).min(geom.cols.saturating_sub(1));
    let row = (y / ch).floor() as i32;
    let row = row.clamp(-1, geom.rows as i32 - 1);
    let side = if (x % cw) < cw / 2. { Side::Left } else { Side::Right };
    (GridPoint::new(Line(row - display_offset as i32), Column(col)), side)
}

#[must_use]
pub fn grid_size_in(bounds: Size<Pixels>, cell: (Pixels, Pixels), pad: (f32, f32)) -> (usize, usize) {
    let w = (bounds.width - px(pad.0 * 2.)).max(px(0.));
    let h = (bounds.height - px(pad.1 * 2.)).max(px(0.));
    (((w / cell.0).floor() as usize).max(2), ((h / cell.1).floor() as usize).max(1))
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext, rgb};
    use nox_terminal::alacritty_terminal::index::Side;
    use nox_terminal::alacritty_terminal::selection::{Selection, SelectionType};
    use nox_terminal::alacritty_terminal::vte::ansi::Rgb;

    use super::*;
    use crate::pane::tests::{cell, feed, geom, open, silent};
    use crate::pane::{HoveredLink, LinkTarget, PaneSpawn};

    fn rgb_hsla(r: u8, g: u8, b: u8) -> Hsla {
        Hsla::from(rgb((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)))
    }

    fn close(a: Hsla, b: Hsla) -> bool {
        let (a, b): (gpui::Rgba, gpui::Rgba) = (a.into(), b.into());
        (a.r - b.r).abs() < 0.01
            && (a.g - b.g).abs() < 0.01
            && (a.b - b.b).abs() < 0.01
            && (a.a - b.a).abs() < 0.01
    }

    // ---- pure helpers ---------------------------------------------------------------------

    #[gpui::test]
    fn colors_resolve_from_overrides_then_theme(cx: &mut TestAppContext) {
        crate::test_support::init(cx);
        cx.update(|cx| {
            let theme = Theme::global(cx).clone();
            let mut colors = Colors::default();
            // Truecolor.
            assert!(close(
                resolve_color(Color::Spec(Rgb { r: 255, g: 0, b: 0 }), &colors, &theme, false),
                rgb_hsla(255, 0, 0)
            ));
            // Palette, with bold-is-bright for the first 8.
            assert_eq!(resolve_color(Color::Indexed(1), &colors, &theme, false), theme.palette[1]);
            assert_eq!(resolve_color(Color::Indexed(1), &colors, &theme, true), theme.palette[9]);
            assert_eq!(resolve_color(Color::Indexed(200), &colors, &theme, true), theme.palette[200]);
            assert_eq!(resolve_color(Color::Named(NamedColor::Red), &colors, &theme, true), theme.palette[9]);
            assert_eq!(
                resolve_color(Color::Named(NamedColor::BrightRed), &colors, &theme, false),
                theme.palette[9]
            );
            // Special colors default to the theme.
            assert_eq!(
                resolve_color(Color::Named(NamedColor::Foreground), &colors, &theme, false),
                theme.text
            );
            assert_eq!(
                resolve_color(Color::Named(NamedColor::BrightForeground), &colors, &theme, false),
                theme.text
            );
            assert_eq!(resolve_color(Color::Named(NamedColor::Background), &colors, &theme, false), theme.bg);
            assert_eq!(resolve_color(Color::Named(NamedColor::Cursor), &colors, &theme, false), theme.caret);
            assert_eq!(
                resolve_color(Color::Named(NamedColor::DimForeground), &colors, &theme, false),
                theme.text.opacity(0.66)
            );
            assert_eq!(
                resolve_color(Color::Named(NamedColor::DimGreen), &colors, &theme, false),
                theme.palette[2].opacity(0.66)
            );
            // Programs can override palette entries and special colors (OSC 4 / 10 / 11 / 12).
            colors[1] = Some(Rgb { r: 0, g: 0, b: 255 });
            colors[NamedColor::Foreground] = Some(Rgb { r: 0, g: 255, b: 0 });
            colors[NamedColor::Background] = Some(Rgb { r: 10, g: 10, b: 10 });
            colors[NamedColor::Cursor] = Some(Rgb { r: 1, g: 2, b: 3 });
            assert!(close(resolve_color(Color::Indexed(1), &colors, &theme, false), rgb_hsla(0, 0, 255)));
            assert!(close(
                resolve_color(Color::Named(NamedColor::Red), &colors, &theme, false),
                rgb_hsla(0, 0, 255)
            ));
            assert!(close(
                resolve_color(Color::Named(NamedColor::Foreground), &colors, &theme, false),
                rgb_hsla(0, 255, 0)
            ));
            assert!(close(
                resolve_color(Color::Named(NamedColor::Background), &colors, &theme, false),
                rgb_hsla(10, 10, 10)
            ));
            assert!(close(
                resolve_color(Color::Named(NamedColor::Cursor), &colors, &theme, false),
                rgb_hsla(1, 2, 3)
            ));
            assert!(close(
                resolve_color(Color::Named(NamedColor::DimForeground), &colors, &theme, false),
                rgb_hsla(0, 255, 0).opacity(0.66)
            ));
            assert!(close(
                resolve_color(Color::Named(NamedColor::DimRed), &colors, &theme, false),
                rgb_hsla(0, 0, 255).opacity(0.66)
            ));
            // Round trip to the theme crate's color type.
            let c = hsla_rgba(rgb_hsla(255, 0, 0));
            assert!((c.r - 1.).abs() < 1e-3 && c.g.abs() < 1e-3 && (c.a - 1.).abs() < 1e-3);
        });
    }

    #[test]
    fn term_font_disables_ligatures_and_picks_weights() {
        let mut f = TermFont {
            family: "JetBrains Mono".into(),
            fallbacks: vec![],
            size: 14.,
            line_height: 1.,
            letter_spacing: 0.,
            weight: FontWeight::NORMAL,
            weight_bold: FontWeight::BOLD,
        };
        let regular = f.font(false, false);
        assert_eq!(regular.family.as_ref(), "JetBrains Mono");
        assert_eq!(regular.weight, FontWeight::NORMAL);
        assert_eq!(regular.style, FontStyle::Normal);
        assert!(regular.fallbacks.is_none());
        let features: Vec<_> = regular.features.tag_value_list().to_vec();
        assert!(features.contains(&("calt".into(), 0)) && features.contains(&("liga".into(), 0)));
        let bi = f.font(true, true);
        assert_eq!(bi.weight, FontWeight::BOLD);
        assert_eq!(bi.style, FontStyle::Italic);
        f.fallbacks = vec!["Menlo".into()];
        assert!(f.font(false, false).fallbacks.is_some());
    }

    fn geom_fixture() -> GridGeom {
        GridGeom {
            bounds: Bounds::new(point(px(0.), px(0.)), size(px(116.), px(212.))),
            origin: point(px(8.), px(6.)),
            cell_w: px(10.),
            cell_h: px(20.),
            cols: 10,
            rows: 10,
        }
    }

    #[test]
    fn positions_map_to_cells_and_sides() {
        let g = geom_fixture();
        let (p, side) = position_to_point(&g, point(px(8. + 22.), px(6. + 45.)), 0., 0);
        assert_eq!((p.line, p.column), (Line(2), Column(2)));
        assert_eq!(side, Side::Left);
        let (_, side) = position_to_point(&g, point(px(8. + 27.), px(6. + 45.)), 0., 0);
        assert_eq!(side, Side::Right);
        // Scrolled up 5 lines: the top row is line -5.
        let (p, _) = position_to_point(&g, point(px(8.), px(6. + 1.)), 0., 5);
        assert_eq!(p.line, Line(-5));
        // A sub-line offset shifts content down.
        let (p, _) = position_to_point(&g, point(px(8.), px(6. + 25.)), 10., 0);
        assert_eq!(p.line, Line(0));
        // Clamped to the grid (one row above for the partially visible line).
        let (p, _) = position_to_point(&g, point(px(-50.), px(-500.)), 0., 0);
        assert_eq!((p.line, p.column), (Line(-1), Column(0)));
        let (p, _) = position_to_point(&g, point(px(5000.), px(5000.)), 0., 0);
        assert_eq!((p.line, p.column), (Line(9), Column(9)));
    }

    #[test]
    fn grid_size_from_bounds() {
        let cell = (px(10.), px(20.));
        assert_eq!(grid_size_in(size(px(116.), px(212.)), cell, (8., 6.)), (10, 10));
        assert_eq!(grid_size_in(size(px(125.), px(239.)), cell, (0., 0.)), (12, 11));
        assert_eq!(grid_size_in(size(px(5.), px(5.)), cell, (8., 6.)), (2, 1), "never below 2×1");
    }

    #[gpui::test]
    fn cell_metrics_follow_font_size_and_line_height(cx: &mut TestAppContext) {
        crate::test_support::init(cx);
        let cx = cx.add_empty_window();
        cx.update(|window, cx| {
            let mut font = Settings::global(cx).term_font.clone();
            let (w, h) = cell_metrics(window, &font, 14.);
            assert!(w > px(0.) && h >= px(1.));
            assert_eq!(f32::from(h).fract(), 0., "whole pixels");
            font.line_height = 2.;
            font.letter_spacing = 1.;
            let (w2, h2) = cell_metrics(window, &font, 14.);
            assert_eq!(w2, w + px(1.));
            assert!((f32::from(h2) - 2. * f32::from(h)).abs() <= 1.);
            let (w3, _) = cell_metrics(window, &font, 28.);
            assert!(w3 > w2);
        });
    }

    // ---- prepaint -------------------------------------------------------------------------

    /// Prepaint the pane's grid at the bounds it was laid out at. (Called directly: `paint`
    /// drains the state, and `cx.draw` runs outside a view, where the scroll animation's
    /// `request_animation_frame` isn't allowed; the window's own frames cover that.)
    fn frame(pane: &Entity<Pane>, cx: &mut VisualTestContext, focused: bool) -> Prepaint {
        let b = geom(pane, cx).bounds;
        let mut el = TerminalElement::new(pane.clone(), focused, true);
        cx.update(|window, cx| el.prepaint(None, None, b, &mut (), window, cx))
    }

    /// Paint through `cx.draw` (layout, prepaint, paint) for the paint code paths.
    fn draw(pane: &Entity<Pane>, cx: &mut VisualTestContext, focused: bool) {
        let b = geom(pane, cx).bounds;
        let pane = pane.clone();
        let _ = cx.draw(b.origin, b.size, move |_, _| TerminalElement::new(pane, focused, true));
    }

    fn texts(pre: &Prepaint) -> Vec<(String, usize, usize)> {
        pre.texts.iter().map(|t| (t.line.text.to_string(), t.col, t.row)).collect()
    }

    fn theme(cx: &mut VisualTestContext) -> Theme {
        cx.update(|_, cx| Theme::global(cx).clone())
    }

    #[gpui::test]
    fn text_is_split_into_styled_runs(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        feed(
            &pane,
            cx,
            "\x1b[31mred\x1b[0m plain   \x1b[1mbold\x1b[0m\r\na中b\x1b[4mu\x1b[9ms\x1b[0m".as_bytes(),
        );
        let pre = frame(&pane, cx, true);
        let t = texts(&pre);
        assert!(t.contains(&("red".into(), 0, 0)), "{t:?}");
        assert!(t.contains(&(" plain".into(), 3, 0)), "same style joins, trailing spaces trimmed: {t:?}");
        assert!(t.contains(&("bold".into(), 12, 0)), "{t:?}");
        assert!(
            t.contains(&("a".into(), 0, 1))
                && t.contains(&("中".into(), 1, 1))
                && t.contains(&("b".into(), 3, 1)),
            "wide chars get their own run: {t:?}"
        );
        assert!(t.contains(&("u".into(), 4, 1)) && t.contains(&("s".into(), 5, 1)), "{t:?}");
        assert!(pre.bg_quads.is_empty(), "default backgrounds aren't painted");
        let g = pre.geom.clone();
        assert_eq!((g.cols, g.rows), (geom(&pane, cx).cols, geom(&pane, cx).rows));
    }

    #[gpui::test]
    fn backgrounds_merge_into_quads(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        feed(&pane, cx, b"ab\x1b[42mgreen\x1b[0m \x1b[7mX\x1b[0m \x1b[8mhidden\x1b[0m \x1b[2mdim\x1b[0m");
        let pre = frame(&pane, cx, true);
        let th = theme(cx);
        let g = pre.geom.clone();
        assert_eq!(pre.bg_quads.len(), 2, "one per background run");
        let green = &pre.bg_quads[0];
        assert_eq!(green.color, th.palette[2]);
        assert_eq!(green.bounds.origin.x, g.origin.x + g.cell_w * 2.);
        assert_eq!(green.bounds.size, size(g.cell_w * 5., g.cell_h));
        // Inverse: the foreground becomes the background.
        let inv = &pre.bg_quads[1];
        assert_eq!(inv.color, th.text);
        assert_eq!(inv.bounds.origin.x, g.origin.x + g.cell_w * 8.);
        assert!(texts(&pre).iter().any(|(s, c, _)| s == "hidden" && *c == 10), "hidden text keeps its cells");
    }

    #[gpui::test]
    fn minimum_contrast_still_draws_everything(cx: &mut TestAppContext) {
        let mut cfg = silent();
        cfg.terminal.minimum_contrast = 4.5;
        cfg.terminal.bold_is_bright = true;
        let (pane, cx) = open(cx, cfg, PaneSpawn::default());
        feed(&pane, cx, b"\x1b[1;30mdark\x1b[0m \x1b[38;2;20;20;20mtc\x1b[0m");
        let t = texts(&frame(&pane, cx, true));
        assert!(t.iter().any(|(s, _, _)| s == "dark") && t.iter().any(|(s, _, _)| s == "tc"), "{t:?}");
    }

    #[gpui::test]
    fn cursor_follows_mode_shape_and_focus(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        feed(&pane, cx, b"ab\x1b[D");
        let g = geom(&pane, cx);
        let pre = frame(&pane, cx, true);
        let c = pre.cursor.as_ref().expect("cursor");
        assert_eq!(c.shape, CursorShape::Block);
        assert!(!c.hollow);
        assert_eq!(c.bounds, Bounds::new(point(g.origin.x + g.cell_w, g.origin.y), size(g.cell_w, g.cell_h)));
        assert_eq!(
            c.text.as_ref().map(|t| t.text.to_string()).as_deref(),
            Some("b"),
            "the glyph under a block cursor"
        );
        assert_eq!(c.color, theme(cx).caret);

        let pre = frame(&pane, cx, false);
        let c = pre.cursor.as_ref().unwrap();
        assert!(c.hollow, "unfocused panes draw a hollow cursor");
        assert!(c.text.is_none());

        feed(&pane, cx, b"\x1b[6 q\x1b]12;#ff0000\x07");
        let c = frame(&pane, cx, true).cursor.unwrap();
        assert_eq!(c.shape, CursorShape::Beam);
        assert!(close(c.color, rgb_hsla(255, 0, 0)), "OSC 12 sets the cursor color");
        feed(&pane, cx, b"\x1b[4 q");
        assert_eq!(frame(&pane, cx, true).cursor.unwrap().shape, CursorShape::Underline);
        feed(&pane, cx, b"\x1b[?25l");
        assert!(frame(&pane, cx, true).cursor.is_none(), "DECTCEM hides it");
        feed(&pane, cx, b"\x1b[?25h\x1b[2 q\xe4\xb8\xad\x1b[D\x1b[D");
        let c = frame(&pane, cx, true).cursor.unwrap();
        assert_eq!(c.bounds.size.width, g.cell_w * 2., "wide cell under the cursor");
    }

    #[gpui::test]
    fn selection_and_search_highlights(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        feed(&pane, cx, b"find me\r\nfind you\r\nthird");
        let g = geom(&pane, cx);
        let th = theme(cx);
        pane.update(cx, |p, _| {
            let mut term = p.session.as_ref().unwrap().term().lock();
            let mut sel =
                Selection::new(SelectionType::Simple, GridPoint::new(Line(0), Column(5)), Side::Left);
            sel.update(GridPoint::new(Line(1), Column(2)), Side::Right);
            term.selection = Some(sel);
        });
        let pre = frame(&pane, cx, true);
        let sel: Vec<_> =
            pre.highlight_quads.iter().filter(|q| q.color == th.selection).map(|q| q.bounds).collect();
        assert_eq!(sel.len(), 2);
        assert_eq!(sel[0].origin.x, g.origin.x + g.cell_w * 5.);
        assert_eq!(sel[0].size.width, g.cell_w * (g.cols - 5) as f32, "to the end of the line");
        assert_eq!(sel[1].origin.x, g.origin.x);
        assert_eq!(sel[1].size.width, g.cell_w * 3.);

        pane.update(cx, |p, _| {
            let mut term = p.session.as_ref().unwrap().term().lock();
            let mut sel =
                Selection::new(SelectionType::Block, GridPoint::new(Line(2), Column(3)), Side::Right);
            sel.update(GridPoint::new(Line(0), Column(1)), Side::Left);
            term.selection = Some(sel);
        });
        let pre = frame(&pane, cx, true);
        let sel: Vec<_> = pre.highlight_quads.iter().map(|q| q.bounds).collect();
        assert_eq!(sel.len(), 3, "a block covers the same columns on every line");
        assert!(sel.iter().all(|b| b.origin.x == g.origin.x + g.cell_w && b.size.width == g.cell_w * 3.));

        pane.update(cx, |p, _| p.session.as_ref().unwrap().term().lock().selection = None);
        cx.dispatch_action(crate::actions::Find);
        cx.simulate_input("find");
        let pre = frame(&pane, cx, true);
        let current = th.accent.opacity(0.45);
        assert_eq!(pre.highlight_quads.len(), 2);
        assert_eq!(
            pre.highlight_quads.iter().filter(|q| q.color == current).count(),
            1,
            "the current match in accent"
        );
        assert!(pre.highlight_quads.iter().all(|q| q.bounds.size.width == g.cell_w * 4.));
    }

    #[gpui::test]
    fn hovered_links_are_underlined(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        let g = geom(&pane, cx);
        pane.update(cx, |p, _| {
            p.hovered_link = Some(HoveredLink {
                spans: vec![(Line(1), 2, 5)],
                target: LinkTarget::Url("https://nox.dev".into()),
            });
        });
        let pre = frame(&pane, cx, true);
        assert_eq!(pre.deco_quads.len(), 1);
        let b = pre.deco_quads[0].bounds;
        assert_eq!(b.origin, point(g.origin.x + g.cell_w * 2., g.origin.y + g.cell_h * 2. - px(2.)));
        assert_eq!(b.size, size(g.cell_w * 4., px(1.)));
        assert_eq!(pre.deco_quads[0].color, theme(cx).accent);
    }

    #[gpui::test]
    fn overlay_scrollbar_only_while_scrolling(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        assert!(frame(&pane, cx, true).scrollbar.is_none(), "hidden without scrollback");
        let lines: String = (0..200).map(|i| format!("{i}\r\n")).collect::<Vec<_>>().concat();
        feed(&pane, cx, lines.as_bytes());
        assert!(frame(&pane, cx, true).scrollbar.is_none(), "hidden until scrolling");
        // While it fades, frames keep coming (drawn by the window: the animation frame request
        // needs the pane's view).
        pane.update(cx, |p, cx| {
            p.scroll.last_scroll = Some(std::time::Instant::now());
            cx.notify();
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn paint_handles_every_cursor_and_overlay(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        feed(&pane, cx, b"\x1b[41mred bg\x1b[0m text\x1b[D");
        pane.update(cx, |p, _| {
            p.hovered_link =
                Some(HoveredLink { spans: vec![(Line(0), 0, 2)], target: LinkTarget::Url("x".into()) });
            let mut term = p.session.as_ref().unwrap().term().lock();
            let mut sel =
                Selection::new(SelectionType::Simple, GridPoint::new(Line(0), Column(0)), Side::Left);
            sel.update(GridPoint::new(Line(0), Column(3)), Side::Right);
            term.selection = Some(sel);
        });
        for shape in [&b"\x1b[2 q"[..], b"\x1b[6 q", b"\x1b[4 q"] {
            feed(&pane, cx, shape);
            draw(&pane, cx, true);
            draw(&pane, cx, false);
        }
        pane.update(cx, |p, _| p.marked_text = Some("か".into()));
        draw(&pane, cx, true);
        assert!(frame(&pane, cx, true).marked.is_some());
    }

    #[gpui::test]
    fn sub_line_offset_draws_an_extra_row(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        let lines: String = (0..200).map(|i| format!("L{i}\r\n")).collect::<Vec<_>>().concat();
        feed(&pane, cx, lines.as_bytes());
        let rows = geom(&pane, cx).rows;
        pane.update(cx, |p, _| {
            p.session
                .as_ref()
                .unwrap()
                .term()
                .lock()
                .scroll_display(nox_terminal::alacritty_terminal::grid::Scroll::Delta(3));
            p.scroll.frac = 5.;
        });
        let pre = frame(&pane, cx, true);
        let drawn: std::collections::BTreeSet<usize> = pre.texts.iter().map(|t| t.row).collect();
        assert_eq!(drawn.len(), rows + 1, "the partially visible line above is drawn too");
    }

    #[gpui::test]
    fn ime_marked_text_is_drawn_at_the_cursor(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        feed(&pane, cx, b"$ ");
        pane.update(cx, |p, _| p.marked_text = Some("にほ".into()));
        let pre = frame(&pane, cx, true);
        let (origin, line, h) = pre.marked.as_ref().expect("marked text");
        let g = geom(&pane, cx);
        assert_eq!(*origin, point(g.origin.x + g.cell_w * 2., g.origin.y));
        assert_eq!(line.text.as_ref(), "にほ");
        assert_eq!(*h, g.cell_h);
    }

    #[gpui::test]
    fn without_a_session_only_geometry_is_computed(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        pane.update(cx, |p, _| p.session = None);
        let pre = frame(&pane, cx, true);
        assert!(pre.texts.is_empty() && pre.cursor.is_none() && pre.scrollbar.is_none());
        assert!(pre.geom.cols >= 2 && pre.geom.rows >= 1);
        let _ = cell;
    }
}
