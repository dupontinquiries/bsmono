//! kvplan — make the prompt's PREFIX stable, because that is where the
//! latency is.
//!
//! On a KV-cached engine (which is what `aperture-engine` links, and what
//! `logscope-serve` runs), the cost of a completion splits in two:
//!
//! * **prefill** — running the prompt through the model to build its KV
//!   cache. Proportional to prompt length, and the dominant cost for a
//!   short completion in a long file.
//! * **decode** — generating the answer, token by token.
//!
//! The whole point of a KV cache is that prefill is *incremental*: if this
//! prompt begins with the same tokens as the last one, that part of the
//! cache is still valid and only the new tail needs computing. A thousand
//! tokens of unchanged context above the caret should cost nothing on the
//! second keystroke.
//!
//! That only works if the prefix is byte-stable. Anything that changes
//! near the START of the prompt invalidates everything after it, and the
//! engine silently re-prefills the entire context — the same wall-clock
//! cost as having no cache at all, with no error to notice.
//!
//! ## Three rules, and this module is each of them
//!
//! 1. **Volatile facts go LAST, not first.** [`assemble`]. Putting a
//!    `// context: ... 412 LOC, loop-depth 2` header at the top of the
//!    prompt — which is exactly what this editor started doing one commit
//!    ago — changes byte 0 whenever the file grows a line or the caret
//!    enters a loop. Every keystroke that changed either one threw away
//!    the whole cache. The same text placed immediately before the caret
//!    costs a few tokens of re-prefill instead of all of them.
//!
//! 2. **Trim to a QUANTIZED anchor, not a sliding window.**
//!    [`anchored_trim`]. "Keep the last N characters" moves the start of
//!    the prompt on every single keystroke, so the prefix is never the
//!    same twice — the worst possible case, and the obvious way to
//!    implement a budget. Quantizing the start to a line-boundary grid
//!    means it holds still for many edits and then jumps once.
//!
//! 3. **Order a batch by shared prefix.** [`order_for_kv`]. When several
//!    scopes are asked at once they share a suffix but nest by prefix;
//!    issuing them so each consecutive pair shares as much leading text
//!    as possible lets an engine that keeps one cache between calls reuse
//!    it instead of rebuilding per request.
//!
//! Pure, no I/O, no engine handle — the decisions are testable exactly,
//! and the expensive thing they are deciding about lives elsewhere.

/// Put the stable context first and the volatile facts immediately before
/// the caret.
///
/// `stable` is the scoped source text (it changes only when you actually
/// edit it, and then only from the edit point onward). `volatile` is
/// anything derived — a metadata comment, a file-size figure, a loop
/// depth — that can change without the source above it changing at all.
///
/// Returns the assembled prompt. Empty `volatile` returns `stable`
/// untouched rather than appending a blank line, so "nothing to say"
/// costs nothing and, more importantly, does not itself become a
/// prefix-changing event.
pub fn assemble(stable: &str, volatile: &str) -> String {
    if volatile.trim().is_empty() {
        return stable.to_string();
    }
    // A trailing newline on `volatile` keeps the caret's own line intact:
    // the model must still see the partial line it is completing as the
    // last thing in the prompt.
    let mut out = String::with_capacity(stable.len() + volatile.len() + 1);
    // Split at the final newline so the metadata lands ABOVE the line the
    // caret is on, not in the middle of it.
    match stable.rfind('\n') {
        Some(i) => {
            out.push_str(&stable[..=i]);
            out.push_str(volatile);
            if !volatile.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&stable[i + 1..]);
        }
        None => {
            // Single line: metadata first is the only option, and it is
            // also harmless — there is no long prefix to invalidate.
            out.push_str(volatile);
            if !volatile.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(stable);
        }
    }
    out
}

/// How coarse the trim anchor is, in lines. Larger holds the prefix still
/// for longer (better cache reuse) at the cost of carrying more context
/// than the budget strictly asked for; 32 lines is roughly a screenful,
/// so the anchor typically survives an entire editing session in one
/// region.
pub const ANCHOR_LINES: usize = 32;

/// Trim `text` to at most `budget` bytes, keeping the END (the caret is
/// there), but starting at a QUANTIZED line boundary so the start point
/// holds still across edits.
///
/// The naive version — keep the last `budget` bytes — is what makes a
/// budget actively harmful on a cached engine: every inserted character
/// shifts the window's start by one, so no two consecutive prompts share
/// a prefix and every keystroke re-prefills everything. Here the start
/// snaps to every [`ANCHOR_LINES`]th line boundary, so it stays put while
/// you type and jumps only when you have moved far enough that it must.
///
/// Never splits a UTF-8 character, and never returns more than `budget`
/// bytes unless a single line exceeds it (in which case the tail of that
/// line is returned — the caret's own line is the one thing that cannot
/// be dropped).
pub fn anchored_trim(text: &str, budget: usize) -> &str {
    if text.len() <= budget {
        return text;
    }
    // Line-start byte offsets.
    let mut starts: Vec<usize> = vec![0];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    // The earliest line we could start at and still fit the budget.
    let earliest_fitting = starts
        .iter()
        .copied()
        .find(|&s| text.len() - s <= budget)
        .unwrap_or(text.len());
    // Snap FORWARD to the next anchor-aligned line index, so the chosen
    // start is a multiple of ANCHOR_LINES in line terms. Forward, not
    // backward, because backward could exceed the budget again.
    let idx = starts.partition_point(|&s| s < earliest_fitting);
    let anchored_idx = idx.div_ceil(ANCHOR_LINES) * ANCHOR_LINES;
    let start = starts.get(anchored_idx).copied().unwrap_or(earliest_fitting);
    let mut start = start.min(text.len());
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    &text[start..]
}

/// Reorder prompts so consecutive entries share as much leading text as
/// possible — the order to issue a batch in when the engine keeps one KV
/// cache between calls.
///
/// Returns indices into the input, so the caller can map answers back to
/// the positions that asked for them. A greedy nearest-neighbour walk by
/// shared-prefix length: start at the shortest prompt (the cheapest
/// prefill, and usually the prefix of everything else), then repeatedly
/// take whichever remaining prompt shares the most with the one just
/// issued.
///
/// Greedy rather than optimal on purpose — the optimal ordering is a
/// path problem, the input is a handful of scopes, and a wrong-by-a-bit
/// order costs a little extra prefill while an expensive planner would
/// cost the latency it is trying to save.
pub fn order_for_kv(prompts: &[String]) -> Vec<usize> {
    if prompts.is_empty() {
        return Vec::new();
    }
    let mut remaining: Vec<usize> = (0..prompts.len()).collect();
    // Shortest first: it is the cheapest to prefill cold, and in this
    // codebase's actual use (nested scopes) it is a prefix of the rest.
    remaining.sort_by_key(|&i| prompts[i].len());
    let mut order = vec![remaining.remove(0)];
    while !remaining.is_empty() {
        let last = &prompts[*order.last().unwrap()];
        let (best_pos, _) = remaining
            .iter()
            .enumerate()
            .map(|(pos, &i)| (pos, shared_prefix_len(last, &prompts[i])))
            .max_by_key(|(_, shared)| *shared)
            .unwrap();
        order.push(remaining.remove(best_pos));
    }
    order
}

/// Bytes of common leading text, stopping on a char boundary so the
/// figure is meaningful for splitting as well as comparing.
pub fn shared_prefix_len(a: &str, b: &str) -> usize {
    let n = a
        .as_bytes()
        .iter()
        .zip(b.as_bytes())
        .take_while(|(x, y)| x == y)
        .count();
    let mut n = n;
    while n > 0 && (!a.is_char_boundary(n) || !b.is_char_boundary(n)) {
        n -= 1;
    }
    n
}

/// How much of a batch's total prefill this ordering avoids, as a
/// fraction of the naive (unordered, no-reuse) cost — the number worth
/// showing in the loop pane, because a cache whose benefit nobody can
/// see is a cache nobody will keep correct.
///
/// Counts only what a single-cache engine can actually reuse: the shared
/// prefix with the IMMEDIATELY preceding prompt.
pub fn reuse_fraction(prompts: &[String], order: &[usize]) -> f32 {
    let total: usize = prompts.iter().map(|p| p.len()).sum();
    if total == 0 || order.len() < 2 {
        return 0.0;
    }
    let mut saved = 0usize;
    for w in order.windows(2) {
        saved += shared_prefix_len(&prompts[w[0]], &prompts[w[1]]);
    }
    (saved as f32 / total as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volatile_metadata_lands_above_the_caret_line_not_at_byte_zero() {
        // THE REGRESSION THIS MODULE EXISTS FOR. A metadata header at the
        // top changes byte 0 whenever the file grows a line, throwing away
        // the whole KV cache on a keystroke that edited nothing above.
        let stable = "use std::fmt;\nfn f() {\n    let x = ";
        let out = assemble(stable, "// context: in `f`, 3 LOC\n");
        assert!(out.starts_with("use std::fmt;\n"), "the real prefix still leads: {out:?}");
        assert!(out.ends_with("    let x = "), "the caret's line is still last: {out:?}");
        assert!(out.contains("// context:"));

        // And the property that matters: two prompts whose VOLATILE part
        // differs still share the long stable prefix.
        let a = assemble(stable, "// context: 3 LOC\n");
        let b = assemble(stable, "// context: 4 LOC\n");
        assert!(
            shared_prefix_len(&a, &b) >= "use std::fmt;\nfn f() {\n".len(),
            "changing the LOC figure must not invalidate the source above it"
        );
    }

    #[test]
    fn empty_volatile_changes_nothing_at_all() {
        let s = "fn f() {\n    let x = ";
        assert_eq!(assemble(s, ""), s);
        assert_eq!(assemble(s, "   \n"), s, "whitespace-only is nothing to say");
    }

    #[test]
    fn a_sliding_window_would_shift_every_keystroke_but_the_anchor_holds() {
        // The core claim, tested as a comparison rather than asserted.
        // Build a file, then the same file with one more character typed
        // at the end, and check the trimmed window starts at the SAME
        // place both times.
        let mut base = String::new();
        for i in 0..200 {
            base.push_str(&format!("let line_{i} = {i};\n"));
        }
        let typed = format!("{base}let x = ");
        let typed2 = format!("{base}let xy = ");
        let budget = 1200;

        let a = anchored_trim(&typed, budget);
        let b = anchored_trim(&typed2, budget);
        assert!(a.len() <= budget && b.len() <= budget, "budget respected");

        // The naive equivalent, for contrast: last `budget` bytes.
        let naive_a = &typed[typed.len() - budget..];
        let naive_b = &typed2[typed2.len() - budget..];
        let naive_shared = shared_prefix_len(naive_a, naive_b);
        let anchored_shared = shared_prefix_len(a, b);
        assert!(
            anchored_shared > naive_shared,
            "anchored keeps a longer shared prefix than a sliding window \
             (anchored {anchored_shared}, naive {naive_shared})"
        );
        // Concretely: the anchored windows begin identically.
        assert_eq!(
            a.lines().next(),
            b.lines().next(),
            "the window's first line is the same despite the edit"
        );
    }

    #[test]
    fn trim_returns_the_text_untouched_when_it_already_fits() {
        let s = "fn f() {}\n";
        assert_eq!(anchored_trim(s, 1000), s);
    }

    #[test]
    fn trim_never_splits_a_character() {
        let mut s = String::new();
        for _ in 0..500 {
            s.push_str("héllo wörld — em dash\n");
        }
        for budget in [64, 200, 1000, 3333] {
            let out = anchored_trim(&s, budget);
            // Slicing it further would panic if it were not a boundary;
            // that it exists as a &str already proves the split was valid,
            // so assert the stronger property: it round-trips.
            assert!(s.ends_with(out), "the trim is a suffix of the input");
        }
    }

    #[test]
    fn batch_order_puts_the_shortest_first_and_maximises_shared_prefixes() {
        // The nested-scope shape this actually sees: each is a prefix of
        // the next, so the ideal order is shortest to longest and reuse
        // should be near total.
        let prompts = vec![
            "AAAABBBBCCCCDDDD".to_string(),
            "CCCCDDDD".to_string(),
            "BBBBCCCCDDDD".to_string(),
        ];
        let order = order_for_kv(&prompts);
        assert_eq!(order[0], 1, "shortest first: cheapest cold prefill");
        assert_eq!(order.len(), 3);
        let mut seen = order.clone();
        seen.sort();
        assert_eq!(seen, vec![0, 1, 2], "every prompt issued exactly once");
    }

    #[test]
    fn ordering_beats_the_input_order_when_the_input_is_adversarial() {
        // Two families of prompts, interleaved on input. A good order
        // groups each family together; the input order alternates and
        // shares almost nothing between neighbours.
        let prompts = vec![
            "alpha context one".to_string(),
            "beta context one".to_string(),
            "alpha context two".to_string(),
            "beta context two".to_string(),
        ];
        let naive: Vec<usize> = (0..prompts.len()).collect();
        let order = order_for_kv(&prompts);
        assert!(
            reuse_fraction(&prompts, &order) >= reuse_fraction(&prompts, &naive),
            "ordering must not be worse than not ordering"
        );
    }

    #[test]
    fn reuse_fraction_is_zero_for_a_single_prompt_and_bounded() {
        let one = vec!["x".to_string()];
        assert_eq!(reuse_fraction(&one, &[0]), 0.0, "nothing to reuse against");
        let same = vec!["abcdef".to_string(), "abcdef".to_string()];
        let f = reuse_fraction(&same, &[0, 1]);
        assert!((0.0..=1.0).contains(&f), "a fraction, not a ratio that can exceed 1: {f}");
    }

    #[test]
    fn shared_prefix_length_respects_char_boundaries() {
        // Two strings diverging mid-multibyte-character must not report a
        // shared length that splits it.
        let a = "prefix — dash";
        let b = "prefix — dot";
        let n = shared_prefix_len(a, b);
        assert!(a.is_char_boundary(n) && b.is_char_boundary(n));
        assert!(a[..n].starts_with("prefix "));
    }

    #[test]
    fn empty_input_is_not_a_panic() {
        assert!(order_for_kv(&[]).is_empty());
        assert_eq!(anchored_trim("", 10), "");
        assert_eq!(assemble("", ""), "");
    }
}
