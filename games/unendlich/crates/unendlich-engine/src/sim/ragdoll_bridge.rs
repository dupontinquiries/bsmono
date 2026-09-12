//! ragdoll_bridge — the `Creature`-side of `sim::ragdoll`: build a skeleton
//! from a live creature, and read the settled nodes back as body-local
//! centres for `anim_rig`.
//!
//! `ragdoll::from_creature` built nodes from `Creature::limbs` — the
//! COLLISION boxes — while the renderer draws `Anatomy::parts`. So a dead
//! creature's rendered parts had no nodes to land on, which is why "render
//! parts at node positions" (BACKLOG A) never had a bridge to wire.
//! [`from_anatomy`] builds one node per anatomy part, in part order, so node
//! `i+1` is part `i` and the sampler can hand the rig a
//! [`RagdollSample`](crate::anim_rig::RagdollSample) with no index juggling.
//!
//! Split out of `ragdoll.rs` so the solver stays crate-free for the rig
//! harness (games/unendlich/harness/rig).

use super::ragdoll::{from_limbs, Node, Stick};
use crate::creature::{Anatomy, Creature};

/// Build a ragdoll from a live [`Creature`], plus an extra `impulse`
/// (blocks/s) to add on top of the body's own velocity — e.g. the knockback
/// direction of the killing blow. Nodes come from the creature's collision
/// `limbs`; prefer [`from_anatomy`] when the parts will be rendered.
pub fn from_creature_with_impulse(c: &Creature, impulse: [f32; 3], dt: f32) -> (Vec<Node>, Vec<Stick>) {
    let he = c.body.half_extents;
    let body_radius = (he[0] + he[1] + he[2]) / 3.0;
    let v = c.body.velocity;
    let launch = [v[0] + impulse[0], v[1] + impulse[1], v[2] + impulse[2]];
    let limbs: Vec<([f32; 3], [f32; 3])> = c.limbs.iter().map(|l| (l.offset, l.half_extents)).collect();
    from_limbs(c.body.position, c.yaw, body_radius, &limbs, launch, dt)
}

/// Build a ragdoll from a creature with no extra impulse (just its own motion).
pub fn from_creature(c: &Creature, dt: f32) -> (Vec<Node>, Vec<Stick>) {
    from_creature_with_impulse(c, [0.0, 0.0, 0.0], dt)
}

/// Build a ragdoll whose nodes are the creature's RENDERED parts: node 0 is
/// the torso, node `i+1` is `anatomy.parts[i]` (destroyed parts included, so
/// indices line up). The body's velocity plus `impulse` seeds every node, so
/// a creature killed mid-run tumbles forward. Call on death; then `step` it
/// each frame and hand [`sample_body_local`] to `anim_rig::pose`.
pub fn from_anatomy(c: &Creature, anatomy: &Anatomy, impulse: [f32; 3], dt: f32) -> (Vec<Node>, Vec<Stick>) {
    let he = c.body.half_extents;
    let body_radius = (he[0] + he[1] + he[2]) / 3.0;
    let v = c.body.velocity;
    let launch = [v[0] + impulse[0], v[1] + impulse[1], v[2] + impulse[2]];
    let parts: Vec<([f32; 3], [f32; 3])> = anatomy.parts.iter().map(|p| (p.offset, p.half_extents)).collect();
    from_limbs(c.body.position, c.yaw, body_radius, &parts, launch, dt)
}

/// Read the nodes back in the body-local frame the rig poses in: relative to
/// `body_pos` and rotated by `-yaw` (so a torso that has not moved samples to
/// the origin). Returns `(torso, parts)` with `parts[i]` for node `i+1`; a
/// node list shorter than expected yields fewer parts, never a panic.
pub fn sample_body_local(nodes: &[Node], body_pos: [f32; 3], yaw: f32) -> ([f32; 3], Vec<[f32; 3]>) {
    let (s, c) = yaw.sin_cos();
    let local = |p: [f32; 3]| {
        let dx = p[0] - body_pos[0];
        let dy = p[1] - body_pos[1];
        let dz = p[2] - body_pos[2];
        // inverse of ragdoll::rotate_yaw / push_box_oriented's yaw about +Y
        [dx * c + dz * s, dy, -dx * s + dz * c]
    };
    let torso = nodes.first().map(|n| local(n.pos)).unwrap_or([0.0; 3]);
    let parts = nodes.iter().skip(1).map(|n| local(n.pos)).collect();
    (torso, parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creature::{Part, PartState};
    use crate::physics::PhysicsBody;
    use crate::sim::ragdoll::{settle, MIN_NODE_RADIUS};

    fn quadruped() -> (Creature, Anatomy) {
        let mut c = Creature::new(PhysicsBody::new_player([10.0, 5.0, 20.0]));
        c.body.half_extents = [0.55, 0.32, 0.3];
        let a = Anatomy::new()
            .with(PartState::new(Part::Jaw, 12, [0.6, 0.1, 0.0], [0.16, 0.12, 0.16]))
            .with(PartState::new(Part::Foot, 10, [0.45, -0.5, 0.25], [0.12, 0.3, 0.12]))
            .with(PartState::new(Part::Foot, 10, [0.45, -0.5, -0.25], [0.12, 0.3, 0.12]))
            .with(PartState::new(Part::Foot, 10, [-0.45, -0.5, 0.25], [0.12, 0.3, 0.12]))
            .with(PartState::new(Part::Foot, 10, [-0.45, -0.5, -0.25], [0.12, 0.3, 0.12]))
            .with(PartState::new(Part::Tail, 12, [-0.7, 0.1, 0.0], [0.4, 0.12, 0.12]));
        (c, a)
    }

    #[test]
    fn from_anatomy_makes_one_node_per_part_in_order_plus_the_torso() {
        let (c, a) = quadruped();
        let (nodes, sticks) = from_anatomy(&c, &a, [0.0; 3], 1.0 / 60.0);
        assert_eq!(nodes.len(), a.parts.len() + 1);
        assert_eq!(sticks.len(), a.parts.len(), "star: one stick per part to the torso");
        assert_eq!(nodes[0].pos, c.body.position);
        for (i, p) in a.parts.iter().enumerate() {
            let n = nodes[i + 1].pos;
            // yaw 0: node = body + offset
            for k in 0..3 {
                assert!((n[k] - (c.body.position[k] + p.offset[k])).abs() < 1e-5, "part {i} axis {k}");
            }
            assert!(nodes[i + 1].radius >= MIN_NODE_RADIUS - 1e-6);
        }
    }

    #[test]
    fn sample_inverts_the_yaw_so_a_fresh_ragdoll_samples_to_the_rest_offsets() {
        let (mut c, a) = quadruped();
        c.yaw = 1.1;
        let (nodes, _) = from_anatomy(&c, &a, [0.0; 3], 1.0 / 60.0);
        let (torso, parts) = sample_body_local(&nodes, c.body.position, c.yaw);
        assert!(torso.iter().all(|v| v.abs() < 1e-5), "unmoved torso samples to origin: {torso:?}");
        assert_eq!(parts.len(), a.parts.len());
        for (i, p) in a.parts.iter().enumerate() {
            for k in 0..3 {
                assert!((parts[i][k] - p.offset[k]).abs() < 1e-4, "part {i} axis {k}: {:?} vs {:?}", parts[i], p.offset);
            }
        }
    }

    #[test]
    fn a_killed_creature_settles_below_its_standing_height_and_the_sample_follows() {
        let (mut c, a) = quadruped();
        c.body.velocity = [3.0, 0.0, 0.0];
        let (mut nodes, sticks) = from_anatomy(&c, &a, [1.0, 2.0, 0.0], 1.0 / 60.0);
        // ground under the creature's feet: sole = body.y - 0.8
        let ground = c.body.position[1] - 0.8;
        settle(&mut nodes, &sticks, ground, 1.0 / 60.0, 4, 600);
        let (torso, parts) = sample_body_local(&nodes, c.body.position, c.yaw);
        assert!(torso[1] < -0.2, "the torso collapses toward the ground: {torso:?}");
        assert!(torso[0] > 0.3, "and carries its momentum forward: {torso:?}");
        // every node rests on or above the ground
        for (i, n) in nodes.iter().enumerate() {
            assert!(n.pos[1] >= ground + MIN_NODE_RADIUS - 1e-2, "node {i} below ground: {}", n.pos[1]);
        }
        // the sample keeps part order and count
        assert_eq!(parts.len(), a.parts.len());
    }

    #[test]
    fn from_creature_uses_limbs_and_an_empty_list_is_a_lone_torso() {
        let (c, _) = quadruped();
        let (nodes, sticks) = from_creature(&c, 1.0 / 60.0);
        assert_eq!(nodes.len(), 1 + c.limbs.len());
        assert_eq!(sticks.len(), c.limbs.len());
        let (torso, parts) = sample_body_local(&[], [0.0; 3], 0.0);
        assert_eq!(torso, [0.0; 3]);
        assert!(parts.is_empty());
    }
}
