//! curves — reusable automation, bound to clips rather than to the clock.
//!
//! ## The flaw this exists to fix
//!
//! `Project::automation` is one lane per target on the ABSOLUTE timeline —
//! FL Studio's shape, and FL's limitation with it: a ride drawn under a
//! clip belongs to the timeline, not to the clip. Move the clip and the
//! ride stays behind; duplicate the clip and you redraw the ride, then keep
//! the copies in step by hand. The request that prompted this named it
//! exactly: automation would be reusable "if only they were bindable to
//! multiple clips with time-gated selections".
//!
//! So a [`Curve`] is a project object in its own right — named, with points
//! in its OWN time (t = 0 is the curve's start) — and a [`Binding`] is what
//! aims it at a target through a [`Gate`]. One curve, any number of
//! bindings: the same filter sweep on twelve ad-libs is one curve and twelve
//! bindings, and editing a point moves all twelve. A binding gated to a clip
//! is active only while that clip is under the playhead and reads the curve
//! in CLIP-RELATIVE time, so moving the clip moves the ride, and
//! [`Project::duplicate_clip_ops`](crate::Project::duplicate_clip_ops)
//! duplicates the bindings (never the curve) — which is the whole point.
//!
//! ## What a gate is
//!
//! A gate answers two questions and only two: is the binding active at
//! this tick, and where is the curve's origin. [`Gate::Always`] is the old
//! absolute lane (origin 0, always on). [`Gate::Clip`] is on inside the
//! clip's span with origin at `clip.pos`. [`Gate::Window`] is on inside
//! `[start, end)` with origin at `start` — a clip-shaped gate with no clip,
//! for a ride over a span nothing is placed on. There is deliberately no
//! separate "time base" enum: absolute-time-inside-a-clip-gate was
//! considered and dropped, because a ride that does NOT follow its clip is
//! exactly what the plain lane already provides.
//!
//! ## Bindings multiply
//!
//! The render's rule for lanes is that a lane RIDES its fader — multiplied,
//! never substituted (`Project::automation`'s own docs). Bindings follow
//! it: the value driving a target is `lane × Π(active bindings)`, so a
//! project with no bindings renders bit-identically to one from before
//! bindings existed (pinned one crate over), and two bindings on one
//! target compose instead of fighting. The named alternative is
//! last-writer-wins between bindings, which FL users may expect; it was not
//! chosen because it needs an ordering rule nobody drew, and multiply is
//! already the rule everywhere else in this model.
//!
//! ## Dangling is harmless, like everywhere else here
//!
//! A binding whose gate clip was deleted is simply never active; a binding
//! whose curve was deleted cannot exist (`RemoveCurve` refuses while bound,
//! the `RemoveSource` rule). Undoing the clip's removal brings its bindings
//! back to life with nothing else to restore — the same "dangling is
//! cheaper than a second cleanup undo can't reverse" trade the lanes and
//! the groups already make.

use serde::{Deserialize, Serialize};

use crate::{eval_lane, eval_lane_cursor, AutoTarget, BindingId, ClipId, CurveId, Op, Project, Ticks};

/// A reusable automation curve: points in curve-local time, sorted
/// ascending, sparse. Values are the same linear multipliers a lane holds
/// (1.0 is transparent, 0.5 is 6 dB down); an empty curve reads 1.0
/// everywhere, so a freshly created curve changes nothing until a point is
/// placed on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    pub name: String,
    pub points: Vec<(Ticks, f32)>,
}

/// When a binding is active and where its curve's t = 0 sits. See the
/// module docs for why the origin is a property of the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Gate {
    /// Always active, origin at the project start — the shape of a plain
    /// lane, kept so a curve can also be used as one.
    Always,
    /// Active while the clip is under the playhead; origin at the clip's
    /// `pos`, so the ride follows the clip through every move and every
    /// duplicate. A gate on a clip that no longer exists is never active.
    Clip(ClipId),
    /// Active inside `[start, end)`, origin at `start`. A clip-shaped gate
    /// with nothing placed on it — for ducking a bus through a break, say.
    Window { start: Ticks, end: Ticks },
    /// Active over the group's HULL — from its first member's start to
    /// its last member's end — origin at the hull's start. A curve over
    /// a composition: a riser across the whole drop, a filter opening
    /// over a chorus made of twelve clips, following the group as its
    /// members move and growing as they are added. A gate on a group
    /// with no members (or no group) is never active.
    Group(crate::GroupId),
}

/// One curve aimed at one target through one gate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    pub curve: CurveId,
    pub target: AutoTarget,
    pub gate: Gate,
}

/// A binding's gate, resolved against a project into the two facts the
/// render needs: the span it is active over (`None` = always) and its
/// origin. `None` from the resolver means the gate can never open — a
/// missing clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opening {
    pub span: Option<(Ticks, Ticks)>,
    pub origin: Ticks,
}

impl Opening {
    pub fn contains(&self, at: Ticks) -> bool {
        match self.span {
            None => true,
            Some((s, e)) => at >= s && at < e,
        }
    }
}

impl Gate {
    /// Resolve against the project; `None` when the gate names a clip that
    /// is not there (deleted, or not yet arrived from a peer).
    pub fn opening(&self, project: &Project) -> Option<Opening> {
        match *self {
            Gate::Always => Some(Opening { span: None, origin: Ticks::ZERO }),
            Gate::Clip(id) => {
                let c = project.clips.get(&id)?;
                Some(Opening { span: Some((c.pos, c.end())), origin: c.pos })
            }
            Gate::Window { start, end } => {
                if end <= start { return None; }
                Some(Opening { span: Some((start, end)), origin: start })
            }
            Gate::Group(id) => {
                let (start, end) = project.group_hull(id)?;
                Some(Opening { span: Some((start, end)), origin: start })
            }
        }
    }
}

impl Project {
    /// The span a group's members cover — first start to last end — or
    /// `None` for a group with no members (or none at all). Members that
    /// no longer exist as clips are skipped, not counted.
    pub fn group_hull(&self, id: crate::GroupId) -> Option<(Ticks, Ticks)> {
        let g = self.groups.get(&id)?;
        let mut span: Option<(Ticks, Ticks)> = None;
        for c in g.clips.iter().filter_map(|c| self.clips.get(c)) {
            span = Some(match span {
                None => (c.pos, c.end()),
                Some((s, e)) => (s.min(c.pos), e.max(c.end())),
            });
        }
        span.filter(|(s, e)| e > s)
    }
}

/// A binding prepared for a forward sweep: the curve's points, the gate
/// resolved once, and a cursor so a dense curve costs one comparison per
/// sample (the `eval_lane_cursor` rule). Built per render, per target, so
/// the sample loop never touches a map.
#[derive(Debug, Clone)]
pub struct Sweep<'a> {
    points: &'a [(Ticks, f32)],
    opening: Opening,
    cursor: usize,
}

impl<'a> Sweep<'a> {
    /// The binding's multiplier at `at`: 1.0 outside the gate, the curve at
    /// gate-relative time inside it.
    pub fn value(&mut self, at: Ticks) -> f32 {
        if !self.opening.contains(at) { return 1.0; }
        eval_lane_cursor(self.points, at.saturating_sub(self.opening.origin), &mut self.cursor)
    }
}

impl Project {
    /// The bindings aimed at `target`, in id order — deterministic on every
    /// replica, so two peers folding the same bindings multiply in the
    /// same sequence (which matters only for float rounding, and rounding
    /// that differs per peer would be a convergence bug in the render).
    pub fn bindings_for(&self, target: AutoTarget) -> impl Iterator<Item = (BindingId, &Binding)> {
        self.bindings.iter().filter(move |(_, b)| b.target == target).map(|(id, b)| (*id, b))
    }

    /// One binding's multiplier at `at`, with no cursor — the pure
    /// definition the sweep is checked against.
    pub fn binding_value(&self, b: &Binding, at: Ticks) -> f32 {
        let Some(curve) = self.curves.get(&b.curve) else { return 1.0 };
        let Some(opening) = b.gate.opening(self) else { return 1.0 };
        if !opening.contains(at) { return 1.0; }
        eval_lane(&curve.points, at.saturating_sub(opening.origin))
    }

    /// Everything driving `target` at `at`: the lane times every active
    /// binding. A target with neither reads 1.0.
    pub fn auto_value(&self, target: AutoTarget, at: Ticks) -> f32 {
        let lane = match self.automation.get(&target) {
            Some(l) => eval_lane(l, at),
            None => 1.0,
        };
        self.bindings_for(target).fold(lane, |acc, (_, b)| acc * self.binding_value(b, at))
    }

    /// The bindings for `target`, resolved for a forward sweep. Bindings
    /// whose gate can never open (a missing clip) or whose curve is missing
    /// are left out — they would read 1.0 every sample anyway.
    pub fn binding_sweeps(&self, target: AutoTarget) -> Vec<Sweep<'_>> {
        self.bindings_for(target)
            .filter_map(|(_, b)| {
                let curve = self.curves.get(&b.curve)?;
                let opening = b.gate.opening(self)?;
                Some(Sweep { points: &curve.points, opening, cursor: 0 })
            })
            .collect()
    }

    /// Every parameter target ridden on bus `bus`'s chain — the union of
    /// lanes and bindings — as `(effect index, key, target)`, sorted, so
    /// a render can ask once per bus and drive each one at control rate.
    /// Indices past the chain's end are included: the chain is the
    /// caller's to check, and a dangling target is harmless.
    pub fn bus_param_targets(&self, bus: crate::BusId) -> Vec<(u8, crate::ParamKey, AutoTarget)> {
        let mut out: Vec<_> = self.automation.keys().copied()
            .chain(self.bindings.values().map(|b| b.target))
            .filter_map(|t| match t {
                AutoTarget::BusParam { bus: b, effect, key } if b == bus => Some((effect, key, t)),
                _ => None,
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The same for clip `clip`'s effect chain.
    pub fn clip_param_targets(&self, clip: ClipId) -> Vec<(u8, crate::ParamKey, AutoTarget)> {
        let mut out: Vec<_> = self.automation.keys().copied()
            .chain(self.bindings.values().map(|b| b.target))
            .filter_map(|t| match t {
                AutoTarget::ClipParam { clip: c, effect, key } if c == clip => Some((effect, key, t)),
                _ => None,
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The ops that place a copy of clip `id` at (`pos`, `track`) AND
    /// re-bind every curve gated to it onto the copy. Fresh binding ids
    /// are handed out from `next_binding` (max-plus-one is the caller's
    /// convention for every id here). The curves are shared, not copied:
    /// that is what makes the copy's ride follow an edit to the original's.
    ///
    /// Returned as ops rather than applied, the `split_ops` shape, so the
    /// shell can apply them as ONE undo step through its group door.
    pub fn duplicate_clip_ops(&self, id: ClipId, new_id: ClipId, pos: Ticks, track: crate::TrackId,
                              next_binding: &mut u64) -> Result<Vec<Op>, crate::EditError> {
        let src = self.clips.get(&id).ok_or(crate::EditError::NoSuchClip(id))?;
        let mut copy = src.clone();
        copy.id = new_id;
        copy.pos = pos;
        copy.track = track;
        let mut ops = vec![Op::AddClip { clip: copy }];
        for (_, b) in self.bindings.iter() {
            if b.gate == Gate::Clip(id) {
                let bid = BindingId(*next_binding);
                *next_binding += 1;
                ops.push(Op::SetBinding { id: bid, binding: Binding { gate: Gate::Clip(new_id), ..*b } });
            }
        }
        Ok(ops)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, EditError, Source, SourceId, Track, TrackId, TrackKind};

    fn project() -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio, name: "vox".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "a.wav".into(),
            len: Ticks(10_000), sample_rate: 48_000, channels: 1, has_video: false } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1),
            pos: Ticks(1_000), src_in: Ticks::ZERO, len: Ticks(1_000), gain: 1.0, effects: vec![] } }).unwrap();
        p
    }

    fn curve_with_dip(p: &mut Project, id: CurveId) {
        p.apply(&Op::SetCurve { id, name: "dip".into() }).unwrap();
        // 1.0 at t=0, 0.0 at t=500, 1.0 at t=1000 — a V in curve-local time.
        p.apply(&Op::SetCurvePoint { curve: id, at: Ticks(0), value: 1.0 }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: id, at: Ticks(500), value: 0.0 }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: id, at: Ticks(1_000), value: 1.0 }).unwrap();
    }

    const T: AutoTarget = AutoTarget::TrackGain(TrackId(1));

    #[test]
    fn a_curve_gated_to_a_clip_reads_in_clip_time_and_is_silent_outside_it() {
        let mut p = project();
        curve_with_dip(&mut p, CurveId(1));
        p.apply(&Op::SetBinding { id: BindingId(1), binding: Binding {
            curve: CurveId(1), target: T, gate: Gate::Clip(ClipId(1)) } }).unwrap();
        // Outside the clip: transparent, not "the curve's first value".
        assert_eq!(p.auto_value(T, Ticks(0)), 1.0);
        assert_eq!(p.auto_value(T, Ticks(999)), 1.0);
        // Inside: the V, anchored at clip.pos = 1000.
        assert!((p.auto_value(T, Ticks(1_000)) - 1.0).abs() < 1e-6);
        assert!((p.auto_value(T, Ticks(1_500)) - 0.0).abs() < 1e-6, "the dip lands 500 into the clip");
        assert!((p.auto_value(T, Ticks(1_250)) - 0.5).abs() < 1e-6, "linear between points");
        // The gate closes at end, exclusive.
        assert_eq!(p.auto_value(T, Ticks(2_000)), 1.0);
    }

    #[test]
    fn moving_the_clip_moves_the_ride() {
        let mut p = project();
        curve_with_dip(&mut p, CurveId(1));
        p.apply(&Op::SetBinding { id: BindingId(1), binding: Binding {
            curve: CurveId(1), target: T, gate: Gate::Clip(ClipId(1)) } }).unwrap();
        p.apply(&Op::MoveClip { id: ClipId(1), pos: Ticks(5_000), track: TrackId(1) }).unwrap();
        assert_eq!(p.auto_value(T, Ticks(1_500)), 1.0, "nothing left behind at the old place");
        assert!((p.auto_value(T, Ticks(5_500)) - 0.0).abs() < 1e-6, "the dip followed the clip");
    }

    #[test]
    fn one_curve_many_bindings_and_an_edit_reaches_every_one() {
        let mut p = project();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(2), source: SourceId(1), track: TrackId(1),
            pos: Ticks(4_000), src_in: Ticks::ZERO, len: Ticks(1_000), gain: 1.0, effects: vec![] } }).unwrap();
        curve_with_dip(&mut p, CurveId(1));
        for (bid, cid) in [(1, 1), (2, 2)] {
            p.apply(&Op::SetBinding { id: BindingId(bid), binding: Binding {
                curve: CurveId(1), target: T, gate: Gate::Clip(ClipId(cid)) } }).unwrap();
        }
        assert!((p.auto_value(T, Ticks(1_500)) - 0.0).abs() < 1e-6);
        assert!((p.auto_value(T, Ticks(4_500)) - 0.0).abs() < 1e-6);
        // Lift the dip to half on the ONE curve: both clips hear it.
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks(500), value: 0.5 }).unwrap();
        assert!((p.auto_value(T, Ticks(1_500)) - 0.5).abs() < 1e-6);
        assert!((p.auto_value(T, Ticks(4_500)) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn duplicating_a_clip_rebinds_its_curves_onto_the_copy_without_copying_them() {
        let mut p = project();
        curve_with_dip(&mut p, CurveId(1));
        p.apply(&Op::SetBinding { id: BindingId(1), binding: Binding {
            curve: CurveId(1), target: T, gate: Gate::Clip(ClipId(1)) } }).unwrap();
        let mut next = 2;
        let ops = p.duplicate_clip_ops(ClipId(1), ClipId(9), Ticks(7_000), TrackId(1), &mut next).unwrap();
        assert_eq!(ops.len(), 2, "the clip and one re-binding");
        for op in &ops { p.apply(op).unwrap(); }
        assert_eq!(p.curves.len(), 1, "the curve is shared, not copied");
        assert_eq!(p.bindings.len(), 2);
        assert!((p.auto_value(T, Ticks(7_500)) - 0.0).abs() < 1e-6, "the copy rides the same dip");
        assert!((p.auto_value(T, Ticks(1_500)) - 0.0).abs() < 1e-6, "and the original still does");
        assert_eq!(next, 3, "one id consumed");
    }

    #[test]
    fn bindings_multiply_with_the_lane_and_with_each_other() {
        let mut p = project();
        p.apply(&Op::SetAutoPoint { target: T, at: Ticks(0), value: 0.5 }).unwrap();
        p.apply(&Op::SetCurve { id: CurveId(1), name: "half".into() }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks(0), value: 0.5 }).unwrap();
        p.apply(&Op::SetBinding { id: BindingId(1), binding: Binding {
            curve: CurveId(1), target: T, gate: Gate::Always } }).unwrap();
        p.apply(&Op::SetBinding { id: BindingId(2), binding: Binding {
            curve: CurveId(1), target: T, gate: Gate::Window { start: Ticks(100), end: Ticks(200) } } }).unwrap();
        assert!((p.auto_value(T, Ticks(50)) - 0.25).abs() < 1e-6, "lane × always-binding");
        assert!((p.auto_value(T, Ticks(150)) - 0.125).abs() < 1e-6, "× the window binding inside it");
        assert!((p.auto_value(T, Ticks(250)) - 0.25).abs() < 1e-6, "the window closed again");
    }

    #[test]
    fn the_sweep_agrees_with_the_pure_definition_at_every_sample() {
        let mut p = project();
        curve_with_dip(&mut p, CurveId(1));
        p.apply(&Op::SetBinding { id: BindingId(1), binding: Binding {
            curve: CurveId(1), target: T, gate: Gate::Clip(ClipId(1)) } }).unwrap();
        p.apply(&Op::SetBinding { id: BindingId(2), binding: Binding {
            curve: CurveId(1), target: T, gate: Gate::Window { start: Ticks(1_200), end: Ticks(1_800) } } }).unwrap();
        let mut sweeps = p.binding_sweeps(T);
        assert_eq!(sweeps.len(), 2);
        let mut checked = 0;
        for t in (0..3_000).step_by(7) {
            let swept: f32 = sweeps.iter_mut().map(|s| s.value(Ticks(t))).product();
            let pure = p.auto_value(T, Ticks(t));
            assert!((swept - pure).abs() < 1e-6, "tick {t}: sweep {swept} vs pure {pure}");
            checked += 1;
        }
        assert!(checked > 400, "the sweep must actually have been exercised");
        // And a backwards jump (a loop wrap) still agrees.
        let swept: f32 = sweeps.iter_mut().map(|s| s.value(Ticks(1_500))).product();
        assert!((swept - p.auto_value(T, Ticks(1_500))).abs() < 1e-6);
    }

    #[test]
    fn a_gate_on_a_deleted_clip_is_never_active_and_revives_on_undo() {
        let mut p = project();
        curve_with_dip(&mut p, CurveId(1));
        p.apply(&Op::SetBinding { id: BindingId(1), binding: Binding {
            curve: CurveId(1), target: T, gate: Gate::Clip(ClipId(1)) } }).unwrap();
        let undo = p.apply(&Op::RemoveClip { id: ClipId(1) }).unwrap();
        assert_eq!(p.bindings.len(), 1, "the binding dangles rather than being cleaned up");
        assert_eq!(p.auto_value(T, Ticks(1_500)), 1.0, "and is transparent while it dangles");
        assert!(p.binding_sweeps(T).is_empty(), "the render leaves it out entirely");
        p.apply(&undo).unwrap();
        assert!((p.auto_value(T, Ticks(1_500)) - 0.0).abs() < 1e-6, "undo brought the ride back");
    }

    #[test]
    fn curve_and_binding_ops_invert_exactly() {
        let mut p = project();
        let create = p.apply(&Op::SetCurve { id: CurveId(1), name: "a".into() }).unwrap();
        assert!(matches!(create, Op::RemoveCurve { id: CurveId(1) }));
        let rename = p.apply(&Op::SetCurve { id: CurveId(1), name: "b".into() }).unwrap();
        assert_eq!(rename, Op::SetCurve { id: CurveId(1), name: "a".into() });
        let add = p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks(10), value: 0.5 }).unwrap();
        assert_eq!(add, Op::RemoveCurvePoint { curve: CurveId(1), at: Ticks(10) });
        let restate = p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks(10), value: 0.7 }).unwrap();
        assert_eq!(restate, Op::SetCurvePoint { curve: CurveId(1), at: Ticks(10), value: 0.5 });
        let b = Binding { curve: CurveId(1), target: T, gate: Gate::Always };
        let bind = p.apply(&Op::SetBinding { id: BindingId(1), binding: b }).unwrap();
        assert!(matches!(bind, Op::RemoveBinding { id: BindingId(1) }));
        // A bound curve refuses removal — the RemoveSource rule.
        assert_eq!(p.apply(&Op::RemoveCurve { id: CurveId(1) }), Err(EditError::CurveInUse(CurveId(1))));
        let unbind = p.apply(&Op::RemoveBinding { id: BindingId(1) }).unwrap();
        assert_eq!(unbind, Op::SetBinding { id: BindingId(1), binding: b });
        // Now removal is a Group that restores the name AND the point.
        let before = p.clone();
        let remove = p.apply(&Op::RemoveCurve { id: CurveId(1) }).unwrap();
        assert!(p.curves.is_empty());
        assert!(matches!(remove, Op::Group { .. }), "the inverse restores name and points as one step");
        p.apply(&remove).unwrap();
        assert_eq!(p, before, "undo of a curve removal is exact");
    }

    #[test]
    fn a_binding_is_validated_like_the_lane_it_multiplies() {
        let mut p = project();
        p.apply(&Op::SetCurve { id: CurveId(1), name: "a".into() }).unwrap();
        let err = |p: &mut Project, b: Binding| p.apply(&Op::SetBinding { id: BindingId(1), binding: b }).unwrap_err();
        assert_eq!(err(&mut p, Binding { curve: CurveId(9), target: T, gate: Gate::Always }),
                   EditError::NoSuchCurve(CurveId(9)));
        assert_eq!(err(&mut p, Binding { curve: CurveId(1), target: AutoTarget::TrackGain(TrackId(9)), gate: Gate::Always }),
                   EditError::NoSuchTrack(TrackId(9)));
        assert_eq!(err(&mut p, Binding { curve: CurveId(1), target: T, gate: Gate::Clip(ClipId(9)) }),
                   EditError::NoSuchClip(ClipId(9)));
        assert_eq!(err(&mut p, Binding { curve: CurveId(1), target: T, gate: Gate::Window { start: Ticks(5), end: Ticks(5) } }),
                   EditError::EmptyWindow);
        p.apply(&Op::SetTrackFlags { id: TrackId(1), muted: false, locked: true }).unwrap();
        assert_eq!(err(&mut p, Binding { curve: CurveId(1), target: T, gate: Gate::Always }),
                   EditError::TrackLocked(TrackId(1)), "a ride is a mix edit; a locked track refuses it");
        assert_eq!(p.apply(&Op::RemoveBinding { id: BindingId(1) }), Err(EditError::NoSuchBinding(BindingId(1))));
        assert_eq!(p.apply(&Op::SetCurvePoint { curve: CurveId(9), at: Ticks(0), value: 1.0 }),
                   Err(EditError::NoSuchCurve(CurveId(9))));
        assert_eq!(p.apply(&Op::RemoveCurvePoint { curve: CurveId(1), at: Ticks(0) }),
                   Err(EditError::NoSuchAutoPoint));
    }

    #[test]
    fn a_parameter_target_is_a_key_on_an_effect_and_is_validated_like_gain() {
        use crate::{BusId, ParamKey};
        let mut p = project();
        let bus = BusId(1);
        p.apply(&Op::AddBus { bus: crate::Bus { id: bus, name: "b".into(), parent: None, gain: 1.0,
                                                 chain: vec![], sends: vec![], muted: false } }).unwrap();
        let mix = ParamKey::new("mix").unwrap();
        let on_bus = AutoTarget::BusParam { bus, effect: 0, key: mix };
        let on_clip = AutoTarget::ClipParam { clip: ClipId(1), effect: 2, key: ParamKey::new("freq0").unwrap() };
        // Lanes on both, before any chain exists: the ride may be drawn first.
        p.apply(&Op::SetAutoPoint { target: on_bus, at: Ticks(0), value: 0.0 }).unwrap();
        p.apply(&Op::SetAutoPoint { target: on_bus, at: Ticks(1_000), value: 1.0 }).unwrap();
        p.apply(&Op::SetAutoPoint { target: on_clip, at: Ticks(0), value: 2.0 }).unwrap();
        assert!((p.auto_value(on_bus, Ticks(500)) - 0.5).abs() < 1e-6, "a parameter lane reads like any lane");
        assert_eq!(p.auto_value(on_clip, Ticks(9)), 2.0);
        // A binding on a parameter, gated to a clip.
        p.apply(&Op::SetCurve { id: CurveId(1), name: "sweep".into() }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks(0), value: 4.0 }).unwrap();
        let bound = AutoTarget::BusParam { bus, effect: 1, key: ParamKey::new("freq0").unwrap() };
        p.apply(&Op::SetBinding { id: BindingId(1), binding: Binding {
            curve: CurveId(1), target: bound, gate: Gate::Clip(ClipId(1)) } }).unwrap();
        // The render's question: what is ridden on this bus, and where.
        let targets = p.bus_param_targets(bus);
        assert_eq!(targets.iter().map(|(e, k, _)| (*e, k.as_str())).collect::<Vec<_>>(),
                   vec![(0, "mix"), (1, "freq0")], "lanes and bindings, sorted by effect");
        assert_eq!(targets[1].2, bound);
        assert_eq!(p.clip_param_targets(ClipId(1)).len(), 1);
        assert!(p.clip_param_targets(ClipId(7)).is_empty());
        assert!(p.bus_param_targets(BusId(7)).is_empty());
        // Validation: the bus or the clip's track must exist, and a locked
        // track refuses a parameter ride on its clip like it refuses its fader.
        let missing = AutoTarget::BusParam { bus: BusId(9), effect: 0, key: mix };
        assert_eq!(p.apply(&Op::SetAutoPoint { target: missing, at: Ticks(0), value: 1.0 }),
                   Err(EditError::NoSuchBus(BusId(9))));
        let no_clip = AutoTarget::ClipParam { clip: ClipId(9), effect: 0, key: mix };
        assert_eq!(p.apply(&Op::SetAutoPoint { target: no_clip, at: Ticks(0), value: 1.0 }),
                   Err(EditError::NoSuchClip(ClipId(9))));
        p.apply(&Op::SetTrackFlags { id: TrackId(1), muted: false, locked: true }).unwrap();
        assert_eq!(p.apply(&Op::SetAutoPoint { target: on_clip, at: Ticks(5), value: 1.0 }),
                   Err(EditError::TrackLocked(TrackId(1))));
        // The key itself: 16 ASCII bytes, no more, no NUL, no empty.
        assert_eq!(ParamKey::new("preserve_pitch").unwrap().as_str(), "preserve_pitch");
        assert_eq!(ParamKey::new("sixteen_chars_ok").unwrap().as_str(), "sixteen_chars_ok");
        assert!(ParamKey::new("seventeen_chars__").is_none(), "too long is refused, never truncated");
        assert!(ParamKey::new("").is_none());
        assert!(ParamKey::new("naïve").is_none(), "ASCII only");
        assert!(ParamKey::new("a\0b").is_none(), "the pad byte cannot be in a key");
        assert!(ParamKey::new("mix") < ParamKey::new("size"), "orders as the string does");
        assert_eq!(format!("{}", mix), "mix");
    }

    #[test]
    fn a_group_gate_opens_over_the_hull_and_follows_the_members() {
        use crate::GroupId;
        let mut p = project(); // clip 1 sits at 1000..2000
        // A second clip well apart; a group of the two.
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(3), source: SourceId(1), track: TrackId(1),
            pos: Ticks(5_000), src_in: Ticks::ZERO, len: Ticks(1_000), gain: 1.0, effects: vec![] } }).unwrap();
        p.apply(&Op::SetGroup { id: GroupId(1), name: "drop".into(), clips: vec![ClipId(1), ClipId(3)] }).unwrap();
        assert_eq!(p.group_hull(GroupId(1)), Some((Ticks(1_000), Ticks(6_000))), "first start to last end");
        assert_eq!(p.group_hull(GroupId(9)), None);
        p.apply(&Op::SetCurve { id: CurveId(1), name: "riser".into() }).unwrap();
        // 0 at the hull's start, 1 five thousand ticks later: a riser
        // across the composition, in hull-relative time.
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks(0), value: 0.0 }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks(5_000), value: 1.0 }).unwrap();
        p.apply(&Op::SetBinding { id: BindingId(1), binding: Binding {
            curve: CurveId(1), target: T, gate: Gate::Group(GroupId(1)) } }).unwrap();
        let v = |p: &Project, t: i64| p.auto_value(T, Ticks(t));
        assert!((v(&p, 1_000)).abs() < 1e-6, "hull start: the curve's 0");
        assert!((v(&p, 3_500) - 0.5).abs() < 1e-6, "halfway across the hull — INCLUDING the gap between members");
        assert!((v(&p, 6_000) - 1.0).abs() < 1e-6, "past the hull: transparent, which is also the curve's end");
        assert!((v(&p, 500) - 1.0).abs() < 1e-6, "before the hull: transparent");
        // Move the last member later: the hull grows, the ride keeps its
        // origin at the first member and holds its last point past its end.
        p.apply(&Op::MoveClip { id: ClipId(3), pos: Ticks(9_000), track: TrackId(1) }).unwrap();
        assert_eq!(p.group_hull(GroupId(1)), Some((Ticks(1_000), Ticks(10_000))));
        assert!((v(&p, 3_500) - 0.5).abs() < 1e-6, "the curve is in hull-relative time; its own length is 5000");
        assert!((v(&p, 8_000) - 1.0).abs() < 1e-6, "past the curve's end but inside the hull: held at the last point");
        // Move the FIRST member: the origin moves with it.
        p.apply(&Op::MoveClip { id: ClipId(1), pos: Ticks(2_000), track: TrackId(1) }).unwrap();
        assert!((v(&p, 4_500) - 0.5).abs() < 1e-6, "origin now 2000: halfway is at 4500");
        assert!((v(&p, 1_500) - 1.0).abs() < 1e-6, "before the hull: transparent");
        // Validation and the empty case.
        assert_eq!(p.apply(&Op::SetBinding { id: BindingId(2), binding: Binding {
            curve: CurveId(1), target: T, gate: Gate::Group(GroupId(9)) } }),
            Err(EditError::NoSuchGroup(GroupId(9))));
        p.apply(&Op::RemoveGroup { id: GroupId(1) }).unwrap();
        assert!((v(&p, 3_500) - 1.0).abs() < 1e-6, "the group is gone: the binding stays, never active");
        assert_eq!(p.binding_sweeps(T).len(), 0, "and a sweep is not built for it");
    }

    #[test]
    fn an_empty_curve_is_transparent() {
        let mut p = project();
        p.apply(&Op::SetCurve { id: CurveId(1), name: "a".into() }).unwrap();
        p.apply(&Op::SetBinding { id: BindingId(1), binding: Binding {
            curve: CurveId(1), target: T, gate: Gate::Clip(ClipId(1)) } }).unwrap();
        assert_eq!(p.auto_value(T, Ticks(1_500)), 1.0, "a curve with no points changes nothing");
    }
}
