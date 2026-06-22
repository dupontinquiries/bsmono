//! v0.1.738: layered (Sugiyama/dagre-style) graph layout. Force-directed
//! placement scatters directed flowcharts — the dogfood MIPS-datapath diagram
//! overlapped badly while a mature mermaid renderer laid the same graph out as
//! clean top-to-bottom layers. This module is the proper algorithm for a
//! DIRECTED graph: assign layers along edge direction, order within each layer
//! to reduce crossings, then place by (layer depth, within-layer slot).
//!
//! Pure: graph (CardPos + LayoutEdge) in, positions written onto the CardPos
//! out. No QML, no force iteration. Operates in the same coordinate space as
//! graph_layout (screen coords, +y down) and honors FlowDirection.

use crate::graph_layout::{CardPos, LayoutEdge, FlowDirection};
use std::collections::{HashMap, HashSet, VecDeque};

/// Spacing knobs. Gaps are added to node half-extents so big cards don't
/// collide. Tuned for the ~10-60 node flowcharts this targets.
#[derive(Clone, Copy, Debug)]
pub struct LayeredConfig {
    /// gap between adjacent layers (in the flow direction).
    pub layer_gap: f32,
    /// gap between siblings within a layer (across the flow direction).
    pub sibling_gap: f32,
    /// crossing-reduction sweeps (down+up counts as 2).
    pub sweeps: u32,
    /// v0.1.741: arrow-crossover tuning knobs.
    /// adjacent-swap (transpose) passes after the median sweeps — the standard
    /// Sugiyama crossing-reduction refinement. Each pass walks every layer and
    /// swaps adjacent nodes when it strictly lowers the crossing count with the
    /// neighbor layers. 0 disables.
    pub transpose_passes: u32,
    /// when true, transpose also keeps swaps that leave crossings EQUAL (helps
    /// escape plateaus); when false, only strict improvements are kept.
    pub transpose_break_ties: bool,
}

impl Default for LayeredConfig {
    fn default() -> Self {
        LayeredConfig {
            layer_gap: 90.0,
            sibling_gap: 36.0,
            sweeps: 8,
            transpose_passes: 4,
            transpose_break_ties: false,
        }
    }
}

/// Lay out `cards` as a layered graph along `dir`. Returns true if it ran
/// (i.e. there were edges to define direction); false if the graph is edgeless
/// (caller should fall back to a grid / force layout). Pinned cards keep their
/// position but still occupy a slot.
pub fn layout_layered(
    cards: &mut [CardPos],
    edges: &[LayoutEdge],
    dir: FlowDirection,
    cfg: &LayeredConfig,
) -> bool {
    layout_layered_grouped(cards, edges, dir, cfg, &[])
}

/// v0.1.759: layered layout with optional subgraph group membership. Each entry
/// of `group_members` is a list of member node ids (raw or wire ids — matched by
/// the contiguity pass). Supplying groups makes the ordering keep each group's
/// nodes adjacent within every layer so the rendered group box stays tight.
/// Pass groups OUTER-FIRST so nested groups refine within their parent cluster.
pub fn layout_layered_grouped(
    cards: &mut [CardPos],
    edges: &[LayoutEdge],
    dir: FlowDirection,
    cfg: &LayeredConfig,
    group_members: &[Vec<String>],
) -> bool {
    if cards.is_empty() || edges.is_empty() {
        return false;
    }
    // index by id
    let idx: HashMap<&str, usize> =
        cards.iter().enumerate().map(|(i, c)| (c.id.as_str(), i)).collect();

    // adjacency (only edges whose endpoints both exist)
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); cards.len()];
    let mut pred: Vec<Vec<usize>> = vec![Vec::new(); cards.len()];
    for e in edges {
        if let (Some(&a), Some(&b)) = (idx.get(e.from.as_str()), idx.get(e.to.as_str())) {
            if a != b {
                succ[a].push(b);
                pred[b].push(a);
            }
        }
    }

    // ── Phase 1: layer assignment (longest-path from sources) ──
    // Break cycles greedily so longest-path terminates: a DFS that ignores
    // back-edges (edges to a node currently on the recursion stack).
    let n = cards.len();
    let back_edges = find_back_edges(&succ, n);
    // forward adjacency without back-edges
    let mut fsucc: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut findeg: Vec<usize> = vec![0; n];
    for a in 0..n {
        for &b in &succ[a] {
            if back_edges.contains(&(a, b)) { continue; }
            fsucc[a].push(b);
            findeg[b] += 1;
        }
    }
    // layer = longest path from any source, via Kahn topological relaxation.
    let mut layer = vec![0u32; n];
    let mut indeg = findeg.clone();
    let mut q: VecDeque<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut seen = 0usize;
    while let Some(u) = q.pop_front() {
        seen += 1;
        for &v in &fsucc[u] {
            if layer[u] + 1 > layer[v] { layer[v] = layer[u] + 1; }
            indeg[v] -= 1;
            if indeg[v] == 0 { q.push_back(v); }
        }
    }
    // defensive: if a residual cycle left nodes unprocessed, place them at the
    // max layer so they at least don't pile at 0.
    if seen < n {
        let maxl = *layer.iter().max().unwrap_or(&0);
        for i in 0..n {
            if indeg[i] > 0 { layer[i] = maxl + 1; }
        }
    }

    // v0.1.740/760: pull leaf-SOURCE nodes down toward their consumer(s)
    // instead of leaving them all piled at rank 0. A node with no incoming edge
    // (a control signal like RegDst, or an input) reads far better docked just
    // above its EARLIEST consumer than stranded in a giant top layer (MIPS
    // layer-imbalance was 2.84 — max layer 9 nodes vs mean 3.2). v760 extends
    // this from single-successor to ANY number of successors, docking to
    // (min successor layer - 1): for a control bit fanning out to several muxes,
    // this still pulls it out of rank 0 and beside its closest consumer. Docking
    // to min-1 is strictly < every successor layer, so it never creates a
    // back-edge. Pinned nodes are left alone.
    {
        let mut new_layer = layer.clone();
        for u in 0..n {
            if findeg[u] != 0 { continue; }          // not a source
            if cards[u].pinned { continue; }
            if fsucc[u].is_empty() { continue; }     // fully disconnected — leave
            let min_succ_layer = fsucc[u].iter().map(|&s| layer[s]).min().unwrap_or(0);
            if min_succ_layer >= 1 {
                new_layer[u] = min_succ_layer - 1;
            }
        }
        layer = new_layer;
    }

    // v0.1.760: distribute fully-disconnected nodes (no in- AND no out-edges)
    // instead of leaving them piled at rank 0. With no edges they have no
    // natural layer, and dumping them all at 0 both lopsides layer occupancy
    // (MIPS: 7 isolated nodes stranded in the top row of the screenshot) and
    // gives them no relationship to the flow. Assign each to the currently
    // least-occupied layer so they spread evenly rather than crowding one rank.
    {
        let max_layer = *layer.iter().max().unwrap_or(&0);
        if max_layer >= 1 {
            // occupancy of CONNECTED nodes per layer
            let mut occ = vec![0usize; (max_layer + 1) as usize];
            for u in 0..n {
                if findeg[u] != 0 || !fsucc[u].is_empty() {
                    occ[layer[u] as usize] += 1;
                }
            }
            for u in 0..n {
                if cards[u].pinned { continue; }
                if findeg[u] == 0 && fsucc[u].is_empty() {
                    // pick the least-occupied layer, then count it
                    let (li, _) = occ.iter().enumerate()
                        .min_by_key(|(_, &c)| c).unwrap();
                    layer[u] = li as u32;
                    occ[li] += 1;
                }
            }
        }
    }

    // ── Phase 1b: dummy nodes for multi-layer edges (v0.1.741) ──
    // The user hand-inserts `tmp` routing nodes to keep long wires from
    // tangling — exactly what a layout engine should do automatically. For
    // every forward edge whose endpoints are >1 layer apart, we insert a chain
    // of invisible DUMMY nodes (one per intermediate layer). They participate
    // in ordering + coordinate assignment so the edge routes cleanly through
    // intermediate layers (fewer crossings) and reserves cross-axis space (so
    // it doesn't pass through other cards). This is the standard Sugiyama step
    // we'd been skipping (layout lesson #10) and the thing that lets big graphs
    // route without manual `tmp` nodes.
    //
    // Real nodes are indices 0..n; dummies are n.. . We track per-node layer +
    // extents + real-flag, and a chain map so edges can later expose waypoints.
    let mut node_layer: Vec<u32> = layer.clone();
    let mut ext_w: Vec<f32> = (0..n).map(|i| cards[i].half_w).collect();
    let mut ext_h: Vec<f32> = (0..n).map(|i| cards[i].half_h).collect();
    let mut succ_a = fsucc.clone();          // augmented forward adjacency
    let mut pred_a: Vec<Vec<usize>> = vec![Vec::new(); n];
    for a in 0..n { for &b in &succ_a[a] { pred_a[b].push(a); } }
    // dummy extents: small so they barely reserve space (just enough to route).
    const DUMMY_EXT: f32 = 5.0;
    // walk a snapshot of the real edges (forward, non-back) and splice dummies.
    let real_edges: Vec<(usize, usize)> =
        (0..n).flat_map(|a| fsucc[a].iter().map(move |&b| (a, b))).collect();
    for (a, b) in real_edges {
        let (la, lb) = (node_layer[a], node_layer[b]);
        let (lo, hi) = if la <= lb { (la, lb) } else { (lb, la) };
        if hi <= lo + 1 { continue; }        // adjacent layers — no dummy needed
        // remove the direct a→b adjacency; rebuild through the chain.
        succ_a[a].retain(|&x| x != b);
        pred_a[b].retain(|&x| x != a);
        let mut prev = a;
        // intermediate layers strictly between a and b (in flow order la→lb)
        let step: i64 = if la < lb { 1 } else { -1 };
        let mut l = la as i64 + step;
        while l != lb as i64 {
            let d = node_layer.len();
            node_layer.push(l as u32);
            ext_w.push(DUMMY_EXT);
            ext_h.push(DUMMY_EXT);
            succ_a.push(Vec::new());
            pred_a.push(Vec::new());
            succ_a[prev].push(d);
            pred_a[d].push(prev);
            prev = d;
            l += step;
        }
        succ_a[prev].push(b);
        pred_a[b].push(prev);
    }
    let na = node_layer.len();               // augmented node count (real+dummy)
    let real_n = cards.len();                 // real nodes are indices 0..real_n
    // ordering/transpose use undirected adjacency among layers, so build
    // symmetric pred/succ over the augmented graph for neighbor lookups.
    let succ = succ_a;
    let pred = pred_a;
    let n = na;                              // shadow n for the augmented phases

    let max_layer = *node_layer.iter().max().unwrap_or(&0);
    let layer = node_layer;                  // shadow layer (now per augmented node)
    // group node indices by layer
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); (max_layer + 1) as usize];
    for i in 0..n {
        layers[layer[i] as usize].push(i);
    }

    // ── Phase 2: within-layer ordering (median heuristic) ──
    // initial order = insertion order; then sweep down/up adjusting each node's
    // position to the median of its neighbors' positions in the adjacent layer.
    let mut order: Vec<Vec<usize>> = layers.clone();
    let mut pos_in_layer: Vec<usize> = vec![0; n]; // node → its index within its layer
    recompute_positions(&order, &mut pos_in_layer);

    for sweep in 0..cfg.sweeps {
        let down = sweep % 2 == 0;
        let range: Vec<usize> = if down {
            (1..order.len()).collect()
        } else {
            (0..order.len().saturating_sub(1)).rev().collect()
        };
        for li in range {
            // neighbors come from the previous layer (down) or next (up)
            let medians: Vec<(usize, f32)> = order[li].iter().map(|&u| {
                let neigh: &Vec<usize> = if down { &pred[u] } else { &succ[u] };
                let m = median_pos(neigh, &pos_in_layer);
                (u, m)
            }).collect();
            // sort by median; nodes with no neighbors (-1) keep relative order
            let mut sorted = medians.clone();
            sorted.sort_by(|a, b| {
                let av = if a.1 < 0.0 { pos_in_layer[a.0] as f32 } else { a.1 };
                let bv = if b.1 < 0.0 { pos_in_layer[b.0] as f32 } else { b.1 };
                av.partial_cmp(&bv).unwrap_or(std::cmp::Ordering::Equal)
            });
            order[li] = sorted.into_iter().map(|(u, _)| u).collect();
            recompute_positions(&order, &mut pos_in_layer);
        }
    }

    // ── Phase 2b: transpose (adjacent-swap) crossing reduction ──
    // v0.1.741: the median heuristic gives a good ordering but leaves local
    // inversions. Each pass walks every layer and swaps adjacent node pairs
    // when it lowers the crossings against BOTH neighbor layers. Repeats until
    // no improvement or transpose_passes is exhausted. This is the standard
    // Sugiyama refinement and the main "arrow crossover" knob.
    for _ in 0..cfg.transpose_passes {
        let mut improved = false;
        for li in 0..order.len() {
            let mut k = 0;
            while k + 1 < order[li].len() {
                let before = local_crossings(li, &order, &pred, &succ, &pos_in_layer);
                order[li].swap(k, k + 1);
                recompute_positions(&order, &mut pos_in_layer);
                let after = local_crossings(li, &order, &pred, &succ, &pos_in_layer);
                let keep = if cfg.transpose_break_ties { after <= before } else { after < before };
                if keep && after != before {
                    improved = true;
                } else if !(keep && after < before) {
                    // revert
                    order[li].swap(k, k + 1);
                    recompute_positions(&order, &mut pos_in_layer);
                }
                k += 1;
            }
        }
        if !improved { break; }
    }

    // ── Phase 2c: group contiguity (v0.1.759) ──
    // Subgraph members were ordered purely by crossing reduction, so a group's
    // nodes could end up scattered within a layer — making the rendered group
    // box sprawl across unrelated cards. This pass, when groups are supplied,
    // clusters each group's members ADJACENTLY within every layer, anchoring the
    // cluster at the members' mean position so the overall left-to-right order
    // (and thus crossings) is disturbed as little as possible. Non-members keep
    // their relative order. Groups are applied outer-first so nested groups
    // refine within their parent's cluster.
    if !group_members.is_empty() {
        // group index per node (last group wins for overlap; outer-first order
        // means inner groups refine after). -1 = ungrouped.
        for members in group_members {
            let gset: std::collections::HashSet<usize> = members.iter()
                .filter_map(|m| {
                    // tolerate gn_ namespace on either side
                    let raw = m.strip_prefix("gn_").unwrap_or(m);
                    idx.get(m.as_str()).copied()
                        .or_else(|| idx.get(raw).copied())
                        .or_else(|| {
                            let pref = format!("gn_{}", raw);
                            idx.get(pref.as_str()).copied()
                        })
                }).collect();
            if gset.len() < 2 { continue; }
            for li in 0..order.len() {
                let in_layer: Vec<usize> = order[li].iter().copied()
                    .filter(|u| gset.contains(u)).collect();
                if in_layer.len() < 2 { continue; }
                // anchor = mean current index of the group's members in this layer
                let positions: Vec<usize> = order[li].iter().enumerate()
                    .filter(|(_, u)| gset.contains(u)).map(|(p, _)| p).collect();
                let anchor = positions.iter().sum::<usize>() / positions.len();
                // rebuild the layer: non-members in order, with the contiguous
                // member cluster inserted at the anchor slot.
                let non: Vec<usize> = order[li].iter().copied()
                    .filter(|u| !gset.contains(u)).collect();
                let insert_at = anchor.min(non.len());
                let mut rebuilt = Vec::with_capacity(order[li].len());
                rebuilt.extend_from_slice(&non[..insert_at]);
                rebuilt.extend_from_slice(&in_layer); // preserve member order
                rebuilt.extend_from_slice(&non[insert_at..]);
                order[li] = rebuilt;
            }
        }
        recompute_positions(&order, &mut pos_in_layer);
    }

    // ── Phase 3: coordinate assignment ──
    // cross-axis: pack each layer's nodes left-to-right (or top-to-bottom for
    // LR/RL) with sibling_gap + their half-extents; center each layer.
    // flow-axis: layer index × (layer_gap + layer's max extent).
    let horizontal = matches!(dir, FlowDirection::LeftRight | FlowDirection::RightLeft);
    let reverse_flow = matches!(dir, FlowDirection::BottomUp | FlowDirection::RightLeft);

    // flow-axis position per layer (cumulative by max extent in that axis).
    // v0.1.754: ADAPTIVE layer gap. Pure chains (one node per layer) stretched
    // to extreme aspect ratios (cpu_pipeline 16:1) because a fixed 90px gap ×
    // many layers dwarfs the thin cross axis. Estimate the cross-axis span and
    // the natural flow span, and if flow would dwarf cross, shrink the gap
    // toward a target aspect — never below a readable floor, never above the
    // configured gap. Graphs that are already balanced (mips) keep the full gap.
    let n_layers = order.len().max(1) as f32;
    // rough cross span = widest layer's packed width (sum of extents + sib gaps)
    let est_cross = (0..order.len()).map(|li| {
        let mut w = 0.0f32;
        for (k, &u) in order[li].iter().enumerate() {
            if k > 0 { w += cfg.sibling_gap; }
            w += 2.0 * if horizontal { ext_h[u] } else { ext_w[u] };
        }
        w
    }).fold(0.0f32, f32::max).max(1.0);
    // natural flow span with the full gap (sum of layer extents + gaps)
    let est_flow_ext: f32 = (0..order.len()).map(|li| {
        2.0 * order[li].iter()
            .map(|&u| if horizontal { ext_w[u] } else { ext_h[u] })
            .fold(0.0f32, f32::max)
    }).sum();
    let est_flow_full = est_flow_ext + cfg.layer_gap * (n_layers - 1.0).max(0.0);
    // current aspect along the flow axis (flow / cross, both as the long/short
    // the user sees). If flow >> cross, pull the gap down toward target.
    let aspect = est_flow_full / est_cross;
    let target_aspect = 2.2f32; // beyond this it reads "flat and long"
    let eff_gap = if aspect > target_aspect && n_layers > 1.0 {
        // gap that would hit target_aspect, clamped to [22, layer_gap]
        let wanted_flow = est_cross * target_aspect;
        let wanted_gap = (wanted_flow - est_flow_ext) / (n_layers - 1.0);
        wanted_gap.clamp(18.0, cfg.layer_gap)
    } else {
        cfg.layer_gap
    };

    let mut layer_flow: Vec<f32> = vec![0.0; order.len()];
    let mut acc = 0.0f32;
    for li in 0..order.len() {
        let max_ext = order[li].iter()
            .map(|&u| if horizontal { ext_w[u] } else { ext_h[u] })
            .fold(0.0f32, f32::max);
        layer_flow[li] = acc + max_ext;
        acc += 2.0 * max_ext + eff_gap;
    }
    let total_flow = acc;

    // cross-axis: place within each layer, then center.
    // First compute each layer's total cross extent to center the whole graph.
    let mut layer_cross_positions: Vec<Vec<f32>> = Vec::with_capacity(order.len());
    let mut layer_widths: Vec<f32> = Vec::with_capacity(order.len());
    for li in 0..order.len() {
        let mut c = 0.0f32;
        let mut positions = Vec::with_capacity(order[li].len());
        for (k, &u) in order[li].iter().enumerate() {
            let ext = if horizontal { ext_h[u] } else { ext_w[u] };
            if k > 0 { c += cfg.sibling_gap; }
            c += ext;
            positions.push(c);
            c += ext;
        }
        layer_cross_positions.push(positions);
        layer_widths.push(c);
    }
    let max_cross = layer_widths.iter().cloned().fold(0.0f32, f32::max);

    for li in 0..order.len() {
        let flow = if reverse_flow { total_flow - layer_flow[li] } else { layer_flow[li] };
        let center_shift = (max_cross - layer_widths[li]) * 0.5;
        for (k, &u) in order[li].iter().enumerate() {
            let cross = layer_cross_positions[li][k] + center_shift;
            if u >= real_n { continue; }     // dummy node — no card to position
            if cards[u].pinned { continue; } // keep pinned cards put
            if horizontal {
                cards[u].x = flow;
                cards[u].y = cross;
            } else {
                cards[u].x = cross;
                cards[u].y = flow;
            }
        }
    }
    true
}

/// Crossings between two adjacent layers given their orderings. For every pair
/// of edges (u1→v1, u2→v2) connecting the layers, they cross when the relative
/// order of the endpoints is inverted. `pos` gives within-layer index.
fn bilayer_crossings(
    upper: &[usize], lower: &[usize],
    down_adj: &[Vec<usize>], // for each lower node, its neighbors in `upper`
    pos_upper: &[usize],
) -> u64 {
    // collect edge endpoint position-pairs (upper_pos, lower_pos)
    let mut seq: Vec<usize> = Vec::new(); // upper positions ordered by lower order
    for (lower_pos, &v) in lower.iter().enumerate() {
        let _ = lower_pos;
        let mut ups: Vec<usize> = down_adj[v].iter()
            .filter(|&&u| upper.contains(&u))
            .map(|&u| pos_upper[u]).collect();
        ups.sort_unstable();
        seq.extend(ups);
    }
    // count inversions in seq = number of crossings
    let mut crossings = 0u64;
    for i in 0..seq.len() {
        for j in (i + 1)..seq.len() {
            if seq[i] > seq[j] { crossings += 1; }
        }
    }
    crossings
}

/// Crossings of layer `li` against its immediate neighbor layers (li-1, li+1).
fn local_crossings(
    li: usize,
    order: &[Vec<usize>],
    pred: &[Vec<usize>],
    succ: &[Vec<usize>],
    pos: &[usize],
) -> u64 {
    let mut total = 0u64;
    if li > 0 {
        // edges between layer li-1 (upper) and li (lower); lower's up-neighbors = pred
        total += bilayer_crossings(&order[li - 1], &order[li], pred, pos);
    }
    if li + 1 < order.len() {
        // edges between li (upper) and li+1 (lower); lower's up-neighbors = pred too
        total += bilayer_crossings(&order[li], &order[li + 1], pred, pos);
    }
    let _ = succ;
    total
}

/// Recompute node→within-layer-index from the order lists.
fn recompute_positions(order: &[Vec<usize>], pos: &mut [usize]) {
    for layer in order {
        for (i, &u) in layer.iter().enumerate() {
            pos[u] = i;
        }
    }
}

/// Median of the neighbors' within-layer positions. Returns -1.0 if no
/// neighbors (caller keeps the node's current position).
fn median_pos(neigh: &[usize], pos: &[usize]) -> f32 {
    if neigh.is_empty() { return -1.0; }
    let mut ps: Vec<usize> = neigh.iter().map(|&u| pos[u]).collect();
    ps.sort_unstable();
    let m = ps.len() / 2;
    if ps.len() % 2 == 1 {
        ps[m] as f32
    } else {
        (ps[m - 1] + ps[m]) as f32 / 2.0
    }
}

/// Identify back-edges (a→b where b is an ancestor on the DFS stack) so layer
/// assignment can ignore them and terminate on cyclic graphs.
fn find_back_edges(succ: &[Vec<usize>], n: usize) -> HashSet<(usize, usize)> {
    let mut back = HashSet::new();
    let mut state = vec![0u8; n]; // 0=unvisited 1=on-stack 2=done
    // iterative DFS to avoid blowing the stack on large graphs
    for start in 0..n {
        if state[start] != 0 { continue; }
        // stack of (node, next-child-index)
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        state[start] = 1;
        while let Some(&(u, ci)) = stack.last() {
            if ci < succ[u].len() {
                stack.last_mut().unwrap().1 += 1;
                let v = succ[u][ci];
                match state[v] {
                    1 => { back.insert((u, v)); }       // on stack → back edge
                    0 => { state[v] = 1; stack.push((v, 0)); }
                    _ => {}
                }
            } else {
                state[u] = 2;
                stack.pop();
            }
        }
    }
    back
}

/// v0.1.761: public id-based back-edge classifier for the renderer. Returns the
/// set of (from_id, to_id) edges that point AGAINST the flow (cycle back-edges),
/// so the canvas can draw them as distinct curved "return" edges instead of
/// straight lines that read as errors. Pure: builds the same adjacency
/// layout_layered uses and runs the same cycle-breaking DFS, but keyed by id so
/// the caller (mermaid_to_layout) can flag edges without re-deriving indices.
pub fn classify_back_edges(
    cards: &[CardPos],
    edges: &[LayoutEdge],
) -> std::collections::HashSet<(String, String)> {
    let idx: HashMap<&str, usize> =
        cards.iter().enumerate().map(|(i, c)| (c.id.as_str(), i)).collect();
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); cards.len()];
    for e in edges {
        if let (Some(&a), Some(&b)) = (idx.get(e.from.as_str()), idx.get(e.to.as_str())) {
            if a != b { succ[a].push(b); }
        }
    }
    let back_idx = find_back_edges(&succ, cards.len());
    // map index pairs back to id pairs
    let mut out = std::collections::HashSet::new();
    for e in edges {
        if let (Some(&a), Some(&b)) = (idx.get(e.from.as_str()), idx.get(e.to.as_str())) {
            if back_idx.contains(&(a, b)) {
                out.insert((e.from.clone(), e.to.clone()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_layout::{CardPos, LayoutEdge, FlowDirection};

    fn card(id: &str) -> CardPos {
        let mut c = CardPos::new(id);
        c.half_w = 40.0; c.half_h = 20.0;
        c
    }
    fn edge(a: &str, b: &str) -> LayoutEdge { LayoutEdge::new(a, b) }

    #[test]
    fn empty_or_edgeless_returns_false() {
        let mut cards = vec![card("a"), card("b")];
        assert!(!layout_layered(&mut cards, &[], FlowDirection::TopDown, &LayeredConfig::default()));
    }

    #[test]
    fn simple_chain_layers_increase_downward() {
        // a -> b -> c  (TD): y should strictly increase a < b < c
        let mut cards = vec![card("a"), card("b"), card("c")];
        let edges = vec![edge("a", "b"), edge("b", "c")];
        assert!(layout_layered(&mut cards, &edges, FlowDirection::TopDown, &LayeredConfig::default()));
        let y: Vec<f32> = cards.iter().map(|c| c.y).collect();
        assert!(y[0] < y[1] && y[1] < y[2], "TD chain should descend: {:?}", y);
    }

    #[test]
    fn left_right_uses_x_axis() {
        let mut cards = vec![card("a"), card("b"), card("c")];
        let edges = vec![edge("a", "b"), edge("b", "c")];
        layout_layered(&mut cards, &edges, FlowDirection::LeftRight, &LayeredConfig::default());
        let x: Vec<f32> = cards.iter().map(|c| c.x).collect();
        assert!(x[0] < x[1] && x[1] < x[2], "LR chain should advance in x: {:?}", x);
    }

    #[test]
    fn bottom_up_reverses_flow() {
        let mut cards = vec![card("a"), card("b")];
        let edges = vec![edge("a", "b")];
        layout_layered(&mut cards, &edges, FlowDirection::BottomUp, &LayeredConfig::default());
        // BT: b is "downstream" of a but placed ABOVE it (smaller y)
        assert!(cards[1].y < cards[0].y, "BT should put successor above");
    }

    #[test]
    fn siblings_share_layer_and_separate() {
        // a -> b, a -> c : b and c on the same layer, different cross positions
        let mut cards = vec![card("a"), card("b"), card("c")];
        let edges = vec![edge("a", "b"), edge("a", "c")];
        layout_layered(&mut cards, &edges, FlowDirection::TopDown, &LayeredConfig::default());
        assert!((cards[1].y - cards[2].y).abs() < 0.01, "b,c same layer (same y)");
        assert!((cards[1].x - cards[2].x).abs() > 1.0, "b,c separated in x");
        assert!(cards[1].y > cards[0].y, "children below parent");
    }

    #[test]
    fn handles_cycle_without_hanging() {
        // a -> b -> c -> a (cycle). Must terminate + assign distinct layers
        // for the acyclic part.
        let mut cards = vec![card("a"), card("b"), card("c")];
        let edges = vec![edge("a", "b"), edge("b", "c"), edge("c", "a")];
        let ran = layout_layered(&mut cards, &edges, FlowDirection::TopDown, &LayeredConfig::default());
        assert!(ran);
        // a<b<c in layer order (the c->a back edge is dropped)
        assert!(cards[0].y < cards[1].y && cards[1].y < cards[2].y);
    }

    #[test]
    fn pinned_cards_keep_position() {
        let mut cards = vec![CardPos::pinned_at("a", 999.0, 888.0), card("b")];
        cards[0].half_w = 40.0; cards[0].half_h = 20.0;
        let edges = vec![edge("a", "b")];
        layout_layered(&mut cards, &edges, FlowDirection::TopDown, &LayeredConfig::default());
        assert_eq!(cards[0].x, 999.0);
        assert_eq!(cards[0].y, 888.0);
    }

    #[test]
    fn bilayer_crossings_counts_inversions() {
        // upper [a,b] at pos 0,1 ; lower [x,y] where x↔b, y↔a → 1 crossing.
        let upper = vec![0usize, 1]; // a=0, b=1
        let lower = vec![2usize, 3]; // x=2, y=3
        let mut pred = vec![Vec::new(); 4];
        pred[2] = vec![1]; // x ← b (upper pos 1)
        pred[3] = vec![0]; // y ← a (upper pos 0)
        let pos = vec![0usize, 1, 0, 1];
        assert_eq!(bilayer_crossings(&upper, &lower, &pred, &pos), 1);
        // swap lower order → 0 crossings
        let lower2 = vec![3usize, 2];
        assert_eq!(bilayer_crossings(&upper, &lower2, &pred, &pos), 0);
    }

    #[test]
    fn transpose_passes_knob_is_respected() {
        // a graph whose median pass leaves a fixable inversion is rare to
        // construct (median is strong), so just assert the knob runs without
        // changing a clean result and that 0 passes is also valid.
        let mut cards = vec![card("a"), card("b"), card("c")];
        let edges = vec![edge("a", "b"), edge("b", "c")];
        let mut cfg = LayeredConfig::default();
        cfg.transpose_passes = 0;
        assert!(layout_layered(&mut cards, &edges, FlowDirection::TopDown, &cfg));
        let y0: Vec<f32> = cards.iter().map(|c| c.y).collect();
        let mut cards2 = vec![card("a"), card("b"), card("c")];
        cfg.transpose_passes = 10;
        layout_layered(&mut cards2, &edges, FlowDirection::TopDown, &cfg);
        let y1: Vec<f32> = cards2.iter().map(|c| c.y).collect();
        assert_eq!(y0, y1, "clean chain unaffected by transpose count");
    }

    #[test]
    fn multilayer_edge_gets_routing_room() {
        // a->b->c->d chain plus a long edge a->d spanning 3 layers. Without
        // dummies the a->d edge would pass straight through b/c's column; with
        // dummies, intermediate layers reserve cross-axis space so b and c are
        // pushed off the a→d line. Verify b (layer 1) is offset from the
        // straight a→d vertical (i.e. cross-axis spread > 0 at the middle).
        let mut cards = vec![card("a"), card("b"), card("c"), card("d")];
        let edges = vec![edge("a","b"), edge("b","c"), edge("c","d"), edge("a","d")];
        assert!(layout_layered(&mut cards, &edges, FlowDirection::TopDown, &LayeredConfig::default()));
        // layers should be a<b<c<d in y
        assert!(cards[0].y < cards[1].y && cards[1].y < cards[2].y && cards[2].y < cards[3].y);
        // the dummy for a→d occupies layer 1 & 2, so b and c are shifted off
        // a's x (the chain isn't a single straight column).
        let spread = (cards[1].x - cards[0].x).abs() + (cards[2].x - cards[0].x).abs();
        assert!(spread > 0.0, "multi-layer edge should push chain off-axis (spread={})", spread);
    }

    #[test]
    fn leaf_source_docks_beside_successor() {
        // a -> b -> c (chain), plus a control signal sig --> b. sig is a
        // leaf-source (no in-edge, one out-edge) so it should dock at b's
        // layer-1, NOT pile at the top with a.
        let mut cards = vec![card("a"), card("b"), card("c"), card("sig")];
        let edges = vec![edge("a","b"), edge("b","c"), edge("sig","b")];
        layout_layered(&mut cards, &edges, FlowDirection::TopDown, &LayeredConfig::default());
        // a is the true root (layer 0); sig docks to b's layer-1 == a's layer,
        // so sig.y should be near a.y, and both strictly above b.
        assert!((cards[3].y - cards[0].y).abs() < 0.01, "sig should dock to a's layer");
        assert!(cards[3].y < cards[1].y, "sig above its successor b");
    }

    #[test]
    fn diamond_no_overlap_within_layer() {
        // a -> b, a -> c, b -> d, c -> d : classic diamond. b,c same layer must
        // not overlap (their x ranges shouldn't intersect given half_w + gap).
        let mut cards = vec![card("a"), card("b"), card("c"), card("d")];
        let edges = vec![edge("a","b"), edge("a","c"), edge("b","d"), edge("c","d")];
        layout_layered(&mut cards, &edges, FlowDirection::TopDown, &LayeredConfig::default());
        let (b, c) = (&cards[1], &cards[2]);
        let gap = (b.x - c.x).abs();
        assert!(gap >= b.half_w + c.half_w, "siblings overlap: gap {} < {}", gap, b.half_w + c.half_w);
        // d is below both b and c
        assert!(cards[3].y > b.y && cards[3].y > c.y);
    }

    /// v0.1.759: group contiguity — members of a supplied group end up adjacent
    /// within their layer, so their bounding box is tight (no non-member wedged
    /// between them).
    #[test]
    fn group_members_stay_contiguous_in_layer() {
        // root fans out to four siblings in one layer; g={s1,s3} are a group.
        // Without contiguity the median order could interleave s2 between them.
        let mut cards = vec![card("r"), card("s1"), card("s2"), card("s3"), card("s4")];
        let edges = vec![edge("r","s1"), edge("r","s2"), edge("r","s3"), edge("r","s4")];
        let groups = vec![vec!["s1".to_string(), "s3".to_string()]];
        layout_layered_grouped(&mut cards, &edges, FlowDirection::TopDown,
            &LayeredConfig::default(), &groups);
        // s1 and s3 should be horizontally adjacent: no other sibling's center
        // x lies strictly between them.
        let x = |id: &str| cards.iter().find(|c| c.id == id).unwrap().x;
        let (x1, x3) = (x("s1"), x("s3"));
        let (lo, hi) = (x1.min(x3), x1.max(x3));
        for other in ["s2", "s4"] {
            let xo = x(other);
            assert!(!(xo > lo && xo < hi),
                "non-member {} (x={}) wedged between group members [{}, {}]", other, xo, lo, hi);
        }
    }

    /// Contiguity must not change a layout that has no groups (regression).
    #[test]
    fn no_groups_identical_to_ungrouped() {
        let mut a = vec![card("x"), card("y"), card("z")];
        let mut b = a.clone();
        let edges = vec![edge("x","y"), edge("y","z")];
        layout_layered(&mut a, &edges, FlowDirection::TopDown, &LayeredConfig::default());
        layout_layered_grouped(&mut b, &edges, FlowDirection::TopDown,
            &LayeredConfig::default(), &[]);
        for (ca, cb) in a.iter().zip(b.iter()) {
            assert!((ca.x - cb.x).abs() < 0.01 && (ca.y - cb.y).abs() < 0.01,
                "grouped(empty) diverged from ungrouped at {}", ca.id);
        }
    }

    /// v0.1.760: fully-disconnected nodes should NOT all pile at layer 0 — they
    /// spread to the least-occupied layers so layer occupancy stays balanced.
    #[test]
    fn disconnected_nodes_distribute_not_pile() {
        // a connected chain a->b->c (3 layers) plus 3 isolated nodes.
        let mut cards = vec![card("a"), card("b"), card("c"),
                             card("i1"), card("i2"), card("i3")];
        let edges = vec![edge("a","b"), edge("b","c")];
        layout_layered(&mut cards, &edges, FlowDirection::TopDown, &LayeredConfig::default());
        // the three isolated nodes must not all share one y (layer). With 3
        // layers available and least-occupied placement, they should land on
        // distinct layers.
        let ys: Vec<f32> = ["i1","i2","i3"].iter()
            .map(|id| cards.iter().find(|c| &c.id == id).unwrap().y).collect();
        let distinct = {
            let mut v = ys.clone();
            v.sort_by(|a,b| a.partial_cmp(b).unwrap());
            v.dedup_by(|a,b| (*a-*b).abs() < 1.0);
            v.len()
        };
        assert!(distinct >= 2, "isolated nodes piled on the same layer: {:?}", ys);
    }

    /// v0.1.761: classify_back_edges flags the feedback edge in a cycle.
    #[test]
    fn classify_back_edges_finds_feedback() {
        // a -> b -> c -> a : the c->a edge closes the cycle (back edge).
        let cards = vec![card("a"), card("b"), card("c")];
        let edges = vec![edge("a","b"), edge("b","c"), edge("c","a")];
        let back = classify_back_edges(&cards, &edges);
        assert!(back.contains(&("c".to_string(), "a".to_string())),
            "expected c->a flagged as back edge, got {:?}", back);
        // the forward edges are NOT back edges
        assert!(!back.contains(&("a".to_string(), "b".to_string())));
        assert!(!back.contains(&("b".to_string(), "c".to_string())));
    }

    #[test]
    fn classify_back_edges_empty_for_dag() {
        let cards = vec![card("a"), card("b"), card("c")];
        let edges = vec![edge("a","b"), edge("a","c"), edge("b","c")];
        assert!(classify_back_edges(&cards, &edges).is_empty(), "a DAG has no back edges");
    }
}
