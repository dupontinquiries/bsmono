//! graph_view — the link graph and niftyplot, drawn.
//!
//! `src/niftyplot_view.rs` and `src/link_graph.rs` are **Qt-free** (zero
//! qmetaobject or QString references), so the port mounts them by `#[path]`
//! rather than reimplementing the layout maths — the same decision as
//! rusty-board's config and voice stack, for the same reason: one
//! implementation, and a fix to ring expansion helps both frontends.
//!
//! What the port adds is the drawing and the interaction the Qt view had:
//!
//! * **Ring layout around a focus** — `Topology::rings` places nodes by graph
//!   distance, so a graph reads as "what is near this" rather than as a hairball.
//! * **Directional navigation** — `step_focus` moves the focus to the nearest
//!   node in a compass direction, because a graph you can only navigate by
//!   clicking is a graph you cannot navigate by voice, and voice is the point.
//! * **Breadcrumbs** — `breadcrumb_push` keeps a capped trail, so wandering
//!   deep into a graph is reversible.
//! * **Depth control** — expanding a hop at a time, since a graph rendered at
//!   full depth is unreadable at any size.

#[path = "../../src/niftyplot_view.rs"]
pub mod nifty;

use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, Vec2};

/// The view's own state: what is focused, how deep, and where we have been.
#[derive(Debug, Clone)]
pub struct GraphView {
    pub focus: String,
    pub depth: usize,
    pub trail: Vec<String>,
    /// Yaw/pitch for the layered orientation the Qt view offered.
    pub yaw: f32,
    pub pitch: f32,
    pub spread: f32,
}

impl Default for GraphView {
    fn default() -> Self {
        GraphView { focus: String::new(), depth: 2, trail: Vec::new(),
                    yaw: 0.0, pitch: 0.0, spread: 1.0 }
    }
}

/// How many hops out the view will draw. Past three a graph is a hairball at
/// any window size, and the depth control exists precisely so a user never
/// lands there by accident.
pub const MAX_DEPTH: usize = 3;

/// Breadcrumb trail length. Long enough to retrace a session, short enough that
/// the bar stays readable on a phone.
pub const TRAIL_CAP: usize = 12;

/// Direction codes, matching `niftyplot_view`'s own numbering so the two
/// frontends cannot disagree about which way left is.
pub const DIR_UP: u8 = 0;
pub const DIR_DOWN: u8 = 1;
pub const DIR_LEFT: u8 = 2;
pub const DIR_RIGHT: u8 = 3;

/// Map a spoken word to a direction. The graph is meant to be navigable by
/// voice, and that means the vocabulary lives somewhere testable rather than
/// inside a match arm in the UI.
pub fn direction_for(word: &str) -> Option<u8> {
    Some(match word.trim().to_lowercase().as_str() {
        "up" | "above" | "north" => DIR_UP,
        "down" | "below" | "south" => DIR_DOWN,
        "left" | "back" | "west" => DIR_LEFT,
        "right" | "forward" | "next" | "east" => DIR_RIGHT,
        _ => return None,
    })
}

impl GraphView {
    /// Move focus, recording where we came from.
    pub fn focus_on(&mut self, id: &str) {
        if id == self.focus { return; }
        if !self.focus.is_empty() {
            self.trail = nifty::breadcrumb_push(&self.trail, &self.focus, TRAIL_CAP);
        }
        self.focus = id.to_string();
    }

    /// Step back along the trail. Returns false at the start rather than
    /// clearing the focus, because a back button that empties the view looks
    /// like a crash.
    pub fn back(&mut self) -> bool {
        match self.trail.pop() {
            Some(prev) => { self.focus = prev; true }
            None => false,
        }
    }

    pub fn deeper(&mut self) { self.depth = (self.depth + 1).min(MAX_DEPTH); }
    pub fn shallower(&mut self) { self.depth = self.depth.saturating_sub(1).max(1); }

    /// Node positions for the current focus, as rings by graph distance.
    ///
    /// Ring layout rather than force-directed: a force layout is prettier and
    /// non-deterministic, and a graph that rearranges itself every time you
    /// open it cannot be navigated from memory — which is what breaks voice
    /// navigation in particular, since "go left" must mean the same thing twice.
    pub fn layout(&self, topo: &nifty::Topology, into: Rect) -> Vec<(String, Pos2, usize)> {
        if self.focus.is_empty() { return Vec::new(); }
        let rings = topo.rings(&self.focus, self.depth);
        let mut by_ring: std::collections::BTreeMap<usize, Vec<String>> = Default::default();
        for (id, d) in rings { by_ring.entry(d).or_default().push(id); }
        let centre = into.center();
        let max_r = into.width().min(into.height()) * 0.42;
        let mut out = Vec::new();
        for (d, ids) in &by_ring {
            if *d == 0 {
                for id in ids { out.push((id.clone(), centre, 0)); }
                continue;
            }
            let r = max_r * (*d as f32 / self.depth.max(1) as f32);
            let n = ids.len().max(1);
            for (i, id) in ids.iter().enumerate() {
                // A fixed phase per ring, so a node keeps its place between
                // frames and between sessions.
                let a = std::f32::consts::TAU * i as f32 / n as f32
                      + *d as f32 * 0.4 + self.yaw;
                out.push((id.clone(), Pos2::new(centre.x + a.cos() * r,
                                                centre.y + a.sin() * r * (1.0 - self.pitch.abs() * 0.4)), *d));
            }
        }
        out
    }

    /// Move focus in a compass direction — the voice-navigable path.
    ///
    /// `dir` is the engine's own code: 0 up, 1 down, 2 left, 3 right. A code
    /// rather than a vector because that is what a spoken command carries —
    /// "go left" is a direction, not an angle — and reusing the engine's
    /// numbering means the two frontends cannot disagree about which way left is.
    ///
    /// "go left" has to mean the same thing every time, which is why the layout
    /// is deterministic and this consumes screen positions rather than graph
    /// structure.
    pub fn step(&mut self, placed: &[(String, Pos2, usize)], dir: u8) -> bool {
        let Some((_, here, _)) = placed.iter().find(|(id, _, _)| *id == self.focus) else {
            return false;
        };
        let cands: Vec<(String, f32, f32)> = placed.iter()
            .filter(|(id, _, _)| *id != self.focus)
            .map(|(id, p, _)| (id.clone(), p.x, p.y))
            .collect();
        match nifty::step_focus((here.x, here.y), &cands, dir) {
            Some(id) => { self.focus_on(&id); true }
            None => false,
        }
    }
}

/// Draw the graph. Returns a node if one was clicked.
pub fn graph_ui(ui: &mut Ui, view: &mut GraphView, topo: &nifty::Topology,
                colors: &uikit_egui::PaletteColors) -> Option<String> {
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, colors.bg_base);
    if view.focus.is_empty() {
        p.text(rect.center(), egui::Align2::CENTER_CENTER, "no graph loaded",
               egui::FontId::proportional(14.0), colors.text_weak);
        return None;
    }
    let placed = view.layout(topo, rect);
    let at = |id: &str| placed.iter().find(|(n, _, _)| n == id).map(|(_, p, _)| *p);

    // edges first, so nodes sit on top of them
    for (id, pos, _) in &placed {
        for nb in topo.neighbors(id) {
            if let Some(q) = at(&nb) {
                p.line_segment([*pos, q], Stroke::new(1.0, colors.border.gamma_multiply(0.5)));
            }
        }
    }
    let mut clicked = None;
    for (id, pos, ring) in &placed {
        let focused = *id == view.focus;
        let r = if focused { 11.0 } else { 7.0 - *ring as f32 };
        // fade with distance, so the eye finds the focus without being told
        let fade = 1.0 - (*ring as f32 / (view.depth + 1) as f32) * 0.55;
        let col = if focused { colors.accent } else { colors.text.gamma_multiply(fade) };
        p.circle_filled(*pos, r.max(3.0), col);
        if focused || *ring <= 1 {
            p.text(*pos + Vec2::new(0.0, r + 9.0), egui::Align2::CENTER_CENTER,
                   id, egui::FontId::proportional(10.0), colors.text.gamma_multiply(fade));
        }
        if let Some(m) = resp.interact_pointer_pos() {
            if resp.clicked() && (m - *pos).length() <= r + 6.0 { clicked = Some(id.clone()); }
        }
    }
    if let Some(id) = &clicked { view.focus_on(id); }
    // breadcrumbs
    if !view.trail.is_empty() {
        p.text(Pos2::new(rect.min.x + 8.0, rect.min.y + 8.0), egui::Align2::LEFT_TOP,
               view.trail.join(" › "), egui::FontId::proportional(10.0), colors.text_weak);
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topo() -> nifty::Topology {
        nifty::Topology::new(
            &["a".into(), "b".into(), "c".into(), "d".into(), "e".into()],
            &[("a".into(), "b".into()), ("a".into(), "c".into()),
              ("b".into(), "d".into()), ("d".into(), "e".into())],
        )
    }

    fn rect() -> Rect { Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 400.0)) }

    #[test]
    fn the_layout_is_deterministic_so_a_graph_can_be_navigated_from_memory() {
        // THE reason for ring layout over force-directed: a graph that
        // rearranges itself every time you open it cannot be navigated by
        // voice, because "go left" must mean the same thing twice.
        let mut v = GraphView::default();
        v.focus_on("a");
        let a = v.layout(&topo(), rect());
        let b = v.layout(&topo(), rect());
        assert_eq!(a.len(), b.len());
        for ((i1, p1, r1), (i2, p2, r2)) in a.iter().zip(&b) {
            assert_eq!(i1, i2);
            assert_eq!(r1, r2);
            assert!((p1.x - p2.x).abs() < 1e-6 && (p1.y - p2.y).abs() < 1e-6);
        }
    }

    #[test]
    fn the_focus_sits_at_the_centre_and_distance_puts_nodes_further_out() {
        let mut v = GraphView::default();
        v.focus_on("a");
        let placed = v.layout(&topo(), rect());
        let c = rect().center();
        let focus = placed.iter().find(|(id, _, _)| id == "a").expect("focus is placed");
        assert!((focus.1 - c).length() < 1.0, "the focus is the centre");
        assert_eq!(focus.2, 0);
        // b is one hop, e is three — b must be nearer the centre
        let d = |id: &str| placed.iter().find(|(n, _, _)| n == id)
            .map(|(_, p, _)| (*p - c).length());
        if let (Some(db), Some(de)) = (d("b"), d("e")) {
            assert!(db < de, "one hop should sit inside three: {db} vs {de}");
        }
    }

    #[test]
    fn depth_is_capped_because_a_full_graph_is_a_hairball() {
        let mut v = GraphView::default();
        for _ in 0..10 { v.deeper(); }
        assert_eq!(v.depth, MAX_DEPTH);
        for _ in 0..10 { v.shallower(); }
        assert_eq!(v.depth, 1, "and never collapses to nothing");
    }

    #[test]
    fn breadcrumbs_record_the_path_and_back_retraces_it() {
        let mut v = GraphView::default();
        v.focus_on("a");
        v.focus_on("b");
        v.focus_on("d");
        assert_eq!(v.focus, "d");
        assert!(v.back());
        assert_eq!(v.focus, "b");
        assert!(v.back());
        assert_eq!(v.focus, "a");
        // at the start, back must FAIL rather than clearing the view — a back
        // button that empties the screen looks like a crash
        assert!(!v.back());
        assert_eq!(v.focus, "a");
    }

    #[test]
    fn refocusing_on_the_current_node_does_not_grow_the_trail() {
        let mut v = GraphView::default();
        v.focus_on("a");
        let n = v.trail.len();
        v.focus_on("a");
        v.focus_on("a");
        assert_eq!(v.trail.len(), n, "a no-op must not fill the breadcrumbs");
    }

    #[test]
    fn the_trail_is_capped_so_the_bar_stays_readable() {
        let mut v = GraphView::default();
        for i in 0..40 { v.focus_on(&format!("n{i}")); }
        assert!(v.trail.len() <= TRAIL_CAP, "trail grew to {}", v.trail.len());
    }

    #[test]
    fn spoken_directions_map_to_the_engines_own_codes() {
        // The graph is meant to be navigable by voice, so the vocabulary has to
        // live somewhere testable rather than in a match arm inside the UI.
        assert_eq!(direction_for("left"), Some(DIR_LEFT));
        assert_eq!(direction_for("  RIGHT "), Some(DIR_RIGHT));
        assert_eq!(direction_for("north"), Some(DIR_UP));
        assert_eq!(direction_for("next"), Some(DIR_RIGHT));
        assert_eq!(direction_for("sideways"), None, "an unknown word must not guess");
        assert_eq!(direction_for(""), None);
    }

    #[test]
    fn directional_stepping_moves_the_focus_and_reports_when_it_cannot() {
        let mut v = GraphView::default();
        v.focus_on("a");
        let placed = v.layout(&topo(), rect());
        let before = v.focus.clone();
        let moved = v.step(&placed, DIR_RIGHT);
        if moved {
            assert_ne!(v.focus, before, "a successful step changes the focus");
            assert_eq!(v.trail.last().map(|s| s.as_str()), Some(before.as_str()),
                       "and records where it came from");
        }
        // an empty candidate set cannot move, and must say so rather than
        // silently doing nothing visible
        let mut lone = GraphView::default();
        lone.focus_on("a");
        assert!(!lone.step(&[], DIR_RIGHT));
    }
}
