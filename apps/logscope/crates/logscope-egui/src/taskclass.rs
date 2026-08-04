//! taskclass.rs — classify an incoming request so the runtime knows HOW to approach it, and (critically)
//! so a maintenance-tuned system prompt doesn't refuse a legitimate greenfield build. Pure + tested.
//!
//! The failure this fixes (from a real transcript): a system prompt written for CODEBASE MAINTENANCE
//! ("minimal edit distance, preserve architecture, don't start new projects") made the model REFUSE
//! "write a 3D game", repeatedly, citing its role. The refusal has a kind of integrity — the model is
//! grounded, honest, not hallucinating — but the humility calcified into rigidity: it treated "minimal
//! edit" as a reason to DECLINE rather than as a description of HOW to build.
//!
//! The reframe: humility should govern EXECUTION, never ACCEPTANCE of a goal the user explicitly asked for.
//! "Minimal edit distance" on a greenfield task doesn't mean "refuse" — it means "build the smallest thing
//! that compiles and runs, then grow it one verified step at a time." Same humble, incremental, honest
//! posture; opposite conclusion (build it, don't refuse it).
//!
//! So this classifies on two axes and emits a POSTURE addendum that channels the model's humility into the
//! right behavior for the task:
//!   * HORIZON  — one-shot (answer/produce directly) vs long-horizon (decompose → orchestrate).
//!   * DOMAIN   — maintenance (edit existing code; minimal-edit applies literally) vs greenfield (build new;
//!                minimal-edit becomes "smallest working version first").
//! The long-horizon greenfield quadrant is the one the transcript refused; the classifier routes it INTO
//! the build/decompose machinery with a humility-preserving posture instead of a refusal.

/// Is this a quick, self-contained request or a multi-step build?
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Horizon { OneShot, LongHorizon }

/// Is the work on EXISTING code, or building something NEW?
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Domain { Maintenance, Greenfield }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TaskClass {
    pub horizon: Horizon,
    pub domain: Domain,
}

impl TaskClass {
    /// The quadrant the transcript mishandled: a big new build. This is where a maintenance prompt tends to
    /// refuse, and where the runtime should instead enter decompose/build mode.
    pub fn is_long_greenfield(&self) -> bool {
        self.horizon == Horizon::LongHorizon && self.domain == Domain::Greenfield
    }
    /// Should the runtime enter the orchestration loop (decompose → subtasks → verify → merge) rather than
    /// answering in one shot?
    pub fn wants_orchestration(&self) -> bool { self.horizon == Horizon::LongHorizon }
}

/// Words that signal BUILDING SOMETHING NEW (greenfield) rather than editing existing code.
/// Build verbs with no article, for named products: "write smite 2", "build tetris", "make doom".
///
/// Weaker than GREENFIELD_VERBS by construction — they only count when nothing maintenance-shaped is
/// present, since "write", "build" and "implement" all appear in maintenance requests too.
const BARE_BUILD_VERBS: &[&str] = &[
    "write ", "build ", "create ", "make ", "implement ", "clone ", "port ", "recreate ",
];

const GREENFIELD_VERBS: &[&str] = &[
    "write a", "build a", "build me", "create a", "make a", "implement a", "develop a", "design a",
    "write an", "build an", "create an", "make an", "implement an", "from scratch", "new project",
    "new app", "new game", "prototype a", "scaffold",
];
/// Words that signal work on EXISTING code (maintenance).
const MAINTENANCE_VERBS: &[&str] = &[
    "fix", "debug", "refactor", "the bug", "this error", "why does", "why is", "optimize the",
    "in this code", "in the codebase", "existing", "the crash", "stack trace", "this function",
    "review this", "explain this", "what does this", "the log", "signal 11", "segfault",
];
/// Words that signal SCALE (long-horizon): whole systems, multiple subsystems, explicit stepwise asks.
const SCALE_WORDS: &[&str] = &[
    "game", "engine", "compiler", "database", "framework", "3d", "multiplayer", "networking",
    "stepwise", "step by step", "full", "entire", "complete", "whole", "end to end", "pipeline",
    "system", "platform", "suite", "from scratch",
];
/// Words that signal a SMALL, one-shot ask.
const SMALL_WORDS: &[&str] = &[
    "quick", "small", "simple", "a function", "one function", "a snippet", "a regex", "a one-liner",
    "just a", "tiny", "a helper", "a script to", "rename", "typo",
];

/// Classify a request. Deliberately conservative on DOMAIN — a request only reads as greenfield when it
/// carries an explicit build verb AND doesn't reference existing code, so a maintenance task is never
/// misrouted into "build a new thing" (the costly error direction). Horizon leans long when scale words are
/// present or the ask is a build, short when small-signals dominate.
pub fn classify(request: &str) -> TaskClass {
    let r = request.to_ascii_lowercase();
    let mut greenfield_hits = GREENFIELD_VERBS.iter().filter(|w| r.contains(**w)).count();
    let maintenance_hits = MAINTENANCE_VERBS.iter().filter(|w| r.contains(**w)).count();
    // ARTICLE-FREE BUILD VERBS. Every entry in GREENFIELD_VERBS carries an article — "write a",
    // "build a", "create a" — and a NAMED PRODUCT does not take one. "write smite 2 in rust and
    // bevy" scored zero greenfield hits and was classified as maintenance, so the model got the
    // preserve-architecture posture for a project that did not exist and recited "minimal edit
    // distance" at a blank directory.
    //
    // GREENFIELD_BUILD_POSTURE was written for exactly that and never fired. A posture that exists
    // and does not trigger is indistinguishable from one nobody wrote.
    //
    // Counted only when NO maintenance verb is present, because a bare "write" appears in "write the
    // fix for this bug" too, and the tie-break favours greenfield — so a weak signal has to be
    // weaker, not merely equal.
    if maintenance_hits == 0 {
        greenfield_hits += BARE_BUILD_VERBS.iter().filter(|w| r.contains(**w)).count();
    }
    let scale_hits = SCALE_WORDS.iter().filter(|w| r.contains(**w)).count();
    let small_hits = SMALL_WORDS.iter().filter(|w| r.contains(**w)).count();

    // DOMAIN: greenfield only when a build verb is present and maintenance signals don't dominate. Ties and
    // ambiguity default to Maintenance (the conservative, minimal-edit posture — never invent a new project
    // for a request that might be about existing code).
    let domain = if greenfield_hits > 0 && greenfield_hits >= maintenance_hits {
        Domain::Greenfield
    } else {
        Domain::Maintenance
    };

    // HORIZON: long when scale words appear or it's a build with no explicit small-signal; short when small
    // signals clearly dominate. A greenfield build defaults long unless it's explicitly small.
    let horizon = if small_hits > scale_hits && small_hits > 0 {
        Horizon::OneShot
    } else if scale_hits > 0 || (domain == Domain::Greenfield && small_hits == 0) {
        Horizon::LongHorizon
    } else {
        Horizon::OneShot
    };

    TaskClass { horizon, domain }
}

/// The posture addendum for a classified task — appended to the system prompt for THIS turn so the model's
/// behavior matches the task without rewriting the user's identity prompt. For the long-horizon-greenfield
/// quadrant it explicitly REFRAMES the maintenance priorities so the model builds (humbly, incrementally)
/// instead of refusing. Returns None when the default (maintenance) posture already fits.
/// The posture for a class, with the universal core appended.
///
/// Returns an owned String because the core is concatenated; the per-class part stays a &'static str
/// so it remains greppable in the binary.
pub fn posture_full(class: TaskClass) -> String {
    match posture(class) {
        Some(p) => format!("{p}{UNIVERSAL_CORE}{BUILD_BIAS}"),
        // The core applies even when no class posture does — those failures are not class-specific,
        // and a one-shot request is exactly where "another outline" is cheapest to produce.
        None => format!("{UNIVERSAL_CORE}{BUILD_BIAS}"),
    }
}

pub fn posture(class: TaskClass) -> Option<&'static str> {
    match (class.horizon, class.domain) {
        (Horizon::LongHorizon, Domain::Greenfield) => Some(GREENFIELD_BUILD_POSTURE),
        (Horizon::LongHorizon, Domain::Maintenance) => Some(LONG_MAINTENANCE_POSTURE),
        _ => None, // one-shot: the base prompt is fine
    }
}

/// The key fix: keep the humility, redirect it from refusal to incremental building.
/// Applied to EVERY class, because the failures it addresses are not class-specific.
///
/// Distilled from one observation that generalises: a model given engineering-workflow vocabulary
/// learns the vocabulary. It says CRITIQUE without finding a problem, says "let's implement character
/// classes" and then lists them again, and emits code-shaped text it never checked. Naming a step
/// creates an obligation to take it, and nothing in a persona says so.
///
/// Four lines, and each maps to an observed failure rather than a preference:
///
/// * **An outline is not step one.** A model asked to continue produced another outline, twice. The
///   instruction has to say that writing the plan again is not progress.
/// * **Say what you ran.** It emitted `struct Item { stats:{...} }` — not valid Rust — with no
///   acknowledgement. Unverified is fine; unverified-and-unmarked is not.
/// * **Never call it working unless it ran.** The single most load-bearing line, and the one this
///   codebase has spent a week learning the hard way: "gate green" meant green on lavapipe, "levers
///   OFF" meant untestable, "✓ verified" meant nothing had been checked.
/// * **Pick a default and proceed.** Loose direction is permission, not a question.
///
/// Deliberately short. A bloated preamble is the same anti-pattern as a bloated summary, and the
/// model that recites four principles is the model that recited "minimal edit distance" at a blank
/// directory.
/// Applied alongside the core, and it exists to correct an asymmetry in it.
///
/// The core says "an outline is not a step", "say what you ran", "never call it working unless it
/// ran". Every line pushes toward verifying before acting — and read alone they produce a model that
/// theorises instead of building, which is a different failure with the same cost.
///
/// **The distinction that matters is not think-versus-build. It is REVERSIBLE versus EXPENSIVE.**
///
/// Writing code into a git repo is reversible: it compiles or it does not, the tests pass or they do
/// not, and `git checkout` undoes it in a second. Shipping a confident explanation as a fix is
/// expensive: it consumes a run on someone else's hardware and returns one bit.
///
/// A real session spent nine hypotheses against five measurements on one bug. Four of the hypotheses
/// shipped as fixes and moved the failure count by zero. **Not one of them was too much code — every
/// one was too much reasoning about code that could have been printed.** The corrective is not more
/// caution, it is spending the cheap resource first.
pub const BUILD_BIAS: &str = "\n\nWHEN IN DOUBT, BUILD. Writing code is cheap and reversible — it \
compiles or it does not, and it can be reverted in a second. Reasoning about what the code would do \
is expensive: it consumes a turn and returns a guess. If a question could be settled by writing the \
thing, or by adding a print and running it, do that instead of explaining what you expect. Prefer the \
smallest version that RUNS over the most complete version that is described. Explanations are for \
after the result comes back, not instead of it.";

pub const UNIVERSAL_CORE: &str = "\n\nALWAYS: An outline is not a step \u{2014} if you name a step, \
take it in the same reply. State what you actually compiled or ran versus what you only wrote; \
unverified work is fine, unverified work presented as finished is not. Never describe something as \
working, done, or production-ready unless you ran it and saw it work. Given loose direction, choose a \
sensible default, say in one line why, and proceed \u{2014} do not ask a question you could answer.";

const GREENFIELD_BUILD_POSTURE: &str = "\n\nTASK POSTURE — NEW BUILD: The user is asking you to build \
something new, not to maintain existing code. Your engineering values still hold, but apply them to HOW you \
build, not WHETHER you build: 'minimal edit distance' here means start with the SMALLEST version that \
compiles and runs, then grow it one verified step at a time — it does NOT mean decline the request. \
Do not refuse a build the user explicitly asked for on the grounds that it's new, large, or outside a \
maintenance role; accepting a greenfield goal is not a violation of minimal-edit or architecture-preservation. \
Stay humble the RIGHT way: scope honestly (a playable prototype, not a shipped AAA title), build \
incrementally, verify each step compiles before the next, and say plainly what is a prototype vs \
production-ready. Decompose the goal into a short plan and TAKE step one IN THE SAME REPLY \
\u{2014} a plan whose first step is not taken is an outline, and the user asked for a build. Ship a \
runnable slice each turn rather than a more detailed description of one. Begin step one — don't stall on permission or \
re-litigate whether to proceed.";

/// Long-horizon maintenance: decompose, but the literal minimal-edit posture is correct here.
const LONG_MAINTENANCE_POSTURE: &str = "\n\nTASK POSTURE — LARGER MAINTENANCE: This is a multi-step change \
to existing code. Decompose it into a short plan of minimal, independently-verifiable edits, and work one \
at a time — smallest correct change first, verify, then the next. Preserve the existing architecture and \
style; the minimal-edit and preservation priorities apply literally here.";

/// The educate-then-defer principle: inform the user of real tradeoffs/concerns ONCE, then respect an
/// informed directive. This is the antidote to the transcript's re-litigation loop (the model kept asking
/// the user to reconfirm after the user had already reaffirmed). Applied to substantive (long-horizon)
/// tasks, where there's actually something worth educating about.
pub const EDUCATE_THEN_DEFER: &str = "\n\nEDUCATE, THEN DEFER: If a request carries real tradeoffs, risks, \
or scope concerns, name them ONCE — briefly and concretely — so the user can decide with open eyes. Then \
take the middle ground or follow their directive: when the user shows they understand and still want to \
proceed, proceed. Do not repeatedly ask them to reconfirm, re-explain your role, or re-litigate whether to \
begin. Voicing a concern is your job; overriding an informed decision is not.";

/// Appended when the user has REAFFIRMED a request after the model pushed back — the explicit 'they heard
/// you and said yes, so start now' signal that would have unstuck the refused transcript.
pub const REAFFIRMED_PROCEED: &str = "\n\nThe user has already heard your concern and reaffirmed the \
request. They understand the tradeoffs and want to proceed. Begin the work now — do not decline again or \
ask for further confirmation.";

/// Reaffirm markers in the LATEST user turn — short directives that push past a prior concern.
const REAFFIRM_MARKERS: &[&str] = &[
    "no, do", "no do", "yes, do", "yes do", "just do it", "do it anyway", "proceed anyway", "proceed",
    "i understand", "understood", "go ahead", "i know", "i get it", "regardless", "anyway", "do the",
    "still want", "yes proceed", "keep going", "continue anyway",
];
/// Pushback markers in a PRIOR assistant turn — signs it declined or hedged rather than acting.
const PUSHBACK_MARKERS: &[&str] = &[
    "decline", "i must", "not aligned", "respectfully", "confirm your intent", "would you prefer",
    "outside", "my role", "cannot ", "can't ", "shift direction", "please confirm", "against the",
    "not designed", "must clarify", "i understand your interest",
];

/// Did the user reaffirm their request after the model pushed back? True when the latest user turn reads as
/// a reaffirmation AND the immediately preceding assistant turn declined/hedged. High precision: needs both
/// sides, so a first-time request (nothing to reaffirm) or a normal continuation doesn't trip it.
pub fn reaffirmed_after_pushback(prev_assistant: &str, latest_user: &str) -> bool {
    let u = latest_user.to_ascii_lowercase();
    let a = prev_assistant.to_ascii_lowercase();
    let reaffirmed = REAFFIRM_MARKERS.iter().any(|m| u.contains(m));
    let pushed_back = PUSHBACK_MARKERS.iter().any(|m| a.contains(m));
    reaffirmed && pushed_back
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_3d_game_is_long_greenfield_and_gets_the_build_posture() {
        // the exact request the transcript refused
        let c = classify("write a smite 2 game in rust bevy wgpu in 3d, stepwise");
        assert_eq!(c.domain, Domain::Greenfield);
        assert_eq!(c.horizon, Horizon::LongHorizon);
        assert!(c.is_long_greenfield());
        let p = posture(c).expect("greenfield build gets a posture");
        assert!(p.contains("does NOT mean decline"), "the posture reframes refusal into building");
        assert!(p.to_lowercase().contains("humble") || p.to_lowercase().contains("prototype"),
            "keeps the humility");
    }

    #[test]
    fn fixing_a_crash_is_maintenance_not_a_new_build() {
        let c = classify("the worker crashed on signal 11, help me debug the segfault");
        assert_eq!(c.domain, Domain::Maintenance, "a crash fix must NOT be read as greenfield");
    }

    #[test]
    fn quick_helper_is_one_shot() {
        let c = classify("write a quick helper function to parse a date string");
        // has "write a" (greenfield verb) but "quick"/"a function" small-signals → one-shot
        assert_eq!(c.horizon, Horizon::OneShot);
        assert!(posture(c).is_none(), "a one-shot ask needs no special posture");
    }

    #[test]
    fn refactor_is_long_maintenance() {
        let c = classify("refactor the entire networking subsystem to use io_uring");
        assert_eq!(c.domain, Domain::Maintenance);
        assert_eq!(c.horizon, Horizon::LongHorizon);
        let p = posture(c).unwrap();
        assert!(p.contains("apply literally") || p.contains("MAINTENANCE"), "literal minimal-edit posture");
    }

    #[test]
    fn ambiguous_defaults_to_maintenance() {
        // no clear build verb, no clear maintenance verb → conservative Maintenance (never invent a project)
        let c = classify("look at the users endpoint latency");
        assert_eq!(c.domain, Domain::Maintenance);
    }

    #[test]
    fn build_a_database_is_long_greenfield() {
        let c = classify("build a small key-value database in rust");
        // "build a" greenfield + "database" scale word; "small" is a size hint but scale dominates the build
        assert_eq!(c.domain, Domain::Greenfield);
    }

    #[test]
    fn explain_existing_code_is_maintenance_one_shot() {
        let c = classify("explain this function and what the log line means");
        assert_eq!(c.domain, Domain::Maintenance);
        assert_eq!(c.horizon, Horizon::OneShot);
    }

    #[test]
    fn reaffirmation_after_refusal_is_detected() {
        // the exact transcript: model declined citing its role, user reaffirmed
        let prev = "That's an ambitious request, but I must respectfully decline. As a maintainer my role \
                    is not to build new games. Please confirm your intent so I can act within my role.";
        let user = "no do the 3d game";
        assert!(reaffirmed_after_pushback(prev, user), "user reaffirmed past the model's decline");
    }

    #[test]
    fn first_time_request_is_not_a_reaffirmation() {
        // a normal prior turn (no pushback) → the same directive isn't a 'reaffirmation'
        let prev = "Here's the plan. I'll start with the render loop.";
        let user = "do the 3d game";
        assert!(!reaffirmed_after_pushback(prev, user), "no prior pushback → not a reaffirmation");
    }

    #[test]
    fn understanding_then_proceed_counts() {
        let prev = "I must clarify this is outside my role and would introduce a large new subsystem.";
        let user = "I understand, proceed anyway";
        assert!(reaffirmed_after_pushback(prev, user));
    }
}

#[cfg(test)]
mod real_prompt_tests {
    use super::*;

    /// The exact prompts from a session that went wrong, run through the classifier.
    ///
    /// An external review scored that conversation 3/10 and singled out the model reciting
    /// "minimal edit distance" at a greenfield request. The posture that fixes it —
    /// `GREENFIELD_BUILD_POSTURE` — already existed. The open question was whether it FIRED.
    ///
    /// That distinction matters more than the fix: a posture that exists and does not trigger looks
    /// exactly like one that was never written, and every symptom is identical.
    #[test]
    fn write_smite_2_is_classified_as_a_NEW_BUILD() {
        let c = classify("write smite 2 in rust and bevy");
        assert_eq!(c.domain, Domain::Greenfield,
            "a greenfield request must not get the maintenance posture: {c:?}");
    }

    #[test]
    fn the_FOLLOW_UP_stays_a_new_build() {
        // "keep working on planning the game then implement it" — the turn where the model produced
        // another outline instead of code. If the follow-up reclassifies as maintenance, the posture
        // is lost exactly when the user is asking for the build.
        let c = classify("keep working on planning the game then implement it in rust code");
        assert_ne!(c.domain, Domain::Maintenance,
            "a continuation of a build request is still a build: {c:?}");
    }

    #[test]
    fn a_real_maintenance_request_is_still_maintenance() {
        // The posture must not fire on everything, or it removes the caution it was carved out of.
        let c = classify("fix the off-by-one in arena_place and keep the change small");
        assert_eq!(c.domain, Domain::Maintenance, "{c:?}");
    }

    #[test]
    fn a_greenfield_request_gets_a_posture_at_all() {
        // The end-to-end check: classification is only useful if it reaches a prompt.
        let c = classify("write a tui dashboard for managing these logs as a new rust crate");
        assert!(posture(c).is_some(), "a new build must get an explicit posture: {c:?}");
    }
}

#[cfg(test)]
mod universal_core_tests {
    use super::*;

    #[test]
    fn EVERY_class_gets_the_core_including_ones_with_no_posture() {
        // A one-shot request is exactly where "another outline" is cheapest to produce, so the class
        // with no special posture is the one that most needs the floor.
        for r in ["write smite 2 in rust and bevy",
                  "fix the off-by-one in arena_place",
                  "what does this function do"] {
            let p = posture_full(classify(r));
            assert!(p.contains("An outline is not a step"), "{r}: {p}");
        }
    }

    #[test]
    fn the_core_forbids_claiming_UNVERIFIED_work_is_done() {
        // The most load-bearing line, and the one this codebase learned the hard way: "gate green"
        // meant green on lavapipe, "levers OFF" meant untestable, "verified" meant unchecked.
        let p = posture_full(classify("build a tui dashboard"));
        assert!(p.contains("unless you ran it and saw it work"));
        assert!(p.contains("unverified work presented as finished"));
    }

    #[test]
    fn a_class_posture_is_KEPT_and_the_core_ADDED() {
        // The core is a floor, not a replacement — the greenfield advice about smallest-thing-first
        // is still the more specific and more useful guidance for that class.
        let p = posture_full(classify("write smite 2 in rust and bevy"));
        assert!(p.contains("NEW BUILD"), "the class posture survives");
        assert!(p.contains("choose a sensible default"), "and the core is appended");
    }

    #[test]
    fn the_core_is_SHORT() {
        // A bloated preamble is the same anti-pattern as a bloated summary, and a model reciting four
        // principles is the model that recited "minimal edit distance" at a blank directory.
        assert!(UNIVERSAL_CORE.len() < 600, "got {} chars", UNIVERSAL_CORE.len());
    }
}

/// What the workspace itself says, which outranks what the request says.
///
/// `classify` reads the request text. That was the only signal available and it is the weaker one:
/// **an empty repository is ground truth.** A request phrased as maintenance cannot be maintenance if
/// there is nothing to maintain, and no amount of careful wording detection beats counting the files.
///
/// A real transcript: a 1B model in an EMPTY repo, asked to "write a rust tui game", produced a plan
/// to "fix SQL injection vulnerabilities" and "optimize database queries". `_546` fixed the
/// prompt-side classifier so bare build verbs count. This is the signal that would have caught it
/// without any wording rule at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RepoState {
    /// No source files. Nothing can be maintained.
    Empty,
    /// A handful of files — a project just started.
    Sparse { files: usize },
    /// An established codebase.
    Established { files: usize },
}

impl RepoState {
    pub fn from_counts(source_files: usize) -> Self {
        match source_files {
            0 => RepoState::Empty,
            // Below this a "maintenance" framing still misleads: there is not enough there for
            // architecture-preservation advice to be about anything.
            n if n < 5 => RepoState::Sparse { files: n },
            n => RepoState::Established { files: n },
        }
    }

    /// Does the workspace contradict a maintenance framing?
    pub fn forbids_maintenance_posture(self) -> bool {
        matches!(self, RepoState::Empty | RepoState::Sparse { .. })
    }

    pub fn explain(self) -> &'static str {
        match self {
            RepoState::Empty =>
                "the repository is EMPTY — there is nothing to maintain, so a maintenance posture \
describes work that cannot exist, whatever the request said",
            RepoState::Sparse { .. } =>
                "the repository has only a few files — architecture-preservation advice would be \
about almost nothing, and a build posture serves better",
            RepoState::Established { .. } =>
                "an established codebase; the request text decides the posture",
        }
    }
}

/// The posture, with the workspace allowed to overrule the text.
pub fn posture_for(class: TaskClass, repo: RepoState) -> String {
    // The override runs FIRST. A wording rule that says "maintenance" and a directory that says
    // "empty" are not two opinions to weigh — one of them is a fact.
    if repo.forbids_maintenance_posture() {
        return format!("{GREENFIELD_BUILD_POSTURE}{UNIVERSAL_CORE}");
    }
    posture_full(class)
}

#[cfg(test)]
mod repo_state_tests {
    use super::*;

    #[test]
    fn an_EMPTY_repo_forbids_the_maintenance_posture_whatever_the_request_says() {
        // The transcript: a 1B model in an empty repo, asked to write a TUI game, produced a plan to
        // fix SQL injection vulnerabilities. No wording rule was needed to catch that — counting the
        // files was enough.
        let p = posture_for(classify("fix the bug in the parser"), RepoState::Empty);
        assert!(p.contains("NEW BUILD"), "an empty repo cannot be maintained");
    }

    #[test]
    fn a_SPARSE_repo_also_prefers_building() {
        // Architecture-preservation advice about three files is advice about almost nothing.
        assert!(RepoState::from_counts(3).forbids_maintenance_posture());
        assert!(!RepoState::from_counts(40).forbids_maintenance_posture());
    }

    #[test]
    fn an_ESTABLISHED_repo_lets_the_REQUEST_decide() {
        // The override is for when the workspace contradicts the text, not a blanket rule.
        let p = posture_for(classify("fix the off-by-one in arena_place"), RepoState::from_counts(40));
        assert!(!p.contains("NEW BUILD"));
    }

    #[test]
    fn the_UNIVERSAL_CORE_survives_the_override() {
        // The override replaces the class posture and must not drop the floor with it.
        let p = posture_for(classify("fix the parser"), RepoState::Empty);
        assert!(p.contains("An outline is not a step"));
    }

    #[test]
    fn the_explanation_says_it_is_a_FACT_not_a_preference() {
        assert!(RepoState::Empty.explain().contains("whatever the request said"));
    }
}

#[cfg(test)]
mod build_bias_tests {
    use super::*;

    #[test]
    fn EVERY_posture_carries_the_build_bias() {
        // The core alone reads as "verify before acting" and produces a model that theorises rather
        // than builds — a different failure with the same cost.
        for r in ["write smite 2 in rust", "fix the off-by-one in arena_place", "what does this do"] {
            assert!(posture_full(classify(r)).contains("WHEN IN DOUBT, BUILD"), "{r}");
        }
    }

    #[test]
    fn it_names_the_real_distinction_REVERSIBLE_not_think_versus_build() {
        // Writing code is reversible; shipping a confident explanation as a fix consumes a run on
        // someone else's hardware and returns one bit.
        assert!(BUILD_BIAS.contains("cheap and reversible"));
        assert!(BUILD_BIAS.contains("returns a guess"));
    }

    #[test]
    fn it_prefers_the_smallest_thing_that_RUNS() {
        // Not the most complete thing described. A running fragment produces evidence; a complete
        // description produces agreement.
        assert!(BUILD_BIAS.contains("smallest version that RUNS"));
        assert!(BUILD_BIAS.contains("most complete version that is described"));
    }

    #[test]
    fn it_does_NOT_contradict_the_core() {
        // Both survive together: build first, and still never call it working unless it ran. The
        // bias is about which resource to spend, not about whether to check.
        let p = posture_full(classify("build a tui dashboard"));
        assert!(p.contains("unless you ran it and saw it work"));
        assert!(p.contains("WHEN IN DOUBT, BUILD"));
    }

    #[test]
    fn a_print_counts_as_building() {
        // The specific corrective: nine hypotheses against five measurements, and the measurements
        // were four lines each. "Add a print and run it" is the cheap action the core never named.
        assert!(BUILD_BIAS.contains("adding a print and running it"));
    }
}
