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
    let greenfield_hits = GREENFIELD_VERBS.iter().filter(|w| r.contains(**w)).count();
    let maintenance_hits = MAINTENANCE_VERBS.iter().filter(|w| r.contains(**w)).count();
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
pub fn posture(class: TaskClass) -> Option<&'static str> {
    match (class.horizon, class.domain) {
        (Horizon::LongHorizon, Domain::Greenfield) => Some(GREENFIELD_BUILD_POSTURE),
        (Horizon::LongHorizon, Domain::Maintenance) => Some(LONG_MAINTENANCE_POSTURE),
        _ => None, // one-shot: the base prompt is fine
    }
}

/// The key fix: keep the humility, redirect it from refusal to incremental building.
const GREENFIELD_BUILD_POSTURE: &str = "\n\nTASK POSTURE — NEW BUILD: The user is asking you to build \
something new, not to maintain existing code. Your engineering values still hold, but apply them to HOW you \
build, not WHETHER you build: 'minimal edit distance' here means start with the SMALLEST version that \
compiles and runs, then grow it one verified step at a time — it does NOT mean decline the request. \
Do not refuse a build the user explicitly asked for on the grounds that it's new, large, or outside a \
maintenance role; accepting a greenfield goal is not a violation of minimal-edit or architecture-preservation. \
Stay humble the RIGHT way: scope honestly (a playable prototype, not a shipped AAA title), build \
incrementally, verify each step compiles before the next, and say plainly what is a prototype vs \
production-ready. Decompose the goal into a short plan and begin step one — don't stall on permission or \
re-litigate whether to proceed.";

/// Long-horizon maintenance: decompose, but the literal minimal-edit posture is correct here.
const LONG_MAINTENANCE_POSTURE: &str = "\n\nTASK POSTURE — LARGER MAINTENANCE: This is a multi-step change \
to existing code. Decompose it into a short plan of minimal, independently-verifiable edits, and work one \
at a time — smallest correct change first, verify, then the next. Preserve the existing architecture and \
style; the minimal-edit and preservation priorities apply literally here.";

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
}
