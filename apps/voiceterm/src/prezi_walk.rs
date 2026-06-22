//! v0.1.675: "prezi walk" — fit a too-big-to-see graph into the viewport by
//! computing an ordered tour of focal stops, each a zoom/pan transform that
//! frames a cluster of high-priority connected nodes within the viewport.
//!
//! The problem (user, with the scattered-cards screenshot): a mindmap/graph
//! is laid out across a huge canvas; zoomed to fit, nothing is readable;
//! zoomed in, you're lost. A Prezi-style walk fixes this: hop between focal
//! points, each framed so its nodes are legible, ordered so connected +
//! keyword-relevant geometry comes first.
//!
//! Priority = degree centrality (connected edges) + keyword match (the user's
//! "prioritize geometry... with connected edges and keywords"). PURE +
//! testable; consumes graph_layout::CardPos positions; the animated camera
//! transitions are the QML layer (a cinematic_camera.md path already exists).

use crate::graph_layout::{CardPos, LayoutEdge};

/// A node enriched with the signals the walk prioritizes by.
#[derive(Debug, Clone)]
pub struct WalkNode<'a> {
    pub card: &'a CardPos,
    /// keyword-relevance in [0,1] (e.g. fraction of query words present in
    /// the card's text/keywords). 0 if no query.
    pub keyword: f32,
}

/// One focal stop on the walk: a viewport center + a zoom that frames the
/// stop's nodes with padding, plus which node ids are framed.
#[derive(Debug, Clone, PartialEq)]
pub struct Stop {
    pub center_x: f32,
    pub center_y: f32,
    /// zoom = viewport_w / framed_world_w (clamped); >1 zooms in.
    pub zoom: f32,
    pub node_ids: Vec<String>,
    /// the lead node (highest priority) — the stop's anchor/title.
    pub lead: String,
}

/// Viewport + walk tuning.
#[derive(Debug, Clone)]
pub struct WalkConfig {
    pub viewport_w: f32,
    pub viewport_h: f32,
    /// padding fraction around a framed cluster (0.15 = 15% margin)
    pub pad: f32,
    /// max zoom-in (don't blow a single small node up absurdly)
    pub max_zoom: f32,
    /// min zoom (don't zoom out past readable)
    pub min_zoom: f32,
    /// how many nodes to try to frame per stop (a cluster)
    pub cluster_size: usize,
    /// weight of keyword relevance vs degree centrality in priority
    pub keyword_weight: f32,
}

impl Default for WalkConfig {
    fn default() -> Self {
        WalkConfig {
            viewport_w: 1200.0, viewport_h: 800.0,
            pad: 0.15, max_zoom: 2.5, min_zoom: 0.15,
            cluster_size: 4, keyword_weight: 1.0,
        }
    }
}

/// Degree (number of incident edges) for each node id.
fn degrees(edges: &[LayoutEdge]) -> std::collections::HashMap<&str, usize> {
    let mut d = std::collections::HashMap::new();
    for e in edges {
        *d.entry(e.from.as_str()).or_insert(0) += 1;
        *d.entry(e.to.as_str()).or_insert(0) += 1;
    }
    d
}

/// Priority of a node: normalized degree centrality + keyword relevance.
/// Higher = visit earlier / frame more prominently.
pub fn priority(node: &WalkNode, deg: usize, max_deg: usize, cfg: &WalkConfig) -> f32 {
    let centrality = if max_deg > 0 { deg as f32 / max_deg as f32 } else { 0.0 };
    centrality + cfg.keyword_weight * node.keyword
}

/// Adjacency: ids directly connected to `id`.
fn neighbors<'a>(id: &str, edges: &'a [LayoutEdge]) -> Vec<&'a str> {
    let mut out = Vec::new();
    for e in edges {
        if e.from == id { out.push(e.to.as_str()); }
        else if e.to == id { out.push(e.from.as_str()); }
    }
    out
}

/// Build the walk: order nodes by priority, then greedily form stops where
/// each stop frames a lead node + its highest-priority unvisited neighbors
/// (up to cluster_size), computing the zoom that fits them.
pub fn build_walk(nodes: &[WalkNode], edges: &[LayoutEdge], cfg: &WalkConfig) -> Vec<Stop> {
    if nodes.is_empty() { return Vec::new(); }
    let deg = degrees(edges);
    let max_deg = deg.values().copied().max().unwrap_or(0);

    // priority per id
    let mut prio: Vec<(String, f32)> = nodes.iter().map(|n| {
        let d = deg.get(n.card.id.as_str()).copied().unwrap_or(0);
        (n.card.id.clone(), priority(n, d, max_deg, cfg))
    }).collect();
    prio.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        .then(a.0.cmp(&b.0)));

    let by_id: std::collections::HashMap<&str, &WalkNode> =
        nodes.iter().map(|n| (n.card.id.as_str(), n)).collect();
    let prio_of: std::collections::HashMap<&str, f32> =
        prio.iter().map(|(id, p)| (id.as_str(), *p)).collect();

    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut stops = Vec::new();

    for (lead_id, _) in &prio {
        if visited.contains(lead_id) { continue; }
        // gather a cluster: lead + its highest-priority unvisited neighbors
        let mut cluster = vec![lead_id.clone()];
        let mut nbrs: Vec<&str> = neighbors(lead_id, edges).into_iter()
            .filter(|n| !visited.contains(*n) && *n != lead_id.as_str())
            .collect();
        nbrs.sort_by(|a, b| {
            let pa = prio_of.get(a).copied().unwrap_or(0.0);
            let pb = prio_of.get(b).copied().unwrap_or(0.0);
            pb.partial_cmp(&pa).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(b))
        });
        for n in nbrs.into_iter().take(cfg.cluster_size.saturating_sub(1)) {
            cluster.push(n.to_string());
        }
        for c in &cluster { visited.insert(c.clone()); }

        // frame the cluster: bounding box of member cards + padding → zoom
        let members: Vec<&CardPos> = cluster.iter()
            .filter_map(|id| by_id.get(id.as_str()).map(|n| n.card)).collect();
        let stop = frame(&cluster, &members, lead_id, cfg);
        stops.push(stop);
    }
    stops
}

/// Compute the viewport transform that frames the given cards with padding.
fn frame(ids: &[String], cards: &[&CardPos], lead: &str, cfg: &WalkConfig) -> Stop {
    let (cx, cy, world_w, world_h) = if cards.is_empty() {
        (0.0, 0.0, 1.0, 1.0)
    } else {
        let mut min_x = f32::INFINITY; let mut max_x = f32::NEG_INFINITY;
        let mut min_y = f32::INFINITY; let mut max_y = f32::NEG_INFINITY;
        for c in cards {
            min_x = min_x.min(c.x - c.half_w); max_x = max_x.max(c.x + c.half_w);
            min_y = min_y.min(c.y - c.half_h); max_y = max_y.max(c.y + c.half_h);
        }
        (((min_x + max_x) / 2.0), ((min_y + max_y) / 2.0),
         (max_x - min_x).max(1.0), (max_y - min_y).max(1.0))
    };
    // zoom to fit the larger dimension with padding, clamped
    let pad_factor = 1.0 + cfg.pad * 2.0;
    let zoom_x = cfg.viewport_w / (world_w * pad_factor);
    let zoom_y = cfg.viewport_h / (world_h * pad_factor);
    let zoom = zoom_x.min(zoom_y).clamp(cfg.min_zoom, cfg.max_zoom);
    Stop {
        center_x: cx, center_y: cy, zoom,
        node_ids: ids.to_vec(), lead: lead.to_string(),
    }
}

/// Wire format for the QML camera: one stop per line,
/// "lead\tcenter_x\tcenter_y\tzoom\tid1,id2,...".
pub fn to_wire(stops: &[Stop]) -> String {
    stops.iter().map(|s| format!("{}\t{:.1}\t{:.1}\t{:.3}\t{}",
        s.lead, s.center_x, s.center_y, s.zoom, s.node_ids.join(",")))
        .collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, x: f32, y: f32) -> CardPos {
        let mut c = CardPos::new(id); c.x = x; c.y = y; c.half_w = 80.0; c.half_h = 40.0; c
    }

    #[test]
    fn high_degree_node_visited_first() {
        // hub connected to a,b,c; a lone node z far away
        let cards = vec![card("hub",0.0,0.0), card("a",100.0,0.0),
                         card("b",-100.0,0.0), card("c",0.0,100.0), card("z",9000.0,9000.0)];
        let edges = vec![LayoutEdge::new("hub","a"), LayoutEdge::new("hub","b"),
                         LayoutEdge::new("hub","c")];
        let nodes: Vec<WalkNode> = cards.iter().map(|c| WalkNode{card:c, keyword:0.0}).collect();
        let walk = build_walk(&nodes, &edges, &WalkConfig::default());
        assert_eq!(walk[0].lead, "hub", "the hub (highest degree) leads the walk");
        assert!(walk[0].node_ids.contains(&"a".to_string()));
    }

    #[test]
    fn keyword_relevance_promotes_a_node() {
        let cards = vec![card("plain",0.0,0.0), card("relevant",500.0,500.0)];
        let edges: Vec<LayoutEdge> = vec![];
        // no edges → degree equal; keyword on "relevant" should win
        let nodes = vec![
            WalkNode{card:&cards[0], keyword:0.0},
            WalkNode{card:&cards[1], keyword:1.0},
        ];
        let walk = build_walk(&nodes, &edges, &WalkConfig::default());
        assert_eq!(walk[0].lead, "relevant");
    }

    #[test]
    fn zoom_fits_within_constraints() {
        // a huge spread → must zoom OUT but not past min_zoom
        let cards = vec![card("a",0.0,0.0), card("b",100000.0,100000.0)];
        let edges = vec![LayoutEdge::new("a","b")];
        let nodes: Vec<WalkNode> = cards.iter().map(|c| WalkNode{card:c, keyword:0.0}).collect();
        let cfg = WalkConfig::default();
        let walk = build_walk(&nodes, &edges, &cfg);
        for s in &walk {
            assert!(s.zoom >= cfg.min_zoom - 1e-6 && s.zoom <= cfg.max_zoom + 1e-6,
                "zoom {} out of [{}, {}]", s.zoom, cfg.min_zoom, cfg.max_zoom);
        }
    }

    #[test]
    fn small_cluster_zooms_in_but_capped() {
        // two close small cards → would zoom way in; capped at max_zoom
        let cards = vec![card("a",0.0,0.0), card("b",10.0,0.0)];
        let edges = vec![LayoutEdge::new("a","b")];
        let nodes: Vec<WalkNode> = cards.iter().map(|c| WalkNode{card:c, keyword:0.0}).collect();
        let cfg = WalkConfig::default();
        let walk = build_walk(&nodes, &edges, &cfg);
        assert!((walk[0].zoom - cfg.max_zoom).abs() < 1e-6, "zoom capped at max");
    }

    #[test]
    fn every_node_visited_once() {
        let cards = vec![card("a",0.0,0.0), card("b",100.0,0.0),
                         card("c",200.0,0.0), card("d",300.0,0.0)];
        let edges = vec![LayoutEdge::new("a","b"), LayoutEdge::new("c","d")];
        let nodes: Vec<WalkNode> = cards.iter().map(|c| WalkNode{card:c, keyword:0.0}).collect();
        let walk = build_walk(&nodes, &edges, &WalkConfig::default());
        let mut seen: Vec<String> = walk.iter().flat_map(|s| s.node_ids.clone()).collect();
        seen.sort(); seen.dedup();
        assert_eq!(seen.len(), 4, "all nodes framed exactly once across stops");
    }

    #[test]
    fn center_is_cluster_centroid() {
        let cards = vec![card("a",0.0,0.0), card("b",200.0,0.0)];
        let edges = vec![LayoutEdge::new("a","b")];
        let nodes: Vec<WalkNode> = cards.iter().map(|c| WalkNode{card:c, keyword:0.0}).collect();
        let walk = build_walk(&nodes, &edges, &WalkConfig::default());
        // cluster {a,b} centered at x=100
        assert!((walk[0].center_x - 100.0).abs() < 1.0);
    }

    #[test]
    fn wire_format() {
        let cards = vec![card("a",0.0,0.0), card("b",100.0,0.0)];
        let edges = vec![LayoutEdge::new("a","b")];
        let nodes: Vec<WalkNode> = cards.iter().map(|c| WalkNode{card:c, keyword:0.0}).collect();
        let w = to_wire(&build_walk(&nodes, &edges, &WalkConfig::default()));
        assert!(w.contains("a\t")); // lead id present
        assert!(w.contains(",")); // cluster member list
    }
}
