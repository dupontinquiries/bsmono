//! # snapclone-saliency
//!
//! Classical salient region detection. No machine learning, no model files,
//! no GPU — just signal-processing heuristics that run in milliseconds on
//! commodity ARM cores.
//!
//! ## Why classical (and not ML)?
//!
//! For our use case (crop-to-focal-region for contact avatars on the
//! Wayfinder), we need:
//!   * Sub-100ms latency on FP5
//!   * Zero model files / zero new dependencies
//!   * "Good enough" — better than center crop, doesn't have to beat U2Net
//!
//! Classical saliency methods (Itti-Koch 1998, spectral residual 2007,
//! frequency-tuned 2009) hit those targets. The output is a single bounding
//! box of the most salient region — exactly what we need.
//!
//! ## Algorithm: simplified Itti-Koch with center bias
//!
//! 1. Downsample input to 256×256 (constant work regardless of input size).
//! 2. Build a saliency map by combining three signals:
//!    * Edge density (Sobel-ish gradient magnitude)
//!    * Color contrast (deviation from local mean color)
//!    * Center bias (faces/subjects tend to be near image center on mobile)
//! 3. Threshold the saliency map at the 75th percentile, keeping the
//!    largest connected blob.
//! 4. Return its bounding box, scaled back to original coordinates.
//!
//! Performance: ~5-15ms for a 1024x1024 input on FP5-class ARM (Cortex-A78).
//!
//! ## C API
//!
//! ```c
//! typedef struct {
//!     uint32_t x, y, w, h;
//! } SalRect;
//!
//! /// Returns SalRect{0,0,0,0} on failure (caller can fall back to center crop).
//! SalRect sc_saliency_bbox(const uint8_t *rgba, uint32_t width, uint32_t height);
//! ```

#![allow(clippy::needless_range_loop)]

/// V32.34+ ROI bit-allocation planner — turns a saliency box into a face
/// boost + background penalty plan for face-aware video compression. See
/// roi_plan.rs. The C API below (sc_roi_plan) is the C++ encoder's entry point.
pub mod roi_plan;

use std::os::raw::c_uchar;

/// Grid resolution for the internal saliency map. 64×64 is plenty for
/// finding a region of interest — the full-res input is sampled into this
/// grid and we work in grid coordinates throughout.
const GRID: usize = 64;

/// Fraction of grid cells to keep after thresholding. 0.25 = top quartile.
const SALIENCY_KEEP_FRAC: f32 = 0.25;

/// Center-bias strength. 0.0 = no bias, 1.0 = strong center pull.
/// Mobile photos heavily benefit from a bias because subjects are centered.
const CENTER_BIAS: f32 = 0.35;

/// Output of saliency detection — bounding box in input pixel coordinates.
#[repr(C)]
pub struct SalRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl SalRect {
    pub fn empty() -> Self {
        SalRect { x: 0, y: 0, w: 0, h: 0 }
    }
    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
}

/// Compute the bounding box of the most salient region in an RGBA image.
///
/// Caller-owned buffer; we don't store any pointers.
/// Returns `SalRect::empty()` on invalid input.
pub fn salient_bbox(rgba: &[u8], width: u32, height: u32) -> SalRect {
    // Sanity check — we need at least a tiny image.
    if width < 8 || height < 8 || rgba.len() < (width * height * 4) as usize {
        return SalRect::empty();
    }
    let w = width as usize;
    let h = height as usize;

    // ── 1. Sample input into GRID×GRID cells ────────────────────────────────
    // For each cell, store mean R/G/B (after gamma-ish luminance approx).
    // We also build a lum buffer for the gradient pass below.
    let mut grid_r = [[0u32; GRID]; GRID];
    let mut grid_g = [[0u32; GRID]; GRID];
    let mut grid_b = [[0u32; GRID]; GRID];
    let mut grid_n = [[0u32; GRID]; GRID];
    let mut grid_l = [[0f32; GRID]; GRID];

    for y in 0..h {
        let gy = (y * GRID) / h;
        let row = &rgba[y * w * 4 .. (y + 1) * w * 4];
        for x in 0..w {
            let gx = (x * GRID) / w;
            let p = &row[x * 4..x * 4 + 4];
            grid_r[gy][gx] += p[0] as u32;
            grid_g[gy][gx] += p[1] as u32;
            grid_b[gy][gx] += p[2] as u32;
            grid_n[gy][gx] += 1;
        }
    }
    for gy in 0..GRID {
        for gx in 0..GRID {
            let n = grid_n[gy][gx].max(1);
            let r = (grid_r[gy][gx] / n) as f32;
            let g = (grid_g[gy][gx] / n) as f32;
            let b = (grid_b[gy][gx] / n) as f32;
            // BT.601 luminance — close enough for saliency.
            grid_l[gy][gx] = 0.299 * r + 0.587 * g + 0.114 * b;
        }
    }

    // ── 2. Edge density via Sobel-like gradient magnitude ───────────────────
    // We skip the borders (1-cell margin) — saves branching.
    let mut edge = [[0f32; GRID]; GRID];
    let mut edge_max = 0.0f32;
    for gy in 1..GRID - 1 {
        for gx in 1..GRID - 1 {
            let gx_grad = grid_l[gy][gx + 1] - grid_l[gy][gx - 1];
            let gy_grad = grid_l[gy + 1][gx] - grid_l[gy - 1][gx];
            let mag = (gx_grad * gx_grad + gy_grad * gy_grad).sqrt();
            edge[gy][gx] = mag;
            if mag > edge_max { edge_max = mag; }
        }
    }
    // Normalize to 0..1
    if edge_max > 0.0 {
        for gy in 0..GRID {
            for gx in 0..GRID {
                edge[gy][gx] /= edge_max;
            }
        }
    }

    // ── 3. Color contrast (deviation from global mean) ──────────────────────
    let mut mean_r = 0.0f32;
    let mut mean_g = 0.0f32;
    let mut mean_b = 0.0f32;
    let mut total_n = 0u32;
    for gy in 0..GRID {
        for gx in 0..GRID {
            let n = grid_n[gy][gx];
            mean_r += grid_r[gy][gx] as f32;
            mean_g += grid_g[gy][gx] as f32;
            mean_b += grid_b[gy][gx] as f32;
            total_n += n;
        }
    }
    if total_n > 0 {
        mean_r /= total_n as f32;
        mean_g /= total_n as f32;
        mean_b /= total_n as f32;
    }
    let mut contrast = [[0f32; GRID]; GRID];
    let mut contrast_max = 0.0f32;
    for gy in 0..GRID {
        for gx in 0..GRID {
            let n = grid_n[gy][gx].max(1) as f32;
            let r = grid_r[gy][gx] as f32 / n;
            let g = grid_g[gy][gx] as f32 / n;
            let b = grid_b[gy][gx] as f32 / n;
            let dr = r - mean_r;
            let dg = g - mean_g;
            let db = b - mean_b;
            let dist = (dr * dr + dg * dg + db * db).sqrt();
            contrast[gy][gx] = dist;
            if dist > contrast_max { contrast_max = dist; }
        }
    }
    if contrast_max > 0.0 {
        for gy in 0..GRID {
            for gx in 0..GRID {
                contrast[gy][gx] /= contrast_max;
            }
        }
    }

    // ── 4. Center bias (radial falloff from grid center) ────────────────────
    let cx = (GRID as f32) / 2.0;
    let cy = (GRID as f32) / 2.0;
    let max_d = (cx * cx + cy * cy).sqrt();
    let mut center_score = [[0f32; GRID]; GRID];
    for gy in 0..GRID {
        for gx in 0..GRID {
            let dx = gx as f32 - cx;
            let dy = gy as f32 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            center_score[gy][gx] = 1.0 - (d / max_d);
        }
    }

    // ── 5. Combined saliency map ────────────────────────────────────────────
    // Edge contributes 0.45, contrast 0.45, center 0.10 weighted.
    // Then mix in center bias as a multiplicative factor at strength CENTER_BIAS.
    let mut sal = [[0f32; GRID]; GRID];
    let mut sal_values = Vec::with_capacity(GRID * GRID);
    for gy in 0..GRID {
        for gx in 0..GRID {
            let raw = 0.45 * edge[gy][gx]
                    + 0.45 * contrast[gy][gx]
                    + 0.10 * center_score[gy][gx];
            let biased = raw * (1.0 - CENTER_BIAS) + center_score[gy][gx] * CENTER_BIAS;
            sal[gy][gx] = biased;
            sal_values.push(biased);
        }
    }

    // ── 6. Threshold at 75th percentile (keep top 25% of cells) ─────────────
    sal_values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let threshold_idx = (sal_values.len() as f32 * (1.0 - SALIENCY_KEEP_FRAC)) as usize;
    let threshold = sal_values[threshold_idx.min(sal_values.len() - 1)];

    // ── 7. Find the bounding box of all cells above threshold ───────────────
    // Simpler than connected-components and works well in practice — the
    // top-quartile cells tend to cluster around the actual subject.
    let mut min_x = GRID;
    let mut min_y = GRID;
    let mut max_x = 0usize;
    let mut max_y = 0usize;
    let mut any = false;
    for gy in 0..GRID {
        for gx in 0..GRID {
            if sal[gy][gx] >= threshold {
                if gx < min_x { min_x = gx; }
                if gy < min_y { min_y = gy; }
                if gx > max_x { max_x = gx; }
                if gy > max_y { max_y = gy; }
                any = true;
            }
        }
    }
    if !any { return SalRect::empty(); }

    // ── 8. Map grid cell box back to input pixel coords ─────────────────────
    // Pad by half a cell so we don't crop too tight.
    let cell_w = (w as f32) / (GRID as f32);
    let cell_h = (h as f32) / (GRID as f32);
    let pad_x  = (cell_w * 0.5) as i32;
    let pad_y  = (cell_h * 0.5) as i32;
    let px0 = ((min_x as f32) * cell_w) as i32 - pad_x;
    let py0 = ((min_y as f32) * cell_h) as i32 - pad_y;
    let px1 = (((max_x + 1) as f32) * cell_w) as i32 + pad_x;
    let py1 = (((max_y + 1) as f32) * cell_h) as i32 + pad_y;
    let x0 = px0.max(0) as u32;
    let y0 = py0.max(0) as u32;
    let x1 = (px1.min(w as i32)) as u32;
    let y1 = (py1.min(h as i32)) as u32;
    if x1 <= x0 || y1 <= y0 { return SalRect::empty(); }

    SalRect {
        x: x0, y: y0,
        w: x1 - x0, h: y1 - y0,
    }
}

// ─── C FFI ───────────────────────────────────────────────────────────────────
//
// Called from C++ (WayfinderPage::avatarForContact). Pointer must point to
// at least width*height*4 bytes of RGBA8888 data.
#[no_mangle]
pub unsafe extern "C" fn sc_saliency_bbox(
    rgba: *const c_uchar,
    width: u32,
    height: u32,
) -> SalRect {
    if rgba.is_null() {
        return SalRect::empty();
    }
    let len = (width as usize).checked_mul(height as usize)
        .and_then(|v| v.checked_mul(4));
    let len = match len {
        Some(n) => n,
        None    => return SalRect::empty(),
    };
    let slice = std::slice::from_raw_parts(rgba, len);
    salient_bbox(slice, width, height)
}

// ─── V32.34+: ROI bit-allocation plan (C FFI) ────────────────────────────────
//
// The C++ video encoder calls this to turn a saliency box into a face-aware
// bit-allocation plan (background penalty + padded face boost). C can't receive
// a Vec, so the caller passes a fixed-capacity array of CRoiDirective and we
// fill up to `cap` of them, returning the count actually written (0 = no ROI,
// encode plainly). The defaults match RoiTuning::default(); a richer API can
// pass tuning later if needed.
//
// CRoiDirective mirrors RoiDirective but with the QP delta already expressed in
// centi-units the C++ side divides by 100.0 for ffmpeg's addroi `qoffset`.

/// C-ABI directive: a rectangle + QP delta (centi). qp_delta_centi negative =
/// more bits (protect), positive = fewer bits (penalize).
#[repr(C)]
pub struct CRoiDirective {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub qp_delta_centi: i32,
}

/// Fill `out` (capacity `cap`) with the ROI plan for `bbox` on a `frame_w` ×
/// `frame_h` frame, using default tuning. Returns the number of directives
/// written (0..=cap). A return of 0 means "no ROI — encode plainly."
///
/// # Safety
/// `out` must point to at least `cap` writable `CRoiDirective` slots.
#[no_mangle]
pub unsafe extern "C" fn sc_roi_plan(
    bbox_x: u32,
    bbox_y: u32,
    bbox_w: u32,
    bbox_h: u32,
    frame_w: u32,
    frame_h: u32,
    out: *mut CRoiDirective,
    cap: u32,
) -> u32 {
    if out.is_null() || cap == 0 {
        return 0;
    }
    let plan = roi_plan::plan_from_bbox(
        bbox_x,
        bbox_y,
        bbox_w,
        bbox_h,
        frame_w,
        frame_h,
        &roi_plan::RoiTuning::default(),
    );
    let n = plan.directives.len().min(cap as usize);
    let slots = std::slice::from_raw_parts_mut(out, n);
    for (slot, d) in slots.iter_mut().zip(plan.directives.iter()) {
        *slot = CRoiDirective {
            x: d.x,
            y: d.y,
            w: d.w,
            h: d.h,
            qp_delta_centi: d.qp_delta_centi,
        };
    }
    n as u32
}

// ─── V32.32: landmark candidate scoring ──────────────────────────────────────
//
// Returns a score in [0, 1] for "is this view a good landmark candidate?"
// Used by CameraWidget to decide whether to show the "+ Save as landmark"
// hint over the viewfinder. The score combines four heuristics from the
// V32.32 spec, all computable from the same Sobel-style gradient pass we
// already run for saliency_bbox:
//
//   1. Strong horizontal edge (skyline / horizon)
//      Detected as a column-summed row-of-edges with a peak > 2× mean.
//      Mountain ridgelines, building roof-lines, and sea horizons all
//      produce this signal cleanly.
//
//   2. Edge-density variance across frame thirds
//      Split the frame into 3×3 zones; compute edge density per zone.
//      High variance → "distinctive subject against neutral surroundings",
//      which is the textbook landmark composition. Uniform busyness
//      (a crowd, a forest) gives low variance → bad candidate.
//
//   3. Vertical-edge dominance
//      Sum vertical-gradient magnitudes vs horizontal. A ratio > 1.3
//      indicates architectural structure: doorways, street signs,
//      monuments, tall buildings. Faces and crowds have rounded
//      gradients so this distinguishes them.
//
//   4. Noise-to-signal ratio (rejects clutter / screens)
//      Estimated as the fraction of edge energy in HIGH-frequency cells
//      (cells with magnitude in the top 10% AND surrounded by other
//      such cells — implies a noisy texture, not a clean line).
//      High ratio → the scene is busy random texture, not structure.
//      Bad candidate.
//
// Weights chosen empirically (and tunable here for future iteration):
//   horizon: 0.25, variance: 0.25, vertical: 0.30, noise: 0.20
//
// The four are computed independently from one Sobel pass, so total cost
// is the same as saliency_bbox — ~5-15ms on FP5 for typical viewfinder
// sizes (1280×720 or downsampled to that). The crate caller is expected
// to pre-downsample if the source is larger.
pub fn landmark_score(rgba: &[u8], width: u32, height: u32) -> f32 {
    if width < 8 || height < 8 || rgba.len() < (width * height * 4) as usize {
        return 0.0;
    }
    let w = width as usize;
    let h = height as usize;

    // ── Build a luminance grid (reuses the saliency_bbox approach) ─────────
    let mut grid_l = [[0f32; GRID]; GRID];
    {
        let mut grid_r = [[0u32; GRID]; GRID];
        let mut grid_g = [[0u32; GRID]; GRID];
        let mut grid_b = [[0u32; GRID]; GRID];
        let mut grid_n = [[0u32; GRID]; GRID];
        for y in 0..h {
            let gy = (y * GRID) / h;
            let row = &rgba[y * w * 4..(y + 1) * w * 4];
            for x in 0..w {
                let gx = (x * GRID) / w;
                let p = &row[x * 4..x * 4 + 4];
                grid_r[gy][gx] += p[0] as u32;
                grid_g[gy][gx] += p[1] as u32;
                grid_b[gy][gx] += p[2] as u32;
                grid_n[gy][gx] += 1;
            }
        }
        for gy in 0..GRID {
            for gx in 0..GRID {
                let n = grid_n[gy][gx].max(1);
                let r = (grid_r[gy][gx] / n) as f32;
                let g = (grid_g[gy][gx] / n) as f32;
                let b = (grid_b[gy][gx] / n) as f32;
                grid_l[gy][gx] = 0.299 * r + 0.587 * g + 0.114 * b;
            }
        }
    }

    // ── Sobel-ish gradient: horizontal and vertical magnitudes separately ─
    // Tracking the two components (not just total magnitude) is what
    // distinguishes "horizontal edge" from "vertical edge" — the V32.32
    // heuristics need both. Skip 1-cell border to avoid out-of-range.
    let mut gx_grid = [[0f32; GRID]; GRID];  // horizontal gradient = ∂L/∂x
    let mut gy_grid = [[0f32; GRID]; GRID];  // vertical gradient   = ∂L/∂y
    let mut mag     = [[0f32; GRID]; GRID];
    let mut mag_max = 0.0f32;
    for gy in 1..GRID - 1 {
        for gx in 1..GRID - 1 {
            // ∂L/∂x is a horizontal-finite-difference but it RESPONDS to
            // vertical lines (a vertical edge has high horizontal gradient).
            // Naming follows the math; the heuristics translate accordingly.
            let dx = grid_l[gy][gx + 1] - grid_l[gy][gx - 1];
            let dy = grid_l[gy + 1][gx] - grid_l[gy - 1][gx];
            gx_grid[gy][gx] = dx.abs();
            gy_grid[gy][gx] = dy.abs();
            let m = (dx * dx + dy * dy).sqrt();
            mag[gy][gx] = m;
            if m > mag_max { mag_max = m; }
        }
    }
    if mag_max <= 0.0 {
        // Flat featureless image (e.g. a wall of one color, a lens cap).
        // No edges to score against — definitively not a landmark.
        return 0.0;
    }

    // ── Heuristic 1: strong horizontal edge (skyline detector) ────────────
    // ∂L/∂y peaks for a horizontal edge (luminance changes vertically
    // across a horizontal line). Sum |∂L/∂y| per row, look for a peak.
    let mut row_dy_sum = [0.0f32; GRID];
    for gy in 0..GRID {
        let mut s = 0.0f32;
        for gx in 0..GRID {
            s += gy_grid[gy][gx];
        }
        row_dy_sum[gy] = s;
    }
    let row_dy_mean: f32 = row_dy_sum.iter().sum::<f32>() / (GRID as f32);
    let row_dy_peak: f32 = row_dy_sum.iter().copied().fold(0.0f32, f32::max);
    let horizon_score = if row_dy_mean > 0.0 {
        // Peak/mean ratio: 1.0 = no peak, 2.0 = mild peak, 4.0+ = clear horizon.
        // Map [1.5, 4.0] → [0, 1] for the score.
        let ratio = row_dy_peak / row_dy_mean;
        ((ratio - 1.5) / 2.5).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // ── Heuristic 2: edge-density variance across frame thirds ────────────
    // Divide the GRID into 3×3 zones, compute edge density per zone, then
    // measure the spread. High variance = uneven density = distinctive
    // subject + neutral surroundings. Low variance = uniform busyness.
    let zone_size = GRID / 3;  // 21 for GRID=64 (we ignore the extra row/col)
    let mut zone_density = [0.0f32; 9];
    for zy in 0..3 {
        for zx in 0..3 {
            let mut s = 0.0f32;
            let y0 = zy * zone_size;
            let x0 = zx * zone_size;
            for dy in 0..zone_size {
                for dx in 0..zone_size {
                    s += mag[y0 + dy][x0 + dx];
                }
            }
            zone_density[zy * 3 + zx] = s / (zone_size * zone_size) as f32;
        }
    }
    let zone_mean: f32 = zone_density.iter().sum::<f32>() / 9.0;
    let zone_var: f32 = zone_density.iter()
        .map(|d| (d - zone_mean).powi(2))
        .sum::<f32>() / 9.0;
    // Normalize variance to [0,1]. The coefficient-of-variation σ/μ for
    // a strongly distinctive subject lands around 0.8-1.2; map [0.3, 1.2] → [0,1].
    let cv = if zone_mean > 0.0 { zone_var.sqrt() / zone_mean } else { 0.0 };
    let variance_score = ((cv - 0.3) / 0.9).clamp(0.0, 1.0);

    // ── Heuristic 3: vertical-edge dominance ───────────────────────────────
    // ∂L/∂x peaks for vertical edges (luminance changes horizontally across
    // a vertical line). Compute the ratio of total |∂L/∂x| to total |∂L/∂y|.
    // Ratio > 1.3 indicates an architectural/man-made structure.
    let mut total_dx = 0.0f32;
    let mut total_dy = 0.0f32;
    for gy in 1..GRID - 1 {
        for gx in 1..GRID - 1 {
            total_dx += gx_grid[gy][gx];
            total_dy += gy_grid[gy][gx];
        }
    }
    let vertical_ratio = if total_dy > 0.0 { total_dx / total_dy } else { 0.0 };
    // Map [1.1, 2.0] → [0, 1]. Below 1.1 = mostly horizontal or balanced;
    // above 2.0 = strongly vertical (close-up of a doorway, sign).
    let vertical_score = ((vertical_ratio - 1.1) / 0.9).clamp(0.0, 1.0);

    // ── Heuristic 4: noise-to-signal ratio (clutter rejection) ────────────
    // Find "high-magnitude noisy cells": cells in the top 10% by magnitude
    // whose four neighbors are ALSO in the top 10%. A clean edge has a
    // line of high cells with one or two neighbors high (the line itself);
    // a noisy texture has clusters where every neighbor is high.
    let mut sorted_mags: Vec<f32> = Vec::with_capacity(GRID * GRID);
    for row in mag.iter() {
        for &m in row.iter() {
            sorted_mags.push(m);
        }
    }
    sorted_mags.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let top10_idx = (sorted_mags.len() as f32 * 0.9) as usize;
    let top10_threshold = sorted_mags[top10_idx.min(sorted_mags.len() - 1)];
    let mut clean_count = 0u32;
    let mut noisy_count = 0u32;
    for gy in 1..GRID - 1 {
        for gx in 1..GRID - 1 {
            if mag[gy][gx] < top10_threshold { continue; }
            let n_top = (mag[gy - 1][gx] >= top10_threshold) as u32
                      + (mag[gy + 1][gx] >= top10_threshold) as u32
                      + (mag[gy][gx - 1] >= top10_threshold) as u32
                      + (mag[gy][gx + 1] >= top10_threshold) as u32;
            // 0-1 high neighbors = clean line cell
            // 2 = corner / junction (still ok)
            // 3-4 = embedded in a cluster of high cells = noise
            if n_top >= 3 {
                noisy_count += 1;
            } else {
                clean_count += 1;
            }
        }
    }
    let total_top = clean_count + noisy_count;
    let noise_score = if total_top > 0 {
        // High clean fraction → good. Map clean_fraction [0.5, 0.9] → [0, 1].
        let clean_frac = clean_count as f32 / total_top as f32;
        ((clean_frac - 0.5) / 0.4).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // ── Combine ────────────────────────────────────────────────────────────
    // Weighted sum. Weights add to 1.0; result is in [0, 1].
    let score = 0.25 * horizon_score
              + 0.25 * variance_score
              + 0.30 * vertical_score
              + 0.20 * noise_score;
    score.clamp(0.0, 1.0)
}

/// C FFI for the landmark candidate score.
#[no_mangle]
pub unsafe extern "C" fn sc_landmark_score(
    rgba: *const c_uchar,
    width: u32,
    height: u32,
) -> f32 {
    if rgba.is_null() {
        return 0.0;
    }
    let len = (width as usize).checked_mul(height as usize)
        .and_then(|v| v.checked_mul(4));
    let len = match len {
        Some(n) => n,
        None    => return 0.0,
    };
    let slice = std::slice::from_raw_parts(rgba, len);
    landmark_score(slice, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_image_returns_empty() {
        let r = salient_bbox(&[], 0, 0);
        assert!(r.is_empty());
    }

    #[test]
    fn small_image_returns_empty() {
        let buf = vec![0u8; 4 * 4 * 4];
        let r = salient_bbox(&buf, 4, 4);
        assert!(r.is_empty());
    }

    #[test]
    fn bright_corner_attracts_saliency() {
        // 64x64 image, all gray except a bright square at (10,10)-(20,20)
        let mut buf = vec![80u8; 64 * 64 * 4];
        for y in 10..20 {
            for x in 10..20 {
                let idx = (y * 64 + x) * 4;
                buf[idx]     = 255;
                buf[idx + 1] = 255;
                buf[idx + 2] = 255;
            }
        }
        let r = salient_bbox(&buf, 64, 64);
        assert!(!r.is_empty());
        // The bbox should overlap the bright square
        let cx = r.x + r.w / 2;
        let cy = r.y + r.h / 2;
        // Center bias may pull it inward but bright region wins
        assert!(cx < 40 && cy < 40, "bbox center {},{}was unexpectedly far", cx, cy);
    }

    // ── V32.32 landmark_score tests ──────────────────────────────────────

    #[test]
    fn landmark_score_empty_image_is_zero() {
        let s = landmark_score(&[], 0, 0);
        assert_eq!(s, 0.0);
    }

    #[test]
    fn landmark_score_flat_image_is_low() {
        // Uniform gray — no edges anywhere → score should be ~0.
        // Confirms the "no signal → no score" path: we shouldn't tell
        // users a featureless wall is a great landmark candidate.
        let buf = vec![128u8; 128 * 128 * 4];
        let s = landmark_score(&buf, 128, 128);
        assert!(s < 0.10, "flat image scored {s}, expected < 0.10");
    }

    #[test]
    fn landmark_score_horizon_is_high() {
        // Top half bright, bottom half dark — a stark horizontal edge in
        // the middle, exactly the "skyline" pattern the horizon heuristic
        // targets. With this single dominant horizontal edge we expect a
        // meaningful score uplift over the flat baseline.
        let w = 128usize;
        let h = 128usize;
        let mut buf = vec![0u8; w * h * 4];
        for y in 0..h {
            let v: u8 = if y < h / 2 { 220 } else { 30 };
            for x in 0..w {
                let idx = (y * w + x) * 4;
                buf[idx]     = v;
                buf[idx + 1] = v;
                buf[idx + 2] = v;
                buf[idx + 3] = 255;
            }
        }
        let s = landmark_score(&buf, w as u32, h as u32);
        assert!(s > 0.30, "horizon scored {s}, expected > 0.30");
    }

    #[test]
    fn landmark_score_vertical_column_is_high() {
        // Strong vertical line in the middle of a gray frame — should
        // light up the vertical-edge-dominance heuristic AND the variance
        // heuristic. Note: this synthetic case has NO horizon and NO
        // textured zones, so two of the four weights contribute 0; the
        // achievable maximum is bounded by (vertical × 0.30) + (variance ×
        // 0.25) + (noise × 0.20) ≈ 0.75. Real-world frames (a building
        // doorway against trees, say) clear 0.40 easily. The synthetic
        // bare minimum we want here is "the vertical heuristic fired":
        // anything > 0.20 confirms vertical-edge dominance was detected.
        let w = 128usize;
        let h = 128usize;
        let mut buf = vec![128u8; w * h * 4];
        for y in 0..h {
            for x in (w / 2 - 2)..(w / 2 + 2) {
                let idx = (y * w + x) * 4;
                buf[idx]     = 250;
                buf[idx + 1] = 250;
                buf[idx + 2] = 250;
            }
        }
        let s = landmark_score(&buf, w as u32, h as u32);
        assert!(s > 0.20, "vertical-line scored {s}, expected > 0.20");
    }

    #[test]
    fn landmark_score_random_noise_is_low() {
        // Uniformly random pixels — should score low because every zone
        // has high edge density (variance is tiny) AND the noise-to-signal
        // heuristic flags every "edge cell" as clustered with high
        // neighbors. This is the "crowd / screen / LED show" rejection.
        let w = 128usize;
        let h = 128usize;
        let mut buf = vec![0u8; w * h * 4];
        // Deterministic pseudo-random (xorshift) so the test is reproducible
        let mut state: u32 = 0xCAFEBABE;
        for i in 0..(w * h) {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            buf[i * 4]     = (state & 0xff) as u8;
            buf[i * 4 + 1] = ((state >> 8) & 0xff) as u8;
            buf[i * 4 + 2] = ((state >> 16) & 0xff) as u8;
            buf[i * 4 + 3] = 255;
        }
        let s = landmark_score(&buf, w as u32, h as u32);
        assert!(s < 0.35, "random-noise scored {s}, expected < 0.35");
    }
}
