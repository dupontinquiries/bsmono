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

use crate::{Presence, RackSync, SiteId, Stamp, StampedOp, ViewMode};
use studio_core::{AutoTarget, Binding, BindingId, Bus, BusId, Clip, ClipId, CurveId, Effect, Gate, GroupId, Op, ParamKey, Send, Source, SourceId, Ticks, Track, TrackId, TrackKind};
use studio_live::{Channel, Note, Pattern, Rack, StepMod, SynthKind};

/// Bumped whenever the encoding changes shape. A peer that sees a version it
/// does not know refuses the packet rather than guessing.
///
/// v16: `Presence` gained `following` (has-site byte + site id, same shape
/// as `active_track`) and `view_mode` (one byte, 0-3) -- for
/// `follow_graph`'s chain resolution: a peer several hops down a follow
/// chain needs to see the ROOT's view mode, which means every peer's own
/// follow target and view mode both have to be on the wire, not just
/// derivable locally.
///
/// v15: `Presence` gained `recording` (one byte after `seq`) and
/// `Packet::Voice`(10) — one encoded voice frame for the in-house call,
/// `site` + `seq` + bytes; the codec is the shell's business.
///
/// v14: reusable automation — `SetCurve`(32), `RemoveCurve`(33),
/// `SetCurvePoint`(34), `RemoveCurvePoint`(35), `SetBinding`(36),
/// `RemoveBinding`(37) — with the `Gate` encoding (kind byte: 0 always,
/// 1 clip + id, 2 window + start + end) and `CurveId`/`BindingId`.
///
/// v13: the key and the markers — `SetKey`(26), `SetMarker`(27),
/// `RemoveMarker`(28), `ClearMarkers`(29).
///
/// v12: `Op::Group`(25) — several ops as one undo step, nested encoding.
///
/// v11: `Op` gained the input-side routing and composition set —
/// `SetBusParent`(22), `SetGroup`(23), `RemoveGroup`(24) — and `GroupId`.
///
/// v10: `Op` gained the automation pair — `SetAutoPoint`(20) /
/// `RemoveAutoPoint`(21) — and with them the `AutoTarget` encoding
/// (kind byte + id).
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
/// v18: `Op::SetSourcePath` (tag 46). A tag addition only — no existing
/// encoding changed — but the version still moves, because a v17 peer that
/// receives one cannot apply it and would silently diverge from everyone
/// who can.
///
/// v17: `Bus` gained `silent: bool` (a measurement tap that feeds nothing —
/// see `Bus::silent`), and `Op` gained `SetBusSilent` (tag 45). The bus
/// shape itself changed, so this is a bump and not a quiet tag addition: a
/// v16 peer reading a v17 bus would take the silent byte for the next
/// field and desynchronise the whole stream.
/// v19: `Binding` gained `response` and `fit_span` (a scene's stage signal
/// driving several targets differently, and surviving a re-trim).
/// v20: `Source` carries its KIND. Until now `put_source` wrote no kind and
/// `get_source` rebuilt every source as `SourceKind::File`, so a linked
/// pattern clip's source came back from a saved file -- or a peer -- as a
/// file with an empty path: "missing or undecodable", silent (found when an
/// FL import saved 51 pattern sources and reopened with 51 missing). A shape
/// change, so a bump: tag 0 File, 1 Pattern (u64 pattern, str name).
/// v21: `Op::SetBusPan` (tag 47) -- Studio's first pan in the timeline mix.
/// v22: `SynthKind::Poly` (tag 6) -- the compact generated synth.
/// v23: a Poly patch's TEXTURE -- harmonic profile, noise, FM, drive.
/// v24: a Poly patch's IMAGE -- width, low width, pan by key, pan LFO.
pub const WIRE_VERSION: u8 = 24;

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
    /// One encoded voice frame from `site` — the in-house call. Never
    /// logged, never replayed; a late frame is dropped by the receiver's
    /// jitter buffer (`voice::Jitter`). The bytes are whatever codec the
    /// shell chose (Opus by intent); this crate never decodes them.
    Voice { site: SiteId, seq: u32, frame: Vec<u8> },
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

/// The oldest version a SAVE FILE may be and still open. Live packets
/// between peers stay exact-version (peers upgrade together); a file is
/// read years later, so it is decoded with the version it was written at.
/// Every bump this session (16 -> 17 silent busses, 18 relink, 19 binding
/// response) had, until this existed, silently stopped every older project
/// from opening.
pub const FIRST_FILE_VERSION: u8 = 16;

/// The version the encoder writes. Always WIRE_VERSION in the product; tests
/// can write an OLDER version to prove old files still open, which needs real
/// old bytes and not a hand-built imitation of them.
#[cfg(test)]
thread_local! { pub(crate) static ENCODE_AS: std::cell::Cell<u8> = const { std::cell::Cell::new(WIRE_VERSION) }; }
/// The version the encoder is writing (the product always writes the newest).
pub(crate) fn current_encode_version() -> u8 { enc_v() }
fn enc_v() -> u8 {
    #[cfg(test)] { return ENCODE_AS.with(|c| c.get()); }
    #[allow(unreachable_code)] WIRE_VERSION
}

struct R<'a> { b: &'a [u8], i: usize, v: u8 }
impl<'a> R<'a> {
    fn new(b: &'a [u8]) -> Self { R { b, i: 0, v: WIRE_VERSION } }
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

fn put_opt_clip_id(o: &mut Vec<u8>, id: Option<ClipId>) {
    match id {
        Some(c) => { put_u8(o, 1); put_u64(o, c.0); }
        None => put_u8(o, 0),
    }
}
fn get_opt_clip_id(r: &mut R) -> Option<Option<ClipId>> {
    Some(match r.u8()? {
        0 => None,
        _ => Some(ClipId(r.u64()?)),
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
    if enc_v() >= 17 { put_u8(o, b.silent as u8); }
}
fn get_bus(r: &mut R) -> Option<Bus> {
    Some(Bus {
        id: BusId(r.u64()?), name: r.str()?, parent: get_opt_bus_id(r)?,
        gain: r.f32()?, chain: get_effects(r)?, sends: get_sends(r)?,
        muted: r.u8()? != 0,
        // v17 added it; a bus from an older file was never silent
        silent: if r.v >= 17 { r.u8()? != 0 } else { false },
    })
}

fn put_auto_target(o: &mut Vec<u8>, t: AutoTarget) {
    match t {
        AutoTarget::TrackGain(id) => { put_u8(o, 0); put_u64(o, id.0); }
        AutoTarget::BusGain(id) => { put_u8(o, 1); put_u64(o, id.0); }
        // The parameter targets: id, effect index, then the key's 16
        // fixed bytes as they are — no length prefix, the pad is the
        // encoding. Kinds 2 and 3 do not bump WIRE_VERSION: a v15 peer
        // already refuses an unknown target kind (the arm below), and a
        // bump would refuse every v15 save file for a target most
        // projects never draw.
        AutoTarget::BusParam { bus, effect, key } => {
            put_u8(o, 2); put_u64(o, bus.0); put_u8(o, effect); o.extend_from_slice(&key.0);
        }
        AutoTarget::ClipParam { clip, effect, key } => {
            put_u8(o, 3); put_u64(o, clip.0); put_u8(o, effect); o.extend_from_slice(&key.0);
        }
    }
}
fn get_auto_target(r: &mut R) -> Option<AutoTarget> {
    match r.u8()? {
        0 => Some(AutoTarget::TrackGain(TrackId(r.u64()?))),
        1 => Some(AutoTarget::BusGain(BusId(r.u64()?))),
        2 => {
            let bus = BusId(r.u64()?);
            let effect = r.u8()?;
            let key = ParamKey(r.take(16)?.try_into().ok()?);
            Some(AutoTarget::BusParam { bus, effect, key })
        }
        3 => {
            let clip = ClipId(r.u64()?);
            let effect = r.u8()?;
            let key = ParamKey(r.take(16)?.try_into().ok()?);
            Some(AutoTarget::ClipParam { clip, effect, key })
        }
        _ => None, // an unknown target refuses the packet, like an unknown op
    }
}

fn put_gate(o: &mut Vec<u8>, g: Gate) {
    match g {
        Gate::Always => put_u8(o, 0),
        Gate::Clip(id) => { put_u8(o, 1); put_u64(o, id.0); }
        Gate::Window { start, end } => { put_u8(o, 2); put_ticks(o, start); put_ticks(o, end); }
        // Kind 3 without a WIRE_VERSION bump, for the same reason as the
        // parameter targets: an unknown gate kind refuses the packet, and
        // a bump would refuse every older save file.
        Gate::Group(g) => { put_u8(o, 3); put_u64(o, g.0); }
    }
}
fn get_gate(r: &mut R) -> Option<Gate> {
    match r.u8()? {
        0 => Some(Gate::Always),
        1 => Some(Gate::Clip(ClipId(r.u64()?))),
        2 => Some(Gate::Window { start: r.ticks()?, end: r.ticks()? }),
        3 => Some(Gate::Group(GroupId(r.u64()?))),
        _ => None, // an unknown gate kind refuses the packet, like an unknown op
    }
}
fn put_binding(o: &mut Vec<u8>, b: &Binding) {
    put_u64(o, b.curve.0); put_auto_target(o, b.target); put_gate(o, b.gate);
    if enc_v() >= 19 {
        match b.response {
            None => put_u8(o, 0),
            Some(rsp) => { put_u8(o, 1); put_f32(o, rsp.from); put_f32(o, rsp.to); put_f32(o, rsp.floor); put_f32(o, rsp.ceil); }
        }
        put_u8(o, b.fit_span as u8);
    }
}
fn get_binding(r: &mut R) -> Option<Binding> {
    let (curve, target, gate) = (CurveId(r.u64()?), get_auto_target(r)?, get_gate(r)?);
    // v19 added the response and fit; an older binding read its curve straight
    let (response, fit_span) = if r.v >= 19 {
        let response = match r.u8()? {
            0 => None,
            1 => Some(studio_core::Response { from: r.f32()?, to: r.f32()?, floor: r.f32()?, ceil: r.f32()? }),
            _ => return None,
        };
        (response, r.u8()? != 0)
    } else { (None, false) };
    Some(Binding { curve, target, gate, response, fit_span })
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
    if enc_v() >= 20 {
        match &s.kind {
            studio_core::SourceKind::File => put_u8(o, 0),
            studio_core::SourceKind::Pattern { pattern, name } => { put_u8(o, 1); put_u64(o, *pattern as u64); put_str(o, name); }
        }
    }
}
fn get_source(r: &mut R) -> Option<Source> {
    let (id, path, len, sample_rate, channels, has_video) =
        (SourceId(r.u64()?), r.str()?, r.ticks()?, r.u32()?, r.u16()?, r.u8()? != 0);
    // before v20 no kind was written: every source read as a file (the bug above)
    let kind = if r.v >= 20 {
        match r.u8()? {
            0 => studio_core::SourceKind::File,
            1 => studio_core::SourceKind::Pattern { pattern: r.u64()? as usize, name: r.str()? },
            _ => return None,
        }
    } else { studio_core::SourceKind::File };
    Some(Source { id, path, len, sample_rate, channels, has_video, kind })
}

/// `Option<String>`: 0 = None, 1 + the string = Some. Shared because
/// `Provenance`/`ProxyInfo` between them need it three times.
fn put_opt_str(o: &mut Vec<u8>, s: &Option<String>) {
    match s {
        None => put_u8(o, 0),
        Some(s) => { put_u8(o, 1); put_str(o, s); }
    }
}
fn get_opt_str(r: &mut R) -> Option<Option<String>> {
    Some(match r.u8()? { 0 => None, _ => Some(r.str()?) })
}

/// `Option<[u8; 32]>` (a content hash): 0 = None, 1 + 32 raw bytes = Some.
fn put_opt_hash(o: &mut Vec<u8>, h: &Option<[u8; 32]>) {
    match h {
        None => put_u8(o, 0),
        Some(h) => { put_u8(o, 1); o.extend_from_slice(h); }
    }
}
fn get_opt_hash(r: &mut R) -> Option<Option<[u8; 32]>> {
    Some(match r.u8()? {
        0 => None,
        _ => {
            let mut buf = [0u8; 32];
            for b in buf.iter_mut() { *b = r.u8()?; }
            Some(buf)
        }
    })
}

fn put_fingerprint(o: &mut Vec<u8>, f: &studio_core::FingerprintData) {
    put_ticks(o, f.interval);
    put_u64(o, f.hashes.len() as u64);
    for h in &f.hashes {
        put_u64(o, *h);
    }
}
fn get_fingerprint(r: &mut R) -> Option<studio_core::FingerprintData> {
    let interval = r.ticks()?;
    let n = r.u64()? as usize;
    let mut hashes = Vec::with_capacity(n.min(1 << 20)); // sane cap against a corrupt length prefix
    for _ in 0..n {
        hashes.push(r.u64()?);
    }
    Some(studio_core::FingerprintData { hashes, interval })
}

fn put_provenance(o: &mut Vec<u8>, p: &studio_core::Provenance) {
    o.extend_from_slice(&p.content_hash);
    put_u64(o, p.file_size);
    put_opt_str(o, &p.last_verified_path);
    put_opt_str(o, &p.verified_at);
    match &p.proxy_of {
        None => put_u8(o, 0),
        Some(px) => {
            put_u8(o, 1);
            put_opt_hash(o, &px.original_hash);
            put_opt_str(o, &px.reacquire_hint);
            put_opt_str(o, &px.note);
        }
    }
}
fn get_provenance(r: &mut R) -> Option<studio_core::Provenance> {
    let mut content_hash = [0u8; 32];
    for b in content_hash.iter_mut() { *b = r.u8()?; }
    let file_size = r.u64()?;
    let last_verified_path = get_opt_str(r)?;
    let verified_at = get_opt_str(r)?;
    let proxy_of = match r.u8()? {
        0 => None,
        _ => Some(studio_core::ProxyInfo {
            original_hash: get_opt_hash(r)?,
            reacquire_hint: get_opt_str(r)?,
            note: get_opt_str(r)?,
        }),
    };
    Some(studio_core::Provenance { content_hash, file_size, last_verified_path, verified_at, proxy_of })
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
        Op::SetBusSilent { id, silent } => { put_u8(o, 45); put_u64(o, id.0); put_u8(o, *silent as u8); }
        Op::SetSourcePath { id, path } => { put_u8(o, 46); put_u64(o, id.0); put_str(o, path); }
        Op::SetBusPan { id, pan } => { put_u8(o, 47); put_u64(o, id.0); put_f32(o, *pan); }
        Op::SetBusChain { id, chain } => { put_u8(o, 14); put_u64(o, id.0); put_effects(o, chain); }
        Op::SetBusSends { id, sends } => { put_u8(o, 15); put_u64(o, id.0); put_sends(o, sends); }
        Op::SetRoute { track, bus } => { put_u8(o, 16); put_u64(o, track.0); put_opt_bus_id(o, *bus); }
        Op::RemoveSource { id } => { put_u8(o, 18); put_u64(o, id.0); }
        Op::SetTrackGain { id, gain } => { put_u8(o, 19); put_u64(o, id.0); put_f32(o, *gain); }
        Op::SetAutoPoint { target, at, value } => {
            put_u8(o, 20); put_auto_target(o, *target); put_ticks(o, *at); put_f32(o, *value);
        }
        Op::RemoveAutoPoint { target, at } => {
            put_u8(o, 21); put_auto_target(o, *target); put_ticks(o, *at);
        }
        Op::SetBusParent { id, parent } => {
            put_u8(o, 22); put_u64(o, id.0);
            match parent {
                Some(p) => { put_u8(o, 1); put_u64(o, p.0); }
                None => put_u8(o, 0),
            }
        }
        Op::SetGroup { id, name, clips } => {
            put_u8(o, 23); put_u64(o, id.0); put_str(o, name);
            put_u16(o, clips.len().min(u16::MAX as usize) as u16);
            for c in clips { put_u64(o, c.0); }
        }
        Op::RemoveGroup { id } => { put_u8(o, 24); put_u64(o, id.0); }
        Op::SetKey { key } => {
            put_u8(o, 26);
            put_opt_key(o, key);
        }
        Op::SetMarker { at, name } => { put_u8(o, 27); put_ticks(o, *at); put_str(o, name); }
        Op::RemoveMarker { name } => { put_u8(o, 28); put_str(o, name); }
        Op::ClearMarkers => put_u8(o, 29),
        Op::SetGroupKey { id, key } => {
            put_u8(o, 30); put_u64(o, id.0); put_opt_key(o, key);
        }
        Op::SetGroupTempo { id, tempo } => {
            put_u8(o, 31); put_u64(o, id.0); put_opt_tempo(o, tempo);
        }
        Op::SetCurve { id, name } => { put_u8(o, 32); put_u64(o, id.0); put_str(o, name); }
        Op::RemoveCurve { id } => { put_u8(o, 33); put_u64(o, id.0); }
        Op::SetCurvePoint { curve, at, value } => {
            put_u8(o, 34); put_u64(o, curve.0); put_ticks(o, *at); put_f32(o, *value);
        }
        Op::RemoveCurvePoint { curve, at } => { put_u8(o, 35); put_u64(o, curve.0); put_ticks(o, *at); }
        Op::SetBinding { id, binding } => { put_u8(o, 36); put_u64(o, id.0); put_binding(o, binding); }
        Op::RemoveBinding { id } => { put_u8(o, 37); put_u64(o, id.0); }
        Op::SetProvenance { source, provenance } => {
            put_u8(o, 38); put_u64(o, source.0); put_provenance(o, provenance);
        }
        Op::RemoveProvenance { source } => { put_u8(o, 39); put_u64(o, source.0); }
        Op::SetFingerprint { source, fingerprint } => {
            put_u8(o, 40); put_u64(o, source.0); put_fingerprint(o, fingerprint);
        }
        Op::RemoveFingerprint { source } => { put_u8(o, 41); put_u64(o, source.0); }
        // 43, 44: appended, never inserted. A tag byte IS the wire format --
        // renumbering an existing op would make two peers on different builds
        // decode each other's edits as the wrong operation, which is worse
        // than failing to decode them at all.
        Op::SetGroupWindow { id, window } => {
            put_u8(o, 43); put_u64(o, id.0);
            match window {
                Some((a, b)) => { put_u8(o, 1); put_ticks(o, *a); put_ticks(o, *b); }
                None => put_u8(o, 0),
            }
        }
        Op::SetClipFade { id, fade_in, fade_out } => {
            put_u8(o, 44); put_u64(o, id.0); put_ticks(o, *fade_in); put_ticks(o, *fade_out);
        }
        Op::SetGroupActive { id, active } => {
            put_u8(o, 42); put_u64(o, id.0); put_opt_clip_id(o, *active);
        }
        Op::Group { ops } => {
            put_u8(o, 25);
            put_u16(o, ops.len().min(u16::MAX as usize) as u16);
            for inner in ops.iter().take(u16::MAX as usize) { put_op(o, inner); }
        }
    }
}

/// `SetKey` and `SetGroupKey` write the identical `Option<Key>` shape
/// (0 = None, 1 + root + scale-tag = Some) — factored out once this
/// session added the second writer of it, rather than a second hand-kept
/// copy of the scale-tag table drifting from the first.
fn put_opt_key(o: &mut Vec<u8>, key: &Option<studio_core::Key>) {
    match key {
        None => put_u8(o, 0),
        Some(k) => {
            put_u8(o, 1);
            put_u8(o, k.root);
            put_u8(o, match k.scale {
                studio_core::Scale::Major => 0, studio_core::Scale::Minor => 1,
                studio_core::Scale::Dorian => 2, studio_core::Scale::Mixolydian => 3,
                studio_core::Scale::Chromatic => 4,
            });
        }
    }
}

fn get_opt_key(r: &mut R) -> Option<Option<studio_core::Key>> {
    Some(match r.u8()? {
        0 => None,
        1 => Some(studio_core::Key {
            root: r.u8()?,
            scale: match r.u8()? {
                0 => studio_core::Scale::Major, 1 => studio_core::Scale::Minor,
                2 => studio_core::Scale::Dorian, 3 => studio_core::Scale::Mixolydian,
                4 => studio_core::Scale::Chromatic, _ => return None,
            } }),
        _ => return None,
    })
}

/// `TempoMap` is opaque (its `points` field is private, deliberately, so
/// every reader gets the sorted/floored/non-empty invariant `TempoMap::new`
/// enforces) — the wire round-trips it through `.points()`/`::new()`
/// rather than reaching into a field the type does not expose, same
/// discipline as encoding `Effect`/`Bus` elsewhere in this file via their
/// own public accessors.
fn put_opt_tempo(o: &mut Vec<u8>, tempo: &Option<studio_core::TempoMap>) {
    match tempo {
        None => put_u8(o, 0),
        Some(t) => {
            put_u8(o, 1);
            let pts = t.points();
            put_u16(o, pts.len().min(u16::MAX as usize) as u16);
            for (tick, bpm) in pts { put_ticks(o, *tick); put_f32(o, *bpm); }
        }
    }
}

fn get_opt_tempo(r: &mut R) -> Option<Option<studio_core::TempoMap>> {
    Some(match r.u8()? {
        0 => None,
        1 => {
            let n = r.u16()? as usize;
            let mut pts = Vec::with_capacity(n.min(4096));
            for _ in 0..n { pts.push((r.ticks()?, r.f32()?)); }
            Some(studio_core::TempoMap::new(pts))
        }
        _ => return None,
    })
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
        20 => Op::SetAutoPoint { target: get_auto_target(r)?, at: r.ticks()?, value: r.f32()? },
        21 => Op::RemoveAutoPoint { target: get_auto_target(r)?, at: r.ticks()? },
        22 => Op::SetBusParent {
            id: BusId(r.u64()?),
            parent: match r.u8()? { 0 => None, 1 => Some(BusId(r.u64()?)), _ => return None },
        },
        23 => {
            let id = GroupId(r.u64()?);
            let name = r.str()?;
            let n = r.u16()? as usize;
            let mut clips = Vec::with_capacity(n.min(4096));
            for _ in 0..n { clips.push(ClipId(r.u64()?)); }
            Op::SetGroup { id, name, clips }
        }
        24 => Op::RemoveGroup { id: GroupId(r.u64()?) },
        26 => Op::SetKey { key: get_opt_key(r)? },
        27 => Op::SetMarker { at: Ticks(r.i64()?), name: r.str()? },
        28 => Op::RemoveMarker { name: r.str()? },
        29 => Op::ClearMarkers,
        30 => Op::SetGroupKey { id: GroupId(r.u64()?), key: get_opt_key(r)? },
        31 => Op::SetGroupTempo { id: GroupId(r.u64()?), tempo: get_opt_tempo(r)? },
        32 => Op::SetCurve { id: CurveId(r.u64()?), name: r.str()? },
        33 => Op::RemoveCurve { id: CurveId(r.u64()?) },
        34 => Op::SetCurvePoint { curve: CurveId(r.u64()?), at: r.ticks()?, value: r.f32()? },
        35 => Op::RemoveCurvePoint { curve: CurveId(r.u64()?), at: r.ticks()? },
        36 => Op::SetBinding { id: BindingId(r.u64()?), binding: get_binding(r)? },
        37 => Op::RemoveBinding { id: BindingId(r.u64()?) },
        38 => Op::SetProvenance { source: SourceId(r.u64()?), provenance: get_provenance(r)? },
        39 => Op::RemoveProvenance { source: SourceId(r.u64()?) },
        40 => Op::SetFingerprint { source: SourceId(r.u64()?), fingerprint: get_fingerprint(r)? },
        41 => Op::RemoveFingerprint { source: SourceId(r.u64()?) },
        42 => Op::SetGroupActive { id: GroupId(r.u64()?), active: get_opt_clip_id(r)? },
        43 => {
            let id = GroupId(r.u64()?);
            let window = match r.u8()? {
                0 => None,
                _ => Some((Ticks(r.i64()?), Ticks(r.i64()?))),
            };
            Op::SetGroupWindow { id, window }
        }
        45 => Op::SetBusSilent { id: BusId(r.u64()?), silent: r.u8()? != 0 },
        46 => Op::SetSourcePath { id: SourceId(r.u64()?), path: r.str()? },
        47 => Op::SetBusPan { id: BusId(r.u64()?), pan: r.f32()? },
        44 => Op::SetClipFade {
            id: ClipId(r.u64()?),
            fade_in: Ticks(r.i64()?),
            fade_out: Ticks(r.i64()?),
        },
        25 => {
            let n = r.u16()? as usize;
            let mut ops = Vec::with_capacity(n.min(4096));
            for _ in 0..n { ops.push(get_op(r)?); }
            Op::Group { ops }
        }
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
        SynthKind::Poly { hz, patch: p } => {
            put_u8(o, 6);
            put_f32(o, *hz);
            put_u8(o, p.wave); put_u8(o, p.voices);
            for v in [p.detune, p.sub, p.attack_ms, p.decay_ms, p.sustain, p.release_ms, p.cutoff, p.resonance, p.env_amount, p.vibrato, p.gain] {
                put_f32(o, v);
            }
            if enc_v() >= 23 {
                o.extend_from_slice(&p.partials);
                for v in [p.noise, p.fm_ratio, p.fm_index, p.drive] { put_f32(o, v); }
            }
            if enc_v() >= 24 {
                for v in [p.width, p.low_width, p.key_pan, p.pan_lfo_hz, p.pan_lfo_depth] { put_f32(o, v); }
            }
        }
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
        SynthKind::Sf2 { path, bank, preset, transpose } => {
            put_u8(o, 5);
            // The PATH crosses the wire, and it is a path on the SENDER's
            // machine. That is deliberate rather than an oversight: a
            // collaborator who has the same soundfont at the same place
            // gets the instrument, and one who does not gets a channel
            // that renders silent and still carries every note written
            // on it.
            //
            // The alternative — refusing to sync a channel whose file
            // the receiver lacks — would delete a part from their copy
            // of a shared project, which is worse than silence.
            put_str(o, &path.to_string_lossy());
            put_u16(o, *bank);
            put_u8(o, *preset);
            put_u8(o, *transpose as u8);
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
        6 => {
            let hz = r.f32()?;
            let (wave, voices) = (r.u8()?, r.u8()?);
            let mut f = [0f32; 11];
            for v in f.iter_mut() { *v = r.f32()?; }
            let mut patch = studio_live::PolyPatch { wave, voices, detune: f[0], sub: f[1], attack_ms: f[2], decay_ms: f[3],
                sustain: f[4], release_ms: f[5], cutoff: f[6], resonance: f[7], env_amount: f[8], vibrato: f[9], gain: f[10], ..Default::default() };
            if r.v >= 23 {
                patch.partials.copy_from_slice(r.take(16)?);
                patch.noise = r.f32()?; patch.fm_ratio = r.f32()?; patch.fm_index = r.f32()?; patch.drive = r.f32()?;
            }
            if r.v >= 24 {
                patch.width = r.f32()?; patch.low_width = r.f32()?; patch.key_pan = r.f32()?;
                patch.pan_lfo_hz = r.f32()?; patch.pan_lfo_depth = r.f32()?;
            }
            SynthKind::Poly { hz, patch }
        }
        5 => SynthKind::Sf2 {
            path: std::path::PathBuf::from(r.str()?),
            bank: r.u16()?,
            preset: r.u8()?,
            transpose: r.u8()? as i8,
        },
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
    Some(Rack { channels, patterns, tempo_bpm, swing, ..Default::default() })
}

impl Packet {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(64);
        put_u8(&mut o, enc_v());
        match self {
            Packet::Op(s) => { put_u8(&mut o, 1); put_stamped(&mut o, s); }
            Packet::Presence(p) => {
                put_u8(&mut o, 2);
                put_u64(&mut o, p.site.0);
                put_str(&mut o, &p.name);
                put_ticks(&mut o, p.playhead);
                put_u64(&mut o, p.seq);
                put_u8(&mut o, p.recording as u8);
                put_u8(&mut o, p.active_track.is_some() as u8);
                put_u64(&mut o, p.active_track.map(|t| t.0).unwrap_or(0));
                put_u16(&mut o, p.selection.len().min(u16::MAX as usize) as u16);
                for c in &p.selection { put_u64(&mut o, c.0); }
                put_u8(&mut o, p.following.is_some() as u8);
                put_u64(&mut o, p.following.map(|s| s.0).unwrap_or(0));
                put_u8(&mut o, match p.view_mode {
                    ViewMode::Plain => 0, ViewMode::Badges => 1,
                    ViewMode::Thumbnails => 2, ViewMode::DataInspector => 3,
                });
            }
            Packet::Hello { site, since } => {
                put_u8(&mut o, 3); put_u64(&mut o, site.0); put_u64(&mut o, *since);
            }
            Packet::Voice { site, seq, frame } => {
                put_u8(&mut o, 10); put_u64(&mut o, site.0); put_u32(&mut o, *seq);
                put_u16(&mut o, frame.len().min(u16::MAX as usize) as u16);
                o.extend_from_slice(&frame[..frame.len().min(u16::MAX as usize)]);
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
        Packet::decode_in(b, WIRE_VERSION..=WIRE_VERSION)
    }

    /// Decode a packet stored in a SAVE FILE: any version from
    /// FIRST_FILE_VERSION up, each read with the fields its version had.
    pub fn decode_stored(b: &[u8]) -> Option<Packet> {
        Packet::decode_in(b, FIRST_FILE_VERSION..=WIRE_VERSION)
    }

    fn decode_in(b: &[u8], accept: std::ops::RangeInclusive<u8>) -> Option<Packet> {
        let mut r = R::new(b);
        let v = r.u8()?;
        if !accept.contains(&v) { return None; }
        r.v = v;
        let p = match r.u8()? {
            1 => Packet::Op(get_stamped(&mut r)?),
            2 => {
                let site = SiteId(r.u64()?);
                let name = r.str()?;
                let playhead = r.ticks()?;
                let seq = r.u64()?;
                let recording = r.u8()? != 0;
                let has_track = r.u8()? != 0;
                let track = r.u64()?;
                let n = r.u16()? as usize;
                let mut selection = Vec::with_capacity(n.min(4096));
                for _ in 0..n { selection.push(ClipId(r.u64()?)); }
                let has_following = r.u8()? != 0;
                let following_site = r.u64()?;
                let view_mode = match r.u8()? {
                    1 => ViewMode::Badges,
                    2 => ViewMode::Thumbnails,
                    3 => ViewMode::DataInspector,
                    _ => ViewMode::Plain,
                };
                Packet::Presence(Presence {
                    site, name, playhead, seq,
                    active_track: has_track.then_some(TrackId(track)),
                    selection, recording,
                    following: has_following.then_some(SiteId(following_site)),
                    view_mode,
                })
            }
            3 => Packet::Hello { site: SiteId(r.u64()?), since: r.u64()? },
            10 => {
                let site = SiteId(r.u64()?);
                let seq = r.u32()?;
                let n = r.u16()? as usize;
                Packet::Voice { site, seq, frame: r.take(n)?.to_vec() }
            }
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
            // Voice is audio for the shell's jitter buffer and sink — the
            // Doc must never see it, log it, or replay it.
            Packet::Voice { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn an_sf2_channel_round_trips_on_the_wire() {
        // A collaborator who lacks the soundfont must still receive the
        // CHANNEL — silence is recoverable, a missing part is not.
        let k = SynthKind::Sf2 {
            path: std::path::PathBuf::from("/home/x/piano.sf2"),
            bank: 0, preset: 42, transpose: -12,
        };
        let mut o = Vec::new();
        put_synthkind(&mut o, &k);
        let mut r = R::new(&o);
        assert_eq!(get_synthkind(&mut r), Some(k));
    }

    #[test]
    fn a_negative_transpose_survives_the_byte_it_travels_in() {
        // `transpose` is i8 and goes over as u8. A sign lost here drops
        // an octave-down channel an octave UP, which is audible and
        // would look like a synth bug rather than a wire bug.
        for t in [-12i8, -1, 0, 1, 127, -128] {
            let k = SynthKind::Sf2 {
                path: "/p.sf2".into(), bank: 128, preset: 0, transpose: t,
            };
            let mut o = Vec::new();
            put_synthkind(&mut o, &k);
            assert_eq!(get_synthkind(&mut R::new(&o)), Some(k), "transpose {t}");
        }
    }
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
                len: Ticks::from_seconds(90.0), sample_rate: 44_100, channels: 1, has_video: false, kind: studio_core::SourceKind::File } },
            Op::AddBus { bus: Bus {
                id: BusId(5), name: "808 — sub".into(), parent: Some(BusId(1)), gain: 0.9,
                chain: effects(),
                sends: vec![Send { to: BusId(2), level: 0.35, pre_fader: false }],
                muted: false, silent: false,
            } },
            Op::AddBus { bus: Bus { id: BusId(1), name: "Master-ish".into(), parent: None, gain: 1.0, chain: vec![], sends: vec![], muted: false, silent: false } },
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
            Op::SetAutoPoint { target: AutoTarget::BusParam { bus: BusId(1), effect: 2,
                                   key: ParamKey::new("freq0").unwrap() }, at: Ticks(7), value: 0.25 },
            Op::RemoveAutoPoint { target: AutoTarget::ClipParam { clip: ClipId(3), effect: 0,
                                      key: ParamKey::new("preserve_pitch").unwrap() }, at: Ticks(7) },
            Op::SetCurve { id: CurveId(4), name: "filter sweep — ad-lib".into() },
            Op::RemoveCurve { id: CurveId(4) },
            Op::SetCurvePoint { curve: CurveId(4), at: Ticks::from_seconds(0.5), value: 0.0 },
            Op::RemoveCurvePoint { curve: CurveId(4), at: Ticks::from_seconds(0.5) },
            Op::SetBinding { id: BindingId(1), binding: Binding { curve: CurveId(4),
                target: AutoTarget::TrackGain(TrackId(2)), gate: Gate::Always, response: None, fit_span: false } },
            Op::SetBinding { id: BindingId(2), binding: Binding { curve: CurveId(4),
                target: AutoTarget::BusGain(BusId(1)), gate: Gate::Clip(ClipId(9)), response: None, fit_span: false } },
            Op::SetBinding { id: BindingId(3), binding: Binding { curve: CurveId(4),
                target: AutoTarget::TrackGain(TrackId(2)),
                gate: Gate::Window { start: Ticks::from_seconds(8.0), end: Ticks::from_seconds(16.0) }, response: None, fit_span: false } },
            Op::SetBinding { id: BindingId(3), binding: Binding { curve: CurveId(4),
                target: AutoTarget::TrackGain(TrackId(1)), gate: Gate::Group(GroupId(7)),
                response: Some(studio_core::Response { from: 0.5, to: 0.9, floor: 0.0, ceil: 1.0 }), fit_span: true } },
            Op::RemoveBinding { id: BindingId(3) },
            Op::SetProvenance { source: SourceId(5), provenance: studio_core::Provenance {
                content_hash: [7u8; 32], file_size: 123_456_789,
                last_verified_path: Some("footage/clip moved.mov".into()), // the space and awkward name are the point
                verified_at: Some("2026-09-14T00:00:00Z".into()),
                proxy_of: Some(studio_core::ProxyInfo {
                    original_hash: Some([9u8; 32]),
                    reacquire_hint: Some("Blackmagic 6K card, shoot 2026-09-06".into()),
                    note: Some("original deleted to free the card".into()),
                }),
            } },
            // The all-None shape too -- a provenance record with no proxy
            // info is the common case (most sources are never proxies).
            Op::SetProvenance { source: SourceId(6), provenance: studio_core::Provenance {
                content_hash: [0u8; 32], file_size: 0,
                last_verified_path: None, verified_at: None, proxy_of: None,
            } },
            Op::RemoveProvenance { source: SourceId(5) },
            Op::SetFingerprint { source: SourceId(1), fingerprint: studio_core::FingerprintData {
                hashes: vec![0, u64::MAX, 0x0123_4567_89ab_cdef, 1],
                interval: Ticks::from_seconds(1.5),
            } },
            // Empty hash list -- a fingerprint attempt on a zero-duration
            // or unreadable clip is a real, not-hypothetical shape.
            Op::SetFingerprint { source: SourceId(2), fingerprint: studio_core::FingerprintData {
                hashes: vec![], interval: Ticks::from_seconds(1.0),
            } },
            Op::RemoveFingerprint { source: SourceId(1) },
            Op::SetGroupActive { id: GroupId(1), active: Some(ClipId(2)) },
            Op::SetGroupActive { id: GroupId(1), active: None },
            // Both window states, because the presence byte is the part that
            // can be wrong -- a None that decoded as Some would read two ticks
            // of the NEXT op's bytes and corrupt everything after it.
            Op::SetGroupWindow {
                id: GroupId(3),
                window: Some((Ticks::from_seconds(1.0), Ticks::from_seconds(2.5))),
            },
            Op::SetGroupWindow { id: GroupId(3), window: None },
            // Negative-capable: Ticks is i64 and a fade is arithmetic on it.
            Op::SetClipFade {
                id: ClipId(9),
                fade_in: Ticks::from_seconds(0.25),
                fade_out: Ticks(0),
            },
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
            selection: vec![ClipId(1), ClipId(4)], active_track: Some(TrackId(9)), seq: 17, recording: false, following: None, view_mode: ViewMode::Plain,
        });
        assert_eq!(Packet::decode(&pr.encode()).unwrap(), pr);
        // no active track is a distinct state from track 0
        let none = Packet::Presence(Presence {
            site: SiteId(2), name: "ada".into(), playhead: Ticks::ZERO,
            selection: vec![], active_track: None, seq: 1, recording: false, following: None, view_mode: ViewMode::Plain,
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
            tempo_bpm: 140.0, swing: 0.11, ..Default::default() }
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
        assert_eq!(WIRE_VERSION, 24, "bump this alongside the next encoding change, not instead of it");
    }

    #[test]
    fn a_relink_survives_the_wire() {
        let p = Packet::Op(StampedOp { stamp: Stamp { lamport: 3, site: SiteId(2) },
            op: Op::SetSourcePath { id: SourceId(7), path: "/card/A001/C0057.MXF".into() } });
        assert_eq!(Packet::decode(&p.encode()).expect("decodes"), p);
    }

    #[test]
    fn a_silent_bus_and_its_op_survive_the_wire() {
        let bus = Bus { id: BusId(7), name: "Control".into(), parent: None, gain: 1.0,
            chain: vec![], sends: vec![Send { to: BusId(2), level: 1.0, pre_fader: true }],
            muted: false, silent: true };
        for op in [Op::AddBus { bus: bus.clone() }, Op::SetBusSilent { id: BusId(7), silent: true },
                   Op::SetBusSilent { id: BusId(7), silent: false }] {
            let p = Packet::Op(StampedOp { stamp: Stamp { lamport: 1, site: SiteId(1) }, op: op.clone() });
            let back = Packet::decode(&p.encode()).expect("decodes");
            assert_eq!(back, p, "{op:?}");
        }
        // and a peer that predates silent busses is refused rather than
        // reading the silent byte as the start of the next field
        let full = Packet::Op(StampedOp { stamp: Stamp { lamport: 1, site: SiteId(1) },
            op: Op::AddBus { bus } }).encode();
        let mut stale = full.clone();
        stale[0] = 16;
        assert!(Packet::decode(&stale).is_none(), "a v16 peer's packet must be refused");
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
            len: Ticks::from_seconds(60.0), sample_rate: 48_000, channels: 2, has_video: true, kind: studio_core::SourceKind::File } }).unwrap();
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

#[cfg(test)]
mod automation_wire_tests {
    use super::*;
    use studio_core::AutoTarget;

    #[test]
    fn the_automation_ops_survive_the_wire_exactly() {
        for op in [
            Op::SetAutoPoint { target: AutoTarget::TrackGain(TrackId(3)),
                               at: Ticks(705_600_000), value: 0.25 },
            Op::SetAutoPoint { target: AutoTarget::BusGain(BusId(9)),
                               at: Ticks(0), value: 1.5 },
            Op::RemoveAutoPoint { target: AutoTarget::TrackGain(TrackId(3)),
                                  at: Ticks(705_600_000) },
        ] {
            let p = Packet::Op(StampedOp { stamp: Stamp { lamport: 7, site: SiteId(2) }, op });
            assert_eq!(Packet::decode(&p.encode()).expect("decodes"), p);
        }
    }

    #[test]
    fn presence_following_and_view_mode_survive_the_wire() {
        for (following, mode) in [
            (None, ViewMode::Plain),
            (Some(SiteId(7)), ViewMode::Badges),
            (Some(SiteId(0)), ViewMode::Thumbnails), // site id 0 specifically: must not be confused with "no following" (has-byte + 0, not just 0)
            (None, ViewMode::DataInspector),
        ] {
            let p = Packet::Presence(Presence { site: SiteId(1), name: "a".into(), playhead: Ticks(0),
                selection: vec![], active_track: None, seq: 1, recording: false,
                following, view_mode: mode });
            let back = Packet::decode(&p.encode()).expect("decodes");
            match back {
                Packet::Presence(pr) => {
                    assert_eq!(pr.following, following, "following mismatch for {mode:?}");
                    assert_eq!(pr.view_mode, mode);
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn a_recording_flag_and_a_voice_frame_survive_the_wire_and_refuse_every_prefix() {
        let p = Packet::Presence(Presence { site: SiteId(4), name: "ada".into(), playhead: Ticks(77),
            selection: vec![ClipId(2)], active_track: Some(TrackId(1)), seq: 9, recording: true,
            following: None, view_mode: ViewMode::Plain });
        let back = Packet::decode(&p.encode()).expect("decodes");
        assert_eq!(back, p);
        match back { Packet::Presence(pr) => assert!(pr.recording), _ => unreachable!() }
        let v = Packet::Voice { site: SiteId(4), seq: 123_456, frame: vec![1, 2, 3, 250, 0, 7] };
        let full = v.encode();
        assert_eq!(Packet::decode(&full).expect("decodes"), v);
        for cut in 1..full.len() {
            assert!(Packet::decode(&full[..cut]).is_none(), "a {cut}-byte prefix must not decode");
        }
        let empty = Packet::Voice { site: SiteId(1), seq: 0, frame: vec![] };
        assert_eq!(Packet::decode(&empty.encode()).expect("an empty frame is a legal packet"), empty);
    }

    #[test]
    fn an_unknown_gate_kind_refuses_the_packet_and_every_binding_prefix_fails() {
        let p = Packet::Op(StampedOp { stamp: Stamp { lamport: 7, site: SiteId(2) },
            op: Op::SetBinding { id: BindingId(5), binding: Binding { curve: CurveId(1),
                target: AutoTarget::BusGain(BusId(2)), gate: Gate::Clip(ClipId(3)), response: None, fit_span: false } } });
        let full = p.encode();
        assert_eq!(Packet::decode(&full).expect("decodes"), p);
        for cut in 1..full.len() {
            assert!(Packet::decode(&full[..cut]).is_none(), "a {cut}-byte prefix must not decode");
        }
        // The gate kind byte is the LAST tag in the packet: after the header,
        // stamp, op tag, binding id, curve id and the target's kind+id.
        let gate_at = full.iter().rposition(|&b| b == 1).expect("the clip-gate tag is there");
        let mut future = full.clone();
        future[gate_at] = 9;
        assert!(Packet::decode(&future).is_none(), "an unknown gate must not be guessed at");
    }
}

#[cfg(test)]
mod group_wire_tests {
    use super::*;
    use studio_core::GroupId;

    #[test]
    fn the_v11_ops_survive_the_wire_exactly() {
        for op in [
            Op::SetBusParent { id: BusId(3), parent: Some(BusId(9)) },
            Op::SetBusParent { id: BusId(3), parent: None },
            Op::SetGroup { id: GroupId(7), name: "hook".into(),
                           clips: vec![ClipId(1), ClipId(5)] },
            Op::RemoveGroup { id: GroupId(7) },
        ] {
            let p = Packet::Op(StampedOp { stamp: Stamp { lamport: 3, site: SiteId(1) }, op });
            assert_eq!(Packet::decode(&p.encode()).expect("decodes"), p);
        }
    }
}

#[cfg(test)]
mod group_key_wire_tests {
    use super::*;
    use studio_core::{GroupId, Key, Scale};

    #[test]
    fn set_group_key_survives_the_wire_exactly_some_and_none() {
        for op in [
            Op::SetGroupKey { id: GroupId(7), key: Some(Key { root: 2, scale: Scale::Dorian }) },
            Op::SetGroupKey { id: GroupId(7), key: None },
            // every scale tag, since put_opt_key/get_opt_key hand-encode
            // the tag table rather than deriving it — a new Scale variant
            // added without a matching wire tag would silently miscode
            // rather than fail to compile, so every tag gets exercised.
            Op::SetGroupKey { id: GroupId(1), key: Some(Key { root: 0, scale: Scale::Major }) },
            Op::SetGroupKey { id: GroupId(1), key: Some(Key { root: 0, scale: Scale::Minor }) },
            Op::SetGroupKey { id: GroupId(1), key: Some(Key { root: 0, scale: Scale::Mixolydian }) },
            Op::SetGroupKey { id: GroupId(1), key: Some(Key { root: 0, scale: Scale::Chromatic }) },
        ] {
            let p = Packet::Op(StampedOp { stamp: Stamp { lamport: 4, site: SiteId(1) }, op });
            assert_eq!(Packet::decode(&p.encode()).expect("decodes"), p);
        }
    }

    #[test]
    fn a_parameter_target_a_peer_does_not_know_refuses_the_packet_without_a_version_bump() {
        // Kinds 2 and 3 were added without bumping WIRE_VERSION (see
        // put_auto_target). The claim that makes that safe: a peer that
        // does not know a target kind refuses the whole packet, and does
        // not read the bytes after it as something else. Find the kind
        // byte by diffing two otherwise-identical packets, then set it to
        // a kind no build has.
        let at = |t: AutoTarget| Packet::Op(StampedOp {
            stamp: Stamp { lamport: 4, site: SiteId(1) },
            op: Op::SetAutoPoint { target: t, at: Ticks(9), value: 0.5 } }).encode();
        let a = at(AutoTarget::TrackGain(TrackId(5)));
        let b = at(AutoTarget::BusGain(BusId(5)));
        assert_eq!(a.len(), b.len());
        let kind_at = a.iter().zip(&b).position(|(x, y)| x != y).expect("the kind byte differs");
        assert_eq!((a[kind_at], b[kind_at]), (0, 1));
        let mut future = a.clone();
        future[kind_at] = 9;
        assert!(Packet::decode(&future).is_none(), "kind 9: refused, not misread");
        // And the parameter kinds carry their key with no length prefix:
        // exactly 16 bytes after the index, whatever the key's length.
        let short = at(AutoTarget::BusParam { bus: BusId(5), effect: 0, key: ParamKey::new("q").unwrap() });
        let long = at(AutoTarget::BusParam { bus: BusId(5), effect: 0, key: ParamKey::new("preserve_pitch").unwrap() });
        assert_eq!(short.len(), long.len(), "fixed-shape on the wire");
        assert_eq!(short.len(), a.len() + 1 + 16, "index byte plus the key's 16");
    }
}

#[cfg(test)]
mod poly_wire_tests {
    use super::*;

    /// A Poly patch round-trips exactly, and is SMALL: the point of it.
    #[test]
    fn a_poly_patch_round_trips_and_fits_in_a_few_dozen_bytes() {
        let mut partials = [0u8; 16];
        partials[0] = 255; partials[4] = 90;
        let k = SynthKind::Poly { hz: 261.63, patch: studio_live::PolyPatch { wave: 3, voices: 5, detune: 14.0, sub: 0.3, cutoff: 900.0,
            partials, noise: 0.1, fm_ratio: 2.0, fm_index: 1.5, drive: 0.4,
            width: 0.7, low_width: 0.3, key_pan: 0.25, pan_lfo_hz: 0.2, pan_lfo_depth: 0.4, ..Default::default() } };
        let mut o = Vec::new();
        put_synthkind(&mut o, &k);
        assert!(o.len() <= 120, "a patch WITH its texture and image: {} bytes", o.len());
        let mut r = R { b: &o, i: 0, v: WIRE_VERSION };
        assert_eq!(get_synthkind(&mut r), Some(k));
    }
}
