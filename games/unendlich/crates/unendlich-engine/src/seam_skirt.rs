//! Skirt depth to hide the vertical step between two adjacent LOD levels so no
//! crack/gap shows at a part or chunk boundary (handoff/BODY_PART_MESHING.md).
//! Symmetric in the two levels; equal levels ⇒ 0 (no skirt needed).

pub fn skirt_depth(lod_a: u8, lod_b: u8, voxel_size: f32) -> f32 {
    let vs = voxel_size.max(0.0);
    let step = (lod_a as i32 - lod_b as i32).unsigned_abs();
    let factor = ((1u32 << step.min(16)) - 1) as f32; // (2^step - 1) coarse voxels
    vs * factor
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn equal_levels_no_skirt() { assert_eq!(skirt_depth(2, 2, 1.0), 0.0); }
    #[test] fn bigger_gap_deeper() { assert!(skirt_depth(0, 3, 1.0) > skirt_depth(0, 1, 1.0)); }
    #[test] fn scales_with_voxel() { assert!((skirt_depth(0, 2, 2.0) - 2.0 * skirt_depth(0, 2, 1.0)).abs() < 1e-6); }
    #[test] fn symmetric() { assert_eq!(skirt_depth(1, 4, 1.5), skirt_depth(4, 1, 1.5)); }
    #[test] fn bounded_finite() { assert!(skirt_depth(0, 255, 1.0).is_finite()); }
    #[test] fn deterministic() { assert_eq!(skirt_depth(0, 3, 1.0).to_bits(), skirt_depth(0, 3, 1.0).to_bits()); }
}
