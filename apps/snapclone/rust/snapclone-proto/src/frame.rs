//! v3 frame serialisation and deserialisation.
//!
//! Wire format (QDataStream big-endian, matching SnapRouter.cpp sendV3):
//!
//!   u8  flags        — see types::flags
//!   u32 seq          — monotonic sequence number per peer
//!   String snap_id   — UUID string (length-prefixed u32 + bytes)
//!   String hash      — hex SHA-256 of plaintext (or "" for ACK/SYNC)
//!   u32 payload_len
//!   [payload_len bytes] encrypted blob (may be empty for ANNOUNCE-only)

use crate::types::flags;
use anyhow::{bail, Result};
use bytes::{Buf, BufMut, Bytes, BytesMut};

/// A decoded v3 frame.
#[derive(Debug, Clone)]
pub struct Frame {
    pub flags:      u8,
    pub seq:        u32,
    pub snap_id:    String,
    pub hash:       String,
    pub payload:    Bytes,
}

impl Frame {
    pub fn is_announce(&self) -> bool { self.flags & flags::ANNOUNCE != 0 }
    pub fn is_data(&self)     -> bool { self.flags & flags::DATA != 0 }
    pub fn is_ack(&self)      -> bool { self.flags & flags::ACK != 0 }
    pub fn is_sync(&self)     -> bool { self.flags & flags::SYNC != 0 }
    pub fn is_need(&self)     -> bool { self.flags & flags::NEED != 0 }
}

/// Encode a frame into bytes ready for transmission.
pub fn build(flags: u8, seq: u32, snap_id: &str, hash: &str, payload: &[u8]) -> Bytes {
    let mut buf = BytesMut::with_capacity(1 + 4 + 4 + snap_id.len() + 4 + hash.len() + 4 + payload.len());
    buf.put_u8(flags);
    buf.put_u32(seq);
    put_qstring(&mut buf, snap_id);
    put_qstring(&mut buf, hash);
    buf.put_u32(payload.len() as u32);
    buf.put_slice(payload);
    buf.freeze()
}

/// Decode a frame from a byte slice.
pub fn parse(data: &[u8]) -> Result<Frame> {
    if data.len() < 9 { bail!("Frame too short: {} bytes", data.len()); }
    let mut r = data;

    let frame_flags = r.get_u8();
    let seq         = r.get_u32();
    let snap_id     = get_qstring(&mut r)?;
    let hash_str    = get_qstring(&mut r)?;

    if r.remaining() < 4 { bail!("Frame truncated before payload length"); }
    let payload_len = r.get_u32() as usize;

    if r.remaining() < payload_len {
        bail!("Payload truncated: need {} have {}", payload_len, r.remaining());
    }

    // Guard against malicious large payloads (10 MB max)
    if payload_len > 10 * 1024 * 1024 {
        bail!("Payload too large: {} bytes", payload_len);
    }

    let payload = Bytes::copy_from_slice(&r[..payload_len]);
    Ok(Frame { flags: frame_flags, seq, snap_id, hash: hash_str, payload })
}

// QDataStream string encoding: u32 length (0xFFFFFFFF = null), then UTF-8 bytes.
fn put_qstring(buf: &mut BytesMut, s: &str) {
    if s.is_empty() {
        buf.put_u32(0);
    } else {
        buf.put_u32(s.len() as u32);
        buf.put_slice(s.as_bytes());
    }
}

fn get_qstring(r: &mut &[u8]) -> Result<String> {
    if r.remaining() < 4 { bail!("Truncated string length"); }
    let len = r.get_u32() as usize;
    if len == 0xFFFF_FFFF || len == 0 { return Ok(String::new()); }
    if len > 65536 { bail!("String too long: {}", len); }
    if r.remaining() < len { bail!("String truncated"); }
    let s = std::str::from_utf8(&r[..len])
        .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in frame string: {}", e))?
        .to_owned();
    r.advance(len);
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let data = b"hello world";
        let encoded = build(flags::ANNOUNCE | flags::DATA, 42, "snap-123", "deadbeef", data);
        let frame = parse(&encoded).unwrap();
        assert_eq!(frame.flags, flags::ANNOUNCE | flags::DATA);
        assert_eq!(frame.seq, 42);
        assert_eq!(frame.snap_id, "snap-123");
        assert_eq!(frame.hash, "deadbeef");
        assert_eq!(&frame.payload[..], data);
    }

    #[test]
    fn rejects_large_payload() {
        let flags = flags::DATA;
        let mut buf = BytesMut::new();
        buf.put_u8(flags);
        buf.put_u32(1);
        buf.put_u32(0); buf.put_u32(0); // empty strings
        buf.put_u32(20 * 1024 * 1024); // 20 MB — too large
        assert!(parse(&buf.freeze()).is_err());
    }
}
