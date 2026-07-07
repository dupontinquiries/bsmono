//! `biome_blend` — blend biome-mix weights into terrain parameters.
//!
//! Worker-03 / WORLD-PERF pure core, round 8. At a biome boundary the terrain
//! generator has fractional membership in several biomes; this blends their
//! parameters by normalized weight so height/roughness/temperature/palette vary
//! continuously across the seam instead of stepping. Pairs with the worldgen
//! params path.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Per-biome terrain parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BiomeParams {
    pub height_scale: f32,
    pub roughness: f32,
    pub temp: f32,
    pub palette: [f32; 4],
}

impl BiomeParams {
    pub const ZERO: BiomeParams = BiomeParams {
        height_scale: 0.0,
        roughness: 0.0,
        temp: 0.0,
        palette: [0.0; 4],
    };
}

/// Blend biome parameters by weight. Negative/NaN weights are treated as 0.
/// Weights are normalized to sum 1; if they sum to ≤ 0 (e.g. all zero), a
/// uniform average is used. The result is a convex combination of the inputs,
/// so every field lies within the per-field min/max of the contributing biomes.
pub fn blend(weights: &[f32], biomes: &[BiomeParams]) -> BiomeParams {
    let n = weights.len().min(biomes.len());
    if n == 0 {
        return BiomeParams::ZERO;
    }

    let mut wsum = 0.0f32;
    for i in 0..n {
        wsum += clean_weight(weights[i]);
    }
    let uniform = !(wsum > 0.0);

    let mut acc = BiomeParams::ZERO;
    for i in 0..n {
        let w = if uniform {
            1.0 / n as f32
        } else {
            clean_weight(weights[i]) / wsum
        };
        acc.height_scale += biomes[i].height_scale * w;
        acc.roughness += biomes[i].roughness * w;
        acc.temp += biomes[i].temp * w;
        for k in 0..4 {
            acc.palette[k] += biomes[i].palette[k] * w;
        }
    }
    acc
}

fn clean_weight(w: f32) -> f32 {
    if w.is_nan() || w < 0.0 {
        0.0
    } else {
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bp(hs: f32, r: f32, t: f32, pal: [f32; 4]) -> BiomeParams {
        BiomeParams { height_scale: hs, roughness: r, temp: t, palette: pal }
    }

    fn approx(a: &BiomeParams, b: &BiomeParams) -> bool {
        let close = |x: f32, y: f32| (x - y).abs() < 1e-5;
        close(a.height_scale, b.height_scale)
            && close(a.roughness, b.roughness)
            && close(a.temp, b.temp)
            && (0..4).all(|k| close(a.palette[k], b.palette[k]))
    }

    #[test]
    fn single_biome_returns_itself() {
        let b = bp(10.0, 0.3, 0.7, [0.1, 0.2, 0.3, 0.4]);
        assert!(approx(&blend(&[1.0], &[b]), &b));
        assert!(approx(&blend(&[5.0], &[b]), &b)); // any positive weight
    }

    #[test]
    fn weights_are_normalized() {
        let a = bp(0.0, 0.0, 0.0, [0.0; 4]);
        let b = bp(10.0, 1.0, 1.0, [1.0; 4]);
        // [2,2] should behave like [1,1] → midpoint.
        let r = blend(&[2.0, 2.0], &[a, b]);
        assert!(approx(&r, &bp(5.0, 0.5, 0.5, [0.5; 4])));
        // scaling all weights doesn't change the result.
        assert!(approx(&blend(&[1.0, 3.0], &[a, b]), &blend(&[2.0, 6.0], &[a, b])));
    }

    #[test]
    fn all_zero_weights_are_uniform_average() {
        let a = bp(0.0, 0.0, 0.0, [0.0; 4]);
        let b = bp(6.0, 0.6, 0.6, [0.6; 4]);
        let c = bp(3.0, 0.3, 0.3, [0.3; 4]);
        let r = blend(&[0.0, 0.0, 0.0], &[a, b, c]);
        assert!(approx(&r, &bp(3.0, 0.3, 0.3, [0.3; 4]))); // mean of the three
    }

    #[test]
    fn result_is_convex_combination() {
        let biomes = [
            bp(2.0, 0.1, -1.0, [0.0, 0.5, 1.0, 0.2]),
            bp(20.0, 0.9, 1.0, [1.0, 0.0, 0.0, 0.8]),
            bp(8.0, 0.5, 0.2, [0.3, 0.3, 0.3, 0.5]),
        ];
        // sweep some weight vectors deterministically
        for wv in [[1.0, 2.0, 3.0], [0.0, 1.0, 0.0], [5.0, 5.0, 0.0], [1.0, 0.0, 9.0]] {
            let r = blend(&wv, &biomes);
            let field = |sel: fn(&BiomeParams) -> f32| {
                let mut lo = f32::INFINITY;
                let mut hi = f32::NEG_INFINITY;
                for b in &biomes {
                    let v = sel(b);
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
                (lo, hi)
            };
            let (lo, hi) = field(|b| b.height_scale);
            assert!(r.height_scale >= lo - 1e-4 && r.height_scale <= hi + 1e-4);
            let (lo, hi) = field(|b| b.temp);
            assert!(r.temp >= lo - 1e-4 && r.temp <= hi + 1e-4);
            for k in 0..4 {
                let mut lo = f32::INFINITY;
                let mut hi = f32::NEG_INFINITY;
                for b in &biomes {
                    lo = lo.min(b.palette[k]);
                    hi = hi.max(b.palette[k]);
                }
                assert!(r.palette[k] >= lo - 1e-4 && r.palette[k] <= hi + 1e-4);
            }
        }
    }

    #[test]
    fn negative_and_nan_weights_treated_as_zero() {
        let a = bp(0.0, 0.0, 0.0, [0.0; 4]);
        let b = bp(10.0, 1.0, 1.0, [1.0; 4]);
        // negative weight on `a` ignored → all weight on `b`.
        assert!(approx(&blend(&[-3.0, 2.0], &[a, b]), &b));
        assert!(approx(&blend(&[f32::NAN, 1.0], &[a, b]), &b));
    }

    #[test]
    fn deterministic() {
        let biomes = [bp(1.0, 0.2, 0.3, [0.1; 4]), bp(9.0, 0.8, 0.7, [0.9; 4])];
        assert_eq!(blend(&[0.3, 0.7], &biomes), blend(&[0.3, 0.7], &biomes));
    }
}
