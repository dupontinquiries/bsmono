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
pub mod gossip;
pub mod identity;
pub mod presence;

pub use dht_kit::{InfoHash, Value, HASH_LEN};

/// The `(key, value)` an announce writes.
///
/// Kept pure so it is testable without a running node — the socket loop feeds
/// the result to whichever engine it drives.
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
}
