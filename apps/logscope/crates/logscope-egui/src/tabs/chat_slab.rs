//! chat_slab — the 3D slab's geometry and animation helpers, out of `chat.rs`.
//!
//! Second cut of the 11,614-line file. These seven functions are pure: a camera, a centre and
//! two axes in, a rect or a scalar out. None of them touches `Chat`, which is what makes them
//! separable at all — and being buried between the command dispatch and the message store is
//! why the projection maths was hard to find when the slab was wrong.
//!
//! `mic_shape` and `MicShape` joined this file rather than getting a sixth: they are scene
//! presentation like the rest of it, and a module per function is not a split, it is a
//! filing cabinet.
//!
//! `pub(super)` rather than `pub`: they are `chat`'s internals that happen to live in another
//! file, not a new public surface. A child module, so `use super::*` reaches `chat`'s types
//! (`Ctx`, `MicShape`) without re-importing each one.

use super::*;

/// The mic pearl's SHAPE — a SECOND per-scene derivative (beyond the colour idiom) so the in-scene voice
/// indicator matches each world's physical substance, not just its tint. Kept a FREE derivative (no bespoke
/// per-world table): it falls out of the palette `Material` plus a small crystalline/watery name set —
///   * `Pearl`  — smooth orb: watery/airy worlds (Atlantis, Cloudscape) and paper worlds (a wafer).
///   * `Gem`    — faceted iridescent crystal: the crystalline worlds (Fractal Spires, Recursive Citadel,
///                Frozen Bifrost) — the pass-67 "faceted gem vs smooth pearl" second axis.
///   * `Stone`  — weathered/carved stone bead: rock & marble & cave/mist worlds (Slate, Titan's Ribcage,
///                Mistfall, Mansion Garden…).
///   * `Bead`   — chunky turned wooden bead: the wood-board worlds (Old Library, Archive, Alley…).
///   * `Leaf`   — a backlit leaf blade: the foliage worlds (Forest, Canopy, Treehouse, Hollow Jungle) — the
///                signature "leaves instead of the pearl" object.
/// Rendering maps these onto existing mesh primitives (uv_sphere / cracked_pearl / rock / leaf_single).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MicShape { Pearl, Gem, Stone, Bead, Leaf }

/// Derive the mic pearl shape for a scene from its name + palette material (see `MicShape`). Pure so it can
/// be unit-tested without a renderer (the GUI verification path is headless render; this is the logic gate).
pub fn mic_shape(scene: &str, material: theme::Material) -> MicShape {
    use theme::Material::*;
    match scene {
        // foliage worlds → a backlit LEAF blade (the signature "leaves instead of the pearl" object).
        "Forest" | "Canopy" | "Treehouse" | "Hollow Jungle" => MicShape::Leaf,
        // crystalline worlds → a faceted gem (the shape derivative the pass-67 note calls for).
        "Fractal Spires" | "Recursive Citadel" | "Frozen Bifrost" => MicShape::Gem,
        // cave / carved-rune / mist worlds → weathered stone (the "caves → stones" idiom).
        "Mistfall" => MicShape::Stone,
        // watery / airy worlds → a smooth pearl regardless of their (stone) material.
        "Atlantis" | "Cloudscape" => MicShape::Pearl,
        // everything else follows its physical material.
        _ => match material {
            Wood => MicShape::Bead,          // a turned wooden bead
            Paper | None => MicShape::Pearl, // a paper wafer reads as a smooth bead
            Stone | Marble => MicShape::Stone,
        },
    }
}

/// Project a billboarded in-scene UI slab (centre + right/up basis, size w×h in world units) to its
/// screen-space bounding Rect. This is the basis for making in-scene panels/buttons INTERACTIVE and
/// ACCESSIBLE: the 3D slab is the visual, and an egui widget placed over this rect is the hit-test +
/// AccessKit overlay, so pointer clicks and screen-reader focus both target the physical object in the
/// scene. Returns None if any corner is behind the camera (don't interact with an off-screen slab).
pub(super) fn project_slab_rect(cam: &crate::spatial::OrbitCam, center: crate::spatial::Vec3,
                         right: crate::spatial::Vec3, up: crate::spatial::Vec3, w: f32, h: f32,
                         cx: f32, cy: f32, scale: f32) -> Option<egui::Rect> {
    let (hw, hh) = (w * 0.5, h * 0.5);
    let corner = |sx: f32, sy: f32| crate::spatial::Vec3::new(
        center.x + right.x * (sx * hw) + up.x * (sy * hh),
        center.y + right.y * (sx * hw) + up.y * (sy * hh),
        center.z + right.z * (sx * hw) + up.z * (sy * hh));
    let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let (x, y, _d) = cam.project(corner(sx, sy), cx, cy, scale)?;
        lo.0 = lo.0.min(x); lo.1 = lo.1.min(y); hi.0 = hi.0.max(x); hi.1 = hi.1.max(y);
    }
    Some(egui::Rect::from_min_max(egui::pos2(lo.0, lo.1), egui::pos2(hi.0, hi.1)))
}

/// A specular GLINT for a camera-facing physical slab: a soft bright highlight that sits where the scene's sun
/// reflects toward the eye. As the camera orbits (or the slab moves), the glint slides across the surface, so
/// light genuinely *plays* over the slab instead of it reading as a flat quad. `sun` is the world sun dir;
/// `hw`/`hu` are the slab half-width/half-height. Intensity rises when the sun faces the slab toward the eye.
pub(super) fn slab_glint(graph: &mut Vec<crate::mesh3d::Mesh3>, c: crate::spatial::Vec3, right: crate::spatial::Vec3,
              up: crate::spatial::Vec3, toward: crate::spatial::Vec3, hw: f32, hu: f32,
              sun: crate::spatial::Vec3, tglow: f32) {
    let dot = |a: crate::spatial::Vec3, b: crate::spatial::Vec3| a.x * b.x + a.y * b.y + a.z * b.z;
    let du = dot(right, sun).clamp(-1.0, 1.0);
    let dv = dot(up, sun).clamp(-1.0, 1.0);
    // brightness varies with how the slab faces the sun (either sign — the sign convention is refined with the
    // viewer), but stays present so it reads as a moving sheen rather than blinking on/off.
    let inten = 0.9 + 0.7 * dot(toward, sun).abs();
    let (gu, gv) = (du * hw * 0.72, dv * hu * 0.72);
    let gc = crate::spatial::Vec3::new(c.x + right.x * gu + up.x * gv - toward.x * 0.03,
                                       c.y + right.y * gu + up.y * gv - toward.y * 0.03,
                                       c.z + right.z * gu + up.z * gv - toward.z * 0.03);
    let r = (hw.min(hu) * 0.5).clamp(0.05, 0.4);
    let mut glint = crate::mesh3d::uv_sphere(gc, r, 6, 4, egui::Color32::from_rgb(255, 251, 236));
    glint.emissive = inten * tglow.max(0.6);
    crate::mesh3d::set_alpha(&mut glint, 135);
    graph.push(glint);
}

/// A subtle per-scene emissive undulation (~0.85..1.25) so physical UI slabs catch the scene's light rhythm —
/// light rippling on Atlantis glass, a slow sway in Forest, an icy glint in Frozen — instead of reading flat.
pub(super) fn slab_sheen(anim: crate::mesh3d::TextAnim, t: f32) -> f32 {
    use crate::mesh3d::TextAnim::*;
    match anim {
        Ripple => 1.0 + 0.14 * (t * 1.6).sin(),
        Sway => 1.0 + 0.08 * (t * 1.1).sin(),
        Shimmer => 1.0 + 0.22 * ((t * 3.0).sin() * 0.5 + 0.5).powi(4),
        Drift => 1.0 + 0.10 * (t * 0.8).sin(),
        Fade => 0.9 + 0.22 * (0.5 + 0.5 * (t * 1.4).sin()),
        Pulse => 1.0 + 0.16 * (t * 2.2).sin(),
        Plain => 1.0,
    }
}

/// Local screen scale (pixels per world unit) at `cen`'s depth, along right and up — for sizing physical UI
/// slabs to a target fraction of the canvas so they don't blow up or shrink with window size.
pub(super) fn px_per_world(cam: &crate::spatial::OrbitCam, cen: crate::spatial::Vec3, right: crate::spatial::Vec3, up: crate::spatial::Vec3, cx: f32, cy: f32, scale: f32) -> (f32, f32) {
    let pt = |p: crate::spatial::Vec3| cam.project(p, cx, cy, scale).map(|(x, y, _)| (x, y));
    let add = |a: crate::spatial::Vec3, b: crate::spatial::Vec3| crate::spatial::Vec3::new(a.x + b.x, a.y + b.y, a.z + b.z);
    match (pt(cen), pt(add(cen, right)), pt(add(cen, up))) {
        (Some(p0), Some(pr), Some(pu)) => (((pr.0 - p0.0).powi(2) + (pr.1 - p0.1).powi(2)).sqrt().max(1.0),
                                           ((pu.0 - p0.0).powi(2) + (pu.1 - p0.1).powi(2)).sqrt().max(1.0)),
        _ => (scale.max(1.0), scale.max(1.0)),
    }
}

/// Unproject a screen point `(ax, ay)` back to a world position on the plane through `cen` spanned by
/// `right`/`up` (where the billboarded physical UI lives), so UI components can be ANCHORED to screen
/// positions that adapt to canvas size instead of using fixed world offsets. Falls back to `cen`.
pub(super) fn screen_to_world(cam: &crate::spatial::OrbitCam, cen: crate::spatial::Vec3, right: crate::spatial::Vec3, up: crate::spatial::Vec3, cx: f32, cy: f32, scale: f32, ax: f32, ay: f32) -> crate::spatial::Vec3 {
    let add = |a: crate::spatial::Vec3, b: crate::spatial::Vec3| crate::spatial::Vec3::new(a.x + b.x, a.y + b.y, a.z + b.z);
    let (p0, pr, pu) = match (cam.project(cen, cx, cy, scale), cam.project(add(cen, right), cx, cy, scale), cam.project(add(cen, up), cx, cy, scale)) {
        (Some(a), Some(b), Some(c)) => (a, b, c),
        _ => return cen,
    };
    let (drx, dry) = (pr.0 - p0.0, pr.1 - p0.1);
    let (dux, duy) = (pu.0 - p0.0, pu.1 - p0.1);
    let det = drx * duy - dux * dry;
    if det.abs() < 1e-4 { return cen; }
    let (tx, ty) = (ax - p0.0, ay - p0.1);
    let wr = (tx * duy - dux * ty) / det;
    let wu = (drx * ty - tx * dry) / det;
    crate::spatial::Vec3::new(cen.x + right.x * wr + up.x * wu, cen.y + right.y * wr + up.y * wu, cen.z + right.z * wr + up.z * wu)
}

pub(super) fn hud_place(
    placed: &mut Vec<crate::hud_pack::Placed>, avail: egui::Rect,
    cam: &crate::spatial::OrbitCam, right: crate::spatial::Vec3, up: crate::spatial::Vec3,
    cx: f32, cy: f32, scale: f32,
    world_c: crate::spatial::Vec3, world_w: f32, world_h: f32, pad: f32,
) -> crate::spatial::Vec3 {
    let Some((sx, sy, _)) = cam.project(world_c, cx, cy, scale) else { return world_c };
    let (ppw_x, ppw_y) = px_per_world(cam, world_c, right, up, cx, cy, scale);
    let desired = egui::Rect::from_center_size(egui::pos2(sx, sy), egui::vec2(world_w * ppw_x, world_h * ppw_y));
    let resolved = crate::hud_pack::place(placed, avail, desired, pad);
    screen_to_world(cam, world_c, right, up, cx, cy, scale, resolved.center().x, resolved.center().y)
}

/// Map a scene/theme name to its text-animation idiom, so each scene animates its UI text in character.
pub(super) fn text_anim_for(name: &str) -> crate::mesh3d::TextAnim {
    use crate::mesh3d::TextAnim::*;
    match name {
        "Atlantis" => Ripple,
        "Forest" | "Treehouse" | "Canopy" => Sway,
        "Frozen Bifrost" => Shimmer,
        "Cloudscape" => Drift,
        "Mistfall" => Fade,
        "Folding@home" => Pulse,
        _ => Plain, // Archive / Slate / Alley and any others: the steady twinkle
    }
}
