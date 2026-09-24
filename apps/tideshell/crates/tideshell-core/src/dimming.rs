//! Going darker than the panel will go.
//!
//! A backlight has a floor: one raw unit, below which the only other value
//! is off. On an OLED phone that floor is still too bright to read in a dark
//! room, and the bottom of the slider is where the steps between levels are
//! coarsest — the difference between raw 1 and raw 2 can be a doubling.
//!
//! Below that, brightness has to come from the PIXELS. Scaling what is drawn
//! toward black ("reduced white point") multiplies with the backlight, so
//! the two together reach far lower than the panel alone, and the slider
//! gets a long fine-grained tail exactly where it was previously stuck.
//!
//! ## Why both move together
//!
//! The obvious arrangement — pin the backlight at its floor and ramp only
//! the pixels — puts a step in the middle of the slider, because the
//! perceived level just above the crossover is the floor's, not the
//! crossover's. So instead the backlight keeps falling to its floor AND the
//! white point starts closing in, over the same span. At the crossover the
//! gain is exactly 1, so there is nothing to see; below it the two multiply.
//!
//! ## What this is not
//!
//! Not a substitute for a real low-brightness panel mode, and not colour
//! management: scaling toward black loses bit depth, so deep in the tail the
//! darkest greys band. That is a fair trade against a screen that is
//! painful to look at, but it is why it is off by default and lives under
//! accessibility rather than in the main brightness settings.

/// Below this slider position the white point starts closing.
pub const DIGITAL_BELOW: f32 = 0.05;

/// The gain at the very bottom: the screen ends up at about a tenth of what
/// one backlight unit alone produces.
pub const MIN_GAIN: f32 = 0.10;

/// How a slider position is split between the panel and the pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dim {
    /// What to ask the backlight for, 0..1. Zero still means the hardware
    /// floor, not off: the backlight write clamps to one raw unit.
    pub backlight: f32,
    /// What to multiply drawn pixels by, `MIN_GAIN`..1.
    pub gain: f32,
}

impl Dim {
    /// The alpha of a black veil that produces `gain`.
    pub fn veil_alpha(self) -> f32 {
        (1.0 - self.gain).clamp(0.0, 1.0)
    }
    pub fn is_dimming(self) -> bool {
        self.gain < 1.0
    }
}

/// Split a slider position. With `enabled` false this is the old behaviour
/// exactly: the panel does everything and the gain is 1.
pub fn split(slider: f32, enabled: bool) -> Dim {
    // clamp() propagates NaN, and a NaN would be written to the backlight as
    // a raw value. Treat a nonsense slider as the floor: dark is recoverable
    // by dragging up, an unwritable value is not.
    let s = if slider.is_finite() { slider.clamp(0.0, 1.0) } else { 0.0 };
    if !enabled || s >= DIGITAL_BELOW {
        return Dim { backlight: s, gain: 1.0 };
    }
    let t = s / DIGITAL_BELOW; // 0 at the bottom, 1 at the crossover
    Dim { backlight: s, gain: MIN_GAIN + (1.0 - MIN_GAIN) * t }
}

/// Roughly what the eye gets, for comparing two positions. The panel's own
/// response is already gamma-corrected where it is written, so this is just
/// the product.
pub fn effective(d: Dim) -> f32 {
    d.backlight.max(0.0) * d.gain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default_changes_nothing() {
        for i in 0..=20 {
            let s = i as f32 / 20.0;
            let d = split(s, false);
            assert_eq!(d.backlight, s);
            assert_eq!(d.gain, 1.0);
            assert!(!d.is_dimming());
            assert_eq!(d.veil_alpha(), 0.0);
        }
    }

    #[test]
    fn nothing_visible_happens_at_the_crossover() {
        // A step in the middle of the slider is the failure mode this design
        // exists to avoid, so the gain must reach exactly 1 there.
        let at = split(DIGITAL_BELOW, true);
        assert_eq!(at.gain, 1.0);
        let just_below = split(DIGITAL_BELOW - 0.0005, true);
        assert!((just_below.gain - 1.0).abs() < 0.02, "gain {} jumps at the crossover", just_below.gain);
        assert!((effective(just_below) - effective(at)).abs() < 0.001);
        // And above it, the panel is still doing all the work.
        assert_eq!(split(0.5, true).gain, 1.0);
    }

    #[test]
    fn the_tail_is_monotonic_and_reaches_about_a_tenth() {
        let mut prev = f32::MAX;
        for i in (0..=50).rev() {
            let s = i as f32 / 1000.0; // 0 .. 0.05, the tail
            let e = effective(split(s, true));
            assert!(e <= prev + 1e-6, "brightness went UP while dragging down at {s}");
            prev = e;
        }
        // At the very bottom the panel is at its floor and the pixels carry
        // the rest: about a tenth of one unit.
        let bottom = split(0.0, true);
        assert_eq!(bottom.backlight, 0.0, "still the hardware floor, never off");
        assert!((bottom.gain - MIN_GAIN).abs() < 1e-6);
        assert!(bottom.veil_alpha() > 0.85, "a veil that is nearly opaque, {}", bottom.veil_alpha());
    }

    #[test]
    fn the_extra_range_is_finer_than_the_panel_alone() {
        // The point of the tail: several distinguishable steps where the
        // panel offers one.
        let steps: Vec<f32> = (0..=10).map(|i| effective(split(i as f32 / 200.0, true))).collect();
        for w in steps.windows(2) {
            assert!(w[1] > w[0], "two positions in the tail that look the same: {steps:?}");
        }
        // Enabled and disabled agree everywhere above the crossover.
        for i in 5..=20 {
            let s = i as f32 / 20.0;
            assert_eq!(split(s, true), split(s, false));
        }
    }

    #[test]
    fn a_slider_outside_its_range_is_clamped_not_wrapped() {
        assert_eq!(split(-1.0, true).gain, MIN_GAIN);
        assert_eq!(split(2.0, true), Dim { backlight: 1.0, gain: 1.0 });
        assert_eq!(split(f32::NAN, true).backlight.is_nan(), false, "NaN must not reach the panel");
    }
}
