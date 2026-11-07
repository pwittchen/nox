//! The split tree of a tab (SPEC §4.3, §6.1–6.3).

use serde::{Deserialize, Serialize};

use crate::geometry::{Direction, Point, Rect, Size, SplitDirection};
use crate::ids::PaneId;

/// A pane is never narrower than this many columns (SPEC §6.1).
pub const MIN_COLS: u32 = 20;
/// A pane is never shorter than this many rows (SPEC §6.1).
pub const MIN_ROWS: u32 = 5;
/// Bounds used by [`Layout::set_ratio_raw`] when no pixel sizes are known.
pub const RATIO_MIN: f32 = 0.05;
pub const RATIO_MAX: f32 = 0.95;

/// Tolerance (px) for edge matching and min-size checks (rects are rounded to whole pixels).
const EPS: f32 = 0.5;

/// Minimum pane size in pixels: `MIN_COLS × MIN_ROWS` cells plus the pane's non-grid pixels.
///
/// `padding_px` is the total extra size of a pane per axis: padding on both sides, plus the
/// pane header height in `h` if the UI draws one inside the pane rect.
pub fn min_pane_size(cell: Size, padding_px: Size) -> Size {
    Size::new(MIN_COLS as f32 * cell.w + padding_px.w, MIN_ROWS as f32 * cell.h + padding_px.h)
}

/// Clamps `ratio` of a split whose extent along its axis is `total_px` so that the first side
/// is at least `min_first_px` and the second at least `min_second_px`.
///
/// When both minimums cannot fit, the space is shared in proportion to the minimums.
pub fn clamp_ratio(ratio: f32, total_px: f32, divider_px: f32, min_first_px: f32, min_second_px: f32) -> f32 {
    let ratio = if ratio.is_finite() { ratio } else { 0.5 };
    let avail = (total_px - divider_px).max(0.0);
    if avail <= 0.0 {
        return ratio.clamp(0.0, 1.0);
    }
    let lo = min_first_px / avail;
    let hi = 1.0 - min_second_px / avail;
    if lo > hi {
        let sum = min_first_px + min_second_px;
        if sum > 0.0 { min_first_px / sum } else { 0.5 }
    } else {
        ratio.clamp(lo.max(0.0), hi.min(1.0))
    }
}

/// Which child of a split.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Branch {
    /// Left (vertical split) or top (horizontal split).
    First,
    /// Right (vertical split) or bottom (horizontal split).
    Second,
}

/// Path from the root to a node: the branch taken at each split. The empty path is the root.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SplitPath(pub Vec<Branch>);

impl SplitPath {
    pub fn root() -> Self {
        Self(Vec::new())
    }

    pub fn child(&self, b: Branch) -> Self {
        let mut v = self.0.clone();
        v.push(b);
        Self(v)
    }

    /// The parent path and the branch taken from it; `None` for the root.
    pub fn parent(&self) -> Option<(SplitPath, Branch)> {
        let (last, rest) = self.0.split_last()?;
        Some((SplitPath(rest.to_vec()), *last))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// Same as [`SplitPath::is_root`].
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A divider between the two sides of a split.
#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    /// The split node this divider belongs to.
    pub path: SplitPath,
    pub dir: SplitDirection,
    /// The visible line (`divider_px` thick).
    pub rect: Rect,
    /// The area of the split node (both sides plus the divider).
    pub parent_area: Rect,
}

impl Divider {
    /// The line widened to `hit_px` across its thickness (SPEC §6.3: a 6px hit area),
    /// centered on the line.
    pub fn hit_rect(&self, hit_px: f32) -> Rect {
        let r = self.rect;
        match self.dir {
            SplitDirection::Vertical => {
                let grow = (hit_px - r.w).max(0.0) / 2.0;
                Rect::new(r.x - grow, r.y, r.w + 2.0 * grow, r.h)
            }
            SplitDirection::Horizontal => {
                let grow = (hit_px - r.h).max(0.0) / 2.0;
                Rect::new(r.x, r.y - grow, r.w, r.h + 2.0 * grow)
            }
        }
    }
}

/// A binary tree of panes (SPEC §4.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    Leaf(PaneId),
    Split {
        dir: SplitDirection,
        /// Share of the first child in the space left after the divider, 0.0–1.0.
        #[serde(serialize_with = "ser_ratio")]
        ratio: f32,
        first: Box<Layout>,
        second: Box<Layout>,
    },
}

/// Writes an `f32` ratio as the `f64` with the same shortest decimal form (`0.3`, not
/// `0.30000001192092896`); reading it back as `f32` gives the identical value.
fn ser_ratio<S: serde::Serializer>(ratio: &f32, s: S) -> Result<S::Ok, S::Error> {
    let v: f64 = ratio.to_string().parse().unwrap_or(f64::from(*ratio));
    s.serialize_f64(v)
}

/// Splits `area` into (first, divider, second). With `round`, the first side's extent is
/// rounded to whole pixels so dividers land on pixel boundaries.
fn split_area(
    area: Rect,
    dir: SplitDirection,
    ratio: f32,
    divider_px: f32,
    round: bool,
) -> (Rect, Rect, Rect) {
    match dir {
        SplitDirection::Vertical => {
            let d = divider_px.clamp(0.0, area.w.max(0.0));
            let avail = (area.w - d).max(0.0);
            let mut fw = avail * ratio;
            if round {
                fw = fw.round();
            }
            let fw = fw.clamp(0.0, avail);
            (
                Rect::new(area.x, area.y, fw, area.h),
                Rect::new(area.x + fw, area.y, d, area.h),
                Rect::new(area.x + fw + d, area.y, avail - fw, area.h),
            )
        }
        SplitDirection::Horizontal => {
            let d = divider_px.clamp(0.0, area.h.max(0.0));
            let avail = (area.h - d).max(0.0);
            let mut fh = avail * ratio;
            if round {
                fh = fh.round();
            }
            let fh = fh.clamp(0.0, avail);
            (
                Rect::new(area.x, area.y, area.w, fh),
                Rect::new(area.x, area.y + fh, area.w, d),
                Rect::new(area.x, area.y + fh + d, area.w, avail - fh),
            )
        }
    }
}

fn extent(r: Rect, dir: SplitDirection) -> f32 {
    match dir {
        SplitDirection::Vertical => r.w,
        SplitDirection::Horizontal => r.h,
    }
}

fn size_along(s: Size, dir: SplitDirection) -> f32 {
    match dir {
        SplitDirection::Vertical => s.w,
        SplitDirection::Horizontal => s.h,
    }
}

impl Layout {
    pub fn new(pane: PaneId) -> Self {
        Self::Leaf(pane)
    }

    /// All panes in tree order (left/top first).
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect_panes(&mut out);
        out
    }

    fn collect_panes(&self, out: &mut Vec<PaneId>) {
        match self {
            Self::Leaf(p) => out.push(*p),
            Self::Split { first, second, .. } => {
                first.collect_panes(out);
                second.collect_panes(out);
            }
        }
    }

    pub fn first_pane(&self) -> PaneId {
        match self {
            Self::Leaf(p) => *p,
            Self::Split { first, .. } => first.first_pane(),
        }
    }

    pub fn contains(&self, pane: PaneId) -> bool {
        match self {
            Self::Leaf(p) => *p == pane,
            Self::Split { first, second, .. } => first.contains(pane) || second.contains(pane),
        }
    }

    /// Number of panes.
    pub fn len(&self) -> usize {
        match self {
            Self::Leaf(_) => 1,
            Self::Split { first, second, .. } => first.len() + second.len(),
        }
    }

    /// Always false: a layout has at least one pane. Present for API symmetry with `len`.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// True when no pane appears twice.
    pub fn is_valid(&self) -> bool {
        let mut panes = self.panes();
        let n = panes.len();
        panes.sort();
        panes.dedup();
        panes.len() == n
    }

    /// A copy with every pane id replaced by `f(id)`.
    pub fn map_panes(&self, f: &mut impl FnMut(PaneId) -> PaneId) -> Layout {
        match self {
            Self::Leaf(p) => Self::Leaf(f(*p)),
            Self::Split { dir, ratio, first, second } => Self::Split {
                dir: *dir,
                ratio: *ratio,
                first: Box::new(first.map_panes(f)),
                second: Box::new(second.map_panes(f)),
            },
        }
    }

    // ---------------------------------------------------------------- paths

    /// Path to the leaf holding `pane`.
    pub fn path_to(&self, pane: PaneId) -> Option<SplitPath> {
        fn walk(node: &Layout, pane: PaneId, acc: &mut Vec<Branch>) -> bool {
            match node {
                Layout::Leaf(p) => *p == pane,
                Layout::Split { first, second, .. } => {
                    acc.push(Branch::First);
                    if walk(first, pane, acc) {
                        return true;
                    }
                    acc.pop();
                    acc.push(Branch::Second);
                    if walk(second, pane, acc) {
                        return true;
                    }
                    acc.pop();
                    false
                }
            }
        }
        let mut acc = Vec::new();
        walk(self, pane, &mut acc).then_some(SplitPath(acc))
    }

    pub fn node_at(&self, path: &SplitPath) -> Option<&Layout> {
        let mut node = self;
        for b in &path.0 {
            match node {
                Self::Leaf(_) => return None,
                Self::Split { first, second, .. } => {
                    node = match b {
                        Branch::First => first,
                        Branch::Second => second,
                    }
                }
            }
        }
        Some(node)
    }

    pub fn node_at_mut(&mut self, path: &SplitPath) -> Option<&mut Layout> {
        let mut node = self;
        for b in &path.0 {
            match node {
                Self::Leaf(_) => return None,
                Self::Split { first, second, .. } => {
                    node = match b {
                        Branch::First => first,
                        Branch::Second => second,
                    }
                }
            }
        }
        Some(node)
    }

    /// The area occupied by the node at `path` when the whole layout fills `area`.
    pub fn area_at(&self, path: &SplitPath, area: Rect, divider_px: f32) -> Option<Rect> {
        let mut node = self;
        let mut area = area;
        for b in &path.0 {
            match node {
                Self::Leaf(_) => return None,
                Self::Split { dir, ratio, first, second } => {
                    let (a, _, c) = split_area(area, *dir, *ratio, divider_px, true);
                    (node, area) = match b {
                        Branch::First => (first.as_ref(), a),
                        Branch::Second => (second.as_ref(), c),
                    };
                }
            }
        }
        Some(area)
    }

    // ---------------------------------------------------------------- split / remove

    /// Splits `target` in `dir`; `new` goes second (right / below), ratio 0.5.
    /// Returns false when `target` is missing or `new` is already in the layout.
    pub fn split(&mut self, target: PaneId, dir: SplitDirection, new: PaneId) -> bool {
        if target == new || self.contains(new) {
            return false;
        }
        let Some(path) = self.path_to(target) else {
            return false;
        };
        let node = self.node_at_mut(&path).expect("path_to returned a valid path");
        *node = Self::Split {
            dir,
            ratio: 0.5,
            first: Box::new(Self::Leaf(target)),
            second: Box::new(Self::Leaf(new)),
        };
        true
    }

    /// Whether splitting `target` in `dir` keeps both resulting panes at least
    /// `MIN_COLS × MIN_ROWS` (checked along the split axis; the other axis does not change).
    #[allow(clippy::too_many_arguments)]
    pub fn can_split(
        &self,
        target: PaneId,
        dir: SplitDirection,
        area: Rect,
        cell: Size,
        padding_px: Size,
        divider_px: f32,
    ) -> bool {
        if !self.contains(target) {
            return false;
        }
        let probe =
            (0..).map(|i| PaneId(u64::MAX - i)).find(|p| !self.contains(*p)).expect("some id is free");
        let mut l = self.clone();
        l.split(target, dir, probe);
        let min = size_along(min_pane_size(cell, padding_px), dir);
        l.layout_rects(area, divider_px)
            .into_iter()
            .filter(|(p, _)| *p == target || *p == probe)
            .all(|(_, r)| extent(r, dir) + EPS >= min)
    }

    /// Removes `target`; its sibling takes its space. Returns the pane that should get focus:
    /// the leaf of the sibling subtree adjacent to where the removed pane was (greatest
    /// overlap with its edge). Returns `None` when `target` is missing or is the only pane —
    /// a layout is never empty, so closing the last pane means closing the tab (see
    /// [`crate::Tab::close_pane`]).
    pub fn remove(&mut self, target: PaneId) -> Option<PaneId> {
        let path = self.path_to(target)?;
        let (parent, side) = path.parent()?;
        let Some(Self::Split { dir, first, second, .. }) = self.node_at(&parent) else {
            unreachable!("parent of a leaf is a split");
        };
        let toward = match (dir, side) {
            (SplitDirection::Vertical, Branch::First) => Direction::Right,
            (SplitDirection::Vertical, Branch::Second) => Direction::Left,
            (SplitDirection::Horizontal, Branch::First) => Direction::Down,
            (SplitDirection::Horizontal, Branch::Second) => Direction::Up,
        };
        let sibling = match side {
            Branch::First => second,
            Branch::Second => first,
        };
        let sibling_panes = sibling.panes();
        let nominal = Rect::new(0.0, 0.0, 10_000.0, 10_000.0);
        let focus = self
            .neighbor_where(target, toward, nominal, |p| sibling_panes.contains(&p))
            .unwrap_or(sibling_panes[0]);

        let node = self.node_at_mut(&parent).expect("valid path");
        let Self::Split { first, second, .. } = std::mem::replace(node, Self::Leaf(target)) else {
            unreachable!()
        };
        *node = match side {
            Branch::First => *second,
            Branch::Second => *first,
        };
        Some(focus)
    }

    // ---------------------------------------------------------------- geometry

    /// Rect of every pane (tree order) when the layout fills `area`; dividers are
    /// `divider_px` thick. Pane extents are rounded to whole pixels.
    pub fn layout_rects(&self, area: Rect, divider_px: f32) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::with_capacity(self.len());
        self.rects_inner(area, divider_px, true, &mut out);
        out
    }

    fn rects_inner(&self, area: Rect, divider_px: f32, round: bool, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Self::Leaf(p) => out.push((*p, area)),
            Self::Split { dir, ratio, first, second } => {
                let (a, _, c) = split_area(area, *dir, *ratio, divider_px, round);
                first.rects_inner(a, divider_px, round, out);
                second.rects_inner(c, divider_px, round, out);
            }
        }
    }

    pub fn rect_of(&self, pane: PaneId, area: Rect, divider_px: f32) -> Option<Rect> {
        self.area_at(&self.path_to(pane)?, area, divider_px)
    }

    /// Every divider (pre-order: outer splits first).
    pub fn dividers(&self, area: Rect, divider_px: f32) -> Vec<Divider> {
        let mut out = Vec::new();
        self.dividers_inner(SplitPath::root(), area, divider_px, &mut out);
        out
    }

    fn dividers_inner(&self, path: SplitPath, area: Rect, divider_px: f32, out: &mut Vec<Divider>) {
        if let Self::Split { dir, ratio, first, second } = self {
            let (a, d, c) = split_area(area, *dir, *ratio, divider_px, true);
            out.push(Divider { path: path.clone(), dir: *dir, rect: d, parent_area: area });
            first.dividers_inner(path.child(Branch::First), a, divider_px, out);
            second.dividers_inner(path.child(Branch::Second), c, divider_px, out);
        }
    }

    /// The divider whose hit rect (`hit_px` wide) contains `point`; inner (deeper) dividers
    /// win over outer ones.
    pub fn divider_at(&self, point: Point, area: Rect, divider_px: f32, hit_px: f32) -> Option<Divider> {
        self.dividers(area, divider_px).into_iter().rev().find(|d| d.hit_rect(hit_px).contains(point))
    }

    /// Minimum size of this subtree given the minimum size of one pane.
    pub fn min_size(&self, divider_px: f32, min_pane: Size) -> Size {
        match self {
            Self::Leaf(_) => min_pane,
            Self::Split { dir, first, second, .. } => {
                let a = first.min_size(divider_px, min_pane);
                let b = second.min_size(divider_px, min_pane);
                match dir {
                    SplitDirection::Vertical => Size::new(a.w + divider_px + b.w, a.h.max(b.h)),
                    SplitDirection::Horizontal => Size::new(a.w.max(b.w), a.h + divider_px + b.h),
                }
            }
        }
    }

    // ---------------------------------------------------------------- ratios

    /// Sets the ratio of the split at `path`, clamped so that neither side gets smaller than
    /// its minimum size (`min_pane` per pane, see [`min_pane_size`]). Returns false when
    /// `path` is not a split.
    pub fn set_ratio(
        &mut self,
        path: &SplitPath,
        ratio: f32,
        area: Rect,
        divider_px: f32,
        min_pane: Size,
    ) -> bool {
        let Some(parent_area) = self.area_at(path, area, divider_px) else {
            return false;
        };
        let Some(Self::Split { ratio: r, .. }) = self.node_at_mut(path) else {
            return false;
        };
        *r = ratio;
        self.node_at_mut(path).expect("checked above").clamp_to_min_sizes(parent_area, divider_px, min_pane);
        true
    }

    /// Clamps every split ratio in this subtree (laid out in `area`) so that each side keeps
    /// its minimum size where possible. Ratios that already fit are left alone. Call after a
    /// ratio change or when the area shrinks.
    pub fn clamp_to_min_sizes(&mut self, area: Rect, divider_px: f32, min_pane: Size) {
        if let Self::Split { dir, ratio, first, second } = self {
            let min1 = size_along(first.min_size(divider_px, min_pane), *dir);
            let min2 = size_along(second.min_size(divider_px, min_pane), *dir);
            *ratio = clamp_ratio(*ratio, extent(area, *dir), divider_px, min1, min2);
            let (a, _, c) = split_area(area, *dir, *ratio, divider_px, true);
            first.clamp_to_min_sizes(a, divider_px, min_pane);
            second.clamp_to_min_sizes(c, divider_px, min_pane);
        }
    }

    /// Sets the ratio of the split at `path`, clamped to `[RATIO_MIN, RATIO_MAX]`.
    pub fn set_ratio_raw(&mut self, path: &SplitPath, ratio: f32) -> bool {
        match self.node_at_mut(path) {
            Some(Self::Split { ratio: r, .. }) => {
                let ratio = if ratio.is_finite() { ratio } else { 0.5 };
                *r = ratio.clamp(RATIO_MIN, RATIO_MAX);
                true
            }
            _ => false,
        }
    }

    pub fn ratio_at(&self, path: &SplitPath) -> Option<f32> {
        match self.node_at(path)? {
            Self::Split { ratio, .. } => Some(*ratio),
            Self::Leaf(_) => None,
        }
    }

    /// The (unclamped, within 0–1) ratio that puts the divider of the split at `path` under
    /// the pointer. Feed it to [`Layout::set_ratio`] to apply the min-size clamp.
    pub fn ratio_for_drag(
        &self,
        path: &SplitPath,
        pointer: Point,
        area: Rect,
        divider_px: f32,
    ) -> Option<f32> {
        let Self::Split { dir, .. } = self.node_at(path)? else {
            return None;
        };
        let pa = self.area_at(path, area, divider_px)?;
        let (offset, total) = match dir {
            SplitDirection::Vertical => (pointer.x - pa.x, pa.w),
            SplitDirection::Horizontal => (pointer.y - pa.y, pa.h),
        };
        let avail = (total - divider_px).max(0.0);
        if avail <= 0.0 {
            return Some(0.5);
        }
        Some(((offset - divider_px / 2.0) / avail).clamp(0.0, 1.0))
    }

    /// Equalizes the whole tree, iTerm style: a chain of same-direction splits gives every
    /// member an equal share (three side-by-side panes get 1/3 each; a stacked column counts
    /// as one member of a side-by-side chain).
    pub fn equalize(&mut self) {
        if let Self::Split { dir, ratio, first, second } = self {
            let a = first.chain_count(*dir) as f32;
            let b = second.chain_count(*dir) as f32;
            *ratio = a / (a + b);
            first.equalize();
            second.equalize();
        }
    }

    fn chain_count(&self, dir: SplitDirection) -> u32 {
        match self {
            Self::Split { dir: d, first, second, .. } if *d == dir => {
                first.chain_count(dir) + second.chain_count(dir)
            }
            _ => 1,
        }
    }

    /// Sets the split at `path` to 0.5 (double-click on a divider).
    pub fn equalize_at(&mut self, path: &SplitPath) -> bool {
        match self.node_at_mut(path) {
            Some(Self::Split { ratio, .. }) => {
                *ratio = 0.5;
                true
            }
            _ => false,
        }
    }

    /// Moves the nearest divider of `pane` in `dir` by `delta_px` (`⌘⌃` + arrow, one cell).
    ///
    /// The divider on the `dir` side of the pane is used (the pane grows); when the pane has
    /// none on that side (it touches the edge), the nearest divider on the opposite side is
    /// moved in `dir` instead (the pane shrinks). The result is clamped to min sizes.
    /// Returns true if a ratio changed.
    pub fn resize_toward(
        &mut self,
        pane: PaneId,
        dir: Direction,
        delta_px: f32,
        area: Rect,
        divider_px: f32,
        min_pane: Size,
    ) -> bool {
        let Some(leaf) = self.path_to(pane) else {
            return false;
        };
        let axis = dir.axis();
        let grow_side = match dir {
            Direction::Right | Direction::Down => Branch::First,
            Direction::Left | Direction::Up => Branch::Second,
        };
        let mut chosen = None;
        let mut fallback = None;
        for i in (0..leaf.len()).rev() {
            let p = SplitPath(leaf.0[..i].to_vec());
            if let Some(Self::Split { dir: d, .. }) = self.node_at(&p)
                && *d == axis
            {
                if leaf.0[i] == grow_side {
                    chosen = Some(p);
                    break;
                } else if fallback.is_none() {
                    fallback = Some(p);
                }
            }
        }
        let Some(p) = chosen.or(fallback) else {
            return false;
        };
        let (Some(pa), Some(old)) = (self.area_at(&p, area, divider_px), self.ratio_at(&p)) else {
            return false;
        };
        let avail = (extent(pa, axis) - divider_px).max(0.0);
        if avail <= 0.0 {
            return false;
        }
        let signed = match dir {
            Direction::Right | Direction::Down => delta_px,
            Direction::Left | Direction::Up => -delta_px,
        };
        let new = (old * avail + signed) / avail;
        self.set_ratio(&p, new, area, divider_px, min_pane);
        self.ratio_at(&p) != Some(old)
    }

    // ---------------------------------------------------------------- navigation

    /// The nearest pane in `dir` (SPEC §6.2, iTerm-like): among panes whose edge touches the
    /// edge of `from` in that direction, the one with the greatest overlap on the
    /// perpendicular axis; ties go to the one closest to `from`'s center.
    pub fn neighbor(&self, from: PaneId, dir: Direction, area: Rect) -> Option<PaneId> {
        self.neighbor_where(from, dir, area, |_| true)
    }

    fn neighbor_where(
        &self,
        from: PaneId,
        dir: Direction,
        area: Rect,
        filter: impl Fn(PaneId) -> bool,
    ) -> Option<PaneId> {
        let mut rects = Vec::with_capacity(self.len());
        self.rects_inner(area, 0.0, false, &mut rects);
        let (_, f) = *rects.iter().find(|(p, _)| *p == from)?;
        let fc = f.center();
        let mut best: Option<(PaneId, f32, f32)> = None;
        for &(p, r) in &rects {
            if p == from || !filter(p) {
                continue;
            }
            let touches = match dir {
                Direction::Left => (r.right() - f.x).abs() <= EPS,
                Direction::Right => (r.x - f.right()).abs() <= EPS,
                Direction::Up => (r.bottom() - f.y).abs() <= EPS,
                Direction::Down => (r.y - f.bottom()).abs() <= EPS,
            };
            if !touches {
                continue;
            }
            let rc = r.center();
            let (overlap, dist) = match dir {
                Direction::Left | Direction::Right => {
                    (r.bottom().min(f.bottom()) - r.y.max(f.y), (rc.y - fc.y).abs())
                }
                Direction::Up | Direction::Down => {
                    (r.right().min(f.right()) - r.x.max(f.x), (rc.x - fc.x).abs())
                }
            };
            if overlap <= 0.0 {
                continue;
            }
            let better = match best {
                None => true,
                Some((_, bo, bd)) => {
                    if (overlap - bo).abs() > EPS {
                        overlap > bo
                    } else {
                        dist < bd
                    }
                }
            };
            if better {
                best = Some((p, overlap, dist));
            }
        }
        best.map(|(p, _, _)| p)
    }

    /// The pane after `from` in tree order, wrapping. Returns the first pane if `from` is
    /// missing.
    pub fn next(&self, from: PaneId) -> PaneId {
        let panes = self.panes();
        match panes.iter().position(|p| *p == from) {
            Some(i) => panes[(i + 1) % panes.len()],
            None => panes[0],
        }
    }

    /// The pane before `from` in tree order, wrapping. Returns the first pane if `from` is
    /// missing.
    pub fn prev(&self, from: PaneId) -> PaneId {
        let panes = self.panes();
        match panes.iter().position(|p| *p == from) {
            Some(i) => panes[(i + panes.len() - 1) % panes.len()],
            None => panes[0],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SplitDirection::{Horizontal as H, Vertical as V};

    fn p(n: u64) -> PaneId {
        PaneId(n)
    }

    fn leaf(n: u64) -> Layout {
        Layout::Leaf(p(n))
    }

    fn split(dir: SplitDirection, ratio: f32, a: Layout, b: Layout) -> Layout {
        Layout::Split { dir, ratio, first: Box::new(a), second: Box::new(b) }
    }

    const AREA: Rect = Rect::new(0.0, 0.0, 1001.0, 601.0);

    /// ```text
    /// +---+---+
    /// | 1 | 2 |
    /// |   +---+
    /// |   | 3 |
    /// +---+---+
    /// ```
    fn l_shape() -> Layout {
        split(V, 0.5, leaf(1), split(H, 0.5, leaf(2), leaf(3)))
    }

    /// ```text
    /// +---+---+
    /// | 1 | 2 |
    /// +---+---+
    /// | 3 | 4 |
    /// +---+---+
    /// ```
    /// Built as H(V(1,2), V(3,4)) with the bottom divider offset, so 1/2 and 3/4 dividers
    /// don't line up.
    fn grid() -> Layout {
        split(H, 0.5, split(V, 0.5, leaf(1), leaf(2)), split(V, 0.3, leaf(3), leaf(4)))
    }

    fn rect(l: &Layout, n: u64) -> Rect {
        l.rect_of(p(n), AREA, 1.0).unwrap()
    }

    #[test]
    fn panes_in_tree_order() {
        assert_eq!(grid().panes(), vec![p(1), p(2), p(3), p(4)]);
        assert_eq!(grid().len(), 4);
        assert!(grid().contains(p(4)));
        assert!(!grid().contains(p(5)));
    }

    #[test]
    fn split_puts_new_second() {
        let mut l = leaf(1);
        assert!(l.split(p(1), V, p(2)));
        assert_eq!(l, split(V, 0.5, leaf(1), leaf(2)));
        assert!(l.split(p(2), H, p(3)));
        assert_eq!(l, l_shape());
        // Unknown target / duplicate id are refused.
        assert!(!l.split(p(9), V, p(10)));
        assert!(!l.split(p(1), V, p(2)));
        assert!(l.is_valid());
    }

    #[test]
    fn rects_vertical_split() {
        let l = split(V, 0.5, leaf(1), leaf(2));
        let r = l.layout_rects(AREA, 1.0);
        assert_eq!(r[0], (p(1), Rect::new(0.0, 0.0, 500.0, 601.0)));
        assert_eq!(r[1], (p(2), Rect::new(501.0, 0.0, 500.0, 601.0)));
        let d = l.dividers(AREA, 1.0);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].rect, Rect::new(500.0, 0.0, 1.0, 601.0));
        assert_eq!(d[0].parent_area, AREA);
        assert_eq!(d[0].path, SplitPath::root());
    }

    #[test]
    fn rects_sum_to_area_minus_dividers() {
        for l in [l_shape(), grid(), leaf(1)] {
            let rects = l.layout_rects(AREA, 1.0);
            let divs = l.dividers(AREA, 1.0);
            let sum: f32 = rects.iter().map(|(_, r)| r.area()).sum::<f32>()
                + divs.iter().map(|d| d.rect.area()).sum::<f32>();
            assert!((sum - AREA.area()).abs() < 0.01, "{sum}");
            for (i, (_, a)) in rects.iter().enumerate() {
                for (_, b) in &rects[i + 1..] {
                    assert_eq!(a.intersection_area(b), 0.0);
                }
                for d in &divs {
                    assert_eq!(a.intersection_area(&d.rect), 0.0);
                }
            }
        }
    }

    #[test]
    fn divider_hit_rects() {
        let l = l_shape();
        let divs = l.dividers(AREA, 1.0);
        assert_eq!(divs.len(), 2);
        let outer = &divs[0];
        assert_eq!(outer.hit_rect(6.0), Rect::new(497.5, 0.0, 6.0, 601.0));
        let inner = &divs[1];
        assert_eq!(inner.path, SplitPath(vec![Branch::Second]));
        assert_eq!(inner.rect, Rect::new(501.0, 300.0, 500.0, 1.0));
        assert_eq!(inner.hit_rect(6.0), Rect::new(501.0, 297.5, 500.0, 6.0));

        let hit = l.divider_at(Point::new(499.0, 100.0), AREA, 1.0, 6.0).unwrap();
        assert_eq!(hit.path, SplitPath::root());
        let hit = l.divider_at(Point::new(700.0, 302.0), AREA, 1.0, 6.0).unwrap();
        assert_eq!(hit.path, SplitPath(vec![Branch::Second]));
        assert!(l.divider_at(Point::new(200.0, 200.0), AREA, 1.0, 6.0).is_none());
    }

    #[test]
    fn navigate_l_shape_every_direction() {
        let l = l_shape();
        use Direction::*;
        assert_eq!(l.neighbor(p(1), Right, AREA), Some(p(2)));
        assert_eq!(l.neighbor(p(1), Left, AREA), None);
        assert_eq!(l.neighbor(p(1), Up, AREA), None);
        assert_eq!(l.neighbor(p(1), Down, AREA), None);
        assert_eq!(l.neighbor(p(2), Left, AREA), Some(p(1)));
        assert_eq!(l.neighbor(p(3), Left, AREA), Some(p(1)));
        assert_eq!(l.neighbor(p(2), Down, AREA), Some(p(3)));
        assert_eq!(l.neighbor(p(3), Up, AREA), Some(p(2)));
        assert_eq!(l.neighbor(p(2), Up, AREA), None);
        assert_eq!(l.neighbor(p(3), Right, AREA), None);
    }

    #[test]
    fn navigate_prefers_greatest_overlap() {
        // 1 is the left pane; right side split 0.8 / 0.2 → 2 overlaps 1 the most.
        let l = split(V, 0.5, leaf(1), split(H, 0.8, leaf(2), leaf(3)));
        assert_eq!(l.neighbor(p(1), Direction::Right, AREA), Some(p(2)));
        let l = split(V, 0.5, leaf(1), split(H, 0.2, leaf(2), leaf(3)));
        assert_eq!(l.neighbor(p(1), Direction::Right, AREA), Some(p(3)));
    }

    #[test]
    fn navigate_tie_goes_to_closest_center() {
        // Left: 1 on top (0.3), 2 below. Right: three equal rows 3, 4, 5.
        // From 4 (middle right, y 1/3..2/3) going left: overlaps 1 by 0..0.3 → none-ish:
        // 1 covers y 0..0.3, 2 covers 0.3..1. 4 covers 0.333..0.667 → overlaps only 2.
        let right = split(H, 1.0 / 3.0, leaf(3), split(H, 0.5, leaf(4), leaf(5)));
        let l = split(V, 0.5, split(H, 0.3, leaf(1), leaf(2)), right);
        assert_eq!(l.neighbor(p(4), Direction::Left, AREA), Some(p(2)));
        // From 2 (y 0.3..1) going right: overlaps 3 by 0.033, 4 by 0.333, 5 by 0.333 → tie
        // between 4 and 5; 2's center is 0.65, 4's center 0.5, 5's 0.833 → 4.
        assert_eq!(l.neighbor(p(2), Direction::Right, AREA), Some(p(4)));
    }

    #[test]
    fn navigate_grid_every_direction() {
        let l = grid();
        use Direction::*;
        assert_eq!(l.neighbor(p(1), Right, AREA), Some(p(2)));
        assert_eq!(l.neighbor(p(2), Left, AREA), Some(p(1)));
        assert_eq!(l.neighbor(p(3), Right, AREA), Some(p(4)));
        assert_eq!(l.neighbor(p(4), Left, AREA), Some(p(3)));
        // Bottom divider at 0.3: 1 (0..0.5) overlaps 3 (0..0.3) by 0.3 and 4 (0.3..1) by 0.2.
        assert_eq!(l.neighbor(p(1), Down, AREA), Some(p(3)));
        // 2 (0.5..1) overlaps only 4.
        assert_eq!(l.neighbor(p(2), Down, AREA), Some(p(4)));
        assert_eq!(l.neighbor(p(3), Up, AREA), Some(p(1)));
        // 4 (0.3..1) overlaps 1 by 0.2, 2 by 0.5 → 2.
        assert_eq!(l.neighbor(p(4), Up, AREA), Some(p(2)));
        assert_eq!(l.neighbor(p(1), Up, AREA), None);
        assert_eq!(l.neighbor(p(4), Right, AREA), None);
        assert_eq!(l.neighbor(p(4), Down, AREA), None);
    }

    #[test]
    fn next_prev_wrap() {
        let l = grid();
        assert_eq!(l.next(p(1)), p(2));
        assert_eq!(l.next(p(4)), p(1));
        assert_eq!(l.prev(p(1)), p(4));
        assert_eq!(l.prev(p(3)), p(2));
        assert_eq!(leaf(7).next(p(7)), p(7));
    }

    #[test]
    fn remove_sibling_takes_space_and_focus_is_adjacent() {
        let mut l = l_shape();
        // Removing 1 (left): right column takes the space; focus goes to the right column's
        // pane with the greatest overlap with 1's right edge (tie → closest center → 2).
        assert_eq!(l.remove(p(1)), Some(p(2)));
        assert_eq!(l, split(H, 0.5, leaf(2), leaf(3)));

        let mut l = l_shape();
        assert_eq!(l.remove(p(3)), Some(p(2)));
        assert_eq!(l, split(V, 0.5, leaf(1), leaf(2)));

        // Grid: removing 1 → sibling is 2 (to the right).
        let mut l = grid();
        assert_eq!(l.remove(p(1)), Some(p(2)));
        assert_eq!(l.panes(), vec![p(2), p(3), p(4)]);

        // Removing the top row subtree member whose sibling is a whole subtree.
        // V(1, V(2, 3)): removing 1 → focus 2 (closest to where 1 was).
        let mut l = split(V, 0.5, leaf(1), split(V, 0.5, leaf(2), leaf(3)));
        assert_eq!(l.remove(p(1)), Some(p(2)));
        // V(V(1, 2), 3): removing 3 → focus 2.
        let mut l = split(V, 0.5, split(V, 0.5, leaf(1), leaf(2)), leaf(3));
        assert_eq!(l.remove(p(3)), Some(p(2)));
        // H(1, H(V(2,3), 4)) with pane 1 removed → nearest is the V(2,3) row; 2 and 3 tie on
        // overlap (each half) → closest center: equal distance, first wins → 2.
        let mut l = split(H, 0.5, leaf(1), split(H, 0.5, split(V, 0.5, leaf(2), leaf(3)), leaf(4)));
        assert_eq!(l.remove(p(1)), Some(p(2)));
    }

    #[test]
    fn remove_last_or_missing_is_none() {
        let mut l = leaf(1);
        assert_eq!(l.remove(p(1)), None);
        assert_eq!(l, leaf(1));
        let mut l = l_shape();
        assert_eq!(l.remove(p(9)), None);
        assert_eq!(l, l_shape());
    }

    #[test]
    fn min_sizes_refuse_split() {
        let cell = Size::new(8.0, 16.0);
        let pad = Size::new(8.0, 8.0);
        // Min pane: 20*8+8 = 168 wide, 5*16+8 = 88 tall.
        assert_eq!(min_pane_size(cell, pad), Size::new(168.0, 88.0));
        let l = leaf(1);
        // 337 wide: (337 - 1) / 2 = 168 each → fits exactly.
        let a = Rect::new(0.0, 0.0, 337.0, 177.0);
        assert!(l.can_split(p(1), V, a, cell, pad, 1.0));
        assert!(l.can_split(p(1), H, a, cell, pad, 1.0));
        let a = Rect::new(0.0, 0.0, 335.0, 175.0);
        assert!(!l.can_split(p(1), V, a, cell, pad, 1.0));
        assert!(!l.can_split(p(1), H, a, cell, pad, 1.0));
        // Missing target.
        assert!(!l.can_split(p(2), V, AREA, cell, pad, 1.0));
        // In a split layout only the target's own space counts.
        let l = split(V, 0.8, leaf(1), leaf(2));
        let a = Rect::new(0.0, 0.0, 1000.0, 600.0);
        assert!(l.can_split(p(1), V, a, cell, pad, 1.0));
        assert!(!l.can_split(p(2), V, a, cell, pad, 1.0));
        assert!(l.can_split(p(2), H, a, cell, pad, 1.0));
    }

    #[test]
    fn set_ratio_clamps_to_min_sizes() {
        let min = Size::new(100.0, 50.0);
        let mut l = split(V, 0.5, leaf(1), split(V, 0.5, leaf(2), leaf(3)));
        let a = Rect::new(0.0, 0.0, 1001.0, 600.0);
        // Right side needs 100 + 1 + 100 = 201 px; avail 1000 → ratio ≤ 0.799.
        assert!(l.set_ratio(&SplitPath::root(), 0.95, a, 1.0, min));
        let r = l.ratio_at(&SplitPath::root()).unwrap();
        assert!((r - 0.799).abs() < 1e-4, "{r}");
        assert!(l.set_ratio(&SplitPath::root(), 0.01, a, 1.0, min));
        assert!((l.ratio_at(&SplitPath::root()).unwrap() - 0.1).abs() < 1e-4);
        // Not a split.
        assert!(!l.set_ratio(&SplitPath(vec![Branch::First]), 0.5, a, 1.0, min));
        for (_, r) in l.layout_rects(a, 1.0) {
            assert!(r.w + EPS >= min.w);
        }
    }

    #[test]
    fn clamp_ratio_infeasible_shares_proportionally() {
        assert_eq!(clamp_ratio(0.9, 101.0, 1.0, 100.0, 100.0), 0.5);
        assert_eq!(clamp_ratio(0.9, 101.0, 1.0, 100.0, 300.0), 0.25);
        assert_eq!(clamp_ratio(0.3, 1001.0, 1.0, 100.0, 100.0), 0.3);
        assert_eq!(clamp_ratio(f32::NAN, 1001.0, 1.0, 0.0, 0.0), 0.5);
    }

    #[test]
    fn set_ratio_raw_clamps() {
        let mut l = split(V, 0.5, leaf(1), leaf(2));
        assert!(l.set_ratio_raw(&SplitPath::root(), 2.0));
        assert_eq!(l.ratio_at(&SplitPath::root()), Some(RATIO_MAX));
        assert!(!l.set_ratio_raw(&SplitPath(vec![Branch::First]), 0.5));
    }

    #[test]
    fn ratio_for_drag_follows_pointer() {
        let l = l_shape();
        let r = l.ratio_for_drag(&SplitPath::root(), Point::new(250.5, 10.0), AREA, 1.0).unwrap();
        assert!((r - 0.25).abs() < 1e-4);
        // Inner horizontal split lives in the right column (y from 0 to 601).
        let inner = SplitPath(vec![Branch::Second]);
        let r = l.ratio_for_drag(&inner, Point::new(800.0, 450.5), AREA, 1.0).unwrap();
        assert!((r - 0.75).abs() < 1e-4);
        let r = l.ratio_for_drag(&inner, Point::new(800.0, -50.0), AREA, 1.0).unwrap();
        assert_eq!(r, 0.0);
        assert!(l.ratio_for_drag(&SplitPath(vec![Branch::First]), Point::default(), AREA, 1.0).is_none());
    }

    #[test]
    fn equalize_three_way_chain() {
        let mut l = split(V, 0.8, leaf(1), split(V, 0.1, leaf(2), leaf(3)));
        l.equalize();
        assert_eq!(l.ratio_at(&SplitPath::root()), Some(1.0 / 3.0));
        assert_eq!(l.ratio_at(&SplitPath(vec![Branch::Second])), Some(0.5));
        let a = Rect::new(0.0, 0.0, 902.0, 100.0);
        let rects = l.layout_rects(a, 1.0);
        let widths: Vec<f32> = rects.iter().map(|(_, r)| r.w).collect();
        assert!(widths.iter().all(|w| (w - 300.0).abs() <= 1.0), "{widths:?}");
    }

    #[test]
    fn equalize_mixed() {
        // V(H(1,2), V(3, H(4,5))): side-by-side chain has 3 members: column(1,2), 3,
        // column(4,5) → root = 1/3, inner V = 1/2; each H = 1/2.
        let mut l = split(
            V,
            0.9,
            split(H, 0.2, leaf(1), leaf(2)),
            split(V, 0.9, leaf(3), split(H, 0.7, leaf(4), leaf(5))),
        );
        l.equalize();
        assert_eq!(l.ratio_at(&SplitPath::root()), Some(1.0 / 3.0));
        assert_eq!(l.ratio_at(&SplitPath(vec![Branch::First])), Some(0.5));
        assert_eq!(l.ratio_at(&SplitPath(vec![Branch::Second])), Some(0.5));
        assert_eq!(l.ratio_at(&SplitPath(vec![Branch::Second, Branch::Second])), Some(0.5));
    }

    #[test]
    fn equalize_at_only_that_split() {
        let mut l = split(V, 0.8, leaf(1), split(V, 0.1, leaf(2), leaf(3)));
        assert!(l.equalize_at(&SplitPath(vec![Branch::Second])));
        assert_eq!(l.ratio_at(&SplitPath::root()), Some(0.8));
        assert_eq!(l.ratio_at(&SplitPath(vec![Branch::Second])), Some(0.5));
        assert!(!l.equalize_at(&SplitPath(vec![Branch::First])));
    }

    #[test]
    fn resize_toward_moves_nearest_divider() {
        let min = Size::new(100.0, 50.0);
        let a = Rect::new(0.0, 0.0, 1001.0, 601.0);
        let mut l = l_shape();
        // 1 grows right by 10 px.
        assert!(l.resize_toward(p(1), Direction::Right, 10.0, a, 1.0, min));
        assert_eq!(rect(&l, 1).w, 510.0);
        // 1 has no divider on its left: its right divider moves left (1 shrinks).
        assert!(l.resize_toward(p(1), Direction::Left, 20.0, a, 1.0, min));
        assert_eq!(rect(&l, 1).w, 490.0);
        // 2 (top right): Down moves the inner horizontal divider down.
        let before = rect(&l, 2).h;
        assert!(l.resize_toward(p(2), Direction::Down, 16.0, a, 1.0, min));
        assert_eq!(rect(&l, 2).h, before + 16.0);
        // 3 Up: the same divider moves up (3 grows).
        assert!(l.resize_toward(p(3), Direction::Up, 16.0, a, 1.0, min));
        assert_eq!(rect(&l, 2).h, before);
        // 2 Left: divider on its left side (root) moves left; 2 grows.
        let w = rect(&l, 2).w;
        assert!(l.resize_toward(p(2), Direction::Left, 10.0, a, 1.0, min));
        assert_eq!(rect(&l, 2).w, w + 10.0);
        // Clamped at min size: a huge move stops at the minimum.
        l.resize_toward(p(2), Direction::Right, 5000.0, a, 1.0, min);
        assert!((rect(&l, 2).w - 100.0).abs() <= 1.0);
        assert!(!l.resize_toward(p(2), Direction::Right, 10.0, a, 1.0, min));
        // A single pane cannot be resized.
        assert!(!leaf(1).resize_toward(p(1), Direction::Right, 10.0, a, 1.0, min));
    }

    #[test]
    fn min_size_of_tree() {
        let min = Size::new(100.0, 50.0);
        assert_eq!(l_shape().min_size(1.0, min), Size::new(201.0, 101.0));
        assert_eq!(grid().min_size(1.0, min), Size::new(201.0, 101.0));
    }

    #[test]
    fn paths() {
        let l = grid();
        let path = l.path_to(p(4)).unwrap();
        assert_eq!(path, SplitPath(vec![Branch::Second, Branch::Second]));
        assert_eq!(l.node_at(&path), Some(&leaf(4)));
        let (parent, b) = path.parent().unwrap();
        assert_eq!(b, Branch::Second);
        assert_eq!(parent, SplitPath(vec![Branch::Second]));
        assert!(SplitPath::root().parent().is_none());
        assert_eq!(l.area_at(&parent, AREA, 1.0), Some(Rect::new(0.0, 301.0, 1001.0, 300.0)));
    }
}
