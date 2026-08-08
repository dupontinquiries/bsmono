//! reachability.rs — a peer you can find is not a peer you can reach.
//!
//! LAN discovery and a DHT answer different questions, and the difference is the whole reason this
//! module exists. On a LAN, finding a peer and reaching it are the same event: the broadcast that
//! found it came over the path you will use. **Across networks they are not.** A DHT tells you a
//! peer published a record; whether packets get there depends on NAT, firewalls and whether both
//! ends are behind symmetric translation that no amount of hole-punching will open.
//!
//! ## The failure this prevents
//!
//! Offering work to a peer you cannot send it to. The DHT lookup succeeds, the device appears in the
//! list, the scheduler assigns it a build — and the transfer times out sixty seconds later, by which
//! point the operator has been told a machine is available that is not.
//!
//! **Discovery is a claim; reachability is a measurement**, and the two must not share a field.

/// How a peer became known.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Discovery {
    /// Found by LAN broadcast. **Reachability is implied**: the packet that announced it came over
    /// the path you will use.
    LocalBroadcast,
    /// Found in the DHT. Says the peer published a record, which may be stale and says nothing about
    /// the path between you.
    Dht,
    /// Configured by hand. The operator asserts it exists; they may be wrong about right now.
    Manual,
}

impl Discovery {
    /// Does finding it this way tell you anything about reaching it?
    pub fn implies_reachable(self) -> bool { matches!(self, Discovery::LocalBroadcast) }
}

/// What a connection attempt found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Path {
    /// Direct connection worked.
    Direct,
    /// Hole-punched through NAT. Works, and is fragile — it dies when either NAT rebinds, so a long
    /// job on this path may lose its peer partway.
    Punched,
    /// Relayed. Works, and every byte costs the relay's bandwidth — which makes it fine for a build
    /// check and wrong for anything moving activations.
    Relayed,
    /// Nothing got through.
    Unreachable,
}

impl Path {
    /// Suitable for work that streams data continuously?
    pub fn carries_bulk(self) -> bool { matches!(self, Path::Direct) }

    pub fn explain(self) -> &'static str {
        match self {
            Path::Direct => "direct — the only path suitable for work that moves data continuously",
            Path::Punched =>
                "hole-punched through NAT. Works now and dies when either NAT rebinds, so a long job \
on this path may lose its peer partway through and report a failure that is about the network",
            Path::Relayed =>
                "relayed — every byte costs the relay's bandwidth, which is fine for a build check \
that ships source once and wrong for anything streaming",
            Path::Unreachable =>
                "no path. The DHT record exists and the packets do not arrive, which is the normal \
outcome between two symmetric NATs and not an error to retry indefinitely",
        }
    }
}

/// A peer, with discovery and reachability kept apart.
#[derive(Clone, PartialEq, Debug)]
pub struct Peer {
    pub id: String,
    pub how_found: Discovery,
    /// **None until something has actually connected.** Not defaulted to Unreachable, because "we
    /// have not tried" and "we tried and failed" produce different next actions.
    pub path: Option<Path>,
    /// Seconds since the record was published, for DHT peers. A record outlives the machine.
    pub record_age_s: f32,
}

/// A DHT record older than this describes a machine that may be long gone.
pub const RECORD_STALE_S: f32 = 300.0;

impl Peer {
    /// May work be offered to this peer?
    pub fn assignable(&self) -> Result<Path, String> {
        match self.path {
            Some(Path::Unreachable) => Err(format!("{}: {}", self.id, Path::Unreachable.explain())),
            Some(p) => {
                if self.how_found == Discovery::Dht && self.record_age_s > RECORD_STALE_S {
                    // A path that worked five minutes ago and a record that old together mean the
                    // peer may have moved. Worth saying rather than silently assigning.
                    return Err(format!("{}: reachable when last tried, but its DHT record is {:.0}s \
old and a record outlives the machine that published it — re-probe before assigning", self.id,
                        self.record_age_s));
                }
                Ok(p)
            }
            None if self.how_found.implies_reachable() => {
                // LAN broadcast: the announcement arrived over the path we will use, so discovery IS
                // the measurement here and requiring a separate probe would be ceremony.
                Ok(Path::Direct)
            }
            None => Err(format!(
                "{}: found via {:?} but never connected. Discovery is a CLAIM and reachability is a \
MEASUREMENT — assigning work now risks a sixty-second timeout after telling the operator a machine \
was available", self.id, self.how_found)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(how: Discovery, path: Option<Path>, age: f32) -> Peer {
        Peer { id: "peer1".into(), how_found: how, path, record_age_s: age }
    }

    #[test]
    fn a_LAN_peer_is_reachable_by_the_fact_of_being_found() {
        // The announcement arrived over the path we will use, so discovery IS the measurement and a
        // separate probe would be ceremony.
        assert_eq!(p(Discovery::LocalBroadcast, None, 0.0).assignable(), Ok(Path::Direct));
    }

    #[test]
    fn a_DHT_peer_that_was_never_probed_is_REFUSED() {
        // The failure this exists to prevent: the lookup succeeds, the device appears in the list,
        // the scheduler assigns it, and the transfer times out a minute later.
        let e = p(Discovery::Dht, None, 10.0).assignable().unwrap_err();
        assert!(e.contains("Discovery is a CLAIM and reachability is a MEASUREMENT"));
    }

    #[test]
    fn NEVER_TRIED_is_distinct_from_TRIED_AND_FAILED() {
        // Different next actions: one says probe, the other says stop retrying.
        let untried = p(Discovery::Dht, None, 10.0).assignable().unwrap_err();
        let failed = p(Discovery::Dht, Some(Path::Unreachable), 10.0).assignable().unwrap_err();
        assert!(untried.contains("never connected"));
        assert!(failed.contains("not an error to retry indefinitely"));
    }

    #[test]
    fn a_STALE_record_is_refused_even_though_a_path_once_worked() {
        // A record outlives the machine that published it, so "it worked" plus "the record is old"
        // together mean the peer may have moved.
        let e = p(Discovery::Dht, Some(Path::Direct), 600.0).assignable().unwrap_err();
        assert!(e.contains("a record outlives the machine"));
    }

    #[test]
    fn a_RELAYED_path_is_usable_and_not_for_bulk() {
        // Fine for a build check that ships source once; wrong for anything streaming, and the
        // difference is bandwidth someone else is paying for.
        assert_eq!(p(Discovery::Dht, Some(Path::Relayed), 10.0).assignable(), Ok(Path::Relayed));
        assert!(!Path::Relayed.carries_bulk());
        assert!(Path::Direct.carries_bulk());
    }

    #[test]
    fn a_PUNCHED_path_warns_that_a_long_job_may_lose_it() {
        // It dies when either NAT rebinds, and a failure then is about the network rather than the
        // work — which is what an operator needs told BEFORE the job, not after.
        assert!(Path::Punched.explain().contains("may lose its peer partway"));
        assert!(!Path::Punched.carries_bulk());
    }

    #[test]
    fn a_MANUAL_peer_still_needs_probing() {
        // The operator asserts it exists; they may be right and wrong about right now.
        assert!(p(Discovery::Manual, None, 0.0).assignable().is_err());
        assert!(!Discovery::Manual.implies_reachable());
    }
}
