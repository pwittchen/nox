//! Workspaces: named groups of tabs (SPEC §4.7, §6.5, §6.7).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::ids::{PaneId, TabId, WorkspaceId};
use crate::tab::Tab;

/// Status dot of a tab or workspace (SPEC §4.2). Ordered by priority:
/// `Bell > NeedsInput > Running > Done > None`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TabStatus {
    #[default]
    None,
    Done,
    Running,
    NeedsInput,
    Bell,
}

impl TabStatus {
    pub fn aggregate(iter: impl IntoIterator<Item = TabStatus>) -> TabStatus {
        iter.into_iter().max().unwrap_or_default()
    }
}

/// The highest-priority status of `iter` (a workspace's status from its tabs, SPEC §4.7).
pub fn aggregate_status(iter: impl IntoIterator<Item = TabStatus>) -> TabStatus {
    TabStatus::aggregate(iter)
}

/// Outcome of [`Workspace::close_tab`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceAfterClose {
    /// Tabs remain; `active_tab` was adjusted.
    Remaining,
    /// That was the last tab: close the workspace or open a new tab
    /// (`workspaces.close_when_empty`).
    Empty,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub root: Option<PathBuf>,
    /// A theme ANSI color name.
    pub color: Option<String>,
    pub env: BTreeMap<String, String>,
    pub tabs: Vec<Tab>,
    /// Index into `tabs`; meaningless (0) while `tabs` is empty.
    pub active_tab: usize,
    /// The saved root no longer existed on restore (SPEC §6.6).
    pub root_missing: bool,
}

impl Workspace {
    /// A workspace without tabs; add one with [`Workspace::add_tab_after_active`].
    pub fn new(id: WorkspaceId, name: impl Into<String>, root: Option<PathBuf>) -> Self {
        Self {
            id,
            name: name.into(),
            root,
            color: None,
            env: BTreeMap::new(),
            tabs: Vec::new(),
            active_tab: 0,
            root_missing: false,
        }
    }

    #[must_use]
    pub fn with_tab(mut self, tab: Tab) -> Self {
        self.add_tab_after_active(tab);
        self
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    #[must_use]
    pub fn active(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab)
    }

    pub fn active_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active_tab)
    }

    #[must_use]
    pub fn tab_index(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    #[must_use]
    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    /// Index of the tab holding `pane`.
    #[must_use]
    pub fn find_pane(&self, pane: PaneId) -> Option<usize> {
        self.tabs.iter().position(|t| t.contains(pane))
    }

    /// `⌘T`: inserts `tab` right after the active tab and activates it. Returns its index.
    pub fn add_tab_after_active(&mut self, tab: Tab) -> usize {
        let i = if self.tabs.is_empty() { 0 } else { self.active_tab + 1 };
        self.tabs.insert(i, tab);
        self.active_tab = i;
        i
    }

    /// Inserts `tab` at `index` (clamped) without changing which tab is active (the first
    /// tab of an empty workspace becomes active). Returns its index.
    pub fn insert_tab(&mut self, index: usize, tab: Tab) -> usize {
        let i = index.min(self.tabs.len());
        let was_empty = self.tabs.is_empty();
        self.tabs.insert(i, tab);
        if was_empty {
            self.active_tab = 0;
        } else if i <= self.active_tab {
            self.active_tab += 1;
        }
        i
    }

    /// Removes the tab and returns it. When it was active, the tab that takes its index (the
    /// one to its right, or the new last tab) becomes active.
    pub fn take_tab(&mut self, id: TabId) -> Option<Tab> {
        let i = self.tab_index(id)?;
        let tab = self.tabs.remove(i);
        if i < self.active_tab {
            self.active_tab -= 1;
        }
        self.active_tab = self.active_tab.min(self.tabs.len().saturating_sub(1));
        Some(tab)
    }

    /// `⌘⇧W`: closes the tab. `None` if it does not exist.
    pub fn close_tab(&mut self, id: TabId) -> Option<WorkspaceAfterClose> {
        self.take_tab(id)?;
        Some(if self.tabs.is_empty() { WorkspaceAfterClose::Empty } else { WorkspaceAfterClose::Remaining })
    }

    /// Moves the tab at `from` to index `to` (clamped); the active tab stays active.
    pub fn move_tab(&mut self, from: usize, to: usize) -> bool {
        if from >= self.tabs.len() {
            return false;
        }
        let to = to.min(self.tabs.len() - 1);
        let active_id = self.active().map(|t| t.id);
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        if let Some(id) = active_id {
            self.active_tab = self.tab_index(id).unwrap_or(0);
        }
        from != to
    }

    /// `⌘⇧←`: moves the active tab one to the left, **wrapping** (the first tab becomes the
    /// last), consistent with tab navigation.
    pub fn move_tab_left(&mut self) -> bool {
        let n = self.tabs.len();
        n > 1 && self.move_tab(self.active_tab, (self.active_tab + n - 1) % n)
    }

    /// `⌘⇧→`: moves the active tab one to the right, wrapping (the last becomes the first).
    pub fn move_tab_right(&mut self) -> bool {
        let n = self.tabs.len();
        n > 1 && self.move_tab(self.active_tab, (self.active_tab + 1) % n)
    }

    /// `⌘1…⌘8`: activates tab `index` (0-based). False when out of range.
    pub fn select_tab(&mut self, index: usize) -> bool {
        if index < self.tabs.len() {
            self.active_tab = index;
            true
        } else {
            false
        }
    }

    /// `⌘9`.
    pub fn select_last(&mut self) -> bool {
        !self.tabs.is_empty() && self.select_tab(self.tabs.len() - 1)
    }

    /// `⌘⇧]`, wrapping.
    pub fn next_tab(&mut self) -> bool {
        let n = self.tabs.len();
        n > 0 && self.select_tab((self.active_tab + 1) % n)
    }

    /// `⌘⇧[`, wrapping.
    pub fn prev_tab(&mut self) -> bool {
        let n = self.tabs.len();
        n > 0 && self.select_tab((self.active_tab + n - 1) % n)
    }

    /// The workspace's status from its tabs' statuses.
    pub fn status(&self, tab_status: impl Fn(&Tab) -> TabStatus) -> TabStatus {
        aggregate_status(self.tabs.iter().map(tab_status))
    }
}

/// Outcome of [`Workspaces::close`]. The removed workspace is handed back so the caller can
/// shut down its panes.
#[derive(Clone, Debug, PartialEq)]
pub enum WorkspaceClose {
    /// Others remain; the most recently used one is now active.
    Closed { removed: Workspace },
    /// That was the last workspace: the list is now empty. The caller must quit or push a new
    /// workspace (`app.quit_when_last_tab_closed`).
    WasLast { removed: Workspace },
}

/// Outcome of [`Workspaces::move_tab_between`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MovedTab {
    /// Index of the tab in the target workspace.
    pub index: usize,
    /// The source workspace has no tabs left (close it, per `workspaces.close_when_empty`).
    pub source_empty: bool,
}

/// All workspaces of the window. There is always at least one, except transiently after
/// [`WorkspaceClose::WasLast`].
#[derive(Clone, Debug, PartialEq)]
pub struct Workspaces {
    pub list: Vec<Workspace>,
    /// Index of the active workspace.
    pub active: usize,
    /// Most recently used first, by id (robust to reordering).
    mru: Vec<WorkspaceId>,
}

impl Workspaces {
    #[must_use]
    pub fn new(first: Workspace) -> Self {
        let id = first.id;
        Self { list: vec![first], active: 0, mru: vec![id] }
    }

    /// Builds from a list (e.g. after restore). `active` is clamped.
    #[must_use]
    pub fn from_list(list: Vec<Workspace>, active: usize) -> Self {
        let active = active.min(list.len().saturating_sub(1));
        let first = list.get(active).map(|w| w.id);
        let mut mru: Vec<WorkspaceId> = first.into_iter().collect();
        mru.extend(list.iter().map(|w| w.id).filter(|id| Some(*id) != first));
        Self { list, active, mru }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.list.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    #[must_use]
    pub fn active(&self) -> &Workspace {
        &self.list[self.active]
    }

    pub fn active_mut(&mut self) -> &mut Workspace {
        &mut self.list[self.active]
    }

    #[must_use]
    pub fn index_of(&self, id: WorkspaceId) -> Option<usize> {
        self.list.iter().position(|w| w.id == id)
    }

    #[must_use]
    pub fn get(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.list.iter().find(|w| w.id == id)
    }

    pub fn get_mut(&mut self, id: WorkspaceId) -> Option<&mut Workspace> {
        self.list.iter_mut().find(|w| w.id == id)
    }

    /// Index of the previously active workspace (the `⌃⌘\` target).
    #[must_use]
    pub fn last_active(&self) -> Option<usize> {
        let cur = self.list.get(self.active)?.id;
        self.mru.iter().filter(|id| **id != cur).find_map(|id| self.index_of(*id))
    }

    fn touch(&mut self, id: WorkspaceId) {
        self.mru.retain(|x| *x != id);
        self.mru.insert(0, id);
    }

    /// Appends a workspace (not activated). Returns its index.
    pub fn push(&mut self, ws: Workspace) -> usize {
        self.mru.push(ws.id);
        self.list.push(ws);
        self.list.len() - 1
    }

    /// Appends a workspace and switches to it. Returns its index.
    pub fn push_and_switch(&mut self, ws: Workspace) -> usize {
        let i = self.push(ws);
        self.switch_to(i);
        i
    }

    /// Activates workspace `idx`, recording the previous one for [`Self::toggle_last`].
    pub fn switch_to(&mut self, idx: usize) -> bool {
        let Some(id) = self.list.get(idx).map(|w| w.id) else {
            return false;
        };
        if let Some(cur) = self.list.get(self.active).map(|w| w.id) {
            self.touch(cur);
        }
        self.touch(id);
        self.active = idx;
        true
    }

    /// `⌃⌘\`: back to the previously active workspace.
    pub fn toggle_last(&mut self) -> bool {
        match self.last_active() {
            Some(i) => self.switch_to(i),
            None => false,
        }
    }

    /// `⌃⌘]`, wrapping.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> bool {
        let n = self.list.len();
        n > 1 && self.switch_to((self.active + 1) % n)
    }

    /// `⌃⌘[`, wrapping.
    pub fn prev(&mut self) -> bool {
        let n = self.list.len();
        n > 1 && self.switch_to((self.active + n - 1) % n)
    }

    #[must_use]
    pub fn name_taken(&self, name: &str) -> bool {
        self.list.iter().any(|w| w.name == name)
    }

    /// `base`, or `base 2`, `base 3`, … — the first not taken.
    #[must_use]
    pub fn unique_name(&self, base: &str) -> String {
        if !self.name_taken(base) {
            return base.to_string();
        }
        (2..).map(|n| format!("{base} {n}")).find(|s| !self.name_taken(s)).expect("some name is free")
    }

    /// `workspace N` for a workspace without name or root: N = count + 1, bumped while taken.
    #[must_use]
    pub fn default_name(&self) -> String {
        (self.list.len() + 1..)
            .map(|n| format!("workspace {n}"))
            .find(|s| !self.name_taken(s))
            .expect("some name is free")
    }

    /// The first workspace rooted at `path` (compared as given; canonicalize beforehand).
    #[must_use]
    pub fn find_by_root(&self, path: &Path) -> Option<usize> {
        self.list.iter().position(|w| w.root.as_deref() == Some(path))
    }

    /// Closes workspace `idx`. If it was active, the most recently used remaining workspace
    /// becomes active. `None` if `idx` is out of range.
    pub fn close(&mut self, idx: usize) -> Option<WorkspaceClose> {
        if idx >= self.list.len() {
            return None;
        }
        let removed = self.list.remove(idx);
        self.mru.retain(|id| *id != removed.id);
        if self.list.is_empty() {
            self.active = 0;
            return Some(WorkspaceClose::WasLast { removed });
        }
        if idx == self.active {
            self.active = self.mru.iter().find_map(|id| self.index_of(*id)).unwrap_or(0);
            let id = self.list[self.active].id;
            self.touch(id);
        } else if idx < self.active {
            self.active -= 1;
        }
        Some(WorkspaceClose::Closed { removed })
    }

    /// Moves a workspace in the list (drag in switcher / sidebar); the active one stays active.
    pub fn reorder(&mut self, from: usize, to: usize) -> bool {
        if from >= self.list.len() {
            return false;
        }
        let to = to.min(self.list.len() - 1);
        let active_id = self.list[self.active].id;
        let ws = self.list.remove(from);
        self.list.insert(to, ws);
        self.active = self.index_of(active_id).unwrap_or(0);
        from != to
    }

    /// Moves tab `tab` from workspace `from_ws` to the end of workspace `to_ws`. The target's
    /// active tab does not change (unless it was empty). The source is left as is, even when
    /// empty: see [`MovedTab::source_empty`].
    pub fn move_tab_between(&mut self, from_ws: usize, tab: TabId, to_ws: usize) -> Option<MovedTab> {
        if from_ws == to_ws || to_ws >= self.list.len() {
            return None;
        }
        let t = self.list.get_mut(from_ws)?.take_tab(tab)?;
        let source_empty = self.list[from_ws].is_empty();
        let target = &mut self.list[to_ws];
        let index = target.insert_tab(target.len(), t);
        Some(MovedTab { index, source_empty })
    }

    /// (workspace index, tab index) of the tab holding `pane`.
    #[must_use]
    pub fn find_pane(&self, pane: PaneId) -> Option<(usize, usize)> {
        self.list.iter().enumerate().find_map(|(wi, w)| w.find_pane(pane).map(|ti| (wi, ti)))
    }

    /// (workspace index, tab index) of a tab.
    #[must_use]
    pub fn find_tab(&self, tab: TabId) -> Option<(usize, usize)> {
        self.list.iter().enumerate().find_map(|(wi, w)| w.tab_index(tab).map(|ti| (wi, ti)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(id: u64, name: &str) -> Workspace {
        Workspace::new(WorkspaceId(id), name, None).with_tab(Tab::new(TabId(id * 100), PaneId(id * 100)))
    }

    fn tab(n: u64) -> Tab {
        Tab::new(TabId(n), PaneId(n))
    }

    fn ids(w: &Workspace) -> Vec<u64> {
        w.tabs.iter().map(|t| t.id.0).collect()
    }

    #[test]
    fn status_priority() {
        use TabStatus::*;
        assert_eq!(aggregate_status([]), None);
        assert_eq!(aggregate_status([Done, None]), Done);
        assert_eq!(aggregate_status([Done, Running]), Running);
        assert_eq!(aggregate_status([Running, NeedsInput, Done]), NeedsInput);
        assert_eq!(aggregate_status([NeedsInput, Bell, Running]), Bell);
    }

    #[test]
    fn add_after_active_and_close() {
        let mut w = Workspace::new(WorkspaceId(1), "w", None);
        assert!(w.active().is_none());
        w.add_tab_after_active(tab(1));
        w.add_tab_after_active(tab(2));
        w.select_tab(0);
        w.add_tab_after_active(tab(3));
        assert_eq!(ids(&w), vec![1, 3, 2]);
        assert_eq!(w.active_tab, 1);
        // Closing the active tab activates the one to its right.
        assert_eq!(w.close_tab(TabId(3)), Some(WorkspaceAfterClose::Remaining));
        assert_eq!(w.active().unwrap().id, TabId(2));
        // Closing a tab before the active one keeps the same active tab.
        assert_eq!(w.close_tab(TabId(1)), Some(WorkspaceAfterClose::Remaining));
        assert_eq!(w.active().unwrap().id, TabId(2));
        assert_eq!(w.close_tab(TabId(9)), None);
        assert_eq!(w.close_tab(TabId(2)), Some(WorkspaceAfterClose::Empty));
        assert!(w.is_empty());
    }

    #[test]
    fn close_last_active_tab_selects_new_last() {
        let mut w = Workspace::new(WorkspaceId(1), "w", None);
        for n in 1..=3 {
            w.add_tab_after_active(tab(n));
        }
        assert_eq!(w.active_tab, 2);
        w.close_tab(TabId(3));
        assert_eq!(w.active().unwrap().id, TabId(2));
    }

    #[test]
    fn move_and_select_tabs() {
        let mut w = Workspace::new(WorkspaceId(1), "w", None);
        for n in 1..=4 {
            w.add_tab_after_active(tab(n));
        }
        w.select_tab(1);
        assert!(w.move_tab(1, 3));
        assert_eq!(ids(&w), vec![1, 3, 4, 2]);
        assert_eq!(w.active().unwrap().id, TabId(2));
        assert!(w.move_tab_right()); // wraps to the front
        assert_eq!(ids(&w), vec![2, 1, 3, 4]);
        assert_eq!(w.active_tab, 0);
        assert!(w.move_tab_left()); // wraps to the back
        assert_eq!(ids(&w), vec![1, 3, 4, 2]);
        assert!(w.move_tab_left());
        assert_eq!(ids(&w), vec![1, 3, 2, 4]);
        assert_eq!(w.active().unwrap().id, TabId(2));

        assert!(w.select_last());
        assert_eq!(w.active_tab, 3);
        assert!(w.next_tab());
        assert_eq!(w.active_tab, 0);
        assert!(w.prev_tab());
        assert_eq!(w.active_tab, 3);
        assert!(!w.select_tab(4));
        assert!(!w.move_tab(9, 0));
    }

    #[test]
    fn insert_keeps_active() {
        let mut w = Workspace::new(WorkspaceId(1), "w", None);
        w.add_tab_after_active(tab(1));
        w.add_tab_after_active(tab(2));
        assert_eq!(w.insert_tab(0, tab(3)), 0);
        assert_eq!(w.active().unwrap().id, TabId(2));
        assert_eq!(w.insert_tab(99, tab(4)), 3);
        assert_eq!(w.active().unwrap().id, TabId(2));
    }

    #[test]
    fn unique_and_default_names() {
        let mut all = Workspaces::new(ws(1, "nox"));
        assert_eq!(all.unique_name("kayet"), "kayet");
        assert_eq!(all.unique_name("nox"), "nox 2");
        all.push(ws(2, "nox 2"));
        assert_eq!(all.unique_name("nox"), "nox 3");
        assert_eq!(all.default_name(), "workspace 3");
        all.push(ws(3, "workspace 4"));
        assert_eq!(all.default_name(), "workspace 5");
    }

    #[test]
    fn switch_toggle_and_mru_close() {
        let mut all = Workspaces::new(ws(1, "a"));
        all.push(ws(2, "b"));
        all.push(ws(3, "c"));
        assert_eq!(all.last_active(), Some(1));
        assert!(all.switch_to(2));
        assert_eq!(all.last_active(), Some(0));
        assert!(all.toggle_last());
        assert_eq!(all.active, 0);
        assert!(all.toggle_last());
        assert_eq!(all.active, 2);
        assert!(all.next());
        assert_eq!(all.active, 0);
        assert!(all.prev());
        assert_eq!(all.active, 2);
        assert!(!all.switch_to(7));

        // MRU now: c, a, b. Switch to b → MRU b, c, a. Closing b activates c.
        all.switch_to(1);
        let WorkspaceClose::Closed { removed } = all.close(1).unwrap() else { panic!() };
        assert_eq!(removed.name, "b");
        assert_eq!(all.active().name, "c");
        assert_eq!(all.last_active().map(|i| all.list[i].name.clone()), Some("a".into()));

        // Closing a non-active workspace before the active one keeps it active.
        all.close(0);
        assert_eq!(all.active().name, "c");
        assert_eq!(all.last_active(), None);
        assert!(matches!(all.close(0), Some(WorkspaceClose::WasLast { .. })));
        assert!(all.is_empty());
        assert!(all.close(0).is_none());
    }

    #[test]
    fn reorder_keeps_active() {
        let mut all = Workspaces::new(ws(1, "a"));
        all.push(ws(2, "b"));
        all.push(ws(3, "c"));
        all.switch_to(1);
        assert!(all.reorder(1, 0));
        assert_eq!(all.active, 0);
        assert_eq!(all.active().name, "b");
        assert!(all.reorder(2, 0));
        assert_eq!(all.active().name, "b");
        assert_eq!(all.active, 1);
        // MRU survives reorder: last active is still "a".
        assert_eq!(all.list[all.last_active().unwrap()].name, "a");
    }

    #[test]
    fn move_tab_between_workspaces() {
        let mut all = Workspaces::new(ws(1, "a"));
        all.push(ws(2, "b"));
        all.list[0].add_tab_after_active(tab(7));
        let moved = all.move_tab_between(0, TabId(7), 1).unwrap();
        assert_eq!(moved, MovedTab { index: 1, source_empty: false });
        assert_eq!(all.find_tab(TabId(7)), Some((1, 1)));
        assert_eq!(all.find_pane(PaneId(7)), Some((1, 1)));
        // The target's active tab didn't change.
        assert_eq!(all.list[1].active().unwrap().id, TabId(200));
        // Moving the source's last tab reports it empty.
        let moved = all.move_tab_between(0, TabId(100), 1).unwrap();
        assert!(moved.source_empty);
        assert!(all.list[0].is_empty());
        assert!(all.move_tab_between(0, TabId(100), 1).is_none());
        assert!(all.move_tab_between(1, TabId(100), 1).is_none());
        assert_eq!(all.find_pane(PaneId(999)), None);
    }

    #[test]
    fn find_by_root() {
        let mut all = Workspaces::new(ws(1, "a"));
        let mut b = ws(2, "nox");
        b.root = Some(PathBuf::from("/src/nox"));
        all.push(b);
        assert_eq!(all.find_by_root(Path::new("/src/nox")), Some(1));
        assert_eq!(all.find_by_root(Path::new("/src")), None);
    }

    #[test]
    fn workspace_status() {
        let w = ws(1, "a").with_tab(tab(5));
        let s = w.status(|t| if t.id == TabId(5) { TabStatus::NeedsInput } else { TabStatus::Running });
        assert_eq!(s, TabStatus::NeedsInput);
    }
}
