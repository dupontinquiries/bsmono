//! `aquatic_materials` — seabed material classifier.
//!
//! Worker-03 / WORLD-PERF pure core, round 3. Classifies the substrate of a
//! submerged cell from its depth below sea level, the local slope, and water
//! temperature → sand / gravel / stone / coral-substrate. This addresses the
//! "everything underground is grass" gap: today there is no sand or gravel on
//! the seabed. Slope is the primary sand↔gravel↔stone driver; depth + warmth
//! gate where coral substrate may form.
//!
//! Pure, deterministic, total (every input maps to exactly one material); no
//! clock/rng/unwrap. Self-contained for standalone `rustc --test`.
#![allow(dead_code)]

/// Seabed substrate material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeabedMaterial {
    Sand,
    Gravel,
    Stone,
    CoralSubstrate,
}

/// Classifier thresholds.
#[derive(Clone, Copy, Debug)]
pub struct SeabedParams {
    /// slope ≤ this counts as "flat".
    pub flat_slope_max: f32,
    /// slope ≥ this counts as "steep" (bare stone).
    pub steep_slope_min: f32,
    /// Coral needs water at least this warm (normalized temp, 0..1).
    pub coral_warm_temp_min: f32,
    /// Coral forms only in this depth band (units = depth below sea level).
    pub coral_depth_min: f32,
    pub coral_depth_max: f32,
    /// Coral needs substrate no steeper than this.
    pub coral_slope_max: f32,
}

impl Default for SeabedParams {
    fn default() -> Self {
        SeabedParams {
            flat_slope_max: 0.15,
            steep_slope_min: 0.5,
            coral_warm_temp_min: 0.6,
            coral_depth_min: 0.5,
            coral_depth_max: 12.0,
            coral_slope_max: 0.15,
        }
    }
}

/// Classify with explicit params. Inputs: `depth_below_sea` ≥ 0, `slope` ≥ 0
/// (rise over run), `temp` normalized [0,1].
pub fn classify_with(
    depth_below_sea: f32,
    slope: f32,
    temp: f32,
    p: &SeabedParams,
) -> SeabedMaterial {
    let depth = nonneg(depth_below_sea);
    let slope = nonneg(slope);
    let temp = clamp01(temp);

    // Coral substrate: warm, in the shallow band, on gentle ground.
    if temp >= p.coral_warm_temp_min
        && depth >= p.coral_depth_min
        && depth <= p.coral_depth_max
        && slope <= p.coral_slope_max
    {
        return SeabedMaterial::CoralSubstrate;
    }

    // Otherwise slope decides: steep → bare stone, moderate → gravel, flat → sand.
    if slope >= p.steep_slope_min {
        SeabedMaterial::Stone
    } else if slope > p.flat_slope_max {
        SeabedMaterial::Gravel
    } else {
        SeabedMaterial::Sand
    }
}

/// Classify using default params.
pub fn classify(depth_below_sea: f32, slope: f32, temp: f32) -> SeabedMaterial {
    classify_with(depth_below_sea, slope, temp, &SeabedParams::default())
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

fn nonneg(v: f32) -> f32 {
    if v.is_nan() || v < 0.0 {
        0.0
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shallow_flat_cold_is_sand() {
        // flat + cold (no coral) → sand, at shallow and deep alike.
        assert_eq!(classify(2.0, 0.0, 0.1), SeabedMaterial::Sand);
        assert_eq!(classify(200.0, 0.05, 0.1), SeabedMaterial::Sand);
    }

    #[test]
    fn steep_is_stone_moderate_is_gravel() {
        let p = SeabedParams::default();
        assert_eq!(classify(20.0, 0.9, 0.1), SeabedMaterial::Stone); // steep
        assert_eq!(classify(20.0, 0.3, 0.1), SeabedMaterial::Gravel); // moderate
        // steep always lands in {Gravel, Stone}
        for d in [1.0, 50.0, 500.0] {
            let m = classify(d, p.steep_slope_min + 0.1, 0.1);
            assert!(m == SeabedMaterial::Stone || m == SeabedMaterial::Gravel);
        }
    }

    #[test]
    fn coral_only_in_warm_shallow_gentle_band() {
        // warm + in band + flat → coral
        assert_eq!(classify(5.0, 0.05, 0.8), SeabedMaterial::CoralSubstrate);
        // cold → not coral
        assert_ne!(classify(5.0, 0.05, 0.2), SeabedMaterial::CoralSubstrate);
        // too deep → not coral
        assert_ne!(classify(50.0, 0.05, 0.8), SeabedMaterial::CoralSubstrate);
        // too steep → not coral
        assert_ne!(classify(5.0, 0.6, 0.8), SeabedMaterial::CoralSubstrate);
    }

    #[test]
    fn prop_total_and_deterministic() {
        // Every input in a sweep classifies to exactly one variant, and the
        // result is reproducible.
        let mut d = 0.0f32;
        while d <= 300.0 {
            let mut s = 0.0f32;
            while s <= 1.5 {
                for ti in 0..=10 {
                    let t = ti as f32 / 10.0;
                    let a = classify(d, s, t);
                    let b = classify(d, s, t);
                    assert_eq!(a, b);
                    // a is necessarily one of the four variants (compile-time
                    // total); assert it's a known value to exercise the match.
                    assert!(matches!(
                        a,
                        SeabedMaterial::Sand
                            | SeabedMaterial::Gravel
                            | SeabedMaterial::Stone
                            | SeabedMaterial::CoralSubstrate
                    ));
                }
                s += 0.05;
            }
            d += 5.0;
        }
    }

    #[test]
    fn out_of_range_inputs_are_safe() {
        // NaN / negative inputs don't panic and still classify.
        let _ = classify(f32::NAN, f32::NAN, f32::NAN);
        let _ = classify(-5.0, -1.0, 5.0);
        assert_eq!(classify(-5.0, -1.0, 0.0), SeabedMaterial::Sand); // clamps to flat/shallow/cold
    }
}
