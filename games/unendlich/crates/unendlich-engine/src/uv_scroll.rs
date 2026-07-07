//! uv_scroll.rs — pure animated UV scroll for water/lava.
//!
//! Offsets UVs by `vel * t` and wraps the result into `[0,1)`. Pure,
//! deterministic, panic-free (R1/R11/R21).

fn wrap01(v: f32) -> f32 {
    let f = v - v.floor(); // in [0,1) for all finite v (handles negatives)
    if f >= 1.0 {
        0.0
    } else {
        f
    }
}

/// Scroll `uv` by `vel * t`, wrapped into `[0,1)`.
pub fn scroll(uv: [f32; 2], vel: [f32; 2], t: f32) -> [f32; 2] {
    [wrap01(uv[0] + vel[0] * t), wrap01(uv[1] + vel[1] * t)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_zero_is_identity() {
        let uv = [0.3f32, 0.7];
        assert_eq!(scroll(uv, [0.5, -0.2], 0.0), uv);
    }

    #[test]
    fn advances_by_vel_times_t() {
        // unwrapped advance equals vel*t (no wrap in this range)
        let r = scroll([0.1, 0.1], [0.05, 0.02], 4.0); // -> [0.3, 0.18]
        assert!((r[0] - 0.3).abs() < 1e-5 && (r[1] - 0.18).abs() < 1e-5, "advance wrong: {:?}", r);
    }

    #[test]
    fn wraps_into_unit_range() {
        let mut seed = 0x1937_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..3000 {
            let r = scroll([lcg() * 4.0 - 2.0, lcg() * 4.0 - 2.0], [lcg() * 2.0 - 1.0, lcg() * 2.0 - 1.0], lcg() * 100.0);
            assert!(r[0] >= 0.0 && r[0] < 1.0 && r[1] >= 0.0 && r[1] < 1.0, "not wrapped: {:?}", r);
        }
    }

    #[test]
    fn zero_velocity_is_static() {
        let uv = [0.42f32, 0.13];
        for &t in &[0.0f32, 1.0, 100.0] {
            assert_eq!(scroll(uv, [0.0, 0.0], t), uv);
        }
    }

    #[test]
    fn continuous_away_from_wrap() {
        // staying inside [0,1) so no wrap occurs, small dt => small change
        let mut prev = scroll([0.1, 0.1], [0.05, 0.05], 0.0);
        let mut t = 0.0f32;
        while t <= 10.0 {
            let cur = scroll([0.1, 0.1], [0.05, 0.05], t);
            assert!((cur[0] - prev[0]).abs() < 0.05 && (cur[1] - prev[1]).abs() < 0.05, "jump at t={}", t);
            prev = cur;
            t += 0.05;
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(scroll([0.2, 0.8], [0.3, 0.1], 2.5), scroll([0.2, 0.8], [0.3, 0.1], 2.5));
    }
}
