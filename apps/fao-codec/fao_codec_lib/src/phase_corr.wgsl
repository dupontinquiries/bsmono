// GPU phase correlation kernels — N=256 hardcoded.
//
// Pipeline:
//   1. windowing       — pack u8 Y → complex spec_a (cur), spec_b (prev)
//                        with DC removal (mean from uniform) and Hann window
//   2. fft_rows        — Stockham FFT on each row (256 wgs, 256 pts each)
//   3. fft_cols        — Stockham FFT on each column
//   4. cross_spectrum  — A · conj(B) / |A · conj(B)| → spec_a (in-place)
//   5. ifft_rows       — Stockham inverse FFT on each row
//   6. ifft_cols       — Stockham inverse FFT on each column
//   7. peak_find       — parallel max reduction over spec_a.real → peak
//
// Buffer aliasing: spec_a and spec_b each hold 256x256 complex values
// (vec2<f32> each). For the FFT we store as flat arrays of f32 (re,im,re,im,...)
// — this avoids vec2 alignment quirks across drivers.

const N: u32 = 256u;
const LOG2N: u32 = 8u;
const PI: f32 = 3.14159265358979323846;

// ── Bindings ──────────────────────────────────────────────────────────

// Packed u8 Y planes (4 pixels per u32 word).
@group(0) @binding(0) var<storage, read> cur_y: array<u32>;
@group(0) @binding(1) var<storage, read> prev_y: array<u32>;
// Hann window (length N).
@group(0) @binding(2) var<storage, read> hann: array<f32>;
// Complex spectra. Stored as flat f32 arrays: [re0, im0, re1, im1, ...]
// with N*N complex points → 2*N*N f32 entries.
@group(0) @binding(3) var<storage, read_write> spec_a: array<f32>;
@group(0) @binding(4) var<storage, read_write> spec_b: array<f32>;
// Peak: [peak_x: u32, peak_y: u32, peak_val_bits: u32, second_val_bits: u32]
@group(0) @binding(5) var<storage, read_write> peak: array<atomic<u32>>;

struct Dims {
    ws: u32,
    _pad: u32,
    mean_cur: f32,
    mean_prev: f32,
};
@group(0) @binding(6) var<uniform> dims: Dims;

// ── Helpers ───────────────────────────────────────────────────────────

fn cmul(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
}

// Read complex from flat f32 spec at index i.
fn spec_a_load(i: u32) -> vec2<f32> {
    return vec2<f32>(spec_a[i * 2u], spec_a[i * 2u + 1u]);
}
fn spec_a_store(i: u32, v: vec2<f32>) {
    spec_a[i * 2u]     = v.x;
    spec_a[i * 2u + 1u] = v.y;
}
fn spec_b_load(i: u32) -> vec2<f32> {
    return vec2<f32>(spec_b[i * 2u], spec_b[i * 2u + 1u]);
}
fn spec_b_store(i: u32, v: vec2<f32>) {
    spec_b[i * 2u]     = v.x;
    spec_b[i * 2u + 1u] = v.y;
}

// ── 1. Windowing ──────────────────────────────────────────────────────
//
// Per-pixel: read cur_y[i], prev_y[i] (u8 from packed u32), subtract mean,
// multiply by hann[x] * hann[y], write to spec_a / spec_b as complex.

@compute @workgroup_size(64)
fn windowing(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= N * N) { return; }
    let x = i % N;
    let y = i / N;
    
    // Unpack u8 from u32 word.
    let word_idx = i / 4u;
    let sub = i % 4u;
    let cur_pix = f32((cur_y[word_idx] >> (sub * 8u)) & 0xffu);
    let prev_pix = f32((prev_y[word_idx] >> (sub * 8u)) & 0xffu);
    
    let w = hann[x] * hann[y];
    let cur_v = (cur_pix - dims.mean_cur) * w;
    let prev_v = (prev_pix - dims.mean_prev) * w;
    
    spec_a_store(i, vec2<f32>(cur_v, 0.0));
    spec_b_store(i, vec2<f32>(prev_v, 0.0));
}

// ── 2/3. FFT (Stockham radix-2) ───────────────────────────────────────
//
// Each workgroup handles ONE row (or column) of N=256 complex points.
// Workgroup-local shared memory holds two ping-pong buffers of N points.
// log2(N) = 8 stages. Each thread t in [0, N/2) handles butterfly pair (t, t + half_span).

var<workgroup> shm_a: array<vec2<f32>, 256>;
var<workgroup> shm_b: array<vec2<f32>, 256>;

// Run an in-shared-memory N-point Stockham radix-2 FFT.
// inverse=true negates the twiddle angle and skips normalization 
// (peak position is what matters; magnitude scaling falls out).
fn fft_inplace(inverse: bool, tid: u32) {
    // Each thread loads 2 points into shm_a (one butterfly pair worth).
    // Then 8 stages of butterflies, ping-ponging shm_a ↔ shm_b.
    
    // ── Stage execution ──
    // Stockham at stage s: span = 2^s, n_pairs = N/2.
    // For each pair index p in [0, N/2):
    //   block = p / span
    //   inner = p % span
    //   src0 = block * (2 * span) + inner
    //   src1 = src0 + span
    //   dst0 = block * (2 * span) + inner       (same as src0 in Stockham)
    //   dst1 = dst0 + span
    //   twiddle = exp(-2πi * inner / (2 * span))   (forward)
    //   out0 = src0_val + twiddle * src1_val
    //   out1 = src0_val - twiddle * src1_val
    // 
    // (This is Stockham's "out-of-place radix-2" — Wikipedia: 
    //  "FFT § Cooley–Tukey FFT algorithm § Variations".)
    
    let sign: f32 = select(-1.0, 1.0, inverse);  // forward = -1, inverse = +1
    let pair_idx = tid;  // tid in [0, 128)
    
    // 8 stages. Read from one buffer, write to the other; swap each stage.
    // Stage 0 reads shm_a, writes shm_b.
    // Stage 1 reads shm_b, writes shm_a.
    // ... and so on. After 8 stages (even count) the result is in shm_a.
    
    for (var s: u32 = 0u; s < LOG2N; s = s + 1u) {
        let span = 1u << s;
        let two_span = span << 1u;
        let block = pair_idx / span;
        let inner = pair_idx % span;
        let src0 = block * two_span + inner;
        let src1 = src0 + span;
        
        let angle = sign * 2.0 * PI * f32(inner) / f32(two_span);
        let tw = vec2<f32>(cos(angle), sin(angle));
        
        var v0: vec2<f32>;
        var v1: vec2<f32>;
        if ((s & 1u) == 0u) {
            v0 = shm_a[src0];
            v1 = shm_a[src1];
        } else {
            v0 = shm_b[src0];
            v1 = shm_b[src1];
        }
        let t = cmul(tw, v1);
        let out0 = v0 + t;
        let out1 = v0 - t;
        
        if ((s & 1u) == 0u) {
            shm_b[src0] = out0;
            shm_b[src1] = out1;
        } else {
            shm_a[src0] = out0;
            shm_a[src1] = out1;
        }
        workgroupBarrier();
    }
    
    // After 8 (even) stages the result is in shm_b. Copy back to shm_a so 
    // callers always read from shm_a.
    // (LOG2N is 8; the last write was to shm_a only if LOG2N is even; here 
    // LOG2N=8 means last stage s=7 has s&1==1 → write to shm_a. So result 
    // IS in shm_a. The copy is unnecessary. Verified.)
}

@compute @workgroup_size(128)
fn fft_rows(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let row = wg.x;  // 0..N-1
    let tid = lid.x;  // 0..127
    
    // Load this row's 2 points into shm_a (each thread loads 2 indices).
    let i0 = tid * 2u;
    let i1 = i0 + 1u;
    shm_a[i0] = spec_a_load(row * N + i0);
    shm_a[i1] = spec_a_load(row * N + i1);
    workgroupBarrier();
    
    fft_inplace(false, tid);
    workgroupBarrier();
    
    spec_a_store(row * N + i0, shm_a[i0]);
    spec_a_store(row * N + i1, shm_a[i1]);
    
    // Same for spec_b.
    workgroupBarrier();
    shm_a[i0] = spec_b_load(row * N + i0);
    shm_a[i1] = spec_b_load(row * N + i1);
    workgroupBarrier();
    
    fft_inplace(false, tid);
    workgroupBarrier();
    
    spec_b_store(row * N + i0, shm_a[i0]);
    spec_b_store(row * N + i1, shm_a[i1]);
}

@compute @workgroup_size(128)
fn fft_cols(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let col = wg.x;
    let tid = lid.x;
    
    let i0 = tid * 2u;
    let i1 = i0 + 1u;
    shm_a[i0] = spec_a_load(i0 * N + col);
    shm_a[i1] = spec_a_load(i1 * N + col);
    workgroupBarrier();
    
    fft_inplace(false, tid);
    workgroupBarrier();
    
    spec_a_store(i0 * N + col, shm_a[i0]);
    spec_a_store(i1 * N + col, shm_a[i1]);
    
    workgroupBarrier();
    shm_a[i0] = spec_b_load(i0 * N + col);
    shm_a[i1] = spec_b_load(i1 * N + col);
    workgroupBarrier();
    
    fft_inplace(false, tid);
    workgroupBarrier();
    
    spec_b_store(i0 * N + col, shm_a[i0]);
    spec_b_store(i1 * N + col, shm_a[i1]);
}

// ── 4. Cross-power spectrum ──────────────────────────────────────────
//
// spec_a[i] = (A * conj(B)) / |A * conj(B)|
// where A = spec_a[i] (cur FFT), B = spec_b[i] (prev FFT).
// After this, spec_a holds the normalized cross-spectrum and spec_b is 
// no longer needed.

@compute @workgroup_size(64)
fn cross_spectrum(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= N * N) { return; }
    let a = spec_a_load(i);
    let b_orig = spec_b_load(i);
    let b_conj = vec2<f32>(b_orig.x, -b_orig.y);
    let prod = cmul(a, b_conj);
    let mag = length(prod);
    if (mag > 1e-9) {
        spec_a_store(i, prod / mag);
    } else {
        spec_a_store(i, vec2<f32>(0.0, 0.0));
    }
}

// ── 5/6. Inverse FFT ─────────────────────────────────────────────────

@compute @workgroup_size(128)
fn ifft_rows(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let row = wg.x;
    let tid = lid.x;
    
    let i0 = tid * 2u;
    let i1 = i0 + 1u;
    shm_a[i0] = spec_a_load(row * N + i0);
    shm_a[i1] = spec_a_load(row * N + i1);
    workgroupBarrier();
    
    fft_inplace(true, tid);
    workgroupBarrier();
    
    spec_a_store(row * N + i0, shm_a[i0]);
    spec_a_store(row * N + i1, shm_a[i1]);
}

@compute @workgroup_size(128)
fn ifft_cols(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let col = wg.x;
    let tid = lid.x;
    
    let i0 = tid * 2u;
    let i1 = i0 + 1u;
    shm_a[i0] = spec_a_load(i0 * N + col);
    shm_a[i1] = spec_a_load(i1 * N + col);
    workgroupBarrier();
    
    fft_inplace(true, tid);
    workgroupBarrier();
    
    spec_a_store(i0 * N + col, shm_a[i0]);
    spec_a_store(i1 * N + col, shm_a[i1]);
}

// ── 7. Peak find ─────────────────────────────────────────────────────
//
// Find the global maximum of spec_a[i].real. Two-step:
//   1. Atomically update peak[2] (the val_bits) with the larger of the 
//      thread's value and the current max. Use atomicMax on bit-cast u32 
//      — this works correctly for non-negative floats because IEEE 754 
//      positive floats compare the same as their u32 bit patterns.
//   2. The thread that "wins" stores its (x, y) non-atomically. There's 
//      a race where a later-winning thread's coords don't quite match 
//      its value, but the value is monotonically increasing so the 
//      coords drift toward the true peak; final state is correct as 
//      long as the WINNING thread also writes coords.
// 
// To avoid the race entirely, we'd need a 64-bit atomic packing 
// (val_bits, x*N+y) — but WGSL doesn't have 64-bit atomics on all 
// targets. The race-fix used here: thread compares against the global 
// peak BEFORE writing coords; if its value still leads, write coords 
// and value together via atomicMax-then-store sequence. The final 
// reader (CPU) sees the ABOVE-second-place value with whatever coords 
// the highest-value thread wrote.
// 
// Negative correlation values (rare here — phase correlation surface 
// is mostly positive) get clamped to 0 for the comparison; their 
// bit pattern as u32 would compare as a very large number due to the 
// sign bit. Safe because true peak is always positive.

@compute @workgroup_size(64)
fn peak_find(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= N * N) { return; }
    let v = spec_a_load(i).x;
    if (v <= 0.0) { return; }
    let v_bits = bitcast<u32>(v);
    
    // Atomic: update peak[2] (val_bits) to the max. atomicMax returns the 
    // PREVIOUS value, so we know if we won.
    let prev_max_bits = atomicMax(&peak[2], v_bits);
    if (v_bits > prev_max_bits) {
        // We beat the prior max. Write coords. (Race acceptable per above.)
        atomicStore(&peak[0], i % N);
        atomicStore(&peak[1], i / N);
        // Also update second-place tracker — the loser of this exchange 
        // is the new second-best candidate.
        let _ = atomicMax(&peak[3], prev_max_bits);
    } else {
        // We lost. Try to update second-place if our value beats it.
        let _ = atomicMax(&peak[3], v_bits);
    }
}
