//! Cameras: orbit (used by `unendlich-chunk`) and free-fly first person
//! (used by `unendlich-world`).
//!
//! Vulkan's clip-space Y axis points DOWN (opposite of OpenGL/GLM). We
//! correct for this by flipping the projection's [1][1] entry. Combined
//! with back-face culling + front_face = CLOCKWISE in the chunk pipeline
//! this produces correctly-oriented outward faces.
//!
//! ## View vs. controller (multiplayer prep)
//!
//! Three concepts that used to be tangled are now distinct here:
//!
//! - **`View`**: a pure observation pose — position, yaw, pitch, fov,
//!   clip planes, plus `view_projection()`. Multiple Views can exist
//!   simultaneously (split-screen, picture-in-picture, security
//!   cameras, replay viewing, debug "see-yourself" perspective).
//!   Cheap value type. Serializable. **In a networked build this is
//!   what gets replicated** — the server doesn't need to know your
//!   move_speed or mouse_sensitivity, but it absolutely needs to
//!   know where each connected player's eye is for AOI/culling.
//!
//! - **`FlyController`**: input-handling state — move_speed and
//!   mouse_sensitivity. Mutates a View based on local input.
//!   One per local human (so 0..4 in 4-player split-screen, 0
//!   in dedicated-server mode). NOT replicated.
//!
//! - **`Camera` / `FlyCamera`**: legacy types that bundle View +
//!   controller in one struct. Kept for now so existing call sites
//!   keep working; new code should reach for `View` + `FlyController`
//!   directly. The legacy types delegate to View under the hood.

use nalgebra_glm as glm;

/// A pure view pose. Everything needed to render the world from a
/// vantage point; no controller state, no input config.
///
/// Multiplayer notes:
///   - One `View` per simultaneous viewport. Split-screen 2-player
///     means 2 Views, one per panel.
///   - On the wire, send the position+yaw+pitch+fov; the receiver
///     reconstructs near/far locally if not sent (they're typically
///     constant per-game).
///   - The renderer takes a `&View` on draw; doesn't own one. This
///     unlocks: multiple draw calls per frame with different views,
///     server-side culling using the same View math, replay systems
///     stepping through saved Views.
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub position: glm::Vec3,
    /// Yaw in radians, 0 = looking +X.
    pub yaw: f32,
    /// Pitch in radians, positive = looking up.
    pub pitch: f32,
    pub fov_y_radians: f32,
    pub near: f32,
    pub far: f32,
}

impl View {
    pub fn new(position: glm::Vec3) -> Self {
        Self {
            position,
            yaw: 0.0,
            pitch: 0.0,
            fov_y_radians: std::f32::consts::FRAC_PI_3,
            near: 0.1,
            far: 1024.0,
        }
    }

    /// Direction the view is looking. Unit vector in world space.
    pub fn forward(&self) -> glm::Vec3 {
        let cp = self.pitch.cos();
        glm::vec3(
            self.yaw.cos() * cp,
            self.pitch.sin(),
            self.yaw.sin() * cp,
        )
    }

    /// Right-hand vector (perpendicular to forward, on horizontal plane).
    pub fn right(&self) -> glm::Vec3 {
        let f = self.forward();
        let r = glm::cross(&f, &glm::vec3(0.0, 1.0, 0.0));
        if r.norm_squared() < 1e-6 {
            glm::vec3(0.0, 0.0, 1.0)
        } else {
            r.normalize()
        }
    }

    /// Build the view-projection matrix for this View at the given
    /// aspect ratio. The Vulkan Y-flip is applied here.
    pub fn view_projection(&self, aspect: f32) -> glm::Mat4 {
        let eye = self.position;
        let target = eye + self.forward();
        let view = glm::look_at_rh(&eye, &target, &glm::vec3(0.0, 1.0, 0.0));
        let mut proj = glm::perspective_rh_zo(aspect, self.fov_y_radians, self.near, self.far);
        proj[(1, 1)] *= -1.0;
        proj * view
    }

    /// Return the view (camera-to-world inverse) matrix on its own.
    /// Used by v0.24.0b where the GPU `view_compute.comp` shader takes
    /// view and projection separately and derives view_proj internally.
    pub fn view_matrix(&self) -> glm::Mat4 {
        let eye = self.position;
        let target = eye + self.forward();
        glm::look_at_rh(&eye, &target, &glm::vec3(0.0, 1.0, 0.0))
    }

    /// Return the projection matrix (including Vulkan Y-flip) on its own.
    /// Pair with `view_matrix` to feed the GPU view-derivation pipeline.
    pub fn projection_matrix(&self, aspect: f32) -> glm::Mat4 {
        let mut proj = glm::perspective_rh_zo(aspect, self.fov_y_radians, self.near, self.far);
        proj[(1, 1)] *= -1.0;
        proj
    }
}

/// Input-handling configuration for a local player driving a View.
/// Holds the per-player tuning (move_speed, mouse_sensitivity) and
/// the methods that mutate a View in response to keyboard/mouse.
///
/// Multiplayer notes:
///   - One `FlyController` per local human player. In 4-player
///     split-screen there'd be 4. On a dedicated server with no
///     local players there'd be 0.
///   - NOT serialized over the network. The server/peers only see
///     the resulting View pose changes, not the input that produced
///     them. (For prediction/rollback, they see the input *commands*
///     coming from clients, which is yet another layer.)
#[derive(Debug, Clone, Copy)]
pub struct FlyController {
    pub move_speed: f32,           // blocks/second when walking
    pub mouse_sensitivity: f32,    // radians per pixel
}

impl Default for FlyController {
    fn default() -> Self {
        Self {
            move_speed: 16.0,
            mouse_sensitivity: 0.0025,
        }
    }
}

impl FlyController {
    /// Apply a mouse delta in pixels. Positive dx looks right, positive
    /// dy looks down (matches typical screen coordinates).
    pub fn apply_mouse(&self, view: &mut View, dx: f64, dy: f64) {
        view.yaw += dx as f32 * self.mouse_sensitivity;
        view.pitch -= dy as f32 * self.mouse_sensitivity;

        // Clamp pitch to just inside ±90° to avoid gimbal flips.
        let limit = std::f32::consts::FRAC_PI_2 - 0.001;
        view.pitch = view.pitch.clamp(-limit, limit);

        // Wrap yaw to keep it bounded.
        let two_pi = std::f32::consts::TAU;
        if view.yaw > two_pi { view.yaw -= two_pi; }
        if view.yaw < 0.0 { view.yaw += two_pi; }
    }

    /// Apply a movement vector in local space (forward=+x, right=+z,
    /// up=+y). `dt` is delta time in seconds.
    pub fn apply_move(&self, view: &mut View, local: glm::Vec3, dt: f32) {
        if local.norm_squared() < 1e-6 { return; }

        let f = view.forward();
        let f_horiz = glm::vec3(f.x, 0.0, f.z);
        let f_horiz = if f_horiz.norm_squared() > 1e-6 {
            f_horiz.normalize()
        } else {
            glm::vec3(1.0, 0.0, 0.0)
        };
        let r_horiz = view.right();
        let up = glm::vec3(0.0, 1.0, 0.0);

        let world = f_horiz * local.x + r_horiz * local.z + up * local.y;
        let world = if world.norm_squared() > 1e-6 {
            world.normalize()
        } else {
            return;
        };

        view.position += world * (self.move_speed * dt);
    }
}

/// Orbit camera — keeps targeting a point in space, rotates around it.
/// Used by the single-chunk demo binary.
pub struct Camera {
    pub target: glm::Vec3,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y_radians: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    pub fn new(target: glm::Vec3, distance: f32) -> Self {
        Self {
            target,
            distance,
            yaw: 0.5,
            pitch: -0.5,
            fov_y_radians: std::f32::consts::FRAC_PI_3,
            near: 0.1,
            far: 1024.0,
        }
    }

    pub fn position(&self) -> glm::Vec3 {
        let cp = self.pitch.cos();
        let offset = glm::vec3(
            self.distance * self.yaw.cos() * cp,
            self.distance * self.pitch.sin(),
            self.distance * self.yaw.sin() * cp,
        );
        self.target - offset
    }

    pub fn view_projection(&self, aspect: f32) -> glm::Mat4 {
        let eye = self.position();
        let view = glm::look_at_rh(&eye, &self.target, &glm::vec3(0.0, 1.0, 0.0));
        let mut proj = glm::perspective_rh_zo(aspect, self.fov_y_radians, self.near, self.far);
        proj[(1, 1)] *= -1.0;
        proj * view
    }

    pub fn auto_orbit(&mut self, time_secs: f32) {
        self.yaw = 0.5 + time_secs * 0.15;
    }
}

/// Free-fly first-person camera — LEGACY type, bundles View +
/// FlyController fields. New code should use `View` and
/// `FlyController` directly.
///
/// This wrapper is preserved so existing call sites keep working
/// during the multiplayer-prep transition. It delegates field-by-
/// field to the underlying split.
pub struct FlyCamera {
    pub position: glm::Vec3,
    pub yaw: f32,           // radians; 0 = looking +X
    pub pitch: f32,         // radians; positive = looking up
    pub fov_y_radians: f32,
    pub near: f32,
    pub far: f32,
    pub move_speed: f32,    // blocks/second when walking
    pub mouse_sensitivity: f32, // radians per pixel
}

impl FlyCamera {
    pub fn new(position: glm::Vec3) -> Self {
        let v = View::new(position);
        let c = FlyController::default();
        Self {
            position: v.position,
            yaw: v.yaw,
            pitch: v.pitch,
            fov_y_radians: v.fov_y_radians,
            near: v.near,
            far: v.far,
            move_speed: c.move_speed,
            mouse_sensitivity: c.mouse_sensitivity,
        }
    }

    /// Lift the legacy fields into a fresh `View` value.
    pub fn as_view(&self) -> View {
        View {
            position: self.position,
            yaw: self.yaw,
            pitch: self.pitch,
            fov_y_radians: self.fov_y_radians,
            near: self.near,
            far: self.far,
        }
    }

    /// Push fields back from a View. Lets callers manipulate via the
    /// View API and persist the result on this legacy struct.
    pub fn copy_from_view(&mut self, view: &View) {
        self.position = view.position;
        self.yaw = view.yaw;
        self.pitch = view.pitch;
        self.fov_y_radians = view.fov_y_radians;
        self.near = view.near;
        self.far = view.far;
    }

    pub fn forward(&self) -> glm::Vec3 { self.as_view().forward() }
    pub fn right(&self) -> glm::Vec3 { self.as_view().right() }

    pub fn apply_mouse(&mut self, dx: f64, dy: f64) {
        let mut v = self.as_view();
        let c = FlyController { move_speed: self.move_speed, mouse_sensitivity: self.mouse_sensitivity };
        c.apply_mouse(&mut v, dx, dy);
        self.copy_from_view(&v);
    }

    pub fn apply_move(&mut self, local: glm::Vec3, dt: f32) {
        let mut v = self.as_view();
        let c = FlyController { move_speed: self.move_speed, mouse_sensitivity: self.mouse_sensitivity };
        c.apply_move(&mut v, local, dt);
        self.copy_from_view(&v);
    }

    pub fn view_projection(&self, aspect: f32) -> glm::Mat4 {
        self.as_view().view_projection(aspect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nalgebra_glm as glm;

    #[test]
    fn fly_forward_moves_in_yaw_direction() {
        let mut cam = FlyCamera::new(glm::vec3(0.0, 0.0, 0.0));
        cam.yaw = 0.0; // looking +X
        cam.apply_move(glm::vec3(1.0, 0.0, 0.0), 1.0);
        // Should have moved roughly +X by move_speed.
        assert!(cam.position.x > 1.0, "x={}", cam.position.x);
        assert!(cam.position.y.abs() < 1e-3);
        assert!(cam.position.z.abs() < 1e-3);
    }

    #[test]
    fn fly_pitch_doesnt_affect_horizontal_walk() {
        let mut cam = FlyCamera::new(glm::vec3(0.0, 100.0, 0.0));
        cam.yaw = 0.0;
        cam.pitch = -0.5; // looking down
        cam.apply_move(glm::vec3(1.0, 0.0, 0.0), 1.0);
        // Walking forward shouldn't change Y when looking down.
        assert!((cam.position.y - 100.0).abs() < 1e-3, "y drifted to {}", cam.position.y);
    }

    #[test]
    fn pitch_clamps_at_poles() {
        let mut cam = FlyCamera::new(glm::vec3(0.0, 0.0, 0.0));
        cam.apply_mouse(0.0, 100000.0); // huge upward look
        let limit = std::f32::consts::FRAC_PI_2;
        assert!(cam.pitch.abs() < limit, "pitch escaped clamp: {}", cam.pitch);
    }

    /// View is a pure pose: cloning produces an independent value.
    /// Multiplayer prep — Views must be cheap to copy because the
    /// renderer takes them by value/reference per draw.
    #[test]
    fn view_is_copy() {
        let v1 = View::new(glm::vec3(1.0, 2.0, 3.0));
        let v2 = v1; // Copy
        assert_eq!(v1.position.x, v2.position.x);
        assert_eq!(v1.yaw, v2.yaw);
    }

    /// View.forward agrees with the legacy FlyCamera.forward at the
    /// same pose. This is the contract that lets us migrate callers
    /// gradually without behavior change.
    #[test]
    fn view_forward_matches_flycamera() {
        let mut cam = FlyCamera::new(glm::vec3(5.0, 6.0, 7.0));
        cam.yaw = 1.2;
        cam.pitch = -0.3;
        let v = cam.as_view();
        let cf = cam.forward();
        let vf = v.forward();
        assert!((cf - vf).norm() < 1e-5, "forward mismatch: cam={:?} view={:?}", cf, vf);
    }

    /// FlyController applied to a fresh View produces the same move
    /// the legacy FlyCamera produces. Migration safety.
    #[test]
    fn controller_move_matches_legacy() {
        let mut cam = FlyCamera::new(glm::vec3(0.0, 0.0, 0.0));
        cam.yaw = 0.5;
        cam.apply_move(glm::vec3(1.0, 0.0, 0.0), 0.1);
        let cam_pos = cam.position;

        let mut view = View::new(glm::vec3(0.0, 0.0, 0.0));
        view.yaw = 0.5;
        let ctrl = FlyController::default();
        ctrl.apply_move(&mut view, glm::vec3(1.0, 0.0, 0.0), 0.1);
        let view_pos = view.position;

        assert!((cam_pos - view_pos).norm() < 1e-5,
            "controller move diverged from legacy: cam={:?} view={:?}", cam_pos, view_pos);
    }

    /// Two Views can coexist independently — split-screen prep.
    /// Mutating one must not affect the other.
    #[test]
    fn two_views_are_independent() {
        let mut v1 = View::new(glm::vec3(0.0, 0.0, 0.0));
        let mut v2 = View::new(glm::vec3(100.0, 0.0, 0.0));
        let ctrl = FlyController::default();
        ctrl.apply_mouse(&mut v1, 100.0, 0.0);
        ctrl.apply_move(&mut v2, glm::vec3(1.0, 0.0, 0.0), 1.0);
        // v1 should have rotated yaw but not moved.
        assert!(v1.yaw > 0.1, "v1 yaw didn't change: {}", v1.yaw);
        assert!((v1.position - glm::vec3(0.0, 0.0, 0.0)).norm() < 1e-3,
            "v1 unexpectedly moved: {:?}", v1.position);
        // v2 should have moved but not rotated.
        assert!(v2.position.x > 100.0, "v2 didn't move: {:?}", v2.position);
        assert!(v2.yaw.abs() < 1e-5, "v2 unexpectedly rotated: {}", v2.yaw);
    }
}
