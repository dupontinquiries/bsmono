//! Value structure: whether a map has darks to be deliberate ABOUT.
//!
//! "Washed out" is not a taste judgement, it is a measurement. A frame reads
//! flat when its major surfaces — ground, horizon, sky — sit at nearly the
//! same luminance, and no exposure curve, tonemap or grade recovers contrast
//! that was never in the scene. Grading a flat scene just moves the flat.
//!
//! So this module measures the three key values of a `MapStyle` and reports
//! the spread, plus the two structural facts that separate a photographed
//! world from a toy one:
//!
//! ## Measure it in STOPS, not in linear difference
//!
//! The first version of this module subtracted linear luminances, and its
//! own tests rejected it — correctly. Vision is roughly logarithmic, so a
//! linear difference declares every dark scene flat and every bright scene
//! structured, which is precisely backwards for what a moody map needs.
//!
//! OBSERVATORY has a ground at 0.011 and a zenith at 0.001: a linear gap of
//! one hundredth, and a RATIO of ten — better than three stops, plainly
//! separated to the eye. MEADOW has ground 0.456, horizon 0.606, sky 0.405:
//! a large linear gap and barely half a stop, which is the pale, flat frame
//! in the recording. Ratio is the metric that agrees with looking at it.
//!
//! The point is not to force every map dark. `AURORA` and `ZEN GARDEN`
//! should be bright and open; `CRYPT` should not. The point is that a map
//! must have SEPARATION, whatever register it sits in — which is what makes
//! a dark map read as mystery rather than as mud, and a bright one read as
//! air rather than as fog.

use crate::world::MapStyle;

/// Rec. 709 relative luminance. The same weights `shader.wgsl`'s FXAA uses
/// for its edge detect, so "bright" means the same thing in both places.
pub fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// The three values a frame is built from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Values {
    pub ground: f32,
    pub horizon: f32,
    pub sky: f32,
}

impl Values {
    pub fn of(s: &MapStyle) -> Values {
        Values {
            ground: luma([s.grass[0], s.grass[1], s.grass[2]]),
            horizon: luma(s.fog),
            sky: luma(s.zenith),
        }
    }

    /// Separation between the lightest and darkest of the three, in STOPS
    /// (log2 of their ratio). This is the number that decides whether a
    /// frame has a picture in it.
    ///
    /// Floored at `BLACK` before the ratio: a zenith of exactly 0 would
    /// otherwise report infinite structure, and a scene is not well composed
    /// merely because part of it is absolute black.
    pub fn stops(&self) -> f32 {
        let hi = self.ground.max(self.horizon).max(self.sky).max(BLACK);
        let lo = self.ground.min(self.horizon).min(self.sky).max(BLACK);
        (hi / lo).log2()
    }
}

/// The darkest value treated as distinguishable. Below this a display is
/// showing black and the difference is not a difference.
pub const BLACK: f32 = 0.0015;

/// Below this a map has no picture in it: its ground, horizon and sky are
/// close enough in value that the frame reads as one wash. Chosen from the
/// shipped table rather than invented — see the test, which reports every
/// map's spread so the number can be re-derived if the table changes.
pub const MIN_STOPS: f32 = 0.8;

/// A one-line audit row, for a dev tool or a test failure message.
pub fn report(s: &MapStyle) -> String {
    let v = Values::of(s);
    format!(
        "{:<14} ground {:.4}  horizon {:.4}  sky {:.4}  = {:.2} stops",
        s.name, v.ground, v.horizon, v.sky, v.stops()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::styles;

    #[test]
    fn luma_orders_the_obvious_cases() {
        assert!(luma([0.0; 3]) < luma([0.5; 3]));
        assert!(luma([0.5; 3]) < luma([1.0; 3]));
        assert!((luma([1.0; 3]) - 1.0).abs() < 1e-5);
        // green carries most of the luminance, blue almost none
        assert!(luma([0.0, 1.0, 0.0]) > luma([1.0, 0.0, 0.0]));
        assert!(luma([1.0, 0.0, 0.0]) > luma([0.0, 0.0, 1.0]));
    }

    #[test]
    fn stops_is_a_ratio_not_a_difference() {
        // The bug this module was born with. A dark scene with a 10:1 ratio
        // is well separated; a bright scene with a 1.5:1 ratio is a wash.
        // A linear metric ranks them the other way round, which is why the
        // first version of this file called OBSERVATORY flat and MEADOW the
        // best-structured map in the table.
        let dark = Values { ground: 0.011, horizon: 0.027, sky: 0.0015 };
        let pale = Values { ground: 0.456, horizon: 0.606, sky: 0.405 };
        assert!(dark.stops() > 3.0, "a 10:1 dark scene measured {:.2} stops", dark.stops());
        assert!(pale.stops() < 1.0, "a 1.5:1 pale scene measured {:.2} stops", pale.stops());
        assert!(
            dark.stops() > pale.stops(),
            "the metric still prefers the washed-out frame"
        );
        // and the black floor stops a zero from reporting infinite structure
        let void = Values { ground: 0.0, horizon: 0.0, sky: 0.0 };
        assert!(void.stops().is_finite() && void.stops() < 1e-3);
    }

    #[test]
    fn a_flat_style_is_actually_detected() {
        // CLAUDE.md §5b: prove the check can fail.
        let grey = luma([0.5; 3]);
        let flat = Values { ground: grey, horizon: grey, sky: grey };
        assert!(flat.stops() < MIN_STOPS, "a single-value map read as structured");
        let good = Values { ground: 0.05, horizon: 0.12, sky: 0.008 };
        assert!(good.stops() >= MIN_STOPS, "a clearly-composed frame was rejected");
    }

    /// Every map's numbers, printed. `cargo test -- --nocapture value_report`
    /// is the tuning tool: it is how MIN_STOPS was derived from the shipped
    /// table rather than invented, and how to re-derive it if the table moves.
    #[test]
    fn value_report() {
        let all = styles();
        let mut rows: Vec<(f32, String)> =
            all.iter().map(|s| (Values::of(s).stops(), report(s))).collect();
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, r) in &rows {
            println!("{r}");
        }
        assert!(rows.len() >= 36, "the style table shrank to {}", rows.len());
    }

    #[test]
    fn every_shipped_map_has_a_picture_in_it() {
        let all = styles();
        let mut flat: Vec<String> = Vec::new();
        let mut checked = 0;
        for s in all.iter() {
            if Values::of(s).stops() < MIN_STOPS {
                flat.push(report(s));
            }
            checked += 1;
        }
        assert_eq!(checked, all.len());
        assert!(checked >= 36, "the style table shrank to {checked}");
        assert!(
            flat.is_empty(),
            "{} map(s) have under {MIN_STOPS} stops of separation:\n{}",
            flat.len(),
            flat.join("\n")
        );
    }

    #[test]
    fn the_default_map_is_not_the_flattest_one_in_the_table() {
        // The game's front door decides whether it reads as a demo. Not "the
        // default must be dark" — the first frame a player sees must have
        // structure, since it is the whole basis of a first impression.
        let all = styles();
        let def = &all[crate::save::Save::default().map];
        let v = Values::of(def);
        assert!(
            v.stops() >= MIN_STOPS,
            "the DEFAULT map has no value separation: {}",
            report(def)
        );
        let mut spreads: Vec<f32> = all.iter().map(|s| Values::of(s).stops()).collect();
        spreads.sort_by(f32::total_cmp);
        let median = spreads[spreads.len() / 2];
        assert!(
            v.stops() >= median,
            "the default map's {:.2} stops is below the table median {:.2}: {}",
            v.stops(),
            median,
            report(def)
        );
    }
}
