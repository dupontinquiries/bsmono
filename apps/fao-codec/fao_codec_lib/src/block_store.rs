// FAO Block Store — cache-friendly SOA layout for the prediction web
//
// All per-block data is stored in flat contiguous arrays indexed by block_id.
// block_id = by * blocks_wide + bx (row-major, 8×8 blocks)
//
// This eliminates pointer chasing: scanning all DCs is a sequential
// memory walk, and accessing all properties of one block is N cache-line
// loads from predictable addresses.

/// Maximum trajectory history per block (fixed size, no heap allocation)
const TRAJ_LEN: usize = 8;

/// Maximum anchors per block
const MAX_ANCHORS: usize = 2;

/// Flat, cache-friendly storage for all per-block prediction state.
/// One allocation, zero pointer chasing.
pub struct BlockStore {
    pub n: usize,           // total blocks (bw × bh)
    pub bw: usize,          // blocks wide
    pub bh: usize,          // blocks high

    // ── Spatial properties (per block) ──
    pub dc_y:       Vec<f32>,   // current DC luma
    pub dc_cb:      Vec<f32>,   // current DC chroma blue
    pub dc_cr:      Vec<f32>,   // current DC chroma red
    pub variance:   Vec<f32>,   // spatial variance (for classification)
    pub edge_energy:Vec<f32>,   // gradient magnitude
    pub dir_mode:   Vec<u8>,    // dominant directional mode (DirMode as u8)

    // ── Motion properties ──
    pub mv_x:       Vec<f32>,   // current motion vector X
    pub mv_y:       Vec<f32>,   // current motion vector Y
    pub mv_conf:    Vec<f32>,   // trajectory prediction confidence
    pub traj_x:     Vec<[f32; TRAJ_LEN]>,  // MV history X (fixed ring)
    pub traj_y:     Vec<[f32; TRAJ_LEN]>,  // MV history Y (fixed ring)
    pub traj_len:   Vec<u8>,    // how many history entries are valid
    pub traj_idx:   Vec<u8>,    // current write index in ring

    // ── State tracking ──
    pub skip:       Vec<bool>,  // was this block skipped last frame?
    pub static_count: Vec<u16>, // consecutive static frames
    pub is_ui:      Vec<bool>,  // classified as UI element
    pub last_encode_bits: Vec<u16>, // bits used to encode this block last time

    // ── Anchor store (flat, no inner Vecs) ──
    pub anchor_y:   Vec<[u8; 64]>,  // N * MAX_ANCHORS entries
    pub anchor_cb:  Vec<[u8; 64]>,
    pub anchor_cr:  Vec<[u8; 64]>,
    pub anchor_age: Vec<u16>,
    pub anchor_hits:Vec<u16>,
    pub anchor_valid:Vec<bool>,     // is this anchor slot populated?

    // ── Function nodes (v2 extension point) ──
    pub func_id:    Vec<u32>,   // which function (pattern) this block is assigned to
    pub func_param: Vec<[f32; 4]>, // function parameters (position offset, scale, etc.)
}

impl BlockStore {
    pub fn new(w: usize, h: usize) -> Self {
        let bw = w / 8;
        let bh = h / 8;
        let n = bw * bh;
        let na = n * MAX_ANCHORS;

        Self {
            n, bw, bh,
            dc_y:       vec![128.0; n],
            dc_cb:      vec![128.0; n],
            dc_cr:      vec![128.0; n],
            variance:   vec![0.0; n],
            edge_energy:vec![0.0; n],
            dir_mode:   vec![0; n],

            mv_x:       vec![0.0; n],
            mv_y:       vec![0.0; n],
            mv_conf:    vec![0.0; n],
            traj_x:     vec![[0.0; TRAJ_LEN]; n],
            traj_y:     vec![[0.0; TRAJ_LEN]; n],
            traj_len:   vec![0; n],
            traj_idx:   vec![0; n],

            skip:       vec![false; n],
            static_count: vec![0; n],
            is_ui:      vec![false; n],
            last_encode_bits: vec![0; n],

            anchor_y:   vec![[0; 64]; na],
            anchor_cb:  vec![[0; 64]; na],
            anchor_cr:  vec![[0; 64]; na],
            anchor_age: vec![0; na],
            anchor_hits:vec![0; na],
            anchor_valid:vec![false; na],

            func_id:    vec![u32::MAX; n],  // MAX = no function assigned
            func_param: vec![[0.0; 4]; n],
        }
    }

    /// Memory usage in bytes
    pub fn memory_bytes(&self) -> usize {
        let per_block = 4*3 + 4*2 + 4 + 1  // dc, variance, edge, dir_mode
            + 4*3                            // mv_x, mv_y, mv_conf
            + 4*TRAJ_LEN*2 + 1 + 1          // trajectory rings
            + 1 + 2 + 1 + 2                 // skip, static, is_ui, bits
            + 4 + 4*4;                       // func_id, func_param
        let per_anchor = 64*3 + 2 + 2 + 1;  // y, cb, cr, age, hits, valid
        self.n * per_block + self.n * MAX_ANCHORS * per_anchor
    }

    // ── Trajectory operations (inline, no function pointer overhead) ──

    /// Record a motion vector for a block
    #[inline]
    pub fn record_mv(&mut self, bi: usize, mx: f32, my: f32) {
        if bi >= self.n { return; }
        let idx = self.traj_idx[bi] as usize;
        self.traj_x[bi][idx] = mx;
        self.traj_y[bi][idx] = my;
        self.traj_idx[bi] = ((idx + 1) % TRAJ_LEN) as u8;
        if (self.traj_len[bi] as usize) < TRAJ_LEN {
            self.traj_len[bi] += 1;
        }
        self.mv_x[bi] = mx;
        self.mv_y[bi] = my;
    }

    /// Predict next MV from trajectory history (simple EMA)
    #[inline]
    pub fn predict_mv(&self, bi: usize) -> (f32, f32, f32) {
        if bi >= self.n || self.traj_len[bi] == 0 {
            return (0.0, 0.0, 0.0);
        }
        let len = self.traj_len[bi] as usize;
        let idx = self.traj_idx[bi] as usize;

        // EMA with adaptive reset
        let alpha = 0.4f32;
        let mut sx = self.traj_x[bi][(idx + TRAJ_LEN - len) % TRAJ_LEN];
        let mut sy = self.traj_y[bi][(idx + TRAJ_LEN - len) % TRAJ_LEN];

        for i in 1..len {
            let pos = (idx + TRAJ_LEN - len + i) % TRAJ_LEN;
            let vx = self.traj_x[bi][pos];
            let vy = self.traj_y[bi][pos];
            // Adaptive: reset on sudden change
            if (vx - sx).abs() > 20.0 { sx = vx; }
            else { sx = sx * (1.0 - alpha) + vx * alpha; }
            if (vy - sy).abs() > 20.0 { sy = vy; }
            else { sy = sy * (1.0 - alpha) + vy * alpha; }
        }

        // Confidence from variance of history
        let mut var_x = 0.0f32;
        let mut var_y = 0.0f32;
        for i in 0..len {
            let pos = (idx + TRAJ_LEN - len + i) % TRAJ_LEN;
            var_x += (self.traj_x[bi][pos] - sx).powi(2);
            var_y += (self.traj_y[bi][pos] - sy).powi(2);
        }
        let conf = 1.0 / (1.0 + (var_x + var_y) / len as f32);

        self.mv_conf[bi]; // touch for cache warmth
        (sx, sy, conf)
    }

    // ── Anchor operations ──

    #[inline]
    fn anchor_idx(&self, bi: usize, slot: usize) -> usize {
        bi * MAX_ANCHORS + slot
    }

    /// Store an anchor for a block (returns true if stored)
    pub fn store_anchor(&mut self, bi: usize, y: &[f32; 64], cb: &[f32; 64], cr: &[f32; 64]) -> bool {
        if bi >= self.n { return false; }

        // Check novelty against existing anchors
        for s in 0..MAX_ANCHORS {
            let ai = self.anchor_idx(bi, s);
            if !self.anchor_valid[ai] { continue; }
            let mut diff = 0u32;
            for i in 0..64 {
                diff += (y[i].clamp(0.0, 255.0) as u8 as i16 - self.anchor_y[ai][i] as i16).unsigned_abs() as u32;
            }
            if diff / 64 <= 5 { return false; } // too similar
        }

        // Find a slot: empty or evict worst
        let mut slot = None;
        for s in 0..MAX_ANCHORS {
            let ai = self.anchor_idx(bi, s);
            if !self.anchor_valid[ai] { slot = Some(s); break; }
        }
        if slot.is_none() {
            // Evict: lowest hits, oldest
            let mut worst = 0;
            let mut worst_score = u32::MAX;
            for s in 0..MAX_ANCHORS {
                let ai = self.anchor_idx(bi, s);
                let score = (self.anchor_hits[ai] as u32) * 1000 - self.anchor_age[ai] as u32;
                if score < worst_score { worst_score = score; worst = s; }
            }
            slot = Some(worst);
        }

        let s = slot.unwrap();
        let ai = self.anchor_idx(bi, s);
        for i in 0..64 {
            self.anchor_y[ai][i] = y[i].clamp(0.0, 255.0) as u8;
            self.anchor_cb[ai][i] = cb[i].clamp(0.0, 255.0) as u8;
            self.anchor_cr[ai][i] = cr[i].clamp(0.0, 255.0) as u8;
        }
        self.anchor_age[ai] = 0;
        self.anchor_hits[ai] = 0;
        self.anchor_valid[ai] = true;
        true
    }

    /// Find the best matching anchor for a block
    pub fn find_anchor(&self, bi: usize, cur_y: &[f32; 64]) -> Option<(usize, f32)> {
        if bi >= self.n { return None; }
        let mut best: Option<(usize, f32)> = None;

        for s in 0..MAX_ANCHORS {
            let ai = self.anchor_idx(bi, s);
            if !self.anchor_valid[ai] { continue; }
            let mut sad = 0.0f32;
            for i in 0..64 {
                sad += (cur_y[i] - self.anchor_y[ai][i] as f32).abs();
            }
            if best.is_none() || sad < best.unwrap().1 {
                best = Some((ai, sad));
            }
        }
        best
    }

    /// Age all anchors and evict expired ones
    pub fn age_anchors(&mut self) {
        for ai in 0..self.n * MAX_ANCHORS {
            if self.anchor_valid[ai] {
                self.anchor_age[ai] = self.anchor_age[ai].saturating_add(1);
                if self.anchor_age[ai] > 120 && self.anchor_hits[ai] == 0 {
                    self.anchor_valid[ai] = false;
                }
                if self.anchor_age[ai] > 300 && self.anchor_hits[ai] < 3 {
                    self.anchor_valid[ai] = false;
                }
            }
        }
    }

    // ── Batch operations (SIMD-friendly) ──

    /// Update all block DCs from a Y plane
    pub fn update_dcs(&mut self, y: &[f32], w: usize) {
        for by in 0..self.bh {
            for bx in 0..self.bw {
                let bi = by * self.bw + bx;
                let ox = bx * 8; let oy = by * 8;
                let mut sum = 0.0f32;
                for dy in 0..8 { for dx in 0..8 {
                    sum += y[(oy + dy) * w + ox + dx];
                }}
                self.dc_y[bi] = sum / 64.0;
            }
        }
    }

    /// Classify all blocks (UI detection from static_count)
    pub fn update_ui_flags(&mut self) {
        for bi in 0..self.n {
            self.is_ui[bi] = self.static_count[bi] > 30;
        }
    }

    /// Count blocks by category
    pub fn stats(&self) -> (usize, usize, usize, usize) {
        let mut n_skip = 0; let mut n_ui = 0;
        let mut n_anchored = 0; let mut n_func = 0;
        for bi in 0..self.n {
            if self.skip[bi] { n_skip += 1; }
            if self.is_ui[bi] { n_ui += 1; }
            for s in 0..MAX_ANCHORS {
                if self.anchor_valid[self.anchor_idx(bi, s)] { n_anchored += 1; break; }
            }
            if self.func_id[bi] != u32::MAX { n_func += 1; }
        }
        (n_skip, n_ui, n_anchored, n_func)
    }
}
