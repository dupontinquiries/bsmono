//! Sunlight light-space view-projection math for shadow mapping.
//!
//! v0.13 introduces the FIRST CASCADE — one shadow map covering the
//! near portion of the camera's view frustum. Multi-cascade support
//! (4 cascades, log split distribution, per-cascade depth slices) is
//! filed for the next shader increment but the math here is structured
//! so adding cascades is mechanical: you call `single_cascade(...)`
//! once per cascade with the right near/far slice.
//!
//! ## Why orthographic
//!
//! Sunlight is directional — every shadow ray is parallel. The light's
//! "view" is therefore an orthographic projection along the sun
//! direction. Width/height/near/far are chosen to TIGHTLY enclose the
//! camera's view frustum (or a slice of it for a cascade), so the
//! shadow map texels cover the visible region densely.
//!
//! ## Tight-fit algorithm
//!
//! 1. Compute the 8 world-space corners of the camera's view frustum
//!    (or a near/far slice of it for a cascade).
//! 2. Build the light-space view matrix: an orthonormal basis with the
//!    sun direction as the "forward" axis.
//! 3. Transform the 8 corners into light space → AABB in light space.
//! 4. The orthographic projection's bounds are that AABB.
//! 5. Composing: light_vp = ortho × light_view.
//!
//! ## Why the light "eye" position is arbitrary
//!
//! Directional lights don't have a position; they have a direction.
//! For the math we still need a `look_at` reference point. Convention:
//! place the light eye at the AABB center, looking along sun direction.
//! Any point along the sun's incoming line works — the resulting
//! orthographic VP differs only in its near/far depth offset, which
//! doesn't change which texel a shadow caster lands in.

use nalgebra_glm as glm;

use crate::render::camera::View;

/// A single shadow cascade's light-space view-projection matrix.
///
/// In Phase 1 (this version) we only construct ONE cascade. The slice
/// covers a configurable near/far range of the camera's view frustum.
/// Phase 2 will replace this with a `[LightCascade; 4]` array using
/// log-spaced splits 0–20m / 20–60m / 60–180m / 180–600m.
#[derive(Debug, Clone, Copy)]
pub struct LightCascade {
    /// Light-space view-projection matrix. Vertex shaders use this
    /// (instead of the camera's view-projection) when rendering into
    /// the shadow map. Same orientation convention as the camera VP:
    /// applied to a world-space position to get clip-space.
    pub view_proj: glm::Mat4,

    /// The slice of the camera's view frustum that this cascade covers,
    /// in METERS. `near_meters` and `far_meters` are distances along
    /// the camera's forward axis. Stored so the consuming fragment
    /// shader can pick the right cascade per pixel later (Phase 2).
    pub near_meters: f32,
    pub far_meters: f32,
}

/// Build a single shadow cascade's light-space view-projection matrix
/// by tight-fitting an orthographic projection around the camera
/// frustum's near/far slice.
///
/// `sun_dir` is the direction sunlight TRAVELS (sun → ground), unit.
/// `view` is the camera. `aspect` is width/height. `near_meters`/
/// `far_meters` cut the camera frustum to the slice this cascade
/// should cover; for a single-cascade setup pass `view.near` and
/// some sensible far like 200.0.
///
/// The returned `view_proj` does NOT have the Vulkan Y-flip applied:
/// shadow maps render off-screen and are sampled in shader, never
/// presented; the Vulkan-specific [1][1]-negation is irrelevant for
/// off-screen depth. Front-face culling in the shadow pipeline takes
/// care of the apparent-winding shift.
pub fn single_cascade(
    view: &View,
    sun_dir: glm::Vec3,
    aspect: f32,
    near_meters: f32,
    far_meters: f32,
    shadow_map_size: f32,
) -> LightCascade {
    // 1. Build the camera's view-space slice frustum. We compute the
    //    8 frustum corners in world space.
    let corners = frustum_corners_world(view, aspect, near_meters, far_meters);

    // 2. Light-space orthonormal basis: forward = sun_dir, find an
    //    "up" axis that's NOT parallel to sun_dir.
    let sun_forward = sun_dir.normalize();
    let world_up = if sun_forward.y.abs() > 0.99 {
        glm::vec3(0.0, 0.0, 1.0)
    } else {
        glm::vec3(0.0, 1.0, 0.0)
    };
    // Light view matrix: looks ALONG sun direction (eye = above ground,
    // target = below). The eye position is arbitrary for a directional
    // light; we'll place it at the frustum center and let the
    // orthographic projection's near/far do the actual depth bounding.
    let frustum_center = corners
        .iter()
        .fold(glm::Vec3::zeros(), |a, c| a + c) / 8.0;
    // Place the light "eye" some distance up-sun from the center. The
    // distance just needs to be large enough that the eye is outside
    // the frustum AABB; the orthographic projection clips the actual
    // near/far. We use the diagonal of the AABB as a comfortable margin.
    let aabb_diag = aabb_diagonal(&corners);
    let light_eye = frustum_center - sun_forward * aabb_diag;
    let light_view = glm::look_at_rh(&light_eye, &frustum_center, &world_up);

    // 3. ─── Bounding-SPHERE cascade fit ───────────────────────────
    //
    // We previously tight-fit an AABB around the frustum's 8
    // light-space corners, then texel-snapped that AABB. The texel
    // snap stabilizes the projection ONLY if the AABB *size* is
    // constant frame to frame — but it isn't. As the camera rotates,
    // the 8 corners rotate in world space, and their projection into
    // light-space produces an AABB with different X/Y extents
    // depending on the camera's yaw/pitch. The texel size therefore
    // changes, the snap quantum changes, and shadows shift by
    // fractional texels — visible as "stripes" or shimmer crawling
    // across surfaces under tiny rotation.
    //
    // Fix: enclose the frustum slice in a SPHERE. The sphere's
    // radius depends only on the slice's near/far/fov (geometric
    // constants), NOT on camera orientation, so the orthographic
    // extent is rotation-invariant. The center of the sphere shifts
    // with the camera position, but we snap THAT to texel grid.
    //
    // Sphere construction: the smallest sphere enclosing the 8
    // frustum corners is centered on the slice's mid-axis and has
    // radius = max corner distance from center. Computed once, fixed
    // for this cascade.
    let mut radius = 0.0f32;
    for c in &corners {
        radius = radius.max((c - frustum_center).norm());
    }
    // Round the radius up so it doesn't quantize when the camera
    // moves through worlds. This is constant for a given (near,
    // far, fov, aspect) so we can snap once.
    let radius = (radius * 16.0).ceil() / 16.0;

    // The light-space center of the sphere = light_view * frustum_center.
    let center_ls = (light_view * glm::vec4(frustum_center.x, frustum_center.y, frustum_center.z, 1.0)).xyz();

    // Sphere bounds in light space: cube of side 2r centered on center_ls.
    let mut min_ls = center_ls - glm::vec3(radius, radius, radius);
    let mut max_ls = center_ls + glm::vec3(radius, radius, radius);

    // 4. Build the orthographic projection. Pull near back / push far
    //    forward to catch casters outside the slice that still shadow
    //    INTO it (tree behind camera, sun overhead).
    let z_near = -max_ls.z - 50.0;
    let z_far  = -min_ls.z + 50.0;

    // ─── Texel-snap the X/Y bounds (CRITICAL for stability) ────────
    //
    // With the bounding-sphere fit, the cascade's X/Y extent is now a
    // constant `2 * radius`. Texel size = `2r / shadow_map_size` is
    // therefore also constant — the snap quantum doesn't change with
    // camera rotation. We snap min_ls.xy to multiples of the texel
    // size so a fixed world point projects to the same shadow texel
    // every frame as the camera moves smoothly.
    //
    // v0.24.4: `shadow_map_size` is a parameter (not a hardcoded
    // const) so this stays in sync when the user overrides shadow
    // size via UNENDLICH_SHADOW_MAP_SIZE. If this gets stale, the
    // snap quantum disagrees with the rendered texel grid and the
    // shadow shimmers visibly during camera motion.
    let texel = (2.0 * radius) / shadow_map_size;
    if texel > 0.0 {
        min_ls.x = (min_ls.x / texel).floor() * texel;
        min_ls.y = (min_ls.y / texel).floor() * texel;
        max_ls.x = min_ls.x + 2.0 * radius;
        max_ls.y = min_ls.y + 2.0 * radius;
    }

    let ortho = glm::ortho_rh_zo(
        min_ls.x, max_ls.x,
        min_ls.y, max_ls.y,
        z_near, z_far,
    );

    LightCascade {
        view_proj: ortho * light_view,
        near_meters,
        far_meters,
    }
}

/// 8 corners of a camera view-frustum slice in world space.
///
/// Layout: 4 near corners then 4 far corners, each set in
/// (-x,-y), (+x,-y), (-x,+y), (+x,+y) order in camera-local space
/// before being transformed to world.
fn frustum_corners_world(
    view: &View,
    aspect: f32,
    near_meters: f32,
    far_meters: f32,
) -> [glm::Vec3; 8] {
    let half_v_near = (view.fov_y_radians * 0.5).tan() * near_meters;
    let half_h_near = half_v_near * aspect;
    let half_v_far  = (view.fov_y_radians * 0.5).tan() * far_meters;
    let half_h_far  = half_v_far * aspect;

    let forward = view.forward().normalize();
    let right = view.right().normalize();
    let up = glm::cross(&right, &forward).normalize();

    let near_center = view.position + forward * near_meters;
    let far_center  = view.position + forward * far_meters;

    [
        near_center - right * half_h_near - up * half_v_near,
        near_center + right * half_h_near - up * half_v_near,
        near_center - right * half_h_near + up * half_v_near,
        near_center + right * half_h_near + up * half_v_near,
        far_center  - right * half_h_far  - up * half_v_far,
        far_center  + right * half_h_far  - up * half_v_far,
        far_center  - right * half_h_far  + up * half_v_far,
        far_center  + right * half_h_far  + up * half_v_far,
    ]
}

fn aabb_diagonal(corners: &[glm::Vec3; 8]) -> f32 {
    let mut min = glm::vec3(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut max = glm::vec3(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for c in corners {
        min.x = min.x.min(c.x); min.y = min.y.min(c.y); min.z = min.z.min(c.z);
        max.x = max.x.max(c.x); max.y = max.y.max(c.y); max.z = max.z.max(c.z);
    }
    glm::distance(&min, &max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_view() -> View {
        let mut v = View::new(glm::vec3(0.0, 80.0, 0.0));
        v.yaw = 0.0;
        v.pitch = 0.0;
        v
    }

    #[test]
    fn single_cascade_produces_finite_matrix() {
        let view = make_view();
        let sun = glm::vec3(0.5, -0.8, 0.3).normalize();
        let cascade = single_cascade(&view, sun, 16.0 / 9.0, 0.1, 200.0, 2048.0);
        for r in 0..4 {
            for c in 0..4 {
                let v = cascade.view_proj[(r, c)];
                assert!(v.is_finite(), "cascade view_proj[{},{}] is not finite: {}", r, c, v);
            }
        }
    }

    #[test]
    fn frustum_corners_make_sense() {
        let view = make_view();
        let corners = frustum_corners_world(&view, 1.0, 1.0, 100.0);
        // Looking along +X. Near corners should have x ≈ 1.0, far x ≈ 100.0.
        for c in corners.iter().take(4) {
            assert!((c.x - 1.0).abs() < 0.01, "near corner x={} expected ~1.0", c.x);
        }
        for c in corners.iter().skip(4) {
            assert!((c.x - 100.0).abs() < 0.01, "far corner x={} expected ~100.0", c.x);
        }
    }

    #[test]
    fn cascade_records_slice_bounds() {
        let view = make_view();
        let sun = glm::vec3(0.0, -1.0, 0.0); // straight down
        let cascade = single_cascade(&view, sun, 1.0, 5.0, 50.0, 2048.0);
        assert_eq!(cascade.near_meters, 5.0);
        assert_eq!(cascade.far_meters, 50.0);
    }

    #[test]
    fn vertical_sun_does_not_panic() {
        // Edge case: sun straight overhead — the world_up vector
        // selection branch should kick in, not produce a zero basis.
        let view = make_view();
        let sun = glm::vec3(0.0, -1.0, 0.0);
        let cascade = single_cascade(&view, sun, 1.0, 0.1, 100.0, 2048.0);
        for r in 0..4 {
            for c in 0..4 {
                assert!(cascade.view_proj[(r, c)].is_finite());
            }
        }
    }
}
