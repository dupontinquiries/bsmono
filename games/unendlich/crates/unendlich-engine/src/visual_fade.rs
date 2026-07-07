//! Visual transition math for chunk meshing: fading newly-loaded chunks in,
//! cross-dissolving LOD swaps so geometry doesn't pop, and thickening fog while
//! the world is still streaming so the unloaded edge hides instead of showing
//! holes.
//!
//! This module is deliberately pure (no render/GPU deps) so it compiles and is
//! unit-tested in the default no-render build. The renderer consumes these
//! values: a per-chunk fade alpha multiplies the chunk toward the fog color;
//! during a LOD swap both meshes are drawn with the cross-fade weights; and the
//! load-fog boost scales `AtmosphereParams::fog_density`.

/// Hermite smoothstep on `[0,1]` (3t² − 2t³). Eases every transition so they
/// start and end gently instead of linearly.
pub fn smoothstep(t: f32) -> f32 {
    // r330-refactor-math: identical to the former local clamped Hermite.
    crate::util::mathx::smoothstep(0.0, 1.0, t)
}

/// Opacity of a chunk `age_ticks` after its mesh first appeared, fading in over
/// `fade_ticks`. 0 = just appeared (invisible, hidden in fog), 1 = fully faded
/// in. A chunk pops into existence far less harshly when its color is lerped
/// from the fog color to its real color by this alpha.
pub fn fade_in_alpha(age_ticks: u32, fade_ticks: u32) -> f32 {
    if fade_ticks == 0 {
        return 1.0;
    }
    smoothstep(age_ticks as f32 / fade_ticks as f32)
}

/// Cross-fade weights `(outgoing, incoming)` partway through a LOD swap.
/// `progress` runs 0→1 across the transition window. The outgoing (old) LOD
/// fades 1→0 while the incoming (new) LOD fades 0→1, both smoothstepped, so the
/// switch dissolves instead of snapping. The renderer draws BOTH meshes during
/// the window with these alphas; outside it, only the settled LOD is drawn.
pub fn lod_crossfade(progress: f32) -> (f32, f32) {
    let p = smoothstep(progress);
    (1.0 - p, p)
}

/// Multiplier (≥ 1.0) applied to fog density while the world is still loading.
/// With `pending` chunks still to generate, fog thickens up to `1 + max_boost`
/// and relaxes to 1.0 as the queue drains (reaching baseline at
/// `settle_pending` or fewer). This hides the render-distance edge — where
/// chunks haven't arrived — inside fog rather than as visible holes.
///
/// `settle_pending` is the count at or below which loading is considered
/// "settled" (no boost); `full_pending` is the count at/above which the boost
/// is maxed. Between them the boost smoothsteps.
pub fn load_fog_boost(
    pending: usize,
    settle_pending: usize,
    full_pending: usize,
    max_boost: f32,
) -> f32 {
    if pending <= settle_pending || max_boost <= 0.0 {
        return 1.0;
    }
    let span = full_pending.saturating_sub(settle_pending).max(1) as f32;
    let over = (pending - settle_pending) as f32;
    1.0 + max_boost * smoothstep(over / span)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn smoothstep_endpoints_and_midpoint() {
        assert!(approx(smoothstep(0.0), 0.0));
        assert!(approx(smoothstep(1.0), 1.0));
        assert!(approx(smoothstep(0.5), 0.5));
        // Clamps outside [0,1].
        assert!(approx(smoothstep(-1.0), 0.0));
        assert!(approx(smoothstep(2.0), 1.0));
        // Eased: below the diagonal in the first half, above in the second.
        assert!(smoothstep(0.25) < 0.25);
        assert!(smoothstep(0.75) > 0.75);
    }

    #[test]
    fn fade_in_ramps_zero_to_one() {
        assert!(approx(fade_in_alpha(0, 30), 0.0), "just-appeared chunk is invisible");
        assert!(approx(fade_in_alpha(30, 30), 1.0), "fully faded in at the window end");
        assert!(approx(fade_in_alpha(60, 30), 1.0), "stays opaque past the window");
        let mid = fade_in_alpha(15, 30);
        assert!(mid > 0.0 && mid < 1.0, "mid-fade is partial: {mid}");
        // Monotonic non-decreasing.
        let mut prev = -1.0;
        for age in 0..=40 {
            let a = fade_in_alpha(age, 30);
            assert!(a >= prev - 1e-6, "fade should not decrease");
            prev = a;
        }
        // fade_ticks == 0 → instantly visible (no fade).
        assert!(approx(fade_in_alpha(0, 0), 1.0));
    }

    #[test]
    fn lod_crossfade_dissolves() {
        let (o0, n0) = lod_crossfade(0.0);
        assert!(approx(o0, 1.0) && approx(n0, 0.0), "start: only old LOD");
        let (o1, n1) = lod_crossfade(1.0);
        assert!(approx(o1, 0.0) && approx(n1, 1.0), "end: only new LOD");
        let (om, nm) = lod_crossfade(0.5);
        assert!(approx(om, 0.5) && approx(nm, 0.5), "mid: even blend");
        // Weights always partition unity (a true cross-dissolve, no flash/gap).
        for i in 0..=20 {
            let (o, n) = lod_crossfade(i as f32 / 20.0);
            assert!(approx(o + n, 1.0), "weights should sum to 1 at p={}", i);
        }
    }

    #[test]
    fn load_fog_boost_thickens_while_loading_relaxes_when_settled() {
        // Settled → no boost.
        assert!(approx(load_fog_boost(0, 16, 256, 3.0), 1.0));
        assert!(approx(load_fog_boost(16, 16, 256, 3.0), 1.0));
        // Heavy loading → near max boost.
        let heavy = load_fog_boost(256, 16, 256, 3.0);
        assert!(approx(heavy, 4.0), "full load should reach 1+max_boost: {heavy}");
        assert!(approx(load_fog_boost(10_000, 16, 256, 3.0), 4.0), "clamps at full");
        // Monotonic non-decreasing in pending.
        let mut prev = 0.0;
        for pending in (0..400).step_by(8) {
            let b = load_fog_boost(pending, 16, 256, 3.0);
            assert!(b >= prev - 1e-6, "boost should not decrease with more pending");
            prev = b;
        }
        // max_boost == 0 disables it entirely.
        assert!(approx(load_fog_boost(500, 16, 256, 0.0), 1.0));
    }
}
