//! comic_job — the comic auto mode on a camera group, from the palette.
//!
//! Everything the example program took on the command line is read from the
//! project instead:
//!
//! * **roles** -- the group's first angle is the wide; with three or more
//!   angles the next two are the left and right panels; with two, the second
//!   supplies both panels from its left and right halves;
//! * **lines** -- the transcripts of the group's clips, on the timeline;
//! * **sides** -- whichever side's own mic hears a line loudest (the voice
//!   lanes), so the talker's panel lands on their side of the page without
//!   anyone tagging lines; with no voice evidence a line keeps the previous
//!   line's side;
//! * the render runs on a worker and lands as a new clip on its own track at
//!   the group's start: one undo.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc;

use studio_core::{GroupId, Op, Project, SourceId, Ticks};
use studio_video::comic::{self, Line, Region, Side};
use studio_voice::Segment;

/// What one panel reads: a file, where in it the group starts, and which part
/// of the frame shows its people.
#[derive(Debug, Clone, PartialEq)]
pub struct Feed { pub path: PathBuf, pub from: f64, pub region: Region }

#[derive(Debug, Clone, PartialEq)]
pub struct Roles { pub wide: Feed, pub left: Feed, pub right: Feed }

/// Where in a panel's angle its people are: a choice made by looking, because
/// on real night footage no detector found the people who mattered. The
/// defaults (halves) put a slate in a panel on the first real render.
pub const REGION_PRESETS: &[(&str, Region)] = &[
    ("whole", Region::WHOLE),
    ("left half", Region { x0: 0.0, y0: 0.05, x1: 0.55, y1: 0.95 }),
    ("right half", Region { x0: 0.45, y0: 0.05, x1: 1.0, y1: 0.95 }),
    ("left third", Region { x0: 0.0, y0: 0.1, x1: 0.4, y1: 0.9 }),
    ("centre", Region { x0: 0.3, y0: 0.1, x1: 0.7, y1: 0.9 }),
    ("right third", Region { x0: 0.6, y0: 0.1, x1: 1.0, y1: 0.9 }),
    ("faces (upper centre)", Region { x0: 0.2, y0: 0.0, x1: 0.8, y1: 0.6 }),
];

/// The operator's setup: which angle plays which role (indices into the
/// group's angles), and the region each panel shows -- a preset, or wherever a
/// finger dragged it, so it is a value rather than a preset index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Setup { pub wide: usize, pub left: usize, pub right: usize, pub left_region: Region, pub right_region: Region }

impl Setup {
    /// The same defaults `roles` uses, as a starting point for the window.
    pub fn default_for(angles: usize) -> Setup {
        if angles >= 3 { Setup { wide: 0, left: 1, right: 2, left_region: Region::WHOLE, right_region: Region::WHOLE } }
        else { Setup { wide: 0, left: 1.min(angles.saturating_sub(1)), right: 1.min(angles.saturating_sub(1)),
                       left_region: REGION_PRESETS[1].1, right_region: REGION_PRESETS[2].1 } }
    }
}

/// Roles as the operator set them.
pub fn roles_with(project: &Project, group: GroupId, base: &std::path::Path, s: Setup) -> Result<(Roles, Ticks, Ticks), String> {
    let (angles, start, end) = studio_video::multicam::from_group(project, group)?;
    let pick = |k: usize| angles.get(k).ok_or_else(|| format!("no angle {}", k + 1));
    let feed = |k: usize, r: Region| -> Result<Feed, String> {
        let a = pick(k)?;
        let src = project.sources.get(&a.source).ok_or("an angle's source is gone")?;
        Ok(Feed { path: base.join(&src.path), from: (start.to_seconds() + a.offset).max(0.0), region: r })
    };
    Ok((Roles { wide: feed(s.wide, Region::WHOLE)?, left: feed(s.left, s.left_region)?, right: feed(s.right, s.right_region)? },
        start, end))
}

/// Assign the panels from the group's angles (see the module note).
pub fn roles(project: &Project, group: GroupId, base: &std::path::Path) -> Result<(Roles, Ticks, Ticks), String> {
    let (angles, start, end) = studio_video::multicam::from_group(project, group)?;
    let feed = |k: usize, region: Region| -> Result<Feed, String> {
        let a = &angles[k];
        let src = project.sources.get(&a.source).ok_or("an angle's source is gone")?;
        Ok(Feed { path: base.join(&src.path), from: (start.to_seconds() + a.offset).max(0.0), region })
    };
    let r = if angles.len() >= 3 {
        Roles { wide: feed(0, Region::WHOLE)?, left: feed(1, Region::WHOLE)?, right: feed(2, Region::WHOLE)? }
    } else {
        // two angles: the second holds both groups, split down the middle
        Roles { wide: feed(0, Region::WHOLE)?,
                left: feed(1, Region { x0: 0.0, y0: 0.05, x1: 0.55, y1: 0.95 })?,
                right: feed(1, Region { x0: 0.45, y0: 0.05, x1: 1.0, y1: 0.95 })? }
    };
    Ok((r, start, end))
}

/// The dialogue lines inside the group, relative to its start, with sides
/// decided by `side_at` (evidence per moment: `Some(side)` or no evidence).
pub fn lines(project: &Project, group: GroupId, start: Ticks, span: f64,
             words: &BTreeMap<SourceId, Vec<Segment>>, side_at: &dyn Fn(f64, f64) -> Option<Side>) -> Vec<Line> {
    let Some(g) = project.groups.get(&group) else { return Vec::new() };
    let mut spans: Vec<(f64, f64)> = Vec::new();
    for c in g.clips.iter().filter_map(|id| project.clips.get(id)) {
        let Some(segs) = words.get(&c.source) else { continue };
        for s in segs {
            if s.end <= c.src_in || s.start >= c.src_in.saturating_add(c.len) { continue; }
            let tl = |t: Ticks| (c.pos.0 + (t.0 - c.src_in.0) - start.0) as f64 / studio_core::TICKS_PER_SECOND as f64;
            let (a, b) = (tl(s.start).max(0.0), tl(s.end).min(span));
            if b > a { spans.push((a, b)); }
        }
    }
    spans.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    // the same line heard by two angles' transcripts is one line
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (a, b) in spans {
        match merged.last_mut() { Some(m) if a < m.1 - 0.2 => m.1 = m.1.max(b), _ => merged.push((a, b)) }
    }
    let mut side = Side::Right;
    merged.into_iter().map(|(a, b)| {
        if let Some(sd) = side_at(a, b) { side = sd; }
        Line { start: a, end: b, side }
    }).collect()
}

#[derive(Debug)]
pub enum Event { Done(PathBuf, usize), Failed(String) }

pub struct Job { pub rx: mpsc::Receiver<Event>, pub group: GroupId, pub start: Ticks, pub span: f64 }

/// Render the comic on a worker: the plan from `lines`, the page drawn from
/// the three feeds, the wide's sound under it.
pub fn start(roles: Roles, lines: Vec<Line>, span: f64, out: PathBuf, group: GroupId, start: Ticks, wake: impl Fn() + Send + 'static) -> Job {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        use studio_video::framepipe::{render_multi, Pipe, Size};
        let size = Size { w: 960, h: 540 };
        let fps = 24.0;
        let pipe = |f: &Feed| Pipe { input: f.path.clone(), range: Some((f.from, span)), size, fps };
        let shots = comic::plan(&lines, span, comic::Rules::default());
        let style = comic::Style::default();
        let page = comic::paper(size.w, size.h, &style);
        let (lr, rr) = (roles.left.region, roles.right.region);
        let r = render_multi(&[pipe(&roles.wide), pipe(&roles.left), pipe(&roles.right)], &out, 18, 0, |i, f| {
            let t = i as f64 / fps;
            let layout = shots.iter().find(|s| t >= s.start && t < s.end).map(|s| s.layout).unwrap_or(comic::Layout::Wide);
            comic::compose(layout, &f[0], (&f[1], lr), (&f[2], rr), &page, &style, (t * 1.5) as f32)
        });
        let _ = tx.send(match r { Ok(n) => Event::Done(out, n), Err(e) => Event::Failed(e) });
        wake();
    });
    Job { rx, group, start, span }
}

/// The finished render as edits: a source for the file, a video track named
/// for it, and the clip at the group's start. One group, one undo.
pub fn land_ops(project: &Project, path: &std::path::Path, start: Ticks, span: f64) -> Vec<Op> {
    let sid = SourceId(project.sources.keys().map(|s| s.0).max().unwrap_or(0) + 1);
    let tid = studio_core::TrackId(project.tracks.iter().map(|t| t.id.0).max().unwrap_or(0) + 1);
    let cid = studio_core::ClipId(project.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1);
    let len = Ticks::from_seconds(span);
    vec![
        Op::AddSource { source: studio_core::Source::file(sid, path.display().to_string(), len, 48_000, 2, true) },
        Op::AddTrack { track: studio_core::Track { id: tid, kind: studio_core::TrackKind::Video, name: "comic".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: project.tracks.len() },
        Op::AddClip { clip: studio_core::Clip { id: cid, source: sid, track: tid, pos: start, src_in: Ticks::ZERO,
            len, gain: 1.0, effects: vec![] } },
    ]
}

/// The setup window: each angle's real frame, the roles, and a region per
/// panel drawn over its angle's thumbnail.
pub struct SetupWindow {
    pub group: GroupId,
    pub setup: Setup,
    /// One 320x180 RGB frame per angle, from the middle of the group.
    pub frames: Vec<Vec<u8>>,
    textures: Vec<egui::TextureHandle>,
}

impl SetupWindow {
    pub fn new(group: GroupId, frames: Vec<Vec<u8>>) -> SetupWindow {
        let setup = Setup::default_for(frames.len());
        SetupWindow { group, setup, frames, textures: Vec::new() }
    }

    /// Draw; returns the setup when "render" is pressed.
    ///
    /// Touch-first: the thumbnail is the control. Tap to centre the region on
    /// a person, drag to move it, pinch (or Ctrl+scroll) to resize it -- the
    /// editor's gesture rules, one finger drags and two fingers pinch. The
    /// presets stay as the quick way to a known framing. On a narrow screen the
    /// two panels stack instead of sitting side by side, and every control is
    /// fingertip-sized.
    pub fn show(&mut self, ui: &mut egui::Ui, layout: studio_egui::touch::ToolLayout) -> Option<Setup> {
        layout.apply(ui);
        if self.textures.len() != self.frames.len() {
            self.textures = self.frames.iter().enumerate().map(|(i, px)| ui.ctx().load_texture(
                format!("comic-angle-{i}"), egui::ColorImage::from_rgb([320, 180], px), egui::TextureOptions::LINEAR)).collect();
        }
        let n = self.frames.len();
        ui.horizontal_wrapped(|ui| {
            ui.label("wide:");
            for k in 0..n { ui.selectable_value(&mut self.setup.wide, k, format!("angle {}", k + 1)); }
        });
        let textures = self.textures.clone();
        let thumb_w = if layout.pair_side_by_side { 256.0 } else { (ui.available_width() - 8.0).min(480.0) };
        let mut panel = |ui: &mut egui::Ui, title: &str, angle: &mut usize, region: &mut Region| {
            ui.strong(title);
            ui.horizontal_wrapped(|ui| { for k in 0..n { ui.selectable_value(angle, k, format!("angle {}", k + 1)); } });
            if let Some(tex) = textures.get(*angle) {
                let size = egui::vec2(thumb_w, thumb_w * 9.0 / 16.0);
                let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
                let to_frac = |p: egui::Pos2| (((p.x - rect.min.x) / rect.width()).clamp(0.0, 1.0), ((p.y - rect.min.y) / rect.height()).clamp(0.0, 1.0));
                if resp.clicked() { if let Some(p) = resp.interact_pointer_pos() { *region = region.centred_at(to_frac(p)); } }
                if resp.dragged() {
                    let d = resp.drag_delta();
                    *region = region.moved((d.x / rect.width(), d.y / rect.height()));
                }
                if resp.hovered() || resp.dragged() {
                    let z = ui.input(|i| i.zoom_delta());
                    // zoom in (spread fingers) = look closer = a SMALLER region
                    if (z - 1.0).abs() > 1e-3 { *region = region.scaled(1.0 / z); }
                }
                ui.painter().image(tex.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                let r = *region;
                let rr = egui::Rect::from_min_max(
                    egui::pos2(rect.min.x + r.x0 * rect.width(), rect.min.y + r.y0 * rect.height()),
                    egui::pos2(rect.min.x + r.x1 * rect.width(), rect.min.y + r.y1 * rect.height()));
                for out in [egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rr.min.y)),
                            egui::Rect::from_min_max(egui::pos2(rect.min.x, rr.max.y), rect.max),
                            egui::Rect::from_min_max(egui::pos2(rect.min.x, rr.min.y), egui::pos2(rr.min.x, rr.max.y)),
                            egui::Rect::from_min_max(egui::pos2(rr.max.x, rr.min.y), egui::pos2(rect.max.x, rr.max.y))] {
                    ui.painter().rect_filled(out, 0.0, egui::Color32::from_black_alpha(150));
                }
                ui.painter().rect_stroke(rr, 0.0, egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(255, 60, 180)), egui::StrokeKind::Inside);
                // said on screen, not in a tooltip: there is no hover on a touchscreen
                ui.small("tap a person to centre on them · drag to move · pinch to resize");
            }
            ui.horizontal_wrapped(|ui| {
                for (name, preset) in REGION_PRESETS.iter() {
                    if ui.selectable_label(*region == *preset, *name).clicked() { *region = *preset; }
                }
            });
        };
        let (mut la, mut lr, mut ra, mut rr) = (self.setup.left, self.setup.left_region, self.setup.right, self.setup.right_region);
        if layout.pair_side_by_side {
            ui.columns(2, |cols| {
                panel(&mut cols[0], "left panel", &mut la, &mut lr);
                panel(&mut cols[1], "right panel", &mut ra, &mut rr);
            });
        } else {
            panel(ui, "left panel", &mut la, &mut lr);
            ui.separator();
            panel(ui, "right panel", &mut ra, &mut rr);
        }
        (self.setup.left, self.setup.left_region, self.setup.right, self.setup.right_region) = (la, lr, ra, rr);
        ui.separator();
        if ui.button("render the comic").clicked() { return Some(self.setup); }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::*;

    fn project(angles: u64) -> (Project, GroupId) {
        let mut p = Project::default();
        for k in 1..=angles {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(k), kind: TrackKind::Video, name: format!("a{k}"),
                muted: false, locked: false, gain: 1.0, route: None }, at: (k - 1) as usize }).unwrap();
            p.apply(&Op::AddSource { source: Source::file(SourceId(k), format!("A{k}.mkv"), Ticks::from_seconds(90.0), 48_000, 2, true) }).unwrap();
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(k), source: SourceId(k), track: TrackId(k), pos: Ticks::from_seconds(10.0),
                src_in: Ticks::from_seconds(5.0 * k as f64), len: Ticks::from_seconds(30.0), gain: 1.0, effects: vec![] } }).unwrap();
        }
        p.apply(&Op::SetGroup { id: GroupId(1), name: "cam".into(), clips: (1..=angles).map(ClipId).collect() }).unwrap();
        (p, GroupId(1))
    }

    #[test]
    fn roles_come_from_the_angles_and_two_angles_split_the_second() {
        let (p, g) = project(3);
        let (r, _, _) = roles(&p, g, std::path::Path::new("/m")).unwrap();
        assert_eq!((r.wide.path.to_str().unwrap(), r.left.path.to_str().unwrap(), r.right.path.to_str().unwrap()),
                   ("/m/A1.mkv", "/m/A2.mkv", "/m/A3.mkv"));
        assert!((r.left.from - 10.0).abs() < 1e-9, "each feed starts where the group does in ITS file: {}", r.left.from);
        let (p2, g2) = project(2);
        let (r2, _, _) = roles(&p2, g2, std::path::Path::new("/m")).unwrap();
        assert_eq!(r2.left.path, r2.right.path, "two angles: both panels from the second");
        assert!(r2.left.region.x1 <= 0.6 && r2.right.region.x0 >= 0.4, "its left and right halves");
    }

    #[test]
    fn the_operators_setup_decides_roles_and_regions() {
        let (p, g) = project(3);
        let s = Setup { wide: 2, left: 0, right: 1, left_region: REGION_PRESETS[6].1, right_region: REGION_PRESETS[5].1 };
        let (r, _, _) = roles_with(&p, g, std::path::Path::new("/m"), s).unwrap();
        assert_eq!(r.wide.path.to_str().unwrap(), "/m/A3.mkv", "angle 3 as the wide");
        assert_eq!(r.left.region, REGION_PRESETS[6].1, "faces for the left panel");
        assert_eq!(r.right.region, REGION_PRESETS[5].1);
        assert_eq!(r.wide.region, Region::WHOLE, "the wide is never cropped");
        assert!(roles_with(&p, g, std::path::Path::new("/m"), Setup { wide: 9, ..s }).is_err(), "a missing angle is refused");
        // the window's defaults reproduce roles()
        let (p2, g2) = project(2);
        let (a, _, _) = roles(&p2, g2, std::path::Path::new("/m")).unwrap();
        let (b, _, _) = roles_with(&p2, g2, std::path::Path::new("/m"), Setup::default_for(2)).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn lines_are_merged_across_angles_and_take_their_side_from_the_voice() {
        let (p, g) = project(3);
        let seg = |a: f64, b: f64| Segment { start: Ticks::from_seconds(a), end: Ticks::from_seconds(b), text: "x".into(), speaker: None, confidence: 1.0 };
        let mut w = BTreeMap::new();
        // the same line heard by angles 2 and 3 (source times differ by their offsets)
        w.insert(SourceId(2), vec![seg(12.0, 14.0), seg(25.0, 27.0)]);     // timeline 12..14 -> group 2..4 ; 25..27 -> 15..17
        w.insert(SourceId(3), vec![seg(17.2, 19.0)]);                      // timeline 12.2..14 -> the same first line
        let side_at = |a: f64, _b: f64| if a < 10.0 { Some(Side::Left) } else { None };
        let l = lines(&p, g, Ticks::from_seconds(10.0), 30.0, &w, &side_at);
        assert_eq!(l.len(), 2, "one line heard twice is one line: {l:?}");
        assert!((l[0].start - 2.0).abs() < 1e-9 && (l[0].end - 4.0).abs() < 1e-9, "{l:?}");
        assert_eq!(l[0].side, Side::Left, "the voice put it on the left");
        assert_eq!(l[1].side, Side::Left, "no evidence: it keeps the previous line's side");
    }

    #[test]
    fn a_finished_render_lands_as_one_undoable_clip() {
        let (mut p, _) = project(2);
        let before = (p.clips.len(), p.tracks.len(), p.sources.len());
        let inv = p.apply(&Op::Group { ops: land_ops(&p, std::path::Path::new("/r/comic.mp4"), Ticks::from_seconds(10.0), 30.0) }).unwrap();
        let c = p.clips.values().max_by_key(|c| c.id).unwrap();
        assert_eq!((c.pos, c.len), (Ticks::from_seconds(10.0), Ticks::from_seconds(30.0)));
        assert_eq!(p.tracks.last().unwrap().name, "comic");
        p.apply(&inv).unwrap();
        assert_eq!((p.clips.len(), p.tracks.len(), p.sources.len()), before);
    }
}
