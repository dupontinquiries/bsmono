//! pairings — the Pairings characteristic (spec §5.11/§5.12): removing a pairing, and
//! listing the controllers an accessory holds.
//!
//! ## Why this is not optional
//!
//! An accessory holds ONE pairing (HOMEKIT.md's first constraint, and the reason the
//! pairing dialog shouts before it starts). Until now `unpair` deleted our local record and
//! stopped there — which leaves the accessory paired to a controller that no longer exists.
//! From the plug's side nothing changed: it is still claimed, it will still refuse
//! pair-setup, and the only way back is the factory reset the dialog warned about. The
//! button said "forget this pairing" and did half of it.
//!
//! Remove Pairing is two messages over the Pairings characteristic, inside a verified
//! session:
//!
//! ```text
//!   M1  controller -> accessory   state=1, method=4 (RemovePairing), identifier=<who>
//!   M2  accessory  -> controller  state=2  [, error]
//! ```
//!
//! Removing your OWN pairing is the normal case and the accessory drops the session
//! immediately after answering — which is correct and looks like a transport error if you
//! do not expect it, so [`Controller::remove_pairing`] treats a lost session after a
//! successful M2 as success rather than failure.
//!
//! ## Order matters, and it is the interesting decision
//!
//! Tell the accessory FIRST, delete the local record SECOND. The other order cannot be
//! recovered from: if the local record is gone and the radio call then fails, the
//! controller no longer has the keys needed to ask again, and the accessory is stuck
//! claimed with no way to un-claim it short of the reset. In this order a failure leaves
//! both sides paired, which is a state the user can retry from.
//!
//! ## What is deliberately NOT here
//!
//! Adding a pairing (§5.12.1) — that is how you hand a second controller access, and
//! switcheroo has no second controller to hand it to. The list is here because it answers
//! "is this accessory claimed by something else", which is the question the pairing dialog
//! is warning about.

use crate::ble::{characteristic, Controller, HapError};
use crate::tlv8::{self, Tlv};

/// Pairings methods (spec table 5-4).
pub mod method {
    pub const ADD_PAIRING: u8 = 3;
    pub const REMOVE_PAIRING: u8 = 4;
    pub const LIST_PAIRINGS: u8 = 5;
}

/// A controller an accessory holds a pairing for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedController {
    pub identifier: String,
    pub public_key: [u8; 32],
    /// Admin controllers may add and remove other pairings.
    pub admin: bool,
}

/// Build the M1 body for Remove Pairing.
pub fn remove_request(identifier: &str) -> Vec<u8> {
    Tlv::new()
        .with(tlv8::TYPE_STATE, [1u8])
        .with(tlv8::TYPE_METHOD, [method::REMOVE_PAIRING])
        .with(tlv8::TYPE_IDENTIFIER, identifier.as_bytes())
        .encode()
}

/// Build the M1 body for List Pairings.
pub fn list_request() -> Vec<u8> {
    Tlv::new()
        .with(tlv8::TYPE_STATE, [1u8])
        .with(tlv8::TYPE_METHOD, [method::LIST_PAIRINGS])
        .encode()
}

/// Read an M2: `Ok(())` for a plain success, `Err` carrying the accessory's own reason.
pub fn parse_m2(body: &[u8]) -> Result<(), HapError> {
    let tlv = Tlv::decode(body)?;
    match tlv.get_u8(tlv8::TYPE_STATE) {
        Some(2) => {}
        Some(other) => return Err(HapError::Protocol(format!("pairings M2 is state {other}, expected 2"))),
        None => return Err(HapError::Protocol("pairings M2 has no state".into())),
    }
    match tlv.get_u8(tlv8::TYPE_ERROR) {
        None | Some(0) => Ok(()),
        Some(code) => Err(HapError::Protocol(format!(
            "accessory refused: {}",
            tlv8::error_code::describe(code)
        ))),
    }
}

/// Read a List Pairings M2 into its controllers.
///
/// The reply is a flat TLV list with a separator between records, which is the one part of
/// TLV8 that cannot be read as a map: three controllers means three `identifier` entries,
/// and a decoder that keeps the last of each duplicate type would report one.
pub fn parse_list(body: &[u8]) -> Result<Vec<PairedController>, HapError> {
    parse_m2(body)?;
    let mut out = Vec::new();
    let mut identifier: Option<String> = None;
    let mut public_key: Option<[u8; 32]> = None;
    let mut permissions: u8 = 0;
    let mut flush = |identifier: &mut Option<String>, public_key: &mut Option<[u8; 32]>, permissions: &mut u8, out: &mut Vec<PairedController>| {
        if let (Some(id), Some(pk)) = (identifier.take(), public_key.take()) {
            out.push(PairedController { identifier: id, public_key: pk, admin: *permissions & 1 != 0 });
        }
        *permissions = 0;
    };
    let decoded = Tlv::decode(body)?;
    for (ty, value) in decoded.items() {
        match *ty {
            tlv8::TYPE_IDENTIFIER => identifier = Some(String::from_utf8_lossy(value).to_string()),
            tlv8::TYPE_PUBLIC_KEY => {
                public_key = <[u8; 32]>::try_from(value.as_slice()).ok();
            }
            tlv8::TYPE_PERMISSIONS => permissions = value.first().copied().unwrap_or(0),
            tlv8::TYPE_SEPARATOR => flush(&mut identifier, &mut public_key, &mut permissions, &mut out),
            _ => {}
        }
    }
    flush(&mut identifier, &mut public_key, &mut permissions, &mut out);
    Ok(out)
}

impl Controller<'_> {
    /// Tell the accessory to forget a controller. `identifier` is ours for the ordinary
    /// case — see the module docs on why the local record must be deleted *after* this.
    ///
    /// Needs a verified session, and removing our own pairing ends it: the accessory
    /// answers M2 and drops the session in the same breath. A transport failure after a
    /// successful M2 is therefore expected, not an error.
    pub fn remove_pairing(&mut self, identifier: &str) -> Result<(), HapError> {
        if !self.is_secure() {
            return Err(HapError::NotVerified);
        }
        let body = self.pairings_exchange(remove_request(identifier))?;
        parse_m2(&body)?;
        // The session is gone whether the accessory said so or not.
        self.drop_session();
        Ok(())
    }

    /// Which controllers does this accessory hold? The answer to "is it claimed".
    pub fn list_pairings(&mut self) -> Result<Vec<PairedController>, HapError> {
        if !self.is_secure() {
            return Err(HapError::NotVerified);
        }
        let body = self.pairings_exchange(list_request())?;
        parse_list(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m2(error: Option<u8>) -> Vec<u8> {
        let mut t = Tlv::new().with(tlv8::TYPE_STATE, [2u8]);
        if let Some(e) = error {
            t = t.with(tlv8::TYPE_ERROR, [e]);
        }
        t.encode()
    }

    #[test]
    fn the_request_names_the_method_and_who_is_being_removed() {
        let body = remove_request("AB:CD:EF:01:02:03");
        let t = Tlv::decode(&body).unwrap();
        assert_eq!(t.get_u8(tlv8::TYPE_STATE), Some(1));
        assert_eq!(t.get_u8(tlv8::TYPE_METHOD), Some(method::REMOVE_PAIRING));
        assert_eq!(t.get(tlv8::TYPE_IDENTIFIER).unwrap(), b"AB:CD:EF:01:02:03");
        let l = Tlv::decode(&list_request()).unwrap();
        assert_eq!(l.get_u8(tlv8::TYPE_METHOD), Some(method::LIST_PAIRINGS));
        assert!(l.get(tlv8::TYPE_IDENTIFIER).is_none(), "listing names nobody");
    }

    #[test]
    fn an_m2_carries_the_accessorys_own_reason_for_refusing() {
        assert!(parse_m2(&m2(None)).is_ok());
        assert!(parse_m2(&m2(Some(0))).is_ok(), "error 0 is not an error");
        let e = parse_m2(&m2(Some(tlv8::error_code::AUTHENTICATION))).unwrap_err();
        assert!(format!("{e}").contains("refused"), "{e}");
        // a reply that is not M2 at all
        let wrong = Tlv::new().with(tlv8::TYPE_STATE, [4u8]).encode();
        assert!(parse_m2(&wrong).is_err());
        assert!(parse_m2(&Tlv::new().encode()).is_err(), "no state is not a success");
    }

    #[test]
    fn a_list_of_three_controllers_is_three_records_not_one() {
        // The separator is the whole reason this is not a map lookup: every record uses
        // the same TLV types, so a decoder keeping the last of each would report one
        // controller for an accessory shared by three.
        let mut t = Tlv::new().with(tlv8::TYPE_STATE, [2u8]);
        for (i, admin) in [(1u8, true), (2, false), (3, false)] {
            if i > 1 {
                t = t.with(tlv8::TYPE_SEPARATOR, []);
            }
            t = t
                .with(tlv8::TYPE_IDENTIFIER, format!("ctrl-{i}").as_bytes())
                .with(tlv8::TYPE_PUBLIC_KEY, [i; 32])
                .with(tlv8::TYPE_PERMISSIONS, [admin as u8]);
        }
        let got = parse_list(&t.encode()).unwrap();
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(got[0].identifier, "ctrl-1");
        assert!(got[0].admin && !got[1].admin);
        assert_eq!(got[2].public_key, [3u8; 32]);
        // one controller, no separator
        let one = Tlv::new()
            .with(tlv8::TYPE_STATE, [2u8])
            .with(tlv8::TYPE_IDENTIFIER, b"solo".as_slice())
            .with(tlv8::TYPE_PUBLIC_KEY, [9u8; 32])
            .with(tlv8::TYPE_PERMISSIONS, [1u8]);
        assert_eq!(parse_list(&one.encode()).unwrap().len(), 1);
        // an error reply is not an empty list
        assert!(parse_list(&m2(Some(tlv8::error_code::AUTHENTICATION))).is_err());
    }
}
