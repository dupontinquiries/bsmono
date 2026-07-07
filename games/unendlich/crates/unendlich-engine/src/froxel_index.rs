//! froxel_index.rs — pure froxel-volume clustered indexing for the fill pass.
//!
//! Maps a screen `uv` + view-space `depth` to a flat froxel (frustum-voxel)
//! index, using logarithmic depth slicing so there are more, thinner slices near
//! the camera. Pure, deterministic, panic-free (R1/R11/R21).

/// Logarithmic depth slice for `depth` given `slices` total, between `near` and
/// `far`. Returns `0` at the near plane and `slices-1` at the far plane;
/// monotonically non-decreasing in depth. `slices == 0` yields `0`.
pub fn slice_of(depth: f32, slices: u32, near: f32, far: f32) -> u32 {
    if slices == 0 {
        return 0;
    }
    let near = near.max(1e-4);
    let far = far.max(near * (1.0 + 1e-4));
    let d = depth.max(near).min(far);
    // s = floor( ln(d/near) / ln(far/near) * slices )
    let ratio = (d / near).ln() / (far / near).ln(); // in [0, 1]
    let s = (ratio * slices as f32).floor();
    let s = if s.is_finite() { s } else { 0.0 };
    let max = slices as i64 - 1;
    let si = s as i64;
    (if si < 0 { 0 } else if si > max { max } else { si }) as u32
}

/// Flat froxel index for a screen `uv` (each in `[0,1]`) at view-space `depth`,
/// over a `[nx, ny, nz]` cluster grid. Layout is `(z*ny + y)*nx + x`, always
/// `< nx*ny*nz`.
pub fn froxel_of(uv: [f32; 2], depth: f32, dims: [u32; 3], near: f32, far: f32) -> u32 {
    let nx = dims[0].max(1);
    let ny = dims[1].max(1);
    let nz = dims[2].max(1);

    let cell = |c: f32, n: u32| -> u32 {
        let scaled = (c.max(0.0).min(1.0) * n as f32).floor();
        let v = scaled as i64;
        let max = n as i64 - 1;
        (if v < 0 { 0 } else if v > max { max } else { v }) as u32
    };

    let x = cell(uv[0], nx);
    let y = cell(uv[1], ny);
    let z = slice_of(depth, nz, near, far);
    (z * ny + y) * nx + x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / ((1u32 << 24) as f32)
    }

    #[test]
    fn near_is_slice_zero() {
        assert_eq!(slice_of(0.1, 16, 0.1, 1000.0), 0);
        assert_eq!(slice_of(0.05, 16, 0.1, 1000.0), 0, "before near clamps to 0");
    }

    #[test]
    fn far_is_last_slice() {
        assert_eq!(slice_of(1000.0, 16, 0.1, 1000.0), 15);
        assert_eq!(slice_of(1e9, 16, 0.1, 1000.0), 15, "beyond far clamps to last");
    }

    #[test]
    fn prop_monotonic_depth_to_slice() {
        let (slices, near, far) = (24u32, 0.5f32, 2000.0f32);
        let mut prev = 0u32;
        let mut d = near;
        while d <= far {
            let s = slice_of(d, slices, near, far);
            assert!(s >= prev, "slice decreased at depth {}", d);
            assert!(s < slices);
            prev = s;
            d *= 1.02;
        }
    }

    #[test]
    fn prop_index_within_range() {
        let dims = [12u32, 7, 16];
        let total = dims[0] * dims[1] * dims[2];
        let mut seed = 0xF00D_0001u32;
        for _ in 0..2000 {
            let uv = [lcg(&mut seed) * 1.4 - 0.2, lcg(&mut seed) * 1.4 - 0.2]; // includes out-of-range
            let depth = lcg(&mut seed) * 3000.0 - 100.0;
            let idx = froxel_of(uv, depth, dims, 0.1, 1000.0);
            assert!(idx < total, "index {} >= total {}", idx, total);
        }
        // corner cases
        assert!(froxel_of([0.0, 0.0], 0.1, dims, 0.1, 1000.0) < total);
        assert!(froxel_of([1.0, 1.0], 1000.0, dims, 0.1, 1000.0) < total);
    }

    #[test]
    fn log_distribution_near_slices_thinner() {
        // slice boundary depth s = near*(far/near)^(s/N); near slices span less depth
        let (n, near, far) = (16u32, 0.1f32, 1000.0f32);
        let boundary = |s: u32| near * (far / near).powf(s as f32 / n as f32);
        let first_width = boundary(1) - boundary(0);
        let last_width = boundary(n) - boundary(n - 1);
        assert!(first_width < last_width, "near slice not thinner ({} !< {})", first_width, last_width);
        // and the slicer agrees: a thin near band still resolves to slice 0/1
        assert_eq!(slice_of(boundary(0) + first_width * 0.5, n, near, far), 0);
    }

    #[test]
    fn deterministic() {
        assert_eq!(slice_of(42.0, 16, 0.1, 1000.0), slice_of(42.0, 16, 0.1, 1000.0));
        assert_eq!(
            froxel_of([0.3, 0.7], 50.0, [8, 8, 16], 0.1, 1000.0),
            froxel_of([0.3, 0.7], 50.0, [8, 8, 16], 0.1, 1000.0)
        );
    }
}
