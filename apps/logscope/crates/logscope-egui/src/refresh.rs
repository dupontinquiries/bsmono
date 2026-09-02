//! refresh.rs — student/teacher context compaction.
//!
//! The same model reads the conversation so far and rewrites it into a shorter briefing that preserves every
//! decision, constraint, fact, and open thread while dropping prose and dead ends. That briefing is STAGED in
//! place of the verbose span, so later turns run against a compact context. Three payoffs, in the order the
//! aims were given:
//!
//!   1. **More context.** The summary is a fraction of the folded tokens, so the window holds a much longer
//!      effective history — you can keep going past where the raw transcript would have overflowed.
//!   2. **Faster decode.** Fewer prompt tokens means a smaller prefill and less to attend over every step.
//!   3. **KV reuse for certain paths.** The compaction checkpoint is a short, STABLE prefix. Pin its KV
//!      (see `kvtree::pick_anchor_pinned` / `evict_index`) and every branch that descends from the checkpoint
//!      reuses it — one prefill of the briefing, then N decodes — instead of re-prefilling a long history.
//!
//! This module is the pure planning + prompt + staging logic (fully tested). The summary text itself is
//! produced by the model when a backend is loaded: the teacher prompt rides in a staged turn, exactly like
//! the other synthesis commands (`:strategy`, `:mergectx`).

/// Rough token estimate — the backend tokenizes exactly; this matches the heuristic used elsewhere.
pub fn est(s: &str) -> usize { s.len() / 4 + 1 }

/// A compaction plan: fold the older turns into a summary, keep the most recent `keep` verbatim.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub fold: usize,          // leading turns folded into the summary
    pub keep: usize,          // trailing turns kept verbatim
    pub folded_tokens: usize, // tokens in the folded span (before)
    pub kept_tokens: usize,   // tokens kept verbatim
    pub target_tokens: usize, // the teacher's summary budget (ratio × folded)
    pub before_tokens: usize, // folded + kept — the whole context today
    pub after_tokens: usize,  // target + kept — the projected compact context
}
impl Plan {
    pub fn saved_tokens(&self) -> usize { self.before_tokens.saturating_sub(self.after_tokens) }
    pub fn ratio_saved(&self) -> f32 {
        if self.before_tokens == 0 { 0.0 } else { self.saved_tokens() as f32 / self.before_tokens as f32 }
    }
}

/// Plan a compaction over `turns` (role, text): keep the last `keep_recent` turns verbatim, fold the rest
/// into a summary targeting `ratio` of the folded tokens. `ratio` clamps to [0.1, 0.9]; at least one turn is
/// folded whenever there is any history to compact.
pub fn plan(turns: &[(String, String)], keep_recent: usize, ratio: f32) -> Plan {
    let n = turns.len();
    let ratio = ratio.clamp(0.1, 0.9);
    let keep = keep_recent.min(n.saturating_sub(1)); // leave ≥1 turn to fold when we can
    let fold = n - keep;
    let folded_tokens: usize = turns[..fold].iter().map(|(_, t)| est(t)).sum();
    let kept_tokens: usize = turns[fold..].iter().map(|(_, t)| est(t)).sum();
    let target_tokens = ((folded_tokens as f32 * ratio).round() as usize).max(1);
    Plan {
        fold, keep, folded_tokens, kept_tokens, target_tokens,
        before_tokens: folded_tokens + kept_tokens,
        after_tokens: target_tokens + kept_tokens,
    }
}

/// Below this many folded tokens, compaction cannot pay for itself.
///
/// Folding costs a whole generation — a prompt, a decode, and the operator's
/// attention. Recovering a few hundred tokens is not worth that, and recovering
/// seven is absurd.
pub const MIN_FOLD_TOKENS: usize = 400;

impl Plan {
    /// Is this fold worth a round trip?
    ///
    /// The turn-count guard at the call sites (`turns.len() < 2`) counts the
    /// wrong thing. A two-turn conversation passes it, folds one turn, and a
    /// field report shows the result: a single seven-token user message
    /// ("add a new theme bright pink") staged for compaction with
    /// "Target: about 6 tokens" — a full generation to save one token, on a
    /// conversation with no history to speak of.
    ///
    /// What matters is the SAVING, not the number of turns. Refuse when there
    /// is too little folded to bother with, or when the plan does not actually
    /// shrink anything.
    pub fn worth_doing(&self) -> bool {
        self.fold > 0
            && self.folded_tokens >= MIN_FOLD_TOKENS
            && self.after_tokens < self.before_tokens
    }

    /// Why not, for the operator. `None` when it is worth doing.
    pub fn refusal(&self) -> Option<String> {
        if self.worth_doing() { return None; }
        if self.fold == 0 {
            return Some("compaction: nothing to fold — every turn is being kept".into());
        }
        if self.folded_tokens < MIN_FOLD_TOKENS {
            return Some(format!(
                "compaction: only {} token(s) of history to fold (floor {MIN_FOLD_TOKENS}) — \
a generation to save that much costs more than it recovers",
                self.folded_tokens));
        }
        Some(format!(
            "compaction: this plan does not shrink anything ({} → {} tokens)",
            self.before_tokens, self.after_tokens))
    }
}

/// The compaction instruction handed to the model (student == teacher) over the folded span.
pub const TEACHER_PROMPT: &str = "\
You are compacting your own prior context. Rewrite the conversation below into a single, dense briefing that \
PRESERVES every decision made, constraint agreed, fact established, and thread still open — and DROPS prose, \
restatement, pleasantries, and dead ends. Use terse, bullet-style lines. Do not add new information or \
speculation. This briefing will REPLACE the original turns as the working context, so it must stand alone.";

/// Build the staged teacher turn: the compaction prompt + a token target + the folded transcript.
pub fn teacher_turn(folded: &[(String, String)], target_tokens: usize) -> String {
    let mut body = String::from(TEACHER_PROMPT);
    body.push_str(&format!("\n\nTarget: about {target_tokens} tokens.\n\n--- CONTEXT TO COMPACT ({} turns) ---\n", folded.len()));
    for (role, text) in folded {
        let r = if role.eq_ignore_ascii_case("assistant") { "Assistant" } else { "User" };
        let t: String = text.chars().take(800).collect();
        body.push_str(&format!("\n[{r}] {}\n", t.trim()));
    }
    body
}

/// The compacted context for the next generation: the summary as a leading context turn, then the kept
/// turns verbatim. Substitute this for the original lineage when building the next prompt.
pub fn staged_context(summary: &str, kept: &[(String, String)]) -> Vec<(String, String)> {
    let mut out = Vec::with_capacity(kept.len() + 1);
    out.push(("context".to_string(), format!("[compacted briefing of earlier turns]\n{}", summary.trim())));
    out.extend(kept.iter().cloned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turns(n: usize, words: usize) -> Vec<(String, String)> {
        (0..n).map(|i| {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            (role.to_string(), "lorem ipsum ".repeat(words))
        }).collect()
    }

    #[test]
    fn plan_folds_old_keeps_recent_and_shrinks() {
        let t = turns(10, 40);
        let p = plan(&t, 4, 0.3);
        assert_eq!(p.keep, 4);
        assert_eq!(p.fold, 6);
        assert!(p.after_tokens < p.before_tokens, "compaction shrinks the context");
        assert!(p.saved_tokens() > 0 && p.ratio_saved() > 0.0);
        // target is ~30% of the folded span
        assert!((p.target_tokens as f32 - p.folded_tokens as f32 * 0.3).abs() <= 1.0);
    }

    #[test]
    fn keep_is_bounded_so_something_is_always_folded() {
        let t = turns(3, 10);
        let p = plan(&t, 10, 0.5); // ask to keep more than exist
        assert_eq!(p.keep, 2, "leaves at least one turn to fold");
        assert_eq!(p.fold, 1);
        // ratio clamps
        let p2 = plan(&t, 1, 5.0);
        assert!(p2.target_tokens <= p2.folded_tokens, "ratio clamps to ≤0.9");
    }

    #[test]
    fn teacher_turn_carries_the_prompt_and_transcript() {
        let folded = turns(2, 5);
        let body = teacher_turn(&folded, 50);
        assert!(body.starts_with(TEACHER_PROMPT));
        assert!(body.contains("Target: about 50 tokens"));
        assert!(body.contains("[User]") && body.contains("[Assistant]"));
    }

    #[test]
    fn staged_context_puts_the_briefing_first() {
        let kept = turns(2, 3);
        let staged = staged_context("• decided X\n• open: Y", &kept);
        assert_eq!(staged.len(), kept.len() + 1);
        assert_eq!(staged[0].0, "context");
        assert!(staged[0].1.contains("decided X"));
        assert_eq!(&staged[1..], &kept[..], "kept turns follow verbatim");
    }
}

/// The reply-token floor below which an answer is a fragment, mirroring `ctxwin::REPLY_FLOOR`.
///
/// Duplicated rather than imported because the engine crate is behind the `wgpu-engine` feature
/// and this planner is always built — importing it un-gated is exactly the break that had
/// `cargo check` failing on the default build once already. The two are pinned equal by a test
/// under the feature, same belt-and-suspenders as `mobilelayout::FAN_ROW_PT` vs `holdmenu::ROW_PT`:
/// a silent divergence would make this planner aim at a floor the engine does not use.
pub const REPLY_FLOOR: usize = 512;

/// Turn the engine's TOKEN-shaped compaction request into this planner's TURN-shaped inputs.
///
/// `ctxwin::relief` decides in tokens, because that is what a context window is measured in; this
/// planner folds whole TURNS, because half a turn is not a thing to summarize. The translation is
/// its own function rather than an inline division at the call site for the reason every seam in
/// this file exists: the two units met, and whichever side did the conversion silently would own a
/// rule the other side could not see.
///
/// `keep_recent_tokens` is the engine's floor on what must survive verbatim. Turns are counted
/// from the newest backward until adding one more would exceed it, and AT LEAST ONE recent turn is
/// always kept — a compaction that folds the turn the next reply continues has summarized away the
/// thread it was supposed to pick up, which is the failure this whole path exists to avoid.
///
/// The ratio is chosen so the projected context actually clears `want_room`: a fold that
/// recommends itself and still leaves the reply squeezed has spent a round trip for nothing (the
/// same mistake `ctxwin::relief` prices the summary against, one layer up).
pub fn plan_for_request(turns: &[(String, String)], keep_recent_tokens: usize, window: usize,
                        want_room: usize) -> (usize, f32) {
    let n = turns.len();
    if n == 0 { return (0, 0.3); }
    let mut keep = 0usize;
    let mut kept_tokens = 0usize;
    for (_, t) in turns.iter().rev() {
        let e = est(t);
        if keep > 0 && kept_tokens + e > keep_recent_tokens { break; }
        kept_tokens += e;
        keep += 1;
        if keep >= n.saturating_sub(1) { break; }   // always leave something to fold
    }
    let folded: usize = turns[..n - keep].iter().map(|(_, t)| est(t)).sum();
    // Budget left for the summary after the kept tail and the reply headroom are paid for.
    let budget = window.saturating_sub(kept_tokens + want_room);
    let ratio = if folded == 0 { 0.3 } else { (budget as f32 / folded as f32).clamp(0.1, 0.9) };
    (keep, ratio)
}

#[cfg(test)]
mod request_tests {
    use super::*;

    fn turns(sizes: &[usize]) -> Vec<(String, String)> {
        // est() is ~chars/4, so a turn of n tokens is ~4n characters
        sizes.iter().enumerate()
            .map(|(i, n)| ((if i % 2 == 0 { "user" } else { "assistant" }).to_string(),
                           "x".repeat(n * 4)))
            .collect()
    }

    #[test]
    fn the_newest_turn_always_survives_verbatim() {
        // Even when the engine's keep budget is smaller than the last turn alone: folding the turn
        // the next reply continues is the failure the whole path exists to avoid.
        let t = turns(&[100, 100, 900]);
        let (keep, _) = plan_for_request(&t, 50, 4096, 1024);
        assert!(keep >= 1, "kept {keep}");
        let p = plan(&t, keep, 0.3);
        assert!(p.fold >= 1, "and there is still something to fold");
    }

    #[test]
    fn turns_are_counted_backward_until_the_token_floor_binds() {
        let t = turns(&[500, 500, 300, 200, 100]);
        // 1433 tokens of keep budget: 100 + 200 + 300 = 600, adding 500 would make 1100 (still
        // under), adding the next 500 makes 1600 — over, so 4 turns are kept.
        let (keep, _) = plan_for_request(&t, 1433, 4096, 1024);
        assert_eq!(keep, 4);
        // and a tight budget keeps fewer: the last turn (100) is kept unconditionally, then +200
        // would exceed 250, so the count stops at one
        let (keep_tight, _) = plan_for_request(&t, 250, 4096, 1024);
        assert_eq!(keep_tight, 1);
    }

    #[test]
    fn the_ratio_is_chosen_so_the_fold_actually_clears_the_reply_floor() {
        let t = turns(&[1500, 1000, 400, 200]);
        let (keep, ratio) = plan_for_request(&t, 800, 4096, 1024);
        let p = plan(&t, keep, ratio);
        assert!(p.after_tokens + 1024 <= 4096 + 1,
                "projected {} + reply room must fit the window", p.after_tokens);
        assert!(ratio >= 0.1 && ratio <= 0.9);
    }

    #[test]
    fn a_single_turn_has_nothing_to_fold_and_says_so_by_arithmetic() {
        let t = turns(&[500]);
        let (keep, _) = plan_for_request(&t, 4000, 4096, 1024);
        let p = plan(&t, keep, 0.3);
        assert_eq!(p.keep, 0, "plan() itself leaves a turn to fold rather than folding nothing");
    }

    #[cfg(feature = "wgpu-engine")]
    #[test]
    fn the_reply_floor_matches_the_engine_it_is_planning_for() {
        assert_eq!(REPLY_FLOOR, gpu_kernels::ctxwin::REPLY_FLOOR);
    }

    #[test]
    fn an_empty_history_does_not_divide_by_zero() {
        let (keep, ratio) = plan_for_request(&[], 1000, 4096, 512);
        assert_eq!(keep, 0);
        assert!(ratio > 0.0);
    }
}

#[cfg(test)]
mod worth_doing_tests {
    use super::*;

    #[test]
    fn the_field_case_is_refused() {
        // "add a new theme bright pink" alone, plus one reply. The turn-count
        // guard let this through and staged "Target: about 6 tokens".
        let turns = vec![
            ("user".to_string(), "add a new theme bright pink".to_string()),
            ("assistant".to_string(), "Sure.".to_string()),
        ];
        let p = plan(&turns, 4, 0.3);
        assert!(!p.worth_doing(), "{p:?}");
        let why = p.refusal().expect("a refusal must explain itself");
        assert!(why.contains("token(s) of history"), "{why}");
    }

    #[test]
    fn a_real_conversation_is_still_compacted() {
        // The guard must not become a blanket refusal — that would trade a
        // silly compaction for no compaction at all.
        let turns: Vec<(String, String)> = (0..12)
            .map(|i| ((if i % 2 == 0 { "user" } else { "assistant" }).to_string(),
                      "word ".repeat(200)))
            .collect();
        let p = plan(&turns, 4, 0.3);
        assert!(p.worth_doing(), "{p:?}");
        assert!(p.refusal().is_none());
        assert!(p.after_tokens < p.before_tokens);
    }

    #[test]
    fn a_plan_that_does_not_shrink_is_refused_even_when_large() {
        // ratio clamps at 0.9, so a fold can approach break-even. If it ever
        // reaches it, spending a generation is pure loss.
        let turns: Vec<(String, String)> = (0..6)
            .map(|_| ("user".to_string(), "word ".repeat(300)))
            .collect();
        let p = plan(&turns, 4, 0.9);
        // large enough to clear the floor, so only the shrink test can refuse it
        assert!(p.folded_tokens >= MIN_FOLD_TOKENS);
        assert_eq!(p.worth_doing(), p.after_tokens < p.before_tokens);
    }

    #[test]
    fn every_refusal_names_a_number_the_operator_can_act_on() {
        let turns = vec![
            ("user".to_string(), "hi".to_string()),
            ("assistant".to_string(), "hello".to_string()),
        ];
        let why = plan(&turns, 4, 0.3).refusal().unwrap();
        assert!(why.chars().any(|c| c.is_ascii_digit()),
            "a refusal with no number tells the operator nothing: {why}");
    }
}
