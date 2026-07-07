//! tonemap_lut.rs — pure tonemap curve / LUT generation (photorealism groundwork).
//!
//! Generates a 1D tonemap lookup table mapping scene luminance in `[0, white]`
//! to display value in `[0, 1]`. Pure math, deterministic, panic-free
//! (R1/R11/R21). The renderer samples the table when grading the HDR scene
//! buffer; this module only produces the data.
//!
//! The base curve is extended Reinhard, which is monotonically increasing on
//! `[0, inf)`; the table is normalized so the endpoints are exact (`0.0` at
//! input 0, `1.0` at input `white`).

/// Extended Reinhard tonemap: `f(x) = x*(1 + x/white^2) / (1 + x)`.
/// Monotonically increasing for `x >= 0`, `white > 0`.
fn reinhard(x: f32, white: f32) -> f32 {
    let w2 = white * white;
    let w2 = if w2 > 0.0 { w2 } else { 1.0 };
    (x * (1.0 + x / w2)) / (1.0 + x)
}

/// Build a tonemap LUT of `n` samples (`n` clamped to at least 2) covering
/// inputs `0 ..= white` (a non-positive `white` falls back to `1.0`).
///
/// Guarantees: output values are non-decreasing and lie in `[0, 1]`; the first
/// entry is exactly `0.0` and the last exactly `1.0`; all entries are finite.
pub fn tonemap_lut(n: usize, white: f32) -> Vec<f32> {
    let n = n.max(2);
    let white = if white > 0.0 { white } else { 1.0 };

    // Normalize so endpoints are exact: reinhard(0)=0, divide by reinhard(white).
    let top = reinhard(white, white);
    let inv_top = if top > 0.0 { 1.0 / top } else { 1.0 };

    let mut lut = Vec::with_capacity(n);
    for i in 0..n {
        let x = white * (i as f32) / ((n - 1) as f32);
        let v = (reinhard(x, white) * inv_top).clamp(0.0, 1.0);
        lut.push(v);
    }
    // Pin endpoints exactly (guard against any float drift in the division).
    lut[0] = 0.0;
    let last = n - 1;
    lut[last] = 1.0;
    lut
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITES: &[f32] = &[1.0, 2.0, 4.0, 11.2, 0.0, -3.0];
    const SIZES: &[usize] = &[2, 3, 16, 64, 256];

    #[test]
    fn prop_monotone_non_decreasing() {
        for &w in WHITES {
            for &n in SIZES {
                let lut = tonemap_lut(n, w);
                for i in 1..lut.len() {
                    assert!(
                        lut[i] >= lut[i - 1] - 1e-6,
                        "not monotone at i={} (n={}, white={})",
                        i, n, w
                    );
                }
            }
        }
    }

    #[test]
    fn prop_in_unit_range_and_finite() {
        for &w in WHITES {
            for &n in SIZES {
                for &v in &tonemap_lut(n, w) {
                    assert!(v.is_finite(), "non-finite (n={}, white={})", n, w);
                    assert!(v >= 0.0 && v <= 1.0, "out of range {} (n={}, white={})", v, n, w);
                }
            }
        }
    }

    #[test]
    fn prop_endpoints_exact() {
        for &w in WHITES {
            for &n in SIZES {
                let lut = tonemap_lut(n, w);
                assert_eq!(lut[0], 0.0, "first not 0 (n={}, white={})", n, w);
                assert_eq!(lut[lut.len() - 1], 1.0, "last not 1 (n={}, white={})", n, w);
            }
        }
    }

    #[test]
    fn min_size_is_two() {
        assert_eq!(tonemap_lut(0, 1.0), vec![0.0, 1.0]);
        assert_eq!(tonemap_lut(1, 1.0), vec![0.0, 1.0]);
    }
}
