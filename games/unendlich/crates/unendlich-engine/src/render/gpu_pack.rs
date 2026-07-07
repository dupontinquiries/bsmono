//! render::gpu_pack — r340-sl-gpu: shared CPU-side packing primitives for
//! GPU-bound data.
//!
//! This is the **pure, deterministic layout math** that the GPU buffer pools,
//! quantizers, and mesh packers currently re-derive in scattered, ad-hoc forms.
//! It is intentionally Vulkan-free so it is self-contained and `rustc --test`-
//! able. The *Vulkan* side of the consolidation — host-visible buffer creation
//! + persistent map + upload + memory-type selection, today copy-pasted across
//! ~16 render modules as `host_buf`/`create_buffer`+`map_memory` — is a SEPARATE
//! proposed extraction (`render::gpu_buffer`) kept out of this file precisely so
//! this stays testable; see SHAREDLIB_REPORT-r340-sl-gpu.md.
//!
//! Everything here is lossless: each pack has an exact inverse, proven by
//! round-trip tests. No field is flattened, no precision is changed; these
//! operate on already-encoded `u32` words / integer fields, never on a physics
//! field's float samples.
//!
//! Pure & deterministic; no clock/rng/unwrap/IO. Self-contained → `rustc --test`.
#![allow(dead_code)]

// ===========================================================================
// SlotPool — fixed-capacity slot recycler with a high-water mark.
//
// Consolidates the `free_list: VecDeque<u32>` + `high_water` + `capacity`
// pattern (e.g. `chunk_indirect::IndirectDrawSet`). Pure index bookkeeping only;
// the GPU buffer that the slots index into is the caller's concern. The
// high-water mark lets a culling/compaction pass bound its iteration to
// `0..high_water()` instead of the full capacity.
// ===========================================================================

use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct SlotPool {
    capacity: u32,
    free: VecDeque<u32>,
    high_water: u32,
}

impl SlotPool {
    /// A pool of `capacity` slots, all initially free (handed out in ascending
    /// order so allocation is deterministic).
    pub fn new(capacity: u32) -> Self {
        let mut free = VecDeque::with_capacity(capacity as usize);
        for i in 0..capacity {
            free.push_back(i);
        }
        Self { capacity, free, high_water: 0 }
    }

    pub fn capacity(&self) -> u32 { self.capacity }
    pub fn in_use(&self) -> u32 { self.capacity - self.free.len() as u32 }
    pub fn is_empty(&self) -> bool { self.in_use() == 0 }
    /// One past the highest slot index ever allocated this session. Never
    /// shrinks on `free` (matches the existing `chunk_indirect` semantics: a
    /// freed high slot is reused later, not compacted away mid-session).
    pub fn high_water(&self) -> u32 { self.high_water }

    /// Take a free slot, or `None` if the pool is exhausted.
    pub fn alloc(&mut self) -> Option<u32> {
        let idx = self.free.pop_front()?;
        if idx + 1 > self.high_water {
            self.high_water = idx + 1;
        }
        Some(idx)
    }

    /// Return a slot to the pool. Returns `false` if `slot` is out of range or
    /// already free (double-free guard); `true` on success.
    pub fn free_slot(&mut self, slot: u32) -> bool {
        if slot >= self.capacity || self.free.contains(&slot) {
            return false;
        }
        self.free.push_back(slot);
        true
    }
}

// ===========================================================================
// Tile re-packing (lifted from r336 `q8::pack_tiles`; the canonical home is
// here so q8 keeps quantization and gpu_pack owns layout). Re-lays a row-major
// `[n, k]` word grid (`kw = k/4` words wide) into tile-major order for GEMM /
// mesh tile locality. Exact inverse; pure permutation when evenly tiled.
// `tk % 4 == 0` (a tile column maps to whole 4-int8 words).
// ===========================================================================

pub fn packed_tiles_len(n: usize, k: usize, tn: usize, tk: usize) -> usize {
    let kw = k / 4;
    let twk = tk / 4;
    let tiles_r = (n + tn - 1) / tn;
    let tiles_c = (kw + twk - 1) / twk;
    tiles_r * tn * tiles_c * twk
}

pub fn pack_tiles(words: &[u32], n: usize, k: usize, tn: usize, tk: usize) -> Vec<u32> {
    assert_eq!(k % 4, 0, "k must be a multiple of 4");
    assert_eq!(tk % 4, 0, "tk must be a multiple of 4");
    assert!(tn >= 1 && tk >= 4, "tile must be at least 1 x 4");
    assert_eq!(words.len(), n * k / 4, "words length must be n*k/4");
    let kw = k / 4;
    let twk = tk / 4;
    let tiles_c = (kw + twk - 1) / twk;
    let tiles_r = (n + tn - 1) / tn;
    let mut out = vec![0u32; packed_tiles_len(n, k, tn, tk)];
    let mut dst = 0usize;
    for tr in 0..tiles_r {
        for tc in 0..tiles_c {
            for lr in 0..tn {
                let gr = tr * tn + lr;
                for lc in 0..twk {
                    let gc = tc * twk + lc;
                    out[dst] = if gr < n && gc < kw { words[gr * kw + gc] } else { 0 };
                    dst += 1;
                }
            }
        }
    }
    out
}

pub fn unpack_tiles(packed: &[u32], n: usize, k: usize, tn: usize, tk: usize) -> Vec<u32> {
    assert_eq!(k % 4, 0, "k must be a multiple of 4");
    assert_eq!(tk % 4, 0, "tk must be a multiple of 4");
    assert!(tn >= 1 && tk >= 4, "tile must be at least 1 x 4");
    assert_eq!(packed.len(), packed_tiles_len(n, k, tn, tk), "packed length mismatch");
    let kw = k / 4;
    let twk = tk / 4;
    let tiles_c = (kw + twk - 1) / twk;
    let row_words = tn * twk;
    let mut out = vec![0u32; n * kw];
    for gr in 0..n {
        let tr = gr / tn;
        let lr = gr % tn;
        for gc in 0..kw {
            let tc = gc / twk;
            let lc = gc % twk;
            let tile_index = tr * tiles_c + tc;
            let idx = tile_index * row_words + lr * twk + lc;
            out[gr * kw + gc] = packed[idx];
        }
    }
    out
}

// ===========================================================================
// Bit-packing — PERF BONUS (b): tighter GPU packing, provably lossless.
//
// Packs a slice of unsigned integer fields, each `bits` wide (1..=32), into a
// dense little-endian bit stream of `u32` words. This is the primitive that
// octree-node compression (child masks + small child indices) and sparse-mesh
// word packing both need: e.g. eight 6-bit fields cost 48 bits = 1.5 words
// instead of 8 words at one-field-per-word. Layout: value `i` occupies bit
// range `[i*bits, (i+1)*bits)` of the stream, LSB-first, spilling across word
// boundaries as needed; word `w` holds stream bits `[w*32, w*32+32)` in its
// natural little-endian bit order. Exact inverse: `unpack_bits`.
//
// Lossless contract: each value must fit in `bits` (`value < 2^bits`). Out-of-
// range high bits are masked off (documented), so callers that pass in-range
// values get a perfect round-trip; `fits` / `max_value` help assert that.
// ===========================================================================

/// Largest value representable in `bits` (1..=32). `max_value(32) == u32::MAX`.
pub fn max_value(bits: u32) -> u32 {
    assert!((1..=32).contains(&bits), "bits must be 1..=32");
    if bits == 32 { u32::MAX } else { (1u32 << bits) - 1 }
}

/// True iff every value fits in `bits`.
pub fn fits(values: &[u32], bits: u32) -> bool {
    let m = max_value(bits);
    values.iter().all(|&v| v <= m)
}

/// Number of `u32` words `pack_bits` produces for `count` fields of `bits`.
pub fn packed_bits_len(count: usize, bits: u32) -> usize {
    assert!((1..=32).contains(&bits), "bits must be 1..=32");
    let total_bits = count * bits as usize;
    (total_bits + 31) / 32
}

/// Pack `values` (each `bits` wide) into a dense LE bit stream. High bits beyond
/// `bits` are masked off; pass in-range values (see `fits`) for a lossless
/// round-trip.
pub fn pack_bits(values: &[u32], bits: u32) -> Vec<u32> {
    assert!((1..=32).contains(&bits), "bits must be 1..=32");
    let mask = max_value(bits);
    let mut out = vec![0u32; packed_bits_len(values.len(), bits)];
    let mut bitpos = 0usize;
    for &v in values {
        let v = v & mask;
        let w = bitpos / 32;
        let off = (bitpos % 32) as u32;
        // low part into word w
        out[w] |= v.checked_shl(off).unwrap_or(0);
        // spill into word w+1 if the field crosses the boundary
        let low_bits = 32 - off;
        if (bits > low_bits) && (w + 1 < out.len()) {
            out[w + 1] |= v >> low_bits;
        }
        bitpos += bits as usize;
    }
    out
}

/// Inverse of `pack_bits`: recover `count` fields of `bits` from the stream.
pub fn unpack_bits(packed: &[u32], bits: u32, count: usize) -> Vec<u32> {
    assert!((1..=32).contains(&bits), "bits must be 1..=32");
    assert_eq!(packed.len(), packed_bits_len(count, bits), "packed length mismatch");
    let mask = max_value(bits);
    let mut out = vec![0u32; count];
    let mut bitpos = 0usize;
    for slot in out.iter_mut() {
        let w = bitpos / 32;
        let off = (bitpos % 32) as u32;
        let low = packed[w] >> off;
        let low_bits = 32 - off;
        let v = if (bits > low_bits) && (w + 1 < packed.len()) {
            low | (packed[w + 1] << low_bits)
        } else {
            low
        };
        *slot = v & mask;
        bitpos += bits as usize;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny deterministic LCG (no rng/clock dep) for fuzzing.
    fn lcg(state: &mut u64) -> u32 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*state >> 32) as u32
    }

    // ---- SlotPool ----------------------------------------------------------

    #[test]
    fn slotpool_alloc_free_reuse_and_exhaustion() {
        let mut p = SlotPool::new(3);
        assert_eq!(p.capacity(), 3);
        let a = p.alloc().unwrap();
        let b = p.alloc().unwrap();
        let c = p.alloc().unwrap();
        assert_eq!((a, b, c), (0, 1, 2));
        assert_eq!(p.in_use(), 3);
        assert!(p.alloc().is_none(), "exhausted pool yields None");
        assert!(p.free_slot(b));
        assert!(!p.free_slot(b), "double free is rejected");
        assert!(!p.free_slot(99), "out-of-range free is rejected");
        assert_eq!(p.alloc(), Some(1), "freed slot is reused");
        assert_eq!(p.in_use(), 3);
    }

    #[test]
    fn slotpool_high_water_is_monotone_and_bounded() {
        let mut p = SlotPool::new(4);
        let s: Vec<u32> = (0..4).map(|_| p.alloc().unwrap()).collect();
        assert_eq!(p.high_water(), 4);
        for x in s { assert!(p.free_slot(x)); }
        // Freeing does NOT lower high_water (matches chunk_indirect semantics).
        assert_eq!(p.high_water(), 4);
        let _ = p.alloc();
        assert!(p.high_water() <= p.capacity());
    }

    #[test]
    fn slotpool_deterministic() {
        let mut p = SlotPool::new(5);
        let seq: Vec<Option<u32>> = (0..7).map(|_| p.alloc()).collect();
        let mut q = SlotPool::new(5);
        let seq2: Vec<Option<u32>> = (0..7).map(|_| q.alloc()).collect();
        assert_eq!(seq, seq2);
    }

    // ---- tile packing ------------------------------------------------------

    #[test]
    fn tile_pack_unpack_round_trips() {
        let mut st = 0x0f0f_1234_5678_9abcu64;
        for &(n, k) in &[(8usize, 64usize), (5, 96usize), (1, 32usize), (7, 32usize)] {
            let words: Vec<u32> = (0..n * k / 4).map(|_| lcg(&mut st)).collect();
            for &(tn, tk) in &[(2usize, 16usize), (4, 32usize), (3, 8usize), (1, 4usize), (n, k)] {
                let packed = pack_tiles(&words, n, k, tn, tk);
                assert_eq!(packed.len(), packed_tiles_len(n, k, tn, tk));
                assert_eq!(unpack_tiles(&packed, n, k, tn, tk), words,
                    "round-trip n={n} k={k} tn={tn} tk={tk}");
            }
        }
    }

    #[test]
    fn tile_pack_is_permutation_when_evenly_tiled() {
        let (n, k) = (8usize, 64usize);
        let mut st = 0x9999_8888_7777_6666u64;
        let words: Vec<u32> = (0..n * k / 4).map(|_| lcg(&mut st)).collect();
        let packed = pack_tiles(&words, n, k, 4, 32);
        assert_eq!(packed.len(), words.len(), "even tiling adds no padding");
        let (mut a, mut b) = (words.clone(), packed.clone());
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b, "packed is a reordering of the same words");
    }

    // ---- bit packing (perf bonus, lossless) --------------------------------

    #[test]
    fn bit_pack_round_trips_all_widths() {
        let mut st = 0xdead_beef_cafe_f00du64;
        for bits in 1u32..=32 {
            let count = 37usize; // deliberately crosses many word boundaries
            let m = max_value(bits);
            let vals: Vec<u32> = (0..count).map(|_| lcg(&mut st) & m).collect();
            assert!(fits(&vals, bits));
            let packed = pack_bits(&vals, bits);
            assert_eq!(packed.len(), packed_bits_len(count, bits));
            assert_eq!(unpack_bits(&packed, bits, count), vals, "round-trip at bits={bits}");
        }
    }

    #[test]
    fn bit_pack_is_dense_and_handles_boundaries() {
        // Eight 6-bit fields = 48 bits = 2 words (not 8). Density win.
        let vals = [63u32, 0, 1, 62, 7, 8, 63, 0];
        assert_eq!(packed_bits_len(vals.len(), 6), 2);
        let packed = pack_bits(&vals, 6);
        assert_eq!(packed.len(), 2);
        assert_eq!(unpack_bits(&packed, 6, vals.len()), vals);
        // A value that straddles the 32-bit word seam (field 5 at bit 30..36).
        let many = [max_value(6); 6]; // bits 0..36 → spans word 0 and 1
        let p2 = pack_bits(&many, 6);
        assert_eq!(unpack_bits(&p2, 6, many.len()), many);
        // All-zero packs to all-zero words and round-trips.
        let z = [0u32; 10];
        assert_eq!(pack_bits(&z, 5), vec![0u32; packed_bits_len(10, 5)]);
        assert_eq!(unpack_bits(&pack_bits(&z, 5), 5, 10), z);
    }

    #[test]
    fn bit_pack_masks_out_of_range_and_is_deterministic() {
        // High bits beyond `bits` are dropped (documented lossy-on-overflow).
        let over = [0xFFFF_FFFFu32, 0x100, 0x3F];
        let packed = pack_bits(&over, 6);
        let got = unpack_bits(&packed, 6, over.len());
        assert_eq!(got, [0x3F, 0x00, 0x3F], "values masked to 6 bits");
        // Determinism.
        assert_eq!(pack_bits(&over, 6), pack_bits(&over, 6));
        // 32-bit fields are an identity copy.
        let full = [1u32, u32::MAX, 0, 12345];
        assert_eq!(pack_bits(&full, 32), full.to_vec());
        assert_eq!(unpack_bits(&full, 32, 4), full);
    }
}
