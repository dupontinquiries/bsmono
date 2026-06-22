//! Deterministic-seed fuzz harness. Generates randomized but
//! reproducible action sequences against the simulator, checks
//! invariants after every transition, and prints the full trace if
//! anything breaks.
//!
//! Determinism is the key property: a failure with seed=42 always
//! reproduces with seed=42, so a `cargo test fuzz_walks` failure
//! gives you a concrete trace you can stick in `scenarios.rs` as a
//! permanent regression test.
//!
//! No external PRNG dependency — uses an `Lcg64Xsh32`-style linear
//! congruential generator inlined here. Good enough for fuzz
//! coverage; cryptographic strength irrelevant.

use crate::sim::action::Action;
use crate::sim::env::SimEnvironment;
use crate::sim::invariants::{check_invariants, InvariantViolation};
use crate::sim::state::ControllerState;
use crate::sim::transitions::{apply_action, TransitionOutcome};

/// Minimal PRNG — deterministic, fast, no deps. Used only for fuzz
/// sampling (not cryptography). Constants from PCG family.
struct Prng(u64);

impl Prng {
    fn new(seed: u64) -> Self {
        // Avoid the all-zeros state.
        Prng(seed.wrapping_add(0x9E37_79B9_7F4A_7C15))
    }
    fn next_u32(&mut self) -> u32 {
        // PCG-XSH-RR variant.
        let oldstate = self.0;
        self.0 = oldstate
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let xorshifted = (((oldstate >> 18) ^ oldstate) >> 27) as u32;
        let rot = (oldstate >> 59) as u32;
        xorshifted.rotate_right(rot)
    }
    fn pick<'a, T>(&mut self, slice: &'a [T]) -> Option<&'a T> {
        if slice.is_empty() {
            None
        } else {
            Some(&slice[(self.next_u32() as usize) % slice.len()])
        }
    }
    fn range(&mut self, lo: u32, hi_excl: u32) -> u32 {
        if hi_excl <= lo {
            return lo;
        }
        lo + (self.next_u32() % (hi_excl - lo))
    }
}

#[derive(Debug, Clone)]
pub struct FuzzCfg {
    pub seed: u64,
    pub n_walks: u32,
    pub steps_per_walk: u32,
    /// If true, prints every action of every walk (chatty — for
    /// debugging the fuzzer itself).
    pub verbose_walks: bool,
}

impl Default for FuzzCfg {
    fn default() -> Self {
        FuzzCfg {
            seed: 0xB00B_5A1E_C0DE_C0DEu64,
            n_walks: 100,
            steps_per_walk: 40,
            verbose_walks: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FuzzReport {
    pub walks_run: u32,
    pub total_steps: u32,
    /// First (seed, action-trace) pair that triggered an invariant
    /// violation, if any.
    pub first_failure: Option<FuzzFailure>,
}

#[derive(Debug, Clone)]
pub struct FuzzFailure {
    pub seed: u64,
    pub walk_index: u32,
    pub step_index: u32,
    pub violation: InvariantViolation,
    pub trace: Vec<(Action, TransitionOutcome)>,
}

impl std::fmt::Display for FuzzFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "── fuzz failure (seed={} walk={} step={}) ──",
            self.seed, self.walk_index, self.step_index)?;
        writeln!(f, "violation: {} — {}",
            self.violation.name, self.violation.detail)?;
        writeln!(f, "trace:")?;
        for (i, (a, o)) in self.trace.iter().enumerate() {
            writeln!(f, "  [{:>3}] {}    ↓ {:?}", i, a.label(), o)?;
        }
        Ok(())
    }
}

/// Generate one plausible action from the current state. The
/// distribution mixes user-driven actions (selection, binding,
/// playback, volume) with system events (pactl reads, BT power
/// toggles, notify-probe giveups, sink vanishes) at roughly the rate
/// you'd see in a real session. Tuned to maximize transitions per
/// step while keeping action validity high enough that the fuzzer
/// isn't just hammering refused-action returns.
fn gen_action(
    state: &ControllerState, env: &SimEnvironment, rng: &mut Prng,
) -> Action {
    let bucket = rng.range(0, 100);
    match bucket {
        // ── User-driven actions ──────────────────────────────
        0..=11 => {
            let urls: Vec<&String> = env.devices.keys().collect();
            if let Some(url) = rng.pick(&urls) {
                Action::SelectDevice { url: (*url).clone() }
            } else {
                Action::DriverStop
            }
        }
        12..=27 => {
            let macs: Vec<(&String, &String)> = env.speakers.iter()
                .map(|(k, v)| (k, &v.name))
                .collect();
            if let Some((mac, name)) = rng.pick(&macs) {
                Action::SetActiveBt {
                    mac: (*mac).clone(),
                    name: (*name).clone(),
                }
            } else {
                Action::DriverStop
            }
        }
        28..=35 => {
            // Multi-room. Pick 2-3 distinct speakers.
            let n = rng.range(2, 4) as usize;
            let all: Vec<&String> = env.speakers.keys().collect();
            if all.len() < n { return Action::ClearMultiRoom; }
            let mut chosen: Vec<String> = Vec::with_capacity(n);
            let mut tries = 0;
            while chosen.len() < n && tries < 12 {
                if let Some(mac) = rng.pick(&all) {
                    let mac = (*mac).clone();
                    if !chosen.contains(&mac) { chosen.push(mac); }
                }
                tries += 1;
            }
            if chosen.len() < 2 { return Action::ClearMultiRoom; }
            let names: Vec<String> = chosen.iter()
                .map(|m| env.speakers[m].name.clone())
                .collect();
            Action::SetMultiRoom { macs: chosen, names }
        }
        36..=39 => Action::ClearMultiRoom,
        40..=42 => Action::SelectLocalSink,
        43..=51 => {
            let tracks: Vec<&crate::sim::env::TrackProfile> =
                env.catalog.iter().collect();
            if let Some(t) = rng.pick(&tracks) {
                Action::PlayTrack { track_id: t.id }
            } else {
                Action::DriverStop
            }
        }
        52..=58 => {
            let v = rng.range(0, 101) as i32;
            Action::SetVolume { pct: v }
        }
        59..=60 => Action::ToggleMute,
        61..=63 => Action::DriverStop,
        64..=66 => Action::SimulatePlaybackEnd,

        // ── System events ────────────────────────────────────
        // 30% combined: pactl reads, BT power toggles, notify
        // events, sink vanishes. These are the actions that
        // exercise watchdog + ledger + persistence logic.
        67..=83 => {
            // pactl volume polls — 70% near-current, 20% None
            // (sink unreachable), 10% outlier.
            let r = rng.range(0, 100);
            if r < 70 {
                let near = state.volume + (rng.range(0, 5) as i32 - 2);
                Action::SimulatePactlVolumeRead {
                    pct: Some(near.clamp(0, 100)),
                }
            } else if r < 90 {
                Action::SimulatePactlVolumeRead { pct: None }
            } else {
                Action::SimulatePactlVolumeRead {
                    pct: Some(if r % 2 == 0 { 100 } else { 0 }),
                }
            }
        }
        84..=86 => {
            // User explicitly powers BT on (picker confirm-unlock).
            Action::UserSetBtPower { on: true }
        }
        87..=88 => {
            // Notify-probe give-up on a random known URL.
            let urls: Vec<&String> = env.devices.keys().collect();
            if let Some(url) = rng.pick(&urls) {
                Action::SimulateNotifyProbeGiveUp { url: (*url).clone() }
            } else {
                Action::DriverStop
            }
        }
        89..=90 => {
            // KV load arrives with the notify-blocklist. Usually
            // empty; occasionally pre-populated.
            let urls = if rng.range(0, 100) < 30 {
                env.devices.keys()
                    .take(1).cloned().collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            Action::SimulateNoNotifyKvLoad { urls }
        }
        91..=93 => {
            // Speaker's BT sink vanishes — primarily a state action;
            // the next pactl-None read is what produces the strike.
            let macs: Vec<&String> = env.speakers.keys().collect();
            if let Some(mac) = rng.pick(&macs) {
                Action::SimulateBtSinkVanish { mac: (*mac).clone() }
            } else {
                Action::DriverStop
            }
        }
        _ => Action::AppClose,
    }
}

/// Run a fuzz batch with the given configuration. Returns a report;
/// the caller decides whether to fail the test on `first_failure`.
pub fn run_fuzz(cfg: FuzzCfg, env: SimEnvironment) -> FuzzReport {
    let mut total_steps: u32 = 0;
    let mut first_failure: Option<FuzzFailure> = None;
    for walk_i in 0..cfg.n_walks {
        // Per-walk seed derived from the global seed + index, so
        // repro-by-index is straightforward. Use wrapping_mul because
        // walk_i as u64 * the golden-ratio constant overflows u64 in
        // debug builds for any non-zero walk_i; wrapping is the
        // intended arithmetic anyway (we just want a well-spread
        // perturbation, not actual multiplication).
        let walk_seed = cfg.seed.wrapping_add(
            (walk_i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        );
        let mut rng = Prng::new(walk_seed);

        let mut state = ControllerState::fresh_loaded();
        let mut trace: Vec<(Action, TransitionOutcome)> = Vec::new();

        for step_i in 0..cfg.steps_per_walk {
            let action = gen_action(&state, &env, &mut rng);
            let (next, outcome) = apply_action(&state, &env, &action);
            if cfg.verbose_walks {
                eprintln!("[walk {} step {}] {}  → {:?}",
                    walk_i, step_i, action.label(), outcome);
            }
            trace.push((action, outcome));
            state = next;
            total_steps += 1;

            // Check invariants after every transition.
            if let Err(viol) = check_invariants(&state, &env) {
                let f = FuzzFailure {
                    seed: walk_seed,
                    walk_index: walk_i,
                    step_index: step_i,
                    violation: viol,
                    trace,
                };
                first_failure = Some(f);
                return FuzzReport {
                    walks_run: walk_i + 1,
                    total_steps,
                    first_failure,
                };
            }
        }
    }
    FuzzReport {
        walks_run: cfg.n_walks,
        total_steps,
        first_failure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzz_two_devices_four_speakers_holds_invariants() {
        // 100 walks × 40 steps = 4000 transitions per run. Should
        // complete in <100ms and find no violations.
        let report = run_fuzz(
            FuzzCfg::default(),
            SimEnvironment::two_devices_four_speakers(),
        );
        if let Some(f) = report.first_failure {
            panic!("\n{}", f);
        }
        assert!(report.total_steps > 0);
    }

    #[test]
    fn fuzz_one_device_two_speakers_holds_invariants() {
        let report = run_fuzz(
            FuzzCfg {
                seed: 0xDEAD_BEEF_CAFE_F00Du64,
                n_walks: 50,
                steps_per_walk: 30,
                verbose_walks: false,
            },
            SimEnvironment::one_device_two_speakers(),
        );
        if let Some(f) = report.first_failure {
            panic!("\n{}", f);
        }
    }

    #[test]
    fn fuzz_with_bt_powered_off_holds_invariants() {
        // BT off at startup. Test that the model never lets state
        // drift into a broken configuration even when actions can't
        // succeed.
        let mut env = SimEnvironment::two_devices_four_speakers();
        env.initial_bt_powered = false;
        env.bt_currently_powered = false;
        let report = run_fuzz(
            FuzzCfg {
                seed: 0xC0FFEEu64,
                n_walks: 50,
                steps_per_walk: 30,
                ..FuzzCfg::default()
            },
            env,
        );
        if let Some(f) = report.first_failure {
            panic!("\n{}", f);
        }
    }

    #[test]
    fn fuzz_reproducibility() {
        // Same seed → same trace. Property required for any future
        // bug found via fuzz to be repro-able.
        let cfg = FuzzCfg {
            seed: 12345,
            n_walks: 5,
            steps_per_walk: 20,
            verbose_walks: false,
        };
        let env = SimEnvironment::one_device_two_speakers();
        let r1 = run_fuzz(cfg.clone(), env.clone());
        let r2 = run_fuzz(cfg, env);
        assert_eq!(r1.total_steps, r2.total_steps);
        // Either both pass or both fail with the same trace.
        match (&r1.first_failure, &r2.first_failure) {
            (None, None) => {},
            (Some(a), Some(b)) => {
                assert_eq!(a.seed, b.seed);
                assert_eq!(a.walk_index, b.walk_index);
                assert_eq!(a.step_index, b.step_index);
            }
            other => panic!("reproducibility broken: {:?}", other),
        }
    }

    /// Deep fuzz: 500 walks × 100 steps = 50,000 transitions across
    /// the two-device/four-speaker topology. Slow enough to gate
    /// behind a release-mode test profile in the future but fast
    /// enough (~2s) to keep on the default test set.
    ///
    /// The shallower fuzz tests above run on the same model but with
    /// modest step counts. This one is the wide net: anything subtle
    /// that needs many transitions to surface (state-machine deep
    /// cycles, ledger drift, persistence churn) shows up here.
    #[test]
    fn fuzz_deep_two_devices_four_speakers() {
        let report = run_fuzz(
            FuzzCfg {
                seed: 0xFEED_F00D_BAAD_CAFEu64,
                n_walks: 500,
                steps_per_walk: 100,
                verbose_walks: false,
            },
            SimEnvironment::two_devices_four_speakers(),
        );
        if let Some(f) = report.first_failure {
            panic!("\n{}", f);
        }
        // 50,000 transitions completed without invariant violation.
        assert_eq!(report.total_steps, 500 * 100);
    }

    /// Multi-seed sweep — runs 8 distinct seeds against the heavy
    /// scenario to catch invariant violations that are sensitive to
    /// the exact action interleaving. A single seed might miss a
    /// rare edge case that another lands on; the sweep is cheap and
    /// strengthens the floor.
    #[test]
    fn fuzz_multi_seed_sweep() {
        let seeds: [u64; 8] = [
            0x0000_0000_0000_0001,
            0xFFFF_FFFF_FFFF_FFFF,
            0xDEAD_BEEF_CAFE_BABE,
            0x1234_5678_9ABC_DEF0,
            0x0BAD_FEED_FACE_C0DE,
            0x8675_3090_8675_3090,
            0xC001_C0DE_C001_C0DE,
            0xAAAA_5555_AAAA_5555,
        ];
        for seed in seeds {
            let report = run_fuzz(
                FuzzCfg {
                    seed,
                    n_walks: 30,
                    steps_per_walk: 60,
                    verbose_walks: false,
                },
                SimEnvironment::two_devices_four_speakers(),
            );
            if let Some(f) = report.first_failure {
                panic!("seed {:#x} found a violation:\n{}", seed, f);
            }
        }
    }

    /// Pathological setup: ONLY the unreliable speaker is paired.
    /// Every SetActiveBt and SetMultiRoom should refuse cleanly
    /// without ever putting state into an incoherent configuration.
    /// Catches the case where a session-of-failures somehow drifts.
    #[test]
    fn fuzz_only_unreliable_speaker() {
        let mut env = SimEnvironment::two_devices_four_speakers();
        env.speakers.clear();
        env.speakers.insert(
            "FF:FF:FF:FF:FF:FF".into(),
            crate::sim::env::SpeakerProfile::unreliable(
                "FF:FF:FF:FF:FF:FF", "AlwaysFails",
            ),
        );
        let report = run_fuzz(
            FuzzCfg {
                seed: 0xBEEF_BEEF_BEEF_BEEFu64,
                n_walks: 80,
                steps_per_walk: 50,
                verbose_walks: false,
            },
            env,
        );
        if let Some(f) = report.first_failure {
            panic!("\n{}", f);
        }
    }

    /// Environment-mutating walk. After each ~10 steps, the BT power
    /// state flips and a random speaker's PA visibility toggles.
    /// Exercises the controller's resilience to external state
    /// changes happening between actions.
    #[test]
    fn fuzz_with_environment_churn() {
        let base = SimEnvironment::two_devices_four_speakers();
        let speakers: Vec<String> = base.speakers.keys().cloned().collect();
        let mut env = base;
        let mut rng = Prng::new(0xCAFE_F00D);
        let mut state = ControllerState::fresh_loaded();

        for step in 0..400 {
            // Every 10 steps, churn the environment.
            if step > 0 && step % 10 == 0 {
                let r = rng.range(0, 100);
                if r < 30 {
                    env.bt_currently_powered = !env.bt_currently_powered;
                } else if r < 60 && !speakers.is_empty() {
                    if let Some(mac) = rng.pick(&speakers) {
                        if let Some(sp) = env.speakers.get_mut(mac) {
                            sp.pa_sink_visible = !sp.pa_sink_visible;
                        }
                    }
                }
            }
            let action = gen_action(&state, &env, &mut rng);
            let (next, _) = apply_action(&state, &env, &action);
            state = next;
            if let Err(v) = check_invariants(&state, &env) {
                panic!(
                    "violation '{}' at churn-step {}: {}\nlast action: {}",
                    v.name, step, v.detail, action.label(),
                );
            }
        }
    }

    /// MEGA-fuzz: 10,000 walks × 100 steps = **1 million transitions**.
    /// Catches violations that need the rarest action interleavings.
    /// Runs in ~3-4 seconds (the model is pure functions; everything
    /// fits in CPU caches). Slow enough that gated builds may skip
    /// it via #[ignore], but fast enough to keep in the default set
    /// for now — the wall-clock cost is worth the coverage.
    #[test]
    fn fuzz_mega_two_devices_four_speakers() {
        let report = run_fuzz(
            FuzzCfg {
                seed: 0x1234_5678_9ABC_DEF0,
                n_walks: 10_000,
                steps_per_walk: 100,
                verbose_walks: false,
            },
            SimEnvironment::two_devices_four_speakers(),
        );
        if let Some(f) = report.first_failure {
            panic!("\n{}", f);
        }
        assert_eq!(report.total_steps, 10_000 * 100);
    }

    /// Cross-seed mega sweep: 16 distinct seeds × 200 walks × 50
    /// steps = 160,000 transitions across many starting conditions.
    /// Catches seed-sensitive bugs the single-seed mega run could
    /// miss.
    #[test]
    fn fuzz_cross_seed_mega_sweep() {
        let seeds: [u64; 16] = [
            0x0000_0000_0000_0000,    0xFFFF_FFFF_FFFF_FFFF,
            0x5555_5555_5555_5555,    0xAAAA_AAAA_AAAA_AAAA,
            0xDEAD_BEEF_DEAD_BEEF,    0xFEED_FACE_FEED_FACE,
            0xCAFE_BABE_CAFE_BABE,    0x1337_C0DE_1337_C0DE,
            0x0123_4567_89AB_CDEF,    0xFEDC_BA98_7654_3210,
            0xC001_DEAD_C001_DEAD,    0xBADC_0FFE_BADC_0FFE,
            0x8675_3090_0309_5768,    0x9999_8888_7777_6666,
            0x4242_4242_4242_4242,    0xF00D_BEEF_F00D_BEEF,
        ];
        for seed in seeds {
            let report = run_fuzz(
                FuzzCfg {
                    seed,
                    n_walks: 200,
                    steps_per_walk: 50,
                    verbose_walks: false,
                },
                SimEnvironment::two_devices_four_speakers(),
            );
            if let Some(f) = report.first_failure {
                panic!("seed {:#x} found a violation:\n{}", seed, f);
            }
        }
    }

    #[test]
    fn fuzz_intentionally_buggy_state_caught() {
        // Sanity: if we hand the invariants a state that violates
        // one, the fuzz harness (well, the underlying check) flags
        // it. This guards against the harness being a no-op.
        let mut s = ControllerState::fresh_loaded();
        s.current_driver_kind = crate::sim::state::DriverKind::Bluetooth;
        s.active_sink = crate::sim::state::ActiveSink::None;
        let env = SimEnvironment::default();
        let err = check_invariants(&s, &env);
        assert!(err.is_err(), "harness must catch obvious violations");
    }
}
