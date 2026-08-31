//! The reflection ceiling, in Rust, so the claim can be tested.
//!
//! `shader.wgsl` decides how much of a surface is replaced by its reflection.
//! Getting that wrong does not crash and does not fail naga — it just makes
//! the game look wrong on a machine nobody in this sandbox has. So the
//! formula lives here too, as the reference, with tests that assert the
//! PROPERTY rather than the arithmetic: a dry, rough ground must not turn
//! into a mirror when you look along it.
//!
//! The two copies can drift, which this repo has been bitten by before (the
//! three CSDF constants that had to agree, two of which were wrong). So
//! `wgsl_matches_this_reference` scans the shader for the terms and fails if
//! the ceiling is removed — a structural check with a floor on its own match
//! count, per CLAUDE.md §5b.

/// Schlick's Fresnel with a roughness-aware ceiling.
///
/// Plain Schlick lerps `f0 -> 1.0` as the view goes grazing, which is right
/// for a smooth surface and wrong for a rough one: microfacets scatter the
/// grazing lobe instead of returning it. Lerping toward `max(ceiling, f0)`
/// is the standard correction (the same one image-based lighting applies to
/// rough materials).
///
/// `cos_nv` is `dot(normal, view)`, clamped to `[0,1]` by the caller in the
/// shader; clamped here too so a denormal cannot produce a mirror.
pub fn fresnel(f0: f32, ceiling: f32, cos_nv: f32) -> f32 {
    let c = if cos_nv.is_finite() { cos_nv.clamp(0.0, 1.0) } else { 1.0 };
    let ceil = if ceiling.is_finite() { ceiling.max(f0) } else { f0 };
    f0 + (ceil - f0) * (1.0 - c).powi(5)
}

/// How much of a terrain fragment the reflection replaces, given the map's
/// wetness and the view angle. This is `amount` in the shader.
pub fn terrain_reflection_amount(wetness: f32, cos_nv: f32) -> f32 {
    let w = if wetness.is_finite() { wetness.clamp(0.0, 1.0) } else { 0.0 };
    // Below this a ground is dry: no visible sky in it, and no march spent
    // discovering that. The terrain half of the frame-rate problem.
    if w < DRY_GROUND {
        return 0.0;
    }
    // f0 and smoothness are different quantities: water's normal-incidence
    // reflectance is ~0.02, and what makes it a mirror at grazing is that it
    // is SMOOTH. Wetness is the smoothness, so it is the ceiling.
    let f0 = 0.02 + w * 0.10;
    fresnel(f0, w.max(f0), cos_nv).clamp(0.0, 0.85)
}

/// Wetness below which terrain does not reflect at all.
pub const DRY_GROUND: f32 = 0.25;

/// The grazing angle a chase camera actually looks along the ground at.
/// Not a limit case: this is roughly where the ground fills the frame.
pub const CHASE_GRAZING_COS: f32 = 0.08;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::styles;

    #[test]
    fn a_dry_ground_does_not_become_a_mirror_when_you_look_along_it() {
        // The bug the first live recording showed: MEADOW's wetness is 0.08
        // — a dry field — and plain Schlick drove that to the 0.85 ceiling
        // at grazing incidence, which is every ground pixel under a chase
        // camera. The denoise then distrusted the trace (a big flat plane at
        // a grazing angle has an enormous screen-space spread in the
        // reflected direction) and substituted sky_color, so the ground
        // became the sky.
        let dry = terrain_reflection_amount(0.08, CHASE_GRAZING_COS);
        assert!(
            dry < 0.20,
            "a nearly-dry ground reflects {dry:.2} of the sky at grazing — it is a mirror again"
        );
        // and face-on it is barely there at all
        assert!(terrain_reflection_amount(0.08, 1.0) <= 0.09);
        // a damp-but-not-wet ground is capped too, not just the driest case
        assert!(terrain_reflection_amount(0.30, CHASE_GRAZING_COS) < 0.45);
    }

    #[test]
    fn a_wet_ground_still_reflects_or_the_fix_took_the_look_away() {
        // The complement, per CLAUDE.md §5c: the fix EXCLUDES a category, so
        // test who the exclusion takes something from. OBSERVATORY (0.50) and
        // LIGHTHOUSE (0.40) are meant to be wet and shining.
        let wet = terrain_reflection_amount(0.50, CHASE_GRAZING_COS);
        assert!(
            wet > 0.35,
            "wet stone only reflects {wet:.2} at grazing — the fix flattened the wet maps too"
        );
        assert!(
            wet > terrain_reflection_amount(0.08, CHASE_GRAZING_COS) * 2.0,
            "wet and dry ground are no longer distinguishable"
        );
    }

    #[test]
    fn reflection_is_monotone_in_both_wetness_and_grazing() {
        // Assert a RANGE and a direction, not a single point: a formula that
        // has stopped discriminating passes any one-sided check.
        let mut prev = -1.0f32;
        for i in 0..=20 {
            let w = i as f32 / 20.0;
            let a = terrain_reflection_amount(w, CHASE_GRAZING_COS);
            assert!((0.0..=0.85).contains(&a), "amount {a} out of range at wetness {w}");
            if w >= DRY_GROUND {
                assert!(a >= prev - 1e-6, "wetter ground reflected less at {w}");
                prev = a;
            }
        }
        // grazing must reflect more than face-on, for a surface that reflects
        for w in [0.3f32, 0.5, 0.9] {
            let graze = terrain_reflection_amount(w, 0.05);
            let face = terrain_reflection_amount(w, 1.0);
            assert!(graze > face, "wetness {w}: grazing {graze} <= face-on {face}");
        }
    }

    #[test]
    fn no_shipped_map_has_a_ground_that_swallows_the_frame() {
        // The systemic version, across all 36 styles rather than the one map
        // that happened to be recorded. A map whose ground returns most of
        // the sky at chase angles has no horizon, and no amount of grading
        // recovers a frame with no value separation in it.
        let all = styles();
        assert!(all.len() >= 36, "the style table shrank: {}", all.len());
        let mut checked = 0;
        for s in all.iter() {
            let a = terrain_reflection_amount(s.wet, CHASE_GRAZING_COS);
            // The bound is the wettest shipped map (0.55) plus headroom, NOT
            // a round number: a loose bound is satisfied by a formula that
            // has stopped discriminating. Reverting the ceiling puts the wet
            // maps at ~0.68 and this fails, which is the point.
            assert!(
                a <= 0.60,
                "{}: ground returns {a:.2} of the sky at chase angles (wet {:.2})",
                s.name,
                s.wet
            );
            checked += 1;
        }
        // the scan must have found something — a loop over an empty table
        // passes every assertion inside it
        assert_eq!(checked, all.len(), "the sweep skipped styles");
    }

    #[test]
    fn wgsl_matches_this_reference() {
        // Two copies of a formula drift. This does not prove they agree
        // numerically — nothing here can run WGSL — but it does fail if the
        // ceiling term is deleted from the shader and this file is left
        // behind claiming a property the renderer no longer has.
        let src = include_str!("shader.wgsl");
        let needles = [
            "var ceiling = 1.0;",
            "f0 = 0.02 + G.rt.w * 0.10;",
            "ceiling = max(G.rt.w, f0);",
            "reflective = G.rt.w >= 0.25;",
            "let fres = f0 + (ceiling - f0) * pow(1.0 - max(dot(n, v), 0.0), 5.0);",
        ];
        let mut found = 0;
        for n in needles {
            assert!(src.contains(n), "shader.wgsl no longer contains: {n}");
            found += 1;
        }
        assert_eq!(found, needles.len(), "the scan matched nothing");
        // and the un-ceilinged form must be gone, or both are present and
        // whichever runs is a coin flip
        assert!(
            !src.contains("let fres = f0 + (1.0 - f0) * pow(1.0 - max(dot(n, v), 0.0), 5.0);"),
            "the old un-ceilinged Fresnel is still in the shader"
        );
    }
}
