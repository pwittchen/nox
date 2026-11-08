//! One terminal session: a PTY, its shell, an `alacritty_terminal::Term`, and the threads that
//! connect them (SPEC §8).
//!
//! - **Reader thread**: blocking `poll` + `read` on the PTY master into a 64 KiB buffer; each
//!   chunk goes through the OSC sniffer, then into the VT parser under the `Term`'s
//!   `FairMutex` (released after every chunk so the UI can paint). After parsing it sends one
//!   `Wakeup` unless one is already pending — the UI coalesces into one repaint per frame.
//! - **Waiter thread**: blocking `waitpid` on the shell; reports the exit status.
//! - **Writer thread**: large writes (pastes) so the UI thread never blocks; small writes
//!   (keystrokes, `^C`) are written directly from the caller's thread.
//! - **Resize thread**: applies coalesced `TIOCSWINSZ` (see [`crate::resize`]); idle when the
//!   size isn't changing.
//!
//! No thread polls while idle.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config as TermConfig, Osc52, Term};
use alacritty_terminal::tty::{self, Options as PtyOptions, Shell};
use alacritty_terminal::vte::ansi::{Processor, Rgb, StdSyncHandler};

use crate::grapheme::GraphemeInput;
use crate::osc::{OscEvent, OscSniffer};
use crate::resize::{GridSize, ResizeCoalescer};

/// Events from a session to the UI.
#[derive(Debug, Clone)]
pub enum TerminalEvent {
    /// New content was parsed; repaint.
    Wakeup,
    Title(String),
    ResetTitle,
    Bell,
    /// OSC 52 store (already filtered by the `osc52` policy).
    ClipboardStore(String),
    /// OSC 52 load request (only when `osc52 = read-write`); answer with
    /// [`TerminalSession::answer_clipboard_load`].
    ClipboardLoad,
    CursorBlinkingChange,
    /// OSC 7 / 133 / 9 / 777, sniffed from the stream.
    Osc(OscEvent),
    /// The shell exited (status code; `None` if killed by a signal).
    Exited(Option<i32>),
}

/// Policy for OSC 52 clipboard access (`terminal.osc52`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardPolicy {
    Off,
    Write,
    ReadWrite,
}

/// The configured cursor (`terminal.cursor_style`, `terminal.cursor_blink`); programs can
/// still change it with DECSCUSR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorKind {
    Block,
    Bar,
    Underline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorConfig {
    pub shape: CursorKind,
    pub blinking: bool,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self { shape: CursorKind::Block, blinking: true }
    }
}

#[derive(Debug, Clone)]
pub struct SpawnOptions {
    /// Program to run; `None` → the login shell.
    pub program: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Extra environment, on top of the app's and the TERM variables.
    pub env: HashMap<String, String>,
    pub size: GridSize,
    pub scrollback: usize,
    pub word_separators: String,
    pub kitty_keyboard: bool,
    pub osc52: ClipboardPolicy,
    /// `terminal.grapheme_width = "unicode"`: emoji sequences form one cell (see
    /// [`crate::grapheme`]); `false` is `"legacy"`.
    pub grapheme_clusters: bool,
    pub cursor: CursorConfig,
    /// Directory with the shell integration files, when `shell.integration = true`.
    pub integration_dir: Option<PathBuf>,
}

impl Default for SpawnOptions {
    fn default() -> Self {
        Self {
            program: None,
            args: vec!["-l".into()],
            cwd: None,
            env: HashMap::new(),
            size: GridSize { cols: 80, rows: 24, cell_w: 8, cell_h: 16 },
            scrollback: 10_000,
            word_separators: " ()[]{}',\"`│".into(),
            kitty_keyboard: false,
            osc52: ClipboardPolicy::Write,
            grapheme_clusters: true,
            cursor: CursorConfig::default(),
            integration_dir: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TermDims {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermDims {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Number of entries in the color table answered to OSC 4/10/11/12 queries:
/// 256 indexed colors + foreground, background, cursor and the dim/bright extras.
pub const COLOR_COUNT: usize = 269;

/// Bridges `Term` events to the UI channel and answers terminal queries.
#[derive(Clone)]
pub struct Listener {
    tx: async_channel::Sender<TerminalEvent>,
    writer: Arc<Mutex<File>>,
    size: Arc<Mutex<GridSize>>,
    colors: Arc<RwLock<Vec<Rgb>>>,
    osc52: Arc<RwLock<ClipboardPolicy>>,
    clipboard_reply: ClipboardReply,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let ev = match event {
            Event::Title(t) => TerminalEvent::Title(t),
            Event::ResetTitle => TerminalEvent::ResetTitle,
            Event::Bell => TerminalEvent::Bell,
            Event::CursorBlinkingChange => TerminalEvent::CursorBlinkingChange,
            Event::ClipboardStore(_, text) => {
                if *self.osc52.read().unwrap() == ClipboardPolicy::Off {
                    return;
                }
                TerminalEvent::ClipboardStore(text)
            }
            Event::ClipboardLoad(_, fmt) => {
                if *self.osc52.read().unwrap() != ClipboardPolicy::ReadWrite {
                    return;
                }
                *self.clipboard_reply.lock().unwrap() = Some(fmt);
                TerminalEvent::ClipboardLoad
            }
            Event::PtyWrite(text) => {
                let _ = write_all_fd(&self.writer, text.as_bytes());
                return;
            }
            Event::ColorRequest(index, fmt) => {
                let colors = self.colors.read().unwrap();
                if let Some(c) = colors.get(index) {
                    let reply = fmt(*c);
                    drop(colors);
                    let _ = write_all_fd(&self.writer, reply.as_bytes());
                }
                return;
            }
            Event::TextAreaSizeRequest(fmt) => {
                let s = *self.size.lock().unwrap();
                let reply = fmt(window_size(s));
                let _ = write_all_fd(&self.writer, reply.as_bytes());
                return;
            }
            Event::Wakeup | Event::MouseCursorDirty | Event::Exit | Event::ChildExit(_) => return,
        };
        let _ = self.tx.try_send(ev);
    }
}

fn window_size(s: GridSize) -> WindowSize {
    WindowSize { num_lines: s.rows, num_cols: s.cols, cell_width: s.cell_w, cell_height: s.cell_h }
}

/// A prompt mark recorded at an absolute line (history size + cursor line at the time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub kind: MarkKind,
    pub abs_line: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    Prompt,
    CommandStart,
    CommandEnd,
}

#[derive(Debug, Default)]
struct Marks {
    marks: Vec<Mark>,
}

impl Marks {
    fn push(&mut self, m: Mark) {
        if self.marks.len() >= 4096 {
            self.marks.drain(..1024);
        }
        self.marks.push(m);
    }
}

pub type SharedTerm = Arc<FairMutex<Term<Listener>>>;

/// Formats the reply to an OSC 52 clipboard load.
type ClipboardReply = Arc<Mutex<Option<Arc<dyn Fn(&str) -> String + Sync + Send>>>>;

pub struct TerminalSession {
    term: SharedTerm,
    writer: Arc<Mutex<File>>,
    fd: RawFd,
    pid: i32,
    _pty: Arc<Mutex<tty::Pty>>,
    events: async_channel::Receiver<TerminalEvent>,
    dirty: Arc<AtomicBool>,
    resize_tx: mpsc::Sender<GridSize>,
    write_tx: mpsc::Sender<Vec<u8>>,
    size: Arc<Mutex<GridSize>>,
    colors: Arc<RwLock<Vec<Rgb>>>,
    osc52: Arc<RwLock<ClipboardPolicy>>,
    grapheme_clusters: Arc<AtomicBool>,
    clipboard_reply: ClipboardReply,
    marks: Arc<Mutex<Marks>>,
    exited: Arc<AtomicBool>,
    last_output: Arc<AtomicU64>,
    started: Instant,
}

/// Monotonic milliseconds since an arbitrary process-wide epoch.
fn now_ms() -> u64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

impl TerminalSession {
    pub fn spawn(opts: &SpawnOptions) -> io::Result<Self> {
        let program = opts.program.clone().unwrap_or_else(crate::process::login_shell);
        let mut args = opts.args.clone();
        let mut env: HashMap<String, String> = HashMap::new();
        env.insert("TERM".into(), "xterm-256color".into());
        env.insert("COLORTERM".into(), "truecolor".into());
        env.insert("TERM_PROGRAM".into(), "nox".into());
        env.insert("TERM_PROGRAM_VERSION".into(), env!("CARGO_PKG_VERSION").into());
        if std::env::var("LANG").map_or(true, |l| l.is_empty()) {
            env.insert("LANG".into(), "en_US.UTF-8".into());
        }
        if let Some(dir) = &opts.integration_dir
            && let Some(inj) = crate::shell::inject(dir, &program, &args)
        {
            args = inj.args;
            env.extend(inj.env);
        }
        env.extend(opts.env.clone());

        let pty_opts = PtyOptions {
            shell: Some(Shell::new(program, args)),
            working_directory: opts.cwd.clone().filter(|p| p.is_dir()),
            drain_on_exit: true,
            env,
            #[cfg(target_os = "windows")]
            escape_args: false,
        };
        let pty = tty::new(&pty_opts, window_size(opts.size), 0)?;
        let master = pty.file().try_clone()?;
        let fd = master.as_raw_fd();
        let pid = pty.child().id() as i32;
        let writer = Arc::new(Mutex::new(master.try_clone()?));

        let (tx, rx) = async_channel::unbounded();
        let size = Arc::new(Mutex::new(opts.size));
        let colors = Arc::new(RwLock::new(vec![Rgb { r: 0, g: 0, b: 0 }; COLOR_COUNT]));
        let osc52 = Arc::new(RwLock::new(opts.osc52));
        let grapheme_clusters = Arc::new(AtomicBool::new(opts.grapheme_clusters));
        let clipboard_reply = Arc::new(Mutex::new(None));
        let listener = Listener {
            tx: tx.clone(),
            writer: writer.clone(),
            size: size.clone(),
            colors: colors.clone(),
            osc52: osc52.clone(),
            clipboard_reply: clipboard_reply.clone(),
        };
        let config = term_config(opts);
        let dims = TermDims { cols: opts.size.cols as usize, rows: opts.size.rows as usize };
        let term: SharedTerm = Arc::new(FairMutex::new(Term::new(config, &dims, listener)));

        let dirty = Arc::new(AtomicBool::new(false));
        let marks = Arc::new(Mutex::new(Marks::default()));
        let exited = Arc::new(AtomicBool::new(false));
        let exit_status: Arc<Mutex<Option<Option<i32>>>> = Arc::new(Mutex::new(None));
        let reader_done = Arc::new(AtomicBool::new(false));
        let last_output = Arc::new(AtomicU64::new(now_ms()));

        // Reader thread.
        {
            let term = term.clone();
            let tx = tx.clone();
            let dirty = dirty.clone();
            let marks = marks.clone();
            let exited = exited.clone();
            let exit_status = exit_status.clone();
            let reader_done = reader_done.clone();
            let last_output = last_output.clone();
            let graphemes = grapheme_clusters.clone();
            thread::Builder::new().name(format!("nox-pty-reader-{pid}")).spawn(move || {
                reader_loop(master, term, tx.clone(), dirty, marks, last_output, &graphemes);
                reader_done.store(true, Ordering::SeqCst);
                // Give the waiter a moment to collect the status.
                for _ in 0..50 {
                    if exit_status.lock().unwrap().is_some() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                let status = exit_status.lock().unwrap().unwrap_or(None);
                if !exited.swap(true, Ordering::SeqCst) {
                    let _ = tx.try_send(TerminalEvent::Exited(status));
                }
            })?;
        }

        // Waiter thread.
        {
            let tx = tx.clone();
            let exited = exited.clone();
            let exit_status = exit_status.clone();
            let reader_done = reader_done.clone();
            thread::Builder::new().name(format!("nox-pty-waiter-{pid}")).spawn(move || {
                let mut status: libc::c_int = 0;
                let rc = loop {
                    let rc = unsafe { libc::waitpid(pid, &raw mut status, 0) };
                    if rc == -1 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    break rc;
                };
                let code =
                    if rc == pid && libc::WIFEXITED(status) { Some(libc::WEXITSTATUS(status)) } else { None };
                *exit_status.lock().unwrap() = Some(code);
                // If a background process keeps the PTY open the reader never sees EOF; report
                // the exit anyway after a short grace period.
                thread::sleep(Duration::from_millis(300));
                if !reader_done.load(Ordering::SeqCst) && !exited.swap(true, Ordering::SeqCst) {
                    let _ = tx.try_send(TerminalEvent::Exited(code));
                }
            })?;
        }

        // Resize thread.
        let (resize_tx, resize_rx) = mpsc::channel::<GridSize>();
        {
            let initial = opts.size;
            thread::Builder::new().name(format!("nox-pty-resize-{pid}")).spawn(move || {
                let mut co = ResizeCoalescer::with_initial(initial);
                loop {
                    let msg = match co.deadline() {
                        Some(d) => {
                            let wait = d.saturating_duration_since(Instant::now());
                            match resize_rx.recv_timeout(wait) {
                                Ok(s) => Some(s),
                                Err(mpsc::RecvTimeoutError::Timeout) => None,
                                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                            }
                        }
                        None => match resize_rx.recv() {
                            Ok(s) => Some(s),
                            Err(_) => return,
                        },
                    };
                    let now = Instant::now();
                    let send = match msg {
                        Some(s) => co.request(s, now),
                        None => co.poll(now),
                    };
                    if let Some(s) = send {
                        set_winsize(fd, s);
                    }
                }
            })?;
        }

        // Writer thread for large writes.
        let (write_tx, write_rx) = mpsc::channel::<Vec<u8>>();
        {
            let writer = writer.clone();
            thread::Builder::new().name(format!("nox-pty-writer-{pid}")).spawn(move || {
                while let Ok(buf) = write_rx.recv() {
                    if write_all_fd(&writer, &buf).is_err() {
                        break;
                    }
                }
            })?;
        }

        Ok(Self {
            term,
            writer,
            fd,
            pid,
            _pty: Arc::new(Mutex::new(pty)),
            events: rx,
            dirty,
            resize_tx,
            write_tx,
            size,
            colors,
            osc52,
            grapheme_clusters,
            clipboard_reply,
            marks,
            exited,
            last_output,
            started: Instant::now(),
        })
    }

    #[must_use]
    pub fn term(&self) -> &SharedTerm {
        &self.term
    }

    #[must_use]
    pub fn events(&self) -> async_channel::Receiver<TerminalEvent> {
        self.events.clone()
    }

    /// Call when handling a `Wakeup`, before reading the term, so the next output sends
    /// another one.
    pub fn clear_dirty(&self) {
        self.dirty.store(false, Ordering::SeqCst);
    }

    #[must_use]
    pub fn pid(&self) -> i32 {
        self.pid
    }

    #[must_use]
    pub fn pty_fd(&self) -> RawFd {
        self.fd
    }

    #[must_use]
    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn started(&self) -> Instant {
        self.started
    }

    /// Time since the last output chunk.
    #[must_use]
    pub fn since_last_output(&self) -> Duration {
        Duration::from_millis(now_ms().saturating_sub(self.last_output.load(Ordering::Relaxed)))
    }

    /// Write input to the program. Small writes go straight to the PTY; large ones through the
    /// writer thread.
    pub fn write(&self, bytes: &[u8]) {
        if bytes.is_empty() || self.has_exited() {
            return;
        }
        if bytes.len() <= 1024 {
            let _ = write_all_fd(&self.writer, bytes);
        } else {
            let _ = self.write_tx.send(bytes.to_vec());
        }
    }

    /// Paste text, bracketed when the program enabled bracketed paste. Newlines are sent as CR.
    pub fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE);
        let mut buf = Vec::with_capacity(text.len() + 12);
        if bracketed {
            buf.extend_from_slice(b"\x1b[200~");
            // Strip any embedded end marker so the paste can't escape the bracket.
            let cleaned = text.replace("\x1b[201~", "");
            buf.extend(cleaned.replace("\r\n", "\r").replace('\n', "\r").bytes());
            buf.extend_from_slice(b"\x1b[201~");
        } else {
            buf.extend(text.replace("\r\n", "\r").replace('\n', "\r").bytes());
        }
        self.write(&buf);
    }

    /// Paste as bracketed regardless of the program's mode (used by `nox send`).
    pub fn paste_bracketed(&self, text: &str, submit: bool) {
        let mut buf = b"\x1b[200~".to_vec();
        buf.extend(text.replace("\x1b[201~", "").replace("\r\n", "\r").replace('\n', "\r").bytes());
        buf.extend_from_slice(b"\x1b[201~");
        if submit {
            buf.push(b'\r');
        }
        self.write(&buf);
    }

    /// Resize the grid now; the PTY resize is coalesced.
    pub fn resize(&self, size: GridSize) {
        {
            let mut cur = self.size.lock().unwrap();
            if *cur == size {
                return;
            }
            *cur = size;
        }
        self.term
            .lock()
            .resize(TermDims { cols: size.cols.max(2) as usize, rows: size.rows.max(1) as usize });
        let _ = self.resize_tx.send(size);
    }

    #[must_use]
    pub fn size(&self) -> GridSize {
        *self.size.lock().unwrap()
    }

    /// Colors used to answer OSC 4/10/11/12 queries (index per alacritty's `NamedColor`).
    pub fn set_query_colors(&self, colors: Vec<Rgb>) {
        *self.colors.write().unwrap() = colors;
    }

    /// Apply changed terminal settings live (config reload).
    pub fn set_options(
        &self,
        scrollback: usize,
        word_separators: &str,
        kitty: bool,
        osc52: ClipboardPolicy,
        grapheme_clusters: bool,
        cursor: CursorConfig,
    ) {
        *self.osc52.write().unwrap() = osc52;
        self.grapheme_clusters.store(grapheme_clusters, Ordering::Relaxed);
        let opts = SpawnOptions {
            scrollback,
            word_separators: word_separators.to_string(),
            kitty_keyboard: kitty,
            osc52,
            cursor,
            ..SpawnOptions::default()
        };
        self.term.lock().set_options(term_config(&opts));
    }

    pub fn answer_clipboard_load(&self, text: &str) {
        if let Some(f) = self.clipboard_reply.lock().unwrap().take() {
            let reply = f(text);
            let _ = write_all_fd(&self.writer, reply.as_bytes());
        }
    }

    /// Send SIGHUP to the shell's process group (closing the pane).
    pub fn kill(&self) {
        unsafe {
            libc::kill(-self.pid, libc::SIGHUP);
            libc::kill(self.pid, libc::SIGHUP);
        }
    }

    #[must_use]
    pub fn foreground(&self) -> Option<crate::process::ForegroundProcess> {
        crate::process::foreground_process(self.fd)
    }

    /// Whether something other than the shell itself is in the foreground.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        crate::process::foreground_pid(self.fd).is_some_and(|p| p != self.pid)
    }

    // ---- prompt marks & text extraction -------------------------------------------------

    #[must_use]
    pub fn marks(&self) -> Vec<Mark> {
        self.marks.lock().unwrap().marks.clone()
    }

    /// Lines (viewport-relative `Line` values, may be negative = in history) of prompt starts.
    #[must_use]
    pub fn prompt_lines(&self) -> Vec<i32> {
        let term = self.term.lock();
        let hist = term.grid().history_size() as i64;
        self.marks
            .lock()
            .unwrap()
            .marks
            .iter()
            .filter(|m| m.kind == MarkKind::Prompt)
            .map(|m| (m.abs_line - hist) as i32)
            .collect()
    }

    /// Plain text of the visible screen.
    #[must_use]
    pub fn screen_text(&self) -> String {
        let term = self.term.lock();
        let rows = term.screen_lines() as i32;
        let off = term.grid().display_offset() as i32;
        lines_text(&term, -off, rows - 1 - off)
    }

    /// The last `n` lines (screen bottom upwards, including history).
    #[must_use]
    pub fn last_lines(&self, n: usize) -> String {
        let term = self.term.lock();
        let rows = term.screen_lines() as i32;
        let top = (rows - n as i32).max(-(term.grid().history_size() as i32));
        lines_text(&term, top, rows - 1)
    }

    #[must_use]
    pub fn scrollback_text(&self) -> String {
        let term = self.term.lock();
        let rows = term.screen_lines() as i32;
        lines_text(&term, -(term.grid().history_size() as i32), rows - 1)
    }

    /// Output of the last finished command (between OSC 133 C and D). Needs shell integration.
    #[must_use]
    pub fn last_output(&self) -> Option<String> {
        let marks = self.marks.lock().unwrap().marks.clone();
        let end_idx = marks.iter().rposition(|m| m.kind == MarkKind::CommandEnd)?;
        let start = marks[..end_idx].iter().rev().find(|m| m.kind == MarkKind::CommandStart)?;
        let end = marks[end_idx];
        let term = self.term.lock();
        let hist = term.grid().history_size() as i64;
        let min = -(hist as i32);
        let from = ((start.abs_line - hist) as i32).max(min);
        // D is emitted at the start of the next prompt line; output ends on the line before.
        let to = ((end.abs_line - hist) as i32 - 1).max(from);
        if to < from {
            return Some(String::new());
        }
        Some(lines_text(&term, from, to))
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.kill();
    }
}

fn term_config(opts: &SpawnOptions) -> TermConfig {
    use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
    TermConfig {
        default_cursor_style: CursorStyle {
            shape: match opts.cursor.shape {
                CursorKind::Block => CursorShape::Block,
                CursorKind::Bar => CursorShape::Beam,
                CursorKind::Underline => CursorShape::Underline,
            },
            blinking: opts.cursor.blinking,
        },
        scrolling_history: opts.scrollback.min(100_000),
        semantic_escape_chars: opts.word_separators.clone(),
        kitty_keyboard: opts.kitty_keyboard,
        osc52: match opts.osc52 {
            ClipboardPolicy::Off => Osc52::Disabled,
            ClipboardPolicy::Write => Osc52::OnlyCopy,
            ClipboardPolicy::ReadWrite => Osc52::CopyPaste,
        },
        ..TermConfig::default()
    }
}

/// Text of lines `from..=to` (viewport-relative, negative = history), trailing spaces trimmed.
pub fn lines_text<T>(term: &Term<T>, from: i32, to: i32) -> String {
    if to < from {
        return String::new();
    }
    let last_col = term.last_column();
    let raw = term.bounds_to_string(Point::new(Line(from), Column(0)), Point::new(Line(to), last_col));
    let mut out: String = raw.lines().map(str::trim_end).collect::<Vec<_>>().join("\n");
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

fn set_winsize(fd: RawFd, s: GridSize) {
    let ws = libc::winsize {
        ws_row: s.rows,
        ws_col: s.cols,
        ws_xpixel: s.cols.saturating_mul(s.cell_w),
        ws_ypixel: s.rows.saturating_mul(s.cell_h),
    };
    unsafe {
        libc::ioctl(fd, libc::TIOCSWINSZ, &raw const ws);
    }
}

/// Write everything to the (non-blocking) PTY master, waiting for writability as needed.
fn write_all_fd(writer: &Mutex<File>, mut buf: &[u8]) -> io::Result<()> {
    let mut file = writer.lock().unwrap();
    while !buf.is_empty() {
        match file.write(buf) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => buf = &buf[n..],
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                let mut pfd = libc::pollfd { fd: file.as_raw_fd(), events: libc::POLLOUT, revents: 0 };
                let rc = unsafe { libc::poll(&raw mut pfd, 1, 1000) };
                if rc == 0 {
                    return Err(io::ErrorKind::TimedOut.into());
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

const READ_BUF: usize = 64 * 1024;

// Thread entry point: it owns everything it is handed for the life of the reader thread.
#[allow(clippy::needless_pass_by_value)]
fn reader_loop(
    mut master: File,
    term: SharedTerm,
    tx: async_channel::Sender<TerminalEvent>,
    dirty: Arc<AtomicBool>,
    marks: Arc<Mutex<Marks>>,
    last_output: Arc<AtomicU64>,
    grapheme_clusters: &AtomicBool,
) {
    use std::io::Read;
    let fd = master.as_raw_fd();
    let mut buf = vec![0u8; READ_BUF];
    let mut sniffer = OscSniffer::new();
    let mut processor: Processor<StdSyncHandler> = Processor::new();

    let wake = |dirty: &AtomicBool| {
        if !dirty.swap(true, Ordering::SeqCst) {
            let _ = tx.try_send(TerminalEvent::Wakeup);
        }
    };

    loop {
        // Block until readable, or until a synchronized update (DEC 2026) times out.
        let timeout_ms = match processor.sync_timeout().sync_timeout() {
            Some(deadline) => {
                deadline.saturating_duration_since(Instant::now()).as_millis().min(i32::MAX as u128) as i32
            }
            None => -1,
        };
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let rc = unsafe { libc::poll(&raw mut pfd, 1, timeout_ms) };
        if rc < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        if rc == 0 {
            // Sync timeout expired: flush what was buffered.
            processor.stop_sync(&mut GraphemeInput::new(
                &mut term.lock(),
                grapheme_clusters.load(Ordering::Relaxed),
            ));
            wake(&dirty);
            continue;
        }

        let mut got_any = false;
        let mut eof = false;
        loop {
            match master.read(&mut buf) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(n) => {
                    got_any = true;
                    let chunk = &buf[..n];
                    let hits = sniffer.feed(chunk);
                    let mut t = term.lock();
                    let clusters = grapheme_clusters.load(Ordering::Relaxed);
                    let mut pos = 0;
                    for hit in hits {
                        processor.advance(&mut GraphemeInput::new(&mut t, clusters), &chunk[pos..hit.end]);
                        pos = hit.end;
                        let kind = match hit.event {
                            OscEvent::PromptStart => Some(MarkKind::Prompt),
                            OscEvent::CommandStart => Some(MarkKind::CommandStart),
                            OscEvent::CommandEnd(_) => Some(MarkKind::CommandEnd),
                            _ => None,
                        };
                        if let Some(kind) = kind {
                            let abs =
                                t.grid().history_size() as i64 + i64::from(t.grid().cursor.point.line.0);
                            marks.lock().unwrap().push(Mark { kind, abs_line: abs });
                        }
                        let _ = tx.try_send(TerminalEvent::Osc(hit.event));
                    }
                    processor.advance(&mut GraphemeInput::new(&mut t, clusters), &chunk[pos..]);
                    drop(t);
                    if n < READ_BUF {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => {
                    // EIO: the slave side closed (the shell exited).
                    eof = true;
                    break;
                }
            }
        }
        if got_any {
            last_output.store(now_ms(), Ordering::Relaxed);
            wake(&dirty);
        }
        if eof {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(
        rx: &async_channel::Receiver<TerminalEvent>,
        mut pred: impl FnMut(&TerminalEvent) -> bool,
        secs: u64,
    ) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            match rx.try_recv() {
                Ok(ev) => {
                    if pred(&ev) {
                        return true;
                    }
                }
                Err(_) => thread::sleep(Duration::from_millis(10)),
            }
        }
        false
    }

    #[test]
    fn spawns_shell_and_reads_echo_back_from_grid() {
        let session = TerminalSession::spawn(&SpawnOptions {
            program: Some("/bin/sh".into()),
            args: vec![],
            ..SpawnOptions::default()
        })
        .unwrap();
        let rx = session.events();
        session.write(b"echo nox-$((40+2))\r");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut found = false;
        while Instant::now() < deadline {
            session.clear_dirty();
            if session.screen_text().contains("nox-42") {
                found = true;
                break;
            }
            let _ = wait_for(&rx, |e| matches!(e, TerminalEvent::Wakeup), 1);
        }
        assert!(found, "screen: {}", session.screen_text());

        session.write(b"exit 3\r");
        assert!(wait_for(&rx, |e| matches!(e, TerminalEvent::Exited(Some(3))), 10));
    }

    #[test]
    fn osc_events_and_last_output() {
        let session = TerminalSession::spawn(&SpawnOptions {
            program: Some("/bin/sh".into()),
            args: vec![],
            ..SpawnOptions::default()
        })
        .unwrap();
        let rx = session.events();
        session
            .write(b"printf '\\033]7;file://h/tmp\\007\\033]133;C\\007line1\\nline2\\n\\033]133;D;0\\007'\r");
        assert!(wait_for(&rx, |e| matches!(e, TerminalEvent::Osc(OscEvent::CommandEnd(Some(0)))), 10));
        let out = session.last_output().unwrap();
        assert!(out.contains("line1") && out.contains("line2"), "out: {out:?}");
    }

    #[test]
    fn resize_updates_grid() {
        let session = TerminalSession::spawn(&SpawnOptions {
            program: Some("/bin/sh".into()),
            args: vec![],
            ..SpawnOptions::default()
        })
        .unwrap();
        session.resize(GridSize { cols: 100, rows: 30, cell_w: 8, cell_h: 16 });
        let t = session.term().lock();
        assert_eq!(t.columns(), 100);
        assert_eq!(t.screen_lines(), 30);
    }
}
