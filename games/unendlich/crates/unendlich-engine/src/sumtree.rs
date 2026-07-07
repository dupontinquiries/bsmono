//! A summed octree — the data structure behind "what's the total humidity in
//! the voxels near me?". Each node stores the **sum** of a per-voxel scalar
//! over its cubic region, so a sum over any axis-aligned box is gathered by
//! descending only into nodes that straddle the query edge and reading whole
//! subtree totals everywhere else: roughly O(boundary), not O(volume).
//!
//! It also **compresses**: when all eight children of a node hold a single
//! uniform value, they collapse into one leaf. A field that's mostly flat
//! (dry air at humidity 0, or a slab of uniformly damp snow) costs a handful
//! of nodes instead of one per voxel. That's the property the snow model wants
//! — humidity is smooth and largely uniform, so the tree stays small while
//! still answering neighbourhood sums fast.
//!
//! Values are `u32` per voxel (quantize humidity into, say, 0..=1000 before
//! building); subtree sums are `u64`. Pure and unit-tested. Region queries use
//! half-open boxes `[min, max)`.

/// A node covers a cubic region whose side is a power of two.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    /// Every voxel in this region holds `value`; `sum = value * voxels`.
    Uniform { value: u32, sum: u64 },
    /// Mixed region; `sum` is the total over all eight children.
    Branch { sum: u64, children: Box<[Node; 8]> },
}

impl Node {
    fn sum(&self) -> u64 {
        match self {
            Node::Uniform { sum, .. } | Node::Branch { sum, .. } => *sum,
        }
    }
}

/// Child octant index for a half-offset `(dx,dy,dz)` each in `{0,1}`.
#[inline]
fn octant(dx: u32, dy: u32, dz: u32) -> usize {
    ((dx << 2) | (dy << 1) | dz) as usize
}

/// A summed octree over a `size³` cube (`size` a power of two).
#[derive(Debug, Clone)]
pub struct SumTree {
    size: u32,
    root: Node,
}

impl SumTree {
    pub fn size(&self) -> u32 {
        self.size
    }

    /// The grand total over the whole cube.
    pub fn total(&self) -> u64 {
        self.root.sum()
    }

    /// Build from a per-voxel value function. `size` is rounded up to the next
    /// power of two; coordinates outside the original extent simply read
    /// whatever `f` returns there (callers usually clamp/return 0).
    pub fn from_field(size: u32, f: impl Fn(u32, u32, u32) -> u32) -> SumTree {
        let size = size.max(1).next_power_of_two();
        let root = build(size, 0, 0, 0, &f);
        SumTree { size, root }
    }

    /// Value at a single voxel.
    pub fn point(&self, x: u32, y: u32, z: u32) -> u32 {
        let mut node = &self.root;
        let mut size = self.size;
        let (mut ox, mut oy, mut oz) = (0u32, 0u32, 0u32);
        loop {
            match node {
                Node::Uniform { value, .. } => return *value,
                Node::Branch { children, .. } => {
                    let h = size / 2;
                    let dx = (x >= ox + h) as u32;
                    let dy = (y >= oy + h) as u32;
                    let dz = (z >= oz + h) as u32;
                    node = &children[octant(dx, dy, dz)];
                    ox += dx * h;
                    oy += dy * h;
                    oz += dz * h;
                    size = h;
                }
            }
        }
    }

    /// Sum of values over the half-open box `[min, max)` (clamped to the cube).
    pub fn region_sum(&self, min: [u32; 3], max: [u32; 3]) -> u64 {
        let s = self.size;
        let qmin = [min[0].min(s), min[1].min(s), min[2].min(s)];
        let qmax = [max[0].min(s), max[1].min(s), max[2].min(s)];
        if (0..3).any(|i| qmin[i] >= qmax[i]) {
            return 0;
        }
        rsum(&self.root, [0, 0, 0], s, qmin, qmax)
    }

    /// Set a single voxel's value, repairing sums (and re-collapsing uniform
    /// subtrees) up the path. O(log size). Lets the humidity field evolve —
    /// snowfall adds, melt removes — without a full rebuild.
    pub fn set(&mut self, x: u32, y: u32, z: u32, value: u32) {
        if x >= self.size || y >= self.size || z >= self.size {
            return;
        }
        set_rec(&mut self.root, self.size, [0, 0, 0], [x, y, z], value);
    }

    /// Number of stored nodes — the compression metric. A uniform field is 1.
    pub fn node_count(&self) -> usize {
        count(&self.root)
    }
}

fn build(size: u32, ox: u32, oy: u32, oz: u32, f: &impl Fn(u32, u32, u32) -> u32) -> Node {
    if size == 1 {
        let v = f(ox, oy, oz);
        return Node::Uniform { value: v, sum: v as u64 };
    }
    let h = size / 2;
    let children: [Node; 8] = std::array::from_fn(|i| {
        let dx = ((i >> 2) & 1) as u32;
        let dy = ((i >> 1) & 1) as u32;
        let dz = (i & 1) as u32;
        build(h, ox + dx * h, oy + dy * h, oz + dz * h, f)
    });
    collapse(children)
}

/// Build a node from eight children, collapsing to a uniform leaf when they
/// all agree on a single value.
fn collapse(children: [Node; 8]) -> Node {
    let sum: u64 = children.iter().map(Node::sum).sum();
    if let Node::Uniform { value: v0, .. } = children[0] {
        if children.iter().all(|c| matches!(c, Node::Uniform { value, .. } if *value == v0)) {
            return Node::Uniform { value: v0, sum };
        }
    }
    Node::Branch { sum, children: Box::new(children) }
}

fn rsum(node: &Node, o: [u32; 3], size: u32, qmin: [u32; 3], qmax: [u32; 3]) -> u64 {
    let nmax = [o[0] + size, o[1] + size, o[2] + size];
    // Disjoint?
    if (0..3).any(|i| qmin[i] >= nmax[i] || qmax[i] <= o[i]) {
        return 0;
    }
    // Fully covered by the query → whole subtree sum.
    if (0..3).all(|i| qmin[i] <= o[i] && qmax[i] >= nmax[i]) {
        return node.sum();
    }
    match node {
        Node::Uniform { value, .. } => {
            // Partial overlap of a uniform region: value × overlap volume.
            let mut vox = 1u64;
            for i in 0..3 {
                let lo = qmin[i].max(o[i]);
                let hi = qmax[i].min(nmax[i]);
                vox *= (hi - lo) as u64;
            }
            *value as u64 * vox
        }
        Node::Branch { children, .. } => {
            let h = size / 2;
            let mut total = 0u64;
            for (i, child) in children.iter().enumerate() {
                let dx = ((i >> 2) & 1) as u32;
                let dy = ((i >> 1) & 1) as u32;
                let dz = (i & 1) as u32;
                let co = [o[0] + dx * h, o[1] + dy * h, o[2] + dz * h];
                total += rsum(child, co, h, qmin, qmax);
            }
            total
        }
    }
}

fn set_rec(node: &mut Node, size: u32, o: [u32; 3], p: [u32; 3], value: u32) {
    if size == 1 {
        *node = Node::Uniform { value, sum: value as u64 };
        return;
    }
    let h = size / 2;
    // If we're a uniform leaf and the write doesn't change us, stop.
    if let Node::Uniform { value: v, .. } = node {
        if *v == value {
            return;
        }
        // Split the uniform leaf into eight uniform children before descending.
        let v = *v;
        let child = Node::Uniform { value: v, sum: v as u64 * (h as u64).pow(3) };
        *node = Node::Branch {
            sum: node.sum(),
            children: Box::new(std::array::from_fn(|_| child.clone())),
        };
    }
    if let Node::Branch { children, .. } = node {
        let dx = (p[0] >= o[0] + h) as u32;
        let dy = (p[1] >= o[1] + h) as u32;
        let dz = (p[2] >= o[2] + h) as u32;
        let idx = octant(dx, dy, dz);
        let co = [o[0] + dx * h, o[1] + dy * h, o[2] + dz * h];
        set_rec(&mut children[idx], h, co, p, value);
        // Repair this node: recompute sum, re-collapse if now uniform.
        let kids = std::mem::replace(children, Box::new(std::array::from_fn(|_| Node::Uniform { value: 0, sum: 0 })));
        *node = collapse(*kids);
    }
}

fn count(node: &Node) -> usize {
    match node {
        Node::Uniform { .. } => 1,
        Node::Branch { children, .. } => 1 + children.iter().map(count).sum::<usize>(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Brute-force reference sum over a half-open box.
    fn brute(size: u32, f: &impl Fn(u32, u32, u32) -> u32, min: [u32; 3], max: [u32; 3]) -> u64 {
        let mut s = 0u64;
        for x in min[0]..max[0].min(size) {
            for y in min[1]..max[1].min(size) {
                for z in min[2]..max[2].min(size) {
                    s += f(x, y, z) as u64;
                }
            }
        }
        s
    }

    #[test]
    fn uniform_field_compresses_to_one_node() {
        let t = SumTree::from_field(16, |_, _, _| 5);
        assert_eq!(t.node_count(), 1, "uniform field is a single leaf");
        assert_eq!(t.total(), 5 * 16 * 16 * 16);
        assert_eq!(t.point(3, 7, 11), 5);
        // A sub-box sum is value × volume.
        assert_eq!(t.region_sum([0, 0, 0], [4, 4, 4]), 5 * 64);
    }

    #[test]
    fn single_nonzero_voxel() {
        let f = |x: u32, y: u32, z: u32| if (x, y, z) == (5, 6, 7) { 100 } else { 0 };
        let t = SumTree::from_field(16, f);
        assert_eq!(t.total(), 100);
        assert_eq!(t.point(5, 6, 7), 100);
        assert_eq!(t.point(0, 0, 0), 0);
        // A box containing it sees it; one beside it doesn't.
        assert_eq!(t.region_sum([4, 5, 6], [8, 8, 8]), 100);
        assert_eq!(t.region_sum([0, 0, 0], [5, 5, 5]), 0);
    }

    #[test]
    fn region_sum_matches_brute_force_on_varied_field() {
        // A non-trivial field with structure (so the tree branches).
        let f = |x: u32, y: u32, z: u32| ((x * 7 + y * 13 + z * 5) % 11) as u32;
        let t = SumTree::from_field(16, f);
        assert_eq!(t.total(), brute(16, &f, [0, 0, 0], [16, 16, 16]));
        // Several arbitrary boxes.
        for (mn, mx) in [
            ([0, 0, 0], [16, 16, 16]),
            ([2, 3, 1], [9, 11, 7]),
            ([5, 5, 5], [6, 6, 6]),
            ([10, 0, 4], [16, 8, 16]),
            ([1, 1, 1], [2, 16, 3]),
        ] {
            assert_eq!(
                t.region_sum(mn, mx),
                brute(16, &f, mn, mx),
                "box {mn:?}..{mx:?}"
            );
        }
    }

    #[test]
    fn set_updates_sums_and_recompresses() {
        let mut t = SumTree::from_field(8, |_, _, _| 0);
        assert_eq!(t.node_count(), 1);
        t.set(3, 4, 5, 42);
        assert_eq!(t.point(3, 4, 5), 42);
        assert_eq!(t.total(), 42);
        assert_eq!(t.region_sum([3, 4, 5], [4, 5, 6]), 42);
        assert!(t.node_count() > 1, "writing one voxel splits the tree");
        // Setting it back to 0 must re-collapse to a single uniform node.
        t.set(3, 4, 5, 0);
        assert_eq!(t.total(), 0);
        assert_eq!(t.node_count(), 1, "tree re-compresses when uniform again");
    }

    #[test]
    fn set_then_query_matches_a_fresh_build() {
        let mut t = SumTree::from_field(8, |_, _, _| 1);
        // Stamp a 2×2×2 block of 9s.
        for x in 2..4 {
            for y in 2..4 {
                for z in 2..4 {
                    t.set(x, y, z, 9);
                }
            }
        }
        let f = |x: u32, y: u32, z: u32| if (2..4).contains(&x) && (2..4).contains(&y) && (2..4).contains(&z) { 9 } else { 1 };
        let fresh = SumTree::from_field(8, f);
        assert_eq!(t.total(), fresh.total());
        assert_eq!(t.region_sum([0, 0, 0], [8, 8, 8]), fresh.total());
        assert_eq!(t.region_sum([2, 2, 2], [4, 4, 4]), 9 * 8);
    }
}
