//! The sun over a day: brightness, warmth, ambient fill.
//!
//! Lives HERE, in a crate with no dependencies, because three separate places
//! need it and no two of them can reasonably depend on each other:
//!
//! * `uikit_egui::lux` grades palette colours by time of day, so the panels
//!   change as the day does;
//! * `scene_kit::mesh3d` lights the 3D scene;
//! * `logscope_egui::mesh3d` drives that scene in the app.
//!
//! Before this there were three copies of the curve. That is not a tidiness
//! complaint: the whole reason the panels were taught to track the clock was
//! so the room behind the glass and the glass itself would not disagree about
//! the time of day, and a second copy re-creates exactly the disagreement the
//! change was made to remove. Putting a UI crate in scene-kit's dependency
//! list to share one function would be the wrong trade; putting nine lines of
//! trigonometry in the dependency-free maths crate is not.
//!
//! Hour-parameterised rather than clock-reading, so it is testable and so a
//! caller can drive it from a scrubber — a demo, a screenshot set, "show me
//! this panel at 3am" — instead of only from `now`.

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

/// The sky at local hour `h` (0..24, fractional). Wraps, so 25.0 is 1.0.
pub fn sky_at(h: f32) -> Sky {
    let h = h.rem_euclid(24.0);
    let elev = (core::f32::consts::PI * ((h - 6.0) / 12.0)).sin();
    let day = elev.max(0.0);
    let low_sun = (1.0 - elev.abs()).clamp(0.0, 1.0);
    Sky {
        strength: (0.12 + 0.88 * day).clamp(0.10, 1.0),
        warmth: if elev > 0.04 { low_sun * 0.85 } else { -0.55 * (1.0 - day) },
        ambient: 0.56 + 0.62 * day,
    }
}

/// The same, as the `(strength, warmth, ambient)` tuple the scene renderers
/// already pass around. Kept so migrating them is a one-line change rather
/// than a refactor of every call site.
pub fn sun_clock_at(h: f32) -> (f32, f32, f32) {
    let s = sky_at(h);
    (s.strength, s.warmth, s.ambient)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noon_is_bright_and_neutral_dawn_and_dusk_are_warm() {
        let noon = sky_at(12.0);
        assert!(noon.strength > 0.9, "{noon:?}");
        assert!(noon.warmth.abs() < 0.2, "noon should be near-neutral: {noon:?}");
        // The golden-hour shape: warmest when the sun is LOW but up, which is
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
    fn the_tuple_form_is_the_struct_form() {
        // The two exist only so scene renderers need not change their call
        // shape; if they ever disagree the whole point is lost.
        for h in [0.0_f32, 6.5, 12.0, 19.0] {
            let s = sky_at(h);
            assert_eq!(sun_clock_at(h), (s.strength, s.warmth, s.ambient));
        }
    }
}
