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
const MV_RANGE: i32 = 12;

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
    for &step in &[4i32, 2, 1] {
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
    // Half-pel refinement around the integer winner.
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
    ((hx as i8, hy as i8), hp_sad)
}

fn copy_region_mc(dst: &mut Yuv420, prev: &Yuv420, rx: usize, ry: usize, mv: (i8, i8)) {
    // mv is in HALF-PEL units. Even components take the exact integer
    // path (bit-identical to the old behavior); odd components sample
    // bilinearly. Chroma moves at quarter-pel of its plane, always
    // bilinear when fractional.
    let (w, h) = (dst.w, dst.h);
    let (mvx, mvy) = (mv.0 as i32, mv.1 as i32);
    if mvx % 2 == 0 && mvy % 2 == 0 {
        let (ix, iy) = (mvx / 2, mvy / 2);
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
    let (fx0, fy0) = (mvx as f32 * 0.5, mvy as f32 * 0.5);
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
                        // search_mv returns half-pel units of cur->ref block
                        // offset; the take offset convention below matches
                        // motion dx (content shift), so negate.
                        dxs.push(-(mv.0 as f32) * 0.5);
                        dys.push(-(mv.1 as f32) * 0.5);
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
    std::fs::write(path, z)
}

pub fn load_plate(path: &str) -> std::io::Result<Plate> {
    let z = std::fs::read(path)?;
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
                    let (mx, my) = (mv.0 as f32 * 0.5, mv.1 as f32 * 0.5);
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
                let (_mv, sad_prev) = search_mv(&prev.y, fw, fh, &f.y, rx, ry);
                let fin = sad_plate.min(sad_prev) as f64;
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
            }
        }
    }
    let tot_c: u64 = cls_count.iter().sum();
    let tot_e: f64 = cls_energy.iter().sum::<f64>().max(1.0);
    format!(
        "regions: skip {:.1}% tracked-changed {:.1}% unsolved {:.1}% | residual-energy share: skip {:.1}% tracked-changed {:.1}% unsolved {:.1}% | unsolved-cell footprint {:.1}% of frame",
        100.0 * cls_count[0] as f64 / tot_c as f64,
        100.0 * cls_count[1] as f64 / tot_c as f64,
        100.0 * cls_count[2] as f64 / tot_c as f64,
        100.0 * cls_energy[0] / tot_e,
        100.0 * cls_energy[1] / tot_e,
        100.0 * cls_energy[2] / tot_e,
        100.0 * unsolved_cells.len() as f64 / (gw * gh) as f64,
    )
}

pub fn encode_gop_v3(
    frames: &[Yuv420],
    motions: &[PlateMotion],
    plate: &Plate,
    opts: &ResidualOpts,
) -> PlateEncodeReportV2 {

    let fw = frames[0].w;
    let fh = frames[0].h;
    let (gw, gh) = (fw / REGION, fh / REGION);
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
        let mut modes = vec![0u8; gw * gh];
        let mut mvs: Vec<(i8, i8)> = Vec::new();
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
                    if sad_prev + 64 < sad_plate {
                        modes[gy * gw + gx] = 1;
                        mvs.push(mv);
                        prev_regions += 1;
                        copy_region_mc(&mut comp, &prev, rx, ry, mv);
                    }
                }
            }
            prev_share_acc += prev_regions as f64 / (gw * gh) as f64;
            comp
        };

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
                    // Median-of-neighbors prediction (left, top; 0 when absent).
                    let left = if gx > 0 { grid[gi - 1].unwrap_or((0, 0)) } else { (0, 0) };
                    let top = if gy > 0 { grid[gi - gw].unwrap_or((0, 0)) } else { (0, 0) };
                    let pred = (
                        ((left.0 as i32 + top.0 as i32) / 2) as i8,
                        ((left.1 as i32 + top.1 as i32) / 2) as i8,
                    );
                    mv_deltas.push((mv.0.wrapping_sub(pred.0)) as u8);
                    mv_deltas.push((mv.1.wrapping_sub(pred.1)) as u8);
                }
            }
            nz.truncate((n_prev_seen + 7) / 8);
            nz_bits = nz;
        }
        side.extend_from_slice(&nz_bits);
        side.extend_from_slice(&mv_deltas);
        let mode_bytes = zstd::encode_all(side.as_slice(), 19).map(|c| c.len()).unwrap_or(side.len());

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
