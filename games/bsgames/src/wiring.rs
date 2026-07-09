//! wiring — the I/O seams, connected to the fleet's existing shared code rather than reinvented:
//! * **presence** rides coord-core's `Activity` (the same bus every fleet app publishes to), so
//!   launching a game shows up fleet-wide, not just in bsgames;
//! * **chat** rides share-kit's `Mailbox` over snapcore's `Store` — the fleet's Kademlia DHT — so
//!   messages are sealed, content-addressed, and delivered exactly like every other fleet share.
use netcode::profile::{Presence, Profile};
use netcode::session::{SessionExt, SessionRecord};
use netcode::store::{ProfileBackend, ProfileService};
use share_kit::{Backend, Lifetime, Mailbox};

/// Map a game profile's presence onto the fleet-wide coord-core Activity, so other apps (voiced,
/// media, the launcher) react to "in a game" uniformly.
pub fn activity_of(presence: &Presence) -> coord_core::Activity {
    match presence {
        Presence::InGame { .. } => coord_core::Activity::InGame,
        Presence::Online | Presence::Offline => coord_core::Activity::Idle,
    }
}

/// A deterministic direct-message feed id for two players — order-independent so both sides derive
/// the same mailbox.
pub fn dm_id(a: &str, b: &str) -> String {
    if a <= b {
        format!("{a}|{b}")
    } else {
        format!("{b}|{a}")
    }
}

/// The DM mailbox between two players.
pub fn dm_mailbox(a: &str, b: &str) -> Mailbox {
    Mailbox::new("bsgames-chat", &dm_id(a, b))
}

/// Per-DM sealing key. Derived deterministically from the DM id via netcode's hash (so both peers
/// agree without a handshake). NOTE: this is a stand-in for crypto-kit's `SenderChain` ratchet — a
/// fixed per-conversation key, adequate to wire + test the path; swap in the ratchet for forward
/// secrecy.
pub fn dm_key(a: &str, b: &str) -> [u8; 32] {
    // 32-byte sealing key (crypto-kit seal needs 32) derived from the DM id — separate from the
    // DHT's 20-byte NodeId. Stand-in for crypto-kit's SenderChain ratchet.
    let mut out = [0u8; 32];
    let bytes = dm_id(a, b);
    for (i, slot) in out.iter_mut().enumerate() {
        let mut h: u64 = 0xcbf29ce484222325 ^ (i as u64).wrapping_mul(0x100000001b3);
        for &c in bytes.as_bytes() {
            h ^= c as u64;
            h = h.wrapping_mul(0x100000001b3);
            h = h.rotate_left(5);
        }
        *slot = (h ^ (h >> 32)) as u8;
    }
    out
}

/// Send a chat line into the DM feed. Sender is embedded in the sealed payload so the receiver can
/// attribute it. Returns the blob's content hash.
pub fn send_chat<B: Backend>(
    store: &mut B,
    mb: &Mailbox,
    me: &str,
    key: &[u8; 32],
    text: &str,
    epoch: u64,
    now: u64,
) -> String {
    let payload = format!("{me}\x1f{text}");
    mb.post(store, me, 0, key, payload.as_bytes(), epoch, Lifetime::Ephemeral, now)
}

/// Fetch + open the DM feed into `(sender, text)` lines in feed order.
pub fn recv_chat<B: Backend>(
    store: &B,
    mb: &Mailbox,
    key: &[u8; 32],
    epoch: u64,
    now: u64,
) -> Vec<(String, String)> {
    mb.fetch_open(store, epoch, now, |_s, _i| Some(*key))
        .iter()
        .filter_map(|b| {
            let s = String::from_utf8(b.clone()).ok()?;
            let (from, text) = s.split_once('\x1f')?;
            Some((from.to_string(), text.to_string()))
        })
        .collect()
}

/// A deterministic session token for `me` hosting `game`, so a re-host reuses the same DHT key
/// (fresh announces dedup instead of piling up distinct records). Order-independent of nothing —
/// it's just host+game — but stable, which is what the joiner-side lookup relies on.
pub fn session_token(me: &str, game: &str) -> String {
    format!("{game}@{me}")
}

/// bsgames **"▶ Host"**: announce this player's live game session to the fleet DHT (the same
/// snapcore store chat rides) and reflect "in a game" on the fleet-wide activity bus. `transport` +
/// `addr` describe the host's *own* realtime socket (the data plane); this only publishes discovery.
/// Returns the session token friends resolve.
pub fn host_game_session<P: ProfileBackend, F: ProfileBackend>(
    svc: &mut ProfileService<P, F>,
    me: &mut Profile,
    game: &str,
    transport: &str,
    addr: &str,
) -> (String, coord_core::Activity) {
    let token = session_token(&me.id, game);
    let rec = SessionRecord::new(game, &me.id, transport, addr);
    svc.announce_session(me, &token, &rec);
    (token, activity_of(&me.presence))
}

/// bsgames **"▶ Join"**: resolve a friend's live session (found via their presence) into a
/// `(transport, addr)` dial target for *our* game's own transport. `None` if they're offline or not
/// in a joinable game. The returned dial is handed to the data plane — this crate never speaks it.
pub fn join_friend_session<P: ProfileBackend, F: ProfileBackend>(
    svc: &ProfileService<P, F>,
    friend_id: &str,
) -> Option<(String, String)> {
    svc.join_via_friend(friend_id).map(|r| (r.transport, r.addr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use netcode::profile::Presence;
    use netcode::store::SnapcoreStore;
    use snapcore::store::Store;

    /// Toggleable fake OpenDHT (register semantics), mirroring netcode's own test fake.
    #[derive(Default)]
    struct FakeDht {
        up: bool,
        map: std::collections::HashMap<String, Vec<u8>>,
    }
    impl ProfileBackend for FakeDht {
        fn available(&self) -> bool {
            self.up
        }
        fn put(&mut self, k: &str, v: Vec<u8>) -> bool {
            if self.up {
                self.map.insert(k.to_string(), v);
            }
            self.up
        }
        fn get(&self, k: &str) -> Option<Vec<u8>> {
            self.map.get(k).cloned()
        }
    }

    #[test]
    fn presence_bridges_to_fleet_activity() {
        assert_eq!(activity_of(&Presence::InGame { game: "Blackstar".into(), session: None }), coord_core::Activity::InGame);
        assert_eq!(activity_of(&Presence::Online), coord_core::Activity::Idle);
    }

    #[test]
    fn dm_id_is_order_independent() {
        assert_eq!(dm_id("amy", "bob"), dm_id("bob", "amy"));
        assert_eq!(dm_key("amy", "bob"), dm_key("bob", "amy"));
    }

    // Cross-application: bsgames chat travels through share-kit's Mailbox over snapcore's Store and
    // comes back attributed + in order — proving the wiring against the REAL fleet primitives.
    #[test]
    fn chat_round_trips_through_share_kit_mailbox_on_snapcore_store() {
        let mut store = Store::new();
        let (me, them) = ("kesslerd", "bob");
        let mb = dm_mailbox(me, them);
        let key = dm_key(me, them);
        let (epoch, mut now) = (0u64, 1000u64);

        send_chat(&mut store, &mb, me, &key, "gg wp", epoch, now);
        now += 1;
        send_chat(&mut store, &mb, them, &key, "rematch?", epoch, now);

        // the other side derives the SAME mailbox + key and reads both, attributed + ordered
        let mb2 = dm_mailbox(them, me);
        let key2 = dm_key(them, me);
        let got = recv_chat(&store, &mb2, &key2, epoch, now);
        assert_eq!(got, vec![("kesslerd".to_string(), "gg wp".to_string()), ("bob".to_string(), "rematch?".to_string())]);
    }

    // "▶ Host" then "▶ Join" across the fleet's real snapcore DHT: hosting publishes a discoverable
    // session + flips activity to InGame; a friend resolves the host's dial target for their own
    // transport. Proves bsgames' join seam against the actual netcode + snapcore primitives.
    #[test]
    fn host_then_join_over_the_fleet_dht() {
        let mut svc = ProfileService::new(FakeDht::default(), SnapcoreStore::new()); // OpenDHT down → snapcore floor
        let mut host = Profile::new("kesslerd", "Kessler");

        let (token, activity) = host_game_session(&mut svc, &mut host, "unendlich", "udp", "10.0.0.9:5000");
        assert_eq!(activity, coord_core::Activity::InGame);
        assert_eq!(token, session_token("kesslerd", "unendlich"));

        // A friend with kesslerd in view joins → gets the data-plane dial for unendlich's transport.
        let dial = join_friend_session(&svc, "kesslerd").expect("host is joinable");
        assert_eq!(dial, ("udp".to_string(), "10.0.0.9:5000".to_string()));

        // Someone not in a game is not joinable.
        let mut idler = Profile::new("bob", "Bob");
        svc.publish(&idler);
        idler.presence = Presence::Online;
        assert!(join_friend_session(&svc, "bob").is_none());
    }
}
