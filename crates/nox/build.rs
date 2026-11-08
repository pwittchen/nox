//! Embeds a minimal `Info.plist` into the `nox-app` binary (`__TEXT,__info_plist`), so a bare
//! binary (`cargo run`, `scripts/run.sh`) shows as "nox" in the menu bar and the Dock instead of
//! its file name. Inside `nox.app` the bundle's own `Info.plist` takes precedence. No bundle
//! identifier on purpose: a dev build must not share defaults or permissions with the app.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>nox</string>
	<key>CFBundleDisplayName</key>
	<string>nox</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
"#
    );
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("Info.plist");
    std::fs::write(&out, plist).expect("write Info.plist");
    println!("cargo:rustc-link-arg-bin=nox-app=-Wl,-sectcreate,__TEXT,__info_plist,{}", out.display());
}
