//! Residual image: the missing blocks of a frame packed into a small square
//! image, each with `bleed` pixels of real context around it so the CNN's
//! receptive field sees neighbours instead of a cell edge. The merge undoes
//! this in the upscaled domain.
//!
//! Geometry (all in low-res pixels; multiply by `scale` for the merge):
//! cell = block + 2·bleed; cell `(cx, cy)` starts at `(cx·cell, cy·cell)`;
//! the block proper starts `bleed` further in.

use crate::frame::{Frame, Pad};
use crate::plan::{BlockMatch, FramePlan};

/// What the upscaler is asked to produce for this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Residual {
    /// Nothing changed: no upscale at all.
    None,
    /// Upscale this packed image and stitch per `plan.residuals`.
    Packed(Frame),
    /// Upscale the whole frame; it becomes the output verbatim.
    Whole(Frame),
}

impl Residual {
    pub fn frame(&self) -> Option<&Frame> {
        match self {
            Residual::None => None,
            Residual::Packed(f) | Residual::Whole(f) => Some(f),
        }
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
    let dim = plan.stats.residual_dim;
    debug_assert_eq!(dim, FramePlan::residual_dim_for(plan.residuals.len()));

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
    let mut out = Frame::new(dim * cell, dim * cell);
    for rv in &plan.residuals {
        // In padded coordinates the block starts at (frame_x + bleed); the
        // cell we want starts `bleed` before that, i.e. at (frame_x, frame_y).
        out.copy_block(&padded, cell, rv.frame_x, rv.frame_y, rv.cell_x * cell, rv.cell_y * cell);
    }
    Residual::Packed(out)
}

/// Build the upscaled next frame from the upscaled previous frame, the
/// upscaled residual, and the plan. `scale` is the upscaler's factor.
pub fn merge(prev_up: &Frame, residual_up: Option<&Frame>, plan: &FramePlan, scale: u32, bleed: u32) -> anyhow::Result<Frame> {
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
        let r = residual_up.ok_or_else(|| anyhow::anyhow!("merge: redraw plan without an upscaled frame"))?;
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
        if let BlockMatch::Moved { src_x, src_y } = *m {
            let (x, y) = plan.grid.origin(i);
            out.copy_block(&faded, bs * s, src_x * s, src_y * s, x * s, y * s);
        }
    }

    // 3. Residual blocks.
    if !plan.residuals.is_empty() {
        let r = residual_up.ok_or_else(|| anyhow::anyhow!("merge: plan has residuals but no upscaled residual"))?;
        let cell = bs + 2 * bleed;
        let dim = plan.stats.residual_dim;
        anyhow::ensure!(
            r.width == dim * cell * s && r.height == dim * cell * s,
            "merge: residual_up {}x{} does not match {} cells of {} at scale {}",
            r.width,
            r.height,
            dim,
            cell,
            s
        );
        for rv in &plan.residuals {
            out.copy_block(
                r,
                bs * s,
                (rv.cell_x * cell + bleed) * s,
                (rv.cell_y * cell + bleed) * s,
                rv.frame_x * s,
                rv.frame_y * s,
            );
        }
    }
    Ok(out)
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
        let (plan, working_next) = analyze(&mut w, next, &JpegReference { quality: 97 }, matcher, &cfg).unwrap();
        let res = make_residual(next, &plan, bleed);
        let res_up = res.frame().map(|f| up.upscale(f).unwrap());
        let merged = merge(&prev_up, res_up.as_ref(), &plan, scale, bleed).unwrap();
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

    #[test]
    fn identical_frames_need_no_residual() {
        let prev = smooth(64, 64, 0);
        let cfg = AnalyzeConfig { block_size: 16, ..Default::default() };
        let mut w = prev.clone();
        let (plan, _) = analyze(&mut w, &prev, &JpegReference { quality: 97 }, &Stationary, &cfg).unwrap();
        assert_eq!(make_residual(&prev, &plan, 1), Residual::None);
        let up = NearestUpscaler { scale: 2 }.upscale(&prev).unwrap();
        assert_eq!(merge(&up, None, &plan, 2, 1).unwrap(), up);
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
}
