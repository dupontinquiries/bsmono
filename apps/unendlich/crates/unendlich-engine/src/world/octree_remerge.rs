//! `octree_remerge` — merge-on-ascent the split-only edit path skips.
//!
//! Worker-03 / WORLD pure core, round 7. Collapses any branch whose 8 children
//! are leaves of the same value back into a single leaf, bottom-up. This bounds
//! the node growth that incremental `set_block` edits leave behind, without a
//! full `from_chunk` rebuild — it complements `recompaction` (which decides
//! *when* to pay a full rebuild) by reclaiming cheaply *in place*.
//!
//! Uses a minimal owned (`Box`) octree so it compiles + tests standalone; the
//! coordinator maps `Node` onto the engine's flat `OctreeArray`. Pure &
//! deterministic; no clock/rng/unwrap.
#![allow(dead_code)]

/// A minimal owned octree node: a leaf value, or a branch of 8 children.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Leaf(u16),
    Branch([Box<Node>; 8]),
}

/// Collapse every uniform branch (8 same-valued leaves) into one leaf,
/// bottom-up. Value-preserving: every sampled cell is unchanged. Idempotent.
pub fn try_merge(n: Node) -> Node {
    match n {
        Node::Leaf(v) => Node::Leaf(v),
        Node::Branch(children) => {
            // Merge each child first (bottom-up).
            let merged: [Box<Node>; 8] = children.map(|c| Box::new(try_merge(*c)));
            // If all children are now leaves of the same value, collapse.
            if let Some(v0) = leaf_val(&merged[0]) {
                if merged.iter().all(|c| leaf_val(c) == Some(v0)) {
                    return Node::Leaf(v0);
                }
            }
            Node::Branch(merged)
        }
    }
}

/// Total node count (leaves + branches).
pub fn node_count(n: &Node) -> usize {
    match n {
        Node::Leaf(_) => 1,
        Node::Branch(ch) => 1 + ch.iter().map(|c| node_count(c)).sum::<usize>(),
    }
}

fn leaf_val(n: &Node) -> Option<u16> {
    match n {
        Node::Leaf(v) => Some(*v),
        Node::Branch(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(v: u16) -> Box<Node> {
        Box::new(Node::Leaf(v))
    }

    fn branch_of(vs: [u16; 8]) -> Node {
        Node::Branch([
            leaf(vs[0]),
            leaf(vs[1]),
            leaf(vs[2]),
            leaf(vs[3]),
            leaf(vs[4]),
            leaf(vs[5]),
            leaf(vs[6]),
            leaf(vs[7]),
        ])
    }

    // Build a fully-expanded tree of `depth` from a dense cube (edge 2^depth).
    fn build_full(cells: &[u16], depth: u32, ox: u32, oy: u32, oz: u32, size: u32, edge: u32) -> Node {
        if size == 1 {
            return Node::Leaf(cells[(ox + oy * edge + oz * edge * edge) as usize]);
        }
        let half = size / 2;
        let mk = |o: usize| {
            let dx = (o & 1) as u32;
            let dy = ((o >> 1) & 1) as u32;
            let dz = ((o >> 2) & 1) as u32;
            Box::new(build_full(cells, depth, ox + dx * half, oy + dy * half, oz + dz * half, half, edge))
        };
        Node::Branch([mk(0), mk(1), mk(2), mk(3), mk(4), mk(5), mk(6), mk(7)])
    }

    fn sample(n: &Node, x: u32, y: u32, z: u32, depth: u32) -> u16 {
        let mut cur = n;
        let mut d = depth;
        loop {
            match cur {
                Node::Leaf(v) => return *v,
                Node::Branch(ch) => {
                    d -= 1;
                    let bit = 1u32 << d;
                    let o = (((x & bit) != 0) as usize)
                        | ((((y & bit) != 0) as usize) << 1)
                        | ((((z & bit) != 0) as usize) << 2);
                    cur = &ch[o];
                }
            }
        }
    }

    fn samples_equal(a: &Node, b: &Node, depth: u32) -> bool {
        let edge = 1u32 << depth;
        for z in 0..edge {
            for y in 0..edge {
                for x in 0..edge {
                    if sample(a, x, y, z, depth) != sample(b, x, y, z, depth) {
                        return false;
                    }
                }
            }
        }
        true
    }

    #[test]
    fn eight_identical_leaves_collapse() {
        let t = branch_of([4, 4, 4, 4, 4, 4, 4, 4]);
        let m = try_merge(t);
        assert_eq!(m, Node::Leaf(4));
        assert_eq!(node_count(&m), 1);
    }

    #[test]
    fn mixed_leaves_unchanged() {
        let t = branch_of([4, 4, 4, 4, 4, 4, 4, 9]);
        let before = node_count(&t);
        let m = try_merge(t);
        assert!(matches!(m, Node::Branch(_)));
        assert_eq!(node_count(&m), before); // still 1 branch + 8 leaves = 9
    }

    #[test]
    fn value_preserving() {
        let depth = 2;
        let edge = 1u32 << depth;
        // varied cube with some uniform octants
        let cells: Vec<u16> = (0..(edge * edge * edge))
            .map(|i| ((i / 8) % 3) as u16)
            .collect();
        let full = build_full(&cells, depth, 0, 0, 0, edge, edge);
        let merged = try_merge(full.clone());
        assert!(samples_equal(&full, &merged, depth));
        assert!(node_count(&merged) <= node_count(&full));
    }

    #[test]
    fn idempotent() {
        let t = branch_of([1, 1, 2, 2, 1, 1, 2, 2]);
        let once = try_merge(t);
        let twice = try_merge(once.clone());
        assert_eq!(once, twice);
    }

    #[test]
    fn deeply_nested_uniform_collapses_fully() {
        // A depth-3 cube entirely of one value → single leaf.
        let depth = 3;
        let edge = 1u32 << depth;
        let cells = vec![7u16; (edge * edge * edge) as usize];
        let full = build_full(&cells, depth, 0, 0, 0, edge, edge);
        assert!(node_count(&full) > 1);
        let m = try_merge(full);
        assert_eq!(m, Node::Leaf(7));
        assert_eq!(node_count(&m), 1);
    }

    #[test]
    fn deterministic() {
        let t = branch_of([1, 1, 1, 1, 1, 1, 1, 1]);
        assert_eq!(try_merge(t.clone()), try_merge(t));
    }

    #[test]
    fn partial_nested_collapse() {
        // One octant is itself a uniform branch (should collapse to a leaf),
        // making the parent's 8 children all leaves — but with differing values,
        // so the parent does NOT collapse.
        let uniform_octant = Box::new(branch_of([5, 5, 5, 5, 5, 5, 5, 5]));
        let parent = Node::Branch([
            uniform_octant,
            leaf(1),
            leaf(2),
            leaf(3),
            leaf(4),
            leaf(6),
            leaf(7),
            leaf(8),
        ]);
        let m = try_merge(parent);
        // inner uniform branch (9 nodes) collapsed to 1 leaf → parent now 1+8=9
        assert_eq!(node_count(&m), 9);
        assert!(matches!(m, Node::Branch(_)));
    }
}
