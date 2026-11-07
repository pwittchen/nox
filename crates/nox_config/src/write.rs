//! Surgical, atomic writes to `config.toml` (SPEC §11): only the affected key changes; comments,
//! ordering, blank lines and unknown keys are preserved. A file that fails to parse is never
//! written.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use toml_edit::{DocumentMut, Item, Table, TableLike, Value};

/// Why a write was refused or failed.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("config file does not parse ({0}); not writing")]
    Unparsable(crate::ParseError),
    #[error("invalid key \"{0}\"")]
    InvalidKey(String),
    #[error("\"{0}\" is not a table")]
    NotATable(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Hash of file contents, used to recognize file-watch events caused by our own writes.
#[must_use]
pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

fn self_writes() -> &'static Mutex<HashMap<PathBuf, u64>> {
    static MAP: OnceLock<Mutex<HashMap<PathBuf, u64>>> = OnceLock::new();
    MAP.get_or_init(Default::default)
}

fn guard_key(path: &Path) -> PathBuf {
    // The file may not exist yet; canonicalize its directory instead.
    let file = path.file_name().map(PathBuf::from).unwrap_or_default();
    path.parent()
        .and_then(|p| std::fs::canonicalize(if p.as_os_str().is_empty() { Path::new(".") } else { p }).ok())
        .map_or_else(|| path.to_path_buf(), |p| p.join(&file))
}

/// Records that nox itself wrote `bytes` to `path`.
pub fn record_self_write(path: &Path, bytes: &[u8]) {
    if let Ok(mut map) = self_writes().lock() {
        map.insert(guard_key(path), content_hash(bytes));
    }
}

/// Whether `bytes` (the file's current contents) is exactly what nox last wrote to `path`, so
/// a change event for it is self-caused and can be ignored.
#[must_use]
pub fn is_self_write(path: &Path, bytes: &[u8]) -> bool {
    self_writes().lock().is_ok_and(|map| map.get(&guard_key(path)) == Some(&content_hash(bytes)))
}

/// Writes `config.toml` for one path; a thin wrapper around [`set_value`] / [`remove_value`].
#[derive(Clone, Debug)]
pub struct ConfigWriter {
    path: PathBuf,
}

impl ConfigWriter {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn set(&self, key: &str, value: impl Into<Value>) -> Result<(), WriteError> {
        set_value(&self.path, key, value)
    }

    pub fn set_at(&self, segments: &[&str], value: impl Into<Value>) -> Result<(), WriteError> {
        set_value_at(&self.path, segments, value)
    }

    pub fn remove(&self, key: &str) -> Result<(), WriteError> {
        remove_value(&self.path, key)
    }

    pub fn remove_at(&self, segments: &[&str]) -> Result<(), WriteError> {
        remove_value_at(&self.path, segments)
    }
}

fn split_key(key: &str) -> Result<Vec<&str>, WriteError> {
    let segs: Vec<&str> = key.split('.').collect();
    if segs.iter().any(|s| s.is_empty()) {
        return Err(WriteError::InvalidKey(key.to_string()));
    }
    Ok(segs)
}

/// Sets a dotted key (`"tabs.position"`, `"agents.claude.command"`), creating missing tables.
pub fn set_value(path: &Path, key: &str, value: impl Into<Value>) -> Result<(), WriteError> {
    set_value_at(path, &split_key(key)?, value)
}

/// Like [`set_value`] with explicit path segments, for keys that contain dots
/// (`["icons", "my.tool"]`).
pub fn set_value_at(path: &Path, segments: &[&str], value: impl Into<Value>) -> Result<(), WriteError> {
    let value = value.into();
    edit(path, |doc| set_in_doc(doc, segments, value))
}

/// Removes a dotted key so it falls back to its default. Missing keys are not an error.
pub fn remove_value(path: &Path, key: &str) -> Result<(), WriteError> {
    remove_value_at(path, &split_key(key)?)
}

/// Like [`remove_value`] with explicit path segments.
pub fn remove_value_at(path: &Path, segments: &[&str]) -> Result<(), WriteError> {
    edit(path, |doc| {
        remove_in_doc(doc, segments);
        Ok(())
    })
}

fn edit(path: &Path, f: impl FnOnce(&mut DocumentMut) -> Result<(), WriteError>) -> Result<(), WriteError> {
    let original = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    // Refuse to touch a file the loader would reject.
    crate::load_from_str(&original).map_err(WriteError::Unparsable)?;
    let mut doc: DocumentMut = original.parse().map_err(|e: toml_edit::TomlError| {
        WriteError::Unparsable(crate::ParseError { line: None, message: e.message().to_string() })
    })?;
    f(&mut doc)?;
    let out = doc.to_string();
    if out == original {
        return Ok(());
    }
    atomic_write(path, out.as_bytes())?;
    Ok(())
}

/// Writes via a temp file in the same directory and a rename; records the self-write first so
/// the watcher can ignore it.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    std::fs::create_dir_all(&dir)?;
    let mut tmp = tempfile::Builder::new().prefix(".config.toml.").suffix(".tmp").tempfile_in(&dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(tmp.path(), meta.permissions());
    }
    record_self_write(path, bytes);
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn set_in_doc(doc: &mut DocumentMut, segments: &[&str], value: Value) -> Result<(), WriteError> {
    let Some((last, parents)) = segments.split_last() else {
        return Err(WriteError::InvalidKey(String::new()));
    };
    let mut cur: &mut dyn TableLike = doc.as_table_mut();
    for (i, seg) in parents.iter().enumerate() {
        if cur.get(seg).is_none() {
            let mut t = Table::new();
            t.set_implicit(true);
            cur.insert(seg, Item::Table(t));
        }
        cur = cur
            .get_mut(seg)
            .and_then(Item::as_table_like_mut)
            .ok_or_else(|| WriteError::NotATable(segments[..=i].join(".")))?;
    }
    match cur.get_mut(last) {
        Some(item) if item.is_value() => {
            let old = item.as_value_mut().expect("checked is_value");
            let decor = old.decor().clone();
            *old = value;
            *old.decor_mut() = decor;
        }
        _ => {
            cur.insert(last, Item::Value(value));
        }
    }
    Ok(())
}

fn remove_in_doc(doc: &mut DocumentMut, segments: &[&str]) {
    let Some((last, parents)) = segments.split_last() else {
        return;
    };
    let mut cur: &mut dyn TableLike = doc.as_table_mut();
    for seg in parents {
        match cur.get_mut(seg).and_then(Item::as_table_like_mut) {
            Some(t) => cur = t,
            None => return,
        }
    }
    cur.remove(last);
}
