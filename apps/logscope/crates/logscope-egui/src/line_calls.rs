//! line_calls — the one-line tool grammar for models that cannot balance fences.
//!
//! The field failure (c11): Llama-3.2-1B emits tool blocks with prose inside
//! the fence, regurgitated instruction text and role markers mixed into JSON —
//! the fence parser rightly refuses, and (before the ladder in
//! `workspace_exec`) the model got no correction and re-emitted the same
//! garbage: six turns, zero files. The read_harness thesis applied to OUTPUT:
//! a 1B cannot reliably produce `<tool_call>{json}</tool_call>`, but it can
//! copy a one-line template:
//!
//! ```text
//! !3f2a90c1deadbeef read_range src/lib.rs 40 90
//! ```
//!
//! Prose around the line is IGNORED — extraction is per line, which is the
//! whole point: a model that wraps its call in narration still gets its call.
//! The nonce keeps exactly the channel property `taint::Nonce` documents:
//! content authored before the run cannot carry this run's token, so a call
//! quoted out of a file (with an old token or none) is inert. Only read-side
//! tools live here — a write needs a body, and a body needs a fence; small
//! models on this grammar are readers.

use crate::workspace_exec::Tool;

/// Why a `!`-prefixed line was not a call. Distinct from the fence parser's
/// `Rejected`: these are line-shaped failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineReject {
    /// The token after `!` is not this run's nonce — quoted from somewhere.
    WrongNonce,
    /// Right nonce, unknown verb.
    UnknownVerb(String),
    /// Right nonce and verb, wrong argument count/shape.
    BadArgs { verb: String, want: &'static str },
}

/// Extract one-line calls from a model turn. Every line beginning `!` is
/// considered; everything else is prose and ignored.
pub fn parse_line_calls(text: &str, nonce: &str) -> (Vec<Tool>, Vec<LineReject>) {
    let (mut out, mut bad) = (Vec::new(), Vec::new());
    for raw in text.lines() {
        let line = raw.trim();
        let Some(rest) = line.strip_prefix('!') else { continue };
        let mut it = rest.split_whitespace();
        let (tok, verb) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
        if tok != nonce {
            bad.push(LineReject::WrongNonce);
            continue;
        }
        let args: Vec<&str> = it.collect();
        match verb {
            "read_file" if args.len() == 1 => out.push(Tool::ReadFile { path: args[0].into() }),
            "read_range" if args.len() == 3 => {
                match (args[1].parse::<usize>(), args[2].parse::<usize>()) {
                    (Ok(a), Ok(b)) => out.push(Tool::ReadRange { path: args[0].into(), start: a, end: b }),
                    _ => bad.push(LineReject::BadArgs { verb: verb.into(), want: "read_range <path> <first-line> <last-line>" }),
                }
            }
            "list" if args.len() <= 1 => {
                out.push(Tool::ListDir { path: args.first().copied().unwrap_or("").into() })
            }
            // History is READ-side evidence, so it belongs in the small-model
            // grammar too: `!<nonce> log` or `!<nonce> log 20`.
            "log" if args.is_empty() => out.push(Tool::GitLog { n: 10, path: String::new() }),
            // `log 20`, or `log src/lib.rs` — a count or a path, whichever it
            // parses as. Small models write both and should not need to know
            // which slot they filled.
            "log" if args.len() == 1 => match args[0].parse::<usize>() {
                Ok(n) => out.push(Tool::GitLog { n, path: String::new() }),
                Err(_) => out.push(Tool::GitLog { n: 10, path: args[0].into() }),
            },
            "log" if args.len() == 2 => match args[1].parse::<usize>() {
                Ok(n) => out.push(Tool::GitLog { n, path: args[0].into() }),
                Err(_) => bad.push(LineReject::BadArgs { verb: "log".into(), want: "log <path> <count>" }),
            },
            "read_file" | "read_range" => bad.push(LineReject::BadArgs {
                verb: verb.into(),
                want: if verb == "read_file" { "read_file <path>" } else { "read_range <path> <first-line> <last-line>" },
            }),
            other => bad.push(LineReject::UnknownVerb(other.into())),
        }
    }
    (out, bad)
}

/// The corrective feedback a refused turn earns — one exact example with the
/// LIVE nonce, nothing else. Small models copy examples; they do not apply
/// abstract format descriptions, which is what the old generic nudge was.
pub fn corrective(nonce: &str) -> String {
    format!(
        "Your last reply contained no valid tool call. Reply with EXACTLY one line, \
         nothing else. Examples:\n\
         !{nonce} read_range src/lib.rs 1 60\n\
         !{nonce} list src\n\
         !{nonce} log 10\n\
         Or, if the task is finished, reply exactly: TASK_COMPLETE"
    )
}

/// Consecutive-refusal ladder: refusals escalate to the corrective, then to a
/// bounded abort with a NAMED verdict — the six-turn zero-file loop becomes a
/// two-correction stop that says why. Distinct from the repetition guard,
/// which keys on repeated ACTIONS; refusals can vary in shape every turn and
/// still be the same failure.
#[derive(Debug, Clone, Copy)]
pub struct RefusalLadder {
    misses: u32,
    cap: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LadderStep {
    /// Feed this text back before the next turn.
    Correct(String),
    /// Stop the run; the verdict names the failure for the log.
    Abort(String),
}

impl RefusalLadder {
    pub fn new(cap: u32) -> RefusalLadder {
        RefusalLadder { misses: 0, cap: cap.max(2) }
    }

    /// Call once per turn with whether ANY call was accepted this turn.
    pub fn observe(&mut self, accepted_any: bool, nonce: &str, last_reason: &str) -> Option<LadderStep> {
        if accepted_any {
            self.misses = 0;
            return None;
        }
        self.misses += 1;
        if self.misses >= self.cap {
            Some(LadderStep::Abort(format!(
                "{} consecutive turns produced no valid tool call (last: {last_reason}) — \
                 stopping instead of looping; the model cannot hold this run's call format",
                self.misses
            )))
        } else {
            Some(LadderStep::Correct(corrective(nonce)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace_exec::Tool;

    const N: &str = "3f2a90c1deadbeef";

    /// The point of the grammar: a valid line amid prose IS a call.
    #[test]
    fn a_line_wrapped_in_prose_still_calls() {
        let text = format!(
            "I think I should look at the library first.\n\
             !{N} read_range src/lib.rs 40 90\n\
             That should show the relevant part."
        );
        let (tools, bad) = parse_line_calls(&text, N);
        assert_eq!(bad, vec![]);
        assert_eq!(tools, vec![Tool::ReadRange { path: "src/lib.rs".into(), start: 40, end: 90 }]);
    }

    /// The exact field garbage must parse to NOTHING — not a reject storm,
    /// just prose. (`plan1.edit src/lib` has no `!` line.)
    #[test]
    fn the_field_gibberish_is_prose_not_calls() {
        let text = "```tool block, or reply TASK_COMPLETE.)ASSISTANT:```plan1.edit src/lib";
        let (tools, bad) = parse_line_calls(text, N);
        assert!(tools.is_empty() && bad.is_empty(), "{tools:?} {bad:?}");
    }

    /// The channel property survives the new grammar: an old-run token is a
    /// named reject and never a call.
    #[test]
    fn a_quoted_call_with_a_stale_nonce_is_inert() {
        let text = format!("!aaaabbbbccccdddd read_file secrets.txt\n!{N} list");
        let (tools, bad) = parse_line_calls(&text, N);
        assert_eq!(bad, vec![LineReject::WrongNonce]);
        assert_eq!(tools, vec![Tool::ListDir { path: "".into() }]);
    }

    /// History is reachable from the small-model grammar, with and without a
    /// count, and a non-numeric count is a named reject rather than a default.
    #[test]
    fn the_log_verb_reaches_git_history() {
        let (t, b) = parse_line_calls(&format!("!{N} log"), N);
        assert_eq!(t, vec![Tool::GitLog { n: 10, path: String::new() }]);
        assert!(b.is_empty());
        let (t, _) = parse_line_calls(&format!("!{N} log 25"), N);
        assert_eq!(t, vec![Tool::GitLog { n: 25, path: String::new() }]);
        // a non-numeric single argument is a PATH, not a mistake
        let (t, b) = parse_line_calls(&format!("!{N} log src/lib.rs"), N);
        assert_eq!(t, vec![Tool::GitLog { n: 10, path: "src/lib.rs".into() }]);
        assert!(b.is_empty());
        let (t, _) = parse_line_calls(&format!("!{N} log src/lib.rs 5"), N);
        assert_eq!(t, vec![Tool::GitLog { n: 5, path: "src/lib.rs".into() }]);
    }

    #[test]
    fn wrong_shapes_are_named_with_the_shape_wanted() {
        let text = format!("!{N} read_range src/lib.rs forty ninety\n!{N} frobnicate x");
        let (tools, bad) = parse_line_calls(&text, N);
        assert!(tools.is_empty());
        assert_eq!(bad.len(), 2);
        assert!(matches!(&bad[0], LineReject::BadArgs { want, .. } if want.contains("<first-line>")));
        assert!(matches!(&bad[1], LineReject::UnknownVerb(v) if v == "frobnicate"));
    }

    /// The six-turn loop, replayed against the ladder: two corrections, then
    /// a named abort at the cap — never an unbounded loop.
    #[test]
    fn the_ladder_corrects_then_aborts_and_acceptance_resets() {
        let mut l = RefusalLadder::new(3);
        assert!(matches!(l.observe(false, N, "prose inside the fence"),
            Some(LadderStep::Correct(msg)) if msg.contains(N) && msg.contains("read_range")));
        assert!(matches!(l.observe(false, N, "prose inside the fence"), Some(LadderStep::Correct(_))));
        let step = l.observe(false, N, "prose inside the fence");
        assert!(matches!(&step, Some(LadderStep::Abort(v)) if v.contains("3 consecutive")), "{step:?}");
        // acceptance resets the count
        let mut l = RefusalLadder::new(3);
        assert!(l.observe(true, N, "").is_none());
        assert!(matches!(l.observe(false, N, "x"), Some(LadderStep::Correct(_))));
        assert!(l.observe(true, N, "").is_none(), "reset after success");
    }
}

#[cfg(test)]
mod ladder_progress_tests {
    use super::*;

    const N: &str = "3f2a90c1deadbeef";

    /// A model claiming completion must NOT count as progress.
    ///
    /// The field case: an 8B whose every tool call was refused appended "The
    /// final answer is: TASK_COMPLETE (after verification)" to each reply. The
    /// call site passed `!tools.is_empty() || text.contains(DONE_MARKER)` as
    /// `accepted`, so the marker reset the counter on all nine turns and this
    /// ladder — cap 4 — never fired. The run was eventually stopped by the
    /// looping detector, a coarser guard that took more than twice as long.
    ///
    /// Asserting completion is exactly what a stuck model does, so it cannot
    /// be the evidence that something happened.
    #[test]
    fn repeated_refusals_abort_even_when_the_model_says_it_is_done() {
        let mut l = RefusalLadder::new(4);
        let mut steps = Vec::new();
        for _ in 0..6 {
            // `false` is what the call site now passes for a turn whose calls
            // were all refused, regardless of what the prose claims.
            steps.push(l.observe(false, N, "UnknownTool"));
        }
        assert!(matches!(steps[3], Some(LadderStep::Abort(_))),
            "cap 4 must abort on the 4th consecutive miss, got {:?}", steps[3]);
    }

    #[test]
    fn one_landed_call_still_clears_the_count() {
        // The reset must survive: a model that stumbles twice and then works
        // should not be aborted on its next stumble.
        let mut l = RefusalLadder::new(4);
        l.observe(false, N, "x");
        l.observe(false, N, "x");
        assert!(l.observe(true, N, "x").is_none(), "an accepted call clears the ladder");
        for i in 0..3 {
            assert!(!matches!(l.observe(false, N, "x"), Some(LadderStep::Abort(_))),
                "aborted at miss {i} after a reset — the count did not clear");
        }
    }

    #[test]
    fn the_correction_carries_the_live_nonce() {
        // A correction that omits the run token teaches the model the wrong
        // shape, which is the failure the ladder exists to break out of.
        let mut l = RefusalLadder::new(4);
        match l.observe(false, N, "x") {
            Some(LadderStep::Correct(msg)) => assert!(msg.contains(N), "{msg}"),
            other => panic!("expected a correction first, got {other:?}"),
        }
    }
}
