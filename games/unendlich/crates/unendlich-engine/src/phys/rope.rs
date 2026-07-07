//! phys::rope — Verlet rope/chain physics (r200). A rope is a list of
//! point masses (nodes) linked by distance constraints; gravity + a few
//! constraint-relaxation passes per step give convincing swing, sag, and
//! taut behavior. Models the general "mass-spring chain" a physics sim
//! uses for ropes, vines, chains, and bridges.
//!
//! Deterministic (pure f32 arithmetic, fixed iteration order) so it is
//! replay/rollback-safe, and std-only. Anchors are nodes flagged pinned;
//! everything else falls and is pulled back onto the constraints.

use crate::util::vecmath::{len3, V3};

/// One rope node. `prev` holds last-step position — Verlet integrates
/// velocity implicitly as (pos - prev), so there is no separate velocity
/// field to desync.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    pub pos: V3,
    pub prev: V3,
    pub pinned: bool,
}

impl Node {
    pub fn free(pos: V3) -> Node {
        Node { pos, prev: pos, pinned: false }
    }
    pub fn pinned(pos: V3) -> Node {
        Node { pos, prev: pos, pinned: true }
    }
}

/// A rope: nodes + the rest length between adjacent nodes.
#[derive(Debug, Clone)]
pub struct Rope {
    pub nodes: Vec<Node>,
    pub segment_len: f32,
    /// Constraint-relaxation passes per step. More = stiffer/less stretch.
    pub iterations: u32,
    /// Per-step velocity retention (1.0 = frictionless). (pos-prev)*damping.
    pub damping: f32,
}

impl Rope {
    /// Build a straight rope of `count` nodes hanging from `anchor`,
    /// spaced `segment_len` apart along -Y (so it hangs down then settles).
    pub fn hanging(anchor: V3, count: usize, segment_len: f32) -> Rope {
        let mut nodes = Vec::with_capacity(count);
        for i in 0..count {
            let p = [anchor[0], anchor[1] - segment_len * i as f32, anchor[2]];
            nodes.push(if i == 0 { Node::pinned(p) } else { Node::free(p) });
        }
        Rope { nodes, segment_len, iterations: 16, damping: 0.99 }
    }

    /// Advance one step. `gravity` is acceleration (e.g. [0,-20,0]); `dt`
    /// the timestep. Verlet integrate, then relax distance constraints.
    pub fn step(&mut self, gravity: V3, dt: f32) {
        let dt2 = dt * dt;
        // 1. Verlet integration: each free node moves by its implicit
        //    velocity (pos - prev) * damping plus gravity * dt^2.
        for n in self.nodes.iter_mut() {
            if n.pinned {
                continue;
            }
            let vel = [
                (n.pos[0] - n.prev[0]) * self.damping,
                (n.pos[1] - n.prev[1]) * self.damping,
                (n.pos[2] - n.prev[2]) * self.damping,
            ];
            n.prev = n.pos;
            n.pos = [
                n.pos[0] + vel[0] + gravity[0] * dt2,
                n.pos[1] + vel[1] + gravity[1] * dt2,
                n.pos[2] + vel[2] + gravity[2] * dt2,
            ];
        }
        // 2. Satisfy distance constraints between adjacent nodes. Several
        //    passes converge toward the rest length. Fixed forward order
        //    keeps it deterministic.
        for _ in 0..self.iterations {
            for i in 0..self.nodes.len().saturating_sub(1) {
                self.relax_pair(i, i + 1);
            }
        }
    }

    fn relax_pair(&mut self, i: usize, j: usize) {
        let a = self.nodes[i];
        let b = self.nodes[j];
        let delta = [b.pos[0] - a.pos[0], b.pos[1] - a.pos[1], b.pos[2] - a.pos[2]];
        let dist = len3(delta);
        if dist <= 1e-6 {
            return;
        }
        // How far off the rest length, as a fraction of current distance.
        let diff = (dist - self.segment_len) / dist;
        // Pinned endpoints take none of the correction; free ones split it.
        let (wa, wb) = match (a.pinned, b.pinned) {
            (true, true) => (0.0, 0.0),
            (true, false) => (0.0, 1.0),
            (false, true) => (1.0, 0.0),
            (false, false) => (0.5, 0.5),
        };
        let corr = [delta[0] * diff, delta[1] * diff, delta[2] * diff];
        if !a.pinned {
            self.nodes[i].pos = [
                a.pos[0] + corr[0] * wa,
                a.pos[1] + corr[1] * wa,
                a.pos[2] + corr[2] * wa,
            ];
        }
        if !b.pinned {
            self.nodes[j].pos = [
                b.pos[0] - corr[0] * wb,
                b.pos[1] - corr[1] * wb,
                b.pos[2] - corr[2] * wb,
            ];
        }
    }

    /// Total stretch error = sum over segments of |dist - segment_len|.
    /// Near zero after enough iterations on a settled rope.
    pub fn stretch_error(&self) -> f32 {
        let mut e = 0.0;
        for i in 0..self.nodes.len().saturating_sub(1) {
            let a = self.nodes[i].pos;
            let b = self.nodes[i + 1].pos;
            let d = len3([b[0] - a[0], b[1] - a[1], b[2] - a[2]]);
            e += (d - self.segment_len).abs();
        }
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_stays_pinned() {
        let mut r = Rope::hanging([0.0, 10.0, 0.0], 6, 1.0);
        let anchor0 = r.nodes[0].pos;
        for _ in 0..120 {
            r.step([0.0, -20.0, 0.0], 1.0 / 60.0);
        }
        assert_eq!(r.nodes[0].pos, anchor0, "pinned anchor must not move");
    }

    #[test]
    fn settles_taut_under_gravity() {
        // A hanging rope should converge so segment lengths ~= rest length
        // (low stretch error) and hang essentially straight down.
        let mut r = Rope::hanging([0.0, 10.0, 0.0], 8, 1.0);
        for _ in 0..600 {
            r.step([0.0, -20.0, 0.0], 1.0 / 60.0);
        }
        assert!(r.stretch_error() < 0.05, "stretch error {}", r.stretch_error());
        // bottom node hangs ~7 below anchor (7 segments of length 1).
        let drop = r.nodes[0].pos[1] - r.nodes[7].pos[1];
        assert!((drop - 7.0).abs() < 0.2, "drop {drop}");
        // and stays near the anchor's x/z (hangs straight).
        assert!(r.nodes[7].pos[0].abs() < 0.1 && r.nodes[7].pos[2].abs() < 0.1);
    }

    #[test]
    fn deterministic_replay() {
        let run = || {
            let mut r = Rope::hanging([0.0, 5.0, 0.0], 5, 0.8);
            // shove a node sideways to make motion nontrivial
            r.nodes[2].pos[0] += 0.5;
            for _ in 0..50 {
                r.step([0.0, -20.0, 0.0], 1.0 / 60.0);
            }
            r.nodes[4].pos
        };
        let a = run();
        let b = run();
        assert_eq!(a[0].to_bits(), b[0].to_bits());
        assert_eq!(a[1].to_bits(), b[1].to_bits());
        assert_eq!(a[2].to_bits(), b[2].to_bits());
    }
}
