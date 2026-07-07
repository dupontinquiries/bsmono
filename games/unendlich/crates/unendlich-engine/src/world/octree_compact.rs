//! `octree_compact` — leaf re-merge (merge-on-ascent) for the chunk octree.
//!
//! Worker-03 / WORLD-PERF pure core. The split-only `OctreeArray::set_block`
//! edit path never re-collapses a 2×2×2 subtree that has become uniform again
//! (e.g. after leaf decay → air), so an edited chunk's octree drifts larger
//! than a fresh `from_chunk` build. This routine reclaims that growth: it
//! collapses every uniform 8-leaf sibling group back into a single parent
//! leaf, bottom-up, returning a fresh tree containing only reachable nodes.
//!
//! It is the *how* of recompaction; `recompaction::decide` is the *when*.
//!
//! ## Minimal local model
//!
//! Self-contained so it compiles + tests standalone under `rustc --test`. The
//! engine's real `OctreeArray` is a `Vec<OctreeNode>` (packed header + child
//! indices) with the root fixed at index 0; this captures exactly the
//! structure the merge needs — leaves carry a block, internals carry 8 child
//! indices, root at index 0. The coordinator maps `Block` → `BlockId` and
//! `Node` → `OctreeNode` when wiring this in.
#![allow(dead_code)]

/// Block id in the minimal model (engine maps this to its `BlockId` enum).
pub type Block = u16;

/// A node is either a leaf carrying one block, or an internal node holding
/// 8 octant child indices into the enclosing `Octree::nodes` table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Node {
    Leaf(Block),
    Internal([u32; 8]),
}

/// One chunk's octree. Root is always at index 0 (engine convention).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Octree {
    pub nodes: Vec<Node>,
}

impl Octree {
    pub fn leaf(b: Block) -> Self {
        Octree { nodes: vec![Node::Leaf(b)] }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Point-query at chunk-local (x, y, z) for a tree of the given `depth`
    /// (edge = 2^depth). Descends from the root, stopping at the first leaf
    /// (which covers its whole subcube). Octant order is z-order:
    /// `o = dx | dy<<1 | dz<<2`.
    pub fn sample(&self, x: u32, y: u32, z: u32, depth: u32) -> Block {
        let mut idx = 0usize;
        let (mut ox, mut oy, mut oz) = (0u32, 0u32, 0u32);
        let mut size = 1u32 << depth;
        loop {
            match self.nodes[idx] {
                Node::Leaf(b) => return b,
                Node::Internal(ch) => {
                    let half = size >> 1;
                    let dx = (x - ox >= half) as usize;
                    let dy = (y - oy >= half) as usize;
                    let dz = (z - oz >= half) as usize;
                    let o = dx | (dy << 1) | (dz << 2);
                    if dx == 1 { ox += half; }
                    if dy == 1 { oy += half; }
                    if dz == 1 { oz += half; }
                    size = half;
                    idx = ch[o] as usize;
                }
            }
        }
    }
}

/// Collapse every uniform 2×2×2 subtree into a single parent leaf, bottom-up,
/// returning a fresh octree with only reachable nodes (root at index 0).
///
/// Guarantees:
/// * functionally identical — `sample` agrees everywhere with the input;
/// * minimal — no uniform 8-leaf group survives;
/// * `node_count` never increases;
/// * idempotent — `compact(compact(t))` behaves the same as `compact(t)`.
pub fn compact(src: &Octree) -> Octree {
    if src.nodes.is_empty() {
        return Octree { nodes: Vec::new() };
    }
    let mut out: Vec<Node> = Vec::new();
    match build(src, 0, &mut out) {
        Sub::Uniform(b) => Octree { nodes: vec![Node::Leaf(b)] },
        Sub::Tree(root) => {
            swap_root_to_zero(&mut out, root);
            Octree { nodes: out }
        }
    }
}

/// Result of compacting a subtree: either it collapses to a single uniform
/// block (nothing emitted), or it stays a real tree rooted at the returned
/// index in `out`.
#[derive(Clone, Copy)]
enum Sub {
    Uniform(Block),
    Tree(u32),
}

/// Post-order build: compact `src`'s subtree at `idx` into `out`. A subtree
/// that is uniform returns `Uniform(block)` and emits NOTHING — so when all 8
/// children of an internal node are uniform-and-equal we collapse with no dead
/// nodes left behind. A mixed node materializes each uniform child as one leaf
/// and emits the internal.
fn build(src: &Octree, idx: u32, out: &mut Vec<Node>) -> Sub {
    match src.nodes[idx as usize] {
        Node::Leaf(b) => Sub::Uniform(b),
        Node::Internal(children) => {
            let mut subs = [Sub::Uniform(0); 8];
            for i in 0..8 {
                subs[i] = build(src, children[i], out);
            }
            if let Sub::Uniform(b0) = subs[0] {
                let mut all_same = true;
                for s in subs.iter().skip(1) {
                    match s {
                        Sub::Uniform(b) if *b == b0 => {}
                        _ => {
                            all_same = false;
                            break;
                        }
                    }
                }
                if all_same {
                    return Sub::Uniform(b0);
                }
            }
            let mut ch = [0u32; 8];
            for i in 0..8 {
                ch[i] = match subs[i] {
                    Sub::Uniform(b) => push(out, Node::Leaf(b)),
                    Sub::Tree(t) => t,
                };
            }
            Sub::Tree(push(out, Node::Internal(ch)))
        }
    }
}

fn push(out: &mut Vec<Node>, n: Node) -> u32 {
    let i = out.len() as u32;
    out.push(n);
    i
}

/// Move the node at `root` to index 0 (engine root convention), patching any
/// child pointer that referenced 0 or `root` so the tree stays consistent.
fn swap_root_to_zero(out: &mut [Node], root: u32) {
    if root == 0 {
        return;
    }
    out.swap(0, root as usize);
    for n in out.iter_mut() {
        if let Node::Internal(ch) = n {
            for c in ch.iter_mut() {
                if *c == 0 {
                    *c = root;
                } else if *c == root {
                    *c = 0;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- test helpers (kept inside cfg(test) so they're not crate code) ----

    fn idx(x: u32, y: u32, z: u32, edge: u32) -> usize {
        (x + y * edge + z * edge * edge) as usize
    }

    // Build a FULLY-expanded tree (every internal subdivided to depth-0 leaves,
    // no collapse) from a dense cube — the split-only worst case compaction
    // must reclaim.
    fn build_full(
        cells: &[Block],
        edge: u32,
        ox: u32,
        oy: u32,
        oz: u32,
        size: u32,
        out: &mut Vec<Node>,
    ) -> u32 {
        if size == 1 {
            return push(out, Node::Leaf(cells[idx(ox, oy, oz, edge)]));
        }
        let half = size >> 1;
        let mut ch = [0u32; 8];
        for o in 0..8usize {
            let dx = (o & 1) as u32;
            let dy = ((o >> 1) & 1) as u32;
            let dz = ((o >> 2) & 1) as u32;
            ch[o] = build_full(cells, edge, ox + dx * half, oy + dy * half, oz + dz * half, half, out);
        }
        push(out, Node::Internal(ch))
    }

    fn full_octree(cells: &[Block], depth: u32) -> Octree {
        let edge = 1u32 << depth;
        let mut out = Vec::new();
        let root = build_full(cells, edge, 0, 0, 0, edge, &mut out);
        swap_root_to_zero(&mut out, root);
        Octree { nodes: out }
    }

    // Deterministic dense cube (no rng): small palette so collapses occur.
    fn gen_cube(depth: u32, seed: u32, palette: u16) -> Vec<Block> {
        let edge = 1u32 << depth;
        let n = (edge * edge * edge) as usize;
        let mut v = Vec::with_capacity(n);
        for i in 0..n as u32 {
            let h = i.wrapping_mul(2654435761).wrapping_add(seed.wrapping_mul(40503));
            v.push(((h >> 13) % palette as u32) as Block);
        }
        v
    }

    fn samples_match(a: &Octree, b: &Octree, depth: u32) -> bool {
        let edge = 1u32 << depth;
        for z in 0..edge {
            for y in 0..edge {
                for x in 0..edge {
                    if a.sample(x, y, z, depth) != b.sample(x, y, z, depth) {
                        return false;
                    }
                }
            }
        }
        true
    }

    // ---- unit tests ----

    #[test]
    fn eight_identical_leaves_collapse_to_one() {
        let t = full_octree(&[7u16; 8], 1); // 1 internal + 8 leaves = 9 nodes
        assert_eq!(t.node_count(), 9);
        let c = compact(&t);
        assert_eq!(c.node_count(), 1);
        assert_eq!(c.nodes[0], Node::Leaf(7));
    }

    #[test]
    fn one_differing_sibling_blocks_collapse() {
        let mut cells = [7u16; 8];
        cells[7] = 9; // one octant differs
        let t = full_octree(&cells, 1);
        let c = compact(&t);
        // Can't collapse the parent: stays 8 leaves + 1 internal.
        assert_eq!(c.node_count(), 9);
        assert!(samples_match(&t, &c, 1));
    }

    #[test]
    fn uniform_whole_cube_collapses_to_single_leaf() {
        let cells = vec![3u16; 64]; // depth 2 = 4³, all same
        let t = full_octree(&cells, 2);
        let c = compact(&t);
        assert_eq!(c.node_count(), 1);
        assert_eq!(c.nodes[0], Node::Leaf(3));
    }

    #[test]
    fn already_a_leaf_is_minimal() {
        let t = Octree::leaf(5);
        let c = compact(&t);
        assert_eq!(c.node_count(), 1);
        assert_eq!(c.nodes[0], Node::Leaf(5));
    }

    // ---- property tests (deterministic input sweeps) ----

    #[test]
    fn prop_compact_preserves_all_samples() {
        let depth = 2;
        for seed in 0..64u32 {
            for palette in [1u16, 2, 3, 4] {
                let cells = gen_cube(depth, seed, palette);
                let full = full_octree(&cells, depth);
                let c = compact(&full);
                assert!(
                    samples_match(&full, &c, depth),
                    "sample mismatch seed={seed} palette={palette}"
                );
            }
        }
    }

    #[test]
    fn prop_node_count_never_increases() {
        let depth = 2;
        for seed in 0..128u32 {
            for palette in [1u16, 2, 3, 8] {
                let full = full_octree(&gen_cube(depth, seed, palette), depth);
                let c = compact(&full);
                assert!(c.node_count() <= full.node_count());
            }
        }
    }

    #[test]
    fn prop_idempotent() {
        let depth = 2;
        for seed in 0..64u32 {
            for palette in [1u16, 2, 3] {
                let full = full_octree(&gen_cube(depth, seed, palette), depth);
                let once = compact(&full);
                let twice = compact(&once);
                // Running twice == once: same size and same content everywhere.
                assert_eq!(twice.node_count(), once.node_count());
                assert!(samples_match(&once, &twice, depth));
            }
        }
    }

    #[test]
    fn prop_fully_varied_cube_is_unchanged_size() {
        // All cells distinct → nothing can collapse → size equals the full tree.
        let depth = 2;
        let edge = 1u32 << depth;
        let cells: Vec<Block> = (0..(edge * edge * edge) as u16).collect();
        let full = full_octree(&cells, depth);
        let c = compact(&full);
        assert_eq!(c.node_count(), full.node_count());
        assert!(samples_match(&full, &c, depth));
    }
}
