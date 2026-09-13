//! wire.rs — what a datagram on the punch socket IS, and whether we should be
//! talking to its sender at all. Pure functions, no I/O.
//!
//! Ported from `NatTraversal.cpp`: the demux at the top of `onPunchReadyRead()`
//! and the guards in `initiateHolePunch()`. Kept separate from the socket
//! code so that every decision here is exhaustively unit-testable — this is
//! where the C++ accumulated its hard-won `BUILD-MARKER: ...-LOOPBACK-DROP-V2`
//! fixes, each of which was a real infinite feedback loop in the field
//! (thousands of packets/sec, no backoff, no end condition).
//!
//! # One socket, three things on it
//!
//! The C++ design binds ONE UDP socket and does STUN, hole-punching, and
//! the SCUD data channel all through it — deliberately, so the reflexive
//! address STUN discovers is the one punch packets actually leave from,
//! and so there is no `SO_REUSEPORT` delivery ambiguity between two sockets
//! on the same port. The cost is that inbound bytes have to be told apart
//! by content, which is what [`classify`] does.

use std::net::IpAddr;

/// Every punch packet starts with this, then a newline, then the sender's
/// sc-ID. The sc-ID lets the receiver confirm WHO punched (and lets it
/// drop its own reflected packets — see [`SelfGuard`]).
pub const PUNCH_MAGIC: &[u8] = b"SNAPCLONE_PUNCH_V1";

/// Prefix of a SCUD frame (`UdpChannel`'s data channel, which shares the
/// punch socket via callback rather than binding its own).
pub const SCUD_MAGIC: &[u8] = b"SCUD";

/// STUN Binding Response message type (RFC 5389).
const STUN_BINDING_RESPONSE: u16 = 0x0101;

/// What an inbound datagram on the punch socket turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inbound<'a> {
    /// A STUN Binding Response — `tx_id` is the 12-byte transaction id from
    /// the header, for the caller to match against what it sent (the C++
    /// accepts the current id OR any recent one, since a response can arrive
    /// after `restartStunOnly()` has already replaced the current id).
    StunResponse { tx_id: [u8; 12] },
    /// A SCUD data frame, to hand to the UDP channel untouched.
    Scud(&'a [u8]),
    /// A punch from a peer. `sender_id` is the sc-ID after the newline, or
    /// empty if the packet carried none.
    Punch { sender_id: &'a str },
    /// None of the above. The C++ logs and ignores these.
    Other,
}

/// Tell an inbound datagram's kind apart by its bytes.
pub fn classify(data: &[u8]) -> Inbound<'_> {
    // STUN first: a 20+ byte packet whose type field is Binding Response.
    // Checked before the magic prefixes because a STUN response could, in
    // principle, begin with anything — but its 2-byte type is definitive.
    if data.len() >= 20 {
        let msg_type = u16::from_be_bytes([data[0], data[1]]);
        if msg_type == STUN_BINDING_RESPONSE {
            let mut tx_id = [0u8; 12];
            tx_id.copy_from_slice(&data[8..20]);
            return Inbound::StunResponse { tx_id };
        }
    }
    if data.starts_with(SCUD_MAGIC) {
        return Inbound::Scud(data);
    }
    if data.starts_with(PUNCH_MAGIC) {
        let sender_id = data
            .iter()
            .position(|&b| b == b'\n')
            .and_then(|nl| std::str::from_utf8(&data[nl + 1..]).ok())
            .unwrap_or("");
        return Inbound::Punch { sender_id };
    }
    Inbound::Other
}

/// Build the punch payload: `SNAPCLONE_PUNCH_V1\n<self_id>`. The same
/// bytes are used for the initial burst, maintenance punches, and the reply
/// a receiver sends back to confirm the pinhole.
pub fn punch_payload(self_id: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(PUNCH_MAGIC.len() + 1 + self_id.len());
    v.extend_from_slice(PUNCH_MAGIC);
    v.push(b'\n');
    v.extend_from_slice(self_id.as_bytes());
    v
}

/// Is this an address that means "us"? Loopback in either family, including
/// v4-mapped-v6 forms — the kernel routes a punch at any of these straight
/// back to the sender, and the C++ found out the hard way that without this
/// check it replies to its own reply, forever.
pub fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

/// Why a punch target (or an inbound punch's sender) was refused as self.
/// Ported from the three checks in `initiateHolePunch()` and the two in
/// `onPunchReadyRead()`. The C++ has these in two places — send side and
/// receive side — because together they make the loop "literally
/// impossible" rather than merely unlikely; both call this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfGuard {
    /// 127.x / ::1 / ::ffff:127.x.
    Loopback,
    /// The peer's sc-ID is our own.
    SelfId,
    /// The peer's host is our own reflexive (public) IP. Two devices behind
    /// the same CGNAT report the same reflexive IP, so a peer's connect
    /// request can hand us our own public address as the punch target; the
    /// packet hairpins back as loopback and the receive-side guard drops it
    /// on every maintenance punch, forever. Refusing here stops it at the
    /// source. Host-only compare: the port may legitimately differ.
    SelfReflexive,
}

/// Should we punch this target / accept this sender? `None` means go ahead.
///
/// `self_reflexive` is our own reflexive IP if STUN has found one; the
/// receive-side call passes `None` for it because the C++ receive path only
/// checks loopback and self-ID (an inbound punch from our own public IP is
/// already covered by the send-side refusal that stops us ever sending it).
pub fn self_guard(
    peer_ip: IpAddr,
    peer_id: &str,
    self_id: &str,
    self_reflexive: Option<IpAddr>,
) -> Option<SelfGuard> {
    if is_loopback(peer_ip) {
        return Some(SelfGuard::Loopback);
    }
    if !peer_id.is_empty() && peer_id == self_id {
        return Some(SelfGuard::SelfId);
    }
    if let Some(mine) = self_reflexive {
        if peer_ip == mine {
            return Some(SelfGuard::SelfReflexive);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn stun_response_bytes() -> Vec<u8> {
        let mut v = vec![0u8; 20];
        v[0] = 0x01;
        v[1] = 0x01; // Binding Response
        v[4..8].copy_from_slice(&0x2112A442u32.to_be_bytes());
        for (i, b) in v[8..20].iter_mut().enumerate() {
            *b = i as u8 + 1;
        }
        v
    }

    #[test]
    fn a_stun_response_is_recognised_by_type_and_yields_its_tx_id() {
        let pkt = stun_response_bytes();
        match classify(&pkt) {
            Inbound::StunResponse { tx_id } => assert_eq!(tx_id, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]),
            other => panic!("expected StunResponse, got {other:?}"),
        }
    }

    #[test]
    fn a_short_packet_can_never_be_mistaken_for_stun() {
        // 0x0101 leading bytes but under 20 long: the C++ requires size >= 20
        // before even reading the type, and so do we.
        let pkt = [0x01u8, 0x01, 0, 0];
        assert_eq!(classify(&pkt), Inbound::Other);
    }

    #[test]
    fn punch_payload_round_trips_through_classify() {
        let pkt = punch_payload("sc-abcdef01");
        assert_eq!(classify(&pkt), Inbound::Punch { sender_id: "sc-abcdef01" });
    }

    #[test]
    fn a_punch_with_no_id_is_still_a_punch_with_an_empty_id() {
        // Older peers, or a truncated packet: the C++ falls back to resolving
        // the sender by IP in that case rather than dropping it.
        assert_eq!(classify(PUNCH_MAGIC), Inbound::Punch { sender_id: "" });
        let mut with_nl = PUNCH_MAGIC.to_vec();
        with_nl.push(b'\n');
        assert_eq!(classify(&with_nl), Inbound::Punch { sender_id: "" });
    }

    #[test]
    fn scud_frames_are_handed_over_untouched() {
        let frame = b"SCUD\x00\x01payload";
        assert_eq!(classify(frame), Inbound::Scud(frame));
    }

    #[test]
    fn loopback_is_caught_in_every_form_the_cpp_checks() {
        assert!(is_loopback(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(is_loopback(IpAddr::V4(Ipv4Addr::new(127, 42, 0, 1))));
        assert!(is_loopback(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(is_loopback(IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped())));
        assert!(!is_loopback(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10))));
        assert!(!is_loopback(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
    }

    #[test]
    fn the_three_self_guards_fire_in_the_cpp_order() {
        let pub_ip = IpAddr::V4(Ipv4Addr::new(69, 156, 66, 68));
        let lan = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10));

        assert_eq!(self_guard(IpAddr::V4(Ipv4Addr::LOCALHOST), "sc-them", "sc-me", Some(pub_ip)), Some(SelfGuard::Loopback));
        assert_eq!(self_guard(lan, "sc-me", "sc-me", Some(pub_ip)), Some(SelfGuard::SelfId));
        // Our own public host, even on a different port, even with the
        // peer's real sc-ID — the same-CGNAT case.
        assert_eq!(self_guard(pub_ip, "sc-them", "sc-me", Some(pub_ip)), Some(SelfGuard::SelfReflexive));
        assert_eq!(self_guard(lan, "sc-them", "sc-me", Some(pub_ip)), None);
    }

    #[test]
    fn an_empty_peer_id_never_matches_self_id() {
        // A punch carrying no id must not be dropped as "self" just because
        // both strings are empty on a device that hasn't loaded its identity.
        let lan = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10));
        assert_eq!(self_guard(lan, "", "", None), None);
    }

    #[test]
    fn self_reflexive_is_only_checked_when_known() {
        let pub_ip = IpAddr::V4(Ipv4Addr::new(69, 156, 66, 68));
        assert_eq!(self_guard(pub_ip, "sc-them", "sc-me", None), None);
    }
}
