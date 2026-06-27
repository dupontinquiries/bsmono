//! keys.rs — the "one layer up" hashing snapcore's store expects: map a logical
//! mailbox name to a point in the 160-bit keyspace. Two bucket families realize
//! the DHT-as-host two-layer model:
//!   * FEED bucket — `<namespace>:<id>:<epoch>` → the per-epoch list of pointers a
//!     member polls (multi-value: each post appends one content hash).
//!   * BLOB bucket — `blob:<content_hash>` → the content-addressed sealed blob (a
//!     swarm of holders; identical content de-dups to one key).
//!
//! Content ids are the full lowercase-hex SHA-256; the DHT key is the first 160
//! bits of that digest (snapcore keys are 20 bytes). The feed scheme is
//! byte-compatible with snapclone-group's `group-feed:<group>:<epoch>` — pass
//! `namespace = "group-feed"`.

use sha2::{Digest, Sha256};
use snapcore::routing::NodeId;
use snapcore::store::Key;

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Map a logical name to a keyspace point: the first 160 bits of its SHA-256.
fn key_of(name: &str) -> Key {
    let h = sha256(name.as_bytes());
    let mut k = [0u8; 20];
    k.copy_from_slice(&h[..20]);
    NodeId(k)
}

/// The lowercase-hex SHA-256 content id of a blob (its name in the BLOB bucket).
pub fn content_hash(blob: &[u8]) -> String {
    hex(&sha256(blob))
}

/// The per-epoch FEED bucket key for `<namespace>:<id>:<epoch>`.
pub fn feed_key(namespace: &str, id: &str, epoch: u64) -> Key {
    key_of(&format!("{namespace}:{id}:{epoch}"))
}

/// The content-addressed BLOB bucket key for `blob:<content_hash>`.
pub fn blob_key(content_hash: &str) -> Key {
    key_of(&format!("blob:{content_hash}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_is_64_hex_and_stable() {
        let h = content_hash(b"hello");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(h, content_hash(b"hello"), "deterministic");
        assert!(h != content_hash(b"world"));
    }

    #[test]
    fn feed_key_depends_on_every_part() {
        let a = feed_key("group-feed", "g1", 1);
        assert!(a != feed_key("group-feed", "g1", 2), "epoch matters");
        assert!(a != feed_key("group-feed", "g2", 1), "id matters");
        assert!(a != feed_key("session", "g1", 1), "namespace matters");
        assert!(a == feed_key("group-feed", "g1", 1), "stable");
    }

    #[test]
    fn blob_key_is_content_addressed_and_distinct_from_feed() {
        let h = content_hash(b"some sealed bytes");
        assert!(blob_key(&h) == blob_key(&h));
        assert!(blob_key(&h) != feed_key("group-feed", "g1", 1));
    }
}
