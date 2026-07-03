//! # 2D Residual coding pipeline
//!
//! Encodes the difference between a predicted RGB frame and the actual 
//! target frame. The new pipeline (v10-style) replaces the old v8.1+ 
//! bias-128 box-average residual with DCT-quantized 4×4 blocks plus 
//! optional AV1-inspired post-processing.
//!
//! ## Lineage
//!
//! Earlier versions:
//! - v8.1: per-block 4×4 box-average of (target - predicted), biased by 
//!   128 and clamped to 0..255. Flat quantizer, no frequency awareness.
//! - v8.2: added skip-block RLE, DC prediction, channel deinterleave 
//!   on top of v8.1 box-average. Same flat quantizer.
//! - v9: introduced DCT + quant-matrix-based quantization for 3D 
//!   volumetric blocks. Never crossed over to 2D residuals.
//!
//! v10 unifies: 4×4 DCT (from `transform.rs`) + the v8.2 post-processing 
//! stack. The bias-128 path is gone — DCT is strictly better and the 
//! quant matrix gives finer control than a flat 128-step.
//!
//! ## Wire format
//!
//! ```text
//! Residual trailer (per-frame):
//!   ┌────────────────────────────────────────────────┐
//!   │ rw u16    (bits 0..12 = block width, 13..15 flags)
//!   │ rh u16    (block height)
//!   │ zstd-compressed payload:
//!   │   for each block in raster order:
//!   │     param_byte u8     (QuantParams.pack())
//!   │     coefs [i8; 16]    (DCT-quantized, 4x4)
//!   │   OR with skip-block RLE (bit 15 set):
//!   │     segments of: skip_count u32, n_nonskip u8, blocks...
//!   └────────────────────────────────────────────────┘
//! ```
//!
//! Width header bits:
//! - bit 15: skip-block RLE active
//! - bit 14: DC prediction active (only meaningful with bit 15)
//! - bit 13: channel deinterleave active (only meaningful with bit 15)
//! - bits 0..12: residual width in blocks (max 8191 — covers a 32K-wide source)
//!
//! ## Why 4×4 blocks?
//!
//! Same block size as the legacy bias-128 path, so the residual buffer 
//! dimensions don't change. That lets the v8/v9 callers and the 
//! decoder upsample logic stay unchanged.
//!
//! The variable-block-size (4×4 vs 8×8) work was discussed but is 
//! deferred to v10 proper — it requires a per-block mode flag, and 
//! we want one source of truth for the new pipeline before adding 
//! mode dispatch.

use crate::transform::{dct2d_4x4, idct2d_4x4, quantize_4x4, dequantize_4x4, QuantParams};

// ──────────────────────────────────────────────────────────────────
// Options
// ──────────────────────────────────────────────────────────────────

/// Encoder options. Replaces the FAO_V82_* environment variable sniffing 
/// that the old code did inside `compute_photometric_residual`.
/// 
/// `from_env()` constructs an instance from environment variables for 
/// backward compatibility with the existing toggles.
#[derive(Clone, Copy, Debug)]
pub struct ResidualOpts {
    /// Skip-block RLE for blocks where all coefficients fit within 
    /// the threshold of zero. Big win on flat regions.
    pub skip_block_rle: bool,
    /// DC prediction across adjacent blocks. Subtracts predicted DC 
    /// (from left+top neighbors) before quantizing the actual DC. 
    /// Combines with skip-block: more blocks become "skip" after 
    /// prediction.
    pub dc_prediction: bool,
    /// Per-segment channel deinterleave (RRR…GGG…BBB) within non-skip 
    /// runs. zstd compresses monotone byte streams better than 
    /// channel-interleaved ones.
    pub channel_deinterleave: bool,
    /// Saliency boost (0..3). Halves quantization steps for higher 
    /// boost values, retaining more detail. Typically 0 for default 
    /// quality, 1-2 for feature-dense regions, 3 for foveal areas.
    pub saliency_boost: u8,
    /// Skip threshold — DCT-quantized blocks where every absolute 
    /// coefficient is below this become skip blocks. Tuned for the 
    /// quant matrix in transform::QUANT_MATRIX_4X4.
    pub skip_threshold: i8,
}

impl Default for ResidualOpts {
    fn default() -> Self {
        Self {
            skip_block_rle: true,
            dc_prediction: true,
            channel_deinterleave: true,
            saliency_boost: 0,
            skip_threshold: 1,
        }
    }
}

impl ResidualOpts {
    /// Read v8.2-era environment variables. For backward compatibility 
    /// with existing FAO_V82_SKIP / FAO_V82_DCPRED / FAO_V82_DEINT 
    /// knobs. If none are set, returns Default::default().
    pub fn from_env() -> Self {
        fn env_on(name: &str) -> Option<bool> {
            std::env::var(name).ok().map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        }
        let mut opts = Self::default();
        if let Some(v) = env_on("FAO_V82_SKIP") { opts.skip_block_rle = v; }
        if let Some(v) = env_on("FAO_V82_DCPRED") { opts.dc_prediction = v; }
        if let Some(v) = env_on("FAO_V82_DEINT") { opts.channel_deinterleave = v; }
        opts
    }
}

// ──────────────────────────────────────────────────────────────────
// Header bit layout
// ──────────────────────────────────────────────────────────────────

const FLAG_SKIP_RLE: u16     = 0x8000;
const FLAG_DC_PREDICTION: u16 = 0x4000;
const FLAG_DEINTERLEAVE: u16  = 0x2000;
/// Bits 11+12: combined magic identifying v10 DCT-format residual. 
/// Legacy bias-128 residuals had bits 0..12 dedicated to width; both 
/// bits 11 and 12 set simultaneously would require rw >= 6144, which 
/// is quarter-res of a 24576-wide source — not a realistic value. 
/// Requiring both bits as a signature avoids accidental misrouting on 
/// any plausible legacy file.
/// 
/// With these two bits reserved, the residual width is in bits 0..10 
/// (max 2047, quarter-res of 8188-wide source — covers 8K).
pub const FLAG_DCT_FORMAT: u16 = 0x1800;  // bits 11+12 BOTH set
const RW_MASK: u16            = 0x07FF;  // bits 0..10

// ──────────────────────────────────────────────────────────────────
// Encoder
// ──────────────────────────────────────────────────────────────────

/// Compute a 2D DCT residual trailer for an RGB frame.
/// 
/// `predicted` and `target` are both w*h*3 byte buffers (interleaved RGB). 
/// Residuals are computed at quarter resolution — each output block 
/// covers a 4×4 source-pixel region. So the output residual has w/4 
/// columns and h/4 rows of blocks.
/// 
/// Returns the serialized trailer (rw header + zstd-compressed payload), 
/// ready to be written into the .fao Z-wrapper's residual slot. 
/// Returns None if dimensions are invalid or predicted/target lengths 
/// don't match expected sizes.
pub fn encode(
    predicted: &[u8],
    target: &[u8],
    w: usize,
    h: usize,
    opts: &ResidualOpts,
) -> Option<Vec<u8>> {
    if predicted.len() != w * h * 3 || target.len() != w * h * 3 {
        return None;
    }
    if w < 4 || h < 4 { return None; }
    
    let rw = w / 4;
    let rh = h / 4;
    if rw > RW_MASK as usize { return None; }  // can't fit in header
    
    // Build per-block per-channel DCT-quantized residuals. We store 
    // them as flat (block_idx, channel) → ([i8; 16], QuantParams). 
    // For raster traversal: block_idx = by * rw + bx.
    let n_blocks = rw * rh;
    let mut blocks: Vec<([[i8; 16]; 3], [QuantParams; 3])> = Vec::with_capacity(n_blocks);
    
    let mut buf = [0.0f32; 16];
    for by in 0..rh {
        for bx in 0..rw {
            // Per channel, build the 4×4 residual block.
            let mut q_per_channel: [[i8; 16]; 3] = [[0; 16]; 3];
            let mut p_per_channel: [QuantParams; 3] = [
                QuantParams { scale_byte: 0, saliency_boost: 0 }; 3
            ];
            for c in 0..3 {
                // Fill buf with (target - predicted) for this 4×4 region.
                for ly in 0..4 {
                    for lx in 0..4 {
                        let px = bx * 4 + lx;
                        let py = by * 4 + ly;
                        let i = (py * w + px) * 3 + c;
                        buf[ly * 4 + lx] = target[i] as f32 - predicted[i] as f32;
                    }
                }
                // DCT.
                dct2d_4x4(&mut buf);
                // Quantize.
                let (q, p) = quantize_4x4(&buf, opts.saliency_boost);
                q_per_channel[c] = q;
                p_per_channel[c] = p;
            }
            blocks.push((q_per_channel, p_per_channel));
        }
    }
    
    // Optional: DC prediction. Subtract predicted DC (from neighbors' 
    // reconstructed DC) from this block's DC before serializing. 
    // Reconstructed DC = q[0] * (matrix[0] * scale / boost), which we 
    // can compute on the fly. We mutate the q[0] values in place.
    if opts.dc_prediction {
        // We need to track RECONSTRUCTED DC values for prediction lookups 
        // (encoder and decoder must agree). Maintain a parallel grid 
        // of reconstructed DCs per channel.
        let mut recon_dc: Vec<[f32; 3]> = vec![[0.0; 3]; n_blocks];
        for by in 0..rh {
            for bx in 0..rw {
                let bi = by * rw + bx;
                for c in 0..3 {
                    // Decode current DC.
                    let dq = dequantize_4x4(&blocks[bi].0[c], blocks[bi].1[c]);
                    let cur_dc = dq[0];
                    // Predict from top + left.
                    let pred = match (bx > 0, by > 0) {
                        (true, true) => 0.5 * (recon_dc[by * rw + (bx - 1)][c] + recon_dc[(by - 1) * rw + bx][c]),
                        (true, false) => recon_dc[by * rw + (bx - 1)][c],
                        (false, true) => recon_dc[(by - 1) * rw + bx][c],
                        (false, false) => 0.0,
                    };
                    // Write the residual DC back. The decoder will add 
                    // its own predicted DC to recover.
                    let delta = cur_dc - pred;
                    // Re-quantize delta into the [0] coefficient using 
                    // the same scale.
                    let p = blocks[bi].1[c];
                    let log = p.scale_byte as f32 - 8.0;
                    let scale = 2.0_f32.powf(log) / 127.0;
                    let boost_div = (1u8 << p.saliency_boost.min(3)) as f32;
                    let step = crate::transform::QUANT_MATRIX_4X4[0] as f32 * scale / boost_div;
                    let new_q0 = (delta / step.max(1e-6)).round().clamp(-127.0, 127.0) as i8;
                    blocks[bi].0[c][0] = new_q0;
                    // Track reconstructed-as-decoder-will-see DC for 
                    // downstream predictions.
                    recon_dc[bi][c] = pred + new_q0 as f32 * step;
                }
            }
        }
    }
    
    // Pack blocks into a byte stream, with optional skip-block RLE.
    let payload = if opts.skip_block_rle {
        pack_skip_rle(&blocks, opts)
    } else {
        pack_dense(&blocks)
    };
    
    // zstd compress. Adaptive level — small payloads (typical case) 
    // get the high-quality treatment.
    let lvl = if payload.len() <= 65536 { 12 } else { 6 };
    let compressed = match zstd::encode_all(payload.as_slice(), lvl) {
        Ok(c) => c,
        Err(_) => payload.clone(),
    };
    
    // Assemble final output: 4-byte header (rw_field, rh) + zstd payload.
    let mut rw_field: u16 = (rw as u16) & RW_MASK;
    rw_field |= FLAG_DCT_FORMAT;  // always set — identifies v10 format
    if opts.skip_block_rle      { rw_field |= FLAG_SKIP_RLE; }
    if opts.dc_prediction       { rw_field |= FLAG_DC_PREDICTION; }
    if opts.channel_deinterleave { rw_field |= FLAG_DEINTERLEAVE; }
    
    let mut out = Vec::with_capacity(4 + compressed.len());
    out.extend_from_slice(&rw_field.to_le_bytes());
    out.extend_from_slice(&(rh as u16).to_le_bytes());
    out.extend_from_slice(&compressed);
    Some(out)
}

/// Pack blocks without skip-block RLE — every block emits its full 
/// quantized representation. Less compression but simpler decode.
fn pack_dense(blocks: &[([[i8; 16]; 3], [QuantParams; 3])]) -> Vec<u8> {
    // Per block: 3 channels × (1 param byte + 16 i8 coefs) = 51 bytes.
    let mut out = Vec::with_capacity(blocks.len() * 51);
    for (qs, ps) in blocks {
        for c in 0..3 {
            out.push(ps[c].pack());
            for &q in &qs[c] {
                out.push(q as u8);
            }
        }
    }
    out
}

/// Pack blocks with skip-block RLE. A block is "skip" if every 
/// quantized coefficient (across all 3 channels) is within 
/// ±opts.skip_threshold of zero — the block contributes negligible 
/// signal and we collapse runs of them into a counter.
/// 
/// Format: alternating segments of (skip_count u32, n_nonskip u8, 
/// non_skip_blocks…). The first segment starts at block 0; if block 0 
/// is non-skip, skip_count is 0.
fn pack_skip_rle(
    blocks: &[([[i8; 16]; 3], [QuantParams; 3])],
    opts: &ResidualOpts,
) -> Vec<u8> {
    let thresh = opts.skip_threshold;
    let is_skip = |qs: &[[i8; 16]; 3]| -> bool {
        for c in 0..3 {
            for v in &qs[c] {
                if v.abs() > thresh { return false; }
            }
        }
        true
    };
    
    let mut out = Vec::with_capacity(blocks.len() / 2);
    let mut i = 0;
    while i < blocks.len() {
        // Count consecutive skip blocks.
        let mut skip_count = 0u32;
        while i < blocks.len() && is_skip(&blocks[i].0) {
            skip_count += 1;
            i += 1;
        }
        // Count consecutive non-skip blocks (capped at 255 per segment).
        let nonskip_start = i;
        let mut n_nonskip = 0u8;
        while i < blocks.len() && !is_skip(&blocks[i].0) && n_nonskip < 255 {
            n_nonskip += 1;
            i += 1;
        }
        if skip_count == 0 && n_nonskip == 0 { break; }
        
        out.extend_from_slice(&skip_count.to_le_bytes());
        out.push(n_nonskip);
        
        // Emit non-skip block bytes.
        if opts.channel_deinterleave && n_nonskip > 1 {
            // Per-channel grouping. Inside each channel: params bytes 
            // for all blocks first, then all coefficient streams.
            // 
            // Layout per channel:
            //   params[0..n_nonskip]   (n bytes)
            //   coefs[0][0..16]       
            //   coefs[1][0..16]       
            //   ...
            //   coefs[n-1][0..16]      (16*n bytes)
            // 
            // For three channels, we write channel 0 fully, then 1, then 2.
            for c in 0..3 {
                for j in 0..n_nonskip as usize {
                    out.push(blocks[nonskip_start + j].1[c].pack());
                }
                for j in 0..n_nonskip as usize {
                    for &q in &blocks[nonskip_start + j].0[c] {
                        out.push(q as u8);
                    }
                }
            }
        } else {
            // Interleaved: per block, all three channels in sequence.
            for j in 0..n_nonskip as usize {
                let (qs, ps) = &blocks[nonskip_start + j];
                for c in 0..3 {
                    out.push(ps[c].pack());
                    for &q in &qs[c] {
                        out.push(q as u8);
                    }
                }
            }
        }
    }
    out
}

// ──────────────────────────────────────────────────────────────────
// Decoder
// ──────────────────────────────────────────────────────────────────

/// Inspect a residual blob and return true if it's in the v10 DCT 
/// format (vs legacy bias-128). Used by the v8 wrapper layer to 
/// route between the two decode paths.
pub fn is_dct_format(residual_bytes: &[u8]) -> bool {
    if residual_bytes.len() < 4 { return false; }
    let rw_raw = u16::from_le_bytes([residual_bytes[0], residual_bytes[1]]);
    // Require BOTH bits 11 and 12 set — the 2-bit magic. Avoids 
    // misrouting on legacy bias-128 trailers that happen to have one 
    // of those bits set as part of a large width value.
    (rw_raw & FLAG_DCT_FORMAT) == FLAG_DCT_FORMAT
}

/// Apply a residual trailer to a predicted RGB frame. Returns the 
/// reconstructed RGB output (or the predicted frame unchanged on any 
/// parse error).
/// 
/// Mirror of `encode`. Parses header, zstd-decompresses payload, walks 
/// either dense or skip-RLE segments, inverse-quantizes each block, 
/// inverse-DCTs, adds the recovered residual to the predicted pixels.
pub fn apply(predicted: Vec<u8>, residual_bytes: &[u8], w: usize, h: usize) -> Vec<u8> {
    if residual_bytes.len() < 4 { return predicted; }
    let rw_raw = u16::from_le_bytes([residual_bytes[0], residual_bytes[1]]);
    let skip_rle = (rw_raw & FLAG_SKIP_RLE) != 0;
    let dc_pred  = (rw_raw & FLAG_DC_PREDICTION) != 0;
    let deint    = (rw_raw & FLAG_DEINTERLEAVE) != 0;
    let rw = (rw_raw & RW_MASK) as usize;
    let rh = u16::from_le_bytes([residual_bytes[2], residual_bytes[3]]) as usize;
    
    if rw == 0 || rh == 0 { return predicted; }
    if rw * 4 > w || rh * 4 > h { return predicted; }
    
    let decompressed = match zstd::decode_all(&residual_bytes[4..]) {
        Ok(d) => d,
        Err(_) => return predicted,
    };
    
    let n_blocks = rw * rh;
    let mut blocks: Vec<([[i8; 16]; 3], [QuantParams; 3])> = vec![
        ([[0; 16]; 3], [QuantParams { scale_byte: 0, saliency_boost: 0 }; 3]); 
        n_blocks
    ];
    
    // Unpack into the blocks array.
    let parse_ok = if skip_rle {
        unpack_skip_rle(&decompressed, &mut blocks, deint)
    } else {
        unpack_dense(&decompressed, &mut blocks)
    };
    if !parse_ok { return predicted; }
    
    // Inverse DC prediction — reconstruct true DC from delta.
    if dc_pred {
        let mut recon_dc: Vec<[f32; 3]> = vec![[0.0; 3]; n_blocks];
        for by in 0..rh {
            for bx in 0..rw {
                let bi = by * rw + bx;
                for c in 0..3 {
                    let pred = match (bx > 0, by > 0) {
                        (true, true) => 0.5 * (recon_dc[by * rw + (bx - 1)][c] + recon_dc[(by - 1) * rw + bx][c]),
                        (true, false) => recon_dc[by * rw + (bx - 1)][c],
                        (false, true) => recon_dc[(by - 1) * rw + bx][c],
                        (false, false) => 0.0,
                    };
                    let p = blocks[bi].1[c];
                    let log = p.scale_byte as f32 - 8.0;
                    let scale = 2.0_f32.powf(log) / 127.0;
                    let boost_div = (1u8 << p.saliency_boost.min(3)) as f32;
                    let step = crate::transform::QUANT_MATRIX_4X4[0] as f32 * scale / boost_div;
                    let delta_dc = blocks[bi].0[c][0] as f32 * step;
                    let true_dc = pred + delta_dc;
                    recon_dc[bi][c] = true_dc;
                    // Convert true_dc back to a q0 value that, when 
                    // dequantized, yields true_dc. The decode loop below 
                    // assumes q[0] is the absolute DC.
                    let new_q0 = (true_dc / step.max(1e-6)).round().clamp(-127.0, 127.0) as i8;
                    blocks[bi].0[c][0] = new_q0;
                }
            }
        }
    }
    
    // Now per-block: dequantize, inverse-DCT, add to predicted.
    let mut out = predicted;
    let mut buf = [0.0f32; 16];
    for by in 0..rh {
        for bx in 0..rw {
            let bi = by * rw + bx;
            for c in 0..3 {
                let dq = dequantize_4x4(&blocks[bi].0[c], blocks[bi].1[c]);
                buf.copy_from_slice(&dq);
                idct2d_4x4(&mut buf);
                // Add the recovered residual to the predicted pixels.
                for ly in 0..4 {
                    for lx in 0..4 {
                        let px = bx * 4 + lx;
                        let py = by * 4 + ly;
                        let i = (py * w + px) * 3 + c;
                        let v = out[i] as f32 + buf[ly * 4 + lx];
                        out[i] = v.clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
    }
    out
}

fn unpack_dense(
    data: &[u8],
    blocks: &mut [([[i8; 16]; 3], [QuantParams; 3])],
) -> bool {
    // 51 bytes per block (3 channels × 17 bytes).
    if data.len() < blocks.len() * 51 { return false; }
    let mut off = 0;
    for bi in 0..blocks.len() {
        for c in 0..3 {
            blocks[bi].1[c] = QuantParams::unpack(data[off]);
            off += 1;
            for k in 0..16 {
                blocks[bi].0[c][k] = data[off + k] as i8;
            }
            off += 16;
        }
    }
    true
}

fn unpack_skip_rle(
    data: &[u8],
    blocks: &mut [([[i8; 16]; 3], [QuantParams; 3])],
    deint: bool,
) -> bool {
    let n_blocks = blocks.len();
    let mut off = 0;
    let mut block_idx = 0;
    
    while off + 5 <= data.len() && block_idx < n_blocks {
        let skip_count = u32::from_le_bytes([
            data[off], data[off+1], data[off+2], data[off+3],
        ]) as usize;
        let n_nonskip = data[off + 4] as usize;
        off += 5;
        
        block_idx = block_idx.saturating_add(skip_count);
        if block_idx >= n_blocks { break; }
        
        // Each non-skip block is 51 bytes (3 channels × 17 bytes).
        if off + n_nonskip * 51 > data.len() { return false; }
        
        if deint && n_nonskip > 1 {
            // Per channel: params bytes for n blocks, then coefs for n blocks.
            for c in 0..3 {
                let params_start = off;
                let coefs_start = off + n_nonskip;
                for j in 0..n_nonskip {
                    if block_idx + j >= n_blocks { break; }
                    blocks[block_idx + j].1[c] = QuantParams::unpack(data[params_start + j]);
                    for k in 0..16 {
                        blocks[block_idx + j].0[c][k] = data[coefs_start + j * 16 + k] as i8;
                    }
                }
                off += n_nonskip * 17;  // n params + n*16 coefs
            }
        } else {
            // Interleaved.
            for j in 0..n_nonskip {
                if block_idx + j >= n_blocks { break; }
                for c in 0..3 {
                    blocks[block_idx + j].1[c] = QuantParams::unpack(data[off]);
                    off += 1;
                    for k in 0..16 {
                        blocks[block_idx + j].0[c][k] = data[off + k] as i8;
                    }
                    off += 16;
                }
            }
        }
        
        block_idx += n_nonskip;
    }
    true
}

// ──────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    
    /// Predicted == target → residual encodes to mostly-zero coefficients, 
    /// applies cleanly back. PSNR effectively infinite.
    #[test]
    fn identity_residual_lossless() {
        let w = 16; let h = 16;
        let mut predicted = vec![0u8; w * h * 3];
        for i in 0..predicted.len() { predicted[i] = (i % 200) as u8; }
        let target = predicted.clone();
        
        let opts = ResidualOpts::default();
        let trailer = encode(&predicted, &target, w, h, &opts).expect("encode");
        
        let recovered = apply(predicted.clone(), &trailer, w, h);
        // With zero residual everywhere, recovered should equal predicted.
        // (Lossless because all delta values are exactly 0 → no quant noise.)
        for i in 0..predicted.len() {
            assert_eq!(recovered[i], predicted[i], 
                "identity residual differs at byte {}", i);
        }
    }
    
    /// Small constant difference → encodes + decodes within a few units 
    /// of quant error.
    #[test]
    fn small_constant_offset() {
        let w = 16; let h = 16;
        let predicted = vec![100u8; w * h * 3];
        let target = vec![110u8; w * h * 3];
        
        let opts = ResidualOpts::default();
        let trailer = encode(&predicted, &target, w, h, &opts).expect("encode");
        
        let recovered = apply(predicted, &trailer, w, h);
        // Should recover close to 110.
        let mean_diff: f32 = recovered.iter()
            .map(|&v| (v as f32 - 110.0).abs())
            .sum::<f32>() / recovered.len() as f32;
        assert!(mean_diff < 5.0, 
            "constant-offset residual has mean error {} (expected <5)", mean_diff);
    }
    
    /// Skip-block RLE off vs on should produce the same recovered output 
    /// (different bytes on the wire, identical reconstruction).
    #[test]
    fn skip_rle_roundtrip_matches_dense() {
        let w = 32; let h = 32;
        let predicted = vec![100u8; w * h * 3];
        let mut target = vec![100u8; w * h * 3];
        // Add a feature region (non-skip) in the top-left.
        for py in 0..8 {
            for px in 0..8 {
                for c in 0..3 {
                    target[(py * w + px) * 3 + c] = 200;
                }
            }
        }
        
        let mut opts = ResidualOpts::default();
        opts.skip_block_rle = false;
        let trailer_dense = encode(&predicted, &target, w, h, &opts).expect("dense encode");
        let recovered_dense = apply(predicted.clone(), &trailer_dense, w, h);
        
        opts.skip_block_rle = true;
        opts.dc_prediction = false;
        opts.channel_deinterleave = false;
        let trailer_skip = encode(&predicted, &target, w, h, &opts).expect("skip encode");
        let recovered_skip = apply(predicted.clone(), &trailer_skip, w, h);
        
        // Both reconstructions should agree (same quant, same dequant).
        for i in 0..recovered_dense.len() {
            assert_eq!(recovered_dense[i], recovered_skip[i],
                "dense vs skip-rle reconstruction mismatch at byte {}: {} vs {}",
                i, recovered_dense[i], recovered_skip[i]);
        }
    }
    
    /// DC prediction on/off — should produce different bytes but 
    /// reconstruction within quant error.
    #[test]
    fn dc_pred_roundtrips_within_quant_error() {
        let w = 16; let h = 16;
        let predicted = vec![80u8; w * h * 3];
        let mut target = vec![80u8; w * h * 3];
        for py in 0..h {
            for px in 0..w {
                let v = ((px + py) * 3) as u8;
                for c in 0..3 {
                    target[(py * w + px) * 3 + c] = v;
                }
            }
        }
        
        let mut opts = ResidualOpts::default();
        opts.dc_prediction = true;
        opts.skip_block_rle = false;
        let trailer = encode(&predicted, &target, w, h, &opts).expect("encode");
        let recovered = apply(predicted.clone(), &trailer, w, h);
        
        // Mean error should be bounded by the quant matrix granularity.
        let mean_err: f32 = recovered.iter().zip(target.iter())
            .map(|(&r, &t)| (r as f32 - t as f32).abs())
            .sum::<f32>() / (w * h * 3) as f32;
        assert!(mean_err < 10.0, 
            "DC-pred reconstruction error {} too high (expected <10)", mean_err);
    }
}
