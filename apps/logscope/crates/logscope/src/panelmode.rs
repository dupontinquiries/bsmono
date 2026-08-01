//! panelmode.rs — when a debating panel should replace the thinker, and when it should not.
//!
//! `deliberate` runs a panel and judges its output. What it does not decide is whether a panel is the
//! right shape for *this question* — and the answer is not always yes, which is the part that makes
//! this a mode rather than a replacement.
//!
//! ## The bandwidth case, restated
//!
//! Four 3B models read ~6.7 GiB per token against a 30B's ~17, so a panel gets roughly two and a half
//! rounds before it costs what the single model costs. That budget is the mode's entire justification.
//!
//! ## Where a panel genuinely beats one larger model
//!
//! **Questions with several defensible answers.** Choosing a data structure, naming a trade-off,
//! deciding whether an approach is worth it — the disagreement *is* the information, and a single
//! model gives one answer with no indication of how close the call was.
//!
//! ## Where it is strictly worse, and this is the important half
//!
//! **Questions with one right answer that requires knowledge.** Four small models that do not know an
//! API will produce four wrong answers and agree on some of them, and `deliberate`'s independence
//! guard cannot save that: they will have disagreed elsewhere, so the agreement looks earned. A panel
//! converts "I do not know" into "we concur", which is the worst possible transformation.
//!
//! **Long generation.** Panel cost scales with output length times members. A 400-token function from
//! four models is four times the tokens for one answer, and the comparison is worthless because they
//! will differ in formatting everywhere and in substance nowhere.

/// What kind of question this is, as far as routing can tell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// Several defensible answers; the disagreement is the information.
    Judgement,
    /// One right answer, and getting it needs knowledge the models may lack.
    Factual,
    /// A long artifact — a function, a file, a document.
    Generation,
    /// Short and mechanical.
    Mechanical,
}

/// What to run.
#[derive(Clone, PartialEq, Debug)]
pub enum Route {
    /// A debating panel, with a round budget.
    Panel { rounds: u32 },
    /// One large model.
    Thinker { why: String },
}

impl Route {
    pub fn is_panel(&self) -> bool { matches!(self, Route::Panel { .. }) }
}

/// Choose between a panel and a thinker.
///
/// `est_output_tokens` matters as much as the shape: panel cost scales with output length times
/// members, so a long artifact is expensive in a way a short judgement is not.
pub fn route(shape: Shape, est_output_tokens: usize, rounds_affordable: f32) -> Route {
    if rounds_affordable < 1.0 {
        return Route::Thinker { why: format!(
            "the panel costs more per round than the single model ({rounds_affordable:.1} rounds \
affordable), so deliberation has nothing to spend") };
    }
    match shape {
        Shape::Factual => Route::Thinker { why:
            "one right answer that needs knowledge the small models may not have. Four of them would \
produce four wrong answers and agree on some — and the independence guard cannot catch that, because \
they will have disagreed elsewhere. A panel turns 'I do not know' into 'we concur'".into() },
        Shape::Generation if est_output_tokens > 200 => Route::Thinker { why: format!(
            "{est_output_tokens} tokens of output times the panel is a lot of generation for one \
artifact, and the branches will differ in formatting everywhere and in substance nowhere") },
        Shape::Mechanical => Route::Thinker { why:
            "the answer is determined by the input, so there is nothing to disagree about".into() },
        _ => Route::Panel { rounds: rounds_affordable.floor().max(1.0) as u32 },
    }
}

/// The liaison's job when a panel replaces the thinker.
///
/// It does NOT become a panellist. Its job is unchanged — condense, relay, frame — and adding it to
/// the panel would correlate the panel with the thing summarizing it, which is the one arrangement
/// that makes the summary untrustworthy.
pub const LIAISON_ROLE_UNDER_PANEL: &str = "\
You are summarizing a panel you are not part of. Report what they disagreed about before what they \
agreed on: the disagreement is why the panel was run, and a summary that leads with the consensus \
discards the reason it exists. If they never disagreed, say so plainly — it means the answer is one \
opinion repeated, not a corroborated one.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_factual_question_goes_to_the_THINKER_however_cheap_the_panel() {
        // The important half. Four small models that do not know an API produce four wrong answers and
        // agree on some — and deliberate's independence guard cannot save it, because they will have
        // disagreed elsewhere so the agreement looks earned.
        let r = route(Shape::Factual, 50, 5.0);
        assert!(!r.is_panel());
        if let Route::Thinker { why } = r {
            assert!(why.contains("'I do not know' into 'we concur'"));
        }
    }

    #[test]
    fn a_judgement_question_is_what_a_panel_is_FOR() {
        // Several defensible answers, and the disagreement is the information a single model cannot
        // give — it answers with no indication of how close the call was.
        assert!(route(Shape::Judgement, 80, 2.5).is_panel());
    }

    #[test]
    fn a_long_artifact_goes_to_one_model() {
        // Panel cost scales with output length times members, and the branches differ in formatting
        // everywhere and in substance nowhere — so the comparison is worthless AND expensive.
        assert!(!route(Shape::Generation, 400, 3.0).is_panel());
        assert!(route(Shape::Generation, 60, 3.0).is_panel(), "a short one is still worth debating");
    }

    #[test]
    fn an_unaffordable_panel_is_refused_before_the_shape_is_considered() {
        // If the panel costs more per round than the single model, deliberation has nothing to spend
        // and the shape of the question is irrelevant.
        let r = route(Shape::Judgement, 50, 0.8);
        assert!(!r.is_panel());
        if let Route::Thinker { why } = r { assert!(why.contains("nothing to spend")); }
    }

    #[test]
    fn a_mechanical_question_has_nothing_to_debate() {
        assert!(!route(Shape::Mechanical, 20, 4.0).is_panel());
    }

    #[test]
    fn the_round_budget_is_carried_into_the_panel() {
        // deliberate needs it, and a panel that outruns the budget has spent the argument for its own
        // existence.
        match route(Shape::Judgement, 50, 2.7) {
            Route::Panel { rounds } => assert_eq!(rounds, 2),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn the_liaison_does_not_join_the_panel_it_summarizes() {
        // Adding it would correlate the panel with the thing summarizing it — the one arrangement that
        // makes the summary untrustworthy.
        assert!(LIAISON_ROLE_UNDER_PANEL.contains("a panel you are not part of"));
        assert!(LIAISON_ROLE_UNDER_PANEL.contains("disagreed about before what they agreed on"));
        assert!(LIAISON_ROLE_UNDER_PANEL.contains("one opinion repeated"));
    }
}
