//! Residual image: the missing blocks of a frame packed into a small square
//! image, each with `bleed` pixels of real context around it so the CNN's
//! receptive field sees neighbours instead of a cell edge. The merge undoes
//! this in the upscaled domain.
//!
//! Geometry (all in low-res pixels; multiply by `scale` for the merge):
//! cell = block + 2·bleed; cell `(cx, cy)` starts at `(cx·cell, cy·cell)`;
//! the block proper starts `bleed` further in.

use crate::frame::{Frame, Pad};
use crate::plan::{BlockMatch, FramePlan, MAX_TIERS};
use crate::predict::CachedBlocks;

/// What the upscalers are asked to produce for this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Residual {
    /// Nothing changed: no upscale at all.
    None,
    /// One packed image per tier (`None` where a tier has no blocks);
    /// stitch per `plan.residuals`.
    Packed([Option<Frame>; MAX_TIERS]),
    /// Upscale the whole frame (tier 0); it becomes the output verbatim.
    Whole(Frame),
}

impl Residual {
    /// The image for `tier`, if any.
    pub fn tier(&self, t: usize) -> Option<&Frame> {
        match self {
            Residual::None => None,
            Residual::Packed(v) => v[t].as_ref(),
            Residual::Whole(f) => (t == 0).then_some(f),
        }
    }
    /// Tier-0 image (what a single-upscaler pipeline needs).
    pub fn frame(&self) -> Option<&Frame> {
        self.tier(0)
    }
}

pub fn make_residual(next_original: &Frame, plan: &FramePlan, bleed: u32) -> Residual {
    if plan.redraw {
        return Residual::Whole(next_original.clone());
    }
    if plan.residuals.is_empty() {
        return Residual::None;
    }
    let bs = plan.grid.block_size;
    let cell = bs + 2 * bleed;

    // REPLICATE, not black. The old comment argued for black on the grounds
    // that "the merge never reads the bleed ring back, so it only affects the
    // ring's influence on the block". That reasoning is backwards, and it is
    // the cause of the reported seams.
    //
    // The bleed ring is not decoration — it is the CONTEXT the CNN sees around
    // each residual block, and the network's receptive field carries that
    // context inward. Filling it with black darkens the block's own outer
    // pixels, which the merge very much does read back. Not reading the ring
    // itself is irrelevant; the damage is already inside the block.
    //
    // The observation that identified this: the dark lines MOVE WITH THE
    // CAMERA and grow and shrink with motion locality. CNN tiles are a fixed
    // grid and cannot do that. Residual blocks are chosen by the block matcher
    // and therefore follow motion — so a per-block edge artefact is the only
    // thing that behaves this way, which rules out the CNN tiling I had
    // assumed and points here.
    //
    // Replicate makes the ring an extension of the block's own content, so the
    // CNN sees a plausible continuation instead of a cliff to zero.
    let padded = next_original.padded(bleed, Pad::Replicate);
    let mut imgs: [Option<Frame>; MAX_TIERS] = Default::default();
    for (t, img) in imgs.iter_mut().enumerate() {
        let (cols, rows) = plan.stats.residual_tiers[t];
        if cols > 0 {
            *img = Some(Frame::new(cols * cell, rows * cell));
        }
    }
    for rv in &plan.residuals {
        let out = imgs[rv.tier as usize].as_mut().expect("tier image sized for its blocks");
        // In padded coordinates the block starts at (frame_x + bleed); the
        // cell we want starts `bleed` before that, i.e. at (frame_x, frame_y).
        out.copy_block(&padded, cell, rv.frame_x, rv.frame_y, rv.cell_x * cell, rv.cell_y * cell);
    }
    Residual::Packed(imgs)
}

/// Build the upscaled next frame from the upscaled previous frame, the
/// older upscaled references (same order as given to `analyze`), the
/// upscaled residual (tier 0), and the plan. `scale` is the upscaler's factor.
pub fn merge(prev_up: &Frame, older_up: &[&Frame], residual_up: Option<&Frame>, plan: &FramePlan, scale: u32, bleed: u32) -> anyhow::Result<Frame> {
    let tiers: [Option<&Frame>; MAX_TIERS] = [residual_up, None, None];
    merge_tiered(prev_up, older_up, None, &tiers, &CachedBlocks::new(), plan, scale, bleed)
}

/// [`merge`] with one upscaled residual image per tier and the bytes of
/// any `Cached` blocks (`(block·scale)²` RGB each).
pub fn merge_tiered(prev_up: &Frame, older_up: &[&Frame], plate_up: Option<&Frame>, residual_up: &[Option<&Frame>; MAX_TIERS], cached: &CachedBlocks, plan: &FramePlan, scale: u32, bleed: u32) -> anyhow::Result<Frame> {
    let bs = plan.grid.block_size;
    let s = scale;
    anyhow::ensure!(
        prev_up.width == plan.grid.width() * s && prev_up.height == plan.grid.height() * s,
        "merge: prev_up {}x{} does not match grid {}x{} at scale {}",
        prev_up.width,
        prev_up.height,
        plan.grid.width(),
        plan.grid.height(),
        s
    );

    if plan.redraw {
        let r = residual_up[0].ok_or_else(|| anyhow::anyhow!("merge: redraw plan without an upscaled frame"))?;
        anyhow::ensure!((r.width, r.height) == (prev_up.width, prev_up.height), "merge: redraw frame size mismatch");
        return Ok(r.clone());
    }

    // 1. Fade the previous frame — into a separate buffer so moved-block
    //    copies below read faded sources, exactly as the low-res pass did.
    let mut faded = prev_up.clone();
    for f in &plan.fades {
        faded.add_scalar_block(f.x * s, f.y * s, bs * s, f.scalar);
    }

    // 2. Start from the faded previous frame: every Stationary block is
    //    already in place, so only Moved and Missing blocks need work.
    let mut out = faded.clone();
    for (i, m) in plan.matches.iter().enumerate() {
        let (x, y) = plan.grid.origin(i);
        match *m {
            BlockMatch::Moved { src_x, src_y } => out.copy_block(&faded, bs * s, src_x * s, src_y * s, x * s, y * s),
            BlockMatch::Recalled { r } => {
                let old = older_up.get(r as usize).ok_or_else(|| anyhow::anyhow!("merge: plan recalls reference {r} but only {} were given", older_up.len()))?;
                anyhow::ensure!((old.width, old.height) == (prev_up.width, prev_up.height), "merge: reference {r} size mismatch");
                out.copy_block(old, bs * s, x * s, y * s, x * s, y * s);
            }
            BlockMatch::Plated { px, py } => {
                let p = plate_up.ok_or_else(|| anyhow::anyhow!("merge: plan uses the plate but no upscaled plate was given"))?;
                anyhow::ensure!(p.block_in_bounds(px * s, py * s, bs * s), "merge: plated block ({px},{py}) outside the {}x{} upscaled plate", p.width, p.height);
                out.copy_block(p, bs * s, px * s, py * s, x * s, y * s);
            }
            BlockMatch::Cached { key } => {
                let b = cached.get(&key).ok_or_else(|| anyhow::anyhow!("merge: cached block {key:#x} not attached"))?;
                let side = bs * s;
                anyhow::ensure!(b.len() == (side * side * 3) as usize, "merge: cached block has {} bytes, expected {}", b.len(), side * side * 3);
                let blk = Frame::from_rgb(side, side, (**b).clone())?;
                out.copy_block(&blk, side, 0, 0, x * s, y * s);
            }
            _ => {}
        }
    }

    // 3. Residual blocks, from their tier's image.
    let cell = bs + 2 * bleed;
    for t in 0..MAX_TIERS {
        let (cols, rows) = plan.stats.residual_tiers[t];
        if cols == 0 {
            continue;
        }
        let r = residual_up[t].ok_or_else(|| anyhow::anyhow!("merge: plan has tier-{t} residuals but no upscaled image for that tier"))?;
        anyhow::ensure!(
            r.width == cols * cell * s && r.height == rows * cell * s,
            "merge: tier {t} residual_up {}x{} does not match {}x{} cells of {} at scale {}",
            r.width,
            r.height,
            cols,
            rows,
            cell,
            s
        );
    }
    for rv in &plan.residuals {
        let r = residual_up[rv.tier as usize].expect("checked above");
        blend_block(
            &mut out, r, bs * s, bleed * s,
            (rv.cell_x * cell) * s, (rv.cell_y * cell) * s,
            rv.frame_x * s, rv.frame_y * s,
        );
    }
    Ok(out)
}

/// Write one upscaled block over `out`, feathered across its bleed ring.
///
/// # Why not `copy_block`
///
/// It used to be one. A hard `bs·s` copy pastes a block the network just
/// upscaled edge-to-edge against neighbours that came from somewhere else
/// entirely — the faded previous frame, a moved copy, the block cache, or a
/// DIFFERENT cascade tier's network. Any difference in tone or detail between
/// the two lands on a straight line, and the block grid becomes visible.
/// Reported as "blocky artifacts because of the crossover".
///
/// # The ring was already there
///
/// `make_residual` gives every block `bleed` pixels of real context and the
/// network upscales the whole cell, ring included. The old merge read back
/// only the interior and threw the ring away — so the overlap needed to blend
/// the seam was computed on every block, at full network cost, and discarded.
///
/// This spends it: weight 0 at the ring's outer edge rising to 1 at the block
/// boundary, so a residual fades into whatever was already there instead of
/// cutting against it. Inside the block proper the weight is 1 and the result
/// is byte-identical to the old copy.
///
/// `dst_x`/`dst_y` are the BLOCK's position in the frame; the ring is written
/// at `dst - ring`, clamped at the frame edges — a block on the boundary has
/// no neighbour to blend with on that side and simply writes less.
pub fn blend_block(
    out: &mut Frame,
    src: &Frame,
    block: u32,
    ring: u32,
    src_cell_x: u32,
    src_cell_y: u32,
    dst_x: u32,
    dst_y: u32,
) {
    // NO `ring == 0` SHORTCUT. I wrote one and could not break it: with
    // `ring = 0` the span is the block, every offset is interior, `ramp`
    // returns 1.0, and the loop writes each pixel exactly — the same bytes
    // `copy_block` would. `a_zero_ring_is_the_old_hard_copy` still pins that,
    // now on the one path that provides it.
    let span = block + 2 * ring;
    for j in 0..span {
        // Frame coordinates, skipping anything off the edge.
        let fy = (dst_y + j) as i64 - ring as i64;
        if fy < 0 || fy as u32 >= out.height {
            continue;
        }
        let wy = ramp(j, ring, block);
        for i in 0..span {
            let fx = (dst_x + i) as i64 - ring as i64;
            if fx < 0 || fx as u32 >= out.width {
                continue;
            }
            let w = wy * ramp(i, ring, block);
            if w <= 0.0 {
                continue;
            }
            let sp = src.px(src_cell_x + i, src_cell_y + j);
            if w >= 1.0 {
                out.set_px(fx as u32, fy as u32, sp);
                continue;
            }
            let dp = out.px(fx as u32, fy as u32);
            let mix = |a: u8, b: u8| ((a as f32) * (1.0 - w) + (b as f32) * w + 0.5) as u8;
            out.set_px(fx as u32, fy as u32,
                       [mix(dp[0], sp[0]), mix(dp[1], sp[1]), mix(dp[2], sp[2])]);
        }
    }
}

/// Feather weight at offset `i` across a `ring + block + ring` span.
///
/// Linear, and 1.0 across the whole block interior. A smoothstep would be
/// smoother in the ring and would also make the interior non-uniform at the
/// boundary, which is where an exact copy matters most — the block proper is
/// the network's actual output and should arrive unmodified.
fn ramp(i: u32, ring: u32, block: u32) -> f32 {
    // Distance to the nearer end of the span, saturating to 1.0 across the
    // interior. An explicit `if interior { return 1.0 }` was here and could
    // not be broken either: the clamp already covers it, because the nearest
    // interior offset is `ring + 1` away from the edge and `(ring+1)/(ring+1)`
    // is exactly 1.
    let d = if i < ring { i + 1 } else { ring + block + ring - i };
    (d as f32 / (ring + 1) as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testimg::smooth;
    use crate::gate::JpegReference;
    use crate::matcher::{Diamond, Stationary};
    use crate::predict::{analyze, AnalyzeConfig};
    use crate::upscale::{NearestUpscaler, Upscaler};

    /// With a nearest-neighbour upscaler the whole pipeline is an exact
    /// identity: merge(upscale(prev), upscale(residual)) must equal
    /// upscale(working_next). This pins the residual/merge geometry —
    /// including bleed — without any CNN in the loop.
    fn roundtrip(prev: &Frame, next: &Frame, bleed: u32, scale: u32, matcher: &dyn crate::matcher::BlockMatcher) {
        let cfg = AnalyzeConfig { block_size: 16, bleed, ..Default::default() };
        let mut up = NearestUpscaler { scale };
        // The previous *output* predates this frame's fades — as in the pipeline.
        let prev_up = up.upscale(prev).unwrap();
        let mut w = prev.clone();
        let (plan, working_next) = analyze(&mut w, next, &[], &JpegReference { quality: 97 }, matcher, &cfg).unwrap();
        let res = make_residual(next, &plan, bleed);
        let res_up = res.frame().map(|f| up.upscale(f).unwrap());
        let merged = merge(&prev_up, &[], res_up.as_ref(), &plan, scale, bleed).unwrap();
        let expect = up.upscale(&working_next).unwrap();
        assert_eq!(merged, expect, "roundtrip mismatch (bleed {bleed}, scale {scale}, stats {:?})", plan.stats);
    }

    #[test]
    fn residual_geometry_roundtrips_exactly() {
        let prev = smooth(96, 64, 0);
        let mut next = prev.clone();
        // Two scribbled blocks, one at the frame edge (exercises the black bleed).
        for y in 16..32 {
            for x in 32..48 {
                next.set_px(x, y, [255, 0, 255]);
            }
        }
        for y in 48..64 {
            for x in 80..96 {
                next.set_px(x, y, [(x * 5) as u8, 30, 200]);
            }
        }
        for &bleed in &[0u32, 1, 3] {
            for &scale in &[1u32, 2] {
                roundtrip(&prev, &next, bleed, scale, &Stationary);
            }
        }
    }

    #[test]
    fn moved_blocks_and_fades_roundtrip() {
        let prev = smooth(96, 96, 3);
        let mut next = prev.clone();
        let moved = prev.crop(44, 50, 16, 16);
        next.copy_block(&moved, 16, 0, 0, 48, 48);
        next.add_scalar_block(0, 0, 16, 15);
        for y in 64..80 {
            for x in 16..32 {
                next.set_px(x, y, [0, 255, 0]);
            }
        }
        roundtrip(&prev, &next, 1, 2, &Diamond::default());
    }

    #[test]
    fn redraw_roundtrips() {
        let prev = smooth(64, 64, 0);
        let mut next = Frame::new(64, 64);
        for (i, b) in next.data.iter_mut().enumerate() {
            *b = (i * 31 % 251) as u8;
        }
        roundtrip(&prev, &next, 1, 2, &Stationary);
    }

    /// Plated blocks round-trip exactly: a pan over a world, a plate built
    /// from the whole window, and every frame merged from the plate's
    /// nearest-neighbour upscale must equal the upscale of its low-res
    /// reconstruction. Also pins that the pan's leading edge — content the
    /// previous frame never saw — is Plated, not Missing.
    #[test]
    fn plated_blocks_roundtrip_and_explain_the_leading_edge() {
        use crate::plate::Plate;
        use crate::predict::{analyze_tiered, AnalyzeState};
        let world = smooth(240, 96, 6);
        let (fw, fh) = (128u32, 96u32);
        let frames: Vec<Frame> = (0..8u32).map(|i| world.crop(8 * i, 0, fw, fh)).collect();
        let refs: Vec<&Frame> = frames.iter().collect();
        let plate = Plate::build(&refs, 0.2);
        assert_eq!(plate.lo.width, 8 * 7 + fw);
        let cfg = AnalyzeConfig { block_size: 16, bleed: 1, ..Default::default() };
        let mut up = NearestUpscaler { scale: 2 };
        let plate_up = up.upscale(&plate.lo).unwrap();
        let mut st = AnalyzeState::default();
        let mut w = frames[0].clone();
        let mut prev_up = up.upscale(&frames[0]).unwrap();
        for i in 1..frames.len() {
            let (plan, working_next, cached) = analyze_tiered(&mut w, &frames[i], &[], Some(plate.view(i)), &JpegReference { quality: 97 }, &Stationary, &cfg, 1, &mut st).unwrap();
            assert!(!plan.redraw);
            assert_eq!(plan.stats.missing, 0, "frame {i}: {:?}", plan.stats);
            // Stationary is impossible under an 8 px pan; everything is Plated.
            assert_eq!(plan.stats.plated, plan.grid.len() as u32, "frame {i}: {:?}", plan.stats);
            let res = make_residual(&frames[i], &plan, 1);
            let tiers: [Option<Frame>; MAX_TIERS] = [res.frame().map(|f| up.upscale(f).unwrap()), None, None];
            let refs: [Option<&Frame>; MAX_TIERS] = [tiers[0].as_ref(), None, None];
            let merged = merge_tiered(&prev_up, &[], Some(&plate_up), &refs, &cached, &plan, 2, 1).unwrap();
            assert_eq!(merged, up.upscale(&working_next).unwrap(), "frame {i}");
            // And, because the plate is exact here, the output IS the frame.
            assert_eq!(working_next, frames[i], "frame {i}");
            prev_up = merged;
            w = working_next;
        }
        // A plan that uses the plate must be refused a merge without one.
        let (plan, _, _) = analyze_tiered(&mut frames[0].clone(), &frames[1], &[], Some(plate.view(1)), &JpegReference { quality: 97 }, &Stationary, &cfg, 1, &mut st).unwrap();
        let none: [Option<&Frame>; MAX_TIERS] = [None, None, None];
        assert!(merge_tiered(&prev_up, &[], None, &none, &CachedBlocks::new(), &plan, 2, 1).is_err());
    }

    /// Recalled blocks round-trip exactly through merge with a reference ring.
    #[test]
    fn recalled_blocks_roundtrip() {
        let cfg = AnalyzeConfig { block_size: 16, bleed: 1, ..Default::default() };
        let mut up = NearestUpscaler { scale: 2 };
        let base = smooth(64, 64, 4);
        let mut occluded = base.clone();
        for y in 16..48 {
            for x in 32..48 {
                occluded.set_px(x, y, [250, 250, 250]);
            }
        }
        let older_lo = base.clone();
        let older_hi = up.upscale(&older_lo).unwrap();
        let prev_up = up.upscale(&occluded).unwrap();
        let mut w = occluded.clone();
        let (plan, working_next) = analyze(&mut w, &base, &[&older_lo], &JpegReference { quality: 97 }, &Stationary, &cfg).unwrap();
        assert_eq!(plan.stats.recalled, 2);
        let res = make_residual(&base, &plan, 1);
        let res_up = res.frame().map(|f| up.upscale(f).unwrap());
        let merged = merge(&prev_up, &[&older_hi], res_up.as_ref(), &plan, 2, 1).unwrap();
        assert_eq!(merged, up.upscale(&working_next).unwrap());
    }

    /// Three tiers of nearest upscalers round-trip exactly: routing changes
    /// which image a block comes from, never what it contains.
    #[test]
    fn tiered_residuals_roundtrip() {
        use crate::predict::{analyze_tiered, AnalyzeState};
        // Pinned to the std metric: this fixture separates under luma std-dev,
        // not under the default high-frequency metric (a gentle ramp has no
        // more HF energy than a flat block, so both would route to bicubic).
        let cfg = AnalyzeConfig { block_size: 16, bleed: 1, tier_std: [2.0, 12.0], tier_metric: crate::predict::TierMetric::Std, ..Default::default() };
        let prev = smooth(64, 64, 1);
        let mut next = prev.clone();
        for y in 0..16 {
            for x in 0..16 {
                next.set_px(x, y, [200, 200, 200]);
                next.set_px(16 + x, y, [(100 + x + y) as u8; 3]);
                let c = if (x + y) % 2 == 0 { 30 } else { 220 };
                next.set_px(32 + x, y, [c, c, c]);
            }
        }
        let mut up = NearestUpscaler { scale: 2 };
        let prev_up = up.upscale(&prev).unwrap();
        let mut w = prev.clone();
        let mut st = AnalyzeState::default();
        let (plan, working_next, _) = analyze_tiered(&mut w, &next, &[], None, &JpegReference { quality: 97 }, &Stationary, &cfg, 3, &mut st).unwrap();
        assert_eq!(plan.stats.residual_tiers, [(1, 1), (1, 1), (1, 1)]);
        let res = make_residual(&next, &plan, 1);
        let ups: Vec<Option<Frame>> = (0..3).map(|t| res.tier(t).map(|f| up.upscale(f).unwrap())).collect();
        let refs: [Option<&Frame>; 3] = [ups[0].as_ref(), ups[1].as_ref(), ups[2].as_ref()];
        let merged = merge_tiered(&prev_up, &[], None, &refs, &CachedBlocks::new(), &plan, 2, 1).unwrap();
        assert_eq!(merged, up.upscale(&working_next).unwrap());
    }

    #[test]
    fn identical_frames_need_no_residual() {
        let prev = smooth(64, 64, 0);
        let cfg = AnalyzeConfig { block_size: 16, ..Default::default() };
        let mut w = prev.clone();
        let (plan, _) = analyze(&mut w, &prev, &[], &JpegReference { quality: 97 }, &Stationary, &cfg).unwrap();
        assert_eq!(make_residual(&prev, &plan, 1), Residual::None);
        let up = NearestUpscaler { scale: 2 }.upscale(&prev).unwrap();
        assert_eq!(merge(&up, &[], None, &plan, 2, 1).unwrap(), up);
    }
}

#[cfg(test)]
mod bleed_ring_tests {
    use super::*;
    use crate::frame::Frame;

    /// The bleed ring must continue the block's content, not cut to black.
    ///
    /// This was `Pad::Black`, justified by "the merge never reads the bleed
    /// ring back". That is true and irrelevant: the ring is the CONTEXT the
    /// CNN sees, and its receptive field carries a black cliff INWARD, so the
    /// block's own outer pixels — which the merge does read — come back dark.
    ///
    /// The symptom is specific and worth recording, because it is what
    /// distinguishes this from CNN tile-grid artefacts: the dark lines move
    /// with the camera and grow and shrink with motion locality. A CNN tile
    /// grid is fixed and cannot do that; residual blocks are chosen by the
    /// block matcher and follow motion.
    #[test]
    fn the_bleed_ring_continues_the_image_rather_than_going_black() {
        // A bright frame, so "black bleed" is unmistakable in the packed cell.
        let (w, h) = (64u32, 64u32);
        let mut f = Frame::new(w, h);
        for y in 0..h { for x in 0..w { f.set_px(x, y, [220, 210, 200]); } }

        let bleed = 4u32;
        let padded = f.padded(bleed, Pad::Replicate);
        // The ring immediately outside the frame must carry the frame's value.
        let px = padded.px(0, bleed + 1);
        assert_eq!(px, [220, 210, 200],
            "the bleed ring went to {px:?} instead of continuing the image");
        let px2 = padded.px(bleed + 1, 0);
        assert_eq!(px2, [220, 210, 200], "top ring: {px2:?}");
    }

    #[test]
    fn a_black_ring_would_be_visibly_different() {
        // Guards the test above from passing vacuously: if `Pad::Black` and
        // `Pad::Replicate` produced the same thing here, the assertion proves
        // nothing.
        let mut f = Frame::new(32, 32);
        for y in 0..32 { for x in 0..32 { f.set_px(x, y, [220, 210, 200]); } }
        let black = f.padded(4, Pad::Black);
        let repl = f.padded(4, Pad::Replicate);
        assert_ne!(black.px(0, 5), repl.px(0, 5),
            "the two padding modes are indistinguishable — this test cannot detect a regression");
    }
    #[test]
    fn the_block_interior_is_an_exact_copy() {
        // The block proper is the network's actual output. Feathering it would
        // soften the very pixels the upscale produced, which is the opposite of
        // the point — only the RING blends.
        let mut out = Frame::new(20, 20);
        let mut src = Frame::new(12, 12);
        for y in 0..12 { for x in 0..12 { src.set_px(x, y, [200, 100, 50]); } }
        blend_block(&mut out, &src, 8, 2, 0, 0, 4, 4);
        for y in 4..12 { for x in 4..12 {
            assert_eq!(out.px(x, y), [200, 100, 50], "interior at {x},{y} was blended");
        }}
    }

    #[test]
    fn the_ring_fades_instead_of_cutting() {
        // The reported artefact: a hard edge where an upscaled block meets a
        // neighbour from a different source. The ring should be strictly
        // between the two values, and monotone inward.
        let mut out = Frame::new(20, 20);
        for y in 0..20 { for x in 0..20 { out.set_px(x, y, [0, 0, 0]); } }
        let mut src = Frame::new(12, 12);
        for y in 0..12 { for x in 0..12 { src.set_px(x, y, [200, 200, 200]); } }
        blend_block(&mut out, &src, 8, 2, 0, 0, 4, 4);

        // Walking inward along a row: background, then rising, then full.
        let outer = out.px(2, 8)[0];
        let inner = out.px(3, 8)[0];
        let full = out.px(4, 8)[0];
        assert!(outer < inner, "the outer ring is not lighter than the background: {outer} vs {inner}");
        assert!(inner < full, "the ring did not rise toward the block: {inner} vs {full}");
        assert_eq!(full, 200, "the block edge is not the block's own value");
        // And it is a blend, not a copy — the whole complaint.
        assert!(outer > 0 && outer < 200, "the ring is a hard edge, not a fade: {outer}");
    }

    #[test]
    fn a_block_at_the_frame_edge_writes_less_rather_than_wrapping() {
        // A block on the boundary has no neighbour to blend with on that side.
        // Without the clamp the ring wraps to the far edge, which is a bright
        // line down the opposite side of the picture.
        let mut out = Frame::new(16, 16);
        for y in 0..16 { for x in 0..16 { out.set_px(x, y, [10, 10, 10]); } }
        let mut src = Frame::new(12, 12);
        for y in 0..12 { for x in 0..12 { src.set_px(x, y, [250, 250, 250]); } }
        // Block at the very top-left: two rows/cols of ring fall outside.
        blend_block(&mut out, &src, 8, 2, 0, 0, 0, 0);
        for y in 0..16 {
            assert_eq!(out.px(15, y), [10, 10, 10], "the ring wrapped to the right edge at row {y}");
        }
        assert_eq!(out.px(4, 4), [250, 250, 250], "the block itself did not land");
    }

    #[test]
    fn a_zero_ring_is_the_old_hard_copy() {
        // `bleed = 0` is a valid configuration and there is no context to
        // feather across; the result must be byte-identical to `copy_block`.
        let mut a = Frame::new(16, 16);
        let mut b = Frame::new(16, 16);
        let mut src = Frame::new(8, 8);
        for y in 0..8 { for x in 0..8 { src.set_px(x, y, [7, 8, 9]); } }
        blend_block(&mut a, &src, 8, 0, 0, 0, 4, 4);
        b.copy_block(&src, 8, 0, 0, 4, 4);
        assert_eq!(a.data, b.data, "a zero ring did not reproduce the hard copy");
    }

    #[test]
    fn the_ramp_is_one_across_the_whole_interior_and_rises_at_both_ends() {
        // Both edges, because a ramp that only feathered the leading side
        // would leave the trailing seam exactly as visible as before.
        let (ring, block) = (2u32, 8u32);
        for i in ring..ring + block {
            assert_eq!(ramp(i, ring, block), 1.0, "interior offset {i} is not 1.0");
        }
        assert!(ramp(0, ring, block) < ramp(1, ring, block));
        let last = ring + block + ring - 1;
        assert!(ramp(last, ring, block) < ramp(last - 1, ring, block),
                "the trailing edge does not fall off");
        assert!(ramp(0, ring, block) > 0.0, "the outermost ring row contributes nothing");
    }

}
