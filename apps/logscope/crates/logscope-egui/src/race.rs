//! race.rs — race a COMPACTED context against the FULL context, surface each result as a selectable option
//! as it finishes, and — if the compaction turns out to be a "bad jump" — spawn a SMALLER model on a third
//! run to diagnose why.
//!
//! Why: compaction (`refresh`) is usually a win — shorter context, faster decode, a pinnable KV checkpoint —
//! but it can drop a detail the next answer needed. So at the compaction point you don't commit blind: run
//! BOTH the compacted and the full context off the same parent. The full run is the safety net; the
//! compacted run is the fast path. As each finishes it becomes a notifier card (`cards::CardQueue`) the user
//! can accept; keep whichever is better. If the user flags the compacted path as a regression, the
//! diagnostic run reads the full context, the briefing, and the two answers and reports exactly what the
//! briefing dropped — cheap insight from a tiny model that can sharpen the next compaction.
//!
//! This module is the pure planning + measurement + prompt logic (tested). The parallel execution, the
//! card-on-finish notifier, and the model runs are the app/runner's job (they ride the existing fan queue +
//! model pool); this gives them the specs and the prompts.

use crate::diff;

/// One planned runner: a labeled context to generate against, with its token cost.
#[derive(Clone, Debug, PartialEq)]
pub struct RunSpec {
    pub label: String,
    pub context: Vec<(String, String)>,
    pub est_tokens: usize,
}

fn est_ctx(ctx: &[(String, String)]) -> usize { ctx.iter().map(|(_, t)| crate::refresh::est(t)).sum() }

/// Build the two competing runs at a compaction point: the FULL context (baseline) and the COMPACTED context
/// (briefing + kept turns). `summary` is the teacher's briefing that replaces the folded head.
pub fn dual_run(full: &[(String, String)], summary: &str, kept: &[(String, String)]) -> [RunSpec; 2] {
    let compact = crate::refresh::staged_context(summary, kept);
    [
        RunSpec { label: "full".into(), est_tokens: est_ctx(full), context: full.to_vec() },
        RunSpec { label: "compacted".into(), est_tokens: est_ctx(&compact), context: compact },
    ]
}

/// How far the two outputs diverged, 0..1 (word-level). A small jump → the compaction was safe; a large one
/// → the briefing changed the answer, worth a look (or a bad-jump diagnosis).
pub fn divergence(full_out: &str, compact_out: &str) -> f64 {
    diff::change_ratio(&diff::word_diff(full_out, compact_out))
}

/// Content for the notifier card surfaced when a run finishes: `(key, title, body, action)` to hand to
/// `cards::CardQueue::suggest`. `peer_divergence` (if the sibling has already finished) colours the hint.
pub fn result_card(label: &str, output: &str, peer_divergence: Option<f64>) -> (String, String, String, String) {
    let preview: String = output.chars().take(140).collect();
    let hint = match peer_divergence {
        Some(d) if d >= 0.35 => format!(" · diverges {:.0}% from the other run", d * 100.0),
        Some(d) => format!(" · ~{:.0}% match with the other run", (1.0 - d) * 100.0),
        None => String::new(),
    };
    (
        format!("race-result:{label}"),
        format!("{} run finished{hint}", cap(label)),
        preview,
        format!("Keep the {label} run"),
    )
}

/// The diagnostic prompt for the SMALLER model after a path is flagged a bad jump: it sees the full context,
/// the briefing that replaced it, and both answers, and must name what the briefing dropped or distorted that
/// caused the divergence — so the next compaction can keep it. Empty sections are omitted.
pub const DIAGNOSE_PROMPT: &str = "\
A long context was compacted into the briefing below, and generating from the briefing produced a DIFFERENT \
answer than generating from the full context. You are the reviewer. Compare the full context with the \
briefing and the two answers, and state CONCISELY what the briefing dropped or distorted that caused the \
divergence — the specific facts, constraints, or threads the compaction should have kept. Do not rewrite the \
answer; just diagnose the bad jump.";

/// Build the staged diagnostic turn. `full_ctx` and `summary` may be empty (then those sections are omitted)
/// — the minimum useful diagnosis is the two diverging answers plus their word-level diff.
pub fn diagnose_turn(full_ctx: &[(String, String)], summary: &str, full_out: &str, compact_out: &str) -> String {
    let mut body = String::from(DIAGNOSE_PROMPT);
    if !full_ctx.is_empty() {
        body.push_str("\n\n--- FULL CONTEXT ---\n");
        for (role, text) in full_ctx {
            let r = if role.eq_ignore_ascii_case("assistant") { "Assistant" } else { "User" };
            let t: String = text.chars().take(500).collect();
            body.push_str(&format!("\n[{r}] {}\n", t.trim()));
        }
    }
    if !summary.trim().is_empty() {
        body.push_str(&format!("\n\n--- BRIEFING THAT REPLACED IT ---\n{}\n", summary.trim()));
    }
    let fo: String = full_out.chars().take(800).collect();
    let co: String = compact_out.chars().take(800).collect();
    body.push_str(&format!("\n--- ANSWER FROM FULL CONTEXT ---\n{}\n\n--- ANSWER FROM BRIEFING ---\n{}\n", fo.trim(), co.trim()));
    let redline: String = diff::redline(&diff::word_diff(full_out, compact_out)).chars().take(900).collect();
    body.push_str(&format!("\n--- WORD-LEVEL DIFF (full → briefing) ---\n{}\n", redline));
    body
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() { Some(f) => f.to_uppercase().collect::<String>() + c.as_str(), None => String::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turns(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(r, t)| (r.to_string(), t.to_string())).collect()
    }

    #[test]
    fn dual_run_makes_full_and_compacted_and_compacted_is_smaller() {
        let full = turns(&[
            ("user", "here is a long question with lots of detail ".repeat(20).as_str()),
            ("assistant", "a long detailed answer ".repeat(20).as_str()),
            ("user", "short follow-up"),
        ]);
        let kept = vec![full[2].clone()];
        let [a, b] = dual_run(&full, "• terse briefing of the first two turns", &kept);
        assert_eq!(a.label, "full");
        assert_eq!(b.label, "compacted");
        assert!(b.est_tokens < a.est_tokens, "compacted context is cheaper than full");
        assert!(b.context.first().map(|(r, _)| r == "context").unwrap_or(false), "briefing leads the compacted run");
    }

    #[test]
    fn divergence_is_zero_when_identical_and_high_when_not() {
        assert_eq!(divergence("the same exact answer", "the same exact answer"), 0.0);
        assert!(divergence("use a binary search here", "delete the whole module instead") > 0.5);
    }

    #[test]
    fn diagnose_turn_includes_answers_and_diff_and_omits_empty_sections() {
        let body = diagnose_turn(&[], "", "answer A keeps the retry limit", "answer B forgot the retry limit");
        assert!(body.starts_with(DIAGNOSE_PROMPT));
        assert!(body.contains("ANSWER FROM FULL CONTEXT") && body.contains("ANSWER FROM BRIEFING"));
        assert!(body.contains("WORD-LEVEL DIFF"));
        assert!(!body.contains("FULL CONTEXT ---\n\n["), "no empty context block");
        assert!(!body.contains("BRIEFING THAT REPLACED IT"), "empty briefing omitted");
        // with context + briefing the sections appear
        let rich = diagnose_turn(&turns(&[("user", "q")]), "• briefing", "x", "y");
        assert!(rich.contains("FULL CONTEXT") && rich.contains("BRIEFING THAT REPLACED IT"));
    }

    #[test]
    fn result_card_labels_and_hints() {
        let (key, title, body, action) = result_card("compacted", "here is the compacted run's answer text", Some(0.5));
        assert_eq!(key, "race-result:compacted");
        assert!(title.starts_with("Compacted run finished") && title.contains("diverges 50%"));
        assert!(action.contains("compacted"));
        assert!(body.len() <= 140);
        // matching peer reads as agreement
        let (_, t2, _, _) = result_card("full", "ans", Some(0.1));
        assert!(t2.contains("match"));
    }
}
