use super::*;

#[test]
fn hex_parsing() {
    assert_eq!(Rgba::from_hex("#fff"), Some(Rgba::WHITE));
    assert_eq!(Rgba::from_hex("#000000"), Some(Rgba::BLACK));
    let c = Rgba::from_hex("#22D3EE33").unwrap();
    assert_eq!(c.to_hex(), "#22d3ee33");
    assert!((c.a - 0x33 as f32 / 255.0).abs() < 1e-6);
    assert_eq!(Rgba::from_hex("#18191b").unwrap().to_hex(), "#18191b");
    for bad in ["", "fff", "#ff", "#ggg", "#12345", "#1234567", "#+12345"] {
        assert_eq!(Rgba::from_hex(bad), None, "{bad}");
    }
}

#[test]
fn contrast_basics() {
    assert!((contrast_ratio(Rgba::BLACK, Rgba::WHITE) - 21.0).abs() < 0.01);
    assert!((contrast_ratio(Rgba::WHITE, Rgba::WHITE) - 1.0).abs() < 1e-6);
    let bg = Rgba::from_hex("#18191b").unwrap();
    let fg = Rgba::from_hex("#222326").unwrap();
    let fixed = ensure_contrast(fg, bg, 4.5);
    assert!(contrast_ratio(fixed, bg) >= 4.5);
    assert!(fixed.relative_luminance() > fg.relative_luminance());
    // Already fine / disabled: unchanged.
    assert_eq!(ensure_contrast(Rgba::WHITE, bg, 4.5), Rgba::WHITE);
    assert_eq!(ensure_contrast(fg, bg, 1.0), fg);
    // Light background: darkens.
    let light = ensure_contrast(Rgba::from_hex("#eeeeee").unwrap(), Rgba::WHITE, 4.5);
    assert!(contrast_ratio(light, Rgba::WHITE) >= 4.5);
    // Impossible ratio: best effort, no panic.
    let _ = ensure_contrast(fg, Rgba::from_hex("#777777").unwrap(), 21.0);
}

#[test]
fn blend_and_alpha() {
    let half = Rgba::WHITE.with_alpha(0.5).blend(Rgba::BLACK);
    assert!((half.r - 0.5).abs() < 1e-6 && (half.a - 1.0).abs() < 1e-6);
    assert_eq!(Rgba::BLACK.mix(Rgba::WHITE, 1.0), Rgba::WHITE);
}

fn all_themes() -> Vec<Theme> {
    let mut v = vec![builtin_nox()];
    for name in bundled_theme_names() {
        let (t, notices) = parse_theme(name, bundled_theme_source(name).unwrap()).unwrap();
        assert!(notices.is_empty(), "{name}: {notices:?}");
        v.push(t);
    }
    v
}

#[test]
fn every_bundled_theme_parses_without_notices() {
    assert_eq!(bundled_theme_names().len(), 17);
    let themes = all_themes();
    let nord = themes.iter().find(|t| t.name == "nord").unwrap();
    assert!(nord.is_single_variant() && nord.light.is_none());
    for t in &themes {
        if t.name != "nord" {
            assert!(!t.is_single_variant(), "{}", t.name);
        }
    }
}

#[test]
fn text_contrast_in_every_variant() {
    for t in all_themes() {
        for v in [&t.light, &t.dark].into_iter().flatten() {
            let text = v.text.blend(v.bg);
            let r = contrast_ratio(text, v.bg);
            // Solarized Light keeps upstream's base00 text on base3 (4.1:1) on purpose; it is
            // the one bundled variant below AA, and `terminal.minimum_contrast` can lift it.
            let min = if t.name == "solarized" && !v.bg.is_dark() { 4.0 } else { 4.5 };
            assert!(r >= min, "{}: text on bg is {r:.2}", t.name);
        }
    }
}

#[test]
fn nox_ansi_meets_wcag_aa() {
    let t = builtin_nox();
    for v in [t.dark.unwrap(), t.light.unwrap()] {
        for (c, name) in v.ansi.iter().zip(ANSI_NAMES).skip(1).take(6) {
            let r = contrast_ratio(*c, v.bg);
            assert!(r >= 4.5, "{name} is {r:.2}");
        }
    }
}

#[test]
fn nox_variant_tokens() {
    let t = builtin_nox();
    assert_eq!(t.variant(true).bg.to_hex(), "#18191b");
    assert_eq!(t.variant(false).accent.to_hex(), "#0891b2");
    assert_eq!(t.variant(true).caret_text, t.variant(true).bg);
    assert_eq!(t.variant(false).bg_elevated.to_hex(), "#efeff1");
}

#[test]
fn single_variant_always_returned() {
    let (t, n) = parse_theme("x", "[light]\nbg = \"#ffffff\"\n").unwrap();
    assert_eq!(n, [] as [std::string::String; 0]);
    assert!(t.is_single_variant());
    assert_eq!(t.variant(true).bg, Rgba::WHITE);
    assert_eq!(t.variant(false).bg, Rgba::WHITE);
}

#[test]
fn palette256_values() {
    let v = nox_dark();
    let p = palette256(&v);
    assert_eq!(p[1], v.ansi[1]);
    assert_eq!(p[15], v.ansi[15]);
    assert_eq!(p[16].to_hex(), "#000000");
    assert_eq!(p[21].to_hex(), "#0000ff");
    assert_eq!(p[196].to_hex(), "#ff0000");
    assert_eq!(p[46].to_hex(), "#00ff00");
    assert_eq!(p[231].to_hex(), "#ffffff");
    assert_eq!(p[67].to_hex(), "#5f87af");
    assert_eq!(p[232].to_hex(), "#080808");
    assert_eq!(p[244].to_hex(), "#808080");
    assert_eq!(p[255].to_hex(), "#eeeeee");
}

#[test]
fn missing_keys_fall_back_and_derive() {
    let src = "[dark]\nbg = \"#000000\"\ntext = \"#ffffff\"\n";
    let (t, n) = parse_theme("partial", src).unwrap();
    assert_eq!(n, [] as [std::string::String; 0]);
    let d = t.variant(true);
    let nox = nox_dark();
    assert_eq!(d.bg, Rgba::BLACK);
    assert_eq!(d.accent, nox.accent);
    assert_eq!(d.ansi, nox.ansi);
    assert_eq!(d.caret_text, Rgba::BLACK);
    assert!(d.bg_elevated.relative_luminance() > d.bg.relative_luminance());

    let (t, _) = parse_theme("p", "[light]\nbg = \"#ffffff\"\n").unwrap();
    let l = t.variant(false);
    assert!(l.bg_elevated.relative_luminance() < l.bg.relative_luminance());
}

#[test]
fn invalid_and_unknown_keys_are_notices() {
    let src = r##"
extra = 1
[dark]
bg = "#zzzzzz"
accent = 5
wat = "#ffffff"
hl-keyword = "#ff0000"
caret-text = "#010203"
[dark.ansi]
red = "#ff0000"
purple = "#ff00ff"
green = "nope"
"##;
    let (t, n) = parse_theme("bad", src).unwrap();
    assert_eq!(n.len(), 6, "{n:#?}");
    let d = t.variant(true);
    assert_eq!(d.bg, nox_dark().bg);
    assert_eq!(d.ansi[1].to_hex(), "#ff0000");
    assert_eq!(d.ansi[2], nox_dark().ansi[2]);
    assert_eq!(d.caret_text.to_hex(), "#010203");
    assert!(parse_theme("e", "[x]\n").is_err());
    assert!(parse_theme("e", "[dark\n").unwrap_err().starts_with("line 1"));
}

#[test]
fn kayet_theme_file_works() {
    let kayet = "# Catppuccin for kayet\n[dark]\nbg = \"#1e1e2e\"\nbg-sidebar = \"#181825\"\ntext = \"#cdd6f4\"\ntext-muted = \"#9399b2\"\nborder = \"#313244\"\naccent = \"#cba6f7\"\nselection = \"#9399b240\"\ncaret = \"#f5e0dc\"\nhl-keyword = \"#cba6f7\"\nhl-string = \"#a6e3a1\"\nhl-number = \"#fab387\"\nhl-title = \"#89b4fa\"\nhl-comment = \"#9399b2\"\n";
    let (t, n) = parse_theme("kayet-cat", kayet).unwrap();
    assert!(n.is_empty(), "{n:?}");
    assert_eq!(t.variant(true).ansi, nox_dark().ansi);
    assert_eq!(t.variant(true).accent.to_hex(), "#cba6f7");
}

#[test]
fn load_theme_precedence_and_fallbacks() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();

    let (t, n) = load_theme("nox", p);
    assert_eq!(t, builtin_nox());
    assert_eq!(n, [] as [std::string::String; 0]);

    let (t, n) = load_theme("gruvbox", p);
    assert_eq!(n, [] as [std::string::String; 0]);
    assert_eq!(t.variant(true).bg.to_hex(), "#282828");

    // User file overrides the bundled one.
    std::fs::write(p.join("gruvbox.toml"), "[dark]\nbg = \"#123456\"\n").unwrap();
    let (t, n) = load_theme("gruvbox", p);
    assert_eq!(n, [] as [std::string::String; 0]);
    assert_eq!(t.variant(true).bg.to_hex(), "#123456");
    assert!(t.is_single_variant());

    // Broken user file: notice, bundled one used.
    std::fs::write(p.join("nord.toml"), "[dark\n").unwrap();
    let (t, n) = load_theme("nord", p);
    assert_eq!(n.len(), 1, "{n:?}");
    assert_eq!(t.variant(true).bg.to_hex(), "#2e3440");

    // User-only theme.
    std::fs::write(p.join("mine.toml"), "[light]\nbg = \"#fafafa\"\n").unwrap();
    assert_eq!(load_theme("mine", p).0.name, "mine");
    assert!(available_theme_names(p).contains(&"mine".to_string()));

    for bad in ["does-not-exist", "../etc/passwd", "dark", "system", ""] {
        let (t, n) = load_theme(bad, p);
        assert_eq!(t, builtin_nox(), "{bad}");
        assert_eq!(n.len(), 1, "{bad}: {n:?}");
    }
}

#[test]
fn name_validation() {
    assert!(is_valid_theme_name("tokyo-night"));
    assert!(is_valid_theme_name("my_theme2"));
    assert!(!is_valid_theme_name("a b"));
    assert!(!is_valid_theme_name("a.b"));
    assert!(!is_valid_theme_name(""));
}

#[test]
fn unusable_user_only_themes_fall_back_to_nox() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    // Syntax error and no bundled theme of that name.
    std::fs::write(p.join("broken.toml"), "[dark\n").unwrap();
    let (t, n) = load_theme("broken", p);
    assert_eq!(t, builtin_nox());
    assert_eq!(n.len(), 2, "{n:?}");
    assert!(n[0].starts_with("theme \"broken\" (") && n[0].contains("line 1"), "{n:?}");
    assert_eq!(n[1], "theme \"broken\": using nox");
    // Not UTF-8: can't be read.
    std::fs::write(p.join("binary.toml"), b"\xff\xfe").unwrap();
    let (t, n) = load_theme("binary", p);
    assert_eq!(t, builtin_nox());
    assert!(n[0].starts_with("theme \"binary\": cannot read "), "{n:?}");
    assert_eq!(n.last().unwrap(), "theme \"binary\": using nox");
}

#[test]
fn structural_problems_are_notices() {
    let src = r#"
light = 5
[dark]
bg-elevated = "nope"
ansi = 3
"#;
    let (t, n) = parse_theme("odd", src).unwrap();
    assert!(n.contains(&"theme \"odd\": [light] must be a table; ignored".to_string()), "{n:?}");
    assert!(n.contains(&"theme \"odd\": [dark] ansi must be a table; ignored".to_string()), "{n:?}");
    assert!(
        n.iter().any(|m| m.starts_with("theme \"odd\": [dark] bg-elevated = \"nope\" is not a")),
        "{n:?}"
    );
    assert!(t.light.is_none());
    let d = t.variant(true);
    assert_eq!(d.bg_elevated, nox_dark().bg_elevated);
    assert_eq!(d.ansi, nox_dark().ansi);
    // A valid bg-elevated is taken as is; an invalid bg doesn't derive one.
    let (t, _) = parse_theme("e", "[dark]\nbg = \"bad\"\nbg-elevated = \"#010101\"\n").unwrap();
    assert_eq!(t.variant(true).bg_elevated.to_hex(), "#010101");
    assert_eq!(t.variant(true).bg, nox_dark().bg);
}

#[test]
fn available_names_skip_reserved_invalid_and_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    for f in ["zeta.toml", "alpha.toml", "dark.toml", "bad name.toml", "gruvbox.toml", "notes.txt"] {
        std::fs::write(p.join(f), "[dark]\n").unwrap();
    }
    let names = available_theme_names(p);
    assert_eq!(names[0], "nox");
    let bundled = bundled_theme_names().len();
    assert_eq!(names.len(), 1 + bundled + 2, "{names:?}");
    assert_eq!(&names[names.len() - 2..], ["alpha", "zeta"]);
    assert_eq!(names.iter().filter(|n| *n == "gruvbox").count(), 1);
    // A missing directory: just the built-in and bundled ones.
    assert_eq!(available_theme_names(&p.join("missing")).len(), 1 + bundled);
}
