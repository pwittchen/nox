//! Session save / restore (SPEC §6.6): every workspace with its tabs, splits, names, colors,
//! working directories, and agent sessions resumed through each agent's own resume command.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use gpui::{App, Context, Window};
use nox_agents::{AgentDefLite, ResumeAction, ResumeCandidate};
use nox_layout::{AgentSnapshot, PaneId, PaneSnapshot, Restored, SessionFile, Workspaces};

use crate::pane::{AgentLaunch, PaneSpawn, Startup};
use crate::root::{Coop, Root, default_cwd};
use crate::settings_state::Settings;

#[must_use]
pub fn session_path() -> PathBuf {
    nox_config::nox_dir().join("session.toml")
}

/// What `main` hands to the root view after reading the session file.
pub struct RestorePlan {
    pub restored: Restored,
}

#[must_use]
pub fn load_plan() -> Option<RestorePlan> {
    let path = session_path();
    if !path.exists() {
        return None;
    }
    match nox_layout::load_session(&path) {
        Ok(file) => {
            let restored = Workspaces::restore(file, |p: &Path| p.is_dir())?;
            Some(RestorePlan { restored })
        }
        Err(e) => {
            log::warn!("session file {}: {e}", path.display());
            None
        }
    }
}

fn agent_defs(cfg: &nox_config::Config) -> BTreeMap<String, AgentDefLite> {
    cfg.agents
        .defs
        .iter()
        .map(|(k, d)| {
            (
                k.clone(),
                AgentDefLite {
                    command: d.command.clone(),
                    resume: d.resume.clone(),
                    resume_last: d.resume_last.clone(),
                },
            )
        })
        .collect()
}

/// Resume commands for saved agent panes.
#[must_use]
pub fn plan_resume(
    cfg: &nox_config::Config,
    agents: &[(PaneId, AgentSnapshot)],
) -> HashMap<PaneId, ResumeAction> {
    let candidates: Vec<ResumeCandidate> = agents
        .iter()
        .filter(|(_, a)| a.resume_on_restore)
        .map(|(p, a)| ResumeCandidate {
            pane: p.0,
            kind: a.kind.clone(),
            cwd: a.start_cwd.clone(),
            command: a.command.clone(),
            session_id: a.session_id.clone(),
        })
        .collect();
    nox_agents::plan_resume(&candidates, &agent_defs(cfg))
        .into_iter()
        .map(|p| (PaneId(p.pane), p.action))
        .collect()
}

fn startup_for(action: &ResumeAction) -> Option<Startup> {
    match action {
        ResumeAction::Run(cmd) => Some(Startup::Run(cmd.clone())),
        ResumeAction::TypeOnly(cmd) => Some(Startup::Type(cmd.clone())),
        ResumeAction::SessionUnknown => None,
    }
}

impl Root {
    pub fn apply_restore(&mut self, plan: RestorePlan, window: &mut Window, cx: &mut Context<Self>) {
        let cfg = Settings::global(cx).config.clone();
        let Restored { workspaces, panes, .. } = plan.restored;
        self.ws = workspaces;
        for w in &self.ws.list {
            self.coop.insert(w.id, Coop::new(w.id));
        }
        let mut missing_dirs = 0usize;
        let agents: Vec<(PaneId, AgentSnapshot)> =
            panes.iter().filter_map(|p| p.snapshot.agent.clone().map(|a| (p.new, a))).collect();
        let restore_mode = cfg.agents.restore;
        let actions = if restore_mode == nox_config::AgentRestore::Resume {
            plan_resume(&cfg, &agents)
        } else {
            HashMap::new()
        };
        let mut ask: Vec<(PaneId, AgentSnapshot)> = Vec::new();
        let mut unknown: Vec<(PaneId, AgentSnapshot)> = Vec::new();

        for rp in &panes {
            let ws_idx = self.ws.index_of(rp.workspace).unwrap_or(0);
            let root = self.ws.list[ws_idx].root.clone();
            let cwd = match &rp.snapshot.cwd {
                Some(d) if d.is_dir() => d.clone(),
                Some(_) => {
                    missing_dirs += 1;
                    root.clone().unwrap_or_else(|| default_cwd(cx))
                }
                None => root.clone().unwrap_or_else(|| default_cwd(cx)),
            };
            let mut spawn =
                PaneSpawn { cwd: Some(cwd), name: rp.snapshot.custom_name.clone(), ..Default::default() };
            let mut has_agent = false;
            if let Some(a) = &rp.snapshot.agent {
                self.saved_agents.insert(rp.new, a.clone());
                match restore_mode {
                    nox_config::AgentRestore::Resume => {
                        if let Some(action) = actions.get(&rp.new) {
                            spawn.startup = startup_for(action);
                            if matches!(action, ResumeAction::SessionUnknown) {
                                unknown.push((rp.new, a.clone()));
                            }
                            if let ResumeAction::Run(cmd) = action {
                                has_agent = true;
                                spawn.cwd = Some(if a.start_cwd.is_dir() {
                                    a.start_cwd.clone()
                                } else {
                                    spawn.cwd.clone().unwrap()
                                });
                                spawn.agent = Some(AgentLaunch {
                                    kind: a.kind.clone(),
                                    command: cmd.clone(),
                                    start_cwd: a.start_cwd.clone(),
                                    resumed: true,
                                });
                            }
                        }
                    }
                    nox_config::AgentRestore::Ask => ask.push((rp.new, a.clone())),
                    nox_config::AgentRestore::Off => {}
                }
            }
            let _ = has_agent;
            self.pending.insert(rp.new, spawn);
        }

        // Start the active workspace, workspaces with agents to resume, or all of them.
        let lazy = cfg.workspaces.lazy_restore;
        for i in 0..self.ws.len() {
            let has_agents = self.ws.list[i]
                .tabs
                .iter()
                .flat_map(nox_layout::Tab::panes)
                .any(|p| self.pending.get(&p).is_some_and(|s| s.agent.is_some()));
            if i == self.ws.active || has_agents || !lazy {
                self.ensure_started(i, window, cx);
            }
        }
        self.sync_maximized(cx);
        if missing_dirs > 0 {
            crate::toast::show(
                cx,
                format!(
                    "{missing_dirs} saved directory(ies) no longer exist; those panes start in the workspace root."
                ),
            );
        }
        let missing_roots: Vec<String> =
            self.ws.list.iter().filter(|w| w.root_missing).map(|w| w.name.clone()).collect();
        if !missing_roots.is_empty() {
            crate::toast::show(
                cx,
                format!(
                    "Workspace root missing: {}. Set a new folder from the workspace menu.",
                    missing_roots.join(", ")
                ),
            );
        }
        if !ask.is_empty() {
            self.show_resume_sheet(&ask, window, cx);
        } else if !unknown.is_empty() {
            self.show_resume_sheet(&unknown, window, cx);
        }
    }

    /// The "Resume agents" sheet (SPEC §7.12, `agents.restore = "ask"`).
    pub fn show_resume_sheet(
        &mut self,
        agents: &[(PaneId, AgentSnapshot)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cfg = Settings::global(cx).config.clone();
        let mut b = crate::dialog::DialogBuilder::new("Resume agents?")
            .message("These agents were running when nox closed. Resume their conversations?");
        let mut by_ws: Vec<(String, PaneId, AgentSnapshot)> = agents
            .iter()
            .map(|(p, a)| {
                let ws = self
                    .pane_workspace(*p)
                    .and_then(|id| self.ws.index_of(id))
                    .map(|i| self.ws.list[i].name.clone())
                    .unwrap_or_default();
                (ws, *p, a.clone())
            })
            .collect();
        by_ws.sort_by(|a, b| a.0.cmp(&b.0));
        let actions = plan_resume(
            &cfg,
            &agents
                .iter()
                .map(|(p, a)| (*p, AgentSnapshot { resume_on_restore: true, ..a.clone() }))
                .collect::<Vec<_>>(),
        );
        for (ws, p, a) in &by_ws {
            let unknown = matches!(actions.get(p), Some(ResumeAction::SessionUnknown));
            let detail = format!(
                "{ws} · {}{}",
                nox_terminal::process::tildify(&a.start_cwd),
                if unknown { " · session unknown" } else { "" }
            );
            b = b.check(a.name.clone(), Some(detail), !unknown);
        }
        let this = cx.entity().downgrade();
        let panes: Vec<PaneId> = by_ws.iter().map(|(_, p, _)| *p).collect();
        b.cancel("Start Shells")
            .button("Resume", move |v, window, cx| {
                let chosen: Vec<PaneId> =
                    panes.iter().zip(v.checks.iter()).filter(|(_, on)| **on).map(|(p, _)| *p).collect();
                let _ = this.update(cx, |this, cx| this.resume_agents(&chosen, true, window, cx));
            })
            .default_last()
            .show(window, cx);
    }

    /// Type the resume command into each chosen pane's shell. With `fresh_if_unknown`, panes
    /// without a known session start a new session of the agent.
    pub fn resume_agents(
        &mut self,
        panes: &[PaneId],
        fresh_if_unknown: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cfg = Settings::global(cx).config.clone();
        let agents: Vec<(PaneId, AgentSnapshot)> = panes
            .iter()
            .filter_map(|p| {
                self.saved_agents.get(p).map(|a| (*p, AgentSnapshot { resume_on_restore: true, ..a.clone() }))
            })
            .collect();
        let actions = plan_resume(&cfg, &agents);
        for (p, a) in agents {
            let cmd = match actions.get(&p) {
                Some(ResumeAction::Run(c)) => Some((c.clone(), true)),
                Some(ResumeAction::TypeOnly(c)) => Some((c.clone(), false)),
                Some(ResumeAction::SessionUnknown) if fresh_if_unknown => {
                    cfg.agents.defs.get(&a.kind).map(|d| (d.command.clone(), true))
                }
                _ => None,
            };
            let Some((cmd, run)) = cmd else { continue };
            if let Some(w) = self.ws.find_pane(p).map(|(w, _)| w) {
                self.ensure_started(w, window, cx);
            }
            if let Some(e) = self.panes.get(&p) {
                e.update(cx, |e, cx| {
                    let text = if run { format!("{cmd}\r") } else { cmd.clone() };
                    e.write(text.as_bytes());
                    cx.notify();
                });
            }
        }
    }

    /// Resume All Agents (palette): every saved agent pane that isn't running an agent now.
    pub fn resume_all_agents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panes: Vec<PaneId> = self
            .saved_agents
            .keys()
            .copied()
            .filter(|p| !self.panes.get(p).is_some_and(|e| e.read(cx).is_agent()))
            .collect();
        if panes.is_empty() {
            crate::toast::show(cx, "No saved agent sessions to resume.");
            return;
        }
        self.resume_agents(&panes, false, window, cx);
    }
}

/// Snapshot one pane for the session file.
fn pane_snapshot(root: &Root, p: PaneId, cx: &App) -> PaneSnapshot {
    if let Some(e) = root.panes.get(&p) {
        let e = e.read(cx);
        let agent = match &e.agent {
            Some(a) if a.tracker.state() != nox_agents::AgentState::Exited => Some(AgentSnapshot {
                kind: a.kind.clone(),
                name: root.pane_name(p, cx),
                start_cwd: a
                    .launch
                    .as_ref()
                    .map(|l| l.start_cwd.clone())
                    .or(e.cwd.clone())
                    .unwrap_or_default(),
                command: a
                    .launch
                    .as_ref()
                    .filter(|l| !l.resumed)
                    .map_or_else(|| a.cmdline.clone(), |l| l.command.clone()),
                session_id: a
                    .tracker
                    .session_id()
                    .map(str::to_string)
                    .or_else(|| root.saved_agents.get(&p).and_then(|s| s.session_id.clone())),
                resume_on_restore: e.resume_on_restore,
            }),
            // An agent that exited is not resumed.
            Some(_) => None,
            // Not started this run but saved from the last one (lazy / not resumed).
            None => root.saved_agents.get(&p).cloned(),
        };
        return PaneSnapshot { cwd: e.cwd.clone(), custom_name: e.custom_name.clone(), agent };
    }
    if let Some(s) = root.pending.get(&p) {
        return PaneSnapshot {
            cwd: s.cwd.clone(),
            custom_name: s.name.clone(),
            agent: root.saved_agents.get(&p).cloned(),
        };
    }
    PaneSnapshot::default()
}

pub fn snapshot(root: &Root, cx: &App) -> SessionFile {
    root.ws.snapshot(|p| pane_snapshot(root, p, cx))
}

pub fn save(root: &mut Root, cx: &mut Context<Root>) {
    let file = snapshot(root, cx);
    if let Err(e) = nox_layout::save_session_atomic(&session_path(), &file) {
        log::error!("saving session: {e}");
    }
}
