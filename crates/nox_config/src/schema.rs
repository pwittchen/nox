//! Metadata for the Settings window (SPEC §5.9): sections, labels, descriptions, control kinds,
//! and value lookups so the UI can show "differs from default".

use std::sync::OnceLock;

use crate::model::{
    AgentRestore, AgentScope, Bell, CloseOnExit, Config, ConfigEnum, CursorStyle, Deliver, GraphemeWidth,
    Mode, NewTabCwd, OptionAsMeta, Osc52, SidebarWorkspaces, SizeAppliesTo, TabTitle, TabsPosition,
    WorktreeOpens,
};

/// Sidebar sections of the Settings window, mirroring the config tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Section {
    General,
    Appearance,
    Tabs,
    Font,
    Terminal,
    Shell,
    Workspaces,
    Agents,
    Icons,
    Keys,
}

impl Section {
    pub const ALL: &'static [Section] = &[
        Section::General,
        Section::Appearance,
        Section::Tabs,
        Section::Font,
        Section::Terminal,
        Section::Shell,
        Section::Workspaces,
        Section::Agents,
        Section::Icons,
        Section::Keys,
    ];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Appearance => "Appearance",
            Section::Tabs => "Tabs",
            Section::Font => "Font",
            Section::Terminal => "Terminal",
            Section::Shell => "Shell",
            Section::Workspaces => "Workspaces",
            Section::Agents => "Agents",
            Section::Icons => "Icons",
            Section::Keys => "Keys",
        }
    }
}

/// Which control edits a setting.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingKind {
    Bool,
    Enum(&'static [&'static str]),
    Int {
        min: i64,
        max: i64,
        step: i64,
    },
    Float {
        min: f64,
        max: f64,
        step: f64,
    },
    String,
    StringList,
    /// A table of string values (`[icons]`, `[shell.env]`) or bindings (`[keys]`).
    Map,
    /// Two integers, e.g. `terminal.padding = [horizontal, vertical]`.
    IntPair {
        min: i64,
        max: i64,
        step: i64,
    },
    /// The `[agents.<name>]` tables (name → command, resume, `resume_last`, match, icon).
    Agents,
}

/// One row of the Settings window.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingMeta {
    pub section: Section,
    /// Dotted config key, shown in muted monospace (`tabs.position`).
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub kind: SettingKind,
}

fn m(
    section: Section,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    kind: SettingKind,
) -> SettingMeta {
    SettingMeta { section, key, label, description, kind }
}

const fn int(min: i64, max: i64, step: i64) -> SettingKind {
    SettingKind::Int { min, max, step }
}

const fn float(min: f64, max: f64, step: f64) -> SettingKind {
    SettingKind::Float { min, max, step }
}

fn build() -> Vec<SettingMeta> {
    use Section::{Agents, Appearance, Font, General, Icons, Keys, Shell, Tabs, Terminal, Workspaces};
    use SettingKind::{Bool, Enum, Map, StringList};
    let s = SettingKind::String;
    vec![
        // General — [app]
        m(
            General,
            "app.restore_session",
            "Restore session",
            "Reopen windows, tabs, panes and workspaces on launch.",
            Bool,
        ),
        m(
            General,
            "app.confirm_close",
            "Confirm close",
            "Ask before closing a tab or window with a running process.",
            Bool,
        ),
        m(
            General,
            "app.quit_when_last_tab_closed",
            "Quit with last tab",
            "Quit nox when the last tab is closed.",
            Bool,
        ),
        m(
            General,
            "app.dock_bounce_on_bell",
            "Bounce Dock on bell",
            "Bounce the Dock icon when a background pane rings the bell.",
            Bool,
        ),
        m(
            General,
            "app.check_for_updates",
            "Check for updates",
            "Check GitHub Releases for a new version once a day.",
            Bool,
        ),
        m(
            General,
            "app.auto_install_updates",
            "Install updates automatically",
            "Download updates in the background and install them on quit.",
            Bool,
        ),
        // Appearance — [ui]
        m(
            Appearance,
            "ui.theme",
            "Theme",
            "Color palette: nox, a bundled theme or a file in ~/.nox/themes/.",
            s.clone(),
        ),
        m(
            Appearance,
            "ui.mode",
            "Appearance",
            "Use the light or dark variant, or follow macOS.",
            Enum(Mode::NAMES),
        ),
        m(
            Appearance,
            "ui.font_family",
            "Interface font",
            "Font for tabs, sidebar, pane headers, palette and dialogs.",
            s.clone(),
        ),
        m(
            Appearance,
            "ui.pane_header",
            "Pane header",
            "Title and split / maximize / close controls above each pane.",
            Bool,
        ),
        m(
            Appearance,
            "ui.dim_inactive_panes",
            "Dim inactive panes",
            "How much unfocused panes are dimmed; 0 disables.",
            float(0.0, 1.0, 0.05),
        ),
        m(
            Appearance,
            "ui.focus_follows_mouse",
            "Focus follows mouse",
            "Focus the pane under the pointer.",
            Bool,
        ),
        m(
            Appearance,
            "ui.focus_click_through",
            "Click-through focus",
            "A click that focuses a pane is also passed to the program.",
            Bool,
        ),
        m(
            Appearance,
            "ui.window_padding",
            "Window padding",
            "Pixels between the window edge and the pane card.",
            int(0, 64, 1),
        ),
        m(
            Appearance,
            "ui.show_git_branch",
            "Show Git branch",
            "Branch of the pane's directory in pane headers and the sidebar.",
            Bool,
        ),
        // Tabs — [tabs]
        m(
            Tabs,
            "tabs.position",
            "Tabs position",
            "Tabs on top, at the bottom, or in a left sidebar.",
            Enum(TabsPosition::NAMES),
        ),
        m(Tabs, "tabs.sidebar_visible", "Show sidebar", "Sidebar visibility (left position only).", Bool),
        m(
            Tabs,
            "tabs.sidebar_replaces_tabs",
            "Sidebar instead of tab bar",
            "Top / bottom positions: show the sidebar and hide the tab bar (toggled with ⌘\\).",
            Bool,
        ),
        m(
            Tabs,
            "tabs.sidebar_width",
            "Sidebar width",
            "Sidebar width in pixels (left position only).",
            int(140, 600, 10),
        ),
        m(Tabs, "tabs.show_index", "Show tab number", "Prefix tab titles with their number.", Bool),
        m(
            Tabs,
            "tabs.title",
            "Tab title",
            "Automatic, the working directory, or the running process.",
            Enum(TabTitle::NAMES),
        ),
        m(
            Tabs,
            "tabs.icons",
            "Process icons",
            "Icons for the running process in tabs and pane headers.",
            Bool,
        ),
        m(
            Tabs,
            "tabs.sidebar_workspaces",
            "Sidebar workspaces",
            "Show only the active workspace or all of them in the sidebar.",
            Enum(SidebarWorkspaces::NAMES),
        ),
        m(
            Tabs,
            "tabs.sidebar_agents",
            "Sidebar agents",
            "List the running agents of all workspaces at the bottom of the sidebar.",
            Bool,
        ),
        // Font — [font]
        m(
            Font,
            "font.family",
            "Terminal font",
            "Family or comma-separated list; JetBrains Mono is the fallback.",
            s.clone(),
        ),
        m(Font, "font.size", "Font size", "Terminal font size in points.", float(6.0, 72.0, 0.5)),
        m(
            Font,
            "font.line_height",
            "Line height",
            "Multiple of the font's natural line height (1.0 = 100%, as in iTerm).",
            float(0.8, 3.0, 0.05),
        ),
        m(
            Font,
            "font.letter_spacing",
            "Letter spacing",
            "Extra horizontal space between cells, in pixels.",
            float(-5.0, 20.0, 0.5),
        ),
        m(
            Font,
            "font.size_applies_to",
            "Font size changes apply to",
            "Font size shortcuts change all panes or just the focused one.",
            Enum(SizeAppliesTo::NAMES),
        ),
        // Terminal — [terminal]
        m(
            Terminal,
            "terminal.scrollback",
            "Scrollback",
            "Lines kept in scrollback per pane.",
            int(0, 100_000, 1000),
        ),
        m(
            Terminal,
            "terminal.scroll_multiplier",
            "Scroll speed",
            "Lines scrolled per mouse-wheel step.",
            float(0.1, 20.0, 0.5),
        ),
        m(
            Terminal,
            "terminal.smooth_scroll_duration",
            "Smooth scrolling",
            "Duration of mouse-wheel scroll animation in ms; 0 disables.",
            int(0, 1000, 10),
        ),
        m(Terminal, "terminal.scroll_on_input", "Scroll on input", "Jump to the bottom when typing.", Bool),
        m(
            Terminal,
            "terminal.cursor_style",
            "Cursor style",
            "Shape of the terminal cursor.",
            Enum(CursorStyle::NAMES),
        ),
        m(Terminal, "terminal.cursor_blink", "Blinking cursor", "Blink the terminal cursor.", Bool),
        m(
            Terminal,
            "terminal.padding",
            "Padding",
            "Horizontal and vertical padding around the grid, in pixels.",
            SettingKind::IntPair { min: 0, max: 64, step: 1 },
        ),
        m(
            Terminal,
            "terminal.copy_on_select",
            "Copy on select",
            "Copy selected text to the clipboard.",
            Bool,
        ),
        m(
            Terminal,
            "terminal.confirm_multiline_paste",
            "Confirm multi-line paste",
            "Ask before pasting text with several lines.",
            Bool,
        ),
        m(
            Terminal,
            "terminal.word_separators",
            "Word separators",
            "Characters that end a word on double-click selection.",
            s.clone(),
        ),
        m(
            Terminal,
            "terminal.option_as_meta",
            "Option as Meta",
            "Which Option keys send Meta (Esc-prefixed) sequences.",
            Enum(OptionAsMeta::NAMES),
        ),
        m(
            Terminal,
            "terminal.bold_is_bright",
            "Bold is bright",
            "Draw bold text in bright ANSI colors.",
            Bool,
        ),
        m(Terminal, "terminal.bell", "Bell", "What the terminal bell does.", Enum(Bell::NAMES)),
        m(
            Terminal,
            "terminal.attention_sound",
            "Attention sound",
            "Soft chime when a job finishes or waits for input unseen.",
            Bool,
        ),
        m(
            Terminal,
            "terminal.attention_sound_volume",
            "Attention volume",
            "Volume of the attention chime.",
            float(0.0, 1.0, 0.05),
        ),
        m(
            Terminal,
            "terminal.notifications",
            "Notifications",
            "macOS notification when a job finishes or waits for input unseen.",
            Bool,
        ),
        m(
            Terminal,
            "terminal.attention_min_duration",
            "Attention minimum",
            "Commands shorter than this many seconds finish silently.",
            int(0, 86_400, 1),
        ),
        m(
            Terminal,
            "terminal.close_on_exit",
            "Close on exit",
            "Close a pane when its shell exits: always, only cleanly, or never.",
            Enum(CloseOnExit::NAMES),
        ),
        m(
            Terminal,
            "terminal.osc52",
            "Clipboard access (OSC 52)",
            "Let programs write (or also read) the clipboard.",
            Enum(Osc52::NAMES),
        ),
        m(
            Terminal,
            "terminal.grapheme_width",
            "Emoji width",
            "unicode: emoji sequences (flags, ZWJ, skin tones, VS16) take one 2-column cell; legacy: per code point.",
            Enum(GraphemeWidth::NAMES),
        ),
        m(
            Terminal,
            "terminal.kitty_keyboard",
            "Kitty keyboard protocol",
            "Enable the kitty keyboard protocol for programs that ask.",
            Bool,
        ),
        m(
            Terminal,
            "terminal.minimum_contrast",
            "Minimum contrast",
            "Adjust text too close to its background; 1 disables, 4.5 is WCAG AA.",
            float(1.0, 21.0, 0.5),
        ),
        // Shell — [shell]
        m(
            Shell,
            "shell.program",
            "Shell",
            "Program to run; empty uses the login shell from $SHELL.",
            s.clone(),
        ),
        m(Shell, "shell.args", "Arguments", "Arguments passed to the shell.", StringList),
        m(Shell, "shell.working_directory", "Working directory", "Directory of new windows.", s.clone()),
        m(
            Shell,
            "shell.inherit_cwd",
            "Inherit directory",
            "New tabs and panes start in the focused pane's directory.",
            Bool,
        ),
        m(
            Shell,
            "shell.integration",
            "Shell integration",
            "Inject OSC 7 / OSC 133 snippets for zsh, bash and fish.",
            Bool,
        ),
        m(Shell, "shell.env", "Environment", "Extra environment variables for new shells.", Map),
        // Workspaces — [workspaces]
        m(
            Workspaces,
            "workspaces.new_tab_cwd",
            "New tab directory",
            "Focused pane's directory or the workspace root.",
            Enum(NewTabCwd::NAMES),
        ),
        m(
            Workspaces,
            "workspaces.close_when_empty",
            "Close when empty",
            "Closing a workspace's last tab closes the workspace.",
            Bool,
        ),
        m(
            Workspaces,
            "workspaces.worktree_dir",
            "Worktree directory",
            "Where new Git worktrees go; empty puts them next to the root.",
            s.clone(),
        ),
        m(
            Workspaces,
            "workspaces.worktree_opens",
            "Worktree opens in",
            "Open a new worktree in a tab or its own workspace.",
            Enum(WorktreeOpens::NAMES),
        ),
        m(
            Workspaces,
            "workspaces.lazy_restore",
            "Lazy restore",
            "Start background workspaces' shells when first shown.",
            Bool,
        ),
        m(
            Workspaces,
            "workspaces.rerun_startup_commands",
            "Re-run startup commands",
            "Re-run project file tab commands on session restore.",
            Bool,
        ),
        // Agents — [agents]
        m(Agents, "agents.keep_awake", "Keep awake", "Prevent system sleep while an agent is working.", Bool),
        m(
            Agents,
            "agents.restore",
            "Restore agents",
            "Resume agent sessions on relaunch, ask first, or not.",
            Enum(AgentRestore::NAMES),
        ),
        m(
            Agents,
            "agents.cooperation",
            "Agent cooperation",
            "Control socket, nox CLI and MCP server for agents.",
            Bool,
        ),
        m(
            Agents,
            "agents.scope",
            "Agent scope",
            "Which panes an agent can see and reach.",
            Enum(AgentScope::NAMES),
        ),
        m(
            Agents,
            "agents.deliver",
            "Message delivery",
            "Messages go to the inbox, or are also typed into idle agents.",
            Enum(Deliver::NAMES),
        ),
        m(
            Agents,
            "agents.allow_send_to_shell",
            "Allow send to shells",
            "Allow `nox send` into panes that aren't agents.",
            Bool,
        ),
        m(
            Agents,
            "agents.claim_ttl",
            "Claim lifetime",
            "Seconds until an unrenewed claim expires.",
            int(10, 604_800, 60),
        ),
        m(
            Agents,
            "agents",
            "Agents",
            "Agents for New Agent Tab, worktrees and resume, with their commands.",
            SettingKind::Agents,
        ),
        // Icons — [icons]
        m(
            Icons,
            "icons",
            "Process icons",
            "Executable → built-in icon name or path to an SVG in ~/.nox/icons/.",
            Map,
        ),
        // Keys — [keys]
        m(Keys, "keys", "Shortcuts", "Action → shortcut; empty unbinds; { send = \"…\" } sends bytes.", Map),
    ]
}

/// Every setting shown in the Settings window, in display order.
#[must_use]
pub fn settings_schema() -> Vec<SettingMeta> {
    schema_ref().to_vec()
}

fn schema_ref() -> &'static [SettingMeta] {
    static SCHEMA: OnceLock<Vec<SettingMeta>> = OnceLock::new();
    SCHEMA.get_or_init(build)
}

/// Metadata of one dotted key, if it's a Settings row.
#[must_use]
pub fn find_setting(key: &str) -> Option<&'static SettingMeta> {
    schema_ref().iter().find(|m| m.key == key)
}

/// The current value of a dotted key (`"tabs.position"`, `"agents.claude.command"`, `"keys"`).
#[must_use]
pub fn get_value(config: &Config, key: &str) -> Option<toml::Value> {
    let root = toml::Value::try_from(config).ok()?;
    if key.is_empty() {
        return Some(root);
    }
    let mut cur = &root;
    for seg in key.split('.') {
        cur = cur.as_table()?.get(seg)?;
    }
    Some(cur.clone())
}

/// The default value of a dotted key.
pub fn default_value(key: &str) -> Option<toml::Value> {
    static DEFAULT: OnceLock<Config> = OnceLock::new();
    get_value(DEFAULT.get_or_init(Config::default), key)
}

/// Whether a key's current value differs from its default (shows the "reset" control).
#[must_use]
pub fn differs_from_default(config: &Config, key: &str) -> bool {
    get_value(config, key) != default_value(key)
}
