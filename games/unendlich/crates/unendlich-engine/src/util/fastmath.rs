//! fastmath — approximate transcendentals for SIM/VISUAL hot paths.
//!
//! # PARITY CONTRACT (read before use)
//! These functions are APPROXIMATE and must NEVER be used in any value
//! that also flows through the GPU worldgen shader (density, biome, sea
//! level, surface height). That CPU↔GPU surface must stay bit-identical
//! (see r195 ocean-seam fix); it uses only mul/add/floor anyway, so it
//! has no need for this module. Use fastmath ONLY in CPU-only simulation
//! and visual math (lighting, waves, wander, tremble, viewmodel, …),
//! where there is no GPU twin to disagree with.
//!
//! Every function is DETERMINISTIC (a pure function of its inputs) — the
//! engine's replay/rollback invariants are preserved. std-only.
//!
//! Each function documents its max abs error over its useful domain so
//! the approximation budget is explicit and testable.

/// Fast inverse square root: ~1/sqrt(x). Quake-style seed + one Newton
/// step. Max rel error < 1.8e-3 over x in [1e-6, 1e6]. For x <= 0 returns
/// 0.0 (callers normalizing a zero vector get a zero result, matching the
/// degenerate branch in `vecmath::norm3`).
#[inline]
pub fn rsqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let i = x.to_bits();
    let i = 0x5f37_59df_u32.wrapping_sub(i >> 1);
    let y = f32::from_bits(i);
    // one Newton-Raphson refinement: y *= 1.5 - 0.5*x*y*y
    y * (1.5 - 0.5 * x * y * y)
}

/// Normalize a 3-vector using `rsqrt` (no sqrt, no divide). Returns
/// `[1,0,0]` for near-zero input — identical fallback to `vecmath::norm3`
/// so callers can swap with no behavior change at the degenerate point.
/// Direction error < ~0.1° (within the rsqrt budget); fine for lighting,
/// wander headings, wave normals. NOT for anything parity-bearing.
#[inline]
pub fn norm3_fast(v: [f32; 3]) -> [f32; 3] {
    let sq = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    if sq <= 1e-12 {
        return [1.0, 0.0, 0.0];
    }
    let inv = rsqrt(sq);
    [v[0] * inv, v[1] * inv, v[2] * inv]
}

const PI: f32 = std::f32::consts::PI;
const TWO_PI: f32 = 2.0 * PI;

/// Range-reduce to [-PI, PI].
#[inline]
fn wrap_pi(x: f32) -> f32 {
    let t = (x + PI) * (1.0 / TWO_PI);
    let t = t - t.floor();
    t * TWO_PI - PI
}

/// Fast sine, ~7th-order minimax-ish (Bhaskara-refined). Max abs error
/// < 1.1e-3 over all x (after range reduction). Deterministic.
#[inline]
pub fn sin_fast(x: f32) -> f32 {
    let x = wrap_pi(x);
    // Parabolic approximation + one correction term (Devlin/Bhaskara).
    const B: f32 = 4.0 / PI;
    const C: f32 = -4.0 / (PI * PI);
    let y = B * x + C * x * x.abs();
    // refinement: y = P*(y*|y| - y) + y, P = 0.225
    0.225 * (y * y.abs() - y) + y
}

/// Fast cosine via `sin_fast(x + PI/2)`. Same error budget.
#[inline]
pub fn cos_fast(x: f32) -> f32 {
    sin_fast(x + PI * 0.5)
}

/// Fast exp via 2^(x/ln2) with a bit-hack base and polynomial mantissa.
/// Max rel error < 3e-3 over x in [-20, 20]. Saturates sanely outside.
/// For decay curves, falloffs, soft transitions — not finance.
#[inline]
pub fn exp_fast(x: f32) -> f32 {
    if x < -87.0 {
        return 0.0;
    }
    if x > 88.0 {
        return f32::INFINITY;
    }
    // exp(x) = 2^(x * log2(e))
    let t = x * std::f32::consts::LOG2_E;
    let fl = t.floor();
    let frac = t - fl;
    // 2^frac ≈ polynomial on [0,1)
    let poly = 1.0 + frac * (0.6960656421 + frac * (0.2247852567 + frac * 0.0791451350));
    let pow2i = f32::from_bits((((fl as i32) + 127) as u32) << 23);
    pow2i * poly
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rsqrt_within_budget() {
        for &x in &[1e-3f32, 0.5, 1.0, 2.0, 9.0, 100.0, 1e5] {
            let approx = rsqrt(x);
            let exact = 1.0 / x.sqrt();
            let rel = ((approx - exact) / exact).abs();
            assert!(rel < 2e-3, "rsqrt({x}) rel err {rel}");
        }
        assert_eq!(rsqrt(0.0), 0.0);
        assert_eq!(rsqrt(-1.0), 0.0);
    }

    #[test]
    fn norm3_fast_unit_and_degenerate() {
        let n = norm3_fast([3.0, 0.0, 4.0]);
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        assert!((len - 1.0).abs() < 3e-3, "len {len}");
        assert_eq!(norm3_fast([0.0; 3]), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn sin_cos_within_budget() {
        let mut x = -10.0f32;
        while x < 10.0 {
            assert!((sin_fast(x) - x.sin()).abs() < 1.2e-3, "sin {x}");
            assert!((cos_fast(x) - x.cos()).abs() < 1.2e-3, "cos {x}");
            x += 0.05;
        }
    }

    #[test]
    fn exp_within_budget() {
        for &x in &[-10.0f32, -2.0, -0.5, 0.0, 0.5, 2.0, 10.0] {
            let approx = exp_fast(x);
            let exact = x.exp();
            let rel = ((approx - exact) / exact).abs();
            assert!(rel < 3e-3, "exp({x}) rel err {rel}");
        }
        assert_eq!(exp_fast(-100.0), 0.0);
    }

    #[test]
    fn deterministic() {
        // same input -> same bits, every call (replay invariant).
        assert_eq!(sin_fast(1.234).to_bits(), sin_fast(1.234).to_bits());
        assert_eq!(rsqrt(2.0).to_bits(), rsqrt(2.0).to_bits());
    }
}
