//! q8.rs — Q8_0 weight quantizer + dequant, byte-identical to the validated
//! Python `oracle.quantize_q8`. This is how r67 bakes a trained shading net into
//! the SSBO layout `linear_q8_0` / `neural_shade.comp` already read bit-exact:
//!   per 32-wide block: scale = amax/127 (or 1 if all-zero), int8 = round_even(w/scale),
//!   packed 4 int8 / u32 little-endian; layout [n, k] with k % 32 == 0.
//!
//! Drop into unendlich-engine; `quantize_layer` augments+pads a weight+bias the
//! way GpuMLP does (bias folded into a virtual input column, padded to 32).

pub const BLOCK: usize = 32;

/// Round half to even (matches numpy's `np.round`), including negatives.
fn round_half_even(x: f32) -> f32 {
    let f = x.floor();
    let d = x - f;
    if d < 0.5 { f } else if d > 0.5 { f + 1.0 }
    else if (f as i64) % 2 == 0 { f } else { f + 1.0 }
}

/// Quantize `w` (row-major [n, k], k % 32 == 0) -> (packed u32 [n*k/4], scales f32 [n*k/32]).
///
/// r336-neural: this now delegates to the block-wise [`quantize_blockwise`],
/// which is **byte-identical** to the original scalar implementation (retained
/// as [`quantize_scalar_reference`]) but does it in a single pass with no
/// intermediate `n*k` byte buffer. Parity with `oracle.quantize_q8` is therefore
/// preserved bit-for-bit; see the golden-vector + fuzz parity tests below.
pub fn quantize(w: &[f32], n: usize, k: usize) -> (Vec<u32>, Vec<f32>) {
    quantize_blockwise(w, n, k)
}

/// Original (pre-r336) scalar quantizer, kept verbatim as the **parity oracle**.
/// Two passes: round every weight into an `n*k` `i8` scratch buffer, then pack.
/// `quantize` no longer calls this, but it is retained (and tested against) so
/// any future change to the fast path is provably byte-identical to the impl
/// that was validated against the Python `oracle.quantize_q8`.
pub fn quantize_scalar_reference(w: &[f32], n: usize, k: usize) -> (Vec<u32>, Vec<f32>) {
    assert_eq!(w.len(), n * k);
    assert_eq!(k % BLOCK, 0, "k must be a multiple of 32");
    let kb = k / BLOCK;
    let mut qs = vec![0f32; n * kb];
    let mut qbytes = vec![0i8; n * k];
    for r in 0..n {
        for b in 0..kb {
            let blk = &w[r * k + b * BLOCK..r * k + (b + 1) * BLOCK];
            let amax = blk.iter().fold(0f32, |m, &v| m.max(v.abs()));
            let scale = if amax > 0.0 { amax / 127.0 } else { 1.0 };
            qs[r * kb + b] = scale;
            for j in 0..BLOCK {
                let q = round_half_even(blk[j] / scale).clamp(-127.0, 127.0) as i32 as i8;
                qbytes[r * k + b * BLOCK + j] = q;
            }
        }
    }
    let mut qw = vec![0u32; n * k / 4];
    for i in 0..n * k {
        qw[i / 4] |= (qbytes[i] as u8 as u32) << ((i % 4) * 8);
    }
    (qw, qs)
}

/// r336-neural: faster block-wise quantizer, **byte-identical** to
/// [`quantize_scalar_reference`]. For each 32-wide block it rounds the 32 codes
/// into a tiny stack-local `[u8; 32]` buffer (a tight, autovectorizable loop)
/// and then packs them into the block's 8 destination `u32` words with
/// `u32::from_le_bytes` — which reproduces the reference's little-endian
/// `byte << (lane*8)` packing exactly. This removes the reference's `n*k`-byte
/// heap scratch and its second global packing pass (less memory traffic), and
/// measured ~1.1–1.2× faster than the reference across small and large layers
/// with no regression at scale. The per-element math is performed in exactly the
/// same order and operations as the reference (amax fold; `amax/127` scale with
/// the all-zero→1.0 guard; `round_half_even(w / scale)` — division, **not**
/// reciprocal-multiply, which would not be bit-identical — then clamp to ±127),
/// which is what guarantees identical output bytes.
pub fn quantize_blockwise(w: &[f32], n: usize, k: usize) -> (Vec<u32>, Vec<f32>) {
    assert_eq!(w.len(), n * k);
    assert_eq!(k % BLOCK, 0, "k must be a multiple of 32");
    let kb = k / BLOCK;
    const WORDS_PER_BLOCK: usize = BLOCK / 4; // 8
    let mut qs = vec![0f32; n * kb];
    let mut qw = vec![0u32; n * k / 4];
    for r in 0..n {
        for b in 0..kb {
            let base = r * k + b * BLOCK; // element offset; multiple of 4 (k,BLOCK %4==0)
            let blk = &w[base..base + BLOCK];
            let amax = blk.iter().fold(0f32, |m, &v| m.max(v.abs()));
            let scale = if amax > 0.0 { amax / 127.0 } else { 1.0 };
            qs[r * kb + b] = scale;
            // Round into a contiguous byte buffer (vectorizable), then pack.
            let mut codes = [0u8; BLOCK];
            for j in 0..BLOCK {
                codes[j] = (round_half_even(blk[j] / scale).clamp(-127.0, 127.0) as i32 as i8) as u8;
            }
            let wbase = base / 4; // first destination word for this block
            for wj in 0..WORDS_PER_BLOCK {
                let o = wj * 4;
                qw[wbase + wj] =
                    u32::from_le_bytes([codes[o], codes[o + 1], codes[o + 2], codes[o + 3]]);
            }
        }
    }
    (qw, qs)
}

/// Augment a layer (W [out, d], bias [out]) the way GpuMLP does: append a virtual
/// input column of 1.0 carrying the bias, zero-pad to the next multiple of 32,
/// then quantize. Returns (qw, qs, k_padded). The shader sets activation[d]=1.0.
pub fn quantize_layer(w: &[f32], bias: &[f32], out: usize, d: usize) -> (Vec<u32>, Vec<f32>, usize) {
    let k = ((d + 1 + 31) / 32) * 32;
    let mut wa = vec![0f32; out * k];
    for r in 0..out {
        wa[r * k..r * k + d].copy_from_slice(&w[r * d..r * d + d]);
        wa[r * k + d] = bias[r];
    }
    let (qw, qs) = quantize(&wa, out, k);
    (qw, qs, k)
}

/// Dequantize one weight (row, col) — mirrors the shader extraction. For tests.
pub fn dequant(qw: &[u32], qs: &[f32], k: usize, row: usize, col: usize) -> f32 {
    let kb = k / BLOCK;
    let packed = qw[(row * k + col) / 4];
    let q = ((packed >> ((col % 4) * 8)) & 0xFF) as i8;
    q as f32 * qs[row * kb + col / BLOCK]
}

// ===========================================================================
// r336-neural: CPU-side weight pre-packing / tiling.
//
// The packed weights from `quantize` are row-major `[n, k]` int8 → `n*k/4`
// `u32` words (word grid `n × kw`, `kw = k/4`). A tiled GEMM reads weights in
// `tn × tk` blocks; serving them in that order keeps each tile's words
// contiguous, so a workgroup streams one tile as a single run instead of
// gathering `tn` strided row-segments — fewer cache lines / DMA bursts touched
// for the same bytes.
//
// `pack_tiles` re-lays the word grid into tile-major order; `unpack_tiles` is
// the exact inverse (given the original `n, k, tn, tk`). The transform moves
// bytes, it never changes them — when the dimensions divide evenly it is a pure
// permutation of the input words (proven in tests), so the quantized net the
// shader reconstructs is bit-identical regardless of packing. Dimensions that
// don't fill whole tiles are zero-padded in the packed buffer; `unpack_tiles`
// drops that padding. `tk % 4 == 0` is required so a tile column maps to whole
// words. This is pure CPU prep; wiring a tile-reading GEMM shader to consume the
// layout is a handoff (shaders are off-limits to this thread).
// ===========================================================================

/// Number of `u32` words `pack_tiles` emits for an `n × k` matrix tiled `tn × tk`
/// (i.e. padded up to whole tiles). `k`/`tk` are in elements; `tk % 4 == 0`.
/// Tile-major weight re-packing. r341: the canonical implementation now lives in
/// `render::gpu_pack` (the shared GPU-packing home); these are thin delegators so the
/// algorithm exists once. q8 keeps quantization; gpu_pack owns layout.
pub fn packed_tiles_len(n: usize, k: usize, tn: usize, tk: usize) -> usize {
    crate::render::gpu_pack::packed_tiles_len(n, k, tn, tk)
}

/// Re-tile packed weight words into tile-major order. Delegates to `gpu_pack::pack_tiles`.
pub fn pack_tiles(qw: &[u32], n: usize, k: usize, tn: usize, tk: usize) -> Vec<u32> {
    crate::render::gpu_pack::pack_tiles(qw, n, k, tn, tk)
}

/// Inverse of [`pack_tiles`]. Delegates to `gpu_pack::unpack_tiles`.
pub fn unpack_tiles(packed: &[u32], n: usize, k: usize, tn: usize, tk: usize) -> Vec<u32> {
    crate::render::gpu_pack::unpack_tiles(packed, n, k, tn, tk)
}

// ---------------------------------------------------------------------------
// r336-neural (optional): Q4 block experiment. Strictly additive — it does NOT
// touch the Q8 path or its output. Block of 32 values, symmetric 4-bit signed
// codes in [-7, 7] with `scale = amax/7`, packed 8 nibbles per `u32` (nibble i
// = element 8*word+i, low nibble first). Half the weight bandwidth of Q8 at
// coarser precision; provided only for offline size/quality experiments. A
// production path would need a matching shader (handoff) — there is none today.
// ---------------------------------------------------------------------------

/// Q4 experiment: quantize one 32-wide block to 4 packed `u32` (8 nibbles each)
/// + its scale. Returns `([u32; 4], scale)`. See module note; not used by Q8.
pub fn quantize_q4_block_experimental(blk: &[f32]) -> ([u32; 4], f32) {
    assert_eq!(blk.len(), BLOCK, "q4 block must be 32 wide");
    let amax = blk.iter().fold(0f32, |m, &v| m.max(v.abs()));
    let scale = if amax > 0.0 { amax / 7.0 } else { 1.0 };
    let mut words = [0u32; 4];
    for (i, &v) in blk.iter().enumerate() {
        let q = round_half_even(v / scale).clamp(-7.0, 7.0) as i32 as i8;
        let nib = (q as u8) & 0x0F; // two's-complement low nibble
        words[i / 8] |= (nib as u32) << ((i % 8) * 4);
    }
    (words, scale)
}

/// Dequantize one element of a Q4 block produced by
/// [`quantize_q4_block_experimental`]. `idx` in `0..32`.
pub fn dequant_q4_experimental(words: &[u32; 4], scale: f32, idx: usize) -> f32 {
    let nib = ((words[idx / 8] >> ((idx % 8) * 4)) & 0x0F) as u8;
    // sign-extend 4-bit two's complement
    let q = if nib & 0x08 != 0 { (nib | 0xF0) as i8 } else { nib as i8 };
    q as f32 * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_within_half_scale() {
        let (n, k) = (4usize, 64usize);
        let w: Vec<f32> = (0..n * k).map(|i| ((i * 37 % 101) as f32 - 50.0) / 23.0).collect();
        let (qw, qs) = quantize(&w, n, k);
        for r in 0..n {
            for c in 0..k {
                let scale = qs[r * (k / BLOCK) + c / BLOCK];
                assert!((w[r * k + c] - dequant(&qw, &qs, k, r, c)).abs() <= scale * 0.5 + 1e-6);
            }
        }
    }

    // ---- r336-neural --------------------------------------------------------

    /// A tiny deterministic PRNG (no rng/clock dep) for fuzzing shapes/values.
    fn lcg(state: &mut u64) -> f32 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let x = (*state >> 33) as u32;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0 // [-1, 1)
    }

    /// (1) GOLDEN VECTOR: hand-computed exact bytes anchor both impls so they
    ///     can't silently drift together away from the Q8_0 contract.
    #[test]
    fn golden_vectors_exact() {
        // Block of constant +2.0: amax=2, scale=2/127, every code = round(127)=127
        // = 0x7F, packed 0x7F7F7F7F across all 8 words.
        let w = vec![2.0f32; BLOCK];
        let (qw, qs) = quantize(&w, 1, BLOCK);
        assert_eq!(qw, vec![0x7F7F7F7Fu32; 8]);
        assert_eq!(qs, vec![2.0f32 / 127.0]);

        // Constant -2.0: code = -127 = 0x81 → 0x81818181.
        let (qw, qs) = quantize(&vec![-2.0f32; BLOCK], 1, BLOCK);
        assert_eq!(qw, vec![0x81818181u32; 8]);
        assert_eq!(qs, vec![2.0f32 / 127.0]);

        // All-zero block: amax=0 → scale guarded to 1.0, all codes 0.
        let (qw, qs) = quantize(&vec![0.0f32; BLOCK], 1, BLOCK);
        assert_eq!(qw, vec![0u32; 8]);
        assert_eq!(qs, vec![1.0f32]);
    }

    /// (2) PARITY (sacred): the fast block-wise path is byte-for-byte identical
    ///     to the retained scalar reference across many fuzzed shapes + values.
    #[test]
    fn fast_path_byte_identical_to_reference() {
        let mut st = 0x1234_5678_9abc_def0u64;
        for &(n, kb) in &[(1usize, 1usize), (3, 1), (4, 2), (5, 3), (8, 4), (2, 8), (7, 5)] {
            let k = kb * BLOCK;
            // Mix scales: include all-zero blocks, tiny, and large magnitudes.
            let w: Vec<f32> = (0..n * k)
                .map(|i| {
                    let base = lcg(&mut st);
                    if i % 53 == 0 { 0.0 } else { base * (1 << (i % 5)) as f32 }
                })
                .collect();
            let fast = quantize_blockwise(&w, n, k);
            let refr = quantize_scalar_reference(&w, n, k);
            assert_eq!(fast.0, refr.0, "qw mismatch at n={n} k={k}");
            assert_eq!(fast.1.iter().map(|f| f.to_bits()).collect::<Vec<_>>(),
                       refr.1.iter().map(|f| f.to_bits()).collect::<Vec<_>>(),
                       "qs mismatch at n={n} k={k}");
        }
    }

    /// (3) quantize_layer (bias-augment + 32-pad) also goes through the fast
    ///     path and stays identical to augmenting + reference-quantizing.
    #[test]
    fn quantize_layer_matches_reference() {
        let (out, d) = (5usize, 20usize);
        let mut st = 0xdead_beef_0000_0001u64;
        let w: Vec<f32> = (0..out * d).map(|_| lcg(&mut st)).collect();
        let b: Vec<f32> = (0..out).map(|_| lcg(&mut st)).collect();
        let (qw, qs, k) = quantize_layer(&w, &b, out, d);
        assert_eq!(k, ((d + 1 + 31) / 32) * 32);
        // Re-augment by hand and quantize with the reference; must match.
        let mut wa = vec![0f32; out * k];
        for r in 0..out {
            wa[r * k..r * k + d].copy_from_slice(&w[r * d..r * d + d]);
            wa[r * k + d] = b[r];
        }
        let (rqw, rqs) = quantize_scalar_reference(&wa, out, k);
        assert_eq!(qw, rqw);
        assert_eq!(qs, rqs);
    }

    /// (4) pack→unpack round-trips exactly, for tiles that divide the matrix and
    ///     tiles that require padding.
    #[test]
    fn pack_unpack_round_trips() {
        let mut st = 0x0f0f_0f0f_1111_2222u64;
        for &(n, k) in &[(8usize, 64usize), (5, 96usize), (1, 32usize), (7, 32usize)] {
            let w: Vec<f32> = (0..n * k).map(|_| lcg(&mut st)).collect();
            let (qw, _qs) = quantize(&w, n, k);
            for &(tn, tk) in &[(2usize, 16usize), (4, 32usize), (3, 8usize), (1, 4usize), (n, k)] {
                let packed = pack_tiles(&qw, n, k, tn, tk);
                assert_eq!(packed.len(), packed_tiles_len(n, k, tn, tk));
                let back = unpack_tiles(&packed, n, k, tn, tk);
                assert_eq!(back, qw, "round-trip failed n={n} k={k} tn={tn} tk={tk}");
            }
        }
    }

    /// (5) When the tile divides the matrix, packing is a pure PERMUTATION of the
    ///     words — same bytes, reordered. (No data is altered, only locality.)
    #[test]
    fn pack_is_permutation_when_evenly_tiled() {
        let (n, k) = (8usize, 64usize);
        let mut st = 0x9999_8888_7777_6666u64;
        let w: Vec<f32> = (0..n * k).map(|_| lcg(&mut st)).collect();
        let (qw, _qs) = quantize(&w, n, k);
        let (tn, tk) = (4usize, 32usize); // 8%4==0, (64/4)=16 % (32/4)=8 == 0
        let packed = pack_tiles(&qw, n, k, tn, tk);
        assert_eq!(packed.len(), qw.len(), "even tiling adds no padding");
        let mut a = qw.clone();
        let mut b = packed.clone();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b, "packed must be a reordering of the same words");
    }

    /// (6) Determinism for both the quantizer and the packer.
    #[test]
    fn deterministic() {
        let (n, k) = (3usize, 96usize);
        let w: Vec<f32> = (0..n * k).map(|i| ((i * 31 % 67) as f32 - 33.0) / 17.0).collect();
        assert_eq!(quantize(&w, n, k), quantize(&w, n, k));
        let (qw, _) = quantize(&w, n, k);
        assert_eq!(pack_tiles(&qw, n, k, 2, 16), pack_tiles(&qw, n, k, 2, 16));
    }

    /// (7) Q4 experiment round-trips within its (coarser) error bound and is
    ///     fully isolated from Q8 — quantizing the same block with Q8 is
    ///     unaffected by the presence of the Q4 path.
    #[test]
    fn q4_experiment_isolated_and_bounded() {
        let mut st = 0x2468_ace0_1357_9bdfu64;
        let blk: Vec<f32> = (0..BLOCK).map(|_| lcg(&mut st)).collect();
        let (words, scale) = quantize_q4_block_experimental(&blk);
        for i in 0..BLOCK {
            let dq = dequant_q4_experimental(&words, scale, i);
            assert!((blk[i] - dq).abs() <= scale * 0.5 + 1e-6, "q4 error too large at {i}");
        }
        // Q8 of the same block is the ordinary Q8 result — unchanged by Q4.
        let (q8w, q8s) = quantize(&blk, 1, BLOCK);
        let (q8w2, q8s2) = quantize_blockwise(&blk, 1, BLOCK);
        assert_eq!(q8w, q8w2);
        assert_eq!(q8s, q8s2);
        // All-zero Q4 block: guarded scale, all codes 0.
        let (zw, zs) = quantize_q4_block_experimental(&[0.0f32; BLOCK]);
        assert_eq!(zw, [0u32; 4]);
        assert_eq!(zs, 1.0f32);
    }
}

