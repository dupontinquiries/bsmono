// src/v1.rs
#![allow(dead_code)]
//
// V1 stability flag system.
//
// ── What this is ──
//
// The research arc in docs/v1-plan.md established that the
// codebase has two classes of noise-reduction features:
//
//   - Production-ready features that have been validated against
//     real recordings (the music masker, the backup-beeper masker,
//     the drone canceller in its current form, the audio capture
//     dump, the FSM with geometry-aware Doppler gate).
//
//   - Research-mode features that work on synthetic test signals
//     but have NOT been validated on real recordings — including
//     the passby canceller's extended 8-harmonic mode, the
//     JitterSmoother, octave-shifting, sliding-shelf saturation,
//     and the per-harmonic phase lock auto-mode.
//
// This module is the single source of truth for which class a
// feature falls in, and whether the runtime is permitted to
// enable research-mode features.
//
// ── Why a single source of truth ──
//
// Without this, "is feature X experimental" gets duplicated in:
//   - QML UI (does the toggle show? does it have an "experimental"
//     badge? what tooltip?)
//   - Standby loop (does the canceller actually start?)
//   - Persistence (do we save the experimental setting?)
//   - Logging (do we route diagnostic prints differently?)
//
// All those paths now read from this module's `Stability` enum
// instead of duplicating ad-hoc boolean tests. If a feature moves
// from research to production after validation, the change is one
// line here and the rest of the codebase follows automatically.
//
// ── How the flag is read ──
//
// The runtime has a single `experimental_features_enabled` setting
// (persisted to disk via the settings store). When OFF (the
// default), any feature with stability != Stable is a no-op:
//   - Standby loop skips canceller activation
//   - QML hides the experimental tab
//   - Settings persistence still saves the experimental values
//     (so they survive a re-enable) but they have no runtime effect
//
// When ON, experimental features are active. The QML shows them
// in a clearly-marked "Experimental — research mode" section with
// an orange badge. Users opt in deliberately; nothing surprises
// them.

use std::sync::atomic::{AtomicBool, Ordering};

/// Validation status for a feature.
///
/// `Stable` means: cross-referenced against real recordings,
/// behaves the way the documentation claims, won't regress on
/// inputs we haven't seen. Safe to enable by default.
///
/// `Experimental` means: works on synthetic signals or in
/// narrow conditions, hasn't been validated on real recordings,
/// MAY make things worse in some conditions. User-opt-in only.
///
/// `Broken` means: known not to work, kept around as a research
/// artifact. Code path exists but should never be invoked even
/// with experimental flag on; only invoked from tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stability {
    Stable,
    Experimental,
    Broken,
}

/// All the features the V1 system knows about. Centralized so
/// that "what's the stability of X" has exactly one source of
/// truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    /// 4-band peakingEq music masker. Production.
    MusicMasker,
    /// Voss-McCartney pink-noise backup-beeper masker. Production.
    BackupBeeperMasker,
    /// Drone canceller for stable HVAC-like tonal sources. Works
    /// in narrow conditions but those conditions match the use
    /// case. Production.
    DroneCanceller,
    /// 30-second ring + triggered WAV captures for ongoing
    /// research data collection. Production.
    AudioCaptureDump,
    /// FSM with geometry-aware Doppler gate (accepts perpendicular
    /// passbys). Production.
    PassbyFsm,

    /// 3-harmonic passby canceller — the original design. Works
    /// on textbook synth but delivers ~0 dB on real engines.
    /// Experimental until the cross-reference gap is closed.
    PassbyCanceller3H,
    /// 8-harmonic extended passby canceller — architecturally the
    /// target but has an integration bug preventing it from
    /// approaching the 30 dB headroom. Experimental.
    PassbyCancellerExt,
    /// Per-harmonic phase lock auto-mode. Wins under reverb,
    /// loses on anechoic textbook. Experimental until validated
    /// on real recordings.
    PerHarmonicLock,
    /// JitterSmoother: latency-for-jitter trade buffer. Helps at
    /// high baseline latency, hurts at low. Experimental until
    /// the gating heuristic is reliable.
    JitterSmoother,
    /// Sliding-shelf saturation: per-harmonic drive increasing
    /// with frequency. Null result — doesn't help in any tested
    /// condition. Broken (kept as research artifact).
    SlidingShelfSaturation,
    /// Octave-shifting noise transformer. Required gain ≥ 2.0 to
    /// shift perceived pitch, which doubles acoustic power.
    /// Broken (kept as research artifact).
    OctaveShifter,
}

impl Feature {
    /// The classification for this feature. Single source of truth.
    /// If a feature graduates from Experimental to Stable, change
    /// it here and the rest of the codebase respects the change.
    pub fn stability(self) -> Stability {
        match self {
            Feature::MusicMasker
            | Feature::BackupBeeperMasker
            | Feature::DroneCanceller
            | Feature::AudioCaptureDump
            | Feature::PassbyFsm => Stability::Stable,

            Feature::PassbyCanceller3H
            | Feature::PassbyCancellerExt
            | Feature::PerHarmonicLock
            | Feature::JitterSmoother => Stability::Experimental,

            Feature::SlidingShelfSaturation
            | Feature::OctaveShifter => Stability::Broken,
        }
    }

    /// Short human-readable name for UI display.
    pub fn display_name(self) -> &'static str {
        match self {
            Feature::MusicMasker => "Music masker",
            Feature::BackupBeeperMasker => "Backup-beeper masker",
            Feature::DroneCanceller => "Drone canceller",
            Feature::AudioCaptureDump => "Audio capture dump",
            Feature::PassbyFsm => "Passby state machine",
            Feature::PassbyCanceller3H => "Passby canceller (3-harmonic)",
            Feature::PassbyCancellerExt => "Passby canceller (8-harmonic extended)",
            Feature::PerHarmonicLock => "Per-harmonic phase lock",
            Feature::JitterSmoother => "Jitter smoother",
            Feature::SlidingShelfSaturation => "Sliding-shelf saturation",
            Feature::OctaveShifter => "Octave shifter",
        }
    }

    /// One-sentence summary for tooltips.
    pub fn description(self) -> &'static str {
        match self {
            Feature::MusicMasker =>
                "Boosts music at engine harmonics to mask passby noise. \
                 Works regardless of vehicle type or phase.",
            Feature::BackupBeeperMasker =>
                "Plays pink noise to mask delivery-truck beeping.",
            Feature::DroneCanceller =>
                "Cancels stable tonal sources like HVAC compressors.",
            Feature::AudioCaptureDump =>
                "Records 30s rolling + 40s triggered WAVs for research.",
            Feature::PassbyFsm =>
                "Tracks Quiet/Approach/Peak/Recede states during passbys.",
            Feature::PassbyCanceller3H =>
                "Cancels passby fundamental, h2, h3. Works on textbook \
                 synth but delivers ~0 dB on real engines.",
            Feature::PassbyCancellerExt =>
                "Extended 8-harmonic mode. Architecturally targets +30 dB \
                 but currently has an integration bug.",
            Feature::PerHarmonicLock =>
                "Independently phase-locks each harmonic. Wins under \
                 reverb, loses on clean anechoic.",
            Feature::JitterSmoother =>
                "Adds latency to absorb jitter. Helps at high baseline \
                 latency (>150 ms), hurts at low.",
            Feature::SlidingShelfSaturation =>
                "Ramps saturation drive with frequency. Null result.",
            Feature::OctaveShifter =>
                "Shifts perceived fundamental up an octave. Requires \
                 doubling acoustic power.",
        }
    }
}

/// Global flag for whether experimental features are runtime-enabled.
/// Persisted to settings; default false (production-only behavior).
static EXPERIMENTAL_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn experimental_enabled() -> bool {
    EXPERIMENTAL_ENABLED.load(Ordering::Acquire)
}

pub fn set_experimental_enabled(on: bool) {
    EXPERIMENTAL_ENABLED.store(on, Ordering::Release);
}

/// Should this feature actually run at runtime?
///
/// - Stable features always run.
/// - Experimental features run iff `experimental_enabled()`.
/// - Broken features never run.
///
/// Call sites: `if v1::is_active(Feature::PassbyCanceller3H) { ... }`
pub fn is_active(feature: Feature) -> bool {
    match feature.stability() {
        Stability::Stable => true,
        Stability::Experimental => experimental_enabled(),
        Stability::Broken => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lock down the classifications so they can't drift silently.
    /// If you legitimately graduate a feature, update this test.
    #[test]
    fn stability_classifications_are_explicit() {
        assert_eq!(Feature::MusicMasker.stability(), Stability::Stable);
        assert_eq!(Feature::BackupBeeperMasker.stability(), Stability::Stable);
        assert_eq!(Feature::DroneCanceller.stability(), Stability::Stable);
        assert_eq!(Feature::AudioCaptureDump.stability(), Stability::Stable);
        assert_eq!(Feature::PassbyFsm.stability(), Stability::Stable);

        assert_eq!(Feature::PassbyCanceller3H.stability(), Stability::Experimental);
        assert_eq!(Feature::PassbyCancellerExt.stability(), Stability::Experimental);
        assert_eq!(Feature::PerHarmonicLock.stability(), Stability::Experimental);
        assert_eq!(Feature::JitterSmoother.stability(), Stability::Experimental);

        assert_eq!(Feature::SlidingShelfSaturation.stability(), Stability::Broken);
        assert_eq!(Feature::OctaveShifter.stability(), Stability::Broken);
    }

    #[test]
    fn stable_features_always_active() {
        // Even with experimental DISABLED, stable features run.
        set_experimental_enabled(false);
        assert!(is_active(Feature::MusicMasker));
        assert!(is_active(Feature::BackupBeeperMasker));
        assert!(is_active(Feature::DroneCanceller));
        assert!(is_active(Feature::AudioCaptureDump));
        assert!(is_active(Feature::PassbyFsm));
    }

    #[test]
    fn experimental_features_gated_by_flag() {
        set_experimental_enabled(false);
        assert!(!is_active(Feature::PassbyCanceller3H));
        assert!(!is_active(Feature::PassbyCancellerExt));
        assert!(!is_active(Feature::PerHarmonicLock));
        assert!(!is_active(Feature::JitterSmoother));

        set_experimental_enabled(true);
        assert!(is_active(Feature::PassbyCanceller3H));
        assert!(is_active(Feature::PassbyCancellerExt));
        assert!(is_active(Feature::PerHarmonicLock));
        assert!(is_active(Feature::JitterSmoother));

        // Reset to default for other tests.
        set_experimental_enabled(false);
    }

    #[test]
    fn broken_features_never_active() {
        // Even with experimental ON, broken features stay off.
        set_experimental_enabled(true);
        assert!(!is_active(Feature::SlidingShelfSaturation));
        assert!(!is_active(Feature::OctaveShifter));
        set_experimental_enabled(false);
    }

    #[test]
    fn all_features_have_distinct_display_names() {
        let names = [
            Feature::MusicMasker.display_name(),
            Feature::BackupBeeperMasker.display_name(),
            Feature::DroneCanceller.display_name(),
            Feature::AudioCaptureDump.display_name(),
            Feature::PassbyFsm.display_name(),
            Feature::PassbyCanceller3H.display_name(),
            Feature::PassbyCancellerExt.display_name(),
            Feature::PerHarmonicLock.display_name(),
            Feature::JitterSmoother.display_name(),
            Feature::SlidingShelfSaturation.display_name(),
            Feature::OctaveShifter.display_name(),
        ];
        for i in 0..names.len() {
            for j in (i + 1)..names.len() {
                assert_ne!(names[i], names[j],
                    "display names should be unique");
            }
        }
    }

    #[test]
    fn all_features_have_non_empty_descriptions() {
        for feature in [
            Feature::MusicMasker,
            Feature::BackupBeeperMasker,
            Feature::DroneCanceller,
            Feature::AudioCaptureDump,
            Feature::PassbyFsm,
            Feature::PassbyCanceller3H,
            Feature::PassbyCancellerExt,
            Feature::PerHarmonicLock,
            Feature::JitterSmoother,
            Feature::SlidingShelfSaturation,
            Feature::OctaveShifter,
        ] {
            assert!(!feature.description().is_empty(),
                "{:?} should have a non-empty description", feature);
            assert!(feature.description().len() < 200,
                "{:?}'s description is too long for a tooltip", feature);
        }
    }

    #[test]
    fn experimental_flag_is_thread_safe() {
        // Just verify atomic operations work without panic.
        // The atomic guarantees correctness; we're checking we
        // didn't accidentally use a non-atomic type.
        set_experimental_enabled(true);
        let _ = experimental_enabled();
        set_experimental_enabled(false);
        let _ = experimental_enabled();
    }
}
