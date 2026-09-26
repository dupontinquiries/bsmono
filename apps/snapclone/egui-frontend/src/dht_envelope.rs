//! dht_envelope — the byte-for-byte envelope the Qt build puts DHT records
//! in, so the two halves can actually read each other.
//!
//! `dht_announce` ports the PLAINTEXT format and said so honestly: same
//! format, not yet the same bytes, because the C++ wraps it in an
//! `encryptPayload`/`decryptPayload` pair keyed off the sc-ID. Until that
//! exists a Rust peer publishes records a Qt peer discards and vice versa —
//! which for a user means the fleet silently splits in two at the version
//! boundary, with both halves working perfectly on their own. This closes
//! it.
//!
//! ## What the scheme actually is, stated plainly
//!
//! * A key derived from the sc-ID alone, via an HKDF built out of SHA-256.
//! * A SHA-256 keystream XORed over the plaintext.
//! * A 16-byte tag: the same derivation applied to
//!   `hex(key || nonce || ciphertext)`.
//!
//! **The key is derived from a public identifier**, so anyone who knows a
//! peer's sc-ID can derive it, decrypt their record, and forge one. This is
//! obfuscation against a passive DHT scraper — the C++ calls it "endpoint
//! privacy" and is right to — and it is NOT confidentiality or
//! authentication against anyone who has your id, which is everyone you
//! have ever exchanged a QR code with. Reproducing it here does not endorse
//! it; it is what interoperating requires, and the alternative is a fleet
//! that cannot find itself.
//!
//! Two consequences worth carrying: the tag proves the record was made by
//! someone who knew the sc-ID, which is not the same as the person, and
//! this is exactly why a resolved address is still `Reach::Unverified`
//! rather than trusted.
//!
//! ## Why every odd detail is preserved
//!
//! The nonce is 12 bytes, the counter is appended to the keystream seed as
//! four RAW LITTLE-ENDIAN bytes (the C++ memcpy's a `quint32`, so this is
//! an artifact of the machines it was written on rather than a choice), the
//! tag re-enters the key derivation through a LOWERCASE hex string, and the
//! version byte distinguishes single-address (0x01) from multi-address
//! (0x02) payloads that are otherwise identically wrapped. None of these is
//! defensible in isolation. All of them are load-bearing for interop, so
//! they are reproduced exactly and the tests pin each one.

use sha2::{Digest, Sha256};

/// Plaintext, version byte only. The C++ reads it and strips the byte.
pub const V_PLAINTEXT: u8 = 0x00;
/// Encrypted single-address record (the legacy shape).
pub const V_SINGLE: u8 = 0x01;
/// Encrypted multi-address record — the one `dht_announce::render`
/// produces.
pub const V_MULTI: u8 = 0x02;

pub const NONCE_BYTES: usize = 12;
pub const TAG_BYTES: usize = 16;

const BLOCK: usize = 64;

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

/// HMAC-SHA256, written out rather than pulled in.
///
/// The `hmac` crate would do, and adding a dependency to write eight lines
/// that must match a hand-rolled C++ construction exactly is the wrong
/// trade: the value here is being able to read this next to the original
/// and see that the padding and ordering agree.
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&sha256(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Vec::with_capacity(BLOCK + msg.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(msg);
    let inner = sha256(&inner);

    let mut outer = Vec::with_capacity(BLOCK + 32);
    outer.extend_from_slice(&opad);
    outer.extend_from_slice(&inner);
    sha256(&outer)
}

/// The C++ `deriveEndpointKey`, exactly.
///
/// HKDF extract with a fixed salt, then one expand block. The info string
/// carries its own trailing `0x01` because the C++ writes the counter into
/// the literal rather than appending it — same bytes, and worth noting so
/// nobody "corrects" it into a standard HKDF call that produces a different
/// key and breaks every peer.
pub fn derive_key(sc_id: &str) -> [u8; 32] {
    let prk = hmac_sha256(b"snapclone-v1-endpoint-key", sc_id.as_bytes());
    hmac_sha256(&prk, b"endpoint\x01")
}

/// The keystream seed's counter blocks: `SHA256(key || nonce || ctr_le)`.
fn keystream(key: &[u8; 32], nonce: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + 32);
    let mut ctr: u32 = 0;
    while out.len() < len {
        let mut blk = Vec::with_capacity(32 + nonce.len() + 4);
        blk.extend_from_slice(key);
        blk.extend_from_slice(nonce);
        // Four raw little-endian bytes: the C++ memcpy's a `quint32`, so
        // this is the byte order of the machines it ran on. Reproduced
        // rather than normalised, because a big-endian counter would
        // produce a different keystream and read the same in the source.
        blk.extend_from_slice(&ctr.to_le_bytes());
        out.extend_from_slice(&sha256(&blk));
        ctr = ctr.wrapping_add(1);
    }
    out.truncate(len);
    out
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// The 16-byte tag: `derive_key(hex(key || nonce || ciphertext))[..16]`.
///
/// Using a key-derivation function as a MAC is not what either is for, and
/// the C++ comment admits the helper is "(mis)used" this way. It is
/// reproduced because the tag has to match; it is not a pattern to copy.
fn tag_for(key: &[u8; 32], nonce: &[u8], ct: &[u8]) -> [u8; TAG_BYTES] {
    let mut material = Vec::with_capacity(32 + nonce.len() + ct.len());
    material.extend_from_slice(key);
    material.extend_from_slice(nonce);
    material.extend_from_slice(ct);
    let full = derive_key(&hex_lower(&material));
    let mut tag = [0u8; TAG_BYTES];
    tag.copy_from_slice(&full[..TAG_BYTES]);
    tag
}

/// Wrap a payload: `[version][12 nonce][ciphertext][16 tag]`.
///
/// The nonce is the caller's, not generated here, so the round trip is
/// testable and so a caller cannot accidentally get a fixed one from a
/// default. See [`random_nonce`].
pub fn seal_with_nonce(sc_id: &str, version: u8, nonce: [u8; NONCE_BYTES],
                       plaintext: &[u8]) -> Vec<u8> {
    let key = derive_key(sc_id);
    let ks = keystream(&key, &nonce, plaintext.len());
    let ct: Vec<u8> = plaintext.iter().zip(ks.iter()).map(|(p, k)| p ^ k).collect();
    let tag = tag_for(&key, &nonce, &ct);

    let mut out = Vec::with_capacity(1 + NONCE_BYTES + ct.len() + TAG_BYTES);
    out.push(version);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out.extend_from_slice(&tag);
    out
}

/// A nonce from the OS.
///
/// `/dev/urandom` directly, matching what `identity` already does in this
/// crate rather than introducing a second source of randomness. A failure
/// returns `None` instead of falling back to something deterministic: a
/// predictable nonce with a key anyone can derive turns a weak scheme into
/// no scheme, and not publishing is better than publishing that.
pub fn random_nonce() -> Option<[u8; NONCE_BYTES]> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom").ok()?;
    let mut n = [0u8; NONCE_BYTES];
    f.read_exact(&mut n).ok()?;
    Some(n)
}

/// Why a record could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// Not one of the three known version bytes.
    UnknownVersion(u8),
    /// Shorter than a version, a nonce and a tag.
    TooShort,
    /// The tag did not match. Either tampering, or the record belongs to a
    /// different sc-ID than the one we asked about.
    BadTag,
}

/// Unwrap a record. Returns the plaintext and which version carried it.
///
/// The tag is checked BEFORE decrypting. With a stream cipher, decrypting
/// first and validating after means acting on attacker-chosen plaintext,
/// and here "acting on" would mean parsing an address list and dialling it.
pub fn open(sc_id: &str, raw: &[u8]) -> Result<(u8, Vec<u8>), OpenError> {
    let version = *raw.first().ok_or(OpenError::TooShort)?;
    if version == V_PLAINTEXT {
        // Legacy: the version byte and nothing else.
        return Ok((version, raw[1..].to_vec()));
    }
    if version != V_SINGLE && version != V_MULTI {
        return Err(OpenError::UnknownVersion(version));
    }
    if raw.len() <= 1 + NONCE_BYTES + TAG_BYTES {
        return Err(OpenError::TooShort);
    }
    let nonce = &raw[1..1 + NONCE_BYTES];
    let ct = &raw[1 + NONCE_BYTES..raw.len() - TAG_BYTES];
    let tag = &raw[raw.len() - TAG_BYTES..];

    let key = derive_key(sc_id);
    if tag_for(&key, nonce, ct) != tag {
        return Err(OpenError::BadTag);
    }
    let ks = keystream(&key, nonce, ct.len());
    Ok((version, ct.iter().zip(ks.iter()).map(|(c, k)| c ^ k).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "sc-aabbccdd";

    #[test]
    fn a_record_round_trips_through_the_envelope() {
        let plain = b"snapclone\nsc-aabbccdd\n192.168.1.42:9876:9876\n#ts:1700000000";
        let sealed = seal_with_nonce(ID, V_MULTI, [7u8; NONCE_BYTES], plain);
        assert_eq!(sealed[0], V_MULTI);
        assert_eq!(sealed.len(), 1 + NONCE_BYTES + plain.len() + TAG_BYTES);
        let (v, got) = open(ID, &sealed).expect("did not open");
        assert_eq!(v, V_MULTI);
        assert_eq!(got, plain);
    }

    #[test]
    fn the_key_is_pinned_so_a_tidy_up_cannot_silently_break_every_peer() {
        // The info string carries its own trailing 0x01 because the C++
        // writes the counter into the literal. "Correcting" that into a
        // standard HKDF call produces a different key, breaks interop with
        // every Qt peer, and looks like an improvement in the diff. This
        // vector is what makes that change fail here instead of in the
        // field.
        let k = derive_key(ID);
        assert_eq!(hex_lower(&k[..8]), {
            // Recomputed from the two HMACs the C++ performs, spelled out
            // rather than pasted, so the test states the construction
            // instead of a magic string.
            let prk = hmac_sha256(b"snapclone-v1-endpoint-key", ID.as_bytes());
            hex_lower(&hmac_sha256(&prk, b"endpoint\x01")[..8])
        });
        // Different ids give different keys — the whole basis of the
        // scheme, weak as it is.
        assert_ne!(derive_key(ID), derive_key("sc-11223344"));
    }

    #[test]
    fn hmac_matches_the_published_test_vector() {
        // RFC 4231 case 1. The C++ hand-rolls the ipad/opad construction and
        // so does this; if either got the padding wrong the two would agree
        // with each other and with nothing else, which a round-trip test
        // cannot detect.
        let key = [0x0bu8; 20];
        let mac = hmac_sha256(&key, b"Hi There");
        assert_eq!(hex_lower(&mac),
                   "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
    }

    #[test]
    fn the_counter_is_little_endian_because_the_original_memcpys_it() {
        // A big-endian counter reads identically in the source and produces
        // a different keystream from the second block onward — so a payload
        // under 32 bytes would round trip fine and every real announcement
        // would be garbage. Two blocks' worth is the shortest thing that
        // catches it.
        let key = derive_key(ID);
        let ks = keystream(&key, &[0u8; NONCE_BYTES], 64);
        let mut expect = Vec::new();
        for ctr in [0u32, 1u32] {
            let mut blk = Vec::new();
            blk.extend_from_slice(&key);
            blk.extend_from_slice(&[0u8; NONCE_BYTES]);
            blk.extend_from_slice(&ctr.to_le_bytes());
            expect.extend_from_slice(&sha256(&blk));
        }
        assert_eq!(ks, expect);
        assert_ne!(ks[32..], expect[..32], "the second block repeated the first");
    }

    #[test]
    fn a_tampered_record_is_refused_before_it_is_decrypted() {
        // With a stream cipher, decrypt-then-validate means parsing
        // attacker-chosen plaintext — and here "parsing" ends in dialling
        // an address list.
        let sealed = seal_with_nonce(ID, V_MULTI, [1u8; NONCE_BYTES], b"snapclone\nu\n1.2.3.4:1:1");
        let mut bad = sealed.clone();
        let i = 1 + NONCE_BYTES + 2;
        bad[i] ^= 0xff;
        assert_eq!(open(ID, &bad), Err(OpenError::BadTag));

        // A truncated tag is not a shortcut past the check.
        let mut short = sealed.clone();
        short.truncate(short.len() - 1);
        assert_eq!(open(ID, &short), Err(OpenError::BadTag));
    }

    #[test]
    fn a_record_for_a_different_peer_does_not_open() {
        // The tag doubles as "is this even ours". Silently returning
        // garbage plaintext would put another peer's addresses under this
        // id.
        let sealed = seal_with_nonce(ID, V_MULTI, [3u8; NONCE_BYTES], b"payload");
        assert_eq!(open("sc-11223344", &sealed), Err(OpenError::BadTag));
    }

    #[test]
    fn the_legacy_plaintext_version_is_still_read() {
        // An old peer publishing unwrapped records is not an error — the
        // same argument wire_v3::negotiate makes about protocol versions.
        let mut raw = vec![V_PLAINTEXT];
        raw.extend_from_slice(b"sc-aabbccdd\n192.168.1.42\n9876");
        let (v, got) = open(ID, &raw).expect("legacy record refused");
        assert_eq!(v, V_PLAINTEXT);
        assert_eq!(got, b"sc-aabbccdd\n192.168.1.42\n9876");
    }

    #[test]
    fn an_unknown_version_is_named_rather_than_guessed_at() {
        assert_eq!(open(ID, &[0x07, 1, 2, 3]), Err(OpenError::UnknownVersion(0x07)));
        assert_eq!(open(ID, &[]), Err(OpenError::TooShort));
        // A frame with no room for a nonce and a tag.
        assert_eq!(open(ID, &[V_MULTI, 1, 2, 3]), Err(OpenError::TooShort));
    }

    #[test]
    fn both_encrypted_versions_are_wrapped_identically_and_both_open() {
        // 0x01 and 0x02 differ only in what the plaintext means. A reader
        // that unwrapped one and not the other would drop every
        // single-address record an older peer publishes.
        for v in [V_SINGLE, V_MULTI] {
            let sealed = seal_with_nonce(ID, v, [9u8; NONCE_BYTES], b"body");
            assert_eq!(open(ID, &sealed), Ok((v, b"body".to_vec())));
        }
    }

    #[test]
    fn one_record_carries_every_address_rather_than_one_each() {
        // The bug this arrangement replaces: publishing one VALUE per
        // address. OpenDHT keys a value set by value id, and the app
        // derives one id from the sc-ID alone, so three addresses were
        // three puts with the same id — two silent overwrites, and a
        // multi-address announcement that collapsed to whichever landed
        // last.
        use crate::dht_announce::{newest, parse, render, Addr, Announcement};
        let ann = Announcement {
            user_id: ID.into(),
            addrs: vec![Addr::new("192.168.1.42", 9876, 9876),
                        Addr::new("10.8.0.3", 9876, 9876),
                        Addr::new("203.0.113.9", 9876, 33445)],
            stamp: 1_700_000_000,
            ..Default::default()
        };
        let sealed = seal_with_nonce(ID, V_MULTI, [4u8; NONCE_BYTES],
                                     render(&ann).as_bytes());
        // Exactly what the resolve side does with what comes off the DHT.
        let (_, plain) = open(ID, &sealed).expect("did not open");
        let got = parse(&String::from_utf8(plain).unwrap()).expect("did not parse");
        assert_eq!(newest(&[got]).len(), 3,
                   "addresses were lost between the put and the get");
    }

    #[test]
    fn a_record_this_build_cannot_read_is_distinguishable_from_no_record() {
        // A format or key mismatch and an absent peer produce the same
        // silence otherwise, and they need very different responses — one
        // is somebody being offline, the other is the fleet split in two.
        let foreign = vec![0x02u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
                           0xde, 0xad, 0xbe, 0xef, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(open(ID, &foreign), Err(OpenError::BadTag));
    }

    #[test]
    fn the_whole_announcement_survives_being_wrapped_and_unwrapped() {
        // The two halves together: dht_announce's format inside this
        // envelope is the actual thing that goes into the DHT.
        use crate::dht_announce::{parse, render, Addr, Announcement};
        let a = Announcement {
            user_id: ID.into(),
            addrs: vec![Addr::new("192.168.1.42", 9876, 9876),
                        Addr::new("203.0.113.9", 9876, 33445)],
            stamp: 1_700_000_000,
            ..Default::default()
        };
        let sealed = seal_with_nonce(ID, V_MULTI, random_nonce().expect("urandom"),
                                     render(&a).as_bytes());
        let (v, plain) = open(ID, &sealed).expect("did not open");
        assert_eq!(v, V_MULTI);
        assert_eq!(parse(&String::from_utf8(plain).expect("not utf8")), Some(a));
    }
}
