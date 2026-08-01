//! reflexive.rs — learning your own public address, which is what makes the away-from-home case work.
//!
//! ## Why this specific piece
//!
//! On a LAN, peers find each other by announcing on a multicast address they share. Away from home
//! there is no shared network: the phone is on a carrier, the M2 is behind a home router, and neither
//! can name an address the other can reach. Each must learn what the world sees it as — its
//! **reflexive** address — and post that to the mailbox for the other to fetch.
//!
//! That is one request to a host that echoes back the source address it observed. STUN's core is
//! exactly this and nothing more; the rest of STUN is authentication and relaying we do not need,
//! because `peerauth` already proves identity and the mailbox already carries the signalling.
//!
//! ## What the replies tell you beyond the address
//!
//! Asking TWO different hosts is what classifies the NAT, and the classification is what `natpath`
//! needs to decide whether to punch or relay:
//!
//! * same external port from both → **endpoint-independent**, punchable;
//! * different port per destination → **symmetric**, and punching cannot work;
//! * the observed address equals a local one → **no NAT**, directly reachable.
//!
//! One probe cannot distinguish these, which is the mistake worth avoiding: a single reply looks like
//! success and tells you nothing about whether the address is usable by anyone else.

/// One observation of how a host saw us.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Observation {
    /// Which host reported it — two different ones are required to classify.
    pub from: String,
    pub addr: String,
    pub port: u16,
}

/// Classify from observations plus what we know locally.
///
/// Returns `Unknown` on a single observation rather than guessing. A guess here propagates into
/// `natpath`, which would then choose a strategy on no evidence — and the failure would look like a
/// flaky connection rather than a bad classification.
pub fn classify(obs: &[Observation], local_addrs: &[String]) -> crate::natpath::Mapping {
    if obs.is_empty() { return crate::natpath::Mapping::Unknown; }
    // No NAT: the world sees an address we hold ourselves.
    if obs.iter().any(|o| local_addrs.iter().any(|l| l == &o.addr)) {
        return crate::natpath::Mapping::Open;
    }
    if obs.len() < 2 { return crate::natpath::Mapping::Unknown; }
    let ports_differ = obs.windows(2).any(|w| w[0].port != w[1].port);
    if ports_differ { crate::natpath::Mapping::Symmetric }
    else { crate::natpath::Mapping::EndpointIndependent }
}

/// Is this address in carrier-grade NAT space?
///
/// RFC 6598 reserves 100.64.0.0/10 for CGNAT. Seeing our REFLEXIVE address in that range means the
/// carrier is translating and no port forward on our own router can help — which changes the advice
/// from "open a port" to "use a relay", and that is the difference worth detecting.
pub fn is_cgnat(addr: &str) -> bool {
    let mut p = addr.split('.');
    match (p.next().and_then(|a| a.parse::<u8>().ok()),
           p.next().and_then(|b| b.parse::<u8>().ok())) {
        (Some(100), Some(b)) => (64..=127).contains(&b),
        _ => false,
    }
}

/// Refine a classification with the CGNAT check.
pub fn refine(m: crate::natpath::Mapping, reflexive_addr: &str) -> crate::natpath::Mapping {
    use crate::natpath::Mapping::*;
    if !is_cgnat(reflexive_addr) { return m; }
    match m {
        Symmetric => Cgnat { symmetric: true },
        EndpointIndependent | Open => Cgnat { symmetric: false },
        other => other,
    }
}

/// How many probes are needed before a classification means anything.
pub const MIN_PROBES: usize = 2;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::natpath::Mapping;

    fn ob(from: &str, addr: &str, port: u16) -> Observation {
        Observation { from: from.into(), addr: addr.into(), port }
    }

    #[test]
    fn ONE_probe_cannot_classify_and_says_so() {
        // The mistake worth avoiding: a single reply looks like success and tells you nothing about
        // whether the address is usable by anyone else. A guess here propagates into natpath, which
        // would choose a strategy on no evidence — and the failure would look like a flaky connection.
        let one = vec![ob("stun-a", "203.0.113.7", 51000)];
        assert_eq!(classify(&one, &["10.0.0.5".into()]), Mapping::Unknown);
        assert_eq!(MIN_PROBES, 2);
    }

    #[test]
    fn the_same_port_from_two_hosts_is_endpoint_independent() {
        // The punchable case: the address we signal is the address our peer reaches.
        let o = vec![ob("a", "203.0.113.7", 51000), ob("b", "203.0.113.7", 51000)];
        assert_eq!(classify(&o, &["10.0.0.5".into()]), Mapping::EndpointIndependent);
    }

    #[test]
    fn a_DIFFERENT_port_per_host_is_symmetric() {
        // Each destination gets a fresh external port, so the address we post is stale before it is
        // read. This is the case natpath sends straight to a relay.
        let o = vec![ob("a", "203.0.113.7", 51000), ob("b", "203.0.113.7", 51001)];
        assert_eq!(classify(&o, &["10.0.0.5".into()]), Mapping::Symmetric);
    }

    #[test]
    fn seeing_our_OWN_address_means_no_NAT() {
        // Directly reachable, so neither punching nor relaying is needed — and this is detectable from
        // a single observation, which is why the local-address check runs before the probe count.
        let o = vec![ob("a", "203.0.113.7", 51000)];
        assert_eq!(classify(&o, &["203.0.113.7".into()]), Mapping::Open);
    }

    #[test]
    fn a_reflexive_address_in_100_64_slash_10_is_CGNAT() {
        // RFC 6598. Seeing this means the carrier is translating and no port forward on our own router
        // can help — which changes the advice from "open a port" to "use a relay".
        assert!(is_cgnat("100.64.0.1"));
        assert!(is_cgnat("100.127.255.254"));
        assert!(!is_cgnat("100.63.0.1"), "just below the range");
        assert!(!is_cgnat("100.128.0.1"), "just above it");
        assert!(!is_cgnat("203.0.113.7"));
        assert!(!is_cgnat("10.0.0.1"), "private, but not carrier-grade");
    }

    #[test]
    fn cgnat_refinement_PRESERVES_whether_it_is_symmetric() {
        // The distinction natpath depends on: CGNAT alone does not prevent punching, being symmetric
        // does. Collapsing them would relay a punchable peer for nothing.
        assert_eq!(refine(Mapping::Symmetric, "100.64.1.1"), Mapping::Cgnat { symmetric: true });
        assert_eq!(refine(Mapping::EndpointIndependent, "100.64.1.1"),
                   Mapping::Cgnat { symmetric: false });
        assert_eq!(refine(Mapping::Symmetric, "203.0.113.7"), Mapping::Symmetric,
                   "not CGNAT: unchanged");
    }

    #[test]
    fn an_unknown_classification_stays_unknown_after_refinement() {
        // Refining nothing must not manufacture a classification.
        assert_eq!(refine(Mapping::Unknown, "100.64.1.1"), Mapping::Unknown);
    }
}
