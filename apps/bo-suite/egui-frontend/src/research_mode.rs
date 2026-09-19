//! research_mode — the gate over experimental noise cancellation.
//!
//! `SettingsPage.qml`'s EXPERIMENTAL FEATURES (RESEARCH MODE) card. One
//! switch, and behind it three named features that are real DSP in `bo-mux`,
//! not placeholders:
//!
//! * **Passby canceller (3-harmonic)** — `passby_canceller.rs`, predictive
//!   cancellation of vehicle engine harmonics.
//! * **Per-harmonic phase lock auto-mode** — the same module's phase tracking.
//! * **Jitter smoother (gated on observed latency)** — the soundscape path.
//!
//! ## Why a gate rather than just shipping them
//!
//! Because an anti-noise system that is wrong is not merely ineffective, it is
//! WORSE THAN OFF. `bo-core::music_masker` carries the arithmetic: summing a
//! tone with an inverted copy offset by φ leaves `|2·sin(φ/2)|` of the
//! original, so past 60° of phase error the canceller adds instead of
//! subtracting, and at 180° it doubles the noise. The expected residual under
//! uniformly random phase — which is what jitter produces — is 1.27.
//!
//! These three features are the ones most exposed to that, which is why they
//! sit behind a switch that says "research" rather than "advanced".
//!
//! ## The switch is not a preference
//!
//! Preferences persist quietly and are forgotten. This one should be visible
//! whenever it is on, and [`ResearchMode::banner`] exists so a UI can say so
//! somewhere other than the page where it was enabled — a person debugging odd
//! audio three weeks later needs to find this without remembering it.

/// One experimental feature behind the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Feature {
    PassbyCanceller,
    PhaseLockAuto,
    JitterSmoother,
}

impl Feature {
    pub const ALL: [Feature; 3] =
        [Feature::PassbyCanceller, Feature::PhaseLockAuto, Feature::JitterSmoother];

    /// Verbatim from the QML's list.
    pub fn label(self) -> &'static str {
        match self {
            Feature::PassbyCanceller => "Passby canceller (3-harmonic)",
            Feature::PhaseLockAuto => "Per-harmonic phase lock auto-mode",
            Feature::JitterSmoother => "Jitter smoother (gated on observed latency)",
        }
    }

    /// What this one can do when it is wrong, in the operator's terms.
    ///
    /// Not a disclaimer — a description. "May be unstable" tells nobody
    /// anything; "can make road noise louder rather than quieter" is a thing a
    /// person can recognise happening and switch off.
    pub fn risk(self) -> &'static str {
        match self {
            Feature::PassbyCanceller =>
                "Can make passing traffic louder rather than quieter if the \
                 phase estimate is off.",
            Feature::PhaseLockAuto =>
                "Tracks each harmonic independently, so one bad lock can add \
                 a tone that was not there.",
            Feature::JitterSmoother =>
                "Trades latency for stability. On a jittery link the \
                 correction can lag the noise it is correcting.",
        }
    }
}

/// The gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResearchMode {
    enabled: bool,
}

impl ResearchMode {
    /// OFF. The QML's default and the only defensible one: a system that can
    /// add noise must not arrive switched on.
    pub fn new() -> ResearchMode {
        ResearchMode::default()
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set(&mut self, on: bool) {
        self.enabled = on;
    }

    /// Features actually running. Empty while off — the list is not a menu of
    /// what could be on, it is what IS.
    pub fn active(&self) -> Vec<Feature> {
        if self.enabled {
            Feature::ALL.to_vec()
        } else {
            Vec::new()
        }
    }

    /// A line for somewhere other than this page, or `None` when off.
    ///
    /// The whole reason this is not an ordinary preference: someone debugging
    /// odd audio weeks later must be able to find this without remembering
    /// they enabled it.
    pub fn banner(&self) -> Option<&'static str> {
        self.enabled.then_some(
            "Research mode is on — experimental noise cancellation is active.",
        )
    }

    /// Whether a given feature may run. The single question the engine asks.
    ///
    /// A method rather than callers reading `is_enabled` and deciding, so
    /// "which features the gate covers" has one answer. Adding a fourth
    /// feature must not mean finding every `if enabled` in the tree.
    pub fn allows(&self, _f: Feature) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OFF BY DEFAULT. A system that can add noise must not arrive switched
    /// on, and `music_masker`'s arithmetic is why: past 60 degrees of phase
    /// error an anti-noise signal adds instead of subtracting.
    #[test]
    fn research_mode_starts_off_and_nothing_runs() {
        let r = ResearchMode::new();
        assert!(!r.is_enabled());
        assert!(r.active().is_empty());
        assert!(r.banner().is_none());
        for f in Feature::ALL {
            assert!(!r.allows(f), "{f:?} must not run by default");
        }
    }

    /// The list is what IS running, not a menu of what could be.
    #[test]
    fn enabling_the_gate_activates_every_feature_behind_it() {
        let mut r = ResearchMode::new();
        r.set(true);
        assert_eq!(r.active(), Feature::ALL.to_vec());
        for f in Feature::ALL {
            assert!(r.allows(f));
        }
    }

    /// THE BANNER IS THE POINT. A preference persists quietly and is
    /// forgotten; someone debugging odd audio weeks later must find this
    /// without remembering they turned it on.
    #[test]
    fn the_banner_appears_only_while_it_is_on() {
        let mut r = ResearchMode::new();
        assert!(r.banner().is_none());
        r.set(true);
        let b = r.banner().expect("a banner while on");
        assert!(b.contains("Research mode"), "{b}");
        assert!(b.contains("noise cancellation"), "it must name what is active: {b}");
        r.set(false);
        assert!(r.banner().is_none(), "and it must go away again");
    }

    /// Each risk DESCRIBES a symptom rather than disclaiming. "May be
    /// unstable" tells nobody anything; "can make road noise louder" is
    /// something a person can recognise and act on.
    #[test]
    fn every_feature_describes_what_going_wrong_looks_like() {
        for f in Feature::ALL {
            let r = f.risk();
            assert!(r.len() > 40, "{f:?} has no real description");
            assert!(!r.to_lowercase().contains("unstable"),
                    "{f:?} disclaims instead of describing: {r}");
        }
        assert!(Feature::PassbyCanceller.risk().contains("louder"),
                "the anti-noise failure mode is that it ADDS noise");
    }

    /// Labels are verbatim from the QML, so the two surfaces name the same
    /// features the same way.
    #[test]
    fn the_labels_match_the_qml() {
        assert_eq!(Feature::PassbyCanceller.label(), "Passby canceller (3-harmonic)");
        assert_eq!(Feature::PhaseLockAuto.label(), "Per-harmonic phase lock auto-mode");
        assert_eq!(
            Feature::JitterSmoother.label(),
            "Jitter smoother (gated on observed latency)"
        );
    }

    /// One question, one answer. Adding a fourth feature must not mean finding
    /// every `if enabled` in the tree.
    #[test]
    fn the_gate_is_asked_through_one_method() {
        let mut r = ResearchMode::new();
        r.set(true);
        assert_eq!(
            Feature::ALL.iter().filter(|f| r.allows(**f)).count(),
            r.active().len(),
            "allows() and active() must agree"
        );
    }
}
