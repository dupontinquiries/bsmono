//! `sim::dither_ordered` — ordered (Bayer) dithering to kill gradient banding
//! in sky/fog. Generates the Bayer threshold matrix and applies it to quantize
//! a value to a small number of levels so a flat gradient *dithers* (its
//! spatial average ≈ the input) instead of showing hard bands. The renderer
//! samples [`threshold`] per pixel and calls [`quantize`].
//!
//! Pure and deterministic: data in, data out. No GPU, IO, clock, or RNG; no
//! `.unwrap()`/`.expect()`. Bounded outputs, no panics.

/// Recursive Bayer matrix of side `2^order`, row-major, values in `[0, 1)`.
///
/// Built from the Bayer recurrence on the base `[[0,2],[3,1]]`:
/// `B_{2s}[y][x] = 4*B_s[y%s][x%s] + base[y/s][x/s]`, then normalised by
/// `side²`. The `side²` entries are a permutation of `0..side²`, so all values
/// are distinct and `< 1`. `order == 0` ⇒ the 1×1 matrix `[0.0]`.
pub fn bayer_matrix(order: u8) -> Vec<f32> {
    // build the integer Bayer matrix (values 0..side²) then normalise.
    let mut side: usize = 1;
    let mut ints: Vec<u32> = vec![0]; // order 0
    const BASE: [[u32; 2]; 2] = [[0, 2], [3, 1]];
    for _ in 0..order {
        let new_side = side * 2;
        let mut next = vec![0u32; new_side * new_side];
        for y in 0..new_side {
            for x in 0..new_side {
                let q = ints[(y % side) * side + (x % side)];
                let b = BASE[y / side][x / side];
                next[y * new_side + x] = 4 * q + b;
            }
        }
        ints = next;
        side = new_side;
    }
    let n = (side * side) as f32;
    ints.iter().map(|&v| v as f32 / n).collect()
}

/// Threshold from `matrix` (a `side × side` row-major matrix) at pixel
/// `(x, y)`, wrapping both coordinates modulo `side`. Returns `0.0` for a
/// degenerate (empty) matrix or `side == 0`.
pub fn threshold(matrix: &[f32], side: usize, x: i32, y: i32) -> f32 {
    if side == 0 || matrix.is_empty() {
        return 0.0;
    }
    let s = side as i32;
    let xi = x.rem_euclid(s) as usize;
    let yi = y.rem_euclid(s) as usize;
    let idx = yi * side + xi;
    if idx < matrix.len() {
        matrix[idx]
    } else {
        // matrix shorter than side² (caller mismatch) — stay in bounds.
        matrix[idx % matrix.len()]
    }
}

/// Dithered quantization of `value01` to `levels` bands using ordered-dither
/// threshold `t01` (typically a [`threshold`] sample, in `[0, 1)`).
///
/// `out = floor(value01*(L-1) + t01) / (L-1)` clamped to `[0, 1]`, where
/// `L = max(2, levels)`. Because `floor(f + t)` for `t` uniform in `[0,1)` has
/// expectation `f`, the per-tile spatial average of a flat input dithers back
/// to that input — no banding.
pub fn quantize(value01: f32, levels: u8, t01: f32) -> f32 {
    let v = san(value01).max(0.0).min(1.0);
    let l = (levels as u32).max(2); // at least 2 bands
    let steps = (l - 1) as f32;
    let t = san(t01).max(0.0).min(1.0 - f32::EPSILON);

    let f = v * steps;
    let band = (f + t).floor().max(0.0).min(steps);
    (band / steps).max(0.0).min(1.0)
}

#[inline]
fn san(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_size_distinct_and_in_range() {
        for order in 0..=4u8 {
            let side = 1usize << order;
            let m = bayer_matrix(order);
            assert_eq!(m.len(), side * side, "order {} wrong size", order);
            assert!(m.iter().all(|&v| (0.0..1.0).contains(&v)), "value out of [0,1)");
            // all distinct: scale back to integers and check the set is 0..n
            let n = side * side;
            let mut ints: Vec<u32> = m.iter().map(|&v| (v * n as f32).round() as u32).collect();
            ints.sort_unstable();
            for (i, v) in ints.iter().enumerate() {
                assert_eq!(*v as usize, i, "not a permutation of 0..{}", n);
            }
        }
    }

    #[test]
    fn four_by_four_embeds_two_by_two_recurrence() {
        let m2 = bayer_matrix(1); // 2x2
        let m4 = bayer_matrix(2); // 4x4
        let base = [[0u32, 2], [3, 1]];
        // recurrence: int4[y][x] == 4*int2[y%2][x%2] + base[y/2][x/2]
        let i2 = |y: usize, x: usize| (m2[y * 2 + x] * 4.0).round() as u32;
        let i4 = |y: usize, x: usize| (m4[y * 4 + x] * 16.0).round() as u32;
        for y in 0..4 {
            for x in 0..4 {
                let expect = 4 * i2(y % 2, x % 2) + base[y / 2][x / 2];
                assert_eq!(i4(y, x), expect, "embed mismatch at ({},{})", x, y);
            }
        }
        // canonical 2x2 pattern
        assert_eq!([i2(0, 0), i2(0, 1), i2(1, 0), i2(1, 1)], [0, 2, 3, 1]);
    }

    #[test]
    fn threshold_wraps() {
        let m = bayer_matrix(2);
        let side = 4;
        for y in -8i32..8 {
            for x in -8i32..8 {
                let a = threshold(&m, side, x, y);
                let b = threshold(&m, side, x + side as i32, y - 2 * side as i32);
                assert_eq!(a, b, "threshold did not wrap at ({},{})", x, y);
            }
        }
        // degenerate guards
        assert_eq!(threshold(&[], 4, 1, 1), 0.0);
        assert_eq!(threshold(&m, 0, 1, 1), 0.0);
    }

    #[test]
    fn quantize_dithers_back_to_input_over_a_tile() {
        let order = 2u8;
        let side = 1usize << order;
        let m = bayer_matrix(order);
        for &levels in &[2u8, 3, 4, 8] {
            // sweep input values; the tile average should be within 1/levels.
            for k in 0..=20 {
                let v = k as f32 / 20.0;
                let mut sum = 0.0f64;
                for y in 0..side {
                    for x in 0..side {
                        sum += quantize(v, levels, threshold(&m, side, x as i32, y as i32)) as f64;
                    }
                }
                let avg = (sum / (side * side) as f64) as f32;
                assert!((avg - v).abs() <= 1.0 / levels as f32 + 1e-4,
                        "levels {} v {} avg {} err {}", levels, v, avg, (avg - v).abs());
            }
        }
    }

    #[test]
    fn quantize_outputs_bounded_and_levels_clamped() {
        for &lv in &[0u8, 1, 2, 16] {
            for k in -3..=23 {
                let v = k as f32 / 20.0;
                let o = quantize(v, lv, 0.5);
                assert!((0.0..=1.0).contains(&o), "out {} for v {} lv {}", o, v, lv);
            }
        }
        // levels 0/1 behave as 2 (min)
        assert_eq!(quantize(0.4, 0, 0.5), quantize(0.4, 2, 0.5));
        assert_eq!(quantize(0.4, 1, 0.5), quantize(0.4, 2, 0.5));
        // non-finite inputs guarded
        assert!(quantize(f32::NAN, 4, f32::INFINITY).is_finite());
    }

    #[test]
    fn deterministic() {
        assert_eq!(bayer_matrix(3), bayer_matrix(3));
        assert_eq!(quantize(0.37, 5, 0.2).to_bits(), quantize(0.37, 5, 0.2).to_bits());
    }
}
