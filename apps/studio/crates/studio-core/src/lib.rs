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
id_type!(BusId, "A bus's stable identity. Busses form a routing graph (parent hierarchy plus sends), not a flat list, which is why they get ids rather than being addressed by position.");
id_type!(GroupId, "A composition's stable identity: a named set of clips the timeline folds to one block — kdenlive's pre-batched group, After Effects' precomp, as the smallest model that supports the gesture. Membership is a PROJECT fact (a collaborator sees your composition); folded open or closed is view state in the device's workspace sidecar, the solo rule.");

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
    /// The track's fader, applied to every clip it carries (multiplied with
    /// each clip's own gain) — the mixer's per-track level. `serde(default)`
    /// to UNITY, deliberately not to `f32::default()`: a pre-fader project
    /// loading silent would be the worst possible reading of "missing
    /// field".
    #[serde(default = "unity_gain")]
    pub gain: f32,
    /// Which bus this track's signal feeds. `None` means straight to master —
    /// the implicit root every bus graph has whether or not any `Bus` value
    /// exists for it, so a project with no busses at all still mixes exactly
    /// as it always did.
    pub route: Option<BusId>,
}

/// A send: a parallel, weighted copy of a bus's signal into another bus.
/// Distinct from `parent` (the bus's single primary output) — a bus has at
/// most one parent but any number of sends, which is what lets a vocal bus
/// feed both master (via parent) and a shared reverb bus (via a send) at
/// once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Send {
    pub to: BusId,
    /// Linear gain on the sent copy, 1.0 = unity.
    pub level: f32,
    /// Tapped before this bus's own fader/chain gain is applied, vs. after.
    pub pre_fader: bool,
}

/// A mixer bus: something tracks or other busses route into, with its own
/// gain and effect chain, that in turn routes into a parent (or master, if
/// `parent` is `None`). The vocals/drums/808/melody separation the whole
/// point of a trap mixer is just four `Bus` values with distinct `parent`s
/// (or none) — nothing more structural than that.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bus {
    pub id: BusId,
    pub name: String,
    /// This bus's primary output. `None` means master.
    pub parent: Option<BusId>,
    /// Linear gain, 1.0 = unity.
    pub gain: f32,
    pub chain: Vec<Effect>,
    pub sends: Vec<Send>,
    /// A muted bus (and everything summed into it) is absent from both its
    /// sends and its parent — mute is an absolute override, checked before
    /// solo, which keeps the two rules independent rather than needing a
    /// combined truth table.
    pub muted: bool,
}

/// The project: tracks, clips, the sources they reference, and the bus graph
/// they route into.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub tracks: Vec<Track>,
    /// Clips keyed by id. A map rather than per-track vectors so an op never has
    /// to search, and so ids stay valid no matter what happens to ordering.
    pub clips: BTreeMap<ClipId, Clip>,
    pub sources: BTreeMap<SourceId, Source>,
    /// Busses keyed by id. Empty is a valid, common project: every track
    /// routes to the implicit master and mixes exactly as before busses
    /// existed at all.
    pub busses: BTreeMap<BusId, Bus>,
    pub name: String,
    /// Automation lanes: points per target, each lane sorted by time. A
    /// lane RIDES its fader (the render multiplies the two), it does not
    /// replace it — the fader stays a trim you can grab without fighting
    /// the curve, and a project with no lanes renders bit-identically to
    /// one from before lanes existed. Sparse: an emptied lane drops its
    /// entry, and a target that was removed may leave a dangling lane —
    /// harmless (the render skips unknown targets) and cheaper than making
    /// `RemoveTrack` responsible for a second cleanup undo can't restore.
    /// `serde(default)` so any pre-automation serialization still loads.
    #[serde(default)]
    pub automation: BTreeMap<AutoTarget, Vec<(Ticks, f32)>>,
    /// Compositions (see [`GroupId`]). A clip may sit in several groups; a
    /// removed clip may dangle in one — the automation rule again:
    /// harmless, filtered at draw time, cheaper than a second cleanup
    /// undo could not restore.
    #[serde(default)]
    pub groups: BTreeMap<GroupId, Group>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub name: String,
    pub clips: Vec<ClipId>,
}

/// What an automation lane drives. Track and bus gain first — the volume
/// ride is the automation everyone reaches for — with the enum here so a
/// clip-gain or effect-parameter target is a variant away, not a redesign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AutoTarget {
    TrackGain(TrackId),
    BusGain(BusId),
}

/// A lane's value at `at`: linear between points, held flat before the
/// first and after the last (the ends are what the automation last said,
/// not a cliff back to unity), and 1.0 — a transparent ride — for a lane
/// with no points at all.
/// [`eval_lane`] for a caller sweeping time FORWARD, which every renderer
/// is: `cursor` remembers the segment last used, so a lane of ten thousand
/// points costs one comparison per sample instead of a binary search.
/// Falls back to the search whenever `at` moves backwards (a loop wrap, a
/// seek), so it is exactly `eval_lane` for any input — pinned by a test
/// that sweeps both and compares every sample.
pub fn eval_lane_cursor(points: &[(Ticks, f32)], at: Ticks, cursor: &mut usize) -> f32 {
    if points.is_empty() { return 1.0; }
    if *cursor >= points.len() || (*cursor > 0 && points[*cursor - 1].0 > at) {
        *cursor = points.partition_point(|(t, _)| *t <= at);
    }
    while *cursor < points.len() && points[*cursor].0 <= at { *cursor += 1; }
    let i = *cursor;
    if i == 0 { return points[0].1.max(0.0); }
    if i == points.len() { return points[i - 1].1.max(0.0); }
    let (t0, v0) = points[i - 1];
    let (t1, v1) = points[i];
    let span = (t1.0 - t0.0).max(1) as f64;
    let frac = ((at.0 - t0.0) as f64 / span) as f32;
    (v0 + (v1 - v0) * frac).max(0.0)
}

pub fn eval_lane(points: &[(Ticks, f32)], at: Ticks) -> f32 {
    if points.is_empty() { return 1.0; }
    let i = points.partition_point(|(t, _)| *t <= at);
    if i == 0 { return points[0].1.max(0.0); }
    if i == points.len() { return points[i - 1].1.max(0.0); }
    let (t0, v0) = points[i - 1];
    let (t1, v1) = points[i];
    let span = (t1.0 - t0.0).max(1) as f64;
    let frac = ((at.0 - t0.0) as f64 / span) as f32;
    (v0 + (v1 - v0) * frac).max(0.0)
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

fn unity_gain() -> f32 { 1.0 }

/// Every edit, as a value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    AddTrack { track: Track, at: usize },
    RemoveTrack { id: TrackId },
    SetTrackFlags { id: TrackId, muted: bool, locked: bool },
    /// The track fader. Refused on a locked track — the fader is a mix
    /// edit, not the lock toggle; `SetTrackFlags` stays the only edit a
    /// locked track accepts, because something has to be able to unlock it.
    SetTrackGain { id: TrackId, gain: f32 },
    /// Place (or move, or restate) one automation point: absolute (time,
    /// value), so two peers editing the same lane converge per-point the
    /// way clips converge per-field — a point at the same tick is the same
    /// point, and the last writer's value wins on every replica. Refused on
    /// a locked track for `SetTrackGain`'s reason: a ride is a mix edit.
    SetAutoPoint { target: AutoTarget, at: Ticks, value: f32 },
    /// Remove the point at exactly (target, at). Refused when there is no
    /// such point — the inverse an undo needs (`SetAutoPoint` of the old
    /// value) would otherwise have nothing to restore.
    RemoveAutoPoint { target: AutoTarget, at: Ticks },
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
    /// Refused while any clip still references this source (see
    /// `Op::RemoveTrack`/`Op::RemoveBus` for the same "in use" refusal
    /// pattern) — before this variant existed, `Op::AddSource`'s own
    /// "inverse" was itself (a workaround for idempotent duplicate catch-up
    /// delivery, not a real undo), which meant recording a source and then
    /// undoing left the source permanently in the project. This is the fix.
    RemoveSource { id: SourceId },
    /// Refused if `bus.parent` doesn't exist, or is the bus's own id (a
    /// direct self-loop) — see `Project::apply`'s cycle check for why a
    /// longer cycle can never arise from `AddBus` alone.
    AddBus { bus: Bus },
    /// Refused while anything still points at this bus — a track routed to
    /// it, another bus's `parent`, or another bus's `sends` — so deletion
    /// never silently reroutes audio; the caller re-routes first, same
    /// principle as a locked track refusing edits.
    RemoveBus { id: BusId },
    SetBusGain { id: BusId, gain: f32 },
    /// Bus-level mute, persisted like `SetTrackFlags` — unlike solo, which
    /// (as for tracks) is a mixdown-time parameter, not project state: it's
    /// a listening choice for right now, not an edit worth undoing or
    /// shipping to a collaborator.
    SetBusFlags { id: BusId, muted: bool },
    SetBusChain { id: BusId, chain: Vec<Effect> },    /// Replaces the whole sends list, like `SetEffects` does for a clip's
    /// effects — the inverse is then just "the old vector," no per-send
    /// add/remove bookkeeping needed.
    SetBusSends { id: BusId, sends: Vec<Send> },
    /// `bus: None` routes the track straight to master.
    SetRoute { track: TrackId, bus: Option<BusId> },
    /// Re-parent a bus — the INPUT side of the graph, so a mixer strip can
    /// say "this feeds me" instead of routing only ever being edited from
    /// the source end and only at creation. `None` is master. Refused when
    /// the new parent chain reaches back to `id`: a bus cannot feed itself.
    SetBusParent { id: BusId, parent: Option<BusId> },
    /// Create or restate a composition, absolutely — name and full
    /// membership every time, the LWW-friendly shape. Refused empty: a
    /// group of nothing folds to nothing.
    SetGroup { id: GroupId, name: String, clips: Vec<ClipId> },
    /// Dissolve a composition. Its clips are untouched — ungrouping is an
    /// organizational act, never an edit to the music.
    RemoveGroup { id: GroupId },
    /// Several ops as ONE step: applied in order, inverted as a whole (the
    /// inverse is the members' inverses in reverse). A partial application
    /// rolls back what landed — half an edit is worse than none. This is
    /// what lets a multi-move mastering pass be a single press of `u`.
    Group { ops: Vec<Op> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum EditError {
    NoSuchClip(ClipId),
    /// A `RemoveAutoPoint` aimed at a point that is not there.
    NoSuchAutoPoint,
    /// A `SetGroup` with no members.
    EmptyGroup,
    NoSuchGroup(GroupId),
    NoSuchTrack(TrackId),
    TrackLocked(TrackId),
    /// Refused because clips are still on this track — see `Op::RemoveTrack`.
    /// Removing them (or moving them off first) is the caller's job; a
    /// track's own removal must never be the thing that silently deletes
    /// clips undo can't bring back.
    TrackInUse(TrackId),
    /// Two clips would occupy the same span of the same track.
    Overlap { id: ClipId, with: ClipId },
    /// A trim that would read past the end of the source, or a negative length.
    OutOfSource { id: ClipId },
    NoSuchBus(BusId),
    /// Something still routes into this bus — see `Op::RemoveBus`.
    BusInUse(BusId),
    /// Adding this edge (a `parent` or a `send`) would close a loop back to
    /// `id`, which would make mixdown never terminate.
    BusCycle { id: BusId },
    NoSuchSource(SourceId),
    /// A clip still references this source — see `Op::RemoveSource`.
    SourceInUse(SourceId),
}

impl Project {
    /// Apply an op, returning the op that undoes it.
    ///
    /// The inverse is computed from the state BEFORE the change, which is what
    /// makes undo exact rather than approximate — and it is also why undo and
    /// collaboration share one mechanism: both are just ops.
    /// An automation target must exist to be ridden, and a locked track
    /// refuses its lane the way it refuses its fader.
    fn check_auto_target(&self, target: AutoTarget) -> Result<(), EditError> {
        match target {
            AutoTarget::TrackGain(id) => self.check_track(id),
            AutoTarget::BusGain(id) => {
                if self.busses.contains_key(&id) { Ok(()) } else { Err(EditError::NoSuchBus(id)) }
            }
        }
    }

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
                // Removing a track that still has clips on it would need to
                // return more than one inverse op (the track AND every clip)
                // to be undoable — and `apply` only ever returns one. Rather
                // than silently cascading and losing those clips on undo
                // (which is what this arm used to do — a real bug, caught
                // by noticing the comment describing "the clips' own
                // inverse ops" promised something the return type can't
                // deliver), this refuses outright, the same "in use" refusal
                // `Op::RemoveBus` already applies for exactly this reason.
                // The caller removes the clips first — each of THOSE has its
                // own exact single-op inverse, so nothing is lost either way.
                if self.clips.values().any(|c| c.track == *id) {
                    return Err(EditError::TrackInUse(*id));
                }
                let track = self.tracks.remove(idx);
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
            Op::SetTrackGain { id, gain } => {
                self.check_track(*id)?; // exists, and not locked
                let t = self.tracks.iter_mut().find(|t| t.id == *id)
                    .ok_or(EditError::NoSuchTrack(*id))?;
                let inv = Op::SetTrackGain { id: *id, gain: t.gain };
                t.gain = *gain;
                Ok(inv)
            }
            Op::SetAutoPoint { target, at, value } => {
                self.check_auto_target(*target)?;
                let lane = self.automation.entry(*target).or_default();
                match lane.iter_mut().find(|(t, _)| t == at) {
                    Some(p) => {
                        // Restating a point at its own tick is a value
                        // change; the inverse restores the old value.
                        let inv = Op::SetAutoPoint { target: *target, at: *at, value: p.1 };
                        p.1 = *value;
                        Ok(inv)
                    }
                    None => {
                        lane.push((*at, *value));
                        lane.sort_by_key(|(t, _)| *t);
                        Ok(Op::RemoveAutoPoint { target: *target, at: *at })
                    }
                }
            }
            Op::RemoveAutoPoint { target, at } => {
                self.check_auto_target(*target)?;
                let lane = self.automation.get_mut(target)
                    .ok_or(EditError::NoSuchAutoPoint)?;
                let i = lane.iter().position(|(t, _)| t == at)
                    .ok_or(EditError::NoSuchAutoPoint)?;
                let (_, old) = lane.remove(i);
                // An emptied lane drops its entry — sparse, the same rule
                // `Pattern::set_mod` follows one crate over.
                if lane.is_empty() { self.automation.remove(target); }
                Ok(Op::SetAutoPoint { target: *target, at: *at, value: old })
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
                if let Some(old) = self.sources.get(&source.id) {
                    // idempotent by id — a duplicate catch-up delivery
                    // re-affirms the same source, not a fresh insert, so
                    // its inverse is "put back whatever was already there,"
                    // not the new value (that was the bug: the old code
                    // returned `source.clone()` here, which is only a
                    // correct inverse if the two happen to be identical).
                    let old = old.clone();
                    self.sources.insert(source.id, source.clone());
                    return Ok(Op::AddSource { source: old });
                }
                self.sources.insert(source.id, source.clone());
                Ok(Op::RemoveSource { id: source.id })
            }
            Op::RemoveSource { id } => {
                if self.clips.values().any(|c| c.source == *id) {
                    return Err(EditError::SourceInUse(*id));
                }
                let source = self.sources.remove(id).ok_or(EditError::NoSuchSource(*id))?;
                Ok(Op::AddSource { source })
            }
            Op::AddBus { bus } => {
                if self.busses.contains_key(&bus.id) {
                    // idempotent by id, same as AddSource — a duplicate
                    // catch-up delivery must not error the joiner.
                    let old = self.busses.insert(bus.id, bus.clone());
                    return Ok(Op::AddBus { bus: old.unwrap() });
                }
                if let Some(p) = bus.parent {
                    if p == bus.id {
                        return Err(EditError::BusCycle { id: bus.id });
                    }
                    self.check_bus(p)?;
                }
                // A brand-new id cannot already be reachable from anywhere
                // else in the graph, so a direct self-loop (just handled
                // above) is the only cycle `AddBus` can introduce on its
                // own; the general check below is a backstop, not load-
                // bearing, but kept for symmetry with the other bus ops.
                if let Some(p) = bus.parent {
                    if self.would_create_cycle(bus.id, p) {
                        return Err(EditError::BusCycle { id: bus.id });
                    }
                }
                self.busses.insert(bus.id, bus.clone());
                Ok(Op::RemoveBus { id: bus.id })
            }
            Op::RemoveBus { id } => {
                let bus = self.busses.get(id).cloned().ok_or(EditError::NoSuchBus(*id))?;
                let in_use = self.tracks.iter().any(|t| t.route == Some(*id))
                    || self.busses.values().any(|b| b.id != *id && (b.parent == Some(*id) || b.sends.iter().any(|s| s.to == *id)));
                if in_use {
                    return Err(EditError::BusInUse(*id));
                }
                self.busses.remove(id);
                Ok(Op::AddBus { bus })
            }
            Op::SetBusGain { id, gain } => {
                let b = self.busses.get_mut(id).ok_or(EditError::NoSuchBus(*id))?;
                let inv = Op::SetBusGain { id: *id, gain: b.gain };
                b.gain = *gain;
                Ok(inv)
            }
            Op::SetBusFlags { id, muted } => {
                let b = self.busses.get_mut(id).ok_or(EditError::NoSuchBus(*id))?;
                let inv = Op::SetBusFlags { id: *id, muted: b.muted };
                b.muted = *muted;
                Ok(inv)
            }
            Op::SetBusChain { id, chain } => {
                let b = self.busses.get_mut(id).ok_or(EditError::NoSuchBus(*id))?;
                let inv = Op::SetBusChain { id: *id, chain: b.chain.clone() };
                b.chain = chain.clone();
                Ok(inv)
            }
            Op::SetBusSends { id, sends } => {
                self.check_bus(*id)?;
                for s in sends {
                    self.check_bus(s.to)?;
                    // Checked against this bus's OWN sends excluded (they're
                    // what's being replaced) so re-setting an unchanged sends
                    // list is never spuriously refused for "depending on
                    // itself".
                    if self.would_create_cycle_excluding(*id, s.to, *id) {
                        return Err(EditError::BusCycle { id: *id });
                    }
                }
                let b = self.busses.get_mut(id).unwrap();
                let inv = Op::SetBusSends { id: *id, sends: b.sends.clone() };
                b.sends = sends.clone();
                Ok(inv)
            }
            Op::SetRoute { track, bus } => {
                self.check_track(*track)?;
                if let Some(b) = bus {
                    self.check_bus(*b)?;
                }
                let t = self.tracks.iter_mut().find(|t| t.id == *track).unwrap();
                let inv = Op::SetRoute { track: *track, bus: t.route };
                t.route = *bus;
                Ok(inv)
            }
            Op::SetBusParent { id, parent } => {
                let Some(b) = self.busses.get(id) else {
                    return Err(EditError::NoSuchBus(*id));
                };
                if let Some(p) = parent {
                    if !self.busses.contains_key(p) {
                        return Err(EditError::NoSuchBus(*p));
                    }
                    // Walk the proposed parent's chain; reaching `id` means
                    // the graph would feed itself. Capped at the bus count
                    // so even a corrupt project terminates.
                    let mut at = *p;
                    for _ in 0..=self.busses.len() {
                        if at == *id { return Err(EditError::BusCycle { id: *id }); }
                        match self.busses.get(&at).and_then(|b| b.parent) {
                            Some(next) => at = next,
                            None => break,
                        }
                    }
                }
                let inv = Op::SetBusParent { id: *id, parent: b.parent };
                self.busses.get_mut(id).unwrap().parent = *parent;
                Ok(inv)
            }
            Op::SetGroup { id, name, clips } => {
                if clips.is_empty() {
                    return Err(EditError::EmptyGroup);
                }
                let inv = match self.groups.get(id) {
                    Some(old) => Op::SetGroup { id: *id, name: old.name.clone(),
                                                clips: old.clips.clone() },
                    None => Op::RemoveGroup { id: *id },
                };
                self.groups.insert(*id, Group { name: name.clone(), clips: clips.clone() });
                Ok(inv)
            }
            Op::Group { ops } => {
                let mut inverses = Vec::with_capacity(ops.len());
                for op in ops {
                    match self.apply(op) {
                        Ok(inv) => inverses.push(inv),
                        Err(e) => {
                            for inv in inverses.into_iter().rev() {
                                let _ = self.apply(&inv);
                            }
                            return Err(e);
                        }
                    }
                }
                inverses.reverse();
                Ok(Op::Group { ops: inverses })
            }
            Op::RemoveGroup { id } => {
                let old = self.groups.remove(id).ok_or(EditError::NoSuchGroup(*id))?;
                Ok(Op::SetGroup { id: *id, name: old.name, clips: old.clips })
            }
        }
    }

    fn check_bus(&self, id: BusId) -> Result<(), EditError> {
        if self.busses.contains_key(&id) { Ok(()) } else { Err(EditError::NoSuchBus(id)) }
    }

    /// Would adding an edge `from -> to` (a `parent` pointer or a `send`)
    /// close a loop back to `from`? True exactly when `to` can already reach
    /// `from` by following existing `parent`/`send` edges — including the
    /// trivial case `from == to`, a direct self-loop.
    fn would_create_cycle(&self, from: BusId, to: BusId) -> bool {
        self.reachable(to, from, None)
    }

    /// Same check, but pretend `exclude`'s own `sends` don't exist while
    /// searching — used by `SetBusSends`, which is replacing that bus's
    /// sends and must not be blocked by the very edges it's about to
    /// overwrite. Its `parent` edge (a different field, unaffected by this
    /// op) still counts.
    fn would_create_cycle_excluding(&self, from: BusId, to: BusId, exclude: BusId) -> bool {
        self.reachable(to, from, Some(exclude))
    }

    fn reachable(&self, from: BusId, to: BusId, exclude_sends_of: Option<BusId>) -> bool {
        let mut stack = vec![from];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(cur) = stack.pop() {
            if cur == to {
                return true;
            }
            if !seen.insert(cur) {
                continue;
            }
            if let Some(b) = self.busses.get(&cur) {
                if let Some(p) = b.parent {
                    stack.push(p);
                }
                if Some(cur) != exclude_sends_of {
                    for s in &b.sends {
                        stack.push(s.to);
                    }
                }
            }
        }
        false
    }

    /// A processing order for the bus graph: every bus appears after
    /// everything it depends on (its parent, its sends) has already been
    /// resolved — children before parents, so a mixdown fold can sum a bus
    /// only once every input that feeds it is already computed.
    ///
    /// `apply` refuses cycles as they're introduced, so this should never
    /// see one from a project built through `Op`s — but a loaded or foreign
    /// file did not necessarily come from that path, and a cycle here would
    /// otherwise mean mixdown never terminates. So this is checked
    /// independently rather than trusted, matching the "a damaged file is
    /// refused outright" rule the rest of Studio's persistence follows.
    pub fn bus_order(&self) -> Result<Vec<BusId>, EditError> {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark { Temp, Perm }
        fn visit(
            id: BusId,
            busses: &BTreeMap<BusId, Bus>,
            marks: &mut BTreeMap<BusId, Mark>,
            out: &mut Vec<BusId>,
        ) -> Result<(), EditError> {
            match marks.get(&id) {
                Some(Mark::Perm) => return Ok(()),
                Some(Mark::Temp) => return Err(EditError::BusCycle { id }),
                None => {}
            }
            marks.insert(id, Mark::Temp);
            if let Some(b) = busses.get(&id) {
                let mut targets: Vec<BusId> = Vec::new();
                if let Some(p) = b.parent {
                    targets.push(p);
                }
                for s in &b.sends {
                    targets.push(s.to);
                }
                for t in targets {
                    visit(t, busses, marks, out)?;
                }
            }
            marks.insert(id, Mark::Perm);
            out.push(id);
            Ok(())
        }
        let mut marks = BTreeMap::new();
        let mut out = Vec::new();
        for id in self.busses.keys().copied() {
            visit(id, &self.busses, &mut marks, &mut out)?;
        }
        // `visit` finishes a node only after everything it points to is
        // fully resolved, so the raw push order has parents before the
        // children that feed them — the reverse of what a mixdown fold
        // wants (children summed first, then rolled into the parent).
        out.reverse();
        Ok(out)
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
    /// Positions in `undo` where an `apply_group` call began — the marker
    /// `undo_group` unwinds back to. A plain `apply` never pushes one of
    /// these, so single-op undo/redo behave exactly as before; this is
    /// purely additive.
    undo_group_starts: Vec<usize>,
    /// The `redo`-side mirror, recorded by `undo_group` so a full group can
    /// be redone as a unit too.
    redo_group_starts: Vec<usize>,
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

    /// Apply every op in `ops` as one undo group: a single later
    /// `undo_group` call reverts all of them together. This is what makes
    /// `studio-copilot`'s `Proposal.ops` — several ops from one findings-
    /// backed suggestion — undo as the one action the user perceived
    /// applying, rather than needing N separate presses of undo to get back
    /// to where they started.
    ///
    /// If any op fails partway through, everything already applied in this
    /// call is rolled back (via the ordinary single-op `undo` machinery) and
    /// the project is left exactly as it was before the call — a group
    /// either fully lands or leaves no trace, never half of one.
    pub fn apply_group(&mut self, project: &mut Project, ops: &[Op]) -> Result<(), EditError> {
        let start = self.undo.len();
        for op in ops {
            if let Err(e) = self.apply(project, op) {
                while self.undo.len() > start {
                    self.undo(project);
                }
                // A failed group never really succeeded, so there is
                // nothing meaningful left to redo — clear the trail the
                // rollback's own `undo()` calls just built.
                self.redo.clear();
                return Err(e);
            }
        }
        self.undo_group_starts.push(start);
        Ok(())
    }

    pub fn can_undo_group(&self) -> bool { !self.undo_group_starts.is_empty() }
    pub fn can_redo_group(&self) -> bool { !self.redo_group_starts.is_empty() }

    /// Undo back to the start of the most recent group boundary, however
    /// many individual `undo` steps that takes from wherever the stack
    /// currently is. Returns how many ops it actually undid.
    ///
    /// This is deliberately robust to interleaving with plain `undo`/`redo`
    /// rather than assuming a fixed count: if some of the group's entries
    /// were already undone one at a time before this is called, it simply
    /// finishes what's left — the `while` condition naturally stops at the
    /// group's start position regardless of how the stack got there. See
    /// this module's tests for exactly that scenario pinned as a real case,
    /// not a hypothetical.
    pub fn undo_group(&mut self, project: &mut Project) -> usize {
        let Some(start) = self.undo_group_starts.pop() else { return 0 };
        let mut n = 0;
        while self.undo.len() > start {
            if self.undo(project).is_none() { break; }
            n += 1;
        }
        if n > 0 {
            self.redo_group_starts.push(self.redo.len() - n);
        }
        n
    }

    /// The `redo` counterpart: re-apply everything a preceding `undo_group`
    /// call undid, as one action. Returns how many ops it redid.
    pub fn redo_group(&mut self, project: &mut Project) -> usize {
        let Some(start) = self.redo_group_starts.pop() else { return 0 };
        let mut n = 0;
        while self.redo.len() > start {
            if self.redo(project).is_none() { break; }
            n += 1;
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proj() -> Project {
        let mut p = Project { name: "t".into(), ..Default::default() };
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video,
            name: "V1".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
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

    #[test]
    fn removing_a_track_with_clips_on_it_is_refused_not_silently_cascaded() {
        // Regression test for a real bug: RemoveTrack used to silently
        // delete every clip on the track as a side effect, but `apply`
        // returns exactly one inverse Op — so undo only brought the TRACK
        // back, not the clips. The fix is the same "in use" refusal
        // Op::RemoveBus already uses: the caller removes the clips first.
        let mut p = proj();
        p.apply(&Op::AddClip { clip: clip(1, 0.0, 5.0) }).unwrap();
        let err = p.apply(&Op::RemoveTrack { id: TrackId(1) });
        assert!(matches!(err, Err(EditError::TrackInUse(TrackId(1)))), "{err:?}");
        assert_eq!(p.tracks.len(), 1, "the refused removal must leave the track in place");
        assert!(p.clips.contains_key(&ClipId(1)), "and the clip must still be there too");
    }

    #[test]
    fn removing_an_empty_track_still_works_and_undoes_cleanly() {
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        h.apply(&mut p, &Op::RemoveTrack { id: TrackId(1) }).unwrap();
        assert!(p.tracks.is_empty());
        h.undo(&mut p);
        assert_eq!(p, start, "removing and undoing an empty track must round-trip exactly, same as before this fix");
    }

    #[test]
    fn removing_a_track_with_clips_via_a_group_first_removes_clips_then_the_track_and_undoes_as_one() {
        // This is the actual right way to do "delete this track and
        // everything on it" now: bundle RemoveClip for every clip plus the
        // final RemoveTrack into one apply_group call, so the whole
        // compound action undoes together as the single thing the user
        // perceived doing — the payoff of the undo-group work pairing with
        // this fix, not two unrelated changes.
        let mut p = proj();
        p.apply(&Op::AddClip { clip: clip(1, 0.0, 5.0) }).unwrap();
        p.apply(&Op::AddClip { clip: clip(2, 6.0, 2.0) }).unwrap();
        let before_group = p.clone();

        let mut h = History::default();
        // proj()'s own AddTrack/AddClip calls above weren't through `h`, so
        // start a fresh group covering just the removal itself.
        h.apply_group(&mut p, &[
            Op::RemoveClip { id: ClipId(1) },
            Op::RemoveClip { id: ClipId(2) },
            Op::RemoveTrack { id: TrackId(1) },
        ]).unwrap();
        assert!(p.tracks.is_empty());
        assert!(p.clips.is_empty());

        let n = h.undo_group(&mut p);
        assert_eq!(n, 3);
        assert_eq!(p, before_group, "one undo_group call should restore both clips and the track together");
    }

    #[test]
    fn removing_a_source_still_referenced_by_a_clip_is_refused() {
        let mut p = proj();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "a.wav".into(),
            len: Ticks::from_seconds(10.0), sample_rate: 48_000, channels: 2, has_video: false } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1),
            pos: Ticks::ZERO, src_in: Ticks::ZERO, len: Ticks::from_seconds(2.0), gain: 1.0, effects: Vec::new() } }).unwrap();
        let err = p.apply(&Op::RemoveSource { id: SourceId(1) });
        assert!(matches!(err, Err(EditError::SourceInUse(SourceId(1)))), "{err:?}");
        assert!(p.sources.contains_key(&SourceId(1)), "the refused removal must leave the source in place");
    }

    #[test]
    fn removing_an_unreferenced_source_works_and_undoes_cleanly() {
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        h.apply(&mut p, &Op::AddSource { source: Source { id: SourceId(1), path: "a.wav".into(),
            len: Ticks::from_seconds(10.0), sample_rate: 48_000, channels: 2, has_video: false } }).unwrap();
        assert_eq!(p.sources.len(), 1);
        h.undo(&mut p);
        assert_eq!(p, start, "adding then undoing a source must fully round-trip — the bug this fixes is that it previously didn't");
    }

    #[test]
    fn recording_a_take_now_fully_undoes_via_apply_group_source_included() {
        // The exact regression this fix targets, expressed at the Op level
        // (studio-live's own test covers the same thing through
        // Recorder::commit_ops directly): AddSource followed by AddClip,
        // applied as one group, must undo BOTH — not leave an orphaned
        // source behind the way the old "AddSource's inverse is itself"
        // workaround did.
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        h.apply_group(&mut p, &[
            Op::AddSource { source: Source { id: SourceId(1), path: "take.wav".into(),
                len: Ticks::from_seconds(4.0), sample_rate: 48_000, channels: 2, has_video: false } },
            Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1),
                pos: Ticks::ZERO, src_in: Ticks::ZERO, len: Ticks::from_seconds(4.0), gain: 1.0, effects: Vec::new() } },
        ]).unwrap();
        assert_eq!(p.sources.len(), 1);
        assert_eq!(p.clips.len(), 1);

        let n = h.undo_group(&mut p);
        assert_eq!(n, 2, "both the source and the clip should undo together");
        assert_eq!(p, start, "no orphaned source should remain — this is the bug, made concrete");
    }

    #[test]
    fn a_duplicate_add_source_delivery_restores_the_original_on_undo_not_the_duplicate() {
        // A subtler bug the same fix incidentally corrects: the OLD
        // "idempotent" branch returned the NEW value as its own inverse,
        // which is only a correct undo if the two values happen to be
        // byte-identical. Here they deliberately aren't (different path),
        // so undo must restore the ORIGINAL, not the duplicate.
        let mut p = proj();
        let mut h = History::default();
        h.apply(&mut p, &Op::AddSource { source: Source { id: SourceId(1), path: "original.wav".into(),
            len: Ticks::from_seconds(5.0), sample_rate: 48_000, channels: 2, has_video: false } }).unwrap();
        h.apply(&mut p, &Op::AddSource { source: Source { id: SourceId(1), path: "duplicate-delivery.wav".into(),
            len: Ticks::from_seconds(5.0), sample_rate: 48_000, channels: 2, has_video: false } }).unwrap();
        assert_eq!(p.sources[&SourceId(1)].path, "duplicate-delivery.wav");

        h.undo(&mut p);
        assert_eq!(p.sources[&SourceId(1)].path, "original.wav", "undo of the duplicate delivery must restore the ORIGINAL value, not just re-apply the duplicate");
    }

    fn bus(id: u64, name: &str, parent: Option<u64>) -> Bus {
        Bus { id: BusId(id), name: name.into(), parent: parent.map(BusId), gain: 1.0, chain: Vec::new(), sends: Vec::new(), muted: false }
    }

    #[test]
    fn every_bus_op_has_an_exact_inverse() {
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        h.apply(&mut p, &Op::AddBus { bus: bus(1, "Vocals", None) }).unwrap();
        h.apply(&mut p, &Op::AddBus { bus: bus(2, "Verb", None) }).unwrap();
        h.apply(&mut p, &Op::SetBusGain { id: BusId(1), gain: 0.5 }).unwrap();
        h.apply(&mut p, &Op::SetBusSends { id: BusId(1), sends: vec![Send { to: BusId(2), level: 0.3, pre_fader: false }] }).unwrap();
        h.apply(&mut p, &Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        h.apply(&mut p, &Op::SetBusFlags { id: BusId(1), muted: true }).unwrap();
        assert!(h.can_undo());
        while h.can_undo() { h.undo(&mut p); }
        assert_eq!(p, start, "undoing every bus op must restore the opening project exactly");
    }

    #[test]
    fn a_duplicate_add_bus_delivery_restores_the_original_on_undo_not_the_duplicate() {
        // The same check just applied to AddSource's now-fixed bug, run
        // against AddBus's own idempotent-duplicate branch — which reasoning
        // says should already be correct (BTreeMap::insert returns the
        // PREVIOUS value it displaced, which is what gets used as the
        // inverse here, unlike AddSource's old code which independently
        // re-derived the wrong value). Reasoning that a similar-looking
        // branch is fine is exactly the kind of assumption that let the
        // AddSource bug sit for this long — so this checks it directly
        // rather than trusting the re-derivation on its own.
        let mut p = proj();
        let mut h = History::default();
        h.apply(&mut p, &Op::AddBus { bus: bus(1, "Vocals", None) }).unwrap();
        let mut duplicate = bus(1, "Vocals", None);
        duplicate.gain = 0.42; // deliberately different, so a wrong inverse is detectable
        h.apply(&mut p, &Op::AddBus { bus: duplicate }).unwrap();
        assert_eq!(p.busses[&BusId(1)].gain, 0.42);

        h.undo(&mut p);
        assert_eq!(p.busses[&BusId(1)].gain, 1.0, "undo of the duplicate delivery must restore the ORIGINAL gain, not just re-apply the duplicate");
    }

    #[test]
    fn undo_group_reverts_every_op_in_the_group_together() {
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        h.apply_group(&mut p, &[
            Op::AddBus { bus: bus(1, "Vocals", None) },
            Op::SetBusGain { id: BusId(1), gain: 0.5 },
            Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) },
        ]).unwrap();
        assert_ne!(p, start, "the group should have visibly changed the project");
        assert!(h.can_undo_group());
        let n = h.undo_group(&mut p);
        assert_eq!(n, 3, "should have undone exactly the three ops in the group");
        assert_eq!(p, start, "one undo_group call should fully revert a three-op group");
    }

    #[test]
    fn a_failing_op_partway_through_a_group_rolls_back_atomically() {
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        let err = h.apply_group(&mut p, &[
            Op::AddBus { bus: bus(1, "Vocals", None) },
            Op::SetBusGain { id: BusId(1), gain: 0.5 },
            Op::SetBusGain { id: BusId(99), gain: 0.5 }, // no such bus — must fail
        ]);
        assert!(err.is_err());
        assert_eq!(p, start, "a group that fails partway through must leave the project completely unchanged");
        assert!(!h.can_undo_group(), "a failed group must not register as an undoable group");
        assert!(!h.can_undo(), "a failed group must not leave any of its partial ops on the plain undo stack either");
    }

    #[test]
    fn undo_group_is_lifo_across_multiple_groups() {
        let mut p = proj();
        let mut h = History::default();
        h.apply_group(&mut p, &[Op::AddBus { bus: bus(1, "Vocals", None) }]).unwrap();
        let after_group1 = p.clone();
        h.apply_group(&mut p, &[Op::AddBus { bus: bus(2, "Drums", None) }]).unwrap();
        assert_eq!(p.busses.len(), 2);

        h.undo_group(&mut p);
        assert_eq!(p, after_group1, "the SECOND group should undo first (LIFO), leaving the first group's bus intact");
        assert_eq!(p.busses.len(), 1);

        h.undo_group(&mut p);
        assert_eq!(p.busses.len(), 0, "the first group should now undo too");
    }

    #[test]
    fn undo_group_composes_correctly_with_a_partial_plain_undo_inside_it() {
        // A real scenario, not a hypothetical: something calls plain
        // single-op undo() a couple of times (e.g. the user pressing Ctrl-Z
        // manually) partway through what was originally applied as a group.
        // undo_group must then just finish whatever is left of that group,
        // not double-undo or panic.
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        h.apply_group(&mut p, &[
            Op::AddBus { bus: bus(1, "Vocals", None) },
            Op::AddBus { bus: bus(2, "Drums", None) },
            Op::AddBus { bus: bus(3, "808", None) },
        ]).unwrap();

        h.undo(&mut p); // manually undo just the last op (removes bus 3)
        assert_eq!(p.busses.len(), 2);

        let n = h.undo_group(&mut p); // should finish the remaining two
        assert_eq!(n, 2, "undo_group should report only what IT undid, not the group's original size");
        assert_eq!(p, start, "the group should still end up fully reverted in total");
    }

    #[test]
    fn redo_group_reapplies_a_whole_undone_group() {
        let mut p = proj();
        let mut h = History::default();
        h.apply_group(&mut p, &[
            Op::AddBus { bus: bus(1, "Vocals", None) },
            Op::AddBus { bus: bus(2, "Drums", None) },
        ]).unwrap();
        let after_apply = p.clone();
        h.undo_group(&mut p);
        assert_eq!(p.busses.len(), 0);
        assert!(h.can_redo_group());

        let n = h.redo_group(&mut p);
        assert_eq!(n, 2);
        assert_eq!(p, after_apply, "redo_group should restore the project exactly as the group left it");
    }

    #[test]
    fn plain_undo_still_works_one_step_at_a_time_regardless_of_grouping() {
        // Confirms apply_group is purely additive: ordinary single-op
        // undo/redo behave exactly as before, unaware of and unaffected by
        // group boundaries — a caller that never uses groups sees no
        // change at all.
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        h.apply(&mut p, &Op::AddBus { bus: bus(1, "Vocals", None) }).unwrap();
        h.apply_group(&mut p, &[
            Op::AddBus { bus: bus(2, "Drums", None) },
            Op::AddBus { bus: bus(3, "808", None) },
        ]).unwrap();
        assert_eq!(p.busses.len(), 3);

        h.undo(&mut p);
        assert_eq!(p.busses.len(), 2, "one plain undo should remove exactly one bus, group or not");
        h.undo(&mut p);
        assert_eq!(p.busses.len(), 1);
        h.undo(&mut p);
        assert_eq!(p, start, "three plain undos should fully revert, same total effect as grouping would have");
    }

    #[test]
    fn default_route_is_master_and_costs_nothing() {
        // A project with zero busses must still be exactly as valid as
        // before busses existed — the whole point of `route: None` meaning
        // "master" rather than requiring an explicit master Bus value.
        let p = proj();
        assert!(p.busses.is_empty());
        assert_eq!(p.tracks[0].route, None);
    }

    #[test]
    fn a_bus_parent_must_exist() {
        let mut p = proj();
        let err = p.apply(&Op::AddBus { bus: bus(1, "Vocals", Some(99)) });
        assert!(matches!(err, Err(EditError::NoSuchBus(BusId(99)))), "{err:?}");
    }

    #[test]
    fn a_bus_cannot_be_its_own_parent() {
        let mut p = proj();
        let err = p.apply(&Op::AddBus { bus: bus(1, "Loopy", Some(1)) });
        assert!(matches!(err, Err(EditError::BusCycle { id: BusId(1) })), "{err:?}");
    }

    #[test]
    fn a_send_that_would_close_a_loop_is_refused() {
        let mut p = proj();
        p.apply(&Op::AddBus { bus: bus(1, "Vocals", None) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(2, "Verb", Some(1)) }).unwrap(); // Verb -> Vocals (parent)
        // Vocals sending into Verb would close Vocals -> Verb -> Vocals.
        let err = p.apply(&Op::SetBusSends { id: BusId(1), sends: vec![Send { to: BusId(2), level: 0.2, pre_fader: false }] });
        assert!(matches!(err, Err(EditError::BusCycle { id: BusId(1) })), "{err:?}");
    }

    #[test]
    fn resetting_a_buss_own_unchanged_sends_is_never_spuriously_refused() {
        let mut p = proj();
        p.apply(&Op::AddBus { bus: bus(1, "Vocals", None) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(2, "Verb", None) }).unwrap();
        let sends = vec![Send { to: BusId(2), level: 0.4, pre_fader: false }];
        p.apply(&Op::SetBusSends { id: BusId(1), sends: sends.clone() }).unwrap();
        // Re-applying the identical sends list must not trip the cycle
        // check against itself.
        p.apply(&Op::SetBusSends { id: BusId(1), sends }).unwrap();
    }

    #[test]
    fn removing_a_bus_something_depends_on_is_refused() {
        let mut p = proj();
        p.apply(&Op::AddBus { bus: bus(1, "Drums", None) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        let err = p.apply(&Op::RemoveBus { id: BusId(1) });
        assert!(matches!(err, Err(EditError::BusInUse(BusId(1)))), "{err:?}");

        // Reroute away, then removal succeeds.
        p.apply(&Op::SetRoute { track: TrackId(1), bus: None }).unwrap();
        p.apply(&Op::RemoveBus { id: BusId(1) }).unwrap();
        assert!(p.busses.is_empty());
    }

    #[test]
    fn bus_order_processes_children_before_the_parents_they_feed() {
        let mut p = proj();
        p.apply(&Op::AddBus { bus: bus(1, "Master-ish", None) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(2, "Drums", Some(1)) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(3, "808", Some(1)) }).unwrap();
        let order = p.bus_order().unwrap();
        let pos = |id: u64| order.iter().position(|b| *b == BusId(id)).unwrap();
        assert!(pos(2) < pos(1), "Drums must be processed before the bus it feeds");
        assert!(pos(3) < pos(1), "808 must be processed before the bus it feeds");
    }

    #[test]
    fn bus_order_detects_a_cycle_even_in_data_that_bypassed_apply() {
        // A cycle can only reach live state via a corrupt/foreign file, since
        // `apply` refuses to create one — simulate that by building the
        // graph directly rather than through ops.
        let mut p = proj();
        p.busses.insert(BusId(1), Bus { id: BusId(1), name: "A".into(), parent: Some(BusId(2)), gain: 1.0, chain: vec![], sends: vec![], muted: false });
        p.busses.insert(BusId(2), Bus { id: BusId(2), name: "B".into(), parent: Some(BusId(1)), gain: 1.0, chain: vec![], sends: vec![], muted: false });
        assert!(matches!(p.bus_order(), Err(EditError::BusCycle { .. })));
    }
}

#[cfg(test)]
mod track_gain_tests {
    use super::*;

    fn track(id: u64) -> Track {
        Track { id: TrackId(id), kind: TrackKind::Audio, name: "A".into(),
                muted: false, locked: false, gain: 1.0, route: None }
    }

    #[test]
    fn the_fader_applies_and_its_inverse_restores() {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: track(1), at: 0 }).unwrap();
        let inv = p.apply(&Op::SetTrackGain { id: TrackId(1), gain: 0.5 }).unwrap();
        assert_eq!(p.tracks[0].gain, 0.5);
        p.apply(&inv).unwrap();
        assert_eq!(p.tracks[0].gain, 1.0, "the inverse op is a real undo");
    }

    #[test]
    fn a_locked_track_keeps_its_fader() {
        // SetTrackFlags stays the only edit a locked track accepts; the
        // fader is a mix edit like any clip move.
        let mut p = Project::default();
        let mut t = track(1);
        t.locked = true;
        p.apply(&Op::AddTrack { track: t, at: 0 }).unwrap();
        assert!(matches!(p.apply(&Op::SetTrackGain { id: TrackId(1), gain: 0.2 }),
                         Err(EditError::TrackLocked(_))));
        assert_eq!(p.tracks[0].gain, 1.0);
    }

    #[test]
    fn a_pre_fader_track_deserializes_at_unity_not_silence() {
        // serde(default) is a compatibility promise: a project saved before
        // faders existed must load sounding exactly as it did, and the
        // zero-default a bare f32 would give is the one wrong answer.
        let old = r#"{"id":3,"kind":"Audio","name":"A1","muted":false,"locked":false,"route":null}"#;
        let t: Track = serde_json::from_str(old).expect("pre-fader track loads");
        assert_eq!(t.gain, 1.0);
    }
}

#[cfg(test)]
mod automation_tests {
    use super::*;

    fn project() -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio,
            name: "A1".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p
    }
    fn tg() -> AutoTarget { AutoTarget::TrackGain(TrackId(1)) }

    #[test]
    fn a_point_lands_sorted_and_its_inverse_removes_it() {
        let mut p = project();
        p.apply(&Op::SetAutoPoint { target: tg(), at: Ticks(200), value: 0.5 }).unwrap();
        let inv = p.apply(&Op::SetAutoPoint { target: tg(), at: Ticks(100), value: 0.8 }).unwrap();
        assert_eq!(p.automation[&tg()], vec![(Ticks(100), 0.8), (Ticks(200), 0.5)],
                   "sorted by time regardless of edit order");
        p.apply(&inv).unwrap();
        assert_eq!(p.automation[&tg()], vec![(Ticks(200), 0.5)], "the inverse is a removal");
    }

    #[test]
    fn restating_a_points_tick_changes_its_value_and_undoes_to_the_old_one() {
        let mut p = project();
        p.apply(&Op::SetAutoPoint { target: tg(), at: Ticks(100), value: 0.8 }).unwrap();
        let inv = p.apply(&Op::SetAutoPoint { target: tg(), at: Ticks(100), value: 0.2 }).unwrap();
        assert_eq!(p.automation[&tg()], vec![(Ticks(100), 0.2)], "same tick is the same point");
        p.apply(&inv).unwrap();
        assert_eq!(p.automation[&tg()], vec![(Ticks(100), 0.8)]);
    }

    #[test]
    fn removing_the_last_point_drops_the_lane_and_undo_restores_it() {
        let mut p = project();
        p.apply(&Op::SetAutoPoint { target: tg(), at: Ticks(100), value: 0.8 }).unwrap();
        let inv = p.apply(&Op::RemoveAutoPoint { target: tg(), at: Ticks(100) }).unwrap();
        assert!(p.automation.is_empty(), "an emptied lane keeps the map sparse");
        p.apply(&inv).unwrap();
        assert_eq!(p.automation[&tg()], vec![(Ticks(100), 0.8)]);
    }

    #[test]
    fn removing_a_point_that_is_not_there_is_refused_out_loud() {
        let mut p = project();
        assert!(matches!(p.apply(&Op::RemoveAutoPoint { target: tg(), at: Ticks(9) }),
                         Err(EditError::NoSuchAutoPoint)),
                "an inverse with nothing to restore must never be minted");
    }

    #[test]
    fn a_locked_track_refuses_its_lane_like_it_refuses_its_fader() {
        let mut p = project();
        p.apply(&Op::SetTrackFlags { id: TrackId(1), muted: false, locked: true }).unwrap();
        assert!(matches!(p.apply(&Op::SetAutoPoint { target: tg(), at: Ticks(0), value: 0.5 }),
                         Err(EditError::TrackLocked(_))));
        assert!(matches!(p.apply(&Op::SetAutoPoint {
            target: AutoTarget::BusGain(BusId(7)), at: Ticks(0), value: 0.5 }),
                         Err(EditError::NoSuchBus(_))),
                "and a lane cannot be aimed at a bus that does not exist");
    }

    #[test]
    fn concurrent_lane_edits_converge_in_either_order() {
        // Two peers, two different points on one lane: absolute (time,
        // value) means both survive and both replicas agree — the same
        // property clips get from absolute positions.
        let a = Op::SetAutoPoint { target: tg(), at: Ticks(100), value: 0.2 };
        let b = Op::SetAutoPoint { target: tg(), at: Ticks(300), value: 0.9 };
        let mut ab = project();
        ab.apply(&a).unwrap(); ab.apply(&b).unwrap();
        let mut ba = project();
        ba.apply(&b).unwrap(); ba.apply(&a).unwrap();
        assert_eq!(ab, ba);
    }

    #[test]
    fn eval_is_linear_inside_held_at_the_ends_and_unity_when_empty() {
        let lane = vec![(Ticks(100), 0.0), (Ticks(300), 1.0)];
        assert_eq!(eval_lane(&lane, Ticks(0)), 0.0, "held flat before the first point");
        assert_eq!(eval_lane(&lane, Ticks(100)), 0.0);
        assert!((eval_lane(&lane, Ticks(200)) - 0.5).abs() < 1e-6, "linear between");
        assert_eq!(eval_lane(&lane, Ticks(300)), 1.0);
        assert_eq!(eval_lane(&lane, Ticks(9999)), 1.0, "held flat after the last");
        assert_eq!(eval_lane(&[], Ticks(0)), 1.0, "no lane is a transparent ride");
        // A negative value never reaches the mix as a phase flip.
        assert_eq!(eval_lane(&[(Ticks(0), -2.0)], Ticks(5)), 0.0);
    }
}

#[cfg(test)]
mod group_and_bus_input_tests {
    use super::*;

    fn project() -> Project {
        let mut p = Project::default();
        for id in [1u64, 2, 3] {
            p.apply(&Op::AddBus { bus: Bus { id: BusId(id), name: format!("B{id}"),
                parent: None, gain: 1.0, chain: vec![], sends: vec![], muted: false } }).unwrap();
        }
        p
    }

    #[test]
    fn reparenting_a_bus_is_undoable_and_a_cycle_is_refused_by_name() {
        let mut p = project();
        // Chain: 3 → 2 → 1 → master, built from the INPUT side.
        p.apply(&Op::SetBusParent { id: BusId(2), parent: Some(BusId(1)) }).unwrap();
        let inv = p.apply(&Op::SetBusParent { id: BusId(3), parent: Some(BusId(2)) }).unwrap();
        assert_eq!(p.busses[&BusId(3)].parent, Some(BusId(2)));
        // 1 ← 3 would make the chain feed itself: refused, said which bus.
        assert!(matches!(p.apply(&Op::SetBusParent { id: BusId(1), parent: Some(BusId(3)) }),
                         Err(EditError::BusCycle { id: BusId(1) })));
        assert!(matches!(p.apply(&Op::SetBusParent { id: BusId(1), parent: Some(BusId(1)) }),
                         Err(EditError::BusCycle { .. })), "self-feeding is the shortest cycle");
        p.apply(&inv).unwrap();
        assert_eq!(p.busses[&BusId(3)].parent, None, "undo puts the input back");
    }

    #[test]
    fn a_group_restates_absolutely_and_dissolving_touches_no_clip() {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio,
            name: "A".into(), muted: false, locked: false, gain: 1.0, route: None },
            at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "x".into(),
            len: Ticks::from_seconds(10.0), sample_rate: 48_000, channels: 1,
            has_video: false } }).unwrap();
        for id in [1u64, 2] {
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(1),
                track: TrackId(1), pos: Ticks::from_seconds(id as f64 * 2.0),
                src_in: Ticks::ZERO, len: Ticks::from_seconds(1.0),
                gain: 1.0, effects: vec![] } }).unwrap();
        }
        assert!(matches!(p.apply(&Op::SetGroup { id: GroupId(1), name: "hook".into(),
                                                  clips: vec![] }),
                         Err(EditError::EmptyGroup)));
        let inv = p.apply(&Op::SetGroup { id: GroupId(1), name: "hook".into(),
                                          clips: vec![ClipId(1), ClipId(2)] }).unwrap();
        assert!(matches!(inv, Op::RemoveGroup { .. }), "undo of a new group dissolves it");
        let clips_before = p.clips.clone();
        let inv2 = p.apply(&Op::RemoveGroup { id: GroupId(1) }).unwrap();
        assert_eq!(p.clips, clips_before, "ungrouping is organizational, never musical");
        p.apply(&inv2).unwrap();
        assert_eq!(p.groups[&GroupId(1)].name, "hook");
    }
}

#[cfg(test)]
mod lane_cursor_tests {
    use super::*;

    #[test]
    fn the_cursor_sweep_equals_the_search_at_every_sample_including_backwards() {
        // The optimization's only job is to be identical. A dense lane, a
        // forward sweep at sample resolution, then a jump BACKWARDS (a
        // loop wrap or a seek — the case a naive cursor gets wrong).
        let points: Vec<(Ticks, f32)> = (0..200)
            .map(|i| (Ticks::from_seconds(i as f64 * 0.05),
                      ((i as f32 * 0.37).sin() * 0.5 + 0.5)))
            .collect();
        let mut cursor = 0usize;
        for s in 0..4_000 {
            let at = Ticks::from_samples(s * 12, 48_000);
            let a = eval_lane(&points, at);
            let b = eval_lane_cursor(&points, at, &mut cursor);
            assert!((a - b).abs() < 1e-6, "sample {s}: {a} vs {b}");
        }
        // Wrap to the start: the cursor must reset rather than hold the
        // late segment.
        for s in 0..500 {
            let at = Ticks::from_samples(s * 12, 48_000);
            let a = eval_lane(&points, at);
            let b = eval_lane_cursor(&points, at, &mut cursor);
            assert!((a - b).abs() < 1e-6, "after wrap, sample {s}: {a} vs {b}");
        }
        // Edges and the empty lane behave as the search does.
        let mut c2 = 0;
        assert_eq!(eval_lane_cursor(&[], Ticks::from_seconds(1.0), &mut c2), 1.0);
        let far = Ticks::from_seconds(1000.0);
        assert_eq!(eval_lane_cursor(&points, far, &mut c2), eval_lane(&points, far));
    }
}
