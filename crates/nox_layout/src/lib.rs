//! nox_layout: workspaces → tabs → split trees.
//!
//! Pure data and geometry, no GPUI. The UI crate maps every [`PaneId`] to its own pane entity;
//! everything here deals only in ids, rectangles and plain values (SPEC §4.1–4.3, §4.7, §6).

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
