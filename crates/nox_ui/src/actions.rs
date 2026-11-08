//! Every command is a GPUI action (SPEC §5.6, §10): the palette, the menu bar, context menus
//! and key bindings all dispatch the same actions. Default shortcuts live here; `[keys]` in
//! the config rebinds or unbinds them.

use std::collections::BTreeMap;

use gpui::{Action, App, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType, actions};
use nox_config::KeyBinding as CfgKey;

actions!(
    nox,
    [
        // Tabs.
        NewTab,
        CloseTab,
        ClosePane,
        ActivateTab1,
        ActivateTab2,
        ActivateTab3,
        ActivateTab4,
        ActivateTab5,
        ActivateTab6,
        ActivateTab7,
        ActivateTab8,
        ActivateLastTab,
        PreviousTab,
        NextTab,
        MoveTabLeft,
        MoveTabRight,
        RenameTab,
        DuplicateTab,
        CloseOtherTabs,
        SwitchTab,
        // Panes.
        SplitVertical,
        SplitHorizontal,
        FocusPaneLeft,
        FocusPaneRight,
        FocusPaneUp,
        FocusPaneDown,
        PreviousPane,
        NextPane,
        ResizePaneLeft,
        ResizePaneRight,
        ResizePaneUp,
        ResizePaneDown,
        EqualizePanes,
        ToggleMaximizePane,
        RenamePane,
        // Workspaces.
        NewWorkspace,
        OpenFolderAsWorkspace,
        SwitchWorkspace,
        ActivateWorkspace1,
        ActivateWorkspace2,
        ActivateWorkspace3,
        ActivateWorkspace4,
        ActivateWorkspace5,
        ActivateWorkspace6,
        ActivateWorkspace7,
        ActivateWorkspace8,
        ActivateWorkspace9,
        PreviousWorkspace,
        NextWorkspace,
        LastWorkspace,
        CloseWorkspace,
        RenameWorkspace,
        SetWorkspaceColor,
        // Agents.
        NewAgentTab,
        NewAgentInWorktree,
        AgentsPanel,
        NextAgent,
        CopyLastOutput,
        SendSelectionTo,
        InstallAgentHooks,
        CopyMcpSetupCommand,
        ResumeAllAgents,
        InstallCommandLineTool,
        // Terminal.
        Copy,
        Paste,
        SelectAll,
        Find,
        FindNext,
        FindPrevious,
        ClearBuffer,
        ScrollPageUp,
        ScrollPageDown,
        ScrollToTop,
        ScrollToBottom,
        PreviousPrompt,
        NextPrompt,
        RestartPane,
        SendShiftEnter,
        // View / app.
        CommandPalette,
        ToggleSidebar,
        FontBigger,
        FontSmaller,
        FontReset,
        SwitchTheme,
        SwitchAppearance,
        TabsPositionTop,
        TabsPositionBottom,
        TabsPositionLeft,
        OpenSettings,
        OpenConfigFile,
        ToggleFullScreen,
        Minimize,
        Zoom,
        Quit,
        About,
        HideApp,
        HideOthers,
        ShowAll,
        CheckForUpdates,
    ]
);

/// Key contexts.
pub const ROOT: &str = "NoxRoot";
pub const TERMINAL: &str = "Terminal";

/// Builds a key binding for action `A`.
pub type BindFn = fn(&str, Option<&str>) -> KeyBinding;

fn bind<A: Action + Default>(keys: &str, ctx: Option<&str>) -> KeyBinding {
    KeyBinding::new(keys, A::default(), ctx)
}

/// `(config key, binding constructor, default shortcut, context)`. The config key is what
/// `[keys]` uses.
#[must_use]
pub fn default_bindings() -> Vec<(&'static str, BindFn, &'static str, Option<&'static str>)> {
    let r = Some(ROOT);
    let t = Some(TERMINAL);
    vec![
        ("new_tab", bind::<NewTab> as BindFn, "cmd-t", r),
        ("close_pane", bind::<ClosePane> as BindFn, "cmd-w", r),
        ("close_tab", bind::<CloseTab> as BindFn, "cmd-shift-w", r),
        ("tab_1", bind::<ActivateTab1> as BindFn, "cmd-1", r),
        ("tab_2", bind::<ActivateTab2> as BindFn, "cmd-2", r),
        ("tab_3", bind::<ActivateTab3> as BindFn, "cmd-3", r),
        ("tab_4", bind::<ActivateTab4> as BindFn, "cmd-4", r),
        ("tab_5", bind::<ActivateTab5> as BindFn, "cmd-5", r),
        ("tab_6", bind::<ActivateTab6> as BindFn, "cmd-6", r),
        ("tab_7", bind::<ActivateTab7> as BindFn, "cmd-7", r),
        ("tab_8", bind::<ActivateTab8> as BindFn, "cmd-8", r),
        ("last_tab", bind::<ActivateLastTab> as BindFn, "cmd-9", r),
        ("previous_tab", bind::<PreviousTab> as BindFn, "cmd-shift-[", r),
        ("next_tab", bind::<NextTab> as BindFn, "cmd-shift-]", r),
        ("previous_tab_alt", bind::<PreviousTab> as BindFn, "ctrl-shift-tab", r),
        ("next_tab_alt", bind::<NextTab> as BindFn, "ctrl-tab", r),
        ("move_tab_left", bind::<MoveTabLeft> as BindFn, "cmd-shift-left", r),
        ("move_tab_right", bind::<MoveTabRight> as BindFn, "cmd-shift-right", r),
        ("rename_tab", bind::<RenameTab> as BindFn, "cmd-shift-i", r),
        ("split_vertical", bind::<SplitVertical> as BindFn, "cmd-d", r),
        ("split_horizontal", bind::<SplitHorizontal> as BindFn, "cmd-shift-d", r),
        ("focus_pane_left", bind::<FocusPaneLeft> as BindFn, "cmd-alt-left", r),
        ("focus_pane_right", bind::<FocusPaneRight> as BindFn, "cmd-alt-right", r),
        ("focus_pane_up", bind::<FocusPaneUp> as BindFn, "cmd-alt-up", r),
        ("focus_pane_down", bind::<FocusPaneDown> as BindFn, "cmd-alt-down", r),
        ("previous_pane", bind::<PreviousPane> as BindFn, "cmd-[", r),
        ("next_pane", bind::<NextPane> as BindFn, "cmd-]", r),
        ("resize_pane_left", bind::<ResizePaneLeft> as BindFn, "cmd-ctrl-left", r),
        ("resize_pane_right", bind::<ResizePaneRight> as BindFn, "cmd-ctrl-right", r),
        ("resize_pane_up", bind::<ResizePaneUp> as BindFn, "cmd-ctrl-up", r),
        ("resize_pane_down", bind::<ResizePaneDown> as BindFn, "cmd-ctrl-down", r),
        ("equalize_panes", bind::<EqualizePanes> as BindFn, "cmd-ctrl-=", r),
        ("maximize_pane", bind::<ToggleMaximizePane> as BindFn, "cmd-shift-enter", r),
        ("toggle_sidebar", bind::<ToggleSidebar> as BindFn, "cmd-\\", r),
        ("new_workspace", bind::<NewWorkspace> as BindFn, "cmd-shift-n", r),
        ("open_folder", bind::<OpenFolderAsWorkspace> as BindFn, "cmd-o", r),
        ("switch_workspace", bind::<SwitchWorkspace> as BindFn, "cmd-p", r),
        ("workspace_1", bind::<ActivateWorkspace1> as BindFn, "ctrl-cmd-1", r),
        ("workspace_2", bind::<ActivateWorkspace2> as BindFn, "ctrl-cmd-2", r),
        ("workspace_3", bind::<ActivateWorkspace3> as BindFn, "ctrl-cmd-3", r),
        ("workspace_4", bind::<ActivateWorkspace4> as BindFn, "ctrl-cmd-4", r),
        ("workspace_5", bind::<ActivateWorkspace5> as BindFn, "ctrl-cmd-5", r),
        ("workspace_6", bind::<ActivateWorkspace6> as BindFn, "ctrl-cmd-6", r),
        ("workspace_7", bind::<ActivateWorkspace7> as BindFn, "ctrl-cmd-7", r),
        ("workspace_8", bind::<ActivateWorkspace8> as BindFn, "ctrl-cmd-8", r),
        ("workspace_9", bind::<ActivateWorkspace9> as BindFn, "ctrl-cmd-9", r),
        ("previous_workspace", bind::<PreviousWorkspace> as BindFn, "ctrl-cmd-[", r),
        ("next_workspace", bind::<NextWorkspace> as BindFn, "ctrl-cmd-]", r),
        ("last_workspace", bind::<LastWorkspace> as BindFn, "ctrl-cmd-\\", r),
        ("close_workspace", bind::<CloseWorkspace> as BindFn, "ctrl-cmd-w", r),
        ("new_agent_tab", bind::<NewAgentTab> as BindFn, "alt-cmd-t", r),
        ("agents_panel", bind::<AgentsPanel> as BindFn, "cmd-shift-a", r),
        ("next_agent", bind::<NextAgent> as BindFn, "cmd-j", r),
        ("copy_last_output", bind::<CopyLastOutput> as BindFn, "alt-cmd-c", t),
        ("send_selection", bind::<SendSelectionTo> as BindFn, "cmd-shift-s", r),
        ("command_palette", bind::<CommandPalette> as BindFn, "cmd-k", r),
        ("find", bind::<Find> as BindFn, "cmd-f", t),
        ("find_next", bind::<FindNext> as BindFn, "cmd-g", t),
        ("find_previous", bind::<FindPrevious> as BindFn, "cmd-shift-g", t),
        ("clear_buffer", bind::<ClearBuffer> as BindFn, "cmd-shift-k", t),
        ("copy", bind::<Copy> as BindFn, "cmd-c", t),
        ("paste", bind::<Paste> as BindFn, "cmd-v", t),
        ("select_all", bind::<SelectAll> as BindFn, "cmd-a", t),
        ("font_bigger", bind::<FontBigger> as BindFn, "cmd-=", r),
        ("font_bigger_plus", bind::<FontBigger> as BindFn, "cmd-+", r),
        ("font_smaller", bind::<FontSmaller> as BindFn, "cmd--", r),
        ("font_reset", bind::<FontReset> as BindFn, "cmd-0", r),
        ("previous_prompt", bind::<PreviousPrompt> as BindFn, "cmd-up", t),
        ("next_prompt", bind::<NextPrompt> as BindFn, "cmd-down", t),
        ("page_up", bind::<ScrollPageUp> as BindFn, "shift-pageup", t),
        ("page_down", bind::<ScrollPageDown> as BindFn, "shift-pagedown", t),
        ("scroll_top", bind::<ScrollToTop> as BindFn, "cmd-home", t),
        ("scroll_bottom", bind::<ScrollToBottom> as BindFn, "cmd-end", t),
        ("restart", bind::<RestartPane> as BindFn, "cmd-r", t),
        ("shift-enter", bind::<SendShiftEnter> as BindFn, "shift-enter", t),
        ("switch_appearance", bind::<SwitchAppearance> as BindFn, "cmd-shift-l", r),
        ("settings", bind::<OpenSettings> as BindFn, "cmd-,", None),
        ("open_config", bind::<OpenConfigFile> as BindFn, "alt-cmd-,", None),
        ("full_screen", bind::<ToggleFullScreen> as BindFn, "ctrl-cmd-f", None),
        ("quit", bind::<Quit> as BindFn, "cmd-q", None),
        ("hide", bind::<HideApp> as BindFn, "cmd-h", None),
        ("hide_others", bind::<HideOthers> as BindFn, "alt-cmd-h", None),
        ("minimize", bind::<Minimize> as BindFn, "cmd-m", None),
    ]
}

/// A `[keys]` entry that maps a key to bytes written to the PTY.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = nox, no_json)]
pub struct SendText(pub String);

/// Records a notice for `keys.<key>` (ending in `outcome`) and returns `true` when `shortcut`
/// doesn't parse (`KeyBinding::new` would panic on it).
fn invalid(key: &str, shortcut: &str, outcome: &str, notices: &mut Vec<nox_config::Notice>) -> bool {
    let Some(e) = shortcut.split_whitespace().find_map(|k| gpui::Keystroke::parse(k).err()) else {
        return false;
    };
    notices.push(nox_config::Notice {
        key: format!("keys.{key}"),
        message: format!("invalid shortcut \"{}\", {outcome}", e.keystroke),
    });
    true
}

/// Install the key bindings: defaults, overridden / unbound by `[keys]`. An override that
/// doesn't parse falls back to the action's default, a `{ send = … }` entry whose key doesn't
/// parse is skipped; both are returned as notices (SPEC §11), so a typo in a live-reloaded
/// config can't take the app down.
pub fn bind_keys(cx: &mut App, keys: &BTreeMap<String, CfgKey>) -> Vec<nox_config::Notice> {
    cx.clear_key_bindings();
    crate::text_input::bind_keys(cx);
    crate::palette::bind_keys(cx);
    crate::dialog::bind_keys(cx);
    let mut bindings = Vec::new();
    let mut notices = Vec::new();
    for (name, make, default, ctx) in default_bindings() {
        let shortcut = match keys.get(name) {
            Some(CfgKey::Shortcut(s)) if invalid(name, s, "using the default", &mut notices) => {
                default.to_string()
            }
            Some(CfgKey::Shortcut(s)) => s.clone(),
            Some(CfgKey::Send { .. }) => continue,
            None => default.to_string(),
        };
        if shortcut.trim().is_empty() {
            continue; // unbound: the key goes to the program
        }
        bindings.push(make(&shortcut, ctx));
    }
    for (key, b) in keys {
        if let CfgKey::Send { send } = b {
            // Key names here are shortcuts like "alt-left".
            if key.trim().is_empty() || invalid(key, key, "ignored", &mut notices) {
                continue;
            }
            bindings.push(KeyBinding::new(key, SendText(send.clone()), Some(TERMINAL)));
        }
    }
    cx.bind_keys(bindings);
    notices
}

/// The native menu bar (SPEC §10): nox, Shell, Edit, View, Window, Help.
#[must_use]
pub fn menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "nox".into(),
            items: vec![
                MenuItem::action("About nox", About),
                MenuItem::separator(),
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::action("Open Config File", OpenConfigFile),
                MenuItem::action("Install Command Line Tool…", InstallCommandLineTool),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Hide nox", HideApp),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit nox", Quit),
            ],
        },
        Menu {
            name: "Shell".into(),
            items: vec![
                MenuItem::action("New Tab", NewTab),
                MenuItem::action("New Agent Tab…", NewAgentTab),
                MenuItem::action("New Workspace…", NewWorkspace),
                MenuItem::action("Open Folder as Workspace…", OpenFolderAsWorkspace),
                MenuItem::action("New Agent in Worktree…", NewAgentInWorktree),
                MenuItem::separator(),
                MenuItem::action("Split Vertically", SplitVertical),
                MenuItem::action("Split Horizontally", SplitHorizontal),
                MenuItem::separator(),
                MenuItem::action("Rename Tab…", RenameTab),
                MenuItem::action("Rename Pane…", RenamePane),
                MenuItem::action("Duplicate Tab", DuplicateTab),
                MenuItem::separator(),
                MenuItem::action("Close Pane", ClosePane),
                MenuItem::action("Close Tab", CloseTab),
                MenuItem::action("Close Workspace", CloseWorkspace),
                MenuItem::separator(),
                MenuItem::action("Restart Exited Pane", RestartPane),
                MenuItem::action("Resume All Agents", ResumeAllAgents),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Copy", Copy, OsAction::Copy),
                MenuItem::os_action("Paste", Paste, OsAction::Paste),
                MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
                MenuItem::separator(),
                MenuItem::action("Copy Last Output", CopyLastOutput),
                MenuItem::action("Send Selection to…", SendSelectionTo),
                MenuItem::separator(),
                MenuItem::action("Find…", Find),
                MenuItem::action("Find Next", FindNext),
                MenuItem::action("Find Previous", FindPrevious),
                MenuItem::separator(),
                MenuItem::action("Clear Buffer", ClearBuffer),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Command Palette", CommandPalette),
                MenuItem::action("Agents", AgentsPanel),
                MenuItem::action("Next Agent Needing Attention", NextAgent),
                MenuItem::action("Toggle Sidebar", ToggleSidebar),
                MenuItem::separator(),
                MenuItem::action("Bigger", FontBigger),
                MenuItem::action("Smaller", FontSmaller),
                MenuItem::action("Actual Size", FontReset),
                MenuItem::separator(),
                MenuItem::action("Switch Theme…", SwitchTheme),
                MenuItem::action("Switch Appearance", SwitchAppearance),
                MenuItem::action("Tabs on Top", TabsPositionTop),
                MenuItem::action("Tabs on Bottom", TabsPositionBottom),
                MenuItem::action("Tabs in Sidebar", TabsPositionLeft),
                MenuItem::separator(),
                MenuItem::action("Enter Full Screen", ToggleFullScreen),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
                MenuItem::separator(),
                MenuItem::action("Previous Tab", PreviousTab),
                MenuItem::action("Next Tab", NextTab),
                MenuItem::action("Switch Tab…", SwitchTab),
                MenuItem::separator(),
                MenuItem::action("Previous Pane", PreviousPane),
                MenuItem::action("Next Pane", NextPane),
                MenuItem::action("Equalize Panes", EqualizePanes),
                MenuItem::action("Maximize Pane", ToggleMaximizePane),
                MenuItem::separator(),
                MenuItem::action("Switch Workspace…", SwitchWorkspace),
                MenuItem::action("Previous Workspace", PreviousWorkspace),
                MenuItem::action("Next Workspace", NextWorkspace),
                MenuItem::action("Last Used Workspace", LastWorkspace),
            ],
        },
        Menu {
            name: "Help".into(),
            items: vec![
                MenuItem::action("Install Agent Hooks…", InstallAgentHooks),
                MenuItem::action("Copy MCP Setup Command", CopyMcpSetupCommand),
            ],
        },
    ]
}

/// The palette's command list: (label, action). Shortcuts are looked up from the keymap.
#[must_use]
pub fn palette_commands() -> Vec<(&'static str, Box<dyn Action>)> {
    vec![
        ("New Tab", Box::new(NewTab)),
        ("New Agent Tab…", Box::new(NewAgentTab)),
        ("Close Pane", Box::new(ClosePane)),
        ("Close Tab", Box::new(CloseTab)),
        ("Close Other Tabs", Box::new(CloseOtherTabs)),
        ("Duplicate Tab", Box::new(DuplicateTab)),
        ("Rename Tab…", Box::new(RenameTab)),
        ("Rename Pane…", Box::new(RenamePane)),
        ("Switch Tab…", Box::new(SwitchTab)),
        ("Previous Tab", Box::new(PreviousTab)),
        ("Next Tab", Box::new(NextTab)),
        ("Move Tab Left", Box::new(MoveTabLeft)),
        ("Move Tab Right", Box::new(MoveTabRight)),
        ("Split Vertically", Box::new(SplitVertical)),
        ("Split Horizontally", Box::new(SplitHorizontal)),
        ("Maximize / Restore Pane", Box::new(ToggleMaximizePane)),
        ("Equalize Panes", Box::new(EqualizePanes)),
        ("Previous Pane", Box::new(PreviousPane)),
        ("Next Pane", Box::new(NextPane)),
        ("Focus Pane Left", Box::new(FocusPaneLeft)),
        ("Focus Pane Right", Box::new(FocusPaneRight)),
        ("Focus Pane Up", Box::new(FocusPaneUp)),
        ("Focus Pane Down", Box::new(FocusPaneDown)),
        ("New Workspace…", Box::new(NewWorkspace)),
        ("Open Folder as Workspace…", Box::new(OpenFolderAsWorkspace)),
        ("Switch Workspace…", Box::new(SwitchWorkspace)),
        ("Rename Workspace…", Box::new(RenameWorkspace)),
        ("Set Workspace Color…", Box::new(SetWorkspaceColor)),
        ("Previous Workspace", Box::new(PreviousWorkspace)),
        ("Next Workspace", Box::new(NextWorkspace)),
        ("Last Used Workspace", Box::new(LastWorkspace)),
        ("Close Workspace", Box::new(CloseWorkspace)),
        ("New Agent in Worktree…", Box::new(NewAgentInWorktree)),
        ("Agents Panel", Box::new(AgentsPanel)),
        ("Next Agent Needing Attention", Box::new(NextAgent)),
        ("Resume All Agents", Box::new(ResumeAllAgents)),
        ("Copy Last Output", Box::new(CopyLastOutput)),
        ("Send Selection to…", Box::new(SendSelectionTo)),
        ("Install Agent Hooks…", Box::new(InstallAgentHooks)),
        ("Copy MCP Setup Command", Box::new(CopyMcpSetupCommand)),
        ("Install Command Line Tool…", Box::new(InstallCommandLineTool)),
        ("Find…", Box::new(Find)),
        ("Clear Buffer", Box::new(ClearBuffer)),
        ("Scroll to Top", Box::new(ScrollToTop)),
        ("Scroll to Bottom", Box::new(ScrollToBottom)),
        ("Previous Prompt", Box::new(PreviousPrompt)),
        ("Next Prompt", Box::new(NextPrompt)),
        ("Restart Exited Pane", Box::new(RestartPane)),
        ("Switch Theme…", Box::new(SwitchTheme)),
        ("Switch Appearance…", Box::new(SwitchAppearance)),
        ("Tabs Position: Top", Box::new(TabsPositionTop)),
        ("Tabs Position: Bottom", Box::new(TabsPositionBottom)),
        ("Tabs Position: Left", Box::new(TabsPositionLeft)),
        ("Toggle Sidebar", Box::new(ToggleSidebar)),
        ("Font Bigger", Box::new(FontBigger)),
        ("Font Smaller", Box::new(FontSmaller)),
        ("Font Reset", Box::new(FontReset)),
        ("Settings…", Box::new(OpenSettings)),
        ("Open config.toml", Box::new(OpenConfigFile)),
        ("Toggle Full Screen", Box::new(ToggleFullScreen)),
        ("Quit", Box::new(Quit)),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use gpui::TestAppContext;

    use super::*;
    use crate::util_keys::keystroke_to_config;

    /// The shortcuts bound to `action`, as config text.
    fn shortcuts(cx: &App, action: &dyn Action) -> Vec<String> {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        keymap
            .bindings_for_action(action)
            .map(|b| {
                b.keystrokes().iter().map(|k| keystroke_to_config(k.inner())).collect::<Vec<_>>().join(" ")
            })
            .collect()
    }

    #[test]
    fn default_bindings_are_unique_and_parse() {
        let defaults = default_bindings();
        let mut names = HashSet::new();
        let mut keys = HashSet::new();
        for (name, make, shortcut, ctx) in &defaults {
            assert!(names.insert(*name), "duplicate config key {name}");
            // Each shortcut is used once per context (conflicts are the user's doing).
            assert!(keys.insert((*shortcut, *ctx)), "{shortcut} bound twice in {ctx:?}");
            let b = make(shortcut, *ctx);
            assert!(!b.keystrokes().is_empty());
            assert!(matches!(*ctx, None | Some(ROOT | TERMINAL)), "{name}");
        }
        let get = |n: &str| defaults.iter().find(|d| d.0 == n).map(|d| d.2);
        // SPEC §10 spot checks.
        assert_eq!(get("new_tab"), Some("cmd-t"));
        assert_eq!(get("split_vertical"), Some("cmd-d"));
        assert_eq!(get("command_palette"), Some("cmd-k"));
        assert_eq!(get("settings"), Some("cmd-,"));
    }

    #[gpui::test]
    fn bind_keys_defaults(cx: &mut TestAppContext) {
        cx.update(|cx| {
            bind_keys(cx, &BTreeMap::new());
            assert_eq!(shortcuts(cx, &NewTab), ["cmd-t"]);
            assert_eq!(shortcuts(cx, &FontBigger), ["cmd-=", "cmd-+"]);
            assert_eq!(shortcuts(cx, &PreviousTab), ["shift-cmd-[", "ctrl-shift-tab"]);
            // The text input, palette and dialog bindings are installed too.
            assert_eq!(shortcuts(cx, &crate::text_input::Undo), ["cmd-z"]);
            assert_eq!(shortcuts(cx, &crate::palette::SelectNext), ["down", "ctrl-n"]);
            assert_eq!(shortcuts(cx, &crate::dialog::DialogCancel), ["escape"]);
        });
    }

    #[gpui::test]
    fn config_rebinds_unbinds_and_sends(cx: &mut TestAppContext) {
        let mut keys = BTreeMap::new();
        keys.insert("new_tab".to_string(), CfgKey::Shortcut("ctrl-alt-t".into()));
        keys.insert("split_vertical".to_string(), CfgKey::Shortcut(String::new()));
        keys.insert("find".to_string(), CfgKey::Shortcut("   ".into()));
        // `{ send = … }` on an action name drops that action's default binding …
        keys.insert("close_pane".to_string(), CfgKey::Send { send: "x".into() });
        // … and on any key sends the text to the PTY.
        keys.insert("alt-left".to_string(), CfgKey::Send { send: "\x1bb".into() });
        cx.update(|cx| {
            bind_keys(cx, &keys);
            assert_eq!(shortcuts(cx, &NewTab), ["ctrl-alt-t"]);
            assert!(shortcuts(cx, &SplitVertical).is_empty());
            assert!(shortcuts(cx, &Find).is_empty());
            assert!(shortcuts(cx, &ClosePane).is_empty());
            assert_eq!(shortcuts(cx, &SendText("\x1bb".into())), ["alt-left"]);
            // Rebinding again starts from scratch (no stale bindings).
            bind_keys(cx, &BTreeMap::new());
            assert_eq!(shortcuts(cx, &NewTab), ["cmd-t"]);
            assert!(shortcuts(cx, &SendText("\x1bb".into())).is_empty());
        });
    }

    #[gpui::test]
    fn invalid_shortcuts_fall_back_with_a_notice(cx: &mut TestAppContext) {
        let mut keys = BTreeMap::new();
        keys.insert("new_tab".to_string(), CfgKey::Shortcut("hyper-k".into()));
        keys.insert("find".to_string(), CfgKey::Shortcut("cmd-k foo-x".into()));
        keys.insert("bogus-left".to_string(), CfgKey::Send { send: "x".into() });
        keys.insert("alt-left".to_string(), CfgKey::Send { send: "\x1bb".into() });
        cx.update(|cx| {
            let notices = bind_keys(cx, &keys);
            let got: Vec<_> = notices.iter().map(|n| (n.key.as_str(), n.message.as_str())).collect();
            assert_eq!(
                got,
                [
                    ("keys.new_tab", "invalid shortcut \"hyper-k\", using the default"),
                    ("keys.find", "invalid shortcut \"foo-x\", using the default"),
                    ("keys.bogus-left", "invalid shortcut \"bogus-left\", ignored"),
                ]
            );
            // A bad override falls back to the default; a bad send entry is skipped.
            assert_eq!(shortcuts(cx, &NewTab), ["cmd-t"]);
            assert_eq!(shortcuts(cx, &Find), ["cmd-f"]);
            assert!(shortcuts(cx, &SendText("x".into())).is_empty());
            // Valid entries and untouched defaults still bind.
            assert_eq!(shortcuts(cx, &SendText("\x1bb".into())), ["alt-left"]);
            assert_eq!(shortcuts(cx, &SplitVertical), ["cmd-d"]);
            assert!(bind_keys(cx, &BTreeMap::new()).is_empty());
        });
    }

    #[gpui::test]
    fn bindings_dispatch_in_their_context(cx: &mut TestAppContext) {
        struct Host {
            focus: gpui::FocusHandle,
            hits: Vec<&'static str>,
        }
        impl gpui::Render for Host {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                use gpui::{InteractiveElement as _, ParentElement as _};
                gpui::div()
                    .key_context(ROOT)
                    .on_action(cx.listener(|h: &mut Host, _: &NewTab, _, _| h.hits.push("new_tab")))
                    .on_action(cx.listener(|h: &mut Host, _: &Find, _, _| h.hits.push("find")))
                    .child(gpui::div().key_context(TERMINAL).track_focus(&self.focus).on_action(cx.listener(
                        |h: &mut Host, a: &SendText, _, _| {
                            h.hits.push(if a.0 == "hi" { "send" } else { "?" });
                        },
                    )))
            }
        }
        let mut keys = BTreeMap::new();
        keys.insert("ctrl-y".to_string(), CfgKey::Send { send: "hi".into() });
        cx.update(|cx| bind_keys(cx, &keys));
        let (host, cx) = cx.add_window_view(|_, cx| Host { focus: cx.focus_handle(), hits: vec![] });
        host.update_in(cx, |h, window, _| window.focus(&h.focus));
        cx.simulate_keystrokes("cmd-t cmd-f ctrl-y");
        host.read_with(cx, |h, _| assert_eq!(h.hits, ["new_tab", "find", "send"]));
    }

    #[test]
    fn menus_follow_spec() {
        let m = menus();
        let names: Vec<_> = m.iter().map(|m| m.name.to_string()).collect();
        assert_eq!(names, ["nox", "Shell", "Edit", "View", "Window", "Help"]);
        assert!(m.iter().all(|m| !m.items.is_empty()));
    }

    #[test]
    fn palette_commands_are_unique() {
        let cmds = palette_commands();
        let mut labels = HashSet::new();
        for (label, _) in &cmds {
            assert!(labels.insert(*label), "duplicate palette entry {label}");
        }
        for want in ["New Tab", "Switch Theme…", "Tabs Position: Left", "Settings…", "Quit"] {
            assert!(labels.contains(want), "{want}");
        }
        let names: HashSet<_> = cmds.iter().map(|(_, a)| a.name()).collect();
        assert_eq!(names.len(), cmds.len(), "each entry runs a different action");
    }
}
