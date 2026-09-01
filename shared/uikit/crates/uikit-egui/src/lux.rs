//! Making a panel read as a worked stone surface rather than a filled rect.
//!
//! The brief: interactive stone in a chic mansion, luxurious rather than dark.
//! Looking at what separates the reference renders from the shipping app, the
//! gap is almost never colour — it is that real surfaces respond to a light
//! that has a POSITION, and flat UI does not. A bevel drawn with a fixed lit
//! edge and a fixed shadow edge looks embossed from any angle, which is to say
//! it looks printed.
//!
//! Three pure pieces here, each the geometry/maths half of an effect whose
//! painting belongs to the caller:
//!
//! * [`sheen`] — where the specular highlight sits and how strong it is, given
//!   a pointer. This is the "interactive" in interactive stone: polished
//!   marble under a lamp changes as you move past it, and the cheapest
//!   convincing version of that is a highlight that tracks the cursor and
//!   falls off with distance.
//! * [`bracket`] — the corner inlay the reference uses. Brackets rather than a
//!   full border because a continuous rule reads as a box, while four corner
//!   pieces read as hardware set INTO a surface.
//! * [`Weight`] — how long a thing of a given size should take to move.
//!   Uniform durations are the other tell: in the physical world a heavy panel
//!   and a small chip do not settle at the same speed, and matching them makes
//!   everything feel like the same weightless material.

/// Where the highlight is and how bright, in `[0,1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sheen {
    /// Position along the surface, 0 = left edge, 1 = right edge. Clamped, so
    /// a pointer beyond the panel parks the highlight at the near edge rather
    /// than sliding off into nothing.
    pub pos: f32,
    /// 0 when the pointer is far away, 1 directly over the surface.
    pub intensity: f32,
}

/// Specular response for a panel of width `w` with the pointer at `px`
/// (surface-relative) and `dist` away from the surface in points.
///
/// `falloff` is the distance at which the highlight has faded to nothing.
/// Generous by default in the caller — a highlight that vanishes the instant
/// the pointer leaves the panel snaps, and snapping is what makes a surface
/// read as a hover state rather than as a lit object.
pub fn sheen(px: f32, w: f32, dist: f32, falloff: f32) -> Sheen {
    let w = w.max(1.0);
    let pos = (px / w).clamp(0.0, 1.0);
    let f = falloff.max(1.0);
    // Smoothstep rather than linear: a linear falloff has a visible corner at
    // the moment it reaches zero, and the eye catches it on a slow pass.
    let x = (1.0 - (dist.max(0.0) / f)).clamp(0.0, 1.0);
    Sheen { pos, intensity: x * x * (3.0 - 2.0 * x) }
}

/// The two segments of one corner bracket, as offsets from the corner.
///
/// Returned as lengths rather than points so the caller places them at
/// whichever corner it is drawing without this module knowing about rects.
/// `len` scales with the panel so a large frame does not get jewellery sized
/// for a chip — which is the mistake that makes an ornament look pasted on.
pub fn bracket(rect_w: f32, rect_h: f32) -> f32 {
    let small = rect_w.min(rect_h);
    // 12% of the short side, bounded. Below ~8pt a bracket reads as a stray
    // pixel; above ~28 it stops being a corner and becomes a border.
    (small * 0.12).clamp(8.0, 28.0)
}

/// How heavy a surface is, which decides how long it takes to settle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    /// A chip, a toggle, a tag. Responds almost immediately.
    Light,
    /// A card or a row.
    Medium,
    /// A whole panel or a page. Visibly has mass.
    Heavy,
}

impl Weight {
    /// Pick from a size, so a caller does not have to classify by hand and
    /// then drift.
    pub fn of(area_pt2: f32) -> Weight {
        if area_pt2 < 4_000.0 {
            Weight::Light
        } else if area_pt2 < 60_000.0 {
            Weight::Medium
        } else {
            Weight::Heavy
        }
    }

    /// Seconds to settle, before the user's animation-speed preference is
    /// applied. The caller multiplies by `style.animation_time`'s scale so the
    /// Reduced setting still flattens everything to instant.
    pub fn secs(self) -> f32 {
        match self {
            Weight::Light => 0.09,
            Weight::Medium => 0.16,
            Weight::Heavy => 0.26,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_highlight_tracks_the_pointer_across_the_surface() {
        assert!(sheen(0.0, 100.0, 0.0, 40.0).pos < 0.01);
        assert!((sheen(50.0, 100.0, 0.0, 40.0).pos - 0.5).abs() < 0.01);
        assert!(sheen(100.0, 100.0, 0.0, 40.0).pos > 0.99);
    }

    #[test]
    fn a_pointer_past_the_edge_parks_at_the_edge_rather_than_sliding_away() {
        // Otherwise the highlight keeps travelling off-surface and the panel
        // goes dark while the cursor is still right beside it.
        assert_eq!(sheen(-50.0, 100.0, 0.0, 40.0).pos, 0.0);
        assert_eq!(sheen(500.0, 100.0, 0.0, 40.0).pos, 1.0);
    }

    #[test]
    fn intensity_is_full_on_the_surface_and_zero_past_the_falloff() {
        assert!((sheen(50.0, 100.0, 0.0, 40.0).intensity - 1.0).abs() < 1e-6);
        assert_eq!(sheen(50.0, 100.0, 40.0, 40.0).intensity, 0.0);
        assert_eq!(sheen(50.0, 100.0, 999.0, 40.0).intensity, 0.0);
    }

    #[test]
    fn the_falloff_has_no_corner_at_either_end() {
        // A linear ramp reaches zero with a visible kink and the eye catches it
        // on a slow pass. Smoothstep's derivative is zero at both ends, which is
        // the property that makes the fade read as light rather than as a
        // fading rectangle. Check the curve is flatter near the ends than in
        // the middle.
        let at = |d: f32| sheen(50.0, 100.0, d, 40.0).intensity;
        let near_end = (at(1.0) - at(3.0)).abs();
        let middle = (at(19.0) - at(21.0)).abs();
        assert!(middle > near_end * 2.0,
            "expected a soft shoulder: end delta {near_end}, middle delta {middle}");
    }

    #[test]
    fn intensity_never_leaves_the_unit_range() {
        for d in [-10.0, 0.0, 5.0, 39.9, 40.0, 1000.0] {
            let i = sheen(50.0, 100.0, d, 40.0).intensity;
            assert!((0.0..=1.0).contains(&i), "dist {d} gave {i}");
        }
    }

    #[test]
    fn a_zero_width_surface_does_not_divide_by_zero() {
        let s = sheen(10.0, 0.0, 0.0, 40.0);
        assert!(s.pos.is_finite() && s.intensity.is_finite());
    }

    #[test]
    fn bracket_scales_with_the_panel_but_stays_in_jewellery_range() {
        // Sized off the SHORT side, so a wide thin bar does not get a bracket
        // taller than itself.
        assert_eq!(bracket(40.0, 40.0), 8.0, "tiny panels get the floor");
        assert!((bracket(200.0, 200.0) - 24.0).abs() < 0.01);
        assert_eq!(bracket(4000.0, 3000.0), 28.0, "huge panels get the cap, not a border");
        assert_eq!(bracket(1000.0, 30.0), 8.0, "a thin bar is sized by its short side");
    }

    #[test]
    fn heavier_surfaces_settle_more_slowly() {
        // The uniform-duration tell: if a chip and a page take the same time,
        // nothing has mass.
        assert!(Weight::Light.secs() < Weight::Medium.secs());
        assert!(Weight::Medium.secs() < Weight::Heavy.secs());
        // And the spread must be perceptible — under about 1.5x nobody sees it.
        assert!(Weight::Heavy.secs() > Weight::Light.secs() * 2.0);
    }

    #[test]
    fn weight_is_classified_from_area_so_callers_cannot_drift() {
        assert_eq!(Weight::of(24.0 * 24.0), Weight::Light);     // a toggle
        assert_eq!(Weight::of(320.0 * 64.0), Weight::Medium);   // a row
        assert_eq!(Weight::of(900.0 * 700.0), Weight::Heavy);   // a page
    }

    #[test]
    fn every_duration_is_short_enough_to_feel_responsive() {
        // Luxury is not slowness. Past ~300ms an interface stops feeling
        // expensive and starts feeling laggy, which is the opposite failure.
        for w in [Weight::Light, Weight::Medium, Weight::Heavy] {
            assert!(w.secs() < 0.30, "{w:?} at {}s is sluggish", w.secs());
        }
    }
}
