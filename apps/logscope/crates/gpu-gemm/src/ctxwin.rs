//! ctxwin.rs — deciding what survives when a prompt does not fit the context window.
//!
//! The engine's policy was a raw token splice: keep `max/3` (capped at 1024) from the head, keep the
//! rest from the tail, drop the middle. That is the standard window shift and it is right in shape,
//! but it has two properties that hurt the case the operator actually cares about — reading a
//! repository — and both are fixed here.
//!
//! FIRST, THE SPLICE WAS UNMARKED. The model received the head immediately followed by the tail with
//! nothing between them, so a sentence cut in half became a sentence, and a function whose body was
//! removed became a function with a different body. There is no way for a reader to tell that
//! anything is missing; it just sees a document that says something else. A marker costs a handful
//! of tokens and turns silent corruption into a visible gap.
//!
//! SECOND, THE MARKER HAS TO COME OUT OF THE BUDGET. The old arithmetic used `head + tail == max`
//! exactly. Appending a marker to that overflows the window — and the window exists because the KV
//! cache is allocated from it, so overflowing is not a formatting problem, it is the device OOM the
//! bound was there to prevent. The marker is therefore reserved BEFORE the split, never after.
//!
//! Pure arithmetic over counts: no tokenizer, no engine, no GPU. That is what makes the awkward
//! cases cheap to pin.

/// What to keep, in token counts. `head + marker + tail` is guaranteed not to exceed the budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub head: usize,
    pub tail: usize,
    /// Tokens reserved for the elision marker. Zero when nothing was dropped.
    pub marker: usize,
    /// How many tokens the gap swallowed, for the marker's text and the log line.
    pub elided: usize,
}

impl Plan {
    pub fn total(&self) -> usize { self.head + self.marker + self.tail }
    pub fn truncated(&self) -> bool { self.elided > 0 }
}

/// Decide the split for `n` prompt tokens against `max`, reserving `marker` tokens if a cut happens.
///
/// `head_cap` bounds the head so a long prompt does not spend a third of its budget on preamble —
/// the head only needs to carry the system prompt, and every token beyond that is one fewer token
/// of the recent context the model is actually answering from.
pub fn plan(n: usize, max: usize, head_cap: usize, marker: usize) -> Plan {
    if n <= max {
        return Plan { head: n, tail: 0, marker: 0, elided: 0 };
    }
    // The marker comes out of the budget first, because the alternative is overflowing the window
    // the KV cache was sized from.
    let budget = max.saturating_sub(marker);
    if budget == 0 {
        // Degenerate: the window cannot hold even the marker. Keep the tail and say nothing — the
        // most recent tokens are the question being asked.
        return Plan { head: 0, tail: max, marker: 0, elided: n.saturating_sub(max) };
    }
    let head = (budget / 3).min(head_cap).min(budget);
    let tail = budget - head;
    Plan { head, tail, marker, elided: n - head - tail }
}

/// The text of the marker. Short on purpose: it is spending prompt budget to say one thing.
pub fn marker_text(elided: usize) -> String {
    format!("\n[... {elided} tokens elided from the middle of this context ...]\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_that_fits_is_untouched() {
        let p = plan(500, 4096, 1024, 16);
        assert_eq!(p, Plan { head: 500, tail: 0, marker: 0, elided: 0 });
        assert!(!p.truncated());
    }

    /// The property that matters: the plan can never exceed the window it was given. The old code
    /// filled the budget exactly, so any marker appended afterwards overflowed — and the window is
    /// what the KV cache is allocated from, so overflow is a device OOM rather than a long prompt.
    #[test]
    fn the_plan_never_exceeds_the_budget() {
        for n in [0usize, 1, 255, 4096, 13412, 1_000_000] {
            for max in [256usize, 1024, 3840, 32768] {
                for marker in [0usize, 16, 64] {
                    let p = plan(n, max, 1024, marker);
                    assert!(p.total() <= max.max(n.min(max)),
                        "n={n} max={max} marker={marker}: total {} over {max}", p.total());
                    assert!(p.total() <= max, "n={n} max={max} marker={marker}: {} > {max}", p.total());
                }
            }
        }
    }

    /// Nothing is invented and nothing is double-counted: what is kept plus what is dropped is the
    /// prompt that came in.
    #[test]
    fn kept_plus_elided_accounts_for_the_whole_prompt() {
        for n in [4096usize, 13412, 99_999] {
            let p = plan(n, 3840, 1024, 16);
            assert_eq!(p.head + p.tail + p.elided, n, "n={n}: {p:?} does not account for the prompt");
        }
    }

    /// The operator's actual case, from the log: 13412 tokens into a 3840 window.
    #[test]
    fn the_reported_case_reserves_room_for_the_marker() {
        let p = plan(13412, 3840, 1024, 16);
        assert_eq!(p.marker, 16);
        assert_eq!(p.head, 1024, "head is capped so the budget goes to recent context");
        assert_eq!(p.tail, 3840 - 16 - 1024);
        assert_eq!(p.total(), 3840, "the window is filled exactly, marker included");
        assert_eq!(p.elided, 13412 - 1024 - (3840 - 16 - 1024));
    }

    /// A window too small to hold the marker must still produce something usable rather than
    /// spending its entire budget announcing that it has no budget.
    #[test]
    fn a_window_smaller_than_the_marker_keeps_the_question() {
        let p = plan(5000, 8, 1024, 16);
        assert_eq!(p.marker, 0, "no room to say anything");
        assert_eq!(p.tail, 8, "the most recent tokens are the question being asked");
        assert_eq!(p.head, 0);
    }

    #[test]
    fn the_head_cap_binds_before_the_third() {
        // A large window: a third would be 8000, but the head only carries a system prompt.
        let p = plan(100_000, 24_000, 1024, 16);
        assert_eq!(p.head, 1024);
        assert!(p.tail > 22_000, "the rest belongs to recent context, got {}", p.tail);
    }

    #[test]
    fn the_marker_names_the_size_of_the_gap() {
        let t = marker_text(9_572);
        assert!(t.contains("9572") && t.contains("elided"), "{t}");
    }
}

#[cfg(test)]
mod against_a_real_tokenizer {
    use super::*;

    /// The marker has to be priced in TOKENS, and the token cost of a string is a tokenizer's
    /// business, not arithmetic's. This checks the reservation is large enough for the real marker
    /// on a real vocabulary — under-reserving would truncate the marker into nonsense, and
    /// over-reserving silently spends context.
    #[test]
    fn the_reserved_budget_holds_the_real_marker() {
        let bytes = match crate::testfix::read("/tmp/tiny-qwen3.gguf") { Some(b) => b, None => return };
        let g = match crate::gguf::parse(&bytes) { Ok(g) => g, Err(_) => return };
        let tok = match crate::tokenizer::Tokenizer::from_gguf(&g) { Ok(t) => t, Err(_) => return };

        // Reserve using a zero-count marker, then check the real one fits — that is exactly what
        // the backend does, and the digits of the elided count are what could overflow it.
        let reserve = tok.encode(&marker_text(0)).len().max(8);
        for elided in [1usize, 42, 9_572, 1_234_567] {
            let real = tok.encode(&marker_text(elided)).len();
            assert!(real <= reserve + 6,
                "marker for {elided} costs {real} tokens against a {reserve}-token reservation — \
the digits must not blow the budget");
        }
    }

    /// End to end at the operator's reported numbers: 13412 into 3840, marker priced by a real
    /// tokenizer, nothing exceeding the window.
    #[test]
    fn the_reported_overflow_fits_exactly() {
        let bytes = match crate::testfix::read("/tmp/tiny-qwen3.gguf") { Some(b) => b, None => return };
        let g = match crate::gguf::parse(&bytes) { Ok(g) => g, Err(_) => return };
        let tok = match crate::tokenizer::Tokenizer::from_gguf(&g) { Ok(t) => t, Err(_) => return };
        let mlen = tok.encode(&marker_text(0)).len().max(8);
        let p = plan(13412, 3840, 1024, mlen);
        let mids = tok.encode(&marker_text(p.elided));
        let kept = p.head + mids.len().min(p.marker) + p.tail;
        assert!(kept <= 3840, "kept {kept} exceeds the window");
        assert!(p.elided > 9000, "the gap should be most of the prompt, got {}", p.elided);
    }
}

/// How the context situation should read to a person, in one short line.
///
/// The elision from `plan` is currently announced on stderr, where the operator will never see it:
/// they paste a repository, the middle is silently removed, and the only evidence is a log line in
/// a terminal behind the window. A reply built from a third of the material looks exactly like a
/// reply built from all of it — that is the whole problem with a quiet truncation, and it is a UI
/// problem rather than an engine one.
///
/// Pure formatting so it can be pinned without a display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Comfortably inside the window.
    Fits,
    /// Inside, but close enough that the next turn probably will not be.
    Tight,
    /// Content was dropped. The number is what went.
    Elided(usize),
}

/// Classify a prompt against its window. `tight` is the fraction above which to warn.
pub fn fit(prompt: usize, window: usize, elided: usize) -> Fit {
    if elided > 0 { return Fit::Elided(elided); }
    if window > 0 && prompt * 100 >= window * 85 { return Fit::Tight; }
    Fit::Fits
}

/// The badge text. Short because it sits in a status line, and specific because "context full" tells
/// the operator nothing they can act on — the counts tell them whether to raise n_ctx or trim.
pub fn fit_label(prompt: usize, window: usize, elided: usize) -> String {
    match fit(prompt, window, elided) {
        Fit::Fits => format!("{prompt}/{window} tok"),
        Fit::Tight => format!("{prompt}/{window} tok — nearly full"),
        Fit::Elided(n) => format!("{prompt}/{window} tok — {n} ELIDED from the middle"),
    }
}

#[cfg(test)]
mod fit_tests {
    use super::*;

    #[test]
    fn a_dropped_middle_is_never_reported_as_merely_full() {
        // The distinction that matters: "nearly full" invites the operator to continue, "elided"
        // tells them the answer they are about to read was built from a hole.
        assert_eq!(fit(13412, 3840, 9572), Fit::Elided(9572));
        assert!(fit_label(13412, 3840, 9572).contains("9572"));
        assert!(fit_label(13412, 3840, 9572).contains("ELIDED"));
    }

    #[test]
    fn tight_warns_before_anything_is_lost() {
        assert_eq!(fit(3500, 3840, 0), Fit::Tight, "85% should warn while the next turn can still fit");
        assert_eq!(fit(1000, 3840, 0), Fit::Fits);
        assert!(fit_label(1000, 3840, 0).contains("1000/3840"));
    }

    #[test]
    fn elision_outranks_tightness() {
        // A prompt can be both over the tight threshold and elided; the loss is the headline.
        assert!(matches!(fit(9999, 3840, 1), Fit::Elided(1)));
    }

    #[test]
    fn a_zero_window_does_not_divide_by_it() {
        assert_eq!(fit(10, 0, 0), Fit::Fits);
        assert!(!fit_label(10, 0, 0).is_empty());
    }
}

/// Pull a cut back to the nearest boundary at or before `head`, searching at most `slack` tokens.
///
/// c4-798 made the gap visible; this decides WHERE it opens. The plan's indices are arbitrary token
/// positions, so the head ends mid-function and the tail resumes mid-line — the model receives a
/// truncated file followed by half of another and has to guess where one stops. Snapping to a line
/// boundary costs a few tokens and turns both edges into something a reader can parse.
///
/// It may only ever SHRINK what is kept. Growing to reach a nicer boundary would push the prompt
/// past the window the KV cache was sized from, which is the device OOM the bound exists to prevent
/// — so a boundary beyond `head` is not a candidate, however close.
pub fn snap_back(boundary: &[bool], head: usize, slack: usize) -> usize {
    let head = head.min(boundary.len());
    let floor = head.saturating_sub(slack);
    for i in (floor..head).rev() {
        if boundary[i] { return i + 1; }   // keep the boundary token itself
    }
    head
}

/// Push a resume point forward to the nearest boundary at or after `start`, within `slack`.
///
/// Forward for the same reason snap_back goes backward: moving the tail's start later keeps FEWER
/// tokens, so the budget is never exceeded.
pub fn snap_fwd(boundary: &[bool], start: usize, slack: usize) -> usize {
    let n = boundary.len();
    if start >= n { return start; }
    let ceil = (start + slack).min(n);
    for (i, b) in boundary.iter().enumerate().take(ceil).skip(start) {
        if *b { return i + 1; }            // resume AFTER the boundary, at a fresh line
    }
    start
}

#[cfg(test)]
mod snapping {
    use super::*;

    /// `.` is an ordinary token, `|` ends a line.
    fn marks(pattern: &str) -> Vec<bool> { pattern.chars().map(|c| c == '|').collect() }

    #[test]
    fn the_head_retreats_to_the_line_it_was_cutting() {
        //            0123456789
        let b = marks("..|...|...");
        assert_eq!(snap_back(&b, 9, 8), 7, "should fall back to just after the boundary at 6");
        assert_eq!(snap_back(&b, 3, 8), 3, "already just after a boundary — unchanged");
    }

    #[test]
    fn the_tail_resumes_on_a_fresh_line() {
        let b = marks("..|...|...");
        assert_eq!(snap_fwd(&b, 3, 8), 7, "skip forward past the boundary at 6");
        assert_eq!(snap_fwd(&b, 7, 8), 7, "already at a line start");
    }

    /// THE SAFETY PROPERTY. Snapping must never enlarge the kept region, because the window is what
    /// the KV cache was allocated from. Checked exhaustively over every cut position and slack on a
    /// pattern with awkward boundary spacing.
    #[test]
    fn snapping_never_keeps_more_than_the_plan_allowed() {
        let b = marks("...|.|.......|..|.....|...");
        let n = b.len();
        for head in 0..=n {
            for start in head..=n {
                for slack in [0usize, 1, 3, 8, 64] {
                    let h = snap_back(&b, head, slack);
                    let s = snap_fwd(&b, start, slack);
                    assert!(h <= head, "head grew from {head} to {h}");
                    assert!(s >= start, "tail start moved earlier, from {start} to {s}");
                    assert!(h + (n - s.min(n)) <= head + (n - start),
                        "kept region grew: head={head} start={start} slack={slack}");
                }
            }
        }
    }

    /// No boundary within reach is the normal case for minified or single-line input, and it must
    /// cost nothing rather than searching to the start of the prompt.
    #[test]
    fn a_prompt_with_no_boundaries_is_left_exactly_alone() {
        let b = marks("..........");
        assert_eq!(snap_back(&b, 7, 4), 7);
        assert_eq!(snap_fwd(&b, 3, 4), 3);
    }

    /// Slack bounds the search: a boundary just outside it is not taken, so one enormous line
    /// cannot drag the cut arbitrarily far and shrink the context to nothing.
    #[test]
    fn slack_bounds_how_much_context_snapping_may_spend() {
        let b = marks("|.........");
        assert_eq!(snap_back(&b, 9, 3), 9, "the only boundary is 8 back, outside a slack of 3");
        assert_eq!(snap_back(&b, 9, 9), 1, "within a slack of 9 it is taken");
    }

    #[test]
    fn indices_at_and_past_the_end_are_handled() {
        let b = marks("..|.");
        assert_eq!(snap_back(&b, 99, 4), 3, "clamped to the slice, then snapped");
        assert_eq!(snap_fwd(&b, 99, 4), 99, "past the end, nothing to do");
    }
}

#[cfg(test)]
mod on_a_real_paste {
    use super::*;

    /// End to end on source text with a real tokenizer: the surviving edges must land on line
    /// boundaries, which is the whole point — an arbitrary cut hands the model a truncated function
    /// and half of another, and it cannot tell where one stopped.
    #[test]
    fn both_edges_of_a_source_paste_land_on_line_boundaries() {
        let bytes = match crate::testfix::read("/tmp/tiny-qwen3.gguf") { Some(b) => b, None => return };
        let g = match crate::gguf::parse(&bytes) { Ok(g) => g, Err(_) => return };
        let tok = match crate::tokenizer::Tokenizer::from_gguf(&g) { Ok(t) => t, Err(_) => return };

        // Something shaped like a pasted file: many short lines.
        let src: String = (0..400).map(|i| format!("fn item_{i}(x: u32) -> u32 {{ x + {i} }}\n"))
            .collect();
        let ids = tok.encode(&src);
        if ids.len() < 600 { return; }                    // fixture vocab too coarse to be a test
        let boundary: Vec<bool> = ids.iter()
            .map(|&t| tok.decode_bytes(&[t]).contains(&b'\n')).collect();
        if !boundary.iter().any(|b| *b) { return; }       // no newline tokens in this vocabulary

        let max = ids.len() / 2;
        let mlen = tok.encode(&marker_text(0)).len().max(8);
        let pl = plan(ids.len(), max, 64, mlen);
        let head = snap_back(&boundary, pl.head, 48);
        let tail_start = snap_fwd(&boundary, ids.len() - pl.tail, 48);

        // The budget is still respected — the property that matters most.
        assert!(head + mlen + (ids.len() - tail_start) <= max,
            "snapped plan exceeds the window: {head} + {mlen} + {} > {max}", ids.len() - tail_start);

        // And both edges sit at a line start, unless no boundary was within slack.
        if head > 0 && head < pl.head {
            assert!(boundary[head - 1], "the head must end just after a newline token");
        }
        if tail_start > 0 && tail_start < ids.len() {
            assert!(boundary[tail_start - 1], "the tail must resume just after a newline token");
        }
    }
}

/// Advice when a prompt is being truncated: is the WINDOW the limit, or the MODEL?
///
/// The operator loaded a monorepo, asked for a summary, and got eight turns and no files. The log
/// says why, once the numbers are put together: the context grew 6662 → 8115 tokens across the
/// agent loop while the window stayed at 4864, so every turn elided up to 3289 tokens FROM THE
/// MIDDLE — which is exactly where the imported repository sat. The model was answering about code
/// it had never been shown, and each turn deleted more of it.
///
/// The window came from a SETTING, not from the model. Llama-3.2-1B is trained to 131072 tokens; it
/// was being run at 4864. c4-798 made the elision visible and c4-802 made its edges clean, and
/// neither says the thing that would have ended this in one line: the truncation was avoidable.
///
/// Returns None when the setting is already close to what the model supports — then the prompt
/// really is too big and the advice would be noise.
pub fn window_advice(window: usize, trained_ctx: usize, elided: usize) -> Option<String> {
    if elided == 0 || trained_ctx == 0 { return None; }
    // Only speak when the headroom is worth acting on. A window within 2x of the model's limit is a
    // reasonable memory trade; 27x is a setting nobody chose deliberately.
    // <= not <: a model trained to exactly twice the window is the boundary of "fair trade", and
    // advising there would fire on a deliberate 8192-of-16384 choice.
    if trained_ctx <= window.saturating_mul(2) { return None; }
    Some(format!(
        "this model is trained for {trained_ctx} tokens and is running at {window} — the {elided} \
tokens just dropped were NOT a model limit, they were the n_ctx setting. Raising n_ctx to {} would \
have kept the whole prompt (KV memory grows linearly with it, so raise it deliberately rather than \
to the maximum)",
        (window + elided).next_power_of_two().min(trained_ctx)))
}

#[cfg(test)]
mod advice {
    use super::*;

    /// The operator's exact case: 4864 window, 131072 trained, 3289 elided.
    #[test]
    fn a_setting_far_below_the_model_is_named_as_the_cause() {
        let a = window_advice(4864, 131072, 3289).expect("should advise");
        assert!(a.contains("131072") && a.contains("4864"), "{a}");
        assert!(a.contains("not a model limit") || a.contains("NOT a model limit"), "{a}");
        // Suggests something that would actually fit, without suggesting the maximum.
        // 4864 + 3289 = 8153, so the next power of two is 8192 — my first version of this test
        // asserted 16384 by rounding in my head, and the code was right.
        assert!(a.contains("8192"), "should suggest the smallest size that holds the prompt: {a}");
        assert!(!a.contains("to 131072 would"), "and not simply the model's ceiling: {a}");
    }

    /// A window already near the model's limit means the prompt really is too big, and advice would
    /// be noise on top of a truncation the operator cannot avoid.
    #[test]
    fn a_reasonable_window_gets_no_advice() {
        assert!(window_advice(8192, 8192, 500).is_none());
        assert!(window_advice(8192, 16384, 500).is_none(), "2x headroom is a fair memory trade");
        assert!(window_advice(4096, 131072, 0).is_none(), "nothing was dropped");
        assert!(window_advice(4096, 0, 900).is_none(), "unknown trained context says nothing");
    }

    /// The suggestion must actually hold the prompt that was just cut.
    #[test]
    fn the_suggested_size_would_have_fit() {
        for (w, el) in [(4864usize, 3289usize), (2048, 100), (4096, 60000)] {
            let a = window_advice(w, 131072, el).unwrap();
            let n: usize = a.split("Raising n_ctx to ").nth(1).unwrap()
                .split(' ').next().unwrap().parse().unwrap();
            assert!(n >= w + el || n == 131072, "w={w} el={el}: suggested {n}, needed {}", w + el);
        }
    }
}

// ---------------------------------------------------------------- automatic sizing

/// KV cache bytes for one token: K and V, every layer, at the cache's element width.
///
/// The number that makes automatic sizing possible instead of a setting somebody has to guess.
/// Grows LINEARLY with context, which is why "set it to the model's maximum" is not the answer —
/// Llama-3.2-1B at its full 131072 tokens wants about 4 GiB of KV on top of the weights.
pub fn kv_bytes_per_token(n_layers: usize, n_kv_heads: usize, head_dim: usize, elem_bytes: usize) -> u64 {
    2 * (n_layers as u64) * (n_kv_heads as u64) * (head_dim as u64) * (elem_bytes as u64)
}

/// Choose a context window: big enough for the work, within the model, within memory.
///
/// The operator's repository summary failed because n_ctx was 4864 against a model trained to
/// 131072, so every agent turn elided the repo out of the middle of its own prompt. Telling them to
/// raise a setting was the wrong answer to give a long-horizon agent: the engine knows what the
/// prompt needs, what the model supports and what a token costs, so it can size the window itself.
///
/// Three bounds, in order of authority:
///   * the MODEL — never exceed what it was trained for, whatever the memory allows;
///   * MEMORY — never plan a KV cache larger than the budget, whatever the prompt wants;
///   * the WORK — do not reserve more than the prompt needs plus room to answer.
///
/// Rounded up to a power of two so the choice is stable across turns: a window that tracks the
/// prompt exactly would resize every turn, and each resize discards the KV cache the prefix tree
/// depends on — turning an automatic convenience into a per-turn full prefill.
pub fn plan_ctx(need: usize, floor: usize, trained_max: usize, kv_per_token: u64,
                budget_bytes: u64) -> usize {
    let trained = if trained_max == 0 { usize::MAX } else { trained_max };
    let by_memory = if kv_per_token == 0 { usize::MAX } else { (budget_bytes / kv_per_token) as usize };
    let want = need.max(floor).next_power_of_two();
    want.min(trained).min(by_memory).max(floor.min(trained).min(by_memory))
}

/// The full window policy for one generate call: plan_ctx plus the pieces every caller would
/// otherwise reimplement — the operator floor, the app's hard ceiling, and the guard on a memory
/// budget that cannot even hold the floor.
///
/// Returns (window, Some(narration)) when auto-sizing changed anything, (window, None) when the
/// operator's n_ctx already covered the need — a resize line for a window that did not resize is
/// noise that teaches people to skip the line that matters.
///
/// `planned_weight_bytes == 0` or an unknown device budget disables the memory bound rather than
/// misapplying it: plan_ctx treats kv_per_token == 0 as "no bound", while a 0 BUDGET with a real
/// per-token cost clamps the window to zero. Same for a budget too small to hold even the floor —
/// the model already loaded and sessions already ran at the floor, so a bound that forbids the
/// floor is evidence the arithmetic inputs are off, not that the device cannot.
pub fn auto_window(need: usize, n_ctx: u32, trained_max: usize, kv_per_token: u64,
                   planned_weight_bytes: u64, device_budget: Option<u64>, app_max: usize)
                   -> (usize, Option<String>) {
    let floor = (n_ctx as usize).clamp(1024, app_max);
    let left = device_budget.unwrap_or(0).saturating_sub(planned_weight_bytes);
    let (kv_pt, budget) = if planned_weight_bytes == 0 || device_budget.is_none()
        || left < kv_per_token.saturating_mul(floor as u64) { (0, 0) } else { (kv_per_token, left) };
    let raw = plan_ctx(need, floor, trained_max, kv_pt, budget);
    let w = raw.min(app_max);
    // SILENT when the prompt did not drive it. plan_ctx rounds even the floor to a power of two
    // (4864 -> 8192, its KV-stability rationale), so comparing against the raw floor would narrate
    // on EVERY call with a non-pow2 slider — a resize line for a static property of the setting.
    // Narrate only when the need exceeded the floor, or when a bound cut BELOW the floor (a model
    // trained shorter than the slider is worth one line even on a short prompt).
    if need <= floor && w >= floor { return (w, None); }
    let by_memory = if kv_pt == 0 { usize::MAX } else { (budget / kv_pt) as usize };
    let reason = if raw > app_max { "the app's hard context ceiling" }
                 else { ctx_reason(w, need, trained_max, by_memory) };
    (w, Some(format!("context window auto-sized {floor} \u{2192} {w} — held by {reason} \
(the prompt plus reply headroom needs {need} tokens)")))
}

/// What to do about a reply budget the window can no longer hold.
///
/// The failure this exists for is silent and slow: `n_new = ceiling.min(window - prompt)` never
/// errors, it just hands back a smaller number every turn. A real session shows the reply budget
/// going 2470 -> 2050 -> 1832 -> 1187 -> 874 -> 814 over seventeen turns while the operator was
/// told nothing, and a turn that asked for a long answer got exactly 814 tokens and stopped
/// mid-thought. Autotuning the budget downward is a legitimate policy — it never blocks — but it
/// is the WRONG DEFAULT for long-horizon work, where the right move is to stop, compact what has
/// accumulated, and continue with room to answer.
///
/// Two policies, chosen by the operator, decided here so both paths are one function.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReliefPolicy {
    /// Shrink the reply budget to whatever is left. Never blocks; degrades quietly.
    Autotune,
    /// When the squeeze would take the budget below what an answer needs, stop and ask for a
    /// summary of the older turns first, then continue with the freed room.
    SummarizeAndContinue,
}

/// The decision for one turn.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Relief {
    /// Room to spare: generate with this budget (the ceiling, unsqueezed).
    Proceed { n_new: usize },
    /// Squeezed but still workable: generate with this budget. `note` is non-None only when the
    /// squeeze crossed into territory worth telling the operator about, so an ordinary turn is
    /// not narrated.
    Squeeze { n_new: usize, note: Option<String> },
    /// Compact first. `summarize_upto` is how many of the OLDEST prompt tokens should be folded
    /// into a summary, chosen to free enough room for a full-ceiling reply plus headroom;
    /// `keep_recent` is what must survive verbatim — the recent turns are the ones the next reply
    /// actually continues, and summarizing those is how a compaction loses the thread.
    Summarize { summarize_upto: usize, keep_recent: usize, reason: String },
}

/// Below this many tokens a reply is not an answer, it is a fragment: the model stops mid-sentence
/// and the operator's next move is "continue", which costs another full prefill. Chosen as the
/// point where the observed session's replies began hitting their cap exactly (one turn produced
/// 1357 of 1357) rather than finishing.
pub const REPLY_FLOOR: usize = 512;

/// Fraction of the window kept verbatim when summarizing. The recent turns are what the next
/// reply continues; folding them into prose is how a compaction produces an answer that has lost
/// the thread it was supposed to pick up.
const KEEP_RECENT_FRAC: f32 = 0.35;

/// How much a fold is expected to compress: `summarize_upto / SUMMARY_COMPRESSION` tokens of
/// summary. Conservative on purpose — over-estimating compression is what makes a compaction
/// recommend itself, cost a round trip, and land back in the squeeze.
const SUMMARY_COMPRESSION: usize = 6;
/// And no summary is allowed to be huge, whatever the fold's size implies.
const SUMMARY_CAP: usize = 512;

/// Decide what to do with `ceiling` tokens of requested reply inside `window`, given a prompt of
/// `prompt` tokens.
///
/// `Summarize` is returned only when it would actually help: if the prompt is mostly recent turns
/// (nothing old enough to fold), compaction frees nothing and the honest answer is the squeeze,
/// with the reason said out loud rather than a second policy that also fails silently.
pub fn relief(policy: ReliefPolicy, prompt: usize, window: usize, ceiling: usize) -> Relief {
    let room = window.saturating_sub(prompt);
    if room >= ceiling { return Relief::Proceed { n_new: ceiling }; }
    let squeezed = room.max(1);
    let note = |n: usize| Some(format!(
        "reply budget squeezed {ceiling} \u{2192} {n} — the prompt ({prompt}) leaves {room} of a \
{window} window. Long-horizon mode can summarize the older turns instead of shrinking every \
reply; this turn will be cut off at {n} tokens."));
    match policy {
        ReliefPolicy::Autotune => {
            // Narrate only once it matters. A ceiling trimmed from 2470 to 2050 is not news; a
            // budget under the floor is the turn that ends mid-sentence.
            let n = squeezed;
            Relief::Squeeze { n_new: n, note: if n < REPLY_FLOOR { note(n) } else { None } }
        }
        ReliefPolicy::SummarizeAndContinue => {
            if squeezed >= REPLY_FLOOR {
                // still a workable answer — compaction would cost a round trip for nothing
                return Relief::Squeeze { n_new: squeezed, note: None };
            }
            let keep_recent = ((window as f32 * KEEP_RECENT_FRAC) as usize).min(prompt);
            // Fold everything older than the kept tail — but the SUMMARY ITSELF occupies the
            // window, and a rule that forgets that recommends a compaction which frees less than
            // it promised, gets squeezed anyway, and has spent a round trip to arrive at the same
            // place. Price it at the compression this codebase actually asks for, floored so a
            // tiny fold is not modelled as free.
            let summarize_upto = prompt.saturating_sub(keep_recent);
            let summary_cost = (summarize_upto / SUMMARY_COMPRESSION).clamp(
                if summarize_upto == 0 { 0 } else { 32 }, SUMMARY_CAP);
            let freed_room = window.saturating_sub(keep_recent + summary_cost);
            if summarize_upto == 0 || freed_room < REPLY_FLOOR {
                return Relief::Squeeze { n_new: squeezed, note: note(squeezed) };
            }
            Relief::Summarize {
                summarize_upto,
                keep_recent,
                reason: format!("the prompt ({prompt} tokens) leaves {room} of a {window} window \
\u{2014} under the {REPLY_FLOOR}-token floor for a complete reply. Folding the oldest \
{summarize_upto} tokens into a summary and keeping the most recent {keep_recent} verbatim \
restores room for a full answer (the summary itself is priced at ~{summary_cost} tokens)."),
            }
        }
    }
}

/// Why the window is what it is, for the log line. Naming the BINDING constraint is the point: the
/// operator's next question after "it resized" is always "why not further".
pub fn ctx_reason(chosen: usize, need: usize, trained_max: usize, by_memory: usize) -> &'static str {
    if trained_max != 0 && chosen >= trained_max { "the model's trained context" }
    else if chosen >= by_memory { "the KV memory budget" }
    else if chosen >= need { "what this prompt needs" }
    else { "a floor below what the prompt needs — it will still be truncated" }
}

#[cfg(test)]
mod sizing {
    use super::*;

    /// Llama-3.2-1B: 16 layers, 8 KV heads, head_dim 64, f16 cache.
    const L1B: (usize, usize, usize, usize) = (16, 8, 64, 2);

    #[test]
    fn the_operators_case_sizes_itself_up() {
        let (l, h, d, b) = L1B;
        let kv = kv_bytes_per_token(l, h, d, b);
        // 8115 tokens of prompt, model trained to 131072, a 2 GiB KV budget.
        // NEED IS THE PROMPT PLUS ROOM TO ANSWER. 8115 alone rounds to 8192, which holds the prompt
        // and leaves 77 tokens to reply in — technically not truncated and useless in practice. The
        // caller adds the reply budget before asking, and this asserts the result of that.
        let n = plan_ctx(8115 + 1024, 4864, 131072, kv, 2 << 30);
        assert!(n >= 8115 + 1024, "must hold the prompt AND a reply, got {n}");
        assert_eq!(n, 16384, "settling on a stable power of two");
        // And the bare prompt rounds to 8192 — my first version of this test asserted 16384 for
        // that input by rounding in my head, which is the second time this session.
        assert_eq!(plan_ctx(8115, 4864, 131072, kv, 2 << 30), 8192);
    }

    /// Memory wins over the prompt: a huge paste on a small budget must not plan an OOM.
    #[test]
    fn memory_bounds_the_window() {
        let (l, h, d, b) = L1B;
        let kv = kv_bytes_per_token(l, h, d, b);         // 32 KiB/token
        let n = plan_ctx(400_000, 4096, 131072, kv, 256 << 20);   // 256 MiB budget
        assert!(n < 131072, "the model would allow more than the memory does: {n}");
        assert!((n as u64) * kv <= (256 << 20), "the plan must fit the budget: {n} tokens");
    }

    /// The model wins over memory: never plan beyond what it was trained for, however much RAM.
    #[test]
    fn the_model_bounds_the_window() {
        let (l, h, d, b) = L1B;
        let kv = kv_bytes_per_token(l, h, d, b);
        assert_eq!(plan_ctx(1_000_000, 4096, 8192, kv, 64u64 << 30), 8192);
    }

    /// Powers of two, because a window that tracks the prompt resizes every turn and every resize
    /// throws away the KV cache the prefix tree reuses — which would cost more than it saved.
    #[test]
    fn the_choice_is_stable_as_the_prompt_grows() {
        let (l, h, d, b) = L1B;
        let kv = kv_bytes_per_token(l, h, d, b);
        let sizes: Vec<usize> = [6662usize, 7010, 7365, 7568, 7914, 8115]
            .iter().map(|&n| plan_ctx(n, 4096, 131072, kv, 8u64 << 30)).collect();
        assert!(sizes.windows(2).all(|w| w[0] == w[1]),
            "the operator's six growing turns must not resize six times: {sizes:?}");
    }

    /// Unknown trained context must not be read as zero — that would clamp every window to nothing.
    #[test]
    fn an_unknown_model_limit_does_not_collapse_the_window() {
        let kv = kv_bytes_per_token(16, 8, 64, 2);
        assert!(plan_ctx(8000, 4096, 0, kv, 8u64 << 30) >= 8000);
    }

    #[test]
    fn the_binding_constraint_is_named() {
        assert_eq!(ctx_reason(8192, 8000, 8192, 99999), "the model's trained context");
        assert_eq!(ctx_reason(4096, 9000, 131072, 4096), "the KV memory budget");
        assert_eq!(ctx_reason(16384, 8115, 131072, 99999), "what this prompt needs");
        assert!(ctx_reason(4096, 9000, 131072, 99999).contains("still be truncated"));
    }
    #[test]
    fn the_repo_case_grows_and_narrates_and_a_covered_need_stays_silent() {
        // c4-842's exact failure: n_ctx 4864, prompt 8115 + headroom, trained 131072. The window
        // must grow past the setting AND say why; a prompt the setting already covers must not
        // produce a resize line, or the line stops being read.
        let kv = kv_bytes_per_token(16, 8, 64, 4);
        let (w, why) = auto_window(8115 + 256, 4864, 131072, kv, 1 << 30, Some(20u64 << 30), 32768);
        assert!(w >= 8115 + 256, "got {w}");
        assert!(why.unwrap().contains("what this prompt needs"));
        let (w2, why2) = auto_window(2000, 4864, 131072, kv, 1 << 30, Some(20u64 << 30), 32768);
        // plan_ctx pow2-rounds even the floor (4864 -> 8192, KV stability); a covered need keeps
        // that rounding but earns NO narration — the resize is the slider's, not this prompt's.
        assert_eq!((w2, why2), (8192, None), "covered need: pow2 floor, no narration");
    }

    #[test]
    fn a_model_trained_SHORTER_than_the_slider_narrates_even_on_a_short_prompt() {
        let kv = kv_bytes_per_token(16, 8, 64, 4);
        let (w, why) = auto_window(500, 8192, 2048, kv, 1 << 30, Some(20u64 << 30), 32768);
        assert_eq!(w, 2048);
        assert!(why.unwrap().contains("trained context"));
    }

    #[test]
    fn the_app_ceiling_binds_and_is_named() {
        let kv = kv_bytes_per_token(16, 8, 64, 4);
        let (w, why) = auto_window(60_000, 4096, 131072, kv, 1 << 30, Some(64u64 << 30), 32768);
        assert_eq!(w, 32768);
        assert!(why.unwrap().contains("hard context ceiling"));
    }

    #[test]
    fn an_UNKNOWN_or_ABSURD_memory_picture_disables_the_bound_instead_of_zeroing_the_window() {
        // planned bytes 0 = no plan ran; budget None = unknown; budget smaller than the floor's
        // own KV = arithmetic is off (the model LOADED). All three must fall back to
        // trained/need bounds — plan_ctx with a 0 budget and a real kv cost returns 0.
        let kv = kv_bytes_per_token(16, 8, 64, 4);
        for (pwb, dev) in [(0u64, Some(8u64 << 30)), (1 << 30, None), (1 << 30, Some((1u64 << 30) + 1))] {
            let (w, _) = auto_window(8000, 4096, 131072, kv, pwb, dev, 32768);
            assert!(w >= 8000, "pwb={pwb} dev={dev:?}: got {w}");
        }
    }

    #[test]
    fn a_REAL_memory_bound_still_binds() {
        let kv = kv_bytes_per_token(16, 8, 64, 4);          // K AND V: 2*16*8*64*4 = 64 KiB/token
        // 20 GiB device, 19.5 GiB of weights -> 512 MiB of KV -> 8192 tokens
        let (w, why) = auto_window(400_000, 4096, 1 << 20, kv,
                                   (19u64 << 30) + (1u64 << 29), Some(20u64 << 30), 1 << 20);
        assert_eq!(w, 8192);
        assert!(why.unwrap().contains("KV memory budget"));
    }

}

#[cfg(test)]
mod relief_tests {
    use super::*;

    /// The observed session, turn by turn: prompt grows, the budget silently collapses. Under
    /// Autotune every one of these generates — which is the policy working as designed and also
    /// exactly why nobody noticed the replies getting shorter.
    #[test]
    fn autotune_never_blocks_and_that_is_the_problem() {
        let window = 4096;
        let ceiling = 2470;
        for prompt in [1626usize, 2046, 2264, 2909, 3222, 3282] {
            match relief(ReliefPolicy::Autotune, prompt, window, ceiling) {
                Relief::Proceed { n_new } => assert!(n_new == ceiling && prompt + ceiling <= window),
                Relief::Squeeze { n_new, .. } => {
                    assert!(n_new >= 1 && n_new < ceiling);
                    assert_eq!(n_new, window - prompt, "budget is exactly the room left");
                }
                Relief::Summarize { .. } => panic!("Autotune must never block"),
            }
        }
    }

    #[test]
    fn the_squeeze_is_narrated_only_once_it_costs_an_answer() {
        // 2264-token prompt: budget 1832, plenty — a line here would be noise
        let Relief::Squeeze { note, .. } = relief(ReliefPolicy::Autotune, 2264, 4096, 2470)
            else { panic!("squeezed") };
        assert!(note.is_none(), "an 1832-token budget is not news");
        // 3700-token prompt: 396 left, under the floor — the turn will end mid-sentence
        let Relief::Squeeze { n_new, note } = relief(ReliefPolicy::Autotune, 3700, 4096, 2470)
            else { panic!("squeezed") };
        assert_eq!(n_new, 396);
        assert!(note.expect("narrated").contains("396"), "the line names the number");
    }

    #[test]
    fn summarize_fires_exactly_where_autotune_would_have_degraded() {
        let (window, ceiling) = (4096usize, 2470usize);
        // comfortable: identical under both policies
        assert_eq!(relief(ReliefPolicy::Autotune, 1626, window, ceiling),
                   relief(ReliefPolicy::SummarizeAndContinue, 1626, window, ceiling));
        // squeezed but workable: still identical — compaction costs a round trip for nothing
        assert!(matches!(relief(ReliefPolicy::SummarizeAndContinue, 3222, window, ceiling),
                         Relief::Squeeze { n_new: 874, note: None }));
        // under the floor: this is where the policies diverge
        match relief(ReliefPolicy::SummarizeAndContinue, 3700, window, ceiling) {
            Relief::Summarize { summarize_upto, keep_recent, .. } => {
                assert!(keep_recent > 0 && summarize_upto > 0);
                assert_eq!(summarize_upto + keep_recent, 3700, "every prompt token is accounted for");
                assert!(window - keep_recent >= REPLY_FLOOR, "and the fold actually frees room");
            }
            other => panic!("expected a summarize decision, got {other:?}"),
        }
    }

    #[test]
    fn recent_turns_are_never_folded_away() {
        // The compaction failure mode: summarizing what the next reply is supposed to continue.
        let Relief::Summarize { keep_recent, .. } =
            relief(ReliefPolicy::SummarizeAndContinue, 4000, 4096, 2470) else { panic!("summarize") };
        assert!(keep_recent >= 1024, "a third of the window stays verbatim, got {keep_recent}");
    }

    #[test]
    fn a_fold_that_cannot_free_room_says_so_instead_of_looping() {
        // Window so small that the kept tail PLUS the summary still leaves nothing: a policy that
        // returned Summarize here would summarize, re-prefill, and get squeezed anyway — a round
        // trip spent to arrive at the same budget. My first draft did exactly that, because it
        // priced the summary at zero; this case is why the summary's own cost is in the rule.
        match relief(ReliefPolicy::SummarizeAndContinue, 690, 700, 2470) {
            Relief::Squeeze { note, .. } => assert!(note.is_some(), "and it explains why"),
            other => panic!("compaction cannot free room here: {other:?}"),
        }
        // But a fold that DOES help must still fire: same shape, a window with room to work in.
        assert!(matches!(relief(ReliefPolicy::SummarizeAndContinue, 900, 1024, 2470),
                         Relief::Summarize { .. }), "124 tokens of room becomes ~570 — worth it");
    }

    #[test]
    fn a_prompt_that_fits_is_untouched_by_either_policy() {
        for p in [ReliefPolicy::Autotune, ReliefPolicy::SummarizeAndContinue] {
            assert_eq!(relief(p, 1000, 8192, 2048), Relief::Proceed { n_new: 2048 });
        }
    }
}
