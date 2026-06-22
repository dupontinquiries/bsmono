//! `sim::temporal_accumulate` — pure CPU reference for **temporal froxel
//! accumulation** (the math the temporal-fog compute kernel mirrors).
//!
//! Volumetric fog is rendered into a low-resolution froxel grid and accumulated
//! across frames so it looks dense and stable without paying the full per-frame
//! cost (see `docs/SHADER_ROADMAP.md` §"Temporal reprojection (mandatory)").
//! Each frame, the previous frame's froxel value is reprojected into the current
//! grid, then blended with the freshly-computed sample. Reprojection drags in
//! stale/ghosted values, so the history is first **neighbourhood-clamped**: it is
//! constrained to the per-component min/max AABB of the current froxel's local
//! neighbourhood (the standard TAA colour-clamp), then blended with the current
//! sample by an exponential weight `alpha`.
//!
//! A froxel value here is RGBA (`[f32; 4]`): inscattered radiance in `.rgb` and
//! extinction/density in `.a`. The math is identical per component, so the count
//! is just a convention — every channel goes through the same scalar [`kernel`].
//!
//! Blend convention: `alpha` is the weight of the **current** sample.
//! - `alpha = 0` ⇒ keep history (in-gamut history is returned unchanged).
//! - `alpha = 1` ⇒ take the current sample only (no history).
//! Small `alpha` (e.g. `0.05`) = long history tail = smooth/stable but slow to
//! react; large `alpha` = responsive but noisier. `alpha` is clamped to `[0, 1]`.
//!
//! Pure: data in, data out. No render/Vulkan imports, no clock, no RNG, no
//! `.unwrap()`/`.expect()`. The coordinator mirrors this into the froxel shader
//! and proves GPU↔CPU parity on lavapipe (R22/R24).

/// Number of channels in a froxel value: RGBA (inscatter `.rgb`, extinction `.a`).
pub const CHANNELS: usize = 4;

/// Clamp `x` into `[lo, hi]`. If the bounds arrive reversed (`lo > hi`) they are
/// swapped first, so the clamp is always well-defined for any AABB input.
#[inline]
fn clampf(x: f32, lo: f32, hi: f32) -> f32 {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

/// Endpoint-exact linear blend: `a` at `t=0`, `b` at `t=1`, `a + (b-a)*t`
/// between. The endpoints are returned bit-exactly (not reconstructed by the
/// multiply), so `t=0`⇒`a` and `t=1`⇒`b` hold exactly.
#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    if t <= 0.0 {
        a
    } else if t >= 1.0 {
        b
    } else {
        a + (b - a) * t
    }
}

/// The shared scalar accumulation kernel (one froxel channel).
///
/// 1. clamp `history` into the neighbourhood AABB `[nbhd_min, nbhd_max]`;
/// 2. blend clamped-history → `current` by `alpha` (current's weight).
///
/// `alpha` is clamped to `[0, 1]` before use.
#[inline]
fn kernel(current: f32, history: f32, nbhd_min: f32, nbhd_max: f32, alpha: f32) -> f32 {
    let a = clampf(alpha, 0.0, 1.0);
    let clamped_history = clampf(history, nbhd_min, nbhd_max);
    lerp(clamped_history, current, a)
}

/// Temporal froxel accumulation for one RGBA froxel.
///
/// `current` is the freshly-computed froxel; `history` is the reprojected
/// previous-frame froxel; `nbhd_min`/`nbhd_max` are the per-channel min/max of
/// the current froxel's local neighbourhood (the clamp AABB — by construction it
/// contains `current`). Returns the blended froxel: each channel is
/// `lerp(clamp(history), current, alpha)`.
pub fn accumulate(
    current: [f32; CHANNELS],
    history: [f32; CHANNELS],
    nbhd_min: [f32; CHANNELS],
    nbhd_max: [f32; CHANNELS],
    alpha: f32,
) -> [f32; CHANNELS] {
    let mut out = [0.0f32; CHANNELS];
    let mut i = 0;
    while i < CHANNELS {
        out[i] = kernel(current[i], history[i], nbhd_min[i], nbhd_max[i], alpha);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Seeded deterministic noise (R20) — documented seeds below.
    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn randf(seed: &mut u32, lo: f32, hi: f32) -> f32 {
        lo + (lcg(seed) % 1_000_001) as f32 / 1_000_000.0 * (hi - lo)
    }
    /// A neighbourhood box guaranteed to contain `current` (mins ≤ current ≤
    /// maxs), as the real reprojection clamp always does.
    fn box_around(seed: &mut u32, current: [f32; 4]) -> ([f32; 4], [f32; 4]) {
        let mut lo = [0.0f32; 4];
        let mut hi = [0.0f32; 4];
        for k in 0..4 {
            lo[k] = current[k] - randf(seed, 0.0, 4.0);
            hi[k] = current[k] + randf(seed, 0.0, 4.0);
        }
        (lo, hi)
    }
    fn rand4(seed: &mut u32, lo: f32, hi: f32) -> [f32; 4] {
        [
            randf(seed, lo, hi),
            randf(seed, lo, hi),
            randf(seed, lo, hi),
            randf(seed, lo, hi),
        ]
    }

    #[test]
    fn known_value() {
        // history (0.0) clamped into [−1, 1] stays 0; blend halfway to current 1.0
        let out = accumulate([1.0; 4], [0.0; 4], [-1.0; 4], [1.0; 4], 0.5);
        for k in 0..4 {
            assert!((out[k] - 0.5).abs() < 1e-6, "ch{} = {}", k, out[k]);
        }
    }

    #[test]
    fn alpha_zero_is_identity_to_history() {
        // in-gamut history (inside the box) is returned unchanged at alpha=0.
        let mut s = 0xACC0_0000u32;
        for _ in 0..5000 {
            let current = rand4(&mut s, -10.0, 10.0);
            let (lo, hi) = box_around(&mut s, current);
            // history inside the box per channel
            let mut history = [0.0f32; 4];
            for k in 0..4 {
                history[k] = randf(&mut s, lo[k], hi[k]);
            }
            let out = accumulate(current, history, lo, hi, 0.0);
            assert_eq!(out, history, "alpha=0 must return the (in-gamut) history");
        }
    }

    #[test]
    fn alpha_one_is_pure_current() {
        let mut s = 0xACC0_0001u32;
        for _ in 0..5000 {
            let current = rand4(&mut s, -10.0, 10.0);
            let history = rand4(&mut s, -50.0, 50.0); // arbitrary, even out-of-box
            let (lo, hi) = box_around(&mut s, current);
            let out = accumulate(current, history, lo, hi, 1.0);
            assert_eq!(out, current, "alpha=1 must return the current sample exactly");
        }
    }

    #[test]
    fn output_stays_inside_neighbourhood_box() {
        // current ∈ box (as reprojection guarantees) ⇒ for any history and any
        // alpha∈[0,1] the blended result stays inside the box per channel.
        let mut s = 0xACC0_0002u32;
        for _ in 0..20000 {
            let current = rand4(&mut s, -10.0, 10.0);
            let (lo, hi) = box_around(&mut s, current);
            let history = rand4(&mut s, -100.0, 100.0); // may be far outside
            let alpha = randf(&mut s, 0.0, 1.0);
            let out = accumulate(current, history, lo, hi, alpha);
            for k in 0..4 {
                assert!(
                    out[k] >= lo[k] - 1e-4 && out[k] <= hi[k] + 1e-4,
                    "ch{} {} escaped [{}, {}]",
                    k, out[k], lo[k], hi[k]
                );
            }
        }
    }

    #[test]
    fn alpha_out_of_range_is_clamped() {
        // alpha<0 behaves as 0 (history), alpha>1 behaves as 1 (current).
        let current = [2.0; 4];
        let history = [0.5; 4]; // in [-1,3]
        let lo = [-1.0; 4];
        let hi = [3.0; 4];
        assert_eq!(accumulate(current, history, lo, hi, -5.0), history);
        assert_eq!(accumulate(current, history, lo, hi, 9.0), current);
    }

    #[test]
    fn outputs_finite_for_bounded_inputs() {
        let mut s = 0xACC0_0003u32;
        for _ in 0..20000 {
            let current = rand4(&mut s, -1e3, 1e3);
            let history = rand4(&mut s, -1e3, 1e3);
            let lo = rand4(&mut s, -1e3, 0.0);
            let hi = rand4(&mut s, 0.0, 1e3);
            let alpha = randf(&mut s, -2.0, 3.0);
            let out = accumulate(current, history, lo, hi, alpha);
            for k in 0..4 {
                assert!(out[k].is_finite(), "ch{} not finite: {}", k, out[k]);
            }
        }
    }

    #[test]
    fn blend_is_monotonic_in_alpha() {
        // As alpha rises 0→1 each channel moves monotonically from clamped
        // history toward current (and never overshoots either endpoint).
        let mut s = 0xACC0_0004u32;
        for _ in 0..3000 {
            let current = rand4(&mut s, -10.0, 10.0);
            let (lo, hi) = box_around(&mut s, current);
            let history = rand4(&mut s, -50.0, 50.0);
            let mut clamped = [0.0f32; 4];
            for k in 0..4 {
                clamped[k] = clampf(history[k], lo[k], hi[k]);
            }
            let mut prev = accumulate(current, history, lo, hi, 0.0);
            let steps = 16;
            for j in 1..=steps {
                let alpha = j as f32 / steps as f32;
                let cur = accumulate(current, history, lo, hi, alpha);
                for k in 0..4 {
                    // distance to current is non-increasing in alpha
                    let d_prev = (prev[k] - current[k]).abs();
                    let d_cur = (cur[k] - current[k]).abs();
                    assert!(
                        d_cur <= d_prev + 1e-4,
                        "ch{} not monotonic toward current: {} -> {}",
                        k, d_prev, d_cur
                    );
                    // stays within [clamped_history, current] (ordered)
                    let a = clamped[k].min(current[k]) - 1e-4;
                    let b = clamped[k].max(current[k]) + 1e-4;
                    assert!(cur[k] >= a && cur[k] <= b, "ch{} {} outside blend span", k, cur[k]);
                }
                prev = cur;
            }
        }
    }
}
