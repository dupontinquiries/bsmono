//! classify — turns raw bytes a PC/SC reader hands back (an ATR, and
//! optionally a UID from the standard `GET UID` pseudo-APDU) into a
//! human-readable guess at what kind of contactless card this is.
//!
//! ## Honesty about the confidence level here
//!
//! Most PC/SC-compliant contactless readers (the common cheap USB ones —
//! ACR122U and its many clones) report a *synthesized* ATR for storage
//! cards, following the PC/SC Workgroup's Part 3 supplemental convention:
//! a fixed prefix, the PC/SC storage-card RID, then a 2-byte "card name"
//! code that identifies the chip family. The `card_name_table` below is
//! populated from values that are widely and consistently cited across
//! libnfc/pcsc community references — it has NOT been checked against the
//! primary PC/SC Part 3 specification document in this session (no access
//! to fetch it here), so treat an exact family match as a strong hint, not
//! a certified identification. An unrecognized 2-byte code, or an ATR that
//! doesn't follow this synthetic shape at all (a real ISO 7816 contact
//! card, or a reader that doesn't synthesize this format), falls back
//! honestly to "ISO 14443-4 generic" or "unrecognized", never a guessed
//! specific chip name.
//!
//! The `GET UID` command (`FF CA 00 00 00`) is on much firmer ground: it's
//! the standard PC/SC pseudo-APDU essentially every contactless reader
//! implements for retrieving the anticollision UID, independent of card
//! type — safe to always attempt.

/// The standard PC/SC "Get Data — UID" pseudo-APDU. Not card-type-specific;
/// safe to send to anything ISO 14443-3 compliant.
pub const GET_UID_APDU: [u8; 5] = [0xFF, 0xCA, 0x00, 0x00, 0x00];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confidence {
    /// Matched the PC/SC synthetic-ATR shape AND a known card-name code.
    High,
    /// Matched the synthetic-ATR shape (a real contactless storage/smart
    /// card) but the card-name code isn't in the lookup table.
    Medium,
    /// Doesn't match the synthetic-ATR shape at all — a real ISO 7816
    /// contact ATR, a reader not using this convention, or garbage.
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardProfile {
    pub family: String,
    pub protocols: Vec<String>,
    pub confidence: Confidence,
    /// Anything worth surfacing that isn't a clean fact — e.g. "UID may be
    /// randomized for privacy; taps of the same physical card can show a
    /// different UID each time" for cards known to support that.
    pub notes: Vec<String>,
    pub uid_hex: Option<String>,
    pub atr_hex: String,
}

/// The PC/SC Part 3 synthetic storage-card ATR prefix, common across the
/// contactless readers that use this convention. Bytes after this prefix
/// carry the RID, standard, and card-name code this module actually reads.
const SYNTHETIC_PREFIX: [u8; 6] = [0x3B, 0x8F, 0x80, 0x01, 0x80, 0x4F];
/// The PC/SC storage-card RID (registered application identifier),
/// expected right after the prefix and a length byte.
const PCSC_STORAGE_RID: [u8; 5] = [0xA0, 0x00, 0x00, 0x03, 0x06];

fn card_name_family(code: u16) -> Option<(&'static str, &'static [&'static str])> {
    // Widely-cited values, not independently verified against the primary
    // spec in this session — see this module's own header. Extend rather
    // than trust blindly if precision matters for a specific purchase
    // decision.
    match code {
        0x0001 => Some(("MIFARE Classic 1K", &["ISO/IEC 14443-3A", "MIFARE Classic (Crypto1)"])),
        0x0002 => Some(("MIFARE Classic 4K", &["ISO/IEC 14443-3A", "MIFARE Classic (Crypto1)"])),
        0x0003 => Some(("MIFARE Ultralight", &["ISO/IEC 14443-3A", "MIFARE Ultralight (no crypto)"])),
        0x0026 => Some(("MIFARE Mini", &["ISO/IEC 14443-3A", "MIFARE Classic (Crypto1)"])),
        0x0036 => Some(("MIFARE Plus 2K (SL1)", &["ISO/IEC 14443-3A", "MIFARE Classic-compatible"])),
        0x0037 => Some(("MIFARE Plus 4K (SL1)", &["ISO/IEC 14443-3A", "MIFARE Classic-compatible"])),
        0x0038 => Some(("MIFARE Plus 2K (SL2)", &["ISO/IEC 14443-4", "MIFARE Plus (AES)"])),
        0x0039 => Some(("MIFARE Plus 4K (SL2)", &["ISO/IEC 14443-4", "MIFARE Plus (AES)"])),
        0x0344 => Some(("MIFARE DESFire", &["ISO/IEC 14443-4", "MIFARE DESFire (AES/3DES)"])),
        0xF004 => Some(("Topaz/Jewel", &["ISO/IEC 14443-3A (NFC Type 1 Tag)"])),
        0xF011 => Some(("FeliCa 212K", &["JIS X 6319-4 (FeliCa)"])),
        0xF012 => Some(("FeliCa 424K", &["JIS X 6319-4 (FeliCa)"])),
        _ => None,
    }
}

/// Parses a raw ATR and classifies the card family it most likely
/// describes. `uid`, if already retrieved via `GET_UID_APDU`, is folded
/// into the profile as a fact, not used to influence the classification
/// itself — UID format alone isn't a reliable family signal the way the
/// synthetic ATR's own card-name code is.
pub fn classify(atr: &[u8], uid: Option<&[u8]>) -> CardProfile {
    let uid_hex = uid.map(hex);
    let atr_hex = hex(atr);

    if atr.len() >= 20 && atr[0..6] == SYNTHETIC_PREFIX {
        // Byte 6 is a length/count byte for what follows; the RID sits at
        // 7..12, the "standard" byte at 12, and the 2-byte card-name code
        // at 13..15 — this exact offset layout is the part most consistently
        // reproduced across community references for this ATR convention.
        if atr.len() >= 15 && atr[7..12] == PCSC_STORAGE_RID {
            let code = u16::from_be_bytes([atr[13], atr[14]]);
            if let Some((family, protocols)) = card_name_family(code) {
                let mut notes = Vec::new();
                if matches!(code, 0x0344) {
                    notes.push(
                        "DESFire cards commonly randomize their anticollision UID for privacy — the same physical card can report a different UID on separate taps unless read past this layer via a card-specific GetCardUID command.".to_string(),
                    );
                }
                return CardProfile {
                    family: family.to_string(),
                    protocols: protocols.iter().map(|s| s.to_string()).collect(),
                    confidence: Confidence::High,
                    notes,
                    uid_hex,
                    atr_hex,
                };
            }
            return CardProfile {
                family: format!("Unrecognized contactless storage card (code {code:#06x})"),
                protocols: vec!["ISO/IEC 14443 (exact variant unknown)".to_string()],
                confidence: Confidence::Medium,
                notes: vec!["Matches the PC/SC synthetic storage-card ATR shape, but this specific card-name code isn't in this tool's lookup table yet.".to_string()],
                uid_hex,
                atr_hex,
            };
        }
    }

    CardProfile {
        family: "Unrecognized".to_string(),
        protocols: vec![],
        confidence: Confidence::Low,
        notes: vec!["Doesn't match the PC/SC synthetic storage-card ATR shape — could be a real ISO 7816 contact card, a reader that doesn't synthesize this convention, or an unsupported card type.".to_string()],
        uid_hex,
        atr_hex,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

/// A one-line copyable summary — the actual "copy protocol info to
/// clipboard" content: family, confidence, protocol list, UID if known.
pub fn copy_summary(profile: &CardProfile) -> String {
    let mut lines = vec![
        format!("Family: {} ({:?} confidence)", profile.family, profile.confidence),
        format!("Protocols: {}", if profile.protocols.is_empty() { "none identified".into() } else { profile.protocols.join(", ") }),
    ];
    if let Some(uid) = &profile.uid_hex {
        lines.push(format!("UID: {uid}"));
    }
    lines.push(format!("ATR: {}", profile.atr_hex));
    for note in &profile.notes {
        lines.push(format!("Note: {note}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real, commonly-reproduced example ATR for a MIFARE Classic 1K
    /// card via a PC/SC-synthesizing reader.
    fn mifare_1k_atr() -> Vec<u8> {
        vec![
            0x3B, 0x8F, 0x80, 0x01, 0x80, 0x4F, 0x0C, 0xA0, 0x00, 0x00, 0x03, 0x06, 0x03, 0x00,
            0x01, 0x00, 0x00, 0x00, 0x00, 0x6A,
        ]
    }

    fn desfire_atr() -> Vec<u8> {
        vec![
            0x3B, 0x8F, 0x80, 0x01, 0x80, 0x4F, 0x0C, 0xA0, 0x00, 0x00, 0x03, 0x06, 0x03, 0x03,
            0x44, 0x00, 0x00, 0x00, 0x00, 0x6A,
        ]
    }

    #[test]
    fn a_mifare_classic_1k_atr_is_identified_with_high_confidence() {
        let p = classify(&mifare_1k_atr(), None);
        assert_eq!(p.family, "MIFARE Classic 1K");
        assert_eq!(p.confidence, Confidence::High);
        assert!(p.protocols.iter().any(|s| s.contains("Crypto1")));
    }

    #[test]
    fn a_desfire_atr_carries_the_uid_randomization_note() {
        let p = classify(&desfire_atr(), None);
        assert_eq!(p.family, "MIFARE DESFire");
        assert!(p.notes.iter().any(|n| n.contains("randomize")), "the privacy caveat must be surfaced, not silently omitted");
    }

    #[test]
    fn a_random_short_atr_is_low_confidence_not_a_wrong_guess() {
        let p = classify(&[0x3B, 0x00], None);
        assert_eq!(p.confidence, Confidence::Low);
        assert_eq!(p.family, "Unrecognized");
    }

    #[test]
    fn a_synthetic_shell_with_an_unknown_code_is_medium_not_a_false_family_name() {
        let mut atr = mifare_1k_atr();
        atr[13] = 0xFF;
        atr[14] = 0xFF; // a code not in the table
        let p = classify(&atr, None);
        assert_eq!(p.confidence, Confidence::Medium);
        assert!(p.family.contains("Unrecognized"), "must not confidently misname an unknown code as MIFARE Classic");
    }

    #[test]
    fn a_provided_uid_is_carried_through_as_a_fact_not_used_to_reclassify() {
        let p = classify(&mifare_1k_atr(), Some(&[0x04, 0x1A, 0x2B, 0x3C]));
        assert_eq!(p.uid_hex, Some("04 1A 2B 3C".to_string()));
        assert_eq!(p.family, "MIFARE Classic 1K", "UID presence must not change the family classification");
    }

    #[test]
    fn copy_summary_includes_every_fact_and_every_note() {
        let p = classify(&desfire_atr(), Some(&[0x01, 0x02]));
        let s = copy_summary(&p);
        assert!(s.contains("MIFARE DESFire"));
        assert!(s.contains("01 02"));
        assert!(s.contains("randomize"));
        assert!(s.contains(&p.atr_hex));
    }

    #[test]
    fn get_uid_apdu_matches_the_documented_standard_bytes() {
        assert_eq!(GET_UID_APDU, [0xFF, 0xCA, 0x00, 0x00, 0x00]);
    }
}
