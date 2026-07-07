//! decal_uv.rs — pure decal projection to UV.
//!
//! Projects a surface-local position (relative to the decal centre, in world
//! units) into the decal's `[0,1]` UV space, clamped, with a `covers` helper to
//! flag positions outside the footprint. Pure, deterministic, panic-free
//! (R1/R11/R21).

/// UV for surface-local `local` (relative to the decal centre) given decal
/// `size`. Centre → `0.5`, edges (`±size/2`) → `0`/`1`, outside clamps to the
/// border. Result in `[0,1]`.
pub fn decal_uv(local: [f32; 2], size: f32) -> [f32; 2] {
    let s = size.max(1e-4);
    let u = local[0] / s + 0.5;
    let v = local[1] / s + 0.5;
    [u.max(0.0).min(1.0), v.max(0.0).min(1.0)]
}

/// Whether `local` lies within the decal footprint (`|x|,|y| <= size/2`). Use to
/// discard fragments outside the decal rather than smearing the clamped border.
pub fn covers(local: [f32; 2], size: f32) -> bool {
    let half = size.max(1e-4) * 0.5;
    local[0].abs() <= half && local[1].abs() <= half
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_is_half() {
        let uv = decal_uv([0.0, 0.0], 2.0);
        assert!((uv[0] - 0.5).abs() < 1e-6 && (uv[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn edges_are_zero_and_one() {
        let s = 2.0f32;
        assert!((decal_uv([-s / 2.0, 0.0], s)[0] - 0.0).abs() < 1e-6);
        assert!((decal_uv([s / 2.0, 0.0], s)[0] - 1.0).abs() < 1e-6);
        assert!((decal_uv([0.0, -s / 2.0], s)[1] - 0.0).abs() < 1e-6);
        assert!((decal_uv([0.0, s / 2.0], s)[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn outside_clamps_and_flagged() {
        let uv = decal_uv([10.0, -10.0], 2.0);
        assert_eq!(uv, [1.0, 0.0], "outside should clamp to border");
        assert!(!covers([10.0, 0.0], 2.0), "outside should not be covered");
        assert!(covers([0.3, -0.4], 2.0), "inside should be covered");
    }

    #[test]
    fn size_scales() {
        // bigger decal -> same local maps closer to centre
        let small = decal_uv([0.5, 0.0], 2.0)[0];
        let big = decal_uv([0.5, 0.0], 8.0)[0];
        assert!((big - 0.5).abs() < (small - 0.5).abs(), "bigger size should map closer to centre");
    }

    #[test]
    fn in_unit_range() {
        let mut seed = 0x2007_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            let uv = decal_uv([lcg() * 20.0 - 10.0, lcg() * 20.0 - 10.0], lcg() * 5.0);
            assert!((0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]), "uv oob {:?}", uv);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(decal_uv([0.3, -0.2], 2.0), decal_uv([0.3, -0.2], 2.0));
    }
}
