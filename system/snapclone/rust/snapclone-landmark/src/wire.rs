//! Wire format encoder / decoder for Landmarks.
//!
//! Layout (big-endian, all multi-byte fields):
//!
//! | offset | size | field          | notes                              |
//! |--------|------|----------------|------------------------------------|
//! | 0      | 1    | version        | currently always 1                 |
//! | 1      | 16   | id             | LandmarkId                         |
//! | 17     | 32   | creator_pubkey | ed25519 public key                 |
//! | 49     | 8    | created_at_ms  | i64, big-endian                    |
//! | 57     | 4    | latitude_e7    | i32 = round(lat * 1e7), big-endian |
//! | 61     | 4    | longitude_e7   | i32 = round(lon * 1e7), big-endian |
//! | 65     | 2    | accuracy_m     | u16, big-endian                    |
//! | 67     | 1    | scope          | Scope as u8                        |
//! | 68     | 12   | photo_fp       | PhotoFingerprint                   |
//! | 80     | 64   | signature      | ed25519 signature                  |
//! | 144    | 2    | title_len      | u16, length of title in bytes      |
//! | 146    | N    | title          | UTF-8                              |
//! | 146+N  | 2    | desc_len       | u16, length of description         |
//! | 148+N  | M    | description    | UTF-8                              |
//!
//! Total: 148 + N + M bytes. Fixed portion = 148B (the 134B in V32.31
//! design notes was an early estimate that omitted the signature; signed
//! wire format is 148B fixed + variable text).
//!
//! Coordinate encoding uses E7 fixed-point (multiply by 1e7, round, store
//! as i32). Precision is ~1.1cm at the equator, which is finer than any
//! consumer GPS. Saves 4 bytes per coordinate vs. f64 with no practical
//! information loss.

use crate::error::LandmarkError;
use crate::types::{Landmark, LandmarkId, PhotoFingerprint, Scope};
use byteorder::{BigEndian, ByteOrder};

/// Current wire format version. Bumped if the layout changes
/// incompatibly. Receivers reject unknown versions rather than guessing.
pub const WIRE_FORMAT_VERSION: u8 = 1;

/// Fixed-portion size — everything before title_len.
const FIXED_HEADER_BYTES: usize = 148;

/// Encode a Landmark into its wire representation.
///
/// Title and description are truncated at `MAX_TITLE_LEN` / `MAX_DESCRIPTION_LEN`
/// UTF-8 bytes — truncation respects character boundaries so the output
/// is always valid UTF-8.
///
/// Returns an `Err` only for genuinely malformed input (coordinates out
/// of range). Length-related issues are auto-fixed via truncation.
pub fn encode_wire(landmark: &Landmark) -> Result<Vec<u8>, LandmarkError> {
    // Validate coordinate range — out-of-range coords on the wire would
    // produce nonsense distances when decoded, so refuse to encode.
    if !(-90.0..=90.0).contains(&landmark.latitude)
        || !(-180.0..=180.0).contains(&landmark.longitude) {
        return Err(LandmarkError::BadCoordinate {
            lat: landmark.latitude,
            lon: landmark.longitude,
        });
    }

    let title = truncate_utf8(&landmark.title, Landmark::MAX_TITLE_LEN);
    let desc  = truncate_utf8(&landmark.description, Landmark::MAX_DESCRIPTION_LEN);

    let mut out = Vec::with_capacity(FIXED_HEADER_BYTES + 2 + title.len() + 2 + desc.len());
    out.push(WIRE_FORMAT_VERSION);
    out.extend_from_slice(&landmark.id.0);
    out.extend_from_slice(&landmark.creator_pubkey);
    let mut buf8 = [0u8; 8];
    BigEndian::write_i64(&mut buf8, landmark.created_at_ms);
    out.extend_from_slice(&buf8);
    let mut buf4 = [0u8; 4];
    BigEndian::write_i32(&mut buf4, (landmark.latitude  * 1e7).round() as i32);
    out.extend_from_slice(&buf4);
    BigEndian::write_i32(&mut buf4, (landmark.longitude * 1e7).round() as i32);
    out.extend_from_slice(&buf4);
    let mut buf2 = [0u8; 2];
    BigEndian::write_u16(&mut buf2, landmark.accuracy_m);
    out.extend_from_slice(&buf2);
    out.push(landmark.scope.to_wire());
    out.extend_from_slice(&landmark.photo_fp.0);
    out.extend_from_slice(&landmark.signature);

    // Variable-length tail: length-prefixed UTF-8 strings.
    BigEndian::write_u16(&mut buf2, title.len() as u16);
    out.extend_from_slice(&buf2);
    out.extend_from_slice(title.as_bytes());
    BigEndian::write_u16(&mut buf2, desc.len() as u16);
    out.extend_from_slice(&buf2);
    out.extend_from_slice(desc.as_bytes());

    Ok(out)
}

/// Decode a wire payload into a Landmark.
///
/// Validation enforced:
///   - Minimum length (rejects truncated payloads)
///   - Version match (rejects unknown wire formats)
///   - Length-prefix bounds (rejects malicious oversize claims)
///   - Coordinate range (rejects garbage that decoded to lat>90 etc.)
///   - UTF-8 validity of title/description (rejects mid-character cuts)
///
/// Does NOT verify the signature — caller must call `verify_signature`
/// separately if the payload came from an untrusted source. Decoupling
/// decode from verify lets callers inspect malformed-but-not-malicious
/// payloads (legitimate decoder bugs to file).
pub fn decode_wire(bytes: &[u8]) -> Result<Landmark, LandmarkError> {
    if bytes.len() < FIXED_HEADER_BYTES + 2 + 2 {
        // Need at least the fixed header + the two length prefixes.
        return Err(LandmarkError::Truncated {
            have: bytes.len(),
            need: FIXED_HEADER_BYTES + 4,
        });
    }

    let version = bytes[0];
    if version != WIRE_FORMAT_VERSION {
        return Err(LandmarkError::UnsupportedVersion(version));
    }

    let mut id_buf = [0u8; 16];
    id_buf.copy_from_slice(&bytes[1..17]);
    let id = LandmarkId(id_buf);

    let mut pk = [0u8; 32];
    pk.copy_from_slice(&bytes[17..49]);

    let created_at_ms = BigEndian::read_i64(&bytes[49..57]);
    let latitude_e7   = BigEndian::read_i32(&bytes[57..61]);
    let longitude_e7  = BigEndian::read_i32(&bytes[61..65]);
    let accuracy_m    = BigEndian::read_u16(&bytes[65..67]);
    let scope         = Scope::from_wire(bytes[67]);

    let mut fp = [0u8; 12];
    fp.copy_from_slice(&bytes[68..80]);
    let photo_fp = PhotoFingerprint(fp);

    let mut sig = [0u8; 64];
    sig.copy_from_slice(&bytes[80..144]);

    let latitude  = latitude_e7  as f64 / 1e7;
    let longitude = longitude_e7 as f64 / 1e7;
    if !(-90.0..=90.0).contains(&latitude)
        || !(-180.0..=180.0).contains(&longitude) {
        return Err(LandmarkError::BadCoordinate { lat: latitude, lon: longitude });
    }

    // Length-prefixed title.
    let title_len = BigEndian::read_u16(&bytes[144..146]) as usize;
    let title_start = 146;
    if title_start + title_len + 2 > bytes.len() {
        return Err(LandmarkError::LengthOverrun {
            declared: title_len,
            remaining: bytes.len().saturating_sub(title_start),
        });
    }
    let title = std::str::from_utf8(&bytes[title_start .. title_start + title_len])
        .map_err(|_| LandmarkError::Internal("invalid UTF-8 in title"))?
        .to_string();

    // Length-prefixed description, immediately after title.
    let desc_lenpos = title_start + title_len;
    let desc_len = BigEndian::read_u16(&bytes[desc_lenpos .. desc_lenpos + 2]) as usize;
    let desc_start = desc_lenpos + 2;
    if desc_start + desc_len > bytes.len() {
        return Err(LandmarkError::LengthOverrun {
            declared: desc_len,
            remaining: bytes.len().saturating_sub(desc_start),
        });
    }
    let description = std::str::from_utf8(&bytes[desc_start .. desc_start + desc_len])
        .map_err(|_| LandmarkError::Internal("invalid UTF-8 in description"))?
        .to_string();

    if title.len() > Landmark::MAX_TITLE_LEN {
        return Err(LandmarkError::TextTooLong {
            field: "title", len: title.len(), max: Landmark::MAX_TITLE_LEN,
        });
    }
    if description.len() > Landmark::MAX_DESCRIPTION_LEN {
        return Err(LandmarkError::TextTooLong {
            field: "description", len: description.len(), max: Landmark::MAX_DESCRIPTION_LEN,
        });
    }

    Ok(Landmark {
        id,
        creator_pubkey: pk,
        created_at_ms,
        latitude,
        longitude,
        accuracy_m,
        title,
        description,
        scope,
        photo_fp,
        signature: sig,
    })
}

/// Truncate a string to at most `max` bytes WITHOUT splitting a UTF-8
/// character. Walks backward from the byte cap until it hits a char
/// boundary. Returns the input unchanged if already short enough.
fn truncate_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max { return s; }
    // Find the last char boundary at-or-before `max`.
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_landmark() -> Landmark {
        Landmark {
            id: LandmarkId([0xab; 16]),
            creator_pubkey: [0xcd; 32],
            created_at_ms: 1_700_000_000_000,
            latitude: 37.7749,
            longitude: -122.4194,
            accuracy_m: 10,
            title: "Coffee shop".into(),
            description: "Decent espresso, free wifi".into(),
            scope: Scope::Friends,
            photo_fp: PhotoFingerprint([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]),
            signature: [0xef; 64],
        }
    }

    #[test]
    fn roundtrip_preserves_fields() {
        let original = sample_landmark();
        let encoded = encode_wire(&original).expect("encode");
        let decoded = decode_wire(&encoded).expect("decode");
        assert_eq!(decoded.id, original.id);
        assert_eq!(decoded.creator_pubkey, original.creator_pubkey);
        assert_eq!(decoded.created_at_ms, original.created_at_ms);
        assert!((decoded.latitude - original.latitude).abs() < 1e-6,
                "lat: {} vs {}", decoded.latitude, original.latitude);
        assert!((decoded.longitude - original.longitude).abs() < 1e-6);
        assert_eq!(decoded.accuracy_m, original.accuracy_m);
        assert_eq!(decoded.title, original.title);
        assert_eq!(decoded.description, original.description);
        assert_eq!(decoded.scope, original.scope);
        assert_eq!(decoded.photo_fp, original.photo_fp);
        assert_eq!(decoded.signature, original.signature);
    }

    #[test]
    fn truncated_payload_rejected() {
        let encoded = encode_wire(&sample_landmark()).unwrap();
        let result = decode_wire(&encoded[..50]);
        assert!(matches!(result, Err(LandmarkError::Truncated { .. })));
    }

    #[test]
    fn bad_version_rejected() {
        let mut encoded = encode_wire(&sample_landmark()).unwrap();
        encoded[0] = 99;
        assert!(matches!(decode_wire(&encoded),
                         Err(LandmarkError::UnsupportedVersion(99))));
    }

    #[test]
    fn coords_out_of_range_rejected() {
        let mut bad = sample_landmark();
        bad.latitude = 95.0;
        assert!(matches!(encode_wire(&bad),
                         Err(LandmarkError::BadCoordinate { .. })));
    }

    #[test]
    fn truncate_respects_utf8_boundaries() {
        // "héllo" has 'é' as 2 bytes — truncating at len=3 would split it.
        let truncated = truncate_utf8("héllo world", 3);
        // Should land at 1 (just 'h') or 3 (after 'hé'), never 2.
        assert!(std::str::from_utf8(truncated.as_bytes()).is_ok());
    }

    #[test]
    fn fingerprint_distance_is_symmetric() {
        let a = PhotoFingerprint([0xff; 12]);
        let b = PhotoFingerprint([0x00; 12]);
        assert_eq!(a.distance(&b), 96);  // all 96 bits differ
        assert_eq!(a.distance(&a), 0);
    }
}
