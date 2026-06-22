// src/aggression.rs
#![allow(dead_code)]
//
// AggressionPolicy: the single dial that decides how forward-leaning the
// controller's pre-staging is. Lives separately from the various
// pre-staging subsystems (anticipator, shadow_stream, pre_buffer) so that
// adjusting one threshold ripples consistently to everything.
//
// LEVELS, with the user-facing connotation:
//
//   0 = OFF — no pre-staging. Tap → resolve → POST → wait. Useful for
//             debugging or when the network is degraded enough that
//             extra pre-stages cause more harm than good.
//
//   1 = LOW (default) — modest pre-stage of likely-tapped tracks when
//             browsing, simple low-frequency cue on tap. No shadow
//             stream (no silent pre-play). This is the "safe default":
//             you get tap-feedback latency relief without burning
//             bandwidth on guesses.
//
//   2 = MEDIUM — pre-stage more candidates, richer multi-harmonic cue
//             matched to the predicted track's tempo/key, shadow stream
//             enabled at high confidence (≥0.85). Noticeable latency
//             drop on common tap patterns.
//
//   3 = HIGH (experimental) — eager multi-candidate pre-stage, longest
//             tempo-aligned cue, shadow stream at moderate confidence
//             (≥0.70). Best perceived latency, most bandwidth burned on
//             wrong guesses.
//
// The user can flip between modes at any time via a QML slider. Higher
// levels can interact with their network conditions in unpredictable
// ways; LOW is the recommended sustained mode, HIGH is for testing.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AggressionPolicy {
    level: u8,
}

impl AggressionPolicy {
    pub const OFF:    Self = Self { level: 0 };
    pub const LOW:    Self = Self { level: 1 };
    pub const MEDIUM: Self = Self { level: 2 };
    pub const HIGH:   Self = Self { level: 3 };

    pub fn from_level(level: i32) -> Self {
        Self { level: level.clamp(0, 3) as u8 }
    }

    pub fn level(&self) -> u8 { self.level }

    pub fn label(&self) -> &'static str {
        match self.level {
            0 => "OFF",
            1 => "LOW",
            2 => "MEDIUM",
            _ => "HIGH",
        }
    }

    /// Is pre-staging enabled at all? Drives the anticipator's Moderate-
    /// tier pre-stage path.
    pub fn pre_stage_enabled(&self) -> bool { self.level >= 1 }

    /// Confidence threshold below which the anticipator declines to
    /// fire Moderate-cost pre-stages. Lower → more pre-staging.
    pub fn moderate_floor(&self) -> f32 {
        match self.level {
            0 => 999.0,    // off
            1 => 0.55,
            2 => 0.45,
            _ => 0.35,
        }
    }

    /// Adaptive variant of `moderate_floor` that scales with observed
    /// network latency. On slow networks, the controller has more to
    /// gain from pre-staging earlier (because catch-up takes longer);
    /// on fast networks, an over-eager pre-stage wastes resources.
    ///
    /// `rtt_p95_ms` is the observed 95th-percentile RTT in milliseconds.
    /// We treat 50 ms as "neutral" (use the base floor), reduce the
    /// floor by up to 0.15 as RTT climbs to 500 ms, and clamp from
    /// there. The reduction is linear in log(rtt) so the curve is
    /// gentle in the typical 20-100 ms range and steeper above.
    pub fn moderate_floor_adaptive(&self, rtt_p95_ms: u32) -> f32 {
        let base = self.moderate_floor();
        if base >= 1.0 { return base; } // OFF stays OFF
        if rtt_p95_ms == 0 { return base; }
        // Use a log curve so doubling RTT moves the threshold a
        // constant amount. log10(50)=1.7, log10(500)=2.7. We map
        // RTT 50→0.0 reduction, 500→0.15 reduction.
        let log_rtt = (rtt_p95_ms as f32).log10();
        let reduction = ((log_rtt - 1.7) * 0.15).clamp(0.0, 0.15);
        (base - reduction).max(0.20)
    }

    /// Confidence floor for actually starting a shadow stream (silent
    /// pre-play with vol=0). Returns 999.0 when shadow is disabled at
    /// this level, so the standard `should_shadow` short-circuits.
    pub fn shadow_floor(&self) -> f32 {
        match self.level {
            0 | 1 => 999.0,   // shadow disabled at OFF and LOW
            2 => 0.85,
            _ => 0.70,
        }
    }

    /// Whether the shadow stream subsystem fires at all.
    pub fn shadow_enabled(&self) -> bool { self.level >= 2 }

    /// How long the on-tap audible cue should be, in ms. 0 = silent,
    /// meaning no cue fires at all.
    pub fn cue_duration_ms(&self) -> u32 {
        match self.level {
            0 => 0,
            1 => 250,
            2 => 400,
            _ => 600,
        }
    }

    /// How many harmonics the cue stacks. Higher = richer timbre that
    /// better masks the moment the real audio arrives. 0 = no cue.
    pub fn cue_harmonics(&self) -> u32 {
        match self.level {
            0 => 0,
            1 => 2,
            2 => 4,
            _ => 6,
        }
    }

    /// How many candidate tracks to pre-resolve in parallel when the
    /// anticipator predicts a PlayTrack. Newer platforms with parallel
    /// decode can benefit from larger fanout.
    pub fn multi_prestage_count(&self) -> u32 {
        match self.level {
            0 => 0,
            1 => 3,
            2 => 5,
            _ => 8,
        }
    }

    /// Whether to surface diagnostic indicators in the UI (the breathing
    /// dot, status text). HIGH and above always; otherwise only when
    /// confidence is high enough to be meaningful.
    pub fn show_diagnostic_indicators(&self) -> bool { self.level >= 2 }
}

impl Default for AggressionPolicy {
    fn default() -> Self { Self::LOW }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_clamp_to_valid_range() {
        assert_eq!(AggressionPolicy::from_level(-5).level(), 0);
        assert_eq!(AggressionPolicy::from_level(100).level(), 3);
        assert_eq!(AggressionPolicy::from_level(2).level(), 2);
    }

    #[test]
    fn off_disables_everything() {
        let p = AggressionPolicy::OFF;
        assert!(!p.pre_stage_enabled());
        assert!(!p.shadow_enabled());
        assert_eq!(p.cue_duration_ms(), 0);
        assert_eq!(p.cue_harmonics(), 0);
        assert_eq!(p.multi_prestage_count(), 0);
    }

    #[test]
    fn higher_levels_lower_thresholds() {
        // Moderate floor should decrease (more pre-staging) as aggression rises.
        let low = AggressionPolicy::LOW.moderate_floor();
        let med = AggressionPolicy::MEDIUM.moderate_floor();
        let high = AggressionPolicy::HIGH.moderate_floor();
        assert!(low > med);
        assert!(med > high);
    }

    #[test]
    fn shadow_floor_decreases_with_aggression() {
        // OFF and LOW both disable shadow (floor=999); MEDIUM and HIGH
        // enable it with progressively lower floors.
        assert_eq!(AggressionPolicy::OFF.shadow_floor(), 999.0);
        assert_eq!(AggressionPolicy::LOW.shadow_floor(), 999.0);
        assert!(AggressionPolicy::MEDIUM.shadow_floor() < 999.0);
        assert!(AggressionPolicy::HIGH.shadow_floor() < AggressionPolicy::MEDIUM.shadow_floor());
    }

    #[test]
    fn cue_duration_and_harmonics_grow() {
        let levels = [AggressionPolicy::OFF, AggressionPolicy::LOW,
                      AggressionPolicy::MEDIUM, AggressionPolicy::HIGH];
        let durations: Vec<u32> = levels.iter().map(|p| p.cue_duration_ms()).collect();
        let harmonics: Vec<u32> = levels.iter().map(|p| p.cue_harmonics()).collect();
        // Monotonic non-decreasing
        for w in durations.windows(2) { assert!(w[1] >= w[0]); }
        for w in harmonics.windows(2) { assert!(w[1] >= w[0]); }
    }

    #[test]
    fn multi_prestage_count_scales() {
        let counts: Vec<u32> = (0..=3)
            .map(|l| AggressionPolicy::from_level(l).multi_prestage_count())
            .collect();
        for w in counts.windows(2) { assert!(w[1] >= w[0]); }
    }

    #[test]
    fn default_is_low() {
        let p = AggressionPolicy::default();
        assert_eq!(p, AggressionPolicy::LOW);
    }

    #[test]
    fn labels_are_distinct() {
        let labels: Vec<&str> = (0..=3)
            .map(|l| AggressionPolicy::from_level(l).label())
            .collect();
        let mut sorted = labels.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), labels.len(), "labels not distinct: {:?}", labels);
    }

    #[test]
    fn adaptive_floor_matches_base_for_neutral_rtt() {
        let p = AggressionPolicy::LOW;
        // 50ms RTT is the neutral inflection — no reduction expected.
        let base = p.moderate_floor();
        let adapt = p.moderate_floor_adaptive(50);
        assert!((adapt - base).abs() < 0.01,
                "expected ~{}, got {}", base, adapt);
    }

    #[test]
    fn adaptive_floor_drops_on_slow_network() {
        let p = AggressionPolicy::MEDIUM;
        let base = p.moderate_floor();              // 0.45
        let slow = p.moderate_floor_adaptive(500);  // RTT 500ms
        // Adaptive should be at least 0.05 lower (more aggressive)
        assert!(slow < base - 0.05,
                "expected slow < {} - 0.05 = {}, got {}",
                base, base - 0.05, slow);
    }

    #[test]
    fn adaptive_floor_clamps_to_minimum() {
        let p = AggressionPolicy::HIGH;
        let very_slow = p.moderate_floor_adaptive(10_000);
        // Should not go below 0.20 no matter how slow.
        assert!(very_slow >= 0.20);
    }

    #[test]
    fn adaptive_floor_off_stays_off() {
        let p = AggressionPolicy::OFF;
        // OFF is 999.0 (never fires). Adaptive must not enable it.
        assert!(p.moderate_floor_adaptive(50) >= 999.0);
        assert!(p.moderate_floor_adaptive(500) >= 999.0);
    }

    #[test]
    fn adaptive_floor_handles_zero_rtt() {
        let p = AggressionPolicy::LOW;
        let r = p.moderate_floor_adaptive(0);
        assert_eq!(r, p.moderate_floor(), "0ms RTT should defer to base");
    }
}
