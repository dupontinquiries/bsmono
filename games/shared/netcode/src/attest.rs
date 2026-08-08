//! attest — who says the match was won, and what happens when they disagree.
//!
//! `tournament` holds results in memory and trusts whoever calls `report`. In a
//! peer-to-peer tournament there is no referee: the two players are the only
//! witnesses, they each report separately, and **they can disagree** — through a
//! desync, a disconnect either side read as the other forfeiting, or somebody
//! lying.
//!
//! A host that took the first report wins by whoever's packet arrived first,
//! which is a race the loser can win by reporting faster. So results are
//! **attested**: each side's claim is recorded separately, and only agreement
//! decides the match.
//!
//! ## Disagreement is a state, not an error
//!
//! Two conflicting claims do not produce a winner and do not produce a failure.
//! They produce `Disputed`, which the tournament shows and a human resolves.
//! Auto-resolving would mean picking a rule — first report, higher rank, coin
//! toss — and every one of those is exploitable by the player who knows it.
//!
//! ## One report each, and it cannot be revised
//!
//! A player who could re-report would simply wait to see the other's claim and
//! match or contradict it. The first claim from each side is final; a second is
//! ignored rather than rejected, because a retransmitted packet is ordinary and
//! must not look like cheating.
//!
//! ## Silence is not agreement
//!
//! One report with nothing from the other side stays `Pending`. This is the same
//! rule the bracket already applies to unplayed matches, and for the same
//! reason: during a partition the absent report is far more likely to be
//! delayed than withheld.

use crate::tournament::{EntrantId, Outcome};

/// What one player says happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// Who is making the claim.
    pub by: EntrantId,
    /// What they say the result was.
    pub outcome: Outcome,
}

/// The agreed state of a match, from the claims so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attested {
    /// Fewer than two claims, or a claim from only one side.
    Pending,
    /// Both sides agree.
    Agreed(Outcome),
    /// Both sides claimed, and they do not match.
    Disputed { a: Outcome, b: Outcome },
}

impl Attested {
    /// May the bracket act on this?
    pub fn is_settled(&self) -> bool { matches!(self, Attested::Agreed(_)) }

    /// Does this need a person?
    ///
    /// Separate from `is_settled` because a dispute and a pending match are
    /// both "not settled" and call for opposite responses: one needs an
    /// organiser, the other needs patience.
    pub fn needs_resolution(&self) -> bool { matches!(self, Attested::Disputed { .. }) }
}

/// The claims for one match between two known players.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchAttestation {
    pub a: EntrantId,
    pub b: EntrantId,
    claim_a: Option<Outcome>,
    claim_b: Option<Outcome>,
    /// An organiser's override, which outranks the claims.
    override_outcome: Option<Outcome>,
}

impl MatchAttestation {
    pub fn new(a: impl Into<EntrantId>, b: impl Into<EntrantId>) -> MatchAttestation {
        MatchAttestation { a: a.into(), b: b.into(),
                           claim_a: None, claim_b: None, override_outcome: None }
    }

    pub fn claim_of(&self, who: &str) -> Option<&Outcome> {
        if who == self.a { self.claim_a.as_ref() }
        else if who == self.b { self.claim_b.as_ref() }
        else { None }
    }

    /// Record a claim. Returns false when it was not recorded.
    ///
    /// Refused for three reasons, and the caller can tell them apart by asking:
    /// the claimant is not in this match, they have already claimed, or the
    /// claim names a winner who did not play.
    pub fn claim(&mut self, c: &Claim) -> bool {
        // A claim about a third party is either malformed or hostile, and
        // accepting it would let a spectator settle someone else's match.
        if let Outcome::Win(w) = &c.outcome {
            if *w != self.a && *w != self.b { return false; }
        }
        // Pending is not a claim — it is the absence of one, and recording it
        // would let a player consume their single report by saying nothing.
        if c.outcome == Outcome::Pending { return false; }

        if c.by == self.a {
            if self.claim_a.is_some() { return false; }
            self.claim_a = Some(c.outcome.clone());
            true
        } else if c.by == self.b {
            if self.claim_b.is_some() { return false; }
            self.claim_b = Some(c.outcome.clone());
            true
        } else {
            false
        }
    }

    /// An organiser decides a disputed match.
    ///
    /// Deliberately available for any state, not just disputes: a match can go
    /// wrong in ways the claim model does not model — a rule violation, a
    /// scheduling error — and an organiser who could only act on disputes would
    /// have to manufacture one.
    pub fn resolve(&mut self, outcome: Outcome) { self.override_outcome = Some(outcome); }

    pub fn was_overridden(&self) -> bool { self.override_outcome.is_some() }

    /// What the bracket should believe.
    pub fn state(&self) -> Attested {
        if let Some(o) = &self.override_outcome { return Attested::Agreed(o.clone()); }
        match (&self.claim_a, &self.claim_b) {
            (Some(x), Some(y)) if x == y => Attested::Agreed(x.clone()),
            (Some(x), Some(y)) => Attested::Disputed { a: x.clone(), b: y.clone() },
            // Silence is not agreement — during a partition the absent report
            // is far more likely to be delayed than withheld.
            _ => Attested::Pending,
        }
    }

    /// The outcome the bracket can use, if any.
    pub fn settled_outcome(&self) -> Option<Outcome> {
        match self.state() {
            Attested::Agreed(o) => Some(o),
            _ => None,
        }
    }
}

/// Every match's attestations, keyed by round and index.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AttestationLog {
    entries: Vec<((usize, usize), MatchAttestation)>,
}

impl AttestationLog {
    pub fn get(&self, round: usize, index: usize) -> Option<&MatchAttestation> {
        self.entries.iter().find(|(k, _)| *k == (round, index)).map(|(_, m)| m)
    }

    pub fn get_mut(&mut self, round: usize, index: usize)
        -> Option<&mut MatchAttestation> {
        self.entries.iter_mut().find(|(k, _)| *k == (round, index)).map(|(_, m)| m)
    }

    /// Register a match so claims can be filed against it.
    ///
    /// Registering twice is ignored rather than replacing: replacing would
    /// discard claims already filed, which is exactly what a player would do to
    /// erase a losing report.
    pub fn register(&mut self, round: usize, index: usize,
                    a: &str, b: &str) -> bool {
        if self.get(round, index).is_some() { return false; }
        self.entries.push(((round, index), MatchAttestation::new(a, b)));
        true
    }

    /// Matches an organiser must look at.
    ///
    /// Sorted so the list is stable between refreshes — an organiser working
    /// through disputes should not have rows move under them.
    pub fn disputes(&self) -> Vec<(usize, usize, &MatchAttestation)> {
        let mut v: Vec<(usize, usize, &MatchAttestation)> = self.entries.iter()
            .filter(|(_, m)| m.state().needs_resolution())
            .map(|((r, i), m)| (*r, *i, m))
            .collect();
        v.sort_by_key(|(r, i, _)| (*r, *i));
        v
    }

    /// Is every registered match settled?
    pub fn all_settled(&self) -> bool {
        self.entries.iter().all(|(_, m)| m.state().is_settled())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(by: &str, o: Outcome) -> Claim { Claim { by: by.into(), outcome: o } }
    fn win(w: &str) -> Outcome { Outcome::Win(w.into()) }

    fn m() -> MatchAttestation { MatchAttestation::new("p0", "p1") }

    #[test]
    fn agreement_settles_the_match() {
        let mut a = m();
        assert!(a.claim(&claim("p0", win("p0"))));
        assert!(a.claim(&claim("p1", win("p0"))));
        assert_eq!(a.state(), Attested::Agreed(win("p0")));
        assert_eq!(a.settled_outcome(), Some(win("p0")));
    }

    #[test]
    fn one_report_alone_is_pending_because_silence_is_not_agreement() {
        // During a partition the absent report is far more likely to be delayed
        // than withheld.
        let mut a = m();
        a.claim(&claim("p0", win("p0")));
        assert_eq!(a.state(), Attested::Pending);
        assert_eq!(a.settled_outcome(), None);
    }

    #[test]
    fn two_players_both_claiming_victory_is_disputed_not_decided() {
        // Taking the first report means the loser can win by reporting faster.
        let mut a = m();
        a.claim(&claim("p0", win("p0")));
        a.claim(&claim("p1", win("p1")));
        assert_eq!(a.state(), Attested::Disputed { a: win("p0"), b: win("p1") });
        assert!(!a.state().is_settled());
        assert!(a.state().needs_resolution());
    }

    #[test]
    fn a_dispute_and_a_pending_match_call_for_opposite_responses() {
        // Both are "not settled": one needs an organiser, the other needs
        // patience, and a UI that treated them alike would either nag about
        // matches still being played or hide real disputes.
        let mut pending = m();
        pending.claim(&claim("p0", win("p0")));
        assert!(!pending.state().needs_resolution());

        let mut disputed = m();
        disputed.claim(&claim("p0", win("p0")));
        disputed.claim(&claim("p1", win("p1")));
        assert!(disputed.state().needs_resolution());
    }

    #[test]
    fn a_player_cannot_revise_their_claim() {
        // Otherwise they wait to see the other's report and match or contradict
        // it.
        let mut a = m();
        assert!(a.claim(&claim("p0", win("p0"))));
        assert!(!a.claim(&claim("p0", win("p1"))));
        assert_eq!(a.claim_of("p0"), Some(&win("p0")));
    }

    #[test]
    fn a_retransmitted_identical_claim_is_ignored_rather_than_rejected_loudly() {
        // A duplicate packet is ordinary and must not look like cheating — the
        // second call returns false and changes nothing.
        let mut a = m();
        a.claim(&claim("p0", win("p0")));
        a.claim(&claim("p0", win("p0")));
        assert_eq!(a.claim_of("p0"), Some(&win("p0")));
        assert_eq!(a.state(), Attested::Pending, "still waiting on p1");
    }

    #[test]
    fn a_stranger_cannot_settle_someone_elses_match() {
        let mut a = m();
        assert!(!a.claim(&claim("p9", win("p0"))));
        assert_eq!(a.state(), Attested::Pending);
    }

    #[test]
    fn a_claim_naming_a_winner_who_did_not_play_is_refused() {
        // Malformed or hostile; accepting it would let a spectator be awarded a
        // match.
        let mut a = m();
        assert!(!a.claim(&claim("p0", win("p9"))));
        assert!(a.claim_of("p0").is_none());
    }

    #[test]
    fn pending_is_not_a_claim() {
        // Recording it would let a player consume their single report by saying
        // nothing.
        let mut a = m();
        assert!(!a.claim(&claim("p0", Outcome::Pending)));
        assert!(a.claim_of("p0").is_none());
    }

    #[test]
    fn both_sides_agreeing_on_a_draw_settles_it() {
        let mut a = m();
        a.claim(&claim("p0", Outcome::Draw));
        a.claim(&claim("p1", Outcome::Draw));
        assert_eq!(a.settled_outcome(), Some(Outcome::Draw));
    }

    #[test]
    fn a_draw_against_a_win_is_a_dispute() {
        // Not "the draw loses" — one of them is wrong and only a person knows
        // which.
        let mut a = m();
        a.claim(&claim("p0", win("p0")));
        a.claim(&claim("p1", Outcome::Draw));
        assert!(a.state().needs_resolution());
    }

    #[test]
    fn both_agreeing_the_match_was_void_settles_it_as_void() {
        // A desync both sides saw is the case this handles cleanly.
        let mut a = m();
        a.claim(&claim("p0", Outcome::Void));
        a.claim(&claim("p1", Outcome::Void));
        assert_eq!(a.settled_outcome(), Some(Outcome::Void));
    }

    #[test]
    fn an_organiser_override_outranks_the_claims() {
        let mut a = m();
        a.claim(&claim("p0", win("p0")));
        a.claim(&claim("p1", win("p1")));
        a.resolve(win("p1"));
        assert_eq!(a.settled_outcome(), Some(win("p1")));
        assert!(a.was_overridden());
    }

    #[test]
    fn an_organiser_can_act_without_a_dispute_existing() {
        // A match can go wrong in ways the claim model does not model — a rule
        // violation, a scheduling error — and an organiser who could only act
        // on disputes would have to manufacture one.
        let mut a = m();
        a.resolve(Outcome::Void);
        assert_eq!(a.settled_outcome(), Some(Outcome::Void));
    }

    #[test]
    fn registering_a_match_twice_does_not_erase_filed_claims() {
        // Which is exactly what a player would do to remove a losing report.
        let mut log = AttestationLog::default();
        assert!(log.register(0, 0, "p0", "p1"));
        log.get_mut(0, 0).unwrap().claim(&claim("p0", win("p0")));
        assert!(!log.register(0, 0, "p0", "p1"));
        assert_eq!(log.get(0, 0).unwrap().claim_of("p0"), Some(&win("p0")));
    }

    #[test]
    fn the_dispute_list_is_stable_and_holds_only_disputes() {
        // An organiser working through them should not have rows move.
        let mut log = AttestationLog::default();
        log.register(1, 0, "a", "b");
        log.register(0, 1, "c", "d");
        log.register(0, 0, "e", "f");
        for (r, i) in [(1usize, 0usize), (0, 1)] {
            let m = log.get_mut(r, i).unwrap();
            let (x, y) = (m.a.clone(), m.b.clone());
            m.claim(&claim(&x, Outcome::Win(x.clone())));
            m.claim(&claim(&y, Outcome::Win(y.clone())));
        }
        let d = log.disputes();
        assert_eq!(d.len(), 2, "the unclaimed match is not a dispute");
        assert_eq!((d[0].0, d[0].1), (0, 1));
        assert_eq!((d[1].0, d[1].1), (1, 0));
    }

    #[test]
    fn an_empty_log_is_settled_rather_than_blocked() {
        // Nothing to wait for is not the same as waiting.
        let log = AttestationLog::default();
        assert!(log.all_settled());
        assert!(log.disputes().is_empty());
    }
}

/// Apply every settled attestation to a tournament round.
///
/// The seam between the two halves: the bracket already refuses to advance
/// while anything is `Pending`, and this is what lets a claim become a result
/// without the bracket ever trusting a single player's word.
///
/// Returns how many results were applied. Disputed and pending matches are left
/// alone rather than skipped-with-a-default — a bracket that filled them in
/// would advance somebody on the strength of one unverified report, which is
/// the exact failure attestation exists to prevent.
pub fn apply_to_round(log: &AttestationLog, round_index: usize,
                      round: &mut crate::tournament::Round) -> usize {
    let mut applied = 0;
    for i in 0..round.pairings.len() {
        let Some(m) = log.get(round_index, i) else { continue };
        let Some(o) = m.settled_outcome() else { continue };
        // Round::report does its own validation — a bye cannot be reported on,
        // and a winner must have been in the pairing. Going through it rather
        // than around it keeps one gatekeeper instead of two.
        if round.report(i, o) { applied += 1; }
    }
    applied
}

#[cfg(test)]
mod bracket_tests {
    use super::*;
    use crate::tournament::{Outcome, Tournament};

    fn claim(by: &str, o: Outcome) -> Claim { Claim { by: by.into(), outcome: o } }

    #[test]
    fn an_agreed_result_advances_the_bracket_and_a_disputed_one_does_not() {
        // The whole point: a claim becomes a result only when both sides say
        // the same thing, and the bracket never trusts one player's word.
        let mut t = Tournament::new("cup",
            vec!["p0".into(), "p1".into(), "p2".into(), "p3".into()]);
        t.start();

        let mut log = AttestationLog::default();
        log.register(0, 0, "p0", "p1");
        log.register(0, 1, "p2", "p3");

        // Match 0: both agree p0 won.
        let m0 = log.get_mut(0, 0).unwrap();
        m0.claim(&claim("p0", Outcome::Win("p0".into())));
        m0.claim(&claim("p1", Outcome::Win("p0".into())));
        // Match 1: they disagree.
        let m1 = log.get_mut(0, 1).unwrap();
        m1.claim(&claim("p2", Outcome::Win("p2".into())));
        m1.claim(&claim("p3", Outcome::Win("p3".into())));

        let applied = apply_to_round(&log, 0, t.round_mut(0).unwrap());
        assert_eq!(applied, 1);
        assert!(!t.rounds()[0].complete(), "the dispute holds the round open");
        assert!(!t.advance(), "and nobody is paired on half a result set");
    }

    #[test]
    fn an_organisers_resolution_unblocks_the_round() {
        let mut t = Tournament::new("cup", vec!["p0".into(), "p1".into()]);
        t.start();
        let mut log = AttestationLog::default();
        log.register(0, 0, "p0", "p1");
        let m = log.get_mut(0, 0).unwrap();
        m.claim(&claim("p0", Outcome::Win("p0".into())));
        m.claim(&claim("p1", Outcome::Win("p1".into())));
        assert_eq!(apply_to_round(&log, 0, t.round_mut(0).unwrap()), 0);

        log.get_mut(0, 0).unwrap().resolve(Outcome::Win("p1".into()));
        assert_eq!(apply_to_round(&log, 0, t.round_mut(0).unwrap()), 1);
        assert_eq!(t.champion(), Some(&"p1".to_string()));
    }

    #[test]
    fn a_bye_is_untouched_by_attestation() {
        // Round::report already refuses reports on byes; going through it keeps
        // one gatekeeper rather than two that can disagree.
        let mut t = Tournament::new("cup",
            vec!["p0".into(), "p1".into(), "p2".into()]);
        t.start();
        let mut log = AttestationLog::default();
        log.register(0, 1, "p2", "p2");
        log.get_mut(0, 1).unwrap().resolve(Outcome::Win("p2".into()));
        assert_eq!(apply_to_round(&log, 0, t.round_mut(0).unwrap()), 0);
    }

    #[test]
    fn matches_with_no_attestation_are_left_pending() {
        // Not filled with a default: that would advance somebody on the
        // strength of one unverified report.
        let mut t = Tournament::new("cup", vec!["p0".into(), "p1".into()]);
        t.start();
        let log = AttestationLog::default();
        assert_eq!(apply_to_round(&log, 0, t.round_mut(0).unwrap()), 0);
        assert!(!t.rounds()[0].complete());
    }
}
