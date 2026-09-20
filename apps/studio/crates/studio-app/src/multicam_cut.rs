//! multicam_cut — cutting a camera group to what a control channel is doing.
//!
//! The palette's beat-grid multicam needs nothing but the project. This one
//! needs the MIX: a control channel (a silent bus — see
//! `studio_core::Bus::silent`) is rendered, its level becomes the driver,
//! and loud passages get the busier angle. That is why it lives in the app
//! rather than in `studio_video::multicam`: only the shell holds the decoded
//! audio.
//!
//! **Which angle is "busier"** is the group's own order. Measuring each
//! angle's motion is the better answer (`studio_video::multicam::motion_energy`
//! over a decoded path) but it costs a decode per angle, so until something
//! measures them in the background, the order the operator put the angles in
//! IS the answer: first is calmest, last is busiest. The status line says so,
//! because a silent guess about which angle is which would be worse than an
//! announced convention.

use std::collections::BTreeMap;

use studio_audio::SourceAudio;
use studio_core::{BusId, GroupId, Op, Project, SourceId, TrackId};
use studio_video::multicam::{self, Choice, Control, Rules, Timing};

/// Level hop: 50 ms. Short enough to catch a drop, long enough that one
/// transient does not move the camera.
pub const HOP: f64 = 0.05;

/// The cut, as ops: a new video track and one clip per shot, ready to apply
/// as a single `Op::Group` so one undo takes the whole thing back.
///
/// `Err` describes what the operator has to fix, in their words — an empty
/// result would leave them guessing which of the group, the bus or the mix
/// was the problem.
pub fn ops(project: &Project, audio: &BTreeMap<SourceId, SourceAudio>,
           group: GroupId, bus: BusId, rules: Rules) -> Result<(Vec<Op>, String), String> {
    // Group order IS the calm-to-busy ranking (see the module note): angles
    // come out of `from_group` in the group's own order and `plan` ranks
    // with a stable sort, so equal energies keep it. Assigning energy = index
    // here would be a no-op -- it was, until a break-check showed the test
    // could not tell the two apart.
    let (angles, start, end) = multicam::from_group(project, group)?;
    let signal = studio_audio::bus_signal(project, audio, &BTreeMap::new(), start, end, 48_000, bus)
        .ok_or("that control channel is muted or gone")?;
    if signal.iter().all(|s| *s == 0.0) {
        return Err("the control channel is silent over this span — send something into it first".into());
    }
    let mono: Vec<f32> = signal.chunks(2).map(|p| (p[0] + p.get(1).copied().unwrap_or(0.0)) * 0.5).collect();
    let control = Control::from_samples(&mono, 48_000, HOP);
    let span = (end.0 - start.0) as f64 / studio_core::TICKS_PER_SECOND as f64;
    let shots = multicam::plan(&angles, &Timing::Free, &Choice::ByLevel(control), rules, span);
    let track = TrackId(project.tracks.iter().map(|t| t.id.0).max().unwrap_or(0) + 1);
    let first_clip = project.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1;
    let mut out = vec![Op::AddTrack { track: studio_core::Track {
        id: track, kind: studio_core::TrackKind::Video, name: "multicam (level)".into(),
        muted: false, locked: false, gain: 1.0, route: None }, at: project.tracks.len() }];
    out.extend(multicam::layout(&shots, &angles, track, start, first_clip));
    let note = format!("{} cuts across {} angles, quiet→busy in group order",
                       shots.len(), angles.len());
    Ok((out, note))
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Clip, ClipId, Send, Source, SourceKind, Ticks, Track, TrackKind};

    /// Two angles over the same 8 s, and a track of audio that is quiet for
    /// the first half and loud for the second, routed into a silent bus.
    fn project() -> (Project, BTreeMap<SourceId, SourceAudio>) {
        let mut p = Project::default();
        let mut audio = BTreeMap::new();
        for id in 1..=2u64 {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(id), kind: TrackKind::Video,
                name: format!("angle {id}"), muted: false, locked: false, gain: 1.0, route: None },
                at: (id - 1) as usize }).unwrap();
            p.apply(&Op::AddSource { source: Source { id: SourceId(id), path: format!("/card/C{id}.mkv"),
                len: Ticks::from_seconds(60.0), sample_rate: 48_000, channels: 2, has_video: true,
                kind: SourceKind::File } }).unwrap();
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(id), track: TrackId(id),
                pos: Ticks::ZERO, src_in: Ticks::from_seconds(id as f64), len: Ticks::from_seconds(8.0),
                gain: 1.0, effects: vec![] } }).unwrap();
        }
        p.apply(&Op::SetGroup { id: GroupId(1), name: "cam".into(), clips: vec![ClipId(1), ClipId(2)] }).unwrap();

        p.apply(&Op::AddTrack { track: Track { id: TrackId(3), kind: TrackKind::Audio, name: "music".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 2 }).unwrap();
        let n = 48_000 * 8;
        let wave: Vec<f32> = (0..n).map(|i| {
            let loud = if i > n / 2 { 0.7 } else { 0.03 };
            loud * (i as f32 * 0.05).sin()
        }).collect();
        audio.insert(SourceId(3), SourceAudio { sample_rate: 48_000, channels: vec![wave.clone(), wave] });
        p.apply(&Op::AddSource { source: Source { id: SourceId(3), path: "/music.wav".into(),
            len: Ticks::from_seconds(8.0), sample_rate: 48_000, channels: 2, has_video: false,
            kind: SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(3), source: SourceId(3), track: TrackId(3),
            pos: Ticks::ZERO, src_in: Ticks::ZERO, len: Ticks::from_seconds(8.0), gain: 1.0,
            effects: vec![] } }).unwrap();

        // music -> Mix (audible) -> a pre-fader send into a silent Control
        p.apply(&Op::AddBus { bus: studio_core::Bus { id: BusId(1), name: "Mix".into(), parent: None,
            gain: 1.0, chain: vec![], sends: vec![Send { to: BusId(2), level: 1.0, pre_fader: true }],
            muted: false, silent: false } }).unwrap();
        p.apply(&Op::AddBus { bus: studio_core::Bus { id: BusId(2), name: "Control".into(), parent: None,
            gain: 1.0, chain: vec![], sends: vec![], muted: false, silent: true } }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(3), bus: Some(BusId(1)) }).unwrap();
        (p, audio)
    }

    const RULES: Rules = Rules { min_shot: 1.0, hysteresis_db: 3.0, settle: 0.3 };

    #[test]
    fn the_quiet_half_gets_the_first_angle_and_the_loud_half_the_last() {
        let (p, audio) = project();
        let (plan_ops, note) = super::ops(&p, &audio, GroupId(1), BusId(2), RULES).expect("cuts");
        assert!(note.contains("angles"), "{note}");
        let mut p2 = p.clone();
        let inv = p2.apply(&Op::Group { ops: plan_ops }).expect("applies");
        let mut cuts: Vec<&Clip> = p2.clips.values().filter(|c| c.track == TrackId(4)).collect();
        cuts.sort_by_key(|c| c.pos);
        assert!(cuts.len() >= 2, "the level changes once: {} cuts", cuts.len());
        assert_eq!(cuts[0].source, SourceId(1), "quiet -> the calm angle (first in the group)");
        assert_eq!(cuts.last().unwrap().source, SourceId(2), "loud -> the busy angle");
        let switch = cuts[1].pos.to_seconds();
        assert!((switch - 4.0).abs() <= 0.6, "the cut lands near the level change: {switch}");
        // each cut still reads its own moment in its own file
        for c in &cuts {
            let offset = if c.source == SourceId(1) { 1.0 } else { 2.0 };
            assert!((c.src_in.to_seconds() - c.pos.to_seconds() - offset).abs() < 1e-6, "{c:?}");
        }
        p2.apply(&inv).unwrap();
        assert_eq!(p2.clips.len(), 3, "one undo takes the whole cut back");

        // The ranking really is the group's order: swap it and the loud half
        // swaps with it. (Without this the test passed with every angle
        // ranked equal — a stable sort kept the same answer by accident.)
        let mut swapped = p.clone();
        swapped.apply(&Op::SetGroup { id: GroupId(1), name: "cam".into(),
                                      clips: vec![ClipId(2), ClipId(1)] }).unwrap();
        let (ops2, _) = super::ops(&swapped, &audio, GroupId(1), BusId(2), RULES).unwrap();
        swapped.apply(&Op::Group { ops: ops2 }).unwrap();
        let mut cuts2: Vec<&Clip> = swapped.clips.values().filter(|c| c.track == TrackId(4)).collect();
        cuts2.sort_by_key(|c| c.pos);
        assert_eq!(cuts2[0].source, SourceId(2), "now the second angle is the calm one");
        assert_eq!(cuts2.last().unwrap().source, SourceId(1));
    }

    #[test]
    fn it_says_what_is_wrong_rather_than_returning_nothing() {
        let (p, audio) = project();
        // a bus with nothing sent into it
        let mut quiet = p.clone();
        quiet.apply(&Op::SetBusSends { id: BusId(1), sends: vec![] }).unwrap();
        assert!(super::ops(&quiet, &audio, GroupId(1), BusId(2), RULES).unwrap_err().contains("silent"));
        // a muted control channel
        let mut muted = p.clone();
        muted.apply(&Op::SetBusFlags { id: BusId(2), muted: true }).unwrap();
        assert!(super::ops(&muted, &audio, GroupId(1), BusId(2), RULES).unwrap_err().contains("muted"));
        // a group that is not a camera group
        let mut lonely = p.clone();
        lonely.apply(&Op::SetGroup { id: GroupId(1), name: "cam".into(), clips: vec![ClipId(1)] }).unwrap();
        assert!(super::ops(&lonely, &audio, GroupId(1), BusId(2), RULES).is_err());
    }
}
