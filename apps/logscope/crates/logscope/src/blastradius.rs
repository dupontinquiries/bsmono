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
