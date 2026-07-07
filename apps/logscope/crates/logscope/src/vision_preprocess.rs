//! vision_preprocess.rs — the pixel resampler `multimodal.rs` names as its IO seam.
//!
//! `multimodal::plan_image` decides the resize target and per-tile crop boxes; this turns raw source pixels
//! into the exact float tensors a CLIP-ViT vision encoder consumes: bilinear-resample each tile out of the
//! source, scale to [0,1], apply CLIP mean/std normalization, and lay it out channel-first (CHW) — one
//! `3·tile·tile` buffer per tile, ready to become patch embeddings. Pure (operates on an RGB byte slice, no
//! image-decoding dependency), so the resampling math is unit-tested on synthetic pixels; decoding a PNG to
//! RGB is a thin caller-side step.

use crate::multimodal::{VisionConfig, ImagePlan, TileBox, plan_image};

/// OpenAI CLIP normalization constants (the LLaVA / most-VLM default).
pub const CLIP_MEAN: [f32; 3] = [0.48145466, 0.4578275, 0.40821073];
pub const CLIP_STD: [f32; 3] = [0.26862954, 0.26130258, 0.27577711];

/// Bilinear-sample an RGB8 image (HWC, row-major, 3 bytes/pixel) at fractional `(fx, fy)`, clamped to the
/// edges. Returns raw [r,g,b] in [0,255] as f32.
pub fn sample(rgb: &[u8], w: u32, h: u32, fx: f32, fy: f32) -> [f32; 3] {
    let (w, h) = (w.max(1), h.max(1));
    let x = fx.clamp(0.0, (w - 1) as f32);
    let y = fy.clamp(0.0, (h - 1) as f32);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let dx = x - x0 as f32;
    let dy = y - y0 as f32;
    let px = |xx: u32, yy: u32, c: usize| rgb[((yy * w + xx) * 3) as usize + c] as f32;
    let mut out = [0.0f32; 3];
    for c in 0..3 {
        let top = px(x0, y0, c) * (1.0 - dx) + px(x1, y0, c) * dx;
        let bot = px(x0, y1, c) * (1.0 - dx) + px(x1, y1, c) * dx;
        out[c] = top * (1.0 - dy) + bot * dy;
    }
    out
}

/// Produce one normalized CHW tile (`3·edge·edge` f32) by resampling the source region that maps to `bx`
/// (a crop box in RESIZE space of `resize_to`). Uses half-pixel-centered mapping so the sampling is
/// centered, matching standard vision-preprocessing.
pub fn tile_chw(rgb: &[u8], w: u32, h: u32, resize_to: (u32, u32), bx: &TileBox, edge: u32) -> Vec<f32> {
    let edge = edge.max(1);
    let (rw, rh) = (resize_to.0.max(1) as f32, resize_to.1.max(1) as f32);
    let sx = w as f32 / rw; // resize-space → source-space scale
    let sy = h as f32 / rh;
    let n = (edge * edge) as usize;
    let mut out = vec![0.0f32; 3 * n];
    for oy in 0..edge {
        for ox in 0..edge {
            // this tile pixel in resize space, then mapped to source (half-pixel centers)
            let rx = bx.x as f32 + ox as f32;
            let ry = bx.y as f32 + oy as f32;
            let src_x = (rx + 0.5) * sx - 0.5;
            let src_y = (ry + 0.5) * sy - 0.5;
            let s = sample(rgb, w, h, src_x, src_y);
            let idx = (oy * edge + ox) as usize;
            for c in 0..3 {
                let v = s[c] / 255.0;
                out[c * n + idx] = (v - CLIP_MEAN[c]) / CLIP_STD[c];
            }
        }
    }
    out
}

/// Turn one CHW tile (`3·edge·edge`) into the patch-embedding INPUT matrix: `(edge/patch)²` rows, each a
/// flattened patch of length `3·patch·patch` in `[c, py, px]` order (the layout a ViT conv-patch-embed
/// expects). Multiplying this `[n_patches × 3·p·p]` matrix by the model's patch-embed weights
/// `[3·p·p × embed_dim]` — i.e. one call to the engine's GEMM — yields the `[n_patches × embed_dim]` patch
/// tokens. So this is the exact hand-off from the resampler to the tuned matmul kernels. Returns
/// (row-major matrix, n_patches).
pub fn patchify(tile: &[f32], edge: u32, patch: u32) -> (Vec<f32>, usize) {
    let edge = edge.max(1);
    let patch = patch.clamp(1, edge);
    let per = (edge / patch).max(1);          // patches per side
    let n = (per * per) as usize;             // total patches
    let plen = (3 * patch * patch) as usize;  // flattened patch length
    let n_pix = (edge * edge) as usize;       // per channel
    let mut out = vec![0.0f32; n * plen];
    for gy in 0..per {
        for gx in 0..per {
            let prow = (gy * per + gx) as usize;
            let dst = prow * plen;
            for c in 0..3usize {
                for py in 0..patch {
                    for px in 0..patch {
                        let sy = gy * patch + py;
                        let sx = gx * patch + px;
                        let src = c * n_pix + (sy * edge + sx) as usize;
                        let d = dst + c * (patch * patch) as usize + (py * patch + px) as usize;
                        out[d] = tile[src];
                    }
                }
            }
        }
    }
    (out, n)
}
/// the plan asks, a whole-image thumbnail tile at the end. Each entry is a CHW `3·edge·edge` f32 buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct VisionInput {
    pub edge: u32,          // tile edge (patches run over this)
    pub tiles: Vec<Vec<f32>>,
    pub has_thumbnail: bool,
}
impl VisionInput {
    /// Total encoder input tiles (hi-res + optional thumbnail).
    pub fn len(&self) -> usize { self.tiles.len() }
    pub fn is_empty(&self) -> bool { self.tiles.is_empty() }
}

/// Full resample: `plan_image` → per-tile CHW tensors → (optional) whole-image thumbnail. `rgb` is the raw
/// source (w×h RGB8). This is the single call the native vision path makes before the ViT forward pass.
pub fn preprocess(rgb: &[u8], w: u32, h: u32, cfg: &VisionConfig) -> VisionInput {
    let plan: ImagePlan = plan_image(w, h, cfg);
    let edge = cfg.tile.max(1);
    let mut tiles: Vec<Vec<f32>> = plan.tiles.iter().map(|bx| tile_chw(rgb, w, h, plan.resize_to, bx, edge)).collect();
    if plan.thumbnail {
        // the thumbnail is the WHOLE source scaled into one tile: a crop box covering all of resize_to.
        let whole = TileBox { x: 0, y: 0, w: plan.resize_to.0, h: plan.resize_to.1 };
        // map the full resize-space box into `edge` samples by treating resize_to as the tile's span
        tiles.push(tile_chw(rgb, w, h, (edge, edge), &TileBox { x: 0, y: 0, w: edge, h: edge }, edge));
        let _ = whole;
    }
    VisionInput { edge, tiles, has_thumbnail: plan.thumbnail }
}

#[cfg(test)]
mod tests {
    use super::*;

    // a solid-color w×h RGB8 image
    fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 3) as usize);
        for _ in 0..(w * h) { v.extend_from_slice(&rgb); }
        v
    }

    #[test]
    fn sample_interpolates_a_gradient() {
        // 2x1 image: left black, right white. Center (x=0.5) should be mid-gray.
        let img = vec![0, 0, 0, 255, 255, 255];
        let s = sample(&img, 2, 1, 0.5, 0.0);
        for c in 0..3 { assert!((s[c] - 127.5).abs() < 0.01, "midpoint interpolates"); }
        // exact endpoints
        assert_eq!(sample(&img, 2, 1, 0.0, 0.0), [0.0, 0.0, 0.0]);
        assert_eq!(sample(&img, 2, 1, 1.0, 0.0), [255.0, 255.0, 255.0]);
    }

    #[test]
    fn tile_of_solid_image_is_uniform_normalized_color() {
        let img = solid(10, 10, [255, 0, 0]); // pure red
        let bx = TileBox { x: 0, y: 0, w: 4, h: 4 };
        let t = tile_chw(&img, 10, 10, (4, 4), &bx, 4);
        assert_eq!(t.len(), 3 * 16);
        // R channel = (1.0 - mean_r)/std_r everywhere; G,B = (0 - mean)/std
        let want = [
            (1.0 - CLIP_MEAN[0]) / CLIP_STD[0],
            (0.0 - CLIP_MEAN[1]) / CLIP_STD[1],
            (0.0 - CLIP_MEAN[2]) / CLIP_STD[2],
        ];
        for c in 0..3 {
            for i in 0..16 {
                assert!((t[c * 16 + i] - want[c]).abs() < 1e-4, "channel {c} normalized uniformly");
            }
        }
    }

    #[test]
    fn preprocess_shapes_match_the_plan() {
        let cfg = VisionConfig { tile: 8, patch: 4, max_tiles: 4, thumbnail: true };
        let img = solid(20, 10, [100, 150, 200]); // wide image → grid favors more cols
        let vi = preprocess(&img, 20, 10, &cfg);
        let plan = crate::multimodal::plan_image(20, 10, &cfg);
        // one CHW tile per plan tile, plus the thumbnail
        assert_eq!(vi.tiles.len(), plan.tiles.len() + 1);
        assert!(vi.has_thumbnail);
        for t in &vi.tiles { assert_eq!(t.len(), 3 * 8 * 8, "each tile is 3·edge·edge"); }
    }

    #[test]
    fn preprocess_without_thumbnail() {
        let cfg = VisionConfig { tile: 8, patch: 4, max_tiles: 1, thumbnail: false };
        let img = solid(8, 8, [10, 20, 30]);
        let vi = preprocess(&img, 8, 8, &cfg);
        assert_eq!(vi.tiles.len(), 1, "1x1 grid, no thumbnail");
        assert!(!vi.has_thumbnail);
    }

    #[test]
    fn patchify_shapes_and_layout() {
        // 4×4 edge, 2×2 patch → 4 patches, each 3·2·2 = 12 long
        let edge = 4u32;
        let mut tile = vec![0.0f32; (3 * edge * edge) as usize];
        // set channel 0 to a ramp = pixel index, so we can verify patch extraction order
        let n_pix = (edge * edge) as usize;
        for i in 0..n_pix { tile[i] = i as f32; } // channel 0
        let (m, n) = patchify(&tile, edge, 2);
        assert_eq!(n, 4);
        assert_eq!(m.len(), 4 * 12);
        // patch (0,0) channel-0 = pixels (0,0),(0,1),(1,0),(1,1) = indices 0,1,4,5
        let p0 = &m[0..12];
        assert_eq!(&p0[0..4], &[0.0, 1.0, 4.0, 5.0], "top-left patch, channel 0, [py,px] order");
        // patch (0,1) [second column] channel-0 = indices 2,3,6,7
        let p1 = &m[12..24];
        assert_eq!(&p1[0..4], &[2.0, 3.0, 6.0, 7.0]);
    }

    #[test]
    fn patchify_full_tile_when_patch_equals_edge() {
        let tile = vec![1.0f32; 3 * 4 * 4];
        let (m, n) = patchify(&tile, 4, 4);
        assert_eq!(n, 1, "one patch covering the whole tile");
        assert_eq!(m.len(), 3 * 16);
    }
}
