//! SCAP — SnapClone Announce Peer frame (magic 0x53434150).
//!
//! Sent as a UDP multicast/broadcast datagram when a peer comes online.
//! Any peer that receives it can attempt a WebSocket connection.
//!
//! Wire layout (big-endian):
//! ```text
//! [4]  magic      0x53434150 ("SCAP")
//! [2]  id_len     length of sc-ID in bytes
//! [N]  sc_id      UTF-8 sc-ID string
//! [2]  ws_port    WebSocket listen port
//! [8]  timestamp  milliseconds since Unix epoch
//! ```

use anyhow::{bail, Result};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use chrono::{DateTime, Utc};

pub const SCAP_MAGIC: u32 = 0x5343_4150; // "SCAP"
pub const SCAP_MIN_LEN: usize = 4 + 2 + 2 + 8; // no id

/// An SCAP announce frame.
#[derive(Debug, Clone, PartialEq)]
pub struct ScapFrame {
    pub sc_id:     String,
    pub ws_port:   u16,
    pub timestamp: DateTime<Utc>,
}

/// Encode an SCAP frame into bytes.
pub fn encode_scap(frame: &ScapFrame) -> Bytes {
    let id_bytes = frame.sc_id.as_bytes();
    let mut buf = BytesMut::with_capacity(4 + 2 + id_bytes.len() + 2 + 8);
    buf.put_u32(SCAP_MAGIC);
    buf.put_u16(id_bytes.len() as u16);
    buf.put_slice(id_bytes);
    buf.put_u16(frame.ws_port);
    buf.put_i64(frame.timestamp.timestamp_millis());
    buf.freeze()
}

/// Decode an SCAP frame from bytes.
pub fn decode_scap(data: &[u8]) -> Result<ScapFrame> {
    if data.len() < SCAP_MIN_LEN {
        bail!("SCAP frame too short: {} bytes", data.len());
    }
    let mut r = data;
    let magic = r.get_u32();
    if magic != SCAP_MAGIC {
        bail!("Not an SCAP frame (magic 0x{:08X})", magic);
    }
    let id_len = r.get_u16() as usize;
    if r.remaining() < id_len + 2 + 8 {
        bail!("SCAP frame truncated after id_len");
    }
    let sc_id = std::str::from_utf8(&r[..id_len])
        .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in sc-ID: {}", e))?
        .to_owned();
    r.advance(id_len);
    let ws_port  = r.get_u16();
    let ts_ms    = r.get_i64();
    let timestamp = DateTime::from_timestamp_millis(ts_ms)
        .unwrap_or_else(Utc::now);
    Ok(ScapFrame { sc_id, ws_port, timestamp })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let f = ScapFrame {
            sc_id:     "sc-deadbeef".to_owned(),
            ws_port:   9876,
            timestamp: Utc::now(),
        };
        let bytes  = encode_scap(&f);
        let parsed = decode_scap(&bytes).unwrap();
        assert_eq!(parsed.sc_id,   f.sc_id);
        assert_eq!(parsed.ws_port, f.ws_port);
    }
    #[test]
    fn rejects_bad_magic() {
        let mut bytes = encode_scap(&ScapFrame {
            sc_id: "sc-test".to_owned(), ws_port: 9876, timestamp: Utc::now()
        }).to_vec();
        bytes[0] = 0xFF; // corrupt magic
        assert!(decode_scap(&bytes).is_err());
    }
    #[test]
    fn rejects_short() {
        assert!(decode_scap(&[0u8; 5]).is_err());
    }
}
