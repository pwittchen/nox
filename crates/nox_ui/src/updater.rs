//! Built-in updater (SPEC §15.3): checks GitHub Releases, downloads the release zip, verifies its
//! SHA-256 and code signature, and swaps the app bundle after nox quits, then relaunches it.
//!
//! Network and file work runs on the background executor through `curl`, `ditto`, `codesign`
//! and `spctl` (all part of macOS), so no TLS / HTTP stack is linked into the app.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use gpui::{AnyElement, App, Global, IntoElement, ParentElement, Styled, WindowHandle, div, prelude::*, px};

use crate::root::Root;
use crate::settings_state::Settings;
use crate::theme::Theme;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const ZIP_ASSET: &str = "nox-macos-aarch64.zip";
const SUMS_ASSET: &str = "SHA256SUMS";
/// First automatic check after launch (never before the first prompt is drawn), then daily.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);
const CHECK_INTERVAL: Duration = Duration::from_hours(24);

/// A release that has the app zip attached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// The GitHub Release page (release notes).
    pub page: String,
    pub zip_url: String,
    /// `sha256` of the zip from the asset's `digest`, when GitHub reports it.
    pub sha256: Option<String>,
    pub sums_url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release),
    /// Downloaded and verified; installed when nox quits.
    Ready(Release),
    Failed(String),
}

/// Who asked for a check: decides how the result is reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Menu bar / palette: a sheet in the main window, or a notice when up to date.
    Menu,
    /// A button in About / Settings: the result shows inline next to it.
    Inline,
    /// The daily check: silent unless an update is found.
    Automatic,
}

struct Staged {
    app: PathBuf,
    staging_dir: PathBuf,
    target: PathBuf,
    relaunch: bool,
}

#[derive(Default)]
pub struct Updater {
    status: Status,
    staged: Option<Staged>,
    /// The version the automatic check already offered this session (don't ask again daily).
    offered: Option<String>,
}
impl Global for Updater {}

/// The main window, so a check started from About / Settings or a timer can show its sheet there
/// and quit through `Root`.
pub struct MainWindow(pub WindowHandle<Root>);
impl Global for MainWindow {}

#[must_use]
pub fn status(cx: &App) -> Status {
    cx.try_global::<Updater>().map(|u| u.status.clone()).unwrap_or_default()
}

fn set_status(cx: &mut App, s: Status) {
    cx.default_global::<Updater>().status = s;
    cx.refresh_windows();
}

/// Start the daily automatic check (`app.check_for_updates`).
pub fn start_automatic(cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(FIRST_CHECK_DELAY).await;
        loop {
            let _ = cx.update(|cx| {
                if Settings::global(cx).config.app.check_for_updates {
                    check(cx, Origin::Automatic);
                }
            });
            cx.background_executor().timer(CHECK_INTERVAL).await;
        }
    })
    .detach();
}

/// Look for a newer release.
pub fn check(cx: &mut App, origin: Origin) {
    match status(cx) {
        Status::Checking | Status::Downloading(_) => {
            if origin == Origin::Menu {
                crate::toast::show(cx, "Already checking for updates…");
            }
            return;
        }
        Status::Ready(r) => {
            if origin == Origin::Menu {
                offer_relaunch(cx, &r);
            }
            return;
        }
        _ => {}
    }
    set_status(cx, Status::Checking);
    let channel = Settings::global(cx).config.app.update_channel.clone();
    let task = cx.background_spawn(async move { fetch_release(&channel) });
    cx.spawn(async move |cx| {
        let res = task.await;
        let _ = cx.update(|cx| on_checked(cx, origin, res));
    })
    .detach();
}

fn on_checked(cx: &mut App, origin: Origin, res: Result<Option<Release>, String>) {
    match res {
        Ok(Some(r)) if is_newer(CURRENT_VERSION, &r.version) => {
            set_status(cx, Status::Available(r.clone()));
            match origin {
                Origin::Menu => offer_install(cx, &r),
                Origin::Inline => {}
                Origin::Automatic => {
                    if Settings::global(cx).config.app.auto_install_updates {
                        install(cx, false);
                    } else if cx.default_global::<Updater>().offered.as_ref() != Some(&r.version) {
                        cx.default_global::<Updater>().offered = Some(r.version.clone());
                        offer_install(cx, &r);
                    }
                }
            }
        }
        Ok(_) => {
            set_status(cx, Status::UpToDate);
            if origin == Origin::Menu {
                crate::toast::show(cx, format!("nox is up to date ({CURRENT_VERSION})."));
            }
        }
        Err(e) => {
            log::warn!("update check: {e}");
            if origin == Origin::Automatic {
                set_status(cx, Status::Idle);
            } else {
                set_status(cx, Status::Failed(e.clone()));
                if origin == Origin::Menu {
                    crate::toast::show(cx, format!("Couldn't check for updates: {e}"));
                }
            }
        }
    }
}

/// Run `f` in the main window (where sheets show), bringing it to the front.
fn in_main_window(cx: &mut App, f: impl FnOnce(&mut Root, &mut gpui::Window, &mut gpui::Context<Root>)) {
    let Some(h) = cx.try_global::<MainWindow>().map(|m| m.0) else { return };
    let _ = h.update(cx, |r, window, cx| {
        window.activate_window();
        f(r, window, cx);
    });
}

fn offer_install(cx: &mut App, r: &Release) {
    let (version, page) = (r.version.clone(), r.page.clone());
    in_main_window(cx, move |_, window, cx| {
        crate::dialog::DialogBuilder::new(format!("nox {version} is available"))
            .message(format!(
                "You have {CURRENT_VERSION}. nox quits, installs the update and opens again with your \
                 session."
            ))
            .cancel("Later")
            .button("Release Notes", move |_, _, cx| cx.open_url(&page))
            .button("Install and Relaunch", |_, _, cx| install(cx, true))
            .default_last()
            .show(window, cx);
    });
}

fn offer_relaunch(cx: &mut App, r: &Release) {
    let version = r.version.clone();
    in_main_window(cx, move |_, window, cx| {
        crate::dialog::confirm(
            window,
            cx,
            format!("nox {version} is ready"),
            "It's installed when nox quits. Relaunch now?",
            "Relaunch",
            |_, cx| relaunch(cx),
        );
    });
}

/// Download, verify and stage the available update; with `relaunch`, quit and reopen right after.
pub fn install(cx: &mut App, relaunch_after: bool) {
    let Status::Available(release) = status(cx) else { return };
    let target = match installable_bundle() {
        Ok(t) => t,
        Err(why) => {
            cx.open_url(&release.page);
            crate::toast::show(cx, format!("{why} Opened the release page instead."));
            return;
        }
    };
    set_status(cx, Status::Downloading(release.clone()));
    let r = release.clone();
    let t = target.clone();
    let task = cx.background_spawn(async move { download_and_verify(&r, &t) });
    cx.spawn(async move |cx| {
        let res = task.await;
        let _ = cx.update(|cx| match res {
            Ok((app, staging_dir)) => {
                cx.default_global::<Updater>().staged =
                    Some(Staged { app, staging_dir, target, relaunch: relaunch_after });
                set_status(cx, Status::Ready(release.clone()));
                if relaunch_after {
                    relaunch(cx);
                } else {
                    crate::toast::show(
                        cx,
                        format!("nox {} is downloaded and will be installed when you quit.", release.version),
                    );
                }
            }
            Err(e) => {
                log::warn!("update download: {e}");
                set_status(cx, Status::Failed(e.clone()));
                crate::toast::show(cx, format!("Update failed: {e}"));
            }
        });
    })
    .detach();
}

/// Quit (asking first only if something is running) and reopen the updated app.
pub fn relaunch(cx: &mut App) {
    if let Some(s) = cx.default_global::<Updater>().staged.as_mut() {
        s.relaunch = true;
    }
    in_main_window(cx, crate::root_actions::request_quit_for_update);
}

/// Called when nox quits: hands a staged update to a detached helper that waits for this
/// process to exit, swaps the bundle and (if asked) opens it again.
pub fn on_quit(cx: &mut App) {
    const SCRIPT: &str = r#"
pid="$1"; new="$2"; target="$3"; relaunch="$4"; staging="$5"
i=0; while kill -0 "$pid" 2>/dev/null && [ "$i" -lt 600 ]; do sleep 0.1; i=$((i+1)); done
old="$target.nox-old"
rm -rf "$old"
if mv "$target" "$old"; then
  if ditto "$new" "$target"; then rm -rf "$old"; else rm -rf "$target"; mv "$old" "$target"; fi
fi
rm -rf "$staging"
if [ "$relaunch" = 1 ]; then open "$target"; fi
"#;
    let Some(s) = cx.try_global::<Updater>().and_then(|u| u.staged.as_ref()) else { return };
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(SCRIPT)
        .arg("nox-update")
        .arg(std::process::id().to_string())
        .arg(&s.app)
        .arg(&s.target)
        .arg(if s.relaunch { "1" } else { "0" })
        .arg(&s.staging_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    if let Err(e) = cmd.spawn() {
        log::error!("starting the update helper: {e}");
    }
}

// ---- UI --------------------------------------------------------------------------------------

/// Update status and its action, for the About window and Settings (About page, General).
#[must_use]
pub fn status_row(theme: &Theme, cx: &App) -> AnyElement {
    let muted = |t: String| div().text_color(theme.text_muted).child(t);
    let row = div().flex().items_center().justify_center().gap_2().text_size(px(12.));
    let check_button = |label: &'static str| {
        crate::settings::button("update-check", label, theme).on_click(|_, _, cx| check(cx, Origin::Inline))
    };
    match status(cx) {
        Status::Idle => row.child(check_button("Check for Updates")),
        Status::Checking => row.child(muted("Checking for updates…".into())),
        Status::UpToDate => row.child(muted("nox is up to date.".into())).child(check_button("Check Again")),
        Status::Available(r) => {
            let page = r.page.clone();
            row.child(muted(format!("nox {} is available.", r.version)))
                .child(
                    crate::settings::button("update-notes", "Release Notes", theme)
                        .on_click(move |_, _, cx| cx.open_url(&page)),
                )
                .child(
                    crate::settings::button("update-install", "Install and Relaunch", theme)
                        .on_click(|_, _, cx| install(cx, true)),
                )
        }
        Status::Downloading(r) => row.child(muted(format!("Downloading nox {}…", r.version))),
        Status::Ready(r) => row.child(muted(format!("nox {} is ready.", r.version))).child(
            crate::settings::button("update-relaunch", "Relaunch to Update", theme)
                .on_click(|_, _, cx| relaunch(cx)),
        ),
        Status::Failed(e) => row
            .child(div().text_color(theme.ansi(1)).child(format!("Update failed: {e}")))
            .child(check_button("Try Again")),
    }
    .into_any_element()
}

// ---- release lookup ---------------------------------------------------------------------------

/// `https://github.com/o/r` → `https://api.github.com/repos/o/r`.
fn api_base(repository: &str) -> String {
    let path = repository.trim_end_matches('/').trim_start_matches("https://github.com/");
    format!("https://api.github.com/repos/{path}")
}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let ua = format!("nox/{CURRENT_VERSION}");
    let out = Command::new("/usr/bin/curl")
        .args(["-fsSL", "--proto", "=https", "--connect-timeout", "15", "-A", &ua])
        .args(args)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().trim_start_matches("curl: ").to_string();
        Err(if err.is_empty() { "network error".into() } else { err })
    }
}

fn fetch_release(channel: &str) -> Result<Option<Release>, String> {
    let base = api_base(crate::about::REPOSITORY);
    let beta = channel == "beta";
    let url = if beta { format!("{base}/releases?per_page=10") } else { format!("{base}/releases/latest") };
    let body = curl(&["--max-time", "30", "-H", "Accept: application/vnd.github+json", &url])?;
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| format!("bad response: {e}"))?;
    Ok(pick_release(&v))
}

/// The newest usable release from `/releases/latest` (an object) or `/releases` (an array, newest
/// first; drafts skipped).
fn pick_release(v: &serde_json::Value) -> Option<Release> {
    match v {
        serde_json::Value::Array(list) => {
            list.iter().filter(|r| !r["draft"].as_bool().unwrap_or(false)).find_map(parse_release)
        }
        _ => parse_release(v),
    }
}

fn parse_release(v: &serde_json::Value) -> Option<Release> {
    let version = v["tag_name"].as_str()?.trim_start_matches('v').to_string();
    let page = v["html_url"].as_str()?.to_string();
    let assets = v["assets"].as_array()?;
    let asset = |name: &str| assets.iter().find(|a| a["name"].as_str() == Some(name));
    let zip = asset(ZIP_ASSET)?;
    Some(Release {
        version,
        page,
        zip_url: zip["browser_download_url"].as_str()?.to_string(),
        sha256: zip["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_lowercase),
        sums_url: asset(SUMS_ASSET).and_then(|a| a["browser_download_url"].as_str()).map(String::from),
    })
}

/// `(major, minor, patch, pre-release)` of `1.2.3` / `v1.2.3-beta.1`.
fn parse_version(v: &str) -> Option<(u64, u64, u64, Option<String>)> {
    let v = v.trim().trim_start_matches('v');
    let v = v.split('+').next()?;
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p.to_string())),
        None => (v, None),
    };
    let mut it = core.split('.').map(str::parse::<u64>);
    let major = it.next()?.ok()?;
    let minor = it.next().unwrap_or(Ok(0)).ok()?;
    let patch = it.next().unwrap_or(Ok(0)).ok()?;
    Some((major, minor, patch, pre))
}

/// Semver precedence of two pre-release strings (`beta.2` < `beta.10` < `rc.1`).
fn cmp_pre(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ia = a.split('.');
    let mut ib = b.split('.');
    loop {
        match (ia.next(), ib.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) => {
                let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => std::cmp::Ordering::Less,
                    (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if o.is_ne() {
                    return o;
                }
            }
        }
    }
}

/// Whether `candidate` is a newer version than `current`.
#[must_use]
pub fn is_newer(current: &str, candidate: &str) -> bool {
    let (Some(a), Some(b)) = (parse_version(current), parse_version(candidate)) else { return false };
    match (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)) {
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Greater => false,
        std::cmp::Ordering::Equal => match (&a.3, &b.3) {
            (Some(_), None) => true,
            (Some(x), Some(y)) => cmp_pre(x, y).is_lt(),
            _ => false,
        },
    }
}

// ---- download, verify, stage -----------------------------------------------------------------

/// The running app's bundle, if it can replace itself (inside a `.app` in a writable folder).
fn installable_bundle() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().and_then(std::fs::canonicalize).map_err(|e| e.to_string())?;
    let bundle = bundle_of(&exe).ok_or("This copy of nox isn't an app bundle.")?;
    let parent = bundle.parent().ok_or("No folder around the app.")?;
    let probe = parent.join(format!(".nox-update-probe-{}", std::process::id()));
    if std::fs::write(&probe, b"").is_err() {
        return Err(format!("nox can't write to {}.", parent.display()));
    }
    let _ = std::fs::remove_file(&probe);
    Ok(bundle)
}

/// `…/nox.app/Contents/MacOS/nox-app` → `…/nox.app`.
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension().is_some_and(|e| e == "app"))
    .then(|| bundle.to_path_buf())
}

fn run(cmd: &str, args: &[&std::ffi::OsStr]) -> Result<std::process::Output, String> {
    Command::new(cmd).args(args).output().map_err(|e| format!("{cmd}: {e}"))
}

/// Downloads the zip, checks its SHA-256, unpacks it and verifies the signature. Returns the
/// staged `nox.app` and its staging folder.
fn download_and_verify(r: &Release, current: &Path) -> Result<(PathBuf, PathBuf), String> {
    let dir = std::env::temp_dir().join(format!("nox-update-{}-{}", r.version, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let res = (|| {
        let zip = dir.join(ZIP_ASSET);
        let zip_s = zip.to_string_lossy().to_string();
        curl(&["--max-time", "900", "-o", &zip_s, &r.zip_url])?;

        let expected = match (&r.sha256, &r.sums_url) {
            (Some(h), _) => h.clone(),
            (None, Some(url)) => {
                let sums = String::from_utf8_lossy(&curl(&["--max-time", "60", url])?).to_string();
                sum_for(&sums, ZIP_ASSET).ok_or("the release has no checksum for the zip")?
            }
            (None, None) => return Err("the release has no checksum for the zip".into()),
        };
        let out = run("/usr/bin/shasum", &["-a".as_ref(), "256".as_ref(), zip.as_os_str()])?;
        let actual =
            String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap_or("").to_lowercase();
        if actual != expected {
            return Err("checksum mismatch".into());
        }

        let unpacked = dir.join("app");
        let out =
            run("/usr/bin/ditto", &["-x".as_ref(), "-k".as_ref(), zip.as_os_str(), unpacked.as_os_str()])?;
        if !out.status.success() {
            return Err("couldn't unpack the update".into());
        }
        let app = unpacked.join("nox.app");
        if !app.is_dir() {
            return Err("the update has no nox.app".into());
        }
        verify_signature(&app, current)?;
        Ok(app)
    })();
    match res {
        Ok(app) => Ok((app, dir)),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

/// The hash for `name` in a `shasum` listing (`<hex>  <name>` per line).
fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let hash = it.next()?;
        (it.next()?.trim_start_matches('*') == name).then(|| hash.to_lowercase())
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
struct SignInfo {
    identifier: Option<String>,
    team: Option<String>,
}

/// `Identifier=` / `TeamIdentifier=` from `codesign -dv` output.
fn parse_sign_info(out: &str) -> SignInfo {
    let field = |key: &str| {
        out.lines()
            .find_map(|l| l.strip_prefix(key))
            .map(str::trim)
            .filter(|v| !v.is_empty() && *v != "not set")
            .map(String::from)
    };
    SignInfo { identifier: field("Identifier="), team: field("TeamIdentifier=") }
}

fn sign_info(app: &Path) -> Result<SignInfo, String> {
    let out = run("/usr/bin/codesign", &["-dv".as_ref(), "--verbose=2".as_ref(), app.as_os_str()])?;
    Ok(parse_sign_info(&String::from_utf8_lossy(&out.stderr)))
}

/// The update must have a valid signature, the same bundle identifier, and the same Developer ID
/// team as the running app (or, if this copy is ad-hoc signed, pass Gatekeeper).
fn verify_signature(app: &Path, current: &Path) -> Result<(), String> {
    let out = run(
        "/usr/bin/codesign",
        &["--verify".as_ref(), "--deep".as_ref(), "--strict".as_ref(), app.as_os_str()],
    )?;
    if !out.status.success() {
        return Err("the update's code signature is invalid".into());
    }
    let new = sign_info(app)?;
    let cur = sign_info(current)?;
    if new.identifier.is_none() || new.identifier != cur.identifier {
        return Err("the update is a different app".into());
    }
    match &cur.team {
        Some(team) if new.team.as_ref() == Some(team) => Ok(()),
        Some(_) => Err("the update is signed by a different developer".into()),
        None => {
            let out = run(
                "/usr/sbin/spctl",
                &["--assess".as_ref(), "--type".as_ref(), "execute".as_ref(), app.as_os_str()],
            )?;
            if out.status.success() { Ok(()) } else { Err("Gatekeeper rejected the update".into()) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("0.1.9", "v0.1.10"));
        assert!(is_newer("1.0.0-beta.2", "1.0.0"));
        assert!(is_newer("1.0.0-beta.2", "1.0.0-beta.10"));
        assert!(is_newer("1.0.0-beta.2", "1.0.0-rc.1"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.0.0", "1.0.0-rc.1"));
        assert!(!is_newer("1.2.0", "1.1.9"));
        assert!(!is_newer("1.0.0", "garbage"));
        assert_eq!(parse_version("v2.3"), Some((2, 3, 0, None)));
    }

    #[test]
    fn api_url() {
        assert_eq!(
            api_base("https://github.com/pwittchen/nox"),
            "https://api.github.com/repos/pwittchen/nox"
        );
        assert_eq!(api_base("https://github.com/a/b/"), "https://api.github.com/repos/a/b");
    }

    fn release_json(tag: &str, draft: bool, digest: bool) -> serde_json::Value {
        let mut zip = serde_json::json!({
            "name": ZIP_ASSET,
            "browser_download_url": format!("https://example.com/{tag}/{ZIP_ASSET}"),
        });
        if digest {
            zip["digest"] = "sha256:ABCDEF".into();
        }
        serde_json::json!({
            "tag_name": tag,
            "draft": draft,
            "html_url": format!("https://github.com/pwittchen/nox/releases/tag/{tag}"),
            "assets": [
                {"name": "nox-macos-aarch64.dmg", "browser_download_url": "https://example.com/dmg"},
                zip,
                {"name": SUMS_ASSET, "browser_download_url": "https://example.com/sums"},
            ],
        })
    }

    #[test]
    fn parses_releases() {
        let r = pick_release(&release_json("v1.2.3", false, true)).unwrap();
        assert_eq!(r.version, "1.2.3");
        assert_eq!(r.zip_url, format!("https://example.com/v1.2.3/{ZIP_ASSET}"));
        assert_eq!(r.sha256.as_deref(), Some("abcdef"));
        assert_eq!(r.sums_url.as_deref(), Some("https://example.com/sums"));
        let r = pick_release(&release_json("v1.2.3", false, false)).unwrap();
        assert_eq!(r.sha256, None);
        // A list (beta channel): drafts are skipped, the newest usable one wins.
        let list = serde_json::json!([
            release_json("v2.0.0-beta.1", true, true),
            release_json("v1.9.0-beta.2", false, true),
            release_json("v1.8.0", false, true),
        ]);
        assert_eq!(pick_release(&list).unwrap().version, "1.9.0-beta.2");
        // No zip asset: not installable.
        let mut v = release_json("v1.0.0", false, true);
        v["assets"] = serde_json::json!([]);
        assert_eq!(pick_release(&v), None);
    }

    #[test]
    fn checksums() {
        let sums = "AAA  nox-macos-aarch64.dmg\nBBB *nox-macos-aarch64.zip\n";
        assert_eq!(sum_for(sums, ZIP_ASSET).as_deref(), Some("bbb"));
        assert_eq!(sum_for(sums, "other"), None);
    }

    #[test]
    fn signing_info() {
        let out = "Executable=/A/nox.app/Contents/MacOS/nox-app\nIdentifier=io.wittchen.nox\n\
                   Format=app bundle\nTeamIdentifier=ABCDE12345\n";
        assert_eq!(
            parse_sign_info(out),
            SignInfo { identifier: Some("io.wittchen.nox".into()), team: Some("ABCDE12345".into()) }
        );
        let adhoc = "Identifier=io.wittchen.nox\nTeamIdentifier=not set\n";
        assert_eq!(parse_sign_info(adhoc).team, None);
    }

    #[test]
    fn bundle_paths() {
        assert_eq!(
            bundle_of(Path::new("/Applications/nox.app/Contents/MacOS/nox-app")),
            Some(PathBuf::from("/Applications/nox.app"))
        );
        assert_eq!(bundle_of(Path::new("/repo/target/debug/nox-app")), None);
    }
}
