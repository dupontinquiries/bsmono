//! spark_burst.rs — pure spark-particle offset + life from a burst seed.
//!
//! Each spark of a burst gets a deterministic radial direction and speed from
//! its index + seed; position spreads radially over time (with gravity on Y) and
//! alpha fades to 0 over its life. Pure, deterministic, panic-free (R1/R11/R21).

use std::f32::consts::PI;

const LIFE: f32 = 1.5;
const GRAVITY: f32 = 4.0;

// r193 (guide 4.1): salt-fold + splitmix-to-unit == mix64_unit of the
// combined seed. Byte-identical (mix64 adds the same GOLDEN constant).
fn hash01(seed: u64, salt: u64) -> f32 {
    crate::util::hash_util::mix64_unit(seed.wrapping_add(salt.wrapping_mul(0x9E37_79B9_7F4A_7C15)))
}

/// Position and alpha of spark `idx` at time `t` for burst `seed`. Direction is
/// uniform-ish on a sphere; X/Z are pure radial (∝ t), Y also falls under
/// gravity; alpha fades linearly to 0 at `LIFE`.
pub fn spark(idx: u32, t: f32, seed: u64) -> ([f32; 3], f32) {
    let h_az = hash01(seed, idx as u64 * 3);
    let h_z = hash01(seed, idx as u64 * 3 + 1);
    let h_sp = hash01(seed, idx as u64 * 3 + 2);

    let az = h_az * 2.0 * PI;
    let z = h_z * 2.0 - 1.0; // [-1,1]
    let r = (1.0 - z * z).max(0.0).sqrt();
    let dir = [r * az.cos(), z, r * az.sin()]; // ~unit
    let speed = 2.0 + 3.0 * h_sp;

    let tt = t.max(0.0);
    let pos = [
        dir[0] * speed * tt,
        dir[1] * speed * tt - 0.5 * GRAVITY * tt * tt,
        dir[2] * speed * tt,
    ];
    let alpha = (1.0 - tt / LIFE).max(0.0).min(1.0);
    (pos, alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn horiz(p: [f32; 3]) -> f32 {
        (p[0] * p[0] + p[2] * p[2]).sqrt()
    }

    #[test]
    fn radial_spread_grows_with_time() {
        for idx in 0..16u32 {
            let near = horiz(spark(idx, 0.2, 42).0);
            let far = horiz(spark(idx, 0.6, 42).0);
            assert!(far > near, "spark {} didn't spread radially", idx);
        }
    }

    #[test]
    fn alpha_fades_to_zero() {
        assert!((spark(0, 0.0, 1).1 - 1.0).abs() < 1e-6, "full at birth");
        assert_eq!(spark(0, LIFE, 1).1, 0.0, "zero at end of life");
        assert_eq!(spark(0, 10.0, 1).1, 0.0, "stays zero past life");
    }

    #[test]
    fn per_idx_distinct() {
        let a = spark(0, 0.5, 7).0;
        let b = spark(1, 0.5, 7).0;
        assert!(a != b, "different indices should differ");
    }

    #[test]
    fn deterministic_per_seed() {
        assert_eq!(spark(3, 0.5, 99), spark(3, 0.5, 99));
    }

    #[test]
    fn bounded_finite() {
        for idx in 0..64u32 {
            let mut t = 0.0f32;
            while t <= LIFE {
                let (p, a) = spark(idx, t, 5);
                assert!(p[0].is_finite() && p[1].is_finite() && p[2].is_finite());
                assert!((0.0..=1.0).contains(&a));
                // within life, horizontal travel is bounded by max speed * life
                assert!(horiz(p) <= 5.0 * LIFE + 1e-3, "spark flew too far");
                t += 0.1;
            }
        }
    }

    #[test]
    fn seed_sensitive() {
        assert!(spark(0, 0.5, 1).0 != spark(0, 0.5, 2).0, "not sensitive to seed");
    }
}
