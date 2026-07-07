//! Boundary-vertex push to close a measured crack between part/LOD meshes,
//! capped at one voxel so it never over-pulls (handoff/BODY_PART_MESHING.md).

pub fn fill_offset(gap: f32, voxel_size: f32) -> f32 {
    let g = gap.max(0.0);
    let vs = voxel_size.max(0.0);
    (g * 0.5).min(vs)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_gap_no_offset() { assert_eq!(fill_offset(0.0, 1.0), 0.0); }
    #[test] fn bigger_gap_more() { assert!(fill_offset(0.5, 10.0) > fill_offset(0.1, 10.0)); }
    #[test] fn capped_at_voxel() { assert_eq!(fill_offset(100.0, 2.0), 2.0); }
    #[test] fn scales_with_voxel_cap() { assert!(fill_offset(100.0, 4.0) > fill_offset(100.0, 2.0)); }
    #[test] fn bounded_by_voxel() {
        let mut g = 0.0f32;
        while g < 50.0 { assert!(fill_offset(g, 3.0) <= 3.0); g += 1.0; }
    }
    #[test] fn deterministic() { assert_eq!(fill_offset(0.4, 2.0).to_bits(), fill_offset(0.4, 2.0).to_bits()); }
}
