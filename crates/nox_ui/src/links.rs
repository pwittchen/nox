//! Link detection on a terminal line (SPEC §7.7): URLs and file paths relative to the pane's
//! cwd. (OSC 8 hyperlinks come from the cells themselves.)

use std::ops::Range;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::pane::LinkTarget;

static URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(?:https?|ftp|file|ssh|git)://[^\s<>"'`\x00-\x1f]+|\bmailto:[^\s<>"'`]+"#).unwrap()
});

static PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:~|\.{1,2})?/?[A-Za-z0-9_@+\-.]+(?:/[A-Za-z0-9_@+\-.]+)*/?(?::\d+(?::\d+)?)?").unwrap()
});

/// Find a link in `text` covering byte index `idx`. Returns its byte range and target.
pub fn find_link(text: &str, idx: usize, cwd: Option<&Path>) -> Option<(Range<usize>, LinkTarget)> {
    for m in URL.find_iter(text) {
        let mut end = m.end();
        // Trim trailing punctuation that is usually not part of the URL.
        while end > m.start() {
            let c = text[..end].chars().next_back().unwrap();
            let unbalanced_paren = c == ')'
                && text[m.start()..end].matches('(').count() < text[m.start()..end].matches(')').count();
            if ".,;:!?'\"".contains(c) || unbalanced_paren || c == ']' || c == '>' {
                end -= c.len_utf8();
            } else {
                break;
            }
        }
        if (m.start()..end).contains(&idx) {
            return Some((m.start()..end, LinkTarget::Url(text[m.start()..end].to_string())));
        }
    }
    for m in PATH.find_iter(text) {
        if !(m.start()..m.end()).contains(&idx) {
            continue;
        }
        let raw = m.as_str().trim_end_matches(['.', ',', ':', ';']);
        if raw.len() < 2 || (!raw.contains('/') && !raw.contains('.')) {
            return None;
        }
        // Strip a `:line[:col]` suffix.
        let path_part = strip_line_suffix(raw);
        let resolved = if let Some(rest) = path_part.strip_prefix("~/") {
            std::env::var_os("HOME").map(|h| Path::new(&h).join(rest))?
        } else if path_part.starts_with('/') {
            Path::new(path_part).to_path_buf()
        } else {
            cwd?.join(path_part)
        };
        if resolved.exists() {
            return Some((m.start()..m.start() + raw.len(), LinkTarget::Path(resolved)));
        }
        return None;
    }
    None
}

fn strip_line_suffix(s: &str) -> &str {
    let mut out = s;
    for _ in 0..2 {
        if let Some((head, tail)) = out.rsplit_once(':')
            && !tail.is_empty()
            && tail.chars().all(|c| c.is_ascii_digit())
        {
            out = head;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_urls() {
        let t = "see https://example.com/a(b)c, then";
        let i = t.find("example").unwrap();
        let (r, target) = find_link(t, i, None).unwrap();
        assert_eq!(&t[r], "https://example.com/a(b)c");
        assert_eq!(target, LinkTarget::Url("https://example.com/a(b)c".into()));
        assert!(find_link(t, 0, None).is_none());
    }

    #[test]
    fn finds_existing_paths() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("src/main.rs"), "").unwrap();
        let t = "error at src/main.rs:12:5 here";
        let i = t.find("main").unwrap();
        let (r, target) = find_link(t, i, Some(d.path())).unwrap();
        assert_eq!(&t[r], "src/main.rs:12:5");
        assert_eq!(target, LinkTarget::Path(d.path().join("src/main.rs")));
        let t2 = "missing/file.rs";
        assert!(find_link(t2, 2, Some(d.path())).is_none());
    }

    #[test]
    fn trims_trailing_punctuation_and_finds_mailto() {
        let t = "(see https://x.dev/a). and [https://y.dev/b] or <https://z.dev>";
        let (r, _) = find_link(t, t.find("x.dev").unwrap(), None).unwrap();
        assert_eq!(&t[r], "https://x.dev/a");
        let (r, _) = find_link(t, t.find("y.dev").unwrap(), None).unwrap();
        assert_eq!(&t[r], "https://y.dev/b");
        let (r, _) = find_link(t, t.find("z.dev").unwrap(), None).unwrap();
        assert_eq!(&t[r], "https://z.dev");
        let t = "write to mailto:me@nox.dev, please";
        let (r, target) = find_link(t, t.find("me@").unwrap(), None).unwrap();
        assert_eq!(&t[r], "mailto:me@nox.dev");
        assert_eq!(target, LinkTarget::Url("mailto:me@nox.dev".into()));
    }

    #[test]
    fn paths_need_a_slash_or_dot_and_must_exist() {
        let d = tempfile::tempdir().unwrap();
        let abs = d.path().join("f.txt");
        std::fs::write(&abs, "").unwrap();
        let t = format!("at {}:3.", abs.display());
        let (r, target) = find_link(&t, 5, None).unwrap();
        assert_eq!(&t[r], format!("{}:3", abs.display()));
        assert_eq!(target, LinkTarget::Path(abs));
        assert!(find_link("just words", 2, Some(d.path())).is_none(), "no slash, no dot");
        assert!(find_link("a b", 0, Some(d.path())).is_none(), "too short");
        assert!(find_link("rel/x.rs", 1, None).is_none(), "relative paths need a cwd");
        assert!(find_link("~/definitely-not-here-nox/x", 3, None).is_none());
        assert_eq!(strip_line_suffix("a.rs:1:2"), "a.rs");
        assert_eq!(strip_line_suffix("a.rs:x"), "a.rs:x");
        assert_eq!(strip_line_suffix("a.rs:"), "a.rs:");
    }
}
