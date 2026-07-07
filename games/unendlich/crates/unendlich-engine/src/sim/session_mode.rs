//! `sim::session_mode` — the pure decision layer for unendlich-fullgame's multiplayer:
//! the session role the world loop runs under, a server browser over advertised
//! worlds, and the host/join sub-FSMs. It sits above `net_candidates` (which target
//! to punch, in what transport order) and `net_rendezvous` (the actual hole-punch),
//! and below the menu (`game_flow`/`menu_screen_layout`). The bin owns DHT/sockets;
//! this owns *which world, what role, and what to try*. Pure: no IO/clock/RNG.

use crate::sim::net_candidates::{is_dead_end, order_targets, transport_order, Candidate, Transport};

/// What `run_world` is launched as. `Local` keeps DHT off (singleplayer);
/// `Host`/`Client` turn the world-join machinery on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRole {
    Local,
    Host,
    Client,
}

/// A world advertised on the DHT — what a host announces and a client browses.
/// `host_username` is the human handle (per the identity decision); `candidates`
/// are the host's reachable endpoints for `net_candidates::order_targets`.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldAdvert {
    pub host_username: String,
    pub world_name: String,
    pub players: u16,
    pub max_players: u16,
    pub last_seen_ms: u64,
    pub candidates: Vec<Candidate>,
    /// The host's world seed. A joiner MUST rebuild its world for this seed on
    /// connect, or it generates a divergent world — the multiplayer analog of
    /// the r346 single-player "two seeds in one world" bug. Carried in the
    /// advert so the joiner learns it before the world handshake.
    pub world_seed: u64,
}

impl WorldAdvert {
    pub fn is_full(&self) -> bool {
        self.max_players > 0 && self.players >= self.max_players
    }
    /// Stable identity for dedupe: a host's world (username + world name).
    pub fn key(&self) -> (String, String) {
        (self.host_username.clone(), self.world_name.clone())
    }
}

/// The Join screen's list of discovered worlds (deduped + freshest-first).
#[derive(Debug, Default)]
pub struct ServerBrowser {
    adverts: Vec<WorldAdvert>,
    selected: Option<(String, String)>,
}

impl ServerBrowser {
    pub fn new() -> ServerBrowser {
        ServerBrowser { adverts: Vec::new(), selected: None }
    }

    /// Fold a freshly-heard advert in: replace an existing one with the same key
    /// (keeping the newer `last_seen_ms`), else append.
    pub fn observe(&mut self, a: WorldAdvert) {
        if let Some(slot) = self.adverts.iter_mut().find(|x| x.key() == a.key()) {
            if a.last_seen_ms >= slot.last_seen_ms {
                *slot = a;
            }
            return;
        }
        self.adverts.push(a);
    }

    /// Drop adverts not heard within `ttl_ms` of `now_ms`.
    pub fn prune(&mut self, now_ms: u64, ttl_ms: u64) {
        self.adverts.retain(|a| now_ms.saturating_sub(a.last_seen_ms) <= ttl_ms);
        if let Some(sel) = &self.selected {
            if !self.adverts.iter().any(|a| &a.key() == sel) {
                self.selected = None;
            }
        }
    }

    /// Freshest-first; ties broken by host_username for stability.
    pub fn by_recent(&self) -> Vec<&WorldAdvert> {
        let mut v: Vec<&WorldAdvert> = self.adverts.iter().collect();
        v.sort_by(|a, b| {
            b.last_seen_ms
                .cmp(&a.last_seen_ms)
                .then_with(|| a.host_username.cmp(&b.host_username))
        });
        v
    }

    pub fn select(&mut self, key: (String, String)) -> bool {
        if self.adverts.iter().any(|a| a.key() == key) {
            self.selected = Some(key);
            true
        } else {
            false
        }
    }

    pub fn selected(&self) -> Option<&WorldAdvert> {
        let sel = self.selected.as_ref()?;
        self.adverts.iter().find(|a| &a.key() == sel)
    }

    pub fn len(&self) -> usize {
        self.adverts.len()
    }
    pub fn is_empty(&self) -> bool {
        self.adverts.is_empty()
    }
}

/// What the bin should actually attempt to reach a host, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectPlan {
    pub targets: Vec<Candidate>,
    pub transports: Vec<Transport>,
    /// No reachable direct target — the caller must fall to relay/mailbox.
    pub dead_end: bool,
}

/// Build the connection plan for a chosen advert: LAN-first targets (hairpin
/// dropped), then the transport order given the local NAT situation.
pub fn plan_connect(advert: &WorldAdvert, self_public: [u8; 4], is_cgnat: bool, has_relay: bool) -> ConnectPlan {
    let targets = order_targets(&advert.candidates, self_public);
    let dead_end = is_dead_end(&advert.candidates, self_public);
    let transports = transport_order(!dead_end, is_cgnat, has_relay);
    ConnectPlan { targets, transports, dead_end }
}

/// The Join flow the menu drives after the player picks a world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinPhase {
    Browsing,
    Connecting,
    Connected,
    Failed,
}

#[derive(Debug, Default)]
pub struct JoinFlow {
    phase: JoinPhase,
    /// Host's world seed, captured from the advert at connect time. The bin
    /// reads this on reaching `Connected` and rebuilds the joiner's world for
    /// it (the multiplayer seed-adoption fix). `None` until a connect attempt.
    host_seed: Option<u64>,
}

impl Default for JoinPhase {
    fn default() -> Self {
        JoinPhase::Browsing
    }
}

impl JoinFlow {
    pub fn new() -> JoinFlow {
        JoinFlow { phase: JoinPhase::Browsing, host_seed: None }
    }
    pub fn phase(&self) -> JoinPhase {
        self.phase
    }
    /// The host's world seed to adopt, available once a connect has been
    /// committed. The bin MUST rebuild the joiner's world for this seed when
    /// the flow reaches `Connected`, or the joiner generates a divergent world.
    pub fn host_seed(&self) -> Option<u64> {
        self.host_seed
    }
    /// Commit to a selected advert → Connecting, returning the plan to execute.
    /// Captures the host's world seed for adoption on connect.
    pub fn connect(&mut self, advert: &WorldAdvert, self_public: [u8; 4], is_cgnat: bool, has_relay: bool) -> ConnectPlan {
        self.phase = JoinPhase::Connecting;
        self.host_seed = Some(advert.world_seed);
        plan_connect(advert, self_public, is_cgnat, has_relay)
    }
    pub fn on_connected(&mut self) {
        if self.phase == JoinPhase::Connecting {
            self.phase = JoinPhase::Connected;
        }
    }
    pub fn on_failed(&mut self) {
        if self.phase == JoinPhase::Connecting {
            self.phase = JoinPhase::Failed;
        }
    }
    /// Return to the browser (from Failed or Connected).
    pub fn back_to_browse(&mut self) {
        self.phase = JoinPhase::Browsing;
        self.host_seed = None;
    }
}

/// The Host flow: configure → announce → live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPhase {
    Configuring,
    Announcing,
    Live,
}

#[derive(Debug)]
pub struct HostFlow {
    phase: HostPhase,
}

impl Default for HostFlow {
    fn default() -> Self {
        HostFlow { phase: HostPhase::Configuring }
    }
}

impl HostFlow {
    pub fn new() -> HostFlow {
        HostFlow::default()
    }
    pub fn phase(&self) -> HostPhase {
        self.phase
    }
    /// Begin announcing the world to the DHT.
    pub fn open(&mut self) {
        if self.phase == HostPhase::Configuring {
            self.phase = HostPhase::Announcing;
        }
    }
    /// First joiner admitted / world fully up.
    pub fn go_live(&mut self) {
        if self.phase == HostPhase::Announcing {
            self.phase = HostPhase::Live;
        }
    }
    pub fn close(&mut self) {
        self.phase = HostPhase::Configuring;
    }
    /// Build the advert this host publishes.
    pub fn make_advert(
        host_username: &str,
        world_name: &str,
        max_players: u16,
        candidates: Vec<Candidate>,
        now_ms: u64,
        world_seed: u64,
    ) -> WorldAdvert {
        WorldAdvert {
            host_username: host_username.to_string(),
            world_name: world_name.to_string(),
            players: 1, // the host counts
            max_players,
            last_seen_ms: now_ms,
            candidates,
            world_seed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::net_candidates::CandKind;

    fn cand(host: [u8; 4], kind: CandKind) -> Candidate {
        Candidate { host, port: 7777, kind }
    }
    fn advert(user: &str, world: &str, seen: u64, cands: Vec<Candidate>) -> WorldAdvert {
        WorldAdvert {
            host_username: user.into(),
            world_name: world.into(),
            players: 1,
            max_players: 8,
            last_seen_ms: seen,
            candidates: cands,
            world_seed: 0,
        }
    }

    #[test]
    fn browser_dedupes_by_key_keeping_freshest_and_sorts_recent() {
        let mut b = ServerBrowser::new();
        b.observe(advert("thornwhisper", "Vale", 100, vec![]));
        b.observe(advert("ashfin", "Deeps", 200, vec![]));
        b.observe(advert("thornwhisper", "Vale", 300, vec![])); // newer dupe
        assert_eq!(b.len(), 2, "same (user,world) deduped");
        let recent = b.by_recent();
        assert_eq!(recent[0].host_username, "thornwhisper");
        assert_eq!(recent[0].last_seen_ms, 300, "kept the freshest");
        assert_eq!(recent[1].host_username, "ashfin");
    }

    #[test]
    fn browser_prunes_stale_and_clears_dangling_selection() {
        let mut b = ServerBrowser::new();
        b.observe(advert("a", "w", 100, vec![]));
        b.observe(advert("b", "w", 500, vec![]));
        assert!(b.select(("a".into(), "w".into())));
        b.prune(1000, 600); // a is 900 old > 600 ⇒ dropped; b is 500 old ≤ 600 ⇒ kept
        assert_eq!(b.len(), 1);
        assert!(b.selected().is_none(), "selection cleared when its advert pruned");
    }

    #[test]
    fn plan_connect_lan_first_and_drops_hairpin() {
        let self_pub = [203, 0, 113, 5];
        // only the host's shared public IP == ours ⇒ dead end (snapclone bug)
        let only_hairpin = advert("h", "w", 1, vec![cand(self_pub, CandKind::Public)]);
        let p = plan_connect(&only_hairpin, self_pub, false, true);
        assert!(p.dead_end, "shared public IP is a dead end");
        assert!(p.targets.is_empty());
        // adding a LAN candidate rescues it, LAN ordered first
        let rescued = advert("h", "w", 1, vec![cand(self_pub, CandKind::Public), cand([192, 168, 1, 9], CandKind::Lan)]);
        let p2 = plan_connect(&rescued, self_pub, false, true);
        assert!(!p2.dead_end);
        assert_eq!(p2.targets[0].kind, CandKind::Lan, "LAN tried first");
    }

    #[test]
    fn join_flow_phases() {
        let mut j = JoinFlow::new();
        assert_eq!(j.phase(), JoinPhase::Browsing);
        let a = advert("h", "w", 1, vec![cand([192, 168, 0, 2], CandKind::Lan)]);
        let _ = j.connect(&a, [203, 0, 113, 1], false, false);
        assert_eq!(j.phase(), JoinPhase::Connecting);
        j.on_connected();
        assert_eq!(j.phase(), JoinPhase::Connected);
        j.back_to_browse();
        assert_eq!(j.phase(), JoinPhase::Browsing);
        // failure path
        let _ = j.connect(&a, [203, 0, 113, 1], false, false);
        j.on_failed();
        assert_eq!(j.phase(), JoinPhase::Failed);
    }

    #[test]
    fn host_flow_configure_announce_live_and_advert() {
        let mut h = HostFlow::new();
        assert_eq!(h.phase(), HostPhase::Configuring);
        h.open();
        assert_eq!(h.phase(), HostPhase::Announcing);
        h.go_live();
        assert_eq!(h.phase(), HostPhase::Live);
        let adv = HostFlow::make_advert("thornwhisper", "Vale", 8, vec![cand([192, 168, 0, 5], CandKind::Lan)], 42, 777);
        assert_eq!(adv.host_username, "thornwhisper");
        assert_eq!(adv.players, 1);
        assert!(!adv.is_full());
        assert_eq!(adv.last_seen_ms, 42);
        assert_eq!(adv.world_seed, 777, "host's world seed published in the advert");
    }

    #[test]
    fn is_full_and_local_role() {
        let mut a = advert("h", "w", 1, vec![]);
        a.players = 8;
        assert!(a.is_full());
        assert_eq!(SessionRole::Local, SessionRole::Local);
    }

    #[test]
    fn join_flow_adopts_host_seed_on_connect() {
        let mut a = advert("host", "Vale", 100, vec![]);
        a.world_seed = 0xABCD_1234;
        let mut jf = JoinFlow::new();
        assert_eq!(jf.host_seed(), None, "no seed before a connect attempt");
        let _plan = jf.connect(&a, [203, 0, 113, 7], false, false);
        assert_eq!(jf.phase(), JoinPhase::Connecting);
        assert_eq!(jf.host_seed(), Some(0xABCD_1234), "joiner captures the host's seed at connect");
        jf.on_connected();
        assert_eq!(jf.phase(), JoinPhase::Connected);
        assert_eq!(
            jf.host_seed(),
            Some(0xABCD_1234),
            "seed survives to Connected so the bin can rebuild the world for it"
        );
    }

    #[test]
    fn back_to_browse_clears_adopted_seed() {
        let mut a = advert("host", "Vale", 100, vec![]);
        a.world_seed = 99;
        let mut jf = JoinFlow::new();
        jf.connect(&a, [203, 0, 113, 7], false, false);
        jf.on_failed();
        assert_eq!(jf.phase(), JoinPhase::Failed);
        jf.back_to_browse();
        assert_eq!(jf.phase(), JoinPhase::Browsing);
        assert_eq!(jf.host_seed(), None, "returning to the browser drops the stale seed");
    }
}
