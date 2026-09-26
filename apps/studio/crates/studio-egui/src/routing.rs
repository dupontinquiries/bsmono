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
    /// A FAMILY of lanes (see `families`), drawn as one node.
    Family(usize),
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
    /// Audio tracks, in project order — the leftmost column (empty when folded).
    pub tracks: Vec<TrackId>,
    /// Families, the leftmost column when folded (empty when not).
    pub families: Vec<Family>,
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

/// A FAMILY of lanes: the tracks that are one thing -- a take and its
/// overflow lanes "(2)", its "(muted in FL)" lanes, its "(muted)" copy --
/// going to one place. Derived, never stored: a lens, so every project gets
/// it, imported or not, and nothing in the model or the file changes.
/// (Video editors' track stacks and roles, for the same reason: a 159-lane
/// project is unreadable lane by lane and readable by what the lanes ARE.)
#[derive(Debug, Clone, PartialEq)]
pub struct Family {
    pub name: String,
    pub route: Option<BusId>,
    pub tracks: Vec<TrackId>,
    /// Lanes that are muted (by the user, or muted in FL on import).
    pub muted: usize,
}

impl Family {
    pub fn all_muted(&self) -> bool { self.muted == self.tracks.len() }
    /// The label cut to `max` characters: the COUNTS are kept whole and the
    /// name shortened with an ellipsis -- "x19 . 17 muted" is the news, and a
    /// centred label wider than its box was cut off at the map's left edge.
    pub fn label_fit(&self, max: usize) -> String {
        let full = self.label();
        if full.chars().count() <= max { return full; }
        let tail: String = full.chars().skip(self.name.chars().count()).collect();
        let room = max.saturating_sub(tail.chars().count() + 1).max(3);
        format!("{}\u{2026}{tail}", self.name.chars().take(room).collect::<String>())
    }

    /// "name x3 . 2 muted" -- what the node says.
    pub fn label(&self) -> String {
        let mut l = self.name.clone();
        if self.tracks.len() > 1 { l.push_str(&format!(" \u{00d7}{}", self.tracks.len())); }
        if self.muted > 0 { l.push_str(&format!(" \u{00b7} {} muted", self.muted)); }
        l
    }
}

/// A lane's family name: its name without the lane suffixes the tools add --
/// " (muted in FL)", " (muted)", and an overflow index " (2)". A " #2" stays:
/// in FL that is a different channel, not a lane of the same one.
pub fn family_name(track_name: &str) -> String {
    let mut n = track_name.trim().to_string();
    loop {
        let before = n.clone();
        for suffix in [" (muted in FL)", " (muted)"] {
            if let Some(x) = n.strip_suffix(suffix) { n = x.trim_end().to_string(); }
        }
        if let Some(open) = n.rfind(" (") {
            let inner = &n[open + 2..];
            if inner.len() > 1 && inner.ends_with(')') && inner[..inner.len() - 1].chars().all(|c| c.is_ascii_digit()) {
                n.truncate(open);
            }
        }
        if n == before { return n; }
    }
}

/// The project's audio tracks as families, in order of first appearance.
pub fn families(project: &Project) -> Vec<Family> {
    let mut out: Vec<Family> = Vec::new();
    for t in project.tracks.iter().filter(|t| t.kind == TrackKind::Audio) {
        let name = family_name(&t.name);
        match out.iter_mut().find(|f| f.name == name && f.route == t.route) {
            Some(f) => { f.tracks.push(t.id); if t.muted { f.muted += 1; } }
            None => out.push(Family { name, route: t.route, tracks: vec![t.id], muted: t.muted as usize }),
        }
    }
    out
}

/// The map with lanes FOLDED into families; with `show_muted` false, a family
/// whose every lane is muted is left out -- the ideas switched off, a toggle
/// away rather than half the map.
pub fn folded_layout(project: &Project, show_muted: bool) -> MapLayout {
    let mut l = map_layout(project);
    let fams: Vec<Family> = families(project).into_iter().filter(|f| show_muted || !f.all_muted()).collect();
    l.edges.retain(|e| !matches!(e.from, Endpoint::Track(_)));
    for (i, f) in fams.iter().enumerate() {
        l.edges.push(Edge { from: Endpoint::Family(i), to: f.route.map(Endpoint::Bus).unwrap_or(Endpoint::Master), send: None });
    }
    l.tracks.clear();
    l.families = fams;
    l
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
    MapLayout { tracks, families: Vec::new(), columns, edges }
}

const NODE_W: f32 = 150.0; // 96 until c20: a family label (name + "×19 · 17 muted") needs the room
const NODE_H: f32 = 22.0;
const COL_GAP: f32 = 56.0;
const ROW_GAP: f32 = 6.0;

/// Draw the map. Read-only: the return is nothing, on purpose.
pub fn routing_map(ui: &mut egui::Ui, project: &Project, c: &uikit_egui::PaletteColors) {
    // View state only (egui memory): fold lanes into families, show muted.
    let (kf, km) = (egui::Id::new("routing-fold"), egui::Id::new("routing-show-muted"));
    let (mut fold, mut show_muted) = ui.data(|d| (d.get_temp::<bool>(kf).unwrap_or(true), d.get_temp::<bool>(km).unwrap_or(false)));
    ui.horizontal(|ui| {
        ui.checkbox(&mut fold, "fold lanes into families");
        ui.add_enabled(fold, egui::Checkbox::new(&mut show_muted, "show muted"));
    });
    ui.data_mut(|d| { d.insert_temp(kf, fold); d.insert_temp(km, show_muted); });
    let layout = if fold { folded_layout(project, show_muted) } else { map_layout(project) };
    let cols = 2 + layout.columns.len(); // tracks + bus columns + master
    let tallest = layout.tracks.len().max(layout.families.len())
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
    for ri in 0..layout.families.len() {
        at.insert(Endpoint::Family(ri), egui::pos2(col_x(0), row_y(ri, layout.families.len())));
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
            Endpoint::Family(i) => layout.families.get(*i).map(|f| f.label_fit((NODE_W / 6.2) as usize)).unwrap_or_default(),
            Endpoint::Master => "master".into(),
        }
    };
    for (e, p) in &at {
        let r = egui::Rect::from_center_size(*p, egui::vec2(NODE_W, NODE_H));
        let fill = match e {
            Endpoint::Track(_) => c.bg_raised,
            Endpoint::Family(i) if layout.families.get(*i).is_some_and(|f| f.all_muted()) => c.bg_base,
            Endpoint::Family(_) => c.bg_raised,
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

    #[test]
    fn lane_suffixes_fold_but_a_different_channel_does_not() {
        assert_eq!(family_name("Triplit_Vocals (muted in FL) (2)"), "Triplit_Vocals");
        assert_eq!(family_name("Triplit_Vocals (2) (muted)"), "Triplit_Vocals");
        assert_eq!(family_name("Triplit_Vocals"), "Triplit_Vocals");
        assert_eq!(family_name("Omnisphere #2"), "Omnisphere #2", "#2 is another FL channel");
        assert_eq!(family_name("Chorus (live)"), "Chorus (live)", "only numeric indices are lanes");
    }

    /// A Top A Tower in miniature: one take's lanes -- playing, overflow,
    /// muted in FL -- going to one bus fold to ONE node; a lane of the same
    /// name to another bus stays apart; an all-muted family hides unless asked.
    #[test]
    fn a_takes_lanes_fold_to_one_node_per_destination_and_muted_families_hide() {
        use studio_core::{Bus, Op, Track};
        let mut p = Project::default();
        let bus = |id: u64, name: &str| Op::AddBus { bus: Bus { id: BusId(id), name: name.into(), parent: None, gain: 1.0,
            chain: vec![], sends: vec![], muted: false, silent: false } };
        p.apply(&bus(1, "vox")).unwrap();
        p.apply(&bus(2, "fx")).unwrap();
        let lanes = [("Take", 1, false), ("Take (2)", 1, false), ("Take (muted in FL)", 1, true), ("Take (muted in FL) (2)", 1, true),
                     ("Take", 2, false), ("Old idea (muted in FL)", 1, true)];
        for (i, (name, b, muted)) in lanes.iter().enumerate() {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(i as u64 + 1), kind: TrackKind::Audio, name: (*name).into(),
                muted: *muted, locked: false, gain: 1.0, route: Some(BusId(*b)) }, at: i }).unwrap();
        }
        let f = families(&p);
        assert_eq!(f.len(), 3, "{f:?}");
        assert_eq!((f[0].name.as_str(), f[0].tracks.len(), f[0].muted), ("Take", 4, 2));
        assert_eq!(f[0].label(), "Take \u{00d7}4 \u{00b7} 2 muted");
        let long = Family { name: "Triplit_BirdsEyeView_Vocals".into(), route: None, tracks: vec![TrackId(1); 19], muted: 17 };
        let fit = long.label_fit(24);
        assert!(fit.chars().count() <= 24 && fit.ends_with("\u{00d7}19 \u{00b7} 17 muted") && fit.starts_with("Tri"), "{fit}");
        assert_eq!(f[1].route, Some(BusId(2)), "same name, other bus: its own node");
        let folded = folded_layout(&p, false);
        assert_eq!(folded.families.len(), 2, "the all-muted idea hides: {:?}", folded.families);
        assert!(folded.tracks.is_empty());
        assert_eq!(folded.edges.iter().filter(|e| matches!(e.from, Endpoint::Family(_))).count(), 2);
        assert_eq!(folded_layout(&p, true).families.len(), 3, "and shows when asked");
        assert_eq!(map_layout(&p).tracks.len(), 6, "unfolded, every lane");
    }

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
                parent, gain: 1.0, chain: vec![], sends: vec![], muted: false, silent: false } }).unwrap();
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
