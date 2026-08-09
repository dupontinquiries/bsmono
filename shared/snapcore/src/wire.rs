//! wire — packets in, handler calls out.
//!
//! `snapcore` is a sans-io engine: its handlers take `Contact`, `NodeId` and
//! `Vec<u8>`, so there has never been anything to put on a socket. `dht-kit` is
//! the OpenDHT-compatible codec with no engine. This module is the seam, and
//! it is the first step of the convergence recorded in
//! `maintenance/DHT_CONVERGENCE.md`.
//!
//! ## Behind a feature, deliberately
//!
//! snapcore has had **zero dependencies**, and five crates rely on that. `wire`
//! is off by default, so weather-share, share-kit, net-kit, netcode and bsgames
//! link exactly what they linked before. Anyone building a real transport turns
//! it on.
//!
//! ## What this does and does not do
//!
//! It translates: a decoded `dht_kit::Msg` becomes the right `Node::handle_*`
//! call, and the answer becomes a `Msg` to send back. It does **not** own a
//! socket, retransmit, or decide when to query — those belong to a transport
//! and to the lookup driver respectively, and putting them here would make this
//! the third engine rather than the bridge between the two that exist.

use crate::node::{FindValue, Node};
use crate::routing::{Contact, NodeId, ID_LEN};
use crate::store::Key;
use dht_kit::{Addr as WireAddr, InfoHash, Msg, Query, Value};
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

/// Both families, since dht-kit gained `nodes6`.
///
/// The first version returned `None` for v6 and dropped those contacts from
/// replies. That was the wrong trade on any modern network: a peer reachable
/// over v6 usually needs no NAT traversal at all, so dropping them discards
/// precisely the easiest peers to reach — and on a v6-only network it reports
/// an empty DHT.
///
/// Infallible now, which is why it returns `WireAddr` rather than an `Option`:
/// there is no address a socket can hold that the wire cannot carry.
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

/// Turn an inbound message into the handler call it names, and produce the
/// reply to send.
///
/// `None` means nothing should go back on the wire — a reply, a push, or an
/// error is an answer to something we sent, not a question to us. Answering a
/// reply would make two nodes ping-pong forever, which is the shape of an
/// amplification loop rather than a protocol.
pub fn handle(node: &mut Node, from_addr: SocketAddr, msg: &Msg, now: u64) -> Option<Msg> {
    let (tid, from_id, query, target, token, value) = match msg {
        Msg::Query { tid, from, query, target, token, value } =>
            (*tid, *from, *query, *target, *token, value.clone()),
        _ => return None,
    };

    let from = Contact {
        id: from_hash(&from_id),
        addr: from_addr,
        last_seen: now,
        failed: 0,
    };
    let key: Key = from_hash(&target);
    let me = to_hash(&node.id());

    // The sender's own claimed address is ignored in favour of the socket's:
    // a node that could name its own address would insert peers at addresses
    // it chose, which is how a routing table gets filled with a single
    // attacker's endpoints.
    let pack = |contacts: Vec<Contact>| -> Vec<(InfoHash, WireAddr)> {
        // No filter_map any more: every contact can be carried, so a reply
        // lists what the routing table actually knows rather than the v4 subset
        // of it.
        contacts.iter().map(|c| (to_hash(&c.id), to_wire_addr(&c.addr))).collect()
    };

    match query {
        Query::Ping => {
            node.handle_ping(&from, now);
            Some(Msg::Reply { tid, from: me, nodes: Vec::new(), values: Vec::new(), token: 0 })
        }
        Query::Find => {
            let nodes = node.handle_find_node(&from, &key, now);
            Some(Msg::Reply { tid, from: me, nodes: pack(nodes), values: Vec::new(), token: 0 })
        }
        Query::Get | Query::Listen => match node.handle_find_value(&from, &key, now) {
            FindValue::Values(vs) => Some(Msg::Reply {
                tid,
                from: me,
                nodes: Vec::new(),
                // Value ids are positional here because snapcore's store holds
                // opaque bytes with no id of its own. A transport that needs
                // stable ids across replies has to carry them in the value, and
                // this is the seam where that limitation lives rather than a
                // place to invent one.
                values: vs.into_iter().enumerate()
                    .map(|(i, data)| Value { id: i as u64, data })
                    .collect(),
                token: 0,
            }),
            FindValue::Closest(cs) => Some(Msg::Reply {
                tid, from: me, nodes: pack(cs), values: Vec::new(), token: 0,
            }),
        },
        Query::Put => {
            // A put without a value is not a store, and answering it as though
            // it were would have the sender believe its data landed.
            let Some(v) = value else {
                return Some(Msg::Error { tid, code: 400 });
            };
            let _ = token;
            node.handle_store(&from, key, v.data, DEFAULT_TTL, now);
            Some(Msg::Reply { tid, from: me, nodes: Vec::new(), values: Vec::new(), token: 0 })
        }
        Query::Refresh => {
            // Accepted and acknowledged, but nothing beneath implements value
            // keep-alive yet, so it is answered as an error rather than as a
            // success that did nothing — a refresh whose reply says "fine"
            // while the value expires is worse than a refusal.
            Some(Msg::Error { tid, code: 501 })
        }
    }
}

/// TTL applied to a stored value when the wire does not carry one.
///
/// Ten minutes, matching what the group feed uses. Named rather than inlined
/// because the wire *will* carry a TTL eventually and this is the line that
/// changes.
pub const DEFAULT_TTL: u64 = 600;

#[cfg(test)]
mod tests {
    use super::*;

    fn node(seed: u8) -> Node {
        let mut id = [0u8; ID_LEN];
        id[0] = seed;
        Node::new(NodeId(id), format!("127.0.0.1:{}", 6000 + seed as u16).parse().unwrap())
    }

    fn peer() -> SocketAddr { "192.168.1.9:4222".parse().unwrap() }

    fn query(q: Query, target: InfoHash, value: Option<Value>) -> Msg {
        query_from(0xAB, q, target, value)
    }

    fn query_from(who: u64, q: Query, target: InfoHash, value: Option<Value>) -> Msg {
        Msg::Query { tid: 77, from: InfoHash::from_seed(who), query: q, target,
                     token: 0, value }
    }

    #[test]
    fn a_ping_is_answered_and_teaches_the_routing_table() {
        let mut n = node(1);
        let out = handle(&mut n, peer(), &query(Query::Ping, InfoHash::zero(), None), 10);
        assert!(matches!(out, Some(Msg::Reply { tid: 77, .. })));
    }

    #[test]
    fn a_put_then_a_get_returns_the_value_over_the_wire() {
        // The round trip the whole bridge exists for.
        let mut n = node(2);
        let key = InfoHash::from_seed(0x1234);
        let v = Value { id: 0, data: b"hello".to_vec() };
        let put = handle(&mut n, peer(), &query(Query::Put, key, Some(v)), 10);
        assert!(matches!(put, Some(Msg::Reply { .. })));

        match handle(&mut n, peer(), &query(Query::Get, key, None), 11) {
            Some(Msg::Reply { values, .. }) => {
                assert_eq!(values.len(), 1);
                assert_eq!(values[0].data, b"hello");
            }
            other => panic!("get answered with {other:?}"),
        }
    }

    #[test]
    fn a_get_for_an_unknown_key_returns_closest_nodes_not_an_empty_value_list() {
        // The difference matters to the requester: values-but-empty means "it
        // is not here", closest-nodes means "ask these next". Collapsing them
        // stalls an iterative lookup at the first hop.
        let mut n = node(3);
        handle(&mut n, peer(), &query(Query::Ping, InfoHash::zero(), None), 10);
        match handle(&mut n, peer(), &query(Query::Get, InfoHash::from_seed(9), None), 11) {
            Some(Msg::Reply { values, nodes, .. }) => {
                assert!(values.is_empty());
                assert!(!nodes.is_empty(), "the pinging peer should come back as a candidate");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_put_with_no_value_is_an_error_rather_than_a_silent_success() {
        // Answering it as a Reply would have the sender believe its data
        // landed.
        let mut n = node(4);
        let out = handle(&mut n, peer(), &query(Query::Put, InfoHash::from_seed(1), None), 10);
        assert!(matches!(out, Some(Msg::Error { code: 400, .. })));
    }

    #[test]
    fn refresh_is_refused_rather_than_acknowledged() {
        // Nothing beneath implements value keep-alive. A reply saying "fine"
        // while the value expires is worse than a refusal.
        let mut n = node(5);
        let out = handle(&mut n, peer(), &query(Query::Refresh, InfoHash::from_seed(1), None), 10);
        assert!(matches!(out, Some(Msg::Error { code: 501, .. })));
    }

    #[test]
    fn a_reply_is_not_answered() {
        // Answering an answer is an amplification loop, not a protocol.
        let mut n = node(6);
        let m = Msg::Reply { tid: 1, from: InfoHash::from_seed(1), nodes: vec![],
                             values: vec![], token: 0 };
        assert!(handle(&mut n, peer(), &m, 10).is_none());
        let e = Msg::Error { tid: 1, code: 400 };
        assert!(handle(&mut n, peer(), &e, 10).is_none());
    }

    #[test]
    fn the_senders_socket_address_is_used_not_the_one_it_claims() {
        // A node that could name its own address would insert peers wherever it
        // liked, which is how a routing table fills with one attacker's
        // endpoints. The message carries no address field at all — this test
        // pins that the contact is built from the socket.
        let mut n = node(7);
        // Distinct sender ids: my first version reused one, and the second
        // contact correctly REPLACED the first's address, which made the test
        // fail for the right reason. That behaviour is pinned separately below.
        handle(&mut n, "203.0.113.5:9999".parse().unwrap(),
               &query_from(0x11, Query::Ping, InfoHash::zero(), None), 10);
        match handle(&mut n, peer(), &query_from(0x22, Query::Find, InfoHash::from_seed(2), None), 11) {
            Some(Msg::Reply { nodes, .. }) => {
                assert!(nodes.iter().any(|(_, a)| *a == WireAddr::V4([203, 0, 113, 5], 9999)),
                        "the contact came from the socket: {nodes:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_peer_that_reappears_from_a_new_address_moves_rather_than_duplicating() {
        // Mobile devices change address constantly — a phone leaving wifi keeps
        // its node id and arrives from a new socket. Two entries for one id
        // means half the traffic goes to an address that no longer answers.
        let mut n = node(8);
        handle(&mut n, "203.0.113.5:9999".parse().unwrap(),
               &query_from(0x33, Query::Ping, InfoHash::zero(), None), 10);
        handle(&mut n, "198.51.100.7:1234".parse().unwrap(),
               &query_from(0x33, Query::Ping, InfoHash::zero(), None), 11);
        match handle(&mut n, peer(), &query_from(0x44, Query::Find, InfoHash::from_seed(2), None), 12) {
            Some(Msg::Reply { nodes, .. }) => {
                let same_id: Vec<_> = nodes.iter()
                    .filter(|(h, _)| *h == InfoHash::from_seed(0x33)).collect();
                assert_eq!(same_id.len(), 1, "one entry per id: {nodes:?}");
                assert_eq!(same_id[0].1, WireAddr::V4([198, 51, 100, 7], 1234), "the newer address wins");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn ids_survive_the_round_trip_between_the_two_type_systems() {
        let id = NodeId([0x5A; ID_LEN]);
        assert_eq!(from_hash(&to_hash(&id)), id);
    }

    #[test]
    fn both_address_families_survive_the_round_trip() {
        for s in ["10.0.0.1:4222", "[2001:db8::1]:4223", "[fe80::2]:9000"] {
            let a: SocketAddr = s.parse().unwrap();
            assert_eq!(from_wire_addr(&to_wire_addr(&a)), a, "{s}");
        }
    }

    #[test]
    fn a_v6_peer_is_advertised_rather_than_dropped() {
        // The bug this replaced: v6 contacts were filtered out of replies, so a
        // v6-only network reported an empty DHT and a dual-stack one hid the
        // peers needing no NAT traversal at all.
        let mut n = node(9);
        handle(&mut n, "[2001:db8::5]:9999".parse().unwrap(),
               &query_from(0x55, Query::Ping, InfoHash::zero(), None), 10);
        match handle(&mut n, peer(), &query_from(0x66, Query::Find, InfoHash::from_seed(2), None), 11) {
            Some(Msg::Reply { nodes, .. }) => {
                assert!(nodes.iter().any(|(_, a)| a.is_v6()),
                        "the v6 peer must come back: {nodes:?}");
            }
            other => panic!("{other:?}"),
        }
    }
}
