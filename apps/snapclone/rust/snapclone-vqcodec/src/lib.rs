//! `snapclone-vqcodec` — shared-codebook (vector-quantization) luma image codec.
//!
//! The core is `std`-only: it operates on raw luma planes ([`GrayImage`]), so
//! PNG/JPEG decode stays at the edges (the app's media layer). Both sides of the
//! network hold the same [`Codebook`]; a snap is then transmitted as one small
//! index per block (plus an optional residual — future) and rebuilt locally.
//! That collapses the per-snap payload far below a raw/JPEG frame — the
//! foundation for pushing images through LoRa-radio bytes-per-second or
//! opportunistic Bluetooth sneakernet.
//!
//! This is **layer 1** (generic block VQ). **Layer 2** (face keypoint "motion
//! factors" + a shared per-person reference) builds on `snapclone-face` once
//! landmark extraction exists; the decoder is swappable (a dumb warp ↔ a learned
//! backbone) without touching the wire format.

#![deny(unsafe_code)] // was forbid: the C FFI module (src/ffi.rs) narrowly
                      // re-allows unsafe for the boundary. deny permits that
                      // override; forbid would not. Mirrors snapclone-face.

pub mod ffi;

/// Single-channel (luma) image. Row-major; `data.len() == width * height`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrayImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl GrayImage {
    /// Panics if `data.len() != width * height`.
    pub fn new(width: usize, height: usize, data: Vec<u8>) -> Self {
        assert_eq!(data.len(), width * height, "data length must equal width*height");
        Self { width, height, data }
    }
}

/// The shared lookup table: `centroids.len()` blocks, each `block*block` samples.
/// Centroids are kept as `f32` in memory (training/encoding stays exact); they
/// serialize to one byte per sample (see [`Codebook::to_bytes`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Codebook {
    pub block: usize,
    pub centroids: Vec<Vec<f32>>,
}

/// A VQ-encoded frame: the cropped block-grid dimensions + one index per block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Encoded {
    /// Cropped width (a multiple of `block`).
    pub width: usize,
    /// Cropped height (a multiple of `block`).
    pub height: usize,
    pub block: usize,
    pub indices: Vec<u16>,
}

impl Encoded {
    /// Size of the index stream alone, in bytes, at `bits` per index. (The
    /// codebook syncs out of band and is amortized across many snaps.)
    pub fn index_payload_bytes(&self, bits: usize) -> usize {
        (self.indices.len() * bits + 7) / 8
    }

    /// Serialize to a compact, self-describing blob. Layout: `"SCVE"` magic,
    /// version=2, block (u8), bits-per-index (u8), width (LE u32), height
    /// (LE u32), index count (LE u32), then the indices bit-packed LSB-first at
    /// `bits` bits each. `bits` is the minimum that holds the largest index
    /// actually present (≤ the codebook's `index_bits`), so the index stream is
    /// ~`bits/16` the size of a naive u16 array — the whole point on a
    /// LoRa/Bluetooth-budget link. This is the per-frame transmitted payload.
    pub fn to_bytes(&self) -> Vec<u8> {
        let max_idx = self.indices.iter().copied().max().unwrap_or(0);
        let bits = if max_idx == 0 { 0 } else { 16 - max_idx.leading_zeros() as usize };
        let payload = (self.indices.len() * bits + 7) / 8;
        let mut out = Vec::with_capacity(19 + payload);
        out.extend_from_slice(b"SCVE");
        out.push(2); // version 2: bit-packed indices
        out.push(self.block as u8);
        out.push(bits as u8);
        out.extend_from_slice(&(self.width as u32).to_le_bytes());
        out.extend_from_slice(&(self.height as u32).to_le_bytes());
        out.extend_from_slice(&(self.indices.len() as u32).to_le_bytes());
        if bits > 0 {
            let mut acc: u64 = 0;
            let mut nbits = 0usize;
            for &idx in &self.indices {
                acc |= (idx as u64) << nbits;
                nbits += bits;
                while nbits >= 8 {
                    out.push((acc & 0xFF) as u8);
                    acc >>= 8;
                    nbits -= 8;
                }
            }
            if nbits > 0 {
                out.push((acc & 0xFF) as u8);
            }
        }
        out
    }

    /// Parse a blob produced by [`Encoded::to_bytes`]. `None` on bad magic,
    /// unknown version, or truncation.
    pub fn from_bytes(data: &[u8]) -> Option<Encoded> {
        if data.len() < 19 || &data[0..4] != b"SCVE" || data[4] != 2 {
            return None;
        }
        let block = data[5] as usize;
        let bits = data[6] as usize;
        let width = u32::from_le_bytes([data[7], data[8], data[9], data[10]]) as usize;
        let height = u32::from_le_bytes([data[11], data[12], data[13], data[14]]) as usize;
        let count = u32::from_le_bytes([data[15], data[16], data[17], data[18]]) as usize;
        if block == 0 || bits > 16 {
            return None;
        }
        let payload = (count.checked_mul(bits)? + 7) / 8;
        let need = 19usize.checked_add(payload)?;
        if data.len() < need {
            return None;
        }
        let mut indices = Vec::with_capacity(count);
        if bits == 0 {
            indices.resize(count, 0u16);
        } else {
            let mask: u64 = (1u64 << bits) - 1;
            let bytes = &data[19..need];
            let mut acc: u64 = 0;
            let mut nbits = 0usize;
            let mut p = 0usize;
            for _ in 0..count {
                while nbits < bits {
                    acc |= (bytes[p] as u64) << nbits;
                    p += 1;
                    nbits += 8;
                }
                indices.push((acc & mask) as u16);
                acc >>= bits;
                nbits -= bits;
            }
        }
        Some(Encoded { width, height, block, indices })
    }
}

// ----------------------------------------------------------------- internals --

#[inline]
fn dist2(a: &[f32], b: &[f32]) -> f32 {
    let mut d = 0.0f32;
    for i in 0..a.len() {
        let e = a[i] - b[i];
        d += e * e;
    }
    d
}

fn nearest(blk: &[f32], centroids: &[Vec<f32>]) -> (usize, f32) {
    let mut best = 0usize;
    let mut best_d = f32::INFINITY;
    for (j, c) in centroids.iter().enumerate() {
        let d = dist2(blk, c);
        if d < best_d {
            best_d = d;
            best = j;
        }
    }
    (best, best_d)
}

/// Extract `block`x`block` luma blocks in raster order, cropping the image to a
/// multiple of `block`. Returns the blocks plus the cropped `(w, h)`.
fn blockify(img: &GrayImage, block: usize) -> (Vec<Vec<f32>>, usize, usize) {
    assert!(block >= 1, "block size must be >= 1");
    let wc = img.width - img.width % block;
    let hc = img.height - img.height % block;
    let bx = wc / block;
    let by = hc / block;
    let mut blocks = Vec::with_capacity(bx * by);
    for byi in 0..by {
        for bxi in 0..bx {
            let mut blk = Vec::with_capacity(block * block);
            for r in 0..block {
                let y = byi * block + r;
                let base = y * img.width + bxi * block;
                for c in 0..block {
                    blk.push(img.data[base + c] as f32);
                }
            }
            blocks.push(blk);
        }
    }
    (blocks, wc, hc)
}

/// Tiny deterministic PRNG (SplitMix64) — keeps the crate `std`-only and makes
/// training reproducible for a given seed.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_add(0x9E37_79B9_7F4A_7C15))
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

// ---------------------------------------------------------------- public API --

/// Train a codebook of `k` entries from one or more images (the recurring blocks
/// across all of them — "common blocks across multiple snaps"), via Lloyd/LBG
/// k-means for `iters` iterations. Dead (empty) entries are reseeded to the
/// worst-fit block so the table stays full.
///
/// Panics if `images` is empty or yields no blocks at the given `block` size.
pub fn train_codebook(
    images: &[GrayImage],
    block: usize,
    k: usize,
    iters: usize,
    seed: u64,
) -> Codebook {
    let mut blocks: Vec<Vec<f32>> = Vec::new();
    for img in images {
        blocks.extend(blockify(img, block).0);
    }
    assert!(!blocks.is_empty(), "no blocks to train on");
    let dim = block * block;
    let k = k.min(blocks.len()).max(1);

    let mut rng = Rng::new(seed);
    let mut centroids: Vec<Vec<f32>> =
        (0..k).map(|_| blocks[rng.below(blocks.len())].clone()).collect();

    for _ in 0..iters {
        let mut sums = vec![vec![0.0f32; dim]; k];
        let mut counts = vec![0usize; k];
        let mut worst_d = -1.0f32;
        let mut worst_blk = 0usize;
        for (bi, blk) in blocks.iter().enumerate() {
            let (j, d) = nearest(blk, &centroids);
            let s = &mut sums[j];
            for t in 0..dim {
                s[t] += blk[t];
            }
            counts[j] += 1;
            if d > worst_d {
                worst_d = d;
                worst_blk = bi;
            }
        }
        for j in 0..k {
            if counts[j] > 0 {
                let inv = 1.0 / counts[j] as f32;
                for t in 0..dim {
                    centroids[j][t] = sums[j][t] * inv;
                }
            } else {
                centroids[j] = blocks[worst_blk].clone();
            }
        }
    }
    Codebook { block, centroids }
}

/// Encode an image against a codebook: one nearest-centroid index per block.
pub fn encode(img: &GrayImage, cb: &Codebook) -> Encoded {
    let (blocks, wc, hc) = blockify(img, cb.block);
    let indices = blocks.iter().map(|b| nearest(b, &cb.centroids).0 as u16).collect();
    Encoded { width: wc, height: hc, block: cb.block, indices }
}

/// Decode an encoded frame back to luma by looking up each block.
pub fn decode(enc: &Encoded, cb: &Codebook) -> GrayImage {
    let b = enc.block;
    let bx = enc.width / b;
    let mut data = vec![0u8; enc.width * enc.height];
    for (i, &idx) in enc.indices.iter().enumerate() {
        let c = &cb.centroids[idx as usize];
        let bxi = i % bx;
        let byi = i / bx;
        for r in 0..b {
            let y = byi * b + r;
            let base = y * enc.width + bxi * b;
            for col in 0..b {
                let v = c[r * b + col].round();
                data[base + col] = if v < 0.0 {
                    0
                } else if v > 255.0 {
                    255
                } else {
                    v as u8
                };
            }
        }
    }
    GrayImage { width: enc.width, height: enc.height, data }
}

/// PSNR (dB) between two equally-sized luma images. `INFINITY` if identical.
pub fn psnr(a: &GrayImage, b: &GrayImage) -> f64 {
    assert_eq!(a.data.len(), b.data.len(), "images must be the same size");
    let n = a.data.len() as f64;
    let mse: f64 = a
        .data
        .iter()
        .zip(b.data.iter())
        .map(|(&x, &y)| {
            let e = x as f64 - y as f64;
            e * e
        })
        .sum::<f64>()
        / n;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        20.0 * 255.0f64.log10() - 10.0 * mse.log10()
    }
}

impl Codebook {
    /// Bits needed to address one entry (the per-block index size): `ceil(log2(k))`, min 1.
    pub fn index_bits(&self) -> usize {
        let k = self.centroids.len().max(1);
        let mut bits = 0usize;
        let mut v = k - 1;
        while v > 0 {
            bits += 1;
            v >>= 1;
        }
        bits.max(1)
    }

    /// Serialize to a compact, self-describing blob (the shareable table):
    /// `"SCVQ"` magic, version, block size, entry count (LE `u16`), then one byte
    /// per centroid sample. This is the form that syncs to both sides.
    pub fn to_bytes(&self) -> Vec<u8> {
        let dim = self.block * self.block;
        let mut out = Vec::with_capacity(8 + self.centroids.len() * dim);
        out.extend_from_slice(b"SCVQ");
        out.push(1); // version
        out.push(self.block as u8);
        out.extend_from_slice(&(self.centroids.len() as u16).to_le_bytes());
        for c in &self.centroids {
            for &v in c {
                let r = v.round();
                out.push(if r < 0.0 {
                    0
                } else if r > 255.0 {
                    255
                } else {
                    r as u8
                });
            }
        }
        out
    }

    /// Parse a blob produced by [`Codebook::to_bytes`]. Returns `None` on a bad
    /// magic / truncated input. Centroids come back as `f32` from stored bytes.
    pub fn from_bytes(data: &[u8]) -> Option<Codebook> {
        if data.len() < 8 || &data[0..4] != b"SCVQ" {
            return None;
        }
        let _version = data[4];
        let block = data[5] as usize;
        let k = u16::from_le_bytes([data[6], data[7]]) as usize;
        if block == 0 {
            return None;
        }
        let dim = block.checked_mul(block)?;
        let need = 8usize.checked_add(k.checked_mul(dim)?)?;
        if data.len() < need {
            return None;
        }
        let mut centroids = Vec::with_capacity(k);
        let mut p = 8;
        for _ in 0..k {
            let mut c = Vec::with_capacity(dim);
            for _ in 0..dim {
                c.push(data[p] as f32);
                p += 1;
            }
            centroids.push(c);
        }
        Some(Codebook { block, centroids })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic textured pattern so codebook size actually matters.
    fn synth(w: usize, h: usize) -> GrayImage {
        let mut data = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let v = (x.wrapping_mul(37) ^ y.wrapping_mul(101)).wrapping_add((x * y) / 3);
                data[y * w + x] = (v & 0xFF) as u8;
            }
        }
        GrayImage::new(w, h, data)
    }

    #[test]
    fn round_trip_dimensions_and_quality() {
        let img = synth(64, 64);
        let cb = train_codebook(&[img.clone()], 4, 128, 20, 7);
        let enc = encode(&img, &cb);
        let out = decode(&enc, &cb);
        assert_eq!((out.width, out.height), (64, 64));
        assert_eq!(enc.indices.len(), (64 / 4) * (64 / 4));
        let q = psnr(&img, &out);
        assert!(q > 18.0, "PSNR too low: {q}");
    }

    #[test]
    fn more_entries_never_worse() {
        let img = synth(64, 64);
        let small = train_codebook(&[img.clone()], 4, 16, 25, 1);
        let large = train_codebook(&[img.clone()], 4, 200, 25, 1);
        let qs = psnr(&img, &decode(&encode(&img, &small), &small));
        let ql = psnr(&img, &decode(&encode(&img, &large), &large));
        assert!(ql >= qs - 0.01, "larger codebook worse: {ql} < {qs}");
    }

    #[test]
    fn codebook_serialization_round_trips() {
        let img = synth(48, 48);
        let cb = train_codebook(&[img], 4, 64, 15, 3);
        let bytes = cb.to_bytes();
        let cb2 = Codebook::from_bytes(&bytes).expect("parse");
        assert_eq!(cb2.block, cb.block);
        assert_eq!(cb2.centroids.len(), cb.centroids.len());
        // Equal once both are at the stored byte precision.
        assert_eq!(cb2.to_bytes(), bytes);
    }

    #[test]
    fn from_bytes_rejects_garbage() {
        assert!(Codebook::from_bytes(b"nope").is_none());
        assert!(Codebook::from_bytes(&[]).is_none());
        assert!(Codebook::from_bytes(b"SCVQ\x01\x04\xff\xff").is_none()); // claims 65535 entries
    }

    #[test]
    fn training_is_deterministic_for_a_seed() {
        let img = synth(64, 64);
        let a = train_codebook(&[img.clone()], 4, 64, 20, 42);
        let b = train_codebook(&[img], 4, 64, 20, 42);
        assert_eq!(a.centroids, b.centroids);
    }

    #[test]
    fn works_for_block_eight() {
        let img = synth(80, 80);
        let cb = train_codebook(&[img.clone()], 8, 128, 20, 5);
        let enc = encode(&img, &cb);
        assert_eq!(enc.block, 8);
        let out = decode(&enc, &cb);
        assert_eq!((out.width, out.height), (80, 80));
        assert!(psnr(&img, &out).is_finite());
    }

    #[test]
    fn index_bits_is_ceil_log2() {
        let mk = |k: usize| Codebook { block: 4, centroids: vec![vec![0.0; 16]; k] };
        assert_eq!(mk(256).index_bits(), 8);
        assert_eq!(mk(255).index_bits(), 8);
        assert_eq!(mk(257).index_bits(), 9);
        assert_eq!(mk(2).index_bits(), 1);
        assert_eq!(mk(1).index_bits(), 1);
    }

    #[test]
    fn crop_to_multiple_of_block() {
        // 66x66 with block 4 -> cropped to 64x64.
        let img = synth(66, 66);
        let cb = train_codebook(&[img.clone()], 4, 32, 10, 9);
        let enc = encode(&img, &cb);
        assert_eq!((enc.width, enc.height), (64, 64));
    }

    #[test]
    fn encoded_serialization_round_trips() {
        let img = synth(64, 64);
        let cb = train_codebook(&[img.clone()], 4, 64, 15, 2);
        let enc = encode(&img, &cb);
        let bytes = enc.to_bytes();
        // bit-packed: must be smaller than a naive 2-bytes-per-index array.
        assert!(bytes.len() < 18 + enc.indices.len() * 2);
        let enc2 = Encoded::from_bytes(&bytes).expect("parse");
        assert_eq!(enc2, enc);
        assert_eq!(decode(&enc2, &cb), decode(&enc, &cb));
        assert!(Encoded::from_bytes(b"nope").is_none());
        assert!(Encoded::from_bytes(&[]).is_none());
    }
}
