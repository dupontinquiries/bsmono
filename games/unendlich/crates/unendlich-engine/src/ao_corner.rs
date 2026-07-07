//! ao_corner.rs — pure voxel vertex ambient occlusion (classic 3-neighbour).
//!
//! The well-known "0fps" voxel AO: a vertex is shadowed by its two edge
//! neighbours and the diagonal corner. Returns one of four levels
//! `{0, 1/3, 2/3, 1}` (1 = fully lit). Pure, deterministic, panic-free
//! (R1/R11/R21).

/// Vertex AO from the three relevant neighbour occupancies. If both edge sides
/// are solid the vertex is fully occluded (`0`); otherwise it's
/// `(3 - occluders) / 3`.
pub fn ao(side1: bool, side2: bool, corner: bool) -> f32 {
    if side1 && side2 {
        return 0.0;
    }
    let occluders = side1 as u32 + side2 as u32 + corner as u32;
    (3 - occluders) as f32 / 3.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_is_full_light() {
        assert_eq!(ao(false, false, false), 1.0);
    }

    #[test]
    fn both_sides_is_zero() {
        assert_eq!(ao(true, true, false), 0.0);
        assert_eq!(ao(true, true, true), 0.0, "corner irrelevant when both sides solid");
    }

    #[test]
    fn one_side_steps_down() {
        assert!((ao(true, false, false) - 2.0 / 3.0).abs() < 1e-6);
        assert!((ao(false, true, false) - 2.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn corner_only_steps_down() {
        assert!((ao(false, false, true) - 2.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn four_distinct_levels() {
        let lit = ao(false, false, false); // 1
        let one = ao(true, false, false); // 2/3
        let two = ao(true, false, true); // 1/3 (side + corner)
        let occ = ao(true, true, false); // 0
        let mut levels = [lit, one, two, occ];
        levels.sort_by(|a, b| a.total_cmp(b));
        assert!((levels[0] - 0.0).abs() < 1e-6);
        assert!((levels[1] - 1.0 / 3.0).abs() < 1e-6);
        assert!((levels[2] - 2.0 / 3.0).abs() < 1e-6);
        assert!((levels[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn deterministic() {
        assert_eq!(ao(true, false, true), ao(true, false, true));
    }
}
