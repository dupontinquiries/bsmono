//! CIRCUIT SHAPES — `circuit_map`'s layout, as real `world::Shape`s.
//!
//! This is the file `circuit_map.rs`'s own doc comment named as not-yet-
//! written: turning a `Placement`/`Waypoint` into the same `Shape` type
//! `world.rs` draws and collides against. It reuses `world::Shape`
//! exactly as-is (mesh id, `Sdf`, colour, `blend`) — no new struct, no
//! new mesh, no new texture. Every mesh id used below is one of
//! `world.rs`'s own `M_*` constants; `every_shape_uses_an_existing_mesh_id`
//! makes that a test rather than a promise, so "zero new asset bytes"
//! stays true even after someone edits this file without rereading the
//! brief.
//!
//! **Still not here**: a world-space anchor for each stop (stops are
//! only ever placed in their own LOCAL space so far — the ship-parks-
//! at-the-origin problem `docs/PRIORITIES.md` §5 already names is the
//! same open question for every stop, not something this file can
//! answer alone), and the actual `World`/`render.rs` wiring that would
//! put these shapes on a GPU. This file produces the `Vec<Shape>`; it
//! does not hand it to anything yet.

use crate::circuit_map::{Fixture, Placement, Waypoint};
use crate::moons::Bridge;
use crate::raidgen::Rng;
use crate::world::{Sdf, Shape};
use glam::{Mat4, Quat, Vec3};

/// (guess) — fixture console footprint, half-extents.
const FIXTURE_HALF: Vec3 = Vec3::new(0.7, 1.1, 0.5);
/// (guess) — a flat pad every fixture stands on, so it doesn't look
/// like it's floating over bare terrain before the ground under a stop
/// is designed.
const PAD_HALF: Vec3 = Vec3::new(1.4, 0.15, 1.4);

fn fixture_color(f: Fixture) -> [f32; 4] {
    // fixed console palette — deliberately NOT derived from
    // `planetlook`: these are player UI objects standing on a moon,
    // not part of the moon's own material story.
    match f {
        Fixture::Rack => [0.85, 0.30, 0.20, 1.0],
        Fixture::Perk => [0.25, 0.55, 0.85, 1.0],
        Fixture::Repair => [0.30, 0.80, 0.35, 1.0],
        Fixture::Forge => [0.90, 0.55, 0.10, 1.0],
        Fixture::Seal { .. } => [0.70, 0.70, 0.75, 1.0],
        Fixture::Crate => [0.80, 0.70, 0.20, 1.0],
        Fixture::Relay => [0.60, 0.20, 0.90, 1.0],
        Fixture::Bank => [0.95, 0.85, 0.30, 1.0],
    }
}

fn shape(mesh: usize, pos: Vec3, rot: Quat, scale: Vec3, sdf: Sdf, color: [f32; 4], emissive: f32) -> Shape {
    Shape {
        mesh,
        mat: Mat4::from_scale_rotation_translation(scale, rot, pos),
        pos,
        inv_rot: rot.inverse(),
        sdf,
        color,
        emissive,
        flash: 0.0,
        vel: Vec3::ZERO,
        omega: Vec3::ZERO,
        spec: 0.4,
        cut: false,
        // same rule `world.rs` states in its own `push` closure: solid
        // built form fillets, spheres stay hard-edged. Every shape this
        // module builds is `RBox`/`Capsule`, so it always blends.
        blend: matches!(sdf, Sdf::RBox { .. } | Sdf::Capsule { .. } | Sdf::Shell { .. }),
    }
}

/// One fixture -> two shapes: the pad it stands on, and the console
/// itself. Stop-local space, same origin `circuit_map::layout` uses.
pub fn fixture_shapes(p: &Placement) -> Vec<Shape> {
    let rot = Quat::from_rotation_y(p.yaw);
    let pad_pos = Vec3::new(p.pos.x, 0.0, p.pos.z);
    let console_pos = Vec3::new(p.pos.x, PAD_HALF.y + FIXTURE_HALF.y, p.pos.z);
    vec![
        shape(
            crate::world::M_BOX,
            pad_pos,
            Quat::IDENTITY,
            PAD_HALF,
            Sdf::RBox { half: PAD_HALF, round: 0.06 },
            [0.35, 0.35, 0.38, 1.0],
            0.0,
        ),
        shape(
            crate::world::M_BOX,
            console_pos,
            rot,
            FIXTURE_HALF,
            Sdf::RBox { half: FIXTURE_HALF, round: 0.08 },
            fixture_color(p.fixture),
            0.35,
        ),
    ]
}

/// Every fixture on a stop, flattened to one shape list.
pub fn stop_shapes(placements: &[Placement]) -> Vec<Shape> {
    placements.iter().flat_map(fixture_shapes).collect()
}

/// A waypoint's position in BRIDGE-local space: `x` runs the bridge's
/// own length along `t`, `z` is the seeded lateral wander, `y` the
/// vertical wander. Not world space — see the module doc's open item.
pub fn waypoint_pos(bridge: &Bridge, w: &Waypoint) -> Vec3 {
    Vec3::new(w.t * bridge.length_m, w.vertical_m, w.lateral_m)
}

/// (guess) — rubble segment radius range; varied per segment so the
/// chain doesn't read as a uniform pipe.
const RUBBLE_R_MIN: f32 = 0.9;
const RUBBLE_R_MAX: f32 = 2.4;

/// The bridge's rubble, as a chain of capsules between consecutive
/// waypoints from `circuit_map::bridge_spline` — "a chain of
/// RBox/Capsule/Shell SDFs on a spline, seeded" per the design brief,
/// literally. One capsule per segment, radius seeded from the SAME
/// `bridge.seed` the spline itself used, so re-deriving the spline and
/// the rubble independently still agrees (no separate seed to drift).
pub fn bridge_rubble_shapes(bridge: &Bridge, spline: &[Waypoint]) -> Vec<Shape> {
    let mut rng = Rng::new(bridge.seed ^ 0xB01D_ED9E);
    let mut out = Vec::with_capacity(spline.len().saturating_sub(1));
    for pair in spline.windows(2) {
        let a = waypoint_pos(bridge, &pair[0]);
        let b = waypoint_pos(bridge, &pair[1]);
        let mid = (a + b) * 0.5;
        let d = b - a;
        let len = d.length();
        if len < 1e-4 {
            continue;
        }
        let dir = d / len;
        let rot = Quat::from_rotation_arc(Vec3::Y, dir);
        let r = rng.f(RUBBLE_R_MIN, RUBBLE_R_MAX);
        let half = (len * 0.5 - r * 0.3).max(0.1); // slight overlap so the chain reads solid, not beaded
        out.push(shape(
            crate::world::M_CAP_TALL,
            mid,
            rot,
            Vec3::splat(r),
            Sdf::Capsule { r, half },
            [0.32, 0.30, 0.29, 1.0],
            0.0,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit_map;
    use crate::moons;

    #[test]
    fn every_shape_uses_an_existing_mesh_id() {
        // zero new asset bytes, enforced: every mesh id this module
        // emits must already be one of world.rs's own M_* constants.
        let known: [usize; 2] = [crate::world::M_BOX, crate::world::M_CAP_TALL];
        for spec in moons::stops() {
            for s in stop_shapes(&circuit_map::layout(&spec)) {
                assert!(known.contains(&s.mesh), "unknown mesh id {}", s.mesh);
            }
        }
        for b in moons::bridges() {
            let sp = circuit_map::bridge_spline(&b);
            for s in bridge_rubble_shapes(&b, &sp) {
                assert!(known.contains(&s.mesh), "unknown mesh id {}", s.mesh);
            }
        }
    }

    #[test]
    fn stop_shapes_count_is_two_per_fixture() {
        for spec in moons::stops() {
            let placements = circuit_map::layout(&spec);
            let shapes = stop_shapes(&placements);
            assert_eq!(shapes.len(), placements.len() * 2, "{}", spec.id);
        }
    }

    #[test]
    fn no_shape_has_a_nan_or_infinite_position() {
        for spec in moons::stops() {
            for s in stop_shapes(&circuit_map::layout(&spec)) {
                assert!(s.pos.is_finite(), "{}: {:?}", spec.id, s.pos);
            }
        }
        for b in moons::bridges() {
            let sp = circuit_map::bridge_spline(&b);
            for s in bridge_rubble_shapes(&b, &sp) {
                assert!(s.pos.is_finite(), "bridge {}->{}: {:?}", b.from, b.to, s.pos);
            }
        }
    }

    #[test]
    fn bridge_rubble_has_one_capsule_per_segment() {
        for b in moons::bridges() {
            let sp = circuit_map::bridge_spline(&b);
            let shapes = bridge_rubble_shapes(&b, &sp);
            assert_eq!(shapes.len(), sp.len() - 1, "bridge {}->{}", b.from, b.to);
        }
    }

    #[test]
    fn rubble_capsules_are_all_actually_capsules_and_blend() {
        for b in moons::bridges() {
            let sp = circuit_map::bridge_spline(&b);
            for s in bridge_rubble_shapes(&b, &sp) {
                assert!(matches!(s.sdf, Sdf::Capsule { .. }));
                assert!(s.blend);
            }
        }
    }

    #[test]
    fn rubble_radius_stays_within_its_own_budget() {
        for b in moons::bridges() {
            let sp = circuit_map::bridge_spline(&b);
            for s in bridge_rubble_shapes(&b, &sp) {
                if let Sdf::Capsule { r, .. } = s.sdf {
                    assert!(r >= RUBBLE_R_MIN - 1e-4 && r <= RUBBLE_R_MAX + 1e-4, "r={r}");
                }
            }
        }
    }

    #[test]
    fn waypoint_pos_endpoints_land_exactly_on_the_bridge_axis() {
        for b in moons::bridges() {
            let sp = circuit_map::bridge_spline(&b);
            let start = waypoint_pos(&b, &sp[0]);
            let end = waypoint_pos(&b, &sp[sp.len() - 1]);
            assert_eq!(start, Vec3::new(0.0, 0.0, 0.0));
            assert_eq!(end, Vec3::new(b.length_m, 0.0, 0.0));
        }
    }

    #[test]
    fn shape_construction_is_deterministic() {
        for spec in moons::stops() {
            let placements = circuit_map::layout(&spec);
            let a = stop_shapes(&placements);
            let b = stop_shapes(&placements);
            for (x, y) in a.iter().zip(b.iter()) {
                assert_eq!(x.pos, y.pos);
                assert_eq!(x.color, y.color);
            }
        }
        for b in moons::bridges() {
            let sp = circuit_map::bridge_spline(&b);
            let x = bridge_rubble_shapes(&b, &sp);
            let y = bridge_rubble_shapes(&b, &sp);
            for (a1, a2) in x.iter().zip(y.iter()) {
                assert_eq!(a1.pos, a2.pos);
                if let (Sdf::Capsule { r: r1, .. }, Sdf::Capsule { r: r2, .. }) = (a1.sdf, a2.sdf) {
                    assert_eq!(r1, r2);
                }
            }
        }
    }

    #[test]
    fn fixture_console_sits_above_its_own_pad() {
        for spec in moons::stops() {
            for p in circuit_map::layout(&spec) {
                let shapes = fixture_shapes(&p);
                let pad = &shapes[0];
                let console = &shapes[1];
                assert!(console.pos.y > pad.pos.y, "{}: console not above pad", spec.id);
            }
        }
    }
}
