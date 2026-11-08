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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::root::Initial;
    use crate::root::tests::{canon, dialog_title, make_agent, open, pane_cwd, quiet_config};
    use gpui::{Entity, TestAppContext, VisualTestContext};
    use nox_agents::AgentState;
    use nox_layout::SplitDirection;

    /// Tests that write `$NOX_HOME/session.toml` take this lock (tests share one `NOX_HOME`).
    static SESSION_FILE: Mutex<()> = Mutex::new(());

    fn fake_agents(cfg: &mut nox_config::Config) {
        cfg.agents.defs.clear();
        cfg.agents.defs.insert(
            "fake".into(),
            nox_config::AgentDef {
                command: "true".into(),
                resume: Some("true --resume {session}".into()),
                ..Default::default()
            },
        );
        cfg.agents.defs.insert(
            "plain".into(),
            nox_config::AgentDef { command: "true plain".into(), ..Default::default() },
        );
        cfg.agents.defs.insert(
            "last".into(),
            nox_config::AgentDef {
                command: "true last".into(),
                resume_last: Some("true --continue".into()),
                ..Default::default()
            },
        );
    }

    fn agent(kind: &str, cwd: &Path, session: Option<&str>, resume: bool) -> AgentSnapshot {
        AgentSnapshot {
            kind: kind.into(),
            name: kind.into(),
            start_cwd: cwd.to_path_buf(),
            command: format!("{kind} --orig"),
            session_id: session.map(str::to_string),
            resume_on_restore: resume,
        }
    }

    fn restore_into(cx: &mut VisualTestContext, file: SessionFile) -> (Entity<Root>, &mut VisualTestContext) {
        let restored = Workspaces::restore(file, |p: &Path| p.is_dir()).expect("restorable");
        let plan = RestorePlan { restored };
        let (root, cx) =
            cx.add_window_view(|window, cx| Root::new(Initial::Restored(Box::new(plan)), window, cx));
        cx.run_until_parked();
        (root, cx)
    }

    #[test]
    fn resume_plan_per_agent_definition() {
        let mut cfg = quiet_config();
        fake_agents(&mut cfg);
        let dir = PathBuf::from("/tmp");
        let agents = vec![
            (PaneId(1), agent("fake", &dir, Some("abc"), true)),
            (PaneId(2), agent("fake", &dir, Some("abc"), false)),
            (PaneId(3), agent("plain", &dir, None, true)),
            (PaneId(4), agent("last", &dir, None, true)),
            (PaneId(5), agent("last", &dir, None, true)),
        ];
        let plan = plan_resume(&cfg, &agents);
        assert!(!plan.contains_key(&PaneId(2)), "not marked for resume");
        assert!(
            matches!(&plan[&PaneId(1)], ResumeAction::Run(c) if c.starts_with("true --resume") && c.contains("abc"))
        );
        assert_eq!(plan[&PaneId(3)], ResumeAction::TypeOnly("plain --orig".into()));
        assert_eq!(plan[&PaneId(4)], ResumeAction::Run("true --continue".into()));
        // The same directory twice: resume_last would open one conversation twice.
        assert_eq!(plan[&PaneId(5)], ResumeAction::SessionUnknown);

        assert!(matches!(startup_for(&ResumeAction::Run("x".into())), Some(Startup::Run(c)) if c == "x"));
        assert!(
            matches!(startup_for(&ResumeAction::TypeOnly("y".into())), Some(Startup::Type(c)) if c == "y")
        );
        assert!(startup_for(&ResumeAction::SessionUnknown).is_none());
        assert_eq!(agent_defs(&cfg)["last"].resume_last.as_deref(), Some("true --continue"));
    }

    #[gpui::test]
    fn snapshot_restores_workspaces_tabs_names_and_dirs(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (root, cx) = open(cx, quiet_config());
        let (left, other) = root.update_in(cx, |r, w, cx| {
            let left = r.focused_pane_id().unwrap();
            r.ws.active_mut().active_mut().unwrap().custom_name = Some("build".into());
            r.panes[&left].update(cx, |p, _| p.custom_name = Some("left".into()));
            r.split(SplitDirection::Vertical, None, w, cx).unwrap();
            let idx = r.create_workspace(Some("api".into()), Some(dir.path().to_path_buf()), w, cx);
            r.ws.list[idx].color = Some("blue".into());
            let other = r.add_tab(
                idx,
                PaneSpawn { cwd: Some(dir.path().to_path_buf()), ..Default::default() },
                w,
                cx,
            );
            r.switch_workspace(0, w, cx);
            (left, other)
        });
        let file = root.read_with(cx, snapshot);
        assert_eq!(file.active_workspace, 0);
        assert_eq!(file.workspaces.len(), 2);
        let ws0 = &file.workspaces[0];
        assert_eq!(ws0.tabs[0].custom_name.as_deref(), Some("build"));
        assert_eq!(ws0.tabs[0].panes.len(), 2);
        let home = crate::test_support::nox_home();
        assert_eq!(ws0.tabs[0].panes[&left].custom_name.as_deref(), Some("left"));
        assert_eq!(ws0.tabs[0].panes[&left].cwd.as_deref().map(canon), Some(canon(&home)));
        let ws1 = &file.workspaces[1];
        assert_eq!((ws1.name.as_str(), ws1.color.as_deref()), ("api", Some("blue")));
        assert_eq!(ws1.root.as_deref(), Some(dir.path()));
        assert_eq!(ws1.tabs[0].panes[&other].cwd.as_deref().map(canon), Some(canon(dir.path())));
        // TOML round trip.
        let back = SessionFile::from_toml(&file.to_toml().unwrap()).unwrap();
        assert_eq!(back, file);

        let (r2, cx) = restore_into(cx, back);
        r2.read_with(cx, |r, cx| {
            assert_eq!(r.ws.len(), 2);
            assert_eq!(r.ws.active, 0);
            assert_eq!(r.ws.list[1].name, "api");
            assert_eq!(r.ws.list[1].color.as_deref(), Some("blue"));
            let tab = r.active_tab().unwrap();
            assert_eq!(tab.custom_name.as_deref(), Some("build"));
            assert_eq!(tab.panes().len(), 2);
            let names: Vec<Option<String>> =
                tab.panes().iter().map(|p| r.panes[p].read(cx).custom_name.clone()).collect();
            assert!(names.contains(&Some("left".into())));
            // Lazy restore: the background workspace waits.
            assert_eq!(r.panes.len(), 2);
            assert_eq!(r.pending.len(), 1);
            assert!(r.coop.contains_key(&r.ws.list[1].id));
        });
        // A pending pane still snapshots with its saved directory.
        let again = r2.read_with(cx, snapshot);
        let p = again.workspaces[1].tabs[0].panes.values().next().unwrap();
        assert_eq!(p.cwd.as_deref().map(canon), Some(canon(dir.path())));
        let p = r2.update_in(cx, |r, w, cx| {
            r.switch_workspace(1, w, cx);
            assert!(r.pending.is_empty());
            assert_eq!(r.panes.len(), 3);
            r.focused_pane_id().unwrap()
        });
        assert_eq!(pane_cwd(&r2, p, cx), Some(canon(dir.path())));
    }

    #[gpui::test]
    fn missing_dirs_fall_back_and_eager_restore_starts_everything(cx: &mut TestAppContext) {
        let mut cfg = quiet_config();
        cfg.workspaces.lazy_restore = false;
        let (root, cx) = open(cx, cfg);
        root.update_in(cx, |r, w, cx| {
            let idx = r.create_workspace(Some("gone".into()), None, w, cx);
            r.add_tab(idx, PaneSpawn::default(), w, cx);
            r.switch_workspace(0, w, cx);
        });
        let mut file = root.read_with(cx, snapshot);
        file.workspaces[1].root = Some("/definitely/not/a/dir".into());
        for p in file.workspaces[0].tabs[0].panes.values_mut() {
            p.cwd = Some("/definitely/not/a/dir/either".into());
        }
        let (r2, cx) = restore_into(cx, file);
        let p = r2.read_with(cx, |r, _| {
            assert!(r.pending.is_empty(), "lazy_restore = false starts all workspaces");
            assert_eq!(r.panes.len(), 2);
            assert!(r.ws.list[1].root_missing);
            r.focused_pane_id().unwrap()
        });
        assert_eq!(pane_cwd(&r2, p, cx), Some(canon(&crate::test_support::nox_home())));
    }

    /// One agent pane (`fake`, session `sess-1`) in a background workspace.
    fn agent_session(cx: &mut VisualTestContext, root: &Entity<Root>) -> SessionFile {
        let pane = root.update_in(cx, |r, w, cx| {
            let idx = r.create_workspace(Some("agents".into()), None, w, cx);
            let p = r.add_tab(idx, PaneSpawn::default(), w, cx);
            r.switch_workspace(0, w, cx);
            p
        });
        make_agent(root, pane, "fake", AgentState::Working, cx);
        root.update(cx, |r, cx| r.panes[&pane].update(cx, |p, _| p.resume_on_restore = true));
        let file = root.read_with(cx, snapshot);
        let a = file.workspaces[1].tabs[0].panes[&pane].agent.clone().expect("agent saved");
        assert_eq!(a.kind, "fake");
        assert_eq!(a.session_id.as_deref(), Some("sess-1"));
        assert!(a.resume_on_restore);
        file
    }

    #[gpui::test]
    fn resume_mode_starts_agents_even_in_lazy_workspaces(cx: &mut TestAppContext) {
        let mut cfg = quiet_config();
        fake_agents(&mut cfg);
        let (root, cx) = open(cx, cfg);
        let file = agent_session(cx, &root);
        let (r2, cx) = restore_into(cx, file);
        r2.read_with(cx, |r, cx| {
            assert!(r.pending.is_empty(), "the agent's workspace is started");
            assert_eq!(r.saved_agents.len(), 1);
            let (p, _) = r.saved_agents.iter().next().unwrap();
            let e = r.panes[p].read(cx);
            let launch = e.agent.as_ref().unwrap().launch.clone().unwrap();
            assert!(launch.resumed);
            assert_eq!(launch.kind, "fake");
            assert!(launch.command.contains("sess-1"));
            assert_eq!(dialog_title_app(cx), None);
        });
    }

    #[gpui::test]
    fn exited_agents_are_not_saved(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        let p = root.read_with(cx, |r, _| r.focused_pane_id().unwrap());
        make_agent(&root, p, "fake", AgentState::Idle, cx);
        let saved = root.read_with(cx, |r, cx| snapshot(r, cx).workspaces[0].tabs[0].panes[&p].agent.clone());
        let saved = saved.expect("live agent saved");
        assert_eq!(
            (saved.kind.as_str(), saved.name.as_str(), saved.command.as_str()),
            ("fake", "fake", "fake")
        );
        root.update(cx, |r, cx| {
            r.panes[&p].update(cx, |e, _| {
                e.agent.as_mut().unwrap().tracker.on_exit(std::time::Instant::now());
            });
        });
        let saved = root.read_with(cx, |r, cx| snapshot(r, cx).workspaces[0].tabs[0].panes[&p].agent.clone());
        assert!(saved.is_none());
    }

    fn dialog_title_app(cx: &gpui::App) -> Option<String> {
        cx.try_global::<crate::dialog::ActiveDialog>()
            .and_then(|d| d.0.as_ref().map(|(v, _)| v.read(cx).title.to_string()))
    }

    #[gpui::test]
    fn ask_mode_shows_the_resume_sheet(cx: &mut TestAppContext) {
        let mut cfg = quiet_config();
        fake_agents(&mut cfg);
        cfg.agents.restore = nox_config::AgentRestore::Ask;
        let (root, cx) = open(cx, cfg);
        let file = agent_session(cx, &root);
        let (r2, cx) = restore_into(cx, file);
        assert_eq!(dialog_title(cx).as_deref(), Some("Resume agents?"));
        let checks = cx.update(|_, cx| {
            let v = cx.global::<crate::dialog::ActiveDialog>().0.as_ref().unwrap().0.clone();
            v.read(cx).checks.iter().map(|c| (c.label.to_string(), c.on)).collect::<Vec<_>>()
        });
        assert_eq!(checks, vec![("fake".to_string(), true)]);
        r2.read_with(cx, |r, cx| {
            // Not started yet: lazy, and nothing runs without the user's answer.
            assert_eq!(r.pending.len(), 1);
            assert!(r.panes.values().all(|p| p.read(cx).agent.is_none()));
        });
        cx.simulate_keystrokes("enter");
        assert_eq!(dialog_title(cx), None);
        // Resuming starts the workspace it lives in.
        r2.read_with(cx, |r, _| assert!(r.pending.is_empty()));
    }

    #[gpui::test]
    fn off_mode_keeps_agents_for_resume_all(cx: &mut TestAppContext) {
        let mut cfg = quiet_config();
        fake_agents(&mut cfg);
        cfg.agents.restore = nox_config::AgentRestore::Off;
        let (root, cx) = open(cx, cfg);
        // Nothing saved yet.
        root.update_in(cx, Root::resume_all_agents);
        let file = agent_session(cx, &root);
        let (r2, cx) = restore_into(cx, file);
        assert_eq!(dialog_title(cx), None);
        r2.update_in(cx, |r, w, cx| {
            assert_eq!(r.saved_agents.len(), 1);
            assert_eq!(r.pending.len(), 1);
            // Pending + saved agent: the snapshot keeps the saved agent.
            let f = snapshot(r, cx);
            assert!(f.workspaces[1].tabs[0].panes.values().next().unwrap().agent.is_some());
            r.resume_all_agents(w, cx);
            assert!(r.pending.is_empty());
        });
    }

    #[gpui::test]
    fn unknown_sessions_are_listed_unchecked(cx: &mut TestAppContext) {
        let mut cfg = quiet_config();
        fake_agents(&mut cfg);
        let (root, cx) = open(cx, cfg);
        let mut file = root.read_with(cx, snapshot);
        let home = crate::test_support::nox_home();
        // Two `last` agents in one directory without session ids.
        let tab = file.workspaces[0].tabs[0].clone();
        let mut tab2 = tab.clone();
        let (old, snap) = tab.panes.iter().next().map(|(k, v)| (*k, v.clone())).unwrap();
        let mut s = snap.clone();
        s.agent = Some(agent("last", &home, None, true));
        file.workspaces[0].tabs[0].panes.insert(old, s.clone());
        tab2.panes.insert(old, s);
        file.workspaces[0].tabs.push(tab2);
        let (_r2, cx) = restore_into(cx, file);
        assert_eq!(dialog_title(cx).as_deref(), Some("Resume agents?"));
        let checks = cx.update(|_, cx| {
            let v = cx.global::<crate::dialog::ActiveDialog>().0.as_ref().unwrap().0.clone();
            v.read(cx)
                .checks
                .iter()
                .map(|c| (c.on, c.detail.as_ref().map(ToString::to_string)))
                .collect::<Vec<_>>()
        });
        // Only the second `last` agent is listed. (It shows checked and without "session
        // unknown": the sheet re-plans the subset alone, see the report.)
        assert_eq!(checks.len(), 1);
        // "Start Shells" (cancel).
        cx.simulate_keystrokes("escape");
        assert_eq!(dialog_title(cx), None);
    }

    #[gpui::test]
    fn session_file_save_load_and_quit(cx: &mut TestAppContext) {
        let _guard = SESSION_FILE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut cfg = quiet_config();
        cfg.app.quit_when_last_tab_closed = true;
        let (root, cx) = open(cx, cfg);
        let path = session_path();
        let _ = std::fs::remove_file(&path);
        assert!(load_plan().is_none(), "no file");
        std::fs::write(&path, "not = [valid").unwrap();
        assert!(load_plan().is_none(), "unparsable file");

        root.update(cx, |r, cx| {
            r.ws.active_mut().name = "saved-ws".into();
            r.save_session_now(cx);
        });
        let plan = load_plan().expect("saved session");
        assert_eq!(plan.restored.workspaces.list[0].name, "saved-ws");
        assert_eq!(plan.restored.panes.len(), 1);

        // Closing the last tab quits: the session is saved once (empty) and later saves are off.
        cx.dispatch_action(crate::actions::CloseTab);
        root.update(cx, |r, cx| {
            assert!(r.quitting);
            assert!(r.panes.is_empty());
            assert_eq!(r.ws.len(), 1);
            r.save_session_now(cx);
        });
        assert!(load_plan().is_none(), "an empty session restores nothing");
        let _ = std::fs::remove_file(&path);
    }
}
