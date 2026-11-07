//! Advisory path claims of a workspace (SPEC 7.13 "Claims").
//!
//! Paths are normalized lexically (no filesystem access): relative paths are joined to the
//! workspace root when there is one, `.` / `..` are resolved, trailing slashes dropped. Two
//! claims overlap when one path is equal to or under the other (component-wise, so `src/a`
//! doesn't overlap `src/ab`). Claiming is all-or-nothing; claiming a path one already holds
//! renews it (and updates the reason).

use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// Default `agents.claim_ttl`.
pub const DEFAULT_CLAIM_TTL: Duration = Duration::from_mins(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// Normalized path (absolute when a root or an absolute path was given).
    pub path: PathBuf,
    pub holder: u64,
    pub holder_name: String,
    pub reason: Option<String>,
    /// Claimed or last renewed.
    pub renewed: Instant,
}

/// Another pane holds an overlapping claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimConflict {
    /// The held (conflicting) claim's path.
    pub path: PathBuf,
    /// The requested path that overlaps it.
    pub requested: PathBuf,
    pub holder: u64,
    pub holder_name: String,
    pub reason: Option<String>,
}

impl fmt::Display for ClaimConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} is claimed by {}", self.path.display(), self.holder_name)?;
        if let Some(r) = &self.reason {
            write!(f, ": {r}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ClaimConflict {}

/// Lexical normalization of a claim path (see the module docs).
#[must_use]
pub fn normalize_claim_path(path: &Path, root: Option<&Path>) -> PathBuf {
    let joined = match root {
        Some(root) if path.is_relative() => root.join(path),
        _ => path.to_path_buf(),
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

#[derive(Debug, Clone, Default)]
pub struct Claims {
    claims: Vec<Claim>,
}

impl Claims {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Claims `paths` for `holder`, all or nothing.
    pub fn claim(
        &mut self,
        holder: u64,
        holder_name: &str,
        paths: &[PathBuf],
        reason: Option<&str>,
        root: Option<&Path>,
        now: Instant,
    ) -> Result<(), ClaimConflict> {
        let wanted: Vec<PathBuf> = paths.iter().map(|p| normalize_claim_path(p, root)).collect();
        for req in &wanted {
            if let Some(c) = self.claims.iter().find(|c| c.holder != holder && overlaps(&c.path, req)) {
                return Err(ClaimConflict {
                    path: c.path.clone(),
                    requested: req.clone(),
                    holder: c.holder,
                    holder_name: c.holder_name.clone(),
                    reason: c.reason.clone(),
                });
            }
        }
        for path in wanted {
            match self.claims.iter_mut().find(|c| c.holder == holder && c.path == path) {
                Some(c) => {
                    c.renewed = now;
                    c.holder_name = holder_name.to_string();
                    if let Some(r) = reason {
                        c.reason = Some(r.to_string());
                    }
                }
                None => self.claims.push(Claim {
                    path,
                    holder,
                    holder_name: holder_name.to_string(),
                    reason: reason.map(str::to_string),
                    renewed: now,
                }),
            }
        }
        Ok(())
    }

    /// Who would block claiming `path` for `holder`, without claiming.
    #[must_use]
    pub fn conflict(&self, holder: u64, path: &Path, root: Option<&Path>) -> Option<&Claim> {
        let p = normalize_claim_path(path, root);
        self.claims.iter().find(|c| c.holder != holder && overlaps(&c.path, &p))
    }

    /// Releases `holder`'s claims on `paths` (a directory also releases the holder's claims
    /// under it), or all of them with `None`. Returns the released claims.
    pub fn release(&mut self, holder: u64, paths: Option<&[PathBuf]>, root: Option<&Path>) -> Vec<Claim> {
        let Some(paths) = paths else {
            return self.release_all(holder);
        };
        let targets: Vec<PathBuf> = paths.iter().map(|p| normalize_claim_path(p, root)).collect();
        self.remove_where(|c| c.holder == holder && targets.iter().any(|t| c.path.starts_with(t)))
    }

    /// Releases everything `holder` holds (pane closed, program exited, tab moved).
    pub fn release_all(&mut self, holder: u64) -> Vec<Claim> {
        self.remove_where(|c| c.holder == holder)
    }

    /// Drops claims not renewed for `ttl`. Returns them.
    pub fn expire(&mut self, now: Instant, ttl: Duration) -> Vec<Claim> {
        self.remove_where(|c| now.saturating_duration_since(c.renewed) >= ttl)
    }

    /// When the next claim expires, to schedule [`Claims::expire`].
    #[must_use]
    pub fn next_expiry(&self, ttl: Duration) -> Option<Instant> {
        self.claims.iter().map(|c| c.renewed + ttl).min()
    }

    fn remove_where(&mut self, mut f: impl FnMut(&Claim) -> bool) -> Vec<Claim> {
        let (gone, keep) = std::mem::take(&mut self.claims).into_iter().partition(|c| f(c));
        self.claims = keep;
        gone
    }

    /// All claims, in claim order.
    #[must_use]
    pub fn list(&self) -> &[Claim] {
        &self.claims
    }

    #[must_use]
    pub fn held_by(&self, holder: u64) -> Vec<&Claim> {
        self.claims.iter().filter(|c| c.holder == holder).collect()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.claims.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pb(v: &[&str]) -> Vec<PathBuf> {
        v.iter().map(PathBuf::from).collect()
    }

    const ROOT: &str = "/work/nox";

    #[test]
    fn normalization() {
        let root = Some(Path::new(ROOT));
        assert_eq!(
            normalize_claim_path(Path::new("src/parser/"), root),
            PathBuf::from("/work/nox/src/parser")
        );
        assert_eq!(
            normalize_claim_path(Path::new("./src/../lib.rs"), root),
            PathBuf::from("/work/nox/lib.rs")
        );
        assert_eq!(normalize_claim_path(Path::new("/etc/hosts"), root), PathBuf::from("/etc/hosts"));
        assert_eq!(normalize_claim_path(Path::new("a/./b"), None), PathBuf::from("a/b"));
    }

    #[test]
    fn overlap_conflicts_both_ways() {
        let t = Instant::now();
        let root = Some(Path::new(ROOT));
        let mut c = Claims::new();
        c.claim(1, "backend", &pb(&["src/parser/lexer.rs"]), Some("refactor"), root, t).unwrap();
        let err = c.claim(2, "docs", &pb(&["src/parser/"]), None, root, t).unwrap_err();
        assert_eq!(err.holder_name, "backend");
        assert_eq!(err.reason.as_deref(), Some("refactor"));
        assert_eq!(err.path, PathBuf::from("/work/nox/src/parser/lexer.rs"));
        assert_eq!(err.to_string(), "/work/nox/src/parser/lexer.rs is claimed by backend: refactor");
        // A directory claim blocks files under it.
        c.claim(2, "docs", &pb(&["docs"]), None, root, t).unwrap();
        assert!(c.claim(1, "backend", &pb(&["/work/nox/docs/a.md"]), None, root, t).is_err());
        // Siblings with a common prefix don't overlap.
        c.claim(2, "docs", &pb(&["src/parser2"]), None, root, t).unwrap();
        // The same holder may claim overlapping paths.
        c.claim(1, "backend", &pb(&["src/parser/lexer.rs", "src/parser/lexer.rs"]), None, root, t).unwrap();
        assert_eq!(c.held_by(1).len(), 1);
        assert!(c.conflict(3, Path::new("src"), root).is_some());
        assert!(c.conflict(3, Path::new("README.md"), root).is_none());
    }

    #[test]
    fn all_or_nothing() {
        let t = Instant::now();
        let mut c = Claims::new();
        c.claim(1, "a", &pb(&["/r/x"]), None, None, t).unwrap();
        assert!(c.claim(2, "b", &pb(&["/r/y", "/r/x/z"]), None, None, t).is_err());
        assert!(c.held_by(2).is_empty());
    }

    #[test]
    fn renew_and_expire() {
        let t0 = Instant::now();
        let ttl = Duration::from_mins(30);
        let mut c = Claims::new();
        c.claim(1, "a", &pb(&["/r/x"]), Some("one"), None, t0).unwrap();
        c.claim(2, "b", &pb(&["/r/y"]), None, None, t0).unwrap();
        let t1 = t0 + Duration::from_secs(1000);
        c.claim(1, "a", &pb(&["/r/x"]), None, None, t1).unwrap();
        assert_eq!(c.list()[0].reason.as_deref(), Some("one"), "renewing keeps the reason");
        assert_eq!(c.next_expiry(ttl), Some(t0 + ttl));
        let gone = c.expire(t0 + ttl, ttl);
        assert_eq!(gone.iter().map(|g| g.holder).collect::<Vec<_>>(), [2]);
        assert_eq!(c.list().len(), 1);
        assert!(c.expire(t1 + ttl.checked_sub(Duration::from_secs(1)).unwrap(), ttl).is_empty());
        assert_eq!(c.expire(t1 + ttl, ttl).len(), 1);
        assert!(c.is_empty());
    }

    #[test]
    fn release_paths_and_on_exit() {
        let t = Instant::now();
        let root = Some(Path::new(ROOT));
        let mut c = Claims::new();
        c.claim(1, "a", &pb(&["src/a.rs", "src/b.rs", "docs"]), None, root, t).unwrap();
        c.claim(2, "b", &pb(&["tests"]), None, root, t).unwrap();
        let gone = c.release(1, Some(&pb(&["src/a.rs"])), root);
        assert_eq!(gone.len(), 1);
        // Releasing a directory releases what is under it; other holders are untouched.
        assert_eq!(c.release(1, Some(&pb(&["src"])), root).len(), 1);
        assert!(c.release(1, Some(&pb(&["tests"])), root).is_empty());
        // The program exits: everything it holds goes.
        assert_eq!(c.release(1, None, root).len(), 1);
        assert_eq!(c.release_all(2).len(), 1);
        assert!(c.is_empty());
    }
}
