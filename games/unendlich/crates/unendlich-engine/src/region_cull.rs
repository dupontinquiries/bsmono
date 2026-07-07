//! region_cull.rs — pure coarse region-vs-frustum test for multi-level culling.
//!
//! The top of the cull hierarchy (GPU_COMPUTE_PLAN #4): test a coarse region AABB
//! against the 6 frustum planes with the conservative p-vertex test, and an
//! optional octant mask to descend into children. Plane convention:
//! `(a,b,c)` is the inward normal and a point is inside when
//! `a·x + b·y + c·z + d >= 0`. Pure, deterministic, panic-free (R1/R11/R21).

/// Conservative visibility of the AABB `[rmin, rmax]` against 6 frustum
/// `planes`. Uses the p-vertex (the corner farthest along each inward normal):
/// if that corner is behind any plane the box is fully outside → not visible.
/// Straddling boxes count as visible (conservative).
pub fn region_visible(rmin: [f32; 3], rmax: [f32; 3], planes: &[[f32; 4]; 6]) -> bool {
    for p in planes.iter() {
        let px = if p[0] >= 0.0 { rmax[0] } else { rmin[0] };
        let py = if p[1] >= 0.0 { rmax[1] } else { rmin[1] };
        let pz = if p[2] >= 0.0 { rmax[2] } else { rmin[2] };
        if p[0] * px + p[1] * py + p[2] * pz + p[3] < 0.0 {
            return false; // even the most-inside corner is outside this plane
        }
    }
    true
}

/// Octant visibility mask for the 8 children of `[rmin, rmax]` (split at the
/// midpoint). Bit `c` is set when child octant `c` (x=bit0, y=bit1, z=bit2) is
/// potentially visible.
pub fn child_mask(rmin: [f32; 3], rmax: [f32; 3], planes: &[[f32; 4]; 6]) -> u8 {
    let mid = [
        (rmin[0] + rmax[0]) * 0.5,
        (rmin[1] + rmax[1]) * 0.5,
        (rmin[2] + rmax[2]) * 0.5,
    ];
    let mut mask = 0u8;
    for c in 0..8u8 {
        let (xlo, xhi) = if c & 1 == 0 { (rmin[0], mid[0]) } else { (mid[0], rmax[0]) };
        let (ylo, yhi) = if c & 2 == 0 { (rmin[1], mid[1]) } else { (mid[1], rmax[1]) };
        let (zlo, zhi) = if c & 4 == 0 { (rmin[2], mid[2]) } else { (mid[2], rmax[2]) };
        if region_visible([xlo, ylo, zlo], [xhi, yhi, zhi], planes) {
            mask |= 1 << c;
        }
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    // Axis-aligned frustum = box [-10,10]^3, inward normals.
    const FRUSTUM: [[f32; 4]; 6] = [
        [1.0, 0.0, 0.0, 10.0],  // x >= -10
        [-1.0, 0.0, 0.0, 10.0], // x <= 10
        [0.0, 1.0, 0.0, 10.0],  // y >= -10
        [0.0, -1.0, 0.0, 10.0], // y <= 10
        [0.0, 0.0, 1.0, 10.0],  // z >= -10
        [0.0, 0.0, -1.0, 10.0], // z <= 10
    ];

    #[test]
    fn inside_is_visible() {
        assert!(region_visible([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0], &FRUSTUM));
    }

    #[test]
    fn fully_outside_a_plane_is_invisible() {
        assert!(!region_visible([20.0, 0.0, 0.0], [30.0, 1.0, 1.0], &FRUSTUM), "right of frustum");
        assert!(!region_visible([0.0, 0.0, -30.0], [1.0, 1.0, -20.0], &FRUSTUM), "behind near");
    }

    #[test]
    fn straddling_is_visible() {
        assert!(region_visible([5.0, 5.0, 5.0], [15.0, 15.0, 15.0], &FRUSTUM), "straddles +corner");
    }

    #[test]
    fn plane_order_symmetric() {
        let region_min = [5.0, -3.0, 2.0];
        let region_max = [15.0, 4.0, 8.0];
        let base = region_visible(region_min, region_max, &FRUSTUM);
        // reverse the plane order; result must not change
        let mut rev = FRUSTUM;
        rev.reverse();
        assert_eq!(region_visible(region_min, region_max, &rev), base, "order-dependent");
    }

    #[test]
    fn large_region_containing_frustum_is_visible() {
        assert!(region_visible([-100.0, -100.0, -100.0], [100.0, 100.0, 100.0], &FRUSTUM));
    }

    #[test]
    fn child_mask_full_and_empty() {
        // a region fully inside -> all 8 children visible
        assert_eq!(child_mask([-2.0, -2.0, -2.0], [2.0, 2.0, 2.0], &FRUSTUM), 0xFF);
        // a region fully outside -> no children visible
        assert_eq!(child_mask([50.0, 50.0, 50.0], [60.0, 60.0, 60.0], &FRUSTUM), 0x00);
    }

    #[test]
    fn child_mask_partial() {
        // region centered so only the lower-x octants fall inside the frustum
        let m = child_mask([2.0, -2.0, -2.0], [22.0, 2.0, 2.0], &FRUSTUM);
        assert!(m != 0x00 && m != 0xFF, "expected a partial mask, got {:#010b}", m);
    }

    #[test]
    fn deterministic() {
        assert_eq!(
            region_visible([1.0, 1.0, 1.0], [2.0, 2.0, 2.0], &FRUSTUM),
            region_visible([1.0, 1.0, 1.0], [2.0, 2.0, 2.0], &FRUSTUM)
        );
    }
}
