//! Verlet soft-body for hanging vines and cloth banners. A [`Strand`] is a
//! chain of point masses: the top node is pinned to its anchor, gravity pulls
//! the chain down, air (from [`crate::air`]) pushes it sideways, and distance
//! constraints hold each segment near its rest length via Jakobsen relaxation.
//!
//! Why Verlet: it stores position + previous position instead of velocity, so
//! a constraint that simply *moves* a node also corrects its velocity for free,
//! and the integrator is unconditionally stable for stiff distance constraints.
//! That's exactly what a swaying vine wants — cheap, robust, no matrix solve.
//!
//! Render-side and visual only: a vine's sway is not load-bearing for the
//! simulation, so this is `f32` and lives outside the rollback state. The
//! glowing-cave-vine effect is this strand positioning the segments while
//! `material.rs` emission + `light.rs` flood-fill supply the glow.
//!
//! All deterministic; unit-tested for the pin, rest-length, wind response,
//! and stability (no energy blow-up).

#[inline]
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
#[inline]
fn len(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// One point mass in a strand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    pub pos: [f32; 3],
    /// Position last step — Verlet derives velocity from `pos - prev`.
    pub prev: [f32; 3],
    /// Pinned nodes are immovable anchors (the vine's attachment point).
    pub pinned: bool,
}

/// A chain of nodes connected by rest-length springs.
#[derive(Debug, Clone, PartialEq)]
pub struct Strand {
    pub nodes: Vec<Node>,
    /// Target length of each segment between adjacent nodes.
    pub rest_len: f32,
    /// Per-step velocity retention in `[0,1]` (1 = frictionless). Air drag.
    pub damping: f32,
}

impl Strand {
    /// A strand hanging straight down from `anchor`, with `segments` segments
    /// (so `segments + 1` nodes) each `rest_len` long. The top node is pinned.
    pub fn hanging(anchor: [f32; 3], segments: usize, rest_len: f32) -> Strand {
        let mut nodes = Vec::with_capacity(segments + 1);
        for i in 0..=segments {
            let p = [anchor[0], anchor[1] - rest_len * i as f32, anchor[2]];
            nodes.push(Node { pos: p, prev: p, pinned: i == 0 });
        }
        Strand { nodes, rest_len, damping: 0.98 }
    }

    /// Advance the strand by `dt` under `gravity` (an acceleration, e.g.
    /// `[0,-9.8,0]`) and a `wind` acceleration (from `air::air_velocity_at`
    /// scaled by how much the light cloth catches it). `iters` constraint
    /// relaxation passes — more = stiffer, less stretchy (3–5 is plenty).
    pub fn step(&mut self, dt: f32, gravity: [f32; 3], wind: [f32; 3], iters: usize) {
        let accel = add(gravity, wind);
        let dt2 = dt * dt;
        // Verlet integration: next = pos + (pos - prev)*damping + accel*dt².
        for n in self.nodes.iter_mut() {
            if n.pinned {
                n.prev = n.pos; // anchor never drifts
                continue;
            }
            let vel = scale(sub(n.pos, n.prev), self.damping);
            let next = add(add(n.pos, vel), scale(accel, dt2));
            n.prev = n.pos;
            n.pos = next;
        }
        // Distance constraints: pull each segment back toward rest length.
        // Pinned endpoints take none of the correction; a segment with one
        // pinned end moves only its free node by the full error.
        for _ in 0..iters {
            for i in 0..self.nodes.len().saturating_sub(1) {
                let (a, b) = (self.nodes[i], self.nodes[i + 1]);
                let delta = sub(b.pos, a.pos);
                let d = len(delta);
                if d <= 1e-6 {
                    continue;
                }
                let diff = (d - self.rest_len) / d; // fractional error
                let corr = scale(delta, 0.5 * diff);
                match (a.pinned, b.pinned) {
                    (true, true) => {}
                    (true, false) => {
                        self.nodes[i + 1].pos = sub(b.pos, scale(delta, diff));
                    }
                    (false, true) => {
                        self.nodes[i].pos = add(a.pos, scale(delta, diff));
                    }
                    (false, false) => {
                        self.nodes[i].pos = add(a.pos, corr);
                        self.nodes[i + 1].pos = sub(b.pos, corr);
                    }
                }
            }
        }
    }

    /// The free end of the strand (last node) — where a hanging light or the
    /// tip of a vine sits.
    pub fn tip(&self) -> [f32; 3] {
        self.nodes.last().map(|n| n.pos).unwrap_or([0.0; 3])
    }

    /// Total current length along the chain. Settles to `segments * rest_len`
    /// when relaxed.
    pub fn total_length(&self) -> f32 {
        self.nodes
            .windows(2)
            .map(|w| len(sub(w[1].pos, w[0].pos)))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(s: &mut Strand, steps: usize, wind: [f32; 3]) {
        for _ in 0..steps {
            s.step(1.0 / 60.0, [0.0, -9.8, 0.0], wind, 4);
        }
    }

    #[test]
    fn anchor_stays_pinned() {
        let anchor = [5.0, 20.0, -3.0];
        let mut s = Strand::hanging(anchor, 6, 1.0);
        settle(&mut s, 300, [8.0, 0.0, 0.0]); // strong sideways wind
        assert_eq!(s.nodes[0].pos, anchor, "pinned anchor must never move");
    }

    #[test]
    fn hangs_to_rest_length_under_gravity() {
        let mut s = Strand::hanging([0.0, 10.0, 0.0], 5, 1.0);
        settle(&mut s, 600, [0.0, 0.0, 0.0]);
        // A relaxed chain is ~taut: total length ≈ segments * rest_len.
        let target = 5.0 * 1.0;
        assert!((s.total_length() - target).abs() < 0.2,
            "chain should settle near rest length, got {}", s.total_length());
        // And it hangs essentially straight down (tip below anchor, x/z ~0).
        let tip = s.tip();
        assert!(tip[1] < 6.0, "tip hangs well below the anchor");
        assert!(tip[0].abs() < 0.2 && tip[2].abs() < 0.2, "hangs straight: {tip:?}");
    }

    #[test]
    fn wind_displaces_the_tip_downwind() {
        let mut calm = Strand::hanging([0.0, 10.0, 0.0], 6, 1.0);
        settle(&mut calm, 400, [0.0, 0.0, 0.0]);
        let calm_tip = calm.tip();

        let mut blown = Strand::hanging([0.0, 10.0, 0.0], 6, 1.0);
        settle(&mut blown, 400, [6.0, 0.0, 0.0]); // wind in +x
        let blown_tip = blown.tip();

        assert!(blown_tip[0] > calm_tip[0] + 0.5,
            "tip should swing downwind (+x): calm {} vs blown {}", calm_tip[0], blown_tip[0]);
        // Constraint keeps it from stretching unboundedly even in steady wind.
        assert!((blown.total_length() - 6.0).abs() < 0.5, "stays near rest length under wind");
    }

    #[test]
    fn stable_under_violent_gusts() {
        // Alternating hard gusts must not blow the sim up to NaN/inf.
        let mut s = Strand::hanging([0.0, 30.0, 0.0], 10, 1.0);
        for i in 0..1000 {
            let w = if i % 2 == 0 { [40.0, 5.0, -20.0] } else { [-35.0, -5.0, 25.0] };
            s.step(1.0 / 60.0, [0.0, -9.8, 0.0], w, 4);
        }
        for n in &s.nodes {
            assert!(n.pos.iter().all(|c| c.is_finite()), "no NaN/inf: {:?}", n.pos);
        }
        // Length stays bounded (no runaway stretch).
        assert!(s.total_length() < 10.0 * 1.0 * 1.6, "length bounded under chaos");
    }

    #[test]
    fn deterministic_replay() {
        let run = || {
            let mut s = Strand::hanging([1.0, 12.0, 2.0], 8, 0.75);
            for i in 0..200 {
                s.step(1.0 / 60.0, [0.0, -9.8, 0.0], [3.0 * (i as f32 * 0.05).sin(), 0.0, 1.0], 4);
            }
            s.tip()
        };
        assert_eq!(run(), run(), "same inputs → identical tip");
    }
}
