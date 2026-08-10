//! qr_render — `QrCodeRenderer.cpp`, ported.
//!
//! Renders an sc-ID as a scannable square.
//!
//! ## This is a QR-shaped identicon, and calling it that matters
//!
//! The C++ draws finder patterns, timing rows and a data field derived from the
//! id — a **Version 1 layout** at 21×21 — but there is no Reed-Solomon encoder,
//! no mask evaluation and no format information. A real QR reader will not
//! decode it. It is a recognisable, deterministic visual for an id, and the
//! port keeps that honestly rather than implying otherwise: `Layout::modules`
//! is the drawing, and the doc says what it is not.
//!
//! Porting it as-is is the right call. Replacing it with a real encoder is a
//! different decision with a dependency attached, and making that decision
//! silently inside a port is how a "faithful" port stops being one.
//!
//! ## Three things that are load-bearing
//!
//! * **Cell size is an integer, floored, with a floor of 2.** A fractional
//!   module means the renderer resamples and the edges blur — the C++ names
//!   this as *"visible as the soft edges on the FP5 QR"*. Two is the minimum
//!   where a module survives at all.
//! * **Rendered at device pixel ratio.** A phone is 2–3x; drawing at logical
//!   size and letting the toolkit upscale is exactly the blur above.
//! * **A WCAG 4.5:1 contrast guard.** Unusual for a QR and completely right: a
//!   camera has to separate the modules, and a themed palette can easily put
//!   foreground and background close together. When the theme fails the check
//!   it falls back to pure black or white rather than to something nearer.

/// Version 1 QR: 21×21 modules.
pub const MODULES: usize = 21;
/// Divisor used to pick a cell size, leaving room for a quiet border.
pub const CELL_DIVISOR: u32 = 22;
/// Smallest cell that still renders as a module.
pub const MIN_CELL: u32 = 2;
/// Contrast floor, WCAG AA for normal text.
pub const MIN_CONTRAST: f32 = 4.5;

/// Pixel size of one module, and the resulting image size.
///
/// Integer and floored, because a fractional module resamples and blurs the
/// edges — the FP5 symptom the C++ calls out.
pub fn cell_size(px: u32, dpr: f32) -> u32 {
    let scaled = (px as f32 * dpr.max(0.1)) as u32;
    (scaled / CELL_DIVISOR).max(MIN_CELL)
}

pub fn image_size(px: u32, dpr: f32) -> u32 { cell_size(px, dpr) * MODULES as u32 }

/// Relative luminance, sRGB.
pub fn luminance(c: [u8; 3]) -> f32 {
    let ch = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2])
}

/// WCAG contrast ratio between two colours.
pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (l1, l2) = (luminance(a), luminance(b));
    (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
}

/// Pick a foreground that a camera can separate from `bg`.
///
/// Falls back to pure black or white rather than to something merely better: a
/// QR that *nearly* scans is worse than one that obviously does, because the
/// user retries instead of moving the phone.
pub fn readable_fg(bg: [u8; 3], preferred: [u8; 3]) -> [u8; 3] {
    if contrast(bg, preferred) >= MIN_CONTRAST { return preferred; }
    if luminance(bg) > 0.5 { [0, 0, 0] } else { [255, 255, 255] }
}

/// The module grid, row-major, `true` where a module is dark.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub modules: Vec<bool>,
}

impl Layout {
    pub fn get(&self, x: usize, y: usize) -> bool {
        if x >= MODULES || y >= MODULES { return false; }
        self.modules[y * MODULES + x]
    }

    /// Is this coordinate inside a finder pattern or its separator?
    ///
    /// Data must never be written here: the finders are what a scanner locates
    /// first, and a stray dark module beside one breaks detection outright
    /// rather than corrupting a byte.
    pub fn is_reserved(x: usize, y: usize) -> bool {
        let corner = |ox: usize, oy: usize| x >= ox && x < ox + 8 && y >= oy && y < oy + 8;
        corner(0, 0)
            || (x >= MODULES - 8 && y < 8)
            || (x < 8 && y >= MODULES - 8)
            // Timing rows.
            || x == 6 || y == 6
    }
}

/// Build the layout for an id.
///
/// Deterministic: one id always renders the same square, which is the property
/// that makes it usable as an identicon at all.
pub fn layout_for(id: &str) -> Layout {
    let mut m = vec![false; MODULES * MODULES];
    let mut set = |x: usize, y: usize, on: bool| {
        if x < MODULES && y < MODULES { m[y * MODULES + x] = on; }
    };

    // Three finder patterns: 7×7 dark, 5×5 light, 3×3 dark.
    for (ox, oy) in [(0, 0), (MODULES - 7, 0), (0, MODULES - 7)] {
        for dy in 0..7 {
            for dx in 0..7 {
                let ring = dx == 0 || dy == 0 || dx == 6 || dy == 6;
                let core = (2..=4).contains(&dx) && (2..=4).contains(&dy);
                set(ox + dx, oy + dy, ring || core);
            }
        }
    }

    // Timing patterns: alternating modules along row and column 6.
    for i in 8..MODULES - 8 {
        let on = i % 2 == 0;
        set(i, 6, on);
        set(6, i, on);
    }

    // Data field, derived from the id. Not an encoding — see the module docs.
    let bytes = id.as_bytes();
    if !bytes.is_empty() {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
        for y in 0..MODULES {
            for x in 0..MODULES {
                if Layout::is_reserved(x, y) { continue; }
                h ^= h << 13; h ^= h >> 7; h ^= h << 17;
                set(x, y, (h >> 33) & 1 == 1);
            }
        }
    }
    Layout { modules: m }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_is_a_whole_number_of_pixels() {
        // A fractional module resamples and blurs the edges — "visible as the
        // soft edges on the FP5 QR".
        for px in [64u32, 100, 220, 333] {
            for dpr in [1.0f32, 1.5, 2.0, 2.625, 3.0] {
                let c = cell_size(px, dpr);
                assert!(c >= MIN_CELL);
                assert_eq!(image_size(px, dpr) % c, 0, "image is a whole number of cells");
            }
        }
    }

    #[test]
    fn a_higher_pixel_ratio_renders_more_pixels() {
        // Drawing at logical size and letting the toolkit upscale is exactly
        // the blur the C++ names.
        assert!(cell_size(200, 3.0) > cell_size(200, 1.0));
    }

    #[test]
    fn a_tiny_request_still_produces_visible_modules() {
        // Below two pixels a module stops rendering as anything.
        assert_eq!(cell_size(10, 1.0), MIN_CELL);
        assert_eq!(cell_size(1, 1.0), MIN_CELL);
        assert_eq!(cell_size(0, 1.0), MIN_CELL, "zero does not divide to zero");
    }

    #[test]
    fn a_zero_or_negative_pixel_ratio_does_not_collapse_the_render() {
        // A platform reporting no DPR yet is reachable during startup.
        assert!(cell_size(200, 0.0) >= MIN_CELL);
        assert!(cell_size(200, -1.0) >= MIN_CELL);
    }

    #[test]
    fn a_low_contrast_theme_falls_back_to_black_or_white() {
        // A camera has to separate the modules, and a QR that NEARLY scans is
        // worse than one that obviously does — the user retries instead of
        // moving the phone.
        let dark_bg = [30, 32, 38];
        let dim_fg = [60, 64, 70];
        assert!(contrast(dark_bg, dim_fg) < MIN_CONTRAST);
        assert_eq!(readable_fg(dark_bg, dim_fg), [255, 255, 255]);

        let light_bg = [245, 245, 245];
        assert_eq!(readable_fg(light_bg, [200, 200, 200]), [0, 0, 0]);
    }

    #[test]
    fn a_theme_that_already_passes_keeps_its_own_colour() {
        // Overriding a palette that works would make every QR look foreign to
        // the app around it.
        let bg = [255, 255, 255];
        let fg = [20, 24, 30];
        assert!(contrast(bg, fg) >= MIN_CONTRAST);
        assert_eq!(readable_fg(bg, fg), fg);
    }

    #[test]
    fn the_contrast_ratio_matches_the_wcag_definition() {
        // Black on white is 21:1 exactly; a formula that is off here is off
        // everywhere.
        assert!((contrast([0, 0, 0], [255, 255, 255]) - 21.0).abs() < 0.01);
        assert!((contrast([255, 255, 255], [255, 255, 255]) - 1.0).abs() < 0.01);
    }

    #[test]
    fn contrast_is_symmetric() {
        let a = [12, 90, 200];
        let b = [240, 240, 230];
        assert!((contrast(a, b) - contrast(b, a)).abs() < 1e-4);
    }

    #[test]
    fn all_three_finder_patterns_are_present_and_correctly_shaped() {
        // These are what a scanner locates first; getting the ring-and-core
        // shape wrong means nothing detects the code at all.
        let l = layout_for("sc-abc12345");
        for (ox, oy) in [(0, 0), (MODULES - 7, 0), (0, MODULES - 7)] {
            assert!(l.get(ox, oy), "outer ring at {ox},{oy}");
            assert!(!l.get(ox + 1, oy + 1), "light ring");
            assert!(l.get(ox + 3, oy + 3), "dark core");
        }
    }

    #[test]
    fn the_fourth_corner_is_not_a_finder() {
        // Four finders is not a QR; a scanner uses the missing one to work out
        // the orientation.
        let l = layout_for("sc-abc12345");
        let (ox, oy) = (MODULES - 7, MODULES - 7);
        let ring_complete = (0..7).all(|d| l.get(ox + d, oy));
        assert!(!ring_complete, "bottom-right must not look like a finder");
    }

    #[test]
    fn data_never_lands_in_a_reserved_area() {
        // A stray dark module beside a finder breaks detection outright rather
        // than corrupting a byte.
        for id in ["sc-00000000", "sc-ffffffff", "sc-abc12345"] {
            let l = layout_for(id);
            for (ox, oy) in [(0usize, 0usize), (MODULES - 7, 0), (0, MODULES - 7)] {
                // The separator ring just outside each finder stays light.
                if ox + 7 < MODULES { assert!(!l.get(ox + 7, oy), "{id}: separator"); }
                if oy + 7 < MODULES { assert!(!l.get(ox, oy + 7), "{id}: separator"); }
            }
        }
    }

    #[test]
    fn timing_patterns_alternate() {
        // The row a scanner uses to measure module size.
        let l = layout_for("sc-abc12345");
        for i in 8..MODULES - 8 {
            assert_eq!(l.get(i, 6), i % 2 == 0, "timing row at {i}");
            assert_eq!(l.get(6, i), i % 2 == 0, "timing column at {i}");
        }
    }

    #[test]
    fn one_id_always_renders_the_same_square() {
        // The property that makes it usable as an identicon at all.
        assert_eq!(layout_for("sc-abc12345"), layout_for("sc-abc12345"));
    }

    #[test]
    fn different_ids_render_differently() {
        // Two contacts sharing a square is worse than no square.
        let a = layout_for("sc-abc12345");
        let b = layout_for("sc-abc12346");
        assert_ne!(a, b, "one character apart must differ");
    }

    #[test]
    fn an_empty_id_still_produces_a_valid_frame() {
        // Reachable before an identity exists; the finders should render so the
        // placeholder looks like the real thing rather than an empty box.
        let l = layout_for("");
        assert!(l.get(0, 0));
        assert!(l.get(3, 3));
    }

    #[test]
    fn reading_outside_the_grid_returns_light_rather_than_panicking() {
        let l = layout_for("sc-abc12345");
        assert!(!l.get(MODULES, 0));
        assert!(!l.get(0, MODULES + 100));
    }
}
