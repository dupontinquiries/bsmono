//! # v10 — strategy orchestrator
//!
//! The frontend that picks among prediction strategies per frame, 
//! sharing the transform + residual + library machinery rather than 
//! each strategy carrying its own.
//!
//! ## Status (Phase C)
//!
//! Implemented:
//! - StaticRepeat (1-byte marker for unchanged frames)
//! - MotionCompensated (global translation estimate + shift + residual)
//! - encode_frame() orchestration with per-strategy quick-estimate filter
//! - Win/loss stat tracking
//! - 'X' wrapper magic for v10 frames
//!
//! Not yet implemented:
//! - MeshRasterized — delegation to v8 (returns None for now)
//! - VolumetricRaymarched — delegation to v9 (returns None for now)
//! - main.rs hasn't switched its encode loop yet — opt-in via FAO_V10=1
//!
//! ## Wire format
//!
//! v10 frames start with magic byte 'X' (0x58) followed by a strategy
//! id and per-strategy payload:
//!
//! ```text
//! 'X' (1)
//! strategy_id (1) — 0=StaticRepeat, 1=MotionComp, 2=Mesh, 3=Volumetric
//! payload (variable)
//! ```
//!
//! Per-strategy payloads:
//!
//! - **StaticRepeat (0)**: empty. 2 bytes total.
//! - **MotionComp (1)**: `mvx i8 + mvy i8 + residual_len u32 + residual_bytes`
//! - **Mesh (2)**: TBD — will wrap v8 frame bytes inside v10 envelope
//! - **Volumetric (3)**: TBD
//!
//! ## Why a new wrapper magic
//!
//! The existing Z-wrapper carries v8 frames with optional bias-128 or
//! DCT residuals. Adding a fundamentally new frame type (motion-
//! compensated frames that reference a previous decoded frame instead
//! of a mesh) needs its own envelope to avoid colliding with the
//! existing decoder paths.
//!
//! 'X' was chosen because it doesn't conflict with any existing magic
//! ('V'=v8 raw, 'Z'=zstd-wrapped, 'D'=DCT v21, 'P'=YUV-split, 'L'=lossy
//! chroma, 'R'=repeat-marker, 'T'=translated). It also visually says
//! "this is a different thing."

#![allow(dead_code)]

use crate::residual::{self, ResidualOpts};

// ──────────────────────────────────────────────────────────────────
// Public types
// ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct EncodedFrame {
    pub bytes: Vec<u8>,
    pub predicted_rgb: Option<Vec<u8>>,
    pub psnr_db: Option<f32>,
    pub strategy: Strategy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    StaticRepeat,
    MotionCompensated,
    MeshRasterized,
    VolumetricRaymarched,
}

impl Strategy {
    fn id(self) -> u8 {
        match self {
            Strategy::StaticRepeat         => 0,
            Strategy::MotionCompensated    => 1,
            Strategy::MeshRasterized       => 2,
            Strategy::VolumetricRaymarched => 3,
        }
    }
    fn from_id(id: u8) -> Option<Strategy> {
        match id {
            0 => Some(Strategy::StaticRepeat),
            1 => Some(Strategy::MotionCompensated),
            2 => Some(Strategy::MeshRasterized),
            3 => Some(Strategy::VolumetricRaymarched),
            _ => None,
        }
    }
}

/// Wrapper magic for v10 frames.
pub const V10_MAGIC: u8 = b'X';

#[derive(Clone, Debug)]
pub struct Plan {
    pub enable_static_repeat: bool,
    pub enable_motion_comp: bool,
    pub enable_mesh: bool,
    pub enable_volumetric: bool,
    pub residual_opts: ResidualOpts,
    pub static_sad_threshold: u32,
    /// Skip a strategy entirely if its quick_estimate is > best_so_far × 
    /// this factor. Tuned so that strategies clearly losing don't pay 
    /// the full encode cost.
    pub quick_estimate_margin: f32,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            enable_static_repeat: true,
            enable_motion_comp: true,
            enable_mesh: true,
            enable_volumetric: false,
            residual_opts: ResidualOpts::default(),
            static_sad_threshold: 2,
            quick_estimate_margin: 1.5,
        }
    }
}

impl Plan {
    pub fn from_env() -> Self {
        fn env_on(name: &str) -> Option<bool> {
            std::env::var(name).ok().map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        }
        let mut plan = Self::default();
        if let Some(v) = env_on("FAO_V83_REPEAT")     { plan.enable_static_repeat = v; }
        if let Some(v) = env_on("FAO_V10_MOTIONCOMP") { plan.enable_motion_comp = v; }
        if let Some(v) = env_on("FAO_V10_MESH")       { plan.enable_mesh = v; }
        if let Some(v) = env_on("FAO_V9")             { plan.enable_volumetric = v; }
        plan.residual_opts = ResidualOpts::from_env();
        plan
    }
}

#[derive(Default)]
pub struct StrategyStats {
    pub static_wins: std::sync::atomic::AtomicU32,
    pub static_losses: std::sync::atomic::AtomicU32,
    pub motion_wins: std::sync::atomic::AtomicU32,
    pub motion_losses: std::sync::atomic::AtomicU32,
    pub mesh_wins: std::sync::atomic::AtomicU32,
    pub mesh_losses: std::sync::atomic::AtomicU32,
    pub volumetric_wins: std::sync::atomic::AtomicU32,
    pub volumetric_losses: std::sync::atomic::AtomicU32,
}

impl StrategyStats {
    pub fn record(&self, strategy: Strategy, won: bool) {
        use std::sync::atomic::Ordering::Relaxed;
        let counter = match (strategy, won) {
            (Strategy::StaticRepeat, true)          => &self.static_wins,
            (Strategy::StaticRepeat, false)         => &self.static_losses,
            (Strategy::MotionCompensated, true)     => &self.motion_wins,
            (Strategy::MotionCompensated, false)    => &self.motion_losses,
            (Strategy::MeshRasterized, true)        => &self.mesh_wins,
            (Strategy::MeshRasterized, false)       => &self.mesh_losses,
            (Strategy::VolumetricRaymarched, true)  => &self.volumetric_wins,
            (Strategy::VolumetricRaymarched, false) => &self.volumetric_losses,
        };
        counter.fetch_add(1, Relaxed);
    }
    
    pub fn report(&self) -> String {
        use std::sync::atomic::Ordering::Relaxed;
        let load = |c: &std::sync::atomic::AtomicU32| c.load(Relaxed);
        format!(
            "v10 strategy stats:\n  \
             static-repeat: {} wins, {} losses\n  \
             motion-comp:   {} wins, {} losses\n  \
             mesh:          {} wins, {} losses\n  \
             volumetric:    {} wins, {} losses",
            load(&self.static_wins),     load(&self.static_losses),
            load(&self.motion_wins),     load(&self.motion_losses),
            load(&self.mesh_wins),       load(&self.mesh_losses),
            load(&self.volumetric_wins), load(&self.volumetric_losses),
        )
    }
}

// ──────────────────────────────────────────────────────────────────
// StaticRepeat strategy
// ──────────────────────────────────────────────────────────────────

/// Quick estimate for static-repeat: returns Some(size) if the frame 
/// is genuinely static (SAD vs prev_rgb is below threshold), None 
/// otherwise.
/// 
/// Size of a static-repeat frame is fixed: 2 bytes (magic + strategy_id).
fn quick_estimate_static(
    plan: &Plan,
    frame_yuv: &[u8],
    prev_yuv: Option<&[u8]>,
    w: usize,
    h: usize,
) -> Option<usize> {
    let prev = prev_yuv?;
    if prev.len() < w * h || frame_yuv.len() < w * h { return None; }
    // SAD on Y plane only — fast, captures most of the signal.
    let mut sad: u64 = 0;
    let n_samples = (w * h / 256).max(64);
    let step = (w * h / n_samples).max(1);
    let mut samples = 0u32;
    let mut i = 0;
    while i < w * h {
        sad += (frame_yuv[i] as i32 - prev[i] as i32).unsigned_abs() as u64;
        samples += 1;
        i += step;
    }
    let avg_sad = if samples > 0 { sad / samples as u64 } else { 255 };
    if avg_sad <= plan.static_sad_threshold as u64 {
        Some(2)  // exactly the encoded size
    } else {
        None  // doesn't apply
    }
}

/// Encode a static-repeat frame. Output is exactly 2 bytes.
fn encode_static_repeat() -> Vec<u8> {
    vec![V10_MAGIC, Strategy::StaticRepeat.id()]
}

// ──────────────────────────────────────────────────────────────────
// MotionCompensated strategy
// ──────────────────────────────────────────────────────────────────

/// Estimate global motion + build predicted RGB by shifting prev_rgb. 
/// Returns (predicted_rgb, mvx, mvy) or None if estimation fails.
fn predict_motion(
    target_yuv: &[u8],
    prev_rgb: &[u8],
    w: usize,
    h: usize,
) -> Option<(Vec<u8>, i8, i8)> {
    if target_yuv.len() < w * h { return None; }
    if prev_rgb.len() != w * h * 3 { return None; }
    
    // Extract Y plane as f32 from both inputs. For target it's the 
    // first plane of YUV. For prev_rgb we convert via the BT.601 matrix.
    let mut target_y = vec![0.0f32; w * h];
    for i in 0..w * h {
        target_y[i] = target_yuv[i] as f32;
    }
    let mut prev_y = vec![0.0f32; w * h];
    for i in 0..w * h {
        let r = prev_rgb[i * 3] as f32;
        let g = prev_rgb[i * 3 + 1] as f32;
        let b = prev_rgb[i * 3 + 2] as f32;
        // BT.601 luma coefficients.
        prev_y[i] = 0.299 * r + 0.587 * g + 0.114 * b;
    }
    
    // Phase correlation / 3-step search → global motion estimate.
    let (mvx, mvy) = crate::estimate_global_motion(&prev_y, &target_y, w, h);
    
    // Shift prev_rgb by (mvx, mvy). Out-of-bounds pixels fall back to 
    // prev_rgb at the same position (clamped edge — prevents creating 
    // visible artifacts from black/zero padding at frame borders).
    let mut predicted = vec![0u8; w * h * 3];
    for py in 0..h {
        for px in 0..w {
            // Source pixel in prev_rgb that ended up here in target.
            let sx = px as i32 - mvx as i32;
            let sy = py as i32 - mvy as i32;
            let (sxc, syc) = (
                sx.clamp(0, w as i32 - 1) as usize,
                sy.clamp(0, h as i32 - 1) as usize,
            );
            let si = (syc * w + sxc) * 3;
            let di = (py * w + px) * 3;
            predicted[di]     = prev_rgb[si];
            predicted[di + 1] = prev_rgb[si + 1];
            predicted[di + 2] = prev_rgb[si + 2];
        }
    }
    Some((predicted, mvx, mvy))
}

/// Quick estimate for motion-comp: cheap proxy for the eventual encoded 
/// size. We don't run the full residual encode in quick_estimate; 
/// instead we measure SAD between target and motion-shifted-prev. 
/// Low SAD → small residual → small encoded size.
/// 
/// Calibration: a residual::encode with skip-block RLE typically 
/// produces ~3-5 bits per pixel of NON-skip area. For a frame where 
/// most blocks are skip-eligible (small SAD), the output is dominated 
/// by header overhead.
fn quick_estimate_motion(
    target_yuv: &[u8],
    prev_rgb: Option<&[u8]>,
    w: usize,
    h: usize,
) -> Option<usize> {
    let prev = prev_rgb?;
    if prev.len() != w * h * 3 { return None; }
    if target_yuv.len() < w * h { return None; }
    
    // We don't actually run motion estimation here — just compare 
    // target_y to prev_y at zero motion. If they're close, motion 
    // comp will work. If they're far apart (scene cut), we'll know 
    // motion comp would have a huge residual.
    let mut sad: u64 = 0;
    let mut samples = 0u32;
    let n_samples = 1024;
    let step = (w * h / n_samples).max(1);
    let mut i = 0;
    while i < w * h {
        let prev_y = (0.299 * prev[i*3] as f32 
                    + 0.587 * prev[i*3+1] as f32 
                    + 0.114 * prev[i*3+2] as f32) as i32;
        let cur_y = target_yuv[i] as i32;
        sad += (cur_y - prev_y).unsigned_abs() as u64;
        samples += 1;
        i += step;
    }
    let avg_sad = if samples > 0 { sad / samples as u64 } else { 255 };
    // Rough byte-count proxy: residual size scales roughly with SAD × pixel count.
    // Calibrated against typical results: 1 SAD-unit per pixel = ~0.1 bytes 
    // encoded after DCT + skip-RLE + zstd. So estimated size = 0.1 × SAD × pixels.
    let estimated = (avg_sad as f32 * 0.1 * (w * h) as f32 / 256.0) as usize;
    // Plus fixed overhead: magic(1) + strategy(1) + mvx(1) + mvy(1) + len(4) = 8 bytes.
    Some(estimated.max(64) + 8)
}

/// Encode a motion-compensated frame. Output:
///   'X' (1) + strategy_id=1 (1) + mvx i8 (1) + mvy i8 (1) + 
///   residual_len u32 (4) + residual_bytes (variable)
fn encode_motion_comp(
    target_yuv: &[u8],
    prev_rgb: &[u8],
    w: usize,
    h: usize,
    opts: &ResidualOpts,
) -> Option<EncodedFrame> {
    let (predicted_rgb, mvx, mvy) = predict_motion(target_yuv, prev_rgb, w, h)?;
    
    // Convert target YUV → RGB for the residual computation.
    let target_rgb = yuv_to_rgb(target_yuv, w, h)?;
    
    let residual = residual::encode(&predicted_rgb, &target_rgb, w, h, opts)?;
    
    let mut bytes = Vec::with_capacity(8 + residual.len());
    bytes.push(V10_MAGIC);
    bytes.push(Strategy::MotionCompensated.id());
    bytes.push(mvx as u8);
    bytes.push(mvy as u8);
    bytes.extend_from_slice(&(residual.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&residual);
    
    // PSNR estimate: residual::apply roundtrip would give exact answer.
    // For quick reporting we estimate from the SAD between predicted 
    // and target — the actual decoded output is between these.
    let mut sad: u64 = 0;
    for i in 0..target_rgb.len() {
        sad += (target_rgb[i] as i32 - predicted_rgb[i] as i32).unsigned_abs() as u64;
    }
    let mse = (sad as f64 / target_rgb.len() as f64).powi(2);
    let psnr = if mse < 1e-9 { 100.0 } else { 10.0 * (255.0_f64 * 255.0 / mse).log10() as f64 };
    
    Some(EncodedFrame {
        bytes,
        predicted_rgb: Some(target_rgb),  // for next frame's chain
        psnr_db: Some(psnr as f32),
        strategy: Strategy::MotionCompensated,
    })
}

// ──────────────────────────────────────────────────────────────────
// Orchestrator
// ──────────────────────────────────────────────────────────────────

/// Encode a single frame using whichever enabled strategy produces 
/// the smallest output. Returns None if no strategy succeeds (caller 
/// should fall back to legacy encoder path).
/// 
/// Algorithm:
/// 1. For each enabled strategy, run quick_estimate. Track best estimate.
/// 2. Drop strategies whose estimate exceeds best × plan.quick_estimate_margin.
/// 3. For surviving strategies, do the full encode.
/// 4. Pick the smallest actual result.
/// 5. Record win/loss in stats.
/// 
/// `prev_yuv` is the previous frame's YUV (used by quick_estimate for 
/// static-repeat — fast Y-plane SAD without converting prev_rgb).
/// `prev_rgb` is the previous frame's RGB after decoding (used by 
/// motion-comp for prediction synthesis).
pub fn encode_frame(
    plan: &Plan,
    stats: &StrategyStats,
    frame_yuv: &[u8],
    prev_yuv: Option<&[u8]>,
    prev_rgb: Option<&[u8]>,
    w: usize,
    h: usize,
) -> Option<EncodedFrame> {
    encode_frame_against(plan, stats, frame_yuv, prev_yuv, prev_rgb, w, h, None)
}

/// Like `encode_frame` but compares against an external baseline byte 
/// count. Returns `Some(v10_result)` only when at least one v10 strategy 
/// beats the baseline; otherwise returns `None` so the caller uses its 
/// own (cheaper) result.
/// 
/// This is the "v10 as an opportunistic enhancement to the existing 
/// encoder" pattern. The caller is presumed to already have a v8/v2/
/// k-frame result; v10 only emits bytes when it can win against that 
/// baseline.
/// 
/// `baseline_bytes` is the size (in bytes) of the caller\'s existing 
/// best result. `None` means "no baseline" — return v10\'s best regardless.
pub fn encode_frame_against(
    plan: &Plan,
    stats: &StrategyStats,
    frame_yuv: &[u8],
    prev_yuv: Option<&[u8]>,
    prev_rgb: Option<&[u8]>,
    w: usize,
    h: usize,
    baseline_bytes: Option<usize>,
) -> Option<EncodedFrame> {
    // Step 1: collect quick estimates for each enabled strategy.
    let mut candidates: Vec<(Strategy, usize)> = Vec::with_capacity(4);
    
    if plan.enable_static_repeat {
        if let Some(est) = quick_estimate_static(plan, frame_yuv, prev_yuv, w, h) {
            candidates.push((Strategy::StaticRepeat, est));
        }
    }
    if plan.enable_motion_comp && prev_rgb.is_some() {
        if let Some(est) = quick_estimate_motion(frame_yuv, prev_rgb, w, h) {
            candidates.push((Strategy::MotionCompensated, est));
        }
    }
    // Mesh + volumetric strategies are owned by v8/v9 directly. v10 
    // doesn\'t replicate them; instead the caller passes baseline_bytes 
    // (typically the v8 result size) and v10 only emits if a v10 
    // strategy beats it.
    
    if candidates.is_empty() {
        return None;
    }
    
    // Step 2: drop strategies clearly losing the v10-internal race.
    let best_est = candidates.iter().map(|(_, e)| *e).min().unwrap_or(usize::MAX);
    let margin = (best_est as f32 * plan.quick_estimate_margin) as usize;
    candidates.retain(|(_, est)| *est <= margin);
    
    // Step 2b: if baseline is provided and our best estimate already 
    // loses to it, bail without running full encodes. Saves work when 
    // v8 is clearly the right answer (rich 3D content).
    if let Some(baseline) = baseline_bytes {
        if best_est >= baseline {
            return None;
        }
    }
    
    // Step 3: run full encode for survivors.
    let mut results: Vec<EncodedFrame> = Vec::with_capacity(candidates.len());
    for (strategy, _est) in &candidates {
        let result = match strategy {
            Strategy::StaticRepeat => {
                Some(EncodedFrame {
                    bytes: encode_static_repeat(),
                    predicted_rgb: prev_rgb.map(|p| p.to_vec()),
                    psnr_db: None,
                    strategy: Strategy::StaticRepeat,
                })
            }
            Strategy::MotionCompensated => {
                if let Some(prgb) = prev_rgb {
                    encode_motion_comp(frame_yuv, prgb, w, h, &plan.residual_opts)
                } else {
                    None
                }
            }
            Strategy::MeshRasterized | Strategy::VolumetricRaymarched => {
                None  // owned by v8/v9
            }
        };
        if let Some(r) = result {
            results.push(r);
        }
    }
    
    if results.is_empty() {
        return None;
    }
    
    // Step 4: pick smallest of our results.
    let mut best_idx = 0;
    let mut best_size = results[0].bytes.len();
    for (i, r) in results.iter().enumerate().skip(1) {
        if r.bytes.len() < best_size {
            best_size = r.bytes.len();
            best_idx = i;
        }
    }
    
    // Step 5: if baseline beats our best, all our candidates lost — 
    // return None so caller uses their own bytes.
    if let Some(baseline) = baseline_bytes {
        if best_size >= baseline {
            for r in &results {
                stats.record(r.strategy, false);
            }
            return None;
        }
    }
    
    // Step 6: update stats. Winner = best_idx, losers = others.
    for (i, r) in results.iter().enumerate() {
        stats.record(r.strategy, i == best_idx);
    }
    
    Some(results.into_iter().nth(best_idx).unwrap())
}

// ──────────────────────────────────────────────────────────────────
// Decoder side
// ──────────────────────────────────────────────────────────────────

/// Decode a v10 frame. Returns the reconstructed RGB.
/// 
/// `prev_rgb` is the previous decoded RGB needed by motion-comp + 
/// static-repeat strategies. The caller (typically the main.rs decode 
/// loop) maintains this.
pub fn decode_frame(
    bytes: &[u8],
    prev_rgb: Option<&[u8]>,
    w: usize,
    h: usize,
) -> Option<Vec<u8>> {
    if bytes.len() < 2 || bytes[0] != V10_MAGIC { return None; }
    let strategy = Strategy::from_id(bytes[1])?;
    
    match strategy {
        Strategy::StaticRepeat => {
            // Same RGB as previous frame.
            prev_rgb.map(|p| p.to_vec())
        }
        Strategy::MotionCompensated => {
            if bytes.len() < 8 { return None; }
            let mvx = bytes[2] as i8;
            let mvy = bytes[3] as i8;
            let res_len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
            if bytes.len() < 8 + res_len { return None; }
            let residual_bytes = &bytes[8..8 + res_len];
            
            // Shift prev_rgb by (mvx, mvy) → predicted.
            let prev = prev_rgb?;
            if prev.len() != w * h * 3 { return None; }
            let mut predicted = vec![0u8; w * h * 3];
            for py in 0..h {
                for px in 0..w {
                    let sx = px as i32 - mvx as i32;
                    let sy = py as i32 - mvy as i32;
                    let sxc = sx.clamp(0, w as i32 - 1) as usize;
                    let syc = sy.clamp(0, h as i32 - 1) as usize;
                    let si = (syc * w + sxc) * 3;
                    let di = (py * w + px) * 3;
                    predicted[di]     = prev[si];
                    predicted[di + 1] = prev[si + 1];
                    predicted[di + 2] = prev[si + 2];
                }
            }
            
            // Apply residual.
            Some(residual::apply(predicted, residual_bytes, w, h))
        }
        Strategy::MeshRasterized | Strategy::VolumetricRaymarched => {
            // Phase D: integrate with v8/v9 decoders.
            None
        }
    }
}

// ──────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────

/// YUV420p → RGB conversion. BT.601 limited-range. Same math as 
/// v8_reconstruction_codec.rs::compute_photometric_residual; duplicated 
/// here so v10 doesn\'t depend on v8 internals.
fn yuv_to_rgb(yuv: &[u8], w: usize, h: usize) -> Option<Vec<u8>> {
    let y_sz = w * h;
    let uv_w = w / 2;
    let uv_h = h / 2;
    let uv_sz = uv_w * uv_h;
    if yuv.len() < y_sz + 2 * uv_sz { return None; }
    let mut rgb = vec![0u8; w * h * 3];
    for py in 0..h {
        for px in 0..w {
            let yv = yuv[py * w + px] as f32;
            let uv_off = (py / 2) * uv_w + (px / 2);
            let cu = yuv[y_sz + uv_off] as f32;
            let cv = yuv[y_sz + uv_sz + uv_off] as f32;
            let yn = (yv - 16.0) * (255.0 / 219.0);
            let un = cu - 128.0;
            let vn = cv - 128.0;
            let r = (yn + 1.402 * vn).clamp(0.0, 255.0) as u8;
            let g = (yn - 0.344136 * un - 0.714136 * vn).clamp(0.0, 255.0) as u8;
            let b = (yn + 1.772 * un).clamp(0.0, 255.0) as u8;
            let oi = (py * w + px) * 3;
            rgb[oi] = r;
            rgb[oi + 1] = g;
            rgb[oi + 2] = b;
        }
    }
    Some(rgb)
}

// ──────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn default_plan_is_sensible() {
        let p = Plan::default();
        assert!(p.enable_static_repeat);
        assert!(p.enable_mesh);
        assert!(!p.enable_volumetric, "volumetric must default off (opt-in)");
    }
    
    #[test]
    fn stats_record_increments() {
        let s = StrategyStats::default();
        s.record(Strategy::MeshRasterized, true);
        s.record(Strategy::MeshRasterized, false);
        s.record(Strategy::MeshRasterized, true);
        use std::sync::atomic::Ordering::Relaxed;
        assert_eq!(s.mesh_wins.load(Relaxed), 2);
        assert_eq!(s.mesh_losses.load(Relaxed), 1);
    }
    
    #[test]
    fn static_repeat_encode_decode() {
        let bytes = encode_static_repeat();
        assert_eq!(bytes.len(), 2);
        assert_eq!(bytes[0], V10_MAGIC);
        assert_eq!(bytes[1], Strategy::StaticRepeat.id());
        
        let prev = vec![42u8; 16 * 16 * 3];
        let decoded = decode_frame(&bytes, Some(&prev), 16, 16).expect("decode");
        assert_eq!(decoded, prev, "static-repeat should return prev_rgb unchanged");
    }
    
    #[test]
    fn motion_comp_roundtrip_zero_motion() {
        // No motion case: predicted == prev → residual is small but 
        // non-zero (due to YUV→RGB rounding). Test that decode produces 
        // something close to target.
        let w = 16; let h = 16;
        // Build a simple YUV frame (gray-ish, no chroma).
        let mut yuv = vec![128u8; w * h + 2 * (w/2) * (h/2)];
        for i in 0..w*h { yuv[i] = 100; }
        let prev_rgb = vec![100u8; w * h * 3];
        
        let opts = ResidualOpts::default();
        let result = encode_motion_comp(&yuv, &prev_rgb, w, h, &opts).expect("encode");
        assert_eq!(result.bytes[0], V10_MAGIC);
        assert_eq!(result.bytes[1], Strategy::MotionCompensated.id());
        
        let decoded = decode_frame(&result.bytes, Some(&prev_rgb), w, h).expect("decode");
        // Reconstruction should be close to target RGB.
        let target_rgb = yuv_to_rgb(&yuv, w, h).expect("yuv_to_rgb");
        let mean_err: f32 = decoded.iter().zip(target_rgb.iter())
            .map(|(&a, &b)| (a as f32 - b as f32).abs())
            .sum::<f32>() / decoded.len() as f32;
        assert!(mean_err < 10.0, "motion-comp reconstruction error {} too high", mean_err);
    }
    
    #[test]
    fn orchestrator_picks_static_when_identical() {
        let w = 16; let h = 16;
        let yuv = vec![100u8; w * h + 2 * (w/2) * (h/2)];
        let prev_yuv = yuv.clone();
        let prev_rgb = vec![100u8; w * h * 3];
        
        let plan = Plan::default();
        let stats = StrategyStats::default();
        let result = encode_frame(&plan, &stats, &yuv, Some(&prev_yuv), Some(&prev_rgb), w, h)
            .expect("encode");
        assert_eq!(result.strategy, Strategy::StaticRepeat);
        assert_eq!(result.bytes.len(), 2);
    }
    
    #[test]
    fn orchestrator_falls_back_to_motion_when_changed() {
        let w = 16; let h = 16;
        let prev_yuv = vec![100u8; w * h + 2 * (w/2) * (h/2)];
        let mut yuv = prev_yuv.clone();
        // Substantial change in Y → not static.
        for i in 0..w * h { yuv[i] = 180; }
        let prev_rgb = vec![100u8; w * h * 3];
        
        let plan = Plan::default();
        let stats = StrategyStats::default();
        let result = encode_frame(&plan, &stats, &yuv, Some(&prev_yuv), Some(&prev_rgb), w, h)
            .expect("encode");
        assert_eq!(result.strategy, Strategy::MotionCompensated);
    }
}
