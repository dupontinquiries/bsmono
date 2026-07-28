//! sim — deterministic in-memory network of Engine nodes. Mirrors the reference's own
//! `python/tools/dht/virtual_network_builder.py` strategy: no sockets, a global clock, packets
//! delivered in FIFO order, so a whole DHT's convergence is a pure function of the seed.

use crate::core::*;
use crate::engine::{Engine, Out};

struct Sim {
    nodes: Vec<Engine>,
    q: std::collections::VecDeque<(Addr, Addr, Vec<u8>)>, // (from, to, bytes)
    now: u64,
}

impl Sim {
    fn new(n: usize) -> Sim {
        let nodes = (0..n).map(|i| Engine::new(InfoHash::from_seed(0xA11CE + i as u64 * 0x9E37))).collect();
        Sim { nodes, q: Default::default(), now: 1 }
    }
    fn push(&mut self, from: Addr, outs: Vec<Out>) {
        for o in outs { self.q.push_back((from, o.to, o.bytes)); }
    }
    fn settle(&mut self, cap: usize) {
        let mut steps = 0;
        while let Some((from, to, bytes)) = self.q.pop_front() {
            steps += 1; if steps > cap { panic!("sim did not settle in {cap} steps"); }
            self.now += 1;
            let outs = self.nodes[to as usize].handle_packet(from, &bytes, self.now);
            self.push(to, outs);
        }
    }
    fn bootstrap_all(&mut self) {
        // node 0 is the bootstrap; every other node pings it and self-finds
        for i in 1..self.nodes.len() {
            let outs = self.nodes[i].bootstrap(0, self.now);
            self.push(i as Addr, outs);
            self.settle(100_000);
        }
        // a second self-find round now that the table is seeded deepens buckets
        for i in 0..self.nodes.len() {
            let id = self.nodes[i].id;
            let outs = self.nodes[i].get(id, self.now); // find-node ride
            self.push(i as Addr, outs);
            self.settle(100_000);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{Msg, Q};

    #[test]
    fn wire_round_trips_every_message() {
        let a = InfoHash::from_seed(1);
        let b = InfoHash::from_seed(2);
        let v = Value { id: 42, data: b"snap".to_vec() };
        let msgs = vec![
            Msg::Query { tid: 7, from: a, q: Q::Get, target: b, token: 0, value: None },
            Msg::Query { tid: 8, from: a, q: Q::Put, target: b, token: 0x1234, value: Some(v.clone()) },
            Msg::Reply { tid: 7, from: b, nodes: vec![(a, 5), (b, 6)], values: vec![v.clone()], token: 0x99 },
            Msg::Push  { tid: 9, from: b, target: b, values: vec![v.clone()] },
            Msg::Error { tid: 7, code: 401 },
        ];
        for m in msgs {
            let enc = m.encode();
            let dec = Msg::decode(&enc).expect("decodes");
            assert_eq!(m, dec, "round-trip mismatch");
        }
    }

    #[test]
    fn xor_distance_and_common_bits() {
        let a = InfoHash([0; HASH_LEN]);
        let mut bb = [0u8; HASH_LEN]; bb[0] = 0b0010_0000;
        let b = InfoHash(bb);
        assert_eq!(InfoHash::common_bits(&a, &b), 2, "two leading zero bits then a 1");
        assert_eq!(InfoHash::common_bits(&a, &a), (HASH_LEN * 8) as u32, "identical = full length");
    }

    #[test]
    fn bucket_split_keeps_closest_and_caps_at_target() {
        let own = InfoHash::from_seed(100);
        let mut t = RoutingTable::new(own);
        for i in 0..200u64 { t.on_seen(InfoHash::from_seed(1000 + i), i, 1); }
        // every bucket respects the K cap
        for b in &t.buckets { assert!(b.len() <= TARGET_NODES, "bucket over K"); }
        // closest(own) returns the nearest by XOR, sorted
        let close = t.closest(&own, 8);
        for w in close.windows(2) {
            assert!(w[0].id.xor(&own).0 <= w[1].id.xor(&own).0, "closest must be distance-sorted");
        }
    }

    #[test]
    fn network_converges_and_get_finds_a_put() {
        let mut sim = Sim::new(30);
        sim.bootstrap_all();
        // every node should have learned a non-trivial routing table
        let filled = sim.nodes.iter().filter(|n| n.table.len() >= 8).count();
        assert!(filled >= 20, "most nodes should fill their tables (got {filled}/30)");

        // node 3 puts a value under a key; node 17 should get it back
        let key = InfoHash::from_seed(0xDEAD_BEEF);
        let val = Value { id: 1, data: b"hello dht".to_vec() };
        let outs = sim.nodes[3].put(key, val.clone(), sim.now);
        sim.push(3, outs);
        sim.settle(100_000);

        let outs = sim.nodes[17].get(key, sim.now);
        sim.push(17, outs);
        sim.settle(100_000);
        let got = sim.nodes[17].take_result(&key);
        assert!(got.contains(&val), "get must return the stored value (got {got:?})");
    }

    #[test]
    fn announce_without_a_token_is_rejected() {
        // a node that never did a get can't announce: forging a Put with token 0 gets a 401
        let mut sim = Sim::new(6);
        sim.bootstrap_all();
        let key = InfoHash::from_seed(7);
        let forged = Msg::Query { tid: 1, from: sim.nodes[2].id, q: Q::Put,
            target: key, token: 0, value: Some(Value { id: 9, data: vec![1] }) };
        let outs = sim.nodes[4].handle_packet(2, &forged.encode(), sim.now);
        let got_error = outs.iter().any(|o| matches!(Msg::decode(&o.bytes), Some(Msg::Error { code: 401, .. })));
        assert!(got_error, "token-less announce must be refused with 401");
        // and nothing was stored
        assert!(sim.nodes[4].store.get(&key, sim.now).is_empty(), "rejected put must not store");
    }

    #[test]
    fn listen_receives_a_later_put_via_push() {
        let mut sim = Sim::new(24);
        sim.bootstrap_all();
        let key = InfoHash::from_seed(0x1337);
        // node 5 listens
        let outs = sim.nodes[5].listen(key, sim.now);
        sim.push(5, outs);
        sim.settle(100_000);
        // node 11 puts — the storing node should push to node 5's listen registration
        let val = Value { id: 2, data: b"pushed".to_vec() };
        let outs = sim.nodes[11].put(key, val.clone(), sim.now);
        sim.push(11, outs);
        sim.settle(100_000);
        let pushed = sim.nodes[5].pushed.iter().any(|(k, v)| *k == key && *v == val);
        assert!(pushed, "a listener must receive later puts via push");
    }

    #[test]
    fn values_expire() {
        let mut st = Storage::default();
        let k = InfoHash::from_seed(1);
        st.put(k, Value { id: 1, data: vec![9] }, 1_000);
        assert_eq!(st.get(&k, 2_000).len(), 1);
        st.expire(1_000 + VALUE_EXPIRE_MS + 1);
        assert!(st.get(&k, 1_000 + VALUE_EXPIRE_MS + 1).is_empty(), "value should expire after 10min");
    }
}
