//! `sim::net::codec` — shared bounded little-endian wire codec.
//!
//! **r340-sl-net extraction.** Every net module re-implements the same two
//! things: a bounded little-endian `Reader` (`take`/`u8`/`u16`/`u32`/`u64`/`i64`,
//! each returning `Option` rather than panicking) and the `Vec<u8>` +
//! `extend_from_slice(&x.to_le_bytes())` write pattern, plus the
//! `magic 0x554E` + version frame header and count-prefixed, length-capped
//! vec/string reads. Concrete duplicate sites in this base:
//!   * `sim::net::wire`        — `Reader` @132-160, header+encoders @55-130.
//!   * `sim::net_rendezvous`   — `Reader` @138-164, version blob @88-130.
//!   * `sim::net::dht_mailbox`, `sim::dht_join` — partial reader/length reads.
//! (The r339 cluster the dispatch names — `chunk_stream`, `entity_snapshot`,
//! `world_advert`, `world_presence` — is not in this r330b base; per the
//! candidate report, the same pattern is documented there and the coordinator
//! maps this codec onto those modules on merge.)
//!
//! **Byte-exactness is the contract.** This codec reproduces the existing wire
//! bytes *exactly* — same little-endian order, same 3-byte `[magic_lo, magic_hi,
//! version]` header, same `u16` count prefixes. The golden-vector tests below
//! pin the `wire` sync-packet and `net_rendezvous` blob-header layouts
//! byte-for-byte; consolidating onto this codec must not change a single byte.
//!
//! Pure / deterministic / panic-free: no IO/clock/RNG, no `unwrap`/`expect`/
//! `panic!`; every read is bounds-checked and returns `None` on truncation,
//! capped length, or bad UTF-8.

// =============================================================================
// Reader — bounded little-endian cursor (the canonical version of the copies in
// wire.rs / net_rendezvous.rs). Every primitive returns None rather than panic.
// =============================================================================

/// A bounds-checked little-endian read cursor over a byte slice.
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    #[inline]
    pub fn new(b: &'a [u8]) -> Reader<'a> {
        Reader { b, pos: 0 }
    }

    /// Bytes not yet consumed.
    #[inline]
    pub fn remaining(&self) -> usize {
        self.b.len().saturating_sub(self.pos)
    }

    /// True once every byte has been consumed — for "no trailing garbage" checks
    /// (e.g. `net_rendezvous::decode` rejects trailing bytes).
    #[inline]
    pub fn is_at_end(&self) -> bool {
        self.pos >= self.b.len()
    }

    /// Take exactly `n` bytes, advancing the cursor. `None` if fewer remain.
    #[inline]
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.b.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }

    #[inline]
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    #[inline]
    pub fn i8(&mut self) -> Option<i8> {
        Some(self.u8()? as i8)
    }
    #[inline]
    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    #[inline]
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    #[inline]
    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    #[inline]
    pub fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    #[inline]
    pub fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    #[inline]
    pub fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    /// A `u8` 0/1 boolean (any non-zero reads as `true`).
    #[inline]
    pub fn bool(&mut self) -> Option<bool> {
        Some(self.u8()? != 0)
    }

    /// Read the `magic` (`u16`) + `version` (`u8`) frame header, returning
    /// `Some(())` only if both match. Mirrors `wire::decode`'s first two checks.
    #[inline]
    pub fn check_header(&mut self, magic: u16, version: u8) -> Option<()> {
        if self.u16()? != magic {
            return None;
        }
        if self.u8()? != version {
            return None;
        }
        Some(())
    }

    /// Read a `u16`-length-prefixed UTF-8 string, rejecting any declared length
    /// over `max_len` (the capped-string-read pattern). Borrows from the buffer.
    pub fn read_str(&mut self, max_len: usize) -> Option<&'a str> {
        let len = self.u16()? as usize;
        if len > max_len {
            return None;
        }
        core::str::from_utf8(self.take(len)?).ok()
    }

    /// Read a `u16`-count-prefixed vec, rejecting a count over `max` (the
    /// capped-vec-read pattern), decoding each element with `f`. `None` if the
    /// count is over the cap or any element fails to decode.
    pub fn read_vec<T, F>(&mut self, max: usize, mut f: F) -> Option<Vec<T>>
    where
        F: FnMut(&mut Reader<'a>) -> Option<T>,
    {
        let count = self.u16()? as usize;
        if count > max {
            return None;
        }
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            out.push(f(self)?);
        }
        Some(out)
    }
}

// =============================================================================
// Writer — little-endian append into a Vec<u8>, byte-identical to the existing
// `extend_from_slice(&x.to_le_bytes())` / `push` sequences. Borrows the buffer
// so it can write into a *reused* allocation (the alloc-free perf bonus).
// =============================================================================

/// A little-endian writer that appends into a caller-owned `Vec<u8>`. Holding a
/// `&mut Vec<u8>` lets the same buffer be reused across encodes (clear + rewrite,
/// retaining capacity) for an allocation-free hot path — see [`encode_into`].
pub struct Writer<'a> {
    buf: &'a mut Vec<u8>,
}

impl<'a> Writer<'a> {
    /// Wrap a buffer and **append** (does not clear). Use [`encode_into`] to
    /// reuse a buffer from scratch.
    #[inline]
    pub fn new(buf: &'a mut Vec<u8>) -> Writer<'a> {
        Writer { buf }
    }

    #[inline]
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }
    #[inline]
    pub fn i8(&mut self, v: i8) -> &mut Self {
        self.buf.push(v as u8);
        self
    }
    #[inline]
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    #[inline]
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    #[inline]
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    #[inline]
    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    #[inline]
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    #[inline]
    pub fn f64(&mut self, v: f64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    #[inline]
    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.buf.push(v as u8);
        self
    }
    #[inline]
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(b);
        self
    }

    /// Write the `magic` (`u16` LE) + `version` (`u8`) frame header — the exact
    /// 3-byte prefix `wire`'s encoders emit.
    #[inline]
    pub fn header(&mut self, magic: u16, version: u8) -> &mut Self {
        self.u16(magic).u8(version)
    }

    /// Write a `u16`-length-prefixed string, truncated to `max_len` bytes (the
    /// inverse of [`Reader::read_str`]).
    pub fn write_str(&mut self, s: &str, max_len: usize) -> &mut Self {
        let bytes = s.as_bytes();
        let n = bytes.len().min(max_len).min(u16::MAX as usize);
        self.u16(n as u16).bytes(&bytes[..n])
    }

    /// Write a `u16`-count-prefixed sequence (count truncated to `max`),
    /// encoding each item with `f` (inverse of [`Reader::read_vec`]).
    pub fn write_vec<T, F>(&mut self, items: &[T], max: usize, mut f: F) -> &mut Self
    where
        F: FnMut(&mut Writer, &T),
    {
        let n = items.len().min(max).min(u16::MAX as usize);
        self.u16(n as u16);
        for it in items.iter().take(n) {
            f(self, it);
        }
        self
    }

    /// Bytes written so far.
    #[inline]
    pub fn len(&self) -> usize {
        self.buf.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

/// Encode into a **reused** buffer (the alloc-free path): clears `buf` while
/// keeping its capacity, then runs `f`. After a warm-up encode, repeated calls
/// allocate nothing. Byte-identical to a one-shot [`encode`].
#[inline]
pub fn encode_into<F: FnOnce(&mut Writer)>(buf: &mut Vec<u8>, f: F) {
    buf.clear();
    let mut w = Writer::new(buf);
    f(&mut w);
}

/// Convenience one-shot encode into a fresh `Vec`. Equivalent to
/// [`encode_into`] on an empty buffer.
#[inline]
pub fn encode<F: FnOnce(&mut Writer)>(f: F) -> Vec<u8> {
    let mut buf = Vec::new();
    encode_into(&mut buf, f);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Reader primitives + bounds ----

    #[test]
    fn reader_scalars_little_endian() {
        let bytes = [0x01u8, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0xAA];
        let mut r = Reader::new(&bytes);
        assert_eq!(r.u64(), Some(0x0807_0605_0403_0201));
        assert_eq!(r.u8(), Some(0xAA));
        assert!(r.is_at_end());
        assert_eq!(r.u8(), None, "read past end → None, no panic");
    }

    #[test]
    fn reader_truncation_returns_none() {
        let bytes = [0x01u8, 0x02];
        let mut r = Reader::new(&bytes);
        assert_eq!(r.u32(), None, "only 2 bytes for a u32");
        // a huge take can't overflow pos
        let mut r2 = Reader::new(&bytes);
        assert_eq!(r2.take(usize::MAX), None);
    }

    // ---- header + capped reads ----

    #[test]
    fn header_round_trips_and_rejects() {
        let magic = 0x554E_u16;
        let frame = encode(|w| {
            w.header(magic, 1).u8(7);
        });
        // exact 3-byte header then payload
        assert_eq!(&frame[..3], &[0x4E, 0x55, 0x01]);
        let mut r = Reader::new(&frame);
        assert_eq!(r.check_header(magic, 1), Some(()));
        assert_eq!(r.u8(), Some(7));
        // wrong version rejected
        let mut bad = frame.clone();
        bad[2] = 2;
        assert_eq!(Reader::new(&bad).check_header(magic, 1), None);
        // wrong magic rejected
        let mut bad2 = frame.clone();
        bad2[0] ^= 0xFF;
        assert_eq!(Reader::new(&bad2).check_header(magic, 1), None);
    }

    #[test]
    fn capped_str_and_vec() {
        let frame = encode(|w| {
            w.write_str("hello", 64);
            w.write_vec(&[1u32, 2, 3], 16, |w, &v| {
                w.u32(v);
            });
        });
        let mut r = Reader::new(&frame);
        assert_eq!(r.read_str(64), Some("hello"));
        let v = r.read_vec(16, |r| r.u32()).unwrap();
        assert_eq!(v, vec![1, 2, 3]);
        assert!(r.is_at_end());
        // over-cap declared length is rejected (the security-relevant part)
        let mut r2 = Reader::new(&frame);
        assert_eq!(r2.read_str(4), None, "len 5 > cap 4 → None");
        let mut r3 = Reader::new(&frame);
        let _ = r3.read_str(64);
        assert_eq!(r3.read_vec(2, |r| r.u32()), None, "count 3 > cap 2 → None");
    }

    #[test]
    fn write_str_truncates_to_max() {
        let frame = encode(|w| {
            w.write_str("abcdef", 3);
        });
        // u16 len = 3, then "abc"
        assert_eq!(frame, vec![3, 0, b'a', b'b', b'c']);
    }

    // ---- BYTE-PARITY golden vectors against the existing wire formats ----

    #[test]
    fn golden_parity_wire_sync_packet() {
        // sim::net::wire::encode_sync_packet layout:
        // [magic u16 LE][version u8][KIND_SYNC=1][tick u64 LE][checksum u64 LE]
        const MAGIC: u16 = 0x554E;
        const VERSION: u8 = 1;
        const KIND_SYNC: u8 = 1;
        let tick = 0x0102_0304_0506_0708u64;
        let cksum = 0x1122_3344_5566_7788u64;

        let via_codec = encode(|w| {
            w.header(MAGIC, VERSION).u8(KIND_SYNC).u64(tick).u64(cksum);
        });
        // hand-laid expected bytes (exactly what wire.rs emits)
        let mut expect = Vec::new();
        expect.extend_from_slice(&MAGIC.to_le_bytes());
        expect.push(VERSION);
        expect.push(KIND_SYNC);
        expect.extend_from_slice(&tick.to_le_bytes());
        expect.extend_from_slice(&cksum.to_le_bytes());
        assert_eq!(via_codec, expect, "codec must reproduce wire sync bytes exactly");
        assert_eq!(via_codec.len(), 20);

        // and the codec reader decodes them back to the same fields
        let mut r = Reader::new(&via_codec);
        assert_eq!(r.check_header(MAGIC, VERSION), Some(()));
        assert_eq!(r.u8(), Some(KIND_SYNC));
        assert_eq!(r.u64(), Some(tick));
        assert_eq!(r.u64(), Some(cksum));
        assert!(r.is_at_end());
    }

    #[test]
    fn golden_parity_rendezvous_blob_header() {
        // sim::net_rendezvous::MailboxBlob::encode header layout:
        // [version u8][peer_id u64 LE][session u64 LE][nonce u64 LE][count u16 LE]
        const BLOB_VERSION: u8 = 1;
        let (peer, session, nonce, count) = (0xAABBu64, 0xCCDDu64, 0xEEFFu64, 2u16);
        let via_codec = encode(|w| {
            w.u8(BLOB_VERSION).u64(peer).u64(session).u64(nonce).u16(count);
        });
        let mut expect = Vec::new();
        expect.push(BLOB_VERSION);
        expect.extend_from_slice(&peer.to_le_bytes());
        expect.extend_from_slice(&session.to_le_bytes());
        expect.extend_from_slice(&nonce.to_le_bytes());
        expect.extend_from_slice(&count.to_le_bytes());
        assert_eq!(via_codec, expect, "codec must reproduce rendezvous blob bytes exactly");
        assert_eq!(via_codec.len(), 27);
    }

    // ---- PERF BONUS: alloc-free reuse path is byte-identical ----

    #[test]
    fn reused_buffer_is_byte_identical_and_allocfree() {
        let enc = |w: &mut Writer| {
            w.header(0x554E, 1).u8(1).u64(42).u64(99);
        };
        // one-shot
        let fresh = encode(enc);

        // reused buffer across several encodes — must match byte-for-byte
        let mut buf = Vec::new();
        for _ in 0..5 {
            encode_into(&mut buf, enc);
            assert_eq!(buf, fresh, "reused-buffer encode must equal one-shot");
        }
        // after warm-up the capacity is retained (no realloc on reuse)
        let cap_before = buf.capacity();
        for _ in 0..100 {
            encode_into(&mut buf, enc);
        }
        assert_eq!(buf.capacity(), cap_before, "reuse must not reallocate");
        assert_eq!(buf, fresh);
    }

    #[test]
    fn round_trip_deterministic() {
        let a = encode(|w| {
            w.header(0x554E, 1).write_str("dragon", 32).write_vec(&[7u64, 8], 8, |w, &v| { w.u64(v); });
        });
        let b = encode(|w| {
            w.header(0x554E, 1).write_str("dragon", 32).write_vec(&[7u64, 8], 8, |w, &v| { w.u64(v); });
        });
        assert_eq!(a, b);
        let mut r = Reader::new(&a);
        assert_eq!(r.check_header(0x554E, 1), Some(()));
        assert_eq!(r.read_str(32), Some("dragon"));
        assert_eq!(r.read_vec(8, |r| r.u64()), Some(vec![7, 8]));
        assert!(r.is_at_end());
    }
}
