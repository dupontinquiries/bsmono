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
