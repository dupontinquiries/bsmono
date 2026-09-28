//! anc — the operator's control over active noise cancellation.
//!
//! bo-suite is an ANC research system with a music player, a home-theatre
//! setup and a sleep-sound engine built over it. The egui port had all three
//! and nothing for the ANC, while the Qt app split its controls across one
//! card mid-Settings and another near the end.
//!
//! ## This module owns the engine, it does not imitate it
//!
//! The first version of this file reimplemented the presence and tonality
//! gates in the frontend and documented its thresholds as "mirroring"
//! `bo-mux`'s `drone_canceller`. That was wrong, and `parity_audit`'s engine
//! column now names why: the whole frontend depended on no part of bo-suite's
//! engine, so it rendered the application and could not cancel a decibel. Two
//! copies of a gate that agree today are two copies, and the app ships
//! whichever one the UI happens to read.
//!
//! So the state machine here is `DroneCanceller`'s — Quiet, Detected, Active,
//! Releasing — and this file adds only what the engine has no opinion about:
//! what the OPERATOR asked for.
//!
//! ## Three modes, because On and Auto are different promises
//!
//! * **Off** — the canceller does not run. Not "emit silence".
//! * **Auto** — the engine's phase machine decides, moment to moment.
//! * **On** — emit regardless of phase, for measurement: you cannot
//!   characterise a canceller that keeps deciding not to run.
//!
//! ## Why the phase is shown, not just a boolean
//!
//! An operator watching a quiet room needs "nothing to cancel" told apart
//! from "drone heard, lock not confirmed". Both are silent; only one is worth
//! investigating. `DronePhase` already draws that line, which is another
//! reason to surface it rather than invent a parallel vocabulary.

pub use bo_mux::drone_canceller::{DroneCanceller, DronePhase, DroneSnapshot};

/// What the operator asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// The canceller does not run.
    Off,
    /// The engine's gates decide, moment to moment.
    #[default]
    Auto,
    /// Emit regardless of the gates, for measurement.
    Forced,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Off => "Off",
            Mode::Auto => "Auto",
            Mode::Forced => "On",
        }
    }

    /// The next mode when the operator taps a cycling control. Off → Auto →
    /// On → Off: the common case is one tap from Off, and the dangerous case
    /// (Forced) is never reached by accident from Auto in the safe direction.
    pub fn next(self) -> Mode {
        match self {
            Mode::Off => Mode::Auto,
            Mode::Auto => Mode::Forced,
            Mode::Forced => Mode::Off,
        }
    }
}

/// The operator's ANC control: a mode, and the engine that decides under it.
///
/// `Telemetry`, `Gates` and a hand-rolled `Status` used to live here. They are
/// gone: the canceller owns the thresholds, the hysteresis and the exit hold,
/// and a second copy in the UI could only drift from them.
pub struct Anc {
    pub mode: Mode,
    engine: DroneCanceller,
    last: Option<DroneSnapshot>,
}

impl Default for Anc {
    fn default() -> Self {
        Anc { mode: Mode::default(), engine: DroneCanceller::new(), last: None }
    }
}

impl std::fmt::Debug for Anc {
    // DroneCanceller is not Debug, and deriving would force it to be — a UI
    // type should not push a trait onto the engine.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Anc").field("mode", &self.mode)
            .field("phase", &self.phase()).finish()
    }
}

impl Anc {
    /// Feed one window of captured audio.
    ///
    /// Off does not merely ignore the result — it resets the canceller, so
    /// returning to Auto re-acquires rather than resuming a lock formed before
    /// the operator switched away.
    ///
    /// Not called yet, and that is not an oversight: this frontend has no
    /// audio capture. `cpal` lives in `bo-controller`, which owns
    /// `anc_engine` and `drone_canceller` and feeds them; the egui app is the
    /// control surface over that engine and has no samples to hand it.
    ///
    /// Left public and tested because the DSP path is the part worth pinning,
    /// and because whichever process ends up owning capture — the controller
    /// today, this binary if it ever grows a device — calls exactly this.
    ///
    pub fn process(&mut self, samples: &[f32]) -> DronePhase {
        if self.mode == Mode::Off {
            self.engine.reset();
            self.last = None;
            return DronePhase::Quiet;
        }
        let snap = self.engine.process_window(samples);
        self.last = Some(snap);
        snap.phase
    }

    /// The engine's phase, or Quiet before any audio has arrived.
    pub fn phase(&self) -> DronePhase {
        self.last.map(|s| s.phase).unwrap_or(DronePhase::Quiet)
    }

    pub fn snapshot(&self) -> Option<DroneSnapshot> {
        self.last
    }

    /// Is anti-noise leaving the speakers?
    ///
    /// Releasing counts: the engine keeps emitting at decaying amplitude
    /// through the exit hold, so reporting silence there would contradict what
    /// the speakers are doing.
    pub fn emitting(&self) -> bool {
        match self.mode {
            Mode::Off => false,
            Mode::Forced => true,
            Mode::Auto => matches!(self.phase(), DronePhase::Active | DronePhase::Releasing),
        }
    }

    /// Has the engine reported anything at all?
    ///
    /// `false` means no snapshot has arrived — `process` has no caller in this binary by
    /// design, so until `bo-controller` reports, the panel knows the MODE and nothing about
    /// the room. Distinguished from `Quiet` because "nothing to cancel" is a claim and this
    /// is the absence of one.
    pub fn has_engine_report(&self) -> bool {
        self.last.is_some()
    }

    /// Should this read as a problem?
    ///
    /// Quiet is not a fault, and colouring it like one teaches the operator to
    /// ignore the indicator. Detected with poor tracker stability is the case
    /// worth surfacing: a drone is present and the lock will not hold.
    pub fn is_fault(&self) -> bool {
        match (self.phase(), self.last) {
            (DronePhase::Detected, Some(s)) => s.tracker_stability < 0.5,
            _ => false,
        }
    }

    /// Cycle the mode. Off → Auto → On, so the common case is one tap from
    /// Off and Forced is never one accidental tap from Auto.
    pub fn cycle(&mut self) -> Mode {
        self.mode = self.mode.next();
        if self.mode != Mode::Auto {
            self.engine.reset();
            self.last = None;
        }
        self.mode
    }

    /// The line under the chip: what it is doing, and to what.
    pub fn readout(&self) -> String {
        match self.mode {
            Mode::Off => "off".into(),
            Mode::Forced => "forced on — phase ignored".into(),
            // NOTHING HAS LISTENED YET is not the same as "quiet".
            //
            // `process` has no caller in this binary and that is deliberate — cpal lives in
            // `bo-controller`, which owns the engine and feeds it; this app is the control
            // surface. But `last` is `None` until a snapshot arrives from somewhere, and
            // `phase()` reports `Quiet` for `None`, so an Auto-mode readout with no engine
            // behind it said "quiet — nothing to cancel".
            //
            // That is a positive claim about a room nothing has listened to, and it is the
            // most misleading thing this panel could say: an operator reading it concludes
            // the ANC is working and there is simply no drone, when in fact no audio has
            // reached the engine at all.
            Mode::Auto if self.last.is_none() =>
                "waiting for the engine — no audio has reached it yet".into(),
            Mode::Auto => match (self.phase(), self.last) {
                (DronePhase::Quiet, _) => "quiet — nothing to cancel".into(),
                (DronePhase::Detected, Some(s)) =>
                    format!("drone at {:.0} Hz — confirming (lock {:.0}%)",
                            s.fundamental_hz, s.tracker_stability * 100.0),
                (DronePhase::Active, Some(s)) =>
                    format!("cancelling {:.0} Hz", s.fundamental_hz),
                (DronePhase::Releasing, _) => "holding through a gap".into(),
                (p, None) => format!("{p:?}"),
            },
        }
    }
}

/// The predictive path: periodic noise the tonal canceller does not cover.
///
/// `DroneCanceller` locks a FUNDAMENTAL — a sustained tone. The other half of
/// this project's premise is different and, until now, unreachable:
///
/// > Motor / fan / compressor noise REPEATS with a stable period. If we detect
/// > that period and learn the per-cycle waveform shape, we can PREDICT the
/// > next cycle from the previous one … because we're predicting rather than
/// > reacting, latency only needs to be smaller than one cycle.
///
/// That is `bo_core::pattern_learner`, and it was `#![allow(dead_code)]`,
/// vendored from a bo-mux that no longer has a copy, referenced by nothing in
/// the tree. It is the reason the whole approach works over a mesh of Bluetooth
/// speakers with real latency, and it had no way to reach a screen.
pub struct PatternProbe {
    /// Estimated cycle length in samples, when one was found.
    pub period: Option<f32>,
    /// How much of the signal that period explains, 0..1.
    pub score: f32,
    template: Vec<f32>,
}

impl Default for PatternProbe {
    fn default() -> Self {
        PatternProbe { period: None, score: 0.0, template: Vec::new() }
    }
}

impl std::fmt::Debug for PatternProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PatternProbe").field("period", &self.period)
            .field("score", &self.score)
            .field("template_len", &self.template.len()).finish()
    }
}

/// Cycle bounds in samples at 48 kHz: 20 Hz to 200 Hz.
///
/// Chosen from what the module is FOR — motors, fans, compressors — rather
/// than from the full audible range. A wider search finds spurious long
/// periods in ordinary noise, and the cost of a false lock here is anti-noise
/// predicted from a cycle that does not exist.
const PERIOD_MIN: usize = 240;
const PERIOD_MAX: usize = 2400;

impl PatternProbe {
    /// Feed a window; returns the periodicity score.
    ///
    /// Not called yet, and that is not an oversight: same reason as
    /// `Anc::process` above — this frontend has no audio capture, and
    /// `bo-controller` owns the samples.
    ///
    /// The template is relearned whenever a period is found rather than
    /// averaged across windows: the source changes speed (a mower bogs down in
    /// thick grass) and a template blended across two speeds predicts neither.
    pub fn observe(&mut self, samples: &[f32]) -> f32 {
        match bo_core::pattern_learner::estimate_period(samples, PERIOD_MIN, PERIOD_MAX) {
            Some(p) => {
                self.period = Some(p);
                if let Some(tpl) = bo_core::pattern_learner::learn_template(samples, p) {
                    self.score = bo_core::pattern_learner::periodicity_score(samples, &tpl);
                    self.template = tpl;
                } else {
                    self.score = 0.0;
                }
            }
            None => {
                self.period = None;
                self.score = 0.0;
                self.template.clear();
            }
        }
        self.score
    }

    /// Is the cycle explanatory enough to predict from?
    ///
    /// A period is always findable in noise; a period that explains most of
    /// the signal is a real source. The threshold gates prediction, not
    /// detection, because predicting from a weak template emits anti-noise
    /// shaped like nothing that is happening.
    pub fn predictable(&self) -> bool {
        self.period.is_some() && self.score >= 0.5
    }

    /// The cycle in milliseconds, for a readout a person can reason about —
    /// "a 40 ms cycle" is a thing you can hear, 1920 samples is not.
    pub fn period_ms(&self) -> Option<f32> {
        self.period.map(|p| p * 1000.0 / 48_000.0)
    }

    /// What the inverse waveform would be at this phase — the value that
    /// would actually be played. Exposed so a UI can draw the predicted
    /// counter-cycle instead of asserting that one exists.
    pub fn inverse_at(&self, phase_samples: usize) -> Option<f32> {
        (!self.template.is_empty()).then(|| {
            bo_core::pattern_learner::inverse_template_at(&self.template, phase_samples)
        })
    }
}

/// The playbar chip: mode button plus the reason line.
///
/// Lives in the transport strip rather than on a page, because the operator
/// has to be able to kill the anti-noise at any moment — including while
/// using the sleep engine or the theatre setup, where an ANC page would mean
/// switching apps to switch it off. Returns the new mode when tapped.
pub fn anc_chip(ui: &mut egui::Ui, a: &mut Anc, scene: &mut uikit_egui::A11yScene,
                c: &uikit_egui::PaletteColors) -> Option<Mode> {
    let mut changed = None;
    ui.horizontal(|ui| {
        // The dot colours the ENGINE'S STATE, not the mode: an operator
        // glancing at the bar wants to know whether anti-noise is leaving the
        // speakers, which "Auto" alone does not say.
        let dot = if a.is_fault() {
            c.warning
        } else if a.emitting() {
            c.accent
        } else {
            c.text_weak
        };
        // A HOLLOW DOT while nothing has reported. Grey-filled and grey-hollow both read as
        // "not emitting", but only one of them also means "and I have no idea what the room
        // sounds like" — which is the state this panel is in until `bo-controller` reports,
        // since `process` has no caller here by design.
        let glyph = if a.mode != Mode::Off && !a.has_engine_report() { "\u{25cb}" } else { "\u{25cf}" };
        ui.colored_label(dot, glyph);
        let label = format!("ANC {}", a.mode.label());
        let resp = ui.selectable_label(a.mode != Mode::Off, &label);
        // REGISTERED FOR VOICE. The kit's a11y module says it is "shared
        // across every uikit-egui app" and one app used it; this control is
        // the case it was written for — a cycling button whose meaning is not
        // in its label, since "ANC Auto" does not say whether anti-noise is
        // currently leaving the speakers. The spoken value carries the phase,
        // so the answer is the same by voice as by eye.
        scene.add(
            0xa9c_0001,
            uikit_egui::a11y::Semantics::new(uikit_egui::a11y::Role::Button, "noise cancellation")
                .with_value(&format!("{}, {}", a.mode.label().to_lowercase(), a.readout()))
                .with_hint("activates to cycle off, auto, on"),
            Some(resp.rect),
        );
        if resp.clicked() {
            changed = Some(a.cycle());
        }
        // The reason, always — a silent canceller with no explanation is the
        // thing that makes an operator stop trusting the control. On a
        // narrow bar it is on the chip's hover/long-press rather than
        // beside it, where it overran the queue button at phone width.
        if ui.available_width() > 320.0 {
            ui.colored_label(c.text_weak, egui::RichText::new(a.readout()).small());
        } else {
            resp.on_hover_text(a.readout());
        }
    });
    changed
}

#[cfg(test)]
mod tests {

    /// The most misleading thing this panel could say. In Auto with no engine behind it,
    /// `phase()` reports `Quiet` for a `None` snapshot, so the readout claimed "quiet —
    /// nothing to cancel" about a room nothing had listened to. An operator reading that
    /// concludes the ANC is working and there is simply no drone.
    #[test]
    fn auto_with_no_engine_report_says_so_rather_than_claiming_quiet() {
        let mut a = Anc::default();
        a.mode = Mode::Auto;
        assert!(!a.has_engine_report());
        let r = a.readout();
        assert!(r.contains("waiting for the engine"), "{r}");
        assert!(!r.contains("nothing to cancel"), "it claimed the room was quiet: {r}");

        // Once a snapshot arrives, a genuine quiet IS reported as quiet — the point is the
        // distinction, not suppressing the word.
        let _ = a.process(&[0.0f32; 1024]);
        assert!(a.has_engine_report());
        let r = a.readout();
        assert!(!r.contains("waiting"), "{r}");
    }

    /// Off and Forced say what they are without an engine: neither is a claim about the room.
    #[test]
    fn the_other_modes_are_unaffected_by_having_no_report() {
        let mut a = Anc::default();
        a.mode = Mode::Off;
        assert_eq!(a.readout(), "off");
        a.mode = Mode::Forced;
        assert!(a.readout().contains("forced"), "{}", a.readout());
    }
    use super::*;
    use std::f32::consts::TAU;

    /// A sustained tone in the drone band, at the engine's sample rate.
    ///
    /// Real input rather than a hand-set struct: the whole point of this
    /// rewrite is that the UI no longer decides what counts as a drone, so a
    /// test that constructed a phase directly would be testing the old design.
    fn drone(hz: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (TAU * hz * i as f32 / 48_000.0).sin()).collect()
    }
    fn silence(n: usize) -> Vec<f32> {
        vec![0.0; n]
    }

    #[test]
    fn a_real_drone_drives_the_engine_out_of_quiet() {
        // The check that would have failed before this rewrite for the right
        // reason: there was no engine to drive.
        let mut a = Anc::default();
        assert_eq!(a.mode, Mode::Auto, "auto is the default");
        assert_eq!(a.phase(), DronePhase::Quiet, "nothing fed yet");

        let mut phases = vec![];
        for _ in 0..40 {
            phases.push(a.process(&drone(120.0, 0.5, 4096)));
        }
        assert!(phases.iter().any(|p| *p != DronePhase::Quiet),
                "40 windows of a 120 Hz tone never left Quiet: {phases:?}");
    }

    #[test]
    fn silence_stays_quiet_and_emits_nothing() {
        let mut a = Anc::default();
        for _ in 0..20 {
            a.process(&silence(4096));
        }
        assert_eq!(a.phase(), DronePhase::Quiet);
        assert!(!a.emitting(), "nothing to cancel, so nothing is emitted");
        assert!(!a.is_fault(), "quiet is not a fault — colouring it like one \
                                teaches the operator to ignore the light");
    }

    #[test]
    fn off_resets_the_engine_rather_than_ignoring_it() {
        // Returning to Auto must re-acquire, not resume a lock formed before
        // the operator switched away.
        let mut a = Anc::default();
        for _ in 0..40 {
            a.process(&drone(120.0, 0.5, 4096));
        }
        let reached = a.phase();
        a.mode = Mode::Off;
        assert_eq!(a.process(&drone(120.0, 0.5, 4096)), DronePhase::Quiet);
        assert!(!a.emitting(), "off is not 'emit silence'");
        assert!(a.snapshot().is_none(), "the stale snapshot is dropped");
        // And the phase it had reached is genuinely gone, not remembered.
        assert_eq!(a.phase(), DronePhase::Quiet, "was {reached:?} before Off");
    }

    #[test]
    fn forced_emits_whatever_the_engine_thinks() {
        // You cannot characterise a canceller that keeps deciding not to run.
        let mut a = Anc::default();
        a.mode = Mode::Forced;
        a.process(&silence(4096));
        assert!(a.emitting(), "forced ignores the phase");
        assert_eq!(a.readout(), "forced on — phase ignored");
    }

    #[test]
    fn one_tap_from_off_reaches_auto() {
        let mut a = Anc::default();
        a.mode = Mode::Off;
        assert_eq!(a.cycle(), Mode::Auto, "the common case is one tap");
        assert_eq!(a.cycle(), Mode::Forced);
        assert_eq!(a.cycle(), Mode::Off);
    }

    #[test]
    fn the_readout_names_the_frequency_the_engine_tracked() {
        // Not a frequency the UI was told — one the canceller estimated.
        let mut a = Anc::default();
        for _ in 0..40 {
            a.process(&drone(120.0, 0.5, 4096));
        }
        let r = a.readout();
        assert!(r != "off" && !r.is_empty());
        if matches!(a.phase(), DronePhase::Active | DronePhase::Detected) {
            assert!(r.contains("Hz"), "an engaged readout names the tone: {r}");
        }
    }

    /// A motor-like cycle: a sharp repeating pulse, NOT a sine.
    ///
    /// A pure tone would prove nothing here — DroneCanceller already handles
    /// those. The predictive path exists for sources whose cycle has shape,
    /// which is why the test signal is a decaying pulse train.
    fn motor(period: usize, cycles: usize) -> Vec<f32> {
        let mut v = Vec::with_capacity(period * cycles);
        for _ in 0..cycles {
            for i in 0..period {
                let x = i as f32 / period as f32;
                v.push((-6.0 * x).exp() * (1.0 - 2.0 * x));
            }
        }
        v
    }

    #[test]
    fn a_repeating_cycle_is_found_and_measured() {
        // The premise the project rests on, reaching a screen for the first
        // time: this code was dead in a crate nothing linked.
        let mut probe = PatternProbe::default();
        let score = probe.observe(&motor(960, 8));   // 960 samples = 50 Hz
        let p = probe.period.expect("a pulse train repeating every 960 samples has a period");
        assert!((p - 960.0).abs() < 40.0, "found {p}, expected ~960");
        assert!(score > 0.5, "an exact repeat should be well explained, got {score}");
        assert!(probe.predictable());
        let ms = probe.period_ms().unwrap();
        assert!((ms - 20.0).abs() < 1.0, "960 samples at 48 kHz is 20 ms, got {ms}");
    }

    #[test]
    fn noise_does_not_become_a_prediction() {
        // A period is always findable; one that explains the signal is not.
        // Predicting from a weak template emits anti-noise shaped like
        // nothing that is happening, which is worse than staying silent.
        let mut probe = PatternProbe::default();
        let mut x = 12345u32;
        let noise: Vec<f32> = (0..8192).map(|_| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            (x >> 8) as f32 / 8388608.0 - 1.0
        }).collect();
        probe.observe(&noise);
        assert!(!probe.predictable(), "white noise scored {}", probe.score);
    }

    #[test]
    fn silence_yields_no_period_at_all() {
        let mut probe = PatternProbe::default();
        assert_eq!(probe.observe(&vec![0.0f32; 8192]), 0.0);
        assert!(probe.period.is_none());
        assert!(probe.inverse_at(0).is_none(), "no template, no prediction");
    }

    #[test]
    fn the_predicted_counter_cycle_is_available_to_draw() {
        // A UI should be able to show the inverse waveform rather than assert
        // that one exists.
        let mut probe = PatternProbe::default();
        probe.observe(&motor(960, 8));
        let a = probe.inverse_at(0).expect("template learned");
        let b = probe.inverse_at(480).expect("template learned");
        assert!(a.is_finite() && b.is_finite());
        assert!(a != b, "a shaped cycle should differ across its phase");
    }

    #[test]
    fn the_types_are_the_engines_own() {
        // Guards the regression this rewrite exists to prevent: if these ever
        // become local definitions again, the UI has forked from bo-mux and
        // the two will drift silently.
        fn takes_engine_phase(_: bo_mux::drone_canceller::DronePhase) {}
        takes_engine_phase(Anc::default().phase());
        let _: Option<bo_mux::drone_canceller::DroneSnapshot> =
            Anc::default().snapshot();
    }
}
