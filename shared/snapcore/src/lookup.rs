//! lookup.rs — iterative Kademlia lookups over the [`Transport`] trait.
//!
//! `lookup_nodes` converges on the `k` contacts closest to a target by repeatedly
//! querying the `alpha` closest *unqueried* nodes it knows, merging what they
//! return, and stopping once the `k` closest in its shortlist have all been
//! queried (querying farther nodes can't improve the top-k). `lookup_value` is
//! the same walk with FIND_VALUE, short-circuiting on the first hit. Both grow
//! the caller's routing table with everything learned en route.

use crate::node::{FindValue, Transport};
use crate::routing::{Contact, NodeId, RoutingTable};
use crate::store::Key;
use std::collections::HashSet;

fn sort_by_distance(list: &mut [Contact], target: &NodeId) {
    list.sort_by(|a, b| a.id.distance(target).cmp(&b.id.distance(target)));
}

fn merge(shortlist: &mut Vec<Contact>, found: Vec<Contact>, me: &NodeId, rt: &mut RoutingTable, now: u64) {
    for nc in found {
        if &nc.id == me {
            continue;
        }
        rt.saw(nc.id, nc.addr, now);
        // One address is one node, here as in the routing table: a reply
        // names the responder under its REAL id, and the shortlist entry
        // we reached it through carried a guessed one. Keeping both would
        // store to the same socket twice and count it as two peers.
        shortlist.retain(|s| s.addr != nc.addr || s.id == nc.id);
        if !shortlist.iter().any(|s| s.id == nc.id) {
            shortlist.push(nc);
        }
    }
}

/// Concurrent queries per lookup round -- OpenDHT's `MAX_REQUESTED_SEARCH_NODES`.
/// Callers passed `k` (20) here, which with a sequential transport meant twenty
/// round trips, each up to a full timeout, per round.
pub const ALPHA: usize = 4;

/// Distinct nodes holding a value before `lookup_value` stops walking. The first
/// holder can hold a stale copy (a node that missed the last republish keeps the
/// old record until it expires); the reference keeps collecting across the whole
/// search. Three agreeing-or-not holders near the key is enough to see the newest
/// record without walking the entire neighbourhood.
pub const VALUE_HOLDERS: usize = 3;

/// Find the `k` contacts closest to `target` reachable from `rt`.
///
/// A node that does not answer is marked failed in `rt` and dropped from the
/// shortlist, so it neither blocks the stop condition nor comes back in the next
/// lookup (`RoutingTable::closest` hides Bad contacts).
pub fn lookup_nodes<T: Transport>(
    me: &Contact,
    rt: &mut RoutingTable,
    target: &NodeId,
    tx: &T,
    alpha: usize,
    now: u64,
) -> Vec<Contact> {
    let k = rt.params().k;
    let alpha = alpha.max(1);
    let mut shortlist: Vec<Contact> = rt.closest(target, k);
    let mut queried: HashSet<NodeId> = HashSet::new();

    loop {
        sort_by_distance(&mut shortlist, target);
        let top_k_all_queried = shortlist.iter().take(k).all(|c| queried.contains(&c.id));
        if top_k_all_queried {
            break;
        }
        let to_query: Vec<Contact> = shortlist
            .iter()
            .filter(|c| !queried.contains(&c.id))
            .take(alpha)
            .cloned()
            .collect();
        if to_query.is_empty() {
            break;
        }
        for c in &to_query {
            queried.insert(c.id);
        }
        let answers = tx.query_nodes_many(me, &to_query, target);
        for (c, ans) in to_query.iter().zip(answers) {
            match ans {
                Some(found) => {
                    rt.saw(c.id, c.addr, now);
                    merge(&mut shortlist, found, &me.id, rt, now);
                }
                None => {
                    rt.failed(&c.id, now);
                    shortlist.retain(|s| s.id != c.id);
                }
            }
        }
    }

    sort_by_distance(&mut shortlist, target);
    shortlist.truncate(k);
    shortlist
}

/// Find a value by `key`, walking toward the nodes closest to it. Collects the
/// values from up to [`VALUE_HOLDERS`] nodes that have them (deduplicated, closest
/// holders first), rather than returning the first holder's copy, which may be
/// stale. `None` if no reachable node near the key has any.
pub fn lookup_value<T: Transport>(
    me: &Contact,
    rt: &mut RoutingTable,
    key: &Key,
    tx: &T,
    alpha: usize,
    now: u64,
) -> Option<Vec<Vec<u8>>> {
    let target = *key;
    let k = rt.params().k;
    let alpha = alpha.max(1);
    let mut shortlist: Vec<Contact> = rt.closest(&target, k);
    let mut queried: HashSet<NodeId> = HashSet::new();
    let mut values: Vec<Vec<u8>> = Vec::new();
    let mut holders = 0usize;

    loop {
        sort_by_distance(&mut shortlist, &target);
        let top_k_all_queried = shortlist.iter().take(k).all(|c| queried.contains(&c.id));
        if top_k_all_queried || holders >= VALUE_HOLDERS {
            break;
        }
        let to_query: Vec<Contact> = shortlist
            .iter()
            .filter(|c| !queried.contains(&c.id))
            .take(alpha)
            .cloned()
            .collect();
        if to_query.is_empty() {
            break;
        }
        for c in &to_query {
            queried.insert(c.id);
        }
        let answers = tx.query_value_many(me, &to_query, key);
        for (c, ans) in to_query.iter().zip(answers) {
            match ans {
                Some(FindValue::Values(v)) => {
                    rt.saw(c.id, c.addr, now);
                    holders += 1;
                    for b in v {
                        if !values.contains(&b) {
                            values.push(b);
                        }
                    }
                }
                Some(FindValue::Closest(found)) => {
                    rt.saw(c.id, c.addr, now);
                    merge(&mut shortlist, found, &me.id, rt, now);
                }
                None => {
                    rt.failed(&c.id, now);
                    shortlist.retain(|s| s.id != c.id);
                }
            }
        }
    }
    if values.is_empty() { None } else { Some(values) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::net::SocketAddr;

    fn nid(n: u8) -> NodeId {
        let mut b = [0u8; 20];
        b[0] = n;
        NodeId(b)
    }
    fn c(n: u8) -> Contact {
        Contact::new(nid(n), SocketAddr::from(([10, 0, 0, n], 4222)), 0)
    }

    /// A network of nodes by id: each knows some others and may hold values; ids in
    /// `dead` never answer. Counts queries per node.
    #[derive(Default)]
    struct Fake {
        knows: HashMap<NodeId, Vec<Contact>>,
        values: HashMap<NodeId, Vec<Vec<u8>>>,
        dead: Vec<NodeId>,
        asked: RefCell<HashMap<NodeId, u32>>,
    }
    impl Fake {
        fn ask(&self, id: &NodeId) -> bool {
            *self.asked.borrow_mut().entry(*id).or_insert(0) += 1;
            !self.dead.contains(id)
        }
    }
    impl Transport for Fake {
        fn find_node(&self, f: &Contact, t: &Contact, g: &NodeId) -> Vec<Contact> {
            self.query_nodes(f, t, g).unwrap_or_default()
        }
        fn find_value(&self, f: &Contact, t: &Contact, k: &Key) -> FindValue {
            self.query_value(f, t, k).unwrap_or(FindValue::Closest(vec![]))
        }
        fn store(&self, _: &Contact, _: &Contact, _: Key, _: Vec<u8>, _: u64) {}
        fn ping(&self, _: &Contact, t: &Contact) -> bool { self.ask(&t.id) }
        fn query_nodes(&self, _: &Contact, t: &Contact, _: &NodeId) -> Option<Vec<Contact>> {
            if !self.ask(&t.id) { return None; }
            Some(self.knows.get(&t.id).cloned().unwrap_or_default())
        }
        fn query_value(&self, _: &Contact, t: &Contact, _: &Key) -> Option<FindValue> {
            if !self.ask(&t.id) { return None; }
            Some(match self.values.get(&t.id) {
                Some(v) => FindValue::Values(v.clone()),
                None => FindValue::Closest(self.knows.get(&t.id).cloned().unwrap_or_default()),
            })
        }
    }

    /// A node that never answers is struck, dropped from the lookup, and after
    /// three lookups no longer offered by the table -- it stops costing a timeout
    /// per lookup. It used to be marked as having RESPONDED.
    #[test]
    fn a_silent_node_is_struck_and_then_skipped() {
        let me = c(1);
        let mut rt = RoutingTable::new(me.id);
        for n in 2..=5 { rt.saw(nid(n), c(n).addr, 0); }
        let fake = Fake { dead: vec![nid(3)], ..Default::default() };
        for _ in 0..3 {
            let found = lookup_nodes(&me, &mut rt, &nid(9), &fake, ALPHA, 1);
            assert!(!found.iter().any(|x| x.id == nid(3)), "a dead node was returned as a result");
        }
        assert_eq!(fake.asked.borrow()[&nid(3)], 3, "struck once per lookup");
        assert!(!rt.closest(&nid(9), 20).iter().any(|x| x.id == nid(3)), "three strikes and it is still offered");
        assert_eq!(rt.usable(), 3);
        let _ = lookup_nodes(&me, &mut rt, &nid(9), &fake, ALPHA, 2);
        assert_eq!(fake.asked.borrow()[&nid(3)], 3, "a Bad node was asked again");
        // An answering node that knows nobody is NOT struck -- empty is an answer.
        assert!(rt.closest(&nid(9), 20).iter().any(|x| x.id == nid(2)));
    }

    /// Values from several holders, not the first one's copy -- the first can be
    /// stale. Here the nearer holder has only the old record; the newer one is on
    /// the next holder and must be returned too.
    #[test]
    fn a_value_lookup_collects_from_several_holders() {
        let me = c(1);
        let mut rt = RoutingTable::new(me.id);
        let key = nid(0x80);
        for n in [0x81u8, 0x82, 0x83, 0x90] { rt.saw(nid(n), c(n).addr, 0); }
        let mut fake = Fake::default();
        fake.values.insert(nid(0x81), vec![b"old".to_vec()]);
        fake.values.insert(nid(0x82), vec![b"old".to_vec(), b"new".to_vec()]);
        fake.values.insert(nid(0x83), vec![b"new".to_vec()]);
        let got = lookup_value(&me, &mut rt, &key, &fake, ALPHA, 1).expect("values");
        assert_eq!(got, vec![b"old".to_vec(), b"new".to_vec()], "deduplicated, from more than the first holder");
        // Bounded: it stops at VALUE_HOLDERS holders rather than walking everyone.
        let mut many = Fake::default();
        let mut rt = RoutingTable::new(me.id);
        for n in 0x81u8..0x8a { rt.saw(nid(n), c(n).addr, 0); many.values.insert(nid(n), vec![vec![n]]); }
        let got = lookup_value(&me, &mut rt, &key, &many, 1, 1).unwrap();
        assert_eq!(got.len(), VALUE_HOLDERS, "walked past {VALUE_HOLDERS} holders: {got:?}");
        // Nobody has it: None, and dead nodes along the way do not change that.
        let mut none = Fake { dead: vec![nid(0x82)], ..Default::default() };
        none.knows.insert(nid(0x81), vec![c(0x83)]);
        let mut rt = RoutingTable::new(me.id);
        rt.saw(nid(0x81), c(0x81).addr, 0);
        rt.saw(nid(0x82), c(0x82).addr, 0);
        assert!(lookup_value(&me, &mut rt, &key, &none, ALPHA, 1).is_none());
    }
}
