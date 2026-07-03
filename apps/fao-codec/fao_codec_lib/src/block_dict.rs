// Persistent Block Dictionary — global 16x16 block reuse across frames
//
// IDEA: in game footage, the SAME 16x16 luma blocks appear thousands of times:
// - UI/HUD elements (health bar, minimap frame, ability icons)
// - Repeated environmental textures (ground tiles, wall patterns)
// - Static character sprites
//
// Traditional codecs RE-ENCODE these blocks each time they appear.
// We build a GLOBAL DICTIONARY of unique blocks, frames just reference them by ID.
//
// Storage win for 4800-frame game video:
//   ~5000 unique blocks across the entire video (HUD + scenes + sprites)
//   Each block: 256 bytes raw, ~80 bytes after zstd
//   Dictionary total: ~400 KB
//
//   Per-frame: 5184 blocks × 2 byte IDs = 10 KB (uncompressed)
//   With block_id RLE + zstd: ~500 bytes per frame
//
//   For 4800 frames × ~500 bytes = 2.4 MB + 400 KB dictionary = 2.8 MB total
//   vs T-frames at ~10-30 KB each = 50-150 MB
//
// File size win: 20-50x SMALLER than current T-frame approach for game footage

use std::collections::HashMap;
use std::io::Cursor;

/// Persistent block dictionary
pub struct BlockDictionary {
    /// hash → (id, pixel_data, ref_count)
    pub blocks: HashMap<u64, (u32, Vec<u8>, u32)>,
    /// reverse lookup: id → pixel_data
    pub by_id: Vec<Vec<u8>>,
    /// next id to assign
    pub next_id: u32,
}

impl BlockDictionary {
    pub fn new() -> Self {
        Self {
            blocks: HashMap::new(),
            by_id: Vec::new(),
            next_id: 0,
        }
    }
    
    /// FNV-1a hash of a block (fast, well-distributed)
    pub fn hash_block(block: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for &b in block {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }
    
    /// Insert a block, return its ID. Reuses existing if hash matches.
    /// Maximum unique blocks. Beyond this, intern returns 0 (meaning K-frames
    /// can't represent this block). Keeps trailer manageable.
    /// Max unique blocks. Sized to fit ~3 frames worth at 2K (14400/frame)
    pub const MAX_BLOCKS: u32 = 65536;
    
    pub fn intern(&mut self, block: Vec<u8>) -> u32 {
        let hash = Self::hash_block(&block);
        if let Some((id, _, count)) = self.blocks.get_mut(&hash) {
            *count += 1;
            return *id;
        }
        // Dict full — caller will get id=0 (default block); K-frame should bail
        if self.next_id >= Self::MAX_BLOCKS {
            return 0;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.by_id.push(block.clone());
        self.blocks.insert(hash, (id, block, 1));
        id
    }
    
    /// Returns true if dict is at capacity (caller should bail on K-frame)
    pub fn is_full(&self) -> bool {
        self.next_id >= Self::MAX_BLOCKS
    }
    
    /// Look up a block by ID (for decoding)
    pub fn get(&self, id: u32) -> Option<&[u8]> {
        self.by_id.get(id as usize).map(|v| v.as_slice())
    }
    
    /// Stats
    pub fn unique_blocks(&self) -> usize { self.by_id.len() }
    pub fn total_refs(&self) -> u64 {
        self.blocks.values().map(|(_, _, c)| *c as u64).sum()
    }
    pub fn dedup_ratio(&self) -> f32 {
        let unique = self.unique_blocks() as f32;
        let total = self.total_refs() as f32;
        if total > 0.0 { total / unique.max(1.0) } else { 0.0 }
    }
    
    /// Serialize the entire dictionary to bytes (zstd-compressed)
    /// Format: u32 n_blocks | for each block: u16 size | bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut raw = Vec::with_capacity(self.by_id.len() * 256);
        raw.extend_from_slice(&(self.by_id.len() as u32).to_le_bytes());
        for block in &self.by_id {
            raw.extend_from_slice(&(block.len() as u16).to_le_bytes());
            raw.extend_from_slice(block);
        }
        zstd::encode_all(Cursor::new(&raw), 12).unwrap_or(raw)
    }
    
    /// Deserialize a dictionary from compressed bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        let raw = zstd::decode_all(Cursor::new(data)).ok()?;
        if raw.len() < 4 { return None; }
        let n = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
        let mut by_id = Vec::with_capacity(n);
        let mut pos = 4;
        for _ in 0..n {
            if pos + 2 > raw.len() { return None; }
            let sz = u16::from_le_bytes([raw[pos], raw[pos+1]]) as usize;
            pos += 2;
            if pos + sz > raw.len() { return None; }
            by_id.push(raw[pos..pos+sz].to_vec());
            pos += sz;
        }
        // Rebuild hash map
        let mut blocks = HashMap::new();
        for (i, block) in by_id.iter().enumerate() {
            let hash = Self::hash_block(block);
            blocks.insert(hash, (i as u32, block.clone(), 1));
        }
        Some(Self {
            blocks,
            next_id: by_id.len() as u32,
            by_id,
        })
    }
}

impl Default for BlockDictionary {
    fn default() -> Self { Self::new() }
}

/// Encode a frame as a list of block IDs from the dictionary
/// Format 'K' (block-keyed):
///   'K' | w(2) | h(2) | block_size(1=16) | n_blocks(2) | zstd(block_ids u32 each)
///   + uv_compressed_size(4) | uv_compressed (chroma copied as-is)
pub fn encode_frame_from_dict(
    yuv: &[u8],
    w: usize, h: usize,
    dict: &mut BlockDictionary,
) -> Option<Vec<u8>> {
    // Bail if dict is at capacity — K-frame would be lossy beyond this point
    if dict.is_full() { return None; }
    
    let y_sz = w * h;
    let uv_sz = y_sz / 2;
    if yuv.len() < y_sz + uv_sz { return None; }
    
    let block_size = 16usize;
    let bw = w / block_size;
    let bh = h / block_size;
    let n_blocks = bw * bh;
    
    // Extract each 16x16 Y block, intern in dictionary, get ID
    let mut block_ids = Vec::with_capacity(n_blocks);
    for by in 0..bh {
        for bx in 0..bw {
            let mut block = Vec::with_capacity(block_size * block_size);
            for py in 0..block_size {
                for px in 0..block_size {
                    let x = bx * block_size + px;
                    let y = by * block_size + py;
                    let idx = if x < w && y < h { y * w + x } else { 0 };
                    block.push(yuv[idx]);
                }
            }
            block_ids.push(dict.intern(block));
        }
    }
    
    // Bit-pack block_ids (variable bits per ID based on max ID value)
    // For typical game footage with 1000-5000 unique blocks: 10-13 bits per ID
    // vs 32 bits raw → 2.5-3.2x smaller
    let id_packed = crate::bitpack::pack_block_ids(&block_ids);
    
    // Then zstd-compress the bit-packed (catches block ID patterns/repeats)
    let id_compressed = zstd::encode_all(Cursor::new(&id_packed), 5).unwrap_or(id_packed);
    
    // Compress UV planes
    let uv = &yuv[y_sz..y_sz + uv_sz];
    let uv_compressed = zstd::encode_all(Cursor::new(uv), 5).unwrap_or(uv.to_vec());
    
    let mut out = Vec::with_capacity(14 + id_compressed.len() + uv_compressed.len());
    out.push(b'K');
    out.extend_from_slice(&(w as u16).to_le_bytes());
    out.extend_from_slice(&(h as u16).to_le_bytes());
    out.push(block_size as u8);
    out.extend_from_slice(&(n_blocks as u32).to_le_bytes());
    out.extend_from_slice(&(id_compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&id_compressed);
    out.extend_from_slice(&(uv_compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&uv_compressed);
    
    Some(out)
}

/// Decode a 'K' frame using the dictionary
pub fn decode_frame_from_dict(
    data: &[u8],
    dict: &BlockDictionary,
) -> Option<(Vec<u8>, usize, usize)> {
    if data.is_empty() || data[0] != b'K' { return None; }
    if data.len() < 14 { return None; }
    
    let w = u16::from_le_bytes([data[1], data[2]]) as usize;
    let h = u16::from_le_bytes([data[3], data[4]]) as usize;
    let block_size = data[5] as usize;
    let n_blocks = u32::from_le_bytes([data[6], data[7], data[8], data[9]]) as usize;
    let id_size = u32::from_le_bytes([data[10], data[11], data[12], data[13]]) as usize;
    
    let mut pos = 14;
    if pos + id_size > data.len() { return None; }
    let id_compressed = &data[pos..pos + id_size];
    pos += id_size;
    
    if pos + 4 > data.len() { return None; }
    let uv_size = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
    pos += 4;
    if pos + uv_size > data.len() { return None; }
    let uv_compressed = &data[pos..pos + uv_size];
    
    // Decompress block IDs (bit-packed format)
    let id_bytes = zstd::decode_all(Cursor::new(id_compressed)).ok()?;
    let block_ids = crate::bitpack::unpack_block_ids(&id_bytes)?;
    if block_ids.len() < n_blocks { return None; }
    
    // Reconstruct Y plane from block IDs
    let bw = w / block_size;
    let bh = h / block_size;
    let mut y_plane = vec![0u8; w * h];
    for bi in 0..n_blocks {
        let id = block_ids[bi];
        let block = match dict.get(id) {
            Some(b) => b,
            None => continue,  // Missing block — fill with zeros
        };
        let by = bi / bw;
        let bx = bi % bw;
        for py in 0..block_size {
            for px in 0..block_size {
                let x = bx * block_size + px;
                let y = by * block_size + py;
                if x < w && y < h {
                    y_plane[y * w + x] = block[py * block_size + px];
                }
            }
        }
    }
    
    // Decompress UV
    let uv = zstd::decode_all(Cursor::new(uv_compressed)).ok()?;
    
    let mut yuv = Vec::with_capacity(w * h + uv.len());
    yuv.extend_from_slice(&y_plane);
    yuv.extend_from_slice(&uv);
    
    Some((yuv, w, h))
}
