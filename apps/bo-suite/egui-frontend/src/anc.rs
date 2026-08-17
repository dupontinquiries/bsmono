//! anc — the operator's control over active noise cancellation.
//!
//! bo-suite is an ANC research system with a music player, a home-theatre
//! setup and a sleep-sound engine built over it. The egui port had all three
//! of those and nothing at all for the ANC — while the Qt app split its
//! controls across one card in the middle of a long Settings page and another
//! near the end, which is how a system's main feature becomes the hardest
//! thing in it to find.
//!
//! ## Three modes, because On and Auto are different promises
//!
//! * **Off** — emit nothing. Not "emit silence": the canceller does not run,
//!   which is what you want when debugging something else or when the anti-
//!   noise is making things worse.
//! * **Auto** — the engine's gates decide. `drone_canceller` already has the
//!   whole policy: a presence gate (energy above the noise floor), a tonality
//!   gate with asymmetric hysteresis, a stability tolerance on the frequency
//!   estimate, and a three-second exit hold. Auto is not a new algorithm; it
//!   is exposing the one that was already there but invisible.
//! * **On** — emit regardless of the gates. Forced, for measurement: you
//!   cannot characterise a canceller that keeps deciding not to run.
//!
//! ## Why the gate's reason is carried, not just its answer
//!
//! An operator watching a quiet room needs to know the difference between
//! "no noise to cancel" and "noise present but the estimate is not stable
//! enough to lock onto". Both are silent. Only one is a fault. A boolean
//! `emitting` collapses them, and the operator learns to distrust the light.

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

/// What the engine reports about the noise it is hearing.
///
/// Mirrors the thresholds in `bo-mux`'s drone canceller rather than inventing
/// its own: `presence_ratio` against `PRESENCE_RATIO`, `tonality` against the
/// enter/exit pair, `freq_jitter_hz` against the stability tolerance. Keeping
/// the numbers here as inputs — not the decision — means this module cannot
/// drift away from the engine's actual behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Telemetry {
    /// Energy above the noise floor, as a ratio.
    pub presence_ratio: f32,
    /// How tonal the noise is, 0..1.
    pub tonality: f32,
    /// Wander in the running fundamental estimate.
    pub freq_jitter_hz: f32,
    /// The fundamental being tracked, for the readout.
    pub fundamental_hz: f32,
}

/// The engine's own thresholds. Defaults match `bo-mux/src/drone_canceller.rs`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gates {
    pub presence_ratio: f32,
    pub tonality_enter: f32,
    pub tonality_exit: f32,
    pub stable_freq_tolerance_hz: f32,
    /// How long the gate stays open after the noise goes, so a passing gap
    /// does not chop the anti-noise on and off.
    pub exit_hold_ms: u64,
}

impl Default for Gates {
    fn default() -> Self {
        Gates {
            presence_ratio: 4.0,
            tonality_enter: 0.30,
            tonality_exit: 0.15,
            stable_freq_tolerance_hz: 25.0,
            exit_hold_ms: 3_000,
        }
    }
}

/// Why the canceller is or is not emitting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Mode is Off.
    Disabled,
    /// Nothing above the noise floor. The room is quiet; this is success.
    Quiet,
    /// Noise present, but too broadband to cancel — a fan, not a drone.
    NotTonal,
    /// Tonal noise present, but the frequency estimate is wandering too much
    /// to lock onto. Distinct from Quiet because this one may be a fault.
    Unstable,
    /// Emitting anti-noise, gates open.
    Emitting,
    /// Emitting because the noise just stopped and the hold has not expired.
    /// Named separately so an operator does not read a stale emission as a
    /// live lock.
    Holding,
    /// Emitting because the operator forced it, gates ignored.
    Forced,
}

impl Status {
    /// Is anti-noise leaving the speakers?
    pub fn emitting(self) -> bool {
        matches!(self, Status::Emitting | Status::Holding | Status::Forced)
    }

    /// One line for the operator.
    pub fn line(self) -> &'static str {
        match self {
            Status::Disabled => "off",
            Status::Quiet => "quiet — nothing to cancel",
            Status::NotTonal => "noise is broadband — not cancellable",
            Status::Unstable => "tonal, but the estimate is wandering",
            Status::Emitting => "cancelling",
            Status::Holding => "holding through a gap",
            Status::Forced => "forced on — gates ignored",
        }
    }

    /// Should this read as a problem? Quiet is not a problem, and colouring it
    /// like one teaches the operator to ignore the indicator.
    pub fn is_fault(self) -> bool {
        self == Status::Unstable
    }
}

/// The operator's ANC control: mode, gate state, and the hold clock.
#[derive(Debug, Clone)]
pub struct Anc {
    pub mode: Mode,
    pub gates: Gates,
    /// True while the tonality gate is open — hysteresis state, which is why
    /// it is remembered rather than recomputed from the current reading.
    gate_open: bool,
    /// Milliseconds since the gate would have closed, or None if it is open.
    closing_for_ms: Option<u64>,
    last: Telemetry,
    status: Status,
}

impl Default for Anc {
    fn default() -> Self {
        Anc {
            mode: Mode::default(),
            gates: Gates::default(),
            gate_open: false,
            closing_for_ms: None,
            last: Telemetry::default(),
            status: Status::Quiet,
        }
    }
}

impl Anc {
    /// Feed a telemetry frame and advance the hold clock.
    ///
    /// `dt_ms` is how long since the last frame. Returns the resulting status,
    /// which is also stored for the readout.
    pub fn update(&mut self, t: Telemetry, dt_ms: u64) -> Status {
        self.last = t;
        self.status = match self.mode {
            Mode::Off => {
                // Off resets the hysteresis. Coming back to Auto after a
                // debugging detour should re-acquire from scratch rather than
                // resume a lock formed several minutes ago.
                self.gate_open = false;
                self.closing_for_ms = None;
                Status::Disabled
            }
            Mode::Forced => Status::Forced,
            Mode::Auto => self.auto_status(t, dt_ms),
        };
        self.status
    }

    fn auto_status(&mut self, t: Telemetry, dt_ms: u64) -> Status {
        let present = t.presence_ratio >= self.gates.presence_ratio;
        // Asymmetric hysteresis: it takes more tonality to open the gate than
        // to keep it open. Symmetric thresholds chatter on noise sitting right
        // at the boundary, and chattering anti-noise is worse than none.
        let tonal = if self.gate_open {
            t.tonality >= self.gates.tonality_exit
        } else {
            t.tonality >= self.gates.tonality_enter
        };
        let stable = t.freq_jitter_hz <= self.gates.stable_freq_tolerance_hz;

        if present && tonal && stable {
            self.gate_open = true;
            self.closing_for_ms = None;
            return Status::Emitting;
        }

        if self.gate_open {
            // The gate was open and the conditions just lapsed. Hold, so a
            // passing gap — the operator walks behind a wall, the compressor
            // cycles — does not chop the anti-noise on and off.
            let held = self.closing_for_ms.unwrap_or(0) + dt_ms;
            if held < self.gates.exit_hold_ms {
                self.closing_for_ms = Some(held);
                return Status::Holding;
            }
            self.gate_open = false;
            self.closing_for_ms = None;
        }

        // Why we are not emitting, in the order that makes the reason true:
        // no noise at all outranks "not tonal", which outranks "unstable".
        if !present {
            Status::Quiet
        } else if !tonal {
            Status::NotTonal
        } else {
            Status::Unstable
        }
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn telemetry(&self) -> Telemetry {
        self.last
    }

    /// Cycle the mode, for a single always-visible control.
    pub fn cycle(&mut self) -> Mode {
        self.mode = self.mode.next();
        // Leaving Off or Forced must not carry stale gate state into Auto.
        if self.mode != Mode::Auto {
            self.gate_open = false;
            self.closing_for_ms = None;
        }
        self.mode
    }

    /// The readout under the control: what it is doing and to what.
    pub fn readout(&self) -> String {
        match self.status {
            Status::Emitting | Status::Holding => {
                format!("{} @ {:.0} Hz", self.status.line(), self.last.fundamental_hz)
            }
            _ => self.status.line().to_string(),
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
        let s = a.status();
        // The dot colours the STATE, not the mode: an operator glancing at
        // the bar wants to know whether anti-noise is leaving the speakers,
        // which "Auto" alone does not say.
        let dot = if s.is_fault() {
            c.warning
        } else if s.emitting() {
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

    fn drone() -> Telemetry {
        Telemetry { presence_ratio: 8.0, tonality: 0.5, freq_jitter_hz: 5.0,
                    fundamental_hz: 120.0 }
    }
    fn quiet() -> Telemetry {
        Telemetry { presence_ratio: 1.0, tonality: 0.0, freq_jitter_hz: 0.0,
                    fundamental_hz: 0.0 }
    }

    #[test]
    fn auto_does_not_emit_into_a_quiet_room() {
        let mut a = Anc::default();
        assert_eq!(a.mode, Mode::Auto, "auto is the default — the safe one");
        assert_eq!(a.update(quiet(), 50), Status::Quiet);
        assert!(!a.status().emitting(), "nothing to cancel, so nothing is emitted");
        // And quiet is NOT a fault: colouring it like one teaches the operator
        // to ignore the indicator.
        assert!(!Status::Quiet.is_fault());
    }

    #[test]
    fn a_drone_opens_the_gate_and_a_gap_is_held_through() {
        let mut a = Anc::default();
        assert_eq!(a.update(drone(), 50), Status::Emitting);
        // The noise stops. The gate holds rather than chopping off — a passing
        // gap must not switch the anti-noise on and off.
        assert_eq!(a.update(quiet(), 1_000), Status::Holding);
        assert!(a.status().emitting());
        assert_eq!(a.update(quiet(), 1_000), Status::Holding);
        // Past the three-second hold it finally closes.
        assert_eq!(a.update(quiet(), 1_500), Status::Quiet);
        assert!(!a.status().emitting());
    }

    #[test]
    fn the_tonality_gate_is_harder_to_open_than_to_keep_open() {
        let mut a = Anc::default();
        // 0.20 is above the exit threshold but below the enter threshold: not
        // enough to start.
        let marginal = Telemetry { tonality: 0.20, ..drone() };
        assert_eq!(a.update(marginal, 50), Status::NotTonal);
        // Once a real drone opens the gate, the same marginal reading keeps it
        // open. Symmetric thresholds would chatter here, and chattering
        // anti-noise is worse than none.
        assert_eq!(a.update(drone(), 50), Status::Emitting);
        assert_eq!(a.update(marginal, 50), Status::Emitting);
    }

    #[test]
    fn silence_has_three_different_reasons_and_says_which() {
        let mut a = Anc::default();
        assert_eq!(a.update(quiet(), 50), Status::Quiet);
        // Loud but broadband — a fan, not a drone. Nothing to lock onto.
        let fan = Telemetry { tonality: 0.05, ..drone() };
        assert_eq!(a.update(fan, 50), Status::NotTonal);
        // Tonal but wandering: this one may be a fault, and it is the only
        // status that says so.
        let wobbly = Telemetry { freq_jitter_hz: 60.0, ..drone() };
        assert_eq!(a.update(wobbly, 50), Status::Unstable);
        assert!(Status::Unstable.is_fault());
        assert!(!Status::NotTonal.is_fault(), "a fan is not a malfunction");
    }

    #[test]
    fn forced_emits_regardless_and_off_emits_nothing() {
        let mut a = Anc::default();
        a.mode = Mode::Forced;
        // Forced ignores every gate — you cannot characterise a canceller that
        // keeps deciding not to run.
        assert_eq!(a.update(quiet(), 50), Status::Forced);
        assert!(a.status().emitting());
        a.mode = Mode::Off;
        assert_eq!(a.update(drone(), 50), Status::Disabled);
        assert!(!a.status().emitting(), "off is not 'emit silence'");
    }

    #[test]
    fn one_tap_from_off_reaches_auto_and_a_detour_does_not_resume_a_stale_lock() {
        let mut a = Anc::default();
        a.mode = Mode::Off;
        assert_eq!(a.cycle(), Mode::Auto, "the common case is one tap");
        assert_eq!(a.cycle(), Mode::Forced);
        assert_eq!(a.cycle(), Mode::Off);

        // Lock onto a drone, switch away, come back: the gate re-acquires
        // rather than resuming a lock formed before the detour.
        a.mode = Mode::Auto;
        assert_eq!(a.update(drone(), 50), Status::Emitting);
        a.mode = Mode::Off;
        a.update(drone(), 50);
        a.mode = Mode::Auto;
        let marginal = Telemetry { tonality: 0.20, ..drone() };
        assert_eq!(a.update(marginal, 50), Status::NotTonal,
                   "a stale open gate would have accepted this");
    }

    #[test]
    fn the_readout_names_the_frequency_only_when_there_is_one() {
        let mut a = Anc::default();
        a.update(drone(), 50);
        assert!(a.readout().contains("120 Hz"));
        a.update(quiet(), 5_000);
        assert_eq!(a.readout(), "quiet — nothing to cancel");
    }
}
