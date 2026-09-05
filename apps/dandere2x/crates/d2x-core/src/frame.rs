//! `Frame`: an RGB8 image in row-major interleaved layout.
//!
//! Upstream's `Frame` is `vector<vector<Color>>` indexed `[x][y]` — a
//! pointer-chase per pixel and column-major, so every block read walks
//! cache lines the wrong way. This is one contiguous buffer; a block copy is
//! `block_size` row `memcpy`s.

use std::fmt;

/// How to fill pixels outside the source when padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pad {
    /// Fill with black. Upstream's residual "bleed" buffer does this.
    Black,
    /// Clamp coordinates to the nearest edge pixel. What a CNN wants at an
    /// image border, and what the pipeline uses to pad a frame up to a
    /// block-size multiple.
    Replicate,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// `width * height * 3` bytes, RGB interleaved, row-major.
    pub data: Vec<u8>,
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Frame({}x{})", self.width, self.height)
    }
}

impl Frame {
    pub fn new(width: u32, height: u32) -> Self {
        Frame { width, height, data: vec![0; (width as usize) * (height as usize) * 3] }
    }

    pub fn from_rgb(width: u32, height: u32, data: Vec<u8>) -> Result<Self, FrameError> {
        let want = (width as usize) * (height as usize) * 3;
        if data.len() != want {
            return Err(FrameError::Size { width, height, want, got: data.len() });
        }
        Ok(Frame { width, height, data })
    }

    #[inline]
    pub fn idx(&self, x: u32, y: u32) -> usize {
        ((y as usize) * (self.width as usize) + (x as usize)) * 3
    }

    #[inline]
    pub fn px(&self, x: u32, y: u32) -> [u8; 3] {
        let i = self.idx(x, y);
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    #[inline]
    pub fn set_px(&mut self, x: u32, y: u32, p: [u8; 3]) {
        let i = self.idx(x, y);
        self.data[i..i + 3].copy_from_slice(&p);
    }

    /// One row of pixels as bytes, `[x0, x1)`.
    #[inline]
    pub fn row(&self, y: u32, x0: u32, x1: u32) -> &[u8] {
        let s = self.idx(x0, y);
        let e = self.idx(x1, y);
        &self.data[s..e]
    }

    pub fn block_in_bounds(&self, x: u32, y: u32, size: u32) -> bool {
        x + size <= self.width && y + size <= self.height
    }

    /// Copy a `size`×`size` block from `src` at `(sx, sy)` to `self` at `(dx, dy)`.
    /// Both blocks must be in bounds; this is the hot inner primitive and
    /// checks only in debug builds.
    pub fn copy_block(&mut self, src: &Frame, size: u32, sx: u32, sy: u32, dx: u32, dy: u32) {
        debug_assert!(src.block_in_bounds(sx, sy, size), "src block OOB");
        debug_assert!(self.block_in_bounds(dx, dy, size), "dst block OOB");
        let row_bytes = (size as usize) * 3;
        for j in 0..size {
            let s = src.idx(sx, sy + j);
            let d = self.idx(dx, dy + j);
            self.data[d..d + row_bytes].copy_from_slice(&src.data[s..s + row_bytes]);
        }
    }

    /// Add a flat scalar to every channel of a block, clamped to `[0, 255]`.
    pub fn add_scalar_block(&mut self, x: u32, y: u32, size: u32, scalar: i32) {
        debug_assert!(self.block_in_bounds(x, y, size));
        if scalar == 0 {
            return;
        }
        for j in 0..size {
            let s = self.idx(x, y + j);
            for b in &mut self.data[s..s + (size as usize) * 3] {
                *b = (*b as i32 + scalar).clamp(0, 255) as u8;
            }
        }
    }

    /// A copy `pad` pixels larger on every side.
    pub fn padded(&self, pad: u32, mode: Pad) -> Frame {
        self.padded_to(self.width + 2 * pad, self.height + 2 * pad, pad, pad, mode)
    }

    /// Place `self` at `(ox, oy)` inside a new `w`×`h` frame; the rest is
    /// filled per `mode`. `w >= ox + width`, `h >= oy + height`.
    pub fn padded_to(&self, w: u32, h: u32, ox: u32, oy: u32, mode: Pad) -> Frame {
        assert!(w >= ox + self.width && h >= oy + self.height, "padded_to: target too small");
        let mut out = Frame::new(w, h);
        match mode {
            Pad::Black => {
                for y in 0..self.height {
                    let d = out.idx(ox, oy + y);
                    let rb = (self.width as usize) * 3;
                    out.data[d..d + rb].copy_from_slice(self.row(y, 0, self.width));
                }
            }
            Pad::Replicate => {
                // Row-wise: the interior is a memcpy, the left/right margins
                // are one pixel repeated, and out-of-range rows repeat the
                // nearest source row. No per-pixel index arithmetic.
                let rb = (self.width as usize) * 3;
                for y in 0..h {
                    let sy = y.saturating_sub(oy).min(self.height - 1);
                    let src = self.row(sy, 0, self.width);
                    let d0 = out.idx(0, y);
                    let row = &mut out.data[d0..d0 + (w as usize) * 3];
                    let (left, rest) = row.split_at_mut((ox as usize) * 3);
                    let (mid, right) = rest.split_at_mut(rb);
                    mid.copy_from_slice(src);
                    let first = &src[..3];
                    let last = &src[rb - 3..];
                    for px in left.chunks_exact_mut(3) {
                        px.copy_from_slice(first);
                    }
                    for px in right.chunks_exact_mut(3) {
                        px.copy_from_slice(last);
                    }
                }
            }
        }
        out
    }

    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Frame {
        assert!(x + w <= self.width && y + h <= self.height, "crop OOB");
        let mut out = Frame::new(w, h);
        let rb = (w as usize) * 3;
        for j in 0..h {
            let s = self.idx(x, y + j);
            let d = out.idx(0, j);
            out.data[d..d + rb].copy_from_slice(&self.data[s..s + rb]);
        }
        out
    }

    /// The `size`×`size` block whose top-left is at `(sx2 / 2, sy2 / 2)` in
    /// half-pel units: bilinear where an ordinate is odd, exact copy where
    /// both are even. Rounded `(a + b + 1) >> 1` / `(a + b + c + d + 2) >> 2`,
    /// which is exactly the 2×2 box-downsample of a 2× nearest-upscaled
    /// frame read at that odd output offset — so this is the true low-res
    /// mirror of what the merge copies, not an approximation of it.
    /// Requires `sx2 / 2 + size (+1 if odd) <= width` and likewise for y.
    pub fn block_halfpel(&self, sx2: u32, sy2: u32, size: u32) -> Frame {
        let (x0, y0) = (sx2 / 2, sy2 / 2);
        let (fx, fy) = (sx2 % 2 == 1, sy2 % 2 == 1);
        debug_assert!(x0 + size + fx as u32 <= self.width && y0 + size + fy as u32 <= self.height, "block_halfpel OOB");
        let mut out = Frame::new(size, size);
        let rb = (size as usize) * 3;
        for j in 0..size {
            let d = out.idx(0, j);
            let r0 = self.row(y0 + j, x0, x0 + size + fx as u32);
            let r1 = if fy { self.row(y0 + j + 1, x0, x0 + size + fx as u32) } else { r0 };
            let o = &mut out.data[d..d + rb];
            match (fx, fy) {
                (false, false) => o.copy_from_slice(&r0[..rb]),
                (true, false) => {
                    for i in 0..rb {
                        o[i] = ((r0[i] as u16 + r0[i + 3] as u16 + 1) >> 1) as u8;
                    }
                }
                (false, true) => {
                    for i in 0..rb {
                        o[i] = ((r0[i] as u16 + r1[i] as u16 + 1) >> 1) as u8;
                    }
                }
                (true, true) => {
                    for i in 0..rb {
                        o[i] = ((r0[i] as u16 + r0[i + 3] as u16 + r1[i] as u16 + r1[i + 3] as u16 + 2) >> 2) as u8;
                    }
                }
            }
        }
        out
    }

    /// [`block_halfpel`](Self::block_halfpel) with H.264's 6-tap half-pel
    /// filter `[1, −5, 20, 20, −5, 1] / 32` instead of bilinear; the
    /// centre position filters the unrounded horizontal intermediates
    /// vertically (`+512 >> 10`), as the standard does. Taps past the frame
    /// edge replicate the edge pixel.
    ///
    /// Why a second filter: bilinear at half-pel is a 2-tap box, which
    /// blurs a sharp edge by a full pixel. A CNN's ×2 output read at an odd
    /// pixel is *sharp* — the network reconstructs the edge — so the
    /// low-res question "would that copy pass the gate" is answered better
    /// by the sharpest cheap interpolant than by the softest. This is the
    /// same filter every video codec uses for the same reason.
    pub fn block_halfpel_6tap(&self, sx2: u32, sy2: u32, size: u32) -> Frame {
        let (x0, y0) = (sx2 / 2, sy2 / 2);
        let (fx, fy) = (sx2 % 2 == 1, sy2 % 2 == 1);
        if !fx && !fy {
            return self.crop(x0, y0, size, size);
        }
        let (w, h) = (self.width as i64, self.height as i64);
        let at = |x: i64, y: i64, c: usize| -> i32 {
            let xi = x.clamp(0, w - 1) as u32;
            let yi = y.clamp(0, h - 1) as u32;
            self.data[self.idx(xi, yi) + c] as i32
        };
        const T: [i32; 6] = [1, -5, 20, 20, -5, 1];
        // Horizontal 6-tap at (x + 1/2, y), unrounded.
        let hsum = |x: i64, y: i64, c: usize| -> i32 { (0..6).map(|k| T[k] * at(x - 2 + k as i64, y, c)).sum() };
        let mut out = Frame::new(size, size);
        for j in 0..size as i64 {
            for i in 0..size as i64 {
                let (x, y) = (x0 as i64 + i, y0 as i64 + j);
                let mut px = [0u8; 3];
                for c in 0..3 {
                    let v = match (fx, fy) {
                        (true, false) => (hsum(x, y, c) + 16) >> 5,
                        (false, true) => ((0..6).map(|k| T[k] * at(x, y - 2 + k as i64, c)).sum::<i32>() + 16) >> 5,
                        _ => ((0..6).map(|k| T[k] * hsum(x, y - 2 + k as i64, c)).sum::<i32>() + 512) >> 10,
                    };
                    px[c] = v.clamp(0, 255) as u8;
                }
                out.set_px(i as u32, j as u32, px);
            }
        }
        out
    }

    /// Add uniform noise in `[-strength, strength]` to every channel, clamped.
    /// Deterministic for a given seed (a SplitMix64 stream), so a run is
    /// reproducible and tests can pin outputs.
    ///
    /// Why the engine wants this at all: the quality gate compares each block
    /// against the frame's own JPEG error. In a flat region JPEG is nearly
    /// lossless, so the gate's threshold collapses to zero and nothing can
    /// match unless it is bit-identical. A little noise raises that floor
    /// uniformly. Upstream does the same with ffmpeg's `noise=c0s=8:c0f=u`.
    pub fn add_uniform_noise(&mut self, strength: u8, seed: u64) {
        if strength == 0 {
            return;
        }
        let mut s = seed ^ 0x9E37_79B9_7F4A_7C15;
        let span = 2 * strength as u32 + 1;
        // One SplitMix64 step feeds eight bytes; the byte-level work is a
        // fixed-width loop the compiler can vectorise.
        for chunk in self.data.chunks_mut(8) {
            s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            let lanes = z.to_le_bytes();
            for (b, &r) in chunk.iter_mut().zip(&lanes) {
                let n = (r as u32 % span) as i32 - strength as i32;
                *b = (*b as i32 + n).clamp(0, 255) as u8;
            }
        }
    }

    /// PSNR in dB between two same-sized frames (per-sample MSE over all
    /// three channels); `f64::INFINITY` if identical.
    ///
    /// Upstream sums the three channels but divides by the pixel count, which
    /// reads 4.77 dB lower than this. Its 10 dB scene-cut gate is therefore
    /// ≈5.2 dB in these units — see `AnalyzeConfig::min_psnr`.
    pub fn psnr(&self, other: &Frame) -> f64 {
        assert_eq!((self.width, self.height), (other.width, other.height));
        // 32-bit partial sums per 4 KiB (max 4096·255² < 2³²) so the reduction
        // stays in packed 32-bit lanes; SSE2 has no 64-bit multiply.
        let sse: u64 = self
            .data
            .chunks(4096)
            .zip(other.data.chunks(4096))
            .map(|(x, y)| {
                let mut s: u32 = 0;
                for (&a, &b) in x.iter().zip(y) {
                    let d = a as i32 - b as i32;
                    s += (d * d) as u32;
                }
                s as u64
            })
            .sum();
        if sse == 0 {
            return f64::INFINITY;
        }
        let mse = sse as f64 / (self.data.len() as f64);
        20.0 * 255f64.log10() - 10.0 * mse.log10()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame {width}x{height} needs {want} bytes, got {got}")]
    Size { width: u32, height: u32, want: usize, got: usize },
}

/// Sum of squared per-channel differences between two same-sized blocks.
/// Exact integer arithmetic; MSE comparisons are invariant to the shared
/// `1/size²` factor, so the engine compares SSEs and never divides.
#[inline]
pub fn block_sse(a: &Frame, ax: u32, ay: u32, b: &Frame, bx: u32, by: u32, size: u32) -> u64 {
    debug_assert!(a.block_in_bounds(ax, ay, size) && b.block_in_bounds(bx, by, size));
    let mut sum: u64 = 0;
    for j in 0..size {
        let ra = a.row(ay + j, ax, ax + size);
        let rb = b.row(by + j, bx, bx + size);
        let mut s: u32 = 0;
        for (&p, &q) in ra.iter().zip(rb) {
            let d = p as i32 - q as i32;
            s += (d * d) as u32;
        }
        sum += s as u64;
    }
    sum
}

/// [`block_sse`] that gives up as soon as the running sum exceeds `limit`,
/// returning `None`. The block matcher rejects most candidates; stopping
/// after the first row or two is where the search's speed comes from.
#[inline]
pub fn block_sse_bounded(
    a: &Frame,
    ax: u32,
    ay: u32,
    b: &Frame,
    bx: u32,
    by: u32,
    size: u32,
    limit: u64,
) -> Option<u64> {
    debug_assert!(a.block_in_bounds(ax, ay, size) && b.block_in_bounds(bx, by, size));
    let mut sum: u64 = 0;
    for j in 0..size {
        let ra = a.row(ay + j, ax, ax + size);
        let rb = b.row(by + j, bx, bx + size);
        let mut s: u32 = 0;
        for (&p, &q) in ra.iter().zip(rb) {
            let d = p as i32 - q as i32;
            s += (d * d) as u32;
        }
        sum += s as u64;
        if sum > limit {
            return None;
        }
    }
    Some(sum)
}

#[cfg(test)]
mod halfpel_tests {
    use super::*;

    /// Both filters are exact on a constant and on a linear ramp (the 6-tap
    /// sums to 32 and is symmetric), and identical to a plain crop at an
    /// integer position.
    #[test]
    fn halfpel_filters_agree_on_ramps_and_integers() {
        let mut f = Frame::new(24, 24);
        for y in 0..24 {
            for x in 0..24 {
                f.set_px(x, y, [(4 * x) as u8, (2 * y + 10) as u8, 77]);
            }
        }
        for &(sx2, sy2) in &[(8u32, 8u32), (9, 8), (8, 9), (9, 9)] {
            let a = f.block_halfpel(sx2, sy2, 8);
            let b = f.block_halfpel_6tap(sx2, sy2, 8);
            assert_eq!(a, b, "({sx2},{sy2})");
            // x ramp of slope 4 per pixel: a half-pel shift adds 2.
            let want_r = (4 * (sx2 / 2) + 2 * (sx2 % 2)) as u8;
            let want_g = (2 * (sy2 / 2) + (sy2 % 2) + 10) as u8;
            assert_eq!(a.px(0, 0), [want_r, want_g, 77], "({sx2},{sy2})");
        }
        assert_eq!(f.block_halfpel_6tap(8, 8, 8), f.crop(4, 4, 8, 8));
    }

    /// On a step edge the 6-tap keeps the edge sharper than the 2-tap box:
    /// its half-pel value overshoots the box's midpoint on both sides.
    #[test]
    fn six_tap_is_sharper_than_bilinear_on_an_edge() {
        let mut f = Frame::new(16, 8);
        for y in 0..8 {
            for x in 0..16 {
                f.set_px(x, y, [if x < 8 { 50 } else { 200 }; 3]);
            }
        }
        let bi = f.block_halfpel(2 * 4 + 1, 0, 8); // columns 4.5 .. 11.5
        let six = f.block_halfpel_6tap(2 * 4 + 1, 0, 8);
        assert_eq!(bi.px(3, 0)[0], 125); // the edge midpoint, box
        assert_eq!(six.px(3, 0)[0], 125); // symmetric: same midpoint
        assert_eq!((six.px(2, 0)[0], six.px(4, 0)[0]), (31, 219)); // (1000/32, 7000/32)
        assert!(six.px(2, 0)[0] < bi.px(2, 0)[0], "6-tap undershoots before the edge: {} vs {}", six.px(2, 0)[0], bi.px(2, 0)[0]);
        assert!(six.px(4, 0)[0] > bi.px(4, 0)[0], "6-tap overshoots after the edge: {} vs {}", six.px(4, 0)[0], bi.px(4, 0)[0]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Frame {
        let mut f = Frame::new(w, h);
        for y in 0..h {
            for x in 0..w {
                f.set_px(x, y, [(x * 7) as u8, (y * 11) as u8, (x + y) as u8]);
            }
        }
        f
    }

    #[test]
    fn copy_block_roundtrip() {
        let src = gradient(20, 20);
        let mut dst = Frame::new(20, 20);
        dst.copy_block(&src, 8, 4, 6, 10, 2);
        for j in 0..8 {
            for i in 0..8 {
                assert_eq!(dst.px(10 + i, 2 + j), src.px(4 + i, 6 + j));
            }
        }
        assert_eq!(dst.px(0, 0), [0, 0, 0]);
    }

    #[test]
    fn replicate_padding_clamps_edges() {
        let f = gradient(4, 3);
        let p = f.padded(2, Pad::Replicate);
        assert_eq!((p.width, p.height), (8, 7));
        assert_eq!(p.px(0, 0), f.px(0, 0));
        assert_eq!(p.px(7, 6), f.px(3, 2));
        assert_eq!(p.px(2, 2), f.px(0, 0));
        assert_eq!(p.px(5, 4), f.px(3, 2));
    }

    #[test]
    fn black_padding_is_black_outside() {
        let f = gradient(4, 3);
        let p = f.padded(1, Pad::Black);
        assert_eq!(p.px(0, 0), [0, 0, 0]);
        assert_eq!(p.px(1, 1), f.px(0, 0));
        assert_eq!(p.crop(1, 1, 4, 3), f);
    }

    #[test]
    fn sse_bounded_matches_unbounded() {
        let a = gradient(16, 16);
        let mut b = a.clone();
        b.add_uniform_noise(5, 1);
        let full = block_sse(&a, 0, 0, &b, 0, 0, 16);
        assert_eq!(block_sse_bounded(&a, 0, 0, &b, 0, 0, 16, full), Some(full));
        assert_eq!(block_sse_bounded(&a, 0, 0, &b, 0, 0, 16, full - 1), None);
        assert_eq!(block_sse(&a, 0, 0, &a, 0, 0, 16), 0);
    }

    #[test]
    fn noise_is_deterministic_and_bounded() {
        let a = gradient(16, 16);
        let mut b = a.clone();
        let mut c = a.clone();
        b.add_uniform_noise(8, 42);
        c.add_uniform_noise(8, 42);
        assert_eq!(b, c);
        assert_ne!(a, b);
        for (&p, &q) in a.data.iter().zip(&b.data) {
            assert!((p as i32 - q as i32).abs() <= 8 || p == 0 || p == 255);
        }
    }

    #[test]
    fn psnr_identical_is_infinite() {
        let a = gradient(8, 8);
        assert!(a.psnr(&a).is_infinite());
        let mut b = a.clone();
        b.add_uniform_noise(3, 7);
        let p = a.psnr(&b);
        assert!(p > 30.0 && p < 60.0, "psnr {p}");
    }
}
