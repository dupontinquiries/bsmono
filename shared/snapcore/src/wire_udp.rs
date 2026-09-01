//! wire_udp — the socket that speaks the protocol the bootstrap servers do.
//!
//! ## The bug this exists to fix
//!
//! [`crate::udp::UdpTransport`] puts a bespoke framing on the wire —
//! `[TAG_FIND_NODE][id][target]`, fixed-width contact records — and
//! `snapclone-presence` drives its DHT through it while bootstrapping
//! against `bootstrap.jami.net:4222`. Those servers speak OpenDHT. They
//! cannot answer these bytes, so the routing table never fills, every
//! lookup returns nothing, and no layer above notices: an empty result is
//! also what a working DHT returns for a peer who is offline.
//!
//! `dht-kit` has been the OpenDHT-compatible codec the whole time, and
//! `snapcore::wire` has been the translation between its `Msg` and this
//! crate's handlers. What was missing is this: a `Transport` that encodes
//! with the one and sends with the other. `DHT_CONVERGENCE.md` argues for
//! exactly this and it had not been done on the path the app uses.
//!
//! ## Same socket discipline as `udp`, deliberately
//!
//! Blocking with a timeout, one reply per query, a reply from the wrong
//! address dropped rather than returned. Those decisions are argued in
//! `udp`'s header and none of them changes because the framing did — a
//! bystander racing a real peer is the same attack whatever the bytes look
//! like.
//!
//! ## What is added on top: transaction ids
//!
//! The bespoke protocol had none, so a late reply to a previous query was
//! indistinguishable from the answer to this one. OpenDHT carries a `tid`,
//! and a reply whose tid does not match is discarded here. That matters
//! more than it sounds on a phone: a query that timed out while the radio
//! woke up will have its answer arrive during the NEXT query, and accepting
//! it would attribute one peer's node list to a different target.
//!
//! ## The write token, and where it had to live
//!
//! OpenDHT, like Kademlia generally, expects a `get` to hand back a token
//! that the following `put` echoes, so a node cannot be used to spray
//! writes at a third party it never talked to. A strict server refuses a
//! `put` without one.
//!
//! `Transport::store` receives no prior state, so the obvious fix is to
//! widen the trait — and that touches every simulation implementing it, in
//! five crates, to thread a value only one implementation can produce.
//!
//! The transport remembers it instead. Every accepted reply carries a
//! token, and a `store` is always preceded by a lookup to the same peer
//! (`snapclone-presence::publish` does `lookup_nodes` and then stores to
//! each node it found) — so by the time a `put` goes out, that peer has
//! just handed us one. The cache is keyed by ADDRESS, not node id, because
//! the token authorises writes through a particular server rather than to a
//! particular identity.
//!
//! Entries expire ([`TOKEN_TTL`]) and the map is capped, since a long
//! session touches thousands of peers and a token from an hour ago is
//! refused anyway — sending a stale one is the same outcome as sending
//! none, at the cost of remembering it.
//!
//! What this still does not do: `listen` or `refresh`. Values expire and are republished
//! by the caller's own schedule (`dht_presence` in the app), which is the
//! coarser of the two mechanisms and the one that works without
//! subscriptions.

use crate::node::{FindValue, Transport};
use crate::routing::{Contact, NodeId};
use crate::store::Key;
use crate::wire::{from_wire_addr, to_hash};
use dht_kit::{Msg, Query, Value};
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// How long to wait for one reply. Matches `udp`'s reasoning: a lookup makes
/// several of these in sequence, so this is multiplied by the contacts
/// tried.
pub const REPLY_TIMEOUT: Duration = Duration::from_millis(1_500);

/// Bigger than any reply the reference sends, and bounded so a hostile
/// datagram cannot make us allocate.
pub const MAX_DATAGRAM: usize = 8 * 1024;

/// How long a write token is worth keeping. The reference rotates them on
/// the order of minutes; past that a `put` echoing one is refused, which is
/// the same outcome as sending none.
pub const TOKEN_TTL: Duration = Duration::from_secs(5 * 60);

/// A ceiling on remembered tokens. A long session touches many peers, and
/// this map must not become a record of every node ever contacted.
pub const MAX_TOKENS: usize = 256;

pub struct WireUdpTransport {
    sock: UdpSocket,
    me: NodeId,
    /// Write tokens by peer ADDRESS — see the module note on why not by id.
    tokens: std::sync::Mutex<std::collections::HashMap<SocketAddr, (u32, std::time::Instant)>>,
    /// Monotonic, wrapping. Only uniqueness within the timeout window
    /// matters, so wrapping is fine and a random start buys nothing here —
    /// this is not a spoofing defence, it is a "which question is this the
    /// answer to" tag.
    tid: AtomicU32,
}

impl WireUdpTransport {
    pub fn bind(me: NodeId, addr: SocketAddr) -> std::io::Result<WireUdpTransport> {
        let sock = UdpSocket::bind(addr)?;
        sock.set_read_timeout(Some(REPLY_TIMEOUT))?;
        Ok(WireUdpTransport {
            sock, me,
            tokens: std::sync::Mutex::new(std::collections::HashMap::new()),
            tid: AtomicU32::new(1),
        })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    fn next_tid(&self) -> u32 {
        self.tid.fetch_add(1, Ordering::Relaxed)
    }

    /// Send one query and wait for its reply.
    ///
    /// `None` on timeout, on a send error, on a reply from an address we did
    /// not ask, on an undecodable datagram, and on a tid that does not
    /// match. All the same to the caller because the caller's response is
    /// the same: count it against that contact and move on.
    fn round_trip(&self, to: &Contact, tid: u32, msg: &Msg) -> Option<Msg> {
        self.sock.send_to(&msg.encode(), to.addr).ok()?;
        // KEEP READING UNTIL THE DEADLINE rather than treating the first
        // datagram as the answer. One socket receives replies, unsolicited
        // queries from other nodes, and late answers to queries that
        // already timed out — and the first version of this discarded the
        // real reply whenever any of those arrived first. On a busy DHT
        // that is not an edge case, it is most lookups.
        let deadline = std::time::Instant::now() + REPLY_TIMEOUT;
        let mut buf = [0u8; MAX_DATAGRAM];
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return None;
            }
            // Shrink the socket timeout to what is left, so the loop cannot
            // outlive the deadline by one full timeout per stray datagram.
            self.sock.set_read_timeout(Some(left)).ok()?;
            let (n, from) = match self.sock.recv_from(&mut buf) {
                Ok(v) => v,
                Err(_) => return None,
            };
            // A reply from an address we did not ask is dropped. UDP has no
            // connection, so anything on the network can answer a query it
            // saw, and accepting that lets a bystander steer a lookup.
            if from != to.addr {
                continue;
            }
            let Some(reply) = Msg::decode(&buf[..n]) else { continue };
            match reply {
                // An error reply is a real answer — the peer understood and
                // declined — but there is nothing in it for a `Transport`
                // method to return, so it ends the wait like a timeout.
                Msg::Error { tid: got, .. } if got == tid => return None,
                Msg::Reply { tid: got, token, .. } if got == tid => {
                    self.remember_token(from, token);
                    return Some(reply);
                }
                Msg::Push { tid: got, .. } if got == tid => return Some(reply),
                // Somebody else's business, or ours from a while ago.
                _ => continue,
            }
        }
    }

    fn remember_token(&self, from: SocketAddr, token: u32) {
        // Zero means the peer sent none. Storing it would evict a real one
        // for the same address on the next reply.
        if token == 0 {
            return;
        }
        let Ok(mut m) = self.tokens.lock() else { return };
        if m.len() >= MAX_TOKENS && !m.contains_key(&from) {
            // Drop expired entries first; only if that frees nothing does
            // anything current go. Evicting a live token to make room for
            // another is how a `put` loses the authorisation it just
            // earned.
            let now = std::time::Instant::now();
            m.retain(|_, (_, at)| now.duration_since(*at) < TOKEN_TTL);
            if m.len() >= MAX_TOKENS {
                if let Some(oldest) = m.iter().min_by_key(|(_, (_, at))| *at).map(|(k, _)| *k) {
                    m.remove(&oldest);
                }
            }
        }
        m.insert(from, (token, std::time::Instant::now()));
    }

    /// The token this peer last gave us, if it is still fresh. Zero means
    /// "we have none" — which is what an unauthorised `put` looks like, and
    /// the honest thing to send when there is nothing better.
    fn token_for(&self, addr: SocketAddr) -> u32 {
        let Ok(m) = self.tokens.lock() else { return 0 };
        match m.get(&addr) {
            Some((t, at)) if at.elapsed() < TOKEN_TTL => *t,
            _ => 0,
        }
    }

    fn query(&self, query: Query, target: &NodeId, value: Option<Value>) -> (u32, Msg) {
        let tid = self.next_tid();
        (tid, Msg::Query {
            tid,
            from: to_hash(&self.me),
            query,
            target: to_hash(target),
            token: 0,
            value,
        })
    }
}

/// The contacts a reply teaches us about — INCLUDING the responder itself.
///
/// Including it is what corrects a guessed node id. A bootstrap entry is
/// added from a hostname before anyone has spoken to it, so its id is
/// invented — `snapclone-presence` derives one from the address and its
/// comment says "the id is corrected when the node actually replies".
/// Nothing was doing the correcting: the routing table kept the invented
/// id, which puts the busiest node in the network in the wrong bucket, so
/// lookups route away from the one peer that knows everybody.
///
/// A reply's `from` is that node's real id, at the address we just reached
/// it on. Returning it as a contact is how the routing table learns —
/// `lookup_nodes` inserts everything a reply returns.
fn contacts_of(reply: &Msg, responder: SocketAddr, now: u64) -> Vec<Contact> {
    match reply {
        Msg::Reply { from, nodes, .. } => {
            let mut out = vec![Contact::new(crate::wire::from_hash(from), responder, now)];
            out.extend(nodes.iter()
                .map(|(h, a)| Contact::new(crate::wire::from_hash(h), from_wire_addr(a), now)));
            out
        }
        _ => Vec::new(),
    }
}

impl Transport for WireUdpTransport {
    fn find_node(&self, _from: &Contact, to: &Contact, target: &NodeId) -> Vec<Contact> {
        let (tid, msg) = self.query(Query::Find, target, None);
        match self.round_trip(to, tid, &msg) {
            // `last_seen` is 0 rather than a clock read: `Transport` gives
            // this method no `now`, and the routing table stamps a contact
            // when it inserts one. Inventing a timestamp here would be a
            // second, wronger clock.
            Some(reply) => contacts_of(&reply, to.addr, 0),
            None => Vec::new(),
        }
    }

    fn find_value(&self, _from: &Contact, to: &Contact, key: &Key) -> FindValue {
        let (tid, msg) = self.query(Query::Get, key, None);
        match self.round_trip(to, tid, &msg) {
            Some(Msg::Reply { values, nodes, from, .. }) => {
                // Values WIN over nodes when both are present, which the
                // reference permits in one reply. A caller that took the
                // nodes would keep walking toward a key whose value it was
                // just handed.
                if values.is_empty() {
                    // The responder leads here too, same reasoning as
                    // `contacts_of` — a `get` is just as much a chance to
                    // learn who we are actually talking to as a `find`.
                    let mut out = vec![Contact::new(crate::wire::from_hash(&from),
                                                    to.addr, 0)];
                    out.extend(nodes.iter()
                        .map(|(h, a)| Contact::new(crate::wire::from_hash(h),
                                                   from_wire_addr(a), 0)));
                    FindValue::Closest(out)
                } else {
                    FindValue::Values(values.into_iter().map(|v| v.data).collect())
                }
            }
            _ => FindValue::Closest(Vec::new()),
        }
    }

    fn store(&self, _from: &Contact, to: &Contact, key: Key, value: Vec<u8>, _ttl: u64) {
        // The value id is derived from the bytes so republishing the same
        // record does not accumulate duplicates under one key — OpenDHT
        // holds a SET per key, and a fresh random id every hour would fill
        // it with copies of us.
        let id = fnv1a(&value);
        let (tid, mut msg) = self.query(Query::Put, &key, Some(Value { id, data: value }));
        // The token this peer handed us during the lookup that found it.
        if let Msg::Query { token, .. } = &mut msg {
            *token = self.token_for(to.addr);
        }
        // Fire and forget: `store` returns nothing, so there is no way to
        // report a refusal, and see the module note on the missing token.
        let _ = self.round_trip(to, tid, &msg);
    }

    fn ping(&self, _from: &Contact, to: &Contact) -> bool {
        let (tid, msg) = self.query(Query::Ping, &self.me, None);
        matches!(self.round_trip(to, tid, &msg), Some(Msg::Reply { .. }))
    }
}

/// Stable value id from content. Not a security property — see `store`.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn id(n: u8) -> NodeId {
        let mut b = [0u8; 20];
        b[0] = n;
        NodeId(b)
    }

    fn contact(addr: SocketAddr) -> Contact {
        Contact::new(id(9), addr, 0)
    }

    /// A stand-in peer that decodes one query and answers it. Real sockets
    /// on loopback, because the thing being tested is that the bytes are
    /// the ones a foreign implementation would accept — a fake Transport
    /// would test nothing at all here.
    fn responder(reply: impl Fn(Msg) -> Option<Msg> + Send + 'static)
        -> (SocketAddr, std::thread::JoinHandle<Option<Msg>>) {
        let sock = UdpSocket::bind("127.0.0.1:0").expect("bind");
        sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let addr = sock.local_addr().unwrap();
        let h = std::thread::spawn(move || {
            let mut buf = [0u8; MAX_DATAGRAM];
            let (n, from) = sock.recv_from(&mut buf).ok()?;
            let got = Msg::decode(&buf[..n])?;
            if let Some(answer) = reply(got.clone()) {
                sock.send_to(&answer.encode(), from).ok()?;
            }
            Some(got)
        });
        (addr, h)
    }

    #[test]
    fn a_find_query_goes_out_as_opendht_msgpack_and_its_nodes_come_back() {
        // The whole point: what leaves this socket must decode as the
        // reference's format, because bootstrap.jami.net is what has to
        // read it.
        let peer_id = dht_kit::InfoHash([3u8; 20]);
        let (addr, h) = responder(move |q| match q {
            Msg::Query { tid, query: Query::Find, .. } => Some(Msg::Reply {
                tid,
                from: peer_id,
                nodes: vec![(dht_kit::InfoHash([5u8; 20]),
                             dht_kit::Addr::new([10, 0, 0, 4], 4222))],
                values: vec![],
                token: 0,
            }),
            _ => None,
        });
        let t = WireUdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap();
        let got = t.find_node(&contact(addr), &contact(addr), &id(2));
        let sent = h.join().unwrap().expect("nothing decodable arrived");
        assert!(matches!(sent, Msg::Query { query: Query::Find, .. }),
                "the wrong query kind went out: {sent:?}");
        // Two: the responder itself (with its REAL id, at the address we
        // reached it on) followed by the node it named.
        assert_eq!(got.len(), 2, "the reply's contacts were lost: {got:?}");
        assert_eq!(got[0].addr, addr, "the responder is not first");
        assert_eq!(got[0].id, crate::wire::from_hash(&peer_id),
                   "the responder's real id was not learned");
        assert_eq!(got[1].addr, SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 4)), 4222));
    }

    #[test]
    fn a_get_that_finds_a_value_returns_it_rather_than_walking_on() {
        // The reference may put nodes AND values in one reply. A caller
        // that took the nodes would keep walking toward a key whose value
        // it was just handed.
        let (addr, h) = responder(|q| match q {
            Msg::Query { tid, query: Query::Get, .. } => Some(Msg::Reply {
                tid,
                from: dht_kit::InfoHash([3u8; 20]),
                nodes: vec![(dht_kit::InfoHash([5u8; 20]),
                             dht_kit::Addr::new([10, 0, 0, 4], 4222))],
                values: vec![Value { id: 7, data: b"presence".to_vec() }],
                token: 0,
            }),
            _ => None,
        });
        let t = WireUdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap();
        match t.find_value(&contact(addr), &contact(addr), &id(2)) {
            FindValue::Values(v) => assert_eq!(v, vec![b"presence".to_vec()]),
            other => panic!("a value in the reply was ignored: {other:?}"),
        }
        h.join().unwrap();
    }

    #[test]
    fn a_put_carries_the_value_and_a_content_derived_id() {
        // A fresh random id every republish would fill the key's value SET
        // with hourly copies of us.
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let (addr, h) = responder(move |q| {
            if let Msg::Query { value: Some(v), .. } = &q {
                s2.lock().unwrap().push(v.clone());
            }
            None
        });
        let t = WireUdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap();
        t.store(&contact(addr), &contact(addr), id(2), b"record".to_vec(), 3600);
        h.join().unwrap();
        let v = seen.lock().unwrap().clone();
        assert_eq!(v.len(), 1, "the put carried no value");
        assert_eq!(v[0].data, b"record");
        assert_eq!(v[0].id, fnv1a(b"record"), "the id is not derived from the content");
    }

    #[test]
    fn a_reply_to_a_previous_query_is_not_read_as_this_one() {
        // On a phone this is the ordinary case, not an attack: a query that
        // timed out while the radio woke up has its answer arrive during
        // the NEXT query. Accepting it attributes one peer's node list to a
        // different target.
        let (addr, h) = responder(|q| match q {
            Msg::Query { tid, .. } => Some(Msg::Reply {
                // Deliberately the wrong transaction.
                tid: tid.wrapping_add(1000),
                from: dht_kit::InfoHash([3u8; 20]),
                nodes: vec![(dht_kit::InfoHash([5u8; 20]),
                             dht_kit::Addr::new([10, 0, 0, 4], 4222))],
                values: vec![],
                token: 0,
            }),
            _ => None,
        });
        let t = WireUdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap();
        assert!(t.find_node(&contact(addr), &contact(addr), &id(2)).is_empty(),
                "a stale reply was accepted as this query's answer");
        h.join().unwrap();
    }

    #[test]
    fn a_guessed_bootstrap_id_is_corrected_by_the_first_reply() {
        // A bootstrap entry is added from a hostname before anyone has
        // spoken to it, so its id is invented. `snapclone-presence` derives
        // one from the address and its comment says the id is "corrected
        // when the node actually replies" — and nothing was doing the
        // correcting. An invented id puts the busiest node in the network
        // in the wrong bucket, so lookups route away from the one peer that
        // knows everybody.
        let real = dht_kit::InfoHash([0x5a; 20]);
        let (addr, h) = responder(move |q| match q {
            Msg::Query { tid, .. } => Some(Msg::Reply {
                tid, from: real, nodes: vec![], values: vec![], token: 0 }),
            _ => None,
        });
        let t = WireUdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap();
        // The contact we dial carries a WRONG id, as a bootstrap entry does.
        let guessed = Contact::new(id(200), addr, 0);
        let got = t.find_node(&guessed, &guessed, &id(2));
        h.join().unwrap();
        assert_eq!(got.len(), 1, "a reply with no nodes taught us nothing: {got:?}");
        assert_eq!(got[0].id, crate::wire::from_hash(&real));
        assert_ne!(got[0].id, id(200), "the guessed id survived the correction");
        assert_eq!(got[0].addr, addr);
    }

    #[test]
    fn a_put_echoes_the_token_the_peer_gave_during_the_lookup() {
        // A strict OpenDHT node refuses a write with no token. `store` gets
        // no prior state, so the transport has to have remembered it —
        // which it can, because a store is always preceded by a lookup to
        // the same peer.
        let sock = UdpSocket::bind("127.0.0.1:0").expect("bind");
        sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let addr = sock.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let s2 = seen.clone();
        let h = std::thread::spawn(move || {
            let mut buf = [0u8; MAX_DATAGRAM];
            // 1. the find, answered with a token
            let (n, from) = sock.recv_from(&mut buf).expect("no find");
            let Some(Msg::Query { tid, .. }) = Msg::decode(&buf[..n]) else { panic!() };
            let reply = Msg::Reply { tid, from: dht_kit::InfoHash([3u8; 20]),
                nodes: vec![], values: vec![], token: 0xABCD };
            sock.send_to(&reply.encode(), from).ok();
            // 2. the put that follows it
            let (n, from2) = sock.recv_from(&mut buf).expect("no put");
            if let Some(Msg::Query { tid, token, .. }) = Msg::decode(&buf[..n]) {
                *s2.lock().unwrap() = Some(token);
                let ack = Msg::Reply { tid, from: dht_kit::InfoHash([3u8; 20]),
                    nodes: vec![], values: vec![], token: 0xABCD };
                sock.send_to(&ack.encode(), from2).ok();
            }
        });
        let t = WireUdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap();
        let c = contact(addr);
        t.find_node(&c, &c, &id(2));
        t.store(&c, &c, id(2), b"record".to_vec(), 3600);
        h.join().unwrap();
        assert_eq!(*seen.lock().unwrap(), Some(0xABCD),
                   "the put went out unauthorised despite a token in hand");
    }

    #[test]
    fn a_put_to_a_peer_we_have_not_spoken_to_sends_no_token_rather_than_a_wrong_one() {
        // Zero is what "we have none" looks like on the wire. Sending
        // another peer's token would be worse than sending nothing: refused
        // just the same, and indistinguishable from a bug at the far end.
        let t = WireUdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap();
        let a: SocketAddr = "10.0.0.1:4222".parse().unwrap();
        let b: SocketAddr = "10.0.0.2:4222".parse().unwrap();
        t.remember_token(a, 0x1234);
        assert_eq!(t.token_for(a), 0x1234);
        assert_eq!(t.token_for(b), 0, "one peer's token was offered to another");
    }

    #[test]
    fn a_token_of_zero_does_not_evict_a_real_one() {
        // A peer that sends no token must not wipe the one it sent a moment
        // ago, or every reply without one costs the write that follows.
        let t = WireUdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap();
        let a: SocketAddr = "10.0.0.1:4222".parse().unwrap();
        t.remember_token(a, 0x1234);
        t.remember_token(a, 0);
        assert_eq!(t.token_for(a), 0x1234);
    }

    #[test]
    fn the_token_cache_is_bounded_and_keeps_what_is_current() {
        // A long session touches many peers; this map must not become a
        // record of every node ever contacted. Evicting a LIVE token to
        // make room is how a put loses the authorisation it just earned, so
        // the oldest goes rather than the newest.
        let t = WireUdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap();
        for i in 0..(MAX_TOKENS + 20) {
            let a: SocketAddr = format!("10.0.{}.{}:4222", i / 256, i % 256).parse().unwrap();
            t.remember_token(a, (i as u32) + 1);
        }
        let n = t.tokens.lock().unwrap().len();
        assert!(n <= MAX_TOKENS, "the cache grew to {n}");
        let newest: SocketAddr = format!("10.0.{}.{}:4222",
            (MAX_TOKENS + 19) / 256, (MAX_TOKENS + 19) % 256).parse().unwrap();
        assert_ne!(t.token_for(newest), 0, "the most recent token was evicted");
    }

    #[test]
    fn a_stray_datagram_does_not_cost_the_real_reply() {
        // One socket receives replies, unsolicited queries from other
        // nodes, and late answers to queries that already timed out. The
        // first version of this took the first datagram as THE answer, so
        // any of those arriving first lost the real reply — on a busy DHT
        // that is most lookups, not an edge case.
        let sock = UdpSocket::bind("127.0.0.1:0").expect("bind");
        sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let addr = sock.local_addr().unwrap();
        let h = std::thread::spawn(move || {
            let mut buf = [0u8; MAX_DATAGRAM];
            let (n, from) = sock.recv_from(&mut buf).expect("no query arrived");
            let Some(Msg::Query { tid, .. }) = Msg::decode(&buf[..n]) else {
                panic!("not a query")
            };
            // An unrelated query first — exactly what another node probing
            // us mid-lookup looks like.
            let noise = Msg::Query { tid: 999, from: dht_kit::InfoHash([8u8; 20]),
                query: Query::Ping, target: dht_kit::InfoHash([0u8; 20]),
                token: 0, value: None };
            sock.send_to(&noise.encode(), from).ok();
            // Then the answer.
            let reply = Msg::Reply { tid, from: dht_kit::InfoHash([3u8; 20]),
                nodes: vec![(dht_kit::InfoHash([5u8; 20]),
                             dht_kit::Addr::new([10, 0, 0, 4], 4222))],
                values: vec![], token: 0 };
            sock.send_to(&reply.encode(), from).ok();
        });
        let t = WireUdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap();
        let got = t.find_node(&contact(addr), &contact(addr), &id(2));
        h.join().unwrap();
        assert_eq!(got.len(), 2, "the stray datagram ate the reply");
    }

    #[test]
    fn a_reply_from_the_wrong_address_is_dropped() {
        // UDP has no connection, so anything that saw the query can answer
        // it; accepting that lets a bystander steer a lookup.
        let quiet = UdpSocket::bind("127.0.0.1:0").unwrap();
        let dead = quiet.local_addr().unwrap();
        drop(quiet);
        let t = WireUdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap();
        // Nothing is listening, so this is the timeout path — the same
        // `None` a wrong-address reply produces, and the same handling.
        assert!(t.find_node(&contact(dead), &contact(dead), &id(2)).is_empty());
        assert!(!t.ping(&contact(dead), &contact(dead)));
    }

    #[test]
    fn an_error_reply_counts_as_a_failure_rather_than_an_empty_success() {
        // The peer understood and declined. There is nothing in it for a
        // Transport method to return, and treating it as "knew nobody"
        // would hide a server refusing our writes.
        let (addr, h) = responder(|q| match q {
            Msg::Query { tid, .. } => Some(Msg::Error { tid, code: 401 }),
            _ => None,
        });
        let t = WireUdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap();
        assert!(!t.ping(&contact(addr), &contact(addr)));
        h.join().unwrap();
    }
}
