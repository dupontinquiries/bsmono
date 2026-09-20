//! multicam — cutting between synced angles, driven by the music.
//!
//! A multicam group is several takes of the same moment, already synced
//! (by [`crate::dualsync`] or a slate). Editing it by hand is choosing, at
//! every moment, which angle is on screen. This module makes that choice
//! from a CONTROL signal, the way FL Studio's mixer sidechains a compressor
//! to a kick: any channel's level can drive the cut.
//!
//! * **Timing** — when a cut may happen: freely, or only on beats
//!   ([`Timing::OnBeats`], every Nth beat).
//! * **Choice** — which angle to cut to: by the control channel's level
//!   ([`Choice::ByLevel`]: quiet passages get the calm angle, loud passages
//!   the busy one — angles are ranked by their measured motion, see
//!   [`motion_energy`]), or round-robin ([`Choice::Cycle`]).
//! * **Rules** — a minimum shot length, so nothing flickers, and level
//!   hysteresis, so a signal hovering at a threshold does not chatter.
//!
//! The control channel is any rendered audio: a stem, a bus, or a "dummy"
//! channel that is routed nowhere audible and exists only to drive cuts —
//! the video equivalent of a ghost kick on a sidechain. Studio's mixer
//! routing (sends, busses) is where that channel will live; this module
//! only needs its samples ([`Control::from_samples`]).
//!
//! The output is a plan ([`Shot`]s) and, through [`layout`], ordinary
//! `AddClip` ops — so a generated cut is undoable, shareable, and editable
//! by hand afterwards like any other.

use studio_core::{Clip, ClipId, Op, SourceId, Ticks, TrackId, TICKS_PER_SECOND};

/// One angle of the group.
#[derive(Debug, Clone, PartialEq)]
pub struct Angle {
    pub name: String,
    /// How busy the picture is — see [`motion_energy`]. Only the ORDER of
    /// angles by energy matters.
    pub energy: f32,
    /// The angle's file, and its sync offset: the position IN THE FILE that
    /// timeline zero corresponds to, in seconds (`src_in - pos` for a clip
    /// already placed in sync). A cut at timeline `t` therefore reads the
    /// file at `t + offset`.
    pub source: SourceId,
    pub offset: f64,
}

/// A picture's motion energy from its measured camera path
/// ([`crate::stabilize::measure`]): mean step length per frame. A locked-off
/// wide is near 0; a handheld chase is large.
pub fn motion_energy(path: &[crate::stabilize::Pose]) -> f32 {
    if path.len() < 2 { return 0.0; }
    path.windows(2).map(|w| ((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt())
        .sum::<f32>() / (path.len() - 1) as f32
}

/// The angles of a camera group as the project holds them, plus the span
/// of timeline they all cover.
///
/// A camera group is an ordinary composition ([`studio_core::Group`]) whose
/// clips are the angles, already synced — each one placed so that the same
/// moment sits at the same timeline position, which is what
/// [`crate::dualsync`] and the slate exist to establish. An angle's sync
/// offset is then just `src_in - pos`: how far into its own file the file
/// was at timeline zero.
///
/// The span is the overlap of every angle, because a plan may only cut
/// between angles where all of them actually have picture. `Err` when the
/// group is empty, when a clip's source is missing, or when the angles do
/// not overlap at all — a "camera group" whose members do not share any
/// moment is a mistake worth refusing rather than cutting into.
pub fn from_group(project: &studio_core::Project, group: studio_core::GroupId)
    -> Result<(Vec<Angle>, Ticks, Ticks), String> {
    let g = project.groups.get(&group).ok_or("no such group")?;
    let mut angles = Vec::new();
    let (mut start, mut end) = (Ticks(i64::MIN), Ticks(i64::MAX));
    for id in &g.clips {
        let Some(c) = project.clips.get(id) else { continue };
        let src = project.sources.get(&c.source).ok_or("an angle's source is missing")?;
        if !src.has_video { continue; }
        start = start.max(c.pos);
        end = end.min(c.pos.saturating_add(c.len));
        angles.push(Angle {
            name: src.path.rsplit('/').next().unwrap_or(&src.path).to_string(),
            energy: 0.0,
            source: c.source,
            offset: (c.src_in.0 - c.pos.0) as f64 / TICKS_PER_SECOND as f64,
        });
    }
    if angles.len() < 2 { return Err("a camera group needs at least two angles with picture".into()); }
    if end <= start { return Err("the angles do not overlap: nothing to cut between".into()); }
    Ok((angles, start, end))
}

/// Beat times in seconds over `[from, to)`, from a tempo map — the grid a
/// [`Timing::OnBeats`] plan cuts on when the music is the project's own
/// rather than a rendered track.
pub fn beats_from_tempo(tempo: &studio_core::TempoMap, from: Ticks, to: Ticks) -> Vec<f64> {
    let first = tempo.tick_to_beat(from).ceil();
    let mut out = Vec::new();
    let mut b = first;
    loop {
        let t = tempo.beat_to_tick(b);
        if t >= to { break; }
        out.push((t.0 - from.0) as f64 / TICKS_PER_SECOND as f64);
        b += 1.0;
        if out.len() > 100_000 { break; }
    }
    out
}

/// A control channel's level over time: dB per hop.
#[derive(Debug, Clone, PartialEq)]
pub struct Control {
    pub db: Vec<f32>,
    pub hop: f64,
}

impl Control {
    /// RMS level per `hop` seconds of mono samples, in dBFS (floored at -90).
    pub fn from_samples(samples: &[f32], rate: u32, hop: f64) -> Control {
        let n = ((rate as f64 * hop).round() as usize).max(1);
        let db = samples.chunks(n).map(|c| {
            let ms = c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32;
            (10.0 * ms.max(1e-9).log10()).max(-90.0)
        }).collect();
        Control { db, hop }
    }

    /// Level at time t, averaged over the preceding `window` seconds.
    fn at(&self, t: f64, window: f64) -> f32 {
        let i1 = ((t / self.hop) as usize).min(self.db.len());
        let i0 = i1.saturating_sub(((window / self.hop).round() as usize).max(1));
        let s = &self.db[i0..i1.max(i0 + 1).min(self.db.len())];
        if s.is_empty() { -90.0 } else { s.iter().sum::<f32>() / s.len() as f32 }
    }

    /// The level range worth mapping: 10th to 90th percentile, so a
    /// silent intro or one peak does not squash every band into a sliver.
    fn range(&self) -> (f32, f32) {
        let mut v = self.db.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        if v.is_empty() { return (-90.0, 0.0); }
        (v[v.len() / 10], v[(v.len() * 9 / 10).min(v.len() - 1)])
    }
}

/// When a cut may happen.
#[derive(Debug, Clone, PartialEq)]
pub enum Timing {
    /// At any control hop.
    Free,
    /// Only on beats (seconds), and only every `every`-th one.
    OnBeats { beats: Vec<f64>, every: usize },
}

/// Which angle to cut to.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    /// Louder → busier angle.
    ByLevel(Control),
    /// Next angle in the order given.
    Cycle,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rules {
    /// No shot shorter than this (seconds), except the last.
    pub min_shot: f64,
    /// A level must cross a band edge by this many dB to change band.
    pub hysteresis_db: f32,
    /// A new band must hold this long (seconds) before it counts. The cut is
    /// then placed where the band STARTED — the plan is offline, so it can
    /// look ahead. Without this a jump from quiet to loud passes through the
    /// middle band on its way (the level is averaged) and cuts to the middle
    /// angle for a minimum-shot's length first.
    pub settle: f64,
}

/// One stretch of the cut: angle index into the group, [start, end) seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Shot {
    pub angle: usize,
    pub start: f64,
    pub end: f64,
}

/// Plan a cut over [0, length) seconds. Shots are contiguous and cover the
/// whole length.
pub fn plan(angles: &[Angle], timing: &Timing, choice: &Choice, rules: Rules, length: f64) -> Vec<Shot> {
    if angles.is_empty() || length <= 0.0 { return Vec::new(); }
    // angles from calmest to busiest
    let mut by_energy: Vec<usize> = (0..angles.len()).collect();
    by_energy.sort_by(|&a, &b| angles[a].energy.partial_cmp(&angles[b].energy).unwrap_or(std::cmp::Ordering::Equal));
    let candidates: Vec<f64> = match (timing, choice) {
        (Timing::OnBeats { beats, every }, _) =>
            beats.iter().step_by((*every).max(1)).copied().filter(|&t| t > 0.0 && t < length).collect(),
        (Timing::Free, Choice::ByLevel(c)) =>
            (1..).map(|i| i as f64 * c.hop).take_while(|&t| t < length).collect(),
        (Timing::Free, Choice::Cycle) =>
            (1..).map(|i| i as f64 * rules.min_shot.max(0.1)).take_while(|&t| t < length).collect(),
    };
    let bands = angles.len();
    // Level → band per control hop, with hysteresis, then settle.
    let desired: Option<(Vec<usize>, f64)> = match choice {
        Choice::ByLevel(c) => {
            let (lo, hi) = c.range();
            let per = (hi - lo).max(1e-3) / bands as f32;
            let h = rules.hysteresis_db / per;  // hysteresis in band units
            let pos = |i: usize| ((c.at((i + 1) as f64 * c.hop, 0.25) - lo) / per)
                .clamp(0.0, bands as f32 - 1e-3);
            let n = c.db.len();
            let mut accepted = if n > 0 { pos(0) as usize } else { 0 };
            let mut out = vec![accepted; n];
            let settle_hops = ((rules.settle / c.hop).round() as usize).max(1);
            let mut run: Option<(usize, usize)> = None;  // (band, first hop)
            for i in 0..n {
                out[i] = accepted;  // a pending run is written back only if it settles
                let p = pos(i);
                let proposal = if p >= accepted as f32 + 1.0 + h || p < accepted as f32 - h { p as usize } else { accepted };
                run = match run {
                    Some((b, from)) if b == proposal => Some((b, from)),
                    _ if proposal != accepted => Some((proposal, i)),
                    _ => None,
                };
                if let Some((b, from)) = run {
                    if i + 1 - from >= settle_hops {
                        for o in out.iter_mut().take(i + 1).skip(from) { *o = b; }
                        accepted = b;
                        run = None;
                    }
                }
            }
            Some((out, c.hop))
        }
        Choice::Cycle => None,
    };
    let band_at = |t: f64| -> usize {
        let (d, hop) = desired.as_ref().expect("level mode");
        d[((t / hop) as usize).min(d.len().saturating_sub(1))]
    };
    let mut current = match &desired { Some(_) => by_energy[band_at(0.0)], None => 0 };
    let mut shots = vec![Shot { angle: current, start: 0.0, end: length }];
    let mut cycle_pos = 0usize;
    for t in candidates {
        let want = if desired.is_some() {
            by_energy[band_at(t)]
        } else {
            cycle_pos = (cycle_pos + 1) % angles.len();
            cycle_pos
        };
        let last = shots.last_mut().unwrap();
        if want != current && t - last.start >= rules.min_shot {
            last.end = t;
            shots.push(Shot { angle: want, start: t, end: length });
            current = want;
        }
    }
    shots
}

/// The plan as timeline ops: one `AddClip` per shot on `track`, starting at
/// `at` on the timeline, each cut from its angle's file at the synced point.
/// Clip ids are allocated from `first_id` upward.
pub fn layout(shots: &[Shot], angles: &[Angle], track: TrackId, at: Ticks, first_id: u64) -> Vec<Op> {
    shots.iter().enumerate().map(|(i, s)| {
        let a = &angles[s.angle];
        Op::AddClip { clip: Clip {
            id: ClipId(first_id + i as u64),
            source: a.source,
            track,
            pos: at.saturating_add(Ticks::from_seconds(s.start)),
            // The shot's TIMELINE position, converted into the angle's own
            // file time. Using the plan-relative start here read the wrong
            // moment whenever the cut did not begin at timeline zero.
            src_in: Ticks::from_seconds(at.to_seconds() + s.start + a.offset),
            len: Ticks::from_seconds(s.end - s.start),
            gain: 1.0,
            effects: Vec::new(),
        } }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group() -> Vec<Angle> {
        vec![
            Angle { name: "handheld".into(), energy: 9.0, source: SourceId(1), offset: 2.0 },
            Angle { name: "wide".into(), energy: 0.3, source: SourceId(2), offset: 0.5 },
            Angle { name: "dolly".into(), energy: 3.0, source: SourceId(3), offset: 1.0 },
        ]
    }

    /// A control channel: `(seconds, dB)` segments at a 50 ms hop.
    fn control(parts: &[(f64, f32)]) -> Control {
        let hop = 0.05;
        let db = parts.iter().flat_map(|&(secs, db)| std::iter::repeat(db).take((secs / hop) as usize)).collect();
        Control { db, hop }
    }

    fn covers(shots: &[Shot], length: f64) {
        assert_eq!(shots[0].start, 0.0);
        assert!((shots.last().unwrap().end - length).abs() < 1e-9);
        for w in shots.windows(2) { assert_eq!(w[0].end, w[1].start); assert_ne!(w[0].angle, w[1].angle); }
    }

    const RULES: Rules = Rules { min_shot: 1.0, hysteresis_db: 2.0, settle: 0.3 };

    #[test]
    fn quiet_gets_the_calm_angle_and_loud_gets_the_busy_one() {
        let c = control(&[(4.0, -40.0), (4.0, -8.0), (4.0, -40.0)]);
        let shots = plan(&group(), &Timing::Free, &Choice::ByLevel(c), RULES, 12.0);
        covers(&shots, 12.0);
        let g = group();
        let names: Vec<&str> = shots.iter().map(|s| g[s.angle].name.as_str()).collect();
        assert_eq!(names, vec!["wide", "handheld", "wide"], "{shots:?}");
        // cuts land within the 0.25 s level window of the real change
        assert!((shots[1].start - 4.0).abs() <= 0.3 && (shots[2].start - 8.0).abs() <= 0.3, "{shots:?}");
    }

    #[test]
    fn no_shot_is_shorter_than_the_minimum() {
        // the level flips every 0.3 s — a naive switcher would flicker
        let parts: Vec<(f64, f32)> = (0..40).map(|i| (0.3, if i % 2 == 0 { -40.0 } else { -6.0 })).collect();
        let shots = plan(&group(), &Timing::Free, &Choice::ByLevel(control(&parts)), RULES, 12.0);
        covers(&shots, 12.0);
        for s in &shots[..shots.len() - 1] {
            assert!(s.end - s.start >= RULES.min_shot - 1e-9, "{s:?} in {shots:?}");
        }
    }

    #[test]
    fn a_level_hovering_at_an_edge_does_not_chatter() {
        // range -40..-4 dB over 3 bands → edges at -28 and -16; hover at -16 ± 0.5
        let mut parts = vec![(1.0, -40.0), (1.0, -4.0)];
        parts.extend((0..30).map(|i| (0.3, if i % 2 == 0 { -15.5 } else { -16.5 })));
        let c = control(&parts);
        let with = plan(&group(), &Timing::Free, &Choice::ByLevel(c.clone()), Rules { min_shot: 0.0, hysteresis_db: 2.0, settle: 0.0 }, 11.0);
        let without = plan(&group(), &Timing::Free, &Choice::ByLevel(c), Rules { min_shot: 0.0, hysteresis_db: 0.0, settle: 0.0 }, 11.0);
        let after = |v: &[Shot]| v.iter().filter(|s| s.start > 2.5).count();
        assert!(after(&without) > 5, "the fixture must actually chatter without hysteresis: {without:?}");
        assert!(after(&with) <= 1, "{with:?}");
    }

    #[test]
    fn beat_timing_cuts_only_on_the_chosen_beats() {
        let beats: Vec<f64> = (0..32).map(|i| i as f64 * 0.5).collect();  // 120 bpm
        let shots = plan(&group(), &Timing::OnBeats { beats, every: 4 }, &Choice::Cycle,
                         Rules { min_shot: 0.0, hysteresis_db: 0.0, settle: 0.0 }, 16.0);
        covers(&shots, 16.0);
        for s in &shots[1..] { assert!((s.start / 2.0).fract().abs() < 1e-9, "every 4th beat = every 2 s: {s:?}"); }
        assert_eq!(shots.iter().map(|s| s.angle).take(4).collect::<Vec<_>>(), vec![0, 1, 2, 0]);
    }

    #[test]
    fn beats_and_level_combine() {
        let beats: Vec<f64> = (0..24).map(|i| i as f64 * 0.5).collect();
        let c = control(&[(5.2, -40.0), (6.8, -8.0)]);
        let shots = plan(&group(), &Timing::OnBeats { beats, every: 2 }, &Choice::ByLevel(c), RULES, 12.0);
        covers(&shots, 12.0);
        assert_eq!(shots.len(), 2, "{shots:?}");
        assert_eq!(shots[1].start, 6.0, "the first allowed beat after the level rises at 5.2 s (+0.25 s window)");
    }

    #[test]
    fn the_plan_lays_out_as_synced_add_clip_ops() {
        let shots = vec![Shot { angle: 1, start: 0.0, end: 2.0 }, Shot { angle: 0, start: 2.0, end: 5.0 }];
        let ops = layout(&shots, &group(), TrackId(7), Ticks::from_seconds(10.0), 100);
        let clips: Vec<&Clip> = ops.iter().map(|o| match o { Op::AddClip { clip } => clip, _ => panic!() }).collect();
        assert_eq!(clips[0].pos, Ticks::from_seconds(10.0));
        assert_eq!(clips[1].pos, Ticks::from_seconds(12.0));
        assert_eq!(clips[0].src_in, Ticks::from_seconds(10.5), "timeline 10 s, wide's offset 0.5");
        assert_eq!(clips[1].src_in, Ticks::from_seconds(14.0), "timeline 12 s, handheld's offset 2");
        assert_eq!((clips[0].id, clips[1].id), (ClipId(100), ClipId(101)));
        // and they apply to a real project without overlap refusals
        let mut p = studio_core::Project::default();
        p.apply(&Op::AddTrack { track: studio_core::Track { id: TrackId(7), kind: studio_core::TrackKind::Video,
            name: "multicam".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        for (id, a) in group().iter().enumerate() {
            p.apply(&Op::AddSource { source: studio_core::Source { id: a.source, path: format!("{id}.mkv"),
                len: Ticks::from_seconds(60.0), sample_rate: 48_000, channels: 2, has_video: true,
                kind: studio_core::SourceKind::File } }).unwrap();
        }
        for o in &ops { p.apply(o).expect("applies"); }
    }

    fn synced_project() -> (studio_core::Project, studio_core::GroupId) {
        use studio_core::*;
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video, name: "angles".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(2), kind: TrackKind::Video, name: "angles 2".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 1 }).unwrap();
        for (i, off) in [2.0f64, 0.5].iter().enumerate() {
            let id = i as u64 + 1;
            p.apply(&Op::AddSource { source: Source { id: SourceId(id), path: format!("/card/C000{id}.mkv"),
                len: Ticks::from_seconds(120.0), sample_rate: 48_000, channels: 2, has_video: true,
                kind: SourceKind::File } }).unwrap();
            // both angles placed so the same moment sits at the same
            // timeline position; the second starts a second later
            let pos = if i == 0 { 10.0 } else { 11.0 };
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(id), track: TrackId(id),
                pos: Ticks::from_seconds(pos), src_in: Ticks::from_seconds(pos + off),
                len: Ticks::from_seconds(20.0), gain: 1.0, effects: vec![] } }).unwrap();
        }
        p.apply(&Op::SetGroup { id: GroupId(1), name: "cam".into(), clips: vec![ClipId(1), ClipId(2)] }).unwrap();
        (p, GroupId(1))
    }

    #[test]
    fn a_camera_group_yields_its_angles_their_offsets_and_the_overlap() {
        let (p, g) = synced_project();
        let (angles, start, end) = from_group(&p, g).expect("a group");
        assert_eq!(angles.len(), 2);
        assert!((angles[0].offset - 2.0).abs() < 1e-6 && (angles[1].offset - 0.5).abs() < 1e-6,
                "src_in - pos is the sync offset: {angles:?}");
        assert_eq!((start, end), (Ticks::from_seconds(11.0), Ticks::from_seconds(30.0)),
                   "the overlap of both angles, not the union");
        // a group with one angle, or none that overlap, is refused
        let mut lonely = p.clone();
        lonely.apply(&Op::SetGroup { id: g, name: "cam".into(), clips: vec![ClipId(1)] }).unwrap();
        assert!(from_group(&lonely, g).is_err());
        let mut apart = p.clone();
        apart.apply(&Op::MoveClip { id: ClipId(2), pos: Ticks::from_seconds(60.0), track: TrackId(2) }).unwrap();
        assert!(from_group(&apart, g).unwrap_err().contains("overlap"));
    }

    #[test]
    fn beats_come_off_the_tempo_map_in_the_span_asked_for() {
        let t = studio_core::TempoMap::constant(120.0);  // 0.5 s a beat
        let b = beats_from_tempo(&t, Ticks::from_seconds(10.0), Ticks::from_seconds(12.0));
        assert_eq!(b.len(), 4, "{b:?}");
        for (i, x) in b.iter().enumerate() { assert!((x - i as f64 * 0.5).abs() < 1e-6, "{b:?}"); }
        // and a plan cut on every second beat lands on whole seconds
        let (p, g) = synced_project();
        let (angles, start, end) = from_group(&p, g).unwrap();
        let span = (end.0 - start.0) as f64 / TICKS_PER_SECOND as f64;
        let beats = beats_from_tempo(&t, start, end);
        let shots = plan(&angles, &Timing::OnBeats { beats, every: 2 }, &Choice::Cycle,
                         Rules { min_shot: 0.0, hysteresis_db: 0.0, settle: 0.0 }, span);
        assert!(shots.len() > 10, "{} shots over {span}s", shots.len());
        for s in &shots[1..] { assert!((s.start.fract()).abs() < 1e-6, "on the beat: {s:?}"); }
        // and laying them out keeps every cut reading the right moment
        let ops = layout(&shots, &angles, TrackId(9), start, 500);
        for (op, s) in ops.iter().zip(&shots) {
            let Op::AddClip { clip } = op else { panic!() };
            let a = &angles[s.angle];
            let moment = clip.src_in.to_seconds() - a.offset;
            assert!((moment - clip.pos.to_seconds()).abs() < 1e-6,
                    "the clip reads its own timeline moment: {clip:?} for {s:?}");
        }
    }

    #[test]
    fn motion_energy_ranks_a_locked_shot_below_a_handheld_one() {
        use crate::stabilize::Pose;
        let locked: Vec<Pose> = (0..24).map(|_| Pose::default()).collect();
        let hand: Vec<Pose> = (0..24).map(|i| Pose { x: (i % 3) as f32 * 2.0, y: (i % 2) as f32, rot: 0.0 }).collect();
        assert!(motion_energy(&locked) < 1e-6 && motion_energy(&hand) > 1.0);
    }
}
