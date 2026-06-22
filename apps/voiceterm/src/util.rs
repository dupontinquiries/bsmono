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
}
