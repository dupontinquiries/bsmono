// src/graph_layout.rs
//
// v0.1.317: Foundation of the "Mermaid plus + cards graph view" arc.

// This module is a foundation ship — its public API is staged for
// consumption by future ships (Backend integration + QML rendering).
// Until those land, `cargo build` would warn about unused items.
// Silence at the module level so we don't pollute output during
// the multi-ship arc.
#![allow(dead_code)]

//
// This module owns one job: take a set of nodes (cards) and edges,
// and produce 2D positions. Parser-agnostic — doesn't care whether
// the graph came from connections.rs (the existing data spine) or
// from a future Mermaid+ parser. Both feed (node_ids, edges) and
// get positions back.
//
// Why "cards" terminology
//
// Each node in the user's vision is a card with title + preview +
// thumbnail (per connections::Node). Cards have non-trivial width
// and height — different from classic node-link diagrams where
// nodes are point-sized. The layout has to account for collision
// between rectangles, not circles.
//
// Approach: Fruchterman-Reingold force-directed (well-trodden,
// cheap, looks decent) with three modifications:
//
//   1. Per-pair repulsion uses card half-extents (rectangle
//      overlap → infinite repulsion in that direction; otherwise
//      standard 1/r^2 falloff). Keeps cards from overlapping at
//      rest.
//
//   2. Pinned nodes don't update. User-positioned cards stay
//      put; the rest reflow around them. This is the "partial
//      constraint" the user asked for in 2.7.2.
//
//   3. Light center-gravity term so disconnected components
//      don't drift to infinity in finite iteration budgets.
//
// What this is NOT:
//
//   - Not perfect graph drawing. Reference impls of FR are
//     ~thousands of lines; this is ~250. Good enough for
//     interactive use.
//   - Not incremental. Each call recomputes from current state.
//     A future ship will add per-frame "small steps" for
//     drag-with-reflow.
//   - Not constraint-solving (Cassowary etc). That's reserved
//     for explicit alignment ("A and B horizontal at y=100"),
//     which the user mentioned but isn't blocking v1.
//
// Design seam: this module knows nothing about QML, Backend,
// or rendering. It's `&mut [CardPos]` in, `&mut [CardPos]` out.
// The Backend will own a (graph snapshot + card positions) pair
// and feed it to QML; the layout engine is just the math.

/// Position + identity of one card. The id is opaque (caller's
/// responsibility to match against their own graph).
#[derive(Clone, Debug, PartialEq)]
pub struct CardPos {
    /// Stable identifier. Must be unique within a layout run.
    pub id: String,
    /// Current x position (centroid of the card rectangle).
    pub x: f32,
    /// Current y position (centroid).
    pub y: f32,
    /// User-pinned: layout doesn't move this node.
    pub pinned: bool,
    /// Half-width for collision/edge-routing (centroid → edge).
    pub half_w: f32,
    /// Half-height for collision/edge-routing.
    pub half_h: f32,
}

impl CardPos {
    /// Construct a card at the origin with a default size hint.
    /// Caller usually sets x/y immediately afterward.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            x: 0.0,
            y: 0.0,
            pinned: false,
            half_w: 80.0,
            half_h: 50.0,
        }
    }

    /// Convenience: pin at a specific position.
    pub fn pinned_at(id: impl Into<String>, x: f32, y: f32) -> Self {
        Self {
            id: id.into(),
            x,
            y,
            pinned: true,
            half_w: 80.0,
            half_h: 50.0,
        }
    }
}

/// An edge for layout purposes. No styling info — just connectivity.
/// Direction is irrelevant for force calculations (attraction is
/// symmetric).
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutEdge {
    pub from: String,
    pub to: String,
}

impl LayoutEdge {
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self { from: from.into(), to: to.into() }
    }
}

/// Directional flow hint for the layout, mirroring Mermaid's
/// graph direction declarations (TD/TB, BT, LR, RL).
///
/// v0.1.464 (niftyplot directional bias): the force-directed
/// layout is inherently undirected — attraction along an edge is
/// symmetric, so `graph TD` and a freeform mindmap produced
/// identical (direction-ignoring) layouts even though the parser
/// knew the declared direction. This enum lets the caller pass
/// that intent through. When set to anything other than `None`,
/// the attraction term gains a directional bias that nudges each
/// edge's `to` node toward the flow direction relative to its
/// `from` node — e.g. TopDown pulls children below parents.
///
/// This is deliberately a *bias on the existing force model*, not
/// a separate layered (Sugiyama) engine: it reuses the whole
/// tested force loop and adds one directional term, so directed
/// and undirected graphs share a single code path. It produces a
/// "flows downward/rightward" feel rather than strict ranked
/// rows — which matches the interactive, organic card-graph look
/// the rest of this module targets. Strict layered ranking is a
/// possible future ship if users want pixel-aligned rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowDirection {
    /// No directional bias — pure force-directed (the default;
    /// freeform mindmaps, connections graph).
    None,
    /// Top-down: `to` nodes pulled below `from` nodes (Mermaid
    /// TD / TB).
    TopDown,
    /// Bottom-up: `to` nodes pulled above `from` nodes (BT).
    BottomUp,
    /// Left-to-right: `to` nodes pulled right of `from` (LR).
    LeftRight,
    /// Right-to-left: `to` nodes pulled left of `from` (RL).
    RightLeft,
}

impl FlowDirection {
    /// Parse a Mermaid direction token (case-insensitive).
    /// Recognizes TD, TB, BT, LR, RL. Anything else → None.
    pub fn from_mermaid(token: &str) -> FlowDirection {
        match token.trim().to_ascii_uppercase().as_str() {
            "TD" | "TB" => FlowDirection::TopDown,
            "BT" => FlowDirection::BottomUp,
            "LR" => FlowDirection::LeftRight,
            "RL" => FlowDirection::RightLeft,
            _ => FlowDirection::None,
        }
    }

    /// The unit (dx, dy) the `to` node should be biased toward
    /// relative to its `from` node. (0,0) for None.
    /// Screen coords: +y is down, +x is right.
    pub fn bias_vector(&self) -> (f32, f32) {
        match self {
            FlowDirection::None => (0.0, 0.0),
            FlowDirection::TopDown => (0.0, 1.0),
            FlowDirection::BottomUp => (0.0, -1.0),
            FlowDirection::LeftRight => (1.0, 0.0),
            FlowDirection::RightLeft => (-1.0, 0.0),
        }
    }

    /// Whether this direction applies any bias at all.
    pub fn is_directional(&self) -> bool {
        !matches!(self, FlowDirection::None)
    }
}

/// Tunable parameters for the layout. Defaults work for ~10-50
/// card graphs (the user's expected scale for connections + most
/// hand-drawn diagrams).
#[derive(Clone, Debug)]
pub struct LayoutConfig {
    /// Canvas width — used for center-gravity and seed-grid.
    pub width: f32,
    /// Canvas height — used for center-gravity and seed-grid.
    pub height: f32,
    /// Maximum iterations of the force loop. Caps wall time so
    /// pathological graphs don't burn CPU.
    pub iterations: u32,
    /// Rest length of an edge — force pulls endpoints toward this
    /// distance.
    pub target_edge_length: f32,
    /// Multiplier on the 1/r^2 repulsion. Higher = nodes push
    /// apart harder.
    pub repulsion_strength: f32,
    /// Multiplier on edge attraction. Higher = connected nodes
    /// pull together more.
    pub attraction_strength: f32,
    /// Velocity damping per step (0..1). Prevents oscillation.
    pub damping: f32,
    /// Light pull toward canvas center. Keeps disconnected
    /// components from drifting off-screen.
    pub center_gravity: f32,
    /// Maximum per-step movement (pixels). Stabilizes early
    /// iterations when forces are huge.
    pub max_step: f32,
    /// v0.1.464: directional flow hint. None = pure
    /// force-directed (default). Set from the parsed Mermaid
    /// graph direction to bias the layout into a flow.
    pub flow_direction: FlowDirection,
    /// v0.1.464: strength of the directional bias, as a fraction
    /// of target_edge_length applied per edge per step. 0 = off
    /// even if flow_direction is set. Tuned so the flow is clearly
    /// visible without overpowering collision/repulsion (which
    /// still prevents overlap). Only consulted when
    /// flow_direction.is_directional().
    pub flow_bias_strength: f32,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            width: 800.0,
            height: 600.0,
            iterations: 200,
            target_edge_length: 180.0,
            repulsion_strength: 15000.0,
            attraction_strength: 0.05,
            damping: 0.85,
            center_gravity: 0.005,
            max_step: 30.0,
            // v0.1.464: default to undirected (existing behavior
            // exactly — freeform/mindmap/connections graphs).
            flow_direction: FlowDirection::None,
            // A modest pull. At 0.08 of edge length per step it
            // accumulates into a clear directional flow over the
            // iteration budget without ejecting nodes or beating
            // collision repulsion. No effect unless flow_direction
            // is set.
            flow_bias_strength: 0.08,
        }
    }
}

impl LayoutConfig {
    /// v0.1.399 (mindmap-crowding fix): adapt layout
    /// parameters to graph size. User feedback on mindmaps:
    /// "a bit crowded" with overlaps. Root cause: fixed
    /// center_gravity + fixed canvas pulls dense graphs into
    /// a crunch. The single-pass force model has a natural
    /// equilibrium density that's fine for ~10 nodes but
    /// overcrowds at 30+.
    ///
    /// Heuristic, applied in-place:
    ///   - Canvas grows with sqrt(n): more nodes → more room
    ///     to spread, so the seed grid and center target
    ///     scale up.
    ///   - center_gravity shrinks as n grows: a strong pull
    ///     is needed to corral a few drifting components, but
    ///     becomes a crushing force on a large graph. Inverse
    ///     relationship keeps big graphs loose.
    ///   - repulsion grows mildly with n: more neighbors
    ///     means each node needs to push a bit harder to
    ///     claim its space.
    ///   - target_edge_length grows slightly: longer edges
    ///     in big graphs reduce the "hairball" effect.
    ///   - iterations grow with n (capped): bigger graphs
    ///     need more steps to settle, but we cap to bound
    ///     wall time.
    ///
    /// Tuned to be a no-op-ish identity around n=12 (the
    /// size where the original constants were chosen) so
    /// small graphs render exactly as before.
    ///
    /// v0.1.399: scales are applied MULTIPLICATIVELY to the
    /// current field values rather than to hardcoded
    /// constants, so a caller that set mermaid-specific
    /// base values (shorter edges, more iterations) keeps
    /// those as the baseline that gets scaled. At n≈12 the
    /// multipliers are ~1.0 so the caller's values pass
    /// through unchanged.
    pub fn adapt_to_graph(&mut self, node_count: usize, edge_count: usize) {
        let n = node_count.max(1) as f32;
        // Reference size where original constants were tuned.
        let n_ref = 12.0f32;
        let scale = (n / n_ref).sqrt().max(1.0);

        // Canvas grows with sqrt(n). At n=12, scale≈1 → no
        // change. At n=48, scale=2 → double canvas. Canvas
        // is absolute (not multiplicative) since seed-grid
        // and center depend on it directly.
        self.width = 800.0 * scale;
        self.height = 600.0 * scale;

        // center_gravity ~ 1/scale. Looser for big graphs.
        // Multiplicative so a caller override is respected.
        self.center_gravity /= scale;

        // Repulsion grows mildly (sqrt of scale) so nodes
        // claim more space without ejecting.
        self.repulsion_strength *= scale.sqrt();

        // Edges a touch longer in big graphs.
        self.target_edge_length *= 1.0 + (scale - 1.0) * 0.3;

        // More iterations for bigger graphs, capped at 400.
        self.iterations = (((self.iterations as f32) * scale) as u32).min(400);

        // v0.1.412: dense graphs (high edge:node ratio) were
        // CROWDING — user reported "spacing is still a bit tight
        // on highly-connected nodes." The previous version
        // bumped ATTRACTION on dense graphs (×1.4), which pulls
        // connected nodes closer and makes the crowding WORSE.
        // The crowding cause is that a node with many edges gets
        // pulled toward all its neighbors at once, collapsing
        // into a tight hub. The fix: on dense graphs, give nodes
        // MORE personal space — boost repulsion and edge length
        // so highly-connected hubs push their neighbors out to a
        // readable radius instead of clumping. A small attraction
        // bump is kept so edges don't stretch absurdly, but it's
        // far gentler than before (×1.1, was ×1.4).
        let density = edge_count as f32 / n;
        if density > 1.5 {
            // Scale the spread with how dense it is, capped so a
            // pathologically dense graph doesn't explode the
            // canvas. density 1.5 → ~1.0 (no change at the
            // threshold); density 3 → ~1.3; clamped at 1.5.
            let spread = (1.0 + (density - 1.5) * 0.2).min(1.5);
            self.repulsion_strength *= spread;
            self.target_edge_length *= spread;
            // Gentle attraction nudge so edges still read as
            // connections, but not enough to re-collapse the hub.
            self.attraction_strength *= 1.1;
        }
    }
}

/// Seed cards on a grid based on canvas dimensions. Used before
/// the force loop to give an initial layout that's better than
/// all-at-origin (which would cause divide-by-zero in repulsion).
///
/// Pinned cards are left alone; only unpinned cards move.
pub fn seed_grid(cards: &mut [CardPos], cfg: &LayoutConfig) {
    let n = cards.iter().filter(|c| !c.pinned).count();
    if n == 0 { return; }
    // Square-ish grid: cols = ceil(sqrt(n))
    let cols = (n as f32).sqrt().ceil().max(1.0) as usize;
    let rows = ((n + cols - 1) / cols).max(1);
    let cell_w = cfg.width / (cols as f32 + 1.0);
    let cell_h = cfg.height / (rows as f32 + 1.0);
    let mut idx = 0usize;
    for c in cards.iter_mut() {
        if c.pinned { continue; }
        let row = idx / cols;
        let col = idx % cols;
        c.x = (col as f32 + 1.0) * cell_w;
        c.y = (row as f32 + 1.0) * cell_h;
        idx += 1;
    }
}

/// Run force-directed layout in place. Pinned cards never move.
///
/// Algorithm: Fruchterman-Reingold with rectangle-aware repulsion.
/// Per iteration:
///   1. Compute repulsion between every pair (O(n^2), fine at n<200).
///   2. Compute attraction along each edge.
///   3. Add center-gravity.
///   4. Apply displacement, damped and capped.
///   5. Skip pinned nodes.
///
/// Convergence: no explicit termination — runs full iteration
/// budget. Step size shrinks naturally because forces balance.
/// For interactive drag-with-reflow, callers should run fewer
/// iterations per frame (say 5-10) and accumulate state.
pub fn layout_force_directed(
    cards: &mut [CardPos],
    edges: &[LayoutEdge],
    cfg: &LayoutConfig,
) {
    if cards.is_empty() { return; }
    let n = cards.len();

    // Precompute edge index pairs — O(E) lookup once vs O(E*N)
    // string compares per iteration.
    let mut edge_idx: Vec<(usize, usize)> = Vec::with_capacity(edges.len());
    for e in edges {
        let a = cards.iter().position(|c| c.id == e.from);
        let b = cards.iter().position(|c| c.id == e.to);
        if let (Some(a), Some(b)) = (a, b) {
            if a != b { edge_idx.push((a, b)); }
        }
    }

    // Per-iteration velocity buffer (also damping target).
    let mut vx = vec![0.0f32; n];
    let mut vy = vec![0.0f32; n];

    let cx = cfg.width * 0.5;
    let cy = cfg.height * 0.5;

    for _iter in 0..cfg.iterations {
        // Reset accumulators.
        for i in 0..n { vx[i] = 0.0; vy[i] = 0.0; }

        // Repulsion: every pair, push apart with 1/r^2 falloff
        // adjusted by rectangle half-extents (so larger cards
        // need more clearance).
        for i in 0..n {
            for j in (i + 1)..n {
                let mut dx = cards[i].x - cards[j].x;
                let mut dy = cards[i].y - cards[j].y;
                let mut dist2 = dx * dx + dy * dy;
                // v0.1.465: coincident-node symmetry break. When two
                // cards share (nearly) identical coords, dx=dy≈0, so
                // the repulsion UNIT vector below would be (0,0) — a
                // large force with no direction, leaving the pair
                // stacked forever (the bug behind
                // disconnected_cards_dont_overlap). Substitute a
                // deterministic tiny offset derived from the pair
                // indices so the push has a direction. Deterministic
                // (not random) keeps layouts reproducible across runs;
                // distinct per-pair angles avoid every coincident
                // cluster fanning out along one axis.
                if dist2 < 1e-6 {
                    // Spread pairs around the circle by index so a
                    // pile of coincident nodes pushes outward evenly.
                    let angle = ((i * 7 + j * 13) % 360) as f32
                        * std::f32::consts::PI / 180.0;
                    dx = angle.cos() * 0.01;
                    dy = angle.sin() * 0.01;
                    dist2 = dx * dx + dy * dy;
                }
                // Effective "rest distance" = sum of half-extents
                // along the axis of separation. Prevents overlap
                // at equilibrium.
                let min_dist = cards[i].half_w + cards[j].half_w
                    + cards[i].half_h + cards[j].half_h;
                let min_dist2 = min_dist * min_dist;
                // Effective squared distance — clamped above zero
                // and below min_dist^2 to avoid division blowups
                // when two cards happen to share coords.
                let eff = dist2.max(1.0).max(min_dist2 * 0.25);
                let force = cfg.repulsion_strength / eff;
                let dist = dist2.max(1.0).sqrt();
                let ux = dx / dist;
                let uy = dy / dist;
                vx[i] += ux * force;
                vy[i] += uy * force;
                vx[j] -= ux * force;
                vy[j] -= uy * force;
            }
        }

        // Attraction along edges. Hooke-style: force = k * (d - rest).
        for &(i, j) in &edge_idx {
            let dx = cards[j].x - cards[i].x;
            let dy = cards[j].y - cards[i].y;
            let dist = (dx * dx + dy * dy).sqrt().max(0.001);
            let delta = dist - cfg.target_edge_length;
            let force = cfg.attraction_strength * delta;
            let ux = dx / dist;
            let uy = dy / dist;
            vx[i] += ux * force;
            vy[i] += uy * force;
            vx[j] -= ux * force;
            vy[j] -= uy * force;
        }

        // v0.1.464: directional flow bias. When a flow direction
        // is set, nudge each edge's `to` node (j) toward the flow
        // direction relative to its `from` node (i), and push the
        // `from` node the opposite way — an antisymmetric pair so
        // the graph's centroid doesn't drift (center-gravity stays
        // meaningful). This is what makes `graph TD` actually flow
        // downward instead of laying out like an undirected blob.
        //
        // Magnitude: a fraction of target_edge_length, so it's
        // commensurate with the attraction rest-length and scales
        // with the configured graph size. Repulsion/collision still
        // dominate at close range, so nodes flow without overlapping.
        if cfg.flow_direction.is_directional() && cfg.flow_bias_strength > 0.0 {
            let (bx, by) = cfg.flow_direction.bias_vector();
            let push = cfg.flow_bias_strength * cfg.target_edge_length
                * cfg.attraction_strength;
            for &(i, j) in &edge_idx {
                // j (to) moves along the flow; i (from) opposite.
                vx[j] += bx * push;
                vy[j] += by * push;
                vx[i] -= bx * push;
                vy[i] -= by * push;
            }
        }

        // Center gravity — gentle pull toward (cx, cy). Linear
        // in distance, so far-away components return faster than
        // nearby ones drift.
        for i in 0..n {
            vx[i] += (cx - cards[i].x) * cfg.center_gravity;
            vy[i] += (cy - cards[i].y) * cfg.center_gravity;
        }

        // Apply displacement with damping + step cap.
        for i in 0..n {
            if cards[i].pinned { continue; }
            let mut sx = vx[i] * cfg.damping;
            let mut sy = vy[i] * cfg.damping;
            // Cap individual step to prevent ejection during
            // early iterations when forces are huge.
            let mag = (sx * sx + sy * sy).sqrt();
            if mag > cfg.max_step {
                let k = cfg.max_step / mag;
                sx *= k;
                sy *= k;
            }
            cards[i].x += sx;
            cards[i].y += sy;
        }
    }
}

/// Compute the bounding box of all cards. Useful for the caller
/// to set the QML canvas viewport. Returns (min_x, min_y, max_x,
/// max_y). Includes card extents (not just centroids).
///
/// Returns (0,0,0,0) for empty input.
pub fn bounding_box(cards: &[CardPos]) -> (f32, f32, f32, f32) {
    if cards.is_empty() { return (0.0, 0.0, 0.0, 0.0); }
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for c in cards {
        min_x = min_x.min(c.x - c.half_w);
        min_y = min_y.min(c.y - c.half_h);
        max_x = max_x.max(c.x + c.half_w);
        max_y = max_y.max(c.y + c.half_h);
    }
    (min_x, min_y, max_x, max_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_no_panic() {
        let mut cards: Vec<CardPos> = Vec::new();
        let edges: Vec<LayoutEdge> = Vec::new();
        let cfg = LayoutConfig::default();
        // Shouldn't panic on empty input.
        layout_force_directed(&mut cards, &edges, &cfg);
        seed_grid(&mut cards, &cfg);
        assert!(cards.is_empty());
    }

    #[test]
    fn single_card_unchanged_by_layout() {
        // Single card has nothing to repel/attract from; it
        // just gets pulled toward center by the (deliberately
        // gentle) center-gravity term.
        let mut cards = vec![CardPos {
            id: "a".into(), x: 100.0, y: 100.0,
            pinned: false, half_w: 80.0, half_h: 50.0,
        }];
        let cfg = LayoutConfig { iterations: 200, ..Default::default() };
        let start_dist = ((cfg.width * 0.5 - 100.0).powi(2)
            + (cfg.height * 0.5 - 100.0).powi(2)).sqrt();
        layout_force_directed(&mut cards, &[], &cfg);
        // v0.1.465: center-gravity is intentionally GENTLE
        // (center_gravity=0.005, damping=0.85), so a lone card
        // doesn't snap to center — it decays geometrically toward
        // it. Over 200 iters from 300px out it reaches ~57% of the
        // way (lands ~x=272 for the default 400 center). That is
        // CORRECT, designed behavior: a strong center-pull would
        // crush dense graphs (the thing adapt_to_graph guards
        // against). The prior test asserted arrival-within-50px,
        // which the gentle gravity can't satisfy in 200 iters — it
        // was an over-tight expectation, not a code bug (verified:
        // the geometric-decay math reproduces x=272 exactly).
        //
        // Assert the real contract instead: the card moves
        // SUBSTANTIALLY toward center (well past halfway) and never
        // overshoots past or drifts away.
        let cx = cfg.width * 0.5;
        let cy = cfg.height * 0.5;
        let end_dist = ((cx - cards[0].x).powi(2)
            + (cy - cards[0].y).powi(2)).sqrt();
        assert!(end_dist < start_dist * 0.6,
            "card should move well toward center: start_dist={}, \
             end_dist={}", start_dist, end_dist);
        // And it must be on the correct side (didn't overshoot the
        // center or move away).
        assert!(cards[0].x > 100.0 && cards[0].x <= cx,
            "card x={} should advance toward center {} without \
             overshoot", cards[0].x, cx);
    }

    #[test]
    fn pinned_card_does_not_move() {
        let mut cards = vec![
            CardPos::pinned_at("a", 100.0, 100.0),
            CardPos { id: "b".into(), x: 500.0, y: 500.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
        ];
        let cfg = LayoutConfig::default();
        layout_force_directed(&mut cards, &[], &cfg);
        // a is pinned — must be exactly at (100, 100).
        assert_eq!(cards[0].x, 100.0);
        assert_eq!(cards[0].y, 100.0);
    }

    #[test]
    fn connected_pair_settles_near_target_length() {
        // Two cards connected by one edge should settle with
        // centroids ~target_edge_length apart.
        let mut cards = vec![
            CardPos { id: "a".into(), x: 100.0, y: 300.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
            CardPos { id: "b".into(), x: 700.0, y: 300.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
        ];
        let edges = vec![LayoutEdge::new("a", "b")];
        let cfg = LayoutConfig {
            iterations: 500,
            target_edge_length: 200.0,
            ..Default::default()
        };
        layout_force_directed(&mut cards, &edges, &cfg);
        let dx = cards[1].x - cards[0].x;
        let dy = cards[1].y - cards[0].y;
        let dist = (dx * dx + dy * dy).sqrt();
        // Allow 50% slop — center-gravity and repulsion push
        // away from exact target_edge_length.
        assert!(dist > 100.0 && dist < 350.0,
            "expected ~200 with slop, got {}", dist);
    }

    #[test]
    fn disconnected_cards_dont_overlap() {
        // Three disconnected cards starting at the same position.
        // After layout they should be separated by at least the
        // sum of their half-extents.
        let mut cards = vec![
            CardPos { id: "a".into(), x: 400.0, y: 300.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
            CardPos { id: "b".into(), x: 400.0, y: 300.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
            CardPos { id: "c".into(), x: 400.0, y: 300.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
        ];
        let cfg = LayoutConfig::default();
        layout_force_directed(&mut cards, &[], &cfg);
        // Pairwise distance must be at least min half-extent.
        for i in 0..3 {
            for j in (i + 1)..3 {
                let dx = cards[i].x - cards[j].x;
                let dy = cards[i].y - cards[j].y;
                let d = (dx * dx + dy * dy).sqrt();
                assert!(d > 50.0,
                    "cards {} and {} too close: {}", i, j, d);
            }
        }
    }

    #[test]
    fn seed_grid_spaces_cards() {
        // 4 cards on a 800x600 canvas → 2x2 grid.
        let mut cards = vec![
            CardPos::new("a"),
            CardPos::new("b"),
            CardPos::new("c"),
            CardPos::new("d"),
        ];
        let cfg = LayoutConfig::default();
        seed_grid(&mut cards, &cfg);
        // No two cards should share coordinates after seeding.
        for i in 0..cards.len() {
            for j in (i + 1)..cards.len() {
                let same = cards[i].x == cards[j].x && cards[i].y == cards[j].y;
                assert!(!same, "cards {} and {} share coords", i, j);
            }
        }
    }

    #[test]
    fn seed_grid_skips_pinned() {
        let mut cards = vec![
            CardPos::pinned_at("p", 999.0, 999.0),
            CardPos::new("a"),
            CardPos::new("b"),
        ];
        let cfg = LayoutConfig::default();
        seed_grid(&mut cards, &cfg);
        // Pinned card preserved exactly.
        assert_eq!(cards[0].x, 999.0);
        assert_eq!(cards[0].y, 999.0);
        // Unpinned cards now at non-default positions.
        assert_ne!(cards[1].x, 0.0);
        assert_ne!(cards[2].x, 0.0);
    }

    #[test]
    fn bounding_box_empty() {
        assert_eq!(bounding_box(&[]), (0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn bounding_box_single() {
        let cards = vec![CardPos {
            id: "a".into(), x: 100.0, y: 200.0,
            pinned: false, half_w: 50.0, half_h: 30.0,
        }];
        let (min_x, min_y, max_x, max_y) = bounding_box(&cards);
        assert_eq!(min_x, 50.0);
        assert_eq!(min_y, 170.0);
        assert_eq!(max_x, 150.0);
        assert_eq!(max_y, 230.0);
    }

    #[test]
    fn bounding_box_multiple() {
        let cards = vec![
            CardPos { id: "a".into(), x: 100.0, y: 100.0,
                pinned: false, half_w: 50.0, half_h: 30.0 },
            CardPos { id: "b".into(), x: 500.0, y: 400.0,
                pinned: false, half_w: 50.0, half_h: 30.0 },
        ];
        let (min_x, min_y, max_x, max_y) = bounding_box(&cards);
        assert_eq!(min_x, 50.0);
        assert_eq!(min_y, 70.0);
        assert_eq!(max_x, 550.0);
        assert_eq!(max_y, 430.0);
    }

    #[test]
    fn edge_with_missing_endpoint_ignored() {
        // Edge references a node that isn't in the cards list.
        // Layout should still run without panic.
        let mut cards = vec![CardPos::new("a"), CardPos::new("b")];
        cards[0].x = 100.0; cards[0].y = 100.0;
        cards[1].x = 300.0; cards[1].y = 100.0;
        let edges = vec![
            LayoutEdge::new("a", "b"),
            LayoutEdge::new("a", "ghost"), // ghost doesn't exist
        ];
        let cfg = LayoutConfig::default();
        // No panic.
        layout_force_directed(&mut cards, &edges, &cfg);
    }

    #[test]
    fn self_edge_ignored() {
        // Edge from a node to itself shouldn't crash or produce NaN.
        let mut cards = vec![
            CardPos { id: "a".into(), x: 100.0, y: 100.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
            CardPos { id: "b".into(), x: 300.0, y: 100.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
        ];
        let edges = vec![
            LayoutEdge::new("a", "a"), // self-edge
            LayoutEdge::new("a", "b"),
        ];
        let cfg = LayoutConfig::default();
        layout_force_directed(&mut cards, &edges, &cfg);
        // Positions should be finite numbers.
        assert!(cards[0].x.is_finite());
        assert!(cards[0].y.is_finite());
        assert!(cards[1].x.is_finite());
        assert!(cards[1].y.is_finite());
    }

    #[test]
    fn all_pinned_skips_force_application() {
        // All cards pinned — no movement regardless of force.
        let mut cards = vec![
            CardPos::pinned_at("a", 100.0, 100.0),
            CardPos::pinned_at("b", 100.0, 100.0), // overlapping pin
        ];
        let edges = vec![LayoutEdge::new("a", "b")];
        let cfg = LayoutConfig::default();
        layout_force_directed(&mut cards, &edges, &cfg);
        // Both must stay exactly where pinned.
        assert_eq!(cards[0].x, 100.0); assert_eq!(cards[0].y, 100.0);
        assert_eq!(cards[1].x, 100.0); assert_eq!(cards[1].y, 100.0);
    }

    // v0.1.399: adapt_to_graph tests (mindmap-crowding fix).

    #[test]
    fn adapt_small_graph_near_identity() {
        // At the reference size (~12 nodes), the multipliers
        // should be ≈1.0 so behavior matches pre-v0.1.399.
        let mut cfg = LayoutConfig::default();
        let base_gravity = cfg.center_gravity;
        let base_repulsion = cfg.repulsion_strength;
        cfg.adapt_to_graph(12, 11);
        // scale = sqrt(12/12) = 1.0 → no change to gravity/repulsion.
        assert!((cfg.center_gravity - base_gravity).abs() < 1e-6,
            "gravity should be unchanged at n=12: {} vs {}",
            cfg.center_gravity, base_gravity);
        assert!((cfg.repulsion_strength - base_repulsion).abs() < 1.0,
            "repulsion should be unchanged at n=12");
    }

    #[test]
    fn adapt_large_graph_loosens_gravity() {
        // Bigger graph → weaker center gravity (looser spread).
        let mut cfg = LayoutConfig::default();
        let base_gravity = cfg.center_gravity;
        cfg.adapt_to_graph(48, 47);
        // scale = sqrt(48/12) = 2.0 → gravity halved.
        assert!(cfg.center_gravity < base_gravity,
            "gravity should shrink for big graphs");
        assert!((cfg.center_gravity - base_gravity / 2.0).abs() < 1e-5,
            "gravity should be ~half at 4x nodes: {} vs {}",
            cfg.center_gravity, base_gravity / 2.0);
    }

    #[test]
    fn adapt_large_graph_grows_canvas() {
        let mut cfg = LayoutConfig::default();
        cfg.adapt_to_graph(48, 47);
        // scale = 2.0 → canvas doubles.
        assert!((cfg.width - 1600.0).abs() < 1.0);
        assert!((cfg.height - 1200.0).abs() < 1.0);
    }

    #[test]
    fn adapt_caps_iterations() {
        let mut cfg = LayoutConfig::default();
        // Huge graph — iterations should cap at 400.
        cfg.adapt_to_graph(10000, 9999);
        assert!(cfg.iterations <= 400,
            "iterations capped: got {}", cfg.iterations);
    }

    #[test]
    fn adapt_dense_graph_spreads_nodes() {
        // v0.1.412: dense graphs now SPREAD (more repulsion +
        // edge length) so highly-connected hubs don't crowd.
        // Attraction still gets a gentle nudge but the dominant
        // effect is spread, not collapse.
        let mut cfg = LayoutConfig::default();
        let base_attraction = cfg.attraction_strength;
        let base_repulsion = cfg.repulsion_strength;
        let base_edge_len = cfg.target_edge_length;
        // 20 nodes, 40 edges → density 2.0 > 1.5 threshold.
        // (n=20 → scale=sqrt(20/12)≈1.29, which also bumps
        // repulsion via the sqrt(scale) line; the density branch
        // adds MORE on top. We assert the combined result
        // exceeds the scale-only baseline.)
        let mut scale_only = LayoutConfig::default();
        scale_only.adapt_to_graph(20, 10); // sparse: same n, no density branch
        cfg.adapt_to_graph(20, 40);         // dense: density branch fires
        assert!(cfg.repulsion_strength > scale_only.repulsion_strength,
            "dense graph should have MORE repulsion than a same-size sparse graph");
        assert!(cfg.target_edge_length > scale_only.target_edge_length,
            "dense graph should have LONGER edges than a same-size sparse graph");
        // Attraction still nudged up (gently).
        assert!(cfg.attraction_strength > base_attraction,
            "dense graph keeps a gentle attraction nudge");
        let _ = (base_repulsion, base_edge_len);
    }

    #[test]
    fn adapt_sparse_graph_keeps_attraction() {
        let mut cfg = LayoutConfig::default();
        let base_attraction = cfg.attraction_strength;
        // 20 nodes, 10 edges → density 0.5 < 1.5.
        cfg.adapt_to_graph(20, 10);
        assert!((cfg.attraction_strength - base_attraction).abs() < 1e-6,
            "sparse graph attraction unchanged");
    }

    #[test]
    fn adapt_respects_caller_base_values() {
        // A caller that set mermaid-specific base values
        // should have those scaled, not replaced. At n=12
        // (scale 1.0) the base value passes through.
        let mut cfg = LayoutConfig {
            target_edge_length: 120.0,
            ..Default::default()
        };
        cfg.adapt_to_graph(12, 11);
        assert!((cfg.target_edge_length - 120.0).abs() < 1.0,
            "caller's 120.0 edge length should survive at n=12: {}",
            cfg.target_edge_length);
    }

    #[test]
    fn adapt_zero_nodes_no_panic() {
        let mut cfg = LayoutConfig::default();
        // Must not divide by zero.
        cfg.adapt_to_graph(0, 0);
        assert!(cfg.width.is_finite());
        assert!(cfg.center_gravity.is_finite());
    }

    // ── v0.1.464: directional flow bias ──

    #[test]
    fn flow_direction_parses_mermaid_tokens() {
        assert_eq!(FlowDirection::from_mermaid("TD"), FlowDirection::TopDown);
        assert_eq!(FlowDirection::from_mermaid("tb"), FlowDirection::TopDown);
        assert_eq!(FlowDirection::from_mermaid("BT"), FlowDirection::BottomUp);
        assert_eq!(FlowDirection::from_mermaid("LR"), FlowDirection::LeftRight);
        assert_eq!(FlowDirection::from_mermaid("rl"), FlowDirection::RightLeft);
        assert_eq!(FlowDirection::from_mermaid("xx"), FlowDirection::None);
        assert_eq!(FlowDirection::from_mermaid(""), FlowDirection::None);
    }

    #[test]
    fn flow_none_is_default_and_undirected() {
        // Default config must be undirected — existing behavior.
        assert_eq!(LayoutConfig::default().flow_direction, FlowDirection::None);
        assert!(!FlowDirection::None.is_directional());
    }

    #[test]
    fn topdown_places_child_below_parent() {
        // a → b under TopDown: b should end up below a (larger y).
        let mut cards = vec![
            CardPos { id: "a".into(), x: 400.0, y: 300.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
            CardPos { id: "b".into(), x: 400.0, y: 300.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
        ];
        let edges = vec![LayoutEdge::new("a", "b")];
        let cfg = LayoutConfig {
            iterations: 500,
            flow_direction: FlowDirection::TopDown,
            ..Default::default()
        };
        layout_force_directed(&mut cards, &edges, &cfg);
        // b (the `to` node) below a (the `from` node).
        assert!(cards[1].y > cards[0].y + 30.0,
            "TD: child y={} should be well below parent y={}",
            cards[1].y, cards[0].y);
    }

    #[test]
    fn leftright_places_child_right_of_parent() {
        let mut cards = vec![
            CardPos { id: "a".into(), x: 400.0, y: 300.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
            CardPos { id: "b".into(), x: 400.0, y: 300.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
        ];
        let edges = vec![LayoutEdge::new("a", "b")];
        let cfg = LayoutConfig {
            iterations: 500,
            flow_direction: FlowDirection::LeftRight,
            ..Default::default()
        };
        layout_force_directed(&mut cards, &edges, &cfg);
        assert!(cards[1].x > cards[0].x + 30.0,
            "LR: child x={} should be well right of parent x={}",
            cards[1].x, cards[0].x);
    }

    #[test]
    fn bottomup_inverts_topdown() {
        // Same graph, BT: child should be ABOVE parent (smaller y).
        let mut cards = vec![
            CardPos { id: "a".into(), x: 400.0, y: 300.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
            CardPos { id: "b".into(), x: 400.0, y: 300.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
        ];
        let edges = vec![LayoutEdge::new("a", "b")];
        let cfg = LayoutConfig {
            iterations: 500,
            flow_direction: FlowDirection::BottomUp,
            ..Default::default()
        };
        layout_force_directed(&mut cards, &edges, &cfg);
        assert!(cards[1].y < cards[0].y - 30.0,
            "BT: child y={} should be well above parent y={}",
            cards[1].y, cards[0].y);
    }

    #[test]
    fn topdown_chain_is_monotonic() {
        // a → b → c under TD: y should increase down the chain.
        let mut cards = vec![
            CardPos { id: "a".into(), x: 400.0, y: 300.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
            CardPos { id: "b".into(), x: 420.0, y: 310.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
            CardPos { id: "c".into(), x: 410.0, y: 290.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
        ];
        let edges = vec![LayoutEdge::new("a", "b"), LayoutEdge::new("b", "c")];
        let cfg = LayoutConfig {
            iterations: 600,
            flow_direction: FlowDirection::TopDown,
            ..Default::default()
        };
        layout_force_directed(&mut cards, &edges, &cfg);
        // Root above middle above leaf.
        assert!(cards[0].y < cards[1].y,
            "a (y={}) should be above b (y={})", cards[0].y, cards[1].y);
        assert!(cards[1].y < cards[2].y,
            "b (y={}) should be above c (y={})", cards[1].y, cards[2].y);
    }

    #[test]
    fn flow_bias_zero_strength_is_undirected() {
        // flow_direction set but strength 0 → no bias applied.
        // Start slightly apart so repulsion has a defined direction
        // (coincident nodes have no symmetry-breaking force — a
        // real seeded layout never starts two nodes exactly atop
        // each other, the seed grid offsets them).
        let mut directed = vec![
            CardPos { id: "a".into(), x: 390.0, y: 300.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
            CardPos { id: "b".into(), x: 410.0, y: 300.0,
                pinned: false, half_w: 80.0, half_h: 50.0 },
        ];
        let edges = vec![LayoutEdge::new("a", "b")];
        let cfg = LayoutConfig {
            iterations: 500,
            flow_direction: FlowDirection::TopDown,
            flow_bias_strength: 0.0,
            ..Default::default()
        };
        layout_force_directed(&mut directed, &edges, &cfg);
        // With zero strength, the directional bias is fully off —
        // behaves exactly like undirected force layout. Assert it
        // stayed finite and didn't eject (connected nodes settle
        // near target_edge_length, so we don't assert a separation
        // floor here — that's covered by the undirected tests).
        assert!(directed[0].x.is_finite() && directed[0].y.is_finite());
        assert!(directed[1].x.is_finite() && directed[1].y.is_finite());
        let dy = directed[1].y - directed[0].y;
        let dx = directed[1].x - directed[0].x;
        let dist = (dx * dx + dy * dy).sqrt();
        // Sane, non-runaway separation (edge attraction keeps them
        // near target_edge_length, repulsion keeps them apart).
        assert!(dist > 1.0 && dist < 2000.0,
            "zero-strength: expected sane separation, got {}", dist);
    }

    #[test]
    fn flow_bias_does_not_eject_nodes() {
        // Directional bias must not send nodes to infinity — the
        // step cap + damping should keep everything finite and
        // on a sane canvas even over many iterations.
        let mut cards: Vec<CardPos> = (0..8)
            .map(|k| CardPos { id: format!("n{}", k), x: 400.0, y: 300.0,
                pinned: false, half_w: 80.0, half_h: 50.0 })
            .collect();
        let edges: Vec<LayoutEdge> = (0..7)
            .map(|k| LayoutEdge::new(format!("n{}", k), format!("n{}", k + 1)))
            .collect();
        let cfg = LayoutConfig {
            iterations: 600,
            flow_direction: FlowDirection::TopDown,
            ..Default::default()
        };
        layout_force_directed(&mut cards, &edges, &cfg);
        for c in &cards {
            assert!(c.x.is_finite() && c.y.is_finite(),
                "node {} ejected to non-finite", c.id);
            // Sane bound: nothing should be light-years away.
            assert!(c.x.abs() < 100_000.0 && c.y.abs() < 100_000.0,
                "node {} at runaway ({}, {})", c.id, c.x, c.y);
        }
    }
}
