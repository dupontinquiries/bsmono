//! Abundance for the video engines: many angles, long spans, many lines.
//! Correctness under clutter, and time bounds that catch quadratic surprises
//! (generous, debug build).
use std::time::{Duration, Instant};

fn timed<T>(what: &str, limit: Duration, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let r = f();
    let e = t.elapsed();
    eprintln!("abundance: {what}: {e:?}");
    assert!(e < limit, "{what} took {e:?} (limit {limit:?})");
    r
}

#[test]
fn a_grid_of_twenty_five_angles_says_which_tiles_are_too_small() {
    let l = crate::grid::layout(25, 1280, 720, 1920, 1080, crate::grid::Options::default());
    assert_eq!(l.tiles.len(), 25);
    assert_eq!((l.cols * l.rows) as usize >= 25, true);
    // 5 across a 1280 canvas is 256 px a tile: the caller must be told
    let min_w = crate::grid::Options::default().min_tile_w;
    let small = l.tiles.iter().filter(|t| t.w < min_w).count();
    assert_eq!(small, l.too_small.len(), "every tile under the minimum is reported, and only those");
    for (i, a) in l.tiles.iter().enumerate() { for b in &l.tiles[i + 1..] {
        assert!(a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y, "tiles overlap: {a:?} {b:?}");
    }}
}

#[test]
fn choosing_among_sixteen_angles_over_ten_minutes() {
    use crate::signals::{choose_angles, AngleInput, Prefer, Signal};
    let hop = 0.04;
    let n = (600.0 / hop) as usize;
    let motion: Vec<Signal> = (0..16).map(|k| Signal::new(&format!("m{k}"), hop,
        (0..n).map(|i| ((i as f32 * 0.01 + k as f32).sin() * 0.5 + 0.5)).collect())).collect();
    let voice: Vec<Signal> = (0..16).map(|k| Signal::new(&format!("v{k}"), hop,
        (0..n).map(|i| if (i / 500) % 16 == k { 1.0 } else { 0.0 }).collect())).collect();
    let inputs = [AngleInput { name: "motion".into(), per_angle: motion, weight: 1.0 },
                  AngleInput { name: "voice".into(), per_angle: voice, weight: 1.0 }];
    let cuts: Vec<f64> = (1..300).map(|i| i as f64 * 2.0).collect();
    let shots = timed("16 angles x 300 shots x 2 inputs", Duration::from_secs(20),
        || choose_angles(&cuts, 600.0, &inputs, Prefer::MostMotion, true));
    assert_eq!(shots.len(), 300);
    assert!(shots.windows(2).all(|w| w[0].2 != w[1].2 && (w[0].1 - w[1].0).abs() < 1e-9), "contiguous, no jump cuts");
    assert!(shots.iter().all(|s| s.2 < 16));
    let used: std::collections::HashSet<usize> = shots.iter().map(|s| s.2).collect();
    assert!(used.len() >= 12, "a crowded group is actually used, not two favourites: {}", used.len());
}

#[test]
fn a_twenty_four_angle_camera_group_plans_and_lays_out() {
    use studio_core::*;
    let mut p = Project::default();
    for k in 0..24u64 {
        p.apply(&Op::AddTrack { track: Track { id: TrackId(k + 1), kind: TrackKind::Video, name: format!("a{k}"),
            muted: false, locked: false, gain: 1.0, route: None }, at: k as usize }).unwrap();
        p.apply(&Op::AddSource { source: Source::file(SourceId(k + 1), format!("A{k}.mkv"), Ticks::from_seconds(400.0), 48_000, 2, true) }).unwrap();
        // staggered starts: the cuttable span is the overlap, 20..300 s
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(k + 1), source: SourceId(k + 1), track: TrackId(k + 1),
            pos: Ticks::from_seconds(k as f64 * 20.0 / 23.0), src_in: Ticks::ZERO, len: Ticks::from_seconds(300.0 - k as f64 * 20.0 / 23.0 + 20.0 * (k as f64 / 23.0) * 0.0), gain: 1.0, effects: vec![] } }).unwrap();
    }
    p.apply(&Op::SetGroup { id: GroupId(1), name: "cam".into(), clips: (1..=24).map(ClipId).collect() }).unwrap();
    let (angles, start, end) = crate::multicam::from_group(&p, GroupId(1)).expect("24 angles");
    assert_eq!(angles.len(), 24);
    assert!((start.to_seconds() - 20.0).abs() < 1e-6, "the span starts where the LAST angle does: {}", start.to_seconds());
    let span = (end.0 - start.0) as f64 / TICKS_PER_SECOND as f64;
    let beats: Vec<f64> = (1..(span as usize)).map(|i| i as f64).collect();
    let shots = timed("plan 24 angles on 280 beats", Duration::from_secs(5), || crate::multicam::plan(&angles,
        &crate::multicam::Timing::OnBeats { beats, every: 1 }, &crate::multicam::Choice::Cycle,
        crate::multicam::Rules { min_shot: 0.0, hysteresis_db: 0.0, settle: 0.0 }, span));
    let ops = crate::multicam::layout(&shots, &angles, TrackId(99), start, 1000);
    let ids: std::collections::HashSet<u64> = ops.iter().filter_map(|o| match o { Op::AddClip { clip } => Some(clip.id.0), _ => None }).collect();
    assert_eq!(ids.len(), ops.len(), "every laid-out clip has its own id");
    assert!(shots.iter().map(|s| s.angle).collect::<std::collections::HashSet<_>>().len() == 24, "all 24 angles take turns");
}

#[test]
fn the_comic_auto_mode_plans_an_hour_of_dialogue() {
    use crate::comic::{plan, Layout, Line, Rules, Side};
    let lines: Vec<Line> = (0..1800).map(|i| {
        let t = i as f64 * 2.0;
        Line { start: t, end: t + 1.2 + (i % 5) as f64 * 0.3, side: if (i / 7) % 2 == 0 { Side::Left } else { Side::Right } }
    }).collect();
    let shots = timed("comic plan, 1800 lines over an hour", Duration::from_secs(10), || plan(&lines, 3600.0, Rules::default()));
    assert_eq!(shots[0].start, 0.0);
    assert!((shots.last().unwrap().end - 3600.0).abs() < 1e-6);
    assert!(shots.windows(2).all(|w| (w[0].end - w[1].start).abs() < 1e-9), "contiguous");
    assert!(shots[..shots.len() - 1].iter().all(|s| s.end - s.start >= 0.8 - 1e-9), "no blips at scale");
    assert!(shots.iter().any(|s| matches!(s.layout, Layout::Split)), "the sides meet and split");
}

#[test]
fn the_signals_candidates_scale_with_many_lanes() {
    use crate::signals::{candidates, pulses};
    let hop = 0.02;
    let lanes: Vec<_> = (0..12).map(|k| {
        let times: Vec<f64> = (0..300).map(|i| i as f64 * 2.0 + k as f64 * 0.15).collect();
        pulses(&format!("lane {k}"), &times, 600.0, hop, 0.1)
    }).collect();
    let c = timed("candidates, 12 lanes x 10 min", Duration::from_secs(20), || candidates(&lanes, &[], 600.0, hop, 0.05, 0.5));
    assert!(c.len() > 250 && c.windows(2).all(|w| w[1].at - w[0].at >= 0.5 - 1e-9), "{}", c.len());
}
