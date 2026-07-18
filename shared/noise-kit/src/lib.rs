//! noise-kit — deterministic coordinate hashing, value noise, and fBm.
//!
//! Both algorithm families here are **bit-identical extractions**, not
//! re-implementations. The golden tests at the bottom pin outputs computed
//! from the original code (soldier `mesh.rs` and logscope `texgen.rs`,
//! captured 2026-07-18), so any drift in constants or op order fails loudly.
//!
//! - [`hash01`] — soldier's per-key 0..1 hash (xxhash/murmur-finalizer
//!   constants), used for vertex-colour dither ("cheap procedural grime").
//! - [`lattice2`] / [`value2`] / [`fbm2`] — the value-noise family soldier
//!   adapted **from logscope texgen.rs**, same 374761393/668265263 lattice
//!   constants, un-tiled, sampled in world space.
//! - [`lattice2_tiled`] / [`value2_tiled`] / [`fbm2_tiled`] — logscope's
//!   period-wrapped originals for seamlessly tiling textures; `fbm2_tiled`
//!   doubles the wrap period per octave exactly as texgen's `fbm` does, so
//!   every octave tiles at the base cell count.
//!
//! Adoption map (sites carrying private copies of these functions):
//! games/soldier/src/mesh.rs (+ per-example mini-copies in
//! games/soldier/examples/*), apps/logscope/crates/logscope-egui/src/texgen.rs,
//! scene3d.rs and theme.rs (related hash-noise variants), clawbound's world
//! scatter. Colour-typed helpers built on these (`dither(Color32, ..)`) stay
//! in their apps — this kit is egui-free by design.

/// Deterministic 0..1 hash of an `i32` key and seed — soldier `hash01`,
/// verbatim (the reference's mixing constants).
#[inline]
pub fn hash01(x: i32, s: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (s as u32).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    (h & 0xFF_FFFF) as f32 / 0xFF_FFFF as f32
}

/// 0..1 hash of a 2D integer lattice point — the shared soldier/logscope
/// lattice hash (`vhash`), verbatim.
#[inline]
pub fn lattice2(xi: i32, yi: i32, seed: u32) -> f32 {
    let mut h = (xi as u32)
        .wrapping_mul(374_761_393)
        .wrapping_add((yi as u32).wrapping_mul(668_265_263))
        .wrapping_add(seed.wrapping_mul(362_437));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    ((h ^ (h >> 16)) & 0xFF_FFFF) as f32 / 0xFF_FFFF as f32
}

/// Period-wrapped lattice hash — logscope texgen's `hashp`, verbatim. Both
/// axes wrap at `period` (`rem_euclid`, so negatives wrap correctly), which is
/// what makes the noise seamlessly tileable.
#[inline]
pub fn lattice2_tiled(xi: i32, yi: i32, period: i32, seed: u32) -> f32 {
    let x = xi.rem_euclid(period.max(1)) as u32;
    let y = yi.rem_euclid(period.max(1)) as u32;
    let mut h = x
        .wrapping_mul(374_761_393)
        .wrapping_add(y.wrapping_mul(668_265_263))
        .wrapping_add(seed.wrapping_mul(362_437));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    ((h ^ (h >> 16)) & 0xFF_FFFF) as f32 / 0xFF_FFFF as f32
}

#[inline]
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Smoothed bilinear value noise in [0,1] — soldier `vnoise2`, verbatim.
pub fn value2(x: f32, y: f32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (smooth(x - x0 as f32), smooth(y - y0 as f32));
    let a = lattice2(x0, y0, seed);
    let b = lattice2(x0 + 1, y0, seed);
    let c = lattice2(x0, y0 + 1, seed);
    let d = lattice2(x0 + 1, y0 + 1, seed);
    let ab = a + (b - a) * fx;
    let cd = c + (d - c) * fx;
    ab + (cd - ab) * fy
}

/// Tileable value noise (wraps at `period` lattice cells) — logscope
/// `vnoise`, verbatim.
pub fn value2_tiled(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (smooth(x - x0 as f32), smooth(y - y0 as f32));
    let a = lattice2_tiled(x0, y0, period, seed);
    let b = lattice2_tiled(x0 + 1, y0, period, seed);
    let c = lattice2_tiled(x0, y0 + 1, period, seed);
    let d = lattice2_tiled(x0 + 1, y0 + 1, period, seed);
    let ab = a + (b - a) * fx;
    let cd = c + (d - c) * fx;
    ab + (cd - ab) * fy
}

/// Multi-octave value noise in [0,1] (amplitude-normalized) — soldier `fbm2`,
/// verbatim: octave `o` samples at `freq = 2^o` with seed `seed + o`.
pub fn fbm2(x: f32, y: f32, oct: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0f32, 0.5f32, 1.0f32, 0.0f32);
    for o in 0..oct {
        sum += value2(x * freq, y * freq, seed.wrapping_add(o)) * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm.max(1e-4)
}

/// Tileable fBm over unit UV space — logscope texgen's `fbm`, verbatim:
/// `u`,`v` in [0,1), `cells` lattice cells across the tile, and the wrap
/// period doubles with the sampling frequency each octave so EVERY octave
/// tiles at the tile boundary.
pub fn fbm2_tiled(u: f32, v: f32, cells: i32, oct: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut p, mut norm) = (0.0f32, 0.5f32, 1.0f32, cells, 0.0f32);
    for o in 0..oct {
        sum += value2_tiled(u * cells as f32 * freq, v * cells as f32 * freq, p, seed + o) * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
        p *= 2;
    }
    sum / norm.max(1e-4)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Outputs pinned from the ORIGINAL implementations (soldier mesh.rs +
    /// logscope texgen.rs compiled verbatim, 2026-07-18). Exact `==` on
    /// purpose: these functions are deterministic integer/float pipelines and
    /// any constant or op-order drift must fail.
    #[test]
    fn golden_parity_with_the_originals() {
        assert_eq!(hash01(5, 17), 0.30728057);
        assert_eq!(hash01(-3, 7), 0.8551008);
        assert_eq!(hash01(0, 0), 0.0);
        assert_eq!(lattice2(3, -2, 9), 0.74807197);
        assert_eq!(value2(1.7, -2.3, 42), 0.52213466);
        assert_eq!(fbm2(0.35, 1.9, 4, 7), 0.5683703);
        assert_eq!(lattice2_tiled(7, -5, 8, 3), 0.8230782);
        assert_eq!(value2_tiled(2.6, 0.4, 8, 11), 0.8603515);
        assert_eq!(fbm2_tiled(0.23, 0.81, 6, 4, 5), 0.44233328);
    }

    #[test]
    fn range_determinism_and_seed_sensitivity() {
        for i in 0..500 {
            let (x, y) = (i as f32 * 0.173 - 40.0, i as f32 * 0.377 - 90.0);
            for v in [
                hash01(i, 17),
                value2(x, y, 42),
                fbm2(x, y, 5, 7),
                value2_tiled(x, y, 16, 3),
                fbm2_tiled(x * 0.01, y * 0.01, 8, 5, 3),
            ] {
                assert!((0.0..=1.0).contains(&v), "out of range: {v}");
            }
        }
        assert_eq!(value2(3.3, 4.4, 5), value2(3.3, 4.4, 5));
        assert_ne!(value2(3.3, 4.4, 5), value2(3.3, 4.4, 6));
        // integer lattice points return the lattice hash exactly
        assert_eq!(value2(4.0, -7.0, 9), lattice2(4, -7, 9));
    }

    #[test]
    fn tiled_noise_actually_tiles() {
        // wrapping the lattice: period cells apart = identical
        for s in 0..8u32 {
            assert_eq!(lattice2_tiled(3, 5, 8, s), lattice2_tiled(3 + 8, 5 - 16, 8, s));
        }
        // and continuous noise repeats with period in lattice units
        let p = 8;
        for i in 0..50 {
            let (x, y) = (i as f32 * 0.37, i as f32 * 0.59);
            let a = value2_tiled(x, y, p, 4);
            let b = value2_tiled(x + p as f32, y + p as f32, p, 4);
            assert!((a - b).abs() < 1e-5, "tiling broke at {i}: {a} vs {b}");
        }
        // fbm over UV: u and u+1 are the same texel column
        for i in 0..20 {
            let (u, v) = (i as f32 * 0.045, i as f32 * 0.031);
            let a = fbm2_tiled(u, v, 6, 4, 5);
            let b = fbm2_tiled(u + 1.0, v, 6, 4, 5);
            assert!((a - b).abs() < 1e-4, "uv tiling broke: {a} vs {b}");
        }
    }

    #[test]
    fn fbm_octaves_add_detail_not_bias() {
        // more octaves keep the mean roughly centered (normalized amplitudes)
        let mut m1 = 0.0;
        let mut m5 = 0.0;
        let n = 400;
        for i in 0..n {
            let (x, y) = (i as f32 * 0.11, i as f32 * 0.07);
            m1 += fbm2(x, y, 1, 3);
            m5 += fbm2(x, y, 5, 3);
        }
        m1 /= n as f32;
        m5 /= n as f32;
        assert!((m1 - 0.5).abs() < 0.1 && (m5 - 0.5).abs() < 0.1, "means drifted: {m1} {m5}");
    }
}
