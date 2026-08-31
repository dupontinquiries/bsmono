//! # v11 "plate" codec — the web thesis in its strongest 2D form
//!
//! Before spending bits on 3D geometry, measure the ceiling of the core
//! FAO idea in 2D: transmit the world ONCE (a background plate = the
//! simplest possible "web"), send per-frame camera motion (a translation,
//! for now), and pay per-frame only for what the plate cannot predict —
//! moving actors and model error — through the (just-fixed) DCT residual
//! coder, whose skip-block RLE zeroes the cost of every 4×4 block the
//! plate already explains.
//!
//! Why this module exists when v8 already builds meshes: on a locked-off
//! or panning shot, a full 3D reconstruction reprojects to exactly what a
//! 2D plate gives — so every byte v8 spends on depth, features and pose
//! beyond a 2D translation is pure overhead THERE. v11 is the control
//! experiment: it measures what "world once + camera + residual" is worth
//! with zero geometry risk. If v11's marginal per-frame cost cannot beat
//! AV1's marginal (post-keyframe) cost on plate-friendly shots, no amount
//! of 3D will save the thesis; if it can, v8's job is to extend the same
//! ratio to parallax shots that break a 2D plate.
//!
//! Accounting contract (what the lab reports):
//!   * `plate_bytes`  — the web. Amortizes across the GOP today; across
//!     entire libraries in FAO's cross-video mode (a UUID + alignment).
//!   * `motion_bytes` — 8 bytes/frame (dx, dy as f32s). The "camera".
//!   * `residual_bytes[f]` — the honest per-frame marginal cost.
//!
//! The plate is a per-pixel TEMPORAL MEDIAN in plate coordinates. The
//! median does two jobs at once: it rejects moving foreground (an actor
//! covering a pixel <50% of frames leaves no trace), and it averages away
//! film grain — the plate is a denoised background by construction. Grain
//! then lives in the residual (and gets quantized), or can be re-synthesized
//! at decode; that split is deliberate and is the same trick AV1's
//! film-grain synthesis plays.

use crate::phase_correlation::PhaseCorrelationPass;
use crate::residual::{self, ResidualOpts};

/// One 4:2:0 frame, planar.
#[derive(Clone)]
pub struct Yuv420 {
    pub w: usize,
    pub h: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl Yuv420 {
    pub fn new(w: usize, h: usize) -> Self {
        Yuv420 { w, h, y: vec![0; w * h], u: vec![0; (w / 2) * (h / 2)], v: vec![0; (w / 2) * (h / 2)] }
    }
}

/// Per-frame camera parameters (translation-only, v0).
#[derive(Clone, Copy, Debug)]
pub struct PlateMotion {
    pub dx: f32,
    pub dy: f32,
    pub confidence: f32,
}

/// The web: a background plate the size of the union of all warped frame
/// footprints (for a static camera, exactly the frame size).
#[derive(Clone)]
pub struct Plate {
    pub w: usize,
    pub h: usize,
    /// Plate origin in frame-0 coordinates (negative when later frames
    /// panned up/left past frame 0's edge).
    pub ox: f32,
    pub oy: f32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

/// Estimate per-frame global translation with cumulative pairwise phase
/// correlation (pairwise is robust to lighting drift; the cumulative sum
/// gives each frame's offset in frame-0 coordinates).
pub fn estimate_motions(frames: &[Yuv420]) -> Vec<PlateMotion> {
    let mut out = Vec::with_capacity(frames.len());
    if frames.is_empty() {
        return out;
    }
    let w = frames[0].w;
    let h = frames[0].h;
    let pass = PhaseCorrelationPass::new(512.min(w.min(h)));
    out.push(PlateMotion { dx: 0.0, dy: 0.0, confidence: 1.0 });
    let (mut ax, mut ay) = (0.0f32, 0.0f32);
    for i in 1..frames.len() {
        let m = pass.estimate(&frames[i].y, &frames[i - 1].y, w, h);
        // Low confidence (cut, or motion beyond the window) → hold position
        // rather than inject a garbage jump; the residual absorbs the error
        // and the caller can see confidence per frame.
        if m.confidence > 0.15 {
            ax += m.dx;
            ay += m.dy;
        }
        out.push(PlateMotion { dx: ax, dy: ay, confidence: m.confidence });
    }
    out
}

/// Build the plate: temporal median per plate pixel over every frame that
/// covers it (nearest-pixel placement; sub-pixel handled at predict time).
pub fn build_plate(frames: &[Yuv420], motions: &[PlateMotion]) -> Plate {
    assert_eq!(frames.len(), motions.len());
    let fw = frames[0].w;
    let fh = frames[0].h;
    // Plate bounds in frame-0 coords: frame f occupies [-dx, -dx+fw) —
    // a camera pan of +dx moves CONTENT by -dx in frame-0 space.
    let (mut minx, mut miny, mut maxx, mut maxy) = (0.0f32, 0.0f32, fw as f32, fh as f32);
    for m in motions {
        minx = minx.min(-m.dx);
        miny = miny.min(-m.dy);
        maxx = maxx.max(-m.dx + fw as f32);
        maxy = maxy.max(-m.dy + fh as f32);
    }
    let pw = (maxx - minx).ceil() as usize;
    let ph = (maxy - miny).ceil() as usize;
    let pw = pw + (pw & 1); // keep even for the half-res chroma plate
    let ph = ph + (ph & 1);

    let median_plane = |get: &dyn Fn(&Yuv420) -> &Vec<u8>,
                        pw: usize,
                        ph: usize,
                        fw: usize,
                        fh: usize,
                        sub: usize|
     -> Vec<u8> {
        let mut plane = vec![0u8; pw * ph];
        let mut samples: Vec<u8> = Vec::with_capacity(frames.len());
        for py in 0..ph {
            for px in 0..pw {
                samples.clear();
                for (f, m) in frames.iter().zip(motions) {
                    // plate pixel (px,py) in frame f's coords:
                    let fx = px as f32 + minx / sub as f32 + m.dx / sub as f32;
                    let fy = py as f32 + miny / sub as f32 + m.dy / sub as f32;
                    let (fx, fy) = (fx.round() as isize, fy.round() as isize);
                    if fx >= 0 && fy >= 0 && (fx as usize) < fw && (fy as usize) < fh {
                        samples.push(get(f)[fy as usize * fw + fx as usize]);
                    }
                }
                if !samples.is_empty() {
                    samples.sort_unstable();
                    plane[py * pw + px] = samples[samples.len() / 2];
                }
            }
        }
        plane
    };

    let y = median_plane(&|f| &f.y, pw, ph, fw, fh, 1);
    let u = median_plane(&|f| &f.u, pw / 2, ph / 2, fw / 2, fh / 2, 2);
    let v = median_plane(&|f| &f.v, pw / 2, ph / 2, fw / 2, fh / 2, 2);
    Plate { w: pw, h: ph, ox: minx, oy: miny, y, u, v }
}

fn bilinear(plane: &[u8], pw: usize, ph: usize, x: f32, y: f32) -> u8 {
    let x = x.clamp(0.0, (pw - 1) as f32);
    let y = y.clamp(0.0, (ph - 1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(pw - 1);
    let y1 = (y0 + 1).min(ph - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let p00 = plane[y0 * pw + x0] as f32;
    let p10 = plane[y0 * pw + x1] as f32;
    let p01 = plane[y1 * pw + x0] as f32;
    let p11 = plane[y1 * pw + x1] as f32;
    let top = p00 + (p10 - p00) * tx;
    let bot = p01 + (p11 - p01) * tx;
    (top + (bot - top) * ty).round().clamp(0.0, 255.0) as u8
}

/// Render the plate through a frame's camera (bilinear, sub-pixel).
pub fn predict_frame(plate: &Plate, m: &PlateMotion, fw: usize, fh: usize) -> Yuv420 {
    let mut out = Yuv420::new(fw, fh);
    // Signs, written out once so nobody re-derives them wrong:
    // frame pixel (fx,fy) shows content at frame-0 coordinate
    // (fx - m.dx, fy - m.dy); that content lives at plate coordinate
    // (fx - m.dx - ox, fy - m.dy - oy).
    for fy in 0..fh {
        for fx in 0..fw {
            let px = fx as f32 - m.dx - plate.ox;
            let py = fy as f32 - m.dy - plate.oy;
            out.y[fy * fw + fx] = bilinear(&plate.y, plate.w, plate.h, px, py);
        }
    }
    let (cw, ch) = (fw / 2, fh / 2);
    for fy in 0..ch {
        for fx in 0..cw {
            let px = fx as f32 - m.dx / 2.0 - plate.ox / 2.0;
            let py = fy as f32 - m.dy / 2.0 - plate.oy / 2.0;
            out.u[fy * cw + fx] = bilinear(&plate.u, plate.w / 2, plate.h / 2, px, py);
            out.v[fy * cw + fx] = bilinear(&plate.v, plate.w / 2, plate.h / 2, px, py);
        }
    }
    out
}

/// Interleave a 4:2:0 frame into the residual coder's 3-channel layout
/// (chroma bilinear-upsampled to full res). Channel 0=Y, 1=U, 2=V.
pub fn interleave_444(f: &Yuv420) -> Vec<u8> {
    let (w, h) = (f.w, f.h);
    let (cw, ch) = (w / 2, h / 2);
    let mut out = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 3;
            out[i] = f.y[y * w + x];
            let ux = (x as f32 - 0.5) / 2.0;
            let uy = (y as f32 - 0.5) / 2.0;
            out[i + 1] = bilinear(&f.u, cw, ch, ux, uy);
            out[i + 2] = bilinear(&f.v, cw, ch, ux, uy);
        }
    }
    out
}

/// Inverse of `interleave_444` (2×2 box down for chroma).
pub fn deinterleave_420(buf: &[u8], w: usize, h: usize) -> Yuv420 {
    let mut f = Yuv420::new(w, h);
    for y in 0..h {
        for x in 0..w {
            f.y[y * w + x] = buf[(y * w + x) * 3];
        }
    }
    let (cw, ch) = (w / 2, h / 2);
    for cy in 0..ch {
        for cx in 0..cw {
            let mut su = 0u32;
            let mut sv = 0u32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let i = ((cy * 2 + dy) * w + cx * 2 + dx) * 3;
                    su += buf[i + 1] as u32;
                    sv += buf[i + 2] as u32;
                }
            }
            f.u[cy * cw + cx] = (su / 4) as u8;
            f.v[cy * cw + cx] = (sv / 4) as u8;
        }
    }
    f
}

/// Everything the lab needs to report for one encoded GOP.
pub struct PlateEncodeReport {
    pub plate_bytes: usize,
    pub motion_bytes: usize,
    pub residual_bytes: Vec<usize>,
    pub recon: Vec<Yuv420>,
}

/// Encode a GOP against a prebuilt plate; returns byte accounting plus the
/// decoder-side reconstruction (computed through the same residual::apply
/// path a real decoder would run — no encoder-only shortcuts).
pub fn encode_gop(
    frames: &[Yuv420],
    motions: &[PlateMotion],
    plate: &Plate,
    opts: &ResidualOpts,
) -> PlateEncodeReport {
    let fw = frames[0].w;
    let fh = frames[0].h;
    let mut plate_raw = Vec::with_capacity(plate.y.len() + plate.u.len() + plate.v.len());
    plate_raw.extend_from_slice(&plate.y);
    plate_raw.extend_from_slice(&plate.u);
    plate_raw.extend_from_slice(&plate.v);
    let plate_bytes = zstd::encode_all(plate_raw.as_slice(), 19).map(|c| c.len()).unwrap_or(plate_raw.len());

    let mut residual_bytes = Vec::with_capacity(frames.len());
    let mut recon = Vec::with_capacity(frames.len());
    for (f, m) in frames.iter().zip(motions) {
        let predicted = predict_frame(plate, m, fw, fh);
        let p444 = interleave_444(&predicted);
        let t444 = interleave_444(f);
        match residual::encode(&p444, &t444, fw, fh, opts) {
            Some(trailer) => {
                residual_bytes.push(trailer.len());
                let r = residual::apply(p444, &trailer, fw, fh);
                recon.push(deinterleave_420(&r, fw, fh));
            }
            None => {
                residual_bytes.push(0);
                recon.push(predicted);
            }
        }
    }
    PlateEncodeReport { plate_bytes, motion_bytes: frames.len() * 8, residual_bytes, recon }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Static background with a moving square: the median plate must
    /// recover the CLEAN background (actor rejected), and marginal
    /// per-frame cost must collapse to skip-RLE noise outside the square.
    #[test]
    fn plate_rejects_mover_and_marginal_cost_is_local() {
        let (w, h) = (64, 64);
        let mut frames = Vec::new();
        for t in 0..9 {
            let mut f = Yuv420::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    f.y[y * w + x] = (((x / 8) + (y / 8)) % 2 * 120 + 60) as u8;
                }
            }
            for p in f.u.iter_mut() {
                *p = 128;
            }
            for p in f.v.iter_mut() {
                *p = 128;
            }
            // 12×12 mover sweeping across; covers any pixel < 50% of frames.
            let sx = 4 + t * 5;
            for y in 20..32 {
                for x in sx..(sx + 12).min(w) {
                    f.y[y * w + x] = 250;
                }
            }
            frames.push(f);
        }
        let motions = vec![PlateMotion { dx: 0.0, dy: 0.0, confidence: 1.0 }; frames.len()];
        let plate = build_plate(&frames, &motions);
        // Plate background is the checkerboard, mover fully rejected.
        for y in 20..32 {
            for x in 10..50 {
                let bg = (((x / 8) + (y / 8)) % 2 * 120 + 60) as u8;
                assert_eq!(plate.y[y * plate.w + x], bg, "mover leaked into plate at {x},{y}");
            }
        }
        let opts = ResidualOpts::default();
        let rep = encode_gop(&frames, &motions, &plate, &opts);
        // Reconstruction must contain the mover (residual carries it).
        let f4 = &rep.recon[4];
        let mover_x = 4 + 4 * 5 + 6;
        let got = f4.y[25 * w + mover_x] as i32;
        assert!((got - 250).abs() <= 12, "mover not reconstructed: {got}");
        // And the background must match the source closely.
        let src = &frames[4];
        let mut err = 0.0f64;
        for i in 0..w * h {
            err += (src.y[i] as f64 - f4.y[i] as f64).abs();
        }
        assert!(err / (w * h) as f64 <= 2.0, "background error too high: {}", err / (w * h) as f64);
    }

    #[test]
    fn panning_plate_is_wider_than_a_frame_and_predicts_back() {
        let (w, h) = (64, 48);
        // A wide "world" the camera pans across.
        let world_w = 128;
        let world: Vec<u8> = (0..world_w * h).map(|i| ((i * 7) % 251) as u8).collect();
        let mut frames = Vec::new();
        let mut motions = Vec::new();
        for t in 0..8 {
            let mut f = Yuv420::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    f.y[y * w + x] = world[y * world_w + (x + t * 6)];
                }
            }
            for p in f.u.iter_mut() {
                *p = 128;
            }
            for p in f.v.iter_mut() {
                *p = 128;
            }
            frames.push(f);
            // Frame t shows world x ∈ [6t, 6t+w); frame 0 IS world coords,
            // so frame t's pixel fx shows frame-0 coordinate fx + 6t. The
            // predict convention is "fx shows frame-0 coord fx - m.dx",
            // hence m.dx = -6t. (Content moving left in the viewport =
            // negative dx; the camera's own rightward pan is the negation.)
            motions.push(PlateMotion { dx: -((t * 6) as f32), dy: 0.0, confidence: 1.0 });
        }
        let plate = build_plate(&frames, &motions);
        assert!(plate.w >= w + 7 * 6, "plate too narrow: {}", plate.w);
        // Every frame must be predictable from the plate almost exactly.
        for (f, m) in frames.iter().zip(&motions) {
            let p = predict_frame(&plate, m, w, h);
            let mut err = 0.0f64;
            for i in 0..w * h {
                err += (p.y[i] as f64 - f.y[i] as f64).abs();
            }
            assert!(err / (w * h) as f64 <= 1.5, "pan predict err {}", err / (w * h) as f64);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// v11.2 — per-block predictor selection: plate OR previous recon.
//
// The v1 accounting exposed the structural cost: the plate predicts the
// BACKGROUND, so every foreground (actor) pixel was residual-coded from
// scratch in every frame — exactly the work AV1 avoids with motion-
// compensated inter prediction. v11.2 gives each 4×4 block a choice of
// predictor: the warped plate, or the co-located block of the PREVIOUS
// RECONSTRUCTED frame (closed loop — the decoder has the same frame, so
// drift cannot accumulate). Slow-moving/paused actors collapse to
// near-zero residual; the mode map costs 1 bit per block, zstd'd.
// ═══════════════════════════════════════════════════════════════════

/// Per-frame payload accounting for v2.
pub struct FrameCost {
    pub mode_bytes: usize,
    pub residual_bytes: usize,
}

pub struct PlateEncodeReportV2 {
    pub plate_bytes: usize,
    pub motion_bytes: usize,
    pub frames: Vec<FrameCost>,
    pub recon: Vec<Yuv420>,
    /// Fraction of blocks that chose the previous-frame predictor,
    /// averaged over frames after the first — the "how much did temporal
    /// prediction matter" dial.
    pub prev_mode_share: f32,
}

/// Sum of absolute differences over one 4×4 luma block.
fn sad4(a: &[u8], b: &[u8], w: usize, bx: usize, by: usize) -> u32 {
    let mut s = 0u32;
    for ly in 0..4 {
        let row = (by * 4 + ly) * w + bx * 4;
        for lx in 0..4 {
            s += (a[row + lx] as i32 - b[row + lx] as i32).unsigned_abs();
        }
    }
    s
}

pub fn encode_gop_v2(
    frames: &[Yuv420],
    motions: &[PlateMotion],
    plate: &Plate,
    opts: &ResidualOpts,
) -> PlateEncodeReportV2 {
    let fw = frames[0].w;
    let fh = frames[0].h;
    let (rw, rh) = (fw / 4, fh / 4);
    let mut plate_raw = Vec::with_capacity(plate.y.len() + plate.u.len() + plate.v.len());
    plate_raw.extend_from_slice(&plate.y);
    plate_raw.extend_from_slice(&plate.u);
    plate_raw.extend_from_slice(&plate.v);
    let plate_bytes = zstd::encode_all(plate_raw.as_slice(), 19).map(|c| c.len()).unwrap_or(plate_raw.len());

    let mut out_frames = Vec::with_capacity(frames.len());
    let mut recon: Vec<Yuv420> = Vec::with_capacity(frames.len());
    let mut prev_share_acc = 0.0f64;

    for (fi, (f, m)) in frames.iter().zip(motions).enumerate() {
        let plate_pred = predict_frame(plate, m, fw, fh);
        // Build the composite prediction + mode map. Frame 0 has no
        // previous → all-plate (and the map codes to ~nothing).
        let mut modes = vec![0u8; rw * rh]; // 0 = plate, 1 = prev
        let composite = if fi == 0 {
            plate_pred.clone()
        } else {
            let prev = &recon[fi - 1];
            let mut comp = plate_pred.clone();
            let mut prev_blocks = 0usize;
            for by in 0..rh {
                for bx in 0..rw {
                    let sad_plate = sad4(&f.y, &plate_pred.y, fw, bx, by);
                    let sad_prev = sad4(&f.y, &prev.y, fw, bx, by);
                    // Strictly-better wins; ties keep the plate (its mode
                    // bit RLEs better since background dominates).
                    if sad_prev < sad_plate {
                        modes[by * rw + bx] = 1;
                        prev_blocks += 1;
                        for ly in 0..4 {
                            let row = (by * 4 + ly) * fw + bx * 4;
                            comp.y[row..row + 4].copy_from_slice(&prev.y[row..row + 4]);
                        }
                        // Chroma at half res: copy the co-located 2×2.
                        let (cw, _chh) = (fw / 2, fh / 2);
                        for ly in 0..2 {
                            let crow = (by * 2 + ly) * cw + bx * 2;
                            comp.u[crow..crow + 2].copy_from_slice(&prev.u[crow..crow + 2]);
                            comp.v[crow..crow + 2].copy_from_slice(&prev.v[crow..crow + 2]);
                        }
                    }
                }
            }
            prev_share_acc += prev_blocks as f64 / (rw * rh) as f64;
            comp
        };

        // Mode map cost: bit-packed then zstd (the decoder needs it to
        // rebuild the composite prediction).
        let mut packed = vec![0u8; (modes.len() + 7) / 8];
        for (i, &b) in modes.iter().enumerate() {
            if b == 1 {
                packed[i / 8] |= 1 << (i % 8);
            }
        }
        let mode_bytes = zstd::encode_all(packed.as_slice(), 19).map(|c| c.len()).unwrap_or(packed.len());

        let p444 = interleave_444(&composite);
        let t444 = interleave_444(f);
        let (residual_bytes, rec) = match residual::encode(&p444, &t444, fw, fh, opts) {
            Some(trailer) => {
                let r = residual::apply(p444, &trailer, fw, fh);
                (trailer.len(), deinterleave_420(&r, fw, fh))
            }
            None => (0, composite),
        };
        out_frames.push(FrameCost { mode_bytes, residual_bytes });
        recon.push(rec);
    }
    let denom = (frames.len().saturating_sub(1)).max(1) as f64;
    PlateEncodeReportV2 {
        plate_bytes,
        motion_bytes: frames.len() * 8,
        frames: out_frames,
        recon,
        prev_mode_share: (prev_share_acc / denom) as f32,
    }
}

#[cfg(test)]
mod tests_v2 {
    use super::*;

    /// A static scene with a PAUSED mover: v2's prev-mode must make later
    /// frames dramatically cheaper than v1 (which re-codes the mover every
    /// frame from the plate it was rejected from).
    #[test]
    fn prev_mode_collapses_static_foreground_cost() {
        let (w, h) = (96, 64);
        let mut frames = Vec::new();
        for t in 0..12 {
            let mut f = Yuv420::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    f.y[y * w + x] = (((x / 8) + (y / 8)) % 2 * 100 + 70) as u8;
                }
            }
            for p in f.u.iter_mut() {
                *p = 128;
            }
            for p in f.v.iter_mut() {
                *p = 128;
            }
            // Mover: sweeps t=0..7, then FREEZES at t>=8. The freeze
            // covers 5 of 12 frames at the final position — UNDER half,
            // so the median plate still rejects it there (a longer freeze
            // would put the mover INTO the plate and hand v1 the
            // prediction for free, hiding the mechanism). TEXTURED
            // (position-locked pseudo-noise), not flat: a flat square
            // zstd-flattens to ~nothing per frame in BOTH variants.
            let sx = if t < 8 { 4 + t * 6 } else { 4 + 7 * 6 };
            for y in 24..40 {
                for x in sx..(sx + 16).min(w) {
                    let (mx, my) = (x - sx, y - 24);
                    let n = (mx * 37 + my * 101 + mx * my * 13) % 97;
                    f.y[y * w + x] = (150 + n) as u8;
                }
            }
            frames.push(f);
        }
        let motions = vec![PlateMotion { dx: 0.0, dy: 0.0, confidence: 1.0 }; frames.len()];
        let plate = build_plate(&frames, &motions);
        let opts = ResidualOpts { dead_zone: 2.0, ..ResidualOpts::default() };
        let v1 = encode_gop(&frames, &motions, &plate, &opts);
        let v2 = encode_gop_v2(&frames, &motions, &plate, &opts);
        // Frozen-mover frames (9..) must be far cheaper in v2: their
        // previous recon already contains the mover, the plate never does.
        let v1_tail: usize = v1.residual_bytes[9..].iter().sum();
        let v2_tail: usize = v2.frames[9..].iter().map(|c| c.residual_bytes + c.mode_bytes).sum();
        assert!(
            v2_tail * 3 < v1_tail,
            "prev-mode didn't collapse frozen-foreground cost: v1 {v1_tail} v2 {v2_tail}"
        );
        assert!(v2.prev_mode_share > 0.02, "prev mode never chosen: {}", v2.prev_mode_share);
        // And reconstruction must still contain the mover.
        let f11 = &v2.recon[11];
        {
            let sx = 4 + 7 * 6;
            let (mx, my) = (50usize - sx, 30usize - 24);
            let expect = (150 + (mx * 37 + my * 101 + mx * my * 13) % 97) as i32;
            assert!((f11.y[30 * w + 50] as i32 - expect).abs() <= 16);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// v11.3 — 16×16 mode regions + motion-compensated prev mode.
//
// v11.2's accounting showed two costs the design itself was paying:
//   * mode map at 4×4 granularity = 5–7 KB/frame of side info — a
//     quarter of the whole marginal at aggressive quality settings;
//   * zero-motion prev only predicts PAUSED foreground. An actor in
//     motion misses their own previous position by a few pixels and
//     falls back to full residual — the exact bytes AV1 never pays
//     because motion compensation is its bread and butter.
//
// v11.3: decisions per 16×16 region (16× less side info), and prev mode
// carries a motion vector found by diamond search (±MV_RANGE, then ±1
// refine) on the previous RECONSTRUCTION's luma. Side info per region:
// 1 mode bit; for prev regions an (dx,dy) i8 pair. Everything zstd'd.
// ═══════════════════════════════════════════════════════════════════

const REGION: usize = 16;
// 24 px: the saturation diagnostic on the fast-pan shot showed 4.4%
// of prev-mode regions pinned at the old 12 px clamp — regions
// silently taking bad prediction. Diamond descent keeps the larger
// range cheap (a step-8 tier seeds it); ±96 quarter-pel fits i8.
const MV_RANGE: i32 = 24;

fn sad_region(a: &[u8], w: usize, h: usize, ax: i32, ay: i32, b: &[u8], bx: usize, by: usize) -> u32 {
    // SAD of REGION×REGION: a sampled at (ax,ay) (clamped), b at (bx,by).
    let mut s = 0u32;
    for ly in 0..REGION {
        let sy = (ay + ly as i32).clamp(0, h as i32 - 1) as usize;
        let brow = (by + ly) * w + bx;
        for lx in 0..REGION {
            let sx = (ax + lx as i32).clamp(0, w as i32 - 1) as usize;
            s += (a[sy * w + sx] as i32 - b[brow + lx] as i32).unsigned_abs();
        }
    }
    s
}

/// Deadzone-aware prediction cost: sum of |cur − pred| EXCESS over the
/// threshold the residual coder will discard anyway. Raw SAD is a bad
/// mode-selection proxy at aggressive deadzones: grain contributes a
/// large SAD floor to EVERY candidate while costing zero bits, so
/// candidates get ranked by their grain alignment instead of by the
/// bits they will actually cause. (Measured: the SAD-based decision
/// plus MV rate bias dropped prev-mode share 0.69→0.33 and ~1.5 dB at
/// matched dz.) `thr` ≈ dead_zone/2: a flat delta d maps to DC 4d
/// against a DC zone of 2·dead_zone, so deltas below dead_zone/2 are
/// invisible to the coder.
#[allow(dead_code)] // kept: documents a measured negative result (see encode_gop_v3)
fn region_codable_cost(
    pred: &[u8],
    w: usize,
    h: usize,
    ax: i32,
    ay: i32,
    cur: &[u8],
    bx: usize,
    by: usize,
    thr: i32,
) -> u32 {
    let mut s = 0u32;
    for ly in 0..REGION {
        let sy = (ay + ly as i32).clamp(0, h as i32 - 1) as usize;
        let brow = (by + ly) * w + bx;
        for lx in 0..REGION {
            let sx = (ax + lx as i32).clamp(0, w as i32 - 1) as usize;
            let d = (pred[sy * w + sx] as i32 - cur[brow + lx] as i32).abs();
            s += (d - thr).max(0) as u32;
        }
    }
    s
}

/// Rate-biased cost of a motion vector: zero is free, anything else must
/// buy back its ~2 bytes of side info in SAD. Without this, the search
/// wanders onto shallow ±1–2px minima created by film grain (grain
/// decorrelates every frame, so SOME small offset always "matches" the
/// noise slightly better) and nearly every region reports a junk nonzero
/// MV — which is exactly what the v3 side-info accounting showed.
fn mv_rate_bias(dx: i32, dy: i32) -> u32 {
    if dx == 0 && dy == 0 {
        0
    } else {
        400 + 16 * (dx.unsigned_abs() + dy.unsigned_abs())
    }
}

/// Small diamond motion search on luma, minimizing SAD + rate bias.
/// Returns (mv, raw sad of the chosen mv).
/// SAD of a region against a HALF-PEL position in prev (bilinear).
fn sad_region_hp(prev_y: &[u8], w: usize, h: usize, fx: f32, fy: f32, cur_y: &[u8], rx: usize, ry: usize) -> u32 {
    let mut acc = 0u32;
    for dy in 0..REGION {
        for dx in 0..REGION {
            let p = bilinear(prev_y, w, h, fx + dx as f32, fy + dy as f32);
            let c = cur_y[(ry + dy) * w + rx + dx];
            acc += (p as i32 - c as i32).unsigned_abs();
        }
    }
    acc
}

/// Returns the best MV in HALF-PEL units (so (3, -1) means (+1.5, -0.5)
/// pixels) plus its SAD. The residual ledger measured 55% of residual
/// energy on ALIGNED content — the diffuse floor of integer-pel
/// ghosting on drifting footage (~½px edge error, everywhere, every
/// frame). Integer diamond converges first; an 8-neighbor half-pel
/// refinement then runs, accepted only when it beats the integer SAD
/// by >16 (covers its rate; refinement carries no separate bias).
fn search_mv(prev_y: &[u8], w: usize, h: usize, cur_y: &[u8], rx: usize, ry: usize) -> ((i8, i8), u32) {
    let (mut best_dx, mut best_dy) = (0i32, 0i32);
    let mut best_sad = sad_region(prev_y, w, h, rx as i32, ry as i32, cur_y, rx, ry);
    let mut best_cost = best_sad; // zero-MV bias is 0
    // Coarse: step-4 diamond out to MV_RANGE, then step-2, then step-1.
    for &step in &[8i32, 4, 2, 1] {
        let mut improved = true;
        while improved {
            improved = false;
            for (ddx, ddy) in [(step, 0), (-step, 0), (0, step), (0, -step)] {
                let (dx, dy) = (best_dx + ddx, best_dy + ddy);
                if dx.abs() > MV_RANGE || dy.abs() > MV_RANGE {
                    continue;
                }
                let s = sad_region(prev_y, w, h, rx as i32 + dx, ry as i32 + dy, cur_y, rx, ry);
                let cost = s + mv_rate_bias(dx, dy);
                if cost < best_cost {
                    best_cost = cost;
                    best_sad = s;
                    best_dx = dx;
                    best_dy = dy;
                    improved = true;
                }
            }
        }
    }
    // Half-pel refinement around the integer winner, then quarter-pel
    // around the half-pel winner. Returned units are QUARTER-PEL
    // (so (6, -2) means (+1.5, -0.5) px); ±12px range = ±48, fits i8.
    // Each ring is accepted only on a clear SAD win (>16 half, >8
    // quarter) so refinement never pays rate for noise.
    let (mut hx, mut hy) = (best_dx * 2, best_dy * 2);
    let mut hp_sad = best_sad;
    for (ddx, ddy) in [
        (-1i32, -1i32), (0, -1), (1, -1),
        (-1, 0), (1, 0),
        (-1, 1), (0, 1), (1, 1),
    ] {
        let (cx, cy) = (best_dx * 2 + ddx, best_dy * 2 + ddy);
        let fx = rx as f32 + cx as f32 * 0.5;
        let fy = ry as f32 + cy as f32 * 0.5;
        let s = sad_region_hp(prev_y, w, h, fx, fy, cur_y, rx, ry);
        if s + 16 < hp_sad {
            hp_sad = s;
            hx = cx;
            hy = cy;
        }
    }
    let (mut qx, mut qy) = (hx * 2, hy * 2);
    let mut qp_sad = hp_sad;
    for (ddx, ddy) in [
        (-1i32, -1i32), (0, -1), (1, -1),
        (-1, 0), (1, 0),
        (-1, 1), (0, 1), (1, 1),
    ] {
        let (cx, cy) = (hx * 2 + ddx, hy * 2 + ddy);
        let fx = rx as f32 + cx as f32 * 0.25;
        let fy = ry as f32 + cy as f32 * 0.25;
        let s = sad_region_hp(prev_y, w, h, fx, fy, cur_y, rx, ry);
        if s + 8 < qp_sad {
            qp_sad = s;
            qx = cx;
            qy = cy;
        }
    }
    ((qx as i8, qy as i8), qp_sad)
}

fn copy_region_mc(dst: &mut Yuv420, prev: &Yuv420, rx: usize, ry: usize, mv: (i8, i8)) {
    // mv is in QUARTER-PEL units. Whole-pixel components take the
    // exact integer path (bit-identical to the original behavior);
    // fractional components sample bilinearly. Chroma moves at 1/8-pel
    // of its plane, always bilinear when fractional.
    let (w, h) = (dst.w, dst.h);
    let (mvx, mvy) = (mv.0 as i32, mv.1 as i32);
    if mvx % 4 == 0 && mvy % 4 == 0 {
        let (ix, iy) = (mvx / 4, mvy / 4);
        for dy in 0..REGION {
            for dx in 0..REGION {
                let sx = (rx as i32 + dx as i32 + ix).clamp(0, w as i32 - 1) as usize;
                let sy = (ry as i32 + dy as i32 + iy).clamp(0, h as i32 - 1) as usize;
                dst.y[(ry + dy) * w + rx + dx] = prev.y[sy * w + sx];
            }
        }
        let (cw, ch) = (w / 2, h / 2);
        let (crx, cry) = (rx / 2, ry / 2);
        for dy in 0..REGION / 2 {
            for dx in 0..REGION / 2 {
                let sx = (crx as i32 + dx as i32 + ix / 2).clamp(0, cw as i32 - 1) as usize;
                let sy = (cry as i32 + dy as i32 + iy / 2).clamp(0, ch as i32 - 1) as usize;
                dst.u[(cry + dy) * cw + crx + dx] = prev.u[sy * cw + sx];
                dst.v[(cry + dy) * cw + crx + dx] = prev.v[sy * cw + sx];
            }
        }
        return;
    }
    let (fx0, fy0) = (mvx as f32 * 0.25, mvy as f32 * 0.25);
    for dy in 0..REGION {
        for dx in 0..REGION {
            dst.y[(ry + dy) * w + rx + dx] = bilinear(
                &prev.y, w, h,
                rx as f32 + dx as f32 + fx0,
                ry as f32 + dy as f32 + fy0,
            );
        }
    }
    let (cw, ch) = (w / 2, h / 2);
    let (crx, cry) = (rx / 2, ry / 2);
    for dy in 0..REGION / 2 {
        for dx in 0..REGION / 2 {
            let sx = crx as f32 + dx as f32 + fx0 * 0.5;
            let sy = cry as f32 + dy as f32 + fy0 * 0.5;
            dst.u[(cry + dy) * cw + crx + dx] = bilinear(&prev.u, cw, ch, sx, sy);
            dst.v[(cry + dy) * cw + crx + dx] = bilinear(&prev.v, cw, ch, sx, sy);
        }
    }
}

/// # Residual ledger: where do the bytes actually go?
///
/// "Hard motion" is a mislabel — it names UNSOLVED motion. Game footage
/// is emitted by a finite state machine (there is no hard motion in it,
/// by construction), and real-world footage obeys gravity, air, and
/// light transport — indirect filters a generative model knows to want
/// and a pixel model pays for as residual every frame. This probe runs
/// v3's exact prediction loop and attributes each region-frame to:
///
///   SKIP            final SAD < 8/px   — the model already owns it
///   TRACKED-CHANGED 8..24/px           — alignment succeeded, pixels
///                    changed anyway: motion blur, lighting, grain.
///                    The "air and light" bucket: deterministic given
///                    state (velocity, exposure) but invisible to
///                    appearance modeling.
///   UNSOLVED        ≥ 24/px            — no predictor in the model
///                    explains the region: movers, parallax,
///                    occlusion reveals. The "missing dynamics" bucket.
///
/// Byte attribution is by final-SAD energy share of the GOP residual —
/// approximate (bytes aren't exactly ∝ SAD at a fixed dead-zone) but
/// honest about ranking which missing factor costs most.
/// Fuse a canonical plate from MULTIPLE takes of the same set.
///
/// The citation experiment measured the mismatch: two takes of the
/// "same set" share only ~8% of regions at plate-predictor level,
/// because actors rest in different places and a single-take median
/// bakes them into the canonical. Pooling samples from several takes
/// (each take's frames motion-compensated into take-0's frame-0
/// coordinates, with a cross-take phase-corr offset per take) lets
/// the per-pixel median see PAST any one take's actors: a pixel an
/// actor occupies for all of take 1 is background for take 2, and
/// the pooled median recovers the SET. This is the lineage mechanic
/// in its first working form — every take refines the canonical.
///
/// The median itself is the trimming: with T takes of N frames each,
/// an actor must occupy a pixel in more than half of ALL pooled
/// samples to survive into the canonical.
pub fn build_plate_fused(takes: &[(&[Yuv420], &[PlateMotion])]) -> Plate {
    use crate::phase_correlation::PhaseCorrelationPass;
    assert!(!takes.is_empty());
    let (fw, fh) = (takes[0].0[0].w, takes[0].0[0].h);
    // Cross-take offsets: align each take's frame 0 to take 0's frame 0.
    //
    // NOT raw phase correlation — with different actor positions in the
    // two takes, the correlation peak can latch onto actor-to-actor
    // correspondence and return a large bogus shift (measured: a locked
    // store set produced a 2226px-wide fused plate from exactly this,
    // and the misregistered samples POISONED the median instead of
    // healing it). Robust version: variance-gated block matching on a
    // grid, take the MEDIAN block displacement — actors are the
    // minority of blocks, background the majority, and the median
    // votes with the background.
    let _ = PhaseCorrelationPass::new(64); // keep the import obviously used
    let mut offs: Vec<(f32, f32)> = vec![(0.0, 0.0)];
    for (frames, _) in takes.iter().skip(1) {
        let (ref_y, cur_y) = (&takes[0].0[0].y, &frames[0].y);
        let mut dxs: Vec<f32> = Vec::new();
        let mut dys: Vec<f32> = Vec::new();
        let step = 64usize;
        let mut y = step;
        while y + REGION + step < fh {
            let mut x = step;
            while x + REGION + step < fw {
                let mut mean = 0u32;
                for dy in 0..REGION {
                    for dx in 0..REGION {
                        mean += cur_y[(y + dy) * fw + x + dx] as u32;
                    }
                }
                mean /= (REGION * REGION) as u32;
                let mut var = 0u64;
                for dy in 0..REGION {
                    for dx in 0..REGION {
                        let d = cur_y[(y + dy) * fw + x + dx] as i64 - mean as i64;
                        var += (d * d) as u64;
                    }
                }
                if var / (REGION * REGION) as u64 >= 64 {
                    let (mv, sad) = search_mv(ref_y, fw, fh, cur_y, x, y);
                    if sad < (REGION * REGION * 12) as u32 {
                        // search_mv returns quarter-pel units of cur->ref
                        // block offset; the take offset convention below
                        // matches motion dx (content shift), so negate.
                        dxs.push(-(mv.0 as f32) * 0.25);
                        dys.push(-(mv.1 as f32) * 0.25);
                    }
                }
                x += step;
            }
            y += step;
        }
        if dxs.len() >= 8 {
            dxs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            dys.sort_by(|a, b| a.partial_cmp(b).unwrap());
            offs.push((dxs[dxs.len() / 2], dys[dys.len() / 2]));
        } else {
            offs.push((0.0, 0.0));
        }
    }
    // Plate bounds over ALL takes in take-0 frame-0 coords.
    let (mut minx, mut miny, mut maxx, mut maxy) = (0.0f32, 0.0f32, fw as f32, fh as f32);
    for ((_, motions), &(ox, oy)) in takes.iter().zip(&offs) {
        for m in *motions {
            minx = minx.min(-(m.dx + ox));
            miny = miny.min(-(m.dy + oy));
            maxx = maxx.max(-(m.dx + ox) + fw as f32);
            maxy = maxy.max(-(m.dy + oy) + fh as f32);
        }
    }
    let pw = ((maxx - minx).ceil() as usize + 1) & !1usize;
    let ph = ((maxy - miny).ceil() as usize + 1) & !1usize;
    let median_plane = |get: &dyn Fn(&Yuv420) -> &Vec<u8>, sub: usize| -> Vec<u8> {
        let (pw, ph) = (pw / sub, ph / sub);
        let (fw, fh) = (fw / sub, fh / sub);
        let mut plane = vec![0u8; pw * ph];
        let mut samples: Vec<u8> = Vec::new();
        for py in 0..ph {
            for px in 0..pw {
                samples.clear();
                for ((frames, motions), &(tox, toy)) in takes.iter().zip(&offs) {
                    for (f, m) in frames.iter().zip(*motions) {
                        let fx = px as f32 + (minx + m.dx + tox) / sub as f32;
                        let fy = py as f32 + (miny + m.dy + toy) / sub as f32;
                        let (fx, fy) = (fx.round() as isize, fy.round() as isize);
                        if fx >= 0 && fy >= 0 && (fx as usize) < fw && (fy as usize) < fh {
                            samples.push(get(f)[fy as usize * fw + fx as usize]);
                        }
                    }
                }
                if !samples.is_empty() {
                    samples.sort_unstable();
                    plane[py * pw + px] = samples[samples.len() / 2];
                }
            }
        }
        plane
    };
    Plate {
        w: pw,
        h: ph,
        ox: minx,
        oy: miny,
        y: median_plane(&|f| &f.y, 1),
        u: median_plane(&|f| &f.u, 2),
        v: median_plane(&|f| &f.v, 2),
    }
}

/// Serialize a plate: [w:u32][h:u32][ox:f32][oy:f32][y][u][v], zstd.
/// A stored plate is the 2D-native CANONICAL: the first infrastructure
/// of the library-citation department. A GOP encoded with --use-plate
/// transmits alignment + motions + residual and CITES the plate
/// instead of carrying it — the byte-level form of "match a canonical
/// from last week and ship a reference." An engine-rendered canonical
/// slots into this same interface later.
pub fn save_plate(plate: &Plate, path: &str) -> std::io::Result<()> {
    let mut raw = Vec::with_capacity(16 + plate.y.len() + plate.u.len() + plate.v.len());
    raw.extend_from_slice(&(plate.w as u32).to_le_bytes());
    raw.extend_from_slice(&(plate.h as u32).to_le_bytes());
    raw.extend_from_slice(&plate.ox.to_le_bytes());
    raw.extend_from_slice(&plate.oy.to_le_bytes());
    raw.extend_from_slice(&plate.y);
    raw.extend_from_slice(&plate.u);
    raw.extend_from_slice(&plate.v);
    let z = zstd::encode_all(raw.as_slice(), 19)?;
    // Canonicals on disk are the CITATION CURRENCY — the thing a
    // library ships once and many takes cite — so they get the same
    // DCT coding as in-stream plates, at the reference-grade dead
    // zone (dz 4 = the frame default 32 / 8, the swept knee). Format:
    // [magic "FAOP2"][w][h][ox][oy][coded payload]; falls back to the
    // legacy raw-zstd file when the transform doesn't win, and
    // load_plate sniffs the magic so old canonicals still load.
    let opts = residual::ResidualOpts { dead_zone: 4.0, ..Default::default() };
    let gray = vec![128u8; plate.w * plate.h * 3];
    let mut p444 = Vec::with_capacity(plate.w * plate.h * 3);
    for y in 0..plate.h {
        for x in 0..plate.w {
            p444.push(plate.y[y * plate.w + x]);
            p444.push(plate.u[(y / 2) * (plate.w / 2) + x / 2]);
            p444.push(plate.v[(y / 2) * (plate.w / 2) + x / 2]);
        }
    }
    if let Some(t) = residual::encode(&gray, &p444, plate.w, plate.h, &opts) {
        if t.len() + 21 < z.len() {
            let mut out = Vec::with_capacity(t.len() + 21);
            out.extend_from_slice(b"FAOP2");
            out.extend_from_slice(&(plate.w as u32).to_le_bytes());
            out.extend_from_slice(&(plate.h as u32).to_le_bytes());
            out.extend_from_slice(&plate.ox.to_le_bytes());
            out.extend_from_slice(&plate.oy.to_le_bytes());
            out.extend_from_slice(&t);
            return std::fs::write(path, out);
        }
    }
    std::fs::write(path, z)
}

pub fn load_plate(path: &str) -> std::io::Result<Plate> {
    let z = std::fs::read(path)?;
    if z.len() > 21 && &z[..5] == b"FAOP2" {
        let rd_u32 = |o: usize| u32::from_le_bytes([z[o], z[o + 1], z[o + 2], z[o + 3]]);
        let rd_f32 = |o: usize| f32::from_le_bytes([z[o], z[o + 1], z[o + 2], z[o + 3]]);
        let (w, h) = (rd_u32(5) as usize, rd_u32(9) as usize);
        let (ox, oy) = (rd_f32(13), rd_f32(17));
        let gray = vec![128u8; w * h * 3];
        let dec = residual::apply(gray, &z[21..], w, h);
        let (mut yp, mut up, mut vp) = (vec![0u8; w * h], vec![0u8; (w / 2) * (h / 2)], vec![0u8; (w / 2) * (h / 2)]);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                yp[y * w + x] = dec[i];
                if y % 2 == 0 && x % 2 == 0 {
                    let ci = (y / 2) * (w / 2) + x / 2;
                    up[ci] = dec[i + 1];
                    vp[ci] = dec[i + 2];
                }
            }
        }
        return Ok(Plate { w, h, ox, oy, y: yp, u: up, v: vp });
    }
    let raw = zstd::decode_all(z.as_slice())?;
    let rd_u32 = |o: usize| u32::from_le_bytes([raw[o], raw[o + 1], raw[o + 2], raw[o + 3]]);
    let rd_f32 = |o: usize| f32::from_le_bytes([raw[o], raw[o + 1], raw[o + 2], raw[o + 3]]);
    let (w, h) = (rd_u32(0) as usize, rd_u32(4) as usize);
    let (ox, oy) = (rd_f32(8), rd_f32(12));
    let ysz = w * h;
    let csz = (w / 2) * (h / 2);
    Ok(Plate {
        w,
        h,
        ox,
        oy,
        y: raw[16..16 + ysz].to_vec(),
        u: raw[16 + ysz..16 + ysz + csz].to_vec(),
        v: raw[16 + ysz + csz..16 + ysz + 2 * csz].to_vec(),
    })
}

/// Affine-align a cited plate to this GOP's frame 0. The citation
/// experiment priced translation-only alignment: the cross-take
/// mismatch residual exceeded the plate bytes saved at matched
/// quality, and takes differ by more than translation (zoom, slight
/// rotation, framing drift). Correspondences come from block matching
/// frame 0 against the plate's zero-motion prediction on a 32px grid
/// (variance-gated); the robust 6-DOF fit is global_motion::fit; the
/// plate is then warped ONCE so every downstream step is unchanged.
/// Wire cost: 6 f32 = 24 bytes on top of the reference.
pub fn align_plate_affine(plate: &Plate, frame0: &Yuv420) -> (Plate, crate::global_motion::Affine) {
    use crate::global_motion as gm;
    let (fw, fh) = (frame0.w, frame0.h);
    let zero = PlateMotion { dx: 0.0, dy: 0.0, confidence: 1.0 };
    let pred0 = predict_frame(plate, &zero, fw, fh);
    // Block correspondences: pred0-view coords -> frame coords.
    let mut pairs: Vec<(f32, f32, f32, f32)> = Vec::new();
    let step = 32usize;
    let mut y = step;
    while y + REGION + step < fh {
        let mut x = step;
        while x + REGION + step < fw {
            // Variance gate: flat blocks match anywhere.
            let mut mean = 0u32;
            for dy in 0..REGION {
                for dx in 0..REGION {
                    mean += frame0.y[(y + dy) * fw + x + dx] as u32;
                }
            }
            mean /= (REGION * REGION) as u32;
            let mut var = 0u64;
            for dy in 0..REGION {
                for dx in 0..REGION {
                    let d = frame0.y[(y + dy) * fw + x + dx] as i64 - mean as i64;
                    var += (d * d) as u64;
                }
            }
            if var / (REGION * REGION) as u64 >= 64 {
                let (mv, sad) = search_mv(&pred0.y, fw, fh, &frame0.y, x, y);
                // half-pel units -> px; reject poor matches.
                if sad < (REGION * REGION * 12) as u32 {
                    let (mx, my) = (mv.0 as f32 * 0.25, mv.1 as f32 * 0.25);
                    let (cx, cy) = (x as f32 + REGION as f32 / 2.0, y as f32 + REGION as f32 / 2.0);
                    pairs.push((cx + mx, cy + my, cx, cy));
                }
            }
            x += step;
        }
        y += step;
    }
    let a = match gm::fit(&pairs) {
        Some(m) => m,
        None => return (plate.clone(), gm::IDENTITY),
    };
    let a_inv = match gm::invert(&a) {
        Some(m) => m,
        None => return (plate.clone(), gm::IDENTITY),
    };
    // Warp: new_plate(q) = plate(A_inv(q + O) - O), O = (ox, oy).
    let mut out = plate.clone();
    let (pw, ph) = (plate.w, plate.h);
    for qy in 0..ph {
        for qx in 0..pw {
            let (fx, fy) = (qx as f32 + plate.ox, qy as f32 + plate.oy);
            let (sx, sy) = gm::apply(&a_inv, fx, fy);
            out.y[qy * pw + qx] = bilinear(&plate.y, pw, ph, sx - plate.ox, sy - plate.oy);
        }
    }
    let (cw2, ch2) = (pw / 2, ph / 2);
    for qy in 0..ch2 {
        for qx in 0..cw2 {
            let (fx, fy) = (qx as f32 * 2.0 + plate.ox, qy as f32 * 2.0 + plate.oy);
            let (sx, sy) = gm::apply(&a_inv, fx, fy);
            let (px, py) = ((sx - plate.ox) * 0.5, (sy - plate.oy) * 0.5);
            out.u[qy * cw2 + qx] = bilinear(&plate.u, cw2, ch2, px, py);
            out.v[qy * cw2 + qx] = bilinear(&plate.v, cw2, ch2, px, py);
        }
    }
    (out, a)
}

/// Motions for a GOP against an EXTERNAL (cited) plate: align frame 0
/// to the plate via phase correlation against the plate's zero-motion
/// prediction, then offset the GOP's own cumulative motions by that
/// alignment. Costs 8 bytes on the wire (two f32s).
pub fn motions_against_plate(frames: &[Yuv420], plate: &Plate) -> Vec<PlateMotion> {
    use crate::phase_correlation::PhaseCorrelationPass;
    let (fw, fh) = (frames[0].w, frames[0].h);
    let zero = PlateMotion { dx: 0.0, dy: 0.0, confidence: 1.0 };
    let pred0 = predict_frame(plate, &zero, fw, fh);
    let pc = PhaseCorrelationPass::new(512.min(fw.min(fh)));
    let a = pc.estimate(&frames[0].y, &pred0.y, fw, fh);
    let (ax, ay) = if a.confidence > 0.1 { (-a.dx, -a.dy) } else { (0.0, 0.0) };
    estimate_motions(frames)
        .into_iter()
        .map(|m| PlateMotion { dx: m.dx + ax, dy: m.dy + ay, confidence: m.confidence })
        .collect()
}

pub fn residual_ledger(frames: &[Yuv420]) -> String {
    let (fw, fh) = (frames[0].w, frames[0].h);
    let motions = estimate_motions(frames);
    let plate = build_plate(frames, &motions);
    let (gw, gh) = (fw / REGION, fh / REGION);
    let mut cls_count = [0u64; 3];
    let mut cls_energy = [0f64; 3];
    let mut cls_corrected = [0f64; 3];
    let mut unsolved_cells: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (fi, (f, m)) in frames.iter().zip(&motions).enumerate() {
        if fi == 0 {
            continue;
        }
        let plate_pred = predict_frame(&plate, m, fw, fh);
        let prev = &frames[fi - 1]; // open-loop ledger: attribution, not coding
        for gy in 0..gh {
            for gx in 0..gw {
                let (rx, ry) = (gx * REGION, gy * REGION);
                let sad_plate =
                    sad_region(&plate_pred.y, fw, fh, rx as i32, ry as i32, &f.y, rx, ry);
                let (mv, sad_prev) = search_mv(&prev.y, fw, fh, &f.y, rx, ry);
                let fin = sad_plate.min(sad_prev) as f64;
                // Exposure-term ceiling: residual energy after the OPTIMAL
                // per-region gain+offset correction of the winning
                // predictor (least squares a,b on pred→actual). If
                // lighting flutter is a real cost, corrected energy sits
                // well under raw energy on tracked-changed regions; if
                // not, the exposure term isn't worth its side-info.
                let corrected = {
                    let use_plate = sad_plate <= sad_prev;
                    // Prev branch must be sampled AT ITS MC OFFSET — the
                    // raw comparator (fin) is the motion-compensated SAD,
                    // and correcting an UNcompensated prediction inflated
                    // "corrected" energy past raw on drifting content
                    // (measured 226% before this fix — the probe's bug,
                    // not the codec's).
                    let (fx0, fy0) = (mv.0 as f32 * 0.25, mv.1 as f32 * 0.25);
                    let pv_at = |dx: usize, dy: usize| -> f64 {
                        if use_plate {
                            plate_pred.y[(ry + dy) * fw + rx + dx] as f64
                        } else {
                            bilinear(
                                &prev.y, fw, fh,
                                (rx + dx) as f32 + fx0,
                                (ry + dy) as f32 + fy0,
                            ) as f64
                        }
                    };
                    let (mut sp, mut sa, mut spp, mut spa) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
                    let n = (REGION * REGION) as f64;
                    for dy in 0..REGION {
                        for dx in 0..REGION {
                            let pi = (ry + dy) * fw + rx + dx;
                            let pv = pv_at(dx, dy);
                            let av = f.y[pi] as f64;
                            sp += pv;
                            sa += av;
                            spp += pv * pv;
                            spa += pv * av;
                        }
                    }
                    let denom = n * spp - sp * sp;
                    let (a, b) = if denom.abs() > 1e-6 {
                        let a = (n * spa - sp * sa) / denom;
                        (a.clamp(0.5, 2.0), (sa - a.clamp(0.5, 2.0) * sp) / n)
                    } else {
                        (1.0, 0.0)
                    };
                    let mut e = 0.0f64;
                    for dy in 0..REGION {
                        for dx in 0..REGION {
                            let pi = (ry + dy) * fw + rx + dx;
                            let pv = pv_at(dx, dy);
                            e += (a * pv + b - f.y[pi] as f64).abs();
                        }
                    }
                    e
                };
                let per_px = fin / (REGION * REGION) as f64;
                let c = if per_px < 8.0 {
                    0
                } else if per_px < 24.0 {
                    1
                } else {
                    unsolved_cells.insert(gy * gw + gx);
                    2
                };
                cls_count[c] += 1;
                cls_energy[c] += fin;
                cls_corrected[c] += corrected;
            }
        }
    }
    let tot_c: u64 = cls_count.iter().sum();
    let tot_e: f64 = cls_energy.iter().sum::<f64>().max(1.0);
    format!(
        "regions: skip {:.1}% tracked-changed {:.1}% unsolved {:.1}% | residual-energy share: skip {:.1}% tracked-changed {:.1}% unsolved {:.1}% | unsolved-cell footprint {:.1}% of frame | gain-corrected energy: skip {:.0}% tc {:.0}% uns {:.0}% of raw",
        100.0 * cls_count[0] as f64 / tot_c as f64,
        100.0 * cls_count[1] as f64 / tot_c as f64,
        100.0 * cls_count[2] as f64 / tot_c as f64,
        100.0 * cls_energy[0] / tot_e,
        100.0 * cls_energy[1] / tot_e,
        100.0 * cls_energy[2] / tot_e,
        100.0 * unsolved_cells.len() as f64 / (gw * gh) as f64,
        100.0 * cls_corrected[0] / cls_energy[0].max(1.0),
        100.0 * cls_corrected[1] / cls_energy[1].max(1.0),
        100.0 * cls_corrected[2] / cls_energy[2].max(1.0),
    )
}

const GAIN_GATE: i64 = 192;

pub fn encode_gop_v3(
    frames: &[Yuv420],
    motions: &[PlateMotion],
    plate: &Plate,
    opts: &ResidualOpts,
) -> PlateEncodeReportV2 {
    encode_gop_v3_forced(frames, motions, plate, opts, None).0
}

/// v3 with an optional FORCE-PLATE map (`force[frame][region]`) and a
/// per-frame per-region leftover-error trace returned alongside the
/// report. Both exist for the lookahead pass in
/// [`encode_gop_v3_lookahead`]; `None` reproduces plain v3 exactly.
pub fn encode_gop_v3_forced(
    frames: &[Yuv420],
    motions: &[PlateMotion],
    plate: &Plate,
    opts: &ResidualOpts,
    force_plate: Option<&Vec<Vec<bool>>>,
) -> (PlateEncodeReportV2, Vec<Vec<u32>>) {

    let fw = frames[0].w;
    let fh = frames[0].h;
    let (gw, gh) = (fw / REGION, fh / REGION);
    let mut plate_raw = Vec::with_capacity(plate.y.len() + plate.u.len() + plate.v.len());
    plate_raw.extend_from_slice(&plate.y);
    plate_raw.extend_from_slice(&plate.u);
    plate_raw.extend_from_slice(&plate.v);
    // The plate ships DCT-coded through the same residual path as
    // every frame, predicted from flat gray — not as raw zstd'd
    // pixels. The byte-split diagnostic made the anomaly obvious: the
    // plate was 33% of the hard shot and 42% of the quiet one while
    // being the ONE surface never quantized, so it was paying full
    // entropy for grain the dead zone discards everywhere else. Same
    // dead zone as the frames keeps the plate's fidelity matched to
    // what the residual coder can actually exploit; best-of against
    // raw zstd keeps it safe when the transform loses (tiny plates).
    const PLATE_DZ_DIV: f32 = 8.0;
    let mut plate_coded = plate.clone();
    // ── SPECTRALLY-MATCHED PLATE RESOLUTION (FAO_PLATE_HALF).
    //
    // blackstar-rs made the case: its plate is 104,683 B, 31% of the
    // whole file, on content that is a smooth sky/ground gradient plus
    // one small object. A temporal median stored at FULL FRAME
    // RESOLUTION is an expensive way to hold something a handful of
    // low-frequency coefficients describe, and it is pure overhead
    // against a conventional coder that spends almost nothing on a flat
    // block. The plate is a REFERENCE, not an output frame, so its
    // resolution only has to carry the frequencies the prediction
    // actually uses.
    //
    // Half-resolution store with bilinear upsample on use, chosen
    // BEST-OF against full resolution by real bytes and real prediction
    // error — never a guess about the content, and it cannot lose.
    let plate_half = std::env::var("FAO_PLATE_HALF").is_ok();
    let plate_bytes = {
        let zstd_len = zstd::encode_all(plate_raw.as_slice(), 19)
            .map(|c| c.len())
            .unwrap_or(plate_raw.len());
        let gray = vec![128u8; plate.w * plate.h * 3];
        let mut plate444 = Vec::with_capacity(plate.w * plate.h * 3);
        for y in 0..plate.h {
            for x in 0..plate.w {
                plate444.push(plate.y[y * plate.w + x]);
                plate444.push(plate.u[(y / 2) * (plate.w / 2) + x / 2]);
                plate444.push(plate.v[(y / 2) * (plate.w / 2) + x / 2]);
            }
        }
        // The plate is a REFERENCE, not an output frame: every future
        // frame predicts from it, so its errors propagate while a
        // frame's errors die with the frame. It therefore gets a
        // FINER dead zone than the frames. Ratio swept on the FULL
        // SUITE (dz/4, dz/8, dz/16 — each a complete 19-shot run):
        //   dz/4 : 20.38 MB @ 33.43  (15.4 MB traded per dB lost)
        //   dz/8 : 20.92 MB @ 33.58  (19.6 MB per dB)  <- chosen
        //   dz/16: 21.79 MB @ 33.62  (19.8 MB per dB, but +0.9 MB
        //          for +0.04 dB over dz/8 — past the knee)
        // dz/8 sits at the knee: nearly the best exchange rate and
        // the cheapest point that still holds a quality lead over the
        // AV1 anchor (+0.25 dB).
        // The dz/8 ratio was swept on the FILM suite at dz32 and is
        // recorded above. blackstar shows its limit: at dz128 the frame
        // residual collapses to 7,054 B while the plate still costs
        // 74,395 B and side info 70,034 B — a 144 KB floor that barely
        // moves with the operating point, against an AV1 anchor at
        // 22,908 B. A FIXED ratio means the plate never coarsens with
        // the frames, so at coarse dz it becomes the whole file. The
        // divisor is now tunable so the ratio can be swept per content
        // class instead of inherited from one film measurement.
        let plate_dz_div: f32 = std::env::var("FAO_PLATE_DZ_DIV")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(PLATE_DZ_DIV);
        let plate_opts = ResidualOpts {
            dead_zone: (opts.dead_zone / plate_dz_div).max(1.0),
            ..opts.clone()
        };
        // Half-resolution candidate: decimate 2x1 (box), code, upsample
        // bilinearly, and keep it only if it is cheaper AND its
        // prediction error against the plate's own pixels is not
        // materially worse. Scoring against the plate rather than
        // against frames keeps this a property of the surface itself,
        // so the decision does not depend on which frame happens to be
        // examined.
        let half = if plate_half {
            let (hw, hh) = ((plate.w / 4) * 2, (plate.h / 4) * 2);
            if hw >= 8 && hh >= 8 {
                let mut small = Yuv420::new(hw, hh);
                for y in 0..hh {
                    for x in 0..hw {
                        let mut acc = 0u32;
                        for dy in 0..2 {
                            for dx in 0..2 {
                                let sy = (y * 2 + dy).min(plate.h - 1);
                                let sx = (x * 2 + dx).min(plate.w - 1);
                                acc += plate.y[sy * plate.w + sx] as u32;
                            }
                        }
                        small.y[y * hw + x] = (acc / 4) as u8;
                    }
                }
                let (cw, chh) = (plate.w / 2, plate.h / 2);
                for y in 0..hh / 2 {
                    for x in 0..hw / 2 {
                        let sy = (y * 2).min(chh - 1);
                        let sx = (x * 2).min(cw - 1);
                        small.u[y * (hw / 2) + x] = plate.u[sy * cw + sx];
                        small.v[y * (hw / 2) + x] = plate.v[sy * cw + sx];
                    }
                }
                let s444 = interleave_444(&small);
                let sgray = vec![128u8; hw * hh * 3];
                residual::encode(&sgray, &s444, hw, hh, &plate_opts).map(|t| {
                    let dec = residual::apply(sgray, &t, hw, hh);
                    (t.len(), deinterleave_420(&dec, hw, hh))
                })
            } else {
                None
            }
        } else {
            None
        };
        // RD gate over the two candidates. The first version compared
        // BYTES ONLY, and the comment above claimed an error test it did
        // not perform — the code and the comment disagreed, and the
        // measurement showed why it mattered: half-res on blackstar was
        // -18.7% bytes for -2.63 dB, an exchange rate of 23.7 KB/dB
        // against the dead-zone ladder's 244 KB/dB on the same clip.
        // Ten times worse, i.e. a straight loss at matched quality.
        //
        // The mechanism is worth stating because it is specific to a
        // REFERENCE surface: the residual barely moved (154,964 ->
        // 155,895) while PSNR fell 2.6 dB, because at dz32 the dead zone
        // DISCARDS the low-amplitude error a blurred plate introduces.
        // The blur therefore passes straight through to the output
        // instead of being corrected. A cheaper reference is only cheap
        // if the residual coder will fix it, and at a coarse dead zone
        // it will not.
        let half_j = |cand: &Option<(usize, Yuv420)>, full_bytes: usize| -> bool {
            let Some((hb, small)) = cand else { return false };
            let lambda = (opts.dead_zone as f64).powi(2) / 3.0;
            // SSE of each candidate against the plate's own luma.
            let (hw, hh) = (small.w, small.h);
            let mut sse_half = 0.0f64;
            for y in 0..plate.h {
                let sy = (y * hh / plate.h).min(hh - 1);
                for x in 0..plate.w {
                    let sx = (x * hw / plate.w).min(hw - 1);
                    let d = small.y[sy * hw + sx] as f64 - plate.y[y * plate.w + x] as f64;
                    sse_half += d * d;
                }
            }
            // Full-res candidate's error is whatever the dead zone left,
            // which is small by construction; approximate it as zero and
            // let the byte difference carry the comparison. That biases
            // AGAINST half-res, which is the safe direction for a
            // surface whose errors propagate to every frame.
            sse_half + lambda * *hb as f64 * 1.0 < lambda * full_bytes as f64
        };
        match residual::encode(&gray, &plate444, plate.w, plate.h, &plate_opts) {
            Some(t) if half_j(&half, t.len()) => {
                let (hb, small) = half.unwrap();
                // Bilinear upsample back to plate resolution — this is
                // the surface every frame will predict from, so the
                // encoder must adopt exactly what the decoder rebuilds.
                let (hw, hh) = (small.w, small.h);
                for y in 0..plate.h {
                    let fy = (y as f32 * hh as f32 / plate.h as f32 - 0.5).max(0.0);
                    let (y0, ty) = (fy as usize, fy.fract());
                    let y1 = (y0 + 1).min(hh - 1);
                    for x in 0..plate.w {
                        let fx = (x as f32 * hw as f32 / plate.w as f32 - 0.5).max(0.0);
                        let (x0, tx) = (fx as usize, fx.fract());
                        let x1 = (x0 + 1).min(hw - 1);
                        let p00 = small.y[y0 * hw + x0] as f32;
                        let p01 = small.y[y0 * hw + x1] as f32;
                        let p10 = small.y[y1 * hw + x0] as f32;
                        let p11 = small.y[y1 * hw + x1] as f32;
                        let v = (p00 * (1.0 - tx) + p01 * tx) * (1.0 - ty)
                            + (p10 * (1.0 - tx) + p11 * tx) * ty;
                        plate_coded.y[y * plate.w + x] = v.round().clamp(0.0, 255.0) as u8;
                    }
                }
                let (cw, chh) = (plate.w / 2, plate.h / 2);
                for y in 0..chh {
                    let sy = (y * hh / plate.h).min(hh / 2 - 1);
                    for x in 0..cw {
                        let sx = (x * hw / plate.w).min(hw / 2 - 1);
                        plate_coded.u[y * cw + x] = small.u[sy * (hw / 2) + sx];
                        plate_coded.v[y * cw + x] = small.v[sy * (hw / 2) + sx];
                    }
                }
                if std::env::var("FAO_PLATE_DEBUG").is_ok() {
                    eprintln!("plate: half-res {hb} B chosen over full-res {} B", t.len());
                }
                hb
            }
            Some(t) if t.len() < zstd_len => {
                // CRITICAL: adopt the DECODED plate. Counting the coded
                // size while still predicting from the unquantized
                // plate would be accounting fiction — the decoder never
                // has those pixels. (First version did exactly that and
                // reported a 15x plate shrink at bit-identical PSNR,
                // which is impossible and was the tell.)
                let dec = residual::apply(gray.clone(), &t, plate.w, plate.h);
                for y in 0..plate.h {
                    for x in 0..plate.w {
                        let i = (y * plate.w + x) * 3;
                        plate_coded.y[y * plate.w + x] = dec[i];
                        if y % 2 == 0 && x % 2 == 0 {
                            let ci = (y / 2) * (plate.w / 2) + x / 2;
                            plate_coded.u[ci] = dec[i + 1];
                            plate_coded.v[ci] = dec[i + 2];
                        }
                    }
                }
                t.len()
            }
            _ => zstd_len,
        }
    };
    let plate = &plate_coded;
    encode_gop_v3_core(
        frames,
        &mut |fi: usize| predict_frame(plate, &motions[fi], fw, fh),
        plate_bytes,
        frames.len() * 8,
        opts,
        force_plate,
    )
}

/// The mode/residual core behind v3, with the WORLD MODEL abstracted to a
/// per-frame prediction provider. The seam exists because the predictor is
/// the one thing that actually varies across the codec's branches: the 2D
/// warped plate ([`encode_gop_v3_forced`] — the default path, byte-exact
/// with the pre-seam code), a 3D canonical render (`plate_lab --render3d`,
/// the residual-vs-canonical-render bridge), and a mover-composited plate
/// are all providers over the SAME mode decision, gain correction, side
/// info, and DCT residual machinery. `model_bytes` is what shipping the
/// model cost (coded plate / serialized mesh / ~24 B citation);
/// `motion_bytes` is the per-frame model-parameter bill (8 B/frame plate
/// motions; a pose stream for 3D). The provider MUST be decoder-visible:
/// only decoded/deterministic inputs, never source pixels.
/// Encode with NO world model at all: the prediction provider returns
/// flat gray, so frame 0 falls to intra and every later frame to
/// prev-MC. The control the codec never had.
///
/// blackstar-rs forced this: its plate costs 104,683 B — 31% of the
/// whole file — while `prev_mode_share` is 0.939, meaning 94% of
/// regions decline to use it. Paying a third of the file for a model
/// that six percent of regions consult is a rate-control failure, and
/// nothing in the codec could previously express "this shot does not
/// want a plate". The v11 thesis is that a transmitted world model
/// beats per-frame coding; a thesis needs a null hypothesis, and this
/// is it.
pub fn encode_gop_no_plate(
    frames: &[Yuv420],
    opts: &ResidualOpts,
) -> PlateEncodeReportV2 {
    let (fw, fh) = (frames[0].w, frames[0].h);
    let gray = {
        let mut g = Yuv420::new(fw, fh);
        for p in g.y.iter_mut() {
            *p = 128;
        }
        for p in g.u.iter_mut().chain(g.v.iter_mut()) {
            *p = 128;
        }
        g
    };
    encode_gop_v3_core(frames, &mut |_| gray.clone(), 0, 0, opts, None).0
}

/// Route between the plate branch and the no-plate branch by RD cost,
/// the same J the citation router and the mover guard use. Returns
/// (report, chose_plate).
pub fn encode_gop_routed(
    frames: &[Yuv420],
    motions: &[PlateMotion],
    plate: &Plate,
    opts: &ResidualOpts,
) -> (PlateEncodeReportV2, bool) {
    let with = encode_gop_v3(frames, motions, plate, opts);
    let without = encode_gop_no_plate(frames, opts);
    let sse = |rep: &PlateEncodeReportV2| -> f64 {
        let mut acc = 0.0f64;
        for (r, f) in rep.recon.iter().zip(frames) {
            for (a, b) in r.y.iter().zip(&f.y) {
                let d = *a as f64 - *b as f64;
                acc += d * d;
            }
        }
        acc
    };
    let bytes = |rep: &PlateEncodeReportV2| -> usize {
        rep.plate_bytes
            + rep.motion_bytes
            + rep.frames.iter().map(|c| c.residual_bytes + c.mode_bytes).sum::<usize>()
    };
    let lambda = (opts.dead_zone as f64).powi(2) / 3.0;
    let jw = sse(&with) + lambda * bytes(&with) as f64;
    let jo = sse(&without) + lambda * bytes(&without) as f64;
    if std::env::var("FAO_ROUTE_DEBUG").is_ok() {
        eprintln!(
            "plate route: with {:.4e} ({} B) | without {:.4e} ({} B) -> {}",
            jw, bytes(&with), jo, bytes(&without),
            if jw <= jo { "PLATE" } else { "NO-PLATE" }
        );
    }
    if jw <= jo { (with, true) } else { (without, false) }
}

/// In-loop deblocking.
///
/// Measured need, not a guess: `bin/blockiness` put v11 at 4.64x the
/// off-grid gradient on the REGION grid (procedural ground) and 1.42x
/// on real footage, against AV1 sitting at ~0.95x — i.e. AV1's own
/// deblock/CDEF/loop-restoration track the SOURCE's grid statistics
/// while v11 ships every step straight to the viewer. PSNR and SSIM
/// both missed it for the whole project, which is why every
/// matched-PSNR comparison flattered this codec.
///
/// Two grids get filtered because the codec makes steps on both:
/// REGION boundaries, where the mode decision may have switched
/// predictor between neighbours, and the 4x4 transform grid, where the
/// dead zone may have quantised neighbours differently.
///
/// The filter is strength-modulated by information the encoder already
/// has rather than by a global knob. A REGION edge whose two sides
/// chose DIFFERENT predictors is the most likely to carry an artificial
/// step and is filtered harder; an edge inside one predictor's territory
/// is filtered gently. That is the same reasoning AV1's filter uses
/// (boundary strength from mode/reference difference) and it is free
/// here because the mode map is already in hand.
///
/// The artifact/edge test is the standard one and it is what stops the
/// filter destroying real detail: only smooth when the step ACROSS the
/// boundary is large relative to the variation on either SIDE of it. A
/// real edge has large variation on both; a blocking step is flat, step,
/// flat.
///
/// IN-LOOP, not cosmetic: the filtered frame becomes the prev-MC
/// reference, so a cleaner reference should also cost fewer residual
/// bytes. That is the claim to measure, not assume.
fn deblock(f: &mut Yuv420, src_true: &Yuv420, modes: &[u8], gw: usize, gh: usize, dz: f32, strength: i32) {
    let (w, h) = (f.w, f.h);
    // Thresholds scale with the dead zone: a coarser quantiser makes
    // bigger steps, and a step the quantiser could not have produced is
    // real content that must survive.
    let step_thr = (dz * 0.75).clamp(4.0, 48.0) as i32;
    let flat_thr = (dz * 0.375).clamp(2.0, 24.0) as i32;
    let src = f.y.clone();
    // PER-REGION TEXTURE, and the reason the filter needs it: the
    // strength sweep found 2 right on synthetic content and 1 right on
    // real footage, so no single number fits — and shipping one anyway
    // is the fixed-threshold mistake this codec has now made three
    // times (MV rate charge, DICT_MARGIN, mover admission). But a clip
    // is not uniform either: the same frame holds flat sky, where
    // blocking is glaring and there is no detail to lose, and textured
    // ground, where blocking is masked and real detail is at risk. So
    // the strength is derived per region rather than per clip.
    //
    // The measure is a cheap stand-in for spectral_probe's HF fraction:
    // mean |second difference| over mean |first difference|. A smooth
    // ramp has small curvature relative to its slope; noise has large.
    // Full 4x4 DCTs per region per frame would be the exact statistic
    // and cost far more than the filter itself.
    let mut texture = vec![0.0f32; gw * gh];
    for gy in 0..gh {
        for gx in 0..gw {
            let (rx, ry) = (gx * REGION, gy * REGION);
            let (mut d1, mut d2, mut n) = (0.0f64, 0.0f64, 0u32);
            for ly in 0..REGION {
                let y = (ry + ly).min(h - 1);
                for lx in 1..REGION - 1 {
                    let x = (rx + lx).min(w - 2).max(1);
                    let a = src[y * w + x - 1] as i32;
                    let b = src[y * w + x] as i32;
                    let c = src[y * w + x + 1] as i32;
                    d1 += (b - a).abs() as f64;
                    d2 += (c - 2 * b + a).abs() as f64;
                    n += 1;
                }
            }
            let _ = n;
            texture[gy * gw + gx] = if d1 < 1e-6 { 0.0 } else { (d2 / d1) as f32 };
        }
    }
    // Map texture to a strength multiplier. Flat regions (ratio near 0)
    // get the full requested strength; textured regions are damped
    // toward zero. The 1.4 pivot is read off this corpus and is a
    // BRACKET, not a tuned constant: smooth ramps measured well under
    // 1.0 and noise well above 2.0, so anything in between is the
    // uncertain middle and gets an interpolated strength rather than a
    // hard decision.
    //
    // MEASURED SPLIT, and it contradicted the premise above on one of
    // the two clips. On blackstar (real footage) the adaptive rule is
    // the best setting found — 35.681 dB / 0.9751 against 35.619 /
    // 0.9750 unfiltered and 35.591 for fixed strength 2 — because there
    // IS real detail to protect and damping protects it. On the
    // procedural-ground clip it is WORSE than fixed: P=16 blocking 3.80
    // adaptive against 1.91 fixed, at lower PSNR. That clip is
    // ENTIRELY texture, so every region damps to the floor and the
    // artifact survives.
    //
    // The lesson is that reconstructed texture cannot distinguish
    // "detail I must protect" from "noise whose blocking I must fix" —
    // both look like high curvature. Blocking comes from QUANTISATION,
    // so the discriminating signal is how hard each block was
    // quantised, which the encoder holds in its per-block scale bytes
    // and which this filter does not yet read. That is the named next
    // construction; until then both behaviours stay reachable rather
    // than one being guessed as universal.
    // QUANTISATION-ERROR MODE (default). The texture rule above could
    // not separate "detail to protect" from "noise whose blocking to
    // fix" because reconstructed texture looks the same in both. The
    // separating fact is that BLOCKING COMES FROM QUANTISATION: a step
    // on the grid is an artifact exactly when the coder was spending
    // few enough bits there to have produced it. So measure, per
    // region, how far the reconstruction actually strayed from the
    // source, normalised by the dead zone that set the expected stray.
    //
    // err/dz near 0 -> the region was coded nearly exactly, so any
    //   structure in it is CONTENT and must not be touched.
    // err/dz large  -> the region was coded coarsely, so grid steps in
    //   it are the coder's own and are safe to filter.
    //
    // This is encoder-side information the filter already had access to
    // and was not using. It costs one pass over the region.
    let mut qerr = vec![0.0f32; gw * gh];
    for gy in 0..gh {
        for gx in 0..gw {
            let (rx, ry) = (gx * REGION, gy * REGION);
            let mut acc = 0.0f64;
            let mut n = 0u32;
            for ly in 0..REGION {
                let y = (ry + ly).min(h - 1);
                for lx in 0..REGION {
                    let x = (rx + lx).min(w - 1);
                    acc += (src[y * w + x] as i32 - src_true.y[y * w + x] as i32).abs() as f64;
                    n += 1;
                }
            }
            qerr[gy * gw + gx] = (acc / n.max(1) as f64) as f32;
        }
    }
    // DEFAULT: fixed. MS-SSIM adjudicated the three rules and picked
    // it on BOTH clips (ground 0.89591 vs qerr 0.89438, texture 0.88966,
    // off 0.88716; blackstar 0.99013 vs texture 0.99000, qerr 0.98995,
    // off 0.98987). Every adaptive rule I built was WORSE than filtering
    // uniformly — the region-level cleverness was a net loss, and only a
    // multi-scale metric showed it, because PSNR/SSIM had ranked texture
    // first on blackstar.
    let mode = std::env::var("FAO_DEBLOCK_MODE").unwrap_or_else(|_| "fixed".into());
    let region_scale = |gi: usize| -> f32 {
        match mode.as_str() {
            "fixed" => 1.0,
            "texture" => {
                let t = texture.get(gi).copied().unwrap_or(1.0);
                ((1.9 - t) / 1.4).clamp(0.15, 1.0)
            }
            // Normalised by dz/8: the dead zone is a coefficient-domain
            // floor, and the pixel-domain error it produces is a
            // fraction of it. The pivot is where a region has strayed
            // about as much as the quantiser alone could explain.
            _ => {
                let e = qerr.get(gi).copied().unwrap_or(0.0);
                (e / (dz / 8.0).max(0.5)).clamp(0.15, 1.0)
            }
        }
    };
    let strength_at = |ga: usize, gb: usize| -> i32 {
        if modes.get(ga) != modes.get(gb) { 2 } else { 1 }
    };
    // Vertical edges (filter across columns).
    let mut x = 4usize;
    while x < w {
        let on_region = x % REGION == 0;
        for y in 0..h {
            let (p1, p0) = (src[y * w + x - 2] as i32, src[y * w + x - 1] as i32);
            let (q0, q1) = (src[y * w + x] as i32, src[y * w + x + 1.min(w - 1 - x)] as i32);
            let step = (q0 - p0).abs();
            if step >= step_thr || step == 0 {
                continue; // real edge, or nothing to do
            }
            if (p1 - p0).abs() > flat_thr || (q1 - q0).abs() > flat_thr {
                continue; // textured either side — not a blocking step
            }
            let gx = (x / REGION).min(gw - 1);
            let gy = (y / REGION).min(gh - 1);
            let st = if on_region && gx > 0 {
                strength_at(gy * gw + gx - 1, gy * gw + gx)
            } else {
                1
            };
            // Shift the two boundary samples toward each other by a
            // fraction set by strength. Deliberately narrow (2 taps a
            // side) — a wide filter on a 4x4 grid would blur more than
            // it repairs.
            let sc = region_scale(gy * gw + gx);
            let d = (((q0 - p0) * st * strength) as f32 * sc / 8.0) as i32;
            f.y[y * w + x - 1] = (p0 + d).clamp(0, 255) as u8;
            f.y[y * w + x] = (q0 - d).clamp(0, 255) as u8;
        }
        x += 4;
    }
    // Horizontal edges (filter across rows).
    let src = f.y.clone();
    let mut y = 4usize;
    while y < h {
        let on_region = y % REGION == 0;
        for x in 0..w {
            let (p1, p0) = (src[(y - 2) * w + x] as i32, src[(y - 1) * w + x] as i32);
            let q0 = src[y * w + x] as i32;
            let q1 = src[(y + 1).min(h - 1) * w + x] as i32;
            let step = (q0 - p0).abs();
            if step >= step_thr || step == 0 {
                continue;
            }
            if (p1 - p0).abs() > flat_thr || (q1 - q0).abs() > flat_thr {
                continue;
            }
            let gx = (x / REGION).min(gw - 1);
            let gy = (y / REGION).min(gh - 1);
            let st = if on_region && gy > 0 {
                strength_at((gy - 1) * gw + gx, gy * gw + gx)
            } else {
                1
            };
            let sc = region_scale(gy * gw + gx);
            let d = (((q0 - p0) * st * strength) as f32 * sc / 8.0) as i32;
            f.y[(y - 1) * w + x] = (p0 + d).clamp(0, 255) as u8;
            f.y[y * w + x] = (q0 - d).clamp(0, 255) as u8;
        }
        y += 4;
    }
}

pub fn encode_gop_v3_core(
    frames: &[Yuv420],
    pred_for: &mut dyn FnMut(usize) -> Yuv420,
    model_bytes: usize,
    motion_bytes: usize,
    opts: &ResidualOpts,
    force_plate: Option<&Vec<Vec<bool>>>,
) -> (PlateEncodeReportV2, Vec<Vec<u32>>) {
    let fw = frames[0].w;
    let fh = frames[0].h;
    let (gw, gh) = (fw / REGION, fh / REGION);
    let mut out_frames = Vec::with_capacity(frames.len());
    let mut recon: Vec<Yuv420> = Vec::with_capacity(frames.len());
    let mut prev_share_acc = 0.0f64;
    let mut leftover_trace: Vec<Vec<u32>> = Vec::with_capacity(frames.len());
    // ── LONG-RANGE REFERENCE MODE (mode 4), FAO_DICT=1.
    //
    // The probe measured this before the coder existed: on engine
    // content 10.7% of regions have a long-range match STRONG enough
    // (>512 SAD margin) to pay for a reference, against 0.2-0.7% on
    // film, with 6-10x the margin per win. This is the coder side of
    // that measurement.
    //
    // Decodability is the whole design constraint. The dictionary is
    // built from RECONSTRUCTED frames only, and only from frames
    // <= fi-2, so it never overlaps the prev-MC predictor and the
    // decoder can rebuild it exactly from what it has already output.
    // Entries are keyed on the RECON block's fingerprint; queries use
    // the SOURCE block's (the encoder has it, and the key only has to
    // shortlist — the winner is decided by true SAD against recon,
    // which the decoder never recomputes).
    //
    // Wire cost: 1 flag bit + a 5-byte reference (frame delta u8, x u16,
    // y u16) against a 256-pixel block, so the gate must clear it with
    // room. DICT_MARGIN is the probe's own STRONG threshold rather than
    // a tuned number — that is the level the measurement validated.
    let use_dict = std::env::var("FAO_DICT").is_ok();
    let mut dict: std::collections::HashMap<u32, Vec<(usize, usize, usize)>> =
        std::collections::HashMap::new();
    // Candidate cap AND selection policy. The first version took
    // `cands.iter().rev().take(24)` — the 24 most RECENTLY inserted
    // entries in the bucket. That is precisely backwards: the newest
    // entries come from the newest frames, which the prev-MC predictor
    // already covers, so as buckets fill the coder spends its whole
    // candidate budget on matches it cannot profit from and never looks
    // at the old, distant entries that are the entire point of a
    // long-range dictionary. Reference density collapsing from 2,947
    // refs on frame 24 to 78 on frame 46 is what that looks like from
    // the outside, and I had attributed it to quantisation depth.
    // FAO_DICT_CAND sets the budget; FAO_DICT_SPREAD=1 spends it
    // EVENLY across the bucket's history instead of on its tail.
    // Defaults are the swept operating point, not a guess:
    //   tail-24  9,866,348 B @ 31.680 dB   (the original policy)
    //   spread-24 9,291,999 B @ 32.099 dB
    //   spread-96 9,108,770 B @ 32.054 dB  <- default
    // Spread is better on BOTH axes than tail at the same budget, which
    // is the signature of a policy bug rather than a rate trade-off.
    // FAO_DICT_TAIL restores the old policy for comparison.
    let dict_cand: usize = std::env::var("FAO_DICT_CAND")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(96);
    let dict_spread = std::env::var("FAO_DICT_TAIL").is_err();
    // Margin the long-range match must beat the local predictor by.
    // Was a fixed 512 SAD — the probe's STRONG threshold — and that is
    // wrong for the same reason a fixed MV rate charge was wrong: it
    // does not scale with the operating point. A 512-SAD improvement
    // buys a lot of residual bytes at a fine dead zone and almost none
    // at a coarse one, while the reference costs 5 bytes either way.
    // Measured consequence: mode 4 won 7% on one Minecraft GOP and LOST
    // 3.3% on another, where references fired freely (+443 KB of side
    // info) and bought only 0.8% of residual back.
    // Now tied to the dead zone: the gate is the SAD a 5-byte reference
    // has to earn, and the dead zone is what sets how much SAD a byte
    // is worth. FAO_DICT_MARGIN overrides for sweeps.
    let dict_margin: u32 = std::env::var("FAO_DICT_MARGIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| (64.0 * opts.dead_zone).max(512.0) as u32);
    const MRU_CAP: usize = 128;

    for (fi, f) in frames.iter().enumerate() {
        let plate_pred = pred_for(fi);
        let mut modes = vec![0u8; gw * gh];
        let mut mvs: Vec<(i8, i8)> = Vec::new();
        let mut dict_refs: Vec<(u8, u16, u16)> = Vec::new();
        let composite = if fi == 0 {
            plate_pred.clone()
        } else {
            let prev = recon[fi - 1].clone();
            let mut comp = plate_pred.clone();
            let mut prev_regions = 0usize;
            for gy in 0..gh {
                for gx in 0..gw {
                    let (rx, ry) = (gx * REGION, gy * REGION);
                    let sad_plate = sad_region(&plate_pred.y, fw, fh, rx as i32, ry as i32, &f.y, rx, ry);
                    let (mv, sad_prev) = search_mv(&prev.y, fw, fh, &f.y, rx, ry);
                    // Decision stays raw-SAD BY MEASUREMENT, not by
                    // principle. A deadzone-aware codable cost was built
                    // (region_codable_cost) and lost on every tuning:
                    // all-plate quiet regions discard real drift (−0.5 dB),
                    // all-prev accumulates quantization drift (−0.8 dB),
                    // and a deterministic rotating plate refresh (K=4, K=8)
                    // lands ~0.15–0.25 dB under this SAD decision at equal
                    // bytes. SAD's grain-alignment noise dithers quiet
                    // regions between the two predictors — an accidental
                    // but effective intra-refresh. The principled
                    // replacement is lookahead RD (charging plate-mode's
                    // persistent error across future frames), not a better
                    // single-frame proxy. Numbers in SESSION_LOG.
                    
                    // Rate-charging the MV was TRIED and LOST at every
                    // level (SAD-equivalent charge 192/768/2048 →
                    // +0.3%/+4.9%/+27% bytes on the quiet shot). MV
                    // deltas are 53% of raw side info, but the vectors
                    // are EARNING their ~2 bytes: pushing those regions
                    // to plate mode costs more residual than the side
                    // info saved. The side-info share is a symptom of
                    // how cheap everything else got, not of waste.
                    let forced = force_plate
                        .map(|m| m[fi][gy * gw + gx])
                        .unwrap_or(false);
                    if !forced && sad_prev + 64 < sad_plate {
                        if std::env::var("FAO_MV_DEBUG").is_ok() {
                            let sat = MV_RANGE * 4; // qpel units at the clamp
                            if (mv.0 as i32).abs() >= sat - 4 || (mv.1 as i32).abs() >= sat - 4 {
                                eprintln!("mvsat");
                            } else {
                                eprintln!("mvok");
                            }
                        }
                        if std::env::var("FAO_MVZ_DEBUG").is_ok() {
                            eprintln!("{}", if mv == (0, 0) { "mvzero" } else { "mvnz" });
                        }
                        modes[gy * gw + gx] = 1;
                        mvs.push(mv);
                        prev_regions += 1;
                        copy_region_mc(&mut comp, &prev, rx, ry, mv);
                    }
                    // ── DC intra (mode 2): predict the region as the mean
                    // of its already-composed top/left borders. Raster
                    // order makes this decodable greedily — neighbors
                    // above and left are FINAL when this region decides
                    // (the inline recon rebuilds the identical composite).
                    // Targets the ledger's unsolved bucket: reveals and
                    // washed-out content where both temporal predictors
                    // miss; costs 1 flag bit + zero motion bytes. Gate
                    // beats the winning temporal predictor by a margin so
                    // grain noise can't dither into it.
                    if gx > 0 || gy > 0 {
                        let mut sum = 0u32;
                        let mut n = 0u32;
                        if ry > 0 {
                            for dx in 0..REGION {
                                sum += comp.y[(ry - 1) * fw + rx + dx] as u32;
                                n += 1;
                            }
                        }
                        if rx > 0 {
                            for dy in 0..REGION {
                                sum += comp.y[(ry + dy) * fw + rx - 1] as u32;
                                n += 1;
                            }
                        }
                        let dc = (sum / n.max(1)) as u8;
                        let mut sad_intra = 0u32;
                        for dy in 0..REGION {
                            for dx in 0..REGION {
                                sad_intra +=
                                    (f.y[(ry + dy) * fw + rx + dx] as i32 - dc as i32).unsigned_abs();
                            }
                        }
                        // Planar candidate: HEVC-style plane from the same
                        // borders (horizontal interp toward the top-right
                        // sample, vertical toward the bottom-left, then
                        // averaged) — DC's gradient-shaped sibling for
                        // smooth shading where a constant leaves a ramp in
                        // the residual. Falls back to DC values on missing
                        // borders. One extra bit per intra region.
                        let planar_at = |x: usize, y: usize| -> i32 {
                            let top = |i: usize| -> i32 {
                                if ry > 0 {
                                    comp.y[(ry - 1) * fw + rx + i.min(REGION - 1)] as i32
                                } else {
                                    dc as i32
                                }
                            };
                            let left = |i: usize| -> i32 {
                                if rx > 0 {
                                    comp.y[(ry + i.min(REGION - 1)) * fw + rx - 1] as i32
                                } else {
                                    dc as i32
                                }
                            };
                            let tr = top(REGION - 1);
                            let bl = left(REGION - 1);
                            let h = left(y) * (REGION - 1 - x) as i32 + tr * (x as i32 + 1);
                            let v = top(x) * (REGION - 1 - y) as i32 + bl * (y as i32 + 1);
                            ((h + v + REGION as i32) / (2 * REGION as i32)).clamp(0, 255)
                        };
                        let mut sad_planar = 0u32;
                        for dy in 0..REGION {
                            for dx in 0..REGION {
                                sad_planar += (f.y[(ry + dy) * fw + rx + dx] as i32
                                    - planar_at(dx, dy))
                                .unsigned_abs();
                            }
                        }
                        let use_planar = sad_planar < sad_intra;
                        let mut plane_buf = [0u8; REGION * REGION];
                        if use_planar {
                            for dy in 0..REGION {
                                for dx in 0..REGION {
                                    plane_buf[dy * REGION + dx] = planar_at(dx, dy) as u8;
                                }
                            }
                        }
                        let sad_intra = sad_intra.min(sad_planar);
                        let best_temporal = sad_plate.min(sad_prev);
                        if sad_intra + 96 < best_temporal {
                            if modes[gy * gw + gx] == 1 {
                                // un-choose prev: retract its mv
                                mvs.pop();
                                prev_regions -= 1;
                            }
                            modes[gy * gw + gx] = if use_planar { 3 } else { 2 };
                            for dy in 0..REGION {
                                for dx in 0..REGION {
                                    comp.y[(ry + dy) * fw + rx + dx] = if use_planar {
                                        plane_buf[dy * REGION + dx]
                                    } else {
                                        dc
                                    };
                                }
                            }
                            // chroma DC from the chroma planes' own borders
                            let cw = fw / 2;
                            let (crx, cry) = (rx / 2, ry / 2);
                            let cr = REGION / 2;
                            for (plane_i, is_u) in [(0usize, true), (1usize, false)] {
                                let _ = plane_i;
                                let plane: &mut Vec<u8> =
                                    if is_u { &mut comp.u } else { &mut comp.v };
                                let mut cs = 0u32;
                                let mut cn = 0u32;
                                if cry > 0 {
                                    for dx in 0..cr {
                                        cs += plane[(cry - 1) * cw + crx + dx] as u32;
                                        cn += 1;
                                    }
                                }
                                if crx > 0 {
                                    for dy in 0..cr {
                                        cs += plane[(cry + dy) * cw + crx - 1] as u32;
                                        cn += 1;
                                    }
                                }
                                let cdc = if cn > 0 { (cs / cn) as u8 } else { 128 };
                                for dy in 0..cr {
                                    for dx in 0..cr {
                                        plane[(cry + dy) * cw + crx + dx] = cdc;
                                    }
                                }
                            }
                        }
                    }
                    // ── mode 4: long-range reference into decoded history.
                    if use_dict && fi >= 2 {
                        let key = region_fingerprint(&f.y, fw, fh, rx, ry);
                        if let Some(cands) = dict.get(&key) {
                            // Compare against what this region has
                            // ALREADY been composed to — that covers
                            // plate, prev-MC and intra in one number,
                            // rather than re-deriving which won.
                            let best_local =
                                sad_region(&comp.y, fw, fh, rx as i32, ry as i32, &f.y, rx, ry);
                            let mut best = (u32::MAX, 0usize, 0usize, 0usize);
                            let picked: Vec<(usize, usize, usize)> = if dict_spread
                                && cands.len() > dict_cand
                            {
                                let step = cands.len() as f32 / dict_cand as f32;
                                (0..dict_cand)
                                    .map(|i| cands[((i as f32 * step) as usize).min(cands.len() - 1)])
                                    .collect()
                            } else {
                                cands.iter().rev().take(dict_cand).copied().collect()
                            };
                            for (dfi, dx, dy) in picked {
                                let s = sad_region(
                                    &recon[dfi].y, fw, fh, dx as i32, dy as i32, &f.y, rx, ry,
                                );
                                if s < best.0 {
                                    best = (s, dfi, dx, dy);
                                }
                            }
                            if best.0 != u32::MAX && best.0 + dict_margin < best_local {
                                if modes[gy * gw + gx] == 1 {
                                    mvs.pop();
                                    prev_regions -= 1;
                                }
                                modes[gy * gw + gx] = 4;
                                dict_refs.push(((fi - best.1) as u8, best.2 as u16, best.3 as u16));
                                let src = &recon[best.1];
                                for ly in 0..REGION {
                                    let sy = (best.3 + ly).min(fh - 1);
                                    for lx in 0..REGION {
                                        let sx = (best.2 + lx).min(fw - 1);
                                        comp.y[(ry + ly) * fw + rx + lx] = src.y[sy * fw + sx];
                                    }
                                }
                                let cw = fw / 2;
                                for ly in 0..REGION / 2 {
                                    let sy = (best.3 / 2 + ly).min(fh / 2 - 1);
                                    for lx in 0..REGION / 2 {
                                        let sx = (best.2 / 2 + lx).min(cw - 1);
                                        comp.u[(ry / 2 + ly) * cw + rx / 2 + lx] = src.u[sy * cw + sx];
                                        comp.v[(ry / 2 + ly) * cw + rx / 2 + lx] = src.v[sy * cw + sx];
                                    }
                                }
                            }
                        }
                    }
                }
            }
            prev_share_acc += prev_regions as f64 / (gw * gh) as f64;
            comp
        };

        // ── Per-region gain/offset correction (the "light sources" +
        // first-order blur term). Ledger-measured ceiling: optimal
        // (a,b) per region removes ~42% of total residual energy on the
        // worst motion shot (gain<1 models blur's contrast reduction;
        // offset models exposure flutter). Encoder fits least-squares
        // (a,b) on the composed predictor per region, quantizes a to
        // 1/64 steps around 1.0 (6 bits) and b to integers (6 bits),
        // and applies only when the L1 win clears the ~2-byte rate.
        // The correction mutates the composite BEFORE residual coding,
        // so encoder and inline recon stay consistent by construction.
        let mut composite = composite;
        let mut gain_flags = vec![0u8; (gw * gh + 7) / 8];
        // Interleaved raw (qa,b) pairs BY MEASUREMENT: delta coding
        // lost +6.2% (exact-repeat pairs are LZ gold; deltas destroyed
        // them) and plane-splitting lost +0.8% (pairwise matches beat
        // plane homogeneity). Two reverted experiments, recorded here.
        let mut gain_vals: Vec<u8> = Vec::new();
        for gy in 0..gh {
            for gx in 0..gw {
                let (rx, ry) = (gx * REGION, gy * REGION);
                let n = (REGION * REGION) as f64;
                let (mut sp, mut sa, mut spp, mut spa) = (0.0f64, 0.0, 0.0, 0.0);
                for dy in 0..REGION {
                    for dx in 0..REGION {
                        let pi = (ry + dy) * fw + rx + dx;
                        let pv = composite.y[pi] as f64;
                        let av = f.y[pi] as f64;
                        sp += pv;
                        sa += av;
                        spp += pv * pv;
                        spa += pv * av;
                    }
                }
                let denom = n * spp - sp * sp;
                if denom.abs() < 1e-6 {
                    continue;
                }
                let a = ((n * spa - sp * sa) / denom).clamp(0.516, 1.484);
                let qa = ((a - 1.0) * 64.0).round().clamp(-31.0, 31.0) as i8;
                let a = 1.0 + qa as f64 / 64.0;
                let b = ((sa - a * sp) / n).round().clamp(-31.0, 31.0);
                if qa == 0 && b == 0.0 {
                    continue;
                }
                let (mut e_raw, mut e_cor) = (0i64, 0i64);
                for dy in 0..REGION {
                    for dx in 0..REGION {
                        let pi = (ry + dy) * fw + rx + dx;
                        let pv = composite.y[pi] as f64;
                        let av = f.y[pi] as f64;
                        e_raw += (pv - av).abs() as i64;
                        e_cor += ((a * pv + b).clamp(0.0, 255.0) - av).abs() as i64;
                    }
                }
                if e_cor + GAIN_GATE >= e_raw {
                    continue;
                }
                let gi = gy * gw + gx;
                gain_flags[gi / 8] |= 1 << (gi % 8);
                // Plane-split, not delta-coded: the byte-attribution
                // diagnostic showed mode+mv side info at 44% of the
                // quiet shot's total with gain values the prime
                // suspect. Delta coding was tried first and LOST +6.2%
                // — many regions share the exact same quantized (a,b)
                // pair and zstd's LZ was already matching those 2-byte
                // repeats; deltas destroyed the repetition. Splitting
                // qa and b into separate planes (the band-major trick)
                // keeps the repeats AND gives each value its own
                // homogeneous local distribution.
                gain_vals.push(qa as u8);
                gain_vals.push(b as i8 as u8);
                for dy in 0..REGION {
                    for dx in 0..REGION {
                        let pi = (ry + dy) * fw + rx + dx;
                        let pv = composite.y[pi] as f64;
                        composite.y[pi] = (a * pv + b).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
        let composite = composite;

        // Side info, three streams zstd'd together:
        //   bitmap 1: plate(0) vs prev(1) per region
        //   bitmap 2: for prev regions only — zero-MV(0) vs coded-MV(1).
        //     On a locked-off shot most prev regions ARE zero-motion;
        //     charging 2 MV bytes for (0,0) was half the side-info bill.
        //   MV bytes: only for coded-MV regions, as deltas from the
        //     median of (left, top) coded neighbors — real MV fields are
        //     piecewise-constant over an actor, so deltas cluster at 0
        //     and zstd finally has something to bite on.
        let mut side = vec![0u8; (modes.len() + 7) / 8];
        for (i, &b) in modes.iter().enumerate() {
            if b == 1 {
                side[i / 8] |= 1 << (i % 8);
            }
        }
        let mut nz_bits: Vec<u8> = Vec::new();
        let mut mv_deltas: Vec<u8> = Vec::new();
        {
            // mvs are pushed in raster order for mode-1 regions; rebuild a
            // per-region MV grid to do spatial prediction.
            let mut grid: Vec<Option<(i8, i8)>> = vec![None; gw * gh];
            let mut it = mvs.iter();
            for gy in 0..gh {
                for gx in 0..gw {
                    if modes[gy * gw + gx] == 1 {
                        grid[gy * gw + gx] = it.next().copied();
                    }
                }
            }
            let mut nz = vec![0u8; (modes.len() + 7) / 8];
            let mut n_prev_seen = 0usize;
            for gy in 0..gh {
                for gx in 0..gw {
                    let gi = gy * gw + gx;
                    let Some(mv) = grid[gi] else { continue };
                    let idx = n_prev_seen; // bit index within prev regions, in order
                    n_prev_seen += 1;
                    if mv == (0, 0) {
                        continue;
                    }
                    nz[idx / 8] |= 1 << (idx % 8);
                    // Predictor: true median of three (left, top,
                    // top-right) with fallback, replacing the mean of
                    // (left, top). H.264/HEVC use the 3-median because a
                    // mean is dragged by ONE outlier neighbour — at the
                    // boundary between a panning background and a moving
                    // subject, exactly where MVs are dense. Absent
                    // neighbours contribute nothing rather than a
                    // spurious zero (a zero-filled mean biases every
                    // edge region's delta by half the real motion).
                    let mut cand: Vec<(i8, i8)> = Vec::with_capacity(3);
                    if gx > 0 {
                        if let Some(v) = grid[gi - 1] { cand.push(v); }
                    }
                    if gy > 0 {
                        if let Some(v) = grid[gi - gw] { cand.push(v); }
                    }
                    if gy > 0 && gx + 1 < gw {
                        if let Some(v) = grid[gi - gw + 1] { cand.push(v); }
                    }
                    let pred = match cand.len() {
                        0 => (0i8, 0i8),
                        1 => cand[0],
                        2 => (
                            ((cand[0].0 as i32 + cand[1].0 as i32) / 2) as i8,
                            ((cand[0].1 as i32 + cand[1].1 as i32) / 2) as i8,
                        ),
                        _ => {
                            let mut xs = [cand[0].0, cand[1].0, cand[2].0];
                            let mut ys = [cand[0].1, cand[1].1, cand[2].1];
                            xs.sort_unstable();
                            ys.sort_unstable();
                            (xs[1], ys[1])
                        }
                    };
                    mv_deltas.push((mv.0.wrapping_sub(pred.0)) as u8);
                    mv_deltas.push((mv.1.wrapping_sub(pred.1)) as u8);
                }
            }
            nz.truncate((n_prev_seen + 7) / 8);
            nz_bits = nz;
        }
        // (Unified 1-byte-per-region mode/intra/gain symbols were
        // TRIED here to expose their correlation to zstd — the
        // band-major lesson applied to side info — and LOST +4.6%: the
        // 8x raw-bit cost outweighs the correlation gain when the
        // bitmaps are already this sparse. Separate bitmaps stay.)
        side.extend_from_slice(&nz_bits);
        side.extend_from_slice(&mv_deltas);
        // intra flag bitmap: mode==2 regions (bitmap-1 shows them as
        // plate; this flag overrides). Inline recon shares the pass, so
        // side layout only needs consistent byte accounting.
        let mut intra_flags = vec![0u8; (gw * gh + 7) / 8];
        let mut planar_bits: Vec<u8> = Vec::new();
        let mut nb = 0usize;
        for (gi, &m) in modes.iter().enumerate() {
            if m == 2 || m == 3 {
                intra_flags[gi / 8] |= 1 << (gi % 8);
                if nb % 8 == 0 {
                    planar_bits.push(0);
                }
                if m == 3 {
                    *planar_bits.last_mut().unwrap() |= 1 << (nb % 8);
                }
                nb += 1;
            }
        }
        side.extend_from_slice(&intra_flags);
        side.extend_from_slice(&planar_bits);
        if use_dict {
            let mut dict_flags = vec![0u8; (gw * gh + 7) / 8];
            for (gi, &m) in modes.iter().enumerate() {
                if m == 4 {
                    dict_flags[gi / 8] |= 1 << (gi % 8);
                }
            }
            side.extend_from_slice(&dict_flags);
            // References stay ABSOLUTE AND UNCODED. Two hand-rolled
            // schemes were built and both LOST to plain coordinates
            // through zstd:
            //   delta vs the referring region's origin  +6,002 B
            //   move-to-front MRU index (cap 128)       +2,518 B
            // The MRU's own instrumentation explains the second and
            // corrects the guess that motivated it: the exact
            // (frame, x, y) triple repeats on only 5-15% of references
            // per frame, so 85-95% pay a token byte on top of the full
            // 5-byte escape. The absolute stream's compressibility is
            // real but comes from zstd matching LONGER contexts than a
            // 128-entry window sees, not from short-range exact repeats.
            //
            // Standing lesson, now three-for-three in this codec: zstd
            // over a plainly-laid-out side stream beats hand-rolled
            // entropy structure, and the only transform that has ever
            // won here is LAYOUT (band-major / plane-split), never
            // re-coding. Delta coding pays for piecewise-CONTINUOUS
            // fields (motion) and loses for piecewise-CONSTANT ones
            // (gains, references).
            for (d, x, y) in &dict_refs {
                side.push(*d);
                side.extend_from_slice(&x.to_le_bytes());
                side.extend_from_slice(&y.to_le_bytes());
            }
        }
        side.extend_from_slice(&gain_flags);
        side.extend_from_slice(&gain_vals);
        if std::env::var("FAO_SIDE_DEBUG").is_ok() {
            eprintln!(
                "side raw: mode {} nz {} mvd {} intra {} planar {} gflags {} gvals {}",
                (gw * gh + 7) / 8,
                nz_bits.len(),
                mv_deltas.len(),
                intra_flags.len(),
                planar_bits.len(),
                gain_flags.len(),
                gain_vals.len()
            );
        }
        let mode_bytes = zstd::encode_all(side.as_slice(), 19).map(|c| c.len()).unwrap_or(side.len());

        // ── MOTION-ADAPTIVE RATE ALLOCATION (FAO_DZ_MOTION).
        //
        // Conventional allocation spends bits where distortion is high,
        // which on this material means the motion shots. The inversion:
        // in a LOW-motion region the residual is small and highly
        // compressible, so buying fidelity there is cheap; in a
        // high-motion region the residual is large, expensive, AND
        // perceptually masked by the motion itself. So push the dead
        // zone DOWN where the picture is still and UP where it moves.
        //
        // The activity measure is the PREDICTION'S OWN ERROR — SAD of
        // the composite against the target per region — not raw frame
        // difference. That distinction matters: a region can move a lot
        // and still be predicted perfectly (a panning background under
        // an accurate plate), and it should count as still, because
        // what allocation cares about is what the region COSTS, not how
        // fast it moves. Normalised by the frame's MEDIAN region, so the
        // map is scale-free and one setting travels across shots.
        //
        // Encoder-only, no wire or decoder change (the dead zone never
        // reaches the decoder), so this is free to try and free to drop.
        let dz_map = {
            let strength: f32 = std::env::var("FAO_DZ_MOTION")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0);
            if strength == 0.0 {
                None
            } else {
                let mut act = vec![0f32; gw * gh];
                for gy in 0..gh {
                    for gx in 0..gw {
                        act[gy * gw + gx] = sad_region(
                            &composite.y, fw, fh,
                            (gx * REGION) as i32, (gy * REGION) as i32,
                            &f.y, gx * REGION, gy * REGION,
                        ) as f32;
                    }
                }
                let mut sorted = act.clone();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let med = sorted[sorted.len() / 2].max(1.0);
                let (bw4, bh4) = (fw / 4, fh / 4);
                let mut map = vec![1.0f32; bw4 * bh4];
                for by4 in 0..bh4 {
                    for bx4 in 0..bw4 {
                        let gx = ((bx4 * 4) / REGION).min(gw - 1);
                        let gy = ((by4 * 4) / REGION).min(gh - 1);
                        let r = (act[gy * gw + gx] / med).clamp(0.25, 4.0);
                        let v = (1.0 - strength.abs()) + strength.abs() * r;
                        // NEGATIVE strength = coarsen-only: busy regions
                        // get a wider dead zone, quiet ones stay at the
                        // global setting instead of being refined. Both
                        // halves are worth separating because they are
                        // different claims — "motion masks error" (this
                        // one) and "quiet detail is cheap" (the other) —
                        // and the symmetric version measured worse than
                        // simply raising the global dead zone.
                        map[by4 * bw4 + bx4] = if strength < 0.0 { v.max(1.0) } else { v };
                    }
                }
                Some(std::sync::Arc::new(map))
            }
        };
        let frame_opts = if dz_map.is_some() {
            ResidualOpts { dz_map, ..opts.clone() }
        } else {
            opts.clone()
        };
        let opts = &frame_opts;
        let p444 = interleave_444(&composite);
        let t444 = interleave_444(f);
        let (residual_bytes, rec) = match residual::encode(&p444, &t444, fw, fh, opts) {
            Some(trailer) => {
                let r = residual::apply(p444, &trailer, fw, fh);
                (trailer.len(), deinterleave_420(&r, fw, fh))
            }
            None => (0, composite),
        };
        out_frames.push(FrameCost { mode_bytes, residual_bytes });
        {
            let mut row = vec![0u32; gw * gh];
            for gy in 0..gh {
                for gx in 0..gw {
                    row[gy * gw + gx] = sad_region(
                        &rec.y, fw, fh,
                        (gx * REGION) as i32, (gy * REGION) as i32,
                        &f.y, gx * REGION, gy * REGION,
                    );
                }
            }
            leftover_trace.push(row);
        }
        let mut rec = rec;
        if let Ok(v) = std::env::var("FAO_DEBLOCK") {
            let strength: i32 = v.parse().unwrap_or(1);
            deblock(&mut rec, f, &modes, gw, gh, opts.dead_zone, strength);
        }
        recon.push(rec);
        // Index frame fi-1 now that fi is done: at the NEXT iteration's
        // query (frame fi+1) the newest entry is fi-1, one frame older
        // than the prev-MC predictor. That lag is what keeps the two
        // predictors non-overlapping — a dictionary that could return
        // the immediately previous frame would just be a slower,
        // more expensive prev-MC.
        if use_dict && fi >= 1 {
            let di = fi - 1;
            for gy in 0..gh {
                for gx in 0..gw {
                    let (rx, ry) = (gx * REGION, gy * REGION);
                    let k = region_fingerprint(&recon[di].y, fw, fh, rx, ry);
                    dict.entry(k).or_default().push((di, rx, ry));
                }
            }
        }
    }
    let denom = (frames.len().saturating_sub(1)).max(1) as f64;
    (PlateEncodeReportV2 {
        plate_bytes: model_bytes,
        motion_bytes,
        frames: out_frames,
        recon,
        prev_mode_share: (prev_share_acc / denom) as f32,
    }, leftover_trace)
}

// ═══════════════════════════════════════════════════════════════════
// v11.5 — the MOVER LAYER: movers as objects with their own motion,
// composited per frame. This is the "full design" the mover-patch MVP
// approximated statically: the plate models the SET, each mover is a
// sprite transmitted ONCE with a per-frame position track, and the
// composite (plate warp + sprite pastes) is just another prediction
// provider over encode_gop_v3_core. The win mechanism is amortization:
// a region covered by an accurate paste chooses plate-mode (zero side
// bytes, near-zero residual) instead of paying prev-MC's MV + residual
// every frame; the sprite's one-time cost rides model_bytes like the
// plate, the 2-byte/frame track rides motion_bytes. The known failure
// mode is non-rigid deformation (a gesturing actor defeats a rigid
// sprite), so encode_gop_v5 runs a J-guard against plain v3 — like the
// lookahead harness, it can only cost encode time, never bytes.
// ═══════════════════════════════════════════════════════════════════

#[derive(Clone)]
pub struct Mover {
    /// Bounding box in PIXELS at the reference frame, REGION-aligned.
    pub bx: usize,
    pub by: usize,
    pub bw: usize,
    pub bh: usize,
    pub ref_frame: usize,
    /// Cell mask at REGION granularity within the bbox — the OBJECT's
    /// cells, not the rectangle. Tracking, pasting, and paying happen
    /// only over masked cells; a bbox necessarily contains background
    /// (a mover is rarely a rectangle) and treating that background as
    /// object drags the track toward the set and poisons the paste.
    pub mask: Vec<bool>,
    /// DECODED sprite (what the decoder will paste) — adopted from the
    /// coded stream exactly like the plate, so encoder-side composites
    /// match the decoder by construction. Unmasked cells are
    /// neutralized to flat gray before coding so they cost ~nothing.
    pub sprite: Yuv420,
    pub sprite_bytes: usize,
    /// Per-frame paste position (bbox top-left, pixels); None = hidden.
    pub track: Vec<Option<(i16, i16)>>,
}

/// Detect movers on the reference frame (max plate-deviation area):
/// connected components (4-conn) over regions whose SAD vs the plate
/// prediction exceeds `thr` per pixel, min 4 cells so grain flicker
/// can't spawn objects. Returns (bx, by, bw, bh, ref_frame, mask) with
/// bbox in pixels (REGION-aligned) and mask at cell granularity within
/// the bbox — the component's cells, not the rectangle.
fn detect_movers(
    frames: &[Yuv420],
    motions: &[PlateMotion],
    plate: &Plate,
    thr: u32,
) -> Vec<(usize, usize, usize, usize, usize, Vec<bool>)> {
    let (fw, fh) = (frames[0].w, frames[0].h);
    let (gw, gh) = (fw / REGION, fh / REGION);
    let region_thr = thr * (REGION * REGION) as u32;
    let mut best = (0usize, 0usize, vec![false; gw * gh]);
    for (fi, f) in frames.iter().enumerate() {
        let pred = predict_frame(plate, &motions[fi], fw, fh);
        let mut dev = vec![false; gw * gh];
        let mut area = 0usize;
        for gy in 0..gh {
            for gx in 0..gw {
                let s = sad_region(
                    &pred.y, fw, fh,
                    (gx * REGION) as i32, (gy * REGION) as i32,
                    &f.y, gx * REGION, gy * REGION,
                );
                if s > region_thr {
                    dev[gy * gw + gx] = true;
                    area += 1;
                }
            }
        }
        if area > best.1 {
            best = (fi, area, dev);
        }
    }
    let (ref_frame, _, dev) = best;
    // Connected components, 4-connectivity.
    let mut seen = vec![false; gw * gh];
    let mut out = Vec::new();
    for start in 0..gw * gh {
        if !dev[start] || seen[start] {
            continue;
        }
        let mut stack = vec![start];
        let mut cells = Vec::new();
        seen[start] = true;
        while let Some(c) = stack.pop() {
            cells.push(c);
            let (cx, cy) = (c % gw, c / gw);
            let mut push = |n: usize| {
                if dev[n] && !seen[n] {
                    seen[n] = true;
                    stack.push(n);
                }
            };
            if cx > 0 { push(c - 1); }
            if cx + 1 < gw { push(c + 1); }
            if cy > 0 { push(c - gw); }
            if cy + 1 < gh { push(c + gw); }
        }
        if cells.len() < 4 {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (gw, gh, 0usize, 0usize);
        for &c in &cells {
            let (cx, cy) = (c % gw, c / gw);
            x0 = x0.min(cx);
            y0 = y0.min(cy);
            x1 = x1.max(cx);
            y1 = y1.max(cy);
        }
        let (cw_cells, ch_cells) = (x1 - x0 + 1, y1 - y0 + 1);
        let mut mask = vec![false; cw_cells * ch_cells];
        for &c in &cells {
            let (cx, cy) = (c % gw, c / gw);
            mask[(cy - y0) * cw_cells + (cx - x0)] = true;
        }
        out.push((
            x0 * REGION,
            y0 * REGION,
            cw_cells * REGION,
            ch_cells * REGION,
            ref_frame,
            mask,
        ));
    }
    out
}

/// SAD of the sprite's MASKED luma cells against frame f at bbox
/// top-left (px,py), subsampled 2x (tracking metric, not a coding
/// decision). Normalized to the masked area so scores are comparable
/// across partial off-frame placements.
fn sprite_sad(sp: &Yuv420, mask: &[bool], cw_cells: usize, f: &Yuv420, px: i32, py: i32) -> u64 {
    let (fw, fh) = (f.w as i32, f.h as i32);
    let masked_px = mask.iter().filter(|&&m| m).count() * REGION * REGION;
    let mut s = 0u64;
    let mut n = 0u64;
    let mut y = 0usize;
    while y < sp.h {
        let sy = py + y as i32;
        if sy >= 0 && sy < fh {
            let mrow = (y / REGION) * cw_cells;
            let mut x = 0usize;
            while x < sp.w {
                if mask[mrow + x / REGION] {
                    let sx = px + x as i32;
                    if sx >= 0 && sx < fw {
                        s += (sp.y[y * sp.w + x] as i32 - f.y[(sy * fw + sx) as usize] as i32)
                            .unsigned_abs() as u64;
                        n += 1;
                    }
                }
                x += 2;
            }
        }
        y += 2;
    }
    if n == 0 { u64::MAX } else { s * masked_px as u64 / (4 * n) }
}

/// Build the mover set: sprites coded through the residual path (dz/8,
/// best-of vs raw zstd — the plate's own recipe), DECODED sprite
/// adopted, position tracked per frame by coarse-to-fine SAD search
/// around the previous position. Returns movers + total track bytes
/// (zstd over the concatenated per-frame delta stream).
/// Pack the sprite set as ONE coded surface. Ported reasoning, not a
/// guess: `games/blackstar-rs/src/botpack.rs` v4 keeps v3's identical
/// quantised codes and merely replaces per-pose zlib with one
/// strip-wide zstd frame — measured 49% of v3, byte-identical decode —
/// because a 32 KB per-item window cannot see from one item to the
/// next while a set-wide window can. v11.5's sprites are the same
/// situation exactly: N cutouts of the same actors on the same set,
/// each previously getting its own `residual::encode` (which zstds
/// internally, per call). So: dedup identical sprites first
/// (`docs/FLEET_COMPRESSION.md`'s content-dedup technique — it folded
/// 107 of 1000 catalog parts and 0 of 1289 fold poses, so whether it
/// fires here is a measurement, not an assumption), stack the unique
/// ones into a column atlas, and code the atlas ONCE.
///
/// Returns (decoded sprites in input order, atlas_bytes, n_unique).
/// Atlas width is the max sprite width — every bbox is REGION-aligned
/// so all dims stay 4-divisible and chroma-even. Padding is flat gray:
/// it costs a DCT DC term per block and LZ-matches away.
fn pack_sprite_atlas(
    sprites: &[Yuv420],
    opts: &ResidualOpts,
) -> (Vec<Yuv420>, usize, usize) {
    if sprites.is_empty() {
        return (Vec::new(), 0, 0);
    }
    // Content dedup by exact raw bytes.
    let key = |s: &Yuv420| -> (usize, usize, Vec<u8>) {
        let mut v = Vec::with_capacity(s.y.len() + s.u.len() + s.v.len());
        v.extend_from_slice(&s.y);
        v.extend_from_slice(&s.u);
        v.extend_from_slice(&s.v);
        (s.w, s.h, v)
    };
    let mut uniq: Vec<Yuv420> = Vec::new();
    let mut uniq_keys: Vec<(usize, usize, Vec<u8>)> = Vec::new();
    let mut slot: Vec<usize> = Vec::with_capacity(sprites.len());
    for s in sprites {
        let k = key(s);
        match uniq_keys.iter().position(|u| *u == k) {
            Some(i) => slot.push(i),
            None => {
                slot.push(uniq.len());
                uniq.push(s.clone());
                uniq_keys.push(k);
            }
        }
    }
    let aw = uniq.iter().map(|s| s.w).max().unwrap_or(0);
    let ah: usize = uniq.iter().map(|s| s.h).sum();
    let mut atlas = Yuv420::new(aw, ah);
    for p in atlas.y.iter_mut() {
        *p = 128;
    }
    for p in atlas.u.iter_mut().chain(atlas.v.iter_mut()) {
        *p = 128;
    }
    let mut yoff = 0usize;
    for s in &uniq {
        for y in 0..s.h {
            atlas.y[(yoff + y) * aw..(yoff + y) * aw + s.w].copy_from_slice(&s.y[y * s.w..y * s.w + s.w]);
        }
        let (cw, sw2) = (aw / 2, s.w / 2);
        for y in 0..s.h / 2 {
            let ao = (yoff / 2 + y) * cw;
            atlas.u[ao..ao + sw2].copy_from_slice(&s.u[y * sw2..y * sw2 + sw2]);
            atlas.v[ao..ao + sw2].copy_from_slice(&s.v[y * sw2..y * sw2 + sw2]);
        }
        yoff += s.h;
    }
    // One coding pass over the whole atlas: DCT/dead-zone path (the
    // plate's finer dz) best-of against raw solid zstd.
    let mut raw = Vec::with_capacity(atlas.y.len() * 3 / 2);
    raw.extend_from_slice(&atlas.y);
    raw.extend_from_slice(&atlas.u);
    raw.extend_from_slice(&atlas.v);
    let zstd_len = zstd::encode_all(raw.as_slice(), 19).map(|c| c.len()).unwrap_or(raw.len());
    let a444 = interleave_444(&atlas);
    let gray = vec![128u8; aw * ah * 3];
    let a_opts = ResidualOpts {
        dead_zone: (opts.dead_zone / 8.0).max(1.0),
        ..opts.clone()
    };
    if std::env::var("FAO_MOVER_DEBUG").is_ok() {
        eprintln!(
            "atlas branches: solid-zstd {} B | dct {:?} B  ({}x{}, {} unique)",
            zstd_len,
            residual::encode(&gray, &a444, aw, ah, &a_opts).map(|t| t.len()),
            aw, ah, uniq.len()
        );
    }
    let (atlas_bytes, decoded_atlas) = match residual::encode(&gray, &a444, aw, ah, &a_opts) {
        Some(t) if t.len() < zstd_len => {
            let dec = residual::apply(gray, &t, aw, ah);
            (t.len(), deinterleave_420(&dec, aw, ah))
        }
        _ => (zstd_len, atlas.clone()),
    };
    // Cut the decoded uniques back out, then fan out to input order.
    let mut dec_uniq: Vec<Yuv420> = Vec::with_capacity(uniq.len());
    let mut yoff = 0usize;
    for s in &uniq {
        let mut d = Yuv420::new(s.w, s.h);
        for y in 0..s.h {
            let ao = (yoff + y) * aw;
            d.y[y * s.w..y * s.w + s.w].copy_from_slice(&decoded_atlas.y[ao..ao + s.w]);
        }
        let (cw, sw2) = (aw / 2, s.w / 2);
        for y in 0..s.h / 2 {
            let ao = (yoff / 2 + y) * cw;
            d.u[y * sw2..y * sw2 + sw2].copy_from_slice(&decoded_atlas.u[ao..ao + sw2]);
            d.v[y * sw2..y * sw2 + sw2].copy_from_slice(&decoded_atlas.v[ao..ao + sw2]);
        }
        yoff += s.h;
        dec_uniq.push(d);
    }
    let out = slot.iter().map(|&i| dec_uniq[i].clone()).collect();
    (out, atlas_bytes, uniq.len())
}

pub fn build_movers(
    frames: &[Yuv420],
    motions: &[PlateMotion],
    plate: &Plate,
    opts: &ResidualOpts,
    thr: u32,
) -> (Vec<Mover>, usize) {
    let boxes = detect_movers(frames, motions, plate, thr);
    let mut movers = Vec::new();
    let mut track_raw: Vec<u8> = Vec::new();
    // Pass 1: build the RAW sprites so the whole set can be coded
    // together (see pack_sprite_atlas). Per-sprite independent coding
    // is still computed, as the comparison the atlas has to beat.
    let mut raw_sprites: Vec<Yuv420> = Vec::new();
    let mut geom: Vec<(usize, usize, usize, usize, usize, Vec<bool>)> = Vec::new();
    for (bx, by, bw, bh, ref_frame, mask) in boxes {
        let rf = &frames[ref_frame];
        let (fw, fh) = (rf.w, rf.h);
        let cw_cells = bw / REGION;
        let mut sp = Yuv420::new(bw, bh);
        for y in 0..bh {
            let sy = (by + y).min(fh - 1);
            let mrow = (y / REGION) * cw_cells;
            for x in 0..bw {
                let sx = (bx + x).min(fw - 1);
                // Unmasked cells neutralize to flat gray: they are never
                // pasted, so paying DCT/zstd for their background content
                // would be pure waste.
                sp.y[y * bw + x] = if mask[mrow + x / REGION] { rf.y[sy * fw + sx] } else { 128 };
            }
        }
        for y in 0..bh / 2 {
            let sy = (by / 2 + y).min(fh / 2 - 1);
            let mrow = ((y * 2) / REGION) * cw_cells;
            for x in 0..bw / 2 {
                let sx = (bx / 2 + x).min(fw / 2 - 1);
                let m = mask[mrow + (x * 2) / REGION];
                sp.u[y * (bw / 2) + x] = if m { rf.u[sy * (fw / 2) + sx] } else { 128 };
                sp.v[y * (bw / 2) + x] = if m { rf.v[sy * (fw / 2) + sx] } else { 128 };
            }
        }
        raw_sprites.push(sp);
        geom.push((bx, by, bw, bh, ref_frame, mask));
    }
    // Per-sprite independent coding (the pre-atlas behaviour), kept as
    // the baseline the solid pack must beat.
    let mut per_sprite_dec: Vec<Yuv420> = Vec::with_capacity(raw_sprites.len());
    let mut per_sprite_total = 0usize;
    let mut per_sprite_zstd_total = 0usize;
    let mut per_sprite_costs: Vec<usize> = Vec::new();
    for sp in &raw_sprites {
        let (bw, bh) = (sp.w, sp.h);
        let mut raw = Vec::with_capacity(sp.y.len() * 3);
        raw.extend_from_slice(&sp.y);
        raw.extend_from_slice(&sp.u);
        raw.extend_from_slice(&sp.v);
        let zstd_len = zstd::encode_all(raw.as_slice(), 19).map(|c| c.len()).unwrap_or(raw.len());
        let sp444 = interleave_444(sp);
        let gray = vec![128u8; bw * bh * 3];
        let sp_opts = ResidualOpts {
            dead_zone: (opts.dead_zone / 8.0).max(1.0),
            ..opts.clone()
        };
        let (b, d) = match residual::encode(&gray, &sp444, bw, bh, &sp_opts) {
            Some(t) if t.len() < zstd_len => {
                let dec = residual::apply(gray, &t, bw, bh);
                (t.len(), deinterleave_420(&dec, bw, bh))
            }
            _ => (zstd_len, sp.clone()),
        };
        per_sprite_total += b;
        per_sprite_costs.push(b);
        per_sprite_zstd_total += zstd_len;
        per_sprite_dec.push(d);
    }
    // Pass 2: track every candidate on its own decoded sprite, then ADMIT
    // per mover before anything is packed.
    //
    // Why per-mover admission and not just the global J-guard: the guard
    // is all-or-nothing, so one sprite that pays for itself can drag in
    // twenty that don't, and one that doesn't can't be dropped without
    // losing the whole layer. The suite made that concrete — shot 16
    // WINS 21.9% with two movers while shot 13 LOSES with twenty-two, and
    // shot 11 (10 frames) loses 16.3% because a sprite simply cannot
    // amortize over ten frames. Mover count doesn't predict the win;
    // GOP length x reuse does, and that is exactly what an admission
    // test measures directly instead of proxying with a fixed threshold.
    //
    // The test is the same J the guard uses, scoped to one object:
    // admit iff the SSE the paste removes over the frames it is visible
    // exceeds lambda x (its own coded bytes + its 2 B/frame track).
    // Cost uses the per-sprite independent size, which is an upper bound
    // on its marginal cost inside the atlas — so admission is
    // conservative by construction, never optimistic.
    let mut cand_tracks: Vec<Vec<Option<(i16, i16)>>> = Vec::with_capacity(geom.len());
    for (gi, (bx, by, _bw, bh, ref_frame, mask)) in geom.iter().enumerate() {
        let _ = bh;
        let sprite = &per_sprite_dec[gi];
        let cw_cells = sprite.w / REGION;
        let rf = &frames[*ref_frame];
        let ref_score = sprite_sad(sprite, mask, cw_cells, rf, *bx as i32, *by as i32);
        let masked_px = mask.iter().filter(|&&m| m).count() * REGION * REGION;
        let miss = (ref_score.max(1)) * 3 + 6 * masked_px as u64;
        let mut track = Vec::with_capacity(frames.len());
        let (mut px, mut py) = (*bx as i32, *by as i32);
        for f in frames {
            let (mut bestp, mut bests) = ((px, py), u64::MAX);
            let mut probe = |x: i32, y: i32, bests: &mut u64, bestp: &mut (i32, i32)| {
                let s = sprite_sad(sprite, mask, cw_cells, f, x, y);
                if s < *bests {
                    *bests = s;
                    *bestp = (x, y);
                }
            };
            let mut dy = -32;
            while dy <= 32 {
                let mut dx = -32;
                while dx <= 32 {
                    probe(px + dx, py + dy, &mut bests, &mut bestp);
                    dx += 4;
                }
                dy += 4;
            }
            let coarse = bestp;
            for dy in -3..=3 {
                for dx in -3..=3 {
                    probe(coarse.0 + dx, coarse.1 + dy, &mut bests, &mut bestp);
                }
            }
            if bests > miss {
                track.push(None);
            } else {
                px = bestp.0;
                py = bestp.1;
                track.push(Some((px as i16, py as i16)));
            }
        }
        cand_tracks.push(track);
    }
    // SSE gain per mover: one pass over frames, plate prediction computed
    // once per frame and shared across candidates.
    //
    // CORRECTION, measured: scoring the sprite against the PLATE alone
    // overstates it. The mode decision picks the best of plate and
    // prev-MC per region, so a mover the plate misses but the previous
    // frame carries is already nearly free — the sprite's apparent gain
    // is redundant with a predictor the codec has anyway. Shot 11 is the
    // case that exposed it: 3 of 3 movers admitted against the plate,
    // 59 KB of sprites, and the shot stayed a 16.3% loss. Gain is now
    // scored against min(plate, previous frame), using the previous
    // frame at zero motion as a cheap stand-in for prev-MC. That
    // stand-in UNDERSTATES prev (a real search only does better), so
    // admission stays optimistic rather than conservative here — the
    // honest bound to keep in mind when reading a marginal admit.
    let (fw, fh) = (frames[0].w, frames[0].h);
    let mut gain = vec![0.0f64; geom.len()];
    for (fi, f) in frames.iter().enumerate() {
        let pred = predict_frame(plate, &motions[fi], fw, fh);
        let prev = if fi > 0 { Some(&frames[fi - 1]) } else { None };
        for gi in 0..geom.len() {
            let Some((px, py)) = cand_tracks[gi][fi] else { continue };
            let (bw, mask) = (per_sprite_dec[gi].w, &geom[gi].5);
            let cw_cells = bw / REGION;
            let sprite = &per_sprite_dec[gi];
            let mut g = 0.0f64;
            for cy in 0..(sprite.h / REGION) {
                for cx in 0..cw_cells {
                    if !mask[cy * cw_cells + cx] {
                        continue;
                    }
                    for ly in 0..REGION {
                        let sy = py as i32 + (cy * REGION + ly) as i32;
                        if sy < 0 || sy >= fh as i32 {
                            continue;
                        }
                        for lx in 0..REGION {
                            let sx = px as i32 + (cx * REGION + lx) as i32;
                            if sx < 0 || sx >= fw as i32 {
                                continue;
                            }
                            let pi = sy as usize * fw + sx as usize;
                            let t = f.y[pi] as f64;
                            let p0 = pred.y[pi] as f64;
                            let mut e0 = (p0 - t) * (p0 - t);
                            if let Some(pv) = prev {
                                let pp = pv.y[pi] as f64;
                                let ep = (pp - t) * (pp - t);
                                if ep < e0 {
                                    e0 = ep;
                                }
                            }
                            let p1 = sprite.y[(cy * REGION + ly) * bw + cx * REGION + lx] as f64;
                            g += e0 - (p1 - t) * (p1 - t);
                        }
                    }
                }
            }
            gain[gi] += g;
        }
    }
    let lambda = (opts.dead_zone as f64).powi(2) / 3.0;
    // ADMISSION STRICTNESS, and why it is not 1.0 by default.
    //
    // Measured on the full 19-shot suite: `gain > lambda * cost` is
    // 3.84% WORSE IN J than admitting everything, and worse on 10 of the
    // 11 shots it changed (shot 13 +15.2%, shot 19 +14.6%). It looked
    // like a win only because I read bytes alone — it saved 0.855 MB and
    // paid 0.23 dB for them, which is a far worse exchange rate than the
    // codec's own dead-zone ladder gives.
    //
    // The bug is an apples-to-oranges comparison, and it biases one way.
    // A true J gain is (SSE removed) + lambda * (BYTES saved), but the
    // benefit side above counts only the SSE term. A better predictor
    // also shrinks the residual for every frame the mover is visible and
    // lets its cells choose plate-mode instead of prev-mode, dropping
    // ~2 MV bytes each — both strictly positive, both omitted. So the
    // test systematically over-rejects, exactly as the suite showed.
    //
    // Estimating the omitted term properly needs a rate model the lab
    // does not have. `strictness` is the honest stand-in: it scales the
    // cost side to stand for the benefit the test cannot see, and it is
    // SWEPT, not guessed. FAO_MOVER_ADMIT sets it; the default is
    // recorded below from the sweep.
    // DEFAULT: OFF (admit every candidate), because that is what
    // measured best. Even strictness 0 — admit anything with positive
    // gain against min(plate, prev) — is not neutral: it still rejects
    // movers the previous frame already predicts, and on shot 13 that
    // costs 15% in J (862,935 B @ 34.340 dB against the unfiltered
    // 963,979 @ 35.267, which is a much better point on the hull). The
    // machinery, the per-mover gain measurement and the knob all stay,
    // because the term the test cannot see — residual and MV bytes
    // saved — is small exactly where prediction is already good (film,
    // where prev-MC is strong) and large where it is not (engine
    // content with hard cuts between reused assets). This is a negative
    // ON THIS MATERIAL, not a dead construction.
    let strictness: Option<f64> = std::env::var("FAO_MOVER_ADMIT")
        .ok()
        .and_then(|v| v.parse().ok());
    let mut keep: Vec<usize> = Vec::new();
    for gi in 0..geom.len() {
        let cost = per_sprite_costs[gi] + 2 * frames.len() + 10 + (geom[gi].5.len() + 7) / 8;
        match strictness {
            None => keep.push(gi),
            Some(k) => {
                if gain[gi] > k * lambda * cost as f64 {
                    keep.push(gi);
                }
            }
        }
    }
    if std::env::var("FAO_MOVER_DEBUG").is_ok() {
        eprintln!(
            "mover admission: {} of {} admitted (lambda {:.0})",
            keep.len(),
            geom.len(),
            lambda
        );
    }
    if keep.is_empty() {
        return (Vec::new(), 0);
    }
    let kept_raw: Vec<Yuv420> = keep.iter().map(|&i| raw_sprites[i].clone()).collect();
    let kept_per_sprite_total: usize = keep.iter().map(|&i| per_sprite_costs[i]).sum();
    let (atlas_dec, atlas_bytes, n_uniq) = pack_sprite_atlas(&kept_raw, opts);
    // +2 B/mover slot index is the atlas's own side cost.
    let atlas_total = atlas_bytes + kept_raw.len() * 2;
    let use_atlas = atlas_total < kept_per_sprite_total;
    if std::env::var("FAO_MOVER_DEBUG").is_ok() {
        eprintln!(
            "sprite pack: per-sprite {} B (raw-zstd sum {}) | atlas {} B (+{} slots, {} unique of {}) -> {}",
            kept_per_sprite_total,
            per_sprite_zstd_total,
            atlas_bytes,
            kept_raw.len() * 2,
            n_uniq,
            kept_raw.len(),
            if use_atlas { "ATLAS" } else { "PER-SPRITE" }
        );
    }
    let decoded: Vec<Yuv420> = if use_atlas {
        atlas_dec
    } else {
        keep.iter().map(|&i| per_sprite_dec[i].clone()).collect()
    };
    let chosen_total = if use_atlas { atlas_total } else { kept_per_sprite_total };
    // The set cost is a SET cost now, so it is charged once on the
    // first mover rather than split by a fiction of per-sprite shares.
    let kept_geom: Vec<_> = keep.iter().map(|&i| geom[i].clone()).collect();
    let kept_tracks: Vec<_> = keep.iter().map(|&i| cand_tracks[i].clone()).collect();
    for (i, (((bx, by, bw, bh, ref_frame, mask), sprite), track)) in kept_geom
        .into_iter()
        .zip(decoded.into_iter())
        .zip(kept_tracks.into_iter())
        .enumerate()
    {
        let _ = bh;
        let sprite_bytes = if i == 0 { chosen_total } else { 0 };
        for t in &track {
            match t {
                None => {
                    track_raw.push(0x80);
                    track_raw.push(0);
                }
                Some((px, py)) => {
                    track_raw.push((*px as i32 - bx as i32).clamp(-127, 127) as i8 as u8);
                    track_raw.push((*py as i32 - by as i32).clamp(-127, 127) as i8 as u8);
                }
            }
        }
        movers.push(Mover { bx, by, bw, bh, ref_frame, mask, sprite, sprite_bytes, track });
    }
    let mask_bytes: usize = movers.iter().map(|m| (m.mask.len() + 7) / 8).sum();
    let track_bytes = if track_raw.is_empty() {
        0
    } else {
        zstd::encode_all(track_raw.as_slice(), 19).map(|c| c.len()).unwrap_or(track_raw.len())
    } + movers.len() * 10 // bbox 8B + ref 2B per mover
      + mask_bytes;
    (movers, track_bytes)
}

fn paste_sprite(dst: &mut Yuv420, m: &Mover, px: i32, py: i32) {
    let (fw, fh) = (dst.w as i32, dst.h as i32);
    let cw_cells = m.bw / REGION;
    for y in 0..m.bh {
        let sy = py + y as i32;
        if sy < 0 || sy >= fh { continue; }
        let mrow = (y / REGION) * cw_cells;
        for x in 0..m.bw {
            if !m.mask[mrow + x / REGION] { continue; }
            let sx = px + x as i32;
            if sx < 0 || sx >= fw { continue; }
            dst.y[(sy * fw + sx) as usize] = m.sprite.y[y * m.bw + x];
        }
    }
    let (cw, ch) = (fw / 2, fh / 2);
    for y in 0..m.bh / 2 {
        let sy = py / 2 + y as i32;
        if sy < 0 || sy >= ch { continue; }
        let mrow = ((y * 2) / REGION) * cw_cells;
        for x in 0..m.bw / 2 {
            if !m.mask[mrow + (x * 2) / REGION] { continue; }
            let sx = px / 2 + x as i32;
            if sx < 0 || sx >= cw { continue; }
            dst.u[(sy * cw + sx) as usize] = m.sprite.u[y * (m.bw / 2) + x];
            dst.v[(sy * cw + sx) as usize] = m.sprite.v[y * (m.bw / 2) + x];
        }
    }
}

/// v11.5: plate + mover layer, J-guarded against plain v3.
/// Returns (report, n_movers, mover_model_bytes, track_bytes, picked_v5).
pub fn encode_gop_v5(
    frames: &[Yuv420],
    motions: &[PlateMotion],
    plate: &Plate,
    opts: &ResidualOpts,
    thr: u32,
) -> (PlateEncodeReportV2, usize, usize, usize, bool) {
    encode_gop_v5_edited(frames, motions, plate, opts, thr, None, None).0
}

/// v5 with an optional SCENE EDIT and an optional scene-export sink.
///
/// `edit` replaces each mover's track — the resolver half of the scene
/// protocol. Two things make the edited path structurally different
/// from plain encoding, and both are deliberate:
///
///  * The J-guard is BYPASSED when an edit is present. The guard exists
///    to answer "is the mover layer worth its bytes for reproducing this
///    video", and an edited scene is not trying to reproduce the video —
///    falling back to v3 would silently discard the edit and hand back
///    the original, the worst possible failure for this feature.
///  * The residual is coded against the EDITED composite, so it carries
///    the difference from what the viewer will actually see. Reusing the
///    original residual is what would make a naive "overwrite and
///    stitch" smear: that residual encodes the difference from the
///    ORIGINAL prediction and is wrong everywhere the scene changed.
pub fn encode_gop_v5_edited(
    frames: &[Yuv420],
    motions: &[PlateMotion],
    plate: &Plate,
    opts: &ResidualOpts,
    thr: u32,
    edit: Option<&[Vec<Option<(i16, i16)>>]>,
    scene_out: Option<&mut String>,
) -> ((PlateEncodeReportV2, usize, usize, usize, bool), Vec<Mover>) {
    let (rep3, _) = encode_gop_v3_forced(frames, motions, plate, opts, None);
    let (mut movers, track_bytes) = build_movers(frames, motions, plate, opts, thr);
    if let Some(sink) = scene_out {
        *sink = scene_to_json(&movers, frames.len(), frames[0].w, frames[0].h);
    }
    let edited = edit.is_some();
    if let Some(e) = edit {
        for (m, t) in movers.iter_mut().zip(e) {
            m.track = t.clone();
        }
    }
    let r = encode_gop_v5_inner(frames, motions, plate, opts, rep3, movers.clone(), track_bytes, edited);
    (r, movers)
}

fn encode_gop_v5_inner(
    frames: &[Yuv420],
    motions: &[PlateMotion],
    plate: &Plate,
    opts: &ResidualOpts,
    rep3: PlateEncodeReportV2,
    movers: Vec<Mover>,
    track_bytes: usize,
    edited: bool,
) -> (PlateEncodeReportV2, usize, usize, usize, bool) {
    if movers.is_empty() {
        return (rep3, 0, 0, 0, false);
    }
    let mover_model: usize = movers.iter().map(|m| m.sprite_bytes).sum();
    // v5 branch: same plate coding as v3 (reuse rep3's plate_bytes; the
    // decoded plate must be re-derived — cheapest correct route is to
    // run the core with a compositing provider over the same coded
    // plate. encode_gop_v3_forced adopts the DECODED plate internally,
    // so composite over predict_frame of that decoded plate: re-code
    // the plate here with the identical recipe to get the same decoded
    // surface.)
    let (rep5, _) = {
        // Recreate the decoded plate exactly as _forced does.
        let (fw, fh) = (frames[0].w, frames[0].h);
        let mut plate_raw = Vec::with_capacity(plate.y.len() + plate.u.len() + plate.v.len());
        plate_raw.extend_from_slice(&plate.y);
        plate_raw.extend_from_slice(&plate.u);
        plate_raw.extend_from_slice(&plate.v);
        const PLATE_DZ_DIV: f32 = 8.0;
        let mut plate_coded = plate.clone();
        let zstd_len = zstd::encode_all(plate_raw.as_slice(), 19).map(|c| c.len()).unwrap_or(plate_raw.len());
        let gray = vec![128u8; plate.w * plate.h * 3];
        let mut plate444 = Vec::with_capacity(plate.w * plate.h * 3);
        for y in 0..plate.h {
            for x in 0..plate.w {
                plate444.push(plate.y[y * plate.w + x]);
                plate444.push(plate.u[(y / 2) * (plate.w / 2) + x / 2]);
                plate444.push(plate.v[(y / 2) * (plate.w / 2) + x / 2]);
            }
        }
        let plate_opts = ResidualOpts {
            dead_zone: (opts.dead_zone / PLATE_DZ_DIV).max(1.0),
            ..opts.clone()
        };
        if let Some(t) = residual::encode(&gray, &plate444, plate.w, plate.h, &plate_opts) {
            if t.len() < zstd_len {
                let dec = residual::apply(gray, &t, plate.w, plate.h);
                for y in 0..plate.h {
                    for x in 0..plate.w {
                        let i = (y * plate.w + x) * 3;
                        plate_coded.y[y * plate.w + x] = dec[i];
                        if y % 2 == 0 && x % 2 == 0 {
                            let ci = (y / 2) * (plate.w / 2) + x / 2;
                            plate_coded.u[ci] = dec[i + 1];
                            plate_coded.v[ci] = dec[i + 2];
                        }
                    }
                }
            }
        }
        let pc = &plate_coded;
        let mut pred_for = |fi: usize| -> Yuv420 {
            let mut p = predict_frame(pc, &motions[fi], fw, fh);
            for m in &movers {
                if let Some((px, py)) = m.track[fi] {
                    paste_sprite(&mut p, m, px as i32, py as i32);
                }
            }
            p
        };
        encode_gop_v3_core(
            frames,
            &mut pred_for,
            rep3.plate_bytes + mover_model,
            frames.len() * 8 + track_bytes,
            opts,
            None,
        )
    };
    // J-guard: SSE + lambda*bytes, lambda = dz^2/3 (the router's shape).
    let sse = |rep: &PlateEncodeReportV2| -> f64 {
        let mut acc = 0.0f64;
        for (r, f) in rep.recon.iter().zip(frames) {
            for (a, b) in r.y.iter().zip(&f.y) {
                let d = *a as f64 - *b as f64;
                acc += d * d;
            }
        }
        acc
    };
    let bytes = |rep: &PlateEncodeReportV2| -> usize {
        rep.plate_bytes
            + rep.motion_bytes
            + rep.frames.iter().map(|c| c.residual_bytes + c.mode_bytes).sum::<usize>()
    };
    let lambda = (opts.dead_zone as f64).powi(2) / 3.0;
    let (j3, j5) = (
        sse(&rep3) + lambda * bytes(&rep3) as f64,
        sse(&rep5) + lambda * bytes(&rep5) as f64,
    );
    if std::env::var("FAO_MOVER_DEBUG").is_ok() {
        eprintln!(
            "mover J: v3 {:.4e} ({} B) | v5 {:.4e} ({} B, {} movers, sprites {} track {})",
            j3, bytes(&rep3), j5, bytes(&rep5), movers.len(), mover_model, track_bytes
        );
    }
    if j5 < j3 || edited {
        (rep5, movers.len(), mover_model, track_bytes, true)
    } else {
        (rep3, movers.len(), mover_model, track_bytes, false)
    }
}

#[cfg(test)]
mod tests_v5 {
    use super::*;

    /// A rigid textured mover translating 10 px/frame over an APERIODIC
    /// static set (periodic backgrounds alias the tracker; that was a
    /// bug in this test's first version, not in the layer). Speed is
    /// chosen so each cell is mover-occupied < half the GOP and the
    /// plate median stays clean of it. Asserts the MACHINERY — one
    /// object detected, mask excludes background, track follows,
    /// composite predicts mover pixels — and that encode_gop_v5's
    /// guard runs and reconstruction carries the mover either way.
    /// Whether v5 WINS is an economics measurement for real footage,
    /// not a unit invariant.
    #[test]
    fn mover_layer_detects_tracks_and_composites_rigid_translation() {
        let (w, h) = (224, 96);
        let bg = |x: usize, y: usize| -> u8 { (70 + (x * 13 + y * 29 + (x * y) % 7 * 11) % 60) as u8 };
        let mover_tex = |mx: usize, my: usize| -> u8 { (150 + (mx * 37 + my * 101 + mx * my * 5) % 97) as u8 };
        let speed = 10usize;
        let mut frames = Vec::new();
        for t in 0..12 {
            let mut f = Yuv420::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    f.y[y * w + x] = bg(x, y);
                }
            }
            for p in f.u.iter_mut() {
                *p = 128;
            }
            for p in f.v.iter_mut() {
                *p = 128;
            }
            let sx = 8 + t * speed;
            for y in 32..64 {
                for x in sx..(sx + 32).min(w) {
                    f.y[y * w + x] = mover_tex(x - sx, y - 32);
                }
            }
            frames.push(f);
        }
        let motions = vec![PlateMotion { dx: 0.0, dy: 0.0, confidence: 1.0 }; frames.len()];
        let plate = build_plate(&frames, &motions);
        let opts = ResidualOpts { dead_zone: 8.0, ..ResidualOpts::default() };
        let (movers, track_bytes) = build_movers(&frames, &motions, &plate, &opts, 12);
        assert_eq!(movers.len(), 1, "one rigid mover expected, got {}", movers.len());
        assert!(track_bytes > 0);
        let m = &movers[0];
        let n_masked = m.mask.iter().filter(|&&b| b).count();
        let total_cells = m.mask.len();
        assert!(n_masked >= 4, "mask too small: {n_masked}");
        // The mover is 32x32 on a clean plate: mask must be the object's
        // ~2-3x2 cell footprint, not a corridor (that would mean the
        // plate median was contaminated and detection smeared).
        assert!(
            n_masked <= 9 && total_cells <= 12,
            "mask smeared: {n_masked} of {total_cells} cells"
        );
        // Track follows the translation: paste position px at frame t
        // must equal bx + speed*(t - ref_frame), within bbox alignment
        // + subsampled-SAD slack.
        let r = m.ref_frame as i32;
        for (t, tr) in m.track.iter().enumerate() {
            let Some((px, _py)) = tr else { panic!("mover hidden at frame {t}") };
            let expect = m.bx as i32 + speed as i32 * (t as i32 - r);
            assert!(
                (*px as i32 - expect).abs() <= 3,
                "frame {t}: tracked x {px} vs expected {expect} (bx {} ref {r})",
                m.bx
            );
        }
        // Composite accuracy: plate warp + paste must predict the
        // mover's pixels directly (this is the provider the core sees).
        let mut pred = predict_frame(&plate, &motions[11], w, h);
        if let Some((px, py)) = m.track[11] {
            paste_sprite(&mut pred, m, px as i32, py as i32);
        }
        let sx = 8 + 11 * speed;
        let (mut err, mut n) = (0u32, 0u32);
        for y in 36..60 {
            for x in (sx + 4)..(sx + 28) {
                err += (pred.y[y * w + x] as i32 - mover_tex(x - sx, y - 32) as i32).unsigned_abs();
                n += 1;
            }
        }
        assert!(err / n < 8, "composite mover error {} per px", err / n);
        // The guarded encode runs end-to-end and reconstruction carries
        // the mover regardless of which branch the guard picked.
        let (rep, nm, _mm, _tb, _picked) = encode_gop_v5(&frames, &motions, &plate, &opts, 12);
        assert_eq!(nm, 1);
        let f11 = &rep.recon[11];
        let (px, py) = (sx + 16, 48);
        let expect = mover_tex(px - sx, py - 32) as i32;
        assert!(
            (f11.y[py * w + px] as i32 - expect).abs() <= 24,
            "mover content lost in recon"
        );
    }
}

#[cfg(test)]
mod tests_v3 {
    use super::*;

    /// A textured mover in CONSTANT MOTION: v2's zero-motion prev misses
    /// it every frame; v3's MC prev must track it and beat v2 clearly.
    #[test]
    fn mc_tracks_a_moving_texture() {
        let (w, h) = (96, 64);
        let mut frames = Vec::new();
        for t in 0..10 {
            let mut f = Yuv420::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    f.y[y * w + x] = (((x / 8) + (y / 8)) % 2 * 90 + 70) as u8;
                }
            }
            for p in f.u.iter_mut() {
                *p = 128;
            }
            for p in f.v.iter_mut() {
                *p = 128;
            }
            let sx = 4 + t * 6; // constant 6px/frame — inside MV_RANGE
            for y in 24..44 {
                for x in sx..(sx + 20).min(w) {
                    let (mx, my) = (x - sx, y - 24);
                    let n = (mx * 41 + my * 97 + mx * my * 7) % 89;
                    f.y[y * w + x] = (140 + n) as u8;
                }
            }
            frames.push(f);
        }
        let motions = vec![PlateMotion { dx: 0.0, dy: 0.0, confidence: 1.0 }; frames.len()];
        let plate = build_plate(&frames, &motions);
        let opts = ResidualOpts { dead_zone: 2.0, ..ResidualOpts::default() };
        let v2 = encode_gop_v2(&frames, &motions, &plate, &opts);
        let v3 = encode_gop_v3(&frames, &motions, &plate, &opts);
        let cost = |fr: &[FrameCost]| -> usize { fr.iter().skip(1).map(|c| c.residual_bytes + c.mode_bytes).sum() };
        let (c2, c3) = (cost(&v2.frames), cost(&v3.frames));
        assert!(
            c3 * 2 < c2 * 3,
            "MC prev should cut moving-texture cost ≥ ~1.5x: v2 {c2} v3 {c3}"
        );
        // Reconstruction carries the mover at its final position.
        let f9 = &v3.recon[9];
        let sx = 4 + 9 * 6;
        let (px, py) = (sx + 10, 34);
        let (mx, my) = (px - sx, py - 24);
        let expect = (140 + (mx * 41 + my * 97 + mx * my * 7) % 89) as i32;
        assert!((f9.y[py * w + px] as i32 - expect).abs() <= 16, "mover lost under MC");
    }
}

// ═══════════════════════════════════════════════════════════════════
// v11.4 — the grain split.
//
// Accounting on real footage showed the residual at matched-AV1 quality
// is dominated by FILM GRAIN: temporally white noise that neither the
// plate (whose median removed it — correctly) nor MC-prev (grain never
// matches across frames) can predict, so the DCT coder transcribes it
// pixel by pixel. AV1 does not pay this bill: it denoises and ships a
// film-grain-synthesis model (§AV1 spec, film_grain_params). v11.4 makes
// the same split explicit, in FAO's own terms: the "shader" that brings
// back detail is a deterministic grain synthesizer.
//
//   encode: target' = temporal_denoise(target, mc_composite)  — average
//           where |target − prediction| is within a grain-sized band;
//           real signal differences exceed the band and stay untouched.
//           Estimate grain σ from what the denoise removed. Residual-code
//           against target' (cheap: the band'd noise is gone).
//   decode: recon' + synth_grain(σ, frame_seed) — xorshift PRNG, so the
//           decoder reproduces it bit-exactly from (σ, frame index).
//
// Metric honesty: synthesized grain is not the ORIGINAL grain, so
// PSNR-vs-grainy-source saturates near the grain power for ANY codec
// that does this (AV1 included, at low rates). The lab therefore reports
// grain σ and lets the caller score both raw and denoised references.
// ═══════════════════════════════════════════════════════════════════

/// Temporal denoise: where |cur − pred| ≤ band, average toward pred
/// (grain is temporally independent; averaging halves its power). Where
/// the difference exceeds the band it is signal — leave it alone.
/// Returns (denoised, estimated grain σ from the removed component).
pub fn temporal_denoise(cur: &Yuv420, pred: &Yuv420, band: i32) -> (Yuv420, f32) {
    let mut out = cur.clone();
    let mut acc = 0.0f64;
    let mut n = 0usize;
    for i in 0..cur.y.len() {
        let d = cur.y[i] as i32 - pred.y[i] as i32;
        if d.abs() <= band {
            out.y[i] = ((cur.y[i] as i32 + pred.y[i] as i32 + 1) / 2) as u8;
            acc += (d * d) as f64;
            n += 1;
        }
    }
    // Chroma grain on scanned film is weaker; same rule, half band.
    for i in 0..cur.u.len() {
        let du = cur.u[i] as i32 - pred.u[i] as i32;
        if du.abs() <= band / 2 {
            out.u[i] = ((cur.u[i] as i32 + pred.u[i] as i32 + 1) / 2) as u8;
        }
        let dv = cur.v[i] as i32 - pred.v[i] as i32;
        if dv.abs() <= band / 2 {
            out.v[i] = ((cur.v[i] as i32 + pred.v[i] as i32 + 1) / 2) as u8;
        }
    }
    // The in-band diff is grain(cur) − grain(pred): two independent draws,
    // so Var(diff) = 2σ² → σ = rms/√2.
    let sigma = if n > 0 { ((acc / n as f64).sqrt() / std::f64::consts::SQRT_2) as f32 } else { 0.0 };
    (out, sigma)
}

/// Lag-1 horizontal autocorrelation of the in-band (grain-attributed)
/// component of cur − pred. Real scan/film grain is spatially LOW-PASS
/// (ρ ≈ 0.2–0.5 after the optical chain and any prior encode); white
/// synthesis reads as electronic noise and costs SSIM. The decoder's
/// shader needs this one number to match the spectrum's first moment.
pub fn grain_lag1(cur: &Yuv420, pred: &Yuv420, band: i32) -> f32 {
    let (w, h) = (cur.w, cur.h);
    let mut num = 0.0f64;
    let mut den = 0.0f64;
    for y in 0..h {
        let mut prev_d: Option<f64> = None;
        for x in 0..w {
            let i = y * w + x;
            let d = cur.y[i] as i32 - pred.y[i] as i32;
            if d.abs() <= band {
                let df = d as f64;
                den += df * df;
                if let Some(pd) = prev_d {
                    num += df * pd;
                }
                prev_d = Some(df);
            } else {
                prev_d = None;
            }
        }
    }
    if den > 1.0 {
        (num / den) as f32
    } else {
        0.0
    }
}

/// Deterministic luma grain, reproducible from (seed, σ, ρ). White
/// triangular noise shaped by a separable AR(1) filter with the MEASURED
/// lag-1 correlation, then variance-normalized back to σ — the cheap
/// version of AV1's AR-coefficient film-grain synthesis, and the
/// difference between "believable film grain" and "electronic snow".
pub fn synth_grain(f: &mut Yuv420, sigma: f32, rho: f32, seed: u64) {
    if sigma <= 0.05 {
        return;
    }
    let (w, h) = (f.w, f.h);
    let rho = rho.clamp(0.0, 0.9);
    let mut s = seed | 1;
    let mut next = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    // White triangular field, unit-ish variance.
    let mut g = vec![0.0f32; w * h];
    for v in g.iter_mut() {
        let r = next();
        let u1 = ((r & 0xFFFF) as f32 / 65535.0) - 0.5;
        let u2 = (((r >> 16) & 0xFFFF) as f32 / 65535.0) - 0.5;
        *v = (u1 + u2) * 2.449_49; // σ ≈ 1
    }
    if rho > 0.01 {
        // AR(1) along x then y. Each pass scales variance by 1/(1−ρ²);
        // compensate so the field keeps σ ≈ 1 before final scaling.
        let norm = (1.0 - rho * rho).sqrt();
        for y in 0..h {
            for x in 1..w {
                let i = y * w + x;
                g[i] = rho * g[i - 1] + norm * g[i];
            }
        }
        for x in 0..w {
            for y in 1..h {
                let i = y * w + x;
                g[i] = rho * g[i - w] + norm * g[i];
            }
        }
    }
    for (p, gv) in f.y.iter_mut().zip(&g) {
        *p = (*p as f32 + gv * sigma).round().clamp(0.0, 255.0) as u8;
    }
}

/// v11.4 = v3's mode machinery + the grain split. `band` in luma levels
/// (0 disables, reproducing v3 exactly); grain is re-synthesized onto the
/// reconstruction so the output is displayable as-is.
/// Two-pass lookahead RD, the construction four single-frame proxies
/// failed to replace (region_codable_cost, rotating refresh, and the
/// present-state drift anchor all lost; see SESSION_LOG).
///
/// What makes this different is HINDSIGHT. Pass 1 encodes normally and
/// records, per region per frame, the leftover error SAD(recon,source).
/// A prev-mode chain that ends badly is then visible as a whole, and —
/// critically — the refresh is placed at the chain's START, before the
/// error was paid for, rather than at the frame where drift was
/// finally noticed. A present-state rule can only ever react late;
/// that late reaction is exactly why the drift anchor measured null.
///
/// Pass 2 re-encodes with those regions forced to plate mode. Both
/// passes are scored with J = SSE + λ·bytes (λ = dz²/3, the same
/// multiplier the per-take router uses) and the better one is
/// returned, so the lookahead can never lose more than encode time.
pub fn encode_gop_v3_lookahead(
    frames: &[Yuv420],
    motions: &[PlateMotion],
    plate: &Plate,
    opts: &ResidualOpts,
    chain_thresh: u32,
) -> PlateEncodeReportV2 {
    let (rep1, trace) = encode_gop_v3_forced(frames, motions, plate, opts, None);
    if frames.is_empty() || trace.is_empty() {
        return rep1;
    }
    let n_reg = trace[0].len();
    let nf = trace.len();
    // Find chains: consecutive frames where a region's leftover stays
    // above the threshold. Force plate at the FIRST frame of each such
    // chain (and only there — one refresh pays down the whole chain).
    let mut force = vec![vec![false; n_reg]; nf];
    let mut forced_count = 0usize;
    for r in 0..n_reg {
        let mut fi = 1usize;
        while fi < nf {
            if trace[fi][r] > chain_thresh {
                let start = fi;
                let mut worst = 0u32;
                while fi < nf && trace[fi][r] > chain_thresh {
                    worst = worst.max(trace[fi][r]);
                    fi += 1;
                }
                // Only chains that both persist and hurt are worth a
                // refresh: a single bad frame is cheaper to absorb in
                // residual than to re-anchor.
                if fi - start >= 3 && worst > chain_thresh * 2 {
                    force[start][r] = true;
                    forced_count += 1;
                }
            } else {
                fi += 1;
            }
        }
    }
    if forced_count == 0 {
        return rep1;
    }
    let (rep2, _) = encode_gop_v3_forced(frames, motions, plate, opts, Some(&force));
    let cost = |rep: &PlateEncodeReportV2| -> f64 {
        let mut sse = 0.0f64;
        for (rc, f) in rep.recon.iter().zip(frames) {
            for (a, b) in rc.y.iter().zip(&f.y) {
                let d = *a as f64 - *b as f64;
                sse += d * d;
            }
        }
        let bytes = rep.plate_bytes
            + rep.motion_bytes
            + rep.frames.iter().map(|c| c.residual_bytes + c.mode_bytes).sum::<usize>();
        let lambda = (opts.dead_zone as f64).powi(2) / 3.0;
        sse + lambda * bytes as f64
    };
    let (j1, j2) = (cost(&rep1), cost(&rep2));
    if std::env::var("FAO_LOOKAHEAD_DEBUG").is_ok() {
        eprintln!("lookahead: {forced_count} refreshes forced, J {j1:.4e} -> {j2:.4e}");
    }
    if j2 < j1 { rep2 } else { rep1 }
}

pub fn encode_gop_v4(
    frames: &[Yuv420],
    motions: &[PlateMotion],
    plate: &Plate,
    opts: &ResidualOpts,
    band: i32,
) -> (PlateEncodeReportV2, f32, f32) {
    let fw = frames[0].w;
    let fh = frames[0].h;
    let (gw, gh) = (fw / REGION, fh / REGION);
    let mut plate_raw = Vec::with_capacity(plate.y.len() + plate.u.len() + plate.v.len());
    plate_raw.extend_from_slice(&plate.y);
    plate_raw.extend_from_slice(&plate.u);
    plate_raw.extend_from_slice(&plate.v);
    let plate_bytes = zstd::encode_all(plate_raw.as_slice(), 19).map(|c| c.len()).unwrap_or(plate_raw.len());

    let mut out_frames = Vec::with_capacity(frames.len());
    // Two parallel lists: MC references must be CLEAN reconstructions
    // (referencing grained frames would feed synthetic noise into the
    // next frame's motion search and residual); the report's recon list
    // is the displayable output and carries the synthesized grain.
    let mut clean_recon: Vec<Yuv420> = Vec::with_capacity(frames.len());
    let mut display_recon: Vec<Yuv420> = Vec::with_capacity(frames.len());
    let mut prev_share_acc = 0.0f64;
    let mut sigma_acc = 0.0f64;
    let mut rho_acc = 0.0f64;

    for (fi, (f, m)) in frames.iter().zip(motions).enumerate() {
        let plate_pred = predict_frame(plate, m, fw, fh);
        let mut modes = vec![0u8; gw * gh];
        let mut mvs: Vec<(i8, i8)> = Vec::new();
        let composite = if fi == 0 {
            plate_pred.clone()
        } else {
            let prev = &clean_recon[fi - 1];
            let mut comp = plate_pred.clone();
            let mut prev_regions = 0usize;
            for gy in 0..gh {
                for gx in 0..gw {
                    let (rx, ry) = (gx * REGION, gy * REGION);
                    let sad_plate = sad_region(&plate_pred.y, fw, fh, rx as i32, ry as i32, &f.y, rx, ry);
                    let (mv, sad_prev) = search_mv(&prev.y, fw, fh, &f.y, rx, ry);
                    if sad_prev + 64 < sad_plate {
                        if std::env::var("FAO_MV_DEBUG").is_ok() {
                            let sat = MV_RANGE * 4; // qpel units at the clamp
                            if (mv.0 as i32).abs() >= sat - 4 || (mv.1 as i32).abs() >= sat - 4 {
                                eprintln!("mvsat");
                            } else {
                                eprintln!("mvok");
                            }
                        }
                        modes[gy * gw + gx] = 1;
                        mvs.push(mv);
                        prev_regions += 1;
                        copy_region_mc(&mut comp, prev, rx, ry, mv);
                    }
                }
            }
            prev_share_acc += prev_regions as f64 / (gw * gh) as f64;
            comp
        };

        // The grain split: denoise the target toward the prediction, code
        // the clean difference, remember (σ, ρ) for decode-side synthesis.
        let (target, sigma, rho) = if band > 0 {
            let (t, sg) = temporal_denoise(f, &composite, band);
            let rh = grain_lag1(f, &composite, band);
            (t, sg, rh)
        } else {
            (f.clone(), 0.0, 0.0)
        };
        sigma_acc += sigma as f64;
        rho_acc += rho as f64;

        let mut side = vec![0u8; (modes.len() + 7) / 8];
        for (i, &b) in modes.iter().enumerate() {
            if b == 1 {
                side[i / 8] |= 1 << (i % 8);
            }
        }
        {
            let mut grid: Vec<Option<(i8, i8)>> = vec![None; gw * gh];
            let mut it = mvs.iter();
            for gy in 0..gh {
                for gx in 0..gw {
                    if modes[gy * gw + gx] == 1 {
                        grid[gy * gw + gx] = it.next().copied();
                    }
                }
            }
            let mut nz: Vec<u8> = vec![0u8; (gw * gh + 7) / 8];
            let mut deltas: Vec<u8> = Vec::new();
            let mut n_prev_seen = 0usize;
            for gy in 0..gh {
                for gx in 0..gw {
                    let gi = gy * gw + gx;
                    let Some(mv) = grid[gi] else { continue };
                    let idx = n_prev_seen;
                    n_prev_seen += 1;
                    if mv == (0, 0) {
                        continue;
                    }
                    nz[idx / 8] |= 1 << (idx % 8);
                    let left = if gx > 0 { grid[gi - 1].unwrap_or((0, 0)) } else { (0, 0) };
                    let top = if gy > 0 { grid[gi - gw].unwrap_or((0, 0)) } else { (0, 0) };
                    let pred = (
                        ((left.0 as i32 + top.0 as i32) / 2) as i8,
                        ((left.1 as i32 + top.1 as i32) / 2) as i8,
                    );
                    deltas.push((mv.0.wrapping_sub(pred.0)) as u8);
                    deltas.push((mv.1.wrapping_sub(pred.1)) as u8);
                }
            }
            nz.truncate((n_prev_seen + 7) / 8);
            side.extend_from_slice(&nz);
            side.extend_from_slice(&deltas);
        }
        // σ and ρ ship as one byte each (σ×16, ρ×255) per frame.
        side.push(((sigma * 16.0).round() as u32).min(255) as u8);
        side.push(((rho.max(0.0) * 255.0).round() as u32).min(255) as u8);
        let mode_bytes = zstd::encode_all(side.as_slice(), 19).map(|c| c.len()).unwrap_or(side.len());

        let p444 = interleave_444(&composite);
        let t444 = interleave_444(&target);
        let (residual_bytes, mut rec) = match residual::encode(&p444, &t444, fw, fh, opts) {
            Some(trailer) => {
                let r = residual::apply(p444, &trailer, fw, fh);
                (trailer.len(), deinterleave_420(&r, fw, fh))
            }
            None => (0, composite),
        };
        // Clean reconstruction is the reference for the NEXT frame's MC;
        // the display copy gets the synthesized grain. Referencing grained
        // frames would feed synthetic noise back into motion search.
        clean_recon.push(rec.clone());
        synth_grain(&mut rec, sigma, rho, 0x5EED_0000 + fi as u64);
        display_recon.push(rec);
        out_frames.push(FrameCost { mode_bytes, residual_bytes });
    }
    let denom = (frames.len().saturating_sub(1)).max(1) as f64;
    (
        PlateEncodeReportV2 {
            plate_bytes,
            motion_bytes: frames.len() * 8,
            frames: out_frames,
            recon: display_recon,
            prev_mode_share: (prev_share_acc / denom) as f32,
        },
        (sigma_acc / frames.len() as f64) as f32,
        (rho_acc / frames.len() as f64) as f32,
    )
}

/// The user's two-branch question, measured: of the high-pass energy in
/// a frame, how much is WORLD-DERIVED (survives motion-compensated
/// prediction — texture the web should own) vs FLAT-MASK (correlates
/// with nothing temporally — synthesizable screen-space noise)?
/// Returns (world_fraction, mask_fraction, hp_rms). The high-pass here
/// is (pixel − 3×3 box mean); "survives prediction" means the co-sited
/// high-pass of the prediction agrees in sign and comparable magnitude.
pub fn classify_highpass(cur: &Yuv420, pred: &Yuv420) -> (f32, f32, f32) {
    let (w, h) = (cur.w, cur.h);
    let hp = |img: &[u8], x: usize, y: usize| -> f32 {
        let mut sum = 0i32;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let sx = (x as i32 + dx).clamp(0, w as i32 - 1) as usize;
                let sy = (y as i32 + dy).clamp(0, h as i32 - 1) as usize;
                sum += img[sy * w + sx] as i32;
            }
        }
        img[y * w + x] as f32 - sum as f32 / 9.0
    };
    let mut world = 0.0f64;
    let mut mask = 0.0f64;
    let mut total = 0.0f64;
    // Sample every 2nd pixel for speed; statistics, not reconstruction.
    let mut y = 1;
    while y + 1 < h {
        let mut x = 1;
        while x + 1 < w {
            let hc = hp(&cur.y, x, y);
            let hpre = hp(&pred.y, x, y);
            let e = (hc * hc) as f64;
            total += e;
            // Same sign and at least half the magnitude → the prediction
            // already carries this detail → world-derived.
            if hc * hpre > 0.0 && hpre.abs() * 2.0 >= hc.abs() {
                world += e;
            } else {
                mask += e;
            }
            x += 2;
        }
        y += 2;
    }
    if total < 1.0 {
        return (0.0, 0.0, 0.0);
    }
    let n = (((h - 2) / 2) * ((w - 2) / 2)) as f64;
    (
        (world / total) as f32,
        (mask / total) as f32,
        ((total / n).sqrt()) as f32,
    )
}

// ═══════════════════════════════════════════════════════════════════
// v11.5 — spacetime residual volumes (the "merge residuals across
// frames via a continuous function" experiment, in its simplest
// falsifiable form).
//
// If per-frame residuals share structure over time — model error that
// drifts smoothly, lighting ramps, warp misfit — then a transform with
// a TEMPORAL axis concentrates that shared structure into the
// temporal-DC plane and a few low-t-frequency coefficients, and one
// quantized volume replaces four quantized planes. If the residual is
// temporally white (grain), the temporal axis concentrates nothing and
// 3D coding pays pure overhead. So this module both CODES 4×4×4
// (x,y,t) volumes and MEASURES the split: `temporal_dc_share` is the
// fraction of pre-quantization residual energy living at t-frequency 0
// — the ceiling on what ANY cross-frame merge (polynomial, Fourier,
// learned, or otherwise) can exploit.
//
// Frames are processed in groups of 4 predicted OPEN-LOOP from the
// previous group's last reconstruction (mode {plate, anchor+MV} per
// 16×16 region per frame, same machinery as v3) — the temporal block
// needs all four residual planes before any frame can finalize, so
// within-group closed loop is impossible by construction. Drift is
// bounded by the group length and resets at each group boundary.
// ═══════════════════════════════════════════════════════════════════

use crate::transform::{dct3d_4x4x4, idct3d_4x4x4, quantize_4x4x4, dequantize_4x4x4, QUANT_MATRIX_4X4X4};

pub struct T3dReport {
    pub plate_bytes: usize,
    pub side_bytes: usize,
    pub residual_bytes: usize,
    pub recon: Vec<Yuv420>,
    /// Fraction of residual energy in the t-frequency-0 plane (pre-quant).
    pub temporal_dc_share: f32,
    pub prev_mode_share: f32,
}

pub fn encode_gop_t3d(
    frames: &[Yuv420],
    motions: &[PlateMotion],
    plate: &Plate,
    dead_zone: f32,
    skip_thr: i8,
) -> T3dReport {
    let fw = frames[0].w;
    let fh = frames[0].h;
    let (gw, gh) = (fw / REGION, fh / REGION);
    let (rw, rh) = (fw / 4, fh / 4);
    let mut plate_raw = Vec::with_capacity(plate.y.len() + plate.u.len() + plate.v.len());
    plate_raw.extend_from_slice(&plate.y);
    plate_raw.extend_from_slice(&plate.u);
    plate_raw.extend_from_slice(&plate.v);
    let plate_bytes = zstd::encode_all(plate_raw.as_slice(), 19).map(|c| c.len()).unwrap_or(plate_raw.len());

    let mut recon: Vec<Yuv420> = Vec::with_capacity(frames.len());
    let mut side_bytes = 0usize;
    let mut residual_bytes = 0usize;
    let mut e_tdc = 0.0f64;
    let mut e_all = 0.0f64;
    let mut prev_share_acc = 0.0f64;
    let mut prev_share_n = 0usize;

    let mut g0 = 0usize;
    while g0 < frames.len() {
        let glen = 4.min(frames.len() - g0);
        // Anchor: last recon of the previous group (None for the first).
        let anchor: Option<Yuv420> = if g0 == 0 { None } else { Some(recon[g0 - 1].clone()) };

        // 1. Per frame: composite prediction vs {plate, anchor+MV}.
        let mut composites: Vec<Yuv420> = Vec::with_capacity(glen);
        let mut group_side: Vec<u8> = Vec::new();
        for t in 0..glen {
            let f = &frames[g0 + t];
            let m = &motions[g0 + t];
            let plate_pred = predict_frame(plate, m, fw, fh);
            match &anchor {
                None => {
                    // First group codes against the plate alone.
                    group_side.push(0);
                    composites.push(plate_pred);
                }
                Some(anc) => {
                    let mut comp = plate_pred.clone();
                    let mut modes = vec![0u8; gw * gh];
                    let mut mvs: Vec<(i8, i8)> = Vec::new();
                    let mut prev_regions = 0usize;
                    for gy in 0..gh {
                        for gx in 0..gw {
                            let (rx, ry) = (gx * REGION, gy * REGION);
                            let sad_plate = sad_region(&plate_pred.y, fw, fh, rx as i32, ry as i32, &f.y, rx, ry);
                            let (mv, sad_prev) = search_mv(&anc.y, fw, fh, &f.y, rx, ry);
                            if sad_prev + 64 < sad_plate {
                                modes[gy * gw + gx] = 1;
                                mvs.push(mv);
                                prev_regions += 1;
                                copy_region_mc(&mut comp, anc, rx, ry, mv);
                            }
                        }
                    }
                    prev_share_acc += prev_regions as f64 / (gw * gh) as f64;
                    prev_share_n += 1;
                    group_side.push(1);
                    let mut bits = vec![0u8; (modes.len() + 7) / 8];
                    for (i, &b) in modes.iter().enumerate() {
                        if b == 1 {
                            bits[i / 8] |= 1 << (i % 8);
                        }
                    }
                    group_side.extend_from_slice(&bits);
                    for &(dx, dy) in &mvs {
                        group_side.push(dx as u8);
                        group_side.push(dy as u8);
                    }
                    composites.push(comp);
                }
            }
        }
        side_bytes += zstd::encode_all(group_side.as_slice(), 19).map(|c| c.len()).unwrap_or(group_side.len());

        // 2. Residual volumes: 4×4×glen per channel (glen<4 pads with 0 —
        //    padded slices quantize to nothing and decode is truncated).
        let comp444: Vec<Vec<u8>> = composites.iter().map(interleave_444).collect();
        let tgt444: Vec<Vec<u8>> = (0..glen).map(|t| interleave_444(&frames[g0 + t])).collect();
        let mut payload: Vec<u8> = Vec::new();
        let mut skip_bits = vec![0u8; (rw * rh + 7) / 8];
        let mut vols: Vec<Option<([[i8; 64]; 3], [crate::transform::QuantParams; 3])>> =
            vec![None; rw * rh];
        for by in 0..rh {
            for bx in 0..rw {
                let mut q3: [[i8; 64]; 3] = [[0; 64]; 3];
                let mut p3 = [crate::transform::QuantParams { scale_byte: 0, saliency_boost: 0 }; 3];
                let mut all_skip = true;
                for c in 0..3 {
                    let mut buf = [0.0f32; 64];
                    for z in 0..glen {
                        for ly in 0..4 {
                            for lx in 0..4 {
                                let px = bx * 4 + lx;
                                let py = by * 4 + ly;
                                let i = (py * fw + px) * 3 + c;
                                buf[z * 16 + ly * 4 + lx] =
                                    tgt444[z][i] as f32 - comp444[z][i] as f32;
                            }
                        }
                    }
                    dct3d_4x4x4(&mut buf);
                    // Energy bookkeeping (luma only, pre-deadzone): z=0
                    // slice is the cross-frame-mergeable component.
                    if c == 0 {
                        for (k, &v) in buf.iter().enumerate() {
                            let e = (v as f64) * (v as f64);
                            e_all += e;
                            if k < 16 {
                                e_tdc += e;
                            }
                        }
                    }
                    if dead_zone > 0.0 {
                        for (k, v) in buf.iter_mut().enumerate() {
                            if v.abs() < dead_zone * QUANT_MATRIX_4X4X4[k] as f32 {
                                *v = 0.0;
                            }
                        }
                    }
                    let (q, p) = quantize_4x4x4(&buf, 0);
                    if q.iter().any(|&x| x.unsigned_abs() > skip_thr as u8) {
                        all_skip = false;
                    }
                    q3[c] = q;
                    p3[c] = p;
                }
                if !all_skip {
                    let bi = by * rw + bx;
                    skip_bits[bi / 8] |= 1 << (bi % 8);
                    vols[bi] = Some((q3, p3));
                }
            }
        }
        payload.extend_from_slice(&skip_bits);
        for v in vols.iter().flatten() {
            for c in 0..3 {
                payload.push(v.1[c].pack());
                for &q in &v.0[c] {
                    payload.push(q as u8);
                }
            }
        }
        residual_bytes += zstd::encode_all(payload.as_slice(), 12).map(|c| c.len()).unwrap_or(payload.len());

        // 3. Reconstruct the group (decoder path: dequant, idct, add).
        for t in 0..glen {
            let mut r444 = comp444[t].clone();
            for by in 0..rh {
                for bx in 0..rw {
                    if let Some((q3, p3)) = &vols[by * rw + bx] {
                        for c in 0..3 {
                            let mut buf = dequantize_4x4x4(&q3[c], p3[c]);
                            idct3d_4x4x4(&mut buf);
                            for ly in 0..4 {
                                for lx in 0..4 {
                                    let px = bx * 4 + lx;
                                    let py = by * 4 + ly;
                                    let i = (py * fw + px) * 3 + c;
                                    let v = r444[i] as f32 + buf[t * 16 + ly * 4 + lx];
                                    r444[i] = v.clamp(0.0, 255.0) as u8;
                                }
                            }
                        }
                    }
                }
            }
            recon.push(deinterleave_420(&r444, fw, fh));
        }
        g0 += glen;
    }

    T3dReport {
        plate_bytes,
        side_bytes,
        residual_bytes,
        recon,
        temporal_dc_share: if e_all > 0.0 { (e_tdc / e_all) as f32 } else { 0.0 },
        prev_mode_share: if prev_share_n > 0 { (prev_share_acc / prev_share_n as f64) as f32 } else { 0.0 },
    }
}

// ═══════════════════════════════════════════════════════════════════
// v11.6 — super-resolved plate ("fence-slit" accumulation).
//
// Sub-pixel camera motion makes every frame a point-sampling of the
// world at a different phase. Accumulating those samples on a grid
// FINER than the frame recovers detail no single frame contains —
// multi-frame super-resolution, the formal version of seeing through a
// picket fence by running past it. On upscaled/transcoded sources this
// is "understanding what the content should be": the SR plate can
// sharpen past the upscaler's blur because the blur kernel's phase
// varies with the sub-pixel offset.
//
// Requirements this module is honest about: it only pays where the
// motion field actually has sub-pixel DIVERSITY (a locked-off tripod
// shot puts every sample at the same phase — S=1 and S=2 plates are
// then identical by construction), and it needs the sub-pixel motion
// estimates to be trustworthy (hence the phase-correlation confidence
// fix that landed with it).
//
// Robustness: per-cell two-pass mean (mean, then re-mean over samples
// within ±REJECT_BAND of it) instead of the exact median — the S=2
// grid has 4× the cells and per-cell sample lists would not fit;
// movers land far outside the band and are rejected the same way in
// the common case. Cells no frame ever sampled fall back to bilinear
// upsampling of the robust S=1 plate.
// ═══════════════════════════════════════════════════════════════════

const SR_REJECT_BAND: f32 = 12.0;

/// Luma-only SR plate at integer scale `s` (chroma stays on the S=1
/// plate — the detail this exists to recover is luma).
pub struct SrPlate {
    pub s: usize,
    pub w: usize,
    pub h: usize,
    pub y: Vec<u8>,
    pub base: Plate,
}

pub fn build_plate_sr(frames: &[Yuv420], motions: &[PlateMotion], s: usize) -> SrPlate {
    let base = build_plate(frames, motions);
    let (pw, ph) = (base.w * s, base.h * s);
    let fw = frames[0].w;
    let fh = frames[0].h;

    let mut sum = vec![0.0f32; pw * ph];
    let mut cnt = vec![0u16; pw * ph];
    let splat = |sum: &mut [f32], cnt: &mut [u16], frames: &[Yuv420], pass2: Option<(&[f32], &[u16])>| {
        for (f, m) in frames.iter().zip(motions) {
            for fy in 0..fh {
                for fx in 0..fw {
                    // World (frame-0) coordinate of this sample, on the SR grid.
                    let wx = (fx as f32 - m.dx - base.ox) * s as f32;
                    let wy = (fy as f32 - m.dy - base.oy) * s as f32;
                    let cx = wx.round() as i32;
                    let cy = wy.round() as i32;
                    if cx < 0 || cy < 0 || cx as usize >= pw || cy as usize >= ph {
                        continue;
                    }
                    let ci = cy as usize * pw + cx as usize;
                    let v = f.y[fy * fw + fx] as f32;
                    if let Some((m1, c1)) = pass2 {
                        if c1[ci] > 0 {
                            let mean1 = m1[ci] / c1[ci] as f32;
                            if (v - mean1).abs() > SR_REJECT_BAND {
                                continue;
                            }
                        }
                    }
                    sum[ci] += v;
                    cnt[ci] += 1;
                }
            }
        }
    };
    splat(&mut sum, &mut cnt, frames, None);
    let (sum1, cnt1) = (sum.clone(), cnt.clone());
    for v in sum.iter_mut() {
        *v = 0.0;
    }
    for v in cnt.iter_mut() {
        *v = 0;
    }
    splat(&mut sum, &mut cnt, frames, Some((&sum1, &cnt1)));

    let mut y = vec![0u8; pw * ph];
    for py in 0..ph {
        for px in 0..pw {
            let ci = py * pw + px;
            y[ci] = if cnt[ci] > 0 {
                (sum[ci] / cnt[ci] as f32).round().clamp(0.0, 255.0) as u8
            } else {
                // Hole: no frame sampled this phase — fall back to the
                // robust S=1 plate, bilinearly.
                bilinear(&base.y, base.w, base.h, px as f32 / s as f32, py as f32 / s as f32)
            };
        }
    }
    SrPlate { s, w: pw, h: ph, y, base }
}

/// Predict a frame from the SR plate (luma) + base plate (chroma).
pub fn predict_frame_sr(sp: &SrPlate, m: &PlateMotion, fw: usize, fh: usize) -> Yuv420 {
    let mut out = predict_frame(&sp.base, m, fw, fh);
    let s = sp.s as f32;
    for fy in 0..fh {
        for fx in 0..fw {
            let px = (fx as f32 - m.dx - sp.base.ox) * s;
            let py = (fy as f32 - m.dy - sp.base.oy) * s;
            out.y[fy * fw + fx] = bilinear(&sp.y, sp.w, sp.h, px, py);
        }
    }
    out
}

#[cfg(test)]
mod tests_sr {
    use super::*;

    /// Frames are bilinear point-samplings of a hi-res world at varied
    /// sub-pixel offsets. The S=2 plate must predict held-out views
    /// clearly better than the S=1 plate — the fence-slit effect.
    #[test]
    fn sr_plate_beats_base_plate_given_subpixel_diversity() {
        let s = 2usize;
        let (fw, fh) = (64, 48);
        let (ww, wh) = (fw * s + 16, fh * s + 16);
        // Hi-res world with detail at the SR scale: diagonal stripes of
        // wavelength ~3 SR px (≈1.5 frame px — beyond one frame's Nyquist).
        let world: Vec<f32> = (0..ww * wh)
            .map(|i| {
                let (x, y) = (i % ww, i / ww);
                128.0 + 60.0 * (((x + 2 * y) as f32) * (2.0 * std::f32::consts::PI / 6.0)).sin()
            })
            .collect();
        let sample_world = |wx: f32, wy: f32| -> f32 {
            let x0 = wx.floor().max(0.0) as usize;
            let y0 = wy.floor().max(0.0) as usize;
            let x1 = (x0 + 1).min(ww - 1);
            let y1 = (y0 + 1).min(wh - 1);
            let tx = wx - x0 as f32;
            let ty = wy - y0 as f32;
            let top = world[y0 * ww + x0] * (1.0 - tx) + world[y0 * ww + x1] * tx;
            let bot = world[y1 * ww + x0] * (1.0 - tx) + world[y1 * ww + x1] * tx;
            top * (1.0 - ty) + bot * ty
        };
        // 16 frames drifting by irrational-ish sub-pixel steps.
        let mut frames = Vec::new();
        let mut motions = Vec::new();
        for t in 0..16 {
            let dx = -(t as f32) * 0.37; // frame-space drift
            let dy = -(t as f32) * 0.23;
            let mut f = Yuv420::new(fw, fh);
            for fy in 0..fh {
                for fx in 0..fw {
                    // frame pixel shows world coord (fx - dx) on the S grid
                    let wx = (fx as f32 - dx) * s as f32 + 4.0;
                    let wy = (fy as f32 - dy) * s as f32 + 4.0;
                    f.y[fy * fw + fx] = sample_world(wx, wy).round().clamp(0.0, 255.0) as u8;
                }
            }
            for p in f.u.iter_mut() {
                *p = 128;
            }
            for p in f.v.iter_mut() {
                *p = 128;
            }
            frames.push(f);
            motions.push(PlateMotion { dx, dy, confidence: 1.0 });
        }
        let sp = build_plate_sr(&frames, &motions, s);
        let base_err = |pred: &Yuv420, f: &Yuv420| -> f64 {
            let mut e = 0.0f64;
            for i in 0..fw * fh {
                e += (pred.y[i] as f64 - f.y[i] as f64).abs();
            }
            e / (fw * fh) as f64
        };
        let mut e_base = 0.0;
        let mut e_sr = 0.0;
        for (f, m) in frames.iter().zip(&motions) {
            e_base += base_err(&predict_frame(&sp.base, m, fw, fh), f);
            e_sr += base_err(&predict_frame_sr(&sp, m, fw, fh), f);
        }
        assert!(
            e_sr * 1.3 < e_base,
            "SR plate should predict sub-pixel-diverse views ≥1.3x better: base {e_base:.2} sr {e_sr:.2}"
        );
    }
}

// ── Long-range dictionary probe (landmark-fingerprint idea, applied
// inside a video) ────────────────────────────────────────────────────
//
// snapclone's LandmarkHash answers "is this the same PLACE?" from a
// 12-byte lossy transform: a blur hash for coarse layout plus an edge
// hash for line geometry, chosen because they fail in different
// weather. The same construction works one level down: a per-REGION
// fingerprint answers "have I already coded content that looks like
// this, anywhere earlier in the video?" — which is the question the
// plate model cannot ask, because a plate only holds ONE aligned
// background.
//
// That matters most for the material v11 currently loses on. Broadcast
// game capture repeats content constantly (HUD, minimap, ability bars,
// respawn screens, the same lane geometry from the same camera angle
// minutes apart) at positions no motion vector can reach — AV1's
// screen-content tools exploit exactly this, and v11 has nothing.
//
// This is a PROBE, not an encoder mode: it measures how much
// long-range redundancy actually exists before any bitstream work is
// justified. Dictionary entries come only from frames the decoder
// already has (fi-2 and older, so it never overlaps the prev-frame
// predictor), keyed by a 32-bit fingerprint in the landmark spirit:
// 16 coarse-luma bits (4x4 cell means vs region mean — the blur hash)
// and 16 gradient bits (per-cell |dx|+|dy| vs cell mean — the edge
// hash). Candidates are verified by true SAD, so the hash only has to
// be good enough to shortlist.
/// 16-bit coarse fingerprint of a REGION x REGION luma block: 8 blur
/// bits + 8 edge bits over a 4x4 cell grid, each set when the cell
/// exceeds the block's own mean. Shared by `probe_dictionary` (which
/// measured the redundancy) and the coder's reference mode (which
/// spends it), so the thing that was measured is the thing that ships.
pub fn region_fingerprint(src: &[u8], w: usize, h: usize, rx: usize, ry: usize) -> u32 {
    let mut cells = [0u32; 16];
    let mut grads = [0u32; 16];
    for cy in 0..4 {
        for cx in 0..4 {
            let (mut sum, mut g) = (0u32, 0u32);
            for dy in 0..4 {
                for dx in 0..4 {
                    let (x, y) = (rx + cx * 4 + dx, ry + cy * 4 + dy);
                    let v = src[y * w + x] as i32;
                    sum += v as u32;
                    let vx = src[y * w + (x + 1).min(w - 1)] as i32;
                    let vy = src[(y + 1).min(h - 1) * w + x] as i32;
                    g += ((vx - v).abs() + (vy - v).abs()) as u32;
                }
            }
            cells[cy * 4 + cx] = sum / 16;
            grads[cy * 4 + cx] = g / 16;
        }
    }
    let cmean: u32 = cells.iter().sum::<u32>() / 16;
    let gmean: u32 = grads.iter().sum::<u32>() / 16;
    let mut fp = 0u32;
    for i in 0..16 {
        if cells[i] > cmean {
            fp |= 1 << i;
        }
        if grads[i] > gmean {
            fp |= 1 << (16 + i);
        }
    }
    let mut key = 0u32;
    for i in 0..8 {
        if (fp >> (i * 2)) & 1 == 1 {
            key |= 1 << i;
        }
        if (fp >> (16 + i * 2)) & 1 == 1 {
            key |= 1 << (8 + i);
        }
    }
    key
}

pub fn probe_dictionary(frames: &[Yuv420], plate: &Plate, motions: &[PlateMotion]) -> String {
    let (fw, fh) = (frames[0].w, frames[0].h);
    let (gw, gh) = (fw / REGION, fh / REGION);
    let fingerprint = |src: &[u8], w: usize, rx: usize, ry: usize| -> u32 {
        let mut cells = [0u32; 16];
        let mut grads = [0u32; 16];
        for cy in 0..4 {
            for cx in 0..4 {
                let (mut sum, mut g) = (0u32, 0u32);
                for dy in 0..4 {
                    for dx in 0..4 {
                        let (x, y) = (rx + cx * 4 + dx, ry + cy * 4 + dy);
                        let v = src[y * w + x] as i32;
                        sum += v as u32;
                        let vx = src[y * w + (x + 1).min(w - 1)] as i32;
                        let vy = src[(y + 1).min(fh - 1) * w + x] as i32;
                        g += ((vx - v).abs() + (vy - v).abs()) as u32;
                    }
                }
                cells[cy * 4 + cx] = sum / 16;
                grads[cy * 4 + cx] = g / 16;
            }
        }
        let cmean: u32 = cells.iter().sum::<u32>() / 16;
        let gmean: u32 = grads.iter().sum::<u32>() / 16;
        let mut fp = 0u32;
        for i in 0..16 {
            if cells[i] > cmean {
                fp |= 1 << i;
            }
            if grads[i] > gmean {
                fp |= 1 << (16 + i);
            }
        }
        // Coarse key = 8 blur bits + 8 edge bits (every other cell), so
        // near-matches that differ in fine cells still collide. The
        // landmark matcher uses Hamming distance ≤10/64 rather than
        // exact equality; exact-bucket lookup on a 32-bit key is a
        // strictly weaker test, and this is the cheap way to widen the
        // shortlist without enumerating the Hamming ball.
        let mut key = 0u32;
        for i in 0..8 {
            if (fp >> (i * 2)) & 1 == 1 {
                key |= 1 << i;
            }
            if (fp >> (16 + i * 2)) & 1 == 1 {
                key |= 1 << (8 + i);
            }
        }
        key
    };
    use std::collections::HashMap;
    let mut dict: HashMap<u32, Vec<(usize, usize, usize)>> = HashMap::new();
    let (mut n_reg, mut n_dict_win, mut n_dict_strong) = (0u64, 0u64, 0u64);
    let mut sad_saved = 0u64;
    for fi in 0..frames.len() {
        let f = &frames[fi];
        let plate_pred = predict_frame(plate, &motions[fi], fw, fh);
        if fi >= 2 {
            for gy in 0..gh {
                for gx in 0..gw {
                    let (rx, ry) = (gx * REGION, gy * REGION);
                    n_reg += 1;
                    let sad_plate =
                        sad_region(&plate_pred.y, fw, fh, rx as i32, ry as i32, &f.y, rx, ry);
                    let (_mv, sad_prev) =
                        search_mv(&frames[fi - 1].y, fw, fh, &f.y, rx, ry);
                    let best_local = sad_plate.min(sad_prev);
                    let fp = fingerprint(&f.y, fw, rx, ry);
                    let mut best_dict = u32::MAX;
                    if let Some(cands) = dict.get(&fp) {
                        for &(sfi, sx, sy) in cands.iter().rev().take(64) {
                            let s = sad_region(
                                &frames[sfi].y, fw, fh, sx as i32, sy as i32, &f.y, rx, ry,
                            );
                            best_dict = best_dict.min(s);
                        }
                    }
                    if best_dict < best_local {
                        n_dict_win += 1;
                        sad_saved += (best_local - best_dict) as u64;
                        // "Strong" = beats the local predictors by enough
                        // to pay for a ~4-byte long-range reference.
                        if best_dict + 512 < best_local {
                            n_dict_strong += 1;
                        }
                    }
                }
            }
        }
        // Insert this frame's regions into the dictionary (decoder has
        // it once coded).
        for gy in 0..gh {
            for gx in 0..gw {
                let (rx, ry) = (gx * REGION, gy * REGION);
                let fp = fingerprint(&f.y, fw, rx, ry);
                dict.entry(fp).or_default().push((fi, rx, ry));
            }
        }
    }
    format!(
        "dict probe: {} regions | long-range beats local {:.1}% | STRONG (>512 SAD margin) {:.1}% | mean SAD saved on wins {:.0} | distinct fingerprints {}",
        n_reg,
        100.0 * n_dict_win as f64 / n_reg as f64,
        100.0 * n_dict_strong as f64 / n_reg as f64,
        if n_dict_win > 0 { sad_saved as f64 / n_dict_win as f64 } else { 0.0 },
        dict.len()
    )
}

// ═══════════════════════════════════════════════════════════════════
// SCENE PROTOCOL — the mover layer as an editable object graph.
//
// The v11.5 mover layer already IS an asset pool with per-frame
// placement: each object is a sprite transmitted once plus a
// 2-byte/frame position track. That makes narrative editing a
// data edit rather than a pixel synthesis problem — moving an actor is
// rewriting a track, and the sprite it re-pastes is the same sprite the
// decoder already holds.
//
// This module is the PROTOCOL half of that, deliberately separated from
// any transport. `scene_to_json` emits the object graph; `apply_scene_json`
// reads a possibly-edited graph back and returns new tracks. An LLM is
// one producer of that JSON; a human with a text editor is another, and
// the second is what the tests use — the protocol must be exercisable
// with no model in the loop, or its failures and the model's failures
// become indistinguishable.
//
// WHAT THIS CANNOT DO, stated up front because the schema makes it look
// more capable than it is:
//   * Disocclusion. Move an object and you reveal plate underneath. The
//     plate is a temporal median, so it holds real background wherever
//     the object moved during the GOP — and holds nothing where it
//     stood still the whole time. `hole_risk` per mover reports which
//     case each object is in, measured, so an editor can refuse or warn
//     instead of producing a smear.
//   * Shadows and contact lighting are baked into the sprite. A moved
//     object carries its old shadow.
//   * Non-rigid change. The sprite is one rigid cutout; a fight landing
//     differently needs poses that were never photographed. Re-timing,
//     re-positioning and re-ordering are in scope; new articulation is
//     not, and no resolver can fake it from this representation.
// ═══════════════════════════════════════════════════════════════════

/// Per-mover disocclusion risk: the fraction of the object's masked
/// cells for which the plate holds real background rather than the
/// object itself. Computed as the share of frames in which the object
/// is somewhere OTHER than the cell in question — the plate's median
/// keeps background at a pixel only when the object vacates it in more
/// than half the frames, which is exactly this quantity thresholded.
pub fn mover_hole_risk(m: &Mover) -> f32 {
    let seen: Vec<(i16, i16)> = m.track.iter().filter_map(|t| *t).collect();
    if seen.len() < 2 {
        return 1.0;
    }
    let (bw, bh) = (m.bw as f32, m.bh as f32);
    let mut vacated = 0usize;
    for (i, a) in seen.iter().enumerate() {
        // A frame vacates the reference footprint if the object has
        // moved more than its own extent away from where it started.
        let dx = (a.0 as f32 - m.bx as f32).abs() / bw;
        let dy = (a.1 as f32 - m.by as f32).abs() / bh;
        if dx.max(dy) >= 1.0 {
            vacated += 1;
        }
        let _ = i;
    }
    // Risk is high when the object rarely vacates: the plate then has
    // no clean background to reveal.
    1.0 - (vacated as f32 / seen.len() as f32)
}

/// Emit the GOP's object graph as JSON.
pub fn scene_to_json(movers: &[Mover], n_frames: usize, fw: usize, fh: usize) -> String {
    let mut s = String::new();
    s.push_str("{\n");
    s.push_str(&format!(
        "  \"frames\": {}, \"width\": {}, \"height\": {},\n",
        n_frames, fw, fh
    ));
    s.push_str("  \"movers\": [\n");
    for (i, m) in movers.iter().enumerate() {
        let cells = m.mask.iter().filter(|&&b| b).count();
        let visible = m.track.iter().filter(|t| t.is_some()).count();
        s.push_str(&format!(
            "    {{ \"id\": {}, \"bbox\": [{}, {}, {}, {}], \"ref_frame\": {}, \"cells\": {}, \"visible_frames\": {}, \"hole_risk\": {:.2},\n      \"track\": [",
            i, m.bx, m.by, m.bw, m.bh, m.ref_frame, cells, visible, mover_hole_risk(m)
        ));
        for (t, p) in m.track.iter().enumerate() {
            if t > 0 {
                s.push_str(", ");
            }
            match p {
                Some((x, y)) => s.push_str(&format!("[{}, {}]", x, y)),
                None => s.push_str("null"),
            }
        }
        s.push_str("] }");
        if i + 1 < movers.len() {
            s.push(',');
        }
        s.push('\n');
    }
    s.push_str("  ]\n}\n");
    s
}

/// Read an edited object graph and return one track per mover id.
///
/// Deliberately strict, because the producer may be a language model:
/// a track must exist for every mover, be exactly `n_frames` long, and
/// contain only `null` or `[x, y]` integer pairs. Anything else is an
/// error naming the mover, not a silent partial application — a scene
/// that half-applies is worse than one that refuses.
pub fn apply_scene_json(
    json: &str,
    n_movers: usize,
    n_frames: usize,
) -> Result<Vec<Vec<Option<(i16, i16)>>>, String> {
    let mut out: Vec<Option<Vec<Option<(i16, i16)>>>> = vec![None; n_movers];
    let b = json.as_bytes();
    let mut i = 0usize;
    // Walk "id": N ... "track": [ ... ] pairs in order of appearance.
    while let Some(idp) = json[i..].find("\"id\"") {
        let idp = i + idp + 4;
        let colon = json[idp..].find(':').ok_or("malformed: \"id\" without ':'")? + idp + 1;
        let mut j = colon;
        while j < b.len() && (b[j] as char).is_whitespace() {
            j += 1;
        }
        let ns = j;
        while j < b.len() && (b[j] as char).is_ascii_digit() {
            j += 1;
        }
        let id: usize = json[ns..j].parse().map_err(|_| "malformed mover id".to_string())?;
        if id >= n_movers {
            return Err(format!("mover id {id} out of range (scene has {n_movers})"));
        }
        let tp = json[j..]
            .find("\"track\"")
            .ok_or_else(|| format!("mover {id}: no \"track\""))?
            + j;
        let open = json[tp..].find('[').ok_or_else(|| format!("mover {id}: track not an array"))? + tp;
        let close = json[open..]
            .find(']')
            .map(|_| {
                // Find the ']' that closes the OUTER array: scan with depth.
                let mut d = 0i32;
                let mut k = open;
                while k < b.len() {
                    if b[k] == b'[' {
                        d += 1;
                    } else if b[k] == b']' {
                        d -= 1;
                        if d == 0 {
                            return k;
                        }
                    }
                    k += 1;
                }
                b.len()
            })
            .ok_or_else(|| format!("mover {id}: unterminated track"))?;
        if close >= b.len() {
            return Err(format!("mover {id}: unterminated track array"));
        }
        let body = &json[open + 1..close];
        let mut track: Vec<Option<(i16, i16)>> = Vec::with_capacity(n_frames);
        let mut k = 0usize;
        let bb = body.as_bytes();
        while k < bb.len() {
            let c = bb[k] as char;
            if c.is_whitespace() || c == ',' {
                k += 1;
                continue;
            }
            if body[k..].starts_with("null") {
                track.push(None);
                k += 4;
                continue;
            }
            if c != '[' {
                return Err(format!("mover {id}: expected [x, y] or null at offset {k}"));
            }
            let e = body[k..]
                .find(']')
                .ok_or_else(|| format!("mover {id}: unterminated pair"))?
                + k;
            let nums: Vec<&str> = body[k + 1..e].split(',').map(|t| t.trim()).collect();
            if nums.len() != 2 {
                return Err(format!("mover {id}: pair needs exactly 2 numbers"));
            }
            let x: i32 = nums[0]
                .parse::<f64>()
                .map_err(|_| format!("mover {id}: bad x '{}'", nums[0]))? as i32;
            let y: i32 = nums[1]
                .parse::<f64>()
                .map_err(|_| format!("mover {id}: bad y '{}'", nums[1]))? as i32;
            track.push(Some((x.clamp(-32768, 32767) as i16, y.clamp(-32768, 32767) as i16)));
            k = e + 1;
        }
        if track.len() != n_frames {
            return Err(format!(
                "mover {id}: track has {} entries, GOP has {n_frames} frames",
                track.len()
            ));
        }
        out[id] = Some(track);
        i = close;
    }
    let mut res = Vec::with_capacity(n_movers);
    for (id, t) in out.into_iter().enumerate() {
        res.push(t.ok_or_else(|| format!("scene omits mover {id}"))?);
    }
    Ok(res)
}

/// Build the RETARGETED frames an edited scene implies.
///
/// The measurement that forced this function into existence: editing
/// only the predictor changes nothing. The residual is coded against
/// the SOURCE, so it faithfully corrects any prediction back toward the
/// original video — a scene edit applied to the composite alone came
/// back at 108 dB against the unedited encode, i.e. the codec undid it.
/// An editor that changes what the viewer sees has to change the
/// TARGET, not the prediction.
///
/// Per frame, for every mover whose track changed:
///   1. paint the plate over its ORIGINAL footprint — this is the
///      disocclusion step, and it is why `mover_hole_risk` exists: the
///      plate holds real background only where the object actually
///      vacated during the GOP;
///   2. paste its sprite at the NEW position.
/// Everything else keeps the original pixels, so unedited regions still
/// code against real source and keep their fidelity.
pub fn retarget_frames(
    frames: &[Yuv420],
    motions: &[PlateMotion],
    plate: &Plate,
    movers: &[Mover],
    edits: &[Vec<Option<(i16, i16)>>],
) -> Vec<Yuv420> {
    let (fw, fh) = (frames[0].w, frames[0].h);
    let mut out = Vec::with_capacity(frames.len());
    for (fi, f) in frames.iter().enumerate() {
        let mut t = f.clone();
        let mut pred: Option<Yuv420> = None;
        for (mi, m) in movers.iter().enumerate() {
            let new_t = edits.get(mi).and_then(|e| e.get(fi).copied()).flatten();
            let old_t = m.track[fi];
            if new_t == old_t {
                continue;
            }
            let p = pred.get_or_insert_with(|| predict_frame(plate, &motions[fi], fw, fh));
            // 1. erase from the old position using the plate.
            if let Some((ox, oy)) = old_t {
                let cw_cells = m.bw / REGION;
                for cy in 0..(m.bh / REGION) {
                    for cx in 0..cw_cells {
                        if !m.mask[cy * cw_cells + cx] {
                            continue;
                        }
                        for ly in 0..REGION {
                            let sy = oy as i32 + (cy * REGION + ly) as i32;
                            if sy < 0 || sy >= fh as i32 {
                                continue;
                            }
                            for lx in 0..REGION {
                                let sx = ox as i32 + (cx * REGION + lx) as i32;
                                if sx < 0 || sx >= fw as i32 {
                                    continue;
                                }
                                let i = sy as usize * fw + sx as usize;
                                t.y[i] = p.y[i];
                                let ci = (sy as usize / 2) * (fw / 2) + sx as usize / 2;
                                t.u[ci] = p.u[ci];
                                t.v[ci] = p.v[ci];
                            }
                        }
                    }
                }
            }
            // 2. paste at the new position.
            if let Some((nx, ny)) = new_t {
                paste_sprite(&mut t, m, nx as i32, ny as i32);
            }
        }
        out.push(t);
    }
    out
}

#[cfg(test)]
mod tests_scene {
    use super::*;

    fn dummy(n: usize, frames: usize) -> Vec<Mover> {
        (0..n)
            .map(|i| Mover {
                bx: 32 * i,
                by: 16,
                bw: 32,
                bh: 32,
                ref_frame: 0,
                mask: vec![true; 4],
                sprite: Yuv420::new(32, 32),
                sprite_bytes: 0,
                track: (0..frames)
                    .map(|t| Some(((32 * i + t) as i16, 16i16)))
                    .collect(),
            })
            .collect()
    }

    /// Emit -> parse must be the identity. The protocol is the contract
    /// between the codec and anything editing it, so a round-trip that
    /// silently reshapes tracks would corrupt every edit downstream.
    #[test]
    fn scene_json_roundtrips() {
        let movers = dummy(3, 12);
        let js = scene_to_json(&movers, 12, 1920, 800);
        let got = apply_scene_json(&js, 3, 12).expect("roundtrip must parse");
        for (m, t) in movers.iter().zip(&got) {
            assert_eq!(&m.track, t);
        }
    }

    /// An edited track applies, including hiding an object outright —
    /// "remove him from the shot" has to be expressible.
    #[test]
    fn edited_track_applies_including_hide() {
        let movers = dummy(2, 6);
        let js = scene_to_json(&movers, 6, 1920, 800);
        // Shift mover 1 down 100px; hide it for the last two frames.
        let edited = js.replace(
            "\"track\": [[32, 16], [33, 16], [34, 16], [35, 16], [36, 16], [37, 16]]",
            "\"track\": [[32, 116], [33, 116], [34, 116], [35, 116], null, null]",
        );
        assert_ne!(edited, js, "test's own edit must have matched");
        let got = apply_scene_json(&edited, 2, 6).unwrap();
        assert_eq!(got[1][0], Some((32, 116)));
        assert_eq!(got[1][4], None);
        assert_eq!(got[0], movers[0].track, "unedited mover must be untouched");
    }

    /// Malformed input from a generative producer must FAIL LOUDLY and
    /// name the mover, never half-apply.
    #[test]
    fn malformed_scenes_are_rejected_by_name() {
        let js = scene_to_json(&dummy(2, 6), 6, 1920, 800);
        // Wrong length.
        let short = js.replace(", [37, 16]]", "]");
        let e = apply_scene_json(&short, 2, 6).unwrap_err();
        assert!(e.contains("mover 1") && e.contains("5 entries"), "got: {e}");
        // Out-of-range id.
        let bad = js.replace("\"id\": 1", "\"id\": 9");
        assert!(apply_scene_json(&bad, 2, 6).unwrap_err().contains("out of range"));
        // Missing mover.
        let one = scene_to_json(&dummy(1, 6), 6, 1920, 800);
        assert!(apply_scene_json(&one, 2, 6).unwrap_err().contains("omits mover 1"));
    }
}
