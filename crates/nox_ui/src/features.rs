//! Workspace and agent features: open folder as workspace (with project files and trust),
//! New Workspace, worktrees, tab moves, Install Agent Hooks, the command line tool
//! (SPEC §4.7, §6.7, §7.12, §7.13).

use std::path::{Path, PathBuf};

use gpui::{App, AppContext as _, ClipboardItem, Context, PathPromptOptions, Window};
use nox_agents::{ProjectFile, ProjectLayout, TrustStore};
use nox_layout::{PaneId, SplitDirection, Tab, TabId, Workspace, WorkspaceId};

use crate::chrome::DraggedTab;
use crate::dialog::DialogBuilder;
use crate::palette::{Palette, PaletteItem, PaletteOptions};
use crate::pane::{AgentLaunch, PaneSpawn, Startup};
use crate::root::{Coop, Root, dir_name};
use crate::settings_state::Settings;

fn trust_path() -> PathBuf {
    nox_config::nox_dir().join("trusted.toml")
}

impl Root {
    /// Create an empty workspace (switched to) and return its index.
    pub fn create_workspace(
        &mut self,
        name: Option<String>,
        root: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let name = match name.filter(|n| !n.trim().is_empty()) {
            Some(n) => self.ws.unique_name(n.trim()),
            None => match &root {
                Some(r) => self.ws.unique_name(&dir_name(r)),
                None => self.ws.default_name(),
            },
        };
        let ws = Workspace::new(WorkspaceId::next(), name, root);
        let id = ws.id;
        let idx = self.ws.push_and_switch(ws);
        self.coop.insert(id, Coop::new(id));
        let _ = window;
        cx.notify();
        idx
    }

    /// Open a folder as a workspace (SPEC §6.7): switch to an existing workspace with that
    /// root unless `force_new`; apply `.nox/workspace.toml` (asking to trust its commands).
    pub fn open_folder(
        &mut self,
        path: PathBuf,
        force_new: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> WorkspaceId {
        let path = path.canonicalize().unwrap_or(path);
        if !force_new && let Some(i) = self.ws.find_by_root(&path) {
            self.switch_workspace(i, window, cx);
            return self.ws.list[i].id;
        }
        let project = match ProjectFile::load(&path) {
            Ok(p) => p,
            Err(e) => {
                crate::toast::show(cx, format!(".nox/workspace.toml: {e}"));
                None
            }
        };
        let idx = self.create_workspace(
            project.as_ref().and_then(|(p, _)| p.name.clone()),
            Some(path.clone()),
            window,
            cx,
        );
        let id = self.ws.list[idx].id;
        let Some((pf, hash)) = project else {
            self.add_tab(idx, PaneSpawn { cwd: Some(path), ..Default::default() }, window, cx);
            self.switch_workspace(idx, window, cx);
            return id;
        };
        if let Some(c) = &pf.color {
            self.ws.list[idx].color = Some(c.clone());
        }
        self.ws.list[idx].env = pf.env.clone();
        let file_path = ProjectFile::path(&path);
        let trusted = TrustStore::load(&trust_path()).is_ok_and(|t| t.is_trusted(&file_path, &hash));
        if !pf.has_commands() || trusted {
            self.open_project_tabs(idx, &path, &pf, true, window, cx);
        } else {
            // Open with plain shells first; run the commands once trusted.
            self.open_project_tabs(idx, &path, &pf, false, window, cx);
            let this = cx.entity().downgrade();
            let commands = pf.commands().join("\n");
            let (pf2, path2, file2) = (pf.clone(), path.clone(), file_path.clone());
            DialogBuilder::new("Trust this project file?")
                .message(format!(
                    "{} wants to run these commands when the workspace opens:",
                    nox_terminal::process::tildify(&file_path)
                ))
                .detail(commands)
                .cancel("Open with Shells")
                .button("Trust and Run", move |_, window, cx| {
                    if let Ok(mut t) = TrustStore::load(&trust_path()) {
                        let _ = t.trust(&file2, &hash);
                    }
                    let _ = this.update(cx, |this, cx| {
                        if let Some(i) = this.ws.index_of(id) {
                            this.run_project_commands(i, &path2, &pf2, window, cx);
                        }
                    });
                })
                .default_last()
                .show(window, cx);
        }
        self.switch_workspace(idx, window, cx);
        id
    }

    fn open_project_tabs(
        &mut self,
        idx: usize,
        root: &Path,
        pf: &ProjectFile,
        run: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if pf.tabs.is_empty() {
            self.add_tab(idx, PaneSpawn { cwd: Some(root.to_path_buf()), ..Default::default() }, window, cx);
            return;
        }
        for t in &pf.tabs {
            let cmds = t.pane_commands();
            let first = cmds.first().copied().flatten().map(str::to_string);
            let spawn = PaneSpawn {
                cwd: Some(root.to_path_buf()),
                startup: if run { first.clone().map(Startup::Run) } else { None },
                ..Default::default()
            };
            let pane = self.add_tab(idx, spawn, window, cx);
            if let Some(title) = &t.title
                && let Some((w, ti)) = self.ws.find_pane(pane)
            {
                self.ws.list[w].tabs[ti].custom_name = Some(title.clone());
            }
            let dir = match t.layout {
                Some(ProjectLayout::Horizontal) => SplitDirection::Horizontal,
                _ => SplitDirection::Vertical,
            };
            for cmd in cmds.iter().skip(1) {
                let Some((w, ti)) = self.ws.find_pane(pane) else { break };
                let new = PaneId::next();
                let tab_id = self.ws.list[w].tabs[ti].id;
                let tab = &mut self.ws.list[w].tabs[ti];
                let focus = tab.focused;
                tab.split_focused(dir, new);
                tab.focus(focus);
                let spawn = PaneSpawn {
                    cwd: Some(root.to_path_buf()),
                    startup: if run { cmd.map(|c| Startup::Run(c.to_string())) } else { None },
                    ..Default::default()
                };
                self.spawn_pane(new, w, tab_id, spawn, window, cx);
            }
        }
        if let Some(w) = self.ws.list.get_mut(idx) {
            w.select_tab(0);
        }
    }

    fn run_project_commands(
        &mut self,
        idx: usize,
        _root: &Path,
        pf: &ProjectFile,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Tabs were opened in order; type each pane's command into its shell.
        let commands: Vec<Option<String>> = pf
            .tabs
            .iter()
            .flat_map(|t| t.pane_commands().into_iter().map(|c| c.map(str::to_string)))
            .collect();
        let panes: Vec<PaneId> = self.ws.list[idx].tabs.iter().flat_map(nox_layout::Tab::panes).collect();
        for (p, cmd) in panes.into_iter().zip(commands) {
            if let (Some(cmd), Some(e)) = (cmd, self.panes.get(&p)) {
                e.update(cx, |e, _| e.write(format!("{cmd}\r").as_bytes()));
            }
        }
    }

    pub fn prompt_open_folder(&mut self, force_new: bool, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open as Workspace".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(p) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.open_folder(p, force_new, window, cx);
                });
            }
        })
        .detach();
    }

    /// New Workspace… sheet (name + root, both optional).
    pub fn new_workspace_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        DialogBuilder::new("New Workspace")
            .message("Both are optional: an empty name takes the folder's name.")
            .field("Name", "", "workspace name")
            .field("Root folder", "", "~/Development/project")
            .cancel("Cancel")
            .button("Choose Folder…", {
                let this = this.clone();
                move |_, window, cx| {
                    let _ = this.update(cx, |this, cx| this.prompt_open_folder(true, window, cx));
                }
            })
            .button("Create", move |v, window, cx| {
                let name = v.fields[0].trim().to_string();
                let root = v.fields[1].trim();
                let root =
                    (!root.is_empty()).then(|| crate::icons::expand_tilde(root)).filter(|p| p.is_dir());
                let _ = this.update(cx, |this, cx| {
                    if let Some(r) = &root
                        && name.is_empty()
                    {
                        this.open_folder(r.clone(), true, window, cx);
                        return;
                    }
                    let idx =
                        this.create_workspace((!name.is_empty()).then_some(name), root.clone(), window, cx);
                    let cwd = root.unwrap_or_else(|| crate::root::default_cwd(cx));
                    this.add_tab(idx, PaneSpawn { cwd: Some(cwd), ..Default::default() }, window, cx);
                    this.switch_workspace(idx, window, cx);
                });
            })
            .default_last()
            .show(window, cx);
    }

    pub fn duplicate_tab(&mut self, ws_idx: usize, tab: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.ws.list[ws_idx]
            .tab(tab)
            .and_then(|t| self.panes.get(&t.focused))
            .and_then(|p| p.read(cx).cwd.clone());
        if let Some(i) = self.ws.list[ws_idx].tab_index(tab) {
            self.ws.list[ws_idx].select_tab(i);
        }
        self.add_tab(ws_idx, PaneSpawn { cwd, ..Default::default() }, window, cx);
    }

    pub fn close_other_tabs(
        &mut self,
        ws_idx: usize,
        keep: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let others: Vec<TabId> =
            self.ws.list[ws_idx].tabs.iter().map(|t| t.id).filter(|t| *t != keep).collect();
        for t in others {
            self.close_tab_now(ws_idx, t, window, cx);
        }
    }

    /// Move a tab to another workspace (SPEC §6.7): panes keep running; agents join the target
    /// bus and their claims are released.
    pub fn move_tab_to_workspace(
        &mut self,
        from: usize,
        tab: TabId,
        to: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if from == to || to >= self.ws.len() {
            return;
        }
        let panes = self.ws.list[from].tab(tab).map(nox_layout::Tab::panes).unwrap_or_default();
        let from_id = self.ws.list[from].id;
        let to_id = self.ws.list[to].id;
        if let Some(c) = self.coop.get_mut(&from_id) {
            for p in &panes {
                c.claims.release_all(p.0);
            }
        }
        let Some(moved) = self.ws.move_tab_between(from, tab, to) else { return };
        if let Some(ipc) = &self.ipc {
            for p in &panes {
                ipc.handle.tokens().update(p.0, tab.0, to_id.0);
            }
        }
        self.ensure_started(to, window, cx);
        if moved.source_empty {
            if Settings::global(cx).config.workspaces.close_when_empty {
                self.close_workspace_now(from, window, cx);
            } else {
                let cwd = self.ws.list[from].root.clone().unwrap_or_else(|| crate::root::default_cwd(cx));
                self.add_tab(from, PaneSpawn { cwd: Some(cwd), ..Default::default() }, window, cx);
            }
        }
        self.coop_changed(cx);
        self.session_changed(cx);
        self.focus_active(window, cx);
        cx.notify();
    }

    /// A tab was dropped on another tab: reorder, or move between workspaces.
    pub fn drop_tab(
        &mut self,
        d: &DraggedTab,
        ws_idx: usize,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if d.ws == ws_idx {
            if let Some(from) = self.ws.list[ws_idx].tab_index(d.tab) {
                self.ws.list[ws_idx].move_tab(from, index);
                self.session_changed(cx);
                cx.notify();
            }
        } else {
            self.move_tab_to_workspace(d.ws, d.tab, ws_idx, window, cx);
        }
    }

    /// New Agent in Worktree… (SPEC §7.12).
    pub fn new_agent_in_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.ws.active().root.clone() else {
            crate::toast::show(cx, "This workspace has no root folder; set one first.");
            return;
        };
        let agents: Vec<String> = Settings::global(cx).config.agents.defs.keys().cloned().collect();
        let default_agent = if agents.iter().any(|a| a == "claude") {
            "claude".to_string()
        } else {
            agents.first().cloned().unwrap_or_default()
        };
        let this = cx.entity().downgrade();
        DialogBuilder::new("New Agent in Worktree")
            .message(format!(
                "Runs `git worktree add` from {} and starts the agent there.",
                nox_terminal::process::tildify(&root)
            ))
            .field("Branch", "", "feat/my-change")
            .field(format!("Agent ({})", agents.join(", ")), default_agent, "claude")
            .cancel("Cancel")
            .button("Create", move |v, window, cx| {
                let branch = v.fields[0].trim().to_string();
                let agent = v.fields[1].trim().to_string();
                if branch.is_empty() {
                    return;
                }
                let root = root.clone();
                let cfg = Settings::global(cx).config.clone();
                let wt_dir = (!cfg.workspaces.worktree_dir.trim().is_empty())
                    .then(|| crate::icons::expand_tilde(cfg.workspaces.worktree_dir.trim()));
                let opens = cfg.workspaces.worktree_opens;
                let this = this.clone();
                let task = cx.background_spawn(async move {
                    nox_agents::worktree_add(&root, wt_dir.as_deref(), &branch)
                });
                window
                    .spawn(cx, async move |cx| {
                        let result = task.await;
                        let _ = this.update_in(cx, |this, window, cx| match result {
                            Ok(path) => match opens {
                                nox_config::WorktreeOpens::Tab => {
                                    let w = this.ws.active;
                                    this.launch_agent(w, &agent, Some(path), window, cx);
                                }
                                nox_config::WorktreeOpens::Workspace => {
                                    this.open_folder(path.clone(), true, window, cx);
                                    let w = this.ws.active;
                                    // Replace the plain shell with the agent tab.
                                    this.launch_agent(w, &agent, Some(path), window, cx);
                                }
                            },
                            Err(out) => {
                                DialogBuilder::new("git worktree add failed")
                                    .detail(out)
                                    .cancel("OK")
                                    .show(window, cx);
                            }
                        });
                    })
                    .detach();
            })
            .default_last()
            .show(window, cx);
    }
}

/// After closing a tab whose cwd is a clean git worktree, offer to remove it (never a dirty one).
pub fn offer_worktree_removal(path: &Path, window: &mut Window, cx: &mut App) {
    let Some(git_dir) = nox_agents::git_dir(path) else { return };
    // Only linked worktrees (their git dir lives under the main repo's .git/worktrees).
    if !git_dir.to_string_lossy().contains("/worktrees/") {
        return;
    }
    let Some(top) = nox_agents::repo_root(path) else { return };
    let task = cx.background_spawn({
        let top = top.clone();
        async move { nox_agents::worktree_is_clean(&top) }
    });
    window
        .spawn(cx, async move |cx| {
            if !task.await {
                return;
            }
            let _ = cx.update(|window, cx| {
                let top2 = top.clone();
                DialogBuilder::new("Remove the worktree?")
                    .message(format!(
                        "{} is clean. Remove it with `git worktree remove`? (The branch is kept.)",
                        nox_terminal::process::tildify(&top)
                    ))
                    .cancel("Keep")
                    .button("Remove", move |_, _, cx| {
                        if let Err(e) = nox_agents::worktree_remove(&top2) {
                            crate::toast::show(cx, format!("git worktree remove: {e}"));
                        }
                    })
                    .default_last()
                    .show(window, cx);
            });
        })
        .detach();
}

pub fn set_root_folder(root: &mut Root, window: &mut Window, cx: &mut Context<Root>) {
    let rx = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Set Root".into()),
    });
    let idx = root.ws.active;
    cx.spawn_in(window, async move |this, cx| {
        if let Ok(Ok(Some(paths))) = rx.await
            && let Some(p) = paths.into_iter().next()
        {
            let _ = this.update(cx, |this, cx| {
                if let Some(w) = this.ws.list.get_mut(idx) {
                    w.root = Some(p);
                    w.root_missing = false;
                }
                this.session_changed(cx);
                cx.notify();
            });
        }
    })
    .detach();
}

/// Set Icon… picker for a tab (its focused pane) or a pane.
pub fn set_icon_picker(
    root: &mut Root,
    tab: Option<TabId>,
    pane: Option<PaneId>,
    window: &mut Window,
    cx: &mut Context<Root>,
) {
    let target = pane.or_else(|| tab.and_then(|t| root.ws.active().tab(t).map(|t| t.focused)));
    let Some(target) = target else { return };
    let this = cx.entity().downgrade();
    let source: crate::palette::Source = std::rc::Rc::new(move |_, _| {
        let mut items = vec![PaletteItem::new("Automatic").detail("follow the running program").on_select({
            let this = this.clone();
            move |_, cx| {
                let _ = this.update(cx, |root, cx| set_custom_icon(root, target, None, cx));
            }
        })];
        for name in nox_agents::builtin_icon_names() {
            let this = this.clone();
            items.push(PaletteItem::new(*name).icon(format!("icons/process/{name}.svg"), None).on_select(
                move |_, cx| {
                    let _ =
                        this.update(cx, |root, cx| set_custom_icon(root, target, Some(name.to_string()), cx));
                },
            ));
        }
        items
    });
    let palette = cx.new(|cx| {
        Palette::new(
            source,
            PaletteOptions { placeholder: "Set icon…".into(), ..Default::default() },
            window,
            cx,
        )
    });
    root.open_palette(palette, window, cx);
}

fn set_custom_icon(root: &mut Root, pane: PaneId, icon: Option<String>, cx: &mut Context<Root>) {
    if let Some(e) = root.panes.get(&pane) {
        e.update(cx, |e, cx| {
            e.custom_icon = icon;
            e.refresh_info(cx);
            cx.notify();
        });
    }
    cx.notify();
}

/// Install Agent Hooks… (SPEC §7.12): show the change to Claude Code's settings and write it
/// only after the user confirms.
pub fn install_agent_hooks(window: &mut Window, cx: &mut App) {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return };
    let path = home.join(".claude/settings.json");
    let existing = std::fs::read_to_string(&path).ok();
    let new = match nox_agents::claude_hooks_patch(existing.as_deref()) {
        Ok(n) => n,
        Err(e) => {
            crate::toast::show(cx, format!("Can't update {}: {e}", path.display()));
            return;
        }
    };
    if existing.as_deref().map(str::trim) == Some(new.trim()) {
        crate::toast::show(cx, "Claude Code hooks are already installed.");
        return;
    }
    let diff = line_diff(existing.as_deref().unwrap_or(""), &new);
    DialogBuilder::new("Install Claude Code hooks?")
        .message(format!(
            "nox will add hooks to {} so Claude Code reports working / needs input / idle and its session id (for resume). Codex and Gemini CLI hooks: see SPEC §7.12.",
            nox_terminal::process::tildify(&path)
        ))
        .detail(diff)
        .cancel("Cancel")
        .button("Install", move |_, _, cx| {
            let res = path
                .parent()
                .map(std::fs::create_dir_all)
                .transpose()
                .and_then(|_| std::fs::write(&path, &new));
            match res {
                Ok(()) => crate::toast::show(cx, "Claude Code hooks installed."),
                Err(e) => crate::toast::show(cx, format!("Writing {}: {e}", path.display())),
            }
        })
        .default_last()
        .show(window, cx);
}

/// A minimal line diff (`-` / `+`) for review sheets.
#[must_use]
pub fn line_diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    // LCS table.
    let (n, m) = (a.len(), b.len());
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            out.push(format!("  {}", a[i]));
            i += 1;
            j += 1;
        } else if j < m && (i == n || dp[i][j + 1] >= dp[i + 1][j]) {
            out.push(format!("+ {}", b[j]));
            j += 1;
        } else {
            out.push(format!("- {}", a[i]));
            i += 1;
        }
    }
    out.join("\n")
}

/// Install Command Line Tool… : symlink the bundled `nox` into /usr/local/bin.
pub fn install_cli(window: &mut Window, cx: &mut App) {
    let Some(src) = crate::root::cli_bin_dir().map(|d| d.join("nox")) else {
        crate::toast::show(cx, "The nox command line tool wasn't found next to the app.");
        return;
    };
    let dest = PathBuf::from("/usr/local/bin/nox");
    DialogBuilder::new("Install the nox command line tool?")
        .message(format!("Creates a symlink {} → {}", dest.display(), src.display()))
        .cancel("Cancel")
        .button("Install", move |_, _, cx| {
            let _ = std::fs::remove_file(&dest);
            match std::os::unix::fs::symlink(&src, &dest) {
                Ok(()) => crate::toast::show(cx, "Installed /usr/local/bin/nox."),
                Err(e) => crate::toast::show(
                    cx,
                    format!(
                        "Couldn't create {} ({e}). Run: sudo ln -sf '{}' {}",
                        dest.display(),
                        src.display(),
                        dest.display()
                    ),
                ),
            }
        })
        .default_last()
        .show(window, cx);
}

/// Copy MCP Setup Command: a picker over configured agents.
pub fn copy_mcp_setup(root: &mut Root, window: &mut Window, cx: &mut Context<Root>) {
    let kinds: Vec<String> = Settings::global(cx).config.agents.defs.keys().cloned().collect();
    let source: crate::palette::Source = std::rc::Rc::new(move |_, _| {
        kinds
            .iter()
            .filter_map(|k| {
                let cmd = nox_agents::mcp_setup_command(k)?;
                let c2 = cmd.clone();
                Some(PaletteItem::new(k.clone()).detail(cmd).on_select(move |_, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(c2.clone()));
                    crate::toast::show(cx, format!("Copied: {c2}"));
                }))
            })
            .collect()
    });
    let palette = cx.new(|cx| {
        Palette::new(
            source,
            PaletteOptions { placeholder: "Copy MCP setup command for…".into(), ..Default::default() },
            window,
            cx,
        )
    });
    root.open_palette(palette, window, cx);
}

/// Open config.toml in `$VISUAL` / `$EDITOR` in a new tab, else `open -t`.
pub fn open_config_file(root: &mut Root, window: &mut Window, cx: &mut Context<Root>) {
    let path = nox_config::config_path();
    let _ = nox_config::ensure_default_file(&path);
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|e| !e.is_empty())
        .or_else(|| std::env::var("EDITOR").ok().filter(|e| !e.is_empty()));
    match editor {
        Some(ed) => {
            let w = root.ws.active;
            let cmd = format!("{ed} {}", crate::pane::shell_escape(&path.to_string_lossy()));
            root.add_tab(
                w,
                PaneSpawn {
                    cwd: path.parent().map(Path::to_path_buf),
                    startup: Some(Startup::Run(cmd)),
                    ..Default::default()
                },
                window,
                cx,
            );
        }
        None => {
            let _ = std::process::Command::new("/usr/bin/open").arg("-t").arg(&path).spawn();
        }
    }
}

#[allow(dead_code)]
fn _unused(_: AgentLaunch, _: Tab) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::tests::{canon, dialog_title, open, pane_cwd, quiet_config, set_dialog_field};
    use gpui::{Entity, TestAppContext, VisualTestContext};

    fn fake_agent_config() -> nox_config::Config {
        let mut cfg = quiet_config();
        cfg.agents.defs.clear();
        cfg.agents
            .defs
            .insert("fake".into(), nox_config::AgentDef { command: "true".into(), ..Default::default() });
        cfg
    }

    fn write_project(dir: &Path, toml: &str) {
        std::fs::create_dir_all(dir.join(".nox")).unwrap();
        std::fs::write(ProjectFile::path(dir), toml).unwrap();
    }

    fn open_folder(root: &Entity<Root>, path: &Path, force: bool, cx: &mut VisualTestContext) -> WorkspaceId {
        let id = root.update_in(cx, |r, w, cx| r.open_folder(path.to_path_buf(), force, w, cx));
        cx.run_until_parked();
        id
    }

    #[test]
    fn line_diff_marks_added_and_removed_lines() {
        assert_eq!(line_diff("a\nb\nc", "a\nc\nd"), "  a\n- b\n  c\n+ d");
        assert_eq!(line_diff("", "x"), "+ x");
        assert_eq!(line_diff("x", ""), "- x");
        assert_eq!(line_diff("same", "same"), "  same");
    }

    #[gpui::test]
    fn create_workspace_names_from_folder_or_default(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (root, cx) = open(cx, quiet_config());
        root.update_in(cx, |r, w, cx| {
            let a = r.create_workspace(None, Some(dir.path().to_path_buf()), w, cx);
            assert_eq!(r.ws.list[a].name, dir_name(dir.path()));
            let b = r.create_workspace(None, Some(dir.path().to_path_buf()), w, cx);
            assert_ne!(r.ws.list[b].name, r.ws.list[a].name, "names stay unique");
            let c = r.create_workspace(Some("   ".into()), None, w, cx);
            assert!(!r.ws.list[c].name.trim().is_empty());
            assert_eq!(r.ws.active, c);
            assert!(r.coop.contains_key(&r.ws.list[c].id));
        });
    }

    #[gpui::test]
    fn open_folder_reuses_a_workspace_with_that_root(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (root, cx) = open(cx, quiet_config());
        let id = open_folder(&root, dir.path(), false, cx);
        let p = root.read_with(cx, |r, _| {
            assert_eq!(r.ws.len(), 2);
            assert_eq!(r.ws.active().id, id);
            assert_eq!(r.ws.active().root.as_deref(), Some(canon(dir.path()).as_path()));
            assert_eq!(r.ws.active().tabs.len(), 1);
            r.focused_pane_id().unwrap()
        });
        assert_eq!(pane_cwd(&root, p, cx), Some(canon(dir.path())));
        root.update_in(cx, |r, w, cx| r.switch_workspace(0, w, cx));
        assert_eq!(open_folder(&root, dir.path(), false, cx), id);
        root.read_with(cx, |r, _| {
            assert_eq!(r.ws.len(), 2);
            assert_eq!(r.ws.active().id, id);
        });
        // ⌥: always a new workspace.
        assert_ne!(open_folder(&root, dir.path(), true, cx), id);
        assert_eq!(root.read_with(cx, |r, _| r.ws.len()), 3);
    }

    #[gpui::test]
    fn project_file_without_commands_opens_its_tabs(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        write_project(
            dir.path(),
            r#"
name = "Proj"
color = "green"
env = { FOO = "bar" }
[[tabs]]
title = "editor"
[[tabs]]
title = "split"
layout = "horizontal"
panes = [{}, {}, {}]
"#,
        );
        let (root, cx) = open(cx, quiet_config());
        open_folder(&root, dir.path(), false, cx);
        assert_eq!(dialog_title(cx), None, "nothing to trust");
        root.read_with(cx, |r, _| {
            let w = r.ws.active();
            assert_eq!(w.name, "Proj");
            assert_eq!(w.color.as_deref(), Some("green"));
            assert_eq!(w.env.get("FOO").map(String::as_str), Some("bar"));
            assert_eq!(w.tabs.len(), 2);
            assert_eq!(w.active_tab, 0);
            assert_eq!(w.tabs[0].custom_name.as_deref(), Some("editor"));
            assert_eq!(w.tabs[1].custom_name.as_deref(), Some("split"));
            assert_eq!(w.tabs[1].panes().len(), 3);
            // The first pane keeps focus.
            assert_eq!(w.tabs[1].focused, w.tabs[1].panes()[0]);
            assert_eq!(r.panes.len(), 1 + 4);
        });
    }

    #[gpui::test]
    fn project_commands_wait_for_trust(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        write_project(
            dir.path(),
            r#"
[[tabs]]
command = "true one"
[[tabs]]
panes = [{ command = "true two" }, {}]
"#,
        );
        let (root, cx) = open(cx, quiet_config());
        open_folder(&root, dir.path(), false, cx);
        assert_eq!(dialog_title(cx).as_deref(), Some("Trust this project file?"));
        let detail = cx.update(|_, cx| {
            let v = cx.global::<crate::dialog::ActiveDialog>().0.as_ref().unwrap().0.clone();
            v.read(cx).detail.clone()
        });
        assert_eq!(detail.as_deref(), Some("true one\ntrue two"));
        // Tabs are open with plain shells already.
        root.read_with(cx, |r, _| {
            assert_eq!(r.ws.active().tabs.len(), 2);
            assert_eq!(r.ws.active().tabs[1].panes().len(), 2);
        });
        cx.simulate_keystrokes("enter");
        assert_eq!(dialog_title(cx), None);
        let file = ProjectFile::path(&canon(dir.path()));
        let (_, hash) = ProjectFile::load(&canon(dir.path())).unwrap().unwrap();
        assert!(TrustStore::load(&trust_path()).unwrap().is_trusted(&file, &hash));
        // Trusted now: opening again asks nothing.
        open_folder(&root, dir.path(), true, cx);
        assert_eq!(dialog_title(cx), None);
    }

    #[gpui::test]
    fn broken_project_file_still_opens_the_folder(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        write_project(dir.path(), "tabs = 3");
        let (root, cx) = open(cx, quiet_config());
        open_folder(&root, dir.path(), false, cx);
        root.read_with(cx, |r, _| {
            assert_eq!(r.ws.len(), 2);
            assert_eq!(r.ws.active().tabs.len(), 1);
            assert_eq!(r.ws.active().name, dir_name(dir.path()));
        });
    }

    #[gpui::test]
    fn new_workspace_sheet_creates_named_or_folder_workspaces(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (root, cx) = open(cx, quiet_config());
        cx.dispatch_action(crate::actions::NewWorkspace);
        assert_eq!(dialog_title(cx).as_deref(), Some("New Workspace"));
        set_dialog_field(cx, 0, "notes");
        cx.simulate_keystrokes("enter");
        root.read_with(cx, |r, _| {
            assert_eq!(r.ws.len(), 2);
            assert_eq!(r.ws.active().name, "notes");
            assert_eq!(r.ws.active().root, None);
            assert_eq!(r.ws.active().tabs.len(), 1);
        });
        // Only a folder: opened as a workspace named after it.
        cx.dispatch_action(crate::actions::NewWorkspace);
        set_dialog_field(cx, 1, &dir.path().to_string_lossy());
        cx.simulate_keystrokes("enter");
        root.read_with(cx, |r, _| {
            assert_eq!(r.ws.len(), 3);
            assert_eq!(r.ws.active().name, dir_name(dir.path()));
            assert_eq!(r.ws.active().root.as_deref(), Some(canon(dir.path()).as_path()));
        });
        // Name and a folder that doesn't exist: the folder is ignored.
        cx.dispatch_action(crate::actions::NewWorkspace);
        set_dialog_field(cx, 0, "named");
        set_dialog_field(cx, 1, "/definitely/not/there");
        cx.simulate_keystrokes("enter");
        root.read_with(cx, |r, _| {
            assert_eq!(r.ws.len(), 4);
            assert_eq!(r.ws.active().name, "named");
            assert_eq!(r.ws.active().root, None);
        });
    }

    #[gpui::test]
    fn moving_tabs_between_workspaces(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        let (t0, t1, p1) = root.update_in(cx, |r, w, cx| {
            r.new_tab(w, cx);
            let t0 = r.ws.list[0].tabs[0].id;
            let t1 = r.ws.list[0].tabs[1].id;
            let p1 = r.ws.list[0].tabs[1].focused;
            let idx = r.create_workspace(Some("dest".into()), None, w, cx);
            r.add_tab(idx, PaneSpawn::default(), w, cx);
            (t0, t1, p1)
        });
        root.update_in(cx, |r, w, cx| {
            // No-ops.
            r.move_tab_to_workspace(0, t1, 0, w, cx);
            r.move_tab_to_workspace(0, t1, 9, w, cx);
            assert_eq!(r.ws.list[0].tabs.len(), 2);
            r.move_tab_to_workspace(0, t1, 1, w, cx);
            assert_eq!(r.ws.list[0].tabs.len(), 1);
            assert_eq!(r.ws.list[1].tabs.len(), 2);
            // The pane keeps running.
            assert!(r.panes.contains_key(&p1));
            assert_eq!(r.pane_workspace(p1), Some(r.ws.list[1].id));
            // Reorder inside the destination by drop.
            let d = DraggedTab { ws: 1, tab: t1, label: "x".into() };
            r.drop_tab(&d, 1, 0, w, cx);
            assert_eq!(r.ws.list[1].tabs[0].id, t1);
            // Dropping the last tab of workspace 0 elsewhere closes it (close_when_empty).
            let d = DraggedTab { ws: 0, tab: t0, label: "y".into() };
            r.drop_tab(&d, 1, 0, w, cx);
            assert_eq!(r.ws.len(), 1);
            assert_eq!(r.ws.list[0].name, "dest");
            assert_eq!(r.ws.list[0].tabs.len(), 3);
        });
    }

    #[gpui::test]
    fn moving_the_last_tab_keeps_the_source_when_configured(cx: &mut TestAppContext) {
        let mut cfg = quiet_config();
        cfg.workspaces.close_when_empty = false;
        let (root, cx) = open(cx, cfg);
        root.update_in(cx, |r, w, cx| {
            let t0 = r.ws.list[0].tabs[0].id;
            let idx = r.create_workspace(Some("dest".into()), None, w, cx);
            r.add_tab(idx, PaneSpawn::default(), w, cx);
            r.move_tab_to_workspace(0, t0, 1, w, cx);
            assert_eq!(r.ws.len(), 2);
            assert_eq!(r.ws.list[0].tabs.len(), 1, "a fresh tab replaces the moved one");
            assert_ne!(r.ws.list[0].tabs[0].id, t0);
            assert_eq!(r.ws.list[1].tabs.len(), 2);
        });
    }

    #[gpui::test]
    fn agent_in_worktree_needs_a_root(cx: &mut TestAppContext) {
        let (_root, cx) = open(cx, fake_agent_config());
        cx.dispatch_action(crate::actions::NewAgentInWorktree);
        assert_eq!(dialog_title(cx), None);
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    #[gpui::test]
    fn agent_in_worktree_creates_the_worktree_and_offers_removal(cx: &mut TestAppContext) {
        let repo = tempfile::tempdir().unwrap();
        let wt = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-q"]);
        std::fs::write(repo.path().join("f"), "x").unwrap();
        git(repo.path(), &["add", "f"]);
        git(repo.path(), &["commit", "-q", "-m", "init"]);
        let mut cfg = fake_agent_config();
        cfg.workspaces.worktree_dir = wt.path().to_string_lossy().into_owned();
        let (root, cx) = open(cx, cfg);
        open_folder(&root, repo.path(), false, cx);

        cx.dispatch_action(crate::actions::NewAgentInWorktree);
        assert_eq!(dialog_title(cx).as_deref(), Some("New Agent in Worktree"));
        // No branch: nothing happens.
        cx.simulate_keystrokes("enter");
        assert_eq!(root.read_with(cx, |r, _| r.ws.active().tabs.len()), 1);

        cx.dispatch_action(crate::actions::NewAgentInWorktree);
        set_dialog_field(cx, 0, "feat/x");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let path = wt.path().join(format!("{}-feat-x", dir_name(repo.path())));
        assert!(path.is_dir(), "worktree created");
        let (tab, kind) = root.read_with(cx, |r, cx| {
            let w = r.ws.active();
            assert_eq!(w.tabs.len(), 2);
            let p = r.focused_pane().unwrap();
            (w.active().unwrap().id, p.read(cx).agent.as_ref().map(|a| a.kind.clone()))
        });
        assert_eq!(kind.as_deref(), Some("fake"));

        // Closing the agent's tab offers to remove the clean worktree.
        root.update_in(cx, |r, w, cx| {
            let a = r.ws.active;
            r.close_tab_now(a, tab, w, cx);
        });
        cx.run_until_parked();
        assert_eq!(dialog_title(cx).as_deref(), Some("Remove the worktree?"));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!path.exists(), "worktree removed");

        // A failing `git worktree add` shows its output.
        cx.dispatch_action(crate::actions::NewAgentInWorktree);
        set_dialog_field(cx, 0, "feat/x");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(dialog_title(cx).as_deref(), Some("git worktree add failed"));
    }

    #[gpui::test]
    fn icon_picker_sets_and_clears_a_custom_icon(cx: &mut TestAppContext) {
        let (root, cx) = open(cx, quiet_config());
        let name = nox_agents::builtin_icon_names()[0];
        let p = root.update_in(cx, |r, w, cx| {
            let p = r.focused_pane_id().unwrap();
            set_icon_picker(r, None, Some(p), w, cx);
            assert!(r.palette.is_some());
            p
        });
        cx.simulate_input(name);
        cx.simulate_keystrokes("enter");
        root.read_with(cx, |r, cx| {
            assert!(r.palette.is_none());
            assert_eq!(r.panes[&p].read(cx).custom_icon.as_deref(), Some(name));
        });
        root.update_in(cx, |r, w, cx| {
            let t = r.active_tab().unwrap().id;
            set_icon_picker(r, Some(t), None, w, cx);
        });
        cx.simulate_input("Automatic");
        cx.simulate_keystrokes("enter");
        root.read_with(cx, |r, cx| assert_eq!(r.panes[&p].read(cx).custom_icon, None));
        // No target: no picker.
        root.update_in(cx, |r, w, cx| {
            set_icon_picker(r, None, None, w, cx);
            assert!(r.palette.is_none());
        });
    }

    #[gpui::test]
    fn copy_mcp_setup_puts_the_command_on_the_clipboard(cx: &mut TestAppContext) {
        let (_root, cx) = open(cx, quiet_config());
        cx.dispatch_action(crate::actions::CopyMcpSetupCommand);
        cx.simulate_input("claude");
        cx.simulate_keystrokes("enter");
        let text = cx.read_from_clipboard().and_then(|c| c.text());
        assert_eq!(text.as_deref(), Some("claude mcp add nox -- nox mcp"));
    }

    #[gpui::test]
    fn install_cli_without_the_tool_shows_no_sheet(cx: &mut TestAppContext) {
        let (_root, cx) = open(cx, quiet_config());
        if crate::root::cli_bin_dir().is_none() {
            cx.dispatch_action(crate::actions::InstallCommandLineTool);
            assert_eq!(dialog_title(cx), None);
        }
    }
}
