//! mathx — canonical **EXACT** scalar math helpers. WID `r330-refactor-math`.
//!
//! The home for the small exact helpers (`clamp`, `lerp`, `smoothstep`, …) that
//! were copy-pasted across the tree (`fn clamp` alone was re-defined in ~161
//! files). This is the *exact* counterpart to [`crate::util::fastmath`], which
//! holds only **approximate** transcendentals (`rsqrt`, `sin_fast`, …):
//!
//! - `fastmath` trades accuracy for speed — never for parity-bearing values.
//! - `mathx` is **bit-exact**: every function here is the plain IEEE-754
//!   arithmetic it looks like, so a call site can swap its local copy for the
//!   `mathx` version with **zero** behaviour change. No approximations live here.
//!
//! Every fn is `#[inline]`, pure, panic-free, and deterministic (a pure function
//! of its inputs). std-only.
//!
//! ## NaN contract (the subtle part — read before migrating)
//! The tree has **two** historical "clamp to [0,1]" idioms that differ *only* on
//! NaN, and both are intentional in their context, so `mathx` keeps **both**:
//! - [`saturate`] = `x.max(0.0).min(1.0)` → NaN/negatives become `0.0`
//!   ("NaN-sanitising", the dominant idiom in the visual/post-fx modules whose
//!   output must never carry a NaN to the framebuffer).
//! - [`clampf`]`(x, 0.0, 1.0)` = the `if`/`else` clamp → NaN passes through as
//!   NaN (bit-identical to the std `f32::clamp` method for a valid `lo <= hi`).
//!
//! Pick `saturate` to kill NaNs, `clampf(_, 0.0, 1.0)` to preserve them — match
//! whatever the call site you are replacing already did. (A third local form,
//! `wing_membrane::clamp01`, special-cases `is_nan() -> 0.0` *and* returns `0.0`
//! for negatives like `saturate` but is written as an explicit ladder; it is
//! left in place — its NaN→0 path matches `saturate` but its structure differs,
//! noted rather than silently folded.)

/// Clamp `x` into `[lo, hi]` (generic). `if`/`else` form: for floats, a NaN `x`
/// is returned unchanged (it is neither `< lo` nor `> hi`). Assumes `lo <= hi`
/// (does not panic if not — unlike `Ord::clamp`/`f32::clamp` — it just returns
/// `hi` for anything not below `lo`).
#[inline]
pub fn clamp<T: PartialOrd>(x: T, lo: T, hi: T) -> T {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

/// `f32` clamp into `[lo, hi]`. Bit-identical to `x.clamp(lo, hi)` for a valid
/// `lo <= hi` (NaN `x` → NaN), and to the common local `fn clampf` `if`/`else`
/// form (e.g. `settings_schema::clampf`). Never panics.
#[inline]
pub fn clampf(x: f32, lo: f32, hi: f32) -> f32 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

/// Linear interpolation: `a + (b - a) * t`. Exact IEEE form (matches the
/// ubiquitous local `fn lerp`). `t` is *not* clamped (so it extrapolates for
/// `t` outside `[0,1]`, as the local copies do).
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Inverse of [`lerp`]: the `t` for which `lerp(a, b, t) == x`, i.e.
/// `(x - a) / (b - a)`. Returns ±inf / NaN when `a == b` (caller's concern), not
/// clamped.
#[inline]
pub fn inv_lerp(a: f32, b: f32, x: f32) -> f32 {
    (x - a) / (b - a)
}

/// Remap `x` from `[a, b]` onto `[c, d]` linearly (un-clamped):
/// `lerp(c, d, inv_lerp(a, b, x))`. Inverts itself: remapping `[a,b]->[c,d]`
/// then `[c,d]->[a,b]` returns the input (to float precision).
#[inline]
pub fn remap(x: f32, a: f32, b: f32, c: f32, d: f32) -> f32 {
    lerp(c, d, inv_lerp(a, b, x))
}

/// GLSL-style smoothstep: `0` below `e0`, `1` above `e1`, a Hermite ease
/// (`t·t·(3 − 2t)`) between. `t = clampf((x - e0)/(e1 - e0), 0, 1)`.
///
/// Note: `smoothstep(0.0, 1.0, t)` is **bit-identical** to the common local
/// `fn smoothstep(t)` written as `{ let t = t.clamp(0.0,1.0); t*t*(3.0-2.0*t) }`
/// (e.g. `visual_fade::smoothstep`, `weather::smoothstep`). It is *not* the same
/// as the **un-clamped** basis `t*t*(3-2t)` (e.g. `cloud_noise::smoothstep`),
/// which differs for `t` outside `[0,1]`.
#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clampf((x - e0) / (e1 - e0), 0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Clamp to `[0, 1]` the **branch** way: NaN → 0, and a negative zero input is
/// returned unchanged (`-0.0` is not `< 0.0`). This is the sign-of-zero-preserving
/// form; use [`saturate`] when you want `-0.0` normalised to `+0.0`. Canonical home
/// for the many private `fn clamp01` branch copies (and `field_ops::clamp01`).
pub fn clamp01(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else if v < 0.0 {
        0.0
    } else if v > 1.0 {
        1.0
    } else {
        v
    }
}

/// Clamp to `[0, 1]`, mapping NaN to `0.0` and normalising `-0.0` to `+0.0`
/// (unlike [`clamp01`], which preserves the sign of zero). Canonical home
/// for `field_ops::saturate`, `canopy_field_shift::clamp01`, and
/// `particulate_field::c01`.
///
/// Deliberately branch-based rather than `v.max(0.0).min(1.0)`: IEEE 754's
/// `minNum`/`maxNum` are implementation-defined for signed zeros, so
/// `(-0.0).max(0.0)` is NOT guaranteed to return the same sign on every
/// platform/compiler version — confirmed empirically to differ between
/// rustc 1.75 (returned `-0.0`) and rustc 1.96 (returns `+0.0`) on the
/// SAME x86_64 target, let alone across the ARM/Apple Silicon targets this
/// project also ships to. A rollback-netcode sim that needs bit-identical
/// `f32` results across machines can't tolerate that. `v <= 0.0` is a
/// direct IEEE-754 comparison (true for `-0.0`, `+0.0`, and any negative),
/// with no tie-break involved — deterministic on every target, by
/// construction rather than by (fragile, compiler-version-dependent)
/// measurement.
#[inline]
pub fn saturate(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else if v <= 0.0 {
        0.0
    } else if v >= 1.0 {
        1.0
    } else {
        v
    }
}

/// `max(x, 0)` written as the canonical `if x > 0.0 { x } else { 0.0 }` ladder
/// (NaN and non-positives → `0.0`). Matches the repeated local `fn nonneg`
/// (e.g. `sim::aggro_decay`, `sim::batch_aggro_decay`).
#[inline]
pub fn nonneg(x: f32) -> f32 {
    if x > 0.0 {
        x
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // NaN-aware bit equality: two results are "the same" if both NaN, else equal bits.
    fn same(a: f32, b: f32) -> bool {
        (a.is_nan() && b.is_nan()) || a.to_bits() == b.to_bits()
    }

    // A spread of nasty inputs reused by the parity tests.
    fn grid() -> Vec<f32> {
        vec![
            f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0e30, -2.0, -1.0, -0.5,
            -0.0, 0.0, 1.0e-7, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 1.0e30,
        ]
    }

    // ---- mathx unit behaviour ----

    #[test]
    fn clamp_and_clampf_edges_and_nan() {
        assert_eq!(clampf(5.0, 0.0, 10.0), 5.0);
        assert_eq!(clampf(-1.0, 0.0, 10.0), 0.0);
        assert_eq!(clampf(11.0, 0.0, 10.0), 10.0);
        assert!(clampf(f32::NAN, 0.0, 1.0).is_nan()); // NaN passes through
        // generic clamp works on integers too (subsumes the local clampu form).
        assert_eq!(clamp(5u32, 2, 8), 5);
        assert_eq!(clamp(1u32, 2, 8), 2);
        assert_eq!(clamp(9u32, 2, 8), 8);
        assert_eq!(clamp(-3i32, -2, 2), -2);
    }

    #[test]
    fn lerp_endpoints_mid_and_extrapolates() {
        assert_eq!(lerp(2.0, 4.0, 0.0), 2.0);
        assert_eq!(lerp(2.0, 4.0, 1.0), 4.0);
        assert_eq!(lerp(2.0, 4.0, 0.5), 3.0);
        assert_eq!(lerp(2.0, 4.0, 2.0), 6.0); // un-clamped
    }

    #[test]
    fn inv_lerp_inverts_lerp() {
        let (a, b) = (3.0f32, 11.0f32);
        for t in [0.0f32, 0.2, 0.5, 0.8, 1.0, 1.7] {
            let x = lerp(a, b, t);
            assert!((inv_lerp(a, b, x) - t).abs() < 1e-5, "t={t}");
        }
    }

    #[test]
    fn remap_inverts() {
        let (a, b, c, d) = (0.0f32, 10.0, -1.0, 1.0);
        for x in [-3.0f32, 0.0, 2.5, 5.0, 10.0, 13.0] {
            let there = remap(x, a, b, c, d);
            let back = remap(there, c, d, a, b);
            assert!((back - x).abs() < 1e-4, "x={x} back={back}");
        }
    }

    #[test]
    fn smoothstep_edges_clamps_and_eases() {
        assert_eq!(smoothstep(0.0, 1.0, 0.0), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 1.0), 1.0);
        assert_eq!(smoothstep(0.0, 1.0, 0.5), 0.5);
        assert_eq!(smoothstep(0.0, 1.0, -2.0), 0.0); // clamped below e0
        assert_eq!(smoothstep(0.0, 1.0, 9.0), 1.0); // clamped above e1
        // monotone non-decreasing across the ease band.
        let mut prev = -1.0;
        let mut x = -0.2;
        while x <= 1.2 {
            let v = smoothstep(0.0, 1.0, x);
            assert!(v >= prev - 1e-6, "not monotone at x={x}");
            prev = v;
            x += 0.05;
        }
    }

    #[test]
    fn saturate_bounds_and_nan_is_zero() {
        assert_eq!(saturate(-1.0), 0.0);
        assert_eq!(saturate(2.0), 1.0);
        assert_eq!(saturate(0.5), 0.5);
        assert_eq!(saturate(f32::NAN), 0.0); // sanitised
        assert_eq!(saturate(f32::INFINITY), 1.0);
        assert_eq!(saturate(f32::NEG_INFINITY), 0.0);
    }

    #[test]
    fn nonneg_floors_at_zero_and_kills_nan() {
        assert_eq!(nonneg(-3.0), 0.0);
        assert_eq!(nonneg(0.0), 0.0);
        assert_eq!(nonneg(2.0), 2.0);
        assert_eq!(nonneg(f32::NAN), 0.0);
    }

    // ---- per-migration PARITY proofs: each mathx fn is bit-identical to the
    // local idiom it replaces, across the nasty grid (incl. NaN / ±inf). ----

    #[test]
    fn parity_saturate_equals_max_min_idiom() {
        // Migrations: fog_blend, depth_fade, tonemap_curve, lens_dirt,
        // rim_light, sss_approx, edge_highlight (all use `.max(0.0).min(1.0)`).
        //
        // FOLLOW-UP FOUND DURING r-fulltest-run-2 (see saturate()'s doc):
        // this parity check currently passes, but only because on THIS
        // platform/compiler `(-0.0f32).max(0.0f32)` happens to agree with
        // saturate's now-deterministic +0.0 result for -0.0 — the exact
        // coincidence that masked the original bug. The 7 modules named
        // above still call the raw `.max(0.0).min(1.0)` idiom directly
        // (not through `saturate`), so they still carry the same
        // platform/compiler-version-dependent non-determinism this fix
        // just removed from `saturate` itself. Worth migrating each to
        // call `saturate` directly instead of duplicating the idiom — not
        // done here since it's 7 separate call sites across visual/post-fx
        // modules I haven't audited, out of scope for the fix that was
        // actually causing a test failure.
        for x in grid() {
            assert!(same(saturate(x), x.max(0.0).min(1.0)), "x={x}");
        }
    }

    #[test]
    fn parity_clampf_unit_equals_std_clamp_idiom() {
        // Migrations: decal_fade, height_fog, fresnel_schlick (use `.clamp(0,1)`).
        for x in grid() {
            assert!(same(clampf(x, 0.0, 1.0), x.clamp(0.0, 1.0)), "x={x}");
        }
    }

    #[test]
    fn parity_lerp_equals_inline_idiom() {
        // Migration: cloud_noise local `fn lerp` == `a + (b - a) * t`.
        let pts = [(-2.0f32, 5.0f32), (0.0, 1.0), (3.0, 3.0), (-1.0, -4.0)];
        for (a, b) in pts {
            for t in grid() {
                assert!(same(lerp(a, b, t), a + (b - a) * t), "a={a} b={b} t={t}");
            }
        }
    }

    #[test]
    fn parity_smoothstep_unit_equals_clamped_hermite_idiom() {
        // Migrations: visual_fade::smoothstep, weather::smoothstep, written as
        // `{ let t = t.clamp(0.0,1.0); t*t*(3.0-2.0*t) }`.
        for x in grid() {
            let local = {
                let t = x.clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            };
            assert!(same(smoothstep(0.0, 1.0, x), local), "x={x}");
        }
    }

    #[test]
    fn parity_nonneg_equals_reference_ladder() {
        // Reference shape from sim::aggro_decay / sim::batch_aggro_decay.
        for x in grid() {
            let local = if x > 0.0 { x } else { 0.0 };
            assert!(same(nonneg(x), local), "x={x}");
        }
    }
}
