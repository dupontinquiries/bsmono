//! punch_addrs — `PeerPunchAddrView` and `GossipView`, ported.
//!
//! Two network surfaces the audit lists as missing, and they are the ones that
//! make the DHT work visible: where a peer was last reachable, and who has been
//! heard from lately.
//!
//! ## The cache is bounded because a peer list is attacker-influenced
//!
//! Anyone who can gossip can cause an entry. The Qt side caps at 1,024 with a
//! 30-minute TTL and evicts 256 at a time, and those numbers are the interesting
//! part — not the map.
//!
//! **Evicting a quarter rather than one** is the choice worth keeping. Dropping
//! a single oldest entry per insert means a full cache does a sort on every
//! write forever; dropping 256 amortises that to one sort per 256 writes. The
//! Qt code makes the same trade for its nonce cache with the same comment, so
//! it is a house pattern rather than a local hack.
//!
//! **The cap is checked before the insert**, so the new entry fits afterwards.
//! Checking after means the cache sits one over its limit between every write
//! and eviction, which is the kind of off-by-one that only shows up as memory
//! growth under load.

use std::collections::HashMap;

/// How long a punch address stays useful. Thirty minutes — a NAT binding
/// outlives a call but not an afternoon.
pub const TTL_MS: u64 = 30 * 60 * 1000;
/// Hard cap on remembered addresses.
pub const MAX_ADDRS: usize = 1024;
/// How many are dropped when the cap is hit.
pub const EVICT_BATCH: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PeerAddr {
    pub ip: [u8; 4],
    pub port: u16,
}

impl PeerAddr {
    pub fn new(ip: [u8; 4], port: u16) -> PeerAddr { PeerAddr { ip, port } }
    pub fn is_unset(&self) -> bool { self.port == 0 && self.ip == [0, 0, 0, 0] }
}

impl Default for PeerAddr {
    fn default() -> Self { PeerAddr { ip: [0, 0, 0, 0], port: 0 } }
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    addr: PeerAddr,
    inserted_ms: u64,
}

/// Where each peer was last reachable for hole-punching.
#[derive(Clone, Default, Debug)]
pub struct PunchAddrs {
    map: HashMap<String, Entry>,
}

impl PunchAddrs {
    pub fn len(&self) -> usize { self.map.len() }
    pub fn is_empty(&self) -> bool { self.map.is_empty() }
    pub fn contains(&self, id: &str) -> bool { self.map.contains_key(id) }
    pub fn clear(&mut self) { self.map.clear(); }

    /// The address, or `None`. The Qt shim returns a default-constructed
    /// `PeerAddress` for a miss, which reads as 0.0.0.0:0 — a real address a
    /// caller could try to punch to. `Option` makes the miss impossible to use
    /// by accident, and `PeerAddr::is_unset` exists for call sites that still
    /// want the old shape.
    /// Every remembered (id, address), newest first. For the DHT's
    /// bootstrap list: a peer we have spoken to is a node we can start
    /// from, and its id is derivable from its sc-ID, which is more than
    /// any bootstrap server offers.
    pub fn all(&self) -> Vec<(String, PeerAddr)> {
        let mut v: Vec<(String, PeerAddr, u64)> = self.map.iter()
            .map(|(id, e)| (id.clone(), e.addr, e.inserted_ms)).collect();
        v.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        v.into_iter().map(|(id, a, _)| (id, a)).collect()
    }

    pub fn get(&self, id: &str) -> Option<PeerAddr> {
        self.map.get(id).map(|e| e.addr)
    }

    pub fn get_or(&self, id: &str, default: PeerAddr) -> PeerAddr {
        self.get(id).unwrap_or(default)
    }

    /// Record an address, enforcing the cap first.
    pub fn set(&mut self, id: &str, addr: PeerAddr, now_ms: u64) {
        // Before the insert, so the cache is never over its limit — checking
        // after leaves it one over between every write and eviction.
        if !self.map.contains_key(id) && self.map.len() >= MAX_ADDRS {
            self.evict_oldest(EVICT_BATCH);
        }
        self.map.insert(id.to_string(), Entry { addr, inserted_ms: now_ms });
    }

    /// Drop entries older than the TTL.
    pub fn expire(&mut self, now_ms: u64) -> usize {
        let before = self.map.len();
        // Saturating: a clock that went backwards must not make every entry
        // look infinitely old and wipe the cache.
        self.map.retain(|_, e| now_ms.saturating_sub(e.inserted_ms) < TTL_MS);
        before - self.map.len()
    }

    fn evict_oldest(&mut self, n: usize) {
        let mut by_age: Vec<(u64, String)> = self.map.iter()
            .map(|(k, e)| (e.inserted_ms, k.clone()))
            .collect();
        // Key breaks ties so eviction is deterministic — two entries written in
        // the same millisecond must not evict differently between runs, or a
        // reproduction stops reproducing.
        by_age.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        for (_, k) in by_age.into_iter().take(n) {
            self.map.remove(&k);
        }
    }
}

/// A peer heard about through gossip.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GossipPeer {
    pub sc_id: String,
    pub addr: PeerAddr,
    pub seen_ms: u64,
}

/// Who has been heard from, newest first.
#[derive(Clone, Default, Debug)]
pub struct Gossip {
    peers: HashMap<String, GossipPeer>,
}

impl Gossip {
    pub fn insert(&mut self, peer: GossipPeer) {
        // Newest wins on a re-sighting rather than first-seen: a peer that
        // moved is at its new address, and keeping the old one sends punches
        // somewhere it has left.
        match self.peers.get(&peer.sc_id) {
            Some(existing) if existing.seen_ms > peer.seen_ms => {}
            _ => { self.peers.insert(peer.sc_id.clone(), peer); }
        }
    }

    pub fn get(&self, sc_id: &str) -> Option<&GossipPeer> { self.peers.get(sc_id) }
    pub fn len(&self) -> usize { self.peers.len() }
    pub fn is_empty(&self) -> bool { self.peers.is_empty() }

    /// Newest first, ties broken by id so the list does not reshuffle between
    /// frames — a row that moves under a tap is a row you mis-tap.
    pub fn recent(&self) -> Vec<&GossipPeer> {
        let mut v: Vec<&GossipPeer> = self.peers.values().collect();
        v.sort_by(|a, b| b.seen_ms.cmp(&a.seen_ms).then_with(|| a.sc_id.cmp(&b.sc_id)));
        v
    }

    pub fn purge_stale(&mut self, now_ms: u64, max_age_ms: u64) -> usize {
        let before = self.peers.len();
        self.peers.retain(|_, p| now_ms.saturating_sub(p.seen_ms) < max_age_ms);
        before - self.peers.len()
    }
}

/// How a peer's last sighting reads.
pub fn seen_label(now_ms: u64, seen_ms: u64) -> String {
    let d = now_ms.saturating_sub(seen_ms) / 1000;
    match d {
        0..=5 => "now".into(),
        6..=59 => format!("{d}s ago"),
        60..=3599 => format!("{}m ago", d / 60),
        _ => format!("{}h ago", d / 3600),
    }
}

/// Draw the gossip roster.
pub fn gossip_ui(ui: &mut egui::Ui, g: &Gossip, punch: &PunchAddrs, now_ms: u64,
                 c: &uikit_egui::PaletteColors) {
    if g.is_empty() {
        ui.colored_label(c.text_weak, "No peers heard from yet.");
        return;
    }
    for p in g.recent() {
        ui.horizontal(|ui| {
            ui.colored_label(c.text, &p.sc_id);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.colored_label(c.text_weak,
                    egui::RichText::new(seen_label(now_ms, p.seen_ms)).small());
                // A punchable address is the actionable fact: it means a direct
                // connection can be attempted rather than a relay.
                if punch.contains(&p.sc_id) {
                    ui.colored_label(c.success, egui::RichText::new("direct").small());
                }
            });
        });
    }
}

// ── whether an address is still worth trying ────────────────────────────────

/// A cached address's failure record, per peer.
///
/// `store_privacy::CachedAddr` carries the rule — an address that has
/// failed repeatedly is known wrong, and trying it again costs a timeout
/// the person waits through — and nothing held one, so a dead address was
/// retried forever. The snap send path reads `PunchAddrs::get` and, on
/// failure, recorded nothing.
///
/// Kept BESIDE the address map rather than inside it. An address arrives
/// from gossip or the DHT and is replaced wholesale; a failure record
/// attached to the value would be reset by every refresh, including the
/// refresh that hands back the same wrong address.
#[derive(Debug, Clone, Default)]
pub struct AddrHealth {
    map: std::collections::HashMap<String, crate::store_privacy::CachedAddr>,
}

impl AddrHealth {
    /// A send to this peer failed.
    pub fn failed(&mut self, id: &str, addr: PeerAddr, now_ms: i64) {
        let e = self.map.entry(id.to_string()).or_insert_with(|| {
            crate::store_privacy::CachedAddr {
                host: format!("{}.{}.{}.{}", addr.ip[0], addr.ip[1], addr.ip[2], addr.ip[3]),
                port: addr.port,
                consecutive_failures: 0,
                cached_at_ms: now_ms,
            }
        });
        e.record_failure();
    }

    /// A send to this peer worked.
    ///
    /// Clears the record entirely rather than decrementing: a peer that was
    /// unreachable for an hour and is now answering is not four-fifths
    /// broken, and carrying old failures forward would abandon it again
    /// after one more blip.
    pub fn worked(&mut self, id: &str, now_ms: i64) {
        if let Some(e) = self.map.get_mut(id) {
            e.record_success(now_ms);
        }
    }

    /// Should the cached address be used, or should the peer be looked up
    /// again?
    ///
    /// Unknown peers are USABLE: never having failed is not evidence
    /// against an address, and refusing to try one would mean no snap is
    /// ever sent to a new contact.
    pub fn usable(&self, id: &str, now_ms: i64) -> bool {
        self.map.get(id).map(|e| e.is_usable(now_ms)).unwrap_or(true)
    }

    /// Forget a peer's record — for a fresh address from gossip or the DHT,
    /// which deserves its own attempts rather than inheriting the last
    /// one's.
    pub fn forget(&mut self, id: &str) {
        self.map.remove(id);
    }

    pub fn failures(&self, id: &str) -> u32 {
        self.map.get(id).map(|e| e.consecutive_failures).unwrap_or(0)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_network_view_is_drawn_and_its_caches_are_fed() {
        // Both halves were missing. gossip_ui was never called, and
        // note_peer_addr was reached only from a test — so `heard` and `punch`
        // were always empty and the view could not have shown anything however
        // it was drawn.
        let lib = include_str!("lib.rs");
        assert!(lib.contains("punch_addrs::gossip_ui("), "the view is not drawn");
        assert!(lib.contains("self.note_peer_addr(&id,"),
                "arriving gossip does not reach the reach caches");
    }

    #[test]
    fn only_an_address_gets_cached_as_one() {
        // PeerAddr is four bytes. A hostname or an IPv6 literal has no honest
        // conversion, and a zero address in this cache would read as a
        // reachable peer — "a guess wearing a fact's clothes", which is the
        // failure the module's own header names.
        let d = crate::parse_dotted_quad("192.168.1.9");
        assert_eq!(d, Some([192, 168, 1, 9]));
        assert_eq!(crate::parse_dotted_quad("peer.local"), None, "a hostname is not an address");
        assert_eq!(crate::parse_dotted_quad("::1"), None, "IPv6 has no [u8; 4] form");
        assert_eq!(crate::parse_dotted_quad("1.2.3"), None, "too few octets");
        assert_eq!(crate::parse_dotted_quad("1.2.3.4.5"), None, "too many");
        assert_eq!(crate::parse_dotted_quad("1.2.3.999"), None, "an octet must fit a byte");
    }

    fn addr(n: u8) -> PeerAddr { PeerAddr::new([10, 0, 0, n], 4000 + n as u16) }

    #[test]
    fn the_cap_is_enforced_before_the_insert_not_after() {
        // Checking after leaves the cache one over its limit between every
        // write and eviction — an off-by-one that only shows up as memory
        // growth under load.
        let mut p = PunchAddrs::default();
        for i in 0..MAX_ADDRS {
            p.set(&format!("peer{i}"), addr(1), i as u64);
        }
        assert_eq!(p.len(), MAX_ADDRS);
        p.set("one-more", addr(2), 99_999);
        assert!(p.len() <= MAX_ADDRS, "went over the cap: {}", p.len());
        assert!(p.contains("one-more"), "the new entry must survive its own eviction");
    }

    #[test]
    fn eviction_drops_a_batch_so_a_full_cache_does_not_sort_on_every_write() {
        // One-at-a-time eviction means a sort per insert forever. The batch
        // amortises it, which is why the Qt side drops 25% and why this does.
        let mut p = PunchAddrs::default();
        for i in 0..MAX_ADDRS { p.set(&format!("peer{i}"), addr(1), i as u64); }
        p.set("new", addr(2), 99_999);
        assert!(p.len() <= MAX_ADDRS - EVICT_BATCH + 1,
                "expected a batch to go, len is {}", p.len());
    }

    #[test]
    fn eviction_takes_the_oldest_and_is_deterministic_on_ties() {
        // Two entries written in the same millisecond must not evict
        // differently between runs, or a reproduction stops reproducing.
        let mut a = PunchAddrs::default();
        let mut b = PunchAddrs::default();
        for p in [&mut a, &mut b] {
            for i in 0..MAX_ADDRS { p.set(&format!("peer{i:04}"), addr(1), 7); }
            p.set("new", addr(2), 8);
        }
        let survived_a: Vec<bool> = (0..MAX_ADDRS)
            .map(|i| a.contains(&format!("peer{i:04}"))).collect();
        let survived_b: Vec<bool> = (0..MAX_ADDRS)
            .map(|i| b.contains(&format!("peer{i:04}"))).collect();
        assert_eq!(survived_a, survived_b, "eviction differed between runs");
    }

    #[test]
    fn re_setting_a_known_peer_does_not_trigger_eviction() {
        // A peer that moves is an update, not a new entry, and evicting 256
        // others because somebody changed address would be absurd.
        let mut p = PunchAddrs::default();
        for i in 0..MAX_ADDRS { p.set(&format!("peer{i}"), addr(1), i as u64); }
        p.set("peer0", addr(9), 50_000);
        assert_eq!(p.len(), MAX_ADDRS);
        assert_eq!(p.get("peer0"), Some(addr(9)));
    }

    #[test]
    fn entries_expire_after_the_ttl() {
        let mut p = PunchAddrs::default();
        p.set("a", addr(1), 0);
        p.set("b", addr(2), TTL_MS);
        assert_eq!(p.expire(TTL_MS + 1), 1);
        assert!(!p.contains("a"));
        assert!(p.contains("b"));
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_wipe_the_cache() {
        // now < inserted underflows on unsigned arithmetic, and an age of
        // u64::MAX would expire everything. NTP corrections happen.
        let mut p = PunchAddrs::default();
        p.set("a", addr(1), 10_000);
        assert_eq!(p.expire(5_000), 0, "a backwards clock evicted live entries");
        assert!(p.contains("a"));
    }

    #[test]
    fn a_miss_is_none_rather_than_an_address_that_looks_punchable() {
        // The Qt shim returns a default-constructed PeerAddress for a miss,
        // which reads as 0.0.0.0:0 — an address a caller could try to punch to.
        let p = PunchAddrs::default();
        assert_eq!(p.get("nobody"), None);
        assert!(PeerAddr::default().is_unset());
        assert_eq!(p.get_or("nobody", addr(3)), addr(3));
    }

    #[test]
    fn a_peer_seen_again_moves_to_its_new_address() {
        // Keeping the first sighting sends punches where the peer used to be.
        let mut g = Gossip::default();
        g.insert(GossipPeer { sc_id: "sc-a".into(), addr: addr(1), seen_ms: 100 });
        g.insert(GossipPeer { sc_id: "sc-a".into(), addr: addr(2), seen_ms: 200 });
        assert_eq!(g.len(), 1);
        assert_eq!(g.get("sc-a").unwrap().addr, addr(2));
    }

    #[test]
    fn an_older_sighting_does_not_overwrite_a_newer_one() {
        // Gossip arrives out of order — a peer's old address relayed late must
        // not undo what we already learned.
        let mut g = Gossip::default();
        g.insert(GossipPeer { sc_id: "sc-a".into(), addr: addr(2), seen_ms: 200 });
        g.insert(GossipPeer { sc_id: "sc-a".into(), addr: addr(1), seen_ms: 100 });
        assert_eq!(g.get("sc-a").unwrap().addr, addr(2));
    }

    #[test]
    fn the_roster_is_newest_first_and_stable() {
        // A row that moves under a tap is a row you mis-tap.
        let mut g = Gossip::default();
        for (id, t) in [("sc-c", 300u64), ("sc-a", 100), ("sc-b", 300)] {
            g.insert(GossipPeer { sc_id: id.into(), addr: addr(1), seen_ms: t });
        }
        let ids: Vec<&str> = g.recent().iter().map(|p| p.sc_id.as_str()).collect();
        assert_eq!(ids, vec!["sc-b", "sc-c", "sc-a"], "ties break by id");
        assert_eq!(g.recent().iter().map(|p| p.sc_id.as_str()).collect::<Vec<_>>(), ids);
    }

    #[test]
    fn stale_peers_are_purged_and_the_count_is_reported() {
        let mut g = Gossip::default();
        g.insert(GossipPeer { sc_id: "old".into(), addr: addr(1), seen_ms: 0 });
        g.insert(GossipPeer { sc_id: "new".into(), addr: addr(2), seen_ms: 9_000 });
        assert_eq!(g.purge_stale(10_000, 5_000), 1);
        assert!(g.get("new").is_some());
    }

    #[test]
    fn a_sighting_reads_as_time_rather_than_a_timestamp() {
        assert_eq!(seen_label(1_000, 1_000), "now");
        assert_eq!(seen_label(30_000, 0), "30s ago");
        assert_eq!(seen_label(600_000, 0), "10m ago");
        assert_eq!(seen_label(7_200_000, 0), "2h ago");
    }
}

#[cfg(test)]
mod health_tests {
    use super::*;

    const A: PeerAddr = PeerAddr { ip: [10, 0, 0, 9], port: 9876 };

    #[test]
    fn a_peer_never_tried_is_worth_trying() {
        // Refusing an unknown address would mean no snap is ever sent to a
        // new contact.
        let h = AddrHealth::default();
        assert!(h.usable("sc-new", 0));
    }

    #[test]
    fn an_address_that_keeps_failing_stops_being_used() {
        // Each retry costs a timeout the person waits through, and the
        // address is known wrong by then.
        let mut h = AddrHealth::default();
        for _ in 0..crate::store_privacy::ADDR_FAIL_THRESHOLD {
            h.failed("sc-1", A, 0);
        }
        assert!(!h.usable("sc-1", 0), "{} failures", h.failures("sc-1"));
    }

    #[test]
    fn one_success_clears_the_record_rather_than_decrementing_it() {
        // A peer unreachable for an hour and now answering is not
        // four-fifths broken; carrying failures forward abandons it again
        // after one more blip.
        let mut h = AddrHealth::default();
        for _ in 0..4 { h.failed("sc-1", A, 0); }
        h.worked("sc-1", 1_000);
        assert_eq!(h.failures("sc-1"), 0);
        assert!(h.usable("sc-1", 1_000));
    }

    #[test]
    fn a_stale_record_forces_a_lookup_even_with_no_failures() {
        // Both conditions, because they fail differently: a stale entry
        // may still be correct and is worth one attempt, but not forever.
        let mut h = AddrHealth::default();
        h.failed("sc-1", A, 0);
        h.worked("sc-1", 0);
        let later = crate::store_privacy::DHT_CACHE_TTL_MS + 1;
        assert!(!h.usable("sc-1", later));
    }

    #[test]
    fn a_fresh_address_gets_its_own_attempts() {
        // Gossip or the DHT handing back a new address must not inherit
        // the last one's failures — that would abandon a working address
        // for the sins of the one it replaced.
        let mut h = AddrHealth::default();
        for _ in 0..9 { h.failed("sc-1", A, 0); }
        h.forget("sc-1");
        assert!(h.usable("sc-1", 0));
    }
}
