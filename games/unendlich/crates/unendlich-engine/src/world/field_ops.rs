//! `world::field_ops` — consolidated stencil/bounds ops for the physics fields.
//!
//! WID: `r340-sl-fields`. The continuous scalar/vector fields
//! (`world::{field_step, field_brush}` + `sim::{wind_field, fire_field,
//! particulate_field, perception_field, canopy_field_shift}`, and the wider
//! `*_field` set) each re-implement the same handful of primitives: a clamp to
//! `[0,1]`, a clamp-to-bounds accumulate, and a diffusion sweep. This is the
//! shared home for the ones that are **provably identical** across call sites.
//!
//! ## RESPECT (field semantics + bit-parity) — the hard constraint
//! These are sampled continuous fields evolving by PDE-like stencils, not block
//! arrays. Every op here is **bit-for-bit identical** to the call site it
//! replaces (proven in the tests against verbatim copies of the originals). In
//! particular the two diffusion formulations in the codebase are *numerically
//! distinct* and are kept as **separate named ops** — collapsing them into one
//! would silently change a field's output:
//!   - [`diffuse_flux`]  = `world::field_step::diffuse_step` — symmetric per-edge
//!     flux, mass-conserving (the "GPU-perfect reference").
//!   - [`diffuse_gather`] = `sim::particulate_field`'s sweep — gathered
//!     4-neighbour exchange (closed edges).
//! Float op order is preserved exactly (it affects the low bits), so the
//! parallel path ([`diffuse_gather_parallel`]) reproduces the serial result
//! bit-for-bit.
//!
//! Pure, deterministic, panic-free. The core (clamp/accumulate/diffuse) is
//! self-contained (`rustc --test`-able); only [`diffuse_gather_parallel`] reaches
//! for `util::task_lanes` (the existing pure parallelism primitive).
#![allow(dead_code)]

/// Clamp to `[0, 1]`, mapping NaN to 0. **Byte-identical** to
/// `world::field_step::clamp01` and `sim::fire_field::clamp01` (the branch
/// form). NOTE: this preserves a `-0.0` input as `-0.0`; the `max/min` form
/// used elsewhere normalizes `-0.0` to `+0.0` — see [`saturate`]. The two agree
/// bit-for-bit on every other f32. Route branch-form call sites here.
#[inline]
pub fn clamp01(v: f32) -> f32 {
    // r343 dedup: canonical branch clamp (sign-of-zero preserving) lives in mathx.
    crate::util::mathx::clamp01(v)
}

/// Clamp to `[0, 1]`, mapping NaN to 0. **Byte-identical** to
/// [`crate::util::mathx::saturate`], [`sim::canopy_field_shift::clamp01`]
/// and [`sim::particulate_field::c01`]. Normalises `-0.0` to `+0.0`, unlike
/// [`clamp01`] (which preserves the sign of zero).
///
/// r-fulltest-run-2 CORRECTION: a prior comment here (and the sibling test
/// `the_two_forms_agree_everywhere_including_negative_zero`) claimed
/// `saturate` and `clamp01` are bit-identical on every input, including
/// `-0.0`, based on `(-0.0f32).max(0.0f32)` returning `-0.0` on rustc 1.75/
/// x86_64. That measurement was real, but the underlying behavior it relied
/// on — `f32::max`'s tie-break for equal-valued signed zeros — is IEEE 754
/// implementation-defined, not guaranteed. Confirmed it silently flipped on
/// rustc 1.96/the same x86_64 target: `(-0.0f32).max(0.0f32)` now returns
/// `+0.0`. A rollback-netcode sim needing bit-identical `f32` results across
/// machines can't rely on that. Rewrote `saturate` to normalise `-0.0` to
/// `+0.0` via a direct comparison (`v <= 0.0`, true for both signs of zero)
/// instead of `.max()/.min()` — deterministic by construction on every
/// platform/compiler version, not by (now-falsified) measurement on one.
/// See `mathx::saturate`'s doc for the full writeup.
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

/// Add `delta` to `base`, clamped to `[lo, hi]`. **Byte-identical** to
/// `world::field_brush::accumulate` — the bounded write every field brush and
/// per-cell source/decay step shares. `lo` must not exceed `hi`.
#[inline]
pub fn accumulate(base: f32, delta: f32, lo: f32, hi: f32) -> f32 {
    (base + delta).clamp(lo, hi)
}

#[inline]
fn idx(x: usize, y: usize, w: usize) -> usize {
    y * w + x
}

/// One **mass-conserving** diffusion step over a `w × h` row-major `field`.
/// Byte-identical to `world::field_step::diffuse_step`: symmetric per-edge flux
/// (`rate` clamped to `[0,1]`, only +x/+y edges visited so each edge moves mass
/// once), accumulated into a delta buffer then applied. `rate == 0` is a no-op.
pub fn diffuse_flux(field: &mut [f32], w: usize, h: usize, rate: f32) {
    let n = w * h;
    if n == 0 || field.len() < n {
        return;
    }
    let r = clamp01(rate);
    if r == 0.0 {
        return;
    }
    let mut delta = vec![0.0f32; n];
    for y in 0..h {
        for x in 0..w {
            let c = y * w + x;
            if x + 1 < w {
                let nb = c + 1;
                let f = r * 0.25 * (field[c] - field[nb]);
                delta[c] -= f;
                delta[nb] += f;
            }
            if y + 1 < h {
                let nb = c + w;
                let f = r * 0.25 * (field[c] - field[nb]);
                delta[c] -= f;
                delta[nb] += f;
            }
        }
    }
    for i in 0..n {
        field[i] += delta[i];
    }
}

/// The pure per-cell kernel of [`diffuse_gather`]: the new value of cell
/// `(x, y)` given an immutable snapshot `src`. Neighbour accumulation order is
/// `-x, +x, -y, +y` — **identical** to the source sweep, so float low bits
/// match. Reads only `src`, so it is order-independent (safe to parallelize).
#[inline]
fn gather_cell(src: &[f32], w: usize, h: usize, diff: f32, x: usize, y: usize) -> f32 {
    let here = src[idx(x, y, w)];
    let mut acc = here;
    if x > 0 {
        acc += diff * (src[idx(x - 1, y, w)] - here);
    }
    if x + 1 < w {
        acc += diff * (src[idx(x + 1, y, w)] - here);
    }
    if y > 0 {
        acc += diff * (src[idx(x, y - 1, w)] - here);
    }
    if y + 1 < h {
        acc += diff * (src[idx(x, y + 1, w)] - here);
    }
    acc
}

/// One **gathered** 4-neighbour diffusion step over a `w × h` row-major `grid`.
/// Byte-identical to the diffusion sweep in `sim::particulate_field::step`
/// (closed edges; `diff` is the already-scaled coefficient the caller passes,
/// e.g. `(p.diffuse * dt).clamp(0.0, 0.25)`). Distinct from [`diffuse_flux`].
pub fn diffuse_gather(grid: &mut [f32], w: usize, h: usize, diff: f32) {
    let n = w * h;
    if n == 0 || grid.len() < n {
        return;
    }
    let src = grid.to_vec();
    for y in 0..h {
        for x in 0..w {
            grid[idx(x, y, w)] = gather_cell(&src, w, h, diff, x, y);
        }
    }
}

/// Parallel form of [`diffuse_gather`] via the existing `util::task_lanes`
/// pure-map primitive. The kernel is pure (reads the immutable snapshot `src`,
/// writes a disjoint owned output), and `task_lanes` preserves input order, so
/// the result is **bit-for-bit identical** to [`diffuse_gather`]. `min_parallel`
/// gates the serial fallback for small grids.
pub fn diffuse_gather_parallel(grid: &mut [f32], w: usize, h: usize, diff: f32, min_parallel: usize) {
    let n = w * h;
    if n == 0 || grid.len() < n {
        return;
    }
    let src = grid.to_vec();
    let coords: Vec<(usize, usize)> =
        (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).collect();
    let out = crate::util::task_lanes::map_pure_threshold(&coords, min_parallel, |&(x, y)| {
        gather_cell(&src, w, h, diff, x, y)
    });
    for (i, &(x, y)) in coords.iter().enumerate() {
        grid[idx(x, y, w)] = out[i];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Verbatim copies of the ORIGINAL call-site impls, to prove bit-parity ──

    fn ref_field_step_clamp01(v: f32) -> f32 {
        if v.is_nan() { 0.0 } else if v < 0.0 { 0.0 } else if v > 1.0 { 1.0 } else { v }
    }
    // canopy_field_shift::clamp01 / particulate_field::c01 form
    fn ref_maxmin_clamp01(x: f32) -> f32 {
        if x.is_nan() { 0.0 } else { x.max(0.0).min(1.0) }
    }
    fn ref_field_brush_accumulate(base: f32, delta: f32, lo: f32, hi: f32) -> f32 {
        (base + delta).clamp(lo, hi)
    }
    // verbatim field_step::diffuse_step
    fn ref_diffuse_step(field: &mut [f32], w: usize, h: usize, rate: f32) {
        let n = w * h;
        if n == 0 || field.len() < n { return; }
        let r = ref_field_step_clamp01(rate);
        if r == 0.0 { return; }
        let mut delta = vec![0.0f32; n];
        for y in 0..h {
            for x in 0..w {
                let c = y * w + x;
                if x + 1 < w {
                    let nb = c + 1;
                    let f = r * 0.25 * (field[c] - field[nb]);
                    delta[c] -= f; delta[nb] += f;
                }
                if y + 1 < h {
                    let nb = c + w;
                    let f = r * 0.25 * (field[c] - field[nb]);
                    delta[c] -= f; delta[nb] += f;
                }
            }
        }
        for i in 0..n { field[i] += delta[i]; }
    }
    // verbatim particulate_field diffusion sweep
    fn ref_particulate_diffuse(grid: &mut [f32], w: usize, h: usize, diff: f32) {
        let src = grid.to_vec();
        let id = |x: usize, y: usize| y * w + x;
        for y in 0..h {
            for x in 0..w {
                let here = src[id(x, y)];
                let mut acc = here;
                let nb = |nx: usize, ny: usize, acc: &mut f32| {
                    *acc += diff * (src[id(nx, ny)] - here);
                };
                if x > 0 { nb(x - 1, y, &mut acc); }
                if x + 1 < w { nb(x + 1, y, &mut acc); }
                if y > 0 { nb(x, y - 1, &mut acc); }
                if y + 1 < h { nb(x, y + 1, &mut acc); }
                grid[id(x, y)] = acc;
            }
        }
    }

    // Deterministic fuzz grid.
    fn fuzz_grid(n: usize, seed: u32) -> Vec<f32> {
        let mut h = seed.wrapping_add(1);
        (0..n)
            .map(|_| {
                h = h.wrapping_mul(0x9E37_79B9).wrapping_add(0x1234_5);
                ((h >> 8) & 0xFFFF) as f32 / 6553.5 // 0..~10
            })
            .collect()
    }

    #[test]
    fn clamp01_and_saturate_match_their_own_originals_bit_for_bit() {
        let mut h = 1u32;
        // Each form is bit-identical to the call sites it replaces.
        for _ in 0..20_000 {
            h = h.wrapping_mul(0x9E37_79B9).wrapping_add(7);
            let v = f32::from_bits(h ^ (h << 13)); // arbitrary bit patterns
            assert_eq!(clamp01(v).to_bits(), ref_field_step_clamp01(v).to_bits(), "clamp01 vs branch form, v={v}");
            assert_eq!(saturate(v).to_bits(), ref_maxmin_clamp01(v).to_bits(), "saturate vs max/min form, v={v}");
        }
    }

    #[test]
    fn the_two_forms_agree_everywhere_except_the_documented_negative_zero_case() {
        // r-fulltest-run-2: this test previously claimed the two forms are
        // bit-identical on EVERY input including -0.0, based on a
        // measurement (`(-0.0f32).max(0.0f32)` returning `-0.0` on rustc
        // 1.75/x86_64) that turned out to depend on IEEE 754's
        // implementation-defined max/min tie-break for signed zeros — and
        // that measurement silently flipped on rustc 1.96 on the SAME
        // target. `saturate` is now rewritten (see its doc) to
        // deterministically normalise -0.0 to +0.0 via direct comparison,
        // independent of any platform/compiler-version-specific tie-break.
        // That means clamp01 and saturate are BY DESIGN different at
        // exactly -0.0 (clamp01 preserves sign, saturate normalises it) —
        // matching each function's own doc comment, and now true on every
        // target rather than true-by-coincidence on one.
        assert_eq!(clamp01(-0.0).to_bits(), (-0.0f32).to_bits(),
            "clamp01 preserves the sign of zero");
        assert_eq!(saturate(-0.0).to_bits(), (0.0f32).to_bits(),
            "saturate normalises -0.0 to +0.0, deterministically");
        assert_ne!(clamp01(-0.0).to_bits(), saturate(-0.0).to_bits(),
            "the two forms deliberately diverge at exactly this input");
        // Everywhere else (incl +0.0, 1.0, NaN→0, ±inf, in-range, out-of-range)
        // they still agree bit-for-bit.
        let mut h = 99u32;
        for _ in 0..20_000 {
            h = h.wrapping_mul(0x9E37_79B9).wrapping_add(3);
            let v = f32::from_bits(h ^ (h << 11));
            if v == 0.0 && v.is_sign_negative() {
                continue; // the one documented, intentional divergence
            }
            assert_eq!(clamp01(v).to_bits(), saturate(v).to_bits(), "diverged at v={v}");
        }
    }

    #[test]
    fn accumulate_matches_field_brush_bit_for_bit() {
        for (b, d, lo, hi) in [
            (0.8, 0.5, 0.0, 1.0),
            (0.3, -0.5, 0.0, 1.0),
            (0.5, 0.2, 0.0, 1.0),
            (2.0, 5.0, 0.0, 10.0),
            (-3.0, 1.0, -1.0, 1.0),
        ] {
            assert_eq!(
                accumulate(b, d, lo, hi).to_bits(),
                ref_field_brush_accumulate(b, d, lo, hi).to_bits()
            );
        }
    }

    #[test]
    fn diffuse_flux_matches_field_step_bit_for_bit() {
        for (w, h, seed, rate) in [(8, 8, 1, 0.5), (5, 7, 2, 0.8), (16, 9, 3, 0.0), (12, 12, 4, 1.0)] {
            let mut a = fuzz_grid(w * h, seed);
            let mut b = a.clone();
            diffuse_flux(&mut a, w, h, rate);
            ref_diffuse_step(&mut b, w, h, rate);
            assert_eq!(
                a.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                b.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                "diffuse_flux diverged from field_step at {w}x{h}"
            );
        }
    }

    #[test]
    fn diffuse_flux_conserves_mass_like_the_reference() {
        let mut f = fuzz_grid(36, 9);
        let before: f32 = f.iter().sum();
        for _ in 0..30 {
            diffuse_flux(&mut f, 6, 6, 0.7);
        }
        let after: f32 = f.iter().sum();
        assert!((after - before).abs() < 1e-2, "edge-flux must conserve total");
    }

    #[test]
    fn diffuse_gather_matches_particulate_bit_for_bit() {
        for (w, h, seed, diff) in [(8, 8, 5, 0.2), (10, 6, 6, 0.25), (16, 16, 7, 0.1), (3, 3, 8, 0.0)] {
            let mut a = fuzz_grid(w * h, seed);
            let mut b = a.clone();
            diffuse_gather(&mut a, w, h, diff);
            ref_particulate_diffuse(&mut b, w, h, diff);
            assert_eq!(
                a.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                b.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                "diffuse_gather diverged from particulate at {w}x{h}"
            );
        }
    }

    #[test]
    fn flux_and_gather_are_genuinely_distinct() {
        // Same input + a single step: the two formulations must differ, proving
        // they cannot be collapsed into one shared op without changing output.
        let base = fuzz_grid(64, 11);
        let mut a = base.clone();
        let mut b = base.clone();
        diffuse_flux(&mut a, 8, 8, 0.25);
        diffuse_gather(&mut b, 8, 8, 0.25);
        assert_ne!(
            a.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            b.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            "flux and gather happen to match — they must stay separate ops anyway, but this guards the de-dup claim"
        );
    }

    #[test]
    fn deterministic() {
        let mut a = fuzz_grid(64, 21);
        let mut b = a.clone();
        diffuse_flux(&mut a, 8, 8, 0.6);
        diffuse_flux(&mut b, 8, 8, 0.6);
        assert_eq!(a, b);
        let mut c = fuzz_grid(64, 22);
        let mut d = c.clone();
        diffuse_gather(&mut c, 8, 8, 0.2);
        diffuse_gather(&mut d, 8, 8, 0.2);
        assert_eq!(c, d);
    }

    #[test]
    fn empty_or_undersized_is_a_noop() {
        let mut empty: [f32; 0] = [];
        diffuse_flux(&mut empty, 0, 0, 0.5);
        diffuse_gather(&mut empty, 0, 0, 0.2);
        let mut short = vec![1.0, 2.0]; // len 2 < 3*3
        let snap = short.clone();
        diffuse_flux(&mut short, 3, 3, 0.5);
        diffuse_gather(&mut short, 3, 3, 0.2);
        assert_eq!(short, snap, "undersized buffer untouched");
    }
}
