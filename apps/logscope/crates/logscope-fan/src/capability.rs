//! capability — what a worker IS, rendered for the worker itself.
//!
//! ## The gap this fills
//!
//! `HiveMind_Strategy_Logscope.md` (not in the tree — the citation outlived
//! the file; `docs/11-hive-matrix.md` is the standing design) makes one
//! requirement mandatory:
//!
//! > a worker cannot infer its job merely from its model or its name. Every
//! > spawned worker needs a compact capability contract telling it what it
//! > _is_, what it _can do_, what it _cannot do_, what evidence it is expected
//! > to produce, and what transitions are available.
//!
//! Three things in the tree are adjacent and none of them is this:
//!
//! * [`crate::workerkit::Duty`] says where a worker may run — `reach()` is
//!   about round-trip cost, not competence.
//! * [`crate::engmode::prompt_for`] says what to do in the CURRENT phase. It is
//!   the same text whatever the worker is.
//! * `WORKER_CONTRACT.md` is one static brief for every worker at once, about
//!   lane discipline and gating.
//!
//! A worker therefore learns its phase and its manners, and never its job.
//!
//! ## Sections drop out
//!
//! The doc's other ask, first line:
//!
//! > Make some parts dropout on ques like if no code then don't say compiler
//! > results or make it a tinier section
//!
//! That is not cosmetic. A contract that demands compiler evidence from a
//! worker with no code teaches it to fabricate the section, and a contract
//! listing capabilities it has no way to use spends context on noise. Every
//! section here is conditional on the [`Ctx`] it is rendered against, and the
//! tests assert ABSENCE as well as presence — a renderer that always emits
//! everything would pass a presence-only suite.
//!
//! Pure: a `Duty` and a `Ctx` in, a string out. No clock, no IO, no model.

use crate::workerkit::Duty;

/// What is actually available to this worker. Every field turns a section on
/// or off; nothing here is advisory.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ctx {
    /// The lane contains source the worker may edit.
    pub has_code: bool,
    /// A compiler or test command can be run and its output read.
    pub has_compiler: bool,
    /// The repository is readable beyond the lane (for context, not editing).
    pub has_repo: bool,
    /// Other workers exist whose reports can be requested.
    pub peers: usize,
    /// The worker may ask the operator a question and wait.
    pub can_ask_operator: bool,
    /// Rounds remaining, when the coordinator bounds them.
    pub rounds_left: Option<usize>,
}

impl Duty {
    /// One line: what this worker is for.
    pub fn mission(self) -> &'static str {
        match self {
            Duty::Thinker => "Reason a problem through to a conclusion in one pass and state it.",
            Duty::FanOutBranch => "Produce ONE independent attempt at the task, to be compared \
against siblings. Do not converge toward what you think the others will say.",
            Duty::SpeculativeDraft => "Propose the next few tokens cheaply. Being wrong costs one \
verification slot; being slow costs the whole speedup.",
            Duty::Liaison => "Turn what the operator said into what the machinery needs, and turn \
what the machinery reported into what the operator asked about.",
            Duty::Checker => "Return one verdict on one artifact, with the evidence that produced \
it. You are not here to improve the artifact.",
            Duty::Build => "Get the tree to a green gate and report the exact command and result.",
        }
    }

    /// What this duty may do, before context narrows it.
    fn can(self) -> &'static [&'static str] {
        match self {
            Duty::Thinker => &["reason at length", "propose an approach", "name what you do not know"],
            Duty::FanOutBranch => &["read the lane", "write in the lane", "choose your own approach"],
            Duty::SpeculativeDraft => &["read the committed context", "propose a continuation"],
            Duty::Liaison => &["read reports", "summarise", "ask the operator", "route a request"],
            Duty::Checker => &["read the artifact", "run the stated check", "return a verdict"],
            Duty::Build => &["edit build files", "run the build", "report failures verbatim"],
        }
    }

    /// What this duty must NOT do. The doc is explicit that this half is
    /// mandatory: a capability list alone reads as permission for anything
    /// unlisted.
    fn cannot(self) -> &'static [&'static str] {
        match self {
            Duty::Thinker => &["edit files", "claim a result you did not compute"],
            Duty::FanOutBranch => &["edit outside your lane", "coordinate with siblings",
                                    "wire your work into shared registries"],
            Duty::SpeculativeDraft => &["take more than a moment", "emit a final answer"],
            Duty::Liaison => &["invent a report nobody sent", "decide the task on the operator's behalf"],
            Duty::Checker => &["fix what you are checking", "pass an artifact you did not run"],
            Duty::Build => &["change program logic to make a build pass"],
        }
    }
}

/// Render the contract this worker should be handed.
///
/// Sections absent from `ctx` are omitted entirely rather than emitted empty —
/// an empty heading is still an instruction to fill it.
pub fn profile(duty: Duty, ctx: &Ctx) -> String {
    let mut s = String::new();
    s.push_str(&format!("ROLE: {duty:?}\nREACH: {:?}\nMISSION: {}\n", duty.reach(), duty.mission()));

    s.push_str("\nYOU CAN:\n");
    for c in duty.can() {
        // A capability that needs code is noise without it.
        if !ctx.has_code && (c.contains("lane") || c.contains("write")) { continue }
        s.push_str(&format!("  - {c}\n"));
    }

    // Requests are only listed when something could answer them. Listing
    // `worker_report` to a lone worker invites it to wait for a reply that
    // cannot arrive.
    let mut req: Vec<&str> = Vec::new();
    if ctx.has_repo { req.push("repository_context"); }
    if ctx.peers > 0 { req.push("worker_report"); }
    if ctx.has_compiler { req.push("compiler_output"); }
    if ctx.can_ask_operator { req.push("operator_clarification"); }
    if !req.is_empty() {
        s.push_str("\nYOU CAN REQUEST:\n");
        for r in &req { s.push_str(&format!("  - {r}\n")); }
    }

    s.push_str("\nYOU CANNOT:\n");
    for c in duty.cannot() { s.push_str(&format!("  - {c}\n")); }

    // EVIDENCE — the section the doc singles out. Demanding compiler results
    // from a worker that has no compiler is how a contract teaches fabrication.
    s.push_str("\nEVIDENCE EXPECTED:\n");
    if ctx.has_compiler {
        s.push_str("  - the exact check command you ran, and its result verbatim\n");
        s.push_str("  - test counts as \"N passed / M failed\"\n");
    } else if ctx.has_code {
        s.push_str("  - state UNVERIFIED: no compiler is available to you. Do not describe \
results you did not obtain.\n");
    } else {
        s.push_str("  - your reasoning, and what you could not determine\n");
    }

    if let Some(n) = ctx.rounds_left {
        s.push_str(&format!("\nROUNDS LEFT: {n}\n"));
        if n <= 1 {
            s.push_str("  This is your last. Report what you have rather than starting work you \
cannot finish.\n");
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Ctx {
        Ctx { has_code: true, has_compiler: true, has_repo: true, peers: 3,
              can_ask_operator: true, rounds_left: Some(5) }
    }

    #[test]
    fn a_worker_is_told_its_role_mission_and_limits() {
        let p = profile(Duty::FanOutBranch, &full());
        assert!(p.contains("ROLE: FanOutBranch"));
        assert!(p.contains("MISSION:"));
        assert!(p.contains("YOU CANNOT:"), "the prohibitions half is mandatory: {p}");
        assert!(p.contains("coordinate with siblings"));
    }

    /// The doc's own example, and the reason this module exists.
    #[test]
    fn no_compiler_means_no_compiler_evidence_section() {
        let ctx = Ctx { has_code: true, has_compiler: false, ..full() };
        let p = profile(Duty::Build, &ctx);
        assert!(!p.contains("check command"),
                "a worker with no compiler was asked for the command it ran: {p}");
        assert!(!p.contains("N passed"),
                "and for test counts it cannot produce: {p}");
        assert!(p.contains("UNVERIFIED"),
                "it must be told to say so instead: {p}");
    }

    #[test]
    fn no_code_means_no_code_capabilities_and_a_reasoning_evidence_bar() {
        let ctx = Ctx { has_code: false, has_compiler: false, ..full() };
        let p = profile(Duty::Thinker, &ctx);
        assert!(!p.contains("write in the lane"));
        assert!(!p.contains("UNVERIFIED"), "UNVERIFIED is for code without a compiler: {p}");
        assert!(p.contains("your reasoning"));
    }

    #[test]
    fn a_lone_worker_is_not_offered_peer_reports() {
        let ctx = Ctx { peers: 0, ..full() };
        let p = profile(Duty::Thinker, &ctx);
        assert!(!p.contains("worker_report"),
                "offering a report request with no peers invites a wait that cannot end: {p}");
        assert!(p.contains("repository_context"), "the other requests must survive: {p}");
    }

    #[test]
    fn a_worker_with_nothing_available_gets_no_request_section_at_all() {
        let ctx = Ctx::default();
        let p = profile(Duty::SpeculativeDraft, &ctx);
        assert!(!p.contains("YOU CAN REQUEST"),
                "an empty heading is still an instruction to fill it: {p}");
    }

    #[test]
    fn the_last_round_is_said_plainly() {
        let p = profile(Duty::Thinker, &Ctx { rounds_left: Some(1), ..full() });
        assert!(p.contains("This is your last"));
        let p5 = profile(Duty::Thinker, &Ctx { rounds_left: Some(5), ..full() });
        assert!(!p5.contains("This is your last"));
        let pnone = profile(Duty::Thinker, &Ctx { rounds_left: None, ..full() });
        assert!(!pnone.contains("ROUNDS LEFT"), "an unbounded run must not claim a bound");
    }

    /// Dropout has to actually shrink the thing, or it is decoration.
    ///
    /// Measured: 404 bytes against 615, a 34% reduction. I first asserted >50%
    /// and it failed at 34% — the threshold was a guess and the code was right.
    /// Roughly two thirds of a contract is IRREDUCIBLE by design: role, reach,
    /// mission and the prohibitions apply to every worker in every context, and
    /// the prohibitions are the half the strategy doc calls mandatory. What
    /// drops is the conditional third — requests nothing can answer, capabilities
    /// needing files that are absent, and evidence bars for tools not present.
    ///
    /// Pinned as a RANGE. Below 15% the conditionals have stopped working;
    /// above 60% something that should always be said has become optional.
    #[test]
    fn a_narrow_context_produces_a_materially_smaller_contract() {
        let big = profile(Duty::FanOutBranch, &full());
        let small = profile(Duty::FanOutBranch, &Ctx::default());
        let saved = 1.0 - (small.len() as f64 / big.len() as f64);
        assert!(
            (0.15..=0.60).contains(&saved),
            "dropout saved {:.0}% ({} vs {} bytes) — under 15% the conditionals are not firing, \
over 60% something unconditional has become optional",
            saved * 100.0, small.len(), big.len()
        );
    }

    #[test]
    fn every_duty_renders_and_names_a_prohibition() {
        for d in [Duty::Thinker, Duty::FanOutBranch, Duty::SpeculativeDraft,
                  Duty::Liaison, Duty::Checker, Duty::Build] {
            let p = profile(d, &full());
            assert!(p.contains("MISSION:"), "{d:?} has no mission");
            assert!(!d.cannot().is_empty(), "{d:?} lists no prohibitions");
            assert!(p.contains(d.cannot()[0]), "{d:?} prohibition missing from render");
        }
    }
}
