//! overview — the whole project at once: sections across, lanes down.
//!
//! The timeline is fine grain: one row per track, one block per clip. That
//! stops working in two places — a project with hundreds of tracks, where
//! the rows are too thin to read and scrolling vertically loses the shape,
//! and any question about STRUCTURE ("which scenes have no sound yet", "where
//! are the takes nobody has checked", "is the third act thin") which is
//! about sections, not clips.
//!
//! This is the other zoom: a grid.
//!
//! * **Columns are sections**, taken from the project's own markers — the
//!   timeline's chapters. A marker named as a problem (starting `check` or
//!   `slate?`, the convention the film-ingest demo writes) is a FLAG, not a
//!   section. With no section markers, the project is cut into equal spans.
//! * **Rows are lanes**: one per bus (the mixer already groups tracks — a
//!   dialogue bus, a music bus), plus one for unrouted picture and one for
//!   unrouted sound. A hundred tracks become a handful of rows. The main
//!   alternative, one row per composition, groups by what the edit contains
//!   instead of where it is heard; buses were chosen because every project
//!   with many tracks routes them, while compositions are optional.
//! * **A cell** says how much of that section the lane covers, how many
//!   clips it holds, and how many problems are flagged inside it.
//!
//! Recogniser loops and unread slates belong here rather than under every
//! clip: at this zoom a flag is one dot per section, whatever the track count.

use studio_core::{BusId, Project, Ticks, TrackKind};

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub name: String,
    pub start: Ticks,
    pub end: Ticks,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum LaneKey {
    Bus(BusId),
    Picture,
    Sound,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Lane {
    pub key: LaneKey,
    pub label: String,
    pub tracks: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Cell {
    /// 0..1: how much of the section's time at least one clip in this lane
    /// covers. Overlapping clips do not count twice.
    pub coverage: f32,
    pub clips: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Overview {
    pub sections: Vec<Section>,
    pub lanes: Vec<Lane>,
    /// `cells[lane][section]`.
    pub cells: Vec<Vec<Cell>>,
    /// Problem markers per section.
    pub flags: Vec<usize>,
}

/// Whether a marker is a working note -- an operator's "cut here" / "no cut",
/// or a signals plan's "why @..." -- rather than a chapter or a problem. Seen
/// on screen: a "no cut" marker became a section of its own, and a signals
/// plan's dozens of "why" markers would have shattered the view into slivers.
pub fn is_note(name: &str) -> bool {
    let n = name.trim_start().to_lowercase();
    n.starts_with("cut here") || n.starts_with("no cut") || n.starts_with("why @") || n.starts_with("why:")
}

/// Whether a marker is a problem to look at rather than a chapter.
pub fn is_flag(name: &str) -> bool {
    let n = name.trim_start().to_lowercase();
    n.starts_with("check") || n.starts_with("slate?") || n.starts_with('?')
}

/// Summarise `project`. `fallback_sections` is how many equal spans to use
/// when there are no section markers.
pub fn summarize(project: &Project, fallback_sections: usize) -> Overview {
    let end = project.duration();
    let mut marks: Vec<&studio_core::Marker> = project.markers.iter().filter(|m| !is_flag(&m.name) && !is_note(&m.name)).collect();
    marks.sort_by_key(|m| m.at);
    let mut sections: Vec<Section> = Vec::new();
    if marks.is_empty() || end <= Ticks::ZERO {
        let n = fallback_sections.max(1) as i64;
        for i in 0..n {
            let a = Ticks(end.0 * i / n);
            let b = Ticks(end.0 * (i + 1) / n);
            sections.push(Section { name: format!("{:.0}s", a.to_seconds()), start: a, end: b });
        }
    } else {
        if marks[0].at > Ticks::ZERO {
            sections.push(Section { name: "(start)".into(), start: Ticks::ZERO, end: marks[0].at });
        }
        for (i, m) in marks.iter().enumerate() {
            let b = marks.get(i + 1).map(|n| n.at).unwrap_or(end).max(m.at);
            sections.push(Section { name: m.name.clone(), start: m.at, end: b });
        }
        sections.retain(|s| s.end > s.start);
    }

    // lanes: one per bus that has tracks routed to it, then unrouted picture/sound
    let mut lanes: Vec<Lane> = Vec::new();
    let lane_of = |t: &studio_core::Track| match t.route {
        Some(b) if project.busses.contains_key(&b) => LaneKey::Bus(b),
        _ => if t.kind == TrackKind::Video { LaneKey::Picture } else { LaneKey::Sound },
    };
    for t in &project.tracks {
        let key = lane_of(t);
        match lanes.iter_mut().find(|l| l.key == key) {
            Some(l) => l.tracks += 1,
            None => {
                let label = match &key {
                    LaneKey::Bus(b) => project.busses[b].name.clone(),
                    LaneKey::Picture => "picture".into(),
                    LaneKey::Sound => "sound".into(),
                };
                lanes.push(Lane { key, label, tracks: 1 });
            }
        }
    }
    lanes.sort_by(|a, b| a.key.cmp(&b.key));

    // One pass to bucket clips by lane, through a track -> lane map. The first
    // version asked, for every lane x section x clip, whether the clip's track
    // was in the lane's track LIST: 1.0 s at 1,000 tracks and 20,000 clips --
    // and the structure view redraws this every frame.
    let lane_of_track: std::collections::HashMap<studio_core::TrackId, usize> = project.tracks.iter()
        .filter_map(|t| lanes.iter().position(|l| l.key == lane_of(t)).map(|i| (t.id, i))).collect();
    let mut by_lane: Vec<Vec<(i64, i64)>> = vec![Vec::new(); lanes.len()];
    for c in project.clips.values() {
        if let Some(&li) = lane_of_track.get(&c.track) { by_lane[li].push((c.pos.0, c.end().0)); }
    }
    for spans in by_lane.iter_mut() { spans.sort(); }
    let mut cells = vec![vec![Cell::default(); sections.len()]; lanes.len()];
    for (li, spans_all) in by_lane.iter().enumerate() {
        for (si, sec) in sections.iter().enumerate() {
            // the lane's clips are sorted by start, so the walk stops at the
            // first that starts after this section (no skip at the front: a
            // long clip that starts early can still reach in)
            let mut clips = 0usize;
            let mut covered = 0i64;
            let mut cur: Option<(i64, i64)> = None;
            for &(a0, b0) in spans_all {
                if a0 >= sec.end.0 { break; }
                let (a, b) = (a0.max(sec.start.0), b0.min(sec.end.0));
                if b <= a { continue; }
                clips += 1;
                match cur {
                    Some((ca, cb)) if a <= cb => cur = Some((ca, cb.max(b))),
                    Some((ca, cb)) => { covered += cb - ca; cur = Some((a, b)); }
                    None => cur = Some((a, b)),
                }
            }
            if let Some((a, b)) = cur { covered += b - a; }
            let len = (sec.end.0 - sec.start.0).max(1);
            cells[li][si] = Cell { coverage: (covered as f64 / len as f64) as f32, clips };
        }
    }
    let flags = sections.iter().map(|sec| project.markers.iter()
        .filter(|m| is_flag(&m.name) && m.at >= sec.start && m.at < sec.end).count()).collect();
    Overview { sections, lanes, cells, flags }
}

/// Draw the overview; returns a section start to jump to when one is clicked.
pub fn show(ui: &mut egui::Ui, ov: &Overview, playhead: Ticks,
            c: &uikit_egui::PaletteColors) -> Option<Ticks> {
    let mut jump = None;
    if ov.sections.is_empty() || ov.lanes.is_empty() {
        ui.colored_label(c.text_weak, "nothing on the timeline yet");
        return None;
    }
    let label_w = 140.0;
    let head_h = 34.0;
    let row_h = 26.0;
    let avail = ui.available_width();
    let total = ov.sections.last().map(|s| s.end.0).unwrap_or(1).max(1) as f32;
    let width = (avail - label_w).max(200.0);
    let height = head_h + row_h * ov.lanes.len() as f32 + 8.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(avail, height), egui::Sense::click());
    let p = ui.painter_at(rect);
    let x_of = |t: Ticks| rect.min.x + label_w + t.0 as f32 / total * width;
    for (si, s) in ov.sections.iter().enumerate() {
        let (x0, x1) = (x_of(s.start), x_of(s.end));
        let col = egui::Rect::from_min_max(egui::pos2(x0, rect.min.y), egui::pos2(x1, rect.max.y));
        if si % 2 == 1 { p.rect_filled(col, 0.0, c.bg_card); }
        p.line_segment([egui::pos2(x0, rect.min.y), egui::pos2(x0, rect.max.y)], egui::Stroke::new(1.0_f32, c.border));
        // names that fit, otherwise nothing: an overlapping name is worse than none
        let name: String = s.name.chars().take(((x1 - x0 - 6.0) / 6.0).max(0.0) as usize).collect();
        if name.chars().count() >= 3 {
            p.text(egui::pos2(x0 + 3.0, rect.min.y + 4.0), egui::Align2::LEFT_TOP, name,
                   egui::FontId::proportional(11.0), c.text);
        }
        if ov.flags[si] > 0 {
            p.text(egui::pos2(x0 + 3.0, rect.min.y + 18.0), egui::Align2::LEFT_TOP,
                   format!("⚑ {}", ov.flags[si]), egui::FontId::proportional(10.0), c.warning);
        }
    }
    for (li, lane) in ov.lanes.iter().enumerate() {
        let y = rect.min.y + head_h + li as f32 * row_h;
        p.text(egui::pos2(rect.min.x + 6.0, y + row_h / 2.0), egui::Align2::LEFT_CENTER,
               format!("{} ({})", lane.label, lane.tracks), egui::FontId::proportional(12.0), c.text);
        for (si, s) in ov.sections.iter().enumerate() {
            let cell = ov.cells[li][si];
            let r = egui::Rect::from_min_max(egui::pos2(x_of(s.start) + 1.0, y + 3.0),
                                             egui::pos2(x_of(s.end) - 1.0, y + row_h - 3.0));
            if cell.clips == 0 { continue; }
            let base = match lane.key { LaneKey::Picture => c.accent, _ => c.success };
            p.rect_filled(r, 2.0, base.gamma_multiply(0.2 + 0.7 * cell.coverage));
        }
    }
    let px = x_of(playhead);
    p.line_segment([egui::pos2(px, rect.min.y), egui::pos2(px, rect.max.y)], egui::Stroke::new(1.5_f32, c.danger));
    if resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            jump = ov.sections.iter().find(|s| pos.x >= x_of(s.start) && pos.x < x_of(s.end)).map(|s| s.start);
        }
    }
    jump
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::*;

    fn project(tracks: usize) -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddBus { bus: Bus { id: BusId(1), name: "dialogue".into(), parent: None, gain: 1.0,
            chain: vec![], sends: vec![], muted: false, silent: false } }).unwrap();
        p.apply(&Op::AddSource { source: Source::file(SourceId(1), "a.wav", Ticks::from_seconds(600.0), 48_000, 2, false) }).unwrap();
        for i in 0..tracks as u64 {
            let kind = if i % 3 == 0 { TrackKind::Video } else { TrackKind::Audio };
            let route = if i % 3 == 1 { Some(BusId(1)) } else { None };
            p.apply(&Op::AddTrack { track: Track { id: TrackId(i + 1), kind, name: format!("t{i}"),
                muted: false, locked: false, gain: 1.0, route }, at: i as usize }).unwrap();
            // each track covers 0..10 s, the dialogue ones also 20..25 s
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(i * 10 + 1), source: SourceId(1), track: TrackId(i + 1),
                pos: Ticks::ZERO, src_in: Ticks::ZERO, len: Ticks::from_seconds(10.0), gain: 1.0, effects: vec![] } }).unwrap();
            if route.is_some() {
                p.apply(&Op::AddClip { clip: Clip { id: ClipId(i * 10 + 2), source: SourceId(1), track: TrackId(i + 1),
                    pos: Ticks::from_seconds(20.0), src_in: Ticks::ZERO, len: Ticks::from_seconds(5.0), gain: 1.0, effects: vec![] } }).unwrap();
            }
        }
        p.apply(&Op::SetMarker { at: Ticks::ZERO, name: "act one".into() }).unwrap();
        p.apply(&Op::SetMarker { at: Ticks::from_seconds(20.0), name: "act two".into() }).unwrap();
        p.apply(&Op::SetMarker { at: Ticks::from_seconds(21.0), name: "check C0031: no slate read".into() }).unwrap();
        p
    }

    #[test]
    fn a_hundred_tracks_become_a_handful_of_lanes() {
        let ov = summarize(&project(120), 8);
        assert_eq!(ov.lanes.len(), 3, "dialogue bus, unrouted picture, unrouted sound: {:?}", ov.lanes);
        assert_eq!(ov.lanes.iter().map(|l| l.tracks).sum::<usize>(), 120, "every track lands in exactly one lane");
        assert_eq!(ov.lanes[0].label, "dialogue");
    }

    #[test]
    fn sections_come_from_markers_and_problem_markers_are_flags() {
        let ov = summarize(&project(6), 8);
        let names: Vec<&str> = ov.sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["act one", "act two"], "the check marker is not a chapter");
        assert_eq!(ov.flags, vec![0, 1], "it is a flag, in the section it falls in");
        assert!(is_flag("slate? C0035 heard 3") && is_flag("  Check this") && !is_flag("S1: nun scene"));
    }

    #[test]
    fn a_cell_says_how_much_of_its_section_is_covered_counting_overlaps_once() {
        let p = project(6);
        let ov = summarize(&p, 8);
        let dia = ov.lanes.iter().position(|l| l.label == "dialogue").unwrap();
        // act one is 0..20 s; two dialogue tracks both cover 0..10 -> 50%, not 100%
        assert!((ov.cells[dia][0].coverage - 0.5).abs() < 1e-3, "{:?}", ov.cells[dia][0]);
        assert_eq!(ov.cells[dia][0].clips, 2);
        // act two is 20..25 s (project end): the dialogue tracks cover all of it
        assert!((ov.cells[dia][1].coverage - 1.0).abs() < 1e-3, "{:?}", ov.cells[dia][1]);
        let pic = ov.lanes.iter().position(|l| l.label == "picture").unwrap();
        assert_eq!(ov.cells[pic][1].clips, 0, "no picture in act two — the thing this view exists to show");
    }

    #[test]
    fn working_notes_are_neither_chapters_nor_problems() {
        let mut p = project(6);
        p.apply(&Op::SetMarker { at: Ticks::from_seconds(5.0), name: "no cut — hold the look".into() }).unwrap();
        p.apply(&Op::SetMarker { at: Ticks::from_seconds(7.0), name: "Cut here".into() }).unwrap();
        for k in 0..12 { p.apply(&Op::SetMarker { at: Ticks::from_seconds(1.0 + k as f64), name: format!("why @{k}.00s: kick 0.5") }).unwrap(); }
        let ov = summarize(&p, 8);
        let names: Vec<&str> = ov.sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["act one", "act two"], "notes do not become chapters: {names:?}");
        assert_eq!(ov.flags, vec![0, 1], "nor problems: only the real check marker counts");
    }

    #[test]
    fn with_no_markers_the_project_is_cut_into_equal_spans() {
        let mut p = project(3);
        p.markers.clear();
        let ov = summarize(&p, 5);
        assert_eq!(ov.sections.len(), 5);
        assert_eq!(ov.sections.last().unwrap().end, p.duration());
        assert!(summarize(&Project::default(), 5).lanes.is_empty());
    }
}
