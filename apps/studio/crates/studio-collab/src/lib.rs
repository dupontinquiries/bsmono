//! studio-collab — two people editing one timeline, without a server deciding
//! who wins.
//!
//! ## The problem, stated honestly
//!
//! Collaborative editing is usually hard because edits are described relative
//! to a document that the other side has already changed: "delete the 4th clip"
//! means something different once a peer inserted one. Text editors solve this
//! with operational transforms or a sequence CRDT; both are subtle.
//!
//! Studio sidesteps most of it at the model layer (see `studio-core`): clips
//! carry **stable ids**, positions are **absolute values**, and there is no
//! ordering array to reindex. What remains is genuinely small:
//!
//! 1. **Duplicate delivery** — the same op arriving twice must not apply twice.
//! 2. **Out-of-order delivery** — an op may arrive before one it logically
//!    follows.
//! 3. **True concurrency** — two peers change the same field at once, and every
//!    replica must pick the same winner without asking anyone.
//!
//! This module handles all three with a Lamport clock plus a site id, and a
//! per-field last-writer-wins rule. Convergence is asserted directly in the
//! tests: the same op set delivered in any permutation, with duplicates, must
//! produce byte-identical projects.
//!
//! ## Why last-writer-wins is the right call *here*
//!
//! LWW is a poor fit for text (it loses characters) but a good fit for a
//! timeline, because the units of edit are already whole objects with
//! independent fields. If two editors drag the same clip to different places,
//! there is no meaningful merge — one of the two intended positions must win,
//! and both peers must agree which. What LWW must NOT do is mix them into a
//! third position nobody chose, which is exactly what a delta-based protocol
//! would produce. Ops carrying absolute values is what buys that.
//!
//! Edits to *different* fields of the same clip (one peer trims, another
//! changes gain) commute and both survive, because they are separate ops.

pub mod wire;
pub mod carrier;
pub mod file;
pub mod assets;
pub mod session;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use studio_core::{ClipId, EditError, Op, Project, Ticks, TrackId};

/// Which replica an op came from. Ties in the Lamport clock break on this, so
/// it must be unique per participant and stable for the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SiteId(pub u64);

/// A globally unique, totally ordered stamp. Ordering is (lamport, site): the
/// clock captures causality where it exists, the site id breaks ties in a way
/// every replica computes identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Stamp {
    pub lamport: u64,
    pub site: SiteId,
}

/// An op with its stamp — the unit that goes over the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StampedOp {
    pub stamp: Stamp,
    pub op: Op,
}

/// What a peer is doing right now: where their playhead sits and what they have
/// selected. Presence is deliberately NOT part of the op log — it is worthless
/// a second later, must never be replayed into a project, and would otherwise
/// bloat the history that undo walks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Presence {
    pub site: SiteId,
    pub name: String,
    pub playhead: Ticks,
    pub selection: Vec<ClipId>,
    /// Which track the peer is working in, for the "someone else is here" hint
    /// that stops two editors colliding in the first place.
    pub active_track: Option<TrackId>,
    /// Monotonic per-site counter so a late presence packet cannot resurrect a
    /// stale playhead.
    pub seq: u64,
}

/// One replica of a shared project.
pub struct Doc {
    pub site: SiteId,
    pub project: Project,
    /// The project as it was when this session opened — the file on disk, before
    /// any collaborative edit. `rebuild` replays the log ON TOP of this rather
    /// than from nothing; the first version of this code rebuilt from
    /// `Project::default()` and silently erased every track and source that
    /// pre-dated the session, which the convergence tests caught immediately.
    base: Project,
    lamport: u64,
    /// Every stamp we have integrated — the duplicate filter.
    seen: BTreeSet<Stamp>,
    /// The full history in stamp order, kept so a joining peer can be brought
    /// up to date by replay, and so a late op can be integrated by rebuilding
    /// (see `integrate`).
    log: Vec<StampedOp>,
    /// Last writer per (clip, field), for the LWW rule.
    field_stamp: BTreeMap<(ClipId, Field), Stamp>,
    pub peers: BTreeMap<SiteId, Presence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Field { Placement, Trim, Gain, Effects, Existence }

fn field_of(op: &Op) -> Option<(ClipId, Field)> {
    match op {
        Op::MoveClip { id, .. } => Some((*id, Field::Placement)),
        Op::TrimClip { id, .. } => Some((*id, Field::Trim)),
        Op::SetGain { id, .. } => Some((*id, Field::Gain)),
        Op::SetEffects { id, .. } => Some((*id, Field::Effects)),
        Op::AddClip { clip } => Some((clip.id, Field::Existence)),
        Op::RemoveClip { id } => Some((*id, Field::Existence)),
        _ => None,
    }
}

impl Doc {
    pub fn new(site: SiteId, project: Project) -> Doc {
        Doc {
            site, base: project.clone(), project, lamport: 0,
            seen: BTreeSet::new(), log: Vec::new(),
            field_stamp: BTreeMap::new(), peers: BTreeMap::new(),
        }
    }

    /// Apply a local edit: stamp it, apply it, and return the packet to
    /// broadcast. A local op is applied optimistically so the UI never waits
    /// for the network — the convergence rules below are what make that safe.
    pub fn local(&mut self, op: Op) -> Result<StampedOp, EditError> {
        self.lamport += 1;
        let stamped = StampedOp { stamp: Stamp { lamport: self.lamport, site: self.site }, op };
        self.integrate(&stamped)?;
        Ok(stamped)
    }

    /// Integrate an op from anywhere (including our own). Idempotent, and
    /// order-independent: delivering the same set in any sequence converges.
    pub fn integrate(&mut self, s: &StampedOp) -> Result<(), EditError> {
        if !self.seen.insert(s.stamp) {
            return Ok(()); // duplicate — the network's problem, not the model's
        }
        self.lamport = self.lamport.max(s.stamp.lamport);

        // Last-writer-wins per field: an op that loses to a newer writer is
        // still LOGGED (so a peer joining later replays the same history) but
        // does not touch the project.
        if let Some(key) = field_of(&s.op) {
            if let Some(prev) = self.field_stamp.get(&key) {
                if *prev > s.stamp {
                    self.log.push(s.clone());
                    self.log.sort_by_key(|e| e.stamp);
                    return Ok(());
                }
            }
            self.field_stamp.insert(key, s.stamp);
        }

        self.log.push(s.clone());
        self.log.sort_by_key(|e| e.stamp);

        // Rebuild rather than patch when an op arrives out of order. Simple and
        // obviously correct; a timeline's op count is in the thousands, not the
        // millions, so the cost is irrelevant next to the risk of a subtle
        // incremental merge bug. (If that ever stops being true, the fix is a
        // snapshot every N ops — not a cleverer merge.)
        if self.log.last().map(|e| e.stamp) != Some(s.stamp) {
            self.rebuild();
        } else if self.project.apply(&s.op).is_err() {
            // A losing op (overlap, missing clip) is kept in the log but does
            // not corrupt the project: convergence only requires that every
            // replica make the SAME decision, and every replica runs this
            // identical check against the identical state.
        }
        Ok(())
    }

    fn rebuild(&mut self) {
        let mut fresh = self.base.clone();
        let mut winners: BTreeMap<(ClipId, Field), Stamp> = BTreeMap::new();
        for e in &self.log {
            if let Some(key) = field_of(&e.op) {
                let w = winners.entry(key).or_insert(e.stamp);
                if e.stamp > *w { *w = e.stamp; }
            }
        }
        for e in &self.log {
            if let Some(key) = field_of(&e.op) {
                if winners.get(&key) != Some(&e.stamp) { continue; }
            }
            let _ = fresh.apply(&e.op);
        }
        self.project = fresh;
        self.field_stamp = winners;
    }

    /// Everything a joining peer needs to catch up.
    pub fn history(&self) -> &[StampedOp] { &self.log }

    /// Merge a peer's presence packet, ignoring stale ones.
    pub fn presence(&mut self, p: Presence) {
        match self.peers.get(&p.site) {
            Some(cur) if cur.seq >= p.seq => {}
            _ => { self.peers.insert(p.site, p); }
        }
    }

    /// Peers who are working in `clip`, for the "two people on the same clip"
    /// warning that prevents most conflicts before they happen.
    ///
    /// Being inside the clip's time span is necessary but NOT sufficient: a peer
    /// scrubbing the video track is not editing the audio clip that happens to
    /// sit under the same instant. When a peer declares an `active_track` we
    /// require it to match, which is what that field is for; a peer who declares
    /// none (a viewer, or a client that does not track focus) still matches on
    /// time alone, so the warning degrades to "someone is around here" rather
    /// than disappearing.
    pub fn peers_on(&self, id: ClipId) -> Vec<&Presence> {
        let Some(c) = self.project.clips.get(&id) else { return Vec::new() };
        self.peers.values()
            .filter(|p| p.site != self.site)
            .filter(|p| p.playhead >= c.pos && p.playhead < c.end())
            .filter(|p| p.active_track.map(|t| t == c.track).unwrap_or(true))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Clip, Source, Track, TrackKind};

    fn seed() -> Project {
        let mut p = Project { name: "session".into(), ..Default::default() };
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video,
            name: "V1".into(), muted: false, locked: false }, at: 0 }).unwrap();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(2), kind: TrackKind::Audio,
            name: "A1".into(), muted: false, locked: false }, at: 1 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "take1.mov".into(),
            len: Ticks::from_seconds(60.0), sample_rate: 48_000, channels: 2, has_video: true } }).unwrap();
        p
    }
    use studio_core::SourceId;

    fn clip(id: u64, pos: f64, len: f64) -> Clip {
        Clip { id: ClipId(id), source: SourceId(1), track: TrackId(1),
               pos: Ticks::from_seconds(pos), src_in: Ticks::ZERO,
               len: Ticks::from_seconds(len), gain: 1.0, effects: Vec::new() }
    }

    #[test]
    fn duplicate_delivery_is_harmless() {
        let mut d = Doc::new(SiteId(1), seed());
        let s = d.local(Op::AddClip { clip: clip(10, 0.0, 5.0) }).unwrap();
        for _ in 0..5 { d.integrate(&s).unwrap(); }
        assert_eq!(d.project.clips.len(), 1, "the same op applied once, however often it arrives");
    }

    #[test]
    fn any_delivery_order_converges_on_the_same_project() {
        // The property that matters: three peers editing at once, their ops
        // delivered to each other in DIFFERENT orders, must end up byte-equal.
        let mut a = Doc::new(SiteId(1), seed());
        let mut b = Doc::new(SiteId(2), seed());
        let mut c = Doc::new(SiteId(3), seed());

        let o1 = a.local(Op::AddClip { clip: clip(10, 0.0, 5.0) }).unwrap();
        let o2 = b.local(Op::AddClip { clip: clip(11, 10.0, 5.0) }).unwrap();
        let o3 = c.local(Op::AddClip { clip: clip(12, 20.0, 5.0) }).unwrap();
        // everyone learns of everyone, in a different sequence each time
        for o in [&o2, &o3] { a.integrate(o).unwrap(); }
        for o in [&o3, &o1] { b.integrate(o).unwrap(); }
        for o in [&o1, &o2] { c.integrate(o).unwrap(); }

        // now concurrent edits to the SAME clip from two sites
        let m1 = a.local(Op::MoveClip { id: ClipId(11), pos: Ticks::from_seconds(30.0), track: TrackId(1) }).unwrap();
        let m2 = b.local(Op::MoveClip { id: ClipId(11), pos: Ticks::from_seconds(40.0), track: TrackId(1) }).unwrap();
        b.integrate(&m1).unwrap();
        a.integrate(&m2).unwrap();
        c.integrate(&m2).unwrap();
        c.integrate(&m1).unwrap();

        assert_eq!(a.project, b.project, "sites 1 and 2 must converge");
        assert_eq!(b.project, c.project, "sites 2 and 3 must converge");
        // and the winner is ONE of the two intended positions — never a blend
        let pos = a.project.clips[&ClipId(11)].pos;
        assert!(pos == Ticks::from_seconds(30.0) || pos == Ticks::from_seconds(40.0),
                "LWW must pick an intended position, not invent one: {pos:?}");
    }

    #[test]
    fn edits_to_different_fields_of_one_clip_both_survive() {
        // One editor trims while another rides the gain: nothing is lost, because
        // they are separate fields and therefore separate ops.
        let mut a = Doc::new(SiteId(1), seed());
        let mut b = Doc::new(SiteId(2), seed());
        let add = a.local(Op::AddClip { clip: clip(10, 0.0, 5.0) }).unwrap();
        b.integrate(&add).unwrap();

        let trim = a.local(Op::TrimClip { id: ClipId(10),
            src_in: Ticks::from_seconds(1.0), len: Ticks::from_seconds(3.0) }).unwrap();
        let gain = b.local(Op::SetGain { id: ClipId(10), gain: 0.5 }).unwrap();
        b.integrate(&trim).unwrap();
        a.integrate(&gain).unwrap();

        assert_eq!(a.project, b.project);
        let c = &a.project.clips[&ClipId(10)];
        assert_eq!(c.len, Ticks::from_seconds(3.0), "the trim survived");
        assert_eq!(c.gain, 0.5, "and so did the gain");
    }

    #[test]
    fn a_late_op_does_not_corrupt_the_timeline() {
        // Out-of-order arrival: an op stamped earlier shows up after a later one.
        let mut a = Doc::new(SiteId(1), seed());
        let mut b = Doc::new(SiteId(2), seed());
        let add = a.local(Op::AddClip { clip: clip(10, 0.0, 5.0) }).unwrap();
        b.integrate(&add).unwrap();
        let early = a.local(Op::SetGain { id: ClipId(10), gain: 0.2 }).unwrap();
        let late = a.local(Op::SetGain { id: ClipId(10), gain: 0.9 }).unwrap();
        // b hears them backwards
        b.integrate(&late).unwrap();
        b.integrate(&early).unwrap();
        assert_eq!(a.project, b.project, "backwards delivery still converges");
        assert_eq!(b.project.clips[&ClipId(10)].gain, 0.9, "the newer write wins regardless of arrival");
    }

    #[test]
    fn presence_is_not_history_and_stale_packets_are_ignored() {
        let mut d = Doc::new(SiteId(1), seed());
        d.local(Op::AddClip { clip: clip(10, 0.0, 5.0) }).unwrap();
        let before = d.history().len();
        d.presence(Presence { site: SiteId(2), name: "kes".into(),
            playhead: Ticks::from_seconds(2.0), selection: vec![ClipId(10)],
            active_track: Some(TrackId(1)), seq: 5 });
        assert_eq!(d.history().len(), before, "presence must never enter the op log");
        assert_eq!(d.peers_on(ClipId(10)).len(), 1, "a peer inside the clip is reported");
        // a stale packet cannot rewind them
        d.presence(Presence { site: SiteId(2), name: "kes".into(),
            playhead: Ticks::from_seconds(99.0), selection: vec![],
            active_track: None, seq: 4 });
        assert_eq!(d.peers[&SiteId(2)].playhead, Ticks::from_seconds(2.0), "stale presence ignored");
        // and a peer outside the clip is not reported as being on it
        d.presence(Presence { site: SiteId(2), name: "kes".into(),
            playhead: Ticks::from_seconds(99.0), selection: vec![],
            active_track: None, seq: 6 });
        assert!(d.peers_on(ClipId(10)).is_empty());
    }

    #[test]
    fn a_joining_peer_replays_history_into_the_same_project() {
        let mut a = Doc::new(SiteId(1), seed());
        for i in 0..8u64 {
            a.local(Op::AddClip { clip: clip(100 + i, i as f64 * 6.0, 5.0) }).unwrap();
        }
        a.local(Op::MoveClip { id: ClipId(103), pos: Ticks::from_seconds(200.0), track: TrackId(2) }).unwrap();
        // someone opens the session late and is handed the log
        let mut late = Doc::new(SiteId(9), seed());
        for e in a.history().to_vec() { late.integrate(&e).unwrap(); }
        assert_eq!(late.project, a.project, "replaying the log reconstructs the session exactly");
    }
}
