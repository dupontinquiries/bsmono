//! `apply_action` — the heart of the simulator. Takes a state and
//! environment, runs an action, and returns a (new_state, outcome)
//! pair. Pure: no side effects on either input.
//!
//! Each branch encodes the same decision logic the real `QtState`
//! uses. Where the real code's logic forks based on async results
//! that aren't available synchronously, the model uses the
//! environment's deterministic profile (e.g., `connect_outcome` on
//! a SpeakerProfile decides whether the BT bind succeeds).
//!
//! When the model's decision differs from the real code, it's a
//! design escape — the discrepancy is either a bug in the real code
//! (which the invariants check will surface) or a place where the
//! model is wrong and needs an update.

use crate::sim::action::Action;
use crate::sim::env::*;
use crate::sim::state::*;

/// Information about what the transition did. Mostly diagnostic — the
/// caller uses it for logging and to detect "the action was a no-op."
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionOutcome {
    /// Action applied successfully.
    Ok,
    /// Action refused for stated reason (precondition not met, etc.).
    Refused { reason: String },
    /// Action applied with a side note (e.g. "stopped playback because
    /// the new driver doesn't support the current source format").
    OkWithNote { note: String },
}

impl TransitionOutcome {
    pub fn refused(s: impl Into<String>) -> Self {
        TransitionOutcome::Refused { reason: s.into() }
    }
    pub fn with_note(s: impl Into<String>) -> Self {
        TransitionOutcome::OkWithNote { note: s.into() }
    }
}

/// Apply an action. Pure: returns a new state + outcome without
/// mutating inputs.
pub fn apply_action(
    state: &ControllerState,
    env: &SimEnvironment,
    action: &Action,
) -> (ControllerState, TransitionOutcome) {
    let mut s = state.clone();
    let outcome = apply_in_place(&mut s, env, action);
    (s, outcome)
}

fn apply_in_place(
    s: &mut ControllerState,
    env: &SimEnvironment,
    action: &Action,
) -> TransitionOutcome {
    match action {
        Action::SelectDevice { url } => {
            if url.is_empty() {
                return TransitionOutcome::refused("empty URL");
            }
            if url == &s.active_url {
                return TransitionOutcome::with_note("already active");
            }
            let dev = match env.devices.get(url) {
                Some(d) => d,
                None => {
                    return TransitionOutcome::refused(
                        format!("unknown device {}", url),
                    );
                }
            };
            if !dev.reachable {
                return TransitionOutcome::refused("device unreachable");
            }
            // Switching device — sink-binding watchdog resets.
            s.sink_unreachable_strikes = 0;
            s.volume_pending = None;
            s.active_url = url.clone();
            // Stop any in-flight non-Mozart playback. Mozart-active
            // routing doesn't have a local PID to kill but the model
            // still clears `playing_format` since the previous
            // device's stream isn't ours anymore.
            s.playing_format = None;

            // Restore persisted sink choice if any. Otherwise default
            // to the firmware-based pick (Mozart-active → Mozart;
            // legacy → noop until user binds a sink).
            s.known_devices
                .entry(url.clone())
                .or_insert(KnownDevice {
                    url: url.clone(),
                    product_name: dev.product_name.clone(),
                    family_classification: Some(dev.firmware),
                });
            match s.persisted_sinks.get(url).cloned() {
                Some(sink) => {
                    s.active_sink = sink;
                    s.current_driver_kind = derive_driver_kind(&s.active_sink, dev);
                }
                None => {
                    s.active_sink = ActiveSink::None;
                    s.current_driver_kind = default_kind_for(dev);
                }
            }
            TransitionOutcome::Ok
        }

        Action::SetActiveBt { mac, name } => {
            if !env.bt_currently_powered {
                return TransitionOutcome::refused(
                    "system BT is OFF — turn it on first",
                );
            }
            let speaker = match env.speakers.get(mac) {
                Some(s) => s,
                None => return TransitionOutcome::refused(
                    format!("unpaired MAC {}", mac),
                ),
            };
            // Driver-kind transition: Mozart-active devices fall
            // through to BT routing when the user explicitly picks
            // a sink (overriding the REST path).
            s.sink_unreachable_strikes = 0;
            s.volume_pending = None;
            // Record that we attempted/succeeded to connect this MAC.
            // In the real code BluetoothDriver::create runs bluetoothctl
            // connect inside the driver build; we model the same.
            match speaker.connect_outcome {
                ConnectOutcome::Success | ConnectOutcome::SuccessThenDrops => {
                    s.bt_respect.we_connected.insert(mac.clone());
                    s.active_sink = ActiveSink::Bluetooth {
                        mac: mac.clone(),
                        name: name.clone(),
                    };
                    s.current_driver_kind = DriverKind::Bluetooth;
                    // Persist for this device URL.
                    if !s.active_url.is_empty() {
                        s.persisted_sinks
                            .insert(s.active_url.clone(), s.active_sink.clone());
                    }
                    TransitionOutcome::Ok
                }
                ConnectOutcome::Fails => {
                    TransitionOutcome::refused(format!(
                        "bluetoothctl connect {} failed", mac
                    ))
                }
            }
        }

        Action::SetMultiRoom { macs, names } => {
            if !env.bt_currently_powered {
                return TransitionOutcome::refused("system BT is OFF");
            }
            if macs.len() < 2 {
                return TransitionOutcome::refused(
                    "multi-room needs ≥2 speakers",
                );
            }
            // Pre-check: every speaker must have a visible PA sink.
            // Real code does `find_missing_pa_sinks` first to give a
            // friendly error rather than the cryptic combine-sink
            // "Input/Output error."
            let missing: Vec<String> = macs.iter()
                .filter(|m| {
                    env.speakers.get(*m)
                        .map(|sp| !sp.pa_sink_visible)
                        .unwrap_or(true)
                })
                .cloned()
                .collect();
            if !missing.is_empty() {
                return TransitionOutcome::refused(format!(
                    "{} speaker(s) not PA-visible: {:?}",
                    missing.len(), missing
                ));
            }
            // Each member's connect was recorded by a prior SetActiveBt
            // OR is implicitly recorded now if we never tried it.
            // Don't credit ourselves with connecting a speaker whose
            // connect_outcome is Fails — if its PA sink is currently
            // visible it was made so by something else (a prior user
            // session, an external app, etc.). Adding it to
            // we_connected would mean we'd disconnect it on cleanup
            // even though we didn't cause it.
            for m in macs {
                let sp_connectable = env.speakers.get(m)
                    .map(|sp| sp.connect_outcome != ConnectOutcome::Fails)
                    .unwrap_or(false);
                if sp_connectable {
                    s.bt_respect.we_connected.insert(m.clone());
                }
            }
            s.sink_unreachable_strikes = 0;
            s.volume_pending = None;
            s.active_sink = ActiveSink::MultiRoom {
                macs: macs.clone(),
                names: names.clone(),
            };
            s.current_driver_kind = DriverKind::MultiRoom;
            if !s.active_url.is_empty() {
                s.persisted_sinks
                    .insert(s.active_url.clone(), s.active_sink.clone());
            }
            TransitionOutcome::Ok
        }

        Action::ClearMultiRoom => {
            if !matches!(s.active_sink, ActiveSink::MultiRoom { .. }) {
                return TransitionOutcome::with_note("no multi-room active");
            }
            s.sink_unreachable_strikes = 0;
            s.volume_pending = None;
            s.active_sink = ActiveSink::None;
            // Fall back to the device's default kind.
            if let Some(dev) = env.devices.get(&s.active_url) {
                s.current_driver_kind = default_kind_for(dev);
            } else {
                s.current_driver_kind = DriverKind::Noop;
            }
            // If the new driver kind can't carry the current playback
            // (e.g. landed on Noop because the device is legacy), stop.
            // Real behavior: combine-sink teardown interrupts paplay
            // immediately. The model should mirror that.
            let new_can_play = matches!(
                s.current_driver_kind,
                DriverKind::Mozart | DriverKind::Bluetooth
                    | DriverKind::Local | DriverKind::MultiRoom
            );
            if !new_can_play {
                s.playing_format = None;
            }
            if !s.active_url.is_empty() {
                s.persisted_sinks
                    .insert(s.active_url.clone(), ActiveSink::None);
            }
            TransitionOutcome::Ok
        }

        Action::SelectLocalSink => {
            s.sink_unreachable_strikes = 0;
            s.active_sink = ActiveSink::Local;
            s.current_driver_kind = DriverKind::Local;
            if !s.active_url.is_empty() {
                s.persisted_sinks
                    .insert(s.active_url.clone(), ActiveSink::Local);
            }
            TransitionOutcome::Ok
        }

        Action::DriverStop => {
            // driver_stop kills in-flight playback but the binding
            // stays — the user can re-press play.
            s.playing_format = None;
            s.sink_unreachable_strikes = 0;
            TransitionOutcome::Ok
        }

        Action::SetVolume { pct } => {
            let clamped = (*pct).clamp(0, 100);
            s.volume = clamped;
            s.muted = false;  // any explicit volume un-mutes
            // User-driven volume changes invalidate any in-flight
            // pactl-pending value — the user has explicitly chosen,
            // so the held confirmation candidate is now obsolete.
            // Without this clear, a stale pending from before the
            // manual change would remain in state until the next
            // pactl-confirmation cycle adopted or rejected it,
            // potentially overriding the user's chosen value.
            s.volume_pending = None;
            TransitionOutcome::Ok
        }

        Action::ToggleMute => {
            s.muted = !s.muted;
            TransitionOutcome::Ok
        }

        Action::PlayTrack { track_id } => {
            let track = match env.catalog.iter().find(|t| t.id == *track_id) {
                Some(t) => t,
                None => return TransitionOutcome::refused(
                    format!("track {} not in catalog", track_id)
                ),
            };
            // Decide the routing dispatch from the active driver kind.
            // - Bluetooth/Local/MultiRoom: paplay (+ffmpeg if needed)
            // - Mozart: REST play_uri
            // - Noop: refuse — there's nowhere to play
            match s.current_driver_kind {
                DriverKind::Noop => {
                    TransitionOutcome::refused(
                        "no driver — pick a device or sink first"
                    )
                }
                DriverKind::Mozart => {
                    // The device must accept URI playback.
                    let dev = env.devices.get(&s.active_url);
                    match dev {
                        Some(d) if d.firmware.accepts_uri_playback() => {
                            s.playing_format = Some(track.format);
                            TransitionOutcome::Ok
                        }
                        Some(d) => TransitionOutcome::refused(format!(
                            "device firmware {:?} doesn't accept URI playback",
                            d.firmware
                        )),
                        None => TransitionOutcome::refused(
                            "active device gone"
                        ),
                    }
                }
                _ => {
                    // PA-driven kinds. paplay-only for WAV; ffmpeg pipe
                    // for everything else. Both succeed in the model
                    // when the sink is reachable.
                    if let Some(mac) = match &s.active_sink {
                        ActiveSink::Bluetooth { mac, .. } => Some(mac.clone()),
                        ActiveSink::MultiRoom { macs, .. } => macs.first().cloned(),
                        _ => None,
                    } {
                        let reachable = env.speakers.get(&mac)
                            .map(|sp| sp.pa_sink_visible)
                            .unwrap_or(false);
                        if !reachable {
                            return TransitionOutcome::refused(
                                format!("sink for {} not PA-visible", mac),
                            );
                        }
                    }
                    s.playing_format = Some(track.format);
                    TransitionOutcome::Ok
                }
            }
        }

        Action::SimulatePactlVolumeRead { pct } => {
            match pct {
                Some(p) => {
                    s.sink_unreachable_strikes = 0;
                    // Match the real `driver_volume` handler in main.rs:
                    //
                    //   delta < 2  → no-op, clear any held pending
                    //   delta ≤ 50 → adopt immediately
                    //   delta > 50 → hold for confirmation; on the
                    //                second consecutive same-value read,
                    //                adopt. Otherwise stay held.
                    //
                    // The 50pt threshold isn't a "reject outright" — a
                    // genuine user-driven jump can be that big across two
                    // ticks. The hold gives a transient blip time to
                    // self-correct before yanking the slider.
                    let delta = (p - s.volume).abs();
                    if delta < 2 {
                        s.volume_pending = None;
                    } else if delta > 50 {
                        if s.volume_pending == Some(*p) {
                            s.volume = *p;
                            s.volume_pending = None;
                        } else {
                            s.volume_pending = Some(*p);
                        }
                    } else {
                        s.volume = *p;
                        s.volume_pending = None;
                    }
                }
                None => {
                    // Only count strikes if we believe audio is playing.
                    let playing_via_pa = s.playing_format.is_some()
                        && s.current_driver_kind.plays_via_pulseaudio();
                    if !playing_via_pa {
                        s.sink_unreachable_strikes = 0;
                        return TransitionOutcome::with_note(
                            "no playback — strike skipped"
                        );
                    }
                    s.sink_unreachable_strikes += 1;
                    if s.sink_unreachable_strikes >= 2 {
                        // Two strikes → sink confirmed gone. Stop.
                        s.playing_format = None;
                        s.sink_unreachable_strikes = 0;
                        return TransitionOutcome::with_note(
                            "two strikes — driver stopped"
                        );
                    }
                }
            }
            TransitionOutcome::Ok
        }

        Action::SimulateBtSinkVanish { mac } => {
            // Doesn't directly mutate state — represents the system
            // event. The next SimulatePactlVolumeRead with None will
            // be the actual detection path.
            //
            // We DO mark the speaker offline for the model (so
            // subsequent operations on it fail).
            // But env is &SimEnvironment — we can't mutate it here.
            // For modeling, the caller is responsible for also
            // updating env to reflect this. The action itself is a
            // no-op on state but exists so traces are explicit.
            let _ = mac;
            TransitionOutcome::Ok
        }

        Action::ToggleSystemBtPower => {
            // Power-state changes mutate env, but env is immutable
            // through apply. In tests we apply this and then update
            // env separately. The state-level effect is that any
            // active BT sink becomes "in limbo" until the next read.
            TransitionOutcome::Ok
        }

        Action::UserSetBtPower { on } => {
            if *on {
                s.bt_respect.we_powered_on = true;
            }
            TransitionOutcome::Ok
        }

        Action::SimulateNotifyProbeGiveUp { url } => {
            s.gave_up_notify_this_session.insert(url.clone());
            // If KV already loaded, persistence happens immediately.
            // Otherwise the controller stages the URL until load.
            s.no_notify_devices.insert(url.clone());
            TransitionOutcome::Ok
        }

        Action::SimulateNoNotifyKvLoad { urls } => {
            for u in urls {
                s.no_notify_devices.insert(u.clone());
            }
            s.no_notify_loaded = true;
            // Real code: drain ALL pending. For blocklisted URLs the
            // drain skips dispatching the Subscribe command; for the
            // rest it dispatches via the worker. Either way they leave
            // the pending set entirely — "pending" means "queued
            // before KV had loaded," and once KV loads the queue is
            // resolved one way or the other.
            s.pending_notify_subscriptions.clear();
            TransitionOutcome::Ok
        }

        Action::SimulatePlaybackEnd => {
            s.playing_format = None;
            TransitionOutcome::Ok
        }

        Action::AppClose => {
            // Cleanup model: each (we_connected ∖ initial_connected)
            // gets a disconnect attempt. Power restoration if
            // applicable. We don't mutate env here — the simulator
            // updates env to "after-shutdown" state on its own if it
            // wants to verify the disconnect-on-shutdown side effect.
            TransitionOutcome::Ok
        }
    }
}

/// What driver kind does a device default to when no sink is bound?
/// Mozart-active → Mozart; otherwise Noop (the user must bind a sink).
fn default_kind_for(dev: &DeviceProfile) -> DriverKind {
    if dev.firmware.accepts_uri_playback() {
        DriverKind::Mozart
    } else {
        DriverKind::Noop
    }
}

/// Given a sink binding and the active device, derive the driver kind.
/// Used when restoring a persisted choice.
fn derive_driver_kind(sink: &ActiveSink, dev: &DeviceProfile) -> DriverKind {
    match sink {
        ActiveSink::None => default_kind_for(dev),
        ActiveSink::Bluetooth { .. } => DriverKind::Bluetooth,
        ActiveSink::Local => DriverKind::Local,
        ActiveSink::MultiRoom { .. } => DriverKind::MultiRoom,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> SimEnvironment {
        SimEnvironment::two_devices_four_speakers()
    }

    #[test]
    fn select_unknown_device_refused() {
        let s = ControllerState::fresh_loaded();
        let (_, out) = apply_action(
            &s, &env(),
            &Action::SelectDevice { url: "http://nope".into() },
        );
        assert!(matches!(out, TransitionOutcome::Refused { .. }));
    }

    #[test]
    fn select_mozart_device_lands_on_mozart_kind() {
        let mut env = env();
        env.devices.get_mut("http://192.168.2.14").unwrap().firmware =
            FirmwareFamily::Mozart;
        let s = ControllerState::fresh_loaded();
        let (s2, out) = apply_action(
            &s, &env,
            &Action::SelectDevice { url: "http://192.168.2.14".into() },
        );
        assert_eq!(out, TransitionOutcome::Ok);
        assert_eq!(s2.current_driver_kind, DriverKind::Mozart);
        assert!(matches!(s2.active_sink, ActiveSink::None));
    }

    #[test]
    fn select_legacy_device_lands_on_noop() {
        // MozartLegacy doesn't accept URI playback → noop until user
        // picks a sink.
        let s = ControllerState::fresh_loaded();
        let (s2, _) = apply_action(
            &s, &env(),
            &Action::SelectDevice { url: "http://192.168.2.12".into() },
        );
        assert_eq!(s2.current_driver_kind, DriverKind::Noop);
    }

    #[test]
    fn bind_bt_with_bt_powered_off_refused() {
        let mut e = env();
        e.bt_currently_powered = false;
        let s = ControllerState::fresh_loaded();
        let (_, out) = apply_action(
            &s, &e,
            &Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
        );
        assert!(matches!(out, TransitionOutcome::Refused { .. }));
    }

    #[test]
    fn bind_bt_records_we_connected() {
        let s = ControllerState::fresh_loaded();
        let (s2, out) = apply_action(
            &s, &env(),
            &Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
        );
        assert_eq!(out, TransitionOutcome::Ok);
        assert_eq!(s2.current_driver_kind, DriverKind::Bluetooth);
        assert!(s2.bt_respect.we_connected.contains("00:09:A7:77:CD:55"));
    }

    #[test]
    fn bind_smokey_fails_cleanly() {
        // The unreliable speaker. Driver kind stays Noop, sink None,
        // no panic, no zombie state.
        let s = ControllerState::fresh_loaded();
        let (s2, out) = apply_action(
            &s, &env(),
            &Action::SetActiveBt {
                mac: "50:1E:2D:5B:E6:EF".into(),
                name: "Smokey".into(),
            },
        );
        assert!(matches!(out, TransitionOutcome::Refused { .. }));
        assert_eq!(s2.current_driver_kind, DriverKind::Noop);
        assert!(matches!(s2.active_sink, ActiveSink::None));
        // CRUCIALLY: BT respect ledger should NOT carry a we_connected
        // entry for a connect that didn't succeed.
        assert!(
            !s2.bt_respect.we_connected.contains("50:1E:2D:5B:E6:EF"),
            "failed connects must not pollute the ledger"
        );
    }

    #[test]
    fn multiroom_with_invisible_sink_refused_friendly() {
        // Combining Emerge01 + Smokey (sink not visible) should
        // refuse with a list of which speakers are missing — the
        // user's report of "multi-room: Input/Output error" should
        // never happen in the model.
        let s = ControllerState::fresh_loaded();
        let (_, out) = apply_action(
            &s, &env(),
            &Action::SetMultiRoom {
                macs: vec![
                    "00:09:A7:77:CD:55".into(),
                    "50:1E:2D:5B:E6:EF".into(),
                ],
                names: vec!["Emerge01".into(), "Smokey".into()],
            },
        );
        match out {
            TransitionOutcome::Refused { reason } => {
                assert!(reason.contains("50:1E:2D:5B:E6:EF"),
                        "refusal must name the missing speaker: {}", reason);
                assert!(reason.contains("not PA-visible"));
            }
            other => panic!("expected refused, got {:?}", other),
        }
    }

    #[test]
    fn multiroom_with_one_speaker_refused() {
        let s = ControllerState::fresh_loaded();
        let (_, out) = apply_action(
            &s, &env(),
            &Action::SetMultiRoom {
                macs: vec!["00:09:A7:77:CD:55".into()],
                names: vec!["Emerge01".into()],
            },
        );
        assert!(matches!(out, TransitionOutcome::Refused { .. }));
    }

    #[test]
    fn play_with_no_driver_refused() {
        let s = ControllerState::fresh_loaded();
        let (_, out) = apply_action(
            &s, &env(),
            &Action::PlayTrack { track_id: 2060 },
        );
        assert!(matches!(out, TransitionOutcome::Refused { .. }));
    }

    #[test]
    fn play_on_legacy_mozart_via_bt_works() {
        // The user's actual scenario: Stage (legacy) device active,
        // BT routing to Emerge speaker. play_track should route via
        // PulseAudio, not REST.
        let s = ControllerState::fresh_loaded();
        let (s, _) = apply_action(
            &s, &env(),
            &Action::SelectDevice { url: "http://192.168.2.12".into() },
        );
        let (s, _) = apply_action(
            &s, &env(),
            &Action::SetActiveBt {
                mac: "00:09:A7:77:C2:88".into(),
                name: "Beosound Emerge".into(),
            },
        );
        let (s, out) = apply_action(
            &s, &env(),
            &Action::PlayTrack { track_id: 2060 },
        );
        assert_eq!(out, TransitionOutcome::Ok);
        assert!(s.playing_format.is_some());
        // Driver kind must NOT be Mozart — that would route REST.
        assert_eq!(s.current_driver_kind, DriverKind::Bluetooth);
    }

    #[test]
    fn play_uri_refused_on_legacy_when_kind_is_mozart() {
        // Hypothetical: if the driver kind somehow ended up Mozart
        // on a legacy device, play should be refused (matches what
        // the device's REST returns). Tests the predicate boundary.
        let mut s = ControllerState::fresh_loaded();
        s.active_url = "http://192.168.2.12".into();
        s.current_driver_kind = DriverKind::Mozart;
        let (_, out) = apply_action(
            &s, &env(),
            &Action::PlayTrack { track_id: 2060 },
        );
        assert!(matches!(out, TransitionOutcome::Refused { .. }));
    }

    #[test]
    fn sink_disappeared_two_strike_watchdog() {
        // Start playing through BT, then simulate two consecutive
        // pactl None reads. Watchdog stops playback.
        let s = ControllerState::fresh_loaded();
        let (s, _) = apply_action(
            &s, &env(),
            &Action::SelectDevice { url: "http://192.168.2.14".into() },
        );
        let (s, _) = apply_action(
            &s, &env(),
            &Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
        );
        let (s, _) = apply_action(
            &s, &env(),
            &Action::PlayTrack { track_id: 2060 },
        );
        assert!(s.playing_format.is_some(), "should be playing");

        // First None → strike 1, still playing.
        let (s, _) = apply_action(
            &s, &env(),
            &Action::SimulatePactlVolumeRead { pct: None },
        );
        assert_eq!(s.sink_unreachable_strikes, 1);
        assert!(s.playing_format.is_some());

        // Second None → strike 2 → driver stopped.
        let (s, out) = apply_action(
            &s, &env(),
            &Action::SimulatePactlVolumeRead { pct: None },
        );
        assert!(matches!(out, TransitionOutcome::OkWithNote { .. }));
        assert!(s.playing_format.is_none(), "playback stopped");
        assert_eq!(s.sink_unreachable_strikes, 0, "strikes reset on stop");
    }

    #[test]
    fn volume_big_jump_first_read_held() {
        // First big-jump read holds rather than adopts. Mirrors the
        // real `driver_volume` outlier-confirmation logic.
        let mut s = ControllerState::fresh_loaded();
        s.volume = 30;
        s.current_driver_kind = DriverKind::Bluetooth;
        s.active_sink = ActiveSink::Bluetooth {
            mac: "x".into(), name: "x".into(),
        };
        s.playing_format = Some(AudioFormat::Opus);

        let (s2, _) = apply_action(
            &s, &env(),
            &Action::SimulatePactlVolumeRead { pct: Some(100) },
        );
        assert_eq!(s2.volume, 30,
                   "outlier must not yank slider on first read");
        assert_eq!(s2.volume_pending, Some(100));
    }

    #[test]
    fn volume_big_jump_adopts_on_confirmation() {
        // Two consecutive same-value big-jump reads → adopt.
        let mut s = ControllerState::fresh_loaded();
        s.volume = 30;
        s.current_driver_kind = DriverKind::Bluetooth;
        s.active_sink = ActiveSink::Bluetooth {
            mac: "x".into(), name: "x".into(),
        };
        s.playing_format = Some(AudioFormat::Opus);

        let (s, _) = apply_action(
            &s, &env(),
            &Action::SimulatePactlVolumeRead { pct: Some(95) },
        );
        let (s, _) = apply_action(
            &s, &env(),
            &Action::SimulatePactlVolumeRead { pct: Some(95) },
        );
        assert_eq!(s.volume, 95);
        assert!(s.volume_pending.is_none());
    }

    #[test]
    fn volume_mid_jump_adopted_immediately() {
        // Real-code behavior: any delta ≤ 50 adopts immediately.
        let mut s = ControllerState::fresh_loaded();
        s.volume = 20;
        s.current_driver_kind = DriverKind::Bluetooth;
        s.active_sink = ActiveSink::Bluetooth {
            mac: "x".into(), name: "x".into(),
        };
        s.playing_format = Some(AudioFormat::Opus);

        let (s, _) = apply_action(
            &s, &env(),
            &Action::SimulatePactlVolumeRead { pct: Some(45) },
        );
        assert_eq!(s.volume, 45,
                   "delta of 25 should adopt immediately, not hold");
        assert!(s.volume_pending.is_none());
    }

    #[test]
    fn persisted_sink_restored_on_redeselect() {
        let s = ControllerState::fresh_loaded();
        let url = "http://192.168.2.14".to_string();
        let (s, _) = apply_action(&s, &env(),
            &Action::SelectDevice { url: url.clone() });
        let (s, _) = apply_action(&s, &env(),
            &Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            });
        assert_eq!(s.current_driver_kind, DriverKind::Bluetooth);
        // Switch away.
        let (s, _) = apply_action(&s, &env(),
            &Action::SelectDevice {
                url: "http://192.168.2.12".into() });
        assert_ne!(s.current_driver_kind, DriverKind::Bluetooth);
        // Switch back — persisted choice restored.
        let (s, _) = apply_action(&s, &env(),
            &Action::SelectDevice { url: url.clone() });
        assert_eq!(s.current_driver_kind, DriverKind::Bluetooth);
        match &s.active_sink {
            ActiveSink::Bluetooth { mac, .. } => {
                assert_eq!(mac, "00:09:A7:77:CD:55");
            }
            other => panic!("expected restored BT, got {:?}", other),
        }
    }

    #[test]
    fn no_notify_load_drains_pending_only_for_unblocked() {
        let mut s = ControllerState::fresh_loaded();
        s.no_notify_loaded = false;
        s.pending_notify_subscriptions
            .insert("http://192.168.2.12".into());
        s.pending_notify_subscriptions
            .insert("http://other".into());
        let (s, _) = apply_action(
            &s, &env(),
            &Action::SimulateNoNotifyKvLoad {
                urls: vec!["http://192.168.2.12".into()],
            },
        );
        // Blocklisted URL got drained out of pending (because the
        // subscribe would have been refused). Other one also drained
        // (because it WAS dispatched — in the real code it goes to
        // the worker; in the model we just remove it from pending).
        assert!(s.no_notify_loaded);
        assert!(s.no_notify_devices.contains("http://192.168.2.12"));
        assert!(!s.pending_notify_subscriptions
                  .contains("http://192.168.2.12"));
    }
}
