//! guard — the hardening a public DHT needs and neither engine had.
//!
//! Audited both Kademlia implementations in this repo against what the
//! reference does operationally. Token handling, search termination, node
//! liveness and the replacement cache were all present in one or both. **Rate
//! limiting, subnet diversity and token rotation were in neither**, and each is
//! a way a public DHT gets damaged rather than merely slowed.
//!
//! No dependencies, no clock, no sockets — the caller supplies `now_ms`, so the
//! same policy runs under a real transport and under `sim`.

use crate::routing::NodeId;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

// ── token rotation ──────────────────────────────────────────────────────────

/// How long a token stays valid before the secret rotates.
///
/// Five minutes, matching the reference's cadence. Short enough that a captured
/// token stops working within a coffee break, long enough that a slow peer's
/// get→put round trip does not expire mid-flight.
pub const TOKEN_ROTATE_MS: u64 = 5 * 60 * 1000;

/// Write tokens that expire.
///
/// The bug this replaces: `snapclone-dht` derives its token secret once at
/// construction and never changes it, so **a token is valid for the lifetime of
/// the process**. Anyone who observes one can keep announcing to that node
/// forever, which is exactly what tokens exist to prevent — they are the proof
/// that a writer actually received a packet at the address it claims, and proof
/// that never expires is not proof of anything current.
///
/// Two secrets are live at once and both are accepted. With a single secret,
/// every rotation would reject tokens issued moments before it — a peer that did
/// `get` at 4:59 and `put` at 5:01 would fail for no reason it could see or fix.
#[derive(Clone, Debug)]
pub struct TokenRing {
    current: u64,
    previous: u64,
    rotated_ms: u64,
}

impl TokenRing {
    /// `seed` should be unpredictable to peers — a node id is fine as a starting
    /// point precisely because rotation makes the initial value short-lived.
    pub fn new(seed: u64, now_ms: u64) -> TokenRing {
        let c = mix(seed);
        TokenRing { current: c, previous: mix(c), rotated_ms: now_ms }
    }

    /// Rotate if due. Cheap and idempotent, so a caller can invoke it on every
    /// packet rather than owning a timer.
    pub fn tick(&mut self, now_ms: u64) {
        if now_ms.saturating_sub(self.rotated_ms) >= TOKEN_ROTATE_MS {
            self.previous = self.current;
            self.current = mix(self.current ^ now_ms);
            self.rotated_ms = now_ms;
        }
    }

    /// The token this node hands to `who` for `key`.
    ///
    /// Bound to both: a token is a claim about one address asking about one key,
    /// and one that travelled would let a peer write where it never read.
    pub fn issue(&self, who: &SocketAddr, key: &NodeId) -> u64 {
        derive(self.current, who, key)
    }

    /// Accept a token issued under either live secret.
    pub fn check(&self, who: &SocketAddr, key: &NodeId, token: u64) -> bool {
        token == derive(self.current, who, key) || token == derive(self.previous, who, key)
    }
}

fn mix(x: u64) -> u64 {
    let mut h = x ^ 0x9E37_79B9_7F4A_7C15;
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

fn derive(secret: u64, who: &SocketAddr, key: &NodeId) -> u64 {
    let mut h = secret;
    // Port is deliberately excluded: a NAT can rewrite it between the get and
    // the put, and binding to it would fail exactly the peers that need
    // hole-punching most.
    match who.ip() {
        IpAddr::V4(v4) => for b in v4.octets() { h = mix(h ^ b as u64); },
        IpAddr::V6(v6) => for b in v6.octets() { h = mix(h ^ b as u64); },
    }
    for b in key.0 { h = mix(h ^ b as u64); }
    h
}

// ── rate limiting ───────────────────────────────────────────────────────────

/// Requests one source may make per window before it is ignored.
pub const RATE_BURST: u32 = 60;
/// The window over which `RATE_BURST` applies.
pub const RATE_WINDOW_MS: u64 = 10_000;

/// Per-source request limiting.
///
/// Without it a single peer can saturate a node's CPU with `find` queries, each
/// of which is a routing-table scan. That is not a theoretical attack: it is
/// what a buggy peer in a retry loop does by accident, and the effect on
/// everyone else is identical.
///
/// Keyed by **IP, not by node id**, because a node id is self-asserted and an
/// attacker rolls a new one per packet. The IP is what the socket saw.
#[derive(Clone, Default, Debug)]
pub struct RateLimiter {
    seen: HashMap<IpAddr, (u32, u64)>,
}

impl RateLimiter {
    /// Should this request be served?
    pub fn allow(&mut self, who: &SocketAddr, now_ms: u64) -> bool {
        let e = self.seen.entry(who.ip()).or_insert((0, now_ms));
        if now_ms.saturating_sub(e.1) >= RATE_WINDOW_MS {
            *e = (0, now_ms);
        }
        e.0 += 1;
        e.0 <= RATE_BURST
    }

    /// Forget idle sources so the table cannot grow without bound — the limiter
    /// must not become the memory exhaustion it exists to prevent.
    pub fn gc(&mut self, now_ms: u64) {
        self.seen.retain(|_, (_, t)| now_ms.saturating_sub(*t) < RATE_WINDOW_MS * 6);
    }

    pub fn tracked(&self) -> usize { self.seen.len() }
}

// ── subnet diversity ────────────────────────────────────────────────────────

/// Most contacts one /24 (or /64 on v6) may hold in a bucket.
///
/// Two. A Kademlia bucket filled from one subnet is a bucket an attacker owns:
/// they answer every lookup that lands there and can eclipse a node from the
/// rest of the network. Capping by prefix costs a little routing efficiency on
/// networks that genuinely are one subnet — a LAN — and buys eclipse resistance
/// everywhere else.
pub const MAX_PER_SUBNET: usize = 2;

/// The /24 for v4 or /64 for v6, as a comparable key.
///
/// v6 uses /64 rather than /48 or /128 because /64 is the standard host
/// allocation: /128 would treat one machine's many addresses as many operators,
/// which is exactly the confusion an attacker exploits.
pub fn subnet_key(addr: &SocketAddr) -> u64 {
    match addr.ip() {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            u64::from_be_bytes([0, 0, 0, 0, 0, o[0], o[1], o[2]])
        }
        IpAddr::V6(v6) => {
            let o = v6.octets();
            u64::from_be_bytes([o[0], o[1], o[2], o[3], o[4], o[5], o[6], o[7]])
        }
    }
}

/// May `candidate` join a bucket already holding `existing`?
pub fn subnet_allows(existing: &[SocketAddr], candidate: &SocketAddr) -> bool {
    let k = subnet_key(candidate);
    existing.iter().filter(|a| subnet_key(a) == k).count() < MAX_PER_SUBNET
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sa(s: &str) -> SocketAddr { s.parse().unwrap() }
    fn key(b: u8) -> NodeId { NodeId([b; 20]) }

    #[test]
    fn a_token_is_bound_to_the_address_that_asked() {
        // A token that travelled would let a peer write where it never read,
        // which is the whole thing tokens prevent.
        let r = TokenRing::new(1, 0);
        let t = r.issue(&sa("10.0.0.1:4222"), &key(1));
        assert!(r.check(&sa("10.0.0.1:4222"), &key(1), t));
        assert!(!r.check(&sa("10.0.0.2:4222"), &key(1), t), "reused from another host");
    }

    #[test]
    fn a_token_is_bound_to_the_key_it_was_issued_for() {
        let r = TokenRing::new(1, 0);
        let t = r.issue(&sa("10.0.0.1:4222"), &key(1));
        assert!(!r.check(&sa("10.0.0.1:4222"), &key(2), t));
    }

    #[test]
    fn the_port_is_not_part_of_the_binding() {
        // A NAT can rewrite the source port between the get and the put.
        // Binding to it would fail exactly the peers that need hole-punching
        // most.
        let r = TokenRing::new(1, 0);
        let t = r.issue(&sa("10.0.0.1:4222"), &key(1));
        assert!(r.check(&sa("10.0.0.1:9999"), &key(1), t));
    }

    #[test]
    fn a_token_stops_working_after_two_rotations() {
        // The bug this replaces: snapclone-dht's secret is set once at
        // construction, so a token is valid for the process lifetime and
        // anyone who observes one can announce forever.
        let mut r = TokenRing::new(1, 0);
        let who = sa("10.0.0.1:4222");
        let t = r.issue(&who, &key(1));
        r.tick(TOKEN_ROTATE_MS);
        assert!(r.check(&who, &key(1), t), "one rotation is still in grace");
        r.tick(TOKEN_ROTATE_MS * 2);
        assert!(!r.check(&who, &key(1), t), "two rotations must expire it");
    }

    #[test]
    fn a_get_just_before_a_rotation_can_still_put_just_after() {
        // With one live secret, a peer that read at 4:59 and wrote at 5:01
        // would fail for no reason it could see or fix. That is why two are
        // live at once.
        let mut r = TokenRing::new(7, 0);
        let who = sa("192.168.0.5:1234");
        let t = r.issue(&who, &key(3));
        r.tick(TOKEN_ROTATE_MS + 1);
        assert!(r.check(&who, &key(3), t));
    }

    #[test]
    fn rotation_only_happens_when_due() {
        // tick() is called per packet, so a rotation on every call would make
        // every token last one packet.
        let mut r = TokenRing::new(1, 0);
        let who = sa("10.0.0.1:4222");
        let t = r.issue(&who, &key(1));
        for ms in [1u64, 100, 1_000, TOKEN_ROTATE_MS - 1] { r.tick(ms); }
        assert!(r.check(&who, &key(1), t));
    }

    #[test]
    fn a_burst_from_one_source_is_cut_off() {
        // One peer in a retry loop does this by accident, and the effect on
        // everyone else is the same as an attack.
        let mut l = RateLimiter::default();
        let who = sa("203.0.113.9:5000");
        for i in 0..RATE_BURST { assert!(l.allow(&who, 1_000), "denied at {i}"); }
        assert!(!l.allow(&who, 1_000), "the burst should now be cut off");
    }

    #[test]
    fn the_limit_is_per_source_not_global() {
        // A busy peer must not deny service to everyone else — that would make
        // the limiter itself the outage.
        let mut l = RateLimiter::default();
        let noisy = sa("203.0.113.9:5000");
        for _ in 0..=RATE_BURST { l.allow(&noisy, 1_000); }
        assert!(!l.allow(&noisy, 1_000));
        assert!(l.allow(&sa("198.51.100.4:5000"), 1_000), "a quiet peer is unaffected");
    }

    #[test]
    fn the_window_refills() {
        let mut l = RateLimiter::default();
        let who = sa("203.0.113.9:5000");
        for _ in 0..=RATE_BURST { l.allow(&who, 0); }
        assert!(!l.allow(&who, 0));
        assert!(l.allow(&who, RATE_WINDOW_MS), "a new window starts fresh");
    }

    #[test]
    fn the_limiter_is_keyed_by_ip_not_by_port() {
        // Otherwise a source rolls a new ephemeral port per packet and the
        // limit never fires.
        let mut l = RateLimiter::default();
        for p in 0..=RATE_BURST {
            l.allow(&sa(&format!("203.0.113.9:{}", 1000 + p as u16)), 0);
        }
        assert!(!l.allow(&sa("203.0.113.9:60000"), 0), "ports must not evade the limit");
    }

    #[test]
    fn idle_sources_are_forgotten_so_the_limiter_cannot_exhaust_memory() {
        // The limiter must not become the exhaustion it prevents.
        let mut l = RateLimiter::default();
        for i in 0..500u32 {
            l.allow(&sa(&format!("10.{}.{}.1:1", i / 256, i % 256)), 0);
        }
        assert_eq!(l.tracked(), 500);
        l.gc(RATE_WINDOW_MS * 10);
        assert_eq!(l.tracked(), 0);
    }

    #[test]
    fn a_bucket_cannot_be_filled_from_one_subnet() {
        // A bucket an attacker owns answers every lookup that lands there and
        // eclipses the node from the rest of the network.
        let existing = vec![sa("203.0.113.1:1"), sa("203.0.113.2:1")];
        assert!(!subnet_allows(&existing, &sa("203.0.113.3:1")),
                "a third from the same /24 must be refused");
        assert!(subnet_allows(&existing, &sa("198.51.100.1:1")));
    }

    #[test]
    fn v6_diversity_is_measured_per_64_not_per_address() {
        // A /128 cap would treat one machine's many addresses as many
        // operators, which is exactly the confusion an attacker exploits.
        let existing = vec![sa("[2001:db8:0:1::1]:1"), sa("[2001:db8:0:1::2]:1")];
        assert!(!subnet_allows(&existing, &sa("[2001:db8:0:1::3]:1")));
        assert!(subnet_allows(&existing, &sa("[2001:db8:0:2::1]:1")), "a different /64");
    }

    #[test]
    fn a_lan_still_works_even_though_it_is_one_subnet() {
        // The cost of the policy, stated: on a genuine single-subnet network it
        // caps contacts. Two is still enough to route, which is why the number
        // is two rather than one.
        let existing: Vec<SocketAddr> = vec![];
        assert!(subnet_allows(&existing, &sa("192.168.1.10:1")));
        let one = vec![sa("192.168.1.10:1")];
        assert!(subnet_allows(&one, &sa("192.168.1.11:1")));
    }
}
