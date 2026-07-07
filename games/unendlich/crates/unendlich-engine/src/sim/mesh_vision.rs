//! `mesh_vision` — vision-driven mesh scheduling (EXPERIMENTAL, r239).
//!
//! Today a chunk is meshed when it loads or its blocks change, **blind to
//! whether anything can see it**. The GPU frustum-culls at *draw* time
//! (`render::chunk_indirect`), but the expensive part — building the vertex
//! data — already happened. For dense **cave** chunks this is the worst case:
//! a cubic chunk of twisty rock has a huge interior surface area, almost all of
//! it occluded by other rock, and we mesh every face of it even though the
//! player will only ever see the few faces along the corridor they're walking.
//!
//! This module is the **decision model** for meshing *what vision cones need*
//! instead of everything: given the actors that matter (the player, plus mobs
//! whose POV can drive nearby detail) as **view cones** (position + facing +
//! fov + range), and a chunk's bounds, it returns a [`MeshUrgency`]:
//!
//! - **`Now`** — in a cone, near, and not occluded → mesh this frame.
//! - **`Soon`** — in a cone but far, or just outside the fov margin (about to
//!   rotate into view) → mesh at low priority / prefetch.
//! - **`Defer`** — loaded but not currently needed by any cone (behind an
//!   actor, or fully occluded) → keep the voxels, skip the mesh until a cone
//!   turns toward it. This is where the cave-chunk saving lives.
//!
//! Generalises beyond caves to most blocks: any chunk no cone needs is
//! deferred, so meshing cost tracks *what's about to be looked at*, not the
//! whole loaded set. Combined with the existing draw-time frustum cull, the
//! pipeline does visible-work twice over (mesh the soon-visible, draw the
//! visible) rather than meshing the world.
//!
//! **Occlusion** is modelled as an injected predicate (`occluded`) rather than
//! hardcoded: the integration backs it with the engine's raycast / a cheap
//! chunk-level visibility test / GPU occlusion queries. Here we just take "is
//! the chunk's representative point hidden behind solid blocks from this eye?"
//! — so the model is pure and testable, and the real occlusion source plugs in
//! at the call site.
//!
//! Cone math mirrors `sim::perception` (`being_watched` / `horiz_angle`) so
//! mesh-vision and AI-vision agree on what "in view" means. Pure + det.

#![allow(dead_code)]

/// A view cone: an eye at `pos` looking along `yaw` (radians, world XZ) with a
/// half-angle `half_fov` and a `range` (blocks). The player is one; mobs near
/// the player can contribute theirs so their immediate surroundings stay
/// detailed even slightly off the player's own axis.
#[derive(Clone, Copy, Debug)]
pub struct ViewCone {
    pub pos: [f32; 3],
    pub yaw: f32,
    pub half_fov: f32,
    pub range: f32,
    /// A margin (radians) added to `half_fov` for the *Soon* band — geometry
    /// just outside the fov that a small turn would reveal, so it's prefetched
    /// rather than hitching when the actor rotates.
    pub turn_margin: f32,
}

impl ViewCone {
    /// The player cone with sensible defaults (≈90° fov → half 0.785, a 30°
    /// turn margin).
    pub fn player(pos: [f32; 3], yaw: f32, range: f32) -> ViewCone {
        ViewCone { pos, yaw, half_fov: 0.785, range, turn_margin: 0.52 }
    }
}

/// How urgently a chunk needs meshing, given the cones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MeshUrgency {
    /// Not needed by any cone right now — keep voxels, skip meshing.
    Defer = 0,
    /// In a cone but far, or just outside fov (about to be seen) — low prio.
    Soon = 1,
    /// Visible, near, unoccluded — mesh now.
    Now = 2,
}

/// Horizontal angle from `from` to `to` in the XZ plane (matches
/// `sim::perception::horiz_angle`).
fn horiz_angle(from: [f32; 3], to: [f32; 3]) -> f32 {
    (to[2] - from[2]).atan2(to[0] - from[0])
}

fn ang_diff(a: f32, b: f32) -> f32 {
    let mut d = (a - b).abs() % std::f32::consts::TAU;
    if d > std::f32::consts::PI {
        d = std::f32::consts::TAU - d;
    }
    d
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Urgency of one chunk (represented by its center `chunk_center`) for a single
/// cone. `occluded` is the injected line-of-sight test: `true` if the chunk's
/// representative point is hidden behind solid blocks from `cone.pos`.
///
/// - Beyond `range` (with a little slack) → `Defer`.
/// - Within range & within `half_fov` & not occluded → `Now` if within the
///   near half of range, else `Soon` (in view but far).
/// - Within range but only within `half_fov + turn_margin` (the about-to-turn
///   band), or in-fov-but-occluded → `Soon`.
/// - Otherwise (behind the actor) → `Defer`.
pub fn urgency_for_cone(cone: &ViewCone, chunk_center: [f32; 3], occluded: bool) -> MeshUrgency {
    let d = dist(cone.pos, chunk_center);
    // a chunk the eye is basically inside is always needed.
    if d < f32::EPSILON {
        return MeshUrgency::Now;
    }
    if d > cone.range * 1.1 {
        return MeshUrgency::Defer;
    }
    let to = horiz_angle(cone.pos, chunk_center);
    let off_axis = ang_diff(cone.yaw, to);

    if off_axis <= cone.half_fov {
        // in the field of view
        if occluded {
            // hidden behind rock — defer the mesh, but keep it Soon-ish if it's
            // near (a step could expose it); far+occluded → Defer.
            return if d <= cone.range * 0.5 { MeshUrgency::Soon } else { MeshUrgency::Defer };
        }
        if d <= cone.range * 0.5 {
            MeshUrgency::Now
        } else {
            MeshUrgency::Soon
        }
    } else if off_axis <= cone.half_fov + cone.turn_margin {
        // just outside the fov — about to rotate in; prefetch.
        MeshUrgency::Soon
    } else {
        // behind the actor
        MeshUrgency::Defer
    }
}

/// Urgency across *all* cones — a chunk takes the **max** urgency any cone
/// assigns it (if the player can't see it but a nearby mob can, it still gets
/// meshed). `occluded_for` returns the occlusion flag for `(cone_index,
/// chunk)`; the integration backs it with the real LOS test per eye.
pub fn urgency_for_cones(
    cones: &[ViewCone],
    chunk_center: [f32; 3],
    mut occluded_for: impl FnMut(usize, [f32; 3]) -> bool,
) -> MeshUrgency {
    let mut best = MeshUrgency::Defer;
    for (i, cone) in cones.iter().enumerate() {
        let u = urgency_for_cone(cone, chunk_center, occluded_for(i, chunk_center));
        if u > best {
            best = u;
        }
        if best == MeshUrgency::Now {
            break; // can't beat Now
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: f32 = 100.0;

    fn cone_facing_pos_x() -> ViewCone {
        // eye at origin, looking +X (yaw 0), 90° fov, range 100.
        ViewCone::player([0.0, 0.0, 0.0], 0.0, R)
    }

    #[test]
    fn near_in_view_unoccluded_is_now() {
        let c = cone_facing_pos_x();
        // straight ahead, near (25 < 50 = half range), clear.
        assert_eq!(urgency_for_cone(&c, [25.0, 0.0, 0.0], false), MeshUrgency::Now);
    }

    #[test]
    fn far_in_view_is_soon() {
        let c = cone_facing_pos_x();
        // straight ahead but far (80 > 50), clear → in view but not urgent.
        assert_eq!(urgency_for_cone(&c, [80.0, 0.0, 0.0], false), MeshUrgency::Soon);
    }

    #[test]
    fn behind_the_actor_is_deferred() {
        let c = cone_facing_pos_x();
        // directly behind (−X) → outside fov+margin → defer.
        assert_eq!(urgency_for_cone(&c, [-25.0, 0.0, 0.0], false), MeshUrgency::Defer);
    }

    #[test]
    fn occluded_near_in_view_defers_to_soon_not_now() {
        let c = cone_facing_pos_x();
        // near + in view but hidden behind rock (the cave case): not Now.
        let u = urgency_for_cone(&c, [25.0, 0.0, 0.0], true);
        assert_eq!(u, MeshUrgency::Soon, "occluded near chunk is Soon, not Now");
    }

    #[test]
    fn occluded_far_in_view_is_deferred() {
        let c = cone_facing_pos_x();
        // far + in view + occluded → the dense-cave-interior case → defer.
        assert_eq!(urgency_for_cone(&c, [80.0, 0.0, 0.0], true), MeshUrgency::Defer);
    }

    #[test]
    fn out_of_range_is_deferred() {
        let c = cone_facing_pos_x();
        assert_eq!(urgency_for_cone(&c, [200.0, 0.0, 0.0], false), MeshUrgency::Defer);
    }

    #[test]
    fn just_outside_fov_is_soon_prefetch() {
        let c = cone_facing_pos_x(); // half_fov 0.785, margin 0.52
        // ~60° off axis: outside fov (45°) but inside fov+margin (~75°).
        let ang = 1.05f32; // radians ≈ 60°
        let p = [25.0 * ang.cos(), 0.0, 25.0 * ang.sin()];
        assert_eq!(urgency_for_cone(&c, p, false), MeshUrgency::Soon);
    }

    #[test]
    fn max_across_cones_wins() {
        // player can't see it (behind), but a mob cone facing it can → Now.
        let player = ViewCone::player([0.0, 0.0, 0.0], 0.0, R); // looks +X
        let chunk = [-25.0, 0.0, 0.0]; // behind player
        let mob = ViewCone::player([-50.0, 0.0, 0.0], 0.0, R); // at −50 looking +X → sees the chunk at −25
        let u = urgency_for_cones(&[player, mob], chunk, |_, _| false);
        assert_eq!(u, MeshUrgency::Now, "a mob that can see it pulls it to Now");
    }

    #[test]
    fn nothing_sees_it_defers() {
        let player = ViewCone::player([0.0, 0.0, 0.0], 0.0, R);
        let chunk = [-25.0, 0.0, 0.0]; // behind the only cone
        assert_eq!(urgency_for_cones(&[player], chunk, |_, _| false), MeshUrgency::Defer);
    }
}
