//! batch — edits over several clips, returned as the ops that make them
//! one fact.
//!
//! A composition folds many clips into one block, and the whole point of
//! the fold is to act on the innards without touching them one at a
//! time. Three gestures needed more than "run the single-clip op N times":
//!
//! * **Moving together.** `MoveClip` checks room against the project as it
//!   is, so moving a group of adjacent clips to the right one at a time
//!   collides each clip with the neighbour that has not moved yet. The
//!   fix is ordering, not a new op: for a rightward delta move the
//!   rightmost clip first, for a leftward delta the leftmost. Every member
//!   keeps its own track — a group is a horizontal thing — and the delta
//!   is one number, so the innards keep their spacing exactly.
//!
//! * **Splitting on a grid.** Cutting a take on every beat is one gesture
//!   in the request and dozens of `split_ops` in the model. A split clip
//!   that belonged to a composition must leave its pieces IN the
//!   composition, or the fold has just lost its innards; and a curve gated
//!   to the clip must keep playing what it played, so the gate becomes a
//!   window over the clip's original span. (A gate on the composition
//!   itself — active over its hull, moving with it — is the named next
//!   step; it needs a wire bump and a hull rule, and this works without
//!   it.)
//!
//! * **Making a curve unique.** One curve, many bindings is the reuse the
//!   curves module exists for, and the reason to break it is exactly as
//!   real: "this one needs to be different now". The clone takes a fresh
//!   id and the binding is re-aimed; the other bindings never notice.
//!   [`Project::curve_share_count`] is what a UI shows beside the name so
//!   the person knows, before editing, whether eleven other clips will
//!   hear it.

use crate::{Binding, BindingId, Clip, ClipId, CurveId, EditError, Gate, Op, Project, Ticks};

impl Project {
    /// The ops that move every clip in `ids` by the delta that puts
    /// `anchor` at `pos`, each on its own track, ordered so the members
    /// cannot collide with each other on the way. Clamped at zero as a
    /// whole: if the leftmost member would go negative the delta shrinks
    /// so it lands at zero, and the spacing is still preserved. Ids not in
    /// the project are skipped; an anchor not in the project is an error.
    pub fn move_together_ops(&self, ids: &[ClipId], anchor: ClipId, pos: Ticks)
        -> Result<Vec<Op>, EditError>
    {
        let a = self.clips.get(&anchor).ok_or(EditError::NoSuchClip(anchor))?;
        let mut members: Vec<&Clip> = ids.iter().filter_map(|id| self.clips.get(id)).collect();
        if !members.iter().any(|c| c.id == anchor) { members.push(a); }
        let mut delta = pos.0 - a.pos.0;
        let leftmost = members.iter().map(|c| c.pos.0).min().unwrap_or(0);
        if leftmost + delta < 0 { delta = -leftmost; }
        if delta == 0 { return Ok(Vec::new()); }
        // Rightward: rightmost first. Leftward: leftmost first. Same-track
        // members never overlap each other, so this order is enough.
        members.sort_by_key(|c| if delta > 0 { -c.pos.0 } else { c.pos.0 });
        Ok(members.iter()
            .map(|c| Op::MoveClip { id: c.id, pos: Ticks(c.pos.0 + delta), track: c.track })
            .collect())
    }

    /// The grid ticks strictly inside `(from, to)`: every multiple of
    /// `grid` past `phase`, ascending. Pure, so the cut points can be
    /// pinned without a project.
    pub fn grid_cuts(from: Ticks, to: Ticks, grid: Ticks, phase: Ticks) -> Vec<Ticks> {
        let g = grid.0.max(1);
        let mut out = Vec::new();
        // First multiple of g (relative to phase) strictly greater than from.
        let rel = from.0 - phase.0;
        let mut k = rel.div_euclid(g) + 1;
        loop {
            let t = phase.0 + k * g;
            if t >= to.0 { break; }
            out.push(Ticks(t));
            k += 1;
        }
        out
    }

    /// Split every clip in `ids` at every grid tick inside it, minting ids
    /// from `next_id`. Group membership follows the pieces (each group the
    /// clip was in is restated with the new pieces added), and a curve
    /// gated to the clip becomes gated to a window over the clip's
    /// ORIGINAL span, so the render is unchanged by the cut — pinned one
    /// crate over. Clips no grid line crosses produce no ops.
    pub fn split_on_grid_ops(&self, ids: &[ClipId], grid: Ticks, phase: Ticks, next_id: &mut u64)
        -> Result<Vec<Op>, EditError>
    {
        let mut ops = Vec::new();
        let mut group_adds: std::collections::BTreeMap<crate::GroupId, Vec<ClipId>> = Default::default();
        for id in ids {
            let c = self.clips.get(id).ok_or(EditError::NoSuchClip(*id))?;
            let cuts = Self::grid_cuts(c.pos, c.end(), grid, phase);
            if cuts.is_empty() { continue; }
            // Piece boundaries: pos, cut1, …, cutN, end.
            let mut bounds = Vec::with_capacity(cuts.len() + 2);
            bounds.push(c.pos);
            bounds.extend(cuts.iter().copied());
            bounds.push(c.end());
            // The original keeps the first piece.
            ops.push(Op::TrimClip { id: *id, src_in: c.src_in, len: bounds[1].saturating_sub(bounds[0]) });
            let mut new_ids = Vec::new();
            for w in bounds.windows(2).skip(1) {
                let (start, stop) = (w[0], w[1]);
                let nid = ClipId(*next_id);
                *next_id += 1;
                new_ids.push(nid);
                ops.push(Op::AddClip { clip: Clip {
                    id: nid, source: c.source, track: c.track, pos: start,
                    src_in: c.src_in.saturating_add(start.saturating_sub(c.pos)),
                    len: stop.saturating_sub(start), gain: c.gain, effects: c.effects.clone(),
                } });
            }
            for (gid, g) in &self.groups {
                if g.clips.contains(id) {
                    group_adds.entry(*gid).or_default().extend(new_ids.iter().copied());
                }
            }
            for (bid, b) in &self.bindings {
                if b.gate == Gate::Clip(*id) {
                    ops.push(Op::SetBinding { id: *bid, binding: Binding {
                        gate: Gate::Window { start: c.pos, end: c.end() }, ..*b } });
                }
            }
        }
        for (gid, adds) in group_adds {
            let g = &self.groups[&gid];
            let mut clips = g.clips.clone();
            clips.extend(adds);
            ops.push(Op::SetGroup { id: gid, name: g.name.clone(), clips });
        }
        Ok(ops)
    }

    /// How many bindings drive `curve` — what a label shows beside the
    /// curve's name, so "shared ×4" is known before a point is moved.
    pub fn curve_share_count(&self, curve: CurveId) -> usize {
        self.bindings.values().filter(|b| b.curve == curve).count()
    }

    /// Clone the curve behind `binding` to `new_curve` and re-aim the
    /// binding at the clone. The original and every other binding on it
    /// are untouched. Refused when the binding or its curve is missing;
    /// on an already-unique curve it still clones (the caller can check
    /// [`Self::curve_share_count`] first if it wants to say "already
    /// unique" instead).
    pub fn make_curve_unique_ops(&self, binding: BindingId, new_curve: CurveId)
        -> Result<Vec<Op>, EditError>
    {
        let b = self.bindings.get(&binding).ok_or(EditError::NoSuchBinding(binding))?;
        let c = self.curves.get(&b.curve).ok_or(EditError::NoSuchCurve(b.curve))?;
        let mut ops = vec![Op::SetCurve { id: new_curve, name: format!("{} (unique)", c.name) }];
        ops.extend(c.points.iter().map(|(at, value)| Op::SetCurvePoint { curve: new_curve, at: *at, value: *value }));
        ops.push(Op::SetBinding { id: binding, binding: Binding { curve: new_curve, ..*b } });
        Ok(ops)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AutoTarget, GroupId, Source, SourceId, Track, TrackId, TrackKind};

    /// One track, one source, three adjacent 1000-tick clips at 0, 1000,
    /// 2000 — the shape a chopped take has.
    fn chopped() -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio, name: "vox".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "take.wav".into(),
            len: Ticks(100_000), sample_rate: 48_000, channels: 1, has_video: false } }).unwrap();
        for i in 0..3u64 {
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(i + 1), source: SourceId(1), track: TrackId(1),
                pos: Ticks(i as i64 * 1000), src_in: Ticks(i as i64 * 1000), len: Ticks(1000),
                gain: 1.0, effects: vec![] } }).unwrap();
        }
        p
    }
    const ALL: [ClipId; 3] = [ClipId(1), ClipId(2), ClipId(3)];

    #[test]
    fn moving_adjacent_clips_right_does_not_collide_with_itself() {
        let mut p = chopped();
        // One at a time in id order, this refuses at the first clip: 1
        // would land on 2. Together, it must land — spacing intact.
        let ops = p.move_together_ops(&ALL, ClipId(1), Ticks(500)).unwrap();
        assert_eq!(ops.len(), 3);
        for op in &ops { p.apply(op).expect("no self-collision"); }
        assert_eq!(p.clips[&ClipId(1)].pos, Ticks(500));
        assert_eq!(p.clips[&ClipId(2)].pos, Ticks(1500));
        assert_eq!(p.clips[&ClipId(3)].pos, Ticks(2500));
        // And back left by the same amount, anchored on the LAST clip.
        let ops = p.move_together_ops(&ALL, ClipId(3), Ticks(2000)).unwrap();
        for op in &ops { p.apply(op).expect("no self-collision leftward either"); }
        assert_eq!(p.clips[&ClipId(1)].pos, Ticks(0));
    }

    #[test]
    fn a_group_move_clamps_as_a_whole_and_keeps_spacing() {
        let p = chopped();
        let ops = p.move_together_ops(&ALL, ClipId(3), Ticks(500)).unwrap();
        // Asked to move 3 to 500 (delta −1500); 1 would reach −1500, so
        // the delta shrinks to −0: nothing moves, nothing is squashed.
        assert!(ops.is_empty(), "a move that would push the leftmost past zero clamps the whole gesture");
        // With room to shrink into: the group at 500/1500/2500, clip 2
        // asked to 600 (delta −900). The leftmost can only give up 500,
        // so the whole group moves by −500 and lands at 0/1000/2000.
        let mut p = p;
        for op in p.move_together_ops(&ALL, ClipId(1), Ticks(500)).unwrap() { p.apply(&op).unwrap(); }
        let ops = p.move_together_ops(&ALL, ClipId(2), Ticks(600)).unwrap();
        let pos: Vec<i64> = ops.iter().map(|o| match o { Op::MoveClip { pos, .. } => pos.0, _ => unreachable!() }).collect();
        assert_eq!(pos.iter().min(), Some(&0), "the leftmost lands at zero");
        assert_eq!(pos.iter().max(), Some(&2000), "spacing of 1000 preserved");
    }

    #[test]
    fn move_together_refuses_a_missing_anchor_and_skips_missing_members() {
        let p = chopped();
        assert!(matches!(p.move_together_ops(&ALL, ClipId(9), Ticks(0)), Err(EditError::NoSuchClip(ClipId(9)))));
        let ops = p.move_together_ops(&[ClipId(1), ClipId(9)], ClipId(1), Ticks(100)).unwrap();
        assert_eq!(ops.len(), 1);
    }

    #[test]
    fn grid_cuts_are_strictly_inside_and_honour_the_phase() {
        assert_eq!(Project::grid_cuts(Ticks(0), Ticks(1000), Ticks(250), Ticks::ZERO),
                   vec![Ticks(250), Ticks(500), Ticks(750)], "endpoints are not cuts");
        assert_eq!(Project::grid_cuts(Ticks(100), Ticks(1000), Ticks(250), Ticks::ZERO),
                   vec![Ticks(250), Ticks(500), Ticks(750)]);
        assert_eq!(Project::grid_cuts(Ticks(250), Ticks(1000), Ticks(250), Ticks::ZERO),
                   vec![Ticks(500), Ticks(750)], "a clip starting ON a line is not cut there");
        assert_eq!(Project::grid_cuts(Ticks(0), Ticks(1000), Ticks(400), Ticks(100)),
                   vec![Ticks(100), Ticks(500), Ticks(900)], "phase shifts the lines");
        assert!(Project::grid_cuts(Ticks(0), Ticks(100), Ticks(250), Ticks::ZERO).is_empty());
        assert!(Project::grid_cuts(Ticks(0), Ticks(100), Ticks(0), Ticks::ZERO).len() < 200, "a zero grid is treated as 1, never a hang");
    }

    #[test]
    fn splitting_on_the_grid_keeps_the_pieces_in_the_group_and_the_audio_contiguous() {
        let mut p = chopped();
        p.apply(&Op::SetGroup { id: GroupId(1), name: "hook".into(), clips: ALL.to_vec() }).unwrap();
        let mut next = 10;
        let ops = p.split_on_grid_ops(&[ClipId(2)], Ticks(250), Ticks::ZERO, &mut next).unwrap();
        for op in &ops { p.apply(op).unwrap(); }
        // Clip 2 (1000..2000) became four 250-tick pieces reading the
        // source contiguously.
        let mut pieces: Vec<&Clip> = p.clips.values().filter(|c| c.pos >= Ticks(1000) && c.pos < Ticks(2000)).collect();
        pieces.sort_by_key(|c| c.pos);
        assert_eq!(pieces.len(), 4);
        for (i, c) in pieces.iter().enumerate() {
            assert_eq!(c.pos, Ticks(1000 + 250 * i as i64));
            assert_eq!(c.len, Ticks(250));
            assert_eq!(c.src_in, Ticks(1000 + 250 * i as i64), "source reads on without a seam");
        }
        assert_eq!(p.groups[&GroupId(1)].clips.len(), 6, "three new pieces joined the composition");
        assert_eq!(next, 13);
        // And a clip nothing crosses is left alone.
        let none = p.split_on_grid_ops(&[ClipId(1)], Ticks(5000), Ticks::ZERO, &mut next).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn a_clip_gated_curve_survives_the_split_as_a_window_over_the_original_span() {
        let mut p = chopped();
        p.apply(&Op::SetCurve { id: CurveId(1), name: "dip".into() }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks(0), value: 1.0 }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks(500), value: 0.0 }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks(1000), value: 1.0 }).unwrap();
        let t = AutoTarget::TrackGain(TrackId(1));
        p.apply(&Op::SetBinding { id: BindingId(1), binding: Binding { curve: CurveId(1), target: t,
            gate: Gate::Clip(ClipId(2)) } }).unwrap();
        let before: Vec<f32> = (900..2100).step_by(10).map(|x| p.auto_value(t, Ticks(x))).collect();
        let mut next = 10;
        for op in p.split_on_grid_ops(&[ClipId(2)], Ticks(250), Ticks::ZERO, &mut next).unwrap() { p.apply(&op).unwrap(); }
        assert_eq!(p.bindings[&BindingId(1)].gate, Gate::Window { start: Ticks(1000), end: Ticks(2000) });
        let after: Vec<f32> = (900..2100).step_by(10).map(|x| p.auto_value(t, Ticks(x))).collect();
        assert_eq!(before, after, "the ride plays exactly what it played before the cut");
    }

    #[test]
    fn make_unique_clones_the_curve_for_one_binding_only() {
        let mut p = chopped();
        p.apply(&Op::SetCurve { id: CurveId(1), name: "sweep".into() }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks(0), value: 0.2 }).unwrap();
        let t = AutoTarget::TrackGain(TrackId(1));
        for (bid, cid) in [(1, 1), (2, 2), (3, 3)] {
            p.apply(&Op::SetBinding { id: BindingId(bid), binding: Binding { curve: CurveId(1), target: t,
                gate: Gate::Clip(ClipId(cid)) } }).unwrap();
        }
        assert_eq!(p.curve_share_count(CurveId(1)), 3);
        for op in p.make_curve_unique_ops(BindingId(2), CurveId(7)).unwrap() { p.apply(&op).unwrap(); }
        assert_eq!(p.curve_share_count(CurveId(1)), 2);
        assert_eq!(p.curve_share_count(CurveId(7)), 1);
        assert_eq!(p.curves[&CurveId(7)].name, "sweep (unique)");
        assert_eq!(p.curves[&CurveId(7)].points, p.curves[&CurveId(1)].points, "a clone starts identical");
        // Edit the clone: the others do not hear it.
        p.apply(&Op::SetCurvePoint { curve: CurveId(7), at: Ticks(0), value: 0.9 }).unwrap();
        assert!((p.auto_value(t, Ticks(1500)) - 0.9).abs() < 1e-6, "clip 2 rides the clone");
        assert!((p.auto_value(t, Ticks(500)) - 0.2).abs() < 1e-6, "clip 1 still rides the original");
        assert!(matches!(p.make_curve_unique_ops(BindingId(9), CurveId(8)), Err(EditError::NoSuchBinding(BindingId(9)))));
    }
}
