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
                    // Conflict: wait for the paths to be released (see `resolve_waiters`).
                    Err(_) if wait => {
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
        if todo.is_empty() {
            return;
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
        // The messages are read now: refresh the unread badges in the pane headers.
        self.coop_changed(cx);
    }
}

fn wire(m: nox_agents::Message) -> MessageWire {
    MessageWire { id: m.id.0, from: m.from, from_name: m.from_name, text: m.text, ts_ms: m.ts }
}

#[cfg(test)]
mod tests {
    //! `handle_ipc` driven with real `Responder`s: a private `nox_ipc::Server` on a scratch
    //! socket hands each request to the test thread, which applies it to `Root` and reads the
    //! response line back from its client end.

    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::{Mutex, mpsc};

    use gpui::{Entity, TestAppContext, VisualTestContext};
    use nox_config::Config;
    use nox_ipc::{PROTOCOL_VERSION, Request as WireRequest, Response, Server, Tokens};
    use serde_json::{Value, json};

    use super::*;
    use crate::root::Initial;

    type Captured = (Option<Caller>, Method, Responder);

    struct Capture(Mutex<mpsc::Sender<Captured>>);

    impl Handler for Capture {
        fn handle(&self, caller: Option<Caller>, method: Method, reply: Responder) {
            let _ = self.0.lock().unwrap().send((caller, method, reply));
        }
    }

    struct Harness<'a> {
        root: Entity<Root>,
        cx: &'a mut VisualTestContext,
        rx: mpsc::Receiver<Captured>,
        writer: UnixStream,
        reader: BufReader<UnixStream>,
        tokens: Tokens,
        stash: HashMap<u64, Result<Value, IpcError>>,
        next_id: u64,
        _dir: tempfile::TempDir,
    }

    fn base_config() -> Config {
        let mut c = crate::test_support::config();
        // Keep native side effects (power assertions, sounds, notifications) out of tests.
        c.agents.keep_awake = false;
        c.terminal.attention_sound = false;
        c.terminal.notifications = false;
        c
    }

    fn setup(cx: &mut TestAppContext, tweak: impl FnOnce(&mut Config)) -> Harness<'_> {
        let mut config = base_config();
        tweak(&mut config);
        cx.update(|cx| crate::test_support::init_app(cx, config));
        let (root, vcx) = cx.add_window_view(|window, cx| Root::new(Initial::Fresh, window, cx));
        vcx.run_until_parked();
        let dir = tempfile::Builder::new().prefix("nx").tempdir().unwrap();
        let path = dir.path().join("s");
        let (tx, rx) = mpsc::channel();
        let handle = Server::start(&path, Arc::new(Capture(Mutex::new(tx)))).unwrap();
        let tokens = handle.tokens().clone();
        root.update(vcx, |r, _| r.ipc = Some(IpcState { handle, waiters: vec![] }));
        let writer = UnixStream::connect(&path).unwrap();
        let reader = BufReader::new(writer.try_clone().unwrap());
        Harness { root, cx: vcx, rx, writer, reader, tokens, stash: HashMap::new(), next_id: 1, _dir: dir }
    }

    fn code(r: &Result<Value, IpcError>) -> Option<ErrorCode> {
        r.as_ref().err().map(|e| e.code)
    }

    impl Harness<'_> {
        /// Send `method` as `caller` (token-authenticated) and apply it to `Root`; returns the id.
        fn send(&mut self, caller: Option<Caller>, method: Method) -> u64 {
            let id = self.next_id;
            self.next_id += 1;
            // Without a caller the server only lets `ls` / `open` through; borrow an `ls`
            // responder and hand `Root` the real method (what a token-less request would get).
            let (token, wire) = match caller {
                Some(c) => (Some(self.tokens.issue(c)), method.clone()),
                None => (None, Method::Ls { all: false }),
            };
            let req = WireRequest { v: PROTOCOL_VERSION, id, token, method: wire };
            let mut line = serde_json::to_string(&req).unwrap();
            line.push('\n');
            self.writer.write_all(line.as_bytes()).unwrap();
            let (got_caller, _, reply) = self.rx.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(got_caller, caller);
            self.root.update_in(self.cx, |r, window, cx| r.handle_ipc(caller, method, reply, window, cx));
            self.cx.run_until_parked();
            id
        }

        fn call(&mut self, caller: Option<Caller>, method: Method) -> Result<Value, IpcError> {
            let id = self.send(caller, method);
            self.response(id).expect("no response")
        }

        fn read_lines(&mut self, timeout: Duration) {
            self.reader.get_ref().set_read_timeout(Some(timeout)).unwrap();
            let mut line = String::new();
            while let Ok(n) = self.reader.read_line(&mut line) {
                if n == 0 {
                    break;
                }
                let r: Response = serde_json::from_str(&line).unwrap();
                self.stash.insert(r.id, r.into_result());
                line.clear();
                // Anything already written is available at once; don't wait for more.
                self.reader.get_ref().set_read_timeout(Some(Duration::from_millis(20))).unwrap();
            }
        }

        /// The response to `id`, if one was written (responses are written synchronously).
        fn response(&mut self, id: u64) -> Option<Result<Value, IpcError>> {
            if !self.stash.contains_key(&id) {
                self.read_lines(Duration::from_millis(200));
            }
            self.stash.remove(&id)
        }

        fn caller(&mut self, p: PaneId) -> Caller {
            self.root.read_with(self.cx, |r, _| {
                let (w, t) = r.ws.find_pane(p).unwrap();
                Caller { pane_id: p.0, tab_id: r.ws.list[w].tabs[t].id.0, workspace_id: r.ws.list[w].id.0 }
            })
        }

        fn first_pane(&mut self) -> PaneId {
            self.root.read_with(self.cx, |r, _| r.ws.list[0].tabs[0].focused)
        }

        fn spawn(&mut self, from: PaneId, placement: Placement, name: Option<&str>) -> PaneId {
            let c = self.caller(from);
            let v = self
                .call(
                    Some(c),
                    Method::Spawn { placement, name: name.map(str::to_string), cwd: None, command: vec![] },
                )
                .unwrap();
            PaneId(v["pane_id"].as_u64().unwrap())
        }

        /// A pane standing in for an agent: `sleep` runs in the foreground (so process polling
        /// doesn't see the shell come back and mark the agent exited), then it reports `state`.
        fn agent(&mut self, from: PaneId, name: &str, state: AgentStateWire) -> PaneId {
            let c = self.caller(from);
            let v = self
                .call(
                    Some(c),
                    Method::Spawn {
                        placement: Placement::Tab,
                        name: Some(name.into()),
                        cwd: None,
                        command: vec!["sleep".into(), "600".into()],
                    },
                )
                .unwrap();
            let p = PaneId(v["pane_id"].as_u64().unwrap());
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                self.root.update(self.cx, |r, cx| r.panes[&p].update(cx, crate::pane::Pane::refresh_info));
                let fg = self.pane(p, crate::pane::Pane::process_display_name);
                if fg.as_deref() == Some("sleep") {
                    break;
                }
                assert!(Instant::now() < deadline, "sleep never reached the foreground");
                self.cx.run_until_parked();
                std::thread::sleep(Duration::from_millis(10));
            }
            self.status(p, state);
            p
        }

        fn status(&mut self, p: PaneId, state: AgentStateWire) {
            let c = self.caller(p);
            self.call(Some(c), Method::Status { state, message: None, session: None }).unwrap();
        }

        fn tick(&mut self) {
            self.root.update(self.cx, Root::ipc_tick);
            self.cx.run_until_parked();
        }

        fn expire_waiters_now(&mut self) {
            self.root.update(self.cx, |r, _| {
                for w in &mut r.ipc.as_mut().unwrap().waiters {
                    w.deadline = Some(Instant::now());
                }
            });
        }

        fn waiter_count(&mut self) -> usize {
            self.root.read_with(self.cx, |r, _| r.ipc.as_ref().unwrap().waiters.len())
        }

        fn pane<R>(&mut self, p: PaneId, f: impl FnOnce(&crate::pane::Pane) -> R) -> R {
            self.root.read_with(self.cx, |r, cx| f(r.panes[&p].read(cx)))
        }

        fn activity(&mut self, p: PaneId) -> Vec<(ActivityKind, String)> {
            self.root.read_with(self.cx, |r, _| {
                let ws = r.pane_workspace(p).unwrap();
                r.coop[&ws].activity.newest_first().map(|e| (e.kind, e.detail.clone())).collect()
            })
        }
    }

    fn msg(to: &str, text: &str) -> Method {
        Method::Msg { to: to.into(), text: text.into() }
    }

    fn inbox(wait: bool, peek: bool, timeout_s: Option<f64>) -> Method {
        Method::Inbox { wait, from: None, peek, timeout_s }
    }

    fn claim(paths: &[&str], reason: Option<&str>, wait: bool, timeout_s: Option<f64>) -> Method {
        Method::Claim {
            paths: paths.iter().map(|s| (*s).to_string()).collect(),
            reason: reason.map(str::to_string),
            wait,
            timeout_s,
        }
    }

    #[gpui::test]
    fn methods_needing_a_pane_refuse_token_less_callers(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let methods = [
            Method::Name { name: "x".into() },
            Method::Status { state: AgentStateWire::Idle, message: None, session: None },
            Method::Send { pane: "1".into(), text: "x".into(), submit: false },
            msg("all", "x"),
            inbox(false, false, None),
            claim(&["/p/a"], None, false, None),
            Method::Release { paths: vec![] },
            Method::Spawn { placement: Placement::Tab, name: None, cwd: None, command: vec![] },
            Method::Notify { title: "t".into(), body: None },
        ];
        for m in methods {
            let name = m.name();
            assert_eq!(code(&h.call(None, m)), Some(ErrorCode::Unauthorized), "{name}");
        }
        assert_eq!(code(&h.call(None, Method::Claims)), Some(ErrorCode::NotFound));
    }

    #[gpui::test]
    fn ls_lists_the_callers_workspace_unless_all(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let a = h.first_pane();
        let ca = h.caller(a);
        let v = h.call(Some(ca), Method::Ls { all: false }).unwrap();
        let ls: LsResult = serde_json::from_value(v).unwrap();
        assert_eq!(ls.workspaces.len(), 1);
        assert_eq!(ls.workspaces[0].id, ca.workspace_id);
        assert!(ls.workspaces[0].active);
        let pane = &ls.workspaces[0].tabs[0].panes[0];
        assert_eq!(pane.id, a.0);
        assert!(pane.focused);
        assert_eq!(pane.agent_state, None);

        // A second workspace (via `open`) only shows up with `all`.
        let dir = tempfile::tempdir().unwrap();
        let v = h
            .call(Some(ca), Method::Open { path: dir.path().to_string_lossy().into(), force_new: false })
            .unwrap();
        let ws2 = v["workspace_id"].as_u64().unwrap();
        assert_ne!(ws2, ca.workspace_id);
        let ls: LsResult =
            serde_json::from_value(h.call(Some(ca), Method::Ls { all: false }).unwrap()).unwrap();
        assert_eq!(ls.workspaces.len(), 1);
        let ls: LsResult =
            serde_json::from_value(h.call(Some(ca), Method::Ls { all: true }).unwrap()).unwrap();
        assert_eq!(ls.workspaces.len(), 2);
        assert!(ls.workspaces[1].active, "open switches to the new workspace");
        assert!(ls.workspaces[1].root.is_some());

        // Without a token: every workspace, but no tabs.
        let ls: LsResult = serde_json::from_value(h.call(None, Method::Ls { all: false }).unwrap()).unwrap();
        assert_eq!(ls.workspaces.len(), 2);
        assert!(ls.workspaces.iter().all(|w| w.tabs.is_empty()));
    }

    #[gpui::test]
    fn open_requires_a_directory_and_reuses_the_workspace(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let r = h.call(None, Method::Open { path: "/nonexistent/nox-test".into(), force_new: false });
        assert_eq!(code(&r), Some(ErrorCode::NotFound));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().into_owned();
        let a = h.call(None, Method::Open { path: path.clone(), force_new: false }).unwrap();
        let b = h.call(None, Method::Open { path: path.clone(), force_new: false }).unwrap();
        assert_eq!(a["workspace_id"], b["workspace_id"]);
        let c = h.call(None, Method::Open { path, force_new: true }).unwrap();
        assert_ne!(a["workspace_id"], c["workspace_id"]);
        assert_eq!(h.root.read_with(h.cx, |r, _| r.ws.list.len()), 3);
    }

    #[gpui::test]
    fn name_validates_and_rejects_duplicates(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let a = h.first_pane();
        let b = h.spawn(a, Placement::Tab, None);
        let (ca, cb) = (h.caller(a), h.caller(b));
        for bad in ["", "   ", "42"] {
            let r = h.call(Some(ca), Method::Name { name: bad.into() });
            assert_eq!(code(&r), Some(ErrorCode::Usage), "{bad:?}");
        }
        let v = h.call(Some(ca), Method::Name { name: "  backend ".into() }).unwrap();
        assert_eq!(v, json!({ "name": "backend" }));
        assert_eq!(h.pane(a, |p| p.custom_name.clone()), Some("backend".into()));
        // Renaming to one's own name is fine; taking another pane's name is not.
        assert!(h.call(Some(ca), Method::Name { name: "backend".into() }).is_ok());
        let r = h.call(Some(cb), Method::Name { name: "backend".into() });
        assert_eq!(code(&r), Some(ErrorCode::Conflict));
        assert_eq!(h.pane(b, |p| p.custom_name.clone()), None);
    }

    #[gpui::test]
    fn status_makes_the_pane_an_agent_and_logs_activity(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let first = h.first_pane();
        let a = h.agent(first, "agent", AgentStateWire::Working);
        let ca = h.caller(a);
        let v = h
            .call(
                Some(ca),
                Method::Status {
                    state: AgentStateWire::NeedsInput,
                    message: Some("approve?".into()),
                    session: Some("s1".into()),
                },
            )
            .unwrap();
        assert_eq!(v, json!({ "state": "needs-input" }));
        assert!(h.pane(a, crate::pane::Pane::is_agent));
        assert_eq!(h.pane(a, crate::pane::Pane::agent_state), Some(AgentState::NeedsInput));
        let act = h.activity(a);
        assert_eq!(act[0], (ActivityKind::Status, "needs-input — approve?".into()));
        let ls: LsResult =
            serde_json::from_value(h.call(Some(ca), Method::Ls { all: false }).unwrap()).unwrap();
        let info = ls.workspaces[0].tabs.iter().flat_map(|t| &t.panes).find(|i| i.id == a.0).unwrap();
        assert_eq!(info.agent_state.as_deref(), Some("needs-input"));
        assert_eq!(info.name, "agent");
    }

    #[gpui::test]
    fn read_returns_pane_text_or_not_found(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let a = h.first_pane();
        let b = h.spawn(a, Placement::Tab, Some("other"));
        let ca = h.caller(a);
        for mode in [ReadMode::Screen, ReadMode::Lines(3), ReadMode::Scrollback] {
            let v = h.call(Some(ca), Method::Read { pane: b.0.to_string(), mode }).unwrap();
            assert!(v["text"].is_string(), "{mode:?}");
        }
        // By name, too.
        assert!(h.call(Some(ca), Method::Read { pane: "other".into(), mode: ReadMode::Screen }).is_ok());
        // `/bin/sh` has no shell integration, so there is no finished command output.
        let r = h.call(Some(ca), Method::Read { pane: b.0.to_string(), mode: ReadMode::LastOutput });
        assert_eq!(code(&r), Some(ErrorCode::NotFound));
        let r = h.call(Some(ca), Method::Read { pane: "nobody".into(), mode: ReadMode::Screen });
        assert_eq!(code(&r), Some(ErrorCode::NotFound));
        let r = h.call(Some(ca), Method::Read { pane: "999999".into(), mode: ReadMode::Screen });
        assert_eq!(code(&r), Some(ErrorCode::NotFound));
    }

    #[gpui::test]
    fn targets_in_other_workspaces_are_out_of_scope(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let a = h.first_pane();
        let ca = h.caller(a);
        let dir = tempfile::tempdir().unwrap();
        h.call(None, Method::Open { path: dir.path().to_string_lossy().into(), force_new: false }).unwrap();
        let other = h.root.read_with(h.cx, |r, _| r.ws.list[1].tabs[0].focused);
        let co = h.caller(other);
        h.call(Some(co), Method::Name { name: "far".into() }).unwrap();
        let r = h.call(Some(ca), Method::Read { pane: other.0.to_string(), mode: ReadMode::Screen });
        assert_eq!(code(&r), Some(ErrorCode::Forbidden));
        let r = h.call(Some(ca), Method::Read { pane: "far".into(), mode: ReadMode::Screen });
        assert_eq!(code(&r), Some(ErrorCode::NotFound));
        // The same name may be used again in another workspace.
        assert!(h.call(Some(ca), Method::Name { name: "far".into() }).is_ok());
    }

    #[gpui::test]
    fn scope_all_reaches_other_workspaces(cx: &mut TestAppContext) {
        let mut h = setup(cx, |c| c.agents.scope = nox_config::AgentScope::All);
        let a = h.first_pane();
        let ca = h.caller(a);
        let dir = tempfile::tempdir().unwrap();
        h.call(None, Method::Open { path: dir.path().to_string_lossy().into(), force_new: false }).unwrap();
        let other = h.root.read_with(h.cx, |r, _| r.ws.list[1].tabs[0].focused);
        let co = h.caller(other);
        h.call(Some(co), Method::Name { name: "far".into() }).unwrap();
        assert!(h.call(Some(ca), Method::Read { pane: other.0.to_string(), mode: ReadMode::Screen }).is_ok());
        assert!(h.call(Some(ca), Method::Read { pane: "far".into(), mode: ReadMode::Screen }).is_ok());
        // Default `ls` covers every workspace with scope = all.
        let ls: LsResult =
            serde_json::from_value(h.call(Some(ca), Method::Ls { all: false }).unwrap()).unwrap();
        assert_eq!(ls.workspaces.len(), 2);
        // A message to a pane in another workspace lands on that workspace's bus.
        h.call(Some(ca), msg("far", "hi")).unwrap();
        let v = h.call(Some(co), inbox(false, false, None)).unwrap();
        assert_eq!(v["messages"][0]["text"], "hi");
    }

    #[gpui::test]
    fn send_into_a_shell_needs_opt_in(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let a = h.first_pane();
        h.spawn(a, Placement::Tab, Some("sh"));
        let ca = h.caller(a);
        let r = h.call(Some(ca), Method::Send { pane: "sh".into(), text: "ls".into(), submit: true });
        assert_eq!(code(&r), Some(ErrorCode::Forbidden));
        // Agent panes accept sends.
        let b = h.agent(a, "bot", AgentStateWire::Idle);
        let v = h
            .call(Some(ca), Method::Send { pane: "bot".into(), text: "hello".into(), submit: false })
            .unwrap();
        assert_eq!(v, json!({ "sent": true }));
        assert_eq!(h.pane(b, |p| p.from_banner.as_ref().map(|(n, _)| n.clone())), Some(h_name(&mut h, a)));
        assert_eq!(h.activity(b)[0], (ActivityKind::Send, "hello".into()));
        assert_eq!(h.waiter_count(), 0);
    }

    fn h_name(h: &mut Harness<'_>, p: PaneId) -> String {
        h.root.read_with(h.cx, |r, cx| r.pane_name(p, cx))
    }

    #[gpui::test]
    fn send_to_shell_allowed_by_config(cx: &mut TestAppContext) {
        let mut h = setup(cx, |c| c.agents.allow_send_to_shell = true);
        let a = h.first_pane();
        let b = h.spawn(a, Placement::Tab, None);
        let ca = h.caller(a);
        let v = h
            .call(Some(ca), Method::Send { pane: b.0.to_string(), text: "true".into(), submit: true })
            .unwrap();
        assert_eq!(v, json!({ "sent": true }));
    }

    #[gpui::test]
    fn send_waits_while_the_user_types_then_times_out(cx: &mut TestAppContext) {
        let mut h = setup(cx, |c| c.agents.allow_send_to_shell = true);
        let a = h.first_pane();
        let b = h.spawn(a, Placement::Tab, None);
        let ca = h.caller(a);
        h.root.update(h.cx, |r, cx| r.panes[&b].update(cx, |p, _| p.last_user_input = Some(Instant::now())));
        let id = h.send(Some(ca), Method::Send { pane: b.0.to_string(), text: "x".into(), submit: false });
        assert!(h.response(id).is_none(), "held while the user types");
        assert_eq!(h.waiter_count(), 1);
        // A tick before the deadline keeps it held.
        h.tick();
        assert!(h.response(id).is_none());
        h.expire_waiters_now();
        h.tick();
        let r = h.response(id).unwrap();
        let e = r.unwrap_err();
        assert_eq!(e.code, ErrorCode::Timeout);
        assert!(e.message.contains("kept typing"), "{}", e.message);
        assert_eq!(h.waiter_count(), 0);
    }

    #[gpui::test]
    fn held_send_goes_through_once_typing_stops(cx: &mut TestAppContext) {
        let mut h = setup(cx, |c| c.agents.allow_send_to_shell = true);
        let a = h.first_pane();
        let b = h.spawn(a, Placement::Tab, None);
        let ca = h.caller(a);
        h.root.update(h.cx, |r, cx| r.panes[&b].update(cx, |p, _| p.last_user_input = Some(Instant::now())));
        let id = h.send(Some(ca), Method::Send { pane: b.0.to_string(), text: "x".into(), submit: false });
        assert!(h.response(id).is_none());
        h.root.update(h.cx, |r, cx| r.panes[&b].update(cx, |p, _| p.last_user_input = None));
        h.tick();
        assert_eq!(h.response(id).unwrap().unwrap(), json!({ "sent": true }));
    }

    #[gpui::test]
    fn messages_and_inbox(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let a = h.first_pane();
        let b = h.spawn(a, Placement::Tab, Some("bee"));
        let c = h.spawn(a, Placement::Tab, Some("cee"));
        let (ca, cb, cc) = (h.caller(a), h.caller(b), h.caller(c));

        let v = h.call(Some(ca), msg("bee", "one")).unwrap();
        let res: MsgResult = serde_json::from_value(v).unwrap();
        assert_eq!(res.recipients, vec!["bee".to_string()]);
        assert_eq!(res.ids.len(), 1);
        assert_eq!(h.pane(b, |p| p.unread), 1);
        assert_eq!(h.pane(c, |p| p.unread), 0);
        assert!(matches!(&h.activity(a)[0], (ActivityKind::Message, d) if d == "→ bee: one"));

        // Peek leaves it unread; a normal read marks it read.
        let v = h.call(Some(cb), inbox(false, true, None)).unwrap();
        assert_eq!(v["messages"].as_array().unwrap().len(), 1);
        assert_eq!(v["messages"][0]["from"], a.0);
        assert_eq!(h.pane(b, |p| p.unread), 1);
        let v = h.call(Some(cb), inbox(false, false, None)).unwrap();
        assert_eq!(v["messages"][0]["text"], "one");
        assert_eq!(h.pane(b, |p| p.unread), 0);
        let v = h.call(Some(cb), inbox(false, false, None)).unwrap();
        assert_eq!(v["messages"], json!([]));

        // `all`: everyone in the workspace but the sender.
        let res: MsgResult = serde_json::from_value(h.call(Some(ca), msg("all", "hey")).unwrap()).unwrap();
        let mut rec = res.recipients.clone();
        rec.sort();
        assert_eq!(rec, vec!["bee".to_string(), "cee".to_string()]);
        assert_eq!(h.pane(c, |p| p.unread), 1);
        assert_eq!(h.pane(a, |p| p.unread), 0);

        // `from` filters by sender; unknown senders / recipients are not found.
        h.call(Some(cb), msg("cee", "from bee")).unwrap();
        let v = h
            .call(
                Some(cc),
                Method::Inbox { wait: false, from: Some("bee".into()), peek: false, timeout_s: None },
            )
            .unwrap();
        let texts: Vec<&str> =
            v["messages"].as_array().unwrap().iter().map(|m| m["text"].as_str().unwrap()).collect();
        assert_eq!(texts, vec!["from bee"]);
        let r = h.call(
            Some(cc),
            Method::Inbox { wait: false, from: Some("ghost".into()), peek: false, timeout_s: None },
        );
        assert_eq!(code(&r), Some(ErrorCode::NotFound));
        assert_eq!(code(&h.call(Some(ca), msg("ghost", "x"))), Some(ErrorCode::NotFound));
    }

    #[gpui::test]
    fn inbox_wait_is_answered_by_the_next_message(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let a = h.first_pane();
        let b = h.spawn(a, Placement::Tab, Some("bee"));
        let (ca, cb) = (h.caller(a), h.caller(b));
        let id = h.send(Some(cb), inbox(true, false, None));
        assert!(h.response(id).is_none());
        assert_eq!(h.waiter_count(), 1);
        h.call(Some(ca), msg("bee", "wake up")).unwrap();
        let v = h.response(id).unwrap().unwrap();
        assert_eq!(v["messages"][0]["text"], "wake up");
        assert_eq!(h.waiter_count(), 0);
        // A wait with messages already queued answers at once.
        h.call(Some(ca), msg("bee", "queued")).unwrap();
        let v = h.call(Some(cb), inbox(true, false, None)).unwrap();
        assert_eq!(v["messages"][0]["text"], "queued");
    }

    #[gpui::test]
    fn inbox_wait_times_out(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let a = h.first_pane();
        let ca = h.caller(a);
        let id = h.send(Some(ca), inbox(true, false, Some(30.0)));
        h.tick();
        assert!(h.response(id).is_none(), "deadline not reached yet");
        h.expire_waiters_now();
        h.tick();
        let e = h.response(id).unwrap().unwrap_err();
        assert_eq!((e.code, e.message.as_str()), (ErrorCode::Timeout, "timed out"));
    }

    #[gpui::test]
    fn rate_limit_refuses_message_floods(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let a = h.first_pane();
        let b = h.spawn(a, Placement::Tab, Some("bee"));
        let ca = h.caller(a);
        for i in 0..nox_agents::RateLimiter::MAX_EVENTS {
            assert!(h.call(Some(ca), msg("bee", &format!("m{i}"))).is_ok(), "message {i}");
        }
        let r = h.call(Some(ca), msg("bee", "one too many"));
        assert_eq!(code(&r), Some(ErrorCode::RateLimited));
        assert_eq!(h.pane(b, |p| p.unread), nox_agents::RateLimiter::MAX_EVENTS);
        // Other methods aren't limited.
        assert!(h.call(Some(ca), Method::Claims).is_ok());
    }

    #[gpui::test]
    fn wait_for_agent_states(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let a = h.first_pane();
        let b = h.agent(a, "agent", AgentStateWire::NeedsInput);
        let ca = h.caller(a);
        let wait = |state, timeout_s| Method::Wait { pane: "agent".into(), state, timeout_s };

        let v = h.call(Some(ca), wait(WaitState::NeedsInput, None)).unwrap();
        assert_eq!(v, json!({ "state": "needs-input" }));

        h.status(b, AgentStateWire::Working);
        let id = h.send(Some(ca), wait(WaitState::Idle, None));
        assert!(h.response(id).is_none(), "held while the agent works");
        h.status(b, AgentStateWire::Idle);
        h.root.update(h.cx, Root::resolve_waiters);
        assert_eq!(h.response(id).unwrap().unwrap(), json!({ "state": "idle" }));

        let id = h.send(Some(ca), wait(WaitState::Exited, Some(5.0)));
        assert!(h.response(id).is_none());
        h.expire_waiters_now();
        h.tick();
        assert_eq!(code(&h.response(id).unwrap()), Some(ErrorCode::Timeout));

        let r =
            h.call(Some(ca), Method::Wait { pane: "ghost".into(), state: WaitState::Idle, timeout_s: None });
        assert_eq!(code(&r), Some(ErrorCode::NotFound));
    }

    #[gpui::test]
    fn wait_on_a_vanished_pane_reports_exited(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let a = h.first_pane();
        let b = h.spawn(a, Placement::Tab, None);
        let ca = h.caller(a);
        let id = h.send(
            Some(ca),
            Method::Wait { pane: b.0.to_string(), state: WaitState::NeedsInput, timeout_s: None },
        );
        assert!(h.response(id).is_none());
        h.root.update(h.cx, |r, cx| {
            r.panes.remove(&b);
            r.resolve_waiters(cx);
        });
        assert_eq!(h.response(id).unwrap().unwrap(), json!({ "state": "exited" }));
    }

    #[gpui::test]
    fn claims_conflict_wait_and_release(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let a = h.first_pane();
        let b = h.spawn(a, Placement::Tab, Some("bee"));
        let (ca, cb) = (h.caller(a), h.caller(b));
        let a_name = h_name(&mut h, a);

        let v = h.call(Some(ca), claim(&["/proj/src/./a.rs"], Some("refactor"), false, None)).unwrap();
        assert_eq!(v, json!({ "claimed": ["/proj/src/a.rs"] }));
        assert_eq!(h.pane(a, |p| p.claims.clone()), vec!["/proj/src/a.rs".to_string()]);
        assert!(matches!(&h.activity(a)[0], (ActivityKind::Claim, d) if d == "/proj/src/a.rs — refactor"));

        // Overlapping claim (a parent directory) conflicts, with the holder in `data`.
        let e = h.call(Some(cb), claim(&["/proj/src"], None, false, None)).unwrap_err();
        assert_eq!(e.code, ErrorCode::Conflict);
        assert!(e.message.contains(&a_name) && e.message.contains("(refactor)"), "{}", e.message);
        let data: ClaimConflictsData = serde_json::from_value(e.data.unwrap()).unwrap();
        assert_eq!(data.conflicts[0].holder, a.0);
        assert_eq!(data.conflicts[0].reason.as_deref(), Some("refactor"));

        let v = h.call(Some(cb), Method::Claims).unwrap();
        let claims: ClaimsResult = serde_json::from_value(v).unwrap();
        assert_eq!(claims.claims.len(), 1);
        assert_eq!(claims.claims[0].holder, a.0);
        assert_eq!(claims.claims[0].holder_name, a_name);

        // `--wait`: held until the holder releases, then granted.
        let id = h.send(Some(cb), claim(&["/proj/src"], None, true, None));
        assert!(h.response(id).is_none());
        // Releasing nothing (someone else's path) changes nothing.
        let v = h.call(Some(cb), Method::Release { paths: vec!["/proj/src/a.rs".into()] }).unwrap();
        assert_eq!(v, json!({ "released": [] }));
        assert!(h.response(id).is_none());
        let v = h.call(Some(ca), Method::Release { paths: vec!["/proj/src/a.rs".into()] }).unwrap();
        assert_eq!(v, json!({ "released": ["/proj/src/a.rs"] }));
        assert_eq!(h.response(id).unwrap().unwrap(), json!({ "claimed": ["/proj/src"] }));
        assert!(h.pane(a, |p| p.claims.is_empty()));
        assert_eq!(h.waiter_count(), 0);

        // An empty release list releases everything the caller holds.
        h.call(Some(cb), claim(&["/proj/b.rs"], None, false, None)).unwrap();
        let v = h.call(Some(cb), Method::Release { paths: vec![] }).unwrap();
        let mut rel: Vec<String> = serde_json::from_value(v["released"].clone()).unwrap();
        rel.sort();
        assert_eq!(rel, vec!["/proj/b.rs".to_string(), "/proj/src".to_string()]);
        assert!(matches!(&h.activity(b)[0], (ActivityKind::Release, _)));
        let claims: ClaimsResult = serde_json::from_value(h.call(Some(cb), Method::Claims).unwrap()).unwrap();
        assert!(claims.claims.is_empty());
    }

    #[gpui::test]
    fn claim_wait_times_out(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let a = h.first_pane();
        let b = h.spawn(a, Placement::Tab, None);
        let (ca, cb) = (h.caller(a), h.caller(b));
        h.call(Some(ca), claim(&["/proj/x"], None, false, None)).unwrap();
        let id = h.send(Some(cb), claim(&["/proj/x"], None, true, Some(60.0)));
        assert!(h.response(id).is_none());
        h.expire_waiters_now();
        h.tick();
        assert_eq!(code(&h.response(id).unwrap()), Some(ErrorCode::Timeout));
        assert_eq!(h.waiter_count(), 0);
    }

    #[gpui::test]
    fn tick_expires_stale_claims(cx: &mut TestAppContext) {
        let mut h = setup(cx, |c| c.agents.claim_ttl = 0);
        let a = h.first_pane();
        let ca = h.caller(a);
        h.call(Some(ca), claim(&["/proj/x"], None, false, None)).unwrap();
        assert_eq!(h.pane(a, |p| p.claims.len()), 1);
        h.tick();
        assert!(h.pane(a, |p| p.claims.is_empty()));
        let claims: ClaimsResult = serde_json::from_value(h.call(Some(ca), Method::Claims).unwrap()).unwrap();
        assert!(claims.claims.is_empty());
    }

    #[gpui::test]
    fn spawn_tab_keeps_the_active_tab(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let a = h.first_pane();
        let ca = h.caller(a);
        let first = h.spawn(a, Placement::Tab, None);
        let v = h
            .call(
                Some(ca),
                Method::Spawn {
                    placement: Placement::Tab,
                    name: Some("worker".into()),
                    cwd: None,
                    command: vec!["echo".into(), "hi there".into()],
                },
            )
            .unwrap();
        let second = PaneId(v["pane_id"].as_u64().unwrap());
        h.root.read_with(h.cx, |r, cx| {
            let w = &r.ws.list[0];
            // Inserted right after the caller's tab, and focus stays with the user.
            let order: Vec<PaneId> = w.tabs.iter().map(|t| t.focused).collect();
            assert_eq!(order, vec![a, second, first]);
            assert_eq!(w.active_tab, 0);
            assert_eq!(r.pane_name(second, cx), "worker");
            assert!(r.panes.contains_key(&second));
        });
        let act = h.activity(a);
        assert_eq!(act[0], (ActivityKind::Spawn, "worker echo 'hi there'".into()));
    }

    #[gpui::test]
    fn spawn_splits_the_callers_pane(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let a = h.first_pane();
        let ca = h.caller(a);
        for placement in [Placement::SplitRight, Placement::SplitBelow] {
            let r = h.call(Some(ca), Method::Spawn { placement, name: None, cwd: None, command: vec![] });
            match r {
                Ok(v) => {
                    let p = PaneId(v["pane_id"].as_u64().unwrap());
                    h.root.read_with(h.cx, |r, _| {
                        let t = &r.ws.list[0].tabs[0];
                        assert!(t.panes().contains(&p), "{placement:?}");
                        assert_eq!(t.focused, a, "the split doesn't take focus");
                    });
                }
                Err(e) => assert_eq!(e.code, ErrorCode::Conflict, "{placement:?}: {}", e.message),
            }
        }
        // Keep splitting until there's no room: refused with `conflict`, nothing added.
        let mut refused = false;
        for _ in 0..64 {
            let r = h.call(
                Some(ca),
                Method::Spawn { placement: Placement::SplitRight, name: None, cwd: None, command: vec![] },
            );
            if let Err(e) = r {
                assert_eq!(e.code, ErrorCode::Conflict);
                refused = true;
                break;
            }
        }
        assert!(refused, "splits never ran out of room");
    }

    #[gpui::test]
    fn notify_answers_ok(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let a = h.first_pane();
        let b = h.spawn(a, Placement::Tab, None);
        // Looked at (no attention) and in a background tab (attention; sound and
        // notifications are off in the test config).
        for p in [a, b] {
            let c = h.caller(p);
            let v =
                h.call(Some(c), Method::Notify { title: "done".into(), body: Some("ok".into()) }).unwrap();
            assert_eq!(v, json!({}));
        }
    }

    #[gpui::test]
    fn deliver_type_pastes_messages_into_idle_agents(cx: &mut TestAppContext) {
        let mut h = setup(cx, |c| c.agents.deliver = nox_config::Deliver::Type);
        let a = h.first_pane();
        let b = h.agent(a, "bee", AgentStateWire::Idle);
        let c = h.agent(a, "cee", AgentStateWire::Working);
        let ca = h.caller(a);
        h.call(Some(ca), msg("all", "ping")).unwrap();
        // The idle agent got it typed in (and marked read); the busy one keeps it queued.
        let unread = |h: &mut Harness<'_>, p: PaneId| {
            h.root.read_with(h.cx, |r, _| r.coop[&r.pane_workspace(p).unwrap()].bus.unread_count(p.0))
        };
        assert_eq!(unread(&mut h, b), 0);
        assert_eq!(h.pane(b, |p| p.from_banner.as_ref().map(|(n, _)| n.clone())), Some(h_name(&mut h, a)));
        assert_eq!(unread(&mut h, c), 1);
        assert!(h.pane(c, |p| p.from_banner.is_none()));
        // The pane headers' unread badges follow.
        assert_eq!(h.pane(b, |p| p.unread), 0);
        assert_eq!(h.pane(c, |p| p.unread), 1);
        // When the busy one turns idle, it gets the message and its badge clears.
        h.status(c, AgentStateWire::Idle);
        assert_eq!(unread(&mut h, c), 0);
        assert_eq!(h.pane(c, |p| p.unread), 0);
        assert!(h.pane(c, |p| p.from_banner.is_some()));
    }

    #[gpui::test]
    fn bridge_refuses_when_the_app_is_gone(cx: &mut TestAppContext) {
        let mut h = setup(cx, |_| {});
        let (tx, rx) = async_channel::unbounded::<Request>();
        drop(rx);
        let bridge = Bridge { tx };
        // Borrow a responder from the capture server.
        let id = h.next_id;
        h.next_id += 1;
        let req = WireRequest { v: PROTOCOL_VERSION, id, token: None, method: Method::Ls { all: false } };
        h.writer.write_all(format!("{}\n", serde_json::to_string(&req).unwrap()).as_bytes()).unwrap();
        let (caller, method, reply) = h.rx.recv_timeout(Duration::from_secs(10)).unwrap();
        bridge.handle(caller, method, reply);
        let e = h.response(id).unwrap().unwrap_err();
        assert_eq!(e.code, ErrorCode::Internal);
    }

    #[gpui::test]
    fn start_ipc_serves_the_control_socket(cx: &mut TestAppContext) {
        // The only test that binds the default socket (`$NOX_HOME/nox.sock`, per process).
        let mut config = base_config();
        config.agents.cooperation = true;
        cx.update(|cx| crate::test_support::init_app(cx, config));
        let (root, cx) = cx.add_window_view(|window, cx| Root::new(Initial::Fresh, window, cx));
        cx.run_until_parked();
        let (path, token) = root.read_with(cx, |r, _| {
            let ipc = r.ipc.as_ref().expect("cooperation on: socket started");
            let p = r.ws.list[0].tabs[0].focused;
            let caller =
                Caller { pane_id: p.0, tab_id: r.ws.list[0].tabs[0].id.0, workspace_id: r.ws.list[0].id.0 };
            (ipc.handle.socket_path().to_path_buf(), ipc.handle.tokens().issue(caller))
        });
        let client = std::thread::spawn(move || {
            let mut c = nox_ipc::Client::connect(&path, Some(token)).unwrap();
            c.call(Method::Name { name: "via-socket".into() }, Some(Duration::from_secs(10)))
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !client.is_finished() && Instant::now() < deadline {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(client.join().unwrap().unwrap(), json!({ "name": "via-socket" }));
        // The once-a-second tick runs on the (fake) clock.
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        root.read_with(cx, |r, cx| assert_eq!(r.pane_name(r.ws.list[0].tabs[0].focused, cx), "via-socket"));
    }
}
