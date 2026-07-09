//! store — where profiles live. Backends implement [`ProfileBackend`]; [`ProfileService`] composes
//! a primary (OpenDHT, global) with a fallback and keeps the fleet's graceful floor.
//!
//! **The fallback is snapcore's DHT — the one the whole fleet already uses** (share-kit Mailbox,
//! snapclone relay/session all ride it). We do NOT ship a second DHT: [`SnapcoreStore`] wraps
//! `snapcore::store::Store`, so a profile and a chat message land in the same key space.
use crate::profile::Profile;
use snapcore::routing::NodeId;
use snapcore::store::Store as SnapStore;

/// A key→bytes store for profiles.
pub trait ProfileBackend {
    fn available(&self) -> bool;
    fn put(&mut self, key: &str, value: Vec<u8>) -> bool;
    fn get(&self, key: &str) -> Option<Vec<u8>>;
}

/// Hash an arbitrary key string to a snapcore 20-byte NodeId (dependency-free FNV mix). The DHT's
/// own placement/hashing takes over from here; this only maps our string ids into its key space.
pub fn key_id(s: &str) -> NodeId {
    let mut out = [0u8; 20];
    let b = s.as_bytes();
    for (i, slot) in out.iter_mut().enumerate() {
        let mut h: u64 = 0xcbf29ce484222325 ^ (i as u64).wrapping_mul(0x100000001b3);
        for &c in b {
            h ^= c as u64;
            h = h.wrapping_mul(0x100000001b3);
            h = h.rotate_left(5);
        }
        *slot = (h ^ (h >> 32)) as u8;
    }
    NodeId(out)
}

/// The always-on backend: the fleet's snapcore DHT store. Profiles are durable (long TTL, refreshed
/// on re-publish). A fixed logical clock keeps them live; the real node advances it from wall time.
///
/// snapcore is a **multi-value** store (a republished value coexists with the stale one under the
/// same key). This wrapper gives it **register (latest-wins)** semantics — what a profile/presence
/// needs — by stamping every write with `(now, seq)` and returning the newest-stamped value on read.
/// `now` orders across nodes (wall time, when advanced); `seq` breaks ties between writes made at the
/// same `now` on this node.
pub struct SnapcoreStore {
    inner: SnapStore,
    now: u64,
    ttl: u64,
    seq: u64,
}
impl Default for SnapcoreStore {
    fn default() -> Self {
        SnapcoreStore { inner: SnapStore::new(), now: 1, ttl: 1 << 40, seq: 0 }
    }
}
impl SnapcoreStore {
    pub fn new() -> Self {
        Self::default()
    }
    /// Wrap an existing snapcore store (e.g. the same one chat rides on) so profiles + messages
    /// share one DHT.
    pub fn over(inner: SnapStore) -> Self {
        SnapcoreStore { inner, now: 1, ttl: 1 << 40, seq: 0 }
    }
}
impl ProfileBackend for SnapcoreStore {
    fn available(&self) -> bool {
        true
    }
    fn put(&mut self, key: &str, value: Vec<u8>) -> bool {
        // Prefix an (now, seq) stamp so get() can pick the newest of the values snapcore keeps under
        // this key. 16 bytes: now (8, LE) then seq (8, LE).
        let mut stamped = Vec::with_capacity(16 + value.len());
        stamped.extend_from_slice(&self.now.to_le_bytes());
        stamped.extend_from_slice(&self.seq.to_le_bytes());
        stamped.extend_from_slice(&value);
        self.inner.put(key_id(key), stamped, self.ttl, self.now);
        self.seq = self.seq.wrapping_add(1);
        true
    }
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        // Of the live values under this key, return the body with the highest (now, seq) stamp — the
        // most recently written one — so a republished profile supersedes the stale copy.
        self.inner
            .get(&key_id(key), self.now)
            .into_iter()
            .filter_map(|v| {
                if v.len() < 16 {
                    return None; // unstamped/foreign value — ignore
                }
                let now = u64::from_le_bytes(v[0..8].try_into().ok()?);
                let seq = u64::from_le_bytes(v[8..16].try_into().ok()?);
                Some(((now, seq), v[16..].to_vec()))
            })
            .max_by_key(|(stamp, _)| *stamp)
            .map(|(_, body)| body)
    }
}

/// Which backend served a lookup — surfaced so the UI can show "global" vs "LAN" reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    OpenDht,
    Snapcore,
    Miss,
}

/// Composes a primary (OpenDHT) and a fallback (snapcore DHT).
pub struct ProfileService<P: ProfileBackend, F: ProfileBackend> {
    primary: P,
    fallback: F,
}

impl<P: ProfileBackend, F: ProfileBackend> ProfileService<P, F> {
    pub fn new(primary: P, fallback: F) -> Self {
        ProfileService { primary, fallback }
    }

    /// Publish a profile: always write the snapcore DHT (instant local + fleet discovery); also
    /// OpenDHT when reachable (global). True if it landed anywhere.
    pub fn publish(&mut self, profile: &Profile) -> bool {
        let bytes = profile.encode();
        let local = self.fallback.put(&profile.id, bytes.clone());
        let global = self.primary.available() && self.primary.put(&profile.id, bytes);
        local || global
    }

    /// Look up a profile: OpenDHT first (global), snapcore DHT if it's down or misses.
    pub fn lookup(&self, id: &str) -> (Option<Profile>, Reach) {
        if self.primary.available() {
            if let Some(bytes) = self.primary.get(id) {
                if let Some(p) = Profile::decode(&bytes) {
                    return (Some(p), Reach::OpenDht);
                }
            }
        }
        if let Some(bytes) = self.fallback.get(id) {
            if let Some(p) = Profile::decode(&bytes) {
                return (Some(p), Reach::Snapcore);
            }
        }
        (None, Reach::Miss)
    }

    pub fn friends_of(&self, profile: &Profile) -> Vec<Profile> {
        profile.friends.iter().filter_map(|id| self.lookup(id).0).collect()
    }

    /// Publish raw bytes under a key — for stored values that aren't [`Profile`]s (e.g. the session
    /// records in [`crate::session`]). Same dual-write discipline as [`Self::publish`]: always the
    /// snapcore floor for instant LAN/fleet reach, plus OpenDHT when it's up for global reach.
    pub fn put_raw(&mut self, key: &str, value: Vec<u8>) -> bool {
        let local = self.fallback.put(key, value.clone());
        let global = self.primary.available() && self.primary.put(key, value);
        local || global
    }

    /// Fetch raw bytes for a key: OpenDHT first (global), snapcore DHT if it's down or misses.
    /// Surfaces which backend served it, like [`Self::lookup`].
    pub fn get_raw(&self, key: &str) -> (Option<Vec<u8>>, Reach) {
        if self.primary.available() {
            if let Some(bytes) = self.primary.get(key) {
                return (Some(bytes), Reach::OpenDht);
            }
        }
        if let Some(bytes) = self.fallback.get(key) {
            return (Some(bytes), Reach::Snapcore);
        }
        (None, Reach::Miss)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{Presence, Profile};
    use std::collections::HashMap;

    /// Toggleable fake standing in for OpenDHT.
    #[derive(Default)]
    struct Fake {
        up: bool,
        map: HashMap<String, Vec<u8>>,
    }
    impl ProfileBackend for Fake {
        fn available(&self) -> bool {
            self.up
        }
        fn put(&mut self, key: &str, value: Vec<u8>) -> bool {
            if !self.up {
                return false;
            }
            self.map.insert(key.to_string(), value);
            true
        }
        fn get(&self, key: &str) -> Option<Vec<u8>> {
            if self.up {
                self.map.get(key).cloned()
            } else {
                None
            }
        }
    }

    #[test]
    fn snapcore_is_the_fallback_when_opendht_down() {
        let mut svc = ProfileService::new(Fake { up: false, ..Default::default() }, SnapcoreStore::new());
        let mut p = Profile::new("kd@a1", "kesslerd");
        p.presence = Presence::InGame { game: "Blackstar".into(), session: Some("l7".into()) };
        assert!(svc.publish(&p), "publish succeeds via the snapcore floor");
        let (got, reach) = svc.lookup("kd@a1");
        assert_eq!(got, Some(p));
        assert_eq!(reach, Reach::Snapcore);
    }

    #[test]
    fn opendht_preferred_when_up() {
        let mut svc = ProfileService::new(Fake { up: true, ..Default::default() }, SnapcoreStore::new());
        let p = Profile::new("kd@a1", "kesslerd");
        assert!(svc.publish(&p));
        assert_eq!(svc.lookup("kd@a1"), (Some(p), Reach::OpenDht));
    }

    #[test]
    fn friends_resolve_over_snapcore() {
        let mut svc = ProfileService::new(Fake { up: false, ..Default::default() }, SnapcoreStore::new());
        for (id, n) in [("bob@f0", "bob"), ("amy@9c", "amy")] {
            svc.publish(&Profile::new(id, n));
        }
        let mut me = Profile::new("kd@a1", "kesslerd");
        me.friends = vec!["bob@f0".into(), "amy@9c".into(), "ghost@00".into()];
        assert_eq!(svc.friends_of(&me).len(), 2);
    }

    #[test]
    fn republished_profile_supersedes_stale_on_snapcore() {
        // snapcore keeps both the old and new value under a key; the wrapper must return the newer,
        // or presence never updates on the LAN/offline floor. OpenDHT down → snapcore only.
        let mut svc = ProfileService::new(Fake { up: false, ..Default::default() }, SnapcoreStore::new());
        let mut p = Profile::new("ada@01", "Ada");
        assert!(svc.publish(&p)); // Online
        p.presence = Presence::InGame { game: "unendlich".into(), session: Some("tok".into()) };
        assert!(svc.publish(&p)); // republish: now InGame
        let (got, reach) = svc.lookup("ada@01");
        assert_eq!(reach, Reach::Snapcore);
        assert!(
            matches!(got.unwrap().presence, Presence::InGame { .. }),
            "the most recently published presence must win"
        );

        // ...and going back to Online supersedes again (not stuck InGame).
        p.presence = Presence::Online;
        assert!(svc.publish(&p));
        assert!(matches!(svc.lookup("ada@01").0.unwrap().presence, Presence::Online));
    }
}
