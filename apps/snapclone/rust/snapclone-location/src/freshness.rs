//! Position freshness & confidence — the model that gives the wayfinder
//! "near-realtime usefulness".
//!
//! The wayfinder points an arrow at where a peer is. The danger is a confident
//! arrow aimed at where someone was an hour ago: that's worse than no arrow,
//! because it misleads. Today the wayfinder draws bearing/distance whenever a
//! location exists *at all*, with no regard for how old or accurate the fix is.
//!
//! This module fixes that by grading every fix into a [`Confidence`] from two
//! inputs the storage layer already has (`recorded_at`, `accuracy_m`):
//!
//! - **age** — how long ago the fix was recorded. A GPS point decays in
//!   usefulness: seconds-old is gospel, minutes-old is good, tens-of-minutes is
//!   "roughly", hours-old is "last seen".
//! - **accuracy** — the fix's own error radius. A 5km IP-geolocation fix should
//!   never read as a precise arrow even if it's seconds old.
//!
//! The output [`PresentationTier`] is what the Qt layer renders against: a solid
//! arrow, a faded "approximate" arrow, or a "last seen N ago" label with no
//! arrow at all. Keeping this as tested pure logic means the thresholds — which
//! the wayfinder's feel depends on — can be tuned with a safety net instead of
//! being magic numbers buried in paint code.

/// Graded confidence in a peer's displayed position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// Fresh + accurate → trust the arrow fully.
    Live,
    /// A little old or a little coarse → show it, but signal "approximate".
    Recent,
    /// Old or coarse enough that the *direction* is only a rough hint.
    Stale,
    /// So old it's only a "last known" record, not a navigation target.
    LastKnown,
}

/// What the wayfinder should actually draw for this node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationTier {
    /// Solid bearing arrow + distance, full opacity.
    SolidArrow,
    /// Faded arrow + distance with an "~" approximate marker.
    FadedArrow,
    /// No arrow; just a "last seen N ago" label at the node's last position.
    LastSeenLabel,
}

/// Thresholds (tunable). Times in seconds, accuracy in meters.
#[derive(Debug, Clone, Copy)]
pub struct FreshnessPolicy {
    /// <= this age AND accurate → Live.
    pub live_age_secs: i64,
    /// <= this age → at best Recent.
    pub recent_age_secs: i64,
    /// <= this age → at best Stale; beyond → LastKnown.
    pub stale_age_secs: i64,
    /// Accuracy at/under this is "precise" (won't cap a fresh fix below Live).
    pub precise_accuracy_m: f32,
    /// Accuracy worse than this can never read better than Stale (e.g. IP geo).
    pub coarse_accuracy_m: f32,
}

impl Default for FreshnessPolicy {
    fn default() -> Self {
        // Tuned for person-finding on foot/vehicle: a couple minutes is still
        // "live enough", ten minutes is "recent", an hour is the nav cutoff.
        FreshnessPolicy {
            live_age_secs:      120,    // 2 min
            recent_age_secs:    600,    // 10 min
            stale_age_secs:     3600,   // 1 h
            precise_accuracy_m: 50.0,   // good GPS
            coarse_accuracy_m:  1000.0, // 1km+ ≈ IP / cell-tower geo
        }
    }
}

/// Grade a fix. `age_secs` is how long ago it was recorded (>= 0);
/// `accuracy_m` is its error radius (0 = unknown, treated as precise).
pub fn confidence(policy: &FreshnessPolicy, age_secs: i64, accuracy_m: f32) -> Confidence {
    let age = age_secs.max(0);
    // Accuracy ceiling: a coarse fix can never be better than Stale.
    let coarse = accuracy_m > policy.coarse_accuracy_m;
    let imprecise = accuracy_m > policy.precise_accuracy_m;

    // Age tier first.
    let by_age = if age <= policy.live_age_secs {
        Confidence::Live
    } else if age <= policy.recent_age_secs {
        Confidence::Recent
    } else if age <= policy.stale_age_secs {
        Confidence::Stale
    } else {
        Confidence::LastKnown
    };

    // Apply accuracy ceilings (never let accuracy IMPROVE the age tier).
    match by_age {
        Confidence::Live if coarse => Confidence::Stale,
        Confidence::Live if imprecise => Confidence::Recent,
        Confidence::Recent if coarse => Confidence::Stale,
        other => other,
    }
}

/// Map confidence to what the wayfinder draws.
pub fn presentation(conf: Confidence) -> PresentationTier {
    match conf {
        Confidence::Live => PresentationTier::SolidArrow,
        Confidence::Recent => PresentationTier::FadedArrow,
        Confidence::Stale => PresentationTier::FadedArrow,
        Confidence::LastKnown => PresentationTier::LastSeenLabel,
    }
}

/// Opacity multiplier (0..1) the renderer applies to the arrow/avatar, so the
/// visual fades smoothly as confidence drops. Pure function of confidence so
/// the paint code stays declarative.
pub fn opacity(conf: Confidence) -> f32 {
    match conf {
        Confidence::Live => 1.0,
        Confidence::Recent => 0.85,
        Confidence::Stale => 0.55,
        Confidence::LastKnown => 0.35,
    }
}

/// Human "last seen" phrasing for a stale/last-known age. Compact for a node
/// label ("12s", "5m", "2h", "3d"). The wayfinder shows this instead of a
/// distance when the tier is LastSeenLabel.
pub fn last_seen_label(age_secs: i64) -> String {
    let a = age_secs.max(0);
    if a < 60 {
        format!("{}s ago", a)
    } else if a < 3600 {
        format!("{}m ago", a / 60)
    } else if a < 86_400 {
        format!("{}h ago", a / 3600)
    } else {
        format!("{}d ago", a / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pol() -> FreshnessPolicy { FreshnessPolicy::default() }

    #[test]
    fn fresh_accurate_is_live() {
        assert_eq!(confidence(&pol(), 10, 20.0), Confidence::Live);
        assert_eq!(presentation(Confidence::Live), PresentationTier::SolidArrow);
    }

    #[test]
    fn age_tiers() {
        assert_eq!(confidence(&pol(), 30, 10.0), Confidence::Live);     // 30s
        assert_eq!(confidence(&pol(), 300, 10.0), Confidence::Recent);  // 5m
        assert_eq!(confidence(&pol(), 1800, 10.0), Confidence::Stale);  // 30m
        assert_eq!(confidence(&pol(), 7200, 10.0), Confidence::LastKnown); // 2h
    }

    #[test]
    fn coarse_fix_never_reads_live_even_when_fresh() {
        // 5km IP-geo fix, 5 seconds old → capped at Stale (direction is a hint).
        assert_eq!(confidence(&pol(), 5, 5000.0), Confidence::Stale);
    }

    #[test]
    fn imprecise_but_fresh_drops_to_recent() {
        // 200m accuracy, fresh → not Live (not precise) but not coarse → Recent.
        assert_eq!(confidence(&pol(), 5, 200.0), Confidence::Recent);
    }

    #[test]
    fn accuracy_never_improves_age_tier() {
        // A perfectly precise fix that is 2h old is still LastKnown.
        assert_eq!(confidence(&pol(), 7200, 1.0), Confidence::LastKnown);
    }

    #[test]
    fn boundary_ages_inclusive() {
        assert_eq!(confidence(&pol(), 120, 10.0), Confidence::Live);     // == live cutoff
        assert_eq!(confidence(&pol(), 121, 10.0), Confidence::Recent);   // just over
        assert_eq!(confidence(&pol(), 600, 10.0), Confidence::Recent);   // == recent cutoff
        assert_eq!(confidence(&pol(), 3600, 10.0), Confidence::Stale);   // == stale cutoff
        assert_eq!(confidence(&pol(), 3601, 10.0), Confidence::LastKnown);
    }

    #[test]
    fn negative_age_clamped() {
        // Clock skew could yield a negative age; treat as 0 (fresh).
        assert_eq!(confidence(&pol(), -50, 10.0), Confidence::Live);
    }

    #[test]
    fn zero_accuracy_treated_precise() {
        assert_eq!(confidence(&pol(), 10, 0.0), Confidence::Live);
    }

    #[test]
    fn presentation_and_opacity_monotonic() {
        // Opacity must not increase as confidence worsens.
        let order = [Confidence::Live, Confidence::Recent, Confidence::Stale, Confidence::LastKnown];
        for w in order.windows(2) {
            assert!(opacity(w[0]) >= opacity(w[1]),
                    "opacity should be non-increasing across tiers");
        }
        assert_eq!(presentation(Confidence::LastKnown), PresentationTier::LastSeenLabel);
    }

    #[test]
    fn last_seen_phrasing() {
        assert_eq!(last_seen_label(5), "5s ago");
        assert_eq!(last_seen_label(90), "1m ago");
        assert_eq!(last_seen_label(7200), "2h ago");
        assert_eq!(last_seen_label(180_000), "2d ago");
        assert_eq!(last_seen_label(-5), "0s ago");
    }
}
