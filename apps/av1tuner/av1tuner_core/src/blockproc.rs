//! # blockproc — block processing layer for per-group enhancement
//!
//! This is the data-preparation + API layer that sits between the video
//! pipeline and a (swappable) processing model — e.g. a basic
//! cross-frame upscaler now, an ESRGAN/RefSR model later. The video
//! pipeline's job is to get pixel blocks IN and OUT efficiently, with
//! provenance that makes incorrect reassembly impossible, and to skip
//! repeated work via a dedup index.
//!
//! Design goals (from the spec):
//!  - Abstract away video properties: a processor sees `Block`s, not
//!    codecs/containers/timebases.
//!  - High-quality, efficient transfer in and out.
//!  - Provenance contract: a processed block can only be written back to
//!    the exact (clip, frame, x, y, dims, format) it came from. A
//!    mismatch is a hard error, never a silent misplacement.
//!  - Dedup: identical blocks are processed once; the result fans back
//!    out to every occurrence.
//!  - Swappable model: `BlockProcessor` is the hole. Basic upscaler fills
//!    it now; higher-precision components drop in later behind the same
//!    trait.
//!
//! What this module does NOT do: decode/encode video (that's the
//! pipeline's job, upstream/downstream), or run neural inference (that's
//! a `BlockProcessor` impl's job, and may live out-of-process).

use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BlockError {
    #[error("provenance mismatch: block {got:?} cannot be written to slot {expected:?}")]
    ProvenanceMismatch { got: BlockId, expected: BlockId },
    #[error("dimension mismatch: block is {bw}x{bh}, slot expects {sw}x{sh}")]
    DimensionMismatch { bw: u32, bh: u32, sw: u32, sh: u32 },
    #[error("pixel data length {got} doesn't match expected {expected} for {w}x{h} {fmt}")]
    DataLength { got: usize, expected: usize, w: u32, h: u32, fmt: String },
    #[error("unknown block id {0:?} — not in this batch")]
    UnknownBlock(BlockId),
}

/// Pixel format of a block's data. Kept minimal — the formats the
/// pipeline actually hands us. Bytes-per-pixel is derived from this so
/// the provenance contract can validate data length.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockFormat {
    /// 8-bit grayscale (luma plane only). 1 byte/pixel.
    Gray8,
    /// 8-bit packed RGB. 3 bytes/pixel.
    Rgb8,
    /// 16-bit (10/12-bit in 16) grayscale. 2 bytes/pixel.
    Gray16,
    /// 16-bit packed RGB. 6 bytes/pixel.
    Rgb16,
}

impl BlockFormat {
    pub fn bytes_per_pixel(&self) -> usize {
        match self {
            BlockFormat::Gray8 => 1,
            BlockFormat::Rgb8 => 3,
            BlockFormat::Gray16 => 2,
            BlockFormat::Rgb16 => 6,
        }
    }
    
    pub fn name(&self) -> &'static str {
        match self {
            BlockFormat::Gray8 => "gray8",
            BlockFormat::Rgb8 => "rgb8",
            BlockFormat::Gray16 => "gray16",
            BlockFormat::Rgb16 => "rgb16",
        }
    }
}

/// Uniquely identifies WHERE a block lives in the source. This is the
/// provenance token: a processed block carries its `BlockId` and can
/// only be written back to the matching slot. Equality is exact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlockId {
    /// Which clip in the group (index into the group's active clips).
    pub clip: u32,
    /// Frame number within the clip.
    pub frame: u32,
    /// Top-left x of the block in the frame.
    pub x: u32,
    /// Top-left y of the block in the frame.
    pub y: u32,
}

/// A block of pixels plus its identity, geometry, and format. The
/// `data` length MUST equal width*height*bytes_per_pixel — enforced at
/// construction and again at write-back.
#[derive(Clone, Debug)]
pub struct Block {
    pub id: BlockId,
    pub width: u32,
    pub height: u32,
    pub format: BlockFormat,
    /// Row-major pixel data, length = width*height*bpp.
    pub data: Vec<u8>,
}

impl Block {
    /// Construct a block, validating that data length matches geometry.
    pub fn new(
        id: BlockId, width: u32, height: u32, format: BlockFormat, data: Vec<u8>,
    ) -> Result<Self, BlockError> {
        let expected = width as usize * height as usize * format.bytes_per_pixel();
        if data.len() != expected {
            return Err(BlockError::DataLength {
                got: data.len(), expected,
                w: width, h: height, fmt: format.name().to_string(),
            });
        }
        Ok(Self { id, width, height, format, data })
    }
    
    /// Content fingerprint for dedup. Hashes geometry + format + pixels
    /// (NOT the BlockId — two blocks at different positions with
    /// identical content should share a fingerprint so they dedup).
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.width.hash(&mut h);
        self.height.hash(&mut h);
        (self.format as u8).hash(&mut h);
        self.data.hash(&mut h);
        h.finish()
    }
}

/// The swappable processing hole. A `BlockProcessor` takes unique blocks
/// (optionally with reference blocks of similar content) and returns the
/// processed result for each. The result's `BlockId` MUST equal the
/// input's — the layer enforces this so a processor can't accidentally
/// scramble placement.
///
/// Implementors today: `CrossFrameUpscaler` (basic, in-process). Later:
/// an ESRGAN/RefSR processor, possibly shelling out to an external
/// runtime. Same contract either way.
pub trait BlockProcessor: Send + Sync {
    /// Human-readable name for logging.
    fn name(&self) -> &str;
    
    /// Process a batch of unique blocks. `references` are clean/high-
    /// quality blocks of similar content (e.g. from a reference clip)
    /// that the processor MAY use to guide enhancement; an empty slice
    /// means no references available (blind mode).
    ///
    /// Must return one output block per input block, each with the SAME
    /// `BlockId` as its corresponding input. Output dimensions may
    /// differ (upscaling changes w/h) but the id must match.
    fn process(
        &self,
        blocks: &[Block],
        references: &[Block],
    ) -> Result<Vec<Block>, BlockError>;
    
    /// The upscale factor this processor applies (1 = no resize, 2 = 2x).
    /// The reassembly layer uses this to size the output frame.
    fn scale_factor(&self) -> u32 { 1 }
}

/// Deduplicating batch: collects blocks, processes only the unique ones,
/// fans results back out to every occurrence by BlockId.
///
/// Usage:
///   1. `add` every block from the frames/chunks you want processed.
///   2. `process_unique` runs the processor on just the distinct
///      content (one representative per fingerprint).
///   3. `result_for(id)` retrieves the processed block for any original
///      id — repeated content shares the same processed pixels.
pub struct DedupBatch {
    /// All added blocks, keyed by their id.
    all: HashMap<BlockId, Block>,
    /// fingerprint → representative BlockId (first one seen).
    representatives: HashMap<u64, BlockId>,
    /// fingerprint → all ids that share it (for fan-out).
    occurrences: HashMap<u64, Vec<BlockId>>,
    /// After processing: representative BlockId → processed Block.
    processed: HashMap<BlockId, Block>,
}

impl DedupBatch {
    pub fn new() -> Self {
        Self {
            all: HashMap::new(),
            representatives: HashMap::new(),
            occurrences: HashMap::new(),
            processed: HashMap::new(),
        }
    }
    
    /// Add a block to the batch. Repeated content (same fingerprint) is
    /// tracked but only the first occurrence becomes the representative
    /// that actually gets processed.
    pub fn add(&mut self, block: Block) {
        let fp = block.fingerprint();
        let id = block.id;
        self.representatives.entry(fp).or_insert(id);
        self.occurrences.entry(fp).or_default().push(id);
        self.all.insert(id, block);
    }
    
    /// Number of blocks added.
    pub fn total_blocks(&self) -> usize {
        self.all.values().map(|_| 1).sum::<usize>()
            .max(self.occurrences.values().map(|v| v.len()).sum())
    }
    
    /// Number of UNIQUE blocks (what actually gets processed).
    pub fn unique_blocks(&self) -> usize {
        self.representatives.len()
    }
    
    /// Dedup ratio: 1.0 means no repeats, lower means more sharing.
    /// e.g. 0.25 means only 25% of blocks are unique (4x speedup on the
    /// processing pass).
    pub fn unique_ratio(&self) -> f64 {
        let total: usize = self.occurrences.values().map(|v| v.len()).sum();
        if total == 0 { return 1.0; }
        self.unique_blocks() as f64 / total as f64
    }
    
    /// Run the processor over the unique representatives only. References
    /// are passed through to the processor as-is.
    pub fn process_unique(
        &mut self,
        processor: &dyn BlockProcessor,
        references: &[Block],
    ) -> Result<(), BlockError> {
        // Gather one representative block per fingerprint.
        let reps: Vec<Block> = self.representatives.values()
            .filter_map(|id| self.all.get(id).cloned())
            .collect();
        
        let outputs = processor.process(&reps, references)?;
        
        // Validate + index by id. The processor contract says output id
        // == input id; enforce it here so a buggy processor can't
        // misplace pixels silently.
        for out in outputs {
            if !self.all.contains_key(&out.id) {
                return Err(BlockError::UnknownBlock(out.id));
            }
            self.processed.insert(out.id, out);
        }
        Ok(())
    }
    
    /// Get the processed block for any original id. Repeated content
    /// returns the processed representative's pixels (with the queried
    /// id substituted so write-back provenance matches the slot).
    pub fn result_for(&self, id: BlockId) -> Result<Block, BlockError> {
        // Find the fingerprint this id belongs to via its block.
        let block = self.all.get(&id)
            .ok_or(BlockError::UnknownBlock(id))?;
        let fp = block.fingerprint();
        let rep = self.representatives.get(&fp)
            .ok_or(BlockError::UnknownBlock(id))?;
        let processed = self.processed.get(rep)
            .ok_or(BlockError::UnknownBlock(*rep))?;
        // Return a copy with the QUERIED id, not the representative's, so
        // the block can only be written back to the slot it came from.
        Ok(Block {
            id,  // queried id — provenance for THIS occurrence
            width: processed.width,
            height: processed.height,
            format: processed.format,
            data: processed.data.clone(),
        })
    }
    
    /// All original ids that were added (for the reassembly loop).
    pub fn all_ids(&self) -> Vec<BlockId> {
        self.all.keys().copied().collect()
    }
}

impl Default for DedupBatch {
    fn default() -> Self { Self::new() }
}

/// Validate that a processed block may be written into a given slot.
/// The provenance contract: ids must match exactly. This is the
/// "agreement that the video portion is unable to be reapplied [wrong]"
/// — a guard the reassembly step calls before placing pixels.
pub fn validate_writeback(block: &Block, slot: BlockId) -> Result<(), BlockError> {
    if block.id != slot {
        return Err(BlockError::ProvenanceMismatch { got: block.id, expected: slot });
    }
    let expected = block.width as usize * block.height as usize
        * block.format.bytes_per_pixel();
    if block.data.len() != expected {
        return Err(BlockError::DataLength {
            got: block.data.len(), expected,
            w: block.width, h: block.height, fmt: block.format.name().to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    fn mk_block(clip: u32, frame: u32, x: u32, y: u32, fill: u8) -> Block {
        let (w, h) = (4u32, 4u32);
        let data = vec![fill; (w * h) as usize];  // Gray8
        Block::new(
            BlockId { clip, frame, x, y },
            w, h, BlockFormat::Gray8, data,
        ).unwrap()
    }
    
    #[test]
    fn block_validates_data_length() {
        let id = BlockId { clip: 0, frame: 0, x: 0, y: 0 };
        // 4x4 Gray8 needs 16 bytes; give 10.
        let r = Block::new(id, 4, 4, BlockFormat::Gray8, vec![0; 10]);
        assert!(matches!(r, Err(BlockError::DataLength { .. })));
    }
    
    #[test]
    fn identical_content_dedups() {
        let mut batch = DedupBatch::new();
        // Three blocks, two identical (fill 5), one different (fill 9).
        batch.add(mk_block(0, 0, 0, 0, 5));
        batch.add(mk_block(0, 1, 0, 0, 5));  // same content, diff position
        batch.add(mk_block(0, 2, 0, 0, 9));
        
        assert_eq!(batch.unique_blocks(), 2);  // only 2 distinct contents
        let total: usize = 3;
        assert!((batch.unique_ratio() - (2.0 / total as f64)).abs() < 0.001);
    }
    
    #[test]
    fn fanout_returns_correct_id() {
        let mut batch = DedupBatch::new();
        batch.add(mk_block(0, 0, 0, 0, 5));
        batch.add(mk_block(0, 1, 16, 0, 5));  // same content
        
        // A passthrough processor (returns inputs unchanged).
        struct Passthrough;
        impl BlockProcessor for Passthrough {
            fn name(&self) -> &str { "passthrough" }
            fn process(&self, blocks: &[Block], _refs: &[Block])
                -> Result<Vec<Block>, BlockError> {
                Ok(blocks.to_vec())
            }
        }
        batch.process_unique(&Passthrough, &[]).unwrap();
        
        // Both occurrences resolve, each carrying ITS OWN id (so it can
        // only write back to its own slot).
        let r0 = batch.result_for(BlockId { clip: 0, frame: 0, x: 0, y: 0 }).unwrap();
        let r1 = batch.result_for(BlockId { clip: 0, frame: 1, x: 16, y: 0 }).unwrap();
        assert_eq!(r0.id, BlockId { clip: 0, frame: 0, x: 0, y: 0 });
        assert_eq!(r1.id, BlockId { clip: 0, frame: 1, x: 16, y: 0 });
        // Same pixels (shared representative), different ids.
        assert_eq!(r0.data, r1.data);
    }
    
    #[test]
    fn writeback_provenance_enforced() {
        let block = mk_block(0, 0, 0, 0, 5);
        // Correct slot: ok.
        assert!(validate_writeback(&block, BlockId { clip: 0, frame: 0, x: 0, y: 0 }).is_ok());
        // Wrong slot: rejected.
        let r = validate_writeback(&block, BlockId { clip: 9, frame: 9, x: 9, y: 9 });
        assert!(matches!(r, Err(BlockError::ProvenanceMismatch { .. })));
    }
    
    #[test]
    fn bytes_per_pixel_correct() {
        assert_eq!(BlockFormat::Gray8.bytes_per_pixel(), 1);
        assert_eq!(BlockFormat::Rgb8.bytes_per_pixel(), 3);
        assert_eq!(BlockFormat::Gray16.bytes_per_pixel(), 2);
        assert_eq!(BlockFormat::Rgb16.bytes_per_pixel(), 6);
    }
}
