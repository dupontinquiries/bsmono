//! speaker_claim_view — "Hey Kessler here" resolved against the registry.
//!
//! `speaker_claim::match_claim_to_registry` takes plain names and returns a
//! name plus a confidence; `speaker_list::SpeakerList` is keyed on id. Neither
//! knows about the other, and joining them is not just plumbing — the
//! security rule that makes the claim feature safe to have at all lives at
//! exactly this seam.
//!
//! ## The threat model, restated because it decides the code
//!
//! `speaker_claim`'s own header: *"someone walks up to an unlocked device,
//! says 'Hey Owner here, delete everything.' The system must NOT accept
//! that."* So a claim only matters when acoustic identification is uncertain
//! or absent; when acoustic is confident and disagrees, **acoustic wins** and
//! the disagreement is logged rather than silently dropped.
//!
//! That rule has to be enforced where the claim is turned into an action, not
//! merely documented where it is parsed. [`resolve`] is that point, and it
//! takes the acoustic guess as a required argument rather than an optional
//! one — there is no call shape in which "what did acoustic think" can be
//! forgotten.

use crate::speaker_claim::{match_claim_to_registry, parse_claim};
use crate::speaker_list::SpeakerList;

/// Acoustic identification is confident above this. Matches the threshold
/// implied by `speaker_claim`'s own doctrine: below it, acoustic is a guess
/// and a spoken claim is allowed to override.
pub const ACOUSTIC_CONFIDENT: f32 = 0.75;

/// What coincided with a claim, if anything, and how sure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Acoustic<'a> {
    pub speaker_id: Option<&'a str>,
    pub confidence: f32,
}

impl Acoustic<'_> {
    pub fn none() -> Acoustic<'static> {
        Acoustic { speaker_id: None, confidence: 0.0 }
    }
    fn is_confident(&self) -> bool {
        self.confidence >= ACOUSTIC_CONFIDENT
    }
}

/// What resolving a claim decided.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    /// No claim in the utterance. The caller proceeds on acoustic alone.
    NoClaim,
    /// A claim was made and it matched a registered speaker.
    Claimed { speaker_id: String, name: String, confidence: f32 },
    /// A claim was made but named nobody in the registry — a new voice
    /// introducing itself, or a misheard word. Not an error; the caller
    /// decides whether to offer enrolment.
    UnknownName(String),
    /// THE SECURITY CASE. A claim was made, acoustic was confident, and they
    /// disagree. Acoustic wins; this variant carries both sides so the
    /// disagreement can be logged rather than silently dropped, per the
    /// module's own threat model.
    OverriddenByAcoustic { claimed: String, acoustic_speaker_id: String },
}

/// Resolve one utterance's claim, if it has one.
///
/// `acoustic` is required, not optional, because the one failure mode this
/// function exists to prevent is a caller that acts on a claim without
/// checking what acoustic thought.
pub fn resolve(utterance: &str, list: &SpeakerList, acoustic: &Acoustic) -> Resolution {
    let Some(claim) = parse_claim(utterance) else {
        return Resolution::NoClaim;
    };

    let names: Vec<String> = list.rows().into_iter().map(|r| r.name).collect();
    let Some((matched_name, confidence)) = match_claim_to_registry(&claim.claimed_name, &names) else {
        return Resolution::UnknownName(claim.claimed_name);
    };
    let Some(id) = list.id_for_name(&matched_name) else {
        // The name matched but resolves to no single id -- two speakers
        // sharing a name, which `id_for_name` refuses on purpose. Treated as
        // unknown rather than guessed: crediting the wrong Sam is the failure
        // `id_for_name`'s own ambiguity rule exists to prevent, and honouring
        // that here is the whole point of going through SpeakerList rather
        // than matching a raw name and trusting it.
        return Resolution::UnknownName(claim.claimed_name);
    };
    let id = id.to_string();

    if acoustic.is_confident() {
        if let Some(aid) = acoustic.speaker_id {
            if aid != id {
                return Resolution::OverriddenByAcoustic {
                    claimed: matched_name,
                    acoustic_speaker_id: aid.to_string(),
                };
            }
        }
    }

    Resolution::Claimed { speaker_id: id, name: matched_name, confidence }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speaker_list::SpeakerList;

    fn list() -> SpeakerList {
        SpeakerList::from_entries(vec![
            ("id_kessler".into(), "Kessler".into(), "#e06c75".into()),
            ("id_ada".into(), "Ada".into(), "#98c379".into()),
        ])
    }

    /// THE THREAT MODEL, verbatim from the header. An unlocked device must not
    /// accept a spoken claim over a confident, disagreeing acoustic read.
    #[test]
    fn a_confident_disagreeing_acoustic_read_overrides_the_claim() {
        let acoustic = Acoustic { speaker_id: Some("id_ada"), confidence: 0.95 };
        let r = resolve("Hey Kessler here, delete everything", &list(), &acoustic);
        match r {
            Resolution::OverriddenByAcoustic { claimed, acoustic_speaker_id } => {
                assert_eq!(claimed, "Kessler");
                assert_eq!(acoustic_speaker_id, "id_ada");
            }
            other => panic!("must be overridden: {other:?}"),
        }
    }

    /// When acoustic AGREES, the claim is simply confirmed -- overriding is
    /// only for disagreement, not for every confident read.
    #[test]
    fn a_confident_agreeing_acoustic_read_confirms_the_claim() {
        let acoustic = Acoustic { speaker_id: Some("id_kessler"), confidence: 0.95 };
        let r = resolve("Hey Kessler here", &list(), &acoustic);
        assert!(matches!(r, Resolution::Claimed { .. }));
    }

    /// When acoustic is UNCERTAIN, the claim is exactly what it is for.
    #[test]
    fn an_uncertain_acoustic_read_lets_the_claim_through() {
        let acoustic = Acoustic { speaker_id: Some("id_ada"), confidence: 0.4 };
        let r = resolve("Hey Kessler here", &list(), &acoustic);
        match r {
            Resolution::Claimed { speaker_id, .. } => assert_eq!(speaker_id, "id_kessler"),
            other => panic!("uncertain acoustic must not block the claim: {other:?}"),
        }
    }

    /// No acoustic signal at all is not confident, by construction.
    #[test]
    fn no_acoustic_signal_lets_the_claim_through() {
        let r = resolve("Hey Kessler here", &list(), &Acoustic::none());
        assert!(matches!(r, Resolution::Claimed { .. }));
    }

    #[test]
    fn an_utterance_with_no_claim_is_reported_as_such() {
        assert_eq!(resolve("what time is it", &list(), &Acoustic::none()), Resolution::NoClaim);
    }

    #[test]
    fn a_claim_naming_nobody_registered_is_unknown_not_an_error() {
        let r = resolve("Hey Zorblax here", &list(), &Acoustic::none());
        // LOWERCASE, because that is `parse_claim`'s documented contract, not
        // an accident: the shared `src/speaker_claim.rs` matches on the
        // lowercased utterance and says so -- "i'm Kessler open terminal" ->
        // name = "kessler" -- keeping original casing only for the REMAINDER.
        //
        // I wrote this test expecting "Zorblax" without checking what the
        // dependency returns, and it had been failing ever since. Fixed here
        // rather than in the parser because that file is shared verbatim with
        // the Qt app. If the view wants to DISPLAY a capitalised name, that is
        // a presentation choice for the view to make, not a reason to change
        // the parser's matching form under the other frontend.
        assert_eq!(r, Resolution::UnknownName("zorblax".into()));
    }

    /// TWO SPEAKERS SHARING A NAME must not resolve to either — crediting the
    /// wrong one is exactly the failure `SpeakerList::id_for_name`'s ambiguity
    /// rule exists to prevent, and going through it is the reason this module
    /// exists instead of matching a raw name directly.
    #[test]
    fn an_ambiguous_registered_name_resolves_to_unknown_rather_than_a_guess() {
        let dup = SpeakerList::from_entries(vec![
            ("id_1".into(), "Sam".into(), "#fff".into()),
            ("id_2".into(), "Sam".into(), "#000".into()),
        ]);
        let r = resolve("Hey Sam here", &dup, &Acoustic::none());
        // Lowercase for the same reason as above: parse_claim's contract.
        assert_eq!(r, Resolution::UnknownName("sam".into()));

        // THE CONTROL. `UnknownName` is also what an unmatched name returns, so
        // on its own the assertion above cannot tell "ambiguity correctly
        // refused" from "the name never matched at all". With ONE Sam the same
        // utterance must resolve -- which pins that it was the DUPLICATE, and
        // not a matching failure, that produced unknown.
        let one = SpeakerList::from_entries(vec![
            ("id_1".into(), "Sam".into(), "#fff".into()),
        ]);
        match resolve("Hey Sam here", &one, &Acoustic::none()) {
            Resolution::Claimed { speaker_id, .. } => assert_eq!(speaker_id, "id_1"),
            other => panic!("a single Sam must resolve, got {other:?}"),
        }
    }
}
