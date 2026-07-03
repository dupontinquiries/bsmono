// Pixel pre-filters for analysis and compression optimization
//
// IDEA: Different content compresses best with different pre-filters.
//   - Smooth gradients → denoise filter (removes JPEG noise, helps zstd)
//   - Sharp UI edges → edge-preserve filter (keeps crispness, removes noise)
//   - Solid color regions → quantize filter (rounds to nearest N, ultra compressible)
//   - Textured regions → identity (no filter, preserve detail)
//
// PIPELINE:
//   1. Apply each filter to a block
//   2. Compute SAD vs prediction for each
//   3. Pick the filter with lowest residual + filter overhead
//   4. Encode: 2-bit filter_id + residual data
//   5. Decoder: applies inverse-filter (or just uses filtered prediction)
//
// MARKING:
//   - filter_used: u8 per block (which filter won)
//   - residual_after: u32 SAD per block (compression potential)
//   - These get passed to web viz for visualization

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PixelFilter {
    Identity = 0,        // no filter (preserves all detail)
    Denoise3x3 = 1,      // 3x3 average (smooths noise)
    Quantize8 = 2,       // round to nearest 8 (lossy but ultra-compressible)
    EdgePreserve = 3,    // 3x3 median (keeps edges, removes salt-pepper noise)
}

impl PixelFilter {
    pub fn from_u8(v: u8) -> Self {
        match v & 0x03 {
            0 => Self::Identity,
            1 => Self::Denoise3x3,
            2 => Self::Quantize8,
            _ => Self::EdgePreserve,
        }
    }
    
    pub fn to_u8(self) -> u8 {
        self as u8
    }
}

/// Apply a 3x3 average filter to a Y plane (in-place)
pub fn apply_denoise_3x3(pixels: &mut [u8], w: usize, h: usize) {
    let mut output = vec![0u8; pixels.len()];
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let mut sum = 0u32;
            for dy in 0..3 {
                for dx in 0..3 {
                    sum += pixels[(y + dy - 1) * w + (x + dx - 1)] as u32;
                }
            }
            output[y * w + x] = (sum / 9) as u8;
        }
    }
    // Copy borders unchanged
    for x in 0..w {
        output[x] = pixels[x];
        if h > 0 { output[(h - 1) * w + x] = pixels[(h - 1) * w + x]; }
    }
    for y in 0..h {
        output[y * w] = pixels[y * w];
        if w > 0 { output[y * w + w - 1] = pixels[y * w + w - 1]; }
    }
    pixels.copy_from_slice(&output);
}

/// Quantize pixels to nearest multiple of N (lossy, very compressible)
pub fn apply_quantize(pixels: &mut [u8], step: u8) {
    if step <= 1 { return; }
    let half = step / 2;
    for p in pixels.iter_mut() {
        let q = (*p / step) * step + half;
        *p = q.min(255);
    }
}

/// Compute SAD between two same-sized byte slices
pub fn block_sad(a: &[u8], b: &[u8]) -> u32 {
    let n = a.len().min(b.len());
    let mut sum = 0u32;
    for i in 0..n {
        sum += (a[i] as i32 - b[i] as i32).unsigned_abs();
    }
    sum
}

/// Apply 3x3 median filter to a single 16x16 block (extract, median, write back)
pub fn apply_median_3x3_block(block: &mut [u8; 256]) {
    let mut out = [0u8; 256];
    for y in 0..16 {
        for x in 0..16 {
            let mut window = [0u8; 9];
            let mut count = 0;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx >= 0 && nx < 16 && ny >= 0 && ny < 16 {
                        window[count] = block[(ny as usize) * 16 + (nx as usize)];
                        count += 1;
                    }
                }
            }
            // Sort and take median
            let used = &mut window[..count];
            used.sort_unstable();
            out[y * 16 + x] = used[count / 2];
        }
    }
    block.copy_from_slice(&out);
}

/// Apply 3x3 average filter to a single 16x16 block
pub fn apply_denoise_3x3_block(block: &mut [u8; 256]) {
    let mut out = [0u8; 256];
    for y in 0..16 {
        for x in 0..16 {
            let mut sum = 0u32;
            let mut count = 0u32;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx >= 0 && nx < 16 && ny >= 0 && ny < 16 {
                        sum += block[(ny as usize) * 16 + (nx as usize)] as u32;
                        count += 1;
                    }
                }
            }
            out[y * 16 + x] = (sum / count.max(1)) as u8;
        }
    }
    block.copy_from_slice(&out);
}

/// Apply quantize filter to a 16x16 block in-place
pub fn apply_quantize_block(block: &mut [u8; 256], step: u8) {
    if step <= 1 { return; }
    let half = step / 2;
    for p in block.iter_mut() {
        let q = (*p / step) * step + half;
        *p = q.min(255);
    }
}

/// For a 16x16 block, try all 4 filters and return (best_filter, sad_to_target).
/// 
/// `target` is what we WANT the block to look like (e.g., the actual frame).
/// `predicted` is what we have (e.g., motion-compensated anchor).
/// 
/// We try each filter on the predicted block and see which gets closest to target.
/// Returns: (winner_filter, sad_after_filter).
pub fn find_best_filter(
    target: &[u8; 256],
    predicted: &[u8; 256],
) -> (PixelFilter, u32) {
    // 1. Identity: no filter
    let sad_identity = {
        let mut sum = 0u32;
        for i in 0..256 {
            sum += (target[i] as i32 - predicted[i] as i32).unsigned_abs();
        }
        sum
    };
    
    // 2. Denoise: filter PREDICTED, then SAD
    let mut filtered = *predicted;
    apply_denoise_3x3_block(&mut filtered);
    let sad_denoise = {
        let mut sum = 0u32;
        for i in 0..256 {
            sum += (target[i] as i32 - filtered[i] as i32).unsigned_abs();
        }
        sum
    };
    
    // 3. Quantize: round target to step 8, store steps (very compressible)
    let mut q_target = *target;
    apply_quantize_block(&mut q_target, 8);
    let sad_quantize = {
        let mut sum = 0u32;
        for i in 0..256 {
            sum += (q_target[i] as i32 - predicted[i] as i32).unsigned_abs();
        }
        sum
    };
    
    // 4. Edge-preserve (median)
    let mut median = *predicted;
    apply_median_3x3_block(&mut median);
    let sad_median = {
        let mut sum = 0u32;
        for i in 0..256 {
            sum += (target[i] as i32 - median[i] as i32).unsigned_abs();
        }
        sum
    };
    
    // Pick winner with bias toward Identity (filter has overhead)
    // Identity wins ties — prefer simpler when comparable.
    let filter_overhead = 8u32;  // ~1 byte per block for filter id
    let mut best_filter = PixelFilter::Identity;
    let mut best_sad = sad_identity;
    
    if sad_denoise + filter_overhead < best_sad {
        best_sad = sad_denoise + filter_overhead;
        best_filter = PixelFilter::Denoise3x3;
    }
    if sad_quantize + filter_overhead < best_sad {
        best_sad = sad_quantize + filter_overhead;
        best_filter = PixelFilter::Quantize8;
    }
    if sad_median + filter_overhead < best_sad {
        best_sad = sad_median + filter_overhead;
        best_filter = PixelFilter::EdgePreserve;
    }
    
    (best_filter, best_sad)
}

/// Extract a 16x16 block from a Y plane at (bx, by) into a fixed array
pub fn extract_block(yuv: &[u8], w: usize, h: usize, bx: usize, by: usize) -> [u8; 256] {
    let mut block = [0u8; 256];
    for py in 0..16 {
        for px in 0..16 {
            let x = bx * 16 + px;
            let y = by * 16 + py;
            if x < w && y < h {
                let i = y * w + x;
                if i < yuv.len() {
                    block[py * 16 + px] = yuv[i];
                }
            }
        }
    }
    block
}

/// Stats from running filter analysis on a frame
#[derive(Default, Clone)]
pub struct FilterAnalysis {
    /// Per-block winning filter (one byte per block)
    pub filter_used: Vec<u8>,
    /// Distribution: how many blocks used each filter
    pub filter_counts: [u32; 4],
    /// Total SAD reduction vs identity (estimates compression win)
    pub sad_reduction: i64,
    /// Total blocks analyzed
    pub n_blocks: u32,
}

impl FilterAnalysis {
    pub fn percent(&self, filter: PixelFilter) -> f32 {
        if self.n_blocks == 0 { return 0.0; }
        self.filter_counts[filter as usize] as f32 / self.n_blocks as f32 * 100.0
    }
}

/// Analyze all blocks in a frame, picking best filter for each
pub fn analyze_frame_filters(
    target: &[u8],
    predicted: &[u8],
    w: usize, h: usize,
) -> FilterAnalysis {
    let bw = w / 16;
    let bh = h / 16;
    let n_blocks = bw * bh;
    
    let mut analysis = FilterAnalysis {
        filter_used: vec![0u8; n_blocks],
        filter_counts: [0; 4],
        sad_reduction: 0,
        n_blocks: n_blocks as u32,
    };
    
    for by in 0..bh {
        for bx in 0..bw {
            let target_block = extract_block(target, w, h, bx, by);
            let pred_block = extract_block(predicted, w, h, bx, by);
            
            // SAD without filter (baseline)
            let mut sad_identity = 0u32;
            for i in 0..256 {
                sad_identity += (target_block[i] as i32 - pred_block[i] as i32).unsigned_abs();
            }
            
            let (filter, sad_filtered) = find_best_filter(&target_block, &pred_block);
            
            let bi = by * bw + bx;
            analysis.filter_used[bi] = filter.to_u8();
            analysis.filter_counts[filter as usize] += 1;
            analysis.sad_reduction += sad_identity as i64 - sad_filtered as i64;
        }
    }
    
    analysis
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_filter_roundtrip() {
        let mut pixels = [128u8; 256];
        let original = pixels;
        
        apply_denoise_3x3_block(&mut pixels);
        // For uniform pixels, denoise should be no-op
        assert_eq!(pixels, original);
    }
    
    #[test]
    fn test_quantize() {
        let mut pixels = [42u8, 100, 200, 250].repeat(64);
        let mut a: [u8; 256] = [0; 256];
        a.copy_from_slice(&pixels[..256]);
        apply_quantize_block(&mut a, 16);
        // Each value should be a multiple of 16 + 8 (= midpoint)
        for v in a.iter() {
            assert_eq!((*v - 8) % 16, 0);
        }
    }
}
