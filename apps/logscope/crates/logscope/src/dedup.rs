//! dedup.rs — cross-model file compression for the `~/models` folder via content-defined chunking (CDC)
//! and content-addressed dedup.
//!
//! GGUF files in a models folder overlap heavily: quants of one base share vocab/embedding regions,
//! finetunes share most of the base weights, and re-exports differ only in metadata. Fixed-size blocks
//! miss this because one inserted byte shifts every later block; **content-defined** boundaries (cut where
//! a rolling hash hits a pattern) keep identical regions aligned across files, so a [`Store`] keeps each
//! unique chunk once and represents every model as a manifest of chunk hashes.
//!
//! Pure + unit-tested. The seam is just file I/O (read each model's bytes, write the chunk store); the
//! chunking and dedup accounting are here. Chunk *identity* uses a fast 64-bit hash — fine for accounting
//! and tests; a real store should use a cryptographic digest (BLAKE3/SHA-256) for collision safety.
#![allow(dead_code)]

use std::collections::HashMap;

/// Gear table for the rolling hash, generated deterministically at compile time.
const GEAR: [u64; 256] = {
    let mut t = [0u64; 256];
    let mut s: u64 = 0x2545_F491_4F6C_DD1D; // splitmix64 seed
    let mut i = 0;
    while i < 256 {
        s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = s;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        t[i] = z;
        i += 1;
    }
    t
};

/// Chunking parameters. Cut when the low `mask_bits` of the rolling hash are zero (average chunk ≈
/// `2^mask_bits` bytes), clamped to `[min, max]`.
#[derive(Clone, Copy, Debug)]
pub struct ChunkCfg { pub min: usize, pub max: usize, pub mask_bits: u32 }
impl Default for ChunkCfg {
    fn default() -> Self { Self { min: 2048, max: 16384, mask_bits: 12 } } // ~4 KiB average
}

/// A chunk's position and content hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chunk { pub offset: usize, pub len: usize, pub hash: u64 }

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes { h ^= b as u64; h = h.wrapping_mul(0x0000_0100_0000_01B3); }
    h
}

/// Split `data` into content-defined chunks. Boundaries fall where the Gear rolling hash hits the mask,
/// subject to `min`/`max`; identical regions in different files produce identical chunks (and hashes).
pub fn chunk(data: &[u8], cfg: &ChunkCfg) -> Vec<Chunk> {
    let mut out = Vec::new();
    if data.is_empty() { return out; }
    let mask = (1u64 << cfg.mask_bits.min(63)) - 1;
    let (min, max) = (cfg.min.max(1), cfg.max.max(cfg.min.max(1)));
    let mut start = 0usize;
    let mut h = 0u64;
    let mut i = 0usize;
    while i < data.len() {
        h = (h << 1).wrapping_add(GEAR[data[i] as usize]);
        let len = i - start + 1;
        let boundary = len >= min && (h & mask) == 0;
        if boundary || len >= max {
            out.push(Chunk { offset: start, len, hash: fnv1a(&data[start..start + len]) });
            start = i + 1;
            h = 0;
        }
        i += 1;
    }
    if start < data.len() {
        out.push(Chunk { offset: start, len: data.len() - start, hash: fnv1a(&data[start..]) });
    }
    out
}

/// One model's representation in the store: an ordered list of chunk hashes (its "recipe").
#[derive(Clone, Debug)]
pub struct Manifest { pub name: String, pub chunks: Vec<u64> }

/// A content-addressed chunk store with dedup accounting.
#[derive(Default)]
pub struct Store {
    sizes: HashMap<u64, usize>, // unique chunk hash → its byte length (stored once)
    logical: u64,               // total bytes across all added files
}

impl Store {
    pub fn new() -> Self { Self::default() }

    /// Add a model's bytes; returns its manifest. New chunks grow the store; chunks already present are
    /// free (the dedup win).
    pub fn add(&mut self, name: &str, data: &[u8], cfg: &ChunkCfg) -> Manifest {
        let chunks = chunk(data, cfg);
        let mut hashes = Vec::with_capacity(chunks.len());
        for c in &chunks {
            self.sizes.entry(c.hash).or_insert(c.len);
            self.logical += c.len as u64;
            hashes.push(c.hash);
        }
        Manifest { name: name.to_string(), chunks: hashes }
    }

    /// Bytes actually stored (sum of unique chunk lengths).
    pub fn stored_bytes(&self) -> u64 { self.sizes.values().map(|&s| s as u64).sum() }
    /// Bytes across all files as if stored independently.
    pub fn logical_bytes(&self) -> u64 { self.logical }
    pub fn unique_chunks(&self) -> usize { self.sizes.len() }
    /// Compression ratio from cross-file dedup (1.0 = no sharing; higher = more shared).
    pub fn dedup_ratio(&self) -> f32 {
        let stored = self.stored_bytes();
        if stored == 0 { 1.0 } else { self.logical as f32 / stored as f32 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // a deterministic pseudo-random blob (stands in for model weights)
    fn blob(seed: u64, n: usize) -> Vec<u8> {
        let mut s = seed;
        (0..n).map(|_| { s = s.wrapping_mul(6364136223846793005).wrapping_add(1); (s >> 33) as u8 }).collect()
    }

    #[test]
    fn chunking_is_deterministic_and_within_bounds() {
        let data = blob(1, 200_000);
        let cfg = ChunkCfg::default();
        let a = chunk(&data, &cfg);
        let b = chunk(&data, &cfg);
        assert_eq!(a, b, "same bytes ⇒ same chunks");
        assert!(a.len() > 1);
        // every chunk respects max; all but the last respect min; offsets/lengths tile the file exactly
        assert!(a.iter().all(|c| c.len <= cfg.max));
        assert!(a[..a.len() - 1].iter().all(|c| c.len >= cfg.min));
        let covered: usize = a.iter().map(|c| c.len).sum();
        assert_eq!(covered, data.len());
        assert_eq!(a[0].offset, 0);
        for w in a.windows(2) { assert_eq!(w[1].offset, w[0].offset + w[0].len); }
    }

    #[test]
    fn identical_files_dedupe_completely() {
        let data = blob(7, 120_000);
        let mut s = Store::new();
        s.add("model-a.gguf", &data, &ChunkCfg::default());
        let stored_after_first = s.stored_bytes();
        s.add("model-a-copy.gguf", &data, &ChunkCfg::default());
        assert_eq!(s.stored_bytes(), stored_after_first, "second identical file adds nothing");
        assert_eq!(s.logical_bytes(), 2 * data.len() as u64);
        assert!((s.dedup_ratio() - 2.0).abs() < 1e-3);
    }

    #[test]
    fn files_sharing_a_base_dedupe_the_shared_region() {
        // a base + finetune: same first 256 KiB of "weights", different tails (like a LoRA-merged model)
        let base = blob(42, 262_144);
        let mut a = base.clone(); a.extend(blob(100, 40_000));
        let mut b = base.clone(); b.extend(blob(200, 40_000));
        let cfg = ChunkCfg::default();
        let mut s = Store::new();
        s.add("base.gguf", &a, &cfg);
        let before = s.stored_bytes();
        s.add("finetune.gguf", &b, &cfg);
        let added = s.stored_bytes() - before;
        // only the differing tail (~40 KiB) plus a boundary chunk should be new — far less than the whole file
        assert!(added < 60_000, "shared base should not be re-stored (added {added} bytes)");
        assert!(s.dedup_ratio() > 1.5);
    }

    #[test]
    fn a_byte_insertion_only_disturbs_local_chunks() {
        // content-defined boundaries should re-sync after an edit, not invalidate the whole file
        let data = blob(9, 150_000);
        let mut edited = data.clone();
        edited.insert(75_000, 0xAB); // single inserted byte mid-file
        let cfg = ChunkCfg::default();
        let mut s = Store::new();
        s.add("orig", &data, &cfg);
        let before = s.stored_bytes();
        s.add("edited", &edited, &cfg);
        let added = (s.stored_bytes() - before) as usize;
        // re-sync means only a few chunks around the edit are new, not ~half the file
        assert!(added < data.len() / 4, "CDC should re-sync (added {added} of {} bytes)", data.len());
    }

    #[test]
    fn empty_input_makes_no_chunks() {
        assert!(chunk(&[], &ChunkCfg::default()).is_empty());
    }
}
