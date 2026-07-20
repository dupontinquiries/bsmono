//! Stepwise generation: prompt the model to work in NUMBERED STEPS and pause at each boundary with a
//! `⏸ STEP k/n — summary` marker line. The stream watcher halts generation at the marker; the harness
//! auto-continues on the user's behalf (warm-KV resume via the prefix tree, so a pause costs ~nothing) —
//! and each pause is an injection point where fan-out results (a small model's side-task output, compile
//! errors, etc.) can be spliced in before the next step. The UI collects the markers into a checklist so
//! a many-pause generation reads as one coherent, guided flow.
//!
//! Pure logic (parsing, policy, checklist extraction) lives here and is unit-tested; the chat layer wires
//! it to the token stream and rendering.

/// The instruction block appended to the system prompt when stepwise mode is on. Kept short and
/// mechanical — models follow format contracts better than prose requests.
pub const STEPWISE_PROMPT: &str = "\n\nSTEPWISE PROTOCOL: Structure your work as a short numbered plan (2-9 steps), state the plan first, \
then do ONE step at a time. After completing each step, output on its own line exactly:\n\
⏸ STEP <k>/<n> — <one-line summary of what you just did>\n\
and then STOP. You will be told to continue. Do not do more than one step between markers. \
If the work needs more steps than you first planned, just keep numbering past <n> (e.g. 4/3, 5/3) — the \
plan can grow. The run ends ONLY when you write TASK_COMPLETE on its own line, NOT when k reaches n; so on \
your actual final step, write your marker line and then TASK_COMPLETE.";

/// A parsed step marker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepMark { pub k: u32, pub n: u32, pub summary: String }

/// Find a completed step marker in `text` (scans from the end — the freshest marker wins).
/// Marker grammar: a line `⏸ STEP k/n — summary` (also tolerates `STEP k/n:` and ASCII dashes,
/// since models drift on punctuation).
pub fn last_step_mark(text: &str) -> Option<StepMark> {
    for line in text.lines().rev() {
        let l = line.trim();
        let body = l.strip_prefix('⏸').map(str::trim).unwrap_or(l);
        let rest = match body.strip_prefix("STEP ").or_else(|| body.strip_prefix("Step ")) {
            Some(r) => r,
            None => continue,
        };
        // k/n then a separator (— , - , :) then summary
        let (kn, tail) = match rest.split_once(|c: char| c == '—' || c == '-' || c == ':') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (rest.trim(), ""),
        };
        let (ks, ns) = match kn.split_once('/') { Some(p) => p, None => continue };
        let (Ok(k), Ok(n)) = (ks.trim().parse::<u32>(), ns.trim().parse::<u32>()) else { continue };
        if k == 0 || n == 0 || k > n || n > 50 { continue; }
        return Some(StepMark { k, n, summary: tail.trim_start_matches('—').trim().to_string() });
    }
    None
}

/// Should the harness pause the stream now? True when the tail of the streamed text has a marker for a
/// step we haven't already paused on (dedup by `last_paused` step number).
pub fn should_pause(text: &str, last_paused: u32) -> Option<StepMark> {
    let m = last_step_mark(text)?;
    if m.k > last_paused { Some(m) } else { None }
}

/// Explicit "I'm finished" signals a model emits. `k==n` is deliberately NOT one of these — models
/// routinely under-plan (say 3 steps, need 5), so treating the nominal last step as "done" was the main
/// cause of the harness stopping too early. Completion must be stated, not merely counted.
pub fn signals_complete(text: &str) -> bool {
    // scan only the tail — an early "TASK_COMPLETE" inside a plan/quote shouldn't end the run.
    let tail_start = text.len().saturating_sub(400);
    let tail = &text[tail_start..];
    const DONE: &[&str] = &["TASK_COMPLETE", "TASK COMPLETE", "ALL STEPS COMPLETE", "ALL DONE", "<DONE>"];
    DONE.iter().any(|d| tail.contains(d))
}

/// Auto-continue policy after a pause. Redesigned so the harness stops UNDER-continuing:
/// - An explicit completion signal (`signals_complete`) always stops — the clean finish.
/// - Reaching the nominal last step (`k==n`) NO LONGER stops on its own. A model that planned too few
///   steps gets a bounded GRACE of extra continues past `n` to actually finish (it can also just renumber
///   to a larger `n`, which this naturally follows). Only an explicit signal — or the grace running out —
///   ends a run that overshoots its plan.
/// - Two caps: `cap` bounds total continues (confused-model backstop); `grace` bounds how far PAST the
///   nominal plan length we'll keep going without an explicit completion (so `k==n` with no TASK_COMPLETE
///   doesn't silently ride the full `cap`). Returns whether to continue.
pub fn should_continue_ex(mark: &StepMark, text: &str, continues_so_far: u32, cap: u32, grace: u32) -> bool {
    if signals_complete(text) { return false; }     // explicit finish — the intended stop
    if continues_so_far >= cap { return false; }     // hard backstop against loops
    if mark.k < mark.n { return true; }              // mid-plan — obviously continue
    // At/past the nominal last step with NO completion signal: the model likely under-planned. Grant a
    // bounded grace of extra continues so it can finish, rather than cutting it off at a miscounted `n`.
    let past_nominal = continues_so_far.saturating_sub(mark.n.saturating_sub(1));
    past_nominal < grace
}

/// Back-compat wrapper: the old signature, now routed through the improved policy with a default grace so
/// existing callers stop under-continuing without a code change. `k==n` no longer hard-stops; an explicit
/// completion signal (or the caps) does. Prefer `should_continue_ex` for an explicit grace.
pub fn should_continue(mark: &StepMark, text: &str, continues_so_far: u32, cap: u32) -> bool {
    should_continue_ex(mark, text, continues_so_far, cap, DEFAULT_GRACE)
}

/// Default grace: how many continues past the model's nominal plan length we allow without an explicit
/// completion signal. Enough for a typical under-plan (planned N, needed a few more) without letting a
/// truly stuck model ride the whole `cap`.
pub const DEFAULT_GRACE: u32 = 4;

/// Does the tail of the reply end by ASKING PERMISSION to continue rather than finishing or pausing on a
/// marker? Small models (see the 4B transcripts) habitually close a step with "Let me know when you'd like
/// to proceed" / "Shall I continue?" / "Ready for the next step?" — a natural-language stop that is neither
/// a `⏸ STEP` marker nor `TASK_COMPLETE`. With auto-continue ON that's a SOFT CONTINUE signal, not a real
/// stop: the model has more to do and is just being polite. The harness should answer "yes, continue"
/// rather than surfacing it as a wait-for-user. High precision — only fires on a trailing solicitation in
/// the last line or two, so a mid-reply "let me know if you have questions" doesn't trip it.
pub fn trailing_solicitation(text: &str) -> bool {
    // an explicit completion beats a solicitation (a model can say "…done. Let me know if you need more")
    if signals_complete(text) { return false; }
    let tail = text.trim_end();
    // look only at the final ~2 lines / 200 chars — a solicitation is a closing line
    let start = tail.len().saturating_sub(200);
    let window = tail[start..].to_ascii_lowercase();
    // must actually be at the end: the last non-empty line should contain it
    let last_line = tail.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_ascii_lowercase();
    const SOLICIT: &[&str] = &[
        "let me know when you", "let me know if you", "let me know how you",
        "would you like me to", "shall i continue", "shall i proceed", "should i continue",
        "should i proceed", "ready to proceed", "ready for the next", "want me to continue",
        "want me to proceed", "when you're ready", "when you are ready", "just let me know",
        "let me know and i", "proceed to the next step", "move on to the next step",
    ];
    // fire only when the solicitation is in the CLOSING line (not buried mid-reply)
    SOLICIT.iter().any(|s| last_line.contains(s) || (window.contains(s) && window.rfind(s).unwrap_or(0) > window.len().saturating_sub(120)))
}

/// Strip a trailing permission-asking question from a reply so the continuation doesn't re-read the model's
/// own "let me know…" and stall again. Removes the final line(s) that are pure solicitation, keeping the
/// substantive content. Used to clean the text handed into the auto-continue prompt.
pub fn strip_trailing_solicitation(text: &str) -> &str {
    if !trailing_solicitation(text) { return text; }
    let trimmed = text.trim_end();
    // drop trailing lines while they look like solicitation / are empty, stop at real content
    let mut end = trimmed.len();
    for line in trimmed.lines().rev() {
        let low = line.trim().to_ascii_lowercase();
        let is_solicit = low.is_empty() || SOLICIT_ANY.iter().any(|s| low.contains(s));
        if is_solicit { end = end.saturating_sub(line.len() + 1); } else { break; }
    }
    trimmed[..end.min(trimmed.len())].trim_end()
}

/// The solicitation phrases, shared by detection and stripping.
const SOLICIT_ANY: &[&str] = &[
    "let me know", "would you like me to", "shall i continue", "shall i proceed",
    "should i continue", "should i proceed", "ready to proceed", "ready for the next",
    "want me to continue", "want me to proceed", "when you're ready", "when you are ready",
    "proceed to the next step", "move on to the next step",
];

/// Extract the checklist for rendering: every step marker in order, plus whether the run is finished.
/// The UI shows these as a compact steps guide; the marker lines can then be de-emphasized in the body.
pub fn checklist(text: &str) -> (Vec<StepMark>, bool) {
    let mut steps: Vec<StepMark> = Vec::new();
    for line in text.lines() {
        if let Some(m) = last_step_mark(line) {
            if steps.last().map_or(true, |p| m.k > p.k) { steps.push(m); }
        }
    }
    // "done" means the model explicitly signalled completion — NOT merely that the last emitted marker had
    // k==n (models under-plan, so k==n is not a reliable terminator; matches should_continue's policy).
    let done = signals_complete(text);
    (steps, done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_marker_variants() {
        let m = last_step_mark("did things\n⏸ STEP 2/5 — wrote the parser\nmore").unwrap();
        assert_eq!((m.k, m.n), (2, 5));
        assert_eq!(m.summary, "wrote the parser");
        // punctuation drift: colon, ascii dash, no pause glyph
        assert_eq!(last_step_mark("STEP 3/4: ran tests").unwrap().k, 3);
        assert_eq!(last_step_mark("Step 1/9 - scaffolded crate").unwrap().n, 9);
        // rejects nonsense
        assert!(last_step_mark("STEP 7/3 — impossible").is_none());
        assert!(last_step_mark("STEP x/y — nope").is_none());
        assert!(last_step_mark("no markers here").is_none());
    }

    #[test]
    fn pause_dedups_and_policy_caps() {
        let text = "plan...\n⏸ STEP 1/3 — set up crate\n";
        let m = should_pause(text, 0).unwrap();
        assert_eq!(m.k, 1);
        assert!(should_pause(text, 1).is_none(), "already paused on step 1");
        assert!(should_continue(&m, text, 0, 12), "steps remain → continue");
        // NEW behavior: reaching the nominal last step does NOT stop on its own (models under-plan).
        let last = StepMark { k: 3, n: 3, summary: String::new() };
        assert!(should_continue(&last, text, 0, 12), "k==n alone no longer stops — grace to finish");
        assert!(!should_continue(&m, text, 12, 12), "cap reached → stop");
        assert!(!should_continue(&m, "…TASK_COMPLETE", 0, 12), "explicit completion → stop");
    }

    #[test]
    fn grace_lets_underplanned_runs_finish_then_bounds_them() {
        let last = StepMark { k: 4, n: 4, summary: "did step 4".into() };
        let body = "working...\n⏸ STEP 4/4 — did step 4\n";
        // just past the plan: keep going (grace not exhausted) so a model that needed step 5,6 can get there
        assert!(should_continue_ex(&last, body, 4, 20, 4), "within grace past nominal plan → continue");
        assert!(should_continue_ex(&last, body, 6, 20, 4), "still within grace → continue");
        // grace exhausted: stop even though the global cap isn't hit (don't ride the full cap on a miscount)
        assert!(!should_continue_ex(&last, body, 8, 20, 4), "grace past nominal exhausted → stop");
        // explicit completion always wins, even with grace remaining
        assert!(!should_continue_ex(&last, "done\nTASK_COMPLETE", 4, 20, 4), "explicit finish → stop");
    }

    #[test]
    fn completion_signal_only_counts_at_the_tail() {
        // a plan that MENTIONS TASK_COMPLETE up front (as instructions) must not be treated as finished
        let planny = format!("I'll end with TASK_COMPLETE when done.\n{}⏸ STEP 1/2 — began", "x".repeat(500));
        assert!(!signals_complete(&planny), "early mention isn't a finish");
        assert!(signals_complete("…\n⏸ STEP 2/2 — wrapped up\nTASK_COMPLETE"), "tail signal counts");
        // tolerant variants
        assert!(signals_complete("all done, ALL STEPS COMPLETE"));
    }

    #[test]
    fn checklist_collects_in_order() {
        let text = "plan\n⏸ STEP 1/3 — a\nbody\n⏸ STEP 2/3 — b\nbody\n⏸ STEP 3/3 — c\nTASK_COMPLETE";
        let (steps, done) = checklist(text);
        assert_eq!(steps.len(), 3);
        assert!(done);
        assert_eq!(steps[1].summary, "b");
    }

    #[test]
    fn trailing_solicitation_detected() {
        // straight from the 4B feedback: the model closes a step by asking permission
        assert!(trailing_solicitation("Here's the setup.\n\nLet me know when you're ready to proceed."));
        assert!(trailing_solicitation("...added the terrain.\nShall I continue to the next step?"));
        assert!(trailing_solicitation("Done with step 2.\nWould you like me to proceed?"));
    }

    #[test]
    fn task_complete_beats_solicitation() {
        // a polite sign-off after finishing must NOT be read as "keep going"
        assert!(!trailing_solicitation("All done.\nTASK_COMPLETE\nLet me know if you need anything else."));
    }

    #[test]
    fn mid_reply_let_me_know_does_not_trip() {
        // a "let me know if you have questions" buried mid-reply, with real content AFTER it, isn't a stop
        let text = "Let me know if this is unclear.\n\nHere is the full implementation:\n\nfn main() {}\n\nThe code compiles and runs.";
        assert!(!trailing_solicitation(text), "solicitation not in the closing line → not a trailing ask");
    }

    #[test]
    fn strip_removes_the_trailing_ask_only() {
        let text = "Added the grid terrain component.\n\nLet me know when you'd like to proceed.";
        let stripped = strip_trailing_solicitation(text);
        assert_eq!(stripped, "Added the grid terrain component.");
        assert!(!stripped.to_ascii_lowercase().contains("let me know"));
    }

    #[test]
    fn strip_noop_when_no_solicitation() {
        let text = "Here is the code:\nfn main() {}";
        assert_eq!(strip_trailing_solicitation(text), text);
    }
}

/// Does this step call for running tests/build? (the tests→cargo stepwise hook)
pub fn step_wants_tests(summary: &str) -> bool {
    let l = summary.to_ascii_lowercase();
    ["test", "cargo build", "cargo check", "compile", "verify"].iter().any(|k| l.contains(k))
}

/// Extract `// file: <path>` + fenced blocks from a reply (fence-nesting aware: a block ends at the LAST
/// fence before the next `// file:` header or end-of-text, so contents containing ``` survive intact).
pub fn extract_files(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut idx = 0usize;
    let bytes = text;
    while let Some(h) = bytes[idx..].find("// file:") {
        let h = idx + h;
        let line_end = bytes[h..].find('\n').map(|e| h + e).unwrap_or(bytes.len());
        let path = bytes[h + "// file:".len()..line_end].trim().trim_end_matches(':').to_string();
        let Some(fs_rel) = bytes[line_end..].find("```") else { break };
        let fence_open = line_end + fs_rel;
        let body_start = bytes[fence_open..].find('\n').map(|e| fence_open + e + 1).unwrap_or(bytes.len());
        // block ends at the LAST fence before the next `// file:` (or end)
        let next_hdr = bytes[body_start..].find("// file:").map(|e| body_start + e).unwrap_or(bytes.len());
        let region = &bytes[body_start..next_hdr];
        let Some(last_fence) = region.rfind("\n```") else { idx = next_hdr; continue };
        if !path.is_empty() && !path.contains("..") {
            out.push((path, region[..last_fence + 1].to_string()));
        }
        idx = next_hdr.max(body_start + last_fence + 4);
    }
    out
}

#[cfg(test)]
mod file_extract_tests {
    use super::*;
    #[test]
    fn nested_fences_survive_extraction() {
        let t = "// file: README.md\n```markdown\n# X\n```rust\nfn a(){}\n```\ntail\n```\n\n// file: src/lib.rs\n```rust\npub fn b(){}\n```\n";
        let files = extract_files(t);
        assert_eq!(files.len(), 2);
        assert!(files[0].1.contains("```rust"), "inner fence kept in README");
        assert!(files[0].1.contains("tail"));
        assert_eq!(files[1].0, "src/lib.rs");
        assert!(step_wants_tests("ran cargo test on the parser"));
        assert!(!step_wants_tests("wrote the README"));
    }
}
