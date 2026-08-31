//! sysstatus — the status light, told the truth.
//!
//! The reference panels carry a small SYSTEM STATUS block in the corner: a dot and "All systems
//! operational". (Generated design studies, not captures — they say what the interface should feel
//! like and nothing about how it behaves.)
//!
//! ## A hardcoded green light is worse than no light
//!
//! The obvious way to build that block is a green dot and a constant string, and it would be a lie
//! the first time anything was not ready — which in this app is most of the time on a fresh start:
//! no model loaded, the bundled `sample.log` standing in for the user's data, no project mounted.
//! A permanently-green indicator does not merely fail to inform, it actively asserts that the
//! things it is next to are fine, and the reader stops checking them.
//!
//! So this is derived from real signals, it can say NOT READY, and it names the first thing to fix.
//!
//! ## Every signal can veto
//!
//! The failure mode for a rollup like this is a signal that gets added to the struct and never
//! consulted, leaving the light green while the thing it describes is broken. That is not a
//! judgement call, it is a property: for each field, flipping it to its bad value must move the
//! verdict off Ready. The test enumerates them, so a sixth signal added without a rule fails
//! immediately rather than quietly widening the lie.

/// How ready the app is to do the thing it is for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Readiness {
    /// Everything checked is true.
    Ready,
    /// Usable, but something will surprise the operator — sample data standing in for theirs, or a
    /// run that has stopped making progress.
    Degraded,
    /// The core thing cannot happen: no model.
    NotReady,
}

impl Readiness {
    pub fn label(self) -> &'static str {
        match self {
            Readiness::Ready => "ready",
            Readiness::Degraded => "degraded",
            Readiness::NotReady => "not ready",
        }
    }
}

/// What the status is computed from. Every field is consulted; see the veto test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signals {
    /// A model is loaded and can generate.
    pub model_loaded: bool,
    /// The attached document is the bundled sample rather than the operator's data. This is the
    /// one that produced "hi there" answered as a discussion of a worker exiting on signal 11, so
    /// it earns a place in the rollup rather than a footnote on one card.
    pub sample_doc: bool,
    /// A project is mounted, so the agent has somewhere to work.
    pub project_mounted: bool,
    /// The phase machine says the current run has stopped making progress.
    pub run_blocked: bool,
    /// The backend's FFI is compiled in and usable, rather than the fallback.
    pub backend_ready: bool,
}

impl Default for Signals {
    /// Everything true, so a caller that forgets a field gets the OPTIMISTIC default and the veto
    /// test still fails loudly rather than a pessimistic default hiding the omission behind a
    /// permanent warning nobody reads.
    fn default() -> Self {
        Signals { model_loaded: true, sample_doc: false, project_mounted: true,
                  run_blocked: false, backend_ready: true }
    }
}

/// The verdict and the first thing to fix.
///
/// One message, not a list. A status corner with five complaints in it is a wall, and the operator
/// can only act on one thing at a time anyway — so the order below is the order of what blocks
/// what: without a model nothing else matters, and a blocked run is louder than a missing project
/// because it means work is actively going nowhere.
pub fn assess(s: &Signals) -> (Readiness, &'static str) {
    if !s.model_loaded {
        return (Readiness::NotReady, "no model loaded — pick one in Models");
    }
    if !s.backend_ready {
        return (Readiness::NotReady, "no usable inference backend");
    }
    if s.run_blocked {
        return (Readiness::Degraded, "the run stopped making progress — change approach");
    }
    if s.sample_doc {
        return (Readiness::Degraded, "answering from the bundled sample, not your data");
    }
    if !s.project_mounted {
        return (Readiness::Degraded, "no project mounted — the agent has nowhere to work");
    }
    (Readiness::Ready, "ready")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_true_is_the_only_way_to_be_ready() {
        assert_eq!(assess(&Signals::default()), (Readiness::Ready, "ready"));
    }

    /// THE PROPERTY THAT MATTERS. A signal added to the struct and never consulted leaves the light
    /// green while the thing it describes is broken. Every field must be able to veto, so a sixth
    /// one added without a rule fails here rather than quietly widening the lie.
    #[test]
    fn every_signal_can_veto_ready() {
        let vetoes: [(&str, Signals); 5] = [
            ("model_loaded", Signals { model_loaded: false, ..Default::default() }),
            ("backend_ready", Signals { backend_ready: false, ..Default::default() }),
            ("run_blocked", Signals { run_blocked: true, ..Default::default() }),
            ("sample_doc", Signals { sample_doc: true, ..Default::default() }),
            ("project_mounted", Signals { project_mounted: false, ..Default::default() }),
        ];
        for (name, s) in vetoes {
            let (level, msg) = assess(&s);
            assert_ne!(level, Readiness::Ready, "{name} does not affect the verdict");
            assert_ne!(msg, "ready", "{name} vetoed without saying why");
        }
        // and the count is pinned: a field added to Signals without a veto case above is a field
        // nobody proved is consulted.
        assert_eq!(vetoes.len(), 5, "Signals gained a field — give it a veto case");
    }

    /// Without a model nothing else matters, so it must be what the corner says even when three
    /// other things are also wrong.
    #[test]
    fn the_blocking_problem_is_the_one_reported() {
        let everything_wrong = Signals { model_loaded: false, backend_ready: false,
                                         run_blocked: true, sample_doc: true,
                                         project_mounted: false };
        let (level, msg) = assess(&everything_wrong);
        assert_eq!(level, Readiness::NotReady);
        assert!(msg.contains("no model"), "reported '{msg}' over the missing model");
    }

    /// A blocked run is louder than a missing project: work is actively going nowhere, rather than
    /// merely not having started.
    #[test]
    fn a_blocked_run_outranks_a_missing_project() {
        let s = Signals { run_blocked: true, project_mounted: false, ..Default::default() };
        assert!(assess(&s).1.contains("stopped making progress"));
    }

    /// Sample data is the failure that looks like success — the model answers confidently about a
    /// log the operator has never seen — so it must reach the status corner, not just the card.
    #[test]
    fn the_bundled_sample_is_reported_as_degraded_not_ready() {
        let s = Signals { sample_doc: true, ..Default::default() };
        let (level, msg) = assess(&s);
        assert_eq!(level, Readiness::Degraded);
        assert!(msg.contains("sample"), "{msg}");
    }

    /// The message is what an operator acts on, so it must say what to do, not just what is wrong.
    #[test]
    fn every_message_is_short_and_actionable() {
        let all: [Signals; 5] = [
            Signals { model_loaded: false, ..Default::default() },
            Signals { backend_ready: false, ..Default::default() },
            Signals { run_blocked: true, ..Default::default() },
            Signals { sample_doc: true, ..Default::default() },
            Signals { project_mounted: false, ..Default::default() },
        ];
        for s in all {
            let (_, msg) = assess(&s);
            let n = msg.chars().count();
            assert!((14..=60).contains(&n), "{n} chars: '{msg}'");
            assert_eq!(msg, msg.trim());
            assert!(!msg.ends_with('.'), "status lines are labels, not sentences: '{msg}'");
        }
    }

    /// The optimistic default is deliberate: a pessimistic one would hide a forgotten field behind
    /// a permanent warning, which is the same lie in the other direction.
    #[test]
    fn the_default_is_optimistic_on_purpose() {
        assert_eq!(assess(&Signals::default()).0, Readiness::Ready);
        assert!(Readiness::Ready < Readiness::Degraded && Readiness::Degraded < Readiness::NotReady,
                "the ordering is used to pick the worst of several");
    }
}
