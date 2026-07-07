//! `sim::sound_masking` — a mob only reacts to sounds it can actually hear.
//!
//! Ambient noise (wind, water, rain) raises a masking floor; distance and
//! obstruction attenuate a source. This replaces a raw hearing-radius check with
//! a perceived-loudness test. Composes with `hearing_radius` (base range),
//! `ambient` (the floor) and `sound`/`roar_sample` (emitters).
//!
//! Loudness is in dB-ish units. Pure, deterministic, panic-free.

#[derive(Clone, Copy, Debug)]
pub struct Ears {
    pub base_range: f32,
    /// 1.0 = normal hearing; >1 hears farther, <1 duller.
    pub sensitivity: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Sound {
    pub loudness: f32,
    /// Tone 0..1 (unused in the level math, kept for callers that weight by pitch).
    pub freq01: f32,
}

const OCCLUSION_MAX_DB: f32 = 30.0;
const SENSITIVITY_DB_PER_UNIT: f32 = 6.0;

/// Perceived loudness at a listener: spherical spreading (−10·log10(1+d²)) minus
/// up to 30 dB of occlusion. Finite for d²=0 and huge.
pub fn perceived_loudness(s: Sound, dist2: f32, occlusion01: f32) -> f32 {
    let d2 = dist2.max(0.0);
    let occ = occlusion01.clamp(0.0, 1.0);
    s.loudness - 10.0 * (1.0 + d2).log10() - OCCLUSION_MAX_DB * occ
}

/// True if the (sensitivity-boosted) perceived loudness rises above the masking
/// ambient floor.
pub fn can_hear(ears: Ears, s: Sound, dist2: f32, occlusion01: f32, ambient_floor: f32) -> bool {
    let boost = SENSITIVITY_DB_PER_UNIT * (ears.sensitivity - 1.0);
    perceived_loudness(s, dist2, occlusion01) + boost > ambient_floor
}

/// Effective hearing radius for a source of `source_loudness` against an ambient
/// floor — the distance at which perceived == floor, capped by `base_range`.
/// Shrinks (toward 0) as ambient rises.
pub fn effective_range(ears: Ears, source_loudness: f32, ambient_floor: f32) -> f32 {
    let boost = SENSITIVITY_DB_PER_UNIT * (ears.sensitivity - 1.0);
    // source - 10*log10(1+d2) + boost = floor  ->  d2 = 10^((source+boost-floor)/10) - 1
    let margin = source_loudness + boost - ambient_floor;
    let d2 = 10f32.powf(margin / 10.0) - 1.0;
    if d2 <= 0.0 {
        0.0
    } else {
        d2.sqrt().min(ears.base_range.max(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snd(l: f32) -> Sound { Sound { loudness: l, freq01: 0.5 } }
    fn ears(r: f32, s: f32) -> Ears { Ears { base_range: r, sensitivity: s } }

    #[test]
    fn perceived_monotone_and_finite() {
        let s = snd(80.0);
        let near = perceived_loudness(s, 0.0, 0.0);
        let far = perceived_loudness(s, 400.0, 0.0);
        assert!(near > far, "louder up close");
        assert!(perceived_loudness(s, 0.0, 0.0) > perceived_loudness(s, 0.0, 1.0), "occlusion muffles");
        for d2 in [0.0, 1.0, 1e6, 1e12] {
            assert!(perceived_loudness(s, d2, 0.5).is_finite());
        }
    }

    #[test]
    fn can_hear_flips_at_floor() {
        let e = ears(50.0, 1.0);
        // perceived at d2=0 = 80; floor 70 -> hear, floor 90 -> not.
        assert!(can_hear(e, snd(80.0), 0.0, 0.0, 70.0));
        assert!(!can_hear(e, snd(80.0), 0.0, 0.0, 90.0));
    }

    #[test]
    fn higher_sensitivity_hears_farther() {
        let dull = ears(1000.0, 0.5);
        let sharp = ears(1000.0, 2.0);
        assert!(effective_range(sharp, 80.0, 40.0) > effective_range(dull, 80.0, 40.0));
    }

    #[test]
    fn louder_ambient_shrinks_range() {
        let e = ears(1000.0, 1.0);
        let quiet = effective_range(e, 80.0, 40.0);
        let noisy = effective_range(e, 80.0, 70.0);
        assert!(noisy < quiet);
        assert!(effective_range(e, 80.0, 200.0) >= 0.0, "never negative");
    }

    #[test]
    fn deterministic() {
        let e = ears(50.0, 1.2);
        assert_eq!(can_hear(e, snd(75.0), 12.0, 0.3, 60.0), can_hear(e, snd(75.0), 12.0, 0.3, 60.0));
    }
}
