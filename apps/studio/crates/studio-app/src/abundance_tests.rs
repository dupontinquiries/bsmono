//! Abundance: the app's tools on a big shoot, not a toy one.
//!
//! Real projects carry far more than a fixture: dozens of angles, hundreds of
//! recorder files, thousands of clips and tracks. Two things break there that
//! never break at ten: correctness under clutter, and cost that grows faster
//! than the material. Every test here builds the big case, checks the answer,
//! and bounds the time -- generous bounds in a debug build, meant to catch a
//! quadratic surprise rather than to benchmark.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use studio_core::*;

fn big_project(tracks: u64, clips_per_track: u64, sources: u64) -> Project {
    let mut p = Project::default();
    for s in 0..sources {
        p.apply(&Op::AddSource { source: Source::file(SourceId(s + 1), format!("/card/A{:03}/C{:05}.mkv", s / 100, s),
            Ticks::from_seconds(120.0), 48_000, 2, true) }).unwrap();
    }
    let mut id = 1u64;
    for t in 0..tracks {
        p.apply(&Op::AddTrack { track: Track { id: TrackId(t + 1), kind: if t % 2 == 0 { TrackKind::Video } else { TrackKind::Audio },
            name: format!("t{t}"), muted: false, locked: false, gain: 1.0, route: None }, at: t as usize }).unwrap();
        for c in 0..clips_per_track {
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId((id % sources) + 1), track: TrackId(t + 1),
                pos: Ticks::from_seconds(c as f64 * 4.0), src_in: Ticks::ZERO, len: Ticks::from_seconds(3.5), gain: 1.0, effects: vec![] } }).unwrap();
            id += 1;
        }
    }
    p
}

fn timed<T>(what: &str, limit: Duration, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let r = f();
    let e = t.elapsed();
    eprintln!("abundance: {what}: {e:?}");
    assert!(e < limit, "{what} took {e:?} (limit {limit:?}) -- cost is growing faster than the material");
    r
}

#[test]
fn the_structure_view_summarises_a_thousand_tracks() {
    let mut p = big_project(1000, 20, 400);   // 20,000 clips
    for k in 0..15u64 { p.apply(&Op::SetMarker { at: Ticks::from_seconds(k as f64 * 5.0), name: format!("section {k}") }).unwrap(); }
    // redrawn every frame in the structure view, so it must stay well under a
    // frame's budget even here (debug build); the first version took 1.0 s
    let ov = timed("overview of 1000 tracks x 20 clips", Duration::from_millis(250), || crate::overview::summarize(&p, 12));
    assert_eq!(ov.lanes.iter().map(|l| l.tracks).sum::<usize>(), 1000);
    assert_eq!(ov.sections.len(), 15);
    assert!(ov.cells.iter().all(|row| row.len() == 15));
}

#[test]
fn multiview_lays_out_sixteen_angles_without_overlap() {
    let mut p = big_project(16, 1, 16);
    let group_clips: Vec<ClipId> = p.clips.keys().copied().collect();
    p.apply(&Op::SetGroup { id: GroupId(1), name: "cam".into(), clips: group_clips }).unwrap();
    // only the video tracks' clips have picture in this fixture: 8 angles
    let panes = crate::viewport::multiview_at(&p, Ticks::from_seconds(1.0), GroupId(1)).expect("a multiview");
    assert!(panes.len() >= 8, "{}", panes.len());
    let rect = |t: &studio_fx::Transform| (t.x, t.y, t.x + t.scale_x, t.y + t.scale_y);
    for (i, a) in panes.iter().enumerate() { for b in &panes[i + 1..] {
        let (a, b) = (rect(&a.transform), rect(&b.transform));
        assert!(!(a.0 < b.2 - 1e-4 && b.0 < a.2 - 1e-4 && a.1 < b.3 - 1e-4 && b.1 < a.3 - 1e-4), "{a:?} overlaps {b:?}");
    }}
}

#[test]
fn relinking_two_thousand_sources_stays_linear() {
    let p = big_project(2, 1, 2000);
    let dir = std::env::temp_dir().join(format!("abund-relink-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("A000")).unwrap();
    for s in 0..2000u64 { std::fs::write(dir.join("A000").join(format!("C{:05}.MXF", s)), b"x").unwrap(); }
    let plan = timed("relink plan, 2000 sources", Duration::from_secs(10), || crate::relink::plan(&p, &dir));
    assert_eq!(plan.matched.len(), 2000, "every source found: {}", plan.summary());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn transcripts_for_a_thousand_sources_load_by_name() {
    let p = big_project(2, 1, 1000);
    let dir = std::env::temp_dir().join(format!("abund-tx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for s in (0..1000u64).step_by(2) {
        std::fs::write(dir.join(format!("C{:05}.srt", s)), "1\n00:00:01,000 --> 00:00:02,000\nhello\n").unwrap();
    }
    let l = timed("transcripts for 1000 sources", Duration::from_secs(10), || crate::transcripts::load(&p, &dir));
    assert_eq!((l.timed, l.missing), (500, 500), "{}", l.summary());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_signals_plan_with_twelve_angles_and_hundreds_of_cuts_applies_whole() {
    let mut p = Project::default();
    for k in 0..12u64 {
        p.apply(&Op::AddTrack { track: Track { id: TrackId(k + 1), kind: TrackKind::Video, name: format!("a{k}"),
            muted: false, locked: false, gain: 1.0, route: None }, at: k as usize }).unwrap();
        p.apply(&Op::AddSource { source: Source::file(SourceId(k + 1), format!("A{k}.mkv"), Ticks::from_seconds(700.0), 48_000, 2, true) }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(k + 1), source: SourceId(k + 1), track: TrackId(k + 1), pos: Ticks::ZERO,
            src_in: Ticks::from_seconds(k as f64), len: Ticks::from_seconds(600.0), gain: 1.0, effects: vec![] } }).unwrap();
    }
    p.apply(&Op::SetGroup { id: GroupId(1), name: "cam".into(), clips: (1..=12).map(ClipId).collect() }).unwrap();
    let hop = crate::signals_panel::HOP;
    let kick_times: Vec<f64> = (1..300).map(|i| i as f64 * 2.0).collect();
    let kicks = studio_video::signals::pulses("kick", &kick_times, 600.0, hop, 0.1);
    let cands = timed("candidates, 10 min", Duration::from_secs(20),
        || studio_video::signals::candidates(&[kicks], &[], 600.0, hop, 0.3, 1.0));
    assert!(cands.len() > 250, "{}", cands.len());
    // motion per angle: each angle busy in its own stretch
    let n = (600.0 / hop) as usize;
    let per_angle: Vec<studio_video::signals::Signal> = (0..12).map(|k| studio_video::signals::Signal::new(&format!("m{k}"), hop,
        (0..n).map(|i| if ((i as f64 * hop) / 50.0) as usize % 12 == k { 0.9 } else { 0.1 }).collect())).collect();
    let inputs = [studio_video::signals::AngleInput { name: "motion".into(), per_angle, weight: 1.0 }];
    let (ops, shots, lines) = timed("plan 12 angles x ~300 cuts", Duration::from_secs(20), || crate::signals_panel::plan_ops(
        &p, GroupId(1), &cands, &inputs, studio_video::signals::Prefer::MostMotion, true).unwrap());
    assert_eq!(lines.len(), cands.len());
    let inv = p.apply(&Op::Group { ops }).expect("the whole plan applies as one group");
    let cut_track = p.tracks.last().unwrap().id;
    let mut cut_clips: Vec<&Clip> = p.clips.values().filter(|c| c.track == cut_track).collect();
    cut_clips.sort_by_key(|c| c.pos);
    assert_eq!(cut_clips.len(), shots);
    assert!(cut_clips.windows(2).all(|w| w[0].source != w[1].source), "no jump cuts among twelve angles");
    let whys = p.markers.iter().filter(|m| m.name.starts_with("why")).count();
    assert_eq!(whys, cands.len(), "every cut kept its own marker -- none collapsed by name");
    p.apply(&inv).unwrap();
    assert!(p.markers.is_empty() && p.tracks.len() == 12, "and one undo takes all of it back");
}

#[test]
fn a_stage_over_a_four_hundred_clip_scene() {
    let mut p = big_project(4, 100, 20);
    p.apply(&Op::AddBus { bus: Bus { id: BusId(1), name: "Music".into(), parent: None, gain: 1.0, chain: vec![],
        sends: vec![], muted: false, silent: false } }).unwrap();
    let scene: Vec<ClipId> = p.clips.keys().copied().collect();
    let (ops, _) = timed("stage over 400 clips", Duration::from_secs(5), || crate::stage::bind_ops(&p, &scene, BusId(1), 0).unwrap());
    p.apply(&Op::Group { ops }).unwrap();
    let v = timed("auto value, 2000 samples", Duration::from_secs(10), || {
        (0..2000).map(|i| p.auto_value(AutoTarget::BusGain(BusId(1)), Ticks::from_seconds(i as f64 * 0.2))).collect::<Vec<_>>()
    });
    assert!(v[5] < 0.1 && v[1000] > 0.9, "the arc still rises across a big scene: {} {}", v[5], v[1000]);
}

#[test]
fn the_palette_lists_many_groups_and_busses_quickly() {
    let mut p = big_project(40, 2, 40);
    for g in 0..40u64 {
        p.apply(&Op::SetGroup { id: GroupId(g + 1), name: format!("cam {g}"), clips: vec![ClipId(g * 2 + 1), ClipId(g * 2 + 3)] }).unwrap();
    }
    for b in 0..60u64 {
        p.apply(&Op::AddBus { bus: Bus { id: BusId(b + 1), name: format!("bus {b}"), parent: None, gain: 1.0, chain: vec![],
            sends: vec![], muted: false, silent: b % 3 == 0 } }).unwrap();
    }
    let _ = BTreeMap::<u8, u8>::new();
    let c = timed("palette: stage bus list", Duration::from_secs(2), || crate::palette::candidates(crate::palette::Ask::StageBus, &p));
    assert_eq!(c.len(), 40, "the audible busses (every third is a control channel)");
    let tail = timed("palette: tail choices", Duration::from_secs(2), || crate::palette::candidates(crate::palette::Ask::ClipTailChoice, &p));
    assert!(!tail.is_empty());
}
