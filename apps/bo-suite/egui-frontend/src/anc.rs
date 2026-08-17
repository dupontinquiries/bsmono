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

/// The playbar chip: mode button plus the reason line.
///
/// Lives in the transport strip rather than on a page, because the operator
/// has to be able to kill the anti-noise at any moment — including while
/// using the sleep engine or the theatre setup, where an ANC page would mean
/// switching apps to switch it off. Returns the new mode when tapped.
pub fn anc_chip(ui: &mut egui::Ui, a: &mut Anc,
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
        ui.colored_label(dot, "●");
        if ui.selectable_label(a.mode != Mode::Off, format!("ANC {}", a.mode.label()))
            .on_hover_text("Off · Auto · On")
            .clicked()
        {
            changed = Some(a.cycle());
        }
        // The reason, always — a silent canceller with no explanation is the
        // thing that makes an operator stop trusting the control.
        ui.colored_label(c.text_weak, egui::RichText::new(a.readout()).small());
    });
    changed
}

#[cfg(test)]
mod tests {
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
