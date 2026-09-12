//! Canonical Huffman codes plus MSB-first bit I/O.
//!
//! The code is fully determined by one `u8` length per symbol, assigned the
//! DEFLATE way (RFC 1951 §3.2.2): shorter codes first, ties by symbol index.
//! That is the whole contract between the Python trainer and this decoder —
//! the trainer emits lengths, both sides derive identical codes.
//!
//! Decoding uses a 10-bit primary lookup table for the common short codes and
//! falls back to the bit-serial canonical walk (puff.c style) for the rest.

/// Primary decode table width in bits. Codes of this length or shorter decode
/// in one table lookup; longer ones take the serial path.
pub const FAST_BITS: u32 = 10;

#[derive(Debug, Clone)]
pub struct Code {
    /// Code word (right-aligned) and length in bits, per symbol.
    pub codes: Vec<(u32, u8)>,
    /// Number of codes of each length, index = length.
    count: Vec<u16>,
    /// Symbols sorted by (length, index): the canonical order.
    sorted: Vec<u16>,
    /// `fast[peek10]` = (symbol, len) or (0, 0) meaning "use the slow path".
    fast: Vec<(u16, u8)>,
    max_len: u8,
}

impl Code {
    /// Build from per-symbol lengths (0 = symbol has no code). Panics if the
    /// lengths over-subscribe the Kraft budget, which the trainer guarantees
    /// they never do; a vocab that does is a build error, not a runtime one.
    pub fn from_lengths(lens: &[u8]) -> Code {
        let max_len = lens.iter().copied().max().unwrap_or(0);
        let mut count = vec![0u16; max_len as usize + 1];
        for &l in lens {
            if l > 0 {
                count[l as usize] += 1;
            }
        }
        // Kraft check: sum 2^-len <= 1  <=>  sum count[len] << (max-len) <= 1 << max
        let kraft: u64 = (1..=max_len as usize)
            .map(|l| (count[l] as u64) << (max_len as usize - l))
            .sum();
        assert!(kraft <= 1u64 << max_len, "over-subscribed Huffman lengths");

        let mut next_code = vec![0u32; max_len as usize + 2];
        let mut code = 0u32;
        for bits in 1..=max_len as usize {
            code = (code + count[bits - 1] as u32) << 1;
            next_code[bits] = code;
        }
        // (count[0] is always 0 here: symbols with length 0 have no code, so
        // the RFC recurrence above is exact.)
        let mut codes = vec![(0u32, 0u8); lens.len()];
        let mut next = next_code;
        for (i, &l) in lens.iter().enumerate() {
            if l > 0 {
                codes[i] = (next[l as usize], l);
                next[l as usize] += 1;
            }
        }
        let mut sorted: Vec<u16> = (0..lens.len() as u16).filter(|&i| lens[i as usize] > 0).collect();
        sorted.sort_by_key(|&i| (lens[i as usize], i));

        let mut fast = vec![(0u16, 0u8); 1 << FAST_BITS];
        for (i, &(c, l)) in codes.iter().enumerate() {
            if l == 0 || l as u32 > FAST_BITS {
                continue;
            }
            let shift = FAST_BITS - l as u32;
            let base = (c << shift) as usize;
            for k in 0..(1usize << shift) {
                fast[base + k] = (i as u16, l);
            }
        }
        Code { codes, count, sorted, fast, max_len }
    }

    pub fn max_len(&self) -> u8 {
        self.max_len
    }

    /// Length in bits of `sym`'s code (0 if it has none).
    #[inline]
    pub fn len(&self, sym: u16) -> u8 {
        self.codes[sym as usize].1
    }

    /// Decode one symbol. Returns `None` if the bits do not form a valid code
    /// before `max_len` bits are consumed (corrupt or exhausted input).
    #[inline]
    pub fn decode(&self, br: &mut BitReader) -> Option<u16> {
        let peek = br.peek(FAST_BITS);
        let (sym, l) = self.fast[peek as usize];
        if l > 0 {
            if br.bits_left() < l as u32 {
                return None;
            }
            br.consume(l as u32);
            return Some(sym);
        }
        // serial canonical walk
        let mut code = 0u32;
        let mut first = 0u32;
        let mut index = 0u32;
        for len in 1..=self.max_len as usize {
            code |= br.read_bit()?;
            let cnt = self.count[len] as u32;
            if code.wrapping_sub(first) < cnt {
                return Some(self.sorted[(index + (code - first)) as usize]);
            }
            index += cnt;
            first += cnt;
            first <<= 1;
            code <<= 1;
        }
        None
    }
}

/// MSB-first bit writer.
#[derive(Default)]
pub struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }
    #[inline]
    pub fn write(&mut self, code: u32, len: u8) {
        debug_assert!(len <= 32);
        self.acc = (self.acc << len) | code as u64;
        self.n += len as u32;
        while self.n >= 8 {
            self.n -= 8;
            self.out.push((self.acc >> self.n) as u8);
        }
        self.acc &= (1u64 << self.n) - 1;
    }
    /// Pad with zero bits to a byte boundary and return the bytes.
    pub fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push((self.acc << (8 - self.n)) as u8);
        }
        self.out
    }
    pub fn bits_written(&self) -> usize {
        self.out.len() * 8 + self.n as usize
    }
}

/// MSB-first bit reader over a byte slice. Reads past the end yield zeros for
/// `peek` (so the fast table can be probed) but `read_bit` / `consume` refuse
/// to go past the real end.
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize, // next byte to load
    acc: u64,
    n: u32,     // valid bits in acc (top-aligned at bit n-1 .. 0)
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, acc: 0, n: 0 }
    }
    #[inline]
    fn refill(&mut self) {
        while self.n <= 56 && self.pos < self.data.len() {
            self.acc = (self.acc << 8) | self.data[self.pos] as u64;
            self.pos += 1;
            self.n += 8;
        }
    }
    /// Top `k` bits (k <= 32), zero-padded past the end of input.
    #[inline]
    pub fn peek(&mut self, k: u32) -> u32 {
        self.refill();
        if self.n >= k {
            ((self.acc >> (self.n - k)) & ((1u64 << k) - 1)) as u32
        } else {
            ((self.acc << (k - self.n)) & ((1u64 << k) - 1)) as u32
        }
    }
    #[inline]
    pub fn consume(&mut self, k: u32) {
        debug_assert!(self.n >= k);
        self.n -= k;
        self.acc &= if self.n == 64 { u64::MAX } else { (1u64 << self.n) - 1 };
    }
    #[inline]
    pub fn bits_left(&mut self) -> u32 {
        self.refill();
        self.n + ((self.data.len() - self.pos) as u32).saturating_mul(8)
    }
    #[inline]
    pub fn read_bit(&mut self) -> Option<u32> {
        self.refill();
        if self.n == 0 {
            return None;
        }
        self.n -= 1;
        let b = ((self.acc >> self.n) & 1) as u32;
        self.acc &= (1u64 << self.n) - 1;
        Some(b)
    }
    #[inline]
    pub fn read(&mut self, k: u32) -> Option<u32> {
        if self.bits_left() < k {
            return None;
        }
        let v = self.peek(k);
        self.consume(k);
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_assignment_matches_rfc1951_example() {
        // RFC 1951 §3.2.2 example: lengths (3,3,3,3,3,2,4,4) for A..H
        let lens = [3u8, 3, 3, 3, 3, 2, 4, 4];
        let c = Code::from_lengths(&lens);
        let want = [0b010, 0b011, 0b100, 0b101, 0b110, 0b00, 0b1110, 0b1111];
        for (i, w) in want.iter().enumerate() {
            assert_eq!(c.codes[i].0, *w, "symbol {i}");
        }
    }

    #[test]
    fn roundtrip_random_symbols_fast_and_slow_paths() {
        // 300 symbols, lengths 2..=16 by construction (Kraft sum 0.9928),
        // so both the 10-bit table path and the serial path are exercised.
        let mut lens = Vec::new();
        for (n, l) in [(2usize, 2u8), (4, 4), (8, 6), (16, 8), (32, 10), (64, 12), (128, 14), (46, 16)] {
            lens.extend(std::iter::repeat_n(l, n));
        }
        assert_eq!(lens.len(), 300);
        let c = Code::from_lengths(&lens);
        let mut w = BitWriter::new();
        let mut seq = Vec::new();
        let mut s = 12345u32;
        for _ in 0..5000 {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let sym = (s >> 8) as usize % 300;
            seq.push(sym as u16);
            let (code, l) = c.codes[sym];
            w.write(code, l);
        }
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes);
        for (i, &sym) in seq.iter().enumerate() {
            assert_eq!(c.decode(&mut r), Some(sym), "at {i}");
        }
    }

    #[test]
    fn decode_refuses_to_run_past_input() {
        let lens = [1u8, 2, 2];
        let c = Code::from_lengths(&lens);
        let mut w = BitWriter::new();
        w.write(0b10, 2); // symbol 1
        let bytes = w.finish(); // one byte, 6 padding zero bits -> symbol 0 x6 then None
        let mut r = BitReader::new(&bytes);
        assert_eq!(c.decode(&mut r), Some(1));
        for _ in 0..6 {
            assert_eq!(c.decode(&mut r), Some(0));
        }
        assert_eq!(c.decode(&mut r), None);
    }

    #[test]
    #[should_panic(expected = "over-subscribed")]
    fn oversubscribed_lengths_panic() {
        Code::from_lengths(&[1, 1, 1]);
    }
}
