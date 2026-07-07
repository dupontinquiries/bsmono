//! `sim::net_candidates` — which IP to punch / relay for world-join (WID r58-07).
//!
//! First concrete module of the multiplayer world-join port (spec
//! `handoff/MULTIPLAYER_WORLD_JOIN.md` §5; port of snapclone
//! `connreq_targets::order_targets` + a trimmed `TransportSelector`). Pure
//! candidate-ranking + transport-decision logic: the bin does OpenDHT/UDP/TCP;
//! this only decides *what* to try and *in what order*.
//!
//! The snapclone bug it fixes: two phones on one WiFi each knew only the other's
//! shared public IP; both correctly skip the hairpin (a public candidate whose
//! host is our own public IP — punching the router's external face fails) and
//! were then left with nothing, so messages hung forever. The fix is to include
//! LAN addresses (tried first) and, when ordering is empty, fall straight to
//! relay/mailbox instead of reporting failure.
//!
//! Dependency-free plain types (no serde/chrono, IPv4 octets not `String` —
//! engine sim is alloc-light). Pure: no `crate::render`, no GPU, no sockets/DHT,
//! no clock, no RNG, no `.unwrap()`/`.expect()` in non-test code. Release-safe.

/// Where a candidate address lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandKind {
    /// RFC1918 / same-subnet — fastest, dodges NAT.
    Lan,
    /// Public reflexive address.
    Public,
}

/// A connection candidate (IPv4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub host: [u8; 4],
    pub port: u16,
    pub kind: CandKind,
}

/// Transport method to attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    UdpPunch,
    DirectTcp,
    Relay,
    Mailbox,
}

/// Order the candidate set to try: **LAN first**, then public, dropping any
/// public hairpin (host == `self_public`) and de-duping identical `host:port`.
/// (Port of snapclone `order_targets`.)
pub fn order_targets(cands: &[Candidate], self_public: [u8; 4]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut seen: Vec<([u8; 4], u16)> = Vec::new();

    let push_unique = |c: Candidate, out: &mut Vec<Candidate>, seen: &mut Vec<([u8; 4], u16)>| {
        let key = (c.host, c.port);
        if !seen.contains(&key) {
            seen.push(key);
            out.push(c);
        }
    };

    // LAN candidates first (in input order), de-duped.
    for c in cands {
        if c.kind == CandKind::Lan {
            push_unique(*c, &mut out, &mut seen);
        }
    }
    // then public, skipping the hairpin (our own public IP).
    for c in cands {
        if c.kind == CandKind::Public && c.host != self_public {
            push_unique(*c, &mut out, &mut seen);
        }
    }
    out
}

/// True when [`order_targets`] yields nothing (e.g. the only candidate was the
/// hairpin) ⇒ the caller should fall straight to relay/mailbox, not report failure.
pub fn is_dead_end(cands: &[Candidate], self_public: [u8; 4]) -> bool {
    order_targets(cands, self_public).is_empty()
}

/// Choose transport methods in priority order (trimmed `TransportSelector`):
/// * no target ⇒ `[Mailbox]` (nothing direct to try — wait for the peer),
/// * CGNAT ⇒ `[Mailbox, Relay?]` (skip direct punching entirely),
/// * otherwise ⇒ `[UdpPunch, DirectTcp, Relay?, Mailbox]`.
///
/// `Relay` is included only when `has_relay`. Mailbox is always the final fallback.
pub fn transport_order(has_target: bool, is_cgnat: bool, has_relay: bool) -> Vec<Transport> {
    let mut out = Vec::new();
    if !has_target {
        out.push(Transport::Mailbox); // nothing to connect to directly
        return out;
    }
    if is_cgnat {
        out.push(Transport::Mailbox);
        if has_relay {
            out.push(Transport::Relay);
        }
        return out;
    }
    out.push(Transport::UdpPunch);
    out.push(Transport::DirectTcp);
    if has_relay {
        out.push(Transport::Relay);
    }
    out.push(Transport::Mailbox);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lan(h: [u8; 4], p: u16) -> Candidate {
        Candidate { host: h, port: p, kind: CandKind::Lan }
    }
    fn public(h: [u8; 4], p: u16) -> Candidate {
        Candidate { host: h, port: p, kind: CandKind::Public }
    }

    #[test]
    fn snapclone_bug_only_hairpin_is_dead_end() {
        let me = [203, 0, 113, 7];
        let cands = [public(me, 9000)]; // only candidate is our own shared public IP
        assert!(order_targets(&cands, me).is_empty(), "hairpin dropped ⇒ empty");
        assert!(is_dead_end(&cands, me), "dead-end ⇒ fall to relay/mailbox");
    }

    #[test]
    fn lan_candidate_rescues_delivery() {
        let me = [203, 0, 113, 7];
        let cands = [public(me, 9000), lan([192, 168, 1, 22], 7777)];
        let ordered = order_targets(&cands, me);
        assert_eq!(ordered.len(), 1, "hairpin dropped, LAN kept");
        assert_eq!(ordered[0].kind, CandKind::Lan);
        assert_eq!(ordered[0].host, [192, 168, 1, 22]);
        assert!(!is_dead_end(&cands, me));
    }

    #[test]
    fn lan_ordered_before_public_and_hairpin_dropped() {
        let me = [198, 51, 100, 1];
        let cands = [
            public([198, 51, 100, 9], 5000), // non-hairpin public
            lan([10, 0, 0, 5], 6000),        // LAN
            public(me, 5001),                // hairpin ⇒ dropped
        ];
        let ordered = order_targets(&cands, me);
        assert_eq!(ordered.len(), 2);
        assert_eq!(ordered[0].kind, CandKind::Lan, "LAN first");
        assert_eq!(ordered[1].kind, CandKind::Public);
        assert!(!ordered.iter().any(|c| c.host == me), "no hairpin in output");
    }

    #[test]
    fn duplicate_host_port_deduped() {
        let me = [1, 1, 1, 1];
        let cands = [
            lan([10, 0, 0, 2], 100),
            lan([10, 0, 0, 2], 100), // exact dup
            public([9, 9, 9, 9], 200),
            public([9, 9, 9, 9], 200), // exact dup
        ];
        let ordered = order_targets(&cands, me);
        assert_eq!(ordered.len(), 2, "identical host:port de-duped");
    }

    #[test]
    fn transport_cgnat_skips_direct() {
        let t = transport_order(true, true, true);
        assert!(!t.contains(&Transport::UdpPunch) && !t.contains(&Transport::DirectTcp), "CGNAT skips direct");
        assert!(t.contains(&Transport::Mailbox));
        assert!(t.contains(&Transport::Relay), "relay present when has_relay");
        // relay omitted when no relay
        let t2 = transport_order(true, true, false);
        assert_eq!(t2, vec![Transport::Mailbox]);
    }

    #[test]
    fn transport_no_target_is_mailbox_only() {
        assert_eq!(transport_order(false, false, false), vec![Transport::Mailbox]);
        assert_eq!(transport_order(false, false, true), vec![Transport::Mailbox], "no target ⇒ mailbox only");
        assert_eq!(transport_order(false, true, true), vec![Transport::Mailbox]);
    }

    #[test]
    fn transport_full_path_order_and_relay_gating() {
        let with_relay = transport_order(true, false, true);
        assert_eq!(
            with_relay,
            vec![Transport::UdpPunch, Transport::DirectTcp, Transport::Relay, Transport::Mailbox]
        );
        let no_relay = transport_order(true, false, false);
        assert_eq!(no_relay, vec![Transport::UdpPunch, Transport::DirectTcp, Transport::Mailbox]);
        assert!(!no_relay.contains(&Transport::Relay), "relay omitted when has_relay false");
        // direct methods always precede the fallbacks
        let p = with_relay.iter().position(|t| *t == Transport::UdpPunch).unwrap_or(99);
        let m = with_relay.iter().position(|t| *t == Transport::Mailbox).unwrap_or(0);
        assert!(p < m, "punch tried before mailbox fallback");
    }

    #[test]
    fn deterministic() {
        let me = [203, 0, 113, 7];
        let cands = [lan([10, 0, 0, 1], 1), public([8, 8, 8, 8], 2), public(me, 3)];
        assert_eq!(order_targets(&cands, me), order_targets(&cands, me));
        assert_eq!(transport_order(true, false, true), transport_order(true, false, true));
        // empty input ⇒ empty ordering ⇒ dead end
        assert!(order_targets(&[], me).is_empty());
        assert!(is_dead_end(&[], me));
    }
}
