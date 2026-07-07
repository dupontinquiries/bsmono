//! `sim::world_management` — the pure world-management settings model and the
//! **Open-to-DHT** host-lifecycle decision (WID r335-worldmgmt).
//!
//! The multiplayer pure layer already exists (`session_mode` roles/advert/flows,
//! `net::{identity, dht_mailbox, ...}`). The operator wants a world-management
//! settings page whose **"Open to DHT"** toggle flips a running single-player
//! world into a host: DHT stays off in SP and turns on when hosting
//! (`MULTIPLAYER_WORLD_JOIN §3`). This module owns the *settings* + the *decision*
//! of what transition a toggle implies, and **builds the advert from the
//! settings** via `session_mode::HostFlow::make_advert`. It does **not** do the
//! DHT bootstrap/announce — that IO is bin glue (see `HANDOFF_worldmgmt.md`).
//!
//! Pure: validated data + a transition function. No IO/clock/RNG, no
//! `unwrap`/`expect`/`panic` in non-test code. The two things `make_advert` needs
//! that are inherently bin-owned — the NAT-discovered `candidates` and the wall
//! clock `now_ms` — are **passed in** per SHARED_RULES §1, not sourced here.

use crate::sim::net::identity::UnendlichId;
use crate::sim::net_candidates::Candidate;
use crate::sim::session_mode::{HostFlow, SessionRole, WorldAdvert};

/// Upper bound on a hosted world's player count (small co-op). `set_max_players`
/// clamps into `[1, MAX_PLAYERS]` — a host always seats at least itself.
pub const MAX_PLAYERS: u8 = 16;
/// Default seats for a freshly-opened world.
pub const DEFAULT_MAX_PLAYERS: u8 = 8;

/// World-management settings surfaced on the management page. Whitelisting is on
/// by default (matches `net::identity`'s "whitelist on by default"). Mutate only
/// through the validated setters so invariants hold (1..=MAX_PLAYERS players,
/// non-empty name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldMgmtSettings {
    open_to_dht: bool,
    whitelist_on: bool,
    max_players: u8,
    world_name: String,
}

impl Default for WorldMgmtSettings {
    fn default() -> Self {
        WorldMgmtSettings {
            open_to_dht: false, // DHT off in single-player
            whitelist_on: true, // on by default
            max_players: DEFAULT_MAX_PLAYERS,
            world_name: String::from("World"),
        }
    }
}

impl WorldMgmtSettings {
    /// New settings for a named world. A blank/whitespace-only name is rejected
    /// and the default name kept; `name_valid()` reflects whether `name` stuck.
    pub fn new(world_name: &str) -> Self {
        let mut s = Self::default();
        s.set_world_name(world_name);
        s
    }

    pub fn open_to_dht(&self) -> bool {
        self.open_to_dht
    }
    pub fn whitelist_on(&self) -> bool {
        self.whitelist_on
    }
    pub fn max_players(&self) -> u8 {
        self.max_players
    }
    pub fn world_name(&self) -> &str {
        &self.world_name
    }

    /// Set the Open-to-DHT toggle. (The *transition* it implies is decided by
    /// [`apply_open_to_dht`]; this only records intent.)
    pub fn set_open_to_dht(&mut self, on: bool) {
        self.open_to_dht = on;
    }

    /// Toggle whitelisting (id-gated joins). On by default.
    pub fn set_whitelist_on(&mut self, on: bool) {
        self.whitelist_on = on;
    }

    /// Clamp + store the seat count into `[1, MAX_PLAYERS]`; returns the stored
    /// value. `0` becomes `1` (a host seats itself); over-cap saturates.
    pub fn set_max_players(&mut self, n: u8) -> u8 {
        self.max_players = n.clamp(1, MAX_PLAYERS);
        self.max_players
    }

    /// Set the world name iff it is non-empty after trimming; returns whether it
    /// was accepted (an invalid name leaves the previous one intact).
    pub fn set_world_name(&mut self, name: &str) -> bool {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return false;
        }
        self.world_name = trimmed.to_string();
        true
    }

    /// Whether the current name is non-empty (always true after construction;
    /// useful as a page-validity check).
    pub fn name_valid(&self) -> bool {
        !self.world_name.trim().is_empty()
    }
}

/// The host-lifecycle transition an Open-to-DHT change implies.
#[derive(Clone, Debug, PartialEq)]
pub enum HostTransition {
    /// No host-lifecycle change: keep the current role. Either already-Local with
    /// DHT off, or already-Host with DHT on ("already-host stays"), or a Client
    /// (which cannot open its own world to the DHT).
    StayLocal,
    /// Begin hosting: flip the Local world to a host and announce this advert
    /// (the bin does the DHT bootstrap + announce; the advert is built here).
    GoHost(WorldAdvert),
    /// Stop hosting and return the world to a local session.
    CloseHost,
}

/// Decide the host-lifecycle transition for the current `open_to_dht` setting,
/// given the running `role`.
///
/// - `open_to_dht == true` on [`SessionRole::Local`] → [`HostTransition::GoHost`]
///   carrying a `session_mode`-built [`WorldAdvert`] (host = `self_id`, name +
///   seats from `settings`, host's NAT `candidates` and `now_ms` passed in).
/// - `open_to_dht == false` on [`SessionRole::Host`] → [`HostTransition::CloseHost`].
/// - already in the target state (Host+on, Local+off) or a Client →
///   [`HostTransition::StayLocal`] (no transition).
///
/// `candidates` (the host's reachable endpoints from NAT discovery) and `now_ms`
/// (wall clock) are **bin-owned** and passed in — this stays pure. Deterministic
/// in its inputs.
pub fn apply_open_to_dht(
    settings: &WorldMgmtSettings,
    self_id: &UnendlichId,
    role: SessionRole,
    candidates: Vec<Candidate>,
    now_ms: u64,
    world_seed: u64,
) -> HostTransition {
    if settings.open_to_dht {
        match role {
            SessionRole::Local => {
                let advert = HostFlow::make_advert(
                    self_id.as_str(),
                    &settings.world_name,
                    settings.max_players as u16,
                    candidates,
                    now_ms,
                    world_seed,
                );
                HostTransition::GoHost(advert)
            }
            // already hosting, or a client that can't host its own world.
            SessionRole::Host | SessionRole::Client => HostTransition::StayLocal,
        }
    } else {
        match role {
            SessionRole::Host => HostTransition::CloseHost,
            SessionRole::Local | SessionRole::Client => HostTransition::StayLocal,
        }
    }
}

/// A field on the management page, for the apply-classification below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MgmtField {
    OpenToDht,
    WhitelistOn,
    MaxPlayers,
    WorldName,
}

/// How a management-field change takes effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MgmtApplyKind {
    /// Applies in place without a session change (whitelist, seats, name).
    Live,
    /// Changes the session lifecycle (start/stop hosting) — the bin runs the DHT
    /// bootstrap/announce or teardown. Only `OpenToDht`.
    SessionLifecycle,
}

/// Classify how each field applies. `OpenToDht` is a session-lifecycle change
/// (it starts or stops hosting); everything else is live. Grounds the brief's
/// "toggling DHT is a session lifecycle change (not live render) — flag it".
pub fn apply_kind(field: MgmtField) -> MgmtApplyKind {
    match field {
        MgmtField::OpenToDht => MgmtApplyKind::SessionLifecycle,
        MgmtField::WhitelistOn | MgmtField::MaxPlayers | MgmtField::WorldName => MgmtApplyKind::Live,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::net_candidates::CandKind;

    fn cand() -> Candidate {
        Candidate { host: [192, 168, 0, 5], port: 7777, kind: CandKind::Lan }
    }
    fn id() -> UnendlichId {
        UnendlichId::new("THORNWHISPER1") // normalizes to 12+ alnum chars
    }

    #[test]
    fn open_on_local_goes_host_with_well_formed_advert() {
        let mut s = WorldMgmtSettings::new("Verdant Vale");
        s.set_open_to_dht(true);
        s.set_max_players(8);
        let t = apply_open_to_dht(&s, &id(), SessionRole::Local, vec![cand()], 42_000, 4242);
        match t {
            HostTransition::GoHost(a) => {
                assert_eq!(a.host_username, id().as_str(), "host = self id");
                assert_eq!(a.world_name, "Verdant Vale");
                assert_eq!(a.max_players, 8);
                assert_eq!(a.players, 1, "host counts as the first player");
                assert_eq!(a.last_seen_ms, 42_000, "advert stamped with passed-in clock");
                assert_eq!(a.candidates, vec![cand()], "host's candidates carried");
                assert_eq!(a.world_seed, 4242, "host's world seed carried in advert");
                assert!(!a.is_full(), "1/8 is not full");
            }
            other => panic!("expected GoHost, got {:?}", other),
        }
    }

    #[test]
    fn close_when_dht_off_while_hosting() {
        let mut s = WorldMgmtSettings::new("W");
        s.set_open_to_dht(false);
        assert_eq!(
            apply_open_to_dht(&s, &id(), SessionRole::Host, vec![], 0, 0),
            HostTransition::CloseHost
        );
    }

    #[test]
    fn already_host_stays_when_dht_on() {
        let mut s = WorldMgmtSettings::new("W");
        s.set_open_to_dht(true);
        // already hosting → no re-GoHost, no teardown.
        assert_eq!(
            apply_open_to_dht(&s, &id(), SessionRole::Host, vec![cand()], 5, 0),
            HostTransition::StayLocal
        );
    }

    #[test]
    fn local_with_dht_off_and_client_are_noops() {
        let mut s = WorldMgmtSettings::new("W");
        s.set_open_to_dht(false);
        assert_eq!(apply_open_to_dht(&s, &id(), SessionRole::Local, vec![], 0, 0), HostTransition::StayLocal);
        // a client can't open its own world to the DHT, either way.
        s.set_open_to_dht(true);
        assert_eq!(apply_open_to_dht(&s, &id(), SessionRole::Client, vec![cand()], 0, 0), HostTransition::StayLocal);
        s.set_open_to_dht(false);
        assert_eq!(apply_open_to_dht(&s, &id(), SessionRole::Client, vec![], 0, 0), HostTransition::StayLocal);
    }

    #[test]
    fn whitelist_defaults_on() {
        assert!(WorldMgmtSettings::default().whitelist_on());
        assert!(WorldMgmtSettings::new("Anything").whitelist_on());
    }

    #[test]
    fn max_players_clamps() {
        let mut s = WorldMgmtSettings::default();
        assert_eq!(s.set_max_players(0), 1, "0 seats → 1 (host seats itself)");
        assert_eq!(s.set_max_players(99), MAX_PLAYERS, "over-cap saturates");
        assert_eq!(s.set_max_players(4), 4, "in-range preserved");
        assert_eq!(s.max_players(), 4);
    }

    #[test]
    fn world_name_validation() {
        let mut s = WorldMgmtSettings::new("Start");
        assert!(!s.set_world_name(""), "empty rejected");
        assert!(!s.set_world_name("   "), "whitespace-only rejected");
        assert_eq!(s.world_name(), "Start", "rejected name leaves previous intact");
        assert!(s.set_world_name("  Trimmed Keep  "), "valid accepted");
        assert_eq!(s.world_name(), "Trimmed Keep", "trimmed on store");
        assert!(s.name_valid());
    }

    #[test]
    fn apply_kind_flags_dht_as_session_lifecycle() {
        assert_eq!(apply_kind(MgmtField::OpenToDht), MgmtApplyKind::SessionLifecycle);
        for f in [MgmtField::WhitelistOn, MgmtField::MaxPlayers, MgmtField::WorldName] {
            assert_eq!(apply_kind(f), MgmtApplyKind::Live, "{:?} should be live", f);
        }
    }

    #[test]
    fn deterministic() {
        let mut s = WorldMgmtSettings::new("Vale");
        s.set_open_to_dht(true);
        let a = apply_open_to_dht(&s, &id(), SessionRole::Local, vec![cand()], 7, 0);
        let b = apply_open_to_dht(&s, &id(), SessionRole::Local, vec![cand()], 7, 0);
        assert_eq!(a, b);
    }
}
