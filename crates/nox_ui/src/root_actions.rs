//! Action handlers of the root view (SPEC §10): tabs, panes, workspaces, agents, palette and
//! pickers, view settings.

use std::fmt::Write as _;
use std::rc::Rc;

use gpui::{AppContext as _, Context, Div, Interactivity, Stateful, Window, prelude::*};
use nox_agents::AgentState;
use nox_layout::{Direction, SplitDirection};

use crate::actions::{
    ActivateLastTab, ActivateTab1, ActivateTab2, ActivateTab3, ActivateTab4, ActivateTab5, ActivateTab6,
    ActivateTab7, ActivateTab8, ActivateWorkspace1, ActivateWorkspace2, ActivateWorkspace3,
    ActivateWorkspace4, ActivateWorkspace5, ActivateWorkspace6, ActivateWorkspace7, ActivateWorkspace8,
    ActivateWorkspace9, AgentsPanel, CloseOtherTabs, ClosePane, CloseTab, CloseWorkspace, CommandPalette,
    CopyMcpSetupCommand, DuplicateTab, EqualizePanes, FocusPaneDown, FocusPaneLeft, FocusPaneRight,
    FocusPaneUp, FontBigger, FontReset, FontSmaller, InstallAgentHooks, InstallCommandLineTool,
    LastWorkspace, Minimize, MoveTabLeft, MoveTabRight, NewAgentInWorktree, NewAgentTab, NewTab,
    NewWorkspace, NextAgent, NextPane, NextTab, NextWorkspace, OpenConfigFile, OpenFolderAsWorkspace,
    OpenSettings, PreviousPane, PreviousTab, PreviousWorkspace, Quit, RenamePane, RenameTab, RenameWorkspace,
    ResizePaneDown, ResizePaneLeft, ResizePaneRight, ResizePaneUp, ResumeAllAgents, SendSelectionTo,
    SendText, SetWorkspaceColor, SplitHorizontal, SplitVertical, SwitchAppearance, SwitchTab, SwitchTheme,
    SwitchWorkspace, TabsPositionBottom, TabsPositionLeft, TabsPositionTop, ToggleFullScreen,
    ToggleMaximizePane, ToggleSidebar, Zoom,
};
use crate::palette::{Palette, PaletteItem, PaletteOptions, Source};
use crate::pane::{PaneSpawn, Startup};
use crate::root::{RenameTarget, Root};
use crate::settings_state::Settings;
use crate::theme::Theme;

macro_rules! on {
    ($el:expr, $cx:expr, $action:ty, $body:expr) => {
        $el.on_action($cx.listener(
            move |this: &mut Root, _: &$action, window: &mut Window, cx: &mut Context<Root>| {
                #[allow(clippy::redundant_closure_call)]
                ($body)(this, window, cx)
            },
        ))
    };
}

pub fn register(el: Stateful<Div>, cx: &mut Context<Root>) -> Stateful<Div> {
    let el = on!(el, cx, NewTab, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| r.new_tab(w, cx));
    let el = on!(el, cx, ClosePane, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        if let Some(p) = r.focused_pane_id() {
            r.request_close_pane(p, w, cx);
        }
    });
    let el = on!(el, cx, CloseTab, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        let a = r.ws.active;
        if let Some(t) = r.active_tab().map(|t| t.id) {
            r.request_close_tab(a, t, w, cx);
        }
    });
    let el = on!(el, cx, ActivateTab1, |r: &mut Root, w: &mut Window, cx| r.activate_tab(0, w, cx));
    let el = on!(el, cx, ActivateTab2, |r: &mut Root, w: &mut Window, cx| r.activate_tab(1, w, cx));
    let el = on!(el, cx, ActivateTab3, |r: &mut Root, w: &mut Window, cx| r.activate_tab(2, w, cx));
    let el = on!(el, cx, ActivateTab4, |r: &mut Root, w: &mut Window, cx| r.activate_tab(3, w, cx));
    let el = on!(el, cx, ActivateTab5, |r: &mut Root, w: &mut Window, cx| r.activate_tab(4, w, cx));
    let el = on!(el, cx, ActivateTab6, |r: &mut Root, w: &mut Window, cx| r.activate_tab(5, w, cx));
    let el = on!(el, cx, ActivateTab7, |r: &mut Root, w: &mut Window, cx| r.activate_tab(6, w, cx));
    let el = on!(el, cx, ActivateTab8, |r: &mut Root, w: &mut Window, cx| r.activate_tab(7, w, cx));
    let el = on!(el, cx, ActivateLastTab, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        if r.ws.active_mut().select_last() {
            r.after_tab_switch(w, cx);
        }
    });
    let el = on!(el, cx, PreviousTab, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        r.ws.active_mut().prev_tab();
        r.after_tab_switch(w, cx);
    });
    let el = on!(el, cx, NextTab, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        r.ws.active_mut().next_tab();
        r.after_tab_switch(w, cx);
    });
    let el = on!(el, cx, MoveTabLeft, |r: &mut Root, _w: &mut Window, cx: &mut Context<Root>| {
        r.ws.active_mut().move_tab_left();
        r.session_changed(cx);
        cx.notify();
    });
    let el = on!(el, cx, MoveTabRight, |r: &mut Root, _w: &mut Window, cx: &mut Context<Root>| {
        r.ws.active_mut().move_tab_right();
        r.session_changed(cx);
        cx.notify();
    });
    let el = on!(el, cx, RenameTab, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        if let Some(t) = r.active_tab().map(|t| t.id) {
            r.start_rename(RenameTarget::Tab(t), w, cx);
        }
    });
    let el = on!(el, cx, RenamePane, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        if let Some(p) = r.focused_pane_id() {
            r.start_rename(RenameTarget::Pane(p), w, cx);
        }
    });
    let el = on!(el, cx, DuplicateTab, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        let a = r.ws.active;
        if let Some(t) = r.active_tab().map(|t| t.id) {
            r.duplicate_tab(a, t, w, cx);
        }
    });
    let el = on!(el, cx, CloseOtherTabs, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        let a = r.ws.active;
        if let Some(t) = r.active_tab().map(|t| t.id) {
            r.close_other_tabs(a, t, w, cx);
        }
    });
    let el = on!(el, cx, SplitVertical, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        r.split(SplitDirection::Vertical, None, w, cx);
    });
    let el = on!(el, cx, SplitHorizontal, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        r.split(SplitDirection::Horizontal, None, w, cx);
    });
    let el =
        on!(el, cx, FocusPaneLeft, |r: &mut Root, w: &mut Window, cx| focus_dir(r, Direction::Left, w, cx));
    let el =
        on!(el, cx, FocusPaneRight, |r: &mut Root, w: &mut Window, cx| focus_dir(r, Direction::Right, w, cx));
    let el = on!(el, cx, FocusPaneUp, |r: &mut Root, w: &mut Window, cx| focus_dir(r, Direction::Up, w, cx));
    let el =
        on!(el, cx, FocusPaneDown, |r: &mut Root, w: &mut Window, cx| focus_dir(r, Direction::Down, w, cx));
    let el = on!(el, cx, PreviousPane, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        if let Some(t) = r.ws.active_mut().active_mut() {
            if t.maximized.is_some() {
                t.toggle_maximize();
            }
            t.focus_prev();
        }
        r.sync_maximized(cx);
        r.focus_active(w, cx);
        cx.notify();
    });
    let el = on!(el, cx, NextPane, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        if let Some(t) = r.ws.active_mut().active_mut() {
            if t.maximized.is_some() {
                t.toggle_maximize();
            }
            t.focus_next();
        }
        r.sync_maximized(cx);
        r.focus_active(w, cx);
        cx.notify();
    });
    let el =
        on!(el, cx, ResizePaneLeft, |r: &mut Root, w: &mut Window, cx| resize(r, Direction::Left, w, cx));
    let el =
        on!(el, cx, ResizePaneRight, |r: &mut Root, w: &mut Window, cx| resize(r, Direction::Right, w, cx));
    let el = on!(el, cx, ResizePaneUp, |r: &mut Root, w: &mut Window, cx| resize(r, Direction::Up, w, cx));
    let el =
        on!(el, cx, ResizePaneDown, |r: &mut Root, w: &mut Window, cx| resize(r, Direction::Down, w, cx));
    let el = on!(el, cx, EqualizePanes, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        let area = r.layout_area(w, cx);
        let min = r.min_pane(w, cx);
        if let Some(t) = r.ws.active_mut().active_mut() {
            t.equalize();
            t.layout.clamp_to_min_sizes(area, crate::root::DIVIDER_PX, min);
        }
        r.session_changed(cx);
        cx.notify();
    });
    let el = on!(el, cx, ToggleMaximizePane, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        if let Some(t) = r.ws.active_mut().active_mut() {
            t.toggle_maximize();
        }
        r.sync_maximized(cx);
        r.focus_active(w, cx);
        r.session_changed(cx);
        cx.notify();
    });

    // Workspaces.
    let el = on!(el, cx, NewWorkspace, |r: &mut Root, w: &mut Window, cx| r.new_workspace_sheet(w, cx));
    let el = on!(el, cx, OpenFolderAsWorkspace, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        let force = w.modifiers().alt;
        r.prompt_open_folder(force, w, cx);
    });
    let el =
        on!(el, cx, SwitchWorkspace, |r: &mut Root, w: &mut Window, cx| open_workspace_switcher(r, w, cx));
    let el = on!(el, cx, ActivateWorkspace1, |r: &mut Root, w: &mut Window, cx| r.switch_workspace(0, w, cx));
    let el = on!(el, cx, ActivateWorkspace2, |r: &mut Root, w: &mut Window, cx| r.switch_workspace(1, w, cx));
    let el = on!(el, cx, ActivateWorkspace3, |r: &mut Root, w: &mut Window, cx| r.switch_workspace(2, w, cx));
    let el = on!(el, cx, ActivateWorkspace4, |r: &mut Root, w: &mut Window, cx| r.switch_workspace(3, w, cx));
    let el = on!(el, cx, ActivateWorkspace5, |r: &mut Root, w: &mut Window, cx| r.switch_workspace(4, w, cx));
    let el = on!(el, cx, ActivateWorkspace6, |r: &mut Root, w: &mut Window, cx| r.switch_workspace(5, w, cx));
    let el = on!(el, cx, ActivateWorkspace7, |r: &mut Root, w: &mut Window, cx| r.switch_workspace(6, w, cx));
    let el = on!(el, cx, ActivateWorkspace8, |r: &mut Root, w: &mut Window, cx| r.switch_workspace(7, w, cx));
    let el = on!(el, cx, ActivateWorkspace9, |r: &mut Root, w: &mut Window, cx| r.switch_workspace(8, w, cx));
    let el = on!(el, cx, PreviousWorkspace, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        r.ws.prev();
        let a = r.ws.active;
        r.switch_workspace(a, w, cx);
    });
    let el = on!(el, cx, NextWorkspace, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        r.ws.next();
        let a = r.ws.active;
        r.switch_workspace(a, w, cx);
    });
    let el = on!(el, cx, LastWorkspace, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        if r.ws.toggle_last() {
            let a = r.ws.active;
            r.switch_workspace(a, w, cx);
        }
    });
    let el = on!(el, cx, CloseWorkspace, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        let a = r.ws.active;
        r.request_close_workspace(a, w, cx);
    });
    let el = on!(el, cx, RenameWorkspace, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        let id = r.ws.active().id;
        r.start_rename(RenameTarget::Workspace(id), w, cx);
    });
    let el = on!(el, cx, SetWorkspaceColor, |r: &mut Root, w: &mut Window, cx| open_color_picker(r, w, cx));

    // Agents.
    let el = on!(el, cx, NewAgentTab, |r: &mut Root, w: &mut Window, cx| open_agent_picker(r, w, cx));
    let el =
        on!(el, cx, NewAgentInWorktree, |r: &mut Root, w: &mut Window, cx| r.new_agent_in_worktree(w, cx));
    let el = on!(el, cx, AgentsPanel, |r: &mut Root, w: &mut Window, cx| open_agents_panel(r, w, cx));
    let el = on!(el, cx, NextAgent, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        let waiting = r.waiting_agents(cx);
        if waiting.is_empty() {
            crate::toast::show(cx, "No agent is waiting for you.");
            return;
        }
        // Cycle: skip agents already visited in this round.
        r.next_agent_cursor.retain(|p| waiting.contains(p));
        let next = waiting.iter().find(|p| !r.next_agent_cursor.contains(p)).copied().unwrap_or_else(|| {
            r.next_agent_cursor.clear();
            waiting[0]
        });
        r.next_agent_cursor.push(next);
        r.reveal_pane(next, w, cx);
    });
    let el = on!(el, cx, SendSelectionTo, |r: &mut Root, w: &mut Window, cx| open_send_to(r, w, cx));
    let el = on!(el, cx, InstallAgentHooks, |_r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        crate::features::install_agent_hooks(w, cx);
    });
    let el =
        on!(el, cx, CopyMcpSetupCommand, |r: &mut Root, w: &mut Window, cx| crate::features::copy_mcp_setup(
            r, w, cx
        ));
    let el = on!(el, cx, ResumeAllAgents, |r: &mut Root, w: &mut Window, cx| r.resume_all_agents(w, cx));
    let el = on!(el, cx, InstallCommandLineTool, |_r: &mut Root, w: &mut Window, cx: &mut Context<Root>| {
        crate::features::install_cli(w, cx);
    });

    // View.
    let el = on!(el, cx, CommandPalette, |r: &mut Root, w: &mut Window, cx| open_command_palette(r, w, cx));
    let el = on!(el, cx, SwitchTab, |r: &mut Root, w: &mut Window, cx| open_tab_switcher(r, w, cx));
    let el = on!(el, cx, ToggleSidebar, |_r: &mut Root, _w: &mut Window, cx: &mut Context<Root>| {
        let t = &Settings::global(cx).config.tabs;
        // Left mode hides / shows the sidebar; top / bottom mode swaps the tab bar for it.
        if t.position == nox_config::TabsPosition::Left {
            let v = !t.sidebar_visible;
            crate::config_io::set_value(cx, "tabs.sidebar_visible", v);
        } else {
            let v = !t.sidebar_replaces_tabs;
            crate::config_io::set_value(cx, "tabs.sidebar_replaces_tabs", v);
        }
    });
    let el = on!(el, cx, FontBigger, |r: &mut Root, w: &mut Window, cx| font_step(r, 1.0, w, cx));
    let el = on!(el, cx, FontSmaller, |r: &mut Root, w: &mut Window, cx| font_step(r, -1.0, w, cx));
    let el = on!(el, cx, FontReset, |r: &mut Root, w: &mut Window, cx| font_step(r, 0.0, w, cx));
    let el = on!(el, cx, SwitchTheme, |r: &mut Root, w: &mut Window, cx| open_theme_picker(r, w, cx));
    let el =
        on!(el, cx, SwitchAppearance, |r: &mut Root, w: &mut Window, cx| open_appearance_picker(r, w, cx));
    let el = on!(el, cx, TabsPositionTop, |_r: &mut Root, _w: &mut Window, cx: &mut Context<Root>| {
        crate::config_io::set_value(cx, "tabs.position", "top");
    });
    let el = on!(el, cx, TabsPositionBottom, |_r: &mut Root, _w: &mut Window, cx: &mut Context<Root>| {
        crate::config_io::set_value(cx, "tabs.position", "bottom");
    });
    let el = on!(el, cx, TabsPositionLeft, |_r: &mut Root, _w: &mut Window, cx: &mut Context<Root>| {
        crate::config_io::set_value(cx, "tabs.position", "left");
    });
    let el = on!(el, cx, OpenSettings, |_r: &mut Root, _w: &mut Window, cx: &mut Context<Root>| {
        crate::settings::open_settings_window(cx);
    });
    let el =
        on!(el, cx, OpenConfigFile, |r: &mut Root, w: &mut Window, cx| crate::features::open_config_file(
            r, w, cx
        ));
    let el = on!(el, cx, ToggleFullScreen, |_r: &mut Root, w: &mut Window, _cx: &mut Context<Root>| w
        .toggle_fullscreen());
    let el =
        on!(el, cx, Minimize, |_r: &mut Root, w: &mut Window, _cx: &mut Context<Root>| w.minimize_window());
    let el = on!(el, cx, Zoom, |_r: &mut Root, w: &mut Window, _cx: &mut Context<Root>| w.zoom_window());
    let el = on!(el, cx, SendText, |_r: &mut Root, _w: &mut Window, _cx: &mut Context<Root>| {});
    on!(el, cx, Quit, |r: &mut Root, w: &mut Window, cx: &mut Context<Root>| request_quit(r, w, cx))
}

fn focus_dir(r: &mut Root, dir: Direction, w: &mut Window, cx: &mut Context<Root>) {
    let area = r.layout_area(w, cx);
    if let Some(t) = r.ws.active_mut().active_mut() {
        t.focus_direction(dir, area);
    }
    r.sync_maximized(cx);
    r.focus_active(w, cx);
    cx.notify();
}

fn resize(r: &mut Root, dir: Direction, w: &mut Window, cx: &mut Context<Root>) {
    let area = r.layout_area(w, cx);
    let min = r.min_pane(w, cx);
    let (cell, _) = r.cell_and_padding(w, cx);
    let delta = match dir {
        Direction::Left | Direction::Right => cell.w,
        _ => cell.h,
    };
    if let Some(t) = r.ws.active_mut().active_mut() {
        t.resize_focused(dir, delta, area, crate::root::DIVIDER_PX, min);
    }
    r.session_changed(cx);
    cx.notify();
}

fn font_step(r: &mut Root, delta: f32, _w: &mut Window, cx: &mut Context<Root>) {
    let settings = Settings::global(cx);
    let base = settings.term_font.size;
    let per_pane = settings.config.font.size_applies_to == nox_config::SizeAppliesTo::Pane;
    if per_pane {
        if let Some(p) = r.focused_pane() {
            p.update(cx, |p, cx| {
                let cur = p.font_size_override.unwrap_or(base);
                p.font_size_override = if delta == 0.0 { None } else { Some((cur + delta).clamp(6.0, 72.0)) };
                cx.notify();
            });
        }
    } else {
        let cur = r.font_override.unwrap_or(base);
        r.font_override = if delta == 0.0 { None } else { Some((cur + delta).clamp(6.0, 72.0)) };
        let f = r.font_override;
        for p in r.panes.values() {
            p.update(cx, |p, cx| {
                p.font_size_override = f;
                cx.notify();
            });
        }
    }
    cx.notify();
}

pub fn request_quit(r: &mut Root, w: &mut Window, cx: &mut Context<Root>) {
    let busy: Vec<String> = r
        .panes
        .values()
        .filter(|p| p.read(cx).is_busy() && !p.read(cx).is_agent())
        .filter_map(|p| p.read(cx).process_display_name())
        .collect();
    let agents = r.panes.values().filter(|p| p.read(cx).agent_state() == Some(AgentState::Working)).count();
    if (!busy.is_empty() || agents > 0) && Settings::global(cx).config.app.confirm_close {
        let mut msg = String::new();
        if !busy.is_empty() {
            let _ = write!(msg, "Still running: {}. ", busy.join(", "));
        }
        if agents > 0 {
            let _ = write!(msg, "{agents} agent(s) are working; they'll be resumed next time.");
        }
        let this = cx.entity().downgrade();
        crate::dialog::confirm(w, cx, "Quit nox?", msg, "Quit", move |_, cx| {
            let _ = this.update(cx, |r, cx| {
                r.save_session_now(cx);
                cx.quit();
            });
        });
        return;
    }
    r.save_session_now(cx);
    cx.quit();
}

// ---- palettes --------------------------------------------------------------------------------

fn shortcut_hint(action: &dyn gpui::Action, w: &Window) -> Option<String> {
    let binding = w.bindings_for_action(action).into_iter().last()?;
    Some(
        binding
            .keystrokes()
            .iter()
            .map(|k| crate::util_keys::pretty_keystroke(k.inner()))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn open_command_palette(r: &mut Root, w: &mut Window, cx: &mut Context<Root>) {
    // Snapshot shortcut hints now (needs the window's keymap).
    let commands: Vec<(String, Box<dyn gpui::Action>, Option<String>)> = crate::actions::palette_commands()
        .into_iter()
        .map(|(label, a)| {
            let hint = shortcut_hint(a.as_ref(), w);
            (label.to_string(), a, hint)
        })
        .collect();
    let commands = Rc::new(commands);
    let source: Source = Rc::new(move |_, _| {
        commands
            .iter()
            .map(|(label, action, hint)| {
                let a = action.boxed_clone();
                let mut it = PaletteItem::new(label.clone())
                    .on_select(move |window, cx| window.dispatch_action(a.boxed_clone(), cx));
                if let Some(h) = hint {
                    it = it.hint(h.clone());
                }
                it
            })
            .collect()
    });
    let palette = cx.new(|cx| Palette::new(source, PaletteOptions::default(), w, cx));
    r.open_palette(palette, w, cx);
}

fn open_tab_switcher(r: &mut Root, w: &mut Window, cx: &mut Context<Root>) {
    let this = cx.entity().downgrade();
    let mut items = Vec::new();
    let theme = Theme::global(cx).clone();
    for (wi, ws) in r.ws.list.iter().enumerate() {
        for (ti, t) in ws.tabs.iter().enumerate() {
            let title = r.tab_title(t, ti, cx);
            let cwd = r.panes.get(&t.focused).map(|p| p.read(cx).cwd_display()).unwrap_or_default();
            let icon =
                r.panes.get(&t.focused).map_or(nox_agents::Icon::TERMINAL, |p| p.read(cx).icon.clone());
            let pane = t.focused;
            let this = this.clone();
            items.push(
                PaletteItem::new(title)
                    .detail(format!("{} · {cwd}", ws.name))
                    .icon(
                        crate::icons::icon_path(&icon),
                        Some(crate::icons::icon_color(&icon, theme.text_muted)),
                    )
                    .dot(crate::chrome::status_dot(&theme, r.tab_status(t, cx)))
                    .keywords(ws.name.clone())
                    .on_select(move |window, cx| {
                        let _ = this.update(cx, |r, cx| r.reveal_pane(pane, window, cx));
                    }),
            );
            let _ = wi;
        }
    }
    let items = Rc::new(items);
    let source: Source = Rc::new(move |_, _| (*items).clone());
    let palette = cx.new(|cx| {
        Palette::new(
            source,
            PaletteOptions { placeholder: "Switch to tab…".into(), ..Default::default() },
            w,
            cx,
        )
    });
    r.open_palette(palette, w, cx);
}

fn open_workspace_switcher(r: &mut Root, w: &mut Window, cx: &mut Context<Root>) {
    let this = cx.entity().downgrade();
    let theme = Theme::global(cx).clone();
    let mut items = Vec::new();
    for (i, ws) in r.ws.list.iter().enumerate() {
        let mut counts = [0usize; 3];
        for t in &ws.tabs {
            for p in t.panes() {
                match r.panes.get(&p).and_then(|e| e.read(cx).agent_state()) {
                    Some(AgentState::NeedsInput) => counts[0] += 1,
                    Some(AgentState::Working) => counts[1] += 1,
                    Some(AgentState::Idle) => counts[2] += 1,
                    _ => {}
                }
            }
        }
        let mut hint = Vec::new();
        if counts[0] > 0 {
            hint.push(format!("{} needs input", counts[0]));
        }
        if counts[1] > 0 {
            hint.push(format!("{} working", counts[1]));
        }
        if counts[2] > 0 {
            hint.push(format!("{} idle", counts[2]));
        }
        let root_dir = ws.root.as_ref().map_or_else(
            || if ws.root_missing { "root missing".into() } else { "no root".into() },
            |p| nox_terminal::process::tildify(p),
        );
        let swatch = ws.color.as_deref().and_then(|c| theme.named(c)).or(Some(theme.text_muted.opacity(0.4)));
        let status = crate::chrome::status_dot(&theme, r.workspace_status(ws, cx));
        let this = this.clone();
        items.push(
            PaletteItem::new(ws.name.clone())
                .detail(root_dir)
                .hint(if hint.is_empty() { format!("⌃⌘{}", i + 1) } else { hint.join(" · ") })
                .dot(status.or(swatch))
                .on_select(move |window, cx| {
                    let _ = this.update(cx, |r, cx| r.switch_workspace(i, window, cx));
                }),
        );
    }
    let this2 = cx.entity().downgrade();
    items.push(PaletteItem::new("New Workspace…").on_select(move |window, cx| {
        let _ = this2.update(cx, |r, cx| r.new_workspace_sheet(window, cx));
    }));
    let items = Rc::new(items);
    let source: Source = Rc::new(move |_, _| (*items).clone());
    let active = r.ws.active;
    let palette = cx.new(|cx| {
        Palette::new(
            source,
            PaletteOptions {
                placeholder: "Switch workspace…".into(),
                preselect: Some(active),
                ..Default::default()
            },
            w,
            cx,
        )
    });
    r.open_palette(palette, w, cx);
}

fn open_color_picker(r: &mut Root, w: &mut Window, cx: &mut Context<Root>) {
    let this = cx.entity().downgrade();
    let idx = r.ws.active;
    let source: Source = Rc::new(move |_, cx| {
        let theme = Theme::global(cx).clone();
        let mut v = vec![{
            let this = this.clone();
            PaletteItem::new("None").on_select(move |_, cx| {
                let _ = this.update(cx, |r, cx| {
                    r.ws.list[idx].color = None;
                    r.session_changed(cx);
                    cx.notify();
                });
            })
        }];
        for name in nox_theme::ANSI_NAMES.iter().take(8).skip(1) {
            let this = this.clone();
            let n = name.to_string();
            v.push(PaletteItem::new(*name).dot(theme.named(name)).on_select(move |_, cx| {
                let n = n.clone();
                let _ = this.update(cx, |r, cx| {
                    r.ws.list[idx].color = Some(n);
                    r.session_changed(cx);
                    cx.notify();
                });
            }));
        }
        v
    });
    let palette = cx.new(|cx| {
        Palette::new(
            source,
            PaletteOptions { placeholder: "Workspace color…".into(), ..Default::default() },
            w,
            cx,
        )
    });
    r.open_palette(palette, w, cx);
}

fn open_agent_picker(r: &mut Root, w: &mut Window, cx: &mut Context<Root>) {
    let this = cx.entity().downgrade();
    let defs: Vec<(String, String)> =
        Settings::global(cx).config.agents.defs.iter().map(|(k, d)| (k.clone(), d.command.clone())).collect();
    let ws = r.ws.active;
    let source: Source = Rc::new(move |_, cx| {
        let theme = Theme::global(cx).clone();
        let reg = cx.global::<crate::icons::IconRegistry>();
        defs.iter()
            .map(|(k, cmd)| {
                let icon = reg.table.icon_for_kind(k);
                let this = this.clone();
                let k2 = k.clone();
                PaletteItem::new(k.clone())
                    .detail(cmd.clone())
                    .icon(crate::icons::icon_path(&icon), Some(crate::icons::icon_color(&icon, theme.text)))
                    .on_select(move |window, cx| {
                        let _ = this.update(cx, |r, cx| r.launch_agent(ws, &k2, None, window, cx));
                    })
            })
            .collect()
    });
    let palette = cx.new(|cx| {
        Palette::new(
            source,
            PaletteOptions { placeholder: "New agent tab…".into(), ..Default::default() },
            w,
            cx,
        )
    });
    r.open_palette(palette, w, cx);
}

fn open_send_to(r: &mut Root, w: &mut Window, cx: &mut Context<Root>) {
    let Some(src) = r.focused_pane() else { return };
    let text = {
        let p = src.read(cx);
        p.session
            .as_ref()
            .and_then(|s| s.term().lock().selection_to_string().filter(|t| !t.trim().is_empty()))
            .or_else(|| p.session.as_ref().and_then(nox_terminal::TerminalSession::last_output))
    };
    let Some(text) = text else {
        crate::toast::show(cx, "Nothing to send: no selection and no last command output.");
        return;
    };
    let src_id = r.focused_pane_id();
    let ws = r.ws.active().id;
    let this = cx.entity().downgrade();
    let theme = Theme::global(cx).clone();
    let mut panes: Vec<(bool, nox_layout::PaneId, String, String, nox_agents::Icon)> = Vec::new();
    if let Some(i) = r.ws.index_of(ws) {
        for (ti, t) in r.ws.list[i].tabs.iter().enumerate() {
            for p in t.panes() {
                if Some(p) == src_id {
                    continue;
                }
                let Some(e) = r.panes.get(&p) else { continue };
                let e = e.read(cx);
                panes.push((e.is_agent(), p, r.pane_name(p, cx), r.tab_title(t, ti, cx), e.icon.clone()));
            }
        }
    }
    panes.sort_by_key(|(agent, ..)| !*agent);
    let text = Rc::new(text);
    let items: Vec<PaletteItem> = panes
        .into_iter()
        .map(|(agent, p, name, tab, icon)| {
            let this = this.clone();
            let text = text.clone();
            PaletteItem::new(name)
                .detail(tab)
                .hint(if agent { "agent" } else { "" })
                .icon(crate::icons::icon_path(&icon), Some(crate::icons::icon_color(&icon, theme.text_muted)))
                .on_select(move |window, cx| {
                    let text = text.clone();
                    let _ = this.update(cx, |r, cx| {
                        if let Some(e) = r.panes.get(&p).cloned() {
                            e.update(cx, |e, _| {
                                if let Some(s) = &e.session {
                                    s.paste_bracketed(&text, false);
                                }
                            });
                        }
                        r.reveal_pane(p, window, cx);
                    });
                })
        })
        .collect();
    let items = Rc::new(items);
    let source: Source = Rc::new(move |_, _| (*items).clone());
    let palette = cx.new(|cx| {
        Palette::new(
            source,
            PaletteOptions {
                placeholder: "Send selection to…".into(),
                keep_order: true,
                ..Default::default()
            },
            w,
            cx,
        )
    });
    r.open_palette(palette, w, cx);
}

fn open_agents_panel(r: &mut Root, w: &mut Window, cx: &mut Context<Root>) {
    // Build both lists now: the palette's source must not read `Root` while it is updating.
    let lists = Rc::new([agents_items(r, cx), activity_items(r)]);
    let source: Source = Rc::new(move |tab, _| lists[tab.min(1)].clone());
    let palette = cx.new(|cx| {
        Palette::new(
            source,
            PaletteOptions {
                placeholder: "Agents…".into(),
                tabs: vec!["Agents".into(), "Activity".into()],
                keep_order: true,
                footer: "↑↓ navigate   ↵ jump   ⇥ activity   esc close".into(),
                ..Default::default()
            },
            w,
            cx,
        )
    });
    r.open_palette(palette, w, cx);
}

/// The Activity log of the active workspace, newest first.
fn activity_items(root: &Root) -> Vec<PaletteItem> {
    let ws = root.ws.active().id;
    let Some(coop) = root.coop.get(&ws) else { return vec![] };
    let items: Vec<PaletteItem> = coop
        .activity
        .newest_first()
        .map(|e| {
            let ago = e.ts.elapsed().map(crate::util::short_duration).unwrap_or_default();
            PaletteItem::new(format!("{} · {:?}", e.source_name, e.kind))
                .detail(e.detail.clone())
                .hint(format!("{ago} ago"))
        })
        .collect();
    if items.is_empty() {
        return vec![PaletteItem::header("No messages, sends, spawns or claims yet in this workspace.")];
    }
    items
}

/// Every agent of every workspace, grouped by workspace: needs input first, then idle-unseen,
/// then working (SPEC §7.12).
fn agents_items(root: &Root, cx: &mut Context<Root>) -> Vec<PaletteItem> {
    let this = cx.entity().downgrade();
    let theme = Theme::global(cx).clone();
    let mut groups: Vec<PaletteItem> = Vec::new();
    for w in &root.ws.list {
        let mut rows: Vec<(u8, PaletteItem)> = Vec::new();
        for (ti, t) in w.tabs.iter().enumerate() {
            for p in t.panes() {
                let Some(e) = root.panes.get(&p) else { continue };
                let e = e.read(cx);
                let Some(a) = &e.agent else { continue };
                if a.tracker.state() == AgentState::Exited {
                    continue;
                }
                let dur = crate::util::short_duration(a.tracker.since().elapsed());
                let (rank, state) = match a.tracker.state() {
                    AgentState::NeedsInput => (0, format!("needs input · {dur}")),
                    AgentState::Idle if e.done_unseen => (1, "idle · unseen".to_string()),
                    AgentState::Working => (2, format!("working · {dur}")),
                    _ => (3, "idle".to_string()),
                };
                let mut detail = vec![root.tab_title(t, ti, cx)];
                if let Some(m) = a.tracker.message() {
                    detail.push(m.to_string());
                }
                detail.push(e.cwd_display());
                if let Some(b) = &e.branch {
                    detail.push(format!("⎇ {b}"));
                }
                if e.unread > 0 {
                    detail.push(format!("✉ {}", e.unread));
                }
                if !e.claims.is_empty() {
                    detail.push(format!("{} claim(s)", e.claims.len()));
                }
                let color = match a.tracker.state() {
                    AgentState::NeedsInput => Some(theme.ansi(5)),
                    AgentState::Working => Some(theme.ansi(3)),
                    _ => None,
                };
                let this2 = this.clone();
                rows.push((
                    rank,
                    PaletteItem::new(root.pane_name(p, cx))
                        .detail(detail.join(" · "))
                        .hint(state)
                        .dot(color)
                        .icon(
                            crate::icons::icon_path(&e.icon),
                            Some(crate::icons::icon_color(&e.icon, theme.text_muted)),
                        )
                        .keywords(w.name.clone())
                        .on_select(move |window, cx| {
                            let _ = this2.update(cx, |r, cx| r.reveal_pane(p, window, cx));
                        }),
                ));
            }
        }
        if rows.is_empty() {
            continue;
        }
        rows.sort_by_key(|(r, _)| *r);
        groups.push(PaletteItem::header(w.name.clone()));
        groups.extend(rows.into_iter().map(|(_, i)| i));
    }
    if groups.is_empty() {
        groups.push(PaletteItem::header("No agents are running. ⌥⌘T starts one."));
    }
    groups
}

fn open_theme_picker(r: &mut Root, w: &mut Window, cx: &mut Context<Root>) {
    let original = cx.global::<crate::theme::ThemeState>().clone();
    let names = nox_theme::available_theme_names(&nox_config::themes_dir());
    let current = original.file.name.clone();
    let preselect = names.iter().position(|n| *n == current);
    let this = cx.entity().downgrade();
    let orig2 = original.clone();
    let this_cancel = this.clone();
    let source: Source = Rc::new(move |_, _| {
        names
            .iter()
            .map(|name| {
                let (n1, n2) = (name.clone(), name.clone());
                let this = this.clone();
                let this2 = this.clone();
                let mode = orig2.mode;
                PaletteItem::new(name.clone())
                    .on_highlight(move |window, cx| {
                        let (file, _) = crate::theme::load_theme_file(&n1);
                        let state = crate::theme::ThemeState { file, mode, notices: vec![] };
                        cx.set_global(state.clone());
                        let appearance = window.appearance();
                        crate::theme::apply(cx, &state, appearance);
                        let _ = this.update(cx, |r, cx| r.on_theme_changed(window, cx));
                    })
                    .on_select(move |window, cx| {
                        crate::config_io::set_value(cx, "ui.theme", n2.as_str());
                        let _ = this2.update(cx, |r, cx| r.on_theme_changed(window, cx));
                    })
            })
            .collect()
    });
    let on_cancel: crate::palette::Action = Rc::new(move |window, cx| {
        cx.set_global(original.clone());
        let appearance = window.appearance();
        crate::theme::apply(cx, &original, appearance);
        let _ = this_cancel.update(cx, |r, cx| r.on_theme_changed(window, cx));
    });
    let palette = cx.new(|cx| {
        Palette::new(
            source,
            PaletteOptions {
                placeholder: "Switch theme…".into(),
                on_cancel: Some(on_cancel),
                keep_order: true,
                preselect,
                footer: "↑↓ preview   ↵ apply   esc restore".into(),
                ..Default::default()
            },
            w,
            cx,
        )
    });
    r.open_palette(palette, w, cx);
}

fn open_appearance_picker(r: &mut Root, w: &mut Window, cx: &mut Context<Root>) {
    let original = cx.global::<crate::theme::ThemeState>().clone();
    if original.file.is_single_variant() {
        crate::toast::show(
            cx,
            format!("“{}” has a single variant; appearance can't change.", original.file.name),
        );
        return;
    }
    let this = cx.entity().downgrade();
    let this_cancel = this.clone();
    let orig2 = original.clone();
    let modes = [
        ("System", nox_config::Mode::System, "system"),
        ("Light", nox_config::Mode::Light, "light"),
        ("Dark", nox_config::Mode::Dark, "dark"),
    ];
    let preselect = modes.iter().position(|(_, m, _)| *m == original.mode);
    let source: Source = Rc::new(move |_, _| {
        modes
            .iter()
            .map(|(label, mode, key)| {
                let (mode, key) = (*mode, *key);
                let this = this.clone();
                let this2 = this.clone();
                let base = orig2.clone();
                PaletteItem::new(*label)
                    .on_highlight(move |window, cx| {
                        let state = crate::theme::ThemeState { mode, ..base.clone() };
                        cx.set_global(state.clone());
                        let appearance = window.appearance();
                        crate::theme::apply(cx, &state, appearance);
                        let _ = this.update(cx, |r, cx| r.on_theme_changed(window, cx));
                    })
                    .on_select(move |window, cx| {
                        crate::config_io::set_value(cx, "ui.mode", key);
                        let _ = this2.update(cx, |r, cx| r.on_theme_changed(window, cx));
                    })
            })
            .collect()
    });
    let on_cancel: crate::palette::Action = Rc::new(move |window, cx| {
        cx.set_global(original.clone());
        let appearance = window.appearance();
        crate::theme::apply(cx, &original, appearance);
        let _ = this_cancel.update(cx, |r, cx| r.on_theme_changed(window, cx));
    });
    let palette = cx.new(|cx| {
        Palette::new(
            source,
            PaletteOptions {
                placeholder: "Switch appearance…".into(),
                on_cancel: Some(on_cancel),
                keep_order: true,
                preselect,
                footer: "↑↓ preview   ↵ apply   esc restore".into(),
                ..Default::default()
            },
            w,
            cx,
        )
    });
    r.open_palette(palette, w, cx);
}

#[allow(dead_code)]
fn _unused(_: PaneSpawn, _: Startup, _: Interactivity) {}
