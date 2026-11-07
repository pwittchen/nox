//! Colors: hex parsing, WCAG contrast and minimum-contrast adjustment.

/// A straight (non-premultiplied) sRGB color, components in `0.0..=1.0`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const BLACK: Rgba = Rgba::rgb(0.0, 0.0, 0.0);
    pub const WHITE: Rgba = Rgba::rgb(1.0, 1.0, 1.0);
    pub const TRANSPARENT: Rgba = Rgba::new(0.0, 0.0, 0.0, 0.0);

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// From 8-bit channels.
    pub fn from_u8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0)
    }

    /// Parses `#rgb`, `#rrggbb` or `#rrggbbaa` (the `#` is required).
    pub fn from_hex(s: &str) -> Option<Self> {
        let hex = s.trim().strip_prefix('#')?;
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        match hex.len() {
            3 => {
                let n = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok().map(|v| v * 17);
                Some(Self::from_u8(n(0)?, n(1)?, n(2)?, 255))
            }
            6 => Some(Self::from_u8(byte(0)?, byte(2)?, byte(4)?, 255)),
            8 => Some(Self::from_u8(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        }
    }

    fn to_u8(c: f32) -> u8 {
        (c.clamp(0.0, 1.0) * 255.0).round() as u8
    }

    /// `#rrggbb`, or `#rrggbbaa` when not fully opaque.
    pub fn to_hex(self) -> String {
        let (r, g, b, a) =
            (Self::to_u8(self.r), Self::to_u8(self.g), Self::to_u8(self.b), Self::to_u8(self.a));
        if a == 255 { format!("#{r:02x}{g:02x}{b:02x}") } else { format!("#{r:02x}{g:02x}{b:02x}{a:02x}") }
    }

    /// The same color with a different alpha.
    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// Linear interpolation towards `other` (`t = 0` → self, `t = 1` → other), alpha included.
    pub fn mix(self, other: Rgba, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let l = |a: f32, b: f32| a + (b - a) * t;
        Self::new(l(self.r, other.r), l(self.g, other.g), l(self.b, other.b), l(self.a, other.a))
    }

    /// Composites `self` (using its alpha) over an opaque `bg`; the result is opaque.
    pub fn blend(self, bg: Rgba) -> Self {
        let a = self.a.clamp(0.0, 1.0);
        let l = |f: f32, b: f32| f * a + b * (1.0 - a);
        Self::rgb(l(self.r, bg.r), l(self.g, bg.g), l(self.b, bg.b))
    }

    /// WCAG 2 relative luminance (alpha ignored).
    pub fn relative_luminance(self) -> f32 {
        fn lin(c: f32) -> f32 {
            let c = c.clamp(0.0, 1.0);
            if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        }
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// Whether the color reads as dark (luminance below the midpoint).
    pub fn is_dark(self) -> bool {
        self.relative_luminance() < 0.18
    }
}

/// WCAG 2 contrast ratio (1.0–21.0), alpha ignored. Composite with [`Rgba::blend`] first for
/// translucent colors.
pub fn contrast_ratio(a: Rgba, b: Rgba) -> f32 {
    let (la, lb) = (a.relative_luminance(), b.relative_luminance());
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Lightens or darkens `fg` just enough to reach `min_ratio` against `bg`
/// (`terminal.minimum_contrast`). `min_ratio <= 1` returns `fg` unchanged. Alpha is kept.
pub fn ensure_contrast(fg: Rgba, bg: Rgba, min_ratio: f32) -> Rgba {
    if min_ratio <= 1.0 || contrast_ratio(fg, bg) >= min_ratio {
        return fg;
    }
    let toward = |target: Rgba| -> (Rgba, f32) {
        let target = target.with_alpha(fg.a);
        if contrast_ratio(target, bg) < min_ratio {
            return (target, contrast_ratio(target, bg));
        }
        // Binary search for the smallest step that meets the ratio.
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..20 {
            let mid = (lo + hi) / 2.0;
            if contrast_ratio(fg.mix(target, mid), bg) >= min_ratio {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        let c = fg.mix(target, hi);
        (c, contrast_ratio(c, bg))
    };
    // Prefer moving away from the background's lightness.
    let (first, second) =
        if bg.relative_luminance() < 0.18 { (Rgba::WHITE, Rgba::BLACK) } else { (Rgba::BLACK, Rgba::WHITE) };
    let (c1, r1) = toward(first);
    if r1 >= min_ratio {
        return c1;
    }
    let (c2, r2) = toward(second);
    if r2 >= min_ratio || r2 > r1 { c2 } else { c1 }
}
