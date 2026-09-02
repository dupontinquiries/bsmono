//! # bincoder — adaptive binary range coder with context modelling
//!
//! Why this exists (SESSION_LOG 2026-09-02): after motion layers, a
//! film frame's side info is ~5.6 KB raw — three region bitmaps, ~1,050
//! motion-vector deltas, ~690 gain pairs — and zstd brings it to ~4 KB.
//! The rANS path with static per-frame tables is chosen on 0 of 49
//! frames: an order-0 coder loses to LZ+FSE on this data, and adding
//! contexts to a path that is never taken buys nothing. What the
//! bytes ask for is ADAPTIVE probabilities under CONTEXT: a region's
//! mode given its neighbours' modes and its motion layer, a vector
//! delta's magnitude given the neighbours', a gain flag given the
//! layer. Every one of those is decodable state.
//!
//! The coder is the VP8/AV1-family binary arithmetic coder: 32-bit
//! range, 8-bit probability of the zero branch, adaptive by a
//! shift-based update whose rate speeds up for the first few symbols
//! of each context. Symbols wider than a bit are binarised: unary
//! prefixes for small alphabets, exp-Golomb for magnitudes, with a
//! context per bin position so each bin has its own adapted
//! probability.

pub struct Ctx {
    /// Probability of a 0 bit, in 1/256.
    p: u16,
    n: u16,
}

impl Ctx {
    pub fn new() -> Self {
        Ctx { p: 128, n: 0 }
    }
    #[inline]
    fn prob(&self) -> u32 {
        self.p.clamp(1, 255) as u32
    }
    #[inline]
    fn update(&mut self, bit: u8) {
        // Fast then slow: rate 3 for the first symbols, 5 after.
        let rate = if self.n < 16 { 3 } else { 5 };
        self.n = self.n.saturating_add(1);
        let target: i32 = if bit == 0 { 256 } else { 0 };
        let p = self.p as i32;
        self.p = (p + ((target - p) >> rate)).clamp(1, 255) as u16;
    }
}

impl Default for Ctx {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Encoder {
    low: u64,
    range: u32,
    out: Vec<u8>,
    // LZMA-style carry propagation: one cached byte plus a run of
    // 0xFF bytes whose value depends on a carry not yet known.
    cache: u8,
    cache_size: u64,
}

impl Encoder {
    pub fn new() -> Self {
        Encoder { low: 0, range: 0xFFFF_FFFF, out: Vec::new(), cache: 0, cache_size: 1 }
    }

    fn shift_low(&mut self) {
        if (self.low as u32) < 0xFF00_0000 || (self.low >> 32) != 0 {
            let carry = (self.low >> 32) as u8;
            let mut temp = self.cache;
            loop {
                self.out.push(temp.wrapping_add(carry));
                temp = 0xFF;
                self.cache_size -= 1;
                if self.cache_size == 0 {
                    break;
                }
            }
            self.cache = ((self.low as u32) >> 24) as u8;
        }
        self.cache_size += 1;
        self.low = ((self.low as u32) << 8) as u64;
    }

    #[inline]
    pub fn bit(&mut self, ctx: &mut Ctx, bit: u8) {
        let p0 = ctx.prob();
        let bound = (self.range >> 8) * p0;
        if bit == 0 {
            self.range = bound;
        } else {
            self.low += bound as u64;
            self.range -= bound;
        }
        while self.range < (1 << 24) {
            self.range <<= 8;
            self.shift_low();
        }
        ctx.update(bit);
    }

    /// Equiprobable bit, no context.
    pub fn raw_bit(&mut self, bit: u8) {
        let bound = self.range >> 1;
        if bit == 0 {
            self.range = bound;
        } else {
            self.low += bound as u64;
            self.range -= bound;
        }
        while self.range < (1 << 24) {
            self.range <<= 8;
            self.shift_low();
        }
    }

    /// Unsigned magnitude: exp-Golomb order 0, prefix bins in
    /// `ctxs[0..]` (one per prefix position, last one reused), suffix
    /// bits raw.
    pub fn ueg(&mut self, ctxs: &mut [Ctx], v: u32) {
        let x = v + 1;
        let nbits = 32 - x.leading_zeros(); // >= 1
        for i in 0..nbits - 1 {
            let ci = (i as usize).min(ctxs.len() - 1);
            self.bit(&mut ctxs[ci], 1);
        }
        let ci = ((nbits - 1) as usize).min(ctxs.len() - 1);
        self.bit(&mut ctxs[ci], 0);
        for i in (0..nbits - 1).rev() {
            self.raw_bit(((x >> i) & 1) as u8);
        }
    }

    pub fn seg(&mut self, ctxs: &mut [Ctx], sign_ctx: &mut Ctx, v: i32) {
        self.ueg(ctxs, v.unsigned_abs());
        if v != 0 {
            self.bit(sign_ctx, (v < 0) as u8);
        }
    }

    pub fn finish(mut self) -> Vec<u8> {
        for _ in 0..5 {
            self.shift_low();
        }
        self.out
    }
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Decoder<'a> {
    data: &'a [u8],
    pos: usize,
    code: u32,
    range: u32,
}

impl<'a> Decoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        let mut d = Decoder { data, pos: 0, code: 0, range: 0xFFFF_FFFF };
        // first byte is the (carry-free) cache of an untouched low = 0
        d.pos = 1;
        for _ in 0..4 {
            d.code = (d.code << 8) | d.next() as u32;
        }
        d
    }
    #[inline]
    fn next(&mut self) -> u8 {
        let b = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        b
    }
    #[inline]
    pub fn bit(&mut self, ctx: &mut Ctx) -> u8 {
        let p0 = ctx.prob();
        let bound = (self.range >> 8) * p0;
        let bit = if self.code < bound {
            self.range = bound;
            0
        } else {
            self.code -= bound;
            self.range -= bound;
            1
        };
        while self.range < (1 << 24) {
            self.range <<= 8;
            self.code = (self.code << 8) | self.next() as u32;
        }
        ctx.update(bit);
        bit
    }
    pub fn raw_bit(&mut self) -> u8 {
        let bound = self.range >> 1;
        let bit = if self.code < bound {
            self.range = bound;
            0
        } else {
            self.code -= bound;
            self.range -= bound;
            1
        };
        while self.range < (1 << 24) {
            self.range <<= 8;
            self.code = (self.code << 8) | self.next() as u32;
        }
        bit
    }
    pub fn ueg(&mut self, ctxs: &mut [Ctx]) -> u32 {
        let mut nbits = 1u32;
        loop {
            let ci = ((nbits - 1) as usize).min(ctxs.len() - 1);
            if self.bit(&mut ctxs[ci]) == 0 {
                break;
            }
            nbits += 1;
            if nbits > 32 {
                return u32::MAX;
            }
        }
        let mut x = 1u32;
        for _ in 0..nbits - 1 {
            x = (x << 1) | self.raw_bit() as u32;
        }
        x - 1
    }
    pub fn seg(&mut self, ctxs: &mut [Ctx], sign_ctx: &mut Ctx) -> i32 {
        let m = self.ueg(ctxs) as i32;
        if m != 0 && self.bit(sign_ctx) == 1 {
            -m
        } else {
            m
        }
    }
}

// ── Side-info coder ─────────────────────────────────────────────────
//
// One pass over the regions in raster order. Everything conditioned
// on decodable state: the already-coded left/top neighbours and the
// region's motion-layer selection (transmitted before this stream).

pub struct RegionSide<'a> {
    pub gw: usize,
    pub gh: usize,
    /// 0 plate/model, 1 prev-MC, 2 intra DC, 3 intra planar, 4 dict.
    pub modes: &'a [u8],
    /// Per mode-1 region in raster order: quarter-pel vector.
    pub mvs: &'a [(i8, i8)],
    /// Per region: gain flag.
    pub gain_flag: &'a [bool],
    /// Per gain region in raster order: (qa, b).
    pub gains: &'a [(i8, i8)],
    /// Per region: motion-layer selection symbol (0 = model), or None.
    pub layer: Option<&'a [u8]>,
}

struct SideCtx {
    mode_first: Vec<Ctx>,   // [layer 0/1/2+][left mode class][top mode class]
    mode_rest: Vec<Ctx>,    // second bin: temporal vs intra, then planar/dc
    mv_zero: Vec<Ctx>,      // [neighbour zero count 0..2]
    mv_mag: Vec<Ctx>,       // exp-golomb prefix bins, x and y
    mv_sign: Ctx,
    gain: Vec<Ctx>,         // [layer][left gain][top gain]
    qa_mag: Vec<Ctx>,
    qa_sign: Ctx,
    b_mag: Vec<Ctx>,
    b_sign: Ctx,
    /// "same pair as the last coded gain" — the exact repeats zstd's
    /// LZ was finding (the code's own note: repeat pairs are LZ gold).
    gain_rep: Vec<Ctx>,
    /// "same delta as the last coded vector".
    mv_rep: Vec<Ctx>,
}

impl SideCtx {
    fn new() -> Self {
        let mk = |n: usize| (0..n).map(|_| Ctx::new()).collect::<Vec<_>>();
        SideCtx {
            mode_first: mk(3 * 3 * 3),
            mode_rest: mk(3 * 4),
            mv_zero: mk(3 * 3),
            mv_mag: mk(2 * 8),
            mv_sign: Ctx::new(),
            gain: mk(3 * 2 * 2),
            qa_mag: mk(6),
            qa_sign: Ctx::new(),
            b_mag: mk(6),
            b_sign: Ctx::new(),
            gain_rep: mk(2),
            mv_rep: mk(2),
        }
    }
}

fn mode_class(m: u8) -> usize {
    match m {
        0 => 0,
        1 => 1,
        _ => 2,
    }
}

/// Median-of-neighbours MV predictor over the decoded grid, as the
/// existing zstd path uses.
fn mv_pred(grid: &[Option<(i8, i8)>], gw: usize, gi: usize, gx: usize, gy: usize) -> (i8, i8) {
    let mut cand: Vec<(i8, i8)> = Vec::with_capacity(3);
    if gx > 0 {
        if let Some(v) = grid[gi - 1] { cand.push(v); }
    }
    if gy > 0 {
        if let Some(v) = grid[gi - gw] { cand.push(v); }
        if gx + 1 < gw {
            if let Some(v) = grid[gi - gw + 1] { cand.push(v); }
        }
    }
    match cand.len() {
        0 => (0, 0),
        1 => cand[0],
        2 => (((cand[0].0 as i32 + cand[1].0 as i32) / 2) as i8, ((cand[0].1 as i32 + cand[1].1 as i32) / 2) as i8),
        _ => {
            let mut xs = [cand[0].0, cand[1].0, cand[2].0];
            let mut ys = [cand[0].1, cand[1].1, cand[2].1];
            xs.sort_unstable();
            ys.sort_unstable();
            (xs[1], ys[1])
        }
    }
}

pub fn encode_side(s: &RegionSide) -> Vec<u8> {
    let (gw, gh) = (s.gw, s.gh);
    let mut e = Encoder::new();
    let mut c = SideCtx::new();
    let mut grid: Vec<Option<(i8, i8)>> = vec![None; gw * gh];
    let (mut mvi, mut gi_) = (0usize, 0usize);
    let (mut last_mvd, mut last_mvd_rep) = ((0i32, 0i32), false);
    let (mut last_gain, mut last_gain_rep) = ((0i8, 0i8), false);
    for gy in 0..gh {
        for gx in 0..gw {
            let gi = gy * gw + gx;
            let layer = s.layer.map(|l| (l[gi] as usize).min(2)).unwrap_or(0);
            let left = if gx > 0 { mode_class(s.modes[gi - 1]) } else { 0 };
            let top = if gy > 0 { mode_class(s.modes[gi - gw]) } else { 0 };
            let m = s.modes[gi];
            // bin 1: model (0) vs anything else
            e.bit(&mut c.mode_first[layer * 9 + left * 3 + top], (m != 0) as u8);
            if m != 0 {
                // bin 2: prev-MC (0) vs intra/dict (1); bin 3: dc/planar/dict
                e.bit(&mut c.mode_rest[layer * 4], (m != 1) as u8);
                if m != 1 {
                    e.bit(&mut c.mode_rest[layer * 4 + 1], (m == 4) as u8);
                    if m != 4 {
                        e.bit(&mut c.mode_rest[layer * 4 + 2], (m == 3) as u8);
                    }
                }
            }
            if m == 1 {
                let mv = s.mvs[mvi];
                mvi += 1;
                let pred = mv_pred(&grid, gw, gi, gx, gy);
                let nz_neigh = (gx > 0 && grid[gi - 1].map(|v| v != (0, 0)).unwrap_or(false)) as usize
                    + (gy > 0 && grid[gi - gw].map(|v| v != (0, 0)).unwrap_or(false)) as usize;
                e.bit(&mut c.mv_zero[layer * 3 + nz_neigh], (mv != (0, 0)) as u8);
                if mv != (0, 0) {
                    let dx = mv.0 as i32 - pred.0 as i32;
                    let dy = mv.1 as i32 - pred.1 as i32;
                    let rep = (dx, dy) == last_mvd;
                    e.bit(&mut c.mv_rep[last_mvd_rep as usize], rep as u8);
                    last_mvd_rep = rep;
                    if !rep {
                        let (mx, my) = c.mv_mag.split_at_mut(8);
                        e.seg(mx, &mut c.mv_sign, dx);
                        e.seg(my, &mut c.mv_sign, dy);
                        last_mvd = (dx, dy);
                    }
                }
                grid[gi] = Some(mv);
            }
            let gl = if gx > 0 { s.gain_flag[gi - 1] as usize } else { 0 };
            let gt = if gy > 0 { s.gain_flag[gi - gw] as usize } else { 0 };
            e.bit(&mut c.gain[layer * 4 + gl * 2 + gt], s.gain_flag[gi] as u8);
            if s.gain_flag[gi] {
                let (qa, b) = s.gains[gi_];
                gi_ += 1;
                let rep = (qa, b) == last_gain;
                e.bit(&mut c.gain_rep[last_gain_rep as usize], rep as u8);
                last_gain_rep = rep;
                if !rep {
                    e.seg(&mut c.qa_mag, &mut c.qa_sign, qa as i32);
                    e.seg(&mut c.b_mag, &mut c.b_sign, b as i32);
                    last_gain = (qa, b);
                }
            }
        }
    }
    e.finish()
}

/// Decode back into (modes, mvs, gain flags, gains). Used by the
/// round-trip test; a decoder for the GOP would call it in place of
/// unpacking the zstd'd bitmaps.
pub fn decode_side(
    data: &[u8],
    gw: usize,
    gh: usize,
    layer: Option<&[u8]>,
) -> (Vec<u8>, Vec<(i8, i8)>, Vec<bool>, Vec<(i8, i8)>) {
    let mut d = Decoder::new(data);
    let mut c = SideCtx::new();
    let mut modes = vec![0u8; gw * gh];
    let mut mvs = Vec::new();
    let mut gain_flag = vec![false; gw * gh];
    let mut gains = Vec::new();
    let mut grid: Vec<Option<(i8, i8)>> = vec![None; gw * gh];
    let (mut last_mvd, mut last_mvd_rep) = ((0i32, 0i32), false);
    let (mut last_gain, mut last_gain_rep) = ((0i8, 0i8), false);
    for gy in 0..gh {
        for gx in 0..gw {
            let gi = gy * gw + gx;
            let lay = layer.map(|l| (l[gi] as usize).min(2)).unwrap_or(0);
            let left = if gx > 0 { mode_class(modes[gi - 1]) } else { 0 };
            let top = if gy > 0 { mode_class(modes[gi - gw]) } else { 0 };
            let mut m = 0u8;
            if d.bit(&mut c.mode_first[lay * 9 + left * 3 + top]) == 1 {
                m = 1;
                if d.bit(&mut c.mode_rest[lay * 4]) == 1 {
                    if d.bit(&mut c.mode_rest[lay * 4 + 1]) == 1 {
                        m = 4;
                    } else {
                        m = if d.bit(&mut c.mode_rest[lay * 4 + 2]) == 1 { 3 } else { 2 };
                    }
                }
            }
            modes[gi] = m;
            if m == 1 {
                let pred = mv_pred(&grid, gw, gi, gx, gy);
                let nz_neigh = (gx > 0 && grid[gi - 1].map(|v| v != (0, 0)).unwrap_or(false)) as usize
                    + (gy > 0 && grid[gi - gw].map(|v| v != (0, 0)).unwrap_or(false)) as usize;
                let mv = if d.bit(&mut c.mv_zero[lay * 3 + nz_neigh]) == 1 {
                    let rep = d.bit(&mut c.mv_rep[last_mvd_rep as usize]) == 1;
                    last_mvd_rep = rep;
                    let (dx, dy) = if rep {
                        last_mvd
                    } else {
                        let (mx, my) = c.mv_mag.split_at_mut(8);
                        let dx = d.seg(mx, &mut c.mv_sign);
                        let dy = d.seg(my, &mut c.mv_sign);
                        last_mvd = (dx, dy);
                        (dx, dy)
                    };
                    ((pred.0 as i32 + dx) as i8, (pred.1 as i32 + dy) as i8)
                } else {
                    (0, 0)
                };
                grid[gi] = Some(mv);
                mvs.push(mv);
            }
            let gl = if gx > 0 { gain_flag[gi - 1] as usize } else { 0 };
            let gt = if gy > 0 { gain_flag[gi - gw] as usize } else { 0 };
            if d.bit(&mut c.gain[lay * 4 + gl * 2 + gt]) == 1 {
                gain_flag[gi] = true;
                let rep = d.bit(&mut c.gain_rep[last_gain_rep as usize]) == 1;
                last_gain_rep = rep;
                let pair = if rep {
                    last_gain
                } else {
                    let qa = d.seg(&mut c.qa_mag, &mut c.qa_sign) as i8;
                    let b = d.seg(&mut c.b_mag, &mut c.b_sign) as i8;
                    last_gain = (qa, b);
                    (qa, b)
                };
                gains.push(pair);
            }
        }
    }
    (modes, mvs, gain_flag, gains)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_roundtrip() {
        let mut e = Encoder::new();
        let mut c = Ctx::new();
        let bits: Vec<u8> = (0..5000).map(|i| ((i * 7919) % 13 < 3) as u8).collect();
        for &b in &bits {
            e.bit(&mut c, b);
        }
        let mut cm = [Ctx::new(), Ctx::new(), Ctx::new()];
        let mut cs = Ctx::new();
        for v in [-40i32, 0, 1, 7, -300, 12345] {
            e.seg(&mut cm, &mut cs, v);
        }
        let out = e.finish();
        let mut d = Decoder::new(&out);
        let mut c = Ctx::new();
        for &b in &bits {
            assert_eq!(d.bit(&mut c), b);
        }
        let mut cm = [Ctx::new(), Ctx::new(), Ctx::new()];
        let mut cs = Ctx::new();
        for v in [-40i32, 0, 1, 7, -300, 12345] {
            assert_eq!(d.seg(&mut cm, &mut cs), v);
        }
        // and it compressed the skewed source
        assert!(out.len() < 5000 / 8, "{} bytes", out.len());
    }

    #[test]
    fn side_roundtrip() {
        let (gw, gh) = (40, 25);
        let mut modes = vec![0u8; gw * gh];
        let mut mvs = Vec::new();
        let mut gain_flag = vec![false; gw * gh];
        let mut gains = Vec::new();
        let mut layer = vec![1u8; gw * gh];
        for gi in 0..gw * gh {
            let (gx, gy) = (gi % gw, gi / gw);
            if gx > 10 && gx < 25 && gy > 5 && gy < 18 {
                layer[gi] = 2;
                modes[gi] = if (gi * 31) % 7 == 0 { 3 } else { 1 };
            } else if (gi * 17) % 11 == 0 {
                modes[gi] = 1;
            }
            if modes[gi] == 1 {
                mvs.push((((gi % 5) as i8) - 2, ((gi % 3) as i8) - 1));
            }
            if (gi * 13) % 9 == 0 {
                gain_flag[gi] = true;
                gains.push((((gi % 7) as i8) - 3, ((gi % 5) as i8) - 2));
            }
        }
        let side = RegionSide { gw, gh, modes: &modes, mvs: &mvs, gain_flag: &gain_flag, gains: &gains, layer: Some(&layer) };
        let bytes = encode_side(&side);
        let (m2, mv2, g2, gv2) = decode_side(&bytes, gw, gh, Some(&layer));
        assert_eq!(m2, modes);
        assert_eq!(mv2, mvs);
        assert_eq!(g2, gain_flag);
        assert_eq!(gv2, gains);
    }
}
