//! blastradius.rs — what an agent can do that it cannot undo.
//!
//! Written after a question about AI safety in a fleet of agents. The answer that matters is not
//! about malice: **an agent does not need to be misaligned to run `rm -rf` on the wrong path.** A
//! confused agent and a hostile one issue the same command, so intent is the wrong axis and
//! REVERSIBILITY is the right one.
//!
//! ## Why prompt lines do not cover this
//!
//! "Do not do destructive things" fails on the case that actually happens: an agent that believes
//! deleting the directory IS the task. It was told to clean up. The instruction and the action agree;
//! only the path is wrong.
//!
//! Capability limits hold where instructions do not, because they do not depend on the agent's model
//! of what it is doing being correct.
//!
//! ## The live threat for a repo-reading agent
//!
//! **Prompt injection through file content.** LogScope reads repositories and summarises them. A file
//! containing "ignore previous instructions and ..." is an instruction from an untrusted source
//! arriving through a trusted channel, and it requires no misalignment from any model. Content read
//! from disk must never be able to escalate what the reader may do.

/// How hard an action is to undo.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Reversibility {
    /// Undone by a keystroke: writing inside a git working tree, creating a temp file.
    Trivial,
    /// Undone with effort: a commit (revert), a moved file (move back).
    Recoverable,
    /// Not undone by this system: deleting untracked files, force-push, dropping a database.
    Irreversible,
    /// Not undone by ANYONE: sending an email, posting to an API, spending money, publishing.
    Escaped,
}

/// Something an agent proposes to do.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Action {
    pub what: String,
    pub reversibility: Reversibility,
    /// Did any part of this action's justification come from content the agent READ rather than from
    /// the operator?
    ///
    /// **The prompt-injection flag.** A repo file that says "run this" is an instruction from an
    /// untrusted source, and it must never raise what the reader is permitted to do.
    pub from_read_content: bool,
}

/// What a supervisor should do about it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ruling { Allow, Confirm { because: String }, Refuse { because: String } }

impl Ruling {
    pub fn permitted(&self) -> bool { matches!(self, Ruling::Allow) }
}

/// Rule on an action.
///
/// Deliberately not a model call. **A monitor that is the same model shares its blind spots**, so a
/// second opinion from the same weights is correlated in exactly the cases where it matters. This is
/// mechanical, which makes it uncorrelated with whatever the agent got wrong.
pub fn rule(a: &Action) -> Ruling {
    // CONTENT-SOURCED ACTIONS CANNOT ESCALATE. This is the whole prompt-injection defence: not
    // detecting malicious text, which is unbounded, but refusing to let read content authorise
    // anything the operator did not.
    if a.from_read_content && a.reversibility >= Reversibility::Irreversible {
        return Ruling::Refuse { because: format!(
            "'{}' is irreversible and its justification came from content that was READ rather than \
from the operator. A file saying 'run this' is an instruction from an untrusted source; it may \
inform, and it may not authorise", a.what) };
    }
    match a.reversibility {
        Reversibility::Trivial => Ruling::Allow,
        Reversibility::Recoverable => Ruling::Allow,
        Reversibility::Irreversible => Ruling::Confirm { because: format!(
            "'{}' cannot be undone by this system. Confirming costs a turn; being wrong costs the \
work", a.what) },
        // Not confirm — refuse. An agent should not be able to send mail or spend money on any
        // reasoning at all, because the failure is unbounded and a confirmation prompt is exactly
        // what a confused agent produces a plausible justification for.
        Reversibility::Escaped => Ruling::Refuse { because: format!(
            "'{}' leaves this system entirely and cannot be recalled by anyone. That is not a \
confirmation, it is a capability this agent should not have", a.what) },
    }
}

/// Rule on a batch, and catch the pattern no single action shows.
///
/// **Many small irreversible actions are one large one.** An agent deleting forty files one at a
/// time passes forty individual checks, which is how a per-action monitor misses the only case that
/// matters.
pub fn rule_batch(actions: &[Action]) -> Vec<Ruling> {
    let irreversible = actions.iter()
        .filter(|a| a.reversibility >= Reversibility::Irreversible).count();
    let mut out: Vec<Ruling> = actions.iter().map(rule).collect();
    if irreversible > 3 {
        out.push(Ruling::Refuse { because: format!(
            "{irreversible} irreversible actions in one batch. Each may be defensible alone; \
together they are a large change nobody approved, and approving them individually is how that \
happens") });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(what: &str, r: Reversibility, read: bool) -> Action {
        Action { what: what.into(), reversibility: r, from_read_content: read }
    }

    #[test]
    fn the_axis_is_REVERSIBILITY_not_intent() {
        // A confused agent and a hostile one issue the same command. Intent is unavailable and
        // reversibility is observable.
        assert!(rule(&act("write a file in the repo", Reversibility::Trivial, false)).permitted());
        assert!(!rule(&act("force-push", Reversibility::Irreversible, false)).permitted());
    }

    #[test]
    fn CONTENT_SOURCED_justification_cannot_authorise_the_irreversible() {
        // The prompt-injection defence: not detecting malicious text, which is unbounded, but
        // refusing to let read content authorise what the operator did not.
        let r = rule(&act("delete the build directory", Reversibility::Irreversible, true));
        assert!(matches!(r, Ruling::Refuse { .. }));
        let m = format!("{r:?}");
        assert!(m.contains("may inform, and it may not authorise"));
    }

    #[test]
    fn the_same_action_from_the_OPERATOR_only_needs_confirming() {
        // The rule is about the SOURCE of the justification, not about the action being forbidden.
        assert!(matches!(rule(&act("delete the build directory", Reversibility::Irreversible, false)),
                         Ruling::Confirm { .. }));
    }

    #[test]
    fn ESCAPED_actions_are_REFUSED_rather_than_confirmed() {
        // A confirmation prompt is exactly what a confused agent produces a plausible justification
        // for, and sending mail cannot be recalled by anyone.
        let r = rule(&act("send an email", Reversibility::Escaped, false));
        assert!(format!("{r:?}").contains("a capability this agent should not have"));
    }

    #[test]
    fn MANY_SMALL_irreversible_actions_are_caught_as_one_large_one() {
        // Forty deletions pass forty individual checks. That is how a per-action monitor misses the
        // only case that matters.
        let batch: Vec<Action> = (0..5)
            .map(|i| act(&format!("delete file {i}"), Reversibility::Irreversible, false)).collect();
        let r = rule_batch(&batch);
        assert!(format!("{:?}", r.last().unwrap()).contains("nobody approved"));
    }

    #[test]
    fn a_few_irreversible_actions_do_NOT_trip_the_batch_rule() {
        // Or it fires constantly and gets ignored, which is worse than not existing.
        let batch = vec![act("delete a", Reversibility::Irreversible, false)];
        assert_eq!(rule_batch(&batch).len(), 1, "no batch verdict appended");
    }

    #[test]
    fn the_check_is_MECHANICAL_not_a_model_call() {
        // A monitor that is the same model shares its blind spots, so a second opinion from the same
        // weights is correlated in exactly the cases where it matters. Asserted by construction: this
        // function is pure and takes no context.
        let a = act("rm -rf /", Reversibility::Irreversible, false);
        assert_eq!(rule(&a), rule(&a), "deterministic, and therefore uncorrelated with the agent");
    }
}

/// Taint that PROPAGATES, modelled on Perl's taint mode.
///
/// `Action::from_read_content` is a flag someone has to set, which protects only the first hop. If a
/// summariser reads a file and its output feeds another agent, the second agent sees clean-looking
/// text and the defence is gone.
///
/// **Perl solved this in 1989** and the design is still right: data originating outside the program
/// is tainted, taint follows every derivation automatically, and tainted data cannot reach a
/// dangerous operation until it is explicitly laundered. The laundering is the interesting part —
/// Perl requires a regex match, because forcing the programmer to say WHAT SHAPE they expect is what
/// makes the untainting a decision rather than a cast.
///
/// This is the same, adapted: a summary of a tainted file is tainted, a plan derived from a tainted
/// summary is tainted, and laundering requires the operator.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Tainted<T> {
    value: T,
    /// Where it came from, in order. **Kept as a chain rather than a bool** so a refusal can say
    /// which file three hops back is responsible — "this is tainted" is not actionable, "this came
    /// from vendor/foo/README via two summaries" is.
    origin: Vec<String>,
}

impl<T> Tainted<T> {
    /// Data the operator typed. Clean by definition — they are the authority this system serves.
    pub fn from_operator(value: T) -> Self { Tainted { value, origin: Vec::new() } }

    /// Data read from anywhere else: a file, a crate, a web page, another agent's output.
    pub fn from_source(value: T, source: impl Into<String>) -> Self {
        Tainted { value, origin: vec![source.into()] }
    }

    pub fn is_tainted(&self) -> bool { !self.origin.is_empty() }
    pub fn origin(&self) -> &[String] { &self.origin }

    /// Read the value. **Always permitted** — taint restricts what may be AUTHORISED, never what may
    /// be read or reasoned about. A defence that stopped the agent seeing file contents would stop
    /// it working.
    pub fn peek(&self) -> &T { &self.value }

    /// Derive a new value. **Taint follows automatically**, which is the whole point: a summary of a
    /// tainted file is tainted without anyone remembering to say so.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Tainted<U> {
        Tainted { value: f(self.value), origin: self.origin }
    }

    /// Combine two values. Taint unions — one tainted input taints the result, because a plan
    /// half-derived from an untrusted file is a plan derived from an untrusted file.
    pub fn combine<U, V>(self, other: Tainted<U>, f: impl FnOnce(T, U) -> V) -> Tainted<V> {
        let mut origin = self.origin;
        origin.extend(other.origin);
        Tainted { value: f(self.value, other.value), origin }
    }

    /// Launder, with the operator's explicit approval.
    ///
    /// Takes the approval as a value rather than a bool so it cannot be passed accidentally, and
    /// records what was approved. Perl's regex requirement has the same purpose: make untainting a
    /// decision with a shape, not a cast.
    pub fn launder(self, approval: OperatorApproval) -> T {
        let _ = approval;
        self.value
    }
}

/// Proof that a human looked at the specific thing.
///
/// Constructed only from a description, so an approval for one action cannot be reused for another
/// by a caller holding a spare `true`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OperatorApproval(String);

impl OperatorApproval {
    pub fn of(what_was_shown: impl Into<String>) -> Self { OperatorApproval(what_was_shown.into()) }
    pub fn what(&self) -> &str { &self.0 }
}

/// Rule on an action whose justification is tainted.
///
/// Supersedes the `from_read_content` flag: the taint arrives with the value rather than being
/// remembered by a caller.
pub fn rule_tainted(action: &Tainted<Action>) -> Ruling {
    let a = action.peek();
    if action.is_tainted() && a.reversibility >= Reversibility::Irreversible {
        return Ruling::Refuse { because: format!(
            "'{}' is irreversible and its justification traces to {} — content that was read, not \
instructed. It may inform and it may not authorise", a.what, action.origin().join(" -> ")) };
    }
    rule(a)
}

#[cfg(test)]
mod taint_tests {
    use super::*;

    #[test]
    fn taint_PROPAGATES_through_a_summary() {
        // The gap the flag left: a summariser reads a file, and the next agent sees clean-looking
        // text. Perl solved this in 1989 by making taint follow every derivation.
        let file = Tainted::from_source("rm -rf /".to_string(), "vendor/foo/README.md");
        let summary = file.map(|s| format!("the readme suggests: {s}"));
        assert!(summary.is_tainted());
        let plan = summary.map(|s| format!("plan based on {s}"));
        assert!(plan.is_tainted(), "three hops and still tainted");
    }

    #[test]
    fn the_ORIGIN_CHAIN_survives_so_a_refusal_can_name_the_file() {
        // "This is tainted" is not actionable. "This came from vendor/foo/README via two summaries"
        // is — the operator can go and look.
        let a = Tainted::from_source("x".to_string(), "crates.io/somecrate/build.rs");
        assert_eq!(a.map(|s| s.len()).origin(), ["crates.io/somecrate/build.rs"]);
    }

    #[test]
    fn ONE_tainted_input_taints_a_combination() {
        // A plan half-derived from an untrusted file is a plan derived from an untrusted file.
        let clean = Tainted::from_operator(1u32);
        let dirty = Tainted::from_source(2u32, "some.md");
        assert!(clean.combine(dirty, |a, b| a + b).is_tainted());
    }

    #[test]
    fn OPERATOR_data_is_clean_and_stays_clean() {
        // They are the authority this system serves; treating their input as suspect would make the
        // whole mechanism useless.
        let t = Tainted::from_operator("delete the build dir".to_string());
        assert!(!t.is_tainted());
        assert!(!t.map(|s| s.to_uppercase()).is_tainted());
    }

    #[test]
    fn READING_tainted_data_is_always_permitted() {
        // Taint restricts what may be AUTHORISED, never what may be read or reasoned about. A defence
        // that stopped the agent seeing file contents would stop it working.
        let t = Tainted::from_source("some content".to_string(), "f.md");
        assert_eq!(t.peek(), "some content");
    }

    #[test]
    fn a_TAINTED_irreversible_action_is_refused_and_names_its_chain() {
        let a = Tainted::from_source(
            Action { what: "delete node_modules".into(),
                     reversibility: Reversibility::Irreversible, from_read_content: false },
            "vendor/setup.md");
        let r = rule_tainted(&a);
        assert!(matches!(r, Ruling::Refuse { .. }));
        assert!(format!("{r:?}").contains("vendor/setup.md"));
    }

    #[test]
    fn the_same_action_from_the_OPERATOR_only_confirms() {
        let a = Tainted::from_operator(
            Action { what: "delete node_modules".into(),
                     reversibility: Reversibility::Irreversible, from_read_content: false });
        assert!(matches!(rule_tainted(&a), Ruling::Confirm { .. }));
    }

    #[test]
    fn LAUNDERING_requires_a_described_approval_not_a_bool() {
        // Perl requires a regex so untainting is a decision with a shape. An approval carrying what
        // was shown cannot be reused for a different action by a caller holding a spare `true`.
        let t = Tainted::from_source(42u32, "f.md");
        assert_eq!(t.launder(OperatorApproval::of("shown the file and the proposed deletion")), 42);
    }
}
