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

#[cfg(test)]
mod integration_notes {
    //! Properties the wiring depends on, checked here so a change to `Weight`
    //! cannot quietly break `anim_secs_for`.

    use super::*;

    #[test]
    fn the_weight_ladder_brackets_the_old_uniform_duration() {
        // Everything used to animate at BASE_ANIM_SECS (0.15s). The point of a
        // weight model is that the old value now sits INSIDE the range rather
        // than being every value — light is quicker, heavy is slower. If all
        // three landed on one side, this would just be a global speed change
        // wearing a new name.
        const OLD_UNIFORM: f32 = 0.15;
        assert!(Weight::Light.secs() < OLD_UNIFORM);
        assert!(Weight::Heavy.secs() > OLD_UNIFORM);
        assert!((Weight::Medium.secs() - OLD_UNIFORM).abs() < 0.05,
            "medium should stay near the value everything already had, so most \
             surfaces feel unchanged");
    }
}

// ── time of day ──────────────────────────────────────────────────────────────
//
// logscope's 3D scene already tracks the real local clock (`mesh3d::sun_clock`)
// — a warm low sun at dawn and dusk, neutral at noon, cool and dim at night.
// The 2D panels do not, so the room behind the glass changes through the day
// and the glass itself does not, which is the single most obvious tell that
// they are two unrelated renderers rather than one place.
//
// This is the same model, extracted so both can share it. Pure and
// hour-parameterised rather than clock-reading, so it is testable and so a
// caller can drive it from a scrubber (a demo, a screenshot set, "show me this
// panel at 3am") instead of only from `now`.

/// The light at a moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sky {
    /// Sun brightness, 0.1 (night) to 1.0 (noon).
    pub strength: f32,
    /// -1 cool to +1 warm. Peaks near sunrise and sunset, neutral at noon,
    /// negative at night — the golden-hour curve, which is NOT the same as
    /// "warm during the day".
    pub warmth: f32,
    /// Ambient fill multiplier.
    pub ambient: f32,
}

/// The sky at local hour `h` (0..24, fractional).
///
/// Matches `mesh3d::sun_clock`'s curve exactly so the panels and the scene
/// cannot drift apart; that function should delegate here rather than keep a
/// second copy.
pub fn sky_at(h: f32) -> Sky {
    let h = h.rem_euclid(24.0);
    let elev = (std::f32::consts::PI * ((h - 6.0) / 12.0)).sin();
    let day = elev.max(0.0);
    let low_sun = (1.0 - elev.abs()).clamp(0.0, 1.0);
    Sky {
        strength: (0.12 + 0.88 * day).clamp(0.10, 1.0),
        warmth: if elev > 0.04 { low_sun * 0.85 } else { -0.55 * (1.0 - day) },
        ambient: 0.56 + 0.62 * day,
    }
}

/// Grade one RGB colour by the sky.
///
/// `amount` in `[0,1]` is how much of the effect to apply — 0 is the palette
/// untouched. It exists because time-of-day grading must be DEFEATABLE: a
/// palette is a deliberate choice, and an operator who picked Travertine and
/// gets muddy blue Travertine at 2am will conclude the theme is broken rather
/// than that it is 2am. The default should be subtle and the setting should
/// reach zero.
///
/// Deliberately a channel tilt plus a luminance nudge rather than a hue
/// rotation: rotating hue moves an accent off the palette's identity (gold
/// becomes green), while tilting red against blue keeps the colour recognisably
/// itself and just changes the light falling on it — which is the physical
/// thing being modelled.
pub fn grade(rgb: [u8; 3], sky: Sky, amount: f32) -> [u8; 3] {
    let a = amount.clamp(0.0, 1.0);
    if a == 0.0 {
        return rgb;
    }
    // Warm tilts red up and blue down; cool does the reverse. Bounded so a
    // fully warm sky can never blow a channel out on its own.
    let tilt = sky.warmth.clamp(-1.0, 1.0) * 0.10 * a;
    // Night dims, noon leaves alone. Never brightens past the palette's own
    // value: a theme author picked those numbers and the clock is not entitled
    // to overrule them upward.
    let lum = 1.0 - (1.0 - sky.strength.clamp(0.0, 1.0)) * 0.18 * a;
    let ch = |v: u8, t: f32| -> u8 {
        let x = v as f32 / 255.0;
        ((x * (1.0 + t) * lum).clamp(0.0, 1.0) * 255.0).round() as u8
    };
    [ch(rgb[0], tilt), ch(rgb[1], tilt * 0.25), ch(rgb[2], -tilt)]
}

#[cfg(test)]
mod sky_tests {
    use super::*;

    #[test]
    fn noon_is_bright_and_neutral_dawn_and_dusk_are_warm() {
        let noon = sky_at(12.0);
        assert!(noon.strength > 0.9, "{noon:?}");
        assert!(noon.warmth.abs() < 0.2, "noon should be near-neutral: {noon:?}");
        // The golden-hour curve: warmest when the sun is LOW but up, which is
        // not the same as "warm all day".
        assert!(sky_at(6.5).warmth > noon.warmth);
        assert!(sky_at(17.5).warmth > noon.warmth);
    }

    #[test]
    fn night_is_dim_and_cool() {
        let n = sky_at(2.0);
        assert!(n.strength <= 0.15, "{n:?}");
        assert!(n.warmth < 0.0, "night must go cool, not merely dark: {n:?}");
        assert!(n.ambient < sky_at(12.0).ambient);
    }

    #[test]
    fn the_curve_wraps_and_never_produces_nonsense() {
        for h in [-6.0, 0.0, 5.9, 12.0, 23.9, 30.0] {
            let s = sky_at(h);
            assert!(s.strength.is_finite() && (0.0..=1.0).contains(&s.strength), "h={h} {s:?}");
            assert!((-1.0..=1.0).contains(&s.warmth), "h={h} {s:?}");
            assert!(s.ambient > 0.0, "h={h} {s:?}");
        }
        assert_eq!(sky_at(1.0), sky_at(25.0), "the day must wrap");
    }

    #[test]
    fn amount_zero_leaves_the_palette_exactly_alone() {
        // The defeat switch has to be exact, not approximately exact — a user
        // who turns this off and still sees drift will not trust the setting.
        let c = [0xC8, 0xA0, 0x4B];
        assert_eq!(grade(c, sky_at(3.0), 0.0), c);
        assert_eq!(grade(c, sky_at(12.0), 0.0), c);
    }

    #[test]
    fn grading_never_brightens_past_the_authors_colour() {
        // A theme author picked these values; the clock may dim them, not
        // overrule them upward.
        let c = [0x80, 0x80, 0x80];
        for h in [0.0, 6.0, 12.0, 18.0, 22.0] {
            let g = grade(c, sky_at(h), 1.0);
            let lum = |v: [u8; 3]| v[0] as u32 + v[1] as u32 + v[2] as u32;
            assert!(lum(g) <= lum(c) + 8, "h={h} brightened {c:?} -> {g:?}");
        }
    }

    #[test]
    fn a_warm_sky_tilts_red_up_and_blue_down_without_rotating_hue() {
        // Gold must stay gold. A hue rotation would move it off the palette's
        // identity, which is why this is a channel tilt.
        //
        // Asserted first as "red must not fall in warm light", which failed —
        // and the code was right. Dawn is warm AND DIM: strength is low just
        // after sunrise, so the luminance term pulls every channel down and
        // absolute red drops even as the light warms. The physical property is
        // RELATIVE — warm light moves the red:blue ratio up — and comparing
        // against noon is how you see it without the brightness confound.
        let gold = [0xC8, 0xA0, 0x4B];
        let dawn = grade(gold, sky_at(6.5), 1.0);
        let noon = grade(gold, sky_at(12.0), 1.0);
        let ratio = |c: [u8; 3]| c[0] as f32 / (c[2] as f32).max(1.0);
        assert!(ratio(dawn) > ratio(noon),
            "warm light must raise red against blue: dawn {dawn:?} noon {noon:?}");
        // still recognisably gold: red > green > blue
        assert!(dawn[0] > dawn[1] && dawn[1] > dawn[2], "{dawn:?} is no longer gold");
    }

    #[test]
    fn no_channel_can_overflow_or_wrap() {
        for c in [[0xFF, 0xFF, 0xFF], [0x00, 0x00, 0x00], [0xFF, 0x00, 0x80]] {
            for h in [0.0, 6.0, 12.0, 18.0] {
                let g = grade(c, sky_at(h), 1.0);
                // u8 cannot exceed 255, but a wrap would show as a dark result
                // for a white input — check the direction is sane.
                if c == [0xFF, 0xFF, 0xFF] {
                    assert!(g[0] > 150 && g[1] > 150, "white went dark at h={h}: {g:?}");
                }
            }
        }
    }
}
