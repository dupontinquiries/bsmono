// v2.1 Predictive Encoder — "Translate" Tier
// 
// Goal: instead of passing through small frames, transform them via the
// web function. Output = (web_traversal_ref + minimal_residual).
//
// This achieves real compression on near-static content by encoding only
// the DIFFERENCE between predicted and actual.
//
// Encoding pipeline:
//   1. Get anchor frame (recent full encode)
//   2. Predict: warp anchor by motion vectors → predicted_yuv
//   3. Compute residual = actual - predicted (signed delta per pixel)
//   4. Quantize residual aggressively (most pixels become 0)
//   5. RLE + zstd the quantized residual
//   6. Output: anchor_ref(2B) + mv_count(1B) + mvs(N×4B) + residual_size(2B) + residual
//
// Typical sizes:
//   Static UI frame: 50-200 bytes (almost all zero residual)
//   Slow motion: 500-2000 bytes
//   Heavy motion: 5-15KB (still smaller than full encode at ~50KB)
//
// Decoding:
//   1. Read anchor_ref → fetch anchor from cache
//   2. Read MVs → warp anchor → predicted_yuv  
//   3. Read residual → de-RLE → de-quantize → add to predicted
//   4. Output: actual_yuv

use std::io::{Cursor, Write, Read};

/// A "translated" frame: prediction reference + residual.
/// This is what Tier 1 produces in v2.1.
#[derive(Clone, Debug)]
pub struct TranslatedFrame {
    pub anchor_ref: u16,        // index of anchor frame in archive (0-65535)
    pub motion_vectors: Vec<(i8, i8)>,  // 16x16 block MVs (dx, dy in pixels, ±127)
    pub residual_quantized: Vec<i8>,    // quantized pixel differences (full or half res based on flag)
    pub quant_step: u8,         // quantization step size (1=lossless, 8=aggressive)
    pub flags: u8,              // bit 0: half_res residual, bit 1: motion-only, bit 2: filter_mask present
    /// Optional per-block filter selection (2 bits per block, packed)
    /// When flags bit 2 is set, decoder applies these filters BEFORE adding residual.
    /// Empty = no filtering (legacy behavior, identity filter for all blocks).
    pub filter_mask: Vec<u8>,
}

impl TranslatedFrame {
    /// Encode a frame as TranslatedFrame given anchor and motion estimate.
    /// Returns (TranslatedFrame, estimated_psnr_dB).
    pub fn encode_from_anchor(
        actual_yuv: &[u8],
        anchor_yuv: &[u8],
        w: usize, h: usize,
        anchor_ref: u16,
        quality_target: u8,  // 0-255: 0=tiny+lossy, 255=large+lossless
    ) -> Option<(Self, f32)> {
        let y_sz = w * h;
        let uv_sz = y_sz / 2;
        if actual_yuv.len() < y_sz + uv_sz || anchor_yuv.len() < y_sz + uv_sz {
            return None;
        }
        
        // FAST PATH: detect mostly-static frames in O(N/64) time
        // If average per-block SAD is very low, skip motion search entirely
        // (saves 95% of ME time for static UI / paused gameplay)
        let bw = w / 16;
        let bh = h / 16;
        let n_blocks = bw * bh;
        
        // Quick whole-frame SAD: every 16th pixel (~5184 samples for 1080p)
        let mut total_sad: u64 = 0;
        for py in (0..h).step_by(16) {
            for px in (0..w).step_by(16) {
                let i = py * w + px;
                if i < actual_yuv.len() && i < anchor_yuv.len() {
                    total_sad += (actual_yuv[i] as i32 - anchor_yuv[i] as i32).unsigned_abs() as u64;
                }
            }
        }
        // If avg SAD per sample < 4 luma units, frame is essentially static
        let n_samples = (h / 16) * (w / 16);
        let avg_sad = if n_samples > 0 { total_sad / n_samples as u64 } else { 0 };
        let frame_is_static = avg_sad < 4;
        
        let mut mvs: Vec<(i8, i8)> = Vec::with_capacity(n_blocks);
        
        if frame_is_static {
            // Skip motion estimation entirely — all-zero MVs
            mvs = vec![(0, 0); n_blocks];
        } else {
        // Stage 1: motion estimation (only run if frame isn't mostly static)
        
        // Ultra-sparse SAD: only 16 sample points per 256-pixel block (every 4th)
        // Still enough to distinguish motion vectors, 4x faster than dense SAD
        let fast_sad = |bx: usize, by: usize, dx: i32, dy: i32| -> u32 {
            let mut sum = 0u32;
            for py in (0..16).step_by(4) {
                for px in (0..16).step_by(4) {
                    let dst_x = bx * 16 + px;
                    let dst_y = by * 16 + py;
                    if dst_x >= w || dst_y >= h { continue; }
                    let actual_v = actual_yuv[dst_y * w + dst_x] as i32;
                    let sx = (dst_x as i32 + dx).clamp(0, w as i32 - 1) as usize;
                    let sy = (dst_y as i32 + dy).clamp(0, h as i32 - 1) as usize;
                    let pred_v = anchor_yuv[sy * w + sx] as i32;
                    sum += (actual_v - pred_v).unsigned_abs();
                }
            }
            sum
        };
        
        for by in 0..bh {
            for bx in 0..bw {
                let mut best_dx = 0i32;
                let mut best_dy = 0i32;
                let mut best_sad = fast_sad(bx, by, 0, 0);
                
                // STATIC FAST PATH: most game blocks are static, early exit
                if best_sad < 8 {
                    mvs.push((0, 0));
                    continue;
                }
                
                // STICKINESS BONUS: prefer (0,0) when alternatives are close
                // This eliminates "off-frame quantization" jitter where small
                // SAD differences cause MVs to flicker between candidates.
                // Cost: small SAD penalty for non-zero MVs (must be clearly better)
                let stickiness = best_sad / 8;  // 12.5% bonus for staying at (0,0)
                
                // Test 4 cardinal directions at ±2 step (small motion is most common)
                for &(dx, dy) in &[(-2, 0), (2, 0), (0, -2), (0, 2)] {
                    let s = fast_sad(bx, by, dx, dy);
                    // Non-zero MV must beat zero by at least the stickiness penalty
                    if s + stickiness < best_sad {
                        best_sad = s;
                        best_dx = dx;
                        best_dy = dy;
                    }
                }
                
                // SKIP REFINE if coarse search already found very good match
                if best_sad < 16 {
                    mvs.push((best_dx.clamp(-127, 127) as i8, best_dy.clamp(-127, 127) as i8));
                    continue;
                }
                
                // Wider search at ±4 only when ±2 search was hitting boundaries
                if best_dx.abs() == 2 || best_dy.abs() == 2 {
                    for &(dx, dy) in &[(-4, 0), (4, 0), (0, -4), (0, 4)] {
                        let s = fast_sad(bx, by, dx, dy);
                        if s + stickiness < best_sad {
                            best_sad = s;
                            best_dx = dx;
                            best_dy = dy;
                        }
                    }
                }
                
                mvs.push((best_dx.clamp(-127, 127) as i8, best_dy.clamp(-127, 127) as i8));
            }
        }
        }  // close else (non-static path)
        
        // Stage 2: build prediction
        let predicted = predict_from_motion(anchor_yuv, &mvs, w, h);
        
        // Stage 3: compute residual per pixel, ZERO it out for well-predicted blocks
        // Quality target → quant_step:
        //   quality=255: step=1 (lossless), big files
        //   quality=200: step=4 (transparent ~38dB), medium files  
        //   quality=128: step=8 (visible ~30dB but acceptable), small files
        //   quality=0:   step=16 (heavy ~24dB), tiny files
        let quant_step = (((255 - quality_target) as u32 * 15 / 255) + 1).min(16) as u8;
        
        // First pass: compute per-block SAD to identify "free" blocks
        // (blocks where motion compensation alone is good enough to skip residual)
        let block_size = 16usize;
        let bw = w / block_size;
        let bh = h / block_size;
        let n_blocks = bw * bh;
        let mut block_keep = vec![false; n_blocks];  // true = store residual
        
        // SAD threshold: VERY aggressive (skip residuals unless prediction is terrible)
        // 2048 per quant_step = ~64 luma units avg per pixel before storing residual
        // This is the main file-size win.
        // 
        // In FAO_V21_TINY mode, we bump the threshold ~3× so even more 
        // blocks skip residual — relying on motion compensation alone 
        // wherever it gets close. Combined with quarter-res residual on 
        // the blocks that DO keep, this is the path to ~10× T-frame 
        // size reduction.
        let tiny_mode_thresh = std::env::var("FAO_V21_TINY")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let sad_mul = if tiny_mode_thresh { 4096u32 } else { 2048u32 };
        let sad_skip_threshold = (quant_step as u32) * sad_mul;
        
        for by in 0..bh {
            for bx in 0..bw {
                let mut block_sad: u32 = 0;
                for py in 0..block_size {
                    for px in 0..block_size {
                        let x = bx * block_size + px;
                        let y = by * block_size + py;
                        if x >= w || y >= h { continue; }
                        let i = y * w + x;
                        block_sad += (actual_yuv[i] as i32 - predicted[i] as i32).unsigned_abs();
                    }
                }
                // Block needs residual only if motion comp alone wasn't good enough
                if block_sad > sad_skip_threshold {
                    block_keep[by * bw + bx] = true;
                }
            }
        }
        
        let kept_blocks = block_keep.iter().filter(|&&k| k).count();
        
        // OPTIMIZATION: if no blocks need residual, return motion-only frame
        // This is the smallest possible T-frame (~200 bytes after MV compression)
        if kept_blocks == 0 {
            // Compute total error from prediction-only path
            let mut sum_sq: u64 = 0;
            for i in 0..y_sz {
                let err = actual_yuv[i] as i32 - predicted[i] as i32;
                sum_sq += (err * err) as u64;
            }
            let mse = sum_sq as f64 / y_sz as f64;
            let psnr = if mse > 0.0 {
                10.0 * (255.0 * 255.0 / mse).log10() as f32
            } else { 99.0 };
            
            return Some((Self {
                anchor_ref,
                motion_vectors: mvs,
                residual_quantized: vec![0i8; 0],
                quant_step: 0,
                flags: 0x02,
                filter_mask: Vec::new(),
            }, psnr));
        }
        
        // Stage 4: Residual quantization. Two resolution modes:
        //   - Half-res (default): 1/4 the samples. ~68KB/frame at 1080p.
        //   - Quarter-res (FAO_V21_TINY=1): 1/16 the samples. ~7-10KB/frame.
        //     Each quarter-res pixel covers a 4×4 full-res tile. Trades 
        //     visual detail for size — useful when v2.1 T-frames are 
        //     dominating filesize (as in DBD content where v8 can't hold 
        //     enough detail in mesh form).
        // 
        // The mode is encoded in `flags` so the decoder picks the right 
        // upsample factor without an out-of-band signal:
        //   flag bit 0 (0x01) — half-res (legacy)
        //   flag bit 2 (0x04) — quarter-res (new in this session)
        let tiny_mode = std::env::var("FAO_V21_TINY")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let downscale: usize = if tiny_mode { 4 } else { 2 };
        let hw = w / downscale;
        let hh = h / downscale;
        let half_sz = hw * hh;
        let mut residual_quantized = vec![0i8; half_sz];
        let mut sum_sq_err: u64 = 0;
        let mut sampled = 0u64;
        
        // Per-block iteration: each 16×16 block contains (16/downscale)² 
        // downsampled samples. Half-res: 64 samples (8×8). Quarter-res: 
        // 16 samples (4×4). Each sample is the average of a downscale×downscale 
        // tile.
        let samples_per_axis = block_size / downscale;
        for by in 0..bh {
            for bx in 0..bw {
                let keep = block_keep[by * bw + bx];
                if keep {
                    for hpy in 0..samples_per_axis {
                        for hpx in 0..samples_per_axis {
                            let full_px0 = bx * block_size + hpx * downscale;
                            let full_py0 = by * block_size + hpy * downscale;
                            
                            // Average a downscale×downscale tile of actual and predicted.
                            let mut a_sum = 0i32;
                            let mut p_sum = 0i32;
                            let mut count = 0i32;
                            for dy in 0..downscale {
                                for dx in 0..downscale {
                                    let x = full_px0 + dx;
                                    let y = full_py0 + dy;
                                    if x >= w || y >= h { continue; }
                                    let i = y * w + x;
                                    a_sum += actual_yuv[i] as i32;
                                    p_sum += predicted[i] as i32;
                                    count += 1;
                                }
                            }
                            if count == 0 { continue; }
                            let a_avg = a_sum / count;
                            let p_avg = p_sum / count;
                            let d = (a_avg - p_avg) as i16;
                            let r = d.clamp(-128, 127) as i8;
                            let q = (r as i32 / quant_step as i32) as i8;
                            
                            // Store at downsampled position.
                            let half_x = full_px0 / downscale;
                            let half_y = full_py0 / downscale;
                            if half_x < hw && half_y < hh {
                                residual_quantized[half_y * hw + half_x] = q;
                            }
                            
                            // Track PSNR (using avg as proxy — close to per-pixel)
                            let dequant = q as i32 * quant_step as i32;
                            let err = (a_avg - p_avg) - dequant;
                            sum_sq_err += (err * err) as u64;
                            sampled += 1;
                        }
                    }
                } else {
                    // Skipped blocks: sample for PSNR
                    for py in (0..block_size).step_by(4) {
                        for px in (0..block_size).step_by(4) {
                            let x = bx * block_size + px;
                            let y = by * block_size + py;
                            if x >= w || y >= h { continue; }
                            let i = y * w + x;
                            let err = actual_yuv[i] as i32 - predicted[i] as i32;
                            sum_sq_err += (err * err) as u64;
                            sampled += 1;
                        }
                    }
                }
            }
        }
        let _ = kept_blocks;
        
        // Compute estimated PSNR from samples
        let mse = if sampled > 0 { sum_sq_err as f64 / sampled as f64 } else { 0.0 };
        let psnr = if mse > 0.0 {
            10.0 * (255.0 * 255.0 / mse).log10() as f32
        } else { 99.0 };
        
        // Flag bit layout:
        //   0x01 — half-res residual (legacy default)
        //   0x02 — motion-only frame (no residual)
        //   0x04 — quarter-res residual (FAO_V21_TINY=1, this session)
        // 
        // Quarter-res and half-res are mutually exclusive; setting both 
        // would confuse the decoder. We set exactly one per the chosen 
        // downscale.
        let res_flag: u8 = if downscale == 4 { 0x04 } else { 0x01 };
        Some((Self {
            anchor_ref,
            motion_vectors: mvs,
            residual_quantized,
            quant_step,
            flags: res_flag,
            filter_mask: Vec::new(),
        }, psnr))
    }
    
    /// Decode this TranslatedFrame using the anchor.
    pub fn decode_with_anchor(&self, anchor_yuv: &[u8], w: usize, h: usize) -> Option<Vec<u8>> {
        let y_sz = w * h;
        let uv_sz = y_sz / 2;
        if anchor_yuv.len() < y_sz + uv_sz { return None; }
        
        // Build prediction from anchor + MVs (motion compensation)
        let predicted = predict_from_motion(anchor_yuv, &self.motion_vectors, w, h);
        
        // Motion-only frame: no residual, prediction is the output
        if self.quant_step == 0 || self.residual_quantized.is_empty() {
            return Some(predicted);
        }
        
        // Standard path: de-quantize residual and add to prediction
        let mut output = predicted;
        
        // Flag-based downsample factor (encoder picks one):
        //   0x01 — half-res (2× downsample)
        //   0x04 — quarter-res (4× downsample, FAO_V21_TINY mode)
        //   neither — legacy full-res
        // 
        // The decoder reads the flag and picks the correct upsample 
        // factor. Older frames with flags=0x01 still decode at half-res, 
        // so files encoded before FAO_V21_TINY existed continue to play 
        // unchanged.
        let is_quarter_res = (self.flags & 0x04) != 0;
        let is_half_res = (self.flags & 0x01) != 0;
        
        if is_quarter_res {
            // Quarter-res residual: upsample 4×4 (each quarter-res 
            // pixel → 16 full-res pixels). Same delta value applied 
            // uniformly across the tile — block-averaged signal so 
            // upsample is just nearest-neighbor flood.
            let qw = w / 4;
            let qh = h / 4;
            let expected_q = qw * qh;
            if self.residual_quantized.len() < expected_q {
                return Some(output);
            }
            for qy in 0..qh {
                for qx in 0..qw {
                    let q = self.residual_quantized[qy * qw + qx];
                    if q == 0 { continue; }
                    let dequant = q as i32 * self.quant_step as i32;
                    for dy in 0..4 {
                        for dx in 0..4 {
                            let x = qx * 4 + dx;
                            let y = qy * 4 + dy;
                            if x < w && y < h {
                                let i = y * w + x;
                                let v = output[i] as i32 + dequant;
                                output[i] = v.clamp(0, 255) as u8;
                            }
                        }
                    }
                }
            }
        } else if is_half_res {
            // Half-res residual: upsample 2×2 (each half-res pixel → 4 full-res pixels)
            let hw = w / 2;
            let hh = h / 2;
            let expected_half = hw * hh;
            if self.residual_quantized.len() < expected_half {
                return Some(output);
            }
            for hy in 0..hh {
                for hx in 0..hw {
                    let q = self.residual_quantized[hy * hw + hx];
                    if q == 0 { continue; }
                    let dequant = q as i32 * self.quant_step as i32;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let x = hx * 2 + dx;
                            let y = hy * 2 + dy;
                            if x < w && y < h {
                                let i = y * w + x;
                                let v = output[i] as i32 + dequant;
                                output[i] = v.clamp(0, 255) as u8;
                            }
                        }
                    }
                }
            }
        } else {
            // Full-res residual (legacy path)
            let res_len = self.residual_quantized.len().min(y_sz);
            for i in 0..res_len {
                let dequant = self.residual_quantized[i] as i32 * self.quant_step as i32;
                let v = output[i] as i32 + dequant;
                output[i] = v.clamp(0, 255) as u8;
            }
        }
        
        // Chroma copied from anchor (predictive only)
        for i in y_sz..output.len().min(anchor_yuv.len()) {
            output[i] = anchor_yuv[i];
        }
        
        Some(output)
    }
    
    /// Serialize to bytes for storage in .fao file.
    /// Format ('T' marker, v2 with MV-RLE + zstd-compressed MVs):
    ///   'T' | w(2) | h(2) | anchor_ref(2) | quant_step(1) | n_mvs(2) 
    ///       | mvs_compressed_size(2) | mvs_data (zstd-compressed) 
    ///       | residual_compressed_size(4) | residual_data
    pub fn to_bytes(&self, w: usize, h: usize) -> Vec<u8> {
        // ── TINY-T FAST PATH ──
        // 
        // When the residual is empty AND every motion vector is zero, the 
        // frame is literally "use this anchor unchanged". Encoding ~36 
        // bytes of zstd-compressed zeros for that case is silly.
        // 
        // Compact form: 'T' + 't' + anchor_ref (u16) + dims (u16+u16) = 8 bytes.
        // The 't' subtype magic disambiguates from full-T (which has quant_step 
        // at byte 7 — 't' = 0x74 is unlikely to collide with realistic quant_step 
        // values and the from_bytes parser sniffs for it explicitly).
        // 
        // Savings: 49B → 8B = 84% smaller for static T-frames. A talking-head 
        // video with 80% static T-frames and 4179 frames saves:
        //   4179 × 0.8 × (49-8) = 137KB on the compressed file.
        // Modest absolute, but free — doesn't change anything else in the pipeline.
        let is_tiny = self.residual_quantized.is_empty()
            && self.motion_vectors.iter().all(|(dx, dy)| *dx == 0 && *dy == 0);
        if is_tiny {
            let mut out = Vec::with_capacity(8);
            out.push(b'T');
            out.push(b't');  // tiny subtype
            out.extend_from_slice(&(w as u16).to_le_bytes());
            out.extend_from_slice(&(h as u16).to_le_bytes());
            out.extend_from_slice(&self.anchor_ref.to_le_bytes());
            return out;
        }
        
        // Bit-pack MVs (Opus-style variable-length codes)
        // Zero MV: 1 bit. Small (±1..±3): 4 bits. Medium (±4..±31): 8 bits. Large: 12 bits
        // For game footage with mostly-zero MVs: ~1.2 bits avg per MV (8x denser)
        let mv_packed = crate::bitpack::pack_motion_vectors(&self.motion_vectors);
        
        // Optional: zstd over the bit-packed (often shrinks further on repeating patterns)
        let mvs_compressed = zstd::encode_all(Cursor::new(&mv_packed), 5)
            .unwrap_or_else(|_| mv_packed.clone());
        
        // RLE then zstd the residual (huge win: zeros compress to nothing)
        let rle = rle_encode(&self.residual_quantized);
        // zstd level 6: 2x faster than 12, only ~5% larger output for our sparse residuals
        let res_compressed = zstd::encode_all(Cursor::new(&rle), 6)
            .unwrap_or_else(|_| rle.clone());
        
        let mut out = Vec::with_capacity(17 + mvs_compressed.len() + res_compressed.len());
        out.push(b'T');
        out.extend_from_slice(&(w as u16).to_le_bytes());
        out.extend_from_slice(&(h as u16).to_le_bytes());
        out.extend_from_slice(&self.anchor_ref.to_le_bytes());
        out.push(self.quant_step);
        out.push(self.flags);  // NEW: flags byte
        out.extend_from_slice(&(self.motion_vectors.len() as u16).to_le_bytes());
        out.extend_from_slice(&(mvs_compressed.len() as u16).to_le_bytes());
        out.extend_from_slice(&mvs_compressed);
        out.extend_from_slice(&(res_compressed.len() as u32).to_le_bytes());
        out.extend_from_slice(&res_compressed);
        out
    }
    
    pub fn from_bytes(data: &[u8]) -> Option<(Self, usize, usize)> {
        if data.is_empty() || data[0] != b'T' { return None; }
        
        // ── TINY-T DECODE ──
        // Disambiguated by byte 1 == 't' (subtype magic). The full-T format 
        // never has 't' (0x74) at byte 1: byte 1 is the low byte of width, 
        // which is either > 4096 or pixel-aligned and not 0x74.
        // 
        // Wait — "t" is 0x74 = 116, which IS a possible low byte of small 
        // widths (e.g. 1908, 1908 % 256 = 116). So we need a stronger check: 
        // tiny-T is exactly 8 bytes. Real T is always > 13 bytes. Length 
        // alone disambiguates.
        if data.len() == 8 && data[1] == b't' {
            let w = u16::from_le_bytes([data[2], data[3]]) as usize;
            let h = u16::from_le_bytes([data[4], data[5]]) as usize;
            let anchor_ref = u16::from_le_bytes([data[6], data[7]]);
            return Some((
                Self {
                    anchor_ref,
                    quant_step: 0,
                    flags: 0,
                    motion_vectors: Vec::new(),
                    residual_quantized: Vec::new(),
                    filter_mask: Vec::new(),
                },
                w, h,
            ));
        }
        
        if data.len() < 13 { return None; }
        let w = u16::from_le_bytes([data[1], data[2]]) as usize;
        let h = u16::from_le_bytes([data[3], data[4]]) as usize;
        let anchor_ref = u16::from_le_bytes([data[5], data[6]]);
        let quant_step = data[7];
        let flags = data[8];  // NEW: flags byte
        let n_mvs = u16::from_le_bytes([data[9], data[10]]) as usize;
        let mvs_sz = u16::from_le_bytes([data[11], data[12]]) as usize;
        let mut pos = 13;
        
        // Read bit-packed MVs (decompress then unpack)
        if pos + mvs_sz > data.len() { return None; }
        let mvs_compressed = &data[pos..pos + mvs_sz];
        let mv_packed = zstd::decode_all(Cursor::new(mvs_compressed)).ok()?;
        let mvs = crate::bitpack::unpack_motion_vectors(&mv_packed)?;
        if mvs.len() != n_mvs {
            // Length mismatch — format inconsistency
            return None;
        }
        pos += mvs_sz;
        
        // Read compressed residual
        if pos + 4 > data.len() { return None; }
        let res_sz = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
        pos += 4;
        if pos + res_sz > data.len() { return None; }
        let res_compressed = &data[pos..pos + res_sz];
        let rle = zstd::decode_all(Cursor::new(res_compressed)).ok()?;
        let residual_quantized = rle_decode(&rle)?;
        
        Some((Self {
            anchor_ref,
            motion_vectors: mvs,
            residual_quantized,
            quant_step,
            flags,
            filter_mask: Vec::new(),  // not yet read from format
        }, w, h))
    }
}

/// Predict a frame by warping anchor with motion vectors.
fn predict_from_motion(anchor_yuv: &[u8], mvs: &[(i8, i8)], w: usize, h: usize) -> Vec<u8> {
    let y_sz = w * h;
    
    // FAST PATH: if all MVs are zero, the prediction IS the anchor
    // (just clone, no warp needed) — common case for static UI/scenes
    if mvs.is_empty() || mvs.iter().all(|(dx, dy)| *dx == 0 && *dy == 0) {
        return anchor_yuv.to_vec();
    }
    
    let mut out = anchor_yuv.to_vec();
    let bw = w / 16;
    let _bh = h / 16;
    
    // Warp Y plane block by block (only blocks with non-zero MV)
    for (bi, (dx, dy)) in mvs.iter().enumerate() {
        if *dx == 0 && *dy == 0 { continue; }  // already correct from copy
        let by = bi / bw;
        let bx = bi % bw;
        if bx >= bw { continue; }
        
        for py in 0..16 {
            for px in 0..16 {
                let dst_x = bx * 16 + px;
                let dst_y = by * 16 + py;
                if dst_x >= w || dst_y >= h { continue; }
                
                let sx = (dst_x as i32 + *dx as i32).clamp(0, w as i32 - 1) as usize;
                let sy = (dst_y as i32 + *dy as i32).clamp(0, h as i32 - 1) as usize;
                
                out[dst_y * w + dst_x] = anchor_yuv[sy * w + sx];
            }
        }
    }
    
    let _ = y_sz;
    out
}

/// RLE encode signed bytes with u16 run lengths (max 65535 per run).
/// Format: header(4: total_len) + pairs of (u16 count LE, i8 value)
/// Better than u8 RLE for residuals with long zero runs (common with block skipping)
fn rle_encode(data: &[i8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 16 + 16);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    
    let mut i = 0;
    while i < data.len() {
        let v = data[i];
        let mut run = 1usize;
        while i + run < data.len() && data[i + run] == v && run < 65535 {
            run += 1;
        }
        out.extend_from_slice(&(run as u16).to_le_bytes());
        out.push(v as u8);
        i += run;
    }
    out
}

fn rle_decode(data: &[u8]) -> Option<Vec<i8>> {
    if data.len() < 4 { return None; }
    let total = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let mut out = Vec::with_capacity(total);
    let mut pos = 4;
    while pos + 2 < data.len() && out.len() < total {
        let run = u16::from_le_bytes([data[pos], data[pos+1]]) as usize;
        let val = data[pos+2] as i8;
        pos += 3;
        for _ in 0..run {
            out.push(val);
            if out.len() >= total { break; }
        }
    }
    if out.len() == total { Some(out) } else { None }
}

/// Decision: should this frame use translate (Tier 1) encoding?
/// Returns Some((TranslatedFrame, estimated_size_bytes, estimated_psnr)) if viable,
/// None if full encode is recommended instead.
pub fn try_translate_frame(
    actual_yuv: &[u8],
    anchor_yuv: &[u8],
    w: usize, h: usize,
    anchor_ref: u16,
    quality_target: u8,
    full_encode_size: usize,
) -> Option<(TranslatedFrame, usize, f32)> {
    let (frame, psnr) = TranslatedFrame::encode_from_anchor(
        actual_yuv, anchor_yuv, w, h, anchor_ref, quality_target
    )?;
    
    let serialized = frame.to_bytes(w, h);
    let est_size = serialized.len();
    
    // Reject if translation isn't smaller than full encode
    if est_size >= full_encode_size {
        return None;
    }
    
    // Reject only if quality is GARBAGE (< 8dB = essentially random pixels)
    // Even low-PSNR T-frames are usually better than yuv_split fallback in size,
    // and visual quality often improves with subsequent better-anchored frames.
    if psnr < 8.0 {
        return None;
    }
    
    Some((frame, est_size, psnr))
}


// ═══════════════════════════════════════════════════════════
// Game-Aware Compression — exploit UI/HUD static regions
// ═══════════════════════════════════════════════════════════
//
// Game footage has invariants:
//   - HUD elements (health bars, ability icons, scoreboard) are pixel-stable
//   - Map borders, fonts, frames are static
//   - Background is mostly stable except for animated lighting
//
// Detection: scan a window of frames, mark blocks that NEVER change.
// Store these blocks ONCE in a "stable region" plate.
// Per-frame data only stores the regions that DO change.

#[derive(Clone, Debug)]
pub struct StableRegionPlate {
    pub width: u16,
    pub height: u16,
    pub block_size: u8,           // typically 16
    /// Mask: 1 bit per block, 1 = block is stable across all frames
    pub stable_mask: Vec<u8>,     // packed bitmask
    /// Compressed YUV pixels for stable blocks only
    pub stable_blocks: Vec<u8>,
}

impl StableRegionPlate {
    /// Analyze a window of frames to find stable regions.
    /// Returns the plate plus a "delta-only" version of each frame.
    /// The delta frames only encode pixels NOT in the stable mask.
    pub fn analyze_window(
        frames: &[&[u8]],   // YUV frames in the window
        w: usize, h: usize,
    ) -> Option<Self> {
        if frames.len() < 2 { return None; }
        let block_size = 16usize;
        let bw = w / block_size;
        let bh = h / block_size;
        let n_blocks = bw * bh;
        
        let y_sz = w * h;
        if frames[0].len() < y_sz { return None; }
        
        // For each block, check if ALL frames have nearly identical content
        let mut stable_mask = vec![0u8; (n_blocks + 7) / 8];
        let mut stable_count = 0u32;
        
        for by in 0..bh {
            for bx in 0..bw {
                let block_idx = by * bw + bx;
                let mut is_stable = true;
                
                // Compare frame 0 against all others
                for f_idx in 1..frames.len() {
                    let mut max_diff = 0u32;
                    for py in 0..block_size {
                        for px in 0..block_size {
                            let x = bx * block_size + px;
                            let y = by * block_size + py;
                            if x >= w || y >= h { continue; }
                            let i = y * w + x;
                            if i >= frames[0].len() || i >= frames[f_idx].len() { continue; }
                            let d = (frames[0][i] as i32 - frames[f_idx][i] as i32).unsigned_abs();
                            if d > max_diff { max_diff = d; }
                        }
                    }
                    // Threshold: any pixel differing by more than 4 = block is dynamic
                    if max_diff > 4 {
                        is_stable = false;
                        break;
                    }
                }
                
                if is_stable {
                    stable_mask[block_idx / 8] |= 1 << (block_idx % 8);
                    stable_count += 1;
                }
            }
        }
        
        if stable_count == 0 { return None; }  // nothing stable, no benefit
        
        // Extract stable block pixels from frame 0 (representative)
        let mut stable_pixels = Vec::with_capacity(stable_count as usize * block_size * block_size);
        for by in 0..bh {
            for bx in 0..bw {
                let block_idx = by * bw + bx;
                if stable_mask[block_idx / 8] & (1 << (block_idx % 8)) == 0 { continue; }
                
                for py in 0..block_size {
                    for px in 0..block_size {
                        let x = bx * block_size + px;
                        let y = by * block_size + py;
                        if x < w && y < h {
                            stable_pixels.push(frames[0][y * w + x]);
                        } else {
                            stable_pixels.push(0);
                        }
                    }
                }
            }
        }
        
        // Compress the stable pixels — they typically have lots of repetition
        let stable_blocks = zstd::encode_all(std::io::Cursor::new(&stable_pixels), 12)
            .unwrap_or(stable_pixels);
        
        Some(Self {
            width: w as u16,
            height: h as u16,
            block_size: block_size as u8,
            stable_mask,
            stable_blocks,
        })
    }
    
    pub fn stable_block_ratio(&self) -> f32 {
        let total_bits = self.stable_mask.len() * 8;
        if total_bits == 0 { return 0.0; }
        let set_bits: u32 = self.stable_mask.iter().map(|b| b.count_ones()).sum();
        set_bits as f32 / total_bits as f32
    }
    
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + self.stable_mask.len() + self.stable_blocks.len());
        out.extend_from_slice(b"GAMEPLT1");
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        out.push(self.block_size);
        out.extend_from_slice(&(self.stable_mask.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.stable_mask);
        out.extend_from_slice(&(self.stable_blocks.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.stable_blocks);
        out
    }
}

// ═══════════════════════════════════════════════════════════
// Bilateral Filter — edge-preserving smoothing for residual cleanup
// ═══════════════════════════════════════════════════════════
//
// Reduces noise without blurring edges. Applied to source frames
// BEFORE delta computation: smaller residuals → smaller T-frames.
// Also reduces compression-noise feedback in the delta.

/// Apply a small bilateral filter to a Y plane in-place.
/// kernel_radius=2 → 5x5 window, fast enough for real-time encode.
/// sigma_space and sigma_intensity control edge preservation.
pub fn bilateral_filter_y(
    y_plane: &mut [u8],
    w: usize, h: usize,
    radius: usize,
    sigma_intensity: f32,
) {
    if w < radius * 2 + 1 || h < radius * 2 + 1 { return; }
    
    let original = y_plane.to_vec();
    let inv_2sigma_int_sq = 1.0 / (2.0 * sigma_intensity * sigma_intensity);
    
    for y in radius..(h - radius) {
        for x in radius..(w - radius) {
            let center = original[y * w + x] as f32;
            let mut sum = 0.0f32;
            let mut weight_sum = 0.0f32;
            
            for ky in 0..=(2 * radius) {
                for kx in 0..=(2 * radius) {
                    let nx = x + kx - radius;
                    let ny = y + ky - radius;
                    let neighbor = original[ny * w + nx] as f32;
                    
                    // Spatial weight (precomputed could help, but radius is small)
                    let dx = (kx as i32 - radius as i32) as f32;
                    let dy = (ky as i32 - radius as i32) as f32;
                    let spatial_w = (-(dx*dx + dy*dy) * 0.125).exp();  // sigma_space=2
                    
                    // Intensity weight (preserves edges)
                    let int_diff = neighbor - center;
                    let intensity_w = (-int_diff * int_diff * inv_2sigma_int_sq).exp();
                    
                    let w = spatial_w * intensity_w;
                    sum += neighbor * w;
                    weight_sum += w;
                }
            }
            
            if weight_sum > 0.0 {
                y_plane[y * w + x] = (sum / weight_sum).clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Apply unsharp masking to enhance edges in decoded frames.
/// Compensates for any blur introduced by interpolation/motion comp.
pub fn unsharp_mask_y(
    y_plane: &mut [u8],
    w: usize, h: usize,
    strength: f32,  // 0..1, typical 0.3-0.5
) {
    if w < 3 || h < 3 { return; }
    let original = y_plane.to_vec();
    
    for y in 1..(h - 1) {
        for x in 1..(w - 1) {
            // 3x3 box blur (cheap)
            let mut sum = 0u32;
            for dy in 0..3 {
                for dx in 0..3 {
                    sum += original[(y + dy - 1) * w + (x + dx - 1)] as u32;
                }
            }
            let blurred = (sum / 9) as f32;
            let center = original[y * w + x] as f32;
            // Sharpen: original + strength * (original - blurred)
            let sharpened = center + strength * (center - blurred);
            y_plane[y * w + x] = sharpened.clamp(0.0, 255.0) as u8;
        }
    }
}
