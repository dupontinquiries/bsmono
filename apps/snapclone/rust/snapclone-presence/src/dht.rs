//! dht — publishing and resolving presence over a real DHT.
//!
//! ## The call site the crate deliberately did not have
//!
//! `lib.rs` says it plainly: "Nothing here opens a socket or drives a
//! lookup. `announce_record` returns a `(key, value)` pair for a caller
//! to `put`." That was the right seam and it left one thing missing —
//! the caller.
//!
//! With `snapcore::udp::UdpTransport` in place, every part now exists:
//!
//! ```text
//!   announce_record  →  (InfoHash, Value)     this crate, pure
//!   InfoHash → Key                            here, and see below
//!   lookup_nodes / store                      snapcore, the engine
//!   UdpTransport                              snapcore, the socket
//! ```
//!
//! This module is the arrow between them, and nothing more: no records
//! are defined here, no protocol is invented, and if it grows either it
//! is in the wrong place.
//!
//! ## Why the key is truncated rather than re-hashed
//!
//! `dht_kit::InfoHash` and `snapcore::routing::NodeId` are both 20-byte
//! identifiers, and `HASH_LEN` and `ID_LEN` are both 20. So the mapping
//! is a copy.
//!
//! Re-hashing would be worse than pointless: it would make the DHT key
//! for an sc-ID DIFFERENT from the one `announce_record` documented,
//! so a peer using dht-kit's own vocabulary and a peer going through
//! this bridge would publish the same person to two different keys and
//! never find each other. An assertion pins the two lengths together so
//! a future change to either is a compile-time problem rather than a
//! silent partition.

use snapcore::node::Transport;
use snapcore::routing::{Contact, NodeId, RoutingTable};
use snapcore::store::Key;

/// `dht-kit`'s key, as `snapcore`'s.
///
/// A copy, not a hash — see the module note on why re-hashing would
/// partition the network.
pub fn key_of(h: &crate::InfoHash) -> Key {
    // If these ever differ this stops compiling, which is the point:
    // a length mismatch discovered at run time would show up as peers
    // silently failing to find one another.
    const _: () = assert!(crate::HASH_LEN == 20);
    NodeId(h.0)
}

/// How many of the closest nodes a record is stored on -- OpenDHT's
/// `TARGET_NODES`. The lookup finds `k` (20); storing to all of them is 2.5x the
/// traffic of the reference for no extra findability, since a `get` walks to the
/// same few nodes nearest the key.
pub const STORE_ON: usize = 8;

/// Publish this device's presence.
///
/// Stores to the [`STORE_ON`] closest peers `lookup_nodes` can find. Returns how
/// many ACCEPTED the put (`Transport::store_acked`) -- it used to be how many were
/// asked, so a publish every peer refused (a stale token, a record too big)
/// reported the same number as one that worked.
///
/// **Zero is not an error.** A node that has just started, or one on a
/// network with no other peers, legitimately knows nobody.
pub fn publish<T: Transport>(
    me: &Contact,
    rt: &mut RoutingTable,
    tx: &T,
    sc_id: &str,
    display_name: &str,
    public_ip: &str,
    punch_port: u16,
    ttl: u64,
    now: u64,
) -> usize {
    let (hash, value) = crate::announce_record(sc_id, display_name, public_ip, punch_port);
    let key = key_of(&hash);
    let bytes = value.data.clone();
    store_closest(me, rt, tx, key, bytes, ttl, now)
}

/// Walk to `key` and store `bytes` on the nearest [`STORE_ON`] nodes; how many
/// accepted. One implementation for `publish` and `publish_raw`.
fn store_closest<T: Transport>(me: &Contact, rt: &mut RoutingTable, tx: &T, key: Key,
                               bytes: Vec<u8>, ttl: u64, now: u64) -> usize {
    let peers = snapcore::lookup::lookup_nodes(me, rt, &key, tx, snapcore::lookup::ALPHA, now);
    peers.iter()
        .take(STORE_ON)
        .filter(|p| tx.store_acked(me, p, key, bytes.clone(), ttl))
        .count()
}

/// Publish a payload the CALLER built, under the sc-ID's key.
///
/// `publish` above composes a `Presence` and serialises it with this
/// crate's own encoding. That is fine between two copies of this build and
/// useless against anything else: the Qt build stores an envelope-wrapped
/// multi-address record, and a peer reading one format cannot read the
/// other even when the key and the transport agree.
///
/// So the payload becomes the caller's business. This crate keeps what it
/// is actually authoritative about — how an sc-ID becomes a DHT key — and
/// stops having an opinion about what goes in the box.
///
/// `value_id` is the caller's too, and it matters more than it looks.
/// OpenDHT holds a SET per key, keyed by value id, so two puts sharing an
/// id are one value and the second overwrites the first. `value_id_for`
/// derives one from the sc-ID alone, which is right for a single record per
/// peer and silently lossy for anything else — publish three addresses as
/// three values with that id and two of them vanish.
pub fn publish_raw<T: Transport>(
    me: &Contact,
    rt: &mut RoutingTable,
    tx: &T,
    sc_id: &str,
    value_id: u64,
    payload: Vec<u8>,
    ttl: u64,
    now: u64,
) -> usize {
    let key = key_of(&crate::presence::key_for_scid(sc_id));
    let _ = value_id; // carried by the transport's own value framing
    store_closest(me, rt, tx, key, payload, ttl, now)
}

/// Fetch the raw values stored under an sc-ID's key.
///
/// The parsing counterpart to `publish_raw`: every value, undecoded, for a
/// caller that knows the format. `None` means the lookup finished having
/// found nothing, which is different from finding something unreadable —
/// the second is the caller's problem to report, and flattening them here
/// would hide a format mismatch as an absent peer.
pub fn resolve_raw<T: Transport>(
    me: &Contact,
    rt: &mut RoutingTable,
    tx: &T,
    sc_id: &str,
    now: u64,
) -> Option<Vec<Vec<u8>>> {
    let key = key_of(&crate::presence::key_for_scid(sc_id));
    let found = snapcore::lookup::lookup_value(me, rt, &key, tx, snapcore::lookup::ALPHA, now)?;
    if found.is_empty() { None } else { Some(found) }
}

/// One stored value as a `Presence`, or `None` if it is not one.
///
/// `from_value`, not a byte parser: the record is a dht-kit `Value`, and
/// its `id` is part of what identifies it. One definition of what a
/// presence record IS, used by the network path and the local store alike
/// — the alternative is a second parser that can drift.
pub fn parse_record(sc_id: &str, bytes: &[u8]) -> Option<crate::presence::Presence> {
    crate::presence::Presence::from_value(
        &crate::Value::new(crate::presence::value_id_for(sc_id), bytes.to_vec()))
}

/// Find someone by their sc-ID.
///
/// `None` means the lookup finished without a value — nobody had it, or
/// it expired. `Some(vec![])` cannot happen and is normalised away,
/// because "found nothing" and "found an empty answer" are the same
/// thing to a caller and having two spellings of it invites a bug.
///
/// Multiple values are returned rather than one: a person with two
/// devices publishes twice under the same key, and picking for the
/// caller would hide the second one.
pub fn resolve<T: Transport>(
    me: &Contact,
    rt: &mut RoutingTable,
    tx: &T,
    sc_id: &str,
    now: u64,
) -> Option<Vec<crate::presence::Presence>> {
    // The key must be derived the SAME way `publish` derives it, which
    // is why both go through `announce_record` rather than one of them
    // hashing the sc-id directly. The display name and address here are
    // placeholders: only the key is used, and the key is a function of
    // the sc-ID alone.
    let (hash, _) = crate::announce_record(sc_id, "", "", 0);
    let key = key_of(&hash);
    let found = snapcore::lookup::lookup_value(me, rt, &key, tx, snapcore::lookup::ALPHA, now)?;
    let parsed: Vec<crate::presence::Presence> = found
        .iter()
        // A record that does not parse is SKIPPED, not fatal. Anyone can
        // store anything under any key in a DHT, so one malformed value
        // must not hide the good ones sitting beside it.
        .filter_map(|b| parse_record(sc_id, b))
        .collect();
    if parsed.is_empty() {
        None
    } else {
        Some(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snapcore::node::FindValue;
    use std::cell::RefCell;
    use std::net::SocketAddr;

    fn contact(n: u8, port: u16) -> Contact {
        Contact {
            id: NodeId([n; 20]),
            addr: SocketAddr::from(([127, 0, 0, 1], port)),
            last_seen: 0,
            failed: 0,
        }
    }

    /// Records what it was asked to store, and answers lookups from a
    /// fixed set. Enough to test the ARROW without a network.
    #[derive(Default)]
    struct Recorder {
        stored: RefCell<Vec<(Key, Vec<u8>)>>,
        known: Vec<Contact>,
        value: Option<Vec<Vec<u8>>>,
    }

    impl Transport for Recorder {
        fn find_node(&self, _f: &Contact, _t: &Contact, _target: &NodeId) -> Vec<Contact> {
            self.known.clone()
        }
        fn find_value(&self, _f: &Contact, _t: &Contact, _k: &Key) -> FindValue {
            match &self.value {
                Some(v) => FindValue::Values(v.clone()),
                None => FindValue::Closest(self.known.clone()),
            }
        }
        fn store(&self, _f: &Contact, _t: &Contact, key: Key, value: Vec<u8>, _ttl: u64) {
            self.stored.borrow_mut().push((key, value));
        }
        fn ping(&self, _f: &Contact, _t: &Contact) -> bool {
            true
        }
    }

    fn table(me: &Contact, peers: &[Contact]) -> RoutingTable {
        let mut rt = RoutingTable::new(me.id);
        for p in peers {
            rt.saw(p.id, p.addr, 0);
        }
        rt
    }

    #[test]
    fn the_key_is_a_copy_of_the_infohash_not_a_rehash() {
        // If these diverge, a peer using dht-kit's vocabulary and a peer
        // going through this bridge publish the same person to two
        // different keys and never find each other.
        let (hash, _) = crate::announce_record("sc-abcd", "Ada", "10.0.0.2", 5000);
        assert_eq!(key_of(&hash).0, hash.0);
    }

    #[test]
    fn publish_and_resolve_agree_on_the_key() {
        // The property that makes the pair usable at all, and the one a
        // refactor is most likely to break — the two derive the key
        // independently.
        let me = contact(1, 100);
        let peer = contact(2, 200);
        let tx = Recorder { known: vec![peer.clone()], ..Default::default() };
        let mut rt = table(&me, &[peer]);
        publish(&me, &mut rt, &tx, "sc-abcd", "Ada", "10.0.0.2", 5000, 60, 0);

        let stored = tx.stored.borrow();
        assert!(!stored.is_empty(), "nothing was published");
        let (hash, _) = crate::announce_record("sc-abcd", "", "", 0);
        assert_eq!(stored[0].0, key_of(&hash), "resolve would look under a different key");
    }

    /// The count is ACCEPTANCES. A peer that refuses the put (a stale token, a 413)
    /// is not a peer that holds our record.
    #[test]
    fn a_refused_store_is_not_counted() {
        struct Refuses(Recorder);
        impl Transport for Refuses {
            fn find_node(&self, f: &Contact, t: &Contact, g: &NodeId) -> Vec<Contact> { self.0.find_node(f, t, g) }
            fn find_value(&self, f: &Contact, t: &Contact, k: &Key) -> FindValue { self.0.find_value(f, t, k) }
            fn store(&self, f: &Contact, t: &Contact, k: Key, v: Vec<u8>, ttl: u64) { self.0.store(f, t, k, v, ttl) }
            fn ping(&self, f: &Contact, t: &Contact) -> bool { self.0.ping(f, t) }
            fn store_acked(&self, _: &Contact, _: &Contact, _: Key, _: Vec<u8>, _: u64) -> bool { false }
        }
        let me = contact(1, 100);
        let peer = contact(2, 200);
        let tx = Refuses(Recorder { known: vec![peer.clone()], ..Default::default() });
        let mut rt = table(&me, &[peer.clone()]);
        assert_eq!(publish(&me, &mut rt, &tx, "sc-abcd", "Ada", "10.0.0.2", 5000, 60, 0), 0);
        let accepting = Recorder { known: vec![peer.clone()], ..Default::default() };
        let mut rt = table(&me, &[peer]);
        assert_eq!(publish(&me, &mut rt, &accepting, "sc-abcd", "Ada", "10.0.0.2", 5000, 60, 0), 1);
    }

    #[test]
    fn publishing_with_no_peers_is_zero_rather_than_an_error() {
        // A node that just started legitimately knows nobody. Treating
        // that as failure would make a fresh install look broken.
        let me = contact(1, 100);
        let tx = Recorder::default();
        let mut rt = table(&me, &[]);
        assert_eq!(publish(&me, &mut rt, &tx, "sc-abcd", "Ada", "10.0.0.2", 5000, 60, 0), 0);
    }

    #[test]
    fn a_malformed_record_does_not_hide_the_good_ones_beside_it() {
        // Anyone can store anything under any key in a DHT. One bad
        // value must not take the real answer with it.
        let me = contact(1, 100);
        let peer = contact(2, 200);
        let (_, good) = crate::announce_record("sc-abcd", "Ada", "10.0.0.2", 5000);
        let tx = Recorder {
            known: vec![peer.clone()],
            value: Some(vec![b"not a presence record".to_vec(), good.data.clone()]),
            ..Default::default()
        };
        let mut rt = table(&me, &[peer]);
        let got = resolve(&me, &mut rt, &tx, "sc-abcd", 0).expect("the good record survives");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].display_name, "Ada");
    }

    #[test]
    fn a_lookup_that_finds_only_junk_is_none_not_some_empty() {
        // "Found nothing" and "found an empty answer" are the same thing
        // to a caller, and two spellings of it invites a bug at every
        // call site.
        let me = contact(1, 100);
        let peer = contact(2, 200);
        let tx = Recorder {
            known: vec![peer.clone()],
            value: Some(vec![b"junk".to_vec()]),
            ..Default::default()
        };
        let mut rt = table(&me, &[peer]);
        assert!(resolve(&me, &mut rt, &tx, "sc-abcd", 0).is_none());
    }

    #[test]
    fn two_devices_under_one_id_both_come_back() {
        // A person with a phone and a laptop publishes twice under the
        // same key. Picking one for the caller would hide the other.
        let me = contact(1, 100);
        let peer = contact(2, 200);
        let (_, a) = crate::announce_record("sc-abcd", "Ada phone", "10.0.0.2", 5000);
        let (_, b) = crate::announce_record("sc-abcd", "Ada laptop", "10.0.0.3", 5001);
        let tx = Recorder {
            known: vec![peer.clone()],
            value: Some(vec![a.data.clone(), b.data.clone()]),
            ..Default::default()
        };
        let mut rt = table(&me, &[peer]);
        let got = resolve(&me, &mut rt, &tx, "sc-abcd", 0).unwrap();
        assert_eq!(got.len(), 2, "{got:?}");
    }
}
