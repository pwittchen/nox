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
                    let failed = exit.is_some_and(|c| c != 0);
                    p.update(cx, |p, _| {
                        p.done_unseen = true;
                        p.done_failed = failed;
                    });
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
                    p.update(cx, |p, _| {
                        p.done_unseen = true;
                        p.done_failed = false;
                    });
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::pane::AgentInfo;
    use gpui::{TestAppContext, VisualTestContext};
    use nox_agents::AgentTracker;

    /// The test config with every native side effect (notifications, sounds, dock bounce, sleep
    /// assertions) and quitting switched off. Panes run `/bin/cat` instead of a shell: it starts
    /// instantly, reads no profile, and dies on the `SIGHUP` a closed pane sends (a shell killed
    /// while still starting up can survive it, and dropping its session then blocks in
    /// `waitpid`). Tests that need a real shell use [`shell_config`].
    pub(crate) fn quiet_config() -> nox_config::Config {
        let mut c = shell_config();
        c.shell.program = "/bin/cat".into();
        c
    }

    /// [`quiet_config`] with a plain, non-login `/bin/sh`.
    pub(crate) fn shell_config() -> nox_config::Config {
        let mut c = crate::test_support::config();
        c.shell.args.clear();
        c.terminal.notifications = false;
        c.terminal.attention_sound = false;
        c.app.dock_bounce_on_bell = false;
        c.agents.keep_awake = false;
        c.app.quit_when_last_tab_closed = false;
        c
    }

    /// Install globals and keys for `cfg`, then open a fresh root window.
    pub(crate) fn open(
        cx: &mut TestAppContext,
        cfg: nox_config::Config,
    ) -> (Entity<Root>, &mut VisualTestContext) {
        let keys = cfg.keys.clone();
        cx.update(|cx| {
            crate::test_support::init_app(cx, cfg);
            crate::actions::bind_keys(cx, &keys);
        });
        let (root, cx) = cx.add_window_view(|window, cx| Root::new(Initial::Fresh, window, cx));
        cx.run_until_parked();
        (root, cx)
    }

    pub(crate) fn dialog_title(cx: &mut VisualTestContext) -> Option<String> {
        cx.update(|_, cx| {
            cx.try_global::<crate::dialog::ActiveDialog>()
                .and_then(|d| d.0.as_ref().map(|(v, _)| v.read(cx).title.to_string()))
        })
    }

    /// Replace the text of the dialog's field `i`.
    pub(crate) fn set_dialog_field(cx: &mut VisualTestContext, i: usize, text: &str) {
        cx.update(|_, cx| {
            let view = cx.global::<crate::dialog::ActiveDialog>().0.as_ref().unwrap().0.clone();
            let input = view.read(cx).fields[i].input.clone();
            input.update(cx, |inp, cx| inp.set_text(text.to_string(), cx));
        });
    }

    pub(crate) fn pane_count(root: &Entity<Root>, cx: &mut VisualTestContext) -> usize {
        root.read_with(cx, |r, _| r.panes.len())
    }

    thread_local! {
        /// Shell sessions taken out of panes by [`detach_session`]; dropped (killed) at test end.
        static DETACHED: std::cell::RefCell<Vec<nox_terminal::TerminalSession>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Take the shell out of `pane` (kept alive until the test ends), so its process detection
    /// can't race with a test that drives the pane's agent state.
    pub(crate) fn detach_session(root: &Entity<Root>, pane: PaneId, cx: &mut VisualTestContext) {
        let e = root.read_with(cx, |r, _| r.panes[&pane].clone());
        cx.update(|_, cx| {
            if let Some(s) = e.update(cx, |p, _| p.session.take()) {
                DETACHED.with(|d| d.borrow_mut().push(s));
            }
        });
    }

    /// `p` canonicalized (temp dirs are under a symlinked `/var`).
    pub(crate) fn canon(p: &Path) -> PathBuf {
        p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
    }

    pub(crate) fn pane_cwd(root: &Entity<Root>, pane: PaneId, cx: &mut VisualTestContext) -> Option<PathBuf> {
        root.read_with(cx, |r, cx| r.panes[&pane].read(cx).cwd.as_deref().map(canon))
    }

    /// Give `pane` an agent of `kind` in `state` (its shell detached, see [`detach_session`]).
    pub(crate) fn make_agent(
        root: &Entity<Root>,
        pane: PaneId,
        kind: &str,
        state: AgentState,
        cx: &mut VisualTestContext,
    ) {
        detach_session(root, pane, cx);
        let e = root.read_with(cx, |r, _| r.panes[&pane].clone());
        cx.update(|_, cx| {
            e.update(cx, |p, _| {
                let now = Instant::now();
                let mut tracker = AgentTracker::new(kind, now);
                tracker.on_report(state, Some(format!("{kind} msg")), Some("sess-1".into()), now);
                p.agent = Some(AgentInfo { kind: kind.into(), tracker, launch: None, cmdline: kind.into() });
            });
        });
    }

    fn emit(root: &Entity<Root>, pane: PaneId, ev: PaneEvent, cx: &mut VisualTestContext) {
        let e = root.read_with(cx, |r, _| r.panes[&pane].clone());
        cx.update(|_, cx| e.update(cx, |_, cx| cx.emit(ev)));
        cx.run_until_parked();
    }

    fn focused(root: &Entity<Root>, cx: &mut VisualTestContext) -> PaneId {
        root.read_with(cx, |r, _| r.focused_pane_id().unwrap())
    }

    #[gpui::test]
    fn fresh_root_has_one_tab_in_the_shell_directory(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        let p = focused(&root, cx);
        assert_eq!(pane_cwd(&root, p, cx), Some(canon(&crate::test_support::nox_home())));
        root.read_with(cx, |r, _| {
            assert_eq!(r.ws.len(), 1);
            assert_eq!(r.ws.active().tabs.len(), 1);
            assert!(r.coop.contains_key(&r.ws.active().id));
            assert!(r.pending.is_empty());
            assert!(!r.quitting);
        });
    }

    #[gpui::test]
    fn new_tabs_open_after_the_active_one(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        let first = focused(&root, cx);
        cx.dispatch_action(crate::actions::NewTab);
        cx.dispatch_action(crate::actions::NewTab);
        let third = focused(&root, cx);
        root.update_in(cx, |r, w, cx| {
            assert_eq!(r.ws.active().tabs.len(), 3);
            assert_eq!(r.ws.active().active_tab, 2);
            r.activate_tab(0, w, cx);
            assert_eq!(r.focused_pane_id(), Some(first));
            r.new_tab(w, cx);
            // Inserted right after tab 0.
            assert_eq!(r.ws.active().active_tab, 1);
            assert_eq!(r.ws.active().tabs[3].focused, third);
            // Activating a missing tab does nothing.
            r.activate_tab(9, w, cx);
            assert_eq!(r.ws.active().active_tab, 1);
        });
        assert_eq!(pane_count(&root, cx), 4);
    }

    #[gpui::test]
    fn split_adds_a_focused_pane_and_closing_it_restores_one(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        let first = focused(&root, cx);
        cx.dispatch_action(crate::actions::SplitVertical);
        let second = focused(&root, cx);
        assert_ne!(first, second);
        root.update_in(cx, |r, w, cx| {
            assert_eq!(r.active_tab().unwrap().panes().len(), 2);
            let third = r.split(SplitDirection::Horizontal, None, w, cx).expect("room to split");
            assert_eq!(r.focused_pane_id(), Some(third));
            assert_eq!(r.pane_workspace(third), Some(r.ws.active().id));
            r.close_pane(third, w, cx);
            r.close_pane(second, w, cx);
            assert_eq!(r.active_tab().unwrap().panes(), vec![first]);
            assert_eq!(r.focused_pane_id(), Some(first));
            // Unknown panes are ignored.
            r.close_pane(PaneId(u64::MAX), w, cx);
        });
        assert_eq!(pane_count(&root, cx), 1);
    }

    #[gpui::test]
    fn split_is_refused_when_the_pane_is_too_small(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        cx.simulate_resize(gpui::size(px(220.), px(160.)));
        cx.run_until_parked();
        let p = focused(&root, cx);
        let made = root.update_in(cx, |r, w, cx| r.split(SplitDirection::Vertical, None, w, cx));
        assert!(made.is_none());
        root.read_with(cx, |r, cx| {
            assert_eq!(r.active_tab().unwrap().panes(), vec![p]);
            assert!(r.panes[&p].read(cx).shake.is_some(), "refusal shakes the pane");
        });
    }

    #[gpui::test]
    fn closing_the_last_tab_starts_a_fresh_workspace(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        let old_pane = focused(&root, cx);
        let old_ws = root.read_with(cx, |r, _| r.ws.active().id);
        cx.dispatch_action(crate::actions::CloseTab);
        root.read_with(cx, |r, _| {
            assert!(!r.quitting);
            assert_eq!(r.ws.len(), 1);
            assert_ne!(r.ws.active().id, old_ws);
            assert_eq!(r.ws.active().tabs.len(), 1);
            assert_ne!(r.focused_pane_id(), Some(old_pane));
            assert!(!r.panes.contains_key(&old_pane));
            assert!(!r.coop.contains_key(&old_ws));
        });
    }

    #[gpui::test]
    fn closing_the_last_tab_keeps_the_workspace_unless_close_when_empty(cx: &mut TestAppContext) {
        let mut cfg = quiet_config();
        cfg.workspaces.close_when_empty = false;
        let (root, cx) = open(cx, cfg);
        let ws = root.read_with(cx, |r, _| r.ws.active().id);
        let old = focused(&root, cx);
        cx.dispatch_action(crate::actions::ClosePane);
        root.read_with(cx, |r, _| {
            assert_eq!(r.ws.active().id, ws);
            assert_eq!(r.ws.active().tabs.len(), 1);
            assert_ne!(r.focused_pane_id(), Some(old));
        });
    }

    #[gpui::test]
    fn close_other_tabs_and_duplicate(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        cx.dispatch_action(crate::actions::NewTab);
        cx.dispatch_action(crate::actions::DuplicateTab);
        let keep = focused(&root, cx);
        assert_eq!(root.read_with(cx, |r, _| r.ws.active().tabs.len()), 3);
        assert_eq!(pane_cwd(&root, keep, cx), Some(canon(&crate::test_support::nox_home())));
        cx.dispatch_action(crate::actions::CloseOtherTabs);
        root.read_with(cx, |r, _| {
            assert_eq!(r.ws.active().tabs.len(), 1);
            assert_eq!(r.focused_pane_id(), Some(keep));
            assert_eq!(r.panes.len(), 1);
        });
    }

    #[gpui::test]
    fn workspaces_create_switch_reveal_and_close(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        let p0 = focused(&root, cx);
        let p1 = root.update_in(cx, |r, w, cx| {
            let idx = r.create_workspace(Some("  work ".into()), None, w, cx);
            assert_eq!(idx, 1);
            assert_eq!(r.ws.active, 1);
            assert_eq!(r.ws.active().name, "work");
            // Unique names.
            let i2 = r.create_workspace(Some("work".into()), None, w, cx);
            assert_ne!(r.ws.list[i2].name, "work");
            r.close_workspace_now(i2, w, cx);
            r.add_tab(1, PaneSpawn::default(), w, cx)
        });
        root.update_in(cx, |r, w, cx| {
            assert_eq!(r.ws.len(), 2);
            r.switch_workspace(0, w, cx);
            assert_eq!(r.focused_pane_id(), Some(p0));
            r.switch_workspace(7, w, cx);
            assert_eq!(r.ws.active, 0);
            r.reveal_pane(p1, w, cx);
            assert_eq!(r.ws.active, 1);
            assert_eq!(r.focused_pane_id(), Some(p1));
            assert!(r.is_looked_at(p1));
            assert!(!r.is_looked_at(p0));
            r.window_active = false;
            assert!(!r.is_looked_at(p1));
            r.window_active = true;
        });
        cx.dispatch_action(crate::actions::PreviousWorkspace);
        root.read_with(cx, |r, _| assert_eq!(r.ws.active, 0));
        cx.dispatch_action(crate::actions::NextWorkspace);
        root.read_with(cx, |r, _| assert_eq!(r.ws.active, 1));
        cx.dispatch_action(crate::actions::LastWorkspace);
        root.read_with(cx, |r, _| assert_eq!(r.ws.active, 0));
        cx.dispatch_action(crate::actions::ActivateWorkspace2);
        root.read_with(cx, |r, _| assert_eq!(r.ws.active, 1));
        cx.dispatch_action(crate::actions::CloseWorkspace);
        root.read_with(cx, |r, _| {
            assert_eq!(r.ws.len(), 1);
            assert_eq!(r.focused_pane_id(), Some(p0));
            assert!(!r.panes.contains_key(&p1));
            assert_eq!(r.coop.len(), 1);
        });
    }

    #[gpui::test]
    fn tab_rename_commits_trimmed_and_empty_restores_auto(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        cx.dispatch_action(crate::actions::RenameTab);
        assert!(root.read_with(cx, |r, _| r.rename.is_some()));
        cx.simulate_input("  build  ");
        cx.simulate_keystrokes("enter");
        root.read_with(cx, |r, cx| {
            assert!(r.rename.is_none());
            let t = r.active_tab().unwrap();
            assert_eq!(t.custom_name.as_deref(), Some("build"));
            // show_index is on by default.
            assert_eq!(r.tab_title(t, 0, cx), "1 build");
            assert_eq!(r.pane_tab_title(t.focused, cx), "1 build");
            assert_eq!(r.pane_tab_title(PaneId(u64::MAX), cx), "");
        });
        // Cancel keeps the name.
        let id = root.read_with(cx, |r, _| r.active_tab().unwrap().id);
        root.update_in(cx, |r, w, cx| {
            r.start_rename(RenameTarget::Tab(id), w, cx);
            r.rename.as_ref().unwrap().input.update(cx, |i, cx| i.set_text("other".to_string(), cx));
            r.finish_rename(false, w, cx);
            assert_eq!(r.active_tab().unwrap().custom_name.as_deref(), Some("build"));
            r.start_rename(RenameTarget::Tab(id), w, cx);
            assert_eq!(r.rename.as_ref().unwrap().input.read(cx).text(), "build");
            r.rename.as_ref().unwrap().input.update(cx, |i, cx| i.set_text("   ".to_string(), cx));
            r.finish_rename(true, w, cx);
            assert_eq!(r.active_tab().unwrap().custom_name, None);
            // Nothing to finish.
            r.finish_rename(true, w, cx);
        });
    }

    #[gpui::test]
    fn pane_and_workspace_rename_sheets(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        let p = focused(&root, cx);
        cx.dispatch_action(crate::actions::RenamePane);
        assert_eq!(dialog_title(cx).as_deref(), Some("Rename Pane"));
        set_dialog_field(cx, 0, " reviewer ");
        cx.simulate_keystrokes("enter");
        assert_eq!(dialog_title(cx), None);
        root.read_with(cx, |r, cx| {
            assert_eq!(r.panes[&p].read(cx).custom_name.as_deref(), Some("reviewer"));
            assert_eq!(r.pane_name(p, cx), "reviewer");
        });

        cx.dispatch_action(crate::actions::RenameWorkspace);
        assert_eq!(dialog_title(cx).as_deref(), Some("Rename Workspace"));
        set_dialog_field(cx, 0, "api");
        cx.simulate_keystrokes("enter");
        root.read_with(cx, |r, _| assert_eq!(r.ws.active().name, "api"));
        // An empty workspace name is ignored.
        cx.dispatch_action(crate::actions::RenameWorkspace);
        set_dialog_field(cx, 0, "  ");
        cx.simulate_keystrokes("enter");
        root.read_with(cx, |r, _| assert_eq!(r.ws.active().name, "api"));
        // Escape cancels.
        cx.dispatch_action(crate::actions::RenamePane);
        set_dialog_field(cx, 0, "zzz");
        cx.simulate_keystrokes("escape");
        assert_eq!(dialog_title(cx), None);
        root.read_with(cx, |r, cx| assert_eq!(r.panes[&p].read(cx).custom_name.as_deref(), Some("reviewer")));
    }

    #[gpui::test]
    fn agent_names_get_suffixes_per_workspace(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        let a = focused(&root, cx);
        let b = root.update_in(cx, |r, w, cx| r.split(SplitDirection::Vertical, None, w, cx).unwrap());
        let c = root.update_in(cx, |r, w, cx| r.split(SplitDirection::Horizontal, None, w, cx).unwrap());
        root.read_with(cx, |r, cx| {
            assert_eq!(r.pane_name(a, cx), format!("pane-{a}"));
            assert_eq!(r.pane_name(PaneId(u64::MAX), cx), PaneId(u64::MAX).to_string());
        });
        make_agent(&root, a, "claude", AgentState::Working, cx);
        make_agent(&root, b, "claude", AgentState::Idle, cx);
        make_agent(&root, c, "codex", AgentState::NeedsInput, cx);
        root.read_with(cx, |r, cx| {
            assert_eq!(r.pane_name(a, cx), "claude");
            assert_eq!(r.pane_name(b, cx), "claude-2");
            assert_eq!(r.pane_name(c, cx), "codex");
            // Needs input first, then working, then idle (seen).
            assert_eq!(r.live_agents(cx), vec![(0, c), (0, a), (0, b)]);
            assert_eq!(r.waiting_agents(cx), vec![c]);
            let tab = r.active_tab().unwrap();
            assert_eq!(r.tab_status(tab, cx), TabStatus::NeedsInput);
            assert_eq!(r.workspace_status(r.ws.active(), cx), TabStatus::NeedsInput);
        });
        root.update_in(cx, |r, _, cx| {
            r.panes[&b].update(cx, |p, _| p.done_unseen = true);
            assert_eq!(r.waiting_agents(cx), vec![c, b]);
            assert_eq!(r.live_agents(cx), vec![(0, c), (0, b), (0, a)]);
        });
    }

    #[gpui::test]
    fn bell_and_finished_commands_mark_unseen_panes(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        let a = focused(&root, cx);
        // Looked at: nothing sticks.
        emit(&root, a, PaneEvent::Bell, cx);
        emit(
            &root,
            a,
            PaneEvent::CommandFinished { command: "make".into(), exit: Some(2), duration: Duration::ZERO },
            cx,
        );
        root.read_with(cx, |r, cx| {
            let p = r.panes[&a].read(cx);
            assert!(!p.bell_unseen && !p.done_unseen);
        });
        root.update(cx, |r, _| r.window_active = false);
        emit(&root, a, PaneEvent::Bell, cx);
        emit(
            &root,
            a,
            PaneEvent::CommandFinished {
                command: "make".into(),
                exit: Some(2),
                duration: Duration::from_secs(60),
            },
            cx,
        );
        emit(&root, a, PaneEvent::Notify { title: Some("t".into()), body: "b".into() }, cx);
        root.read_with(cx, |r, cx| {
            let p = r.panes[&a].read(cx);
            assert!(p.bell_unseen);
            assert!(p.done_unseen && p.done_failed);
            assert_eq!(r.tab_status(r.active_tab().unwrap(), cx), TabStatus::Bell);
        });
        // Coming back marks it seen.
        root.update(cx, |r, cx| {
            r.window_active = true;
            r.mark_visible_seen(cx);
        });
        root.read_with(cx, |r, cx| {
            let p = r.panes[&a].read(cx);
            assert!(!p.bell_unseen && !p.done_unseen && !p.done_failed);
        });
        emit(&root, a, PaneEvent::UserInput, cx);
        emit(&root, a, PaneEvent::TitleChanged, cx);
        assert!(root.read_with(cx, |r, _| r.session_soft_task.is_some()));
    }

    #[gpui::test]
    fn exit_closes_the_pane_only_when_clean(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        let a = focused(&root, cx);
        let b = root.update_in(cx, |r, w, cx| r.split(SplitDirection::Vertical, None, w, cx).unwrap());
        emit(&root, b, PaneEvent::Exited { code: Some(1) }, cx);
        assert!(root.read_with(cx, |r, _| r.panes.contains_key(&b)));
        emit(&root, b, PaneEvent::Exited { code: Some(0) }, cx);
        root.read_with(cx, |r, _| {
            assert!(!r.panes.contains_key(&b));
            assert_eq!(r.active_tab().unwrap().panes(), vec![a]);
        });
    }

    #[gpui::test]
    fn pane_events_split_focus_and_maximize(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        let a = focused(&root, cx);
        emit(&root, a, PaneEvent::Split(SplitDirection::Vertical), cx);
        let b = focused(&root, cx);
        assert_ne!(a, b);
        emit(&root, a, PaneEvent::Focused, cx);
        assert_eq!(focused(&root, cx), a);
        emit(&root, b, PaneEvent::ToggleMaximize, cx);
        root.read_with(cx, |r, cx| {
            assert_eq!(r.focused_pane_id(), Some(b));
            assert_eq!(r.active_tab().unwrap().maximized, Some(b));
            assert!(cx.global::<UiFlags>().maximized_panes.contains(&b));
        });
        // Next pane un-maximizes and moves focus.
        cx.dispatch_action(crate::actions::NextPane);
        root.read_with(cx, |r, cx| {
            assert_eq!(r.active_tab().unwrap().maximized, None);
            assert!(cx.global::<UiFlags>().maximized_panes.is_empty());
            assert_eq!(r.focused_pane_id(), Some(a));
        });
        cx.dispatch_action(crate::actions::ToggleMaximizePane);
        root.read_with(cx, |r, _| assert_eq!(r.active_tab().unwrap().maximized, Some(a)));
        cx.dispatch_action(crate::actions::PreviousPane);
        root.read_with(cx, |r, _| {
            assert_eq!(r.active_tab().unwrap().maximized, None);
            assert_eq!(r.focused_pane_id(), Some(b));
        });
        // Pane asks to close (nothing running: no sheet).
        emit(&root, b, PaneEvent::Close, cx);
        assert_eq!(dialog_title(cx), None);
        root.read_with(cx, |r, _| assert_eq!(r.active_tab().unwrap().panes(), vec![a]));
    }

    #[gpui::test]
    fn agent_turn_end_marks_done_when_not_looked_at(cx: &mut TestAppContext) {
        let mut cfg = quiet_config();
        cfg.terminal.attention_min_duration = 0;
        let (root, cx) = open(cx, cfg);
        let a = focused(&root, cx);
        detach_session(&root, a, cx);
        let e = root.read_with(cx, |r, _| r.panes[&a].clone());
        cx.update(|_, cx| e.update(cx, |p, cx| p.report_status(AgentState::Working, None, None, cx)));
        cx.run_until_parked();
        root.update(cx, |r, _| r.window_active = false);
        cx.update(|_, cx| {
            e.update(cx, |p, cx| p.report_status(AgentState::Idle, Some("done".into()), None, cx));
        });
        cx.run_until_parked();
        root.read_with(cx, |r, cx| {
            assert!(r.panes[&a].read(cx).done_unseen);
            assert_eq!(r.waiting_agents(cx), vec![a]);
        });
    }

    #[gpui::test]
    fn launch_agent_opens_an_agent_tab(cx: &mut TestAppContext) {
        let mut cfg = quiet_config();
        cfg.agents.defs.clear();
        cfg.agents
            .defs
            .insert("fake".into(), nox_config::AgentDef { command: "true".into(), ..Default::default() });
        let (root, cx) = open(cx, cfg);
        root.update_in(cx, |r, w, cx| {
            r.launch_agent(0, "missing", None, w, cx);
            assert_eq!(r.ws.active().tabs.len(), 1);
            r.launch_agent(0, "fake", None, w, cx);
            assert_eq!(r.ws.active().tabs.len(), 2);
            let p = r.focused_pane().unwrap();
            let p = p.read(cx);
            let a = p.agent.as_ref().expect("agent pane");
            assert_eq!(a.kind, "fake");
            assert_eq!(a.launch.as_ref().unwrap().command, "true");
        });
    }

    #[gpui::test]
    fn geometry_follows_the_tabs_position(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        let set = |cx: &mut VisualTestContext, f: &dyn Fn(&mut nox_config::Config)| {
            cx.update(|_, cx| {
                let mut c = (*Settings::global(cx).config).clone();
                f(&mut c);
                cx.set_global(Settings::new(c, nox_mac::Accessibility::default()));
            });
        };
        let top = root.update_in(cx, |r, w, cx| {
            assert!(r.tab_bar_shown(cx));
            assert!(!r.sidebar_shown(cx));
            assert!((r.titlebar_height(cx) - TITLEBAR_TOP_PX).abs() < f32::EPSILON);
            let min = r.min_pane(w, cx);
            assert!(min.w > 0. && min.h > 0.);
            r.layout_area(w, cx)
        });
        set(cx, &|c| c.tabs.position = nox_config::TabsPosition::Bottom);
        let bottom = root.update_in(cx, |r, w, cx| {
            assert!((r.titlebar_height(cx) - TITLEBAR_SLIM_PX).abs() < f32::EPSILON);
            r.layout_area(w, cx)
        });
        assert!((top.h - (bottom.h + BOTTOM_BAR_PX - (TITLEBAR_TOP_PX - TITLEBAR_SLIM_PX))).abs() < 0.01);
        set(cx, &|c| {
            c.tabs.position = nox_config::TabsPosition::Left;
            c.tabs.sidebar_visible = true;
            c.tabs.sidebar_width = 9999;
        });
        root.update_in(cx, |r, w, cx| {
            assert!(r.sidebar_shown(cx));
            assert!(!r.tab_bar_shown(cx));
            assert!((r.sidebar_width(cx) - 360.).abs() < f32::EPSILON);
            let left = r.layout_area(w, cx);
            assert!((left.x - top.x - 361.).abs() < 0.01);
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn new_tab_cwd_prefers_root_or_the_focused_pane(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = quiet_config();
        cfg.workspaces.new_tab_cwd = nox_config::NewTabCwd::Root;
        let (root, cx) = open(cx, cfg);
        let home = crate::test_support::nox_home();
        root.update_in(cx, |r, w, cx| {
            assert_eq!(canon(&r.new_cwd(0, cx)), canon(&home));
            let idx = r.create_workspace(None, Some(dir.path().to_path_buf()), w, cx);
            assert_eq!(r.ws.list[idx].name, dir_name(dir.path()));
            // Empty workspace: its root.
            assert_eq!(r.new_cwd(idx, cx), dir.path());
            r.add_tab(idx, PaneSpawn { cwd: Some(home.clone()), ..Default::default() }, w, cx);
            // Root mode: the root even though the pane is elsewhere.
            assert_eq!(r.new_cwd(idx, cx), dir.path());
        });
        cx.update(|_, cx| {
            let mut c = (*Settings::global(cx).config).clone();
            c.workspaces.new_tab_cwd = nox_config::NewTabCwd::Inherit;
            cx.set_global(Settings::new(c, nox_mac::Accessibility::default()));
        });
        root.read_with(cx, |r, cx| assert_eq!(canon(&r.new_cwd(1, cx)), canon(&home)));
    }

    #[gpui::test]
    fn font_size_steps_apply_to_every_pane(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        cx.dispatch_action(crate::actions::SplitVertical);
        let base = cx.update(|_, cx| Settings::global(cx).term_font.size);
        cx.dispatch_action(crate::actions::FontBigger);
        cx.dispatch_action(crate::actions::FontBigger);
        root.read_with(cx, |r, cx| {
            assert_eq!(r.font_override, Some(base + 2.));
            assert!(r.panes.values().all(|p| p.read(cx).font_size_override == Some(base + 2.)));
        });
        cx.dispatch_action(crate::actions::FontSmaller);
        root.read_with(cx, |r, _| assert_eq!(r.font_override, Some(base + 1.)));
        // New panes inherit the override.
        cx.dispatch_action(crate::actions::NewTab);
        root.read_with(cx, |r, cx| {
            assert_eq!(r.focused_pane().unwrap().read(cx).font_size_override, Some(base + 1.));
        });
        cx.dispatch_action(crate::actions::FontReset);
        root.read_with(cx, |r, cx| {
            assert_eq!(r.font_override, None);
            assert!(r.panes.values().all(|p| p.read(cx).font_size_override.is_none()));
        });
    }

    #[gpui::test]
    fn theme_change_and_session_debounce(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        root.update_in(cx, |r, w, cx| {
            r.on_theme_changed(w, cx);
            r.session_task = None;
            r.session_changed(cx);
            assert!(r.session_task.is_some());
            // A second change while pending doesn't restart it.
            r.session_changed(cx);
            r.quitting = true;
        });
        // The debounced save fires after 1 s (quitting: it writes nothing).
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        assert!(root.read_with(cx, |r, _| r.session_task.is_none()));
    }

    #[test]
    fn helpers() {
        assert_eq!(dir_name(Path::new("/a/b/proj")), "proj");
        assert_eq!(dir_name(Path::new("/")), "/");
        assert_eq!(direction_of("left"), Direction::Left);
        assert_eq!(direction_of("right"), Direction::Right);
        assert_eq!(direction_of("up"), Direction::Up);
        assert_eq!(direction_of("down"), Direction::Down);
        assert_eq!(direction_of("??"), Direction::Down);
        let b = px_rect(Rect::new(1., 2., 3., 4.));
        assert_eq!(b.origin, gpui::point(px(1.), px(2.)));
        assert_eq!(b.size, gpui::size(px(3.), px(4.)));
        assert!(dirs_home().is_absolute());
    }

    #[gpui::test]
    fn default_cwd_falls_back_to_home(cx: &mut TestAppContext) {
        let mut cfg = quiet_config();
        cfg.shell.working_directory = "/definitely/not/here".into();
        cx.update(|cx| {
            crate::test_support::init_app(cx, cfg.clone());
            assert_eq!(default_cwd(cx), dirs_home());
            let mut c = cfg.clone();
            c.shell.working_directory = "  ".into();
            cx.set_global(Settings::new(c, nox_mac::Accessibility::default()));
            assert_eq!(default_cwd(cx), dirs_home());
        });
    }
}
