//! Adaptive binary arithmetic coder (32-bit range, 12-bit probabilities).
//!
//! The shape is the classic fpaq0 / lpaq one: the coder keeps a live
//! interval `[x1, x2]`, splits it at `p(bit=1)`, and emits a byte whenever
//! the top byte of both ends agrees. Probabilities are adaptive counters
//! nudged toward the observed bit by `1/2^RATE`. There is nothing to
//! transmit: the decoder runs the same model in lock-step.
//!
//! Two properties matter for a byte-budgeted codec: the flush is one byte,
//! and cost tracks −log2 p exactly, so a coefficient that is almost always
//! zero costs almost nothing.

/// Probability precision. p is in `1..4096`, meaning p/4096 that the bit is 1.
pub const PROB_BITS: u32 = 12;
const PROB_ONE: u32 = 1 << PROB_BITS;

/// One adaptive bit model.
#[derive(Debug, Clone, Copy)]
pub struct Bit {
    p: u16,
}

impl Default for Bit {
    fn default() -> Self {
        Bit { p: (PROB_ONE / 2) as u16 }
    }
}

impl Bit {
    /// Adaptation speed: bigger is slower/steadier. 4 suits tiny images
    /// (few symbols per context, must learn fast); 5 is better past ~1 KB.
    pub const RATE: u32 = 4;
    #[inline]
    pub fn p(&self) -> u32 {
        self.p as u32
    }
    #[inline]
    pub fn update(&mut self, bit: u32) {
        let p = self.p as u32;
        self.p = if bit != 0 { p + ((PROB_ONE - p) >> Self::RATE) } else { p - (p >> Self::RATE) } as u16;
        // keep strictly inside (0, 4096) so neither symbol becomes impossible
        self.p = self.p.clamp(1, (PROB_ONE - 1) as u16);
    }
}

pub struct Encoder {
    x1: u32,
    x2: u32,
    out: Vec<u8>,
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Encoder {
    pub fn new() -> Self {
        Encoder { x1: 0, x2: u32::MAX, out: Vec::new() }
    }
    /// Start on an existing buffer (e.g. after a hand-written header).
    pub fn with_prefix(out: Vec<u8>) -> Self {
        Encoder { x1: 0, x2: u32::MAX, out }
    }
    #[inline]
    pub fn encode(&mut self, bit: u32, m: &mut Bit) {
        let p = m.p();
        let xmid = self.x1 + (((self.x2 - self.x1) as u64 * p as u64) >> PROB_BITS) as u32;
        debug_assert!(xmid >= self.x1 && xmid < self.x2);
        if bit != 0 {
            self.x2 = xmid;
        } else {
            self.x1 = xmid + 1;
        }
        m.update(bit);
        while (self.x1 ^ self.x2) & 0xFF00_0000 == 0 {
            self.out.push((self.x2 >> 24) as u8);
            self.x1 <<= 8;
            self.x2 = (self.x2 << 8) | 255;
        }
    }
    /// Encode a bit at fixed probability 1/2 (no model).
    #[inline]
    pub fn encode_raw(&mut self, bit: u32) {
        let p = PROB_ONE / 2;
        let xmid = self.x1 + (((self.x2 - self.x1) as u64 * p as u64) >> PROB_BITS) as u32;
        if bit != 0 {
            self.x2 = xmid;
        } else {
            self.x1 = xmid + 1;
        }
        while (self.x1 ^ self.x2) & 0xFF00_0000 == 0 {
            self.out.push((self.x2 >> 24) as u8);
            self.x1 <<= 8;
            self.x2 = (self.x2 << 8) | 255;
        }
    }
    /// Flush with a single byte: after normalisation the top bytes of x1 and
    /// x2 differ, so `top(x1)+1 << 24` lies in `[x1, x2]` and the decoder's
    /// zero-padding lands inside the interval.
    pub fn finish(mut self) -> Vec<u8> {
        self.out.push(((self.x1 >> 24) + 1) as u8);
        self.out
    }
    pub fn len(&self) -> usize {
        self.out.len()
    }
    pub fn is_empty(&self) -> bool {
        self.out.is_empty()
    }
}

pub struct Decoder<'a> {
    x1: u32,
    x2: u32,
    x: u32,
    data: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        let mut d = Decoder { x1: 0, x2: u32::MAX, x: 0, data, pos: 0 };
        for _ in 0..4 {
            d.x = (d.x << 8) | d.next_byte() as u32;
        }
        d
    }
    #[inline]
    fn next_byte(&mut self) -> u8 {
        let b = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        b
    }
    /// Bytes consumed so far (may exceed the input by up to 4 after the end).
    pub fn position(&self) -> usize {
        self.pos
    }
    #[inline]
    pub fn decode(&mut self, m: &mut Bit) -> u32 {
        let p = m.p();
        let xmid = self.x1 + (((self.x2 - self.x1) as u64 * p as u64) >> PROB_BITS) as u32;
        let bit = if self.x <= xmid {
            self.x2 = xmid;
            1
        } else {
            self.x1 = xmid + 1;
            0
        };
        m.update(bit);
        while (self.x1 ^ self.x2) & 0xFF00_0000 == 0 {
            self.x1 <<= 8;
            self.x2 = (self.x2 << 8) | 255;
            self.x = (self.x << 8) | self.next_byte() as u32;
        }
        bit
    }
    #[inline]
    pub fn decode_raw(&mut self) -> u32 {
        let p = PROB_ONE / 2;
        let xmid = self.x1 + (((self.x2 - self.x1) as u64 * p as u64) >> PROB_BITS) as u32;
        let bit = if self.x <= xmid {
            self.x2 = xmid;
            1
        } else {
            self.x1 = xmid + 1;
            0
        };
        while (self.x1 ^ self.x2) & 0xFF00_0000 == 0 {
            self.x1 <<= 8;
            self.x2 = (self.x2 << 8) | 255;
            self.x = (self.x << 8) | self.next_byte() as u32;
        }
        bit
    }
}

// ── binarisations shared by the image codec ──────────────────────────────────

/// Unary-then-Exp-Golomb magnitude coder over a small context array: the
/// first `ctx.len()` unary steps are modelled, the remainder is Exp-Golomb
/// with raw bits. `v >= 0`.
pub fn encode_mag(e: &mut Encoder, ctx: &mut [Bit], v: u32) {
    let n = ctx.len() as u32;
    let mut i = 0;
    while i < n {
        if v > i {
            e.encode(1, &mut ctx[i as usize]);
        } else {
            e.encode(0, &mut ctx[i as usize]);
            return;
        }
        i += 1;
    }
    // v >= n: Exp-Golomb of (v - n)
    let r = v - n + 1;
    let nbits = 32 - r.leading_zeros(); // >= 1
    for _ in 1..nbits {
        e.encode_raw(1);
    }
    e.encode_raw(0);
    for k in (0..nbits - 1).rev() {
        e.encode_raw((r >> k) & 1);
    }
}

pub fn decode_mag(d: &mut Decoder, ctx: &mut [Bit]) -> u32 {
    let n = ctx.len() as u32;
    let mut i = 0;
    while i < n {
        if d.decode(&mut ctx[i as usize]) == 0 {
            return i;
        }
        i += 1;
    }
    let mut nbits = 1;
    while d.decode_raw() == 1 {
        nbits += 1;
        if nbits > 31 {
            return u32::MAX; // corrupt; caller bounds-checks
        }
    }
    let mut r = 1u32;
    for _ in 1..nbits {
        r = (r << 1) | d.decode_raw();
    }
    n + r - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_skewed_bits_costs_near_entropy() {
        // 20k bits, p(1) = 1/16 → entropy ~0.337 bits/bit ≈ 843 bytes
        let mut s = 0x1234_5678u32;
        let bits: Vec<u32> = (0..20_000)
            .map(|_| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                ((s >> 16) & 15 == 0) as u32
            })
            .collect();
        let mut e = Encoder::new();
        let mut m = Bit::default();
        for &b in &bits {
            e.encode(b, &mut m);
        }
        let out = e.finish();
        assert!(out.len() > 700 && out.len() < 1000, "{} bytes", out.len());
        let mut d = Decoder::new(&out);
        let mut m = Bit::default();
        for (i, &b) in bits.iter().enumerate() {
            assert_eq!(d.decode(&mut m), b, "bit {i}");
        }
    }

    #[test]
    fn raw_bits_roundtrip_and_cost_one_bit_each() {
        let mut e = Encoder::new();
        for i in 0..800u32 {
            e.encode_raw((i * 7 + i / 3) & 1);
        }
        let out = e.finish();
        assert!(out.len() >= 100 && out.len() <= 102, "{}", out.len());
        let mut d = Decoder::new(&out);
        for i in 0..800u32 {
            assert_eq!(d.decode_raw(), (i * 7 + i / 3) & 1);
        }
    }

    #[test]
    fn magnitude_binarisation_roundtrips_all_ranges() {
        let vals = [0u32, 1, 2, 3, 4, 5, 6, 7, 8, 15, 16, 17, 100, 1000, 65535, 1 << 20];
        let mut e = Encoder::new();
        let mut ctx = [Bit::default(); 6];
        for &v in &vals {
            encode_mag(&mut e, &mut ctx, v);
        }
        let out = e.finish();
        let mut d = Decoder::new(&out);
        let mut ctx = [Bit::default(); 6];
        for &v in &vals {
            assert_eq!(decode_mag(&mut d, &mut ctx), v);
        }
    }

    #[test]
    fn one_byte_flush_is_enough_even_for_a_single_symbol() {
        for bit in 0..2 {
            let mut e = Encoder::new();
            let mut m = Bit::default();
            e.encode(bit, &mut m);
            let out = e.finish();
            assert_eq!(out.len(), 1);
            let mut d = Decoder::new(&out);
            let mut m = Bit::default();
            assert_eq!(d.decode(&mut m), bit);
        }
    }
}
