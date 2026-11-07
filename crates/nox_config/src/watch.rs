//! Live reload (SPEC §11): watch `config.toml`, debounced 150 ms, ignoring nox's own writes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};

use crate::{Notice, ParseError, is_self_write, load_from_str};

/// Debounce interval of the config watcher.
pub const DEBOUNCE: Duration = Duration::from_millis(150);

/// Keeps the watcher alive; dropping it stops watching.
pub struct WatchHandle {
    _debouncer: Debouncer<RecommendedWatcher>,
}

impl std::fmt::Debug for WatchHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WatchHandle")
    }
}

/// Starting the watcher failed.
#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("config path {0} has no parent directory")]
    NoParent(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Notify(#[from] notify_debouncer_mini::notify::Error),
}

/// Decides whether a change of the file to `bytes` should be delivered; remembers the last
/// delivered contents so repeated events for identical contents are dropped.
#[derive(Debug, Default)]
pub struct ChangeFilter {
    last_delivered: Option<u64>,
}

impl ChangeFilter {
    pub fn new(initial: Option<&[u8]>) -> Self {
        Self { last_delivered: initial.map(crate::write::content_hash) }
    }

    /// `true` if the event is new and not caused by nox's own write.
    pub fn should_deliver(&mut self, path: &Path, bytes: &[u8]) -> bool {
        let hash = crate::write::content_hash(bytes);
        if self.last_delivered == Some(hash) {
            return false;
        }
        self.last_delivered = Some(hash);
        !is_self_write(path, bytes)
    }
}

/// Watches `path` (through its parent directory, since editors replace files) and calls
/// `on_change` with the reparsed config after each external change.
pub fn watch(
    path: PathBuf,
    on_change: impl Fn(Result<(crate::Config, Vec<Notice>), ParseError>) + Send + 'static,
) -> Result<WatchHandle, WatchError> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| WatchError::NoParent(path.clone()))?
        .to_path_buf();
    std::fs::create_dir_all(&dir)?;
    let file_name = path.file_name().map(|n| n.to_os_string());
    let initial = std::fs::read(&path).ok();
    let mut filter = ChangeFilter::new(initial.as_deref());
    let watched = path.clone();

    let mut debouncer = new_debouncer(DEBOUNCE, move |res: DebounceEventResult| {
        let events = match res {
            Ok(events) => events,
            Err(e) => {
                log::warn!("config watcher error: {e}");
                return;
            }
        };
        let relevant = events.iter().any(|e| e.path.file_name().map(|n| n.to_os_string()) == file_name);
        if !relevant {
            return;
        }
        let bytes = match std::fs::read(&watched) {
            Ok(b) => b,
            // Deleted or mid-replace: keep the last good config.
            Err(_) => return,
        };
        if !filter.should_deliver(&watched, &bytes) {
            return;
        }
        let result = match String::from_utf8(bytes) {
            Ok(s) => load_from_str(&s),
            Err(_) => Err(ParseError { line: None, message: "config file is not valid UTF-8".into() }),
        };
        on_change(result);
    })?;
    debouncer.watcher().watch(&dir, RecursiveMode::NonRecursive)?;
    Ok(WatchHandle { _debouncer: debouncer })
}
