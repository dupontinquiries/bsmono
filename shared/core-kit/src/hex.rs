//! hex.rs — lowercase hexadecimal encode / decode.
//!
//! The `format!("{:02x}")` loop reimplemented in netserve, share-kit's `keys`,
//! crypto-kit, kitfinder, and elsewhere, plus the inverse (`from_str_radix` /
//! nibble parsing). One tested implementation. std only.

const HEX: &[u8; 16] = b"0123456789abcdef";

/// Encode bytes as a lowercase hex string (`[0xde, 0xad]` → `"dead"`).
pub fn encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

/// Decode a hex string to bytes. Accepts upper- or lower-case. Returns `None` for
/// an odd length or any non-hex character.
pub fn decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 2);
    let mut i = 0;
    while i < b.len() {
        let hi = nibble(b[i])?;
        let lo = nibble(b[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Some(out)
}

fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_known_vectors() {
        assert_eq!(encode(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
        assert_eq!(encode(&[0x00, 0x0f, 0xff]), "000fff");
        assert_eq!(encode(&[]), "");
    }

    #[test]
    fn roundtrips() {
        let bytes: Vec<u8> = (0..=255u16).map(|x| x as u8).collect();
        assert_eq!(decode(&encode(&bytes)).unwrap(), bytes);
    }

    #[test]
    fn decode_accepts_uppercase() {
        assert_eq!(decode("DEADBEEF").unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(decode("De0F").unwrap(), vec![0xde, 0x0f]);
    }

    #[test]
    fn decode_rejects_odd_length_and_non_hex() {
        assert!(decode("abc").is_none(), "odd length");
        assert!(decode("xy").is_none(), "non-hex");
        assert!(decode("de ad").is_none(), "space is non-hex");
    }
}
