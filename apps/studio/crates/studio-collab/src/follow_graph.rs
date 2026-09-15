//! follow_graph — resolving WHOSE view mode you actually see, when
//! following can chain.
//!
//! `follow` (studio-app) is local view state: who I've pointed my own
//! screen at. But once a peer's view MODE (not just their playhead) is
//! meant to come along for the ride, a single hop stops being enough --
//! if I follow someone who is themselves following a third person, I
//! should see what the ORIGINAL person (the one not following anyone)
//! is looking at, not my immediate followee's own independent mode.
//!
//! The rule, stated precisely: walk each peer's `follows` edge until
//! reaching someone who isn't following anyone (a ROOT) -- that root's
//! own view mode is what every peer in the chain effectively sees. A
//! root's followers inherit the root's mode; the root itself just sees
//! its own, same as if nobody were following it at all ("hands off to
//! your view mode when someone follows you and you don't follow anyone").
//!
//! "Follows" is a FUNCTIONAL graph: each peer follows at most one other
//! peer at a time (out-degree <= 1), so the structure is a forest of
//! chains hanging off roots, plus -- since nothing stops two people from
//! following each other -- possibly a cycle. A cycle has no root, so a
//! peer caught in one falls back to their own view mode rather than
//! resolving to nothing or looping forever.

use crate::SiteId;
use std::collections::{BTreeMap, BTreeSet};

/// For `peer`, walks `follows` (peer -> who they follow, `None` = a root)
/// to find whichever site's OWN view mode should be shown. Returns that
/// site's id -- the caller looks up the actual mode in whatever map holds
/// per-site view-mode values (kept separate on purpose: this function only
/// resolves the GRAPH, not the payload, the same separation
/// `group_tangles` keeps between "which groups are linked" and "what's in
/// them").
///
/// Cycle-safe: tracks the path walked so far, and if `peer` (or anyone
/// already on the path) is revisited, the whole cycle has no root to
/// defer to -- every site on it falls back to its OWN id.
pub fn resolve_effective_source(follows: &BTreeMap<SiteId, Option<SiteId>>, peer: SiteId) -> SiteId {
    let mut current = peer;
    let mut visited: BTreeSet<SiteId> = BTreeSet::new();
    loop {
        if !visited.insert(current) {
            // Revisited someone already on this walk -- a cycle. No site
            // on a cycle has a valid root to defer to, so the ORIGINAL
            // peer we were asked about falls back to itself.
            return peer;
        }
        match follows.get(&current).copied().flatten() {
            Some(next) => current = next,
            None => return current, // a root: not following anyone
        }
    }
}

/// The effective source for every peer in `follows` at once -- what a
/// view-mode broadcast actually needs each frame, computed once rather
/// than per-peer-per-frame. Deliberately not memoized/incremental: a
/// collab session's peer count is small (a handful, not thousands), and
/// correctness from a plain re-walk is worth more here than an
/// optimization nothing in this codebase has needed yet.
pub fn resolve_all(follows: &BTreeMap<SiteId, Option<SiteId>>) -> BTreeMap<SiteId, SiteId> {
    let mut out = BTreeMap::new();
    for &peer in follows.keys() {
        out.insert(peer, resolve_effective_source(follows, peer));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(pairs: &[(u64, Option<u64>)]) -> BTreeMap<SiteId, Option<SiteId>> {
        pairs.iter().map(|(p, f)| (SiteId(*p), f.map(SiteId))).collect()
    }

    #[test]
    fn a_root_resolves_to_itself() {
        let g = graph(&[(1, None)]);
        assert_eq!(resolve_effective_source(&g, SiteId(1)), SiteId(1));
    }

    #[test]
    fn a_direct_follower_resolves_to_the_root_it_follows() {
        // 2 follows 1, 1 follows nobody.
        let g = graph(&[(1, None), (2, Some(1))]);
        assert_eq!(resolve_effective_source(&g, SiteId(2)), SiteId(1));
        assert_eq!(resolve_effective_source(&g, SiteId(1)), SiteId(1), "a root always resolves to itself, follower or not");
    }

    /// The exact scenario that makes single-hop following insufficient:
    /// 3 follows 2, 2 follows 1, 1 follows nobody. Site 3 must resolve all
    /// the way to 1, not stop at 2.
    #[test]
    fn a_chain_of_follows_resolves_all_the_way_to_the_root() {
        let g = graph(&[(1, None), (2, Some(1)), (3, Some(2))]);
        assert_eq!(resolve_effective_source(&g, SiteId(3)), SiteId(1), "must walk the full chain, not stop at the immediate followee");
        assert_eq!(resolve_effective_source(&g, SiteId(2)), SiteId(1));
        assert_eq!(resolve_effective_source(&g, SiteId(1)), SiteId(1));
    }

    #[test]
    fn multiple_followers_of_the_same_root_all_resolve_to_it() {
        // A root can have any number of followers (in-degree is
        // unrestricted; only out-degree is capped at 1).
        let g = graph(&[(1, None), (2, Some(1)), (3, Some(1)), (4, Some(1))]);
        for follower in [2u64, 3, 4] {
            assert_eq!(resolve_effective_source(&g, SiteId(follower)), SiteId(1));
        }
    }

    #[test]
    fn a_two_cycle_falls_back_to_each_sites_own_id() {
        // 1 follows 2, 2 follows 1 -- no root exists on this cycle.
        let g = graph(&[(1, Some(2)), (2, Some(1))]);
        assert_eq!(resolve_effective_source(&g, SiteId(1)), SiteId(1), "no root on a cycle -- fall back to self");
        assert_eq!(resolve_effective_source(&g, SiteId(2)), SiteId(2));
    }

    #[test]
    fn a_self_follow_is_a_one_node_cycle() {
        let g = graph(&[(1, Some(1))]);
        assert_eq!(resolve_effective_source(&g, SiteId(1)), SiteId(1));
    }

    #[test]
    fn a_longer_cycle_with_a_tail_leaves_the_tail_resolving_into_the_cycle() {
        // 3 -> 1 -> 2 -> 1 (a 2-cycle between 1 and 2, with 3 following
        // into it). 3 is not ITSELF on the cycle, but chases into one --
        // must still terminate (not loop forever) and fall back sanely.
        let g = graph(&[(1, Some(2)), (2, Some(1)), (3, Some(1))]);
        // 3's own walk is 3 -> 1 -> 2 -> 1 (revisit) -- the cycle is
        // detected on 3's OWN walk, so 3 falls back to itself, same rule
        // as if 3 were on the cycle directly. This is a real product
        // choice worth naming: 3 does not get to "escape" into whichever
        // of 1/2 it saw first.
        assert_eq!(resolve_effective_source(&g, SiteId(3)), SiteId(3));
    }

    #[test]
    fn resolve_all_matches_calling_the_single_resolver_for_every_peer() {
        let g = graph(&[(1, None), (2, Some(1)), (3, Some(2)), (4, Some(4))]);
        let all = resolve_all(&g);
        for &peer in g.keys() {
            assert_eq!(all[&peer], resolve_effective_source(&g, peer), "resolve_all must agree with resolve_effective_source for every peer");
        }
    }

    #[test]
    fn an_empty_graph_resolves_nothing() {
        let g: BTreeMap<SiteId, Option<SiteId>> = BTreeMap::new();
        assert!(resolve_all(&g).is_empty());
    }

    /// A peer not present in `follows` at all (e.g. joined after the
    /// snapshot was taken) is treated as a root of itself -- `.get()`
    /// misses, `.flatten()` on `None` gives `None`, same as an explicit
    /// `(peer, None)` entry. Worth pinning so a missing-entry bug doesn't
    /// silently read as "follows nobody" by accident later.
    #[test]
    fn a_peer_missing_from_the_map_resolves_to_itself() {
        let g = graph(&[(1, None)]);
        assert_eq!(resolve_effective_source(&g, SiteId(2)), SiteId(2));
    }
}
