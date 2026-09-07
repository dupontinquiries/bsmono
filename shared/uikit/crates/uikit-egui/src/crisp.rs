//! crisp — text and edges that land on the pixel grid.
//!
//! Reported on a desktop: rusty-board's key labels look tall and blurry. Two causes, both
//! from sizes computed as a fraction of a box rather than chosen:
//!
//! **A fractional font size.** A cap font of `key_height * 0.42` is 18.48 pt on a 44 pt key,
//! and at a 1.25 or 1.5 scale factor that is 23.1 or 27.72 device pixels. A glyph rasterised
//! at 27.72 px has no whole-pixel stem anywhere in it, so every vertical looks soft — and
//! because the softness is asymmetric between the x and y axes of the hinting, the result
//! reads as slightly TALL as well as blurry. egui also keys its glyph atlas on the exact
//! size, so a board whose keys change height by a pixel re-rasterises the whole alphabet.
//!
//! **A fractional rect.** A 1 px border drawn at x = 40.3 covers 70% of one column and 30%
//! of the next: two grey columns instead of one dark line. On a keyboard, where every key
//! has a border, that is the whole surface looking smeared.
//!
//! Both are one idea: pick sizes and positions that are whole numbers of DEVICE pixels, and
//! convert back to points. Pure arithmetic — no egui state — so it is tested directly.

use egui::{Pos2, Rect, Vec2};

/// The largest font size not exceeding `pt` whose height is a whole number of device pixels.
///
/// Rounds DOWN rather than to nearest: a cap font is sized to fit inside a key, and rounding
/// up can push a two-line label or a wide glyph past the edge it was measured against.
/// Never returns less than 1 device pixel.
pub fn snap_font_pt(pt: f32, pixels_per_point: f32) -> f32 {
    let ppp = pixels_per_point.max(0.01);
    let px = (pt * ppp).floor().max(1.0);
    px / ppp
}

/// Snap a length (a gutter, a radius) to whole device pixels, to nearest.
pub fn snap_len(len: f32, pixels_per_point: f32) -> f32 {
    let ppp = pixels_per_point.max(0.01);
    (len * ppp).round() / ppp
}

/// Move a rect's edges onto the device-pixel grid, keeping its size within a pixel.
///
/// The corners are rounded independently so adjacent rects that shared an edge still do —
/// snapping position and size separately can leave a one-pixel gap or overlap between
/// neighbouring keys, which is a different visible fault from the one being fixed.
pub fn snap_rect(rect: Rect, pixels_per_point: f32) -> Rect {
    let ppp = pixels_per_point.max(0.01);
    let snap = |v: f32| (v * ppp).round() / ppp;
    Rect::from_min_max(
        Pos2::new(snap(rect.min.x), snap(rect.min.y)),
        Pos2::new(snap(rect.max.x), snap(rect.max.y)),
    )
}

/// Snap a size the same way.
pub fn snap_size(size: Vec2, pixels_per_point: f32) -> Vec2 {
    Vec2::new(snap_len(size.x, pixels_per_point), snap_len(size.y, pixels_per_point))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_whole_px(v: f32, ppp: f32) -> bool {
        let px = v * ppp;
        (px - px.round()).abs() < 1e-3
    }

    #[test]
    fn a_snapped_font_is_a_whole_number_of_device_pixels() {
        for ppp in [1.0f32, 1.25, 1.5, 2.0, 2.75, 3.0] {
            for pt in [11.0f32, 13.3, 18.48, 22.0, 26.9] {
                let s = snap_font_pt(pt, ppp);
                assert!(is_whole_px(s, ppp), "pt {pt} at {ppp}x → {s} is not whole pixels");
                assert!(s <= pt + 1e-3, "snapping grew the font: {pt} → {s}");
                assert!(s > pt - 1.0 / ppp - 1e-3, "snapping lost more than a pixel: {pt} → {s}");
            }
        }
        // never smaller than one device pixel, however absurd the request
        assert!(snap_font_pt(0.1, 3.0) * 3.0 >= 0.999);
    }

    /// The reported case: a 44 pt key at 1.5× gives an 18.48 pt cap font — 27.72 device
    /// pixels, which has no whole-pixel stem in it.
    #[test]
    fn the_reported_cap_font_lands_on_the_grid() {
        let raw = 44.0 * 0.42;
        assert!(!is_whole_px(raw, 1.5), "the bug premise: {raw} at 1.5x was already whole");
        let snapped = snap_font_pt(raw, 1.5);
        assert!(is_whole_px(snapped, 1.5));
        assert!((snapped - raw).abs() < 0.7, "the cap barely changes size: {raw} → {snapped}");
    }

    #[test]
    fn neighbouring_rects_still_share_their_edge_after_snapping() {
        let ppp = 1.5;
        // two keys side by side at fractional positions, touching exactly
        let a = Rect::from_min_size(Pos2::new(10.3, 20.7), Vec2::new(30.4, 44.0));
        let b = Rect::from_min_size(Pos2::new(a.max.x, 20.7), Vec2::new(30.4, 44.0));
        let (sa, sb) = (snap_rect(a, ppp), snap_rect(b, ppp));
        assert_eq!(sa.max.x, sb.min.x, "a gap or an overlap opened between two keys");
        for v in [sa.min.x, sa.max.x, sa.min.y, sa.max.y, sb.max.x] {
            assert!(is_whole_px(v, ppp), "{v} is off the grid");
        }
        // and nothing moved by more than a pixel
        assert!((sa.min.x - a.min.x).abs() <= 1.0 / ppp);
        assert!((sa.height() - a.height()).abs() <= 1.0 / ppp);
    }

    #[test]
    fn at_one_x_snapping_is_plain_rounding_and_a_whole_size_is_untouched() {
        assert_eq!(snap_font_pt(18.0, 1.0), 18.0);
        assert_eq!(snap_font_pt(18.7, 1.0), 18.0);
        assert_eq!(snap_len(4.4, 1.0), 4.0);
        assert_eq!(snap_len(4.6, 1.0), 5.0);
        let r = Rect::from_min_size(Pos2::new(8.0, 16.0), Vec2::new(40.0, 44.0));
        assert_eq!(snap_rect(r, 2.0), r);
    }
}
