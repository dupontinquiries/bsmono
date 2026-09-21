//! signals_panel — see the lanes, weight them, plan a cut that says why.
//!
//! For a camera group: its span gets one lane per input the project can
//! offer -- the kick in the mix, each control channel's level, where
//! transcript lines start -- plus the operator's hard marks from markers named
//! "cut here" / "no cut". The window draws every lane as a sparkline with an
//! on/off switch and a weight; "plan cuts" turns the weighted candidates into
//! a multicam cut on a new track, with a marker at every cut naming the lanes
//! that put it there. One group, one undo.
//!
//! Which ANGLE each cut goes to is round-robin for now: the signals decide
//! WHEN. Choosing the angle from signals (motion, who is speaking) is the next
//! lane to add, and it slots in here.

use studio_core::{GroupId, Op, Project, Ticks, TrackId};
use studio_video::signals::{self, Candidate, Mark, Signal};

pub const HOP: f64 = 0.02;

/// Transcript line starts that fall inside the group's clips, on the timeline,
/// relative to `start`.
pub fn line_starts(project: &Project, group: GroupId, start: Ticks,
                   words: &std::collections::BTreeMap<studio_core::SourceId, Vec<studio_voice::Segment>>) -> Vec<f64> {
    let Some(g) = project.groups.get(&group) else { return Vec::new() };
    let mut out = Vec::new();
    for c in g.clips.iter().filter_map(|id| project.clips.get(id)) {
        let Some(segs) = words.get(&c.source) else { continue };
        for s in segs {
            if s.start >= c.src_in && s.start < c.src_in.saturating_add(c.len) {
                let tl = c.pos.saturating_add(s.start.saturating_sub(c.src_in));
                out.push((tl.0 - start.0) as f64 / studio_core::TICKS_PER_SECOND as f64);
            }
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup_by(|a, b| (*a - *b).abs() < 0.05);
    out
}

/// The operator's marks inside the span, relative to its start.
pub fn marks(project: &Project, start: Ticks, end: Ticks) -> Vec<Mark> {
    let named: Vec<(f64, String)> = project.markers.iter().filter(|m| m.at >= start && m.at < end)
        .map(|m| ((m.at.0 - start.0) as f64 / studio_core::TICKS_PER_SECOND as f64, m.name.clone())).collect();
    signals::marks_from_names(&named)
}

/// The cut, as ops: a new video track with a clip per shot, and a marker at
/// every cut naming why. Applied as one `Op::Group`.
pub fn plan_ops(project: &Project, group: GroupId, cands: &[Candidate]) -> Result<(Vec<Op>, usize), String> {
    let (angles, start, end) = studio_video::multicam::from_group(project, group)?;
    let span = (end.0 - start.0) as f64 / studio_core::TICKS_PER_SECOND as f64;
    let times: Vec<f64> = cands.iter().map(|c| c.at).filter(|t| *t > 0.0 && *t < span).collect();
    let shots = studio_video::multicam::plan(&angles,
        &studio_video::multicam::Timing::OnBeats { beats: times, every: 1 },
        &studio_video::multicam::Choice::Cycle,
        studio_video::multicam::Rules { min_shot: 0.0, hysteresis_db: 0.0, settle: 0.0 }, span);
    let track = TrackId(project.tracks.iter().map(|t| t.id.0).max().unwrap_or(0) + 1);
    let first_clip = project.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1;
    let mut ops = vec![Op::AddTrack { track: studio_core::Track { id: track, kind: studio_core::TrackKind::Video,
        name: "multicam (signals)".into(), muted: false, locked: false, gain: 1.0, route: None }, at: project.tracks.len() }];
    ops.extend(studio_video::multicam::layout(&shots, &angles, track, start, first_clip));
    for c in cands.iter().filter(|c| c.at > 0.0 && c.at < span) {
        // the time is part of the name: markers are unique BY NAME, so three cuts
        // with the same reason ("kick 1.00") collapsed into one marker without it
        let at = start.saturating_add(Ticks::from_seconds(c.at));
        ops.push(Op::SetMarker { at, name: format!("why @{:.2}s: {}", at.to_seconds(), c.explain()) });
    }
    Ok((ops, shots.len()))
}

/// The window's state for one group.
pub struct Panel {
    pub group: GroupId,
    pub lanes: Vec<Signal>,
    pub marks: Vec<Mark>,
    pub span: f64,
    pub threshold: f32,
    pub min_gap: f64,
    pub planned: Vec<Candidate>,
}

impl Panel {
    /// Draw; returns the candidates when "plan cuts" is pressed.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<Vec<Candidate>> {
        let mut go = None;
        ui.label(format!("{:.1} s · {} lane(s) · {} operator mark(s) (markers named “cut here” / “no cut”)",
                         self.span, self.lanes.len(), self.marks.len()));
        for lane in self.lanes.iter_mut() {
            ui.horizontal(|ui| {
                ui.checkbox(&mut lane.on, "");
                ui.add_sized([110.0, 18.0], egui::Label::new(&lane.name).truncate());
                ui.add(egui::Slider::new(&mut lane.weight, 0.0..=3.0).text("weight"));
                let (rect, _) = ui.allocate_exact_size(egui::vec2(260.0, 22.0), egui::Sense::hover());
                let p = ui.painter_at(rect);
                p.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
                let n = lane.values.len().max(1);
                let cols = rect.width() as usize;
                let pts: Vec<egui::Pos2> = (0..cols).map(|x| {
                    let a = x * n / cols; let b = ((x + 1) * n / cols).max(a + 1).min(n);
                    let v = lane.values[a..b].iter().cloned().fold(0.0f32, f32::max);
                    egui::pos2(rect.min.x + x as f32, rect.max.y - v * rect.height())
                }).collect();
                let c = if lane.on { ui.visuals().selection.bg_fill } else { ui.visuals().weak_text_color() };
                p.add(egui::Shape::line(pts, egui::Stroke::new(1.0, c)));
            });
        }
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut self.threshold, 0.05..=0.9).text("threshold"));
            ui.add(egui::Slider::new(&mut self.min_gap, 0.3..=6.0).text("min gap s"));
        });
        let preview = signals::candidates(&self.lanes, &self.marks, self.span, HOP, self.threshold, self.min_gap);
        ui.label(format!("{} cut(s) at these settings", preview.len()));
        if ui.button("plan cuts on a new track").clicked() { go = Some(preview.clone()); }
        if !self.planned.is_empty() {
            ui.separator();
            ui.label("last plan — why each cut is there:");
            egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                for c in &self.planned { ui.small(format!("{:6.2} s  {}", c.at, c.explain())); }
            });
        }
        go
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::*;

    fn project() -> (Project, GroupId) {
        let mut p = Project::default();
        for id in 1..=2u64 {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(id), kind: TrackKind::Video, name: format!("a{id}"),
                muted: false, locked: false, gain: 1.0, route: None }, at: (id - 1) as usize }).unwrap();
            p.apply(&Op::AddSource { source: Source::file(SourceId(id), format!("C{id}.mkv"), Ticks::from_seconds(60.0), 48_000, 2, true) }).unwrap();
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(id), track: TrackId(id),
                pos: Ticks::from_seconds(10.0), src_in: Ticks::from_seconds(2.0), len: Ticks::from_seconds(8.0), gain: 1.0, effects: vec![] } }).unwrap();
        }
        p.apply(&Op::SetGroup { id: GroupId(1), name: "cam".into(), clips: vec![ClipId(1), ClipId(2)] }).unwrap();
        (p, GroupId(1))
    }

    #[test]
    fn transcript_lines_become_a_lane_on_the_timeline() {
        let (p, g) = project();
        let seg = |a: f64| studio_voice::Segment { start: Ticks::from_seconds(a), end: Ticks::from_seconds(a + 1.0),
            text: "x".into(), speaker: None, confidence: 1.0 };
        let mut w = std::collections::BTreeMap::new();
        w.insert(SourceId(1), vec![seg(1.0), seg(4.0), seg(7.5), seg(20.0)]);   // 1.0 and 20.0 are outside the clip
        let t = line_starts(&p, g, Ticks::from_seconds(10.0), &w);
        assert_eq!(t, vec![2.0, 5.5], "source 4.0 s -> timeline 12.0 s -> 2.0 s into the span");
    }

    #[test]
    fn a_plan_cuts_where_the_candidates_are_and_marks_why() {
        let (mut p, g) = project();
        p.apply(&Op::SetMarker { at: Ticks::from_seconds(15.0), name: "no cut — the look".into() }).unwrap();
        let kicks = signals::pulses("kick", &[1.0, 3.0, 5.0, 7.0], 8.0, HOP, 0.1);
        let m = marks(&p, Ticks::from_seconds(10.0), Ticks::from_seconds(18.0));
        let cands = signals::candidates(&[kicks], &m, 8.0, HOP, 0.3, 0.5);
        assert!(cands.iter().all(|c| (c.at - 5.0).abs() > 0.4), "the no-cut marker blocked 5.0: {cands:?}");
        let (ops, shots) = plan_ops(&p, g, &cands).unwrap();
        let before = p.clips.len();
        let inv = p.apply(&Op::Group { ops }).unwrap();
        let cut_track = p.tracks.last().unwrap().id;
        let mut cuts: Vec<f64> = p.clips.values().filter(|c| c.track == cut_track).map(|c| c.pos.to_seconds()).collect();
        cuts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(shots, cuts.len());
        assert_eq!(cuts, vec![10.0, 11.0, 13.0, 17.0], "cuts at the candidates (1, 3, 7 s into the span)");
        let whys: Vec<&str> = p.markers.iter().filter(|m| m.name.starts_with("why")).map(|m| m.name.as_str()).collect();
        assert_eq!(whys.len(), 3);
        assert!(whys.iter().all(|w| w.contains("kick")), "{whys:?}");
        p.apply(&inv).unwrap();
        assert_eq!(p.clips.len(), before, "one undo takes the plan back");
        assert!(p.markers.iter().all(|m| !m.name.starts_with("why")), "including its markers");
    }
}
