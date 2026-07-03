//! Signature verification for landmarks.
//!
//! V32.32: scaffolding only. The verification function takes the wire
//! bytes (with signature embedded) and a 32-byte ed25519 public key,
//! returns Ok if the signature is valid for the wire payload's
//! signed-portion (everything except the signature itself), Err
//! otherwise.
//!
//! The signed portion is bytes 0..80 (version through photo_fp) +
//! bytes 144..N (length-prefixed title and description). The 64-byte
//! signature at bytes 80..144 is what we verify against this.
//!
//! Real implementation pending: needs to plug into the existing
//! snapclone-identity / snapclone-crypto signing infrastructure so
//! the public key handling is consistent across the codebase.
//! For now the stub returns Err(NotImplemented) so callers can
//! wire up the call site and we fill in the algorithm later.

use crate::error::LandmarkError;

/// Errors that signal verification failure specifically (as opposed to
/// general landmark issues). Re-exported via LandmarkError::BadSignature.
#[derive(Debug, Clone, Copy)]
pub enum VerifyError {
    /// Verification logic isn't wired up yet. Callers can match on this
    /// during the transition period and treat it as "skip verification"
    /// in dev builds.
    NotImplemented,
    /// Signature bytes failed cryptographic verification.
    Invalid,
}

/// Verify the signature inside a wire-encoded Landmark payload.
///
/// `wire_bytes` is the full encoded payload (as produced by
/// `wire::encode_wire`). `expected_pubkey` is the 32-byte ed25519
/// public key the signature should validate against — typically pulled
/// from the creator_pubkey field of the same landmark, OR from a
/// trusted contact directory (verifying that the key claiming to be
/// "alice@example" is the one your address book says is hers).
///
/// V32.32: returns `Err(BadSignature)` always until the ed25519 plumbing
/// is wired in. Stub kept so call sites can adopt the API now.
pub fn verify_signature(
    wire_bytes: &[u8],
    expected_pubkey: &[u8; 32],
) -> Result<(), LandmarkError> {
    // Bind unused args so they don't warn — once the real impl lands
    // these will be used.
    let _ = (wire_bytes, expected_pubkey);

    // The intended implementation:
    //
    //   use ed25519_dalek::{Signature, VerifyingKey, Verifier};
    //   // Signature lives at bytes 80..144 of the wire payload.
    //   let sig = Signature::from_bytes(
    //       wire_bytes[80..144].try_into()
    //           .map_err(|_| LandmarkError::BadSignature)?);
    //   let pk = VerifyingKey::from_bytes(expected_pubkey)
    //       .map_err(|_| LandmarkError::BadSignature)?;
    //   // Signed portion: header (0..80) + length-prefixed text tail
    //   // (144..end). Signature bytes themselves are excluded.
    //   let mut signed = Vec::with_capacity(wire_bytes.len() - 64);
    //   signed.extend_from_slice(&wire_bytes[..80]);
    //   signed.extend_from_slice(&wire_bytes[144..]);
    //   pk.verify(&signed, &sig)
    //       .map_err(|_| LandmarkError::BadSignature)
    //
    // Held until snapclone-identity exposes the signing helpers we
    // need; doing ad-hoc ed25519 here would duplicate plumbing and
    // risk key-handling drift between crates.
    Err(LandmarkError::BadSignature)
}
