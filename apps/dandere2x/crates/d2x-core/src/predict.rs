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

use crate::blockcache::{coarse_key, hash_cell, BlockCache};
use crate::plan::HarvestCell;
use crate::frame::{block_sse, block_sse_bounded, Frame, Pad};
use crate::gate::{ReferenceMaker, ThresholdMap};
use crate::matcher::BlockMatcher;
use crate::plan::{BlockMatch, FadeBlock, FramePlan, FrameStats, Grid, ResidualVector, MAX_TIERS};
use crate::plate::PlateView;
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
    /// After the integer motion search fails the gate, try the eight
    /// half-pel neighbours of its best candidate (bilinear). Sharp content
    /// under sub-pixel camera motion fails an integer match by construction;
    /// this is the cheapest thing that can catch it. Needs an even upscale
    /// factor for the merge to stay an exact copy.
    pub halfpel: bool,
    /// The interpolant used to evaluate (and mirror) half-pel candidates.
    pub subpel_filter: SubpelFilter,
    /// After half-pel, try the eight quarter-pel neighbours of the best
    /// half-pel candidate (bilinear). Exact copy only at scales that are a
    /// multiple of 4.
    pub quarterpel: bool,
    /// When the packed residual would cost more than `redraw_fraction` of
    /// the frame, or the bounding rectangle of missing blocks is cheaper
    /// than packing them, upscale that rectangle as ONE image (with bleed)
    /// instead of the whole frame. Blocks matched outside it are kept.
    pub region_redraw: bool,
    /// RMSE gate (8-bit units) for motion search against the anchor —
    /// the last redrawn frame, whose output is a fresh network pass. 0 =
    /// no anchor search. Content-independent, unlike the JPEG gate, which
    /// is what the propagation measurement used.
    pub anchor_rmse: f64,
    /// Force a full redraw (a new anchor) when the last anchor is this many
    /// frames old (0 = never). The propagation measurement gained for ±2
    /// frames from a restored anchor and lost past ±4; this bounds the
    /// distance blocks are propagated over, at one network pass per N frames.
    pub anchor_max_age: u32,
    /// Harvest the subtiles of every big upscale (redraw, region redraw)
    /// into the block cache. Needs a cache budget to do anything.
    pub harvest: bool,
    /// Probe the cache by coarse bucket and verify the candidate against the
    /// frame's own block under the gate, instead of requiring an exact byte
    /// match. 0 = exact only; otherwise the quantisation step of the coarse
    /// key (8–24 is the useful range; larger buckets, more candidates).
    pub cache_quant: u32,
    /// Candidates kept per coarse bucket (probe cost is linear in this).
    pub cache_bucket: usize,
    /// Texture thresholds (luma standard deviation, 0–255 units) for routing
    /// missing blocks to cheaper tiers: below `[0]` → tier 2 (bicubic),
    /// below `[1]` → tier 1 (cheap network), else tier 0. Only tiers the
    /// pipeline actually has upscalers for are used. Units depend on
    /// `tier_metric`; `dandere2x calibrate` measures the right values for a
    /// clip instead of guessing them.
    pub tier_std: [f64; 2],
    /// Which measure `tier_std` is compared against.
    pub tier_metric: TierMetric,
    /// Gate hysteresis: a block that has been reused for `n` consecutive
    /// frames gets its threshold scaled by `1 + streak_bonus · min(n, 8) / 8`.
    /// Bounds drift at that multiple of the JPEG error. 0 = off.
    pub streak_bonus: f64,
}

impl Default for AnalyzeConfig {
    fn default() -> Self {
        AnalyzeConfig { block_size: 30, bleed: 1, min_psnr: 5.2, redraw_fraction: 0.90, fade: true, halfpel: false, subpel_filter: SubpelFilter::Bilinear, quarterpel: false, region_redraw: false, anchor_rmse: 0.0, anchor_max_age: 0, harvest: true, cache_quant: 0, cache_bucket: 4, tier_std: [1.0, 4.0], tier_metric: TierMetric::HighFreq, streak_bonus: 0.0 }
    }
}

/// Per-cell state carried across frames by the caller (both stages keep
/// none of it on the merge side; it only affects the gate), plus the shared
/// block cache if one is in use.
#[derive(Debug, Clone, Default)]
pub struct AnalyzeState {
    /// Consecutive frames each cell has been reused (any non-Missing match).
    pub streak: Vec<u16>,
    pub cache: Option<std::sync::Arc<std::sync::Mutex<BlockCache>>>,
    /// Index into `older` of the anchor for this frame (set by the pipeline
    /// per frame; the ring puts the newest retained anchor after the recent
    /// references). `None` = no anchor search.
    pub anchor: Option<u8>,
    /// Frames since the last anchor (full redraw or ≥ half-frame region).
    pub since_anchor: u32,
    /// Set by the wrapper when `anchor_max_age` forces a new anchor: the
    /// match pass is skipped, the cache probe is not.
    pub force_cut: bool,
}

/// Upscaled bytes for the `Cached` blocks of one plan, attached by the
/// analysis stage so eviction between the stages cannot lose them.
pub type CachedBlocks = std::collections::HashMap<u64, std::sync::Arc<Vec<u8>>>;

/// How a half-pel block of the previous frame is interpolated for the gate
/// and for the low-res mirror.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubpelFilter {
    /// Rounded 2-tap box: exactly the box-downsample of a nearest ×2
    /// upscale read at the odd offset. Soft.
    #[default]
    Bilinear,
    /// H.264's 6-tap `[1, −5, 20, 20, −5, 1] / 32`: a sharp interpolant,
    /// closer to what a CNN's ×2 output holds at an odd pixel.
    SixTap,
}

impl SubpelFilter {
    pub fn block(self, f: &Frame, sx2: u32, sy2: u32, size: u32) -> Frame {
        match self {
            SubpelFilter::Bilinear => f.block_halfpel(sx2, sy2, size),
            SubpelFilter::SixTap => f.block_halfpel_6tap(sx2, sy2, size),
        }
    }
}

/// Which measure decides a missing block's tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TierMetric {
    /// Luma standard deviation. Cheap, but conflates a smooth gradient with
    /// film grain: both land mid-range while wanting opposite treatment.
    Std,
    /// Mean absolute 3x3 Laplacian response — high-frequency energy. This is
    /// the quantity a super-resolution network can actually act on: bokeh,
    /// motion blur and defocus have low HF energy no matter how much contrast
    /// or noise they carry, so the expensive model has nothing to recover
    /// there and provably cannot beat the cheap one.
    #[default]
    HighFreq,
}

/// Mean absolute 3x3 Laplacian of luma over a block's interior, in 0-255
/// units. The standard defocus measure, computed on integer luma.
pub fn block_hf_energy(f: &Frame, x: u32, y: u32, size: u32) -> f64 {
    if size < 3 {
        return 0.0;
    }
    let luma = |px: &[u8]| (77 * px[0] as i32 + 150 * px[1] as i32 + 29 * px[2] as i32) >> 8;
    let mut sum: u64 = 0;
    let mut n: u64 = 0;
    for j in 1..size - 1 {
        let above = f.row(y + j - 1, x, x + size);
        let mid = f.row(y + j, x, x + size);
        let below = f.row(y + j + 1, x, x + size);
        for i in 1..(size as usize) - 1 {
            let c = luma(&mid[i * 3..i * 3 + 3]);
            let lap = 4 * c
                - luma(&mid[(i - 1) * 3..(i - 1) * 3 + 3])
                - luma(&mid[(i + 1) * 3..(i + 1) * 3 + 3])
                - luma(&above[i * 3..i * 3 + 3])
                - luma(&below[i * 3..i * 3 + 3]);
            sum += lap.unsigned_abs() as u64;
            n += 1;
        }
    }
    sum as f64 / n.max(1) as f64
}

/// Remove isolated tier assignments — a block whose Missing neighbours all
/// agree on a different tier joins them.
///
/// # Why
///
/// Tier is a hard threshold on a per-block texture measure, so two ADJACENT
/// blocks either side of `tier_std[1]` are upscaled by different networks. A
/// difference between two models is a larger discontinuity than a difference
/// between two applications of one model, and `residual::blend_block` feathers
/// the seam without making the two halves match.
///
/// The visible case is a SINGLETON: one block routed to bicubic in the middle
/// of a network-upscaled region, or the reverse. It reads as a patch, not as a
/// texture boundary.
///
/// # Deliberately conservative
///
/// Only a block whose Missing neighbours UNANIMOUSLY disagree with it is
/// moved. A majority rule would erode genuine texture boundaries — a real
/// edge between smooth sky and detailed foliage SHOULD change tier, and
/// smoothing it away spends network time on sky.
///
/// Non-Missing blocks are not considered on either side: they carry tier 0 by
/// default and are not upscaled at all, so counting them as votes would drag
/// every block beside a stationary region down to tier 0.
///
/// One pass, not to convergence. A second pass would let a change propagate,
/// which is the erosion this avoids.
pub fn despeckle_tiers(tier_of: &[u8], matches: &[BlockMatch], cols: usize, rows: usize) -> Vec<u8> {
    let mut out = tier_of.to_vec();
    for i in 0..tier_of.len() {
        if matches.get(i) != Some(&BlockMatch::Missing) {
            continue;
        }
        let (cx, cy) = (i % cols, i / cols);
        let mut agreed: Option<u8> = None;
        let mut n = 0usize;
        for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
            let (nx, ny) = (cx as i64 + dx, cy as i64 + dy);
            if nx < 0 || ny < 0 || nx >= cols as i64 || ny >= rows as i64 {
                continue;
            }
            let j = ny as usize * cols + nx as usize;
            if matches.get(j) != Some(&BlockMatch::Missing) {
                continue;
            }
            match agreed {
                None => { agreed = Some(tier_of[j]); n = 1; }
                Some(t) if t == tier_of[j] => n += 1,
                // Neighbours disagree with each other: this is a real
                // boundary, not a speckle. Leave it alone.
                Some(_) => { n = 0; break; }
            }
        }
        // At least two agreeing neighbours: a single one is not evidence, and
        // a block with one Missing neighbour is itself on an edge.
        if n >= 2 {
            if let Some(t) = agreed {
                if t != tier_of[i] {
                    out[i] = t;
                }
            }
        }
    }
    out
}

/// The tier feature for a block under `metric`.
pub fn block_feature(f: &Frame, x: u32, y: u32, size: u32, metric: TierMetric) -> f64 {
    match metric {
        TierMetric::Std => block_luma_std(f, x, y, size),
        TierMetric::HighFreq => block_hf_energy(f, x, y, size),
    }
}

/// Luma standard deviation of a block — the texture measure for tiering.
fn block_luma_std(f: &Frame, x: u32, y: u32, size: u32) -> f64 {
    // Integer luma (BT.601 weights in 8.8 fixed point) with 32-bit row
    // sums: luma ≤ 255, luma² ≤ 65 025, so a row of ≤ 255 pixels fits.
    let n = (size * size) as f64;
    let (mut s, mut s2) = (0u64, 0u64);
    for j in 0..size {
        let row = f.row(y + j, x, x + size);
        let (mut rs, mut rs2) = (0u32, 0u32);
        for px in row.chunks_exact(3) {
            let l = (77 * px[0] as u32 + 150 * px[1] as u32 + 29 * px[2] as u32) >> 8;
            rs += l;
            rs2 += l * l;
        }
        s += rs as u64;
        s2 += rs2 as u64;
    }
    let mean = s as f64 / n;
    (s2 as f64 / n - mean * mean).max(0.0).sqrt()
}

/// Mean per-channel difference `next - prev` over a block, rounded.
fn fade_scalar(prev: &Frame, next: &Frame, x: u32, y: u32, size: u32) -> i32 {
    let mut sum: i64 = 0;
    for j in 0..size {
        let rp = prev.row(y + j, x, x + size);
        let rn = next.row(y + j, x, x + size);
        // A row is at most 3·size ≤ 3·255 bytes of |diff| ≤ 255: i32 is safe
        // and keeps the reduction in packed lanes.
        let mut row: i32 = 0;
        for (&p, &n) in rp.iter().zip(rn) {
            row += n as i32 - p as i32;
        }
        sum += row as i64;
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
    let (plan, w, _) = analyze_tiered(prev_working, next, older, None, None, reference, matcher, cfg, 1, &mut st)?;
    Ok((plan, w))
}

/// [`analyze`] with `tiers` upscalers available (1–3) and persistent state.
///
/// `plate`, when given, is the window's plate as this frame sees it. It is
/// tried after the stationary test and before the motion search: one bounded
/// SSE, and a hit sources the block from the plate's (denoised, once-
/// upscaled) pixels rather than extending the copy chain through the
/// previous output.
///
/// `mv_hint` is the frame's global motion as a source offset — the block at
/// `(x, y)` is expected at `(x + hint.0, y + hint.1)` in `prev` — handed to
/// [`BlockMatcher::search_from`].
#[allow(clippy::too_many_arguments)]
pub fn analyze_tiered(
    prev_working: &mut Frame,
    next: &Frame,
    older: &[&Frame],
    plate: Option<PlateView<'_>>,
    mv_hint: Option<(i64, i64)>,
    reference: &dyn ReferenceMaker,
    matcher: &dyn BlockMatcher,
    cfg: &AnalyzeConfig,
    tiers: usize,
    state: &mut AnalyzeState,
) -> anyhow::Result<(FramePlan, Frame, CachedBlocks)> {
    state.since_anchor = state.since_anchor.saturating_add(1);
    // A forced anchor does NOT short-circuit into a whole-frame redraw. It
    // marks every block missing and lets the normal path run: the cache is
    // probed, and whatever it cannot explain becomes a region or a packed
    // residual. Short-circuiting here was measured to cost the cache most of
    // its hits, because a forced anchor is one of the frames most likely to
    // repeat content the job has already drawn.
    let forced = cfg.anchor_max_age > 0 && state.since_anchor > cfg.anchor_max_age;
    state.force_cut = forced;
    let r = analyze_tiered_inner(prev_working, next, older, plate, mv_hint, reference, matcher, cfg, tiers, state)?;
    state.force_cut = false;
    if forced || r.0.stats.is_anchor(&r.0.grid) {
        state.since_anchor = 0;
    }
    Ok(r)
}

#[allow(clippy::too_many_arguments)]
fn analyze_tiered_inner(
    prev_working: &mut Frame,
    next: &Frame,
    older: &[&Frame],
    plate: Option<PlateView<'_>>,
    mv_hint: Option<(i64, i64)>,
    reference: &dyn ReferenceMaker,
    matcher: &dyn BlockMatcher,
    cfg: &AnalyzeConfig,
    tiers: usize,
    state: &mut AnalyzeState,
) -> anyhow::Result<(FramePlan, Frame, CachedBlocks)> {
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
    // A scene cut: nothing in the previous frame explains this one, so the
    // match pass is skipped. Not an early return any more — the cache still
    // gets its probe below, because a cut is precisely when a job may be
    // returning to a shot it has already paid for.
    let cut = psnr < cfg.min_psnr || state.force_cut;
    if cut {
        state.streak.iter_mut().for_each(|s| *s = 0);
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
    let do_fade = cfg.fade && !cut;
    if do_fade {
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
    let anchor = state.anchor;
    let matches: Vec<BlockMatch> = (0..grid.len())
        .into_par_iter()
        .map(|i| {
            if cut {
                return BlockMatch::Missing;
            }
            let (x, y) = grid.origin(i);
            let limit = thresholds.get(i);
            if block_sse_bounded(next, x, y, prev_ro, x, y, bs, limit).is_some() {
                return BlockMatch::Stationary;
            }
            if let Some(pv) = plate {
                let (px, py) = pv.at(x, y);
                if pv.lo.block_in_bounds(px, py, bs) && block_sse_bounded(next, x, y, pv.lo, px, py, bs, limit).is_some() {
                    return BlockMatch::Plated { px, py };
                }
            }
            // The integer search is bounded by the gate; for half-pel
            // refinement we want its best candidate even when that fails, so
            // the bound is lifted only when refinement is on.
            let ilimit = if cfg.halfpel { u64::MAX } else { limit };
            let found = match mv_hint {
                Some(h) => matcher.search_from(prev_ro, next, x, y, bs, ilimit, h),
                None => matcher.search(prev_ro, next, x, y, bs, ilimit),
            };
            if let Some(c) = found {
                if c.sse <= limit {
                    return BlockMatch::Moved { src_x: c.src_x, src_y: c.src_y };
                }
                if cfg.halfpel {
                    // With quarter-pel on, the half-pel stage is a search
                    // stage, not a gate: its best (which may be the integer
                    // candidate) centres the quarter-pel search.
                    let lim_h = if cfg.quarterpel { u64::MAX } else { limit };
                    if let Some((hsse, sx2, sy2)) = best_halfpel(prev_ro, next, x, y, bs, c, lim_h, cfg.subpel_filter) {
                        if cfg.quarterpel {
                            if let Some((qsse, sx4, sy4)) = refine_quarterpel(prev_ro, next, x, y, bs, sx2, sy2, hsse) {
                                if qsse <= limit {
                                    return BlockMatch::MovedQuarter { sx4, sy4 };
                                }
                            }
                        }
                        if hsse <= limit && (sx2 % 2 == 1 || sy2 % 2 == 1) {
                            return BlockMatch::MovedHalf { sx2, sy2 };
                        }
                    }
                }
            }
            if let (Some(a), true) = (anchor, cfg.anchor_rmse > 0.0) {
                if let Some(old) = older.get(a as usize) {
                    if (old.width, old.height) == (next.width, next.height) {
                        let alim = (cfg.anchor_rmse * cfg.anchor_rmse * (bs * bs * 3) as f64) as u64;
                        if let Some(c) = matcher.search(old, next, x, y, bs, alim) {
                            if c.sse <= alim {
                                return BlockMatch::RecalledMoved { r: a, src_x: c.src_x, src_y: c.src_y };
                            }
                        }
                    }
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
            BlockMatch::Cached { .. } => stats.cached += 1,
            BlockMatch::Plated { .. } => stats.plated += 1,
            BlockMatch::MovedHalf { .. } => stats.moved_half += 1,
            BlockMatch::MovedQuarter { .. } => stats.moved_quarter += 1,
            BlockMatch::RecalledMoved { .. } => stats.anchored += 1,
            BlockMatch::Missing => stats.missing += 1,
        }
    }

    // --- content hashes of missing cells (block + bleed ring, black beyond
    // the frame, exactly what make_residual packs), and cache lookups.
    //
    // This runs BEFORE the redraw decision, not after. The frame that would
    // be redrawn whole is exactly the frame a cache can help most — a cut
    // back to a shot this job has already drawn — and deciding to redraw
    // first meant those cells were never probed at all. Measured: harvesting
    // tripled the supply and changed nothing while the probe happened after
    // the decision. Now the three-way cost (packed / region / whole) is
    // computed on what is missing *after* the cache has been consulted, so
    // harvested subtiles can shrink a redraw into a region, or a region into
    // a packed residual.
    let cell_px = bs + 2 * cfg.bleed;
    let padded = next.padded(cfg.bleed, Pad::Black);
    let keys: Vec<u64> = (0..grid.len())
        .into_par_iter()
        .map(|i| {
            if matches[i] != BlockMatch::Missing {
                return 0;
            }
            let (x, y) = grid.origin(i);
            hash_cell(&padded, x, y, cell_px)
        })
        .collect();
    let mut cached_blocks = CachedBlocks::new();
    let mut matches = matches;
    if let Some(cache) = &state.cache {
        let mut c = cache.lock().unwrap();
        if c.enabled() {
            for i in 0..grid.len() {
                if matches[i] != BlockMatch::Missing {
                    continue;
                }
                if let Some(b) = c.get(keys[i]) {
                    cached_blocks.insert(keys[i], b);
                    matches[i] = BlockMatch::Cached { key: keys[i] };
                    stats.cached += 1;
                    stats.missing -= 1;
                    continue;
                }
                if cfg.cache_quant > 0 {
                    let (x, y) = grid.origin(i);
                    let ck = coarse_key(&padded, x, y, cell_px, cfg.cache_quant);
                    let limit = thresholds.get(i);
                    let hit = c.get_gated(ck, bs, |lo| block_sse_bounded(next, x, y, lo, 0, 0, bs, limit));
                    if let Some((k, e)) = hit {
                        cached_blocks.insert(k, e.up);
                        matches[i] = BlockMatch::Cached { key: k };
                        stats.cached += 1;
                        stats.cached_gated += 1;
                        stats.missing -= 1;
                    }
                }
            }
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
    // Region redraw: the bounding rectangle of the missing blocks, padded by
    // bleed and clamped to the frame, as one image. Cheaper than packing
    // whenever the misses are dense (no per-block bleed), never dearer than
    // the whole frame, and seamless inside.
    let region: Option<(u32, u32, u32, u32, u64)> = if cfg.region_redraw && stats.missing > 0 {
        let (mut bx0, mut by0, mut bx1, mut by1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for (i, m) in matches.iter().enumerate() {
            if *m == BlockMatch::Missing {
                let (x, y) = grid.origin(i);
                bx0 = bx0.min(x);
                by0 = by0.min(y);
                bx1 = bx1.max(x + bs);
                by1 = by1.max(y + bs);
            }
        }
        let px0 = bx0.saturating_sub(cfg.bleed);
        let py0 = by0.saturating_sub(cfg.bleed);
        let px1 = (bx1 + cfg.bleed).min(grid.width());
        let py1 = (by1 + cfg.bleed).min(grid.height());
        Some((bx0, by0, bx1 - bx0, by1 - by0, ((px1 - px0) as u64) * ((py1 - py0) as u64)))
    } else {
        None
    };
    let packed_ok = residual_px < cfg.redraw_fraction * frame_px;
    if let Some((rx, ry, rw, rh, rpx)) = region {
        let whole = (rw, rh) == (grid.width(), grid.height());
        if (!packed_ok || (rpx as f64) < residual_px) && !whole {
            let mut matches = matches;
            let mut stats = FrameStats { psnr, fades: fades.len() as u32, ..Default::default() };
            for (i, m) in matches.iter_mut().enumerate() {
                let (x, y) = grid.origin(i);
                if x >= rx && x < rx + rw && y >= ry && y < ry + rh {
                    *m = BlockMatch::Missing;
                }
                match *m {
                    BlockMatch::Stationary => stats.stationary += 1,
                    BlockMatch::Moved { .. } => stats.moved += 1,
                    BlockMatch::Recalled { .. } => stats.recalled += 1,
                    BlockMatch::Cached { .. } => stats.cached += 1,
                    BlockMatch::Plated { .. } => stats.plated += 1,
                    BlockMatch::MovedHalf { .. } => stats.moved_half += 1,
                    BlockMatch::MovedQuarter { .. } => stats.moved_quarter += 1,
                    BlockMatch::RecalledMoved { .. } => stats.anchored += 1,
                    BlockMatch::Missing => stats.missing += 1,
                }
            }
            stats.region = Some((rx, ry, rw, rh));
            stats.region_px = rpx;
            let harvest = harvest_for(next, cfg, &grid, Some((rx, ry, rw, rh)), state);
            stats.harvested = harvest.len() as u32;
            for (s, m) in state.streak.iter_mut().zip(&matches) {
                *s = if *m == BlockMatch::Missing { 0 } else { s.saturating_add(1) };
            }
            let working_next = reconstruct(next, prev_working, older, plate, cfg, &grid, &matches);
            return Ok((FramePlan { grid, matches, residuals: Vec::new(), fades, redraw: false, harvest, stats }, working_next, cached_blocks));
        }
    }
    if !packed_ok {
        // Undo the fades: the redraw output is the original next frame, and
        // the low-res prev must not carry a shift the high-res prev never got.
        for f in &fades {
            prev_working.add_scalar_block(f.x, f.y, bs, -f.scalar);
        }
        state.streak.iter_mut().for_each(|s| *s = 0);
        let mut plan = FramePlan::redraw(grid, psnr);
        plan.stats.fades = 0;
        plan.harvest = harvest_for(next, cfg, &plan.grid, None, state);
        plan.stats.harvested = plan.harvest.len() as u32;
        return Ok((plan, next.clone(), CachedBlocks::new()));
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
            let sd = block_feature(next, x, y, bs, cfg.tier_metric);
            if tiers >= 3 && sd < cfg.tier_std[0] {
                2
            } else if tiers >= 2 && sd < cfg.tier_std[1] {
                1
            } else {
                0
            }
        })
        .collect();
    // Isolated tier singletons are removed before packing. See
    // `despeckle_tiers` — a lone cheap block inside a field of expensive ones
    // is upscaled by a DIFFERENT network than everything touching it, and no
    // amount of edge feathering hides that.
    let tier_of = despeckle_tiers(&tier_of, &matches, grid.cols as usize, grid.rows as usize);

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
            residuals.push(ResidualVector { frame_x: x, frame_y: y, cell_x: k[t] % cols, cell_y: k[t] / cols, tier: t as u8, key: keys[i] });
            k[t] += 1;
        }
    }
    stats.residual_tiers = residual_tiers;

    // --- low-res reconstruction of next: matched blocks come from prev.
    let working_next = reconstruct(next, prev_working, older, plate, cfg, &grid, &matches);

    Ok((FramePlan { grid, matches, residuals, fades, redraw: false, harvest: Vec::new(), stats }, working_next, cached_blocks))
}

/// Cache keys for the cells a big upscale will draw: every cell for a whole
/// redraw, the cells inside the rectangle for a region redraw. Same key
/// function and same padding as the lookup path, so a harvested block is
/// found by a later frame's ordinary cache probe.
fn harvest_for(next: &Frame, cfg: &AnalyzeConfig, grid: &Grid, region: Option<(u32, u32, u32, u32)>, state: &AnalyzeState) -> Vec<HarvestCell> {
    if !cfg.harvest {
        return Vec::new();
    }
    match &state.cache {
        Some(c) if c.lock().unwrap().enabled() => harvest_keys(next, cfg, grid, region),
        _ => Vec::new(),
    }
}

fn harvest_keys(next: &Frame, cfg: &AnalyzeConfig, grid: &Grid, region: Option<(u32, u32, u32, u32)>) -> Vec<HarvestCell> {
    let cell_px = cfg.block_size + 2 * cfg.bleed;
    let padded = next.padded(cfg.bleed, Pad::Black);
    (0..grid.len())
        .into_par_iter()
        .filter_map(|i| {
            let (x, y) = grid.origin(i);
            if let Some((rx, ry, rw, rh)) = region {
                // Only cells wholly inside the rectangle: those are the ones
                // the merge writes at full weight, so the block in the output
                // is exactly the network's, not a feathered blend.
                if x < rx || y < ry || x + cfg.block_size > rx + rw || y + cfg.block_size > ry + rh {
                    return None;
                }
            }
            Some(HarvestCell {
                key: hash_cell(&padded, x, y, cell_px),
                coarse: coarse_key(&padded, x, y, cell_px, cfg.cache_quant.max(1)),
                x,
                y,
                lo: std::sync::Arc::new(next.crop(x, y, cfg.block_size, cfg.block_size).data),
            })
        })
        .collect()
}

/// The low-res mirror of what the merge will output: matched blocks from
/// their sources, everything else the frame's own pixels.
fn reconstruct(next: &Frame, prev_working: &Frame, older: &[&Frame], plate: Option<PlateView<'_>>, cfg: &AnalyzeConfig, grid: &Grid, matches: &[BlockMatch]) -> Frame {
    let bs = cfg.block_size;
    let mut working_next = next.clone();
    for (i, m) in matches.iter().enumerate() {
        let (x, y) = grid.origin(i);
        match *m {
            BlockMatch::Stationary => working_next.copy_block(prev_working, bs, x, y, x, y),
            BlockMatch::Moved { src_x, src_y } => working_next.copy_block(prev_working, bs, src_x, src_y, x, y),
            BlockMatch::Recalled { r } => working_next.copy_block(older[r as usize], bs, x, y, x, y),
            BlockMatch::RecalledMoved { r, src_x, src_y } => working_next.copy_block(older[r as usize], bs, src_x, src_y, x, y),
            BlockMatch::Plated { px, py } => working_next.copy_block(plate.expect("Plated without a plate").lo, bs, px, py, x, y),
            BlockMatch::MovedHalf { sx2, sy2 } => {
                let b = cfg.subpel_filter.block(prev_working, sx2, sy2, bs);
                working_next.copy_block(&b, bs, 0, 0, x, y);
            }
            BlockMatch::MovedQuarter { sx4, sy4 } => {
                let b = prev_working.block_quarterpel(sx4, sy4, bs);
                working_next.copy_block(&b, bs, 0, 0, x, y);
            }
            // Cached blocks are the frame's own content, like Missing.
            BlockMatch::Cached { .. } | BlockMatch::Missing => {}
        }
    }
    working_next
}

/// The best of the eight quarter-pel neighbours of a half-pel position
/// (bilinear) that beats `start_sse`, as `(sse, sx4, sy4)`.
#[allow(clippy::too_many_arguments)]
fn refine_quarterpel(prev: &Frame, next: &Frame, x: u32, y: u32, bs: u32, sx2: u32, sy2: u32, start_sse: u64) -> Option<(u64, u32, u32)> {
    let limit = start_sse.saturating_sub(1);
    let mut best: Option<(u64, u32, u32)> = None;
    for dy4 in -1i64..=1 {
        for dx4 in -1i64..=1 {
            if dx4 == 0 && dy4 == 0 {
                continue;
            }
            let sx4 = sx2 as i64 * 2 + dx4;
            let sy4 = sy2 as i64 * 2 + dy4;
            if sx4 < 0 || sy4 < 0 {
                continue;
            }
            let (sx4, sy4) = (sx4 as u32, sy4 as u32);
            if sx4 / 4 + bs + (sx4 % 4 > 0) as u32 > prev.width || sy4 / 4 + bs + (sy4 % 4 > 0) as u32 > prev.height {
                continue;
            }
            let b = prev.block_quarterpel(sx4, sy4, bs);
            let lim = best.map_or(limit, |b| b.0);
            if let Some(sse) = block_sse_bounded(next, x, y, &b, 0, 0, bs, lim) {
                if best.map_or(true, |b| sse < b.0) {
                    best = Some((sse, sx4, sy4));
                }
            }
        }
    }
    best
}

/// The best of an integer candidate and its eight half-pel neighbours
/// (`(sse, sx2, sy2)`, half-pel units) under `limit`. Bilinear samples of
/// `prev`; `block_sse` against the materialised sample (eight small blocks
/// per failing block — cheap next to the JPEG gate, and only paid where the
/// integer search failed).
#[allow(clippy::too_many_arguments)]
fn best_halfpel(prev: &Frame, next: &Frame, x: u32, y: u32, bs: u32, c: crate::matcher::Candidate, limit: u64, filter: SubpelFilter) -> Option<(u64, u32, u32)> {
    let mut best: Option<(u64, u32, u32)> = if c.sse <= limit { Some((c.sse, c.src_x * 2, c.src_y * 2)) } else { None };
    for dy2 in -1i64..=1 {
        for dx2 in -1i64..=1 {
            if dx2 == 0 && dy2 == 0 {
                continue;
            }
            let sx2 = c.src_x as i64 * 2 + dx2;
            let sy2 = c.src_y as i64 * 2 + dy2;
            if sx2 < 0 || sy2 < 0 {
                continue;
            }
            let (sx2, sy2) = (sx2 as u32, sy2 as u32);
            if sx2 / 2 + bs + (sx2 % 2) > prev.width || sy2 / 2 + bs + (sy2 % 2) > prev.height {
                continue;
            }
            let b = filter.block(prev, sx2, sy2, bs);
            let lim = best.map_or(limit, |b| b.0);
            if let Some(sse) = block_sse_bounded(next, x, y, &b, 0, 0, bs, lim) {
                if best.map_or(true, |b| sse < b.0) {
                    best = Some((sse, sx2, sy2));
                }
            }
        }
    }
    best
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
        let cfg = AnalyzeConfig { block_size: 16, bleed: 1, tier_std: [2.0, 12.0], tier_metric: TierMetric::Std, ..Default::default() };
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
        let (plan, _, _) = analyze_tiered(&mut w, &next, &[], None, None, &JpegReference { quality: 97 }, &Stationary, &cfg, 3, &mut st).unwrap();
        assert_eq!(plan.stats.missing, 3, "{:?}", plan.stats);
        let tier_at = |x: u32| plan.residuals.iter().find(|r| r.frame_x == x && r.frame_y == 0).unwrap().tier;
        assert_eq!(tier_at(0), 2, "flat -> bicubic");
        assert_eq!(tier_at(16), 1, "smooth -> cheap net");
        assert_eq!(tier_at(32), 0, "textured -> main net");
        assert_eq!(plan.stats.residual_tiers, [(1, 1), (1, 1), (1, 1)]);
        // With a single tier everything is tier 0.
        let mut w = prev.clone();
        let (plan1, _, _) = analyze_tiered(&mut w, &next, &[], None, None, &JpegReference { quality: 97 }, &Stationary, &cfg, 1, &mut st).unwrap();
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
            let (_, nw, _) = analyze_tiered(&mut w, &base, &[], None, None, &JpegReference { quality: 97 }, &Stationary, &cfg, 1, &mut st).unwrap();
            w = nw;
        }
        assert!(st.streak.iter().all(|&s| s == 8));
        // A change a bit above the JPEG threshold is now absorbed on a
        // long-reused block (bonus 1.0 doubles the gate).
        let mut next = base.clone();
        next.add_uniform_noise(2, 5);
        let mut w0 = base.clone();
        let mut fresh = AnalyzeState::default();
        let (p_fresh, _, _) = analyze_tiered(&mut w0, &next, &[], None, None, &JpegReference { quality: 97 }, &Stationary, &cfg, 1, &mut fresh).unwrap();
        let mut w1 = base.clone();
        let (p_streak, _, _) = analyze_tiered(&mut w1, &next, &[], None, None, &JpegReference { quality: 97 }, &Stationary, &cfg, 1, &mut st).unwrap();
        assert!(p_streak.stats.missing <= p_fresh.stats.missing, "streak {:?} vs fresh {:?}", p_streak.stats, p_fresh.stats);
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::blockcache::BlockCache;
    use crate::gate::JpegReference;
    use crate::matcher::Stationary;
    use crate::testimg::smooth;
    use std::sync::{Arc, Mutex};

    /// The same new content at a different position on a later frame is a
    /// cache hit, carrying the bytes that were inserted for it.
    #[test]
    fn repeated_cell_content_is_served_from_the_cache() {
        let cfg = AnalyzeConfig { block_size: 16, bleed: 1, ..Default::default() };
        let base = smooth(64, 64, 6);
        let stamp = |f: &mut Frame, x0: u32, y0: u32| {
            for y in 0..16 {
                for x in 0..16 {
                    f.set_px(x0 + x, y0 + y, [((x * 13 + y * 7) % 200) as u8, 40, ((x ^ y) * 9) as u8]);
                }
            }
        };
        let cache = Arc::new(Mutex::new(BlockCache::new(1 << 20)));
        let mut st = AnalyzeState { streak: Vec::new(), cache: Some(cache.clone()), anchor: None, since_anchor: 0, force_cut: false };
        // Frame 1: the stamp at (16,16), with a black bleed ring? No — bleed
        // ring comes from the surrounding smooth image, so use a frame where
        // the ring is identical both times: put the stamp fully inside a
        // uniform region.
        let mut uniform = Frame::new(64, 64);
        uniform.data.iter_mut().for_each(|b| *b = 90);
        let mut f1 = uniform.clone();
        stamp(&mut f1, 16, 16);
        let _ = base;
        let mut w = uniform.clone();
        let (p1, _, c1) = analyze_tiered(&mut w, &f1, &[], None, None, &JpegReference { quality: 97 }, &Stationary, &cfg, 1, &mut st).unwrap();
        assert!(c1.is_empty());
        let rv = p1.residuals.iter().find(|r| (r.frame_x, r.frame_y) == (16, 16)).unwrap();
        cache.lock().unwrap().insert(rv.key, Arc::new(vec![7u8; 32 * 32 * 3]));
        // Frame 2: same stamp at (32,32) in the same uniform surround.
        let mut f2 = uniform.clone();
        stamp(&mut f2, 32, 32);
        let mut w = f1.clone();
        let (p2, _, c2) = analyze_tiered(&mut w, &f2, &[], None, None, &JpegReference { quality: 97 }, &Stationary, &cfg, 1, &mut st).unwrap();
        let cell = (32 / 16) + (32 / 16) * p2.grid.cols;
        assert!(matches!(p2.matches[cell as usize], BlockMatch::Cached { .. }), "{:?}", p2.matches[cell as usize]);
        assert_eq!(p2.stats.cached, 1);
        assert_eq!(c2.len(), 1);
        assert!(c2.values().next().unwrap().iter().all(|&b| b == 7));
    }
}

#[cfg(test)]
mod feature_tests {
    use super::*;
    use crate::testimg::smooth;

    /// The point of using high-frequency energy: it separates cases that luma
    /// standard deviation cannot. A blurred edge and a sharp checkerboard can
    /// carry the same contrast, but only one has detail a network could
    /// restore.
    #[test]
    fn hf_energy_separates_blur_from_detail_where_std_does_not() {
        let mut sharp = Frame::new(32, 32);
        let mut blurred = Frame::new(32, 32);
        let mut grain = Frame::new(32, 32);
        for y in 0..32u32 {
            for x in 0..32u32 {
                // 2x2 checkerboard: maximum high-frequency detail.
                let c = if ((x / 2) + (y / 2)) % 2 == 0 { 40u8 } else { 210 };
                sharp.set_px(x, y, [c, c, c]);
                // A smooth ramp of the same span: same contrast, no detail.
                let g = (40.0 + 170.0 * (x as f64 / 31.0)) as u8;
                blurred.set_px(x, y, [g, g, g]);
            }
        }
        grain.data.iter_mut().for_each(|b| *b = 128);
        grain.add_uniform_noise(30, 4);

        let f = |fr: &Frame| (block_luma_std(fr, 0, 0, 32), block_hf_energy(fr, 0, 0, 32));
        let (s_sharp, h_sharp) = f(&sharp);
        let (s_blur, h_blur) = f(&blurred);
        let (s_grain, h_grain) = f(&grain);

        // Contrast is comparable between the checkerboard and the ramp — as a
        // ratio, not a difference: a checkerboard's std is half its span (85)
        // while a linear ramp's is span/sqrt(12) (51), so the same span never
        // gives the same number, only the same order.
        assert!(s_sharp < 2.0 * s_blur, "std {s_sharp} vs {s_blur} is not the same order");
        // ...but high-frequency energy is not, by a wide margin.
        assert!(h_sharp > 8.0 * h_blur, "hf {h_sharp} vs {h_blur}");
        // Grain reads as detail on both measures — it is genuinely
        // high-frequency, so routing it cheap is a choice, not a free lunch.
        assert!(h_grain > h_blur, "hf grain {h_grain} vs blur {h_blur}");
        assert!(s_grain > 5.0);
        // A smooth synthetic frame sits far below the checkerboard.
        assert!(block_hf_energy(&smooth(32, 32, 1), 0, 0, 32) < h_sharp);
    }
    fn miss(n: usize) -> Vec<BlockMatch> { vec![BlockMatch::Missing; n] }

    #[test]
    fn a_lone_block_in_a_different_tier_joins_its_neighbours() {
        // The visible case: one block routed to a different NETWORK in the
        // middle of a uniform region. It reads as a patch, and feathering the
        // seam does not make the two halves match.
        let (cols, rows) = (3, 3);
        let mut t = vec![0u8; 9];
        t[4] = 2; // the centre, surrounded by tier 0
        let out = despeckle_tiers(&t, &miss(9), cols, rows);
        assert_eq!(out[4], 0, "the singleton was left alone");
        assert_eq!(out.iter().filter(|&&x| x == 0).count(), 9);
    }

    #[test]
    fn a_real_texture_boundary_survives() {
        // A genuine edge between smooth and detailed SHOULD change tier.
        // Eroding it spends network time on sky, which is what the cascade
        // exists to avoid — so a block whose neighbours disagree with EACH
        // OTHER is left alone.
        let (cols, rows) = (3, 3);
        // A block on the boundary with a 2-to-1 majority against it. Under a
        // MAJORITY rule it flips and the boundary erodes; under unanimity it
        // stays. My first version used a straight 4x2 split where every block
        // agreed with its own side, so both rules gave the same answer and the
        // test could not tell them apart.
        let t = vec![2, 2, 0,
                     2, 0, 0,
                     0, 0, 0];
        let out = despeckle_tiers(&t, &miss(9), cols, rows);
        assert_eq!(out[4], 0, "the centre flipped on a 2-1 majority");
        assert_eq!(out, t, "a diagonal boundary was smoothed away");
    }

    #[test]
    fn a_block_needs_two_agreeing_neighbours_not_one() {
        // A corner block has two Missing neighbours at most; one agreeing
        // neighbour is not evidence of a field, and acting on it would flip
        // blocks along every edge of the frame.
        let (cols, rows) = (2, 1);
        let t = vec![0u8, 2];
        let out = despeckle_tiers(&t, &miss(2), cols, rows);
        assert_eq!(out, t, "a single neighbour was treated as a majority");
    }

    #[test]
    fn non_missing_neighbours_do_not_vote() {
        // Stationary blocks carry tier 0 by default and are never upscaled.
        // Counting them would drag every block beside a still region down to
        // tier 0 — the cascade would collapse wherever the picture holds.
        let (cols, rows) = (3, 3);
        // The neighbours must carry a DIFFERENT tier from the centre, or the
        // test passes whether or not they vote — which is how my first version
        // of it missed the bug entirely.
        let mut t = vec![0u8; 9];
        t[4] = 2;
        let mut m = miss(9);
        m[1] = BlockMatch::Stationary;
        m[3] = BlockMatch::Stationary;
        m[5] = BlockMatch::Stationary;
        m[7] = BlockMatch::Stationary;
        let out = despeckle_tiers(&t, &m, cols, rows);
        assert_eq!(out[4], 2, "stationary neighbours voted the centre down to tier 0");
    }

    #[test]
    fn one_pass_only_so_a_change_cannot_propagate() {
        // A second pass would let each fixed block become evidence for the
        // next, which is the erosion the unanimity rule is there to prevent.
        // A 1x5 strip of alternating tiers must not collapse to one tier.
        let (cols, rows) = (5, 1);
        let t = vec![0u8, 2, 0, 2, 0];
        let out = despeckle_tiers(&t, &miss(5), cols, rows);
        // Interior blocks have two neighbours that agree with each other and
        // differ from them, so they flip — once.
        assert_eq!(out, vec![0, 0, 2, 0, 0],
                   "a single pass did not produce a single-pass result: {out:?}");
    }

}
