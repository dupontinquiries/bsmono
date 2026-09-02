//! presence — the app-facing bridge between sc-IDs and the DHT engine: announce our address
//! under our sc-ID's key, and resolve a peer's sc-ID to their address. This replaces the
//! "via OpenDHT FFI / JamiAdapter" TODOs in announce.rs / identity::resolver with the native
//! engine, keeping the same intent: presence is a signed(-able) Value stored at hash(sc-id).

use dht_kit::{InfoHash, Value, HASH_LEN};

/// The DHT key a sc-ID publishes presence under: **SHA-1 of the id string**.
///
/// Not a free choice. OpenDHT's `InfoHash::get` runs `crypto::hash`, which
/// asks gnutls for a digest sized to the output — and for 20 bytes that is
/// SHA-1. The Qt build calls `dht_infohash_get_from_string(&key, scId)` and
/// gets exactly that. Anything else and the two halves of this app publish
/// to and look up different keys forever, each perfectly self-consistent.
///
/// This WAS anything else. It was FNV-1a smeared over 20 bytes, with a
/// comment explaining that it is deterministic "so announcer and resolver
/// agree without shared state" — true, and the wrong requirement: the
/// announcer and resolver that have to agree are on different machines
/// running different builds. Self-consistency was never the hard part.
///
/// SHA-1 is used here as an ADDRESS, not as a security property. Its
/// collision weakness does not matter for bucket placement, and the
/// alternative — a stronger hash nobody else computes — is not stronger,
/// it is unreachable.
pub fn key_for_scid(sc_id: &str) -> InfoHash {
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    h.update(sc_id.as_bytes());
    let out = h.finalize();
    let mut key = [0u8; HASH_LEN];
    // SHA-1 is 20 bytes and HASH_LEN is 20, so this is a copy. Written as a
    // bounded copy rather than a `try_into` so a future HASH_LEN change is a
    // shorter key rather than a panic in the middle of a lookup.
    let n = HASH_LEN.min(out.len());
    key[..n].copy_from_slice(&out[..n]);
    InfoHash(key)
}

/// A presence record: reflexive address + display name + punch port, serialized compactly into
/// a Value payload. Mirrors the announce() signature the Qt side calls.
#[derive(Clone, Debug, PartialEq)]
pub struct Presence {
    pub display_name: String,
    pub host: String,
    pub port: u16,
}

impl Presence {
    pub fn to_value(&self, value_id: u64) -> Value {
        let mut data = Vec::new();
        let put = |d: &mut Vec<u8>, s: &str| { d.push(s.len() as u8); d.extend_from_slice(s.as_bytes()); };
        put(&mut data, &self.display_name);
        put(&mut data, &self.host);
        data.extend_from_slice(&self.port.to_be_bytes());
        Value::new(value_id, data)
    }
    pub fn from_value(v: &Value) -> Option<Presence> {
        let d = &v.data;
        let mut i = 0;
        let take = |i: &mut usize| -> Option<String> {
            let n = *d.get(*i)? as usize; *i += 1;
            let s = d.get(*i..*i + n)?; *i += n;
            String::from_utf8(s.to_vec()).ok()
        };
        let display_name = take(&mut i)?;
        let host = take(&mut i)?;
        let port = u16::from_be_bytes(d.get(i..i + 2)?.try_into().ok()?);
        Some(Presence { display_name, host, port })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_key_is_the_one_opendht_computes() {
        // The number that has to match a Qt peer, a Jami bootstrap server
        // and anything else speaking this protocol. SHA-1("sc-abc12345"),
        // because `InfoHash::get` asks gnutls for a 20-byte digest and
        // gnutls answers SHA-1 for that length.
        //
        // Pinned as a literal rather than recomputed with the same code
        // that produced it: a test that calls Sha1 to check Sha1 passes
        // whatever the function does. The literal came from an INDEPENDENT
        // implementation — `python3 -c "hashlib.sha1(b\'sc-abc12345\')"` —
        // which is the only way it is worth anything.
        let k = super::key_for_scid("sc-abc12345");
        let hex: String = k.0.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "a41248613c0d47531a9c4c2314fdbc60b2ad0bc2");
    }

    #[test]
    fn sha1_itself_matches_the_published_vector() {
        // FIPS 180-1: SHA-1("abc"). If the digest is wrong, the key test
        // above is only self-consistent — which is precisely the failure
        // that made the old FNV key look fine.
        use sha1::{Digest, Sha1};
        let mut h = Sha1::new();
        h.update(b"abc");
        let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    use super::*;
    #[test]
    fn key_is_stable_and_id_specific() {
        assert_eq!(key_for_scid("sc-32a51661"), key_for_scid("sc-32a51661"));
        assert_ne!(key_for_scid("sc-32a51661"), key_for_scid("sc-00000000"));
    }
    #[test]
    fn presence_round_trips_through_a_value() {
        let p = Presence { display_name: "kesslerd".into(), host: "192.168.1.5".into(), port: 41000 };
        let v = p.to_value(1);
        assert_eq!(Presence::from_value(&v), Some(p));
    }
}

/// The value id a peer always uses for its own presence record.
///
/// Derived from the sc-ID rather than random, because a re-announce must
/// REPLACE the peer's previous record. With a random id every stale address
/// stays live until its TTL and the peer is listed at every place it has ever
/// been — which is worse than being listed nowhere, since a caller cannot tell
/// which entry is current.
pub fn value_id_for(sc_id: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in sc_id.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

#[cfg(test)]
mod value_id_tests {
    use super::*;

    #[test]
    fn a_peers_value_id_is_stable_across_announces() {
        assert_eq!(value_id_for("sc-abc12345"), value_id_for("sc-abc12345"));
    }

    #[test]
    fn different_peers_get_different_value_ids() {
        assert_ne!(value_id_for("sc-aaaaaaaa"), value_id_for("sc-bbbbbbbb"));
    }
}
