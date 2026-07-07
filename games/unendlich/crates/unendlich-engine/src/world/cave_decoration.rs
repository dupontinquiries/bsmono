//! `cave_decoration` — stalactite / stalagmite lengths in a cave cell.
//!
//! Worker-03 / WORLD pure core, round 18. Given a cell's distance to the cave
//! ceiling and floor, returns the length of a stalactite (hanging from the
//! ceiling) and a stalagmite (rising from the floor). Each grows longer the
//! closer the cell is to its surface and is jittered per seed for variety; a
//! cell mid-air between far surfaces grows little. Decorates `cave_room` (R17)
//! and `cave_carve` (R11) interiors.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const MAX_LEN: f32 = 4.0; // longest a formation can grow
const SCALE: f32 = 2.0; // distance scale of the growth falloff

/// `(stalactite_len, stalagmite_len)` for a cell `ceiling_dist` below the
/// ceiling and `floor_dist` above the floor. Each in `[0, MAX_LEN)`.
pub fn deco(ceiling_dist: f32, floor_dist: f32, seed: u64) -> (f32, f32) {
    let tite = MAX_LEN * falloff(ceiling_dist) * jitter(seed ^ 0x7411);
    let mite = MAX_LEN * falloff(floor_dist) * jitter(seed ^ 0x9122);
    (tite, mite)
}

/// 1 at distance 0, decaying toward 0 with distance.
fn falloff(d: f32) -> f32 {
    let dd = d.max(0.0);
    1.0 / (1.0 + dd / SCALE)
}

/// Per-seed length variation in [0.5, 1.0).
fn jitter(seed: u64) -> f32 {
    0.5 + unit(hash(seed)) * 0.5
}

fn hash(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
fn unit(h: u64) -> f32 {
    ((h >> 40) as f32) / ((1u64 << 24) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_ceiling_grows_stalactite() {
        // Close to ceiling, far from floor → tite ≫ mite.
        let (tite, mite) = deco(0.2, 20.0, 5);
        assert!(tite > mite);
        // and a closer ceiling gives a longer tite
        assert!(deco(0.1, 20.0, 5).0 > deco(5.0, 20.0, 5).0);
    }

    #[test]
    fn near_floor_grows_stalagmite() {
        let (tite, mite) = deco(20.0, 0.2, 5);
        assert!(mite > tite);
        assert!(deco(20.0, 0.1, 5).1 > deco(20.0, 5.0, 5).1);
    }

    #[test]
    fn mid_air_is_small() {
        let (tite, mite) = deco(20.0, 20.0, 5);
        assert!(tite < 0.5 && mite < 0.5);
    }

    #[test]
    fn deterministic_per_seed() {
        assert_eq!(deco(1.0, 3.0, 99), deco(1.0, 3.0, 99));
    }

    #[test]
    fn bounded() {
        for cd in 0..30 {
            for fd in 0..30 {
                let (t, m) = deco(cd as f32 * 0.5, fd as f32 * 0.5, 3);
                assert!(t >= 0.0 && t < MAX_LEN);
                assert!(m >= 0.0 && m < MAX_LEN);
            }
        }
    }

    #[test]
    fn seed_sensitive() {
        assert_ne!(deco(1.0, 1.0, 1), deco(1.0, 1.0, 2));
    }
}
