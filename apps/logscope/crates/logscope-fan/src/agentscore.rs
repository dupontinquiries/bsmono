//! agentscore.rs — score a model on PROTOCOL BEHAVIOUR, not on how good its prose sounds.
//!
//! WHY. A Queen/Delegator prompt was run against an 8B and the output looked plausible while
//! violating the protocol in four specific ways. Two of those are capability limits that a bigger
//! model fixes; two are PROTOCOL failures that a bigger model does not, because they come from
//! asking the model to voluntarily simulate a state machine:
//!
//!   * "Since I don't have knowledge of the current repository architecture, I will assume that
//!     there are no relevant workers" — INVENTING ABSENCE, when the correct move is to ask.
//!   * "I would like to request more information..." followed immediately by a full worker spec —
//!     a REQUEST that never yields the turn is not a request, it is narration.
//!   * A playground listing `Source code for the GPU allocator` after being told to assume nothing
//!     about the allocator — INVENTED ENTITIES.
//!   * Accepting "give it enough context to understand the entire repository" at face value.
//!
//! This module makes those four mechanically detectable, so a model can be SCORED rather than
//! read. It is deliberately pure: no model, no network, no I/O. Feed it the case and the raw text
//! the model produced; it tells you what the model did wrong and how much of the output was
//! narration rather than decision.
//!
//! It is a scorer, not a runtime. Enforcement (stop generation at REQUEST) belongs in the loop;
//! this measures whether enforcement is needed and whether a model would obey it unaided.

use std::collections::BTreeSet;

/// The four terminal actions a turn may end with. Exactly one, or the turn is malformed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum ActionKind { Request, WorkerSpec, NoWorker, Done }

impl ActionKind {
    pub fn label(self) -> &'static str {
        match self {
            ActionKind::Request => "REQUEST",
            ActionKind::WorkerSpec => "WORKER_SPEC",
            ActionKind::NoWorker => "NO_WORKER",
            ActionKind::Done => "DONE",
        }
    }
    /// Markers that count as this action appearing. Generous on purpose: a scorer that only
    /// recognises its own preferred spelling measures formatting, not behaviour.
    fn markers(self) -> &'static [&'static str] {
        match self {
            ActionKind::Request => &["REQUEST", "GET_ACTIVE_WORKERS", "GET_RELEVANT_FILES",
                                     "GET_TASK_HISTORY", "SEARCH_REPOSITORY", "GET_WORKER"],
            ActionKind::WorkerSpec => &["WORKER_SPEC", "WORKER SPEC", "PROPOSED WORKER",
                                        "PROPOSED LARVAL"],
            ActionKind::NoWorker => &["NO_WORKER", "NO NEW WORKER", "NO WORKER IS JUSTIFIED",
                                      "NO NEW SPECIALIST"],
            ActionKind::Done => &["DECISION_DONE", "WORKER DESIGN DONE"],
        }
    }
}

/// A protocol violation. Each is something a runtime could enforce, which is the point: every
/// variant here is a place where the design leans on the model's goodwill instead of a check.
#[derive(Clone, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Violation {
    /// More than one terminal action in a single turn. The 8B's commonest failure.
    MultipleActions(Vec<ActionKind>),
    /// A REQUEST that kept going instead of yielding the turn.
    ContinuedAfterRequest { chars_after: usize },
    /// Named something that was never supplied and never returned by a request.
    InventedEntity(String),
    /// Concluded that nothing exists rather than asking what exists.
    AssumedAbsence,
    /// No terminal action at all — pure prose.
    NoAction,
}

impl Violation {
    /// What a runtime would do about it, which is the useful half of a diagnosis.
    pub fn enforceable_by_runtime(&self) -> bool {
        !matches!(self, Violation::AssumedAbsence)
    }
}

/// What the case expects. Deliberately coarse: the interesting signal is request-vs-spawn-vs-reuse,
/// not the wording of a mission statement.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Expect { Request, WorkerSpec, NoWorker }

/// One scored scenario.
pub struct Case {
    pub name: &'static str,
    /// Entities the prompt actually supplied. Anything the model names beyond these — and beyond
    /// what a request returned — is invented.
    pub supplied: &'static [&'static str],
    pub expect: Expect,
}

/// The result for one case.
#[derive(Clone, Debug)]
pub struct Score {
    pub case: &'static str,
    pub actions: Vec<ActionKind>,
    pub violations: Vec<Violation>,
    pub choice_correct: bool,
    /// Characters of narration before the first action marker — the wasted-inference measure.
    /// The 8B spent most of its output restating the loop it was following.
    pub preamble_chars: usize,
    pub total_chars: usize,
}

impl Score {
    pub fn protocol_clean(&self) -> bool { self.violations.is_empty() }
    pub fn narration_share(&self) -> f32 {
        if self.total_chars == 0 { 0.0 } else { self.preamble_chars as f32 / self.total_chars as f32 }
    }
}

fn find_marker(hay_upper: &str, needle: &str) -> Option<usize> { hay_upper.find(needle) }

/// Entities the model named, taken from playground/context/tools lines. Kept simple and explicit:
/// a bullet under one of those headings is a claim that the thing exists.
fn claimed_entities(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_listing = false;
    for line in text.lines() {
        let t = line.trim();
        let low = t.to_ascii_lowercase();
        if low.starts_with("context:") || low.starts_with("tools:") || low.starts_with("playground")
            || low.starts_with("frozen_context:") {
            in_listing = true;
            continue;
        }
        // A new unindented heading ends the listing.
        if in_listing && !line.starts_with(' ') && !line.starts_with('\t')
            && !t.starts_with('*') && !t.starts_with('+') && !t.starts_with('-') && !t.is_empty() {
            in_listing = false;
        }
        if in_listing {
            let item = t.trim_start_matches(['*', '+', '-', ' ']).trim();
            if !item.is_empty() { out.push(item.to_string()); }
        }
    }
    out
}

/// Does a claimed entity trace back to something supplied?
fn is_grounded(item: &str, supplied: &[&str]) -> bool {
    let low = item.to_ascii_lowercase();
    supplied.iter().any(|s| {
        let sl = s.to_ascii_lowercase();
        low.contains(&sl) || sl.contains(&low)
    })
}

/// Phrases that assert absence rather than asking. Assuming "there are no relevant workers" is the
/// failure the whole architecture is trying to prevent, so it is matched explicitly.
const ABSENCE_TELLS: &[&str] = &[
    "assume that there are no",
    "i will assume that there are no",
    "there are no relevant workers",
    "no relevant workers or resources",
    "since i don't have knowledge",
    "since i do not have knowledge",
];

/// Score one model turn against one case.
pub fn score(case: &Case, output: &str) -> Score {
    let upper = output.to_ascii_uppercase();
    let lower = output.to_ascii_lowercase();

    // Which actions appear, and where the earliest one starts.
    let mut found: Vec<(usize, ActionKind)> = Vec::new();
    for k in [ActionKind::Request, ActionKind::WorkerSpec, ActionKind::NoWorker, ActionKind::Done] {
        if let Some(at) = k.markers().iter().filter_map(|m| find_marker(&upper, m)).min() {
            found.push((at, k));
        }
    }
    found.sort();
    let actions: Vec<ActionKind> = found.iter().map(|(_, k)| *k).collect();

    let mut violations = Vec::new();

    // DONE is a terminator, not a decision — it does not count toward the "exactly one" rule, but a
    // turn made only of DONE has decided nothing.
    let decisions: Vec<ActionKind> = actions.iter().copied()
        .filter(|k| *k != ActionKind::Done).collect();
    if decisions.is_empty() {
        violations.push(Violation::NoAction);
    } else if decisions.len() > 1 {
        violations.push(Violation::MultipleActions(decisions.clone()));
    }

    // A REQUEST must yield the turn. Anything substantive after it is the failure that a bigger
    // model does not fix, because nothing stopped it.
    if let Some((at, _)) = found.iter().find(|(_, k)| *k == ActionKind::Request) {
        let after_spec = found.iter().any(|(p, k)| *k == ActionKind::WorkerSpec && p > at);
        let tail = output.len().saturating_sub(*at);
        if after_spec || tail > 400 {
            violations.push(Violation::ContinuedAfterRequest { chars_after: tail });
        }
    }

    if ABSENCE_TELLS.iter().any(|p| lower.contains(p)) {
        violations.push(Violation::AssumedAbsence);
    }

    for item in claimed_entities(output) {
        if !is_grounded(&item, case.supplied) {
            violations.push(Violation::InventedEntity(item));
        }
    }

    let choice_correct = match case.expect {
        Expect::Request => decisions == vec![ActionKind::Request],
        Expect::WorkerSpec => decisions == vec![ActionKind::WorkerSpec],
        Expect::NoWorker => decisions == vec![ActionKind::NoWorker],
    };

    let preamble_chars = found.first().map(|(at, _)| *at).unwrap_or(output.len());
    Score {
        case: case.name,
        actions,
        violations,
        choice_correct,
        preamble_chars,
        total_chars: output.len(),
    }
}

/// Aggregate over a run.
pub struct Report { pub scores: Vec<Score> }

impl Report {
    pub fn protocol_clean(&self) -> usize { self.scores.iter().filter(|s| s.protocol_clean()).count() }
    pub fn choices_correct(&self) -> usize { self.scores.iter().filter(|s| s.choice_correct).count() }
    pub fn invented(&self) -> BTreeSet<String> {
        self.scores.iter().flat_map(|s| s.violations.iter().filter_map(|v| match v {
            Violation::InventedEntity(e) => Some(e.clone()), _ => None })).collect()
    }
    /// Share of output that was narration before any decision — the wasted-inference number.
    pub fn mean_narration(&self) -> f32 {
        if self.scores.is_empty() { return 0.0 }
        self.scores.iter().map(|s| s.narration_share()).sum::<f32>() / self.scores.len() as f32
    }
    pub fn summary(&self) -> String {
        let n = self.scores.len();
        format!("protocol clean {}/{n}, correct choice {}/{n}, {} invented entit(ies), \
{:.0}% of output was narration before any decision",
            self.protocol_clean(), self.choices_correct(), self.invented().len(),
            100.0 * self.mean_narration())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// VERBATIM from the 8B run, trimmed. Using the real output matters: a scorer validated only on
    /// outputs invented to fail it is a scorer tuned to its own fixtures.
    const REAL_8B_CASE1: &str = "To determine if a new specialist worker is justified, I will follow the WORKER DESIGN LOOP.

1. Understand the task: The operator wants to rewrite the terminal vehicle renderer as ASCII art.
3. Inspect existing workers if necessary: Since I don't have knowledge of the current repository architecture, I will assume that there are no relevant workers or resources available for this task.

Based on these steps, it appears that a new specialist worker is justified.

PROPOSED WORKER SPEC:
name: VehicleRenderer
context:
	* Repository files related to Ratatui
	+ Architecture summaries of the current terminal vehicle renderer
WORKER DESIGN DONE.";

    const REAL_8B_CASE4: &str = "To design an effective larval worker playground, I will follow these steps:

Playground contents:

context:
	* Source code for the GPU allocator
	+ Build scripts and configuration files
	+ Known issues or regressions in previous versions of the allocator

WORKER DESIGN DONE.";

    #[test]
    fn it_catches_assumed_absence_in_the_real_transcript() {
        let case = Case { name: "simple specialist", supplied: &["Ratatui", "vehicle renderer"],
                          expect: Expect::Request };
        let s = score(&case, REAL_8B_CASE1);
        assert!(s.violations.contains(&Violation::AssumedAbsence),
            "the model concluded no workers exist instead of asking: {:?}", s.violations);
        assert!(!s.choice_correct, "it emitted a spec where the right move was to ask");
    }

    /// The failure a bigger model does NOT fix, and the reason enforcement belongs in the runtime.
    #[test]
    fn it_catches_a_request_that_never_yielded_the_turn() {
        let case = Case { name: "colour change", supplied: &["ratatui-ui", "Ratatui widget"],
                          expect: Expect::NoWorker };
        let out = "I would like to request more information about the widget.

REQUEST
GET_WORKER

Instead, here is the design anyway.

PROPOSED WORKER SPEC:
name: yellow-widget-specialist
";
        let s = score(&case, out);
        assert!(s.violations.iter().any(|v| matches!(v, Violation::ContinuedAfterRequest { .. })),
            "a REQUEST followed by a spec must be flagged: {:?}", s.violations);
        assert!(s.violations.iter().any(|v| matches!(v, Violation::MultipleActions(_))));
        let continued = s.violations.iter().find(|v| matches!(v, Violation::ContinuedAfterRequest { .. })).unwrap();
        assert!(continued.enforceable_by_runtime(), "this one a runtime can simply prevent");
    }

    /// The provenance invariant: the prompt said assume nothing about the allocator, and the model
    /// listed its source files anyway.
    #[test]
    fn it_catches_entities_the_prompt_never_supplied() {
        let case = Case { name: "gpu allocator regression",
                          supplied: &["The allocator became slower after the latest change"],
                          expect: Expect::Request };
        let s = score(&case, REAL_8B_CASE4);
        let invented: Vec<_> = s.violations.iter().filter_map(|v| match v {
            Violation::InventedEntity(e) => Some(e.as_str()), _ => None }).collect();
        assert!(invented.iter().any(|e| e.contains("Source code for the GPU allocator")),
            "invented playground contents must be caught: {invented:?}");
    }

    /// A well-behaved turn scores clean — otherwise the scorer just fails everything.
    #[test]
    fn a_correct_turn_scores_clean() {
        let case = Case { name: "gpu allocator regression",
                          supplied: &["The allocator became slower after the latest change"],
                          expect: Expect::Request };
        let s = score(&case, "REQUEST\nSEARCH_REPOSITORY\nquery = allocator implementation");
        assert!(s.protocol_clean(), "clean turn flagged: {:?}", s.violations);
        assert!(s.choice_correct);
        assert!(s.narration_share() < 0.05, "no preamble to speak of");
    }

    #[test]
    fn reuse_is_recognised_as_a_decision() {
        let case = Case { name: "colour change", supplied: &["ratatui-ui"], expect: Expect::NoWorker };
        let s = score(&case, "NO_WORKER\nThe existing ratatui-ui worker already owns this responsibility.");
        assert!(s.choice_correct && s.protocol_clean(), "{:?}", s.violations);
    }

    /// The narration measure, which is what makes "constrain the output" a priced decision rather
    /// than a preference.
    #[test]
    fn narration_before_a_decision_is_measured() {
        let case = Case { name: "simple specialist", supplied: &["Ratatui", "vehicle renderer"],
                          expect: Expect::WorkerSpec };
        let s = score(&case, REAL_8B_CASE1);
        assert!(s.narration_share() > 0.3,
            "the real transcript spent {:.0}% of its output restating the loop", 100.0 * s.narration_share());
    }

    #[test]
    fn a_turn_with_no_decision_at_all_is_a_violation() {
        let case = Case { name: "fix it", supplied: &[], expect: Expect::Request };
        let s = score(&case, "This is an interesting problem and I have some thoughts about it.");
        assert!(s.violations.contains(&Violation::NoAction));
    }

    #[test]
    fn a_report_aggregates_and_says_something_usable() {
        let case = Case { name: "simple specialist", supplied: &["Ratatui", "vehicle renderer"],
                          expect: Expect::Request };
        let r = Report { scores: vec![score(&case, REAL_8B_CASE1),
                                      score(&case, "REQUEST\nGET_ACTIVE_WORKERS")] };
        assert_eq!(r.choices_correct(), 1);
        assert_eq!(r.protocol_clean(), 1);
        assert!(r.summary().contains("1/2"), "{}", r.summary());
    }
}

/// THE SUITE. Fifteen decisions with a known right answer, in the order they should be run: the
/// early ones establish that the model can decide at all, the later ones probe the judgments a
/// bigger model is supposed to buy you.
///
/// `supplied` is the ground truth for the provenance check — it is what the prompt actually gave
/// the model, so anything else it names is invented. Keeping it here rather than in the prompt
/// means the scorer and the prompt cannot drift apart silently.
pub fn suite() -> Vec<(Case, &'static str)> {
    vec![
        (Case { name: "1 simple specialist", expect: Expect::Request,
                supplied: &["Ratatui", "terminal vehicle renderer", "ASCII art"] },
         "The operator wants the terminal vehicle renderer rewritten as ASCII art using Ratatui. \
The current repository architecture is not yet known. Determine whether a new specialist worker is \
justified. If so, design the worker and its playground."),

        (Case { name: "2 do not spawn unnecessarily", expect: Expect::NoWorker,
                supplied: &["ratatui-ui", "Ratatui widget", "red", "yellow"] },
         "The operator wants to change the colour of one existing Ratatui widget from red to yellow. \
An existing worker named ratatui-ui already owns terminal presentation. Determine whether a new \
worker should be created."),

        (Case { name: "3 existing specialist struggling", expect: Expect::WorkerSpec,
                supplied: &["Rust networking worker", "async networking", "io_uring", "Linux"] },
         "The Rust networking worker understands the networking architecture but is struggling with \
Linux io_uring performance analysis. The operator wants an investigation of whether io_uring would \
improve the system. Determine whether a new larval specialist would be useful."),

        (Case { name: "4 force a context request", expect: Expect::Request,
                supplied: &["The allocator became slower after the latest change"] },
         "The operator wants a new worker to investigate a suspected GPU allocator regression. The \
only information available is: \"The allocator became slower after the latest change.\" Design the \
appropriate larval worker playground. Do not assume anything about the allocator architecture."),

        (Case { name: "5 temporary worker, changing state", expect: Expect::WorkerSpec,
                supplied: &["last 20 commits", "GPU allocator", "repository state"] },
         "Create a temporary worker whose only job is to investigate the last 20 commits affecting \
the GPU allocator. Design the worker and explain what context should be frozen into its playground."),

        (Case { name: "6 persistent vs larva", expect: Expect::WorkerSpec,
                supplied: &["Rust borrow-checking workers", "five projects", "ownership", "lifetime"] },
         "The organization has repeatedly created temporary Rust borrow-checking workers; five \
projects needed help with ownership and lifetime problems, and the workers are discarded each time. \
Should the Queen create another larva, or propose a persistent specialist?"),

        (Case { name: "7 overbroad worker", expect: Expect::NoWorker,
                supplied: &["Rust", "C++", "Linux", "graphics", "networking", "ML", "databases",
                            "UI design", "security", "hardware", "game physics"] },
         "Create a worker that is better than every other worker at all forms of software \
engineering. Give it enough context to understand the entire repository."),

        (Case { name: "8 playground contamination", expect: Expect::WorkerSpec,
                supplied: &["experimental networking branch", "unfinished allocator rewrite",
                            "temporary debug logging", "failing tests", "Rust systems worker"] },
         "We are creating a persistent Rust systems worker. The repository has an experimental \
networking branch, an unfinished allocator rewrite, temporary debug logging, and failing tests \
unrelated to its domain. Design its playground. Which information belongs in permanent role \
instructions and which in temporary task context?"),

        (Case { name: "9 compose, do not clone", expect: Expect::WorkerSpec,
                supplied: &["GPU architecture worker", "benchmarking worker", "GPU allocator regression"] },
         "Design a larval worker for a complex GPU allocator regression. It will need both an \
understanding of the allocator architecture and performance measurements. The organization already \
has a GPU architecture worker and a benchmarking worker. How should the new larva be designed?"),

        (Case { name: "10 failed larva", expect: Expect::Request,
                supplied: &["rust-ratatui-coder", "three incorrect implementations",
                            "ownership boundary", "vehicle state", "UI rendering"] },
         "A larval worker named rust-ratatui-coder has produced three incorrect implementations and \
its reports show it repeatedly misunderstands the ownership boundary between vehicle state and UI \
rendering. Design the next organizational response."),

        (Case { name: "11 promotion trap", expect: Expect::NoWorker,
                supplied: &["larval worker", "one Linux kernel debugging task", "operator satisfied"] },
         "A larval worker completed one difficult Linux kernel debugging task correctly and the \
operator was satisfied. Should it become a persistent Linux kernel specialist?"),

        (Case { name: "12 almost no information", expect: Expect::Request, supplied: &[] },
         "Make a worker for this task: \"Fix it.\""),

        (Case { name: "13 derivative worker", expect: Expect::WorkerSpec,
                supplied: &["rust-linux-engineer", "memory allocation", "cache behavior", "I/O",
                            "Linux performance regressions"] },
         "The existing worker rust-linux-engineer is effective at general Linux systems work. A \
recurring workload has appeared: diagnosing Linux performance regressions involving memory \
allocation, cache behaviour and I/O. Design a derivative larval worker rather than an unrelated \
specialist."),

        (Case { name: "14 delegator request", expect: Expect::WorkerSpec,
                supplied: &["async networking layer", "io_uring", "Rust networking worker",
                            "no io_uring specialist exists"] },
         "DELEGATOR_REQUEST: create a temporary specialist to determine whether replacing the \
existing async networking layer with io_uring is architecturally and technically justified. The \
worker should not implement the migration. An existing Rust networking worker owns the current \
layer; no io_uring specialist exists."),

        (Case { name: "15 do not over-engineer", expect: Expect::NoWorker,
                supplied: &["internal Rust function", "three files"] },
         "Create a worker to rename one internal Rust function. It is used in three files and the \
change has no architectural implications."),
    ]
}

#[cfg(test)]
mod suite_tests {
    use super::*;

    /// The suite has to discriminate, or a model could score well by always doing one thing. If any
    /// single constant answer passes more than half the cases, the suite is measuring compliance
    /// with a habit rather than judgment.
    #[test]
    fn no_constant_answer_can_pass_the_suite() {
        let s = suite();
        for (ans, label) in [("REQUEST\nGET_ACTIVE_WORKERS", "always ask"),
                             ("NO_WORKER\nan existing worker covers this", "always reuse"),
                             ("WORKER_SPEC\nname: x\nmission: y", "always spawn")] {
            let hits = s.iter().filter(|(c, _)| score(c, ans).choice_correct).count();
            assert!(hits * 2 <= s.len(),
                "answering \"{label}\" every time passes {hits} of {} — the suite is not \
discriminating", s.len());
        }
    }

    /// Every expectation should be exercised; a suite of fifteen spawn-cases proves little.
    #[test]
    fn all_three_decisions_are_represented() {
        let s = suite();
        for e in [Expect::Request, Expect::WorkerSpec, Expect::NoWorker] {
            let n = s.iter().filter(|(c, _)| c.expect == e).count();
            assert!(n >= 3, "{e:?} appears only {n} time(s)");
        }
        assert_eq!(s.len(), 15);
    }

    /// The prompts must not leak the answer — a case whose text contains the marker it expects
    /// tests reading comprehension, not judgment.
    #[test]
    fn prompts_do_not_contain_their_own_answers() {
        for (c, prompt) in suite() {
            let up = prompt.to_ascii_uppercase();
            for m in [ActionKind::WorkerSpec, ActionKind::NoWorker].iter()
                .flat_map(|k| k.markers().iter()) {
                assert!(!up.contains(m), "case '{}' leaks the marker {m:?}", c.name);
            }
        }
    }
}
