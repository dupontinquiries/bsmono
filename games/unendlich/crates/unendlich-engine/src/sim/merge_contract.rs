//! merge_contract — one place that pins the cross-session boundaries.
//!
//! This module exists so that a merge with the **shaders** session or the
//! **items + entities** session fails *loudly and locally* if it disturbs an
//! interface the fauna locomotion work depends on. The tests intentionally reach
//! across module boundaries (render vertex path, creature orientation, the pure
//! locomotion pipeline, the head/foot models) rather than testing one unit in
//! isolation. See `docs/SESSION_MERGE.md` for the full guide.
//!
//! Run: `cargo test -p unendlich-engine --no-default-features --lib
//! sim::merge_contract` (plus `sim::locomotion_sim`, the behavioural guard).

#[cfg(test)]
mod tests {
    use crate::creature::Creature;
    use crate::physics::PhysicsBody;
    use crate::sim::body_collision::resolve_overlap;
    use crate::sim::collision_probe::GroundProbe;
    use crate::sim::fall::{sighted_step, SAFE_FALL_BLOCKS, STEP_UP_BLOCKS};
    use crate::sim::grounding::{in_stance, Foot};
    use crate::sim::head_steer::{GazeLimits, Head};
    use crate::sim::leap::{Leap, GRAVITY};
    use crate::sim::stumble::{stumble_risk, trips};

    // ---- Render contract: creature orientation flows into the vertex path ----
    // If a merge drops Creature::pitch/roll or stops routing them through
    // push_box_oriented, the re-oriented creature will produce the SAME vertices
    // as the upright one and this fails.
    #[test]
    fn creature_orientation_changes_its_vertices() {
        let upright = Creature::new(PhysicsBody::new_player([0.0, 0.0, 0.0]));
        let (v0, i0) = crate::world::entity::creature_to_vertices(&upright);
        assert!(!v0.is_empty() && !i0.is_empty(), "a creature must mesh to some geometry");

        let mut leaned = Creature::new(PhysicsBody::new_player([0.0, 0.0, 0.0]));
        leaned.pitch = 0.5;
        leaned.roll = 0.3;
        let (v1, _) = crate::world::entity::creature_to_vertices(&leaned);
        assert_eq!(v0.len(), v1.len(), "same parts ⇒ same vertex count");
        // At least one vertex position must differ once it leans.
        let moved = v0.iter().zip(&v1).any(|(a, b)| a.position != b.position);
        assert!(moved, "pitch/roll must transform the geometry (render contract broken)");
    }

    #[test]
    fn yaw_alone_still_orients_geometry() {
        let mut a = Creature::new(PhysicsBody::new_player([0.0, 0.0, 0.0]));
        a.yaw = std::f32::consts::FRAC_PI_2;
        let (va, _) = crate::world::entity::creature_to_vertices(&a);
        let b = Creature::new(PhysicsBody::new_player([0.0, 0.0, 0.0]));
        let (vb, _) = crate::world::entity::creature_to_vertices(&b);
        assert!(va.iter().zip(&vb).any(|(x, y)| x.position != y.position), "yaw must orient");
    }

    // ---- Locomotion pipeline: the pure chain a Roamer tick depends on stays
    // callable with these signatures. A rename/move by any session breaks here. --
    #[test]
    fn locomotion_pipeline_composes() {
        let surf_block = |x: f32, _z: f32| if x < 5.0 { Some(64) } else { None };
        let surf_foot = |x: f32, z: f32| surf_block(x, z).map(|t| (t + 1) as f32);

        // ground probe → lean → roughness → leap target
        let probe = GroundProbe::sample(2.0, 2.0, 0.0, 1.0, 65.0, surf_foot);
        let (_pitch, _roll) = probe.lean_default();
        let _rough = probe.roughness();
        let _walk = probe.walkable_ahead(STEP_UP_BLOCKS, SAFE_FALL_BLOCKS);
        let leap = Leap::from_genes(1.0, 1.0);
        let _tgt = probe.leap_target(&leap, GRAVITY);

        // terrain-aware step
        let (_dx, _dz) = sighted_step((2.0, 2.0), 0.0, 0.2, 100, 0.0, surf_block);

        // stumble draw (deterministic)
        let risk = stumble_risk(6.0, 1.5, 0.3);
        let _ = trips(risk, 12345);

        // body collision split by mass
        let (_a, _b) = resolve_overlap((0.0, 0.0), 1.0, (0.4, 0.0), 1.0e6, 0.8);

        // head overlay + foot lock
        let mut head = Head::new();
        let lim = GazeLimits::default();
        head.track(0.0, 0.6, 0.05, &lim);
        let _turn = head.pull_body(0.05, &lim);
        let mut foot = Foot::new();
        let _p = foot.resolve(in_stance(0.1, 0.0), [1.0, 0.0]);
        // If this compiles and runs, the interface contract holds.
    }

    // ---- Anti-moonwalk invariant restated at the seam: a body-relative head
    // offset is just that — it never *is* the body heading. ----
    #[test]
    fn gaze_is_an_overlay_not_the_heading() {
        let lim = GazeLimits::default();
        let body_yaw = 0.3_f32;
        let mut head = Head::new();
        head.track(body_yaw, body_yaw + 1.0, 0.05, &lim); // look to the side
        assert!(head.offset > 0.0, "the head turned");
        // The world gaze = body + offset; locomotion must use body_yaw, which the
        // head never overwrites. A glance-sized offset leaves the body untouched.
        let mut glance = Head { offset: 0.03 };
        assert_eq!(glance.pull_body(0.05, &lim), 0.0, "a micro-look can't move the body");
    }
}
