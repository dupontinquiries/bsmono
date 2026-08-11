//! peer_dial — which address to try, and which to refuse, from
//! `AppController_commands`.
//!
//! [`crate::handshake`] runs once a socket is open. This decides what to open
//! it to: a peer advertises several addresses and they are not equally good,
//! and one class of them is actively harmful.
//!
//! ## The hairpin bug, and why the obvious guard missed it
//!
//! Recorded in the source with its symptom: *"both phones default to the
//! gateway IP 172.16.42.1 when each is its own hotspot host, so a peer's
//! advertised `ws://172.16.42.1:9876` is ALSO our own server address."*
//!
//! The original guard compared only against the **public** address discovered
//! by STUN, so the LAN self-address slipped through — *"and we connected to
//! ourselves — the log showed a self↔self handshake looping saveContact / SHK2 /
//! wayfinder-refresh for our own id."*
//!
//! An infinite handshake loop with itself, on a phone, over the radio. The fix
//! is to treat **any** address whose host is one of our live interface IPs as a
//! hairpin — not just the public one.
//!
//! ## Reranking must apply to both paths or cold start is worse than warm
//!
//! *"Previously only the cache-hit path applied the learned per-peer ordering;
//! live-DHT results were appended in raw arrival order, so the first
//! lookup-after-cache-miss got worse routing decisions than the second... This
//! made the cold-start case feel worse than steady-state with the same peer —
//! opposite of what we wanted."*
//!
//! A learner wired into one of two paths is a learner that makes the first
//! experience the worst one.

/// One advertised way to reach a peer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PeerAddress {
    pub host: String,
    pub port: u16,
}

impl PeerAddress {
    pub fn new(host: &str, port: u16) -> PeerAddress {
        PeerAddress { host: host.to_string(), port }
    }
}

/// Is this a private address? RFC1918, CGNAT, link-local, ULA and loopback in
/// one place, because the inline version of this check drifted between call
/// sites in the original.
pub fn is_private(host: &str) -> bool {
    if host == "localhost" { return true; }
    if let Ok(v4) = host.parse::<std::net::Ipv4Addr>() {
        let o = v4.octets();
        return v4.is_loopback()
            || v4.is_link_local()
            || o[0] == 10
            || (o[0] == 172 && (16..=31).contains(&o[1]))
            || (o[0] == 192 && o[1] == 168)
            // CGNAT 100.64.0.0/10 — a carrier-grade NAT address is not
            // routable between subscribers, so it behaves like a private one.
            || (o[0] == 100 && (64..=127).contains(&o[1]));
    }
    if let Ok(v6) = host.parse::<std::net::Ipv6Addr>() {
        let s = v6.segments();
        // ULA fc00::/7 and link-local fe80::/10.
        return v6.is_loopback()
            || (s[0] & 0xFE00) == 0xFC00
            || (s[0] & 0xFFC0) == 0xFE80;
    }
    false
}

/// Would dialling this address connect us to ourselves?
///
/// Checks the public address AND every live interface IP. Checking only the
/// public one is the bug this function exists for: on a hotspot both phones
/// answer to the same gateway IP, so the peer's address is also ours.
pub fn is_hairpin(addr: &PeerAddress, public_host: Option<&str>, local_hosts: &[String]) -> bool {
    if let Some(p) = public_host {
        if !p.is_empty() && addr.host == p { return true; }
    }
    local_hosts.iter().any(|h| !h.is_empty() && *h == addr.host)
}

/// Why an address was not dialled.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Skip {
    /// It is one of ours — dialling it loops back.
    Hairpin,
    /// Port zero is not dialable.
    NoPort,
}

/// Filter a peer's advertised addresses down to the dialable ones.
pub fn dialable(addrs: &[PeerAddress], public_host: Option<&str>, local_hosts: &[String])
    -> Vec<PeerAddress> {
    addrs.iter()
        .filter(|a| classify(a, public_host, local_hosts).is_none())
        .cloned()
        .collect()
}

pub fn classify(addr: &PeerAddress, public_host: Option<&str>, local_hosts: &[String])
    -> Option<Skip> {
    if addr.port == 0 { return Some(Skip::NoPort); }
    if is_hairpin(addr, public_host, local_hosts) { return Some(Skip::Hairpin); }
    None
}

/// Priority given to an address whose class the learner has no data for.
///
/// Large, so unknown classes sort AFTER everything the learner has an opinion
/// about — but they keep their relative order among themselves, which is what
/// preserves the heuristic ordering underneath.
pub const UNKNOWN_PRIORITY: u32 = u32::MAX;

/// Address class, for the learner's per-peer ranking.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AddrClass { SameLan, Private, Public }

pub fn class_of(host: &str, local_prefix_match: bool) -> AddrClass {
    if local_prefix_match { AddrClass::SameLan }
    else if is_private(host) { AddrClass::Private }
    else { AddrClass::Public }
}

/// Reorder addresses by learned class priority.
///
/// **Stable**, and that is the whole design: *"addresses whose class the learner
/// has no data for keep their original heuristic position."* An unstable sort
/// would discard the heuristic ordering the learner is supposed to be refining,
/// so a peer with partial data would route worse than one with none.
///
/// An empty ranking is a no-op, which is what preserves the A/B switch — with
/// tuning off, the learner returns nothing and this function must not reorder.
pub fn rerank(addrs: &mut [PeerAddress], class_of_addr: impl Fn(&PeerAddress) -> AddrClass,
              ranking: &[AddrClass]) {
    if ranking.is_empty() { return; }
    let priority = |a: &PeerAddress| -> u32 {
        let c = class_of_addr(a);
        ranking.iter().position(|r| *r == c).map(|i| i as u32).unwrap_or(UNKNOWN_PRIORITY)
    };
    // sort_by_key on a slice is stable in Rust, which is exactly the property
    // the C++ comment demands of its stable_sort.
    addrs.sort_by_key(priority);
}

/// Attempts before a DHT relookup is abandoned.
pub const MAX_RELOOKUPS: u32 = 3;

/// May we look this peer up again?
pub fn may_relookup(attempts: u32) -> bool { attempts < MAX_RELOOKUPS }

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(h: &str) -> PeerAddress { PeerAddress::new(h, 9876) }

    #[test]
    fn the_hotspot_gateway_is_recognised_as_ourselves() {
        // The recorded bug: "both phones default to the gateway IP 172.16.42.1
        // when each is its own hotspot host, so a peer's advertised address is
        // ALSO our own server address" — and the result was a self-to-self
        // handshake LOOPING over the radio.
        let local = vec!["172.16.42.1".to_string()];
        assert!(is_hairpin(&addr("172.16.42.1"), Some("203.0.113.9"), &local));
    }

    #[test]
    fn checking_only_the_public_address_is_what_missed_it() {
        // The original guard compared against the STUN-discovered public host
        // only, so the LAN self-address slipped through. This asserts the gap
        // rather than describing it.
        let local = vec!["172.16.42.1".to_string()];
        let public = Some("203.0.113.9");
        // public-only logic would say false here:
        assert_ne!(addr("172.16.42.1").host, "203.0.113.9");
        // the real guard says true:
        assert!(is_hairpin(&addr("172.16.42.1"), public, &local));
    }

    #[test]
    fn the_public_address_is_still_a_hairpin() {
        // The original check was incomplete, not wrong — it must survive.
        assert!(is_hairpin(&addr("203.0.113.9"), Some("203.0.113.9"), &[]));
    }

    #[test]
    fn a_genuine_peer_address_is_not_a_hairpin() {
        let local = vec!["172.16.42.1".to_string(), "10.0.0.5".to_string()];
        assert!(!is_hairpin(&addr("192.168.1.44"), Some("203.0.113.9"), &local));
    }

    #[test]
    fn an_empty_self_address_matches_nothing() {
        // Before STUN completes the public host is empty, and an empty-matches
        // -empty bug would make every address look like a hairpin and the app
        // would connect to nobody.
        assert!(!is_hairpin(&addr(""), Some(""), &["".to_string()]));
        assert!(!is_hairpin(&addr("192.168.1.2"), None, &[]));
    }

    #[test]
    fn hairpins_are_filtered_out_of_the_dial_list() {
        let local = vec!["172.16.42.1".to_string()];
        let addrs = vec![addr("172.16.42.1"), addr("192.168.1.44"), addr("203.0.113.7")];
        let out = dialable(&addrs, Some("203.0.113.7"), &local);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].host, "192.168.1.44");
    }

    #[test]
    fn a_zero_port_is_not_dialable() {
        assert_eq!(classify(&PeerAddress::new("192.168.1.2", 0), None, &[]), Some(Skip::NoPort));
    }

    #[test]
    fn private_covers_rfc1918_cgnat_link_local_ula_and_loopback() {
        // One place, because the inline version of this check drifted between
        // call sites in the original.
        for h in ["10.1.2.3", "172.16.0.1", "172.31.255.255", "192.168.0.1",
                  "127.0.0.1", "169.254.1.1", "100.64.0.1", "100.127.255.255",
                  "::1", "fc00::1", "fe80::1", "localhost"] {
            assert!(is_private(h), "{h} should be private");
        }
        for h in ["8.8.8.8", "203.0.113.7", "172.32.0.1", "100.128.0.1", "2001:db8::1"] {
            assert!(!is_private(h), "{h} should be public");
        }
    }

    #[test]
    fn reranking_is_stable_so_unknown_classes_keep_their_heuristic_order() {
        // "Addresses whose class the learner has no data for keep their
        // original heuristic position." An unstable sort would discard the
        // ordering the learner is supposed to be REFINING, so a peer with
        // partial data would route worse than one with none.
        let mut a = vec![addr("a-pub1"), addr("b-lan"), addr("c-pub2")];
        let cls = |p: &PeerAddress| if p.host.contains("lan") { AddrClass::SameLan }
                                    else { AddrClass::Public };
        // learner only has an opinion about SameLan
        rerank(&mut a, cls, &[AddrClass::SameLan]);
        assert_eq!(a[0].host, "b-lan");
        assert_eq!(a[1].host, "a-pub1", "unknown class lost its heuristic order");
        assert_eq!(a[2].host, "c-pub2");
    }

    #[test]
    fn an_empty_ranking_preserves_the_ab_switch() {
        // With tuning off the learner returns nothing, and this must not
        // reorder — otherwise the A and B arms differ by more than the flag.
        let original = vec![addr("x"), addr("y"), addr("z")];
        let mut a = original.clone();
        rerank(&mut a, |_| AddrClass::Public, &[]);
        assert_eq!(a, original);
    }

    #[test]
    fn the_same_rerank_applies_to_both_lookup_paths() {
        // The stated bug: only the cache-hit path was reranked, so "the first
        // lookup-after-cache-miss got worse routing decisions than the second"
        // — cold start worse than steady state, the opposite of the intent.
        // One function, so both call sites get identical ordering.
        let cls = |p: &PeerAddress| if p.host.starts_with("lan") { AddrClass::SameLan }
                                    else { AddrClass::Public };
        let ranking = [AddrClass::SameLan, AddrClass::Public];
        let mut from_cache = vec![addr("pub1"), addr("lan1")];
        let mut from_dht = vec![addr("pub1"), addr("lan1")];
        rerank(&mut from_cache, cls, &ranking);
        rerank(&mut from_dht, cls, &ranking);
        assert_eq!(from_cache, from_dht);
        assert_eq!(from_cache[0].host, "lan1");
    }

    #[test]
    fn a_class_the_learner_ranks_last_still_beats_one_it_does_not_know() {
        // Unknown sorts after everything ranked, or a class with bad measured
        // performance would be tried after one with no data at all.
        let mut a = vec![addr("unknown"), addr("ranked")];
        let cls = |p: &PeerAddress| if p.host == "ranked" { AddrClass::Public }
                                    else { AddrClass::SameLan };
        rerank(&mut a, cls, &[AddrClass::Public]);
        assert_eq!(a[0].host, "ranked");
    }

    #[test]
    fn class_of_puts_same_lan_ahead_of_merely_private() {
        // A private address on ANOTHER subnet is not reachable the way one on
        // ours is, so they are different classes rather than one bucket.
        assert_eq!(class_of("192.168.1.5", true), AddrClass::SameLan);
        assert_eq!(class_of("192.168.9.5", false), AddrClass::Private);
        assert_eq!(class_of("8.8.8.8", false), AddrClass::Public);
    }

    #[test]
    fn relookups_are_bounded() {
        // An unbounded relookup loop on an unreachable peer is a phone radio
        // held awake by a contact who is simply offline.
        assert!(may_relookup(0));
        assert!(may_relookup(MAX_RELOOKUPS - 1));
        assert!(!may_relookup(MAX_RELOOKUPS));
    }
}
