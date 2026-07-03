// FAO v2 — Graph-guided encoder
//
// The scene graph discovers WHAT exists before touching pixels.
// Encoding is guided by object identity and function, not pixel scanning.

use crate::*;

// ══════════════════════════════════════════════════════════
// Scene Graph — nodes represent objects, regions, and functions
// ══════════════════════════════════════════════════════════

/// Unique identifier for a graph node
pub type NodeId = u32;

/// What kind of thing this node represents
#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    Region {
        dir_mode: DirMode,      // dominant directional mode
        dc: [u8; 3],            // average YCbCr
    },
    Object {
        signature: u64,         // texture hash for recognition
        trajectory: Vec<(f32, f32)>,  // motion history (mvx, mvy)
    },
    Effect {
        birth_frame: usize,
        expected_duration: usize,
    },
    UI {
        static_since: usize,    // frame when this became static
    },
}

/// A node in the scene graph
#[derive(Clone)]
pub struct SceneNode {
    pub id: NodeId,
    pub kind: NodeKind,
    pub blocks: Vec<u16>,       // which 8×8 block indices this node covers
    pub confidence: f32,        // 0.0 = unknown, 1.0 = certain
    pub last_seen: usize,       // frame index
    pub times_predicted: u32,   // how many frames this node successfully predicted
    pub total_residual: f32,    // accumulated prediction error
}

/// The scene graph — discovered understanding of the video content
pub struct SceneGraph {
    pub nodes: Vec<SceneNode>,
    next_id: NodeId,
    pub w: usize,
    pub h: usize,
    bw: usize,
    bh: usize,
    /// Which node owns each block (None = unclaimed)
    pub block_owner: Vec<Option<NodeId>>,
    /// Per-block: frames since last change
    block_stable: Vec<u16>,
    /// Per-block: accumulated DC for change detection
    block_dc: Vec<f32>,
    frame_count: usize,
}

impl SceneGraph {
    pub fn new(w: usize, h: usize) -> Self {
        let bw = w / 8;
        let bh = h / 8;
        let nb = bw * bh;
        Self {
            nodes: Vec::new(),
            next_id: 0,
            w, h, bw, bh,
            block_owner: vec![None; nb],
            block_stable: vec![0; nb],
            block_dc: vec![0.0; nb],
            frame_count: 0,
        }
    }

    /// Analyze a frame and update the graph
    pub fn analyze_frame(&mut self, y: &[f32], cb: &[f32], cr: &[f32]) {
        let w = self.w;
        
        // Phase 1: Compute per-block DC and detect changes
        for by in 0..self.bh {
            for bx in 0..self.bw {
                let bi = by * self.bw + bx;
                let ox = bx * 8;
                let oy = by * 8;

                // Block DC (average luma)
                let mut dc = 0.0f32;
                for dy in 0..8 { for dx in 0..8 {
                    dc += y[(oy + dy) * w + ox + dx];
                }}
                dc /= 64.0;

                // Change detection
                let prev_dc = self.block_dc[bi];
                let changed = (dc - prev_dc).abs() > 5.0;
                self.block_dc[bi] = dc;

                if changed {
                    self.block_stable[bi] = 0;
                    // If this block had an owner, reduce its confidence
                    if let Some(owner_id) = self.block_owner[bi] {
                        if let Some(node) = self.nodes.iter_mut().find(|n| n.id == owner_id) {
                            node.confidence *= 0.95;
                        }
                    }
                } else {
                    self.block_stable[bi] = self.block_stable[bi].saturating_add(1);
                }
            }
        }

        // Phase 2: Discover new regions from stable unclaimed blocks
        if self.frame_count % 10 == 0 {
            self.discover_regions(y, cb, cr);
        }

        // Phase 3: Classify stable blocks as UI
        if self.frame_count > 30 {
            self.detect_ui();
        }

        // Phase 4: Evict dead nodes
        self.evict_dead_nodes();

        self.frame_count += 1;
    }

    /// Find contiguous regions of similar unclaimed blocks
    fn discover_regions(&mut self, y: &[f32], _cb: &[f32], _cr: &[f32]) {
        let w = self.w;
        let mut visited = vec![false; self.bw * self.bh];

        for by in 0..self.bh {
            for bx in 0..self.bw {
                let bi = by * self.bw + bx;
                if visited[bi] || self.block_owner[bi].is_some() { continue; }
                if self.block_stable[bi] < 5 { continue; } // not stable enough

                // Flood fill from this block to find similar neighbors
                let seed_dc = self.block_dc[bi];
                let mut region_blocks = Vec::new();
                let mut stack = vec![(bx, by)];

                while let Some((cx, cy)) = stack.pop() {
                    let ci = cy * self.bw + cx;
                    if visited[ci] || self.block_owner[ci].is_some() { continue; }
                    if (self.block_dc[ci] - seed_dc).abs() > 10.0 { continue; }

                    visited[ci] = true;
                    region_blocks.push(ci as u16);

                    // 4-connected neighbors
                    if cx > 0 { stack.push((cx - 1, cy)); }
                    if cx + 1 < self.bw { stack.push((cx + 1, cy)); }
                    if cy > 0 { stack.push((cx, cy - 1)); }
                    if cy + 1 < self.bh { stack.push((cx, cy + 1)); }
                }

                // Only create a node if the region is large enough (4+ blocks)
                if region_blocks.len() >= 4 {
                    let id = self.next_id;
                    self.next_id += 1;

                    // Determine dominant directional mode from a sample block
                    let sample_bi = region_blocks[0] as usize;
                    let sample_bx = sample_bi % self.bw;
                    let sample_by = sample_bi / self.bw;
                    let ox = sample_bx * 8;
                    let oy = sample_by * 8;

                    // Simple mode detection from gradient
                    let mut gx = 0.0f32;
                    let mut gy = 0.0f32;
                    for dy in 0..7 { for dx in 0..7 {
                        gx += (y[(oy+dy)*w+ox+dx+1] - y[(oy+dy)*w+ox+dx]).abs();
                        gy += (y[(oy+dy+1)*w+ox+dx] - y[(oy+dy)*w+ox+dx]).abs();
                    }}

                    let dir = if gx < 5.0 && gy < 5.0 { DirMode::DC }
                        else if gx > gy * 2.0 { DirMode::V }
                        else if gy > gx * 2.0 { DirMode::H }
                        else { DirMode::Gradient };

                    let dc_y = seed_dc.clamp(0.0, 255.0) as u8;

                    for &bi in &region_blocks {
                        self.block_owner[bi as usize] = Some(id);
                    }

                    self.nodes.push(SceneNode {
                        id,
                        kind: NodeKind::Region {
                            dir_mode: dir,
                            dc: [dc_y, 128, 128],
                        },
                        blocks: region_blocks,
                        confidence: 0.5,
                        last_seen: self.frame_count,
                        times_predicted: 0,
                        total_residual: 0.0,
                    });
                }
            }
        }
    }

    /// Detect UI elements: blocks that have been static for 30+ frames
    fn detect_ui(&mut self) {
        for bi in 0..self.block_stable.len() {
            if self.block_stable[bi] >= 30 && self.block_owner[bi].is_none() {
                // Check if nearby blocks are also static (UI tends to be rectangular)
                let bx = bi % self.bw;
                let by = bi / self.bw;
                let mut neighbors_static = 0;
                for (dx, dy) in &[(-1i32,0),(1,0),(0,-1),(0,1)] {
                    let nx = bx as i32 + dx;
                    let ny = by as i32 + dy;
                    if nx >= 0 && nx < self.bw as i32 && ny >= 0 && ny < self.bh as i32 {
                        let ni = ny as usize * self.bw + nx as usize;
                        if self.block_stable[ni] >= 30 { neighbors_static += 1; }
                    }
                }

                if neighbors_static >= 2 {
                    let id = self.next_id;
                    self.next_id += 1;
                    self.block_owner[bi] = Some(id);
                    self.nodes.push(SceneNode {
                        id,
                        kind: NodeKind::UI { static_since: self.frame_count.saturating_sub(30) },
                        blocks: vec![bi as u16],
                        confidence: 0.9,
                        last_seen: self.frame_count,
                        times_predicted: 0,
                        total_residual: 0.0,
                    });
                }
            }
        }
    }

    /// Remove nodes that haven't been seen recently or have low confidence
    fn evict_dead_nodes(&mut self) {
        let fc = self.frame_count;
        self.nodes.retain(|n| {
            let alive = n.confidence > 0.1 && fc - n.last_seen < 120;
            if !alive {
                // Release blocks
                // (can't modify block_owner here — handled separately)
            }
            alive
        });

        // Release blocks from evicted nodes
        let active_ids: std::collections::HashSet<NodeId> = self.nodes.iter().map(|n| n.id).collect();
        for owner in &mut self.block_owner {
            if let Some(id) = *owner {
                if !active_ids.contains(&id) { *owner = None; }
            }
        }
    }

    /// Get encoding guidance for a block: what should the encoder do?
    pub fn guide_block(&self, bi: usize) -> BlockGuidance {
        if let Some(owner_id) = self.block_owner[bi] {
            if let Some(node) = self.nodes.iter().find(|n| n.id == owner_id) {
                return match &node.kind {
                    NodeKind::Region { dir_mode, dc } => BlockGuidance::PredictDir {
                        mode: *dir_mode,
                        dc: dc[0],
                        confidence: node.confidence,
                    },
                    NodeKind::UI { .. } => BlockGuidance::Skip {
                        confidence: node.confidence,
                    },
                    NodeKind::Object { .. } => BlockGuidance::MotionPredict {
                        confidence: node.confidence,
                    },
                    NodeKind::Effect { .. } => BlockGuidance::FullEncode,
                };
            }
        }
        BlockGuidance::FullEncode
    }

    /// Stats: how much of the frame is graph-guided
    pub fn coverage_stats(&self) -> (usize, usize, usize, usize) {
        let total = self.block_owner.len();
        let claimed = self.block_owner.iter().filter(|o| o.is_some()).count();
        let ui = self.nodes.iter()
            .filter(|n| matches!(n.kind, NodeKind::UI { .. }))
            .map(|n| n.blocks.len()).sum::<usize>();
        let regions = self.nodes.iter()
            .filter(|n| matches!(n.kind, NodeKind::Region { .. }))
            .map(|n| n.blocks.len()).sum::<usize>();
        (total, claimed, ui, regions)
    }
}

/// What the graph tells the encoder to do with a block
#[derive(Debug)]
pub enum BlockGuidance {
    /// Use directional prediction with this mode and DC
    PredictDir { mode: DirMode, dc: u8, confidence: f32 },
    /// Skip this block (UI element, static)
    Skip { confidence: f32 },
    /// Use motion prediction (object with trajectory)
    MotionPredict { confidence: f32 },
    /// No graph guidance — full encode needed
    FullEncode,
}

// ══════════════════════════════════════════════════════════
// Extended directional modes (12 modes)
// ══════════════════════════════════════════════════════════

#[derive(Clone, Copy, PartialEq, Debug)]
#[repr(u8)]
pub enum DirMode {
    DC = 0, H = 1, V = 2,
    D45 = 3, D135 = 4, D27 = 5, D63 = 6, D117 = 7, D153 = 8,
    Smooth = 9, Paeth = 10, Gradient = 11,
}

const ALL_DIR_MODES: [DirMode; 12] = [
    DirMode::DC, DirMode::H, DirMode::V,
    DirMode::D45, DirMode::D135, DirMode::D27, DirMode::D63,
    DirMode::D117, DirMode::D153,
    DirMode::Smooth, DirMode::Paeth, DirMode::Gradient,
];

pub fn dir_predict(recon: &[f32], w: usize, h: usize, ox: usize, oy: usize, mode: DirMode) -> [f32; 64] {
    let mut pred = [128.0f32; 64];
    let has_top = oy >= 1;
    let has_left = ox >= 1;
    let mut top = [128.0f32; 8];
    let mut left = [128.0f32; 8];
    let tl = if has_top && has_left { recon[(oy-1)*w+ox-1] } else { 128.0 };
    if has_top { for x in 0..8 { top[x] = recon[(oy-1)*w+ox+x]; } }
    if has_left { for y in 0..8 { left[y] = recon[(oy+y)*w+ox-1]; } }

    match mode {
        DirMode::DC => {
            let mut s = 0.0f32; let mut n = 0;
            if has_top { for x in 0..8 { s += top[x]; n += 1; } }
            if has_left { for y in 0..8 { s += left[y]; n += 1; } }
            pred.fill(if n > 0 { s / n as f32 } else { 128.0 });
        }
        DirMode::H => { for y in 0..8 { for x in 0..8 { pred[y*8+x] = left[y]; } } }
        DirMode::V => { for y in 0..8 { for x in 0..8 { pred[y*8+x] = top[x]; } } }
        DirMode::D45 => { for y in 0..8 { for x in 0..8 {
            let i = x+y; pred[y*8+x] = if i < 8 { top[i.min(7)] } else { left[(i-7).min(7)] };
        }}}
        DirMode::D135 => { for y in 0..8 { for x in 0..8 {
            let d = x as i32 - y as i32;
            pred[y*8+x] = if d > 0 { top[(d as usize).min(7)] }
                else if d < 0 { left[((-d) as usize).min(7)] } else { tl };
        }}}
        DirMode::D27 => { for y in 0..8 { for x in 0..8 {
            let sx = x as f32 + y as f32 * 0.5;
            let i = sx as usize;
            let f = sx - i as f32;
            pred[y*8+x] = if i < 7 { top[i]*(1.0-f) + top[i+1]*f } else { top[7] };
        }}}
        DirMode::D63 => { for y in 0..8 { for x in 0..8 {
            let sy = y as f32 + x as f32 * 0.5;
            let i = sy as usize;
            let f = sy - i as f32;
            pred[y*8+x] = if i < 7 { left[i]*(1.0-f) + left[i+1]*f } else { left[7] };
        }}}
        DirMode::D117 => { for y in 0..8 { for x in 0..8 {
            let sy = y as f32 - x as f32 * 0.5;
            if sy >= 0.0 { let i = sy as usize; let f = sy - i as f32;
                pred[y*8+x] = if i < 7 { left[i]*(1.0-f)+left[i+1]*f } else { left[7] };
            } else { pred[y*8+x] = top[((x as f32-y as f32*2.0).max(0.0) as usize).min(7)]; }
        }}}
        DirMode::D153 => { for y in 0..8 { for x in 0..8 {
            let sx = x as f32 - y as f32 * 0.5;
            if sx >= 0.0 { let i = sx as usize; let f = sx - i as f32;
                pred[y*8+x] = if i < 7 { top[i]*(1.0-f)+top[i+1]*f } else { top[7] };
            } else { pred[y*8+x] = left[((y as f32-x as f32*2.0).max(0.0) as usize).min(7)]; }
        }}}
        DirMode::Smooth => { for y in 0..8 { for x in 0..8 {
            let wt = (7-y) as f32 / 7.0;
            let wl = (7-x) as f32 / 7.0;
            let wtl = (1.0-wt)*(1.0-wl);
            pred[y*8+x] = (top[x]*wt + left[y]*wl + tl*wtl) / (wt+wl+wtl).max(0.01);
        }}}
        DirMode::Paeth => { for y in 0..8 { for x in 0..8 {
            let (t, l) = (top[x], left[y]);
            let p = t + l - tl;
            let (dt, dl, dtl) = ((p-t).abs(), (p-l).abs(), (p-tl).abs());
            pred[y*8+x] = if dt<=dl && dt<=dtl { t } else if dl<=dtl { l } else { tl };
        }}}
        DirMode::Gradient => { for y in 0..8 { for x in 0..8 {
            pred[y*8+x] = (top[x] + left[y] - tl).clamp(0.0, 255.0);
        }}}
    }
    pred
}

pub fn best_dir_mode(recon: &[f32], actual: &[f32], w: usize, h: usize, ox: usize, oy: usize) -> (DirMode, [f32; 64], f32) {
    let mut best = (DirMode::DC, [128.0f32; 64], f32::MAX);
    let mut block = [0.0f32; 64];
    for y in 0..8 { for x in 0..8 { block[y*8+x] = actual[(oy+y)*w+ox+x]; } }
    for &mode in &ALL_DIR_MODES {
        let pred = dir_predict(recon, w, h, ox, oy, mode);
        let sad: f32 = (0..64).map(|i| (block[i]-pred[i]).abs()).sum();
        if sad < best.2 { best = (mode, pred, sad); }
    }
    best
}

// ══════════════════════════════════════════════════════════
// v2 VideoEncoder — graph-guided encoding
// ══════════════════════════════════════════════════════════

pub struct V2Encoder {
    pub w: usize,
    pub h: usize,
    pub quality: f32,
    pub graph: SceneGraph,
    v1_fallback: crate::VideoEncoder,
    frame_count: usize,
}

impl V2Encoder {
    pub fn new(w: usize, h: usize, quality: f32) -> Self {
        Self {
            w, h, quality,
            graph: SceneGraph::new(w, h),
            v1_fallback: crate::VideoEncoder::new(w, h, quality, 1.0, 5),
            frame_count: 0,
        }
    }

    pub fn encode_frame(&mut self, rgb_in: &[u8], rgb_out: &mut [u8]) -> FrameResult {
        let n = self.w * self.h;
        let mut cur_y = vec![0.0f32; n];
        let mut cur_cb = vec![0.0f32; n];
        let mut cur_cr = vec![0.0f32; n];
        rgb_to_planes(rgb_in, self.w, self.h, &mut cur_y, &mut cur_cb, &mut cur_cr);

        // Update the scene graph with this frame
        self.graph.analyze_frame(&cur_y, &cur_cb, &cur_cr);

        // Encode using v1 (for now — graph guidance will replace this)
        let mut result = self.v1_fallback.encode_frame(rgb_in, rgb_out);

        // Report graph stats
        let (total, claimed, ui, regions) = self.graph.coverage_stats();
        let coverage = if total > 0 { claimed as f64 / total as f64 * 100.0 } else { 0.0 };
        result.route_stats = format!(
            "v2: {:.0}% mapped ({} regions, {} UI, {} unclaimed)",
            coverage, regions, ui, total - claimed
        );

        self.frame_count += 1;
        result
    }
}


// ══════════════════════════════════════════════════════════
// Regional prediction — 16×16 base + 8×8 refinement
// ══════════════════════════════════════════════════════════

/// Compute a 16×16 regional prediction using directional modes.
/// Returns the predicted 16×16 block and the best mode.
pub fn regional_predict(recon: &[f32], w: usize, h: usize, ox: usize, oy: usize) -> (DirMode, Vec<f32>) {
    let mut best_mode = DirMode::DC;
    let mut best_pred = vec![128.0f32; 256];
    let mut best_sad = f32::MAX;

    // Gather 16-wide neighbor pixels
    let has_top = oy >= 1;
    let has_left = ox >= 1;
    let has_tl = has_top && has_left;

    let mut top = [128.0f32; 16];
    let mut left = [128.0f32; 16];
    let tl = if has_tl { recon[(oy - 1) * w + ox - 1] } else { 128.0 };

    if has_top { for x in 0..16.min(w - ox) { top[x] = recon[(oy - 1) * w + ox + x]; } }
    if has_left { for y in 0..16.min(h - oy) { left[y] = recon[(oy + y) * w + ox - 1]; } }

    // Test key modes at 16×16 scale
    for &mode in &[DirMode::DC, DirMode::H, DirMode::V, DirMode::Gradient,
                   DirMode::Smooth, DirMode::D45, DirMode::D135] {
        let mut pred = vec![128.0f32; 256];

        match mode {
            DirMode::DC => {
                let mut sum = 0.0f32; let mut n = 0;
                if has_top { for x in 0..16 { sum += top[x]; n += 1; } }
                if has_left { for y in 0..16 { sum += left[y]; n += 1; } }
                let dc = if n > 0 { sum / n as f32 } else { 128.0 };
                pred.fill(dc);
            }
            DirMode::H => {
                for y in 0..16 { for x in 0..16 { pred[y*16+x] = left[y]; } }
            }
            DirMode::V => {
                for y in 0..16 { for x in 0..16 { pred[y*16+x] = top[x]; } }
            }
            DirMode::Gradient => {
                for y in 0..16 { for x in 0..16 {
                    pred[y*16+x] = (top[x] + left[y] - tl).clamp(0.0, 255.0);
                }}
            }
            DirMode::Smooth => {
                for y in 0..16 { for x in 0..16 {
                    let wt = (15 - y) as f32 / 15.0;
                    let wl = (15 - x) as f32 / 15.0;
                    let denom = wt + wl + (1.0 - wt) * (1.0 - wl);
                    pred[y*16+x] = if denom > 0.0 {
                        (top[x] * wt + left[y] * wl + tl * (1.0 - wt) * (1.0 - wl)) / denom
                    } else { 128.0 };
                }}
            }
            DirMode::D45 => {
                for y in 0..16 { for x in 0..16 {
                    let idx = x + y;
                    pred[y*16+x] = if idx < 16 { top[idx.min(15)] }
                        else { left[(idx - 15).min(15)] };
                }}
            }
            DirMode::D135 => {
                for y in 0..16 { for x in 0..16 {
                    let d = x as i32 - y as i32;
                    pred[y*16+x] = if d > 0 { top[(d as usize).min(15)] }
                        else if d < 0 { left[((-d) as usize).min(15)] }
                        else { tl };
                }}
            }
            _ => {}
        }

        // SAD against actual content
        let mut sad = 0.0f32;
        for y in 0..16.min(h - oy) { for x in 0..16.min(w - ox) {
            sad += (recon[(oy+y)*w+ox+x] - pred[y*16+x]).abs();
        }}

        if sad < best_sad {
            best_sad = sad;
            best_mode = mode;
            best_pred = pred;
        }
    }

    (best_mode, best_pred)
}

/// Extract the 8×8 sub-block from a 16×16 regional prediction
pub fn extract_subblock(regional: &[f32], quadrant: usize) -> [f32; 64] {
    let mut sub = [0.0f32; 64];
    let qx = (quadrant % 2) * 8;
    let qy = (quadrant / 2) * 8;
    for y in 0..8 { for x in 0..8 {
        sub[y*8+x] = regional[(qy+y)*16 + qx+x];
    }}
    sub
}

// ══════════════════════════════════════════════════════════
// Quadratic gradient prediction for curved gradients
// ══════════════════════════════════════════════════════════

/// Fit a quadratic surface to the neighbors and extrapolate.
/// Handles curved gradients (sunsets, vignettes, radial lighting).
pub fn quadratic_predict(recon: &[f32], w: usize, h: usize, ox: usize, oy: usize) -> [f32; 64] {
    let mut pred = [128.0f32; 64];

    let has_top = oy >= 1;
    let has_left = ox >= 1;
    let has_top2 = oy >= 2;
    let has_left2 = ox >= 2;

    let mut top = [128.0f32; 8];
    let mut left = [128.0f32; 8];
    let mut top2 = [128.0f32; 8];  // row 2 above
    let mut left2 = [128.0f32; 8]; // column 2 left

    if has_top { for x in 0..8 { top[x] = recon[(oy-1)*w+ox+x]; } }
    if has_left { for y in 0..8 { left[y] = recon[(oy+y)*w+ox-1]; } }
    if has_top2 { for x in 0..8 { top2[x] = recon[(oy-2)*w+ox+x]; } }
    if has_left2 { for y in 0..8 { left2[y] = recon[(oy+y)*w+ox-2]; } }

    if has_top2 && has_left2 {
        // Quadratic: extrapolate from two rows/columns of neighbors
        // Curvature = second derivative: top - 2*top2_extrapolated + top_extrapolated
        for y in 0..8 { for x in 0..8 {
            // Horizontal curvature from left neighbors
            let h_accel = if has_left2 { left[y] - left2[y] } else { 0.0 };
            let h_pred = left[y] + h_accel * (x as f32 + 1.0) / 8.0;

            // Vertical curvature from top neighbors
            let v_accel = if has_top2 { top[x] - top2[x] } else { 0.0 };
            let v_pred = top[x] + v_accel * (y as f32 + 1.0) / 8.0;

            // Blend horizontal and vertical quadratic predictions
            pred[y*8+x] = ((h_pred + v_pred) / 2.0).clamp(0.0, 255.0);
        }}
    } else {
        // Fallback to linear gradient
        let tl = if has_top && has_left { recon[(oy-1)*w+ox-1] } else { 128.0 };
        if has_top { for x in 0..8 { top[x] = recon[(oy-1)*w+ox+x]; } }
        if has_left { for y in 0..8 { left[y] = recon[(oy+y)*w+ox-1]; } }
        for y in 0..8 { for x in 0..8 {
            pred[y*8+x] = (top[x] + left[y] - tl).clamp(0.0, 255.0);
        }}
    }
    pred
}

// ══════════════════════════════════════════════════════════
// Temporal flicker reduction
// ══════════════════════════════════════════════════════════

/// Track per-block encode/skip alternation for flicker detection
pub struct FlickerTracker {
    /// Bit pattern of recent skip/encode decisions per block
    /// 1 = skip, 0 = encode. Low bit = most recent frame.
    pub history: Vec<u8>,
    pub w_blocks: usize,
}

impl FlickerTracker {
    pub fn new(w: usize, h: usize) -> Self {
        let wb = w / 8; let hb = h / 8;
        Self { history: vec![0u8; wb * hb], w_blocks: wb }
    }

    /// Record whether a block was skipped or encoded
    pub fn record(&mut self, bi: usize, skipped: bool) {
        if bi >= self.history.len() { return; }
        self.history[bi] = (self.history[bi] << 1) | if skipped { 1 } else { 0 };
    }

    /// Check if a block is flickering (alternating skip/encode pattern)
    pub fn is_flickering(&self, bi: usize) -> bool {
        if bi >= self.history.len() { return false; }
        let h = self.history[bi];
        // Pattern 01010101 or 10101010 = alternating every frame
        h == 0b01010101 || h == 0b10101010
        // Also catch every-2-frame alternation
        || h == 0b00110011 || h == 0b11001100
    }

    /// For flickering blocks, suggest blending factor (0.0 = use encode, 1.0 = use skip)
    pub fn blend_factor(&self, bi: usize) -> f32 {
        if !self.is_flickering(bi) { return 0.0; }
        // Count recent skips vs encodes
        let h = self.history[bi];
        let skips = h.count_ones() as f32;
        skips / 8.0  // ratio of skips in last 8 frames
    }
}

// ══════════════════════════════════════════════════════════
// Fourier Field Prediction — continuous spatial functions
// ══════════════════════════════════════════════════════════

use std::f32::consts::PI;

/// A single Fourier basis function: amplitude * cos(freq_x * x + freq_y * y + phase)
#[derive(Clone, Copy, Debug)]
pub struct FourierCoeff {
    pub freq_x: f32,   // spatial frequency in x (cycles per frame width)
    pub freq_y: f32,   // spatial frequency in y
    pub amplitude: f32, // strength
    pub phase: f32,     // offset
    pub channel: u8,    // 0=Y, 1=Cb, 2=Cr
    pub age: u16,       // frames since last update
    pub rmse_reduction: f32, // how much this coefficient reduces error
}

impl FourierCoeff {
    /// Evaluate this basis function at pixel (x, y) given frame dimensions
    pub fn eval(&self, x: usize, y: usize, w: usize, h: usize) -> f32 {
        let nx = x as f32 / w as f32;  // normalized [0, 1]
        let ny = y as f32 / h as f32;
        self.amplitude * (2.0 * PI * (self.freq_x * nx + self.freq_y * ny) + self.phase).cos()
    }

    /// Packed representation (12 bytes)
    pub fn to_bytes(&self) -> [u8; 12] {
        let mut b = [0u8; 12];
        b[0..2].copy_from_slice(&(self.freq_x as i16).to_le_bytes());
        b[2..4].copy_from_slice(&(self.freq_y as i16).to_le_bytes());
        b[4..6].copy_from_slice(&((self.amplitude * 100.0) as i16).to_le_bytes());
        b[6..8].copy_from_slice(&((self.phase * 1000.0) as i16).to_le_bytes());
        b[8] = self.channel;
        b[9..11].copy_from_slice(&self.age.to_le_bytes());
        b[11] = (self.rmse_reduction.min(25.5) * 10.0) as u8;
        b
    }
}

/// Fourier field: a set of basis functions that describe the spatial background
pub struct FourierField {
    pub coeffs: Vec<FourierCoeff>,
    pub dc: [f32; 3],  // DC offset per channel (Y, Cb, Cr)
    pub max_coeffs: usize,
}

impl FourierField {
    pub fn new(max_coeffs: usize) -> Self {
        Self {
            coeffs: Vec::new(),
            dc: [128.0, 128.0, 128.0],
            max_coeffs,
        }
    }

    /// Evaluate the field at a pixel position
    pub fn eval(&self, x: usize, y: usize, w: usize, h: usize, channel: u8) -> f32 {
        let mut val = self.dc[channel as usize];
        for c in &self.coeffs {
            if c.channel == channel {
                val += c.eval(x, y, w, h);
            }
        }
        val.clamp(0.0, 255.0)
    }

    /// Render the entire field to a plane
    pub fn render_plane(&self, plane: &mut [f32], w: usize, h: usize, channel: u8) {
        let dc = self.dc[channel as usize];
        for y in 0..h { for x in 0..w {
            plane[y * w + x] = dc;
        }}
        // Add each coefficient's contribution
        for c in &self.coeffs {
            if c.channel != channel { continue; }
            for y in 0..h { for x in 0..w {
                plane[y * w + x] += c.eval(x, y, w, h);
            }}
        }
        // Clamp
        for v in plane.iter_mut() { *v = v.clamp(0.0, 255.0); }
    }

    /// Greedy discovery: generate candidate coefficients and keep those
    /// that reduce RMSE enough to justify their bit cost.
    /// This is the "slightly greedy" approach — over-generate, then cull.
    pub fn discover_greedy(
        &mut self, 
        actual_y: &[f32], actual_cb: &[f32], actual_cr: &[f32],
        w: usize, h: usize,
        min_rmse_gain: f32,  // minimum RMSE reduction to keep a coefficient
    ) {
        // Compute DC for each channel
        let n = w * h;
        self.dc[0] = actual_y.iter().sum::<f32>() / n as f32;
        self.dc[1] = actual_cb.iter().sum::<f32>() / n as f32;
        self.dc[2] = actual_cr.iter().sum::<f32>() / n as f32;

        // Generate candidate frequencies: low to high
        // Greedy: try many, keep what works
        let max_freq = 8.0;  // up to 8 cycles across the frame
        let freq_steps = 16;  // 16 frequency candidates per axis

        // Process per channel with independent residuals
        for channel in 0..3u8 {
            let actual: &[f32] = match channel { 0 => actual_y, 1 => actual_cb, _ => actual_cr };
            let dc = self.dc[channel as usize];
            let mut running_res: Vec<f32> = actual.iter().map(|&v| v - dc).collect();

            for fi in 1..=freq_steps {
                let freq = fi as f32 * max_freq / freq_steps as f32;

                // Try horizontal, vertical, and diagonal orientations
                for &(fx, fy) in &[(freq, 0.0f32), (0.0, freq), (freq, freq), (freq, -freq)] {
                    // Fit amplitude and phase to the residual
                    // Simple: amplitude = correlation with basis, phase = 0 or π
                    let mut cos_sum = 0.0f32;
                    let mut sin_sum = 0.0f32;
                    let mut basis_energy = 0.0f32;

                    // Sample (don't scan every pixel for speed)
                    let step = (n / 1024).max(1);
                    let mut samples = 0;
                    for i in (0..n).step_by(step) {
                        let x = i % w;
                        let y = i / w;
                        let nx = x as f32 / w as f32;
                        let ny = y as f32 / h as f32;
                        let angle = 2.0 * PI * (fx * nx + fy * ny);
                        cos_sum += running_res[i] * angle.cos();
                        sin_sum += running_res[i] * angle.sin();
                        basis_energy += 1.0;
                        samples += 1;
                    }

                    if samples == 0 { continue; }
                    let amplitude = (cos_sum * cos_sum + sin_sum * sin_sum).sqrt() / basis_energy;
                    let phase = sin_sum.atan2(cos_sum);

                    if amplitude < 0.5 { continue; }  // too weak

                    // Compute RMSE reduction from adding this coefficient
                    let mut old_mse = 0.0f32;
                    let mut new_mse = 0.0f32;
                    for i in (0..n).step_by(step) {
                        let x = i % w;
                        let y = i / w;
                        let nx = x as f32 / w as f32;
                        let ny = y as f32 / h as f32;
                        let pred = amplitude * (2.0 * PI * (fx * nx + fy * ny) + phase).cos();
                        old_mse += running_res[i] * running_res[i];
                        new_mse += (running_res[i] - pred) * (running_res[i] - pred);
                    }
                    old_mse /= samples as f32;
                    new_mse /= samples as f32;
                    let rmse_gain = old_mse.sqrt() - new_mse.sqrt();

                    if rmse_gain >= min_rmse_gain && self.coeffs.len() < self.max_coeffs {
                        let coeff = FourierCoeff {
                            freq_x: fx, freq_y: fy,
                            amplitude, phase,
                            channel, age: 0,
                            rmse_reduction: rmse_gain,
                        };
                        self.coeffs.push(coeff);

                        // Subtract this coefficient from running residual
                        for i in 0..n {
                            let x = i % w;
                            let y = i / w;
                            running_res[i] -= coeff.eval(x, y, w, h);
                        }
                    }
                }
            }
        }
    }

    /// Cull coefficients that no longer reduce error enough
    /// Called periodically with the current frame to verify coefficients
    pub fn cull(&mut self, actual_y: &[f32], actual_cb: &[f32], actual_cr: &[f32],
                w: usize, h: usize, min_rmse_gain: f32) {
        let n = w * h;
        let step = (n / 512).max(1);
        let actuals: [&[f32]; 3] = [actual_y, actual_cb, actual_cr];

        self.coeffs.retain(|c| {
            let actual = actuals[c.channel as usize];
            let dc = self.dc[c.channel as usize];

            // Compute error with and without this coefficient
            let mut mse_with = 0.0f32;
            let mut mse_without = 0.0f32;
            let mut samples = 0;
            for i in (0..n).step_by(step) {
                let x = i % w;
                let y = i / w;
                let pred_with = dc + c.eval(x, y, w, h);
                let pred_without = dc;
                mse_with += (actual[i] - pred_with) * (actual[i] - pred_with);
                mse_without += (actual[i] - pred_without) * (actual[i] - pred_without);
                samples += 1;
            }
            if samples == 0 { return false; }
            let rmse_gain = (mse_without / samples as f32).sqrt()
                - (mse_with / samples as f32).sqrt();
            rmse_gain >= min_rmse_gain * 0.5  // keep if still provides half the original gain
        });

        // Age all coefficients
        for c in &mut self.coeffs { c.age = c.age.saturating_add(1); }
    }

    /// Total bytes to represent this field
    pub fn byte_cost(&self) -> usize {
        12 + self.coeffs.len() * 12  // DC (3×f32) + coefficients (12 bytes each)
    }
}
