//! peerauth.rs — proving a peer is the machine you registered.
//!
//! ## Why this is not a feature but a precondition
//!
//! `trustkit` filters by peer ID. If an ID is just a string a peer claims, the filter is **decorative**:
//! anyone says "I am m2" and receives whatever m2 was granted. Registration only means something if the
//! ID is bound to a key nobody else holds.
//!
//! That is a gap in `trustkit` as written — `register(id, level)` records a level against a *name*.
//! `Registry` needs to bind a public key at registration, and this module is what consumes it.
//!
//! ## The crypto is a library call; the protocol is where the mistakes are
//!
//! `shared/crypto-kit` already provides Ed25519. What it cannot provide is the shape of what gets
//! signed, and three failures live there:
//!
//! * **Replay** — a response captured once is presented again later. Fixed by a nonce the *verifier*
//!   chose, never the prover.
//! * **Reflection** — the challenge is bounced back at its sender, who signs it and proves themselves
//!   to an attacker. Fixed by binding the signer's identity into the payload.
//! * **Cross-context reuse** — a signature obtained for one purpose is presented for another. Fixed by
//!   binding a purpose string, so a signature over a build handshake is not valid for a handoff.
//!
//! Each of these is invisible if you only test the happy path, which is why the tests below are
//! attacks rather than round trips.

/// What a challenge is being issued for. Bound into the signed payload so a signature obtained in one
/// context cannot be presented in another.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Purpose { Enlist, Compute, Handoff, Build }

impl Purpose {
    fn tag(self) -> &'static str {
        match self { Purpose::Enlist => "enlist", Purpose::Compute => "compute",
                     Purpose::Handoff => "handoff", Purpose::Build => "build" }
    }
}

/// A challenge issued by the VERIFIER.
///
/// The nonce is chosen by the side doing the checking. A prover-chosen nonce proves nothing: an
/// attacker replays a captured pair and the nonce inside it matches itself.
#[derive(Clone, PartialEq, Debug)]
pub struct Challenge {
    pub nonce: [u8; 32],
    /// Who is being asked to prove themselves.
    pub prover_id: String,
    /// Who is asking. Bound in so the response cannot be reflected back at us.
    pub verifier_id: String,
    pub purpose: Purpose,
    pub issued_at_s: u64,
}

/// How long a challenge stays valid.
///
/// Short: the only reason to allow any window is clock skew and network delay. A long window is a
/// replay window.
pub const CHALLENGE_TTL_S: u64 = 30;

impl Challenge {
    /// The exact bytes to sign.
    ///
    /// Domain-separated with a fixed prefix so a signature over this can never be a valid signature
    /// over some other message this system signs. Fields are length-prefixed rather than concatenated:
    /// without that, `("ab", "c")` and `("a", "bc")` produce identical bytes, and an attacker who can
    /// influence one field can forge the other.
    pub fn payload(&self) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"logscope-peerauth-v1\0");
        for part in [self.prover_id.as_bytes(), self.verifier_id.as_bytes(),
                     self.purpose.tag().as_bytes()] {
            v.extend_from_slice(&(part.len() as u32).to_le_bytes());
            v.extend_from_slice(part);
        }
        v.extend_from_slice(&self.nonce);
        v.extend_from_slice(&self.issued_at_s.to_le_bytes());
        v
    }

    pub fn expired(&self, now_s: u64) -> bool {
        now_s.saturating_sub(self.issued_at_s) > CHALLENGE_TTL_S
    }
}

/// Why a proof was rejected.
#[derive(Clone, PartialEq, Debug)]
pub enum AuthError {
    /// No public key is registered for this id, so nothing can be checked.
    NoKeyRegistered(String),
    /// The signature does not verify under the registered key.
    BadSignature,
    /// The challenge is too old.
    Expired { age_s: u64 },
    /// The response answers a different challenge than the one issued.
    WrongChallenge,
}

impl AuthError {
    pub fn explain(&self) -> String {
        match self {
            AuthError::NoKeyRegistered(id) => format!(
                "{id} is registered by NAME with no public key, so its identity cannot be checked — \
anyone claiming that name would pass. Re-register it with its key"),
            AuthError::BadSignature => "the signature does not verify under the key registered for this \
peer: either it is not that machine, or the key changed and registration is stale".into(),
            AuthError::Expired { age_s } => format!(
                "the challenge is {age_s}s old, past the {CHALLENGE_TTL_S}s window. A longer window is a \
replay window, so this is refused rather than widened"),
            AuthError::WrongChallenge => "the response signs a different challenge than the one issued \
— a replayed or reflected proof".into(),
        }
    }
}

/// Verify a proof. `verify_sig` is the Ed25519 check from `crypto-kit`, injected so this module holds
/// the protocol and not the primitive.
pub fn verify(issued: &Challenge, answered: &Challenge, sig: &[u8],
              registered_key: Option<&[u8]>, now_s: u64,
              verify_sig: impl Fn(&[u8], &[u8], &[u8]) -> bool) -> Result<(), AuthError> {
    let Some(key) = registered_key else {
        return Err(AuthError::NoKeyRegistered(issued.prover_id.clone()));
    };
    if issued != answered { return Err(AuthError::WrongChallenge); }
    if issued.expired(now_s) {
        return Err(AuthError::Expired { age_s: now_s.saturating_sub(issued.issued_at_s) });
    }
    if !verify_sig(key, &issued.payload(), sig) { return Err(AuthError::BadSignature); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(prover: &str, verifier: &str, p: Purpose, at: u64) -> Challenge {
        Challenge { nonce: [7u8; 32], prover_id: prover.into(), verifier_id: verifier.into(),
                    purpose: p, issued_at_s: at }
    }
    /// A stand-in that accepts any signature, so the test isolates the PROTOCOL from the primitive.
    fn always(_k: &[u8], _m: &[u8], _s: &[u8]) -> bool { true }
    fn never(_k: &[u8], _m: &[u8], _s: &[u8]) -> bool { false }

    #[test]
    fn a_peer_registered_by_NAME_ONLY_cannot_be_authenticated() {
        // The gap this surfaced in trustkit: register(id, level) records a level against a string. If
        // no key is bound, anyone claiming the name passes and the whole filter is decorative.
        let c = ch("m2", "phone", Purpose::Compute, 100);
        let e = verify(&c, &c, b"sig", None, 100, always).unwrap_err();
        assert!(matches!(e, AuthError::NoKeyRegistered(_)));
        assert!(e.explain().contains("anyone claiming that name would pass"));
    }

    #[test]
    fn a_response_to_a_DIFFERENT_challenge_is_rejected() {
        // Replay: a captured pair presented later. The nonce is chosen by the VERIFIER, so a replayed
        // response answers a challenge nobody issued this time.
        let issued = ch("m2", "phone", Purpose::Compute, 100);
        let mut old = issued.clone();
        old.nonce = [9u8; 32];
        assert_eq!(verify(&issued, &old, b"sig", Some(b"k"), 100, always),
                   Err(AuthError::WrongChallenge));
    }

    #[test]
    fn a_reflected_challenge_does_not_verify_because_identities_are_BOUND() {
        // Reflection: bounce our challenge back at us so we sign it and prove ourselves to an attacker.
        // The payload binds prover and verifier, so a swapped pair is different bytes.
        let ours = ch("m2", "phone", Purpose::Compute, 100);
        let reflected = ch("phone", "m2", Purpose::Compute, 100);
        assert_ne!(ours.payload(), reflected.payload(), "swapping the roles must change the payload");
    }

    #[test]
    fn a_signature_for_one_PURPOSE_is_not_valid_for_another() {
        // Cross-context reuse: a proof gathered for a build handshake must not authorise a handoff,
        // which is the level that lets a machine keep your conversation.
        let build = ch("m2", "phone", Purpose::Build, 100);
        let handoff = ch("m2", "phone", Purpose::Handoff, 100);
        assert_ne!(build.payload(), handoff.payload());
    }

    #[test]
    fn the_payload_is_length_prefixed_so_fields_cannot_be_slid() {
        // Without length prefixes, ("ab","c") and ("a","bc") produce identical bytes — an attacker who
        // can influence one field forges the other. This is the classic concatenation bug and it is
        // invisible in a round-trip test.
        let a = ch("ab", "c", Purpose::Compute, 1);
        let b = ch("a", "bc", Purpose::Compute, 1);
        assert_ne!(a.payload(), b.payload());
    }

    #[test]
    fn the_payload_is_domain_separated() {
        // So a signature over this can never be a valid signature over some other message this system
        // signs — mailbox postings, for instance, which share-kit also signs.
        assert!(ch("a", "b", Purpose::Enlist, 0).payload().starts_with(b"logscope-peerauth-v1\0"));
    }

    #[test]
    fn an_expired_challenge_is_refused_rather_than_the_window_widened() {
        // The only reason to allow any window is clock skew and network delay. A long window IS a
        // replay window, so the error says that rather than suggesting a larger TTL.
        let c = ch("m2", "phone", Purpose::Compute, 100);
        let e = verify(&c, &c, b"sig", Some(b"k"), 100 + CHALLENGE_TTL_S + 1, always).unwrap_err();
        assert!(matches!(e, AuthError::Expired { .. }));
        assert!(e.explain().contains("A longer window is a replay window"));
    }

    #[test]
    fn a_bad_signature_names_BOTH_causes_because_the_fixes_differ() {
        // Not that machine, or a stale registration after a key rotation. Telling someone only the
        // first sends them looking for an attacker when they rotated a key.
        let c = ch("m2", "phone", Purpose::Compute, 100);
        let e = verify(&c, &c, b"sig", Some(b"k"), 100, never).unwrap_err();
        assert_eq!(e, AuthError::BadSignature);
        assert!(e.explain().contains("or the key changed and registration is stale"));
    }

    #[test]
    fn a_valid_proof_passes() {
        let c = ch("m2", "phone", Purpose::Compute, 100);
        assert!(verify(&c, &c, b"sig", Some(b"k"), 105, always).is_ok());
    }
}
