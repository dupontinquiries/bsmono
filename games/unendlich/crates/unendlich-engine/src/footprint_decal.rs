//! footprint_decal.rs — pure footprint/decal parameter solver.
//!
//! Turns "a body part of shape S, mass M, pressed into ground of softness G,
//! `age` seconds ago, under `weather` erosion" into decal parameters (outline,
//! depth, fade) the decal renderer consumes. Heavier parts press deeper; prints
//! fade with age and weather; the outline mirrors the impacting part's shape.
//! Pure math, deterministic, panic-free (R1/R11/R21).

/// Shape of the body part that made contact (the print's silhouette).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PartShape {
    /// Long-axis size of the contact, metres.
    pub length: f32,
    /// Short-axis size of the contact, metres.
    pub width: f32,
    /// Distinct marks (toes / claws / pads) — shape detail carried to the decal.
    pub marks: u8,
}

/// Everything needed to resolve one footprint.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FootprintInput {
    pub shape: PartShape,
    /// Mass pressing through this single contact, kilograms.
    pub weight_kg: f32,
    /// Ground yield: 0 = rock (no impression), 1 = soft mud.
    pub ground_softness: f32,
    /// Seconds since the print was made.
    pub age_s: f32,
    /// Weather erosion rate: 0 = calm/dry, 1 = heavy rain/wind.
    pub weather: f32,
}

/// Resolved decal parameters.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DecalParams {
    /// World-space footprint outline `[length, width]`, metres (mirrors the part,
    /// splayed slightly by how deep it pressed).
    pub outline: [f32; 2],
    /// Shape detail carried through (toe/claw/pad marks).
    pub marks: u8,
    /// Impression depth, metres (>= 0, saturating at `MAX_DEPTH_M`).
    pub depth: f32,
    /// Opacity 0..1 (1 = fresh, 0 = erased).
    pub fade: f32,
}

/// Deepest impression any footprint can leave, metres. Caps the sqrt pressure
/// law so a very heavy step doesn't produce an absurd crater.
pub const MAX_DEPTH_M: f32 = 0.12;
/// Depth (metres) produced per sqrt(kg) on fully soft ground, before the cap.
const DEPTH_PER_SQRT_KG: f32 = 0.006;
/// Seconds for a print's natural (weatherless) opacity to fall to 1/e.
const FADE_LIFETIME_S: f32 = 600.0;
/// How much heavy weather accelerates fading (multiplier on the decay rate).
const WEATHER_ACCEL: f32 = 6.0;
/// Extra splay of the outline per metre of depth.
const SPLAY_PER_M: f32 = 1.5;

/// Resolve decal parameters for a footprint. See the property tests for the
/// guaranteed monotonicities.
pub fn resolve(input: &FootprintInput) -> DecalParams {
    let softness = input.ground_softness.clamp(0.0, 1.0);
    let weight = input.weight_kg.max(0.0);
    let age = input.age_s.max(0.0);
    let weather = input.weather.clamp(0.0, 1.0);

    // Pressure law: depth ~ sqrt(mass), scaled by how much the ground yields.
    let raw_depth = DEPTH_PER_SQRT_KG * weight.sqrt() * softness;
    let depth = raw_depth.clamp(0.0, MAX_DEPTH_M);

    // Exponential opacity decay; weather multiplies the decay rate.
    let rate = (1.0 + weather * WEATHER_ACCEL) / FADE_LIFETIME_S;
    let fade = (-age * rate).exp().clamp(0.0, 1.0);

    // Outline mirrors the part, splayed a touch by the impression depth.
    let splay = 1.0 + depth * SPLAY_PER_M;
    let outline = [
        input.shape.length.max(0.0) * splay,
        input.shape.width.max(0.0) * splay,
    ];

    DecalParams {
        outline,
        marks: input.shape.marks,
        depth,
        fade,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> FootprintInput {
        FootprintInput {
            shape: PartShape { length: 0.30, width: 0.12, marks: 4 },
            weight_kg: 40.0,
            ground_softness: 0.6,
            age_s: 0.0,
            weather: 0.0,
        }
    }

    #[test]
    fn prop_heavier_presses_deeper() {
        // Monotone non-decreasing in weight (saturating at MAX_DEPTH_M).
        let mut prev = -1.0f32;
        let mut w = 0.0f32;
        while w <= 2000.0 {
            let mut inp = base();
            inp.weight_kg = w;
            let d = resolve(&inp).depth;
            assert!(d >= prev - 1e-7, "depth dropped at w={} ({} < {})", w, d, prev);
            assert!(d >= 0.0 && d <= MAX_DEPTH_M, "depth out of range {}", d);
            prev = d;
            w += 25.0;
        }
    }

    #[test]
    fn prop_fade_monotone_in_age() {
        let mut prev = 2.0f32;
        let mut age = 0.0f32;
        while age <= 5000.0 {
            let mut inp = base();
            inp.age_s = age;
            let f = resolve(&inp).fade;
            assert!(f <= prev + 1e-7, "fade rose with age at {} ({} > {})", age, f, prev);
            assert!(f >= 0.0 && f <= 1.0);
            prev = f;
            age += 50.0;
        }
    }

    #[test]
    fn prop_fade_monotone_in_weather() {
        let mut prev = 2.0f32;
        let mut wx = 0.0f32;
        while wx <= 1.0 {
            let mut inp = base();
            inp.age_s = 120.0; // fixed age so weather is the only variable
            inp.weather = wx;
            let f = resolve(&inp).fade;
            assert!(f <= prev + 1e-7, "fade rose with weather at {} ({} > {})", wx, f, prev);
            assert!(f >= 0.0 && f <= 1.0);
            prev = f;
            wx += 0.05;
        }
    }

    #[test]
    fn fresh_print_is_opaque_outline_mirrors_part() {
        let p = resolve(&base());
        assert!((p.fade - 1.0).abs() < 1e-6, "fresh print should be opaque");
        assert_eq!(p.marks, 4);
        // outline is the part shape splayed by depth (>= the raw part size).
        assert!(p.outline[0] >= 0.30 && p.outline[1] >= 0.12);
    }

    #[test]
    fn rock_takes_no_impression() {
        let mut inp = base();
        inp.ground_softness = 0.0;
        assert_eq!(resolve(&inp).depth, 0.0);
    }

    #[test]
    fn prop_deterministic_and_finite() {
        let mut seed = 0x577E_9001u32;
        let mut lcg = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / ((1u32 << 24) as f32)
        };
        for _ in 0..500 {
            let inp = FootprintInput {
                shape: PartShape { length: lcg() * 2.0, width: lcg(), marks: (lcg() * 6.0) as u8 },
                weight_kg: lcg() * 1500.0,
                ground_softness: lcg(),
                age_s: lcg() * 4000.0,
                weather: lcg(),
            };
            let a = resolve(&inp);
            let b = resolve(&inp);
            assert_eq!(a, b, "non-deterministic");
            assert!(a.depth.is_finite() && a.fade.is_finite());
            assert!(a.outline[0].is_finite() && a.outline[1].is_finite());
        }
    }
}
