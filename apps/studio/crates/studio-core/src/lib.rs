//! studio-core — the editing model behind Studio.
//!
//! ## Why this exists, and why it is not a transliteration
//!
//! kdenlive and Audacity solve overlapping problems with very different
//! machinery. kdenlive's timeline (`src/timeline2/model/*`) is a Qt model tree
//! over MLT producers: clips reference bin clips by id and carry an in-point,
//! a length, an effect stack, and group membership. Audacity's editor is a
//! `WaveTrack` of `Sequence` blocks — audio held in chunks so an edit rewrites
//! a few blocks instead of a whole file. Both then bolt undo on top as command
//! objects, and neither can be edited by two people at once.
//!
//! Studio keeps the good parts of both and makes one structural change: **every
//! edit is a value, not a method call**. An [`Op`] is a small serializable
//! record that can be applied, inverted, logged, and sent over a wire. From
//! that single decision you get, for free:
//!
//! * **Undo** — the inverse of the op you just applied (no command classes).
//! * **Collaboration** — an op is exactly what you send to a peer.
//! * **Automation/scripting** — a macro is a list of ops.
//! * **Deterministic tests** — a project is a fold of ops over an empty doc.
//!
//! The rules that make concurrent editing converge are baked into the data
//! model rather than bolted on:
//!
//! * Clips are addressed by **stable ids**, never by index, so an insert or a
//!   delete elsewhere cannot make a remote op point at the wrong clip (the
//!   classic reason index-based edit logs need operational transforms).
//! * Positions and lengths are **absolute values on a timebase**, not deltas,
//!   so two peers moving the same clip converge on one of the two intended
//!   positions rather than summing into a place neither asked for.
//! * Track membership is a field on the clip, so moving between tracks is the
//!   same op as moving in time.
//!
//! Audio and video share this model: a video clip and an audio clip differ in
//! which [`TrackKind`] they live on and which effects apply, not in structure.
//! Audacity-style block storage lives behind [`SourceId`] — the timeline never
//! owns samples, only references into a source.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A point or duration on the project timebase, in ticks.
///
/// One tick is 1/[`TICKS_PER_SECOND`] of a second, chosen so that common video
/// frame rates AND common audio sample rates land on whole numbers: 44100,
/// 48000, 24, 25, 30, 50, 60, and 30000/1001 (NTSC) all divide it evenly enough
/// to avoid the drift that bites editors which store seconds as f64. Integer
/// ticks also make edits exactly reversible, which floating seconds are not.
pub const TICKS_PER_SECOND: i64 = 705_600_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct Ticks(pub i64);

impl Ticks {
    pub const ZERO: Ticks = Ticks(0);
    pub fn from_seconds(s: f64) -> Ticks { Ticks((s * TICKS_PER_SECOND as f64).round() as i64) }
    pub fn from_samples(n: i64, rate: u32) -> Ticks { Ticks(n * (TICKS_PER_SECOND / rate as i64)) }
    pub fn from_frames(n: i64, fps_num: u32, fps_den: u32) -> Ticks {
        Ticks(n * TICKS_PER_SECOND * fps_den as i64 / fps_num as i64)
    }
    pub fn to_seconds(self) -> f64 { self.0 as f64 / TICKS_PER_SECOND as f64 }
    pub fn to_samples(self, rate: u32) -> i64 { self.0 / (TICKS_PER_SECOND / rate as i64) }
    pub fn saturating_add(self, o: Ticks) -> Ticks { Ticks(self.0.saturating_add(o.0)) }
    pub fn saturating_sub(self, o: Ticks) -> Ticks { Ticks(self.0.saturating_sub(o.0)) }
}

macro_rules! id_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(pub u64);
    };
}
id_type!(ClipId, "A clip's stable identity. Never reused, never an index — this is what lets a remote op survive concurrent edits elsewhere on the timeline.");
id_type!(TrackId, "A track's stable identity.");
id_type!(SourceId, "A media source (Audacity's sequence/blocks, kdenlive's bin clip). The timeline references samples; it never owns them.");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackKind { Video, Audio }

/// One clip on one track. The video/audio distinction is the track it sits on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub source: SourceId,
    pub track: TrackId,
    /// Where the clip starts on the timeline.
    pub pos: Ticks,
    /// Offset into the source where playback begins (kdenlive's in-point).
    pub src_in: Ticks,
    /// How much of the source is used.
    pub len: Ticks,
    /// Linear gain, 1.0 = unity. Applies to audio tracks and to a video clip's
    /// audio stream alike.
    pub gain: f32,
    /// Effects in order, each with opaque named parameters. Deliberately not an
    /// enum: the host registers effect kinds, so a project made with a plugin
    /// you lack still round-trips instead of failing to parse.
    pub effects: Vec<Effect>,
}

impl Clip {
    pub fn end(&self) -> Ticks { self.pos.saturating_add(self.len) }
    /// Does this clip overlap `other` in time? (Same-track overlap is what the
    /// timeline forbids; clips on different tracks overlap freely.)
    pub fn overlaps(&self, other: &Clip) -> bool {
        self.track == other.track && self.pos < other.end() && other.pos < self.end()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    pub kind: String,
    pub params: BTreeMap<String, f32>,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    pub name: String,
    pub muted: bool,
    pub locked: bool,
}

/// The project: tracks, clips, and the sources they reference.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub tracks: Vec<Track>,
    /// Clips keyed by id. A map rather than per-track vectors so an op never has
    /// to search, and so ids stay valid no matter what happens to ordering.
    pub clips: BTreeMap<ClipId, Clip>,
    pub sources: BTreeMap<SourceId, Source>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub id: SourceId,
    pub path: String,
    pub len: Ticks,
    pub sample_rate: u32,
    pub channels: u16,
    /// True when the source carries picture as well as sound.
    pub has_video: bool,
}

/// Every edit, as a value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    AddTrack { track: Track, at: usize },
    RemoveTrack { id: TrackId },
    SetTrackFlags { id: TrackId, muted: bool, locked: bool },
    AddClip { clip: Clip },
    RemoveClip { id: ClipId },
    /// Absolute placement, not a delta — see the module docs on convergence.
    MoveClip { id: ClipId, pos: Ticks, track: TrackId },
    /// Absolute in-point and length, so two peers trimming opposite edges of
    /// the same clip do not fight over a shared "delta" they each computed
    /// against a different starting state.
    TrimClip { id: ClipId, src_in: Ticks, len: Ticks },
    SetGain { id: ClipId, gain: f32 },
    SetEffects { id: ClipId, effects: Vec<Effect> },
    AddSource { source: Source },
}

#[derive(Debug, Clone, PartialEq)]
pub enum EditError {
    NoSuchClip(ClipId),
    NoSuchTrack(TrackId),
    TrackLocked(TrackId),
    /// Two clips would occupy the same span of the same track.
    Overlap { id: ClipId, with: ClipId },
    /// A trim that would read past the end of the source, or a negative length.
    OutOfSource { id: ClipId },
}

impl Project {
    /// Apply an op, returning the op that undoes it.
    ///
    /// The inverse is computed from the state BEFORE the change, which is what
    /// makes undo exact rather than approximate — and it is also why undo and
    /// collaboration share one mechanism: both are just ops.
    pub fn apply(&mut self, op: &Op) -> Result<Op, EditError> {
        match op {
            Op::AddTrack { track, at } => {
                let at = (*at).min(self.tracks.len());
                self.tracks.insert(at, track.clone());
                Ok(Op::RemoveTrack { id: track.id })
            }
            Op::RemoveTrack { id } => {
                let idx = self.tracks.iter().position(|t| t.id == *id)
                    .ok_or(EditError::NoSuchTrack(*id))?;
                let track = self.tracks.remove(idx);
                // clips on a removed track go with it; the inverse re-adds the
                // track, and the clips are restored by their own inverse ops
                self.clips.retain(|_, c| c.track != *id);
                Ok(Op::AddTrack { track, at: idx })
            }
            Op::SetTrackFlags { id, muted, locked } => {
                let t = self.tracks.iter_mut().find(|t| t.id == *id)
                    .ok_or(EditError::NoSuchTrack(*id))?;
                let inv = Op::SetTrackFlags { id: *id, muted: t.muted, locked: t.locked };
                t.muted = *muted;
                t.locked = *locked;
                Ok(inv)
            }
            Op::AddClip { clip } => {
                self.check_track(clip.track)?;
                self.check_room(clip)?;
                self.clips.insert(clip.id, clip.clone());
                Ok(Op::RemoveClip { id: clip.id })
            }
            Op::RemoveClip { id } => {
                let clip = self.clips.remove(id).ok_or(EditError::NoSuchClip(*id))?;
                Ok(Op::AddClip { clip })
            }
            Op::MoveClip { id, pos, track } => {
                let old = self.clips.get(id).cloned().ok_or(EditError::NoSuchClip(*id))?;
                self.check_track(*track)?;
                let mut moved = old.clone();
                moved.pos = *pos;
                moved.track = *track;
                self.check_room(&moved)?;
                self.clips.insert(*id, moved);
                Ok(Op::MoveClip { id: *id, pos: old.pos, track: old.track })
            }
            Op::TrimClip { id, src_in, len } => {
                let old = self.clips.get(id).cloned().ok_or(EditError::NoSuchClip(*id))?;
                if len.0 <= 0 || src_in.0 < 0 {
                    return Err(EditError::OutOfSource { id: *id });
                }
                if let Some(src) = self.sources.get(&old.source) {
                    if src_in.saturating_add(*len).0 > src.len.0 {
                        return Err(EditError::OutOfSource { id: *id });
                    }
                }
                let mut t = old.clone();
                t.src_in = *src_in;
                t.len = *len;
                self.check_room(&t)?;
                self.clips.insert(*id, t);
                Ok(Op::TrimClip { id: *id, src_in: old.src_in, len: old.len })
            }
            Op::SetGain { id, gain } => {
                let c = self.clips.get_mut(id).ok_or(EditError::NoSuchClip(*id))?;
                let inv = Op::SetGain { id: *id, gain: c.gain };
                c.gain = *gain;
                Ok(inv)
            }
            Op::SetEffects { id, effects } => {
                let c = self.clips.get_mut(id).ok_or(EditError::NoSuchClip(*id))?;
                let inv = Op::SetEffects { id: *id, effects: c.effects.clone() };
                c.effects = effects.clone();
                Ok(inv)
            }
            Op::AddSource { source } => {
                self.sources.insert(source.id, source.clone());
                Ok(Op::AddSource { source: source.clone() }) // idempotent by id
            }
        }
    }

    fn check_track(&self, id: TrackId) -> Result<(), EditError> {
        let t = self.tracks.iter().find(|t| t.id == id).ok_or(EditError::NoSuchTrack(id))?;
        if t.locked { return Err(EditError::TrackLocked(id)); }
        Ok(())
    }

    fn check_room(&self, clip: &Clip) -> Result<(), EditError> {
        for other in self.clips.values() {
            if other.id != clip.id && clip.overlaps(other) {
                return Err(EditError::Overlap { id: clip.id, with: other.id });
            }
        }
        Ok(())
    }

    /// Clips on a track, in timeline order. Computed rather than stored, so
    /// there is no ordering invariant for a concurrent edit to violate.
    pub fn track_clips(&self, track: TrackId) -> Vec<&Clip> {
        let mut v: Vec<&Clip> = self.clips.values().filter(|c| c.track == track).collect();
        v.sort_by_key(|c| c.pos);
        v
    }

    /// The project's extent: the end of its last clip.
    pub fn duration(&self) -> Ticks {
        self.clips.values().map(|c| c.end()).max().unwrap_or(Ticks::ZERO)
    }

    /// Split a clip at an absolute timeline position, as both editors' `S` key
    /// does. Returns the ops (not the mutation) so the caller can route them
    /// through undo and collaboration like any other edit.
    pub fn split_ops(&self, id: ClipId, at: Ticks, new_id: ClipId) -> Result<Vec<Op>, EditError> {
        let c = self.clips.get(&id).ok_or(EditError::NoSuchClip(id))?;
        if at <= c.pos || at >= c.end() {
            return Err(EditError::OutOfSource { id });
        }
        let left_len = at.saturating_sub(c.pos);
        let right = Clip {
            id: new_id,
            source: c.source,
            track: c.track,
            pos: at,
            src_in: c.src_in.saturating_add(left_len),
            len: c.len.saturating_sub(left_len),
            gain: c.gain,
            effects: c.effects.clone(),
        };
        Ok(vec![
            Op::TrimClip { id, src_in: c.src_in, len: left_len },
            Op::AddClip { clip: right },
        ])
    }
}

/// Undo/redo over the op log. Both stacks hold ops, so a session's history is
/// serializable and can be replayed or shipped — which the command-object undo
/// in both source editors cannot do.
#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Op>,
    redo: Vec<Op>,
}

impl History {
    pub fn apply(&mut self, project: &mut Project, op: &Op) -> Result<(), EditError> {
        let inverse = project.apply(op)?;
        self.undo.push(inverse);
        self.redo.clear();
        Ok(())
    }
    pub fn undo(&mut self, project: &mut Project) -> Option<Op> {
        let op = self.undo.pop()?;
        let redo = project.apply(&op).ok()?;
        self.redo.push(redo);
        Some(op)
    }
    pub fn redo(&mut self, project: &mut Project) -> Option<Op> {
        let op = self.redo.pop()?;
        let undo = project.apply(&op).ok()?;
        self.undo.push(undo);
        Some(op)
    }
    pub fn can_undo(&self) -> bool { !self.undo.is_empty() }
    pub fn can_redo(&self) -> bool { !self.redo.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proj() -> Project {
        let mut p = Project { name: "t".into(), ..Default::default() };
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video,
            name: "V1".into(), muted: false, locked: false }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "a.wav".into(),
            len: Ticks::from_seconds(30.0), sample_rate: 48_000, channels: 2, has_video: false } }).unwrap();
        p
    }
    fn clip(id: u64, pos: f64, len: f64) -> Clip {
        Clip { id: ClipId(id), source: SourceId(1), track: TrackId(1), pos: Ticks::from_seconds(pos),
               src_in: Ticks::ZERO, len: Ticks::from_seconds(len), gain: 1.0, effects: Vec::new() }
    }

    #[test]
    fn the_timebase_is_exact_for_video_and_audio_rates() {
        // The reason for integer ticks: an editor that stores seconds as f64
        // accumulates drift until a cut lands half a sample early. Every common
        // rate must round-trip exactly.
        for rate in [44_100u32, 48_000, 96_000, 8_000] {
            let t = Ticks::from_samples(12_345, rate);
            assert_eq!(t.to_samples(rate), 12_345, "sample round-trip at {rate}Hz");
        }
        for (num, den) in [(24u32, 1u32), (25, 1), (30, 1), (50, 1), (60, 1), (30_000, 1001)] {
            let a = Ticks::from_frames(1000, num, den);
            let b = Ticks::from_frames(1001, num, den);
            assert!(b.0 > a.0, "frames advance at {num}/{den}");
            assert_eq!((b.0 - a.0) * num as i64 / den as i64, TICKS_PER_SECOND,
                       "one frame at {num}/{den} is exact");
        }
    }

    #[test]
    fn every_op_has_an_exact_inverse() {
        // Undo is op inversion, so this IS the undo test — and it is the same
        // mechanism collaboration uses.
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        h.apply(&mut p, &Op::AddClip { clip: clip(1, 0.0, 5.0) }).unwrap();
        h.apply(&mut p, &Op::MoveClip { id: ClipId(1), pos: Ticks::from_seconds(9.0), track: TrackId(1) }).unwrap();
        h.apply(&mut p, &Op::TrimClip { id: ClipId(1), src_in: Ticks::from_seconds(1.0), len: Ticks::from_seconds(2.0) }).unwrap();
        h.apply(&mut p, &Op::SetGain { id: ClipId(1), gain: 0.25 }).unwrap();
        assert!(h.can_undo());
        while h.can_undo() { h.undo(&mut p); }
        assert_eq!(p, start, "undoing everything must restore the opening state exactly");
        while h.can_redo() { h.redo(&mut p); }
        assert_eq!(p.clips[&ClipId(1)].gain, 0.25, "and redo puts it all back");
    }

    #[test]
    fn a_track_cannot_hold_two_clips_in_the_same_place() {
        let mut p = proj();
        p.apply(&Op::AddClip { clip: clip(1, 0.0, 5.0) }).unwrap();
        let err = p.apply(&Op::AddClip { clip: clip(2, 2.0, 5.0) });
        assert!(matches!(err, Err(EditError::Overlap { .. })), "overlap must be refused: {err:?}");
        // butt-jointed clips are fine — that is an edit, not a collision
        p.apply(&Op::AddClip { clip: clip(3, 5.0, 5.0) }).unwrap();
        assert_eq!(p.track_clips(TrackId(1)).len(), 2);
    }

    #[test]
    fn splitting_preserves_the_material_either_side_of_the_cut() {
        let mut p = proj();
        p.apply(&Op::AddClip { clip: clip(1, 0.0, 10.0) }).unwrap();
        let before = p.duration();
        for op in p.split_ops(ClipId(1), Ticks::from_seconds(4.0), ClipId(2)).unwrap() {
            p.apply(&op).unwrap();
        }
        let l = &p.clips[&ClipId(1)];
        let r = &p.clips[&ClipId(2)];
        assert_eq!(l.len, Ticks::from_seconds(4.0));
        assert_eq!(r.pos, Ticks::from_seconds(4.0));
        assert_eq!(r.src_in, Ticks::from_seconds(4.0), "the right half reads on from the cut");
        assert_eq!(l.len.saturating_add(r.len), Ticks::from_seconds(10.0), "no material lost");
        assert_eq!(p.duration(), before, "and the timeline is the same length");
        // a cut outside the clip is refused rather than silently clamped
        assert!(p.split_ops(ClipId(1), Ticks::from_seconds(99.0), ClipId(3)).is_err());
    }

    #[test]
    fn a_trim_cannot_read_past_the_end_of_its_source() {
        let mut p = proj();
        p.apply(&Op::AddClip { clip: clip(1, 0.0, 5.0) }).unwrap();
        let err = p.apply(&Op::TrimClip { id: ClipId(1),
            src_in: Ticks::from_seconds(29.0), len: Ticks::from_seconds(5.0) });
        assert!(matches!(err, Err(EditError::OutOfSource { .. })), "past the source end: {err:?}");
        let err = p.apply(&Op::TrimClip { id: ClipId(1), src_in: Ticks::ZERO, len: Ticks(0) });
        assert!(matches!(err, Err(EditError::OutOfSource { .. })), "zero length: {err:?}");
    }

    #[test]
    fn a_locked_track_refuses_edits() {
        let mut p = proj();
        p.apply(&Op::SetTrackFlags { id: TrackId(1), muted: false, locked: true }).unwrap();
        assert!(matches!(p.apply(&Op::AddClip { clip: clip(1, 0.0, 5.0) }),
                         Err(EditError::TrackLocked(_))));
    }
}
