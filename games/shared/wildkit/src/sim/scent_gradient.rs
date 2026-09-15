//! `sim::scent_gradient` — turn scent readings at the four neighbouring cells
//! into a direction to follow (follow-on to `scent_track`/`scent_decay`).
//!
//! [`gradient_dir`] returns a 2D vector pointing toward stronger scent, built
//! from how much each neighbour exceeds the current cell `here`: only neighbours
//! stronger than `here` pull, in their own direction, so a creature climbs the
//! gradient. A uniform field (or balanced opposing neighbours) yields no
//! direction; a stronger east reading points `+x`. The vector is clamped to unit
//! length, so it is a bounded steering hint rather than a raw magnitude.
//!
//! Pure and deterministic: differences and a length clamp — no clock, no
//! randomness, no cross-imports. A GPU kernel sampling a scent field per cell
//! mirrors this.

fn pos(x: f32) -> f32 {
    if x > 0.0 {
        x
    } else {
        0.0
    }
}

/// Direction toward stronger scent from the four neighbours.
///
/// `x = pos(e - here) - pos(w - here)`, `y = pos(n - here) - pos(s - here)`,
/// clamped to unit length. `[0,0]` when no neighbour beats `here` or opposing
/// pulls cancel.
pub fn gradient_dir(here: f32, n: f32, e: f32, s: f32, w: f32) -> [f32; 2] {
    let x = pos(e - here) - pos(w - here);
    let y = pos(n - here) - pos(s - here);
    let mag = (x * x + y * y).sqrt();
    if mag > 1.0 {
        [x / mag, y / mag]
    } else {
        [x, y]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn uniform_is_zero() {
        assert_eq!(gradient_dir(5.0, 5.0, 5.0, 5.0, 5.0), [0.0, 0.0]);
        // uniform neighbours (here lower) still cancel
        assert_eq!(gradient_dir(0.0, 3.0, 3.0, 3.0, 3.0), [0.0, 0.0]);
    }

    #[test]
    fn stronger_east_points_plus_x() {
        let d = gradient_dir(0.0, 0.0, 1.0, 0.0, 0.0);
        assert!(d[0] > 0.0 && d[1].abs() < 1e-6, "{:?}", d);
    }

    #[test]
    fn symmetric_swap_negates_x() {
        // swapping the east/west readings flips the x direction
        let d1 = gradient_dir(0.0, 0.2, 0.8, 0.1, 0.3);
        let d2 = gradient_dir(0.0, 0.2, 0.3, 0.1, 0.8);
        assert!((d1[0] + d2[0]).abs() < 1e-6, "{} vs {}", d1[0], d2[0]);
        assert!((d1[1] - d2[1]).abs() < 1e-6);
    }

    #[test]
    fn normalized_ish_bound() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..5000 {
            let here = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let n = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let e = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let s = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let w = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let d = gradient_dir(here, n, e, s, w);
            let mag = (d[0] * d[0] + d[1] * d[1]).sqrt();
            assert!(mag <= 1.0 + 1e-5, "magnitude {}", mag);
        }
    }

    #[test]
    fn ties_cancel() {
        // equal opposing neighbours → no net direction
        assert_eq!(gradient_dir(0.0, 0.5, 0.7, 0.5, 0.7), [0.0, 0.0]);
        // peak: here stronger than all neighbours → no uphill
        assert_eq!(gradient_dir(9.0, 1.0, 2.0, 3.0, 4.0), [0.0, 0.0]);
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let here = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let n = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let e = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let s = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let w = (lcg(&mut seed) % 1000) as f32 / 100.0;
            assert_eq!(gradient_dir(here, n, e, s, w), gradient_dir(here, n, e, s, w));
        }
    }
}
