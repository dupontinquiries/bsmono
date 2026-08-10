//! file — a project on disk.
//!
//! An editor that cannot save is not an editor, and this is the last piece of
//! the port that is pure logic rather than hardware.
//!
//! ## The file is the ops
//!
//! kdenlive saves an MLT XML tree; Audacity saves an AUP3 SQLite database. Both
//! describe the project's *final state*, which means the file format is a second
//! model that has to be kept in step with the first one — and drifting between
//! them is where "this project opens wrong in the new version" comes from.
//!
//! Studio does not need a second model. Every edit is already a serializable
//! [`Op`](studio_core::Op), so a project file is simply **the ops that build
//! it**: add the sources, add the tracks, add the clips. Loading is replaying
//! them into an empty project — the same `apply` the editor uses, so a file can
//! never describe something the editor cannot construct.
//!
//! Three things fall out for free:
//!
//! * **The wire codec is the file codec.** One encoding to keep correct, and it
//!   is already tested against truncation and version skew.
//! * **A file is replayable into a live session.** Opening a project while
//!   collaborating is just integrating its ops — the same path a joiner takes.
//! * **History is optional.** Saving the undo log alongside the snapshot makes
//!   undo survive a restart; omitting it costs nothing and is the default,
//!   because most people do not want a week of history in every file.

use crate::wire::{Packet, WIRE_VERSION};
use crate::{Stamp, SiteId, StampedOp};
use studio_core::{Op, Project};

/// Magic so a wrong file is rejected immediately rather than misparsed into a
/// plausible-looking empty project — the failure mode that loses work quietly.
pub const MAGIC: &[u8; 8] = b"STUDIO\x00\x01";

/// What a saved file contains.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveFile {
    pub name: String,
    /// The ops that rebuild the project, in dependency order.
    pub snapshot: Vec<Op>,
    /// The edit history, when the user asked to keep it.
    pub history: Vec<StampedOp>,
}

/// The ops that rebuild `project`, in an order that always applies cleanly:
/// sources first (a clip's trim is validated against its source), then
/// busses (so a track's own `route` field — carried on the `AddTrack` op
/// below, since `route` lives on `Track` itself — points at a bus that
/// already exists by the time that track lands; a bus's `parent` must also
/// already exist, so parents are emitted before the children routed through
/// them, following the same order [`studio_core::Project::bus_order`]
/// computes for exactly this reason), then tracks (a clip needs somewhere to
/// live), then clips. Getting this order wrong is the classic "my project
/// loads with half the clips missing" bug — the loader would silently drop
/// every clip whose track had not been added yet.
pub fn snapshot(project: &Project) -> Vec<Op> {
    let mut ops = Vec::with_capacity(project.clips.len() + project.tracks.len() + project.busses.len() + 8);
    for s in project.sources.values() {
        ops.push(Op::AddSource { source: s.clone() });
    }
    // Busses in dependency order (parents before the children routed
    // through them) — `bus_order` already computes children-before-parents
    // for mixdown; reversed, that's exactly parents-before-children, the
    // order `AddBus` needs so each bus's `parent` reference already exists
    // when it lands. A cycle can't reach a live project (`apply` refuses to
    // create one), so this only fails against a project that reached memory
    // some other way — the same case `Project::bus_order`'s own docs already
    // call out, and the same answer: skip busses `bus_order` can't order
    // rather than let one bad bus corrupt the whole save.
    if let Ok(mut order) = project.bus_order() {
        order.reverse();
        for id in order {
            if let Some(bus) = project.busses.get(&id) {
                ops.push(Op::AddBus { bus: bus.clone() });
            }
        }
    }
    for (i, t) in project.tracks.iter().enumerate() {
        ops.push(Op::AddTrack { track: t.clone(), at: i });
    }
    // Locked tracks refuse clips, so a project saved with a locked track would
    // reload EMPTY. Add tracks unlocked, place the clips, then restore the
    // flags — the loader must not have to know this, so it happens here.
    //
    // `tracks_start` is where the AddTrack ops begin in `ops` — sources plus
    // however many bus ops actually got pushed above it, NOT a fixed offset.
    // This used to assume tracks always start right after sources; that broke
    // silently the moment bus ops were added above (a real instance of the
    // exact bug this comment is now warning about, caught while adding them).
    let tracks_start = ops.len() - project.tracks.len();
    let mut relock = Vec::new();
    for (i, t) in project.tracks.iter().enumerate() {
        if t.locked {
            if let Some(Op::AddTrack { track, .. }) = ops.get_mut(tracks_start + i) {
                track.locked = false;
            }
            relock.push(Op::SetTrackFlags { id: t.id, muted: t.muted, locked: true });
        }
    }
    for c in project.clips.values() {
        ops.push(Op::AddClip { clip: c.clone() });
    }
    ops.extend(relock);
    ops
}

impl SaveFile {
    pub fn from_project(project: &Project, history: Vec<StampedOp>) -> SaveFile {
        SaveFile { name: project.name.clone(), snapshot: snapshot(project), history }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(1024);
        o.extend_from_slice(MAGIC);
        o.push(WIRE_VERSION);
        let nb = self.name.as_bytes();
        o.extend_from_slice(&(nb.len() as u32).to_le_bytes());
        o.extend_from_slice(nb);
        // Both sections ride the wire codec: one encoding to keep correct.
        let snap = Packet::Catchup {
            ops: self.snapshot.iter().enumerate().map(|(i, op)| StampedOp {
                stamp: Stamp { lamport: i as u64 + 1, site: SiteId(0) }, op: op.clone(),
            }).collect(),
        }.encode();
        o.extend_from_slice(&(snap.len() as u32).to_le_bytes());
        o.extend_from_slice(&snap);
        let hist = Packet::Catchup { ops: self.history.clone() }.encode();
        o.extend_from_slice(&(hist.len() as u32).to_le_bytes());
        o.extend_from_slice(&hist);
        o
    }

    pub fn decode(b: &[u8]) -> Option<SaveFile> {
        if b.len() < 13 || &b[..8] != MAGIC { return None; }
        if b[8] != WIRE_VERSION { return None; }
        let mut i = 9;
        let take_u32 = |b: &[u8], i: &mut usize| -> Option<usize> {
            if *i + 4 > b.len() { return None; }
            let v = u32::from_le_bytes(b[*i..*i + 4].try_into().ok()?) as usize;
            *i += 4;
            Some(v)
        };
        let n = take_u32(b, &mut i)?;
        if i + n > b.len() { return None; }
        let name = String::from_utf8(b[i..i + n].to_vec()).ok()?;
        i += n;
        let sl = take_u32(b, &mut i)?;
        if i + sl > b.len() { return None; }
        let snapshot = match Packet::decode(&b[i..i + sl])? {
            Packet::Catchup { ops } => ops.into_iter().map(|s| s.op).collect(),
            _ => return None,
        };
        i += sl;
        let hl = take_u32(b, &mut i)?;
        if i + hl > b.len() { return None; }
        let history = match Packet::decode(&b[i..i + hl])? {
            Packet::Catchup { ops } => ops,
            _ => return None,
        };
        Some(SaveFile { name, snapshot, history })
    }

    /// Rebuild the project by replaying the snapshot — the same `apply` the
    /// editor uses, so a file can never describe something unconstructable.
    pub fn to_project(&self) -> Project {
        let mut p = Project { name: self.name.clone(), ..Default::default() };
        for op in &self.snapshot {
            let _ = p.apply(op);
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Doc;
    use studio_core::{Clip, ClipId, Effect, Source, SourceId, Ticks, Track, TrackId, TrackKind};

    fn built() -> Project {
        let mut p = Project { name: "the cut".into(), ..Default::default() };
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "/takes/a b.wav".into(),
            len: Ticks::from_seconds(120.0), sample_rate: 44_100, channels: 2, has_video: false } }).unwrap();
        p.apply(&Op::AddBus { bus: studio_core::Bus {
            id: studio_core::BusId(1), name: "Vocals".into(), parent: None, gain: 0.8,
            chain: vec![Effect { kind: "eq".into(), params: Default::default(), enabled: true }],
            sends: Vec::new(), muted: false,
        } }).unwrap();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video,
            name: "V1".into(), muted: false, locked: false, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(2), kind: TrackKind::Audio,
            name: "A1 — vocals".into(), muted: true, locked: false, route: Some(studio_core::BusId(1)) }, at: 1 }).unwrap();
        for i in 0..12u64 {
            let mut params = std::collections::BTreeMap::new();
            params.insert("gain".to_string(), 0.5 + i as f32 * 0.01);
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(i), source: SourceId(1),
                track: if i % 2 == 0 { TrackId(1) } else { TrackId(2) },
                pos: Ticks::from_seconds(i as f64 * 3.0), src_in: Ticks::from_seconds(0.5),
                len: Ticks::from_seconds(2.5), gain: 0.9,
                effects: vec![Effect { kind: "eq".into(), params, enabled: i % 3 == 0 }] } }).unwrap();
        }
        p
    }

    #[test]
    fn a_project_round_trips_through_a_file_exactly() {
        let p = built();
        let f = SaveFile::from_project(&p, Vec::new());
        let bytes = f.encode();
        let back = SaveFile::decode(&bytes).expect("decodes");
        assert_eq!(back, f);
        assert_eq!(back.to_project(), p, "and rebuilds the identical project");
    }

    #[test]
    fn busses_survive_a_save_and_reload() {
        // Regression test for a real bug: `snapshot` was written before
        // busses existed and never emitted `Op::AddBus` at all — a project's
        // entire bus graph (routing, sends, chains, gain) silently vanished
        // on save. `built()` now includes a bus and a track routed to it, so
        // `a_project_round_trips_through_a_file_exactly` above would also
        // have caught this on its own; this test names the failure mode
        // explicitly rather than relying on the general round-trip alone.
        let p = built();
        let reloaded = SaveFile::from_project(&p, Vec::new()).to_project();
        assert_eq!(reloaded.busses.len(), 1, "the bus itself must survive");
        let bus = reloaded.busses.get(&studio_core::BusId(1)).expect("bus 1 should exist after reload");
        assert_eq!(bus.gain, 0.8);
        assert!(!bus.chain.is_empty(), "the bus's own effect chain must survive too");
        let routed_track = reloaded.tracks.iter().find(|t| t.id == TrackId(2)).unwrap();
        assert_eq!(routed_track.route, Some(studio_core::BusId(1)), "the track's route must still point at the bus");
    }

    #[test]
    fn a_bus_with_a_parent_reloads_in_an_order_that_applies_cleanly() {
        // A child bus's AddBus must not land before its parent's — apply()
        // would refuse it (NoSuchBus) since a bus's parent must already
        // exist. This is exactly the ordering bug the file-format's own
        // docs warn about for clips/tracks, now checked for busses too.
        let mut p = built();
        p.apply(&Op::AddBus { bus: studio_core::Bus {
            id: studio_core::BusId(2), name: "Master-ish".into(), parent: None, gain: 1.0,
            chain: Vec::new(), sends: Vec::new(), muted: false,
        } }).unwrap();
        p.busses.get_mut(&studio_core::BusId(1)).unwrap().parent = Some(studio_core::BusId(2));

        let f = SaveFile::from_project(&p, Vec::new());
        // Directly confirm the ORDER within the snapshot, not just the end
        // result — replaying it through fresh `apply` calls in file order
        // must never hit NoSuchBus.
        let mut fresh = Project::default();
        for op in &f.snapshot {
            fresh.apply(op).expect("every op in the snapshot must apply cleanly in the order it was written, including a child bus never preceding its parent");
        }
        assert_eq!(fresh.busses.len(), 2);
    }

    #[test]
    fn a_locked_track_does_not_reload_empty() {
        // The trap: locked tracks refuse clips, so a naive snapshot that adds
        // the track locked and then adds its clips loses every one of them.
        let mut p = built();
        p.apply(&Op::SetTrackFlags { id: TrackId(1), muted: false, locked: true }).unwrap();
        let clips_before = p.clips.len();
        let reloaded = SaveFile::from_project(&p, Vec::new()).to_project();
        assert_eq!(reloaded.clips.len(), clips_before, "clips survive a locked track");
        assert!(reloaded.tracks.iter().find(|t| t.id == TrackId(1)).unwrap().locked,
                "and the track is still locked afterwards");
        assert_eq!(reloaded, p);
    }

    #[test]
    fn a_locked_track_still_reloads_correctly_alongside_a_bus() {
        // The exact interaction that broke: the locked-track relock fix
        // indexes into the snapshot assuming a fixed offset from the start
        // of the track ops. Adding bus ops before the tracks shifted that
        // offset — this pins the fix, not just the presence of busses.
        let mut p = built(); // built() now always includes a bus
        p.apply(&Op::SetTrackFlags { id: TrackId(2), muted: true, locked: true }).unwrap();
        let clips_before = p.clips.len();
        let reloaded = SaveFile::from_project(&p, Vec::new()).to_project();
        assert_eq!(reloaded.clips.len(), clips_before, "clips must survive a locked track even with a bus present in the snapshot");
        assert!(reloaded.tracks.iter().find(|t| t.id == TrackId(2)).unwrap().locked);
        assert_eq!(reloaded.busses.len(), 1, "the bus itself must also still be there");
        assert_eq!(reloaded, p);
    }

    #[test]
    fn the_snapshot_orders_sources_and_tracks_before_clips() {
        // A clip validates against its source and needs its track to exist.
        // Wrong order silently drops clips — the classic "half my project is
        // missing" bug.
        let ops = snapshot(&built());
        let first_clip = ops.iter().position(|o| matches!(o, Op::AddClip { .. })).unwrap();
        let last_src = ops.iter().rposition(|o| matches!(o, Op::AddSource { .. })).unwrap();
        let last_track = ops.iter().rposition(|o| matches!(o, Op::AddTrack { .. })).unwrap();
        assert!(last_src < first_clip, "sources before clips");
        assert!(last_track < first_clip, "tracks before clips");
    }

    #[test]
    fn a_wrong_or_damaged_file_is_refused_rather_than_half_read() {
        // Quietly loading an empty project over someone's work is the failure
        // that loses it.
        assert!(SaveFile::decode(b"").is_none());
        assert!(SaveFile::decode(b"not a studio file at all").is_none());
        let good = SaveFile::from_project(&built(), Vec::new()).encode();
        for cut in 0..good.len() {
            assert!(SaveFile::decode(&good[..cut]).is_none(), "a {cut}-byte prefix must not load");
        }
        let mut wrong_version = good.clone();
        wrong_version[8] = WIRE_VERSION + 1;
        assert!(SaveFile::decode(&wrong_version).is_none(), "a newer file is refused, not guessed at");
        let mut wrong_magic = good.clone();
        wrong_magic[2] = b'X';
        assert!(SaveFile::decode(&wrong_magic).is_none());
    }

    #[test]
    fn history_is_optional_and_survives_when_kept() {
        let p = built();
        let hist = vec![StampedOp { stamp: Stamp { lamport: 9, site: SiteId(4) },
            op: Op::SetGain { id: ClipId(2), gain: 0.1 } }];
        let with = SaveFile::from_project(&p, hist.clone());
        assert_eq!(SaveFile::decode(&with.encode()).unwrap().history, hist);
        let without = SaveFile::from_project(&p, Vec::new());
        assert!(SaveFile::decode(&without.encode()).unwrap().history.is_empty());
        // history does not alter the project the file rebuilds
        assert_eq!(with.to_project(), without.to_project());
    }

    #[test]
    fn a_saved_file_can_be_opened_straight_into_a_live_session() {
        // Because the file IS ops, opening a project while collaborating is the
        // same path a joiner takes — no separate import.
        let f = SaveFile::from_project(&built(), Vec::new());
        let mut doc = Doc::new(SiteId(2), Project { name: f.name.clone(), ..Default::default() });
        for op in &f.snapshot {
            doc.local(op.clone()).ok();
        }
        assert_eq!(doc.project, f.to_project(), "the session holds what the file held");
        // and the peer catching up gets it by the ordinary mechanism
        let mut peer = Doc::new(SiteId(3), Project { name: f.name.clone(), ..Default::default() });
        let answer = doc.receive(peer.hello()).unwrap();
        peer.receive(answer);
        assert_eq!(peer.project, doc.project);
    }
}
