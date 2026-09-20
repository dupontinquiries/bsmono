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

use studio_core::{Clip, ClipId, Op, SourceId, Ticks, TrackId};

/// One angle of the group.
#[derive(Debug, Clone, PartialEq)]
pub struct Angle {
    pub name: String,
    /// How busy the picture is — see [`motion_energy`]. Only the ORDER of
    /// angles by energy matters.
    pub energy: f32,
    /// The angle's file, and where the group's time zero falls in it (the
    /// sync offset, seconds).
    pub source: SourceId,
    pub offset: f64,
}

/// A picture's motion energy from its measured camera path
/// ([`crate::stabilize::measure`]): mean step length per frame. A locked-off
/// wide is near 0; a handheld chase is large.
pub fn motion_energy(path: &[(f32, f32)]) -> f32 {
    if path.len() < 2 { return 0.0; }
    path.windows(2).map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt())
        .sum::<f32>() / (path.len() - 1) as f32
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
            src_in: Ticks::from_seconds(s.start + a.offset),
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
        assert_eq!(clips[0].src_in, Ticks::from_seconds(0.5), "wide's sync offset");
        assert_eq!(clips[1].src_in, Ticks::from_seconds(4.0), "handheld: 2 s into the group + 2 s offset");
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

    #[test]
    fn motion_energy_ranks_a_locked_shot_below_a_handheld_one() {
        let locked: Vec<(f32, f32)> = (0..24).map(|_| (0.0, 0.0)).collect();
        let hand: Vec<(f32, f32)> = (0..24).map(|i| ((i % 3) as f32 * 2.0, (i % 2) as f32)).collect();
        assert!(motion_energy(&locked) < 1e-6 && motion_energy(&hand) > 1.0);
    }
}
