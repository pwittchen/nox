//! Git context (SPEC 7.12 "Git context") and worktrees for parallel agents.
//!
//! [`git_branch`] / [`repo_root`] only read files (`.git`, `HEAD`); no process is spawned, so
//! they are cheap enough to run on every cwd refresh. The worktree helpers run `git`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The directory containing `.git` (a work tree root, main or linked), walking up from `cwd`.
pub fn repo_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors().find(|d| d.join(".git").exists()).map(Path::to_path_buf)
}

/// The git directory for `cwd`: `<root>/.git`, or for a linked worktree / submodule the
/// directory a `.git` file points to (`gitdir: <path>`, relative to the file's directory).
#[must_use]
pub fn git_dir(cwd: &Path) -> Option<PathBuf> {
    let root = repo_root(cwd)?;
    let dot_git = root.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let content = std::fs::read_to_string(&dot_git).ok()?;
    let target = content.lines().find_map(|l| l.trim().strip_prefix("gitdir:"))?.trim();
    if target.is_empty() {
        return None;
    }
    let path = Path::new(target);
    Some(if path.is_absolute() { path.to_path_buf() } else { root.join(path) })
}

/// The branch from `HEAD` contents: `ref: refs/heads/x` → `x` (other refs: the part after
/// `refs/`), a detached sha → its first 7 characters.
#[must_use]
pub fn branch_from_head(head: &str) -> Option<String> {
    let head = head.trim();
    if let Some(r) = head.strip_prefix("ref:") {
        let r = r.trim();
        let name = r.strip_prefix("refs/heads/").or_else(|| r.strip_prefix("refs/")).unwrap_or(r);
        return (!name.is_empty()).then(|| name.to_string());
    }
    let is_sha = head.len() >= 7 && head.chars().all(|c| c.is_ascii_hexdigit());
    is_sha.then(|| head[..7].to_string())
}

/// The current branch of the repository containing `cwd` (worktree-aware), without spawning
/// git.
#[must_use]
pub fn git_branch(cwd: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir(cwd)?.join("HEAD")).ok()?;
    branch_from_head(&head)
}

/// A branch name usable in a directory name: `/` and other separators → `-`.
#[must_use]
pub fn sanitize_branch(branch: &str) -> String {
    let s: String = branch
        .trim()
        .chars()
        .map(|c| if c == '/' || c == '\\' || c == ':' || c.is_whitespace() { '-' } else { c })
        .collect();
    s.trim_matches(['-', '.']).to_string()
}

/// `<worktree_dir or root's parent>/<repo>-<sanitized branch>`.
#[must_use]
pub fn worktree_path(root: &Path, worktree_dir: Option<&Path>, branch: &str) -> Option<PathBuf> {
    let repo = root.file_name()?.to_string_lossy();
    let dir = match worktree_dir {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => root.parent()?.to_path_buf(),
    };
    Some(dir.join(format!("{repo}-{}", sanitize_branch(branch))))
}

fn run_git(dir: &Path, args: &[&std::ffi::OsStr]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(text) } else { Err(text.trim().to_string()) }
}

/// `git worktree add <path> -b <branch>` from `root` (see [`worktree_path`]). Returns the new
/// worktree's path, or git's output on failure (shown in a sheet).
pub fn worktree_add(root: &Path, worktree_dir: Option<&Path>, branch: &str) -> Result<PathBuf, String> {
    if sanitize_branch(branch).is_empty() {
        return Err(format!("invalid branch name {branch:?}"));
    }
    let path = worktree_path(root, worktree_dir, branch)
        .ok_or_else(|| format!("{} has no parent directory", root.display()))?;
    run_git(root, &["worktree".as_ref(), "add".as_ref(), path.as_os_str(), "-b".as_ref(), branch.as_ref()])?;
    Ok(path)
}

/// Is the worktree at `path` free of changes (incl. untracked files)? `false` on any error.
#[must_use]
pub fn worktree_is_clean(path: &Path) -> bool {
    run_git(path, &["status".as_ref(), "--porcelain".as_ref()]).is_ok_and(|out| out.trim().is_empty())
}

/// `git worktree remove <path>` (refuses dirty worktrees, never deletes the branch).
pub fn worktree_remove(path: &Path) -> Result<(), String> {
    // Run from the main work tree: the common git dir's parent.
    let common = run_git(
        path,
        &["rev-parse".as_ref(), "--path-format=absolute".as_ref(), "--git-common-dir".as_ref()],
    )?;
    let common = PathBuf::from(common.trim());
    let main = common.parent().map_or_else(|| path.to_path_buf(), Path::to_path_buf);
    run_git(&main, &["worktree".as_ref(), "remove".as_ref(), path.as_os_str()]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn head_parsing() {
        assert_eq!(branch_from_head("ref: refs/heads/main\n").as_deref(), Some("main"));
        assert_eq!(branch_from_head("ref: refs/heads/feat/x").as_deref(), Some("feat/x"));
        assert_eq!(branch_from_head("ref: refs/remotes/origin/main").as_deref(), Some("remotes/origin/main"));
        assert_eq!(
            branch_from_head("434f3d4a1b2c3d4e5f60718293a4b5c6d7e8f901\n").as_deref(),
            Some("434f3d4")
        );
        assert_eq!(branch_from_head("garbage"), None);
        assert_eq!(branch_from_head(""), None);
    }

    #[test]
    fn repo_and_worktree_fixtures() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("nox");
        fs::create_dir_all(repo.join(".git/worktrees/nox-feat-x")).unwrap();
        fs::create_dir_all(repo.join("src/deep")).unwrap();
        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(git_branch(&repo.join("src/deep")).as_deref(), Some("main"));
        assert_eq!(repo_root(&repo.join("src/deep")), Some(repo.clone()));

        // A linked worktree: `.git` is a file pointing into the main repo (absolute path).
        let wt = tmp.path().join("nox-feat-x");
        fs::create_dir_all(wt.join("crates")).unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", repo.join(".git/worktrees/nox-feat-x").display()))
            .unwrap();
        fs::write(repo.join(".git/worktrees/nox-feat-x/HEAD"), "ref: refs/heads/feat/x\n").unwrap();
        assert_eq!(git_branch(&wt.join("crates")).as_deref(), Some("feat/x"));
        assert_eq!(repo_root(&wt.join("crates")), Some(wt.clone()));

        // A relative gitdir (submodules) and a detached HEAD.
        let sub = repo.join("vendor/lib");
        fs::create_dir_all(&sub).unwrap();
        fs::create_dir_all(repo.join(".git/modules/lib")).unwrap();
        fs::write(sub.join(".git"), "gitdir: ../../.git/modules/lib\n").unwrap();
        fs::write(repo.join(".git/modules/lib/HEAD"), "0123456789abcdef0123456789abcdef01234567\n").unwrap();
        assert_eq!(git_branch(&sub).as_deref(), Some("0123456"));

        let none = tmp.path().join("plain");
        fs::create_dir_all(&none).unwrap();
        // tempdirs may live inside a repo on some machines; only assert when they don't.
        if repo_root(tmp.path()).is_none() {
            assert_eq!(git_branch(&none), None);
            assert_eq!(repo_root(&none), None);
        }
    }

    #[test]
    fn worktree_paths() {
        let root = Path::new("/dev/prv/nox");
        assert_eq!(
            worktree_path(root, None, "feat/parser").unwrap(),
            PathBuf::from("/dev/prv/nox-feat-parser")
        );
        assert_eq!(worktree_path(root, Some(Path::new("/wt")), "fix").unwrap(), PathBuf::from("/wt/nox-fix"));
        assert_eq!(
            worktree_path(root, Some(Path::new("")), "fix").unwrap(),
            PathBuf::from("/dev/prv/nox-fix")
        );
        assert_eq!(sanitize_branch("feat/a b"), "feat-a-b");
    }

    fn git(dir: &Path, args: &[&str]) -> bool {
        Command::new("git").arg("-C").arg(dir).args(args).output().is_ok_and(|o| o.status.success())
    }

    #[test]
    fn worktree_add_check_remove_with_real_git() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        if !git(&root, &["init", "-q", "-b", "main"]) {
            eprintln!("git not available; skipping");
            return;
        }
        fs::write(root.join("a.txt"), "a").unwrap();
        assert!(git(&root, &["add", "."]));
        assert!(git(
            &root,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "init"
            ]
        ));

        let wt = worktree_add(&root, None, "feat/x").unwrap();
        assert_eq!(wt, tmp.path().join("repo-feat-x"));
        assert_eq!(git_branch(&wt).as_deref(), Some("feat/x"));
        assert!(worktree_is_clean(&wt));
        // A second add of the same branch fails with git's message.
        let err = worktree_add(&root, Some(&tmp.path().join("other")), "feat/x").unwrap_err();
        assert!(!err.is_empty());

        fs::write(wt.join("dirty.txt"), "x").unwrap();
        assert!(!worktree_is_clean(&wt));
        assert!(worktree_remove(&wt).is_err(), "dirty worktrees are not removed");
        fs::remove_file(wt.join("dirty.txt")).unwrap();
        worktree_remove(&wt).unwrap();
        assert!(!wt.exists());
        assert!(!worktree_is_clean(&tmp.path().join("missing")));
    }
}
