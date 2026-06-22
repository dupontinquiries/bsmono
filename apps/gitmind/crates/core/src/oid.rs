// gitmind-core/src/oid.rs
//
// Content address for everything in the chunk store: chunks, blob
// manifests, media proxies. We use BLAKE3 (32-byte digest) rather than
// git's SHA-1/SHA-256 because:
//   - it is a Merkle tree internally, so hashing one big file
//     parallelizes across cores (relevant to ingest throughput);
//   - no known collision weakness (SHA-1 has chosen-prefix attacks);
//   - it is fast enough that hashing is never the bottleneck — the
//     realistic answer to "make git operations faster" is "stop using
//     a slow hash + slow compression", not "move it to the GPU".
//
// Oid is deliberately Copy + fixed-size so it lives in redb keys and
// manifest vectors without allocation.

use serde::{Deserialize, Serialize};
use std::fmt;

/// 32-byte BLAKE3 content address.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Oid(pub [u8; 32]);

impl Oid {
    /// Hash arbitrary bytes. Used for chunks and for manifests (a
    /// manifest's Oid is the hash of its serialized bytes, making the
    /// whole blob content-addressed end to end).
    pub fn hash(bytes: &[u8]) -> Self {
        Oid(*blake3::hash(bytes).as_bytes())
    }

    /// Lowercase hex, full 64 chars.
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in &self.0 {
            // Cheap two-nibble hex without pulling a formatting crate.
            const H: &[u8; 16] = b"0123456789abcdef";
            s.push(H[(b >> 4) as usize] as char);
            s.push(H[(b & 0xf) as usize] as char);
        }
        s
    }

    /// Short form for UI/CLI display (first 12 hex chars, like git's
    /// abbreviated hashes).
    pub fn short(&self) -> String {
        self.to_hex()[..12].to_string()
    }

    /// Parse from full hex; returns None on bad length/char so callers
    /// can surface a clean error rather than panic.
    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 64 { return None; }
        let mut out = [0u8; 32];
        let bytes = s.as_bytes();
        for i in 0..32 {
            let hi = hex_val(bytes[i * 2])?;
            let lo = hex_val(bytes[i * 2 + 1])?;
            out[i] = (hi << 4) | lo;
        }
        Some(Oid(out))
    }
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

// Debug shows the short form so log lines stay readable.
impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Oid({})", self.short())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrips() {
        let oid = Oid::hash(b"hello world");
        let hex = oid.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(Oid::from_hex(&hex), Some(oid));
    }

    #[test]
    fn stable_and_distinct() {
        // Same input → same address (dedup depends on this); different
        // input → different address.
        assert_eq!(Oid::hash(b"abc"), Oid::hash(b"abc"));
        assert_ne!(Oid::hash(b"abc"), Oid::hash(b"abd"));
    }

    #[test]
    fn rejects_bad_hex() {
        assert_eq!(Oid::from_hex("xyz"), None);
        assert_eq!(Oid::from_hex(&"z".repeat(64)), None);
    }
}
