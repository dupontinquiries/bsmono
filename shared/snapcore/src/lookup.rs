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
        if !shortlist.iter().any(|s| s.id == nc.id) {
            shortlist.push(nc);
        }
    }
}

/// Find the `k` contacts closest to `target` reachable from `rt`.
pub fn lookup_nodes<T: Transport>(
    me: &Contact,
    rt: &mut RoutingTable,
    target: &NodeId,
    tx: &T,
    alpha: usize,
    now: u64,
) -> Vec<Contact> {
    let k = rt.params().k;
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
        for c in to_query {
            queried.insert(c.id);
            let found = tx.find_node(me, &c, target);
            rt.saw(c.id, c.addr, now); // it responded
            merge(&mut shortlist, found, &me.id, rt, now);
        }
    }

    sort_by_distance(&mut shortlist, target);
    shortlist.truncate(k);
    shortlist
}

/// Find a value by `key`, walking toward the nodes closest to it. Returns the
/// values on the first node that has them, or `None` if the closest reachable
/// nodes don't.
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
    let mut shortlist: Vec<Contact> = rt.closest(&target, k);
    let mut queried: HashSet<NodeId> = HashSet::new();

    loop {
        sort_by_distance(&mut shortlist, &target);
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
        for c in to_query {
            queried.insert(c.id);
            match tx.find_value(me, &c, key) {
                FindValue::Values(v) => {
                    rt.saw(c.id, c.addr, now);
                    return Some(v);
                }
                FindValue::Closest(found) => {
                    rt.saw(c.id, c.addr, now);
                    merge(&mut shortlist, found, &me.id, rt, now);
                }
            }
        }
    }
    None
}
