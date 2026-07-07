//! ragdoll — when a creature is knocked out or killed its parts go limp. This is
//! a small position-based-dynamics (Verlet) solver: point-mass **nodes** (the
//! part joints) joined by distance **sticks**, pulled by gravity, stopped by the
//! ground, and shoved apart where they overlap. Verlet's implicit velocity plus a
//! little damping means it loses energy and settles into a heap on its own. Pure
//! and deterministic; the bin builds nodes/sticks from a creature's anatomy when
//! it dies and renders the parts at the node positions.

/// Gravity (blocks/s²), matching the leap/airborne model.
pub const GRAVITY: f32 = 20.0;
/// Velocity retained per step (slight loss → the heap settles).
pub const DAMPING: f32 = 0.98;
/// Ground tangential friction applied when a node is resting (0 = ice, 1 = grip).
pub const FRICTION: f32 = 0.5;

/// A point mass. `prev` holds last frame's position; velocity is implicit in
/// `pos - prev`.
#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub pos: [f32; 3],
    pub prev: [f32; 3],
    /// Collision/rest radius (keeps the joint off the ground and out of others).
    pub radius: f32,
    /// A pinned node never moves (e.g. a held or hooked part).
    pub pinned: bool,
}

impl Node {
    pub fn new(pos: [f32; 3], radius: f32) -> Node {
        Node { pos, prev: pos, radius, pinned: false }
    }
    /// Give the node an initial velocity (blocks/s) by offsetting `prev`.
    pub fn with_velocity(mut self, v: [f32; 3], dt: f32) -> Node {
        self.prev = [self.pos[0] - v[0] * dt, self.pos[1] - v[1] * dt, self.pos[2] - v[2] * dt];
        self
    }
    pub fn speed(&self, dt: f32) -> f32 {
        let d = [self.pos[0] - self.prev[0], self.pos[1] - self.prev[1], self.pos[2] - self.prev[2]];
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() / dt.max(1e-4)
    }
}

/// A distance constraint between two nodes (a bone).
#[derive(Clone, Copy, Debug)]
pub struct Stick {
    pub a: usize,
    pub b: usize,
    pub rest: f32,
}

impl Stick {
    /// Build a stick at the nodes' current separation.
    pub fn between(nodes: &[Node], a: usize, b: usize) -> Stick {
        let (pa, pb) = (nodes[a].pos, nodes[b].pos);
        let rest = dist(pa, pb);
        Stick { a, b, rest }
    }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// Advance the ragdoll one frame: Verlet-integrate under gravity, then satisfy
/// the stick lengths, the ground floor, and node-node overlap for `iterations`
/// relaxation passes. `ground_y` is the floor the lowest point of each node rests
/// on (`pos.y >= ground_y + radius`).
pub fn step(nodes: &mut [Node], sticks: &[Stick], ground_y: f32, dt: f32, iterations: usize) {
    // --- Verlet integration ---
    let accel_y = -GRAVITY;
    for n in nodes.iter_mut() {
        if n.pinned {
            n.prev = n.pos;
            continue;
        }
        let vx = (n.pos[0] - n.prev[0]) * DAMPING;
        let vy = (n.pos[1] - n.prev[1]) * DAMPING;
        let vz = (n.pos[2] - n.prev[2]) * DAMPING;
        n.prev = n.pos;
        n.pos[0] += vx;
        n.pos[1] += vy + accel_y * dt * dt;
        n.pos[2] += vz;
    }

    // --- Relaxation: sticks, ground, self-collision ---
    for _ in 0..iterations.max(1) {
        // distance constraints
        for s in sticks {
            let (pa, pb) = (nodes[s.a].pos, nodes[s.b].pos);
            let d = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
            let diff = (len - s.rest) / len;
            let (ap, bp) = (nodes[s.a].pinned, nodes[s.b].pinned);
            let (wa, wb) = match (ap, bp) {
                (true, true) => (0.0, 0.0),
                (true, false) => (0.0, 1.0),
                (false, true) => (1.0, 0.0),
                (false, false) => (0.5, 0.5),
            };
            for k in 0..3 {
                let c = d[k] * diff;
                nodes[s.a].pos[k] += c * wa;
                nodes[s.b].pos[k] -= c * wb;
            }
        }
        // node-node overlap (self-collision)
        let n = nodes.len();
        for i in 0..n {
            for j in (i + 1)..n {
                let want = nodes[i].radius + nodes[j].radius;
                let (pi, pj) = (nodes[i].pos, nodes[j].pos);
                let d = [pj[0] - pi[0], pj[1] - pi[1], pj[2] - pi[2]];
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                if len < want && len > 1e-6 {
                    let push = (want - len) / len;
                    let (ip, jp) = (nodes[i].pinned, nodes[j].pinned);
                    let (wi, wj) = match (ip, jp) {
                        (true, true) => (0.0, 0.0),
                        (true, false) => (0.0, 1.0),
                        (false, true) => (1.0, 0.0),
                        (false, false) => (0.5, 0.5),
                    };
                    for k in 0..3 {
                        let c = d[k] * push;
                        nodes[i].pos[k] -= c * wi;
                        nodes[j].pos[k] += c * wj;
                    }
                }
            }
        }
        // ground floor (+ tangential friction when resting)
        for nd in nodes.iter_mut() {
            if nd.pinned {
                continue;
            }
            let floor = ground_y + nd.radius;
            if nd.pos[1] < floor {
                nd.pos[1] = floor;
                // friction: bleed horizontal velocity by moving prev toward pos
                nd.prev[0] += (nd.pos[0] - nd.prev[0]) * FRICTION;
                nd.prev[2] += (nd.pos[2] - nd.prev[2]) * FRICTION;
            }
        }
    }
}

/// Run `frames` of [`step`] (convenience for settling/tests).
pub fn settle(nodes: &mut [Node], sticks: &[Stick], ground_y: f32, dt: f32, iterations: usize, frames: usize) {
    for _ in 0..frames {
        step(nodes, sticks, ground_y, dt, iterations);
    }
}

// ── anatomy → ragdoll bridge ─────────────────────────────────────────────────
// Turns a creature's body + limbs into a node/stick skeleton the solver can
// collapse. Topology is a star: the body is the central node (index 0) and each
// limb hangs off it by one stick at its rest separation. This is the piece the
// bin calls on death/knockout ("build nodes/sticks from a creature's anatomy").

/// A joint node smaller than this would tunnel through the ground or jitter, so
/// limb/body radii are clamped up to it.
pub const MIN_NODE_RADIUS: f32 = 0.15;

/// Rotate a body-local offset (X=forward, Y=up, Z=right) into world space by
/// `yaw` about the Y axis — the exact convention `Creature::world_aabbs` uses,
/// duplicated here so this module stays std-only and independently testable.
fn rotate_yaw(off: [f32; 3], yaw: f32) -> [f32; 3] {
    let (c, s) = (yaw.cos(), yaw.sin());
    [off[0] * c - off[2] * s, off[1], off[0] * s + off[2] * c]
}

/// Build a ragdoll skeleton from raw limb geometry: `body_pos` + `yaw`, a
/// `body_radius`, and each limb as `(body_local_offset, half_extents)`. Every
/// node is seeded with `impulse` (blocks/s) so a creature that was moving — or
/// took a death blow — carries that momentum into the collapse. Returns
/// `(nodes, sticks)` ready to hand straight to [`step`]/[`settle`]. Pure and
/// deterministic; std-only (no `Creature` dependency) so it's unit-testable on
/// its own.
pub fn from_limbs(
    body_pos: [f32; 3],
    yaw: f32,
    body_radius: f32,
    limbs: &[([f32; 3], [f32; 3])],
    impulse: [f32; 3],
    dt: f32,
) -> (Vec<Node>, Vec<Stick>) {
    let mut nodes = Vec::with_capacity(limbs.len() + 1);
    nodes.push(Node::new(body_pos, body_radius.max(MIN_NODE_RADIUS)).with_velocity(impulse, dt));
    for (offset, half) in limbs {
        let w = rotate_yaw(*offset, yaw);
        let pos = [body_pos[0] + w[0], body_pos[1] + w[1], body_pos[2] + w[2]];
        let r = ((half[0] + half[1] + half[2]) / 3.0).max(MIN_NODE_RADIUS);
        nodes.push(Node::new(pos, r).with_velocity(impulse, dt));
    }
    // Star topology: every limb tied to the trunk (node 0) at its rest length.
    let mut sticks = Vec::with_capacity(limbs.len());
    for i in 1..nodes.len() {
        sticks.push(Stick::between(&nodes, 0, i));
    }
    (nodes, sticks)
}

/// Build a ragdoll from a live [`crate::creature::Creature`], plus an extra
/// `impulse` (blocks/s) to add on top of the body's own velocity — e.g. the
/// knockback direction of the killing blow. Call on death/knockout, then drive
/// the returned skeleton with [`step`] each frame and render each part at its
/// node position. The body's current velocity is included automatically so a
/// creature killed mid-run tumbles forward.
pub fn from_creature_with_impulse(
    c: &crate::creature::Creature,
    impulse: [f32; 3],
    dt: f32,
) -> (Vec<Node>, Vec<Stick>) {
    let he = c.body.half_extents;
    let body_radius = (he[0] + he[1] + he[2]) / 3.0;
    let v = c.body.velocity;
    let launch = [v[0] + impulse[0], v[1] + impulse[1], v[2] + impulse[2]];
    let limbs: Vec<([f32; 3], [f32; 3])> =
        c.limbs.iter().map(|l| (l.offset, l.half_extents)).collect();
    from_limbs(c.body.position, c.yaw, body_radius, &limbs, launch, dt)
}

/// Build a ragdoll from a creature with no extra impulse (just its own motion).
pub fn from_creature(c: &crate::creature::Creature, dt: f32) -> (Vec<Node>, Vec<Stick>) {
    from_creature_with_impulse(c, [0.0, 0.0, 0.0], dt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dropped_node_rests_on_the_ground() {
        let mut nodes = [Node::new([0.0, 10.0, 0.0], 0.3)];
        settle(&mut nodes, &[], 0.0, 1.0 / 60.0, 4, 600);
        assert!((nodes[0].pos[1] - 0.3).abs() < 0.05, "should rest at ground+radius, got {}", nodes[0].pos[1]);
        assert!(nodes[0].speed(1.0 / 60.0) < 0.05, "should come to rest");
    }

    #[test]
    fn a_node_never_sinks_below_ground() {
        let mut nodes = [Node::new([0.0, 0.1, 0.0], 0.3)];
        for _ in 0..300 {
            step(&mut nodes, &[], 0.0, 1.0 / 60.0, 4);
            assert!(nodes[0].pos[1] >= 0.3 - 1e-3, "sank to {}", nodes[0].pos[1]);
        }
    }

    #[test]
    fn a_stretched_stick_pulls_back_to_rest_length() {
        let mut nodes = [Node::new([0.0, 5.0, 0.0], 0.1), Node::new([1.0, 5.0, 0.0], 0.1)];
        let sticks = [Stick { a: 0, b: 1, rest: 1.0 }];
        // yank them apart, then relax (no gravity influence in one frame matters less)
        nodes[1].pos[0] = 3.0;
        nodes[1].prev[0] = 3.0;
        for _ in 0..50 {
            step(&mut nodes, &sticks, -100.0, 1.0 / 60.0, 8); // ground far below
        }
        let len = dist(nodes[0].pos, nodes[1].pos);
        assert!((len - 1.0).abs() < 0.05, "stick should restore rest length, got {len}");
    }

    #[test]
    fn a_pinned_node_does_not_move() {
        let mut a = Node::new([0.0, 5.0, 0.0], 0.2);
        a.pinned = true;
        let mut nodes = [a, Node::new([0.0, 4.0, 0.0], 0.2)];
        let sticks = [Stick { a: 0, b: 1, rest: 1.0 }];
        settle(&mut nodes, &sticks, -100.0, 1.0 / 60.0, 6, 100);
        assert_eq!(nodes[0].pos, [0.0, 5.0, 0.0], "pinned node is fixed");
        // the hanging node should dangle ~1 below the pin
        assert!((dist(nodes[0].pos, nodes[1].pos) - 1.0).abs() < 0.1);
    }

    #[test]
    fn a_dropped_chain_settles_into_a_heap_on_the_ground() {
        // four nodes in a vertical chain, dropped; should collapse and rest, all
        // above the floor, with low residual motion.
        let mut nodes = [
            Node::new([0.0, 5.0, 0.0], 0.3),
            Node::new([0.0, 5.6, 0.0], 0.3),
            Node::new([0.0, 6.2, 0.0], 0.3),
            Node::new([0.0, 6.8, 0.0], 0.3),
        ];
        let sticks = [
            Stick::between(&nodes, 0, 1),
            Stick::between(&nodes, 1, 2),
            Stick::between(&nodes, 2, 3),
        ];
        settle(&mut nodes, &sticks, 0.0, 1.0 / 60.0, 8, 1200);
        let dt = 1.0 / 60.0;
        for (i, n) in nodes.iter().enumerate() {
            assert!(n.pos[1] >= 0.3 - 1e-2, "node {i} above floor, y={}", n.pos[1]);
            assert!(n.speed(dt) < 0.1, "node {i} at rest, speed {}", n.speed(dt));
        }
        // self-collision kept them from all stacking at the exact same point
        let spread = dist(nodes[0].pos, nodes[3].pos);
        assert!(spread > 0.2, "the heap should have some spread, got {spread}");
    }

    #[test]
    fn overlapping_nodes_are_pushed_apart() {
        let mut nodes = [Node::new([0.0, 5.0, 0.0], 0.4), Node::new([0.1, 5.0, 0.0], 0.4)];
        step(&mut nodes, &[], -100.0, 1.0 / 60.0, 6);
        let sep = dist(nodes[0].pos, nodes[1].pos);
        assert!(sep > 0.6, "self-collision should separate them toward 0.8, got {sep}");
    }

    // ── from_limbs bridge ────────────────────────────────────────────────────

    #[test]
    fn body_only_makes_one_node_no_sticks() {
        let (nodes, sticks) = from_limbs([2.0, 3.0, 4.0], 0.0, 0.5, &[], [0.0; 3], 1.0 / 60.0);
        assert_eq!(nodes.len(), 1);
        assert!(sticks.is_empty());
        assert_eq!(nodes[0].pos, [2.0, 3.0, 4.0]);
        assert!((nodes[0].radius - 0.5).abs() < 1e-6);
    }

    #[test]
    fn tiny_parts_get_a_floor_radius() {
        // A tiny limb (and a tiny body) must be clamped to MIN_NODE_RADIUS.
        let (nodes, _) = from_limbs([0.0; 3], 0.0, 0.01, &[([1.0, 0.0, 0.0], [0.01, 0.01, 0.01])], [0.0; 3], 1.0 / 60.0);
        assert!(nodes[0].radius >= MIN_NODE_RADIUS - 1e-6);
        assert!(nodes[1].radius >= MIN_NODE_RADIUS - 1e-6);
    }

    #[test]
    fn each_limb_becomes_a_node_and_a_stick_to_the_trunk() {
        let limbs = [
            ([1.0, 0.0, 0.0], [0.3, 0.3, 0.3]),
            ([-1.0, 0.0, 0.0], [0.3, 0.3, 0.3]),
            ([0.0, 0.0, 1.0], [0.3, 0.3, 0.3]),
        ];
        let (nodes, sticks) = from_limbs([0.0; 3], 0.0, 0.5, &limbs, [0.0; 3], 1.0 / 60.0);
        assert_eq!(nodes.len(), 4, "trunk + 3 limbs");
        assert_eq!(sticks.len(), 3);
        // every stick ties a limb back to the trunk (node 0)
        for (i, s) in sticks.iter().enumerate() {
            assert_eq!(s.a, 0);
            assert_eq!(s.b, i + 1);
            assert!((s.rest - 1.0).abs() < 1e-5, "rest = current separation (1.0), got {}", s.rest);
        }
    }

    #[test]
    fn limb_offsets_respect_body_yaw() {
        // At yaw = +90°, a forward (+X) limb offset must land on +Z, matching
        // Creature::world_aabbs' convention.
        let limbs = [([1.0, 0.0, 0.0], [0.2, 0.2, 0.2])];
        let (nodes, _) = from_limbs([0.0; 3], std::f32::consts::FRAC_PI_2, 0.4, &limbs, [0.0; 3], 1.0 / 60.0);
        let p = nodes[1].pos;
        assert!(p[0].abs() < 1e-5 && (p[2] - 1.0).abs() < 1e-5, "forward limb should rotate to +Z, got {p:?}");
    }

    #[test]
    fn impulse_seeds_every_node_with_velocity() {
        let dt = 1.0 / 60.0;
        let limbs = [([1.0, 0.0, 0.0], [0.3, 0.3, 0.3])];
        let (nodes, _) = from_limbs([0.0; 3], 0.0, 0.5, &limbs, [5.0, 0.0, 0.0], dt);
        for n in &nodes {
            assert!((n.speed(dt) - 5.0).abs() < 1e-3, "node should carry the launch impulse, got {}", n.speed(dt));
        }
    }

    #[test]
    fn a_built_skeleton_settles_into_a_heap() {
        // End-to-end: build from limbs, drop it, everything comes to rest above
        // the floor without exploding.
        let limbs = [
            ([0.8, 0.5, 0.0], [0.3, 0.3, 0.3]),
            ([-0.8, 0.5, 0.0], [0.3, 0.3, 0.3]),
            ([0.0, 0.5, 0.8], [0.3, 0.3, 0.3]),
            ([0.0, 1.0, 0.0], [0.25, 0.25, 0.25]),
        ];
        let dt = 1.0 / 60.0;
        let (mut nodes, sticks) = from_limbs([0.0, 6.0, 0.0], 0.7, 0.5, &limbs, [2.0, 0.0, 0.0], dt);
        settle(&mut nodes, &sticks, 0.0, dt, 8, 1500);
        for (i, n) in nodes.iter().enumerate() {
            assert!(n.pos[1] >= MIN_NODE_RADIUS - 1e-2, "node {i} above floor, y={}", n.pos[1]);
            assert!(n.speed(dt) < 0.15, "node {i} at rest, speed {}", n.speed(dt));
            assert!(n.pos[1].is_finite() && n.pos[0].is_finite(), "node {i} stayed finite");
        }
    }
}

// ── service_registry wiring (S2) ───────────────────────────────────
// ragdoll exposes a free `step(nodes, sticks, ...)`; the service owns a
// minimal two-node, one-stick chain over flat ground and relaxes it each
// tick. Proves the solver wiring is panic-free.
use crate::sim::service_registry::{Service, ServiceCtx, ServiceEntry, SERVICES};
use linkme::distributed_slice;

struct RagdollService {
    nodes: Vec<Node>,
    sticks: Vec<Stick>,
}
impl Service for RagdollService {
    fn name(&self) -> &'static str {
        "ragdoll"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        step(&mut self.nodes, &self.sticks, 0.0, ctx.dt, 4);
    }
}
#[distributed_slice(SERVICES)]
static RAGDOLL_SERVICE: ServiceEntry = ServiceEntry {
    name: "ragdoll",
    make: || {
        let nodes = vec![Node::new([0.0, 2.0, 0.0], 0.25), Node::new([0.0, 1.0, 0.0], 0.25)];
        let sticks = vec![Stick::between(&nodes, 0, 1)];
        Box::new(RagdollService { nodes, sticks })
    },
};
