//! `nox_terminal` — terminal sessions without any UI dependency (SPEC §13).
//!
//! PTY spawn / resize / kill, the reader thread, the OSC sniffer, the `Term` wrapper, the key
//! encoder, process inspection (foreground process, cwd) and shell integration snippets.

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

pub mod keys;
pub mod mouse;
pub mod osc;
pub mod process;
pub mod resize;
pub mod session;
pub mod shell;

pub use alacritty_terminal;
pub use osc::{OscEvent, OscSniffer};
pub use resize::GridSize;
pub use session::{ClipboardPolicy, Listener, SharedTerm, SpawnOptions, TerminalEvent, TerminalSession};
