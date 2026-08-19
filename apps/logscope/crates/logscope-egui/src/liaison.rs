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

/// One statement, with its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub kind: Epistemic,
    pub text: String,
}

impl Statement {
    pub fn operator(text: impl Into<String>) -> Self {
        Statement { kind: Epistemic::OperatorFact, text: text.into() }
    }
    pub fn inference(text: impl Into<String>) -> Self {
        Statement { kind: Epistemic::Inference, text: text.into() }
    }
    pub fn unknown(text: impl Into<String>) -> Self {
        Statement { kind: Epistemic::Unknown, text: text.into() }
    }
}

/// A question for the runtime, not for the human.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The runtime verb, e.g. `GET_ACTIVE_WORKERS`.
    pub verb: String,
    /// Filled in when the runtime answers. `None` while it is outstanding, and
    /// an outstanding request is what blocks the phase.
    pub answer: Option<Answer>,
}

/// What the runtime came back with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// Facts, which become ORGANIZATION_FACTs.
    Facts(Vec<String>),
    /// The runtime looked and there was nothing — KNOWN_EMPTY, not UNKNOWN.
    Empty,
    /// The runtime cannot answer. Stays UNKNOWN; it does NOT become Empty,
    /// which is exactly the conflation that produced "I will assume there are
    /// no relevant workers".
    Unavailable(String),
}

/// The protocol phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Building the machine-facing representation.
    CrossAgent,
    /// Blocked on the runtime. The model may not proceed from here.
    AwaitingRuntime,
    /// Cross-agent is complete; the human-facing prose may be written.
    Summarise,
    Done,
}

/// One liaison turn's accumulated state.
#[derive(Debug, Clone, Default)]
pub struct Liaison {
    pub intent: String,
    statements: Vec<Statement>,
    requests: Vec<Request>,
    phase_done: bool,
}

impl Liaison {
    pub fn new(intent: impl Into<String>) -> Self {
        Liaison { intent: intent.into(), ..Default::default() }
    }

    /// Record something the MODEL produced.
    ///
    /// Refuses organizational facts and known-emptiness outright, with the
    /// reason. This is the V0 fix, enforced by the type system rather than by
    /// asking the prompt nicely.
    pub fn model_says(&mut self, s: Statement) -> Result<(), String> {
        if !s.kind.model_may_assert() {
            return Err(format!(
                "a model turn cannot assert {} — that is an observation of the world and only \
the runtime can make it. Emit a REQUEST instead.", s.kind.tag()));
        }
        self.statements.push(s);
        Ok(())
    }

    /// Record something the RUNTIME supplied. Always permitted.
    pub fn runtime_says(&mut self, kind: Epistemic, text: impl Into<String>) {
        self.statements.push(Statement { kind, text: text.into() });
    }

    /// Ask the runtime for something.
    pub fn request(&mut self, verb: impl Into<String>) {
        let verb = verb.into();
        if !self.requests.iter().any(|r| r.verb == verb) {
            self.requests.push(Request { verb, answer: None });
        }
    }

    /// The runtime answers a request.
    ///
    /// `Unavailable` deliberately does NOT become `KnownEmpty`. "I could not
    /// find out" and "there is nothing" are different facts, and treating the
    /// first as the second is how absence gets invented.
    pub fn answer(&mut self, verb: &str, a: Answer) {
        let Some(r) = self.requests.iter_mut().find(|r| r.verb == verb) else { return };
        r.answer = Some(a.clone());
        match a {
            Answer::Facts(fs) => {
                for f in fs { self.statements.push(Statement {
                    kind: Epistemic::OrganizationFact, text: f }); }
            }
            Answer::Empty => self.statements.push(Statement {
                kind: Epistemic::KnownEmpty, text: format!("{verb}: nothing") }),
            Answer::Unavailable(why) => self.statements.push(Statement {
                kind: Epistemic::Unknown, text: format!("{verb}: unavailable ({why})") }),
        }
    }

    /// Requests still waiting.
    pub fn open_requests(&self) -> Vec<&str> {
        self.requests.iter().filter(|r| r.answer.is_none())
            .map(|r| r.verb.as_str()).collect()
    }

    /// May the protocol move past CROSS_AGENT?
    ///
    /// THE RUNTIME-ENFORCED BLOCK. The failure was a model that asked for
    /// information and designed the worker in the same turn; the note says the
    /// protocol must make that impossible. So this is not advice — `advance`
    /// consults it and refuses.
    pub fn may_proceed(&self) -> bool {
        self.open_requests().is_empty()
    }

    /// Move the phase on, or say why not.
    pub fn advance(&self, from: Phase) -> Result<Phase, String> {
        match from {
            Phase::CrossAgent if !self.may_proceed() => Err(format!(
                "cannot leave CROSS_AGENT with {} open request(s): {}. Asking and then \
proceeding anyway is not a request/response loop.",
                self.open_requests().len(), self.open_requests().join(", "))),
            Phase::CrossAgent => Ok(Phase::Summarise),
            Phase::AwaitingRuntime if self.may_proceed() => Ok(Phase::CrossAgent),
            Phase::AwaitingRuntime => Err("still waiting on the runtime".into()),
            Phase::Summarise => Ok(Phase::Done),
            Phase::Done => Ok(Phase::Done),
        }
    }

    fn by_kind(&self, k: Epistemic) -> Vec<&str> {
        self.statements.iter().filter(|s| s.kind == k).map(|s| s.text.as_str()).collect()
    }

    /// The machine-facing representation, in the spec's field order.
    pub fn cross_agent(&self) -> String {
        let mut o = String::new();
        o.push_str(&format!("INTENT:\n  {}\n", self.intent));
        let section = |o: &mut String, name: &str, items: Vec<&str>| {
            o.push_str(&format!("{name}:\n"));
            if items.is_empty() { o.push_str("  (none)\n"); }
            for i in items { o.push_str(&format!("  {i}\n")); }
        };
        section(&mut o, "OPERATOR_FACTS", self.by_kind(Epistemic::OperatorFact));
        section(&mut o, "REQUESTS", self.open_requests());
        let mut known = self.by_kind(Epistemic::OrganizationFact);
        known.extend(self.by_kind(Epistemic::KnownEmpty));
        section(&mut o, "KNOWN_STATE", known);
        section(&mut o, "RELEVANT_INFERENCES", self.by_kind(Epistemic::Inference));
        section(&mut o, "UNKNOWNS", self.by_kind(Epistemic::Unknown));
        o
    }

    /// The human-facing prose.
    ///
    /// Engineering prose with the labels stripped, and it must not invent
    /// progress: no assignment, investigation, implementation, completion or
    /// verification unless an ORGANIZATION_FACT establishes it. The spec's own
    /// preferred phrasing is *"I don't have the current renderer information
    /// yet"* — which is what an Unknown renders as.
    pub fn summarise(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        for s in self.by_kind(Epistemic::OperatorFact) {
            lines.push(format!("You've said {s}."));
        }
        let known = self.by_kind(Epistemic::OrganizationFact);
        for s in &known {
            lines.push(format!("{s}."));
        }
        for s in self.by_kind(Epistemic::KnownEmpty) {
            lines.push(format!("There is nothing recorded for {s}."));
        }
        for s in self.by_kind(Epistemic::Unknown) {
            // "yet", because the thing is unknown NOW and a request may be
            // outstanding — not unknowable.
            lines.push(format!("I don't have {s} yet."));
        }
        if known.is_empty() && !self.statements.is_empty() {
            // Said explicitly rather than left as an absence of good news. The
            // operator otherwise reads a paragraph with no claim of progress as
            // a paragraph that forgot to mention it.
            lines.push("Nothing has been evaluated or assigned yet.".into());
        }
        if lines.is_empty() { return "There is nothing to report.".into(); }
        lines.join(" ")
    }

    /// Every statement, grouped — for the introspection view.
    pub fn introspect(&self) -> BTreeMap<&'static str, Vec<&str>> {
        let mut m = BTreeMap::new();
        for k in [Epistemic::OperatorFact, Epistemic::OrganizationFact,
                  Epistemic::Inference, Epistemic::Unknown, Epistemic::KnownEmpty] {
            let v = self.by_kind(k);
            if !v.is_empty() { m.insert(k.tag(), v); }
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v0_a_model_cannot_invent_organizational_state() {
        // "I've reached out to our team…" — the model minting a fact about the
        // organization. Refused by construction, with the remedy named.
        let mut l = Liaison::new("migrate async networking to io_uring");
        let err = l.model_says(Statement {
            kind: Epistemic::OrganizationFact,
            text: "the team has been notified".into(),
        }).unwrap_err();
        assert!(err.contains("only the runtime"), "{err}");
        assert!(err.contains("REQUEST"), "the error should say what to do instead: {err}");
        assert!(l.introspect().get("ORGANIZATION_FACT").is_none());
    }

    #[test]
    fn v1_an_operator_statement_is_a_fact_not_an_unknown() {
        // The overcorrection: answering UNKNOWN to everything and refusing to
        // acknowledge what the operator plainly said. Recording it claims
        // nothing about the organization.
        let mut l = Liaison::new("io_uring");
        l.model_says(Statement::operator("you want io_uring for async networking")).unwrap();
        let s = l.summarise();
        assert!(s.contains("You've said"), "{s}");
        assert!(!s.starts_with("UNKNOWN"), "{s}");
    }

    #[test]
    fn v2_the_three_statuses_stay_separate() {
        // The target: "Operator wants X. X has not been evaluated or assigned.
        // Current implementation is unknown." Three epistemic statuses, none
        // pretending to be another.
        let mut l = Liaison::new("io_uring migration");
        l.model_says(Statement::operator("you want io_uring")).unwrap();
        l.model_says(Statement::unknown("the current async implementation")).unwrap();
        let s = l.summarise();
        assert!(s.contains("You've said"), "{s}");
        assert!(s.contains("I don't have"), "{s}");
        assert!(s.contains("Nothing has been evaluated or assigned"), "{s}");
        // And the machine view keeps them in separate sections.
        let x = l.cross_agent();
        assert!(x.contains("OPERATOR_FACTS:\n  you want io_uring"), "{x}");
        assert!(x.contains("UNKNOWNS:\n  the current async implementation"), "{x}");
    }

    #[test]
    fn it_cannot_invent_absence() {
        // "Since I don't have knowledge of the repository, I will assume there
        // are no relevant workers." Unknown and KnownEmpty are different
        // variants so that sentence has no spelling, and an Unavailable answer
        // stays Unknown rather than collapsing to Empty.
        let mut l = Liaison::new("design a worker");
        l.request("GET_ACTIVE_WORKERS");
        l.answer("GET_ACTIVE_WORKERS", Answer::Unavailable("registry offline".into()));
        let i = l.introspect();
        assert!(i.get("KNOWN_EMPTY").is_none(), "unavailable became known-empty: {i:?}");
        assert!(i.get("UNKNOWN").is_some());

        // And when the runtime really did look, it IS known-empty — the
        // distinction has to cut both ways or it is just pessimism.
        let mut l2 = Liaison::new("design a worker");
        l2.request("GET_ACTIVE_WORKERS");
        l2.answer("GET_ACTIVE_WORKERS", Answer::Empty);
        assert!(l2.introspect().get("KNOWN_EMPTY").is_some());
        assert!(l2.summarise().contains("nothing recorded"));
    }

    #[test]
    fn a_request_blocks_the_phase_rather_than_advising_against_proceeding() {
        // The failure was asking for information and designing the worker in
        // the same turn. "You need the runtime protocol to make this
        // impossible" — so it is a refusal, not a warning.
        let mut l = Liaison::new("design a worker");
        l.request("GET_ACTIVE_WORKERS");
        assert!(!l.may_proceed());
        let err = l.advance(Phase::CrossAgent).unwrap_err();
        assert!(err.contains("GET_ACTIVE_WORKERS"), "{err}");
        assert!(err.contains("not a request/response loop"), "{err}");

        l.answer("GET_ACTIVE_WORKERS", Answer::Facts(vec!["rust-ratatui-larva".into()]));
        assert!(l.may_proceed());
        assert_eq!(l.advance(Phase::CrossAgent).unwrap(), Phase::Summarise);
    }

    #[test]
    fn only_an_organization_fact_can_justify_a_claim_of_progress() {
        // The summarise rule: no assignment, investigation, implementation,
        // completion or verification unless organizational state establishes
        // it. With one, the disclaimer goes away; without, it appears.
        assert!(Epistemic::OrganizationFact.supports_progress());
        for k in [Epistemic::OperatorFact, Epistemic::Inference,
                  Epistemic::Unknown, Epistemic::KnownEmpty] {
            assert!(!k.supports_progress(), "{k:?} should not support a progress claim");
        }
        let mut l = Liaison::new("x");
        l.model_says(Statement::operator("you want io_uring")).unwrap();
        assert!(l.summarise().contains("Nothing has been evaluated"));
        l.runtime_says(Epistemic::OrganizationFact, "TASK-184 is assigned to rust-ratatui-larva");
        assert!(!l.summarise().contains("Nothing has been evaluated"),
                "a real organizational fact should retire the disclaimer");
    }

    #[test]
    fn the_summary_hides_the_labels_and_introspection_keeps_them() {
        // SUMMARISE is human-facing prose and should not expose internal
        // epistemic labels; INTROSPECT is the machine-facing view that does.
        let mut l = Liaison::new("x");
        l.model_says(Statement::operator("you want io_uring")).unwrap();
        l.model_says(Statement::inference("this likely touches the socket layer")).unwrap();
        let s = l.summarise();
        for tag in ["OPERATOR_FACT", "INFERENCE", "UNKNOWN"] {
            assert!(!s.contains(tag), "the prose leaked {tag}:\n{s}");
        }
        assert!(l.introspect().contains_key("INFERENCE"));
    }

    #[test]
    fn the_cross_agent_block_has_every_field_even_when_empty() {
        // A field that disappears when empty is a field a consumer cannot
        // parse, and the reader cannot tell "none" from "not asked".
        let l = Liaison::new("x");
        let x = l.cross_agent();
        for f in ["INTENT:", "OPERATOR_FACTS:", "REQUESTS:", "KNOWN_STATE:",
                  "RELEVANT_INFERENCES:", "UNKNOWNS:"] {
            assert!(x.contains(f), "missing {f}:\n{x}");
        }
        assert_eq!(x.matches("(none)").count(), 5);
    }

    #[test]
    fn asking_twice_is_one_request() {
        // A model that repeats itself should not deepen the block it is
        // already under.
        let mut l = Liaison::new("x");
        l.request("GET_ACTIVE_WORKERS");
        l.request("GET_ACTIVE_WORKERS");
        assert_eq!(l.open_requests().len(), 1);
    }
}
