//! udp — the `Transport` that talks to a real network.
//!
//! ## What was missing
//!
//! Three pieces of a DHT existed and nothing joined them:
//!
//! * `dht-kit` — InfoHash, Value, the `Msg`/`Query` wire vocabulary. Types
//!   and a protocol, no engine.
//! * `snapcore` — `lookup_value`, `lookup_nodes`, routing table, all
//!   generic over `Transport`. The engine.
//! * `snapclone-presence` — `announce_record` produces the `(key, value)`
//!   a caller should `put`. Its header says so explicitly: "Nothing here
//!   opens a socket or drives a lookup."
//!
//! Every `impl Transport` in the tree was a simulation (`sim.rs`,
//! `proximity.rs`). So the DHT worked in tests, over an imaginary
//! network, and `DiscoveryBackend::Jami` in snapclone's settings had
//! nothing to reach.
//!
//! This is the socket.
//!
//! ## Blocking, with a timeout, on purpose
//!
//! `Transport`'s methods return values rather than futures, so the trait
//! is already synchronous — and `lookup_nodes` drives them in rounds it
//! controls. Making this async would mean either changing the trait
//! (and the simulations that depend on it) or blocking on a runtime
//! inside a sync method, which is worse than blocking on a socket.
//!
//! The timeout is what keeps that honest: a dead peer must cost one
//! timeout, not a hung lookup. Kademlia is built to tolerate
//! non-responders, and the routing table's `failed` counter is how it
//! learns — but only if the call RETURNS.

use crate::node::{FindValue, Transport};
use crate::routing::{Contact, NodeId};
use crate::store::Key;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

/// How long to wait for one reply.
///
/// Short: a lookup makes several of these in sequence, so this is
/// multiplied by the number of contacts tried. Long enough for a LAN
/// round trip and a phone's radio waking up, short enough that a
/// silent peer does not stall a lookup a person is waiting on.
pub const REPLY_TIMEOUT: Duration = Duration::from_millis(600);

/// Largest datagram accepted.
///
/// A `find_node` reply carries k contacts; at 38 bytes each that is well
/// under 1 KiB, and 1500 is the ethernet MTU below which a datagram is
/// not fragmented. A larger buffer would invite a peer to send something
/// that fragments, and a fragmented UDP reply is one that arrives
/// sometimes.
const MAX_DATAGRAM: usize = 1500;

pub struct UdpTransport {
    sock: UdpSocket,
    me: NodeId,
}

impl UdpTransport {
    /// Bind a socket for this node.
    ///
    /// `0.0.0.0:0` is the ordinary case — the OS picks a port and peers
    /// learn it from the source address of what we send. A fixed port is
    /// only needed for a bootstrap node that others are configured to
    /// find.
    pub fn bind(me: NodeId, addr: SocketAddr) -> std::io::Result<UdpTransport> {
        let sock = UdpSocket::bind(addr)?;
        sock.set_read_timeout(Some(REPLY_TIMEOUT))?;
        Ok(UdpTransport { sock, me })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    /// Send one query and wait for one reply.
    ///
    /// Returns `None` on timeout, on a send error, and on a reply from
    /// the WRONG PEER — see below. Every failure is the same to the
    /// caller because the caller's response is the same: count it
    /// against that contact and move on.
    fn round_trip(&self, to: &Contact, msg: &[u8]) -> Option<Vec<u8>> {
        self.sock.send_to(msg, to.addr).ok()?;
        let mut buf = [0u8; MAX_DATAGRAM];
        let (n, from) = self.sock.recv_from(&mut buf).ok()?;
        // A reply from an address we did not ask is dropped, not
        // returned. UDP has no connection, so anything on the network
        // can answer a query it saw — accepting that would let a
        // bystander steer a lookup by racing the real peer.
        //
        // This does NOT retry: a spoofed reply costs the real one, and
        // the contact takes the blame. That is the accepted trade at
        // this layer; authenticating replies is the crypto layer's job
        // and `snapclone-crypto` is where it belongs.
        if from != to.addr {
            return None;
        }
        Some(buf[..n].to_vec())
    }
}

impl Transport for UdpTransport {
    fn find_node(&self, _from: &Contact, to: &Contact, target: &NodeId) -> Vec<Contact> {
        let mut msg = vec![TAG_FIND_NODE];
        msg.extend_from_slice(&self.me.0);
        msg.extend_from_slice(&target.0);
        match self.round_trip(to, &msg) {
            // An empty vec is what the engine reads as "this peer knew
            // nobody", which is also the right reading of "this peer did
            // not answer" — it advances the lookup either way rather
            // than stalling it.
            None => Vec::new(),
            Some(reply) => decode_contacts(&reply),
        }
    }

    fn find_value(&self, _from: &Contact, to: &Contact, key: &Key) -> FindValue {
        let mut msg = vec![TAG_FIND_VALUE];
        msg.extend_from_slice(&self.me.0);
        msg.extend_from_slice(&key.0);
        match self.round_trip(to, &msg) {
            None => FindValue::Closest(Vec::new()),
            Some(reply) => match reply.first() {
                Some(&TAG_VALUES) => FindValue::Values(decode_values(&reply[1..])),
                _ => FindValue::Closest(decode_contacts(&reply)),
            },
        }
    }

    fn store(&self, _from: &Contact, to: &Contact, key: Key, value: Vec<u8>, ttl: u64) {
        let mut msg = vec![TAG_STORE];
        msg.extend_from_slice(&self.me.0);
        msg.extend_from_slice(&key.0);
        msg.extend_from_slice(&ttl.to_be_bytes());
        msg.extend_from_slice(&(value.len() as u16).to_be_bytes());
        msg.extend_from_slice(&value);
        // Fire and forget, and NOT because the result is uninteresting:
        // `store` returns `()`, so there is nowhere to report a failure
        // to. Kademlia stores to k peers precisely so one silent node
        // does not lose the value.
        let _ = self.sock.send_to(&msg, to.addr);
    }

    fn ping(&self, _from: &Contact, to: &Contact) -> bool {
        let mut msg = vec![TAG_PING];
        msg.extend_from_slice(&self.me.0);
        self.round_trip(to, &msg).is_some()
    }
}

const TAG_PING: u8 = 1;
const TAG_FIND_NODE: u8 = 2;
const TAG_FIND_VALUE: u8 = 3;
const TAG_STORE: u8 = 4;
const TAG_VALUES: u8 = 5;

/// Contacts from a reply body: id, then 4-byte IPv4 and 2-byte port.
///
/// A short tail is IGNORED rather than an error. A truncated reply means
/// a malformed or hostile peer, and the contacts that did parse are
/// still usable — refusing all of them would let one bad peer erase good
/// information.
fn decode_contacts(reply: &[u8]) -> Vec<Contact> {
    const ID: usize = 20;
    const REC: usize = ID + 4 + 2;
    let body = if reply.first() == Some(&TAG_FIND_NODE) { &reply[1..] } else { reply };
    body.chunks_exact(REC)
        .map(|c| {
            let mut id = [0u8; ID];
            id.copy_from_slice(&c[..ID]);
            let ip = [c[ID], c[ID + 1], c[ID + 2], c[ID + 3]];
            let port = u16::from_be_bytes([c[ID + 4], c[ID + 5]]);
            Contact {
                id: NodeId(id),
                addr: SocketAddr::from((ip, port)),
                last_seen: 0,
                failed: 0,
            }
        })
        .collect()
}

/// Length-prefixed values.
fn decode_values(body: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 <= body.len() {
        let len = u16::from_be_bytes([body[i], body[i + 1]]) as usize;
        i += 2;
        // A length running past the buffer stops the loop instead of
        // panicking on the slice. This is parsing bytes from the
        // network, where the length field is exactly what a hostile peer
        // would lie about.
        if i + len > body.len() {
            break;
        }
        out.push(body[i..i + len].to_vec());
        i += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> NodeId {
        NodeId([n; 20])
    }

    fn contact(n: u8, port: u16) -> Contact {
        Contact {
            id: id(n),
            addr: SocketAddr::from(([127, 0, 0, 1], port)),
            last_seen: 0,
            failed: 0,
        }
    }

    #[test]
    fn a_silent_peer_costs_one_timeout_and_returns() {
        // The property the whole design rests on: Kademlia tolerates
        // non-responders, but only if the call RETURNS so the routing
        // table can count the failure.
        let t = UdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap();
        // Port 1 on loopback: nothing is listening, so this times out.
        let dead = contact(2, 1);
        let start = std::time::Instant::now();
        assert!(!t.ping(&dead, &dead), "a dead peer must not read as alive");
        assert!(start.elapsed() < REPLY_TIMEOUT * 3, "took {:?}", start.elapsed());
    }

    #[test]
    fn an_unanswered_find_node_yields_no_contacts_rather_than_hanging() {
        let t = UdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap();
        let dead = contact(2, 1);
        assert!(t.find_node(&dead, &dead, &id(3)).is_empty());
    }

    #[test]
    fn an_unanswered_find_value_is_closest_empty_not_values_empty() {
        // The difference matters to the engine: `Values(vec![])` would
        // read as "found, and the answer is nothing", ending the lookup.
        // `Closest(vec![])` reads as "this peer added nothing", and the
        // lookup continues with its other candidates.
        let t = UdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap();
        let dead = contact(2, 1);
        match t.find_value(&dead, &dead, &NodeId([0u8; 20])) {
            FindValue::Closest(c) => assert!(c.is_empty()),
            FindValue::Values(_) => panic!("a timeout must not read as a found value"),
        }
    }

    #[test]
    fn a_truncated_contact_list_keeps_what_parsed() {
        // A malformed reply is a hostile or buggy peer. Discarding every
        // contact would let one bad peer erase good information.
        let mut body = Vec::new();
        body.extend_from_slice(&[7u8; 20]);
        body.extend_from_slice(&[10, 0, 0, 5]);
        body.extend_from_slice(&1234u16.to_be_bytes());
        body.extend_from_slice(&[9u8; 11]); // a half record
        let got = decode_contacts(&body);
        assert_eq!(got.len(), 1, "the whole record should survive the broken tail");
        assert_eq!(got[0].addr.port(), 1234);
    }

    #[test]
    fn a_lying_length_prefix_does_not_panic() {
        // Parsing bytes from the network, where the length field is
        // exactly what a hostile peer would lie about. An out-of-range
        // length must stop the loop, not index past the buffer.
        let body = vec![0xff, 0xff, 1, 2, 3];
        assert!(decode_values(&body).is_empty());
    }

    #[test]
    fn values_round_trip_through_their_length_prefixes() {
        let mut body = Vec::new();
        for v in [b"one".as_slice(), b"two".as_slice()] {
            body.extend_from_slice(&(v.len() as u16).to_be_bytes());
            body.extend_from_slice(v);
        }
        assert_eq!(decode_values(&body), vec![b"one".to_vec(), b"two".to_vec()]);
    }

    #[test]
    fn a_reply_from_the_wrong_address_is_not_accepted() {
        // UDP has no connection, so anything on the network can answer a
        // query it saw. Accepting that would let a bystander steer a
        // lookup by racing the real peer.
        let t = UdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap();
        let target = contact(2, 1);
        let impostor = UdpSocket::bind("127.0.0.1:0").unwrap();
        let t_addr = t.local_addr().unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            let _ = impostor.send_to(&[TAG_PING], t_addr);
        });
        // The impostor's reply arrives first and is dropped; the real
        // peer never answers, so this is false.
        assert!(!t.ping(&target, &target), "a reply from elsewhere was accepted");
    }
}
