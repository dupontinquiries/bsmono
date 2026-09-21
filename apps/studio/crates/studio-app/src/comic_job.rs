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
