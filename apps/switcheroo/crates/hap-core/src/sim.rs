//! A simulated HAP accessory behind [`Transport`]: one Outlet with an
//! `On` characteristic, a setup code, and the accessory halves of
//! pair-setup, pair-verify and the secure session.
//!
//! It exists so the controller's whole flow — fragmentation at a small
//! MTU, the six-message setup, the four-message verify, encrypted
//! reads/writes, and every refusal branch — runs as unit tests. It is also
//! the seed of the fabric-oracle design HOMEKIT.md settles on: a node that
//! holds the pairing and answers for the accessory needs exactly the
//! accessory-side machines this file contains.
//!
//! What it cannot tell you: whether Apple's hardware agrees on the two
//! SRP conventions named in [`crate::srp::Client::process`]. Only a plug
//! can.

use crate::ble::{self, characteristic, opcode, param, status, Request, Response, Transport, TransportError};
use crate::keys::{hkdf_32, label_nonce, open, seal};
use crate::session::SecureSession;
use crate::srp;
use crate::tlv8::{self, Tlv};
use ed25519_dalek::{Signer, Verifier};
use std::collections::VecDeque;

/// Deliberate misbehaviours, to test the controller's refusals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    None,
    /// Corrupt the M6 ciphertext.
    TamperM6,
    /// Sign M6 with a key other than the one whose public key is sent.
    LieAboutIdentity,
    /// In pair-verify M2, sign with a fresh key instead of the paired one.
    VerifyWithWrongKey,
}

const IID_PAIR_SETUP: u16 = 0x0A;
const IID_PAIR_VERIFY: u16 = 0x0B;
const IID_ON: u16 = 0x0C;
/// The rest of an OUTLET, which is what a WSP100 is: the load sensor, the blinker and the
/// owner's name. Simulated so `Controller::probe` is exercised end to end — a probe tested
/// only against absent characteristics proves the negative and nothing else.
const IID_OUTLET_IN_USE: u16 = 0x0D;
const IID_IDENTIFY: u16 = 0x0E;
const IID_NAME: u16 = 0x0F;

pub struct Accessory {
    pub id: String,
    ltsk: ed25519_dalek::SigningKey,
    salt: [u8; 16],
    verifier: num_bigint::BigUint,
    mtu: usize,
    pub fault: Fault,
    /// Set on a successful pair-setup: (controller id, controller LTPK).
    pub paired_controller: Option<(String, [u8; 32])>,
    pub on: bool,
    /// Something is plugged in and drawing. Read-only to a controller, as on real
    /// hardware — the accessory decides, and a UI that offers to set it is lying.
    pub in_use: bool,
    /// Set when a controller writes Identify: the blink a person is meant to see.
    pub identified: u32,
    /// The owner's name for this accessory.
    pub name: String,
    // in-flight
    inbound: ble::Reassembler,
    outbound: VecDeque<Vec<u8>>,
    session: Option<SecureSession>,
    /// Staged by pair-verify M3; installed by `write` after M4 has been queued in the clear.
    pending_session: Option<SecureSession>,
    setup: SetupState,
    verify: VerifyState,
    /// Every request the accessory processed, for assertions.
    pub log: Vec<String>,
}

enum SetupState {
    Idle,
    SentM2 { srp: srp::Server<sha2::Sha512> },
    SentM4 { srp_key: Vec<u8>, session_key: [u8; 32] },
}

enum VerifyState {
    Idle,
    SentM2 { shared: [u8; 32], session_key: [u8; 32], our_pub: [u8; 32], their_pub: [u8; 32] },
}

impl Accessory {
    pub fn new(id: &str, setup_code: &str, mtu: usize) -> Self {
        let code = crate::pair_setup::normalize_setup_code(setup_code).expect("valid setup code");
        let mut salt = [0u8; 16];
        rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut salt);
        let x = srp::compute_x::<sha2::Sha512>(&salt, b"Pair-Setup", code.as_bytes());
        let verifier = srp::verifier(&srp::Group::rfc5054_3072(), &x);
        Accessory {
            id: id.to_string(),
            ltsk: ed25519_dalek::SigningKey::generate(&mut rand_core::OsRng),
            salt,
            verifier,
            mtu,
            fault: Fault::None,
            paired_controller: None,
            on: false,
            in_use: false,
            identified: 0,
            name: "sim outlet".to_string(),
            inbound: ble::Reassembler::default(),
            outbound: VecDeque::new(),
            session: None,
            pending_session: None,
            setup: SetupState::Idle,
            verify: VerifyState::Idle,
            log: Vec::new(),
        }
    }

    pub fn ltpk(&self) -> [u8; 32] { self.ltsk.verifying_key().to_bytes() }

    fn respond(&mut self, tid: u8, st: u8, body: Vec<u8>) {
        let resp = Response { tid, status: st, body };
        let max = if self.session.is_some() { self.mtu - 16 } else { self.mtu };
        for f in resp.fragments(max) {
            let bytes = match &mut self.session { Some(s) => s.seal_ble(&f), None => f };
            self.outbound.push_back(bytes);
        }
    }

    fn handle(&mut self, req: Request) {
        self.log.push(format!("op {:#04x} iid {} body {}B", req.opcode, req.iid, req.body.len()));
        let value = Tlv::decode(&req.body).ok().and_then(|t| t.get(param::VALUE).map(|v| v.to_vec()));
        match (req.iid, req.opcode) {
            (IID_PAIR_SETUP, opcode::CHARACTERISTIC_WRITE) => {
                let reply = self.pair_setup_step(value.unwrap_or_default());
                let body = Tlv::new().with(param::VALUE, reply).encode();
                self.respond(req.tid, status::SUCCESS, body);
            }
            (IID_PAIR_VERIFY, opcode::CHARACTERISTIC_WRITE) => {
                let reply = self.pair_verify_step(value.unwrap_or_default());
                let body = Tlv::new().with(param::VALUE, reply).encode();
                self.respond(req.tid, status::SUCCESS, body);
            }
            (IID_ON, op) if self.session.is_none() => {
                let _ = op;
                self.respond(req.tid, status::INSUFFICIENT_AUTHENTICATION, Vec::new());
            }
            (IID_ON, opcode::CHARACTERISTIC_WRITE) => {
                match value.as_deref() {
                    Some([b]) => { self.on = *b != 0; self.respond(req.tid, status::SUCCESS, Vec::new()); }
                    _ => self.respond(req.tid, status::INVALID_REQUEST, Vec::new()),
                }
            }
            (IID_ON, opcode::CHARACTERISTIC_READ) => {
                let body = Tlv::new().with(param::VALUE, [self.on as u8]).encode();
                self.respond(req.tid, status::SUCCESS, body);
            }
            // ── the rest of the outlet ──
            (iid, op) if matches!(iid, IID_OUTLET_IN_USE | IID_IDENTIFY | IID_NAME) && self.session.is_none() => {
                let _ = op;
                self.respond(req.tid, status::INSUFFICIENT_AUTHENTICATION, Vec::new());
            }
            (IID_OUTLET_IN_USE, opcode::CHARACTERISTIC_READ) => {
                let body = Tlv::new().with(param::VALUE, [self.in_use as u8]).encode();
                self.respond(req.tid, status::SUCCESS, body);
            }
            // READ-ONLY, and the accessory says so rather than accepting the write: a
            // controller that writes here has misread the signature, and silently
            // succeeding would hide that.
            (IID_OUTLET_IN_USE, opcode::CHARACTERISTIC_WRITE) => {
                self.respond(req.tid, status::INVALID_REQUEST, Vec::new());
            }
            (IID_IDENTIFY, opcode::CHARACTERISTIC_WRITE) => {
                self.identified += 1;
                self.respond(req.tid, status::SUCCESS, Vec::new());
            }
            (IID_NAME, opcode::CHARACTERISTIC_READ) => {
                let body = Tlv::new().with(param::VALUE, self.name.as_bytes()).encode();
                self.respond(req.tid, status::SUCCESS, body);
            }
            // ── signatures: what each characteristic is ──
            (iid, opcode::CHARACTERISTIC_SIGNATURE_READ) => {
                match Self::signature_for(iid) {
                    Some(body) => self.respond(req.tid, status::SUCCESS, body),
                    None => self.respond(req.tid, status::INVALID_INSTANCE_ID, Vec::new()),
                }
            }
            (_, _) => self.respond(req.tid, status::INVALID_INSTANCE_ID, Vec::new()),
        }
    }

    /// The signature TLV for one instance, or `None` for an instance with no signature.
    ///
    /// Properties match real hardware: `On` reads and writes, `Outlet In Use` only reads,
    /// `Identify` only writes (a controller cannot ask whether a device is blinking).
    fn signature_for(iid: u16) -> Option<Vec<u8>> {
        use crate::signature::{prop, sig};
        let (ty, props, format) = match iid {
            IID_ON => (characteristic::ON, prop::SECURE_READ | prop::SECURE_WRITE | prop::NOTIFY_CONNECTED, 0x01u8),
            IID_OUTLET_IN_USE => (characteristic::OUTLET_IN_USE, prop::SECURE_READ | prop::NOTIFY_CONNECTED, 0x01),
            IID_IDENTIFY => (characteristic::IDENTIFY, prop::SECURE_WRITE, 0x01),
            IID_NAME => (characteristic::NAME, prop::SECURE_READ, 0x19),
            _ => return None,
        };
        Some(
            Tlv::new()
                .with(sig::CHARACTERISTIC_TYPE, ty.to_le_bytes())
                .with(sig::PROPERTIES, props.to_le_bytes())
                .with(sig::FORMAT, [format, 0x00, 0x00])
                .encode(),
        )
    }

    fn error(state: u8, code: u8) -> Vec<u8> {
        Tlv::new().with(tlv8::TYPE_STATE, [state]).with(tlv8::TYPE_ERROR, [code]).encode()
    }

    fn pair_setup_step(&mut self, bytes: Vec<u8>) -> Vec<u8> {
        let tlv = match Tlv::decode(&bytes) { Ok(t) => t, Err(_) => return Self::error(2, tlv8::error_code::UNKNOWN) };
        let state = tlv.get_u8(tlv8::TYPE_STATE).unwrap_or(0);
        match (std::mem::replace(&mut self.setup, SetupState::Idle), state) {
            (SetupState::Idle, 1) => {
                let server = srp::Server::<sha2::Sha512>::new_random(srp::Group::rfc5054_3072(), self.verifier.clone());
                let m2 = Tlv::new()
                    .with(tlv8::TYPE_STATE, [2u8])
                    .with(tlv8::TYPE_PUBLIC_KEY, server.public_key())
                    .with(tlv8::TYPE_SALT, self.salt)
                    .encode();
                self.setup = SetupState::SentM2 { srp: server };
                m2
            }
            (SetupState::SentM2 { srp }, 3) => {
                let a = tlv.get(tlv8::TYPE_PUBLIC_KEY).unwrap_or(&[]);
                let proof = tlv.get(tlv8::TYPE_PROOF).unwrap_or(&[]);
                match srp.verify(b"Pair-Setup", &self.salt, a, proof) {
                    Ok((m2, key)) => {
                        let session_key = hkdf_32(b"Pair-Setup-Encrypt-Salt", b"Pair-Setup-Encrypt-Info", &key);
                        self.setup = SetupState::SentM4 { srp_key: key, session_key };
                        Tlv::new().with(tlv8::TYPE_STATE, [4u8]).with(tlv8::TYPE_PROOF, m2).encode()
                    }
                    Err(_) => Self::error(4, tlv8::error_code::AUTHENTICATION),
                }
            }
            (SetupState::SentM4 { srp_key, session_key }, 5) => {
                let enc = tlv.get(tlv8::TYPE_ENCRYPTED_DATA).unwrap_or(&[]);
                let plain = match open(&session_key, &label_nonce(b"PS-Msg05"), b"", enc) {
                    Ok(p) => p,
                    Err(_) => return Self::error(6, tlv8::error_code::AUTHENTICATION),
                };
                let sub = match Tlv::decode(&plain) { Ok(t) => t, Err(_) => return Self::error(6, tlv8::error_code::UNKNOWN) };
                let cid = sub.get(tlv8::TYPE_IDENTIFIER).unwrap_or(&[]).to_vec();
                let cltpk: [u8; 32] = match sub.get(tlv8::TYPE_PUBLIC_KEY).and_then(|k| k.try_into().ok()) { Some(k) => k, None => return Self::error(6, tlv8::error_code::UNKNOWN) };
                let sig: [u8; 64] = match sub.get(tlv8::TYPE_SIGNATURE).and_then(|s| s.try_into().ok()) { Some(s) => s, None => return Self::error(6, tlv8::error_code::UNKNOWN) };
                let device_x = hkdf_32(b"Pair-Setup-Controller-Sign-Salt", b"Pair-Setup-Controller-Sign-Info", &srp_key);
                let mut info = device_x.to_vec();
                info.extend_from_slice(&cid);
                info.extend_from_slice(&cltpk);
                let ok = ed25519_dalek::VerifyingKey::from_bytes(&cltpk)
                    .map(|vk| vk.verify(&info, &ed25519_dalek::Signature::from_bytes(&sig)).is_ok())
                    .unwrap_or(false);
                if !ok { return Self::error(6, tlv8::error_code::AUTHENTICATION); }
                self.paired_controller = Some((String::from_utf8_lossy(&cid).into_owned(), cltpk));

                // M6: our identity, signed, encrypted.
                let accessory_x = hkdf_32(b"Pair-Setup-Accessory-Sign-Salt", b"Pair-Setup-Accessory-Sign-Info", &srp_key);
                let signer = match self.fault {
                    Fault::LieAboutIdentity => ed25519_dalek::SigningKey::generate(&mut rand_core::OsRng),
                    _ => self.ltsk.clone(),
                };
                let mut info = accessory_x.to_vec();
                info.extend_from_slice(self.id.as_bytes());
                info.extend_from_slice(&self.ltpk());
                let sig = signer.sign(&info).to_bytes();
                let sub = Tlv::new()
                    .with(tlv8::TYPE_IDENTIFIER, self.id.as_bytes())
                    .with(tlv8::TYPE_PUBLIC_KEY, self.ltpk())
                    .with(tlv8::TYPE_SIGNATURE, sig)
                    .encode();
                let mut enc = seal(&session_key, &label_nonce(b"PS-Msg06"), b"", &sub);
                if self.fault == Fault::TamperM6 { enc[3] ^= 0x55; }
                Tlv::new().with(tlv8::TYPE_STATE, [6u8]).with(tlv8::TYPE_ENCRYPTED_DATA, enc).encode()
            }
            (_, s) => Self::error(s.wrapping_add(1), tlv8::error_code::UNKNOWN),
        }
    }

    fn pair_verify_step(&mut self, bytes: Vec<u8>) -> Vec<u8> {
        let tlv = match Tlv::decode(&bytes) { Ok(t) => t, Err(_) => return Self::error(2, tlv8::error_code::UNKNOWN) };
        let state = tlv.get_u8(tlv8::TYPE_STATE).unwrap_or(0);
        match (std::mem::replace(&mut self.verify, VerifyState::Idle), state) {
            (VerifyState::Idle, 1) => {
                let their_pub: [u8; 32] = match tlv.get(tlv8::TYPE_PUBLIC_KEY).and_then(|k| k.try_into().ok()) { Some(k) => k, None => return Self::error(2, tlv8::error_code::UNKNOWN) };
                let secret = x25519_dalek::StaticSecret::random_from_rng(rand_core::OsRng);
                let our_pub = x25519_dalek::PublicKey::from(&secret).to_bytes();
                let shared = secret.diffie_hellman(&x25519_dalek::PublicKey::from(their_pub)).to_bytes();
                let session_key = hkdf_32(b"Pair-Verify-Encrypt-Salt", b"Pair-Verify-Encrypt-Info", &shared);
                let signer = match self.fault {
                    Fault::VerifyWithWrongKey => ed25519_dalek::SigningKey::generate(&mut rand_core::OsRng),
                    _ => self.ltsk.clone(),
                };
                let mut info = our_pub.to_vec();
                info.extend_from_slice(self.id.as_bytes());
                info.extend_from_slice(&their_pub);
                let sig = signer.sign(&info).to_bytes();
                let sub = Tlv::new().with(tlv8::TYPE_IDENTIFIER, self.id.as_bytes()).with(tlv8::TYPE_SIGNATURE, sig).encode();
                let enc = seal(&session_key, &label_nonce(b"PV-Msg02"), b"", &sub);
                self.verify = VerifyState::SentM2 { shared, session_key, our_pub, their_pub };
                Tlv::new().with(tlv8::TYPE_STATE, [2u8]).with(tlv8::TYPE_PUBLIC_KEY, our_pub).with(tlv8::TYPE_ENCRYPTED_DATA, enc).encode()
            }
            (VerifyState::SentM2 { shared, session_key, our_pub, their_pub }, 3) => {
                let enc = tlv.get(tlv8::TYPE_ENCRYPTED_DATA).unwrap_or(&[]);
                let plain = match open(&session_key, &label_nonce(b"PV-Msg03"), b"", enc) {
                    Ok(p) => p,
                    Err(_) => return Self::error(4, tlv8::error_code::AUTHENTICATION),
                };
                let sub = match Tlv::decode(&plain) { Ok(t) => t, Err(_) => return Self::error(4, tlv8::error_code::UNKNOWN) };
                let cid = sub.get(tlv8::TYPE_IDENTIFIER).unwrap_or(&[]);
                let sig: [u8; 64] = match sub.get(tlv8::TYPE_SIGNATURE).and_then(|s| s.try_into().ok()) { Some(s) => s, None => return Self::error(4, tlv8::error_code::UNKNOWN) };
                let Some((pid, pltpk)) = &self.paired_controller else { return Self::error(4, tlv8::error_code::AUTHENTICATION) };
                if cid != pid.as_bytes() { return Self::error(4, tlv8::error_code::AUTHENTICATION); }
                let mut info = their_pub.to_vec();
                info.extend_from_slice(cid);
                info.extend_from_slice(&our_pub);
                let ok = ed25519_dalek::VerifyingKey::from_bytes(pltpk)
                    .map(|vk| vk.verify(&info, &ed25519_dalek::Signature::from_bytes(&sig)).is_ok())
                    .unwrap_or(false);
                if !ok { return Self::error(4, tlv8::error_code::AUTHENTICATION); }
                let m4 = Tlv::new().with(tlv8::TYPE_STATE, [4u8]).encode();
                // The session starts AFTER M4 goes out in the clear; `write` installs it.
                self.pending_session = Some(SecureSession::accessory_side(&shared));
                m4
            }
            (_, s) => Self::error(s.wrapping_add(1), tlv8::error_code::UNKNOWN),
        }
    }
}

impl Transport for Accessory {
    fn write(&mut self, _char_type: u16, data: &[u8]) -> Result<(), TransportError> {
        let plain = match &mut self.session {
            Some(s) => s.open_ble(data).map_err(|_| TransportError("accessory: frame failed to authenticate".into()))?,
            None => data.to_vec(),
        };
        self.inbound.feed(&plain).map_err(|e| TransportError(format!("accessory: {e}")))?;
        if self.inbound.done() {
            let bytes = std::mem::take(&mut self.inbound).into_bytes();
            let req = Request::decode(&bytes).map_err(|e| TransportError(format!("accessory: {e}")))?;
            self.handle(req);
            // Pair-verify M4 was queued in the clear by `respond`; everything
            // after this point is under the session.
            if let Some(s) = self.pending_session.take() {
                self.session = Some(s);
            }
        }
        Ok(())
    }

    fn read(&mut self, _char_type: u16) -> Result<Vec<u8>, TransportError> {
        self.outbound.pop_front().ok_or_else(|| TransportError("accessory: nothing to read".into()))
    }

    fn instance_id(&mut self, char_type: u16) -> Result<u16, TransportError> {
        match char_type {
            characteristic::PAIR_SETUP => Ok(IID_PAIR_SETUP),
            characteristic::PAIR_VERIFY => Ok(IID_PAIR_VERIFY),
            characteristic::ON => Ok(IID_ON),
            characteristic::OUTLET_IN_USE => Ok(IID_OUTLET_IN_USE),
            characteristic::IDENTIFY => Ok(IID_IDENTIFY),
            characteristic::NAME => Ok(IID_NAME),
            other => Err(TransportError(format!("accessory has no characteristic {other:#04x}"))),
        }
    }

    fn mtu(&self) -> usize { self.mtu }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ble::{Controller, HapError};
    use crate::keys::new_controller_identity;
    use crate::pair_setup::PairSetupError;
    use crate::pair_verify::PairVerifyError;

    fn pair(acc: &mut Accessory, code: &str) -> Result<crate::keys::Pairing, HapError> {
        let (id, sk) = new_controller_identity();
        let mut c = Controller::new(acc);
        c.pair_setup(&id, sk, code)
    }

    /// THE PROBE, against an accessory that has an outlet's characteristics and not a
    /// light's. What the controller may offer is what the accessory said, not a guess
    /// from its advertisement.
    #[test]
    fn a_probe_learns_the_outlet_and_refuses_to_invent_a_dimmer() {
        let mut acc = Accessory::new("AA:BB:CC:DD:EE:FF", "031-45-154", 100);
        acc.name = "desk lamp".into();
        acc.in_use = true;
        let pairing = pair(&mut acc, "03145154").expect("pair-setup");
        let mut c = Controller::new(&mut acc);
        assert_eq!(c.probe().unwrap_err(), HapError::NotVerified, "a probe is a read like any other");
        c.pair_verify(&pairing).expect("pair-verify");

        let caps = c.probe().expect("probe");
        assert!(caps.switchable(), "On is writable");
        assert!(caps.reports_load(), "an outlet says whether it is loaded");
        assert!(caps.identifiable());
        assert!(!caps.dimmable(), "no Brightness on this accessory, and none invented");
        assert_eq!(caps.describe(), "outlet · identify");
        assert_eq!(caps.name.as_deref(), Some("desk lamp"));
        // the read-only one is read-only, and the controller can see that BEFORE writing
        assert!(!caps.get(crate::ble::characteristic::OUTLET_IN_USE).unwrap().writable());

        assert!(c.outlet_in_use().unwrap(), "something is plugged in");
        c.identify().expect("identify");
        drop(c);
        assert_eq!(acc.identified, 1, "the accessory was asked to blink");
        // Brightness never reached the accessory: the transport refused it by name, so
        // the probe skipped it rather than the accessory rejecting a request.
        assert!(!acc.log.iter().any(|l| l.contains("iid 8")), "{:?}", acc.log);
    }

    #[test]
    fn writing_a_read_only_characteristic_is_refused_by_the_accessory() {
        // The signature is the controller's warning; this is what happens when it is
        // ignored. A silent success here would make the UI show a value the plug does
        // not have.
        let mut acc = Accessory::new("AA:BB:CC:DD:EE:FF", "031-45-154", 100);
        let pairing = pair(&mut acc, "03145154").unwrap();
        let mut c = Controller::new(&mut acc);
        c.pair_verify(&pairing).unwrap();
        let err = c.write_value(crate::ble::characteristic::OUTLET_IN_USE, &[1]).unwrap_err();
        assert!(matches!(err, HapError::Status { .. }), "{err:?}");
    }

    #[test]
    fn the_whole_flow_pair_verify_switch_read_at_a_small_mtu() {
        // MTU 60: M2 (salt + 384-byte B) and M3 (A + proof) both fragment,
        // so the reassembler and the fragmenter are exercised both ways,
        // and once the session is up every fragment carries its own tag.
        let mut acc = Accessory::new("AA:BB:CC:DD:EE:FF", "031-45-154", 60);
        let pairing = pair(&mut acc, "03145154").expect("pair-setup");
        assert_eq!(pairing.accessory_id, "AA:BB:CC:DD:EE:FF");
        assert_eq!(pairing.accessory_ltpk, acc.ltpk());
        assert_eq!(acc.paired_controller.as_ref().unwrap().1, pairing.controller_ltpk());

        // A new connection: verify with the stored record, then switch.
        let mut c = Controller::new(&mut acc);
        assert_eq!(c.set_on(true).unwrap_err(), HapError::NotVerified, "no writes before verify");
        c.pair_verify(&pairing).expect("pair-verify");
        assert!(c.is_secure());
        c.set_on(true).unwrap();
        assert!(c.get_on().unwrap());
        c.set_on(false).unwrap();
        assert!(!c.get_on().unwrap());
        assert!(c.trace.iter().any(|l| l.contains("fragment(s)")));
        drop(c);
        assert!(!acc.on);
        assert!(acc.log.iter().filter(|l| l.contains("iid 12")).count() >= 4, "{:?}", acc.log);
    }

    #[test]
    fn the_wrong_setup_code_is_refused_at_m4_with_the_authentication_code() {
        let mut acc = Accessory::new("A", "031-45-154", 512);
        match pair(&mut acc, "999-99-999").unwrap_err() {
            HapError::PairSetup(PairSetupError::Accessory { state: 4, code }) => assert_eq!(code, tlv8::error_code::AUTHENTICATION),
            other => panic!("{other:?}"),
        }
        assert!(acc.paired_controller.is_none(), "nothing stored on the accessory either");
    }

    #[test]
    fn a_tampered_m6_leaves_no_pairing_behind() {
        let mut acc = Accessory::new("A", "031-45-154", 512);
        acc.fault = Fault::TamperM6;
        assert!(matches!(pair(&mut acc, "031-45-154").unwrap_err(), HapError::PairSetup(PairSetupError::IdentityUnverified(_))));
    }

    #[test]
    fn an_accessory_that_signs_with_a_key_it_did_not_present_is_refused() {
        let mut acc = Accessory::new("A", "031-45-154", 512);
        acc.fault = Fault::LieAboutIdentity;
        assert!(matches!(pair(&mut acc, "031-45-154").unwrap_err(), HapError::PairSetup(PairSetupError::IdentityUnverified(_))));
    }

    #[test]
    fn verify_refuses_an_accessory_that_cannot_sign_with_the_paired_key() {
        let mut acc = Accessory::new("A", "031-45-154", 512);
        let pairing = pair(&mut acc, "031-45-154").unwrap();
        acc.fault = Fault::VerifyWithWrongKey;
        let mut c = Controller::new(&mut acc);
        assert!(matches!(c.pair_verify(&pairing).unwrap_err(), HapError::PairVerify(PairVerifyError::AccessoryUnverified(_))));
        assert!(!c.is_secure());
    }

    #[test]
    fn verify_with_someone_elses_pairing_record_is_refused_by_the_accessory() {
        let mut acc = Accessory::new("A", "031-45-154", 512);
        let _ = pair(&mut acc, "031-45-154").unwrap();
        // A record for the same accessory id and LTPK, but a controller it never paired.
        let (id, sk) = new_controller_identity();
        let stranger = crate::keys::Pairing { controller_id: id, controller_ltsk: sk, accessory_id: "A".into(), accessory_ltpk: acc.ltpk() };
        let mut c = Controller::new(&mut acc);
        match c.pair_verify(&stranger).unwrap_err() {
            HapError::PairVerify(PairVerifyError::Accessory { state: 4, code }) => assert_eq!(code, tlv8::error_code::AUTHENTICATION),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_write_to_on_without_a_session_is_refused_by_the_accessory_too() {
        // Bypass the controller's own guard by driving `exchange` directly.
        let mut acc = Accessory::new("A", "031-45-154", 512);
        let mut c = Controller::new(&mut acc);
        let body = Tlv::new().with(param::VALUE, [1u8]).encode();
        match c.exchange(characteristic::ON, opcode::CHARACTERISTIC_WRITE, body).unwrap_err() {
            HapError::Status { status: s, .. } => assert_eq!(s, status::INSUFFICIENT_AUTHENTICATION),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_pairing_record_survives_bytes_and_still_verifies() {
        let mut acc = Accessory::new("A", "031-45-154", 512);
        let p = pair(&mut acc, "031-45-154").unwrap();
        let p2 = crate::keys::Pairing::from_bytes(&p.to_bytes()).unwrap();
        let mut c = Controller::new(&mut acc);
        c.pair_verify(&p2).unwrap();
        c.set_on(true).unwrap();
    }
}
