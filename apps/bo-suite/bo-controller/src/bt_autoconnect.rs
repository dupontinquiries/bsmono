// src/bt_autoconnect.rs
#![allow(dead_code)]
//
// Per-device Bluetooth auto-connect management.
//
// The user-facing promise: a per-device toggle labeled "let this
// device connect automatically". When ON, the controller periodically
// ensures the device is connected — useful for the H100 / HX
// headphones and the Beo Grace earbuds, which often need a manual
// connect step in `bluetoothctl` after a sleep.
//
// This module is the SCHEDULING brain. It tracks per-MAC config and
// backoff state, decides when to retry, and answers "which devices
// should I attempt to connect right now?". It does NOT call
// `bluetoothctl` itself — that lives in QtState, which knows about
// the spawn machinery and tokio runtime.
//
// Backoff philosophy:
//   * First attempt: try immediately when auto-connect is enabled.
//   * Each subsequent failure doubles the wait, up to 5 min cap.
//   * Success resets the backoff.
// This keeps retries quick when a device is briefly in range (e.g.
// you just walked back into the room) while preventing a hot loop
// when the device is unreachable (off, no battery, paired with another
// host).

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Backoff ladder, in seconds. Each entry is the wait BEFORE the n-th
/// retry. After exhausting the list we hold at the last value.
pub const BACKOFF_LADDER: &[u64] = &[10, 30, 60, 120, 300];

#[derive(Debug, Clone, PartialEq)]
pub enum AttemptState {
    /// Not currently attempting; eligible for the next retry as soon
    /// as `next_attempt_at` is reached.
    Idle { next_attempt_at: Instant },
    /// Caller is currently spawning/running bluetoothctl. We don't
    /// fire another attempt for this MAC until the result comes back.
    InFlight { started_at: Instant },
}

#[derive(Debug, Clone)]
pub struct DeviceConfig {
    /// User's toggle: should we manage this device automatically?
    pub auto_connect: bool,
    /// Display name for UI / logs. Filled in when the user toggles or
    /// when we discover the device.
    pub name: String,
    /// Most recent successful connect, if any. Used for diagnostics.
    pub last_success: Option<Instant>,
    /// Most recent failed attempt, if any.
    pub last_failure: Option<Instant>,
    /// Where we are in the backoff ladder. Resets on success.
    pub backoff_index: usize,
    /// Current scheduling state.
    pub state: AttemptState,
}

impl DeviceConfig {
    fn new(name: &str, now: Instant) -> Self {
        Self {
            auto_connect: false,
            name: name.to_string(),
            last_success: None,
            last_failure: None,
            backoff_index: 0,
            state: AttemptState::Idle { next_attempt_at: now },
        }
    }

    /// Compute the next attempt deadline given current backoff index.
    fn next_deadline(&self, from: Instant) -> Instant {
        let wait = BACKOFF_LADDER
            .get(self.backoff_index.min(BACKOFF_LADDER.len() - 1))
            .copied()
            .unwrap_or(*BACKOFF_LADDER.last().unwrap());
        from + Duration::from_secs(wait)
    }
}

#[derive(Debug, Default)]
pub struct BtAutoConnect {
    devices: HashMap<String, DeviceConfig>,
}

impl BtAutoConnect {
    pub fn new() -> Self { Self::default() }

    /// Get or create the config for a given MAC. Used when applying a
    /// user toggle or when we observe a device for the first time.
    pub fn ensure(&mut self, mac: &str, name: &str, now: Instant) -> &mut DeviceConfig {
        self.devices.entry(mac.to_string())
            .or_insert_with(|| DeviceConfig::new(name, now))
    }

    /// User toggled auto-connect for this MAC. When enabling, schedule
    /// an immediate first attempt (resets backoff) UNLESS an attempt
    /// is already in flight, in which case we let it complete — its
    /// `note_attempt_result` will land normally. Disabling never
    /// touches state; existing in-flight attempts complete and their
    /// results are recorded.
    ///
    /// Without the InFlight guard, re-enabling while an attempt was
    /// running would overwrite the state to Idle{now}, causing
    /// `devices_due` to fire AGAIN — yielding two concurrent
    /// `bluetoothctl connect <mac>` invocations against the same
    /// device. The second would either race-or-fail-spuriously, and
    /// the first's eventual result would land into post-reset state
    /// causing further bookkeeping drift.
    pub fn set_auto_connect(&mut self, mac: &str, name: &str, enabled: bool, now: Instant) {
        let cfg = self.ensure(mac, name, now);
        cfg.auto_connect = enabled;
        if enabled {
            cfg.backoff_index = 0;
            if !matches!(cfg.state, AttemptState::InFlight { .. }) {
                cfg.state = AttemptState::Idle { next_attempt_at: now };
            }
        }
    }

    /// Read current state for a MAC. Empty for unknown.
    pub fn get(&self, mac: &str) -> Option<&DeviceConfig> {
        self.devices.get(mac)
    }

    /// Iterate (MAC, name) for currently-enabled devices. Useful for
    /// the UI to list "managed" devices.
    pub fn enabled_devices(&self) -> Vec<(String, String)> {
        self.devices.iter()
            .filter(|(_, c)| c.auto_connect)
            .map(|(m, c)| (m.clone(), c.name.clone()))
            .collect()
    }

    /// Drop a device entirely from the table. Used when the user
    /// chooses "Forget" — corresponds to a bluetoothctl `remove`.
    /// Returns true if the device was present, false if no-op.
    pub fn forget(&mut self, mac: &str) -> bool {
        self.devices.remove(mac).is_some()
    }

    /// Merge a list of discovered (MAC, name) pairs into the table.
    /// Devices already present keep their existing config (so a
    /// re-discovery doesn't flip toggles); devices new to the table
    /// land as disabled. Returns the count of newly-added entries.
    pub fn merge_discovered(&mut self, found: &[(String, String)], now: Instant) -> usize {
        let mut added = 0;
        for (mac, name) in found {
            if self.devices.contains_key(mac) {
                // Update the display name in case it changed, but
                // leave the toggle and backoff alone.
                if let Some(cfg) = self.devices.get_mut(mac) {
                    if !name.is_empty() { cfg.name = name.clone(); }
                }
            } else {
                self.devices.insert(mac.clone(), DeviceConfig::new(name, now));
                added += 1;
            }
        }
        added
    }

    /// Which devices are due for a connect attempt right now? Returns
    /// (mac, name) tuples. Caller spawns the actual bluetoothctl
    /// invocation and reports back via `note_attempt_started` +
    /// `note_attempt_result`.
    pub fn devices_due(&self, now: Instant) -> Vec<(String, String)> {
        self.devices.iter()
            .filter(|(_, c)| c.auto_connect)
            .filter(|(_, c)| match &c.state {
                AttemptState::Idle { next_attempt_at } => now >= *next_attempt_at,
                AttemptState::InFlight { .. } => false,
            })
            .map(|(m, c)| (m.clone(), c.name.clone()))
            .collect()
    }

    /// Mark that we've kicked off a bluetoothctl invocation for this MAC.
    /// Prevents the scheduler from firing again until we report a result.
    pub fn note_attempt_started(&mut self, mac: &str, now: Instant) {
        if let Some(c) = self.devices.get_mut(mac) {
            c.state = AttemptState::InFlight { started_at: now };
        }
    }

    /// Report the outcome of an attempt. Success → reset backoff and
    /// remember the connect. Failure → advance the backoff ladder and
    /// schedule the next retry.
    pub fn note_attempt_result(&mut self, mac: &str, success: bool, now: Instant) {
        if let Some(c) = self.devices.get_mut(mac) {
            if success {
                c.last_success = Some(now);
                c.backoff_index = 0;
                // After success, schedule a "soft refresh" check
                // a long ways out — if the connection drops, we'll
                // re-attempt when devices_due fires for it.
                c.state = AttemptState::Idle {
                    next_attempt_at: now + Duration::from_secs(*BACKOFF_LADDER.last().unwrap()),
                };
            } else {
                c.last_failure = Some(now);
                // Schedule using CURRENT backoff_index — that's the
                // wait for THIS failure. Then advance.
                c.state = AttemptState::Idle {
                    next_attempt_at: c.next_deadline(now),
                };
                c.backoff_index = (c.backoff_index + 1).min(BACKOFF_LADDER.len() - 1);
            }
        }
    }

    /// Serialize to JSON: array of {mac, name, auto_connect}. Used to
    /// persist across restarts via the app_state KV.
    pub fn to_json(&self) -> String {
        let entries: Vec<serde_json::Value> = self.devices.iter()
            .map(|(mac, c)| serde_json::json!({
                "mac": mac,
                "name": c.name,
                "auto_connect": c.auto_connect,
            }))
            .collect();
        serde_json::to_string(&entries).unwrap_or_else(|_| "[]".into())
    }

    /// Restore from JSON produced by `to_json`. Resets backoff state
    /// for all loaded devices to start cleanly.
    pub fn load_json(&mut self, json: &str, now: Instant) -> usize {
        let arr: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
        let mut loaded = 0;
        for entry in arr {
            let mac = entry.get("mac").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let ac = entry.get("auto_connect").and_then(|v| v.as_bool()).unwrap_or(false);
            if mac.is_empty() { continue; }
            let cfg = DeviceConfig {
                auto_connect: ac,
                name,
                last_success: None,
                last_failure: None,
                backoff_index: 0,
                state: AttemptState::Idle { next_attempt_at: now },
            };
            self.devices.insert(mac, cfg);
            loaded += 1;
        }
        loaded
    }
}

/// Parse `bluetoothctl devices Paired` output. Each line looks like:
///   "Device AA:BB:CC:DD:EE:FF Display Name Goes Here"
/// We return (MAC, name) tuples; lines that don't match are skipped.
/// Defensive: bluetoothctl's exact line format varies slightly across
/// distros and versions, but the prefix + uppercase-hex MAC is stable.
pub fn parse_bluetoothctl_devices(output: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        let after_device = match line.strip_prefix("Device ") {
            Some(s) => s,
            None => continue,
        };
        let (mac, rest) = match after_device.split_once(' ') {
            Some((m, r)) => (m, r.trim()),
            None => continue,
        };
        if !looks_like_mac(mac) { continue; }
        if rest.is_empty() { continue; }
        out.push((mac.to_string(), rest.to_string()));
    }
    out
}

fn looks_like_mac(s: &str) -> bool {
    if s.len() != 17 { return false; }
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if i % 3 == 2 {
            if *b != b':' { return false; }
        } else {
            let c = *b as char;
            if !c.is_ascii_hexdigit() { return false; }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_has_no_devices_due() {
        let b = BtAutoConnect::new();
        assert!(b.devices_due(Instant::now()).is_empty());
    }

    #[test]
    fn enable_makes_device_due_immediately() {
        let mut b = BtAutoConnect::new();
        let now = Instant::now();
        b.set_auto_connect("AA:BB", "Grace", true, now);
        let due = b.devices_due(now);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].0, "AA:BB");
    }

    #[test]
    fn disable_removes_from_due() {
        let mut b = BtAutoConnect::new();
        let now = Instant::now();
        b.set_auto_connect("AA:BB", "Grace", true, now);
        b.set_auto_connect("AA:BB", "Grace", false, now);
        assert!(b.devices_due(now).is_empty());
    }

    #[test]
    fn in_flight_blocks_further_attempts() {
        let mut b = BtAutoConnect::new();
        let now = Instant::now();
        b.set_auto_connect("AA:BB", "Grace", true, now);
        b.note_attempt_started("AA:BB", now);
        // Even though enabled, no longer "due" because already attempting.
        assert!(b.devices_due(now + Duration::from_secs(60)).is_empty());
    }

    #[test]
    fn success_resets_backoff() {
        let mut b = BtAutoConnect::new();
        let now = Instant::now();
        b.set_auto_connect("AA:BB", "Grace", true, now);
        b.note_attempt_started("AA:BB", now);
        // Fail twice to build up backoff
        b.note_attempt_result("AA:BB", false, now);
        b.note_attempt_started("AA:BB", now + Duration::from_secs(15));
        b.note_attempt_result("AA:BB", false, now + Duration::from_secs(15));
        let bi_before = b.get("AA:BB").unwrap().backoff_index;
        assert!(bi_before > 0);
        // Success should reset
        b.note_attempt_started("AA:BB", now + Duration::from_secs(100));
        b.note_attempt_result("AA:BB", true, now + Duration::from_secs(100));
        let cfg = b.get("AA:BB").unwrap();
        assert_eq!(cfg.backoff_index, 0);
        assert!(cfg.last_success.is_some());
    }

    #[test]
    fn failure_advances_backoff() {
        let mut b = BtAutoConnect::new();
        let now = Instant::now();
        b.set_auto_connect("AA:BB", "Grace", true, now);
        for i in 0..3 {
            b.note_attempt_started("AA:BB", now + Duration::from_secs(i * 30));
            b.note_attempt_result("AA:BB", false, now + Duration::from_secs(i * 30));
        }
        let cfg = b.get("AA:BB").unwrap();
        assert_eq!(cfg.backoff_index, 3);
    }

    #[test]
    fn backoff_caps_at_ladder_end() {
        let mut b = BtAutoConnect::new();
        let now = Instant::now();
        b.set_auto_connect("AA:BB", "Grace", true, now);
        // 100 failures
        for i in 0..100 {
            b.note_attempt_started("AA:BB", now + Duration::from_secs(i * 30));
            b.note_attempt_result("AA:BB", false, now + Duration::from_secs(i * 30));
        }
        let cfg = b.get("AA:BB").unwrap();
        assert_eq!(cfg.backoff_index, BACKOFF_LADDER.len() - 1);
    }

    #[test]
    fn json_roundtrip_preserves_enable_state() {
        let mut b = BtAutoConnect::new();
        let now = Instant::now();
        b.set_auto_connect("AA:BB", "Grace", true, now);
        b.set_auto_connect("CC:DD", "H100", false, now);
        let j = b.to_json();
        let mut b2 = BtAutoConnect::new();
        let loaded = b2.load_json(&j, now);
        assert_eq!(loaded, 2);
        assert!(b2.get("AA:BB").unwrap().auto_connect);
        assert!(!b2.get("CC:DD").unwrap().auto_connect);
        assert_eq!(b2.get("AA:BB").unwrap().name, "Grace");
    }

    #[test]
    fn disabled_devices_never_due() {
        let mut b = BtAutoConnect::new();
        let now = Instant::now();
        b.set_auto_connect("AA:BB", "Grace", false, now);
        // Even after a long time, never due if disabled.
        assert!(b.devices_due(now + Duration::from_secs(86400)).is_empty());
    }

    #[test]
    fn enabled_devices_listed() {
        let mut b = BtAutoConnect::new();
        let now = Instant::now();
        b.set_auto_connect("AA:BB", "Grace", true, now);
        b.set_auto_connect("CC:DD", "H100", false, now);
        let enabled = b.enabled_devices();
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].0, "AA:BB");
    }

    #[test]
    fn second_due_after_backoff_elapses() {
        let mut b = BtAutoConnect::new();
        let now = Instant::now();
        b.set_auto_connect("AA:BB", "Grace", true, now);
        b.note_attempt_started("AA:BB", now);
        b.note_attempt_result("AA:BB", false, now);
        // Within backoff window: not due
        assert!(b.devices_due(now + Duration::from_secs(5)).is_empty());
        // After ladder[0]=10s: due
        let later = now + Duration::from_secs(BACKOFF_LADDER[0] + 1);
        assert_eq!(b.devices_due(later).len(), 1);
    }

    // ── parse_bluetoothctl_devices tests ──────────────────────────────

    #[test]
    fn parse_typical_bluetoothctl_output() {
        let output = "\
Device AA:BB:CC:DD:EE:FF Beo Grace
Device 11:22:33:44:55:66 Beoplay H100
Device aa:bb:cc:dd:ee:00 lowercase mac";
        let parsed = parse_bluetoothctl_devices(output);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].0, "AA:BB:CC:DD:EE:FF");
        assert_eq!(parsed[0].1, "Beo Grace");
        assert_eq!(parsed[1].0, "11:22:33:44:55:66");
        assert_eq!(parsed[1].1, "Beoplay H100");
        // Lowercase MAC accepted
        assert_eq!(parsed[2].0, "aa:bb:cc:dd:ee:00");
    }

    #[test]
    fn parse_skips_non_device_lines() {
        let output = "\
[bluetoothctl]# devices Paired
Device AA:BB:CC:DD:EE:FF Real Device
Some other line
NOT-A-DEVICE
Device 11:22:33:44:55:66 Another One";
        let parsed = parse_bluetoothctl_devices(output);
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn parse_rejects_malformed_mac() {
        let output = "\
Device GG:HH:II:JJ:KK:LL Bad MAC (non-hex)
Device 11:22:33:44:55 Too Short
Device AA:BB:CC:DD:EE:FF Good One";
        let parsed = parse_bluetoothctl_devices(output);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn parse_handles_empty_output() {
        assert_eq!(parse_bluetoothctl_devices("").len(), 0);
        assert_eq!(parse_bluetoothctl_devices("\n\n\n").len(), 0);
    }

    #[test]
    fn parse_skips_mac_with_no_name() {
        // bluetoothctl always includes a name; if it doesn't we skip.
        let output = "Device AA:BB:CC:DD:EE:FF";
        let parsed = parse_bluetoothctl_devices(output);
        assert_eq!(parsed.len(), 0);
    }

    #[test]
    fn merge_discovered_adds_new_keeps_existing() {
        let mut b = BtAutoConnect::new();
        let now = Instant::now();
        // Pre-existing enabled device
        b.set_auto_connect("AA:BB:CC:DD:EE:FF", "Grace", true, now);

        let discovered = vec![
            ("AA:BB:CC:DD:EE:FF".to_string(), "Grace (renamed)".to_string()),
            ("11:22:33:44:55:66".to_string(), "New Device".to_string()),
        ];
        let added = b.merge_discovered(&discovered, now);
        assert_eq!(added, 1);
        // Existing one kept its toggle but got the new name
        let existing = b.get("AA:BB:CC:DD:EE:FF").unwrap();
        assert!(existing.auto_connect);
        assert_eq!(existing.name, "Grace (renamed)");
        // New one is present but disabled
        let new_dev = b.get("11:22:33:44:55:66").unwrap();
        assert!(!new_dev.auto_connect);
        assert_eq!(new_dev.name, "New Device");
    }

    #[test]
    fn forget_drops_device() {
        let mut b = BtAutoConnect::new();
        let now = Instant::now();
        b.set_auto_connect("AA:BB:CC:DD:EE:FF", "Grace", true, now);
        assert!(b.get("AA:BB:CC:DD:EE:FF").is_some());
        assert!(b.forget("AA:BB:CC:DD:EE:FF"));
        assert!(b.get("AA:BB:CC:DD:EE:FF").is_none());
        // Second forget of same MAC is a no-op (returns false)
        assert!(!b.forget("AA:BB:CC:DD:EE:FF"));
    }

    /// Regression: re-enabling auto_connect while an attempt was
    /// already in-flight used to overwrite the state to Idle{now},
    /// which would cause `devices_due` to fire again on the next tick
    /// and launch a SECOND parallel bluetoothctl connect against the
    /// same MAC. The fix is to skip the state-reset when we're
    /// already InFlight.
    #[test]
    fn reenable_during_inflight_does_not_double_attempt() {
        let mut b = BtAutoConnect::new();
        let t0 = Instant::now();
        b.set_auto_connect("AA:BB", "Grace", true, t0);

        // Take the first attempt off the queue.
        let due = b.devices_due(t0);
        assert_eq!(due.len(), 1, "expected initial due");
        b.note_attempt_started("AA:BB", t0);

        // While the attempt is in-flight, user toggles off then on.
        let t_mid = t0 + Duration::from_millis(500);
        b.set_auto_connect("AA:BB", "Grace", false, t_mid);
        b.set_auto_connect("AA:BB", "Grace", true, t_mid);

        // Crucially: the device should NOT be due again. The original
        // attempt is still running; its result hasn't landed.
        let due_again = b.devices_due(t_mid + Duration::from_millis(1));
        assert!(due_again.is_empty(),
            "no double-attempt: original attempt still in-flight \
             but got due list = {:?}", due_again);

        // When the original attempt completes (say, succeeds), state
        // resets correctly.
        b.note_attempt_result("AA:BB", true, t_mid + Duration::from_secs(2));
        let cfg = b.get("AA:BB").unwrap();
        assert_eq!(cfg.backoff_index, 0);
        // Long pause before next refresh check — success scheduled
        // at last-ladder-step out.
        match &cfg.state {
            AttemptState::Idle { next_attempt_at } => {
                let wait = *next_attempt_at - (t_mid + Duration::from_secs(2));
                assert!(wait >= Duration::from_secs(60),
                    "success → long refresh wait, got {:?}", wait);
            }
            other => panic!("expected Idle after success, got {:?}", other),
        }
    }
}
