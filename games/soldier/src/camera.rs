//! camera.rs — a first-person pinhole camera.
//!
//! The reference renderer ships an `OrbitCam` (camera orbits a target). An FPS instead wants the camera
//! to sit AT the eye and look along a yaw/pitch. So this is `OrbitCam::project` rebuilt around an
//! orthonormal view basis: we form `right/up/forward` from the look angles, transform world points into
//! camera space, and divide by depth for perspective — the same "scale / depth" trick the reference uses,
//! so nearer things project larger. Crucially, `project`, `forward()`, `right()` and `up()` all derive
//! from the *same* basis, which is what makes billboards face you and raycasts hit what the crosshair is on.

use crate::math::{clampf, Vec3};

#[derive(Clone, Copy)]
pub struct FpsCam {
    /// Eye position in world space.
    pub pos: Vec3,
    /// Heading around world-Y. yaw = 0 looks toward −Z.
    pub yaw: f32,
    /// Look up/down, clamped just shy of straight up/down to avoid gimbal flip.
    pub pitch: f32,
}

impl Default for FpsCam {
    fn default() -> Self {
        Self { pos: Vec3::new(0.0, 1.6, 0.0), yaw: 0.0, pitch: 0.0 }
    }
}

impl FpsCam {
    pub const NEAR: f32 = 0.1;
    pub const PITCH_LIMIT: f32 = 1.50; // ~86°

    /// Unit forward (the look direction). yaw=0 → −Z, pitch>0 → up.
    #[inline]
    pub fn forward(&self) -> Vec3 {
        let (cy, sy) = (self.yaw.cos(), self.yaw.sin());
        let (cp, sp) = (self.pitch.cos(), self.pitch.sin());
        Vec3::new(sy * cp, sp, -cy * cp)
    }
    /// Unit right (camera-space +X), always horizontal-ish; stable because pitch is clamped.
    #[inline]
    pub fn right(&self) -> Vec3 {
        self.forward().cross(Vec3::UP).normalized()
    }
    /// Unit up (camera-space +Y).
    #[inline]
    pub fn up(&self) -> Vec3 {
        self.right().cross(self.forward()).normalized()
    }
    /// Forward flattened to the ground plane — what WASD walks along (so looking up doesn't slow you).
    #[inline]
    pub fn forward_flat(&self) -> Vec3 {
        Vec3::new(self.yaw.sin(), 0.0, -self.yaw.cos())
    }

    pub fn add_look(&mut self, dyaw: f32, dpitch: f32) {
        self.yaw += dyaw;
        // keep yaw in a sane range without affecting trig
        if self.yaw > std::f32::consts::PI {
            self.yaw -= std::f32::consts::TAU;
        } else if self.yaw < -std::f32::consts::PI {
            self.yaw += std::f32::consts::TAU;
        }
        self.pitch = clampf(self.pitch + dpitch, -Self::PITCH_LIMIT, Self::PITCH_LIMIT);
    }

    /// Project a world point to screen space. Returns `(sx, sy, depth)` with depth>0 in front of the
    /// camera, or `None` if at/behind the near plane. `(cx,cy)` is the viewport centre; `scale` ties world
    /// units to pixels (use [`scale_for_fov`]).
    pub fn project(&self, p: Vec3, cx: f32, cy: f32, scale: f32) -> Option<(f32, f32, f32)> {
        let rel = p - self.pos;
        let (right, up, fwd) = self.basis();
        let depth = rel.dot(fwd);
        if depth <= Self::NEAR {
            return None;
        }
        let camx = rel.dot(right);
        let camy = rel.dot(up);
        let persp = scale / depth;
        Some((cx + camx * persp, cy - camy * persp, depth))
    }

    /// The orthonormal view basis `(right, up, forward)`. Computed once for hot loops like `project`.
    #[inline]
    pub fn basis(&self) -> (Vec3, Vec3, Vec3) {
        let fwd = self.forward();
        let right = fwd.cross(Vec3::UP).normalized();
        let up = right.cross(fwd).normalized();
        (right, up, fwd)
    }
}

/// Pixels-per-world-unit so a vertical field-of-view of `vfov` radians fills a viewport `viewport_h` px tall.
#[inline]
pub fn scale_for_fov(viewport_h: f32, vfov: f32) -> f32 {
    (viewport_h * 0.5) / (vfov * 0.5).tan()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_right_up_orthonormal() {
        let mut c = FpsCam::default();
        c.yaw = 0.7;
        c.pitch = -0.3;
        let (r, u, f) = c.basis();
        for v in [r, u, f] {
            assert!((v.len() - 1.0).abs() < 1e-5, "basis vectors are unit length");
        }
        assert!(r.dot(u).abs() < 1e-5 && r.dot(f).abs() < 1e-5 && u.dot(f).abs() < 1e-5, "orthogonal");
    }

    #[test]
    fn yaw_zero_looks_down_neg_z_and_right_is_plus_x() {
        let c = FpsCam::default();
        let f = c.forward();
        assert!(f.z < -0.99, "yaw 0 looks toward -Z, got {f:?}");
        let r = c.right();
        assert!(r.x > 0.99, "right is +X when looking -Z, got {r:?}");
    }

    #[test]
    fn point_dead_ahead_lands_at_center_and_behind_is_culled() {
        let c = FpsCam::default(); // at (0,1.6,0) looking -Z
        let ahead = c.pos + c.forward() * 5.0;
        let (sx, sy, d) = c.project(ahead, 400.0, 300.0, 500.0).unwrap();
        assert!((sx - 400.0).abs() < 0.01 && (sy - 300.0).abs() < 0.01, "centered: {sx},{sy}");
        assert!(d > 0.0);
        // a point behind the camera projects to nothing
        assert!(c.project(c.pos - c.forward() * 5.0, 400.0, 300.0, 500.0).is_none());
    }

    #[test]
    fn nearer_projects_larger() {
        let c = FpsCam::default();
        let off = c.right() * 1.0; // one unit to the right
        let near = c.project(c.pos + c.forward() * 2.0 + off, 0.0, 0.0, 500.0).unwrap();
        let far = c.project(c.pos + c.forward() * 8.0 + off, 0.0, 0.0, 500.0).unwrap();
        assert!(near.0.abs() > far.0.abs(), "near point spreads wider: {near:?} vs {far:?}");
    }

    #[test]
    fn pitch_is_clamped() {
        let mut c = FpsCam::default();
        c.add_look(0.0, 100.0);
        assert!(c.pitch <= FpsCam::PITCH_LIMIT + 1e-6);
        c.add_look(0.0, -200.0);
        assert!(c.pitch >= -FpsCam::PITCH_LIMIT - 1e-6);
    }
}
