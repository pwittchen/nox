//! nox_terminal — terminal sessions without any UI dependency (SPEC §13).
//!
//! PTY spawn / resize / kill, the reader thread, the OSC sniffer, the `Term` wrapper, the key
//! encoder, process inspection (foreground process, cwd) and shell integration snippets.

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
