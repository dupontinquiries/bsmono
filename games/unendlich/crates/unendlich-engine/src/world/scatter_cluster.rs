//! `scatter_cluster` — clustered scatter so flora forms groves, not a lawn.
//!
//! Worker-03 / WORLD pure core, round 17. Uniform per-cell scatter looks like
//! evenly-sprinkled noise; real vegetation clumps. A low-frequency cluster field
//! biases the per-cell place probability toward groves as `cluster_bias` rises,
//! while keeping the overall planted fraction roughly constant (the cluster
//! field averages out). At `bias = 0` it degrades to uniform scatter. Extends
//! `vegetation_scatter` (R12).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const BASE_RATE: f32 = 0.3; // mean planted fraction
const CLUSTER_FREQ: f32 = 0.05; // low-frequency grove field

/// Effective place probability at `cell`, blending a uniform base toward a
/// cluster field as `cluster_bias` (0..1) rises. Mean ≈ `BASE_RATE` for any
/// bias.
pub fn density(cell: [i32; 2], cluster_bias: f32, seed: u64) -> f32 {
    let bias = clamp01(cluster_bias);
    let cf = cluster_field(cell, seed); // [0,1], mean ≈ 0.5
    let clustered = (2.0 * cf) * BASE_RATE; // mean ≈ BASE_RATE, range [0, 2·base]
    clamp01(BASE_RATE * (1.0 - bias) + clustered * bias)
}

/// Whether `cell` gets a plant. Deterministic per cell; seed-sensitive.
pub fn place(cell: [i32; 2], cluster_bias: f32, seed: u64) -> bool {
    let r = unit(hash2(cell[0], cell[1], seed ^ 0x9111));
    r < density(cell, cluster_bias, seed)
}

fn cluster_field(cell: [i32; 2], seed: u64) -> f32 {
    value_noise(
        cell[0] as f32 * CLUSTER_FREQ,
        cell[1] as f32 * CLUSTER_FREQ,
        seed ^ 0xC1C1,
    )
}

fn value_noise(x: f32, z: f32, seed: u64) -> f32 {
    let x0 = x.floor();
    let z0 = z.floor();
    let xf = x - x0;
    let zf = z - z0;
    let (ix, iz) = (x0 as i32, z0 as i32);
    let v00 = corner(ix, iz, seed);
    let v10 = corner(ix + 1, iz, seed);
    let v01 = corner(ix, iz + 1, seed);
    let v11 = corner(ix + 1, iz + 1, seed);
    let sx = smoothstep(xf);
    let sz = smoothstep(zf);
    lerp(lerp(v00, v10, sx), lerp(v01, v11, sx), sz)
}

fn corner(ix: i32, iz: i32, seed: u64) -> f32 {
    unit(hash2(ix, iz, seed))
}

fn hash2(x: i32, z: i32, seed: u64) -> u64 {
    let mut h = seed ^ 0x9E37_79B9_7F4A_7C15;
    h = mix(h ^ (x as u32 as u64).wrapping_mul(0x85EB_CA6B));
    h = mix(h ^ (z as u32 as u64).wrapping_mul(0xC2B2_AE35));
    h
}

fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
fn unit(h: u64) -> f32 {
    ((h >> 40) as f32) / ((1u64 << 24) as f32)
}
fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    crate::util::mathx::lerp(a, b, t) // r342 dedup
}
fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    fn density_variance(bias: f32, seed: u64, span: i32) -> f32 {
        let mut vals = Vec::new();
        for x in 0..span {
            for z in 0..span {
                vals.push(density([x, z], bias, seed));
            }
        }
        let mean = vals.iter().sum::<f32>() / vals.len() as f32;
        vals.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / vals.len() as f32
    }

    #[test]
    fn bias_raises_clumping() {
        // Higher bias ⇒ more spatial variance in the density field (clumps).
        let v0 = density_variance(0.0, 5, 60);
        let v1 = density_variance(0.9, 5, 60);
        assert!(v1 > v0, "higher bias should increase clumping (variance)");
    }

    #[test]
    fn bias_zero_is_uniform() {
        // At bias 0, density is the constant base everywhere.
        for x in [-3, 0, 7, 50] {
            for z in [-9, 0, 12, 33] {
                assert!((density([x, z], 0.0, 1) - BASE_RATE).abs() < 1e-6);
            }
        }
        assert!(density_variance(0.0, 1, 40) < 1e-9);
    }

    #[test]
    fn deterministic_per_cell() {
        assert_eq!(place([3, 4], 0.7, 9), place([3, 4], 0.7, 9));
        assert_eq!(density([3, 4], 0.7, 9), density([3, 4], 0.7, 9));
    }

    #[test]
    fn seed_sensitive() {
        let mut differ = false;
        for x in 0..400 {
            if place([x, 0], 0.6, 1) != place([x, 0], 0.6, 2) {
                differ = true;
                break;
            }
        }
        assert!(differ);
    }

    #[test]
    fn roughly_base_fraction() {
        // Overall planted fraction ≈ BASE_RATE regardless of clustering.
        let span = 200;
        let mut hits = 0;
        for x in 0..span {
            for z in 0..span {
                if place([x, z], 0.8, 7) {
                    hits += 1;
                }
            }
        }
        let frac = hits as f32 / (span * span) as f32;
        assert!(frac > 0.2 && frac < 0.4, "fraction {frac} drifted from base");
    }

    #[test]
    fn density_bounded() {
        for x in 0..50 {
            for z in 0..50 {
                for bi in 0..=10 {
                    let d = density([x, z], bi as f32 / 10.0, 3);
                    assert!(d >= 0.0 && d <= 1.0);
                }
            }
        }
    }
}
