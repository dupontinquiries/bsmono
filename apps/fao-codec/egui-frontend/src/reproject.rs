//! reproject — per-pixel motion from depth and camera pose.
//!
//! ## Correcting what I wrote earlier
//!
//! `NEURAL_UPSCALE_APPLICABILITY.md` says FAO has block motion vectors but "no
//! depth buffer", and concludes that occlusion boundaries are where a temporal
//! upscaler would fail with no way to fix it. **That was wrong, and it was wrong
//! because I read `fao_codec_core` and stopped.**
//!
//! v8's own header states the design: *"Per-frame depth map (each pixel gets a
//! Z value)"*, a triangle mesh of major surfaces, and — the line that settles
//! it — *"CompressionPass — use mesh+texture to encode each frame as 'render
//! this mesh from this camera pose'. Frame becomes a 12-byte camera matrix
//! instead of pixel data."*
//!
//! So FAO carries depth, rotation and zoom as first-class bitstream content.
//! That is not a codec lacking the third ingredient of a game upscaler; it is a
//! codec whose entire thesis is to have all three.
//!
//! ## Why that changes the ranking, not just the verdict
//!
//! A game engine DERIVES motion vectors from geometry it already has. FAO's
//! geometry IS the bitstream. Where a renderer writes a motion vector buffer as
//! a side effect, a v8 decoder can compute the same thing from data it decoded
//! anyway — and unlike a renderer, it also knows the depth of the *previous*
//! frame, which is what makes disocclusion detectable rather than guessable.
//!
//! ## And it changes what "residual" should mean
//!
//! The goal stated for this work is to store less residual. Reprojection is how:
//! if the decoder can predict frame N from frame N-1's pixels plus this frame's
//! depth and pose, the encoder only has to send what reprojection gets WRONG —
//! disocclusions, specular changes, moving objects. Those are small and
//! spatially clustered, which is exactly the shape entropy coding likes.
//!
//! PSNR is the wrong yardstick for judging that, and the reason is specific: a
//! reprojected frame that is geometrically right but shifted half a pixel scores
//! terribly on PSNR while looking correct, whereas a blurred frame that is
//! wrong everywhere-but-slightly scores well and looks soft. Optimising a
//! reprojecting codec against PSNR steers it toward blur, which is the one
//! artefact this design should not have. See `masked_error` below.

/// A camera, mirroring `v8_reconstruction_codec::CameraPose`.
///
/// Duplicated as a small plain type rather than depended on, because this is
/// analysis code that must be testable without constructing a whole codec — and
/// because the fields that matter for reprojection are these five, not the
/// distortion model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub position: [f32; 3],
    /// Quaternion (w, x, y, z).
    pub rotation: [f32; 4],
    /// fx, fy in pixels.
    pub focal: [f32; 2],
    /// cx, cy.
    pub principal: [f32; 2],
}

impl Default for Pose {
    fn default() -> Self {
        Pose { position: [0.0; 3], rotation: [1.0, 0.0, 0.0, 0.0],
               focal: [1000.0, 1000.0], principal: [0.0, 0.0] }
    }
}

fn quat_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let (w, x, y, z) = (q[0], q[1], q[2], q[3]);
    // v + 2 * cross(q.xyz, cross(q.xyz, v) + w*v)
    let t = [2.0 * (y * v[2] - z * v[1]),
             2.0 * (z * v[0] - x * v[2]),
             2.0 * (x * v[1] - y * v[0])];
    [v[0] + w * t[0] + (y * t[2] - z * t[1]),
     v[1] + w * t[1] + (z * t[0] - x * t[2]),
     v[2] + w * t[2] + (x * t[1] - y * t[0])]
}

fn quat_conj(q: [f32; 4]) -> [f32; 4] { [q[0], -q[1], -q[2], -q[3]] }

impl Pose {
    /// Unproject a pixel at depth `z` into world space.
    ///
    /// `z` is depth along the camera axis, not distance from the eye. Using
    /// range instead would bend a flat wall into a sphere, which is the
    /// classic sign that someone mixed the two.
    pub fn unproject(&self, x: f32, y: f32, z: f32) -> [f32; 3] {
        let cam = [(x - self.principal[0]) * z / self.focal[0],
                   (y - self.principal[1]) * z / self.focal[1],
                   z];
        let world = quat_rotate(self.rotation, cam);
        [world[0] + self.position[0],
         world[1] + self.position[1],
         world[2] + self.position[2]]
    }

    /// Project a world point back to a pixel, with its camera-space depth.
    ///
    /// Returns `None` for points behind the camera. A projection that let them
    /// through produces a mirrored ghost in front — the artefact that makes a
    /// naive reprojector look haunted.
    pub fn project(&self, p: [f32; 3]) -> Option<(f32, f32, f32)> {
        let rel = [p[0] - self.position[0], p[1] - self.position[1],
                   p[2] - self.position[2]];
        let cam = quat_rotate(quat_conj(self.rotation), rel);
        if cam[2] <= 1e-6 { return None; }
        Some((self.principal[0] + cam[0] * self.focal[0] / cam[2],
              self.principal[1] + cam[1] * self.focal[1] / cam[2],
              cam[2]))
    }
}

/// Where a pixel from the previous frame lands in this one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reprojected {
    /// Destination pixel in the current frame.
    pub to: (f32, f32),
    /// Motion vector, current minus previous.
    pub motion: (f32, f32),
    /// Depth in the current camera.
    pub depth: f32,
}

/// Reproject one pixel of the previous frame into the current one.
///
/// This is the per-pixel motion vector a temporal upscaler needs, computed from
/// data a v8 decoder already has. A block motion vector approximates it; this
/// IS it, including rotation and zoom, which block matching represents badly
/// and a mesh represents exactly.
pub fn reproject(prev: &Pose, cur: &Pose, x: f32, y: f32, depth: f32)
    -> Option<Reprojected> {
    // A non-positive or non-finite depth is a hole in the depth map, not a
    // point at the camera. Reprojecting it would smear the image origin across
    // the frame.
    if !depth.is_finite() || depth <= 0.0 { return None; }
    let world = prev.unproject(x, y, depth);
    let (nx, ny, nz) = cur.project(world)?;
    Some(Reprojected { to: (nx, ny), motion: (nx - x, ny - y), depth: nz })
}

/// How much closer a reprojected sample must be than the current depth before
/// it counts as occluded, as a fraction.
///
/// Relative rather than absolute because depth error scales with depth: 5cm is
/// decisive at one metre and meaningless at fifty.
pub const OCCLUSION_TOLERANCE: f32 = 0.02;

/// Did this reprojection land on a surface that is no longer visible?
///
/// The test a game upscaler uses depth for, and the one I said FAO could not
/// do. `cur_depth` is what the current frame's depth map says is at that pixel.
///
/// Three outcomes rather than a boolean, because they call for different
/// handling: agreement means reuse the history, occlusion means something moved
/// in front and the history is of the wrong surface, and disocclusion means
/// nothing was there before and the encoder must send it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// The reprojected sample matches what is there now: history is valid.
    Consistent,
    /// Something nearer is at that pixel now: the history is a hidden surface.
    Occluded,
    /// The reprojection went off-frame, or there is no history for this pixel.
    Disoccluded,
}

/// Classify a reprojected sample against the current depth map.
pub fn visibility(r: &Reprojected, cur_depth: Option<f32>,
                  width: usize, height: usize) -> Visibility {
    // Off-frame is disocclusion: the content genuinely was not observed before.
    if r.to.0 < 0.0 || r.to.1 < 0.0
        || r.to.0 >= width as f32 || r.to.1 >= height as f32 {
        return Visibility::Disoccluded;
    }
    let Some(d) = cur_depth else { return Visibility::Disoccluded };
    if !d.is_finite() || d <= 0.0 { return Visibility::Disoccluded; }
    // The reprojected surface is BEHIND what is there now by more than
    // tolerance: something moved in front of it.
    if r.depth > d * (1.0 + OCCLUSION_TOLERANCE) { return Visibility::Occluded; }
    Visibility::Consistent
}

/// Fraction of pixels the encoder must actually send.
///
/// The number this whole approach is trying to reduce: if reprojection is
/// consistent, the decoder already has those pixels and the residual is zero —
/// only occlusions and disocclusions need coding.
pub fn residual_fraction(v: &[Visibility]) -> f32 {
    if v.is_empty() { return 0.0; }
    let n = v.iter().filter(|x| **x != Visibility::Consistent).count();
    n as f32 / v.len() as f32
}

/// Mean absolute error over pixels the encoder is responsible for.
///
/// **Not** PSNR over the whole frame, and that is the point. A reprojected
/// frame that is geometrically right but shifted half a pixel scores terribly
/// on whole-frame PSNR while looking correct; a blurred frame that is wrong
/// everywhere-but-slightly scores well and looks soft. Optimising a
/// reprojecting codec against PSNR steers it toward blur, which is the one
/// artefact this design must not have.
///
/// Measuring error only where the encoder had to send something separates "the
/// prediction was good" from "the residual coding was good", which are the two
/// things a rate-distortion decision needs to weigh and which a single PSNR
/// number fuses into one.
pub fn masked_error(err: &[f32], v: &[Visibility]) -> Option<f32> {
    let pairs: Vec<f32> = err.iter().zip(v)
        .filter(|(_, vis)| **vis != Visibility::Consistent)
        .map(|(e, _)| e.abs())
        .collect();
    if pairs.is_empty() { return None; }
    Some(pairs.iter().sum::<f32>() / pairs.len() as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32, y: f32, z: f32) -> Pose {
        Pose { position: [x, y, z], principal: [160.0, 120.0], ..Default::default() }
    }

    #[test]
    fn a_static_camera_produces_no_motion() {
        // The base case, and the one that catches a sign error immediately.
        let p = at(0.0, 0.0, 0.0);
        let r = reproject(&p, &p, 100.0, 90.0, 5.0).expect("in front");
        assert!(r.motion.0.abs() < 1e-3 && r.motion.1.abs() < 1e-3, "{:?}", r.motion);
        assert!((r.to.0 - 100.0).abs() < 1e-3);
    }

    #[test]
    fn a_dolly_forward_expands_the_frame_from_the_principal_point() {
        // Zoom by translation: points move AWAY from the centre as the camera
        // approaches, which block matching represents as a field of divergent
        // vectors and geometry represents exactly.
        let prev = at(0.0, 0.0, 0.0);
        let cur = at(0.0, 0.0, 1.0);
        let left = reproject(&prev, &cur, 60.0, 120.0, 5.0).expect("visible");
        assert!(left.to.0 < 60.0, "a point left of centre moves further left");
        let right = reproject(&prev, &cur, 260.0, 120.0, 5.0).expect("visible");
        assert!(right.to.0 > 260.0);
    }

    #[test]
    fn nearer_geometry_moves_more_under_the_same_camera_motion() {
        // Parallax. This is the information a single block motion vector cannot
        // carry, because one block can contain both depths.
        let prev = at(0.0, 0.0, 0.0);
        let cur = at(0.5, 0.0, 0.0);
        let near = reproject(&prev, &cur, 200.0, 120.0, 2.0).unwrap();
        let far = reproject(&prev, &cur, 200.0, 120.0, 20.0).unwrap();
        assert!(near.motion.0.abs() > far.motion.0.abs(),
                "near {:?} far {:?}", near.motion, far.motion);
    }

    #[test]
    fn a_point_behind_the_camera_is_refused() {
        // Letting it through produces a mirrored ghost in front — the artefact
        // that makes a naive reprojector look haunted.
        let prev = at(0.0, 0.0, 0.0);
        let cur = at(0.0, 0.0, 10.0);
        assert_eq!(reproject(&prev, &cur, 200.0, 120.0, 5.0), None);
    }

    #[test]
    fn a_hole_in_the_depth_map_is_not_a_point_at_the_camera() {
        // Reprojecting depth 0 would smear the image origin across the frame.
        let p = at(0.0, 0.0, 0.0);
        assert_eq!(reproject(&p, &p, 100.0, 90.0, 0.0), None);
        assert_eq!(reproject(&p, &p, 100.0, 90.0, f32::NAN), None);
        assert_eq!(reproject(&p, &p, 100.0, 90.0, -3.0), None);
    }

    #[test]
    fn unproject_and_project_round_trip() {
        let p = Pose { position: [1.0, -2.0, 3.0], rotation: [1.0, 0.0, 0.0, 0.0],
                       focal: [800.0, 800.0], principal: [160.0, 120.0] };
        let w = p.unproject(200.0, 90.0, 7.0);
        let (x, y, z) = p.project(w).expect("in front");
        assert!((x - 200.0).abs() < 1e-2 && (y - 90.0).abs() < 1e-2);
        assert!((z - 7.0).abs() < 1e-3);
    }

    #[test]
    fn rotation_round_trips_too() {
        // A 90-degree yaw. If the quaternion convention were wrong the round
        // trip would land somewhere plausible but not here, which is why the
        // test checks the value rather than just finiteness.
        let s = (std::f32::consts::FRAC_PI_4).sin();
        let c = (std::f32::consts::FRAC_PI_4).cos();
        let p = Pose { rotation: [c, 0.0, s, 0.0], principal: [160.0, 120.0],
                       ..Default::default() };
        let w = p.unproject(210.0, 130.0, 4.0);
        let (x, y, _) = p.project(w).expect("in front");
        assert!((x - 210.0).abs() < 1e-2 && (y - 130.0).abs() < 1e-2);
    }

    #[test]
    fn a_matching_depth_means_the_history_is_reusable() {
        let r = Reprojected { to: (100.0, 100.0), motion: (2.0, 0.0), depth: 5.0 };
        assert_eq!(visibility(&r, Some(5.0), 320, 240), Visibility::Consistent);
        assert_eq!(visibility(&r, Some(5.05), 320, 240), Visibility::Consistent,
                   "within tolerance");
    }

    #[test]
    fn a_nearer_surface_now_means_the_history_is_hidden() {
        // The test a game upscaler uses depth for — the one I wrote that FAO
        // could not do.
        let r = Reprojected { to: (100.0, 100.0), motion: (2.0, 0.0), depth: 5.0 };
        assert_eq!(visibility(&r, Some(2.0), 320, 240), Visibility::Occluded);
    }

    #[test]
    fn reprojecting_off_frame_is_disocclusion_not_occlusion() {
        // The content genuinely was not observed before, so the encoder must
        // send it — a different action from reusing a different surface.
        let r = Reprojected { to: (-3.0, 100.0), motion: (-9.0, 0.0), depth: 5.0 };
        assert_eq!(visibility(&r, Some(5.0), 320, 240), Visibility::Disoccluded);
        let r2 = Reprojected { to: (400.0, 100.0), motion: (9.0, 0.0), depth: 5.0 };
        assert_eq!(visibility(&r2, Some(5.0), 320, 240), Visibility::Disoccluded);
    }

    #[test]
    fn a_missing_current_depth_is_disocclusion_rather_than_agreement() {
        // Unknown must not be optimistically treated as matching, or the
        // encoder skips a region it cannot reconstruct.
        let r = Reprojected { to: (100.0, 100.0), motion: (0.0, 0.0), depth: 5.0 };
        assert_eq!(visibility(&r, None, 320, 240), Visibility::Disoccluded);
        assert_eq!(visibility(&r, Some(0.0), 320, 240), Visibility::Disoccluded);
    }

    #[test]
    fn the_occlusion_tolerance_is_relative_because_depth_error_scales() {
        // 5cm is decisive at one metre and meaningless at fifty.
        let near = Reprojected { to: (10.0, 10.0), motion: (0.0, 0.0), depth: 1.05 };
        assert_eq!(visibility(&near, Some(1.0), 320, 240), Visibility::Occluded);
        let far = Reprojected { to: (10.0, 10.0), motion: (0.0, 0.0), depth: 50.5 };
        assert_eq!(visibility(&far, Some(50.0), 320, 240), Visibility::Consistent);
    }

    #[test]
    fn only_the_inconsistent_pixels_cost_bits() {
        // The number this whole approach exists to reduce.
        let v = vec![Visibility::Consistent; 96].into_iter()
            .chain(vec![Visibility::Disoccluded; 4])
            .collect::<Vec<_>>();
        assert!((residual_fraction(&v) - 0.04).abs() < 1e-6);
    }

    #[test]
    fn a_fully_consistent_frame_costs_no_residual() {
        // The ideal case: a pure camera move over static geometry becomes a
        // pose and nothing else.
        let v = vec![Visibility::Consistent; 100];
        assert_eq!(residual_fraction(&v), 0.0);
    }

    #[test]
    fn error_is_measured_where_the_encoder_was_responsible() {
        // Not whole-frame PSNR. A frame that is geometrically right but shifted
        // half a pixel scores terribly on PSNR while looking correct, and a
        // blurred frame that is slightly wrong everywhere scores well and looks
        // soft — so optimising against PSNR steers a reprojecting codec toward
        // blur, the one artefact it must not have.
        let err = vec![0.0, 0.0, 8.0, 4.0];
        let vis = vec![Visibility::Consistent, Visibility::Consistent,
                       Visibility::Disoccluded, Visibility::Occluded];
        assert_eq!(masked_error(&err, &vis), Some(6.0));
    }

    #[test]
    fn a_frame_with_nothing_to_send_has_no_masked_error() {
        // None rather than zero: "we sent nothing and it was perfect" and "we
        // sent nothing" are different claims, and only one of them is evidence.
        let vis = vec![Visibility::Consistent; 4];
        assert_eq!(masked_error(&[0.0, 0.0, 0.0, 0.0], &vis), None);
    }
}
