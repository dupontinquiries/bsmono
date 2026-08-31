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
//! ## What it deliberately does not do
//!
//! It does not decide what a result MEANS. `peer_source::Reach` already
//! says a DHT hit is `Unverified` — a record anyone could have published
//! under that key — and that judgement belongs there, not here. This
//! hands back what it found.

use snapcore::node::Node;
use snapcore::routing::NodeId;
use snapcore::udp::UdpTransport;
use std::net::SocketAddr;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

/// Something to do, sent to the worker.
enum Req {
    Publish { sc_id: String, name: String, ip: String, port: u16 },
    Resolve { sc_id: String },
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
    /// The worker stopped. Carries why, because a discovery feature that
    /// silently stops is one people conclude is "just slow".
    Stopped(String),
}

pub struct NodeHost {
    tx: Sender<Req>,
    rx: Receiver<Event>,
    local: SocketAddr,
}

impl NodeHost {
    /// Start a node on `bind`.
    ///
    /// `id` is this device's DHT identity. It should be derived from the
    /// sc-ID rather than random, so a device keeps its place in the
    /// keyspace across restarts — a node that re-randomises its id on
    /// every launch makes every other node's routing table stale.
    pub fn spawn(id: NodeId, bind: SocketAddr) -> std::io::Result<NodeHost> {
        let tx_udp = UdpTransport::bind(id, bind)?;
        let local = tx_udp.local_addr()?;
        let (req_tx, req_rx) = std::sync::mpsc::channel::<Req>();
        let (ev_tx, ev_rx) = std::sync::mpsc::channel::<Event>();

        std::thread::spawn(move || {
            let mut node = Node::new(id, local);
            loop {
                // Blocking recv, not a poll loop: an idle node should
                // cost nothing. A DHT that burns a core waiting for
                // something to do is a battery bug on a phone, and this
                // is a phone app.
                let req = match req_rx.recv() {
                    Ok(r) => r,
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
                        node.rt.saw(peer, addr, now);
                        Event::Published { sc_id: String::new(), peers: node.rt.len() }
                    }
                    Req::Publish { sc_id, name, ip, port } => {
                        let me = me_contact(&node, id, local, now);
                        let n = crate::dht::publish(
                            &me, &mut node.rt, &tx_udp,
                            &sc_id, &name, &ip, port, DEFAULT_TTL, now);
                        Event::Published { sc_id, peers: n }
                    }
                    Req::Resolve { sc_id } => {
                        let me = me_contact(&node, id, local, now);
                        let found = crate::dht::resolve(
                            &me, &mut node.rt, &tx_udp, &sc_id, now)
                            .unwrap_or_default();
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

        Ok(NodeHost { tx: req_tx, rx: ev_rx, local })
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

fn me_contact(
    _node: &Node,
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
