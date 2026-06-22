//! context_graph.rs — reverse-anchored traversal graph for sideband context retrieval.
//!
//! `insert(context, endpoint)` stores the endpoint as the anchor, then the context tokens walking
//! BACKWARD from it — so contexts that share the tokens nearest the endpoint merge into one path.
//! `minimal_context` walks back from an endpoint until the path is unique (hits == 1): the fewest
//! tokens of context that still determine what follows. The model never runs backward; this is a
//! forward-fed retrieval index that hands it the least context that pins the continuation, which is
//! the lever for keeping working-context small on huge workloads. Units are u32 = tokens (practical)
//! or chars (finer, bigger). Literal/exact matching — the semantic layer is the embedding sideband.
#![allow(dead_code)]
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap};

struct Node {
    edges: BTreeMap<u32, usize>, // backward token -> child node
    hits: u32,                   // inserted contexts passing through (branching/frequency)
}

#[derive(Default)]
pub struct ContextGraph {
    nodes: Vec<Node>, // node 0 = root anchor
}

impl ContextGraph {
    pub fn new() -> Self { Self { nodes: vec![Node { edges: BTreeMap::new(), hits: 0 }] } }
    pub fn node_count(&self) -> usize { self.nodes.len() }

    fn step(&mut self, from: usize, tok: u32) -> usize {
        if let Some(&n) = self.nodes[from].edges.get(&tok) { return n; }
        let n = self.nodes.len();
        self.nodes.push(Node { edges: BTreeMap::new(), hits: 0 });
        self.nodes[from].edges.insert(tok, n);
        n
    }

    /// Anchor `endpoint`, then insert `context` reversed (nearest-endpoint token first).
    pub fn insert(&mut self, context: &[u32], endpoint: u32) {
        let mut cur = self.step(0, endpoint);
        self.nodes[cur].hits += 1;
        for &t in context.iter().rev() {
            cur = self.step(cur, t);
            self.nodes[cur].hits += 1;
        }
    }

    /// Backward tokens (nearest-endpoint first) needed before the path from `endpoint` is unique —
    /// the minimal context that determines the continuation. 0 if the endpoint alone is unique;
    /// context.len() if it never uniquifies (identical contexts were inserted more than once).
    pub fn minimal_context(&self, context: &[u32], endpoint: u32) -> usize {
        let mut cur = match self.nodes[0].edges.get(&endpoint) { Some(&n) => n, None => return 0 };
        if self.nodes[cur].hits <= 1 { return 0; }
        let mut depth = 0;
        for &t in context.iter().rev() {
            match self.nodes[cur].edges.get(&t) {
                Some(&n) => { cur = n; depth += 1; if self.nodes[cur].hits <= 1 { return depth; } }
                None => return depth,
            }
        }
        depth
    }

    /// How many distinct contexts lead to `endpoint` (its anchor's out-degree of backward tokens
    /// summed by frequency = ambiguity at the endpoint).
    pub fn endpoint_load(&self, endpoint: u32) -> u32 {
        self.nodes[0].edges.get(&endpoint).map(|&n| self.nodes[n].hits).unwrap_or(0)
    }
}

/// Self-test (no deps): backward-suffix sharing, minimal-context retrieval, and the compression it buys.
pub fn gate_context_graph() {
    let mut g = ContextGraph::new();
    g.insert(&[8, 8, 3], 99); // A: context ...8 8 3 -> 99
    g.insert(&[8, 8, 7], 99); // B: diverges from A at the token nearest 99 (3 vs 7)
    g.insert(&[8, 8, 3], 99); // dup of A (identical context)
    g.insert(&[8, 8, 3], 42); // same context, different endpoint

    let m_b = g.minimal_context(&[8, 8, 7], 99); // unique at the first backward token -> 1
    let m_a = g.minimal_context(&[8, 8, 3], 99); // shares with its dup, never uniquifies -> len 3
    let m_42 = g.minimal_context(&[8, 8, 3], 42); // only context for 42 -> endpoint alone -> 0

    let naive = 1 + 4 * (1 + 3); // root + (anchor + 3 ctx) per insert with zero sharing = 17
    let shared = g.node_count() < naive; // trie merged shared backward suffixes (A==dup, shared 99 anchor)

    for (label, ok) in [
        ("B uniquified by 1 nearest token", m_b == 1),
        ("identical contexts need full length", m_a == 3),
        ("lone-context endpoint needs 0", m_42 == 0),
        ("backward suffixes merged (nodes<naive)", shared),
        ("endpoint 99 carries 3 contexts", g.endpoint_load(99) == 3),
    ] {
        println!("  [{}] {}", if ok { "\u{2713}" } else { "\u{2717} FAIL" }, label);
    }
    let all = m_b == 1 && m_a == 3 && m_42 == 0 && shared && g.endpoint_load(99) == 3;
    println!("gate-context-graph: endpoint 99 / ctx B needs {m_b} of 3 tokens (3x context cut) | nodes {} < naive {naive} | {}",
        g.node_count(), if all { "checks \u{2713}" } else { "FAIL \u{2717}" });
}

// ───────────────── associative bridge layer (small-world fragment linking) ─────────────────
// The literal index doesn't connect arbitrary fragments. This layer does: endpoints that share
// immediate predecessors are associated (they're produced from overlapping contexts), and `bridge`
// finds a richest path between two endpoints that AREN'T directly associated — the "if they're not
// connected, route through the graph" idea. max_hops + min_weight come from model capability: a
// stronger model needs fewer, stronger hops; a weaker one tolerates a longer, weaker chain.
impl ContextGraph {
    /// endpoint -> [(endpoint, weight)] where weight = count of shared immediate-predecessor tokens.
    pub fn associations(&self) -> HashMap<u32, Vec<(u32, f32)>> {
        let preds: HashMap<u32, BTreeSet<u32>> = self.nodes[0].edges.iter()
            .map(|(&ep, &node)| (ep, self.nodes[node].edges.keys().copied().collect()))
            .collect();
        let eps: Vec<u32> = preds.keys().copied().collect();
        let mut adj: HashMap<u32, Vec<(u32, f32)>> = HashMap::new();
        for i in 0..eps.len() {
            for j in 0..eps.len() {
                if i == j { continue; }
                let shared = preds[&eps[i]].intersection(&preds[&eps[j]]).count();
                if shared > 0 { adj.entry(eps[i]).or_default().push((eps[j], shared as f32)); }
            }
        }
        adj
    }
}

/// Richest path from `a` to `b` of at most `max_hops` edges — "richest" = the path whose weakest link
/// is strongest (widest-path / bottleneck), ignoring edges below `min_weight`. None if unreachable
/// within the bound. Bottleneck-greedy with a hop cap; good enough for retrieval routing.
pub fn bridge(adj: &HashMap<u32, Vec<(u32, f32)>>, a: u32, b: u32, max_hops: usize, min_weight: f32) -> Option<Vec<u32>> {
    struct S { bottleneck: f32, node: u32, hops: usize, path: Vec<u32> }
    impl PartialEq for S { fn eq(&self, o: &Self) -> bool { self.bottleneck == o.bottleneck } }
    impl Eq for S {}
    impl Ord for S { fn cmp(&self, o: &Self) -> std::cmp::Ordering { self.bottleneck.partial_cmp(&o.bottleneck).unwrap_or(std::cmp::Ordering::Equal) } }
    impl PartialOrd for S { fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> { Some(self.cmp(o)) } }
    let mut heap = BinaryHeap::new();
    heap.push(S { bottleneck: f32::INFINITY, node: a, hops: 0, path: vec![a] });
    let mut best: HashMap<u32, f32> = HashMap::new();
    while let Some(S { bottleneck, node, hops, path }) = heap.pop() {
        if node == b { return Some(path); }
        if hops >= max_hops { continue; }
        if best.get(&node).map(|&s| s >= bottleneck).unwrap_or(false) { continue; }
        best.insert(node, bottleneck);
        if let Some(nbrs) = adj.get(&node) {
            for &(nx, w) in nbrs {
                if w < min_weight { continue; }
                let mut np = path.clone();
                np.push(nx);
                heap.push(S { bottleneck: bottleneck.min(w), node: nx, hops: hops + 1, path: np });
            }
        }
    }
    None
}

pub fn gate_bridge() {
    let mut g = ContextGraph::new();
    g.insert(&[5], 100); // A: predecessor {5}
    g.insert(&[5], 200); // C shares pred 5 with A
    g.insert(&[6], 200); // C also has pred 6
    g.insert(&[6], 300); // B shares pred 6 with C, but NOTHING with A
    let adj = g.associations();

    let direct = adj.get(&100).map(|v| v.iter().any(|&(e, _)| e == 300)).unwrap_or(false); // A–B not direct
    let two_hop = bridge(&adj, 100, 300, 3, 0.5);  // routes A→C→B
    let one_hop = bridge(&adj, 100, 300, 1, 0.5);  // can't in a single hop
    let gated = bridge(&adj, 100, 300, 3, 2.0);    // min_weight floor too high → no path

    for (label, ok) in [
        ("A–B not directly associated", !direct),
        ("bridge routes A→C→B in <=3 hops", two_hop.as_deref() == Some(&[100, 200, 300])),
        ("1-hop bound rejects (too far)", one_hop.is_none()),
        ("per-model min_weight floor prunes weak bridge", gated.is_none()),
    ] {
        println!("  [{}] {}", if ok { "\u{2713}" } else { "\u{2717} FAIL" }, label);
    }
    let all = !direct && two_hop.as_deref() == Some(&[100, 200, 300]) && one_hop.is_none() && gated.is_none();
    println!("gate-bridge: {}", if all { "small-world bridge checks \u{2713}" } else { "FAIL \u{2717}" });
}
