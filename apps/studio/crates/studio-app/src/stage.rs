//! stage — a scene's arc as a signal that music, sound effects and rhythm answer.
//!
//! The question behind it: where does "this scene is building" live so that
//! it cannot drift out of step with the scene? Not in absolute automation
//! (it stays behind when the scene moves), and not as three hand-drawn rides
//! (they drift apart the first time one is edited). Here:
//!
//! * the SCENE is a composition of its clips, so it moves and grows with them;
//! * its STAGE is one curve, gated to that composition and fitted to its span,
//!   so a re-trim shortens the arc instead of cutting it off;
//! * each BUS gets a binding with its own response to that one curve.
//!
//! Editing the arc edits every answer at once; moving or trimming the scene
//! carries the arc with it. There is no dummy clip and nothing to resync.

use studio_core::{AutoTarget, Binding, BindingId, BusId, ClipId, CurveId, Gate, GroupId, Op, Project, Response, Ticks};

pub struct Preset { pub label: &'static str, pub what: &'static str, pub response: Response }

/// How a bus answers the stage. The arc itself runs 0 (scene start) to 1
/// (its peak, at the scene's end).
pub const PRESETS: &[Preset] = &[
    Preset { label: "enter early (music)", what: "fades in over the first half of the scene",
             response: Response { from: 0.05, to: 0.5, floor: 0.0, ceil: 1.0 } },
    Preset { label: "enter late (rhythm)", what: "silent until the build, full at the peak",
             response: Response { from: 0.55, to: 0.9, floor: 0.0, ceil: 1.0 } },
    Preset { label: "strengthen (sfx)", what: "present throughout, 6 dB up by the peak",
             response: Response { from: 0.0, to: 1.0, floor: 0.5, ceil: 1.0 } },
    Preset { label: "ease out", what: "full at the start, down to a third at the peak",
             response: Response { from: 0.3, to: 1.0, floor: 1.0, ceil: 0.35 } },
];

/// The ops that make `clips` a staged scene (or reuse the one they already
/// form) and bind `bus` to its stage through `preset`. Returns the ops and a
/// line for the status bar.
pub fn bind_ops(project: &Project, clips: &[ClipId], bus: BusId, preset: usize) -> Result<(Vec<Op>, String), String> {
    let mut sel: Vec<ClipId> = clips.iter().copied().filter(|c| project.clips.contains_key(c)).collect();
    if sel.is_empty() { return Err("select the scene's clips first".into()); }
    if !project.busses.contains_key(&bus) { return Err("that bus is gone".into()); }
    let p = PRESETS.get(preset).ok_or("no such response")?;
    sel.sort();
    let mut ops = Vec::new();
    // reuse the scene if these exact clips already form a staged composition
    let existing = project.groups.iter().find(|(_, g)| {
        let mut c = g.clips.clone(); c.sort(); c == sel && g.name.starts_with("stage")
    }).map(|(id, _)| *id);
    let group = match existing {
        Some(g) => g,
        None => {
            let g = GroupId(project.groups.keys().map(|g| g.0).max().unwrap_or(0) + 1);
            ops.push(Op::SetGroup { id: g, name: format!("stage {}", g.0), clips: sel.clone() });
            g
        }
    };
    // one curve per scene, found by the binding that already uses it
    let curve = project.bindings.values().find(|b| b.gate == Gate::Group(group)).map(|b| b.curve);
    let curve = match curve {
        Some(c) => c,
        None => {
            let c = CurveId(project.curves.keys().map(|c| c.0).max().unwrap_or(0) + 1);
            ops.push(Op::SetCurve { id: c, name: format!("stage {}", group.0) });
            // a plain rising arc; reshape it by editing the curve's points
            ops.push(Op::SetCurvePoint { curve: c, at: Ticks::ZERO, value: 0.0 });
            ops.push(Op::SetCurvePoint { curve: c, at: Ticks::from_seconds(10.0), value: 1.0 });
            c
        }
    };
    let target = AutoTarget::BusGain(bus);
    // one answer per bus per scene: re-choosing replaces it
    let id = project.bindings.iter().find(|(_, b)| b.gate == Gate::Group(group) && b.target == target)
        .map(|(id, _)| *id)
        .unwrap_or_else(|| BindingId(project.bindings.keys().map(|b| b.0).max().unwrap_or(0) + 1));
    ops.push(Op::SetBinding { id, binding: Binding { curve, target, gate: Gate::Group(group),
        response: Some(p.response), fit_span: true } });
    let bus_name = &project.busses[&bus].name;
    Ok((ops, format!("stage: '{bus_name}' will {} across this scene", p.label.split(" (").next().unwrap_or(p.label))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::*;

    fn project() -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio, name: "A".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source::file(SourceId(1), "a.wav", Ticks::from_seconds(60.0), 48_000, 2, false) }).unwrap();
        for (i, pos) in [(1u64, 10.0), (2, 14.0), (3, 18.0)] {
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(i), source: SourceId(1), track: TrackId(1),
                pos: Ticks::from_seconds(pos), src_in: Ticks::ZERO, len: Ticks::from_seconds(4.0), gain: 1.0, effects: vec![] } }).unwrap();
        }
        for (i, n) in [(1u64, "Music"), (2, "Rhythm")] {
            p.apply(&Op::AddBus { bus: Bus { id: BusId(i), name: n.into(), parent: None, gain: 1.0,
                chain: vec![], sends: vec![], muted: false, silent: false } }).unwrap();
        }
        p
    }

    #[test]
    fn one_scene_one_arc_and_each_bus_answers_it() {
        let mut p = project();
        let scene = [ClipId(1), ClipId(2), ClipId(3)];      // 10 s .. 22 s
        let (ops, _) = bind_ops(&p, &scene, BusId(1), 0).unwrap();   // music enters early
        p.apply(&Op::Group { ops }).unwrap();
        let (ops, _) = bind_ops(&p, &scene, BusId(2), 1).unwrap();   // rhythm enters late
        p.apply(&Op::Group { ops }).unwrap();
        assert_eq!(p.curves.len(), 1, "one arc for the scene, shared");
        assert_eq!(p.groups.len(), 1, "the scene is one composition, reused");
        let v = |bus, s| p.auto_value(AutoTarget::BusGain(BusId(bus)), Ticks::from_seconds(s));
        assert!(v(1, 16.5) > 0.95 && v(2, 16.5) < 0.05, "mid-scene: music in, rhythm not yet");
        assert!(v(2, 21.8) > 0.95, "rhythm arrives at the peak");
        assert_eq!((v(1, 5.0), v(2, 30.0)), (1.0, 1.0), "outside the scene nothing is touched");
    }

    #[test]
    fn moving_or_trimming_the_scene_carries_the_arc() {
        let mut p = project();
        let scene = [ClipId(1), ClipId(2), ClipId(3)];
        let (ops, _) = bind_ops(&p, &scene, BusId(2), 1).unwrap();
        p.apply(&Op::Group { ops }).unwrap();
        // move the whole scene 30 s later: the build moves with it
        for (i, pos) in [(1u64, 40.0), (2, 44.0), (3, 48.0)] {
            p.apply(&Op::MoveClip { id: ClipId(i), pos: Ticks::from_seconds(pos), track: TrackId(1) }).unwrap();
        }
        let v = |p: &Project, s| p.auto_value(AutoTarget::BusGain(BusId(2)), Ticks::from_seconds(s));
        assert!(v(&p, 51.8) > 0.95 && v(&p, 46.5) < 0.05, "the peak is at the moved scene's end");
        // trim the last clip away: the scene is now 8 s and still peaks at its end
        p.apply(&Op::RemoveClip { id: ClipId(3) }).unwrap();
        assert!(v(&p, 47.9) > 0.95, "fitted: the shorter scene still builds to its end");
    }

    #[test]
    fn choosing_again_replaces_the_answer_rather_than_stacking_it() {
        let mut p = project();
        let scene = [ClipId(1), ClipId(2)];
        for preset in [0, 3, 1] {
            let (ops, _) = bind_ops(&p, &scene, BusId(1), preset).unwrap();
            p.apply(&Op::Group { ops }).unwrap();
        }
        assert_eq!(p.bindings.len(), 1, "one answer per bus per scene");
        assert!(bind_ops(&p, &[], BusId(1), 0).is_err());
    }
}
