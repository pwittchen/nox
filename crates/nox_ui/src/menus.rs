//! Native context menus (SPEC §5.5): real `NSMenu`s from `nox_mac`; the chosen item is
//! dispatched back to the root view. Also the workspace chip menu (SPEC §5.2).

use gpui::{Context, Window};
use nox_layout::{PaneId, TabId};
use nox_mac::MenuItem;

use crate::actions::*;
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
        nox_mac::show_context_menu(
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
        crate::features::set_icon_picker(root, Some(tab), None, window, cx)
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
            root.move_tab_to_workspace(ws_idx, tab, i, window, cx)
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
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(u2))
                });
            }
            crate::pane::LinkTarget::Path(p) => {
                let (p2, p3) = (p.clone(), p.clone());
                m.add("Open", move |_, _, cx| cx.open_url(&format!("file://{}", p.display())));
                m.add("Copy Path", move |_, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(p2.to_string_lossy().into_owned()))
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
        crate::features::set_icon_picker(root, None, Some(pane), window, cx)
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
