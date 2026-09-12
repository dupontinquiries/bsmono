//! `tinyimg` — a headerless 8×8-DCT image codec that lands under a budget.
//!
//! JPEG at 64×64 is ~600 bytes of tables and markers before the first pixel;
//! WebP/AVIF are better but still carry container framing, and none of them
//! can be told "you have 752 bytes". This codec has a 4–6 byte header and
//! codes everything else with the adaptive arithmetic coder in [`crate::bac`],
//! so a 64×64 greyscale photo at recognisable quality is ~500–750 bytes.
//!
//! Pipeline: optional RGB→YCbCr (chroma at half resolution), pad to 8×8
//! blocks by edge replication, float DCT-II, uniform quantisation with a
//! frequency-dependent step scaled by `q`, then per-block coefficient coding
//! in zigzag order: DC as a delta from the previous block, each AC as a
//! contexted significance bit + sign + magnitude, with a "last nonzero" flag
//! after every significant coefficient so trailing zeros are free.
//!
//! [`fit`] is the part the budget question actually needs: for each
//! candidate resolution it binary-searches `q` for the smallest that fits,
//! decodes, upsamples to a common reference size and scores PSNR there, so
//! "smaller but sharper" and "bigger but blockier" are compared on equal
//! terms. The CLI prints the whole table.

use crate::bac::{decode_mag, encode_mag, Bit, Decoder, Encoder};

/// Format version byte: bit 0 = colour, bits 1..8 = version (currently 0).
const FLAG_COLOR: u8 = 1;

/// An 8-bit image, greyscale (`channels == 1`) or RGB (`channels == 3`),
/// row-major, interleaved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub channels: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Stream is shorter than its header.
    Truncated,
    /// Unknown version bits.
    Version(u8),
    /// Zero dimension or a size the caller's limit refuses.
    BadSize,
    /// Zero quantiser.
    BadQ,
    /// Bad channel count on the input side.
    BadChannels,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tinyimg: {self:?}")
    }
}
impl std::error::Error for Error {}

impl Image {
    pub fn new(width: u32, height: u32, channels: u32) -> Image {
        Image { width, height, channels, data: vec![0; (width * height * channels) as usize] }
    }
    pub fn gray(width: u32, height: u32, data: Vec<u8>) -> Image {
        debug_assert_eq!(data.len(), (width * height) as usize);
        Image { width, height, channels: 1, data }
    }
    pub fn rgb(width: u32, height: u32, data: Vec<u8>) -> Image {
        debug_assert_eq!(data.len(), (width * height * 3) as usize);
        Image { width, height, channels: 3, data }
    }
    #[inline]
    pub fn at(&self, x: u32, y: u32, c: u32) -> u8 {
        self.data[((y * self.width + x) * self.channels + c) as usize]
    }

    /// Luma only (BT.601 full range), for scoring and for greyscale coding.
    pub fn to_gray(&self) -> Image {
        if self.channels == 1 {
            return self.clone();
        }
        let mut g = Vec::with_capacity((self.width * self.height) as usize);
        for px in self.data.chunks_exact(3) {
            g.push(rgb_to_ycc(px[0], px[1], px[2]).0);
        }
        Image::gray(self.width, self.height, g)
    }

    /// Area-averaging (box) resample to `w × h`. Correct for downscaling any
    /// ratio; for upscaling use [`Image::resize_bilinear`].
    pub fn resize_box(&self, w: u32, h: u32) -> Image {
        let mut out = Image::new(w, h, self.channels);
        let sx = self.width as f32 / w as f32;
        let sy = self.height as f32 / h as f32;
        for y in 0..h {
            let y0 = (y as f32 * sy) as u32;
            let y1 = (((y + 1) as f32 * sy).ceil() as u32).clamp(y0 + 1, self.height);
            for x in 0..w {
                let x0 = (x as f32 * sx) as u32;
                let x1 = (((x + 1) as f32 * sx).ceil() as u32).clamp(x0 + 1, self.width);
                for c in 0..self.channels {
                    let mut acc = 0u32;
                    let mut n = 0u32;
                    for yy in y0..y1 {
                        for xx in x0..x1 {
                            acc += self.at(xx, yy, c) as u32;
                            n += 1;
                        }
                    }
                    out.data[((y * w + x) * self.channels + c) as usize] = ((acc + n / 2) / n) as u8;
                }
            }
        }
        out
    }

    /// Bilinear resample (used to bring a decoded thumbnail back up to the
    /// reference size for scoring).
    pub fn resize_bilinear(&self, w: u32, h: u32) -> Image {
        let mut out = Image::new(w, h, self.channels);
        let sx = self.width as f32 / w as f32;
        let sy = self.height as f32 / h as f32;
        for y in 0..h {
            let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
            let y0 = (fy as u32).min(self.height - 1);
            let y1 = (y0 + 1).min(self.height - 1);
            let wy = fy - y0 as f32;
            for x in 0..w {
                let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
                let x0 = (fx as u32).min(self.width - 1);
                let x1 = (x0 + 1).min(self.width - 1);
                let wx = fx - x0 as f32;
                for c in 0..self.channels {
                    let a = self.at(x0, y0, c) as f32;
                    let b = self.at(x1, y0, c) as f32;
                    let cc = self.at(x0, y1, c) as f32;
                    let d = self.at(x1, y1, c) as f32;
                    let v = (a * (1.0 - wx) + b * wx) * (1.0 - wy) + (cc * (1.0 - wx) + d * wx) * wy;
                    out.data[((y * w + x) * self.channels + c) as usize] = (v + 0.5) as u8;
                }
            }
        }
        out
    }

    /// Size that fits the long edge to `long` with the aspect kept (min 1).
    pub fn fit_long_edge(&self, long: u32) -> (u32, u32) {
        if self.width >= self.height {
            (long, ((self.height as u64 * long as u64 + self.width as u64 / 2) / self.width as u64).max(1) as u32)
        } else {
            (((self.width as u64 * long as u64 + self.height as u64 / 2) / self.height as u64).max(1) as u32, long)
        }
    }
}

/// PSNR in dB between two same-shaped images (luma if colour). `inf` when
/// identical.
pub fn psnr(a: &Image, b: &Image) -> f64 {
    let (a, b) = (a.to_gray(), b.to_gray());
    assert_eq!((a.width, a.height), (b.width, b.height));
    let mse: f64 =
        a.data.iter().zip(&b.data).map(|(&x, &y)| (x as f64 - y as f64).powi(2)).sum::<f64>() / a.data.len() as f64;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    }
}

// ── colour ───────────────────────────────────────────────────────────────────

#[inline]
fn rgb_to_ycc(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let (r, g, b) = (r as f32, g as f32, b as f32);
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let cb = 128.0 - 0.168736 * r - 0.331264 * g + 0.5 * b;
    let cr = 128.0 + 0.5 * r - 0.418688 * g - 0.081312 * b;
    (clamp8(y), clamp8(cb), clamp8(cr))
}

#[inline]
fn ycc_to_rgb(y: u8, cb: u8, cr: u8) -> (u8, u8, u8) {
    let (y, cb, cr) = (y as f32, cb as f32 - 128.0, cr as f32 - 128.0);
    (clamp8(y + 1.402 * cr), clamp8(y - 0.344136 * cb - 0.714136 * cr), clamp8(y + 1.772 * cb))
}

#[inline]
fn clamp8(v: f32) -> u8 {
    (v + 0.5).clamp(0.0, 255.0) as u8
}

// ── DCT ──────────────────────────────────────────────────────────────────────

struct Cos {
    t: [[f32; 8]; 8], // t[k][n] = c(k) * cos((2n+1)kπ/16)
}

fn cos_table() -> &'static Cos {
    static C: std::sync::OnceLock<Cos> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        let mut t = [[0f32; 8]; 8];
        for (k, row) in t.iter_mut().enumerate() {
            let ck = if k == 0 { (0.125f32).sqrt() } else { 0.5 };
            for (n, v) in row.iter_mut().enumerate() {
                *v = ck * (((2 * n + 1) as f32 * k as f32 * std::f32::consts::PI) / 16.0).cos();
            }
        }
        Cos { t }
    })
}

/// Forward 8×8 DCT-II (orthonormal). `px` is row-major, level-shifted.
fn dct8x8(px: &[f32; 64]) -> [f32; 64] {
    let c = &cos_table().t;
    let mut tmp = [0f32; 64];
    for y in 0..8 {
        for k in 0..8 {
            let mut s = 0.0;
            for n in 0..8 {
                s += px[y * 8 + n] * c[k][n];
            }
            tmp[y * 8 + k] = s;
        }
    }
    let mut out = [0f32; 64];
    for x in 0..8 {
        for k in 0..8 {
            let mut s = 0.0;
            for n in 0..8 {
                s += tmp[n * 8 + x] * c[k][n];
            }
            out[k * 8 + x] = s;
        }
    }
    out
}

fn idct8x8(co: &[f32; 64]) -> [f32; 64] {
    let c = &cos_table().t;
    let mut tmp = [0f32; 64];
    for v in 0..8 {
        for x in 0..8 {
            let mut s = 0.0;
            for k in 0..8 {
                s += co[v * 8 + k] * c[k][x];
            }
            tmp[v * 8 + x] = s;
        }
    }
    let mut out = [0f32; 64];
    for x in 0..8 {
        for y in 0..8 {
            let mut s = 0.0;
            for k in 0..8 {
                s += tmp[k * 8 + x] * c[k][y];
            }
            out[y * 8 + x] = s;
        }
    }
    out
}

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55,
    62, 63,
];

/// Quantiser step for coefficient `(u, v)` at scale `q`. A flat-ish ramp:
/// at thumbnail sizes every coefficient is visible after upscaling, so the
/// steep perceptual tables JPEG uses for print-size images over-kill the
/// high frequencies.
#[inline]
fn step(q: u8, zz: usize, chroma: bool) -> f32 {
    let (u, v) = (ZIGZAG[zz] % 8, ZIGZAG[zz] / 8);
    let base = 1.0 + 0.20 * (u + v) as f32;
    let s = q as f32 * base;
    if chroma {
        s * 1.6
    } else {
        s
    }
}

// ── coefficient model ────────────────────────────────────────────────────────

const MAG_CTX: usize = 6;

struct Plane {
    sig: [[Bit; 2]; 64],      // [zigzag pos][prev coefficient was zero]
    last: [Bit; 64],          // "this was the last nonzero" after pos
    mag: [[Bit; MAG_CTX]; 8], // [pos bucket][unary step]
    sign: Bit,
    ac_present: Bit,          // block has any nonzero AC at all
    dc_zero: Bit,
    dc_sign: Bit,
    dc_mag: [Bit; MAG_CTX + 2],
}

impl Plane {
    fn new() -> Plane {
        Plane {
            sig: [[Bit::default(); 2]; 64],
            last: [Bit::default(); 64],
            mag: [[Bit::default(); MAG_CTX]; 8],
            sign: Bit::default(),
            ac_present: Bit::default(),
            dc_zero: Bit::default(),
            dc_sign: Bit::default(),
            dc_mag: [Bit::default(); MAG_CTX + 2],
        }
    }
}

#[inline]
fn bucket(zz: usize) -> usize {
    match zz {
        1 => 0,
        2 => 1,
        3..=5 => 2,
        6..=9 => 3,
        10..=14 => 4,
        15..=20 => 5,
        21..=35 => 6,
        _ => 7,
    }
}

fn encode_block(e: &mut Encoder, m: &mut Plane, q: &[i32; 64], prev_dc: &mut i32) {
    // DC delta
    let d = q[0] - *prev_dc;
    *prev_dc = q[0];
    if d == 0 {
        e.encode(1, &mut m.dc_zero);
    } else {
        e.encode(0, &mut m.dc_zero);
        e.encode((d < 0) as u32, &mut m.dc_sign);
        encode_mag(e, &mut m.dc_mag, d.unsigned_abs() - 1);
    }
    // AC
    let last_nz = (1..64).rev().find(|&i| q[i] != 0);
    let Some(last_nz) = last_nz else {
        e.encode(0, &mut m.ac_present);
        return;
    };
    e.encode(1, &mut m.ac_present);
    let mut prev_zero = 1usize;
    for i in 1..=last_nz {
        let v = q[i];
        if v == 0 {
            e.encode(0, &mut m.sig[i][prev_zero]);
            prev_zero = 1;
            continue;
        }
        e.encode(1, &mut m.sig[i][prev_zero]);
        e.encode((v < 0) as u32, &mut m.sign);
        encode_mag(e, &mut m.mag[bucket(i)], v.unsigned_abs() - 1);
        prev_zero = 0;
        if i < 63 {
            e.encode((i == last_nz) as u32, &mut m.last[i]);
        }
    }
}

fn decode_block(d: &mut Decoder, m: &mut Plane, q: &mut [i32; 64], prev_dc: &mut i32) {
    if d.decode(&mut m.dc_zero) == 1 {
        q[0] = *prev_dc;
    } else {
        let neg = d.decode(&mut m.dc_sign) == 1;
        let mag = decode_mag(d, &mut m.dc_mag).saturating_add(1).min(1 << 20) as i32;
        q[0] = *prev_dc + if neg { -mag } else { mag };
    }
    *prev_dc = q[0];
    if d.decode(&mut m.ac_present) == 0 {
        for c in q.iter_mut().skip(1) {
            *c = 0;
        }
        return;
    }
    let mut prev_zero = 1usize;
    for i in 1..64 {
        if d.decode(&mut m.sig[i][prev_zero]) == 0 {
            q[i] = 0;
            prev_zero = 1;
            continue;
        }
        let neg = d.decode(&mut m.sign) == 1;
        let mag = decode_mag(d, &mut m.mag[bucket(i)]).saturating_add(1).min(1 << 20) as i32;
        q[i] = if neg { -mag } else { mag };
        prev_zero = 0;
        if i < 63 && d.decode(&mut m.last[i]) == 1 {
            for c in q.iter_mut().skip(i + 1) {
                *c = 0;
            }
            return;
        }
    }
}

// ── plane coding ─────────────────────────────────────────────────────────────

fn plane_blocks(px: &[u8], w: u32, h: u32) -> (u32, u32) {
    debug_assert_eq!(px.len(), (w * h) as usize);
    (w.div_ceil(8), h.div_ceil(8))
}

fn load_block(px: &[u8], w: u32, h: u32, bx: u32, by: u32) -> [f32; 64] {
    let mut b = [0f32; 64];
    for y in 0..8 {
        let sy = (by * 8 + y).min(h - 1);
        for x in 0..8 {
            let sx = (bx * 8 + x).min(w - 1);
            b[(y * 8 + x) as usize] = px[(sy * w + sx) as usize] as f32 - 128.0;
        }
    }
    b
}

fn encode_plane(e: &mut Encoder, px: &[u8], w: u32, h: u32, q: u8, chroma: bool) {
    let (bw, bh) = plane_blocks(px, w, h);
    let mut m = Plane::new();
    let mut prev_dc = 0;
    let mut qc = [0i32; 64];
    for by in 0..bh {
        for bx in 0..bw {
            let co = dct8x8(&load_block(px, w, h, bx, by));
            for (zz, qv) in qc.iter_mut().enumerate() {
                *qv = (co[ZIGZAG[zz]] / step(q, zz, chroma)).round() as i32;
            }
            encode_block(e, &mut m, &qc, &mut prev_dc);
        }
    }
}

fn decode_plane(d: &mut Decoder, w: u32, h: u32, q: u8, chroma: bool) -> Vec<u8> {
    let (bw, bh) = (w.div_ceil(8), h.div_ceil(8));
    let mut out = vec![0u8; (w * h) as usize];
    let mut m = Plane::new();
    let mut prev_dc = 0;
    let mut qc = [0i32; 64];
    for by in 0..bh {
        for bx in 0..bw {
            decode_block(d, &mut m, &mut qc, &mut prev_dc);
            let mut co = [0f32; 64];
            for zz in 0..64 {
                co[ZIGZAG[zz]] = qc[zz] as f32 * step(q, zz, chroma);
            }
            let px = idct8x8(&co);
            for y in 0..8 {
                let oy = by * 8 + y;
                if oy >= h {
                    break;
                }
                for x in 0..8 {
                    let ox = bx * 8 + x;
                    if ox >= w {
                        break;
                    }
                    out[(oy * w + ox) as usize] = clamp8(px[(y * 8 + x) as usize] + 128.0);
                }
            }
        }
    }
    out
}

// ── stream ───────────────────────────────────────────────────────────────────

fn put_varint(out: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn get_varint(data: &[u8], pos: &mut usize) -> Result<u32, Error> {
    let mut v = 0u32;
    for shift in [0u32, 7, 14, 21, 28] {
        let b = *data.get(*pos).ok_or(Error::Truncated)?;
        *pos += 1;
        v |= ((b & 0x7f) as u32) << shift;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(Error::BadSize)
}

/// Encode at quantiser scale `q` (1 = finest). Colour input is coded as
/// YCbCr 4:2:0; greyscale input as one plane.
pub fn encode(img: &Image, q: u8) -> Result<Vec<u8>, Error> {
    if q == 0 {
        return Err(Error::BadQ);
    }
    if img.width == 0 || img.height == 0 {
        return Err(Error::BadSize);
    }
    let color = match img.channels {
        1 => false,
        3 => true,
        _ => return Err(Error::BadChannels),
    };
    let mut head = vec![if color { FLAG_COLOR } else { 0 }];
    put_varint(&mut head, img.width);
    put_varint(&mut head, img.height);
    head.push(q);
    let mut e = Encoder::with_prefix(head);
    if !color {
        encode_plane(&mut e, &img.data, img.width, img.height, q, false);
    } else {
        let n = (img.width * img.height) as usize;
        let (mut yp, mut cb, mut cr) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
        for px in img.data.chunks_exact(3) {
            let (y, b, r) = rgb_to_ycc(px[0], px[1], px[2]);
            yp.push(y);
            cb.push(b);
            cr.push(r);
        }
        encode_plane(&mut e, &yp, img.width, img.height, q, false);
        let (cw, ch) = (img.width.div_ceil(2), img.height.div_ceil(2));
        let cbs = Image::gray(img.width, img.height, cb).resize_box(cw, ch);
        let crs = Image::gray(img.width, img.height, cr).resize_box(cw, ch);
        encode_plane(&mut e, &cbs.data, cw, ch, q, true);
        encode_plane(&mut e, &crs.data, cw, ch, q, true);
    }
    Ok(e.finish())
}

/// Decode. `max_pixels` bounds the allocation a hostile header can demand.
pub fn decode(data: &[u8], max_pixels: u64) -> Result<Image, Error> {
    let mut pos = 0usize;
    let flags = *data.first().ok_or(Error::Truncated)?;
    pos += 1;
    if flags >> 1 != 0 {
        return Err(Error::Version(flags >> 1));
    }
    let color = flags & FLAG_COLOR != 0;
    let w = get_varint(data, &mut pos)?;
    let h = get_varint(data, &mut pos)?;
    let q = *data.get(pos).ok_or(Error::Truncated)?;
    pos += 1;
    if q == 0 {
        return Err(Error::BadQ);
    }
    if w == 0 || h == 0 || w as u64 * h as u64 > max_pixels {
        return Err(Error::BadSize);
    }
    let mut d = Decoder::new(&data[pos..]);
    let yp = decode_plane(&mut d, w, h, q, false);
    if !color {
        return Ok(Image::gray(w, h, yp));
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let cb = Image::gray(cw, ch, decode_plane(&mut d, cw, ch, q, true)).resize_bilinear(w, h);
    let cr = Image::gray(cw, ch, decode_plane(&mut d, cw, ch, q, true)).resize_bilinear(w, h);
    let mut out = Vec::with_capacity((w * h * 3) as usize);
    for i in 0..(w * h) as usize {
        let (r, g, b) = ycc_to_rgb(yp[i], cb.data[i], cr.data[i]);
        out.extend_from_slice(&[r, g, b]);
    }
    Ok(Image::rgb(w, h, out))
}

// ── budget fitting ───────────────────────────────────────────────────────────

/// One row of the fitting table.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub width: u32,
    pub height: u32,
    pub q: u8,
    pub bytes: usize,
    /// PSNR (dB) of the decoded result against the source, both resampled to
    /// the reference size, so rows at different resolutions are comparable.
    pub psnr_ref: f64,
    /// PSNR at the candidate's own resolution (how faithful the codec was).
    pub psnr_native: f64,
    pub wire: Vec<u8>,
}

/// Default resolution ladder (long edge).
pub const LADDER: [u32; 10] = [128, 96, 80, 64, 56, 48, 40, 32, 24, 16];

/// Smallest `q` at which `img` encodes to `<= budget` bytes, with the
/// encoding. Size is monotone in `q` in practice; a short downward scan
/// after the binary search guards the rare non-monotone step.
pub fn fit_q(img: &Image, budget: usize) -> Option<(u8, Vec<u8>)> {
    let (mut lo, mut hi) = (1u16, 255u16);
    let mut best: Option<(u8, Vec<u8>)> = None;
    while lo <= hi {
        let mid = ((lo + hi) / 2) as u8;
        let w = encode(img, mid).ok()?;
        if w.len() <= budget {
            best = Some((mid, w));
            if mid == 1 {
                break;
            }
            hi = mid as u16 - 1;
        } else {
            lo = mid as u16 + 1;
        }
    }
    let (q, w) = best?;
    let mut q = q;
    let mut w = w;
    for cand in (q.saturating_sub(3).max(1)..q).rev() {
        let t = encode(img, cand).ok()?;
        if t.len() <= budget {
            q = cand;
            w = t;
        }
    }
    Some((q, w))
}

/// Try every resolution in `ladder` that is not larger than the source,
/// fit `q` for each, and return the rows sorted best-first by `psnr_ref`.
/// `color` = keep colour (input must be RGB); otherwise code luma only.
pub fn fit(src: &Image, budget: usize, ladder: &[u32], color: bool) -> Vec<Candidate> {
    let src = if color { src.clone() } else { src.to_gray() };
    let ref_long = ladder.iter().copied().max().unwrap_or(128).min(src.width.max(src.height));
    let (rw, rh) = src.fit_long_edge(ref_long);
    let reference = src.resize_box(rw, rh);
    let mut rows = Vec::new();
    for &long in ladder {
        if long > src.width.max(src.height) {
            continue;
        }
        let (w, h) = src.fit_long_edge(long);
        let small = src.resize_box(w, h);
        let Some((q, wire)) = fit_q(&small, budget) else { continue };
        let dec = decode(&wire, 1 << 24).expect("own stream decodes");
        let up = dec.resize_bilinear(rw, rh);
        rows.push(Candidate {
            width: w,
            height: h,
            q,
            bytes: wire.len(),
            psnr_ref: psnr(&up, &reference),
            psnr_native: psnr(&dec, &small),
            wire,
        });
    }
    rows.sort_by(|a, b| b.psnr_ref.partial_cmp(&a.psnr_ref).unwrap());
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic "photo": smooth gradient + a disc + a soft edge, so it
    /// has DC, low and mid frequencies.
    fn synth(w: u32, h: u32, channels: u32) -> Image {
        let mut img = Image::new(w, h, channels);
        for y in 0..h {
            for x in 0..w {
                let fx = x as f32 / w as f32;
                let fy = y as f32 / h as f32;
                let mut v = 60.0 + 120.0 * fx + 40.0 * (fy * 6.0).sin();
                let d = ((fx - 0.6).powi(2) + (fy - 0.4).powi(2)).sqrt();
                if d < 0.2 {
                    v += 70.0 * (1.0 - d / 0.2);
                }
                for c in 0..channels {
                    let vv = v + c as f32 * 25.0 * fy;
                    img.data[((y * w + x) * channels + c) as usize] = vv.clamp(0.0, 255.0) as u8;
                }
            }
        }
        img
    }

    #[test]
    fn dct_is_orthonormal_roundtrip() {
        let mut px = [0f32; 64];
        for (i, p) in px.iter_mut().enumerate() {
            *p = ((i * 37) % 251) as f32 - 128.0;
        }
        let back = idct8x8(&dct8x8(&px));
        for i in 0..64 {
            assert!((back[i] - px[i]).abs() < 1e-2, "{i}: {} vs {}", back[i], px[i]);
        }
        // Parseval: energy preserved
        let e1: f32 = px.iter().map(|v| v * v).sum();
        let e2: f32 = dct8x8(&px).iter().map(|v| v * v).sum();
        assert!((e1 - e2).abs() / e1 < 1e-4);
    }

    #[test]
    fn roundtrip_gray_and_color_at_fine_q_is_near_lossless() {
        for ch in [1u32, 3] {
            let img = synth(37, 29, ch); // deliberately not a multiple of 8
            let wire = encode(&img, 1).unwrap();
            let dec = decode(&wire, 1 << 20).unwrap();
            assert_eq!((dec.width, dec.height, dec.channels), (37, 29, ch));
            let p = psnr(&dec, &img);
            assert!(p > 40.0, "channels {ch}: psnr {p}");
        }
    }

    #[test]
    fn coarser_q_is_smaller_and_worse() {
        let img = synth(64, 64, 1);
        let a = encode(&img, 2).unwrap();
        let b = encode(&img, 12).unwrap();
        let c = encode(&img, 60).unwrap();
        assert!(a.len() > b.len() && b.len() > c.len(), "{} {} {}", a.len(), b.len(), c.len());
        let pa = psnr(&decode(&a, 1 << 20).unwrap(), &img);
        let pc = psnr(&decode(&c, 1 << 20).unwrap(), &img);
        assert!(pa > pc + 6.0, "{pa} vs {pc}");
    }

    #[test]
    fn a_64x64_gray_photo_fits_in_752_bytes_at_over_30db() {
        let img = synth(64, 64, 1);
        let (q, wire) = fit_q(&img, 752).unwrap();
        assert!(wire.len() <= 752);
        let dec = decode(&wire, 1 << 20).unwrap();
        let p = psnr(&dec, &img);
        assert!(p > 30.0, "q {q}, {} bytes, psnr {p}", wire.len());
        // tightness: q-1 would not fit (or q is already 1)
        if q > 1 {
            assert!(encode(&img, q - 1).unwrap().len() > 752);
        }
    }

    #[test]
    fn fit_table_is_sorted_and_every_row_is_under_budget() {
        let img = synth(300, 200, 3);
        let rows = fit(&img, 504, &LADDER, true);
        assert!(rows.len() >= 5, "{}", rows.len());
        for r in &rows {
            assert!(r.bytes <= 504);
            assert_eq!(decode(&r.wire, 1 << 20).unwrap().width, r.width);
        }
        for w in rows.windows(2) {
            assert!(w[0].psnr_ref >= w[1].psnr_ref);
        }
        // and the ladder never exceeds the source
        assert!(rows.iter().all(|r| r.width <= 300 && r.height <= 200));
    }

    #[test]
    fn decode_rejects_garbage_without_panicking() {
        assert_eq!(decode(&[], 1 << 20), Err(Error::Truncated));
        assert_eq!(decode(&[0x02, 8, 8, 4], 1 << 20), Err(Error::Version(1)));
        assert_eq!(decode(&[0x00, 0, 8, 4], 1 << 20), Err(Error::BadSize));
        assert_eq!(decode(&[0x00, 8, 8, 0], 1 << 20), Err(Error::BadQ));
        assert_eq!(decode(&[0x00, 0xff, 0xff, 0x03, 0xff, 0xff, 0x03, 4], 1 << 20), Err(Error::BadSize));
        // random bodies with a valid header must decode to SOMETHING of the right size
        let mut s = 0x2545F491u32;
        for _ in 0..50 {
            let mut body = vec![0x00, 16, 16, 8];
            for _ in 0..(s % 60) {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                body.push((s >> 24) as u8);
            }
            let dec = decode(&body, 1 << 20).unwrap();
            assert_eq!((dec.width, dec.height), (16, 16));
        }
    }

    #[test]
    fn resize_box_averages_and_bilinear_interpolates() {
        let mut img = Image::new(4, 2, 1);
        img.data = vec![0, 0, 200, 200, 0, 0, 200, 200];
        let s = img.resize_box(2, 1);
        assert_eq!(s.data, vec![0, 200]);
        let up = s.resize_bilinear(4, 1);
        assert!(up.data[0] < up.data[1] && up.data[1] < up.data[2] && up.data[2] < up.data[3], "{:?}", up.data);
    }
}
