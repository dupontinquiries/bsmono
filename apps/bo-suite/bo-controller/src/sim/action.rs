//! User and system actions the simulator can dispatch. Every action
//! has a clear preconditions check and a deterministic effect on the
//! controller state given the environment.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    // ── Device-level actions (the user picks a device) ──────────
    SelectDevice { url: String },

    // ── Driver / sink binding ───────────────────────────────────
    /// Bind a single BT speaker as the current driver.
    SetActiveBt  { mac: String, name: String },
    /// Bind a multi-room set of speakers. Must be ≥2 macs.
    SetMultiRoom { macs: Vec<String>, names: Vec<String> },
    /// Disband multi-room. Returns to whatever sink was active prior
    /// (or none if there wasn't one).
    ClearMultiRoom,
    /// Explicitly choose "play locally" — the controller's default
    /// PulseAudio sink.
    SelectLocalSink,
    /// Reset to the noop driver (no sink). Used by `driver_stop` and
    /// by some recovery paths.
    DriverStop,

    // ── Volume / mute ───────────────────────────────────────────
    SetVolume { pct: i32 },
    ToggleMute,

    // ── Playback ────────────────────────────────────────────────
    PlayTrack { track_id: i64 },

    // ── System-level events (not user-initiated) ────────────────
    /// pactl poll returned `pct` for the active driver's sink.
    /// `None` means the sink wasn't found — feeds the watchdog.
    SimulatePactlVolumeRead { pct: Option<i32> },
    /// The active BT speaker disconnected mid-playback. Causes the
    /// PA sink to disappear; the watchdog should detect this within
    /// two strikes.
    SimulateBtSinkVanish { mac: String },
    /// The user (or external app) toggled the system BT power state.
    /// Affects `bt_currently_powered` and may invalidate active sinks.
    ToggleSystemBtPower,
    /// The user explicitly requested power-on/off (via the picker
    /// confirm-unlock button).
    UserSetBtPower { on: bool },
    /// `device_notify` exhausted its 5-cycle probe and is giving
    /// up. Records the URL in `no_notify_devices` and (if KV was
    /// previously loaded) falls through to polling.
    SimulateNotifyProbeGiveUp { url: String },
    /// The persisted `no_notify_devices` KV value arrived — populates
    /// the blocklist and flips `no_notify_loaded`.
    SimulateNoNotifyKvLoad { urls: Vec<String> },
    /// Track playback naturally completed (paplay exit code 0).
    SimulatePlaybackEnd,

    // ── Lifecycle ───────────────────────────────────────────────
    /// App close — runs the cleanup path. Models BT-respect's
    /// disconnect-on-shutdown decisions.
    AppClose,
}

impl Action {
    /// Short human-readable label. Used in fuzz-failure traces so
    /// the output is grep-able and tells the story directly.
    pub fn label(&self) -> String {
        match self {
            Action::SelectDevice { url }            => format!("select-device {}", url),
            Action::SetActiveBt { mac, name }       => format!("set-bt {} ({})", mac, name),
            Action::SetMultiRoom { macs, .. }       => format!("multiroom [{}]", macs.join(",")),
            Action::ClearMultiRoom                  => "clear-multiroom".into(),
            Action::SelectLocalSink                 => "select-local-sink".into(),
            Action::DriverStop                      => "driver-stop".into(),
            Action::SetVolume { pct }               => format!("set-volume {}", pct),
            Action::ToggleMute                      => "toggle-mute".into(),
            Action::PlayTrack { track_id }          => format!("play-track {}", track_id),
            Action::SimulatePactlVolumeRead { pct } => format!("sim-pactl-vol {:?}", pct),
            Action::SimulateBtSinkVanish { mac }    => format!("sim-bt-vanish {}", mac),
            Action::ToggleSystemBtPower             => "toggle-system-bt".into(),
            Action::UserSetBtPower { on }           => format!("user-set-bt {}", on),
            Action::SimulateNotifyProbeGiveUp { url } =>
                format!("sim-notify-giveup {}", url),
            Action::SimulateNoNotifyKvLoad { urls } =>
                format!("sim-notify-kv-load {:?}", urls),
            Action::SimulatePlaybackEnd             => "sim-playback-end".into(),
            Action::AppClose                        => "app-close".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_labels_are_concise() {
        for a in [
            Action::SelectDevice { url: "http://1".into() },
            Action::SetActiveBt {
                mac: "AA".into(),
                name: "n".into(),
            },
            Action::ToggleMute,
            Action::AppClose,
        ] {
            let l = a.label();
            assert!(!l.is_empty());
            assert!(l.len() < 64, "label too long: {}", l);
        }
    }
}
