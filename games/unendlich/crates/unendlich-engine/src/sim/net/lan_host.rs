//! `sim::net::lan_host` — the host-side **peer table** for LAN multiplayer.
//!
//! `player_roster::Roster` answers "who is in the world" (slots, roles,
//! connection state) but deliberately knows nothing about transport. The
//! fullgame's inline LAN loop, meanwhile, tracked exactly one
//! `Option<SocketAddrV4>` peer — so a second joiner silently *replaced* the
//! first, and the advertised `max_players: 8` was a fiction. This module is the
//! missing layer between them: an address-keyed table that composes the Roster
//! with the transport facts it doesn't hold —
//!
//! - **which UDP address** each player is reachable at,
//! - **which entity id** the host assigned them (see below),
//! - **when they were last heard** (liveness / prune),
//! - **who a datagram fans out to** (everyone but its source — the relay rule
//!   that lets joiner A see joiner B through the host).
//!
//! ### Why the host assigns entity ids
//! The legacy scheme used each process's own UDP session port as its entity
//! id. Both processes prefer the same fixed port (9878), so two joiners on
//! different machines collide on id 9878 and the host merges them into one
//! avatar. Host-assigned ids start at [`ASSIGNED_EID_BASE`] (far above any
//! u16 port), so assigned and legacy port-derived ids can never collide, and
//! each admission gets a fresh id. A peer that rejoins from the same address
//! reclaims its previous id (no ghost avatar on reconnect).
//!
//! Pure and deterministic: time is passed in as `now_ms`, no sockets, no
//! clock. The IO shell that drives this lives in [`super::lan_host_pump`].

use std::net::SocketAddrV4;

use super::player_roster::{ConnState, JoinError, Role, Roster};

/// Host-assigned entity ids start here — far above any UDP-port-derived id
/// (`< 65536`) a legacy peer self-assigns, so the two schemes cannot collide.
pub const ASSIGNED_EID_BASE: u64 = 1 << 32;

/// A live, reachable peer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanPeer {
    pub addr: SocketAddrV4,
    pub entity_id: u64,
    pub last_seen_ms: u64,
}

/// Why an admission was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DenyReason {
    /// The caller's access policy (whitelist) said no.
    NotAllowed,
    /// Roster has no free slot (host + peers at capacity).
    Full,
}

/// Admission outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admit {
    Accepted {
        /// The id this peer must stamp on its outbound entity states.
        entity_id: u64,
        /// True when this address was here before (reconnect) — same id.
        rejoined: bool,
    },
    Denied(DenyReason),
}

/// Address-keyed peer table over the world Roster. The host itself occupies a
/// roster slot (Role::Host) but has no peer entry — it isn't reachable via
/// this table and is never a fanout target.
#[derive(Clone, Debug)]
pub struct LanHost {
    roster: Roster,
    /// Live peers (reachable now). Parallel to Active roster entries.
    peers: Vec<LanPeer>,
    /// Every address ever admitted this session → its entity id, kept across
    /// prune so a rejoin from the same address reclaims the same id.
    known: Vec<(SocketAddrV4, u64)>,
    next_eid: u64,
}

impl LanHost {
    /// A fresh host table. The host joins its own roster immediately (name +
    /// its own entity id — the port-derived one it stamps on outbound states).
    pub fn new(host_name: &str, host_entity_id: u64) -> Self {
        let mut roster = Roster::new();
        // Roster::new() has MAX_PLAYERS free slots; the host takes the first.
        let _ = roster.join(host_entity_id, host_name, Role::Host);
        let _ = roster.set_state(host_entity_id, ConnState::Active);
        LanHost { roster, peers: Vec::new(), known: Vec::new(), next_eid: ASSIGNED_EID_BASE }
    }

    /// Handle a JOIN from `addr`. `allowed` is the caller's access-policy
    /// verdict (e.g. `whitelist.admits(name)`) — policy stays at the call
    /// site so this table works for open servers, closed servers, and tests
    /// alike. Idempotent per address: a repeat JOIN from a live peer (their
    /// Accept datagram was lost) re-accepts with the same id.
    pub fn admit(&mut self, name: &str, addr: SocketAddrV4, allowed: bool, now_ms: u64) -> Admit {
        if !allowed {
            return Admit::Denied(DenyReason::NotAllowed);
        }
        // Already live at this address → same id (lost-Accept retry).
        if let Some(p) = self.peers.iter_mut().find(|p| p.addr == addr) {
            p.last_seen_ms = now_ms;
            return Admit::Accepted { entity_id: p.entity_id, rejoined: true };
        }
        // Known from earlier this session → reclaim the id, reconnect the slot.
        if let Some(&(_, eid)) = self.known.iter().find(|(a, _)| *a == addr) {
            self.roster.reconnect(eid);
            let _ = self.roster.set_state(eid, ConnState::Active);
            self.peers.push(LanPeer { addr, entity_id: eid, last_seen_ms: now_ms });
            return Admit::Accepted { entity_id: eid, rejoined: true };
        }
        // New peer → fresh id + roster slot.
        let eid = self.next_eid;
        let mut joined = self.roster.join(eid, name, Role::Player);
        if joined == Err(JoinError::RosterFull) {
            // Full of ghosts? Slots held for reconnect (Disconnected) must not
            // permanently lock a playtest server out of admitting a NEW
            // machine. Evict one disconnected reservation to make room; a
            // live-full roster still denies.
            if let Some(ghost) = self
                .known
                .iter()
                .map(|&(_, id)| id)
                .find(|id| !self.peers.iter().any(|p| p.entity_id == *id))
            {
                self.roster.leave(ghost);
                self.known.retain(|&(_, id)| id != ghost);
                joined = self.roster.join(eid, name, Role::Player);
            }
        }
        match joined {
            Ok(_) => {
                self.next_eid += 1;
                let _ = self.roster.set_state(eid, ConnState::Active);
                self.peers.push(LanPeer { addr, entity_id: eid, last_seen_ms: now_ms });
                self.known.push((addr, eid));
                Admit::Accepted { entity_id: eid, rejoined: false }
            }
            Err(JoinError::RosterFull) => Admit::Denied(DenyReason::Full),
            // Unreachable with monotonically fresh ids, but never panic.
            Err(JoinError::DuplicateId) => Admit::Denied(DenyReason::Full),
        }
    }

    /// Record traffic from `addr`. Returns true when it belongs to a live peer.
    pub fn note_seen(&mut self, addr: SocketAddrV4, now_ms: u64) -> bool {
        match self.peers.iter_mut().find(|p| p.addr == addr) {
            Some(p) => {
                p.last_seen_ms = now_ms;
                true
            }
            None => false,
        }
    }

    /// Drop peers not heard from within `ttl_ms`. Their roster slot flips to
    /// Disconnected (held for reconnect — same address reclaims the same id).
    /// Returns the removed peers' entity ids so the embedder can clear avatars.
    pub fn prune(&mut self, now_ms: u64, ttl_ms: u64) -> Vec<u64> {
        let mut dropped = Vec::new();
        self.peers.retain(|p| {
            let live = now_ms.saturating_sub(p.last_seen_ms) <= ttl_ms;
            if !live {
                dropped.push(p.entity_id);
            }
            live
        });
        for &eid in &dropped {
            self.roster.disconnect(eid);
        }
        dropped
    }

    /// Everyone a datagram should be sent to: all live peers except `except`
    /// (its source, when relaying). The host is never a target.
    pub fn fanout(&self, except: Option<SocketAddrV4>) -> Vec<SocketAddrV4> {
        self.peers
            .iter()
            .filter(|p| Some(p.addr) != except)
            .map(|p| p.addr)
            .collect()
    }

    /// Live (reachable-now) peer count, excluding the host.
    pub fn live_peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Everyone in the world right now: host + live peers.
    pub fn player_count(&self) -> usize {
        1 + self.peers.len()
    }

    /// The entity id assigned to `addr`, if it's a live peer.
    pub fn eid_of(&self, addr: SocketAddrV4) -> Option<u64> {
        self.peers.iter().find(|p| p.addr == addr).map(|p| p.entity_id)
    }

    /// Names of everyone Active in the roster (host first by slot order) —
    /// feeds the in-game presence list and join logs.
    pub fn active_names(&self) -> Vec<String> {
        self.roster.active().map(|e| e.name.clone()).collect()
    }

    /// Read access to the underlying roster (roles, slots, states).
    pub fn roster(&self) -> &Roster {
        &self.roster
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn a(last: u8, port: u16) -> SocketAddrV4 {
        SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, last), port)
    }

    #[test]
    fn admits_assign_unique_ids_above_the_port_range() {
        let mut h = LanHost::new("host", 9878);
        // The exact legacy failure: two machines, both on the preferred port.
        let x = h.admit("alice", a(10, 9878), true, 0);
        let y = h.admit("bob", a(11, 9878), true, 0);
        let (ex, ey) = match (x, y) {
            (Admit::Accepted { entity_id: ex, .. }, Admit::Accepted { entity_id: ey, .. }) => (ex, ey),
            other => panic!("expected two accepts, got {other:?}"),
        };
        assert_ne!(ex, ey, "same preferred port must not collide ids");
        assert!(ex >= ASSIGNED_EID_BASE && ey >= ASSIGNED_EID_BASE);
        assert_eq!(h.player_count(), 3); // host + 2
    }

    #[test]
    fn policy_denial_changes_nothing() {
        let mut h = LanHost::new("host", 1);
        assert_eq!(h.admit("mallory", a(66, 9878), false, 0), Admit::Denied(DenyReason::NotAllowed));
        assert_eq!(h.live_peer_count(), 0);
        assert_eq!(h.player_count(), 1);
    }

    #[test]
    fn capacity_denies_past_max_players() {
        let mut h = LanHost::new("host", 1);
        let cap = super::super::player_roster::MAX_PLAYERS - 1; // host holds a slot
        for i in 0..cap {
            match h.admit("p", a(20 + i as u8, 9878), true, 0) {
                Admit::Accepted { .. } => {}
                other => panic!("peer {i} should fit, got {other:?}"),
            }
        }
        assert_eq!(h.admit("late", a(99, 9878), true, 0), Admit::Denied(DenyReason::Full));
    }

    #[test]
    fn repeat_join_from_a_live_peer_reaccepts_with_the_same_id() {
        // A lost Accept makes the joiner retry; that must not mint a new id.
        let mut h = LanHost::new("host", 1);
        let e1 = match h.admit("alice", a(10, 9878), true, 0) {
            Admit::Accepted { entity_id, rejoined } => {
                assert!(!rejoined);
                entity_id
            }
            other => panic!("{other:?}"),
        };
        match h.admit("alice", a(10, 9878), true, 500) {
            Admit::Accepted { entity_id, rejoined } => {
                assert_eq!(entity_id, e1);
                assert!(rejoined);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(h.live_peer_count(), 1);
    }

    #[test]
    fn prune_drops_silent_peers_and_rejoin_reclaims_the_id() {
        let mut h = LanHost::new("host", 1);
        let e1 = match h.admit("alice", a(10, 9878), true, 0) {
            Admit::Accepted { entity_id, .. } => entity_id,
            other => panic!("{other:?}"),
        };
        h.note_seen(a(10, 9878), 1000);
        assert_eq!(h.prune(2000, 5000), Vec::<u64>::new()); // within ttl
        assert_eq!(h.prune(7001, 5000), vec![e1]); // silent past ttl
        assert_eq!(h.live_peer_count(), 0);
        assert!(h.fanout(None).is_empty());
        // Rejoin from the same machine: same entity id, no ghost.
        match h.admit("alice", a(10, 9878), true, 8000) {
            Admit::Accepted { entity_id, rejoined } => {
                assert_eq!(entity_id, e1);
                assert!(rejoined);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(h.live_peer_count(), 1);
    }

    #[test]
    fn fanout_excludes_the_source_and_the_host() {
        let mut h = LanHost::new("host", 1);
        h.admit("a", a(10, 9878), true, 0);
        h.admit("b", a(11, 9878), true, 0);
        h.admit("c", a(12, 9878), true, 0);
        let f = h.fanout(Some(a(11, 9878)));
        assert_eq!(f.len(), 2);
        assert!(f.contains(&a(10, 9878)) && f.contains(&a(12, 9878)));
        assert!(!f.contains(&a(11, 9878)));
        // Broadcast (no source) reaches everyone.
        assert_eq!(h.fanout(None).len(), 3);
    }

    #[test]
    fn a_ghost_full_roster_still_admits_a_new_machine() {
        let mut h = LanHost::new("host", 1);
        let cap = super::super::player_roster::MAX_PLAYERS - 1;
        for i in 0..cap {
            h.admit("p", a(20 + i as u8, 9878), true, 0);
        }
        // Everyone times out — slots are now reconnect reservations.
        assert_eq!(h.prune(10_000, 5000).len(), cap);
        // A brand-new machine must still get in (one ghost evicted).
        match h.admit("fresh", a(99, 9878), true, 11_000) {
            Admit::Accepted { rejoined, .. } => assert!(!rejoined),
            other => panic!("ghost-full roster locked out a new joiner: {other:?}"),
        }
        // But a LIVE-full roster still denies.
        for i in 0..cap {
            h.admit("p", a(120 + i as u8, 9878), true, 12_000);
        }
        assert_eq!(h.admit("late", a(200, 9878), true, 12_000), Admit::Denied(DenyReason::Full));
    }

    #[test]
    fn note_seen_only_recognizes_live_peers() {
        let mut h = LanHost::new("host", 1);
        assert!(!h.note_seen(a(10, 9878), 0));
        h.admit("a", a(10, 9878), true, 0);
        assert!(h.note_seen(a(10, 9878), 10));
    }

    #[test]
    fn presence_names_track_joins_and_prunes() {
        let mut h = LanHost::new("kessler", 1);
        h.admit("alice", a(10, 9878), true, 0);
        h.admit("bob", a(11, 9878), true, 0);
        assert_eq!(h.active_names(), vec!["kessler", "alice", "bob"]);
        h.prune(10_000, 5000);
        assert_eq!(h.active_names(), vec!["kessler"]);
    }
}
