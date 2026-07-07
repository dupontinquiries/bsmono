//! Bounded region graph — the guard against the runaway-graph failure mode: if
//! the simulation's region-dependency graph grows without bound it drowns the
//! engine and forces a fully hand-crafted deterministic fallback. This keeps it
//! bounded.
//!
//! Sim regions (chunks, brick clusters, fluid bodies, creature territories) are
//! nodes; an edge's weight is how strongly two regions **couple** — how much
//! processing one wrong without the other costs. We prune the dense coupling
//! graph to a **maximum-weight spanning forest** (Kruskal + union-find): keep the
//! strongest couplings that hold everything connected, drop the weak ones. The
//! result has at most `n - 1` edges no matter how many accumulated, gives a
//! deterministic backbone to walk, and is the structure the schedulers (and any
//! later learned predictors) traverse instead of an unbounded mesh.
//!
//! Pure, deterministic.

/// A weighted coupling between two regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub a: u32,
    pub b: u32,
    /// Coupling strength — higher means "process these together".
    pub weight: i64,
}

/// The pruned graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pruned {
    /// Kept edges — the strong-coupling backbone (≤ n-1).
    pub kept: Vec<Edge>,
    /// How many edges were dropped.
    pub dropped: usize,
}

/// Disjoint-set union-find with path compression + union by size.
struct Dsu {
    parent: Vec<u32>,
    size: Vec<u32>,
}
impl Dsu {
    fn new(n: u32) -> Dsu {
        Dsu { parent: (0..n).collect(), size: vec![1; n as usize] }
    }
    fn find(&mut self, x: u32) -> u32 {
        let mut r = x;
        while self.parent[r as usize] != r {
            r = self.parent[r as usize];
        }
        // Path compression.
        let mut c = x;
        while self.parent[c as usize] != r {
            let next = self.parent[c as usize];
            self.parent[c as usize] = r;
            c = next;
        }
        r
    }
    fn union(&mut self, x: u32, y: u32) -> bool {
        let (rx, ry) = (self.find(x), self.find(y));
        if rx == ry {
            return false;
        }
        let (big, small) = if self.size[rx as usize] >= self.size[ry as usize] { (rx, ry) } else { (ry, rx) };
        self.parent[small as usize] = big;
        self.size[big as usize] += self.size[small as usize];
        true
    }
}

/// Prune `edges` over `n` regions to a maximum-weight spanning forest: keep the
/// strongest edges that connect distinct components, drop the rest. At most
/// `n - 1` edges survive. Deterministic — ties broken by `(a, b)`.
pub fn prune_to_spanning_forest(n: u32, edges: &[Edge]) -> Pruned {
    let mut sorted: Vec<Edge> = edges.to_vec();
    // Strongest first; deterministic tie-break.
    sorted.sort_by(|e, f| f.weight.cmp(&e.weight).then(e.a.cmp(&f.a)).then(e.b.cmp(&f.b)));
    let mut dsu = Dsu::new(n);
    let mut kept = Vec::new();
    for e in sorted {
        if e.a < n && e.b < n && e.a != e.b && dsu.union(e.a, e.b) {
            kept.push(e);
        }
    }
    let dropped = edges.len() - kept.len();
    Pruned { kept, dropped }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(a: u32, b: u32, w: i64) -> Edge {
        Edge { a, b, weight: w }
    }

    #[test]
    fn prunes_a_dense_graph_to_a_tree() {
        // 4 fully-connected regions (6 edges) → a 3-edge tree.
        let edges = [e(0, 1, 5), e(0, 2, 4), e(0, 3, 1), e(1, 2, 3), e(1, 3, 2), e(2, 3, 6)];
        let p = prune_to_spanning_forest(4, &edges);
        assert_eq!(p.kept.len(), 3, "n-1 edges");
        assert_eq!(p.dropped, 3);
    }

    #[test]
    fn keeps_the_strongest_couplings() {
        // Triangle with weights 1, 2, 3: a max spanning tree keeps 3 and 2.
        let edges = [e(0, 1, 1), e(1, 2, 2), e(0, 2, 3)];
        let p = prune_to_spanning_forest(3, &edges);
        let weights: Vec<i64> = p.kept.iter().map(|e| e.weight).collect();
        assert!(weights.contains(&3) && weights.contains(&2), "kept strongest: {weights:?}");
        assert!(!weights.contains(&1), "dropped the weakest edge");
    }

    #[test]
    fn edge_count_is_always_bounded() {
        // Throw a flood of redundant edges at it; result stays <= n-1.
        let mut edges = Vec::new();
        for w in 0..500 {
            edges.push(e(w % 10, (w + 1) % 10, w as i64));
        }
        let p = prune_to_spanning_forest(10, &edges);
        assert!(p.kept.len() <= 9, "bounded to n-1 = 9, got {}", p.kept.len());
    }

    #[test]
    fn disconnected_regions_form_a_forest() {
        // Two separate pairs → a forest of 2 edges, not n-1=3.
        let edges = [e(0, 1, 5), e(2, 3, 5)];
        let p = prune_to_spanning_forest(4, &edges);
        assert_eq!(p.kept.len(), 2, "one edge per component");
    }

    #[test]
    fn deterministic() {
        let edges = [e(0, 1, 5), e(0, 2, 5), e(1, 2, 5)]; // all-equal weights → ties
        let a = prune_to_spanning_forest(3, &edges);
        let b = prune_to_spanning_forest(3, &edges);
        assert_eq!(a, b, "same input → same prune");
    }
}
