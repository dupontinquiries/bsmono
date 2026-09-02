//! Analysis: given the previous frame (as the engine has reconstructed it)
//! and the next frame, decide for every block whether it can be reused.
//!
//! Order matters and mirrors upstream's `driver.h`: fade first (it modifies
//! the previous frame), then matching against the faded previous frame.
//! The merge stage replays the same order in the upscaled domain.
//!
//! The one deliberate departure: the low-resolution reconstruction of the
//! next frame (`working_next`) is what the *upscaled output actually
//! represents* — matched blocks are the previous frame's pixels, not the
//! next frame's. Upstream builds that reconstruction too (`update_frame`) but
//! also applies it in the redraw case, where the output is a plain upscale
//! of the original; here redraw keeps the original so the two domains
//! never disagree.

use crate::frame::{block_sse, block_sse_bounded, Frame};
use crate::gate::{ReferenceMaker, ThresholdMap};
use crate::matcher::BlockMatcher;
use crate::plan::{BlockMatch, FadeBlock, FramePlan, FrameStats, Grid, ResidualVector};
use rayon::prelude::*;

#[derive(Debug, Clone, Copy)]
pub struct AnalyzeConfig {
    pub block_size: u32,
    pub bleed: u32,
    /// Below this PSNR between consecutive frames, skip matching entirely
    /// (scene cut). Upstream gates at 10 dB of its own 3-channel-summed
    /// figure, which is ≈5.2 dB of true PSNR; this is only a shortcut past
    /// matching, `redraw_fraction` is the decision that matters.
    pub min_psnr: f64,
    /// Redraw the whole frame when the packed residual would cover at least
    /// this fraction of the frame's pixels (it can exceed 1.0: bleed makes
    /// cells bigger than blocks). Upstream's test is the same idea at 0.95.
    pub redraw_fraction: f64,
    /// Run the per-block brightness-shift pass.
    pub fade: bool,
}

impl Default for AnalyzeConfig {
    fn default() -> Self {
        AnalyzeConfig { block_size: 30, bleed: 1, min_psnr: 5.2, redraw_fraction: 0.90, fade: true }
    }
}

/// Mean per-channel difference `next - prev` over a block, rounded.
fn fade_scalar(prev: &Frame, next: &Frame, x: u32, y: u32, size: u32) -> i32 {
    let mut sum: i64 = 0;
    for j in 0..size {
        let rp = prev.row(y + j, x, x + size);
        let rn = next.row(y + j, x, x + size);
        for (&p, &n) in rp.iter().zip(rn) {
            sum += n as i64 - p as i64;
        }
    }
    let count = (size as i64) * (size as i64) * 3;
    ((sum as f64) / (count as f64)).round() as i32
}

/// SSE between `next`'s block and `prev`'s block shifted by `scalar`,
/// without materialising the shifted block.
fn shifted_sse_bounded(prev: &Frame, next: &Frame, x: u32, y: u32, size: u32, scalar: i32, limit: u64) -> Option<u64> {
    let mut sum: u64 = 0;
    for j in 0..size {
        let rp = prev.row(y + j, x, x + size);
        let rn = next.row(y + j, x, x + size);
        let mut s: u32 = 0;
        for (&p, &n) in rp.iter().zip(rn) {
            let d = (p as i32 + scalar).clamp(0, 255) - n as i32;
            s += (d * d) as u32;
        }
        sum += s as u64;
        if sum > limit {
            return None;
        }
    }
    Some(sum)
}

/// Analyse one frame transition.
///
/// `prev_working` is mutated: accepted fades are applied to it, so it is the
/// exact low-resolution mirror of what the merge stage will produce. Returns
/// the plan and the reconstructed next frame to use as `prev_working` for
/// the following transition.
pub fn analyze(
    prev_working: &mut Frame,
    next: &Frame,
    reference: &dyn ReferenceMaker,
    matcher: &dyn BlockMatcher,
    cfg: &AnalyzeConfig,
) -> anyhow::Result<(FramePlan, Frame)> {
    anyhow::ensure!(
        (prev_working.width, prev_working.height) == (next.width, next.height),
        "analyze: frame size changed {}x{} -> {}x{}",
        prev_working.width,
        prev_working.height,
        next.width,
        next.height
    );
    let grid = Grid::new(next.width, next.height, cfg.block_size);
    let bs = cfg.block_size;

    let psnr = prev_working.psnr(next);
    if psnr < cfg.min_psnr {
        return Ok((FramePlan::redraw(grid, psnr), next.clone()));
    }

    let degraded = reference.degrade(next)?;
    let thresholds = ThresholdMap::compute(next, &degraded, grid);

    // --- fade pass: evaluated in parallel on the untouched prev, applied after.
    let mut fades: Vec<FadeBlock> = Vec::new();
    if cfg.fade {
        let prev_ro: &Frame = prev_working;
        fades = (0..grid.len())
            .into_par_iter()
            .filter_map(|i| {
                let (x, y) = grid.origin(i);
                let scalar = fade_scalar(prev_ro, next, x, y, bs);
                if scalar == 0 {
                    return None;
                }
                shifted_sse_bounded(prev_ro, next, x, y, bs, scalar, thresholds.get(i))
                    .map(|_| FadeBlock { x, y, scalar })
            })
            .collect();
        for f in &fades {
            prev_working.add_scalar_block(f.x, f.y, bs, f.scalar);
        }
    }

    // --- match pass against the faded prev.
    let prev_ro: &Frame = prev_working;
    let matches: Vec<BlockMatch> = (0..grid.len())
        .into_par_iter()
        .map(|i| {
            let (x, y) = grid.origin(i);
            let limit = thresholds.get(i);
            if block_sse_bounded(next, x, y, prev_ro, x, y, bs, limit).is_some() {
                return BlockMatch::Stationary;
            }
            match matcher.search(prev_ro, next, x, y, bs, limit) {
                Some(c) if c.sse <= limit => BlockMatch::Moved { src_x: c.src_x, src_y: c.src_y },
                _ => BlockMatch::Missing,
            }
        })
        .collect();

    let mut stats = FrameStats { psnr, fades: fades.len() as u32, ..Default::default() };
    for m in &matches {
        match m {
            BlockMatch::Stationary => stats.stationary += 1,
            BlockMatch::Moved { .. } => stats.moved += 1,
            BlockMatch::Missing => stats.missing += 1,
        }
    }

    // Redraw when the packed residual would cover at least `redraw_fraction`
    // of the frame's pixels. Counted in pixels, not blocks: with bleed the
    // residual cells are bigger than blocks, so a mostly-missing frame packs
    // into something *larger* than the frame itself.
    let dim = FramePlan::residual_dim_for(stats.missing as usize);
    let cell = (cfg.block_size + 2 * cfg.bleed) as f64;
    let residual_px = (dim as f64 * cell).powi(2);
    let frame_px = (grid.width() as f64) * (grid.height() as f64);
    if residual_px >= cfg.redraw_fraction * frame_px {
        // Undo the fades: the redraw output is the original next frame, and
        // the low-res prev must not carry a shift the high-res prev never got.
        for f in &fades {
            prev_working.add_scalar_block(f.x, f.y, bs, -f.scalar);
        }
        let mut plan = FramePlan::redraw(grid, psnr);
        plan.stats.fades = 0;
        return Ok((plan, next.clone()));
    }

    // --- residual vectors, row-major over missing cells.
    let mut residuals = Vec::with_capacity(stats.missing as usize);
    let mut k: u32 = 0;
    for (i, m) in matches.iter().enumerate() {
        if *m == BlockMatch::Missing {
            let (x, y) = grid.origin(i);
            residuals.push(ResidualVector { frame_x: x, frame_y: y, cell_x: k % dim, cell_y: k / dim });
            k += 1;
        }
    }
    stats.residual_dim = dim;

    // --- low-res reconstruction of next: matched blocks come from prev.
    let mut working_next = next.clone();
    for (i, m) in matches.iter().enumerate() {
        let (x, y) = grid.origin(i);
        match *m {
            BlockMatch::Stationary => working_next.copy_block(prev_working, bs, x, y, x, y),
            BlockMatch::Moved { src_x, src_y } => working_next.copy_block(prev_working, bs, src_x, src_y, x, y),
            BlockMatch::Missing => {}
        }
    }

    Ok((FramePlan { grid, matches, residuals, fades, redraw: false, stats }, working_next))
}

/// Debug aid: the SSE of the reconstruction against the true next frame,
/// per block, so a viewer can colour blocks by how much the gate let through.
pub fn reconstruction_error(working_next: &Frame, next: &Frame, grid: Grid) -> Vec<u64> {
    (0..grid.len())
        .map(|i| {
            let (x, y) = grid.origin(i);
            block_sse(working_next, x, y, next, x, y, grid.block_size)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testimg::smooth;
    use crate::gate::JpegReference;
    use crate::matcher::{Diamond, Stationary};

    fn cfg() -> AnalyzeConfig {
        AnalyzeConfig { block_size: 16, bleed: 1, ..Default::default() }
    }

    #[test]
    fn identical_frames_are_all_stationary() {
        let prev = smooth(64, 48, 0);
        let mut w = prev.clone();
        let (plan, next_w) = analyze(&mut w, &prev, &JpegReference { quality: 97 }, &Stationary, &cfg()).unwrap();
        assert!(!plan.redraw);
        assert_eq!(plan.stats.stationary, plan.grid.len() as u32);
        assert!(plan.residuals.is_empty());
        assert_eq!(next_w, prev);
    }

    #[test]
    fn unrelated_frames_redraw() {
        let prev = smooth(64, 48, 0);
        let mut next = Frame::new(64, 48);
        for (i, b) in next.data.iter_mut().enumerate() {
            *b = (i * 31 % 251) as u8;
        }
        let mut w = prev.clone();
        let (plan, next_w) = analyze(&mut w, &next, &JpegReference { quality: 97 }, &Stationary, &cfg()).unwrap();
        assert!(plan.redraw);
        assert_eq!(next_w, next);
        assert_eq!(w, prev, "redraw must leave prev untouched");
    }

    #[test]
    fn partial_change_yields_residuals_and_consistent_reconstruction() {
        let prev = smooth(64, 64, 0);
        let mut next = prev.clone();
        // Scribble over one block.
        for y in 16..32 {
            for x in 32..48 {
                next.set_px(x, y, [255, 0, 255]);
            }
        }
        let mut w = prev.clone();
        let (plan, next_w) = analyze(&mut w, &next, &JpegReference { quality: 97 }, &Stationary, &cfg()).unwrap();
        assert!(!plan.redraw);
        assert_eq!(plan.stats.missing, 1);
        assert_eq!(plan.residuals.len(), 1);
        assert_eq!((plan.residuals[0].frame_x, plan.residuals[0].frame_y), (32, 16));
        assert_eq!(plan.stats.residual_dim, 1);
        // Reconstruction: missing block is next's, everything else prev's.
        assert_eq!(next_w.crop(32, 16, 16, 16), next.crop(32, 16, 16, 16));
        assert_eq!(next_w.crop(0, 0, 16, 16), prev.crop(0, 0, 16, 16));
    }

    #[test]
    fn fade_is_caught_as_scalar_not_residual() {
        let prev = smooth(64, 64, 0);
        let mut next = prev.clone();
        // Brighten one block by a flat +20 (values stay in range for this texture).
        next.add_scalar_block(0, 0, 16, 20);
        let mut w = prev.clone();
        let (plan, next_w) = analyze(&mut w, &next, &JpegReference { quality: 97 }, &Stationary, &cfg()).unwrap();
        assert_eq!(plan.stats.missing, 0, "fade should be absorbed, got {:?}", plan.stats);
        assert_eq!(plan.fades.len(), 1);
        assert_eq!(plan.fades[0].scalar, 20);
        assert_eq!(w.crop(0, 0, 16, 16), next.crop(0, 0, 16, 16));
        assert_eq!(next_w, next);
    }

    #[test]
    fn diamond_recovers_a_moved_block() {
        let prev = smooth(96, 96, 0);
        let mut next = prev.clone();
        // Move the block at (48,48) to where (44,50) used to be, i.e. next[48,48] = prev[44,50].
        let moved = prev.crop(44, 50, 16, 16);
        next.copy_block(&moved, 16, 0, 0, 48, 48);
        let mut w = prev.clone();
        let (plan, _) = analyze(&mut w, &next, &JpegReference { quality: 97 }, &Diamond::default(), &cfg()).unwrap();
        let cell = (48 / 16) + (48 / 16) * plan.grid.cols;
        assert!(
            matches!(plan.matches[cell as usize], BlockMatch::Moved { src_x: 44, src_y: 50 }),
            "got {:?}",
            plan.matches[cell as usize]
        );
    }
}
