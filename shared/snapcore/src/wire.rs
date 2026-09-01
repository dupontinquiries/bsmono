//! wire — packets in, handler calls out.
//!
//! `snapcore` is a sans-io engine: its handlers take `Contact`, `NodeId` and
//! `Vec<u8>`, so there has never been anything to put on a socket. `dht-kit` is
//! the OpenDHT-compatible codec with no engine. This module is the seam, and
//! it is the first step of the convergence recorded in
//! `maintenance/DHT_CONVERGENCE.md`.
//!
//! ## What this does and does not do
//!
//! It translates: a decoded `dht_kit::Msg` becomes the right `Node::handle_*`
//! call, and the answer becomes a `Msg` to send back, laid out the way the
//! reference's `sendPong` / `sendNodesValues` / `sendValueAnnounced` /
//! `sendListenConfirmation` lay theirs out. It does **not** own a socket,
//! retransmit, or decide when to query — those belong to a transport and to
//! the lookup driver respectively, and putting them here would make this the
//! third engine rather than the bridge between the two that exist.
//!
//! ## Why a `Responder` rather than a free function
//!
//! Answering a `find` or `get` means issuing a write token, and checking a
//! `put` means verifying one. The first version handed out `0` for every
//! token and accepted anything, which is not a protocol; `guard::TokenRing`
//! had the policy and nothing called it. A responder owns the ring, and its
//! `handle` is the one place tokens are minted and checked.
//!
//! Tokens are bound to the requester's ip and the key, not its port — a NAT
//! rewrites the port between a `get` and the `put` that follows, and binding
//! to it would fail exactly the peers that need this most (`guard.rs`).

use crate::guard::TokenRing;
use crate::node::{FindValue, Node};
use crate::routing::{Contact, NodeId, ID_LEN};
use crate::store::Key;
use dht_kit::{Addr as WireAddr, InfoHash, Ip, Msg, Query, Value, WANT4, WANT6};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// snapcore ids and dht-kit hashes are both 160-bit, so this is a rename rather
/// than a conversion — asserted by the compiler through `ID_LEN`.
pub fn to_hash(id: &NodeId) -> InfoHash {
    let mut h = [0u8; ID_LEN];
    h.copy_from_slice(&id.0);
    InfoHash(h)
}

pub fn from_hash(h: &InfoHash) -> NodeId {
    let mut b = [0u8; ID_LEN];
    b.copy_from_slice(&h.0);
    NodeId(b)
}

pub fn to_wire_addr(a: &SocketAddr) -> WireAddr {
    match a.ip() {
        IpAddr::V4(v4) => WireAddr::V4(v4.octets(), a.port()),
        IpAddr::V6(v6) => WireAddr::V6(v6.octets(), a.port()),
    }
}

pub fn from_wire_addr(a: &WireAddr) -> SocketAddr {
    match a {
        WireAddr::V4(ip, p) => SocketAddr::new(IpAddr::V4(Ipv4Addr::from(*ip)), *p),
        WireAddr::V6(ip, p) => SocketAddr::new(IpAddr::V6(Ipv6Addr::from(*ip)), *p),
    }
}

pub fn to_ip(a: &SocketAddr) -> Ip {
    match a.ip() {
        IpAddr::V4(v4) => Ip::V4(v4.octets()),
        IpAddr::V6(v6) => Ip::V6(v6.octets()),
    }
}

pub fn from_ip(ip: &Ip) -> IpAddr {
    match ip {
        Ip::V4(b) => IpAddr::V4(Ipv4Addr::from(*b)),
        Ip::V6(b) => IpAddr::V6(Ipv6Addr::from(*b)),
    }
}

/// A stable value id from its bytes.
///
/// snapcore's store holds opaque bytes with no id of its own, and OpenDHT
/// needs one per value: a put names the id it stored, a refresh names the id
/// to keep alive, and a key holds a SET keyed by id. Deriving the id from
/// content means the same record republished gets the same id — under a
/// random id every republish would fill our own key with hourly copies of
/// us — and means a reader can match a `get` result to a later `refresh`.
///
/// Never zero: that is `Value::INVALID_ID`, which the reference answers as
/// "unknown announce" and stores nowhere.
pub fn content_id(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    if h == 0 { 1 } else { h }
}

/// TTL applied to a stored value when the wire does not carry one.
///
/// Ten minutes, matching what the group feed uses. Named rather than inlined
/// because the wire *will* carry a TTL eventually and this is the line that
/// changes.
pub const DEFAULT_TTL: u64 = 600;

/// The half of a node that answers other nodes.
pub struct Responder {
    tokens: TokenRing,
}

impl Responder {
    /// `seed` should not be guessable by peers; a node id is an adequate start
    /// because rotation makes the first secret short-lived.
    pub fn new(seed: u64, now_ms: u64) -> Responder {
        Responder { tokens: TokenRing::new(seed, now_ms) }
    }

    fn token_for(&self, who: &SocketAddr, key: &NodeId) -> Vec<u8> {
        self.tokens.issue(who, key).to_be_bytes().to_vec()
    }

    fn token_ok(&self, who: &SocketAddr, key: &NodeId, token: &[u8]) -> bool {
        let Ok(b) = <[u8; 8]>::try_from(token) else { return false };
        self.tokens.check(who, key, u64::from_be_bytes(b))
    }

    /// Turn an inbound message into the handler call it names, and produce
    /// the reply to send.
    ///
    /// `None` means nothing should go back on the wire — a reply, parts, or
    /// an error is an answer to something we sent, not a question to us.
    /// Answering a reply would make two nodes ping-pong forever, which is the
    /// shape of an amplification loop rather than a protocol.
    ///
    /// `now` is in seconds, as everywhere in snapcore.
    pub fn handle(&mut self, node: &mut Node, from_addr: SocketAddr, msg: &Msg, now: u64) -> Option<Msg> {
        self.tokens.tick(now.saturating_mul(1000));

        let (tid, from_id, query, target, token, values, want, vid) = match msg {
            Msg::Query { tid, from, query, target, token, values, want, vid, .. } =>
                (*tid, *from, *query, *target, token, values, *want, *vid),
            _ => return None,
        };

        // The sender's own claimed address is ignored in favour of the
        // socket's: a node that could name its own address would insert peers
        // at addresses it chose, which is how a routing table gets filled with
        // a single attacker's endpoints.
        let from = Contact { id: from_hash(&from_id), addr: from_addr, last_seen: now, failed: 0 };
        let key: Key = from_hash(&target);
        let me = to_hash(&node.id());
        let sa = Some(to_ip(&from_addr));

        // `want` absent means "the family I came from" — `bufferNodes` in
        // the reference. A v4 requester handed v6 contacts cannot use them.
        let want = if want == 0 {
            if from_addr.is_ipv6() { WANT6 } else { WANT4 }
        } else { want };
        let pack = |contacts: Vec<Contact>| -> Vec<(InfoHash, WireAddr)> {
            contacts.iter()
                .filter(|c| if c.addr.is_ipv6() { want & WANT6 != 0 } else { want & WANT4 != 0 })
                .map(|c| (to_hash(&c.id), to_wire_addr(&c.addr)))
                .collect()
        };
        let reply = |nodes, values, token, vid| Some(Msg::Reply { tid, from: me, sa, nodes, values, token, vid });

        match query {
            Query::Ping => {
                node.handle_ping(&from, now);
                reply(Vec::new(), Vec::new(), Vec::new(), None)
            }
            Query::Find => {
                let nodes = node.handle_find_node(&from, &key, now);
                reply(pack(nodes), Vec::new(), self.token_for(&from_addr, &key), None)
            }
            Query::Get => {
                let token = self.token_for(&from_addr, &key);
                match node.handle_find_value(&from, &key, now) {
                    FindValue::Values(vs) => {
                        let values = vs.into_iter()
                            .map(|data| Value::new(content_id(&data), data))
                            .collect();
                        // The reference sends the closest nodes beside the
                        // values; our `FindValue` is either/or, and a reader
                        // that has the values does not need the nodes.
                        reply(Vec::new(), values, token, None)
                    }
                    FindValue::Closest(cs) => reply(pack(cs), Vec::new(), token, None),
                }
            }
            Query::Put => {
                // A put without a value is not a store, and answering it as
                // though it were would have the sender believe its data
                // landed. (The reference answers with silence, which is
                // worse: the sender waits out a timeout for no reason.)
                let Some(first) = values.first() else {
                    return Some(Msg::Error { tid, code: dht_kit::ERR_NON_AUTHORITATIVE, from: Some(me) });
                };
                if !self.token_ok(&from_addr, &key, token) {
                    return Some(Msg::Error { tid, code: dht_kit::ERR_UNAUTHORIZED, from: Some(me) });
                }
                let vid = first.id;
                for v in values {
                    if v.data.len() > dht_kit::MAX_VALUE_SIZE { continue; }
                    node.handle_store(&from, key.clone(), v.data.clone(), DEFAULT_TTL, now);
                }
                // One acknowledgement, naming the first value. The reference
                // sends one reply PER value with the same tid; a client that
                // stores one value per put — every caller here — sees no
                // difference.
                reply(Vec::new(), Vec::new(), Vec::new(), Some(vid))
            }
            Query::Refresh => {
                if !self.token_ok(&from_addr, &key, token) {
                    return Some(Msg::Error { tid, code: dht_kit::ERR_UNAUTHORIZED, from: Some(me) });
                }
                let vid = vid?;
                // The value to keep alive is the one whose content id the
                // requester names. Re-putting the same bytes is the store's
                // own refresh path: same entry, later expiry, no duplicate.
                let hit = node.store.get(&key, now).into_iter().find(|d| content_id(d) == vid);
                match hit {
                    Some(data) => {
                        node.handle_store(&from, key, data, DEFAULT_TTL, now);
                        reply(Vec::new(), Vec::new(), Vec::new(), Some(vid))
                    }
                    None => Some(Msg::Error { tid, code: dht_kit::ERR_NOT_FOUND, from: Some(me) }),
                }
            }
            Query::Listen => {
                // Confirmed the way the reference confirms — `{id, sa, token}`
                // — and NOT serviced: nothing here keeps a listener list or
                // pushes updates. A listener that polls with `get` still
                // works, which is what our own client does. Stated here so
                // it is a known limit rather than a discovered one.
                if !self.token_ok(&from_addr, &key, token) {
                    return Some(Msg::Error { tid, code: dht_kit::ERR_UNAUTHORIZED, from: Some(me) });
                }
                node.handle_ping(&from, now);
                reply(Vec::new(), Vec::new(), self.token_for(&from_addr, &key), None)
            }
            Query::Update => {
                // A push for a listen we never registered. The reference
                // acknowledges with a listen confirmation and drops it.
                reply(Vec::new(), Vec::new(), Vec::new(), None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(seed: u8) -> Node {
        Node::new(NodeId([seed; ID_LEN]), "127.0.0.1:1".parse().unwrap())
    }
    fn resp() -> Responder { Responder::new(42, 0) }
    fn peer() -> SocketAddr { "192.168.1.9:4222".parse().unwrap() }
    fn peer6() -> SocketAddr { "[fd00::9]:4222".parse().unwrap() }

    fn query(q: Query, target: InfoHash) -> Msg {
        query_from(7, q, target, Vec::new(), Vec::new())
    }
    fn query_from(who: u64, q: Query, target: InfoHash, values: Vec<Value>, token: Vec<u8>) -> Msg {
        Msg::Query { tid: 77, from: InfoHash::from_seed(who), query: q, target,
                     token, values, want: 0, sid: None, vid: None, client: false }
    }
    fn token_from(m: &Option<Msg>) -> Vec<u8> {
        match m { Some(Msg::Reply { token, .. }) => token.clone(), other => panic!("{other:?}") }
    }

    #[test]
    fn a_ping_is_answered_with_id_and_sa_and_teaches_the_routing_table() {
        let mut n = node(1);
        let mut r = resp();
        let out = r.handle(&mut n, peer(), &query(Query::Ping, InfoHash::zero()), 10);
        match out {
            Some(Msg::Reply { tid: 77, sa, nodes, values, token, vid, .. }) => {
                assert_eq!(sa, Some(Ip::V4([192, 168, 1, 9])), "the requester learns its address");
                assert!(nodes.is_empty() && values.is_empty() && token.is_empty() && vid.is_none());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(n.rt.len(), 1);
    }

    #[test]
    fn a_get_hands_out_a_token_that_the_following_put_needs() {
        let mut n = node(1);
        let mut r = resp();
        let key = InfoHash::from_seed(5);
        let token = token_from(&r.handle(&mut n, peer(), &query(Query::Get, key), 10));
        assert_eq!(token.len(), 8);

        let v = Value::new(content_id(b"hello"), b"hello".to_vec());
        // Without the token: refused, and stored nowhere.
        let bad = r.handle(&mut n, peer(), &query_from(7, Query::Put, key, vec![v.clone()], vec![]), 10);
        assert!(matches!(bad, Some(Msg::Error { code: dht_kit::ERR_UNAUTHORIZED, .. })), "{bad:?}");
        assert_eq!(n.store.value_count(&from_hash(&key)), 0);
        // With it: stored, and the ack names the value.
        let ok = r.handle(&mut n, peer(), &query_from(7, Query::Put, key, vec![v.clone()], token), 10);
        assert!(matches!(ok, Some(Msg::Reply { vid: Some(id), .. }) if id == v.id), "{ok:?}");
        assert_eq!(n.store.value_count(&from_hash(&key)), 1);
    }

    #[test]
    fn a_token_is_bound_to_the_requesters_address_and_key() {
        let mut n = node(1);
        let mut r = resp();
        let key = InfoHash::from_seed(5);
        let token = token_from(&r.handle(&mut n, peer(), &query(Query::Get, key), 10));
        let v = vec![Value::new(1, b"x".to_vec())];
        let other_key = InfoHash::from_seed(6);
        let wrong_key = r.handle(&mut n, peer(), &query_from(7, Query::Put, other_key, v.clone(), token.clone()), 10);
        assert!(matches!(wrong_key, Some(Msg::Error { code: 401, .. })));
        let other_addr: SocketAddr = "192.168.1.10:4222".parse().unwrap();
        let wrong_addr = r.handle(&mut n, other_addr, &query_from(7, Query::Put, key, v.clone(), token.clone()), 10);
        assert!(matches!(wrong_addr, Some(Msg::Error { code: 401, .. })));
        // Same ip, different port — a NAT rewrote it. Still accepted.
        let new_port: SocketAddr = "192.168.1.9:50000".parse().unwrap();
        let ok = r.handle(&mut n, new_port, &query_from(7, Query::Put, key, v, token), 10);
        assert!(matches!(ok, Some(Msg::Reply { .. })));
    }

    #[test]
    fn a_put_then_a_get_returns_the_value_with_a_stable_content_id() {
        let mut n = node(1);
        let mut r = resp();
        let key = InfoHash::from_seed(5);
        let token = token_from(&r.handle(&mut n, peer(), &query(Query::Get, key), 10));
        let v = Value::new(content_id(b"hello"), b"hello".to_vec());
        r.handle(&mut n, peer(), &query_from(7, Query::Put, key, vec![v.clone()], token), 10);
        match r.handle(&mut n, peer(), &query(Query::Get, key), 11) {
            Some(Msg::Reply { values, nodes, .. }) => {
                assert_eq!(values, vec![v]);
                assert!(nodes.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_get_for_an_unknown_key_returns_closest_nodes_not_an_empty_value_list() {
        let mut n = node(1);
        let mut r = resp();
        r.handle(&mut n, peer(), &query(Query::Ping, InfoHash::zero()), 10);
        match r.handle(&mut n, peer(), &query(Query::Get, InfoHash::from_seed(9)), 11) {
            Some(Msg::Reply { values, nodes, .. }) => {
                assert!(values.is_empty());
                assert_eq!(nodes.len(), 1);
                assert_eq!(nodes[0].1, WireAddr::V4([192, 168, 1, 9], 4222));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_put_with_no_value_is_an_error_rather_than_a_silent_success() {
        let mut n = node(1);
        let mut r = resp();
        let out = r.handle(&mut n, peer(), &query(Query::Put, InfoHash::from_seed(1)), 10);
        assert!(matches!(out, Some(Msg::Error { code: 201, .. })));
        assert_eq!(n.store.key_count(), 0);
    }

    #[test]
    fn refresh_extends_a_value_it_can_name_and_404s_one_it_cannot() {
        let mut n = node(1);
        let mut r = resp();
        let key = InfoHash::from_seed(5);
        let token = token_from(&r.handle(&mut n, peer(), &query(Query::Get, key), 10));
        let v = Value::new(content_id(b"keep"), b"keep".to_vec());
        r.handle(&mut n, peer(), &query_from(7, Query::Put, key, vec![v.clone()], token.clone()), 10);

        let refresh = |vid: u64, tok: Vec<u8>| Msg::Query {
            tid: 78, from: InfoHash::from_seed(7), query: Query::Refresh, target: key,
            token: tok, values: vec![], want: 0, sid: None, vid: Some(vid), client: false };
        // Well past the first TTL — a refresh at 500 pushes expiry to 1100.
        let ok = r.handle(&mut n, peer(), &refresh(v.id, token.clone()), 500);
        assert!(matches!(ok, Some(Msg::Reply { vid: Some(id), .. }) if id == v.id), "{ok:?}");
        assert_eq!(n.store.get(&from_hash(&key), 1000).len(), 1, "still alive after the original TTL");

        let missing = r.handle(&mut n, peer(), &refresh(12345, token.clone()), 500);
        assert!(matches!(missing, Some(Msg::Error { code: 404, .. })), "{missing:?}");
        let unauth = r.handle(&mut n, peer(), &refresh(v.id, vec![0; 8]), 500);
        assert!(matches!(unauth, Some(Msg::Error { code: 401, .. })));
    }

    #[test]
    fn listen_is_confirmed_in_the_references_shape() {
        let mut n = node(1);
        let mut r = resp();
        let key = InfoHash::from_seed(5);
        let token = token_from(&r.handle(&mut n, peer(), &query(Query::Get, key), 10));
        let m = Msg::Query { tid: 1, from: InfoHash::from_seed(7), query: Query::Listen, target: key,
                             token, values: vec![], want: 0, sid: Some(9), vid: None, client: false };
        match r.handle(&mut n, peer(), &m, 10) {
            Some(Msg::Reply { sa, token, nodes, values, .. }) => {
                assert!(sa.is_some() && !token.is_empty() && nodes.is_empty() && values.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_reply_parts_or_error_is_not_answered() {
        let mut n = node(1);
        let mut r = resp();
        let m = Msg::Reply { tid: 1, from: InfoHash::from_seed(1), sa: None, nodes: vec![],
                             values: vec![], token: vec![], vid: None };
        assert!(r.handle(&mut n, peer(), &m, 10).is_none());
        let e = Msg::Error { tid: 1, code: 400, from: None };
        assert!(r.handle(&mut n, peer(), &e, 10).is_none());
        let p = Msg::Parts { tid: 1, fragments: vec![] };
        assert!(r.handle(&mut n, peer(), &p, 10).is_none());
    }

    #[test]
    fn the_senders_socket_address_is_used_not_the_one_it_claims() {
        // dht-kit carries no self-claimed address at all, so the only thing a
        // node can learn is where the packet came from. Pinned so that if a
        // future field appears, this test is the one that has to change.
        let mut n = node(1);
        let mut r = resp();
        r.handle(&mut n, peer(), &query(Query::Ping, InfoHash::zero()), 10);
        let c = n.rt.closest(&NodeId([0; ID_LEN]), 1);
        assert_eq!(c[0].addr, peer());
    }

    #[test]
    fn a_node_that_moves_replaces_rather_than_duplicates() {
        let mut n = node(1);
        let mut r = resp();
        r.handle(&mut n, peer(), &query(Query::Ping, InfoHash::zero()), 10);
        let moved: SocketAddr = "10.0.0.2:4222".parse().unwrap();
        r.handle(&mut n, moved, &query(Query::Ping, InfoHash::zero()), 11);
        assert_eq!(n.rt.len(), 1);
        assert_eq!(n.rt.closest(&NodeId([0; ID_LEN]), 1)[0].addr, moved);
    }

    #[test]
    fn want_absent_means_the_requesters_family() {
        let mut n = node(1);
        let mut r = resp();
        r.handle(&mut n, peer(), &query(Query::Ping, InfoHash::zero()), 10);
        // Odd seeds only: `from_seed(8) == from_seed(9)` (the seed is OR'd with 1).
        r.handle(&mut n, peer6(), &query_from(11, Query::Ping, InfoHash::zero(), vec![], vec![]), 10);
        assert_eq!(n.rt.len(), 2);
        let v4 = r.handle(&mut n, "192.168.1.20:1".parse().unwrap(), &query_from(13, Query::Find, InfoHash::from_seed(3), vec![], vec![]), 11);
        match v4 {
            Some(Msg::Reply { nodes, .. }) => assert!(nodes.iter().all(|(_, a)| !a.is_v6()) && !nodes.is_empty()),
            other => panic!("{other:?}"),
        }
        let v6 = r.handle(&mut n, "[fd00::20]:1".parse().unwrap(), &query_from(13, Query::Find, InfoHash::from_seed(3), vec![], vec![]), 11);
        match v6 {
            Some(Msg::Reply { nodes, .. }) => assert!(nodes.iter().all(|(_, a)| a.is_v6()) && !nodes.is_empty()),
            other => panic!("{other:?}"),
        }
        let mut both = query_from(13, Query::Find, InfoHash::from_seed(3), vec![], vec![]);
        if let Msg::Query { want, .. } = &mut both { *want = WANT4 | WANT6; }
        match r.handle(&mut n, "192.168.1.20:1".parse().unwrap(), &both, 11) {
            Some(Msg::Reply { nodes, .. }) => {
                assert!(nodes.iter().any(|(_, a)| a.is_v6()) && nodes.iter().any(|(_, a)| !a.is_v6()),
                        "both families when both are wanted: {nodes:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn content_id_is_never_the_invalid_id() {
        assert_ne!(content_id(b""), 0);
        assert_eq!(content_id(b"abc"), content_id(b"abc"));
        assert_ne!(content_id(b"abc"), content_id(b"abd"));
    }

    #[test]
    fn every_reply_encodes_and_decodes_through_dht_kit() {
        let mut n = node(1);
        let mut r = resp();
        let key = InfoHash::from_seed(5);
        r.handle(&mut n, peer(), &query(Query::Ping, InfoHash::zero()), 10);
        let g = r.handle(&mut n, peer(), &query(Query::Get, key), 10).unwrap();
        assert_eq!(Msg::decode(&g.encode()), Some(g.clone()));
        let token = token_from(&Some(g));
        let p = r.handle(&mut n, peer(), &query_from(7, Query::Put, key, vec![Value::new(1, b"v".to_vec())], token), 10).unwrap();
        assert_eq!(Msg::decode(&p.encode()), Some(p));
        let g2 = r.handle(&mut n, peer(), &query(Query::Get, key), 11).unwrap();
        assert_eq!(Msg::decode(&g2.encode()), Some(g2));
    }
}
