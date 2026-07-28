//! presence — the app-facing bridge between sc-IDs and the DHT engine: announce our address
//! under our sc-ID's key, and resolve a peer's sc-ID to their address. This replaces the
//! "via OpenDHT FFI / JamiAdapter" TODOs in announce.rs / identity::resolver with the native
//! engine, keeping the same intent: presence is a signed(-able) Value stored at hash(sc-id).

use crate::core::{InfoHash, Value, HASH_LEN};

/// The DHT key a sc-ID publishes presence under: a stable hash of the id string. (On-device
/// this is the identity public-key hash; here we derive it deterministically from the text so
/// announcer and resolver agree without shared state.)
pub fn key_for_scid(sc_id: &str) -> InfoHash {
    // FNV-1a spread across 20 bytes — deterministic, uniform enough for bucket placement.
    let mut h = [0u8; HASH_LEN];
    let mut acc: u64 = 0xcbf29ce484222325;
    for (i, byte) in sc_id.bytes().enumerate() {
        acc ^= byte as u64; acc = acc.wrapping_mul(0x100000001b3);
        h[i % HASH_LEN] ^= (acc >> ((i % 8) * 8)) as u8;
    }
    // fold the accumulator through all bytes so short ids still fill the hash
    for i in 0..HASH_LEN { acc = acc.wrapping_mul(0x100000001b3) ^ (i as u64); h[i] ^= (acc >> 24) as u8; }
    InfoHash(h)
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
        Value { id: value_id, data }
    }
    pub fn from_value(v: &Value) -> Option<Presence> {
        let d = &v.data;
        let mut i = 0;
        let mut take = |i: &mut usize| -> Option<String> {
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
