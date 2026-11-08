//! The window root (SPEC §4.1, §5.2–5.4, §13): title bar, workspace chip, tab bar / sidebar,
//! the active workspace's pane layout, and overlays (palette, sheets, notices).
//!
//! `Root` owns every workspace (`nox_layout::Workspaces`: workspaces → tabs → split trees,
//! pure data) and maps each `PaneId` to its `Entity<Pane>`. Panes of background workspaces and
//! inactive tabs keep running; only the visible tab renders.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, Global, Pixels, Subscription,
    Task, Window, WindowAppearance, prelude::*, px,
};
use nox_agents::{ActivityLog, AgentState, Claims, MessageBus, RateLimiter};
use nox_layout::{
    ClosePaneResult, Direction, PaneId, Rect, Size, SplitDirection, SplitPath, Tab, TabId, TabStatus,
    Workspace, WorkspaceAfterClose, WorkspaceId, Workspaces,
};

use crate::palette::{Palette, PaletteEvent};
use crate::pane::{AgentLaunch, Pane, PaneEvent, PaneSpawn, Startup};
use crate::settings_state::Settings;
use crate::theme::Theme;

pub const DIVIDER_PX: f32 = 1.0;
pub const DIVIDER_HIT_PX: f32 = 6.0;
pub const HEADER_PX: f32 = 28.0;
pub const TITLEBAR_TOP_PX: f32 = 38.0;
pub const TITLEBAR_SLIM_PX: f32 = 28.0;
pub const BOTTOM_BAR_PX: f32 = 34.0;
pub const TRAFFIC_LIGHTS_PX: f32 = 78.0;

/// UI state that pane headers need to read.
#[derive(Default)]
pub struct UiFlags {
    pub maximized_panes: HashSet<PaneId>,
}
impl Global for UiFlags {}

/// Per-workspace cooperation state (SPEC §7.13).
pub struct Coop {
    pub bus: MessageBus,
    pub claims: Claims,
    pub activity: ActivityLog,
}

impl Coop {
    #[must_use]
    pub fn new(ws: WorkspaceId) -> Self {
        let path = nox_config::nox_dir().join("workspaces").join(ws.to_string()).join("messages.jsonl");
        let bus = MessageBus::load(&path).unwrap_or_else(|e| {
            log::warn!("message bus for workspace {ws}: {e}");
            MessageBus::new()
        });
        Self { bus, claims: Claims::new(), activity: ActivityLog::new(ActivityLog::DEFAULT_CAP) }
    }
}

pub enum RenameTarget {
    Tab(TabId),
    Pane(PaneId),
    Workspace(WorkspaceId),
}

pub struct Rename {
    pub target: RenameTarget,
    pub input: Entity<crate::text_input::TextInput>,
    _sub: Subscription,
}

pub struct Root {
    pub ws: Workspaces,
    pub panes: HashMap<PaneId, Entity<Pane>>,
    pane_subs: HashMap<PaneId, Subscription>,
    /// Panes of not-yet-shown workspaces (lazy restore): spawned on first show.
    pub pending: HashMap<PaneId, PaneSpawn>,
    pub coop: HashMap<WorkspaceId, Coop>,
    pub palette: Option<(Entity<Palette>, Subscription)>,
    pub rename: Option<Rename>,
    pub drag_divider: Option<SplitPath>,
    pub drag_sidebar: bool,
    pub focus_handle: FocusHandle,
    pub ipc: Option<crate::coop::IpcState>,
    pub attention: crate::attention::Attention,
    pub rate: RateLimiter,
    pub saved_agents: HashMap<PaneId, nox_layout::AgentSnapshot>,
    pub session_task: Option<Task<()>>,
    pub session_soft_task: Option<Task<()>>,
    pub next_agent_cursor: Vec<PaneId>,
    pub font_override: Option<f32>,
    pub window_active: bool,
    pub appearance: WindowAppearance,
    /// Set once the last tab / workspace closed and the app is quitting: the session is already
    /// saved, so later saves (debounced, on-quit hook) must not overwrite it.
    pub quitting: bool,
    _subs: Vec<Subscription>,
    pub tasks: Vec<Task<()>>,
}

impl Focusable for Root {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Initial contents for a new window.
pub enum Initial {
    Fresh,
    Restored(Box<crate::persist::RestorePlan>),
}

impl Root {
    pub fn new(initial: Initial, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.set_global(UiFlags::default());
        let focus_handle = cx.focus_handle();
        let appearance = window.appearance();
        let mut subs = vec![];
        subs.push(cx.observe_window_appearance(window, |this: &mut Self, window, cx| {
            this.appearance = window.appearance();
            let state = cx.global::<crate::theme::ThemeState>().clone();
            if crate::theme::apply(cx, &state, this.appearance) {
                this.on_theme_changed(window, cx);
            }
        }));
        subs.push(cx.observe_window_activation(window, |this: &mut Self, window, cx| {
            this.window_active = window.is_window_active();
            if this.window_active {
                this.mark_visible_seen(cx);
            }
            cx.notify();
        }));

        let placeholder = Workspace::new(WorkspaceId::next(), "default", None);
        let mut this = Self {
            ws: Workspaces::new(placeholder),
            panes: HashMap::new(),
            pane_subs: HashMap::new(),
            pending: HashMap::new(),
            coop: HashMap::new(),
            palette: None,
            rename: None,
            drag_divider: None,
            drag_sidebar: false,
            focus_handle,
            ipc: None,
            attention: crate::attention::Attention::default(),
            rate: RateLimiter::new(),
            saved_agents: HashMap::new(),
            session_task: None,
            session_soft_task: None,
            next_agent_cursor: vec![],
            font_override: None,
            window_active: true,
            appearance,
            quitting: false,
            _subs: subs,
            tasks: vec![],
        };
        this.start_ipc(window, cx);
        match initial {
            Initial::Restored(plan) => this.apply_restore(*plan, window, cx),
            Initial::Fresh => {
                let id = this.ws.active().id;
                this.coop.insert(id, Coop::new(id));
                let home = default_cwd(cx);
                this.add_tab(0, PaneSpawn { cwd: Some(home), ..Default::default() }, window, cx);
            }
        }
        this.focus_active(window, cx);
        this
    }

    // ---- lookup helpers ---------------------------------------------------------------------

    #[must_use]
    pub fn active_tab(&self) -> Option<&Tab> {
        self.ws.active().active()
    }

    #[must_use]
    pub fn focused_pane_id(&self) -> Option<PaneId> {
        self.active_tab().map(|t| t.focused)
    }

    #[must_use]
    pub fn focused_pane(&self) -> Option<Entity<Pane>> {
        self.focused_pane_id().and_then(|id| self.panes.get(&id).cloned())
    }

    #[must_use]
    pub fn pane_workspace(&self, pane: PaneId) -> Option<WorkspaceId> {
        self.ws.find_pane(pane).map(|(w, _)| self.ws.list[w].id)
    }

    /// Is this pane what the user is looking at (focused window, active workspace & tab)?
    #[must_use]
    pub fn is_looked_at(&self, pane: PaneId) -> bool {
        if !self.window_active {
            return false;
        }
        match self.ws.find_pane(pane) {
            Some((w, t)) => w == self.ws.active && t == self.ws.list[w].active_tab,
            None => false,
        }
    }

    pub fn tab_status(&self, tab: &Tab, cx: &App) -> TabStatus {
        nox_layout::aggregate_status(
            tab.panes().into_iter().filter_map(|p| self.panes.get(&p)).map(|p| p.read(cx).status()),
        )
    }

    pub fn workspace_status(&self, ws: &Workspace, cx: &App) -> TabStatus {
        nox_layout::aggregate_status(ws.tabs.iter().map(|t| self.tab_status(t, cx)))
    }

    pub fn tab_title(&self, tab: &Tab, index: usize, cx: &App) -> String {
        let settings = Settings::global(cx);
        let name = match &tab.custom_name {
            Some(n) => n.clone(),
            None => self
                .panes
                .get(&tab.focused)
                .map_or_else(|| "~".into(), |p| p.read(cx).title(settings.config.tabs.title)),
        };
        if settings.config.tabs.show_index { format!("{} {}", index + 1, name) } else { name }
    }

    /// The agent name of a pane (SPEC §4.8): its custom name, else the agent kind, suffixed when
    /// it repeats in the workspace.
    pub fn pane_name(&self, pane: PaneId, cx: &App) -> String {
        let Some(p) = self.panes.get(&pane) else { return pane.to_string() };
        let p = p.read(cx);
        if let Some(n) = &p.custom_name {
            return n.clone();
        }
        let Some(kind) = p.agent.as_ref().map(|a| a.kind.clone()) else {
            return format!("pane-{pane}");
        };
        // Order same-kind agents in the workspace by pane id for stable suffixes.
        let Some((w, _)) = self.ws.find_pane(pane) else { return kind };
        let mut same: Vec<PaneId> = self.ws.list[w]
            .tabs
            .iter()
            .flat_map(nox_layout::Tab::panes)
            .filter(|id| {
                self.panes.get(id).is_some_and(|e| {
                    let e = e.read(cx);
                    e.custom_name.is_none() && e.agent.as_ref().is_some_and(|a| a.kind == kind)
                })
            })
            .collect();
        same.sort();
        let idx = same.iter().position(|id| *id == pane).unwrap_or(0);
        if idx == 0 { kind } else { format!("{kind}-{}", idx + 1) }
    }

    // ---- panes & tabs -----------------------------------------------------------------------

    fn pane_env(&mut self, ws_idx: usize, tab: TabId, pane: PaneId) -> HashMap<String, String> {
        let w = &self.ws.list[ws_idx];
        let mut env: HashMap<String, String> = w.env.clone().into_iter().collect();
        env.insert(nox_ipc::ENV_WORKSPACE.into(), w.name.clone());
        env.insert(nox_ipc::ENV_WORKSPACE_ID.into(), w.id.to_string());
        if let Some(root) = &w.root {
            env.insert(nox_ipc::ENV_WORKSPACE_ROOT.into(), root.to_string_lossy().into_owned());
        }
        env.insert(nox_ipc::ENV_TAB_ID.into(), tab.to_string());
        env.insert(nox_ipc::ENV_PANE_ID.into(), pane.to_string());
        if let Some(ipc) = &self.ipc {
            let token = ipc.handle.tokens().issue(nox_ipc::Caller {
                pane_id: pane.0,
                tab_id: tab.0,
                workspace_id: w.id.0,
            });
            env.insert(nox_ipc::ENV_SOCKET.into(), ipc.handle.socket_path().to_string_lossy().into_owned());
            env.insert(nox_ipc::ENV_TOKEN.into(), token);
        }
        if let Some(bin) = cli_bin_dir() {
            let path = std::env::var("PATH").unwrap_or_default();
            env.insert("PATH".into(), format!("{}:{}", bin.display(), path));
            // The shell integration re-adds it if rc files reset PATH.
            env.insert("NOX_BIN_DIR".into(), bin.to_string_lossy().into_owned());
        }
        env
    }

    /// Create the pane entity (spawning its shell).
    pub fn spawn_pane(
        &mut self,
        pane: PaneId,
        ws_idx: usize,
        tab: TabId,
        mut spawn: PaneSpawn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Pane> {
        let env = self.pane_env(ws_idx, tab, pane);
        spawn.env.extend(env);
        let entity = cx.new(|cx| Pane::new(pane, spawn, window, cx));
        if let Some(f) = self.font_override {
            entity.update(cx, |p, _| p.font_size_override = Some(f));
        }
        let sub = cx.subscribe_in(&entity, window, move |this: &mut Self, _, ev: &PaneEvent, window, cx| {
            this.on_pane_event(pane, ev, window, cx);
        });
        self.panes.insert(pane, entity.clone());
        self.pane_subs.insert(pane, sub);
        entity
    }

    /// Spawn panes of a workspace that were deferred by lazy restore.
    pub fn ensure_started(&mut self, ws_idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        let todo: Vec<(PaneId, TabId)> = self.ws.list[ws_idx]
            .tabs
            .iter()
            .flat_map(|t| t.panes().into_iter().map(move |p| (p, t.id)))
            .filter(|(p, _)| self.pending.contains_key(p))
            .collect();
        for (p, t) in todo {
            if let Some(spawn) = self.pending.remove(&p) {
                self.spawn_pane(p, ws_idx, t, spawn, window, cx);
            }
        }
    }

    /// Working directory for a new tab / pane (SPEC §4.5, §4.7).
    pub fn new_cwd(&self, ws_idx: usize, cx: &App) -> PathBuf {
        let settings = Settings::global(cx);
        let w = &self.ws.list[ws_idx];
        if w.tabs.is_empty() {
            if let Some(root) = &w.root {
                return root.clone();
            }
            return default_cwd(cx);
        }
        let mode = settings.config.workspaces.new_tab_cwd;
        if mode == nox_config::NewTabCwd::Root
            && let Some(root) = &w.root
        {
            return root.clone();
        }
        if settings.config.shell.inherit_cwd
            && let Some(tab) = w.active()
            && let Some(p) = self.panes.get(&tab.focused)
            && let Some(cwd) = p.read(cx).cwd.clone()
        {
            return cwd;
        }
        w.root.clone().unwrap_or_else(|| default_cwd(cx))
    }

    /// Open a new tab after the active one in workspace `ws_idx`.
    pub fn add_tab(
        &mut self,
        ws_idx: usize,
        spawn: PaneSpawn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PaneId {
        let pane = PaneId::next();
        let tab_id = TabId::next();
        let tab = Tab::new(tab_id, pane);
        self.ws.list[ws_idx].add_tab_after_active(tab);
        self.spawn_pane(pane, ws_idx, tab_id, spawn, window, cx);
        if ws_idx == self.ws.active {
            self.focus_active(window, cx);
        }
        self.session_changed(cx);
        cx.notify();
        pane
    }

    pub fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ws_idx = self.ws.active;
        let cwd = self.new_cwd(ws_idx, cx);
        self.add_tab(ws_idx, PaneSpawn { cwd: Some(cwd), ..Default::default() }, window, cx);
    }

    /// Split the focused pane; refused with a shake when either side would be too small.
    pub fn split(
        &mut self,
        dir: SplitDirection,
        spawn: Option<PaneSpawn>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PaneId> {
        let ws_idx = self.ws.active;
        let area = self.layout_area(window, cx);
        let (cell, pad) = self.cell_and_padding(window, cx);
        let tab = self.ws.list[ws_idx].active()?;
        let focused = tab.focused;
        let tab_id = tab.id;
        let can = tab.layout.can_split(focused, dir, area, cell, pad, DIVIDER_PX);
        if !can {
            if let Some(p) = self.panes.get(&focused) {
                if Settings::global(cx).reduce_motion() {
                    crate::toast::show(cx, "Not enough room to split this pane.");
                } else {
                    p.update(cx, super::pane::Pane::shake);
                }
            }
            return None;
        }
        let cwd = self.new_cwd(ws_idx, cx);
        let spawn = spawn.unwrap_or(PaneSpawn { cwd: Some(cwd), ..Default::default() });
        let pane = PaneId::next();
        let tab = self.ws.list[ws_idx].active_mut()?;
        tab.split_focused(dir, pane);
        self.spawn_pane(pane, ws_idx, tab_id, spawn, window, cx);
        self.sync_maximized(cx);
        self.focus_active(window, cx);
        self.session_changed(cx);
        cx.notify();
        Some(pane)
    }

    /// Close a pane after confirming if something runs in it.
    pub fn request_close_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let busy = self.panes.get(&pane).is_some_and(|p| p.read(cx).is_busy());
        if busy && Settings::global(cx).config.app.confirm_close {
            let what = self
                .panes
                .get(&pane)
                .and_then(|p| p.read(cx).process_display_name())
                .unwrap_or_else(|| "A process".into());
            let this = cx.entity().downgrade();
            crate::dialog::confirm(
                window,
                cx,
                "Close this pane?",
                format!("{what} is still running in it."),
                "Close",
                move |window, cx| {
                    let _ = this.update(cx, |this, cx| this.close_pane(pane, window, cx));
                },
            );
            return;
        }
        self.close_pane(pane, window, cx);
    }

    pub fn close_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((w, t)) = self.ws.find_pane(pane) else { return };
        let tab = &mut self.ws.list[w].tabs[t];
        match tab.close_pane(pane) {
            ClosePaneResult::Closed { .. } => {
                self.drop_pane(pane, cx);
            }
            ClosePaneResult::TabEmpty => {
                let tab_id = tab.id;
                self.close_tab_now(w, tab_id, window, cx);
                return;
            }
            ClosePaneResult::NotFound => return,
        }
        self.sync_maximized(cx);
        self.focus_active(window, cx);
        self.session_changed(cx);
        cx.notify();
    }

    fn drop_pane(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        if let Some(ws) = self.pane_workspace(pane)
            && let Some(coop) = self.coop.get_mut(&ws)
        {
            coop.claims.release_all(pane.0);
        }
        if let Some(ipc) = &self.ipc {
            ipc.handle.tokens().revoke_pane(pane.0);
        }
        nox_mac::remove_notification(&crate::attention::notification_id(pane));
        self.pane_subs.remove(&pane);
        if let Some(p) = self.panes.remove(&pane) {
            p.update(cx, |p, _| {
                if let Some(s) = &p.session {
                    s.kill();
                }
            });
        }
        self.pending.remove(&pane);
        self.saved_agents.remove(&pane);
        self.rate.forget(pane.0);
        self.coop_changed(cx);
    }

    pub fn request_close_tab(
        &mut self,
        w: usize,
        tab_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.ws.list[w].tab(tab_id) else { return };
        let busy: Vec<String> = tab
            .panes()
            .iter()
            .filter_map(|p| self.panes.get(p))
            .filter(|p| p.read(cx).is_busy())
            .filter_map(|p| p.read(cx).process_display_name())
            .collect();
        if !busy.is_empty() && Settings::global(cx).config.app.confirm_close {
            let this = cx.entity().downgrade();
            crate::dialog::confirm(
                window,
                cx,
                "Close this tab?",
                format!("Still running: {}.", busy.join(", ")),
                "Close",
                move |window, cx| {
                    let _ = this.update(cx, |this, cx| this.close_tab_now(w, tab_id, window, cx));
                },
            );
            return;
        }
        self.close_tab_now(w, tab_id, window, cx);
    }

    pub fn close_tab_now(&mut self, w: usize, tab_id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panes) = self.ws.list.get(w).and_then(|ws| ws.tab(tab_id)).map(nox_layout::Tab::panes)
        else {
            return;
        };
        let worktree = self.ws.list[w]
            .tab(tab_id)
            .and_then(|t| self.panes.get(&t.focused).and_then(|p| p.read(cx).cwd.clone()));
        for p in panes {
            self.drop_pane(p, cx);
        }
        match self.ws.list[w].close_tab(tab_id) {
            Some(WorkspaceAfterClose::Empty) => {
                if Settings::global(cx).config.workspaces.close_when_empty
                    || self.ws.len() == 1 && Self::quit_on_last(cx)
                {
                    self.close_workspace_now(w, window, cx);
                } else {
                    let cwd = self.ws.list[w].root.clone().unwrap_or_else(|| default_cwd(cx));
                    self.add_tab(w, PaneSpawn { cwd: Some(cwd), ..Default::default() }, window, cx);
                }
            }
            Some(WorkspaceAfterClose::Remaining) | None => {}
        }
        if self.quitting {
            return;
        }
        if let Some(path) = worktree {
            crate::features::offer_worktree_removal(&path, window, cx);
        }
        self.mark_visible_seen(cx);
        self.sync_maximized(cx);
        self.focus_active(window, cx);
        self.session_changed(cx);
        cx.notify();
    }

    fn quit_on_last(cx: &App) -> bool {
        Settings::global(cx).config.app.quit_when_last_tab_closed
    }

    pub fn close_workspace_now(&mut self, w: usize, window: &mut Window, cx: &mut Context<Self>) {
        let panes: Vec<PaneId> = self.ws.list[w].tabs.iter().flat_map(nox_layout::Tab::panes).collect();
        for p in panes {
            self.drop_pane(p, cx);
        }
        let id = self.ws.list[w].id;
        match self.ws.close(w) {
            Some(nox_layout::WorkspaceClose::WasLast { .. }) => {
                self.coop.remove(&id);
                if Self::quit_on_last(cx) {
                    self.save_session_now(cx);
                    self.quitting = true;
                    self.session_task = None;
                    // `cx.quit()` is asynchronous: keep `ws` non-empty (an empty placeholder, no
                    // panes) so callers and renders before the app exits still see a valid state.
                    self.ws = Workspaces::new(Workspace::new(WorkspaceId::next(), "default", None));
                    cx.quit();
                    return;
                }
                let ws = Workspace::new(WorkspaceId::next(), "default", None);
                let nid = ws.id;
                self.ws = Workspaces::new(ws);
                self.coop.insert(nid, Coop::new(nid));
                let home = default_cwd(cx);
                self.add_tab(0, PaneSpawn { cwd: Some(home), ..Default::default() }, window, cx);
            }
            Some(nox_layout::WorkspaceClose::Closed { .. }) => {
                self.coop.remove(&id);
                let a = self.ws.active;
                self.ensure_started(a, window, cx);
            }
            None => {}
        }
        self.mark_visible_seen(cx);
        self.focus_active(window, cx);
        self.session_changed(cx);
        cx.notify();
    }

    pub fn request_close_workspace(&mut self, w: usize, window: &mut Window, cx: &mut Context<Self>) {
        let mut running = 0;
        let mut agents = 0;
        for t in &self.ws.list[w].tabs {
            for p in t.panes() {
                if let Some(p) = self.panes.get(&p) {
                    let p = p.read(cx);
                    if p.agent_state() == Some(AgentState::Working) {
                        agents += 1;
                    } else if p.is_busy() {
                        running += 1;
                    }
                }
            }
        }
        if (running > 0 || agents > 0) && Settings::global(cx).config.app.confirm_close {
            let name = self.ws.list[w].name.clone();
            let this = cx.entity().downgrade();
            crate::dialog::confirm(
                window,
                cx,
                format!("Close workspace “{name}”?"),
                format!("{running} running process(es) and {agents} working agent(s) will be stopped."),
                "Close",
                move |window, cx| {
                    let _ = this.update(cx, |this, cx| this.close_workspace_now(w, window, cx));
                },
            );
            return;
        }
        self.close_workspace_now(w, window, cx);
    }

    pub fn activate_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.ws.active_mut().select_tab(index) {
            self.after_tab_switch(window, cx);
        }
    }

    pub fn after_tab_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rename = None;
        self.mark_visible_seen(cx);
        self.sync_maximized(cx);
        self.focus_active(window, cx);
        self.session_changed(cx);
        cx.notify();
    }

    pub fn switch_workspace(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        if idx >= self.ws.len() {
            return;
        }
        if idx != self.ws.active {
            self.ws.switch_to(idx);
        }
        self.ensure_started(idx, window, cx);
        self.after_tab_switch(window, cx);
    }

    /// Focus pane `pane` wherever it is: switch workspace and tab first.
    pub fn reveal_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((w, t)) = self.ws.find_pane(pane) else { return };
        if w != self.ws.active {
            self.ws.switch_to(w);
            self.ensure_started(w, window, cx);
        }
        self.ws.list[w].select_tab(t);
        if let Some(tab) = self.ws.list[w].tabs.get_mut(t) {
            tab.focus(pane);
        }
        self.after_tab_switch(window, cx);
    }

    /// Clear done / bell marks and notifications of what the user now sees.
    pub fn mark_visible_seen(&mut self, cx: &mut Context<Self>) {
        if !self.window_active {
            return;
        }
        let Some(tab) = self.ws.active().active() else { return };
        for p in tab.panes() {
            if let Some(e) = self.panes.get(&p) {
                e.update(cx, |p, _| p.mark_seen());
                self.attention.visited(p);
            }
        }
    }

    pub fn sync_maximized(&mut self, cx: &mut Context<Self>) {
        let set: HashSet<PaneId> =
            self.ws.list.iter().flat_map(|w| w.tabs.iter().filter_map(|t| t.maximized)).collect();
        cx.global_mut::<UiFlags>().maximized_panes = set;
    }

    pub fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if crate::dialog::is_open(window, cx) || self.palette.is_some() || self.rename.is_some() {
            return;
        }
        if let Some(p) = self.focused_pane() {
            let h = p.read(cx).focus_handle.clone();
            window.focus(&h);
        } else {
            window.focus(&self.focus_handle);
        }
    }

    // ---- geometry ---------------------------------------------------------------------------

    pub fn sidebar_shown(&self, cx: &App) -> bool {
        let t = &Settings::global(cx).config.tabs;
        if t.position == nox_config::TabsPosition::Left { t.sidebar_visible } else { t.sidebar_replaces_tabs }
    }

    /// Whether the tab strip and workspace chip show (top / bottom mode, sidebar not toggled on).
    pub fn tab_bar_shown(&self, cx: &App) -> bool {
        let t = &Settings::global(cx).config.tabs;
        t.position != nox_config::TabsPosition::Left && !t.sidebar_replaces_tabs
    }

    pub fn sidebar_width(&self, cx: &App) -> f32 {
        (Settings::global(cx).config.tabs.sidebar_width as f32).clamp(160., 360.)
    }

    pub fn titlebar_height(&self, cx: &App) -> f32 {
        match Settings::global(cx).config.tabs.position {
            nox_config::TabsPosition::Bottom => TITLEBAR_SLIM_PX,
            _ => TITLEBAR_TOP_PX,
        }
    }

    /// The pane layout area inside the card, in window coordinates.
    pub fn layout_area(&self, window: &Window, cx: &App) -> Rect {
        let vp = window.viewport_size();
        let pad = Settings::global(cx).config.ui.window_padding as f32;
        let top = self.titlebar_height(cx);
        let bottom = if Settings::global(cx).config.tabs.position == nox_config::TabsPosition::Bottom {
            BOTTOM_BAR_PX
        } else {
            0.
        };
        let left = if self.sidebar_shown(cx) { self.sidebar_width(cx) + 1. } else { 0. };
        // Card is inset by `pad`; its 1px border is inside the inset.
        let x = left + pad + 1.;
        let y = top + 1.;
        let w = f32::from(vp.width) - x - pad - 1.;
        let h = f32::from(vp.height) - y - pad - bottom - 1.;
        Rect::new(x, y, w.max(1.), h.max(1.))
    }

    pub fn cell_and_padding(&self, window: &Window, cx: &App) -> (Size, Size) {
        let s = Settings::global(cx);
        let size = self.font_override.unwrap_or(s.term_font.size);
        let (cw, ch) = crate::terminal_element::cell_metrics(window, &s.term_font, size);
        let pad = s.config.terminal.padding;
        let header = if s.config.ui.pane_header { HEADER_PX } else { 0. };
        (Size::new(f32::from(cw), f32::from(ch)), Size::new(pad[0] as f32 * 2., pad[1] as f32 * 2. + header))
    }

    pub fn min_pane(&self, window: &Window, cx: &App) -> Size {
        let (cell, pad) = self.cell_and_padding(window, cx);
        nox_layout::min_pane_size(cell, pad)
    }

    // ---- events -----------------------------------------------------------------------------

    fn on_pane_event(&mut self, pane: PaneId, ev: &PaneEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            PaneEvent::Focused => {
                if let Some((w, t)) = self.ws.find_pane(pane)
                    && w == self.ws.active
                {
                    let ws = &mut self.ws.list[w];
                    if ws.active_tab != t {
                        ws.select_tab(t);
                    }
                    if let Some(tab) = ws.tabs.get_mut(t)
                        && tab.focused != pane
                    {
                        tab.focus(pane);
                        self.sync_maximized(cx);
                    }
                }
                self.mark_visible_seen(cx);
                cx.notify();
            }
            PaneEvent::TitleChanged => {
                self.session_soft_changed(cx);
                cx.notify();
            }
            PaneEvent::UserInput => {
                if let Some(p) = self.panes.get(&pane) {
                    p.update(cx, |p, _| p.mark_seen());
                }
            }
            PaneEvent::Bell => {
                if !self.is_looked_at(pane)
                    && let Some(p) = self.panes.get(&pane)
                {
                    p.update(cx, |p, _| p.bell_unseen = true);
                }
                if !self.window_active && Settings::global(cx).config.app.dock_bounce_on_bell {
                    nox_mac::request_attention();
                }
                cx.notify();
            }
            PaneEvent::CommandFinished { command, exit, duration } => {
                let looked_at = self.is_looked_at(pane);
                if !looked_at && let Some(p) = self.panes.get(&pane) {
                    p.update(cx, |p, _| p.done_unseen = true);
                }
                let min = Duration::from_secs(Settings::global(cx).config.terminal.attention_min_duration);
                let is_agent = self.panes.get(&pane).is_some_and(|p| p.read(cx).is_agent());
                if !looked_at && *duration >= min && !is_agent {
                    let tab_title = self.pane_tab_title(pane, cx);
                    let (title, body) = crate::attention::done_text(command, *exit, &tab_title, *duration);
                    self.trigger_attention(pane, &title, &body, cx);
                }
                cx.notify();
            }
            PaneEvent::Notify { title, body } => {
                let process = self
                    .panes
                    .get(&pane)
                    .and_then(|p| p.read(cx).process_display_name())
                    .unwrap_or_else(|| "nox".into());
                if !self.is_looked_at(pane) {
                    let t = title.clone().unwrap_or(process);
                    self.trigger_attention(pane, &t, body, cx);
                }
            }
            PaneEvent::AgentChanged { transition } => {
                self.on_agent_changed(pane, *transition, cx);
            }
            PaneEvent::Exited { code } => {
                let policy = Settings::global(cx).config.terminal.close_on_exit;
                let close = match policy {
                    nox_config::CloseOnExit::Always => true,
                    nox_config::CloseOnExit::Clean => *code == Some(0),
                    nox_config::CloseOnExit::Never => false,
                };
                if let Some(ws) = self.pane_workspace(pane)
                    && let Some(coop) = self.coop.get_mut(&ws)
                {
                    coop.claims.release_all(pane.0);
                }
                self.resolve_waiters(cx);
                if close {
                    self.close_pane(pane, window, cx);
                }
            }
            PaneEvent::Close => self.request_close_pane(pane, window, cx),
            PaneEvent::Split(dir) => {
                if let Some((w, t)) = self.ws.find_pane(pane)
                    && w == self.ws.active
                {
                    self.ws.list[w].select_tab(t);
                    self.ws.list[w].tabs[t].focus(pane);
                    self.split(*dir, None, window, cx);
                }
            }
            PaneEvent::ToggleMaximize => {
                if let Some((w, t)) = self.ws.find_pane(pane) {
                    let tab = &mut self.ws.list[w].tabs[t];
                    tab.focus(pane);
                    tab.toggle_maximize();
                    self.sync_maximized(cx);
                    self.focus_active(window, cx);
                    self.session_changed(cx);
                    cx.notify();
                }
            }
            PaneEvent::ContextMenu => crate::menus::pane_context_menu(self, pane, window, cx),
        }
    }

    pub fn pane_tab_title(&self, pane: PaneId, cx: &App) -> String {
        match self.ws.find_pane(pane) {
            Some((w, t)) => self.tab_title(&self.ws.list[w].tabs[t], t, cx),
            None => String::new(),
        }
    }

    pub fn on_agent_changed(
        &mut self,
        pane: PaneId,
        transition: Option<nox_agents::Transition>,
        cx: &mut Context<Self>,
    ) {
        // Keep awake while any agent works.
        let any_working = self.panes.values().any(|p| p.read(cx).agent_state() == Some(AgentState::Working));
        let keep = Settings::global(cx).config.agents.keep_awake && any_working;
        nox_mac::set_keep_awake(keep, "nox: a coding agent is working");

        if let Some(t) = transition {
            let min = Duration::from_secs(Settings::global(cx).config.terminal.attention_min_duration);
            if t.wants_attention(min) && !self.is_looked_at(pane) {
                if t.to == AgentState::Idle
                    && let Some(p) = self.panes.get(&pane)
                {
                    p.update(cx, |p, _| p.done_unseen = true);
                }
                let name = self.pane_name(pane, cx);
                let msg = self.panes.get(&pane).and_then(|p| {
                    p.read(cx).agent.as_ref().and_then(|a| a.tracker.message().map(str::to_string))
                });
                let wsname = self
                    .pane_workspace(pane)
                    .and_then(|id| self.ws.get(id))
                    .map(|w| w.name.clone())
                    .unwrap_or_default();
                let tab = self.pane_tab_title(pane, cx);
                let (title, body) = crate::attention::agent_text(&name, t.to, msg.as_deref(), &wsname, &tab);
                self.trigger_attention(pane, &title, &body, cx);
            }
            if t.to == AgentState::Exited
                && let Some(ws) = self.pane_workspace(pane)
                && let Some(coop) = self.coop.get_mut(&ws)
            {
                coop.claims.release_all(pane.0);
            }
        }
        self.resolve_waiters(cx);
        self.deliver_typed_messages(cx);
        self.session_soft_changed(cx);
        cx.notify();
    }

    pub fn trigger_attention(&mut self, pane: PaneId, title: &str, body: &str, cx: &mut Context<Self>) {
        let cfg = Settings::global(cx).config.clone();
        self.attention.trigger(pane, title, body, &cfg);
    }

    pub fn on_theme_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = Theme::global(cx).clone();
        let colors = crate::pane::query_colors(&theme);
        for p in self.panes.values() {
            p.update(cx, |p, _| {
                if let Some(s) = &p.session {
                    s.set_query_colors(colors.clone());
                }
            });
        }
        crate::window_bg::apply(window, &theme);
        window.refresh();
        cx.notify();
    }

    pub fn coop_changed(&mut self, cx: &mut Context<Self>) {
        // Refresh unread counts and claim lists shown in pane headers.
        let mut updates: Vec<(PaneId, usize, Vec<String>)> = Vec::new();
        for w in &self.ws.list {
            let Some(coop) = self.coop.get(&w.id) else { continue };
            for t in &w.tabs {
                for p in t.panes() {
                    let unread = coop.bus.unread_count(p.0);
                    let claims = coop
                        .claims
                        .held_by(p.0)
                        .iter()
                        .map(|c| c.path.to_string_lossy().into_owned())
                        .collect();
                    updates.push((p, unread, claims));
                }
            }
        }
        for (p, unread, claims) in updates {
            if let Some(e) = self.panes.get(&p) {
                e.update(cx, |e, cx| {
                    if e.unread != unread || e.claims != claims {
                        e.unread = unread;
                        e.claims = claims;
                        cx.notify();
                    }
                });
            }
        }
    }

    // ---- rename -----------------------------------------------------------------------------

    pub fn start_rename(&mut self, target: RenameTarget, window: &mut Window, cx: &mut Context<Self>) {
        let current = match &target {
            RenameTarget::Tab(id) => {
                let ws = self.ws.active();
                ws.tab(*id).map(|t| {
                    t.custom_name.clone().unwrap_or_else(|| {
                        self.panes
                            .get(&t.focused)
                            .map(|p| p.read(cx).title(Settings::global(cx).config.tabs.title))
                            .unwrap_or_default()
                    })
                })
            }
            RenameTarget::Pane(p) => self.panes.get(p).map(|p| p.read(cx).title(nox_config::TabTitle::Auto)),
            RenameTarget::Workspace(id) => self.ws.index_of(*id).map(|i| self.ws.list[i].name.clone()),
        }
        .unwrap_or_default();
        match target {
            RenameTarget::Tab(_) => {
                let input = cx.new(|cx| {
                    let mut i = crate::text_input::TextInput::new(cx, "Tab name");
                    i.set_text(current, cx);
                    i
                });
                let sub = cx.subscribe_in(
                    &input,
                    window,
                    |this: &mut Self, _, ev: &crate::text_input::InputEvent, window, cx| {
                        use crate::text_input::InputEvent;
                        match ev {
                            InputEvent::Submit | InputEvent::Blur => this.finish_rename(true, window, cx),
                            InputEvent::Cancel => this.finish_rename(false, window, cx),
                            InputEvent::Changed => {}
                        }
                    },
                );
                input.read(cx).focus(window);
                self.rename = Some(Rename { target, input, _sub: sub });
                cx.notify();
            }
            RenameTarget::Pane(p) => {
                let this = cx.entity().downgrade();
                crate::dialog::DialogBuilder::new("Rename Pane")
                    .message(
                        "An empty name restores the automatic title. Agents find each other by this name.",
                    )
                    .field("Name", current, "Automatic")
                    .cancel("Cancel")
                    .button("Rename", move |v, window, cx| {
                        let name = v.fields[0].trim().to_string();
                        let _ = this.update(cx, |this, cx| {
                            if let Some(e) = this.panes.get(&p) {
                                e.update(cx, |e, cx| {
                                    e.custom_name = (!name.is_empty()).then_some(name);
                                    cx.notify();
                                });
                            }
                            this.session_changed(cx);
                            this.focus_active(window, cx);
                            cx.notify();
                        });
                    })
                    .default_last()
                    .show(window, cx);
            }
            RenameTarget::Workspace(id) => {
                let this = cx.entity().downgrade();
                crate::dialog::DialogBuilder::new("Rename Workspace")
                    .field("Name", current, "Workspace name")
                    .cancel("Cancel")
                    .button("Rename", move |v, window, cx| {
                        let name = v.fields[0].trim().to_string();
                        let _ = this.update(cx, |this, cx| {
                            if let Some(i) = this.ws.index_of(id)
                                && !name.is_empty()
                            {
                                this.ws.list[i].name = name;
                            }
                            this.session_changed(cx);
                            this.focus_active(window, cx);
                            cx.notify();
                        });
                    })
                    .default_last()
                    .show(window, cx);
            }
        }
    }

    pub fn finish_rename(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.rename.take() else { return };
        if commit {
            let name = r.input.read(cx).text().trim().to_string();
            if let RenameTarget::Tab(id) = r.target
                && let Some(t) = self.ws.active_mut().tab_mut(id)
            {
                t.custom_name = (!name.is_empty()).then_some(name);
            }
            self.session_changed(cx);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    // ---- palette ----------------------------------------------------------------------------

    pub fn open_palette(&mut self, palette: Entity<Palette>, window: &mut Window, cx: &mut Context<Self>) {
        let sub =
            cx.subscribe_in(&palette, window, |this: &mut Self, _, ev: &PaletteEvent, window, cx| match ev {
                PaletteEvent::Dismissed => {
                    this.palette = None;
                    this.focus_active(window, cx);
                    cx.notify();
                }
            });
        self.palette = Some((palette, sub));
        cx.notify();
    }

    pub fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    // ---- agents -----------------------------------------------------------------------------

    /// Start an agent in a new tab (SPEC §7.12 "Launching").
    pub fn launch_agent(
        &mut self,
        ws_idx: usize,
        kind: &str,
        cwd: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(def) = Settings::global(cx).config.agents.defs.get(kind).cloned() else { return };
        let cwd =
            cwd.or_else(|| self.ws.list[ws_idx].root.clone()).unwrap_or_else(|| self.new_cwd(ws_idx, cx));
        let spawn = PaneSpawn {
            cwd: Some(cwd.clone()),
            startup: Some(Startup::Run(def.command.clone())),
            agent: Some(AgentLaunch {
                kind: kind.to_string(),
                command: def.command.clone(),
                start_cwd: cwd,
                resumed: false,
            }),
            ..Default::default()
        };
        self.add_tab(ws_idx, spawn, window, cx);
    }

    /// Agents waiting for the user, longest first: needs input, then idle-unseen (SPEC §7.12).
    pub fn waiting_agents(&self, cx: &App) -> Vec<PaneId> {
        let mut v: Vec<(u8, Instant, PaneId)> = Vec::new();
        for w in &self.ws.list {
            for t in &w.tabs {
                for p in t.panes() {
                    let Some(e) = self.panes.get(&p) else { continue };
                    let e = e.read(cx);
                    let Some(a) = &e.agent else { continue };
                    match a.tracker.state() {
                        AgentState::NeedsInput => v.push((0, a.tracker.since(), p)),
                        AgentState::Idle if e.done_unseen => v.push((1, a.tracker.since(), p)),
                        _ => {}
                    }
                }
            }
        }
        v.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        v.into_iter().map(|(_, _, p)| p).collect()
    }

    /// Every running agent of every workspace with its workspace index: needs input first, then
    /// idle-unseen, working, idle; workspace order within a rank (SPEC §5.4).
    pub fn live_agents(&self, cx: &App) -> Vec<(usize, PaneId)> {
        let mut v: Vec<(u8, usize, PaneId)> = Vec::new();
        for (wi, w) in self.ws.list.iter().enumerate() {
            for p in w.tabs.iter().flat_map(nox_layout::Tab::panes) {
                let Some(e) = self.panes.get(&p) else { continue };
                let e = e.read(cx);
                let Some(a) = &e.agent else { continue };
                let rank = match a.tracker.state() {
                    AgentState::Exited => continue,
                    AgentState::NeedsInput => 0,
                    AgentState::Idle if e.done_unseen => 1,
                    AgentState::Working => 2,
                    AgentState::Idle => 3,
                };
                v.push((rank, wi, p));
            }
        }
        v.sort_by_key(|(r, w, _)| (*r, *w));
        v.into_iter().map(|(_, w, p)| (w, p)).collect()
    }

    // ---- session ----------------------------------------------------------------------------

    /// A structural change: save within 1 s.
    pub fn session_changed(&mut self, cx: &mut Context<Self>) {
        if self.session_task.is_some() {
            return;
        }
        self.session_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let _ = this.update(cx, |this, cx| {
                this.session_task = None;
                this.save_session_now(cx);
            });
        }));
    }

    /// A soft change (cwd, titles, agent state): save within 30 s.
    pub fn session_soft_changed(&mut self, cx: &mut Context<Self>) {
        if self.session_soft_task.is_some() {
            return;
        }
        self.session_soft_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(30)).await;
            let _ = this.update(cx, |this, cx| {
                this.session_soft_task = None;
                this.save_session_now(cx);
            });
        }));
    }

    pub fn save_session_now(&mut self, cx: &mut Context<Self>) {
        if self.quitting {
            return;
        }
        crate::persist::save(self, cx);
    }
}

pub fn default_cwd(cx: &App) -> PathBuf {
    let wd = &Settings::global(cx).config.shell.working_directory;
    let p = crate::icons::expand_tilde(if wd.trim().is_empty() { "~" } else { wd });
    if p.as_os_str() == "~" {
        return dirs_home();
    }
    if p.is_dir() { p } else { dirs_home() }
}

pub fn dirs_home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// Directory with the `nox` CLI to put on every pane's PATH.
#[must_use]
pub fn cli_bin_dir() -> Option<PathBuf> {
    if let Some(res) = nox_mac::bundle_resources_dir() {
        let bin = res.join("bin");
        if bin.join("nox").exists() {
            return Some(bin);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    dir.join("nox").exists().then(|| dir.to_path_buf())
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::chrome::render_root(self, window, cx)
    }
}

#[must_use]
pub fn px_rect(r: Rect) -> gpui::Bounds<Pixels> {
    gpui::Bounds::new(gpui::point(px(r.x), px(r.y)), gpui::size(px(r.w), px(r.h)))
}

#[must_use]
pub fn dir_name(p: &Path) -> String {
    p.file_name().map_or_else(|| p.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned())
}

#[must_use]
pub fn direction_of(a: &str) -> Direction {
    match a {
        "left" => Direction::Left,
        "right" => Direction::Right,
        "up" => Direction::Up,
        _ => Direction::Down,
    }
}

#[allow(unused)]
fn _assert_any_element(_: AnyElement) {}
