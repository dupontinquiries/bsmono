//! snapclone-presence — what snapclone puts *in* a DHT, not a DHT.
//!
//! `snapclone-dht` was 1,388 lines with **zero dependants**, and a module-level
//! count showed why that was tolerable for so long: 1,062 of them were a second
//! implementation of things the fleet already had — its `wire` (which
//! `shared/dht-kit` was modelled on), its `engine` and `core` (snapcore's node,
//! routing and store), and its `sim` (`snapcore::sim`). The LAN parity harness
//! had been demonstrating for several commits that the two engines agree.
//!
//! These 297 lines are the part that was never duplicated: **what snapclone
//! stores and looks up**. An sc-ID, a presence record, the key it lives under,
//! and the gossip table of who has been heard from.
//!
//! ## Records, not transport
//!
//! Nothing here opens a socket or drives a lookup. `announce_record` returns a
//! `(key, value)` pair for a caller to `put`; `Presence` serialises and parses.
//! That is what lets the same code work over snapcore's engine today and a
//! different one later without the records changing — and it is why the
//! dependency is `dht-kit` (types) rather than an engine.

/// The caller `lib.rs` deliberately did not have: publish and resolve
/// presence over snapcore's engine.
pub mod dht;
/// A running DHT node: owns the socket, the table and the thread.
pub mod node_host;
pub mod gossip;
pub mod identity;
pub mod presence;

pub use dht_kit::{InfoHash, Value, HASH_LEN};

/// The `(key, value)` an announce writes.
///
/// Kept pure so it is testable without a running node — the socket loop feeds
/// the result to whichever engine it drives.
/// Why a host is not worth announcing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unannounceable {
    /// RFC 6598 carrier NAT. Reachable by nobody, including the peer's own ISP
    /// neighbours.
    Cgnat,
    /// RFC 1918. Meaningful on the LAN and meaningless in a global DHT, where
    /// 192.168.1.4 names a different machine in every house that reads it.
    Private,
    /// Loopback. Every reader dials itself.
    Loopback,
    /// Not an IP address at all.
    Unparseable,
}

impl Unannounceable {
    /// What to do instead, because "cannot announce" without a next step sends
    /// the caller to guess.
    pub fn remedy(self) -> &'static str {
        match self {
            Unannounceable::Cgnat =>
                "behind carrier NAT — announce the STUN reflexive address, or a relay token                  if punching has already failed",
            Unannounceable::Private =>
                "a LAN address — announce the reflexive address from STUN, and use mDNS for                  local discovery",
            Unannounceable::Loopback => "loopback — this build has no externally reachable address",
            Unannounceable::Unparseable => "not an IP address",
        }
    }
}

/// Is this host worth putting in a global DHT record?
///
/// ## Why the DHT is where this belongs
///
/// `snapclone_nat::cgnat::AddrClass` has classified addresses since the NAT
/// port, and NOTHING in `snapclone-presence` referenced it — so a peer behind
/// carrier NAT published its `100.64/10` address and every peer that found the
/// record dialled an address that cannot answer. The lookup succeeds, the
/// connection times out, and the failure looks like the far peer being offline
/// rather than like a record that was never dialable.
///
/// The parameter was already named `public_ip`. Naming is not enforcement.
pub fn announceable(host: &str) -> Result<(), Unannounceable> {
    use snapclone_nat::cgnat::AddrClass;
    let Ok(ip) = host.parse::<std::net::IpAddr>() else {
        return Err(Unannounceable::Unparseable);
    };
    match AddrClass::of(ip) {
        AddrClass::Public => Ok(()),
        AddrClass::Cgnat => Err(Unannounceable::Cgnat),
        AddrClass::Private => Err(Unannounceable::Private),
        AddrClass::Loopback => Err(Unannounceable::Loopback),
    }
}

pub fn announce_record(
    sc_id: &str,
    display_name: &str,
    public_ip: &str,
    punch_port: u16,
) -> (InfoHash, Value) {
    // The sc-ID is NOT in the record: it is the key. Storing it in the value
    // too would let a peer publish one id under another's key, and a reader
    // that trusted the field would believe it.
    let p = presence::Presence {
        display_name: display_name.to_string(),
        host: public_ip.to_string(),
        port: punch_port,
    };
    // The value id is derived from the sc-ID rather than random: a peer
    // re-announcing must REPLACE its own record, not add a second one under the
    // same key. A random id would leave every stale address live until its TTL.
    let vid = presence::value_id_for(sc_id);
    (presence::key_for_scid(sc_id), p.to_value(vid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sc_id_lives_in_the_key_not_the_record() {
        // Storing it in the value as well would let a peer publish one id under
        // another's key, and a reader trusting the field would believe it. The
        // key is the claim; the record is what it claims.
        let (_, v) = announce_record("sc-abc12345", "Ada", "203.0.113.7", 4222);
        assert!(!String::from_utf8_lossy(&v.data).contains("sc-abc12345"));
    }

    #[test]
    fn an_announce_round_trips_through_a_value() {
        let (key, v) = announce_record("sc-abc12345", "Ada", "203.0.113.7", 4222);
        assert_eq!(key, presence::key_for_scid("sc-abc12345"));
        let back = presence::Presence::from_value(&v).expect("parses");
        assert_eq!(back.display_name, "Ada");
        assert_eq!(back.host, "203.0.113.7");
        assert_eq!(back.port, 4222);
    }

    #[test]
    fn re_announcing_replaces_rather_than_accumulates() {
        // A random value id would leave every stale address live until its TTL,
        // so a peer that moved would be listed at both places.
        let (_, a) = announce_record("sc-abc12345", "Ada", "203.0.113.7", 4222);
        let (_, b) = announce_record("sc-abc12345", "Ada", "198.51.100.9", 5000);
        assert_eq!(a.id, b.id, "same peer must reuse its value id");
        assert_ne!(a.data, b.data, "but the record itself changed");
    }

    #[test]
    fn two_peers_do_not_share_a_value_id() {
        let (_, a) = announce_record("sc-aaaaaaaa", "A", "1.1.1.1", 1);
        let (_, b) = announce_record("sc-bbbbbbbb", "B", "1.1.1.2", 2);
        assert_ne!(a.id, b.id, "one peer would overwrite the other");
    }

#[cfg(test)]
mod announceable_tests {
    use super::*;

    /// THE CGNAT CASE THIS EXISTS FOR. A peer on carrier NAT published
    /// 100.64/10 and every reader dialled an address that cannot answer --
    /// which times out looking like the far peer is offline.
    #[test]
    fn a_carrier_nat_address_is_refused_and_says_what_to_do() {
        let e = announceable("100.64.1.1").unwrap_err();
        assert_eq!(e, Unannounceable::Cgnat);
        assert!(e.remedy().contains("reflexive"), "{}", e.remedy());
        // The whole /10, not just the first octet.
        assert_eq!(announceable("100.127.255.255").unwrap_err(), Unannounceable::Cgnat);
    }

    /// The range's EDGES, because an off-by-one here silently drops real
    /// public addresses or admits unroutable ones.
    #[test]
    fn the_cgnat_range_stops_where_rfc_6598_says() {
        assert!(announceable("100.63.255.255").is_ok(), "below the range is public");
        assert!(announceable("100.128.0.1").is_ok(), "above the range is public");
        assert!(announceable("100.64.0.0").is_err());
    }

    /// A LAN address in a GLOBAL dht names a different machine in every house
    /// that reads it.
    #[test]
    fn a_private_address_is_refused() {
        for h in ["192.168.1.4", "10.0.0.7", "172.16.5.9"] {
            assert_eq!(announceable(h).unwrap_err(), Unannounceable::Private, "{h}");
        }
    }

    #[test]
    fn loopback_and_nonsense_are_refused_distinctly() {
        assert_eq!(announceable("127.0.0.1").unwrap_err(), Unannounceable::Loopback);
        assert_eq!(announceable("not-an-ip").unwrap_err(), Unannounceable::Unparseable);
        assert_eq!(announceable("").unwrap_err(), Unannounceable::Unparseable);
    }

    /// A real public address still passes -- a guard that refused everything
    /// would pass every test above and stop the app working.
    #[test]
    fn a_public_address_is_announceable() {
        for h in ["8.8.8.8", "1.1.1.1", "203.0.113.5"] {
            assert!(announceable(h).is_ok(), "{h} should announce");
        }
        // IPv6 is not CGNAT and is routable.
        assert!(announceable("2001:db8::1").is_ok());
    }

    /// Every refusal names a next step: "cannot announce" alone sends the
    /// caller to guess.
    #[test]
    fn every_refusal_carries_a_remedy() {
        for e in [Unannounceable::Cgnat, Unannounceable::Private,
                  Unannounceable::Loopback, Unannounceable::Unparseable] {
            assert!(e.remedy().len() > 10, "{e:?}");
        }
    }
}
}
