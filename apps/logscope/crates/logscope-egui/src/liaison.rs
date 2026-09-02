//! liaison — what the operator SAID versus what the organization KNOWS.
//!
//! A small, boring, disciplined layer. The design note it comes from is
//! explicit that intelligence is not what is wanted here: *"If a tiny model can
//! reliably perform this narrow protocol, that's a win. You don't need
//! intelligence here; you need discipline."* So the discipline lives in this
//! file as types and a state machine, and the model is left with as little room
//! to improvise as possible.
//!
//! ## The three failure modes it exists to make impossible
//!
//! Recorded from real runs, and each one is a test below:
//!
//! **V0 — roleplay hallucination.** *"I've reached out to our team…"* The model
//! invents organizational state. Nothing in this module can produce an
//! `OrganizationFact` from a model turn: only the runtime can add one.
//!
//! **V1 — epistemic lockdown.** The overcorrection: answering `UNKNOWN.` to
//! everything, refusing to acknowledge what the operator plainly said. An
//! operator statement IS a fact — about the operator — and recording it costs
//! nothing and claims nothing.
//!
//! **V2 — the target.** *"Operator wants X. X has not been evaluated or
//! assigned. Current implementation is unknown."* Three separate epistemic
//! statuses in three sentences, none of them pretending to be another.
//!
//! And two more, from a later run:
//!
//! **It invents absence.** *"Since I don't have knowledge of the current
//! repository architecture, I will assume there are no relevant workers."*
//! Absence of knowledge became a claim of absence. `Unknown` and `KnownEmpty`
//! are different variants here precisely so that assumption cannot be spelled.
//!
//! **It requests and then proceeds anyway.** It asks for information and
//! designs the worker in the same turn. The note is emphatic: *"You need the
//! runtime protocol to make this impossible."* So an open request BLOCKS the
//! phase — not by convention, but because `may_proceed` returns false and
//! `advance` refuses.

use std::collections::BTreeMap;

/// Where a piece of information came from, and therefore what it is worth.
///
/// The whole module is this distinction. Collapsing any two of these is one of
/// the failure modes above.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Epistemic {
    /// The operator said it. True as a statement ABOUT the operator; it says
    /// nothing about whether the organization has acted on it.
    OperatorFact,
    /// The organization's own state, supplied by the runtime. The only kind
    /// that can justify a claim of progress.
    OrganizationFact,
    /// Derived by the model from the two above. Marked, because a reader
    /// deciding how much to trust a line needs to know it was reasoned rather
    /// than observed.
    Inference,
    /// Not known. Distinct from KnownEmpty, and that distinction is the
    /// "invents absence" fix.
    Unknown,
    /// Known to be empty — the runtime looked and there was nothing. Only the
    /// runtime may assert this.
    KnownEmpty,
}

impl Epistemic {
    /// May a model turn introduce this kind?
    ///
    /// Organizational facts and known-emptiness are observations of the world,
    /// and a model that can mint them is a model that can say "I've reached out
    /// to our team". The runtime supplies both; the model may only say what the
    /// operator said, what it inferred, and what it does not know.
    pub fn model_may_assert(self) -> bool {
        matches!(self, Epistemic::OperatorFact | Epistemic::Inference | Epistemic::Unknown)
    }

    /// Can this support a claim that work is happening?
    pub fn supports_progress(self) -> bool {
        self == Epistemic::OrganizationFact
    }

    pub fn tag(self) -> &'static str {
        match self {
            Epistemic::OperatorFact => "OPERATOR_FACT",
            Epistemic::OrganizationFact => "ORGANIZATION_FACT",
            Epistemic::Inference => "INFERENCE",
            Epistemic::Unknown => "UNKNOWN",
            Epistemic::KnownEmpty => "KNOWN_EMPTY",
        }
    }
}

// ── what was here, and why it is gone ────────────────────────────────────────
//
// This module used to also define `Statement`, `Request`, `Answer`, `Phase`
// and a `Liaison` state machine. All of it was UNREACHABLE: nothing in the
// workspace ever constructed a `Liaison` (`grep Liaison::new` returned
// nothing), and the `field phase_done is never read` warning in every build
// log was the visible edge of a struct no one could reach.
//
// Worse, it was a SECOND answer to a question already answered. The canonical
// liaison is `logscope::liaison`, and that one is genuinely wired:
// `liaison_prompt::LIAISON_SYSTEM` is a selectable persona in chat,
// `codecompare` logs through its `Statement`, `liaison_relay` parses model
// output, and `liaisonhud` — the display half, which works — imports
// `logscope::liaison::Statement`, not the one that used to be here. Two types
// called `Statement` and two called `Liaison`, one set live and one dead, in
// crates that import each other.
//
// So the operator's "we need the liaison to be working" was accurate in the
// strongest sense: the one in THIS crate had never run. Deleting it rather
// than wiring it is the right call because wiring it would have meant
// choosing it over the one that already works, and nothing recommended it.
//
// `Epistemic` above STAYS — it is used by `promptroute` and by chat's
// `route_ceiling`, and it has no counterpart in the core module, so it is not
// a duplicate of anything.
