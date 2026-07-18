//! Column-major 4×4 matrix (wgpu/WGSL convention, NDC z in [0, 1]).
//!
//! `Mat4`'s layout (`pub [f32; 16]`, index `col*4 + row`), `mul`, and the
//! `perspective`/`look_at` constructors are clawbound math.rs **verbatim** —
//! ready to upload to a WGSL `mat4x4`. The extra constructors (identity,
//! translate, scaling, axis rotations, orthographic, mul_point/mul_dir) are
//! the ones soldier gpu.rs and unendlich's render pipelines re-derive.

use crate::vec::{v3, Vec3};

/// Column-major 4×4, stored as 16 f32 (`col*4 + row`), ready to upload to a
/// WGSL `mat4x4`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4(pub [f32; 16]);

impl Mat4 {
    pub const IDENTITY: Mat4 = Mat4([
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ]);

    /// Matrix product `self × b` — clawbound's loop, verbatim.
    pub fn mul(&self, b: &Mat4) -> Mat4 {
        let a = &self.0;
        let b = &b.0;
        let mut c = [0.0f32; 16];
        for col in 0..4 {
            for row in 0..4 {
                let mut s = 0.0;
                for k in 0..4 {
                    s += a[k * 4 + row] * b[col * 4 + k];
                }
                c[col * 4 + row] = s;
            }
        }
        Mat4(c)
    }

    pub fn translate(t: Vec3) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        m.0[12] = t.x;
        m.0[13] = t.y;
        m.0[14] = t.z;
        m
    }

    pub fn scaling(s: Vec3) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        m.0[0] = s.x;
        m.0[5] = s.y;
        m.0[10] = s.z;
        m
    }

    pub fn rotate_x(a: f32) -> Mat4 {
        let (s, c) = a.sin_cos();
        let mut m = Mat4::IDENTITY;
        m.0[5] = c;
        m.0[6] = s;
        m.0[9] = -s;
        m.0[10] = c;
        m
    }

    pub fn rotate_y(a: f32) -> Mat4 {
        let (s, c) = a.sin_cos();
        let mut m = Mat4::IDENTITY;
        m.0[0] = c;
        m.0[2] = -s;
        m.0[8] = s;
        m.0[10] = c;
        m
    }

    pub fn rotate_z(a: f32) -> Mat4 {
        let (s, c) = a.sin_cos();
        let mut m = Mat4::IDENTITY;
        m.0[0] = c;
        m.0[1] = s;
        m.0[4] = -s;
        m.0[5] = c;
        m
    }

    /// Transform a POINT (w = 1), with perspective divide.
    pub fn mul_point(&self, p: Vec3) -> Vec3 {
        let m = &self.0;
        let x = m[0] * p.x + m[4] * p.y + m[8] * p.z + m[12];
        let y = m[1] * p.x + m[5] * p.y + m[9] * p.z + m[13];
        let z = m[2] * p.x + m[6] * p.y + m[10] * p.z + m[14];
        let w = m[3] * p.x + m[7] * p.y + m[11] * p.z + m[15];
        if w.abs() > 1e-8 && (w - 1.0).abs() > 1e-8 {
            v3(x / w, y / w, z / w)
        } else {
            v3(x, y, z)
        }
    }

    /// Transform a DIRECTION (w = 0): rotation/scale only, no translation.
    pub fn mul_dir(&self, d: Vec3) -> Vec3 {
        let m = &self.0;
        v3(
            m[0] * d.x + m[4] * d.y + m[8] * d.z,
            m[1] * d.x + m[5] * d.y + m[9] * d.z,
            m[2] * d.x + m[6] * d.y + m[10] * d.z,
        )
    }
}

/// Right-handed perspective, NDC z ∈ [0, 1] (wgpu) — clawbound verbatim.
pub fn perspective(fovy_rad: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fovy_rad * 0.5).tan();
    let nf = 1.0 / (near - far);
    let mut m = [0.0f32; 16];
    m[0] = f / aspect; // col0.row0
    m[5] = f; // col1.row1
    m[10] = far * nf; // col2.row2
    m[11] = -1.0; // col2.row3
    m[14] = near * far * nf; // col3.row2
    Mat4(m)
}

/// Right-handed look-at view matrix — clawbound verbatim
/// (screen-right = forward × up; at yaw 0, forward = +z ⇒ right = −x).
pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let fwd = target.sub(eye).norm();
    let s = fwd.cross(up).norm();
    let u = s.cross(fwd);
    Mat4([
        s.x, u.x, -fwd.x, 0.0, //
        s.y, u.y, -fwd.y, 0.0, //
        s.z, u.z, -fwd.z, 0.0, //
        -s.dot(eye),
        -u.dot(eye),
        fwd.dot(eye),
        1.0,
    ])
}

/// Right-handed orthographic, NDC z ∈ [0, 1] (wgpu).
pub fn orthographic(l: f32, r: f32, b: f32, t: f32, near: f32, far: f32) -> Mat4 {
    let mut m = [0.0f32; 16];
    m[0] = 2.0 / (r - l);
    m[5] = 2.0 / (t - b);
    m[10] = 1.0 / (near - far);
    m[12] = -(r + l) / (r - l);
    m[13] = -(t + b) / (t - b);
    m[14] = near / (near - far);
    m[15] = 1.0;
    Mat4(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        a.dist(b) < 1e-4
    }

    #[test]
    fn identity_and_mul() {
        let m = Mat4::translate(v3(1.0, 2.0, 3.0)).mul(&Mat4::scaling(v3(2.0, 2.0, 2.0)));
        // scale-then-translate: p=(1,0,0) → (2,0,0) → (3,2,3)
        assert!(close(m.mul_point(v3(1.0, 0.0, 0.0)), v3(3.0, 2.0, 3.0)));
        assert_eq!(Mat4::IDENTITY.mul(&m), m);
        assert_eq!(m.mul(&Mat4::IDENTITY), m);
        // direction ignores translation
        assert!(close(
            Mat4::translate(v3(5.0, 5.0, 5.0)).mul_dir(v3(0.0, 1.0, 0.0)),
            v3(0.0, 1.0, 0.0)
        ));
    }

    #[test]
    fn rotations_are_right_handed() {
        use std::f32::consts::FRAC_PI_2;
        assert!(close(Mat4::rotate_z(FRAC_PI_2).mul_dir(v3(1.0, 0.0, 0.0)), v3(0.0, 1.0, 0.0)));
        assert!(close(Mat4::rotate_x(FRAC_PI_2).mul_dir(v3(0.0, 1.0, 0.0)), v3(0.0, 0.0, 1.0)));
        assert!(close(Mat4::rotate_y(FRAC_PI_2).mul_dir(v3(0.0, 0.0, 1.0)), v3(1.0, 0.0, 0.0)));
    }

    #[test]
    fn look_at_puts_target_on_neg_z_and_perspective_maps_depth() {
        let eye = v3(0.0, 0.0, 5.0);
        let view = look_at(eye, Vec3::ZERO, v3(0.0, 1.0, 0.0));
        let vt = view.mul_point(Vec3::ZERO);
        assert!(vt.x.abs() < 1e-4 && vt.y.abs() < 1e-4 && vt.z < 0.0, "target ahead on −z: {vt:?}");

        let proj = perspective(60f32.to_radians(), 16.0 / 9.0, 0.1, 100.0);
        let near_p = proj.mul_point(v3(0.0, 0.0, -0.1));
        let far_p = proj.mul_point(v3(0.0, 0.0, -100.0));
        // wgpu convention: near → 0, far → 1
        assert!(near_p.z.abs() < 1e-3, "near plane at z≈0: {}", near_p.z);
        assert!((far_p.z - 1.0).abs() < 1e-3, "far plane at z≈1: {}", far_p.z);
    }

    #[test]
    fn orthographic_maps_the_box_to_ndc() {
        let m = orthographic(-2.0, 2.0, -1.0, 1.0, 0.5, 10.0);
        assert!(close(m.mul_point(v3(-2.0, -1.0, -0.5)), v3(-1.0, -1.0, 0.0)));
        assert!(close(m.mul_point(v3(2.0, 1.0, -10.0)), v3(1.0, 1.0, 1.0)));
    }
}
