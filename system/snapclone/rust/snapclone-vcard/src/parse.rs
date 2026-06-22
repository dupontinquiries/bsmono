//! vCard 3.0 parser — ported from VCardIO::importContact().
//!
//! Handles both CRLF (RFC 2426 standard) and bare LF line endings.
//! Implements line folding (RFC 2426 §2.6): a line that starts with
//! a space or tab is a continuation of the previous line.

use crate::{escape::unescape, VCard};
use anyhow::{bail, Result};

/// Parse a vCard 3.0 string into a `VCard`.
/// Returns `Err` for completely invalid input; missing optional fields default to empty.
pub fn parse_vcard(input: &str) -> Result<VCard> {
    // Unfold lines (CRLF + space/tab → continuation)
    let unfolded = unfold(input);
    let mut card = VCard::default();
    let mut in_card = false;

    for raw_line in unfolded.lines() {
        let line = raw_line.trim_end();
        if line.eq_ignore_ascii_case("BEGIN:VCARD") { in_card = true;  continue; }
        if line.eq_ignore_ascii_case("END:VCARD")   { in_card = false; break;    }
        if !in_card { continue; }

        // Split at first ':' to get property name (+ params) and value
        let Some(colon) = line.find(':') else { continue };
        let prop_part = &line[..colon];
        let value     = unescape(&line[colon + 1..]);

        // Strip parameters (e.g. "TEL;TYPE=VOICE" → "TEL")
        let prop = prop_part.split(';').next().unwrap_or("").to_ascii_uppercase();

        match prop.as_str() {
            "FN"              => card.display_name = value,
            "NICKNAME"        => card.nickname     = value,
            "NOTE"            => card.notes        = value,
            "UID"             => card.uid          = value,
            "REV"             => card.rev          = Some(value),
            "X-SNAPCLONE-ID"  => card.sc_id        = Some(value),
            "TEL"             => card.phone.push(value),
            "EMAIL"           => card.email.push(value),
            "ADR"             => card.address      = Some(value),
            "PHOTO"           => {
                // Base64 photo — value may be the data or empty (data on continuation lines)
                // For simplicity, store whatever is on this line
                if !value.trim().is_empty() {
                    card.photo_b64 = Some(value.trim().to_owned());
                }
            }
            "N" if card.display_name.is_empty() => {
                // Fallback: extract from N: field (family;given;...)
                let parts: Vec<&str> = value.splitn(2, ';').collect();
                if let Some(name) = parts.first() {
                    if !name.is_empty() { card.display_name = name.to_owned(); }
                }
            }
            _ => {} // unknown property — ignore per RFC
        }
    }

    if !in_card && card.display_name.is_empty() && card.uid.is_empty() {
        bail!("No valid VCARD block found");
    }
    Ok(card)
}

/// Unfold RFC 2426 line folding: CRLF (or LF) followed by space/tab.
fn unfold(input: &str) -> String {
    // Replace CRLF with LF first for uniform processing
    let normalised = input.replace("\r\n", "\n");
    let mut out = String::with_capacity(normalised.len());
    let mut prev_was_newline = false;
    for ch in normalised.chars() {
        if prev_was_newline && (ch == ' ' || ch == '\t') {
            // Continuation: remove the newline that was just added
            if out.ends_with('\n') { out.pop(); }
            prev_was_newline = false;
        } else {
            prev_was_newline = ch == '\n';
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice Wonderland\r\n\
        NICKNAME:Ali\r\nUID:abc-123\r\nX-SNAPCLONE-ID:sc-deadbeef\r\n\
        NOTE:Best friend\\, always\r\nEND:VCARD\r\n";

    #[test]
    fn parse_basic() {
        let v = parse_vcard(SAMPLE).unwrap();
        assert_eq!(v.display_name, "Alice Wonderland");
        assert_eq!(v.nickname,     "Ali");
        assert_eq!(v.uid,          "abc-123");
        assert_eq!(v.sc_id,        Some("sc-deadbeef".to_owned()));
        assert_eq!(v.notes,        "Best friend, always");
    }

    #[test]
    fn roundtrip() {
        use crate::write::write_vcard;
        let original = VCard {
            display_name: "Bob Test".to_owned(),
            nickname:     "Bobby".to_owned(),
            notes:        "Some notes; with semicolons".to_owned(),
            sc_id:        Some("sc-12345678".to_owned()),
            ..Default::default()
        };
        let text   = write_vcard(&original);
        let parsed = parse_vcard(&text).unwrap();
        assert_eq!(parsed.display_name, original.display_name);
        assert_eq!(parsed.nickname,     original.nickname);
        assert_eq!(parsed.notes,        original.notes);
        assert_eq!(parsed.sc_id,        original.sc_id);
    }

    #[test]
    fn lf_line_endings() {
        let input = "BEGIN:VCARD\nVERSION:3.0\nFN:Test\nEND:VCARD\n";
        let v = parse_vcard(input).unwrap();
        assert_eq!(v.display_name, "Test");
    }

    #[test]
    fn line_folding() {
        let input = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Very Long\r\n \
            Name Continued\r\nEND:VCARD\r\n";
        let v = parse_vcard(input).unwrap();
        assert_eq!(v.display_name, "Very LongName Continued");
    }
}
