//! v0.1.623: new-speaker detection — decides when to offer the OWNER an
//! "someone new is speaking, add them?" prompt. Built to the demo's
//! requirement: a brand-new UNKNOWN voice should surface the enroll
//! offer, but conservatively — the owner confirms first, and a one-off
//! ambiguous read of the owner's OWN voice must NOT trigger it.
//!
//! Why this is needed: speakers::identify() returns None for an unknown
//! voice (no match, or an ambiguous match it refuses to guess on), and
//! today nothing acts on that — unknown utterances silently fall
//! through, so a guest who wants to jump in is never offered enrollment
//! (the existing _mutedEnrollPopup only fires for an already-enrolled
//! but muted speaker). This fills that gap.
//!
//! The decision is PURE: it operates on per-utterance signals the listen
//! path already has (was the speaker identified? how similar was this
//! utterance to the *previous* unknown one?), not on audio. The caller
//! computes the similarities; this module only decides.
//!
//! Avoiding false positives (the hard part):
//!   - Only UNKNOWN-verdict utterances feed the detector (identified
//!     speakers, owner or guest, never do).
//!   - We require the recent unknowns to CLUSTER — each similar to the
//!     prior unknown above a threshold — so scattered noise / one-off
//!     ambiguous reads don't accumulate into a false "new person".
//!   - We require a minimum RUN of clustered unknowns before offering.
//!   - After offering once, we latch until reset (the owner answers, or
//!     the guest is enrolled / dismissed) so we don't nag.

/// What the caller should do with this utterance, per the detector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Nothing special — let normal processing continue.
    Idle,
    /// A distinct new voice has recurred enough; offer the OWNER the
    /// "add new speaker?" prompt. Emitted exactly once per run (latched).
    OfferEnroll,
}

/// Per-utterance input. `identified` is true when speakers::identify()
/// returned Some (any known speaker — owner or guest). When false, the
/// utterance is an unknown verdict and `sim_to_prev_unknown` is the
/// tonal similarity (0..1) of THIS unknown utterance to the PREVIOUS
/// unknown one (the caller caches the previous unknown profile and
/// computes this; on the first unknown it passes 0.0).
pub struct Utterance {
    pub identified: bool,
    pub sim_to_prev_unknown: f32,
}

pub struct NewSpeakerDetector {
    /// consecutive clustered-unknown count
    run: u32,
    /// min run length before offering
    min_run: u32,
    /// similarity floor for two unknowns to count as "the same person"
    cluster_thr: f32,
    /// latched after an offer so we don't repeat until reset
    offered: bool,
}

impl Default for NewSpeakerDetector {
    fn default() -> Self {
        // min_run 3: a guest must speak ~3 clustered times before we
        // bother the owner — deliberately conservative ("owner confirms
        // first" + don't nag). cluster_thr 0.80: same-person tonal reads
        // run ~0.97 day-to-day, different people ~0.90, so 0.80 is a
        // loose-but-safe "plausibly the same unknown person" bar that
        // still rejects scattered noise.
        Self { run: 0, min_run: 3, cluster_thr: 0.80, offered: false }
    }
}

impl NewSpeakerDetector {
    pub fn new(min_run: u32, cluster_thr: f32) -> Self {
        Self { run: 0, min_run: min_run.max(1), cluster_thr, offered: false }
    }

    /// Feed one utterance; get the decision.
    pub fn observe(&mut self, u: &Utterance) -> Decision {
        if u.identified {
            // A known speaker spoke — the unknown run is broken. (This is
            // what protects the owner: their identified utterances reset
            // any accumulating count, and an occasional ambiguous read of
            // the owner won't cluster with a real guest's voice anyway.)
            self.run = 0;
            return Decision::Idle;
        }
        // Unknown utterance. Does it cluster with the previous unknown?
        if self.run == 0 {
            // first unknown in a potential run — start counting
            self.run = 1;
        } else if u.sim_to_prev_unknown >= self.cluster_thr {
            self.run = self.run.saturating_add(1);
        } else {
            // an unknown that DOESN'T match the prior unknown → different
            // voice or noise; restart the run at this new voice.
            self.run = 1;
        }
        if self.run >= self.min_run && !self.offered {
            self.offered = true;
            return Decision::OfferEnroll;
        }
        Decision::Idle
    }

    /// Reset after the owner answers the prompt (enrolled or dismissed),
    /// so a later genuinely-new voice can offer again.
    pub fn reset(&mut self) {
        self.run = 0;
        self.offered = false;
    }

    /// Has an offer already been surfaced this run? (for the caller to
    /// avoid double-prompting across the FFI boundary)
    pub fn already_offered(&self) -> bool {
        self.offered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unknown(sim: f32) -> Utterance { Utterance { identified: false, sim_to_prev_unknown: sim } }
    fn known() -> Utterance { Utterance { identified: true, sim_to_prev_unknown: 0.0 } }

    #[test]
    fn offers_after_clustered_run() {
        let mut d = NewSpeakerDetector::default(); // min_run 3
        assert_eq!(d.observe(&unknown(0.0)), Decision::Idle); // 1st unknown
        assert_eq!(d.observe(&unknown(0.95)), Decision::Idle); // 2nd, clusters
        assert_eq!(d.observe(&unknown(0.93)), Decision::OfferEnroll); // 3rd → offer
    }

    #[test]
    fn latches_after_offering() {
        let mut d = NewSpeakerDetector::default();
        d.observe(&unknown(0.0));
        d.observe(&unknown(0.95));
        assert_eq!(d.observe(&unknown(0.93)), Decision::OfferEnroll);
        // keeps talking — must NOT keep offering
        assert_eq!(d.observe(&unknown(0.94)), Decision::Idle);
        assert_eq!(d.observe(&unknown(0.92)), Decision::Idle);
    }

    #[test]
    fn known_speaker_resets_run() {
        // owner (identified) speaking between guest utterances breaks the
        // run, so a guest who only speaks once between owner turns never
        // accumulates to an offer.
        let mut d = NewSpeakerDetector::default();
        d.observe(&unknown(0.0));
        d.observe(&known());            // reset
        d.observe(&unknown(0.0));       // run restarts at 1
        d.observe(&known());            // reset again
        assert_eq!(d.observe(&unknown(0.0)), Decision::Idle); // still only run=1
    }

    #[test]
    fn scattered_unknowns_dont_cluster() {
        // unknown reads that DON'T match each other (different noise /
        // different people) should not accumulate into one offer.
        let mut d = NewSpeakerDetector::default();
        assert_eq!(d.observe(&unknown(0.0)), Decision::Idle);  // run 1
        assert_eq!(d.observe(&unknown(0.40)), Decision::Idle); // below thr → run 1
        assert_eq!(d.observe(&unknown(0.30)), Decision::Idle); // below thr → run 1
        // never reached min_run because nothing clustered
    }

    #[test]
    fn reset_allows_new_offer_later() {
        let mut d = NewSpeakerDetector::default();
        d.observe(&unknown(0.0));
        d.observe(&unknown(0.95));
        assert_eq!(d.observe(&unknown(0.93)), Decision::OfferEnroll);
        d.reset(); // owner answered
        // a new guest later
        d.observe(&unknown(0.0));
        d.observe(&unknown(0.95));
        assert_eq!(d.observe(&unknown(0.93)), Decision::OfferEnroll);
    }

    #[test]
    fn custom_eagerness() {
        // min_run 2 = more eager
        let mut d = NewSpeakerDetector::new(2, 0.80);
        assert_eq!(d.observe(&unknown(0.0)), Decision::Idle);
        assert_eq!(d.observe(&unknown(0.90)), Decision::OfferEnroll);
    }

    #[test]
    fn run_of_one_never_offers_with_high_min() {
        let mut d = NewSpeakerDetector::new(5, 0.80);
        for _ in 0..3 { assert_eq!(d.observe(&unknown(0.0)), Decision::Idle); }
    }
}
