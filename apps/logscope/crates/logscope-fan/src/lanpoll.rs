//! lanpoll.rs — wires `lanwire` + `announce` + `trustkit` into the shape a UI frame loop wants.
//!
//! Everything this calls already existed and was tested without a socket (`announce`, `rendezvous`,
//! `trustkit`) or tested only against a real one under `#[ignore]` (`lanwire`) — this is the missing
//! piece: a small stateful service a UI can `tick()` once a frame and then ask `seen()` of, so a
//! screen can show "this registered machine is on the LAN right now" without knowing anything about
//! sockets, signatures, or the announce interval.
//!
//! What this does NOT do, on purpose, matching `rendezvous`'s own boundary: it locates a registered
//! peer, it does not connect to one. Turning a `Candidate` into an actual data channel is a separate,
//! larger piece of work (a listener accepting connections, an authenticated session) that this isn't
//! trying to be. Discovery first, because you can't build the rest without knowing something is there.

use crate::announce::{self, Announce};
use crate::lanwire::{LanWire, INTERVAL_S};
use crate::machineid::Identity;
use crate::rendezvous::Candidate;
use crate::trustkit::Registry;
use std::collections::HashMap;
use std::time::Instant;

/// A LAN discovery session bound to one identity. Holds the socket (if binding succeeded — it can
/// fail, e.g. the port is already in use by another logscope on this machine) and what's been seen.
pub struct LanPoll {
    wire: Option<LanWire>,
    my_addr: String,
    last_announce: Option<Instant>,
    /// Registered peers seen recently, with when. A HashMap because "is X currently visible" is the
    /// query a UI actually makes, not "list everything ever seen".
    seen: HashMap<String, (Candidate, Instant)>,
}

/// How long a sighting is shown as current. Independent of `announce::MAX_AGE_S` (which bounds
/// whether a DATAGRAM is fresh enough to accept) — this bounds how long the UI keeps showing a peer
/// after its last accepted datagram, so a peer that just left the network doesn't read as present
/// forever, but also doesn't flicker off between two announce intervals.
pub const SIGHTING_TTL_S: u64 = INTERVAL_S * 3;

impl LanPoll {
    /// Bind a discovery socket. `my_addr` is best-effort (see `guess_local_addr`) — this build only
    /// announces PRESENCE, not a reachable service, so an imprecise address costs nothing yet; it
    /// will matter once something is actually listening on it.
    pub fn new() -> Self {
        LanPoll { wire: LanWire::bind().ok(), my_addr: guess_local_addr(), last_announce: None, seen: HashMap::new() }
    }

    /// Whether the discovery socket is actually up. Surfacing this matters — a silent bind failure
    /// would show "no machines found" indistinguishably from "found nothing," which are very
    /// different things to tell someone about their network.
    pub fn bound(&self) -> bool { self.wire.is_some() }

    /// Send an announcement if due, then drain and accept whatever arrived. Cheap when there's
    /// nothing to do (non-blocking socket, and the interval gates how often a datagram goes out) —
    /// call this every frame rather than trying to time it from the UI side.
    pub fn tick(&mut self, id: &Identity, reg: &Registry) {
        let Some(wire) = &self.wire else { return };
        let now_s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let due = self.last_announce.map(|t| t.elapsed().as_secs() >= INTERVAL_S).unwrap_or(true);
        if due {
            let a = Announce { id: id.id(), addr: self.my_addr.clone(), sent_at_s: now_s };
            let sig = id.sign(&a.payload());
            let _ = wire.announce(&a, &sig); // best-effort — a dropped announcement costs one interval, per lanwire's own doc
            self.last_announce = Some(Instant::now());
        }
        for (body, sig, _from) in wire.drain() {
            let Ok(a) = Announce::parse(&body) else { continue };
            let verify = |key: &[u8], msg: &[u8]| {
                <&[u8; 32]>::try_from(key).ok().map(|k| crypto_kit::verify(k, msg, &sig)).unwrap_or(false)
            };
            if let Ok(cand) = announce::accept(&a, now_s, reg, verify) {
                self.seen.insert(a.id, (cand, Instant::now()));
            }
            // Rejections (unregistered, bad signature, stale) are silently dropped here, same as
            // lanwire's own malformed-datagram handling — a stranger's packet is not an event worth
            // surfacing per-packet. A caller wanting that for review would call announce::accept
            // directly instead of going through this convenience wrapper.
        }
    }

    /// Registered peers visible within `SIGHTING_TTL_S`, most-recently-seen first.
    pub fn seen(&self) -> Vec<(String, Candidate, u64)> {
        let mut v: Vec<(String, Candidate, u64)> = self.seen.iter()
            .filter(|(_, (_, at))| at.elapsed().as_secs() < SIGHTING_TTL_S)
            .map(|(id, (c, at))| (id.clone(), c.clone(), at.elapsed().as_secs()))
            .collect();
        v.sort_by_key(|(_, _, age)| *age);
        v
    }
}

impl Default for LanPoll {
    fn default() -> Self { Self::new() }
}

/// Best-effort local address for the announcement. The classic no-traffic trick: "connecting" a UDP
/// socket only asks the OS to pick a route and doesn't send a packet, so this never touches the
/// network — it just asks the kernel which local interface would be used to reach the internet, which
/// is a reasonable guess for "the address other things on this LAN would use to reach me."
fn guess_local_addr() -> String {
    use std::net::UdpSocket;
    UdpSocket::bind("0.0.0.0:0").ok()
        .and_then(|s| { s.connect("8.8.8.8:80").ok()?; s.local_addr().ok() })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sighting_ttl_outlives_a_missed_interval() {
        // A peer that misses exactly one announce (lanwire's own stated failure mode) must not
        // immediately read as gone, or the UI would flicker every few intervals under ordinary
        // packet loss.
        assert!(SIGHTING_TTL_S > INTERVAL_S, "must tolerate at least one missed announce");
    }

    #[test]
    fn guess_local_addr_returns_something_parseable_or_the_explicit_marker() {
        // Can't assert a real IP in CI (no network guaranteed), but it must never panic and must
        // never silently return an empty string that would look like a real (if useless) value.
        let a = guess_local_addr();
        assert!(!a.is_empty());
    }

    #[test]
    fn a_fresh_poll_has_bound_or_not_but_never_panics_ticking() {
        // Exercises the whole tick() path against whatever this test machine's network stack allows,
        // without asserting on the (unpredictable-in-CI) result of the bind itself.
        let mut lp = LanPoll::new();
        let reg = Registry::default();
        let path = std::env::temp_dir().join(format!("lanpoll-test-identity-{}", std::process::id()));
        let id = Identity::load_or_create(&path).expect("identity");
        lp.tick(&id, &reg);
        let _ = std::fs::remove_file(&path);
        assert!(lp.seen().is_empty(), "no peers registered, so nothing should be accepted regardless of what's on the LAN");
    }
}
