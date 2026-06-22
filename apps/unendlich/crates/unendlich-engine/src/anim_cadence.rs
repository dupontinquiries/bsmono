//! anim_cadence.rs — pure animation-cadence downsampling (GPU-load lever).
//!
//! Slow and/or distant motions can be updated every N frames and interpolated
//! between updates, while fast motions stay full-rate. Decides N from motion
//! speed and distance, and provides the sub-frame interpolation. Pure,
//! deterministic, panic-free (R1/R11/R21).

/// Maximum frames between animation updates.
pub const MAX_N: u32 = 8;

/// How many frames to wait between full animation updates: `1` (full rate) for
/// fast motion, growing with distance and falling with motion speed, capped at
/// [`MAX_N`]. Always `>= 1`.
pub fn update_every_n(motion_speed: f32, dist: f32) -> u32 {
    let speed = motion_speed.max(0.0);
    let d = dist.max(0.0);
    // farther -> larger N; faster -> smaller N
    let n_f = (1.0 + d / 16.0) / (1.0 + speed * 4.0);
    let n = n_f.floor();
    let clamped = if n < 1.0 {
        1.0
    } else if n > MAX_N as f32 {
        MAX_N as f32
    } else {
        n
    };
    clamped as u32
}

/// Sub-frame interpolation between the last update (`prev`) and the next
/// (`target`). `sub` is clamped to `[0,1]`; the endpoints are exact
/// (`sub == 0 → prev`, `sub == 1 → target`).
pub fn downsampled_lerp(prev: f32, target: f32, sub: f32) -> f32 {
    let t = sub.max(0.0).min(1.0);
    if t == 0.0 {
        prev
    } else if t == 1.0 {
        target
    } else {
        prev + (target - prev) * t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_distant_gives_larger_n() {
        let near_slow = update_every_n(0.0, 0.0);
        let far_slow = update_every_n(0.0, 200.0);
        assert!(far_slow > near_slow, "distant slow motion should update less often");
        assert!(far_slow > 1, "far/slow should downsample");
    }

    #[test]
    fn fast_motion_is_full_rate() {
        for &d in &[0.0f32, 50.0, 200.0] {
            assert_eq!(update_every_n(20.0, d), 1, "fast motion should be full-rate at d={}", d);
        }
    }

    #[test]
    fn n_at_least_one() {
        let mut seed = 0xA417_0001u32;
        let mut lcg = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / ((1u32 << 24) as f32)
        };
        for _ in 0..5000 {
            let n = update_every_n(lcg() * 30.0 - 5.0, lcg() * 2000.0 - 100.0);
            assert!(n >= 1 && n <= MAX_N, "N out of range: {}", n);
        }
    }

    #[test]
    fn lerp_endpoints_exact() {
        assert_eq!(downsampled_lerp(0.1, 0.9, 0.0), 0.1);
        assert_eq!(downsampled_lerp(0.1, 0.9, 1.0), 0.9);
        // and clamps beyond the ends
        assert_eq!(downsampled_lerp(2.0, 5.0, -1.0), 2.0);
        assert_eq!(downsampled_lerp(2.0, 5.0, 3.0), 5.0);
        // midpoint
        assert!((downsampled_lerp(0.0, 10.0, 0.5) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn n_monotone_in_distance() {
        let mut prev = 0u32;
        let mut d = 0.0f32;
        while d <= 1000.0 {
            let n = update_every_n(0.5, d);
            assert!(n >= prev, "N decreased with distance at {}", d);
            prev = n;
            d += 4.0;
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(update_every_n(1.0, 100.0), update_every_n(1.0, 100.0));
        assert_eq!(downsampled_lerp(1.0, 2.0, 0.3), downsampled_lerp(1.0, 2.0, 0.3));
    }
}
