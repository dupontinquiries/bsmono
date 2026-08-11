//! nat_punch — the traversal strategy from `NatTraversal`.
//!
//! Whether two phones on different networks can reach each other at all. Four
//! steps, and the first one is a correction the source calls out explicitly.
//!
//! ## STUN must run on the PUNCH socket, not an ephemeral one
//!
//! *"Send a STUN binding request FROM m_punchSocket... The STUN response gives
//! us our TRUE reflexive address for that socket: the IP:port the NAT assigned
//! for UDP from port 9876. **It is NOT the same as the ephemeral STUN socket
//! port used previously.**"*
//!
//! That is the bug the whole file is a fix for. A NAT assigns mappings **per
//! socket**, so discovering your public address from a throwaway socket tells
//! you the port for *that* socket — and announcing it sends every peer's packets
//! to a mapping that closes the moment the throwaway socket does. The address is
//! right, the port is useless, and the failure looks like "hole punching does
//! not work" rather than "we measured the wrong socket".
//!
//! ## Why TCP works afterwards, and when it does not
//!
//! *"For port-preserving NATs (most home routers), the TCP SYN now passes
//! because the NAT already has a mapping for that addr:port pair from UDP. For
//! non-port-preserving NATs we may need the punched UDP port for TCP too — but
//! that requires SO_REUSEPORT TCP which Qt doesn't expose. TURN is the fallback
//! for those (symmetric NAT / carrier-grade NAT)."*
//!
//! An honest limit rather than a claim of universal traversal: symmetric and
//! CGNAT need a relay, and the code says so instead of retrying forever.
//!
//! ## The burst window exists to absorb DHT propagation
//!
//! *"The 2-second window (8 × 250ms packets) gives enough overlap even with a
//! few seconds of DHT propagation delay."* Simultaneous punching cannot be
//! simultaneous — the two sides learn each other's endpoints at different
//! moments — so the window is what makes "simultaneous" achievable in practice.

/// The port both UDP punching and TCP use, shared so a port-preserving NAT's
/// UDP mapping is the one TCP arrives on.
pub const PUNCH_PORT: u16 = 9876;

/// STUN wire constants (RFC 5389).
pub const STUN_MAGIC_COOKIE: u32 = 0x2112_A442;
pub const STUN_BINDING_REQUEST: u16 = 0x0001;
pub const STUN_BINDING_RESPONSE: u16 = 0x0101;
pub const ATTR_MAPPED_ADDRESS: u16 = 0x0001;
pub const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;

/// Punch burst: 8 packets at 250 ms is a 2 s window.
pub const PUNCH_INTERVAL_MS: u64 = 250;
pub const PUNCH_COUNT: u32 = 8;

pub fn punch_window_ms() -> u64 { PUNCH_INTERVAL_MS * PUNCH_COUNT as u64 }

/// What kind of NAT we appear to be behind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NatKind {
    /// No translation — the reflexive address equals the local one.
    Open,
    /// The mapping is stable across destinations: punching works.
    ConeLike,
    /// A different mapping per destination: punching cannot work, relay needed.
    Symmetric,
    /// Not enough observations yet.
    Unknown,
}

impl NatKind {
    /// Can hole punching succeed?
    ///
    /// Symmetric is a firm no rather than a "try anyway": the mapping the peer
    /// was told about is not the one their packets will arrive on, so every
    /// attempt fails identically and retrying only spends radio.
    pub fn can_punch(self) -> bool { matches!(self, NatKind::Open | NatKind::ConeLike) }

    /// Do we need a relay?
    pub fn needs_relay(self) -> bool { self == NatKind::Symmetric }
}

/// A reflexive address as STUN reported it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Reflexive {
    pub ip: String,
    pub port: u16,
    /// The local port the request went out from — the thing that makes this
    /// address meaningful or useless.
    pub from_local_port: u16,
}

impl Reflexive {
    /// Is this address usable as a punch endpoint?
    ///
    /// Only if it was measured from the socket that will actually receive the
    /// punch. A reflexive address from a throwaway socket names a mapping that
    /// closes with that socket.
    pub fn is_punchable(&self, punch_socket_port: u16) -> bool {
        self.from_local_port == punch_socket_port && self.port != 0
    }
}

/// Classify from two observations taken to different STUN servers.
///
/// Two servers, because one cannot distinguish symmetric from cone: a single
/// observation is consistent with both, and guessing cone means every symmetric
/// user gets a punch that silently never connects.
pub fn classify(local_port: u16, a: &Reflexive, b: &Reflexive) -> NatKind {
    if a.port == 0 || b.port == 0 { return NatKind::Unknown; }
    if a.port != b.port || a.ip != b.ip { return NatKind::Symmetric; }
    if a.port == local_port { return NatKind::Open; }
    NatKind::ConeLike
}

/// How the connection should be attempted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Strategy {
    /// Straight to TCP — nothing in the way.
    DirectTcp,
    /// Punch UDP first, then TCP on the same port.
    PunchThenTcp,
    /// Relay. Not a failure, just the honest answer for symmetric/CGNAT.
    Relay,
    /// Not enough information yet.
    Wait,
}

/// Pick a strategy from what we know about both ends.
///
/// Either side being symmetric forces a relay: punching needs both mappings to
/// be predictable, and one unpredictable end is enough to break it.
pub fn strategy(ours: NatKind, theirs: NatKind) -> Strategy {
    match (ours, theirs) {
        (NatKind::Unknown, _) | (_, NatKind::Unknown) => Strategy::Wait,
        (NatKind::Symmetric, _) | (_, NatKind::Symmetric) => Strategy::Relay,
        (NatKind::Open, NatKind::Open) => Strategy::DirectTcp,
        _ => Strategy::PunchThenTcp,
    }
}

/// Parse the XOR-MAPPED-ADDRESS port from a STUN response.
///
/// XOR'd with the magic cookie's top half. The XOR exists because some NATs
/// rewrite anything that looks like an IP address in a payload — a plain
/// MAPPED-ADDRESS can arrive mangled, which is why the XOR form is preferred
/// when both are present.
pub fn decode_xor_port(xor_port: u16) -> u16 {
    xor_port ^ (STUN_MAGIC_COOKIE >> 16) as u16
}

pub fn encode_xor_port(port: u16) -> u16 { decode_xor_port(port) }

/// Does a buffer look like a STUN response to our request?
///
/// The magic cookie is checked because this socket also carries punch packets
/// and application data: without it, a punch packet whose first bytes happen to
/// match a message type would be parsed as an address.
pub fn is_stun_response(buf: &[u8]) -> bool {
    if buf.len() < 20 { return false; }
    let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
    let cookie = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    msg_type == STUN_BINDING_RESPONSE && cookie == STUN_MAGIC_COOKIE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refl(ip: &str, port: u16, from: u16) -> Reflexive {
        Reflexive { ip: ip.into(), port, from_local_port: from }
    }

    #[test]
    fn a_reflexive_address_from_the_wrong_socket_is_not_punchable() {
        // The bug the whole file is a fix for. A NAT assigns mappings PER
        // SOCKET, so an address discovered from a throwaway socket names a
        // mapping that closes with that socket — the address is right, the port
        // is useless, and it looks like "hole punching does not work".
        let ephemeral = refl("203.0.113.9", 54_321, 54_321);
        assert!(!ephemeral.is_punchable(PUNCH_PORT));
        let correct = refl("203.0.113.9", 41_002, PUNCH_PORT);
        assert!(correct.is_punchable(PUNCH_PORT));
    }

    #[test]
    fn a_zero_reflexive_port_is_never_punchable() {
        // A failed or truncated STUN parse must not be announced as an
        // endpoint; peers would punch at port 0 forever.
        assert!(!refl("203.0.113.9", 0, PUNCH_PORT).is_punchable(PUNCH_PORT));
    }

    #[test]
    fn symmetric_nat_is_detected_from_two_servers_not_one() {
        // One observation is consistent with both cone and symmetric, and
        // guessing cone means every symmetric user gets a punch that silently
        // never connects.
        let a = refl("203.0.113.9", 41_002, PUNCH_PORT);
        let differs = refl("203.0.113.9", 41_919, PUNCH_PORT);
        assert_eq!(classify(PUNCH_PORT, &a, &differs), NatKind::Symmetric);
        let same = refl("203.0.113.9", 41_002, PUNCH_PORT);
        assert_eq!(classify(PUNCH_PORT, &a, &same), NatKind::ConeLike);
    }

    #[test]
    fn an_unmapped_port_reads_as_an_open_network() {
        // The reflexive port equalling the local one means nothing translated.
        let a = refl("203.0.113.9", PUNCH_PORT, PUNCH_PORT);
        assert_eq!(classify(PUNCH_PORT, &a, &a), NatKind::Open);
    }

    #[test]
    fn a_changed_reflexive_ip_is_symmetric_too() {
        // Multi-homed CGNAT hands out different addresses per destination, and
        // only the port changing is not the only symmetric shape.
        let a = refl("203.0.113.9", 41_002, PUNCH_PORT);
        let b = refl("198.51.100.4", 41_002, PUNCH_PORT);
        assert_eq!(classify(PUNCH_PORT, &a, &b), NatKind::Symmetric);
    }

    #[test]
    fn an_incomplete_observation_is_unknown_rather_than_a_guess() {
        // Announcing a punch endpoint we are not sure about wastes both sides'
        // 2-second window.
        let good = refl("203.0.113.9", 41_002, PUNCH_PORT);
        let bad = refl("", 0, PUNCH_PORT);
        assert_eq!(classify(PUNCH_PORT, &good, &bad), NatKind::Unknown);
        assert_eq!(strategy(NatKind::Unknown, NatKind::ConeLike), Strategy::Wait);
    }

    #[test]
    fn symmetric_on_either_side_forces_a_relay() {
        // Punching needs BOTH mappings predictable; one unpredictable end
        // breaks it. This is the honest limit the source states rather than a
        // claim of universal traversal.
        assert_eq!(strategy(NatKind::Symmetric, NatKind::ConeLike), Strategy::Relay);
        assert_eq!(strategy(NatKind::ConeLike, NatKind::Symmetric), Strategy::Relay);
        assert!(NatKind::Symmetric.needs_relay());
        assert!(!NatKind::Symmetric.can_punch());
    }

    #[test]
    fn two_open_networks_skip_the_punch_entirely() {
        // Spending a 2-second punch window on a connection that would have
        // succeeded immediately is two seconds of radio for nothing.
        assert_eq!(strategy(NatKind::Open, NatKind::Open), Strategy::DirectTcp);
    }

    #[test]
    fn one_open_end_still_punches() {
        // The cone side's mapping still has to be opened by an outbound packet
        // before the open side's TCP SYN arrives.
        assert_eq!(strategy(NatKind::Open, NatKind::ConeLike), Strategy::PunchThenTcp);
        assert_eq!(strategy(NatKind::ConeLike, NatKind::Open), Strategy::PunchThenTcp);
    }

    #[test]
    fn the_burst_window_covers_dht_propagation() {
        // "The 2-second window (8 x 250ms packets) gives enough overlap even
        // with a few seconds of DHT propagation delay." Simultaneous punching
        // cannot be simultaneous — the sides learn each other's endpoints at
        // different moments — so the window is what makes it work in practice.
        assert_eq!(punch_window_ms(), 2_000);
        assert_eq!(PUNCH_COUNT, 8);
        assert_eq!(PUNCH_INTERVAL_MS, 250);
    }

    #[test]
    fn the_xor_port_round_trips() {
        // XOR-MAPPED exists because some NATs rewrite anything resembling an IP
        // in a payload, so a plain MAPPED-ADDRESS can arrive mangled.
        for p in [9876u16, 41_002, 1, 65_535] {
            assert_eq!(decode_xor_port(encode_xor_port(p)), p, "port {p}");
        }
        // and it is genuinely obfuscated on the wire
        assert_ne!(encode_xor_port(9876), 9876);
    }

    #[test]
    fn a_punch_packet_is_not_mistaken_for_a_stun_response() {
        // The punch socket carries STUN, punch packets and application data.
        // Without the cookie check, a punch packet whose first bytes happen to
        // match a message type would be parsed as an address.
        let mut resp = vec![0u8; 20];
        resp[0..2].copy_from_slice(&STUN_BINDING_RESPONSE.to_be_bytes());
        resp[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
        assert!(is_stun_response(&resp));

        let mut impostor = resp.clone();
        impostor[4..8].copy_from_slice(&0xDEAD_BEEFu32.to_be_bytes());
        assert!(!is_stun_response(&impostor), "accepted a packet with no magic cookie");

        assert!(!is_stun_response(b"punch"), "a short packet is not a response");
    }

    #[test]
    fn a_binding_request_is_not_a_response() {
        // Both peers punch, so requests arrive on this socket too; treating one
        // as a response would read our own address out of a peer's packet.
        let mut req = vec![0u8; 20];
        req[0..2].copy_from_slice(&STUN_BINDING_REQUEST.to_be_bytes());
        req[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
        assert!(!is_stun_response(&req));
    }

    #[test]
    fn tcp_reuses_the_punched_port() {
        // "For port-preserving NATs the TCP SYN now passes because the NAT
        // already has a mapping for that addr:port pair from UDP." That only
        // holds if both use the same port.
        assert_eq!(PUNCH_PORT, 9876);
    }
}
