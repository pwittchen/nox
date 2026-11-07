//! Property tests: random split / close / navigate / resize sequences keep the tree sane.

use nox_layout::*;
use proptest::prelude::*;

const AREA: Rect = Rect::new(0.0, 0.0, 1600.0, 1000.0);
const DIVIDER: f32 = 1.0;
const CELL: Size = Size::new(8.0, 16.0);
const PAD: Size = Size::new(4.0, 4.0);

#[derive(Clone, Debug)]
enum Op {
    Split { pick: usize, vertical: bool },
    Close { pick: usize },
    Focus { pick: usize },
    FocusDir(Direction),
    Next,
    Prev,
    Maximize,
    Resize(Direction, u8),
    Drag { pick: usize, frac: f32 },
    EqualizeAt { pick: usize },
    Equalize,
}

fn dir() -> impl Strategy<Value = Direction> {
    prop_oneof![Just(Direction::Left), Just(Direction::Right), Just(Direction::Up), Just(Direction::Down)]
}

fn op(with_equalize: bool) -> impl Strategy<Value = Op> {
    let base = prop_oneof![
        4 => (any::<usize>(), any::<bool>()).prop_map(|(pick, vertical)| Op::Split { pick, vertical }),
        2 => any::<usize>().prop_map(|pick| Op::Close { pick }),
        1 => any::<usize>().prop_map(|pick| Op::Focus { pick }),
        2 => dir().prop_map(Op::FocusDir),
        1 => Just(Op::Next),
        1 => Just(Op::Prev),
        1 => Just(Op::Maximize),
        2 => (dir(), 1u8..200).prop_map(|(d, n)| Op::Resize(d, n)),
        2 => (any::<usize>(), 0.0f32..1.0).prop_map(|(pick, frac)| Op::Drag { pick, frac }),
        1 => any::<usize>().prop_map(|pick| Op::EqualizeAt { pick }),
    ];
    if with_equalize { prop_oneof![10 => base, 1 => Just(Op::Equalize)].boxed() } else { base.boxed() }
}

fn check_tiling(tab: &Tab) {
    let panes = tab.panes();
    assert!(!panes.is_empty(), "tree never empty while tab alive");
    assert!(tab.is_consistent(), "{tab:?}");
    let mut sorted = panes.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), panes.len(), "duplicate panes");

    let rects = tab.layout.layout_rects(AREA, DIVIDER);
    let divs = tab.layout.dividers(AREA, DIVIDER);
    assert_eq!(rects.len(), panes.len());
    assert_eq!(divs.len(), panes.len() - 1);
    assert_eq!(rects.iter().map(|(p, _)| *p).collect::<Vec<_>>(), panes);
    let mut covered = 0.0f64;
    for (i, (_, a)) in rects.iter().enumerate() {
        assert!(a.w >= 0.0 && a.h >= 0.0);
        assert!(a.x >= AREA.x - 0.01 && a.right() <= AREA.right() + 0.01, "{a:?}");
        assert!(a.y >= AREA.y - 0.01 && a.bottom() <= AREA.bottom() + 0.01, "{a:?}");
        covered += a.area() as f64;
        for (_, b) in &rects[i + 1..] {
            assert!(a.intersection_area(b) < 0.01, "overlap {a:?} {b:?}");
        }
        for d in &divs {
            assert!(a.intersection_area(&d.rect) < 0.01, "pane over divider");
        }
    }
    for d in &divs {
        covered += d.rect.area() as f64;
    }
    let total = AREA.area() as f64;
    assert!((covered - total).abs() < 1.0, "covered {covered} of {total}");

    let visible = tab.visible_rects(AREA, DIVIDER);
    match tab.maximized {
        Some(m) => assert_eq!(visible, vec![(m, AREA)]),
        None => assert_eq!(visible, rects),
    }
}

fn pick(tab: &Tab, i: usize) -> PaneId {
    let panes = tab.panes();
    panes[i % panes.len()]
}

/// Applies ops; returns false once the tab would be closed.
fn apply(tab: &mut Tab, op: &Op, next_id: &mut u64, guarded: bool) -> bool {
    let min = min_pane_size(CELL, PAD);
    match *op {
        Op::Split { pick: i, vertical } => {
            let target = pick(tab, i);
            tab.focus(target);
            let d = if vertical { SplitDirection::Vertical } else { SplitDirection::Horizontal };
            *next_id += 1;
            let new = PaneId(*next_id);
            let before = tab.layout.len();
            let ok = if guarded {
                tab.try_split_focused(d, new, AREA, CELL, PAD, DIVIDER)
            } else {
                tab.split_focused(d, new)
            };
            if ok {
                assert_eq!(tab.layout.len(), before + 1);
                assert_eq!(tab.focused, new);
            } else {
                assert!(guarded);
                assert_eq!(tab.layout.len(), before);
            }
        }
        Op::Close { pick: i } => {
            let p = pick(tab, i);
            let before = tab.layout.len();
            match tab.close_pane(p) {
                ClosePaneResult::Closed { focus } => {
                    assert_eq!(tab.layout.len(), before - 1);
                    assert!(!tab.contains(p));
                    assert_eq!(focus, tab.focused);
                }
                ClosePaneResult::TabEmpty => {
                    assert_eq!(before, 1);
                    return false;
                }
                ClosePaneResult::NotFound => unreachable!(),
            }
        }
        Op::Focus { pick: i } => {
            let p = pick(tab, i);
            assert!(tab.focus(p));
            assert_eq!(tab.focused, p);
        }
        Op::FocusDir(d) => {
            let from = tab.focused;
            if let Some(n) = tab.focus_direction(d, AREA) {
                assert_ne!(n, from);
                // Neighbor relation is geometric: the target lies on that side.
                let a = tab.layout.rect_of(from, AREA, 0.0).unwrap();
                let b = tab.layout.rect_of(n, AREA, 0.0).unwrap();
                match d {
                    Direction::Left => assert!(b.right() <= a.x + 1.0),
                    Direction::Right => assert!(b.x >= a.right() - 1.0),
                    Direction::Up => assert!(b.bottom() <= a.y + 1.0),
                    Direction::Down => assert!(b.y >= a.bottom() - 1.0),
                }
                assert!(tab.maximized.is_none());
                // Going back returns to a pane touching the original's edge.
                assert!(tab.layout.neighbor(n, d.opposite(), AREA).is_some());
            }
        }
        Op::Next => {
            tab.focus_next();
        }
        Op::Prev => {
            tab.focus_prev();
        }
        Op::Maximize => {
            tab.toggle_maximize();
        }
        Op::Resize(d, n) => {
            tab.resize_focused(d, n as f32 * CELL.w, AREA, DIVIDER, min);
        }
        Op::Drag { pick: i, frac } => {
            let divs = tab.layout.dividers(AREA, DIVIDER);
            if !divs.is_empty() {
                let d = &divs[i % divs.len()];
                let pa = d.parent_area;
                let pointer = Point::new(pa.x + pa.w * frac, pa.y + pa.h * frac);
                let r = tab.layout.ratio_for_drag(&d.path, pointer, AREA, DIVIDER).unwrap();
                assert!((0.0..=1.0).contains(&r));
                assert!(tab.layout.set_ratio(&d.path, r, AREA, DIVIDER, min));
            }
        }
        Op::EqualizeAt { pick: i } => {
            let divs = tab.layout.dividers(AREA, DIVIDER);
            if !divs.is_empty() {
                assert!(tab.layout.equalize_at(&divs[i % divs.len()].path));
                if guarded {
                    // The UI re-applies min sizes after an equalize.
                    tab.layout.clamp_to_min_sizes(AREA, DIVIDER, min);
                }
            }
        }
        Op::Equalize => tab.equalize(),
    }
    true
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn random_ops_keep_invariants(ops in prop::collection::vec(op(true), 1..60)) {
        let mut tab = Tab::new(TabId(1), PaneId(1));
        let mut next_id = 1;
        for op in &ops {
            if !apply(&mut tab, op, &mut next_id, false) {
                break;
            }
            check_tiling(&tab);
        }
    }

    #[test]
    fn guarded_ops_respect_min_sizes(ops in prop::collection::vec(op(false), 1..80)) {
        let min = min_pane_size(CELL, PAD);
        let mut tab = Tab::new(TabId(1), PaneId(1));
        let mut next_id = 1;
        for op in &ops {
            if !apply(&mut tab, op, &mut next_id, true) {
                break;
            }
            check_tiling(&tab);
            for (p, r) in tab.layout.layout_rects(AREA, DIVIDER) {
                prop_assert!(r.w + 2.0 >= min.w && r.h + 2.0 >= min.h,
                    "pane {p} is {r:?}, min {min:?}, layout {:?}", tab.layout);
            }
        }
    }

    #[test]
    fn layout_toml_round_trip(ops in prop::collection::vec(op(true), 1..40)) {
        let mut tab = Tab::new(TabId(1), PaneId(1));
        let mut next_id = 1;
        for op in &ops {
            if !apply(&mut tab, op, &mut next_id, false) {
                break;
            }
        }
        let all = Workspaces::new(Workspace::new(WorkspaceId(1), "w", None).with_tab(tab));
        let file = all.snapshot(|p| PaneSnapshot { cwd: Some(format!("/d/{p}").into()), ..Default::default() });
        let text = file.to_toml().unwrap();
        prop_assert_eq!(SessionFile::from_toml(&text).unwrap(), file);
    }

    #[test]
    fn workspace_ops_keep_invariants(ops in prop::collection::vec((0u8..9, any::<usize>(), any::<usize>()), 1..80)) {
        let mut n = 0u64;
        let mut fresh = || { n += 1; n };
        let first = fresh();
        let mut all = Workspaces::new(Workspace::new(WorkspaceId(first), "a", None).with_tab(Tab::new(TabId(first), PaneId(first))));
        for (kind, a, b) in ops {
            let len = all.len();
            match kind {
                0 => { let id = fresh(); all.active_mut().add_tab_after_active(Tab::new(TabId(id), PaneId(id))); }
                1 => {
                    let w = all.active_mut();
                    let id = w.tabs[a % w.len()].id;
                    if w.close_tab(id) == Some(WorkspaceAfterClose::Empty) {
                        let i = all.active;
                        if matches!(all.close(i), Some(WorkspaceClose::WasLast { .. })) { break; }
                    }
                }
                2 => {
                    let id = fresh();
                    let name = all.unique_name("ws");
                    prop_assert!(!all.name_taken(&name));
                    all.push_and_switch(Workspace::new(WorkspaceId(id), name, None).with_tab(Tab::new(TabId(id), PaneId(id))));
                }
                3 => { all.switch_to(a % len); }
                4 => { all.toggle_last(); }
                5 => { all.reorder(a % len, b % len); }
                6 => {
                    let from = a % len;
                    let to = b % len;
                    let w = &all.list[from];
                    let id = w.tabs[a % w.len()].id;
                    if let Some(m) = all.move_tab_between(from, id, to) {
                        prop_assert_eq!(all.find_tab(id), Some((to, m.index)));
                        if m.source_empty && matches!(all.close(from), Some(WorkspaceClose::WasLast { .. })) {
                            break;
                        }
                    }
                }
                7 => { if a % 2 == 0 { all.next(); } else { all.prev(); } }
                _ => {
                    let w = all.active_mut();
                    match b % 4 { 0 => { w.move_tab_left(); } 1 => { w.move_tab_right(); } 2 => { w.next_tab(); } _ => { w.move_tab(a % w.len(), b % w.len()); } }
                }
            }
            prop_assert!(!all.is_empty());
            prop_assert!(all.active < all.len());
            if let Some(l) = all.last_active() {
                prop_assert!(l < all.len() && l != all.active);
            }
            let mut tab_ids = Vec::new();
            for w in &all.list {
                prop_assert!(!w.is_empty());
                prop_assert!(w.active_tab < w.len());
                tab_ids.extend(w.tabs.iter().map(|t| t.id));
            }
            let total = tab_ids.len();
            tab_ids.sort();
            tab_ids.dedup();
            prop_assert_eq!(tab_ids.len(), total);
        }
    }
}
