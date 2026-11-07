//! Asset source for GPUI: the SVG icons (compiled in) and user icon files (absolute paths,
//! read from disk), plus bundled-font discovery.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use anyhow::Result;
use gpui::{AssetSource, SharedString};

#[derive(rust_embed::RustEmbed)]
#[folder = "../../assets/icons"]
#[prefix = "icons/"]
struct Icons;

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.starts_with('/') {
            return Ok(std::fs::read(path).ok().map(Cow::Owned));
        }
        Ok(Icons::get(path).map(|f| f.data))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Icons::iter().filter(|p| p.starts_with(path)).map(|p| SharedString::from(p.to_string())).collect())
    }
}

/// The directory holding bundled resources (`assets/` in a checkout, `Contents/Resources/assets`
/// in the app bundle).
pub fn resources_dir() -> PathBuf {
    if let Some(res) = nox_mac::bundle_resources_dir() {
        let p = res.join("assets");
        if p.is_dir() {
            return p;
        }
    }
    // Development: next to the workspace root.
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    if dev.is_dir() {
        return dev;
    }
    // Next to the executable.
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join("assets")))
        .unwrap_or_else(|| PathBuf::from("assets"))
}

/// Bundled font families: (display name, directory slug, monospace).
pub const BUNDLED_TERMINAL_FONTS: &[(&str, &str)] = &[
    ("JetBrains Mono", "jetbrains-mono"),
    ("MesloLGS NF", "meslolgs-nf"),
    ("Geist Mono", "geist-mono"),
    ("IBM Plex Mono", "ibm-plex-mono"),
    ("Fira Code", "fira-code"),
    ("Cascadia Code", "cascadia-code"),
    ("Hack", "hack"),
    ("Source Code Pro", "source-code-pro"),
    ("CommitMono", "commit-mono"),
];

pub const BUNDLED_UI_FONTS: &[(&str, &str)] =
    &[("Geist", "geist"), ("Inter", "inter"), ("IBM Plex Sans", "ibm-plex-sans")];

/// System monospace families offered alongside the bundled ones.
pub const SYSTEM_TERMINAL_FONTS: &[&str] = &["SF Mono", "Menlo"];

/// Font files of a bundled family (empty if the family isn't bundled).
pub fn font_files(family: &str) -> Vec<PathBuf> {
    let slug = BUNDLED_TERMINAL_FONTS
        .iter()
        .chain(BUNDLED_UI_FONTS.iter())
        .find(|(name, _)| name.eq_ignore_ascii_case(family))
        .map(|(_, s)| *s);
    let Some(slug) = slug else { return vec![] };
    let dir = resources_dir().join("fonts").join(slug);
    let Ok(rd) = std::fs::read_dir(&dir) else { return vec![] };
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("ttf" | "otf")))
        .collect();
    files.sort();
    files
}
