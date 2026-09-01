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
                for y in 0..h {
                    let sy = y.saturating_sub(oy).min(self.height - 1);
                    for x in 0..w {
                        let sx = x.saturating_sub(ox).min(self.width - 1);
                        let p = self.px(sx, sy);
                        out.set_px(x, y, p);
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
        for b in &mut self.data {
            s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            let n = (z % span as u64) as i32 - strength as i32;
            *b = (*b as i32 + n).clamp(0, 255) as u8;
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
        let sse: u64 = self
            .data
            .iter()
            .zip(&other.data)
            .map(|(&a, &b)| {
                let d = a as i64 - b as i64;
                (d * d) as u64
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
