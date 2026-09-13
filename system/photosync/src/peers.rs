//! Sync-partner discovery — the DHT "footwork" from `dht-kit`, wired to
//! photosync specifically: turning a library id into an `InfoHash`,
//! announcing this device under it, and reading back who else has.
//!
//! # Why this stops here
//!
//! `dht-kit` is deliberately wire-format only — its own description is "no
//! transport, no routing table, no engine." That's as far as this module
//! goes too: it can tell you which `Announcement`s exist for a library, not
//! fetch anything from the peer they name. The fleet's actual encrypted
//! payload exchange for a mailbox lives in `share-kit::mailbox`, built on
//! `snapcore` (DHT-as-host) + `crypto-kit` (group sealing) — real
//! dependency weight for a crate that otherwise does no networking at all.
//! `snapclone-presence/src/dht.rs` and `snapclone-mailbox` are the
//! reference for that next layer, at the scale a synced photo library
//! would actually need it; this module is the same announce/find shape,
//! scaled down, so photosync has its own discovery step to hand off from
//! rather than pulling that whole stack in speculatively.
//!
//! # Testability
//!
//! `AnnounceStore` is the seam, the same role `share-kit::mailbox::Backend`
//! plays for its own put/get: an in-memory `MemStore` here for tests, a
//! networked DHT client as the production impl neither this module nor its
//! tests exercise.

use crate::Permission;
use dht_kit::{InfoHash, Value};
use std::collections::HashMap;

/// A library's discovery key. Two devices syncing the "same" library (same
/// id string, agreed out of band — a share link, a paired code) announce
/// and look each other up under this. Not a content hash of the photos
/// themselves, and not cryptographic — see `InfoHash::from_seed`'s own doc.
pub fn library_infohash(library_id: &str) -> InfoHash {
    InfoHash::from_seed(fnv64(library_id.as_bytes()))
}

fn fnv64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// What a device announces under a library's `InfoHash`: enough for another
/// device to recognize a real sync partner (the DHT lets any writer put a
/// value under a key, so a bare presence flag would be indistinguishable
/// from noise) and to know what access it's offering, before any actual
/// connection is made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    pub device_id: String,
    pub offer: Permission,
}

impl Announcement {
    /// A small pipe-delimited payload rather than pulling in `serde_json`
    /// for two fields — `dht-kit` itself carries no dependencies, and a DHT
    /// value has `MAX_VALUE_SIZE` to respect, not that two short strings
    /// would come near it.
    pub fn to_value(&self, value_id: u64) -> Value {
        Value::new(value_id, format!("{}|{}", self.device_id, self.offer.as_str()).into_bytes())
    }
    pub fn from_value(v: &Value) -> Option<Announcement> {
        let s = std::str::from_utf8(&v.data).ok()?;
        let (device_id, offer) = s.split_once('|')?;
        Some(Announcement { device_id: device_id.to_string(), offer: Permission::from_str(offer)? })
    }
}

/// Where announcements live. `dht-kit` has no transport of its own; this
/// trait is where one would plug in — a real implementation puts to and
/// gets from the network, `MemStore` below just puts to and gets from a
/// `HashMap` for tests.
pub trait AnnounceStore {
    fn put(&mut self, key: InfoHash, value: Value);
    fn get(&self, key: &InfoHash) -> Vec<Value>;
}

/// Test/local double for `AnnounceStore`. Re-announcing under the same
/// `value_id` replaces the previous value there — a device only ever holds
/// one live announcement per library, not an ever-growing history of them.
#[derive(Default)]
pub struct MemStore(HashMap<InfoHash, Vec<Value>>);

impl AnnounceStore for MemStore {
    fn put(&mut self, key: InfoHash, value: Value) {
        let slot = self.0.entry(key).or_default();
        slot.retain(|existing| existing.id != value.id);
        slot.push(value);
    }
    fn get(&self, key: &InfoHash) -> Vec<Value> {
        self.0.get(key).cloned().unwrap_or_default()
    }
}

/// Announce this device as a sync partner for `library_id`, offering
/// `offer` access to whoever finds it. `value_id` should be stable per
/// device (e.g. a hash of its own device id) so re-announcing (a periodic
/// refresh, the DHT-value equivalent of a TTL renewal) replaces the same
/// slot instead of accumulating duplicates — see `MemStore::put`.
pub fn announce(store: &mut dyn AnnounceStore, library_id: &str, me: &Announcement, value_id: u64) {
    store.put(library_infohash(library_id), me.to_value(value_id));
}

/// Every other device currently announced for `library_id`. Never includes
/// `self_device_id` — a device does not need to discover itself, and with a
/// stable `value_id` its own announcement is exactly one of the entries
/// `get` would otherwise return.
pub fn find_partners(store: &dyn AnnounceStore, library_id: &str, self_device_id: &str) -> Vec<Announcement> {
    store
        .get(&library_infohash(library_id))
        .iter()
        .filter_map(Announcement::from_value)
        .filter(|a| a.device_id != self_device_id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_devices_find_each_other_but_not_themselves() {
        let mut store = MemStore::default();
        let alice = Announcement { device_id: "alice-phone".into(), offer: Permission::Contribute };
        let bob = Announcement { device_id: "bob-laptop".into(), offer: Permission::FullSync };
        announce(&mut store, "family-photos", &alice, fnv64(b"alice-phone"));
        announce(&mut store, "family-photos", &bob, fnv64(b"bob-laptop"));

        let alice_sees = find_partners(&store, "family-photos", "alice-phone");
        assert_eq!(alice_sees, vec![bob.clone()]);
        let bob_sees = find_partners(&store, "family-photos", "bob-laptop");
        assert_eq!(bob_sees, vec![alice]);
    }

    #[test]
    fn different_libraries_do_not_cross_talk() {
        let mut store = MemStore::default();
        let alice = Announcement { device_id: "alice-phone".into(), offer: Permission::ReadOnly };
        announce(&mut store, "family-photos", &alice, 1);
        assert!(find_partners(&store, "vacation-2026", "someone-else").is_empty());
    }

    #[test]
    fn re_announcing_replaces_rather_than_accumulates() {
        let mut store = MemStore::default();
        let mut me = Announcement { device_id: "alice-phone".into(), offer: Permission::ReadOnly };
        announce(&mut store, "family-photos", &me, 42);
        me.offer = Permission::FullSync;
        announce(&mut store, "family-photos", &me, 42); // same value_id: a refresh, not a new peer

        let seen = store.get(&library_infohash("family-photos"));
        assert_eq!(seen.len(), 1, "one physical value under the key, not two");
        assert_eq!(Announcement::from_value(&seen[0]).unwrap().offer, Permission::FullSync);
    }

    #[test]
    fn garbled_bytes_under_the_key_are_ignored_not_a_panic() {
        let mut store = MemStore::default();
        store.put(library_infohash("family-photos"), Value::new(99, b"not the wire format".to_vec()));
        assert!(find_partners(&store, "family-photos", "nobody").is_empty());
    }
}
