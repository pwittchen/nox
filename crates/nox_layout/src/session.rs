//! Session file (`~/.nox/session.toml`, SPEC §6.6): snapshot, atomic save, load, restore.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ids::{PaneId, TabId, WorkspaceId};
use crate::layout::Layout;
use crate::tab::Tab;
use crate::workspace::{Workspace, Workspaces};

/// Current session file format version.
pub const SESSION_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session file I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot serialize session: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("cannot parse session: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("unsupported session version {0} (this nox reads up to {SESSION_VERSION})")]
    UnsupportedVersion(u32),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionFile {
    pub version: u32,
    pub active_workspace: usize,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub id: WorkspaceId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub active_tab: usize,
    #[serde(default)]
    pub tabs: Vec<TabSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_icon: Option<String>,
    pub focused: PaneId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximized: Option<PaneId>,
    pub layout: Layout,
    /// Per pane state, keyed by pane id (as a string in TOML).
    #[serde(default, with = "pane_map")]
    pub panes: BTreeMap<PaneId, PaneSnapshot>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PaneSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSnapshot {
    /// `[agents.<name>]` key, e.g. `claude`.
    pub kind: String,
    /// Agent name (SPEC §4.8), e.g. `claude-2`.
    pub name: String,
    pub start_cwd: PathBuf,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub resume_on_restore: bool,
}

/// TOML map keys must be strings: (de)serialize `BTreeMap<PaneId, _>` with string keys.
mod pane_map {
    use std::collections::BTreeMap;

    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    use super::PaneSnapshot;
    use crate::ids::PaneId;

    pub fn serialize<S: Serializer>(map: &BTreeMap<PaneId, PaneSnapshot>, s: S) -> Result<S::Ok, S::Error> {
        s.collect_map(map.iter().map(|(k, v)| (k.to_string(), v)))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<PaneId, PaneSnapshot>, D::Error> {
        BTreeMap::<String, PaneSnapshot>::deserialize(d)?
            .into_iter()
            .map(|(k, v)| {
                k.parse::<PaneId>()
                    .map(|id| (id, v))
                    .map_err(|e| D::Error::custom(format!("invalid pane id {k:?}: {e}")))
            })
            .collect()
    }
}

impl SessionFile {
    pub fn to_toml(&self) -> Result<String, SessionError> {
        Ok(toml::to_string(self)?)
    }

    pub fn from_toml(s: &str) -> Result<Self, SessionError> {
        let file: SessionFile = toml::from_str(s)?;
        if file.version > SESSION_VERSION {
            return Err(SessionError::UnsupportedVersion(file.version));
        }
        Ok(file)
    }
}

/// Writes the session atomically: a temp file in the same directory, fsync, rename.
/// Creates the parent directory if needed.
pub fn save_session_atomic(path: &Path, session: &SessionFile) -> Result<(), SessionError> {
    let text = session.to_toml()?;
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::Builder::new().prefix(".session.").suffix(".tmp").tempfile_in(dir)?;
    tmp.write_all(text.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

pub fn load_session(path: &Path) -> Result<SessionFile, SessionError> {
    SessionFile::from_toml(&std::fs::read_to_string(path)?)
}

/// One pane created by [`Workspaces::restore`]: the UI spawns a shell (or resumes the agent)
/// for each.
#[derive(Clone, Debug, PartialEq)]
pub struct RestoredPane {
    pub old: PaneId,
    pub new: PaneId,
    pub workspace: WorkspaceId,
    pub tab: TabId,
    pub snapshot: PaneSnapshot,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Restored {
    pub workspaces: Workspaces,
    /// Saved pane id → fresh pane id.
    pub id_map: BTreeMap<PaneId, PaneId>,
    /// Every new pane, in workspace / tab / tree order.
    pub panes: Vec<RestoredPane>,
}

impl Workspaces {
    /// Snapshot for the session file; `pane_info` supplies each pane's cwd, name and agent.
    pub fn snapshot(&self, pane_info: impl Fn(PaneId) -> PaneSnapshot) -> SessionFile {
        SessionFile {
            version: SESSION_VERSION,
            active_workspace: self.active,
            workspaces: self
                .list
                .iter()
                .map(|w| WorkspaceSnapshot {
                    id: w.id,
                    name: w.name.clone(),
                    root: w.root.clone(),
                    color: w.color.clone(),
                    env: w.env.clone(),
                    active_tab: w.active_tab,
                    tabs: w
                        .tabs
                        .iter()
                        .map(|t| TabSnapshot {
                            custom_name: t.custom_name.clone(),
                            custom_icon: t.custom_icon.clone(),
                            focused: t.focused,
                            maximized: t.maximized,
                            layout: t.layout.clone(),
                            panes: t.panes().into_iter().map(|p| (p, pane_info(p))).collect(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    /// Rebuilds workspaces from a session file with fresh pane and tab ids (workspace ids are
    /// kept, and the id generators are bumped above them). `root_exists` decides whether a
    /// saved root is still there; a missing root is dropped and `root_missing` set.
    ///
    /// Tolerant of hand-edited files: a focused / maximized pane not in the layout falls back
    /// to the first pane / no maximize, out-of-range indices are clamped, duplicate workspace
    /// ids get fresh ids, workspaces without tabs are dropped. Returns `None` when no
    /// workspace with tabs remains.
    pub fn restore(file: SessionFile, root_exists: impl Fn(&Path) -> bool) -> Option<Restored> {
        for w in &file.workspaces {
            WorkspaceId::bump_above(w.id);
        }
        let mut id_map = BTreeMap::new();
        let mut panes = Vec::new();
        let mut list: Vec<Workspace> = Vec::new();
        let mut active = 0;
        for (wi, ws) in file.workspaces.into_iter().enumerate() {
            if ws.tabs.is_empty() {
                continue;
            }
            if wi == file.active_workspace {
                active = list.len();
            }
            let id = if list.iter().any(|w| w.id == ws.id) { WorkspaceId::next() } else { ws.id };
            let mut w = Workspace::new(id, ws.name, None);
            w.color = ws.color;
            w.env = ws.env;
            match ws.root {
                Some(r) if root_exists(&r) => w.root = Some(r),
                Some(_) => w.root_missing = true,
                None => {}
            }
            for ts in ws.tabs {
                let tab_id = TabId::next();
                let mut local: BTreeMap<PaneId, PaneId> = BTreeMap::new();
                let layout = ts.layout.map_panes(&mut |old| {
                    let new = PaneId::next();
                    local.entry(old).or_insert(new);
                    id_map.insert(old, new);
                    panes.push(RestoredPane {
                        old,
                        new,
                        workspace: id,
                        tab: tab_id,
                        snapshot: ts.panes.get(&old).cloned().unwrap_or_default(),
                    });
                    new
                });
                let mut tab = Tab::with_layout(tab_id, layout);
                tab.custom_name = ts.custom_name;
                tab.custom_icon = ts.custom_icon;
                if let Some(f) = local.get(&ts.focused) {
                    tab.focused = *f;
                }
                tab.maximized =
                    ts.maximized.and_then(|m| local.get(&m).copied()).filter(|_| tab.layout.len() > 1);
                w.tabs.push(tab);
            }
            w.active_tab = ws.active_tab.min(w.tabs.len() - 1);
            list.push(w);
        }
        if list.is_empty() {
            return None;
        }
        Some(Restored { workspaces: Workspaces::from_list(list, active), id_map, panes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::SplitDirection;

    fn sample() -> Workspaces {
        let mut t1 = Tab::new(TabId(10), PaneId(1));
        t1.split_focused(SplitDirection::Vertical, PaneId(2));
        t1.split_focused(SplitDirection::Horizontal, PaneId(3));
        t1.layout.set_ratio_raw(&crate::SplitPath::root(), 0.3);
        t1.custom_name = Some("agent \"quoted\"".into());
        t1.toggle_maximize();
        let mut w1 = Workspace::new(WorkspaceId(5), "nox", Some("/src/nox".into())).with_tab(t1);
        w1.color = Some("blue".into());
        w1.env.insert("FOO".into(), "bar baz".into());
        w1.add_tab_after_active(Tab::new(TabId(11), PaneId(4)));
        w1.select_tab(0);
        let w2 = Workspace::new(WorkspaceId(6), "scratch", None).with_tab(Tab::new(TabId(12), PaneId(5)));
        let mut all = Workspaces::new(w1);
        all.push(w2);
        all.switch_to(1);
        all
    }

    fn info(p: PaneId) -> PaneSnapshot {
        PaneSnapshot {
            cwd: Some(format!("/tmp/{p}").into()),
            custom_name: (p.0 == 2).then(|| "server".to_string()),
            agent: (p.0 == 3).then(|| AgentSnapshot {
                kind: "claude".into(),
                name: "claude-2".into(),
                start_cwd: "/src/nox".into(),
                command: "claude --model x".into(),
                session_id: Some("abc-123".into()),
                resume_on_restore: true,
            }),
        }
    }

    #[test]
    fn toml_round_trip() {
        let file = sample().snapshot(info);
        let text = file.to_toml().unwrap();
        let back = SessionFile::from_toml(&text).unwrap();
        assert_eq!(back, file, "{text}");
        assert!(text.contains("[workspaces.tabs.panes.3.agent]"));
        assert!(text.contains("ratio = 0.3\n"), "{text}");
    }

    #[test]
    fn deep_layout_round_trips_in_toml() {
        let mut t = Tab::new(TabId(1), PaneId(1));
        for i in 2..12 {
            let dir = if i % 2 == 0 { SplitDirection::Vertical } else { SplitDirection::Horizontal };
            t.split_focused(dir, PaneId(i));
            if i % 3 == 0 {
                t.focus(PaneId(i - 2));
            }
        }
        t.layout.set_ratio_raw(&crate::SplitPath::root(), 0.123_456_79);
        let all = Workspaces::new(Workspace::new(WorkspaceId(1), "w", None).with_tab(t));
        let file = all.snapshot(|_| PaneSnapshot::default());
        let back = SessionFile::from_toml(&file.to_toml().unwrap()).unwrap();
        assert_eq!(back, file);
    }

    #[test]
    fn save_and_load_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("session.toml");
        let file = sample().snapshot(info);
        save_session_atomic(&path, &file).unwrap();
        assert_eq!(load_session(&path).unwrap(), file);
        // Overwrite works and leaves no temp files behind.
        save_session_atomic(&path, &file).unwrap();
        let entries: Vec<_> =
            std::fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("session.toml")]);
    }

    #[test]
    fn load_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.toml");
        assert!(matches!(load_session(&path), Err(SessionError::Io(_))));
        std::fs::write(&path, "not = [valid").unwrap();
        assert!(matches!(load_session(&path), Err(SessionError::Parse(_))));
        std::fs::write(&path, "version = 99\nactive_workspace = 0\n").unwrap();
        assert!(matches!(load_session(&path), Err(SessionError::UnsupportedVersion(99))));
    }

    #[test]
    fn restore_rebuilds_with_fresh_ids() {
        let original = sample();
        let file = original.snapshot(info);
        let r = Workspaces::restore(file, |p| p != Path::new("/gone")).unwrap();
        let all = &r.workspaces;
        assert_eq!(all.len(), 2);
        assert_eq!(all.active, 1);
        assert_eq!(all.list[0].id, WorkspaceId(5));
        assert_eq!(all.list[0].root.as_deref(), Some(Path::new("/src/nox")));
        assert_eq!(all.list[0].env.get("FOO").map(String::as_str), Some("bar baz"));
        assert_eq!(all.list[0].active_tab, 0);
        assert_eq!(all.list[1].name, "scratch");
        assert_eq!(r.panes.len(), 5);
        assert_eq!(r.id_map.len(), 5);

        let t = &all.list[0].tabs[0];
        assert_eq!(t.custom_name.as_deref(), Some("agent \"quoted\""));
        assert_eq!(t.focused, r.id_map[&PaneId(3)]);
        assert_eq!(t.maximized, Some(t.focused));
        assert!(t.is_consistent());
        let old = &original.list[0].tabs[0].layout;
        let mapped = old.map_panes(&mut |p| r.id_map[&p]);
        assert_eq!(t.layout, mapped);
        // Fresh ids differ from the saved ones.
        assert!(r.panes.iter().all(|p| p.old != p.new || p.old.0 > 1000));
        let agent_pane = r.panes.iter().find(|p| p.old == PaneId(3)).unwrap();
        assert_eq!(agent_pane.snapshot.agent.as_ref().unwrap().session_id.as_deref(), Some("abc-123"));
        assert_eq!(agent_pane.tab, t.id);
        // The previously active workspace is the toggle target.
        assert_eq!(all.last_active(), Some(0));
    }

    #[test]
    fn restore_missing_root_and_bad_indices() {
        let mut file = sample().snapshot(info);
        file.active_workspace = 42;
        file.workspaces[0].root = Some("/gone".into());
        file.workspaces[0].active_tab = 99;
        file.workspaces[0].tabs[0].focused = PaneId(777);
        file.workspaces[0].tabs[0].maximized = Some(PaneId(778));
        file.workspaces.push(WorkspaceSnapshot {
            id: WorkspaceId(9),
            name: "empty".into(),
            root: None,
            color: None,
            env: BTreeMap::new(),
            active_tab: 0,
            tabs: vec![],
        });
        let r = Workspaces::restore(file, |p| p != Path::new("/gone")).unwrap();
        let w = &r.workspaces.list[0];
        assert!(w.root_missing);
        assert_eq!(w.root, None);
        assert_eq!(w.active_tab, 1);
        assert_eq!(w.tabs[0].focused, w.tabs[0].layout.first_pane());
        assert_eq!(w.tabs[0].maximized, None);
        assert_eq!(r.workspaces.len(), 2);
        assert_eq!(r.workspaces.active, 0);
        assert!(WorkspaceId::next().0 > 9);
    }

    #[test]
    fn restore_empty_is_none() {
        let file = SessionFile { version: SESSION_VERSION, active_workspace: 0, workspaces: vec![] };
        assert!(Workspaces::restore(file, |_| true).is_none());
    }
}
