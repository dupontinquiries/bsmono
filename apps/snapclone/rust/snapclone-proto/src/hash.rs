//! Content hash verification — SHA-256 of plaintext snap payload.
//!
//! The C++ uses QCryptographicHash::Sha256. This matches it exactly.

use sha2::{Digest, Sha256};

/// Compute the hex-encoded SHA-256 of `data`.
/// Matches QCryptographicHash::hash(data, QCryptographicHash::Sha256).toHex()
pub fn compute(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

/// Verify that `data` matches `expected_hex`.
/// Returns `Ok(())` or an error describing the mismatch.
pub fn verify(data: &[u8], expected_hex: &str) -> anyhow::Result<()> {
    let actual = compute(data);
    if actual.eq_ignore_ascii_case(expected_hex) {
        Ok(())
    } else {
        anyhow::bail!(
            "Content hash mismatch: expected {} got {}",
            expected_hex, actual
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_hash() {
        // SHA-256("abc") = ba7816bf...
        let h = compute(b"abc");
        assert!(h.starts_with("ba7816bf"));
    }

    #[test]
    fn verify_ok() {
        let data = b"hello";
        let h = compute(data);
        verify(data, &h).unwrap();
    }

    #[test]
    fn verify_fail() {
        assert!(verify(b"hello", "deadbeef").is_err());
    }
}
