//! session — the control-plane handoff that makes "▶ join" real without a central server.
//!
//! The design (see `games/NETCODE_DHT_ANALYSIS.md`) is a *handoff*, not a merge of the two
//! transport planes:
//!   * **control plane** (this module): a game host announces a [`SessionRecord`] to the fleet DHT
//!     — the same snapcore store profiles + chat already ride — and flips its [`Profile`] presence
//!     to [`Presence::InGame`] carrying a session token. Friends discover it by looking up the
//!     host's profile, reading the token, and resolving the record.
//!   * **data plane** (each game keeps its own): the record hands back a `transport` + `addr`; the
//!     game's own UDP/enet transport dials it for realtime packets. This module never touches
//!     gameplay bytes.
//!
//! Same graceful floor as the rest of the crate: OpenDHT is the global reach, the snapcore micro-DHT
//! is the always-on LAN/offline fallback, and the network is a step-up, never a requirement.
//!
//! Wire framing matches [`crate::profile`] — a versioned, length-prefixed frame — so records
//! round-trip through any backend as opaque bytes. (net-kit `wire` is an alternative encoding if the
//! fleet ever wants one canonical line format across languages; kept dependency-free here.)

use crate::profile::{Presence, Profile};
use crate::store::{ProfileBackend, ProfileService, Reach};

const WIRE_V: u8 = 1;

fn put_str(b: &mut Vec<u8>, s: &str) {
    b.extend_from_slice(&(s.len() as u32).to_le_bytes());
    b.extend_from_slice(s.as_bytes());
}
fn take_str(b: &mut &[u8]) -> Option<String> {
    if b.len() < 4 {
        return None;
    }
    let n = u32::from_le_bytes(b[..4].try_into().ok()?) as usize;
    let rest = &b[4..];
    if rest.len() < n {
        return None;
    }
    let (s, tail) = rest.split_at(n);
    *b = tail;
    Some(String::from_utf8_lossy(s).into_owned())
}

/// Where + how to reach a live game session — the control-plane record friends resolve before
/// their game's own transport dials in for the realtime (data-plane) connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    /// Game name (matches [`Presence::InGame`]'s `game`, e.g. "unendlich", "blackstar").
    pub game: String,
    /// Host player id (their profile is the discovery anchor).
    pub host: String,
    /// Data-plane transport the host is listening on ("udp", "enet", "quic", …). Opaque to the DHT;
    /// the joining game decides whether it can speak it.
    pub transport: String,
    /// Dial string for that transport ("ip:port", a rendezvous token, …).
    pub addr: String,
}

impl SessionRecord {
    pub fn new(
        game: impl Into<String>,
        host: impl Into<String>,
        transport: impl Into<String>,
        addr: impl Into<String>,
    ) -> Self {
        SessionRecord {
            game: game.into(),
            host: host.into(),
            transport: transport.into(),
            addr: addr.into(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![WIRE_V];
        put_str(&mut b, &self.game);
        put_str(&mut b, &self.host);
        put_str(&mut b, &self.transport);
        put_str(&mut b, &self.addr);
        b
    }

    pub fn decode(bytes: &[u8]) -> Option<SessionRecord> {
        let mut b = bytes;
        let (&v, rest) = b.split_first()?;
        if v != WIRE_V {
            return None;
        }
        b = rest;
        let game = take_str(&mut b)?;
        let host = take_str(&mut b)?;
        let transport = take_str(&mut b)?;
        let addr = take_str(&mut b)?;
        Some(SessionRecord { game, host, transport, addr })
    }
}

/// The DHT key a session record lives under. Namespaced so a session token can never collide with a
/// player-id profile key in the shared store.
pub fn session_key(token: &str) -> String {
    format!("session:{token}")
}

/// Control-plane operations layered on the existing [`ProfileService`]. Blanket-implemented, so any
/// primary/fallback backend pair (OpenDHT + snapcore, or two fakes in a test) gets the handoff.
pub trait SessionExt {
    /// Host side: publish `rec` under `token` and flip `host`'s presence to
    /// `InGame { game, session: token }`, republishing the profile so friends see it. Returns true
    /// if the record landed on at least one backend.
    fn announce_session(&mut self, host: &mut Profile, token: &str, rec: &SessionRecord) -> bool;

    /// Host side: tear down. Flips presence back to `Online` and republishes. Because
    /// [`crate::store::SnapcoreStore`] is now latest-wins, this republish supersedes the `InGame`
    /// presence even on the micro-DHT floor, so `join_via_friend` stops offering the session
    /// immediately. The session *record* itself isn't deleted — it's left to expire by TTL — so a
    /// peer resolving the raw token directly could still get a now-dead `addr` until then; its own
    /// transport dial failing is the correct place to notice. (Presence, the path friends actually
    /// use, is crisp.)
    fn end_session(&mut self, host: &mut Profile) -> bool;

    /// Joiner side: fetch a session record by its token.
    fn resolve_session(&self, token: &str) -> (Option<SessionRecord>, Reach);

    /// Joiner side, the full "▶ join a friend" path: look the friend up, read the session token off
    /// their presence, and resolve the record. `None` if they're offline, not in a game, or the
    /// record has expired. The caller hands the returned `(transport, addr)` to its own game
    /// transport — that dial is the data plane and lives outside this crate.
    fn join_via_friend(&self, friend_id: &str) -> Option<SessionRecord>;
}

impl<P: ProfileBackend, F: ProfileBackend> SessionExt for ProfileService<P, F> {
    fn announce_session(&mut self, host: &mut Profile, token: &str, rec: &SessionRecord) -> bool {
        // Store the record on the DHT under its namespaced session key, via the profile-agnostic
        // raw put (a session record is a stored value like a profile, just under a different key).
        let landed = self.put_raw(&session_key(token), rec.encode());
        // Flip + republish presence so discovery works.
        host.presence = Presence::InGame { game: rec.game.clone(), session: Some(token.to_string()) };
        let presence_ok = self.publish(host);
        landed || presence_ok
    }

    fn end_session(&mut self, host: &mut Profile) -> bool {
        host.presence = Presence::Online;
        self.publish(host)
    }

    fn resolve_session(&self, token: &str) -> (Option<SessionRecord>, Reach) {
        let key = session_key(token);
        let (bytes, reach) = self.get_raw(&key);
        match bytes.and_then(|b| SessionRecord::decode(&b)) {
            Some(rec) => (Some(rec), reach),
            None => (None, Reach::Miss),
        }
    }

    fn join_via_friend(&self, friend_id: &str) -> Option<SessionRecord> {
        let (profile, _) = self.lookup(friend_id);
        let profile = profile?;
        match profile.presence {
            Presence::InGame { session: Some(token), .. } => self.resolve_session(&token).0,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SnapcoreStore;
    use std::collections::HashMap;

    /// Toggleable fake standing in for OpenDHT (mirrors store.rs's test fake).
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
            self.map.get(key).cloned()
        }
    }

    #[test]
    fn decodes_bytes_from_the_python_fleet_module() {
        // Byte-for-byte interop with games/paperbound/fleet.py: the exact bytes its
        // SessionRecord("paperbound","kesslerd","http","10.0.0.5:7777").encode() produces.
        let py: &[u8] = &[1, 10, 0, 0, 0, 112, 97, 112, 101, 114, 98, 111, 117, 110, 100, 8, 0, 0, 0, 107, 101, 115, 115, 108, 101, 114, 100, 4, 0, 0, 0, 104, 116, 116, 112, 13, 0, 0, 0, 49, 48, 46, 48, 46, 48, 46, 53, 58, 55, 55, 55, 55];
        let rec = SessionRecord::decode(py).expect("python bytes must decode");
        assert_eq!(rec, SessionRecord::new("paperbound", "kesslerd", "http", "10.0.0.5:7777"));
        assert_eq!(rec.encode(), py, "our encode must match Python byte-for-byte");
    }

    #[test]
    fn session_record_roundtrips() {
        let rec = SessionRecord::new("unendlich", "player-a", "udp", "192.168.1.5:4123");
        let back = SessionRecord::decode(&rec.encode()).unwrap();
        assert_eq!(rec, back);
    }

    #[test]
    fn decode_rejects_bad_version_and_truncation() {
        assert!(SessionRecord::decode(&[]).is_none());
        assert!(SessionRecord::decode(&[9, 0, 0, 0, 0]).is_none()); // wrong WIRE_V
        let mut good = SessionRecord::new("g", "h", "t", "a").encode();
        good.truncate(good.len() - 2);
        assert!(SessionRecord::decode(&good).is_none());
    }

    #[test]
    fn session_keys_are_namespaced() {
        // A session token and a same-named player id must not collide in the shared store.
        assert_ne!(session_key("player-a"), "player-a");
        assert!(session_key("abc").starts_with("session:"));
    }

    #[test]
    fn announce_then_join_over_snapcore_fallback() {
        // OpenDHT down: everything must still work on the snapcore floor.
        let mut svc = ProfileService::new(Fake::default(), SnapcoreStore::new());
        let mut host = Profile::new("player-a", "Ada");
        let rec = SessionRecord::new("unendlich", "player-a", "udp", "10.0.0.2:5000");
        assert!(svc.announce_session(&mut host, "tok-123", &rec));

        // Host presence now advertises the joinable session.
        match &host.presence {
            Presence::InGame { game, session } => {
                assert_eq!(game, "unendlich");
                assert_eq!(session.as_deref(), Some("tok-123"));
            }
            other => panic!("expected InGame, got {other:?}"),
        }

        // A friend who has the host in their list resolves the record end-to-end.
        let (got, reach) = svc.resolve_session("tok-123");
        assert_eq!(got.as_ref(), Some(&rec));
        assert_eq!(reach, Reach::Snapcore);

        let joined = svc.join_via_friend("player-a").expect("friend is joinable");
        assert_eq!(joined, rec);
        assert_eq!(joined.transport, "udp");
        assert_eq!(joined.addr, "10.0.0.2:5000");
    }

    #[test]
    fn ending_a_session_stops_the_invite_on_the_snapcore_floor() {
        // With the latest-wins SnapcoreStore, republishing Online supersedes the InGame presence even
        // on the LAN/offline floor, so a friend stops seeing the invite — no register primary needed.
        let mut svc = ProfileService::new(Fake::default(), SnapcoreStore::new()); // OpenDHT down
        let mut host = Profile::new("player-a", "Ada");
        let rec = SessionRecord::new("blackstar", "player-a", "enet", "10.0.0.2:6001");
        svc.announce_session(&mut host, "tok-x", &rec);
        assert!(matches!(host.presence, Presence::InGame { .. }));
        assert!(svc.join_via_friend("player-a").is_some());

        svc.end_session(&mut host);
        assert!(matches!(host.presence, Presence::Online));
        assert!(svc.join_via_friend("player-a").is_none(), "ended session no longer joinable via presence");
    }

    #[test]
    fn latest_wins_when_the_primary_is_a_register() {
        // With a latest-wins backend up (OpenDHT here, faked as replace-by-key), ending a session
        // *does* immediately stop the invite — showing the design is correct and only the multi-value
        // micro-DHT fallback is eventually-consistent.
        let mut up = Fake::default();
        up.up = true;
        let mut svc = ProfileService::new(up, SnapcoreStore::new());
        let mut host = Profile::new("player-b", "Grace");
        let rec = SessionRecord::new("unendlich", "player-b", "udp", "10.0.0.3:7000");
        svc.announce_session(&mut host, "tok-y", &rec);
        assert!(svc.join_via_friend("player-b").is_some());

        svc.end_session(&mut host); // republishes Online; Fake replaces by key → newest wins
        assert!(svc.join_via_friend("player-b").is_none());
    }
}
