//! The plate: one background image per window of frames, in a shared
//! "world" coordinate system, built as a per-pixel temporal median of the
//! frames placed under their global motion.
//!
//! This is fao-codec's v11 plate (`fao_codec_lib::v11_plate::build_plate`)
//! reduced to what bscale needs from it. There it is the thing that gets
//! *transmitted*; here it is a *reference*: the analysis stage may source a
//! block from the plate the same way it sources one from the previous
//! output or the ring, and the merge stage copies the block from the plate's
//! upscale. What that buys over the existing references:
//!
//! - **Accretion.** A pan reveals background at one edge every frame and the
//!   previous output cannot explain it. The plate can, once any frame in the
//!   window has seen it. Content that was occluded by a passing foreground
//!   and is uncovered again is served from the plate, not re-upscaled —
//!   this is the picket-fence integration the brain does, in its formal
//!   form: samples of the same surface from many frames, stitched by motion.
//! - **One upscale per window.** The plate goes through the network once;
//!   every block it explains is an integer copy in the upscaled domain.
//! - **A denoised source.** The median rejects grain and codec noise that
//!   changes frame to frame — the very thing that lowers reuse on AV1
//!   sources — and rejects a mover that covers a pixel in fewer than half
//!   the frames, so the plate is background by construction.
//!
//! What it deliberately is NOT yet: sub-pixel. Placements are rounded to
//! whole pixels, so a block is served from the plate only when the frame's
//! own phase is close enough to the plate's for the JPEG gate to accept it,
//! exactly the limit `BlockMatch::Moved` already has. The multi-frame
//! super-resolution version (`v11_plate::build_plate_sr`, S=2 grid, real
//! samples at every half-pel phase) is the second step, and at scale 2 a
//! half-pel input placement is an integer placement in the upscaled domain,
//! so the merge stays an exact copy. It is not built here because its value
//! is gated on the measurement this step provides: if integer plates do not
//! move the reuse fraction on real footage, sub-pixel plates will not either.
//!
//! Measured on real footage this is a NET LOSS on handheld and on a 3D game
//! camera; see docs/REUSE_FINDINGS.md §4 before enabling it.
//!
//! Memory: the median is computed in row strips so the per-pixel sample
//! lists never exceed `strip_rows · plate_width · 3 · frames` bytes.

use crate::frame::Frame;
use crate::phase_corr::{GlobalMotion, PhaseCorrelation};

/// Where a frame sits in its plate: `plate = frame + (x, y)`, in pixels,
/// non-negative by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub x: u32,
    pub y: u32,
}

/// A window's plate at input resolution, plus every frame's placement in it.
#[derive(Debug, Clone)]
pub struct Plate {
    pub lo: Frame,
    pub placements: Vec<Placement>,
    /// Per-frame pairwise phase-correlation confidence (frame 0 is 1.0).
    pub confidence: Vec<f32>,
    /// Per-pixel number of frames that observed each plate pixel.
    pub observed: Vec<u16>,
}

/// The plate as one frame sees it: the reference the analysis stage reads.
#[derive(Debug, Clone, Copy)]
pub struct PlateView<'a> {
    pub lo: &'a Frame,
    pub place: Placement,
}

impl<'a> PlateView<'a> {
    /// Plate coordinates of frame position `(x, y)`.
    #[inline]
    pub fn at(&self, x: u32, y: u32) -> (u32, u32) {
        (x + self.place.x, y + self.place.y)
    }
}

/// Cumulative frame-0-relative displacement from pairwise estimates. Below
/// `min_confidence` a pair contributes no motion (hold), so a cut inside a
/// window does not fling later frames off the plate; the caller should split
/// windows at cuts anyway (see `Plate::build`'s `confidence`).
pub fn cumulative_motion(frames: &[&Frame], pc: &PhaseCorrelation, min_confidence: f32) -> (Vec<(f32, f32)>, Vec<f32>) {
    let mut disp = Vec::with_capacity(frames.len());
    let mut conf = Vec::with_capacity(frames.len());
    let (mut ax, mut ay) = (0.0f32, 0.0f32);
    for i in 0..frames.len() {
        let m: GlobalMotion = if i == 0 { GlobalMotion { dx: 0.0, dy: 0.0, confidence: 1.0 } } else { pc.estimate(frames[i], frames[i - 1]) };
        if i > 0 && m.confidence >= min_confidence {
            ax += m.dx;
            ay += m.dy;
        }
        disp.push((ax, ay));
        conf.push(m.confidence);
    }
    (disp, conf)
}

impl Plate {
    /// Build a plate over `frames` (all the same size). `min_confidence`
    /// gates the pairwise motion estimates.
    pub fn build(frames: &[&Frame], min_confidence: f32) -> Plate {
        assert!(!frames.is_empty(), "Plate::build: no frames");
        let (fw, fh) = (frames[0].width, frames[0].height);
        assert!(frames.iter().all(|f| (f.width, f.height) == (fw, fh)), "Plate::build: frame sizes differ");
        let pc = PhaseCorrelation::for_frame(fw, fh);
        let (disp, confidence) = cumulative_motion(frames, &pc, min_confidence);
        Self::build_placed(frames, &disp, confidence)
    }

    /// Build from already-estimated frame-0-relative displacements
    /// (`cur[x] ≈ frame0[x − disp]`). Displacements are rounded to pixels.
    pub fn build_placed(frames: &[&Frame], disp: &[(f32, f32)], confidence: Vec<f32>) -> Plate {
        let (fw, fh) = (frames[0].width, frames[0].height);
        // Frame i's pixel fx shows world coordinate fx − disp_i. Plate
        // coordinates are world coordinates shifted so the union of
        // footprints starts at 0.
        let world: Vec<(i64, i64)> = disp.iter().map(|&(dx, dy)| (-(dx.round() as i64), -(dy.round() as i64))).collect();
        let ox = world.iter().map(|w| w.0).min().unwrap();
        let oy = world.iter().map(|w| w.1).min().unwrap();
        let placements: Vec<Placement> = world.iter().map(|&(wx, wy)| Placement { x: (wx - ox) as u32, y: (wy - oy) as u32 }).collect();
        let pw = placements.iter().map(|p| p.x + fw).max().unwrap();
        let ph = placements.iter().map(|p| p.y + fh).max().unwrap();

        let mut lo = Frame::new(pw, ph);
        let mut observed = vec![0u16; (pw as usize) * (ph as usize)];
        // Median per plate pixel per channel, in strips of rows.
        let n = frames.len();
        let strip_rows: u32 = (((8u64 << 20) / (pw as u64 * 3 * n as u64)).max(1) as u32).min(ph);
        let mut samples: Vec<u8> = Vec::new();
        let mut counts: Vec<u16> = Vec::new();
        let mut y0 = 0u32;
        while y0 < ph {
            let rows = strip_rows.min(ph - y0);
            let cells = (rows as usize) * (pw as usize) * 3;
            samples.clear();
            samples.resize(cells * n, 0);
            counts.clear();
            counts.resize((rows as usize) * (pw as usize), 0);
            for (f, p) in frames.iter().zip(&placements) {
                // Rows of this frame that land in [y0, y0 + rows).
                let fy0 = y0.saturating_sub(p.y).min(fh);
                let fy1 = (y0 + rows).saturating_sub(p.y).min(fh);
                for fy in fy0..fy1 {
                    let py = p.y + fy - y0;
                    let row = f.row(fy, 0, fw);
                    for fx in 0..fw as usize {
                        let cell = (py as usize) * (pw as usize) + p.x as usize + fx;
                        let c = counts[cell] as usize;
                        let base = cell * 3 * n;
                        samples[base + c] = row[fx * 3];
                        samples[base + n + c] = row[fx * 3 + 1];
                        samples[base + 2 * n + c] = row[fx * 3 + 2];
                        counts[cell] += 1;
                    }
                }
            }
            for r in 0..rows {
                for x in 0..pw {
                    let cell = (r as usize) * (pw as usize) + x as usize;
                    let c = counts[cell] as usize;
                    if c == 0 {
                        continue;
                    }
                    let base = cell * 3 * n;
                    let mut px = [0u8; 3];
                    for ch in 0..3 {
                        let s = &mut samples[base + ch * n..base + ch * n + c];
                        // Upper median: for even counts the larger of the two
                        // middle samples, so a 2-frame window is one frame's
                        // real pixel rather than a blend.
                        let (_, m, _) = s.select_nth_unstable(c / 2);
                        px[ch] = *m;
                    }
                    lo.set_px(x, y0 + r, px);
                    observed[((y0 + r) as usize) * (pw as usize) + x as usize] = c as u16;
                }
            }
            y0 += rows;
        }
        Plate { lo, placements, confidence, observed }
    }

    pub fn view(&self, frame_index: usize) -> PlateView<'_> {
        PlateView { lo: &self.lo, place: self.placements[frame_index] }
    }

    /// Plate pixels as a multiple of one frame's pixels: the cost of
    /// upscaling the plate once, in frame units, before amortising over the
    /// window.
    pub fn area_in_frames(&self, fw: u32, fh: u32) -> f64 {
        (self.lo.width as f64 * self.lo.height as f64) / (fw as f64 * fh as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testimg::{shifted, smooth};

    /// Sixteen frames panning across a larger world; the plate must be the
    /// world, and every frame's placement must read back exactly.
    #[test]
    fn pan_reconstructs_world_and_placements_are_exact() {
        let world = smooth(200, 120, 4);
        let (fw, fh) = (128u32, 96u32);
        let mut frames = Vec::new();
        let mut want = Vec::new();
        for i in 0..16u32 {
            // Camera pans right and slightly down: frame i shows world at (3i, i).
            let (wx, wy) = (3 * i, i);
            frames.push(world.crop(wx, wy, fw, fh));
            want.push((wx, wy));
        }
        let refs: Vec<&Frame> = frames.iter().collect();
        let plate = Plate::build(&refs, 0.2);
        assert_eq!(plate.placements[0], Placement { x: 0, y: 0 });
        for (i, &(wx, wy)) in want.iter().enumerate() {
            assert_eq!(plate.placements[i], Placement { x: wx, y: wy }, "frame {i}: {:?}", plate.confidence[i]);
        }
        assert_eq!((plate.lo.width, plate.lo.height), (3 * 15 + fw, 15 + fh));
        // Every frame reads back bit-exactly from the plate (no movers, no noise).
        for (i, f) in frames.iter().enumerate() {
            let v = plate.view(i);
            for y in 0..fh {
                let (px, py) = v.at(0, y);
                assert_eq!(v.lo.row(py, px, px + fw), f.row(y, 0, fw), "frame {i} row {y}");
            }
        }
    }

    /// A mover covering a pixel in fewer than half the frames leaves no
    /// trace. Placements are given, not estimated: `smooth` is nearly
    /// periodic, which is the one thing phase correlation cannot register
    /// (every multiple of the period is a peak), so a coherent 8 px mover
    /// wins the estimate on that background where it would not on footage.
    #[test]
    fn median_rejects_a_minority_mover() {
        let bg = smooth(96, 64, 9);
        let mut frames = Vec::new();
        for i in 0..9u32 {
            let mut f = bg.clone();
            if i < 4 {
                // A dark square sweeping across.
                for y in 20..36 {
                    for x in (10 + 8 * i)..(26 + 8 * i) {
                        f.set_px(x, y, [5, 5, 5]);
                    }
                }
            }
            frames.push(f);
        }
        let refs: Vec<&Frame> = frames.iter().collect();
        let plate = Plate::build_placed(&refs, &vec![(0.0, 0.0); 9], vec![1.0; 9]);
        assert_eq!((plate.lo.width, plate.lo.height), (96, 64));
        assert_eq!(plate.lo, bg);
        assert!(plate.observed.iter().all(|&c| c == 9));
    }

    #[test]
    fn placed_build_matches_shifted_convention() {
        // shifted(prev, dx, 0): next[x] = prev[x - dx] — content moved right.
        let f0 = smooth(96, 64, 2);
        let f1 = shifted(&f0, 5, 0);
        let plate = Plate::build_placed(&[&f0, &f1], &[(0.0, 0.0), (5.0, 0.0)], vec![1.0, 1.0]);
        // Frame 1 shows world x − 5, so frame 0 sits 5 to the right of frame 1.
        assert_eq!(plate.placements, vec![Placement { x: 5, y: 0 }, Placement { x: 0, y: 0 }]);
        assert_eq!(plate.lo.width, 101);
    }
}
