//! Should speculative drafting be on, without anyone typing a flag?
//!
//! `LOGSCOPE_SPEC_DRAFT=lookup` is real, measured and free — no second
//! checkpoint, and at temperature 0 the output is identical to plain decoding.
//! It is also a flag, which means it is on for whoever read the log line
//! recommending it and off for everyone else. A lever that only the author
//! pulls is not a feature, and the engine already tells the operator to pull
//! this one, which is the tell that it should have pulled it itself.
//!
//! This is the decision, as a pure function, so the policy is testable without
//! a GPU and arguable without reading the backend.
//!
//! ## Why temperature is the hard gate
//!
//! The verifier ([`crate::spec::accepted_run`]) is GREEDY: it accepts drafted
//! tokens while they equal the verifier's own, and commits the verifier's
//! tokens. So the committed sequence is exactly what the model produced — but
//! only if "what the model produced" is deterministic. At temperature 0 it is
//! (argmax), so speculation is bit-identical and cannot change a single token.
//! Above 0 the verifier samples, and a speculative run consumes randomness in a
//! different order: the output is still a valid draw from the same
//! distribution, but it is not the same draw. Re-running a prompt and getting
//! different text because a performance heuristic engaged is exactly the
//! surprise that got `LOGSCOPE_PASS_FUSE` retracted.
//!
//! So: auto-enable at temperature 0 only. Above it, the operator can still ask
//! explicitly — they are then choosing a different sample knowingly, which is
//! a different thing from the engine choosing it for them.
//!
//! Note this corrects an overstatement in the codebase: `backend.rs` said
//! "output is identical to plain decoding, only the speed differs" with no
//! temperature qualifier, while `perfverdict.rs` said "identical at
//! temperature 0". Both described the same verifier and only one was right.

/// What the engine decided, and why — the reason is printed, because a
/// heuristic that silently changes performance characteristics is one nobody
/// can debug.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    On(String),
    Off(String),
}

impl Decision {
    pub fn is_on(&self) -> bool {
        matches!(self, Decision::On(_))
    }
    pub fn why(&self) -> &str {
        match self {
            Decision::On(s) | Decision::Off(s) => s,
        }
    }
}

/// Acceptance below this is not worth the verification slot. A drafter that
/// lands under a fifth of its guesses still never makes output worse (a
/// rejected step commits the verifier's own token), but it spends a wider
/// forward pass to do it, and on prose it will sit here.
pub const MIN_ACCEPT_RATE: f64 = 0.20;

/// Decode this many times slower per token than the model's own prefill is the
/// signature the drafter addresses: nothing in flight to cover memory latency.
/// Measured on the reported run at 23.6x; a model that is merely 3x is not
/// starved in the same way and gains little.
pub const PHASE_RATIO_TRIGGER: f64 = 6.0;

/// Decide.
///
/// * `explicit` — the env var, if set. Always wins, in both directions: an
///   operator who typed `0` gets it off even where the heuristic would enable
///   it, and one who typed `lookup` at temperature 0.8 gets it on with the
///   sampling caveat their problem.
/// * `temperature` — the hard gate above.
/// * `phase_ratio` — decode ms/token ÷ prefill ms/token, if measured.
/// * `accept_rate` — this model's remembered acceptance, if it has run before.
pub fn decide(
    explicit: Option<&str>,
    temperature: f32,
    phase_ratio: Option<f64>,
    accept_rate: Option<f64>,
) -> Decision {
    if let Some(e) = explicit {
        let e = e.trim();
        return match e {
            "0" | "off" | "false" | "" => {
                Decision::Off(format!("LOGSCOPE_SPEC_DRAFT={e} — off by request"))
            }
            other => Decision::On(format!("LOGSCOPE_SPEC_DRAFT={other} — on by request")),
        };
    }
    if temperature > 0.0 {
        return Decision::Off(format!(
            "temperature {temperature:.2} > 0: the verifier samples, so speculation would return a \
             different (equally valid) draw and the same prompt would stop reproducing. \
             LOGSCOPE_SPEC_DRAFT=lookup enables it anyway"
        ));
    }
    // A remembered poor acceptance outranks the phase signal: the phase ratio
    // says the machine WOULD benefit, acceptance says this workload does not.
    if let Some(r) = accept_rate {
        if r < MIN_ACCEPT_RATE {
            return Decision::Off(format!(
                "measured acceptance {:.0}% on this model is below {:.0}% — drafting would cost a \
                 wider forward pass for tokens mostly rejected",
                r * 100.0,
                MIN_ACCEPT_RATE * 100.0
            ));
        }
        return Decision::On(format!(
            "temperature 0 (bit-identical) and measured acceptance {:.0}% on this model",
            r * 100.0
        ));
    }
    match phase_ratio {
        Some(p) if p >= PHASE_RATIO_TRIGGER => Decision::On(format!(
            "temperature 0 (bit-identical) and decode is {p:.1}x slower per token than this \
             model's own prefill — the starved-decode signature drafting addresses"
        )),
        Some(p) => Decision::Off(format!(
            "decode is only {p:.1}x prefill (trigger {PHASE_RATIO_TRIGGER:.0}x) — not \
             latency-starved enough for drafting to pay"
        )),
        None => Decision::Off(
            "no phase measurement yet — drafting decides after the first turn, so the first \
             turn is never slowed by a guess"
                .into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_setting_wins_in_both_directions() {
        // Someone who typed a value meant it. Off must survive a machine the
        // heuristic loves, and on must survive a temperature that would gate it.
        assert!(!decide(Some("0"), 0.0, Some(50.0), Some(0.9)).is_on());
        assert!(decide(Some("lookup"), 0.8, None, None).is_on());
    }

    #[test]
    fn temperature_above_zero_gates_it_and_says_why() {
        let d = decide(None, 0.35, Some(23.6), Some(0.9));
        assert!(!d.is_on(), "{d:?}");
        assert!(d.why().contains("different"), "{}", d.why());
        // And the message must name the escape hatch, or the operator has no
        // way to act on a refusal they disagree with.
        assert!(d.why().contains("LOGSCOPE_SPEC_DRAFT=lookup"), "{}", d.why());
    }

    #[test]
    fn the_reported_run_would_have_enabled_it() {
        // The field case: 12.69 tok/s, decode 23.6x its own prefill, greedy.
        // If this does not turn on, the heuristic does not address the thing
        // that motivated it.
        let d = decide(None, 0.0, Some(23.6), None);
        assert!(d.is_on(), "{d:?}");
        assert!(d.why().contains("23.6x"), "{}", d.why());
    }

    #[test]
    fn a_healthy_decode_is_left_alone() {
        assert!(!decide(None, 0.0, Some(2.0), None).is_on());
    }

    #[test]
    fn a_remembered_bad_acceptance_outranks_a_tempting_phase_ratio() {
        // The phase ratio says the MACHINE would benefit; acceptance says this
        // WORKLOAD does not. Prose is the case: starved decode, drafter whiffs.
        let d = decide(None, 0.0, Some(23.6), Some(0.05));
        assert!(!d.is_on(), "{d:?}");
        assert!(d.why().contains("acceptance"), "{}", d.why());
    }

    #[test]
    fn a_remembered_good_acceptance_enables_without_a_phase_measurement() {
        assert!(decide(None, 0.0, None, Some(0.6)).is_on());
    }

    #[test]
    fn the_first_turn_is_never_slowed_by_a_guess() {
        // Nothing measured: stay off and decide next turn. Turning on
        // speculatively would put a cold drafter in front of time-to-first-token,
        // which is the one number a user feels immediately.
        let d = decide(None, 0.0, None, None);
        assert!(!d.is_on(), "{d:?}");
        assert!(d.why().contains("first turn"), "{}", d.why());
    }

    #[test]
    fn every_decision_carries_a_reason() {
        for d in [
            decide(None, 0.0, None, None),
            decide(None, 0.9, None, None),
            decide(None, 0.0, Some(23.6), None),
            decide(Some("0"), 0.0, None, None),
        ] {
            assert!(d.why().len() > 20, "a bare verdict is undebuggable: {d:?}");
        }
    }
}
