//! `sim::audio_occlusion` — occlusion layer on top of `sim::spatial_audio`
//! (WID r332-sound).
//!
//! `spatial_audio` attenuates a stem by **distance only**, so a sound is heard at
//! full distance-gain even through a wall. This module adds the missing physical
//! cue: solid voxels on the listener→source segment **muffle** the sound — a gain
//! drop (level) plus an optional low-pass cutoff hint (spectral, since a wall
//! eats highs first). It is the next step toward the legible mixer in
//! `handoff/AUDIO_SPATIAL_MIXER.md` / `docs/design/acoustic_perception_and_poi.md`.
//!
//! **Pure math only.** The block count between listener and source comes from a
//! voxel raymarch the *bin* performs (that's glue — see `HANDOFF_audio_occlusion.md`);
//! here the counts are passed in. dB→linear conversion is guarded so the result
//! is always finite and clamped. No IO/clock/RNG, no `unwrap`/`expect`/`panic`.
//!
//! Layers on, doesn't replace: [`occluded_mix_gains`] is exactly
//! `spatial_audio::mix_gains` when every occlusion count is `0`, so callers can
//! adopt it without changing the unoccluded behaviour.

use crate::sim::spatial_audio::{mix_gains, StemSource};

/// Gain floor for a fully-occluded source: a thick wall muffles the sound rather
/// than perfectly silencing it (you still feel a muffled thud through stone).
pub const OCCLUSION_FLOOR: f32 = 0.05;

/// Default attenuation contributed by each solid voxel on the segment, in dB.
/// ~3 dB per block ≈ halving acoustic power per block — a sensible wall default
/// the bin can override per material.
pub const DEFAULT_PER_BLOCK_DB: f32 = 3.0;

/// Full (unoccluded) audio bandwidth used by the muffle-cutoff hint.
pub const FULL_BANDWIDTH_HZ: f32 = 20_000.0;
/// Lowest cutoff the muffle hint will fall to (deep muffle through thick solid).
pub const MUFFLE_FLOOR_HZ: f32 = 250.0;
/// Fraction of the cutoff that survives each solid block (spectral muffling).
pub const MUFFLE_SURVIVE_PER_BLOCK: f32 = 0.6;

/// dB attenuation → linear amplitude multiplier: `10^(-dB/20)`.
/// `db` here is a non-negative attenuation; callers pass attenuation, not gain.
#[inline]
fn db_atten_to_gain(db: f32) -> f32 {
    // amplitude ratio; -20 dB = 0.1×, -6 dB ≈ 0.5×.
    10f32.powf(-db / 20.0)
}

/// Gain multiplier from the number of solid voxels between listener and source.
///
/// `0` blocks → exactly `1.0` (unoccluded). Each block costs `per_block_db` of
/// attenuation; the result is floored at [`OCCLUSION_FLOOR`] so a wall muffles
/// rather than perfectly silences. Monotonically decreasing in the block count.
/// Fully guarded: a non-finite or negative `per_block_db` is treated as no
/// attenuation, and any non-finite intermediate collapses to the floor — the
/// output is always finite and in `[OCCLUSION_FLOOR, 1.0]`.
pub fn occlusion_gain(solid_blocks_between: u32, per_block_db: f32) -> f32 {
    if solid_blocks_between == 0 {
        return 1.0; // unoccluded, exactly — no float drift
    }
    // attenuation only: a negative/NaN/inf per-block value must never amplify.
    let per = if per_block_db.is_finite() { per_block_db.max(0.0) } else { 0.0 };
    let total_db = per * solid_blocks_between as f32;
    let g = db_atten_to_gain(total_db);
    if g.is_finite() {
        g.clamp(OCCLUSION_FLOOR, 1.0)
    } else {
        OCCLUSION_FLOOR
    }
}

/// [`spatial_audio::mix_gains`] with a per-stem occlusion multiplier layered on,
/// using [`DEFAULT_PER_BLOCK_DB`]. `occlusion[i]` is the solid-block count between
/// the listener and stem `i` (raymarched by the bin; passed in here). A stem with
/// no matching entry (shorter `occlusion` slice) is treated as unoccluded.
///
/// Equal to `mix_gains` exactly when every count is `0`.
pub fn occluded_mix_gains(
    listener: [f32; 3],
    stems: &[StemSource],
    falloff: f32,
    occlusion: &[u32],
) -> Vec<f32> {
    occluded_mix_gains_db(listener, stems, falloff, occlusion, DEFAULT_PER_BLOCK_DB)
}

/// As [`occluded_mix_gains`] but with a caller-chosen per-block attenuation (e.g.
/// a denser material). The single code path both 4-arg and tuned callers share.
pub fn occluded_mix_gains_db(
    listener: [f32; 3],
    stems: &[StemSource],
    falloff: f32,
    occlusion: &[u32],
    per_block_db: f32,
) -> Vec<f32> {
    let base = mix_gains(listener, stems, falloff);
    base.iter()
        .enumerate()
        .map(|(i, &g)| {
            let blocks = occlusion.get(i).copied().unwrap_or(0);
            // base is already in [0,1]; the floor-clamped multiplier keeps it there.
            (g * occlusion_gain(blocks, per_block_db)).clamp(0.0, 1.0)
        })
        .collect()
}

/// Low-pass cutoff hint (Hz) for a source behind `solid_blocks` of solid voxels —
/// occlusion is spectral, not just level, so the bin can drive a filter with this.
///
/// `0` blocks → [`FULL_BANDWIDTH_HZ`]; each block keeps
/// [`MUFFLE_SURVIVE_PER_BLOCK`] of the cutoff; floored at [`MUFFLE_FLOOR_HZ`].
/// Monotonically decreasing, always finite.
pub fn muffle_cutoff_hz(solid_blocks: u32) -> f32 {
    if solid_blocks == 0 {
        return FULL_BANDWIDTH_HZ;
    }
    let n = solid_blocks.min(64) as i32; // beyond ~64 it's already at the floor
    let c = FULL_BANDWIDTH_HZ * MUFFLE_SURVIVE_PER_BLOCK.powi(n);
    c.max(MUFFLE_FLOOR_HZ)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn zero_blocks_is_unoccluded_gain_one() {
        for &db in &[0.0f32, 3.0, 6.0, 100.0, -5.0, f32::NAN, f32::INFINITY] {
            assert_eq!(occlusion_gain(0, db), 1.0, "0 blocks must be exactly 1.0 (db={})", db);
        }
    }

    #[test]
    fn gain_decreases_monotonically_with_blocks() {
        let mut prev = occlusion_gain(0, DEFAULT_PER_BLOCK_DB); // 1.0
        for blocks in 1..=40u32 {
            let g = occlusion_gain(blocks, DEFAULT_PER_BLOCK_DB);
            assert!(g <= prev + 1e-9, "gain rose: {} -> {} at {} blocks", prev, g, blocks);
            assert!(g >= OCCLUSION_FLOOR - 1e-9 && g <= 1.0 + 1e-9, "out of range: {}", g);
            prev = g;
        }
    }

    #[test]
    fn floor_respected_for_heavy_occlusion() {
        assert!(approx(occlusion_gain(1000, DEFAULT_PER_BLOCK_DB), OCCLUSION_FLOOR, 1e-9));
        assert!(occlusion_gain(u32::MAX, DEFAULT_PER_BLOCK_DB) >= OCCLUSION_FLOOR - 1e-9);
        // one block of a strong material still never goes below the floor.
        assert!(occlusion_gain(3, 1000.0) >= OCCLUSION_FLOOR - 1e-9);
    }

    #[test]
    fn occluded_equals_mix_when_all_zero() {
        use crate::sim::spatial_audio::menu_stem_layout;
        let stems = menu_stem_layout();
        let falloff = 6.0;
        for listener in [[8.0, 0.0, 4.0], [-8.0, 0.0, 4.0], [0.0, 0.0, 0.0], [3.0, 1.0, -2.0]] {
            let base = mix_gains(listener, &stems, falloff);
            let zeros = vec![0u32; stems.len()];
            let occ = occluded_mix_gains(listener, &stems, falloff, &zeros);
            assert_eq!(base.len(), occ.len());
            for i in 0..base.len() {
                assert_eq!(base[i].to_bits(), occ[i].to_bits(), "stem {} differs at {:?}", i, listener);
            }
            // an empty occlusion slice is also treated as fully unoccluded.
            let occ_empty = occluded_mix_gains(listener, &stems, falloff, &[]);
            for i in 0..base.len() {
                assert_eq!(base[i].to_bits(), occ_empty[i].to_bits());
            }
        }
    }

    #[test]
    fn occlusion_reduces_the_walled_stem() {
        use crate::sim::spatial_audio::menu_stem_layout;
        let stems = menu_stem_layout();
        let falloff = 6.0;
        let listener = [0.0, 0.0, 0.0];
        let base = mix_gains(listener, &stems, falloff);
        // put 4 solid blocks in front of stem index 2 (drums), none elsewhere.
        let occ = occluded_mix_gains(listener, &stems, falloff, &[0, 0, 4]);
        assert!(occ[2] < base[2], "walled stem should drop: {} !< {}", occ[2], base[2]);
        assert_eq!(occ[0].to_bits(), base[0].to_bits(), "unwalled stem unchanged");
        assert_eq!(occ[1].to_bits(), base[1].to_bits(), "unwalled stem unchanged");
    }

    #[test]
    fn db_math_finite_and_clamped_over_fuzz() {
        let mut x: u32 = 0x5046_1D5;
        let mut next = || { x = x.wrapping_mul(1664525).wrapping_add(1013904223); x >> 8 };
        let weird = [-10.0f32, -1.0, 0.0, 0.5, 3.0, 12.0, 1e9, f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
        for _ in 0..50_000 {
            let blocks = next() % 5000;
            let db = weird[(next() as usize) % weird.len()];
            let g = occlusion_gain(blocks, db);
            assert!(g.is_finite(), "non-finite gain for blocks={} db={}", blocks, db);
            assert!(g >= OCCLUSION_FLOOR - 1e-9 && g <= 1.0 + 1e-9, "out of range {} (blocks={} db={})", g, blocks, db);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(occlusion_gain(7, DEFAULT_PER_BLOCK_DB), occlusion_gain(7, DEFAULT_PER_BLOCK_DB));
        assert_eq!(muffle_cutoff_hz(5), muffle_cutoff_hz(5));
        use crate::sim::spatial_audio::menu_stem_layout;
        let s = menu_stem_layout();
        let a = occluded_mix_gains([1.0, 2.0, 3.0], &s, 5.0, &[1, 2, 3]);
        let b = occluded_mix_gains([1.0, 2.0, 3.0], &s, 5.0, &[1, 2, 3]);
        assert_eq!(a, b);
    }

    #[test]
    fn muffle_cutoff_full_then_falls_to_floor() {
        assert_eq!(muffle_cutoff_hz(0), FULL_BANDWIDTH_HZ);
        let mut prev = muffle_cutoff_hz(0);
        for b in 1..=30u32 {
            let c = muffle_cutoff_hz(b);
            assert!(c <= prev + 1e-3, "cutoff rose at {} blocks: {} -> {}", b, prev, c);
            assert!(c >= MUFFLE_FLOOR_HZ - 1e-3, "below floor: {}", c);
            prev = c;
        }
        assert!(approx(muffle_cutoff_hz(1000), MUFFLE_FLOOR_HZ, 1e-3));
    }
}
