//! rendezvous.rs — find machines you already registered. Not a DHT, and here is why.
//!
//! ## What OpenDHT costs, measured from its own tree
//!
//! ```text
//!   29,346 lines of C++ (src + include)
//!   native deps: fmt, nettle, gnutls, msgpack, jsoncpp, asio, argon2, simdutf
//!   the bundled rust/ crate is FFI BINDINGS, not a port — it links libopendht
//! ```
//!
//! The bindings do not remove the dependency, they wrap it. And the hard part is cross-compiling
//! GnuTLS and nettle for Android, which is precisely the case that motivated this — a phone reaching
//! an M2 Ultra.
//!
//! ## What a DHT is FOR, and why we forbade it
//!
//! A DHT solves *find an arbitrary peer among millions with no server*. That is a real problem and it
//! is not ours. `trustkit` establishes that an unregistered machine gets **nothing** — not compute, not
//! build, not handoff. So the peer set is enumerated in advance, by a person, on purpose.
//!
//! **Discovering unknown peers is work whose output we would immediately discard.** The DHT's core
//! value is the thing the trust model already refuses.
//!
//! What remains is a much smaller problem: given a peer ID you already vouched for, find its current
//! address. That is rendezvous, not routing.
//!
//! ## The security property this preserves
//!
//! Discovery here can only ever RESOLVE a registered peer, never introduce one. A mechanism that could
//! add peers would reintroduce exactly what `trustkit` forbade — and on a café LAN, "a machine answered
//! the broadcast" is what an attacker also does.

/// How a peer's address was learned, in ascending cost.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Route {
    /// Same machine.
    Loopback,
    /// Found by multicast on the local network.
    Lan,
    /// An address that worked before, tried directly.
    LastKnown,
    /// Via a rendezvous host, because nothing cheaper reached it.
    Relay,
}

impl Route {
    /// Rough round trip, for ordering attempts and for `workerkit` duty placement.
    pub fn typical_rtt_ms(self) -> u32 {
        match self { Route::Loopback => 0, Route::Lan => 5, Route::LastKnown => 40, Route::Relay => 120 }
    }
}

/// One way to reach a peer.
#[derive(Clone, PartialEq, Debug)]
pub struct Candidate {
    pub addr: String,
    pub route: Route,
    /// Seconds since this address was last confirmed working.
    pub age_s: u64,
}

/// How long a learned address stays credible.
///
/// A laptop that changed networks has a last-known address that now belongs to someone else. Trying it
/// is not merely useless — it means opening a connection to an unknown host, so an expired candidate is
/// dropped rather than deprioritised.
pub const MAX_AGE_S: u64 = 900;

/// Resolve a REGISTERED peer to the cheapest working candidate.
///
/// Takes the registry so it cannot resolve anyone else. That is the whole security argument, expressed
/// as a parameter rather than a comment: a caller has no way to ask this about an unregistered id.
pub fn resolve<'a>(id: &str, candidates: &'a [Candidate],
                   reg: &crate::trustkit::Registry) -> Option<&'a Candidate> {
    if reg.level(id) == crate::trustkit::Trust::Unknown { return None; }
    candidates.iter()
        .filter(|c| c.age_s <= MAX_AGE_S)
        .min_by_key(|c| c.route)
}

/// What a discovery response may do.
///
/// Deliberately not "add a peer". A response can only confirm where a machine you already vouched for
/// currently is.
#[derive(Clone, PartialEq, Debug)]
pub enum Discovery {
    /// A registered peer announced itself. Update its address.
    Located { id: String, candidate: Candidate },
    /// Someone answered who is not registered. Recorded for a person to review, never acted on.
    Unregistered { id: String, addr: String },
}

/// Classify a discovery response.
pub fn classify(id: &str, addr: &str, route: Route, reg: &crate::trustkit::Registry) -> Discovery {
    if reg.level(id) == crate::trustkit::Trust::Unknown {
        return Discovery::Unregistered { id: id.into(), addr: addr.into() };
    }
    Discovery::Located { id: id.into(), candidate: Candidate { addr: addr.into(), route, age_s: 0 } }
}

impl Discovery {
    /// One line for the operator. An unregistered responder is worth SHOWING — it is how you notice a
    /// machine you meant to register, and equally how you notice one you did not.
    pub fn note(&self) -> String {
        match self {
            Discovery::Located { id, candidate } =>
                format!("{id} at {} via {:?}", candidate.addr, candidate.route),
            Discovery::Unregistered { id, addr } =>
                format!("{id} answered from {addr} and is NOT registered — nothing was sent to it. \
Register it explicitly if it is yours"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trustkit::{Registry, Trust};

    fn c(addr: &str, route: Route, age: u64) -> Candidate {
        Candidate { addr: addr.into(), route, age_s: age }
    }
    fn reg() -> Registry {
        let mut r = Registry::new();
        r.register("m2", Trust::Handoff);
        r
    }

    #[test]
    fn an_unregistered_peer_cannot_be_resolved_AT_ALL() {
        // The security argument as a parameter rather than a comment: a caller has no way to ask this
        // about an id nobody vouched for, so discovery cannot introduce a peer.
        let cands = vec![c("10.0.0.5:7000", Route::Lan, 0)];
        assert!(resolve("stranger", &cands, &reg()).is_none());
        assert!(resolve("m2", &cands, &reg()).is_some());
    }

    #[test]
    fn the_cheapest_route_wins() {
        // LAN before last-known before relay. Ordering by Route rather than by measured latency because
        // an unmeasured relay would otherwise sort ahead of a LAN address on missing data.
        let cands = vec![
            c("relay.example:443", Route::Relay, 0),
            c("10.0.0.5:7000", Route::Lan, 0),
            c("203.0.113.9:7000", Route::LastKnown, 0),
        ];
        assert_eq!(resolve("m2", &cands, &reg()).unwrap().route, Route::Lan);
    }

    #[test]
    fn a_stale_address_is_DROPPED_not_merely_deprioritised() {
        // A laptop that changed networks has a last-known address that now belongs to someone else.
        // Trying it means opening a connection to an unknown host, which is worse than failing to
        // connect at all.
        let cands = vec![c("203.0.113.9:7000", Route::LastKnown, MAX_AGE_S + 1)];
        assert!(resolve("m2", &cands, &reg()).is_none());
    }

    #[test]
    fn a_stale_cheap_route_loses_to_a_fresh_expensive_one() {
        let cands = vec![
            c("10.0.0.5:7000", Route::Lan, MAX_AGE_S + 1),
            c("relay.example:443", Route::Relay, 10),
        ];
        assert_eq!(resolve("m2", &cands, &reg()).unwrap().route, Route::Relay);
    }

    #[test]
    fn an_unregistered_responder_is_SHOWN_but_never_acted_on() {
        // How you notice a machine you meant to register — and equally how you notice one you did not.
        // On a cafe LAN, "a machine answered the broadcast" is what an attacker also does.
        let d = classify("laptop", "10.0.0.9:7000", Route::Lan, &reg());
        assert!(matches!(d, Discovery::Unregistered { .. }));
        assert!(d.note().contains("nothing was sent to it"));
        assert!(d.note().contains("Register it explicitly"));
    }

    #[test]
    fn a_registered_responder_updates_its_address_at_age_zero() {
        let d = classify("m2", "10.0.0.5:7000", Route::Lan, &reg());
        match d {
            Discovery::Located { candidate, .. } => {
                assert_eq!(candidate.age_s, 0, "just confirmed");
                assert_eq!(candidate.route, Route::Lan);
            }
            other => panic!("expected Located, got {other:?}"),
        }
    }

    #[test]
    fn route_rtts_match_what_workerkit_uses_to_place_duties() {
        // A LAN route must land in workerkit's Lan band and a relay must not, or a speculative draft
        // could be placed across a relay — the one placement that cannot work.
        assert!(Route::Lan.typical_rtt_ms() <= 5, "colocated-or-lan");
        assert!(Route::Relay.typical_rtt_ms() > 60, "relay must fall outside the Lan band");
        assert!(Route::Loopback < Route::Lan && Route::Lan < Route::Relay);
    }
}
