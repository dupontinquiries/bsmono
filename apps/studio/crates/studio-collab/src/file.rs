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

use crate::wire::{Packet, FIRST_FILE_VERSION, WIRE_VERSION};
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
    /// The rack — the beat half of a project, absent only in files from
    /// before the rack was saved (or projects that never touched it).
    pub rack: Option<studio_live::Rack>,
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
    for (id, pan) in &project.bus_pans {
        ops.push(Op::SetBusPan { id: *id, pan: *pan });
    }
    // Clip fades. Never emitted until c20: every fade was lost on save and
    // reopen, and on a peer's join (which catches up from this snapshot) --
    // found when an FL import's 3 ms declick fades never reached a render made
    // from the saved file. After the clips, which they name.
    for (id, (fi, fo)) in &project.fades {
        ops.push(Op::SetClipFade { id: *id, fade_in: *fi, fade_out: *fo });
    }
    // Automation lanes, as the ops that place their points — before the
    // relock, because a locked track refuses its lane the way it refuses
    // its fader, and a save that silently dropped a locked track's ride
    // would be the same class of bug as the one relock itself fixes.
    for (target, lane) in &project.automation {
        for (at, value) in lane {
            ops.push(Op::SetAutoPoint { target: *target, at: *at, value: *value });
        }
    }
    // Curves, then the bindings that aim them — curves first because a
    // binding validates against its curve, bindings after clips because
    // a clip gate validates against its clip, and all of it BEFORE the
    // relock, for the lanes' reason: a locked track refuses a binding on
    // its gain. A binding gated to a clip that no longer exists is left
    // out (SetBinding would refuse it); it was inert in memory anyway.
    for (id, c) in &project.curves {
        ops.push(Op::SetCurve { id: *id, name: c.name.clone() });
        for (at, value) in &c.points {
            ops.push(Op::SetCurvePoint { curve: *id, at: *at, value: *value });
        }
    }
    for (id, b) in &project.bindings {
        if let studio_core::Gate::Clip(c) = b.gate {
            if !project.clips.contains_key(&c) { continue; }
        }
        ops.push(Op::SetBinding { id: *id, binding: *b });
    }
    // Compositions rebuild after their clips exist; a group whose clips all
    // dangled (every member deleted) has an unappliable op, so it is
    // filtered to the members that still exist — and dropped entirely when
    // none do, the same fate the timeline already gives it visually.
    if project.key.is_some() {
        ops.push(Op::SetKey { key: project.key });
    }
    for m in &project.markers {
        ops.push(Op::SetMarker { at: m.at, name: m.name.clone() });
    }
    for (id, g) in &project.groups {
        let clips: Vec<_> = g.clips.iter().copied()
            .filter(|c| project.clips.contains_key(c)).collect();
        if !clips.is_empty() {
            ops.push(Op::SetGroup { id: *id, name: g.name.clone(), clips });
            // SetGroup does not carry the key (see its own doc comment —
            // key has its own last-write-wins slot), so a group's key
            // needs its own op here too, or replaying this reconstruction
            // silently drops every song's key.
            if g.key.is_some() {
                ops.push(Op::SetGroupKey { id: *id, key: g.key });
            }
            if g.tempo.is_some() {
                ops.push(Op::SetGroupTempo { id: *id, tempo: g.tempo.clone() });
            }
            // Same reasoning again for the take bucket's chosen take: it is
            // its own slot, not part of SetGroup, so without this a saved
            // project came back with every bucket's choice forgotten —
            // found by saving an organised camera card and loading it.
            if g.active.is_some() {
                ops.push(Op::SetGroupActive { id: *id, active: g.active });
            }
        }
    }
    ops.extend(relock);
    ops
}

impl SaveFile {
    pub fn from_project(
        project: &Project,
        history: Vec<StampedOp>,
        rack: Option<studio_live::Rack>,
    ) -> SaveFile {
        SaveFile { name: project.name.clone(), snapshot: snapshot(project), history, rack }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(1024);
        o.extend_from_slice(MAGIC);
        o.push(crate::wire::current_encode_version());
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
        // Fourth section, the rack — length 0 when there is none. Same wire
        // codec as everything else (Packet::Rack with a zeroed sync stamp:
        // a FILE has no author sequence to claim, and the loader treats what
        // it reads as local truth, not as an LWW contender).
        match &self.rack {
            Some(r) => {
                let rk = Packet::Rack(crate::RackSync {
                    rack: r.clone(), seq: 0, site: SiteId(0),
                }).encode();
                o.extend_from_slice(&(rk.len() as u32).to_le_bytes());
                o.extend_from_slice(&rk);
            }
            None => o.extend_from_slice(&0u32.to_le_bytes()),
        }
        o
    }

    pub fn decode(b: &[u8]) -> Option<SaveFile> {
        if b.len() < 13 || &b[..8] != MAGIC { return None; }
        // Any version this build knows how to read. Refusing everything but
        // the current one meant each format bump orphaned every older project.
        if !(FIRST_FILE_VERSION..=WIRE_VERSION).contains(&b[8]) { return None; }
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
        let snapshot = match Packet::decode_stored(&b[i..i + sl])? {
            Packet::Catchup { ops } => ops.into_iter().map(|s| s.op).collect(),
            _ => return None,
        };
        i += sl;
        let hl = take_u32(b, &mut i)?;
        if i + hl > b.len() { return None; }
        let history = match Packet::decode_stored(&b[i..i + hl])? {
            Packet::Catchup { ops } => ops,
            _ => return None,
        };
        i += hl;
        // The rack section is MANDATORY (length 0 = "no rack"), not a
        // tolerated absence: the header's version gate means no legitimate
        // file of this version can end after three sections, so a missing
        // tail is truncation — and a truncated file that quietly loaded
        // rack-less would, on the next save, overwrite a complete original
        // with the beat gone. The first draft here was tolerant; the
        // damaged-file test's every-prefix-refuses sweep caught the hole.
        let rl = take_u32(b, &mut i)?;
        let rack = if rl == 0 {
            None
        } else {
            if i + rl > b.len() { return None; }
            match Packet::decode_stored(&b[i..i + rl])? {
                Packet::Rack(rs) => Some(rs.rack),
                _ => return None,
            }
        };
        Some(SaveFile { name, snapshot, history, rack })
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
            len: Ticks::from_seconds(120.0), sample_rate: 44_100, channels: 2, has_video: false, kind: studio_core::SourceKind::File } }).unwrap();
        p.apply(&Op::AddBus { bus: studio_core::Bus {
            id: studio_core::BusId(1), name: "Vocals".into(), parent: None, gain: 0.8,
            chain: vec![Effect { kind: "eq".into(), params: Default::default(), enabled: true }],
            sends: Vec::new(), muted: false, silent: false,
        } }).unwrap();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video,
            name: "V1".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(2), kind: TrackKind::Audio,
            name: "A1 — vocals".into(), muted: true, locked: false, gain: 1.0, route: Some(studio_core::BusId(1)) }, at: 1 }).unwrap();
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
        let f = SaveFile::from_project(&p, Vec::new(), None);
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
        let reloaded = SaveFile::from_project(&p, Vec::new(), None).to_project();
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
            chain: Vec::new(), sends: Vec::new(), muted: false, silent: false,
        } }).unwrap();
        p.busses.get_mut(&studio_core::BusId(1)).unwrap().parent = Some(studio_core::BusId(2));

        let f = SaveFile::from_project(&p, Vec::new(), None);
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
        let reloaded = SaveFile::from_project(&p, Vec::new(), None).to_project();
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
        let reloaded = SaveFile::from_project(&p, Vec::new(), None).to_project();
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
        let good = SaveFile::from_project(&built(), Vec::new(), None).encode();
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
        let with = SaveFile::from_project(&p, hist.clone(), None);
        assert_eq!(SaveFile::decode(&with.encode()).unwrap().history, hist);
        let without = SaveFile::from_project(&p, Vec::new(), None);
        assert!(SaveFile::decode(&without.encode()).unwrap().history.is_empty());
        // history does not alter the project the file rebuilds
        assert_eq!(with.to_project(), without.to_project());
    }

    #[test]
    fn a_saved_file_can_be_opened_straight_into_a_live_session() {
        // Because the file IS ops, opening a project while collaborating is the
        // same path a joiner takes — no separate import.
        let f = SaveFile::from_project(&built(), Vec::new(), None);
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

#[cfg(test)]
mod rack_section_tests {
    use super::*;

    #[test]
    fn a_saved_rack_comes_back_and_a_rackless_file_still_loads() {
        let p = Project { name: "beat".into(), ..Default::default() };
        let rack = studio_live::Rack::trap_kit();
        let with = SaveFile::from_project(&p, Vec::new(), Some(rack.clone()));
        let back = SaveFile::decode(&with.encode()).expect("decodes");
        assert_eq!(back.rack.as_ref(), Some(&rack), "the beat survives the file");

        let without = SaveFile::from_project(&p, Vec::new(), None);
        let back = SaveFile::decode(&without.encode()).expect("decodes");
        assert_eq!(back.rack, None);
    }

    #[test]
    fn a_file_cut_at_the_rack_boundary_is_damage_and_refuses() {
        // The tempting "tolerate a missing tail for old files" reading is a
        // truncation hole: no legitimate file of this version has three
        // sections, so a clean cut at the boundary must refuse like any
        // other prefix does.
        let p = Project { name: "beat".into(), ..Default::default() };
        let bytes = SaveFile::from_project(&p, Vec::new(), Some(studio_live::Rack::trap_kit()))
            .encode();
        assert!(SaveFile::decode(&bytes[..bytes.len() - 4]).is_none());
    }
}

#[cfg(test)]
mod automation_file_tests {
    use super::*;
    use studio_core::{AutoTarget, Project, Ticks, Track, TrackId, TrackKind};

    #[test]
    fn a_saved_projects_lanes_survive_the_round_trip() {
        // A file is the ops that rebuild the project; a lane that snapshot()
        // forgot would silently vanish on every save — the exact bug class
        // the locked-track relock already guards one field over. Locked
        // track included ON PURPOSE: its lane must land before the relock.
        let mut p = Project { name: "ride".into(), ..Default::default() };
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio,
            name: "vox".into(), muted: false, locked: false, gain: 0.8, route: None }, at: 0 }).unwrap();
        let t = AutoTarget::TrackGain(TrackId(1));
        p.apply(&Op::SetAutoPoint { target: t, at: Ticks(100), value: 0.2 }).unwrap();
        p.apply(&Op::SetAutoPoint { target: t, at: Ticks(900), value: 1.0 }).unwrap();
        p.apply(&Op::SetTrackFlags { id: TrackId(1), muted: false, locked: true }).unwrap();

        let file = SaveFile::from_project(&p, Vec::new(), None);
        let back = SaveFile::decode(&file.encode()).expect("decodes");
        let mut rebuilt = Project { name: back.name.clone(), ..Default::default() };
        for op in &back.snapshot {
            rebuilt.apply(op).expect("every snapshot op applies cleanly");
        }
        assert_eq!(rebuilt, p, "lanes, lock state and all");
    }

    #[test]
    fn a_take_buckets_chosen_take_survives_the_round_trip() {
        // Found by saving an organised camera card: every bucket came back
        // with no take chosen, because `active` is its own slot and
        // `SetGroup` does not carry it — the same trap key and tempo were
        // already fixed for.
        use studio_core::{Clip, ClipId, GroupId, Source, SourceId};
        let mut p = Project { name: "card".into(), ..Default::default() };
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video,
            name: "V1".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "C0057.mkv".into(),
            len: Ticks::from_seconds(60.0), sample_rate: 48_000, channels: 2, has_video: true,
            kind: studio_core::SourceKind::File } }).unwrap();
        for id in 1..=2u64 {
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(1), track: TrackId(1),
                pos: Ticks::from_seconds(id as f64 * 10.0), src_in: Ticks::ZERO,
                len: Ticks::from_seconds(5.0), gain: 1.0, effects: vec![] } }).unwrap();
        }
        p.apply(&Op::SetGroup { id: GroupId(1), name: "1B".into(),
                                clips: vec![ClipId(1), ClipId(2)] }).unwrap();
        p.apply(&Op::SetGroupActive { id: GroupId(1), active: Some(ClipId(2)) }).unwrap();

        let file = SaveFile::from_project(&p, Vec::new(), None);
        let back = SaveFile::decode(&file.encode()).expect("decodes");
        let mut rebuilt = Project { name: back.name.clone(), ..Default::default() };
        for op in &back.snapshot { rebuilt.apply(op).expect("applies"); }
        assert_eq!(rebuilt.groups[&GroupId(1)].active, Some(ClipId(2)), "the chosen take came back");
        assert_eq!(rebuilt, p);
    }

    #[test]
    fn curves_and_bindings_survive_the_round_trip_and_a_dangling_gate_is_dropped() {
        use studio_core::{Binding, BindingId, Clip, ClipId, CurveId, Gate, Source, SourceId};
        let mut p = Project { name: "bound".into(), ..Default::default() };
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio,
            name: "vox".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "a.wav".into(),
            len: Ticks(10_000), sample_rate: 48_000, channels: 1, has_video: false, kind: studio_core::SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1),
            pos: Ticks(100), src_in: Ticks::ZERO, len: Ticks(500), gain: 1.0, effects: vec![] } }).unwrap();
        p.apply(&Op::SetCurve { id: CurveId(1), name: "dip".into() }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks(0), value: 1.0 }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks(250), value: 0.2 }).unwrap();
        let t = AutoTarget::TrackGain(TrackId(1));
        p.apply(&Op::SetBinding { id: BindingId(1), binding: Binding {
            curve: CurveId(1), target: t, gate: Gate::Clip(ClipId(1)), response: None, fit_span: false } }).unwrap();
        p.apply(&Op::SetBinding { id: BindingId(2), binding: Binding {
            curve: CurveId(1), target: t, gate: Gate::Window { start: Ticks(1_000), end: Ticks(2_000) }, response: None, fit_span: false } }).unwrap();
        // Locked on purpose, the lane test's reason: the bindings must land
        // before the relock or a locked track's ride vanishes on save.
        p.apply(&Op::SetTrackFlags { id: TrackId(1), muted: false, locked: true }).unwrap();

        let file = SaveFile::from_project(&p, Vec::new(), None);
        let back = SaveFile::decode(&file.encode()).expect("decodes");
        let mut rebuilt = Project { name: back.name.clone(), ..Default::default() };
        for op in &back.snapshot {
            rebuilt.apply(op).expect("every snapshot op applies cleanly");
        }
        assert_eq!(rebuilt, p, "curves, bindings, lock state and all");
        assert!((rebuilt.auto_value(t, Ticks(350)) - 0.2).abs() < 1e-6, "and the ride still reads");

        // A binding whose clip is gone dangles in memory and is dropped from
        // the file rather than saved as an op the loader would refuse.
        p.apply(&Op::SetTrackFlags { id: TrackId(1), muted: false, locked: false }).unwrap();
        p.apply(&Op::RemoveClip { id: ClipId(1) }).unwrap();
        let snap = snapshot(&p);
        let bindings_saved = snap.iter().filter(|o| matches!(o, Op::SetBinding { .. })).count();
        assert_eq!(bindings_saved, 1, "the window binding survives, the dangling clip gate does not");
        let mut again = Project { name: p.name.clone(), ..Default::default() };
        for op in &snap { again.apply(op).expect("no refused op in a snapshot"); }
    }

    #[test]
    fn projects_saved_by_older_versions_still_open() {
        // Until this test existed, SaveFile::decode refused any version but
        // the current one, so every format bump -- three this session --
        // orphaned every project saved before it. Real old bytes, written by
        // the encoder at the old version, not a hand-built imitation.
        use studio_core::{Bus, BusId, CurveId, Binding, BindingId, Gate, AutoTarget};
        let mut p = Project { name: "old".into(), ..Default::default() };
        p.apply(&Op::AddBus { bus: Bus { id: BusId(1), name: "Music".into(), parent: None, gain: 0.8,
            chain: vec![], sends: vec![], muted: false, silent: false } }).unwrap();
        p.apply(&Op::SetCurve { id: CurveId(1), name: "stage".into() }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks::ZERO, value: 0.0 }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks::from_seconds(4.0), value: 1.0 }).unwrap();
        p.apply(&Op::SetBinding { id: BindingId(1), binding: Binding { curve: CurveId(1),
            target: AutoTarget::BusGain(BusId(1)), gate: Gate::Always, response: None, fit_span: false } }).unwrap();
        for v in crate::wire::FIRST_FILE_VERSION..=crate::wire::WIRE_VERSION {
            crate::wire::ENCODE_AS.with(|c| c.set(v));
            let bytes = SaveFile::from_project(&p, Vec::new(), None).encode();
            crate::wire::ENCODE_AS.with(|c| c.set(crate::wire::WIRE_VERSION));
            assert_eq!(bytes[8], v, "written at v{v}");
            let back = SaveFile::decode(&bytes).unwrap_or_else(|| panic!("a v{v} file must open"));
            assert_eq!(back.to_project(), p, "v{v} opens to the same project");
        }
        // a file from a NEWER build is still refused, not guessed at
        let mut future = SaveFile::from_project(&p, Vec::new(), None).encode();
        future[8] = crate::wire::WIRE_VERSION + 1;
        assert!(SaveFile::decode(&future).is_none());
        // and live packets stay exact: an old peer's packet is refused
        crate::wire::ENCODE_AS.with(|c| c.set(crate::wire::WIRE_VERSION - 1));
        let old_packet = crate::wire::Packet::Catchup { ops: vec![] }.encode();
        crate::wire::ENCODE_AS.with(|c| c.set(crate::wire::WIRE_VERSION));
        assert!(crate::wire::Packet::decode(&old_packet).is_none(), "peers must match exactly");
        assert!(crate::wire::Packet::decode_stored(&old_packet).is_some(), "a stored one opens");
    }
}

#[cfg(test)]
mod source_kind_tests {
    use super::*;
    use studio_core::{Op, Project, Source, SourceId, Ticks};

    fn with_pattern_source() -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddSource { source: Source::pattern(SourceId(7), 3, "Beat (2/2)", Ticks::ZERO, 48_000, 2) }).unwrap();
        p.apply(&Op::AddSource { source: Source::file(SourceId(8), "vox.opus", Ticks::from_seconds(2.0), 48_000, 1, false) }).unwrap();
        p
    }

    /// A pattern source survives a save: it came back as a FILE with an empty
    /// path before v20, and a reopened project called it missing.
    #[test]
    fn a_pattern_source_is_still_a_pattern_after_a_save() {
        let p = with_pattern_source();
        let back = SaveFile::decode(&SaveFile::from_project(&p, Vec::new(), None).encode()).unwrap().to_project();
        assert_eq!(back.sources[&SourceId(7)].kind, studio_core::SourceKind::Pattern { pattern: 3, name: "Beat (2/2)".into() });
        assert!(back.sources[&SourceId(7)].is_generator());
        assert_eq!(back.sources[&SourceId(8)], p.sources[&SourceId(8)]);
    }

    /// A v19 file still opens (its sources as files, which is all v19 knew).
    #[test]
    fn a_v19_file_still_opens() {
        let p = with_pattern_source();
        crate::wire::ENCODE_AS.with(|c| c.set(19));
        let bytes = SaveFile::from_project(&p, Vec::new(), None).encode();
        crate::wire::ENCODE_AS.with(|c| c.set(crate::wire::WIRE_VERSION));
        let back = SaveFile::decode(&bytes).expect("a v19 file opens").to_project();
        assert_eq!(back.sources.len(), 2);
        assert_eq!(back.sources[&SourceId(8)].path, "vox.opus");
    }
}

#[cfg(test)]
mod fade_save_tests {
    use super::*;
    use studio_core::{Clip, ClipId, Op, Project, Source, SourceId, Ticks, Track, TrackId, TrackKind};

    #[test]
    fn clip_fades_survive_a_save() {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio, name: "a".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source::file(SourceId(1), "a.opus", Ticks::from_seconds(5.0), 48_000, 1, false) }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1), pos: Ticks::ZERO, src_in: Ticks::ZERO,
            len: Ticks::from_seconds(4.0), gain: 1.0, effects: vec![] } }).unwrap();
        p.apply(&Op::SetClipFade { id: ClipId(1), fade_in: Ticks::from_seconds(0.003), fade_out: Ticks::from_seconds(1.5) }).unwrap();
        p.apply(&Op::AddBus { bus: studio_core::Bus { id: studio_core::BusId(3), name: "b".into(), parent: None, gain: 1.0,
            chain: vec![], sends: vec![], muted: false, silent: false } }).unwrap();
        p.apply(&Op::SetBusPan { id: studio_core::BusId(3), pan: -0.25 }).unwrap();
        let back = SaveFile::decode(&SaveFile::from_project(&p, Vec::new(), None).encode()).unwrap().to_project();
        assert_eq!(back.fades, p.fades, "the fades came back");
        assert_eq!(back.bus_pans, p.bus_pans, "and the bus pan");
    }
}
