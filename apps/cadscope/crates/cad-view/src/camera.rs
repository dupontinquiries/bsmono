//! camera.rs — the inspection orbit camera. Skydribble's chase camera is a driver's eye; this is a
//! machinist's: orbit around a pivot, dolly, pan in the view plane, frame the selection. It keeps
//! skydribble's `view_proj` conventions exactly — `look_at_rh` + `perspective_rh`, the
//! degenerate-basis guard when the view direction grazes up, and the (vp, vp⁻¹) return pair — so
//! the unprojection that feeds `cad_kernel::Ray` picks and the 2D overlay projection stay
//! pixel-consistent with the render (the gpuscene alignment discipline from logscope).

use cad_kernel::Ray;
use glam::{Mat4, Vec2, Vec3, Vec4, Vec4Swizzles};

#[derive(Clone, Copy, Debug)]
pub struct OrbitCamera {
    pub target: Vec3,
    /// Radians around +Y.
    pub yaw: f32,
    /// Radians above the horizon; clamped shy of the poles.
    pub pitch: f32,
    pub dist: f32,
    pub fov_deg: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        OrbitCamera { target: Vec3::new(0.0, 1.0, 0.0), yaw: 0.7, pitch: 0.5, dist: 28.0, fov_deg: 45.0 }
    }
}

const PITCH_LIMIT: f32 = 1.54; // just under ±π/2 — the guard below covers the rest
const NEAR: f32 = 0.05;
const FAR: f32 = 5000.0;

impl OrbitCamera {
    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + Vec3::new(cy * cp, sp, sy * cp) * self.dist
    }

    pub fn orbit(&mut self, d_yaw: f32, d_pitch: f32) {
        self.yaw += d_yaw;
        self.pitch = (self.pitch + d_pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Multiplicative dolly; factor > 1 backs away.
    pub fn dolly(&mut self, factor: f32) {
        self.dist = (self.dist * factor).clamp(NEAR * 8.0, FAR * 0.5);
    }

    /// Pan the pivot in the view plane by screen-fraction deltas (drag_px / viewport_px).
    pub fn pan(&mut self, frac: Vec2, aspect: f32) {
        let fwd = (self.target - self.eye()).normalize_or(Vec3::NEG_Z);
        let right = fwd.cross(Vec3::Y).normalize_or(Vec3::X);
        let up = right.cross(fwd);
        let h = 2.0 * self.dist * (self.fov_deg.to_radians() * 0.5).tan();
        self.target += right * (-frac.x * h * aspect) + up * (frac.y * h);
    }

    /// Frame a bounding sphere: pivot to its center, back off until it fits with 15% margin.
    pub fn frame(&mut self, center: Vec3, radius: f32) {
        self.target = center;
        let r = radius.max(1e-3);
        self.dist = (r / (self.fov_deg.to_radians() * 0.5).sin()) * 1.15;
    }

    /// (view_proj, inverse) — skydribble's pair, with its degenerate-basis guard.
    pub fn view_proj(&self, aspect: f32) -> (Mat4, Mat4) {
        let eye = self.eye();
        let fwd = (self.target - eye).normalize_or(Vec3::NEG_Z);
        let up = if fwd.dot(Vec3::Y).abs() > 0.985 {
            fwd.cross(Vec3::X).normalize_or(Vec3::Y)
        } else {
            Vec3::Y
        };
        let view = Mat4::look_at_rh(eye, self.target, up);
        let proj = Mat4::perspective_rh(self.fov_deg.to_radians(), aspect.max(1e-3), NEAR, FAR);
        let vp = proj * view;
        (vp, vp.inverse())
    }

    /// The pick ray through a point given in normalized device coordinates (x,y ∈ [-1,1], y up).
    pub fn screen_ray(&self, ndc: Vec2, inv_vp: &Mat4) -> Ray {
        let un = |z: f32| {
            let p = *inv_vp * Vec4::new(ndc.x, ndc.y, z, 1.0);
            p.xyz() / p.w
        };
        let near = un(0.0); // wgpu clip z ∈ [0,1]
        let far = un(1.0);
        Ray::new(near, far - near)
    }
}

/// Convert an egui pointer position inside `rect` to NDC (y flipped to point up).
pub fn ndc_of(pos: egui::Pos2, rect: egui::Rect) -> Vec2 {
    Vec2::new(
        (pos.x - rect.left()) / rect.width() * 2.0 - 1.0,
        1.0 - (pos.y - rect.top()) / rect.height() * 2.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_ray_points_at_the_target() {
        let cam = OrbitCamera::default();
        let (_, inv) = cam.view_proj(1.6);
        let ray = cam.screen_ray(Vec2::ZERO, &inv);
        let want = (cam.target - cam.eye()).normalize();
        assert!(ray.dir.dot(want) > 0.9999, "{:?} vs {:?}", ray.dir, want);
        assert!((ray.origin - cam.eye()).length() < NEAR * 4.0, "origin sits on the near plane");
    }

    #[test]
    fn unproject_round_trips_a_projected_point() {
        let cam = OrbitCamera { yaw: 1.2, pitch: 0.4, ..OrbitCamera::default() };
        let (vp, inv) = cam.view_proj(1.3);
        let world = Vec3::new(3.0, 2.0, -1.0);
        let clip = vp * world.extend(1.0);
        let ndc = Vec2::new(clip.x / clip.w, clip.y / clip.w);
        let ray = cam.screen_ray(ndc, &inv);
        // The ray must pass within a hair of the original world point.
        let to = world - ray.origin;
        let closest = ray.origin + ray.dir * to.dot(ray.dir);
        assert!((closest - world).length() < 1e-2);
    }

    #[test]
    fn frame_fits_the_sphere() {
        let mut cam = OrbitCamera::default();
        cam.frame(Vec3::new(5.0, 0.0, 0.0), 10.0);
        assert_eq!(cam.target, Vec3::new(5.0, 0.0, 0.0));
        // Angular radius of the framed sphere must be inside the half-fov.
        let ang = (10.0f32 / cam.dist).asin();
        assert!(ang < cam.fov_deg.to_radians() * 0.5);
    }

    #[test]
    fn pitch_clamps_at_the_pole_guard() {
        let mut cam = OrbitCamera::default();
        cam.orbit(0.0, 10.0);
        assert!(cam.pitch <= PITCH_LIMIT);
        let (vp, _) = cam.view_proj(1.0);
        assert!(vp.is_finite());
    }
}
