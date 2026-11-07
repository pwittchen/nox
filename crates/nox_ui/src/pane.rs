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

use crate::actions::*;
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
    _tasks: Vec<Task<()>>,
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
        let mut this = Self {
            id,
            session: None,
            spawn_error: None,
            focus_handle: focus_handle.clone(),
            spawn: spawn.clone(),
            custom_name: spawn.name.clone(),
            custom_icon: None,
            osc_title: None,
            cwd: spawn.cwd.clone(),
            process: None,
            process_is_shell: true,
            icon: Icon::TERMINAL,
            branch: None,
            last_refresh: None,
            refresh_task: None,
            shell_integration: false,
            running_since: None,
            done_unseen: false,
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
            _tasks: vec![],
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
        match TerminalSession::spawn(opts) {
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
                self._tasks.push(task);
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
        self._tasks.clear();
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
                    let wait = Duration::from_millis(500) - now.duration_since(t);
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
                        let command = self.process.as_ref().map(|p| p.exe.clone()).unwrap_or_default();
                        finished = Some((command, start.elapsed()));
                    }
                    _ => {}
                }
            }
        }
        let proc_for_icon = if self.process_is_shell { None } else { self.process.as_ref() };
        let _ = &finished;
        let icon = match &self.custom_icon {
            Some(name) => registry.custom_icon(name),
            None => registry.table.icon_for(
                self.process.as_ref().filter(|_| !self.process_is_shell).or(proc_for_icon),
                self.osc_title.as_deref(),
                &login_shell,
            ),
        };
        if icon != self.icon {
            self.icon = icon;
            changed = true;
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
                let cmdline = self
                    .process
                    .as_ref()
                    .map(|p| {
                        std::iter::once(p.exe.clone())
                            .chain(p.args.iter().cloned())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_else(|| kind.clone());
                self.agent = Some(AgentInfo { kind, tracker, launch: None, cmdline });
                changed = true;
                cx.emit(PaneEvent::AgentChanged { transition: None });
            }
            (Some(a), None) if self.process_is_shell && a.tracker.state() != AgentState::Exited => {
                // The agent left the foreground: it exited.
                let t = a.tracker.on_exit(Instant::now());
                cx.emit(PaneEvent::AgentChanged { transition: t });
                changed = true;
            }
            (Some(a), Some(kind)) if a.tracker.state() == AgentState::Exited || a.kind != kind => {
                let now = Instant::now();
                a.kind = kind.clone();
                a.tracker = AgentTracker::new(kind, now);
                changed = true;
                cx.emit(PaneEvent::AgentChanged { transition: None });
            }
            _ => {}
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
            });
        }
        if let Some(a) = &mut self.agent {
            let t = a.tracker.on_report(state, msg, session, now);
            cx.emit(PaneEvent::AgentChanged { transition: t });
        }
        cx.notify();
    }

    // ---- derived info ---------------------------------------------------------------------

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

    pub fn cwd_display(&self) -> String {
        match &self.cwd {
            Some(p) => nox_terminal::process::tildify(p),
            None => "~".into(),
        }
    }

    /// The pane title (SPEC §4.4).
    pub fn title(&self, mode: nox_config::TabTitle) -> String {
        if let Some(n) = &self.custom_name {
            return n.clone();
        }
        match mode {
            nox_config::TabTitle::Cwd => return self.cwd_display(),
            nox_config::TabTitle::Process => {
                return self.process_display_name().unwrap_or_else(|| {
                    self.process.as_ref().map(|p| p.exe.clone()).unwrap_or_else(|| "shell".into())
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

    pub fn agent_state(&self) -> Option<AgentState> {
        self.agent.as_ref().map(|a| a.tracker.state())
    }

    pub fn is_agent(&self) -> bool {
        self.agent.as_ref().is_some_and(|a| a.tracker.state() != AgentState::Exited)
    }

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
            return TabStatus::Done;
        }
        TabStatus::None
    }

    /// Mark everything as seen (the user is looking at this pane).
    pub fn mark_seen(&mut self) {
        self.done_unseen = false;
        self.bell_unseen = false;
    }

    /// Something other than the shell runs (close confirmation, SPEC §6.4).
    pub fn is_busy(&self) -> bool {
        self.exited.is_none() && self.session.as_ref().is_some_and(|s| s.is_busy())
    }

    pub fn font_size(&self, settings: &Settings) -> f32 {
        self.font_size_override.unwrap_or(settings.term_font.size)
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if let Some(s) = &self.session {
            s.write(bytes);
        }
    }

    // ---- cursor blink ---------------------------------------------------------------------

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
                nox_config::OptionAsMeta::Left => l || (!l && !r),
                nox_config::OptionAsMeta::Right => r,
                nox_config::OptionAsMeta::Both => true,
                nox_config::OptionAsMeta::None => false,
            }
        };
        // Plain text goes through the input handler (IME); encode only what isn't text.
        let is_plain_text = !m.control
            && !m.platform
            && !(m.alt && alt_is_meta)
            && ks.key_char.as_ref().is_some_and(|c| !c.is_empty() && !c.chars().any(|ch| ch.is_control()))
            && !matches!(ks.key.as_str(), "enter" | "tab" | "backspace" | "escape" | "space")
            && !(ks.key == "space" && m.shift);
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

    fn mouse_mods(m: &Modifiers) -> MouseMods {
        MouseMods { shift: m.shift, alt: m.alt, ctrl: m.control }
    }

    fn report_mouse(
        &self,
        button: mouse::MouseButton,
        action: MouseAction,
        m: &Modifiers,
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
            && self.report_mouse(button_of(ev.button), MouseAction::Press, &ev.modifiers, ev.position)
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
        if self.report_mouse(button_of(ev.button), MouseAction::Release, &ev.modifiers, ev.position)
            && self.selecting.is_none()
        {
            return;
        }
        let _ = window;
        self.selecting = None;
        self.pending_selection = None;
        self.autoscroll = None;
        if Settings::global(cx).config.terminal.copy_on_select {
            self.copy_selection(cx);
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
        let pressed = ev.pressed_button;
        if self.selecting.is_none() {
            let button = pressed.map(button_of).unwrap_or(mouse::MouseButton::None);
            self.report_mouse(button, MouseAction::Motion, &ev.modifiers, ev.position);
            return;
        }
        if pressed != Some(MouseButton::Left) {
            self.selecting = None;
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
            if over != 0.0 {
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
            } else {
                self.autoscroll = None;
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
                self.report_mouse(button, MouseAction::Press, &ev.modifiers, ev.position);
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
            let dur = Duration::from_millis(cfg.terminal.smooth_scroll_duration as u64);
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

    pub fn find_highlights(&self) -> Vec<Match> {
        self.find.as_ref().map(|f| f.matches.clone()).unwrap_or_default()
    }

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
        let mut pattern = if find.regex { query.clone() } else { regex_escape(&query) };
        if !find.case_sensitive {
            pattern = format!("(?i){pattern}");
        }
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
                let trimmed: String = t.lines().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n");
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
        if let Some(text) = self.session.as_ref().and_then(|s| s.last_output()) {
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
                AgentState::NeedsInput => theme.ansi(5),
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
                    f.case_sensitive = !f.case_sensitive
                }))
                .child(toggle("find-regex", ".*", find.regex, cx, |f| f.regex = !f.regex))
                .into_any_element(),
        )
    }
}

fn button_of(b: MouseButton) -> mouse::MouseButton {
    match b {
        MouseButton::Left => mouse::MouseButton::Left,
        MouseButton::Middle => mouse::MouseButton::Middle,
        MouseButton::Right => mouse::MouseButton::Right,
        _ => mouse::MouseButton::Left,
    }
}

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
                                &ev.modifiers,
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

pub fn status_color(theme: &Theme, status: TabStatus) -> Option<Hsla> {
    match status {
        TabStatus::None => None,
        TabStatus::Done => Some(theme.ansi(4)),
        TabStatus::Running => Some(theme.ansi(3)),
        TabStatus::NeedsInput => Some(theme.ansi(5)),
        TabStatus::Bell => Some(theme.accent),
    }
}
