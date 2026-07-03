//! # Geometric primitives
//!
//! 3D vector + small operations used by v8 reconstruction, v9 volumetric, 
//! and the main app for SfM/pose work. Originally these lived in 
//! `v6_object_codec.rs` but were extracted during the v10 cleanup when 
//! v6 was deleted.
//!
//! Intentionally minimal — no SIMD, no advanced math. The hot paths in 
//! v8/v9 use raw `[f32; 3]` arrays. This `Vec3` is the public-API 
//! shape for things like camera poses and feature world-positions.

/// Simple 3D vector. `Default::default()` is the zero vector.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self { Self { x, y, z } }
    pub fn zero() -> Self { Self { x: 0.0, y: 0.0, z: 0.0 } }
    pub fn distance(&self, o: &Vec3) -> f32 {
        let dx = self.x - o.x;
        let dy = self.y - o.y;
        let dz = self.z - o.z;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }
}
