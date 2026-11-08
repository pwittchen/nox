//! Native context menus (SPEC §5.5): real `NSMenu`s from `nox_mac`; the chosen item is
//! dispatched back to the root view. Also the workspace chip menu (SPEC §5.2).

use gpui::{Context, Window};
use nox_layout::{PaneId, TabId};
use nox_mac::MenuItem;

use crate::actions::{
    AgentsPanel, ClearBuffer, CloseWorkspace, Copy, CopyLastOutput, NewAgentInWorktree, NewAgentTab, NewTab,
    NewWorkspace, OpenFolderAsWorkspace, Paste, RenameWorkspace, SelectAll, SendSelectionTo, SplitHorizontal,
    SplitVertical, ToggleMaximizePane,
};
use crate::root::{RenameTarget, Root};

type Handler = Box<dyn FnOnce(&mut Root, &mut Window, &mut Context<Root>)>;

/// Builds a menu: items plus the closures their tags map to.
#[derive(Default)]
struct MenuBuilder {
    items: Vec<MenuItem>,
    handlers: Vec<Handler>,
}

impl MenuBuilder {
    fn item(
        &mut self,
        title: impl Into<String>,
        f: impl FnOnce(&mut Root, &mut Window, &mut Context<Root>) + 'static,
    ) -> MenuItem {
        let tag = self.handlers.len() as i64;
        self.handlers.push(Box::new(f));
        MenuItem::new(title, tag)
    }
    fn add(
        &mut self,
        title: impl Into<String>,
        f: impl FnOnce(&mut Root, &mut Window, &mut Context<Root>) + 'static,
    ) {
        let it = self.item(title, f);
        self.items.push(it);
    }
    fn action(&mut self, title: impl Into<String>, action: impl gpui::Action + Clone) {
        self.add(title, move |_, window, cx| window.dispatch_action(Box::new(action.clone()), cx));
    }
    fn sep(&mut self) {
        self.items.push(MenuItem::separator());
    }

    fn show(self, window: &mut Window, cx: &mut Context<Root>) {
        let (tx, rx) = async_channel::bounded::<Option<i64>>(1);
        let mut handlers: Vec<Option<Handler>> = self.handlers.into_iter().map(Some).collect();
        present(
            self.items,
            Box::new(move |choice| {
                let _ = tx.try_send(choice);
            }),
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Some(tag)) = rx.recv().await
                && let Some(h) = handlers.get_mut(tag as usize).and_then(Option::take)
            {
                let _ = this.update_in(cx, move |root, window, cx| h(root, window, cx));
            }
        })
        .detach();
    }
}

/// Shows the native menu. Tests get the menu handed to them instead (no `AppKit` there).
#[cfg(not(test))]
fn present(items: Vec<MenuItem>, on_choice: Box<dyn FnOnce(Option<i64>) + Send>) {
    nox_mac::show_context_menu(items, on_choice);
}
#[cfg(test)]
use tests::present;

pub fn tab_context_menu(
    root: &mut Root,
    ws_idx: usize,
    tab: TabId,
    window: &mut Window,
    cx: &mut Context<Root>,
) {
    let mut m = MenuBuilder::default();
    m.action("New Tab", NewTab);
    m.add("Rename Tab…", move |root, window, cx| {
        if let Some(i) = root.ws.list[ws_idx].tab_index(tab) {
            root.ws.list[ws_idx].select_tab(i);
        }
        root.start_rename(RenameTarget::Tab(tab), window, cx);
    });
    m.add("Set Icon…", move |root, window, cx| {
        crate::features::set_icon_picker(root, Some(tab), None, window, cx);
    });
    m.add("Duplicate Tab", move |root, window, cx| root.duplicate_tab(ws_idx, tab, window, cx));
    m.sep();
    m.add("Move Left", move |root, _, cx| {
        if let Some(i) = root.ws.list[ws_idx].tab_index(tab) {
            let n = root.ws.list[ws_idx].tabs.len();
            root.ws.list[ws_idx].move_tab(i, (i + n - 1) % n);
            root.session_changed(cx);
            cx.notify();
        }
    });
    m.add("Move Right", move |root, _, cx| {
        if let Some(i) = root.ws.list[ws_idx].tab_index(tab) {
            let n = root.ws.list[ws_idx].tabs.len();
            root.ws.list[ws_idx].move_tab(i, (i + 1) % n);
            root.session_changed(cx);
            cx.notify();
        }
    });
    let mut sub = Vec::new();
    for (i, w) in root.ws.list.iter().enumerate() {
        if i == ws_idx {
            continue;
        }
        let it = m.item(w.name.clone(), move |root, window, cx| {
            root.move_tab_to_workspace(ws_idx, tab, i, window, cx);
        });
        sub.push(it);
    }
    if !sub.is_empty() {
        sub.push(MenuItem::separator());
    }
    sub.push(m.item("New Workspace…", move |root, window, cx| {
        let new = root.create_workspace(None, None, window, cx);
        root.move_tab_to_workspace(ws_idx, tab, new, window, cx);
    }));
    m.items.push(MenuItem::submenu("Move to Workspace", sub));
    m.sep();
    m.add("Close Tab", move |root, window, cx| root.request_close_tab(ws_idx, tab, window, cx));
    m.add("Close Other Tabs", move |root, window, cx| root.close_other_tabs(ws_idx, tab, window, cx));
    m.show(window, cx);
}

pub fn pane_context_menu(root: &mut Root, pane: PaneId, window: &mut Window, cx: &mut Context<Root>) {
    let Some(entity) = root.panes.get(&pane).cloned() else { return };
    let (has_selection, link, is_agent, has_claims, resume_flag) = {
        let p = entity.read(cx);
        let sel = p
            .session
            .as_ref()
            .is_some_and(|s| s.term().lock().selection.as_ref().is_some_and(|s| !s.is_empty()));
        (sel, p.hovered_link.clone(), p.is_agent(), !p.claims.is_empty(), p.resume_on_restore)
    };
    let mut m = MenuBuilder::default();
    if let Some(link) = link {
        match link.target {
            crate::pane::LinkTarget::Url(u) => {
                let u2 = u.clone();
                m.add("Open Link", move |_, _, cx| cx.open_url(&u));
                m.add("Copy Link", move |_, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(u2));
                });
            }
            crate::pane::LinkTarget::Path(p) => {
                let (p2, p3) = (p.clone(), p.clone());
                m.add("Open", move |_, _, cx| cx.open_url(&format!("file://{}", p.display())));
                m.add("Copy Path", move |_, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(p2.to_string_lossy().into_owned()));
                });
                m.add("Reveal in Finder", move |_, _, cx| cx.reveal_path(&p3));
            }
        }
        m.sep();
    }
    let copy = m.item("Copy", |_, window, cx| window.dispatch_action(Box::new(Copy), cx));
    m.items.push(if has_selection { copy } else { copy.disabled() });
    m.action("Copy Last Output", CopyLastOutput);
    m.action("Send Selection to…", SendSelectionTo);
    m.action("Paste", Paste);
    m.action("Select All", SelectAll);
    m.action("Clear Buffer", ClearBuffer);
    m.sep();
    m.action("Split Vertically", SplitVertical);
    m.action("Split Horizontally", SplitHorizontal);
    m.action("Maximize Pane", ToggleMaximizePane);
    m.add("Rename Pane…", move |root, window, cx| root.start_rename(RenameTarget::Pane(pane), window, cx));
    m.add("Set Icon…", move |root, window, cx| {
        crate::features::set_icon_picker(root, None, Some(pane), window, cx);
    });
    if is_agent {
        m.sep();
        m.action("Show in Agents Panel", AgentsPanel);
        if has_claims {
            m.add("Release Claims", move |root, _, cx| {
                if let Some(ws) = root.pane_workspace(pane)
                    && let Some(c) = root.coop.get_mut(&ws)
                {
                    c.claims.release_all(pane.0);
                }
                root.coop_changed(cx);
                root.resolve_waiters(cx);
            });
        }
        let it = m.item("Resume Agent on Restore", move |root, _, cx| {
            if let Some(e) = root.panes.get(&pane) {
                e.update(cx, |e, _| e.resume_on_restore = !e.resume_on_restore);
            }
            root.session_changed(cx);
        });
        m.items.push(it.checked(resume_flag));
    }
    m.sep();
    m.add("Close Pane", move |root, window, cx| root.request_close_pane(pane, window, cx));
    m.show(window, cx);
}

pub fn workspace_menu(root: &mut Root, window: &mut Window, cx: &mut Context<Root>) {
    let theme_names = nox_theme::ANSI_NAMES;
    let mut m = MenuBuilder::default();
    for (i, w) in root.ws.list.iter().enumerate() {
        let status = root.workspace_status(w, cx);
        let mark = match status {
            nox_layout::TabStatus::NeedsInput => "  ● needs input",
            nox_layout::TabStatus::Running => "  ● running",
            nox_layout::TabStatus::Done => "  ● done",
            nox_layout::TabStatus::Failed => "  ● failed",
            nox_layout::TabStatus::Bell => "  ● bell",
            nox_layout::TabStatus::None => "",
        };
        let it =
            m.item(format!("{}{mark}", w.name), move |root, window, cx| root.switch_workspace(i, window, cx));
        m.items.push(it.checked(i == root.ws.active));
    }
    m.sep();
    m.action("New Workspace…", NewWorkspace);
    m.action("Open Folder as Workspace…", OpenFolderAsWorkspace);
    m.action("New Agent…", NewAgentTab);
    m.action("New Agent in Worktree…", NewAgentInWorktree);
    m.sep();
    m.action("Rename Workspace…", RenameWorkspace);
    let mut colors = Vec::new();
    let active = root.ws.active;
    let current = root.ws.active().color.clone();
    colors.push(
        m.item("None", move |root, _, cx| {
            root.ws.list[active].color = None;
            root.session_changed(cx);
            cx.notify();
        })
        .checked(current.is_none()),
    );
    for name in theme_names.iter().take(8).skip(1) {
        let n = name.to_string();
        let checked = current.as_deref() == Some(*name);
        colors.push(
            m.item(capitalize(name), move |root, _, cx| {
                root.ws.list[active].color = Some(n);
                root.session_changed(cx);
                cx.notify();
            })
            .checked(checked),
        );
    }
    m.items.push(MenuItem::submenu("Set Color", colors));
    m.add("Set Root Folder…", crate::features::set_root_folder);
    let root_dir = root.ws.active().root.clone();
    let copy = m.item("Copy Path", {
        let r = root_dir.clone();
        move |_, _, cx| {
            if let Some(r) = r {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(r.to_string_lossy().into_owned()));
            }
        }
    });
    m.items.push(if root_dir.is_some() { copy } else { copy.disabled() });
    let reveal = m.item("Reveal in Finder", {
        let r = root_dir.clone();
        move |_, _, cx| {
            if let Some(r) = r {
                cx.reveal_path(&r);
            }
        }
    });
    m.items.push(if root_dir.is_some() { reveal } else { reveal.disabled() });
    m.sep();
    m.action("Close Workspace", CloseWorkspace);
    m.show(window, cx);
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui::{Entity, TestAppContext, VisualTestContext};

    use super::*;

    type Choice = Box<dyn FnOnce(Option<i64>) + Send>;

    thread_local! {
        static SHOWN: RefCell<Option<(Vec<MenuItem>, Choice)>> = const { RefCell::new(None) };
    }

    pub(super) fn present(items: Vec<MenuItem>, on_choice: Choice) {
        SHOWN.with(|s| *s.borrow_mut() = Some((items, on_choice)));
    }

    /// The shown menu as `title` / `title[x]` (checked) / `title(-)` (disabled), submenus as
    /// `title > a, b`; separators as `-`.
    fn outline(items: &[MenuItem]) -> Vec<String> {
        items
            .iter()
            .map(|i| {
                if i.tag < 0 && i.submenu.is_empty() && i.title.is_empty() {
                    return "-".to_string();
                }
                let mut t = i.title.clone();
                if i.checked {
                    t.push_str("[x]");
                }
                if !i.enabled {
                    t.push_str("(-)");
                }
                if !i.submenu.is_empty() {
                    t = format!("{t} > {}", outline(&i.submenu).join(", "));
                }
                t
            })
            .collect()
    }

    fn shown() -> Vec<String> {
        SHOWN.with(|s| s.borrow().as_ref().map(|(items, _)| outline(items)).unwrap_or_default())
    }

    fn find(items: &[MenuItem], path: &[&str]) -> Option<i64> {
        let item = items.iter().find(|i| i.title == path[0])?;
        if path.len() == 1 { Some(item.tag) } else { find(&item.submenu, &path[1..]) }
    }

    /// Pick an item (by title path) from the shown menu, or dismiss it with `[]`.
    fn choose(cx: &mut VisualTestContext, path: &[&str]) {
        let (items, on_choice) = SHOWN.with(|s| s.borrow_mut().take()).expect("a menu is shown");
        let tag = if path.is_empty() { None } else { Some(find(&items, path).expect("menu item")) };
        on_choice(tag);
        cx.run_until_parked();
    }

    fn root(cx: &mut TestAppContext) -> (Entity<Root>, &mut VisualTestContext) {
        crate::test_support::init(cx);
        let (root, cx) =
            cx.add_window_view(|window, cx| crate::root::Root::new(crate::root::Initial::Fresh, window, cx));
        cx.run_until_parked();
        (root, cx)
    }

    fn tab_ids(root: &Entity<Root>, cx: &mut VisualTestContext) -> Vec<TabId> {
        root.read_with(cx, |r, _| r.ws.active().tabs.iter().map(|t| t.id).collect())
    }

    #[test]
    fn capitalized_color_names() {
        assert_eq!(capitalize("red"), "Red");
        assert_eq!(capitalize("bright-blue"), "Bright-blue");
        assert_eq!(capitalize(""), "");
    }

    #[gpui::test]
    fn tab_menu_moves_and_renames(cx: &mut TestAppContext) {
        let (root, cx) = root(cx);
        root.update_in(cx, Root::new_tab);
        cx.run_until_parked();
        let before = tab_ids(&root, cx);
        assert_eq!(before.len(), 2);
        let first = before[0];

        root.update_in(cx, |r, window, cx| tab_context_menu(r, 0, first, window, cx));
        assert_eq!(
            shown(),
            [
                "New Tab",
                "Rename Tab…",
                "Set Icon…",
                "Duplicate Tab",
                "-",
                "Move Left",
                "Move Right",
                "Move to Workspace > New Workspace…",
                "-",
                "Close Tab",
                "Close Other Tabs",
            ]
        );
        choose(cx, &["Move Right"]);
        assert_eq!(tab_ids(&root, cx), [before[1], first]);

        root.update_in(cx, |r, window, cx| tab_context_menu(r, 0, first, window, cx));
        choose(cx, &["Move Right"]);
        assert_eq!(tab_ids(&root, cx), before, "wraps around");
        root.update_in(cx, |r, window, cx| tab_context_menu(r, 0, first, window, cx));
        choose(cx, &["Move Left"]);
        assert_eq!(tab_ids(&root, cx), [before[1], first]);

        // Dismissing runs nothing.
        root.update_in(cx, |r, window, cx| tab_context_menu(r, 0, first, window, cx));
        choose(cx, &[]);
        assert_eq!(tab_ids(&root, cx), [before[1], first]);

        root.update_in(cx, |r, window, cx| tab_context_menu(r, 0, first, window, cx));
        choose(cx, &["Rename Tab…"]);
        root.read_with(cx, |r, _| {
            assert!(
                matches!(r.rename.as_ref().map(|r| &r.target), Some(RenameTarget::Tab(t)) if *t == first)
            );
            assert_eq!(r.ws.active().active().map(|t| t.id), Some(first), "the tab is selected");
        });
    }

    #[gpui::test]
    fn tab_menu_duplicates_and_moves_to_workspaces(cx: &mut TestAppContext) {
        let (root, cx) = root(cx);
        let tab = tab_ids(&root, cx)[0];
        root.update_in(cx, |r, window, cx| tab_context_menu(r, 0, tab, window, cx));
        choose(cx, &["Duplicate Tab"]);
        assert_eq!(tab_ids(&root, cx).len(), 2);

        root.update_in(cx, |r, window, cx| tab_context_menu(r, 0, tab, window, cx));
        choose(cx, &["Move to Workspace", "New Workspace…"]);
        root.read_with(cx, |r, _| {
            assert_eq!(r.ws.list.len(), 2);
            assert!(r.ws.list[0].tab_index(tab).is_none());
            assert!(r.ws.list[1].tab_index(tab).is_some());
        });

        // With another workspace, it's listed by name.
        let tab2 = root.read_with(cx, |r, _| r.ws.list[0].tabs[0].id);
        root.update_in(cx, |r, window, cx| tab_context_menu(r, 0, tab2, window, cx));
        let ws1 = root.read_with(cx, |r, _| r.ws.list[1].name.clone());
        assert!(shown().contains(&format!("Move to Workspace > {ws1}, -, New Workspace…")), "{:?}", shown());
        choose(cx, &["Move to Workspace", &ws1]);
        root.read_with(cx, |r, _| {
            assert!(r.ws.list.iter().any(|w| w.tab_index(tab2).is_some() && w.tabs.len() == 2));
        });
    }

    #[gpui::test]
    fn tab_menu_new_tab_dispatches_the_action(cx: &mut TestAppContext) {
        let (root, cx) = root(cx);
        let tab = tab_ids(&root, cx)[0];
        root.update_in(cx, |r, window, cx| tab_context_menu(r, 0, tab, window, cx));
        choose(cx, &["New Tab"]);
        assert_eq!(tab_ids(&root, cx).len(), 2);
        root.update_in(cx, |r, window, cx| tab_context_menu(r, 0, tab, window, cx));
        choose(cx, &["Close Other Tabs"]);
        assert_eq!(tab_ids(&root, cx), [tab]);
    }

    #[gpui::test]
    fn pane_menu(cx: &mut TestAppContext) {
        let (root, cx) = root(cx);
        let pane = root.read_with(cx, |r, _| r.focused_pane_id().unwrap());
        root.update_in(cx, |r, window, cx| pane_context_menu(r, pane, window, cx));
        assert_eq!(
            shown(),
            [
                "Copy(-)",
                "Copy Last Output",
                "Send Selection to…",
                "Paste",
                "Select All",
                "Clear Buffer",
                "-",
                "Split Vertically",
                "Split Horizontally",
                "Maximize Pane",
                "Rename Pane…",
                "Set Icon…",
                "-",
                "Close Pane",
            ]
        );
        choose(cx, &["Split Vertically"]);
        assert_eq!(root.read_with(cx, |r, _| r.panes.len()), 2);
        root.update_in(cx, |r, window, cx| pane_context_menu(r, pane, window, cx));
        // Panes are renamed in a sheet.
        choose(cx, &["Rename Pane…"]);
        assert!(cx.update(|window, cx| crate::dialog::is_open(window, cx)));
        cx.simulate_input("worker");
        cx.simulate_keystrokes("enter");
        assert!(!cx.update(|window, cx| crate::dialog::is_open(window, cx)));
        let name = root.read_with(cx, |r, cx| r.panes[&pane].read(cx).custom_name.clone());
        assert_eq!(name.as_deref(), Some("worker"));
        // An unknown pane shows nothing.
        SHOWN.with(|s| s.borrow_mut().take());
        root.update_in(cx, |r, window, cx| pane_context_menu(r, PaneId(u64::MAX), window, cx));
        assert!(shown().is_empty());
    }

    #[gpui::test]
    fn workspace_menu_lists_workspaces_and_sets_colors(cx: &mut TestAppContext) {
        let (root, cx) = root(cx);
        root.update_in(cx, workspace_menu);
        let menu = shown();
        let name = root.read_with(cx, |r, _| r.ws.active().name.clone());
        assert_eq!(menu[0], format!("{name}[x]"));
        assert!(
            menu.contains(&"Set Color > None[x], Red, Green, Yellow, Blue, Magenta, Cyan, White".to_string())
        );
        assert!(menu.contains(&"Copy Path(-)".to_string()), "no root folder: {menu:?}");
        assert!(menu.contains(&"Reveal in Finder(-)".to_string()));
        choose(cx, &["Set Color", "Cyan"]);
        assert_eq!(root.read_with(cx, |r, _| r.ws.active().color.clone()), Some("cyan".into()));

        root.update_in(cx, workspace_menu);
        assert!(
            shown()
                .contains(&"Set Color > None, Red, Green, Yellow, Blue, Magenta, Cyan[x], White".to_string())
        );
        choose(cx, &["Set Color", "None"]);
        assert_eq!(root.read_with(cx, |r, _| r.ws.active().color.clone()), None);

        // A second workspace; switching through the menu.
        let dir = tempfile::tempdir().unwrap();
        root.update_in(cx, |r, window, cx| {
            r.create_workspace(Some("other".into()), Some(dir.path().to_path_buf()), window, cx);
        });
        cx.run_until_parked();
        root.update_in(cx, workspace_menu);
        let menu = shown();
        assert!(menu.contains(&"Copy Path".to_string()), "{menu:?}");
        choose(cx, &["Copy Path"]);
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()),
            Some(dir.path().to_string_lossy().into_owned())
        );
        root.update_in(cx, workspace_menu);
        choose(cx, &[name.as_str()]);
        assert_eq!(root.read_with(cx, |r, _| r.ws.active), 0);
    }
}
