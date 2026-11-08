//! The configuration model (SPEC §11): every key of `~/.nox/config.toml` with its default.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A closed set of string values for a config key (`"top" | "bottom" | "left"`, …).
pub trait ConfigEnum: Sized + Copy + 'static {
    /// Every accepted spelling, in documentation order.
    const NAMES: &'static [&'static str];
    /// Parses the config spelling; `None` for an unknown value.
    fn parse(s: &str) -> Option<Self>;
    /// The config spelling.
    fn as_str(self) -> &'static str;
}

macro_rules! config_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident => $s:literal),+ $(,)? } default $def:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $( #[serde(rename = $s)] $variant, )+
        }

        impl ConfigEnum for $name {
            const NAMES: &'static [&'static str] = &[$($s),+];
            fn parse(s: &str) -> Option<Self> {
                match s {
                    $($s => Some($name::$variant),)+
                    _ => None,
                }
            }
            fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $s,)+
                }
            }
        }

        impl Default for $name {
            fn default() -> Self {
                $name::$def
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

config_enum!(
    /// `tabs.position`.
    TabsPosition { Top => "top", Bottom => "bottom", Left => "left" } default Top
);
config_enum!(
    /// `ui.mode`: when the light or dark variant of the theme is used.
    Mode { System => "system", Light => "light", Dark => "dark" } default System
);
config_enum!(
    /// `tabs.title`.
    TabTitle { Auto => "auto", Cwd => "cwd", Process => "process" } default Auto
);
config_enum!(
    /// `tabs.sidebar_workspaces`.
    SidebarWorkspaces { Active => "active", All => "all" } default All
);
config_enum!(
    /// `workspaces.new_tab_cwd`.
    NewTabCwd { Inherit => "inherit", Root => "root" } default Inherit
);
config_enum!(
    /// `workspaces.worktree_opens`.
    WorktreeOpens { Tab => "tab", Workspace => "workspace" } default Tab
);
config_enum!(
    /// `agents.restore`.
    AgentRestore { Resume => "resume", Ask => "ask", Off => "off" } default Resume
);
config_enum!(
    /// `agents.scope`.
    AgentScope { Workspace => "workspace", All => "all" } default Workspace
);
config_enum!(
    /// `agents.deliver`.
    Deliver { Inbox => "inbox", Type => "type" } default Inbox
);
config_enum!(
    /// `font.size_applies_to`.
    SizeAppliesTo { All => "all", Pane => "pane" } default All
);
config_enum!(
    /// `terminal.cursor_style`.
    CursorStyle { Block => "block", Bar => "bar", Underline => "underline" } default Block
);
config_enum!(
    /// `terminal.option_as_meta`.
    OptionAsMeta { Left => "left", Right => "right", Both => "both", None => "none" } default Left
);
config_enum!(
    /// `terminal.bell`.
    Bell { Visual => "visual", Sound => "sound", None => "none" } default Visual
);
config_enum!(
    /// `terminal.close_on_exit`.
    CloseOnExit { Always => "always", Clean => "clean", Never => "never" } default Clean
);
config_enum!(
    /// `terminal.osc52`.
    Osc52 { Write => "write", ReadWrite => "read-write", Off => "off" } default Write
);
config_enum!(
    /// `terminal.grapheme_width`.
    GraphemeWidth { Unicode => "unicode", Legacy => "legacy" } default Unicode
);

/// The whole `config.toml`.
#[derive(Clone, Debug, PartialEq, Default, Serialize)]
pub struct Config {
    pub ui: UiConfig,
    pub tabs: TabsConfig,
    pub workspaces: WorkspacesConfig,
    pub agents: AgentsConfig,
    pub icons: BTreeMap<String, String>,
    pub font: FontConfig,
    pub terminal: TerminalConfig,
    pub shell: ShellConfig,
    pub app: AppConfig,
    pub keys: BTreeMap<String, KeyBinding>,
    pub window: WindowConfig,
}

/// `[ui]`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UiConfig {
    pub theme: String,
    pub mode: Mode,
    pub font_family: String,
    pub pane_header: bool,
    pub dim_inactive_panes: f64,
    pub focus_follows_mouse: bool,
    pub focus_click_through: bool,
    pub window_padding: u32,
    pub show_git_branch: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "nox".into(),
            mode: Mode::System,
            font_family: "system".into(),
            pane_header: true,
            dim_inactive_panes: 0.15,
            focus_follows_mouse: false,
            focus_click_through: false,
            window_padding: 6,
            show_git_branch: true,
        }
    }
}

/// `[tabs]`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TabsConfig {
    pub position: TabsPosition,
    pub sidebar_visible: bool,
    pub sidebar_replaces_tabs: bool,
    pub sidebar_width: u32,
    pub show_index: bool,
    pub title: TabTitle,
    pub icons: bool,
    pub sidebar_workspaces: SidebarWorkspaces,
    pub sidebar_agents: bool,
}

impl Default for TabsConfig {
    fn default() -> Self {
        Self {
            position: TabsPosition::Top,
            sidebar_visible: true,
            sidebar_replaces_tabs: false,
            sidebar_width: 220,
            show_index: true,
            title: TabTitle::Auto,
            icons: true,
            sidebar_workspaces: SidebarWorkspaces::All,
            sidebar_agents: true,
        }
    }
}

/// `[workspaces]`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WorkspacesConfig {
    pub new_tab_cwd: NewTabCwd,
    pub close_when_empty: bool,
    pub worktree_dir: String,
    pub worktree_opens: WorktreeOpens,
    pub lazy_restore: bool,
    pub rerun_startup_commands: bool,
}

impl Default for WorkspacesConfig {
    fn default() -> Self {
        Self {
            new_tab_cwd: NewTabCwd::Inherit,
            close_when_empty: true,
            worktree_dir: String::new(),
            worktree_opens: WorktreeOpens::Tab,
            lazy_restore: true,
            rerun_startup_commands: false,
        }
    }
}

/// `[agents]`: fixed settings plus one `[agents.<name>]` table per agent.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AgentsConfig {
    pub keep_awake: bool,
    pub restore: AgentRestore,
    pub cooperation: bool,
    pub scope: AgentScope,
    pub deliver: Deliver,
    pub allow_send_to_shell: bool,
    /// Seconds; unrenewed claims expire.
    pub claim_ttl: u64,
    /// `[agents.<name>]` tables; the name is also the executable matched for detection.
    #[serde(flatten)]
    pub defs: BTreeMap<String, AgentDef>,
}

impl AgentsConfig {
    /// The built-in agent definitions (`claude`, `codex`).
    #[must_use]
    pub fn default_defs() -> BTreeMap<String, AgentDef> {
        let mut defs = BTreeMap::new();
        defs.insert(
            "claude".to_string(),
            AgentDef {
                command: "claude".into(),
                resume: Some("claude --resume {session}".into()),
                resume_last: Some("claude --continue".into()),
                match_: Vec::new(),
                icon: None,
            },
        );
        defs.insert(
            "codex".to_string(),
            AgentDef {
                command: "codex".into(),
                resume: Some("codex resume {session}".into()),
                resume_last: Some("codex resume --last".into()),
                match_: Vec::new(),
                icon: None,
            },
        );
        defs
    }
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            keep_awake: true,
            restore: AgentRestore::Resume,
            cooperation: true,
            scope: AgentScope::Workspace,
            deliver: Deliver::Inbox,
            allow_send_to_shell: false,
            claim_ttl: 1800,
            defs: Self::default_defs(),
        }
    }
}

/// One `[agents.<name>]` table.
#[derive(Clone, Debug, PartialEq, Default, Serialize)]
pub struct AgentDef {
    /// Command line that starts the agent.
    pub command: String,
    /// Resume a known session; `{session}` is replaced by the session id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
    /// Resume the latest session in the directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_last: Option<String>,
    /// Extra executables detected as this agent (TOML key `match`).
    #[serde(rename = "match", skip_serializing_if = "Vec::is_empty")]
    pub match_: Vec<String>,
    /// Built-in icon name or path to an SVG.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// `[font]` (the terminal font).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FontConfig {
    pub family: String,
    pub size: f64,
    pub line_height: f64,
    pub letter_spacing: f64,
    pub weight: String,
    pub weight_bold: String,
    pub size_applies_to: SizeAppliesTo,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "JetBrains Mono".into(),
            size: 14.0,
            line_height: 1.0,
            letter_spacing: 0.0,
            weight: "normal".into(),
            weight_bold: "bold".into(),
            size_applies_to: SizeAppliesTo::All,
        }
    }
}

/// `[terminal]`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TerminalConfig {
    pub scrollback: u32,
    pub scroll_multiplier: f64,
    /// Milliseconds; 0 disables.
    pub smooth_scroll_duration: u32,
    pub scroll_on_input: bool,
    pub cursor_style: CursorStyle,
    pub cursor_blink: bool,
    /// `[horizontal, vertical]` px.
    pub padding: [u32; 2],
    pub copy_on_select: bool,
    pub confirm_multiline_paste: bool,
    pub word_separators: String,
    pub option_as_meta: OptionAsMeta,
    pub bold_is_bright: bool,
    pub bell: Bell,
    pub attention_sound: bool,
    pub attention_sound_volume: f64,
    pub notifications: bool,
    /// Seconds.
    pub attention_min_duration: u64,
    pub close_on_exit: CloseOnExit,
    pub osc52: Osc52,
    pub grapheme_width: GraphemeWidth,
    pub kitty_keyboard: bool,
    pub minimum_contrast: f64,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            scrollback: 10_000,
            scroll_multiplier: 3.0,
            smooth_scroll_duration: 100,
            scroll_on_input: true,
            cursor_style: CursorStyle::Block,
            cursor_blink: false,
            padding: [8, 6],
            copy_on_select: true,
            confirm_multiline_paste: true,
            word_separators: " ()[]{}',\"`│".into(),
            option_as_meta: OptionAsMeta::Left,
            bold_is_bright: false,
            bell: Bell::Visual,
            attention_sound: true,
            attention_sound_volume: 0.4,
            notifications: true,
            attention_min_duration: 10,
            close_on_exit: CloseOnExit::Clean,
            osc52: Osc52::Write,
            grapheme_width: GraphemeWidth::Unicode,
            kitty_keyboard: false,
            minimum_contrast: 1.0,
        }
    }
}

/// `[shell]` and `[shell.env]`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ShellConfig {
    /// Empty: login shell from `$SHELL`.
    pub program: String,
    pub args: Vec<String>,
    pub working_directory: String,
    pub inherit_cwd: bool,
    pub integration: bool,
    pub env: BTreeMap<String, String>,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            program: String::new(),
            args: vec!["-l".into()],
            working_directory: "~".into(),
            inherit_cwd: true,
            integration: true,
            env: BTreeMap::new(),
        }
    }
}

/// `[app]`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AppConfig {
    pub restore_session: bool,
    pub confirm_close: bool,
    pub quit_when_last_tab_closed: bool,
    pub dock_bounce_on_bell: bool,
    pub check_for_updates: bool,
    pub auto_install_updates: bool,
    /// Undocumented: `"stable"` (default) or another release channel.
    pub update_channel: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            restore_session: true,
            confirm_close: true,
            quit_when_last_tab_closed: true,
            dock_bounce_on_bell: true,
            check_for_updates: true,
            auto_install_updates: false,
            update_channel: "stable".into(),
        }
    }
}

/// A `[keys]` entry: a shortcut (`"cmd-d"`, `""` unbinds) or bytes to send to the PTY.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeyBinding {
    Shortcut(String),
    Send { send: String },
}

impl KeyBinding {
    /// `""`: the action is unbound and the key passes through to the program.
    #[must_use]
    pub fn is_unbind(&self) -> bool {
        matches!(self, KeyBinding::Shortcut(s) if s.trim().is_empty())
    }
}

/// `[window]` (owned by the running app; not live-reloaded).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WindowConfig {
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self { width: 1000, height: 680, x: 0, y: 0 }
    }
}
