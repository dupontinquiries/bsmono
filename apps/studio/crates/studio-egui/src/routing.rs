//! routing — the mix graph, drawn so it can be understood.
//!
//! The Mixer edits routing through per-row combos and send lists, which is
//! the right place to CHANGE it and a hopeless place to READ it: "which
//! busses feed which, and where do the sends go" is a graph question, and a
//! column of dropdowns answers it one edge at a time. This module draws the
//! whole graph at once — tracks on the left, busses in columns by depth,
//! master on the right, main routes as solid lines and sends as dashed ones
//! — read-only on purpose: the map explains, the Mixer's rows edit, and one
//! surface doing both would be two half-surfaces.
//!
//! Layout is pure (`map_layout`) and tested headless, the crate's standing
//! rule: geometry is the part that goes subtly wrong, and it is only
//! testable if nothing needs a window.

use std::collections::BTreeMap;
use studio_core::{BusId, Project, TrackId, TrackKind};

/// One end of an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Endpoint {
    Track(TrackId),
    Bus(BusId),
    Master,
}

/// One connection in the map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edge {
    pub from: Endpoint,
    pub to: Endpoint,
    /// `None` is the main route; `Some(pre)` is a send, pre- or post-fader.
    pub send: Option<bool>,
}

/// The whole map, laid out but unit-less: rows and columns, not pixels —
/// the drawing pass owns pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct MapLayout {
    /// Audio tracks, in project order — the leftmost column.
    pub tracks: Vec<TrackId>,
    /// Bus columns, deepest first, so reading left to right follows the
    /// signal: tracks → sub-busses → the busses that feed master → master.
    pub columns: Vec<Vec<BusId>>,
    pub edges: Vec<Edge>,
}

/// A bus's distance from master along its parent chain. Capped at the bus
/// count: `apply` refuses cycles, so the cap only matters for a project
/// that reached memory some other way, and a wrong depth beats a hang.
fn depth(project: &Project, id: BusId) -> usize {
    let mut d = 0;
    let mut at = id;
    while let Some(parent) = project.busses.get(&at).and_then(|b| b.parent) {
        d += 1;
        at = parent;
        if d > project.busses.len() { break; }
    }
    d
}

/// Lay the graph out. Video tracks are absent — they carry no signal into
/// this mixer, and drawing them would claim otherwise.
pub fn map_layout(project: &Project) -> MapLayout {
    let tracks: Vec<TrackId> = project.tracks.iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .map(|t| t.id).collect();

    let mut by_depth: BTreeMap<usize, Vec<BusId>> = BTreeMap::new();
    for id in project.busses.keys() {
        by_depth.entry(depth(project, *id)).or_default().push(*id);
    }
    // Deepest first: the map reads left→right the way the signal flows.
    let columns: Vec<Vec<BusId>> = by_depth.into_iter().rev().map(|(_, v)| v).collect();

    let mut edges = Vec::new();
    for t in project.tracks.iter().filter(|t| t.kind == TrackKind::Audio) {
        edges.push(Edge {
            from: Endpoint::Track(t.id),
            to: t.route.map(Endpoint::Bus).unwrap_or(Endpoint::Master),
            send: None,
        });
    }
    for (id, b) in &project.busses {
        edges.push(Edge {
            from: Endpoint::Bus(*id),
            to: b.parent.map(Endpoint::Bus).unwrap_or(Endpoint::Master),
            send: None,
        });
        for s in &b.sends {
            edges.push(Edge {
                from: Endpoint::Bus(*id),
                to: Endpoint::Bus(s.to),
                send: Some(s.pre_fader),
            });
        }
    }
    MapLayout { tracks, columns, edges }
}

const NODE_W: f32 = 96.0;
const NODE_H: f32 = 22.0;
const COL_GAP: f32 = 56.0;
const ROW_GAP: f32 = 6.0;

/// Draw the map. Read-only: the return is nothing, on purpose.
pub fn routing_map(ui: &mut egui::Ui, project: &Project, c: &uikit_egui::PaletteColors) {
    let layout = map_layout(project);
    let cols = 2 + layout.columns.len(); // tracks + bus columns + master
    let tallest = layout.tracks.len()
        .max(layout.columns.iter().map(Vec::len).max().unwrap_or(0))
        .max(1);
    let width = cols as f32 * (NODE_W + COL_GAP) - COL_GAP;
    let height = tallest as f32 * (NODE_H + ROW_GAP) + 8.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter();

    // Node centres, computed once so edges and boxes agree by construction.
    let col_x = |ci: usize| rect.left() + ci as f32 * (NODE_W + COL_GAP) + NODE_W * 0.5;
    let row_y = |ri: usize, n: usize| {
        // Each column vertically centred: a two-bus column beside twelve
        // tracks should sit mid-map, not hug the ceiling.
        let block = n as f32 * (NODE_H + ROW_GAP) - ROW_GAP;
        rect.top() + (rect.height() - block) * 0.5 + ri as f32 * (NODE_H + ROW_GAP) + NODE_H * 0.5
    };
    let mut at: BTreeMap<Endpoint, egui::Pos2> = BTreeMap::new();
    for (ri, t) in layout.tracks.iter().enumerate() {
        at.insert(Endpoint::Track(*t), egui::pos2(col_x(0), row_y(ri, layout.tracks.len())));
    }
    for (ci, col) in layout.columns.iter().enumerate() {
        for (ri, b) in col.iter().enumerate() {
            at.insert(Endpoint::Bus(*b), egui::pos2(col_x(ci + 1), row_y(ri, col.len())));
        }
    }
    at.insert(Endpoint::Master, egui::pos2(col_x(cols - 1), row_y(0, 1)));

    // Edges first, boxes on top, so a line never crosses a label.
    for e in &layout.edges {
        let (Some(a), Some(b)) = (at.get(&e.from), at.get(&e.to)) else { continue };
        let a = egui::pos2(a.x + NODE_W * 0.5, a.y);
        let b = egui::pos2(b.x - NODE_W * 0.5, b.y);
        match e.send {
            None => { painter.line_segment([a, b], egui::Stroke::new(1.5, c.accent)); }
            Some(pre) => {
                let col = if pre { c.warning } else { c.text_weak };
                painter.add(egui::Shape::dashed_line(&[a, b],
                    egui::Stroke::new(1.0, col), 5.0, 4.0));
            }
        }
    }
    let name_of = |e: &Endpoint| -> String {
        match e {
            Endpoint::Track(id) => project.tracks.iter().find(|t| t.id == *id)
                .map(|t| t.name.clone()).unwrap_or_else(|| format!("t{}", id.0)),
            Endpoint::Bus(id) => project.busses.get(id)
                .map(|b| b.name.clone()).unwrap_or_else(|| format!("b{}", id.0)),
            Endpoint::Master => "master".into(),
        }
    };
    for (e, p) in &at {
        let r = egui::Rect::from_center_size(*p, egui::vec2(NODE_W, NODE_H));
        let fill = match e {
            Endpoint::Track(_) => c.bg_raised,
            Endpoint::Bus(_) => c.bg_base,
            Endpoint::Master => c.accent.gamma_multiply(0.35),
        };
        painter.rect_filled(r, 4.0, fill);
        painter.rect_stroke(r, 4.0, egui::Stroke::new(1.0, c.text_weak),
                            egui::StrokeKind::Inside);
        painter.text(r.center(), egui::Align2::CENTER_CENTER, name_of(e),
                     egui::FontId::proportional(11.0), c.text);
    }
    ui.colored_label(c.text_weak,
        "solid: main route · dashed: send (amber = pre-fader) — edit routes and sends in the rows below");
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Bus, Op, Send, Track};

    fn project() -> Project {
        let mut p = Project::default();
        for (i, (kind, nm)) in [(TrackKind::Audio, "vox"), (TrackKind::Video, "V1"),
                                 (TrackKind::Audio, "drums")].iter().enumerate() {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(i as u64 + 1), kind: *kind,
                name: (*nm).into(), muted: false, locked: false, gain: 1.0, route: None },
                at: i }).unwrap();
        }
        // Beat ← Drums (a chain two deep), plus a Verb bus fed only by sends.
        for (id, name, parent) in [(1u64, "Beat", None), (2, "Drums", Some(BusId(1))),
                                    (3, "Verb", None)] {
            p.apply(&Op::AddBus { bus: Bus { id: BusId(id), name: name.into(),
                parent, gain: 1.0, chain: vec![], sends: vec![], muted: false } }).unwrap();
        }
        p.apply(&Op::SetRoute { track: TrackId(3), bus: Some(BusId(2)) }).unwrap();
        p.apply(&Op::SetBusSends { id: BusId(2), sends: vec![
            Send { to: BusId(3), level: 0.3, pre_fader: true } ] }).unwrap();
        p
    }

    #[test]
    fn columns_read_left_to_right_the_way_the_signal_flows() {
        let l = map_layout(&project());
        // Drums (depth 1) before Beat and Verb (depth 0): deepest first.
        assert_eq!(l.columns, vec![vec![BusId(2)], vec![BusId(1), BusId(3)]]);
        assert_eq!(l.tracks, vec![TrackId(1), TrackId(3)],
                   "audio tracks only — a video track carries no signal here");
    }

    #[test]
    fn every_route_and_send_is_an_edge_and_defaults_point_at_master() {
        let l = map_layout(&project());
        // vox has no route: it feeds master, and the map must SAY so —
        // an implicit default nobody can see is most of why routing reads
        // as confusing in the first place.
        assert!(l.edges.contains(&Edge { from: Endpoint::Track(TrackId(1)),
            to: Endpoint::Master, send: None }));
        assert!(l.edges.contains(&Edge { from: Endpoint::Track(TrackId(3)),
            to: Endpoint::Bus(BusId(2)), send: None }));
        assert!(l.edges.contains(&Edge { from: Endpoint::Bus(BusId(2)),
            to: Endpoint::Bus(BusId(1)), send: None }));
        assert!(l.edges.contains(&Edge { from: Endpoint::Bus(BusId(1)),
            to: Endpoint::Master, send: None }));
        assert!(l.edges.contains(&Edge { from: Endpoint::Bus(BusId(2)),
            to: Endpoint::Bus(BusId(3)), send: Some(true) }),
            "the pre-fader send is on the map, distinguished from the route");
    }

    #[test]
    fn a_foreign_cycle_terminates_instead_of_hanging_the_frame() {
        // apply refuses cycles, so this state can only arrive from a
        // corrupt or foreign file — the map must survive it anyway.
        let mut p = project();
        p.busses.get_mut(&BusId(1)).unwrap().parent = Some(BusId(2)); // 1↔2
        let l = map_layout(&p);
        assert!(!l.columns.is_empty(), "laid out something rather than spinning");
    }
}
