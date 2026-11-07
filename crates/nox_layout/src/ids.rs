//! Ids of panes, tabs and workspaces.
//!
//! Each kind has a process-global atomic generator (`PaneId::next()` …). Ids loaded from a
//! session file can be reserved with `bump_above`, so that freshly generated ids never collide
//! with them.

use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($name:ident, $counter:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u64);

        static $counter: AtomicU64 = AtomicU64::new(1);

        impl $name {
            /// A new id, unique in this process (never 0).
            pub fn next() -> Self {
                Self($counter.fetch_add(1, Ordering::Relaxed))
            }

            /// Makes sure every id returned by [`Self::next`] from now on is greater than `id`.
            pub fn bump_above(id: Self) {
                $counter.fetch_max(id.0.saturating_add(1), Ordering::Relaxed);
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = std::num::ParseIntError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                s.parse().map(Self)
            }
        }
    };
}

id_type!(PaneId, NEXT_PANE_ID, "Id of a pane (one terminal session).");
id_type!(TabId, NEXT_TAB_ID, "Id of a tab.");
id_type!(WorkspaceId, NEXT_WORKSPACE_ID, "Id of a workspace.");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_is_unique_and_bump_works() {
        let a = PaneId::next();
        let b = PaneId::next();
        assert_ne!(a, b);
        PaneId::bump_above(PaneId(1_000_000));
        assert!(PaneId::next().0 > 1_000_000);
        // Bumping below the current value never goes back.
        PaneId::bump_above(PaneId(5));
        assert!(PaneId::next().0 > 1_000_000);
    }

    #[test]
    fn display_and_parse() {
        assert_eq!(TabId(42).to_string(), "42");
        assert_eq!("42".parse::<WorkspaceId>().unwrap(), WorkspaceId(42));
    }
}
