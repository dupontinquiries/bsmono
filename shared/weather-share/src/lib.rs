//! weather-share — an iOS-like, per-contact exposed database for sharing weather with snapclone
//! contacts.
//!
//! The model mirrors how iOS shares an album or a location: sharing is **granular and revocable**.
//! Each `(record, contact)` grant is explicit; the recipient sees *exactly* what's shared with them and
//! nothing else; a revoke removes it from their view like "Stop Sharing". Two contacts never see each
//! other's shares.
//!
//! It's a thin composition over the fleet's existing kits rather than new machinery:
//!   * [`datashare_core::Hub`] — the live namespaced pub/sub DB. Each recipient has a private namespace
//!     `wx:{owner}:{contact}`; sharing is a `Put` there, revoking a `Delete`, and a subscribed contact
//!     gets live `Event`s. This is the "exposed database".
//!   * [`share_kit::Mailbox`] — durable, offline delivery of a share to a contact that's currently
//!     away, over the DHT. [`WeatherShare::mailbox_for`] hands back the contact's mailbox.
//!   * [`snapcore::Store`] — the mesh store that backs the mailbox (`share-kit` already implements its
//!     `Backend` for it), so a share reaches the contact through the snap network.
//!
//! Contacts are referenced by their snapclone sc-ID string, so this kit stays app-independent (no
//! dependency back onto snapclone); the snapclone side passes `contact.id`.

use datashare_core::{Hub, Op, SubId};
use std::collections::{HashMap, HashSet};

/// A shareable weather reading — the shape ambient-learner's forecast produces.
#[derive(Clone, Debug, PartialEq)]
pub struct WeatherRecord {
    pub location: String,
    pub temp_c: f32,
    pub condition: String,
    /// Unix seconds the reading was taken.
    pub updated: u64,
}

impl WeatherRecord {
    pub fn new(location: impl Into<String>, temp_c: f32, condition: impl Into<String>, updated: u64) -> Self {
        Self { location: location.into(), temp_c, condition: condition.into(), updated }
    }

    /// Encode to the Hub's string value (unit-separated) so it round-trips through `Op::Put`/`Value`.
    pub fn encode(&self) -> String {
        format!("{}\u{1f}{}\u{1f}{}\u{1f}{}", self.location, self.temp_c, self.condition, self.updated)
    }

    /// Decode a Hub value back into a record. Returns `None` on a malformed value.
    pub fn decode(s: &str) -> Option<WeatherRecord> {
        let mut p = s.split('\u{1f}');
        let location = p.next()?.to_string();
        let temp_c = p.next()?.parse().ok()?;
        let condition = p.next()?.to_string();
        let updated = p.next()?.parse().ok()?;
        Some(WeatherRecord { location, temp_c, condition, updated })
    }
}

/// One owner's exposed weather DB and the per-contact grants over it.
pub struct WeatherShare {
    hub: Hub,
    owner: String,
    owner_sub: SubId,
    /// record key -> contacts it's shared with.
    grants: HashMap<String, HashSet<String>>,
    /// contact id -> their live-events subscription id.
    subs: HashMap<String, SubId>,
    next_sub: SubId,
}

impl WeatherShare {
    /// A new exposed DB owned by `owner` (their snapclone sc-ID).
    pub fn new(owner: impl Into<String>) -> Self {
        Self {
            hub: Hub::new(),
            owner: owner.into(),
            owner_sub: 1,
            grants: HashMap::new(),
            subs: HashMap::new(),
            next_sub: 2,
        }
    }

    /// The private namespace a contact reads to see weather this owner shared *with them*.
    fn ns_for(&self, contact_id: &str) -> String {
        format!("wx:{}:{}", self.owner, contact_id)
    }

    /// A contact subscribes to receive live share/revoke events (returns their `SubId`). Idempotent.
    /// After subscribing, [`share_with`]/[`revoke`] return the `Event`s pushed to them.
    pub fn subscribe(&mut self, contact_id: &str) -> SubId {
        if let Some(&s) = self.subs.get(contact_id) {
            return s;
        }
        let sub = self.next_sub;
        self.next_sub += 1;
        self.subs.insert(contact_id.to_string(), sub);
        self.hub.handle(sub, Op::Sub { ns: self.ns_for(contact_id) });
        sub
    }

    /// Share a weather record with a contact ("Share With…"). Idempotent — re-sharing the same key
    /// updates the value. Returns the live `Event`s pushed to subscribers of that contact's namespace.
    pub fn share_with(&mut self, contact_id: &str, key: &str, rec: &WeatherRecord) -> Vec<(SubId, Op)> {
        self.grants.entry(key.to_string()).or_default().insert(contact_id.to_string());
        self.hub
            .handle(
                self.owner_sub,
                Op::Put { ns: self.ns_for(contact_id), key: key.to_string(), val: rec.encode() },
            )
            .notify
    }

    /// Stop sharing a record with a contact ("Stop Sharing"). Removes it from their view and returns
    /// the revoke `Event`.
    pub fn revoke(&mut self, contact_id: &str, key: &str) -> Vec<(SubId, Op)> {
        if let Some(set) = self.grants.get_mut(key) {
            set.remove(contact_id);
        }
        self.hub
            .handle(self.owner_sub, Op::Delete { ns: self.ns_for(contact_id), key: key.to_string() })
            .notify
    }

    /// Who a record is currently shared with (sorted contact ids).
    pub fn shared_with(&self, key: &str) -> Vec<String> {
        let mut v: Vec<String> = self.grants.get(key).map(|s| s.iter().cloned().collect()).unwrap_or_default();
        v.sort();
        v
    }

    /// The recipient's view: every weather record shared with `contact_id`, decoded and sorted by key.
    pub fn visible_to(&self, contact_id: &str) -> Vec<(String, WeatherRecord)> {
        let mut out: Vec<(String, WeatherRecord)> = self
            .hub
            .snapshot(&self.ns_for(contact_id))
            .into_iter()
            .filter_map(|(k, v)| WeatherRecord::decode(&v).map(|r| (k, r)))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// A durable [`share_kit::Mailbox`] for delivering shares to `contact_id` while they're offline,
    /// backed by the snap mesh ([`snapcore::Store`] implements `share-kit`'s `Backend`). The caller
    /// seals + posts the encoded record and the contact fetches it on reconnect.
    pub fn mailbox_for(&self, contact_id: &str) -> share_kit::Mailbox {
        share_kit::Mailbox::new("wx", contact_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(loc: &str, t: f32) -> WeatherRecord {
        WeatherRecord::new(loc, t, "clear", 1_700_000_000)
    }

    #[test]
    fn record_round_trips_through_the_hub_value() {
        let r = WeatherRecord::new("Montréal", -4.5, "snow", 1_700_000_123);
        assert_eq!(WeatherRecord::decode(&r.encode()), Some(r));
        assert_eq!(WeatherRecord::decode("garbage"), None);
    }

    #[test]
    fn share_then_recipient_sees_exactly_what_was_shared() {
        let mut ws = WeatherShare::new("sc-owner");
        ws.share_with("sc-alice", "home", &rec("Montréal", 21.0));
        ws.share_with("sc-alice", "cottage", &rec("Tremblant", 17.5));

        let seen = ws.visible_to("sc-alice");
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].0, "cottage"); // sorted by key
        assert_eq!(seen[1].1.location, "Montréal");
        assert_eq!(ws.shared_with("home"), vec!["sc-alice"]);
    }

    #[test]
    fn contacts_are_isolated_from_each_others_shares() {
        let mut ws = WeatherShare::new("sc-owner");
        ws.share_with("sc-alice", "home", &rec("Montréal", 21.0));
        ws.share_with("sc-bob", "home", &rec("Toronto", 19.0));
        // each contact sees only their own value under the same key
        assert_eq!(ws.visible_to("sc-alice")[0].1.location, "Montréal");
        assert_eq!(ws.visible_to("sc-bob")[0].1.location, "Toronto");
        assert_eq!(ws.shared_with("home"), vec!["sc-alice", "sc-bob"]);
    }

    #[test]
    fn revoke_removes_it_from_the_recipient_view_and_grants() {
        let mut ws = WeatherShare::new("sc-owner");
        ws.share_with("sc-alice", "home", &rec("Montréal", 21.0));
        ws.revoke("sc-alice", "home");
        assert!(ws.visible_to("sc-alice").is_empty());
        assert!(ws.shared_with("home").is_empty());
    }

    #[test]
    fn subscribers_get_live_share_and_revoke_events() {
        let mut ws = WeatherShare::new("sc-owner");
        let sub = ws.subscribe("sc-alice");

        let ev = ws.share_with("sc-alice", "home", &rec("Montréal", 21.0));
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, sub);
        match &ev[0].1 {
            Op::Event { key, val: Some(_), .. } => assert_eq!(key, "home"),
            other => panic!("expected a share Event, got {other:?}"),
        }

        let ev = ws.revoke("sc-alice", "home");
        assert_eq!(ev.len(), 1);
        assert!(matches!(&ev[0].1, Op::Event { val: None, .. })); // a revoke pushes val: None
    }

    #[test]
    fn mailbox_is_namespaced_per_contact_for_offline_delivery() {
        let ws = WeatherShare::new("sc-owner");
        // distinct contacts get distinct mailboxes (delivery isolation); this just has to construct.
        let _a = ws.mailbox_for("sc-alice");
        let _b = ws.mailbox_for("sc-bob");
    }
}
