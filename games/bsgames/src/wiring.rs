//! wiring — the I/O seams, connected to the fleet's existing shared code rather than reinvented:
//! * **presence** rides coord-core's `Activity` (the same bus every fleet app publishes to), so
//!   launching a game shows up fleet-wide, not just in bsgames;
//! * **chat** rides share-kit's `Mailbox` over snapcore's `Store` — the fleet's Kademlia DHT — so
//!   messages are sealed, content-addressed, and delivered exactly like every other fleet share.
use netcode::microdht::NodeId;
use netcode::profile::Presence;
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
    NodeId::from_key(&dm_id(a, b)).0
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

#[cfg(test)]
mod tests {
    use super::*;
    use netcode::profile::Presence;
    use snapcore::store::Store;

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
}
