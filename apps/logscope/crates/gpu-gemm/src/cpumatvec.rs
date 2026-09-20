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
        let blk = &raw[off..off + 34];
        let d = half_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
        let w: &[u8; 32] = blk[2..34].try_into().unwrap();
        let a = &act[b * 32..b * 32 + 32];
        // 32 quants over LANES accumulators: each lane sums every LANES-th element, so the adds are
        // independent and the loop vectorizes. Lane assignment is FIXED, which keeps the summation
        // order deterministic — a different order would change the last bits and the reference is
        // bit-exact by design.
        for i in 0..32 { acc[i % LANES] += d * (w[i] as i8 as f32) * a[i]; }
    }
    acc.iter().sum()
}

/// f16 -> f32. Same seeding as `gguf::f16_to_f32`: subnormals start at exponent -14 and bias by +127.
///
/// Written out rather than reused across a crate boundary, and therefore a fourth copy of a function
/// that has already gone wrong once — so the test below asserts it against the reference for every
/// subnormal, not a sample.
/// IEEE half -> f32: `gguf::f16_to_f32`, the ONE implementation.
///
/// This module used to carry its own — the branchy form gguf.rs replaced (three branches and a
/// data-dependent normalising loop, so it neither pipelines nor vectorises), called once per block
/// per row: 128 times per 4096-wide Q8_0 row. With realistic, VARYING block scales that cost the
/// Q8_0 dot ~6% (1.205 -> 1.137 ms per 1024x4096, -O). The better reason is gguf.rs's own warning:
/// that function HAS A HISTORY — decoding subnormal halves 4x small produced word salad from one
/// model family while another stayed coherent — and two independent copies of it is how that
/// happens twice. `the_half_decoder_is_the_one_in_gguf` pins the old copy to it over all 65536
/// inputs: identical everywhere except the PAYLOAD bits of 1022 NaNs, which are NaN either way.
#[inline(always)]
fn half_to_f32(h: u16) -> f32 { crate::gguf::f16_to_f32(h) }

/// The copy this module used to decode with. Kept only as the oracle for the test named above.
#[cfg(test)]
fn half_to_f32_branchy(h: u16) -> f32 {
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
#[inline(always)]
fn dot_q8_0_iq_body(raw: &[u8], act_q: &[(f32, [i8; 32])]) -> f32 {
    let mut total = 0f32;
    for (b, (da, qa)) in act_q.iter().enumerate() {
        let off = b * 34;
        if off + 34 > raw.len() { break; }
        // SLICE THE BLOCK ONCE. Indexing `raw[off + 2 + i]` cannot be proven in range, so LLVM emits a
        // bounds check per element; slicing to a fixed-size array proves the length once and the checks
        // vanish. Measured at 3x on this loop — which is the answer to "is Rust compiling differently
        // than C++": no, but Rust makes it easy to write a shape that defeats the optimizer, and the fix
        // is a slice rather than `unsafe` or FFI.
        let blk = &raw[off..off + 34];
        let dw = half_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
        let w: &[u8; 32] = blk[2..34].try_into().unwrap();
        // i32 accumulation: exact, no rounding until the single scale below.
        let mut sum = 0i32;
        for i in 0..32 { sum += (w[i] as i8 as i32) * (qa[i] as i32); }
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
/// All eight (scale6, min6) pairs of a Q4_K super-block, unpacked in one straight line:
/// `gguf::get_scale_min_k4(j, ..)` for j = 0..8 with its `j < 4` branch resolved at compile time.
#[inline(always)]
pub fn scales_k4(q: &[u8; 12]) -> ([u8; 8], [u8; 8]) {
    let (mut sc, mut mn) = ([0u8; 8], [0u8; 8]);
    for j in 0..4 { sc[j] = q[j] & 63; mn[j] = q[j + 4] & 63; }
    for j in 4..8 { sc[j] = (q[j + 4] & 0xF) | ((q[j - 4] >> 6) << 4); mn[j] = (q[j + 4] >> 4) | ((q[j] >> 6) << 4); }
    (sc, mn)
}

/// Dot a Q4_K weight row with a pre-quantized activation.
///
/// THE VECTOR LOOP WAS NEVER THE PROBLEM. This ran at half Q8_0's rate and gained almost nothing
/// from wider SIMD (1.23 ms at SSE2, 1.05 at AVX2 per 1024x4096 matvec), which is the signature of
/// scalar work starving the vector work. The disassembly said what: around the packed multiply-adds
/// sat 32 scalar byte loads, 8 scalar multiplies, 28 branches and 3 bounds-check panic paths PER
/// SUPER-BLOCK — `get_scale_min_k4` called eight times per 256 weights, each with its own `j < 4`
/// branch, plus checked indexing into `act_q` and `act_sum` per sub-block. (Q8_0: 2 byte loads, 11
/// branches.) Now the eight (scale, min) pairs are unpacked once in a straight line, and the three
/// inputs are walked in lockstep as fixed-size chunks — one length proof per super-block.
///
/// What I tried first and did NOT keep: computing the products in i16 and widening only the sum,
/// on the theory that the i32 multiply was the cost. Bit-identical, and exactly the same speed —
/// LLVM was already narrowing it to the packed 16-bit form. Measured before editing; no edit.
///
/// The float expression and its ORDER are unchanged (low half, then high half, per pair), so this
/// is BIT-IDENTICAL to `dot_q4_k_reference`, which is the old function verbatim and the oracle.
/// 1.30 -> 0.64 ms at baseline x86-64, 1.48 -> 0.64 at AVX2. ARM not compiled or measured here; the
/// gain is from removing scalar bookkeeping, which a narrow-SIMD core pays for at least as dearly.
///
/// One behavioural difference, on MALFORMED input only: a too-short `act_sum` used to panic on an
/// index; the lockstep walk now stops at the shortest input instead.
#[inline(always)]
fn dot_q4_k_body(raw: &[u8], act_q: &[(f32, [i8; 32])], act_sum: &[f32]) -> f32 {
    let nblk = act_q.len() / 8;              // 8 sub-blocks of 32 per super-block
    let mut total = 0f32;
    for ((blk, aq), asum) in raw.chunks_exact(144).zip(act_q.chunks_exact(8)).zip(act_sum.chunks_exact(8)).take(nblk) {
        let blk: &[u8; 144] = blk.try_into().unwrap();
        let aq: &[(f32, [i8; 32]); 8] = aq.try_into().unwrap();
        let asum: &[f32; 8] = asum.try_into().unwrap();
        let d = half_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
        let dmin = half_to_f32(u16::from_le_bytes([blk[2], blk[3]]));
        let (sc, mn) = scales_k4(blk[4..16].try_into().unwrap());
        for pair in 0..4 {
            // Each 32-byte run holds TWO sub-blocks: low nibbles, then high nibbles.
            let nibs: &[u8; 32] = blk[16 + pair * 32..16 + pair * 32 + 32].try_into().unwrap();
            let (lo, hi) = (2 * pair, 2 * pair + 1);
            let mut acc_lo = 0i32;
            for (&n, &a) in nibs.iter().zip(aq[lo].1.iter()) { acc_lo += ((n & 0xF) as i32) * (a as i32); }
            let mut acc_hi = 0i32;
            for (&n, &a) in nibs.iter().zip(aq[hi].1.iter()) { acc_hi += ((n >> 4) as i32) * (a as i32); }
            // d*sc*sum(nib*a) - dmin*m*sum(a): the min term uses the activation sum, which does not
            // depend on the weights and is therefore hoisted out of every row. SAME order as the
            // reference: low half, then high half.
            total += d * sc[lo] as f32 * aq[lo].0 * acc_lo as f32 - dmin * mn[lo] as f32 * asum[lo];
            total += d * sc[hi] as f32 * aq[hi].0 * acc_hi as f32 - dmin * mn[hi] as f32 * asum[hi];
        }
    }
    total
}

/// `dot_q4_k` as it was. Kept verbatim: it is the oracle the fast one is held bit-identical to.
pub fn dot_q4_k_reference(raw: &[u8], act_q: &[(f32, [i8; 32])], act_sum: &[f32]) -> f32 {
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
                // Same slicing discipline as Q8_0: a fixed-size view proves the length once instead of
                // a bounds check per nibble.
                let nibs: &[u8; 32] = qs[qoff..qoff + 32].try_into().unwrap();
                // UNSWITCHED. `half` is loop-invariant, but with the test inside the body the
                // compiler emitted a mixed scalar/SIMD loop (18 vector vs 25 scalar instructions in
                // the disassembly). Two straight-line loops give it 32 identical lanes to widen.
                let mut acc = 0i32;
                if half == 0 {
                    for (&n, &a) in nibs.iter().zip(qa.iter()) { acc += ((n & 0xF) as i32) * (a as i32); }
                } else {
                    for (&n, &a) in nibs.iter().zip(qa.iter()) { acc += ((n >> 4) as i32) * (a as i32); }
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

/// Dot a Q6_K weight row with a pre-quantized activation.
///
/// Why this format, and why now: `Q4_K_M` — the GGUF nearly everyone downloads — stores ffn_gate and
/// ffn_up as Q4_K and **ffn_down as Q6_K**. With only Q4_K here, not one expert of such a file could
/// be computed on the CPU, which is what stands between this module and computing a streamed MoE's
/// MISSED experts in place against the mmap instead of copying them into a GPU slot (the GPU-side
/// fill cannot take Q6_K either: see the LOGSCOPE_Q6K_VIEWS gate in the loader).
///
/// 210-byte super-block of 256 weights: 128 bytes `ql` (low nibbles), 64 bytes `qh` (high 2 bits),
/// 16 int8 sub-block scales, f16 `d` LAST (offset 208 — Q4_K keeps its `d` first). A weight is
/// `d * sc[j/16] * (q6 - 32)`. No min term: the -32 recentre folds into the integer, so unlike Q4_K
/// there is one integer sum per sub-block and no activation-sum side channel.
///
/// The two granularities do not line up and that is the one thing to get right: weights are scaled
/// per SIXTEEN, activations were quantized per THIRTY-TWO. So each activation block spans two weight
/// sub-blocks with different scales, and the integer sum must be split at 16 — one sum per 32 with
/// either scale is wrong by exactly the difference between neighbouring scales.
#[inline(always)]
fn dot_q6_k_body(raw: &[u8], act_q: &[(f32, [i8; 32])]) -> f32 {
    let nblk = act_q.len() / 8;              // 8 activation blocks of 32 per super-block
    let mut total = 0f32;
    for b in 0..nblk {
        let off = b * 210;
        if off + 210 > raw.len() { break; }
        let blk = &raw[off..off + 210];
        let d = half_to_f32(u16::from_le_bytes([blk[208], blk[209]]));
        // Unpack the 6-bit quants once into a flat, recentred array. gguf::dequantize's layout, in
        // integers: two 128-wide halves, each interleaving four 32-runs across ql's nibbles and qh's
        // bit pairs. Flat i8 is what lets the dot below be 16 identical lanes.
        let mut q = [0i8; 256];
        for half in 0..2usize {
            let ql = &blk[half * 64..half * 64 + 64];
            let qh = &blk[128 + half * 32..128 + half * 32 + 32];
            let o = half * 128;
            for l in 0..32 {
                q[o + l]      = (((ql[l] & 0xF)      | ((qh[l] & 3) << 4)) as i32 - 32) as i8;
                q[o + l + 32] = (((ql[l + 32] & 0xF) | (((qh[l] >> 2) & 3) << 4)) as i32 - 32) as i8;
                q[o + l + 64] = (((ql[l] >> 4)       | (((qh[l] >> 4) & 3) << 4)) as i32 - 32) as i8;
                q[o + l + 96] = (((ql[l + 32] >> 4)  | (((qh[l] >> 6) & 3) << 4)) as i32 - 32) as i8;
            }
        }
        let sc = &blk[192..208];
        for a32 in 0..8 {
            let (da, qa) = &act_q[b * 8 + a32];
            for sub in 0..2 {
                let w: &[i8; 16] = q[a32 * 32 + sub * 16..a32 * 32 + sub * 16 + 16].try_into().unwrap();
                let a: &[i8; 16] = qa[sub * 16..sub * 16 + 16].try_into().unwrap();
                let mut acc = 0i32;
                for (&wv, &av) in w.iter().zip(a.iter()) { acc += (wv as i32) * (av as i32); }
                total += d * (sc[a32 * 2 + sub] as i8) as f32 * da * acc as f32;
            }
        }
    }
    total
}

// ───────────────────────── instruction-set dispatch ─────────────────────────
//
// WHAT THE BUILD TELLS THE COMPILER ABOUT THE CPU: nothing. apps/logscope/.cargo/config.toml sets
// rustflags for x86_64 only (a linker flag), so these dots compile for BASELINE — x86-64 SSE2,
// AArch64 ARMv8.0. `dot_q8_0_iq`'s own doc says the int8 dot "lowers to SDOT on aarch64"; that is
// only true if `dotprod` is enabled, and it never was, so on ARM it has been widening multiplies.
// `target-cpu=native` is the wrong fix for anything that gets packaged: the PinePhone's A53 has no
// dotprod and would SIGILL.
//
// So: each dot is ONE `#[inline(always)]` body, and `isa_dispatch!` compiles that body a second
// time inside a `#[target_feature]` wrapper, chosen at runtime. No intrinsics and no second
// implementation — the same source, which is what keeps the results BIT-IDENTICAL across paths
// (integer sums are exact; the float expressions are untouched, and only `avx2` / `dotprod` are
// enabled, never `fma`, so nothing can be contracted). Measured on x86-64, whole matvec, -O:
// Q8_0 0.60 -> 0.38 ms, Q6_K 0.91 -> 0.50 ms with AVX2; Q4_K does not move (it was never
// SIMD-bound — see dot_q4_k_body).
//
// LOGSCOPE_CPU_ISA=baseline forces the baseline copy, for A/B and for blaming this first.

/// 0 = undecided, 1 = baseline, 2 = wide. Decided once; `set_cpu_isa_wide` overrides (tests, levers).
static ISA: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Force (`Some`) or re-detect (`None`) the wide instruction-set path. Returns what is now in force.
pub fn set_cpu_isa_wide(force: Option<bool>) -> bool {
    let v = match force { Some(w) => w && isa_detect(), None => isa_detect() && std::env::var("LOGSCOPE_CPU_ISA").as_deref() != Ok("baseline") };
    ISA.store(if v { 2 } else { 1 }, std::sync::atomic::Ordering::Relaxed);
    v
}

#[inline(always)]
fn isa_wide() -> bool {
    match ISA.load(std::sync::atomic::Ordering::Relaxed) { 2 => true, 1 => false, _ => set_cpu_isa_wide(None) }
}

/// Does THIS CPU have the feature the wide copy was compiled for? False where no wide copy exists.
fn isa_detect() -> bool {
    #[cfg(target_arch = "x86_64")]
    { return std::arch::is_x86_feature_detected!("avx2"); }
    #[cfg(all(target_arch = "aarch64", feature = "arm-dotprod"))]
    { return std::arch::is_aarch64_feature_detected!("dotprod"); }
    #[allow(unreachable_code)]
    false
}

/// Which copy the dots are running, for a log line: the question "did the fast path engage" should
/// have an answer that does not involve a profiler.
pub fn cpu_isa_name() -> &'static str {
    if !isa_wide() { return "baseline"; }
    if cfg!(target_arch = "x86_64") { "avx2" } else { "dotprod" }
}

macro_rules! isa_dispatch {
    ($(#[$doc:meta])* $name:ident = $body:ident, wide: $wide:ident, ($($arg:ident : $ty:ty),*) -> $ret:ty) => {
        #[cfg(target_arch = "x86_64")]
        #[target_feature(enable = "avx2")]
        unsafe fn $wide($($arg: $ty),*) -> $ret { $body($($arg),*) }
        // NEVER COMPILED by its author (no AArch64 std in the sandbox) — hence the opt-in feature.
        // It is the x86 arm above with two strings changed.
        #[cfg(all(target_arch = "aarch64", feature = "arm-dotprod"))]
        #[target_feature(enable = "dotprod")]
        unsafe fn $wide($($arg: $ty),*) -> $ret { $body($($arg),*) }

        $(#[$doc])*
        pub fn $name($($arg: $ty),*) -> $ret {
            #[cfg(any(target_arch = "x86_64", all(target_arch = "aarch64", feature = "arm-dotprod")))]
            if isa_wide() {
                // SAFETY: `isa_wide` is true only after the runtime check for exactly the feature
                // `$wide` was compiled with (isa_detect), on this CPU.
                return unsafe { $wide($($arg),*) };
            }
            $body($($arg),*)
        }
    };
}

isa_dispatch! {
    /// Q8_0 row · pre-quantized activation. See `dot_q8_0_iq_body` for the arithmetic; this picks the copy.
    dot_q8_0_iq = dot_q8_0_iq_body, wide: dot_q8_0_iq_wide, (raw: &[u8], act_q: &[(f32, [i8; 32])]) -> f32
}
isa_dispatch! {
    /// Q4_K row · pre-quantized activation. See `dot_q4_k_body`; bit-identical to `dot_q4_k_reference`.
    dot_q4_k = dot_q4_k_body, wide: dot_q4_k_wide, (raw: &[u8], act_q: &[(f32, [i8; 32])], act_sum: &[f32]) -> f32
}
isa_dispatch! {
    /// Q6_K row · pre-quantized activation. See `dot_q6_k_body` for the arithmetic; this picks the copy.
    dot_q6_k = dot_q6_k_body, wide: dot_q6_k_wide, (raw: &[u8], act_q: &[(f32, [i8; 32])]) -> f32
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
/// Named with ggml's spellings rather than Rust's casing convention, deliberately: renaming these to
/// `Q8Zero` and `Q4K` would make a grep for the format name miss the enum representing it, and this
/// codebase cross-references ggml type names constantly.
#[allow(non_camel_case_types)]
pub enum Quant { Q8_0, Q4_K, Q6_K }

impl Quant {
    /// Weights per quantization block. A row whose width is not a multiple of this has no on-disk
    /// layout in this format, and `row_bytes` — integer division — would report ZERO bytes for a
    /// 64-wide Q6_K row, which `matvec` then reads as "every row is empty" and returns zeros for.
    /// Callers must check `fits` before trusting `row_bytes`.
    pub fn block(self) -> usize {
        match self { Quant::Q8_0 => 32, Quant::Q4_K | Quant::Q6_K => 256 }
    }
    pub fn fits(self, k: usize) -> bool { k > 0 && k % self.block() == 0 }
    /// Bytes one row of `k` weights occupies.
    pub fn row_bytes(self, k: usize) -> usize {
        match self { Quant::Q8_0 => (k / 32) * 34, Quant::Q4_K => (k / 256) * 144, Quant::Q6_K => (k / 256) * 210 }
    }
    /// From a GGUF type id, or None for formats this path does not read.
    ///
    /// Returning None rather than a default matters: a wrong guess here would read a Q6_K tensor as
    /// Q4_K and produce plausible-looking garbage, which is the failure mode this project keeps paying
    /// for. Unsupported means fall back to the GPU, not improvise.
    pub fn from_gguf(t: u32) -> Option<Self> {
        match t { 8 => Some(Quant::Q8_0), 12 => Some(Quant::Q4_K), 14 => Some(Quant::Q6_K), _ => None }
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

/// The byte range of one layer's weights, for prefetching the next while computing this one.
///
/// Colibri's `couple_prefetch` issued the NEXT layer's expert fills during the current layer's stall.
/// The CPU analogue is first-touch page faults: weights read from an mmap are not resident until
/// touched, and a 60 GiB model faults every page exactly once — during the first token, serialised
/// against the arithmetic that needs them. Handing this range to `crate::prefetch_mapped` for layer
/// N+1 while computing layer N overlaps the fault with useful work.
///
/// Returns None past the end rather than clamping: a clamped range would prefetch the last layer
/// repeatedly, which is wasted syscalls that look like progress.
pub fn layer_range(raw: &[u8], layer: usize, bytes_per_layer: usize) -> Option<&[u8]> {
    let start = layer.checked_mul(bytes_per_layer)?;
    if start >= raw.len() { return None; }
    let end = (start + bytes_per_layer).min(raw.len());
    Some(&raw[start..end])
}

// A SCRATCH BUFFER WAS CONSIDERED AND IS NOT WORTH IT. `matvec` allocates a fresh Vec per call and a
// decode makes ~336 calls per token, which looks like it should matter. Measured:
//
//     full call     : 13.487 ms
//     the Vec alone :  0.001 ms   (0.01% of the call)
//     at 336 calls  :  0.24 ms per token spent allocating
//
// Below noise. Adding a reusable buffer would be the per-token arena from _227 repeated on a different
// processor — built on a plausible argument, measured at zero, and deleted in _231. The measurement is
// recorded so the argument does not get made a third time.

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
                Quant::Q6_K => dot_q6_k(&raw[off..off + row_bytes], &act.q),
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

    /// Pseudo-random but VALID Q6_K rows: every byte pattern is a legal ql/qh/scale, and `d` is set to
    /// a normal f16 so the oracle is not comparing NaNs. `rows` super-blocks per row, seeded.
    fn q6k_bytes(blocks: usize, seed: u32) -> Vec<u8> {
        let mut x = seed | 1;
        let mut v = Vec::with_capacity(blocks * 210);
        for b in 0..blocks {
            for _ in 0..208 { x = x.wrapping_mul(1664525).wrapping_add(1013904223); v.push((x >> 24) as u8); }
            v.extend_from_slice(&f32_to_half(0.004 + 0.001 * b as f32).to_le_bytes());
        }
        v
    }

    /// The sharp oracle. Against the TRUE activation a quantized dot is only ever "within a percent",
    /// and a percent hides a wrong sub-block scale. So compare against gguf::dequantize dotted with
    /// the DEQUANTIZED QUANTIZED activation — the same two operands dot_q6_k sees — where the only
    /// difference left is float summation order.
    #[test]
    fn the_q6k_dot_is_dequantize_then_multiply_on_the_same_operands() {
        for (blocks, seed) in [(1usize, 7u32), (4, 99), (16, 12345)] {
            let raw = q6k_bytes(blocks, seed);
            let k = blocks * 256;
            let w = crate::gguf::dequantize(14, &raw, k).expect("cpu q6_k");
            let act: Vec<f32> = (0..k).map(|i| (((i * 31 + 7) % 61) as f32 - 30.0) * 0.173).collect();
            let aq = quantize_act(&act);
            let act_rt: Vec<f32> = aq.iter().flat_map(|(d, q)| q.iter().map(move |&v| v as f32 * d)).collect();
            let want: f64 = w.iter().zip(&act_rt).map(|(a, b)| *a as f64 * *b as f64).sum();
            let got = dot_q6_k(&raw, &aq) as f64;
            let scale: f64 = w.iter().zip(&act_rt).map(|(a, b)| (*a as f64 * *b as f64).abs()).sum();
            assert!((want - got).abs() <= 2e-6 * scale, "{blocks} blocks: want {want}, got {got}, |terms| {scale}");
            // and against the true activation it is the usual percent-class agreement
            let truth: f64 = w.iter().zip(&act).map(|(a, b)| *a as f64 * *b as f64).sum();
            assert!((truth - got).abs() <= 0.02 * scale, "{blocks} blocks vs true activation: {truth} vs {got}");
        }
    }

    /// Weights scale per 16, activations per 32: each activation block straddles two scales. Make
    /// exactly ONE of the 16 sub-block scales non-zero and the dot must see exactly that sub-block —
    /// a sum taken per 32 with either neighbour's scale gets this wrong for 15 of the 16.
    #[test]
    fn every_q6k_sub_block_uses_its_own_scale() {
        let base = q6k_bytes(1, 4242);
        let act: Vec<f32> = (0..256).map(|i| ((i % 17) as f32 - 8.0) * 0.5 + 0.25).collect();
        let aq = quantize_act(&act);
        let act_rt: Vec<f32> = aq.iter().flat_map(|(d, q)| q.iter().map(move |&v| v as f32 * d)).collect();
        for live in 0..16usize {
            let mut raw = base.clone();
            for s in 0..16 { raw[192 + s] = if s == live { 0x51 } else { 0 }; }
            let w = crate::gguf::dequantize(14, &raw, 256).unwrap();
            assert!(w.iter().enumerate().all(|(j, v)| (j / 16 == live) || *v == 0.0), "fixture: only sub-block {live} is live");
            let want: f64 = w.iter().zip(&act_rt).map(|(a, b)| *a as f64 * *b as f64).sum();
            let got = dot_q6_k(&raw, &aq) as f64;
            assert!(want.abs() > 1e-3, "sub-block {live}: a zero expectation would prove nothing");
            assert!((want - got).abs() <= 1e-5 * want.abs(), "sub-block {live}: want {want}, got {got}");
        }
    }

    #[test]
    fn q6k_negative_scales_and_truncation() {
        // scales are SIGNED int8; reading them unsigned flips the sign of half the model
        let mut raw = q6k_bytes(1, 5);
        for s in 0..16 { raw[192 + s] = 0xF0; } // -16
        let act = vec![0.5f32; 256];
        let aq = quantize_act(&act);
        let w = crate::gguf::dequantize(14, &raw, 256).unwrap();
        let want: f32 = w.iter().map(|v| v * 0.5).sum();
        let got = dot_q6_k(&raw, &aq);
        assert!(want.abs() > 1e-3 && (want - got).abs() <= 1e-4 * want.abs().max(1.0), "want {want}, got {got}");
        raw.truncate(150);
        assert_eq!(dot_q6_k(&raw, &aq), 0.0, "a truncated block stops rather than reading past the map");
    }

    /// Through the pool, as the caller will use it: a [rows, k] Q6_K matrix, every row against the
    /// per-row reference, and the type plumbing (row_bytes, from_gguf) the loader would key on.
    #[test]
    fn q6k_matvec_through_the_pool_matches_per_row() {
        assert_eq!(Quant::from_gguf(14), Some(Quant::Q6_K));
        assert_eq!(Quant::Q6_K.row_bytes(1024), 4 * 210);
        let (rows, k) = (37usize, 512usize);
        let raw = q6k_bytes(rows * k / 256, 31337);
        let act: Vec<f32> = (0..k).map(|i| ((i * 7 % 23) as f32 - 11.0) * 0.31).collect();
        let prep = ActPrep::new(&act);
        let pool = crate::cpupool::Pool::new(3);
        let got = matvec(&pool, Quant::Q6_K, &raw, rows, k, &prep);
        assert_eq!(got.len(), rows);
        let rb = Quant::Q6_K.row_bytes(k);
        for r in 0..rows {
            assert_eq!(got[r], dot_q6_k(&raw[r * rb..(r + 1) * rb], &prep.q), "row {r}");
        }
        assert!(got.iter().any(|v| *v != 0.0) && got.windows(2).any(|w| w[0] != w[1]), "rows are distinct and non-zero");
    }

    #[test]
    fn the_half_decoder_is_the_one_in_gguf() {
        let (mut differ, mut nan_pairs) = (0u32, 0u32);
        for h in 0..=u16::MAX {
            let (a, b) = (half_to_f32_branchy(h), half_to_f32(h));
            if a.to_bits() != b.to_bits() { differ += 1; if a.is_nan() && b.is_nan() { nan_pairs += 1; } }
        }
        assert_eq!(differ, nan_pairs, "a non-NaN half decodes differently");
        // every SUBNORMAL half, by value — the class the historical bug lived in
        for m in 1..0x400u16 { for sign in [0u16, 0x8000] {
            let want = (m as f32) * 2f32.powi(-24) * if sign == 0 { 1.0 } else { -1.0 };
            assert_eq!(half_to_f32(m | sign), want, "subnormal 0x{:04x}", m | sign);
        } }
        assert_eq!((half_to_f32(0x3C00), half_to_f32(0xC000), half_to_f32(0x7BFF), half_to_f32(0x0000).to_bits(), half_to_f32(0x8000).to_bits()),
                   (1.0, -2.0, 65504.0, 0, 0x8000_0000));
        assert!(half_to_f32(0x7C00).is_infinite() && half_to_f32(0x7E00).is_nan());
    }

    /// BOTH COPIES, SAME BITS. The wide copy is the same source compiled with more instructions
    /// available; if it ever differs from baseline, a feature that changes float semantics got
    /// enabled (fma), or the dispatch is calling the wrong thing. Forces each path explicitly —
    /// relying on detection alone would silently test one copy twice on a CPU without the feature.
    #[test]
    fn the_wide_copy_of_every_dot_is_bit_identical_to_baseline() {
        let blocks = 16usize; let k = blocks * 256;
        let act: Vec<f32> = (0..k).map(|i| (((i * 37 + 11) % 97) as f32 - 48.0) * 0.21).collect();
        let prep = ActPrep::new(&act);
        let q8: Vec<u8> = { let mut v = Vec::new(); let mut x = 99u32; for _ in 0..k / 32 { v.extend_from_slice(&f32_to_half(0.004).to_le_bytes()); for _ in 0..32 { x = x.wrapping_mul(1664525).wrapping_add(1013904223); v.push((x >> 24) as u8); } } v };
        let (q4, q6) = (q4k_bytes(blocks, 606), q6k_bytes(blocks, 707));
        let had_wide = set_cpu_isa_wide(Some(true));
        let wide = (dot_q8_0_iq(&q8, &prep.q), dot_q4_k(&q4, &prep.q, &prep.sums), dot_q6_k(&q6, &prep.q));
        let name_wide = cpu_isa_name();
        assert!(!set_cpu_isa_wide(Some(false)));
        let base = (dot_q8_0_iq(&q8, &prep.q), dot_q4_k(&q4, &prep.q, &prep.sums), dot_q6_k(&q6, &prep.q));
        assert_eq!(cpu_isa_name(), "baseline");
        set_cpu_isa_wide(None);
        assert_eq!((wide.0.to_bits(), wide.1.to_bits(), wide.2.to_bits()), (base.0.to_bits(), base.1.to_bits(), base.2.to_bits()));
        assert!(base.0 != 0.0 && base.1 != 0.0 && base.2 != 0.0 && base.0.is_finite() && base.1.is_finite() && base.2.is_finite());
        // and say which case this run actually covered, so a green result is not over-read
        eprintln!("isa test: wide copy {} on this CPU ({name_wide})", if had_wide { "EXERCISED" } else { "NOT AVAILABLE — baseline compared with itself" });
        #[cfg(target_arch = "x86_64")]
        assert_eq!(had_wide, std::arch::is_x86_feature_detected!("avx2"));
    }

    /// Valid-looking Q4_K rows: random quants and 6-bit scales, `d`/`dmin` set to normal f16s.
    fn q4k_bytes(blocks: usize, seed: u32) -> Vec<u8> {
        let mut x = seed | 1; let mut v = Vec::with_capacity(blocks * 144);
        for b in 0..blocks {
            v.extend_from_slice(&f32_to_half(0.003 + 0.0007 * b as f32).to_le_bytes());
            v.extend_from_slice(&f32_to_half(0.011 + 0.0003 * b as f32).to_le_bytes());
            for _ in 0..140 { x = x.wrapping_mul(1664525).wrapping_add(1013904223); v.push((x >> 24) as u8); }
        }
        v
    }

    /// The fast Q4_K dot is the old one, bit for bit — every row, several shapes, negative and
    /// zero activations — and its straight-line scale unpack is `get_scale_min_k4` for every j.
    #[test]
    fn the_q4k_dot_is_bit_identical_to_the_function_it_replaced() {
        let mut x = 12345u32;
        for (blocks, seed) in [(1usize, 3u32), (4, 77), (16, 4242), (16, 9)] {
            let raw = q4k_bytes(blocks, seed);
            let k = blocks * 256;
            for shape in 0..3 {
                let act: Vec<f32> = (0..k).map(|i| match shape { 0 => (((i * 31 + 7) % 61) as f32 - 30.0) * 0.173, 1 => if i % 5 == 0 { 0.0 } else { -((i % 13) as f32) * 0.4 }, _ => { x = x.wrapping_mul(1664525).wrapping_add(1013904223); ((x >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 9.0 } }).collect();
                let prep = ActPrep::new(&act);
                let (a, b) = (dot_q4_k_reference(&raw, &prep.q, &prep.sums), dot_q4_k(&raw, &prep.q, &prep.sums));
                assert!(a.to_bits() == b.to_bits() && a != 0.0 && a.is_finite(), "{blocks} blocks seed {seed} shape {shape}: {a} vs {b}");
            }
            for sb in raw.chunks_exact(144) {
                let (sc, mn) = scales_k4(sb[4..16].try_into().unwrap());
                for j in 0..8 { assert_eq!(crate::gguf::get_scale_min_k4(j, &sb[4..16]), (sc[j], mn[j]), "j={j}"); }
            }
        }
        // every 6-bit value reaches every slot: the two unpack branches (j<4, j>=4) each see all 64
        for v in 0..64u8 { for j in 0..8usize {
            let mut q = [0u8; 12];
            if j < 4 { q[j] = v; q[j + 4] = 63 - v; } else { q[j + 4] = (v & 0xF) | (((63 - v) & 0xF) << 4); q[j - 4] = (v >> 4) << 6; q[j] = ((63 - v) >> 4) << 6; }
            let (sc, mn) = scales_k4(&q);
            assert_eq!((sc[j], mn[j]), (v, 63 - v), "slot {j} value {v}");
            assert_eq!(crate::gguf::get_scale_min_k4(j, &q), (v, 63 - v));
        } }
        // truncated raw: both stop at the last whole super-block
        let raw = q4k_bytes(4, 5); let act = vec![0.5f32; 1024]; let prep = ActPrep::new(&act);
        assert_eq!(dot_q4_k(&raw[..400], &prep.q, &prep.sums).to_bits(), dot_q4_k_reference(&raw[..400], &prep.q, &prep.sums).to_bits());
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
    fn a_layer_range_past_the_end_is_None_not_a_clamped_repeat() {
        // A clamped range would prefetch the last layer over and over — wasted syscalls that look like
        // progress, and the kind of thing that shows up as "the prefetch is enabled" while doing
        // nothing useful.
        let raw = vec![0u8; 1000];
        assert_eq!(layer_range(&raw, 0, 300).map(|s| s.len()), Some(300));
        assert_eq!(layer_range(&raw, 3, 300).map(|s| s.len()), Some(100), "last layer is short");
        assert!(layer_range(&raw, 4, 300).is_none(), "past the end is None");
        assert!(layer_range(&raw, usize::MAX, 300).is_none(), "and the multiply cannot overflow");
    }

    #[test]
    fn an_unsupported_quant_returns_None_rather_than_guessing() {
        // A wrong guess would read a Q6_K tensor as Q4_K and produce plausible garbage — the exact
        // failure this project keeps paying for. Unsupported means fall back to the GPU, not improvise.
        assert_eq!(Quant::from_gguf(8), Some(Quant::Q8_0));
        assert_eq!(Quant::from_gguf(12), Some(Quant::Q4_K));
        // 14 (Q6_K) sat in the None list until dot_q6_k existed. It left the list WITH its kernel and
        // an oracle test, which is the only way a type should ever leave it. Q5_K (13) and Q3_K (11)
        // are what a Q4_K_S / Q3_K_M file would add next; they are still refused.
        assert_eq!(Quant::from_gguf(14), Some(Quant::Q6_K));
        for t in [13u32, 11, 10, 19, 0, 1, 99] { assert_eq!(Quant::from_gguf(t), None, "type {t}"); }
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

/// Dot one IQ1_S super-block row against a prepared f32 activation, reading the source bytes directly.
///
/// ## Why this exists
///
/// GLM-5.2 ships as IQ1_S and the loader converts every source format to f32 and then to q4 — a 20x
/// transient and a 2.8x resident inflation, which is what puts a 25 GiB checkpoint over a 63 GiB
/// ceiling. A path that reads the source layout removes both costs, because the weights never leave
/// the size the checkpoint author chose.
///
/// This is the CPU half: usable on its own, and the reference a GPU kernel's gate would compare
/// against. Written first deliberately — `gate-validity.md` records four faults found and fixed against
/// a software rasterizer on a kernel that was still wrong on hardware, so the reference is the part
/// that can be trusted from here.
///
/// ## The layout
///
/// 50 bytes per 256 weights: `d` as f16, 32 bytes of `qs`, then 8 `u16` of `qh`. Each group of 8
/// weights takes an **11-bit** codebook index — 8 bits from `qs`, 3 more from `qh` — into the 2048
/// ternary entries of `iq1s_grid`. Per 32-weight sub-block `qh` also carries a 3-bit scale
/// (`dl = d·(2s+1)`) and a sign bit selecting `±IQ1S_DELTA`.
///
/// Activations stay f32 rather than being quantized: the weights are ternary, so the product is a
/// select-and-add and there is nothing for an integer path to accelerate. That is the opposite of the
/// Q8_0 case, where the integer dot was worth 2x.
pub fn dot_iq1_s(raw: &[u8], act: &[f32]) -> Option<f32> {
    let nblk = act.len() / 256;
    if raw.len() < nblk * 50 { return None; }
    let mut total = 0f32;
    for b in 0..nblk {
        let blk = &raw[b * 50..(b + 1) * 50];
        let d = crate::gguf::f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
        let qs = &blk[2..34];
        for ib in 0..8 {
            let h = u16::from_le_bytes([blk[34 + 2 * ib], blk[35 + 2 * ib]]);
            let dl = d * (2 * ((h >> 12) & 7) + 1) as f32;
            let delta = if h & 0x8000 != 0 { -crate::iq1s_grid::IQ1S_DELTA }
                        else { crate::iq1s_grid::IQ1S_DELTA };
            // Four groups of 8 weights per 32-wide sub-block.
            let mut sub = 0f32;
            for g in 0..4 {
                let qi = ib * 4 + g;
                // 11 bits: 8 from qs, 3 from qh at a per-group offset.
                let idx = (qs[qi] as usize) | ((((h >> (3 * g)) & 7) as usize) << 8);
                // `entry` returns the 8 ternary values already unpacked. I first wrote a 2-bit
                // unpack of a raw grid word — a layout I INVENTED rather than read, and the test
                // caught it at 56.8 against -6.9. Using the same accessor the dequantizer uses is
                // what makes this a reading of the format rather than a second guess at it.
                let gr = crate::iq1s_grid::entry(idx);
                let base = b * 256 + ib * 32 + g * 8;
                for j in 0..8 {
                    sub += act[base + j] * (gr[j] as f32 + delta);
                }
            }
            total += dl * sub;
        }
    }
    Some(total)
}

#[cfg(test)]
mod iq1s_dot_tests {
    use super::*;

    /// The dot must agree with dequantize-then-multiply.
    ///
    /// That equivalence is the whole claim: reading the source layout directly is only worth doing if
    /// it produces the same answer as the path it replaces. Compared against `gguf::dequantize`, which
    /// is itself the transcribed llama.cpp routine — so this checks the reading, not the codebook.
    #[test]
    fn it_matches_dequantize_then_dot() {
        let nblk = 3usize;
        // Plausible bytes: a real f16 scale, and qs/qh patterns that exercise several codebook entries.
        let mut raw = vec![0u8; nblk * 50];
        for b in 0..nblk {
            let blk = &mut raw[b * 50..(b + 1) * 50];
            blk[0..2].copy_from_slice(&0x2E66u16.to_le_bytes());   // ~0.1 in f16
            for i in 0..32 { blk[2 + i] = ((i * 7 + b * 13) % 251) as u8; }
            for i in 0..8 {
                let h = ((i as u16 * 4099) & 0x7FFF) | if i % 3 == 0 { 0x8000 } else { 0 };
                blk[34 + 2 * i..36 + 2 * i].copy_from_slice(&h.to_le_bytes());
            }
        }
        let act: Vec<f32> = (0..nblk * 256).map(|i| ((i % 19) as f32 - 9.0) * 0.11).collect();

        let w = crate::gguf::dequantize(19, &raw, nblk * 256).expect("iq1_s dequantizes");
        let want: f32 = w.iter().zip(&act).map(|(a, b)| a * b).sum();
        let got = dot_iq1_s(&raw, &act).expect("shapes agree");

        let tol = want.abs().max(1.0) * 1e-4;
        assert!((got - want).abs() <= tol,
            "direct read {got} vs dequantize-then-dot {want} (tol {tol})");
    }

    #[test]
    fn a_short_source_returns_None_rather_than_reading_past_it() {
        // A truncated tensor would otherwise index out of bounds or, worse, read adjacent weights and
        // return a plausible number — the failure this codebase keeps paying for.
        assert!(dot_iq1_s(&[0u8; 49], &vec![0f32; 256]).is_none());
        assert!(dot_iq1_s(&[0u8; 50], &vec![0f32; 256]).is_some());
    }

    #[test]
    fn iq1s_row_slices_of_a_stack_recover_the_independent_rows() {
        // The MoE arm hands MOE_MATVEC_IQ1_S per-expert byte ranges computed by
        // Iq1Weight::row_bytes — the arithmetic that shipped broken three separate ways. This
        // pins it against ground truth the offsets cannot fake: rows are generated
        // INDEPENDENTLY, dotted via the reference, then concatenated into the stacked layout the
        // loader uploads; slicing the stack at row r must reproduce row r's own dot exactly.
        let k = 512usize; // two blocks per row — exercises the per-row block stride too
        let rb = crate::Iq1Weight::row_bytes(k).unwrap();
        assert_eq!(rb, 2 * 50);
        // deterministic pseudo-random block bytes (SplitMix-ish), valid by construction: every
        // byte pattern decodes — the grid is total over its 11-bit index space.
        let mut s = 0x9e3779b97f4a7c15u64;
        let mut byte = || { s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (s >> 33) as u8 };
        let rows: Vec<Vec<u8>> = (0..3).map(|_| (0..rb).map(|_| byte()).collect()).collect();
        let act: Vec<f32> = (0..k).map(|i| (i as f32 * 0.37).sin()).collect();
        let expect: Vec<f32> = rows.iter().map(|r| dot_iq1_s(r, &act).unwrap()).collect();
        let stack: Vec<u8> = rows.concat();
        for (r, want) in expect.iter().enumerate() {
            let got = dot_iq1_s(&stack[r * rb..(r + 1) * rb], &act).unwrap();
            assert_eq!(got.to_bits(), want.to_bits(), "row {r}: slice must be BIT-identical");
        }
        // and the alignment gate refuses the row width ggml itself cannot quantize (the
        // DiffusionGemma expert down-proj width) instead of cutting mid-block
        assert!(crate::Iq1Weight::row_bytes(704).is_none());
    }
}
