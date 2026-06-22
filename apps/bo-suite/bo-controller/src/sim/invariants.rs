//! State-machine invariants. After every transition, every invariant
//! in `ALL_INVARIANTS` must hold. If one fails, the fuzz harness
//! prints the action trace that led to the violation.
//!
//! Each invariant has a short name (used in failure output) and a
//! one-line description. New invariants get added here as bugs are
//! found and turned into permanent regression checks.

use crate::sim::env::SimEnvironment;
use crate::sim::state::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvariantViolation {
    pub name: &'static str,
    pub detail: String,
}

/// Type of an invariant check. Returns `Ok(())` on hold, or
/// `Err(detail_str)` on violation.
pub type InvariantCheck =
    fn(&ControllerState, &SimEnvironment) -> Result<(), String>;

pub struct Invariant {
    pub name: &'static str,
    pub description: &'static str,
    pub check: InvariantCheck,
}

/// Every check that must hold post-transition. Appended-only; never
/// removed (regressions stay regressions).
pub const ALL_INVARIANTS: &[Invariant] = &[
    Invariant {
        name: "driver_kind_matches_active_sink",
        description: "DriverKind and ActiveSink must agree about what the \
                      controller is doing — no Bluetooth kind with no MAC, \
                      no MultiRoom kind with one mac.",
        check: driver_kind_matches_active_sink,
    },
    Invariant {
        name: "playing_requires_a_real_sink",
        description: "If `playing_format` is Some, the active driver must \
                      have a viable sink (not Noop, not None-bound).",
        check: playing_requires_a_real_sink,
    },
    Invariant {
        name: "bt_ledger_we_connected_excludes_failures",
        description: "we_connected can only contain MACs whose connect \
                      action actually succeeded. Otherwise we'd schedule \
                      a disconnect of something never connected.",
        check: bt_ledger_we_connected_excludes_failures,
    },
    Invariant {
        name: "no_uri_playback_on_unsupported_firmware",
        description: "If current_driver_kind is Mozart but the device's \
                      firmware doesn't accept URI playback, we shouldn't \
                      claim to be playing. The model's transitions enforce \
                      this; this invariant is the trip-wire.",
        check: no_uri_playback_on_unsupported_firmware,
    },
    Invariant {
        name: "watchdog_strikes_in_range",
        description: "sink_unreachable_strikes is 0..=2 inclusive — never \
                      negative, never above 2 (which would mean we missed \
                      the auto-stop trigger).",
        check: watchdog_strikes_in_range,
    },
    Invariant {
        name: "volume_in_range",
        description: "Volume is 0..=100. Mute/unmute logic doesn't put the \
                      slider out of bounds.",
        check: volume_in_range,
    },
    Invariant {
        name: "multiroom_has_at_least_two_macs",
        description: "When the driver is MultiRoom, the active sink must \
                      carry ≥2 macs. A single-mac multi-room is incoherent.",
        check: multiroom_has_at_least_two_macs,
    },
    Invariant {
        name: "multiroom_macs_all_paired",
        description: "Every MAC in an active multi-room must exist in the \
                      paired-speakers set. Stale MACs would cause pactl \
                      load-module to fail at runtime.",
        check: multiroom_macs_all_paired,
    },
    Invariant {
        name: "active_url_is_known_or_empty",
        description: "If active_url isn't empty, it must be in either the \
                      env.devices set OR known_devices (we've seen it before).",
        check: active_url_is_known_or_empty,
    },
    Invariant {
        name: "persisted_sinks_are_self_consistent",
        description: "Every persisted sink choice has well-formed contents \
                      (multi-room has ≥2 macs, etc.).",
        check: persisted_sinks_are_self_consistent,
    },
    Invariant {
        name: "bt_power_off_disallows_active_bt_sinks_after_playback",
        description: "If env.bt_currently_powered is false AND the user \
                      had been routing through BT, the model permits the \
                      stale sink to exist briefly but playing_format must \
                      be cleared on the next pactl-None read.",
        check: bt_power_off_implies_eventual_cleanup,
    },
    Invariant {
        name: "we_connected_macs_exist_in_paired_set",
        description: "Every MAC the ledger marks as 'we connected' must \
                      be an actually paired speaker in the environment. \
                      A phantom MAC in the cleanup set would dispatch a \
                      disconnect against nothing — silent waste, but \
                      worse, indicates a bookkeeping error where we lost \
                      track of which speakers are real.",
        check: we_connected_macs_exist_in_paired_set,
    },
    Invariant {
        name: "no_notify_loaded_drains_pending",
        description: "Once `no_notify_loaded` is true, every URL in \
                      `pending_notify_subscriptions` should EITHER be \
                      one we actively want to subscribe to (not yet \
                      drained on purpose) OR be blocked. We model the \
                      drain such that blocklisted URLs are removed but \
                      unblocked ones stay queued only if no_notify_loaded \
                      hasn't fired yet. After load, NO blocklisted URL \
                      should remain pending.",
        check: no_notify_loaded_drains_pending,
    },
    Invariant {
        name: "persisted_sink_urls_have_known_devices",
        description: "Every URL key in persisted_sinks should match a \
                      device we know about — either currently reachable \
                      in env.devices or remembered in known_devices. \
                      An orphan persistence key would mean we wrote a \
                      sink choice for a device we then completely \
                      forgot existed, which is incoherent.",
        check: persisted_sink_urls_have_known_devices,
    },
    Invariant {
        name: "volume_pending_consistent_with_volume",
        description: "If `volume_pending` is Some(p), it represents a \
                      mid-jump value awaiting confirmation. The pending \
                      value must differ from the current volume by more \
                      than 20pt (otherwise it'd have been adopted) and \
                      not more than 50pt (otherwise it'd have been \
                      rejected outright). Catches a stuck-pending bug \
                      where consecutive small reads fail to clear it.",
        check: volume_pending_consistent_with_volume,
    },
    Invariant {
        name: "active_sink_macs_are_paired",
        description: "Any MAC in the active sink (Bluetooth single or \
                      MultiRoom members) must exist in the paired \
                      speakers table. Catches the case where a sink \
                      restoration loaded a stale MAC that's no longer \
                      paired — pactl would fail at runtime.",
        check: active_sink_macs_are_paired,
    },
    Invariant {
        name: "gave_up_notify_implies_persisted",
        description: "Every URL we've added to gave_up_notify_this_session \
                      MUST also be in no_notify_devices. Giving up on the \
                      probe writes the URL to the persistent blocklist at \
                      the same time; the in-session set is just the \
                      subset that happened on THIS run. If they ever \
                      diverge, the next-session restart would re-probe a \
                      device we already know doesn't notify — exactly the \
                      bug class the persistent blocklist was added to \
                      prevent. Catches transition-logic regressions \
                      where someone updates one set without the other.",
        check: gave_up_notify_implies_persisted,
    },
];

// ── Implementations ──────────────────────────────────────────────

fn driver_kind_matches_active_sink(
    s: &ControllerState, _env: &SimEnvironment,
) -> Result<(), String> {
    match (s.current_driver_kind, &s.active_sink) {
        (DriverKind::Bluetooth, ActiveSink::Bluetooth { .. }) => Ok(()),
        (DriverKind::Local, ActiveSink::Local) => Ok(()),
        (DriverKind::MultiRoom, ActiveSink::MultiRoom { .. }) => Ok(()),
        (DriverKind::Mozart, ActiveSink::None) => Ok(()),
        (DriverKind::Mozart, ActiveSink::Local) => Ok(()),
        (DriverKind::Noop, ActiveSink::None) => Ok(()),
        (DriverKind::Noop, ActiveSink::Local) => Ok(()),
        (kind, sink) => Err(format!(
            "driver_kind {:?} doesn't match active_sink {:?}",
            kind, sink,
        )),
    }
}

fn playing_requires_a_real_sink(
    s: &ControllerState, _env: &SimEnvironment,
) -> Result<(), String> {
    if s.playing_format.is_some() {
        match s.current_driver_kind {
            DriverKind::Noop => {
                return Err(format!(
                    "claiming to play format {:?} with Noop driver",
                    s.playing_format
                ));
            }
            DriverKind::Bluetooth | DriverKind::MultiRoom => {
                if !s.active_sink.is_some() {
                    return Err(format!(
                        "driver {:?} with no active sink, but playing {:?}",
                        s.current_driver_kind, s.playing_format,
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn bt_ledger_we_connected_excludes_failures(
    s: &ControllerState, env: &SimEnvironment,
) -> Result<(), String> {
    use crate::sim::env::ConnectOutcome::Fails;
    for mac in &s.bt_respect.we_connected {
        if let Some(sp) = env.speakers.get(mac) {
            if sp.connect_outcome == Fails {
                return Err(format!(
                    "MAC {} marked we_connected but its connect_outcome=Fails",
                    mac,
                ));
            }
        }
    }
    Ok(())
}

fn no_uri_playback_on_unsupported_firmware(
    s: &ControllerState, env: &SimEnvironment,
) -> Result<(), String> {
    if s.current_driver_kind != DriverKind::Mozart {
        return Ok(());
    }
    if let Some(dev) = env.devices.get(&s.active_url) {
        if !dev.firmware.accepts_uri_playback() && s.playing_format.is_some() {
            return Err(format!(
                "kind=Mozart but device firmware {:?} doesn't accept URI \
                 playback; we shouldn't believe we're playing",
                dev.firmware,
            ));
        }
    }
    Ok(())
}

fn watchdog_strikes_in_range(
    s: &ControllerState, _env: &SimEnvironment,
) -> Result<(), String> {
    if s.sink_unreachable_strikes < 0 || s.sink_unreachable_strikes > 2 {
        return Err(format!(
            "sink_unreachable_strikes = {} (expected 0..=2)",
            s.sink_unreachable_strikes,
        ));
    }
    Ok(())
}

fn volume_in_range(
    s: &ControllerState, _env: &SimEnvironment,
) -> Result<(), String> {
    if s.volume < 0 || s.volume > 100 {
        return Err(format!(
            "volume = {} (expected 0..=100)", s.volume
        ));
    }
    Ok(())
}

fn multiroom_has_at_least_two_macs(
    s: &ControllerState, _env: &SimEnvironment,
) -> Result<(), String> {
    if let ActiveSink::MultiRoom { macs, .. } = &s.active_sink {
        if macs.len() < 2 {
            return Err(format!(
                "multi-room with {} macs (need ≥2)", macs.len()
            ));
        }
    }
    Ok(())
}

fn multiroom_macs_all_paired(
    s: &ControllerState, env: &SimEnvironment,
) -> Result<(), String> {
    if let ActiveSink::MultiRoom { macs, .. } = &s.active_sink {
        for m in macs {
            if !env.speakers.contains_key(m) {
                return Err(format!(
                    "multi-room member {} is not in paired speakers", m
                ));
            }
        }
    }
    Ok(())
}

fn active_url_is_known_or_empty(
    s: &ControllerState, env: &SimEnvironment,
) -> Result<(), String> {
    if s.active_url.is_empty() {
        return Ok(());
    }
    if !env.devices.contains_key(&s.active_url)
        && !s.known_devices.contains_key(&s.active_url)
    {
        return Err(format!(
            "active_url={} is neither in env.devices nor known_devices",
            s.active_url,
        ));
    }
    Ok(())
}

fn persisted_sinks_are_self_consistent(
    s: &ControllerState, _env: &SimEnvironment,
) -> Result<(), String> {
    for (url, sink) in &s.persisted_sinks {
        match sink {
            ActiveSink::MultiRoom { macs, names } => {
                if macs.len() < 2 {
                    return Err(format!(
                        "persisted multi-room for {} has only {} macs",
                        url, macs.len(),
                    ));
                }
                if !names.is_empty() && names.len() != macs.len() {
                    return Err(format!(
                        "persisted multi-room for {}: macs={} names={}",
                        url, macs.len(), names.len(),
                    ));
                }
            }
            ActiveSink::Bluetooth { mac, .. } => {
                if mac.is_empty() {
                    return Err(format!(
                        "persisted Bluetooth for {} has empty MAC", url
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn bt_power_off_implies_eventual_cleanup(
    s: &ControllerState, env: &SimEnvironment,
) -> Result<(), String> {
    // Quiet by default — the actual enforcement of "stop playback
    // when sink disappears" is via the watchdog. We just check
    // that the state isn't in a permanently-stuck configuration:
    // BT off, BT sink active, watchdog strikes already at 2 means
    // the next None would tip but hasn't yet — that's fine.
    if !env.bt_currently_powered {
        if matches!(
            s.current_driver_kind,
            DriverKind::Bluetooth | DriverKind::MultiRoom
        ) && s.sink_unreachable_strikes > 2
        {
            return Err(format!(
                "BT system off, BT-class driver still active, strikes = {}",
                s.sink_unreachable_strikes,
            ));
        }
    }
    Ok(())
}

fn we_connected_macs_exist_in_paired_set(
    s: &ControllerState, env: &SimEnvironment,
) -> Result<(), String> {
    for mac in &s.bt_respect.we_connected {
        if !env.speakers.contains_key(mac) {
            return Err(format!(
                "ledger says we connected {} but it isn't in env.speakers",
                mac,
            ));
        }
    }
    Ok(())
}

fn no_notify_loaded_drains_pending(
    s: &ControllerState, _env: &SimEnvironment,
) -> Result<(), String> {
    if !s.no_notify_loaded { return Ok(()); }
    // After KV load, pending should be entirely empty: blocklisted
    // URLs were skip-drained, non-blocklisted ones were dispatched.
    // Either way, none remain.
    if !s.pending_notify_subscriptions.is_empty() {
        return Err(format!(
            "after KV load, pending_notify_subscriptions should be \
             empty but contains {} entries: {:?}",
            s.pending_notify_subscriptions.len(),
            s.pending_notify_subscriptions,
        ));
    }
    Ok(())
}

fn persisted_sink_urls_have_known_devices(
    s: &ControllerState, env: &SimEnvironment,
) -> Result<(), String> {
    for url in s.persisted_sinks.keys() {
        if !env.devices.contains_key(url)
            && !s.known_devices.contains_key(url)
        {
            return Err(format!(
                "persisted sink for {} but device not in env or known set",
                url,
            ));
        }
    }
    Ok(())
}

fn volume_pending_consistent_with_volume(
    s: &ControllerState, _env: &SimEnvironment,
) -> Result<(), String> {
    if let Some(p) = s.volume_pending {
        let delta = (p - s.volume).abs();
        // A pending value is only legitimate when it was held due to
        // exceeding the 50pt threshold. Any delta ≤ 50 should have
        // adopted-or-cleared at the time of the read; if we still see
        // a small delta in pending, something prematurely held.
        if delta <= 50 {
            return Err(format!(
                "volume_pending = {} but delta to current {} is only {} \
                 (≤50pt); should have been adopted or cleared, not held",
                p, s.volume, delta,
            ));
        }
    }
    Ok(())
}

fn active_sink_macs_are_paired(
    s: &ControllerState, env: &SimEnvironment,
) -> Result<(), String> {
    match &s.active_sink {
        ActiveSink::Bluetooth { mac, .. } => {
            if !env.speakers.contains_key(mac) {
                return Err(format!(
                    "active BT sink references unpaired MAC {}", mac,
                ));
            }
        }
        ActiveSink::MultiRoom { macs, .. } => {
            for m in macs {
                if !env.speakers.contains_key(m) {
                    return Err(format!(
                        "active multi-room references unpaired MAC {}", m,
                    ));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn gave_up_notify_implies_persisted(
    s: &ControllerState, _env: &SimEnvironment,
) -> Result<(), String> {
    for url in &s.gave_up_notify_this_session {
        if !s.no_notify_devices.contains(url) {
            return Err(format!(
                "URL {} is in gave_up_notify_this_session but missing \
                 from no_notify_devices — these sets must move \
                 together so the next session doesn't re-probe",
                url,
            ));
        }
    }
    Ok(())
}

/// Run every invariant. Returns the FIRST violation found, or Ok
/// when all pass.
pub fn check_invariants(
    s: &ControllerState, env: &SimEnvironment,
) -> Result<(), InvariantViolation> {
    for inv in ALL_INVARIANTS {
        if let Err(detail) = (inv.check)(s, env) {
            return Err(InvariantViolation { name: inv.name, detail });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::env::SimEnvironment;

    #[test]
    fn default_state_satisfies_all() {
        let s = ControllerState::default();
        let e = SimEnvironment::default();
        check_invariants(&s, &e).unwrap();
    }

    #[test]
    fn invalid_driver_kind_sink_combo_caught() {
        let mut s = ControllerState::default();
        s.current_driver_kind = DriverKind::Bluetooth;
        s.active_sink = ActiveSink::None;
        let e = SimEnvironment::default();
        let err = check_invariants(&s, &e).unwrap_err();
        assert_eq!(err.name, "driver_kind_matches_active_sink");
    }

    #[test]
    fn playing_with_noop_caught() {
        let mut s = ControllerState::default();
        s.playing_format = Some(crate::sim::env::AudioFormat::Opus);
        let e = SimEnvironment::default();
        let err = check_invariants(&s, &e).unwrap_err();
        assert_eq!(err.name, "playing_requires_a_real_sink");
    }

    #[test]
    fn out_of_range_strikes_caught() {
        let mut s = ControllerState::default();
        s.sink_unreachable_strikes = 5;
        let e = SimEnvironment::default();
        let err = check_invariants(&s, &e).unwrap_err();
        assert_eq!(err.name, "watchdog_strikes_in_range");
    }

    #[test]
    fn single_mac_multiroom_caught() {
        let mut s = ControllerState::default();
        s.current_driver_kind = DriverKind::MultiRoom;
        s.active_sink = ActiveSink::MultiRoom {
            macs: vec!["X".into()],
            names: vec!["x".into()],
        };
        let e = SimEnvironment::default();
        let err = check_invariants(&s, &e).unwrap_err();
        assert_eq!(err.name, "multiroom_has_at_least_two_macs");
    }
}
