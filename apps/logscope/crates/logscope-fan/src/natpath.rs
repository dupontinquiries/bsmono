//! natpath.rs — deciding how two trusted peers reach each other through NAT, before trying.
//!
//! ## What is already built, and what is genuinely missing
//!
//! `shared/share-kit`'s `Mailbox` posts to a `Backend` that is `put(key, value, ttl)` / `get(key)` —
//! which is exactly a DHT's interface, and exactly the signalling channel hole punching needs. Peers
//! post their candidate addresses; the other side fetches them. That substrate exists.
//!
//! What does not exist is the **traversal**: discovering your own reflexive address, punching, and
//! knowing when punching cannot work. That is the piece worth writing in Rust, and it is a few hundred
//! lines — not the 29,346 of OpenDHT, whose routing layer solves a problem `trustkit` already removed.
//!
//! ## Why classifying BEFORE trying matters
//!
//! Hole punching between two endpoint-independent NATs succeeds routinely. Between two **symmetric**
//! NATs it essentially never does, because each side's external port changes per destination, so the
//! address you signalled is not the address your peer must reach.
//!
//! Attempting anyway costs seconds of timeout per candidate pair and then fails — during which a user
//! is watching nothing happen. Classifying first turns that into an immediate relay, which is slower
//! per byte and *far* faster to first byte.
//!
//! **CGNAT is the case that motivated this**, and it is worse than ordinary NAT in a specific way: you
//! cannot port-forward your way out, because the mapping is on the carrier's equipment. So "open a port
//! on the router" — the usual advice — is not available, and a relay is not a fallback but the answer.

/// How a NAT rewrites source ports, which is what decides whether punching can work.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mapping {
    /// No NAT, or a forwarded port. Reachable directly.
    Open,
    /// Same external port regardless of destination. Punching works: the address you signal is the
    /// address your peer reaches.
    EndpointIndependent,
    /// External port varies per destination. The signalled address is stale before it is read.
    Symmetric,
    /// Carrier-grade NAT: endpoint-independent or not, you cannot forward a port because the mapping
    /// is not on equipment you control.
    Cgnat { symmetric: bool },
    /// Never measured.
    Unknown,
}

impl Mapping {
    /// Can a signalled address be reached from outside?
    pub fn punchable(self) -> bool {
        match self {
            Mapping::Open | Mapping::EndpointIndependent => true,
            Mapping::Cgnat { symmetric } => !symmetric,
            Mapping::Symmetric => false,
            // Unmeasured is treated as punchable so a first attempt happens and MEASURES something.
            // Assuming the worst would send every unknown peer to a relay and never learn otherwise.
            Mapping::Unknown => true,
        }
    }
}

/// What to do for a given pair.
#[derive(Clone, PartialEq, Debug)]
pub enum Strategy {
    /// One side is directly reachable. No punching needed.
    Direct { to: Mapping },
    /// Both sides punch simultaneously.
    Punch,
    /// Punching cannot work for this pair. Go straight to a relay.
    RelayOnly { why: String },
}

impl Strategy {
    pub fn needs_relay(&self) -> bool { matches!(self, Strategy::RelayOnly { .. }) }
}

/// Choose a strategy from both sides' measured mappings.
///
/// `Direct` is checked first: if either side is openly reachable, punching is wasted work and a relay
/// is wasted bandwidth.
pub fn strategy(local: Mapping, remote: Mapping) -> Strategy {
    if remote == Mapping::Open { return Strategy::Direct { to: remote }; }
    if local == Mapping::Open { return Strategy::Direct { to: local }; }
    if local.punchable() || remote.punchable() {
        // One punchable side is enough: it holds a stable port the other can aim at.
        return Strategy::Punch;
    }
    Strategy::RelayOnly { why: format!(
        "both sides are symmetric ({local:?} / {remote:?}), so each external port changes per \
destination and the address either side signals is not the one the other must reach. Punching would \
time out on every candidate pair before failing") }
}

/// A candidate address, posted to the mailbox for the other side to fetch.
#[derive(Clone, PartialEq, Debug)]
pub struct Candidate { pub addr: String, pub kind: CandKind }

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum CandKind {
    /// An address on this machine's own interface.
    Host,
    /// This machine's address as a third party observed it — what NAT rewrote it to.
    Reflexive,
    /// An address on a relay that forwards to this machine.
    Relayed,
}

/// Order candidate pairs by how cheap they are to try.
///
/// Host pairs first because two peers on the same LAN should never touch a relay, and that case is
/// common enough — a phone and a desktop at home — that trying reflexive first would add a round trip
/// to the situation that needs none.
pub fn ordered_pairs(local: &[Candidate], remote: &[Candidate]) -> Vec<(usize, usize)> {
    let mut v: Vec<(usize, usize)> = Vec::new();
    for (i, a) in local.iter().enumerate() {
        for (j, b) in remote.iter().enumerate() { v.push((i, j)); }
        let _ = a;
    }
    v.sort_by_key(|(i, j)| (local[*i].kind.max(remote[*j].kind), local[*i].kind, remote[*j].kind));
    v
}

/// Seconds to wait on a punch before falling back.
///
/// Short on purpose. A punch that has not succeeded in this long is very unlikely to, and the user is
/// watching nothing happen — a relay that connects in a second beats a direct path that connects in
/// fifteen.
pub const PUNCH_TIMEOUT_S: u64 = 3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_symmetric_nats_skip_punching_entirely() {
        // The case worth classifying before trying: each side's external port changes per destination,
        // so the address either signals is not the one the other must reach. Attempting costs seconds
        // of timeout per candidate pair while a user watches nothing happen.
        let s = strategy(Mapping::Symmetric, Mapping::Symmetric);
        assert!(s.needs_relay());
        if let Strategy::RelayOnly { why } = s {
            assert!(why.contains("changes per destination"));
            assert!(why.contains("time out on every candidate pair"));
        }
    }

    #[test]
    fn one_punchable_side_is_enough() {
        // It holds a stable port the other can aim at, so the pair does not need both.
        assert_eq!(strategy(Mapping::Symmetric, Mapping::EndpointIndependent), Strategy::Punch);
        assert_eq!(strategy(Mapping::EndpointIndependent, Mapping::Symmetric), Strategy::Punch);
    }

    #[test]
    fn an_openly_reachable_peer_is_used_DIRECTLY() {
        // Punching an open peer is wasted work and relaying to it is wasted bandwidth.
        assert!(matches!(strategy(Mapping::Symmetric, Mapping::Open), Strategy::Direct { .. }));
        assert!(matches!(strategy(Mapping::Open, Mapping::Symmetric), Strategy::Direct { .. }));
    }

    #[test]
    fn cgnat_is_punchable_only_when_it_is_not_symmetric() {
        // CGNAT's distinguishing property is that you cannot port-forward out of it — the mapping is on
        // the carrier's equipment. That does not by itself prevent punching; being symmetric does.
        assert!(Mapping::Cgnat { symmetric: false }.punchable());
        assert!(!Mapping::Cgnat { symmetric: true }.punchable());
        assert!(strategy(Mapping::Cgnat { symmetric: true }, Mapping::Symmetric).needs_relay());
    }

    #[test]
    fn an_unmeasured_mapping_is_TRIED_rather_than_relayed() {
        // Assuming the worst would send every unknown peer to a relay and never learn otherwise — the
        // first attempt is also the measurement, so refusing to attempt means never classifying.
        assert!(Mapping::Unknown.punchable());
        assert_eq!(strategy(Mapping::Unknown, Mapping::Unknown), Strategy::Punch);
    }

    #[test]
    fn host_pairs_are_tried_before_anything_touches_a_relay() {
        // A phone and a desktop at home should never relay, and that case is common enough that trying
        // reflexive first would add a round trip to the situation needing none.
        let l = vec![Candidate { addr: "10.0.0.2".into(), kind: CandKind::Host },
                     Candidate { addr: "203.0.113.1".into(), kind: CandKind::Reflexive }];
        let r = vec![Candidate { addr: "relay:443".into(), kind: CandKind::Relayed },
                     Candidate { addr: "10.0.0.5".into(), kind: CandKind::Host }];
        let first = ordered_pairs(&l, &r)[0];
        assert_eq!((l[first.0].kind, r[first.1].kind), (CandKind::Host, CandKind::Host));
        let last = *ordered_pairs(&l, &r).last().unwrap();
        assert_eq!(r[last.1].kind, CandKind::Relayed, "relay pairs sort last");
    }

    #[test]
    fn the_punch_timeout_is_short_because_a_user_is_waiting() {
        // A punch that has not succeeded in a few seconds very likely will not, and a relay connecting
        // in one second beats a direct path connecting in fifteen.
        assert!(PUNCH_TIMEOUT_S <= 5);
    }
}
