//! room — what happens to the clips a placement lands on.
//!
//! `check_room` refuses any overlap on a track, and a refusal is the right
//! MODEL rule (two peers must never converge on two clips playing the
//! same span of one track). It is a bad EDITING rule: a three-minute take
//! sitting two screens to the right refuses every drop in front of it,
//! and on a phone the thing refusing you is never on screen. So the
//! refusal stays in the model and the shell chooses, per track, what to
//! do about it before the op is sent:
//!
//! * [`Overlap::Refuse`] — today's behaviour, said out loud.
//! * [`Overlap::Trim`] — newest wins. The placed clip keeps its span and
//!   whatever it lands on gives way: covered entirely, it is removed;
//!   head covered, it is left-trimmed and slid (trim then move, the
//!   left-trim precedent); tail covered, right-trimmed; pierced, split
//!   into a head and a tail around the newcomer. Every one of those is
//!   an op — undoable, on the wire — and a trim never loses audio,
//!   because the source still has it. This is Logic's and Ableton's
//!   rule, and the default here.
//! * [`Overlap::Lane`] — the overlap goes to a shadow lane: a track made
//!   beside the parent with the same kind and route, named after it, so
//!   the clip plays through the same bus and the fold/mute story stays
//!   one track's. The parent relation is carried by NAME (`"<parent> ↓"`)
//!   rather than a field, because a field would cost a wire bump and a
//!   file migration for a fact the name already states.
//!
//! Everything here returns ops; nothing mutates. The shell composes
//! `make_room_ops` + the move into one group so one `u` reverses both.

use crate::{Clip, ClipId, EditError, Op, Project, Track, TrackId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Overlap {
    Refuse,
    #[default]
    Trim,
    Lane,
}

impl Overlap {
    pub fn name(self) -> &'static str {
        match self { Overlap::Refuse => "refuse", Overlap::Trim => "trim", Overlap::Lane => "lane" }
    }
    pub fn parse(s: &str) -> Option<Overlap> {
        match s.trim() { "refuse" => Some(Overlap::Refuse), "trim" => Some(Overlap::Trim),
                         "lane" => Some(Overlap::Lane), _ => None }
    }
}

/// The suffix a shadow lane's name carries.
pub const LANE_SUFFIX: &str = " ↓";

impl Project {
    /// The clips on `clip`'s track that `clip` would overlap, excluding
    /// `exclude` (the set that is moving with it) and itself, in
    /// timeline order.
    pub fn overlappers(&self, clip: &Clip, exclude: &[ClipId]) -> Vec<&Clip> {
        let mut v: Vec<&Clip> = self.clips.values()
            .filter(|o| o.id != clip.id && !exclude.contains(&o.id) && clip.overlaps(o))
            .collect();
        v.sort_by_key(|o| o.pos);
        v
    }

    /// The ops that make `clip`'s span free on its track by giving way —
    /// see [`Overlap::Trim`]. Ids for tail pieces come from `next_id`.
    /// Groups keep their pieces: a pierced member's tail joins every
    /// composition the member was in.
    pub fn make_room_ops(&self, clip: &Clip, exclude: &[ClipId], next_id: &mut u64) -> Vec<Op> {
        let mut ops = Vec::new();
        let mut group_adds: std::collections::BTreeMap<crate::GroupId, Vec<ClipId>> = Default::default();
        for o in self.overlappers(clip, exclude) {
            let covers_head = clip.pos <= o.pos;
            let covers_tail = clip.end() >= o.end();
            match (covers_head, covers_tail) {
                (true, true) => ops.push(Op::RemoveClip { id: o.id }),
                (true, false) => {
                    // Left-trim then slide: shrinking cannot collide, and the
                    // move lands in the space the shrink vacated.
                    let cut = clip.end().saturating_sub(o.pos);
                    ops.push(Op::TrimClip { id: o.id, src_in: o.src_in.saturating_add(cut),
                                            len: o.len.saturating_sub(cut) });
                    ops.push(Op::MoveClip { id: o.id, pos: clip.end(), track: o.track });
                }
                (false, true) => {
                    ops.push(Op::TrimClip { id: o.id, src_in: o.src_in, len: clip.pos.saturating_sub(o.pos) });
                }
                (false, false) => {
                    // Pierced: head keeps the id, the tail is new.
                    let head_len = clip.pos.saturating_sub(o.pos);
                    let tail_off = clip.end().saturating_sub(o.pos);
                    let tail = ClipId(*next_id);
                    *next_id += 1;
                    ops.push(Op::TrimClip { id: o.id, src_in: o.src_in, len: head_len });
                    ops.push(Op::AddClip { clip: Clip {
                        id: tail, source: o.source, track: o.track, pos: clip.end(),
                        src_in: o.src_in.saturating_add(tail_off), len: o.len.saturating_sub(tail_off),
                        gain: o.gain, effects: o.effects.clone(),
                    } });
                    for (gid, g) in &self.groups {
                        if g.clips.contains(&o.id) { group_adds.entry(*gid).or_default().push(tail); }
                    }
                }
            }
        }
        for (gid, adds) in group_adds {
            let g = &self.groups[&gid];
            let mut clips = g.clips.clone();
            clips.extend(adds);
            ops.push(Op::SetGroup { id: gid, name: g.name.clone(), clips });
        }
        ops
    }

    /// `track`'s shadow lane, if one exists: the track right after it
    /// whose name is the parent's plus [`LANE_SUFFIX`].
    pub fn shadow_lane(&self, track: TrackId) -> Option<TrackId> {
        let i = self.tracks.iter().position(|t| t.id == track)?;
        let parent = &self.tracks[i];
        let want = format!("{}{}", parent.name, LANE_SUFFIX);
        self.tracks.get(i + 1).filter(|t| t.name == want && t.kind == parent.kind).map(|t| t.id)
    }

    /// The op that creates `track`'s shadow lane as `id`: same kind, same
    /// route, same gain, unlocked, placed directly after the parent.
    pub fn shadow_lane_ops(&self, track: TrackId, id: TrackId) -> Result<Vec<Op>, EditError> {
        let i = self.tracks.iter().position(|t| t.id == track).ok_or(EditError::NoSuchTrack(track))?;
        let parent = &self.tracks[i];
        Ok(vec![Op::AddTrack { track: Track {
            id, kind: parent.kind, name: format!("{}{}", parent.name, LANE_SUFFIX),
            muted: parent.muted, locked: false, gain: parent.gain, route: parent.route,
        }, at: i + 1 }])
    }

    /// The parent of a shadow lane, if `track` is one.
    pub fn lane_parent(&self, track: TrackId) -> Option<TrackId> {
        let i = self.tracks.iter().position(|t| t.id == track)?;
        if i == 0 { return None; }
        let lane = &self.tracks[i];
        let parent = &self.tracks[i - 1];
        (lane.name == format!("{}{}", parent.name, LANE_SUFFIX) && lane.kind == parent.kind).then_some(parent.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BusId, GroupId, Source, SourceId, Ticks, TrackKind};

    /// One track with a long clip 1000..5000 (the offscreen take) and a
    /// source long enough for everything.
    fn with_long_take() -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio, name: "vox".into(),
            muted: false, locked: false, gain: 0.8, route: Some(BusId(3)) }, at: 0 }).ok();
        // The route's bus need not exist for these tests; AddTrack does
        // not validate it, and `shadow_lane_ops` copies it verbatim.
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "take.wav".into(),
            len: Ticks(100_000), sample_rate: 48_000, channels: 1, has_video: false, kind: crate::SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1),
            pos: Ticks(1000), src_in: Ticks(500), len: Ticks(4000), gain: 1.0, effects: vec![] } }).unwrap();
        p
    }
    fn newcomer(pos: i64, len: i64) -> Clip {
        Clip { id: ClipId(9), source: SourceId(1), track: TrackId(1), pos: Ticks(pos), src_in: Ticks::ZERO,
               len: Ticks(len), gain: 1.0, effects: vec![] }
    }
    fn land(p: &mut Project, c: Clip) {
        let mut next = 100;
        for op in p.make_room_ops(&c, &[], &mut next) { p.apply(&op).expect("room ops apply"); }
        p.apply(&Op::AddClip { clip: c }).expect("and then the newcomer fits");
    }

    #[test]
    fn a_drop_in_front_of_the_take_trims_its_head_and_the_audio_stays_in_time() {
        let mut p = with_long_take();
        land(&mut p, newcomer(0, 2000));           // covers 1000..2000 of the take's head
        let take = &p.clips[&ClipId(1)];
        assert_eq!(take.pos, Ticks(2000));
        assert_eq!(take.len, Ticks(3000));
        assert_eq!(take.src_in, Ticks(1500), "the take reads on from where the newcomer ends — no smear");
    }

    #[test]
    fn a_drop_on_the_tail_trims_it_and_a_drop_over_it_removes_it() {
        let mut p = with_long_take();
        land(&mut p, newcomer(4000, 3000));
        assert_eq!(p.clips[&ClipId(1)].len, Ticks(3000), "tail trimmed to the newcomer's start");
        let mut p = with_long_take();
        land(&mut p, newcomer(500, 6000));
        assert!(!p.clips.contains_key(&ClipId(1)), "wholly covered: removed");
        assert_eq!(p.clips.len(), 1);
    }

    #[test]
    fn a_drop_inside_the_take_pierces_it_and_the_tail_joins_its_composition() {
        let mut p = with_long_take();
        p.apply(&Op::SetGroup { id: GroupId(1), name: "verse".into(), clips: vec![ClipId(1)] }).unwrap();
        let mut next = 100;
        let c = newcomer(2000, 1000);
        let ops = p.make_room_ops(&c, &[], &mut next);
        for op in &ops { p.apply(op).unwrap(); }
        p.apply(&Op::AddClip { clip: c }).unwrap();
        let head = &p.clips[&ClipId(1)];
        assert_eq!((head.pos, head.len, head.src_in), (Ticks(1000), Ticks(1000), Ticks(500)));
        let tail = &p.clips[&ClipId(100)];
        assert_eq!((tail.pos, tail.len, tail.src_in), (Ticks(3000), Ticks(2000), Ticks(2500)),
                   "the tail reads the source where the head plus the hole leaves off");
        assert_eq!(p.groups[&GroupId(1)].clips, vec![ClipId(1), ClipId(100)]);
        assert_eq!(next, 101);
        // And every one of those ops was exactly invertible: undo them all.
        let mut q = with_long_take();
        q.apply(&Op::SetGroup { id: GroupId(1), name: "verse".into(), clips: vec![ClipId(1)] }).unwrap();
        let snapshot = q.clone();
        let mut inverses = Vec::new();
        for op in &ops { inverses.push(q.apply(op).unwrap()); }
        for inv in inverses.into_iter().rev() { q.apply(&inv).unwrap(); }
        assert_eq!(q, snapshot);
    }

    #[test]
    fn the_moving_set_is_never_its_own_obstacle_and_no_overlap_means_no_ops() {
        let p = with_long_take();
        let mut next = 100;
        // A member of the moving set that the placement covers is not trimmed.
        assert!(p.make_room_ops(&newcomer(0, 9000), &[ClipId(1)], &mut next).is_empty());
        // A placement in free space touches nothing.
        assert!(p.make_room_ops(&newcomer(5000, 1000), &[], &mut next).is_empty(), "end-to-end contact is not overlap");
        assert!(p.make_room_ops(&newcomer(0, 1000), &[], &mut next).is_empty());
    }

    #[test]
    fn a_shadow_lane_sits_after_its_parent_with_the_same_route_and_is_found_by_name() {
        let mut p = with_long_take();
        assert_eq!(p.shadow_lane(TrackId(1)), None);
        for op in p.shadow_lane_ops(TrackId(1), TrackId(7)).unwrap() { p.apply(&op).unwrap(); }
        assert_eq!(p.shadow_lane(TrackId(1)), Some(TrackId(7)));
        let lane = &p.tracks[1];
        assert_eq!(lane.name, "vox ↓");
        assert_eq!(lane.route, Some(BusId(3)), "same bus, so the same effects");
        assert_eq!(lane.gain, 0.8);
        assert_eq!(lane.kind, TrackKind::Audio);
        assert_eq!(p.lane_parent(TrackId(7)), Some(TrackId(1)));
        assert_eq!(p.lane_parent(TrackId(1)), None);
        assert!(matches!(p.shadow_lane_ops(TrackId(9), TrackId(8)), Err(EditError::NoSuchTrack(TrackId(9)))));
    }

    #[test]
    fn overlap_policy_names_round_trip() {
        for o in [Overlap::Refuse, Overlap::Trim, Overlap::Lane] {
            assert_eq!(Overlap::parse(o.name()), Some(o));
        }
        assert_eq!(Overlap::parse("garbage"), None);
        assert_eq!(Overlap::default(), Overlap::Trim, "newest wins is the default: refusing is the offscreen fight");
    }
}
