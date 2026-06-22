// src/bt_respect.rs
//
// Bluetooth "system respect" layer.
//
// The principle: a music controller shouldn't permanently alter the
// user's Bluetooth landscape. If the user's Emerge01 was connected
// when the app started, leave it connected when the app exits — they
// might be using it from another app, or just want it as the
// background pair. If we connected it ourselves to route audio, we
// should disconnect it when we're done (app close, or idle timeout).
//
// State machine:
//
//   App start
//     ↓
//   Snapshot: for each paired MAC, was it Connected? (initial_connected)
//     ↓
//   During session:
//     - We issue `bluetoothctl connect <MAC>` → mark MAC as we_connected
//     - User activity (track play, volume change, page nav) → reset idle clock
//     ↓
//   App close OR idle threshold reached
//     ↓
//   For each MAC in we_connected:
//     - If initial_connected[mac] was true → leave it alone
//     - If initial_connected[mac] was false → disconnect it
//   For each MAC NOT in we_connected:
//     - Leave it alone regardless of current state
//
// The history log records every transition so the user can audit
// "what did the controller do to my BT?" via Settings.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

/// A single ledger entry — what happened, when, why. Surfaced to the
/// UI via JSON for the BT audit log.
#[derive(Debug, Clone)]
pub struct BtLogEntry {
    /// Wall-clock time at which the event occurred. Stored as
    /// "seconds since session start" so we don't depend on SystemTime
    /// for monotonic ordering; the QML side formats it relative to
    /// "now" or as a session-relative duration.
    pub at_secs_since_start: u64,
    pub mac: String,
    pub name: String,
    pub action: BtAction,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BtAction {
    /// Captured initial state at startup. `was_connected` recorded
    /// in the entry's `reason` field for human-readability.
    Snapshot,
    /// We told bluetoothctl to connect this MAC.
    ConnectAttempted,
    /// bluetoothctl reported success.
    ConnectSucceeded,
    /// bluetoothctl reported failure.
    ConnectFailed,
    /// We told bluetoothctl to disconnect this MAC.
    DisconnectAttempted,
    /// User activity reset the idle clock.
    ActivityObserved,
    /// Idle threshold reached — we may disconnect MACs we connected.
    IdleThresholdReached,
}

impl BtAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            BtAction::Snapshot              => "snapshot",
            BtAction::ConnectAttempted      => "connect_attempted",
            BtAction::ConnectSucceeded      => "connect_succeeded",
            BtAction::ConnectFailed         => "connect_failed",
            BtAction::DisconnectAttempted   => "disconnect_attempted",
            BtAction::ActivityObserved      => "activity",
            BtAction::IdleThresholdReached  => "idle_reached",
        }
    }
}

/// The respect ledger. Lives for the lifetime of the QtState; queries
/// answer "which MACs should I clean up on shutdown?".
#[derive(Debug)]
pub struct BtRespect {
    /// Session start, used to compute relative timestamps for the log.
    session_start: Instant,
    /// MACs we observed as Connected at startup. Anything in here was
    /// NOT caused by us — we leave these alone on shutdown.
    initial_connected: HashSet<String>,
    /// MACs we successfully connected during this session. On
    /// shutdown, we'll disconnect any of these that wasn't in
    /// initial_connected.
    we_connected: HashSet<String>,
    /// Friendly names per MAC, captured from bluetoothctl output or
    /// the device list. Used for the log + UI.
    names: HashMap<String, String>,
    /// Last activity timestamp (track play, volume change, page nav,
    /// any user-driven event). Used by `idle_expired`.
    last_activity: Instant,
    /// Idle threshold — when `now - last_activity` exceeds this and
    /// nothing is currently playing through us, we may disconnect
    /// owned MACs. Default 15 minutes.
    pub idle_threshold: Duration,
    /// Bounded log of state changes — used for the Settings audit.
    /// Capped at 200 entries; oldest evicted.
    log: VecDeque<BtLogEntry>,
    /// Cap on log entries to keep memory bounded.
    log_cap: usize,
    /// Was the initial snapshot ever captured? If false, on_shutdown
    /// returns an empty list — we don't disconnect things if we never
    /// recorded what was connected to begin with.
    snapshot_taken: bool,
    /// Was the system's BT adapter powered on at startup?
    /// None until snapshotted. Used to decide whether to power off
    /// on shutdown if we turned it on during the session.
    initial_bt_powered: Option<bool>,
    /// Did we issue `bluetoothctl power on` during this session?
    /// On shutdown, if initial_bt_powered == Some(false) && this is
    /// true, we restore the system by powering off.
    we_powered_on: bool,
}

impl Default for BtRespect {
    fn default() -> Self {
        Self::new()
    }
}

impl BtRespect {
    pub fn new() -> Self {
        BtRespect {
            session_start: Instant::now(),
            initial_connected: HashSet::new(),
            we_connected: HashSet::new(),
            names: HashMap::new(),
            last_activity: Instant::now(),
            idle_threshold: Duration::from_secs(15 * 60),
            log: VecDeque::new(),
            log_cap: 200,
            snapshot_taken: false,
            initial_bt_powered: None,
            we_powered_on: false,
        }
    }

    /// Record the system BT adapter's power state at startup. Called
    /// once when `bluetoothctl show` returns. None means "not yet
    /// observed".
    pub fn observe_initial_power(&mut self, powered: bool) {
        if self.initial_bt_powered.is_none() {
            self.initial_bt_powered = Some(powered);
            self.push_log(BtLogEntry {
                at_secs_since_start: self.now_secs(),
                mac: String::new(),
                name: "system".to_string(),
                action: BtAction::Snapshot,
                reason: if powered {
                    "BT adapter powered ON at startup".into()
                } else {
                    "BT adapter powered OFF at startup".into()
                },
            });
        }
    }

    /// Note that we issued `bluetoothctl power on`. Marks the
    /// "we_powered_on" flag so the cleanup path knows to restore.
    pub fn note_powered_on(&mut self) {
        self.we_powered_on = true;
        self.push_log(BtLogEntry {
            at_secs_since_start: self.now_secs(),
            mac: String::new(),
            name: "system".to_string(),
            action: BtAction::ConnectAttempted,
            reason: "controller powered BT adapter ON".into(),
        });
    }

    /// Should we power off the BT adapter on shutdown? True only if
    /// the snapshot recorded BT as OFF at startup AND we turned it on.
    pub fn should_restore_power_off(&self) -> bool {
        self.we_powered_on && self.initial_bt_powered == Some(false)
    }

    /// Did the snapshot observe BT as powered off at startup?
    /// Used by the picker UI to know whether to show the "BT off"
    /// prompt. Returns false until snapshot completes (conservative
    /// default — assume on).
    pub fn bt_was_off_at_startup(&self) -> bool {
        self.initial_bt_powered == Some(false)
    }

    /// Number of MACs observed as connected at startup.
    pub fn initial_connected_count(&self) -> usize {
        self.initial_connected.len()
    }

    /// Number of MACs we connected during this session.
    pub fn we_connected_count(&self) -> usize {
        self.we_connected.len()
    }

    /// Has the initial snapshot been captured? While false, we treat
    /// every connect as "preserved" — we don't disconnect anything on
    /// shutdown because we don't know what we'd be undoing.
    pub fn snapshot_done(&self) -> bool {
        self.snapshot_taken
    }

    /// Mark the snapshot as captured. Call after the BtPairedSnapshot
    /// worker command returns and we've fed it observations.
    pub fn mark_snapshot_done(&mut self) {
        self.snapshot_taken = true;
    }

    /// Record one MAC's initial state. `was_connected` is whether
    /// bluetoothctl reported "Connected: yes" at startup. Called once
    /// per paired MAC during the startup snapshot.
    pub fn observe_initial(&mut self, mac: &str, name: &str, was_connected: bool) {
        let mac_n = normalize(mac);
        self.names.insert(mac_n.clone(), name.to_string());
        if was_connected {
            self.initial_connected.insert(mac_n.clone());
        }
        self.push_log(BtLogEntry {
            at_secs_since_start: self.now_secs(),
            mac: mac_n,
            name: name.to_string(),
            action: BtAction::Snapshot,
            reason: if was_connected {
                "connected at startup".into()
            } else {
                "disconnected at startup".into()
            },
        });
    }

    /// Note that we issued a `bluetoothctl connect <mac>` call. The
    /// caller follows up with `note_connect_result` when the spawn
    /// finishes — we log both for the audit.
    pub fn note_connect_attempt(&mut self, mac: &str, name: &str) {
        let mac_n = normalize(mac);
        self.names.insert(mac_n.clone(), name.to_string());
        self.push_log(BtLogEntry {
            at_secs_since_start: self.now_secs(),
            mac: mac_n,
            name: name.to_string(),
            action: BtAction::ConnectAttempted,
            reason: "controller requested connect".into(),
        });
    }

    /// Record the outcome of a connect attempt. On success, the MAC
    /// joins we_connected; on failure we just log.
    pub fn note_connect_result(&mut self, mac: &str, name: &str, succeeded: bool) {
        let mac_n = normalize(mac);
        self.names.insert(mac_n.clone(), name.to_string());
        if succeeded {
            self.we_connected.insert(mac_n.clone());
        }
        self.push_log(BtLogEntry {
            at_secs_since_start: self.now_secs(),
            mac: mac_n,
            name: name.to_string(),
            action: if succeeded {
                BtAction::ConnectSucceeded
            } else {
                BtAction::ConnectFailed
            },
            reason: if succeeded {
                "bluetoothctl reported success".into()
            } else {
                "bluetoothctl reported failure".into()
            },
        });
    }

    /// Note that we issued a `bluetoothctl disconnect <mac>` call.
    pub fn note_disconnect_attempt(&mut self, mac: &str, reason: &str) {
        let mac_n = normalize(mac);
        let name = self.names.get(&mac_n).cloned().unwrap_or_default();
        // Remove from we_connected since we're undoing our work.
        self.we_connected.remove(&mac_n);
        self.push_log(BtLogEntry {
            at_secs_since_start: self.now_secs(),
            mac: mac_n,
            name,
            action: BtAction::DisconnectAttempted,
            reason: reason.to_string(),
        });
    }

    /// Reset the idle clock. Called from QtState on track play,
    /// volume change, device select, any explicit user gesture.
    pub fn note_activity(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Has the idle threshold been exceeded since the last activity?
    /// QtState polls this periodically and triggers
    /// `macs_to_disconnect_on_idle` when true.
    pub fn idle_expired(&self, now: Instant) -> bool {
        now.duration_since(self.last_activity) >= self.idle_threshold
    }

    /// MACs we should disconnect on shutdown. Subset of we_connected
    /// that wasn't in initial_connected — devices we caused to
    /// connect that weren't connected before we started.
    ///
    /// If the initial snapshot was never captured, returns empty —
    /// we don't disconnect things we don't have a baseline for.
    pub fn macs_to_disconnect_on_shutdown(&self) -> Vec<(String, String)> {
        if !self.snapshot_taken { return vec![]; }
        self.we_connected.iter()
            .filter(|m| !self.initial_connected.contains(*m))
            .map(|m| (m.clone(), self.names.get(m).cloned().unwrap_or_default()))
            .collect()
    }

    /// Same selection for idle expiry. Currently identical to the
    /// shutdown set — kept separate so we can diverge policy later
    /// (e.g. on idle, only disconnect headphones; on shutdown,
    /// everything).
    pub fn macs_to_disconnect_on_idle(&self) -> Vec<(String, String)> {
        self.macs_to_disconnect_on_shutdown()
    }

    /// Serialize the log to JSON. Newest-first for UI display.
    pub fn log_json(&self) -> String {
        let arr: Vec<serde_json::Value> = self.log.iter().rev()
            .map(|e| serde_json::json!({
                "at_secs": e.at_secs_since_start,
                "mac": e.mac,
                "name": e.name,
                "action": e.action.as_str(),
                "reason": e.reason,
            }))
            .collect();
        serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into())
    }

    /// Summary line for the Settings header: "Captured N initially
    /// connected, Y caused-by-us this session".
    pub fn summary(&self) -> String {
        if !self.snapshot_taken {
            return "snapshot pending".to_string();
        }
        format!(
            "{} connected at startup; {} caused by controller; {} eligible for cleanup",
            self.initial_connected.len(),
            self.we_connected.len(),
            self.macs_to_disconnect_on_shutdown().len(),
        )
    }

    fn now_secs(&self) -> u64 {
        self.session_start.elapsed().as_secs()
    }

    fn push_log(&mut self, e: BtLogEntry) {
        self.log.push_back(e);
        while self.log.len() > self.log_cap {
            self.log.pop_front();
        }
    }
}

/// Normalize a MAC string for comparison: uppercase, colon-separated.
/// Accepts colons or dashes as separators, any case.
fn normalize(mac: &str) -> String {
    mac.replace('-', ":").to_uppercase()
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_respect_returns_no_cleanup() {
        let r = BtRespect::new();
        assert!(r.macs_to_disconnect_on_shutdown().is_empty());
    }

    #[test]
    fn we_connected_a_fresh_device_is_eligible_for_cleanup() {
        let mut r = BtRespect::new();
        r.observe_initial("AA:BB:CC:DD:EE:FF", "Emerge", false);
        r.mark_snapshot_done();
        r.note_connect_attempt("AA:BB:CC:DD:EE:FF", "Emerge");
        r.note_connect_result("AA:BB:CC:DD:EE:FF", "Emerge", true);
        let cleanup = r.macs_to_disconnect_on_shutdown();
        assert_eq!(cleanup.len(), 1);
        assert_eq!(cleanup[0].0, "AA:BB:CC:DD:EE:FF");
    }

    #[test]
    #[allow(non_snake_case)]   // NOT is emphatic — preserves meaning
    fn we_connected_an_already_connected_device_does_NOT_get_cleaned_up() {
        // The user had Emerge connected before we started. Even though
        // we ALSO connected it (no-op essentially), we should leave it.
        let mut r = BtRespect::new();
        r.observe_initial("AA:BB:CC:DD:EE:FF", "Emerge", true);
        r.mark_snapshot_done();
        r.note_connect_attempt("AA:BB:CC:DD:EE:FF", "Emerge");
        r.note_connect_result("AA:BB:CC:DD:EE:FF", "Emerge", true);
        assert!(r.macs_to_disconnect_on_shutdown().is_empty());
    }

    #[test]
    fn we_didnt_connect_doesnt_get_cleaned() {
        let mut r = BtRespect::new();
        r.observe_initial("AA:BB:CC:DD:EE:FF", "Emerge", true);
        r.observe_initial("11:22:33:44:55:66", "Stage", false);
        r.mark_snapshot_done();
        // We didn't issue any connect calls — nothing to clean up.
        assert!(r.macs_to_disconnect_on_shutdown().is_empty());
    }

    #[test]
    fn snapshot_pending_means_no_cleanup() {
        let mut r = BtRespect::new();
        r.note_connect_attempt("AA:BB:CC:DD:EE:FF", "X");
        r.note_connect_result("AA:BB:CC:DD:EE:FF", "X", true);
        // Snapshot never marked done — we don't know baseline,
        // safer to leave everything alone.
        assert!(r.macs_to_disconnect_on_shutdown().is_empty());
    }

    #[test]
    fn idle_expiry_threshold() {
        let mut r = BtRespect::new();
        r.idle_threshold = Duration::from_secs(60);
        let later = r.last_activity + Duration::from_secs(30);
        assert!(!r.idle_expired(later));
        let way_later = r.last_activity + Duration::from_secs(120);
        assert!(r.idle_expired(way_later));
    }

    #[test]
    fn mac_normalization() {
        assert_eq!(normalize("aa-bb-cc-dd-ee-ff"), "AA:BB:CC:DD:EE:FF");
        assert_eq!(normalize("AA:BB:CC:DD:EE:FF"), "AA:BB:CC:DD:EE:FF");
        assert_eq!(normalize("aa:bb:cc:dd:ee:ff"), "AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn duplicate_observations_dont_double_count() {
        let mut r = BtRespect::new();
        r.observe_initial("AA:BB:CC:DD:EE:FF", "X", true);
        r.observe_initial("AA:BB:CC:DD:EE:FF", "X", true);
        assert_eq!(r.initial_connected_count(), 1);
    }

    #[test]
    fn explicit_disconnect_removes_from_we_connected() {
        let mut r = BtRespect::new();
        r.observe_initial("AA:BB:CC:DD:EE:FF", "X", false);
        r.mark_snapshot_done();
        r.note_connect_result("AA:BB:CC:DD:EE:FF", "X", true);
        assert_eq!(r.we_connected_count(), 1);
        r.note_disconnect_attempt("AA:BB:CC:DD:EE:FF", "user requested");
        assert_eq!(r.we_connected_count(), 0);
    }

    #[test]
    fn log_caps_at_log_cap() {
        let mut r = BtRespect::new();
        r.log_cap = 5;
        for i in 0..20 {
            r.note_connect_attempt(
                &format!("AA:BB:CC:DD:EE:{:02X}", i),
                &format!("Dev{}", i),
            );
        }
        // We pushed 20 but cap is 5 — only the 5 most recent remain.
        let log = r.log_json();
        let arr: Vec<serde_json::Value> = serde_json::from_str(&log).unwrap();
        assert_eq!(arr.len(), 5);
        // Newest-first: latest mac (EE:13) should be first.
        assert_eq!(arr[0]["mac"], "AA:BB:CC:DD:EE:13");
    }

    #[test]
    fn log_normalizes_mac_case() {
        let mut r = BtRespect::new();
        r.note_connect_attempt("aa:bb:cc:dd:ee:ff", "X");
        let log = r.log_json();
        let arr: Vec<serde_json::Value> = serde_json::from_str(&log).unwrap();
        assert_eq!(arr[0]["mac"], "AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn power_state_off_at_start_we_turned_on_should_restore() {
        let mut r = BtRespect::new();
        r.observe_initial_power(false);
        r.note_powered_on();
        assert!(r.should_restore_power_off());
        assert!(r.bt_was_off_at_startup());
    }

    #[test]
    fn power_state_on_at_start_no_restore_needed() {
        let mut r = BtRespect::new();
        r.observe_initial_power(true);
        // Even if we "turned it on" (no-op), nothing to restore.
        r.note_powered_on();
        assert!(!r.should_restore_power_off());
        assert!(!r.bt_was_off_at_startup());
    }

    #[test]
    fn power_state_off_but_we_didnt_turn_on_no_restore() {
        let mut r = BtRespect::new();
        r.observe_initial_power(false);
        assert!(!r.should_restore_power_off());
        // BT was off and we never touched it — leave it off.
    }

    #[test]
    fn power_state_unknown_until_observed() {
        let r = BtRespect::new();
        assert!(!r.bt_was_off_at_startup(),
                "default should be conservative — assume on, don't show prompt");
        assert!(!r.should_restore_power_off());
    }

    #[test]
    fn observe_initial_power_is_idempotent() {
        let mut r = BtRespect::new();
        r.observe_initial_power(false);
        // Subsequent observations should NOT overwrite — the snapshot
        // is the source of truth.
        r.observe_initial_power(true);
        assert!(r.bt_was_off_at_startup());
    }
}
