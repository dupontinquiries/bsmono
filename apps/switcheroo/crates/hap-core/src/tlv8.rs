//! TLV8 — HAP's type-length-value encoding (HAP spec §14.1).
//!
//! One byte of type, one byte of length, up to 255 bytes of value. A value
//! longer than 255 bytes is split into consecutive items of the SAME type,
//! each carrying at most 255 bytes; a decoder merges consecutive same-type
//! items back into one value. That fragmentation rule is the whole reason
//! this is not `Vec<(u8, Vec<u8>)>`: SRP's 384-byte public keys and the
//! 3072-bit salt+key replies in pair-setup M2 are always fragmented on the
//! wire, and a codec that forgets to merge them yields a 255-byte `B` that
//! is silently wrong rather than obviously broken.
//!
//! Separators: two items of the same type that must NOT merge (a list of
//! pairings, say) are separated by a zero-length item of type
//! [`TYPE_SEPARATOR`]. Encoding a list handles that; nothing in the
//! controller flows needs it today, but the decoder honours it so a reply
//! that uses it is not mis-merged.

/// HAP pairing TLV types (spec table 5-6).
pub const TYPE_METHOD: u8 = 0x00;
pub const TYPE_IDENTIFIER: u8 = 0x01;
pub const TYPE_SALT: u8 = 0x02;
pub const TYPE_PUBLIC_KEY: u8 = 0x03;
pub const TYPE_PROOF: u8 = 0x04;
pub const TYPE_ENCRYPTED_DATA: u8 = 0x05;
pub const TYPE_STATE: u8 = 0x06;
pub const TYPE_ERROR: u8 = 0x07;
pub const TYPE_RETRY_DELAY: u8 = 0x08;
pub const TYPE_CERTIFICATE: u8 = 0x09;
pub const TYPE_SIGNATURE: u8 = 0x0A;
pub const TYPE_PERMISSIONS: u8 = 0x0B;
pub const TYPE_FRAGMENT_DATA: u8 = 0x0C;
pub const TYPE_FRAGMENT_LAST: u8 = 0x0D;
pub const TYPE_FLAGS: u8 = 0x13;
pub const TYPE_SEPARATOR: u8 = 0xFF;

/// Pairing error codes carried in a `TYPE_ERROR` item (spec table 5-5).
pub mod error_code {
    pub const UNKNOWN: u8 = 0x01;
    pub const AUTHENTICATION: u8 = 0x02;
    pub const BACKOFF: u8 = 0x03;
    pub const MAX_PEERS: u8 = 0x04;
    pub const MAX_TRIES: u8 = 0x05;
    pub const UNAVAILABLE: u8 = 0x06;
    pub const BUSY: u8 = 0x07;

    pub fn describe(code: u8) -> &'static str {
        match code {
            UNKNOWN => "unknown error",
            AUTHENTICATION => "authentication failed (wrong setup code, or a stale pairing)",
            BACKOFF => "accessory asked us to back off",
            MAX_PEERS => "accessory already has the maximum number of pairings",
            MAX_TRIES => "too many failed attempts; the accessory is locked until it is power-cycled",
            UNAVAILABLE => "accessory unavailable for pairing",
            BUSY => "accessory busy with another pairing",
            _ => "unrecognised error code",
        }
    }
}

const MAX_FRAGMENT: usize = 255;

/// An ordered list of (type, value). Values may exceed 255 bytes; encoding
/// fragments them and decoding reassembles them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tlv {
    items: Vec<(u8, Vec<u8>)>,
}

impl Tlv {
    pub fn new() -> Self { Self::default() }

    /// Append an item. Builder-style so a message reads top to bottom.
    pub fn with(mut self, t: u8, v: impl Into<Vec<u8>>) -> Self {
        self.items.push((t, v.into()));
        self
    }

    pub fn push(&mut self, t: u8, v: impl Into<Vec<u8>>) {
        self.items.push((t, v.into()));
    }

    /// First value of type `t`, if any. Pairing messages carry each type at
    /// most once, so "first" is "the" one.
    pub fn get(&self, t: u8) -> Option<&[u8]> {
        self.items.iter().find(|(k, _)| *k == t).map(|(_, v)| v.as_slice())
    }

    /// Single-byte value of type `t` (state, method, error).
    pub fn get_u8(&self, t: u8) -> Option<u8> {
        match self.get(t) {
            Some([b]) => Some(*b),
            _ => None,
        }
    }

    pub fn items(&self) -> &[(u8, Vec<u8>)] { &self.items }

    pub fn is_empty(&self) -> bool { self.items.is_empty() }

    /// Wire bytes. Every value longer than 255 bytes becomes a run of
    /// same-type items.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (t, v) in &self.items {
            if v.is_empty() {
                out.push(*t);
                out.push(0);
                continue;
            }
            for chunk in v.chunks(MAX_FRAGMENT) {
                out.push(*t);
                out.push(chunk.len() as u8);
                out.extend_from_slice(chunk);
            }
        }
        out
    }

    /// Parse wire bytes, merging consecutive same-type items. A truncated
    /// trailing item is an error rather than a silently short value.
    pub fn decode(mut bytes: &[u8]) -> Result<Self, TlvError> {
        let mut items: Vec<(u8, Vec<u8>)> = Vec::new();
        while !bytes.is_empty() {
            if bytes.len() < 2 {
                return Err(TlvError::Truncated);
            }
            let t = bytes[0];
            let len = bytes[1] as usize;
            if bytes.len() < 2 + len {
                return Err(TlvError::Truncated);
            }
            let v = &bytes[2..2 + len];
            bytes = &bytes[2 + len..];
            match items.last_mut() {
                // Continuation of a fragmented value: same type, previous
                // fragment was full-length, and not a separator.
                Some((pt, pv)) if *pt == t && t != TYPE_SEPARATOR && pv.len() % MAX_FRAGMENT == 0 && !pv.is_empty() => {
                    pv.extend_from_slice(v);
                }
                _ => items.push((t, v.to_vec())),
            }
        }
        Ok(Tlv { items })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlvError {
    /// The byte stream ended inside an item header or value.
    Truncated,
}

impl std::fmt::Display for TlvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TlvError::Truncated => write!(f, "TLV8 stream truncated"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_values_round_trip_in_order() {
        let t = Tlv::new().with(TYPE_STATE, [1u8]).with(TYPE_METHOD, [0u8]).with(TYPE_FLAGS, [0u8, 0, 0, 0]);
        let bytes = t.encode();
        assert_eq!(bytes, vec![0x06, 1, 1, 0x00, 1, 0, 0x13, 4, 0, 0, 0, 0]);
        assert_eq!(Tlv::decode(&bytes).unwrap(), t);
        assert_eq!(Tlv::decode(&bytes).unwrap().get_u8(TYPE_STATE), Some(1));
    }

    #[test]
    fn a_384_byte_srp_key_fragments_into_255_plus_129_and_merges_back() {
        let key: Vec<u8> = (0..384u32).map(|i| (i * 7 % 251) as u8).collect();
        let t = Tlv::new().with(TYPE_STATE, [2u8]).with(TYPE_PUBLIC_KEY, key.clone()).with(TYPE_SALT, [9u8; 16]);
        let bytes = t.encode();
        // header(2)+1  +  header(2)+255 + header(2)+129  +  header(2)+16
        assert_eq!(bytes.len(), 3 + 257 + 131 + 18);
        assert_eq!(bytes[3], TYPE_PUBLIC_KEY);
        assert_eq!(bytes[4], 255);
        assert_eq!(bytes[3 + 257], TYPE_PUBLIC_KEY);
        assert_eq!(bytes[3 + 257 + 1], 129);
        let back = Tlv::decode(&bytes).unwrap();
        assert_eq!(back.get(TYPE_PUBLIC_KEY).unwrap(), key.as_slice());
        assert_eq!(back.get(TYPE_SALT).unwrap().len(), 16);
    }

    #[test]
    fn an_exact_255_byte_value_followed_by_a_different_type_does_not_swallow_it() {
        let t = Tlv::new().with(TYPE_PROOF, vec![1u8; 255]).with(TYPE_STATE, [3u8]);
        let back = Tlv::decode(&t.encode()).unwrap();
        assert_eq!(back.get(TYPE_PROOF).unwrap().len(), 255);
        assert_eq!(back.get_u8(TYPE_STATE), Some(3));
    }

    #[test]
    fn a_separator_keeps_two_same_type_items_apart() {
        let bytes = [0x01, 1, b'a', 0xFF, 0, 0x01, 1, b'b'];
        let back = Tlv::decode(&bytes).unwrap();
        assert_eq!(back.items().len(), 3);
        assert_eq!(back.items()[0], (0x01, b"a".to_vec()));
        assert_eq!(back.items()[2], (0x01, b"b".to_vec()));
    }

    #[test]
    fn truncation_is_an_error_not_a_short_value() {
        assert_eq!(Tlv::decode(&[0x03, 10, 1, 2, 3]), Err(TlvError::Truncated));
        assert_eq!(Tlv::decode(&[0x03]), Err(TlvError::Truncated));
        assert!(Tlv::decode(&[]).unwrap().is_empty());
    }
}
