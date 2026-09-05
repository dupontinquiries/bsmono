//! Pair Setup, controller side (HAP spec §5.6). Six messages, three of
//! them ours. Once per accessory, with the setup code off the sticker.
//!
//! ```text
//!   M1 ->  state=1, method=PairSetup
//!   <- M2  state=2, salt, B                       (SRP: accessory's public value)
//!   M3 ->  state=3, A, M1proof                    (SRP: our public value + proof)
//!   <- M4  state=4, M2proof                       (SRP: accessory proved it knows the code too)
//!   M5 ->  state=5, encrypted{ id, LTPK, sig }    (our long-term identity, under the SRP key)
//!   <- M6  state=6, encrypted{ id, LTPK, sig }    (theirs)
//! ```
//!
//! The machine is fed bytes and hands back bytes; it never touches a
//! radio. [`crate::ble::Controller`] drives it over a transport and
//! [`crate::sim::Accessory`] drives the other side in tests, so every
//! branch below — wrong code, tampered M6, accessory error codes — is a
//! unit test rather than a plug that has to be factory-reset.

use crate::keys::{self, hkdf_32, label_nonce, open, seal, Pairing};
use crate::srp;
use crate::tlv8::{self, Tlv};
use ed25519_dalek::{Signer, Verifier};

const IDENTITY: &[u8] = b"Pair-Setup";

/// Method values for M1 (spec table 5-3). `PAIR_SETUP` is the software
/// path every accessory since iOS 11.3 accepts; `PAIR_SETUP_WITH_AUTH`
/// asks for the MFi coprocessor exchange, which this crate does not do.
/// If an accessory answers M1 with `TYPE_ERROR`, the method is the first
/// thing to try changing.
pub const METHOD_PAIR_SETUP: u8 = 0x00;
pub const METHOD_PAIR_SETUP_WITH_AUTH: u8 = 0x01;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairSetupError {
    /// The accessory sent a `TYPE_ERROR` item. The code says why.
    Accessory { state: u8, code: u8 },
    /// A reply for a state we were not in, or missing a required item.
    Protocol(String),
    Tlv(tlv8::TlvError),
    Srp(srp::SrpError),
    /// The accessory's SRP proof did not match: it does not know the setup
    /// code we used, or we mis-computed. Either way, do not send M5 — that
    /// would hand our identity to an unverified party.
    ServerProofMismatch,
    /// M6 did not decrypt, or its signature failed. The accessory's
    /// identity claim is unverified; nothing is stored.
    IdentityUnverified(String),
    /// Setup code is not 8 digits.
    BadSetupCode,
}

impl std::fmt::Display for PairSetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PairSetupError::Accessory { state, code } => write!(f, "accessory refused at M{state}: {}", tlv8::error_code::describe(*code)),
            PairSetupError::Protocol(s) => write!(f, "pair-setup protocol: {s}"),
            PairSetupError::Tlv(e) => write!(f, "pair-setup: {e}"),
            PairSetupError::Srp(e) => write!(f, "pair-setup: {e}"),
            PairSetupError::ServerProofMismatch => write!(f, "pair-setup: accessory failed to prove it knows the setup code"),
            PairSetupError::IdentityUnverified(s) => write!(f, "pair-setup: accessory identity unverified ({s})"),
            PairSetupError::BadSetupCode => write!(f, "setup code must be 8 digits (XXX-XX-XXX)"),
        }
    }
}

impl From<tlv8::TlvError> for PairSetupError {
    fn from(e: tlv8::TlvError) -> Self { PairSetupError::Tlv(e) }
}
impl From<srp::SrpError> for PairSetupError {
    fn from(e: srp::SrpError) -> Self { PairSetupError::Srp(e) }
}

/// Normalise a typed setup code to the `XXX-XX-XXX` form SRP hashes. The
/// dashes are part of the password: "03145154" and "031-45-154" must
/// produce the same verifier, and only the dashed form does.
pub fn normalize_setup_code(input: &str) -> Result<String, PairSetupError> {
    let digits: String = input.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 8 || input.chars().any(|c| !(c.is_ascii_digit() || c == '-' || c == ' ')) {
        return Err(PairSetupError::BadSetupCode);
    }
    Ok(format!("{}-{}-{}", &digits[0..3], &digits[3..5], &digits[5..8]))
}

/// What to do after feeding a reply in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Send these TLV bytes to the accessory and feed its reply back.
    Send(Vec<u8>),
    /// Paired. Persist this; it is the only copy.
    Done(Pairing),
}

enum State {
    Fresh,
    SentM1,
    SentM3 { session: srp::Session },
    SentM5 { session_key: [u8; 32], srp_key: Vec<u8> },
    Finished,
}

pub struct PairSetup {
    controller_id: String,
    controller_ltsk: [u8; 32],
    setup_code: String,
    method: u8,
    client: srp::Client<sha2::Sha512>,
    salt: Vec<u8>,
    state: State,
}

impl PairSetup {
    pub fn new(controller_id: impl Into<String>, controller_ltsk: [u8; 32], setup_code: &str) -> Result<Self, PairSetupError> {
        Ok(PairSetup {
            controller_id: controller_id.into(),
            controller_ltsk,
            setup_code: normalize_setup_code(setup_code)?,
            method: METHOD_PAIR_SETUP,
            client: srp::Client::new_random(srp::Group::rfc5054_3072()),
            salt: Vec::new(),
            state: State::Fresh,
        })
    }

    /// Ask for the MFi-auth method instead. Not implemented past M1; only
    /// useful for observing what an accessory answers.
    pub fn with_method(mut self, method: u8) -> Self {
        self.method = method;
        self
    }

    /// M1 bytes. Call once.
    pub fn start(&mut self) -> Vec<u8> {
        self.state = State::SentM1;
        Tlv::new().with(tlv8::TYPE_STATE, [1u8]).with(tlv8::TYPE_METHOD, [self.method]).encode()
    }

    /// Feed the accessory's reply. Each call advances one step.
    pub fn handle(&mut self, reply: &[u8]) -> Result<Outcome, PairSetupError> {
        let tlv = Tlv::decode(reply)?;
        let state = tlv.get_u8(tlv8::TYPE_STATE).ok_or_else(|| PairSetupError::Protocol("reply has no state".into()))?;
        if let Some(code) = tlv.get_u8(tlv8::TYPE_ERROR) {
            self.state = State::Finished;
            return Err(PairSetupError::Accessory { state, code });
        }
        match (std::mem::replace(&mut self.state, State::Finished), state) {
            (State::SentM1, 2) => self.on_m2(&tlv),
            (State::SentM3 { session }, 4) => self.on_m4(&tlv, session),
            (State::SentM5 { session_key, srp_key }, 6) => self.on_m6(&tlv, session_key, &srp_key),
            (_, got) => Err(PairSetupError::Protocol(format!("unexpected state {got} in reply"))),
        }
    }

    fn on_m2(&mut self, tlv: &Tlv) -> Result<Outcome, PairSetupError> {
        let salt = tlv.get(tlv8::TYPE_SALT).ok_or_else(|| PairSetupError::Protocol("M2 has no salt".into()))?;
        let b_pub = tlv.get(tlv8::TYPE_PUBLIC_KEY).ok_or_else(|| PairSetupError::Protocol("M2 has no public key".into()))?;
        if salt.len() != 16 {
            return Err(PairSetupError::Protocol(format!("M2 salt is {} bytes, expected 16", salt.len())));
        }
        self.salt = salt.to_vec();
        let session = self.client.process(IDENTITY, self.setup_code.as_bytes(), salt, b_pub)?;
        let m3 = Tlv::new()
            .with(tlv8::TYPE_STATE, [3u8])
            .with(tlv8::TYPE_PUBLIC_KEY, self.client.public_key())
            .with(tlv8::TYPE_PROOF, session.client_proof.clone())
            .encode();
        self.state = State::SentM3 { session };
        Ok(Outcome::Send(m3))
    }

    fn on_m4(&mut self, tlv: &Tlv, session: srp::Session) -> Result<Outcome, PairSetupError> {
        let proof = tlv.get(tlv8::TYPE_PROOF).ok_or_else(|| PairSetupError::Protocol("M4 has no proof".into()))?;
        if proof != session.expected_server_proof.as_slice() {
            return Err(PairSetupError::ServerProofMismatch);
        }
        // Spec §5.6.6.1: derive our signing input and the message key from
        // the SRP session key, sign, encrypt.
        let device_x = hkdf_32(b"Pair-Setup-Controller-Sign-Salt", b"Pair-Setup-Controller-Sign-Info", &session.key);
        let session_key = hkdf_32(b"Pair-Setup-Encrypt-Salt", b"Pair-Setup-Encrypt-Info", &session.key);
        let sk = ed25519_dalek::SigningKey::from_bytes(&self.controller_ltsk);
        let ltpk = sk.verifying_key().to_bytes();
        let mut info = Vec::with_capacity(32 + 36 + 32);
        info.extend_from_slice(&device_x);
        info.extend_from_slice(self.controller_id.as_bytes());
        info.extend_from_slice(&ltpk);
        let sig = sk.sign(&info).to_bytes();
        let sub = Tlv::new()
            .with(tlv8::TYPE_IDENTIFIER, self.controller_id.as_bytes())
            .with(tlv8::TYPE_PUBLIC_KEY, ltpk)
            .with(tlv8::TYPE_SIGNATURE, sig)
            .encode();
        let encrypted = seal(&session_key, &label_nonce(b"PS-Msg05"), b"", &sub);
        let m5 = Tlv::new().with(tlv8::TYPE_STATE, [5u8]).with(tlv8::TYPE_ENCRYPTED_DATA, encrypted).encode();
        self.state = State::SentM5 { session_key, srp_key: session.key };
        Ok(Outcome::Send(m5))
    }

    fn on_m6(&mut self, tlv: &Tlv, session_key: [u8; 32], srp_key: &[u8]) -> Result<Outcome, PairSetupError> {
        let enc = tlv.get(tlv8::TYPE_ENCRYPTED_DATA).ok_or_else(|| PairSetupError::Protocol("M6 has no encrypted data".into()))?;
        let plain = open(&session_key, &label_nonce(b"PS-Msg06"), b"", enc)
            .map_err(|_| PairSetupError::IdentityUnverified("M6 did not decrypt under the session key".into()))?;
        let sub = Tlv::decode(&plain)?;
        let acc_id = sub.get(tlv8::TYPE_IDENTIFIER).ok_or_else(|| PairSetupError::Protocol("M6 has no identifier".into()))?;
        let acc_ltpk: [u8; 32] = sub.get(tlv8::TYPE_PUBLIC_KEY)
            .and_then(|k| k.try_into().ok())
            .ok_or_else(|| PairSetupError::Protocol("M6 public key is not 32 bytes".into()))?;
        let sig: [u8; 64] = sub.get(tlv8::TYPE_SIGNATURE)
            .and_then(|s| s.try_into().ok())
            .ok_or_else(|| PairSetupError::Protocol("M6 signature is not 64 bytes".into()))?;
        // Spec §5.6.6.2: AccessoryX || AccessoryPairingID || AccessoryLTPK, signed by AccessoryLTSK.
        let accessory_x = hkdf_32(b"Pair-Setup-Accessory-Sign-Salt", b"Pair-Setup-Accessory-Sign-Info", srp_key);
        let mut info = Vec::with_capacity(32 + acc_id.len() + 32);
        info.extend_from_slice(&accessory_x);
        info.extend_from_slice(acc_id);
        info.extend_from_slice(&acc_ltpk);
        let vk = ed25519_dalek::VerifyingKey::from_bytes(&acc_ltpk)
            .map_err(|_| PairSetupError::IdentityUnverified("M6 public key is not a valid Ed25519 point".into()))?;
        vk.verify(&info, &ed25519_dalek::Signature::from_bytes(&sig))
            .map_err(|_| PairSetupError::IdentityUnverified("M6 signature does not verify".into()))?;
        let accessory_id = String::from_utf8(acc_id.to_vec())
            .map_err(|_| PairSetupError::Protocol("M6 identifier is not UTF-8".into()))?;
        self.state = State::Finished;
        Ok(Outcome::Done(Pairing {
            controller_id: self.controller_id.clone(),
            controller_ltsk: self.controller_ltsk,
            accessory_id,
            accessory_ltpk: acc_ltpk,
        }))
    }
}

// Silence the unused-import lint for `keys` when only its items are used.
#[allow(unused_imports)]
use keys::hex as _hex;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_codes_normalise_to_the_dashed_form_srp_hashes() {
        assert_eq!(normalize_setup_code("031-45-154").unwrap(), "031-45-154");
        assert_eq!(normalize_setup_code("03145154").unwrap(), "031-45-154");
        assert_eq!(normalize_setup_code("031 45 154").unwrap(), "031-45-154");
        assert_eq!(normalize_setup_code("0314515").unwrap_err(), PairSetupError::BadSetupCode);
        assert_eq!(normalize_setup_code("03145154x").unwrap_err(), PairSetupError::BadSetupCode);
    }

    #[test]
    fn m1_is_state_one_and_the_method() {
        let mut ps = PairSetup::new("C", [1; 32], "03145154").unwrap();
        let m1 = Tlv::decode(&ps.start()).unwrap();
        assert_eq!(m1.get_u8(tlv8::TYPE_STATE), Some(1));
        assert_eq!(m1.get_u8(tlv8::TYPE_METHOD), Some(METHOD_PAIR_SETUP));
    }

    #[test]
    fn an_accessory_error_surfaces_with_its_code_and_ends_the_machine() {
        let mut ps = PairSetup::new("C", [1; 32], "03145154").unwrap();
        ps.start();
        let reply = Tlv::new().with(tlv8::TYPE_STATE, [2u8]).with(tlv8::TYPE_ERROR, [tlv8::error_code::MAX_TRIES]).encode();
        assert_eq!(ps.handle(&reply).unwrap_err(), PairSetupError::Accessory { state: 2, code: tlv8::error_code::MAX_TRIES });
        // The machine does not resurrect: a well-formed M2 after the error is a protocol error.
        let m2 = Tlv::new().with(tlv8::TYPE_STATE, [2u8]).with(tlv8::TYPE_SALT, [0u8; 16]).with(tlv8::TYPE_PUBLIC_KEY, [1u8; 384]).encode();
        assert!(matches!(ps.handle(&m2).unwrap_err(), PairSetupError::Protocol(_)));
    }

    #[test]
    fn a_reply_for_the_wrong_state_is_refused() {
        let mut ps = PairSetup::new("C", [1; 32], "03145154").unwrap();
        ps.start();
        let reply = Tlv::new().with(tlv8::TYPE_STATE, [4u8]).with(tlv8::TYPE_PROOF, [0u8; 64]).encode();
        assert!(matches!(ps.handle(&reply).unwrap_err(), PairSetupError::Protocol(_)));
    }

    // The full six-message flow, including the wrong-code and tampered-M6
    // branches, lives in `sim::tests`, where the accessory half exists.
}
