//! snapclone-wirecodec — content-aware compression for snap payloads.
//!
//! For hard-to-service areas, every byte that doesn't go on the wire is a byte
//! that can't be lost on a flaky link. This layer compresses snap payloads, but
//! *intelligently*: it only spends bytes (and CPU) when compression actually
//! helps, and it **never expands** the payload.
//!
//! ## The framing
//! Every encoded payload is prefixed with ONE codec tag byte:
//! ```text
//!   [tag:u8][body...]
//!   tag 0x00 = Stored   — body is the original bytes verbatim (no compression)
//!   tag 0x01 = Deflate  — body is raw-deflate compressed
//! ```
//! The receiver reads the tag and either returns the body as-is or inflates it.
//! Because the tag is one byte, the worst case (incompressible data) costs
//! exactly +1 byte — and we only pay even that when the caller opts in.
//!
//! ## The policy (why "content-aware")
//! Already-compressed media (AV1 video, JPEG images) does NOT shrink under
//! deflate — re-compressing wastes CPU and can add overhead. So
//! [`should_compress`] skips those by content class, and even for compressible
//! classes [`encode`] *measures*: if the deflate result isn't smaller than the
//! original, it stores verbatim instead. The decision is:
//!
//! - Already-compressed media (`Image`, `Video`) → Stored (tag only, no deflate
//!   attempt — saves CPU on the big payloads).
//! - Text / Link / Location / Unknown → try Deflate, keep it only if smaller.
//! - Tiny payloads (< [`MIN_COMPRESS_BYTES`]) → Stored (deflate overhead isn't
//!   worth it; matches the DHT-mailbox reasoning that <~64B shouldn't deflate).
//!
//! This is deliberately a separate, transport-agnostic layer: SnapRouter wraps
//! the serialized envelope's MEDIA bytes through `encode`/`decode` and the
//! transport never knows. Lossless and reversible: decode(encode(x)) == x for
//! all inputs.

use miniz_oxide::deflate::compress_to_vec;
use miniz_oxide::inflate::decompress_to_vec;

/// Codec tag — the first byte of every encoded payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Codec {
    /// Body is the original bytes, verbatim.
    Stored = 0x00,
    /// Body is raw-deflate compressed.
    Deflate = 0x01,
}

impl Codec {
    pub fn from_tag(b: u8) -> Option<Codec> {
        match b {
            0x00 => Some(Codec::Stored),
            0x01 => Some(Codec::Deflate),
            _ => None,
        }
    }
    pub fn tag(self) -> u8 { self as u8 }
}

/// Content class of the payload, so the policy can skip already-compressed media.
/// Mirrors `Core::MediaType` plus an `Unknown` for transport-level callers that
/// don't have the snap's type handy (treated as compressible).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    /// JPEG/PNG/etc — already compressed, don't bother.
    Image,
    /// AV1/VP8/etc — already compressed, don't bother.
    Video,
    /// UTF-8 text / captions — highly compressible.
    Text,
    /// URL — compressible.
    Link,
    /// Serialized lat/lon/acc — small, compressible.
    Location,
    /// Caller doesn't know — treat as compressible (try and measure).
    Unknown,
}

impl Content {
    /// Map a `Core::MediaType` discriminant (0=Image,1=Video,2=Text,3=Link,
    /// 4=Location) to a Content. Out-of-range → Unknown.
    pub fn from_media_type(mt: i32) -> Content {
        match mt {
            0 => Content::Image,
            1 => Content::Video,
            2 => Content::Text,
            3 => Content::Link,
            4 => Content::Location,
            _ => Content::Unknown,
        }
    }
}

/// Compression level passed to deflate (0–10 in miniz_oxide; 6 is a good
/// size/speed balance for mobile CPUs on a flaky link where size dominates).
pub const DEFLATE_LEVEL: u8 = 6;

/// Don't attempt compression below this size: the deflate stream framing plus
/// our tag isn't worth it, and tiny payloads are usually metadata. (Matches the
/// existing DHT-mailbox reasoning that sub-~64-byte payloads shouldn't deflate.)
pub const MIN_COMPRESS_BYTES: usize = 64;

/// Whether the policy will even *try* to compress this content class. Already-
/// compressed media is skipped outright (saves CPU on the biggest payloads).
pub fn should_compress(content: Content, len: usize) -> bool {
    if len < MIN_COMPRESS_BYTES {
        return false;
    }
    match content {
        Content::Image | Content::Video => false, // already compressed
        Content::Text | Content::Link | Content::Location | Content::Unknown => true,
    }
}

/// Encode a payload for the wire: returns `[tag][body]`. Compresses only when
/// the policy says to AND the result is strictly smaller; otherwise stores
/// verbatim. **Never expands** beyond +1 tag byte.
pub fn encode(content: Content, data: &[u8]) -> Vec<u8> {
    if should_compress(content, data.len()) {
        let compressed = compress_to_vec(data, DEFLATE_LEVEL);
        // Keep compression only if it actually saved space (after accounting
        // for the fact that both forms carry the same 1-byte tag).
        if compressed.len() < data.len() {
            let mut out = Vec::with_capacity(compressed.len() + 1);
            out.push(Codec::Deflate.tag());
            out.extend_from_slice(&compressed);
            return out;
        }
    }
    // Stored: tag + verbatim.
    let mut out = Vec::with_capacity(data.len() + 1);
    out.push(Codec::Stored.tag());
    out.extend_from_slice(data);
    out
}

/// Errors decoding a wire payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Empty input — not even a tag byte.
    Empty,
    /// First byte isn't a known codec tag.
    UnknownCodec(u8),
    /// Deflate body was corrupt / truncated.
    InflateFailed,
}

/// Decode a wire payload back to the original bytes. Inverse of [`encode`].
pub fn decode(wire: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let (&tag, body) = wire.split_first().ok_or(DecodeError::Empty)?;
    match Codec::from_tag(tag) {
        Some(Codec::Stored) => Ok(body.to_vec()),
        Some(Codec::Deflate) => decompress_to_vec(body)
            .map_err(|_| DecodeError::InflateFailed),
        None => Err(DecodeError::UnknownCodec(tag)),
    }
}

/// Compression ratio achieved, as encoded_len / original_len (for diagnostics /
/// logging). 1.0 means no saving; < 1.0 means smaller. Includes the tag byte.
pub fn ratio(original_len: usize, encoded_len: usize) -> f32 {
    if original_len == 0 { return 1.0; }
    encoded_len as f32 / original_len as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(content: Content, data: &[u8]) {
        let wire = encode(content, data);
        let back = decode(&wire).expect("decode");
        assert_eq!(back, data, "decode(encode(x)) must equal x");
    }

    #[test]
    fn roundtrip_all_content_kinds() {
        let text = b"the quick brown fox jumps over the lazy dog ".repeat(10);
        roundtrip(Content::Text, &text);
        roundtrip(Content::Link, b"https://example.com/a/very/long/path?with=querystring&and=more");
        roundtrip(Content::Location, &[0u8; 128]);
        roundtrip(Content::Unknown, &text);
        roundtrip(Content::Image, &text);   // stored, still round-trips
        roundtrip(Content::Video, &text);
    }

    #[test]
    fn empty_and_tiny_roundtrip() {
        roundtrip(Content::Text, b"");
        roundtrip(Content::Text, b"hi");
        roundtrip(Content::Unknown, b"under-min-bytes");
    }

    #[test]
    fn compressible_text_actually_shrinks() {
        // Repetitive text → deflate should win and the tag should be Deflate.
        let text = b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\
                     BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB".to_vec();
        let wire = encode(Content::Text, &text);
        assert_eq!(Codec::from_tag(wire[0]), Some(Codec::Deflate));
        assert!(wire.len() < text.len() + 1, "should be smaller than stored");
    }

    #[test]
    fn already_compressed_media_is_stored_not_deflated() {
        // Pretend this is AV1/JPEG bytes (high-entropy). Policy must NOT deflate
        // Image/Video even though it's over the min size.
        let pseudo_media: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
        let wire = encode(Content::Video, &pseudo_media);
        assert_eq!(Codec::from_tag(wire[0]), Some(Codec::Stored),
                   "media must be stored, not deflated");
        assert_eq!(wire.len(), pseudo_media.len() + 1, "stored = tag + verbatim");
    }

    #[test]
    fn incompressible_text_falls_back_to_stored() {
        // Genuinely high-entropy bytes tagged as Text: deflate can't help, so
        // even though the policy TRIES, encode must fall back to Stored (never
        // expand). Use a xorshift PRNG so the bytes have no exploitable pattern.
        let mut state: u64 = 0x9E3779B97F4A7C15;
        let random: Vec<u8> = (0..4096).map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        }).collect();
        let wire = encode(Content::Text, &random);
        assert_eq!(Codec::from_tag(wire[0]), Some(Codec::Stored),
                   "high-entropy data must fall back to Stored");
        // Never expands beyond the 1-byte tag.
        assert_eq!(wire.len(), random.len() + 1);
        assert_eq!(decode(&wire).unwrap(), random);
    }

    #[test]
    fn never_expands_beyond_one_byte() {
        // For ANY input, encoded length <= original + 1.
        for content in [Content::Text, Content::Image, Content::Unknown] {
            for n in [0usize, 1, 63, 64, 100, 1000, 5000] {
                let data: Vec<u8> = (0..n as u32).map(|i| (i.wrapping_mul(40503) >> 8) as u8).collect();
                let wire = encode(content, &data);
                assert!(wire.len() <= data.len() + 1,
                        "content {:?} n={} expanded: {} > {}+1",
                        content, n, wire.len(), data.len());
            }
        }
    }

    #[test]
    fn should_compress_policy() {
        assert!(!should_compress(Content::Image, 10_000));   // already compressed
        assert!(!should_compress(Content::Video, 10_000));
        assert!(should_compress(Content::Text, 10_000));
        assert!(should_compress(Content::Unknown, 10_000));
        assert!(!should_compress(Content::Text, 10));        // under min
    }

    #[test]
    fn decode_errors() {
        assert_eq!(decode(&[]), Err(DecodeError::Empty));
        assert_eq!(decode(&[0x7F, 1, 2, 3]), Err(DecodeError::UnknownCodec(0x7F)));
        // a Deflate tag with garbage body → InflateFailed
        assert_eq!(decode(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF]), Err(DecodeError::InflateFailed));
    }

    #[test]
    fn content_from_media_type_maps_correctly() {
        assert_eq!(Content::from_media_type(0), Content::Image);
        assert_eq!(Content::from_media_type(1), Content::Video);
        assert_eq!(Content::from_media_type(2), Content::Text);
        assert_eq!(Content::from_media_type(3), Content::Link);
        assert_eq!(Content::from_media_type(4), Content::Location);
        assert_eq!(Content::from_media_type(99), Content::Unknown);
    }

    #[test]
    fn ratio_reports_savings() {
        assert_eq!(ratio(0, 1), 1.0);
        assert_eq!(ratio(100, 50), 0.5);
        assert!(ratio(1000, 1001) > 1.0); // expansion shows as > 1
    }

    #[test]
    fn realistic_chat_caption_compresses_well() {
        // A realistic group-chat text snap with repeated structure.
        let caption = "Hey everyone! Meeting moved to 3pm. Hey everyone! \
                       Please confirm. Hey everyone! Thanks. ".repeat(5);
        let bytes = caption.as_bytes();
        let wire = encode(Content::Text, bytes);
        let r = ratio(bytes.len(), wire.len());
        assert!(r < 0.6, "chat caption should compress to <60%, got {}", r);
        assert_eq!(decode(&wire).unwrap(), bytes);
    }
}
pub mod ffi;
