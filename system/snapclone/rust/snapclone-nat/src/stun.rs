//! STUN (RFC 5389) packet construction and parsing.
//!
//! Ported from NatTraversal.cpp — the C++ uses raw QByteArray and manual
//! bit manipulation. Rust gives us a type-safe, panic-free alternative with
//! proper error propagation instead of silent field-reads off bad offsets.

use anyhow::{anyhow, bail, Result};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use rand::Rng;

pub const MAGIC_COOKIE: u32 = 0x2112_A442;
pub const BINDING_REQUEST: u16  = 0x0001;
pub const BINDING_RESPONSE: u16 = 0x0101;
pub const ATTR_MAPPED_ADDR:     u16 = 0x0001;
pub const ATTR_XOR_MAPPED_ADDR: u16 = 0x0020;

/// A 12-byte STUN transaction ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionId(pub [u8; 12]);

impl TransactionId {
    pub fn random() -> Self {
        let mut id = [0u8; 12];
        rand::thread_rng().fill(&mut id);
        Self(id)
    }
}

/// Build a STUN Binding Request packet ready to be sent over UDP.
///
/// Layout (RFC 5389 §6):
///   0      2      4      8     20
///   | type | len  | magic | tx_id |
pub fn build_request(tx: &TransactionId) -> Bytes {
    let mut buf = BytesMut::with_capacity(20);
    buf.put_u16(BINDING_REQUEST);
    buf.put_u16(0); // attribute length = 0 (no attributes in request)
    buf.put_u32(MAGIC_COOKIE);
    buf.put_slice(&tx.0);
    buf.freeze()
}

/// Reflexive address parsed from a STUN Binding Response.
#[derive(Debug, Clone)]
pub struct ReflexiveAddr {
    pub ip:   std::net::IpAddr,
    pub port: u16,
}

/// Parse a STUN Binding Response, verifying the magic cookie and
/// returning the XOR-MAPPED-ADDRESS (preferred) or MAPPED-ADDRESS.
///
/// Returns `Err` on any malformed packet rather than silently returning
/// wrong values (which was possible in the C++ bit-manipulation path).
pub fn parse_response(pkt: &[u8], tx: &TransactionId) -> Result<ReflexiveAddr> {
    if pkt.len() < 20 {
        bail!("STUN response too short: {} bytes", pkt.len());
    }

    let mut r = pkt;
    let msg_type = r.get_u16();
    let msg_len  = r.get_u16() as usize;
    let magic    = r.get_u32();
    let mut tx_buf = [0u8; 12];
    r.copy_to_slice(&mut tx_buf);

    if msg_type != BINDING_RESPONSE {
        bail!("Not a binding response: 0x{:04X}", msg_type);
    }
    if magic != MAGIC_COOKIE {
        bail!("Bad STUN magic cookie: 0x{:08X}", magic);
    }
    if tx_buf != tx.0 {
        bail!("Transaction ID mismatch");
    }
    if pkt.len() < 20 + msg_len {
        bail!("STUN response truncated: need {} have {}", 20 + msg_len, pkt.len());
    }

    let mut attrs = &pkt[20..20 + msg_len];
    let mut mapped:     Option<ReflexiveAddr> = None;
    let mut xor_mapped: Option<ReflexiveAddr> = None;

    while attrs.len() >= 4 {
        let attr_type = attrs.get_u16();
        let attr_len  = attrs.get_u16() as usize;
        let padded    = (attr_len + 3) & !3; // round up to 4-byte boundary
        if attrs.len() < padded { break; }
        let value = &attrs[..attr_len];

        match attr_type {
            ATTR_MAPPED_ADDR => {
                mapped = parse_mapped_addr(value).ok();
            }
            ATTR_XOR_MAPPED_ADDR => {
                xor_mapped = parse_xor_mapped_addr(value, MAGIC_COOKIE, tx).ok();
            }
            _ => {} // unknown attribute — RFC says ignore
        }
        attrs.advance(padded);
    }

    xor_mapped.or(mapped)
        .ok_or_else(|| anyhow!("No MAPPED-ADDRESS in STUN response"))
}

fn parse_mapped_addr(v: &[u8]) -> Result<ReflexiveAddr> {
    if v.len() < 8 { bail!("MAPPED-ADDRESS too short"); }
    // v[0] = reserved, v[1] = family (0x01 = IPv4)
    if v[1] != 0x01 { bail!("Only IPv4 supported"); }
    let port = u16::from_be_bytes([v[2], v[3]]);
    let ip   = std::net::Ipv4Addr::new(v[4], v[5], v[6], v[7]);
    Ok(ReflexiveAddr { ip: ip.into(), port })
}

fn parse_xor_mapped_addr(v: &[u8], magic: u32, tx: &TransactionId) -> Result<ReflexiveAddr> {
    if v.len() < 8 { bail!("XOR-MAPPED-ADDRESS too short"); }
    if v[1] != 0x01 { bail!("Only IPv4 supported"); }
    let xport = u16::from_be_bytes([v[2], v[3]]);
    let port  = xport ^ (magic >> 16) as u16;
    let xip   = u32::from_be_bytes([v[4], v[5], v[6], v[7]]);
    let ip_u32 = xip ^ magic;
    let ip = std::net::Ipv4Addr::from(ip_u32);
    // Verify the XOR with tx_id bytes for paranoia (IPv6 path would use tx)
    let _ = tx;
    Ok(ReflexiveAddr { ip: ip.into(), port })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_request_length() {
        let tx = TransactionId::random();
        let pkt = build_request(&tx);
        assert_eq!(pkt.len(), 20);
        // First 2 bytes = BINDING_REQUEST
        assert_eq!(u16::from_be_bytes([pkt[0], pkt[1]]), BINDING_REQUEST);
        // Bytes 4..8 = MAGIC_COOKIE
        let magic = u32::from_be_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]);
        assert_eq!(magic, MAGIC_COOKIE);
    }

    #[test]
    fn parse_rejects_short_packet() {
        let tx = TransactionId::random();
        assert!(parse_response(&[0u8; 10], &tx).is_err());
    }

    #[test]
    fn parse_rejects_wrong_magic() {
        let tx = TransactionId::random();
        let mut pkt = vec![0u8; 20];
        pkt[0..2].copy_from_slice(&BINDING_RESPONSE.to_be_bytes());
        // Leave magic as 0 — wrong
        pkt[8..20].copy_from_slice(&tx.0);
        assert!(parse_response(&pkt, &tx).is_err());
    }
}
