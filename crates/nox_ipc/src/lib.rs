//! `nox_ipc` — the control socket of nox (SPEC §7.13): protocol types, the server the app runs
//! and the client used by the `nox` command-line tool and `nox mcp`.

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

mod client;
pub mod protocol;
mod server;

use std::path::PathBuf;

pub use client::Client;
pub use protocol::*;
pub use server::{Caller, Handler, MAX_LINE_BYTES, Responder, Server, ServerHandle, Tokens};

/// Path of the control socket.
pub const ENV_SOCKET: &str = "NOX_SOCKET";
/// Random per-pane token authenticating requests.
pub const ENV_TOKEN: &str = "NOX_TOKEN";
pub const ENV_PANE_ID: &str = "NOX_PANE_ID";
pub const ENV_TAB_ID: &str = "NOX_TAB_ID";
pub const ENV_WORKSPACE_ID: &str = "NOX_WORKSPACE_ID";
/// Workspace name.
pub const ENV_WORKSPACE: &str = "NOX_WORKSPACE";
/// Workspace root directory (only set when the workspace has one).
pub const ENV_WORKSPACE_ROOT: &str = "NOX_WORKSPACE_ROOT";
/// Overrides `~/.nox`.
pub const ENV_NOX_HOME: &str = "NOX_HOME";

/// `$NOX_HOME` or `~/.nox`.
pub fn nox_home() -> PathBuf {
    if let Some(h) = std::env::var_os(ENV_NOX_HOME).filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    let home =
        std::env::var_os("HOME").filter(|h| !h.is_empty()).map_or_else(|| PathBuf::from("/"), PathBuf::from);
    home.join(".nox")
}

/// `$NOX_HOME/run/nox.sock` (default `~/.nox/run/nox.sock`).
///
/// Unix socket paths are limited to 104 bytes on macOS (`sun_path`); when `<home>/run/nox.sock`
/// is longer, a per-user directory under `/tmp` is used instead (`/tmp/nox-<uid>/nox.sock`).
/// The app and the CLI compute the same path, and panes get it via `NOX_SOCKET` anyway.
#[must_use]
pub fn default_socket_path() -> PathBuf {
    let p = nox_home().join("run").join("nox.sock");
    if p.as_os_str().len() < 100 {
        return p;
    }
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/nox-{uid}/nox.sock"))
}
