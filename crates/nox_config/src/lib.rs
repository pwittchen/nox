//! `nox_config`: the `~/.nox/config.toml` model (SPEC §11), lenient loading, surgical writes,
//! live reload, and metadata for the Settings window (SPEC §5.9).

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
