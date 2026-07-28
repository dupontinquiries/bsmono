//! cpumatvec.rs — quantized matvec on the CPU, straight out of the mapped file.
//!
//! The case for this got much stronger after reading the driver: M1/M2 AGX has **no matrix units**, so
//! `VK_KHR_cooperative_matrix` would lower to the same FMAs the GPU already issues; the matrix hardware
//! on the SoC is the ANE, behind a driver with no dma_buf import. Meanwhile the CPU side of an M2 Ultra
//! is 24 cores against the same unified DRAM the GPU reads — no transfer, no dispatch, no watchdog.
//!
//! ## Three things this does differently from the GPU path
//!
//! 1. **No dequantization pass.** The dot product consumes the packed format directly. Dequantizing to
//!    f32 first would triple the bytes touched, and this is bandwidth-bound — the arithmetic is free by
//!    comparison.
//! 2. **No copy.** Weights are read from the `&[u8]` the file is mapped at. The GPU path's entire
//!    staging apparatus — the ring, the completion fence, the 200 GiB of traffic per generation —
//!    exists to move bytes the CPU can simply read.
//! 3. **Vectorized by construction, not by intrinsics.** The inner loop accumulates into a fixed-size
//!    array so LLVM emits NEON on aarch64 and AVX on x86 from the same source. Hand-written intrinsics
//!    would be faster on one target and untestable on the other — and untestable is how the shader's
//!    f16 subnormal bug survived three copies.
//!
//! Correctness is defined against `gguf::dequantize` followed by a plain dot product. Anything else is
//! a different function.

/// Accumulator lanes. Chosen so the array fits a NEON `float32x4_t` pair and an AVX register, which is
/// what lets one source vectorize on both.
const LANES: usize = 8;

/// Dot product of a Q8_0 weight row with an f32 activation, read straight from the file bytes.
///
/// Q8_0 block: f16 scale then 32 int8 quants, 34 bytes for 32 weights.
pub fn dot_q8_0(raw: &[u8], act: &[f32]) -> f32 {
    let nblk = act.len() / 32;
    let mut acc = [0f32; LANES];
    for b in 0..nblk {
        let off = b * 34;
        if off + 34 > raw.len() { break; }
        let d = half_to_f32(u16::from_le_bytes([raw[off], raw[off + 1]]));
        let a = &act[b * 32..b * 32 + 32];
        // 32 quants over LANES accumulators: each lane sums every LANES-th element, so the adds are
        // independent and the loop vectorizes. Lane assignment is FIXED, which keeps the summation
        // order deterministic — a different order would change the last bits and the reference is
        // bit-exact by design.
        for i in 0..32 {
            let q = raw[off + 2 + i] as i8 as f32;
            acc[i % LANES] += d * q * a[i];
        }
    }
    acc.iter().sum()
}

/// f16 -> f32. Same seeding as `gguf::f16_to_f32`: subnormals start at exponent -14 and bias by +127.
///
/// Written out rather than reused across a crate boundary, and therefore a fourth copy of a function
/// that has already gone wrong once — so the test below asserts it against the reference for every
/// subnormal, not a sample.
fn half_to_f32(h: u16) -> f32 {
    let (sign, exp, mant) = ((h >> 15) as u32, ((h >> 10) & 0x1F) as u32, (h & 0x3FF) as u32);
    let bits = if exp == 0 {
        if mant == 0 { sign << 31 } else {
            let (mut m, mut e) = (mant, -14i32);
            while (m & 0x400) == 0 { m <<= 1; e -= 1; }
            (sign << 31) | (((e + 127) as u32) << 23) | ((m & 0x3FF) << 13)
        }
    } else if exp == 31 {
        (sign << 31) | 0x7F80_0000 | (mant << 13)
    } else {
        (sign << 31) | ((exp + 112) << 23) | (mant << 13)
    };
    f32::from_bits(bits)
}

/// How to split `n_rows` across `n_threads`.
///
/// Contiguous ranges, not interleaved: each thread then walks a contiguous span of the weight file,
/// which is what the prefetcher is built for. Interleaving would give every thread the whole file's
/// stride pattern and defeat it.
pub fn split_rows(n_rows: usize, n_threads: usize) -> Vec<(usize, usize)> {
    let t = n_threads.clamp(1, n_rows.max(1));
    let per = n_rows.div_ceil(t);
    (0..t).filter_map(|i| {
        let (s, e) = (i * per, ((i + 1) * per).min(n_rows));
        if s < e { Some((s, e)) } else { None }
    }).collect()
}

/// Threads worth using for a matvec of this size.
///
/// Below a threshold the spawn cost exceeds the work. Measured in bytes touched rather than rows,
/// because a short wide row and a tall narrow matrix cost differently and rows alone cannot tell them
/// apart.
pub fn threads_for(bytes: u64, available: usize) -> usize {
    const MIN_BYTES_PER_THREAD: u64 = 1 << 20;    // 1 MiB
    let want = (bytes / MIN_BYTES_PER_THREAD).max(1) as usize;
    want.min(available.max(1))
}

// ROW BLOCKING WAS TRIED AND IS SLOWER. Kept as a note rather than code.
//
// The idea: one row at a time re-walks the whole activation vector per row, so processing four rows
// together should read it once for four and keep it in L1 while the weights stream past. The reasoning
// is sound and the measurement disagrees:
//
//     per-row  : 19.97 ms   1.3 GiB/s
//     blocked  : 26.10 ms   1.0 GiB/s   (-23.5%)
//
// The 2-D accumulator `[[f32; LANES]; ROW_BLOCK]` does not stay in registers. Per-row, the single
// `[f32; LANES]` lives in registers for the whole loop; blocked, it spills, and the reload traffic
// costs more than the saved activation reads win. The activation was already hot in L1 anyway — it is
// 20 KiB and re-read immediately — so the term being optimized away was close to free to begin with.
//
// Recorded because the argument for it is good enough that someone will propose it again, and because
// deleting a plausible optimization on the strength of a measurement is the same call as removing the
// per-token arena in _231.

/// Quantize an activation block of 32 to int8 with one shared scale, llama.cpp's `quantize_row_q8_0`.
///
/// Scale is `max|x| / 127`, so the largest element maps to +/-127 and nothing clips.
pub fn quantize_act_block(a: &[f32]) -> (f32, [i8; 32]) {
    let amax = a.iter().fold(0f32, |m, v| m.max(v.abs()));
    if amax == 0.0 { return (0.0, [0; 32]); }
    let d = amax / 127.0;
    let inv = 1.0 / d;
    let mut q = [0i8; 32];
    for i in 0..32 { q[i] = (a[i] * inv).round().clamp(-127.0, 127.0) as i8; }
    (d, q)
}

/// Dot with BOTH sides quantized: int8 x int8 accumulated in i32, scaled once per block.
///
/// This is the lesson from llama.cpp's CPU path, and it is not a micro-optimization — it changes what
/// the inner loop IS. Ours multiplies floats: `d * q as f32 * a[i]`, so 32 float multiply-adds per
/// block plus a f32 widening of every quant. llama.cpp accumulates `q_w * q_a` in i32 and applies
/// `d_w * d_a` ONCE at the end of the block. On aarch64 that lowers to SDOT (four int8 MACs per
/// instruction) where the float version lowers to FMLA over widened values.
///
/// ## It is NOT bit-exact, and that is the trade
///
/// Quantizing the activation loses precision the f32 path keeps. llama.cpp accepts this because the
/// error is far below what the weights already introduce and the throughput is several times better.
/// So the gate for this measures ERROR AGAINST A BOUND rather than demanding exactness — demanding
/// exactness of an approximation would just mean never adopting it.
///
/// The activation is quantized once per block and reused across every row in a matvec, so its cost
/// amortizes over `n_rows` — which is why llama.cpp quantizes the whole activation vector up front.
pub fn dot_q8_0_iq(raw: &[u8], act_q: &[(f32, [i8; 32])]) -> f32 {
    let mut total = 0f32;
    for (b, (da, qa)) in act_q.iter().enumerate() {
        let off = b * 34;
        if off + 34 > raw.len() { break; }
        let dw = half_to_f32(u16::from_le_bytes([raw[off], raw[off + 1]]));
        // i32 accumulation: exact, no rounding until the single scale below.
        let mut sum = 0i32;
        for i in 0..32 { sum += (raw[off + 2 + i] as i8 as i32) * (qa[i] as i32); }
        total += dw * da * sum as f32;
    }
    total
}

/// Quantize a whole activation vector once, for reuse across every row.
pub fn quantize_act(act: &[f32]) -> Vec<(f32, [i8; 32])> {
    act.chunks_exact(32).map(quantize_act_block).collect()
}

/// Dot a Q4_K weight row with a pre-quantized activation.
///
/// Q4_K is what every model here actually ships — `Q4_K_M` for Scout, Qwen3 and DeepSeek — so a CPU path
/// that only handled Q8_0 could not run any of them.
///
/// 144-byte super-block of 256 weights: f16 `d`, f16 `dmin`, 12 bytes of packed 6-bit scales and mins,
/// then 128 nibble bytes. A weight is `d*sc*nib - dmin*m`, and the MIN TERM is what makes this different
/// from Q8_0: it does not vanish into a single scale, so the integer accumulation has to carry it
/// separately. `sum(w*a) = d*sc*sum(nib*a) - dmin*m*sum(a)` — two integer sums per sub-block, one
/// float combine. The second sum is over the activation alone and is the same for every row, so it is
/// computed once in `quantize_act` and reused.
pub fn dot_q4_k(raw: &[u8], act_q: &[(f32, [i8; 32])], act_sum: &[f32]) -> f32 {
    let nblk = act_q.len() / 8;              // 8 sub-blocks of 32 per super-block
    let mut total = 0f32;
    for b in 0..nblk {
        let off = b * 144;
        if off + 144 > raw.len() { break; }
        let blk = &raw[off..off + 144];
        let d = half_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
        let dmin = half_to_f32(u16::from_le_bytes([blk[2], blk[3]]));
        let (scales, qs) = (&blk[4..16], &blk[16..144]);
        let (mut is, mut qoff, mut sub) = (0usize, 0usize, 0usize);
        for _ in 0..4 {
            let (sc1, m1) = crate::gguf::get_scale_min_k4(is, scales);
            let (sc2, m2) = crate::gguf::get_scale_min_k4(is + 1, scales);
            for (half, sc, m) in [(0usize, sc1, m1), (1, sc2, m2)] {
                let idx = b * 8 + sub + half;
                let (da, qa) = &act_q[idx];
                let mut acc = 0i32;
                for l in 0..32 {
                    let nib = if half == 0 { qs[qoff + l] & 0xF } else { qs[qoff + l] >> 4 };
                    acc += (nib as i32) * (qa[l] as i32);
                }
                // d*sc*sum(nib*a) - dmin*m*sum(a): the min term uses the activation sum, which does not
                // depend on the weights and is therefore hoisted out of every row.
                total += d * sc as f32 * da * acc as f32 - dmin * m as f32 * act_sum[idx];
            }
            is += 2; qoff += 32; sub += 2;
        }
    }
    total
}

/// Per-block sums of the raw activation, for Q4_K's min term.
///
/// Separate from `quantize_act` because it is the sum of the ORIGINAL floats, not the quantized ones —
/// using the quantized sum would fold the activation's quantization error into the min term as well,
/// where it does not belong.
pub fn act_block_sums(act: &[f32]) -> Vec<f32> {
    act.chunks_exact(32).map(|c| c.iter().sum()).collect()
}

/// Weight formats the CPU path can consume directly from the mapped file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Quant { Q8_0, Q4_K }

impl Quant {
    /// Bytes one row of `k` weights occupies.
    pub fn row_bytes(self, k: usize) -> usize {
        match self { Quant::Q8_0 => (k / 32) * 34, Quant::Q4_K => (k / 256) * 144 }
    }
    /// From a GGUF type id, or None for formats this path does not read.
    ///
    /// Returning None rather than a default matters: a wrong guess here would read a Q6_K tensor as
    /// Q4_K and produce plausible-looking garbage, which is the failure mode this project keeps paying
    /// for. Unsupported means fall back to the GPU, not improvise.
    pub fn from_gguf(t: u32) -> Option<Self> {
        match t { 8 => Some(Quant::Q8_0), 12 => Some(Quant::Q4_K), _ => None }
    }
}

/// Everything about the activation that every row reuses, computed once.
///
/// llama.cpp hoists exactly this out of the row loop, and it is most of why its CPU path is fast: the
/// quantization and the block sums are O(k) while the matvec is O(n_rows * k), so paying them per row
/// would add a term that grows with the matrix.
pub struct ActPrep { pub q: Vec<(f32, [i8; 32])>, pub sums: Vec<f32> }

impl ActPrep {
    pub fn new(act: &[f32]) -> Self { ActPrep { q: quantize_act(act), sums: act_block_sums(act) } }
}

/// Matvec over a mapped weight tensor, on a persistent pool.
///
/// `raw` is the tensor exactly as it sits in the file. No copy, no dequantization pass, no staging.
///
/// Rows split into contiguous spans, one per worker — contiguous so each worker walks a contiguous
/// span of the file and the prefetcher can work. Every worker writes only its own rows, so there is no
/// sharing to synchronise beyond the pool's own barrier.
pub fn matvec(pool: &crate::cpupool::Pool, q: Quant, raw: &[u8], n_rows: usize, k: usize,
              act: &ActPrep) -> Vec<f32> {
    let row_bytes = q.row_bytes(k);
    let mut out = vec![0f32; n_rows];
    let threads = pool.threads();
    let spans = split_rows(n_rows, threads);
    // Raw pointer so each worker can write its disjoint slice; the spans are non-overlapping by
    // construction and `run` joins before `out` is read.
    let base = out.as_mut_ptr() as usize;
    pool.run(|id, _| {
        let Some(&(s, e)) = spans.get(id) else { return };
        for r in s..e {
            let off = r * row_bytes;
            if off + row_bytes > raw.len() { break; }
            let v = match q {
                Quant::Q8_0 => dot_q8_0_iq(&raw[off..off + row_bytes], &act.q),
                Quant::Q4_K => dot_q4_k(&raw[off..off + row_bytes], &act.q, &act.sums),
            };
            // SAFETY: spans are disjoint, so no two workers write the same index, and `run` does not
            // return until every worker has finished.
            unsafe { *(base as *mut f32).add(r) = v; }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a Q8_0 block: f16 scale then 32 int8 quants.
    fn q8_block(d: f32, q: &[i8; 32]) -> Vec<u8> {
        let h = f32_to_half(d);
        let mut v = h.to_le_bytes().to_vec();
        v.extend(q.iter().map(|&x| x as u8));
        v
    }
    fn f32_to_half(f: f32) -> u16 {
        let b = f.to_bits();
        let sign = ((b >> 16) & 0x8000) as u16;
        let exp = ((b >> 23) & 0xFF) as i32 - 127 + 15;
        let mant = (b >> 13) & 0x3FF;
        if exp <= 0 { return sign; }
        sign | ((exp as u16) << 10) | mant as u16
    }

    #[test]
    fn the_dot_matches_dequantize_then_multiply() {
        // The definition of correct. Anything that disagrees with dequantize-then-dot is a different
        // function, however fast it is.
        let d = 0.125f32;
        let q: [i8; 32] = std::array::from_fn(|i| (i as i8) - 16);
        let raw = q8_block(d, &q);
        let act: Vec<f32> = (0..32).map(|i| (i as f32) * 0.25 - 4.0).collect();

        let want: f32 = (0..32).map(|i| d * q[i] as f32 * act[i]).sum();
        let got = dot_q8_0(&raw, &act);
        assert!((want - got).abs() < 1e-4, "want {want}, got {got}");
    }

    #[test]
    fn multiple_blocks_accumulate() {
        let q: [i8; 32] = std::array::from_fn(|i| ((i % 7) as i8) - 3);
        let mut raw = q8_block(0.5, &q);
        raw.extend(q8_block(0.25, &q));
        let act: Vec<f32> = (0..64).map(|i| ((i % 5) as f32) - 2.0).collect();
        let want: f32 = (0..32).map(|i| 0.5 * q[i] as f32 * act[i]).sum::<f32>()
                      + (0..32).map(|i| 0.25 * q[i] as f32 * act[i + 32]).sum::<f32>();
        assert!((want - dot_q8_0(&raw, &act)).abs() < 1e-3);
    }

    #[test]
    fn a_truncated_block_stops_rather_than_reading_past_the_map() {
        // The weights are a view into a mapped file. Reading past the end is a fault, not a wrong
        // answer, so the bound check matters more here than in an owned buffer.
        let q: [i8; 32] = [1; 32];
        let mut raw = q8_block(1.0, &q);
        raw.truncate(20);                       // half a block
        let act = vec![1.0f32; 32];
        assert_eq!(dot_q8_0(&raw, &act), 0.0, "an incomplete block contributes nothing");
    }

    #[test]
    fn the_f16_helper_agrees_with_the_reference_on_EVERY_subnormal() {
        // This is a fourth copy of a function that has already been wrong once: the shader's subnormal
        // branch seeded its exponent at -1 instead of -14 and halved every subnormal, surviving review
        // in two of three copies. Sampling would have missed it — subnormal scales are rare — so this
        // checks all 1024 of them plus the boundaries.
        for m in 0..0x400u16 {
            let h = m;                                    // exp=0 => subnormal
            let want = crate::gguf::f16_to_f32(h);
            assert_eq!(half_to_f32(h).to_bits(), want.to_bits(), "subnormal mant={m}");
        }
        for h in [0u16, 0x8000, 0x3C00, 0xBC00, 0x7BFF, 0x0400, 0x7C00] {
            assert_eq!(half_to_f32(h).to_bits(), crate::gguf::f16_to_f32(h).to_bits(), "h={h:#06x}");
        }
    }

    #[test]
    fn the_threaded_matvec_is_INDEPENDENT_of_thread_count() {
        // A result that varies with the pool size is untestable, and would make every later comparison
        // ambiguous. Each row's arithmetic must not depend on which thread ran it.
        let q: [i8; 32] = std::array::from_fn(|i| ((i * 7 % 13) as i8) - 6);
        let (rows, k) = (37usize, 64usize);
        let mut raw = Vec::new();
        for r in 0..rows {
            for b in 0..k / 32 {
                let d = 0.05 * (r + b + 1) as f32;
                raw.extend(q8_block(d, &q));
            }
        }
        let act: Vec<f32> = (0..k).map(|i| ((i % 9) as f32) - 4.0).collect();
        let prep = ActPrep::new(&act);
        let one = matvec(&crate::cpupool::Pool::new(1), Quant::Q8_0, &raw, rows, k, &prep);
        for t in [2usize, 3, 8, 24, 64] {
            let many = matvec(&crate::cpupool::Pool::new(t), Quant::Q8_0, &raw, rows, k, &prep);
            assert_eq!(one, many, "thread count {t} changed the result");
        }
        assert_eq!(one.len(), rows);
        assert!(one.iter().any(|v| *v != 0.0), "and it computed something");
    }

    #[test]
    fn each_row_matches_the_single_row_dot() {
        // The threaded driver must not be a second implementation. Every row equals dot_q8_0 on that
        // row's bytes, which is the property that lets the dot's own tests carry the correctness.
        let q: [i8; 32] = std::array::from_fn(|i| (i as i8) - 16);
        let (rows, k) = (5usize, 32usize);
        let mut raw = Vec::new();
        for r in 0..rows { raw.extend(q8_block(0.1 * (r + 1) as f32, &q)); }
        let act: Vec<f32> = (0..k).map(|i| (i as f32) * 0.1).collect();
        let prep = ActPrep::new(&act);
        let out = matvec(&crate::cpupool::Pool::new(4), Quant::Q8_0, &raw, rows, k, &prep);
        for r in 0..rows {
            // The driver must not be a second implementation: every row equals the single-row dot on
            // that row's bytes, which is what lets the dot's own tests carry the correctness.
            let want = dot_q8_0_iq(&raw[r * 34..(r + 1) * 34], &prep.q);
            assert_eq!(out[r], want, "row {r}");
        }
    }

    #[test]
    fn the_integer_dot_is_close_to_the_float_one_but_NOT_equal() {
        // The trade llama.cpp makes. Quantizing the activation loses precision the f32 path keeps, and
        // demanding exactness of an approximation would just mean never adopting it — so the bar is a
        // BOUND, not equality. Anything under ~1% here is far below what Q8_0 weights already introduce.
        let q: [i8; 32] = std::array::from_fn(|i| ((i * 5 % 61) as i8) - 30);
        let raw = q8_block(0.07, &q);
        let act: Vec<f32> = (0..32).map(|i| ((i * 13 % 29) as f32) - 14.0).collect();
        let f = dot_q8_0(&raw, &act);
        let i = dot_q8_0_iq(&raw, &quantize_act(&act));
        let rel = (f - i).abs() / f.abs().max(1e-6);
        assert!(rel < 0.01, "float {f}, integer {i}, rel {rel:.4}");
        assert_ne!(f, i, "and it is genuinely a different computation, not a renamed one");
    }

    #[test]
    fn activation_quantization_never_clips_the_largest_element() {
        // Scale is max|x|/127, so the extreme maps to exactly +/-127. A scale that clipped would lose
        // the largest contribution to every dot — the one that matters most.
        let a: Vec<f32> = (0..32).map(|i| if i == 7 { -9.5 } else { (i as f32) * 0.1 }).collect();
        let (d, q) = quantize_act_block(&a);
        assert_eq!(q[7], -127, "the extreme maps to the limit, not past it");
        assert!((d - 9.5 / 127.0).abs() < 1e-6);
        assert!(q.iter().all(|v| *v != i8::MIN), "and nothing lands on -128, which has no positive twin");
    }

    #[test]
    fn an_all_zero_activation_block_quantizes_to_zero_not_NaN() {
        // max|x| = 0 makes the scale a division by zero. A NaN here would poison an entire row.
        let (d, q) = quantize_act_block(&[0.0; 32]);
        assert_eq!(d, 0.0);
        assert!(q.iter().all(|v| *v == 0));
        let raw = q8_block(1.0, &[1i8; 32]);
        assert_eq!(dot_q8_0_iq(&raw, &[(d, q)]), 0.0);
    }

    #[test]
    fn the_q4k_dot_matches_dequantize_then_multiply() {
        // Q4_K_M is what every model here ships, so this is the format that decides whether the CPU
        // path can run anything at all. Correctness is defined against gguf::dequantize followed by a
        // plain dot — anything else is a different function.
        let raw: Vec<u8> = (0..144).map(|i| ((i * 37 + 11) % 251) as u8).collect();
        let w = crate::gguf::dequantize(12, &raw, 256).expect("cpu q4_k");
        let act: Vec<f32> = (0..256).map(|i| ((i * 13 % 29) as f32) - 14.0).collect();

        let want: f32 = w.iter().zip(&act).map(|(a, b)| a * b).sum();
        let got = dot_q4_k(&raw, &quantize_act(&act), &act_block_sums(&act));
        let rel = (want - got).abs() / want.abs().max(1e-3);
        assert!(rel < 0.01, "want {want}, got {got}, rel {rel:.4}");
    }

    #[test]
    fn the_q4k_min_term_uses_the_UNQUANTIZED_activation_sum() {
        // sum(w*a) = d*sc*sum(nib*a) - dmin*m*sum(a). The second sum is over the ORIGINAL floats:
        // using the quantized ones would fold the activation's quantization error into the min term as
        // well, where it does not belong, and the error would grow with dmin rather than staying bounded.
        let act: Vec<f32> = (0..64).map(|i| (i as f32) * 0.37 - 11.0).collect();
        let sums = act_block_sums(&act);
        assert_eq!(sums.len(), 2);
        let direct: f32 = act[..32].iter().sum();
        assert_eq!(sums[0], direct, "the sum is of the raw activation, not a reconstruction");

        // and it is genuinely different from summing the quantized values back
        let (d, q) = quantize_act_block(&act[..32]);
        let requant: f32 = q.iter().map(|v| *v as f32 * d).sum();
        assert_ne!(sums[0], requant, "if these were equal the distinction would be untestable");
    }

    #[test]
    fn a_truncated_q4k_block_stops_rather_than_reading_past_the_map() {
        let mut raw: Vec<u8> = (0..144).map(|i| (i % 251) as u8).collect();
        raw.truncate(100);
        let act = vec![1.0f32; 256];
        assert_eq!(dot_q4_k(&raw, &quantize_act(&act), &act_block_sums(&act)), 0.0);
    }

    #[test]
    fn rows_split_into_CONTIGUOUS_spans() {
        // Contiguous, not interleaved: each thread walks a contiguous span of the weight file, which is
        // what the hardware prefetcher is built for. Interleaving hands every thread the whole file's
        // stride pattern and defeats it.
        let s = split_rows(100, 8);
        assert_eq!(s.first().unwrap().0, 0);
        assert_eq!(s.last().unwrap().1, 100);
        for w in s.windows(2) { assert_eq!(w[0].1, w[1].0, "spans must be contiguous and gapless"); }
        // and every row is covered exactly once
        assert_eq!(s.iter().map(|(a, b)| b - a).sum::<usize>(), 100);
    }

    #[test]
    fn splitting_never_produces_empty_spans_or_more_threads_than_rows() {
        assert_eq!(split_rows(3, 24).len(), 3, "24 threads for 3 rows is 3 spans, not 24");
        assert_eq!(split_rows(0, 8).len(), 0);
        assert!(split_rows(1, 1).iter().all(|(a, b)| a < b));
    }

    #[test]
    fn a_q4k_matvec_matches_row_by_row_dequantization() {
        // End to end on the format every model here ships. If this is wrong the CPU path is useless
        // however fast it is.
        let (rows, k) = (9usize, 256usize);
        let mut raw = Vec::new();
        for r in 0..rows {
            let mut b: Vec<u8> = (0..144).map(|i| ((i * 37 + r * 13 + 11) % 251) as u8).collect();
            // Pin d and dmin to real f16 values. Arbitrary bytes there decode to NaN or Inf for some
            // rows, and then BOTH paths agree at NaN while the comparison itself is NaN — a fixture
            // that fails without either implementation being wrong.
            let (d, dmin) = (f32_to_half(0.05 + r as f32 * 0.01), f32_to_half(0.02));
            b[0..2].copy_from_slice(&d.to_le_bytes());
            b[2..4].copy_from_slice(&dmin.to_le_bytes());
            raw.extend(b);
        }
        let act: Vec<f32> = (0..k).map(|i| ((i * 7 % 23) as f32) - 11.0).collect();
        let prep = ActPrep::new(&act);
        let out = matvec(&crate::cpupool::Pool::new(3), Quant::Q4_K, &raw, rows, k, &prep);
        for r in 0..rows {
            let w = crate::gguf::dequantize(12, &raw[r * 144..(r + 1) * 144], 256).unwrap();
            let want: f32 = w.iter().zip(&act).map(|(a, b)| a * b).sum();
            let rel = (want - out[r]).abs() / want.abs().max(1e-3);
            assert!(rel < 0.01, "row {r}: want {want}, got {}, rel {rel:.4}", out[r]);
        }
    }

    #[test]
    fn an_unsupported_quant_returns_None_rather_than_guessing() {
        // A wrong guess would read a Q6_K tensor as Q4_K and produce plausible garbage — the exact
        // failure this project keeps paying for. Unsupported means fall back to the GPU, not improvise.
        assert_eq!(Quant::from_gguf(8), Some(Quant::Q8_0));
        assert_eq!(Quant::from_gguf(12), Some(Quant::Q4_K));
        for t in [14u32, 19, 0, 1, 99] { assert_eq!(Quant::from_gguf(t), None, "type {t}"); }
    }

    #[test]
    fn row_bytes_matches_the_on_disk_layout() {
        // Getting this wrong reads every row at the wrong offset — a silent misalignment rather than an
        // error, so it is worth pinning against the format constants directly.
        assert_eq!(Quant::Q8_0.row_bytes(4096), (4096 / 32) * 34);
        assert_eq!(Quant::Q4_K.row_bytes(4096), (4096 / 256) * 144);
    }

    #[test]
    fn thread_count_scales_with_BYTES_not_rows() {
        // A short wide row and a tall narrow matrix cost differently, and a row count cannot tell them
        // apart. Below a threshold the spawn cost exceeds the work.
        assert_eq!(threads_for(1 << 10, 24), 1, "a kilobyte is not worth a thread pool");
        assert_eq!(threads_for(64 << 20, 24), 24, "64 MiB uses all of them");
        assert_eq!(threads_for(4 << 20, 24), 4);
        assert_eq!(threads_for(1 << 30, 1), 1, "never more than are available");
    }
}
