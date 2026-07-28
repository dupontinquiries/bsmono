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

use crate::{Presence, SiteId, Stamp, StampedOp};
use studio_core::{Clip, ClipId, Effect, Op, Source, SourceId, Ticks, Track, TrackId, TrackKind};

/// Bumped whenever the encoding changes shape. A peer that sees a version it
/// does not know refuses the packet rather than guessing.
pub const WIRE_VERSION: u8 = 1;

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

fn put_track(o: &mut Vec<u8>, t: &Track) {
    put_u64(o, t.id.0);
    put_u8(o, matches!(t.kind, TrackKind::Audio) as u8);
    put_str(o, &t.name);
    put_u8(o, t.muted as u8);
    put_u8(o, t.locked as u8);
}
fn get_track(r: &mut R) -> Option<Track> {
    Some(Track {
        id: TrackId(r.u64()?),
        kind: if r.u8()? != 0 { TrackKind::Audio } else { TrackKind::Video },
        name: r.str()?, muted: r.u8()? != 0, locked: r.u8()? != 0,
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
            Packet::Hello { since, .. } => Some(self.catchup_for(since)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Doc;
    use studio_core::Project;

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
                name: "A1 — vocals".into(), muted: true, locked: false }, at: 3 },
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
            name: "V1".into(), muted: false, locked: false }, at: 0 }).unwrap();
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
