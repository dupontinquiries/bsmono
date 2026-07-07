//! Run identifiers for cross-referencing log lines, structured events,
//! and `run_summaries.txt` entries.
//!
//! A run UUID is generated once at startup and threaded through:
//!   - The env_logger format (each log line is prefixed with the UUID).
//!   - The structured `events.jsonl` file (every event records the UUID).
//!   - The `RunBlock` appended to `run_summaries.txt` (so a reader can
//!     find the matching events.jsonl entries).
//!
//! ## Format
//!
//! `r-` + 16 hex chars from a random u64. Looks like `r-9f3a2b1c0e4d5678`.
//! Short enough to be readable in a log prefix, long enough to be unique
//! across a developer's iteration lifetime (2^64 ≈ 1.8e19 possible IDs).
//!
//! The `r-` prefix is a visual cue — when scanning a log file or grep
//! results, an unprefixed hex blob could be a memory address, a hash,
//! or many other things. `r-` says "this is a run ID."

use rand::Rng;

/// Generate a fresh run UUID.
pub fn fresh() -> String {
    let n: u64 = rand::thread_rng().gen();
    format!("r-{:016x}", n)
}

/// Validate a string matches the run-UUID format. Useful for the
/// `unendlich-analyze` binary which accepts UUIDs on the command line.
///
/// Standardised on lowercase hex to match `fresh()`'s `{:016x}` output.
/// Uppercase hex is rejected so a stray copy-paste from a different
/// format doesn't accidentally validate.
pub fn is_valid(s: &str) -> bool {
    s.starts_with("r-")
        && s.len() == 18
        && s[2..].chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_returns_valid() {
        for _ in 0..100 {
            let id = fresh();
            assert!(is_valid(&id), "fresh() produced invalid id: {}", id);
        }
    }

    #[test]
    fn fresh_returns_distinct() {
        // Birthday paradox: 100 IDs from a 2^64 space have negligible
        // collision probability. If this test ever fails, either the
        // RNG is broken or you got struck by lightning.
        let mut ids = std::collections::HashSet::new();
        for _ in 0..100 {
            ids.insert(fresh());
        }
        assert_eq!(ids.len(), 100, "fresh() collided in 100 draws");
    }

    #[test]
    fn is_valid_accepts_well_formed() {
        assert!(is_valid("r-0123456789abcdef"));
        assert!(is_valid("r-ffffffffffffffff"));
        assert!(is_valid("r-0000000000000000"));
    }

    #[test]
    fn is_valid_rejects_malformed() {
        assert!(!is_valid(""));
        assert!(!is_valid("r-"));
        assert!(!is_valid("r-abc"));               // too short
        assert!(!is_valid("r-0123456789abcdefg"));  // too long, bad char
        assert!(!is_valid("r-0123456789abcdef0")); // too long
        assert!(!is_valid("r_0123456789abcdef"));  // wrong separator
        assert!(!is_valid("0123456789abcdef"));    // no prefix
        assert!(!is_valid("r-0123456789ABCDEF"));  // we standardise on lowercase
    }
}
