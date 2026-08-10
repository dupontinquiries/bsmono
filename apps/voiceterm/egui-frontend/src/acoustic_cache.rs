//! acoustic_cache — the accept/reject policy from `acoustic_index.rs`.
//!
//! A recognition cache that sits **upstream of whisper**: an utterance already
//! confirmed once gets a compact fingerprint, and a close enough match at
//! listen time emits the cached transcript and **skips transcription
//! entirely**.
//!
//! That is a cache whose miss costs latency and whose FALSE HIT puts words in
//! someone's mouth. So the policy is the port — the fingerprinting belongs with
//! the audio thread, but what counts as close enough does not.
//!
//! ## Three gates, not one
//!
//! The original: emit only if *"the distance is small enough AND the speaker
//! matches AND the entry has been confirmed often enough"*. All three, and the
//! third is the one an implementation drops: a single confirmation is one
//! person agreeing once, which is not evidence that a fingerprint means what
//! the transcript says.
//!
//! ## Cross-speaker is twice as permissive, deliberately
//!
//! Same-speaker promotes at 0.10, cross-speaker at 0.20 — both tuned in
//! v0.1.304 rather than picked. The asymmetry looks backwards until you read it
//! as what it is: a same-speaker match is the common case and cheap to get
//! again, so it can afford to be strict; a cross-speaker match is rare and
//! discarding it loses the whole benefit of a shared corpus.
//!
//! Thresholds are runtime-settable and clamped to 0..=1, with **zero meaning
//! disabled** — which is how the feature ships dark on a new speaker.
//!
//! ## Shadow mode is why the numbers are trustworthy
//!
//! Before promoting anything, the original records `(distance, agreement)`
//! pairs into a histogram without acting on them. That is the difference
//! between a threshold somebody chose and one somebody measured, and the
//! histogram geometry ports with it.

/// Fingerprint width. 39 = 13 MFCC coefficients with deltas and
/// delta-deltas — the standard triple, not an arbitrary size.
pub const EMBED_DIM: usize = 39;

/// Distance below which a same-speaker match may be promoted.
pub const PROMOTE_SAME: f32 = 0.10;
/// Cross-speaker equivalent — deliberately looser, see the module docs.
pub const PROMOTE_CROSS: f32 = 0.20;

/// Confirmations before an entry may serve a cached transcript.
///
/// One is not enough: a single confirmation is one person agreeing once, which
/// is not evidence that a fingerprint means what the transcript says.
pub const MIN_CONFIRMS: u32 = 2;

/// Index caps, so a long-lived corpus cannot grow without bound.
pub const MAX_PER_SPEAKER: usize = 5_000;
pub const MAX_TOTAL: usize = 20_000;
pub const MAX_TRANSCRIPT_LEN: usize = 200;

/// Shadow histogram geometry: 22 buckets of 0.05 covers 0..1.1, so a distance
/// slightly above 1.0 still lands in a bucket rather than being dropped.
pub const HISTOGRAM_BUCKETS: usize = 22;
pub const HISTOGRAM_BUCKET_WIDTH: f32 = 0.05;

/// A candidate match considered at listen time.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub transcript: String,
    pub distance: f32,
    pub same_speaker: bool,
    pub confirms: u32,
}

/// What the cache decided, and why — the reason is carried so a log line can
/// explain a miss instead of just reporting one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// Emit the cached transcript; skip transcription.
    Promote,
    /// Too far from anything known.
    TooFar,
    /// Close, but not confirmed enough to trust yet.
    Unconfirmed,
    /// Promotion is switched off for this case.
    Disabled,
    /// Nothing in the index to compare against.
    NoCandidate,
}

/// Runtime thresholds, clamped as the original clamps them.
#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    pub same: f32,
    pub cross: f32,
}

impl Default for Thresholds {
    fn default() -> Self { Thresholds { same: PROMOTE_SAME, cross: PROMOTE_CROSS } }
}

impl Thresholds {
    /// Clamped to 0..=1. Above 1 would promote on any distance at all, which
    /// for a cache that skips transcription means emitting arbitrary text.
    pub fn set(same: f32, cross: f32) -> Thresholds {
        Thresholds { same: same.clamp(0.0, 1.0), cross: cross.clamp(0.0, 1.0) }
    }

    pub fn for_match(&self, same_speaker: bool) -> f32 {
        if same_speaker { self.same } else { self.cross }
    }

    /// Zero disables — how the feature ships dark on a new speaker.
    pub fn enabled(&self, same_speaker: bool) -> bool { self.for_match(same_speaker) > 0.0 }
}

/// The whole decision.
///
/// Order matters: a disabled threshold is reported as `Disabled` rather than
/// `TooFar`, because "switched off" and "no match" are different problems and a
/// log that conflates them sends someone tuning a threshold that is not in use.
pub fn decide(c: Option<&Candidate>, t: &Thresholds) -> Verdict {
    let Some(c) = c else { return Verdict::NoCandidate };
    if !t.enabled(c.same_speaker) { return Verdict::Disabled; }
    if !(c.distance <= t.for_match(c.same_speaker)) {
        // `!(<=)` rather than `>`: a NaN distance from a degenerate
        // fingerprint must reject, and `NaN > x` is false.
        return Verdict::TooFar;
    }
    if c.confirms < MIN_CONFIRMS { return Verdict::Unconfirmed; }
    Verdict::Promote
}

/// Which shadow bucket a distance falls in. Saturates at the last bucket so an
/// outlier is counted rather than lost.
pub fn bucket(distance: f32) -> usize {
    if !distance.is_finite() || distance < 0.0 { return 0; }
    ((distance / HISTOGRAM_BUCKET_WIDTH) as usize).min(HISTOGRAM_BUCKETS - 1)
}

/// A shadow observation: what the cache WOULD have done, and whether it would
/// have been right. Recorded without acting, which is what makes the eventual
/// threshold a measurement.
#[derive(Clone, Debug, Default)]
pub struct Shadow {
    matched: [u32; HISTOGRAM_BUCKETS],
    missed: [u32; HISTOGRAM_BUCKETS],
}

impl Shadow {
    pub fn record(&mut self, distance: f32, agreed: bool) {
        let b = bucket(distance);
        if agreed { self.matched[b] += 1 } else { self.missed[b] += 1 }
    }

    pub fn at(&self, b: usize) -> (u32, u32) { (self.matched[b], self.missed[b]) }

    /// Agreement rate at or below a distance — the number a threshold should be
    /// chosen from.
    pub fn agreement_below(&self, distance: f32) -> Option<f32> {
        let top = bucket(distance);
        let (mut m, mut n) = (0u32, 0u32);
        for b in 0..=top {
            m += self.matched[b];
            n += self.matched[b] + self.missed[b];
        }
        if n == 0 { None } else { Some(m as f32 / n as f32) }
    }
}

/// Is an entry worth keeping when the index is full?
///
/// Confirmations first, then recency. A frequently-confirmed old entry is the
/// cache's whole value; evicting it for a newer single-confirm one would empty
/// the cache of exactly what works.
pub fn eviction_rank(confirms: u32, last_used_ms: u64) -> (u32, u64) { (confirms, last_used_ms) }

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(distance: f32, same: bool, confirms: u32) -> Candidate {
        Candidate { transcript: "open the terminal".into(), distance, same_speaker: same, confirms }
    }

    #[test]
    fn all_three_gates_must_pass() {
        // "the distance is small enough AND the speaker matches AND the entry
        // has been confirmed often enough" — the third is the one an
        // implementation drops.
        let t = Thresholds::default();
        assert_eq!(decide(Some(&cand(0.05, true, 5)), &t), Verdict::Promote);
        assert_eq!(decide(Some(&cand(0.50, true, 5)), &t), Verdict::TooFar);
        assert_eq!(decide(Some(&cand(0.05, true, 1)), &t), Verdict::Unconfirmed);
    }

    #[test]
    fn one_confirmation_is_not_evidence() {
        // A single confirmation is one person agreeing once, which is not
        // evidence that a fingerprint means what the transcript says — and the
        // cost of being wrong is words in somebody's mouth.
        let t = Thresholds::default();
        assert_eq!(decide(Some(&cand(0.01, true, 1)), &t), Verdict::Unconfirmed);
        assert_eq!(decide(Some(&cand(0.01, true, MIN_CONFIRMS)), &t), Verdict::Promote);
    }

    #[test]
    fn cross_speaker_is_looser_and_that_is_deliberate() {
        // A same-speaker match is common and cheap to get again, so it can
        // afford to be strict; a cross-speaker match is rare, and discarding it
        // loses the whole benefit of a shared corpus.
        let t = Thresholds::default();
        assert!(t.cross > t.same);
        assert_eq!(decide(Some(&cand(0.15, false, 5)), &t), Verdict::Promote);
        assert_eq!(decide(Some(&cand(0.15, true, 5)), &t), Verdict::TooFar,
                   "the same distance is too far for the strict case");
    }

    #[test]
    fn zero_disables_and_says_so_rather_than_reporting_too_far() {
        // "Switched off" and "no match" are different problems; conflating them
        // sends someone tuning a threshold that is not in use.
        let t = Thresholds::set(0.0, 0.2);
        assert_eq!(decide(Some(&cand(0.001, true, 9)), &t), Verdict::Disabled);
        assert_eq!(decide(Some(&cand(0.001, false, 9)), &t), Verdict::Promote,
                   "the other case is unaffected");
    }

    #[test]
    fn thresholds_are_clamped_so_a_cache_cannot_promote_on_anything() {
        // Above 1.0 would promote at any distance, which for a cache that skips
        // transcription means emitting arbitrary text.
        let t = Thresholds::set(50.0, -3.0);
        assert_eq!(t.same, 1.0);
        assert_eq!(t.cross, 0.0);
    }

    #[test]
    fn a_nan_distance_rejects_rather_than_promoting() {
        // A degenerate fingerprint (silence, a divide by zero in the metric)
        // yields NaN, and `NaN > x` is false — so a naive `>` check would
        // PROMOTE it. The comparison is written to reject.
        let t = Thresholds::default();
        assert_eq!(decide(Some(&cand(f32::NAN, true, 9)), &t), Verdict::TooFar);
        assert_eq!(decide(Some(&cand(f32::INFINITY, true, 9)), &t), Verdict::TooFar);
    }

    #[test]
    fn an_empty_index_is_its_own_verdict() {
        // First run, or a new speaker. Not a failure.
        assert_eq!(decide(None, &Thresholds::default()), Verdict::NoCandidate);
    }

    #[test]
    fn the_boundary_promotes() {
        // Exactly at the tuned threshold is inside it; rejecting there would
        // make the measured number mean something slightly different from what
        // was measured.
        let t = Thresholds::default();
        assert_eq!(decide(Some(&cand(PROMOTE_SAME, true, 5)), &t), Verdict::Promote);
    }

    #[test]
    fn the_histogram_covers_past_one_so_outliers_are_counted() {
        // 22 buckets of 0.05 reaches 1.1; a distance above 1.0 still lands
        // somewhere rather than being dropped from the tuning data.
        assert_eq!(bucket(0.0), 0);
        assert_eq!(bucket(0.07), 1);
        assert!(bucket(1.05) < HISTOGRAM_BUCKETS);
        assert_eq!(bucket(99.0), HISTOGRAM_BUCKETS - 1, "an outlier saturates rather than panicking");
        assert_eq!(bucket(f32::NAN), 0);
        assert_eq!(bucket(-1.0), 0);
    }

    #[test]
    fn shadow_mode_measures_before_anything_is_promoted() {
        // The difference between a threshold somebody chose and one somebody
        // measured.
        let mut s = Shadow::default();
        for _ in 0..90 { s.record(0.05, true); }
        for _ in 0..10 { s.record(0.05, false); }
        for _ in 0..50 { s.record(0.60, false); }
        let near = s.agreement_below(0.09).expect("observations below 0.09");
        assert!((near - 0.9).abs() < 0.01, "close matches agreed 90% of the time: {near}");
        let all = s.agreement_below(1.0).unwrap();
        assert!(all < near, "including the far ones must lower it: {all} vs {near}");
    }

    #[test]
    fn agreement_with_no_observations_is_none_rather_than_zero() {
        // Zero would read as "always wrong" and argue for disabling a feature
        // nobody has data on yet.
        assert_eq!(Shadow::default().agreement_below(0.1), None);
    }

    #[test]
    fn eviction_keeps_what_works() {
        // Confirmations first: a frequently-confirmed old entry is the cache's
        // whole value, and evicting it for a newer single-confirm one empties
        // the cache of exactly what works.
        let proven_old = eviction_rank(40, 1_000);
        let fresh_new = eviction_rank(1, 9_999_999);
        assert!(proven_old > fresh_new);
        // recency only breaks ties
        assert!(eviction_rank(3, 500) < eviction_rank(3, 900));
    }

    #[test]
    fn the_caps_bound_a_long_lived_corpus() {
        assert!(MAX_PER_SPEAKER < MAX_TOTAL, "one speaker must not fill the index");
        assert!(MAX_TOTAL / MAX_PER_SPEAKER >= 4, "room for several speakers");
        assert_eq!(EMBED_DIM, 39, "13 MFCC with deltas and delta-deltas");
    }
}
