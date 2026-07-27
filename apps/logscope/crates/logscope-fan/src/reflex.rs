//! reflex.rs — the aggressive coordinator's REFLEXES: automatic, instant reactions to what a fan
//! branch just produced, fired without a model round-trip and without waiting for the user.
//!
//! A reflex is deliberately small and mechanical — the coordinator's spinal cord, not its brain:
//! pattern → reaction, deterministic, explainable. The brain (the user, or a follow-up model turn)
//! stays in charge of everything a reflex queues; a reflex only ever ADDS a bounded follow-up or an
//! annotation, never cancels or rewrites work.
//!
//! Triggers (v1, each with a distinct reaction):
//!   * CompileError   — the reply contains rustc-style errors → queue ONE fix follow-up that quotes
//!                      the first error lines back at the branch.
//!   * TestFailure    — "test result: FAILED" / "panicked at" → queue ONE follow-up asking for the
//!                      failing case to be fixed and the suite re-run.
//!   * EmptyReply     — effectively no content → queue ONE retry follow-up (the cheapest reflex).
//!   * DuplicateOf    — near-identical to an earlier SIBLING (token-set Jaccard ≥ threshold) →
//!                      annotate only (a duplicate isn't wrong; it's just not adding variation) and
//!                      suggest picking between them.
//!
//! Every reflex is bounded: the caller enforces at-most-one follow-up per branch, and follow-ups
//! inherit the fan's hard queue cap. Detection is pure over strings so all of it tests headless.

#[derive(Clone, Debug, PartialEq)]
pub enum Reflex {
    CompileError { first_lines: String },
    TestFailure { first_line: String },
    EmptyReply,
    DuplicateOf { sibling: usize, jaccard: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reaction {
    /// Queue this prompt as a follow-up branch under the same parent.
    FollowUp { prompt: String, why: &'static str },
    /// Just annotate — visible in the liaison strip and the branch note.
    Annotate { note: String },
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Below this many non-whitespace chars a reply counts as empty.
    pub min_chars: usize,
    /// Token-set Jaccard at/above which two siblings are duplicates.
    pub dup_jaccard: f32,
}
impl Default for Config {
    fn default() -> Self { Config { min_chars: 24, dup_jaccard: 0.90 } }
}

fn token_set(s: &str) -> std::collections::BTreeSet<&str> {
    s.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|t| t.len() > 2).collect()
}

pub fn jaccard(a: &str, b: &str) -> f32 {
    let (sa, sb) = (token_set(a), token_set(b));
    if sa.is_empty() && sb.is_empty() { return 1.0; }
    let inter = sa.intersection(&sb).count() as f32;
    let union = sa.union(&sb).count() as f32;
    if union == 0.0 { 0.0 } else { inter / union }
}

/// Scan one finished branch reply against its earlier siblings. Order of checks is severity:
/// an empty reply reports only EmptyReply (nothing else is meaningful on no content); errors and
/// failures can co-occur; duplicate is checked last and only when the reply has content.
pub fn scan(reply: &str, earlier_siblings: &[&str], cfg: Config) -> Vec<Reflex> {
    let mut out = Vec::new();
    let trimmed: String = reply.chars().filter(|c| !c.is_whitespace()).collect();
    if trimmed.len() < cfg.min_chars { return vec![Reflex::EmptyReply]; }
    if let Some(pos) = reply.find("error[").or_else(|| reply.find("\nerror:")) {
        let first_lines: String = reply[pos..].lines().take(3).collect::<Vec<_>>().join("\n");
        out.push(Reflex::CompileError { first_lines });
    }
    if let Some(pos) = reply.find("test result: FAILED").or_else(|| reply.find("panicked at")) {
        let first_line = reply[pos..].lines().next().unwrap_or("").to_string();
        out.push(Reflex::TestFailure { first_line });
    }
    for (i, s) in earlier_siblings.iter().enumerate() {
        let j = jaccard(reply, s);
        if j >= cfg.dup_jaccard {
            out.push(Reflex::DuplicateOf { sibling: i, jaccard: j });
            break; // one duplicate note is enough
        }
    }
    out
}

/// The reaction a reflex implies. Follow-up prompts quote the evidence so the branch fixes THE
/// problem, not a paraphrase of it.
pub fn reaction(r: &Reflex) -> Reaction {
    match r {
        Reflex::CompileError { first_lines } => Reaction::FollowUp {
            prompt: format!(
                "Your previous attempt does not compile. Fix exactly these errors and return the \
corrected code in full:\n\n{first_lines}"),
            why: "compile error",
        },
        Reflex::TestFailure { first_line } => Reaction::FollowUp {
            prompt: format!(
                "Your previous attempt fails its tests ({first_line}). Fix the failing case and \
show the corrected code; state which case failed and why."),
            why: "test failure",
        },
        Reflex::EmptyReply => Reaction::FollowUp {
            prompt: "Your previous reply was empty. Answer the task in full this time.".into(),
            why: "empty reply",
        },
        Reflex::DuplicateOf { sibling, jaccard } => Reaction::Annotate {
            note: format!("near-duplicate of branch {} ({:.0}% token overlap) — consider :pick",
                          sibling + 1, jaccard * 100.0),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compile_error_becomes_a_quoting_followup() {
        let r = scan("fn x() {}\nerror[E0308]: mismatched types\n --> src/a.rs:3:5\nexpected u32", &[], Config::default());
        assert!(matches!(r.as_slice(), [Reflex::CompileError { .. }]));
        match reaction(&r[0]) {
            Reaction::FollowUp { prompt, why } => {
                assert_eq!(why, "compile error");
                assert!(prompt.contains("E0308"), "quotes the evidence: {prompt}");
            }
            other => panic!("wanted follow-up, got {other:?}"),
        }
    }
    #[test]
    fn empty_reply_shortcuts_everything_else() {
        let r = scan("  \n ok \n", &["something long enough here to matter"], Config::default());
        assert_eq!(r, vec![Reflex::EmptyReply]);
    }
    #[test]
    fn duplicates_annotate_but_never_queue() {
        let a = "The quick brown fox implements the parser with recursive descent and error recovery.";
        let r = scan(a, &[a], Config::default());
        assert!(matches!(r.as_slice(), [Reflex::DuplicateOf { sibling: 0, .. }]));
        assert!(matches!(reaction(&r[0]), Reaction::Annotate { .. }));
    }
    #[test]
    fn distinct_siblings_do_not_trip_the_duplicate_reflex() {
        let r = scan(
            "Approach one: build an index over the tokens and binary search each query in it.",
            &["Approach two: stream the file once keeping a rolling hash window per pattern."],
            Config::default());
        assert!(r.is_empty(), "{r:?}");
    }
    #[test]
    fn test_failure_detected_and_quoted() {
        let r = scan("running 3 tests\ntest result: FAILED. 1 passed; 2 failed", &[], Config::default());
        assert!(matches!(r.as_slice(), [Reflex::TestFailure { .. }]));
    }
}
