//! `packet_compress` — optional per-datagram compression for LAN traffic
//! (r498). A 1-byte header (`0` = raw, `1` = RLE) precedes the payload, so small
//! packets (transforms, a couple of edits) pay a single byte and never expand,
//! while bulk payloads (large block-edit batches, structure pastes) shrink via
//! the dependency-free PackBits codec already used for chunk streaming.
//!
//! Pure + panic-free. `unpack` is bounded by `max_out` so a corrupt/hostile
//! compressed packet can neither panic nor exhaust memory.

use crate::sim::net::chunk_compress;

/// Header byte values.
const RAW: u8 = 0;
const RLE: u8 = 1;

/// Frame `payload` for the wire. Below `min_len` (or when compression wouldn't
/// help) it's sent raw with a `0` header; otherwise RLE-compressed with a `1`
/// header — but only if that's actually smaller.
pub fn pack(payload: &[u8], min_len: usize) -> Vec<u8> {
    if payload.len() >= min_len {
        let comp = chunk_compress::compress(payload);
        if comp.len() + 1 < payload.len() {
            let mut v = Vec::with_capacity(comp.len() + 1);
            v.push(RLE);
            v.extend_from_slice(&comp);
            return v;
        }
    }
    let mut v = Vec::with_capacity(payload.len() + 1);
    v.push(RAW);
    v.extend_from_slice(payload);
    v
}

/// Recover the original payload. `max_out` bounds the decompressed size.
pub fn unpack(buf: &[u8], max_out: usize) -> Option<Vec<u8>> {
    let (flag, rest) = buf.split_first()?;
    match *flag {
        RAW => Some(rest.to_vec()),
        RLE => chunk_compress::decompress(rest, max_out),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_round_trips_small() {
        let p = b"hello world";
        let framed = pack(p, 64); // below threshold → raw
        assert_eq!(framed[0], RAW);
        assert_eq!(unpack(&framed, 4096).unwrap(), p);
    }

    #[test]
    fn compressible_payload_shrinks_and_round_trips() {
        let p = vec![7u8; 2000]; // highly repetitive → RLE wins
        let framed = pack(&p, 64);
        assert_eq!(framed[0], RLE);
        assert!(framed.len() < p.len(), "should shrink: {} vs {}", framed.len(), p.len());
        assert_eq!(unpack(&framed, 8192).unwrap(), p);
    }

    #[test]
    fn incompressible_payload_stays_raw_no_expand() {
        // pseudo-random-ish bytes don't compress; must fall back to raw.
        let p: Vec<u8> = (0..500u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let framed = pack(&p, 64);
        assert_eq!(framed.len(), p.len() + 1, "raw fallback adds only the 1-byte header");
        assert_eq!(unpack(&framed, 4096).unwrap(), p);
    }

    #[test]
    fn corrupt_unpack_is_bounded() {
        // unknown flag → None, never panics
        assert!(unpack(&[9, 1, 2, 3], 100).is_none());
        assert!(unpack(&[], 100).is_none());
    }
}
