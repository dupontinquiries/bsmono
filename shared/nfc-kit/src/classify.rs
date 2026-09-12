//! classify — turns raw bytes a PC/SC reader hands back (an ATR, and
//! optionally a UID from the standard `GET UID` pseudo-APDU) into a
//! human-readable guess at what kind of contactless card this is.
//!
//! ## Honesty about the confidence level here
//!
//! Most PC/SC-compliant contactless readers (the common cheap USB ones —
//! ACR122U and its many clones) report a *synthesized* ATR for storage
//! cards, following the PC/SC Workgroup's Part 3 §3.1.3.2.3.2 (Table 3-6)
//! convention: a fixed header, a variable-length historical-byte block
//! (its length, `n`, sits in the low nibble of byte 1 — genuinely per-card,
//! not a fixed count; an earlier version of this parser wrongly hardcoded
//! `n=15` and would have silently rejected any real card with a different
//! historical-byte count), and inside those historical bytes, the PC/SC
//! storage-card RID followed by a 2-byte "card name" code that identifies
//! the chip family.
//!
//! **What the primary spec (checked directly, PDF in hand) settles**: the
//! byte structure above — confirmed against Table 3-6's actual text, not
//! a community shortcut. **What it explicitly does NOT settle**: the
//! mapping from a specific 2-byte code to a specific chip family. Section
//! 3.1.3.2.3.2 says outright that "its Standard and Card Name describing
//! bytes must be interpreted according to the Part 3 Supplemental
//! Document, maintained by PC/SC" — a separate document, still not
//! available to check directly in this session. `card_name_family`'s
//! entries are instead sourced from `pcsc-tools`' `smartcard_list.txt`
//! (maintained by Ludovic Rousseau, who also maintains `pcsc-lite` itself),
//! whose entries are each labeled "as per PCSC std part3" — a strong,
//! named, checkable secondary source, not the vaguer "widely cited
//! community references" this module relied on before. It caught a real
//! error in the process: an earlier hand-picked entry claimed MIFARE
//! DESFire used code `0x0344`, which doesn't appear anywhere in the
//! verified table — DESFire mostly doesn't use this synthetic ATR
//! convention at all (see `classify`'s fallback case). An unrecognized
//! code, or an ATR that doesn't follow this structural convention at all (a real
//! ISO 7816 contact card, or a reader that doesn't synthesize this
//! format), falls back honestly to "unrecognized", never a guessed
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

/// The PC/SC storage-card RID (registered application identifier), the one
/// piece of this ATR convention that IS a truly fixed byte string.
const PCSC_STORAGE_RID: [u8; 5] = [0xA0, 0x00, 0x00, 0x03, 0x06];

/// Card-name codes for the PC/SC storage-card ATR convention (Table 3-6).
/// Sourced from `pcsc-tools`' `smartcard_list.txt` (maintained by Ludovic
/// Rousseau, who also maintains `pcsc-lite` itself — about as authoritative
/// a secondary source as exists for this), whose own entries are each
/// labeled "as per PCSC std part3". This replaces an earlier, much smaller
/// hand-picked table built from memory of community references — that
/// version had at least one confirmed wrong entry: a fabricated `0x0344`
/// code for MIFARE DESFire, which doesn't actually appear in the verified
/// table at all. Checked against real pcsc-tools examples: DESFire mostly
/// doesn't use this synthetic ATR convention in the first place (see
/// `classify`'s fallback case, and the `desfire_real_atr` test fixture).
fn card_name_family(code: u16) -> Option<(&'static str, &'static [&'static str])> {
    match code {
        0x0000 => Some(("Storage card (name not given by reader)", &["ISO/IEC 14443 or 15693 (exact variant unspecified)"])),
        0x0001 => Some(("MIFARE Classic 1K", &["ISO/IEC 14443-3A", "MIFARE Classic (Crypto1)"])),
        0x0002 => Some(("MIFARE Classic 4K", &["ISO/IEC 14443-3A", "MIFARE Classic (Crypto1)"])),
        0x0003 => Some(("MIFARE Ultralight", &["ISO/IEC 14443-3A", "MIFARE Ultralight (no crypto)"])),
        0x0004 => Some(("Infineon SLE55R_XXXX", &["ISO/IEC 14443-2/3"])),
        0x0006 => Some(("STMicroelectronics SR176", &["ISO/IEC 14443-2/3B"])),
        0x0007 => Some(("STMicroelectronics SRI X4K", &["ISO/IEC 14443-2/3B"])),
        0x0008 => Some(("Atmel AT88RF020", &["ISO/IEC 14443, memory tag"])),
        0x0009 => Some(("Atmel AT88SC0204CRF", &["ISO/IEC 14443, memory tag"])),
        0x000A => Some(("Atmel AT88SC0808CRF", &["ISO/IEC 14443, memory tag"])),
        0x000B => Some(("Atmel AT88SC1616CRF", &["ISO/IEC 14443, memory tag"])),
        0x000C => Some(("Atmel AT88SC3216CRF", &["ISO/IEC 14443, memory tag"])),
        0x000D => Some(("Atmel AT88SC6416CRF", &["ISO/IEC 14443, memory tag"])),
        0x000E => Some(("STMicroelectronics SRF55V10P", &["ISO/IEC 14443-2/3B"])),
        0x000F => Some(("STMicroelectronics SRF55V02P", &["ISO/IEC 14443-2/3B"])),
        0x0010 => Some(("STMicroelectronics SRF55V10S", &["ISO/IEC 14443-2/3B"])),
        0x0011 => Some(("STMicroelectronics SRF55V02S", &["ISO/IEC 14443-2/3B"])),
        0x0012 => Some(("Tag-it (Texas Instruments)", &["ISO/IEC 15693"])),
        0x0013 => Some(("NXP LRI512", &["ISO/IEC 15693"])),
        0x0014 => Some(("NXP ICODE SLI", &["ISO/IEC 15693"])),
        0x0015 => Some(("NXP TEMPSENS", &["ISO/IEC 15693, temperature-sensing tag"])),
        0x0016 => Some(("NXP I-CODE1", &["ISO/IEC 15693-adjacent (proprietary)"])),
        0x0017 => Some(("Inside Secure PicoPass 2K", &["ISO/IEC 15693 (HID iCLASS family)"])),
        0x0018 => Some(("Inside Secure PicoPass 2KS", &["ISO/IEC 15693 (HID iCLASS family)"])),
        0x0019 => Some(("Inside Secure PicoPass 16K", &["ISO/IEC 15693 (HID iCLASS family)"])),
        0x001A => Some(("Inside Secure PicoPass 16Ks", &["ISO/IEC 15693 (HID iCLASS family)"])),
        0x001B => Some(("Inside Secure PicoPass 16K (8x2)", &["ISO/IEC 15693 (HID iCLASS family)"])),
        0x001C => Some(("Inside Secure PicoPass 16KS (8x2)", &["ISO/IEC 15693 (HID iCLASS family)"])),
        0x001D => Some(("Inside Secure PicoPass 32KS (16+16)", &["ISO/IEC 15693 (HID iCLASS family)"])),
        0x001E => Some(("Inside Secure PicoPass 32KS (16+8x2)", &["ISO/IEC 15693 (HID iCLASS family)"])),
        0x001F => Some(("Inside Secure PicoPass 32KS (8x2+16)", &["ISO/IEC 15693 (HID iCLASS family)"])),
        0x0020 => Some(("Inside Secure PicoPass 32KS (8x2+8x2)", &["ISO/IEC 15693 (HID iCLASS family)"])),
        0x0021 => Some(("NXP LRI64", &["ISO/IEC 15693"])),
        0x0022 => Some(("NXP I-CODE UID", &["ISO/IEC 15693"])),
        0x0023 => Some(("NXP I-CODE EPC", &["ISO/IEC 15693"])),
        0x0024 => Some(("NXP LRI12", &["ISO/IEC 15693"])),
        0x0025 => Some(("NXP LRI128", &["ISO/IEC 15693"])),
        0x0026 => Some(("MIFARE Mini", &["ISO/IEC 14443-3A", "MIFARE Classic (Crypto1)"])),
        0x0027 => Some(("Infineon my-d move (SLE 66R01P)", &["ISO/IEC 14443-3A"])),
        0x0028 => Some(("Infineon my-d NFC (SLE 66RxxP)", &["ISO/IEC 14443-3A"])),
        0x0029 => Some(("Infineon my-d proximity 2 (SLE 66RxxS)", &["ISO/IEC 14443-3A"])),
        0x002A => Some(("Infineon my-d proximity enhanced (SLE 55RxxE)", &["ISO/IEC 14443-3A"])),
        0x002B => Some(("Infineon my-d light (SRF 55V01P)", &["ISO/IEC 14443-2/3B"])),
        0x002C => Some(("Infineon PJM Stack Tag (SRF 66V10ST)", &["ISO/IEC 14443-2/3B"])),
        0x002D => Some(("Infineon PJM Item Tag (SRF 66V10IT)", &["ISO/IEC 14443-2/3B"])),
        0x002E => Some(("Infineon PJM Light (SRF 66V01ST)", &["ISO/IEC 14443-2/3B"])),
        0x002F => Some(("Jewel Tag", &["ISO/IEC 14443-3A (NFC Type 1 Tag)"])),
        0x0030 => Some(("Topaz NFC Tag", &["ISO/IEC 14443-3A (NFC Type 1 Tag)"])),
        0x0031 => Some(("Atmel AT88SC0104CRF", &["ISO/IEC 14443, memory tag"])),
        0x0032 => Some(("Atmel AT88SC0404CRF", &["ISO/IEC 14443, memory tag"])),
        0x0033 => Some(("Atmel AT88RF01C", &["ISO/IEC 14443, memory tag"])),
        0x0034 => Some(("Atmel AT88RF04C", &["ISO/IEC 14443, memory tag"])),
        0x0035 => Some(("NXP i-Code SL2", &["ISO/IEC 15693"])),
        0x0036 => Some(("MIFARE Plus SL1 2K", &["ISO/IEC 14443-3A", "MIFARE Classic-compatible (Security Level 1)"])),
        0x0037 => Some(("MIFARE Plus SL1 4K", &["ISO/IEC 14443-3A", "MIFARE Classic-compatible (Security Level 1)"])),
        0x0038 => Some(("MIFARE Plus SL2 2K", &["ISO/IEC 14443-4", "MIFARE Plus (AES, Security Level 2)"])),
        0x0039 => Some(("MIFARE Plus SL2 4K", &["ISO/IEC 14443-4", "MIFARE Plus (AES, Security Level 2)"])),
        0x003A => Some(("MIFARE Ultralight C", &["ISO/IEC 14443-3A", "MIFARE Ultralight C (3DES)"])),
        0x003B => Some(("FeliCa", &["JIS X 6319-4 (FeliCa)"])),
        0x003C => Some(("Melexis Sensor Tag (MLX90129)", &["ISO/IEC 15693, sensor tag"])),
        0x003D => Some(("MIFARE Ultralight EV1", &["ISO/IEC 14443-3A", "MIFARE Ultralight EV1 (no crypto)"])),
        _ => None,
    }
}

/// Locates the RID + standard byte + card-name code within a synthetic
/// storage-card ATR, per PC/SC Part 3 §3.1.3.2.3.2 (Table 3-6) — checked
/// directly against that spec text, not the community-reference shortcuts
/// this module used before.
///
/// Fixed bytes, per the table: `atr[0] == 0x3B` (initial header),
/// `atr[2] == 0x80` (TD1: high nibble 8 = only TD2 follows, low nibble 0 =
/// T=0), `atr[3] == 0x01` (TD2: high nibble 0 = nothing further follows,
/// low nibble 1 = T=1). `atr[1]` is **not** fixed — its own low nibble
/// *is* `n`, the historical-byte count, which varies by card. An earlier
/// version of this parser hardcoded `atr[1] == 0x8F` as part of a fixed
/// 6-byte prefix, which only matched a card with exactly 15 historical
/// bytes and would have silently rejected any real storage card ATR with
/// a different count — a real false-negative, not a hypothetical one,
/// caught by checking the actual PC/SC Part 3 spec text directly rather
/// than trusting an assumed fixed shape.
///
/// The historical bytes themselves then carry, in order: a category
/// indicator (`0x80`), the AID-presence tag (`0x4F`), a length byte, the
/// 5-byte RID, a standard byte, and the 2-byte card-name code — everything
/// after the code (further PIX bytes, RFU) is present per the spec but not
/// needed for family classification, so this parser doesn't require a
/// specific total length beyond having enough bytes to reach the code.
fn locate_card_name_code(atr: &[u8]) -> Option<u16> {
    if atr.len() < 4 {
        return None;
    }
    if atr[0] != 0x3B || atr[2] != 0x80 || atr[3] != 0x01 {
        return None;
    }
    if atr[1] & 0xF0 != 0x80 {
        return None; // high nibble must indicate "only TD1 follows"
    }
    let n = (atr[1] & 0x0F) as usize; // historical byte count — varies per card, not assumed
    let hist_start = 4;
    let hist_end = hist_start + n; // exclusive
    if atr.len() < hist_end {
        return None; // ATR claims more historical bytes than it actually carries
    }
    let hist = &atr[hist_start..hist_end];
    // Within the historical bytes: category indicator, AID-presence tag,
    // length, then the 5-byte RID, then the standard byte, then the 2-byte
    // card-name code.
    if hist.len() < 2 || hist[0] != 0x80 || hist[1] != 0x4F {
        return None;
    }
    let rid_start = 3; // after category indicator, AID tag, and the length byte
    if hist.len() < rid_start + 5 + 1 + 2 {
        return None; // not enough historical bytes to hold RID + standard + code
    }
    if hist[rid_start..rid_start + 5] != PCSC_STORAGE_RID {
        return None;
    }
    let code_start = rid_start + 5 + 1; // skip the RID, then the one standard byte
    Some(u16::from_be_bytes([hist[code_start], hist[code_start + 1]]))
}

/// Parses a raw ATR and classifies the card family it most likely
/// describes. `uid`, if already retrieved via `GET_UID_APDU`, is folded
/// into the profile as a fact, not used to influence the classification
/// itself — UID format alone isn't a reliable family signal the way the
/// synthetic ATR's own card-name code is.
pub fn classify(atr: &[u8], uid: Option<&[u8]>) -> CardProfile {
    let uid_hex = uid.map(hex);
    let atr_hex = hex(atr);

    if let Some(code) = locate_card_name_code(atr) {
        if let Some((family, protocols)) = card_name_family(code) {
            return CardProfile {
                family: family.to_string(),
                protocols: protocols.iter().map(|s| s.to_string()).collect(),
                confidence: Confidence::High,
                notes: Vec::new(),
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

    CardProfile {
        family: "Unrecognized".to_string(),
        protocols: vec![],
        confidence: Confidence::Low,
        notes: vec![
            "Doesn't match the PC/SC synthetic storage-card ATR shape. This is EXPECTED, not a failure, for many ISO 14443-4 cards — MIFARE DESFire in particular usually presents a genuine ATS-derived ATR (PC/SC Part 3 Table 3-5, \"Contactless Smart Card\") rather than the synthetic storage-card format this classifier reads (Table 3-6). A card that never appears with a family name above may simply be one this tool doesn't parse yet, not an unknown chip.".to_string(),
        ],
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

    /// Builds a synthetic storage-card ATR per PC/SC Part 3 Table 3-6,
    /// with an explicit, caller-chosen historical-byte count `n` — the
    /// thing the old, buggy parser assumed was always 15. `extra_tail` is
    /// arbitrary PIX/RFU padding appended after the mandatory RID +
    /// standard + code bytes, letting a test build ATRs of genuinely
    /// different total lengths for the same card-name code.
    fn synthetic_atr(code: u16, extra_tail: &[u8]) -> Vec<u8> {
        let code_bytes = code.to_be_bytes();
        let mut hist = vec![0x80, 0x4F, 0x0C]; // category indicator, AID-presence tag, length (value unchecked by the parser)
        hist.extend_from_slice(&PCSC_STORAGE_RID);
        hist.push(0x00); // standard byte (value unchecked by the parser)
        hist.extend_from_slice(&code_bytes);
        hist.extend_from_slice(extra_tail);
        let n = hist.len() as u8;

        let mut atr = vec![0x3B, 0x80 | n, 0x80, 0x01];
        atr.extend_from_slice(&hist);
        atr.push(0x00); // TCK — value unchecked by this parser
        atr
    }

    fn mifare_1k_atr() -> Vec<u8> {
        synthetic_atr(0x0001, &[])
    }

    /// A real, verified example from pcsc-tools' database (line 6456) of
    /// how an actual DESFire/DESFire EV1 card presents: a short, raw
    /// ATS-derived ATR that does NOT follow the synthetic storage-card
    /// convention at all — confirming this classifier's own honest
    /// fallback note. Replaces an earlier fixture that pretended DESFire
    /// had a synthetic card-name code (0x0344); it doesn't, and that
    /// fabricated code was never real.
    fn desfire_real_atr() -> Vec<u8> {
        vec![0x3B, 0x81, 0x80, 0x01, 0x80, 0x80]
    }

    #[test]
    fn two_cards_with_different_historical_byte_counts_both_classify_correctly() {
        // The actual bug this rewrite fixes: an earlier version hardcoded
        // n=15 as part of a fixed prefix, which would have silently
        // rejected either of these if their real n happened to differ
        // from that assumption. mifare_1k_atr() has no tail (n=11);
        // this one pads with 3 extra RFU-ish bytes (n=14) — genuinely
        // different total ATR lengths, same card-name code.
        let padded = synthetic_atr(0x0001, &[0x00, 0x00, 0x00]);
        assert_ne!(mifare_1k_atr().len(), padded.len(), "sanity: the two fixtures really do have different n");
        let p = classify(&padded, None);
        assert_eq!(p.family, "MIFARE Classic 1K");
        assert_eq!(p.confidence, Confidence::High);
    }

    #[test]
    fn a_claimed_historical_byte_count_longer_than_the_atr_actually_is_is_rejected_not_read_out_of_bounds() {
        // atr[1]'s low nibble claims more historical bytes than the ATR
        // actually contains — malformed input a real (or hostile) reader
        // could hand back. Must be a clean "unrecognized", not a panic
        // from indexing past the end of the slice.
        let atr = vec![0x3B, 0x8F, 0x80, 0x01, 0x80, 0x4F]; // claims n=15, only has 2 historical bytes
        let p = classify(&atr, None);
        assert_eq!(p.confidence, Confidence::Low);
    }

    #[test]
    fn a_mifare_classic_1k_atr_is_identified_with_high_confidence() {
        let p = classify(&mifare_1k_atr(), None);
        assert_eq!(p.family, "MIFARE Classic 1K");
        assert_eq!(p.confidence, Confidence::High);
        assert!(p.protocols.iter().any(|s| s.contains("Crypto1")));
    }

    #[test]
    fn a_real_desfire_atr_correctly_falls_back_to_low_confidence_with_the_iso14443_4_explanation() {
        // The important negative result: DESFire does NOT match the
        // synthetic storage-card shape (verified against a real example,
        // not assumed) — so Low confidence here is the classifier working
        // correctly, not a gap in the lookup table. The fallback note
        // must say so explicitly rather than leave that ambiguous.
        let p = classify(&desfire_real_atr(), None);
        assert_eq!(p.confidence, Confidence::Low);
        assert!(p.notes.iter().any(|n| n.contains("DESFire")), "the explanation must name DESFire specifically, not just say \"unrecognized\"");
    }

    #[test]
    fn a_random_short_atr_is_low_confidence_not_a_wrong_guess() {
        let p = classify(&[0x3B, 0x00], None);
        assert_eq!(p.confidence, Confidence::Low);
        assert_eq!(p.family, "Unrecognized");
    }

    #[test]
    fn a_synthetic_shell_with_an_unknown_code_is_medium_not_a_false_family_name() {
        let atr = synthetic_atr(0xFFFF, &[]); // a code not in the table
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
        let p = classify(&desfire_real_atr(), Some(&[0x01, 0x02]));
        let s = copy_summary(&p);
        assert!(s.contains("Unrecognized"));
        assert!(s.contains("01 02"));
        assert!(s.contains("DESFire"));
        assert!(s.contains(&p.atr_hex));
    }

    #[test]
    fn spot_checks_on_the_newly_verified_table_beyond_mifare_classic() {
        // A handful of entries from the pcsc-tools rewrite that weren't in
        // the original hand-picked table at all — confirms the whole
        // replacement table is actually wired up, not just the two
        // entries the other tests happen to exercise.
        assert_eq!(classify(&synthetic_atr(0x003B, &[]), None).family, "FeliCa");
        assert_eq!(classify(&synthetic_atr(0x0017, &[]), None).family, "Inside Secure PicoPass 2K");
        assert_eq!(classify(&synthetic_atr(0x003D, &[]), None).family, "MIFARE Ultralight EV1");
        assert_eq!(classify(&synthetic_atr(0x0000, &[]), None).confidence, Confidence::High, "code 0 is itself a defined, real entry (\"name not given\"), not a gap");
    }

    #[test]
    fn get_uid_apdu_matches_the_documented_standard_bytes() {
        assert_eq!(GET_UID_APDU, [0xFF, 0xCA, 0x00, 0x00, 0x00]);
    }
}
