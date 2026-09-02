//! node_host — the component that owns a running DHT node.
//!
//! ## What this is, and why it is not an arrow
//!
//! `dht.rs` next door is pure: hand it a `Transport` and a
//! `RoutingTable` and it publishes or resolves. That is the right shape
//! for testing and the wrong shape for an app, because somebody has to
//! own the socket, the identity, the routing table and the thread that
//! keeps them alive across frames.
//!
//! This is that somebody. Same shape as voiceterm's `PtyHost`: spawn it,
//! poll it, drop it.
//!
//! ## Why a thread and a channel rather than a call
//!
//! A lookup makes several UDP round trips in sequence, each with a
//! 600 ms timeout. On a phone with two unreachable peers in its routing
//! table that is over a second — and a UI thread that blocks for a
//! second has dropped sixty frames and reads as a freeze.
//!
//! So requests go down a channel and results come back up one, and the
//! app polls. The app never waits.
//!
//! ## It answers, too
//!
//! The first version only asked. Its worker blocked on the request channel
//! and nothing read the socket between lookups, so an unsolicited query
//! from another node — the thing that makes a node a node — was never
//! seen. Two of these on a LAN could not find each other: each `find`
//! went unanswered, and both depended on a third party that did answer,
//! which has meant `bootstrap.jami.net`. The transport now serves
//! (`WireUdpTransport::serve`): one thread blocks in `recv_from` and
//! answers queries through `snapcore::wire::Responder`, while the worker
//! here drives lookups. Two hosts need nobody else — `tests/two_hosts.rs`.
//!
//! The two threads share the `Node` under a mutex. The serving thread
//! locks it per inbound packet, briefly. The worker does NOT hold it across
//! a lookup: it clones the routing table, runs the lookup on the clone, and
//! merges back whatever the lookup touched (`with_rt`). A lookup is
//! seconds of round trips, and a node that stops answering for the
//! duration of its own questions is a node its peers mark bad.
//!
//! ## What it deliberately does not do
//!
//! It does not decide what a result MEANS. `peer_source::Reach` already
//! says a DHT hit is `Unverified` — a record anyone could have published
//! under that key — and that judgement belongs there, not here. This
//! hands back what it found.

use snapcore::node::Node;
use snapcore::routing::NodeId;
// THE OPENDHT-SPEAKING SOCKET, not snapcore's bespoke one.
//
// `snapcore::udp::UdpTransport` frames its own way — `[TAG_FIND_NODE][id]
// [target]` — and this crate's bootstrap list is entirely Jami servers,
// which speak OpenDHT. The node was therefore sending bytes nothing on that
// list could answer: the routing table never filled, every lookup returned
// empty, and an empty lookup is also what a working DHT returns for a peer
// who is offline, so nothing above ever noticed.
//
// `snapcore::wire_udp` encodes with dht-kit, which this crate already
// depended on for its TYPES. See maintenance/DHT_CONVERGENCE.md — this is
// the convergence it argues for, applied to the path the app actually uses.
use snapcore::wire_udp::WireUdpTransport as UdpTransport;
use std::net::SocketAddr;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

/// Something to do, sent to the worker.
enum Req {
    Publish { sc_id: String, name: String, ip: String, port: u16 },
    PublishRaw { sc_id: String, value_id: u64, payload: Vec<u8> },
    Resolve { sc_id: String },
    ResolveRaw { sc_id: String },
    Bootstrap { addr: SocketAddr, id: NodeId },
}

/// Something that happened, polled by the app.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Published to this many peers. Zero is normal on a fresh node —
    /// see `dht::publish`.
    Published { sc_id: String, peers: usize },
    /// Resolved. An empty vec means the lookup finished and found
    /// nothing, which is different from never having asked.
    Resolved { sc_id: String, found: Vec<crate::presence::Presence> },
    /// Resolved, UNDECODED. For a caller that knows a payload format this
    /// crate does not — see `dht::publish_raw` on why the payload stopped
    /// being this crate's business.
    ResolvedRaw { sc_id: String, values: Vec<Vec<u8>> },
    /// The worker stopped. Carries why, because a discovery feature that
    /// silently stops is one people conclude is "just slow".
    Stopped(String),
    /// A keepalive round went out. `answered` of `pinged` peers replied —
    /// zero of several is the app's cue that the network changed under
    /// it (a NAT mapping expired, wifi handed off to cellular).
    Keepalive { pinged: usize, answered: usize },
}

pub struct NodeHost {
    tx: Sender<Req>,
    rx: Receiver<Event>,
    local: SocketAddr,
    transport: std::sync::Arc<UdpTransport>,
    serving: Option<std::thread::JoinHandle<()>>,
}

/// Run `f` on a snapshot of the routing table, then merge back what it
/// touched. Contacts the lookup saw carry `last_seen >= now`; those it did
/// not are left as the shared table already has them, so a lookup cannot
/// refresh staleness it never checked.
fn with_rt<R>(node: &std::sync::Mutex<Node>, now: u64,
              f: impl FnOnce(&mut snapcore::routing::RoutingTable) -> R) -> R {
    let mut rt = match node.lock() { Ok(n) => n.rt.clone(), Err(p) => p.into_inner().rt.clone() };
    let r = f(&mut rt);
    let mut n = match node.lock() { Ok(n) => n, Err(p) => p.into_inner() };
    let me = n.id();
    for c in rt.closest(&me, usize::MAX) {
        if c.last_seen >= now {
            n.rt.saw(c.id, c.addr, c.last_seen);
        }
    }
    r
}

impl NodeHost {
    /// Start a node on `bind`.
    ///
    /// `id` is this device's DHT identity. It should be derived from the
    /// sc-ID rather than random, so a device keeps its place in the
    /// keyspace across restarts — a node that re-randomises its id on
    /// every launch makes every other node's routing table stale.
    pub fn spawn(id: NodeId, bind: SocketAddr) -> std::io::Result<NodeHost> {
        Self::spawn_with_keepalive(id, bind, KEEPALIVE)
    }

    /// `spawn` with the keepalive period chosen — tests use a short one.
    pub fn spawn_with_keepalive(id: NodeId, bind: SocketAddr, keepalive: std::time::Duration)
        -> std::io::Result<NodeHost> {
        let tx_udp = std::sync::Arc::new(UdpTransport::bind(id, bind)?);
        let local = tx_udp.local_addr()?;
        let (req_tx, req_rx) = std::sync::mpsc::channel::<Req>();
        let (ev_tx, ev_rx) = std::sync::mpsc::channel::<Event>();
        let node = std::sync::Arc::new(std::sync::Mutex::new(Node::new(id, local)));
        let serving = tx_udp.serve(node.clone())?;

        let transport = tx_udp.clone();
        std::thread::spawn(move || {
            loop {
                // Blocking recv, not a poll loop: an idle node should
                // cost nothing. A DHT that burns a core waiting for
                // something to do is a battery bug on a phone, and this
                // is a phone app.
                // Blocking with a period, not a poll loop: the thread
                // wakes once per keepalive when idle, and that wake is the
                // keepalive. See `KEEPALIVE` for why it exists at all.
                let req = match req_rx.recv_timeout(keepalive) {
                    Ok(r) => r,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        let now = now_secs();
                        let me = me_contact(id, local, now);
                        let targets = {
                            let n = match node.lock() { Ok(n) => n, Err(p) => p.into_inner() };
                            // The peers most likely to hold our mapping and
                            // to be asked about us: the ones nearest our
                            // own id. Bounded, because this runs for the
                            // life of the app.
                            n.rt.closest(&id, KEEPALIVE_PEERS)
                        };
                        if targets.is_empty() { continue; }
                        let mut answered = 0;
                        for c in &targets {
                            if snapcore::node::Transport::ping(&*tx_udp, &me, c) {
                                answered += 1;
                                match node.lock() { Ok(mut n) => n.rt.saw(c.id, c.addr, now), Err(p) => p.into_inner().rt.saw(c.id, c.addr, now) }
                            } else {
                                match node.lock() { Ok(mut n) => n.rt.failed(&c.id, now), Err(p) => p.into_inner().rt.failed(&c.id, now) }
                            }
                        }
                        if ev_tx.send(Event::Keepalive { pinged: targets.len(), answered }).is_err() {
                            return;
                        }
                        continue;
                    }
                    // The sender is gone, which means NodeHost was
                    // dropped. Exit rather than report — nobody is
                    // listening to the event channel either.
                    Err(_) => return,
                };
                let now = now_secs();
                let ev = match req {
                    Req::Bootstrap { addr, id: peer } => {
                        // A routing table with no contacts can find
                        // nobody, so this is the step that makes every
                        // other one possible. Recorded as a publish of
                        // zero rather than a distinct event: the app's
                        // question is "can I reach anyone", and the
                        // table size answers it.
                        let mut n = match node.lock() { Ok(n) => n, Err(p) => p.into_inner() };
                        n.rt.saw(peer, addr, now);
                        Event::Published { sc_id: String::new(), peers: n.rt.len() }
                    }
                    Req::Publish { sc_id, name, ip, port } => {
                        let me = me_contact(id, local, now);
                        let (_, value) = crate::announce_record(&sc_id, &name, &ip, port);
                        let n = with_rt(&node, now, |rt| crate::dht::publish(
                            &me, rt, &*tx_udp,
                            &sc_id, &name, &ip, port, DEFAULT_TTL, now));
                        store_local(&node, &sc_id, value.data, now);
                        Event::Published { sc_id, peers: n }
                    }
                    Req::PublishRaw { sc_id, value_id, payload } => {
                        let me = me_contact(id, local, now);
                        let n = with_rt(&node, now, |rt| crate::dht::publish_raw(
                            &me, rt, &*tx_udp,
                            &sc_id, value_id, payload.clone(), DEFAULT_TTL, now));
                        store_local(&node, &sc_id, payload, now);
                        Event::Published { sc_id, peers: n }
                    }
                    Req::ResolveRaw { sc_id } => {
                        let me = me_contact(id, local, now);
                        let mut values = with_rt(&node, now, |rt| crate::dht::resolve_raw(
                            &me, rt, &*tx_udp, &sc_id, now))
                            .unwrap_or_default();
                        for b in local_values(&node, &sc_id, now) {
                            if !values.contains(&b) { values.push(b); }
                        }
                        Event::ResolvedRaw { sc_id, values }
                    }
                    Req::Resolve { sc_id } => {
                        let me = me_contact(id, local, now);
                        let mut found = with_rt(&node, now, |rt| crate::dht::resolve(
                            &me, rt, &*tx_udp, &sc_id, now))
                            .unwrap_or_default();
                        for b in local_values(&node, &sc_id, now) {
                            if let Some(p) = crate::dht::parse_record(&sc_id, &b) {
                                if !found.contains(&p) { found.push(p); }
                            }
                        }
                        Event::Resolved { sc_id, found }
                    }
                };
                // A send failure means the app dropped the host mid
                // lookup. Stop, rather than finish work nobody will
                // read.
                if ev_tx.send(ev).is_err() {
                    return;
                }
            }
        });

        Ok(NodeHost { tx: req_tx, rx: ev_rx, local, transport, serving: Some(serving) })
    }

    /// The ip peers say we reach them from — see
    /// `WireUdpTransport::public_ip`. `None` until any peer has answered.
    pub fn public_ip(&self) -> Option<std::net::IpAddr> {
        self.transport.public_ip()
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    /// Add a known peer so lookups have somewhere to start.
    pub fn bootstrap(&self, addr: SocketAddr, id: NodeId) -> bool {
        self.tx.send(Req::Bootstrap { addr, id }).is_ok()
    }

    pub fn publish(&self, sc_id: &str, name: &str, ip: &str, port: u16) -> bool {
        self.tx
            .send(Req::Publish {
                sc_id: sc_id.to_string(),
                name: name.to_string(),
                ip: ip.to_string(),
                port,
            })
            .is_ok()
    }

    /// Publish a payload the caller built. Answered with `Published`.
    pub fn publish_raw(&self, sc_id: &str, value_id: u64, payload: Vec<u8>) -> bool {
        self.tx.send(Req::PublishRaw {
            sc_id: sc_id.to_string(), value_id, payload }).is_ok()
    }

    /// Ask for the raw values under an sc-ID. Answered with `ResolvedRaw`.
    pub fn resolve_raw(&self, sc_id: &str) -> bool {
        self.tx.send(Req::ResolveRaw { sc_id: sc_id.to_string() }).is_ok()
    }

    pub fn resolve(&self, sc_id: &str) -> bool {
        self.tx.send(Req::Resolve { sc_id: sc_id.to_string() }).is_ok()
    }

    /// Everything that finished since the last call.
    ///
    /// Drains rather than returning one: a frame that produced three
    /// results should show three, and returning one per frame would make
    /// a burst of discoveries trickle in over three frames for no
    /// reason.
    pub fn poll(&self) -> Vec<Event> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(e) => out.push(e),
                Err(TryRecvError::Empty) => return out,
                Err(TryRecvError::Disconnected) => {
                    // Said once, not every frame: the worker is gone and
                    // will not come back, and a per-frame error line
                    // would bury everything else in the log.
                    if out.is_empty() {
                        out.push(Event::Stopped("the DHT worker stopped".into()));
                    }
                    return out;
                }
            }
        }
    }
}

/// How often an idle node pings the peers nearest it.
///
/// A served node is reachable only while its NAT mapping lives, and a
/// UDP mapping dies after roughly thirty seconds of silence on most home
/// routers and sooner on some carriers. Between lookups nothing else
/// leaves this socket, so the mapping that every peer's record of us
/// points at would expire within a minute of the last publish. Twenty-five
/// seconds keeps it under the common thirty; it is also the figure the
/// reference and Jami use, so a phone running both wakes its radio no
/// more often than it already did. The cost is a few datagrams a
/// minute; the alternative is a node that is only reachable for a minute
/// after it last spoke.
pub const KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(25);

/// Peers pinged per keepalive. Three is enough to keep a mapping open
/// through the routers between us and them, and to notice when none of
/// them answer.
pub const KEEPALIVE_PEERS: usize = 3;

/// How long a published record lives, in seconds.
///
/// Short enough that a device that goes away stops being advertised
/// within a few minutes; long enough that a phone does not have to wake
/// up constantly to republish. Republishing is the caller's job.
pub const DEFAULT_TTL: u64 = 300;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The reference's `put` stores locally as well as announcing, and its
/// `get` reads the local store first. Without both, the record a lone
/// pair of nodes exchanged lives only on the peer that received it, and
/// that peer resolving it would ask the other and find nothing.
fn store_local(node: &std::sync::Mutex<Node>, sc_id: &str, bytes: Vec<u8>, now: u64) {
    let key = crate::dht::key_of(&crate::presence::key_for_scid(sc_id));
    let mut n = match node.lock() { Ok(n) => n, Err(p) => p.into_inner() };
    n.store.put(key, bytes, DEFAULT_TTL, now);
}

fn local_values(node: &std::sync::Mutex<Node>, sc_id: &str, now: u64) -> Vec<Vec<u8>> {
    let key = crate::dht::key_of(&crate::presence::key_for_scid(sc_id));
    let n = match node.lock() { Ok(n) => n, Err(p) => p.into_inner() };
    n.store.get(&key, now)
}

impl Drop for NodeHost {
    fn drop(&mut self) {
        // Wake the serving thread so it notices the transport is going.
        self.transport.stop_serving();
        if let Some(h) = self.serving.take() {
            let _ = h.join();
        }
    }
}

fn me_contact(
    id: NodeId,
    addr: SocketAddr,
    now: u64,
) -> snapcore::routing::Contact {
    snapcore::routing::Contact { id, addr, last_seen: now, failed: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> NodeId {
        NodeId([n; 20])
    }

    fn host() -> NodeHost {
        NodeHost::spawn(id(1), "127.0.0.1:0".parse().unwrap()).expect("bound")
    }

    #[test]
    fn a_publish_with_no_peers_reports_zero_rather_than_hanging() {
        // The state every node is in for its first few seconds. It must
        // come back, and it must not look like an error.
        let h = host();
        assert!(h.publish("sc-abcd", "Ada", "10.0.0.2", 5000));
        let ev = wait_for(&h);
        match ev {
            Some(Event::Published { peers, .. }) => assert_eq!(peers, 0),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_resolve_that_finds_nothing_still_answers() {
        // Silence from a lookup is indistinguishable from a hung one, so
        // "found nothing" has to arrive as an event.
        let h = host();
        assert!(h.resolve("sc-nobody"));
        match wait_for(&h) {
            Some(Event::Resolved { found, .. }) => assert!(found.is_empty()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bootstrap_grows_the_routing_table() {
        // A table with no contacts can find nobody, so this is the step
        // that makes every other one possible.
        let h = host();
        h.bootstrap("127.0.0.1:9".parse().unwrap(), id(2));
        match wait_for(&h) {
            Some(Event::Published { peers, .. }) => assert_eq!(peers, 1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn poll_drains_rather_than_returning_one() {
        // Three results in a frame should show three. Returning one per
        // frame would trickle a burst of discoveries in over three
        // frames for no reason.
        let h = host();
        for i in 0..3 {
            h.resolve(&format!("sc-{i}"));
        }
        let mut seen = 0;
        for _ in 0..50 {
            seen += h.poll().len();
            if seen >= 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(seen, 3, "poll did not drain");
    }

    #[test]
    fn polling_an_idle_host_is_empty_not_an_error() {
        // Called every frame. If an idle poll produced an event, the log
        // would fill at sixty lines a second.
        let h = host();
        assert!(h.poll().is_empty());
    }

    /// Poll until something arrives, or give up.
    ///
    /// Bounded rather than a bare loop: a test that hangs on a broken
    /// channel is worse than one that fails, because CI reports it as a
    /// timeout with no message.
    fn wait_for(h: &NodeHost) -> Option<Event> {
        for _ in 0..100 {
            if let Some(e) = h.poll().into_iter().next() {
                return Some(e);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        None
    }
}
