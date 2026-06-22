//! Hand-crafted action sequences derived from real bug reports and
//! observed-behavior cases. Each scenario corresponds to a class of
//! defect the controller has hit at least once.
//!
//! These complement `fuzz.rs`: random walks find new bugs, scenarios
//! lock in the specific cases that have already been fixed. Together
//! they form a defense-in-depth regression net.
//!
//! Naming convention: `scenario_<bug_class>__<short_description>()`.

use crate::sim::action::Action;
use crate::sim::env::*;
use crate::sim::invariants::check_invariants;
use crate::sim::state::*;
use crate::sim::transitions::apply_action;

/// Run a scenario: apply each action, check invariants after each,
/// return the final state. Panics with a descriptive message on the
/// first invariant violation.
fn run(
    env: &SimEnvironment,
    actions: &[Action],
) -> ControllerState {
    let mut state = ControllerState::fresh_loaded();
    for (i, action) in actions.iter().enumerate() {
        let (next, outcome) = apply_action(&state, env, action);
        state = next;
        if let Err(v) = check_invariants(&state, env) {
            panic!(
                "scenario invariant '{}' violated at step {} ({}): {}\n\
                 outcome: {:?}",
                v.name, i, action.label(), v.detail, outcome,
            );
        }
    }
    state
}

#[cfg(test)]
// Test names use a `category__detail` convention with double
// underscores to read as two labelled segments — easier to scan in
// a long list than `category_detail` where the segment boundary
// disappears. rustc flags double underscores as non-snake-case;
// silence at the module boundary rather than per-function.
#[allow(non_snake_case)]
mod tests {
    use super::*;

    /// User's reproducible flow: pick the Stage, bind Beosound
    /// Emerge over BT, play an opus track. Audio should route
    /// through the BT driver, not Stage's REST.
    #[test]
    fn scenario_user_main_flow__stage_to_bt_emerge_opus() {
        let env = SimEnvironment::two_devices_four_speakers();
        let actions = vec![
            Action::SelectDevice {
                url: "http://192.168.2.12".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:77:C2:88".into(),
                name: "Beosound Emerge-36184504".into(),
            },
            Action::PlayTrack { track_id: 2060 },
        ];
        let s = run(&env, &actions);
        assert_eq!(s.current_driver_kind, DriverKind::Bluetooth);
        assert!(s.playing_format.is_some());
        // The watchdog hadn't tripped.
        assert_eq!(s.sink_unreachable_strikes, 0);
    }

    /// Three-speaker multi-room attempt where one speaker (Smokey)
    /// isn't PA-visible. Should refuse cleanly with a friendly error
    /// naming the missing speaker — NOT the cryptic "Input/Output
    /// error" the user originally saw.
    #[test]
    fn scenario_multiroom_with_offline_member__refuses_cleanly() {
        let env = SimEnvironment::two_devices_four_speakers();
        let actions = vec![
            Action::SelectDevice {
                url: "http://192.168.2.14".into(),
            },
            // Each of the three members in advance — the friendly
            // first two connect.
            Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:77:C2:88".into(),
                name: "Beosound Emerge-36184504".into(),
            },
            Action::SetMultiRoom {
                macs: vec![
                    "00:09:A7:77:CD:55".into(),
                    "00:09:A7:77:C2:88".into(),
                    "50:1E:2D:5B:E6:EF".into(),
                ],
                names: vec![
                    "Emerge01".into(),
                    "Beosound Emerge-36184504".into(),
                    "Smokey".into(),
                ],
            },
        ];
        let s = run(&env, &actions);
        // The refused multi-room must NOT have polluted state.
        assert_ne!(s.current_driver_kind, DriverKind::MultiRoom);
        // Last successful bind was Beosound Emerge-36184504.
        match &s.active_sink {
            ActiveSink::Bluetooth { mac, .. } => {
                assert_eq!(mac, "00:09:A7:77:C2:88");
            }
            other => panic!("expected single BT, got {:?}", other),
        }
    }

    /// Sink-disappeared watchdog regression case. User reported:
    /// "turning off the bluetooth started playing the audio on my
    /// laptop speakers." Two consecutive None reads should stop
    /// playback before PA can migrate.
    #[test]
    fn scenario_sink_disappeared__two_strikes_stops_audio() {
        let env = SimEnvironment::two_devices_four_speakers();
        let s = run(&env, &[
            Action::SelectDevice {
                url: "http://192.168.2.14".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
            Action::PlayTrack { track_id: 8381 },
        ]);
        assert!(s.playing_format.is_some(), "should be playing first");

        // Two consecutive None reads — sink gone.
        let (s, _) = apply_action(&s, &env,
            &Action::SimulatePactlVolumeRead { pct: None });
        let (s, _) = apply_action(&s, &env,
            &Action::SimulatePactlVolumeRead { pct: None });

        assert!(s.playing_format.is_none(),
                "two-strike watchdog should have stopped playback");
        assert_eq!(s.sink_unreachable_strikes, 0,
                   "strikes reset after stop");
    }

    /// Volume outlier filter. The pactl read of 100 right after a BT
    /// renegotiation must not yank the slider from 30 to 100 on the
    /// FIRST read. The real handler holds the candidate for the next
    /// confirmation cycle, so a transient blip self-corrects rather
    /// than becoming permanent. (The user's log showed "sync slider
    /// 21 → 100" being adopted only after a second consistent read.)
    #[test]
    fn scenario_volume_outlier__held_then_resolves() {
        let env = SimEnvironment::two_devices_four_speakers();
        let s = run(&env, &[
            Action::SelectDevice {
                url: "http://192.168.2.14".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
            Action::PlayTrack { track_id: 8381 },
            Action::SetVolume { pct: 30 },
        ]);
        assert_eq!(s.volume, 30);

        // First read of 100 (delta 70) → held, slider stays.
        let (s, _) = apply_action(&s, &env,
            &Action::SimulatePactlVolumeRead { pct: Some(100) });
        assert_eq!(s.volume, 30,
                   "first big-jump read must NOT yank the slider");
        assert_eq!(s.volume_pending, Some(100));

        // Next read of 32 (normal, ≤50 delta from 30) → adopt 32,
        // clear pending. The held outlier was correctly transient.
        let (s, _) = apply_action(&s, &env,
            &Action::SimulatePactlVolumeRead { pct: Some(32) });
        assert_eq!(s.volume, 32);
        assert!(s.volume_pending.is_none(),
                "non-confirming read clears pending");
    }

    /// Per-device sink persistence. Bind BT for Emerge device,
    /// switch to Stage, switch BACK — Emerge's BT binding should
    /// restore automatically. The user shouldn't have to re-pick
    /// every device switch.
    #[test]
    fn scenario_device_swap__sink_choice_persists() {
        let env = SimEnvironment::two_devices_four_speakers();
        let emerge = "http://192.168.2.14".to_string();
        let stage  = "http://192.168.2.12".to_string();
        let s = run(&env, &[
            Action::SelectDevice { url: emerge.clone() },
            Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
            Action::SelectDevice { url: stage.clone() },
            Action::SelectDevice { url: emerge.clone() },
        ]);
        assert_eq!(s.current_driver_kind, DriverKind::Bluetooth);
        match &s.active_sink {
            ActiveSink::Bluetooth { mac, .. } => {
                assert_eq!(mac, "00:09:A7:77:CD:55");
            }
            other => panic!("expected restored BT, got {:?}", other),
        }
    }

    /// The "Smokey" connect-failure case. Past versions of the
    /// ledger incorrectly added failed connects to we_connected,
    /// which led to phantom disconnects at app close. Must not
    /// happen.
    #[test]
    fn scenario_connect_failure_doesnt_pollute_ledger() {
        let env = SimEnvironment::two_devices_four_speakers();
        let s = run(&env, &[
            Action::SelectDevice {
                url: "http://192.168.2.14".into(),
            },
            // Bind succeeds.
            Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
            // Then attempt the unreliable speaker — refuse.
            Action::SetActiveBt {
                mac: "50:1E:2D:5B:E6:EF".into(),
                name: "Smokey".into(),
            },
        ]);
        let cleanup = s.bt_respect.macs_to_disconnect();
        assert!(cleanup.contains("00:09:A7:77:CD:55"),
                "Emerge01 succeeded — should be in cleanup");
        assert!(!cleanup.contains("50:1E:2D:5B:E6:EF"),
                "Smokey failed to connect — must NOT be in cleanup");
    }

    /// BT-power was off at startup; user turns it on; binds a speaker;
    /// plays; closes app. The cleanup should disconnect the speaker
    /// AND restore the BT-off state.
    #[test]
    fn scenario_bt_off_user_turns_on_then_closes__restores_off() {
        let mut env = SimEnvironment::two_devices_four_speakers();
        env.initial_bt_powered = false;
        env.bt_currently_powered = false;

        let s = ControllerState::fresh_loaded();
        let s = {
            let mut s = s;
            // Model the initial snapshot. The real code does this
            // via spawn_bt_power_probe(true); we shortcut here.
            s.bt_respect.initial_bt_powered = Some(false);
            s
        };

        // User turns on system BT via the picker confirm-unlock.
        let (s, _) = apply_action(&s, &env,
            &Action::UserSetBtPower { on: true });
        assert!(s.bt_respect.we_powered_on);

        // Reflect environment change.
        let mut env = env.clone();
        env.bt_currently_powered = true;

        // Bind speaker + play.
        let (s, _) = apply_action(&s, &env,
            &Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            });
        let (s, _) = apply_action(&s, &env,
            &Action::SelectDevice {
                url: "http://192.168.2.14".into() });
        let (s, _) = apply_action(&s, &env,
            &Action::PlayTrack { track_id: 2060 });

        // App close — verify cleanup decisions.
        let cleanup = s.bt_respect.macs_to_disconnect();
        assert_eq!(cleanup.len(), 1);
        assert!(cleanup.contains("00:09:A7:77:CD:55"));
        assert!(s.bt_respect.should_restore_power_off(),
                "we turned BT on AND it was off at startup → restore");
    }

    /// no_notify persistence load-ordering regression. First-session:
    /// device_notify gives up on http://192.168.2.12. Persisted.
    /// Second session: KV load fires AFTER refresh_all's initial
    /// subscribe. We must NOT re-probe the device.
    #[test]
    fn scenario_no_notify_persistence__second_session_skips_probe() {
        let env = SimEnvironment::two_devices_four_speakers();

        // Second session: state starts with no_notify_loaded=false,
        // pending subscriptions queued, then load fires.
        let mut s = ControllerState::default();
        s.pending_notify_subscriptions
            .insert("http://192.168.2.12".into());

        let (s, _) = apply_action(&s, &env,
            &Action::SimulateNoNotifyKvLoad {
                urls: vec!["http://192.168.2.12".into()],
            });

        assert!(s.no_notify_loaded);
        // The pending URL was blocklisted — drained out, NOT
        // dispatched.
        assert!(!s.pending_notify_subscriptions
                .contains("http://192.168.2.12"));
    }

    /// Mid/Side mixer page doesn't actually appear in the state
    /// model since it's pure DSP. But this scenario captures a
    /// general "state mutation through 30 actions still well-formed."
    /// Acts as a smoke test that long sessions don't accumulate
    /// drift.
    #[test]
    fn scenario_long_session__30_random_state_changes_stable() {
        let env = SimEnvironment::two_devices_four_speakers();
        let s = run(&env, &[
            Action::SelectDevice {
                url: "http://192.168.2.12".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
            Action::PlayTrack { track_id: 2060 },
            Action::SetVolume { pct: 45 },
            Action::SetVolume { pct: 55 },
            Action::ToggleMute,
            Action::ToggleMute,
            Action::SimulatePlaybackEnd,
            Action::PlayTrack { track_id: 8381 },
            Action::SetActiveBt {
                mac: "00:09:A7:77:C2:88".into(),
                name: "Beosound Emerge".into(),
            },
            Action::PlayTrack { track_id: 7947 },
            Action::SelectDevice {
                url: "http://192.168.2.14".into(),
            },
            Action::SetMultiRoom {
                macs: vec![
                    "00:09:A7:77:CD:55".into(),
                    "00:09:A7:77:C2:88".into(),
                ],
                names: vec![
                    "Emerge01".into(),
                    "Beosound Emerge".into(),
                ],
            },
            Action::PlayTrack { track_id: 999_002 },
            Action::SimulatePactlVolumeRead { pct: Some(56) },
            Action::SetVolume { pct: 60 },
            Action::SetVolume { pct: 65 },
            Action::ClearMultiRoom,
            Action::SimulatePlaybackEnd,
            Action::SelectDevice {
                url: "http://192.168.2.12".into(),
            },
            Action::SimulatePactlVolumeRead { pct: Some(67) },
            Action::DriverStop,
            Action::SetActiveBt {
                mac: "00:09:A7:8A:94:64".into(),
                name: "Beoplay H95".into(),
            },
            Action::PlayTrack { track_id: 999_003 },
            Action::SimulatePactlVolumeRead { pct: Some(70) },
            Action::SetVolume { pct: 50 },
            Action::SimulatePlaybackEnd,
            Action::SelectLocalSink,
            Action::PlayTrack { track_id: 999_001 },
            Action::AppClose,
        ]);

        // Final state should still be coherent — invariants checked
        // throughout via `run()`. Just spot-check a few things.
        assert!(s.volume >= 0 && s.volume <= 100);
        assert!(s.sink_unreachable_strikes >= 0
                && s.sink_unreachable_strikes <= 2);
    }

    /// All audio formats produce a routable playback. The fuzz
    /// harness covers this incidentally; this scenario makes the
    /// matrix explicit.
    #[test]
    fn scenario_every_audio_format__routes_through_bt() {
        let env = SimEnvironment::two_devices_four_speakers();
        let setup = vec![
            Action::SelectDevice {
                url: "http://192.168.2.14".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
        ];
        let s = run(&env, &setup);
        for track in &env.catalog {
            let (s2, outcome) = apply_action(&s, &env,
                &Action::PlayTrack { track_id: track.id });
            assert_eq!(
                outcome,
                crate::sim::transitions::TransitionOutcome::Ok,
                "format {:?} (track {}) should route through BT",
                track.format, track.id,
            );
            assert!(s2.playing_format.is_some());
            // Watchdog stays clean across format changes.
            assert_eq!(s2.sink_unreachable_strikes, 0);
        }
    }

    /// Rapid speaker cycling: A → B → A → B → A → cleanup.
    /// We should converge to a coherent we_connected set: all 2
    /// distinct MACs we tried, despite 5 binding flips.
    #[test]
    fn scenario_rapid_speaker_cycling__ledger_converges() {
        let env = SimEnvironment::two_devices_four_speakers();
        let s = run(&env, &[
            Action::SelectDevice {
                url: "http://192.168.2.14".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:77:C2:88".into(),
                name: "Beosound Emerge-36184504".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:77:C2:88".into(),
                name: "Beosound Emerge-36184504".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
        ]);
        // Both MACs were successfully connected at some point — both
        // should be in we_connected (the ledger doesn't decay; it's a
        // historical record of "we caused these connections this
        // session").
        assert!(s.bt_respect.we_connected.contains("00:09:A7:77:CD:55"));
        assert!(s.bt_respect.we_connected.contains("00:09:A7:77:C2:88"));
        // The active sink is the LAST bind.
        match &s.active_sink {
            ActiveSink::Bluetooth { mac, .. } => {
                assert_eq!(mac, "00:09:A7:77:CD:55");
            }
            other => panic!("expected last BT bind, got {:?}", other),
        }
    }

    /// Bind multi-room of 4 speakers, then disband while playing.
    /// Playback should stop (combine-sink torn down) AND all four
    /// MACs should be in we_connected (we caused them all to be
    /// connected for the combine sink). Cleanup math on AppClose
    /// should match this.
    #[test]
    fn scenario_four_speaker_multiroom__full_cleanup() {
        let env = SimEnvironment::two_devices_four_speakers();
        let s = run(&env, &[
            Action::SelectDevice {
                url: "http://192.168.2.12".into(),
            },
            // First bind each speaker to seed connectability.
            Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:77:C2:88".into(),
                name: "Beosound Emerge-36184504".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:8A:94:64".into(),
                name: "Beoplay H95".into(),
            },
            // The four-speaker combine (excluding unreliable Smokey).
            Action::SetMultiRoom {
                macs: vec![
                    "00:09:A7:77:CD:55".into(),
                    "00:09:A7:77:C2:88".into(),
                    "00:09:A7:8A:94:64".into(),
                ],
                names: vec![
                    "Emerge01".into(),
                    "Beosound Emerge-36184504".into(),
                    "Beoplay H95".into(),
                ],
            },
            Action::PlayTrack { track_id: 2060 },
            // Disband.
            Action::ClearMultiRoom,
        ]);
        // After clear, we're on Noop (mozart_legacy device) and not
        // playing.
        assert_eq!(s.current_driver_kind, DriverKind::Noop);
        assert!(s.playing_format.is_none(),
                "playback should stop when multi-room disbands on legacy");
        // we_connected has the three successfully-bound macs.
        assert!(s.bt_respect.we_connected.contains("00:09:A7:77:CD:55"));
        assert!(s.bt_respect.we_connected.contains("00:09:A7:77:C2:88"));
        assert!(s.bt_respect.we_connected.contains("00:09:A7:8A:94:64"));
        // The unreliable speaker was never connected so it must NOT
        // be in we_connected.
        assert!(!s.bt_respect.we_connected.contains("50:1E:2D:5B:E6:EF"));
    }

    /// The Smokey-in-PA-table churn case from the fuzz harness,
    /// distilled to a hand-crafted scenario. Even if a speaker's PA
    /// sink becomes visible (race with external app, prior session
    /// residue, etc.), we MUST NOT credit ourselves with connecting
    /// it when its connect_outcome is Fails.
    #[test]
    fn scenario_externally_visible_unreliable_speaker__not_credited() {
        let mut env = SimEnvironment::two_devices_four_speakers();
        // Pretend Smokey's PA sink is somehow visible (maybe a prior
        // session left it). Connect outcome remains Fails — calling
        // bluetoothctl connect on it would still fail.
        env.speakers.get_mut("50:1E:2D:5B:E6:EF").unwrap()
            .pa_sink_visible = true;
        let s = run(&env, &[
            Action::SelectDevice {
                url: "http://192.168.2.14".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
            // Build a multi-room INCLUDING the externally-visible
            // unreliable speaker.
            Action::SetMultiRoom {
                macs: vec![
                    "00:09:A7:77:CD:55".into(),
                    "50:1E:2D:5B:E6:EF".into(),
                ],
                names: vec![
                    "Emerge01".into(),
                    "Smokey".into(),
                ],
            },
        ]);
        // Despite Smokey appearing in PA, it must NOT be in we_connected
        // because we couldn't have connected it (its outcome is Fails).
        assert!(!s.bt_respect.we_connected.contains("50:1E:2D:5B:E6:EF"),
                "must not credit ourselves with a connection we can't \
                 actually make: we_connected = {:?}",
                s.bt_respect.we_connected);
        // The successful member IS in we_connected.
        assert!(s.bt_respect.we_connected.contains("00:09:A7:77:CD:55"));
    }

    /// External BT power-off while a multi-room is playing. In reality
    /// the combine-sink loses all its members, paplay sees closed
    /// stdin, exits. Our watchdog catches the missing sink within
    /// two strikes and stops playback.
    #[test]
    fn scenario_external_bt_power_off_mid_playback__watchdog_stops() {
        let mut env = SimEnvironment::two_devices_four_speakers();
        let setup = vec![
            Action::SelectDevice {
                url: "http://192.168.2.14".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:77:C2:88".into(),
                name: "Beosound Emerge-36184504".into(),
            },
            Action::SetMultiRoom {
                macs: vec![
                    "00:09:A7:77:CD:55".into(),
                    "00:09:A7:77:C2:88".into(),
                ],
                names: vec![
                    "Emerge01".into(),
                    "Beosound Emerge".into(),
                ],
            },
            Action::PlayTrack { track_id: 2060 },
        ];
        let s = run(&env, &setup);
        assert!(s.playing_format.is_some());
        assert_eq!(s.current_driver_kind, DriverKind::MultiRoom);

        // External actor (settings app, etc.) turns BT power off.
        // Both speakers' sinks vanish from PA.
        env.bt_currently_powered = false;
        for sp in env.speakers.values_mut() {
            sp.pa_sink_visible = false;
        }

        // Two pactl polls return None → watchdog stops playback.
        let (s, _) = apply_action(&s, &env,
            &Action::SimulatePactlVolumeRead { pct: None });
        let (s, _) = apply_action(&s, &env,
            &Action::SimulatePactlVolumeRead { pct: None });
        assert!(s.playing_format.is_none(),
                "watchdog must stop playback when sinks vanish");
        assert_eq!(s.sink_unreachable_strikes, 0,
                   "strikes reset on auto-stop");
    }

    /// Sequence the user actually does: play music through BT, then
    /// switch device while playing. The OLD BT sink should be
    /// invalidated. New device gets a fresh routing decision based
    /// on its persisted sink (or default kind).
    #[test]
    fn scenario_device_swap_while_playing__clean_handoff() {
        let env = SimEnvironment::two_devices_four_speakers();
        let s = run(&env, &[
            Action::SelectDevice {
                url: "http://192.168.2.14".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
            Action::PlayTrack { track_id: 2060 },
        ]);
        assert!(s.playing_format.is_some());

        // Switch to the other device WHILE PLAYING.
        let (s, _) = apply_action(&s, &env,
            &Action::SelectDevice {
                url: "http://192.168.2.12".into() });
        // The model's SelectDevice should have cleared playing_format
        // (we're not playing the same audio anywhere now — driver
        // changed and the old paplay would have been killed).
        assert!(s.playing_format.is_none(),
                "device switch must invalidate playing state");
        assert_eq!(s.active_url, "http://192.168.2.12");
        // The Stage device has no persisted sink; default for legacy
        // is Noop.
        assert_eq!(s.current_driver_kind, DriverKind::Noop);
    }

    /// Rapid mute-toggle. Mute is an immediate state flip. After 20
    /// flips, state must be coherent — no drift, no stuck-mute.
    #[test]
    fn scenario_rapid_mute_toggle__no_drift() {
        let env = SimEnvironment::two_devices_four_speakers();
        let s = run(&env, &[
            Action::SelectDevice {
                url: "http://192.168.2.14".into(),
            },
            Action::SetActiveBt {
                mac: "00:09:A7:77:CD:55".into(),
                name: "Emerge01".into(),
            },
            Action::SetVolume { pct: 40 },
        ]);
        assert_eq!(s.volume, 40);
        assert!(!s.muted);

        let mut s = s;
        for _ in 0..20 {
            let (next, _) = apply_action(&s, &env, &Action::ToggleMute);
            s = next;
        }
        // 20 toggles → even count → unmuted again.
        assert!(!s.muted);
        // Volume should be unchanged through the toggles.
        assert_eq!(s.volume, 40);
    }
}
