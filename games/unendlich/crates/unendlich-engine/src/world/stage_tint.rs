//! Per-fragment stage-tint reference for shader-side plant color
//! animation (lever #3 — see `TICK_APPLY_FIXES.md`).
//!
//! ## Why this exists
//!
//! Plant lifecycle stages (Seedling → Growing → Mature → Drying →
//! Withered) each have a color modulation
//! (`plant_lifecycle::stage_color_modulation`). Today, animating between
//! two stages is done CPU-side by re-meshing the chunk every tick the
//! transition is in flight (`chunk_manager`'s transition-remesh sweep):
//! each tick clones the whole chunk and bumps its version so the GPU
//! re-bakes the tint into vertex colors. That clone + re-mesh, every tick,
//! for every animating chunk, is a large slice of `tick_apply` — and it
//! changes no geometry, only color.
//!
//! The fix is to compute the tint **per fragment** on the GPU instead of
//! baking it into the mesh: upload each plant cell's `(from_stage,
//! to_stage, started_tick)` once, pass a `current_tick` uniform, and let
//! the fragment shader lerp. Then a stage change is a tiny per-cell buffer
//! write — no clone, no re-mesh, no octree touch. The sim still owns stage
//! transitions authoritatively (rollback-safe); the shader only *reads*
//! them to color pixels, so float math on the GPU is fine (cosmetic, never
//! fed back into the sim).
//!
//! ## What this module is
//!
//! The CPU twin of the GLSL the renderer will run, so the exact arithmetic
//! is unit-tested here and the shader can be checked against it. It is pure
//! and deterministic; it builds under `--no-default-features` (no render
//! feature needed) precisely so the tint math is testable without a GPU.

use crate::world::plant_lifecycle::{stage_color_modulation, Stage};

/// Linear-interpolated tint between two lifecycle stages' color
/// modulations by animation progress.
///
/// * `from` / `to` — the stages being transitioned between.
/// * `age_ticks` — how many ticks the transition has been running.
/// * `anim_ticks` — the transition's total duration in ticks.
///
/// At `age_ticks == 0` the result is `from`'s color; at
/// `age_ticks >= anim_ticks` it is `to`'s color; in between it is the
/// per-channel linear blend. `anim_ticks == 0` snaps straight to `to`
/// (degenerate / instant transition).
///
/// This is exactly the computation the fragment shader should perform:
/// ```glsl
/// float t = anim_ticks == 0u ? 1.0
///         : clamp(float(age_ticks) / float(anim_ticks), 0.0, 1.0);
/// vec3 tint = mix(from_color, to_color, t);
/// ```
pub fn transition_tint(from: Stage, to: Stage, age_ticks: u32, anim_ticks: u32) -> [f32; 3] {
    let a = stage_color_modulation(from);
    let b = stage_color_modulation(to);
    let t = if anim_ticks == 0 {
        1.0
    } else {
        (age_ticks as f32 / anim_ticks as f32).clamp(0.0, 1.0)
    };
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// The steady-state tint of a settled stage (no transition in flight).
/// Thin wrapper over `stage_color_modulation` so render-side callers have
/// a single tint entry point that pairs with [`transition_tint`].
pub fn stage_tint(stage: Stage) -> [f32; 3] {
    stage_color_modulation(stage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::plant_lifecycle::{stage_color_modulation, Stage};

    fn approx(a: [f32; 3], b: [f32; 3]) -> bool {
        (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6 && (a[2] - b[2]).abs() < 1e-6
    }

    #[test]
    fn at_age_zero_is_from_color() {
        let (from, to) = (Stage::Mature, Stage::Drying);
        assert!(approx(
            transition_tint(from, to, 0, 6),
            stage_color_modulation(from)
        ));
    }

    #[test]
    fn at_or_past_full_age_is_to_color() {
        let (from, to) = (Stage::Mature, Stage::Drying);
        assert!(approx(
            transition_tint(from, to, 6, 6),
            stage_color_modulation(to)
        ));
        // Beyond the duration stays clamped at the target color.
        assert!(approx(
            transition_tint(from, to, 999, 6),
            stage_color_modulation(to)
        ));
    }

    #[test]
    fn midpoint_is_per_channel_halfway() {
        let (from, to) = (Stage::Mature, Stage::Drying);
        let a = stage_color_modulation(from);
        let b = stage_color_modulation(to);
        let mid = transition_tint(from, to, 3, 6);
        for i in 0..3 {
            assert!((mid[i] - (a[i] + (b[i] - a[i]) * 0.5)).abs() < 1e-6);
        }
    }

    #[test]
    fn zero_duration_snaps_to_target() {
        assert!(approx(
            transition_tint(Stage::Seedling, Stage::Mature, 0, 0),
            stage_color_modulation(Stage::Mature)
        ));
    }

    #[test]
    fn same_stage_is_constant() {
        let s = Stage::Growing;
        let c = stage_color_modulation(s);
        assert!(approx(transition_tint(s, s, 0, 6), c));
        assert!(approx(transition_tint(s, s, 3, 6), c));
        assert!(approx(transition_tint(s, s, 6, 6), c));
    }

    #[test]
    fn progress_is_monotonic_per_channel() {
        // Each channel's gap to the target never grows as age increases.
        let (from, to) = (Stage::Seedling, Stage::Withered);
        let b = stage_color_modulation(to);
        let mut prev = transition_tint(from, to, 0, 10);
        for age in 1..=10u32 {
            let cur = transition_tint(from, to, age, 10);
            for i in 0..3 {
                assert!((b[i] - cur[i]).abs() <= (b[i] - prev[i]).abs() + 1e-6);
            }
            prev = cur;
        }
    }

    #[test]
    fn stage_tint_matches_modulation() {
        for s in [
            Stage::Seedling,
            Stage::Growing,
            Stage::Mature,
            Stage::Drying,
            Stage::Withered,
        ] {
            assert!(approx(stage_tint(s), stage_color_modulation(s)));
        }
    }
}
