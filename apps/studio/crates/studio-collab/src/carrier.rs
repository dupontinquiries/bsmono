//! carrier — putting the wire onto something that actually moves bytes.
//!
//! [`wire`](crate::wire) says what a packet looks like; this says how it
//! survives a carrier with real limits. The intended one is the monorepo's
//! `snapclone-dht`, whose shape is the awkward case and therefore the one worth
//! designing against:
//!
//! * **Records have a size cap.** A single clip nudge fits easily; the catch-up
//!   that brings a joiner up to date does not — a few hundred ops is tens of
//!   kilobytes and a DHT value is measured in low kilobytes. Something has to
//!   split it.
//! * **Delivery is unordered and lossy.** Records arrive out of sequence, twice,
//!   or not at all.
//! * **Storage is keyed, not streamed.** You publish under a key and peers read
//!   that key; there is no connection to carry framing.
//!
//! So the adapter here does three things: derives a stable **session key** both
//! sides compute independently, **chunks** anything too large with enough
//! header to reassemble it, and **tolerates** duplicates and reordering because
//! the layer above already does.
//!
//! What it deliberately does NOT do is retransmission or ordering. `Doc` is
//! already idempotent and order-independent, and a lost chunk is recovered by
//! the joiner simply asking again — building a reliability layer on top of a
//! CRDT is solving a problem the CRDT already solved.

use crate::wire::Packet;

/// Conservative payload budget for one carrier record. Real DHT values run to a
/// few kilobytes; staying well under leaves room for the carrier's own signing
/// and envelope without having to model it here.
pub const MAX_CHUNK: usize = 1024;

/// A session's key, derived from its name so every participant computes the
/// same one without being told it. Same construction as snapclone's
/// `key_for_scid`: an FNV-1a walk folded across all 20 bytes, so short names
/// still fill the key and land in different parts of the keyspace.
pub fn session_key(session: &str) -> [u8; 20] {
    let mut h = [0u8; 20];
    let mut acc: u64 = 0xcbf2_9ce4_8422_2325;
    for (i, b) in session.bytes().enumerate() {
        acc ^= b as u64;
        acc = acc.wrapping_mul(0x100_0000_01b3);
        h[i % 20] ^= (acc >> ((i % 8) * 8)) as u8;
    }
    for (i, slot) in h.iter_mut().enumerate() {
        acc = acc.wrapping_mul(0x100_0000_01b3) ^ (i as u64);
        *slot ^= (acc >> 24) as u8;
    }
    h
}

/// One carrier record: a slice of a packet, with what is needed to rebuild it.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// Identifies the packet this belongs to. Unique per sender per packet.
    pub msg: u64,
    pub index: u16,
    pub total: u16,
    pub body: Vec<u8>,
}

impl Chunk {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(self.body.len() + 12);
        o.extend_from_slice(&self.msg.to_le_bytes());
        o.extend_from_slice(&self.index.to_le_bytes());
        o.extend_from_slice(&self.total.to_le_bytes());
        o.extend_from_slice(&self.body);
        o
    }
    pub fn decode(b: &[u8]) -> Option<Chunk> {
        if b.len() < 12 { return None; }
        Some(Chunk {
            msg: u64::from_le_bytes(b[0..8].try_into().ok()?),
            index: u16::from_le_bytes(b[8..10].try_into().ok()?),
            total: u16::from_le_bytes(b[10..12].try_into().ok()?),
            body: b[12..].to_vec(),
        })
    }
}

/// Split a packet into carrier-sized records.
///
/// `msg` must differ per packet from the same sender; a Lamport clock or a
/// counter both work. A packet that already fits still becomes one chunk, so
/// the receiving path has exactly one shape to handle rather than two.
pub fn split(packet: &Packet, msg: u64) -> Vec<Chunk> {
    let body = packet.encode();
    let per = MAX_CHUNK.saturating_sub(12).max(1);
    let total = body.len().div_ceil(per).max(1);
    (0..total)
        .map(|i| Chunk {
            msg,
            index: i as u16,
            total: total as u16,
            body: body[i * per..((i + 1) * per).min(body.len())].to_vec(),
        })
        .collect()
}

/// Reassembles chunks into packets, tolerating duplicates and reordering.
#[derive(Debug, Default)]
pub struct Reassembler {
    partial: std::collections::BTreeMap<u64, Vec<Option<Vec<u8>>>>,
}

impl Reassembler {
    /// Feed one chunk. Returns the packet once its last piece arrives.
    ///
    /// A duplicate is ignored rather than counted twice — the failure that
    /// would otherwise "complete" a message that is still missing a piece and
    /// hand up a truncated packet, which `Packet::decode` would then reject,
    /// losing the edit for good.
    pub fn feed(&mut self, c: Chunk) -> Option<Packet> {
        if c.total == 0 || c.index >= c.total { return None; }
        let slots = self.partial.entry(c.msg)
            .or_insert_with(|| vec![None; c.total as usize]);
        // A sender reusing a msg id with a different length is a bug or an
        // attack; drop the old attempt rather than indexing out of bounds.
        if slots.len() != c.total as usize {
            *slots = vec![None; c.total as usize];
        }
        if slots[c.index as usize].is_none() {
            slots[c.index as usize] = Some(c.body);
        }
        if slots.iter().any(|s| s.is_none()) { return None; }
        let body: Vec<u8> = slots.iter().flatten().flatten().copied().collect();
        self.partial.remove(&c.msg);
        Packet::decode(&body)
    }

    /// Messages still waiting on pieces — what a joiner would re-request.
    pub fn pending(&self) -> usize { self.partial.len() }

    /// Forget partial messages, for when a session is left or a peer is known
    /// gone. Without this, a sender that dies mid-packet leaks its slots for
    /// the life of the process.
    pub fn clear(&mut self) { self.partial.clear(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Doc, SiteId, Stamp, StampedOp};
    use studio_core::{Clip, ClipId, Op, Project, Source, SourceId, Ticks, Track, TrackId, TrackKind};

    fn big_catchup(n: u64) -> Packet {
        Packet::Catchup {
            ops: (0..n).map(|i| StampedOp {
                stamp: Stamp { lamport: i + 1, site: SiteId(1) },
                op: Op::AddClip { clip: Clip {
                    id: ClipId(i), source: SourceId(1), track: TrackId(1),
                    pos: Ticks::from_seconds(i as f64), src_in: Ticks::ZERO,
                    len: Ticks::from_seconds(1.0), gain: 1.0, effects: Vec::new() } },
            }).collect(),
        }
    }

    #[test]
    fn a_session_key_is_stable_and_session_specific() {
        assert_eq!(session_key("cut-42"), session_key("cut-42"), "both sides derive the same key");
        assert_ne!(session_key("cut-42"), session_key("cut-43"));
        // a short name still fills the key rather than leaving it mostly zero
        let k = session_key("a");
        assert!(k.iter().filter(|b| **b != 0).count() >= 16, "short names still spread: {k:?}");
    }

    #[test]
    fn a_small_packet_is_still_one_chunk() {
        // One shape for the receiver to handle, not two.
        let p = Packet::Hello { site: SiteId(1), since: 0 };
        let chunks = split(&p, 1);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].total, 1);
        let mut r = Reassembler::default();
        assert_eq!(r.feed(chunks[0].clone()), Some(p));
    }

    #[test]
    fn a_catchup_too_big_for_one_record_survives_being_split() {
        // The case that forces chunking to exist: a joiner's catch-up is tens
        // of kilobytes and a DHT value is a couple.
        let p = big_catchup(400);
        assert!(p.encode().len() > MAX_CHUNK * 4, "this really is a large packet");
        let chunks = split(&p, 7);
        assert!(chunks.len() > 4);
        assert!(chunks.iter().all(|c| c.encode().len() <= MAX_CHUNK), "every record fits the carrier");
        let mut r = Reassembler::default();
        let mut out = None;
        for c in &chunks { out = r.feed(c.clone()).or(out); }
        assert_eq!(out, Some(p), "and rebuilds exactly");
        assert_eq!(r.pending(), 0, "with nothing left over");
    }

    #[test]
    fn chunks_arriving_backwards_and_twice_still_rebuild() {
        // The carrier is unordered and lossy; this is its normal behaviour, not
        // an edge case.
        let p = big_catchup(120);
        let chunks = split(&p, 3);
        let mut r = Reassembler::default();
        let mut out = None;
        for c in chunks.iter().rev() { out = r.feed(c.clone()).or(out); }
        assert_eq!(out, Some(p.clone()), "reverse order is fine");

        // duplicates must not be counted twice — otherwise a message
        // "completes" while still missing a piece and hands up a truncated
        // packet, losing the edit for good
        let mut r = Reassembler::default();
        let n = chunks.len();
        for _ in 0..3 { assert_eq!(r.feed(chunks[0].clone()), None, "a repeat does not complete it"); }
        for c in &chunks[1..n - 1] { assert_eq!(r.feed(c.clone()), None); }
        assert_eq!(r.feed(chunks[n - 1].clone()), Some(p), "only the genuinely last piece completes it");
    }

    #[test]
    fn a_missing_chunk_leaves_the_message_pending_rather_than_corrupt() {
        let p = big_catchup(200);
        let chunks = split(&p, 9);
        let mut r = Reassembler::default();
        for c in chunks.iter().skip(1) {
            assert_eq!(r.feed(c.clone()), None, "incomplete must never hand up a packet");
        }
        assert_eq!(r.pending(), 1, "it is waiting, and a joiner can ask again");
        r.clear();
        assert_eq!(r.pending(), 0, "and a departed sender's slots can be released");
    }

    #[test]
    fn a_malformed_chunk_is_refused() {
        assert!(Chunk::decode(&[]).is_none());
        assert!(Chunk::decode(&[0u8; 11]).is_none(), "a short header is not a chunk");
        let c = Chunk { msg: 1, index: 0, total: 1, body: vec![1, 2, 3] };
        assert_eq!(Chunk::decode(&c.encode()), Some(c));
        // an index past the total is nonsense and must not index out of bounds
        let mut r = Reassembler::default();
        assert_eq!(r.feed(Chunk { msg: 1, index: 5, total: 2, body: vec![] }), None);
        assert_eq!(r.feed(Chunk { msg: 1, index: 0, total: 0, body: vec![] }), None);
    }

    #[test]
    fn a_whole_session_joins_through_the_carrier() {
        // End to end over the awkward carrier: chunked, shuffled, duplicated.
        let mut host = Doc::new(SiteId(1), Project { name: "cut".into(), ..Default::default() });
        host.local(Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video,
            name: "V1".into(), muted: false, locked: false }, at: 0 }).unwrap();
        host.local(Op::AddSource { source: Source { id: SourceId(1), path: "a.mov".into(),
            len: Ticks::from_seconds(600.0), sample_rate: 48_000, channels: 2, has_video: true } }).unwrap();
        for i in 0..150u64 {
            host.local(Op::AddClip { clip: Clip { id: ClipId(i), source: SourceId(1),
                track: TrackId(1), pos: Ticks::from_seconds(i as f64 * 2.0), src_in: Ticks::ZERO,
                len: Ticks::from_seconds(1.5), gain: 1.0, effects: Vec::new() } }).unwrap();
        }

        let mut joiner = Doc::new(SiteId(2), Project { name: "cut".into(), ..Default::default() });
        let answer = host.receive(joiner.hello()).expect("hello answered");
        let chunks = split(&answer, 1);
        assert!(chunks.len() > 1, "a real catch-up needs chunking");

        // deliver in a shuffled order with every third chunk repeated
        let mut r = Reassembler::default();
        let mut order: Vec<usize> = (0..chunks.len()).collect();
        order.reverse();
        for (n, i) in order.into_iter().enumerate() {
            if let Some(p) = r.feed(chunks[i].clone()) { joiner.receive(p); }
            if n % 3 == 0 {
                if let Some(p) = r.feed(chunks[i].clone()) { joiner.receive(p); }
            }
        }
        assert_eq!(joiner.project, host.project, "the joiner rebuilt the session over a lossy carrier");
    }
}
