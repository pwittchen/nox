//! nox_config: the `~/.nox/config.toml` model (SPEC §11), lenient loading, surgical writes,
//! live reload, and metadata for the Settings window (SPEC §5.9).

mod load;
mod model;
mod paths;
mod schema;
mod watch;
mod write;

pub use load::{Notice, ParseError, load_from_path, load_from_str};
pub use model::*;
pub use paths::{
    DEFAULT_CONFIG_TOML, config_path, ensure_default_file, icons_dir, nox_dir, run_dir, themes_dir,
};
pub use schema::{
    Section, SettingKind, SettingMeta, default_value, differs_from_default, find_setting, get_value,
    settings_schema,
};
pub use watch::{ChangeFilter, DEBOUNCE, WatchError, WatchHandle, watch};
pub use write::{
    ConfigWriter, WriteError, atomic_write, content_hash, is_self_write, record_self_write, remove_value,
    remove_value_at, set_value, set_value_at,
};

/// Re-exported so callers can build values for [`set_value`].
pub use toml_edit;

#[cfg(test)]
mod tests;
