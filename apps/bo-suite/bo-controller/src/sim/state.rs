//! `ControllerState` — pure model of what the controller currently
//! believes is true about the world. Mirrors the QtState fields the
//! routing logic actually cares about (active_url, current_driver_kind,
//! active_bt_mac, active_multiroom, volume, sink_unreachable_strikes,
//! the BT respect ledger).
//!
//! Field naming intentionally mirrors `QtState` so a future refactor
//! can pull the routing decisions out of main.rs and reuse this
//! struct verbatim.

use std::collections::{BTreeMap, BTreeSet};

/// Driver kind — same five values as `current_driver_kind` on QtState.
/// Using an enum here so transitions can't accidentally land on an
/// unknown string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverKind {
    /// No device selected, no sink chosen — passthrough silence.
    Noop,
    /// Device is Mozart-active: routing happens via the device's REST
    /// API (play_uri, SetVolume, etc.). No PulseAudio involvement.
    Mozart,
    /// Single Bluetooth sink: paplay to a single `bluez_sink.<MAC>...`.
    Bluetooth,
    /// Local default sink — paplay to the controller's own speakers.
    Local,
    /// Multi-room: PulseAudio combine-sink fanning out to ≥2 BT
    /// speakers.
    MultiRoom,
}

impl DriverKind {
    /// Volume changes route through the device's REST API for Mozart
    /// (and for the noop kind, where there's no PA sink to talk to);
    /// everything else uses pactl on the local sink.
    pub fn volume_uses_rest(self) -> bool {
        matches!(self, DriverKind::Mozart | DriverKind::Noop)
    }
    /// Track playback for non-Mozart drivers routes through PA via
    /// the local file path. Mozart hands a URL to the device's REST.
    pub fn plays_via_pulseaudio(self) -> bool {
        matches!(
            self,
            DriverKind::Bluetooth | DriverKind::Local | DriverKind::MultiRoom
        )
    }
}

/// Which sink the active driver is bound to. Mirrors the
/// `active_bt_mac` / `active_multiroom_json` split in QtState but as
/// a single sum type for clarity in the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveSink {
    None,
    Bluetooth { mac: String, name: String },
    Local,
    MultiRoom { macs: Vec<String>, names: Vec<String> },
}

impl ActiveSink {
    pub fn is_some(&self) -> bool {
        !matches!(self, ActiveSink::None)
    }
    /// All MACs this sink involves. Empty for None / Local; one for
    /// Bluetooth; ≥2 for MultiRoom. Used by the watchdog and the
    /// BT-respect ledger.
    pub fn involved_macs(&self) -> Vec<String> {
        match self {
            ActiveSink::None | ActiveSink::Local => Vec::new(),
            ActiveSink::Bluetooth { mac, .. } => vec![mac.clone()],
            ActiveSink::MultiRoom { macs, .. } => macs.clone(),
        }
    }
}

/// The BT-respect ledger. Pure model of `bt_respect::BtRespect` with
/// the same disconnect-on-shutdown decision rule.
#[derive(Debug, Clone, Default)]
pub struct BtRespectLedger {
    /// MACs the controller observed as Connected at startup. NEVER
    /// scheduled for cleanup — those are pre-existing user choices.
    pub initial_connected: BTreeSet<String>,
    /// MACs the controller caused to connect during this session.
    /// On shutdown, (we_connected ∖ initial_connected) are disconnected.
    pub we_connected: BTreeSet<String>,
    /// Was the system BT adapter powered at startup?
    pub initial_bt_powered: Option<bool>,
    /// Did we issue `bluetoothctl power on`? Triggers restore-to-off
    /// when initial was false.
    pub we_powered_on: bool,
}

impl BtRespectLedger {
    /// Set of MACs that should be disconnected on app close.
    pub fn macs_to_disconnect(&self) -> BTreeSet<String> {
        self.we_connected
            .difference(&self.initial_connected)
            .cloned()
            .collect()
    }
    /// Should `bluetoothctl power off` fire on shutdown?
    pub fn should_restore_power_off(&self) -> bool {
        self.we_powered_on && self.initial_bt_powered == Some(false)
    }
}

/// Captures the controller's view of the world. Pure data — no Qt,
/// no async. Every field has a one-to-one correspondence with a
/// field on `QtState` (often with a slightly different name where
/// `QtState` keeps separate JSON/property pairs that the model
/// collapses to a single value).
#[derive(Debug, Clone)]
pub struct ControllerState {
    /// All discovered devices keyed by URL. The map is what the user
    /// would see in the device list.
    pub known_devices: BTreeMap<String, KnownDevice>,
    /// Currently selected device URL — empty string when no device
    /// has ever been picked.
    pub active_url: String,
    /// The driver kind selected for `active_url`. Authoritative — the
    /// `apply_action` function transitions this and the actual sink
    /// binding in lockstep.
    pub current_driver_kind: DriverKind,
    /// What the driver is bound to. Should be consistent with the
    /// driver kind (see `invariants::driver_kind_matches_active_sink`).
    pub active_sink: ActiveSink,
    /// Per-device persisted sink choices. When the user switches back
    /// to a known device, we restore its sink.
    pub persisted_sinks: BTreeMap<String, ActiveSink>,
    /// Set of URLs we've persisted as "doesn't support push notify".
    /// Modeled because the persistence-load-ordering bug was real.
    pub no_notify_devices: BTreeSet<String>,
    /// Has the no_notify KV load completed? Affects whether subscribe
    /// actions queue or dispatch.
    pub no_notify_loaded: bool,
    /// URLs we wanted to subscribe to before the load completed.
    pub pending_notify_subscriptions: BTreeSet<String>,
    /// Volume 0..100, regardless of routing.
    pub volume: i32,
    /// Mute state. Toggle-mute goes through the volume routing too.
    pub muted: bool,
    /// What the driver pid_slot looks like — `Some(format)` if audio
    /// is currently playing through a non-Mozart driver; None if idle.
    /// For Mozart we don't track this since pid is conceptually on
    /// the device, not us.
    pub playing_format: Option<AudioFormat>,
    /// Sink-unreachable watchdog counter. Increments on Some(None)
    /// from pactl while playing; resets on Some(Some(_)) or any
    /// user-driven sink change.
    pub sink_unreachable_strikes: i32,
    /// Volume outlier filter — last pending high-delta value if any.
    pub volume_pending: Option<i32>,
    /// BT respect ledger.
    pub bt_respect: BtRespectLedger,
    /// Has `device_notify` "given up" on the active device this
    /// session? Used by the model to track the cumulative
    /// observed-behavior set for that URL.
    pub gave_up_notify_this_session: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub struct KnownDevice {
    pub url: String,
    pub product_name: String,
    pub family_classification: Option<crate::sim::env::FirmwareFamily>,
}

use crate::sim::env::AudioFormat;

impl Default for ControllerState {
    fn default() -> Self {
        ControllerState {
            known_devices: BTreeMap::new(),
            active_url: String::new(),
            current_driver_kind: DriverKind::Noop,
            active_sink: ActiveSink::None,
            persisted_sinks: BTreeMap::new(),
            no_notify_devices: BTreeSet::new(),
            no_notify_loaded: false,
            pending_notify_subscriptions: BTreeSet::new(),
            volume: 30,
            muted: false,
            playing_format: None,
            sink_unreachable_strikes: 0,
            volume_pending: None,
            bt_respect: BtRespectLedger::default(),
            gave_up_notify_this_session: BTreeSet::new(),
        }
    }
}

impl ControllerState {
    /// Convenience constructor — fresh state with the no_notify
    /// blocklist already loaded (empty). Used by tests that don't
    /// care about the load-ordering dimension.
    pub fn fresh_loaded() -> Self {
        let mut s = Self::default();
        s.no_notify_loaded = true;
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_idle_noop() {
        let s = ControllerState::default();
        assert!(matches!(s.current_driver_kind, DriverKind::Noop));
        assert!(matches!(s.active_sink, ActiveSink::None));
        assert!(s.active_url.is_empty());
        assert_eq!(s.volume, 30);
        assert!(!s.muted);
        assert!(s.playing_format.is_none());
    }

    #[test]
    fn driver_kind_routing_dispatch() {
        // The volume-routing decision predicate must match what the
        // real refresh_volume / set_volume_api code does.
        assert!(DriverKind::Mozart.volume_uses_rest());
        assert!(DriverKind::Noop.volume_uses_rest());
        assert!(!DriverKind::Bluetooth.volume_uses_rest());
        assert!(!DriverKind::Local.volume_uses_rest());
        assert!(!DriverKind::MultiRoom.volume_uses_rest());

        assert!(!DriverKind::Mozart.plays_via_pulseaudio());
        assert!(!DriverKind::Noop.plays_via_pulseaudio());
        assert!(DriverKind::Bluetooth.plays_via_pulseaudio());
        assert!(DriverKind::Local.plays_via_pulseaudio());
        assert!(DriverKind::MultiRoom.plays_via_pulseaudio());
    }

    #[test]
    fn active_sink_involved_macs() {
        assert_eq!(ActiveSink::None.involved_macs(), Vec::<String>::new());
        assert_eq!(ActiveSink::Local.involved_macs(), Vec::<String>::new());
        assert_eq!(
            ActiveSink::Bluetooth {
                mac: "AA:BB".into(),
                name: "X".into(),
            }
            .involved_macs(),
            vec!["AA:BB".to_string()]
        );
        assert_eq!(
            ActiveSink::MultiRoom {
                macs: vec!["A".into(), "B".into(), "C".into()],
                names: vec!["a".into(), "b".into(), "c".into()],
            }
            .involved_macs(),
            vec!["A".to_string(), "B".into(), "C".into()]
        );
    }

    #[test]
    fn bt_ledger_clean_when_we_didnt_connect() {
        let mut l = BtRespectLedger::default();
        l.initial_connected.insert("USER:CONN".into());
        // Their connection persists — we DON'T schedule it for cleanup.
        assert!(l.macs_to_disconnect().is_empty());
    }

    #[test]
    fn bt_ledger_marks_only_our_connections_for_cleanup() {
        let mut l = BtRespectLedger::default();
        l.initial_connected.insert("USER:CONN".into());
        l.we_connected.insert("USER:CONN".into());   // we re-connected something already on
        l.we_connected.insert("OUR:NEW1".into());    // we caused new
        l.we_connected.insert("OUR:NEW2".into());
        let cleanup = l.macs_to_disconnect();
        // USER:CONN must NOT be in cleanup — it was already on.
        assert!(!cleanup.contains("USER:CONN"));
        assert!(cleanup.contains("OUR:NEW1"));
        assert!(cleanup.contains("OUR:NEW2"));
        assert_eq!(cleanup.len(), 2);
    }

    #[test]
    fn power_restore_only_when_off_at_start() {
        let mut l = BtRespectLedger::default();
        l.initial_bt_powered = Some(false);
        l.we_powered_on = true;
        assert!(l.should_restore_power_off());

        l.initial_bt_powered = Some(true);
        assert!(!l.should_restore_power_off(),
                "BT was already on — nothing to restore");

        l.initial_bt_powered = None;
        assert!(!l.should_restore_power_off(),
                "snapshot incomplete — conservative don't-touch");

        l.initial_bt_powered = Some(false);
        l.we_powered_on = false;
        assert!(!l.should_restore_power_off(),
                "we never powered on — nothing to undo");
    }
}
