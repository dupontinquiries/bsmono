//! Pair Verify, controller side (HAP spec §5.7). Every connection, using
//! the long-term keys pair-setup stored.
//!
//! ```text
//!   M1 ->  state=1, our ephemeral Curve25519 public key
//!   <- M2  state=2, their ephemeral key, encrypted{ id, sig }   (sig by their LTSK over their eph | id | our eph)
//!   M3 ->  state=3, encrypted{ id, sig }                        (sig by our LTSK over our eph | id | their eph)
//!   <- M4  state=4                                              (empty = accepted)
//! ```
//!
//! Both signatures are checked against the pairing record, so a device that
//! merely knows the protocol but not the keys is refused at M2, before we
//! sign anything.

use crate::keys::{hkdf_32, label_nonce, open, seal, Pairing};
use crate::session::SecureSession;
use crate::tlv8::{self, Tlv};
use ed25519_dalek::{Signer, Verifier};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairVerifyError {
    Accessory { state: u8, code: u8 },
    Protocol(String),
    Tlv(tlv8::TlvError),
    /// M2 did not decrypt, named a different accessory than we paired, or
    /// its signature failed against the stored LTPK. The pairing record
    /// and this device disagree; refuse rather than sign.
    AccessoryUnverified(String),
}

impl std::fmt::Display for PairVerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PairVerifyError::Accessory { state, code } => write!(f, "accessory refused at M{state}: {}", tlv8::error_code::describe(*code)),
            PairVerifyError::Protocol(s) => write!(f, "pair-verify protocol: {s}"),
            PairVerifyError::Tlv(e) => write!(f, "pair-verify: {e}"),
            PairVerifyError::AccessoryUnverified(s) => write!(f, "pair-verify: {s}"),
        }
    }
}

impl From<tlv8::TlvError> for PairVerifyError {
    fn from(e: tlv8::TlvError) -> Self { PairVerifyError::Tlv(e) }
}

#[derive(Debug)]
pub enum Outcome {
    Send(Vec<u8>),
    /// Verified. The session's counters start at zero here.
    Done(SecureSession),
}

enum State {
    Fresh,
    SentM1,
    SentM3 { shared: [u8; 32] },
    Finished,
}

pub struct PairVerify {
    pairing: Pairing,
    secret: x25519_dalek::StaticSecret,
    public: [u8; 32],
    state: State,
}

impl PairVerify {
    pub fn new(pairing: Pairing) -> Self {
        let secret = x25519_dalek::StaticSecret::random_from_rng(rand_core::OsRng);
        let public = x25519_dalek::PublicKey::from(&secret).to_bytes();
        PairVerify { pairing, secret, public, state: State::Fresh }
    }

    /// Deterministic ephemeral key, for tests that want to replay.
    pub fn with_ephemeral(pairing: Pairing, secret: [u8; 32]) -> Self {
        let secret = x25519_dalek::StaticSecret::from(secret);
        let public = x25519_dalek::PublicKey::from(&secret).to_bytes();
        PairVerify { pairing, secret, public, state: State::Fresh }
    }

    pub fn start(&mut self) -> Vec<u8> {
        self.state = State::SentM1;
        Tlv::new().with(tlv8::TYPE_STATE, [1u8]).with(tlv8::TYPE_PUBLIC_KEY, self.public).encode()
    }

    pub fn handle(&mut self, reply: &[u8]) -> Result<Outcome, PairVerifyError> {
        let tlv = Tlv::decode(reply)?;
        let state = tlv.get_u8(tlv8::TYPE_STATE).ok_or_else(|| PairVerifyError::Protocol("reply has no state".into()))?;
        if let Some(code) = tlv.get_u8(tlv8::TYPE_ERROR) {
            self.state = State::Finished;
            return Err(PairVerifyError::Accessory { state, code });
        }
        match (std::mem::replace(&mut self.state, State::Finished), state) {
            (State::SentM1, 2) => self.on_m2(&tlv),
            (State::SentM3 { shared }, 4) => Ok(Outcome::Done(SecureSession::from_shared_secret(&shared))),
            (_, got) => Err(PairVerifyError::Protocol(format!("unexpected state {got} in reply"))),
        }
    }

    fn on_m2(&mut self, tlv: &Tlv) -> Result<Outcome, PairVerifyError> {
        let their_pub: [u8; 32] = tlv.get(tlv8::TYPE_PUBLIC_KEY)
            .and_then(|k| k.try_into().ok())
            .ok_or_else(|| PairVerifyError::Protocol("M2 public key is not 32 bytes".into()))?;
        let enc = tlv.get(tlv8::TYPE_ENCRYPTED_DATA).ok_or_else(|| PairVerifyError::Protocol("M2 has no encrypted data".into()))?;
        let shared = self.secret.diffie_hellman(&x25519_dalek::PublicKey::from(their_pub)).to_bytes();
        // Spec §5.7.2: the M2/M3 message key is derived from the ECDH secret.
        let session_key = hkdf_32(b"Pair-Verify-Encrypt-Salt", b"Pair-Verify-Encrypt-Info", &shared);
        let plain = open(&session_key, &label_nonce(b"PV-Msg02"), b"", enc)
            .map_err(|_| PairVerifyError::AccessoryUnverified("M2 did not decrypt (different ephemeral secret than ours?)".into()))?;
        let sub = Tlv::decode(&plain)?;
        let id = sub.get(tlv8::TYPE_IDENTIFIER).ok_or_else(|| PairVerifyError::Protocol("M2 has no identifier".into()))?;
        if id != self.pairing.accessory_id.as_bytes() {
            return Err(PairVerifyError::AccessoryUnverified(format!(
                "M2 identifies as {:?}, the pairing is with {:?}", String::from_utf8_lossy(id), self.pairing.accessory_id)));
        }
        let sig: [u8; 64] = sub.get(tlv8::TYPE_SIGNATURE)
            .and_then(|s| s.try_into().ok())
            .ok_or_else(|| PairVerifyError::Protocol("M2 signature is not 64 bytes".into()))?;
        // AccessoryInfo = their eph | their id | our eph, signed by AccessoryLTSK.
        let mut info = Vec::with_capacity(32 + id.len() + 32);
        info.extend_from_slice(&their_pub);
        info.extend_from_slice(id);
        info.extend_from_slice(&self.public);
        let vk = ed25519_dalek::VerifyingKey::from_bytes(&self.pairing.accessory_ltpk)
            .map_err(|_| PairVerifyError::AccessoryUnverified("stored accessory LTPK is not a valid key".into()))?;
        vk.verify(&info, &ed25519_dalek::Signature::from_bytes(&sig))
            .map_err(|_| PairVerifyError::AccessoryUnverified("M2 signature does not verify against the stored LTPK".into()))?;

        // M3: ControllerInfo = our eph | our id | their eph, signed by our LTSK.
        let sk = ed25519_dalek::SigningKey::from_bytes(&self.pairing.controller_ltsk);
        let mut our_info = Vec::with_capacity(32 + 36 + 32);
        our_info.extend_from_slice(&self.public);
        our_info.extend_from_slice(self.pairing.controller_id.as_bytes());
        our_info.extend_from_slice(&their_pub);
        let our_sig = sk.sign(&our_info).to_bytes();
        let sub = Tlv::new()
            .with(tlv8::TYPE_IDENTIFIER, self.pairing.controller_id.as_bytes())
            .with(tlv8::TYPE_SIGNATURE, our_sig)
            .encode();
        let encrypted = seal(&session_key, &label_nonce(b"PV-Msg03"), b"", &sub);
        let m3 = Tlv::new().with(tlv8::TYPE_STATE, [3u8]).with(tlv8::TYPE_ENCRYPTED_DATA, encrypted).encode();
        self.state = State::SentM3 { shared };
        Ok(Outcome::Send(m3))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m1_carries_a_32_byte_ephemeral_key() {
        let p = Pairing { controller_id: "C".into(), controller_ltsk: [3; 32], accessory_id: "A".into(), accessory_ltpk: [0; 32] };
        let mut pv = PairVerify::new(p);
        let m1 = Tlv::decode(&pv.start()).unwrap();
        assert_eq!(m1.get_u8(tlv8::TYPE_STATE), Some(1));
        assert_eq!(m1.get(tlv8::TYPE_PUBLIC_KEY).unwrap().len(), 32);
    }

    #[test]
    fn an_m2_that_does_not_decrypt_is_refused_before_we_sign() {
        let p = Pairing { controller_id: "C".into(), controller_ltsk: [3; 32], accessory_id: "A".into(), accessory_ltpk: [0; 32] };
        let mut pv = PairVerify::new(p);
        pv.start();
        let junk = Tlv::new().with(tlv8::TYPE_STATE, [2u8]).with(tlv8::TYPE_PUBLIC_KEY, [9u8; 32]).with(tlv8::TYPE_ENCRYPTED_DATA, vec![0u8; 40]).encode();
        assert!(matches!(pv.handle(&junk).unwrap_err(), PairVerifyError::AccessoryUnverified(_)));
    }

    // The full flow, the wrong-accessory and wrong-key branches, are in
    // `sim::tests`.
}
