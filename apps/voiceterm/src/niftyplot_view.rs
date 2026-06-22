//! v0.1.697: niftyplot VIEW-layer computations — pure logic the QML canvas
//! drives, kept out of the parser. Two pieces:
//!
//! 1. SMART EXPANSION — progressive disclosure. From a focus node, reveal its
//!    neighborhood out to `depth` hops (BFS over edges). Lets a big graph open
//!    one ring at a time ("expand", "go deeper") instead of dumping every node.
//!
//! 2. 3D LAYER ORIENTATION — the "rotate" vision. Planes (layers) are stacked
//!    in pseudo-3D; a yaw/pitch produces a per-plane 2D affine (offset + scale
//!    + horizontal skew) so the stack fans out into cool isometric / fanned
//!    orientations. EVENT-DRIVEN: callers compute a TARGET orientation on a
//!    "rotate" command and let the QML interpolate (no per-frame polling),
//!    consistent with the camera guidance.
//!
//! Both are pure + testable; NOT wired yet (awaiting greenlight + QML canvas).

use std::collections::{HashMap, HashSet, VecDeque};

/// A minimal view of the graph topology for expansion — id list + undirected
/// adjacency. Built from a GraphDoc's nodes/edges by the caller.
pub struct Topology {
    pub ids: Vec<String>,
    adj: HashMap<String, Vec<String>>,
}

impl Topology {
    pub fn new(ids: &[String], edges: &[(String, String)]) -> Self {
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        for id in ids { adj.entry(id.clone()).or_default(); }
        for (a, b) in edges {
            adj.entry(a.clone()).or_default().push(b.clone());
            adj.entry(b.clone()).or_default().push(a.clone()); // undirected
        }
        Topology { ids: ids.to_vec(), adj }
    }

    /// Smart expansion: BFS from `focus` out to `depth` hops. Returns the
    /// visible node ids (including focus). depth 0 = just the focus; depth 1 =
    /// focus + direct neighbors; etc. Unknown focus → empty.
    pub fn expand(&self, focus: &str, depth: usize) -> Vec<String> {
        if !self.adj.contains_key(focus) { return Vec::new(); }
        let mut seen: HashSet<String> = HashSet::new();
        let mut order: Vec<String> = Vec::new();
        let mut q: VecDeque<(String, usize)> = VecDeque::new();
        q.push_back((focus.to_string(), 0));
        seen.insert(focus.to_string());
        while let Some((id, d)) = q.pop_front() {
            order.push(id.clone());
            if d >= depth { continue; }
            if let Some(ns) = self.adj.get(&id) {
                let mut ns_sorted = ns.clone();
                ns_sorted.sort();            // deterministic ordering
                for n in ns_sorted {
                    if seen.insert(n.clone()) {
                        q.push_back((n, d + 1));
                    }
                }
            }
        }
        order
    }

    /// The hop-distance ring of each visible node from `focus` (for fading
    /// outer rings). Parallel to expand()'s output.
    pub fn rings(&self, focus: &str, depth: usize) -> Vec<(String, usize)> {
        if !self.adj.contains_key(focus) { return Vec::new(); }
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<(String, usize)> = Vec::new();
        let mut q: VecDeque<(String, usize)> = VecDeque::new();
        q.push_back((focus.to_string(), 0));
        seen.insert(focus.to_string());
        while let Some((id, d)) = q.pop_front() {
            out.push((id.clone(), d));
            if d >= depth { continue; }
            if let Some(ns) = self.adj.get(&id) {
                let mut s = ns.clone(); s.sort();
                for n in s {
                    if seen.insert(n.clone()) { q.push_back((n, d + 1)); }
                }
            }
        }
        out
    }

    /// v0.1.699: neighbors of `id` in stable sorted order (for cycling focus
    /// with "go up"/"go down" or Tab). Empty if unknown.
    pub fn neighbors(&self, id: &str) -> Vec<String> {
        match self.adj.get(id) {
            Some(ns) => { let mut v = ns.clone(); v.sort(); v.dedup(); v }
            None => Vec::new(),
        }
    }

    /// v0.1.699: cycle the focus to the next (`forward`) / previous neighbor of
    /// `from`. Used by "go down"/"go up". Returns the new focus id, or `from`
    /// unchanged if it has no neighbors.
    pub fn cycle_neighbor(&self, from: &str, forward: bool) -> String {
        let ns = self.neighbors(from);
        if ns.is_empty() { return from.to_string(); }
        if forward { ns[0].clone() } else { ns[ns.len() - 1].clone() }
    }
}

/// v0.1.699: pick the neighbor of a focus node most in a screen direction,
/// given each candidate's (id,x,y) and the focus's (x,y). dir: 0=up,1=down,
/// 2=left,3=right. Returns the chosen id, or None if no candidate lies that
/// way. Pure geometry — the QML supplies positions from its layout. Used by
/// arrow-key pan of the selected node.
/// v0.1.700: rank a focus node's neighbors for directional pan as a RANKED
/// MATCH rather than a strict axis gate (per user note: "not sure if always
/// directional… could be a ranked match"). Each candidate is scored by how
/// well the focus→candidate vector aligns with the requested direction
/// (cosine of the angle), weighted up for nearness. Candidates in the wrong
/// hemisphere (alignment ≤ 0) are dropped; everything else is ranked, so a
/// diagonal neighbor is still reachable instead of dead-ending. Returns
/// (id, score) sorted best-first. `dir`: 0=up,1=down,2=left,3=right.
pub fn rank_neighbors(focus_xy: (f32, f32), candidates: &[(String, f32, f32)],
                      dir: u8) -> Vec<(String, f32)> {
    let (fx, fy) = focus_xy;
    let (ux, uy): (f32, f32) = match dir {
        0 => (0.0, -1.0),  // up
        1 => (0.0, 1.0),   // down
        2 => (-1.0, 0.0),  // left
        _ => (1.0, 0.0),   // right
    };
    let mut scored: Vec<(String, f32)> = Vec::new();
    for (id, x, y) in candidates {
        let dx = x - fx;
        let dy = y - fy;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < 0.001 { continue; }
        // cosine alignment of the direction with the candidate vector
        let align = (dx * ux + dy * uy) / dist;
        if align <= 0.05 { continue; } // wrong hemisphere → not a match
        // rank: alignment dominates, nearness breaks ties (closer = better).
        let score = align * 1.0 + (1.0 / (1.0 + dist)) * 0.4;
        scored.push((id.clone(), score));
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
}

/// Pick the best-ranked neighbor in a screen direction (ranked match). Returns
/// the chosen id, or None if nothing lies that way. Used by arrow-key pan.
pub fn step_focus(focus_xy: (f32, f32), candidates: &[(String, f32, f32)],
                  dir: u8) -> Option<String> {
    rank_neighbors(focus_xy, candidates, dir).into_iter().next().map(|(id, _)| id)
}

/// A per-plane 2D affine for the pseudo-3D layered view. The QML applies it as
/// a translate + scale + horizontal skew to each plane's contents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerTransform {
    pub dx: f32,
    pub dy: f32,
    pub scale: f32,
    /// horizontal skew in radians (gives the isometric "fan")
    pub skew: f32,
    /// 0..1 opacity — deeper layers fade slightly
    pub opacity: f32,
}

/// The 3D-rotate vision: lay out `n` planes (index 0 = front) under a yaw and
/// pitch (radians) into a fanned/isometric stack. `spread` = px between layers
/// at zero rotation. Front layer is full size/opacity; deeper layers recede.
/// EVENT-DRIVEN: this returns the TARGET transforms for a given orientation;
/// the QML interpolates from current to these (retaining motion).
pub fn layer_orientation(n: usize, yaw: f32, pitch: f32, spread: f32) -> Vec<LayerTransform> {
    let mut out = Vec::with_capacity(n);
    if n == 0 { return out; }
    for i in 0..n {
        let depth = i as f32;             // 0 = front
        // yaw fans layers horizontally; pitch stacks them vertically.
        let dx = depth * spread * yaw.sin();
        let dy = depth * spread * pitch.sin() * 0.6;
        // recede: deeper layers shrink + fade a touch
        let scale = (1.0 - depth * 0.06).max(0.4);
        let skew = yaw * 0.5 * (1.0 - depth * 0.1).max(0.0);
        let opacity = (1.0 - depth * 0.12).clamp(0.35, 1.0);
        out.push(LayerTransform { dx, dy, scale, skew, opacity });
    }
    out
}

/// A few named orientation presets the user can say ("rotate", "flat",
/// "fan", "stack") → (yaw, pitch) targets in radians. Keeps the voice command
/// vocabulary and the geometry in one tested place.
pub fn orientation_preset(name: &str) -> Option<(f32, f32)> {
    let n = name.trim().to_lowercase();
    match n.as_str() {
        "flat" | "front" | "reset" => Some((0.0, 0.0)),
        "fan" | "rotate" | "spread" => Some((0.6, 0.18)),
        "stack" | "iso" | "isometric" => Some((0.5, 0.5)),
        "tilt" => Some((0.25, 0.4)),
        _ => None,
    }
}

/// v0.1.701 (#2 focus breadcrumb): push `id` onto a focus trail. Skips a
/// consecutive duplicate; if `id` is already in the trail, COLLAPSES the loop
/// (truncates back to that earlier visit) so revisiting a node doesn't grow an
/// ever-longer path; caps the trail length (drops oldest). Returns the new
/// trail. Pure + testable.
pub fn breadcrumb_push(trail: &[String], id: &str, cap: usize) -> Vec<String> {
    if id.is_empty() { return trail.to_vec(); }
    // already the tip → no change
    if trail.last().map(|s| s.as_str()) == Some(id) {
        return trail.to_vec();
    }
    // revisit → collapse the loop back to that point (inclusive)
    if let Some(pos) = trail.iter().position(|s| s == id) {
        return trail[..=pos].to_vec();
    }
    let mut out = trail.to_vec();
    out.push(id.to_string());
    if cap > 0 && out.len() > cap {
        let overflow = out.len() - cap;
        out.drain(0..overflow);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topo() -> Topology {
        // A-B-C-D chain + A-E branch
        Topology::new(
            &["A","B","C","D","E"].map(String::from),
            &[("A".into(),"B".into()), ("B".into(),"C".into()),
              ("C".into(),"D".into()), ("A".into(),"E".into())],
        )
    }

    #[test]
    fn breadcrumb_appends_and_dedups_tip() {
        let t: Vec<String> = vec![];
        let t = breadcrumb_push(&t, "A", 8);
        assert_eq!(t, vec!["A"]);
        let t = breadcrumb_push(&t, "A", 8);  // same tip → no change
        assert_eq!(t, vec!["A"]);
        let t = breadcrumb_push(&t, "B", 8);
        assert_eq!(t, vec!["A", "B"]);
    }

    #[test]
    fn breadcrumb_collapses_loop_on_revisit() {
        let t = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        // revisit A → trail collapses to just [A]
        let t = breadcrumb_push(&t, "A", 8);
        assert_eq!(t, vec!["A"]);
        // revisit a middle node
        let t2 = vec!["A".to_string(), "B".to_string(), "C".to_string(), "D".to_string()];
        assert_eq!(breadcrumb_push(&t2, "B", 8), vec!["A", "B"]);
    }

    #[test]
    fn breadcrumb_caps_length() {
        let mut t: Vec<String> = vec![];
        for id in ["A","B","C","D","E"] { t = breadcrumb_push(&t, id, 3); }
        assert_eq!(t, vec!["C", "D", "E"]); // oldest dropped
    }

    #[test]
    fn neighbors_sorted_unique() {
        let t = topo();
        assert_eq!(t.neighbors("A"), vec!["B", "E"]);
        assert_eq!(t.neighbors("B"), vec!["A", "C"]);
        assert!(t.neighbors("Z").is_empty());
    }

    #[test]
    fn cycle_neighbor_forward_back() {
        let t = topo();
        assert_eq!(t.cycle_neighbor("A", true), "B");   // first
        assert_eq!(t.cycle_neighbor("A", false), "E");  // last
        assert_eq!(t.cycle_neighbor("Z", true), "Z");   // no neighbors → unchanged
    }

    #[test]
    fn step_focus_picks_directional_neighbor() {
        // focus at origin; candidates above/below/right
        let cands = vec![
            ("up".to_string(), 0.0, -50.0),
            ("down".to_string(), 5.0, 60.0),
            ("right".to_string(), 70.0, 0.0),
        ];
        assert_eq!(step_focus((0.0, 0.0), &cands, 0).as_deref(), Some("up"));
        assert_eq!(step_focus((0.0, 0.0), &cands, 1).as_deref(), Some("down"));
        assert_eq!(step_focus((0.0, 0.0), &cands, 3).as_deref(), Some("right"));
        // nothing to the left (all candidates are right/up/down)
        assert!(step_focus((0.0, 0.0), &cands, 2).is_none());
    }

    #[test]
    fn step_focus_ranked_match_reaches_diagonal() {
        // a purely diagonal neighbor (up-right) — strict-axis stepping would
        // miss it for a plain "up", but ranked match still reaches it.
        let cands = vec![("ne".to_string(), 40.0, -40.0)];
        // it's in the upper hemisphere → "up" finds it
        assert_eq!(step_focus((0.0, 0.0), &cands, 0).as_deref(), Some("ne"));
        // and the right hemisphere → "right" finds it too (bidirectional reach)
        assert_eq!(step_focus((0.0, 0.0), &cands, 3).as_deref(), Some("ne"));
        // but not "down" or "left"
        assert!(step_focus((0.0, 0.0), &cands, 1).is_none());
    }

    #[test]
    fn rank_neighbors_orders_by_alignment_then_nearness() {
        let cands = vec![
            ("far_up".to_string(), 0.0, -200.0),
            ("near_up".to_string(), 0.0, -30.0),
            ("up_right".to_string(), 60.0, -30.0),
        ];
        let r = rank_neighbors((0.0, 0.0), &cands, 0);
        // straight-up beats diagonal; among straight-up, nearer ranks higher
        assert_eq!(r[0].0, "near_up");
        assert!(r.iter().any(|(id, _)| id == "up_right"));
    }

    #[test]
    fn expand_depth_zero_is_focus_only() {
        assert_eq!(topo().expand("A", 0), vec!["A"]);
    }

    #[test]
    fn expand_one_hop_reveals_neighbors() {
        let v = topo().expand("A", 1);
        // A + its neighbors B and E (sorted)
        assert_eq!(v, vec!["A", "B", "E"]);
    }

    #[test]
    fn expand_two_hops() {
        let v = topo().expand("A", 2);
        // A, then B,E, then C (B's other neighbor)
        assert!(v.contains(&"C".to_string()));
        assert!(!v.contains(&"D".to_string()), "D is 3 hops away");
    }

    #[test]
    fn expand_unknown_focus_empty() {
        assert!(topo().expand("Z", 2).is_empty());
    }

    #[test]
    fn rings_report_hop_distance() {
        let r = topo().rings("A", 2);
        let dist = |id: &str| r.iter().find(|(n,_)| n==id).map(|(_,d)| *d);
        assert_eq!(dist("A"), Some(0));
        assert_eq!(dist("B"), Some(1));
        assert_eq!(dist("E"), Some(1));
        assert_eq!(dist("C"), Some(2));
    }

    #[test]
    fn layer_orientation_front_is_identity_ish() {
        let t = layer_orientation(3, 0.0, 0.0, 40.0);
        assert_eq!(t.len(), 3);
        // front layer: no offset, full scale/opacity
        assert!((t[0].dx).abs() < 0.001 && (t[0].scale - 1.0).abs() < 0.001);
        assert!((t[0].opacity - 1.0).abs() < 0.001);
        // deeper layers recede
        assert!(t[2].scale < t[0].scale);
        assert!(t[2].opacity < t[0].opacity);
    }

    #[test]
    fn layer_orientation_yaw_fans_horizontally() {
        let flat = layer_orientation(3, 0.0, 0.0, 40.0);
        let fanned = layer_orientation(3, 0.6, 0.0, 40.0);
        // a back layer moves sideways when yawed
        assert!(fanned[2].dx.abs() > flat[2].dx.abs());
    }

    #[test]
    fn orientation_presets() {
        assert_eq!(orientation_preset("flat"), Some((0.0, 0.0)));
        assert!(orientation_preset("rotate").is_some());
        assert!(orientation_preset("isometric").is_some());
        assert!(orientation_preset("nonsense").is_none());
    }
}
