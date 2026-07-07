//! lightning_path.rs — pure procedural lightning bolt geometry.
//!
//! Builds a branching bolt from a charged point as a flat segment tree: a main
//! trunk (a jittered random walk) plus a bounded number of forked branches.
//! Two variants: cloud-to-ground (the trunk descends until it reaches the
//! ground plane) and in-cloud (the walk stays within a horizontal cloud layer).
//! Fully deterministic for a given seed (a small internal PRNG — no clock, no
//! std RNG); R1/R11/R21 pure and panic-free.

/// One bolt segment from `a` to `b`. `parent` is the index of the segment this
/// one grows out of (`None` for the root), so the whole bolt is a tree.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Segment {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub parent: Option<usize>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Bolt {
    pub segments: Vec<Segment>,
}

/// Deterministic splitmix64-style PRNG. Seeded explicitly; never reads a clock.
struct Rng {
    state: u64,
}
impl Rng {
    fn new(seed: u64) -> Self {
        // avoid the all-zero fixed point
        Rng { state: seed ^ 0x9E37_79B9_7F4A_7C15 }
    }
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform f32 in [-1, 1).
    fn signed(&mut self) -> f32 {
        let u = (self.next_u64() >> 40) as f32 / ((1u64 << 24) as f32); // [0,1)
        u * 2.0 - 1.0
    }
    /// Uniform f32 in [0, 1).
    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / ((1u64 << 24) as f32)
    }
}

/// Tunables for bolt generation.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BoltParams {
    /// Length of one trunk step (world units).
    pub step: f32,
    /// Lateral jitter per step (world units).
    pub jitter: f32,
    /// Probability per trunk node of spawning a branch.
    pub branch_chance: f32,
    /// Hard cap on total segments (keeps geometry bounded).
    pub max_segments: usize,
    /// Max segments in any single branch.
    pub branch_len: usize,
}

impl Default for BoltParams {
    fn default() -> Self {
        BoltParams {
            step: 4.0,
            jitter: 3.0,
            branch_chance: 0.25,
            max_segments: 256,
            branch_len: 6,
        }
    }
}

fn push_branch(
    segs: &mut Vec<Segment>,
    rng: &mut Rng,
    p: &BoltParams,
    from_seg: usize,
    start: [f32; 3],
    dir_y: f32, // downward bias (negative) or ~0 for in-cloud
    layer: Option<(f32, f32)>, // clamp y to [lo,hi] for in-cloud branches
) {
    let mut parent = from_seg;
    let mut cur = start;
    let n = p.branch_len.min(p.max_segments.saturating_sub(segs.len()));
    for _ in 0..n {
        if segs.len() >= p.max_segments {
            break;
        }
        let mut next = [
            cur[0] + rng.signed() * p.jitter,
            cur[1] + dir_y * p.step * (0.5 + 0.5 * rng.unit()),
            cur[2] + rng.signed() * p.jitter,
        ];
        if let Some((lo, hi)) = layer {
            next[1] = next[1].clamp(lo, hi);
        }
        segs.push(Segment { a: cur, b: next, parent: Some(parent) });
        parent = segs.len() - 1;
        cur = next;
    }
}

/// Cloud-to-ground bolt: the trunk descends from `start` to `ground_y`, then a
/// bounded number of branches fork off. The final trunk node sits exactly on the
/// ground plane.
pub fn cloud_to_ground(start: [f32; 3], ground_y: f32, seed: u64, p: &BoltParams) -> Bolt {
    let mut rng = Rng::new(seed);
    let mut segs: Vec<Segment> = Vec::new();
    let step = p.step.max(1e-3);

    // trunk nodes from start.y down to ground_y
    let span = (start[1] - ground_y).max(0.0);
    let steps = ((span / step).ceil() as usize).max(1);
    let mut cur = start;
    let mut trunk_nodes: Vec<usize> = Vec::new(); // segment index ending at each node
    for s in 0..steps {
        let last = s + 1 == steps;
        let y = if last { ground_y } else { (start[1] - step * (s as f32 + 1.0)).max(ground_y) };
        let next = [cur[0] + rng.signed() * p.jitter, y, cur[2] + rng.signed() * p.jitter];
        let parent = if segs.is_empty() { None } else { Some(segs.len() - 1) };
        segs.push(Segment { a: cur, b: next, parent });
        trunk_nodes.push(segs.len() - 1);
        cur = next;
    }

    // branches fork from trunk nodes, descending, until the segment budget is hit
    for &t in &trunk_nodes {
        if segs.len() >= p.max_segments {
            break;
        }
        if rng.unit() < p.branch_chance {
            let from = segs[t].b;
            push_branch(&mut segs, &mut rng, p, t, from, -1.0, None);
        }
    }

    Bolt { segments: segs }
}

/// In-cloud bolt: the walk stays within the horizontal layer `[layer_y0,
/// layer_y1]` (no descent to ground). Useful for sheet-lightning flashes.
pub fn in_cloud(start: [f32; 3], layer_y0: f32, layer_y1: f32, seed: u64, p: &BoltParams) -> Bolt {
    let (lo, hi) = (layer_y0.min(layer_y1), layer_y0.max(layer_y1));
    let mut rng = Rng::new(seed ^ 0xA5A5_A5A5);
    let mut segs: Vec<Segment> = Vec::new();
    let step = p.step.max(1e-3);
    let steps = (p.max_segments / 4).max(4);

    let mut cur = [start[0], start[1].clamp(lo, hi), start[2]];
    let mut trunk_nodes: Vec<usize> = Vec::new();
    for _ in 0..steps {
        if segs.len() >= p.max_segments {
            break;
        }
        let mut next = [
            cur[0] + rng.signed() * step,
            (cur[1] + rng.signed() * p.jitter).clamp(lo, hi),
            cur[2] + rng.signed() * step,
        ];
        next[1] = next[1].clamp(lo, hi);
        let parent = if segs.is_empty() { None } else { Some(segs.len() - 1) };
        segs.push(Segment { a: cur, b: next, parent });
        trunk_nodes.push(segs.len() - 1);
        cur = next;
    }
    for &t in &trunk_nodes {
        if segs.len() >= p.max_segments {
            break;
        }
        if rng.unit() < p.branch_chance {
            let from = segs[t].b;
            push_branch(&mut segs, &mut rng, p, t, from, 0.0, Some((lo, hi)));
        }
    }

    Bolt { segments: segs }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connects_to_root(segs: &[Segment]) -> bool {
        for (i, s) in segs.iter().enumerate() {
            // walk parents up to the root; must terminate at a `None` and only
            // ever reference earlier segments (so it's an acyclic tree).
            let mut cur = s.parent;
            let mut guard = 0usize;
            while let Some(pi) = cur {
                if pi >= i || guard > segs.len() {
                    return false;
                }
                cur = segs[pi].parent;
                guard += 1;
            }
            let _ = i;
        }
        true
    }

    #[test]
    fn prop_every_segment_connects_to_trunk() {
        let p = BoltParams::default();
        for seed in 0..64u64 {
            let bolt = cloud_to_ground([0.0, 120.0, 0.0], 0.0, seed, &p);
            assert!(!bolt.segments.is_empty());
            assert!(connects_to_root(&bolt.segments), "ctg seed {} not a tree", seed);
            assert!(bolt.segments[0].parent.is_none(), "root has a parent");

            let sheet = in_cloud([0.0, 100.0, 0.0], 90.0, 110.0, seed, &p);
            assert!(connects_to_root(&sheet.segments), "in-cloud seed {} not a tree", seed);
        }
    }

    #[test]
    fn prop_ground_bolt_reaches_ground() {
        let p = BoltParams::default();
        for seed in 0..64u64 {
            let bolt = cloud_to_ground([5.0, 200.0, -3.0], 10.0, seed, &p);
            let lowest = bolt
                .segments
                .iter()
                .map(|s| s.a[1].min(s.b[1]))
                .fold(f32::INFINITY, f32::min);
            assert!(lowest <= 10.0 + 1e-4, "bolt didn't reach ground (seed {}): {}", seed, lowest);
        }
    }

    #[test]
    fn prop_branch_count_bounded() {
        let p = BoltParams::default();
        for seed in 0..128u64 {
            let bolt = cloud_to_ground([0.0, 300.0, 0.0], 0.0, seed, &p);
            assert!(bolt.segments.len() <= p.max_segments, "over budget seed {}", seed);
            assert!(!bolt.segments.is_empty());
        }
    }

    #[test]
    fn prop_in_cloud_stays_in_layer() {
        let p = BoltParams::default();
        let (lo, hi) = (90.0f32, 110.0f32);
        for seed in 0..64u64 {
            let bolt = in_cloud([0.0, 100.0, 0.0], lo, hi, seed, &p);
            for s in &bolt.segments {
                assert!(s.a[1] >= lo - 1e-4 && s.a[1] <= hi + 1e-4, "a.y out of layer");
                assert!(s.b[1] >= lo - 1e-4 && s.b[1] <= hi + 1e-4, "b.y out of layer");
            }
        }
    }

    #[test]
    fn prop_deterministic_per_seed() {
        let p = BoltParams::default();
        let a = cloud_to_ground([1.0, 150.0, 2.0], 0.0, 42, &p);
        let b = cloud_to_ground([1.0, 150.0, 2.0], 0.0, 42, &p);
        assert_eq!(a, b, "same seed should produce the same bolt");
        let c = cloud_to_ground([1.0, 150.0, 2.0], 0.0, 43, &p);
        assert!(a != c, "different seeds produced identical bolts");
    }
}
