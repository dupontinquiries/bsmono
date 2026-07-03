// src/util.rs
//
// v0.1.454: small shared utilities that were duplicated across
// many modules. Consolidated so there's one source of truth.
//
//   - epoch_secs()      → u64 unix seconds (was duplicated in 8
//                         modules: decision_cache, llm_cache,
//                         log_summary, missing_packages,
//                         term_blocks, web_summary, workflow,
//                         and now_secs in two of them)
//   - epoch_secs_i64()  → i64 variant (system_info wanted i64)
//   - fnv1a(&str)       → u64 FNV-1a hash (was in llm_cache, nlp,
//                         and the basis of connections::short_hash)
//
// Behavior is byte-for-byte identical to the originals: each
// epoch helper was `SystemTime::now().duration_since(UNIX_EPOCH)
// .map(|d| d.as_secs()).unwrap_or(0)`; fnv1a used the standard
// 64-bit offset basis 0xcbf29ce484222325 and prime 0x100000001b3.

#![allow(dead_code)]

use std::time::{SystemTime, UNIX_EPOCH};

/// Unix time in whole seconds, or 0 if the clock is before the
/// epoch (shouldn't happen; matches the prior `.unwrap_or(0)`).
pub fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// i64 variant (system_info stored timestamps as i64).
pub fn epoch_secs_i64() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// FNV-1a 64-bit hash of a string. Tiny, dependency-free, good
/// distribution. Used for content-addressed cache keys and stable
/// short IDs. NOT cryptographic.
pub fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Lowercase hex of the low 32 bits of fnv1a(s). Stable short id
/// (connections::short_hash used this for node ids).
pub fn short_hash(s: &str) -> String {
    format!("{:x}", fnv1a(s) & 0xffff_ffff)
}

/// Truncate `s` to at most `max_bytes` bytes, snapping the cut DOWN to
/// the nearest UTF-8 char boundary so the result is always valid `&str`.
///
/// Plain `&s[..max_bytes]` panics when `max_bytes` lands in the middle
/// of a multibyte codepoint — the exact class of bug that crashed the
/// renderer on mojibake/em-dash content (v0.1.396, v0.1.880). Use this
/// anywhere a `&str` is byte-capped for display or storage. Returns the
/// whole string when it already fits; returns `""` when `max_bytes` is 0.
pub fn safe_byte_truncate(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut cut = max_bytes;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    &s[..cut]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_is_deterministic_and_distinct() {
        assert_eq!(fnv1a("hello"), fnv1a("hello"));
        assert_ne!(fnv1a("hello"), fnv1a("world"));
        // Known FNV-1a-64 value for "hello".
        assert_eq!(fnv1a("hello"), 0xa430d84680aabd0b);
    }

    #[test]
    fn fnv1a_empty() {
        // Empty string → the offset basis.
        assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn short_hash_is_8_hex_or_fewer_and_stable() {
        let a = short_hash("node-a");
        assert_eq!(a, short_hash("node-a"));
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(a.len() <= 8);
    }

    #[test]
    fn epoch_secs_is_recent_and_positive() {
        let s = epoch_secs();
        // After 2020 (1577836800) — sanity that we got real time.
        assert!(s > 1_577_836_800, "epoch_secs looks wrong: {}", s);
        assert_eq!(epoch_secs_i64(), epoch_secs_i64()); // doesn't panic
        assert!(epoch_secs_i64() > 0);
    }

    #[test]
    fn safe_byte_truncate_ascii() {
        assert_eq!(safe_byte_truncate("hello world", 5), "hello");
        assert_eq!(safe_byte_truncate("hi", 5), "hi", "fits → whole string");
        assert_eq!(safe_byte_truncate("hello", 5), "hello", "exact fit");
        assert_eq!(safe_byte_truncate("hello", 0), "", "zero → empty");
        assert_eq!(safe_byte_truncate("", 5), "");
    }

    #[test]
    fn safe_byte_truncate_never_splits_multibyte() {
        // "é" is 2 bytes (0xC3 0xA9); "…" is 3 bytes; "🎉" is 4 bytes.
        // A naive &s[..n] at any of the interior byte offsets would panic.
        let s = "café";          // c a f é → bytes: 0..3 ASCII, 3..5 = é
        assert_eq!(safe_byte_truncate(s, 4), "caf", "byte 4 splits é → snap to 3");
        assert_eq!(safe_byte_truncate(s, 5), "café", "byte 5 is the boundary");

        let dash = "a—b";        // — is U+2014, 3 bytes (1..4)
        assert_eq!(safe_byte_truncate(dash, 2), "a", "mid em-dash → snap back to 1");
        assert_eq!(safe_byte_truncate(dash, 3), "a", "still mid em-dash");
        assert_eq!(safe_byte_truncate(dash, 4), "a—", "boundary after em-dash");

        let party = "go🎉now";    // 🎉 is 4 bytes (2..6)
        for n in 3..=5 {
            // every interior offset of the emoji snaps back to "go"
            assert_eq!(safe_byte_truncate(party, n), "go", "n={n}");
        }
        assert_eq!(safe_byte_truncate(party, 6), "go🎉");

        // The historical crash input: mojibake "havenât" (â = 2 bytes).
        let mojibake = "havenât";
        for n in 0..=mojibake.len() {
            // None of these panic, and each result is valid UTF-8.
            let out = safe_byte_truncate(mojibake, n);
            assert!(out.len() <= n);
            assert!(mojibake.starts_with(out));
        }
    }
}
