//! memory — `READ BINARY` / `UPDATE BINARY` APDU construction and response
//! parsing, per PC/SC Part 3 §3.2.2.1.8/3.2.2.1.9 (Tables 3-18 through
//! 3-21) — checked directly against the spec PDF, same document that
//! caught the ATR-parsing bug and the fabricated DESFire code elsewhere in
//! this crate.
//!
//! ## What this is for here specifically
//!
//! The write side of a card-based claim ticket: a tiny (a handful of
//! bytes) capability written into a card's free user memory at drop-off
//! time, so ANY reader — not just the device that started a staged file
//! transfer — can redeem it later by reading the card. This is
//! deliberately NOT a way to put files on a card: even the largest storage
//! cards this crate classifies have a few hundred bytes of free memory at
//! most, Ultralight/NTAG-class tags often under 150. `ClaimTicket` is
//! sized to fit comfortably under that floor.
//!
//! ## What this module does NOT attempt
//!
//! Authentication. `MIFARE Classic`-class cards require a `LOAD KEYS` +
//! `GENERAL AUTHENTICATE` exchange (§3.2.2.1.4/3.2.2.1.6) before any
//! sector's `READ BINARY`/`UPDATE BINARY` will succeed; `MIFARE
//! Ultralight`/NTAG-class tags generally don't need this at all. This
//! module only builds the two commands that are safe to attempt
//! regardless of card type — a caller targeting an authenticated sector
//! needs the key-loading dance first, real, separate work this crate
//! doesn't implement.
//!
//! ## Address semantics, stated honestly
//!
//! The spec's own text: for file-oriented cards, `Address` is a byte
//! offset (`MSB*256+LSB`); for block-oriented cards, it's a block number.
//! Which one applies is a property of the specific card, not something
//! this generic builder resolves — a caller needs to already know which
//! kind of card it's talking to.

/// `READ BINARY`: Class 0xFF, INS 0xB0, P1/P2 = address, Le = how many
/// bytes to read (0x00 per the spec means "return the full available
/// length", not zero bytes).
pub fn read_binary_apdu(address: u16, len: u8) -> Vec<u8> {
    let [msb, lsb] = address.to_be_bytes();
    vec![0xFF, 0xB0, msb, lsb, len]
}

/// `UPDATE BINARY`: Class 0xFF, INS 0xD6, P1/P2 = address, Lc = data
/// length, followed by the data itself. `data.len()` must fit in one byte
/// (the short Lc form this spec mandates) — the caller's responsibility to
/// chunk anything larger across multiple calls at incrementing addresses.
pub fn update_binary_apdu(address: u16, data: &[u8]) -> Option<Vec<u8>> {
    if data.len() > u8::MAX as usize {
        return None; // short-Lc APDU can't express this in one call
    }
    let [msb, lsb] = address.to_be_bytes();
    let mut apdu = vec![0xFF, 0xD6, msb, lsb, data.len() as u8];
    apdu.extend_from_slice(data);
    Some(apdu)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApduOutcome {
    Success,
    /// Some of the returned data may be corrupted (Read) — the data is
    /// still handed back by `parse_read_response`, this just flags that
    /// the card itself warned about it.
    PartialDataMayBeCorrupted,
    /// Read: end of file reached before the expected byte count. Update:
    /// end of file reached before writing all of `Lc` bytes.
    EndOfFileEarly,
    CommandIncompatible,
    SecurityNotSatisfied,
    CommandNotAllowed,
    NotSupported,
    /// File not found, or the addressed block/byte doesn't exist.
    NotFound,
    /// Read only: wrong `Le`; the card reports the actual available
    /// length.
    WrongLength { actual: u8 },
    /// Update only: memory failure, the write did not succeed.
    MemoryFailureWriting,
    Unknown { sw1: u8, sw2: u8 },
}

fn interpret_sw(sw1: u8, sw2: u8, is_read: bool) -> ApduOutcome {
    match (sw1, sw2) {
        (0x90, 0x00) => ApduOutcome::Success,
        (0x62, 0x81) => ApduOutcome::PartialDataMayBeCorrupted,
        (0x62, 0x82) => ApduOutcome::EndOfFileEarly,
        (0x69, 0x81) | (0x69, 0x82) | (0x69, 0x86) => {
            match sw2 {
                0x81 => ApduOutcome::CommandIncompatible,
                0x82 => ApduOutcome::SecurityNotSatisfied,
                _ => ApduOutcome::CommandNotAllowed,
            }
        }
        (0x6A, 0x81) => ApduOutcome::NotSupported,
        (0x6A, 0x82) => ApduOutcome::NotFound,
        (0x65, 0x81) if !is_read => ApduOutcome::MemoryFailureWriting,
        (0x6C, actual) if is_read => ApduOutcome::WrongLength { actual },
        _ => ApduOutcome::Unknown { sw1, sw2 },
    }
}

/// Splits a raw `READ BINARY` response into the data payload and its
/// outcome. The response is `Data + SW1 SW2` per Table 3-18b — the last
/// two bytes are always the status word, never part of the data itself.
pub fn parse_read_response(response: &[u8]) -> (Vec<u8>, ApduOutcome) {
    if response.len() < 2 {
        return (Vec::new(), ApduOutcome::Unknown { sw1: 0, sw2: 0 });
    }
    let split = response.len() - 2;
    let (data, sw) = response.split_at(split);
    (data.to_vec(), interpret_sw(sw[0], sw[1], true))
}

/// `UPDATE BINARY`'s response is just the status word (Table 3-20b) — no
/// data payload to separate out.
pub fn parse_update_response(response: &[u8]) -> ApduOutcome {
    if response.len() < 2 {
        return ApduOutcome::Unknown { sw1: 0, sw2: 0 };
    }
    let sw = &response[response.len() - 2..];
    interpret_sw(sw[0], sw[1], false)
}

/// A small, portable capability written into a card's free memory at
/// staging drop-off time — the thing that makes the card a bearer-token
/// for one pending handoff, readable by any reader, not just the device
/// that started the transfer. Deliberately tiny: 12 bytes total, comfortably
/// under even the smallest common tag's free-memory floor (Ultralight/
/// NTAG213-class tags: tens of bytes at minimum, often 48+).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimTicket {
    pub session_id: u32,
    pub nonce: [u8; 8],
}

impl ClaimTicket {
    pub const ENCODED_LEN: usize = 12;

    pub fn to_bytes(&self) -> [u8; Self::ENCODED_LEN] {
        let mut out = [0u8; Self::ENCODED_LEN];
        out[0..4].copy_from_slice(&self.session_id.to_be_bytes());
        out[4..12].copy_from_slice(&self.nonce);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<ClaimTicket> {
        if bytes.len() != Self::ENCODED_LEN {
            return None;
        }
        let session_id = u32::from_be_bytes(bytes[0..4].try_into().ok()?);
        let nonce = bytes[4..12].try_into().ok()?;
        Some(ClaimTicket { session_id, nonce })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_binary_apdu_matches_the_documented_byte_layout() {
        assert_eq!(read_binary_apdu(0x0010, 0x04), vec![0xFF, 0xB0, 0x00, 0x10, 0x04]);
    }

    #[test]
    fn update_binary_apdu_matches_the_documented_byte_layout() {
        let apdu = update_binary_apdu(0x0004, &[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
        assert_eq!(apdu, vec![0xFF, 0xD6, 0x00, 0x04, 0x04, 0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn update_binary_refuses_data_too_long_for_a_short_lc_apdu() {
        let data = vec![0u8; 256];
        assert_eq!(update_binary_apdu(0, &data), None);
    }

    #[test]
    fn a_successful_read_separates_data_from_the_trailing_status_word() {
        let (data, outcome) = parse_read_response(&[0x01, 0x02, 0x03, 0x90, 0x00]);
        assert_eq!(data, vec![0x01, 0x02, 0x03]);
        assert_eq!(outcome, ApduOutcome::Success);
    }

    #[test]
    fn wrong_length_carries_the_cards_own_reported_actual_length() {
        let (_, outcome) = parse_read_response(&[0x6C, 0x05]);
        assert_eq!(outcome, ApduOutcome::WrongLength { actual: 0x05 });
    }

    #[test]
    fn a_write_failure_is_reported_distinctly_from_a_read_specific_code() {
        // 0x6C is Read-specific (wrong Le); Update's own memory-failure
        // code is different (0x65 0x81) — must not cross-apply either
        // interpretation to the wrong command.
        assert_eq!(parse_update_response(&[0x65, 0x81]), ApduOutcome::MemoryFailureWriting);
        assert_eq!(interpret_sw(0x6C, 0x05, false), ApduOutcome::Unknown { sw1: 0x6C, sw2: 0x05 }, "0x6C is a Read code, not valid for Update");
    }

    #[test]
    fn a_too_short_response_is_unknown_not_a_panic() {
        let (data, outcome) = parse_read_response(&[0x90]);
        assert!(data.is_empty());
        assert_eq!(outcome, ApduOutcome::Unknown { sw1: 0, sw2: 0 });
    }

    #[test]
    fn a_claim_ticket_round_trips_through_its_own_wire_format() {
        let ticket = ClaimTicket { session_id: 0xDEADBEEF, nonce: [1, 2, 3, 4, 5, 6, 7, 8] };
        let bytes = ticket.to_bytes();
        assert_eq!(bytes.len(), ClaimTicket::ENCODED_LEN);
        assert_eq!(ClaimTicket::from_bytes(&bytes), Some(ticket));
    }

    #[test]
    fn a_claim_ticket_fits_comfortably_under_even_the_smallest_common_tags_free_memory() {
        // NTAG213-class tags: ~144 bytes total user memory, less once any
        // NDEF/lock-byte overhead is accounted for. 12 bytes leaves huge
        // headroom rather than assuming a generous card.
        assert!(ClaimTicket::ENCODED_LEN <= 48, "must fit even a conservative small-tag budget");
    }

    #[test]
    fn a_wrong_length_claim_ticket_byte_slice_is_rejected_not_padded_or_truncated() {
        assert_eq!(ClaimTicket::from_bytes(&[0u8; 11]), None);
        assert_eq!(ClaimTicket::from_bytes(&[0u8; 13]), None);
    }
}
