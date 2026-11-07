//! `nox_layout`: workspaces → tabs → split trees.
//!
//! Pure data and geometry, no GPUI. The UI crate maps every [`PaneId`] to its own pane entity;
//! everything here deals only in ids, rectangles and plain values (SPEC §4.1–4.3, §4.7, §6).

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

mod geometry;
mod ids;
mod layout;
mod session;
mod tab;
mod workspace;

pub use geometry::{Direction, Point, Rect, Size, SplitDirection};
pub use ids::{PaneId, TabId, WorkspaceId};
pub use layout::{
    Branch, Divider, Layout, MIN_COLS, MIN_ROWS, RATIO_MAX, RATIO_MIN, SplitPath, clamp_ratio, min_pane_size,
};
pub use session::{
    AgentSnapshot, PaneSnapshot, Restored, RestoredPane, SESSION_VERSION, SessionError, SessionFile,
    TabSnapshot, WorkspaceSnapshot, load_session, save_session_atomic,
};
pub use tab::{ClosePaneResult, Tab};
pub use workspace::{
    MovedTab, TabStatus, Workspace, WorkspaceAfterClose, WorkspaceClose, Workspaces, aggregate_status,
};
