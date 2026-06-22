//! Color-scheme scaffolding (pure, no egui). The live app currently uses hard-coded `theme::*`
//! consts; switching schemes at runtime means reading semantic roles from a `Palette` instead. This
//! defines the role set + several schemes as plain RGB data and a WCAG contrast check, so the
//! migration is mechanical and — crucially — every shipped scheme is verified legible (body text
//! ≥ 4.5:1 against its background) before it can be selected. The egui layer converts `Rgb` →
//! `Color32`; the data and the contrast math live here so they're testable headless.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Semantic roles (not raw colors) so widgets reference intent, not a specific hue.
#[derive(Clone, Debug)]
pub struct Palette {
    pub name: &'static str,
    pub bg: Rgb,       // app background
    pub panel: Rgb,    // window / panel fill
    pub panel2: Rgb,   // raised panel
    pub line: Rgb,     // separators / borders
    pub ink: Rgb,      // body text (must clear AA vs bg)
    pub dim: Rgb,      // secondary text
    pub faint: Rgb,    // decorative / disabled (no contrast guarantee)
    pub accent: Rgb,   // primary accent
    pub good: Rgb,     // success
    pub warn: Rgb,     // warning
    pub bad: Rgb,      // error
}

/// Relative luminance per WCAG 2.1 (sRGB linearized).
fn luminance(c: Rgb) -> f32 {
    let lin = |v: u8| { let s = v as f32 / 255.0; if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) } };
    0.2126 * lin(c.0) + 0.7152 * lin(c.1) + 0.0722 * lin(c.2)
}

/// WCAG contrast ratio between two colors (1.0 … 21.0). ≥4.5 = AA for normal text, ≥3.0 = AA large.
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// The current app look, as a Palette (matches the existing theme:: consts).
pub fn midnight() -> Palette {
    Palette { name: "Midnight",
        bg: Rgb(0x0a,0x0c,0x10), panel: Rgb(0x10,0x14,0x1b), panel2: Rgb(0x16,0x1b,0x24), line: Rgb(0x22,0x2a,0x36),
        ink: Rgb(0xcd,0xd6,0xe3), dim: Rgb(0x9a,0xa6,0xb6), faint: Rgb(0x4b,0x55,0x63),
        accent: Rgb(0xf7,0xa2,0x3b), good: Rgb(0x8f,0xd6,0x94), warn: Rgb(0xf5,0xc2,0x11), bad: Rgb(0xff,0x63,0x63) }
}
pub fn nord() -> Palette {
    Palette { name: "Nord",
        bg: Rgb(0x2e,0x34,0x40), panel: Rgb(0x3b,0x42,0x52), panel2: Rgb(0x43,0x4c,0x5e), line: Rgb(0x4c,0x56,0x6a),
        ink: Rgb(0xec,0xef,0xf4), dim: Rgb(0xd8,0xde,0xe9), faint: Rgb(0x7b,0x88,0x9c),
        accent: Rgb(0x88,0xc0,0xd0), good: Rgb(0xa3,0xbe,0x8c), warn: Rgb(0xeb,0xcb,0x8b), bad: Rgb(0xbf,0x61,0x6a) }
}
pub fn gruvbox() -> Palette {
    Palette { name: "Gruvbox",
        bg: Rgb(0x1d,0x20,0x21), panel: Rgb(0x28,0x28,0x28), panel2: Rgb(0x32,0x30,0x2f), line: Rgb(0x50,0x49,0x45),
        ink: Rgb(0xeb,0xdb,0xb2), dim: Rgb(0xd5,0xc4,0xa1), faint: Rgb(0x92,0x83,0x74),
        accent: Rgb(0xfe,0x80,0x19), good: Rgb(0xb8,0xbb,0x26), warn: Rgb(0xfa,0xbd,0x2f), bad: Rgb(0xfb,0x49,0x34) }
}
pub fn paper() -> Palette { // light scheme
    Palette { name: "Paper",
        bg: Rgb(0xfb,0xfb,0xf7), panel: Rgb(0xff,0xff,0xff), panel2: Rgb(0xf0,0xf0,0xea), line: Rgb(0xd6,0xd6,0xcc),
        ink: Rgb(0x23,0x28,0x2e), dim: Rgb(0x52,0x5a,0x63), faint: Rgb(0x9a,0xa0,0xa8),
        accent: Rgb(0xb5,0x5a,0x00), good: Rgb(0x2f,0x7a,0x33), warn: Rgb(0x8a,0x6d,0x00), bad: Rgb(0xc0,0x2a,0x2a) }
}

pub fn all() -> Vec<Palette> { vec![midnight(), nord(), gruvbox(), paper()] }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_extremes() {
        assert!((contrast(Rgb(255,255,255), Rgb(0,0,0)) - 21.0).abs() < 0.1, "white on black ≈ 21:1");
        assert!((contrast(Rgb(80,80,80), Rgb(80,80,80)) - 1.0).abs() < 1e-3, "same color = 1:1");
    }

    #[test]
    fn every_scheme_has_legible_body_text() {
        for p in all() {
            let c = contrast(p.ink, p.bg);
            assert!(c >= 4.5, "{}: ink/bg contrast {c:.2} < 4.5 (AA) — would ship illegible body text", p.name);
        }
    }

    #[test]
    fn secondary_text_meets_large_aa() {
        for p in all() {
            let c = contrast(p.dim, p.bg);
            assert!(c >= 3.0, "{}: dim/bg contrast {c:.2} < 3.0 (AA-large)", p.name);
        }
    }

    #[test]
    fn status_colors_distinct_from_bg() {
        for p in all() {
            for (role, col) in [("good", p.good), ("warn", p.warn), ("bad", p.bad), ("accent", p.accent)] {
                assert!(contrast(col, p.bg) >= 2.0, "{}: {role} barely visible on bg", p.name);
            }
        }
    }
}
