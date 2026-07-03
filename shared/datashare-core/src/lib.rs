//! datashare-core — the cross-app data bus.
//!
//! Apps keep re-implementing "let another app see this": snapclone contacts, unendlich
//! session state, voiceterm draft handoff, "now playing" from beatspace/bo-suite, a
//! clipboard-like "open this here". This crate is the shared brain of a single daemon that
//! holds that data once — a **namespaced key/value store with pub-sub** — so an app writes
//! `now_playing/track` and any subscriber is notified, with no direct app-to-app coupling.
//!
//! Transport is ipc-kit (apps try the daemon socket first, fall back to private state if
//! it's down). Durability and cross-device federation come from share-kit's DHT mailbox —
//! this in-memory [`Hub`] is the local, synchronous layer that share-kit backs.

use std::collections::HashMap;

/// A subscriber handle (opaque id the transport maps to a connection).
pub type SubId = u64;

/// The result of handling one bus operation: an optional reply to the caller plus any
/// notifications to fan out to subscribers. The transport turns these into ipc-kit
/// messages.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Effects {
    /// Reply to the requester (e.g. a `get` value or an `ok`).
    pub reply: Option<Op>,
    /// (subscriber, event) pairs to push.
    pub notify: Vec<(SubId, Op)>,
}

/// Bus operations — also the wire verbs (see [`Op::verb`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Put { ns: String, key: String, val: String },
    Get { ns: String, key: String },
    Value { ns: String, key: String, val: Option<String> },
    Delete { ns: String, key: String },
    List { ns: String },
    Listing { ns: String, entries: Vec<(String, String)> },
    Sub { ns: String },
    /// Pushed to subscribers when a key in their namespace changes.
    Event { ns: String, key: String, val: Option<String> },
    Ok,
}

impl Op {
    pub fn verb(&self) -> &'static str {
        match self {
            Op::Put { .. } => "put",
            Op::Get { .. } => "get",
            Op::Value { .. } => "value",
            Op::Delete { .. } => "del",
            Op::List { .. } => "list",
            Op::Listing { .. } => "listing",
            Op::Sub { .. } => "sub",
            Op::Event { .. } => "event",
            Op::Ok => "ok",
        }
    }
}

/// The in-memory cross-app store + subscription table.
#[derive(Default)]
pub struct Hub {
    // ns -> (key -> val)
    data: HashMap<String, HashMap<String, String>>,
    // ns -> subscribers
    subs: HashMap<String, Vec<SubId>>,
}

impl Hub {
    pub fn new() -> Self {
        Hub::default()
    }

    /// Handle one op from `caller`, mutating state and returning the reply + notifications.
    pub fn handle(&mut self, caller: SubId, op: Op) -> Effects {
        match op {
            Op::Put { ns, key, val } => {
                self.data.entry(ns.clone()).or_default().insert(key.clone(), val.clone());
                let notify = self.fanout(&ns, caller, Op::Event { ns: ns.clone(), key, val: Some(val) });
                Effects { reply: Some(Op::Ok), notify }
            }
            Op::Delete { ns, key } => {
                let existed = self.data.get_mut(&ns).map(|m| m.remove(&key).is_some()).unwrap_or(false);
                let notify = if existed {
                    self.fanout(&ns, caller, Op::Event { ns: ns.clone(), key, val: None })
                } else {
                    vec![]
                };
                Effects { reply: Some(Op::Ok), notify }
            }
            Op::Get { ns, key } => {
                let val = self.data.get(&ns).and_then(|m| m.get(&key)).cloned();
                Effects { reply: Some(Op::Value { ns, key, val }), notify: vec![] }
            }
            Op::List { ns } => {
                let mut entries: Vec<(String, String)> = self
                    .data
                    .get(&ns)
                    .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default();
                entries.sort();
                Effects { reply: Some(Op::Listing { ns, entries }), notify: vec![] }
            }
            Op::Sub { ns } => {
                let subs = self.subs.entry(ns).or_default();
                if !subs.contains(&caller) {
                    subs.push(caller);
                }
                Effects { reply: Some(Op::Ok), notify: vec![] }
            }
            // reply/event-shaped ops aren't inbound requests
            _ => Effects { reply: Some(Op::Ok), notify: vec![] },
        }
    }

    /// Drop a subscriber (on disconnect).
    pub fn drop_sub(&mut self, id: SubId) {
        for subs in self.subs.values_mut() {
            subs.retain(|s| *s != id);
        }
    }

    /// A sorted snapshot of all key/values in a namespace (for persistence / inspection).
    pub fn snapshot(&self, ns: &str) -> Vec<(String, String)> {
        let mut entries: Vec<(String, String)> = self
            .data
            .get(ns)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        entries.sort();
        entries
    }

    fn fanout(&self, ns: &str, exclude: SubId, event: Op) -> Vec<(SubId, Op)> {
        self.subs
            .get(ns)
            .map(|subs| subs.iter().filter(|s| **s != exclude).map(|s| (*s, event.clone())).collect())
            .unwrap_or_default()
    }
}

/// Wire mapping between ipc-kit `Message`s and datashare `Op`s — shared by the sharemd and
/// settingsd daemons so the protocol lives in one place.
pub mod wire {
    use super::Op;
    use ipc_kit::Message;

    /// `Message` → `Op` (unknown verbs become `Op::Ok` no-ops).
    pub fn to_op(m: &Message) -> Op {
        let a = |i: usize| m.args.get(i).cloned().unwrap_or_default();
        match m.verb.as_str() {
            "put" => Op::Put { ns: a(0), key: a(1), val: a(2) },
            "get" => Op::Get { ns: a(0), key: a(1) },
            "del" => Op::Delete { ns: a(0), key: a(1) },
            "list" => Op::List { ns: a(0) },
            "sub" => Op::Sub { ns: a(0) },
            _ => Op::Ok,
        }
    }

    /// `Op` → `Message`. Optional values use arg-count: `value`/`event` with 2 args is `None`,
    /// 3 args is `Some`.
    pub fn to_msg(op: &Op) -> Message {
        match op {
            Op::Ok => Message::verb("ok"),
            Op::Value { ns, key, val } => {
                let mut args = vec![ns.clone(), key.clone()];
                if let Some(v) = val {
                    args.push(v.clone());
                }
                Message::new("value", args)
            }
            Op::Listing { ns, entries } => {
                let mut args = vec![ns.clone()];
                for (k, v) in entries {
                    args.push(k.clone());
                    args.push(v.clone());
                }
                Message::new("listing", args)
            }
            Op::Event { ns, key, val } => {
                let mut args = vec![ns.clone(), key.clone()];
                if let Some(v) = val {
                    args.push(v.clone());
                }
                Message::new("event", args)
            }
            _ => Message::verb("ok"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_then_get_across_callers() {
        let mut h = Hub::new();
        h.handle(1, Op::Put { ns: "now_playing".into(), key: "track".into(), val: "koYopper".into() });
        let e = h.handle(2, Op::Get { ns: "now_playing".into(), key: "track".into() });
        assert_eq!(e.reply, Some(Op::Value { ns: "now_playing".into(), key: "track".into(), val: Some("koYopper".into()) }));
    }

    #[test]
    fn subscribers_get_notified_except_the_writer() {
        let mut h = Hub::new();
        h.handle(10, Op::Sub { ns: "contacts".into() });
        h.handle(11, Op::Sub { ns: "contacts".into() });
        // caller 11 writes → 10 is notified, 11 (the writer) is not
        let e = h.handle(11, Op::Put { ns: "contacts".into(), key: "alice".into(), val: "online".into() });
        assert_eq!(e.reply, Some(Op::Ok));
        assert_eq!(e.notify, vec![(10, Op::Event { ns: "contacts".into(), key: "alice".into(), val: Some("online".into()) })]);
    }

    #[test]
    fn delete_notifies_with_none_and_list_is_sorted() {
        let mut h = Hub::new();
        h.handle(1, Op::Put { ns: "s".into(), key: "b".into(), val: "2".into() });
        h.handle(1, Op::Put { ns: "s".into(), key: "a".into(), val: "1".into() });
        let e = h.handle(2, Op::List { ns: "s".into() });
        assert_eq!(e.reply, Some(Op::Listing { ns: "s".into(), entries: vec![("a".into(), "1".into()), ("b".into(), "2".into())] }));
        h.handle(9, Op::Sub { ns: "s".into() });
        let d = h.handle(1, Op::Delete { ns: "s".into(), key: "a".into() });
        assert_eq!(d.notify, vec![(9, Op::Event { ns: "s".into(), key: "a".into(), val: None })]);
    }

    #[test]
    fn dropped_subscriber_stops_receiving() {
        let mut h = Hub::new();
        h.handle(5, Op::Sub { ns: "n".into() });
        h.drop_sub(5);
        let e = h.handle(6, Op::Put { ns: "n".into(), key: "k".into(), val: "v".into() });
        assert!(e.notify.is_empty());
    }
}
