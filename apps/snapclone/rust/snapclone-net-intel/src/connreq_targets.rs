//! CONNREQ target selection — which of a peer's advertised addresses to try,
//! in what order, and which to skip.
//!
//! This pins the exact decision behind a real snap-delivery failure: two phones
//! on the same WiFi each only had the other's PUBLIC IP cached, which is a
//! hairpin (their shared router's external address). The receiver correctly
//! skips the hairpin — but with no LAN address there was then nothing left to
//! try, so the snap stayed queued forever. The fix added the sender's LAN
//! address to the CONNREQ; this module models how the receiver should rank the
//! resulting candidate set so the same-WiFi direct path is preferred and the
//! hairpin is dropped.
//!
//! Pure logic over already-parsed addresses; the C++ does the socket work.

/// A candidate address from a CONNREQ (or DHT cache), tagged with how it relates
/// to us so the ranker can order/skip correctly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub host: String,
    pub port: u16,
    pub kind: CandKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandKind {
    /// An RFC1918 LAN address (192.168/10/172.16).
    Lan,
    /// A routable public address.
    Public,
}

/// Build the ordered list of addresses to actually attempt, given the peer's
/// candidates and OUR own public IP. Rules:
///   1. Drop any public candidate whose host == our public IP — that's a
///      hairpin (we'd be connecting to our own router's external face).
///   2. Prefer LAN candidates first (fastest + works under AP isolation-free
///      same-subnet), then remaining public candidates.
///   3. De-duplicate identical host:port.
pub fn order_targets(candidates: &[Candidate], self_public_ip: &str) -> Vec<Candidate> {
    let mut lan = Vec::new();
    let mut public = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for c in candidates {
        let key = (c.host.clone(), c.port);
        if !seen.insert(key) {
            continue; // dedupe
        }
        match c.kind {
            CandKind::Lan => lan.push(c.clone()),
            CandKind::Public => {
                if c.host == self_public_ip {
                    continue; // hairpin — skip
                }
                public.push(c.clone());
            }
        }
    }
    lan.into_iter().chain(public).collect()
}

/// True if, after ordering, there is nothing to try — the condition that left
/// snaps stuck. Useful for the caller to decide whether to fall straight to
/// relay/rendezvous instead of reporting a dead end.
pub fn is_dead_end(candidates: &[Candidate], self_public_ip: &str) -> bool {
    order_targets(candidates, self_public_ip).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lan(h: &str) -> Candidate { Candidate { host: h.into(), port: 9876, kind: CandKind::Lan } }
    fn pubc(h: &str) -> Candidate { Candidate { host: h.into(), port: 9876, kind: CandKind::Public } }

    #[test]
    fn the_bug_only_hairpin_public_is_dead_end() {
        // Exactly the failing case: the only address is the shared public IP.
        let cands = vec![pubc("69.156.66.68")];
        assert!(is_dead_end(&cands, "69.156.66.68"));
        assert!(order_targets(&cands, "69.156.66.68").is_empty());
    }

    #[test]
    fn the_fix_lan_address_rescues_delivery() {
        // With the LAN addr added (the fix), there's a real target and the
        // hairpin is dropped.
        let cands = vec![pubc("69.156.66.68"), lan("192.168.2.223")];
        let order = order_targets(&cands, "69.156.66.68");
        assert_eq!(order.len(), 1);
        assert_eq!(order[0].host, "192.168.2.223");
        assert!(!is_dead_end(&cands, "69.156.66.68"));
    }

    #[test]
    fn lan_preferred_over_nonhairpin_public() {
        // Different public IP (not a hairpin) is kept, but LAN still goes first.
        let cands = vec![pubc("203.0.113.7"), lan("192.168.2.223")];
        let order = order_targets(&cands, "69.156.66.68");
        assert_eq!(order.len(), 2);
        assert_eq!(order[0].kind, CandKind::Lan);
        assert_eq!(order[1].host, "203.0.113.7");
    }

    #[test]
    fn nonhairpin_public_kept_when_no_lan() {
        // Cross-network peer: their public IP isn't ours, so it's a valid target.
        let cands = vec![pubc("203.0.113.7")];
        let order = order_targets(&cands, "69.156.66.68");
        assert_eq!(order.len(), 1);
        assert_eq!(order[0].host, "203.0.113.7");
    }

    #[test]
    fn dedupes_identical_addrs() {
        let cands = vec![lan("192.168.2.223"), lan("192.168.2.223")];
        assert_eq!(order_targets(&cands, "x").len(), 1);
    }

    #[test]
    fn multiple_lan_all_kept_in_order() {
        let cands = vec![lan("192.168.2.223"), lan("10.0.0.5")];
        let order = order_targets(&cands, "69.156.66.68");
        assert_eq!(order.len(), 2);
        assert_eq!(order[0].host, "192.168.2.223");
        assert_eq!(order[1].host, "10.0.0.5");
    }
}
