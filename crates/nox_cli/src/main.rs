//! `nox` — the command-line tool of nox (SPEC §7.13).
//!
//! Exit codes: `0` ok, `1` error, `2` usage, `3` conflict, `4` timeout.

// `cargo lint` runs clippy::pedantic. These lints are allowed on purpose: numeric casts are
// bounded grid / pixel / color math, `# Errors` / `# Panics` sections would only restate the
// signature, hex color literals read better without separators, and long UI builders and
// flag-heavy config structs are clearer kept whole; `assert!(x.is_empty())` reads fine in tests
// and `(n..).find(..)` searches are meant to be unbounded.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::unreadable_literal,
    clippy::too_many_lines,
    clippy::struct_excessive_bools,
    clippy::similar_names,
    clippy::many_single_char_names,
    clippy::assert_is_empty,
    clippy::maybe_infinite_iter
)]

mod cli;
mod mcp;
mod output;

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;
use nox_ipc::{Client, ENV_SOCKET, ErrorCode, IpcError, Method};

use crate::cli::{Cli, Command, Prepared, RealEnv};

const EXIT_ERROR: u8 = 1;
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    let args = cli::expand_dot_alias(std::env::args_os().collect());
    let cli = match Cli::try_parse_from(args) {
        Ok(c) => c,
        Err(e) => {
            let _ = e.print();
            // clap: help / version → 0, everything else is a usage error.
            return ExitCode::from(if e.use_stderr() { EXIT_USAGE } else { 0 });
        }
    };
    ExitCode::from(run(&cli))
}

fn run(cli: &Cli) -> u8 {
    if let Command::Mcp = cli.command {
        let mut backend = mcp::LazyClient::default();
        return match mcp::serve(&mut backend) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("nox mcp: {e}");
                EXIT_ERROR
            }
        };
    }
    let Prepared { method, timeout } = match cli::to_request(&cli.command, &mut RealEnv) {
        Ok(Some(p)) => p,
        Ok(None) => return 0,
        Err(e) => {
            eprintln!("nox: {e}");
            return EXIT_USAGE;
        }
    };

    let result = match Client::connect_from_env() {
        Ok(mut client) => client.call(method.clone(), timeout),
        Err(e) => {
            if let Method::Notify { title, body } = &method {
                return notify_fallback(title, body.as_deref());
            }
            eprintln!("nox: {}", connect_error_message(&e));
            return EXIT_ERROR;
        }
    };

    match result {
        Ok(value) => {
            let text = if cli.json {
                serde_json::to_string_pretty(&value).unwrap_or_default() + "\n"
            } else {
                output::render(&method, &value)
            };
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(text.as_bytes());
            let _ = out.flush();
            0
        }
        Err(err) => {
            // Outside nox (no token) a notification still works as OSC 777 to the terminal.
            if let (Method::Notify { title, body }, ErrorCode::Unauthorized) = (&method, err.code) {
                return notify_fallback(title, body.as_deref());
            }
            if cli.json {
                let v = serde_json::json!({ "error": err });
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            }
            eprint!("{}", output::render_error(&err));
            exit_code(&err)
        }
    }
}

/// Exit code for an IPC error.
#[must_use]
pub fn exit_code(err: &IpcError) -> u8 {
    err.code.exit_code() as u8
}

/// A clear message for a failed connection.
#[must_use]
pub fn connect_error_message(e: &std::io::Error) -> String {
    let path = Client::socket_path_from_env();
    let inside = std::env::var_os(ENV_SOCKET).is_some_and(|s| !s.is_empty());
    match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused if inside => format!(
            "cannot reach nox at {} — nox is not running, or agent cooperation is disabled \
             (agents.cooperation = false)",
            path.display()
        ),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => format!(
            "nox is not running (no control socket at {}; agent cooperation may be disabled)",
            path.display()
        ),
        _ => format!("cannot connect to nox at {}: {e}", path.display()),
    }
}

/// OSC 777 sequence for a notification (SPEC §7.11).
#[must_use]
pub fn osc777(title: &str, body: Option<&str>) -> String {
    let clean = |s: &str| s.replace(['\x07', '\x1b', ';'], " ");
    format!("\x1b]777;notify;{};{}\x07", clean(title), clean(body.unwrap_or("")))
}

fn notify_fallback(title: &str, body: Option<&str>) -> u8 {
    let seq = osc777(title, body);
    // Prefer the controlling terminal so it works with redirected stdout.
    let written = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/tty")
        .and_then(|mut tty| tty.write_all(seq.as_bytes()));
    if written.is_err() {
        let mut out = std::io::stdout().lock();
        if out.write_all(seq.as_bytes()).and_then(|()| out.flush()).is_err() {
            return EXIT_ERROR;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_mapping() {
        let e = |c| exit_code(&IpcError::new(c, "x"));
        assert_eq!(e(ErrorCode::Usage), 2);
        assert_eq!(e(ErrorCode::Conflict), 3);
        assert_eq!(e(ErrorCode::Timeout), 4);
        for c in [
            ErrorCode::NotFound,
            ErrorCode::Unauthorized,
            ErrorCode::Forbidden,
            ErrorCode::RateLimited,
            ErrorCode::Internal,
        ] {
            assert_eq!(e(c), 1);
        }
    }

    #[test]
    fn osc777_sequence() {
        assert_eq!(osc777("Done", Some("all green")), "\x1b]777;notify;Done;all green\x07");
        assert_eq!(osc777("a;b", None), "\x1b]777;notify;a b;\x07");
    }
}
