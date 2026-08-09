//! Parity: do the two DHT engines behave the same on the same LAN exchange?
//!
//! The fleet has two Kademlia engines — `snapcore` (shared: better routing,
//! store, hole-punch, and no wire format) and this crate's `Engine` (thinner,
//! with the OpenDHT-shaped wire). The convergence plan keeps snapcore's engine
//! and gives it this wire, and that is only safe if the two agree about what a
//! DHT does.
//!
//! ## What parity can and cannot mean here
//!
//! **Not byte-identical packets.** snapcore has no wire at all, so there is
//! nothing to compare bytes against — asserting that would assert something
//! that cannot be true.
//!
//! **Observable behaviour on the same exchange**: given N nodes, a bootstrap, a
//! put, and a get from a node that does not hold the value, do both deliver the
//! same bytes from a lookup that actually traversed other nodes? That is what a
//! caller depends on and what must survive swapping the engine underneath.
//!
//! Both run with **no sockets and a fixed clock** — each sim calls straight into
//! the target's handler — so a disagreement is a behavioural difference rather
//! than a timing race.

use snapclone_dht::core::{Addr, InfoHash, Value};
use snapclone_dht::engine::{Engine, Out};

struct DhtSim {
    nodes: Vec<Engine>,
    q: std::collections::VecDeque<(Addr, Addr, Vec<u8>)>,
    now: u64,
}

impl DhtSim {
    fn new(n: usize) -> DhtSim {
        let nodes = (0..n)
            .map(|i| Engine::new(InfoHash::from_seed(0xA11CE + i as u64 * 0x9E37)))
            .collect();
        DhtSim { nodes, q: Default::default(), now: 1 }
    }
    fn push(&mut self, from: Addr, outs: Vec<Out>) {
        for o in outs { self.q.push_back((from, o.to, o.bytes)); }
    }
    fn settle(&mut self, cap: usize) {
        let mut steps = 0;
        while let Some((from, to, bytes)) = self.q.pop_front() {
            steps += 1;
            assert!(steps <= cap, "sim did not settle in {cap} steps");
            self.now += 1;
            let outs = self.nodes[to as usize].handle_packet(from, &bytes, self.now);
            self.push(to, outs);
        }
    }
    fn bootstrap_all(&mut self) {
        for i in 1..self.nodes.len() {
            let outs = self.nodes[i].bootstrap(0, self.now);
            self.push(i as Addr, outs);
        }
        self.settle(40_000);
    }
    fn put_get(&mut self, key: InfoHash, payload: &[u8], writer: usize, reader: usize)
        -> Vec<Vec<u8>> {
        let outs = self.nodes[writer].put(key, Value { id: 1, data: payload.to_vec() }, self.now);
        self.push(writer as Addr, outs);
        self.settle(40_000);
        let outs = self.nodes[reader].get(key, self.now);
        self.push(reader as Addr, outs);
        self.settle(40_000);
        self.nodes[reader].take_result(&key).into_iter().map(|v| v.data).collect()
    }
}

/// The same exchange on snapcore. `store` false skips the put, so the same code
/// covers the "nobody stored it" case — two near-identical harnesses is how they
/// drift apart and stop comparing the same thing.
fn snapcore_exchange(n: usize, key_bytes: [u8; 20], payload: &[u8], store: bool)
    -> Vec<Vec<u8>> {
    use snapcore::lookup::{lookup_nodes, lookup_value};
    use snapcore::node::Node;
    use snapcore::routing::{NodeId, RoutingTable};
    use snapcore::sim::{Network, SimTransport};

    let mut net = Network::new();
    let ids: Vec<NodeId> = (0..n)
        .map(|i| {
            let mut b = [0u8; 20];
            b[0] = (i as u8).wrapping_mul(37);
            b[19] = i as u8;
            NodeId(b)
        })
        .collect();
    for (i, id) in ids.iter().enumerate() {
        net.add(Node::new(*id, format!("127.0.0.1:{}", 7000 + i).parse().unwrap()));
    }
    let now = 1_000u64;
    for a in &ids { for b in &ids { if a != b { net.introduce(a, b, now); } } }

    let key = NodeId(key_bytes);
    let tx = SimTransport { net: &net, now };

    let table_for = |idx: usize| {
        let nd = net.get(&ids[idx]);
        let (me, mut rt) = {
            let n = nd.borrow();
            (n.contact(now), RoutingTable::new(n.id()))
        };
        for (j, id) in ids.iter().enumerate() {
            if j != idx {
                let c = net.get(id).borrow().contact(now);
                rt.saw(c.id, c.addr, now);
            }
        }
        (me, rt)
    };

    if store {
        let (me, mut rt) = table_for(0);
        for c in lookup_nodes(&me, &mut rt, &key, &tx, 3, now).iter().take(3) {
            if let Some(nd) = net.nodes.get(&c.id) {
                nd.borrow_mut().handle_store(&me, key, payload.to_vec(), 300, now);
            }
        }
    }
    // Read from the last node, which never performed a store.
    let (me, mut rt) = table_for(n - 1);
    lookup_value(&me, &mut rt, &key, &tx, 3, now).unwrap_or_default()
}

const PAYLOAD: &[u8] = b"the same bytes through both engines";

#[test]
fn both_engines_deliver_a_put_to_a_reader_that_did_not_store_it() {
    let key = InfoHash::from_seed(0xBEEF);
    let mut sim = DhtSim::new(8);
    sim.bootstrap_all();
    let dht = sim.put_get(key, PAYLOAD, 1, 7);
    let core = snapcore_exchange(8, key.0, PAYLOAD, true);

    assert_eq!(dht.len(), 1, "snapclone-dht delivered {dht:?}");
    assert_eq!(core.len(), 1, "snapcore delivered {core:?}");
    assert_eq!(dht[0], PAYLOAD);
    assert_eq!(core[0], PAYLOAD, "the two engines agree on the payload");
}

#[test]
fn neither_engine_invents_a_value_for_a_key_nobody_stored() {
    // The easier half to get wrong: an engine that returns something for an
    // unknown key is worse than one that returns nothing, because the caller
    // cannot tell the difference.
    let stored = InfoHash::from_seed(0xBEEF);
    let missing = InfoHash::from_seed(0xFACE);
    let mut sim = DhtSim::new(8);
    sim.bootstrap_all();
    let _ = sim.put_get(stored, PAYLOAD, 1, 7);

    let outs = sim.nodes[7].get(missing, sim.now);
    sim.push(7, outs);
    sim.settle(40_000);
    assert!(sim.nodes[7].take_result(&missing).is_empty(), "snapclone-dht invented a value");
    assert!(snapcore_exchange(8, missing.0, PAYLOAD, false).is_empty(),
            "snapcore invented a value");
}

#[test]
fn both_engines_converge_at_the_same_network_sizes() {
    // Parity of SHAPE rather than of one outcome: if one needed twice the
    // network to find a value, swapping engines would change behaviour under
    // real conditions even though both pass the delivery test.
    for n in [4usize, 8, 16] {
        let key = InfoHash::from_seed(0xBEEF ^ n as u64);
        let mut sim = DhtSim::new(n);
        sim.bootstrap_all();
        let dht = sim.put_get(key, PAYLOAD, 1, n - 1);
        assert_eq!(dht.len(), 1, "snapclone-dht failed at n={n}");
        assert_eq!(snapcore_exchange(n, key.0, PAYLOAD, true).len(), 1,
                   "snapcore failed at n={n}");
    }
}

#[test]
fn both_engines_carry_a_payload_too_big_for_one_msgpack_header_class() {
    // A LAN transfer is not 30 bytes. This is where a wire that works in the
    // small starts writing the wrong length prefix, and where an engine that
    // caps value size silently diverges from one that does not.
    for size in [64usize, 300, 4096] {
        let payload = vec![0x5Au8; size];
        let key = InfoHash::from_seed(0xD00D ^ size as u64);
        let mut sim = DhtSim::new(8);
        sim.bootstrap_all();
        let dht = sim.put_get(key, &payload, 1, 7);
        let core = snapcore_exchange(8, key.0, &payload, true);
        assert_eq!(dht.len(), 1, "snapclone-dht lost a {size}-byte value");
        assert_eq!(core.len(), 1, "snapcore lost a {size}-byte value");
        assert_eq!(dht[0], payload, "snapclone-dht corrupted {size} bytes");
        assert_eq!(core[0], payload, "snapcore corrupted {size} bytes");
    }
}
