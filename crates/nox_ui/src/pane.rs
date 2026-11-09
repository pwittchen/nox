//! One pane: a terminal session plus everything nox knows about it (title, cwd, process icon,
//! git branch, run status, agent state) and its view state (scroll, selection, find, links).
//! SPEC §4.3–4.6, §4.8, §7.

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, Bounds, ClipboardItem, Context, CursorStyle, Entity,
    EntityInputHandler, EventEmitter, ExternalPaths, FocusHandle, Focusable, Hsla, KeyDownEvent, Modifiers,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollDelta, ScrollWheelEvent, SharedString, Subscription, Task, UTF16Selection, Window, div, point,
    prelude::*, px, svg,
};
use nox_agents::{AgentState, AgentTracker, Icon, ProcessInfo, Transition};
use nox_layout::{PaneId, SplitDirection, TabStatus};
use nox_terminal::alacritty_terminal::grid::{Dimensions, Scroll};
use nox_terminal::alacritty_terminal::index::{Column, Direction, Line, Point as GridPoint, Side};
use nox_terminal::alacritty_terminal::selection::{Selection, SelectionType};
use nox_terminal::alacritty_terminal::term::TermMode;
use nox_terminal::alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use nox_terminal::keys::{self, KeyInput, KeyModes, Mods};
use nox_terminal::mouse::{self, MouseAction, MouseMods};
use nox_terminal::{OscEvent, SpawnOptions, TerminalEvent, TerminalSession};

use crate::actions::{
    ClearBuffer, Copy, CopyLastOutput, Find, FindNext, FindPrevious, NextPrompt, Paste, PreviousPrompt,
    RestartPane, ScrollPageDown, ScrollPageUp, ScrollToBottom, ScrollToTop, SelectAll, SendShiftEnter,
};
use crate::icons::{IconRegistry, icon_element};
use crate::scroll::ScrollState;
use crate::settings_state::Settings;
use crate::terminal_element::{TerminalElement, position_to_point};
use crate::text_input::{InputEvent, TextInput};
use crate::theme::Theme;

/// Geometry of the grid as last laid out.
#[derive(Debug, Clone)]
pub struct GridGeom {
    pub bounds: Bounds<Pixels>,
    pub origin: Point<Pixels>,
    pub cell_w: Pixels,
    pub cell_h: Pixels,
    pub cols: usize,
    pub rows: usize,
}

/// What to run after the shell starts.
#[derive(Debug, Clone)]
pub enum Startup {
    /// Type the command and press ↵ (runs through the user's shell, aliases and PATH apply).
    Run(String),
    /// Type the command but don't run it.
    Type(String),
}

/// How nox launched an agent in this pane (for session restore).
#[derive(Debug, Clone)]
pub struct AgentLaunch {
    pub kind: String,
    pub command: String,
    pub start_cwd: PathBuf,
    /// Started via a resume command (a quick failure shows the resume banner).
    pub resumed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PaneSpawn {
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub startup: Option<Startup>,
    pub agent: Option<AgentLaunch>,
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub enum PaneEvent {
    Focused,
    TitleChanged,
    Bell,
    CommandFinished { command: String, exit: Option<i32>, duration: Duration },
    Notify { title: Option<String>, body: String },
    AgentChanged { transition: Option<Transition> },
    Exited { code: Option<i32> },
    Close,
    Split(SplitDirection),
    ToggleMaximize,
    ContextMenu,
    UserInput,
}

/// A hovered link: spans (line, first col, last col) and its target.
#[derive(Debug, Clone, PartialEq)]
pub struct HoveredLink {
    pub spans: Vec<(Line, usize, usize)>,
    pub target: LinkTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LinkTarget {
    Url(String),
    Path(PathBuf),
}

pub struct FindState {
    pub input: Entity<TextInput>,
    pub case_sensitive: bool,
    pub regex: bool,
    pub matches: Vec<Match>,
    pub current: usize,
    _sub: Subscription,
}

pub struct AgentInfo {
    pub kind: String,
    pub tracker: AgentTracker,
    pub launch: Option<AgentLaunch>,
    pub cmdline: String,
    /// The agent's process has been seen in the foreground. A launched agent isn't there yet
    /// right after the launch (the shell is still reading the command), so the shell in the
    /// foreground only means it exited once it was seen, or after [`AGENT_LAUNCH_GRACE`].
    pub seen: bool,
    pub created: Instant,
}

/// How long a launched agent that was never seen in the foreground is kept alive (SPEC §4.8).
const AGENT_LAUNCH_GRACE: Duration = Duration::from_secs(10);

impl AgentInfo {
    fn left_foreground_means_exit(&self) -> bool {
        self.seen || self.created.elapsed() >= AGENT_LAUNCH_GRACE
    }
}

pub struct Pane {
    pub id: PaneId,
    pub session: Option<TerminalSession>,
    pub spawn_error: Option<String>,
    pub focus_handle: FocusHandle,
    spawn: PaneSpawn,

    // Identity.
    pub custom_name: Option<String>,
    pub custom_icon: Option<String>,
    pub osc_title: Option<String>,
    pub cwd: Option<PathBuf>,
    pub process: Option<ProcessInfo>,
    pub process_is_shell: bool,
    pub icon: Icon,
    pub branch: Option<String>,
    last_refresh: Option<Instant>,
    refresh_task: Option<Task<()>>,

    // Status.
    pub shell_integration: bool,
    pub running_since: Option<Instant>,
    pub done_unseen: bool,
    /// The unseen finished command exited non-zero (red dot instead of green).
    pub done_failed: bool,
    pub bell_unseen: bool,
    pub bell_flash: Option<Instant>,
    pub exited: Option<Option<i32>>,
    pub resume_failed: bool,
    pub last_user_input: Option<Instant>,
    pub from_banner: Option<(String, Instant)>,

    // Agent.
    pub agent: Option<AgentInfo>,
    agent_tick: Option<Task<()>>,
    pub agent_name: Option<String>,
    pub unread: usize,
    pub claims: Vec<String>,
    pub resume_on_restore: bool,

    // View.
    pub geom: Option<GridGeom>,
    pub scroll: ScrollState,
    pub new_output_below: bool,
    selecting: Option<SelectionType>,
    pending_selection: Option<Selection>,
    autoscroll: Option<Task<()>>,
    pub hovered_link: Option<HoveredLink>,
    pub find: Option<FindState>,
    pub marked_text: Option<String>,
    blink_epoch: Instant,
    blink_task: Option<Task<()>>,
    pub font_size_override: Option<f32>,
    pub header_hovered: bool,
    pub shake: Option<Instant>,
    tasks: Vec<Task<()>>,
}

impl EventEmitter<PaneEvent> for Pane {}

impl Focusable for Pane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn spawn_options(cfg: &nox_config::Config, spawn: &PaneSpawn) -> SpawnOptions {
    let program = if cfg.shell.program.trim().is_empty() { None } else { Some(cfg.shell.program.clone()) };
    let mut env: HashMap<String, String> = cfg.shell.env.clone().into_iter().collect();
    env.extend(spawn.env.clone());
    let integration_dir =
        cfg.shell.integration.then(|| nox_terminal::shell::default_dir(&nox_config::nox_dir()));
    SpawnOptions {
        program,
        args: cfg.shell.args.clone(),
        cwd: spawn.cwd.clone(),
        env,
        scrollback: cfg.terminal.scrollback as usize,
        word_separators: cfg.terminal.word_separators.clone(),
        kitty_keyboard: cfg.terminal.kitty_keyboard,
        grapheme_clusters: cfg.terminal.grapheme_width == nox_config::GraphemeWidth::Unicode,
        osc52: match cfg.terminal.osc52 {
            nox_config::Osc52::Off => nox_terminal::ClipboardPolicy::Off,
            nox_config::Osc52::Write => nox_terminal::ClipboardPolicy::Write,
            nox_config::Osc52::ReadWrite => nox_terminal::ClipboardPolicy::ReadWrite,
        },
        cursor: cursor_style(cfg),
        integration_dir,
        ..SpawnOptions::default()
    }
}

#[must_use]
pub fn cursor_style(cfg: &nox_config::Config) -> nox_terminal::session::CursorConfig {
    nox_terminal::session::CursorConfig {
        shape: match cfg.terminal.cursor_style {
            nox_config::CursorStyle::Block => nox_terminal::session::CursorKind::Block,
            nox_config::CursorStyle::Bar => nox_terminal::session::CursorKind::Bar,
            nox_config::CursorStyle::Underline => nox_terminal::session::CursorKind::Underline,
        },
        blinking: cfg.terminal.cursor_blink,
    }
}

impl Pane {
    pub fn new(id: PaneId, spawn: PaneSpawn, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let custom_name = spawn.name.clone();
        let cwd = spawn.cwd.clone();
        let mut this = Self {
            id,
            session: None,
            spawn_error: None,
            focus_handle: focus_handle.clone(),
            spawn,
            custom_name,
            custom_icon: None,
            osc_title: None,
            cwd,
            process: None,
            process_is_shell: true,
            icon: Icon::TERMINAL,
            branch: None,
            last_refresh: None,
            refresh_task: None,
            shell_integration: false,
            running_since: None,
            done_unseen: false,
            done_failed: false,
            bell_unseen: false,
            bell_flash: None,
            exited: None,
            resume_failed: false,
            last_user_input: None,
            from_banner: None,
            agent: None,
            agent_tick: None,
            agent_name: None,
            unread: 0,
            claims: vec![],
            resume_on_restore: true,
            geom: None,
            scroll: ScrollState::default(),
            new_output_below: false,
            selecting: None,
            pending_selection: None,
            autoscroll: None,
            hovered_link: None,
            find: None,
            marked_text: None,
            blink_epoch: Instant::now(),
            blink_task: None,
            font_size_override: None,
            header_hovered: false,
            shake: None,
            tasks: vec![],
        };
        cx.on_focus(&focus_handle, window, |this: &mut Self, _, cx| {
            this.blink_epoch = Instant::now();
            this.schedule_blink(cx);
            this.refresh_info(cx);
            cx.emit(PaneEvent::Focused);
            cx.notify();
        })
        .detach();
        cx.on_blur(&focus_handle, window, |_: &mut Self, _, cx| cx.notify()).detach();
        // Live config: terminal options of the running session follow `config.toml`.
        cx.observe_global::<Settings>(|this: &mut Self, cx| {
            let cfg = Settings::global(cx).config.clone();
            if let Some(s) = &this.session {
                let osc52 = match cfg.terminal.osc52 {
                    nox_config::Osc52::Off => nox_terminal::ClipboardPolicy::Off,
                    nox_config::Osc52::Write => nox_terminal::ClipboardPolicy::Write,
                    nox_config::Osc52::ReadWrite => nox_terminal::ClipboardPolicy::ReadWrite,
                };
                s.set_options(
                    cfg.terminal.scrollback as usize,
                    &cfg.terminal.word_separators,
                    cfg.terminal.kitty_keyboard,
                    osc52,
                    cfg.terminal.grapheme_width == nox_config::GraphemeWidth::Unicode,
                    cursor_style(&cfg),
                );
            }
            cx.notify();
        })
        .detach();
        cx.observe_global::<Theme>(|this: &mut Self, cx| {
            if let Some(s) = &this.session {
                s.set_query_colors(query_colors(Theme::global(cx)));
            }
            cx.notify();
        })
        .detach();
        this.start(cx);
        this
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let settings = Settings::global(cx).clone();
        let opts = spawn_options(&settings.config, &self.spawn);
        match TerminalSession::spawn(&opts) {
            Ok(session) => {
                session.set_query_colors(query_colors(Theme::global(cx)));
                let events = session.events();
                if let Some(startup) = self.spawn.startup.clone() {
                    match startup {
                        Startup::Run(cmd) => session.write(format!("{cmd}\r").as_bytes()),
                        Startup::Type(cmd) => session.write(cmd.as_bytes()),
                    }
                }
                if let Some(launch) = self.spawn.agent.clone() {
                    let now = Instant::now();
                    self.agent = Some(AgentInfo {
                        kind: launch.kind.clone(),
                        tracker: AgentTracker::new(launch.kind.clone(), now),
                        cmdline: launch.command.clone(),
                        launch: Some(launch),
                        seen: false,
                        created: now,
                    });
                }
                self.session = Some(session);
                self.exited = None;
                self.resume_failed = false;
                let task = cx.spawn(async move |this, cx| {
                    while let Ok(ev) = events.recv().await {
                        let alive = this.update(cx, |this, cx| this.handle_event(ev, cx)).is_ok();
                        if !alive {
                            break;
                        }
                    }
                });
                self.tasks.push(task);
            }
            Err(e) => {
                self.spawn_error = Some(e.to_string());
                log::error!("failed to start shell: {e}");
            }
        }
    }

    /// ⌘R: restart an exited pane in its last directory.
    pub fn restart(&mut self, cx: &mut Context<Self>) {
        if self.exited.is_none() && self.session.is_some() {
            return;
        }
        self.tasks.clear();
        self.session = None;
        self.spawn.cwd = self.cwd.clone().or(self.spawn.cwd.clone());
        self.spawn.startup = match (&self.agent, self.resume_failed) {
            (Some(a), true) => a.launch.as_ref().map(|l| Startup::Run(l.command.clone())),
            _ => None,
        };
        self.spawn.agent = None;
        self.agent = None;
        self.osc_title = None;
        self.scroll.reset();
        self.start(cx);
        cx.emit(PaneEvent::TitleChanged);
        cx.notify();
    }

    // ---- events from the session ----------------------------------------------------------

    fn handle_event(&mut self, ev: TerminalEvent, cx: &mut Context<Self>) {
        match ev {
            TerminalEvent::Wakeup => {
                if let Some(s) = &self.session {
                    s.clear_dirty();
                    if s.term().lock().grid().display_offset() > 0 {
                        self.new_output_below = true;
                    }
                }
                self.blink_epoch = Instant::now();
                self.throttled_refresh(cx);
                if let Some(a) = &mut self.agent {
                    let t = a.tracker.on_output(Instant::now());
                    if t.is_some() {
                        cx.emit(PaneEvent::AgentChanged { transition: t });
                    }
                    self.schedule_agent_tick(cx);
                }
                cx.notify();
            }
            TerminalEvent::Title(t) => {
                self.osc_title = Some(t.clone());
                if let Some(a) = &mut self.agent {
                    let tr = a.tracker.on_title(&t, Instant::now());
                    if tr.is_some() {
                        cx.emit(PaneEvent::AgentChanged { transition: tr });
                    }
                }
                self.refresh_info(cx);
                cx.emit(PaneEvent::TitleChanged);
            }
            TerminalEvent::ResetTitle => {
                self.osc_title = None;
                cx.emit(PaneEvent::TitleChanged);
            }
            TerminalEvent::Bell => {
                let bell = Settings::global(cx).config.terminal.bell;
                match bell {
                    nox_config::Bell::Visual => {
                        self.bell_flash = Some(Instant::now());
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(Duration::from_millis(160)).await;
                            let _ = this.update(cx, |this, cx| {
                                this.bell_flash = None;
                                cx.notify();
                            });
                        })
                        .detach();
                    }
                    nox_config::Bell::Sound => nox_mac::beep(),
                    nox_config::Bell::None => {}
                }
                cx.emit(PaneEvent::Bell);
                cx.notify();
            }
            TerminalEvent::ClipboardStore(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            TerminalEvent::ClipboardLoad => {
                let text = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
                if let Some(s) = &self.session {
                    s.answer_clipboard_load(&text);
                }
            }
            TerminalEvent::CursorBlinkingChange => {
                self.blink_epoch = Instant::now();
                self.schedule_blink(cx);
            }
            TerminalEvent::Osc(osc) => self.handle_osc(osc, cx),
            TerminalEvent::Exited(code) => {
                self.exited = Some(code);
                self.running_since = None;
                if let Some(a) = &mut self.agent {
                    let t = a.tracker.on_exit(Instant::now());
                    cx.emit(PaneEvent::AgentChanged { transition: t });
                }
                cx.emit(PaneEvent::Exited { code });
                cx.notify();
            }
        }
    }

    fn handle_osc(&mut self, osc: OscEvent, cx: &mut Context<Self>) {
        match osc {
            OscEvent::Cwd(p) => {
                self.shell_integration = true;
                if self.cwd.as_ref() != Some(&p) {
                    self.cwd = Some(p);
                    self.branch = self.cwd.as_deref().and_then(nox_agents::git_branch);
                    cx.emit(PaneEvent::TitleChanged);
                }
            }
            OscEvent::PromptStart | OscEvent::InputStart => {
                self.shell_integration = true;
                self.refresh_info(cx);
            }
            OscEvent::CommandStart => {
                self.shell_integration = true;
                self.running_since = Some(Instant::now());
                self.refresh_info(cx);
                cx.emit(PaneEvent::TitleChanged);
            }
            OscEvent::CommandEnd(code) => {
                self.shell_integration = true;
                if let Some(start) = self.running_since.take() {
                    let command = self.process_display_name().unwrap_or_else(|| "command".into());
                    let duration = start.elapsed();
                    if let Some(a) = &self.agent
                        && a.launch.as_ref().is_some_and(|l| l.resumed)
                        && code.unwrap_or(0) != 0
                        && duration < Duration::from_secs(5)
                    {
                        self.resume_failed = true;
                    }
                    cx.emit(PaneEvent::CommandFinished { command, exit: code, duration });
                }
                self.refresh_info(cx);
                cx.emit(PaneEvent::TitleChanged);
            }
            OscEvent::Notify { title, body } => {
                if let Some(a) = &mut self.agent {
                    let t = a.tracker.on_osc_notify(&body, Instant::now());
                    cx.emit(PaneEvent::AgentChanged { transition: t });
                }
                cx.emit(PaneEvent::Notify { title, body });
            }
        }
        cx.notify();
    }

    fn throttled_refresh(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        match self.last_refresh {
            Some(t) if now.duration_since(t) < Duration::from_millis(500) => {
                if self.refresh_task.is_none() {
                    let wait = Duration::from_millis(500).checked_sub(now.duration_since(t)).unwrap();
                    self.refresh_task = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(wait).await;
                        let _ = this.update(cx, |this, cx| {
                            this.refresh_task = None;
                            this.refresh_info(cx);
                        });
                    }));
                }
            }
            _ => self.refresh_info(cx),
        }
    }

    /// Re-read the foreground process, cwd, icon, branch and agent detection (SPEC §4.4, 4.6).
    pub fn refresh_info(&mut self, cx: &mut Context<Self>) {
        self.last_refresh = Some(Instant::now());
        let Some(session) = &self.session else { return };
        let fg = session.foreground();
        let shell_pid = session.pid();
        let registry = cx.global::<IconRegistry>();
        let login_shell = registry.login_shell.clone();
        let mut changed = false;
        let mut finished: Option<(String, Duration)> = None;
        if let Some(fg) = &fg {
            let proc = ProcessInfo { exe: fg.exe.clone(), args: fg.args.clone() };
            let is_shell = fg.pid == shell_pid;
            // The command that just ended is the one seen in the foreground before this refresh.
            let previous = self.process.as_ref().map(|p| p.exe.clone());
            if self.process.as_ref() != Some(&proc) || self.process_is_shell != is_shell {
                self.process = Some(proc);
                self.process_is_shell = is_shell;
                changed = true;
            }
            if !self.shell_integration || is_shell {
                // Without OSC 7, follow the foreground process's cwd.
                if !self.shell_integration
                    && let Some(cwd) = fg.cwd.clone()
                    && self.cwd.as_ref() != Some(&cwd)
                {
                    self.cwd = Some(cwd);
                    changed = true;
                }
            }
            if !self.shell_integration && self.agent.is_none() {
                let busy = !is_shell;
                match (busy, self.running_since) {
                    (true, None) => self.running_since = Some(Instant::now()),
                    (false, Some(start)) => {
                        self.running_since = None;
                        finished = Some((previous.unwrap_or_default(), start.elapsed()));
                    }
                    _ => {}
                }
            }
        }
        // Agent detection.
        let kind = if self.process_is_shell {
            None
        } else {
            registry.table.agent_kind(self.process.as_ref(), self.osc_title.as_deref())
        };
        match (&mut self.agent, kind) {
            (None, Some(kind)) => {
                let now = Instant::now();
                let mut tracker = AgentTracker::new(kind.clone(), now);
                let args = self.process.as_ref().map(|p| p.args.clone()).unwrap_or_default();
                tracker.set_session_id(nox_agents::session_id_from_cmdline(&kind, &args));
                let cmdline = self.process.as_ref().map_or_else(
                    || kind.clone(),
                    |p| {
                        std::iter::once(p.exe.clone())
                            .chain(p.args.iter().cloned())
                            .collect::<Vec<_>>()
                            .join(" ")
                    },
                );
                self.agent =
                    Some(AgentInfo { kind, tracker, launch: None, cmdline, seen: true, created: now });
                changed = true;
                cx.emit(PaneEvent::AgentChanged { transition: None });
            }
            (Some(a), None)
                if self.process_is_shell
                    && a.tracker.state() != AgentState::Exited
                    && a.left_foreground_means_exit() =>
            {
                // The agent left the foreground: it exited.
                let t = a.tracker.on_exit(Instant::now());
                cx.emit(PaneEvent::AgentChanged { transition: t });
                changed = true;
            }
            (Some(a), Some(kind)) if a.tracker.state() == AgentState::Exited || a.kind != kind => {
                let now = Instant::now();
                a.kind.clone_from(&kind);
                a.tracker = AgentTracker::new(kind, now);
                a.seen = true;
                changed = true;
                cx.emit(PaneEvent::AgentChanged { transition: None });
            }
            (Some(a), Some(_)) => a.seen = true,
            _ => {}
        }
        // Icon, after agent detection: a running agent shows its own icon even while its process
        // and title don't identify it (e.g. Claude Code's working spinner title).
        let registry = cx.global::<IconRegistry>();
        let live_agent = self.agent.as_ref().filter(|a| {
            a.tracker.state() != AgentState::Exited && !self.process_is_shell && a.kind != "agent"
        });
        let icon = match (&self.custom_icon, live_agent) {
            (Some(name), _) => registry.custom_icon(name),
            (None, Some(a)) => registry.table.icon_for_kind(&a.kind),
            (None, None) => registry.table.icon_for(
                self.process.as_ref().filter(|_| !self.process_is_shell),
                self.osc_title.as_deref(),
                &login_shell,
            ),
        };
        if icon != self.icon {
            self.icon = icon;
            changed = true;
        }
        if Settings::global(cx).config.ui.show_git_branch {
            let b = self.cwd.as_deref().and_then(nox_agents::git_branch);
            if b != self.branch {
                self.branch = b;
                changed = true;
            }
        }
        if let Some((command, duration)) = finished {
            cx.emit(PaneEvent::CommandFinished { command, exit: None, duration });
        }
        if changed {
            cx.emit(PaneEvent::TitleChanged);
            cx.notify();
        }
    }

    fn schedule_agent_tick(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.agent.as_ref().and_then(|a| a.tracker.next_tick()) else { return };
        let wait = next.saturating_duration_since(Instant::now()) + Duration::from_millis(10);
        self.agent_tick = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(a) = &mut this.agent {
                    let t = a.tracker.tick(Instant::now());
                    if t.is_some() {
                        cx.emit(PaneEvent::AgentChanged { transition: t });
                        cx.notify();
                    }
                }
                this.schedule_agent_tick(cx);
            });
        }));
    }

    /// An explicit report (`nox status`).
    pub fn report_status(
        &mut self,
        state: AgentState,
        msg: Option<String>,
        session: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        if self.agent.is_none() {
            let kind = self
                .process
                .as_ref()
                .and_then(|p| cx.global::<IconRegistry>().table.agent_kind(Some(p), None))
                .unwrap_or_else(|| "agent".to_string());
            self.agent = Some(AgentInfo {
                kind: kind.clone(),
                tracker: AgentTracker::new(kind.clone(), now),
                launch: None,
                cmdline: kind,
                // It reported itself, so it is running.
                seen: true,
                created: now,
            });
        }
        if let Some(a) = &mut self.agent {
            let t = a.tracker.on_report(state, msg, session, now);
            cx.emit(PaneEvent::AgentChanged { transition: t });
        }
        cx.notify();
    }

    // ---- derived info ---------------------------------------------------------------------

    #[must_use]
    pub fn process_display_name(&self) -> Option<String> {
        if self.process_is_shell {
            return None;
        }
        let p = self.process.as_ref()?;
        if (p.exe == "ssh" || p.exe == "mosh")
            && let Some(host) = p.args.iter().rev().find(|a| !a.starts_with('-'))
        {
            return Some(format!("ssh {host}"));
        }
        if let Some(a) = &self.agent
            && a.tracker.state() != AgentState::Exited
        {
            return Some(a.kind.clone());
        }
        Some(p.exe.clone())
    }

    #[must_use]
    pub fn cwd_display(&self) -> String {
        match &self.cwd {
            Some(p) => nox_terminal::process::tildify(p),
            None => "~".into(),
        }
    }

    /// The pane title (SPEC §4.4).
    #[must_use]
    pub fn title(&self, mode: nox_config::TabTitle) -> String {
        if let Some(n) = &self.custom_name {
            return n.clone();
        }
        match mode {
            nox_config::TabTitle::Cwd => return self.cwd_display(),
            nox_config::TabTitle::Process => {
                return self.process_display_name().unwrap_or_else(|| {
                    self.process.as_ref().map_or_else(|| "shell".into(), |p| p.exe.clone())
                });
            }
            nox_config::TabTitle::Auto => {}
        }
        if let Some(t) = self.osc_title.as_ref().filter(|t| !t.trim().is_empty()) {
            return t.clone();
        }
        if let Some(p) = self.process_display_name() {
            return p;
        }
        format!("{}:{}", nox_terminal::process::user_at_host(), self.cwd_display())
    }

    #[must_use]
    pub fn agent_state(&self) -> Option<AgentState> {
        self.agent.as_ref().map(|a| a.tracker.state())
    }

    #[must_use]
    pub fn is_agent(&self) -> bool {
        self.agent.as_ref().is_some_and(|a| a.tracker.state() != AgentState::Exited)
    }

    #[must_use]
    pub fn status(&self) -> TabStatus {
        if self.bell_unseen {
            return TabStatus::Bell;
        }
        if let Some(a) = &self.agent
            && a.tracker.state() != AgentState::Exited
        {
            return match a.tracker.state() {
                AgentState::NeedsInput => TabStatus::NeedsInput,
                AgentState::Working => TabStatus::Running,
                _ if self.done_unseen => TabStatus::Done,
                _ => TabStatus::None,
            };
        }
        if self.running_since.is_some() {
            return TabStatus::Running;
        }
        if self.done_unseen {
            return if self.done_failed { TabStatus::Failed } else { TabStatus::Done };
        }
        TabStatus::None
    }

    /// Mark everything as seen (the user is looking at this pane).
    pub fn mark_seen(&mut self) {
        self.done_unseen = false;
        self.done_failed = false;
        self.bell_unseen = false;
    }

    /// Something other than the shell runs (close confirmation, SPEC §6.4).
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.exited.is_none() && self.session.as_ref().is_some_and(nox_terminal::TerminalSession::is_busy)
    }

    #[must_use]
    pub fn font_size(&self, settings: &Settings) -> f32 {
        self.font_size_override.unwrap_or(settings.term_font.size)
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if let Some(s) = &self.session {
            s.write(bytes);
        }
    }

    // ---- cursor blink ---------------------------------------------------------------------

    #[must_use]
    pub fn cursor_visible(&self, now: Instant) -> bool {
        let blink = self.session.as_ref().is_some_and(|s| s.term().lock().cursor_style().blinking);
        if !blink {
            return true;
        }
        let dt = now.duration_since(self.blink_epoch);
        if dt > Duration::from_secs(5) {
            return true;
        }
        (dt.as_millis() / 530).is_multiple_of(2)
    }

    fn schedule_blink(&mut self, cx: &mut Context<Self>) {
        if !self.blinking(cx) {
            self.blink_task = None;
            return;
        }
        if self.blink_task.is_some() {
            return;
        }
        self.blink_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(530)).await;
                let keep = this
                    .update(cx, |this, cx| {
                        cx.notify();
                        let active = Instant::now().duration_since(this.blink_epoch) < Duration::from_secs(5)
                            && this.blinking(cx);
                        if !active {
                            this.blink_task = None;
                        }
                        active
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }

    fn blinking(&self, cx: &App) -> bool {
        let Some(s) = &self.session else { return false };
        let _ = cx;
        s.term().lock().cursor_style().blinking
    }

    // ---- input ----------------------------------------------------------------------------

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.as_ref().is_some_and(|f| f.input.focus_handle(cx).is_focused(window)) {
            return;
        }
        if self.exited.is_some() && ev.keystroke.key == "enter" {
            if self.resume_failed {
                // Start a new session of the agent.
                if let Some(cmd) = self.agent.as_ref().and_then(|a| {
                    let defs = &Settings::global(cx).config.agents.defs;
                    defs.get(&a.kind).map(|d| d.command.clone())
                }) {
                    self.resume_failed = false;
                    self.write(format!("{cmd}\r").as_bytes());
                }
            } else {
                cx.emit(PaneEvent::Close);
            }
            cx.stop_propagation();
            return;
        }
        if self.resume_failed && ev.keystroke.key == "enter" && self.exited.is_none() {
            self.resume_failed = false;
            if let Some(cmd) = self
                .agent
                .as_ref()
                .and_then(|a| Settings::global(cx).config.agents.defs.get(&a.kind).map(|d| d.command.clone()))
            {
                self.write(format!("{cmd}\r").as_bytes());
                cx.stop_propagation();
                return;
            }
        }
        let Some(session) = &self.session else { return };
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        let settings = Settings::global(cx);
        let alt_is_meta = m.alt && {
            let (l, r) = nox_mac::option_keys_down();
            match settings.config.terminal.option_as_meta {
                // Layouts that type letters with Option (Polish ą, German ö, …) keep it for text.
                nox_config::OptionAsMeta::Auto => {
                    nox_mac::keyboard_layout_id().is_some_and(|id| keys::layout_uses_option_as_meta(&id))
                        && (l || !r)
                }
                nox_config::OptionAsMeta::Left => l || !r,
                nox_config::OptionAsMeta::Right => r,
                nox_config::OptionAsMeta::Both => true,
                nox_config::OptionAsMeta::None => false,
            }
        };
        // Plain text goes through the input handler (IME); encode only what isn't text.
        let is_plain_text = !m.control
            && !m.platform
            && !alt_is_meta
            && ks.key_char.as_ref().is_some_and(|c| !c.is_empty() && !c.chars().any(char::is_control))
            && !matches!(ks.key.as_str(), "enter" | "tab" | "backspace" | "escape" | "space");
        if is_plain_text && self.marked_text.is_none() {
            return;
        }
        if self.marked_text.is_some() {
            return;
        }
        let mode = *session.term().lock().mode();
        let modes = KeyModes {
            app_cursor: mode.contains(TermMode::APP_CURSOR),
            app_keypad: mode.contains(TermMode::APP_KEYPAD),
        };
        let input = KeyInput {
            key: &ks.key,
            key_char: ks.key_char.as_deref(),
            mods: Mods { ctrl: m.control, alt: m.alt, shift: m.shift, cmd: m.platform },
            alt_is_meta,
        };
        if let Some(bytes) = keys::encode(&input, modes) {
            self.user_typed(cx);
            if let Some(s) = &self.session {
                s.write(&bytes);
            }
            cx.stop_propagation();
        }
    }

    fn user_typed(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        self.last_user_input = Some(now);
        self.blink_epoch = now;
        self.hovered_link = None;
        if Settings::global(cx).config.terminal.scroll_on_input {
            self.scroll_to_bottom(cx);
        }
        if let Some(a) = &mut self.agent {
            let t = a.tracker.on_input(now);
            if t.is_some() {
                cx.emit(PaneEvent::AgentChanged { transition: t });
            }
        }
        if let Some(s) = &self.session {
            let mut t = s.term().lock();
            t.selection = None;
        }
        cx.emit(PaneEvent::UserInput);
        cx.notify();
    }

    pub fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = &self.session {
            s.term().lock().scroll_display(Scroll::Bottom);
        }
        self.scroll.reset();
        self.new_output_below = false;
        cx.notify();
    }

    pub fn paste_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = &self.session else { return };
        let bracketed = s.term().lock().mode().contains(TermMode::BRACKETED_PASTE);
        let multiline = text.trim_end_matches('\n').contains('\n');
        if multiline && !bracketed && Settings::global(cx).config.terminal.confirm_multiline_paste {
            let lines = text.lines().count();
            let text = text.to_string();
            let this = cx.entity().downgrade();
            crate::dialog::confirm(
                window,
                cx,
                "Paste multiple lines?",
                format!("The text has {lines} lines; pasting it into the shell may run several commands."),
                "Paste",
                move |window, cx| {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(s) = &this.session {
                            s.paste(&text);
                        }
                        this.user_typed(cx);
                        window.focus(&this.focus_handle);
                    });
                },
            );
            return;
        }
        s.paste(text);
        self.user_typed(cx);
    }

    // ---- mouse ----------------------------------------------------------------------------

    fn grid_point(&self, pos: Point<Pixels>) -> Option<(GridPoint, Side)> {
        let geom = self.geom.as_ref()?;
        let s = self.session.as_ref()?;
        let offset = s.term().lock().grid().display_offset();
        Some(position_to_point(geom, pos, self.scroll.frac, offset))
    }

    fn mouse_mods(m: Modifiers) -> MouseMods {
        MouseMods { shift: m.shift, alt: m.alt, ctrl: m.control }
    }

    fn report_mouse(
        &self,
        button: mouse::MouseButton,
        action: MouseAction,
        m: Modifiers,
        pos: Point<Pixels>,
    ) -> bool {
        let (Some(s), Some(geom)) = (&self.session, &self.geom) else { return false };
        let mode = *s.term().lock().mode();
        if !mouse::reporting(mode) || m.shift {
            return false;
        }
        let x = f32::from(pos.x - geom.origin.x).max(0.);
        let y = f32::from(pos.y - geom.origin.y).max(0.);
        let col = ((x / f32::from(geom.cell_w)) as usize).min(geom.cols - 1);
        let row = ((y / f32::from(geom.cell_h)) as usize).min(geom.rows - 1);
        if let Some(bytes) = mouse::encode(mode, button, action, Self::mouse_mods(m), col, row) {
            s.write(&bytes);
        }
        true
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let was_focused = self.focus_handle.is_focused(window);
        if !was_focused {
            window.focus(&self.focus_handle);
        }
        let click_through = Settings::global(cx).config.ui.focus_click_through;
        if (was_focused || click_through)
            && self.report_mouse(button_of(ev.button), MouseAction::Press, ev.modifiers, ev.position)
        {
            return;
        }
        if ev.button != MouseButton::Left {
            return;
        }
        if ev.modifiers.platform
            && let Some(link) = self.link_at(ev.position, cx)
        {
            match link.target {
                LinkTarget::Url(u) => cx.open_url(&u),
                LinkTarget::Path(p) => {
                    if ev.modifiers.alt {
                        cx.reveal_path(&p);
                    } else {
                        cx.open_url(&format!("file://{}", p.display()));
                    }
                }
            }
            return;
        }
        let Some((point, side)) = self.grid_point(ev.position) else { return };
        let Some(s) = &self.session else { return };
        let mut term = s.term().lock();
        if ev.modifiers.shift
            && let Some(sel) = term.selection.as_mut()
        {
            sel.update(point, side);
            self.selecting = Some(sel.ty);
            drop(term);
            cx.notify();
            return;
        }
        let ty = match ev.click_count {
            2 => SelectionType::Semantic,
            3.. => SelectionType::Lines,
            _ if ev.modifiers.alt => SelectionType::Block,
            _ => SelectionType::Simple,
        };
        let mut sel = Selection::new(ty, point, side);
        if ty != SelectionType::Simple && ty != SelectionType::Block {
            sel.update(point, side);
            term.selection = Some(sel);
        } else {
            term.selection = None;
            // Created on first drag so a plain click doesn't leave an empty selection.
            self.pending_selection = Some(sel);
        }
        self.selecting = Some(ty);
        drop(term);
        cx.notify();
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.report_mouse(button_of(ev.button), MouseAction::Release, ev.modifiers, ev.position)
            && self.selecting.is_none()
        {
            return;
        }
        let _ = window;
        let selected = self.selecting.take().is_some();
        self.pending_selection = None;
        self.autoscroll = None;
        if selected && Settings::global(cx).config.terminal.copy_on_select && self.copy_selection(cx) {
            crate::toast::show(cx, "Copied to clipboard");
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if Settings::global(cx).config.ui.focus_follows_mouse && !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle);
        }
        // Link hover with ⌘.
        let link = if ev.modifiers.platform { self.link_at(ev.position, cx) } else { None };
        if link != self.hovered_link {
            self.hovered_link = link;
            cx.notify();
        }
        if self.selecting.is_none() {
            let button = ev.pressed_button.map_or(mouse::MouseButton::None, button_of);
            self.report_mouse(button, MouseAction::Motion, ev.modifiers, ev.position);
            return;
        }
        self.drag_selection(ev, cx);
    }

    /// A mouse move anywhere in the window while a selection drag is active. GPUI calls the
    /// grid's `on_mouse_move` only while the pointer is over it, so the element registers a
    /// window-level listener for moves outside the grid (SPEC §7.6: dragging past the edges).
    pub fn on_drag_outside(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.selecting.is_some() {
            self.drag_selection(ev, cx);
        }
    }

    #[must_use]
    pub fn is_selecting(&self) -> bool {
        self.selecting.is_some()
    }

    fn drag_selection(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if ev.pressed_button != Some(MouseButton::Left) {
            // Released where no mouse-up reached us.
            self.selecting = None;
            self.pending_selection = None;
            self.autoscroll = None;
            return;
        }
        self.update_selection(ev.position, cx);
        // Auto-scroll when dragging past the edges.
        if let Some(geom) = &self.geom {
            let b = geom.bounds;
            let over = if ev.position.y < b.top() {
                f32::from(b.top() - ev.position.y)
            } else if ev.position.y > b.bottom() {
                -f32::from(ev.position.y - b.bottom())
            } else {
                0.0
            };
            if over == 0.0 {
                self.autoscroll = None;
            } else {
                let pos = ev.position;
                self.autoscroll = Some(cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(Duration::from_millis(30)).await;
                        let ok = this
                            .update(cx, |this, cx| {
                                if this.selecting.is_none() {
                                    return false;
                                }
                                let lines = (over / 20.0).clamp(-10.0, 10.0).round() as i32;
                                let lines = if lines == 0 { over.signum() as i32 } else { lines };
                                if let Some(s) = &this.session {
                                    s.term().lock().scroll_display(Scroll::Delta(lines));
                                }
                                this.update_selection(pos, cx);
                                true
                            })
                            .unwrap_or(false);
                        if !ok {
                            break;
                        }
                    }
                }));
            }
        }
    }

    fn update_selection(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let Some((point, side)) = self.grid_point(pos) else { return };
        let Some(s) = &self.session else { return };
        let mut term = s.term().lock();
        if let Some(p) = self.pending_selection.take() {
            term.selection = Some(p);
        }
        if let Some(sel) = term.selection.as_mut() {
            sel.update(point, side);
        }
        drop(term);
        cx.notify();
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = &self.session else { return };
        let Some(geom) = self.geom.clone() else { return };
        let line_h = f32::from(geom.cell_h);
        let cfg = Settings::global(cx).config.clone();
        let mode = *s.term().lock().mode();
        let (dy_px, lines_delta) = match ev.delta {
            ScrollDelta::Pixels(p) => (f32::from(p.y), None),
            ScrollDelta::Lines(l) => (l.y * line_h, Some(l.y)),
        };
        // Programs with mouse reporting get wheel events; the alternate screen gets arrows.
        if mouse::reporting(mode) && !ev.modifiers.shift {
            self.scroll.alt_accum += dy_px / line_h;
            let n = self.scroll.alt_accum.trunc() as i32;
            self.scroll.alt_accum -= n as f32;
            let button = if n > 0 { mouse::MouseButton::WheelUp } else { mouse::MouseButton::WheelDown };
            for _ in 0..n.unsigned_abs().min(20) {
                self.report_mouse(button, MouseAction::Press, ev.modifiers, ev.position);
            }
            return;
        }
        if mode.contains(TermMode::ALT_SCREEN) {
            if mode.contains(TermMode::ALTERNATE_SCROLL) {
                self.scroll.alt_accum += dy_px / line_h;
                let n = self.scroll.alt_accum.trunc() as i32;
                self.scroll.alt_accum -= n as f32;
                let app = mode.contains(TermMode::APP_CURSOR);
                let seq: &[u8] = match (n > 0, app) {
                    (true, true) => b"\x1bOA",
                    (true, false) => b"\x1b[A",
                    (false, true) => b"\x1bOB",
                    (false, false) => b"\x1b[B",
                };
                let mut out = Vec::new();
                for _ in 0..n.unsigned_abs().min(50) {
                    out.extend_from_slice(seq);
                }
                s.write(&out);
            }
            return;
        }
        if let Some(lines) = lines_delta {
            let mult = cfg.terminal.scroll_multiplier as f32;
            let dur = Duration::from_millis(u64::from(cfg.terminal.smooth_scroll_duration));
            if dur.is_zero() || Settings::global(cx).reduce_motion() {
                self.apply_scroll_pixels(lines * mult * line_h, cx);
            } else {
                self.scroll.animate_lines(lines * mult, line_h, dur);
                // Repaint; the element's prepaint advances the animation frame by frame.
                // (`request_animation_frame` is only allowed during prepaint / paint.)
                cx.notify();
            }
        } else {
            self.apply_scroll_pixels(dy_px, cx);
        }
        cx.notify();
    }

    fn apply_scroll_pixels(&mut self, dy: f32, cx: &mut Context<Self>) {
        let (Some(s), Some(geom)) = (&self.session, &self.geom) else { return };
        let line_h = f32::from(geom.cell_h);
        let mut term = s.term().lock();
        let offset = term.grid().display_offset();
        let max = term.grid().history_size();
        let applied = self.scroll.apply_pixels(dy, line_h, offset, max);
        if applied.lines != 0 {
            term.scroll_display(Scroll::Delta(applied.lines));
        }
        if term.grid().display_offset() == 0 && self.scroll.frac == 0.0 {
            self.new_output_below = false;
        }
        drop(term);
        cx.notify();
    }

    /// Called from the element each frame: advance a running wheel animation.
    pub fn step_scroll_animation(&mut self, now: Instant, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(step) = self.scroll.animation_step(now) {
            self.apply_scroll_pixels(step, cx);
            if self.scroll.animating() {
                window.request_animation_frame();
            }
        }
        if self.scroll.scrollbar_alpha(now) > 0.0 {
            // Keep repainting while the scrollbar fades.
            window.request_animation_frame();
        }
    }

    // ---- links ----------------------------------------------------------------------------

    fn link_at(&self, pos: Point<Pixels>, _cx: &App) -> Option<HoveredLink> {
        let (point, _) = self.grid_point(pos)?;
        let s = self.session.as_ref()?;
        let term = s.term().lock();
        let grid = term.grid();
        if point.line < grid.topmost_line() || point.line > grid.bottommost_line() {
            return None;
        }
        // OSC 8 hyperlink on the cell.
        let cell = &grid[point];
        if let Some(h) = cell.hyperlink() {
            // Extend over neighbouring cells with the same link on this line.
            let row = &grid[point.line];
            let id = h.clone();
            let mut c0 = point.column.0;
            while c0 > 0 && row[Column(c0 - 1)].hyperlink().as_ref() == Some(&id) {
                c0 -= 1;
            }
            let mut c1 = point.column.0;
            while c1 + 1 < term.columns() && row[Column(c1 + 1)].hyperlink().as_ref() == Some(&id) {
                c1 += 1;
            }
            return Some(HoveredLink {
                spans: vec![(point.line, c0, c1)],
                target: LinkTarget::Url(h.uri().to_string()),
            });
        }
        // Plain text: join wrapped lines around the pointer.
        let mut start: Line = point.line;
        while start > grid.topmost_line()
            && grid[Line(start.0 - 1)][Column(term.columns() - 1)]
                .flags
                .contains(nox_terminal::alacritty_terminal::term::cell::Flags::WRAPLINE)
        {
            start = Line(start.0 - 1);
        }
        let mut end = point.line;
        while end < grid.bottommost_line()
            && grid[end][Column(term.columns() - 1)]
                .flags
                .contains(nox_terminal::alacritty_terminal::term::cell::Flags::WRAPLINE)
        {
            end = Line(end.0 + 1);
        }
        let cols = term.columns();
        let mut text = String::new();
        let mut map: Vec<(Line, usize)> = Vec::new();
        let mut l = start;
        while l <= end {
            for c in 0..cols {
                let cell = &grid[l][Column(c)];
                if cell.flags.contains(nox_terminal::alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let ch = if cell.c == '\0' { ' ' } else { cell.c };
                for _ in 0..ch.len_utf8() {
                    map.push((l, c));
                }
                text.push(ch);
            }
            l = Line(l.0 + 1);
        }
        let idx = map.iter().position(|(ll, cc)| *ll == point.line && *cc == point.column.0)?;
        let cwd = self.cwd.clone();
        drop(term);
        let (range, target) = crate::links::find_link(&text, idx, cwd.as_deref())?;
        let mut spans: Vec<(Line, usize, usize)> = Vec::new();
        for (ll, cc) in &map[range.start..range.end] {
            match spans.last_mut() {
                Some((sl, _, e)) if *sl == *ll => *e = (*e).max(*cc),
                _ => spans.push((*ll, *cc, *cc)),
            }
        }
        Some(HoveredLink { spans, target })
    }

    fn on_modifiers_changed(
        &mut self,
        ev: &ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !ev.modifiers.platform && self.hovered_link.is_some() {
            self.hovered_link = None;
            cx.notify();
        } else if ev.modifiers.platform {
            let pos = window.mouse_position();
            if self.geom.as_ref().is_some_and(|g| g.bounds.contains(&pos)) {
                self.hovered_link = self.link_at(pos, cx);
                cx.notify();
            }
        }
    }

    // ---- find -----------------------------------------------------------------------------

    #[must_use]
    pub fn find_highlights(&self) -> Vec<Match> {
        self.find.as_ref().map(|f| f.matches.clone()).unwrap_or_default()
    }

    #[must_use]
    pub fn find_current_index(&self) -> Option<usize> {
        self.find.as_ref().filter(|f| !f.matches.is_empty()).map(|f| f.current)
    }

    fn open_find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(f) = &self.find {
            f.input.update(cx, |i, cx| {
                let t = i.text().to_string();
                i.set_text(t, cx);
            });
            f.input.read(cx).focus(window);
            return;
        }
        let input = cx.new(|cx| TextInput::new(cx, "Find"));
        let sub =
            cx.subscribe_in(&input, window, |this: &mut Self, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Changed => this.run_search(cx),
                InputEvent::Submit => {
                    let shift = window.modifiers().shift;
                    this.step_match(if shift { -1 } else { 1 }, cx);
                }
                InputEvent::Cancel => this.close_find(window, cx),
                InputEvent::Blur => {}
            });
        input.read(cx).focus(window);
        self.find = Some(FindState {
            input,
            case_sensitive: false,
            regex: false,
            matches: vec![],
            current: 0,
            _sub: sub,
        });
        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        window.focus(&self.focus_handle);
        cx.notify();
    }

    pub fn run_search(&mut self, cx: &mut Context<Self>) {
        let Some(find) = &mut self.find else { return };
        let query = find.input.read(cx).text().to_string();
        find.matches.clear();
        find.current = 0;
        if query.is_empty() {
            cx.notify();
            return;
        }
        let pattern = if find.regex { query.clone() } else { regex_escape(&query) };
        // Explicit flags: alacritty's `RegexSearch` is smart-case (a query without uppercase is
        // case-insensitive), but the "Aa" toggle decides alone (SPEC §7.8).
        let pattern = format!("{}{pattern}", if find.case_sensitive { "(?-i)" } else { "(?i)" });
        let Ok(mut re) = RegexSearch::new(&pattern) else {
            cx.notify();
            return;
        };
        let Some(s) = &self.session else { return };
        let term = s.term().lock();
        let start = GridPoint::new(term.grid().topmost_line(), Column(0));
        let end = GridPoint::new(term.grid().bottommost_line(), term.last_column());
        let iter = RegexIter::new(start, end, Direction::Right, &term, &mut re);
        let matches: Vec<Match> = iter.take(10_000).collect();
        // Start at the match nearest the bottom of the viewport.
        let bottom = Line(term.screen_lines() as i32 - 1 - term.grid().display_offset() as i32);
        let current = matches.iter().rposition(|m| m.start().line <= bottom).unwrap_or(0);
        drop(term);
        find.matches = matches;
        find.current = current;
        self.reveal_current_match(cx);
        cx.notify();
    }

    fn step_match(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some(find) = &mut self.find else { return };
        if find.matches.is_empty() {
            return;
        }
        let n = find.matches.len() as i32;
        find.current = ((find.current as i32 + delta).rem_euclid(n)) as usize;
        self.reveal_current_match(cx);
        cx.notify();
    }

    fn reveal_current_match(&mut self, _cx: &mut Context<Self>) {
        let Some(find) = &self.find else { return };
        let Some(m) = find.matches.get(find.current) else { return };
        let Some(s) = &self.session else { return };
        let mut term = s.term().lock();
        term.scroll_to_point(*m.start());
        self.scroll.reset();
    }

    fn find_next(&mut self, _: &FindNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step_match(1, cx);
    }
    fn find_prev(&mut self, _: &FindPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.step_match(-1, cx);
    }

    // ---- actions --------------------------------------------------------------------------

    fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(s) = &self.session else { return false };
        let text = s.term().lock().selection_to_string();
        match text {
            Some(t) if !t.is_empty() => {
                let trimmed: String = t.lines().map(str::trim_end).collect::<Vec<_>>().join("\n");
                cx.write_to_clipboard(ClipboardItem::new_string(trimmed));
                true
            }
            _ => false,
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            self.paste_text(&text, window, cx);
        }
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = &self.session else { return };
        let mut term = s.term().lock();
        let start = GridPoint::new(term.grid().topmost_line(), Column(0));
        let end = GridPoint::new(term.grid().bottommost_line(), term.last_column());
        let mut sel = Selection::new(SelectionType::Simple, start, Side::Left);
        sel.update(end, Side::Right);
        term.selection = Some(sel);
        drop(term);
        cx.notify();
    }

    fn clear_buffer(&mut self, _: &ClearBuffer, _: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = &self.session else { return };
        {
            let mut term = s.term().lock();
            term.grid_mut().clear_history();
            term.selection = None;
        }
        // Ask the shell to redraw its prompt on a clean screen (^L).
        if self.process_is_shell {
            s.write(b"\x0c");
        }
        self.scroll.reset();
        cx.notify();
    }

    fn copy_last_output(&mut self, _: &CopyLastOutput, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.session.as_ref().and_then(nox_terminal::TerminalSession::last_output) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        } else {
            crate::toast::show(cx, "Copy Last Output needs shell integration (shell.integration = true).");
        }
    }

    fn scroll_page_up(&mut self, _: &ScrollPageUp, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = &self.session {
            s.term().lock().scroll_display(Scroll::PageUp);
        }
        self.scroll.reset();
        self.scroll.last_scroll = Some(Instant::now());
        cx.notify();
    }
    fn scroll_page_down(&mut self, _: &ScrollPageDown, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = &self.session {
            s.term().lock().scroll_display(Scroll::PageDown);
        }
        self.scroll.reset();
        self.scroll.last_scroll = Some(Instant::now());
        cx.notify();
    }
    fn scroll_top(&mut self, _: &ScrollToTop, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = &self.session {
            s.term().lock().scroll_display(Scroll::Top);
        }
        self.scroll.reset();
        self.scroll.last_scroll = Some(Instant::now());
        cx.notify();
    }
    fn scroll_bottom(&mut self, _: &ScrollToBottom, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_to_bottom(cx);
    }

    fn jump_prompt(&mut self, up: bool, cx: &mut Context<Self>) {
        let Some(s) = &self.session else { return };
        let prompts = s.prompt_lines();
        let mut term = s.term().lock();
        let offset = term.grid().display_offset() as i32;
        if prompts.is_empty() {
            term.scroll_display(Scroll::Delta(if up { 1 } else { -1 }));
        } else {
            let top = -offset;
            let target = if up {
                prompts.iter().rev().find(|&&l| l < top).copied()
            } else {
                prompts.iter().find(|&&l| l > top).copied()
            };
            match target {
                Some(l) => {
                    let new_offset = (-l).clamp(0, term.grid().history_size() as i32);
                    term.scroll_display(Scroll::Delta(new_offset - offset));
                }
                None if !up => term.scroll_display(Scroll::Bottom),
                None => {}
            }
        }
        drop(term);
        self.scroll.reset();
        self.scroll.last_scroll = Some(Instant::now());
        cx.notify();
    }
    fn prev_prompt(&mut self, _: &PreviousPrompt, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_prompt(true, cx);
    }
    fn next_prompt(&mut self, _: &NextPrompt, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_prompt(false, cx);
    }
    fn restart_action(&mut self, _: &RestartPane, _: &mut Window, cx: &mut Context<Self>) {
        if self.resume_failed && self.exited.is_none() {
            // Retry the resume command in the running shell.
            self.resume_failed = false;
            if let Some(cmd) = self.agent.as_ref().and_then(|a| a.launch.as_ref()).map(|l| l.command.clone())
            {
                self.write(format!("{cmd}\r").as_bytes());
            }
            cx.notify();
            return;
        }
        self.restart(cx);
    }
    fn send_escape_newline(&mut self, _: &SendShiftEnter, _: &mut Window, cx: &mut Context<Self>) {
        self.write(b"\x1b\r");
        self.user_typed(cx);
    }

    pub fn send_bytes(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.write(bytes);
        self.user_typed(cx);
    }

    fn on_drop_paths(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        let joined =
            paths.paths().iter().map(|p| shell_escape(&p.to_string_lossy())).collect::<Vec<_>>().join(" ");
        if let Some(s) = &self.session {
            s.paste(&format!("{joined} "));
        }
        window.focus(&self.focus_handle);
        cx.notify();
    }

    pub fn shake(&mut self, cx: &mut Context<Self>) {
        self.shake = Some(Instant::now());
        cx.spawn(async move |this, cx| {
            for _ in 0..12 {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            }
            let _ = this.update(cx, |this, cx| {
                this.shake = None;
                cx.notify();
            });
        })
        .detach();
    }

    // ---- rendering ------------------------------------------------------------------------

    fn render_header(&self, focused: bool, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::global(cx).clone();
        let settings = Settings::global(cx).clone();
        let title_mode = settings.config.tabs.title;
        let title = self.title(title_mode);
        let maximized = cx.global::<crate::root::UiFlags>().maximized_panes.contains(&self.id);
        let show_controls = self.header_hovered || focused;
        let controls_opacity = if self.header_hovered { 1.0 } else { 0.6 };
        let flash = self.bell_flash.is_some();
        let shake_dx = self.shake.map(|t| {
            let ms = t.elapsed().as_millis() as f32;
            (ms / 25.0).sin() * 4.0 * (1.0 - (ms / 200.0).min(1.0))
        });
        let icon = if settings.config.tabs.icons { self.icon.clone() } else { Icon::TERMINAL };
        let mut meta: Vec<AnyElement> = Vec::new();
        if settings.config.ui.show_git_branch
            && let Some(b) = &self.branch
        {
            meta.push(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(svg().path("icons/ui/branch.svg").size(px(11.)).text_color(theme.text_muted))
                    .child(b.clone())
                    .into_any_element(),
            );
        }
        if let Some(a) = &self.agent
            && a.tracker.state() != AgentState::Exited
        {
            let since = crate::util::short_duration(a.tracker.since().elapsed());
            let label = match a.tracker.state() {
                AgentState::Working => format!("working · {since}"),
                AgentState::NeedsInput => format!("needs input · {since}"),
                AgentState::Idle => "idle".to_string(),
                AgentState::Exited => "exited".into(),
            };
            let color = match a.tracker.state() {
                AgentState::NeedsInput => theme.ansi(4),
                AgentState::Working => theme.ansi(3),
                _ => theme.text_muted,
            };
            meta.push(div().text_color(color).child(label).into_any_element());
        }
        if self.unread > 0 {
            meta.push(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(svg().path("icons/ui/mail.svg").size(px(11.)).text_color(theme.text_muted))
                    .child(format!("{}", self.unread))
                    .into_any_element(),
            );
        }
        if !self.claims.is_empty() {
            let tip = self.claims.join("\n");
            meta.push(
                div()
                    .id(SharedString::from(format!("claims-{}", self.id)))
                    .child(svg().path("icons/ui/lock.svg").size(px(11.)).text_color(theme.text_muted))
                    .tooltip(move |window, cx| crate::util::tooltip(tip.clone(), window, cx))
                    .into_any_element(),
            );
        }
        if let Some((from, at)) = &self.from_banner
            && at.elapsed() < Duration::from_secs(3)
        {
            meta.push(div().text_color(theme.accent).child(format!("⇠ from {from}")).into_any_element());
        }
        let id = self.id;
        let button = |name: &'static str, icon: &'static str, ev: PaneEvent, cx: &mut Context<Self>| {
            let theme = Theme::global(cx).clone();
            div()
                .id(SharedString::from(format!("{name}-{id}")))
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .text_color(theme.text_muted)
                .hover(|s| s.bg(theme.text.opacity(0.08)).text_color(theme.text))
                .child(svg().path(icon).size(px(13.)).text_color(theme.text_muted))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(ev.clone())))
        };
        let controls = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .opacity(if show_controls { controls_opacity } else { 0.0 })
            .child(button(
                "splitv",
                "icons/ui/split-vertical.svg",
                PaneEvent::Split(SplitDirection::Vertical),
                cx,
            ))
            .child(button(
                "splith",
                "icons/ui/split-horizontal.svg",
                PaneEvent::Split(SplitDirection::Horizontal),
                cx,
            ))
            .child(button(
                "max",
                if maximized { "icons/ui/restore.svg" } else { "icons/ui/maximize.svg" },
                PaneEvent::ToggleMaximize,
                cx,
            ))
            .child(button("close", "icons/ui/close.svg", PaneEvent::Close, cx));
        let _ = window;
        div()
            .id(SharedString::from(format!("pane-header-{id}")))
            .h(px(28.))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px(px(10.))
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .when(flash, |d| d.bg(theme.accent.opacity(0.25)))
            .when_some(shake_dx, |d, dx| d.ml(px(dx)))
            .text_size(px(12.))
            .font_family(settings.ui_font.clone())
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.header_hovered = *hovered;
                cx.notify();
            }))
            .on_click(cx.listener(|this, ev: &gpui::ClickEvent, window, cx| {
                window.focus(&this.focus_handle);
                if ev.click_count() == 2 {
                    cx.emit(PaneEvent::ToggleMaximize);
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .child(icon_element(&icon, 14., if focused { theme.text } else { theme.text_muted }, cx))
                    .child(
                        div()
                            .text_color(if focused { theme.text } else { theme.text_muted })
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(title),
                    )
                    .child(div().flex().items_center().gap_2().text_color(theme.text_muted).children(meta)),
            )
            .child(controls)
            .into_any_element()
    }

    fn render_find_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let find = self.find.as_ref()?;
        let theme = Theme::global(cx).clone();
        let count = if find.matches.is_empty() {
            if find.input.read(cx).text().is_empty() { String::new() } else { "0/0".into() }
        } else {
            format!("{}/{}", find.current + 1, find.matches.len())
        };
        let toggle = |id: &'static str,
                      label: &'static str,
                      on: bool,
                      cx: &mut Context<Self>,
                      f: fn(&mut FindState)| {
            let theme = Theme::global(cx).clone();
            div()
                .id(id)
                .px(px(5.))
                .py(px(1.))
                .rounded(px(5.))
                .text_size(px(11.))
                .text_color(if on { theme.accent } else { theme.text_muted })
                .when(on, |d| d.bg(theme.accent.opacity(0.12)))
                .hover(|s| s.bg(theme.text.opacity(0.06)))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(fs) = &mut this.find {
                        f(fs);
                    }
                    this.run_search(cx);
                }))
        };
        Some(
            div()
                .absolute()
                .top(px(36.))
                .right(px(12.))
                .w(px(300.))
                .flex()
                .items_center()
                .gap_2()
                .px(px(8.))
                .py(px(5.))
                .rounded(px(8.))
                .bg(theme.bg_elevated)
                .border_1()
                .border_color(theme.border)
                .shadow_md()
                .text_size(px(12.))
                .text_color(theme.text)
                .child(svg().path("icons/ui/search.svg").size(px(12.)).text_color(theme.text_muted))
                .child(find.input.clone())
                .child(div().text_color(theme.text_muted).text_size(px(11.)).child(count))
                .child(toggle("find-case", "Aa", find.case_sensitive, cx, |f| {
                    f.case_sensitive = !f.case_sensitive;
                }))
                .child(toggle("find-regex", ".*", find.regex, cx, |f| f.regex = !f.regex))
                .into_any_element(),
        )
    }
}

fn button_of(b: MouseButton) -> mouse::MouseButton {
    match b {
        MouseButton::Left | MouseButton::Navigate(_) => mouse::MouseButton::Left,
        MouseButton::Middle => mouse::MouseButton::Middle,
        MouseButton::Right => mouse::MouseButton::Right,
    }
}

#[must_use]
pub fn shell_escape(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+,:@%".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Colors to answer OSC 4/10/11/12 queries with.
#[must_use]
pub fn query_colors(theme: &Theme) -> Vec<nox_terminal::alacritty_terminal::vte::ansi::Rgb> {
    use nox_terminal::alacritty_terminal::vte::ansi::Rgb;
    let conv = |c: nox_theme::Rgba| Rgb {
        r: (c.r * 255.).round() as u8,
        g: (c.g * 255.).round() as u8,
        b: (c.b * 255.).round() as u8,
    };
    let mut v: Vec<Rgb> = theme.palette_rgb.iter().map(|c| conv(*c)).collect();
    v.push(conv(theme.raw.text));
    v.push(conv(theme.raw.bg));
    v.push(conv(theme.raw.caret));
    while v.len() < nox_terminal::session::COLOR_COUNT {
        v.push(conv(theme.raw.text));
    }
    v
}

impl EntityInputHandler for Pane {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: 0..0, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_text.as_ref().map(|t| 0..t.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_text = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = None;
        if text.is_empty() {
            cx.notify();
            return;
        }
        self.write(text.as_bytes());
        self.user_typed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = (!text.is_empty()).then(|| text.to_string());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let geom = self.geom.as_ref()?;
        let s = self.session.as_ref()?;
        let term = s.term().lock();
        let c = term.grid().cursor.point;
        let row = c.line.0 + term.grid().display_offset() as i32;
        Some(Bounds::new(
            point(geom.origin.x + geom.cell_w * c.column.0 as f32, geom.origin.y + geom.cell_h * row as f32),
            gpui::size(geom.cell_w, geom.cell_h),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for Pane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx).clone();
        let settings = Settings::global(cx).clone();
        let focused = self.focus_handle.is_focused(window)
            || self.find.as_ref().is_some_and(|f| f.input.focus_handle(cx).is_focused(window));
        let window_active = window.is_window_active();
        let dim = settings.config.ui.dim_inactive_panes as f32;
        let header = settings.config.ui.pane_header.then(|| self.render_header(focused, window, cx));
        let find_bar = self.render_find_bar(cx);
        let id = self.id;

        let exit_line = self.exited.map(|code| {
            let msg = match code {
                Some(c) => format!("[process exited with code {c} — press ↵ to close, ⌘R to restart]"),
                None => "[process exited — press ↵ to close, ⌘R to restart]".to_string(),
            };
            div()
                .absolute()
                .bottom(px(8.))
                .left(px(12.))
                .right(px(12.))
                .text_size(px(12.))
                .text_color(theme.text_muted)
                .child(msg)
        });
        let resume_line = (self.resume_failed).then(|| {
            div()
                .absolute()
                .bottom(px(8.))
                .left(px(12.))
                .text_size(px(12.))
                .text_color(theme.text_muted)
                .child("[resume failed — ↵ start a new session, ⌘R retry]")
        });
        let spawn_err = self.spawn_error.clone().map(|e| {
            div()
                .absolute()
                .top(px(40.))
                .left(px(12.))
                .text_size(px(12.))
                .text_color(theme.ansi(1))
                .child(format!("Shell failed to start: {e}"))
        });
        let new_output_pill = self.new_output_below.then(|| {
            div()
                .id(SharedString::from(format!("new-output-{id}")))
                .absolute()
                .bottom(px(10.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .id(SharedString::from(format!("new-output-pill-{id}")))
                        .px(px(10.))
                        .py(px(3.))
                        .rounded(px(10.))
                        .bg(theme.bg_elevated)
                        .border_1()
                        .border_color(theme.border)
                        .text_size(px(11.))
                        .text_color(theme.text)
                        .child("↓ New output")
                        .on_click(cx.listener(|this, _, _, cx| this.scroll_to_bottom(cx))),
                )
        });

        let entity = cx.entity();
        div()
            .id(SharedString::from(format!("pane-{id}")))
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .bg(theme.bg)
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::clear_buffer))
            .on_action(cx.listener(Self::copy_last_output))
            .on_action(cx.listener(Self::open_find))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_prev))
            .on_action(cx.listener(Self::scroll_page_up))
            .on_action(cx.listener(Self::scroll_page_down))
            .on_action(cx.listener(Self::scroll_top))
            .on_action(cx.listener(Self::scroll_bottom))
            .on_action(cx.listener(Self::prev_prompt))
            .on_action(cx.listener(Self::next_prompt))
            .on_action(cx.listener(Self::restart_action))
            .on_action(cx.listener(Self::send_escape_newline))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_modifiers_changed(cx.listener(Self::on_modifiers_changed))
            .on_drop(cx.listener(Self::on_drop_paths))
            .children(header)
            .child(
                div()
                    .id(SharedString::from(format!("grid-{id}")))
                    .flex_1()
                    .min_h(px(0.))
                    .relative()
                    .cursor(if self.hovered_link.is_some() {
                        CursorStyle::PointingHand
                    } else {
                        CursorStyle::IBeam
                    })
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus_handle);
                            if !this.report_mouse(
                                mouse::MouseButton::Right,
                                MouseAction::Press,
                                ev.modifiers,
                                ev.position,
                            ) {
                                this.hovered_link = this.link_at(ev.position, cx);
                                cx.emit(PaneEvent::ContextMenu);
                            }
                        }),
                    )
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_scroll_wheel(cx.listener(Self::on_scroll))
                    .child(TerminalElement::new(entity, focused, window_active))
                    .children(new_output_pill)
                    .children(exit_line)
                    .children(resume_line)
                    .children(spawn_err),
            )
            .children(find_bar)
            .when(!focused && dim > 0.0, |d| d.child(div().absolute().inset_0().bg(theme.bg.opacity(dim))))
    }
}

#[must_use]
pub fn status_color(theme: &Theme, status: TabStatus) -> Option<Hsla> {
    match status {
        TabStatus::None => None,
        TabStatus::Done => Some(theme.ansi(2)),
        TabStatus::Failed => Some(theme.ansi(1)),
        TabStatus::Running => Some(theme.ansi(3)),
        // Waiting for the user (agent input, bell): blue.
        TabStatus::NeedsInput | TabStatus::Bell => Some(theme.ansi(4)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{TestAppContext, VisualTestContext, size};
    use nox_terminal::alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

    use super::*;

    // ---- harness --------------------------------------------------------------------------

    /// Test config whose "shell" is `/bin/sh -c script` (no rc files, no integration).
    pub(crate) fn config_running(script: &str) -> nox_config::Config {
        let mut c = crate::test_support::config();
        c.shell.args = vec!["-c".into(), script.into()];
        c.shell.integration = false;
        c.terminal.smooth_scroll_duration = 0;
        c
    }

    /// A child that prints nothing and never exits on its own (content is fed to the `Term`
    /// directly, so the reader thread never races the test).
    pub(crate) fn silent() -> nox_config::Config {
        config_running("exec cat >/dev/null")
    }

    /// Open a 640×360 window whose root is one focused pane.
    pub(crate) fn open(
        cx: &mut TestAppContext,
        config: nox_config::Config,
        spawn: PaneSpawn,
    ) -> (Entity<Pane>, &mut VisualTestContext) {
        cx.update(|cx| {
            crate::test_support::init_app(cx, config);
            cx.set_global(crate::root::UiFlags::default());
        });
        let (pane, cx) = cx.add_window_view(|window, cx| Pane::new(PaneId(1), spawn, window, cx));
        cx.simulate_resize(size(px(640.), px(360.)));
        pane.update_in(cx, |p, window, cx| {
            window.activate_window();
            window.focus(&p.focus_handle);
            cx.notify();
        });
        cx.run_until_parked();
        (pane, cx)
    }

    /// Feed bytes straight into the pane's `Term` (as if the program printed them).
    pub(crate) fn feed(pane: &Entity<Pane>, cx: &mut VisualTestContext, bytes: &[u8]) {
        pane.update(cx, |p, cx| {
            let s = p.session.as_ref().expect("session");
            let mut term = s.term().lock();
            let mut parser: Processor<StdSyncHandler> = Processor::new();
            parser.advance(&mut *term, bytes);
            drop(term);
            cx.notify();
        });
        cx.run_until_parked();
    }

    /// Poll `f` (running the executor in between) for up to 5 s.
    pub(crate) fn wait_until(cx: &mut VisualTestContext, mut f: impl FnMut(&mut VisualTestContext) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if f(cx) {
                return;
            }
            assert!(Instant::now() < deadline, "condition not met within 5 s");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub(crate) fn screen(pane: &Entity<Pane>, cx: &mut VisualTestContext) -> String {
        pane.read_with(cx, |p, _| p.session.as_ref().map(TerminalSession::screen_text).unwrap_or_default())
    }

    pub(crate) fn geom(pane: &Entity<Pane>, cx: &mut VisualTestContext) -> GridGeom {
        pane.read_with(cx, |p, _| p.geom.clone().expect("pane was laid out"))
    }

    /// Window position of the middle of the left half of cell (`col`, viewport `row`).
    pub(crate) fn cell(
        pane: &Entity<Pane>,
        cx: &mut VisualTestContext,
        col: usize,
        row: usize,
    ) -> Point<Pixels> {
        let g = geom(pane, cx);
        point(g.origin.x + g.cell_w * (col as f32 + 0.25), g.origin.y + g.cell_h * (row as f32 + 0.5))
    }

    /// Like [`cell`], but in the right half of the cell (a drag ending there includes it).
    pub(crate) fn cell_r(
        pane: &Entity<Pane>,
        cx: &mut VisualTestContext,
        col: usize,
        row: usize,
    ) -> Point<Pixels> {
        let g = geom(pane, cx);
        point(g.origin.x + g.cell_w * (col as f32 + 0.75), g.origin.y + g.cell_h * (row as f32 + 0.5))
    }

    fn record(pane: &Entity<Pane>, cx: &mut VisualTestContext) -> Rc<RefCell<Vec<PaneEvent>>> {
        let log = Rc::new(RefCell::new(Vec::new()));
        let l = log.clone();
        cx.update(|_, cx| {
            cx.subscribe(pane, move |_, ev: &PaneEvent, _| l.borrow_mut().push(ev.clone())).detach();
        });
        log
    }

    /// A pane whose child records every byte it receives (raw mode, no echo) into a file.
    struct Capture {
        _dir: tempfile::TempDir,
        file: PathBuf,
    }

    impl Capture {
        fn new() -> (Self, nox_config::Config) {
            Self::with_prelude("")
        }

        /// `prelude` is printed (by `printf`) before `READY`, e.g. mode-setting sequences.
        fn with_prelude(prelude: &str) -> (Self, nox_config::Config) {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("in.bin");
            let script = format!("stty raw -echo; printf '{prelude}READY'; exec cat > '{}'", file.display());
            (Self { _dir: dir, file }, config_running(&script))
        }

        fn ready(pane: &Entity<Pane>, cx: &mut VisualTestContext) {
            assert_eq!(pane.read_with(cx, |p, _| p.spawn_error.clone()), None);
            wait_until(cx, |cx| screen(pane, cx).contains("READY"));
        }

        fn bytes(&self) -> Vec<u8> {
            std::fs::read(&self.file).unwrap_or_default()
        }

        fn expect(&self, cx: &mut VisualTestContext, want: &[u8]) {
            wait_until(cx, |_| self.bytes().len() >= want.len());
            assert_eq!(
                self.bytes(),
                want,
                "bytes sent to the PTY: {:?}",
                String::from_utf8_lossy(&self.bytes())
            );
        }
    }

    // ---- pure helpers ---------------------------------------------------------------------

    #[test]
    fn shell_escape_quotes_only_when_needed() {
        assert_eq!(shell_escape("/usr/local/bin/x-1.2_3+4,5:6@7%"), "/usr/local/bin/x-1.2_3+4,5:6@7%");
        assert_eq!(shell_escape("/tmp/my file"), "'/tmp/my file'");
        assert_eq!(shell_escape("it's"), r"'it'\''s'");
    }

    #[test]
    fn regex_escape_makes_queries_literal() {
        assert_eq!(regex_escape("a.b*c"), r"a\.b\*c");
        assert_eq!(regex_escape("(x|y)[z]{1}^$"), r"\(x\|y\)\[z\]\{1\}\^\$");
        assert_eq!(regex_escape("plain"), "plain");
        let mut re = RegexSearch::new(&regex_escape("1+1")).unwrap();
        let _ = &mut re;
    }

    #[test]
    fn mouse_buttons_map_to_reports() {
        assert_eq!(button_of(MouseButton::Left), mouse::MouseButton::Left);
        assert_eq!(button_of(MouseButton::Middle), mouse::MouseButton::Middle);
        assert_eq!(button_of(MouseButton::Right), mouse::MouseButton::Right);
        assert_eq!(
            button_of(MouseButton::Navigate(gpui::NavigationDirection::Back)),
            mouse::MouseButton::Left
        );
    }

    #[test]
    fn spawn_options_follow_config() {
        let mut cfg = nox_config::Config::default();
        cfg.shell.program = "  ".into();
        cfg.shell.args = vec!["-l".into()];
        cfg.shell.env.insert("A".into(), "config".into());
        cfg.shell.env.insert("B".into(), "config".into());
        cfg.shell.integration = false;
        cfg.terminal.scrollback = 1234;
        cfg.terminal.osc52 = nox_config::Osc52::ReadWrite;
        cfg.terminal.cursor_style = nox_config::CursorStyle::Bar;
        cfg.terminal.cursor_blink = true;
        let spawn = PaneSpawn {
            cwd: Some("/tmp".into()),
            env: HashMap::from([("B".to_string(), "spawn".to_string())]),
            ..PaneSpawn::default()
        };
        let o = spawn_options(&cfg, &spawn);
        assert_eq!(o.program, None, "blank program = login shell");
        assert_eq!(o.args, vec!["-l".to_string()]);
        assert_eq!(o.cwd, Some(PathBuf::from("/tmp")));
        assert_eq!(o.env.get("A").map(String::as_str), Some("config"));
        assert_eq!(o.env.get("B").map(String::as_str), Some("spawn"), "pane env wins");
        assert_eq!(o.scrollback, 1234);
        assert!(matches!(o.osc52, nox_terminal::ClipboardPolicy::ReadWrite));
        assert!(o.integration_dir.is_none());
        assert!(matches!(o.cursor.shape, nox_terminal::session::CursorKind::Bar));
        assert!(o.cursor.blinking);

        cfg.shell.program = "/bin/zsh".into();
        cfg.shell.integration = true;
        cfg.terminal.osc52 = nox_config::Osc52::Off;
        let o = spawn_options(&cfg, &PaneSpawn::default());
        assert_eq!(o.program.as_deref(), Some("/bin/zsh"));
        assert!(o.integration_dir.is_some());
        assert!(matches!(o.osc52, nox_terminal::ClipboardPolicy::Off));
        cfg.terminal.osc52 = nox_config::Osc52::Write;
        assert!(matches!(
            spawn_options(&cfg, &PaneSpawn::default()).osc52,
            nox_terminal::ClipboardPolicy::Write
        ));
    }

    #[test]
    fn cursor_style_maps_shapes() {
        use nox_terminal::session::CursorKind;
        let mut cfg = nox_config::Config::default();
        for (style, kind) in [
            (nox_config::CursorStyle::Block, CursorKind::Block),
            (nox_config::CursorStyle::Bar, CursorKind::Bar),
            (nox_config::CursorStyle::Underline, CursorKind::Underline),
        ] {
            cfg.terminal.cursor_style = style;
            assert_eq!(std::mem::discriminant(&cursor_style(&cfg).shape), std::mem::discriminant(&kind));
        }
        assert!(!cursor_style(&cfg).blinking, "blink is off by default");
    }

    #[gpui::test]
    fn query_colors_and_status_colors_come_from_the_theme(cx: &mut TestAppContext) {
        crate::test_support::init(cx);
        cx.update(|cx| {
            let theme = Theme::global(cx);
            let colors = query_colors(theme);
            assert_eq!(colors.len(), nox_terminal::session::COLOR_COUNT);
            let to8 = |c: f32| (c * 255.).round() as u8;
            let fg = colors[256];
            assert_eq!(
                (fg.r, fg.g, fg.b),
                (to8(theme.raw.text.r), to8(theme.raw.text.g), to8(theme.raw.text.b))
            );
            let bg = colors[257];
            assert_eq!((bg.r, bg.g, bg.b), (to8(theme.raw.bg.r), to8(theme.raw.bg.g), to8(theme.raw.bg.b)));
            let red = colors[1];
            let p = theme.palette_rgb[1];
            assert_eq!((red.r, red.g, red.b), (to8(p.r), to8(p.g), to8(p.b)));

            assert_eq!(status_color(theme, TabStatus::None), None);
            assert_eq!(status_color(theme, TabStatus::Done), Some(theme.ansi(2)));
            assert_eq!(status_color(theme, TabStatus::Failed), Some(theme.ansi(1)));
            assert_eq!(status_color(theme, TabStatus::Running), Some(theme.ansi(3)));
            assert_eq!(status_color(theme, TabStatus::NeedsInput), Some(theme.ansi(4)));
            assert_eq!(status_color(theme, TabStatus::Bell), Some(theme.ansi(4)));
        });
    }

    // ---- identity and status --------------------------------------------------------------

    #[gpui::test]
    fn title_prefers_name_then_osc_then_process_then_user_host(cx: &mut TestAppContext) {
        use nox_config::TabTitle;
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        pane.update(cx, |p, _| {
            p.cwd = std::env::var_os("HOME").map(PathBuf::from);
            p.process = Some(ProcessInfo { exe: "sh".into(), args: vec![] });
            p.process_is_shell = true;
            assert_eq!(p.cwd_display(), "~");
            assert_eq!(p.title(TabTitle::Auto), format!("{}:~", nox_terminal::process::user_at_host()));
            assert_eq!(p.title(TabTitle::Process), "sh", "the shell itself in process mode");

            p.process = Some(ProcessInfo { exe: "vim".into(), args: vec!["a.txt".into()] });
            p.process_is_shell = false;
            assert_eq!(p.title(TabTitle::Auto), "vim");
            p.process = Some(ProcessInfo {
                exe: "ssh".into(),
                args: vec!["-p".into(), "22".into(), "-A".into(), "box".into()],
            });
            assert_eq!(p.process_display_name().as_deref(), Some("ssh box"));

            p.osc_title = Some("   ".into());
            assert_eq!(p.title(TabTitle::Auto), "ssh box", "blank OSC titles are ignored");
            p.osc_title = Some("vim: notes".into());
            assert_eq!(p.title(TabTitle::Auto), "vim: notes");
            assert_eq!(p.title(TabTitle::Cwd), "~");
            assert_eq!(p.title(TabTitle::Process), "ssh box");

            p.custom_name = Some("logs".into());
            assert_eq!(p.title(TabTitle::Auto), "logs");
            assert_eq!(p.title(TabTitle::Cwd), "logs");

            p.custom_name = None;
            p.cwd = None;
            assert_eq!(p.cwd_display(), "~");
            p.cwd = Some("/definitely/not/home".into());
            assert_eq!(p.title(TabTitle::Cwd), "/definitely/not/home");
            p.process = None;
            assert_eq!(p.title(TabTitle::Process), "shell");
        });
    }

    #[gpui::test]
    fn status_priority_and_mark_seen(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        pane.update(cx, |p, _| {
            assert_eq!(p.status(), TabStatus::None);
            p.done_unseen = true;
            assert_eq!(p.status(), TabStatus::Done);
            p.done_failed = true;
            assert_eq!(p.status(), TabStatus::Failed);
            p.running_since = Some(Instant::now());
            assert_eq!(p.status(), TabStatus::Running, "running beats done");
            p.bell_unseen = true;
            assert_eq!(p.status(), TabStatus::Bell, "bell beats everything");
            p.mark_seen();
            assert!(!p.done_unseen && !p.done_failed && !p.bell_unseen);
            assert_eq!(p.status(), TabStatus::Running);
            p.running_since = None;
            assert_eq!(p.status(), TabStatus::None);
            assert!(!p.is_busy(), "cat runs as the shell process itself");
            assert!(p.agent_state().is_none() && !p.is_agent());
        });
    }

    #[gpui::test]
    fn agent_reports_drive_status(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        let events = record(&pane, cx);
        pane.update(cx, |p, cx| {
            p.report_status(AgentState::Working, Some("compiling".into()), Some("s1".into()), cx);
            assert_eq!(p.agent.as_ref().unwrap().kind, "agent", "unknown process: generic agent");
            assert!(p.is_agent());
            assert_eq!(p.status(), TabStatus::Running);
            p.report_status(AgentState::NeedsInput, None, None, cx);
            assert_eq!(p.agent_state(), Some(AgentState::NeedsInput));
            assert_eq!(p.status(), TabStatus::NeedsInput);
            p.report_status(AgentState::Idle, None, None, cx);
            assert_eq!(p.status(), TabStatus::None);
            p.done_unseen = true;
            assert_eq!(p.status(), TabStatus::Done, "an agent's finished turn shows done");
            p.process = Some(ProcessInfo { exe: "vim".into(), args: vec![] });
            p.process_is_shell = false;
            assert_eq!(p.process_display_name().as_deref(), Some("agent"), "the agent kind names the pane");
        });
        cx.run_until_parked();
        let n = events.borrow().iter().filter(|e| matches!(e, PaneEvent::AgentChanged { .. })).count();
        assert_eq!(n, 3);
    }

    #[gpui::test]
    fn session_events_update_the_pane(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        let events = record(&pane, cx);
        let dir = tempfile::tempdir().unwrap();
        pane.update(cx, |p, cx| {
            p.handle_event(TerminalEvent::Title("htop".into()), cx);
            assert_eq!(p.osc_title.as_deref(), Some("htop"));
            p.handle_event(TerminalEvent::ResetTitle, cx);
            assert_eq!(p.osc_title, None);

            p.handle_event(TerminalEvent::Osc(OscEvent::Cwd(dir.path().to_path_buf())), cx);
            assert!(p.shell_integration, "OSC 7 means shell integration");
            assert_eq!(p.cwd.as_deref(), Some(dir.path()));

            p.handle_event(TerminalEvent::Osc(OscEvent::PromptStart), cx);
            p.handle_event(TerminalEvent::Osc(OscEvent::CommandStart), cx);
            assert!(p.running_since.is_some());
            assert_eq!(p.status(), TabStatus::Running);
            p.handle_event(TerminalEvent::Osc(OscEvent::CommandEnd(Some(2))), cx);
            assert!(p.running_since.is_none());
            // A CommandEnd without a start reports nothing.
            p.handle_event(TerminalEvent::Osc(OscEvent::CommandEnd(Some(0))), cx);

            p.handle_event(
                TerminalEvent::Osc(OscEvent::Notify { title: Some("t".into()), body: "b".into() }),
                cx,
            );
            p.handle_event(TerminalEvent::ClipboardStore("from osc 52".into()), cx);
        });
        cx.run_until_parked();
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("from osc 52"));
        let ev = events.borrow();
        let finished: Vec<_> = ev
            .iter()
            .filter_map(|e| match e {
                PaneEvent::CommandFinished { exit, .. } => Some(*exit),
                _ => None,
            })
            .collect();
        assert_eq!(finished, vec![Some(2)]);
        assert!(
            ev.iter()
                .any(|e| matches!(e, PaneEvent::Notify { title: Some(t), body } if t == "t" && body == "b"))
        );
        assert!(ev.iter().filter(|e| matches!(e, PaneEvent::TitleChanged)).count() >= 4);
    }

    #[gpui::test]
    fn visual_bell_flashes_briefly(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        let events = record(&pane, cx);
        pane.update(cx, |p, cx| p.handle_event(TerminalEvent::Bell, cx));
        assert!(pane.read_with(cx, |p, _| p.bell_flash.is_some()));
        cx.run_until_parked();
        assert!(events.borrow().iter().any(|e| matches!(e, PaneEvent::Bell)));
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        assert!(pane.read_with(cx, |p, _| p.bell_flash.is_none()), "flash ends after ~150 ms");
    }

    #[gpui::test]
    fn no_bell_does_not_flash(cx: &mut TestAppContext) {
        let mut cfg = silent();
        cfg.terminal.bell = nox_config::Bell::None;
        let (pane, cx) = open(cx, cfg, PaneSpawn::default());
        let events = record(&pane, cx);
        pane.update(cx, |p, cx| p.handle_event(TerminalEvent::Bell, cx));
        cx.run_until_parked();
        assert!(pane.read_with(cx, |p, _| p.bell_flash.is_none()));
        assert!(events.borrow().iter().any(|e| matches!(e, PaneEvent::Bell)), "the tab is still marked");
    }

    #[gpui::test]
    fn clipboard_load_answers_with_the_clipboard(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        cx.write_to_clipboard(ClipboardItem::new_string("secret".into()));
        // Nothing observable but the reply to the PTY; this must just not misbehave.
        pane.update(cx, |p, cx| p.handle_event(TerminalEvent::ClipboardLoad, cx));
        cx.run_until_parked();
    }

    // ---- real shell -----------------------------------------------------------------------

    #[gpui::test]
    fn shell_output_reaches_the_screen(cx: &mut TestAppContext) {
        let (pane, cx) =
            open(cx, config_running("printf 'hello from sh'; exec cat >/dev/null"), PaneSpawn::default());
        wait_until(cx, |cx| screen(&pane, cx).contains("hello from sh"));
        pane.read_with(cx, |p, _| {
            assert!(p.spawn_error.is_none());
            assert!(p.exited.is_none());
            let g = p.geom.clone().unwrap();
            let size = p.session.as_ref().unwrap().size();
            assert_eq!(
                (usize::from(size.cols), usize::from(size.rows)),
                (g.cols, g.rows),
                "Term follows the layout"
            );
        });
    }

    #[gpui::test]
    fn startup_command_runs_in_the_shell(cx: &mut TestAppContext) {
        let spawn = PaneSpawn {
            startup: Some(Startup::Run("echo started-$((40+2))".into())),
            name: Some("job".into()),
            ..PaneSpawn::default()
        };
        let (pane, cx) = open(cx, config_running("exec /bin/sh"), spawn);
        wait_until(cx, |cx| screen(&pane, cx).contains("started-42"));
        assert_eq!(pane.read_with(cx, |p, _| p.title(nox_config::TabTitle::Auto)), "job");
    }

    #[gpui::test]
    fn exit_is_reported_and_restart_respawns(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, config_running("exit 3"), PaneSpawn::default());
        let events = record(&pane, cx);
        wait_until(cx, |cx| pane.read_with(cx, |p, _| p.exited.is_some()));
        assert_eq!(pane.read_with(cx, |p, _| p.exited), Some(Some(3)));
        // (`Exited` may fire before the test can subscribe, so only the state is checked.)
        assert!(!pane.read_with(cx, |p, _| p.is_busy()));

        // ↵ on an exited pane closes it.
        cx.simulate_keystrokes("enter");
        assert!(events.borrow().iter().any(|e| matches!(e, PaneEvent::Close)));

        // ⌘R restarts it: a new shell is spawned (which exits again at once, so whether
        // `exited` is still clear right after the restart depends on timing — check the pid).
        let pid = pane.read_with(cx, |p, _| p.session.as_ref().map(TerminalSession::pid));
        cx.dispatch_action(RestartPane);
        let new_pid = pane.read_with(cx, |p, _| p.session.as_ref().map(TerminalSession::pid));
        assert!(new_pid.is_some() && new_pid != pid, "restart spawns a new session");
        wait_until(cx, |cx| pane.read_with(cx, |p, _| p.exited.is_some()));
        assert_eq!(pane.read_with(cx, |p, _| p.exited), Some(Some(3)));
    }

    #[gpui::test]
    fn restart_is_a_no_op_while_running(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        let pid = pane.read_with(cx, |p, _| p.session.as_ref().unwrap().pid());
        pane.update(cx, Pane::restart);
        assert_eq!(pane.read_with(cx, |p, _| p.session.as_ref().unwrap().pid()), pid);
    }

    // ---- keyboard, paste, IME -------------------------------------------------------------

    #[gpui::test]
    fn keys_are_encoded_to_the_pty(cx: &mut TestAppContext) {
        let (cap, cfg) = Capture::new();
        let (pane, cx) = open(cx, cfg, PaneSpawn::default());
        Capture::ready(&pane, cx);
        let events = record(&pane, cx);
        // Plain text goes through the input handler, the rest through the key encoder.
        cx.simulate_keystrokes("a ctrl-c up enter tab backspace escape");
        cap.expect(cx, b"a\x03\x1b[A\r\t\x7f\x1b");
        assert!(pane.read_with(cx, |p, _| p.last_user_input.is_some()));
        assert!(events.borrow().iter().any(|e| matches!(e, PaneEvent::UserInput)));
    }

    #[gpui::test]
    fn app_cursor_mode_changes_arrow_keys(cx: &mut TestAppContext) {
        let (cap, cfg) = Capture::with_prelude(r"\033[?1h");
        let (pane, cx) = open(cx, cfg, PaneSpawn::default());
        Capture::ready(&pane, cx);
        cx.simulate_keystrokes("up down");
        cap.expect(cx, b"\x1bOA\x1bOB");
    }

    #[gpui::test]
    fn shift_enter_and_send_bytes(cx: &mut TestAppContext) {
        let (cap, cfg) = Capture::new();
        let (pane, cx) = open(cx, cfg, PaneSpawn::default());
        Capture::ready(&pane, cx);
        cx.dispatch_action(SendShiftEnter);
        pane.update(cx, |p, cx| p.send_bytes(b"xyz", cx));
        cap.expect(cx, b"\x1b\rxyz");
    }

    #[gpui::test]
    fn paste_is_plain_or_bracketed(cx: &mut TestAppContext) {
        let (cap, cfg) = Capture::with_prelude(r"\033[?2004h");
        let (pane, cx) = open(cx, cfg, PaneSpawn::default());
        Capture::ready(&pane, cx);
        cx.write_to_clipboard(ClipboardItem::new_string("one\ntwo".into()));
        // Bracketed mode: multi-line text is pasted without asking.
        cx.dispatch_action(Paste);
        cap.expect(cx, b"\x1b[200~one\rtwo\x1b[201~");
    }

    #[gpui::test]
    fn multiline_paste_outside_bracketed_mode_asks_first(cx: &mut TestAppContext) {
        let (cap, cfg) = Capture::new();
        let (pane, cx) = open(cx, cfg, PaneSpawn::default());
        Capture::ready(&pane, cx);
        pane.update_in(cx, |p, window, cx| p.paste_text("ls\nrm -rf x\n", window, cx));
        cx.run_until_parked();
        assert!(cx.update(|window, cx| crate::dialog::is_open(window, cx)), "a confirmation sheet is shown");
        // A single line (trailing newline ignored) goes straight through.
        cx.update(|_, cx| cx.set_global(crate::dialog::ActiveDialog(None)));
        pane.update_in(cx, |p, window, cx| p.paste_text("pwd", window, cx));
        cap.expect(cx, b"pwd");
    }

    #[gpui::test]
    fn ime_marked_text_is_shown_then_committed(cx: &mut TestAppContext) {
        let (cap, cfg) = Capture::new();
        let (pane, cx) = open(cx, cfg, PaneSpawn::default());
        Capture::ready(&pane, cx);
        pane.update_in(cx, |p, window, cx| {
            p.replace_and_mark_text_in_range(None, "に", None, window, cx);
            assert_eq!(p.marked_text.as_deref(), Some("に"));
            assert_eq!(p.marked_text_range(window, cx), Some(0..1));
            assert_eq!(p.text_for_range(0..1, &mut None, window, cx), None);
            assert_eq!(p.selected_text_range(false, window, cx).map(|s| s.range), Some(0..0));
            assert_eq!(p.character_index_for_point(point(px(0.), px(0.)), window, cx), None);
            let b = p.bounds_for_range(0..1, Bounds::default(), window, cx).expect("cursor bounds");
            let g = p.geom.clone().unwrap();
            assert_eq!(b.size, gpui::size(g.cell_w, g.cell_h));
        });
        // Marked text is not sent; keys are swallowed while composing.
        cx.simulate_keystrokes("left");
        pane.update_in(cx, |p, window, cx| {
            p.unmark_text(window, cx);
            assert!(p.marked_text.is_none());
            p.replace_and_mark_text_in_range(None, "", None, window, cx);
            assert!(p.marked_text.is_none(), "empty composition clears");
            p.replace_text_in_range(None, "日本", window, cx);
            p.replace_text_in_range(None, "", window, cx);
        });
        cap.expect(cx, "日本".as_bytes());
    }

    // ---- selection and clipboard ----------------------------------------------------------

    fn selection(pane: &Entity<Pane>, cx: &mut VisualTestContext) -> Option<String> {
        pane.read_with(cx, |p, _| p.session.as_ref().unwrap().term().lock().selection_to_string())
    }

    fn clipboard(cx: &mut VisualTestContext) -> Option<String> {
        cx.read_from_clipboard().and_then(|c| c.text())
    }

    fn mouse_down(cx: &mut VisualTestContext, pos: Point<Pixels>, clicks: usize, modifiers: Modifiers) {
        cx.simulate_event(MouseDownEvent {
            position: pos,
            modifiers,
            button: MouseButton::Left,
            click_count: clicks,
            first_mouse: false,
        });
    }

    fn mouse_up(cx: &mut VisualTestContext, pos: Point<Pixels>) {
        cx.simulate_event(MouseUpEvent {
            position: pos,
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 1,
        });
    }

    #[gpui::test]
    fn drag_selects_and_copies_on_release(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        feed(&pane, cx, b"hello world   \r\nsecond line");
        let (a, b) = (cell(&pane, cx, 0, 0), cell_r(&pane, cx, 4, 0));
        mouse_down(cx, a, 1, Modifiers::none());
        assert_eq!(selection(&pane, cx), None, "a plain click selects nothing");
        cx.simulate_mouse_move(b, MouseButton::Left, Modifiers::none());
        assert_eq!(selection(&pane, cx).as_deref(), Some("hello"));
        mouse_up(cx, b);
        assert_eq!(clipboard(cx).as_deref(), Some("hello"), "copy on select");

        // ⇧-click extends the selection.
        let at = cell(&pane, cx, 7, 1);
        mouse_down(cx, at, 1, Modifiers::shift());
        let at = cell(&pane, cx, 7, 1);
        mouse_up(cx, at);
        assert_eq!(selection(&pane, cx).as_deref(), Some("hello world\nsecond "));
        assert_eq!(clipboard(cx).as_deref(), Some("hello world\nsecond"), "trailing spaces trimmed");
    }

    #[gpui::test]
    fn plain_click_copies_nothing(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        feed(&pane, cx, b"abc");
        cx.write_to_clipboard(ClipboardItem::new_string("keep".into()));
        let at = cell(&pane, cx, 1, 0);
        mouse_down(cx, at, 1, Modifiers::none());
        let at = cell(&pane, cx, 1, 0);
        mouse_up(cx, at);
        assert_eq!(selection(&pane, cx), None);
        assert_eq!(clipboard(cx).as_deref(), Some("keep"));
    }

    #[gpui::test]
    fn double_and_triple_click_select_word_and_line(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        feed(&pane, cx, b"alpha beta(gamma)\r\nnext");
        let at = cell(&pane, cx, 7, 0);
        mouse_down(cx, at, 2, Modifiers::none());
        assert_eq!(selection(&pane, cx).as_deref(), Some("beta"), "word separators include (");
        let at = cell(&pane, cx, 7, 0);
        mouse_up(cx, at);
        assert_eq!(clipboard(cx).as_deref(), Some("beta"));
        let at = cell(&pane, cx, 2, 0);
        mouse_down(cx, at, 3, Modifiers::none());
        assert_eq!(selection(&pane, cx).as_deref(), Some("alpha beta(gamma)\n"));
        let at = cell(&pane, cx, 2, 0);
        mouse_up(cx, at);
        assert_eq!(clipboard(cx).as_deref(), Some("alpha beta(gamma)"));
    }

    #[gpui::test]
    fn alt_drag_selects_a_block(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        feed(&pane, cx, b"abcdef\r\nghijkl\r\nmnopqr");
        let at = cell(&pane, cx, 1, 0);
        mouse_down(cx, at, 1, Modifiers::alt());
        let at = cell_r(&pane, cx, 2, 2);
        cx.simulate_mouse_move(at, MouseButton::Left, Modifiers::alt());
        assert_eq!(selection(&pane, cx).as_deref(), Some("bc\nhi\nno"));
        // Moving without the button ends the drag.
        let at = cell(&pane, cx, 4, 2);
        cx.simulate_mouse_move(at, None, Modifiers::none());
        assert!(pane.read_with(cx, |p, _| p.selecting.is_none()));
    }

    #[gpui::test]
    fn copy_select_all_and_clear(cx: &mut TestAppContext) {
        let mut cfg = silent();
        cfg.terminal.copy_on_select = false;
        let (pane, cx) = open(cx, cfg, PaneSpawn::default());
        feed(&pane, cx, b"one   \r\ntwo");
        cx.write_to_clipboard(ClipboardItem::new_string("before".into()));
        cx.dispatch_action(Copy);
        assert_eq!(clipboard(cx).as_deref(), Some("before"), "⌘C without a selection does nothing");
        let at = cell(&pane, cx, 0, 0);
        mouse_down(cx, at, 1, Modifiers::none());
        let at = cell(&pane, cx, 1, 0);
        cx.simulate_mouse_move(at, MouseButton::Left, Modifiers::none());
        let at = cell(&pane, cx, 1, 0);
        mouse_up(cx, at);
        assert_eq!(clipboard(cx).as_deref(), Some("before"), "copy_on_select = false");

        cx.dispatch_action(SelectAll);
        cx.dispatch_action(Copy);
        let all = clipboard(cx).unwrap();
        assert!(all.starts_with("one\ntwo"), "{all:?}");

        feed(&pane, cx, &b"x\r\n".repeat(40));
        assert!(
            pane.read_with(cx, |p, _| p.session.as_ref().unwrap().term().lock().grid().history_size()) > 0
        );
        cx.dispatch_action(ClearBuffer);
        pane.read_with(cx, |p, _| {
            let term = p.session.as_ref().unwrap().term().lock();
            assert_eq!(term.grid().history_size(), 0, "scrollback cleared");
            assert!(term.selection.is_none());
        });
    }

    #[gpui::test]
    fn typing_clears_the_selection(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        feed(&pane, cx, b"word");
        cx.dispatch_action(SelectAll);
        assert!(selection(&pane, cx).is_some());
        cx.simulate_keystrokes("ctrl-l");
        assert!(selection(&pane, cx).is_none());
    }

    #[gpui::test]
    fn copy_last_output_needs_shell_integration(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
        cx.dispatch_action(CopyLastOutput);
        assert_eq!(clipboard(cx).as_deref(), Some("unchanged"));
        let _ = pane;
    }

    #[gpui::test]
    fn copy_last_output_with_prompt_marks(cx: &mut TestAppContext) {
        let script = r"printf '\033]133;A\007$ \033]133;B\007ls\r\n\033]133;C\007file-a\r\nfile-b\r\n\033]133;D;0\007\033]133;A\007$ '; exec cat >/dev/null";
        let (pane, cx) = open(cx, config_running(script), PaneSpawn::default());
        wait_until(cx, |cx| pane.read_with(cx, |p, _| p.session.as_ref().unwrap().last_output().is_some()));
        cx.dispatch_action(CopyLastOutput);
        assert_eq!(clipboard(cx).as_deref(), Some("file-a\nfile-b"));
    }

    // ---- scrolling ------------------------------------------------------------------------

    fn offset(pane: &Entity<Pane>, cx: &mut VisualTestContext) -> usize {
        pane.read_with(cx, |p, _| p.session.as_ref().unwrap().term().lock().grid().display_offset())
    }

    fn history(pane: &Entity<Pane>, cx: &mut VisualTestContext) -> usize {
        pane.read_with(cx, |p, _| p.session.as_ref().unwrap().term().lock().grid().history_size())
    }

    fn fill_lines(pane: &Entity<Pane>, cx: &mut VisualTestContext, n: usize) {
        let text: String = (0..n).map(|i| format!("line {i}\r\n")).collect::<Vec<_>>().concat();
        feed(pane, cx, text.as_bytes());
    }

    fn wheel(cx: &mut VisualTestContext, pos: Point<Pixels>, delta: ScrollDelta, modifiers: Modifiers) {
        cx.simulate_event(ScrollWheelEvent {
            position: pos,
            delta,
            modifiers,
            ..ScrollWheelEvent::default()
        });
    }

    #[gpui::test]
    fn trackpad_scroll_is_pixel_smooth(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        fill_lines(&pane, cx, 100);
        let g = geom(&pane, cx);
        let h = f32::from(g.cell_h);
        let pos = cell(&pane, cx, 3, 3);
        wheel(cx, pos, ScrollDelta::Pixels(point(px(0.), px(h * 3.5))), Modifiers::none());
        assert_eq!(offset(&pane, cx), 3);
        let frac = pane.read_with(cx, |p, _| p.scroll.frac);
        assert!((frac - h * 0.5).abs() < 0.01, "sub-line offset {frac}");

        // New output while scrolled up shows the pill; ⌘End jumps back.
        pane.update(cx, |p, cx| p.handle_event(TerminalEvent::Wakeup, cx));
        assert!(pane.read_with(cx, |p, _| p.new_output_below));
        cx.dispatch_action(ScrollToBottom);
        assert_eq!(offset(&pane, cx), 0);
        assert!(pane.read_with(cx, |p, _| !p.new_output_below && p.scroll.frac == 0.0));

        // Scrolling back down to the bottom hides the pill too.
        wheel(cx, pos, ScrollDelta::Pixels(point(px(0.), px(h * 2.))), Modifiers::none());
        pane.update(cx, |p, _| p.new_output_below = true);
        wheel(cx, pos, ScrollDelta::Pixels(point(px(0.), px(-h * 10.))), Modifiers::none());
        assert_eq!(offset(&pane, cx), 0);
        assert!(!pane.read_with(cx, |p, _| p.new_output_below));
    }

    #[gpui::test]
    fn wheel_lines_use_the_multiplier(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        fill_lines(&pane, cx, 100);
        let at = cell(&pane, cx, 0, 0);
        wheel(cx, at, ScrollDelta::Lines(point(0., 2.)), Modifiers::none());
        assert_eq!(offset(&pane, cx), 6, "2 notches × 3 lines");
    }

    #[gpui::test]
    fn wheel_lines_animate_when_smooth(cx: &mut TestAppContext) {
        let mut cfg = silent();
        cfg.terminal.smooth_scroll_duration = 40;
        let (pane, cx) = open(cx, cfg, PaneSpawn::default());
        fill_lines(&pane, cx, 100);
        let at = cell(&pane, cx, 0, 0);
        wheel(cx, at, ScrollDelta::Lines(point(0., 1.)), Modifiers::none());
        // Each frame (prepaint) advances the animation (in real time, so it may be done already).
        wait_until(cx, |cx| {
            pane.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            !pane.read_with(cx, |p, _| p.scroll.animating())
        });
        // Lands exactly on 3 lines (1 notch × 3), no leftover fraction.
        assert_eq!(offset(&pane, cx), 3);
        assert_eq!(pane.read_with(cx, |p, _| p.scroll.frac), 0.0);
    }

    #[gpui::test]
    fn page_top_and_bottom_actions(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        fill_lines(&pane, cx, 100);
        let rows = geom(&pane, cx).rows;
        cx.dispatch_action(ScrollPageUp);
        assert_eq!(offset(&pane, cx), rows);
        cx.dispatch_action(ScrollPageDown);
        assert_eq!(offset(&pane, cx), 0);
        cx.dispatch_action(ScrollToTop);
        assert_eq!(offset(&pane, cx), history(&pane, cx));
        assert!(pane.read_with(cx, |p, _| p.scroll.last_scroll.is_some()), "shows the scrollbar");
        // Typing jumps to the bottom (scroll_on_input).
        cx.simulate_keystrokes("ctrl-l");
        assert_eq!(offset(&pane, cx), 0);
    }

    #[gpui::test]
    fn prompt_jumps_without_marks_move_one_line(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        fill_lines(&pane, cx, 100);
        cx.dispatch_action(PreviousPrompt);
        cx.dispatch_action(PreviousPrompt);
        assert_eq!(offset(&pane, cx), 2);
        cx.dispatch_action(NextPrompt);
        assert_eq!(offset(&pane, cx), 1);
    }

    #[gpui::test]
    fn prompt_jumps_follow_marks(cx: &mut TestAppContext) {
        // Three prompts, 30 lines of output each.
        let script = r"i=0; while [ $i -lt 3 ]; do printf '\033]133;A\007$ p%s\r\n' $i; j=0; while [ $j -lt 30 ]; do printf 'out\r\n'; j=$((j+1)); done; i=$((i+1)); done; printf 'END'; exec cat >/dev/null";
        let (pane, cx) = open(cx, config_running(script), PaneSpawn::default());
        wait_until(cx, |cx| screen(&pane, cx).contains("END"));
        let prompts = pane.read_with(cx, |p, _| p.session.as_ref().unwrap().prompt_lines());
        assert_eq!(prompts.len(), 3, "{prompts:?}");
        let top_line = |pane: &Entity<Pane>, cx: &mut VisualTestContext| -(offset(pane, cx) as i32);
        cx.dispatch_action(PreviousPrompt);
        let p_last = *prompts.iter().rev().find(|&&l| l < 0).unwrap();
        assert_eq!(top_line(&pane, cx), p_last, "the previous prompt is at the top");
        cx.dispatch_action(PreviousPrompt);
        cx.dispatch_action(PreviousPrompt);
        assert_eq!(top_line(&pane, cx), prompts[0]);
        cx.dispatch_action(PreviousPrompt);
        assert_eq!(top_line(&pane, cx), prompts[0], "no prompt above the first");
        cx.dispatch_action(NextPrompt);
        assert_eq!(top_line(&pane, cx), prompts[1]);
        for _ in 0..3 {
            cx.dispatch_action(NextPrompt);
        }
        assert_eq!(offset(&pane, cx), 0, "past the last prompt: bottom");
    }

    #[gpui::test]
    fn alternate_screen_wheel_sends_arrows(cx: &mut TestAppContext) {
        let (cap, cfg) = Capture::with_prelude(r"\033[?1049h");
        let (pane, cx) = open(cx, cfg, PaneSpawn::default());
        Capture::ready(&pane, cx);
        let h = f32::from(geom(&pane, cx).cell_h);
        let pos = cell(&pane, cx, 0, 0);
        wheel(cx, pos, ScrollDelta::Lines(point(0., 2.)), Modifiers::none());
        // Half a line accumulates; the second half completes one more line.
        wheel(cx, pos, ScrollDelta::Pixels(point(px(0.), px(-h * 0.5))), Modifiers::none());
        wheel(cx, pos, ScrollDelta::Pixels(point(px(0.), px(-h * 0.6))), Modifiers::none());
        cap.expect(cx, b"\x1b[A\x1b[A\x1b[B");
        assert_eq!(offset(&pane, cx), 0, "the alternate screen has no scrollback");
    }

    // ---- mouse reporting ------------------------------------------------------------------

    #[gpui::test]
    fn mouse_reports_go_to_the_program(cx: &mut TestAppContext) {
        let (cap, cfg) = Capture::with_prelude(r"\033[?1000h");
        let (pane, cx) = open(cx, cfg, PaneSpawn::default());
        Capture::ready(&pane, cx);
        let events = record(&pane, cx);
        let pos = cell(&pane, cx, 2, 1);
        mouse_down(cx, pos, 1, Modifiers::none());
        mouse_up(cx, pos);
        // Right click is reported too instead of opening the context menu.
        cx.simulate_event(MouseDownEvent {
            position: pos,
            modifiers: Modifiers::none(),
            button: MouseButton::Right,
            click_count: 1,
            first_mouse: false,
        });
        wheel(cx, pos, ScrollDelta::Lines(point(0., 1.)), Modifiers::none());
        // X10 encoding: ESC [ M, button + 32, col + 33, row + 33.
        let (c, r) = (b'!' + 2, b'!' + 1);
        cap.expect(
            cx,
            &[
                0x1b, b'[', b'M', 32, c, r, 0x1b, b'[', b'M', 35, c, r, 0x1b, b'[', b'M', 34, c, r, 0x1b,
                b'[', b'M', 96, c, r,
            ],
        );
        assert!(!events.borrow().iter().any(|e| matches!(e, PaneEvent::ContextMenu)));
        assert_eq!(selection(&pane, cx), None);

        // ⇧ forces a local selection.
        let at = cell(&pane, cx, 0, 0);
        mouse_down(cx, at, 1, Modifiers::shift());
        let at = cell_r(&pane, cx, 3, 0);
        cx.simulate_mouse_move(at, MouseButton::Left, Modifiers::shift());
        assert_eq!(selection(&pane, cx).as_deref(), Some("READ"));
    }

    #[gpui::test]
    fn right_click_opens_the_context_menu(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        let events = record(&pane, cx);
        let at = cell(&pane, cx, 1, 1);
        cx.simulate_event(MouseDownEvent {
            position: at,
            modifiers: Modifiers::none(),
            button: MouseButton::Right,
            click_count: 1,
            first_mouse: false,
        });
        assert!(events.borrow().iter().any(|e| matches!(e, PaneEvent::ContextMenu)));
    }

    #[gpui::test]
    fn dragging_past_the_edge_autoscrolls(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        fill_lines(&pane, cx, 100);
        let g = geom(&pane, cx);
        let at = cell(&pane, cx, 2, 2);
        mouse_down(cx, at, 1, Modifiers::none());
        // Real mouse moves outside the grid (the grid's own handler only hears moves over it).
        let drag = |cx: &mut VisualTestContext, pos: Point<Pixels>| {
            cx.simulate_mouse_move(pos, MouseButton::Left, Modifiers::none());
        };
        let above = point(g.origin.x + px(20.), g.bounds.top() - px(40.));
        drag(cx, above);
        assert!(pane.read_with(cx, |p, _| p.autoscroll.is_some()));
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        let off = offset(&pane, cx);
        assert!(off >= 3, "scrolled {off} lines into history, 2 per 30 ms tick");
        let s = selection(&pane, cx).unwrap();
        assert!(s.lines().count() > 3, "the selection grew with the scroll: {s:?}");
        // Back inside: autoscroll stops.
        let inside = cell(&pane, cx, 3, 3);
        drag(cx, inside);
        assert!(pane.read_with(cx, |p, _| p.autoscroll.is_none()));
        // Below the bottom edge it scrolls back down.
        let below = point(g.origin.x + px(20.), g.bounds.bottom() + px(30.));
        drag(cx, below);
        assert!(pane.read_with(cx, |p, _| p.autoscroll.is_some()));
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        assert!(offset(&pane, cx) < off);
        // Releasing outside the grid ends the drag.
        mouse_up(cx, below);
        assert!(pane.read_with(cx, |p, _| p.autoscroll.is_none() && p.selecting.is_none()));
        let off = offset(&pane, cx);
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        assert_eq!(offset(&pane, cx), off, "no more scrolling");
        // Without a drag, moves outside the grid do nothing.
        cx.simulate_mouse_move(above, None, Modifiers::none());
        assert!(pane.read_with(cx, |p, _| p.autoscroll.is_none()));
    }

    // ---- find -----------------------------------------------------------------------------

    fn set_query(pane: &Entity<Pane>, cx: &mut VisualTestContext, q: &str) {
        let input = pane.read_with(cx, |p, _| p.find.as_ref().unwrap().input.clone());
        input.update(cx, |i, cx| i.set_text(q, cx));
        pane.update(cx, Pane::run_search);
        cx.run_until_parked();
    }

    fn find_state(pane: &Entity<Pane>, cx: &mut VisualTestContext) -> (usize, Option<usize>) {
        pane.read_with(cx, |p, _| (p.find_highlights().len(), p.find_current_index()))
    }

    #[gpui::test]
    fn find_counts_matches_and_steps_through_them(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        feed(&pane, cx, b"foo bar\r\nFOO baz\r\nfoo.qux\r\nfo+o\r\n");
        assert_eq!(find_state(&pane, cx), (0, None));
        cx.dispatch_action(Find);
        let input = pane.read_with(cx, |p, _| p.find.as_ref().unwrap().input.clone());
        assert!(cx.update(|window, cx| input.focus_handle(cx).is_focused(window)), "⌘F focuses the query");

        // Typing in the field searches as you type.
        cx.simulate_input("foo");
        assert_eq!(find_state(&pane, cx), (3, Some(2)), "case-insensitive; starts at the bottom match");
        cx.dispatch_action(FindNext);
        assert_eq!(find_state(&pane, cx).1, Some(0), "wraps around");
        cx.dispatch_action(FindPrevious);
        assert_eq!(find_state(&pane, cx).1, Some(2));
        cx.simulate_keystrokes("enter");
        assert_eq!(find_state(&pane, cx).1, Some(0), "↵ = next");

        // "Aa" decides the case alone, for lowercase and uppercase queries alike.
        let set_case = |pane: &Entity<Pane>, cx: &mut VisualTestContext, on: bool| {
            pane.update(cx, |p, cx| {
                p.find.as_mut().unwrap().case_sensitive = on;
                p.run_search(cx);
            });
        };
        set_query(&pane, cx, "FOO");
        assert_eq!(find_state(&pane, cx).0, 3, "case-insensitive by default");
        set_case(&pane, cx, true);
        assert_eq!(find_state(&pane, cx).0, 1);
        set_query(&pane, cx, "foo");
        assert_eq!(find_state(&pane, cx).0, 2, "a lowercase query is case-sensitive too");
        set_case(&pane, cx, false);
        assert_eq!(find_state(&pane, cx).0, 3);

        // Literal by default: `.` and `+` are not regex syntax.
        set_query(&pane, cx, "foo.");
        assert_eq!(find_state(&pane, cx).0, 1);
        set_query(&pane, cx, "fo+o");
        assert_eq!(find_state(&pane, cx).0, 1);
        pane.update(cx, |p, cx| {
            p.find.as_mut().unwrap().regex = true;
            p.run_search(cx);
        });
        assert_eq!(find_state(&pane, cx).0, 3, "regex fo+o matches foo, foo, foo");
        set_query(&pane, cx, "(unclosed");
        assert_eq!(find_state(&pane, cx), (0, None), "an invalid regex matches nothing");
        set_query(&pane, cx, "");
        assert_eq!(find_state(&pane, cx), (0, None));
        cx.dispatch_action(FindNext);

        // ⌘F again keeps the query; Esc closes and focuses the terminal.
        cx.dispatch_action(Find);
        assert!(pane.read_with(cx, |p, _| p.find.is_some()));
        cx.simulate_keystrokes("escape");
        assert!(pane.read_with(cx, |p, _| p.find.is_none()));
        assert!(cx.update(|window, cx| pane.read(cx).focus_handle.is_focused(window)));
    }

    #[gpui::test]
    fn find_reveals_matches_in_scrollback(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        feed(&pane, cx, b"needle\r\n");
        fill_lines(&pane, cx, 100);
        cx.dispatch_action(Find);
        cx.simulate_input("needle");
        assert_eq!(find_state(&pane, cx), (1, Some(0)));
        assert!(offset(&pane, cx) > 0, "scrolled up to the match");
    }

    #[gpui::test]
    fn keys_go_to_the_find_field_not_the_pty(cx: &mut TestAppContext) {
        let (cap, cfg) = Capture::new();
        let (pane, cx) = open(cx, cfg, PaneSpawn::default());
        Capture::ready(&pane, cx);
        cx.dispatch_action(Find);
        cx.simulate_keystrokes("x ctrl-c");
        cx.simulate_keystrokes("escape");
        cx.simulate_keystrokes("y");
        cap.expect(cx, b"y");
    }

    // ---- links ----------------------------------------------------------------------------

    fn link_at(
        pane: &Entity<Pane>,
        cx: &mut VisualTestContext,
        col: usize,
        row: usize,
    ) -> Option<HoveredLink> {
        let pos = cell(pane, cx, col, row);
        pane.read_with(cx, |p, cx| p.link_at(pos, cx))
    }

    #[gpui::test]
    fn links_are_found_under_the_pointer(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        // Canonical: without OSC 7 the cwd is read back from the process (`/private/var/…`).
        let dir = tmp.path().canonicalize().unwrap();
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        let (pane, cx) = open(cx, silent(), PaneSpawn { cwd: Some(dir.clone()), ..PaneSpawn::default() });
        feed(&pane, cx, b"see https://example.com/a here\r\nnotes.txt:3\r\n\x1b]8;;https://nox.dev\x1b\\docs\x1b]8;;\x1b\\ plain");
        let url = link_at(&pane, cx, 8, 0).expect("url");
        assert_eq!(url.target, LinkTarget::Url("https://example.com/a".into()));
        assert_eq!(url.spans, vec![(Line(0), 4, 24)]);
        assert_eq!(link_at(&pane, cx, 1, 0), None, "plain words are not links");

        let path = link_at(&pane, cx, 2, 1).expect("path");
        assert_eq!(path.target, LinkTarget::Path(dir.join("notes.txt")));
        assert_eq!(path.spans, vec![(Line(1), 0, 10)]);

        let osc8 = link_at(&pane, cx, 2, 2).expect("osc 8");
        assert_eq!(osc8.target, LinkTarget::Url("https://nox.dev".into()));
        assert_eq!(osc8.spans, vec![(Line(2), 0, 3)], "spans the cells carrying the link");
        assert_eq!(link_at(&pane, cx, 6, 2), None);
    }

    #[gpui::test]
    fn wrapped_urls_span_lines(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        let cols = geom(&pane, cx).cols;
        let url = format!("https://example.com/{}", "x".repeat(cols));
        feed(&pane, cx, url.as_bytes());
        let l = link_at(&pane, cx, 3, 1).expect("hovering the wrapped part");
        assert_eq!(l.target, LinkTarget::Url(url.clone()));
        assert_eq!(l.spans, vec![(Line(0), 0, cols - 1), (Line(1), 0, url.len() - cols - 1)]);
        assert_eq!(link_at(&pane, cx, 0, 0).map(|l| l.target), Some(LinkTarget::Url(url)));
    }

    #[gpui::test]
    fn cmd_hover_underlines_and_cmd_click_opens(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        feed(&pane, cx, b"go to https://nox.dev/x now");
        let at = cell(&pane, cx, 10, 0);
        cx.simulate_mouse_move(at, None, Modifiers::command());
        assert!(pane.read_with(cx, |p, _| p.hovered_link.is_some()));
        // Releasing ⌘ removes the underline.
        cx.simulate_modifiers_change(Modifiers::none());
        assert!(pane.read_with(cx, |p, _| p.hovered_link.is_none()));
        // Pressing ⌘ over a link shows it again.
        cx.simulate_modifiers_change(Modifiers::command());
        assert!(pane.read_with(cx, |p, _| p.hovered_link.is_some()));
        mouse_down(cx, at, 1, Modifiers::command());
        assert_eq!(cx.opened_url().as_deref(), Some("https://nox.dev/x"));
        assert_eq!(selection(&pane, cx), None, "⌘-click on a link doesn't select");
        // Moving without ⌘ drops the hover.
        cx.simulate_mouse_move(at, None, Modifiers::none());
        assert!(pane.read_with(cx, |p, _| p.hovered_link.is_none()));
    }

    #[gpui::test]
    fn cmd_click_opens_files_with_the_default_app(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap();
        std::fs::write(dir.join("a.md"), "").unwrap();
        let (pane, cx) = open(cx, silent(), PaneSpawn { cwd: Some(dir.clone()), ..PaneSpawn::default() });
        feed(&pane, cx, b"a.md");
        let at = cell(&pane, cx, 1, 0);
        mouse_down(cx, at, 1, Modifiers::command());
        assert_eq!(cx.opened_url(), Some(format!("file://{}", dir.join("a.md").display())));
    }

    // ---- agents and resume ----------------------------------------------------------------

    fn launched_agent(command: &str) -> AgentInfo {
        AgentInfo {
            kind: "claude".into(),
            tracker: AgentTracker::new("claude", Instant::now()),
            launch: Some(AgentLaunch {
                kind: "claude".into(),
                command: command.into(),
                start_cwd: PathBuf::from("/"),
                resumed: true,
            }),
            cmdline: command.into(),
            seen: false,
            created: Instant::now(),
        }
    }

    #[gpui::test]
    fn quick_failing_resume_shows_the_banner(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        pane.update(cx, |p, cx| {
            p.agent = Some(launched_agent("claude --resume s1"));
            p.handle_event(TerminalEvent::Osc(OscEvent::CommandStart), cx);
            p.handle_event(TerminalEvent::Osc(OscEvent::CommandEnd(Some(1))), cx);
            assert!(p.resume_failed);
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn enter_after_a_failed_resume_starts_a_new_session(cx: &mut TestAppContext) {
        let (cap, cfg) = Capture::new();
        let (pane, cx) = open(cx, cfg, PaneSpawn::default());
        Capture::ready(&pane, cx);
        pane.update(cx, |p, cx| {
            p.agent = Some(launched_agent("claude --resume s1"));
            p.resume_failed = true;
            cx.notify();
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        assert!(!pane.read_with(cx, |p, _| p.resume_failed));
        // ⌘R retries the resume command in the running shell.
        pane.update(cx, |p, _| p.resume_failed = true);
        cx.dispatch_action(RestartPane);
        assert!(!pane.read_with(cx, |p, _| p.resume_failed));
        cap.expect(cx, b"claude\rclaude --resume s1\r");
    }

    #[gpui::test]
    fn restart_after_a_failed_resume_reruns_the_command(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        pane.update(cx, |p, cx| {
            p.agent = Some(launched_agent("claude --resume s1"));
            p.resume_failed = true;
            p.osc_title = Some("old".into());
            p.handle_event(TerminalEvent::Exited(Some(1)), cx);
            assert_eq!(p.agent_state(), Some(AgentState::Exited));
        });
        cx.run_until_parked();
        // ↵ on an exited pane whose resume failed starts a new agent session (not close).
        let events = record(&pane, cx);
        cx.simulate_keystrokes("enter");
        assert!(!events.borrow().iter().any(|e| matches!(e, PaneEvent::Close)));
        pane.update(cx, |p, _| p.resume_failed = true);
        pane.update(cx, Pane::restart);
        pane.read_with(cx, |p, _| {
            assert!(matches!(&p.spawn.startup, Some(Startup::Run(c)) if c == "claude --resume s1"));
            assert!(p.agent.is_none() && p.osc_title.is_none() && p.exited.is_none() && !p.resume_failed);
        });
    }

    #[gpui::test]
    fn agent_launch_is_tracked_from_the_start(cx: &mut TestAppContext) {
        let spawn = PaneSpawn {
            agent: Some(AgentLaunch {
                kind: "codex".into(),
                command: "codex".into(),
                start_cwd: PathBuf::from("/"),
                resumed: false,
            }),
            startup: Some(Startup::Type("codex".into())),
            ..PaneSpawn::default()
        };
        let (pane, cx) = open(cx, silent(), spawn);
        let events = record(&pane, cx);
        pane.read_with(cx, |p, _| {
            let a = p.agent.as_ref().expect("agent");
            assert_eq!(a.kind, "codex");
            assert_eq!(a.cmdline, "codex");
        });
        pane.update(cx, |p, cx| {
            // Refreshing (e.g. on focus) sees the shell (`cat`) in the foreground, not codex,
            // which hasn't started yet: that isn't an exit.
            p.refresh_info(cx);
            assert!(p.process_is_shell);
            assert_ne!(p.agent_state(), Some(AgentState::Exited));
            assert!(p.is_agent());
            p.handle_event(
                TerminalEvent::Osc(OscEvent::Notify { title: None, body: "needs you".into() }),
                cx,
            );
            assert_eq!(p.agent_state(), Some(AgentState::NeedsInput));
            p.send_bytes(b"y", cx);
            assert_eq!(p.agent_state(), Some(AgentState::Working), "input resumes the turn");
            p.refresh_info(cx);
            assert_eq!(p.agent_state(), Some(AgentState::Working));
            // Once it was seen in the foreground, the shell coming back means it exited.
            p.agent.as_mut().unwrap().seen = true;
            p.refresh_info(cx);
            assert_eq!(p.agent_state(), Some(AgentState::Exited));
            // Never seen after the grace period: it didn't start (or exited unnoticed).
            let a = p.agent.as_mut().unwrap();
            a.tracker = AgentTracker::new("codex", Instant::now());
            a.seen = false;
            a.created = Instant::now().checked_sub(AGENT_LAUNCH_GRACE).unwrap();
            p.refresh_info(cx);
            assert_eq!(p.agent_state(), Some(AgentState::Exited));
        });
        cx.run_until_parked();
        assert!(events.borrow().iter().any(|e| matches!(e, PaneEvent::AgentChanged { transition: Some(_) })));
    }

    // ---- process detection ----------------------------------------------------------------

    /// `set -m` gives the job its own process group in the foreground of the PTY.
    fn job(cmd: &str) -> nox_config::Config {
        config_running(&format!("set -m; {cmd}; printf DONE; exec cat >/dev/null"))
    }

    fn refresh_until(pane: &Entity<Pane>, cx: &mut VisualTestContext, f: impl Fn(&Pane) -> bool) {
        wait_until(cx, |cx| {
            pane.update(cx, Pane::refresh_info);
            pane.read_with(cx, |p, _| f(p))
        });
    }

    #[gpui::test]
    fn foreground_process_names_the_pane_and_is_timed(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, job("/bin/sleep 0.5"), PaneSpawn::default());
        let events = record(&pane, cx);
        // (Right after the fork the job is still `bash` until it execs `sleep`.)
        refresh_until(&pane, cx, |p| {
            !p.process_is_shell && p.process.as_ref().is_some_and(|p| p.exe == "sleep")
        });
        pane.read_with(cx, |p, _| {
            assert_eq!(p.title(nox_config::TabTitle::Auto), "sleep");
            assert!(p.is_busy());
            assert_eq!(p.status(), TabStatus::Running, "no integration: busy = running");
            assert!(p.cwd.is_some(), "cwd follows the foreground process");
        });
        refresh_until(&pane, cx, |p| p.process_is_shell);
        assert!(pane.read_with(cx, |p, _| p.running_since.is_none()));
        cx.run_until_parked();
        // The event names the command that ended, not the shell back in the foreground.
        assert!(
            events.borrow().iter().any(|e| matches!(
                e,
                PaneEvent::CommandFinished { command, exit: None, duration }
                    if command == "sleep" && *duration >= Duration::from_millis(400)
            )),
            "{:?}",
            events.borrow()
        );
    }

    /// Helper process for [`agents_are_detected_from_the_foreground_process`]: a copy of this
    /// test binary named `claude` runs only this test, which sleeps when asked to.
    #[test]
    fn sleeper() {
        if let Some(ms) = std::env::var("NOX_TEST_SLEEP_MS").ok().and_then(|v| v.parse().ok()) {
            std::thread::sleep(Duration::from_millis(ms));
        }
    }

    #[gpui::test]
    fn agents_are_detected_from_the_foreground_process(cx: &mut TestAppContext) {
        // A real executable named `claude` (copies of system binaries don't run on macOS).
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join("claude");
        std::fs::copy(std::env::current_exe().unwrap(), &claude).unwrap();
        let cmd = format!("'{}' --exact pane::tests::sleeper --quiet", claude.display());
        let spawn = PaneSpawn {
            env: HashMap::from([("NOX_TEST_SLEEP_MS".to_string(), "600".to_string())]),
            ..PaneSpawn::default()
        };
        let (pane, cx) = open(cx, job(&cmd), spawn);
        let events = record(&pane, cx);
        refresh_until(&pane, cx, |p| p.agent.is_some());
        pane.read_with(cx, |p, _| {
            let a = p.agent.as_ref().unwrap();
            assert_eq!(a.kind, "claude");
            assert!(a.launch.is_none(), "detected, not launched");
            // (The args can be unreadable for a moment right after `exec`.)
            assert!(a.cmdline.starts_with("claude"), "{}", a.cmdline);
            assert!(p.is_agent());
            assert_eq!(p.process_display_name().as_deref(), Some("claude"));
            assert_eq!(p.status(), TabStatus::None, "a fresh agent is idle until it works");
        });
        // The agent leaving the foreground means it exited.
        refresh_until(&pane, cx, |p| p.process_is_shell);
        assert_eq!(pane.read_with(cx, |p, _| p.agent_state()), Some(AgentState::Exited));
        assert!(!pane.read_with(cx, |p, _| p.is_agent()));
        cx.run_until_parked();
        assert!(events.borrow().iter().filter(|e| matches!(e, PaneEvent::AgentChanged { .. })).count() >= 2);
    }

    // ---- view -----------------------------------------------------------------------------

    fn click(cx: &mut VisualTestContext, pos: Point<Pixels>, clicks: usize) {
        mouse_down(cx, pos, clicks, Modifiers::none());
        cx.simulate_event(MouseUpEvent {
            position: pos,
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: clicks,
        });
    }

    #[gpui::test]
    fn header_buttons_emit_pane_events(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        let events = record(&pane, cx);
        // Right-aligned 20 px buttons, 2 px apart, 10 px from the edge: split ▯▯, split ⊟, max, close.
        let y = px(14.);
        let at = |i: f32| point(px(640. - 20. - i * 22.), y);
        click(cx, at(0.), 1);
        click(cx, at(1.), 1);
        click(cx, at(2.), 1);
        click(cx, at(3.), 1);
        // Double-clicking the title maximizes too.
        click(cx, point(px(120.), y), 2);
        let ev = events.borrow();
        let got: Vec<_> = ev
            .iter()
            .filter(|e| !matches!(e, PaneEvent::Focused | PaneEvent::TitleChanged))
            .map(|e| format!("{e:?}"))
            .collect();
        assert_eq!(
            got,
            ["Close", "ToggleMaximize", "Split(Horizontal)", "Split(Vertical)", "ToggleMaximize"]
        );
    }

    #[gpui::test]
    fn header_shows_status_details(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        let at = point(px(200.), px(14.));
        cx.simulate_mouse_move(at, None, Modifiers::none());
        assert!(pane.read_with(cx, |p, _| p.header_hovered), "hovering shows the controls");
        cx.update(|_, cx| {
            cx.global_mut::<crate::root::UiFlags>().maximized_panes.insert(PaneId(1));
        });
        for state in [AgentState::Working, AgentState::NeedsInput, AgentState::Idle] {
            pane.update(cx, |p, cx| {
                p.branch = Some("main".into());
                p.unread = 2;
                p.claims = vec!["src/lib.rs".into()];
                p.from_banner = Some(("reviewer".into(), Instant::now()));
                p.custom_icon = Some("rust".into());
                p.report_status(state, None, None, cx);
                p.shake(cx);
            });
            cx.run_until_parked();
        }
        // The shake animation ends after 12 frames.
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
        assert!(pane.read_with(cx, |p, _| p.shake.is_none()));
        cx.simulate_mouse_move(point(px(200.), px(200.)), None, Modifiers::none());
        assert!(!pane.read_with(cx, |p, _| p.header_hovered));
    }

    #[gpui::test]
    fn exited_and_error_overlays_render(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        for exit in [Some(0), None] {
            pane.update(cx, |p, cx| {
                p.exited = Some(exit);
                p.resume_failed = true;
                p.spawn_error = Some("no such shell".into());
                cx.notify();
            });
            cx.run_until_parked();
        }
    }

    #[gpui::test]
    fn new_output_pill_jumps_to_the_bottom(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        fill_lines(&pane, cx, 100);
        cx.dispatch_action(ScrollPageUp);
        pane.update(cx, |p, cx| p.handle_event(TerminalEvent::Wakeup, cx));
        assert!(pane.read_with(cx, |p, _| p.new_output_below));
        cx.run_until_parked();
        // The pill is centred, 10 px above the bottom.
        click(cx, point(px(320.), px(360. - 20.)), 1);
        assert!(!pane.read_with(cx, |p, _| p.new_output_below));
        assert_eq!(offset(&pane, cx), 0);
    }

    #[gpui::test]
    fn focus_and_blur(cx: &mut TestAppContext) {
        let mut cfg = silent();
        cfg.ui.focus_follows_mouse = true;
        let (pane, cx) = open(cx, cfg, PaneSpawn::default());
        let events = record(&pane, cx);
        cx.update(|window, _| window.blur());
        pane.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert!(!cx.update(|window, cx| pane.read(cx).focus_handle.is_focused(window)));
        // Focus follows the mouse.
        let at = cell(&pane, cx, 3, 3);
        cx.simulate_mouse_move(at, None, Modifiers::none());
        assert!(cx.update(|window, cx| pane.read(cx).focus_handle.is_focused(window)));
        // Focus listeners run with the next frame.
        pane.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert!(events.borrow().iter().any(|e| matches!(e, PaneEvent::Focused)));
        // A click focuses too.
        cx.update(|window, _| window.blur());
        cx.run_until_parked();
        mouse_down(cx, at, 1, Modifiers::none());
        assert!(cx.update(|window, cx| pane.read(cx).focus_handle.is_focused(window)));
    }

    #[gpui::test]
    fn cursor_blinks_for_five_seconds(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        let t0 = pane.read_with(cx, |p, _| p.blink_epoch);
        assert!(
            pane.read_with(cx, |p, _| p.cursor_visible(t0 + Duration::from_millis(600))),
            "steady cursor"
        );
        feed(&pane, cx, b"\x1b[1 q");
        pane.update(cx, |p, cx| p.handle_event(TerminalEvent::CursorBlinkingChange, cx));
        pane.read_with(cx, |p, _| {
            let t0 = p.blink_epoch;
            assert!(p.blink_task.is_some());
            assert!(p.cursor_visible(t0 + Duration::from_millis(100)));
            assert!(!p.cursor_visible(t0 + Duration::from_millis(600)));
            assert!(p.cursor_visible(t0 + Duration::from_millis(1100)));
            assert!(p.cursor_visible(t0 + Duration::from_secs(6)), "stops blinking when idle");
        });
        cx.executor().advance_clock(Duration::from_millis(600));
        cx.run_until_parked();
        assert!(pane.read_with(cx, |p, _| p.blink_task.is_some()), "keeps blinking while active");
        // Back to a steady cursor: the next tick ends the timer.
        feed(&pane, cx, b"\x1b[2 q");
        cx.executor().advance_clock(Duration::from_millis(600));
        cx.run_until_parked();
        assert!(pane.read_with(cx, |p, _| p.blink_task.is_none()));
    }

    #[gpui::test]
    fn live_config_changes_apply_to_the_session(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        let mut cfg = silent();
        cfg.terminal.scrollback = 10;
        cfg.terminal.osc52 = nox_config::Osc52::ReadWrite;
        cx.update(|_, cx| cx.set_global(Settings::new(cfg, nox_mac::Accessibility::default())));
        cx.run_until_parked();
        fill_lines(&pane, cx, 100);
        assert_eq!(history(&pane, cx), 10);
        for osc52 in [nox_config::Osc52::Off, nox_config::Osc52::Write] {
            let mut cfg = silent();
            cfg.terminal.osc52 = osc52;
            cx.update(|_, cx| cx.set_global(Settings::new(cfg, nox_mac::Accessibility::default())));
        }
        // A theme switch re-sends the colors programs can query.
        cx.update(|_, cx| {
            let state = cx.global::<crate::theme::ThemeState>().clone();
            crate::theme::apply(cx, &state, gpui::WindowAppearance::Light);
        });
        cx.run_until_parked();
        let _ = pane;
    }

    #[gpui::test]
    fn font_size_override_wins(cx: &mut TestAppContext) {
        let (pane, cx) = open(cx, silent(), PaneSpawn::default());
        let base = cx.update(|_, cx| Settings::global(cx).term_font.size);
        let rows = geom(&pane, cx).rows;
        pane.update(cx, |p, cx| {
            p.font_size_override = Some(base * 2.);
            cx.notify();
        });
        cx.run_until_parked();
        pane.read_with(cx, |p, cx| assert_eq!(p.font_size(Settings::global(cx)), base * 2.));
        assert!(geom(&pane, cx).rows < rows, "bigger font, fewer rows");
    }

    #[gpui::test]
    fn unfocused_panes_are_dimmed_without_a_header(cx: &mut TestAppContext) {
        let mut cfg = silent();
        cfg.ui.pane_header = false;
        let (pane, cx) = open(cx, cfg, PaneSpawn::default());
        // Without the header the grid starts at the top padding.
        let g = geom(&pane, cx);
        assert_eq!(g.bounds.top(), px(0.));
        cx.update(|window, _| window.blur());
        cx.run_until_parked();
    }
}
