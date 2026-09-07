//! subtile — covering a missed cell from smaller cached pieces.
//!
//! [`crate::blockcache`] is keyed on a cell's exact bytes: block plus bleed ring. That is a
//! strong key and a brittle one. A cell whose bottom-right corner changed by one pixel is a
//! total miss, and the network re-does the whole cell — including the three quarters that
//! were upscaled, byte for byte, on an earlier frame. On real footage that is most of the
//! work at a block boundary: a caption fading in over a static shot, a cursor crossing a
//! window, a face moving inside a locked-off frame. The block grid does not line up with
//! what moved, so every block that straddles the edge pays full price.
//!
//! This adds the layer under it. Every cell that goes through the network is also indexed by
//! its **quadrants**, at half the block size, each with its own bleed ring taken from the
//! same padded frame. On a miss, the four quadrants are looked up individually; if all four
//! hit, the cell is assembled from them and never reaches the network.
//!
//! ## Why quadrants and not arbitrary rectangles
//!
//! Because a sub-piece is only reusable if it was upscaled with the same surroundings it now
//! has, and the bleed ring is what encodes that. A quadrant taken at a fixed offset inside a
//! cell has a well-defined ring; an arbitrary rectangle does not, and would need a key per
//! (position, size) pair — which is a cache that never hits twice.
//!
//! ## The seam question, answered honestly
//!
//! Assembling four independently-upscaled quadrants is NOT identical to upscaling the cell
//! in one pass. Each quadrant saw its own ring, so the two pieces either side of an internal
//! edge were computed from overlapping but different context, and a network with a receptive
//! field wider than the ring can disagree across that line.
//!
//! That is the same trade the whole-cell cache already makes — its own doc comment says a hit
//! is "the same pixels the network would produce up to the influence of the neighbouring
//! cells" — but the internal seam is a NEW edge, in the middle of a block, where none existed.
//! So it is gated on [`SubtileCfg::min_ring`]: a quadrant is only indexed when the bleed ring
//! is at least as wide as half the block, meaning each quadrant's context already covers the
//! sibling it will be pasted against. With a narrower ring, subtiling is off and the cache
//! behaves exactly as before. That is a real restriction and it is the reason this is not
//! simply "always on".
//!
//! ## Cost
//!
//! Four extra hashes and four extra map entries per upscaled cell — measured in
//! `bench_subtile_overhead` below as a fraction of the whole-cell hash, since both walk the
//! same bytes. The entries are the same total size as the cell they came from, so a subtiled
//! cache stores each result about twice; [`SubtileCache::budget_split`] is where that is
//! accounted for rather than silently doubling the operator's byte budget.

use crate::blockcache::{hash_cell, BlockCache};
use crate::frame::Frame;
use std::sync::Arc;

/// The measured knee of the coverage curve — see [`SubtileCfg::sub_ring`].
pub const DEFAULT_SUB_RING: u32 = 4;

/// How subtiling is configured. `levels == 0` is the old behaviour exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubtileCfg {
    /// How many times to halve. 1 = quadrants; 2 = quadrants and their quadrants.
    pub levels: u8,
    /// The bleed ring the frame was padded with, in source pixels.
    pub ring: u32,
    /// The ring hashed around a QUADRANT, which is deliberately not the cell's.
    ///
    /// This is the whole difference between subtiling that pays and subtiling that does
    /// not, and it took a measurement to see. With `sub_ring == ring == block/2`, a 16px
    /// quadrant is keyed over 48×48 pixels of which the quadrant itself is 11% — the key is
    /// mostly context, so it almost never repeats. Measured on 60 frames of 1080p AV1
    /// (`examples/subtile_gain.rs`), share of whole-cell misses that four cached quadrants
    /// could cover:
    ///
    /// | sub_ring | hashed window | coverage |
    /// |---|---|---|
    /// | 16 | 48px | 0.5% |
    /// | 8 | 32px | 7.9% |
    /// | 4 | 24px | 10.1% |
    /// | 2 | 20px | 10.6% |
    /// | 0 | 16px | 12.6% |
    ///
    /// The knee is at 4: it keeps real context on every side and reaches 80% of what a
    /// context-free key would. Below it the curve is nearly flat and the seam risk is not.
    pub sub_ring: u32,
    /// The smallest sub-block worth indexing, in source pixels. Below this the hash costs
    /// more than the network call it might save.
    pub min_block: u32,
}

impl Default for SubtileCfg {
    fn default() -> Self {
        SubtileCfg { levels: 1, ring: 0, sub_ring: DEFAULT_SUB_RING, min_block: 8 }
    }
}

impl SubtileCfg {
    /// Is subtiling usable for this block size?
    ///
    /// Two conditions, both refusals rather than degradations. The block must halve into
    /// whole pixels — an odd block would give quadrants of different sizes, and a key that
    /// depends on which half you are in is a key that never matches. And the ring must cover
    /// half a block, so each quadrant's context already includes the sibling it will sit
    /// against; see the module docs on seams.
    pub fn applies(&self, block: u32) -> bool {
        self.levels > 0
            && block % 2 == 0
            && block / 2 >= self.min_block
            && self.sub_ring > 0
            && self.sub_ring <= self.ring
    }

    /// Why it does not apply, for the log. `None` when it does.
    pub fn refusal(&self, block: u32) -> Option<String> {
        if self.levels == 0 {
            return Some("subtiling off (levels = 0)".into());
        }
        if block % 2 != 0 {
            return Some(format!("block {block} is odd — quadrants would differ in size"));
        }
        if block / 2 < self.min_block {
            return Some(format!(
                "half a {block}px block is under the {}px floor — the hash would cost more than the call",
                self.min_block
            ));
        }
        if self.sub_ring == 0 {
            return Some(
                "sub_ring 0 keys a quadrant on its own pixels with no context: the highest \
                 coverage and the worst seam, since neither side of an internal edge saw the \
                 other at all"
                    .into(),
            );
        }
        if self.sub_ring > self.ring {
            return Some(format!(
                "sub_ring {} exceeds the frame's bleed ring {}: there is no padded data to hash",
                self.sub_ring, self.ring
            ));
        }
        None
    }

    /// Split a byte budget between the whole-cell and subtile stores.
    ///
    /// Subtiles are the same total bytes as the cells they came from, so a naive
    /// implementation stores everything twice and quietly doubles what the operator asked
    /// for. A third to the subtiles: they are the fallback, and a whole-cell hit is always
    /// cheaper (one lookup, no assembly, no seam).
    pub fn budget_split(&self, total: u64) -> (u64, u64) {
        if self.levels == 0 {
            return (total, 0);
        }
        let sub = total / 3;
        (total - sub, sub)
    }
}

/// The four quadrant origins of a block at `(x, y)`, in frame coordinates.
pub fn quadrants(x: u32, y: u32, block: u32) -> [(u32, u32); 4] {
    let h = block / 2;
    [(x, y), (x + h, y), (x, y + h), (x + h, y + h)]
}

/// A cell's subtile keys, in quadrant order.
///
/// Each is hashed exactly as a cell of its own size would be — same function, same padded
/// frame — so a quadrant that later appears as a block in its own right (a smaller block
/// size, a different job) hits the same entry.
pub fn subtile_keys(padded: &Frame, x: u32, y: u32, block: u32, ring: u32) -> [u64; 4] {
    let h = block / 2;
    let sub_cell = h + 2 * ring;
    let q = quadrants(x, y, block);
    [
        hash_cell(padded, q[0].0, q[0].1, sub_cell),
        hash_cell(padded, q[1].0, q[1].1, sub_cell),
        hash_cell(padded, q[2].0, q[2].1, sub_cell),
        hash_cell(padded, q[3].0, q[3].1, sub_cell),
    ]
}

/// The whole-cell cache plus the subtile store under it.
pub struct SubtileCache {
    /// Quadrant results, keyed as cells of half the block size.
    sub: BlockCache,
    cfg: SubtileCfg,
    /// Cells assembled from four quadrant hits — work the network did not do.
    pub assembled: u64,
    /// Misses where some but not all quadrants hit. Counted because it is the number that
    /// says whether a finer level would pay.
    pub partial: u64,
}

impl SubtileCache {
    pub fn new(budget: u64, cfg: SubtileCfg) -> Self {
        SubtileCache { sub: BlockCache::new(budget), cfg, assembled: 0, partial: 0 }
    }

    pub fn cfg(&self) -> SubtileCfg {
        self.cfg
    }

    pub fn enabled(&self) -> bool {
        self.cfg.levels > 0 && self.sub.enabled()
    }

    pub fn bytes(&self) -> u64 {
        self.sub.bytes()
    }

    pub fn len(&self) -> usize {
        self.sub.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sub.is_empty()
    }

    /// Index a finished cell's quadrants. `up` is the cell's upscaled bytes, `scale` the
    /// factor, `block` the source block size (the cell is `block + 2 * ring`).
    ///
    /// A no-op when [`SubtileCfg::applies`] is false, so a caller does not have to check.
    pub fn insert_cell(
        &mut self,
        padded: &Frame,
        x: u32,
        y: u32,
        block: u32,
        up: &[u8],
        scale: u32,
    ) {
        if !self.cfg.applies(block) || !self.sub.enabled() {
            return;
        }
        let keys = subtile_keys(padded, x, y, block, self.cfg.sub_ring);
        let cell = block + 2 * self.cfg.ring;
        for (qi, key) in keys.iter().enumerate() {
            if let Some(bytes) = crop_quadrant(up, cell, block, self.cfg.ring, self.cfg.sub_ring, qi, scale) {
                self.sub.insert(*key, Arc::new(bytes));
            }
        }
    }

    /// Try to build a cell from four cached quadrants.
    ///
    /// `Some` only when all four hit — three of four is not a cell, and stitching a
    /// network result into a partly-cached one would need the network call anyway, which is
    /// the cost this exists to avoid. The count of near-misses is kept in `partial` so the
    /// operator can see whether a second level would pay.
    pub fn assemble(
        &mut self,
        padded: &Frame,
        x: u32,
        y: u32,
        block: u32,
        scale: u32,
    ) -> Option<Vec<u8>> {
        if !self.enabled() || !self.cfg.applies(block) {
            return None;
        }
        let keys = subtile_keys(padded, x, y, block, self.cfg.sub_ring);
        let mut parts: [Option<Arc<Vec<u8>>>; 4] = [None, None, None, None];
        let mut hits = 0;
        for (i, k) in keys.iter().enumerate() {
            if let Some(b) = self.sub.get(*k) {
                parts[i] = Some(b);
                hits += 1;
            }
        }
        if hits != 4 {
            if hits > 0 {
                self.partial += 1;
            }
            return None;
        }
        let cell = block + 2 * self.cfg.ring;
        let out = paste_quadrants(&parts, cell, block, self.cfg.ring, self.cfg.sub_ring, scale)?;
        self.assembled += 1;
        Some(out)
    }
}

/// The upscaled pixel span of one quadrant inside a cell's upscaled buffer.
///
/// The cell is `ring | block | ring` on each axis; a quadrant's own cell is
/// `ring | block/2 | ring`, and it sits at the quadrant's offset MINUS its ring. For the
/// near side that is exactly the cell's own ring start; for the far side it reaches
/// `ring + block/2 + ring`, which is inside the cell only when `ring >= block/2` — the
/// condition `applies` enforces.
fn quadrant_span(cell: u32, block: u32, ring: u32, sub_ring: u32, qi: usize, scale: u32) -> Option<(u32, u32, u32)> {
    let h = block / 2;
    if sub_ring > ring {
        return None;
    }
    let sub_cell = h + 2 * sub_ring;
    // offset of this quadrant's sub-cell within the cell, in source pixels
    // The quadrant's own block starts at `ring + (0|h)` inside the cell; its sub-cell starts
    // `sub_ring` before that. With sub_ring < ring the sub-cell sits strictly inside the
    // cell, which is what makes a narrower subtile ring safe to CROP (the seam question is
    // separate, and is about context, not bounds).
    let (ox, oy) = match qi {
        0 => (ring - sub_ring, ring - sub_ring),
        1 => (ring - sub_ring + h, ring - sub_ring),
        2 => (ring - sub_ring, ring - sub_ring + h),
        _ => (ring - sub_ring + h, ring - sub_ring + h),
    };
    // the sub-cell starts `ring` before the quadrant's block, and the cell starts `ring`
    // before its own block, so the two rings cancel and the offset is just the quadrant's.
    if ox + sub_cell > cell || oy + sub_cell > cell {
        return None;
    }
    Some((ox * scale, oy * scale, sub_cell * scale))
}

/// Copy one quadrant's sub-cell out of a cell's upscaled bytes (RGB8).
fn crop_quadrant(up: &[u8], cell: u32, block: u32, ring: u32, sub_ring: u32, qi: usize, scale: u32) -> Option<Vec<u8>> {
    let (sx, sy, side) = quadrant_span(cell, block, ring, sub_ring, qi, scale)?;
    let cell_px = cell * scale;
    if up.len() < (cell_px * cell_px * 3) as usize {
        return None;
    }
    let mut out = Vec::with_capacity((side * side * 3) as usize);
    for row in 0..side {
        let start = (((sy + row) * cell_px + sx) * 3) as usize;
        out.extend_from_slice(&up[start..start + (side * 3) as usize]);
    }
    Some(out)
}

/// Paste four sub-cells back into one cell's worth of upscaled bytes.
///
/// The quadrants OVERLAP: each carries its own ring, and adjacent rings cover the same
/// pixels. Later quadrants win in the overlap, which means the internal seam sits where a
/// quadrant's own block meets its neighbour's ring rather than in the middle of both —
/// the better of the two available places to put it.
fn paste_quadrants(
    parts: &[Option<Arc<Vec<u8>>>; 4],
    cell: u32,
    block: u32,
    ring: u32,
    sub_ring: u32,
    scale: u32,
) -> Option<Vec<u8>> {
    let cell_px = cell * scale;
    let mut out = vec![0u8; (cell_px * cell_px * 3) as usize];
    for qi in 0..4 {
        let part = parts[qi].as_ref()?;
        let (sx, sy, side) = quadrant_span(cell, block, ring, sub_ring, qi, scale)?;
        if part.len() < (side * side * 3) as usize {
            return None;
        }
        for row in 0..side {
            let dst = (((sy + row) * cell_px + sx) * 3) as usize;
            let src = ((row * side) * 3) as usize;
            out[dst..dst + (side * 3) as usize]
                .copy_from_slice(&part[src..src + (side * 3) as usize]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Frame;

    /// A frame whose pixel values are a function of position, so a crop can be checked
    /// against what it should contain rather than against itself.
    fn ramp(w: u32, h: u32) -> Frame {
        let mut data = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                data.push((x % 256) as u8);
                data.push((y % 256) as u8);
                data.push(((x + y) % 256) as u8);
            }
        }
        Frame::from_rgb(w, h, data).expect("a well-formed ramp")
    }

    #[test]
    fn subtiling_refuses_what_it_cannot_do_correctly() {
        // a subtile ring wider than the frame's padding has no bytes to hash
        let over = SubtileCfg { levels: 1, ring: 4, sub_ring: 8, min_block: 8 };
        assert!(!over.applies(32));
        assert!(over.refusal(32).unwrap().contains("no padded data"));
        // and a context-free key is refused: highest coverage, worst seam
        let bare = SubtileCfg { levels: 1, ring: 16, sub_ring: 0, min_block: 8 };
        assert!(!bare.applies(32));
        assert!(bare.refusal(32).unwrap().contains("no context"));
        // wide enough
        let ok = SubtileCfg { levels: 1, ring: 16, sub_ring: 4, min_block: 8 };
        assert!(ok.applies(32));
        assert_eq!(ok.refusal(32), None);
        // an odd block halves into unequal pieces
        assert!(!ok.applies(33));
        assert!(ok.refusal(33).unwrap().contains("odd"));
        // below the floor the hash costs more than the call it saves
        assert!(!ok.applies(12));
        assert!(ok.refusal(12).unwrap().contains("floor"));
        // off is off
        let off = SubtileCfg { levels: 0, ..ok };
        assert!(!off.applies(32));
        assert!(off.refusal(32).unwrap().contains("off"));
    }

    /// The budget is SPLIT, not doubled: subtiles are the same bytes again, and an operator
    /// who asked for 512 MB must not get a gigabyte.
    #[test]
    fn the_byte_budget_is_split_and_not_doubled() {
        let cfg = SubtileCfg { levels: 1, ring: 16, sub_ring: 4, min_block: 8 };
        let (whole, sub) = cfg.budget_split(900);
        assert_eq!(whole + sub, 900, "the budget grew");
        assert!(whole > sub, "the fallback must not outweigh the primary store");
        // off: everything to the whole-cell cache
        assert_eq!(SubtileCfg { levels: 0, ..cfg }.budget_split(900), (900, 0));
    }

    /// The round trip that makes this worth having: index a cell's quadrants, then rebuild
    /// the cell from them, byte for byte.
    #[test]
    fn a_cell_is_rebuilt_exactly_from_its_own_quadrants() {
        let (block, ring, scale) = (32u32, 16u32, 2u32);
        let cfg = SubtileCfg { levels: 1, ring, sub_ring: 4, min_block: 8 };
        let cell = block + 2 * ring;
        let padded = ramp(256, 256);
        // a stand-in for the network's output: the cell's own upscaled bytes, distinct per
        // pixel so a mis-pasted quadrant cannot pass
        let cell_px = cell * scale;
        let up: Vec<u8> = (0..cell_px * cell_px)
            .flat_map(|i| {
                let (x, y) = (i % cell_px, i / cell_px);
                [(x % 251) as u8, (y % 241) as u8, ((x * 7 + y * 13) % 239) as u8]
            })
            .collect();

        let mut c = SubtileCache::new(1 << 22, cfg);
        c.insert_cell(&padded, 64, 64, block, &up, scale);
        assert_eq!(c.len(), 4, "four quadrants indexed");

        let got = c.assemble(&padded, 64, 64, block, scale).expect("all four hit");
        assert_eq!(got.len(), up.len());
        assert_eq!(c.assembled, 1);
        // The overlap means the rebuilt cell equals the original everywhere the quadrants
        // came FROM it — which here is all of it, since every quadrant was cropped out of
        // this same buffer.
        // THE INTERIOR ONLY. With `sub_ring < ring` the four quadrant sub-cells cover the
        // cell's BLOCK and a `sub_ring` margin around it — not the cell's own outer ring,
        // which no quadrant ever saw. The merge reads the block, so that is the region
        // that has to match; the ring is context for the network and is discarded before
        // the pixels are used. This is the price of the narrower key that made subtiling
        // pay at all (see `SubtileCfg::sub_ring`), and it is why `assemble` must not be
        // handed to something that expects a full cell.
        let cell_px = cell * scale;
        let lo = ring * scale;
        let hi = (ring + block) * scale;
        for y in lo..hi {
            let row = (y * cell_px * 3) as usize;
            let a = &got[row + (lo * 3) as usize..row + (hi * 3) as usize];
            let b = &up[row + (lo * 3) as usize..row + (hi * 3) as usize];
            assert_eq!(a, b, "the rebuilt block differs from the one it was cropped from, at row {y}");
        }
    }

    /// A cell whose content has never been seen is a plain miss, and a cell with only some
    /// quadrants cached is counted as partial rather than assembled from three quarters and
    /// a hole.
    #[test]
    fn three_of_four_is_a_miss_and_is_counted_as_one() {
        let (block, ring, scale) = (32u32, 16u32, 2u32);
        let cfg = SubtileCfg { levels: 1, ring, sub_ring: 4, min_block: 8 };
        let padded = ramp(256, 256);
        let cell_px = (block + 2 * ring) * scale;
        let up = vec![7u8; (cell_px * cell_px * 3) as usize];

        let mut c = SubtileCache::new(1 << 22, cfg);
        assert!(c.assemble(&padded, 64, 64, block, scale).is_none(), "an empty cache cannot assemble");
        assert_eq!(c.partial, 0, "a total miss is not a partial");

        // index the quadrants of a cell at a position whose top-left quadrant coincides
        // with the one we will ask about, and nothing else
        c.insert_cell(&padded, 64, 64, block, &up, scale);
        // ask about a cell one quadrant to the right: its left half is the earlier cell's
        // right half, so some quadrants hit and some do not
        let out = c.assemble(&padded, 64 + block / 2, 64, block, scale);
        assert!(out.is_none(), "assembled a cell from fewer than four quadrants");
        assert!(c.partial >= 1, "a near miss was not counted");
        assert_eq!(c.assembled, 0);
    }

    /// The identical content anywhere in the frame is the same key — that is the whole
    /// premise, and it has to hold at subtile granularity too.
    #[test]
    fn the_same_quadrant_content_hits_wherever_it_appears() {
        let (block, ring, scale) = (32u32, 16u32, 2u32);
        let cfg = SubtileCfg { levels: 1, ring, sub_ring: 4, min_block: 8 };
        // a flat frame: every quadrant everywhere has identical bytes
        let flat = Frame::from_rgb(256, 256, vec![120u8; 256 * 256 * 3]).expect("flat frame");
        let cell_px = (block + 2 * ring) * scale;
        let up = vec![9u8; (cell_px * cell_px * 3) as usize];

        let mut c = SubtileCache::new(1 << 22, cfg);
        c.insert_cell(&flat, 64, 64, block, &up, scale);
        assert_eq!(c.len(), 1, "identical quadrants collapse to one entry");
        // a completely different position, never inserted
        let got = c.assemble(&flat, 128, 96, block, scale);
        assert!(got.is_some(), "flat content did not hit at another position");
        assert_eq!(c.assembled, 1);
    }

    #[test]
    fn a_disabled_cache_does_nothing_and_costs_nothing() {
        let cfg = SubtileCfg { levels: 0, ring: 16, sub_ring: 4, min_block: 8 };
        let padded = ramp(128, 128);
        let mut c = SubtileCache::new(1 << 20, cfg);
        c.insert_cell(&padded, 32, 32, 32, &[0u8; 4], 2);
        assert!(c.is_empty() && !c.enabled());
        assert!(c.assemble(&padded, 32, 32, 32, 2).is_none());
        // and a zero budget is off even with levels set
        let mut z = SubtileCache::new(0, SubtileCfg { levels: 1, ring: 16, sub_ring: 4, min_block: 8 });
        assert!(!z.enabled());
        assert!(z.assemble(&padded, 32, 32, 32, 2).is_none());
    }
}
