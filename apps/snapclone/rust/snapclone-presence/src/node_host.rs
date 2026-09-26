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
    /// A `host:port` to resolve ON THE WORKER. DNS blocks for as long as the
    /// resolver likes, and the app used to do it on its UI thread at spawn.
    BootstrapHost { host: String },
}

/// Something that happened, polled by the app.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Published: this many peers ACCEPTED the record (see `dht::publish`).
    /// Zero is normal on a fresh node.
    Published { sc_id: String, peers: usize },
    /// The routing table changed size: `usable` contacts that are not Bad.
    /// Sent after bootstrap entries are added, after the find-self walk that
    /// follows them, and after maintenance. Zero means the node is cut off.
    Table { usable: usize },
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
    // And the STRIKES. A lookup that found a node dead marked it on the copy;
    // merging only the contacts it saw alive threw that away, so the shared
    // table kept offering the same dead node to every lookup.
    let before: std::collections::HashMap<NodeId, u32> = n.rt.failures().into_iter().collect();
    for (id, f) in rt.failures() {
        let had = before.get(&id).copied().unwrap_or(0);
        for _ in had..f {
            n.rt.failed(&id, now);
        }
    }
    r
}

/// Walk toward our own id: the Kademlia join. It is what turns a bootstrap
/// entry -- an address under a GUESSED id -- into a real contact (the reply
/// names the responder), and what fills the buckets near us, which are the ones
/// asked about our key. Adding the entry alone taught the table one address and
/// nothing else.
fn find_self(node: &std::sync::Mutex<Node>, tx: &UdpTransport, id: NodeId, local: SocketAddr) -> usize {
    let now = now_secs();
    let me = me_contact(id, local, now);
    with_rt(node, now, |rt| {
        snapcore::lookup::lookup_nodes(&me, rt, &id, tx, snapcore::lookup::ALPHA, now);
    });
    usable(node)
}

fn usable(node: &std::sync::Mutex<Node>) -> usize {
    match node.lock() { Ok(n) => n.rt.usable(), Err(p) => p.into_inner().rt.usable() }
}

/// Resolve `host:port` to its first address, if any.
fn resolve_host(host: &str) -> Option<SocketAddr> {
    std::net::ToSocketAddrs::to_socket_addrs(host).ok()?.find(|a| a.is_ipv4())
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
        Self::spawn_tuned(id, bind, keepalive, REJOIN_EVERY)
    }

    /// `spawn` with both maintenance periods chosen — tests use short ones.
    pub fn spawn_tuned(id: NodeId, bind: SocketAddr, keepalive: std::time::Duration,
                       rejoin_every: std::time::Duration) -> std::io::Result<NodeHost> {
        let tx_udp = std::sync::Arc::new(UdpTransport::bind(id, bind)?);
        let local = tx_udp.local_addr()?;
        let (req_tx, req_rx) = std::sync::mpsc::channel::<Req>();
        let (ev_tx, ev_rx) = std::sync::mpsc::channel::<Event>();
        let node = std::sync::Arc::new(std::sync::Mutex::new(Node::new(id, local)));
        let serving = tx_udp.serve(node.clone())?;

        let transport = tx_udp.clone();
        std::thread::spawn(move || {
            // Everything we were ever told to bootstrap from, so a node whose
            // table empties (a network change, a long sleep) can join again.
            // The reference re-bootstraps on its own (`BOOTSTRAP_PERIOD`); this
            // node used to add the list once at spawn and never again.
            let mut seeds: Vec<Seed> = Vec::new();
            let mut need_join = false;
            let mut last_rejoin: Option<std::time::Instant> = None;
            // The table size last reported, so a keepalive that strikes the last
            // peer out says so (Event::Table { usable: 0 }) rather than leaving
            // the app to infer it from a run of unanswered pings.
            let mut reported_usable: Option<usize> = None;
            loop {
                // A join is due once the burst of bootstrap requests the app
                // sends at spawn has drained -- one walk for the lot, not one
                // per entry.
                let next = if need_join {
                    match req_rx.try_recv() {
                        Ok(r) => Some(r),
                        Err(TryRecvError::Empty) => None,
                        Err(TryRecvError::Disconnected) => return,
                    }
                } else {
                    // Blocking with a period, not a poll loop: the thread
                    // wakes once per keepalive when idle, and that wake is the
                    // keepalive and the table maintenance. An idle node costs
                    // one wake every 25 s -- see `KEEPALIVE`.
                    match req_rx.recv_timeout(keepalive) {
                        Ok(r) => Some(r),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                        // NodeHost was dropped; nobody is listening.
                        Err(_) => return,
                    }
                };
                let Some(req) = next else {
                    if need_join {
                        need_join = false;
                        let n = find_self(&node, &tx_udp, id, local);
                        if ev_tx.send(Event::Table { usable: n }).is_err() { return; }
                        continue;
                    }
                    // Idle: keepalive, then maintenance.
                    let now = now_secs();
                    let me = me_contact(id, local, now);
                    let targets = {
                        let n = match node.lock() { Ok(n) => n, Err(p) => p.into_inner() };
                        // The peers most likely to hold our mapping and to be
                        // asked about us: the ones nearest our own id. Bounded,
                        // because this runs for the life of the app.
                        n.rt.closest(&id, KEEPALIVE_PEERS)
                    };
                    if !targets.is_empty() {
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
                    }
                    // Outside the ping block: the tick that finds NO usable
                    // target is exactly the one that has to report zero.
                    let u = usable(&node);
                    if reported_usable != Some(u) {
                        reported_usable = Some(u);
                        if ev_tx.send(Event::Table { usable: u }).is_err() { return; }
                    }
                    // A THIN TABLE REJOINS. Below MIN_USABLE contacts a lookup
                    // has too few starting points to converge (and at zero,
                    // none); re-add every seed -- re-resolving hostnames, whose
                    // addresses may have changed with the network -- and walk
                    // to ourselves again. Paced by REJOIN_EVERY so a node with no
                    // network at all is not hammering a dead resolver.
                    let thin = usable(&node) < MIN_USABLE;
                    let due = last_rejoin.map_or(true, |t| t.elapsed() >= rejoin_every);
                    if thin && due && !seeds.is_empty() {
                        last_rejoin = Some(std::time::Instant::now());
                        let now = now_secs();
                        {
                            let mut n = match node.lock() { Ok(n) => n, Err(p) => p.into_inner() };
                            for s in &seeds {
                                if let Some((addr, sid)) = s.resolve() { n.rt.saw(sid, addr, now); }
                            }
                        }
                        let n = find_self(&node, &tx_udp, id, local);
                        if ev_tx.send(Event::Table { usable: n }).is_err() { return; }
                    }
                    continue;
                };
                let now = now_secs();
                let ev = match req {
                    Req::Bootstrap { addr, id: peer } => {
                        // A routing table with no contacts can find nobody, so
                        // this is the step that makes every other one possible.
                        // The join walk follows once the burst has drained.
                        seeds.push(Seed::Addr(addr, peer));
                        let mut n = match node.lock() { Ok(n) => n, Err(p) => p.into_inner() };
                        n.rt.saw(peer, addr, now);
                        need_join = true;
                        Event::Table { usable: n.rt.usable() }
                    }
                    Req::BootstrapHost { host } => {
                        let seed = Seed::Host(host);
                        if let Some((addr, sid)) = seed.resolve() {
                            let mut n = match node.lock() { Ok(n) => n, Err(p) => p.into_inner() };
                            n.rt.saw(sid, addr, now);
                            need_join = true;
                        }
                        // Kept even when it did not resolve now: DNS may come
                        // back with the network, and maintenance retries it.
                        seeds.push(seed);
                        Event::Table { usable: usable(&node) }
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

    /// Add a known peer so lookups have somewhere to start. Remembered, and
    /// re-added whenever the table runs thin (see `MIN_USABLE`).
    pub fn bootstrap(&self, addr: SocketAddr, id: NodeId) -> bool {
        self.tx.send(Req::Bootstrap { addr, id }).is_ok()
    }

    /// Bootstrap from a `host:port`, resolved on the worker thread -- never on
    /// the caller's. The peer's id is guessed from the name until it answers
    /// (the join walk corrects it). Remembered and re-resolved on rejoin.
    pub fn bootstrap_host(&self, host: &str) -> bool {
        self.tx.send(Req::BootstrapHost { host: host.to_string() }).is_ok()
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

/// How long a published record lives, in seconds: OpenDHT's
/// `DEFAULT_VALUE_EXPIRATION` (10 min, `value.h`), which is what every reference
/// node storing our record applies whatever we send. Used for our OWN copy, so
/// we stop answering with a record at the same moment the network does.
///
/// Republishing is the caller's job and must come in under this -- see
/// `REPUBLISH_WITHIN`. It was 300 here while the app republished every 55
/// minutes, so our record existed on the network about one minute in five.
pub const DEFAULT_TTL: u64 = 600;

/// The latest a caller should republish, in seconds: the value lifetime less a
/// margin for the walk and for peers whose clocks run ahead. The C++ build
/// republishes its non-permanent records every 8 minutes against the same
/// 10-minute expiry (`JamiAdapter_dht_impl.cpp`).
pub const REPUBLISH_WITHIN: u64 = 8 * 60;

/// Below this many usable contacts the node rejoins from its seeds. A lookup
/// wants `ALPHA` starting points at the very least, and a table this thin is one
/// dead router away from none.
pub const MIN_USABLE: usize = 8;

/// The shortest interval between rejoins. The reference's `BOOTSTRAP_PERIOD` is
/// 10 s; this node rejoins from the keepalive tick (25 s) and no more often than
/// this, so a phone with no network is not waking to hammer a dead resolver.
pub const REJOIN_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// A remembered bootstrap source.
#[derive(Clone, Debug)]
enum Seed {
    Addr(SocketAddr, NodeId),
    Host(String),
}

impl Seed {
    fn resolve(&self) -> Option<(SocketAddr, NodeId)> {
        match self {
            Seed::Addr(a, id) => Some((*a, *id)),
            // The id is unknown until the node answers. Derived from the name so
            // two servers do not collide in the table; the reply corrects it.
            Seed::Host(h) => resolve_host(h).map(|a| (a, guessed_id(h))),
        }
    }
}

/// A placeholder id for a seed known only by name: SHA-1 of the name (the same
/// derivation sc-ID keys use), so it is stable and spread across the keyspace.
fn guessed_id(name: &str) -> NodeId {
    NodeId(crate::presence::key_for_scid(name).0)
}

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
            Some(Event::Table { usable }) => assert_eq!(usable, 1),
            other => panic!("{other:?}"),
        }
    }

    /// Strikes a lookup records on its COPY of the table reach the shared table.
    /// Only contacts seen alive used to be merged back, so a dead node found by
    /// one lookup was offered to the next one fresh.
    #[test]
    fn a_lookups_strikes_are_merged_back() {
        let node = std::sync::Mutex::new(Node::new(id(1), "127.0.0.1:1".parse().unwrap()));
        let dead = id(9);
        node.lock().unwrap().rt.saw(dead, "127.0.0.1:9".parse().unwrap(), 0);
        for round in 1..=3u32 {
            with_rt(&node, 10, |rt| rt.failed(&dead, 10));
            let f = node.lock().unwrap().rt.failures().into_iter().find(|(i, _)| *i == dead).unwrap().1;
            assert_eq!(f, round, "strike {round} was lost in the merge");
        }
        assert_eq!(node.lock().unwrap().rt.usable(), 0, "three merged strikes should make it Bad");
    }

    /// A hostname is resolved by the worker, not the caller, and reaches the table.
    #[test]
    fn a_hostname_seed_is_resolved_off_the_callers_thread() {
        let h = host();
        let started = std::time::Instant::now();
        assert!(h.bootstrap_host("localhost:9"));
        assert!(started.elapsed() < std::time::Duration::from_millis(50), "bootstrap_host blocked the caller");
        match wait_for(&h) {
            Some(Event::Table { usable }) => assert_eq!(usable, 1, "localhost did not reach the table"),
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
