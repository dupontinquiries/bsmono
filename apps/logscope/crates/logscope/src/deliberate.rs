//! deliberate.rs — several small Thinkers arguing, instead of one large one.
//!
//! ## Why this is a bandwidth question, not a quality question
//!
//! Decode is bound by bytes read per token. Four 3B models at Q4 read ~6.7 GiB per token between them;
//! one 30B reads ~17 GiB. So a panel of small models is **cheaper per round** than the single large one
//! — and if two rounds of disagreement produce an answer the big model would have given, the panel wins
//! on the axis that actually limits us.
//!
//! That framing matters because it sets the bar. The panel does not have to be *better* than the large
//! model. It has to be *as good* while reading fewer bytes, and it gets roughly two and a half rounds
//! to do it in.
//!
//! ## The failure mode this is built against
//!
//! Ensembles that vote converge on the *majority's* error. Small models trained on similar data share
//! their mistakes, so three of them agreeing is much weaker evidence than three independent observers
//! agreeing — and a naive majority vote reports that weak agreement with the confidence of a quorum.
//!
//! So agreement is only trusted when the models can be shown to disagree elsewhere in the same session
//! (`Panel::has_shown_independence`). A panel that has never disagreed about anything is not a panel; it
//! is one model queried repeatedly, and its consensus means nothing.

/// One participant's answer for a round.
#[derive(Clone, PartialEq, Debug)]
pub struct Opinion {
    pub model: String,
    pub answer: String,
    /// The model's own confidence, when it can be read from logits (see `reflex::Verdict`).
    pub confidence: Option<f32>,
}

/// What a round concluded.
#[derive(Clone, PartialEq, Debug)]
pub enum Outcome {
    /// Everyone agreed, and the panel has demonstrated it can disagree.
    Consensus { answer: String, n: usize },
    /// Everyone agreed but the panel has never disagreed about anything — suspect.
    UnearnedConsensus { answer: String, why: String },
    /// Split. The disagreement itself is the signal to escalate.
    Split { positions: Vec<(String, usize)> },
    /// Nothing to decide.
    Empty,
}

impl Outcome {
    /// Should this go to a larger model?
    ///
    /// Both a split and an unearned consensus escalate. The second is the interesting case: agreement
    /// from models that have never disagreed carries no information, and treating it as settled is how a
    /// panel launders a shared blind spot into a confident answer.
    pub fn needs_escalation(&self) -> bool {
        matches!(self, Outcome::Split { .. } | Outcome::UnearnedConsensus { .. })
    }
}

#[derive(Clone, Debug, Default)]
pub struct Panel {
    rounds: u32,
    disagreements: u32,
}

impl Panel {
    pub fn new() -> Self { Self::default() }

    /// Has this panel ever disagreed with itself?
    ///
    /// If not, its agreement is not evidence. Three copies of one opinion is one opinion.
    pub fn has_shown_independence(&self) -> bool { self.disagreements > 0 }

    pub fn rounds(&self) -> u32 { self.rounds }

    /// Judge one round.
    pub fn round(&mut self, ops: &[Opinion]) -> Outcome {
        if ops.is_empty() { return Outcome::Empty; }
        self.rounds += 1;
        let mut tally: Vec<(String, usize)> = Vec::new();
        for o in ops {
            let key = normalize(&o.answer);
            match tally.iter_mut().find(|(k, _)| *k == key) {
                Some((_, n)) => *n += 1,
                None => tally.push((key, 1)),
            }
        }
        if tally.len() > 1 {
            self.disagreements += 1;
            // Most-supported first, then alphabetically so the report is stable across runs.
            tally.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            return Outcome::Split { positions: tally };
        }
        let answer = tally.remove(0).0;
        if self.has_shown_independence() {
            Outcome::Consensus { answer, n: ops.len() }
        } else {
            Outcome::UnearnedConsensus { answer, why: format!(
                "all {} models agreed and this panel has never disagreed about anything. Small models \
trained on similar data share their mistakes, so this is one opinion repeated, not {} observations",
                ops.len(), ops.len()) }
        }
    }
}

/// Compare answers on content, not formatting.
///
/// Whitespace and case differences are not disagreements, and counting them as such would make every
/// panel look independent — which would then satisfy `has_shown_independence` for the wrong reason and
/// defeat the guard entirely.
fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Bytes read per token by a panel versus one large model, at Q4.
///
/// The whole case rests on this arithmetic, so it is computed rather than asserted.
pub fn bytes_per_token(panel_params_b: &[f32], single_params_b: f32) -> (f64, f64) {
    let q4 = |b: f32| (b as f64) * 1e9 * 0.5;    // ~4 bits plus overhead
    (panel_params_b.iter().map(|p| q4(*p)).sum(), q4(single_params_b))
}

/// How many rounds a panel can afford before it costs more than the single model.
pub fn round_budget(panel_params_b: &[f32], single_params_b: f32) -> f64 {
    let (p, s) = bytes_per_token(panel_params_b, single_params_b);
    if p <= 0.0 { return 0.0; }
    s / p
}

/// What each panellist is told on the FIRST round.
///
/// Deliberately does not mention the other models. A panellist that knows it is one of four will hedge
/// toward what it expects the others to say, which is the correlation this whole design is trying to
/// avoid — the independence guard measures disagreement, and priming for agreement would defeat it
/// before it could measure anything.
pub const ROUND_ONE: &str = "\
Answer the question directly and concisely. State your answer, then one sentence of reasoning. If you \
are unsure, say so rather than guessing — an admitted uncertainty is more useful than a confident wrong \
answer, because it is what tells us to ask a larger model.";

/// What panellists are told when the first round SPLIT.
///
/// Now they see each other, because the disagreement itself is the new information. Framed to invite
/// changing position on evidence rather than on the vote count — a panellist that switches because it
/// is outnumbered adds nothing except a larger majority for whatever the majority already believed.
pub fn round_two(positions: &[(String, usize)], own: &str) -> String {
    let mut s = String::from("The panel disagreed. Positions taken:\n");
    for (p, n) in positions {
        let mine = if normalize(own) == *p { "  (yours)" } else { "" };
        s.push_str(&format!("  {n}x  {p}{mine}\n"));
    }
    s.push_str("\nIf another position is better supported than yours, say so and why. If yours still \
holds, say why the others are wrong. Do NOT change your answer because it is outnumbered — a position \
that only gains support by counting heads adds nothing the first round did not already have.");
    s
}

/// Estimated bytes read for a full deliberation, against the single large model.
///
/// The thing to check before running one, since the panel's whole justification is that it reads less.
pub fn cost_estimate(panel_params_b: &[f32], single_params_b: f32, rounds: u32) -> (f64, f64, bool) {
    let (per_round, single) = bytes_per_token(panel_params_b, single_params_b);
    let total = per_round * rounds.max(1) as f64;
    (total, single, total <= single)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(m: &str, a: &str) -> Opinion { Opinion { model: m.into(), answer: a.into(), confidence: None } }

    #[test]
    fn agreement_from_a_panel_that_has_never_disagreed_is_NOT_consensus() {
        // The failure this exists to prevent. Small models trained on similar data share their
        // mistakes, so three agreeing is much weaker evidence than three independent observers
        // agreeing — and a naive vote reports that weak agreement with the confidence of a quorum.
        let mut p = Panel::new();
        let o = p.round(&[op("a", "use a HashMap"), op("b", "use a HashMap"), op("c", "use a HashMap")]);
        assert!(matches!(o, Outcome::UnearnedConsensus { .. }), "{o:?}");
        assert!(o.needs_escalation(), "and it escalates rather than being treated as settled");
        if let Outcome::UnearnedConsensus { why, .. } = &o {
            assert!(why.contains("one opinion repeated"));
        }
    }

    #[test]
    fn once_a_panel_has_disagreed_its_later_agreement_counts() {
        let mut p = Panel::new();
        let _ = p.round(&[op("a", "HashMap"), op("b", "BTreeMap")]);
        assert!(p.has_shown_independence());
        let o = p.round(&[op("a", "Vec"), op("b", "Vec")]);
        assert!(matches!(o, Outcome::Consensus { n: 2, .. }), "{o:?}");
        assert!(!o.needs_escalation());
    }

    #[test]
    fn formatting_differences_are_not_disagreements() {
        // Counting them would make every panel look independent, satisfying the guard for the wrong
        // reason and defeating it entirely.
        let mut p = Panel::new();
        let o = p.round(&[op("a", "use  a HashMap"), op("b", "Use a hashmap")]);
        assert!(matches!(o, Outcome::UnearnedConsensus { .. }), "same answer, different spelling: {o:?}");
        assert!(!p.has_shown_independence());
    }

    #[test]
    fn a_split_reports_positions_most_supported_first_and_stably() {
        let mut p = Panel::new();
        let o = p.round(&[op("a", "X"), op("b", "Y"), op("c", "Y"), op("d", "Z")]);
        match o {
            Outcome::Split { positions } => {
                assert_eq!(positions[0], ("y".to_string(), 2));
                // ties broken alphabetically so two runs produce the same report
                assert_eq!(positions[1].1, 1);
                assert!(positions[1].0 < positions[2].0);
            }
            _ => panic!("expected a split, got {o:?}"),
        }
    }

    #[test]
    fn the_bandwidth_case_is_computed_not_asserted() {
        // The whole argument rests on this: four 3B models read less per token than one 30B, so the
        // panel is cheaper per round on the axis that actually limits decode.
        let (panel, single) = bytes_per_token(&[3.0, 3.0, 3.0, 3.0], 30.0);
        assert!(panel < single, "4x3B={panel:.1e} must be under 30B={single:.1e}");
        let rounds = round_budget(&[3.0, 3.0, 3.0, 3.0], 30.0);
        assert!(rounds > 2.0 && rounds < 3.0, "about two and a half rounds, got {rounds:.2}");
    }

    #[test]
    fn a_panel_as_expensive_as_the_single_model_gets_ONE_round() {
        // Ten 3B models are a 30B's worth of reads per round, so deliberation buys nothing — the panel
        // has to be genuinely smaller to have anything to spend.
        let r = round_budget(&[3.0; 10], 30.0);
        assert!((r - 1.0).abs() < 0.01, "got {r:.2}");
    }

    #[test]
    fn round_one_does_not_tell_a_panellist_it_is_on_a_panel() {
        // A model that knows it is one of four hedges toward what it expects the others to say, which
        // is exactly the correlation the independence guard exists to detect. Priming for agreement
        // would defeat the measurement before it could take one.
        let p = ROUND_ONE.to_lowercase();
        for leak in ["panel", "other model", "others", "consensus", "vote"] {
            assert!(!p.contains(leak), "round one must not mention {leak:?}");
        }
        assert!(p.contains("unsure"), "but it must make admitting uncertainty an available answer");
    }

    #[test]
    fn round_two_shows_the_split_and_forbids_switching_on_headcount() {
        // A panellist that switches because it is outnumbered adds nothing except a larger majority for
        // what the majority already believed — which is precisely how an ensemble converges on its
        // shared error.
        let r = round_two(&[("use a btreemap".into(), 2), ("use a hashmap".into(), 1)], "use a HashMap");
        assert!(r.contains("2x  use a btreemap"));
        assert!(r.contains("(yours)"), "a panellist must be able to find its own position");
        assert!(r.contains("Do NOT change your answer because it is outnumbered"));
    }

    #[test]
    fn the_cost_estimate_says_when_deliberation_stops_being_cheaper() {
        // The panel's entire justification is that it reads less. Two rounds of 4x3B is still under one
        // 30B; three is not, and the caller should know that before running the third.
        let (t2, s, ok2) = cost_estimate(&[3.0; 4], 30.0, 2);
        assert!(ok2 && t2 < s, "two rounds: {t2:.1e} vs {s:.1e}");
        let (t3, _, ok3) = cost_estimate(&[3.0; 4], 30.0, 3);
        assert!(!ok3, "three rounds costs more than the single model: {t3:.1e}");
    }

    #[test]
    fn an_empty_round_is_not_a_consensus() {
        let mut p = Panel::new();
        assert_eq!(p.round(&[]), Outcome::Empty);
        assert_eq!(p.rounds(), 0, "and it does not count as a round");
    }
}
