//! Minimal math — Vec3 + column-major Mat4 (wgpu/WGSL convention, NDC z in [0,1]).

#[derive(Clone, Copy, Debug)]
pub struct Vec3 { pub x: f32, pub y: f32, pub z: f32 }

pub fn v3(x: f32, y: f32, z: f32) -> Vec3 { Vec3 { x, y, z } }

impl Vec3 {
    pub fn add(self, o: Vec3) -> Vec3 { v3(self.x + o.x, self.y + o.y, self.z + o.z) }
    pub fn sub(self, o: Vec3) -> Vec3 { v3(self.x - o.x, self.y - o.y, self.z - o.z) }
    pub fn scale(self, s: f32) -> Vec3 { v3(self.x * s, self.y * s, self.z * s) }
    pub fn dot(self, o: Vec3) -> f32 { self.x * o.x + self.y * o.y + self.z * o.z }
    pub fn cross(self, o: Vec3) -> Vec3 {
        v3(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }
    pub fn len(self) -> f32 { self.dot(self).sqrt() }
    pub fn norm(self) -> Vec3 { let l = self.len(); if l > 1e-6 { self.scale(1.0 / l) } else { self } }
}

/// Column-major 4x4, stored as 16 f32 (col*4 + row), ready to upload to a WGSL mat4x4.
#[derive(Clone, Copy)]
pub struct Mat4(pub [f32; 16]);

impl Mat4 {
    pub fn mul(&self, b: &Mat4) -> Mat4 {
        let a = &self.0; let b = &b.0;
        let mut c = [0.0f32; 16];
        for col in 0..4 {
            for row in 0..4 {
                let mut s = 0.0;
                for k in 0..4 { s += a[k * 4 + row] * b[col * 4 + k]; }
                c[col * 4 + row] = s;
            }
        }
        Mat4(c)
    }
}

pub fn perspective(fovy_rad: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fovy_rad * 0.5).tan();
    let nf = 1.0 / (near - far);
    let mut m = [0.0f32; 16];
    m[0] = f / aspect;         // col0.row0
    m[5] = f;                  // col1.row1
    m[10] = far * nf;          // col2.row2
    m[11] = -1.0;              // col2.row3
    m[14] = near * far * nf;   // col3.row2
    Mat4(m)
}

pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let fwd = target.sub(eye).norm();
    let s = fwd.cross(up).norm();
    let u = s.cross(fwd);
    Mat4([
        s.x, u.x, -fwd.x, 0.0,
        s.y, u.y, -fwd.y, 0.0,
        s.z, u.z, -fwd.z, 0.0,
        -s.dot(eye), -u.dot(eye), fwd.dot(eye), 1.0,
    ])
}
