//! chromatic_aberration.rs — pure per-channel radial UV offset.
//!
//! Radial UV offset that grows from 0 at the screen centre toward the edges,
//! used to split the R/B channels (sample `+offset` and `-offset`). Pure,
//! deterministic, panic-free (R1/R11/R21).

/// Radial UV offset at `uv` (each in `[0,1]`) for `strength`. Zero at the centre
/// `(0.5,0.5)`, growing radially toward the edges, clamped per component to
/// `±0.5`.
pub fn offset(uv: [f32; 2], strength: f32) -> [f32; 2] {
    let dx = uv[0] - 0.5;
    let dy = uv[1] - 0.5;
    let ox = dx * strength;
    let oy = dy * strength;
    [ox.max(-0.5).min(0.5), oy.max(-0.5).min(0.5)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mag(o: [f32; 2]) -> f32 {
        (o[0] * o[0] + o[1] * o[1]).sqrt()
    }

    #[test]
    fn center_is_zero() {
        assert_eq!(offset([0.5, 0.5], 0.1), [0.0, 0.0]);
    }

    #[test]
    fn edge_is_max() {
        let center = mag(offset([0.55, 0.5], 0.2));
        let edge = mag(offset([0.0, 0.0], 0.2)); // corner = farthest
        assert!(edge > center, "edge should offset more than near-centre");
    }

    #[test]
    fn strength_scales() {
        let a = offset([0.2, 0.8], 0.1);
        let b = offset([0.2, 0.8], 0.2);
        assert!((b[0] - 2.0 * a[0]).abs() < 1e-6 && (b[1] - 2.0 * a[1]).abs() < 1e-6);
    }

    #[test]
    fn bounded() {
        let mut seed = 0x2417_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            let o = offset([lcg() * 2.0 - 0.5, lcg() * 2.0 - 0.5], lcg() * 10.0);
            assert!(o[0].abs() <= 0.5 + 1e-6 && o[1].abs() <= 0.5 + 1e-6, "offset oob {:?}", o);
        }
    }

    #[test]
    fn radial_direction() {
        // offset is parallel to (uv - center): cross product ~ 0
        let uv = [0.3f32, 0.7];
        let o = offset(uv, 0.2);
        let dx = uv[0] - 0.5;
        let dy = uv[1] - 0.5;
        assert!((dx * o[1] - dy * o[0]).abs() < 1e-6, "offset not radial");
    }

    #[test]
    fn deterministic() {
        assert_eq!(offset([0.3, 0.7], 0.15), offset([0.3, 0.7], 0.15));
    }
}
