//! `ember_field` — rising embers from a fire source.
//!
//! Worker-03 / WORLD-FX pure core, round 14. Each ember is a deterministic
//! function of time + a per-ember seed: it rises (and drifts slightly), its glow
//! flickers and fades toward the end of its life, and it dies once its (seeded)
//! lifetime elapses. Cosmetic FP32; pure and deterministic (no clock/rng). Ties
//! to fire/weather sources.
//!
//! Self-contained for `rustc --test`.
#![allow(dead_code)]

const LIFE_BASE: f32 = 2.0;
const LIFE_JITTER: f32 = 2.0; // lifetime ∈ [2,4)
const FLICKER_HZ: f32 = 8.0;
const TWO_PI: f32 = 6.283_185_5;

/// A single ember's state at time `t`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ember {
    pub pos: [f32; 3],
    pub glow: f32,
    pub alive: bool,
}

/// State of the ember identified by `seed` at time `t`, rising at `rise`
/// units/sec. Dead embers report `alive=false`, `glow=0`.
pub fn ember(t: f32, seed: u64, rise: f32) -> Ember {
    let life = LIFE_BASE + unit(hash(seed ^ 0xA)) * LIFE_JITTER;
    let tt = t.max(0.0);
    let alive = t >= 0.0 && tt < life;

    let ox = signed(hash(seed ^ 0xB)) * 0.5;
    let oz = signed(hash(seed ^ 0xC)) * 0.5;
    let drift_x = signed(hash(seed ^ 0xD)) * 0.2;
    let drift_z = signed(hash(seed ^ 0xE)) * 0.2;
    let pos = [ox + drift_x * tt, rise * tt, oz + drift_z * tt];

    let glow = if alive && life > 0.0 {
        let phase = unit(hash(seed ^ 0xF)) * TWO_PI;
        let flick = 0.5 + 0.5 * (tt * FLICKER_HZ + phase).sin(); // [0,1]
        let fade = (1.0 - tt / life).max(0.0); // 1 → 0 over the life
        clamp01(flick * fade)
    } else {
        0.0
    };

    Ember { pos, glow, alive }
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
fn signed(h: u64) -> f32 {
    unit(h) * 2.0 - 1.0
}
fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIFE_MAX: f32 = LIFE_BASE + LIFE_JITTER;

    #[test]
    fn rises_with_time() {
        let a = ember(0.5, 7, 3.0);
        let b = ember(1.0, 7, 3.0);
        assert!(b.pos[1] > a.pos[1], "ember did not rise");
    }

    #[test]
    fn glow_bounded_and_flickers() {
        let mut seen = std::collections::BTreeSet::new();
        for i in 0..50 {
            let t = i as f32 * 0.02;
            let g = ember(t, 3, 2.0).glow;
            assert!(g >= 0.0 && g <= 1.0);
            seen.insert((g * 1000.0) as i32);
        }
        assert!(seen.len() > 3, "glow did not vary (no flicker)");
    }

    #[test]
    fn dies_after_lifetime() {
        let e = ember(LIFE_MAX + 1.0, 11, 2.0);
        assert!(!e.alive);
        assert_eq!(e.glow, 0.0);
    }

    #[test]
    fn alive_then_dead_across_life() {
        // Early in life it's alive; well past max life it's dead.
        assert!(ember(0.01, 5, 2.0).alive);
        assert!(!ember(LIFE_MAX + 0.5, 5, 2.0).alive);
    }

    #[test]
    fn deterministic_per_seed() {
        assert_eq!(ember(1.3, 99, 2.5), ember(1.3, 99, 2.5));
    }

    #[test]
    fn seed_sensitive() {
        let a = ember(1.0, 1, 2.0);
        let b = ember(1.0, 2, 2.0);
        assert_ne!(a, b);
    }
}
