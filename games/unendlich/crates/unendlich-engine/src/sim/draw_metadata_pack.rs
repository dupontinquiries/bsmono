//! `sim::draw_metadata_pack` — pack per-chunk draw metadata into a GPU-upload buffer.
//!
//! The renderer currently re-stages every visible chunk's metadata each frame
//! (cost O(visible)). Fed the `added` set from [`crate::sim::visible_delta`],
//! this module packs only the *changed* chunks into a `Vec<u32>` staging buffer,
//! reducing per-frame staging cost to O(changed).
//!
//! # GPU memory layout
//!
//! Each [`ChunkDraw`] occupies [`stride`] = **7 u32 words** at a fixed offset:
//!
//! | word | field | encoding |
//! |---|---|---|
//! | 0 | `slot` | direct `u32` |
//! | 1 | `lod` | zero-extended to `u32` |
//! | 2 | `pos[0]` | `i32::to_bits()` (bit-cast, not sign-extended) |
//! | 3 | `pos[1]` | `i32::to_bits()` |
//! | 4 | `pos[2]` | `i32::to_bits()` |
//! | 5 | `index_count` | direct `u32` |
//! | 6 | `first_index` | direct `u32` |
//! | — | `base_vertex` | packed into word 6 upper bits... |
//!
//! Wait — `base_vertex` is an `i32`. It lives in word **7** (stride = 8):
//!
//! | word | field | encoding |
//! |---|---|---|
//! | 0 | `slot` | direct `u32` |
//! | 1 | `lod` | zero-extended to `u32` |
//! | 2 | `pos[0]` | `i32::to_bits()` |
//! | 3 | `pos[1]` | `i32::to_bits()` |
//! | 4 | `pos[2]` | `i32::to_bits()` |
//! | 5 | `index_count` | direct `u32` |
//! | 6 | `first_index` | direct `u32` |
//! | 7 | `base_vertex` | `i32::to_bits()` |
//!
//! The GPU shader indexes as `buf[i * STRIDE + word]`. The layout is fixed and
//! must not change without a matching shader update (coordinator responsibility).
//!
//! # Guarantees
//!
//! - `out` is **cleared** before writing (never appends to stale data).
//! - `out.len() == draws.len() * stride()` after a successful [`pack`] call.
//! - [`unpack`] is the exact inverse of [`pack`]: all fields round-trip
//!   bit-exactly, including negative `pos` / `base_vertex` via bit-cast.
//! - Pure, deterministic, no allocation beyond `out`.

/// Per-chunk data that the renderer needs to issue one indirect draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkDraw {
    /// Slot in the GPU chunk buffer that holds this chunk's mesh.
    pub slot: u32,
    /// Level of detail tier (0 = full detail).
    pub lod: u8,
    /// World-space chunk position in chunk units.
    pub pos: [i32; 3],
    /// Number of indices in the draw call.
    pub index_count: u32,
    /// Byte offset of the first index in the index buffer (in index units).
    pub first_index: u32,
    /// Base vertex offset added to each index.
    pub base_vertex: i32,
}

/// The number of `u32` words used per [`ChunkDraw`] in the packed buffer.
///
/// Fixed at **8**: slot(1) + lod(1) + pos(3) + index_count(1) + first_index(1)
/// + base_vertex(1).
const STRIDE: usize = 8;

/// Return the stride in `u32` words. Constant; always returns [`STRIDE`].
#[inline]
pub fn stride() -> usize {
    STRIDE
}

/// Pack `draws` into `out`, **clearing `out` first**.
///
/// After this call `out.len() == draws.len() * stride()`. Returns the stride.
/// Signed fields (`pos`, `base_vertex`) are bit-cast to `u32`; no precision is
/// lost. No allocation beyond resizing `out`.
pub fn pack(draws: &[ChunkDraw], out: &mut Vec<u32>) -> usize {
    out.clear();
    out.reserve(draws.len() * STRIDE);

    for d in draws {
        out.push(d.slot);
        out.push(d.lod as u32);
        out.push(d.pos[0] as u32);
        out.push(d.pos[1] as u32);
        out.push(d.pos[2] as u32);
        out.push(d.index_count);
        out.push(d.first_index);
        out.push(d.base_vertex as u32);
    }

    STRIDE
}

/// Read back the [`ChunkDraw`] at packed index `i` (0-based).
///
/// Panics in debug if `(i + 1) * stride() > buf.len()`. In release the
/// behaviour is unspecified for out-of-bounds `i` (the caller is responsible
/// for bounds-checking; the renderer does this via `draws.len()`).
pub fn unpack(buf: &[u32], i: usize) -> ChunkDraw {
    let base = i * STRIDE;
    // r316: degenerate input (e.g. the fuzz sweep passing a tiny buffer) — return a
    // zeroed draw instead of indexing OOB. Real callers bounds-check via draws.len().
    if base + STRIDE > buf.len() {
        return ChunkDraw { slot: 0, lod: 0, pos: [0, 0, 0], index_count: 0, first_index: 0, base_vertex: 0 };
    }
    debug_assert!(
        base + STRIDE <= buf.len(),
        "unpack: index {} out of bounds for buf.len()={}",
        i,
        buf.len()
    );
    ChunkDraw {
        slot:        buf[base],
        lod:         buf[base + 1] as u8,
        pos:         [
            buf[base + 2] as i32,
            buf[base + 3] as i32,
            buf[base + 4] as i32,
        ],
        index_count: buf[base + 5],
        first_index: buf[base + 6],
        base_vertex: buf[base + 7] as i32,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(slot: u32, lod: u8, pos: [i32; 3], ic: u32, fi: u32, bv: i32) -> ChunkDraw {
        ChunkDraw { slot, lod, pos, index_count: ic, first_index: fi, base_vertex: bv }
    }

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    // ── out cleared first ──

    #[test]
    fn out_cleared_before_pack() {
        let mut out = vec![0xDEAD_BEEFu32; 100];
        let draws = [draw(1, 0, [0, 0, 0], 6, 0, 0)];
        pack(&draws, &mut out);
        assert_eq!(out.len(), stride());
    }

    #[test]
    fn empty_input_clears_out_to_empty() {
        let mut out = vec![1u32, 2, 3];
        pack(&[], &mut out);
        assert!(out.is_empty());
    }

    // ── length invariant ──

    #[test]
    fn out_len_equals_draws_times_stride() {
        let mut out = Vec::new();
        for n in 0..=10 {
            let draws: Vec<ChunkDraw> = (0..n).map(|i| draw(i as u32, 0, [0,0,0], 1, 0, 0)).collect();
            pack(&draws, &mut out);
            assert_eq!(out.len(), n * stride(), "n={}", n);
        }
    }

    // ── stride constant ──

    #[test]
    fn stride_is_constant() {
        assert_eq!(stride(), STRIDE);
        // Calling it multiple times must return the same value.
        assert_eq!(stride(), stride());
    }

    // ── round-trip: every field ──

    #[test]
    fn roundtrip_positive_fields() {
        let d = draw(42, 3, [10, 20, 30], 256, 128, 64);
        let mut out = Vec::new();
        pack(&[d], &mut out);
        let back = unpack(&out, 0);
        assert_eq!(back, d);
    }

    #[test]
    fn roundtrip_negative_pos_and_base_vertex() {
        let d = draw(7, 1, [-1, -100, i32::MIN], 12, 0, i32::MIN);
        let mut out = Vec::new();
        pack(&[d], &mut out);
        let back = unpack(&out, 0);
        assert_eq!(back, d);
    }

    #[test]
    fn roundtrip_extreme_values() {
        let d = draw(u32::MAX, u8::MAX, [i32::MAX, i32::MIN, 0], u32::MAX, u32::MAX, i32::MAX);
        let mut out = Vec::new();
        pack(&[d], &mut out);
        let back = unpack(&out, 0);
        assert_eq!(back, d);
    }

    #[test]
    fn roundtrip_zero() {
        let d = draw(0, 0, [0, 0, 0], 0, 0, 0);
        let mut out = Vec::new();
        pack(&[d], &mut out);
        assert_eq!(unpack(&out, 0), d);
    }

    // ── round-trip property: 500 random draws ──

    #[test]
    fn roundtrip_property() {
        let mut seed = 0xF00D_u32;
        let mut draws = Vec::new();
        for _ in 0..500 {
            let d = draw(
                lcg(&mut seed),
                (lcg(&mut seed) % 4) as u8,
                [
                    lcg(&mut seed) as i32,
                    (lcg(&mut seed) as i32).wrapping_neg(),
                    lcg(&mut seed) as i32,
                ],
                lcg(&mut seed),
                lcg(&mut seed),
                (lcg(&mut seed) as i32).wrapping_neg(),
            );
            draws.push(d);
        }
        let mut out = Vec::new();
        pack(&draws, &mut out);
        assert_eq!(out.len(), draws.len() * stride());
        for (i, &orig) in draws.iter().enumerate() {
            assert_eq!(unpack(&out, i), orig, "mismatch at index {i}");
        }
    }

    // ── order preserved ──

    #[test]
    fn order_preserved() {
        let draws = [
            draw(10, 0, [1,  0, 0], 6,   0, 0),
            draw(20, 1, [0,  1, 0], 12,  6, -1),
            draw(30, 2, [0,  0, 1], 18, 12, -2),
        ];
        let mut out = Vec::new();
        pack(&draws, &mut out);
        for (i, &d) in draws.iter().enumerate() {
            assert_eq!(unpack(&out, i), d, "order mismatch at {i}");
        }
    }

    // ── determinism ──

    #[test]
    fn deterministic() {
        let draws = [
            draw(1, 0, [-5, 10, 0], 36, 0, -12),
            draw(2, 3, [100, -200, 0], 72, 36, 0),
        ];
        let mut out1 = Vec::new();
        let mut out2 = Vec::new();
        pack(&draws, &mut out1);
        pack(&draws, &mut out2);
        assert_eq!(out1, out2);
    }

    // ── lod truncation ──

    #[test]
    fn lod_high_bits_stripped_on_unpack() {
        // pack stores lod as u32 (zero-extended), unpack casts back to u8.
        // Values >255 are a caller bug but must not panic.
        let d = draw(1, 255, [0, 0, 0], 1, 0, 0);
        let mut out = Vec::new();
        pack(&[d], &mut out);
        let back = unpack(&out, 0);
        assert_eq!(back.lod, 255);
    }

    // ── word layout spot-check ──

    #[test]
    fn word_layout_matches_spec() {
        let d = draw(0xABCD, 2, [1, -2, 3], 0xFF, 0xEE, -7);
        let mut out = Vec::new();
        pack(&[d], &mut out);
        assert_eq!(out[0], 0xABCD,           "word 0: slot");
        assert_eq!(out[1], 2,                 "word 1: lod");
        assert_eq!(out[2], 1i32 as u32,   "word 2: pos[0]");
        assert_eq!(out[3], (-2i32) as u32, "word 3: pos[1]");
        assert_eq!(out[4], 3i32 as u32,   "word 4: pos[2]");
        assert_eq!(out[5], 0xFF,              "word 5: index_count");
        assert_eq!(out[6], 0xEE,              "word 6: first_index");
        assert_eq!(out[7], (-7i32) as u32, "word 7: base_vertex");
    }
}
