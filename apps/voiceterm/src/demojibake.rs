//! v0.1.708: mojibake repair. Some captured text arrives double-encoded —
//! a UTF-8 string was decoded as Latin-1 and re-encoded as UTF-8, so "→"
//! (e2 86 92) became "â\u{86}\u{92}" (c3a2 c286 c292). The corpus showed this
//! on arrow/dash separators in captured paragraph text.
//!
//! Repair: re-encode each char as a single Latin-1 byte, then decode the byte
//! run as UTF-8. Only applied when the string actually looks double-encoded
//! (every char is U+0000..=U+00FF and a telltale "Â"/"â" + control/high run is
//! present), so clean text and genuine non-Latin scripts pass through
//! untouched. PURE + testable.

/// Returns true if `s` looks like Latin-1↔UTF-8 double-encoded mojibake.
pub fn looks_mojibaked(s: &str) -> bool {
    // every char must fit in a Latin-1 byte (else it can't be the bad
    // re-decode), and at least one must be high (≥0x80) — pure ASCII is fine.
    let mut has_high = false;
    for c in s.chars() {
        let v = c as u32;
        if v > 0xFF { return false; }
        if v >= 0x80 { has_high = true; }
    }
    if !has_high { return false; }
    // telltale: reinterpreting the chars as raw bytes yields VALID UTF-8 that
    // contains a non-ASCII char and differs from the input — i.e. it
    // round-trips to something real (the "→"/"—"/"·" that got mangled).
    let bytes: Vec<u8> = s.chars().map(|c| c as u8).collect();
    match std::str::from_utf8(&bytes) {
        Ok(decoded) => decoded != s && decoded.chars().any(|c| (c as u32) > 0x7F),
        Err(_) => false,
    }
}

/// Repair Latin-1↔UTF-8 double-encoding if present; otherwise return the input
/// unchanged. Safe to call on any text.
pub fn demojibake(s: &str) -> String {
    if !looks_mojibaked(s) {
        return s.to_string();
    }
    // each char is ≤ 0xFF → collect as raw bytes, re-interpret as UTF-8.
    let bytes: Vec<u8> = s.chars().map(|c| c as u8).collect();
    match std::str::from_utf8(&bytes) {
        Ok(fixed) => fixed.to_string(),
        // invalid as UTF-8 (e.g. the corpus's truncated runs) → keep original
        // rather than emit replacement chars.
        Err(_) => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // build the known mojibake of a string the way the bug produced it.
    fn mojify(s: &str) -> String {
        // utf8 bytes re-read as latin-1 → each byte becomes a char
        s.bytes().map(|b| b as char).collect()
    }

    #[test]
    fn repairs_arrow() {
        let bad = mojify("→");           // "â\u{86}\u{92}"
        assert!(looks_mojibaked(&bad));
        assert_eq!(demojibake(&bad), "→");
    }

    #[test]
    fn repairs_emdash_and_bullet() {
        assert_eq!(demojibake(&mojify("a — b")), "a — b");
        assert_eq!(demojibake(&mojify("x · y")), "x · y");
    }

    #[test]
    fn repairs_accented_word() {
        assert_eq!(demojibake(&mojify("café")), "café");
    }

    #[test]
    fn clean_ascii_untouched() {
        assert_eq!(demojibake("welcome to web2qml"), "welcome to web2qml");
        assert!(!looks_mojibaked("welcome to web2qml"));
    }

    #[test]
    fn genuine_unicode_untouched() {
        // real non-Latin text has chars > 0xFF → not treated as mojibake
        assert_eq!(demojibake("日本語 →"), "日本語 →");
        assert!(!looks_mojibaked("日本語"));
    }

    #[test]
    fn idempotent_on_clean() {
        let once = demojibake(&mojify("→ test"));
        assert_eq!(demojibake(&once), once); // re-running doesn't corrupt
    }
}
