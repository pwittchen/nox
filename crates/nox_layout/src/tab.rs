//! A tab: a split tree plus focus and maximize state (SPEC §4.2, §6.1–6.5).

use crate::geometry::{Direction, Rect, Size, SplitDirection};
use crate::ids::{PaneId, TabId};
use crate::layout::Layout;

/// Outcome of [`Tab::close_pane`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClosePaneResult {
    /// The pane was removed; `focus` is the tab's focused pane now.
    Closed { focus: PaneId },
    /// The pane was the last one: the tab must be closed by the caller. The tab itself is
    /// left untouched (a layout is never empty).
    TabEmpty,
    /// The pane is not in this tab.
    NotFound,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub id: TabId,
    /// User-set name (SPEC §6.5); `None` follows the focused pane's title.
    pub custom_name: Option<String>,
    pub custom_icon: Option<String>,
    pub layout: Layout,
    /// Always a pane of `layout`.
    pub focused: PaneId,
    /// The maximized (zoomed) pane, if any; always a pane of `layout`.
    pub maximized: Option<PaneId>,
}

impl Tab {
    #[must_use]
    pub fn new(id: TabId, pane: PaneId) -> Self {
        Self::with_layout(id, Layout::Leaf(pane))
    }

    /// A tab over an existing layout, focused on its first pane.
    #[must_use]
    pub fn with_layout(id: TabId, layout: Layout) -> Self {
        Self {
            id,
            custom_name: None,
            custom_icon: None,
            focused: layout.first_pane(),
            layout,
            maximized: None,
        }
    }

    #[must_use]
    pub fn panes(&self) -> Vec<PaneId> {
        self.layout.panes()
    }

    #[must_use]
    pub fn contains(&self, pane: PaneId) -> bool {
        self.layout.contains(pane)
    }

    #[must_use]
    pub fn is_maximized(&self) -> bool {
        self.maximized.is_some()
    }

    /// Splits the focused pane without a size check; the new pane is focused and a
    /// maximized pane is restored first. Returns false if `new` is already in the tab.
    pub fn split_focused(&mut self, dir: SplitDirection, new: PaneId) -> bool {
        if !self.layout.split(self.focused, dir, new) {
            return false;
        }
        self.maximized = None;
        self.focused = new;
        true
    }

    /// Like [`Tab::split_focused`], but refused (false) when either side would be smaller
    /// than the minimum pane size (SPEC §6.1). `area` is the tab's full layout area.
    #[allow(clippy::too_many_arguments)]
    pub fn try_split_focused(
        &mut self,
        dir: SplitDirection,
        new: PaneId,
        area: Rect,
        cell: Size,
        padding_px: Size,
        divider_px: f32,
    ) -> bool {
        self.layout.can_split(self.focused, dir, area, cell, padding_px, divider_px)
            && self.split_focused(dir, new)
    }

    /// Closes `pane`; its sibling takes its space. When it was focused, focus goes to the
    /// adjacent pane of the sibling. A closed maximized pane un-maximizes the tab.
    pub fn close_pane(&mut self, pane: PaneId) -> ClosePaneResult {
        if !self.layout.contains(pane) {
            return ClosePaneResult::NotFound;
        }
        let Some(next) = self.layout.remove(pane) else {
            return ClosePaneResult::TabEmpty;
        };
        if self.focused == pane {
            self.focused = next;
        }
        if self.maximized == Some(pane) {
            self.maximized = None;
        }
        ClosePaneResult::Closed { focus: self.focused }
    }

    pub fn close_focused(&mut self) -> ClosePaneResult {
        self.close_pane(self.focused)
    }

    /// Focuses `pane` (e.g. on click). Focusing a pane other than the maximized one restores
    /// the layout. Returns false if the pane is not in this tab.
    pub fn focus(&mut self, pane: PaneId) -> bool {
        if !self.layout.contains(pane) {
            return false;
        }
        if self.maximized.is_some_and(|m| m != pane) {
            self.maximized = None;
        }
        self.focused = pane;
        true
    }

    /// `⌘⌥` + arrow: focuses the neighbor in `dir` (restoring a maximized tab first).
    /// Returns the new focused pane, or `None` (state unchanged) when there is no neighbor.
    pub fn focus_direction(&mut self, dir: Direction, area: Rect) -> Option<PaneId> {
        let n = self.layout.neighbor(self.focused, dir, area)?;
        self.focus(n);
        Some(n)
    }

    /// `⌘]`: next pane in tree order, wrapping (restores a maximized tab).
    pub fn focus_next(&mut self) -> PaneId {
        let n = self.layout.next(self.focused);
        self.focus(n);
        n
    }

    /// `⌘[`: previous pane in tree order, wrapping (restores a maximized tab).
    pub fn focus_prev(&mut self) -> PaneId {
        let n = self.layout.prev(self.focused);
        self.focus(n);
        n
    }

    /// `⌘⇧↵`: maximizes the focused pane, or restores. A single-pane tab is never
    /// maximized. Returns whether the tab is maximized afterwards.
    pub fn toggle_maximize(&mut self) -> bool {
        if self.maximized.is_some() {
            self.maximized = None;
        } else if self.layout.len() > 1 {
            self.maximized = Some(self.focused);
        }
        self.maximized.is_some()
    }

    /// The panes to draw and where: only the maximized pane (filling `area`) when maximized.
    #[must_use]
    pub fn visible_rects(&self, area: Rect, divider_px: f32) -> Vec<(PaneId, Rect)> {
        match self.maximized {
            Some(p) => vec![(p, area)],
            None => self.layout.layout_rects(area, divider_px),
        }
    }

    /// `⌘⌃` + arrow: see [`Layout::resize_toward`]. Does nothing while maximized.
    pub fn resize_focused(
        &mut self,
        dir: Direction,
        delta_px: f32,
        area: Rect,
        divider_px: f32,
        min_pane: Size,
    ) -> bool {
        self.maximized.is_none()
            && self.layout.resize_toward(self.focused, dir, delta_px, area, divider_px, min_pane)
    }

    pub fn equalize(&mut self) {
        self.layout.equalize();
    }

    /// True when `focused` and `maximized` point at panes of the layout and no pane repeats.
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        self.layout.is_valid()
            && self.layout.contains(self.focused)
            && self.maximized.is_none_or(|m| self.layout.contains(m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SplitDirection::{Horizontal as H, Vertical as V};

    const AREA: Rect = Rect::new(0.0, 0.0, 1001.0, 601.0);

    fn p(n: u64) -> PaneId {
        PaneId(n)
    }

    fn tab3() -> Tab {
        // 1 | (2 / 3), focused 3.
        let mut t = Tab::new(TabId(1), p(1));
        assert!(t.split_focused(V, p(2)));
        assert!(t.split_focused(H, p(3)));
        t
    }

    #[test]
    fn split_focuses_new_pane() {
        let t = tab3();
        assert_eq!(t.focused, p(3));
        assert_eq!(t.panes(), vec![p(1), p(2), p(3)]);
        assert!(t.is_consistent());
    }

    #[test]
    fn try_split_respects_min_size() {
        let mut t = Tab::new(TabId(1), p(1));
        let cell = Size::new(8.0, 16.0);
        let pad = Size::new(0.0, 0.0);
        let a = Rect::new(0.0, 0.0, 400.0, 400.0);
        assert!(t.try_split_focused(V, p(2), a, cell, pad, 1.0));
        // Each half is ~199 px; another vertical split gives ~99 < 160.
        assert!(!t.try_split_focused(V, p(3), a, cell, pad, 1.0));
        assert_eq!(t.panes(), vec![p(1), p(2)]);
        assert!(t.try_split_focused(H, p(3), a, cell, pad, 1.0));
    }

    #[test]
    fn close_moves_focus_to_sibling() {
        let mut t = tab3();
        assert_eq!(t.close_pane(p(3)), ClosePaneResult::Closed { focus: p(2) });
        assert_eq!(t.close_pane(p(9)), ClosePaneResult::NotFound);
        // Closing an unfocused pane keeps focus.
        assert_eq!(t.close_pane(p(1)), ClosePaneResult::Closed { focus: p(2) });
        assert_eq!(t.close_focused(), ClosePaneResult::TabEmpty);
        assert_eq!(t.panes(), vec![p(2)]);
    }

    #[test]
    fn maximize_and_restore() {
        let mut t = tab3();
        assert!(t.toggle_maximize());
        assert_eq!(t.maximized, Some(p(3)));
        assert_eq!(t.visible_rects(AREA, 1.0), vec![(p(3), AREA)]);
        assert!(!t.toggle_maximize());
        assert_eq!(t.visible_rects(AREA, 1.0).len(), 3);

        // Navigation restores first.
        t.toggle_maximize();
        assert_eq!(t.focus_direction(Direction::Left, AREA), Some(p(1)));
        assert_eq!(t.maximized, None);

        // No neighbor: stays maximized.
        t.toggle_maximize();
        assert_eq!(t.focus_direction(Direction::Left, AREA), None);
        assert_eq!(t.maximized, Some(p(1)));

        // Focusing the maximized pane itself keeps it maximized.
        assert!(t.focus(p(1)));
        assert_eq!(t.maximized, Some(p(1)));
        t.focus_next();
        assert_eq!(t.maximized, None);
        assert_eq!(t.focused, p(2));

        // Splitting restores; closing the maximized pane restores.
        t.toggle_maximize();
        assert!(t.split_focused(V, p(4)));
        assert_eq!(t.maximized, None);
        t.toggle_maximize();
        assert_eq!(t.close_focused(), ClosePaneResult::Closed { focus: p(2) });
        assert_eq!(t.maximized, None);

        // A single pane cannot be maximized.
        let mut single = Tab::new(TabId(2), p(10));
        assert!(!single.toggle_maximize());
    }

    #[test]
    fn focus_cycle() {
        let mut t = tab3();
        assert_eq!(t.focus_next(), p(1));
        assert_eq!(t.focus_prev(), p(3));
        assert!(!t.focus(p(9)));
    }

    #[test]
    fn resize_focused_ignored_when_maximized() {
        let mut t = tab3();
        let min = Size::new(50.0, 50.0);
        t.toggle_maximize();
        assert!(!t.resize_focused(Direction::Up, 10.0, AREA, 1.0, min));
        t.toggle_maximize();
        assert!(t.resize_focused(Direction::Up, 10.0, AREA, 1.0, min));
    }
}
