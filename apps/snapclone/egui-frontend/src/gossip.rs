//! gossip — SWIM-lite peer discovery from `GossipDiscovery`.
//!
//! *"Intentionally UDP-only and requires no central coordinator — it is purely
//! distributed."* Four rules:
//!
//! 1. JOIN broadcast on the local multicast group at startup.
//! 2. Each node keeps a **partial view** — a random subset of known live peers,
//!    size O(log N).
//! 3. Every tick, push our record plus a random subset of the view to `FANOUT`
//!    random peers.
//! 4. A peer unseen for the failure timeout is presumed dead.
//!
//! ## The partial view is what makes this scale, and why it must stay random
//!
//! O(log N) rather than everyone-knows-everyone. A full view costs O(N) memory
//! and O(N) gossip per node, which on a phone is the difference between a
//! background protocol and a battery complaint.
//!
//! But a partial view only works if the subsets **overlap by chance** — that is
//! what keeps the graph connected. Selecting deterministically (say, the first
//! K by id) partitions the network: every node picks the same K, those K carry
//! all traffic, and a node outside that set is unreachable from another node
//! outside it. Randomness here is load-bearing, not a nicety.
//!
//! ## The timings are already at the bottom
//!
//! *"kGossipTickMs (3s): already at the comfortable lower bound."* Worth
//! carrying forward, because 3 seconds looks like an obvious thing to lower
//! when discovery feels slow — and somebody already established it is not.
//!
//! ## Failure is five ticks, not one
//!
//! 15 s against a 3 s tick. A single missed heartbeat on UDP is ordinary — a
//! phone that switched cells, a dropped datagram — and declaring death on it
//! would make the peer list flicker constantly.

/// Multicast group and port for JOIN.
pub const MULTICAST_GROUP: &str = "224.0.0.251";
pub const GOSSIP_PORT: u16 = 9877;

/// Largest datagram accepted, enforced at read time to avoid over-allocation.
pub const MAX_DATAGRAM_BYTES: usize = 4096;
/// Longest single field inside a gossip record.
pub const MAX_FIELD_LEN: usize = 256;

/// How often we push.
pub const TICK_MS: u64 = 3_000;
/// How often we look for stale peers.
pub const PRUNE_MS: u64 = 8_000;
/// Silence after which a peer is presumed lost.
pub const FAILURE_MS: u64 = 15_000;
/// Peers pushed to per tick.
pub const FANOUT: usize = 3;

/// One peer in the view.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PeerRecord {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub last_seen_ms: u64,
    /// Whether the record's signature verified.
    pub verified: bool,
}

impl PeerRecord {
    pub fn is_alive(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.last_seen_ms) < FAILURE_MS
    }
}

/// Why a gossip datagram was rejected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reject {
    /// Larger than we will read.
    TooLarge,
    /// A field longer than the cap — a length field is attacker-controlled
    /// here, since gossip arrives from anyone on the subnet.
    FieldTooLong,
    /// The signature did not verify.
    ///
    /// *"Gossip messages are signed with the sender's identity key so nodes
    /// cannot impersonate each other."* Without this, one host on the subnet
    /// can announce itself as every peer and collect the traffic.
    BadSignature,
    /// It is our own record coming back.
    SelfEcho,
}

/// Validate an incoming record before it can enter the view.
pub fn accept(rec: &PeerRecord, datagram_len: usize, self_id: &str) -> Result<(), Reject> {
    if datagram_len > MAX_DATAGRAM_BYTES { return Err(Reject::TooLarge); }
    if rec.id.len() > MAX_FIELD_LEN || rec.host.len() > MAX_FIELD_LEN {
        return Err(Reject::FieldTooLong);
    }
    if !rec.verified { return Err(Reject::BadSignature); }
    if rec.id == self_id { return Err(Reject::SelfEcho); }
    Ok(())
}

/// The ideal partial-view size for a network of `n` known peers.
///
/// O(log N), floored at the fanout: a view smaller than the fanout means
/// pushing to the same peers every tick, which is a fixed subgraph rather than
/// a gossip protocol.
pub fn view_size(n: usize) -> usize {
    if n <= FANOUT { return n; }
    let log = (usize::BITS - n.leading_zeros()) as usize;
    log.max(FANOUT).min(n)
}

/// The partial view.
#[derive(Clone, Debug, Default)]
pub struct PeerView {
    peers: Vec<PeerRecord>,
}

impl PeerView {
    pub fn len(&self) -> usize { self.peers.len() }
    pub fn is_empty(&self) -> bool { self.peers.is_empty() }
    pub fn peers(&self) -> &[PeerRecord] { &self.peers }

    /// Learn about a peer, or refresh one we know.
    ///
    /// Refreshing rather than duplicating: gossip is redundant by design and
    /// the same peer arrives from several neighbours every tick.
    pub fn observe(&mut self, rec: PeerRecord) {
        match self.peers.iter_mut().find(|p| p.id == rec.id) {
            Some(existing) => {
                // Only move last_seen FORWARD. A relayed record can carry an
                // older timestamp than one we heard directly, and taking it
                // would age a live peer toward death on hearsay.
                if rec.last_seen_ms > existing.last_seen_ms {
                    existing.last_seen_ms = rec.last_seen_ms;
                    existing.host = rec.host;
                    existing.port = rec.port;
                }
            }
            None => self.peers.push(rec),
        }
    }

    /// Drop peers silent for longer than the failure timeout.
    ///
    /// Returns the ids lost, so the caller can emit exactly one event each.
    pub fn prune(&mut self, now_ms: u64) -> Vec<String> {
        let mut lost = Vec::new();
        self.peers.retain(|p| {
            if p.is_alive(now_ms) { true } else { lost.push(p.id.clone()); false }
        });
        lost
    }

    /// Pick `FANOUT` peers to push to this tick.
    ///
    /// `rand` supplies indices; the caller owns the RNG so this stays
    /// deterministic under test while remaining random in production. The
    /// randomness is what keeps the graph connected — see the module docs.
    pub fn select_targets(&self, mut rand: impl FnMut(usize) -> usize) -> Vec<&PeerRecord> {
        if self.peers.is_empty() { return Vec::new(); }
        let want = FANOUT.min(self.peers.len());
        let mut chosen: Vec<usize> = Vec::with_capacity(want);
        // Bounded attempts: with a small view the RNG can repeat, and an
        // unbounded retry loop would spin forever on a view of one.
        for _ in 0..want * 8 {
            if chosen.len() == want { break; }
            let i = rand(self.peers.len());
            if !chosen.contains(&i) { chosen.push(i); }
        }
        chosen.into_iter().map(|i| &self.peers[i]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, seen: u64) -> PeerRecord {
        PeerRecord { id: id.into(), host: "192.168.1.5".into(), port: 9876,
                     last_seen_ms: seen, verified: true }
    }

    #[test]
    fn an_unsigned_record_never_enters_the_view() {
        // "Gossip messages are signed... so nodes cannot impersonate each
        // other." Without it, one host on the subnet announces itself as every
        // peer and collects the traffic.
        let mut r = rec("sc-a", 0);
        r.verified = false;
        assert_eq!(accept(&r, 100, "sc-me"), Err(Reject::BadSignature));
    }

    #[test]
    fn our_own_record_coming_back_is_not_a_peer() {
        // Multicast echoes to the sender; adding ourselves would have the node
        // gossip with itself and inflate every view by one.
        assert_eq!(accept(&rec("sc-me", 0), 100, "sc-me"), Err(Reject::SelfEcho));
    }

    #[test]
    fn an_oversized_datagram_is_refused_before_allocation() {
        // Gossip arrives from anyone on the subnet, so the length is
        // attacker-controlled and the cap is "enforced at read time to avoid
        // over-allocation".
        assert_eq!(accept(&rec("sc-a", 0), MAX_DATAGRAM_BYTES + 1, "sc-me"),
                   Err(Reject::TooLarge));
        assert!(accept(&rec("sc-a", 0), MAX_DATAGRAM_BYTES, "sc-me").is_ok());
    }

    #[test]
    fn an_overlong_field_is_refused() {
        let mut r = rec("sc-a", 0);
        r.host = "x".repeat(MAX_FIELD_LEN + 1);
        assert_eq!(accept(&r, 100, "sc-me"), Err(Reject::FieldTooLong));
    }

    #[test]
    fn the_view_grows_logarithmically_not_linearly() {
        // A full view costs O(N) memory and O(N) gossip per node, which on a
        // phone is the difference between a background protocol and a battery
        // complaint.
        assert!(view_size(1_000) < 20);
        assert!(view_size(1_000_000) < 30);
        assert!(view_size(1_000) > view_size(10), "the view should still grow");
    }

    #[test]
    fn the_view_is_never_smaller_than_the_fanout() {
        // A view below the fanout means pushing to the same peers every tick,
        // which is a fixed subgraph rather than a gossip protocol.
        assert!(view_size(100) >= FANOUT);
        assert_eq!(view_size(2), 2, "but it cannot exceed what we know");
        assert_eq!(view_size(0), 0);
    }

    #[test]
    fn hearing_a_peer_twice_refreshes_rather_than_duplicating() {
        // Gossip is redundant by design; the same peer arrives from several
        // neighbours every tick.
        let mut v = PeerView::default();
        v.observe(rec("sc-a", 100));
        v.observe(rec("sc-a", 200));
        assert_eq!(v.len(), 1);
        assert_eq!(v.peers()[0].last_seen_ms, 200);
    }

    #[test]
    fn a_relayed_older_record_does_not_age_a_live_peer() {
        // A record relayed through two hops can carry an older timestamp than
        // one heard directly, and taking it would move a live peer toward
        // death ON HEARSAY.
        let mut v = PeerView::default();
        v.observe(rec("sc-a", 10_000));
        v.observe(rec("sc-a", 1_000));
        assert_eq!(v.peers()[0].last_seen_ms, 10_000);
    }

    #[test]
    fn a_peer_is_lost_after_five_missed_ticks_not_one() {
        // A single missed heartbeat on UDP is ordinary — a phone that switched
        // cells, a dropped datagram — and declaring death on it would make the
        // peer list flicker constantly.
        assert_eq!(FAILURE_MS / TICK_MS, 5);
        let mut v = PeerView::default();
        v.observe(rec("sc-a", 0));
        assert!(v.prune(TICK_MS).is_empty(), "lost a peer after one tick");
        assert!(v.prune(FAILURE_MS - 1).is_empty());
        assert_eq!(v.prune(FAILURE_MS), vec!["sc-a".to_string()]);
        assert!(v.is_empty());
    }

    #[test]
    fn a_lost_peer_is_reported_once() {
        // The caller emits peerLost per id; reporting twice would show two
        // disconnect notices for one peer.
        let mut v = PeerView::default();
        v.observe(rec("sc-a", 0));
        assert_eq!(v.prune(FAILURE_MS).len(), 1);
        assert!(v.prune(FAILURE_MS).is_empty());
    }

    #[test]
    fn targets_are_chosen_randomly_and_that_is_load_bearing() {
        // Selecting deterministically PARTITIONS the network: every node picks
        // the same K, those K carry all traffic, and a node outside that set is
        // unreachable from another node outside it. Randomness keeps the graph
        // connected.
        let mut v = PeerView::default();
        for i in 0..10 { v.observe(rec(&format!("sc-{i}"), 0)); }
        let mut seq = [7usize, 2, 9].into_iter().cycle();
        let picked: Vec<&str> = v.select_targets(|_| seq.next().unwrap())
            .iter().map(|p| p.id.as_str()).collect();
        assert_eq!(picked.len(), FANOUT);
        // a different draw reaches different peers
        let mut other = [0usize, 1, 3].into_iter().cycle();
        let picked2: Vec<&str> = v.select_targets(|_| other.next().unwrap())
            .iter().map(|p| p.id.as_str()).collect();
        assert_ne!(picked, picked2);
    }

    #[test]
    fn the_same_peer_is_never_pushed_to_twice_in_one_tick() {
        // Two of three pushes wasted, and on a phone that is radio spent for
        // nothing.
        let mut v = PeerView::default();
        for i in 0..5 { v.observe(rec(&format!("sc-{i}"), 0)); }
        let targets = v.select_targets(|_| 2);
        let ids: Vec<&str> = targets.iter().map(|p| p.id.as_str()).collect();
        let mut uniq = ids.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(ids.len(), uniq.len());
    }

    #[test]
    fn a_tiny_view_does_not_spin_looking_for_distinct_targets() {
        // With a view of one, an unbounded retry loop would never find three
        // distinct indices and would hang the gossip tick.
        let mut v = PeerView::default();
        v.observe(rec("sc-only", 0));
        assert_eq!(v.select_targets(|_| 0).len(), 1);
        assert!(PeerView::default().select_targets(|_| 0).is_empty());
    }

    #[test]
    fn the_tick_is_at_its_documented_lower_bound() {
        // "kGossipTickMs (3s): already at the comfortable lower bound." Worth
        // carrying forward — 3 seconds looks like an obvious thing to lower
        // when discovery feels slow, and somebody already established it is
        // not.
        assert_eq!(TICK_MS, 3_000);
        assert_eq!(PRUNE_MS, 8_000);
        assert!(PRUNE_MS < FAILURE_MS, "pruning must run before the timeout expires");
    }
}
