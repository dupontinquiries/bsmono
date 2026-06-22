//! # basic_upscaler — the placeholder `BlockProcessor`
//!
//! Fills the swappable processing hole with a basic, in-process,
//! no-neural-net upscaler so the pipeline is real end-to-end NOW. A
//! higher-precision component (ESRGAN/RefSR, possibly out-of-process)
//! drops in behind the same `BlockProcessor` trait later.
//!
//! Strategy:
//!  - If a reference block of matching content/geometry is available,
//!    prefer transferring the reference's (cleaner) pixels — this is the
//!    "cross-reference" path, the toy version of reference-based SR.
//!  - Otherwise, upscale the block by integer factor via bilinear
//!    interpolation — the "blind" fallback.
//!
//! This is deliberately simple and honest: it does NOT hallucinate
//! detail. Reference transfer copies real pixels; bilinear smooths. When
//! a real super-resolution model replaces this, the contract (BlockId
//! preservation, dedup fan-out, provenance) is unchanged.

use crate::blockproc::{Block, BlockError, BlockFormat, BlockProcessor};

pub struct BasicUpscaler {
    /// Integer upscale factor (1 = no resize, 2 = double each dimension).
    factor: u32,
    /// If true, prefer reference-block transfer when a match is found.
    use_references: bool,
}

impl BasicUpscaler {
    pub fn new(factor: u32, use_references: bool) -> Self {
        Self { factor: factor.max(1), use_references }
    }
}

impl BlockProcessor for BasicUpscaler {
    fn name(&self) -> &str { "basic-upscaler" }
    
    fn scale_factor(&self) -> u32 { self.factor }
    
    fn process(
        &self,
        blocks: &[Block],
        references: &[Block],
    ) -> Result<Vec<Block>, BlockError> {
        let mut out = Vec::with_capacity(blocks.len());
        for block in blocks {
            // Reference path: if enabled and a reference of the same
            // geometry+format exists whose content is "close" to this
            // block, transfer the reference pixels (upscaled to match).
            let processed = if self.use_references {
                match find_reference_match(block, references) {
                    Some(reference) => transfer_from_reference(block, reference, self.factor)?,
                    None => upscale_bilinear(block, self.factor)?,
                }
            } else {
                upscale_bilinear(block, self.factor)?
            };
            out.push(processed);
        }
        Ok(out)
    }
}

/// Find a reference block that plausibly matches this block's content.
/// Toy heuristic: same format, and reference dimensions are an integer
/// multiple of the block's (i.e. the reference IS a higher-res version
/// of the same region). Content similarity uses a cheap mean-absolute-
/// difference on a downsampled comparison. Returns the best match under
/// a similarity threshold, else None.
///
/// A real RefSR model would do learned patch matching here; this is the
/// honest placeholder.
fn find_reference_match<'a>(block: &Block, references: &'a [Block]) -> Option<&'a Block> {
    let mut best: Option<(&Block, f64)> = None;
    for r in references {
        if r.format != block.format { continue; }
        // Reference should be same or higher resolution.
        if r.width < block.width || r.height < block.height { continue; }
        let score = content_distance(block, r);
        match best {
            Some((_, b)) if score >= b => {}
            _ => best = Some((r, score)),
        }
    }
    // Threshold: only use the reference if it's reasonably close. The
    // distance is normalized 0..1 (0 = identical). 0.15 is a loose
    // "same content, different quality" band for the placeholder.
    best.filter(|(_, score)| *score < 0.15).map(|(r, _)| r)
}

/// Cheap normalized content distance between a block and a (possibly
/// larger) reference. Compares the block against the reference's
/// top-left region at the block's resolution via mean absolute diff on
/// the first channel. 0 = identical, 1 = maximally different.
fn content_distance(block: &Block, reference: &Block) -> f64 {
    let bpp = block.format.bytes_per_pixel();
    let max_val = if matches!(block.format, BlockFormat::Gray16 | BlockFormat::Rgb16) {
        65535.0
    } else {
        255.0
    };
    let mut total = 0.0f64;
    let mut count = 0usize;
    // Sample the block's pixels and compare to the reference's
    // corresponding (nearest) pixel. Stride to keep it cheap.
    let stride = (block.width / 8).max(1);
    let mut y = 0;
    while y < block.height {
        let mut x = 0;
        while x < block.width {
            let bi = ((y * block.width + x) as usize) * bpp;
            // Map block coord into reference (reference is >= block size).
            let rx = x * reference.width / block.width;
            let ry = y * reference.height / block.height;
            let ri = ((ry * reference.width + rx) as usize) * bpp;
            if bi < block.data.len() && ri < reference.data.len() {
                let bv = block.data[bi] as f64;
                let rv = reference.data[ri] as f64;
                total += (bv - rv).abs() / max_val;
                count += 1;
            }
            x += stride;
        }
        y += stride;
    }
    if count == 0 { 1.0 } else { total / count as f64 }
}

/// Transfer pixels from a reference into the output, scaled to the
/// block's target output size (block dims * factor). Since the reference
/// is the cleaner/higher-res source, we sample it directly. Returns a
/// block carrying the ORIGINAL block's id (provenance preserved).
fn transfer_from_reference(
    block: &Block, reference: &Block, factor: u32,
) -> Result<Block, BlockError> {
    let out_w = block.width * factor;
    let out_h = block.height * factor;
    let bpp = block.format.bytes_per_pixel();
    let mut data = vec![0u8; (out_w * out_h) as usize * bpp];
    
    // For each output pixel, sample the reference at the corresponding
    // normalized position (nearest-neighbor — the reference is already
    // high quality, so we just resample it to the target grid).
    for oy in 0..out_h {
        for ox in 0..out_w {
            let rx = ox * reference.width / out_w;
            let ry = oy * reference.height / out_h;
            let ri = ((ry * reference.width + rx) as usize) * bpp;
            let oi = ((oy * out_w + ox) as usize) * bpp;
            for c in 0..bpp {
                if ri + c < reference.data.len() && oi + c < data.len() {
                    data[oi + c] = reference.data[ri + c];
                }
            }
        }
    }
    Block::new(block.id, out_w, out_h, block.format, data)
}

/// Nearest-neighbor upscale — byte-faithful, format-agnostic. Used for
/// 16-bit blocks where the bilinear path's 8-bit clamp would corrupt
/// data. Honest placeholder behavior.
fn upscale_nearest(block: &Block, factor: u32) -> Result<Block, BlockError> {
    let out_w = block.width * factor;
    let out_h = block.height * factor;
    let bpp = block.format.bytes_per_pixel();
    let mut data = vec![0u8; (out_w * out_h) as usize * bpp];
    for oy in 0..out_h {
        for ox in 0..out_w {
            let sx = ox / factor;
            let sy = oy / factor;
            let si = ((sy * block.width + sx) as usize) * bpp;
            let oi = ((oy * out_w + ox) as usize) * bpp;
            for c in 0..bpp {
                if si + c < block.data.len() && oi + c < data.len() {
                    data[oi + c] = block.data[si + c];
                }
            }
        }
    }
    Block::new(block.id, out_w, out_h, block.format, data)
}

/// Bilinear upscale of a block by integer factor. The blind fallback
/// when no reference matches. Operates per channel.
fn upscale_bilinear(block: &Block, factor: u32) -> Result<Block, BlockError> {
    if factor == 1 {
        return Ok(block.clone());
    }
    // The bilinear path below clamps to 0..255, which is only correct
    // for 8-bit formats. For 16-bit, the per-byte interpolation + 255
    // clamp would corrupt high bytes — so 16-bit uses nearest-neighbor
    // (a byte-faithful resize) until the real model replaces this.
    if matches!(block.format, BlockFormat::Gray16 | BlockFormat::Rgb16) {
        return upscale_nearest(block, factor);
    }
    let out_w = block.width * factor;
    let out_h = block.height * factor;
    let bpp = block.format.bytes_per_pixel();
    let mut data = vec![0u8; (out_w * out_h) as usize * bpp];
    
    let sample = |x: i64, y: i64, c: usize| -> f64 {
        let xi = x.clamp(0, block.width as i64 - 1) as u32;
        let yi = y.clamp(0, block.height as i64 - 1) as u32;
        let idx = ((yi * block.width + xi) as usize) * bpp + c;
        *block.data.get(idx).unwrap_or(&0) as f64
    };
    
    for oy in 0..out_h {
        for ox in 0..out_w {
            // Map output pixel center back to source space.
            let sx = (ox as f64 + 0.5) / factor as f64 - 0.5;
            let sy = (oy as f64 + 0.5) / factor as f64 - 0.5;
            let x0 = sx.floor() as i64;
            let y0 = sy.floor() as i64;
            let fx = sx - x0 as f64;
            let fy = sy - y0 as f64;
            for c in 0..bpp {
                let p00 = sample(x0, y0, c);
                let p10 = sample(x0 + 1, y0, c);
                let p01 = sample(x0, y0 + 1, c);
                let p11 = sample(x0 + 1, y0 + 1, c);
                let top = p00 + (p10 - p00) * fx;
                let bot = p01 + (p11 - p01) * fx;
                let val = top + (bot - top) * fy;
                let oi = ((oy * out_w + ox) as usize) * bpp + c;
                data[oi] = val.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Block::new(block.id, out_w, out_h, block.format, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockproc::BlockId;
    
    fn solid_block(w: u32, h: u32, fill: u8) -> Block {
        Block::new(
            BlockId { clip: 0, frame: 0, x: 0, y: 0 },
            w, h, BlockFormat::Gray8, vec![fill; (w * h) as usize],
        ).unwrap()
    }
    
    #[test]
    fn bilinear_upscale_doubles_dims() {
        let b = solid_block(4, 4, 128);
        let up = upscale_bilinear(&b, 2).unwrap();
        assert_eq!(up.width, 8);
        assert_eq!(up.height, 8);
        // Solid block stays solid after upscale.
        assert!(up.data.iter().all(|&p| p == 128));
        // Id preserved.
        assert_eq!(up.id, b.id);
    }
    
    #[test]
    fn factor_one_is_identity() {
        let b = solid_block(4, 4, 77);
        let up = upscale_bilinear(&b, 1).unwrap();
        assert_eq!(up.data, b.data);
    }
    
    #[test]
    fn processor_preserves_ids() {
        let up = BasicUpscaler::new(2, false);
        let blocks = vec![
            Block::new(BlockId { clip: 1, frame: 5, x: 16, y: 32 },
                4, 4, BlockFormat::Gray8, vec![100; 16]).unwrap(),
        ];
        let out = up.process(&blocks, &[]).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, BlockId { clip: 1, frame: 5, x: 16, y: 32 });
        assert_eq!(out[0].width, 8);  // 4 * factor 2
    }
    
    #[test]
    fn reference_match_prefers_close_content() {
        // A degraded block (fill 100) and a reference of same content
        // (fill 102 — close) at higher res should match.
        let block = solid_block(4, 4, 100);
        let mut reference = Block::new(
            BlockId { clip: 99, frame: 0, x: 0, y: 0 },
            8, 8, BlockFormat::Gray8, vec![102; 64],
        ).unwrap();
        let refs = vec![reference.clone()];
        assert!(find_reference_match(&block, &refs).is_some());
        
        // A reference of very different content (fill 250) should NOT match.
        reference.data = vec![250; 64];
        let refs2 = vec![reference];
        assert!(find_reference_match(&block, &refs2).is_none());
    }
}
