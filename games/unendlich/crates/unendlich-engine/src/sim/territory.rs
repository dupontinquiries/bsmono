//! `sim::territory` — how deep an intruder is into a creature's territory.
//!
//! [`intrusion`] returns `0` outside the territory and rises linearly to `1`
//! right at the home point, so a defender can scale its response to how far in
//! the trespasser has come. The territory is a circle of `radius` around `home`;
//! a larger radius means the same absolute distance counts as less of an
//! intrusion. A non-positive radius is treated as no territory.
//!
//! Pure and deterministic: a distance and a clamped linear ramp — no clock, no
//! randomness, no cross-imports.

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

/// Intrusion strength at `pos` for a territory of `radius` around `home`.
///
/// `1` at the home point, ramping linearly to `0` at the radius and staying `0`
/// beyond it. Returns `0` for a non-positive radius.
pub fn intrusion(pos: [f32; 2], home: [f32; 2], radius: f32) -> f32 {
    if !(radius > 0.0) {
        return 0.0;
    }
    let dx = pos[0] - home[0];
    let dy = pos[1] - home[1];
    let d = (dx * dx + dy * dy).sqrt();
    clamp01(1.0 - d / radius)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn at_home_is_one() {
        assert_eq!(intrusion([5.0, 5.0], [5.0, 5.0], 10.0), 1.0);
    }

    #[test]
    fn outside_is_zero() {
        // beyond the radius
        assert_eq!(intrusion([100.0, 0.0], [0.0, 0.0], 10.0), 0.0);
        // exactly at the edge
        assert_eq!(intrusion([10.0, 0.0], [0.0, 0.0], 10.0), 0.0);
    }

    #[test]
    fn monotone_inward() {
        let home = [0.0, 0.0];
        let r = 10.0;
        let near = intrusion([2.0, 0.0], home, r);
        let mid = intrusion([5.0, 0.0], home, r);
        let far = intrusion([8.0, 0.0], home, r);
        assert!(near > mid && mid > far, "{} {} {}", near, mid, far);
    }

    #[test]
    fn radius_scales() {
        let home = [0.0, 0.0];
        let pos = [5.0, 0.0];
        // a larger territory makes the same point count as more intruded
        assert!(intrusion(pos, home, 20.0) > intrusion(pos, home, 10.0));
    }

    #[test]
    fn bounded_and_radius_guard() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..4000 {
            let pos = [
                (lcg(&mut seed) % 2000) as f32 - 1000.0,
                (lcg(&mut seed) % 2000) as f32 - 1000.0,
            ];
            let home = [
                (lcg(&mut seed) % 2000) as f32 - 1000.0,
                (lcg(&mut seed) % 2000) as f32 - 1000.0,
            ];
            let r = (lcg(&mut seed) % 2000) as f32 / 10.0 - 50.0; // incl <= 0
            let v = intrusion(pos, home, r);
            assert!(v >= 0.0 && v <= 1.0, "intrusion {}", v);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let pos = [(lcg(&mut seed) % 200) as f32, (lcg(&mut seed) % 200) as f32];
            let home = [(lcg(&mut seed) % 200) as f32, (lcg(&mut seed) % 200) as f32];
            let r = (lcg(&mut seed) % 200) as f32;
            assert_eq!(intrusion(pos, home, r), intrusion(pos, home, r));
        }
    }
}
