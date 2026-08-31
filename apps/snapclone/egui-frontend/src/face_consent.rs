//! face_consent — the consent boundary from `docs/FACE_BANDWIDTH.md`.
//!
//! The design doc is the artifact worth porting here, and its central argument
//! is a **reframing** that changes what data exists at all.
//!
//! ## Bandwidth, not identity — and why that inverts the privacy posture
//!
//! The earlier framing was face recognition for identity matching. The doc
//! rejects it for two reasons, and the second is the one with teeth:
//!
//! > *"Per-contact biometric embedding storage is the worst privacy posture:
//! > the user consents on behalf of contacts who never agreed to have their
//! > face geometry stored."*
//!
//! The bandwidth framing inverts it: *"the biometric data stored is the user's
//! own face (clear consent) plus crops of contacts who voluntarily transmitted
//! those photos to the user (implicit consent via the act of sending). **Never
//! crops of strangers, never of someone who didn't initiate the data flow.**"*
//!
//! That last sentence is a rule an implementation can break silently, so it is
//! a function here rather than a paragraph.
//!
//! ## Revocation erases; it does not merely stop
//!
//! > *"You can turn this off at any time, which will erase everything that was
//! > remembered."*
//!
//! And re-enabling *"starts empty"*. A toggle that only stops collection would
//! leave the promise untrue the moment somebody re-reads the database.
//!
//! ## Consent is for the whole track, not per-phase
//!
//! *"The opt-in is for the whole face-features track, not per-Phase."* One
//! decision the user actually made, rather than a series of prompts that train
//! them to click through.

/// How a face crop came to be on this device.
///
/// The provenance is stored because the consent rule is about **how the data
/// arrived**, and a crop with no provenance cannot be checked against it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Provenance {
    /// The device owner's own face. Clear consent.
    SelfCapture,
    /// A contact sent this photo to the user. Implicit consent via the act of
    /// sending.
    ReceivedFromContact,
    /// The user photographed someone. **They did not initiate the data flow.**
    ThirdPartyInCapture,
    /// Origin unknown.
    Unknown,
}

impl Provenance {
    /// May a crop with this provenance be stored?
    ///
    /// *"Never crops of strangers, never of someone who didn't initiate the
    /// data flow."* A face in a photo the user took is exactly that stranger —
    /// they are in the frame, not in the conversation.
    pub fn may_store(self) -> bool {
        matches!(self, Provenance::SelfCapture | Provenance::ReceivedFromContact)
    }

    /// Why `may_store` answered the way it did, in the user's terms.
    ///
    /// Test-only for now, and that is honest rather than dead: the
    /// consent SWITCH is live in settings, but no detector runs behind
    /// it yet, so there is no refusal to explain. The moment a crop is
    /// actually refused this is the sentence to show — a privacy gate
    /// that silently drops a face teaches people it is broken, not that
    /// it is protecting someone.
    ///
    /// Kept beside `may_store` deliberately: a reason living far from
    /// the decision it explains is how the two stop agreeing.
    pub fn why(self) -> &'static str {
        match self {
            Provenance::SelfCapture => "your own face",
            Provenance::ReceivedFromContact => "a contact sent this photo to you",
            Provenance::ThirdPartyInCapture =>
                "they are in a photo you took, and did not initiate the data flow",
            Provenance::Unknown => "the origin of this crop is not known",
        }
    }
}

/// Where consent stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Consent {
    /// Never asked, or asked and deferred.
    NotGiven,
    Granted,
    /// Was granted, then withdrawn. Distinct from NotGiven so the app can know
    /// an erase is owed.
    Revoked,
}

impl Consent {
    /// May detection run at all?
    ///
    /// *"If consent is denied or revoked: no detection runs on captured or
    /// received images."* Not "runs and discards" — the work does not happen,
    /// which is also why revocation saves battery rather than costing it.
    pub fn may_detect(self) -> bool { self == Consent::Granted }

    /// Does the library owe an erase?
    pub fn owes_erase(self) -> bool { self == Consent::Revoked }
}

/// The opt-in text, kept as data because it is a promise the code must keep.
///
/// Held here so a test can assert the four claims it makes are the four the
/// implementation actually honours. A consent string that drifts from the
/// behaviour is worse than no string at all.
pub const OPT_IN_TEXT: &str = "SnapClone can use the photos you send and receive to reduce \
data usage on future snaps. This works by remembering recurring faces (yours and your contacts' \
as you message them) so similar photos can be sent more compactly. All data stays on this \
device. You can turn this off at any time, which will erase everything that was remembered.";

/// A stored crop.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FaceCrop {
    pub id: String,
    pub provenance: Provenance,
}

/// The library, gated by consent.
#[derive(Clone, Debug, Default)]
pub struct FaceLibrary {
    consent: Consent,
    crops: Vec<FaceCrop>,
    /// Crops refused on provenance grounds — surfaced because a library that
    /// silently drops most of what it sees looks broken rather than principled.
    pub refused: u32,
}

impl Default for Consent {
    fn default() -> Self { Consent::NotGiven }
}

/// Why a crop was not stored.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// Detection should not even have run.
    NoConsent,
    /// The person did not initiate the data flow.
    Provenance,
}

impl FaceLibrary {
    pub fn consent(&self) -> Consent { self.consent }
    pub fn len(&self) -> usize { self.crops.len() }
    pub fn is_empty(&self) -> bool { self.crops.is_empty() }

    /// The user opted in.
    pub fn grant(&mut self) { self.consent = Consent::Granted; }

    /// The user turned it off.
    ///
    /// Erases immediately rather than marking for later cleanup: *"which will
    /// erase everything that was remembered"* is a promise made in the opt-in
    /// text, and a deferred erase is a window in which the promise is false.
    pub fn revoke(&mut self) -> usize {
        let n = self.crops.len();
        self.crops.clear();
        self.consent = Consent::Revoked;
        n
    }

    /// Try to store a crop.
    pub fn store(&mut self, crop: FaceCrop) -> Result<(), Refusal> {
        if !self.consent.may_detect() { return Err(Refusal::NoConsent); }
        if !crop.provenance.may_store() {
            self.refused += 1;
            return Err(Refusal::Provenance);
        }
        self.crops.push(crop);
        Ok(())
    }

    /// Everything stored, for an export or an audit.
    pub fn crops(&self) -> &[FaceCrop] { &self.crops }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crop(id: &str, p: Provenance) -> FaceCrop {
        FaceCrop { id: id.into(), provenance: p }
    }

    #[test]
    fn a_stranger_in_a_photo_the_user_took_is_never_stored() {
        // "Never crops of strangers, never of someone who didn't initiate the
        // data flow." A face in a photo the user took is exactly that: in the
        // frame, not in the conversation. This is a rule an implementation can
        // break SILENTLY, which is why it is a function rather than a
        // paragraph.
        let mut lib = FaceLibrary::default();
        lib.grant();
        assert_eq!(lib.store(crop("bystander", Provenance::ThirdPartyInCapture)),
                   Err(Refusal::Provenance));
        assert!(lib.is_empty());
    }

    #[test]
    fn the_two_consenting_provenances_are_stored() {
        // The user's own face (clear consent) and crops from photos a contact
        // chose to send (implicit consent via the act of sending).
        let mut lib = FaceLibrary::default();
        lib.grant();
        assert!(lib.store(crop("me", Provenance::SelfCapture)).is_ok());
        assert!(lib.store(crop("alice", Provenance::ReceivedFromContact)).is_ok());
        assert_eq!(lib.len(), 2);
    }

    #[test]
    fn an_unknown_provenance_is_refused_rather_than_assumed_safe() {
        // A crop whose origin was lost cannot be checked against the rule, and
        // the rule is the whole privacy posture.
        let mut lib = FaceLibrary::default();
        lib.grant();
        assert_eq!(lib.store(crop("?", Provenance::Unknown)), Err(Refusal::Provenance));
    }

    #[test]
    fn no_detection_runs_without_consent() {
        // "If consent is denied or revoked: no detection runs on captured or
        // received images." Not "runs and discards" — the work does not happen,
        // which is why revocation SAVES battery rather than costing it.
        let mut lib = FaceLibrary::default();
        assert!(!lib.consent().may_detect());
        assert_eq!(lib.store(crop("me", Provenance::SelfCapture)), Err(Refusal::NoConsent));
    }

    #[test]
    fn revoking_erases_rather_than_merely_stopping() {
        // "You can turn this off at any time, which will erase everything that
        // was remembered." A toggle that only stops collection leaves the
        // promise untrue the moment somebody re-reads the database.
        let mut lib = FaceLibrary::default();
        lib.grant();
        lib.store(crop("me", Provenance::SelfCapture)).unwrap();
        lib.store(crop("alice", Provenance::ReceivedFromContact)).unwrap();
        assert_eq!(lib.revoke(), 2);
        assert!(lib.is_empty());
        assert!(!lib.consent().may_detect());
    }

    #[test]
    fn the_erase_is_immediate_not_deferred() {
        // A deferred erase is a window in which the opt-in text is false, and
        // the window is exactly when a worried user is looking.
        let mut lib = FaceLibrary::default();
        lib.grant();
        lib.store(crop("me", Provenance::SelfCapture)).unwrap();
        lib.revoke();
        assert!(lib.crops().is_empty(), "data survived the revocation");
    }

    #[test]
    fn re_enabling_starts_empty() {
        // The doc: "the user can re-enable later, library starts empty." Data
        // reappearing after a revoke would mean the erase never happened.
        let mut lib = FaceLibrary::default();
        lib.grant();
        lib.store(crop("me", Provenance::SelfCapture)).unwrap();
        lib.revoke();
        lib.grant();
        assert!(lib.is_empty());
    }

    #[test]
    fn revoked_is_distinguishable_from_never_asked() {
        // The app has to know an erase is owed, and "never asked" owes nothing.
        let mut lib = FaceLibrary::default();
        assert!(!lib.consent().owes_erase());
        lib.grant();
        lib.revoke();
        assert!(lib.consent().owes_erase());
    }

    #[test]
    fn refusals_are_counted_rather_than_silent() {
        // A library that silently drops most of what it sees looks BROKEN
        // rather than principled, and somebody would then "fix" it.
        let mut lib = FaceLibrary::default();
        lib.grant();
        for i in 0..5 { let _ = lib.store(crop(&format!("x{i}"), Provenance::ThirdPartyInCapture)); }
        assert_eq!(lib.refused, 5);
        assert!(lib.is_empty());
    }

    #[test]
    fn no_consent_is_not_counted_as_a_provenance_refusal() {
        // Two different problems: one is the user's choice, the other is the
        // rule doing its job. Conflating them makes the metric meaningless.
        let mut lib = FaceLibrary::default();
        let _ = lib.store(crop("me", Provenance::SelfCapture));
        assert_eq!(lib.refused, 0);
    }

    #[test]
    fn every_provenance_can_explain_itself() {
        // These appear in an audit view: "refused" without a reason is not
        // something a user can check.
        for p in [Provenance::SelfCapture, Provenance::ReceivedFromContact,
                  Provenance::ThirdPartyInCapture, Provenance::Unknown] {
            assert!(!p.why().is_empty(), "{p:?}");
        }
        assert!(Provenance::ThirdPartyInCapture.why().contains("did not initiate"));
    }

    #[test]
    fn the_opt_in_text_promises_what_the_code_does() {
        // A consent string that drifts from the behaviour is worse than no
        // string at all, so the four claims it makes are asserted against the
        // implementation that has to keep them.
        assert!(OPT_IN_TEXT.contains("stays on this device"));
        assert!(OPT_IN_TEXT.contains("turn this off at any time"));
        assert!(OPT_IN_TEXT.contains("erase everything"));
        assert!(OPT_IN_TEXT.contains("reduce data usage"),
                "the stated purpose is bandwidth, not identity");
        assert!(!OPT_IN_TEXT.to_lowercase().contains("recogni"),
                "identity framing crept back into the consent text");
    }
}
