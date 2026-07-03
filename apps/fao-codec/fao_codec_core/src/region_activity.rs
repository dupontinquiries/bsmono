// Region Activity Tracker — layer-aware per-block stability analysis
//
// CORE INSIGHT: frames are rarely globally "static" or "moving".
// They're MIXED — some regions stable (UI, background), others moving.
// 
// Even MORE NUANCED: UI elements are often RENDERED WITH ALPHA over moving content.
// A HUD health bar is "static" (same shape, same position) but its rendered
// pixels VARY due to alpha-blending with whatever's behind it.
//
// LAYER CLASSIFICATION:
//   PURE_STATIC:    pixels never change (rare — only solid-color UI corners)
//   ALPHA_OVERLAY:  pixels vary in tight range (UI element, BG bleeds through)
//   PARALLAX_BG:    pixels move smoothly (camera pan / scrolling background)
//   ACTIVE:         pixels change frequently (gameplay action)
//   NOISE:          high-variance unstructured (particle effects, fire)
//
// Each block gets a layer assignment. Layer-aware encoding then:
//   - PURE_STATIC: zero residual, dictionary ID
//   - ALPHA_OVERLAY: store base pattern + small variance bits
//   - PARALLAX_BG: motion vector + small residual
//   - ACTIVE: full T-frame residual
//   - NOISE: skip (eye doesn't track noise anyway)

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlockLayer {
    PureStatic,
    AlphaOverlay,    // Static UI rendered with transparency over varying BG
    ParallaxBg,
    Active,
    Noise,
    Unknown,
}

/// Per-block activity stats accumulated across frames
#[derive(Clone)]
pub struct BlockActivity {
    /// Number of consecutive frames this block has been "stable" (hash unchanged)
    pub stable_streak: u32,
    /// Last seen pixel data hash (for change detection)
    pub last_hash: u64,
    /// Min observed luma per pixel (sparse 4x4 grid = 16 samples)
    pub min_samples: [u8; 16],
    /// Max observed luma per pixel
    pub max_samples: [u8; 16],
    /// Running average per sample (smoothed over time)
    pub avg_samples: [f32; 16],
    /// Number of frames this block has been observed
    pub frames_seen: u32,
    /// Total SAD across all observed frames (motion intensity proxy)
    pub total_motion: u64,
    /// Inferred layer classification (updated periodically)
    pub layer: BlockLayer,
}

impl Default for BlockActivity {
    fn default() -> Self {
        Self {
            stable_streak: 0,
            last_hash: 0,
            min_samples: [255u8; 16],
            max_samples: [0u8; 16],
            avg_samples: [0.0; 16],
            frames_seen: 0,
            total_motion: 0,
            layer: BlockLayer::Unknown,
        }
    }
}

impl BlockActivity {
    /// Pure static = pixels never move at all
    pub fn is_pure_static(&self) -> bool {
        if self.frames_seen < 5 { return false; }
        // Range across all samples is essentially zero
        let max_range = (0..16).map(|i| {
            self.max_samples[i].saturating_sub(self.min_samples[i])
        }).max().unwrap_or(0);
        max_range <= 2  // tiny variation = pure static
    }
    
    /// Alpha overlay = pixels vary in TIGHT range (UI over BG)
    pub fn is_alpha_overlay(&self) -> bool {
        if self.frames_seen < 8 { return false; }
        // Each pixel oscillates within a small range, but range is non-zero
        let max_range = (0..16).map(|i| {
            self.max_samples[i].saturating_sub(self.min_samples[i])
        }).max().unwrap_or(0);
        // Range 3..30 = alpha-blended overlay (UI mixing with BG)
        // Range >30 = too much variance, probably full active
        (3..30).contains(&max_range)
    }
    
    /// Active = significant per-frame change
    pub fn is_active(&self) -> bool {
        if self.frames_seen < 3 { return false; }
        let avg_motion_per_frame = self.total_motion / self.frames_seen as u64;
        avg_motion_per_frame > 200  // SAD avg > 200 = real motion
    }
    
    /// Update min/max/avg from a new sample at this position
    pub fn observe_samples(&mut self, samples: &[u8; 16]) {
        let alpha = 0.1f32;  // EMA factor
        for i in 0..16 {
            self.min_samples[i] = self.min_samples[i].min(samples[i]);
            self.max_samples[i] = self.max_samples[i].max(samples[i]);
            self.avg_samples[i] = self.avg_samples[i] * (1.0 - alpha) + samples[i] as f32 * alpha;
        }
    }
    
    /// Classify layer based on observed stats (call periodically)
    /// 
    /// NOTE: ParallaxBg requires motion-direction info (from T-frame MVs)
    /// which we don't have here. Without it, "moving" blocks are classified Active.
    /// External code can promote Active → ParallaxBg if motion vectors are smooth.
    pub fn classify(&mut self) {
        self.layer = if self.is_pure_static() {
            BlockLayer::PureStatic
        } else if self.is_alpha_overlay() {
            BlockLayer::AlphaOverlay
        } else if self.is_active() {
            BlockLayer::Active
        } else {
            // Some motion but not enough to be "active" — likely noise or low-detail bg
            BlockLayer::Unknown
        };
    }
}

/// Tracks per-block activity across frames in a video.
/// Sized by block grid: bw × bh blocks at 16×16 each.
pub struct RegionActivityTracker {
    pub block_w: usize,
    pub block_h: usize,
    pub blocks: Vec<BlockActivity>,
    pub frame_count: u32,
    /// Number of stable blocks in last frame analyzed
    pub last_stable_count: usize,
    /// Frame-level stats: how many blocks changed in last frame
    pub last_changed_count: usize,
    /// Layer counts (updated by classify_all)
    pub layer_counts: [u32; 6],  // index by BlockLayer variant
}

impl RegionActivityTracker {
    pub fn new(w: usize, h: usize) -> Self {
        let bw = w / 16;
        let bh = h / 16;
        Self {
            block_w: bw,
            block_h: bh,
            blocks: vec![BlockActivity::default(); bw * bh],
            frame_count: 0,
            last_stable_count: 0,
            last_changed_count: 0,
            layer_counts: [0; 6],
        }
    }
    
    /// Hash a 16x16 block in the Y plane (FNV-1a, sparse 64-sample)
    fn hash_block(yuv: &[u8], w: usize, bx: usize, by: usize) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for py in (0..16).step_by(2) {
            for px in (0..16).step_by(2) {
                let x = bx * 16 + px;
                let y = by * 16 + py;
                let i = y * w + x;
                if i < yuv.len() {
                    h ^= yuv[i] as u64;
                    h = h.wrapping_mul(0x100000001b3);
                }
            }
        }
        h
    }
    
    /// Sample 16 pixels from a 16x16 block (4x4 grid)
    fn sample_block(yuv: &[u8], w: usize, bx: usize, by: usize) -> [u8; 16] {
        let mut samples = [0u8; 16];
        let mut idx = 0;
        for py in (0..16).step_by(4) {
            for px in (0..16).step_by(4) {
                let x = bx * 16 + px;
                let y = by * 16 + py;
                let i = y * w + x;
                if i < yuv.len() && idx < 16 {
                    samples[idx] = yuv[i];
                }
                idx += 1;
            }
        }
        samples
    }
    
    /// Compute SAD between two sample arrays
    fn samples_sad(a: &[u8; 16], b: &[u8; 16]) -> u64 {
        let mut sad: u64 = 0;
        for i in 0..16 {
            sad += (a[i] as i32 - b[i] as i32).unsigned_abs() as u64;
        }
        sad
    }
    
    /// Update activity stats from a new frame.
    /// Returns: (n_stable, n_changed) — useful for diagnostics.
    pub fn analyze_frame(&mut self, yuv: &[u8], w: usize, h: usize) -> (usize, usize) {
        let bw = self.block_w;
        let bh = self.block_h;
        let n_blocks = bw * bh;
        if self.blocks.len() != n_blocks { return (0, 0); }
        
        let mut stable = 0usize;
        let mut changed = 0usize;
        
        for bi in 0..n_blocks {
            let bx = bi % bw;
            let by = bi / bw;
            let new_hash = Self::hash_block(yuv, w, bx, by);
            let new_samples = Self::sample_block(yuv, w, bx, by);
            
            let block = &mut self.blocks[bi];
            
            // Track motion via sample SAD
            if block.frames_seen > 0 {
                let mut last_samples = [0u8; 16];
                for i in 0..16 {
                    last_samples[i] = block.avg_samples[i] as u8;
                }
                let sad = Self::samples_sad(&new_samples, &last_samples);
                block.total_motion = block.total_motion.saturating_add(sad);
            }
            
            // Track min/max/avg ranges (for alpha-overlay detection)
            block.observe_samples(&new_samples);
            
            block.frames_seen += 1;
            
            if block.frames_seen == 1 {
                block.last_hash = new_hash;
                block.stable_streak = 0;
            } else if new_hash == block.last_hash {
                block.stable_streak += 1;
                stable += 1;
            } else {
                block.last_hash = new_hash;
                block.stable_streak = 0;
                changed += 1;
            }
        }
        
        let _ = h;
        self.frame_count += 1;
        self.last_stable_count = stable;
        self.last_changed_count = changed;
        (stable, changed)
    }
    
    /// Classify all blocks into layers (call after analyzing several frames)
    pub fn classify_all(&mut self) {
        let mut counts = [0u32; 6];
        for block in self.blocks.iter_mut() {
            block.classify();
            let idx = match block.layer {
                BlockLayer::PureStatic => 0,
                BlockLayer::AlphaOverlay => 1,
                BlockLayer::ParallaxBg => 2,
                BlockLayer::Active => 3,
                BlockLayer::Noise => 4,
                BlockLayer::Unknown => 5,
            };
            counts[idx] += 1;
        }
        self.layer_counts = counts;
    }
    
    /// Quick lookup: is block (bx, by) stable?
    pub fn is_stable(&self, bx: usize, by: usize) -> bool {
        let bi = by * self.block_w + bx;
        self.blocks.get(bi).map(|b| b.is_pure_static() || b.is_alpha_overlay()).unwrap_or(false)
    }
    
    /// Layer of a specific block
    pub fn layer_at(&self, bx: usize, by: usize) -> BlockLayer {
        let bi = by * self.block_w + bx;
        self.blocks.get(bi).map(|b| b.layer).unwrap_or(BlockLayer::Unknown)
    }
    
    /// Get a stability mask: 1 bit per block (1 = pure or alpha-overlay static)
    pub fn stability_mask(&self) -> Vec<u8> {
        let n = self.blocks.len();
        let mut mask = vec![0u8; (n + 7) / 8];
        for (i, b) in self.blocks.iter().enumerate() {
            if b.is_pure_static() || b.is_alpha_overlay() {
                mask[i / 8] |= 1 << (i % 8);
            }
        }
        mask
    }
    
    /// Stats: percentage of blocks that are stable (pure or alpha-overlay)
    pub fn stable_ratio(&self) -> f32 {
        if self.blocks.is_empty() { return 0.0; }
        let stable = self.blocks.iter().filter(|b| b.is_pure_static() || b.is_alpha_overlay()).count();
        stable as f32 / self.blocks.len() as f32
    }
    
    /// Layer distribution as percentages: (pure, alpha, parallax, active, noise, unknown)
    pub fn layer_distribution(&self) -> [f32; 6] {
        let total = self.blocks.len() as f32;
        if total == 0.0 { return [0.0; 6]; }
        let mut dist = [0.0; 6];
        for (i, &c) in self.layer_counts.iter().enumerate() {
            dist[i] = c as f32 / total * 100.0;
        }
        dist
    }
}

// ═══════════════════════════════════════════════════════════
// Intra-frame block dedup: blocks repeating WITHIN a single frame
// ═══════════════════════════════════════════════════════════

pub struct IntraFrameDedup {
    pub block_map: HashMap<u64, u16>,
    pub unique_blocks: Vec<Vec<u8>>,
    pub block_ids: Vec<u16>,
    pub block_w: usize,
    pub block_h: usize,
}

impl IntraFrameDedup {
    pub fn new(w: usize, h: usize) -> Self {
        let bw = w / 16;
        let bh = h / 16;
        Self {
            block_map: HashMap::with_capacity(bw * bh / 4),
            unique_blocks: Vec::new(),
            block_ids: Vec::with_capacity(bw * bh),
            block_w: bw,
            block_h: bh,
        }
    }
    
    pub fn analyze_frame(&mut self, yuv: &[u8], w: usize, h: usize) {
        self.block_map.clear();
        self.unique_blocks.clear();
        self.block_ids.clear();
        
        let bw = self.block_w;
        let bh = self.block_h;
        
        for by in 0..bh {
            for bx in 0..bw {
                let mut block_pixels = Vec::with_capacity(256);
                for py in 0..16 {
                    for px in 0..16 {
                        let x = bx * 16 + px;
                        let y = by * 16 + py;
                        let i = if x < w && y < h { y * w + x } else { 0 };
                        block_pixels.push(yuv.get(i).copied().unwrap_or(0));
                    }
                }
                
                let mut hash: u64 = 0xcbf29ce484222325;
                for &b in &block_pixels {
                    hash ^= b as u64;
                    hash = hash.wrapping_mul(0x100000001b3);
                }
                
                let id = if let Some(&existing_id) = self.block_map.get(&hash) {
                    existing_id
                } else {
                    let new_id = self.unique_blocks.len() as u16;
                    self.unique_blocks.push(block_pixels);
                    self.block_map.insert(hash, new_id);
                    new_id
                };
                
                self.block_ids.push(id);
            }
            let _ = h;
        }
    }
    
    pub fn dedup_ratio(&self) -> f32 {
        if self.block_ids.is_empty() { return 1.0; }
        self.unique_blocks.len() as f32 / self.block_ids.len() as f32
    }
    
    pub fn estimated_savings(&self) -> usize {
        let total_blocks = self.block_ids.len();
        let unique_blocks = self.unique_blocks.len();
        let duplicates = total_blocks.saturating_sub(unique_blocks);
        duplicates.saturating_mul(256 - 2)
    }
}
