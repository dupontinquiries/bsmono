//! seams — the thin bright/dark lines along block boundaries.
//!
//! Reported: faint 1-pixel vertical and horizontal lines on a grid, sometimes brighter than
//! their surroundings, sometimes darker. This module is the analysis and the measurement;
//! the fixes it points at are listed at the bottom and are not all built.
//!
//! ## Three different faults produce a grid of lines, and they are not the same bug
//!
//! **1. Residual-packing contamination — the one that produces a HARD 1px rim.**
//!
//! `make_residual` packs the cells that need the network into a small square image, each with
//! `bleed` pixels of real context around it. Beyond that ring sits *the next cell*: content
//! from somewhere else in the frame entirely, with a step change at the join. The network
//! sees that step as an edge and rings against it, and the ringing reaches inward by its
//! receptive radius.
//!
//! [`receptive_radius`] computes that radius from the op list, and it is the number that
//! decides whether the configured bleed is enough. It is usually not close:
//! SRVGGNetCompact's 16 same-pad 3×3 convs give a radius of 16 input pixels; RRDBNet's 23
//! blocks give many times that. The default `bleed` in this pipeline is **1**. So every
//! block's outer pixels are computed partly from unrelated content — which is exactly a
//! thin rim, on a grid, at the block pitch.
//!
//! The feathered merge (`blend_rect`'s ring) cannot fix this. It blends contaminated pixels
//! with their neighbours; the contamination is inside the block before any blending happens.
//!
//! **2. Level mismatch between a CNN block and a copied one — a SOFT step, no rim.**
//!
//! A reused block is a copy of an earlier upscale; a fresh block came out of the network
//! this frame. Both are legitimate under the gate — the gate bounds SSE, and a uniform
//! half-level offset over a 32×32 block is a tiny SSE and a visible edge. This produces a
//! step at the boundary with no ringing on either side, and it is the one that looks like a
//! slightly darker or brighter *region* rather than a line.
//!
//! **3. Rounding at the merge — a true 1px line, geometric.**
//!
//! Block coordinates multiplied by `scale` land exactly, but the feather ramp is evaluated
//! in float and rounded per pixel. Where two blocks' ramps both round to a weight just under
//! 1.0, the destination pixel keeps a fraction of whatever was underneath. On a static
//! background that fraction is the previous frame and invisible; over a cut it is not.
//!
//! ## Telling them apart, which is the point of measuring
//!
//! [`seam_profile`] compares the mean absolute luma step ACROSS block boundaries with the
//! mean absolute step at interior columns and rows of the same frame. On a frame with no
//! seam problem the two are equal — block boundaries are not special places in a picture, so
//! any excess is the pipeline's. The ratio is reported in dB, and the sign of the step
//! (whether boundaries are consistently brighter or darker) separates fault 2 from faults 1
//! and 3: a rim rings both ways and averages out, a level mismatch does not.

use crate::frame::Frame;
use crate::net::{Network, Op};

/// The network's receptive radius, in INPUT pixels: how far from a cell's edge the
/// neighbouring content can still influence a pixel.
///
/// Every conv here is 3×3, so each adds radius 1 at the level it runs on; a level-2 buffer's
/// pixel is half an input pixel, so a conv there contributes 0.5 input pixels. `in_scale: 2`
/// reads the source as if nearest-upsampled, which halves that conv's reach in source terms.
/// The 4×4 stride-2 deconv reaches 1.5 source pixels.
///
/// Rounded up, and deliberately an over-estimate where it is ambiguous: the number is used
/// to decide whether `bleed` is sufficient, and an under-estimate would report a clean run
/// on a contaminated one.
pub fn receptive_radius(net: &Network) -> u32 {
    let mut radius = 0.0f64;
    for op in &net.ops {
        match op {
            Op::Conv(c) => {
                // The level of the buffer this conv writes decides the pixel pitch.
                let level = net.bufs.get(c.dst).map(|b| b.level).unwrap_or(1).max(1) as f64;
                let step = 1.0 / level;
                // `in_scale = 2` reads an upsampled source, so one output step is half a
                // source pixel of reach.
                let step = if c.in_scale >= 2 { step / c.in_scale as f64 } else { step };
                radius += step;
            }
            Op::Deconv(d) => {
                let level = net.bufs.get(d.src).map(|b| b.level).unwrap_or(1).max(1) as f64;
                radius += 1.5 / level;
            }
            // Copy and AddScaled are pointwise: they move channels, not neighbourhoods.
            Op::Copy { .. } | Op::AddScaled { .. } => {}
        }
    }
    radius.ceil() as u32
}

/// What the configured bleed does and does not cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BleedCheck {
    /// The network's reach, in input pixels.
    pub radius: u32,
    /// The bleed the pipeline is configured with.
    pub bleed: u32,
    /// Input pixels of each block's edge that can see the neighbouring cell. 0 = covered.
    pub exposed: u32,
    /// What the model itself declares for tile overlap (`Network::pre_pad`).
    pub pre_pad: u32,
}

impl BleedCheck {
    pub fn new(net: &Network, bleed: u32) -> BleedCheck {
        let radius = receptive_radius(net);
        BleedCheck { radius, bleed, exposed: radius.saturating_sub(bleed), pre_pad: net.pre_pad }
    }

    /// The model's OWN answer to this question, for the tiler. `Network::pre_pad` is
    /// documented as "the overlap that hides tile seams" — Real-ESRGAN ships 10, upconv_7
    /// ships an exact 7 — and the residual packing does not use it. Two numbers for one
    /// physical requirement, and the smaller one is the default.
    pub fn disagrees_with_tiler(&self) -> bool {
        self.bleed < self.pre_pad
    }

    pub fn covered(&self) -> bool {
        self.exposed == 0
    }

    /// One line for the run log. Says the number rather than a verdict, because for a deep
    /// model full coverage is not affordable and "insufficient" every time would be noise.
    pub fn describe(&self) -> String {
        if self.covered() {
            format!("bleed {} covers the network's {}px reach", self.bleed, self.radius)
        } else {
            format!(
                "bleed {} vs {}px reach: the outer {}px of every block can see the next cell \
                 in the residual image (grid-pitch rim risk)",
                self.bleed, self.radius, self.exposed
            )
        }
    }
}

/// Mean absolute luma step across a set of columns.
fn column_steps(f: &Frame, xs: impl Iterator<Item = u32>) -> (f64, f64, u64) {
    let (mut abs, mut signed, mut n) = (0.0f64, 0.0f64, 0u64);
    for x in xs {
        if x == 0 || x >= f.width {
            continue;
        }
        for y in 0..f.height {
            let l = |p: [u8; 3]| (2 * p[0] as i32 + 5 * p[1] as i32 + p[2] as i32) as f64 / 8.0;
            let d = l(f.px(x, y)) - l(f.px(x - 1, y));
            abs += d.abs();
            signed += d;
            n += 1;
        }
    }
    (abs, signed, n)
}

fn row_steps(f: &Frame, ys: impl Iterator<Item = u32>) -> (f64, f64, u64) {
    let (mut abs, mut signed, mut n) = (0.0f64, 0.0f64, 0u64);
    for y in ys {
        if y == 0 || y >= f.height {
            continue;
        }
        for x in 0..f.width {
            let l = |p: [u8; 3]| (2 * p[0] as i32 + 5 * p[1] as i32 + p[2] as i32) as f64 / 8.0;
            let d = l(f.px(x, y)) - l(f.px(x, y - 1));
            abs += d.abs();
            signed += d;
            n += 1;
        }
    }
    (abs, signed, n)
}

/// How much the block grid shows in a finished frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeamProfile {
    /// Mean |luma step| across block boundaries.
    pub boundary: f64,
    /// Mean |luma step| at interior lines, the control.
    pub interior: f64,
    /// Mean SIGNED step at boundaries. Near zero means a rim (rings both ways); consistently
    /// non-zero means a level mismatch between what is on either side.
    pub boundary_signed: f64,
    /// The same, at interior lines — the control for the signed measure.
    pub interior_signed: f64,
    /// Mean SIGNED step one pixel INSIDE the boundary (at `pitch + 1`).
    ///
    /// This is the discriminator, and it exists because the obvious one does not work. A rim
    /// does NOT cancel in the boundary's signed mean: ringing puts a dark pixel on one side
    /// and a bright one on the other, so the step across the boundary is large and signed,
    /// exactly like a level offset. What separates them is the NEXT step in: a rim is a local
    /// extremum, so the step immediately inside reverses sign as the picture comes back to
    /// its true level, while a level offset is monotone and the next step is ~0.
    pub flank_signed: f64,
}

impl SeamProfile {
    /// Excess boundary contrast in dB. 0 dB means block boundaries are indistinguishable
    /// from any other line in the picture, which is what a clean run looks like.
    pub fn excess_db(&self) -> f64 {
        if self.interior <= 1e-9 {
            return if self.boundary <= 1e-9 { 0.0 } else { f64::INFINITY };
        }
        20.0 * (self.boundary / self.interior).max(1e-9).log10()
    }

    /// The DC step across a boundary, in luma levels, with the interior as control.
    ///
    /// This is a separate measure from `excess_db` and not a refinement of it, because the
    /// two faults hide from opposite metrics. On noisy content a one-level block-to-block
    /// offset is invisible in mean |step| — the noise is twenty levels and the offset is one
    /// — but it shows cleanly in the SIGNED mean, because noise cancels and a DC step does
    /// not. A rim is the reverse: it rings both ways, so it moves |step| and leaves the
    /// signed mean near zero. Measuring only one of them misses half the reports.
    pub fn level_step(&self) -> f64 {
        self.boundary_signed - self.interior_signed
    }

    /// How much the step reverses one pixel in: ~-1 for a rim, ~0 for a level offset.
    pub fn flank_ratio(&self) -> f64 {
        let b = self.boundary_signed - self.interior_signed;
        if b.abs() < 1e-9 {
            0.0
        } else {
            (self.flank_signed - self.interior_signed) / b
        }
    }

    /// Which fault this looks like. A hint for where to look, not a diagnosis.
    pub fn looks_like(&self) -> &'static str {
        if self.excess_db() < 1.0 && self.level_step().abs() < 0.5 {
            return "no visible grid";
        }
        if self.flank_ratio() < -0.3 {
            "a rim: the step reverses one pixel in — residual-packing contamination or merge rounding"
        } else {
            "level mismatch (one side is consistently brighter) — reuse vs network, not a rim"
        }
    }
}

/// Measure the block grid in an OUTPUT frame.
///
/// `pitch` is the block size in output pixels (`block_size * scale`). Interior lines are
/// sampled at the pitch's midpoint, which is the furthest place from any boundary and
/// therefore the fairest control.
pub fn seam_profile(frame: &Frame, pitch: u32) -> Option<SeamProfile> {
    if pitch < 4 || frame.width < pitch * 2 || frame.height < pitch * 2 {
        return None;
    }
    let half = pitch / 2;
    let (bx_abs, bx_sig, bx_n) = column_steps(frame, (pitch..frame.width).step_by(pitch as usize));
    let (by_abs, by_sig, by_n) = row_steps(frame, (pitch..frame.height).step_by(pitch as usize));
    let (fx_abs, fx_sig, fx_n) = column_steps(frame, (pitch + 1..frame.width).step_by(pitch as usize));
    let (fy_abs, fy_sig, fy_n) = row_steps(frame, (pitch + 1..frame.height).step_by(pitch as usize));
    let _ = (fx_abs, fy_abs);
    let (ix_abs, ix_sig, ix_n) = column_steps(frame, (pitch + half..frame.width).step_by(pitch as usize));
    let (iy_abs, iy_sig, iy_n) = row_steps(frame, (pitch + half..frame.height).step_by(pitch as usize));
    let bn = (bx_n + by_n) as f64;
    let inn = (ix_n + iy_n) as f64;
    if bn == 0.0 || inn == 0.0 {
        return None;
    }
    Some(SeamProfile {
        boundary: (bx_abs + by_abs) / bn,
        interior: (ix_abs + iy_abs) / inn,
        boundary_signed: (bx_sig + by_sig) / bn,
        interior_signed: (ix_sig + iy_sig) / inn,
        flank_signed: {
            let fnn = (fx_n + fy_n) as f64;
            if fnn == 0.0 { 0.0 } else { (fx_sig + fy_sig) / fnn }
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with no seams: deterministic noise, so every column and row has the same
    /// expected step and no place in the picture is special.
    ///
    /// NOT a `(x + y) % 200` ramp, which was the first fixture and a bad one: its wrap
    /// discontinuity falls on some sampled lines and not others, so the control measured a
    /// property of the fixture rather than of the frame. It reported -2.5 dB on a frame with
    /// no seams at all.
    fn gradient(w: u32, h: u32) -> Frame {
        let mut f = Frame::new(w, h);
        let mut state: u32 = 0x9E37_79B9;
        for y in 0..h {
            for x in 0..w {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                // A NARROW band (±8 levels) around mid grey. Wide noise is not a better
                // fixture: with steps of ±27 the signed means of a clean frame wander by
                // half a level from sampling alone, which is the same size as the fault the
                // level test looks for. Narrow noise keeps the control quiet without making
                // the frame flat.
                let v = ((state >> 24) as u8 / 16) + 120;
                f.set_px(x, y, [v, v, v]);
            }
        }
        f
    }

    #[test]
    fn a_frame_with_no_grid_reports_no_grid() {
        let f = gradient(128, 128);
        let p = seam_profile(&f, 32).expect("big enough");
        assert!(p.excess_db().abs() < 1.0, "{p:?} → {} dB", p.excess_db());
        assert!(p.level_step().abs() < 0.5, "{p:?}");
        assert_eq!(p.looks_like(), "no visible grid");
    }

    /// A LEVEL MISMATCH: every block is one grey level brighter than the one to its left.
    /// Tiny in SSE — the gate would allow it — and plainly visible.
    #[test]
    fn a_level_step_at_every_boundary_is_found_and_named() {
        let mut f = gradient(128, 128);
        for y in 0..128 {
            for x in 0..128u32 {
                let bump = (x / 32) as i32; // +1 level per block column
                let p = f.px(x, y);
                let v = (p[0] as i32 + bump).clamp(0, 255) as u8;
                f.set_px(x, y, [v, v, v]);
            }
        }
        let p = seam_profile(&f, 32).expect("big enough");
        // On noisy content the DC step is invisible in mean |step| — twenty levels of noise
        // against one level of offset — and obvious in the signed mean. That is why there
        // are two measures and not one.
        assert!(p.level_step() > 0.5, "a per-block level step was not detected: {p:?}");
        assert!(p.flank_ratio() > -0.3, "a monotone level step must not reverse: {p:?}");
        assert!(
            p.looks_like().starts_with("level mismatch"),
            "misdiagnosed: {} ({p:?})",
            p.looks_like()
        );
    }

    /// A RIM: the boundary pixels ring, one bright and one dark, which is what
    /// residual-packing contamination looks like. The signed mean stays near zero, and that
    /// is what tells it apart from a level step.
    #[test]
    fn a_ringing_rim_is_found_and_told_apart_from_a_level_step() {
        let mut f = gradient(128, 128);
        for y in 0..128u32 {
            for x in 0..128u32 {
                if x % 32 == 0 {
                    let p = f.px(x, y);
                    let v = (p[0] as i32 + 12).clamp(0, 255) as u8;
                    f.set_px(x, y, [v, v, v]);
                } else if x % 32 == 31 {
                    let p = f.px(x, y);
                    let v = (p[0] as i32 - 12).clamp(0, 255) as u8;
                    f.set_px(x, y, [v, v, v]);
                }
            }
        }
        let p = seam_profile(&f, 32).expect("big enough");
        assert!(p.excess_db() > 1.0, "a rim was not detected: {p:?}");
        // A rim does NOT cancel in the signed mean — that premise was wrong, and this test
        // failed on it. Ringing puts a dark pixel one side and a bright one the other, so
        // the boundary step is large and signed just like a level offset. What separates
        // them is that the step REVERSES one pixel in.
        assert!(p.flank_ratio() < -0.3, "the rim did not reverse one pixel in: {p:?} ratio {}", p.flank_ratio());
        assert!(
            p.looks_like().starts_with("a rim"),
            "misdiagnosed: {} ({p:?})",
            p.looks_like()
        );
    }

    #[test]
    fn a_frame_too_small_for_two_blocks_measures_nothing() {
        assert!(seam_profile(&gradient(40, 40), 32).is_none());
        assert!(seam_profile(&gradient(128, 128), 2).is_none(), "a pitch of 2 has no interior");
    }

    /// The receptive radius is what decides whether `bleed` is enough, and for every model
    /// this pipeline ships it is much larger than the default bleed of 1.
    #[test]
    fn the_receptive_radius_comes_off_the_op_list() {
        use crate::net::{Act, BufId, BufSpec, ConvOp};
        // A SRVGG-shaped stack: N same-pad 3×3 convs at level 1, then a deconv to level 2.
        let conv = |src: usize, dst: usize| {
            Op::Conv(ConvOp {
                name: "c".into(),
                src: src,
                src_ch: 3,
                in_scale: 1,
                dst: dst,
                dst_off: 0,
                out_ch: 3,
                pad: 1,
                act: Act::None,
                prelu: vec![],
                residual: None,
                weight: vec![0.0; 3 * 3 * 3 * 3],
                bias: vec![0.0; 3],
            })
        };
        let net = Network {
            name: "t".into(),
            ops: (0..8).map(|_| conv(0, 1)).collect(),
            bufs: vec![BufSpec { channels: 3, level: 1 }, BufSpec { channels: 3, level: 1 }],
            input: 0,
            output: 1,
            scale: 1,
            unshuffle: 1,
            shuffle: 1,
            add_input: false,
            pre_pad: 0,
            exact: false,
        };
        assert_eq!(receptive_radius(&net), 8, "eight 3x3 convs reach eight pixels");

        // Pointwise ops add nothing — a residual add is not a neighbourhood.
        let mut with_adds = net.clone();
        with_adds.ops.push(Op::AddScaled { dst: 1, src: 0, ch: 3, scale: 0.2 });
        with_adds.ops.push(Op::Copy { src: 0, dst: 1, dst_off: 0, ch: 3 });
        assert_eq!(receptive_radius(&with_adds), 8);

        // The check turns the radius into the thing that matters: exposed pixels.
        let c = BleedCheck::new(&net, 1);
        assert_eq!((c.radius, c.bleed, c.exposed), (8, 1, 7));
        // The model's own tile-overlap number is carried alongside, because the tiler and
        // the residual packing answer the same question with different values.
        let mut tiled = net.clone();
        tiled.pre_pad = 10;
        assert!(BleedCheck::new(&tiled, 1).disagrees_with_tiler(), "a bleed under pre_pad went unflagged");
        assert!(!BleedCheck::new(&tiled, 10).disagrees_with_tiler());
        assert!(!c.covered());
        assert!(c.describe().contains("outer 7px"), "{}", c.describe());
        let ok = BleedCheck::new(&net, 8);
        assert!(ok.covered() && ok.describe().contains("covers"));
        // and over-provisioning is not an error
        assert!(BleedCheck::new(&net, 40).covered());
    }
}
