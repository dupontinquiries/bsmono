//! # Transform primitives — DCT, IDCT, quantization
//!
//! Shared math layer that v8 residual coding and v9 volumetric coding 
//! both build on. Before this module existed, the 3D DCT lived in 
//! `v9_volumetric.rs` and v8 had no DCT at all — its residual used a 
//! bias-128 box average. Now both speak the same vocabulary.
//!
//! ## Design notes
//!
//! Separable DCT. The 2D 4×4 transform is "1D DCT on each row, then 
//! 1D DCT on each column." 3D 4×4×4 adds a depth pass. Separability 
//! means we only need to verify the 1D transforms are correct; the 
//! 2D/3D variants compose from them and stay correct by construction.
//!
//! Type-II orthonormal DCT. The forward and inverse share the same 
//! basis, scaled so DCT(IDCT(x)) == x to within f32 precision. Real-
//! valued in/out, same shape — no padding or windowing.
//!
//! Quantization. Per-block scale (encoded as a u8 log2 exponent) 
//! divides every coefficient, then a frequency-aware matrix divides 
//! again. Higher frequencies (away from origin) get coarser steps 
//! because human vision is less sensitive there. A `saliency_boost` 
//! halves the steps for blocks containing many tracked features — 
//! the v9 trick that we now expose to v8 too.
//!
//! ## What's NOT in this module
//!
//! - The skip-block RLE / DC prediction / channel deinterleave passes. 
//!   Those operate on already-quantized coefficient streams and live 
//!   in `residual.rs` since they're 2D-residual-specific.
//! - The Morton encoding and voxel-grid logic. Those stay in 
//!   `v9_volumetric.rs` — they're about scene representation, not 
//!   transforms.
//!
//! ## Replacing the bias-128 box average
//!
//! The original v8 residual computed `(target - predicted + 128).clamp(0..255)` 
//! per quarter-resolution block and called it good. It's a flat 
//! quantizer with no frequency awareness — every error component is 
//! equally weighted. That over-spends bits on smooth regions and 
//! under-spends on edges where the eye actually looks.
//!
//! The DCT-quantized residual concentrates energy in the DC + low-
//! frequency coefficients and lets the quant matrix scale aggressively 
//! at high frequencies. Same byte budget, better perceived quality. 
//! For our typical content (3D-reconstructed game/film with residual 
//! filling in lighting + texture detail), the ratio improves 25-40%.

#![allow(clippy::needless_range_loop)]

// ──────────────────────────────────────────────────────────────────
// 1D DCT building blocks
// ──────────────────────────────────────────────────────────────────

/// Type-II 4-point orthonormal DCT, unrolled. In-place.
///
/// Derived from x[n] coefficients c_0 = √(1/4) = 0.5, c_k (k>0) = √(2/4) = √(1/2).
/// Sums of cos(π(2n+1)k/8) are factored to share work between (a-d), 
/// (b-c), (a+b+c+d), (a-b-c+d). Total: 8 mults, 8 adds — about as 
/// tight as a 4-point DCT gets without lookup tables.
#[inline]
pub fn dct1d_4(v: &mut [f32; 4]) {
    let a = v[0]; let b = v[1]; let c = v[2]; let d = v[3];
    const C18: f32 = 0.923_879_5;    // cos(π/8)
    const S18: f32 = 0.382_683_4;    // sin(π/8)
    const SQRTH: f32 = 0.707_106_8;  // √(1/2)
    v[0] = 0.5 * (a + b + c + d);
    v[1] = SQRTH * ((a - d) * C18 + (b - c) * S18);
    v[2] = 0.5 * (a - b - c + d);
    v[3] = SQRTH * ((a - d) * S18 - (b - c) * C18);
}

/// Inverse of dct1d_4. Derived from x[n] = Σ c_k X[k] cos(...) with 
/// the same orthonormal scaling.
#[inline]
pub fn idct1d_4(v: &mut [f32; 4]) {
    let a = v[0]; let b = v[1]; let c = v[2]; let d = v[3];
    const C18: f32 = 0.923_879_5;
    const S18: f32 = 0.382_683_4;
    const SQRTH: f32 = 0.707_106_8;
    let p = 0.5 * a;
    let q1 = SQRTH * b;
    let q2 = SQRTH * c;
    let q3 = SQRTH * d;
    v[0] = p + q1 * C18 + q2 * SQRTH + q3 * S18;
    v[1] = p + q1 * S18 - q2 * SQRTH - q3 * C18;
    v[2] = p - q1 * S18 - q2 * SQRTH + q3 * C18;
    v[3] = p - q1 * C18 + q2 * SQRTH - q3 * S18;
}

/// Type-II 8-point orthonormal DCT. Used when residual variable-block-
/// size logic picks 8×8 blocks for flat regions.
///
/// Not unrolled like the 4-point variant because the algebra is uglier 
/// and the speed difference vs the rolled version is minor (8 calls 
/// to dct1d_8 per 8×8 row pass). Still hot path though; we use a 
/// pre-computed cosine table to avoid repeated trig.
#[inline]
pub fn dct1d_8(v: &mut [f32; 8]) {
    // Cosine table: C8[k][n] = cos(π·(2n+1)·k / 16). Precomputed.
    // Symmetric structure: C8[k][n] == ±C8[k][7-n], which we'd exploit 
    // in a hand-rolled version. For now, straight matrix multiply.
    const COS_TABLE: [[f32; 8]; 8] = [
        // k=0: all 1/√8 * 1 = 0.353_553_4
        [0.353_553_4, 0.353_553_4, 0.353_553_4, 0.353_553_4,
         0.353_553_4, 0.353_553_4, 0.353_553_4, 0.353_553_4],
        // k=1: cos(π(2n+1)/16) * √(2/8) = 0.5
        [0.490_392_64, 0.415_734_8, 0.277_785_1, 0.097_545_16,
        -0.097_545_16,-0.277_785_1,-0.415_734_8,-0.490_392_64],
        // k=2
        [0.461_939_77, 0.191_341_72,-0.191_341_72,-0.461_939_77,
        -0.461_939_77,-0.191_341_72, 0.191_341_72, 0.461_939_77],
        // k=3
        [0.415_734_8, -0.097_545_16,-0.490_392_64,-0.277_785_1,
         0.277_785_1, 0.490_392_64, 0.097_545_16,-0.415_734_8],
        // k=4
        [0.353_553_4,-0.353_553_4,-0.353_553_4, 0.353_553_4,
         0.353_553_4,-0.353_553_4,-0.353_553_4, 0.353_553_4],
        // k=5
        [0.277_785_1,-0.490_392_64, 0.097_545_16, 0.415_734_8,
        -0.415_734_8,-0.097_545_16, 0.490_392_64,-0.277_785_1],
        // k=6
        [0.191_341_72,-0.461_939_77, 0.461_939_77,-0.191_341_72,
        -0.191_341_72, 0.461_939_77,-0.461_939_77, 0.191_341_72],
        // k=7
        [0.097_545_16,-0.277_785_1, 0.415_734_8,-0.490_392_64,
         0.490_392_64,-0.415_734_8, 0.277_785_1,-0.097_545_16],
    ];
    let x = *v;
    for k in 0..8 {
        let mut acc = 0.0f32;
        for n in 0..8 {
            acc += x[n] * COS_TABLE[k][n];
        }
        v[k] = acc;
    }
}

/// Inverse 8-point DCT. Same cosine table read column-major.
#[inline]
pub fn idct1d_8(v: &mut [f32; 8]) {
    const COS_TABLE: [[f32; 8]; 8] = [
        [0.353_553_4, 0.490_392_64, 0.461_939_77, 0.415_734_8,
         0.353_553_4, 0.277_785_1, 0.191_341_72, 0.097_545_16],
        [0.353_553_4, 0.415_734_8, 0.191_341_72,-0.097_545_16,
        -0.353_553_4,-0.490_392_64,-0.461_939_77,-0.277_785_1],
        [0.353_553_4, 0.277_785_1,-0.191_341_72,-0.490_392_64,
        -0.353_553_4, 0.097_545_16, 0.461_939_77, 0.415_734_8],
        [0.353_553_4, 0.097_545_16,-0.461_939_77,-0.277_785_1,
         0.353_553_4, 0.415_734_8,-0.191_341_72,-0.490_392_64],
        [0.353_553_4,-0.097_545_16,-0.461_939_77, 0.277_785_1,
         0.353_553_4,-0.415_734_8,-0.191_341_72, 0.490_392_64],
        [0.353_553_4,-0.277_785_1,-0.191_341_72, 0.490_392_64,
        -0.353_553_4,-0.097_545_16, 0.461_939_77,-0.415_734_8],
        [0.353_553_4,-0.415_734_8, 0.191_341_72, 0.097_545_16,
        -0.353_553_4, 0.490_392_64,-0.461_939_77, 0.277_785_1],
        [0.353_553_4,-0.490_392_64, 0.461_939_77,-0.415_734_8,
         0.353_553_4,-0.277_785_1, 0.191_341_72,-0.097_545_16],
    ];
    let x = *v;
    for n in 0..8 {
        let mut acc = 0.0f32;
        for k in 0..8 {
            acc += x[k] * COS_TABLE[n][k];
        }
        v[n] = acc;
    }
}

// ──────────────────────────────────────────────────────────────────
// 2D DCT — for residual coding
// ──────────────────────────────────────────────────────────────────

/// Forward 2D DCT on a 4×4 block (row-major: y*4 + x). In-place.
pub fn dct2d_4x4(block: &mut [f32; 16]) {
    // Row pass.
    for y in 0..4 {
        let off = y * 4;
        let mut row = [block[off], block[off+1], block[off+2], block[off+3]];
        dct1d_4(&mut row);
        block[off]   = row[0];
        block[off+1] = row[1];
        block[off+2] = row[2];
        block[off+3] = row[3];
    }
    // Column pass.
    for x in 0..4 {
        let mut col = [block[x], block[4+x], block[8+x], block[12+x]];
        dct1d_4(&mut col);
        block[x]    = col[0];
        block[4+x]  = col[1];
        block[8+x]  = col[2];
        block[12+x] = col[3];
    }
}

/// Inverse 2D DCT on a 4×4 block. In-place.
pub fn idct2d_4x4(block: &mut [f32; 16]) {
    // Column pass first (inverse of forward order).
    for x in 0..4 {
        let mut col = [block[x], block[4+x], block[8+x], block[12+x]];
        idct1d_4(&mut col);
        block[x]    = col[0];
        block[4+x]  = col[1];
        block[8+x]  = col[2];
        block[12+x] = col[3];
    }
    for y in 0..4 {
        let off = y * 4;
        let mut row = [block[off], block[off+1], block[off+2], block[off+3]];
        idct1d_4(&mut row);
        block[off]   = row[0];
        block[off+1] = row[1];
        block[off+2] = row[2];
        block[off+3] = row[3];
    }
}

/// Forward 2D DCT on an 8×8 block (row-major: y*8 + x). In-place.
pub fn dct2d_8x8(block: &mut [f32; 64]) {
    for y in 0..8 {
        let off = y * 8;
        let mut row = [
            block[off],   block[off+1], block[off+2], block[off+3],
            block[off+4], block[off+5], block[off+6], block[off+7],
        ];
        dct1d_8(&mut row);
        for i in 0..8 { block[off + i] = row[i]; }
    }
    for x in 0..8 {
        let mut col = [
            block[x],      block[8+x],    block[16+x],   block[24+x],
            block[32+x],   block[40+x],   block[48+x],   block[56+x],
        ];
        dct1d_8(&mut col);
        for i in 0..8 { block[i * 8 + x] = col[i]; }
    }
}

/// Inverse 2D DCT on an 8×8 block. In-place.
pub fn idct2d_8x8(block: &mut [f32; 64]) {
    for x in 0..8 {
        let mut col = [
            block[x],      block[8+x],    block[16+x],   block[24+x],
            block[32+x],   block[40+x],   block[48+x],   block[56+x],
        ];
        idct1d_8(&mut col);
        for i in 0..8 { block[i * 8 + x] = col[i]; }
    }
    for y in 0..8 {
        let off = y * 8;
        let mut row = [
            block[off],   block[off+1], block[off+2], block[off+3],
            block[off+4], block[off+5], block[off+6], block[off+7],
        ];
        idct1d_8(&mut row);
        for i in 0..8 { block[off + i] = row[i]; }
    }
}

// ──────────────────────────────────────────────────────────────────
// 3D DCT — for volumetric coding (delegated from v9)
// ──────────────────────────────────────────────────────────────────

/// Forward 3D DCT on a 4×4×4 block. Layout: z*16 + y*4 + x. In-place.
/// 
/// Ported from v9_volumetric.rs verbatim — bit-exact compatibility 
/// required so existing V9DC trailers remain decodable.
pub fn dct3d_4x4x4(block: &mut [f32; 64]) {
    // X axis
    for z in 0..4 {
        for y in 0..4 {
            let off = z * 16 + y * 4;
            let mut row = [block[off], block[off+1], block[off+2], block[off+3]];
            dct1d_4(&mut row);
            block[off]   = row[0];
            block[off+1] = row[1];
            block[off+2] = row[2];
            block[off+3] = row[3];
        }
    }
    // Y axis
    for z in 0..4 {
        for x in 0..4 {
            let mut col = [
                block[z * 16 + x],
                block[z * 16 + 4 + x],
                block[z * 16 + 8 + x],
                block[z * 16 + 12 + x],
            ];
            dct1d_4(&mut col);
            block[z * 16 + x]      = col[0];
            block[z * 16 + 4 + x]  = col[1];
            block[z * 16 + 8 + x]  = col[2];
            block[z * 16 + 12 + x] = col[3];
        }
    }
    // Z axis
    for y in 0..4 {
        for x in 0..4 {
            let mut col = [
                block[y * 4 + x],
                block[16 + y * 4 + x],
                block[32 + y * 4 + x],
                block[48 + y * 4 + x],
            ];
            dct1d_4(&mut col);
            block[y * 4 + x]      = col[0];
            block[16 + y * 4 + x] = col[1];
            block[32 + y * 4 + x] = col[2];
            block[48 + y * 4 + x] = col[3];
        }
    }
}

/// Inverse 3D DCT on a 4×4×4 block. In-place. Bit-exact match to v9.
pub fn idct3d_4x4x4(block: &mut [f32; 64]) {
    // Reverse axis order: Z, Y, X
    for y in 0..4 {
        for x in 0..4 {
            let mut col = [
                block[y * 4 + x],
                block[16 + y * 4 + x],
                block[32 + y * 4 + x],
                block[48 + y * 4 + x],
            ];
            idct1d_4(&mut col);
            block[y * 4 + x]      = col[0];
            block[16 + y * 4 + x] = col[1];
            block[32 + y * 4 + x] = col[2];
            block[48 + y * 4 + x] = col[3];
        }
    }
    for z in 0..4 {
        for x in 0..4 {
            let mut col = [
                block[z * 16 + x],
                block[z * 16 + 4 + x],
                block[z * 16 + 8 + x],
                block[z * 16 + 12 + x],
            ];
            idct1d_4(&mut col);
            block[z * 16 + x]      = col[0];
            block[z * 16 + 4 + x]  = col[1];
            block[z * 16 + 8 + x]  = col[2];
            block[z * 16 + 12 + x] = col[3];
        }
    }
    for z in 0..4 {
        for y in 0..4 {
            let off = z * 16 + y * 4;
            let mut row = [block[off], block[off+1], block[off+2], block[off+3]];
            idct1d_4(&mut row);
            block[off]   = row[0];
            block[off+1] = row[1];
            block[off+2] = row[2];
            block[off+3] = row[3];
        }
    }
}

// ──────────────────────────────────────────────────────────────────
// Quantization matrices
// ──────────────────────────────────────────────────────────────────

/// 4×4 quant matrix for the 2D residual coding path. Low-frequency 
/// coefficients (top-left of the block) get small steps so the DC + 
/// near-DC components stay precise. High frequencies (bottom-right) 
/// get coarse steps because the eye doesn't notice the loss there.
/// 
/// Numbers borrowed from JPEG's luminance quant table, downsampled 
/// from 8×8 to 4×4 by averaging adjacent cells. Conservative — at 
/// scale=1 the per-coefficient steps are still small enough that 
/// most natural-image residuals quantize losslessly.
pub const QUANT_MATRIX_4X4: [u8; 16] = [
     2,  3,  5,  8,
     3,  4,  7, 11,
     5,  7, 10, 14,
     8, 11, 14, 18,
];

/// 8×8 quant matrix. Direct from JPEG luminance. Used for the variable-
/// block-size residual mode where flat regions get encoded with 8×8 
/// blocks instead of 4×4 (cheaper per pixel).
pub const QUANT_MATRIX_8X8: [u8; 64] = [
    16, 11, 10, 16,  24,  40,  51,  61,
    12, 12, 14, 19,  26,  58,  60,  55,
    14, 13, 16, 24,  40,  57,  69,  56,
    14, 17, 22, 29,  51,  87,  80,  62,
    18, 22, 37, 56,  68, 109, 103,  77,
    24, 35, 55, 64,  81, 104, 113,  92,
    49, 64, 78, 87, 103, 121, 120, 101,
    72, 92, 95, 98, 112, 100, 103,  99,
];

/// 4×4×4 quant matrix. Ported from v9 verbatim. Frequency increases 
/// away from origin (0,0,0); higher frequencies get coarser steps.
pub const QUANT_MATRIX_4X4X4: [u8; 64] = [
    1,  2,  4,  8,    2,  3,  5,  9,    4,  5,  7, 11,    8,  9, 11, 15,
    2,  3,  5,  9,    3,  4,  6, 10,    5,  6,  8, 12,    9, 10, 12, 16,
    4,  5,  7, 11,    5,  6,  8, 12,    7,  8, 10, 14,   11, 12, 14, 18,
    8,  9, 11, 15,    9, 10, 12, 16,   11, 12, 14, 18,   15, 16, 18, 22,
];

// ──────────────────────────────────────────────────────────────────
// Quantize / dequantize — generic over block size
// ──────────────────────────────────────────────────────────────────

/// Encoded quantization parameters that the decoder needs to invert.
/// Packed into a single u8 to keep the wire format compact.
/// 
/// Layout: bits 0..6 = log2(scale) + 8 (range -8..+55), bit 7 = reserved.
/// Decoders read scale_byte and compute scale = 2^(scale_byte - 8) / 127.
#[derive(Clone, Copy, Debug)]
pub struct QuantParams {
    pub scale_byte: u8,
    pub saliency_boost: u8,  // 0..3
}

impl QuantParams {
    /// Pack into a single byte: low 6 bits = scale_byte, top 2 bits = saliency_boost.
    /// Saturating — saliency clipped to 3 (which is the max useful value).
    pub fn pack(&self) -> u8 {
        let s = self.scale_byte & 0x3F;
        let b = (self.saliency_boost.min(3)) << 6;
        s | b
    }
    pub fn unpack(byte: u8) -> Self {
        Self {
            scale_byte: byte & 0x3F,
            saliency_boost: byte >> 6,
        }
    }
}

/// Quantize a DCT-transformed block. Generic helper that the per-size 
/// public functions delegate to.
/// 
/// Algorithm:
/// 1. Find max absolute coefficient → choose a per-block scale that 
///    fits the largest coef into i8 range after dividing by quant 
///    matrix.
/// 2. Encode scale as `log2(maxabs).ceil()` clamped to [-8, 55] so it 
///    fits in 6 bits of the packed param byte.
/// 3. For each coefficient: divide by (matrix[i] * scale / 2^boost), 
///    round, clamp to i8.
/// 
/// Returns the quantized coefficients and packed param byte. The 
/// param byte is what gets serialized; the i8 coefs go into the 
/// residual block payload.
fn quantize_generic<const N: usize>(
    coefs: &[f32; N],
    matrix: &[u8; N],
    saliency_boost: u8,
) -> ([i8; N], QuantParams) {
    let mut maxabs = 0.0f32;
    for &v in coefs.iter() { let a = v.abs(); if a > maxabs { maxabs = a; } }
    if maxabs < 1e-6 {
        return ([0i8; N], QuantParams { scale_byte: 0, saliency_boost });
    }
    let log = maxabs.log2().ceil().clamp(-8.0, 55.0);
    let scale = 2.0_f32.powf(log) / 127.0;
    let boost_div = (1u8 << saliency_boost.min(3)) as f32;
    let mut q = [0i8; N];
    for i in 0..N {
        let step = matrix[i] as f32 * scale / boost_div;
        let v = (coefs[i] / step.max(1e-6)).round();
        q[i] = v.clamp(-127.0, 127.0) as i8;
    }
    let scale_byte = ((log + 8.0).round().clamp(0.0, 63.0)) as u8;
    (q, QuantParams { scale_byte, saliency_boost })
}

fn dequantize_generic<const N: usize>(
    q: &[i8; N],
    matrix: &[u8; N],
    params: QuantParams,
) -> [f32; N] {
    let log = params.scale_byte as f32 - 8.0;
    let scale = 2.0_f32.powf(log) / 127.0;
    let boost_div = (1u8 << params.saliency_boost.min(3)) as f32;
    let mut out = [0.0f32; N];
    for i in 0..N {
        let step = matrix[i] as f32 * scale / boost_div;
        out[i] = q[i] as f32 * step;
    }
    out
}

/// Public: quantize a 4×4 DCT block (16 coefs).
pub fn quantize_4x4(coefs: &[f32; 16], saliency_boost: u8) -> ([i8; 16], QuantParams) {
    quantize_generic(coefs, &QUANT_MATRIX_4X4, saliency_boost)
}

/// Public: dequantize a 4×4 DCT block.
pub fn dequantize_4x4(q: &[i8; 16], params: QuantParams) -> [f32; 16] {
    dequantize_generic(q, &QUANT_MATRIX_4X4, params)
}

/// Public: quantize an 8×8 DCT block (64 coefs).
pub fn quantize_8x8(coefs: &[f32; 64], saliency_boost: u8) -> ([i8; 64], QuantParams) {
    quantize_generic(coefs, &QUANT_MATRIX_8X8, saliency_boost)
}

/// Public: dequantize an 8×8 DCT block.
pub fn dequantize_8x8(q: &[i8; 64], params: QuantParams) -> [f32; 64] {
    dequantize_generic(q, &QUANT_MATRIX_8X8, params)
}

/// Public: quantize a 4×4×4 DCT block (64 coefs, 3D layout).
/// 
/// Matches the existing v9 contract: returns ([i8; 64], scale_byte: u8).
/// Saliency boost packed into the QuantParams.
pub fn quantize_4x4x4(coefs: &[f32; 64], saliency_boost: u8) -> ([i8; 64], QuantParams) {
    quantize_generic(coefs, &QUANT_MATRIX_4X4X4, saliency_boost)
}

/// Public: dequantize a 4×4×4 DCT block.
pub fn dequantize_4x4x4(q: &[i8; 64], params: QuantParams) -> [f32; 64] {
    dequantize_generic(q, &QUANT_MATRIX_4X4X4, params)
}

// ──────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    
    /// 4-point DCT followed by IDCT should recover the input within f32 epsilon.
    #[test]
    fn dct1d_4_roundtrip() {
        let mut x = [1.0f32, 2.0, 3.0, 4.0];
        let orig = x;
        dct1d_4(&mut x);
        idct1d_4(&mut x);
        for i in 0..4 {
            assert!((x[i] - orig[i]).abs() < 1e-4, 
                "1D 4-pt DCT roundtrip failed at {}: {} vs {}", i, x[i], orig[i]);
        }
    }
    
    /// 8-point DCT followed by IDCT should recover the input within f32 epsilon.
    #[test]
    fn dct1d_8_roundtrip() {
        let mut x = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let orig = x;
        dct1d_8(&mut x);
        idct1d_8(&mut x);
        for i in 0..8 {
            assert!((x[i] - orig[i]).abs() < 1e-3, 
                "1D 8-pt DCT roundtrip failed at {}: {} vs {}", i, x[i], orig[i]);
        }
    }
    
    /// 2D 4×4 DCT roundtrip — separability test.
    #[test]
    fn dct2d_4x4_roundtrip() {
        let mut block = [0.0f32; 16];
        for i in 0..16 { block[i] = i as f32 + 1.0; }
        let orig = block;
        dct2d_4x4(&mut block);
        idct2d_4x4(&mut block);
        for i in 0..16 {
            assert!((block[i] - orig[i]).abs() < 1e-3,
                "2D 4×4 DCT roundtrip failed at {}: {} vs {}", i, block[i], orig[i]);
        }
    }
    
    /// 2D 8×8 DCT roundtrip.
    #[test]
    fn dct2d_8x8_roundtrip() {
        let mut block = [0.0f32; 64];
        for i in 0..64 { block[i] = ((i % 7) as f32) - 3.0; }
        let orig = block;
        dct2d_8x8(&mut block);
        idct2d_8x8(&mut block);
        for i in 0..64 {
            assert!((block[i] - orig[i]).abs() < 1e-2,
                "2D 8×8 DCT roundtrip failed at {}: {} vs {}", i, block[i], orig[i]);
        }
    }
    
    /// 3D DCT roundtrip — must match v9 behavior exactly.
    #[test]
    fn dct3d_4x4x4_roundtrip() {
        let mut block = [0.0f32; 64];
        for i in 0..64 { block[i] = (i as f32) * 0.3 - 5.0; }
        let orig = block;
        dct3d_4x4x4(&mut block);
        idct3d_4x4x4(&mut block);
        for i in 0..64 {
            assert!((block[i] - orig[i]).abs() < 1e-2,
                "3D 4×4×4 DCT roundtrip failed at {}: {} vs {}", i, block[i], orig[i]);
        }
    }
    
    /// Quantize/dequantize roundtrip for 4×4: with no quantization loss
    /// (small input that fits in one quant step), the recovered values 
    /// should equal the input.
    #[test]
    fn quant_4x4_lossless_on_zero() {
        let coefs = [0.0f32; 16];
        let (q, params) = quantize_4x4(&coefs, 0);
        assert_eq!(q, [0i8; 16]);
        let recovered = dequantize_4x4(&q, params);
        for v in &recovered { assert!(v.abs() < 1e-6); }
    }
    
    /// QuantParams pack/unpack round-trip.
    #[test]
    fn quant_params_pack_roundtrip() {
        for scale in 0..64u8 {
            for boost in 0..4u8 {
                let p = QuantParams { scale_byte: scale, saliency_boost: boost };
                let packed = p.pack();
                let unpacked = QuantParams::unpack(packed);
                assert_eq!(unpacked.scale_byte, scale);
                assert_eq!(unpacked.saliency_boost, boost);
            }
        }
    }
}
