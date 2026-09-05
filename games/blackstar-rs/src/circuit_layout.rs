//! CIRCUIT LAYOUT — the one thing every earlier module deferred: a
//! WORLD position.
//!
//! `moons.rs` gives each stop a spec. `circuit_map.rs` gives each stop
//! a fixture layout and each bridge a rubble spline — both in their
//! own LOCAL space, explicitly not anchored (their own doc comments
//! say so). `circuit_shapes.rs` turns those into real `world::Shape`s,
//! still local. Nothing until this file says where a stop actually
//! IS, which means nothing until this file can answer "where is the
//! player right now" — the one fact a camera, a HUD compass, or
//! `escalation`'s spawn placement all eventually need.
//!
//! **The approximation, stated plainly.** Five stops joined by
//! unequal-length bridges cannot sit on a perfect circle with bridge
//! length equal to arc length unless the radius is chosen per-bridge —
//! which a single ring can't do. This module places every stop at the
//! angle its CUMULATIVE bridge length would reach on a circle sized by
//! the ring's TOTAL length (`radius = total / TAU`), which is exact in
//! total circumference and only approximate in per-bridge chord length.
//! Good enough for a ring you fly and walk, wrong for anything claiming
//! to be a physically exact orbit — and `docs/PRIORITIES.md` already
//! has an open item ("the ship parks at the world origin... should be
//! somewhere the map chose") this doesn't try to solve either; it only
//! gives the ring itself an internally consistent shape.
//!
//! Everything here stays in the XZ plane (`y = 0` per anchor) —
//! "varying moon environments" and elevation are a real future step,
//! not attempted blind.

use crate::circuit::{Circuit, Phase, Rider, DOCK, STOP_COUNT};
use crate::moons::{self, Bridge};
use glam::Vec3;

/// World-space position of every stop's own local origin, indexed the
/// same as `circuit::LEG_S`/`moons::stops()`.
pub fn anchors() -> [Vec3; STOP_COUNT] {
    let total: f32 = crate::circuit::LEG_S
        .iter()
        .zip(std::iter::repeat(crate::circuit::WALK_LEG_RATIO))
        .map(|(leg, ratio)| leg * ratio)
        .sum();
    let radius = total / std::f32::consts::TAU;
    let mut cum = 0.0f32;
    std::array::from_fn(|i| {
        let angle = (cum / total) * std::f32::consts::TAU;
        cum += crate::circuit::LEG_S[i] * crate::circuit::WALK_LEG_RATIO;
        Vec3::new(angle.cos() * radius, 0.0, angle.sin() * radius)
    })
}

/// The forward direction along bridge `from -> next_stop(from)`, in
/// world space — the basis `circuit_shapes`'s local `x` (along-bridge)
/// and `z` (lateral) axes map onto once anchored.
fn bridge_basis(anchors: &[Vec3; STOP_COUNT], from: usize) -> (Vec3, Vec3) {
    let to = (from + 1) % STOP_COUNT;
    let fwd = (anchors[to] - anchors[from]).normalize_or(Vec3::X);
    let right = Vec3::new(-fwd.z, 0.0, fwd.x); // 90 deg in the XZ plane
    (fwd, right)
}

/// A bridge-local point (`circuit_shapes::waypoint_pos`'s space: x =
/// along-bridge, y = vertical, z = lateral) placed in world space.
pub fn bridge_local_to_world(anchors: &[Vec3; STOP_COUNT], bridge: &Bridge, local: Vec3) -> Vec3 {
    let (fwd, right) = bridge_basis(anchors, bridge.from);
    anchors[bridge.from] + fwd * local.x + right * local.z + Vec3::Y * local.y
}

/// A stop-local fixture position (`circuit_map::Placement::pos`, XZ
/// plane around the stop's own centre) placed in world space. Stops
/// are not yet given their own rotation, so this is a pure translate —
/// matching the same "not attempted blind" note as elevation above.
pub fn stop_local_to_world(anchors: &[Vec3; STOP_COUNT], stop: usize, local: Vec3) -> Vec3 {
    anchors[stop] + local
}

/// Where the player actually is right now, in world space, given the
/// ring's `Circuit` state. This is the one function a camera or HUD
/// compass would call every frame:
///
/// * `Aboard` — interpolated along the CURRENT leg (or sitting at a
///   stop's anchor while dwelling/warning), so the Lighter's dot moves
///   smoothly on a projector/minimap even between arrivals.
/// * `Walking`/`Cold` — interpolated along the bridge the rider jumped
///   onto, using how much of the walk is left against the bridge's own
///   total length — the same number `circuit::Rider::total_m` already
///   carries, so there is no second distance to keep in sync.
pub fn rider_world_pos(anchors: &[Vec3; STOP_COUNT], circuit: &Circuit) -> Vec3 {
    match circuit.rider {
        Rider::Aboard => match circuit.phase {
            Phase::Dwell { stop, .. } | Phase::Warn { stop, .. } => anchors[stop],
            Phase::Travel { from, progress } => {
                let to = (from + 1) % STOP_COUNT;
                anchors[from].lerp(anchors[to], progress.clamp(0.0, 1.0))
            }
        },
        Rider::Walking { from, remaining_m, total_m } | Rider::Cold { from, remaining_m, total_m } => {
            let to = (from + 1) % STOP_COUNT;
            let walked = if total_m > 1e-4 { 1.0 - (remaining_m / total_m).clamp(0.0, 1.0) } else { 0.0 };
            anchors[from].lerp(anchors[to], walked)
        }
    }
}

/// Every bridge's basis, precomputed — a `--circuit-layout` driver or
/// a future baking pass wants this once per ring, not once per shape.
pub fn all_bridge_bases(anchors: &[Vec3; STOP_COUNT]) -> [(Vec3, Vec3); STOP_COUNT] {
    std::array::from_fn(|i| bridge_basis(anchors, i))
}

/// `--circuit-layout`: prints every stop's world anchor and a few
/// sample rider positions along the ring, so the shape of the ring —
/// not just the numbers behind it — can be read from a terminal.
pub fn run() {
    let a = anchors();
    println!("CIRCUIT LAYOUT — world-space anchors (approximate ring, see module doc)\n");
    for (i, spec) in moons::stops().iter().enumerate() {
        println!("  {} ({}): ({:.1}, {:.1}, {:.1})", i, spec.name, a[i].x, a[i].y, a[i].z);
    }
    println!("\nrider position along a sample crossing (stop 0 -> stop 1):");
    let mut c = Circuit::new();
    c.phase = Phase::Travel { from: 0, progress: 0.0 };
    for p in [0.0, 0.25, 0.5, 0.75, 1.0] {
        c.phase = Phase::Travel { from: 0, progress: p };
        let pos = rider_world_pos(&a, &c);
        println!("  progress {p:.2}: ({:.1}, {:.1}, {:.1})", pos.x, pos.y, pos.z);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_anchor_is_distinct_and_finite() {
        let a = anchors();
        for v in a {
            assert!(v.is_finite());
        }
        for i in 0..STOP_COUNT {
            for j in (i + 1)..STOP_COUNT {
                assert!(a[i].distance(a[j]) > 1.0, "{i} and {j} nearly coincide");
            }
        }
    }

    #[test]
    fn dock_sits_at_angle_zero() {
        let a = anchors();
        // by construction: cumulative length before stop 0 is 0
        assert!((a[DOCK].x - (a[DOCK].length())).abs() < 1e-3, "dock should be on the +X axis");
        assert!(a[DOCK].z.abs() < 1e-3);
    }

    #[test]
    fn anchors_are_deterministic() {
        assert_eq!(anchors(), anchors());
    }

    #[test]
    fn total_ring_circumference_matches_total_bridge_length() {
        let a = anchors();
        let radius = a[DOCK].length();
        let circumference = radius * std::f32::consts::TAU;
        let total: f32 = crate::circuit::LEG_S.iter().map(|l| l * crate::circuit::WALK_LEG_RATIO).sum();
        assert!((circumference - total).abs() < 1e-2, "{circumference} vs {total}");
    }

    #[test]
    fn rider_world_pos_at_a_dwelling_stop_is_exactly_that_anchor() {
        let a = anchors();
        let c = Circuit::new(); // Dwell at DOCK
        assert_eq!(rider_world_pos(&a, &c), a[DOCK]);
    }

    #[test]
    fn rider_world_pos_mid_travel_lies_between_the_two_anchors() {
        let a = anchors();
        let mut c = Circuit::new();
        c.phase = Phase::Travel { from: 2, progress: 0.5 };
        let pos = rider_world_pos(&a, &c);
        let midpoint = a[2].lerp(a[3], 0.5);
        assert!(pos.distance(midpoint) < 1e-3);
    }

    #[test]
    fn rider_world_pos_advances_monotonically_along_a_leg() {
        let a = anchors();
        let mut c = Circuit::new();
        c.phase = Phase::Travel { from: 0, progress: 0.0 };
        let start = rider_world_pos(&a, &c);
        let mut last_d = 0.0;
        for i in 1..=10 {
            c.phase = Phase::Travel { from: 0, progress: i as f32 / 10.0 };
            let d = rider_world_pos(&a, &c).distance(start);
            assert!(d >= last_d - 1e-4, "distance from start went backwards at step {i}");
            last_d = d;
        }
    }

    #[test]
    fn walking_a_bridge_moves_from_the_from_anchor_toward_the_to_anchor() {
        let a = anchors();
        let mut c = Circuit::new();
        c.phase = Phase::Travel { from: 1, progress: 0.5 };
        c.rider = Rider::Aboard;
        c.disembark(); // -> Walking { from: 1, ... }
        let at_jump = rider_world_pos(&a, &c);
        c.walk(1000.0); // walk far enough to be clamped near the far end
        let after = rider_world_pos(&a, &c);
        let to_anchor = a[2];
        assert!(
            after.distance(to_anchor) < at_jump.distance(to_anchor),
            "walking should move toward stop 2, not away from it"
        );
    }

    #[test]
    fn cold_reports_the_same_position_as_walking_at_the_same_remaining() {
        let a = anchors();
        let mut c = Circuit::new();
        c.phase = Phase::Travel { from: 0, progress: 0.3 };
        c.rider = Rider::Aboard;
        c.disembark();
        let walking_pos = rider_world_pos(&a, &c);
        c.go_cold();
        let cold_pos = rider_world_pos(&a, &c);
        assert_eq!(walking_pos, cold_pos);
    }

    #[test]
    fn bridge_local_to_world_places_the_start_waypoint_at_the_from_anchor() {
        let a = anchors();
        for b in moons::bridges() {
            let world = bridge_local_to_world(&a, &b, Vec3::ZERO);
            assert!(world.distance(a[b.from]) < 1e-3, "bridge {} start", b.from);
        }
    }

    #[test]
    fn bridge_local_to_world_places_the_end_waypoint_near_the_to_anchor() {
        let a = anchors();
        for b in moons::bridges() {
            let world = bridge_local_to_world(&a, &b, Vec3::new(b.length_m, 0.0, 0.0));
            let to = (b.from + 1) % STOP_COUNT;
            // "near", not exact: the bridge's own straight-line length
            // (WALK_LEG_RATIO * leg) does not equal the chord distance
            // between two approximated ring anchors — that mismatch is
            // exactly the approximation this module's doc comment names.
            assert!(world.distance(a[to]) < b.length_m * 0.5, "bridge {}->{} landed too far off", b.from, to);
        }
    }

    #[test]
    fn stop_local_to_world_is_a_pure_translation() {
        let a = anchors();
        let local = Vec3::new(3.0, 0.0, -2.0);
        assert_eq!(stop_local_to_world(&a, DOCK, local), a[DOCK] + local);
    }
}
