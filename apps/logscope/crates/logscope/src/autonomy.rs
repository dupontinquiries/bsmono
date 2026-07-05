//! autonomy — "survival tool" means **self-sufficient**, not resource-starved.
//!
//! The desert-camper scenario is about being *on your own*: no cloud, no external services, fix
//! the hardware in your hand with what the machine itself can do. It is **not** about rationing
//! RAM or power — the kernel already handles memory pressure and energy-aware scheduling far
//! better than an application can, so trying to re-implement that at the app layer is futile.
//!
//! What the app layer actually controls, and where the real wins are: **memory *utilization*,
//! cache reuse, and algorithmic efficiency.** On a computer, doing *more* work up front is often
//! the efficient path (a warm KV cache beats re-prefilling every turn; a prefix tree beats
//! recomputation) — the point of the machine is to spend cycles to save cycles. logscope already
//! leans this way (the KV prefix-tree reuse behind `--kv-tree`, model_selector's warm handoff).
//!
//! So this module holds exactly one thing worth asserting in code: the **self-sufficiency
//! invariant** — every *core* capability runs with no external services. A change that makes a
//! core path need the network is a regression against the whole premise, and the test here guards
//! it. (The one honest hardware question — does a model physically *fit* in RAM — is
//! `model_selector`'s job, not a "frugal mode".)

/// How far a capability has to reach to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Runs entirely on this machine — no network, ever.
    LocalOnly,
    /// Uses local resources by default; can *optionally* use a remote service if configured, but
    /// never requires it.
    PrefersLocal,
    /// Cannot function without the network. No *core* logscope capability is allowed to be this.
    RequiresNetwork,
}

/// logscope's core capabilities and how far each reaches. The desert-camper invariant is that none
/// of these is `RequiresNetwork`.
pub fn core_reach(capability: Capability) -> Reach {
    match capability {
        // local logs + heuristic parsing — never touches the network
        Capability::Diagnose => Reach::LocalOnly,
        // local GGUF inference by default; a remote model is an optional step-up, never required
        Capability::ChatPalace => Reach::PrefersLocal,
        // AST blocs + local model actions
        Capability::Forge => Reach::PrefersLocal,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Diagnose,
    ChatPalace,
    Forge,
}

impl Capability {
    pub const ALL: [Capability; 3] = [Capability::Diagnose, Capability::ChatPalace, Capability::Forge];
}

/// True iff every core capability can run with no network — the property the survival framing is
/// really about. This should always hold; the test asserts it so a future cloud dependency can't
/// sneak into a core path.
pub fn fully_offline_capable() -> bool {
    Capability::ALL.iter().all(|c| core_reach(*c) != Reach::RequiresNetwork)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_core_capability_runs_offline() {
        assert!(fully_offline_capable(), "a core capability must never require the network");
        // Diagnose is the strict floor: pure-local, even without a model
        assert_eq!(core_reach(Capability::Diagnose), Reach::LocalOnly);
    }

    #[test]
    fn remote_is_a_step_up_never_a_requirement() {
        // the palace/forge may *prefer* local and optionally use remote, but never *require* it
        for c in [Capability::ChatPalace, Capability::Forge] {
            assert_ne!(core_reach(c), Reach::RequiresNetwork);
        }
    }
}
