//! layered_graph — Sugiyama layering from `layered_layout.rs`.
//!
//! ## Why this exists rather than force-directed placement
//!
//! The original says it plainly: *"Force-directed placement scatters directed
//! flowcharts — the dogfood MIPS-datapath diagram overlapped badly while a
//! mature mermaid renderer laid the same graph out as clean top-to-bottom
//! layers."*
//!
//! That is a real comparison against a known-good renderer on a real diagram,
//! not a preference. A force simulation optimises for *even spacing*, which is
//! the wrong objective for a graph whose edges mean **direction** — it will
//! happily place a consumer above its producer because the springs balance
//! there.
//!
//! ## Three phases, and the order is the algorithm
//!
//! 1. **Layer** along edge direction — longest path, so every edge points down.
//! 2. **Order** within each layer to reduce crossings — median heuristic,
//!    swept both ways.
//! 3. **Place** by (layer depth, within-layer slot).
//!
//! Skipping phase 2 gives correct layers and unreadable output; it is where
//! the readability actually comes from.
//!
//! ## Cycles are found before layering, not survived during it
//!
//! A cycle has no longest path, so layering a graph containing one either
//! loops forever or silently picks an arbitrary break. The original finds back
//! edges with an **iterative** DFS — its comment says *"to avoid blowing the
//! stack on large graphs"* — and reverses them for layering only. Recursion
//! here is a crash on a diagram somebody generated rather than drew.

use std::collections::HashSet;

/// Which way the graph flows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FlowDirection { Down, Right }

/// Spacing, added to node half-extents so large cards do not collide.
///
/// Tuned for the ~10–60 node flowcharts this targets — stated because it is a
/// range, not a universal: a 500-node graph wants different numbers and this
/// module is not the thing to reach for there.
pub const LAYER_GAP: f32 = 90.0;
pub const SLOT_GAP: f32 = 40.0;
/// Half-extent used for a routing dummy — small, because it is a bend in an
/// edge rather than a thing to look at.
pub const DUMMY_EXT: f32 = 5.0;

/// Sweeps of the median heuristic. Even, so the last pass runs upward and the
/// two directions get equal say.
pub const ORDER_SWEEPS: usize = 4;

/// Find edges that point back into the DFS stack — the ones that make cycles.
///
/// Iterative by necessity: recursion blows the stack on a generated graph, and
/// a layout crash is worse than a slightly worse layout.
pub fn back_edges(succ: &[Vec<usize>]) -> HashSet<(usize, usize)> {
    let n = succ.len();
    let mut back = HashSet::new();
    // 0 = unvisited, 1 = on stack, 2 = done
    let mut state = vec![0u8; n];
    for start in 0..n {
        if state[start] != 0 { continue; }
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        state[start] = 1;
        while let Some(&(u, ci)) = stack.last() {
            if ci < succ[u].len() {
                stack.last_mut().unwrap().1 += 1;
                let v = succ[u][ci];
                match state[v] {
                    1 => { back.insert((u, v)); }
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

/// Assign each node a layer by longest path from a source.
///
/// Longest rather than shortest: with shortest paths a node sits directly under
/// its *earliest* predecessor, so a long edge crosses several layers and the
/// diagram reads as if that dependency were shorter than it is.
///
/// Back edges are excluded, so a cycle cannot make this diverge.
pub fn assign_layers(succ: &[Vec<usize>], back: &HashSet<(usize, usize)>) -> Vec<usize> {
    let n = succ.len();
    let mut indeg = vec![0usize; n];
    for (u, vs) in succ.iter().enumerate() {
        for &v in vs {
            if !back.contains(&(u, v)) { indeg[v] += 1; }
        }
    }
    let mut layer = vec![0usize; n];
    let mut queue: Vec<usize> = (0..n).filter(|i| indeg[*i] == 0).collect();
    // A graph that is entirely a cycle has no zero-indegree node even after
    // back edges are removed only if the removal missed one; seed node 0 so
    // layout still produces something rather than an empty diagram.
    if queue.is_empty() && n > 0 { queue.push(0); }
    let mut head = 0;
    while head < queue.len() {
        let u = queue[head];
        head += 1;
        for &v in &succ[u] {
            if back.contains(&(u, v)) { continue; }
            if layer[v] < layer[u] + 1 { layer[v] = layer[u] + 1; }
            indeg[v] -= 1;
            if indeg[v] == 0 { queue.push(v); }
        }
    }
    layer
}

/// Group nodes into layers, preserving index order as the initial ordering.
pub fn layers_of(layer: &[usize]) -> Vec<Vec<usize>> {
    let depth = layer.iter().copied().max().map(|m| m + 1).unwrap_or(0);
    let mut out = vec![Vec::new(); depth];
    for (i, l) in layer.iter().enumerate() { out[*l].push(i); }
    out
}

/// Count edge crossings between two adjacent layers.
///
/// The objective phase 2 minimises, exposed so a test can assert the ordering
/// actually improves it rather than merely changing it.
pub fn crossings(upper: &[usize], lower: &[usize], succ: &[Vec<usize>]) -> usize {
    let slot = |layer: &[usize], n: usize| layer.iter().position(|x| *x == n);
    let mut edges: Vec<(usize, usize)> = Vec::new();
    for (ui, u) in upper.iter().enumerate() {
        for &v in &succ[*u] {
            if let Some(vi) = slot(lower, v) { edges.push((ui, vi)); }
        }
    }
    let mut c = 0;
    for i in 0..edges.len() {
        for j in i + 1..edges.len() {
            let (a, b) = (edges[i], edges[j]);
            // Two edges cross when their endpoints are in opposite order.
            if (a.0 < b.0 && a.1 > b.1) || (a.0 > b.0 && a.1 < b.1) { c += 1; }
        }
    }
    c
}

/// Median of a node's neighbours' slots, or `None` when it has none.
///
/// `None` rather than 0: a node with no neighbours in the adjacent layer has no
/// opinion about where it belongs, and giving it 0 would drag every such node
/// to the left edge and pile them there.
pub fn median(neigh_slots: &[usize]) -> Option<f32> {
    if neigh_slots.is_empty() { return None; }
    let mut v: Vec<usize> = neigh_slots.to_vec();
    v.sort_unstable();
    let m = v.len() / 2;
    Some(if v.len() % 2 == 1 { v[m] as f32 } else { (v[m - 1] + v[m]) as f32 / 2.0 })
}

/// One ordering sweep. `down` means order each layer by its predecessors.
///
/// Nodes with no opinion keep their current position rather than being sorted
/// to one end — a stable sort on `Option`, with `None` held in place.
pub fn sweep(order: &mut [Vec<usize>], succ: &[Vec<usize>], pred: &[Vec<usize>], down: bool) {
    let n_layers = order.len();
    let range: Vec<usize> = if down { (1..n_layers).collect() }
                            else { (0..n_layers.saturating_sub(1)).rev().collect() };
    for li in range {
        let neighbour_layer = if down { li - 1 } else { li + 1 };
        let slot_of: Vec<Option<usize>> = {
            let nl = &order[neighbour_layer];
            let mut map = vec![None; succ.len()];
            for (i, x) in nl.iter().enumerate() { map[*x] = Some(i); }
            map
        };
        let keyed: Vec<(usize, Option<f32>)> = order[li].iter().map(|&node| {
            let neigh = if down { &pred[node] } else { &succ[node] };
            let slots: Vec<usize> = neigh.iter().filter_map(|x| slot_of[*x]).collect();
            (node, median(&slots))
        }).collect();
        // Stable: equal keys and no-opinion nodes keep their relative order, so
        // a sweep that changes nothing leaves the layout byte-identical.
        let mut with_key: Vec<(usize, Option<f32>)> = keyed;
        with_key.sort_by(|a, b| match (a.1, b.1) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
            _ => std::cmp::Ordering::Equal,
        });
        order[li] = with_key.into_iter().map(|(n, _)| n).collect();
    }
}

/// A placed node.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Placed { pub x: f32, pub y: f32 }

/// Place ordered layers into coordinates.
///
/// Screen coordinates with +y down, matching the rest of the layout code, and
/// honouring the flow direction — a Right-flowing graph swaps the axes rather
/// than rotating afterwards, because rotating would put the text sideways too.
pub fn place(order: &[Vec<usize>], n: usize, dir: FlowDirection) -> Vec<Placed> {
    let mut out = vec![Placed { x: 0.0, y: 0.0 }; n];
    for (li, layer) in order.iter().enumerate() {
        // Centre each layer, so a narrow layer sits under the middle of a wide
        // one rather than left-aligned against it.
        let width = layer.len() as f32;
        for (si, &node) in layer.iter().enumerate() {
            let along = li as f32 * LAYER_GAP;
            let across = (si as f32 - (width - 1.0) / 2.0) * SLOT_GAP;
            out[node] = match dir {
                FlowDirection::Down => Placed { x: across, y: along },
                FlowDirection::Right => Placed { x: along, y: across },
            };
        }
    }
    out
}

/// The whole pipeline.
pub fn layout(succ: &[Vec<usize>], dir: FlowDirection) -> Vec<Placed> {
    let n = succ.len();
    if n == 0 { return Vec::new(); }
    let back = back_edges(succ);
    let layer = assign_layers(succ, &back);
    let mut order = layers_of(&layer);
    let mut pred: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (u, vs) in succ.iter().enumerate() {
        for &v in vs { if !back.contains(&(u, v)) { pred[v].push(u); } }
    }
    for s in 0..ORDER_SWEEPS { sweep(&mut order, succ, &pred, s % 2 == 0); }
    place(&order, n, dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape force-directed layout gets wrong: a diamond.
    fn diamond() -> Vec<Vec<usize>> {
        vec![vec![1, 2], vec![3], vec![3], vec![]]
    }

    #[test]
    fn every_edge_points_down_the_layers() {
        // The property a force simulation does not give you: it optimises for
        // even spacing and will happily place a consumer above its producer
        // because the springs balance there.
        let g = diamond();
        let layer = assign_layers(&g, &back_edges(&g));
        for (u, vs) in g.iter().enumerate() {
            for &v in vs { assert!(layer[v] > layer[u], "{u}->{v} points up or sideways"); }
        }
    }

    #[test]
    fn a_long_edge_lands_at_its_deepest_predecessor() {
        // Longest path, not shortest: with shortest paths a node sits under its
        // EARLIEST predecessor, so the long edge crosses several layers and the
        // diagram reads as if that dependency were shorter than it is.
        //  0 -> 1 -> 2 -> 3, and 0 -> 3 directly
        let g = vec![vec![1, 3], vec![2], vec![3], vec![]];
        let layer = assign_layers(&g, &back_edges(&g));
        assert_eq!(layer[3], 3, "node 3 hopped up to the short path's depth");
    }

    #[test]
    fn a_cycle_is_broken_rather_than_looped_on() {
        // A cycle has no longest path; layering one either loops forever or
        // silently picks an arbitrary break.
        let g = vec![vec![1], vec![2], vec![0]];
        let back = back_edges(&g);
        assert_eq!(back.len(), 1, "found {back:?}");
        let layer = assign_layers(&g, &back);
        assert!(layer.iter().all(|l| *l < 3));
    }

    #[test]
    fn a_self_loop_does_not_hang_the_layering() {
        // Generated graphs contain them, and a self-loop is a cycle of one.
        let g = vec![vec![0, 1], vec![]];
        let back = back_edges(&g);
        assert!(back.contains(&(0, 0)));
        let layer = assign_layers(&g, &back);
        assert_eq!(layer[1], 1);
    }

    #[test]
    fn back_edge_detection_does_not_recurse() {
        // "To avoid blowing the stack on large graphs" — a layout crash is
        // worse than a slightly worse layout. A 20k-node chain would overflow
        // a recursive DFS.
        let n = 20_000;
        let g: Vec<Vec<usize>> = (0..n).map(|i| if i + 1 < n { vec![i + 1] } else { vec![] }).collect();
        assert!(back_edges(&g).is_empty(), "a straight chain has no back edges");
    }

    #[test]
    fn ordering_reduces_crossings_rather_than_merely_changing_them() {
        // Phase 2 is where readability comes from; skipping it gives correct
        // layers and unreadable output.
        //   0,1 on top; 2,3 below; edges 0->3 and 1->2 cross as ordered
        let g = vec![vec![3], vec![2], vec![], vec![]];
        let mut order = vec![vec![0, 1], vec![2, 3]];
        let before = crossings(&order[0], &order[1], &g);
        assert_eq!(before, 1, "the fixture should start crossed");
        let pred = vec![vec![], vec![], vec![1], vec![0]];
        for s in 0..ORDER_SWEEPS { sweep(&mut order, &g, &pred, s % 2 == 0); }
        assert_eq!(crossings(&order[0], &order[1], &g), 0, "still crossed: {order:?}");
    }

    #[test]
    fn a_node_with_no_neighbours_keeps_its_place_rather_than_piling_left() {
        // Giving it a median of 0 would drag every such node to the left edge
        // and stack them there.
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[4]), Some(4.0));
        assert_eq!(median(&[1, 3]), Some(2.0));
        assert_eq!(median(&[1, 2, 9]), Some(2.0), "median, not mean");
    }

    #[test]
    fn a_sweep_that_changes_nothing_leaves_the_order_identical() {
        // Stability matters: an unstable sort makes a redraw reshuffle nodes
        // that had no reason to move, and the diagram appears to twitch.
        let g = vec![vec![2], vec![3], vec![], vec![]];
        let pred = vec![vec![], vec![], vec![0], vec![1]];
        let mut order = vec![vec![0, 1], vec![2, 3]];
        let snapshot = order.clone();
        for s in 0..ORDER_SWEEPS { sweep(&mut order, &g, &pred, s % 2 == 0); }
        assert_eq!(order, snapshot);
    }

    #[test]
    fn layers_are_centred_so_a_narrow_layer_sits_under_a_wide_one() {
        // Left-aligning would put a single output node under the leftmost
        // input, implying a relationship that is not there.
        let order = vec![vec![0, 1, 2], vec![3]];
        let p = place(&order, 4, FlowDirection::Down);
        let mid = (p[0].x + p[2].x) / 2.0;
        assert!((p[3].x - mid).abs() < 1e-5, "not centred: {} vs {mid}", p[3].x);
    }

    #[test]
    fn flow_direction_swaps_the_axes_rather_than_rotating() {
        // Rotating afterwards would put the text sideways too.
        let order = vec![vec![0], vec![1]];
        let down = place(&order, 2, FlowDirection::Down);
        let right = place(&order, 2, FlowDirection::Right);
        assert!(down[1].y > down[0].y && (down[1].x - down[0].x).abs() < 1e-5);
        assert!(right[1].x > right[0].x && (right[1].y - right[0].y).abs() < 1e-5);
    }

    #[test]
    fn an_empty_graph_lays_out_to_nothing() {
        assert!(layout(&[], FlowDirection::Down).is_empty());
        assert!(layers_of(&[]).is_empty());
    }

    #[test]
    fn a_disconnected_graph_still_places_every_node() {
        // Two unrelated flowcharts in one diagram is ordinary, and dropping the
        // second component would lose half the picture.
        let g = vec![vec![1], vec![], vec![3], vec![]];
        let p = layout(&g, FlowDirection::Down);
        assert_eq!(p.len(), 4);
        assert!(p.iter().all(|q| q.x.is_finite() && q.y.is_finite()));
    }

    #[test]
    fn the_diamond_reads_as_a_diamond() {
        // The end-to-end shape: one source, two branches at the same depth,
        // one join below them.
        let p = layout(&diamond(), FlowDirection::Down);
        assert!(p[1].y > p[0].y);
        assert!((p[1].y - p[2].y).abs() < 1e-5, "branches at different depths");
        assert!(p[3].y > p[1].y);
        assert!((p[1].x - p[2].x).abs() > 1.0, "branches on top of each other");
    }
}
