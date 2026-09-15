//! takes — where the second take goes, and how the best parts come back.
//!
//! One take lands where the playhead was, on the track; that has worked
//! since takes were ops. The second take of the same phrase is the
//! problem: it lands on the first, and the overlap policy either
//! refuses it, trims the first (the take you might have wanted), or —
//! with [`Overlap::Lane`](crate::Overlap) — moves it to the one shadow
//! lane, where the third take collides again. A vocal session is eight
//! takes of the same two bars.
//!
//! So a take goes to the first lane in the track's CHAIN that has room:
//! the track, then `track ↓`, then `track ↓ ↓`, each one a shadow lane
//! of the one before (room.rs's own convention, applied to itself — a
//! lane's shadow is the next lane), created only when every existing
//! lane over that span is taken. Eight takes of the same two bars are
//! eight lanes; a ninth take of a DIFFERENT two bars goes back on the
//! parent, because the parent has room there. Nothing here is a new
//! model type: a lane is a track, a take is a clip, and the chain is a
//! fact about names, the way room.rs already states the parent relation.
//!
//! Comping is the other direction. The parent track is the comp; each
//! lane is an alternative. [`Project::comp_ops`] copies a span of one
//! lane's take onto the parent — the same source, the offset carried in
//! `src_in`, whatever was on the parent over that span giving way by
//! the trim rule — so the comp is an ordinary set of clips that renders,
//! exports and travels like any other, and the lanes keep every take
//! intact underneath it. Lanes are muted when they hold takes the comp
//! did not choose; that is a track flag, not a comp state. A swipe that
//! takes half of one take is a split on the lane (`s`, which exists)
//! and a comp of the half.
//!
//! What this does not decide: which take is best. That is a score, and
//! the score the maintainer wants is the transcript's — the take whose
//! words match the lyric — which needs whisper on the machine. The
//! choice arrives here as a lane per span; where it came from is the
//! shell's business.

use crate::{Clip, ClipId, EditError, Op, Project, Ticks, TrackId, TrackKind};

impl Project {
    /// The chain of lanes rooted at `track`: the track itself, then each
    /// shadow lane of the one before, in order. Just `[track]` for a
    /// track with no lanes; empty for a track that does not exist.
    pub fn take_lanes(&self, track: TrackId) -> Vec<TrackId> {
        if !self.tracks.iter().any(|t| t.id == track) { return Vec::new(); }
        let mut out = vec![track];
        let mut cur = track;
        while let Some(next) = self.shadow_lane(cur) {
            out.push(next);
            cur = next;
        }
        out
    }

    /// The root of the chain `track` is in: itself, or the parent's parent's…
    pub fn take_root(&self, track: TrackId) -> TrackId {
        let mut cur = track;
        while let Some(p) = self.lane_parent(cur) { cur = p; }
        cur
    }

    /// Whether `[pos, end)` on `track` holds no clip.
    fn has_room(&self, track: TrackId, pos: Ticks, end: Ticks) -> bool {
        !self.clips.values().any(|c| c.track == track && c.pos < end && c.end() > pos)
    }

    /// Where a take over `[pos, end)` on `track`'s chain should land: the
    /// first lane with room, or a new lane after the last. Returns the
    /// lane's id and the ops that create it (empty when it exists); a new
    /// lane's id is `new_id`. Fails only for a missing track.
    pub fn take_lane_ops(&self, track: TrackId, pos: Ticks, end: Ticks, new_id: TrackId)
        -> Result<(TrackId, Vec<Op>), EditError>
    {
        let lanes = self.take_lanes(track);
        if lanes.is_empty() { return Err(EditError::NoSuchTrack(track)); }
        for &lane in &lanes {
            if self.has_room(lane, pos, end) { return Ok((lane, Vec::new())); }
        }
        let last = *lanes.last().expect("non-empty");
        Ok((new_id, self.shadow_lane_ops(last, new_id)?))
    }

    /// The comp: for each `(from, to, lane)`, the span of `lane`'s take
    /// over `[from, to)` placed on `track` (the chain's root) as a new
    /// clip — same source, `src_in` carried — with whatever the root held
    /// over that span giving way. New clip ids come from `next_clip`.
    ///
    /// Refused when a lane is not in `track`'s chain, when no single clip
    /// on the lane covers the whole span (a comp span inside one take is
    /// the unit; two takes across one span is two choices), or when the
    /// span is empty. A choice whose lane IS the root is a no-op: the
    /// root's own take is already the comp there.
    pub fn comp_ops(&self, track: TrackId, choices: &[(Ticks, Ticks, TrackId)], next_clip: &mut u64)
        -> Result<Vec<Op>, EditError>
    {
        let lanes = self.take_lanes(track);
        if lanes.is_empty() { return Err(EditError::NoSuchTrack(track)); }
        let mut ops = Vec::new();
        // Room is made against a probe that accumulates this comp's own
        // placements, so two choices side by side do not trim each other.
        let mut probe = self.clone();
        for &(from, to, lane) in choices {
            if to <= from { return Err(EditError::EmptyWindow); }
            if !lanes.contains(&lane) { return Err(EditError::NoSuchTrack(lane)); }
            if lane == track { continue; }
            let take = probe.clips.values()
                .find(|c| c.track == lane && c.pos <= from && c.end() >= to)
                .ok_or(EditError::NoTakeOver { lane, from, to })?;
            let id = ClipId(*next_clip);
            *next_clip += 1;
            let piece = Clip {
                id, source: take.source, track,
                pos: from, src_in: take.src_in.saturating_add(from.saturating_sub(take.pos)),
                len: to.saturating_sub(from), gain: take.gain, effects: take.effects.clone(),
            };
            let room = probe.make_room_ops(&piece, &[], next_clip);
            for r in room {
                probe.apply(&r)?;
                ops.push(r);
            }
            let add = Op::AddClip { clip: piece };
            probe.apply(&add)?;
            ops.push(add);
        }
        Ok(ops)
    }

    /// Mute every lane under `track` that is not the root (the comp plays
    /// from the root; the takes stay, silent), or unmute them all to
    /// audition. Ops for the lanes whose flag actually changes.
    pub fn take_lanes_mute_ops(&self, track: TrackId, muted: bool) -> Vec<Op> {
        self.take_lanes(track).into_iter().skip(1)
            .filter_map(|id| self.tracks.iter().find(|t| t.id == id))
            .filter(|t| t.muted != muted && t.kind == TrackKind::Audio)
            .map(|t| Op::SetTrackFlags { id: t.id, muted, locked: t.locked })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Source, SourceId, Track};

    fn project() -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio, name: "vox".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(2), kind: TrackKind::Audio, name: "gtr".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 1 }).unwrap();
        for s in 1..=4 {
            p.apply(&Op::AddSource { source: Source { id: SourceId(s), path: format!("take{s}"),
                len: Ticks(10_000), sample_rate: 48_000, channels: 1, has_video: false, kind: crate::SourceKind::File } }).unwrap();
        }
        p
    }

    /// Land a take of `source` over `[pos, end)` on the vox chain, the
    /// way the shell does: ask for the lane, create it if told to, add
    /// the clip. Returns the lane it landed on.
    fn take(p: &mut Project, source: u64, pos: i64, end: i64) -> TrackId {
        let next_track = TrackId(p.tracks.iter().map(|t| t.id.0).max().unwrap_or(0) + 1);
        let (lane, ops) = p.take_lane_ops(TrackId(1), Ticks(pos), Ticks(end), next_track).unwrap();
        for op in ops { p.apply(&op).unwrap(); }
        let id = ClipId(p.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1);
        p.apply(&Op::AddClip { clip: Clip { id, source: SourceId(source), track: lane, pos: Ticks(pos),
            src_in: Ticks::ZERO, len: Ticks(end - pos), gain: 1.0, effects: vec![] } }).unwrap();
        lane
    }

    #[test]
    fn eight_takes_of_the_same_bars_are_eight_lanes_and_a_take_elsewhere_goes_home() {
        let mut p = project();
        assert_eq!(p.take_lanes(TrackId(1)), vec![TrackId(1)], "no lanes yet: the chain is the track");
        assert!(p.take_lanes(TrackId(9)).is_empty());
        let first = take(&mut p, 1, 0, 1000);
        assert_eq!(first, TrackId(1), "the first take lands on the track");
        let second = take(&mut p, 2, 0, 1000);
        assert_ne!(second, TrackId(1), "the second goes to a lane");
        let third = take(&mut p, 3, 500, 1500);
        assert!(third != first && third != second, "overlapping both: a third lane");
        assert_eq!(p.take_lanes(TrackId(1)), vec![first, second, third], "the chain, in order");
        let names: Vec<_> = p.tracks.iter().map(|t| t.name.clone()).collect();
        assert_eq!(names, vec!["vox", "vox ↓", "vox ↓ ↓", "gtr"], "lanes sit under their parent, before the next track");
        assert_eq!(p.take_root(third), TrackId(1));
        assert_eq!(p.take_root(TrackId(2)), TrackId(2));
        // A take over bars nobody used goes back to the parent.
        assert_eq!(take(&mut p, 4, 5000, 6000), TrackId(1), "room on the parent: no new lane");
        // And one that only misses the third lane's span lands on the first free lane.
        assert_eq!(take(&mut p, 4, 1100, 1400), TrackId(1));
        assert_eq!(take(&mut p, 4, 1000, 1200), second, "the track is busy 1000..1200 now; lane 2 is free");
        assert_eq!(p.take_lanes(TrackId(1)).len(), 3, "still three lanes");
        assert_eq!(p.take_lane_ops(TrackId(9), Ticks(0), Ticks(1), TrackId(99)), Err(EditError::NoSuchTrack(TrackId(9))));
        // The lane a shadow creates routes and gains like its parent.
        let lane = p.tracks.iter().find(|t| t.id == second).unwrap();
        let parent = &p.tracks[0];
        assert_eq!((lane.kind, lane.route, lane.gain), (parent.kind, parent.route, parent.gain));
    }

    #[test]
    fn a_comp_is_spans_of_lanes_copied_onto_the_root_with_the_offset_carried() {
        let mut p = project();
        take(&mut p, 1, 0, 1000);          // root: take 1
        let l2 = take(&mut p, 2, 0, 1000); // lane: take 2
        let l3 = take(&mut p, 3, 0, 1000); // lane: take 3
        // The comp: first half from take 2, last quarter from take 3, the
        // rest is the root's own take 1.
        let mut next = 100;
        let ops = p.comp_ops(TrackId(1), &[(Ticks(0), Ticks(500), l2), (Ticks(750), Ticks(1000), l3),
                                            (Ticks(500), Ticks(750), TrackId(1))], &mut next).unwrap();
        for op in &ops { p.apply(op).unwrap(); }
        let mut on_root: Vec<&Clip> = p.clips.values().filter(|c| c.track == TrackId(1)).collect();
        on_root.sort_by_key(|c| c.pos);
        let shape: Vec<_> = on_root.iter().map(|c| (c.source.0, c.pos.0, c.len.0, c.src_in.0)).collect();
        assert_eq!(shape, vec![(2, 0, 500, 0), (1, 500, 250, 500), (3, 750, 250, 750)],
                   "take 2's head, take 1's middle (trimmed, offset kept), take 3's tail (src_in 750)");
        // The lanes are untouched: every take is still whole underneath.
        for (lane, src) in [(l2, 2), (l3, 3)] {
            let c = p.clips.values().find(|c| c.track == lane).unwrap();
            assert_eq!((c.source.0, c.pos.0, c.len.0), (src, 0, 1000), "lane {} intact", lane.0);
        }
        // Muting the lanes is a flag on each, and only on the ones that change.
        let mute = p.take_lanes_mute_ops(TrackId(1), true);
        assert_eq!(mute.len(), 2);
        for op in &mute { p.apply(op).unwrap(); }
        assert!(p.take_lanes_mute_ops(TrackId(1), true).is_empty(), "already muted: nothing to do");
        assert_eq!(p.take_lanes_mute_ops(TrackId(1), false).len(), 2);
        assert!(!p.tracks[0].muted, "the root is never muted by this");
    }

    #[test]
    fn a_comp_refuses_what_it_cannot_mean() {
        let mut p = project();
        take(&mut p, 1, 0, 1000);
        let l2 = take(&mut p, 2, 0, 1000);
        let mut next = 100;
        assert_eq!(p.comp_ops(TrackId(1), &[(Ticks(0), Ticks(500), TrackId(2))], &mut next),
                   Err(EditError::NoSuchTrack(TrackId(2))), "gtr is not one of vox's lanes");
        assert_eq!(p.comp_ops(TrackId(1), &[(Ticks(500), Ticks(500), l2)], &mut next),
                   Err(EditError::EmptyWindow));
        assert_eq!(p.comp_ops(TrackId(1), &[(Ticks(500), Ticks(1500), l2)], &mut next),
                   Err(EditError::NoTakeOver { lane: l2, from: Ticks(500), to: Ticks(1500) }),
                   "no single take on the lane covers 500..1500");
        assert_eq!(p.comp_ops(TrackId(9), &[], &mut next), Err(EditError::NoSuchTrack(TrackId(9))));
        assert!(p.comp_ops(TrackId(1), &[], &mut next).unwrap().is_empty(), "no choices, no ops");
        // Two adjacent choices from the same lane do not trim each other:
        // the probe carries the first placement into the second's room check.
        let ops = p.comp_ops(TrackId(1), &[(Ticks(0), Ticks(500), l2), (Ticks(500), Ticks(1000), l2)], &mut next).unwrap();
        for op in &ops { p.apply(op).unwrap(); }
        let mut on_root: Vec<&Clip> = p.clips.values().filter(|c| c.track == TrackId(1)).collect();
        on_root.sort_by_key(|c| c.pos);
        assert_eq!(on_root.iter().map(|c| (c.source.0, c.pos.0, c.len.0)).collect::<Vec<_>>(),
                   vec![(2, 0, 500), (2, 500, 500)], "take 1 is gone from the root; take 2 in two pieces");
    }
}
