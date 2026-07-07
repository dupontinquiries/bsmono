//! store — where profiles live. One [`ProfileBackend`] trait, two implementations: the pure
//! [`MicroDhtStore`] (always available) and an OpenDHT backend (FFI, feature-gated). The
//! [`ProfileService`] composes them with the fleet's usual graceful floor: try OpenDHT (global
//! reach); if it's unavailable or misses, fall back to the micro-DHT (LAN) — and always mirror
//! writes to the micro-DHT so a friend on the same network is found instantly, offline or not.

use crate::microdht::MicroDht;
use crate::profile::Profile;

/// A key→bytes store for profiles. `available()` lets the service skip a dead backend without
/// blocking.
pub trait ProfileBackend {
    fn available(&self) -> bool;
    fn put(&mut self, key: &str, value: Vec<u8>) -> bool;
    fn get(&self, key: &str) -> Option<Vec<u8>>;
}

/// The always-on micro-DHT backend.
pub struct MicroDhtStore {
    dht: MicroDht,
}
impl MicroDhtStore {
    pub fn new(dht: MicroDht) -> Self {
        MicroDhtStore { dht }
    }
}
impl ProfileBackend for MicroDhtStore {
    fn available(&self) -> bool {
        true // LAN store is always up
    }
    fn put(&mut self, key: &str, value: Vec<u8>) -> bool {
        self.dht.put(key, value);
        true
    }
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.dht.get(key).cloned()
    }
}

/// Which backend served a lookup — surfaced so the UI can show "global" vs "LAN" reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    OpenDht,
    MicroDht,
    Miss,
}

/// Composes a primary (OpenDHT) and a fallback (micro-DHT). Generic over both so it's testable with
/// fakes and the real OpenDHT FFI drops in unchanged.
pub struct ProfileService<P: ProfileBackend, F: ProfileBackend> {
    primary: P,
    fallback: F,
}

impl<P: ProfileBackend, F: ProfileBackend> ProfileService<P, F> {
    pub fn new(primary: P, fallback: F) -> Self {
        ProfileService { primary, fallback }
    }

    /// Publish a profile. Always writes the LAN fallback (instant local discovery); also writes
    /// OpenDHT when reachable (global). Returns true if it landed anywhere.
    pub fn publish(&mut self, profile: &Profile) -> bool {
        let bytes = profile.encode();
        let local = self.fallback.put(&profile.id, bytes.clone());
        let global = self.primary.available() && self.primary.put(&profile.id, bytes);
        local || global
    }

    /// Look up a profile by id: OpenDHT first (global reach), micro-DHT if it's down or misses.
    /// Reports which path answered.
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
                return (Some(p), Reach::MicroDht);
            }
        }
        (None, Reach::Miss)
    }

    /// Resolve a friends list into their profiles (skipping any that can't be found).
    pub fn friends_of(&self, profile: &Profile) -> Vec<Profile> {
        profile
            .friends
            .iter()
            .filter_map(|id| self.lookup(id).0)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::microdht::{MicroDht, NodeId};
    use crate::profile::{Presence, Profile};
    use std::collections::HashMap;

    /// A fake backend we can toggle up/down, standing in for OpenDHT.
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
            if !self.up {
                return None;
            }
            self.map.get(key).cloned()
        }
    }

    fn micro() -> MicroDhtStore {
        MicroDhtStore::new(MicroDht::new(NodeId::from_key("me"), 20))
    }

    #[test]
    fn opendht_serves_when_up() {
        let primary = Fake { up: true, ..Default::default() };
        let mut svc = ProfileService::new(primary, micro());
        let p = Profile::new("kd@a1", "kesslerd");
        assert!(svc.publish(&p));
        let (got, reach) = svc.lookup("kd@a1");
        assert_eq!(got, Some(p));
        assert_eq!(reach, Reach::OpenDht, "global path preferred when up");
    }

    #[test]
    fn falls_back_to_micro_when_opendht_down() {
        let primary = Fake { up: false, ..Default::default() }; // OpenDHT unreachable
        let mut svc = ProfileService::new(primary, micro());
        let mut p = Profile::new("kd@a1", "kesslerd");
        p.presence = Presence::InGame { game: "clawbound".into(), session: Some("l7".into()) };
        assert!(svc.publish(&p), "publish still succeeds via the LAN floor");
        let (got, reach) = svc.lookup("kd@a1");
        assert_eq!(got, Some(p));
        assert_eq!(reach, Reach::MicroDht, "LAN fallback answers when global is down");
    }

    #[test]
    fn micro_covers_opendht_miss() {
        // OpenDHT up but doesn't have this id; micro does (published locally) → micro answers.
        let primary = Fake { up: true, ..Default::default() };
        let mut svc = ProfileService::new(primary, micro());
        let p = Profile::new("kd@a1", "kesslerd");
        // write ONLY to the fallback to simulate a not-yet-propagated global entry
        svc.fallback.put(&p.id, p.encode());
        let (got, reach) = svc.lookup("kd@a1");
        assert_eq!(got, Some(p));
        assert_eq!(reach, Reach::MicroDht);
    }

    #[test]
    fn friends_resolve_across_backends() {
        let mut svc = ProfileService::new(Fake { up: true, ..Default::default() }, micro());
        let bob = Profile::new("bob@f0", "bob");
        let amy = Profile::new("amy@9c", "amy");
        svc.publish(&bob);
        svc.publish(&amy);
        let mut me = Profile::new("kd@a1", "kesslerd");
        me.friends = vec!["bob@f0".into(), "amy@9c".into(), "ghost@00".into()];
        let friends = svc.friends_of(&me);
        assert_eq!(friends.len(), 2, "resolves known friends, skips the ghost");
    }
}
