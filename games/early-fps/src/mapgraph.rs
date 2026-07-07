//! mapgraph.rs — **flow-first map generation**. A map is abstracted as a 2D graph of nodes (places) and
//! weighted edges (routes between them), and its gameplay is evaluated *on the graph, before any geometry
//! exists*. That inversion is the whole point: a graph is a few hundred bytes, so thousands of candidate
//! layouts can be generated, connectivity-checked, flow-simulated, and scored in milliseconds — and only the
//! handful that pass ever get skinned into 3D. The artist-to-operating-system move from the design notes:
//! generate topology cheaply, filter on metrics, render the survivors.
//!
//! The scoring mirrors the in-engine `mapeval` harness (connectivity is a hard gate; reward balanced flow,
//! sightline variety, flankable vantages) so a high-scoring graph predicts a high-scoring built map. A graph
//! compiles to a [`Blueprint`] — the data-driven hand-off (roles + positions + links) a renderer would skin.

use crate::level::Rng;

/// What a node becomes when skinned: spawns anchor the ends; towers are high vantage; trenches are sunken
/// lanes; rooms are enclosed holds; junctions are open crossroads; cover is a small fought-over pocket.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeRole {
    Spawn,
    Tower,
    Trench,
    Room,
    Junction,
    Cover,
}

impl NodeRole {
    pub fn name(self) -> &'static str {
        match self {
            NodeRole::Spawn => "spawn",
            NodeRole::Tower => "tower",
            NodeRole::Trench => "trench",
            NodeRole::Room => "room",
            NodeRole::Junction => "junction",
            NodeRole::Cover => "cover",
        }
    }
}

/// What an edge becomes: a lane is a long open run (sightline), a corner is a tight broken approach, a ramp
/// connects two different elevations.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EdgeKind {
    Lane,
    Corner,
    Ramp,
}

#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub x: f32, // 0..1 normalized map space
    pub z: f32,
    pub elevation: f32, // -1 (sunken) .. 1 (high)
    pub role: NodeRole,
}

#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub a: usize,
    pub b: usize,
    pub kind: EdgeKind,
    pub cover: f32, // 0 fully open .. 1 fully covered
    pub len: f32,   // normalized length (sightline distance)
}

#[derive(Clone, Debug)]
pub struct MapGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl MapGraph {
    /// Generate a candidate graph: two spawns at opposite ends, a scatter of interior nodes, connected by a
    /// nearest-neighbour spanning tree (guarantees connectivity) plus a few extra links (loops → route
    /// choice). Roles/edge-kinds follow from geometry: high interior nodes become towers, low ones trenches,
    /// well-connected ones junctions; long open edges are lanes, short ones corners, elevation gaps ramps.
    pub fn generate(seed: u64) -> MapGraph {
        let mut rng = Rng::new(seed ^ 0x6A09_E667);
        let count = 8 + rng.i32_range(0, 9) as usize; // 8..16 nodes
        let mut nodes: Vec<Node> = Vec::with_capacity(count);

        // two spawns at opposing corners (jittered)
        nodes.push(Node { x: 0.08 + rng.f32() * 0.1, z: 0.1 + rng.f32() * 0.2, elevation: 0.0, role: NodeRole::Spawn });
        nodes.push(Node { x: 0.82 + rng.f32() * 0.1, z: 0.7 + rng.f32() * 0.2, elevation: 0.0, role: NodeRole::Spawn });

        // interior scatter with Poisson-ish rejection so nodes spread out
        let mut tries = 0;
        while nodes.len() < count && tries < count * 20 {
            tries += 1;
            let nx = 0.12 + rng.f32() * 0.76;
            let nz = 0.12 + rng.f32() * 0.76;
            if nodes.iter().any(|n| (n.x - nx).powi(2) + (n.z - nz).powi(2) < 0.018) {
                continue; // too close to an existing node
            }
            let elev = (rng.f32() * 2.0 - 1.0) * 0.9;
            nodes.push(Node { x: nx, z: nz, elevation: elev, role: NodeRole::Cover });
        }

        // nearest-neighbour spanning tree (Prim) over euclidean distance → guaranteed connected
        let n = nodes.len();
        let dist = |a: &Node, b: &Node| ((a.x - b.x).powi(2) + (a.z - b.z).powi(2)).sqrt();
        let mut in_tree = vec![false; n];
        let mut degree = vec![0u32; n];
        in_tree[0] = true;
        let mut edges: Vec<Edge> = Vec::new();
        for _ in 1..n {
            let mut best = (f32::MAX, 0usize, 0usize);
            for i in 0..n {
                if !in_tree[i] {
                    continue;
                }
                for j in 0..n {
                    if in_tree[j] {
                        continue;
                    }
                    let d = dist(&nodes[i], &nodes[j]);
                    if d < best.0 {
                        best = (d, i, j);
                    }
                }
            }
            let (d, i, j) = best;
            in_tree[j] = true;
            degree[i] += 1;
            degree[j] += 1;
            edges.push(make_edge(i, j, d, &nodes));
        }

        // a few extra chords for route choice (loops) — connect nearby not-yet-linked pairs
        let extra = 2 + rng.i32_range(0, n as i32 / 3) as usize;
        for _ in 0..extra {
            let i = rng.i32_range(0, n as i32) as usize;
            let j = rng.i32_range(0, n as i32) as usize;
            if i == j || edges.iter().any(|e| (e.a == i && e.b == j) || (e.a == j && e.b == i)) {
                continue;
            }
            let d = dist(&nodes[i], &nodes[j]);
            if d > 0.5 {
                continue; // don't add absurd cross-map chords
            }
            degree[i] += 1;
            degree[j] += 1;
            edges.push(make_edge(i, j, d, &nodes));
        }

        // assign roles from elevation + connectivity (spawns keep their role)
        for i in 2..n {
            let e = nodes[i].elevation;
            nodes[i].role = if degree[i] >= 4 {
                NodeRole::Junction
            } else if e > 0.45 {
                NodeRole::Tower
            } else if e < -0.4 {
                NodeRole::Trench
            } else if degree[i] <= 1 {
                NodeRole::Room
            } else {
                NodeRole::Cover
            };
        }

        MapGraph { nodes, edges }
    }

    fn adj(&self) -> Vec<Vec<usize>> {
        let mut a = vec![Vec::new(); self.nodes.len()];
        for e in &self.edges {
            a[e.a].push(e.b);
            a[e.b].push(e.a);
        }
        a
    }

    /// Every node reachable from spawn 0 over the edges.
    pub fn connected(&self) -> bool {
        if self.nodes.is_empty() {
            return true;
        }
        let adj = self.adj();
        let mut seen = vec![false; self.nodes.len()];
        let mut stack = vec![0usize];
        seen[0] = true;
        while let Some(u) = stack.pop() {
            for &v in &adj[u] {
                if !seen[v] {
                    seen[v] = true;
                    stack.push(v);
                }
            }
        }
        seen.iter().all(|&s| s)
    }

    /// Hop distance between the two spawns (BFS). `None` if disconnected.
    pub fn spawn_distance(&self) -> Option<usize> {
        let spawns: Vec<usize> = self.nodes.iter().enumerate().filter(|(_, n)| n.role == NodeRole::Spawn).map(|(i, _)| i).collect();
        if spawns.len() < 2 {
            return None;
        }
        let adj = self.adj();
        let mut dist = vec![usize::MAX; self.nodes.len()];
        let mut q = std::collections::VecDeque::new();
        dist[spawns[0]] = 0;
        q.push_back(spawns[0]);
        while let Some(u) = q.pop_front() {
            for &v in &adj[u] {
                if dist[v] == usize::MAX {
                    dist[v] = dist[u] + 1;
                    q.push_back(v);
                }
            }
        }
        let d = dist[spawns[1]];
        if d == usize::MAX {
            None
        } else {
            Some(d)
        }
    }

    /// Bridge edges whose removal disconnects the graph (chokepoints). Naive O(E·(V+E)) — fine at this scale.
    pub fn chokepoints(&self) -> usize {
        let mut count = 0;
        for skip in 0..self.edges.len() {
            let mut seen = vec![false; self.nodes.len()];
            let mut adj = vec![Vec::new(); self.nodes.len()];
            for (i, e) in self.edges.iter().enumerate() {
                if i == skip {
                    continue;
                }
                adj[e.a].push(e.b);
                adj[e.b].push(e.a);
            }
            let mut stack = vec![0usize];
            seen[0] = true;
            let mut reached = 1;
            while let Some(u) = stack.pop() {
                for &v in &adj[u] {
                    if !seen[v] {
                        seen[v] = true;
                        reached += 1;
                        stack.push(v);
                    }
                }
            }
            if reached < self.nodes.len() {
                count += 1;
            }
        }
        count
    }

    /// Fraction of vantage points (towers) reachable by two or more distinct edges — i.e. flankable rather
    /// than a single dominant perch. The design note's "can A be flanked?" test, aggregated.
    pub fn flank_balance(&self) -> f32 {
        let adj = self.adj();
        let towers: Vec<usize> = self.nodes.iter().enumerate().filter(|(_, n)| n.role == NodeRole::Tower).map(|(i, _)| i).collect();
        if towers.is_empty() {
            return 0.6; // no dominating perch to worry about → neutral-good
        }
        let flankable = towers.iter().filter(|&&t| adj[t].len() >= 2).count();
        flankable as f32 / towers.len() as f32
    }

    /// Compile the graph to a data blueprint — the data-driven hand-off a renderer would skin into geometry.
    pub fn to_blueprint(&self) -> Blueprint {
        Blueprint {
            zones: self
                .nodes
                .iter()
                .enumerate()
                .map(|(i, n)| Zone { id: i, role: n.role, x: n.x, z: n.z, elevation: n.elevation })
                .collect(),
            links: self.edges.iter().map(|e| Link { a: e.a, b: e.b, kind: e.kind, cover: e.cover }).collect(),
        }
    }
}

fn make_edge(i: usize, j: usize, d: f32, nodes: &[Node]) -> Edge {
    let de = (nodes[i].elevation - nodes[j].elevation).abs();
    let kind = if de > 0.6 {
        EdgeKind::Ramp
    } else if d > 0.32 {
        EdgeKind::Lane
    } else {
        EdgeKind::Corner
    };
    // open lanes have little cover; corners and ramps break sightlines
    let cover = match kind {
        EdgeKind::Lane => 0.15,
        EdgeKind::Corner => 0.7,
        EdgeKind::Ramp => 0.5,
    };
    Edge { a: i, b: j, kind, cover, len: d }
}

/// A graph's gameplay scorecard, evaluated before any geometry exists. Sub-scores 0..1; `total` 0..100.
#[derive(Clone, Copy, Debug)]
pub struct GraphScore {
    pub connected: bool,
    pub flow: f32,        // spawn-to-spawn distance in a comfortable band
    pub route_choice: f32, // few enough chokepoints that there are alternate routes
    pub sightlines: f32,   // healthy mix of lane/corner/ramp
    pub flank: f32,        // vantages are flankable
    pub vantage: f32,      // some towers/trenches exist (vertical interest)
    pub total: f32,
}

impl GraphScore {
    pub fn grade(&self) -> char {
        match self.total as i32 {
            85..=100 => 'S',
            72..=84 => 'A',
            60..=71 => 'B',
            48..=59 => 'C',
            34..=47 => 'D',
            _ => 'F',
        }
    }
}

/// Score a candidate graph the way `mapeval` scores a built map — connectivity is a hard gate.
pub fn score(g: &MapGraph) -> GraphScore {
    let connected = g.connected();

    // flow: spawn distance should be neither trivial nor a marathon. Peak around 3–5 hops.
    let flow = match g.spawn_distance() {
        Some(d) => {
            let d = d as f32;
            (1.0 - ((d - 4.0).abs() / 4.0)).clamp(0.0, 1.0)
        }
        None => 0.0,
    };

    // route choice: too many bridge edges → linear/campy. Want a low chokepoint fraction.
    let echoke = g.chokepoints() as f32;
    let efrac = if g.edges.is_empty() { 1.0 } else { echoke / g.edges.len() as f32 };
    let route_choice = (1.0 - efrac).clamp(0.0, 1.0);

    // sightline mix: fractions of lane/corner/ramp vs an ideal spread.
    let (mut lane, mut corner, mut ramp) = (0u32, 0u32, 0u32);
    for e in &g.edges {
        match e.kind {
            EdgeKind::Lane => lane += 1,
            EdgeKind::Corner => corner += 1,
            EdgeKind::Ramp => ramp += 1,
        }
    }
    let tot = (lane + corner + ramp).max(1) as f32;
    let (fl, fc, fr) = (lane as f32 / tot, corner as f32 / tot, ramp as f32 / tot);
    let (il, ic, ir) = (0.40f32, 0.40f32, 0.20f32);
    let sightlines = (1.0 - 0.5 * ((fl - il).abs() + (fc - ic).abs() + (fr - ir).abs())).clamp(0.0, 1.0);

    let flank = g.flank_balance();

    // vantage: at least a couple of high/low features for vertical play.
    let verticals = g.nodes.iter().filter(|n| matches!(n.role, NodeRole::Tower | NodeRole::Trench)).count();
    let vantage = (verticals as f32 / 3.0).min(1.0);

    let weighted = flow * 0.26 + route_choice * 0.22 + sightlines * 0.22 + flank * 0.16 + vantage * 0.14;
    let gate = if connected { 1.0 } else { 0.2 };
    let total = (weighted * gate * 100.0).clamp(0.0, 100.0);

    GraphScore { connected, flow, route_choice, sightlines, flank, vantage, total }
}

/// The factory: generate `n` candidate graphs and return them ranked best-first. This is the cheap filter —
/// thousands of layouts scored in milliseconds, only the top ones worth skinning into geometry.
pub fn mass_rank(n: usize, base_seed: u64) -> Vec<(u64, GraphScore)> {
    let mut out: Vec<(u64, GraphScore)> = (0..n)
        .map(|i| {
            let seed = base_seed.wrapping_add(i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            (seed, score(&MapGraph::generate(seed)))
        })
        .collect();
    out.sort_by(|a, b| b.1.total.partial_cmp(&a.1.total).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Generate `keep` graphs that each clear `min_total`, by generating and filtering (the render-the-survivors
/// pattern). Returns the surviving seeds in score order; stops after scanning `budget` candidates.
pub fn select_best(keep: usize, min_total: f32, base_seed: u64, budget: usize) -> Vec<(u64, GraphScore)> {
    let mut survivors: Vec<(u64, GraphScore)> = Vec::new();
    for i in 0..budget {
        let seed = base_seed.wrapping_add(i as u64).wrapping_mul(0x100_0000_01B3);
        let sc = score(&MapGraph::generate(seed));
        if sc.connected && sc.total >= min_total {
            survivors.push((seed, sc));
            if survivors.len() >= keep {
                break;
            }
        }
    }
    survivors.sort_by(|a, b| b.1.total.partial_cmp(&a.1.total).unwrap_or(std::cmp::Ordering::Equal));
    survivors
}

// ── Data-driven blueprint (the JSON-style hand-off) ──────────────────────────────────────────────────────

/// A zone in the compiled blueprint: a place with a role, position and elevation. The renderer skins this
/// (tower → vantage structure, trench → sunken lane, etc.) without knowing how the graph was scored.
#[derive(Clone, Copy, Debug)]
pub struct Zone {
    pub id: usize,
    pub role: NodeRole,
    pub x: f32,
    pub z: f32,
    pub elevation: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Link {
    pub a: usize,
    pub b: usize,
    pub kind: EdgeKind,
    pub cover: f32,
}

/// The data-driven description a graph compiles to — pure data, no geometry. A universal skinner reads this.
#[derive(Clone, Debug)]
pub struct Blueprint {
    pub zones: Vec<Zone>,
    pub links: Vec<Link>,
}

impl Blueprint {
    /// Serialize to a compact, JSON-ish text form (for logging / hand-off / mass storage). Hand-rolled so the
    /// crate stays dependency-free, but the shape is the data-driven blueprint from the design notes.
    pub fn to_text(&self) -> String {
        let mut s = String::from("{\n  \"zones\": [\n");
        for z in &self.zones {
            s.push_str(&format!(
                "    {{ \"id\": {}, \"role\": \"{}\", \"x\": {:.3}, \"z\": {:.3}, \"elev\": {:.2} }},\n",
                z.id, z.role.name(), z.x, z.z, z.elevation
            ));
        }
        s.push_str("  ],\n  \"links\": [\n");
        for l in &self.links {
            let k = match l.kind {
                EdgeKind::Lane => "lane",
                EdgeKind::Corner => "corner",
                EdgeKind::Ramp => "ramp",
            };
            s.push_str(&format!("    {{ \"a\": {}, \"b\": {}, \"kind\": \"{}\", \"cover\": {:.2} }},\n", l.a, l.b, k, l.cover));
        }
        s.push_str("  ]\n}\n");
        s
    }
}

/// A printable ranking report for a batch of candidate graphs (the factory dashboard).
pub fn report(n: usize, seed: u64) -> String {
    let ranked = mass_rank(n, seed);
    let mut s = format!("GRAPH FACTORY — {} candidates ranked (flow / routes / sightlines / flank)\n", n);
    for (sd, sc) in ranked.iter().take(16) {
        s.push_str(&format!(
            "{:>5.1} {}  seed {:016x}  f{:.2} r{:.2} s{:.2} k{:.2}{}\n",
            sc.total,
            sc.grade(),
            sd,
            sc.flow,
            sc.route_choice,
            sc.sightlines,
            sc.flank,
            if sc.connected { "" } else { "  !DISCONNECTED" },
        ));
    }
    let passing = ranked.iter().filter(|(_, s)| s.connected && s.total >= 48.0).count();
    s.push_str(&format!("\n{} / {} graphs grade C or better — these are the ones worth rendering.\n", passing, n));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_graphs_are_connected() {
        // The spanning-tree backbone guarantees connectivity for every seed.
        for seed in 0..200u64 {
            let g = MapGraph::generate(seed);
            assert!(g.connected(), "graph {} disconnected", seed);
            assert!(g.spawn_distance().is_some(), "graph {} has no spawn path", seed);
        }
    }

    #[test]
    fn scoring_gates_on_connectivity() {
        // A hand-built disconnected graph scores far below a connected one.
        let mut g = MapGraph::generate(3);
        let connected_score = score(&g).total;
        g.edges.clear(); // sever everything
        let severed = score(&g);
        assert!(!severed.connected, "cleared graph should be disconnected");
        assert!(severed.total < connected_score * 0.5, "disconnected graphs must be gated down hard");
    }

    #[test]
    fn the_factory_filters_and_ranks() {
        let ranked = mass_rank(300, 42);
        assert_eq!(ranked.len(), 300);
        // sorted descending
        for w in ranked.windows(2) {
            assert!(w[0].1.total >= w[1].1.total, "ranking must be best-first");
        }
        // selection finds a batch of good graphs within budget
        let best = select_best(5, 55.0, 42, 4000);
        assert!(best.len() >= 1, "factory should surface at least one strong graph");
        assert!(best.iter().all(|(_, s)| s.connected && s.total >= 55.0), "survivors must clear the bar");
    }

    #[test]
    fn graph_compiles_to_a_blueprint() {
        let g = MapGraph::generate(11);
        let bp = g.to_blueprint();
        assert_eq!(bp.zones.len(), g.nodes.len());
        assert_eq!(bp.links.len(), g.edges.len());
        let txt = bp.to_text();
        assert!(txt.contains("\"zones\""));
        assert!(txt.contains("\"links\""));
        // spawns survive the compile
        assert!(bp.zones.iter().filter(|z| z.role == NodeRole::Spawn).count() >= 2);
    }
}
