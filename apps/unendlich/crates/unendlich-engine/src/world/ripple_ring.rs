//! `ripple_ring` — expanding ring height from a water impact over time.
//!
//! Worker-03 / WORLD-FX pure core, round 20. A splash sends a ring outward at a
//! constant speed; this returns the surface displacement in [0,1] at a given
//! radius and time. The crest sits at `radius = speed·t`, its amplitude decays
//! as the ring spreads, and water the ring hasn't reached yet is undisturbed.
//! Cosmetic FP32; pure and deterministic.
//!
//! Self-contained for `rustc --test`.
#![allow(dead_code)]

const DECAY: f32 = 2.0; // amplitude half-life-ish in time
const WIDTH: f32 = 2.0; // crest thickness

/// Ring displacement in [0,1] at `dist` from the impact, `t` seconds after it,
/// expanding at `speed`. Peaks where `dist == speed·t`, decaying with `t`.
pub fn ring(dist: f32, t: f32, speed: f32) -> f32 {
    let tt = t.max(0.0);
    let r = speed.max(0.0) * tt; // current crest radius
    let amp = 1.0 / (1.0 + tt / DECAY); // decays as the ring ages
    let d = dist - r; // signed distance from the crest
    let shape = (-(d * d) / (2.0 * WIDTH * WIDTH)).exp();
    clamp01(amp * shape)
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crest_is_at_speed_times_t() {
        let speed = 3.0;
        let t = 4.0;
        let r = speed * t; // 12
        let at_crest = ring(r, t, speed);
        // The crest is higher than points well off it.
        assert!(at_crest > ring(r + 6.0, t, speed));
        assert!(at_crest > ring(r - 6.0, t, speed));
    }

    #[test]
    fn amplitude_decays_with_time() {
        let speed = 3.0;
        let early = ring(speed * 1.0, 1.0, speed); // crest at t=1
        let late = ring(speed * 8.0, 8.0, speed); // crest at t=8
        assert!(early > late, "ring did not decay over time");
    }

    #[test]
    fn far_ahead_of_ring_is_zero() {
        let v = ring(1000.0, 2.0, 3.0); // way past the crest (r=6)
        assert!(v < 1e-3);
    }

    #[test]
    fn bounded() {
        for di in 0..100 {
            for ti in 0..50 {
                let v = ring(di as f32, ti as f32 * 0.2, 3.0);
                assert!(v >= 0.0 && v <= 1.0);
            }
        }
    }

    #[test]
    fn t_zero_disturbs_only_the_origin() {
        assert!(ring(0.0, 0.0, 3.0) > 0.5); // impact point
        assert!(ring(20.0, 0.0, 3.0) < 1e-3); // elsewhere undisturbed
    }

    #[test]
    fn deterministic() {
        assert_eq!(ring(5.0, 2.0, 3.0), ring(5.0, 2.0, 3.0));
    }
}
