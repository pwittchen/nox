//! Agent cooperation over the control socket (SPEC §7.13).
//!
//! The `nox_ipc` server thread hands every request to the main thread through a channel; the
//! handler here applies it to the owning workspace / pane. `wait`, `inbox --wait` and
//! `claim --wait` are held as subscriptions (re-checked on state changes), never polled.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use gpui::{App, Context, Window};
use nox_agents::{ActivityKind, AgentState, PaneRef, Recipient};
use nox_ipc::{
    AgentStateWire, Caller, ClaimConflictWire, ClaimConflictsData, ClaimResult, ClaimWire, ClaimsResult,
    ErrorCode, Handler, InboxResult, IpcError, LsResult, MessageWire, Method, MsgResult, OpenResult,
    PaneInfo, Placement, ReadMode, ReadResult, ReleaseResult, Responder, ServerHandle, SpawnResult, TabInfo,
    WaitResult, WaitState, WsInfo,
};
use nox_layout::{PaneId, SplitDirection, WorkspaceId};

use crate::pane::{PaneSpawn, Startup, shell_escape};
use crate::root::Root;
use crate::settings_state::Settings;

type Request = (Option<Caller>, Method, Responder);

struct Bridge {
    tx: async_channel::Sender<Request>,
}

impl Handler for Bridge {
    fn handle(&self, caller: Option<Caller>, method: Method, reply: Responder) {
        if let Err(e) = self.tx.try_send((caller, method, reply)) {
            let (_, _, reply) = e.into_inner();
            reply.err(ErrorCode::Internal, "nox is shutting down");
        }
    }
}

pub struct IpcState {
    pub handle: ServerHandle,
    pub waiters: Vec<Waiter>,
}

pub enum WaitKind {
    State { pane: PaneId, state: WaitState },
    Inbox { pane: PaneId, ws: WorkspaceId, from: Option<u64>, peek: bool },
    Claim { pane: PaneId, ws: WorkspaceId, paths: Vec<PathBuf>, reason: Option<String> },
    Send { caller: PaneId, target: PaneId, text: String, submit: bool },
}

pub struct Waiter {
    pub kind: WaitKind,
    pub reply: Option<Responder>,
    pub deadline: Option<Instant>,
}

fn err(code: ErrorCode, msg: impl Into<String>) -> IpcError {
    IpcError::new(code, msg)
}

impl Root {
    pub fn start_ipc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !Settings::global(cx).config.agents.cooperation {
            return;
        }
        let (tx, rx) = async_channel::unbounded::<Request>();
        let path = nox_ipc::default_socket_path();
        match nox_ipc::Server::start(&path, Arc::new(Bridge { tx })) {
            Ok(handle) => {
                self.ipc = Some(IpcState { handle, waiters: vec![] });
                let task = cx.spawn_in(window, async move |this, cx| {
                    while let Ok((caller, method, reply)) = rx.recv().await {
                        let ok = this.update_in(cx, |this, window, cx| {
                            this.handle_ipc(caller, method, reply, window, cx);
                        });
                        if ok.is_err() {
                            break;
                        }
                    }
                });
                self.tasks.push(task);
                // Expire waiter deadlines and claims once a second while anything waits.
                let tick = cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(Duration::from_secs(1)).await;
                        if this.update(cx, super::root::Root::ipc_tick).is_err() {
                            break;
                        }
                    }
                });
                self.tasks.push(tick);
            }
            Err(e) => {
                log::error!("control socket {}: {e}", path.display());
                crate::toast::show(cx, format!("Agent cooperation is off: the control socket failed ({e})."));
            }
        }
    }

    fn ipc_tick(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let ttl = Duration::from_secs(Settings::global(cx).config.agents.claim_ttl);
        let mut expired_any = false;
        for coop in self.coop.values_mut() {
            if !coop.claims.expire(now, ttl).is_empty() {
                expired_any = true;
            }
        }
        if expired_any {
            self.coop_changed(cx);
        }
        let Some(ipc) = &mut self.ipc else { return };
        for w in &mut ipc.waiters {
            if w.deadline.is_some_and(|d| now >= d)
                && let Some(r) = w.reply.take()
            {
                let msg = match w.kind {
                    WaitKind::Send { .. } => "the user kept typing in the target pane",
                    _ => "timed out",
                };
                r.err(ErrorCode::Timeout, msg);
            }
        }
        ipc.waiters.retain(|w| w.reply.is_some());
        if ipc.waiters.iter().any(|w| matches!(w.kind, WaitKind::Send { .. })) {
            self.resolve_waiters(cx);
        }
    }

    /// Resolve the pane a request names (id or name) within the caller's scope.
    fn resolve_target(&self, caller: Option<&Caller>, name: &str, cx: &App) -> Result<PaneId, IpcError> {
        let scope_all = Settings::global(cx).config.agents.scope == nox_config::AgentScope::All;
        let caller_ws = caller.map(|c| WorkspaceId(c.workspace_id));
        let in_scope =
            |p: PaneId| -> bool { scope_all || caller_ws.is_none() || self.pane_workspace(p) == caller_ws };
        if let Ok(id) = name.parse::<u64>() {
            let p = PaneId(id);
            if self.panes.contains_key(&p) || self.pending.contains_key(&p) {
                return if in_scope(p) {
                    Ok(p)
                } else {
                    Err(err(ErrorCode::Forbidden, "pane is in another workspace"))
                };
            }
        }
        // By name: the caller's workspace first, then others when the scope allows.
        let mut candidates: Vec<PaneId> = Vec::new();
        for w in &self.ws.list {
            for t in &w.tabs {
                for p in t.panes() {
                    if self.pane_name(p, cx) == name {
                        candidates.push(p);
                    }
                }
            }
        }
        candidates.sort_by_key(|p| (self.pane_workspace(*p) != caller_ws, p.0));
        match candidates.into_iter().find(|p| in_scope(*p)) {
            Some(p) => Ok(p),
            None => Err(err(ErrorCode::NotFound, format!("no pane named “{name}”"))),
        }
    }

    fn handle_ipc(
        &mut self,
        caller: Option<Caller>,
        method: Method,
        reply: Responder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cpane = caller.map(|c| PaneId(c.pane_id));
        // Refresh the caller's workspace (the tab may have moved since its token was issued).
        let cws = cpane.and_then(|p| self.pane_workspace(p)).or(caller.map(|c| WorkspaceId(c.workspace_id)));
        let cfg = Settings::global(cx).config.clone();
        let now = Instant::now();

        // Loop guard for messages and sends.
        if matches!(method, Method::Msg { .. } | Method::Send { .. })
            && let Some(p) = cpane
            && !self.rate.check(p.0, now)
        {
            let name = self.pane_name(p, cx);
            crate::toast::show(cx, format!("“{name}” is sending too many messages; requests are refused."));
            reply.err(ErrorCode::RateLimited, "more than 20 messages / sends in 10 s");
            return;
        }

        match method {
            Method::Ls { all } => {
                let scope_all = all || cfg.agents.scope == nox_config::AgentScope::All || caller.is_none();
                reply.ok_typed(&self.ls(cws, scope_all, cx));
            }
            Method::Name { name } => {
                let Some(p) = cpane else { return reply.err(ErrorCode::Unauthorized, "no pane") };
                let name = name.trim().to_string();
                if name.is_empty() || name.parse::<u64>().is_ok() {
                    return reply.err(ErrorCode::Usage, "names must be non-empty and not a number");
                }
                if let Ok(other) = self.resolve_target(caller.as_ref(), &name, cx)
                    && other != p
                    && self.pane_workspace(other) == cws
                {
                    return reply
                        .err(ErrorCode::Conflict, format!("“{name}” is already taken in this workspace"));
                }
                if let Some(e) = self.panes.get(&p) {
                    e.update(cx, |e, cx| {
                        e.custom_name = Some(name.clone());
                        cx.notify();
                    });
                }
                self.session_changed(cx);
                reply.ok(serde_json::json!({ "name": name }));
            }
            Method::Status { state, message, session } => {
                let Some(p) = cpane else { return reply.err(ErrorCode::Unauthorized, "no pane") };
                let st = match state {
                    AgentStateWire::Working => AgentState::Working,
                    AgentStateWire::NeedsInput => AgentState::NeedsInput,
                    AgentStateWire::Idle => AgentState::Idle,
                };
                if let Some(e) = self.panes.get(&p).cloned() {
                    e.update(cx, |e, cx| e.report_status(st, message.clone(), session, cx));
                }
                let name = self.pane_name(p, cx);
                if let Some(ws) = cws
                    && let Some(coop) = self.coop.get_mut(&ws)
                {
                    coop.activity.record(
                        ActivityKind::Status,
                        name,
                        format!("{}{}", st, message.map(|m| format!(" — {m}")).unwrap_or_default()),
                        SystemTime::now(),
                    );
                }
                reply.ok(serde_json::json!({ "state": st.to_string() }));
            }
            Method::Read { pane, mode } => {
                let target = match self.resolve_target(caller.as_ref(), &pane, cx) {
                    Ok(t) => t,
                    Err(e) => return reply.error(e),
                };
                let Some(e) = self.panes.get(&target) else {
                    return reply.err(ErrorCode::NotFound, "pane not started");
                };
                let text = e.read(cx).session.as_ref().map(|s| match mode {
                    ReadMode::Screen => Some(s.screen_text()),
                    ReadMode::Lines(n) => Some(s.last_lines(n as usize)),
                    ReadMode::Scrollback => Some(s.scrollback_text()),
                    ReadMode::LastOutput => s.last_output(),
                });
                match text {
                    Some(Some(text)) => reply.ok_typed(&ReadResult { text }),
                    Some(None) => {
                        reply
                            .err(ErrorCode::NotFound, "no finished command output (needs shell integration)");
                    }
                    None => reply.err(ErrorCode::NotFound, "pane has no session"),
                }
            }
            Method::Send { pane, text, submit } => {
                let Some(p) = cpane else { return reply.err(ErrorCode::Unauthorized, "no pane") };
                let target = match self.resolve_target(caller.as_ref(), &pane, cx) {
                    Ok(t) => t,
                    Err(e) => return reply.error(e),
                };
                let is_agent = self.panes.get(&target).is_some_and(|e| e.read(cx).is_agent());
                if !is_agent && !cfg.agents.allow_send_to_shell {
                    return reply.err(
                        ErrorCode::Forbidden,
                        "the target isn't an agent pane; typing into a shell would run commands (agents.allow_send_to_shell)",
                    );
                }
                let w = Waiter {
                    kind: WaitKind::Send { caller: p, target, text, submit },
                    reply: Some(reply),
                    deadline: Some(now + Duration::from_secs(10)),
                };
                if let Some(ipc) = &mut self.ipc {
                    ipc.waiters.push(w);
                }
                self.resolve_waiters(cx);
            }
            Method::Msg { to, text } => {
                let Some(p) = cpane else { return reply.err(ErrorCode::Unauthorized, "no pane") };
                let Some(ws) = cws else { return reply.err(ErrorCode::NotFound, "no workspace") };
                let from = PaneRef::new(p.0, self.pane_name(p, cx));
                let (recipient, recipients, target_ws) = if to == "all" {
                    let names: Vec<String> = self
                        .workspace_panes(ws)
                        .into_iter()
                        .filter(|x| *x != p)
                        .map(|x| self.pane_name(x, cx))
                        .collect();
                    (Recipient::All, names, ws)
                } else {
                    match self.resolve_target(caller.as_ref(), &to, cx) {
                        Ok(t) => (
                            Recipient::Pane(t.0),
                            vec![self.pane_name(t, cx)],
                            self.pane_workspace(t).unwrap_or(ws),
                        ),
                        Err(e) => return reply.error(e),
                    }
                };
                let coop = self.coop.entry(target_ws).or_insert_with(|| crate::root::Coop::new(target_ws));
                let id = coop.bus.post(&from, recipient, text.clone(), SystemTime::now());
                coop.activity.record(
                    ActivityKind::Message,
                    from.name.clone(),
                    format!("→ {}: {}", recipients.join(", "), text),
                    SystemTime::now(),
                );
                reply.ok_typed(&MsgResult { ids: vec![id.0], recipients });
                self.coop_changed(cx);
                self.resolve_waiters(cx);
                self.deliver_typed_messages(cx);
            }
            Method::Inbox { wait, from, peek, timeout_s } => {
                let Some(p) = cpane else { return reply.err(ErrorCode::Unauthorized, "no pane") };
                let Some(ws) = cws else { return reply.err(ErrorCode::NotFound, "no workspace") };
                let from = match from {
                    Some(f) => match self.resolve_target(caller.as_ref(), &f, cx) {
                        Ok(t) => Some(t.0),
                        Err(e) => return reply.error(e),
                    },
                    None => None,
                };
                let coop = self.coop.entry(ws).or_insert_with(|| crate::root::Coop::new(ws));
                let msgs = coop.bus.inbox(p.0, from, peek);
                if !msgs.is_empty() || !wait {
                    reply.ok_typed(&InboxResult { messages: msgs.into_iter().map(wire).collect() });
                    self.coop_changed(cx);
                } else if let Some(ipc) = &mut self.ipc {
                    ipc.waiters.push(Waiter {
                        kind: WaitKind::Inbox { pane: p, ws, from, peek },
                        reply: Some(reply),
                        deadline: timeout_s.map(|s| now + Duration::from_secs_f64(s)),
                    });
                }
            }
            Method::Wait { pane, state, timeout_s } => {
                let target = match self.resolve_target(caller.as_ref(), &pane, cx) {
                    Ok(t) => t,
                    Err(e) => return reply.error(e),
                };
                if let Some(ipc) = &mut self.ipc {
                    ipc.waiters.push(Waiter {
                        kind: WaitKind::State { pane: target, state },
                        reply: Some(reply),
                        deadline: timeout_s.map(|s| now + Duration::from_secs_f64(s)),
                    });
                }
                self.resolve_waiters(cx);
            }
            Method::Claim { paths, reason, wait, timeout_s } => {
                let Some(p) = cpane else { return reply.err(ErrorCode::Unauthorized, "no pane") };
                let Some(ws) = cws else { return reply.err(ErrorCode::NotFound, "no workspace") };
                let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
                match self.try_claim(p, ws, &paths, reason.clone(), cx) {
                    Ok(claimed) => reply.ok_typed(&ClaimResult { claimed }),
                    Err(conflict) if wait => {
                        let _ = conflict;
                        if let Some(ipc) = &mut self.ipc {
                            ipc.waiters.push(Waiter {
                                kind: WaitKind::Claim { pane: p, ws, paths, reason },
                                reply: Some(reply),
                                deadline: timeout_s.map(|s| now + Duration::from_secs_f64(s)),
                            });
                        }
                    }
                    Err(conflict) => reply.error(conflict),
                }
            }
            Method::Release { paths } => {
                let Some(p) = cpane else { return reply.err(ErrorCode::Unauthorized, "no pane") };
                let Some(ws) = cws else { return reply.err(ErrorCode::NotFound, "no workspace") };
                let root = self.ws.index_of(ws).and_then(|i| self.ws.list[i].root.clone());
                let name = self.pane_name(p, cx);
                let coop = self.coop.entry(ws).or_insert_with(|| crate::root::Coop::new(ws));
                let released = if paths.is_empty() {
                    coop.claims.release_all(p.0)
                } else {
                    let ps: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
                    coop.claims.release(p.0, Some(&ps), root.as_deref())
                };
                let released: Vec<String> =
                    released.into_iter().map(|c| c.path.to_string_lossy().into_owned()).collect();
                if !released.is_empty() {
                    coop.activity.record(ActivityKind::Release, name, released.join(", "), SystemTime::now());
                }
                reply.ok_typed(&ReleaseResult { released });
                self.coop_changed(cx);
                self.resolve_waiters(cx);
            }
            Method::Claims => {
                let Some(ws) = cws else { return reply.err(ErrorCode::NotFound, "no workspace") };
                let claims = self
                    .coop
                    .get(&ws)
                    .map(|c| {
                        c.claims
                            .list()
                            .iter()
                            .map(|c| ClaimWire {
                                path: c.path.to_string_lossy().into_owned(),
                                holder: c.holder,
                                holder_name: c.holder_name.clone(),
                                reason: c.reason.clone(),
                                since_ms: c.renewed.elapsed().as_millis() as u64,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                reply.ok_typed(&ClaimsResult { claims });
            }
            Method::Spawn { placement, name, cwd, command } => {
                let Some(p) = cpane else { return reply.err(ErrorCode::Unauthorized, "no pane") };
                let Some(ws) = cws else { return reply.err(ErrorCode::NotFound, "no workspace") };
                let cmd = if command.is_empty() {
                    None
                } else if command.len() == 1 {
                    Some(command[0].clone())
                } else {
                    Some(command.iter().map(|a| shell_escape(a)).collect::<Vec<_>>().join(" "))
                };
                let caller_cwd = self.panes.get(&p).and_then(|e| e.read(cx).cwd.clone());
                let spawn = PaneSpawn {
                    cwd: cwd.map(PathBuf::from).or(caller_cwd),
                    startup: cmd.clone().map(Startup::Run),
                    name: name.clone(),
                    ..Default::default()
                };
                let caller_name = self.pane_name(p, cx);
                let id = self.spawn_from_ipc(ws, p, placement, spawn, window, cx);
                match id {
                    Some(id) => {
                        if let Some(coop) = self.coop.get_mut(&ws) {
                            coop.activity.record(
                                ActivityKind::Spawn,
                                caller_name,
                                format!(
                                    "{} {}",
                                    name.unwrap_or_else(|| id.to_string()),
                                    cmd.unwrap_or_default()
                                ),
                                SystemTime::now(),
                            );
                        }
                        reply.ok_typed(&SpawnResult { pane_id: id.0 });
                    }
                    None => reply.err(ErrorCode::Conflict, "not enough room to split; try --tab"),
                }
            }
            Method::Notify { title, body } => {
                let Some(p) = cpane else { return reply.err(ErrorCode::Unauthorized, "no pane") };
                if !self.is_looked_at(p) {
                    self.trigger_attention(p, &title, &body.unwrap_or_default(), cx);
                }
                reply.ok(serde_json::json!({}));
            }
            Method::Open { path, force_new } => {
                let path = PathBuf::from(path);
                if !path.is_dir() {
                    return reply.err(ErrorCode::NotFound, format!("{} is not a directory", path.display()));
                }
                let ws = self.open_folder(path, force_new, window, cx);
                cx.activate(true);
                reply.ok_typed(&OpenResult { workspace_id: ws.0 });
            }
        }
        cx.notify();
    }

    fn workspace_panes(&self, ws: WorkspaceId) -> Vec<PaneId> {
        self.ws
            .index_of(ws)
            .map(|i| self.ws.list[i].tabs.iter().flat_map(nox_layout::Tab::panes).collect())
            .unwrap_or_default()
    }

    fn ls(&self, caller_ws: Option<WorkspaceId>, all: bool, cx: &App) -> LsResult {
        let mut workspaces = Vec::new();
        for (i, w) in self.ws.list.iter().enumerate() {
            if !all && Some(w.id) != caller_ws {
                continue;
            }
            let tabs = w
                .tabs
                .iter()
                .enumerate()
                .map(|(ti, t)| TabInfo {
                    id: t.id.0,
                    title: self.tab_title(t, ti, cx),
                    panes: t
                        .panes()
                        .into_iter()
                        .map(|p| {
                            let e = self.panes.get(&p).map(|e| e.read(cx));
                            PaneInfo {
                                id: p.0,
                                name: self.pane_name(p, cx),
                                program: e.and_then(super::pane::Pane::process_display_name),
                                agent_state: e
                                    .and_then(super::pane::Pane::agent_state)
                                    .map(|s| s.to_string()),
                                cwd: e.and_then(|e| e.cwd.as_ref().map(|c| c.to_string_lossy().into_owned())),
                                branch: e.and_then(|e| e.branch.clone()),
                                focused: t.focused == p,
                            }
                        })
                        .collect(),
                })
                .collect();
            workspaces.push(WsInfo {
                id: w.id.0,
                name: w.name.clone(),
                root: w.root.as_ref().map(|r| r.to_string_lossy().into_owned()),
                active: i == self.ws.active,
                tabs,
            });
        }
        LsResult { workspaces }
    }

    fn try_claim(
        &mut self,
        p: PaneId,
        ws: WorkspaceId,
        paths: &[PathBuf],
        reason: Option<String>,
        cx: &mut Context<Self>,
    ) -> Result<Vec<String>, IpcError> {
        let root = self.ws.index_of(ws).and_then(|i| self.ws.list[i].root.clone());
        let name = self.pane_name(p, cx);
        let coop = self.coop.entry(ws).or_insert_with(|| crate::root::Coop::new(ws));
        match coop.claims.claim(p.0, &name, paths, reason.as_deref(), root.as_deref(), Instant::now()) {
            Ok(()) => {
                let claimed: Vec<String> = paths
                    .iter()
                    .map(|x| {
                        nox_agents::normalize_claim_path(x, root.as_deref()).to_string_lossy().into_owned()
                    })
                    .collect();
                coop.activity.record(
                    ActivityKind::Claim,
                    name,
                    format!(
                        "{}{}",
                        claimed.join(", "),
                        reason.map(|r| format!(" — {r}")).unwrap_or_default()
                    ),
                    SystemTime::now(),
                );
                self.coop_changed(cx);
                Ok(claimed)
            }
            Err(c) => {
                let msg = format!(
                    "{} is claimed by {}{}",
                    c.path.display(),
                    c.holder_name,
                    c.reason.as_ref().map(|r| format!(" ({r})")).unwrap_or_default()
                );
                let data = ClaimConflictsData {
                    conflicts: vec![ClaimConflictWire {
                        path: c.path.to_string_lossy().into_owned(),
                        holder: c.holder,
                        holder_name: c.holder_name.clone(),
                        reason: c.reason.clone(),
                    }],
                };
                Err(IpcError::new(ErrorCode::Conflict, msg)
                    .with_data(serde_json::to_value(data).unwrap_or_default()))
            }
        }
    }

    /// `nox spawn`: a new tab or a split of the caller's pane, without taking focus.
    fn spawn_from_ipc(
        &mut self,
        ws: WorkspaceId,
        caller: PaneId,
        placement: Placement,
        spawn: PaneSpawn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PaneId> {
        let w = self.ws.index_of(ws)?;
        let pane = PaneId::next();
        match placement {
            Placement::Tab => {
                let tab_id = nox_layout::TabId::next();
                // Insert after the caller's tab, keeping the user's active tab.
                let after = self.ws.find_pane(caller).map_or(self.ws.list[w].tabs.len(), |(_, t)| t + 1);
                self.ws.list[w].insert_tab(after, nox_layout::Tab::new(tab_id, pane));
                self.spawn_pane(pane, w, tab_id, spawn, window, cx);
            }
            Placement::SplitRight | Placement::SplitBelow => {
                let dir = if placement == Placement::SplitRight {
                    SplitDirection::Vertical
                } else {
                    SplitDirection::Horizontal
                };
                let (wi, ti) = self.ws.find_pane(caller)?;
                let area = self.layout_area(window, cx);
                let (cell, pad) = self.cell_and_padding(window, cx);
                let tab = &mut self.ws.list[wi].tabs[ti];
                if !tab.layout.can_split(caller, dir, area, cell, pad, crate::root::DIVIDER_PX) {
                    return None;
                }
                let tab_id = tab.id;
                let prev_focus = tab.focused;
                tab.focus(caller);
                tab.split_focused(dir, pane);
                tab.focus(prev_focus);
                self.spawn_pane(pane, wi, tab_id, spawn, window, cx);
                self.sync_maximized(cx);
            }
        }
        self.session_changed(cx);
        cx.notify();
        Some(pane)
    }

    /// Re-check held requests (state waits, inbox waits, claim waits, deferred sends).
    pub fn resolve_waiters(&mut self, cx: &mut Context<Self>) {
        let Some(ipc) = &mut self.ipc else { return };
        let mut waiters = std::mem::take(&mut ipc.waiters);
        let now = Instant::now();
        for w in &mut waiters {
            let Some(reply) = w.reply.take() else { continue };
            let done: Option<Result<serde_json::Value, IpcError>> = match &w.kind {
                WaitKind::State { pane, state } => {
                    let current = self.panes.get(pane).map(|e| {
                        let e = e.read(cx);
                        (e.exited.is_some(), e.agent_state())
                    });
                    let reached = match (current, state) {
                        (None | Some((true, _)), _)
                        | (Some((_, Some(AgentState::Exited))), WaitState::Exited) => Some(WaitState::Exited),
                        (Some((_, Some(AgentState::Idle))), WaitState::Idle) => Some(WaitState::Idle),
                        (Some((_, Some(AgentState::NeedsInput))), WaitState::NeedsInput) => {
                            Some(WaitState::NeedsInput)
                        }
                        (Some((_, None)), WaitState::Idle) => {
                            // Not an agent: idle when nothing runs.
                            let busy = self
                                .panes
                                .get(pane)
                                .is_some_and(|e| e.read(cx).running_since.is_some() || e.read(cx).is_busy());
                            (!busy).then_some(WaitState::Idle)
                        }
                        _ => None,
                    };
                    reached.map(|s| Ok(serde_json::to_value(WaitResult { state: s }).unwrap_or_default()))
                }
                WaitKind::Inbox { pane, ws, from, peek } => {
                    let coop = self.coop.get_mut(ws);
                    coop.and_then(|c| {
                        let msgs = c.bus.inbox(pane.0, *from, *peek);
                        (!msgs.is_empty()).then(|| {
                            Ok(serde_json::to_value(InboxResult {
                                messages: msgs.into_iter().map(wire).collect(),
                            })
                            .unwrap_or_default())
                        })
                    })
                }
                WaitKind::Claim { .. } => None,
                WaitKind::Send { caller, target, text, submit } => {
                    let typed_recently = self
                        .panes
                        .get(target)
                        .and_then(|e| e.read(cx).last_user_input)
                        .is_some_and(|t| now.duration_since(t) < Duration::from_secs(2));
                    if typed_recently {
                        None
                    } else if let Some(e) = self.panes.get(target).cloned() {
                        let from = self.pane_name(*caller, cx);
                        let (text, submit) = (text.clone(), *submit);
                        e.update(cx, |e, cx| {
                            if let Some(s) = &e.session {
                                s.paste_bracketed(&text, submit);
                            }
                            e.from_banner = Some((from.clone(), Instant::now()));
                            cx.notify();
                        });
                        if let Some(ws) = self.pane_workspace(*target)
                            && let Some(c) = self.coop.get_mut(&ws)
                        {
                            let short: String = text.chars().take(80).collect();
                            c.activity.record(ActivityKind::Send, from, short, SystemTime::now());
                        }
                        Some(Ok(serde_json::json!({ "sent": true })))
                    } else {
                        Some(Err(err(ErrorCode::NotFound, "target pane closed")))
                    }
                }
            };
            match done {
                Some(Ok(v)) => reply.ok(v),
                Some(Err(e)) => reply.error(e),
                None => w.reply = Some(reply),
            }
        }
        // Claim waiters need &mut self for try_claim.
        let mut rest = Vec::new();
        for mut w in waiters {
            if let WaitKind::Claim { pane, ws, paths, reason } = &w.kind
                && let Some(reply) = w.reply.take()
            {
                match self.try_claim(*pane, *ws, paths, reason.clone(), cx) {
                    Ok(claimed) => {
                        reply.ok_typed(&ClaimResult { claimed });
                        continue;
                    }
                    Err(_) => w.reply = Some(reply),
                }
            }
            if w.reply.is_some() {
                rest.push(w);
            }
        }
        if let Some(ipc) = &mut self.ipc {
            rest.append(&mut ipc.waiters);
            ipc.waiters = rest;
        }
        self.coop_changed(cx);
    }

    /// `agents.deliver = "type"`: type queued messages into idle agents' prompts.
    pub fn deliver_typed_messages(&mut self, cx: &mut Context<Self>) {
        if Settings::global(cx).config.agents.deliver != nox_config::Deliver::Type {
            return;
        }
        let mut todo: Vec<(PaneId, Vec<(String, String)>)> = Vec::new();
        for w in &self.ws.list {
            let Some(coop) = self.coop.get_mut(&w.id) else { continue };
            for t in &w.tabs {
                for p in t.panes() {
                    let Some(e) = self.panes.get(&p) else { continue };
                    let e = e.read(cx);
                    if e.agent_state() != Some(AgentState::Idle) || coop.bus.unread_count(p.0) == 0 {
                        continue;
                    }
                    let msgs = coop.bus.inbox(p.0, None, false);
                    todo.push((p, msgs.into_iter().map(|m| (m.from_name, m.text)).collect()));
                }
            }
        }
        for (p, msgs) in todo {
            if let Some(e) = self.panes.get(&p) {
                e.update(cx, |e, cx| {
                    for (from, text) in msgs {
                        if let Some(s) = &e.session {
                            s.paste_bracketed(&format!("[from {from}] {text}"), true);
                        }
                        e.from_banner = Some((from, Instant::now()));
                    }
                    cx.notify();
                });
            }
        }
    }
}

fn wire(m: nox_agents::Message) -> MessageWire {
    MessageWire { id: m.id.0, from: m.from, from_name: m.from_name, text: m.text, ts_ms: m.ts }
}
