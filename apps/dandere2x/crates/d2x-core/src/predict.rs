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
use crate::plan::{BlockMatch, FadeBlock, FramePlan, FrameStats, Grid, ResidualVector, MAX_TIERS};
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
    /// Texture thresholds (luma standard deviation, 0–255 units) for routing
    /// missing blocks to cheaper tiers: below `[0]` → tier 2 (bicubic),
    /// below `[1]` → tier 1 (cheap network), else tier 0. Only tiers the
    /// pipeline actually has upscalers for are used.
    pub tier_std: [f64; 2],
    /// Gate hysteresis: a block that has been reused for `n` consecutive
    /// frames gets its threshold scaled by `1 + streak_bonus · min(n, 8) / 8`.
    /// Bounds drift at that multiple of the JPEG error. 0 = off.
    pub streak_bonus: f64,
}

impl Default for AnalyzeConfig {
    fn default() -> Self {
        AnalyzeConfig { block_size: 30, bleed: 1, min_psnr: 5.2, redraw_fraction: 0.90, fade: true, tier_std: [2.0, 8.0], streak_bonus: 0.0 }
    }
}

/// Per-cell state carried across frames by the caller (both stages keep
/// none of it on the merge side; it only affects the gate).
#[derive(Debug, Clone, Default)]
pub struct AnalyzeState {
    /// Consecutive frames each cell has been reused (any non-Missing match).
    pub streak: Vec<u16>,
}

/// Luma standard deviation of a block — the texture measure for tiering.
fn block_luma_std(f: &Frame, x: u32, y: u32, size: u32) -> f64 {
    let n = (size * size) as f64;
    let (mut s, mut s2) = (0.0f64, 0.0f64);
    for j in 0..size {
        let row = f.row(y + j, x, x + size);
        for px in row.chunks_exact(3) {
            let l = 0.299 * px[0] as f64 + 0.587 * px[1] as f64 + 0.114 * px[2] as f64;
            s += l;
            s2 += l * l;
        }
    }
    let mean = s / n;
    (s2 / n - mean * mean).max(0.0).sqrt()
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
/// `older` are further references (low-res reconstructions), most useful
/// first; a block nothing in `prev_working` explains is tried at the same
/// position in each before being declared missing. One bounded SSE per
/// reference per missing block, so a deep ring is cheap.
pub fn analyze(
    prev_working: &mut Frame,
    next: &Frame,
    older: &[&Frame],
    reference: &dyn ReferenceMaker,
    matcher: &dyn BlockMatcher,
    cfg: &AnalyzeConfig,
) -> anyhow::Result<(FramePlan, Frame)> {
    let mut st = AnalyzeState::default();
    analyze_tiered(prev_working, next, older, reference, matcher, cfg, 1, &mut st)
}

/// [`analyze`] with `tiers` upscalers available (1–3) and persistent state.
#[allow(clippy::too_many_arguments)]
pub fn analyze_tiered(
    prev_working: &mut Frame,
    next: &Frame,
    older: &[&Frame],
    reference: &dyn ReferenceMaker,
    matcher: &dyn BlockMatcher,
    cfg: &AnalyzeConfig,
    tiers: usize,
    state: &mut AnalyzeState,
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
    if state.streak.len() != grid.len() {
        state.streak = vec![0; grid.len()];
    }
    if psnr < cfg.min_psnr {
        state.streak.iter_mut().for_each(|s| *s = 0);
        return Ok((FramePlan::redraw(grid, psnr), next.clone()));
    }

    let degraded = reference.degrade(next)?;
    let mut thresholds = ThresholdMap::compute(next, &degraded, grid);
    if cfg.streak_bonus > 0.0 {
        for (t, &n) in thresholds.sse.iter_mut().zip(&state.streak) {
            *t = (*t as f64 * (1.0 + cfg.streak_bonus * (n.min(8) as f64) / 8.0)) as u64;
        }
    }

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
            if let Some(c) = matcher.search(prev_ro, next, x, y, bs, limit) {
                if c.sse <= limit {
                    return BlockMatch::Moved { src_x: c.src_x, src_y: c.src_y };
                }
            }
            for (r, old) in older.iter().enumerate().take(255) {
                if (old.width, old.height) == (next.width, next.height) && block_sse_bounded(next, x, y, old, x, y, bs, limit).is_some() {
                    return BlockMatch::Recalled { r: r as u8 };
                }
            }
            BlockMatch::Missing
        })
        .collect();

    let mut stats = FrameStats { psnr, fades: fades.len() as u32, ..Default::default() };
    for m in &matches {
        match m {
            BlockMatch::Stationary => stats.stationary += 1,
            BlockMatch::Moved { .. } => stats.moved += 1,
            BlockMatch::Recalled { .. } => stats.recalled += 1,
            BlockMatch::Missing => stats.missing += 1,
        }
    }

    // Redraw when the packed residual would cover at least `redraw_fraction`
    // of the frame's pixels. Counted in pixels, not blocks: with bleed the
    // residual cells are bigger than blocks, so a mostly-missing frame packs
    // into something *larger* than the frame itself.
    let (cols, rows) = FramePlan::residual_dims_for(stats.missing as usize);
    let cell = (cfg.block_size + 2 * cfg.bleed) as f64;
    let residual_px = (cols as f64 * cell) * (rows as f64 * cell);
    let frame_px = (grid.width() as f64) * (grid.height() as f64);
    if residual_px >= cfg.redraw_fraction * frame_px {
        // Undo the fades: the redraw output is the original next frame, and
        // the low-res prev must not carry a shift the high-res prev never got.
        for f in &fades {
            prev_working.add_scalar_block(f.x, f.y, bs, -f.scalar);
        }
        state.streak.iter_mut().for_each(|s| *s = 0);
        let mut plan = FramePlan::redraw(grid, psnr);
        plan.stats.fades = 0;
        return Ok((plan, next.clone()));
    }
    for (s, m) in state.streak.iter_mut().zip(&matches) {
        *s = if *m == BlockMatch::Missing { 0 } else { s.saturating_add(1) };
    }

    // --- route missing blocks to tiers by texture, then pack per tier.
    let tiers = tiers.clamp(1, MAX_TIERS);
    let tier_of: Vec<u8> = (0..grid.len())
        .into_par_iter()
        .map(|i| {
            if matches[i] != BlockMatch::Missing || tiers == 1 {
                return 0;
            }
            let (x, y) = grid.origin(i);
            let sd = block_luma_std(next, x, y, bs);
            if tiers >= 3 && sd < cfg.tier_std[0] {
                2
            } else if tiers >= 2 && sd < cfg.tier_std[1] {
                1
            } else {
                0
            }
        })
        .collect();
    let mut counts = [0usize; MAX_TIERS];
    for (i, m) in matches.iter().enumerate() {
        if *m == BlockMatch::Missing {
            counts[tier_of[i] as usize] += 1;
        }
    }
    let mut residual_tiers = [(0u32, 0u32); MAX_TIERS];
    for t in 0..MAX_TIERS {
        residual_tiers[t] = FramePlan::residual_dims_for(counts[t]);
    }
    let mut residuals = Vec::with_capacity(stats.missing as usize);
    let mut k = [0u32; MAX_TIERS];
    for (i, m) in matches.iter().enumerate() {
        if *m == BlockMatch::Missing {
            let t = tier_of[i] as usize;
            let (x, y) = grid.origin(i);
            let cols = residual_tiers[t].0;
            residuals.push(ResidualVector { frame_x: x, frame_y: y, cell_x: k[t] % cols, cell_y: k[t] / cols, tier: t as u8 });
            k[t] += 1;
        }
    }
    stats.residual_tiers = residual_tiers;

    // --- low-res reconstruction of next: matched blocks come from prev.
    let mut working_next = next.clone();
    for (i, m) in matches.iter().enumerate() {
        let (x, y) = grid.origin(i);
        match *m {
            BlockMatch::Stationary => working_next.copy_block(prev_working, bs, x, y, x, y),
            BlockMatch::Moved { src_x, src_y } => working_next.copy_block(prev_working, bs, src_x, src_y, x, y),
            BlockMatch::Recalled { r } => working_next.copy_block(older[r as usize], bs, x, y, x, y),
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
        let (plan, next_w) = analyze(&mut w, &prev, &[], &JpegReference { quality: 97 }, &Stationary, &cfg()).unwrap();
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
        let (plan, next_w) = analyze(&mut w, &next, &[], &JpegReference { quality: 97 }, &Stationary, &cfg()).unwrap();
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
        let (plan, next_w) = analyze(&mut w, &next, &[], &JpegReference { quality: 97 }, &Stationary, &cfg()).unwrap();
        assert!(!plan.redraw);
        assert_eq!(plan.stats.missing, 1);
        assert_eq!(plan.residuals.len(), 1);
        assert_eq!((plan.residuals[0].frame_x, plan.residuals[0].frame_y), (32, 16));
        assert_eq!(plan.stats.residual_tiers[0], (1, 1));
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
        let (plan, next_w) = analyze(&mut w, &next, &[], &JpegReference { quality: 97 }, &Stationary, &cfg()).unwrap();
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
        let (plan, _) = analyze(&mut w, &next, &[], &JpegReference { quality: 97 }, &Diamond::default(), &cfg()).unwrap();
        let cell = (48 / 16) + (48 / 16) * plan.grid.cols;
        assert!(
            matches!(plan.matches[cell as usize], BlockMatch::Moved { src_x: 44, src_y: 50 }),
            "got {:?}",
            plan.matches[cell as usize]
        );
    }
}

#[cfg(test)]
mod ring_tests {
    use super::*;
    use crate::gate::JpegReference;
    use crate::matcher::Stationary;
    use crate::testimg::smooth;

    /// A block occluded for one frame and revealed again is recalled from
    /// the older reference instead of going to the CNN.
    #[test]
    fn occluded_block_is_recalled_from_older_reference() {
        let cfg = AnalyzeConfig { block_size: 16, bleed: 1, ..Default::default() };
        let base = smooth(64, 64, 4);
        let mut occluded = base.clone();
        for y in 16..32 {
            for x in 16..32 {
                occluded.set_px(x, y, [250, 250, 250]);
            }
        }
        // frame 0 = base, frame 1 = occluded, frame 2 = base again.
        let older = base.clone();
        let mut prev = occluded.clone();
        let (plan, next_w) = analyze(&mut prev, &base, &[&older], &JpegReference { quality: 97 }, &Stationary, &cfg).unwrap();
        assert_eq!(plan.stats.missing, 0, "{:?}", plan.stats);
        assert_eq!(plan.stats.recalled, 1);
        let cell = 1 + plan.grid.cols as usize;
        assert!(matches!(plan.matches[cell], BlockMatch::Recalled { r: 0 }));
        assert_eq!(next_w, base);
    }
}

#[cfg(test)]
mod tier_tests {
    use super::*;
    use crate::gate::JpegReference;
    use crate::matcher::Stationary;
    use crate::testimg::smooth;

    #[test]
    fn missing_blocks_route_by_texture() {
        let cfg = AnalyzeConfig { block_size: 16, bleed: 1, tier_std: [2.0, 12.0], ..Default::default() };
        let prev = smooth(64, 64, 1);
        let mut next = prev.clone();
        // flat block (uniform), smooth block (gentle ramp), textured block (checker)
        for y in 0..16 {
            for x in 0..16 {
                next.set_px(x, y, [200, 200, 200]);
                next.set_px(16 + x, y, [(100 + x + y) as u8, (100 + x + y) as u8, (100 + x + y) as u8]);
                let c = if (x + y) % 2 == 0 { 30 } else { 220 };
                next.set_px(32 + x, y, [c, c, c]);
            }
        }
        let mut w = prev.clone();
        let mut st = AnalyzeState::default();
        let (plan, _) = analyze_tiered(&mut w, &next, &[], &JpegReference { quality: 97 }, &Stationary, &cfg, 3, &mut st).unwrap();
        assert_eq!(plan.stats.missing, 3, "{:?}", plan.stats);
        let tier_at = |x: u32| plan.residuals.iter().find(|r| r.frame_x == x && r.frame_y == 0).unwrap().tier;
        assert_eq!(tier_at(0), 2, "flat -> bicubic");
        assert_eq!(tier_at(16), 1, "smooth -> cheap net");
        assert_eq!(tier_at(32), 0, "textured -> main net");
        assert_eq!(plan.stats.residual_tiers, [(1, 1), (1, 1), (1, 1)]);
        // With a single tier everything is tier 0.
        let mut w = prev.clone();
        let (plan1, _) = analyze_tiered(&mut w, &next, &[], &JpegReference { quality: 97 }, &Stationary, &cfg, 1, &mut st).unwrap();
        assert!(plan1.residuals.iter().all(|r| r.tier == 0));
        assert_eq!(plan1.stats.residual_tiers[0], (2, 2));
    }

    #[test]
    fn streak_hysteresis_widens_the_gate_only_for_reused_blocks() {
        let cfg = AnalyzeConfig { block_size: 16, bleed: 1, streak_bonus: 1.0, ..Default::default() };
        let base = smooth(64, 64, 2);
        let mut st = AnalyzeState::default();
        let mut w = base.clone();
        for _ in 0..8 {
            let (_, nw) = analyze_tiered(&mut w, &base, &[], &JpegReference { quality: 97 }, &Stationary, &cfg, 1, &mut st).unwrap();
            w = nw;
        }
        assert!(st.streak.iter().all(|&s| s == 8));
        // A change a bit above the JPEG threshold is now absorbed on a
        // long-reused block (bonus 1.0 doubles the gate).
        let mut next = base.clone();
        next.add_uniform_noise(2, 5);
        let mut w0 = base.clone();
        let mut fresh = AnalyzeState::default();
        let (p_fresh, _) = analyze_tiered(&mut w0, &next, &[], &JpegReference { quality: 97 }, &Stationary, &cfg, 1, &mut fresh).unwrap();
        let mut w1 = base.clone();
        let (p_streak, _) = analyze_tiered(&mut w1, &next, &[], &JpegReference { quality: 97 }, &Stationary, &cfg, 1, &mut st).unwrap();
        assert!(p_streak.stats.missing <= p_fresh.stats.missing, "streak {:?} vs fresh {:?}", p_streak.stats, p_fresh.stats);
    }
}
