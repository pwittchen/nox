//! Command-line definition of `nox` (SPEC §7.13) and its mapping to IPC methods.

use std::ffi::OsString;
use std::io::Read;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use nox_ipc::{AgentStateWire, Method, Placement, ReadMode, WaitState};

/// Extra time the client waits beyond a server-side timeout before giving up itself.
const TIMEOUT_MARGIN: Duration = Duration::from_secs(5);

#[derive(Debug, Parser)]
#[command(
    name = "nox",
    version,
    about = "Control nox and cooperate with other agents in it",
    after_help = "Exit codes: 0 ok, 1 error, 2 usage, 3 conflict, 4 timeout.\n`nox .` is short for `nox open .`."
)]
pub struct Cli {
    /// Print raw JSON results.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Workspaces, tabs and panes: id, name, program, agent state, cwd, branch.
    Ls {
        /// Include other workspaces (when `agents.scope = "all"`).
        #[arg(long)]
        all: bool,
    },
    /// Name the calling pane (e.g. `backend`, `reviewer`).
    Name { name: String },
    /// Report the calling agent's state and session id.
    Status {
        #[arg(value_enum)]
        state: StateArg,
        /// Short message shown with the state.
        #[arg(short = 'm', long = "message")]
        message: Option<String>,
        /// The agent's session id (used to resume it).
        #[arg(long, conflicts_with = "session_from_stdin")]
        session: Option<String>,
        /// Read the session id from hook JSON on stdin (`session_id`), e.g. Claude Code hooks.
        #[arg(long)]
        session_from_stdin: bool,
    },
    /// Text of another pane (the visible screen by default).
    Read {
        pane: String,
        /// The last N lines.
        #[arg(long, group = "mode")]
        lines: Option<u32>,
        /// The whole scrollback.
        #[arg(long, group = "mode")]
        scrollback: bool,
        /// The output of the last finished command.
        #[arg(long, group = "mode")]
        last_output: bool,
    },
    /// Type text into another pane as a bracketed paste (`-` reads the text from stdin).
    Send {
        pane: String,
        text: String,
        /// Also press Return.
        #[arg(long)]
        submit: bool,
    },
    /// Post a message to a pane (name / id) or to `all` (`-` reads the text from stdin).
    Msg { to: String, text: String },
    /// Read (and mark read) the calling pane's messages.
    Inbox {
        /// Block until a message arrives.
        #[arg(long)]
        wait: bool,
        /// Only messages from this pane.
        #[arg(long)]
        from: Option<String>,
        /// Do not mark messages read.
        #[arg(long)]
        peek: bool,
    },
    /// Block until a pane reaches a state.
    Wait {
        pane: String,
        #[arg(long, value_enum, default_value = "idle")]
        state: WaitArg,
        /// Give up after this many seconds (exit code 4).
        #[arg(long)]
        timeout: Option<f64>,
    },
    /// Take advisory claims on files or directories.
    Claim {
        #[arg(required = true)]
        paths: Vec<String>,
        /// Why (shown to other agents).
        #[arg(short = 'm', long = "message")]
        reason: Option<String>,
        /// Wait for conflicting claims to be released instead of failing.
        #[arg(long)]
        wait: bool,
    },
    /// Release own claims (all by default).
    Release { paths: Vec<String> },
    /// List claims in the workspace.
    Claims,
    /// Open a new pane running a command in the caller's workspace; prints its id.
    Spawn {
        /// In a new tab (default).
        #[arg(long, conflicts_with = "split")]
        tab: bool,
        /// In a split of the calling pane.
        #[arg(long, value_enum)]
        split: Option<SplitArg>,
        /// Name of the new pane.
        #[arg(long)]
        name: Option<String>,
        /// Working directory.
        #[arg(long)]
        cwd: Option<String>,
        /// Command to run (after `--`); empty = a shell.
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Attention sound + notification.
    Notify { title: String, body: Option<String> },
    /// Open or switch to the workspace for a folder.
    Open {
        path: Option<String>,
        /// Always create a new workspace.
        #[arg(long)]
        new: bool,
    },
    /// Run an MCP server over stdio.
    Mcp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StateArg {
    Working,
    NeedsInput,
    Idle,
}

impl From<StateArg> for AgentStateWire {
    fn from(s: StateArg) -> Self {
        match s {
            StateArg::Working => AgentStateWire::Working,
            StateArg::NeedsInput => AgentStateWire::NeedsInput,
            StateArg::Idle => AgentStateWire::Idle,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WaitArg {
    Idle,
    NeedsInput,
    Exited,
}

impl From<WaitArg> for WaitState {
    fn from(s: WaitArg) -> Self {
        match s {
            WaitArg::Idle => WaitState::Idle,
            WaitArg::NeedsInput => WaitState::NeedsInput,
            WaitArg::Exited => WaitState::Exited,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SplitArg {
    Right,
    Below,
}

/// Rewrites `nox . …` to `nox open . …` (global flags may come first).
pub fn expand_dot_alias(args: Vec<OsString>) -> Vec<OsString> {
    let mut out = args;
    if let Some(pos) = out.iter().skip(1).position(|a| !a.to_string_lossy().starts_with('-'))
        && out[pos + 1] == "."
    {
        out.insert(pos + 1, OsString::from("open"));
    }
    out
}

/// What the CLI needs from the environment to build a request (stdin, cwd), injectable for
/// tests.
pub trait Env {
    /// Whole stdin; `None` when stdin is a terminal.
    fn stdin(&mut self) -> Option<String>;
    fn canonicalize(&self, path: &str) -> std::io::Result<String>;
}

pub struct RealEnv;

impl Env for RealEnv {
    fn stdin(&mut self) -> Option<String> {
        // SAFETY: isatty only inspects the file descriptor.
        if unsafe { libc::isatty(0) } == 1 {
            return None;
        }
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).ok()?;
        Some(s)
    }

    fn canonicalize(&self, path: &str) -> std::io::Result<String> {
        let p = std::fs::canonicalize(path)?;
        if !p.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                format!("{} is not a directory", p.display()),
            ));
        }
        Ok(p.to_string_lossy().into_owned())
    }
}

/// A request ready to send: the method and how long the client waits for the answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub method: Method,
    pub timeout: Option<Duration>,
}

/// Map a command to its IPC request. `Mcp` has no request (`Ok(None)`).
pub fn to_request(command: &Command, env: &mut dyn Env) -> Result<Option<Prepared>, String> {
    let plain = |method| Ok(Some(Prepared { method, timeout: None }));
    match command {
        Command::Ls { all } => plain(Method::Ls { all: *all }),
        Command::Name { name } => plain(Method::Name { name: name.clone() }),
        Command::Status { state, message, session, session_from_stdin } => {
            let mut session = session.clone();
            if *session_from_stdin {
                session = env.stdin().as_deref().and_then(session_id_from_hook_json);
            }
            plain(Method::Status { state: (*state).into(), message: message.clone(), session })
        }
        Command::Read { pane, lines, scrollback, last_output } => {
            let mode = match (lines, scrollback, last_output) {
                (Some(n), _, _) => ReadMode::Lines(*n),
                (_, true, _) => ReadMode::Scrollback,
                (_, _, true) => ReadMode::LastOutput,
                _ => ReadMode::Screen,
            };
            plain(Method::Read { pane: pane.clone(), mode })
        }
        Command::Send { pane, text, submit } => {
            plain(Method::Send { pane: pane.clone(), text: text_or_stdin(text, env)?, submit: *submit })
        }
        Command::Msg { to, text } => plain(Method::Msg { to: to.clone(), text: text_or_stdin(text, env)? }),
        Command::Inbox { wait, from, peek } => {
            plain(Method::Inbox { wait: *wait, from: from.clone(), peek: *peek, timeout_s: None })
        }
        Command::Wait { pane, state, timeout } => {
            if let Some(t) = timeout
                && !(t.is_finite() && *t >= 0.0)
            {
                return Err(format!("invalid --timeout {t}"));
            }
            Ok(Some(Prepared {
                method: Method::Wait { pane: pane.clone(), state: (*state).into(), timeout_s: *timeout },
                timeout: timeout.map(|t| Duration::from_secs_f64(t) + TIMEOUT_MARGIN),
            }))
        }
        Command::Claim { paths, reason, wait } => plain(Method::Claim {
            paths: paths.clone(),
            reason: reason.clone(),
            wait: *wait,
            timeout_s: None,
        }),
        Command::Release { paths } => plain(Method::Release { paths: paths.clone() }),
        Command::Claims => plain(Method::Claims),
        Command::Spawn { tab: _, split, name, cwd, command } => {
            let placement = match split {
                Some(SplitArg::Right) => Placement::SplitRight,
                Some(SplitArg::Below) => Placement::SplitBelow,
                None => Placement::Tab,
            };
            let cwd = match cwd {
                Some(c) => Some(env.canonicalize(c).map_err(|e| format!("--cwd {c}: {e}"))?),
                None => None,
            };
            plain(Method::Spawn { placement, name: name.clone(), cwd, command: command.clone() })
        }
        Command::Notify { title, body } => plain(Method::Notify { title: title.clone(), body: body.clone() }),
        Command::Open { path, new } => {
            let p = path.as_deref().unwrap_or(".");
            let abs = env.canonicalize(p).map_err(|e| format!("{p}: {e}"))?;
            plain(Method::Open { path: abs, force_new: *new })
        }
        Command::Mcp => Ok(None),
    }
}

fn text_or_stdin(text: &str, env: &mut dyn Env) -> Result<String, String> {
    if text == "-" {
        env.stdin().ok_or_else(|| "`-` reads the text from stdin, but stdin is a terminal".into())
    } else {
        Ok(text.to_string())
    }
}

/// `session_id` from agent hook JSON (Claude Code passes it to every hook); anything else is
/// ignored.
pub fn session_id_from_hook_json(input: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(input.trim()).ok()?;
    v.get("session_id")
        .or_else(|| v.get("sessionId"))
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeEnv {
        stdin: Option<String>,
    }

    impl Env for FakeEnv {
        fn stdin(&mut self) -> Option<String> {
            self.stdin.take()
        }
        fn canonicalize(&self, path: &str) -> std::io::Result<String> {
            if path == "missing" {
                return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "nope"));
            }
            Ok(format!("/abs/{path}"))
        }
    }

    fn parse(args: &[&str]) -> Cli {
        let argv: Vec<OsString> =
            std::iter::once("nox").chain(args.iter().copied()).map(Into::into).collect();
        Cli::try_parse_from(expand_dot_alias(argv)).unwrap()
    }

    fn method(args: &[&str]) -> Method {
        method_with_stdin(args, None)
    }

    fn method_with_stdin(args: &[&str], stdin: Option<&str>) -> Method {
        let cli = parse(args);
        let mut env = FakeEnv { stdin: stdin.map(str::to_string) };
        to_request(&cli.command, &mut env).unwrap().unwrap().method
    }

    fn fails(args: &[&str]) -> clap::error::ErrorKind {
        let argv: Vec<OsString> =
            std::iter::once("nox").chain(args.iter().copied()).map(Into::into).collect();
        Cli::try_parse_from(expand_dot_alias(argv)).unwrap_err().kind()
    }

    #[test]
    fn ls() {
        assert_eq!(method(&["ls"]), Method::Ls { all: false });
        assert_eq!(method(&["ls", "--all"]), Method::Ls { all: true });
        assert!(parse(&["--json", "ls"]).json);
        assert!(parse(&["ls", "--json"]).json);
    }

    #[test]
    fn name() {
        assert_eq!(method(&["name", "backend"]), Method::Name { name: "backend".into() });
        fails(&["name"]);
    }

    #[test]
    fn status() {
        assert_eq!(
            method(&["status", "needs-input", "-m", "perm?", "--session", "s1"]),
            Method::Status {
                state: AgentStateWire::NeedsInput,
                message: Some("perm?".into()),
                session: Some("s1".into())
            }
        );
        assert_eq!(
            method_with_stdin(
                &["status", "working", "--session-from-stdin"],
                Some(r#"{"session_id":"abc","hook_event_name":"UserPromptSubmit"}"#)
            ),
            Method::Status { state: AgentStateWire::Working, message: None, session: Some("abc".into()) }
        );
        assert_eq!(
            method_with_stdin(&["status", "idle", "--session-from-stdin"], Some("not json")),
            Method::Status { state: AgentStateWire::Idle, message: None, session: None }
        );
        assert_eq!(fails(&["status", "busy"]), clap::error::ErrorKind::InvalidValue);
        assert_eq!(
            fails(&["status", "idle", "--session", "a", "--session-from-stdin"]),
            clap::error::ErrorKind::ArgumentConflict
        );
    }

    #[test]
    fn read() {
        let r = |mode| Method::Read { pane: "backend".into(), mode };
        assert_eq!(method(&["read", "backend"]), r(ReadMode::Screen));
        assert_eq!(method(&["read", "backend", "--lines", "20"]), r(ReadMode::Lines(20)));
        assert_eq!(method(&["read", "backend", "--scrollback"]), r(ReadMode::Scrollback));
        assert_eq!(method(&["read", "backend", "--last-output"]), r(ReadMode::LastOutput));
        assert_eq!(
            fails(&["read", "backend", "--scrollback", "--last-output"]),
            clap::error::ErrorKind::ArgumentConflict
        );
    }

    #[test]
    fn send_and_msg() {
        assert_eq!(
            method(&["send", "reviewer", "hello", "--submit"]),
            Method::Send { pane: "reviewer".into(), text: "hello".into(), submit: true }
        );
        assert_eq!(
            method_with_stdin(&["send", "reviewer", "-"], Some("from stdin\n")),
            Method::Send { pane: "reviewer".into(), text: "from stdin\n".into(), submit: false }
        );
        assert_eq!(
            method(&["msg", "all", "feat/x is ready"]),
            Method::Msg { to: "all".into(), text: "feat/x is ready".into() }
        );
        let cli = parse(&["send", "x", "-"]);
        assert!(to_request(&cli.command, &mut FakeEnv { stdin: None }).is_err());
    }

    #[test]
    fn inbox() {
        assert_eq!(
            method(&["inbox"]),
            Method::Inbox { wait: false, from: None, peek: false, timeout_s: None }
        );
        assert_eq!(
            method(&["inbox", "--wait", "--from", "backend", "--peek"]),
            Method::Inbox { wait: true, from: Some("backend".into()), peek: true, timeout_s: None }
        );
    }

    #[test]
    fn wait() {
        assert_eq!(
            method(&["wait", "backend"]),
            Method::Wait { pane: "backend".into(), state: WaitState::Idle, timeout_s: None }
        );
        let cli = parse(&["wait", "backend", "--state", "exited", "--timeout", "2.5"]);
        let p = to_request(&cli.command, &mut FakeEnv { stdin: None }).unwrap().unwrap();
        assert_eq!(
            p.method,
            Method::Wait { pane: "backend".into(), state: WaitState::Exited, timeout_s: Some(2.5) }
        );
        assert_eq!(p.timeout, Some(Duration::from_millis(7500)));
        assert_eq!(
            method(&["wait", "b", "--state", "needs-input"]),
            Method::Wait { pane: "b".into(), state: WaitState::NeedsInput, timeout_s: None }
        );
    }

    #[test]
    fn claims() {
        assert_eq!(
            method(&["claim", "src/a.rs", "src/b/", "-m", "refactor", "--wait"]),
            Method::Claim {
                paths: vec!["src/a.rs".into(), "src/b/".into()],
                reason: Some("refactor".into()),
                wait: true,
                timeout_s: None
            }
        );
        assert_eq!(fails(&["claim"]), clap::error::ErrorKind::MissingRequiredArgument);
        assert_eq!(method(&["release"]), Method::Release { paths: vec![] });
        assert_eq!(method(&["release", "a", "b"]), Method::Release { paths: vec!["a".into(), "b".into()] });
        assert_eq!(method(&["claims"]), Method::Claims);
    }

    #[test]
    fn spawn() {
        assert_eq!(
            method(&[
                "spawn", "--split", "right", "--name", "helper", "--cwd", "dir", "--", "claude", "-p", "hi"
            ]),
            Method::Spawn {
                placement: Placement::SplitRight,
                name: Some("helper".into()),
                cwd: Some("/abs/dir".into()),
                command: vec!["claude".into(), "-p".into(), "hi".into()]
            }
        );
        assert_eq!(
            method(&["spawn", "--tab", "--", "ls"]),
            Method::Spawn { placement: Placement::Tab, name: None, cwd: None, command: vec!["ls".into()] }
        );
        assert!(matches!(
            method(&["spawn", "--split", "below"]),
            Method::Spawn { placement: Placement::SplitBelow, .. }
        ));
        assert_eq!(fails(&["spawn", "--tab", "--split", "right"]), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn notify() {
        assert_eq!(
            method(&["notify", "Done", "all green"]),
            Method::Notify { title: "Done".into(), body: Some("all green".into()) }
        );
        assert_eq!(method(&["notify", "Done"]), Method::Notify { title: "Done".into(), body: None });
    }

    #[test]
    fn open_and_dot() {
        assert_eq!(method(&["open"]), Method::Open { path: "/abs/.".into(), force_new: false });
        assert_eq!(method(&["open", "x", "--new"]), Method::Open { path: "/abs/x".into(), force_new: true });
        assert_eq!(method(&["."]), Method::Open { path: "/abs/.".into(), force_new: false });
        assert_eq!(
            method(&["--json", ".", "--new"]),
            Method::Open { path: "/abs/.".into(), force_new: true }
        );
        let cli = parse(&["open", "missing"]);
        assert!(to_request(&cli.command, &mut FakeEnv { stdin: None }).is_err());
    }

    #[test]
    fn mcp() {
        let cli = parse(&["mcp"]);
        assert!(matches!(cli.command, Command::Mcp));
        assert_eq!(to_request(&cli.command, &mut FakeEnv { stdin: None }).unwrap(), None);
    }

    #[test]
    fn hook_json() {
        assert_eq!(session_id_from_hook_json(r#"{"session_id":"x"}"#), Some("x".into()));
        assert_eq!(session_id_from_hook_json(r#"{"other":1}"#), None);
        assert_eq!(session_id_from_hook_json(""), None);
    }
}
