//! wire — what actually crosses the network.
//!
//! `Doc` converges given the ops; this is how a peer gets them. The transport
//! underneath is deliberately unspecified — the monorepo's `snapclone-dht`
//! already carries signed records between peers and is the intended carrier,
//! but a socket, a file, or a websocket work identically because a packet is
//! just bytes.
//!
//! ## Why a hand-rolled codec
//!
//! An op is small, fixed-shape and enormously frequent: every nudge of a clip
//! is one. A general format would spend more bytes on field names than on the
//! edit. The encoding below is a few hundred lines of `put_*`/`get_*` with a
//! version byte, and it buys three things that matter more than convenience:
//!
//! * **A version byte**, so a peer running an older build is told to update
//!   rather than silently misreading a struct that gained a field.
//! * **Explicit framing**, so a stream carrying packets back-to-back can find
//!   the boundaries without a delimiter that could appear in the payload.
//! * **Rejecting truncation loudly.** Every `get_*` is checked; a short read
//!   returns `None` instead of a half-decoded op. An edit applied from a
//!   corrupt packet would diverge one replica silently, which is the one
//!   failure this whole design exists to prevent.
//!
//! ## Joining a session
//!
//! A newcomer sends [`Packet::Hello`] with the highest Lamport clock it has
//! seen (0 when opening cold). The peer replies with [`Packet::Catchup`]
//! carrying every op stamped after that — which, because `Doc::integrate` is
//! idempotent and order-independent, needs no coordination at all: the joiner
//! can be receiving live edits while the catch-up is still arriving.

use crate::{Presence, RackSync, SiteId, Stamp, StampedOp};
use studio_core::{Bus, BusId, Clip, ClipId, Effect, Op, Send, Source, SourceId, Ticks, Track, TrackId, TrackKind};
use studio_live::{Channel, Note, Pattern, Rack, StepMod, SynthKind};

/// Bumped whenever the encoding changes shape. A peer that sees a version it
/// does not know refuses the packet rather than guessing.
///
/// v9: `Pattern` gained `rolls` — the piano roll's held notes, per channel.
/// A rack shape change, so it moves both `Packet::Rack` and the file
/// format's mandatory rack section (which shares this codec).
///
/// v2: `Track` gained `route: Option<BusId>`, and `Op` gained the bus-graph
/// variants (`AddBus`/`RemoveBus`/`SetBusGain`/`SetBusChain`/`SetBusSends`/
/// `SetRoute`).
///
/// v8: `Packet` gained the asset quartet — `AssetRequest`(6) /
/// `AssetManifest`(7) / `ChunkRequest`(8) / `Chunk`(9) — so a source's
/// media can follow its `AddSource` op, content-addressed through
/// `assets::Manifest`.
///
/// v7: `Track` gained `gain: f32` (the mixer fader) and `Op` gained
/// `SetTrackGain` (tag 19).
///
/// v3: `Bus` gained `muted: bool`, and `Op` gained `SetBusFlags`. A separate
/// bump from v2 rather than folding it in, because v2 had already landed and
/// been tested as its own shape before mute was designed — the "one bump per
/// shape change" rule is about not bundling *unrelated* future changes into
/// one guess, not about never bumping twice in the same session.
///
/// v4: `Op` gained `RemoveSource` — closing a real undo-correctness bug
/// where `AddSource`'s own "inverse" used to be itself, meaning a source
/// could never actually be undone or removed. `Packet::Rack` (whole-value
/// rack sync) was added in the same working session but deliberately does
/// NOT bump this constant: it isn't an `Op`/`Track`/`Bus` shape at all, and
/// `Packet::decode`'s kind-byte dispatch already refuses an unrecognised
/// packet kind on its own — see the version-refusal tests below for both
/// mechanisms exercised separately.
pub const WIRE_VERSION: u8 = 9;

#[derive(Debug, Clone, PartialEq)]
pub enum Packet {
    /// One edit.
    Op(StampedOp),
    /// Where a peer is looking. Never enters the op log.
    Presence(Presence),
    /// "I have everything up to `since`; send me the rest."
    Hello { site: SiteId, since: u64 },
    /// The answer to a Hello.
    Catchup { ops: Vec<StampedOp> },
    /// A rack sync — whole-value LWW, not an op. See [`RackSync`]'s docs.
    Rack(RackSync),
    /// "Send me the manifest for this source's media." Sent by a peer whose
    /// project references a source it has no bytes for — an `AddSource` op
    /// travels light, and the media follows content-addressed.
    AssetRequest { source: studio_core::SourceId },
    /// The asset's shape: chunk ids only, so a receiver asks for exactly
    /// what its content-addressed store lacks (a re-bounce that changed one
    /// bar re-ships one bar).
    AssetManifest { source: studio_core::SourceId, manifest: crate::assets::Manifest },
    /// "Send these chunks" — first-appearance order, straight from
    /// `Manifest::need`.
    ChunkRequest { ids: Vec<crate::assets::ChunkId> },
    /// One content-addressed chunk. The id IS the integrity check: a
    /// receiver re-hashes and drops a mismatch on the floor.
    Chunk { id: crate::assets::ChunkId, bytes: Vec<u8> },
}

// ── primitives ──────────────────────────────────────────────────────────────

fn put_u8(o: &mut Vec<u8>, v: u8) { o.push(v); }
fn put_u16(o: &mut Vec<u8>, v: u16) { o.extend_from_slice(&v.to_le_bytes()); }
fn put_u32(o: &mut Vec<u8>, v: u32) { o.extend_from_slice(&v.to_le_bytes()); }
fn put_u64(o: &mut Vec<u8>, v: u64) { o.extend_from_slice(&v.to_le_bytes()); }
fn put_i64(o: &mut Vec<u8>, v: i64) { o.extend_from_slice(&v.to_le_bytes()); }
fn put_f32(o: &mut Vec<u8>, v: f32) { o.extend_from_slice(&v.to_le_bytes()); }
fn put_str(o: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    put_u16(o, b.len().min(u16::MAX as usize) as u16);
    o.extend_from_slice(&b[..b.len().min(u16::MAX as usize)]);
}
fn put_ticks(o: &mut Vec<u8>, t: Ticks) { put_i64(o, t.0); }

struct R<'a> { b: &'a [u8], i: usize }
impl<'a> R<'a> {
    fn new(b: &'a [u8]) -> Self { R { b, i: 0 } }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.i + n > self.b.len() { return None; }
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> { Some(self.take(1)?[0]) }
    fn u16(&mut self) -> Option<u16> { Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?)) }
    fn u32(&mut self) -> Option<u32> { Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?)) }
    fn u64(&mut self) -> Option<u64> { Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?)) }
    fn i64(&mut self) -> Option<i64> { Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?)) }
    fn f32(&mut self) -> Option<f32> { Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?)) }
    fn str(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn ticks(&mut self) -> Option<Ticks> { Some(Ticks(self.i64()?)) }
}

// ── model pieces ────────────────────────────────────────────────────────────

fn put_clip(o: &mut Vec<u8>, c: &Clip) {
    put_u64(o, c.id.0); put_u64(o, c.source.0); put_u64(o, c.track.0);
    put_ticks(o, c.pos); put_ticks(o, c.src_in); put_ticks(o, c.len);
    put_f32(o, c.gain);
    put_u16(o, c.effects.len().min(u16::MAX as usize) as u16);
    for e in &c.effects {
        put_str(o, &e.kind);
        put_u8(o, e.enabled as u8);
        put_u16(o, e.params.len().min(u16::MAX as usize) as u16);
        for (k, v) in &e.params { put_str(o, k); put_f32(o, *v); }
    }
}

fn get_clip(r: &mut R) -> Option<Clip> {
    let c = Clip {
        id: ClipId(r.u64()?), source: SourceId(r.u64()?), track: TrackId(r.u64()?),
        pos: r.ticks()?, src_in: r.ticks()?, len: r.ticks()?, gain: r.f32()?,
        effects: {
            let n = r.u16()? as usize;
            let mut v = Vec::with_capacity(n.min(1024));
            for _ in 0..n {
                let kind = r.str()?;
                let enabled = r.u8()? != 0;
                let pn = r.u16()? as usize;
                let mut params = std::collections::BTreeMap::new();
                for _ in 0..pn { params.insert(r.str()?, r.f32()?); }
                v.push(Effect { kind, params, enabled });
            }
            v
        },
    };
    Some(c)
}

fn put_opt_bus_id(o: &mut Vec<u8>, id: Option<BusId>) {
    match id {
        Some(b) => { put_u8(o, 1); put_u64(o, b.0); }
        None => put_u8(o, 0),
    }
}
fn get_opt_bus_id(r: &mut R) -> Option<Option<BusId>> {
    Some(match r.u8()? {
        0 => None,
        _ => Some(BusId(r.u64()?)),
    })
}

fn put_effects(o: &mut Vec<u8>, effects: &[Effect]) {
    put_u16(o, effects.len().min(u16::MAX as usize) as u16);
    for e in effects {
        put_str(o, &e.kind); put_u8(o, e.enabled as u8);
        put_u16(o, e.params.len().min(u16::MAX as usize) as u16);
        for (k, v) in &e.params { put_str(o, k); put_f32(o, *v); }
    }
}
fn get_effects(r: &mut R) -> Option<Vec<Effect>> {
    let n = r.u16()? as usize;
    let mut v = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let kind = r.str()?;
        let enabled = r.u8()? != 0;
        let pn = r.u16()? as usize;
        let mut params = std::collections::BTreeMap::new();
        for _ in 0..pn { params.insert(r.str()?, r.f32()?); }
        v.push(Effect { kind, params, enabled });
    }
    Some(v)
}

fn put_sends(o: &mut Vec<u8>, sends: &[Send]) {
    put_u16(o, sends.len().min(u16::MAX as usize) as u16);
    for s in sends {
        put_u64(o, s.to.0); put_f32(o, s.level); put_u8(o, s.pre_fader as u8);
    }
}
fn get_sends(r: &mut R) -> Option<Vec<Send>> {
    let n = r.u16()? as usize;
    let mut v = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        v.push(Send { to: BusId(r.u64()?), level: r.f32()?, pre_fader: r.u8()? != 0 });
    }
    Some(v)
}

fn put_bus(o: &mut Vec<u8>, b: &Bus) {
    put_u64(o, b.id.0);
    put_str(o, &b.name);
    put_opt_bus_id(o, b.parent);
    put_f32(o, b.gain);
    put_effects(o, &b.chain);
    put_sends(o, &b.sends);
    put_u8(o, b.muted as u8);
}
fn get_bus(r: &mut R) -> Option<Bus> {
    Some(Bus {
        id: BusId(r.u64()?), name: r.str()?, parent: get_opt_bus_id(r)?,
        gain: r.f32()?, chain: get_effects(r)?, sends: get_sends(r)?,
        muted: r.u8()? != 0,
    })
}

fn put_track(o: &mut Vec<u8>, t: &Track) {
    put_u64(o, t.id.0);
    put_u8(o, matches!(t.kind, TrackKind::Audio) as u8);
    put_str(o, &t.name);
    put_u8(o, t.muted as u8);
    put_u8(o, t.locked as u8);
    put_f32(o, t.gain);
    put_opt_bus_id(o, t.route);
}
fn get_track(r: &mut R) -> Option<Track> {
    Some(Track {
        id: TrackId(r.u64()?),
        kind: if r.u8()? != 0 { TrackKind::Audio } else { TrackKind::Video },
        name: r.str()?, muted: r.u8()? != 0, locked: r.u8()? != 0,
        gain: r.f32()?,
        route: get_opt_bus_id(r)?,
    })
}

fn put_source(o: &mut Vec<u8>, s: &Source) {
    put_u64(o, s.id.0); put_str(o, &s.path); put_ticks(o, s.len);
    put_u32(o, s.sample_rate); put_u16(o, s.channels); put_u8(o, s.has_video as u8);
}
fn get_source(r: &mut R) -> Option<Source> {
    Some(Source {
        id: SourceId(r.u64()?), path: r.str()?, len: r.ticks()?,
        sample_rate: r.u32()?, channels: r.u16()?, has_video: r.u8()? != 0,
    })
}

fn put_op(o: &mut Vec<u8>, op: &Op) {
    match op {
        Op::AddTrack { track, at } => { put_u8(o, 1); put_track(o, track); put_u32(o, *at as u32); }
        Op::RemoveTrack { id } => { put_u8(o, 2); put_u64(o, id.0); }
        Op::SetTrackFlags { id, muted, locked } => {
            put_u8(o, 3); put_u64(o, id.0); put_u8(o, *muted as u8); put_u8(o, *locked as u8);
        }
        Op::AddClip { clip } => { put_u8(o, 4); put_clip(o, clip); }
        Op::RemoveClip { id } => { put_u8(o, 5); put_u64(o, id.0); }
        Op::MoveClip { id, pos, track } => {
            put_u8(o, 6); put_u64(o, id.0); put_ticks(o, *pos); put_u64(o, track.0);
        }
        Op::TrimClip { id, src_in, len } => {
            put_u8(o, 7); put_u64(o, id.0); put_ticks(o, *src_in); put_ticks(o, *len);
        }
        Op::SetGain { id, gain } => { put_u8(o, 8); put_u64(o, id.0); put_f32(o, *gain); }
        Op::SetEffects { id, effects } => {
            put_u8(o, 9); put_u64(o, id.0);
            put_u16(o, effects.len().min(u16::MAX as usize) as u16);
            for e in effects {
                put_str(o, &e.kind); put_u8(o, e.enabled as u8);
                put_u16(o, e.params.len().min(u16::MAX as usize) as u16);
                for (k, v) in &e.params { put_str(o, k); put_f32(o, *v); }
            }
        }
        Op::AddSource { source } => { put_u8(o, 10); put_source(o, source); }
        Op::AddBus { bus } => { put_u8(o, 11); put_bus(o, bus); }
        Op::RemoveBus { id } => { put_u8(o, 12); put_u64(o, id.0); }
        Op::SetBusGain { id, gain } => { put_u8(o, 13); put_u64(o, id.0); put_f32(o, *gain); }
        Op::SetBusFlags { id, muted } => { put_u8(o, 17); put_u64(o, id.0); put_u8(o, *muted as u8); }
        Op::SetBusChain { id, chain } => { put_u8(o, 14); put_u64(o, id.0); put_effects(o, chain); }
        Op::SetBusSends { id, sends } => { put_u8(o, 15); put_u64(o, id.0); put_sends(o, sends); }
        Op::SetRoute { track, bus } => { put_u8(o, 16); put_u64(o, track.0); put_opt_bus_id(o, *bus); }
        Op::RemoveSource { id } => { put_u8(o, 18); put_u64(o, id.0); }
        Op::SetTrackGain { id, gain } => { put_u8(o, 19); put_u64(o, id.0); put_f32(o, *gain); }
    }
}

fn get_op(r: &mut R) -> Option<Op> {
    Some(match r.u8()? {
        1 => Op::AddTrack { track: get_track(r)?, at: r.u32()? as usize },
        2 => Op::RemoveTrack { id: TrackId(r.u64()?) },
        3 => Op::SetTrackFlags { id: TrackId(r.u64()?), muted: r.u8()? != 0, locked: r.u8()? != 0 },
        4 => Op::AddClip { clip: get_clip(r)? },
        5 => Op::RemoveClip { id: ClipId(r.u64()?) },
        6 => Op::MoveClip { id: ClipId(r.u64()?), pos: r.ticks()?, track: TrackId(r.u64()?) },
        7 => Op::TrimClip { id: ClipId(r.u64()?), src_in: r.ticks()?, len: r.ticks()? },
        8 => Op::SetGain { id: ClipId(r.u64()?), gain: r.f32()? },
        9 => Op::SetEffects {
            id: ClipId(r.u64()?),
            effects: {
                let n = r.u16()? as usize;
                let mut v = Vec::with_capacity(n.min(1024));
                for _ in 0..n {
                    let kind = r.str()?;
                    let enabled = r.u8()? != 0;
                    let pn = r.u16()? as usize;
                    let mut params = std::collections::BTreeMap::new();
                    for _ in 0..pn { params.insert(r.str()?, r.f32()?); }
                    v.push(Effect { kind, params, enabled });
                }
                v
            },
        },
        10 => Op::AddSource { source: get_source(r)? },
        11 => Op::AddBus { bus: get_bus(r)? },
        12 => Op::RemoveBus { id: BusId(r.u64()?) },
        13 => Op::SetBusGain { id: BusId(r.u64()?), gain: r.f32()? },
        17 => Op::SetBusFlags { id: BusId(r.u64()?), muted: r.u8()? != 0 },
        14 => Op::SetBusChain { id: BusId(r.u64()?), chain: get_effects(r)? },
        15 => Op::SetBusSends { id: BusId(r.u64()?), sends: get_sends(r)? },
        16 => Op::SetRoute { track: TrackId(r.u64()?), bus: get_opt_bus_id(r)? },
        18 => Op::RemoveSource { id: SourceId(r.u64()?) },
        19 => Op::SetTrackGain { id: TrackId(r.u64()?), gain: r.f32()? },
        _ => return None, // an op kind from a newer build: refuse, don't guess
    })
}

fn put_stamped(o: &mut Vec<u8>, s: &StampedOp) {
    put_u64(o, s.stamp.lamport);
    put_u64(o, s.stamp.site.0);
    put_op(o, &s.op);
}
fn get_stamped(r: &mut R) -> Option<StampedOp> {
    Some(StampedOp {
        stamp: Stamp { lamport: r.u64()?, site: SiteId(r.u64()?) },
        op: get_op(r)?,
    })
}

fn put_synthkind(o: &mut Vec<u8>, k: &SynthKind) {
    match k {
        SynthKind::Sine { hz } => { put_u8(o, 0); put_f32(o, *hz); }
        SynthKind::Sampler { source } => { put_u8(o, 1); put_u64(o, source.0); }
        SynthKind::Beatspace { z, crossfade } => {
            put_u8(o, 2);
            for v in z { put_f32(o, *v); }
            put_f32(o, *crossfade);
        }
        SynthKind::Sub { note, glide_from, glide_ms, drive, decay_ms } => {
            put_u8(o, 3);
            put_u8(o, *note as u8);
            put_u8(o, glide_from.is_some() as u8);
            put_u8(o, glide_from.unwrap_or(0) as u8);
            put_f32(o, *glide_ms);
            put_f32(o, *drive);
            put_f32(o, *decay_ms);
        }
        SynthKind::Drum { voice, tone, decay_ms, tune } => {
            put_u8(o, 4);
            put_u8(o, drum_voice_tag(*voice));
            put_f32(o, *tone);
            put_f32(o, *decay_ms);
            put_u8(o, *tune as u8);
        }
    }
}

// The voice's wire tag is written out by hand rather than derived from the
// enum's discriminant: `DrumVoice as u64` would silently renumber the wire
// if someone reorders the enum, and this file's whole job is that bytes on
// the wire never mean something different than they used to.
fn drum_voice_tag(v: studio_live::DrumVoice) -> u8 {
    use studio_live::DrumVoice::*;
    match v { Kick => 0, Snare => 1, Clap => 2, HatClosed => 3, HatOpen => 4 }
}
fn drum_voice_from(tag: u8) -> Option<studio_live::DrumVoice> {
    use studio_live::DrumVoice::*;
    Some(match tag { 0 => Kick, 1 => Snare, 2 => Clap, 3 => HatClosed, 4 => HatOpen,
                     _ => return None })
}
fn get_synthkind(r: &mut R) -> Option<SynthKind> {
    Some(match r.u8()? {
        0 => SynthKind::Sine { hz: r.f32()? },
        1 => SynthKind::Sampler { source: SourceId(r.u64()?) },
        2 => {
            let z = [r.f32()?, r.f32()?, r.f32()?];
            SynthKind::Beatspace { z, crossfade: r.f32()? }
        }
        3 => {
            let note = r.u8()? as i8;
            let has_glide = r.u8()? != 0;
            let glide_val = r.u8()? as i8;
            let glide_ms = r.f32()?;
            let drive = r.f32()?;
            let decay_ms = r.f32()?;
            SynthKind::Sub { note, glide_from: has_glide.then_some(glide_val), glide_ms, drive, decay_ms }
        }
        4 => {
            let voice = drum_voice_from(r.u8()?)?;
            let tone = r.f32()?;
            let decay_ms = r.f32()?;
            let tune = r.u8()? as i8;
            SynthKind::Drum { voice, tone, decay_ms, tune }
        }
        _ => return None, // a synth kind from a newer build: refuse, don't guess
    })
}

fn put_channel(o: &mut Vec<u8>, c: &Channel) {
    put_str(o, &c.name);
    put_synthkind(o, &c.kind);
    put_f32(o, c.gain);
    put_f32(o, c.pan);
    put_u8(o, c.muted as u8);
    put_u8(o, c.choke_group.is_some() as u8);
    put_u8(o, c.choke_group.unwrap_or(0));
}
fn get_channel(r: &mut R) -> Option<Channel> {
    let name = r.str()?;
    let kind = get_synthkind(r)?;
    let gain = r.f32()?;
    let pan = r.f32()?;
    let muted = r.u8()? != 0;
    let has_choke = r.u8()? != 0;
    let choke = r.u8()?;
    Some(Channel { name, kind, gain, pan, muted,
                   choke_group: has_choke.then_some(choke) })
}

fn put_stepmod(o: &mut Vec<u8>, m: &StepMod) {
    put_f32(o, m.vel);
    put_u8(o, m.ratchet);
    put_u8(o, m.tune as u8);
    put_u8(o, m.slide as u8);
    put_f32(o, m.prob);
    put_u8(o, m.every.is_some() as u8);
    let (n, k) = m.every.unwrap_or((0, 0));
    put_u8(o, n);
    put_u8(o, k);
}
fn get_stepmod(r: &mut R) -> Option<StepMod> {
    let vel = r.f32()?;
    let ratchet = r.u8()?;
    let tune = r.u8()? as i8;
    let slide = r.u8()? != 0;
    let prob = r.f32()?;
    let has_every = r.u8()? != 0;
    let n = r.u8()?;
    let k = r.u8()?;
    Some(StepMod { vel, ratchet, tune, slide, prob, every: has_every.then_some((n, k)) })
}

fn put_pattern(o: &mut Vec<u8>, pat: &Pattern) {
    put_str(o, &pat.name);
    put_u32(o, pat.steps);
    put_u16(o, pat.rows.len().min(u16::MAX as usize) as u16);
    for row in &pat.rows { put_u64(o, *row); }
    put_u16(o, pat.mods.len().min(u16::MAX as usize) as u16);
    for ((ch, step), m) in &pat.mods {
        put_u16(o, *ch as u16);
        put_u32(o, *step);
        put_stepmod(o, m);
    }
    // v9: the piano roll. BTreeMap iteration and add_note's sorted lists
    // keep the encoding canonical — the same roll is always the same bytes.
    put_u16(o, pat.rolls.len().min(u16::MAX as usize) as u16);
    for (ch, notes) in &pat.rolls {
        put_u16(o, *ch as u16);
        put_u16(o, notes.len().min(u16::MAX as usize) as u16);
        for n in notes {
            put_u8(o, n.key as u8);
            put_u32(o, n.start);
            put_u32(o, n.len);
            put_f32(o, n.vel);
            put_u8(o, n.slide as u8);
        }
    }
}
fn get_pattern(r: &mut R) -> Option<Pattern> {
    let name = r.str()?;
    let steps = r.u32()?;
    let rn = r.u16()? as usize;
    let mut rows = Vec::with_capacity(rn.min(4096));
    for _ in 0..rn { rows.push(r.u64()?); }
    let mn = r.u16()? as usize;
    let mut mods = std::collections::BTreeMap::new();
    for _ in 0..mn {
        let ch = r.u16()? as usize;
        let step = r.u32()?;
        let m = get_stepmod(r)?;
        mods.insert((ch, step), m);
    }
    let rn = r.u16()? as usize;
    let mut rolls = std::collections::BTreeMap::new();
    for _ in 0..rn {
        let ch = r.u16()? as usize;
        let nn = r.u16()? as usize;
        let mut notes = Vec::with_capacity(nn.min(4096));
        for _ in 0..nn {
            notes.push(Note {
                key: r.u8()? as i8,
                start: r.u32()?,
                len: r.u32()?,
                vel: r.f32()?,
                slide: r.u8()? != 0,
            });
        }
        rolls.insert(ch, notes);
    }
    Some(Pattern { name, steps, rows, mods, rolls })
}

fn put_rack(o: &mut Vec<u8>, rack: &Rack) {
    put_u16(o, rack.channels.len().min(u16::MAX as usize) as u16);
    for c in &rack.channels { put_channel(o, c); }
    put_u16(o, rack.patterns.len().min(u16::MAX as usize) as u16);
    for p in &rack.patterns { put_pattern(o, p); }
    put_f32(o, rack.tempo_bpm);
    put_f32(o, rack.swing);
}
fn get_rack(r: &mut R) -> Option<Rack> {
    let cn = r.u16()? as usize;
    let mut channels = Vec::with_capacity(cn.min(4096));
    for _ in 0..cn { channels.push(get_channel(r)?); }
    let pn = r.u16()? as usize;
    let mut patterns = Vec::with_capacity(pn.min(4096));
    for _ in 0..pn { patterns.push(get_pattern(r)?); }
    let tempo_bpm = r.f32()?;
    let swing = r.f32()?;
    Some(Rack { channels, patterns, tempo_bpm, swing })
}

impl Packet {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(64);
        put_u8(&mut o, WIRE_VERSION);
        match self {
            Packet::Op(s) => { put_u8(&mut o, 1); put_stamped(&mut o, s); }
            Packet::Presence(p) => {
                put_u8(&mut o, 2);
                put_u64(&mut o, p.site.0);
                put_str(&mut o, &p.name);
                put_ticks(&mut o, p.playhead);
                put_u64(&mut o, p.seq);
                put_u8(&mut o, p.active_track.is_some() as u8);
                put_u64(&mut o, p.active_track.map(|t| t.0).unwrap_or(0));
                put_u16(&mut o, p.selection.len().min(u16::MAX as usize) as u16);
                for c in &p.selection { put_u64(&mut o, c.0); }
            }
            Packet::Hello { site, since } => {
                put_u8(&mut o, 3); put_u64(&mut o, site.0); put_u64(&mut o, *since);
            }
            Packet::Catchup { ops } => {
                put_u8(&mut o, 4);
                put_u32(&mut o, ops.len() as u32);
                for s in ops { put_stamped(&mut o, s); }
            }
            Packet::Rack(rs) => {
                put_u8(&mut o, 5);
                put_u64(&mut o, rs.seq);
                put_u64(&mut o, rs.site.0);
                put_rack(&mut o, &rs.rack);
            }
            Packet::AssetRequest { source } => {
                put_u8(&mut o, 6);
                put_u64(&mut o, source.0);
            }
            Packet::AssetManifest { source, manifest } => {
                put_u8(&mut o, 7);
                put_u64(&mut o, source.0);
                put_u64(&mut o, manifest.asset);
                put_u64(&mut o, manifest.total_len as u64);
                put_u32(&mut o, manifest.chunks.len() as u32);
                for c in &manifest.chunks { put_u64(&mut o, *c); }
            }
            Packet::ChunkRequest { ids } => {
                put_u8(&mut o, 8);
                put_u32(&mut o, ids.len() as u32);
                for c in ids { put_u64(&mut o, *c); }
            }
            Packet::Chunk { id, bytes } => {
                put_u8(&mut o, 9);
                put_u64(&mut o, *id);
                put_u32(&mut o, bytes.len() as u32);
                o.extend_from_slice(bytes);
            }
        }
        o
    }

    /// Decode, or `None` for anything malformed, truncated, or from a version
    /// or op kind this build does not know. Never a partial result.
    pub fn decode(b: &[u8]) -> Option<Packet> {
        let mut r = R::new(b);
        if r.u8()? != WIRE_VERSION { return None; }
        let p = match r.u8()? {
            1 => Packet::Op(get_stamped(&mut r)?),
            2 => {
                let site = SiteId(r.u64()?);
                let name = r.str()?;
                let playhead = r.ticks()?;
                let seq = r.u64()?;
                let has_track = r.u8()? != 0;
                let track = r.u64()?;
                let n = r.u16()? as usize;
                let mut selection = Vec::with_capacity(n.min(4096));
                for _ in 0..n { selection.push(ClipId(r.u64()?)); }
                Packet::Presence(Presence {
                    site, name, playhead, seq,
                    active_track: has_track.then_some(TrackId(track)),
                    selection,
                })
            }
            3 => Packet::Hello { site: SiteId(r.u64()?), since: r.u64()? },
            4 => {
                let n = r.u32()? as usize;
                // Bounded so a corrupt length cannot make us allocate a gigabyte
                // before discovering the packet is short.
                let mut ops = Vec::with_capacity(n.min(4096));
                for _ in 0..n { ops.push(get_stamped(&mut r)?); }
                Packet::Catchup { ops }
            }
            5 => {
                let seq = r.u64()?;
                let site = SiteId(r.u64()?);
                let rack = get_rack(&mut r)?;
                Packet::Rack(RackSync { rack, seq, site })
            }
            6 => Packet::AssetRequest { source: SourceId(r.u64()?) },
            7 => {
                let source = SourceId(r.u64()?);
                let asset = r.u64()?;
                let total_len = r.u64()? as usize;
                let n = r.u32()? as usize;
                let mut chunks = Vec::with_capacity(n.min(1 << 20));
                for _ in 0..n { chunks.push(r.u64()?); }
                Packet::AssetManifest { source,
                    manifest: crate::assets::Manifest { asset, total_len, chunks } }
            }
            8 => {
                let n = r.u32()? as usize;
                let mut ids = Vec::with_capacity(n.min(1 << 20));
                for _ in 0..n { ids.push(r.u64()?); }
                Packet::ChunkRequest { ids }
            }
            9 => {
                let id = r.u64()?;
                let n = r.u32()? as usize;
                let bytes = r.take(n)?.to_vec();
                Packet::Chunk { id, bytes }
            }
            _ => return None,
        };
        Some(p)
    }

    /// Length-prefixed form, for a stream that carries packets back to back.
    pub fn frame(&self) -> Vec<u8> {
        let body = self.encode();
        let mut o = Vec::with_capacity(body.len() + 4);
        put_u32(&mut o, body.len() as u32);
        o.extend_from_slice(&body);
        o
    }

    /// Pull whole packets off a stream buffer, leaving any partial tail. Returns
    /// the packets and how many bytes were consumed — the caller drains that
    /// much and keeps the remainder for the next read.
    pub fn drain(buf: &[u8]) -> (Vec<Packet>, usize) {
        let mut out = Vec::new();
        let mut i = 0;
        while i + 4 <= buf.len() {
            let len = u32::from_le_bytes(buf[i..i + 4].try_into().unwrap()) as usize;
            if i + 4 + len > buf.len() { break; } // a partial packet: wait for more
            if let Some(p) = Packet::decode(&buf[i + 4..i + 4 + len]) {
                out.push(p);
            }
            // A packet that fails to decode is SKIPPED, not fatal: the frame
            // length is still trustworthy, so one bad packet must not desync the
            // stream and cost the session every edit after it.
            i += 4 + len;
        }
        (out, i)
    }
}

impl super::Doc {
    /// The packet a joining peer sends.
    pub fn hello(&self) -> Packet {
        Packet::Hello {
            site: self.site,
            since: self.history().last().map(|e| e.stamp.lamport).unwrap_or(0),
        }
    }

    /// The answer: everything stamped after `since`.
    pub fn catchup_for(&self, since: u64) -> Packet {
        Packet::Catchup {
            ops: self.history().iter().filter(|e| e.stamp.lamport > since).cloned().collect(),
        }
    }

    /// Handle an incoming packet. Ops and catch-ups integrate (idempotently, so
    /// overlapping with live traffic is fine); presence merges; a Hello is
    /// answered by the caller with `catchup_for`.
    pub fn receive(&mut self, p: Packet) -> Option<Packet> {
        match p {
            Packet::Op(s) => { let _ = self.integrate(&s); None }
            Packet::Catchup { ops } => {
                for s in ops { let _ = self.integrate(&s); }
                None
            }
            Packet::Presence(pr) => { self.presence(pr); None }
            Packet::Rack(rs) => { self.integrate_rack(rs); None }
            Packet::Hello { since, .. } => Some(self.catchup_for(since)),
            // Assets are bytes, and the Doc holds no bytes — media transfer
            // is the SHELL's layer (it owns the chunk store and the decode
            // path), so these fall through untouched rather than this struct
            // growing a second job.
            Packet::AssetRequest { .. } | Packet::AssetManifest { .. }
            | Packet::ChunkRequest { .. } | Packet::Chunk { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Doc;
    use studio_core::Project;

    #[test]
    fn the_asset_quartet_survives_the_wire() {
        // Built through Manifest::build so the ids in flight are REAL
        // content hashes, not fixture numbers a codec bug could permute
        // without anyone noticing.
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let (manifest, store) = crate::assets::Manifest::build(&data);
        let (first_id, first_bytes) = store.iter().next().map(|(k, v)| (*k, v.clone())).unwrap();
        let packets = vec![
            Packet::AssetRequest { source: SourceId(7) },
            Packet::AssetManifest { source: SourceId(7), manifest: manifest.clone() },
            Packet::ChunkRequest { ids: manifest.chunks.clone() },
            Packet::Chunk { id: first_id, bytes: first_bytes },
        ];
        for p in packets {
            let back = Packet::decode(&p.encode()).expect("decodes");
            assert_eq!(back, p);
        }
    }

    fn effects() -> Vec<Effect> {
        let mut p = std::collections::BTreeMap::new();
        p.insert("cutoff".to_string(), 880.0);
        p.insert("q".to_string(), 0.7);
        vec![Effect { kind: "lowpass".into(), params: p, enabled: true }]
    }

    fn every_op() -> Vec<Op> {
        let clip = Clip {
            id: ClipId(9), source: SourceId(3), track: TrackId(2),
            pos: Ticks::from_seconds(1.5), src_in: Ticks::from_seconds(0.25),
            len: Ticks::from_seconds(4.0), gain: 0.8, effects: effects(),
        };
        vec![
            Op::AddTrack { track: Track { id: TrackId(2), kind: TrackKind::Audio,
                name: "A1 — vocals".into(), muted: true, locked: false, gain: 1.0, route: None }, at: 3 },
            Op::RemoveTrack { id: TrackId(2) },
            Op::SetTrackFlags { id: TrackId(2), muted: false, locked: true },
            Op::AddClip { clip: clip.clone() },
            Op::RemoveClip { id: ClipId(9) },
            Op::MoveClip { id: ClipId(9), pos: Ticks::from_seconds(12.0), track: TrackId(5) },
            Op::TrimClip { id: ClipId(9), src_in: Ticks::from_seconds(2.0), len: Ticks::from_seconds(3.0) },
            Op::SetGain { id: ClipId(9), gain: 0.125 },
            Op::SetEffects { id: ClipId(9), effects: effects() },
            Op::AddSource { source: Source { id: SourceId(3), path: "/takes/vox 03.wav".into(),
                len: Ticks::from_seconds(90.0), sample_rate: 44_100, channels: 1, has_video: false } },
            Op::AddBus { bus: Bus {
                id: BusId(5), name: "808 — sub".into(), parent: Some(BusId(1)), gain: 0.9,
                chain: effects(),
                sends: vec![Send { to: BusId(2), level: 0.35, pre_fader: false }],
                muted: false,
            } },
            Op::AddBus { bus: Bus { id: BusId(1), name: "Master-ish".into(), parent: None, gain: 1.0, chain: vec![], sends: vec![], muted: false } },
            Op::RemoveBus { id: BusId(5) },
            Op::SetBusGain { id: BusId(1), gain: 0.6 },
            Op::SetBusFlags { id: BusId(1), muted: true },
            Op::SetBusChain { id: BusId(1), chain: effects() },
            Op::SetBusSends { id: BusId(1), sends: vec![
                Send { to: BusId(2), level: 0.2, pre_fader: true },
                Send { to: BusId(3), level: 0.0, pre_fader: false },
            ] },
            Op::SetRoute { track: TrackId(2), bus: Some(BusId(1)) },
            Op::SetRoute { track: TrackId(2), bus: None },
            Op::RemoveSource { id: SourceId(3) },
            Op::SetTrackGain { id: TrackId(2), gain: 0.7 },
        ]
    }

    #[test]
    fn every_op_survives_the_round_trip_exactly() {
        // Including the awkward parts: a non-ASCII track name, a path with a
        // space, effect parameters in a map, and negative-capable tick values.
        for op in every_op() {
            let p = Packet::Op(StampedOp { stamp: Stamp { lamport: 42, site: SiteId(7) }, op: op.clone() });
            let bytes = p.encode();
            let back = Packet::decode(&bytes).expect("decodes");
            assert_eq!(p, back, "round-trip changed the op: {op:?}");
        }
    }

    #[test]
    fn presence_and_the_handshake_round_trip() {
        let pr = Packet::Presence(Presence {
            site: SiteId(2), name: "ada".into(), playhead: Ticks::from_seconds(31.5),
            selection: vec![ClipId(1), ClipId(4)], active_track: Some(TrackId(9)), seq: 17,
        });
        assert_eq!(Packet::decode(&pr.encode()).unwrap(), pr);
        // no active track is a distinct state from track 0
        let none = Packet::Presence(Presence {
            site: SiteId(2), name: "ada".into(), playhead: Ticks::ZERO,
            selection: vec![], active_track: None, seq: 1,
        });
        match Packet::decode(&none.encode()).unwrap() {
            Packet::Presence(p) => assert!(p.active_track.is_none(), "None must not decode as Some(0)"),
            other => panic!("{other:?}"),
        }
        let h = Packet::Hello { site: SiteId(5), since: 99 };
        assert_eq!(Packet::decode(&h.encode()).unwrap(), h);
    }

    fn sample_rack() -> Rack {
        let mut pattern = Pattern::new("beat", 16, 3);
        pattern.toggle(0, 0);
        pattern.toggle(1, 4);
        pattern.set_mod(1, 4, StepMod { vel: 0.7, ratchet: 4, tune: -5, slide: true, prob: 0.8, every: Some((1, 2)) });
        // v9: a roll with the awkward parts — a negative key, a chord, and
        // a slide — so every rack round trip exercises the note codec.
        pattern.add_note(1, Note { key: -12, start: 0, len: 4, vel: 0.9, slide: false });
        pattern.add_note(1, Note { key: 7, start: 0, len: 2, vel: 0.5, slide: false });
        pattern.add_note(1, Note { key: 3, start: 6, len: 1, vel: 1.0, slide: true });
        Rack {
            channels: vec![
                Channel { name: "kick".into(), kind: SynthKind::Sine { hz: 60.0 }, gain: 1.0, pan: 0.0, muted: false, choke_group: Some(2) },
                Channel { name: "808".into(), kind: SynthKind::Sub { note: -5, glide_from: Some(0), glide_ms: 40.0, drive: 0.3, decay_ms: 300.0 }, gain: 0.9, pan: -0.1, muted: false, choke_group: None },
                Channel { name: "hat".into(), kind: SynthKind::Sampler { source: SourceId(3) }, gain: 0.6, pan: 0.2, muted: true, choke_group: Some(0) },
            ],
            patterns: vec![pattern],
            tempo_bpm: 140.0, swing: 0.11,
        }
    }

    #[test]
    fn a_rack_survives_the_wire_round_trip_exactly() {
        // Including the awkward parts: a Sub channel's Option<i8> glide_from,
        // a sparse StepMod with every field non-default, and a Beatspace
        // channel's fixed-size z array.
        let mut rack = sample_rack();
        rack.channels.push(Channel {
            name: "gan".into(),
            kind: SynthKind::Beatspace { z: [0.25, -0.5, 0.9], crossfade: 0.4 },
            gain: 1.0, pan: 0.0, muted: false, choke_group: None,
        });
        let p = Packet::Rack(RackSync { rack, seq: 12, site: SiteId(3) });
        let bytes = p.encode();
        let back = Packet::decode(&bytes).expect("decodes");
        assert_eq!(p, back, "round-trip changed the rack");
    }

    #[test]
    fn an_empty_rack_round_trips_too() {
        let p = Packet::Rack(RackSync { rack: Rack::default(), seq: 1, site: SiteId(1) });
        assert_eq!(Packet::decode(&p.encode()).unwrap(), p);
    }

    #[test]
    fn doc_receive_dispatches_a_rack_packet_into_integrate_rack() {
        // The exhaustiveness-checked dispatch point: this is what the
        // compiler's "non-exhaustive patterns" error caught while adding the
        // Rack variant — a packet that encodes/decodes fine but is silently
        // dropped by `receive` would be a real, quiet bug, not a hypothetical.
        let mut doc = Doc::new(SiteId(1), Project::default());
        assert!(doc.rack.is_none());
        let sync = RackSync { rack: sample_rack(), seq: 1, site: SiteId(2) };
        let reply = doc.receive(Packet::Rack(sync.clone()));
        assert!(reply.is_none(), "a rack packet needs no reply, unlike a Hello");
        assert_eq!(doc.rack, Some(sync));
    }

    #[test]
    fn concurrent_rack_edits_converge_via_the_wire_packet_path() {
        // The full path: two sites each publish a rack via set_rack, encode
        // it to a real Packet, decode it back, and feed it to the other's
        // receive() — not just calling integrate_rack directly in memory.
        let mut a = Doc::new(SiteId(1), Project::default());
        let mut b = Doc::new(SiteId(2), Project::default());

        let sync_a = a.set_rack(sample_rack());
        let mut rack_b = sample_rack();
        rack_b.tempo_bpm = 174.0; // a different, conflicting edit
        let sync_b = b.set_rack(rack_b);

        // Deliver each other's packet through the real wire encode/decode.
        let pkt_a = Packet::Rack(sync_a).encode();
        let pkt_b = Packet::Rack(sync_b).encode();
        b.receive(Packet::decode(&pkt_a).unwrap());
        a.receive(Packet::decode(&pkt_b).unwrap());

        assert_eq!(a.rack, b.rack, "both sites must converge on the identical rack");
        // Both started from seq 1 (fresh docs) — the tie breaks on site, and
        // SiteId(2) > SiteId(1), so b's edit (174 bpm) should win.
        assert_eq!(a.rack.as_ref().unwrap().rack.tempo_bpm, 174.0);
    }

    #[test]
    fn a_later_local_edit_always_outranks_a_previously_seen_remote_one() {
        let mut a = Doc::new(SiteId(1), Project::default());
        let remote = RackSync { rack: sample_rack(), seq: 5, site: SiteId(9) };
        a.integrate_rack(remote);
        assert_eq!(a.rack.as_ref().unwrap().seq, 5);

        // A's own fresh edit must win even though A's site id (1) is lower
        // than the remote's (9) — set_rack bases its seq on the highest seq
        // seen so far specifically so a genuinely NEW edit is never
        // out-ranked by an old one just because of site-id tie-breaking.
        let mut new_rack = sample_rack();
        new_rack.tempo_bpm = 90.0;
        let local = a.set_rack(new_rack);
        assert!(local.seq > 5, "a fresh local edit must get a higher seq than whatever was already known");
        assert_eq!(a.rack.as_ref().unwrap().rack.tempo_bpm, 90.0);
    }

    #[test]
    fn truncated_and_unknown_packets_are_refused_not_half_decoded() {
        // An edit applied from a corrupt packet would diverge one replica
        // silently — the exact failure the whole design exists to prevent.
        let full = Packet::Op(StampedOp {
            stamp: Stamp { lamport: 1, site: SiteId(1) },
            op: Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1),
                pos: Ticks::ZERO, src_in: Ticks::ZERO, len: Ticks::from_seconds(1.0),
                gain: 1.0, effects: effects() } },
        }).encode();
        for cut in 1..full.len() {
            assert!(Packet::decode(&full[..cut]).is_none(), "a {cut}-byte prefix must not decode");
        }
        assert!(Packet::decode(&[]).is_none());
        // a future wire version is refused rather than misread
        let mut wrong = full.clone();
        wrong[0] = WIRE_VERSION + 1;
        assert!(Packet::decode(&wrong).is_none(), "an unknown version must not be guessed at");
        // an op kind from a newer build likewise
        let mut future_op = full.clone();
        future_op[18] = 200;
        assert!(Packet::decode(&future_op).is_none());
        // and the specific case this bump exists for: a peer still running
        // the pre-bus-routing build (v1) sends a packet stamped with its own
        // version. The new build must refuse it loudly rather than trying to
        // read a Track that is missing the `route` byte it now expects.
        let mut stale = full.clone();
        stale[0] = 1;
        assert!(Packet::decode(&stale).is_none(), "a v1 peer's packet must be refused, not misread");
        // v2 existed too (bus routing without bus mute) — also refused now.
        let mut stale_v2 = full.clone();
        stale_v2[0] = 2;
        assert!(Packet::decode(&stale_v2).is_none(), "a v2 peer's packet must be refused, not misread");
        // v3 existed too (bus mute, no RemoveSource) — also refused now.
        let mut stale_v3 = full.clone();
        stale_v3[0] = 3;
        assert!(Packet::decode(&stale_v3).is_none(), "a v3 peer's packet must be refused, not misread");
    }

    #[test]
    fn a_v1_peer_sending_the_current_wire_version_would_still_be_misread_if_we_forgot_to_bump() {
        // This is the inverse check: it pins that WIRE_VERSION really did
        // move, so a future edit to the encoding can't silently forget the
        // bump and let two incompatible builds both claim version N.
        assert_eq!(WIRE_VERSION, 9, "bump this alongside the next encoding change, not instead of it");
    }

    #[test]
    fn a_stream_yields_whole_packets_and_keeps_the_partial_tail() {
        let a = Packet::Hello { site: SiteId(1), since: 0 };
        let b = Packet::Op(StampedOp { stamp: Stamp { lamport: 3, site: SiteId(1) },
            op: Op::SetGain { id: ClipId(2), gain: 0.5 } });
        let mut stream = a.frame();
        stream.extend(b.frame());
        let (packets, used) = Packet::drain(&stream);
        assert_eq!(packets, vec![a.clone(), b.clone()]);
        assert_eq!(used, stream.len());

        // a half-delivered second packet leaves the first available and the
        // tail untouched, which is what a socket read actually looks like
        let partial = &stream[..stream.len() - 3];
        let (packets, used) = Packet::drain(partial);
        assert_eq!(packets, vec![a.clone()]);
        assert_eq!(used, a.frame().len(), "only the complete packet was consumed");
        assert!(used < partial.len());
    }

    #[test]
    fn one_corrupt_packet_does_not_desync_the_stream() {
        // The frame length is still trustworthy, so a bad packet is skipped
        // rather than costing the session every edit after it.
        let good = Packet::Hello { site: SiteId(1), since: 0 };
        let mut stream = good.frame();
        let mut bad = good.frame();
        bad[5] = 99; // wreck the body, keep the length
        stream.extend(bad);
        stream.extend(good.frame());
        let (packets, used) = Packet::drain(&stream);
        assert_eq!(packets.len(), 2, "the two good packets survive the bad one between them");
        assert_eq!(used, stream.len(), "and the stream stays aligned");
    }

    #[test]
    fn a_joiner_catches_up_to_an_identical_project_over_the_wire() {
        // End to end: edits made on one site, encoded, decoded, and integrated
        // on another that started cold.
        let mut host = Doc::new(SiteId(1), Project { name: "cut".into(), ..Default::default() });
        host.local(Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video,
            name: "V1".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        host.local(Op::AddSource { source: Source { id: SourceId(1), path: "a.mov".into(),
            len: Ticks::from_seconds(60.0), sample_rate: 48_000, channels: 2, has_video: true } }).unwrap();
        for i in 0..6u64 {
            host.local(Op::AddClip { clip: Clip { id: ClipId(i), source: SourceId(1), track: TrackId(1),
                pos: Ticks::from_seconds(i as f64 * 5.0), src_in: Ticks::ZERO,
                len: Ticks::from_seconds(4.0), gain: 1.0, effects: Vec::new() } }).unwrap();
        }
        host.local(Op::SetGain { id: ClipId(3), gain: 0.3 }).unwrap();

        let mut joiner = Doc::new(SiteId(2), Project { name: "cut".into(), ..Default::default() });
        // handshake, entirely through bytes
        let hello = Packet::decode(&joiner.hello().encode()).unwrap();
        let answer = host.receive(hello).expect("a hello is answered");
        let answer = Packet::decode(&answer.encode()).unwrap();
        joiner.receive(answer);
        assert_eq!(joiner.project, host.project, "the joiner reconstructed the session");

        // and a live edit arriving afterwards still lands
        let live = host.local(Op::MoveClip { id: ClipId(0), pos: Ticks::from_seconds(50.0),
            track: TrackId(1) }).unwrap();
        joiner.receive(Packet::decode(&Packet::Op(live).encode()).unwrap());
        assert_eq!(joiner.project, host.project);

        // catch-up overlapping live traffic is harmless, because integrate is
        // idempotent — the joiner can receive both without coordination
        let again = host.catchup_for(0);
        joiner.receive(Packet::decode(&again.encode()).unwrap());
        assert_eq!(joiner.project, host.project, "replaying everything changes nothing");
    }
}
