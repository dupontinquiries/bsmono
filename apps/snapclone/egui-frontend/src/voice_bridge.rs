//! voice_bridge — `VoiceNetworkBridge.cpp`, ported.
//!
//! Sits between the group-voice coordinator and the transport: envelopes
//! outbound packets to every peer, and routes inbound ones back.
//!
//! ## The identity rule is the security property
//!
//! The C++ comment is the whole thing: *"The asserting peer IS the sender
//! (holder isn't embedded in the wire)."*
//!
//! A floor assert says *"I am speaking now"*, and if the claimant's identity
//! travelled inside the packet, any peer could assert on behalf of anyone —
//! taking the floor as someone else, or releasing theirs. Identity comes from
//! the **transport**, which authenticated it, and the wire carries only the
//! claim timestamp. That is not an optimisation and it does not survive being
//! "tidied" into a self-describing message.
//!
//! ## Three outcomes, not two
//!
//! `onPayloadFromPeer` returns false for *not ours*, and true for both *handled*
//! and *ours but malformed*. The distinction matters to the caller: a payload
//! that is not voice must be offered to other handlers, while a malformed voice
//! packet has been consumed and dropped — passing it on would have another
//! subsystem try to parse a packet already known to be broken.

/// What the wire carries.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Packet {
    /// Raw S16LE samples: what a build without a codec sends.
    VoiceFrame(Vec<u8>),
    /// One Opus packet (`voice_codec`). Its own kind, so a build that predates
    /// it drops it (`decode` answers None for a kind it does not know) instead
    /// of playing Opus bytes as samples, which would be noise.
    ///
    /// Carries the sender's sequence number and talk-spurt flag
    /// (`voice_send::Emit::Send`), which the receiver's jitter scheduler
    /// (`voice_playout`) orders and conceals by. Wire: kind 4, flags (bit 0 =
    /// spurt start), seq as u32 big-endian, payload.
    VoiceOpus { seq: u32, spurt_start: bool, data: Vec<u8> },
    /// A claim to the floor, carrying only its timestamp — see the module docs.
    FloorAssert { claim_ts: u64 },
    FloorRelease,
}

/// Magic prefix identifying a voice packet.
///
/// Checked before decoding so a non-voice payload costs one comparison rather
/// than a parse attempt — this runs on every packet from every peer.
pub const MAGIC: &[u8] = b"SCV1";

/// How an inbound payload was handled.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Inbound {
    /// Not a voice packet. The caller must offer it to other handlers.
    NotOurs,
    /// A voice packet, decoded. `from` is the TRANSPORT's identity.
    Handled { from: String, packet: Packet },
    /// A voice packet we could not decode. Consumed and dropped: passing it on
    /// would have another subsystem parse a packet already known broken.
    Malformed,
}

pub fn is_voice_packet(payload: &[u8]) -> bool { payload.starts_with(MAGIC) }

pub fn encode(p: &Packet) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    match p {
        Packet::VoiceFrame(f) => { out.push(1); out.extend_from_slice(f); }
        Packet::FloorAssert { claim_ts } => {
            out.push(2);
            out.extend_from_slice(&claim_ts.to_be_bytes());
        }
        Packet::FloorRelease => out.push(3),
        Packet::VoiceOpus { seq, spurt_start, data } => {
            out.push(4);
            out.push(if *spurt_start { 1 } else { 0 });
            out.extend_from_slice(&seq.to_be_bytes());
            out.extend_from_slice(data);
        }
    }
    out
}

pub fn decode(payload: &[u8]) -> Option<Packet> {
    if !is_voice_packet(payload) || payload.len() < MAGIC.len() + 1 { return None; }
    let body = &payload[MAGIC.len()..];
    match body[0] {
        1 => Some(Packet::VoiceFrame(body[1..].to_vec())),
        2 => {
            // Length checked before slicing: a truncated assert would otherwise
            // panic on a packet any peer can send.
            if body.len() < 9 { return None; }
            let mut b = [0u8; 8];
            b.copy_from_slice(&body[1..9]);
            Some(Packet::FloorAssert { claim_ts: u64::from_be_bytes(b) })
        }
        3 => Some(Packet::FloorRelease),
        4 => {
            // Length checked before slicing, as for the floor assert.
            if body.len() < 6 { return None; }
            let seq = u32::from_be_bytes([body[2], body[3], body[4], body[5]]);
            Some(Packet::VoiceOpus { seq, spurt_start: body[1] & 1 != 0, data: body[6..].to_vec() })
        }
        _ => None,
    }
}

/// The bridge.
#[derive(Clone, Debug, Default)]
pub struct VoiceBridge {
    peers: Vec<String>,
    /// False until a coordinator is attached; without one there is nothing to
    /// route to, and inbound packets are still CONSUMED rather than passed on.
    connected: bool,
}

impl VoiceBridge {
    pub fn set_peers(&mut self, peers: Vec<String>) { self.peers = peers; }
    pub fn peers(&self) -> &[String] { &self.peers }
    pub fn set_connected(&mut self, on: bool) { self.connected = on; }

    /// Envelope an outbound packet for every peer.
    ///
    /// Returns `(peer, bytes)` pairs. One encode, many sends — encoding per
    /// peer would multiply the cost of a voice frame by the group size at
    /// 50 packets a second.
    pub fn outbound(&self, p: &Packet) -> Vec<(String, Vec<u8>)> {
        if self.peers.is_empty() { return Vec::new(); }
        let bytes = encode(p);
        self.peers.iter().map(|peer| (peer.clone(), bytes.clone())).collect()
    }

    /// Route an inbound payload.
    ///
    /// `sender` comes from the transport and is the ONLY source of identity —
    /// see the module docs.
    pub fn inbound(&self, sender: &str, payload: &[u8]) -> Inbound {
        if !is_voice_packet(payload) { return Inbound::NotOurs; }
        if !self.connected { return Inbound::Malformed; }
        match decode(payload) {
            Some(packet) => Inbound::Handled { from: sender.to_string(), packet },
            None => Inbound::Malformed,
        }
    }

    /// Route a batch buffered before the coordinator existed.
    ///
    /// Non-voice payloads are skipped rather than reported: a preroll buffer
    /// holds whatever arrived, and the caller has no handler to offer them to
    /// by the time it flushes.
    pub fn flush_preroll(&self, sender: &str, payloads: &[Vec<u8>]) -> Vec<Packet> {
        payloads.iter()
            .filter(|p| is_voice_packet(p))
            .filter_map(|p| decode(p))
            .map(|p| { let _ = sender; p })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge() -> VoiceBridge {
        let mut b = VoiceBridge::default();
        b.set_peers(vec!["sc-alice".into(), "sc-bob".into()]);
        b.set_connected(true);
        b
    }

    #[test]
    fn a_floor_assert_carries_no_identity_on_the_wire() {
        // The security property: if the claimant's identity travelled inside
        // the packet, any peer could assert on behalf of anyone — taking the
        // floor as someone else, or releasing theirs.
        let bytes = encode(&Packet::FloorAssert { claim_ts: 1234 });
        assert!(!bytes.windows(8).any(|w| w == b"sc-alice"),
                "no identity in the packet: {bytes:?}");
        assert_eq!(bytes.len(), MAGIC.len() + 1 + 8, "magic, kind, timestamp — nothing else");
    }

    #[test]
    fn the_asserting_peer_is_the_transport_sender() {
        let b = bridge();
        let pkt = encode(&Packet::FloorAssert { claim_ts: 99 });
        match b.inbound("sc-carol", &pkt) {
            Inbound::Handled { from, packet } => {
                assert_eq!(from, "sc-carol", "identity came from the transport");
                assert_eq!(packet, Packet::FloorAssert { claim_ts: 99 });
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_non_voice_payload_is_left_for_other_handlers() {
        // NotOurs rather than dropped: the transport multiplexes, and consuming
        // somebody else's packet makes their feature stop working with no error
        // in either subsystem.
        let b = bridge();
        assert_eq!(b.inbound("sc-alice", b"SNAPmediablob"), Inbound::NotOurs);
        assert_eq!(b.inbound("sc-alice", b""), Inbound::NotOurs);
    }

    #[test]
    fn a_malformed_voice_packet_is_consumed_rather_than_passed_on() {
        // The third outcome. Passing it on would have another subsystem parse a
        // packet already known to be broken.
        let b = bridge();
        let mut bad = MAGIC.to_vec();
        bad.push(99);                       // unknown kind
        assert_eq!(b.inbound("sc-alice", &bad), Inbound::Malformed);
    }

    #[test]
    fn a_truncated_floor_assert_does_not_panic() {
        // Any peer can send this, so a slice past the end is a remote crash.
        let b = bridge();
        let mut short = MAGIC.to_vec();
        short.push(2);
        short.extend_from_slice(&[0, 0, 0]);   // three bytes of an eight-byte ts
        assert_eq!(b.inbound("sc-alice", &short), Inbound::Malformed);
    }

    #[test]
    fn a_packet_with_only_the_magic_is_malformed_not_a_frame() {
        let b = bridge();
        assert_eq!(b.inbound("sc-alice", MAGIC), Inbound::Malformed);
    }

    #[test]
    fn every_packet_kind_round_trips() {
        for p in [Packet::VoiceFrame(vec![1, 2, 3, 4]), Packet::VoiceOpus { seq: 70_000, spurt_start: true, data: vec![0x78, 9, 9] },
                  Packet::VoiceOpus { seq: 3, spurt_start: false, data: vec![] },
                  Packet::FloorAssert { claim_ts: u64::MAX },
                  Packet::FloorRelease] {
            assert_eq!(decode(&encode(&p)), Some(p));
        }
    }

    #[test]
    fn an_empty_voice_frame_round_trips_as_an_empty_frame() {
        // A codec emitting silence is ordinary, and treating it as malformed
        // would drop the packets that keep a stream alive.
        assert_eq!(decode(&encode(&Packet::VoiceFrame(vec![]))),
                   Some(Packet::VoiceFrame(vec![])));
    }

    #[test]
    fn one_encode_serves_every_peer() {
        // Encoding per peer multiplies the cost of a voice frame by the group
        // size at fifty packets a second.
        let b = bridge();
        let out = b.outbound(&Packet::VoiceFrame(vec![7; 160]));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].1, out[1].1, "identical bytes");
        assert_eq!(out[0].0, "sc-alice");
        assert_eq!(out[1].0, "sc-bob");
    }

    #[test]
    fn no_peers_means_no_work_at_all() {
        // A solo call still runs the coordinator, and encoding for nobody is
        // pure battery at fifty packets a second.
        let mut b = VoiceBridge::default();
        b.set_connected(true);
        assert!(b.outbound(&Packet::VoiceFrame(vec![1; 160])).is_empty());
    }

    #[test]
    fn a_voice_packet_arriving_before_the_coordinator_is_still_consumed() {
        // It is ours; offering it to other handlers would have them parse voice
        // audio as their own protocol.
        let mut b = bridge();
        b.set_connected(false);
        let pkt = encode(&Packet::FloorRelease);
        assert_eq!(b.inbound("sc-alice", &pkt), Inbound::Malformed);
        assert_eq!(b.inbound("sc-alice", b"SNAPblob"), Inbound::NotOurs,
                   "but a foreign packet is still foreign");
    }

    #[test]
    fn preroll_skips_foreign_payloads_and_keeps_order() {
        // A preroll buffer holds whatever arrived, and by flush time there is
        // no handler left to offer the others to.
        let b = bridge();
        let payloads = vec![
            encode(&Packet::FloorAssert { claim_ts: 1 }),
            b"SNAPblob".to_vec(),
            encode(&Packet::VoiceFrame(vec![9])),
        ];
        let got = b.flush_preroll("sc-alice", &payloads);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], Packet::FloorAssert { claim_ts: 1 }, "order preserved");
    }

    #[test]
    fn a_malformed_payload_in_preroll_does_not_stop_the_flush() {
        // One bad packet in a buffer must not discard the ones after it.
        let b = bridge();
        let mut bad = MAGIC.to_vec();
        bad.push(99);
        let payloads = vec![bad, encode(&Packet::FloorRelease)];
        assert_eq!(b.flush_preroll("sc-alice", &payloads), vec![Packet::FloorRelease]);
    }
}
