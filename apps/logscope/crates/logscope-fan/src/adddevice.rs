//! adddevice — the flow for bringing a machine into the hive, as a state
//! machine a UI drives rather than a UI that owns the logic.
//!
//! Every piece this needs already existed and none of them were joined up:
//! [`crate::discovery`] finds machines on the LAN, [`crate::pairing`] turns
//! a public key into a typable checksummed code and explains a mistyped
//! one, [`crate::trustkit`] records what a peer is allowed to see. What was
//! missing was the ORDER — and the order is the whole product here, because
//! adding a device is where a person decides how much of their conversation
//! a machine in another room gets to read. Getting that wrong silently is
//! the failure worth designing against.
//!
//! ## Why a state machine and not a dialog
//!
//! Adding a device is a sequence with a decision in the middle that must
//! not be defaulted. Written as a dialog, "which trust level" becomes a
//! dropdown with a sensible-looking preselection, and the sensible-looking
//! preselection is how a build box ends up holding conversation history.
//! Written as a state machine, [`Step::ChooseTrust`] is a state the flow
//! cannot leave without an explicit answer, and that is enforceable here
//! and testable without a window.
//!
//! ## The confirm step is the security boundary
//!
//! A code typed by a person proves they can read the other machine's
//! screen. Discovery alone proves only that something answered on the
//! network, which is not the same claim at all. So the flow will admit a
//! peer at [`Trust::Build`] from discovery alone (source code, no
//! conversation), and requires a matching code before anything above it.
//! That rule lives in [`Flow::may_grant`] rather than in a UI, so no
//! screen can offer a level the flow would not.

use crate::trustkit::Trust;

/// Where the flow is. A UI renders the current step and sends [`Input`]s;
/// it never sets this directly.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Listening for machines that answer on the LAN.
    Scanning,
    /// Something was found and is waiting to be picked.
    Found { id: String },
    /// Type the code shown on the other machine's screen. `attempts`
    /// counts wrong tries, so a UI can say "check you are reading the
    /// right device" after a few rather than repeating one message.
    EnterCode { id: String, attempts: u32 },
    /// The code matched, or was skipped for a build-only peer. This is
    /// the step that must not be auto-answered.
    ChooseTrust { id: String, verified: bool },
    /// Admitted, with what it was granted.
    Done { id: String, level: Trust },
    /// Gave up or was dismissed. Carries why, because "nothing happened"
    /// is the least useful thing a flow can report.
    Abandoned { why: String },
}

/// What a UI can tell the flow.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// Discovery reported a peer.
    Discovered { id: String },
    /// The person picked one of the found machines.
    Pick { id: String },
    /// A typed pairing code, against the peer's real public key.
    Code { typed: String, pubkey: Vec<u8> },
    /// Skip verification and admit as a build-only worker.
    SkipToBuildOnly,
    /// The trust decision.
    Grant(Trust),
    Cancel,
}

/// What the UI should say right now. Returned rather than stored so it
/// cannot go stale against the step.
#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    pub title: String,
    pub detail: String,
    /// True when the flow is waiting on a person rather than the network —
    /// a spinner is wrong in that case and a spinner shown anyway is how
    /// a flow looks broken when it is merely waiting.
    pub needs_answer: bool,
}

pub struct Flow {
    pub step: Step,
}

impl Default for Flow {
    fn default() -> Self {
        Flow { step: Step::Scanning }
    }
}

impl Flow {
    pub fn new() -> Flow {
        Flow::default()
    }

    /// The highest level this peer may be granted given what has been
    /// PROVEN about it, independent of what a UI offers.
    ///
    /// Unverified means "something answered on the network". That is
    /// enough to hand it source and let it run a build, and not enough to
    /// hand it a conversation — the two differ by whether a stranger on
    /// the LAN gets to read what you have been saying, which is not a
    /// difference a dropdown default should be making.
    pub fn may_grant(verified: bool) -> Trust {
        if verified { Trust::Handoff } else { Trust::Build }
    }

    /// Is this level allowed at this point in the flow?
    pub fn allows(verified: bool, want: Trust) -> bool {
        want <= Flow::may_grant(verified)
    }

    pub fn prompt(&self) -> Prompt {
        match &self.step {
            Step::Scanning => Prompt {
                title: "Looking for machines".into(),
                detail: "Anything running logscope on this network will appear here.".into(),
                needs_answer: false,
            },
            Step::Found { id } => Prompt {
                title: format!("Found {id}"),
                detail: "Add it, or keep looking.".into(),
                needs_answer: true,
            },
            Step::EnterCode { id, attempts } => Prompt {
                title: format!("Type the code shown on {id}"),
                detail: if *attempts == 0 {
                    "It is on that machine's screen, under Devices.".into()
                } else {
                    format!(
                        "{attempts} attempt(s) did not match. Check you are reading the code \
                         from {id} and not from another machine."
                    )
                },
                needs_answer: true,
            },
            Step::ChooseTrust { id, verified } => Prompt {
                title: format!("What may {id} see?"),
                detail: if *verified {
                    "The code matched, so this machine can be given anything up to holding \
                     the conversation."
                        .into()
                } else {
                    "Unverified: it answered on the network, but nobody has confirmed which \
                     machine it is. It can run builds — source only — and nothing more."
                        .into()
                },
                needs_answer: true,
            },
            Step::Done { id, level } => Prompt {
                title: format!("{id} added"),
                detail: format!("Granted: {level:?}."),
                needs_answer: false,
            },
            Step::Abandoned { why } => Prompt {
                title: "Not added".into(),
                detail: why.clone(),
                needs_answer: false,
            },
        }
    }

    /// Advance. Returns the peer and level to register when the flow
    /// completes, so the caller performs the registration and this stays
    /// free of a registry it would otherwise have to own.
    pub fn on(&mut self, input: Input) -> Option<(String, Trust)> {
        match (&self.step.clone(), input) {
            (_, Input::Cancel) => {
                self.step = Step::Abandoned { why: "cancelled".into() };
                None
            }
            (Step::Scanning, Input::Discovered { id }) => {
                self.step = Step::Found { id };
                None
            }
            (Step::Found { .. }, Input::Pick { id }) => {
                self.step = Step::EnterCode { id, attempts: 0 };
                None
            }
            (Step::EnterCode { id, attempts }, Input::Code { typed, pubkey }) => {
                if crate::pairing::matches(&typed, &pubkey) {
                    self.step = Step::ChooseTrust { id: id.clone(), verified: true };
                } else {
                    // Stay on the step and count. A wrong code is almost
                    // always a misread, and pairing::parse already explains
                    // which character — dropping back to scanning would
                    // throw that away and make the person start over for a
                    // typo.
                    self.step = Step::EnterCode { id: id.clone(), attempts: attempts + 1 };
                }
                None
            }
            (Step::EnterCode { id, .. }, Input::SkipToBuildOnly) => {
                self.step = Step::ChooseTrust { id: id.clone(), verified: false };
                None
            }
            (Step::ChooseTrust { id, verified }, Input::Grant(level)) => {
                if !Flow::allows(*verified, level) {
                    // Refused rather than silently downgraded: a person who
                    // asked for Handoff and got Build without being told
                    // would believe the wrong thing about their own network.
                    self.step = Step::Abandoned {
                        why: format!(
                            "{id} was never verified, so it cannot be given {level:?}. \
                             Type its code, or add it as a build worker."
                        ),
                    };
                    return None;
                }
                self.step = Step::Done { id: id.clone(), level };
                Some((id.clone(), level))
            }
            _ => None, // an input that does not apply to this step is ignored
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A key and its real code, so the happy path proves the pairing
    // module is actually being consulted rather than a string compared.
    fn key() -> Vec<u8> {
        (0u8..32).collect()
    }

    #[test]
    fn the_whole_path_from_discovery_to_admitted() {
        let mut f = Flow::new();
        assert_eq!(f.step, Step::Scanning);
        f.on(Input::Discovered { id: "attic-box".into() });
        f.on(Input::Pick { id: "attic-box".into() });
        assert!(matches!(f.step, Step::EnterCode { .. }));

        let code = crate::pairing::code_for(&key());
        f.on(Input::Code { typed: code, pubkey: key() });
        assert_eq!(f.step, Step::ChooseTrust { id: "attic-box".into(), verified: true });

        let out = f.on(Input::Grant(Trust::Handoff));
        assert_eq!(out, Some(("attic-box".to_string(), Trust::Handoff)));
    }

    #[test]
    fn an_unverified_peer_cannot_be_given_the_conversation() {
        // THE SECURITY BOUNDARY. Discovery proves something answered on
        // the network; it does not prove which machine it is. A UI must
        // not be able to offer Handoff here, so the refusal lives in the
        // flow rather than in whatever screen is asking.
        let mut f = Flow::new();
        f.on(Input::Discovered { id: "unknown".into() });
        f.on(Input::Pick { id: "unknown".into() });
        f.on(Input::SkipToBuildOnly);
        assert_eq!(f.step, Step::ChooseTrust { id: "unknown".into(), verified: false });

        assert_eq!(f.on(Input::Grant(Trust::Handoff)), None, "handoff was granted unverified");
        assert!(matches!(f.step, Step::Abandoned { .. }));
        // And it says WHY — a silent downgrade would leave the person
        // believing the wrong thing about their own network.
        assert!(f.prompt().detail.contains("never verified"), "{:?}", f.prompt());
    }

    #[test]
    fn an_unverified_peer_can_still_run_builds() {
        // The flow must not be all-or-nothing: a build box that sees
        // source and no conversation is genuinely useful and is the
        // common case on a home LAN.
        let mut f = Flow::new();
        f.on(Input::Discovered { id: "buildbox".into() });
        f.on(Input::Pick { id: "buildbox".into() });
        f.on(Input::SkipToBuildOnly);
        assert_eq!(
            f.on(Input::Grant(Trust::Build)),
            Some(("buildbox".to_string(), Trust::Build))
        );
    }

    #[test]
    fn a_mistyped_code_keeps_the_step_and_counts() {
        // A typo must not throw the person back to scanning. The count
        // exists so the message can escalate from "it's on the screen" to
        // "check you are reading the right machine", which is the actual
        // second cause.
        let mut f = Flow::new();
        f.on(Input::Discovered { id: "attic-box".into() });
        f.on(Input::Pick { id: "attic-box".into() });
        f.on(Input::Code { typed: "WRONG-CODE-HERE".into(), pubkey: key() });
        match &f.step {
            Step::EnterCode { attempts, id } => {
                assert_eq!(*attempts, 1);
                assert_eq!(id, "attic-box", "still the same device");
            }
            other => panic!("dropped out of the code step: {other:?}"),
        }
        f.on(Input::Code { typed: "ALSO-WRONG".into(), pubkey: key() });
        assert!(matches!(f.step, Step::EnterCode { attempts: 2, .. }));
        assert!(f.prompt().detail.contains("2 attempt"), "{:?}", f.prompt());

        // The right code still works after failures.
        f.on(Input::Code { typed: crate::pairing::code_for(&key()), pubkey: key() });
        assert!(matches!(f.step, Step::ChooseTrust { verified: true, .. }));
    }

    #[test]
    fn the_unverified_ceiling_is_below_anything_that_reads_conversation() {
        // The line that matters in `Trust` is not between Compute and
        // Handoff — it is between Build and Compute. Build "sees source,
        // not conversation"; Compute "sees prompts and replies". Any
        // grant path that starts from something unverified (a typed
        // machine id, a discovery hit) must stop below that line, or one
        // keystroke authorises reading the conversation.
        //
        // Asserted as an inequality rather than as `== Build` so it keeps
        // meaning if a level is inserted between them later.
        assert!(
            Flow::may_grant(false) < Trust::Compute,
            "an unverified peer may reach a level that reads conversation"
        );
        assert!(Flow::allows(false, Trust::Build));
        assert!(!Flow::allows(false, Trust::Compute));
        assert!(!Flow::allows(false, Trust::Handoff));
        // Verified clears everything, which is the point of typing a code.
        for want in [Trust::Build, Trust::Compute, Trust::Handoff] {
            assert!(Flow::allows(true, want), "{want:?} refused after verification");
        }
    }

    #[test]
    fn cancel_works_from_every_step_and_says_so() {
        for setup in 0..3 {
            let mut f = Flow::new();
            if setup > 0 {
                f.on(Input::Discovered { id: "x".into() });
            }
            if setup > 1 {
                f.on(Input::Pick { id: "x".into() });
            }
            f.on(Input::Cancel);
            assert!(matches!(f.step, Step::Abandoned { .. }), "setup {setup}");
        }
    }

    #[test]
    fn the_prompt_says_whether_it_is_waiting_on_a_person_or_the_network() {
        // A spinner over a step that is waiting for typing is how a flow
        // looks broken when it is merely waiting.
        let mut f = Flow::new();
        assert!(!f.prompt().needs_answer, "scanning waits on the network");
        f.on(Input::Discovered { id: "x".into() });
        assert!(f.prompt().needs_answer, "a found device waits on a person");
        f.on(Input::Pick { id: "x".into() });
        assert!(f.prompt().needs_answer, "the code step waits on a person");
    }

    #[test]
    fn inputs_that_do_not_apply_to_the_step_are_ignored() {
        // A stray Discovered while choosing trust must not reset the flow
        // and lose the decision in progress — discovery keeps running in
        // the background and will fire whenever it likes.
        let mut f = Flow::new();
        f.on(Input::Discovered { id: "a".into() });
        f.on(Input::Pick { id: "a".into() });
        f.on(Input::SkipToBuildOnly);
        let before = f.step.clone();
        f.on(Input::Discovered { id: "b".into() });
        assert_eq!(f.step, before, "a background discovery reset the flow");
    }
}
