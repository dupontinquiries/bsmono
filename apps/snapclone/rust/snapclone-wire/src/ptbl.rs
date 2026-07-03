//! PTBL — Peer Table frame (magic 0x5054424C).
//!
//! Sent over an established WebSocket session to share the local peer view
//! with a connected peer (gossip exchange).
//!
//! Wire layout (big-endian):
//! ```text
//! [4]  magic      0x5054424C ("PTBL")
//! [2]  count      number of peer entries
//! for each entry:
//!   [2]  id_len
//!   [N]  sc_id    UTF-8
//!   [2]  host_len
//!   [M]  host     UTF-8 IP string
//!   [2]  port
//!   [8]  last_seen_ms
//! ```

use anyhow::{bail, Result};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use chrono::{DateTime, Utc};

pub const PTBL_MAGIC: u32 = 0x5054_424C; // "PTBL"

#[derive(Debug, Clone, PartialEq)]
pub struct PeerEntry {
    pub sc_id:     String,
    pub host:      String,
    pub port:      u16,
    pub last_seen: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PtblFrame {
    pub peers: Vec<PeerEntry>,
}

pub fn encode_ptbl(frame: &PtblFrame) -> Bytes {
    // Pre-calculate size
    let body_size: usize = frame.peers.iter().map(|p| {
        2 + p.sc_id.len() + 2 + p.host.len() + 2 + 8
    }).sum();
    let mut buf = BytesMut::with_capacity(4 + 2 + body_size);
    buf.put_u32(PTBL_MAGIC);
    buf.put_u16(frame.peers.len().min(u16::MAX as usize) as u16);
    for peer in &frame.peers {
        let id   = peer.sc_id.as_bytes();
        let host = peer.host.as_bytes();
        buf.put_u16(id.len() as u16);
        buf.put_slice(id);
        buf.put_u16(host.len() as u16);
        buf.put_slice(host);
        buf.put_u16(peer.port);
        buf.put_i64(peer.last_seen.timestamp_millis());
    }
    buf.freeze()
}

pub fn decode_ptbl(data: &[u8]) -> Result<PtblFrame> {
    if data.len() < 6 { bail!("PTBL frame too short"); }
    let mut r = data;
    if r.get_u32() != PTBL_MAGIC { bail!("Not a PTBL frame"); }
    let count = r.get_u16() as usize;
    // Guard against malicious large count (max 256 peers per gossip message)
    if count > 256 { bail!("PTBL count too large: {}", count); }

    let mut peers = Vec::with_capacity(count);
    for _ in 0..count {
        if r.remaining() < 2 { bail!("PTBL truncated reading id_len"); }
        let id_len = r.get_u16() as usize;
        if r.remaining() < id_len + 2 { bail!("PTBL truncated reading sc_id"); }
        let sc_id = std::str::from_utf8(&r[..id_len])?.to_owned();
        r.advance(id_len);

        let host_len = r.get_u16() as usize;
        if r.remaining() < host_len + 2 + 8 { bail!("PTBL truncated reading host"); }
        let host = std::str::from_utf8(&r[..host_len])?.to_owned();
        r.advance(host_len);

        let port      = r.get_u16();
        let ts_ms     = r.get_i64();
        let last_seen = DateTime::from_timestamp_millis(ts_ms)
            .unwrap_or_else(Utc::now);
        peers.push(PeerEntry { sc_id, host, port, last_seen });
    }
    Ok(PtblFrame { peers })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_empty()    { let f = PtblFrame { peers: vec![] }; assert_eq!(decode_ptbl(&encode_ptbl(&f)).unwrap().peers.len(), 0); }
    #[test]
    fn roundtrip_two_peers() {
        let f = PtblFrame { peers: vec![
            PeerEntry { sc_id: "sc-aabbccdd".to_owned(), host: "192.168.1.5".to_owned(), port: 9876, last_seen: Utc::now() },
            PeerEntry { sc_id: "sc-11223344".to_owned(), host: "10.0.0.2".to_owned(),    port: 9876, last_seen: Utc::now() },
        ]};
        let dec = decode_ptbl(&encode_ptbl(&f)).unwrap();
        assert_eq!(dec.peers[0].sc_id, "sc-aabbccdd");
        assert_eq!(dec.peers[1].host,  "10.0.0.2");
    }
    #[test]
    fn rejects_oversized_count() {
        let mut data = encode_ptbl(&PtblFrame { peers: vec![] }).to_vec();
        data[4] = 0x01; data[5] = 0x00; // set count = 256 → limit OK
        data[5] = 0x01; data[4] = 0x01; // count = 257 → rejected
        assert!(decode_ptbl(&data).is_err());
    }
}
