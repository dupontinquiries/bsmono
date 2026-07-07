//! `sim::frustum_plane_extract` — derive the six frustum planes from a
//! view-projection matrix (the missing pure half of R33's
//! [`super::chunk_frustum_cull`]).
//!
//! R33 culls boxes against six planes in the convention
//! `a·x + b·y + c·z + d >= 0` ⇒ *inside*. This module produces exactly those
//! planes from the camera matrix via the **Gribb–Hartmann** method, so the two
//! together form a complete CPU cull stage with no engine glue: feed
//! `extract(&viewproj)` straight into `chunk_frustum_cull` (its `[[f32;4];6]`
//! plane is `[a, b, c, d]` per plane).
//!
//! ## Conventions (matched to the engine)
//! - **Row-major** input: `viewproj[i]` is row `i`, and the clip coordinate is
//!   `clip[i] = Σⱼ viewproj[i][j]·p[j]` for `p = (x, y, z, 1)`. So
//!   `clip = (row0·p, row1·p, row2·p, row3·p)`.
//! - **Zero-to-one depth** (Vulkan, the engine builds projections with
//!   `perspective_rh_zo`): the depth clip test is `0 <= z_clip <= w`, which makes
//!   the **near** plane `row2` (not `row3 + row2` as in the OpenGL `-1..1`
//!   variant) and the **far** plane `row3 − row2`.
//!
//! Combining the clip-space inequalities `−w ≤ x ≤ w`, `−w ≤ y ≤ w`,
//! `0 ≤ z ≤ w` (each written as `… >= 0`) gives the inside-positive planes:
//!
//! ```text
//!   left   = row3 + row0      right = row3 − row0
//!   bottom = row3 + row1      top   = row3 − row1
//!   near   = row2             far   = row3 − row2
//! ```
//!
//! Each is then normalized so `(a, b, c)` is unit length; dividing by the
//! positive normal magnitude preserves the inside-positive sign. (The engine's
//! Vulkan Y-flip is already baked into `viewproj`, so it is handled
//! automatically — it only swaps which plane is geometrically "top" vs
//! "bottom"; both remain correct inside-positive frustum planes, and culling is
//! orientation-agnostic.)
//!
//! Pure, deterministic, finite. No `crate::render`, no `ash`/`vk`, no IO, no
//! clock, no RNG, no `.unwrap()`/`.expect()` in non-test code.

/// A plane `a·x + b·y + c·z + d >= 0` ⇒ inside (matches `chunk_frustum_cull`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
}

/// Build a plane from raw coefficients and normalize so `(a,b,c)` is unit
/// length. A (degenerate) zero-length normal is left unscaled rather than
/// producing NaN/inf.
fn normalized(a: f32, b: f32, c: f32, d: f32) -> Plane {
    let mag = (a * a + b * b + c * c).sqrt();
    if mag > 1e-20 {
        let inv = 1.0 / mag;
        Plane { a: a * inv, b: b * inv, c: c * inv, d: d * inv }
    } else {
        Plane { a, b, c, d }
    }
}

/// Gribb–Hartmann extraction of the six frustum planes
/// `[left, right, bottom, top, near, far]` from a **row-major**, **zero-to-one**
/// view-projection matrix. Each plane is normalized and inside-positive.
pub fn extract(viewproj: &[[f32; 4]; 4]) -> [Plane; 6] {
    let r0 = viewproj[0];
    let r1 = viewproj[1];
    let r2 = viewproj[2];
    let r3 = viewproj[3];

    let left = normalized(r3[0] + r0[0], r3[1] + r0[1], r3[2] + r0[2], r3[3] + r0[3]);
    let right = normalized(r3[0] - r0[0], r3[1] - r0[1], r3[2] - r0[2], r3[3] - r0[3]);
    let bottom = normalized(r3[0] + r1[0], r3[1] + r1[1], r3[2] + r1[2], r3[3] + r1[3]);
    let top = normalized(r3[0] - r1[0], r3[1] - r1[1], r3[2] - r1[2], r3[3] - r1[3]);
    // zero-to-one depth: near is row2 alone, far is row3 − row2.
    let near = normalized(r2[0], r2[1], r2[2], r2[3]);
    let far = normalized(r3[0] - r2[0], r3[1] - r2[1], r3[2] - r2[2], r3[3] - r2[3]);

    [left, right, bottom, top, near, far]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn randf(seed: &mut u32, lo: f32, hi: f32) -> f32 {
        lo + (lcg(seed) % 1_000_001) as f32 / 1_000_000.0 * (hi - lo)
    }

    /// Row-major right-handed zero-to-one perspective (camera at origin looking
    /// down −z, RH; in front means z < 0), matching `perspective_rh_zo`.
    fn perspective_rh_zo_rowmajor(aspect: f32, fov_y: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
        let f = 1.0 / (fov_y * 0.5).tan();
        [
            [f / aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, far / (near - far), (far * near) / (near - far)],
            [0.0, 0.0, -1.0, 0.0],
        ]
    }

    fn clip(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 4] {
        let v = [p[0], p[1], p[2], 1.0];
        let mut o = [0.0f32; 4];
        for i in 0..4 {
            o[i] = m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2] + m[i][3] * v[3];
        }
        o
    }
    fn eval(pl: &Plane, p: [f32; 3]) -> f32 {
        pl.a * p[0] + pl.b * p[1] + pl.c * p[2] + pl.d
    }

    const NEAR: f32 = 0.5;
    const FAR: f32 = 100.0;
    fn vp() -> [[f32; 4]; 4] {
        perspective_rh_zo_rowmajor(1.5, 60.0_f32.to_radians(), NEAR, FAR)
    }

    #[test]
    fn point_inside_is_positive_on_all_planes() {
        let planes = extract(&vp());
        let inside = [0.0, 0.0, -10.0]; // centered, between near and far
        for (i, pl) in planes.iter().enumerate() {
            assert!(eval(pl, inside) > 0.0, "plane {} should be positive inside, got {}", i, eval(pl, inside));
        }
    }

    #[test]
    fn point_behind_is_negative_on_some_plane() {
        let planes = extract(&vp());
        let behind = [0.0, 0.0, 5.0]; // behind the camera (z > 0)
        assert!(
            planes.iter().any(|pl| eval(pl, behind) < 0.0),
            "a point behind the camera must fail at least one plane"
        );
        // far outside to the side, too
        let side = [1000.0, 0.0, -10.0];
        assert!(planes.iter().any(|pl| eval(pl, side) < 0.0));
    }

    #[test]
    fn planes_are_normalized() {
        let planes = extract(&vp());
        for (i, pl) in planes.iter().enumerate() {
            let n = (pl.a * pl.a + pl.b * pl.b + pl.c * pl.c).sqrt();
            assert!((n - 1.0).abs() < 1e-5, "plane {} normal not unit: {}", i, n);
        }
    }

    #[test]
    fn near_plane_separates_front_and_behind() {
        let planes = extract(&vp());
        let near_plane = &planes[4];
        let just_in_front = [0.0, 0.0, -(NEAR + 1.0)]; // past the near plane
        let before_near = [0.0, 0.0, -(NEAR * 0.5)]; // between camera and near
        assert!(eval(near_plane, just_in_front) > 0.0, "in front of near should be positive");
        assert!(eval(near_plane, before_near) < 0.0, "closer than near should be negative");
    }

    #[test]
    fn far_plane_separates_near_and_beyond() {
        let planes = extract(&vp());
        let far_plane = &planes[5];
        let within = [0.0, 0.0, -(FAR * 0.5)];
        let beyond = [0.0, 0.0, -(FAR + 10.0)];
        assert!(eval(far_plane, within) > 0.0);
        assert!(eval(far_plane, beyond) < 0.0);
    }

    #[test]
    fn plane_signs_match_clip_space_bounds_exactly() {
        // The normalized plane eval is the clip-space bound divided by the
        // (positive) normal magnitude, so their signs must agree per plane.
        // clip bounds (inside = each >= 0): x+w, w-x, y+w, w-y, z, w-z
        let m = vp();
        let planes = extract(&m);
        let mut s = 0x9A5E_0000u32;
        for _ in 0..50_000 {
            let p = [
                randf(&mut s, -60.0, 60.0),
                randf(&mut s, -60.0, 60.0),
                randf(&mut s, -120.0, 120.0),
            ];
            let c = clip(&m, p);
            let (x, y, z, w) = (c[0], c[1], c[2], c[3]);
            let bounds = [x + w, w - x, y + w, w - y, z, w - z];
            for i in 0..6 {
                let e = eval(&planes[i], p);
                // signs agree (allow a hair of fp slack near zero)
                if bounds[i].abs() > 1e-3 && e.abs() > 1e-3 {
                    assert!(
                        (bounds[i] > 0.0) == (e > 0.0),
                        "plane {} sign disagrees with clip bound: bound {} eval {} at {:?}",
                        i, bounds[i], e, p
                    );
                }
                assert!(e.is_finite());
            }
        }
    }

    #[test]
    fn identity_matrix_is_finite() {
        let id = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let planes = extract(&id);
        for pl in planes.iter() {
            assert!(pl.a.is_finite() && pl.b.is_finite() && pl.c.is_finite() && pl.d.is_finite());
            let n = (pl.a * pl.a + pl.b * pl.b + pl.c * pl.c).sqrt();
            assert!((n - 1.0).abs() < 1e-5, "identity plane normal not unit: {}", n);
        }
    }

    #[test]
    fn deterministic() {
        let m = vp();
        let a = extract(&m);
        let b = extract(&m);
        for i in 0..6 {
            assert_eq!(a[i], b[i]);
        }
        // also stable across a sweep of random matrices
        let mut s = 0x9A5E_0001u32;
        for _ in 0..2000 {
            let mut mm = [[0.0f32; 4]; 4];
            for r in 0..4 {
                for cc in 0..4 {
                    mm[r][cc] = randf(&mut s, -5.0, 5.0);
                }
            }
            assert_eq!(extract(&mm), extract(&mm));
            for pl in extract(&mm).iter() {
                assert!(pl.a.is_finite() && pl.b.is_finite() && pl.c.is_finite() && pl.d.is_finite());
            }
        }
    }
}
