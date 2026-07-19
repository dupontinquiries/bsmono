//! Transformer GPU kernels on wgpu (Vulkan on Asahi/Linux). Each kernel is a
//! WGSL compute shader dispatched through one shared helper; correctness is
//! checked against CPU references in the validator binary. These are the ops a
//! wgpu/Burn inference backend composes: linear (nn.Linear with transposed
//! weights), Q4_0 dequant, row softmax, RMSNorm, and scaled-dot-product attention.
use wgpu::util::DeviceExt;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;

pub mod gguf;
pub mod model_kind; // classify a gguf (LLM / diffusion / vision-projector) before the LLM loader tries it
#[allow(dead_code)] pub mod memplan; // scratchwork: footprint + memory-fit planner for bigger models
#[allow(dead_code)] pub mod zerocopy; // scoped experiment: UMA zero-copy weight import (gated `zerocopy`)
#[allow(dead_code)] pub mod oom; // detect GPU allocation OOM → downgrade residency instead of crashing
pub mod wgsl_validate; // in-process WGSL fitness gate (naga parse + validate)
#[allow(dead_code)] pub mod sparse; // sparse-residency planner (+ gated binder) — the arena's architectural replacement
#[allow(dead_code)] pub mod archcfg; // scratchwork: multi-architecture config registry (qwen2/gemma2…)
#[allow(dead_code)] pub mod moe;     // mixture-of-experts routing (Qwen4/Llama4 decode): top-k + weights
pub mod netserve;
pub mod store;

/// Memory-map a GGUF read-only. Big models (30B/70B) are tens of GB — `std::fs::read` would pull the
/// whole file into a RAM `Vec` (the 40 GB read that got OOM-killed before dequant even started).
/// mmap lets the OS page weight bytes in on demand: only the int8-resident tensors + transient
/// per-tensor f32 stay in RAM, and a content-key sample (head/mid/tail) faults a few pages, not GB.
/// The returned `Mmap` derefs to `&[u8]` — pass it anywhere bytes are taken. Unsafe because the file
/// could be mutated underneath us; for immutable model files that's fine.
/// File bytes for loading: normally an mmap (pages in on demand), but LOGSCOPE_NO_MMAP=1 reads the whole
/// file into RAM instead. Escape hatch for volumes where mmap returns ZEROS while read() returns real data
/// (FUSE-backed mounts — exFAT/NTFS drivers, cloud-sync mounts — and some network filesystems don't
/// implement mmap coherence; the checksum looks fine because checksums use read()). Costs file-size RAM
/// transiently; on unified-memory boxes with headroom that's an acceptable diagnosis-and-workaround mode.
pub enum FileBytes { Mapped(memmap2::Mmap), Owned(Vec<u8>) }
impl std::ops::Deref for FileBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] { match self { FileBytes::Mapped(m) => m, FileBytes::Owned(v) => v } }
}
/// Open model bytes honoring LOGSCOPE_NO_MMAP (see FileBytes).
pub fn file_bytes(path: &str) -> std::io::Result<FileBytes> {
    if std::env::var("LOGSCOPE_NO_MMAP").is_ok() {
        eprintln!("wgpu-engine: LOGSCOPE_NO_MMAP — reading the whole file via read() (mmap bypass)");
        return Ok(FileBytes::Owned(std::fs::read(path)?));
    }
    Ok(FileBytes::Mapped(mmap_ro(path)?))
}

pub fn mmap_ro(path: &str) -> std::io::Result<memmap2::Mmap> {
    let f = std::fs::File::open(path)?;
    unsafe { memmap2::Mmap::map(&f) }
}
pub mod tokenizer;
pub mod toolpipe;

const TILE: u32 = 16;

/// Profiling counters: every `queue.submit` (one CPU→GPU command flush) and every blocking
/// device poll (a full GPU sync that stalls the CPU). Decode throughput is dominated by the
/// *count* of these, not by compute, since each decode op is a tiny 1-row kernel. Read via
/// `gpu_submits()` / `gpu_polls()` around a decode to see the per-token structural cost.
pub static GPU_SUBMITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static GPU_POLLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Cumulative CPU nanoseconds spent (a) creating per-dispatch resources — the dims uniform
/// buffer + bind group, which `dispatch` rebuilds every call — and (b) blocked in the GPU
/// sync poll. Splitting these tells whether decode is CPU-bound on resource churn (→ cache
/// bind groups / use push constants) or GPU-bound (→ fuse kernels / fewer dispatches).
pub static GPU_SETUP_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static GPU_SYNC_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Dims-uniform cache accounting: a MISS is a real `create_buffer_init` (an upload); a HIT reuses a
/// cached buffer (the eliminated upload). Hits/(hits+misses) is the upload-churn reduction — a
/// hardware-INDEPENDENT structural number, valid here on lavapipe.
pub static DIMS_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static DIMS_MISSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn dims_cache_hits() -> u64 { DIMS_HITS.load(std::sync::atomic::Ordering::Relaxed) }
pub fn dims_cache_misses() -> u64 { DIMS_MISSES.load(std::sync::atomic::Ordering::Relaxed) }
/// Buffer-pool accounting: a MISS is a real `create_buffer` (allocation churn — the other half of
/// "upload locking"); a HIT recycles a same-size buffer freed by a dropped Tensor. Hardware-independent.
pub static POOL_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static POOL_MISSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn pool_hits() -> u64 { POOL_HITS.load(std::sync::atomic::Ordering::Relaxed) }
pub fn pool_misses() -> u64 { POOL_MISSES.load(std::sync::atomic::Ordering::Relaxed) }
pub fn gpu_submits() -> u64 { GPU_SUBMITS.load(std::sync::atomic::Ordering::Relaxed) }
pub fn gpu_polls() -> u64 { GPU_POLLS.load(std::sync::atomic::Ordering::Relaxed) }
pub fn gpu_setup_ns() -> u64 { GPU_SETUP_NS.load(std::sync::atomic::Ordering::Relaxed) }
pub fn gpu_sync_ns() -> u64 { GPU_SYNC_NS.load(std::sync::atomic::Ordering::Relaxed) }
pub fn gpu_counters_reset() { for c in [&GPU_SUBMITS, &GPU_POLLS, &GPU_SETUP_NS, &GPU_SYNC_NS, &DIMS_HITS, &DIMS_MISSES, &POOL_HITS, &POOL_MISSES] { c.store(0, std::sync::atomic::Ordering::Relaxed); } }

/// Cached per-shader GPU objects: the bind-group layout and compiled compute
/// pipeline. Building these is expensive (shader compilation), so they're created
/// once per kernel and reused — without this, a full model does hundreds of
/// pipeline rebuilds per token and decode is unusably slow.
type Pipe = Arc<(wgpu::BindGroupLayout, wgpu::ComputePipeline)>;

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pub adapter_name: String,
    pipelines: Mutex<HashMap<(usize, u32), Pipe>>,
    /// When `Some`, ops record into this one command encoder instead of each submitting
    /// its own. A whole decode token's ~250 ops then flush in ONE submit (see `begin_frame`/
    /// `end_frame`). `keep` holds the per-op bind groups + dims buffers alive until the flush.
    frame: Mutex<Option<Frame>>,
    /// Index into `DECODE_PROFILES` — the ONE system profile this GPU resolved to (via `autotune_matvec`
    /// on first model load, or pinned by env). The decode hot path does a single profile lookup; new
    /// tuning dimensions are added as fields on `DecodeConfig` + rows in the matrix, never as fresh
    /// hot-path knobs. `profile_forced` is set when an env var pins it, which skips autotuning.
    profile: std::sync::atomic::AtomicU32,
    profile_forced: bool,
    /// Per-SHAPE decode-tile overrides, populated by `autotune_shapes` at model load and reusable across
    /// launches (persist with save/load_autotune) and across MODELS (keyed by (n,k), not by model — two
    /// models sharing an FFN shape share its tuned tile). The global `profile` is the fallback; this map
    /// refines it per shape, because the best tile scales with k (activation-reuse), so a 14B's wide
    /// FFN-down wants a larger tile than its attention projections. Reads on the hot path; writes at load.
    shape_tiles: std::sync::RwLock<std::collections::HashMap<(u32, u32), u32>>,
    /// Per-shape PREFILL GEMM workgroup tile (override-constant specialization), autotuned + persistable
    /// like `shape_tiles`. Separate map because decode (mat-vec) and prefill (GEMM) have different optima.
    gemm_tiles: std::sync::RwLock<std::collections::HashMap<(u32, u32), u32>>,
    /// Max compute passes batched into one GPU submit (the submit-overhead knob). A 14B decode records
    /// ~768 passes; at 32/submit that's ~24 submits/token, and submit/sync latency dominated the profile.
    /// Raising this cuts submits/token (less overhead) but lengthens each submit (a shared GPU's display
    /// can stutter); `SubmitTuner` adapts it on live tokens. Tunable, env-pinnable (LOGSCOPE_FLUSH_EVERY).
    flush_every: std::sync::atomic::AtomicU32,
    /// SUBMIT GOVERNOR — bound how many submits may be in flight at once (0 = off). flush_every bounds
    /// each submit's *duration*; this bounds queue *depth*. Without it a decode token fires its ~24
    /// submits back-to-back and the queue holds hundreds of ms of compute — on a shared GPU the
    /// compositor's tiny flip job then waits behind ALL of it (the "frame pacing stutters while a model
    /// runs" symptom: submit-bound, not bandwidth/compute-bound). Capping in-flight to 2 keeps the GPU
    /// saturated (one executing + one queued) while guaranteeing display work is never more than ~2
    /// submit-durations away from the front of the queue. Env `LOGSCOPE_SUBMIT_INFLIGHT` (default 2,
    /// 0 disables). Purely a pacing change: grouping and order are untouched, so output is bit-exact.
    inflight_cap: std::sync::atomic::AtomicU32,
    /// Submits handed to the queue whose completion callback hasn't fired yet (governor bookkeeping).
    pending_submits: std::sync::Arc<std::sync::atomic::AtomicU32>,
    /// One-shot escape: if completion callbacks ever stop arriving (a driver quirk), the governor
    /// disables itself instead of spinning forever, and says so once.
    governor_dead: std::sync::atomic::AtomicBool,
    /// Content-keyed cache of the small per-dispatch `dims` UNIFORM buffers. The dims for a given op
    /// (m,k,n,grid,…) are byte-identical every token, but `dispatch_with` used to `create_buffer_init`
    /// a fresh one per dispatch — ~250–380 tiny buffer creations+uploads per token (the "upload locking"
    /// that bottlenecks a high-bandwidth UMA GPU like the M2, where the kernel itself isn't the limiter).
    /// Caching by byte content makes it a one-time cost: after warmup the bandwidth-heavy linears reuse
    /// their dims buffer instead of re-uploading. Capped (no LRU): the recurring stable dims are all
    /// inserted during warmup and hit forever; per-position (attention/RoPE) dims that vary each token
    /// fill the rest and, once full, fall back to create-without-cache (i.e. never worse than before).
    /// Bit-exact: identical bytes, identical binding — every `--gate-*` is unchanged.
    dims_cache: Mutex<HashMap<Vec<u8>, std::sync::Arc<wgpu::Buffer>>>,
    /// Free-list recycling per-op output buffers across ops/tokens (see BufferPool).
    pool: std::sync::Arc<BufferPool>,
    arena: Mutex<Option<ResidentArena>>,
    subgroups: bool, // adapter supports subgroup arithmetic (subgroupAdd) — enables the sg reduction kernel
    unified: bool,   // unified-memory GPU (Apple/Asahi): host==device RAM, so weight uploads go zero-copy
                     // (map-at-creation) instead of through the staging belt — see zerocopy::upload_path
    sg_ok: std::sync::atomic::AtomicBool, // sg kernel passed the load-time self-check on THIS driver
    gemm_ok: std::sync::atomic::AtomicBool, // tiled prefill GEMM compiles + matches naive on THIS backend
    valid_gemm_tiles: std::sync::RwLock<Vec<u32>>, // GEMM tiles whose shader actually compiles here (load probe)
    /// NEOX (split-half) RoPE pairing for the CURRENTLY loaded model (qwen/gemma/phi families);
    /// false = llama-style adjacent pairs. Set at model load from the arch config; lives on Gpu to avoid
    /// threading one flag through ~20 rope call sites. (Single-resident-model assumption, like the arena.)
    pub rope_neox: std::sync::atomic::AtomicBool,
    /// YaRN attention scale (f32 bits; 1.0 = off), set by the loader when the GGUF declares
    /// rope.scaling.type = "yarn". Applied inside every rope host fn (multiplies the rotated
    /// output — the ggml formulation: both q and k scaled, so scores scale by mscale²). Gpu-level
    /// state like `rope_neox` above, so the 13 rope call sites need no signature change.
    pub rope_mscale: std::sync::atomic::AtomicU32,
    /// Vulkan-on-Apple (Asahi/Honeykrisp): the platform with the low BO-VMA ceiling; scopes the
    /// conservative GPU-buffer budget clamp to where it's actually needed.
    pub conservative_vma: bool,
}

/// One coherent decode-kernel configuration. Extend with new tuning dimensions (e.g. split-k warp
/// width) as fields here — the autotuner sweeps the whole matrix and the hot path stays a single
/// profile lookup, so optimizations never accrete as scattered branches on the decode path.
/// Which decode mat-vec kernel a profile uses. The autotuner measures all of these per shape and keeps the
/// fastest ON THIS GPU — so the optimum (which differs by platform: wg128@4 won on M2 Ultra, but another
/// GPU may prefer v2@8 or the plain v1) is discovered at runtime, not hardcoded.
///
/// ASAHI/AGX GROUNDING (Mesa Honeykrisp Vulkan, from Apple-GPU compute docs + ggml-metal practice): the
/// AGX SIMD group is 32 threads (the true execution unit — think in subgroups, not threads), and each GPU
/// core has ~32 KB of threadgroup memory. A workgroup that reserves a large threadgroup-memory reduction
/// buffer PINS occupancy to few concurrent workgroups per core (asking for the full 32 KB → exactly one),
/// which on a memory-LATENCY-bound decode matvec is the dominant cost. Implication for the next tuning pass:
/// the SG kernel (`V2WgSg`, native `subgroupAdd`, minimal/zero threadgroup memory) should out-occupy the
/// `red[tile*128]` barrier-tree kernels on Asahi even though wg128 won on M2 Ultra under Metal — worth
/// re-benching SG@2/4 as the Honeykrisp default, and keeping any threadgroup reduction buffer as small as
/// possible. The ggml-metal analogue is "rows per SIMD group": one 32-wide subgroup cooperatively reduces a
/// column-tile with no threadgroup memory at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatvecKernel { V1 = 0, V2 = 1, V2Wg128 = 2, V2WgSg = 3 }
impl MatvecKernel {
    fn from_id(id: u32) -> MatvecKernel { match id { 0 => MatvecKernel::V1, 2 => MatvecKernel::V2Wg128, 3 => MatvecKernel::V2WgSg, _ => MatvecKernel::V2 } }
    pub fn id(self) -> u32 { self as u32 }
    pub fn label(self) -> &'static str { match self { MatvecKernel::V1 => "v1", MatvecKernel::V2 => "v2", MatvecKernel::V2Wg128 => "v2-wg128", MatvecKernel::V2WgSg => "v2-wg128-sg" } }
    /// Whether this kernel needs the SUBGROUP device feature (so the autotuner can skip it where unsupported).
    pub fn needs_subgroups(self) -> bool { matches!(self, MatvecKernel::V2WgSg) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeConfig { pub kernel: MatvecKernel, pub tile: u32 }
impl DecodeConfig {
    /// Pack into the u32 the per-shape cache + system profile already carry. The high bit marks a PACKED
    /// value (vs a legacy bare tile), so V1 (kernel id 0) doesn't collide with an old cached tile.
    pub fn pack(self) -> u32 { 0x8000_0000 | (self.kernel.id() << 16) | (self.tile & 0xffff) }
    pub fn unpack(u: u32) -> DecodeConfig {
        if u & 0x8000_0000 != 0 {
            DecodeConfig { kernel: MatvecKernel::from_id((u >> 16) & 0x7fff), tile: (u & 0xffff).max(1) }
        } else {
            // legacy cache stored a bare tile → the pre-wg128 default kernel (v2)
            DecodeConfig { kernel: MatvecKernel::V2, tile: (u & 0xffff).clamp(1, 8) }
        }
    }
}

/// The matrix of KNOWN decode profiles the autotuner chooses among — now TWO dimensions (kernel × tile).
/// v2-wg128 and v2 across tiles, plus a v1 baseline; v3/t16 are excluded (measured losers everywhere). The
/// autotuner times each per shape and caches the fastest, so performance auto-adapts across platforms.
pub const DECODE_PROFILES: &[DecodeConfig] = &[
    // index 0 is the no-autotune DEFAULT → must be a kernel that compiles everywhere (NOT subgroup).
    DecodeConfig { kernel: MatvecKernel::V2Wg128, tile: 4 }, DecodeConfig { kernel: MatvecKernel::V2Wg128, tile: 2 }, DecodeConfig { kernel: MatvecKernel::V2Wg128, tile: 8 },
    DecodeConfig { kernel: MatvecKernel::V2WgSg, tile: 2 }, DecodeConfig { kernel: MatvecKernel::V2WgSg, tile: 4 }, DecodeConfig { kernel: MatvecKernel::V2WgSg, tile: 8 },
    DecodeConfig { kernel: MatvecKernel::V2, tile: 4 }, DecodeConfig { kernel: MatvecKernel::V2, tile: 8 },
    DecodeConfig { kernel: MatvecKernel::V1, tile: 1 }, DecodeConfig { kernel: MatvecKernel::V1, tile: 4 },
];
/// Autotune probes at a capped output width: the best config transfers across n (it's set by k + kernel
/// overhead, given enough workgroups to saturate the GPU), and full-width probes on a 14B (n up to
/// ~152k) cost seconds apiece. 4096 saturates while keeping each probe cheap.
const PROBE_N_CAP: usize = 4096;
/// The profile index whose tile matches `tile` (or the closest) — used only by the manual tile override
/// (`set_matvec_tile`); prefers a v2-wg128 profile since that's the common best.
fn profile_for_tile(tile: u32) -> u32 {
    DECODE_PROFILES.iter().position(|c| c.tile == tile && c.kernel == MatvecKernel::V2Wg128)
        .or_else(|| DECODE_PROFILES.iter().position(|c| c.tile == tile))
        .or_else(|| DECODE_PROFILES.iter().enumerate().min_by_key(|(_, c)| c.tile.abs_diff(tile)).map(|(i, _)| i))
        .unwrap_or(0) as u32
}

/// LOGSCOPE_DECODE_TILE override for the decode mat-vec tile (read once, cached).
fn env_decode_tile() -> Option<u32> {
    use std::sync::OnceLock;
    static T: OnceLock<Option<u32>> = OnceLock::new();
    *T.get_or_init(|| std::env::var("LOGSCOPE_DECODE_TILE").ok().and_then(|s| s.parse::<u32>().ok()).map(|t| t.clamp(1, 8)))
}
/// Fused-QKV decode is now the DEFAULT (one wqkv matmul instead of three projections, −2 compute
/// dispatches/layer — the dispatch-count lever on the ~210/token decode cost that dominates wall time
/// on a launch-bound UMA GPU). Bit-exact to the separate path (--gate-qkv), so on by default; set
/// LOGSCOPE_FUSED_QKV=0 to fall back to the separate-projection reference path. Read per-call (cheap,
/// matches the original inline env check) so it can be toggled for the A/B equivalence gate.
fn fused_qkv_enabled() -> bool {
    std::env::var("LOGSCOPE_FUSED_QKV").as_deref() != Ok("0")
}
/// Flash split-K decode attention above the 4096-row shared-memory cap (LOGSCOPE_FLASH_ATTN=0 →
/// the old single-workgroup-per-head serial fallback).
fn flash_attn_enabled() -> bool {
    std::env::var("LOGSCOPE_FLASH_ATTN").as_deref() != Ok("0")
}
/// LOGSCOPE_GEMM_TILE override for the prefill GEMM workgroup tile (manual A/B; read once, cached).
fn env_gemm_tile() -> Option<u32> {
    use std::sync::OnceLock;
    static T: OnceLock<Option<u32>> = OnceLock::new();
    *T.get_or_init(|| std::env::var("LOGSCOPE_GEMM_TILE").ok().and_then(|s| s.parse::<u32>().ok()).map(|t| t.clamp(4, 32)))
}

struct Frame { enc: wgpu::CommandEncoder, keep: Vec<Box<dyn std::any::Any + Send>>, passes: u32 }
/// Default passes-per-submit. Frame batching collapsed a token to a single ~129ms submit, which on a
/// shared GPU (display + compute) starves the compositor and shows as screen artifacts. Flushing every N
/// passes bounds each submit's duration so the display is serviced between chunks, while still batching
/// enough to keep submit overhead low. Now the DEFAULT for a tunable knob (see Gpu.flush_every).
const FLUSH_EVERY_DEFAULT: u32 = 32;
/// Candidate passes-per-submit the `SubmitTuner` sweeps on live decode tokens. Smaller = more submits =
/// more overhead but smoother display; larger = fewer submits = less overhead but longer submits.
pub const SUBMIT_PROFILES: &[u32] = &[16, 32, 64, 128, 256, 512];
/// Default in-flight submit cap for the governor (see `Gpu::submit`): one executing + one queued keeps
/// the GPU saturated while bounding how far display work can fall behind. `LOGSCOPE_SUBMIT_INFLIGHT`
/// overrides; 0 disables.
pub const SUBMIT_INFLIGHT_DEFAULT: u32 = 2;
/// Default per-submit duration budget (ms) the tuner may not exceed when settling — roughly half a 60 Hz
/// frame, so the compositor can land a flip between any two submits. `LOGSCOPE_SUBMIT_BUDGET_MS`
/// overrides; 0 (or negative) disables the constraint (pure-speed settle, the old behaviour).
pub const SUBMIT_BUDGET_MS_DEFAULT: f64 = 8.0;
/// The active per-submit budget: env override, else the default. 0/negative → +∞ (unconstrained).
pub fn submit_budget_ms() -> f64 {
    let v = std::env::var("LOGSCOPE_SUBMIT_BUDGET_MS").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(SUBMIT_BUDGET_MS_DEFAULT);
    if v > 0.0 { v } else { f64::INFINITY }
}

/// Online controller that adapts passes-per-submit on LIVE decode tokens. It sweeps `cands`, sampling
/// `per_cand` tokens at each value, takes the median token time, and SETTLES — then holds it for the
/// rest of the stream. Pure (no GPU): each token the caller sets `gpu.set_flush_every(tuner.current())`,
/// generates, then feeds the token's wall-time AND its submit count to `record()`. Safe to adapt
/// mid-stream because batching is bit-exact (it changes only submit grouping, not output).
///
/// SETTLE RULE (smoothness-constrained speed): fastest median token time among candidates whose median
/// per-submit duration fits the budget (`submit_budget_ms`, default 8 ms). Bigger flush values are often
/// fastest in raw ms/tok but each submit then monopolizes a shared GPU long enough to stutter the
/// display — the pure min-median rule reliably picked exactly those. If NO candidate fits the budget
/// (a big model where even flush=16 exceeds it), the smoothest one (min per-submit ms) wins, because at
/// that point the machine's usability is the binding constraint, not tok/s.
pub struct SubmitTuner {
    cands: Vec<u32>,
    per_cand: usize,
    idx: usize,                     // candidate currently being sampled
    samples: Vec<(f64, f64)>,       // (token secs, per-submit ms) for the current candidate
    results: Vec<(u32, f64, f64)>,  // (candidate, median token secs, median per-submit ms)
    budget_ms: f64,
    settled: Option<u32>,
}
impl SubmitTuner {
    pub fn new(cands: &[u32], per_cand: usize) -> Self {
        SubmitTuner { cands: cands.to_vec(), per_cand: per_cand.max(1), idx: 0, samples: Vec::new(), results: Vec::new(),
            budget_ms: submit_budget_ms(),
            settled: if cands.is_empty() { Some(FLUSH_EVERY_DEFAULT) } else { None } }
    }
    /// Override the per-submit budget (tests; callers wanting a different smoothness point).
    pub fn with_budget(mut self, ms: f64) -> Self { self.budget_ms = if ms > 0.0 { ms } else { f64::INFINITY }; self }
    /// The flush_every to use for the NEXT token.
    pub fn current(&self) -> u32 {
        self.settled.unwrap_or_else(|| self.cands[self.idx.min(self.cands.len().saturating_sub(1))])
    }
    /// Feed the wall-time (seconds) and submit count of the token just produced under `current()`.
    /// Advances the sweep; when every candidate has `per_cand` samples it settles (see SETTLE RULE).
    /// Returns Some(chosen) on the transition to settled (for one-time logging), else None.
    pub fn record(&mut self, secs: f64, submits: u64) -> Option<u32> {
        if self.settled.is_some() { return None; }
        self.samples.push((secs, secs * 1000.0 / submits.max(1) as f64));
        if self.samples.len() < self.per_cand { return None; }
        let med = |mut v: Vec<f64>| -> f64 {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)); v[v.len() / 2]
        };
        let tok = med(self.samples.iter().map(|s| s.0).collect());
        let sub = med(self.samples.iter().map(|s| s.1).collect());
        self.results.push((self.cands[self.idx], tok, sub));
        self.samples.clear();
        self.idx += 1;
        if self.idx >= self.cands.len() {
            let within = |r: &&(u32, f64, f64)| r.2 <= self.budget_ms;
            let by_tok = |a: &&(u32, f64, f64), b: &&(u32, f64, f64)| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal);
            let by_sub = |a: &&(u32, f64, f64), b: &&(u32, f64, f64)| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal);
            let best = self.results.iter().filter(within).min_by(by_tok)          // fastest that fits
                .or_else(|| self.results.iter().min_by(by_sub));                  // else smoothest available
            self.settled = best.map(|&(c, _, _)| c).or(Some(FLUSH_EVERY_DEFAULT));
            return self.settled;
        }
        None
    }
    pub fn is_settled(&self) -> bool { self.settled.is_some() }
    pub fn settled(&self) -> Option<u32> { self.settled }
    /// The active per-submit budget (ms; +∞ = unconstrained).
    pub fn budget_ms(&self) -> f64 { self.budget_ms }
    /// `(candidate, median token secs, median per-submit ms)` for every fully-sampled candidate.
    pub fn results(&self) -> &[(u32, f64, f64)] { &self.results }
}

impl Gpu {
    pub fn new() -> Self {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance, force_fallback_adapter: false, compatible_surface: None,
        })).expect("no GPU adapter");
        let info = adapter.get_info();
        let adapter_name = format!("{} [{:?}]", info.name, info.backend);
        // Use the adapter's actual maximum limits, not the conservative downlevel
        // defaults (which cap buffers at 256 MiB / storage bindings at 128 MiB —
        // far too small for a real model's large vocab projections, e.g. a 128k×2048
        // output weight is ~1 GiB). Requesting the adapter's own limits is always valid.
        // Enable subgroup ops when the adapter supports them — lets the decode kernel replace its
        // shared-memory barrier reduction with subgroupAdd (fewer barriers, far less workgroup memory →
        // higher occupancy, the diagnosed decode bottleneck). Harmless where unsupported (flag stays false).
        let want_subgroup = adapter.features().contains(wgpu::Features::SUBGROUP);
        let backend = adapter.get_info().backend;
        // Asahi/Honeykrisp detection (Vulkan driving an Apple GPU): the platform whose BO-VMA ceiling is
        // far below its advertised limits — scoped budget clamp lives behind this, so macOS/Metal keeps
        // its full addressable range (bigger dense residencies stay reachable there).
        let conservative_vma = backend == wgpu::Backend::Vulkan && info.name.contains("Apple");
        let req_features = if want_subgroup { wgpu::Features::SUBGROUP } else { wgpu::Features::empty() };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("kernels"), required_features: req_features,
            required_limits: adapter.limits(), experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance, trace: wgpu::Trace::Off,
        })).expect("no device");
        let subgroups = want_subgroup;
        // Unified-memory detection: on Apple/Asahi the buffer backing IS host RAM, so weight uploads can
        // skip the staging-belt copy (zero-copy map-at-creation). Env override for testing on other HW.
        let unified = match std::env::var("LOGSCOPE_UMA").ok().as_deref() {
            Some("1") => true,
            Some("0") => false,
            _ => crate::zerocopy::is_unified_memory(match info.device_type {
                wgpu::DeviceType::IntegratedGpu => crate::zerocopy::AdapterDeviceType::Integrated,
                wgpu::DeviceType::DiscreteGpu => crate::zerocopy::AdapterDeviceType::Discrete,
                wgpu::DeviceType::VirtualGpu => crate::zerocopy::AdapterDeviceType::Virtual,
                wgpu::DeviceType::Cpu => crate::zerocopy::AdapterDeviceType::Cpu,
                wgpu::DeviceType::Other => crate::zerocopy::AdapterDeviceType::Other,
            }, &info.name),
        };
        let env_tile = std::env::var("LOGSCOPE_MATVEC_TILE").ok().and_then(|v| v.parse::<u32>().ok());
        let profile_forced = env_tile.is_some();
        let profile = std::sync::atomic::AtomicU32::new(env_tile.map(profile_for_tile).unwrap_or(0));
        let flush_every = std::sync::atomic::AtomicU32::new(
            std::env::var("LOGSCOPE_FLUSH_EVERY").ok().and_then(|v| v.parse::<u32>().ok()).map(|n| n.clamp(1, 1024)).unwrap_or(FLUSH_EVERY_DEFAULT));
        let inflight_cap = std::sync::atomic::AtomicU32::new(
            std::env::var("LOGSCOPE_SUBMIT_INFLIGHT").ok().and_then(|v| v.parse::<u32>().ok()).map(|n| n.min(64)).unwrap_or(SUBMIT_INFLIGHT_DEFAULT));
        let gpu = Self { device, queue, adapter_name, pipelines: Mutex::new(HashMap::new()), frame: Mutex::new(None), profile, profile_forced, shape_tiles: std::sync::RwLock::new(std::collections::HashMap::new()), gemm_tiles: std::sync::RwLock::new(std::collections::HashMap::new()), flush_every, inflight_cap, pending_submits: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)), governor_dead: std::sync::atomic::AtomicBool::new(false), dims_cache: Mutex::new(HashMap::new()), pool: std::sync::Arc::new(BufferPool::default()), arena: Mutex::new(None), subgroups, unified, sg_ok: std::sync::atomic::AtomicBool::new(false), gemm_ok: std::sync::atomic::AtomicBool::new(false), valid_gemm_tiles: std::sync::RwLock::new(Vec::new()), rope_neox: std::sync::atomic::AtomicBool::new(false), rope_mscale: std::sync::atomic::AtomicU32::new(1.0f32.to_bits()), conservative_vma };
        // Tiled prefill GEMM uses `override TILE` in @workgroup_size — that needs SPIR-V/MSL specialization
        // constants, which the GL backend lacks (it fails to compile the entry point, and the error surfaces
        // asynchronously so an error scope can't catch it). Restrict the tiled path to non-GL backends, then
        // probe which tiles compile+match there. GL (e.g. llvmpipe CI) stays on the naive kernel.
        if !matches!(backend, wgpu::Backend::Gl) {
            let valid = gpu.probe_valid_gemm_tiles();
            gpu.gemm_ok.store(!valid.is_empty(), std::sync::atomic::Ordering::Relaxed);
            *gpu.valid_gemm_tiles.write().unwrap() = valid;
        }
        // If the driver has subgroups, verify the subgroup decode kernel matches the reference on a tiny
        // shape before ever trusting it — a buggy subgroupAdd (driver bugs are real) is caught here instead
        // of corrupting a token. Cheap (one small mat-vec pair); disable with LOGSCOPE_MATVEC_NOSG.
        if subgroups && std::env::var_os("LOGSCOPE_MATVEC_NOSG").is_none() {
            let ok = gpu.self_check_sg();
            gpu.sg_ok.store(ok, std::sync::atomic::Ordering::Relaxed);
            if !ok { eprintln!("wgpu-engine: subgroup decode kernel failed its load-time self-check on {} — using the barrier-reduction kernel instead", gpu.adapter_name); }
        }
        gpu
    }

    /// Probe which tiled-GEMM tiles COMPILE on this backend and match the naive kernel on a small shape.
    /// Each tile's shader is created inside a validation error scope so a compile failure (e.g. a workgroup
    /// size the backend rejects) is caught, not fatal. Returns the usable subset (may be empty → naive only).
    fn probe_valid_gemm_tiles(&self) -> Vec<u32> {
        let (m, k, n) = (16usize, 64usize, 32usize);
        let mut rng = 0x6E4D_1122_3344_5566u64;
        let mut rnd = || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; (rng >> 40) as f32 / (1u64 << 24) as f32 - 0.5 };
        let w: Vec<f32> = (0..n * k).map(|_| rnd()).collect();
        let act: Vec<f32> = (0..m * k).map(|_| rnd()).collect();
        let qw = self.quantize_q8_resident(&w, n, k);
        let a = self.upload(&act, m, k);
        let naive = self.download(&self.linear_q8_0_naive(&a, &qw, m));
        let scale = naive.iter().map(|x| x.abs()).fold(0.0f32, f32::max).max(1e-6);
        self.gemm_tile_candidates().into_iter().filter(|&t| {
            let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let tiled = self.download(&self.linear_q8_0_gemm(&a, &qw, m, t));
            if pollster::block_on(scope.pop()).is_some() { return false; } // shader didn't compile here
            let worst = naive.iter().zip(&tiled).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
            worst / scale < 1e-2
        }).collect()
    }

    /// Run the subgroup decode kernel against the plain (byte-per-thread) reference on a tiny random shape;
    /// true iff they agree within f32-reassociation tolerance. Gates whether the autotuner may pick sg.
    fn self_check_sg(&self) -> bool {
        let (k, n) = (256usize, 128usize);
        let mut rng = 0x00C0_FFEE_1234_5678u64;
        let mut rnd = || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; (rng >> 40) as f32 / (1u64 << 24) as f32 - 0.5 };
        let w: Vec<f32> = (0..n * k).map(|_| rnd()).collect();
        let act: Vec<f32> = (0..k).map(|_| rnd()).collect();
        let qw = self.quantize_q8_resident(&w, n, k);
        let a = self.upload(&act, 1, k);
        let base = self.download(&self.matvec_decode_v1(&a, &qw, 1));
        let sg = self.download(&self.matvec_decode_v2sg(&a, &qw, 4));
        let scale = base.iter().map(|x| x.abs()).fold(0.0f32, f32::max).max(1e-6);
        let worst = base.iter().zip(&sg).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
        worst / scale < 1e-3
    }

    /// Generic compute dispatch. Convention: storage inputs at bindings 0..n
    /// (read-only), output storage at binding n (read_write), uniform dims at n+1.
    /// Get (or build once and cache) the bind-group layout + compute pipeline for a
    /// kernel with `n` read-only storage inputs (then one read_write output, one
    /// uniform). Keyed by the shader constant's address — our kernels are `&'static`.
    fn pipeline(&self, shader: &str, n: u32) -> Pipe {
        let key = (shader.as_ptr() as usize, n);
        if let Some(p) = self.pipelines.lock().unwrap().get(&key) { return p.clone(); }
        let dev = &self.device;
        let store = |b, ro| wgpu::BindGroupLayoutEntry { binding: b, visibility: wgpu::ShaderStages::COMPUTE, count: None,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: ro }, has_dynamic_offset: false, min_binding_size: None } };
        let mut bgl_entries = Vec::new();
        for i in 0..n { bgl_entries.push(store(i, true)); }
        bgl_entries.push(store(n, false));
        bgl_entries.push(wgpu::BindGroupLayoutEntry { binding: n + 1, visibility: wgpu::ShaderStages::COMPUTE, count: None,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None } });
        let bgl = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &bgl_entries });
        let module = dev.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(shader.into()) });
        let layout = dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
        let pipeline = dev.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: None, layout: Some(&layout),
            module: &module, entry_point: Some("main"), compilation_options: Default::default(), cache: None });
        let p: Pipe = Arc::new((bgl, pipeline));
        self.pipelines.lock().unwrap().insert(key, p.clone());
        p
    }

    pub fn pipeline_count(&self) -> usize { self.pipelines.lock().unwrap().len() }

    /// Build (or fetch) the prefill GEMM pipeline SPECIALIZED to a workgroup tile via a wgpu
    /// pipeline-overridable constant (`override TILE`). One shader source, many specialized pipelines:
    /// the autotuner sweeps tiles by creating these without re-authoring or string-templating WGSL. Keyed
    /// by (shader, 0x4000|tile) so specialized entries never collide with the binding-count keys above.
    fn gemm_pipeline(&self, tile: u32) -> Pipe {
        let key = (LINEAR_Q8_0_TILED.as_ptr() as usize, 0x4000u32 | tile);
        if let Some(p) = self.pipelines.lock().unwrap().get(&key) { return p.clone(); }
        let dev = &self.device;
        let store = |b, ro| wgpu::BindGroupLayoutEntry { binding: b, visibility: wgpu::ShaderStages::COMPUTE, count: None,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: ro }, has_dynamic_offset: false, min_binding_size: None } };
        // a, qw, qs (read-only) + out (storage) + dims (uniform)
        let mut e = vec![store(0, true), store(1, true), store(2, true), store(3, false)];
        e.push(wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::COMPUTE, count: None,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None } });
        let bgl = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &e });
        let module = dev.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(LINEAR_Q8_0_TILED.into()) });
        let layout = dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
        let consts = [("TILE", tile as f64)];
        let pipeline = dev.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: None, layout: Some(&layout),
            module: &module, entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions { constants: &consts, zero_initialize_workgroup_memory: false },
            cache: None });
        let p: Pipe = Arc::new((bgl, pipeline));
        self.pipelines.lock().unwrap().insert(key, p.clone());
        p
    }

    fn run(&self, shader: &str, inputs: &[&[u8]], out_bytes: u64, dims: &[u8], wg: (u32, u32, u32)) -> Vec<u8> {
        let dev = &self.device;
        let in_bufs: Vec<wgpu::Buffer> = inputs.iter().map(|data| dev.create_buffer_init(
            &wgpu::util::BufferInitDescriptor { label: None, contents: data, usage: wgpu::BufferUsages::STORAGE })).collect();
        let out = dev.create_buffer(&wgpu::BufferDescriptor { label: Some("out"), size: out_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        let dbuf = dev.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("dims"), contents: dims, usage: wgpu::BufferUsages::UNIFORM });
        let staging = dev.create_buffer(&wgpu::BufferDescriptor { label: Some("staging"), size: out_bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });

        let n = inputs.len() as u32;
        let (bgl, pipeline) = { let p = self.pipeline(shader, n); (p.0.clone(), p.1.clone()) };

        let mut bg_entries = Vec::new();
        for (i, b) in in_bufs.iter().enumerate() { bg_entries.push(wgpu::BindGroupEntry { binding: i as u32, resource: b.as_entire_binding() }); }
        bg_entries.push(wgpu::BindGroupEntry { binding: n, resource: out.as_entire_binding() });
        bg_entries.push(wgpu::BindGroupEntry { binding: n + 1, resource: dbuf.as_entire_binding() });
        let bg = dev.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &bgl, entries: &bg_entries });

        let mut enc = dev.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
            pass.set_pipeline(&pipeline); pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(wg.0, wg.1, wg.2);
        }
        enc.copy_buffer_to_buffer(&out, 0, &staging, 0, out_bytes);
        self.submit(enc);
        let slice = staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        dev.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let bytes = data.to_vec();
        drop(data); staging.unmap();
        bytes
    }

    fn run_f32(&self, shader: &str, inputs: &[&[u8]], out_len: usize, dims: &[u8], wg: (u32, u32, u32)) -> Vec<f32> {
        let bytes = self.run(shader, inputs, (out_len * 4) as u64, dims, wg);
        bytemuck::cast_slice(&bytes).to_vec()
    }

    pub fn gemm(&self, a: &[f32], b: &[f32], m: usize, n: usize, k: usize) -> Vec<f32> {
        let dims = [m as u32, n as u32, k as u32, 0u32];
        self.run_f32(GEMM, &[bytemuck::cast_slice(a), bytemuck::cast_slice(b)], m * n,
            bytemuck::cast_slice(&dims), ((n as u32).div_ceil(TILE), (m as u32).div_ceil(TILE), 1))
    }
    /// C[M,N] = A[M,K] · Wᵀ, with W stored row-major as [N,K] (nn.Linear weights).
    pub fn linear(&self, a: &[f32], w: &[f32], m: usize, n: usize, k: usize) -> Vec<f32> {
        let dims = [m as u32, n as u32, k as u32, 0u32];
        self.run_f32(LINEAR, &[bytemuck::cast_slice(a), bytemuck::cast_slice(w)], m * n,
            bytemuck::cast_slice(&dims), ((n as u32).div_ceil(TILE), (m as u32).div_ceil(TILE), 1))
    }
    /// Fused dequant+linear for Q8_0-style weights kept resident as int8 (4× less
    /// weight bandwidth than f32). `qw`: row-major [n,k] int8 packed 4-per-u32; `qs`:
    /// [n,kb] per-block f32 scales (kb=k/32). C[m,n]=Σ_k a[m,k]·scale·q. Needs k%32==0.
    pub fn linear_q8_0(&self, a: &[f32], qw: &[u32], qs: &[f32], m: usize, n: usize, k: usize) -> Vec<f32> {
        let kb = k / 32;
        let dims = [m as u32, n as u32, k as u32, kb as u32];
        self.run_f32(LINEAR_Q8_0, &[bytemuck::cast_slice(a), bytemuck::cast_slice(qw), bytemuck::cast_slice(qs)],
            m * n, bytemuck::cast_slice(&dims), ((n as u32).div_ceil(8), (m as u32).div_ceil(8), 1))
    }
    pub fn dequant_q4_0(&self, quant: &[u32], scales: &[f32], nblocks: usize) -> Vec<f32> {
        let nw = nblocks * 32;
        let dims = [nw as u32, 0, 0, 0];
        self.run_f32(DEQUANT_Q4_0, &[bytemuck::cast_slice(quant), bytemuck::cast_slice(scales)], nw,
            bytemuck::cast_slice(&dims), ((nw as u32).div_ceil(64), 1, 1))
    }
    pub fn softmax(&self, x: &[f32], rows: usize, cols: usize) -> Vec<f32> {
        let dims = [rows as u32, cols as u32, 0, 0];
        self.run_f32(SOFTMAX, &[bytemuck::cast_slice(x)], rows * cols, bytemuck::cast_slice(&dims), ((rows as u32).div_ceil(64), 1, 1))
    }
    pub fn rmsnorm(&self, x: &[f32], w: &[f32], rows: usize, cols: usize, eps: f32) -> Vec<f32> {
        let dims = [rows as u32, cols as u32, eps.to_bits(), 0];
        self.run_f32(RMSNORM, &[bytemuck::cast_slice(x), bytemuck::cast_slice(w)], rows * cols, bytemuck::cast_slice(&dims), ((rows as u32).div_ceil(64), 1, 1))
    }
    /// Causal scaled-dot-product attention, single head. Q,K,V are [T,d] row-major.
    pub fn attention(&self, q: &[f32], k: &[f32], v: &[f32], t: usize, d: usize) -> Vec<f32> {
        let dims = [t as u32, d as u32, 1u32, 0];
        self.run_f32(ATTENTION, &[bytemuck::cast_slice(q), bytemuck::cast_slice(k), bytemuck::cast_slice(v)], t * d,
            bytemuck::cast_slice(&dims), ((t as u32).div_ceil(64), 1, 1))
    }
    /// Rotary position embedding (Llama split-half) over [T, d], pos = row index.
    pub fn rope(&self, x: &[f32], t: usize, d: usize, base: f32) -> Vec<f32> {
        let dims = [t as u32, d as u32, base.to_bits(), 0];
        let pairs = (t * d / 2) as u32;
        self.run_f32(ROPE, &[bytemuck::cast_slice(x)], t * d, bytemuck::cast_slice(&dims), (pairs.div_ceil(64), 1, 1))
    }
    pub fn add(&self, a: &[f32], b: &[f32]) -> Vec<f32> {
        let n = a.len(); let dims = [n as u32, 0, 0, 0];
        self.run_f32(ADD, &[bytemuck::cast_slice(a), bytemuck::cast_slice(b)], n, bytemuck::cast_slice(&dims), ((n as u32).div_ceil(64), 1, 1))
    }
    pub fn mul(&self, a: &[f32], b: &[f32]) -> Vec<f32> {
        let n = a.len(); let dims = [n as u32, 0, 0, 0];
        self.run_f32(MUL, &[bytemuck::cast_slice(a), bytemuck::cast_slice(b)], n, bytemuck::cast_slice(&dims), ((n as u32).div_ceil(64), 1, 1))
    }
    pub fn silu(&self, x: &[f32]) -> Vec<f32> {
        let n = x.len(); let dims = [n as u32, 0, 0, 0];
        self.run_f32(SILU, &[bytemuck::cast_slice(x)], n, bytemuck::cast_slice(&dims), ((n as u32).div_ceil(64), 1, 1))
    }
    /// Embedding lookup: gather rows of `table` ([vocab, d]) selected by `ids`.
    pub fn gather(&self, table: &[f32], ids: &[u32], d: usize) -> Vec<f32> {
        let n = ids.len(); let dims = [n as u32, d as u32, 0, 0];
        self.run_f32(GATHER, &[bytemuck::cast_slice(table), bytemuck::cast_slice(ids)], n * d,
            bytemuck::cast_slice(&dims), (((n * d) as u32).div_ceil(64), 1, 1))
    }
    /// GPU argmax of a [1, n] logits tensor → a 1-element u32 tensor holding the max index. Identical
    /// token choice to the CPU `argmax` (lowest index on ties). See --gate-gpusample.
    pub fn argmax_t(&self, logits: &Tensor) -> Tensor {
        let out = self.alloc(1, 1); // 4-byte buffer, bound as array<u32> by ARGMAX
        let dims = [logits.cols as u32, 0u32, 0, 0];
        self.dispatch(ARGMAX, &[logits], &out, bytemuck::cast_slice(&dims), (1, 1, 1));
        out
    }
    /// GPU embedding gather: rows `ids` of resident `table` [vocab, d] → [n, d]. The on-device path for
    /// the next decode step's input, removing the per-token host embedding upload.
    pub fn gather_t(&self, table: &Tensor, ids: &Tensor, n: usize, d: usize) -> Tensor {
        let out = self.alloc(n, d);
        let dims = [n as u32, d as u32, 0, 0];
        self.dispatch(GATHER, &[table, ids], &out, bytemuck::cast_slice(&dims), (((n * d) as u32).div_ceil(64), 1, 1));
        out
    }
    /// gather_t reading a single id at slot `id_off` of a shared ids buffer → [1, d]. For windowed
    /// decode, where step j reads the slot step j's argmax wrote.
    pub fn gather_at(&self, table: &Tensor, ids: &Tensor, id_off: u32, d: usize) -> Tensor {
        let out = self.alloc(1, d);
        let dims = [1u32, d as u32, id_off, 0];
        self.dispatch(GATHER, &[table, ids], &out, bytemuck::cast_slice(&dims), ((d as u32).div_ceil(64), 1, 1));
        out
    }
    /// Gather ONE embedding row from an int8-resident table, by an id read from `ids[id_off]` —
    /// the q8 twin of `gather_at`. This is what lets windowed decode stay GPU-side on a real model
    /// without a full f32 embed copy (151k×2048: ~350 MB int8 vs ~1.2 GB f32); the int8 embed row
    /// is the same tolerance tier as the int8-resident weights the model already runs on.
    pub fn gather_q8_at(&self, table: &QWeight, ids: &Tensor, id_off: u32) -> Tensor {
        let out = self.alloc(1, table.k);
        let dims = [table.k as u32, id_off, (table.k / 32) as u32, 0];
        self.dispatch(GATHER_Q8_AT, &[&table.qw, &table.qs, ids], &out, bytemuck::cast_slice(&dims),
                      ((table.k as u32).div_ceil(64), 1, 1));
        out
    }
    /// argmax_t writing the chosen index into slot `out_slot` of an existing ids buffer (in place), so a
    /// window of K steps can each deposit their token into one buffer without a per-step readback.
    pub fn argmax_at(&self, logits: &Tensor, ids: &Tensor, out_slot: u32) {
        let dims = [logits.cols as u32, out_slot, 0, 0];
        self.dispatch(ARGMAX, &[logits], ids, bytemuck::cast_slice(&dims), (1, 1, 1));
    }

    /// The full sampler (`pick`: repeat-penalty → top-k → temperature softmax → min-p → top-p →
    /// CDF walk) as ONE dispatch, depositing the chosen id into `o[out_slot]` — the sampled twin of
    /// `argmax_at`, which is what lets SAMPLED decode (what chat actually runs) use the windowed
    /// no-per-token-readback loop that greedy already had. `o` is `[kids deposit slots | rcap
    /// recent-ring]`; the ring is seeded with the prompt tail once and extended by the kernel
    /// itself, so the repeat penalty sees the same trailing context the CPU sampler would.
    /// `uni[out_slot]` holds this step's uniform draw (CPU-precomputed xorshift01 stream — rng
    /// stays on the CPU so a seed produces the identical stream the CPU sampler consumes).
    /// Host contract: `temperature > 0` and `2 ≤ top_k ≤ 64`; other configs take the CPU path.
    pub fn sample_at(&self, logits: &Tensor, o: &Tensor, uni: &Tensor, sp: &SamplerParams,
                     out_slot: u32, seeded: u32, rcap: u32, kids: u32, abs_step: u32) {
        let rlast = sp.repeat_last_n.min(64) as u32;
        let dims = [logits.cols as u32, out_slot, sp.top_k as u32, rlast,
                    sp.temperature.to_bits(), sp.top_p.to_bits(), sp.min_p.to_bits(), sp.repeat_penalty.to_bits(),
                    seeded, rcap, kids, abs_step, 0, 0, 0, 0];
        self.dispatch(SAMPLE_AT, &[logits, uni], o, bytemuck::cast_slice(&dims), (1, 1, 1));
    }
    /// Read back a u32 tensor (e.g. the argmax id). Mirrors `download` but reinterprets bytes as u32.
    pub fn download_u32(&self, t: &Tensor) -> Vec<u32> {
        let bytes = (t.len() * 4) as u64;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("dl_u32"), size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        if let Some(mut frame) = self.frame.lock().unwrap().take() {
            frame.enc.copy_buffer_to_buffer(&t.buf, 0, &staging, 0, bytes);
            self.submit(frame.enc);
        } else {
            let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            enc.copy_buffer_to_buffer(&t.buf, 0, &staging, 0, bytes);
            self.submit(enc);
        }
        let slice = staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let v = bytemuck::cast_slice::<u8, u32>(&data).to_vec();
        drop(data); staging.unmap(); v
    }
}

// ── resident GPU tensors: keep intermediates on the device (no CPU round-trips) ──
/// A tensor that lives in a GPU buffer. Ops consume resident tensors and produce
/// resident tensors; only `download` copies back to the host.
/// Size-keyed free-list of `STORAGE|COPY_SRC|COPY_DST` buffers. Every per-op output used to be a fresh
/// `create_buffer` (~hundreds/token of allocation churn — the allocation half of "upload locking"); the
/// pool recycles a buffer of the same byte size when a Tensor drops. Safe because wgpu auto-tracks
/// WAR/WAW hazards on the same queue: a recycled buffer's new write pass is recorded AFTER the old read
/// passes, so the GPU serializes them. Reuse hands back possibly-stale bytes, which is fine — the engine's
/// kernels fully overwrite their output (validated by the multi-op equivalence gates). Per-size cap bounds
/// memory. `wgpu::Buffer` is Arc-backed (Clone), so a clone in the free-list keeps the allocation alive.
#[derive(Default)]
struct BufferPool { free: Mutex<HashMap<u64, Vec<wgpu::Buffer>>> }

/// Slab bump-allocator for resident weights (opt-in: LOGSCOPE_ARENA). Packs many weight tensors into a
/// few large GPU buffers as offset views instead of thousands of separate buffer objects — fewer driver
/// allocations + bind bindings, and on some drivers (Asahi) it dodges per-object VMA/count ceilings that
/// reject a large model even when total memory would fit. Slabs are sized to the device's max buffer.
struct ResidentArena { slabs: Vec<wgpu::Buffer>, slab_size: u64, used: u64, mapped: bool }
/// Clears the active arena when a load finishes (or unwinds) so the bump state never leaks into the next
/// load; the slab buffers themselves stay alive via the weight tensors that cloned them. In zero-copy
/// (UMA `mapped`) mode it also UNMAPS the final slab here — every earlier slab is unmapped when its
/// successor is created, so this makes the last one bindable too. Safe ordering: `load()` returns before
/// the caller runs autotune/prewarm/inference, and nothing inside `load()` dispatches on arena weights.
struct ArenaGuard<'a>(&'a Mutex<Option<ResidentArena>>);
impl Drop for ArenaGuard<'_> {
    fn drop(&mut self) {
        // Tolerate a poisoned mutex: if a load step panicked while holding the arena lock, `.lock().unwrap()`
        // would panic again HERE, inside Drop → non-unwinding panic → abort (killing the process and hiding
        // the real error). We're resetting the bump state to None regardless of prior contents, so the
        // poisoned data is exactly what we want to discard — recover the guard instead of unwrapping.
        let mut g = self.0.lock().unwrap_or_else(|p| p.into_inner());
        // If we're unwinding from a panic (e.g. a slab allocation failing mid-load on a model too big for
        // the GPU's VMA — the 70B on Asahi), do NOT touch the GPU: unmapping a slab whose mapped_at_creation
        // view was never released would panic AGAIN inside this destructor → non-unwinding abort (SIGABRT).
        // Skipping it lets the original panic propagate as a catchable load error instead of crashing.
        if !std::thread::panicking() {
            if let Some(a) = g.as_ref() {
                if a.mapped { if let Some(last) = a.slabs.last() { last.unmap(); } }
            }
        }
        *g = None;
    }
}

pub struct Tensor { buf: wgpu::Buffer, pub rows: usize, pub cols: usize, pool: Option<std::sync::Arc<BufferPool>>, view: Option<(u64, u64)> }
impl Drop for Tensor {
    fn drop(&mut self) {
        if let Some(pool) = &self.pool {
            let size = (self.rows * self.cols * 4) as u64;
            if size > 0 {
                let mut free = pool.free.lock().unwrap();
                let v = free.entry(size).or_default();
                if v.len() < 64 { v.push(self.buf.clone()); } // cap per size → bounded memory
            }
        }
    }
}
impl Tensor {
    pub fn len(&self) -> usize { self.rows * self.cols }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}

/// Weights for one transformer block, uploaded to the GPU once and reused.
pub struct BlockWeights {
    pub wq: Lin, pub wk: Lin, pub wv: Lin, pub wo: Lin,
    /// Fused Q||K||V projection [d+2·kv, d]: one matmul emitting [m, d+2kv] = [q(d) | k(kv) | v(kv)]
    /// per row, mirroring the wgate_up fusion. Built from the same weights as wq/wk/wv and bit-exact
    /// to them (--gate-qkv, --gate-qkv-loader). wq/wk/wv are retained until the hot path (3b) reads
    /// wqkv exclusively; at that point the separates (and their cache entries) can be dropped.
    pub wqkv: Lin,
    pub wgate_up: Lin, pub wdown: Lin, pub g1: Tensor, pub g2: Tensor,
    /// Qwen3/3.5 per-head QK-Norm weights ([head_dim] each), applied as an RMSNorm to Q and K before RoPE.
    /// None for llama/qwen2 (→ pass-through). Loaded from blk.N.attn_{q,k}_norm.weight.
    pub q_norm: Option<Tensor>, pub k_norm: Option<Tensor>,
    /// Mixture-of-experts FFN (qwen3moe): replaces wgate_up/wdown (which hold 1-row placeholders when this
    /// is Some). Router is a small F32 Lin [n_expert, d]; experts are per-expert quantized Lins so the
    /// existing matvec/SwiGLU kernels apply unchanged at an expert's rows (no weight slicing needed).
    pub moe: Option<Box<MoeLayer>>,
}
/// One MoE FFN layer: route(d → n_expert logits) → top-k → per-expert SwiGLU FFN → weighted sum.
pub struct MoeLayer {
    pub router: Lin,                 // [n_expert, d] F32 — precision matters for routing
    /// Per-expert FUSED gate||up projection [2ff, d] — same fusion as the dense wgate_up (one matmul emits
    /// [t, 2ff] = [gate | up], consumed by SWIGLU_PACKED); per-row quant of the concat is identical to
    /// quantizing separately. Cuts one matvec dispatch + weight bind per expert per token.
    pub gate_up: Vec<Lin>,           // n_expert × [2ff, d] — zero-copy row VIEWS into stacked_gu (prefill path)
    pub down: Vec<Lin>,              // n_expert × [d, ff] — views into stacked_down
    /// The full expert stacks as single quantized Lins ([n_expert·2ff, d] / [n_expert·d, ff]) — what the
    /// v2 decode kernels index by GPU-side expert selection (row = sel[slot]·rstride + col, no readback).
    pub stacked_gu: Lin,
    pub stacked_down: Lin,
    pub cfg: crate::moe::MoeConfig,  // n_routed / n_used / norm (softmax for qwen3moe)
    /// SHARED expert (GLM / DeepSeek / Qwen2-MoE): a dense SwiGLU FFN every token runs IN ADDITION
    /// to its routed experts — out = x + Σ wᵢ·expertᵢ(n) + shared(n). Fused gate||up [2·sff, d] +
    /// down [d, sff] (sff read from the tensor itself; shared width differs from the routed ff).
    /// None for qwen3moe. This is the GLM-5.2 / DeepSeek-path stepping stone.
    pub shared: Option<(Lin, Lin)>,
}
/// Build a zero-copy row-range VIEW of a quantized Lin: expert e's [rows, k] slice of a stacked weight,
/// sharing the underlying buffers (no re-quant, no copy). Offsets are 256-aligned for every per-expert
/// slice of the qwen3moe shapes (k=2048/768; scales 32-block → whole f32s per row). F32 Lins (router)
/// aren't row-viewed. Returns None if the Lin kind can't be viewed.
fn lin_row_view(l: &Lin, row0: usize, rows: usize) -> Option<Lin> {
    fn tview(t: &Tensor, byte_off: u64, byte_len: u64, rows: usize, cols: usize) -> Tensor {
        let base = t.view.map(|(o, _)| o).unwrap_or(0);
        Tensor { buf: t.buf.clone(), rows, cols, pool: None, view: Some((base + byte_off, byte_len)) }
    }
    match &l.kind {
        LinKind::Q8(w) => {
            let kb = w.k / 32;
            Some(Lin { kind: LinKind::Q8(QWeight {
                qw: tview(&w.qw, (row0 * w.k) as u64, (rows * w.k) as u64, rows, w.k),
                qs: tview(&w.qs, (row0 * kb * 4) as u64, (rows * kb * 4) as u64, rows, kb),
                n: rows, k: w.k }), bias: None })
        }
        LinKind::Q4(w) => {
            let kb = w.k / 32;
            Some(Lin { kind: LinKind::Q4(Q4Weight {
                qw: tview(&w.qw, (row0 * w.k / 2) as u64, (rows * w.k / 2) as u64, rows, w.k / 2),
                qs: tview(&w.qs, (row0 * kb * 4) as u64, (rows * kb * 4) as u64, rows, kb),
                n: rows, k: w.k }), bias: None })
        }
        LinKind::F32(_) => None,
    }
}
/// One Z-Image/DiT transformer block (Peebles & Xie AdaLN-Zero design): non-affine LayerNorm →
/// AdaLN modulate → bidirectional MHA → AdaLN gated residual → non-affine LayerNorm → AdaLN modulate →
/// MLP (fc1 → GELU → fc2) → AdaLN gated residual. `w_mod` maps `SiLU(cond)` (the timestep+text
/// conditioning embedding) to the 6 per-block modulation vectors (shift/scale/gate × {attn, mlp}),
/// concatenated as one `[1, 6*d]` row that the caller slices — mirroring `wgate_up`'s fusion elsewhere.
pub struct DitBlock {
    pub wq: Lin, pub wk: Lin, pub wv: Lin, pub wo: Lin,
    pub fc1: Lin, pub fc2: Lin,
    pub w_mod: Lin,
}

/// One VAE decoder residual block (the SD/SDXL-family design): GroupNorm → SiLU → conv3x3 → GroupNorm →
/// SiLU → conv3x3, added back to the input — with a 1x1 "skip" conv when `cin != cout` (channel count
/// changes at several points in a real VAE decoder's upsample stack) or a plain passthrough when they
/// match. `gn1_gamma`/`gn1_beta` are `[1,cin]`, `gn2_gamma`/`gn2_beta` are `[1,cout]`, `conv1_w` is
/// `[cout,cin,3,3]`, `conv2_w`/`skip_w` as their shapes imply.
pub struct VaeResBlock {
    pub gn1_gamma: Tensor, pub gn1_beta: Tensor,
    pub conv1_w: Tensor, pub conv1_b: Tensor,
    pub gn2_gamma: Tensor, pub gn2_beta: Tensor,
    pub conv2_w: Tensor, pub conv2_b: Tensor,
    pub skip: Option<(Tensor, Tensor)>,
}

/// One ViT/CLIP encoder layer (pre-norm): LN → multi-head bidirectional attention → residual →
/// LN → MLP (fc1 → GELU → fc2) → residual. Separate Q/K/V (no fusion: t = num_patches > 1 here, and
/// the tower runs once per image). Each Lin may carry its bias via `with_bias`. The GGUF→struct
/// name mapping is model-specific (CLIP vs SigLIP vs Qwen2-VL) and lives in the vision loader.
pub struct VisionBlock {
    pub g1: Tensor, pub b1: Tensor,                 // LN1 scale/bias
    pub wq: Lin, pub wk: Lin, pub wv: Lin, pub wo: Lin,
    pub g2: Tensor, pub b2: Tensor,                 // LN2 scale/bias
    pub fc1: Lin, pub fc2: Lin,                     // MLP: [mlp,d] then [d,mlp]
}

/// A weight matrix kept resident in low-precision int8 form (Q8_0-style): for each
/// 32-wide block of each row, an int8 quant (packed 4-per-u32) and an f32 scale.
/// 4× smaller on the device than f32 — the decode-time bandwidth win.
pub struct QWeight { pub qw: Tensor, pub qs: Tensor, pub n: usize, pub k: usize }

/// A weight kept resident in 4-bit form: per 32-wide block, an f32 scale and 32 symmetric nibbles
/// (value−8 ∈ [−7,7]). 8× smaller than f32, 2× smaller than int8 — the lever that lets a 70B fit
/// (~40 GiB). Symmetric f32-scale (not the GGUF q4_0 f16-scale on-disk format) so CPU and GPU dequant
/// identically and the matmul is bit-exact against a reference.
pub struct Q4Weight { pub qw: Tensor, pub qs: Tensor, pub n: usize, pub k: usize }

/// Little-endian cursor over the int8 cache blob. Every read is bounds-checked and returns Option,
/// so any truncation/corruption makes `load_int8_cache` fail cleanly (→ fall back to GGUF).
struct Rd<'a> { d: &'a [u8], p: usize }
impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> { let s = self.d.get(self.p..self.p + n)?; self.p += n; Some(s) }
    fn u8(&mut self) -> Option<u8> { Some(self.take(1)?[0]) }
    fn u32(&mut self) -> Option<u32> { Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?)) }
    fn u64(&mut self) -> Option<u64> { Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?)) }
    fn f32(&mut self) -> Option<f32> { Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?)) }
    /// A length-prefixed f32 array; built byte-wise so an unaligned cache buffer is fine.
    fn f32s(&mut self) -> Option<Vec<f32>> {
        let n = self.u64()? as usize;
        let b = self.take(n * 4)?;
        Some(b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
    }
}

/// A resident linear weight: full-precision f32 or int8 (`QWeight`), plus an OPTIONAL per-output bias
/// (Qwen2 has one on Q/K/V; llama does not). Both precisions answer the same `apply`, and the bias is
/// folded in there, so every forward path is agnostic to both precision and architecture.
pub struct Lin { kind: LinKind, bias: Option<Tensor> }
enum LinKind { F32(Tensor), Q8(QWeight), Q4(Q4Weight) }
impl Lin {
    pub fn f32(t: Tensor) -> Self { Lin { kind: LinKind::F32(t), bias: None } }
    pub fn q8(q: QWeight) -> Self { Lin { kind: LinKind::Q8(q), bias: None } }
    pub fn q4(q: Q4Weight) -> Self { Lin { kind: LinKind::Q4(q), bias: None } }
    pub fn with_bias(mut self, b: Option<Tensor>) -> Self { self.bias = b; self }
    fn has_bias(&self) -> bool { self.bias.is_some() }
    pub fn apply(&self, gpu: &Gpu, a: &Tensor, m: usize) -> Tensor {
        let out = match &self.kind {
            LinKind::F32(t) => { let (n, k) = (t.rows, t.cols); gpu.linear_t(a, t, m, n, k) }
            LinKind::Q8(q) => gpu.linear_q8_0_t(a, q, m),
            LinKind::Q4(q) => gpu.linear_q4_0_t(a, q, m),
        };
        match &self.bias { Some(b) => gpu.add_bias_t(&out, b), None => out }
    }
    /// apply(), then add `resid` — but for the unbiased quantized m=1 decode case the add is FOLDED
    /// into the matmul epilogue (one dispatch instead of two). Falls back to apply()+add_t otherwise.
    pub fn apply_add(&self, gpu: &Gpu, a: &Tensor, m: usize, resid: &Tensor) -> Tensor {
        if m == 1 && self.bias.is_none() {
            match &self.kind {
                LinKind::Q8(q) => return gpu.linear_q8_0_add(a, q, resid),
                LinKind::Q4(q) => return gpu.linear_q4_0_add(a, q, resid),
                LinKind::F32(_) => {}
            }
        }
        let out = self.apply(gpu, a, m);
        gpu.add_t(&out, resid)
    }
    pub fn is_quantized(&self) -> bool { matches!(self.kind, LinKind::Q8(_) | LinKind::Q4(_)) }
}

impl Gpu {
    pub fn upload(&self, data: &[f32], rows: usize, cols: usize) -> Tensor {
        let bytes: &[u8] = bytemuck::cast_slice(data);
        Tensor { buf: self.upload_bytes(bytes, "tensor"), rows, cols, pool: None, view: None }
    }
    /// Upload `bytes` as a resident STORAGE buffer, zero-copy on unified memory. On UMA (Apple/Asahi) the
    /// buffer's backing IS host RAM, so `mapped_at_creation` + one memcpy writes straight into device memory
    /// — no staging-belt copy, no transient staging allocation (which is what doubled the 70B load peak).
    /// On discrete GPUs we keep `create_buffer` + `write_buffer`: staging is required there, and write_buffer
    /// records a CATCHABLE OutOfMemory instead of the fatal get_mapped_range abort a failed map-on-create
    /// gives. The UMA map path is safe because weight loads memplan-preflight the fit before any upload.
    fn upload_bytes(&self, bytes: &[u8], label: &'static str) -> wgpu::Buffer {
        let size = (bytes.len().max(4)) as u64;
        let usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST;
        if self.unified && !bytes.is_empty() {
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: true });
            {
                let mut view = buf.slice(..).get_mapped_range_mut();
                view.slice(..bytes.len()).copy_from_slice(bytes);
            }
            buf.unmap();
            buf
        } else {
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false });
            if !bytes.is_empty() { self.queue.write_buffer(&buf, 0, bytes); }
            buf
        }
    }
    fn alloc(&self, rows: usize, cols: usize) -> Tensor {
        let size = (rows * cols * 4) as u64;
        // Recycle a same-size buffer freed by a dropped Tensor, else allocate. Kills per-op alloc churn.
        let recycled = if size > 0 { self.pool.free.lock().unwrap().get_mut(&size).and_then(|v| v.pop()) } else { None };
        let buf = match recycled {
            Some(b) => { POOL_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed); b }
            None => {
                POOL_MISSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("tensor"), size,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
            }
        };
        Tensor { buf, rows, cols, pool: Some(self.pool.clone()), view: None }
    }
    pub fn download(&self, t: &Tensor) -> Vec<f32> {
        let bytes = (t.len() * 4) as u64;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("dl"), size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        // If a frame is open, append the readback copy to it and flush the whole frame in one
        // submit — so a decode token's entire compute + its logits readback is a single flush.
        if let Some(mut frame) = self.frame.lock().unwrap().take() {
            frame.enc.copy_buffer_to_buffer(&t.buf, 0, &staging, 0, bytes);
            self.submit(frame.enc);
        } else {
            let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            enc.copy_buffer_to_buffer(&t.buf, 0, &staging, 0, bytes);
            self.submit(enc);
        }
        let slice = staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let t_sync = std::time::Instant::now();
        GPU_POLLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed); self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        GPU_SYNC_NS.fetch_add(t_sync.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        let data = slice.get_mapped_range();
        let v = bytemuck::cast_slice(&data).to_vec();
        drop(data); staging.unmap(); v
    }

    /// Current passes-per-submit (the submit-overhead knob).
    pub fn flush_every(&self) -> u32 { self.flush_every.load(std::sync::atomic::Ordering::Relaxed) }
    /// Set passes-per-submit. Clamped to [1,1024]. Ignored when LOGSCOPE_FLUSH_EVERY pins it, so a manual
    /// override always wins over the adaptive tuner. Returns the value in effect after the call.
    pub fn set_flush_every(&self, n: u32) -> u32 {
        if std::env::var_os("LOGSCOPE_FLUSH_EVERY").is_none() {
            self.flush_every.store(n.clamp(1, 1024), std::sync::atomic::Ordering::Relaxed);
        }
        self.flush_every()
    }
    /// Whether LOGSCOPE_FLUSH_EVERY pins the knob (then the tuner should not run).
    pub fn flush_every_pinned(&self) -> bool { std::env::var_os("LOGSCOPE_FLUSH_EVERY").is_some() }

    /// Single point through which every command buffer is flushed, so submits can be
    /// counted (see GPU_SUBMITS). Each call is one CPU→GPU flush.
    fn submit(&self, enc: wgpu::CommandEncoder) {
        use std::sync::atomic::Ordering::Relaxed;
        GPU_SUBMITS.fetch_add(1, Relaxed);
        self.queue.submit([enc.finish()]);
        // SUBMIT GOVERNOR (see `inflight_cap`): bound queue depth so a display sharing this GPU is
        // serviced between submits. flush_every already bounds each submit's duration; without a depth
        // bound the encoder outruns the GPU and stacks hundreds of ms of compute in front of the
        // compositor. One executing + `cap-1` queued keeps the GPU fed; the wait is a NON-blocking
        // poll pump (PollType::Poll fires completion callbacks without sleeping the queue).
        let cap = self.inflight_cap.load(Relaxed);
        if cap == 0 || self.governor_dead.load(Relaxed) { return; }
        let pending = self.pending_submits.clone();
        pending.fetch_add(1, Relaxed);
        let done = self.pending_submits.clone();
        self.queue.on_submitted_work_done(move || { done.fetch_sub(1, Relaxed); });
        if pending.load(Relaxed) <= cap { return; }
        let t0 = std::time::Instant::now();
        while pending.load(Relaxed) > cap {
            let _ = self.device.poll(wgpu::PollType::Poll);
            std::thread::sleep(std::time::Duration::from_micros(50));
            // Safety escape: if callbacks never arrive (driver quirk), don't spin forever — turn the
            // governor off for this Gpu and say so once. 5s of a full queue means something else is wrong.
            if t0.elapsed().as_secs() >= 5 {
                self.governor_dead.store(true, Relaxed);
                eprintln!("wgpu-engine: submit governor DISABLED — completion callbacks not arriving on this driver (LOGSCOPE_SUBMIT_INFLIGHT=0 silences this)");
                break;
            }
        }
    }
    /// Current in-flight submit cap (0 = governor off).
    pub fn submit_inflight(&self) -> u32 { self.inflight_cap.load(std::sync::atomic::Ordering::Relaxed) }
    /// Set the in-flight submit cap (0 disables). Ignored when LOGSCOPE_SUBMIT_INFLIGHT pins it, so a
    /// manual env choice survives callers that adjust it programmatically.
    pub fn set_submit_inflight(&self, n: u32) -> u32 {
        if std::env::var_os("LOGSCOPE_SUBMIT_INFLIGHT").is_none() {
            self.inflight_cap.store(n.min(64), std::sync::atomic::Ordering::Relaxed);
        }
        self.submit_inflight()
    }

    /// Open a recording frame: subsequent `dispatch`/copy ops record into one shared command
    /// encoder instead of each submitting. Flushed by `end_frame` or the terminal `download`.
    /// Idempotent-safe: an already-open frame is flushed first (no commands are lost).
    pub fn begin_frame(&self) {
        let mut fr = self.frame.lock().unwrap();
        if let Some(prev) = fr.take() { self.submit(prev.enc); }
        *fr = Some(Frame { enc: self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") }), keep: Vec::new(), passes: 0 });
    }
    /// Flush the open frame (one submit for all its recorded ops). No-op if none is open.
    pub fn end_frame(&self) {
        if let Some(frame) = self.frame.lock().unwrap().take() { self.submit(frame.enc); }
    }

    /// Dispatch over resident tensors: inputs at 0..n (read-only), out at n (read_write),
    /// dims uniform at n+1. Output stays on the GPU. When a frame is open the compute pass is
    /// recorded into it (and the bind group + dims buffer kept alive until the frame flushes);
    /// otherwise it submits immediately. wgpu inserts barriers between passes in one encoder,
    /// so recording many ops into one frame preserves each op's writes for the next.
    fn dispatch(&self, shader: &str, inputs: &[&Tensor], out: &Tensor, dims: &[u8], wg: (u32, u32, u32)) {
        let pipe = self.pipeline(shader, inputs.len() as u32);
        self.dispatch_with(&pipe, inputs, out, dims, wg);
    }
    /// Record a dispatch against an already-resolved pipeline. Split out so specialized pipelines (the
    /// override-constant GEMM) share the exact same bind-group + frame-batching path as `dispatch`.
    fn dispatch_with(&self, pipe: &Pipe, inputs: &[&Tensor], out: &Tensor, dims: &[u8], wg: (u32, u32, u32)) {
        let dev = &self.device;
        let t_setup = std::time::Instant::now();
        // Reuse the dims UNIFORM buffer for this op's byte-identical (m,k,n,grid) instead of
        // creating+uploading a fresh one every dispatch — the per-dispatch "upload locking" fix.
        let dbuf = {
            let mut cache = self.dims_cache.lock().unwrap();
            if let Some(b) = cache.get(dims) {
                DIMS_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                b.clone()
            } else {
                DIMS_MISSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let b = std::sync::Arc::new(dev.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("dims"), contents: dims, usage: wgpu::BufferUsages::UNIFORM }));
                // Cap growth: stable dims are inserted during warmup and hit forever; once full,
                // varying per-position dims fall back to create-without-cache (never worse than before).
                if cache.len() < 4096 { cache.insert(dims.to_vec(), b.clone()); }
                b
            }
        };
        let n = inputs.len() as u32;
        let (bgl, pipeline) = (&pipe.0, &pipe.1);
        let mut bg_entries = Vec::new();
        for (i, t) in inputs.iter().enumerate() {
            let resource = match t.view {
                None => t.buf.as_entire_binding(),
                Some((off, len)) => wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &t.buf, offset: off, size: std::num::NonZeroU64::new(len) }),
            };
            bg_entries.push(wgpu::BindGroupEntry { binding: i as u32, resource });
        }
        bg_entries.push(wgpu::BindGroupEntry { binding: n, resource: out.buf.as_entire_binding() });
        bg_entries.push(wgpu::BindGroupEntry { binding: n + 1, resource: dbuf.as_entire_binding() });
        let bg = dev.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: bgl, entries: &bg_entries });
        GPU_SETUP_NS.fetch_add(t_setup.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        let mut fr = self.frame.lock().unwrap();
        if let Some(frame) = fr.as_mut() {
            {
                let mut pass = frame.enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
                pass.set_pipeline(pipeline); pass.set_bind_group(0, &bg, &[]);
                pass.dispatch_workgroups(wg.0, wg.1, wg.2);
            }
            frame.keep.push(Box::new(bg)); // dims buf is cached (or kept alive by `bg` until flush) — no separate keep
            frame.passes += 1;
            // Bound submit duration so a shared GPU still services the display between chunks.
            if frame.passes >= self.flush_every.load(std::sync::atomic::Ordering::Relaxed) {
                let old = fr.take().unwrap();
                self.submit(old.enc);
                *fr = Some(Frame { enc: dev.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") }), keep: Vec::new(), passes: 0 });
            }
        } else {
            drop(fr);
            let mut enc = dev.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            {
                let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
                pass.set_pipeline(pipeline); pass.set_bind_group(0, &bg, &[]);
                pass.dispatch_workgroups(wg.0, wg.1, wg.2);
            }
            self.submit(enc); // queue order makes each op's writes visible to the next
        }
    }

    pub fn linear_t(&self, a: &Tensor, w: &Tensor, m: usize, n: usize, k: usize) -> Tensor {
        let out = self.alloc(m, n); let dims = [m as u32, n as u32, k as u32, 0];
        self.dispatch(LINEAR, &[a, w], &out, bytemuck::cast_slice(&dims), ((n as u32).div_ceil(TILE), (m as u32).div_ceil(TILE), 1)); out
    }
    /// Upload raw u32 words as a resident storage buffer (used for packed int8 weights). Zero-copy on UMA.
    pub fn upload_u32(&self, data: &[u32]) -> Tensor {
        let bytes: &[u8] = bytemuck::cast_slice(data);
        Tensor { buf: self.upload_bytes(bytes, "qw"), rows: 1, cols: data.len(), pool: None, view: None }
    }
    /// Resident-weight uploads: when an arena is active (LOGSCOPE_ARENA, set up by the loader) the bytes
    /// are packed into a shared slab and returned as an offset view; otherwise a dedicated buffer (the
    /// current behaviour). Used only for resident WEIGHTS — activations always get their own buffers.
    fn upload_u32_resident(&self, data: &[u32]) -> Tensor {
        if self.arena.lock().unwrap().is_some() { self.arena_place(bytemuck::cast_slice(data), 1, data.len()) }
        else { self.upload_u32(data) }
    }
    fn upload_resident(&self, data: &[f32], rows: usize, cols: usize) -> Tensor {
        if self.arena.lock().unwrap().is_some() { self.arena_place(bytemuck::cast_slice(data), rows, cols) }
        else { self.upload(data, rows, cols) }
    }
    /// Append `bytes` to the resident arena (256-aligned), starting a new slab when the current one can't
    /// hold it. Returns a view Tensor (buf = slab clone, view = Some(offset,len)). Caller guarantees the
    /// arena is active.
    fn arena_place(&self, bytes: &[u8], rows: usize, cols: usize) -> Tensor {
        const ALIGN: u64 = 256;
        let len = bytes.len() as u64;
        let mut guard = self.arena.lock().unwrap();
        let a = guard.as_mut().expect("arena active");
        let mut off = a.used.next_multiple_of(ALIGN);
        if a.slabs.is_empty() || off + len > a.slab_size {
            if a.mapped {
                // ZERO-COPY: the previous slab is full — unmap it so it can be bound. No staging belt exists
                // in this mode, so there's nothing to drain (this is where the ~2× load peak went away).
                if let Some(prev) = a.slabs.last() { prev.unmap(); }
            } else if !a.slabs.is_empty() {
                // Staging path: drain the PREVIOUS slab's staged writes before allocating the next. wgpu's
                // write_buffer stages through internal buffers that are only reclaimed after a submit+poll;
                // without this, staging for every slab accumulates ON TOP OF the growing arena, so the
                // load-time peak is ~2× the resident size — that's what overran the ~56 GiB GPU VA ceiling
                // loading the 70B (40 GiB arena + ~40 GiB un-drained staging). Flushing per slab caps it.
                self.queue.submit(std::iter::empty());
                let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            }
            // size % COPY_BUFFER_ALIGNMENT (4) == 0 is REQUIRED for a mapped_at_creation buffer; both inputs
            // above are 256-aligned so this is a no-op today, but it guards any future unaligned size source.
            let size = a.slab_size.max(len.next_multiple_of(ALIGN)).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
            // mapped slabs are filled by memcpy into their mapped range (no COPY_DST); staged slabs need COPY_DST
            let usage = if a.mapped { wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC }
                        else { wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST };
            let slab = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("arena-slab"), size,
                usage, mapped_at_creation: a.mapped });
            a.slabs.push(slab);
            a.used = 0;
            off = 0;
        }
        let slab = a.slabs.last().unwrap().clone();
        if a.mapped {
            // UMA zero-copy: write straight into the mapped slab's device-visible backing at `off` — no
            // staging copy. Sub-ranges are non-overlapping (distinct weights), so per-weight views are legal.
            let mut view = slab.slice(off..off + len).get_mapped_range_mut();
            view.copy_from_slice(bytes);
        } else {
            self.queue.write_buffer(&slab, off, bytes);
        }
        a.used = off + len;
        Tensor { buf: slab, rows, cols, pool: None, view: Some((off, len)) }
    }
    /// Quantize a row-major [n,k] f32 weight into a resident int8 `QWeight`
    /// (symmetric per-32-block scales). `k` must be a multiple of 32.
    /// Quantize the (host-side) embedding table into an int8-resident `QWeight` for windowed
    /// decode's GPU-side row gather (`gather_q8_at`). ~4× smaller than an f32 embed copy — a
    /// 151k×2048 table costs ~350 MB instead of ~1.2 GB of Asahi's VMA budget.
    pub fn embed_q8(&self, m: &Model) -> QWeight { self.quantize_q8_resident(&m.embed, m.vocab, m.d) }

    pub fn quantize_q8_resident(&self, w: &[f32], n: usize, k: usize) -> QWeight {
        let kb = k / 32;
        let mut qs = vec![0f32; n * kb];
        let mut qbytes = vec![0i8; n * k];
        // Per-row quantization is independent (per-row scales, disjoint output slices) — fan it out
        // across all cores instead of one. Bit-exact to the sequential version (same per-row math).
        let threads = std::thread::available_parallelism().map(|t| t.get()).unwrap_or(8).max(1).min(n.max(1));
        let rows_per = n.div_ceil(threads);
        std::thread::scope(|s| {
            for (ci, (qs_c, qb_c)) in qs.chunks_mut(rows_per * kb).zip(qbytes.chunks_mut(rows_per * k)).enumerate() {
                s.spawn(move || {
                    let r0 = ci * rows_per;
                    for lr in 0..(qb_c.len() / k) {
                        let r = r0 + lr;
                        for b in 0..kb {
                            let base = r * k + b * 32;
                            let mut amax = 0f32;
                            for j in 0..32 { amax = amax.max(w[base + j].abs()); }
                            let scale = if amax > 0.0 { amax / 127.0 } else { 1.0 };
                            qs_c[lr * kb + b] = scale;
                            for j in 0..32 {
                                qb_c[lr * k + b * 32 + j] = (w[base + j] / scale).round().clamp(-127.0, 127.0) as i32 as i8;
                            }
                        }
                    }
                });
            }
        });
        let mut qw = vec![0u32; n * k / 4];
        let wper = qw.len().div_ceil(threads).max(1);
        std::thread::scope(|s| {
            let qbytes = &qbytes;
            for (ci, qw_c) in qw.chunks_mut(wper).enumerate() {
                s.spawn(move || {
                    let w0 = ci * wper;
                    for (li, word) in qw_c.iter_mut().enumerate() {
                        let i4 = (w0 + li) * 4;
                        *word = (qbytes[i4] as u8 as u32)
                            | ((qbytes[i4 + 1] as u8 as u32) << 8)
                            | ((qbytes[i4 + 2] as u8 as u32) << 16)
                            | ((qbytes[i4 + 3] as u8 as u32) << 24);
                    }
                });
            }
        });
        QWeight { qw: self.upload_u32_resident(&qw), qs: self.upload_resident(&qs, n, kb), n, k }
    }

    /// Load GGUF q8_0 bytes [n,k] DIRECTLY into the int8-resident `QWeight`, skipping the f32 dequant +
    /// re-quantize round-trip. q8_0 (34-byte blocks: f16 scale + 32×i8, w = d·q) and our resident format
    /// (per-32-block scale + packed i8) are the same scheme, so this is exact AND avoids materializing
    /// the f32 weights — the win for big q8_0 models (a 14B's f32 form is ~56 GiB).
    pub fn q8_0_to_resident(&self, raw: &[u8], n: usize, k: usize) -> QWeight {
        let kb = k / 32;
        let nblocks = n * kb;
        let mut qs = vec![0f32; nblocks];
        let mut qw = vec![0u32; n * k / 4];
        for blk in 0..nblocks {
            let off = blk * 34;
            qs[blk] = gguf::f16_to_f32(u16::from_le_bytes([raw[off], raw[off + 1]]));
            for j in 0..32 {
                let idx = blk * 32 + j; // blk = r*kb + b → idx = r*k + b*32 + j (row-major)
                qw[idx / 4] |= (raw[off + 2 + j] as u8 as u32) << ((idx % 4) * 8);
            }
        }
        QWeight { qw: self.upload_u32_resident(&qw), qs: self.upload_resident(&qs, n, kb), n, k }
    }
    /// Quantize f32 weights [n,k] to the resident 4-bit form (per 32-block f32 scale = amax/7, nibble =
    /// clamp(round(w/scale),−7,7)+8). 8 nibbles per u32. k must be a multiple of 32.
    pub fn quantize_q4_resident(&self, w: &[f32], n: usize, k: usize) -> Q4Weight {
        let kb = k / 32;
        let mut qs = vec![0f32; n * kb];
        let mut nib = vec![0u8; n * k];
        let threads = std::thread::available_parallelism().map(|t| t.get()).unwrap_or(8).max(1).min(n.max(1));
        let rows_per = n.div_ceil(threads);
        std::thread::scope(|s| {
            for (ci, (qs_c, nb_c)) in qs.chunks_mut(rows_per * kb).zip(nib.chunks_mut(rows_per * k)).enumerate() {
                s.spawn(move || {
                    let r0 = ci * rows_per;
                    for lr in 0..(nb_c.len() / k) {
                        let r = r0 + lr;
                        for b in 0..kb {
                            let base = r * k + b * 32;
                            let mut amax = 0f32;
                            for j in 0..32 { amax = amax.max(w[base + j].abs()); }
                            let scale = if amax > 0.0 { amax / 7.0 } else { 1.0 };
                            qs_c[lr * kb + b] = scale;
                            for j in 0..32 {
                                let q = (w[base + j] / scale).round().clamp(-7.0, 7.0) as i32;
                                nb_c[lr * k + b * 32 + j] = (q + 8) as u8; // 1..=15, symmetric around 8
                            }
                        }
                    }
                });
            }
        });
        let mut qw = vec![0u32; n * k / 8];
        let wper = qw.len().div_ceil(threads).max(1);
        std::thread::scope(|s| {
            let nib = &nib;
            for (ci, qw_c) in qw.chunks_mut(wper).enumerate() {
                s.spawn(move || {
                    let w0 = ci * wper;
                    for (li, word) in qw_c.iter_mut().enumerate() {
                        let i8b = (w0 + li) * 8;
                        let mut v = 0u32;
                        for b in 0..8 { v |= ((nib[i8b + b] & 0xF) as u32) << (b * 4); }
                        *word = v;
                    }
                });
            }
        });
        Q4Weight { qw: self.upload_u32_resident(&qw), qs: self.upload_resident(&qs, n, kb), n, k }
    }
    /// Resident fused dequant+linear: C[m,n] = A[m,k] · dequant(W)ᵀ, weights int8.
    /// m=1 decode mat-vec with an explicit column tile (1 = classic one-workgroup-per-column;
    /// >1 = tiled, reusing each activation read across `tile` columns). Split out so the tiled and
    /// untiled paths can be A/B-validated on one device.
    pub fn matvec_decode(&self, a: &Tensor, w: &QWeight, tile: u32) -> Tensor {
        // Env escapes force a kernel for A/B (LOGSCOPE_MATVEC_V1/V2/V3); otherwise this entry point defaults
        // to v2-wg128. The forward path doesn't come through here — it uses matvec_decode_cfg with the
        // per-shape AUTOTUNED (kernel, tile), so the winning kernel is chosen at runtime per platform.
        if std::env::var_os("LOGSCOPE_MATVEC_V1").is_some() { return self.matvec_decode_v1(a, w, tile); }
        if std::env::var_os("LOGSCOPE_MATVEC_V2").is_some() { return self.matvec_decode_v2(a, w, tile); }
        if std::env::var_os("LOGSCOPE_MATVEC_V3").is_some() { return self.matvec_decode_v3(a, w, tile); }
        self.matvec_decode_v2wg128(a, w, tile)
    }

    /// Dispatch the decode mat-vec for an explicit (kernel, tile) config — the runtime-autotuned entry the
    /// forward path uses. `matvec_decode_v*` are the concrete kernels; this just routes.
    pub fn matvec_decode_cfg(&self, a: &Tensor, w: &QWeight, cfg: DecodeConfig) -> Tensor {
        match cfg.kernel {
            MatvecKernel::V1 => self.matvec_decode_v1(a, w, cfg.tile),
            MatvecKernel::V2 => self.matvec_decode_v2(a, w, cfg.tile),
            MatvecKernel::V2Wg128 => self.matvec_decode_v2wg128(a, w, cfg.tile),
            MatvecKernel::V2WgSg => self.matvec_decode_v2sg(a, w, cfg.tile),
        }
    }

    /// Whether subgroup ops are available (the SUBGROUP feature was enabled on the device).
    pub fn subgroups(&self) -> bool { self.subgroups }
    /// Whether the tiled prefill GEMM compiled + validated on this backend (else prefill uses naive).
    pub fn gemm_ok(&self) -> bool { self.gemm_ok.load(std::sync::atomic::Ordering::Relaxed) }

    /// v2-wg128 with a subgroup reduction (needs the SUBGROUP feature). Same math; bench candidate.
    pub fn matvec_decode_v2sg(&self, a: &Tensor, w: &QWeight, tile: u32) -> Tensor {
        let out = self.alloc(1, w.n);
        let t = tile.clamp(1, 8);
        let cols_wg = (w.n as u32).div_ceil(t);
        let gx = cols_wg.min(32768);
        let gy = cols_wg.div_ceil(gx);
        let dims = [1u32, w.n as u32, w.k as u32, (w.k / 32) as u32, gx, t, 0, 0];
        self.dispatch(MATVEC_Q8_0_TILED_V2_WG128_SG, &[a, &w.qw, &w.qs], &out, bytemuck::cast_slice(&dims), (gx, gy, 1));
        out
    }

    /// Explicit v2 decode (for validation) — same math as `matvec_decode` with LOGSCOPE_MATVEC_V2 set.
    pub fn matvec_decode_v2(&self, a: &Tensor, w: &QWeight, tile: u32) -> Tensor {
        let out = self.alloc(1, w.n);
        let t = tile.max(1);
        let cols_wg = (w.n as u32).div_ceil(t);
        let gx = cols_wg.min(32768);
        let gy = cols_wg.div_ceil(gx);
        let dims = [1u32, w.n as u32, w.k as u32, (w.k / 32) as u32, gx, t, 0, 0];
        self.dispatch(MATVEC_Q8_0_TILED_V2, &[a, &w.qw, &w.qs], &out, bytemuck::cast_slice(&dims), (gx, gy, 1));
        out
    }

    /// Explicit v3 decode (vec4 128-bit wide loads). Requires k % 16 == 0 (always true for q8). Same math as
    /// the others up to f32 reassociation. The bench compares this against v1/v2 to find the fastest on the
    /// real GPU.
    pub fn matvec_decode_v3(&self, a: &Tensor, w: &QWeight, tile: u32) -> Tensor {
        debug_assert_eq!(w.k % 16, 0, "v3 needs k % 16 == 0");
        let out = self.alloc(1, w.n);
        let t = tile.max(1);
        let cols_wg = (w.n as u32).div_ceil(t);
        let gx = cols_wg.min(32768);
        let gy = cols_wg.div_ceil(gx);
        let dims = [1u32, w.n as u32, w.k as u32, (w.k / 32) as u32, gx, t, 0, 0];
        self.dispatch(MATVEC_Q8_0_TILED_V3, &[a, &w.qw, &w.qs], &out, bytemuck::cast_slice(&dims), (gx, gy, 1));
        out
    }

    /// v2 with tile up to 16 (bigger acc/red). Same math; bench candidate vs v2@8.
    pub fn matvec_decode_v2t16(&self, a: &Tensor, w: &QWeight, tile: u32) -> Tensor {
        let out = self.alloc(1, w.n);
        let t = tile.clamp(1, 16);
        let cols_wg = (w.n as u32).div_ceil(t);
        let gx = cols_wg.min(32768);
        let gy = cols_wg.div_ceil(gx);
        let dims = [1u32, w.n as u32, w.k as u32, (w.k / 32) as u32, gx, t, 0, 0];
        self.dispatch(MATVEC_Q8_0_TILED_V2_T16, &[a, &w.qw, &w.qs], &out, bytemuck::cast_slice(&dims), (gx, gy, 1));
        out
    }

    /// Explicit v1 decode (the original byte-per-thread kernel) — for honest A/B in the bench, since
    /// `matvec_decode` no longer routes to it by default.
    pub fn matvec_decode_v1(&self, a: &Tensor, w: &QWeight, tile: u32) -> Tensor {
        let out = self.alloc(1, w.n);
        if tile > 1 {
            let cols_wg = (w.n as u32).div_ceil(tile);
            let gx = cols_wg.min(32768);
            let gy = cols_wg.div_ceil(gx);
            let dims = [1u32, w.n as u32, w.k as u32, (w.k / 32) as u32, gx, tile, 0, 0];
            self.dispatch(MATVEC_Q8_0_TILED, &[a, &w.qw, &w.qs], &out, bytemuck::cast_slice(&dims), (gx, gy, 1));
        } else {
            let gx = (w.n as u32).min(32768);
            let gy = (w.n as u32).div_ceil(gx);
            let dims = [1u32, w.n as u32, w.k as u32, (w.k / 32) as u32, gx, 0, 0, 0];
            self.dispatch(MATVEC_Q8_0, &[a, &w.qw, &w.qs], &out, bytemuck::cast_slice(&dims), (gx, gy, 1));
        }
        out
    }

    /// v2 with 128-thread workgroups (more k-parallelism per column). Same math; bench candidate.
    pub fn matvec_decode_v2wg128(&self, a: &Tensor, w: &QWeight, tile: u32) -> Tensor {
        let out = self.alloc(1, w.n);
        let t = tile.clamp(1, 8);
        let cols_wg = (w.n as u32).div_ceil(t);
        let gx = cols_wg.min(32768);
        let gy = cols_wg.div_ceil(gx);
        let dims = [1u32, w.n as u32, w.k as u32, (w.k / 32) as u32, gx, t, 0, 0];
        self.dispatch(MATVEC_Q8_0_TILED_V2_WG128, &[a, &w.qw, &w.qs], &out, bytemuck::cast_slice(&dims), (gx, gy, 1));
        out
    }
    /// Residual fold (m=1 decode): c = W·a + resid in one dispatch, folding the post-projection
    /// residual add (wo/wdown) into the matmul epilogue — one fewer ADD launch/layer.
    pub fn linear_q8_0_add(&self, a: &Tensor, w: &QWeight, resid: &Tensor) -> Tensor {
        let out = self.alloc(1, w.n);
        let gx = (w.n as u32).min(32768); let gy = (w.n as u32).div_ceil(gx);
        let dims = [1u32, w.n as u32, w.k as u32, (w.k / 32) as u32, gx, 0, 0, 0];
        self.dispatch(MATVEC_Q8_0_ADD, &[a, &w.qw, &w.qs, resid], &out, bytemuck::cast_slice(&dims), (gx, gy, 1));
        out
    }
    pub fn linear_q4_0_add(&self, a: &Tensor, w: &Q4Weight, resid: &Tensor) -> Tensor {
        let out = self.alloc(1, w.n);
        let gx = (w.n as u32).min(32768); let gy = (w.n as u32).div_ceil(gx);
        let dims = [1u32, w.n as u32, w.k as u32, (w.k / 32) as u32, gx, 0, 0, 0];
        self.dispatch(MATVEC_Q4_ADD, &[a, &w.qw, &w.qs, resid], &out, bytemuck::cast_slice(&dims), (gx, gy, 1));
        out
    }

    /// The resolved system decode profile (a single lookup — the only hot-path read).
    pub fn decode_profile(&self) -> DecodeConfig { DECODE_PROFILES[self.profile.load(std::sync::atomic::Ordering::Relaxed) as usize % DECODE_PROFILES.len()] }
    /// Convenience: the selected profile's column tile.
    pub fn matvec_tile(&self) -> u32 { self.decode_profile().tile }
    pub fn profile_forced(&self) -> bool { self.profile_forced }
    pub fn set_matvec_tile(&self, t: u32) { self.profile.store(profile_for_tile(t), std::sync::atomic::Ordering::Relaxed); }
    /// Set the system profile from a PACKED (kernel, tile) config (what autotune_matvec returns) — matches
    /// the closest DECODE_PROFILES entry so the global fallback uses the autotuned kernel, not just its tile.
    pub fn set_matvec_cfg(&self, packed: u32) {
        let want = DecodeConfig::unpack(packed);
        let idx = DECODE_PROFILES.iter().position(|c| *c == want)
            .or_else(|| DECODE_PROFILES.iter().position(|c| c.kernel == want.kernel && c.tile == want.tile))
            .or_else(|| DECODE_PROFILES.iter().position(|c| c.tile == want.tile))
            .unwrap_or(0);
        self.profile.store(idx as u32, std::sync::atomic::Ordering::Relaxed);
    }

    /// Sweep the KNOWN decode-profile matrix on a representative FFN-sized mat-vec and return the
    /// fastest profile's tile for THIS adapter. Frame-batched so per-dispatch GPU time accrues in one
    /// submit, median-of-reps to shrug off scheduler noise. Pure timing — every profile is
    /// output-equivalent (bit-exact for the tile dimension; see `--gate-matvec`), so a mis-timed pick
    /// can only cost speed, never correctness. Returns a tile value (semantic, survives matrix edits).
    /// Autotune the decode mat-vec for a (k,n) shape: times every (kernel, tile) profile and returns the
    /// fastest, PACKED (see DecodeConfig::pack). This is where the optimal kernel is discovered per platform.
    pub fn autotune_matvec(&self, k: usize, n: usize) -> u32 {
        use std::time::Instant;
        // The best config is set by k (activation-reuse) and kernel overhead, not by the output width n
        // (as long as enough workgroups saturate the GPU), so probe at a CAPPED width — full-width probes
        // on a 14B's output (n=152064) cost seconds each and were the bulk of a ~3-minute load.
        let n = n.min(PROBE_N_CAP);
        let mut rng = 0x1234_5678_9abc_def0u64;
        let mut rnd = || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; (rng >> 40) as f32 / (1u64 << 24) as f32 - 0.5 };
        let w: Vec<f32> = (0..n * k).map(|_| rnd()).collect();
        let act: Vec<f32> = (0..k).map(|_| rnd()).collect();
        let qw = self.quantize_q8_resident(&w, n, k);
        let a = self.upload(&act, 1, k);
        let iters = 48usize; let reps = 5usize;
        let mut best = (DecodeConfig { kernel: MatvecKernel::V2Wg128, tile: 4 }, f64::MAX);
        for &cfg in DECODE_PROFILES {
            // Subgroup kernel: auto-searched only once it PASSED the load-time self_check_sg() on this
            // driver (sg_ok) — so a correct-subgroup GPU gets it automatically, a buggy one silently doesn't,
            // and no manual opt-in is needed. (llvmpipe/GL has no subgroups, so this is simply skipped in CI.)
            if cfg.kernel.needs_subgroups() && !self.sg_ok.load(std::sync::atomic::Ordering::Relaxed) { continue; }
            let _ = self.download(&self.matvec_decode_cfg(&a, &qw, cfg)); // warm pipeline
            let mut times = Vec::with_capacity(reps);
            for _ in 0..reps {
                self.begin_frame();
                let mut last = self.alloc(1, n);
                for _ in 0..iters { last = self.matvec_decode_cfg(&a, &qw, cfg); }
                let t0 = Instant::now();
                let _ = self.download(&last); // flushes the frame (one submit) and syncs
                times.push(t0.elapsed().as_secs_f64() / iters as f64);
            }
            times.sort_by(|x, y| x.partial_cmp(y).unwrap());
            let med = times[times.len() / 2];
            if med < best.1 { best = (cfg, med); }
        }
        best.0.pack()
    }

    /// Probe each DISTINCT (n,k) weight shape in a model and cache the best decode tile per shape, instead
    /// of resolving ONE global tile from whatever shape happened to be benchmarked. A 14B has only a
    /// handful of distinct mat-vec shapes (attention QKVO, FFN gate/up/down, output), so this is a few
    /// quick probes at load — capped-width (see autotune_matvec) and bounded by `deadline` so it can never
    /// stall the load. The per-shape result is reusable across launches and across models that share a
    /// shape. Returns (n, k, tile, probe_ms) for each shape actually probed (cached/env-pinned → skipped).
    pub fn autotune_shapes(&self, shapes: &[(usize, usize)], deadline: Option<std::time::Instant>) -> Vec<(usize, usize, u32, f32)> {
        let mut out = Vec::new();
        if self.profile_forced || env_decode_tile().is_some() { return out; }
        let mut seen = std::collections::HashSet::new();
        for &(n, k) in shapes {
            if k % 32 != 0 || !seen.insert((n, k)) { continue; }
            if self.shape_tiles.read().unwrap().contains_key(&(n as u32, k as u32)) { continue; } // already cached
            if deadline.is_some_and(|d| std::time::Instant::now() >= d) { break; }              // time-boxed
            let t0 = std::time::Instant::now();
            let packed = self.autotune_matvec(k, n); // packed (kernel,tile); reused from the global path
            let ms = t0.elapsed().as_secs_f32() * 1000.0;
            self.shape_tiles.write().unwrap().insert((n as u32, k as u32), packed);
            out.push((n, k, packed, ms));
        }
        out
    }
    /// Manually pin the decode config for one shape (override / testing). Stores a packed (kernel, tile);
    /// a bare tile keeps the current default kernel (v2-wg128).
    pub fn set_shape_tile(&self, n: usize, k: usize, tile: u32) {
        let cfg = DecodeConfig { kernel: MatvecKernel::V2Wg128, tile: tile.clamp(1, 16) };
        self.shape_tiles.write().unwrap().insert((n as u32, k as u32), cfg.pack());
    }
    /// The decode CONFIG (kernel + tile) for a shape: env override wins (manual A/B), then the per-shape
    /// autotune cache (packed), then the global profile. Single map read on the hot path.
    pub fn decode_cfg_for(&self, n: usize, k: usize) -> DecodeConfig {
        if let Some(t) = env_decode_tile() { return DecodeConfig { kernel: MatvecKernel::V2Wg128, tile: t }; }
        let cfg = if let Some(&p) = self.shape_tiles.read().unwrap().get(&(n as u32, k as u32)) { DecodeConfig::unpack(p) } else { self.decode_profile() };
        // Never dispatch the subgroup kernel on a backend that didn't self-check it (no SUBGROUP feature, or
        // it failed the check) — e.g. the default profile or a cache from another GPU. Downgrade to wg128.
        if cfg.kernel.needs_subgroups() && !self.sg_ok.load(std::sync::atomic::Ordering::Relaxed) {
            return DecodeConfig { kernel: MatvecKernel::V2Wg128, tile: cfg.tile };
        }
        cfg
    }
    /// The decode tile for a shape (compat; prefer `decode_cfg_for`).
    pub fn decode_tile_for(&self, n: usize, k: usize) -> u32 {
        self.decode_cfg_for(n, k).tile
    }
    /// Distinct shapes worth tuning are read off a loaded model so the caller doesn't hand-enumerate them.
    pub fn model_decode_shapes(&self, m: &Model) -> Vec<(usize, usize)> {
        let mut s = Vec::new();
        let mut push = |l: &Lin| if let LinKind::Q8(q) = &l.kind { s.push((q.n, q.k)); };
        if let Some(b) = m.layers.first() { push(&b.wq); push(&b.wk); push(&b.wv); push(&b.wo); push(&b.wgate_up); push(&b.wdown); }
        push(&m.output);
        s
    }
    /// Serialize BOTH per-shape caches (decode mat-vec tiles + prefill GEMM tiles). The CALLER keys the
    /// path by adapter (tuning is per-GPU, weight-independent) — mirroring the int8 cache's on-disk
    /// pattern — so a second launch, or a different model sharing shapes, loads tuned tiles instead of
    /// re-probing. Format: magic, then each section as count + (n,k,tile) triples.
    pub fn save_autotune(&self, path: &std::path::Path) -> std::io::Result<()> {
        let dec = self.shape_tiles.read().unwrap();
        let gem = self.gemm_tiles.read().unwrap();
        let mut buf = Vec::with_capacity(8 + (dec.len() + gem.len()) * 12);
        buf.extend_from_slice(&0x4C4154_35u32.to_le_bytes()); // "LAT5" (subgroup kernel now a candidate: re-tune)
        for map in [&*dec, &*gem] {
            buf.extend_from_slice(&(map.len() as u32).to_le_bytes());
            for (&(n, k), &t) in map.iter() { buf.extend_from_slice(&n.to_le_bytes()); buf.extend_from_slice(&k.to_le_bytes()); buf.extend_from_slice(&t.to_le_bytes()); }
        }
        if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &buf)?;
        std::fs::rename(&tmp, path)
    }
    /// Load decode + GEMM tile caches written by `save_autotune` (merged into the in-memory maps). Files
    /// without the magic are read as the old decode-only format. Returns (decode_loaded, gemm_loaded).
    pub fn load_autotune(&self, path: &std::path::Path) -> std::io::Result<(usize, usize)> {
        let buf = std::fs::read(path)?;
        if buf.len() < 4 { return Ok((0, 0)); }
        let rd = |buf: &[u8], o: usize| u32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
        let read_section = |buf: &[u8], mut o: usize, map: &mut std::collections::HashMap<(u32, u32), u32>, clampmax: u32| -> usize {
            let count = rd(buf, o) as usize; o += 4;
            for _ in 0..count {
                if o + 12 > buf.len() { break; }
                map.insert((rd(buf, o), rd(buf, o + 4)), rd(buf, o + 8).clamp(1, clampmax)); o += 12;
            }
            o
        };
        if rd(&buf, 0) == 0x4C4154_35 {
            let mut o = 4;
            let mut dec = self.shape_tiles.write().unwrap();
            let n0 = dec.len(); o = read_section(&buf, o, &mut dec, u32::MAX); let dl = dec.len() - n0; drop(dec);
            let mut gem = self.gemm_tiles.write().unwrap();
            let g0 = gem.len(); read_section(&buf, o, &mut gem, 32); let gl = gem.len() - g0;
            Ok((dl, gl))
        } else {
            // Not the current magic (e.g. a v1-tuned LAT1 file): ignore it so the tiles are re-probed with
            // the current default kernel, rather than reusing stale tile choices from a different kernel.
            Ok((0, 0))
        }
    }

    /// Workgroup-tile candidates for the prefill GEMM, filtered to what this device's limits allow
    /// (TILE×TILE invocations per workgroup, TILE per X/Y dim) — so TILE=32 is dropped on a 256-invocation
    /// device rather than failing pipeline creation.
    pub fn gemm_tile_candidates(&self) -> Vec<u32> {
        let l = self.device.limits();
        let base: Vec<u32> = [8u32, 16, 32].into_iter()
            .filter(|&t| t * t <= l.max_compute_invocations_per_workgroup && t <= l.max_compute_workgroup_size_x && t <= l.max_compute_workgroup_size_y)
            .collect();
        // Once the load probe has run, restrict to tiles that ACTUALLY compiled here (the limit query can
        // over-report — GL accepts the 32×32 limit but rejects the shader). Empty valid list = pre-probe.
        let valid = self.valid_gemm_tiles.read().unwrap();
        if valid.is_empty() { base } else { base.into_iter().filter(|t| valid.contains(t)).collect() }
    }
    /// The prefill GEMM tile for a shape: env override → autotuned per-shape cache → 16 (the prior fixed
    /// default). Clamped to a candidate the device supports.
    pub fn gemm_tile_for(&self, n: usize, k: usize) -> u32 {
        if let Some(t) = env_gemm_tile() { return t; }
        let cands = self.gemm_tile_candidates();
        if let Some(&t) = self.gemm_tiles.read().unwrap().get(&(n as u32, k as u32)) {
            if cands.contains(&t) { return t; } // ignore a stale cached tile that doesn't compile here
        }
        if cands.contains(&16) { 16 } else { cands.last().copied().unwrap_or(16) }
    }
    /// Manually pin the prefill GEMM tile for one shape (override / testing).
    pub fn set_gemm_tile(&self, n: usize, k: usize, tile: u32) { self.gemm_tiles.write().unwrap().insert((n as u32, k as u32), tile); }
    /// Sweep the supported GEMM tiles for each distinct (n,k) shape at a representative prefill width
    /// `m_probe`, time each (warm + median-of-reps), and cache the fastest per shape. Probes at a CAPPED
    /// width + capped m (the tile winner transfers; a full-width probe on the output GEMM is what made a
    /// load take minutes) and is bounded by `deadline`. Returns (n,k,tile,probe_ms) for each shape probed.
    pub fn autotune_gemm(&self, shapes: &[(usize, usize)], m_probe: usize, deadline: Option<std::time::Instant>) -> Vec<(usize, usize, u32, f32)> {
        use std::time::Instant;
        let mut out = Vec::new();
        if env_gemm_tile().is_some() { return out; }
        let cands = self.gemm_tile_candidates();
        if cands.len() < 2 { return out; } // nothing to choose
        let (reps, m) = (3usize, m_probe.clamp(16, 256)); // cap m: tile choice is ~stable for m≥64
        let mut seen = std::collections::HashSet::new();
        for &(n, k) in shapes {
            if k % 32 != 0 || !seen.insert((n, k)) { continue; }
            if self.gemm_tiles.read().unwrap().contains_key(&(n as u32, k as u32)) { continue; } // already cached
            if deadline.is_some_and(|d| Instant::now() >= d) { break; }                         // time-boxed
            let t0 = Instant::now();
            let np = n.min(PROBE_N_CAP); // probe at capped width; winner transfers to full n
            let qw = self.quantize_q8_resident(&vec![0.0625f32; np * k], np, k); // values irrelevant — timing only
            let a = self.upload(&vec![0.03125f32; m * k], m, k);
            let mut best = (cands[0], f64::MAX);
            for &t in &cands {
                let _ = self.download(&self.linear_q8_0_gemm(&a, &qw, m, t)); // warm the pipeline
                let mut times = Vec::with_capacity(reps);
                for _ in 0..reps {
                    let t1 = Instant::now();
                    let _ = self.download(&self.linear_q8_0_gemm(&a, &qw, m, t));
                    times.push(t1.elapsed().as_secs_f64());
                }
                times.sort_by(|x, y| x.partial_cmp(y).unwrap());
                if times[times.len() / 2] < best.1 { best = (t, times[times.len() / 2]); }
            }
            self.gemm_tiles.write().unwrap().insert((n as u32, k as u32), best.0);
            out.push((n, k, best.0, t0.elapsed().as_secs_f32() * 1000.0));
        }
        out
    }

    pub fn linear_q8_0_t(&self, a: &Tensor, w: &QWeight, m: usize) -> Tensor {
        if m == 1 {
            // decode: env override (manual A/B) → per-shape autotuned (kernel, tile) → global profile.
            // The KERNEL is chosen at runtime per shape+platform, not hardcoded (matvec_decode_cfg dispatches).
            if std::env::var_os("LOGSCOPE_MATVEC_V1").is_some() || std::env::var_os("LOGSCOPE_MATVEC_V2").is_some() || std::env::var_os("LOGSCOPE_MATVEC_V3").is_some() {
                return self.matvec_decode(a, w, self.decode_tile_for(w.n, w.k)); // env forces a kernel
            }
            return self.matvec_decode_cfg(a, w, self.decode_cfg_for(w.n, w.k));
        }
        // prefill (m>1): the shared-memory-TILED GEMM is the default (validated bit-exact-to-tol via
        // --gate-gemm-spec; it's the classic compute-bound win over the naive 8x8 kernel that made the
        // first-token latency huge). Escapes: LOGSCOPE_PREFILL_NAIVE forces the old path; LOGSCOPE_GEMM_TILE
        // pins the tile. Small batches (m≤8, spec-decode verify) stay on the coalesced mat-vec.
        if m <= 8 {
            let out = self.alloc(m, w.n);
            // small batch (speculative verify block, batched decode at small N): coalesced
            // mat-vec that reads each weight once and accumulates all m rows — memory-bound cost
            // stays near the m=1 pass instead of riding the strided GEMM.
            let gx = (w.n as u32).min(32768);
            let gy = (w.n as u32).div_ceil(gx);
            let dims = [m as u32, w.n as u32, w.k as u32, (w.k / 32) as u32, gx, 0, 0, 0];
            self.dispatch(MATVEC_SMALL_Q8_0, &[a, &w.qw, &w.qs], &out, bytemuck::cast_slice(&dims), (gx, gy, 1));
            return out;
        }
        if std::env::var_os("LOGSCOPE_PREFILL_NAIVE").is_some() { return self.linear_q8_0_naive(a, w, m); }
        // register-blocked GEMM is the DEFAULT prefill kernel: measured 2795 GFLOP/s on M2 (2.8× the
        // override-tiled kernel, 5.6× naive), and it has no override constants so it compiles on every
        // backend (GL included) — no gemm_ok gate needed. Escape to the tiled kernel with LOGSCOPE_PREFILL_TILED.
        if std::env::var_os("LOGSCOPE_PREFILL_TILED").is_some() && self.gemm_ok() {
            return self.linear_q8_0_gemm(a, w, m, self.gemm_tile_for(w.n, w.k));
        }
        self.linear_q8_0_gemm_rb(a, w, m)
    }

    /// Register-blocked q8 prefill GEMM (4×4 outputs per thread). Fixed 64×64 blocks; grid covers m,n.
    pub fn linear_q8_0_gemm_rb(&self, a: &Tensor, w: &QWeight, m: usize) -> Tensor {
        let out = self.alloc(m, w.n);
        let dims = [m as u32, w.n as u32, w.k as u32, (w.k / 32) as u32];
        let gx = (w.n as u32).div_ceil(64);
        let gy = (m as u32).div_ceil(64);
        self.dispatch(LINEAR_Q8_0_RB, &[a, &w.qw, &w.qs], &out, bytemuck::cast_slice(&dims), (gx, gy, 1));
        out
    }

    /// The naive (un-tiled) 8x8 prefill GEMM — kept as the fallback/reference and for A/B benching.
    pub fn linear_q8_0_naive(&self, a: &Tensor, w: &QWeight, m: usize) -> Tensor {
        let out = self.alloc(m, w.n);
        let dims = [m as u32, w.n as u32, w.k as u32, (w.k / 32) as u32];
        self.dispatch(LINEAR_Q8_0, &[a, &w.qw, &w.qs], &out, bytemuck::cast_slice(&dims),
            ((w.n as u32).div_ceil(8), (m as u32).div_ceil(8), 1));
        out
    }
    /// Shared-memory-tiled prefill GEMM, specialized to a workgroup `tile` via an override constant.
    /// BIT-EXACT across tiles and matches `linear_q8_0_t`'s naive path within float tol. The prefill-speed
    /// lever; opt in via LOGSCOPE_PREFILL_TILED=1 (tile from the GEMM autotune cache, else 16), or call
    /// `linear_q8_0_gemm` directly with an explicit tile.
    pub fn linear_q8_0_tiled_t(&self, a: &Tensor, w: &QWeight, m: usize) -> Tensor {
        self.linear_q8_0_gemm(a, w, m, self.gemm_tile_for(w.n, w.k))
    }
    /// Prefill GEMM at an EXPLICIT workgroup tile (used by the autotuner's sweep and the tiled path).
    pub fn linear_q8_0_gemm(&self, a: &Tensor, w: &QWeight, m: usize, tile: u32) -> Tensor {
        let out = self.alloc(m, w.n);
        let dims = [m as u32, w.n as u32, w.k as u32, (w.k / 32) as u32];
        let pipe = self.gemm_pipeline(tile);
        self.dispatch_with(&pipe, &[a, &w.qw, &w.qs], &out, bytemuck::cast_slice(&dims),
            ((w.n as u32).div_ceil(tile), (m as u32).div_ceil(tile), 1));
        out
    }
    /// Resident 4-bit matmul: a[m,k] · W[n,k]ᵀ where W is `Q4Weight`. Mirrors linear_q8_0_t — a
    /// single-row mat-vec kernel for decode (m=1) and a tiled kernel for prefill (m>1).
    pub fn linear_q4_0_t(&self, a: &Tensor, w: &Q4Weight, m: usize) -> Tensor {
        let out = self.alloc(m, w.n);
        if m == 1 {
            let gx = (w.n as u32).min(32768); let gy = (w.n as u32).div_ceil(gx);
            let dims = [1u32, w.n as u32, w.k as u32, (w.k / 32) as u32, gx, 0, 0, 0];
            self.dispatch(MATVEC_Q4, &[a, &w.qw, &w.qs], &out, bytemuck::cast_slice(&dims), (gx, gy, 1));
        } else if m <= 8 {
            // small batch (speculative verify block / batched decode): read each weight nibble once,
            // accumulate all m rows — keeps Q4's halved weight-bandwidth advantage instead of the
            // strided LINEAR_Q4 re-reading weights per tile. Each row is bit-exact to the m=1 kernel.
            let gx = (w.n as u32).min(32768); let gy = (w.n as u32).div_ceil(gx);
            let dims = [m as u32, w.n as u32, w.k as u32, (w.k / 32) as u32, gx, 0, 0, 0];
            self.dispatch(MATVEC_SMALL_Q4, &[a, &w.qw, &w.qs], &out, bytemuck::cast_slice(&dims), (gx, gy, 1));
        } else {
            let dims = [m as u32, w.n as u32, w.k as u32, (w.k / 32) as u32];
            self.dispatch(LINEAR_Q4, &[a, &w.qw, &w.qs], &out, bytemuck::cast_slice(&dims),
                ((w.n as u32).div_ceil(8), (m as u32).div_ceil(8), 1));
        }
        out
    }
    /// Build a resident linear from f32 weights [n,k]: int8 when `quantize` and k%32==0
    /// (the fused kernel needs whole 32-wide blocks), else full-precision f32.
    fn make_lin(&self, w: &[f32], n: usize, k: usize, quantize: bool, q4: bool) -> Lin {
        if quantize && k % 32 == 0 {
            // q4 = 4-bit residency (8× smaller than f32, 2× smaller than int8) — the lever for fitting a
            // model that won't fit int8. Chosen at load by memplan (or forced via LOGSCOPE_Q4_RESIDENT).
            // Validated bit-exact by --gate-q4resident.
            if q4 { Lin::q4(self.quantize_q4_resident(w, n, k)) }
            else { Lin::q8(self.quantize_q8_resident(w, n, k)) }
        } else { Lin::f32(self.upload(w, n, k)) }
    }
    pub fn rmsnorm_t(&self, x: &Tensor, w: &Tensor, rows: usize, cols: usize, eps: f32) -> Tensor {
        let out = self.alloc(rows, cols); let dims = [rows as u32, cols as u32, eps.to_bits(), 0];
        self.dispatch(RMSNORM, &[x, w], &out, bytemuck::cast_slice(&dims), ((rows as u32).div_ceil(64), 1, 1)); out
    }
    pub fn rope_t(&self, x: &Tensor, t: usize, d: usize, base: f32) -> Tensor {
        let out = self.alloc(t, d); let dims = [t as u32, d as u32, base.to_bits(), 0];
        self.dispatch(ROPE, &[x], &out, bytemuck::cast_slice(&dims), (((t * d / 2) as u32).div_ceil(64), 1, 1)); out
    }
    pub fn attention_t(&self, q: &Tensor, k: &Tensor, v: &Tensor, t: usize, d: usize) -> Tensor {
        let out = self.alloc(t, d); let dims = [t as u32, d as u32, 1u32, 0];
        self.dispatch(ATTENTION, &[q, k, v], &out, bytemuck::cast_slice(&dims), ((t as u32).div_ceil(64), 1, 1)); out
    }
    /// Fused QK-Norm + RoPE (decode t=1): one dispatch for what was a 4-dispatch chain. Input `qkv` is
    /// the fused projection [1, ad+2kv]; returns [1, ad+kv] = roped q ‖ roped k (v stays in `qkv`).
    fn qknorm_rope_fused(&self, qkv: &Tensor, qn: &Tensor, kn: &Tensor, ff: &Tensor,
                         n_heads: usize, n_kv_heads: usize, hd: usize, base: f32, pos: usize, eps: f32) -> Tensor {
        let ad = n_heads * hd; let kv = n_kv_heads * hd;
        let out = self.alloc(1, ad + kv);
        let neox = self.rope_neox.load(std::sync::atomic::Ordering::Relaxed) as u32;
        let msc = self.rope_mscale.load(std::sync::atomic::Ordering::Relaxed);
        let dims = [n_heads as u32, n_kv_heads as u32, hd as u32, base.to_bits(),
                    pos as u32, neox, eps.to_bits(), msc];
        self.dispatch(QKNORM_ROPE, &[qkv, qn, kn, ff], &out, bytemuck::cast_slice(&dims),
                      ((n_heads + n_kv_heads) as u32, 1, 1));
        out
    }

    /// FFN half of a transformer block: norm2 → (dense SwiGLU | MoE) → residual. Dense path is the exact
    /// motif every block fn used inline (rmsnorm → wgate_up → swiglu → wdown+residual), factored so the
    /// MoE branch slots in at every call site (prefill, all decode variants, batched) identically.
    fn ffn_apply(&self, w: &BlockWeights, x2: &Tensor, t: usize, d: usize, eps: f32) -> Tensor {
        let n2 = self.rmsnorm_t(x2, &w.g2, t, d, eps);
        if let Some(m) = w.moe.as_deref() { return self.moe_ffn(m, &n2, t, d, x2); }
        let gu = w.wgate_up.apply(self, &n2, t);
        let hmid = self.swiglu_packed(&gu);
        w.wdown.apply_add(self, &hmid, t, x2)
    }

    /// MoE FFN over `t` tokens (t=1 decode or t=thousands prefill — same path via expert grouping):
    /// router matvec → READBACK [t×n_expert] logits → CPU top-k (moe::route, tested) → for each expert
    /// that won ≥1 token: GATHER its token rows, run the expert's SwiGLU FFN batched (existing kernels),
    /// then weighted SCATTER-ADD into the output. Correctness-first v1: the per-layer router readback is a
    /// GPU sync (~1-3 ms) — the known perf ceiling (~5-10 tok/s decode) until GPU-side top-k lands.
    fn moe_ffn(&self, m: &MoeLayer, n2: &Tensor, t: usize, d: usize, x2: &Tensor) -> Tensor {
        let routed = self.moe_ffn_routed(m, n2, t, d, x2);
        // Shared expert (GLM / DeepSeek / Qwen2-MoE): a dense SwiGLU FFN added on top of the routed
        // sum for EVERY token — the standard shared+routed MoE decomposition. qwen3moe: None →
        // byte-identical to before. Runs batched over t on the existing dense kernels (+3
        // dispatches per MoE layer); a fused MOE_WSUM variant can absorb the add later.
        match &m.shared {
            None => routed,
            Some((sgu, sdn)) => {
                let gu = sgu.apply(self, n2, t);
                let hmid = self.swiglu_packed(&gu);
                let sh = sdn.apply(self, &hmid, t);
                self.add_t(&routed, &sh)
            }
        }
    }
    /// Validation-facing entry for the whole MoE FFN (routing + experts + shared + residual).
    pub fn moe_ffn_t(&self, m: &MoeLayer, n2: &Tensor, t: usize, d: usize, x2: &Tensor) -> Tensor {
        self.moe_ffn(m, n2, t, d, x2)
    }
    fn moe_ffn_routed(&self, m: &MoeLayer, n2: &Tensor, t: usize, d: usize, x2: &Tensor) -> Tensor {
        // v2 DECODE FAST PATH: route ON the GPU — top-k kernel + expert-indexed matvecs over the stacked
        // weights. Zero readbacks (v1 paid 48 submit+poll syncs per token ≈ 340 ms on Asahi — ~90% of the
        // 2.7 tok/s wall time). Prefill (t>1) keeps the v1 grouped path over the SAME weights via row
        // views — which is also the built-in cross-check: if prefill output is coherent and decode isn't,
        // the v2 kernels are wrong, not the weights. LOGSCOPE_MOE_V1=1 forces the v1 path for A/B.
        if t == 1 && matches!(m.cfg.norm, crate::moe::RouteNorm::Softmax)
            && std::env::var("LOGSCOPE_MOE_V1").is_err()
        {
            if std::env::var("LOGSCOPE_MOE_V2_CHECK").is_ok() {
                return self.moe_v2_check(m, n2, d, x2);
            }
            if let Some(out) = self.moe_ffn_gpu_routed(m, n2, d, x2) { return out; }
        }
        self.moe_ffn_v1(m, n2, t, d, x2)
    }

    /// v1 readback-routed MoE FFN (the proven path): router → download → CPU route → grouped experts.
    fn moe_ffn_v1(&self, m: &MoeLayer, n2: &Tensor, t: usize, d: usize, x2: &Tensor) -> Tensor {
        let logits = self.download(&m.router.apply(self, n2, t)); // [t, n_expert] — the sync
        let ne = m.cfg.n_routed as usize;
        // CPU routing: per token, top-k experts + combine weights; group token indices per expert.
        let mut groups: Vec<Vec<(u32, f32)>> = vec![Vec::new(); ne]; // expert → [(token_row, weight)]
        for tok in 0..t {
            for sel in crate::moe::route(&logits[tok * ne..(tok + 1) * ne], &m.cfg) {
                groups[sel.expert as usize].push((tok as u32, sel.weight));
            }
        }
        // residual base: out starts as a copy of x2, experts scatter-add on top
        let out = self.alloc(t, d);
        self.copy_buf(x2, 0, &out, 0, (t * d * 4) as u64);
        for (e, grp) in groups.iter().enumerate() {
            if grp.is_empty() { continue; }
            let n = grp.len();
            let idx = self.upload_u32(&grp.iter().map(|&(r, _)| r).collect::<Vec<_>>());
            let wgt = self.upload(&grp.iter().map(|&(_, w)| w).collect::<Vec<_>>(), 1, n);
            // gather this expert's token rows → [n, d]
            let rows = self.alloc(n, d);
            let dims = [n as u32, d as u32, 0, 0];
            self.dispatch(GATHER_ROWS, &[n2, &idx], &rows, bytemuck::cast_slice(&dims), wg1((n * d) as u32));
            // expert FFN, batched over its n tokens (existing kernels; gate/up unfused per expert)
            let gu = m.gate_up[e].apply(self, &rows, n);
            let hmid = self.swiglu_packed(&gu);
            let dn = m.down[e].apply(self, &hmid, n);
            // out[token] += weight * dn[i]
            self.dispatch(SCATTER_ADD_W, &[&dn, &idx, &wgt], &out, bytemuck::cast_slice(&dims), wg1((n * d) as u32));
        }
        out
    }

    /// Differential checker (LOGSCOPE_MOE_V2_CHECK=1): run BOTH MoE paths on the same token, report which
    /// stage diverges, and RETURN THE V1 RESULT so output stays correct while diagnosing. Per layer it
    /// compares (a) the GPU top-k selection vs the tested CPU route (indices + weights) and (b) the final
    /// FFN output tensors (max |Δ|). Prints the first ~96 layer-calls (≈ 2 tokens) then goes quiet.
    fn moe_v2_check(&self, m: &MoeLayer, n2: &Tensor, d: usize, x2: &Tensor) -> Tensor {
        use std::sync::atomic::{AtomicU32, Ordering};
        static CALLS: AtomicU32 = AtomicU32::new(0);
        let call = CALLS.fetch_add(1, Ordering::Relaxed);
        let v1 = self.moe_ffn_v1(m, n2, 1, d, x2);
        if call >= 96 { return v1; }
        let Some((v2, selw)) = self.moe_ffn_gpu_routed_inner(m, n2, d, x2) else {
            eprintln!("moe-check[{call}]: v2 path unavailable (non-quant stacked)"); return v1;
        };
        // selection: GPU selw vs CPU route on the same logits
        let logits = self.download(&m.router.apply(self, n2, 1));
        let cpu = crate::moe::route(&logits, &m.cfg);
        let raw = self.download_u32(&selw);
        let k = m.cfg.n_used.clamp(1, 16) as usize;
        let gsel: Vec<u32> = raw[..k].to_vec();
        let gwgt: Vec<f32> = raw[k..2 * k].iter().map(|&b| f32::from_bits(b)).collect();
        let csel: Vec<u32> = cpu.iter().map(|s| s.expert).collect();
        let sel_ok = gsel == csel;
        let wgt_dmax = cpu.iter().zip(&gwgt).map(|(c, g)| (c.weight - g).abs()).fold(0.0f32, f32::max);
        // outputs
        let a = self.download(&v1);
        let b = self.download(&v2);
        let out_dmax = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
        let out_ref = a.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
        let line = format!("moe-check[{call:>2}]: sel {} (gpu {:?} vs cpu {:?}) | wgtΔmax {:.2e} | outΔmax {:.3e} (ref max {:.3e})",
            if sel_ok { "OK " } else { "MISMATCH" }, gsel, csel, wgt_dmax, out_dmax, out_ref);
        eprintln!("{line}");
        // If the env value looks like a path (contains '/'), ALSO append there — the reliable sharing
        // channel (attachments keep arriving empty; the GGUF dump-to-file pattern has worked every time).
        if let Ok(v) = std::env::var("LOGSCOPE_MOE_V2_CHECK") {
            if v.contains('/') {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&v) {
                    let _ = writeln!(f, "{line}");
                }
            }
        }
        v1
    }

    /// The GPU-routed MoE decode step (t=1): router matvec → MOE_TOPK (selection + softmax weights stay
    /// on-GPU) → per-slot expert-indexed matvecs over the stacked gate_up/down → weighted combine folded
    /// with the residual. ~20 dispatches, ZERO polls. Returns None if the stacked weights aren't a
    /// quantized kind these kernels read (then the caller runs the v1 readback path).
    fn moe_ffn_gpu_routed(&self, m: &MoeLayer, n2: &Tensor, d: usize, x2: &Tensor) -> Option<Tensor> {
        self.moe_ffn_gpu_routed_inner(m, n2, d, x2).map(|(t, _)| t)
    }
    fn moe_ffn_gpu_routed_inner(&self, m: &MoeLayer, n2: &Tensor, d: usize, x2: &Tensor) -> Option<(Tensor, Tensor)> {
        let (gu_qw, gu_qs, gu_k, sh) = match &m.stacked_gu.kind {
            LinKind::Q8(w) => (&w.qw, &w.qs, w.k, MOE_MATVEC_Q8),
            LinKind::Q4(w) => (&w.qw, &w.qs, w.k, MOE_MATVEC_Q4),
            LinKind::F32(_) => return None,
        };
        let (dn_qw, dn_qs, dn_k, sh_dn) = match &m.stacked_down.kind {
            LinKind::Q8(w) => (&w.qw, &w.qs, w.k, MOE_MATVEC_Q8),
            LinKind::Q4(w) => (&w.qw, &w.qs, w.k, MOE_MATVEC_Q4),
            LinKind::F32(_) => return None,
        };
        let slots = m.cfg.n_used.clamp(1, 16) as usize;
        let ne = m.cfg.n_routed as usize;
        let ff2 = 2 * dn_k;              // fused gate||up rows per expert = 2·ff (ff == down's k)
        // 1) router logits stay on-GPU
        let logits = m.router.apply(self, n2, 1);
        // 2) top-k selection + combine weights → selw [k idx (u32) | k weights (f32 bitcast)]
        let selw = self.alloc(1, 2 * slots);
        let tk = [ne as u32, slots as u32, 0, 0];
        self.dispatch(MOE_TOPK, &[&logits], &selw, bytemuck::cast_slice(&tk), (1, 1, 1));
        // 3) fused gate||up for each selected expert (broadcast activation)
        // one dispatch across ALL selected experts: wg.z = slot (was 8 dispatches — the dominant
        // per-layer dispatch cost; on Asahi each submit batch costs ~4-5 ms of sync, so dispatch count
        // is the wall clock now that the polls are gone)
        let gu_out = self.alloc(slots, ff2);
        {
            let gx = (ff2 as u32).min(32768);
            let gy = (ff2 as u32).div_ceil(gx);
            let dims = [ff2 as u32, gu_k as u32, (gu_k / 32) as u32, gx, 0u32, ff2 as u32, 0u32, 0u32];
            self.dispatch(sh, &[n2, gu_qw, gu_qs, &selw], &gu_out, bytemuck::cast_slice(&dims), (gx, gy, slots as u32));
        }
        // 4) SwiGLU over the packed [slots, 2ff]
        let hmid = self.swiglu_packed(&gu_out);
        // 5) down projection per expert (per-slot activation rows)
        let dn_out = self.alloc(slots, d);
        {
            let gx = (d as u32).min(32768);
            let gy = (d as u32).div_ceil(gx);
            let dims = [d as u32, dn_k as u32, (dn_k / 32) as u32, gx, 0u32, d as u32, 1u32, 0u32];
            self.dispatch(sh_dn, &[&hmid, dn_qw, dn_qs, &selw], &dn_out, bytemuck::cast_slice(&dims), (gx, gy, slots as u32));
        }
        // 6) weighted combine + residual
        let out = self.alloc(1, d);
        let wd = [d as u32, slots as u32, 0, 0];
        self.dispatch(MOE_WSUM, &[&dn_out, &selw, x2], &out, bytemuck::cast_slice(&wd), wg1(d as u32));
        Some((out, selw))
    }

    /// Startup submit-stride probe: measure the SUBMIT_PROFILES ladder with synthetic chained dispatches
    /// (the decode regime — each depends on the previous) and return (candidate, ms) sorted fastest-first.
    /// The per-submit sync tax is machine-specific (Asahi ~0.5 ms, Metal far less), so the best
    /// passes-per-submit is too; probing at load lets every system find its stride immediately instead of
    /// converging over ~a dozen live replies. The ceiling is one submit per TOKEN regardless (the logits
    /// readback forces it), so past ~dispatches-per-token the ladder flattens — the probe sees that too.
    /// Cost: ~256 tiny dispatches per candidate (≈1 s worst-case on Asahi, once per GPU until cached).
    pub fn submit_stride_probe(&self, cands: &[u32]) -> Vec<(u32, f64)> {
        let n = 2048usize;
        let a = self.upload(&vec![1.0f32; n], 1, n);
        let b = self.upload(&vec![0.5f32; n], 1, n);
        let keep = self.flush_every();
        let mut out: Vec<(u32, f64)> = cands.iter().map(|&c| {
            self.set_flush_every(c);
            let mut chain = self.add_t(&a, &b);
            let t0 = std::time::Instant::now();
            for _ in 0..256 { chain = self.add_t(&chain, &b); }
            let _ = self.download(&chain); // final submit + poll, like a token boundary
            (c, t0.elapsed().as_secs_f64() * 1000.0)
        }).collect();
        self.set_flush_every(keep);
        out.sort_by(|x, y| x.1.total_cmp(&y.1));
        out
    }

    /// Dispatch-latency microbench (LOGSCOPE_DISPATCH_BENCH=1, printed once after load): times 256 CHAINED
    /// tiny dispatches (each reads the previous output — hazard-serialized, the decode regime) vs 256
    /// INDEPENDENT ones (parallelizable) vs 8 big matvec-sized ones, all in one submit each. Splits the
    /// per-dispatch cost into launch latency vs serialization vs real compute — the number that decides
    /// whether kernel FUSION is the right next investment (decode is currently ~0.4-0.5 ms × dispatch
    /// count on Honeykrisp by inference; this measures it).
    pub fn dispatch_bench(&self) {
        let n = 4096usize;
        let a = self.upload(&vec![1.0f32; n], 1, n);
        let b = self.upload(&vec![2.0f32; n], 1, n);
        let mut chain = self.add_t(&a, &b);
        let t0 = std::time::Instant::now();
        for _ in 0..256 { chain = self.add_t(&chain, &b); }
        let _ = self.download(&chain);
        let chained = t0.elapsed().as_secs_f64() * 1000.0;
        let t1 = std::time::Instant::now();
        let outs: Vec<Tensor> = (0..256).map(|_| self.add_t(&a, &b)).collect();
        let _ = self.download(outs.last().unwrap());
        let indep = t1.elapsed().as_secs_f64() * 1000.0;
        eprintln!("dispatch-bench: 256 chained tiny = {chained:.1} ms ({:.3} ms/dispatch) | 256 independent tiny = {indep:.1} ms ({:.3} ms/dispatch) | serialization tax = {:.3} ms/dispatch",
            chained / 256.0, indep / 256.0, (chained - indep).max(0.0) / 256.0);
        eprintln!("dispatch-bench: decode ≈ dispatches/token × the CHAINED number — if that matches observed ms/token, fusion (fewer, fatter kernels) is the speed lever; if independent ≈ chained, it's pure launch overhead and fan-out parallelism is near-free");
    }

    pub fn add_t(&self, a: &Tensor, b: &Tensor) -> Tensor {
        let out = self.alloc(a.rows, a.cols); let dims = [a.len() as u32, 0, 0, 0];
        self.dispatch(ADD, &[a, b], &out, bytemuck::cast_slice(&dims), wg1(a.len() as u32)); out
    }
    /// AdaLN-Zero modulation: `x*(1+scale)+shift`, `scale`/`shift` a [1,cols] row broadcast over every
    /// row of `x`. `scale`/`shift` come from a small MLP over the timestep+conditioning embedding.
    pub fn dit_modulate(&self, x: &Tensor, scale: &Tensor, shift: &Tensor) -> Tensor {
        let out = self.alloc(x.rows, x.cols);
        let dims = [x.rows as u32, x.cols as u32, 0u32, 0u32];
        self.dispatch(DIT_MODULATE, &[x, scale, shift], &out, bytemuck::cast_slice(&dims), wg1((x.rows * x.cols) as u32)); out
    }
    /// AdaLN-Zero gated residual: `x + gate*y`, `gate` a [1,cols] row broadcast over every row of `y`.
    pub fn dit_gated_add(&self, x: &Tensor, y: &Tensor, gate: &Tensor) -> Tensor {
        let out = self.alloc(x.rows, x.cols);
        let dims = [x.rows as u32, x.cols as u32, 0u32, 0u32];
        self.dispatch(DIT_GATED_ADD, &[x, y, gate], &out, bytemuck::cast_slice(&dims), wg1((x.rows * x.cols) as u32)); out
    }
    /// [h,w,c] image → [(h/patch)*(w/patch), patch*patch*c] patch tokens.
    pub fn patchify(&self, img: &Tensor, h: usize, w: usize, c: usize, patch: usize) -> Tensor {
        let n = (h / patch) * (w / patch); let plen = patch * patch * c;
        let out = self.alloc(n, plen);
        let dims = [h as u32, w as u32, c as u32, patch as u32];
        self.dispatch(PATCHIFY, &[img], &out, bytemuck::cast_slice(&dims), wg1((h * w * c) as u32)); out
    }
    /// Inverse of `patchify`: [(h/patch)*(w/patch), patch*patch*c] tokens → [h,w,c] image.
    pub fn unpatchify(&self, tok: &Tensor, h: usize, w: usize, c: usize, patch: usize) -> Tensor {
        let out = self.alloc(h, w * c);
        let dims = [h as u32, w as u32, c as u32, patch as u32];
        self.dispatch(UNPATCHIFY, &[tok], &out, bytemuck::cast_slice(&dims), wg1((h * w * c) as u32)); out
    }
    /// Add a per-column bias vector (length = `a.cols`) broadcast across every row of `a` [rows, cols].
    /// This is the only delta Qwen2 needs over llama on the Q/K/V projections.
    pub fn add_bias_t(&self, a: &Tensor, bias: &Tensor) -> Tensor {
        let out = self.alloc(a.rows, a.cols);
        let dims = [a.len() as u32, a.cols as u32, 0, 0];
        self.dispatch(ADD_BIAS, &[a, bias], &out, bytemuck::cast_slice(&dims), wg1(a.len() as u32)); out
    }
    pub fn mul_t(&self, a: &Tensor, b: &Tensor) -> Tensor {
        let out = self.alloc(a.rows, a.cols); let dims = [a.len() as u32, 0, 0, 0];
        self.dispatch(MUL, &[a, b], &out, bytemuck::cast_slice(&dims), wg1(a.len() as u32)); out
    }
    pub fn silu_t(&self, x: &Tensor) -> Tensor {
        let out = self.alloc(x.rows, x.cols); let dims = [x.len() as u32, 0, 0, 0];
        self.dispatch(SILU, &[x], &out, bytemuck::cast_slice(&dims), wg1(x.len() as u32)); out
    }
    /// 2D convolution, channel-last, batched. `x` is `[n_batch*h*w, cin]` (rows = flattened n,h,w),
    /// `weight` is the flat `[cout,cin,k,k]` tensor, `bias` is `[1,cout]`. Returns
    /// `[n_batch*hout*wout, cout]` with `hout=(h+2*pad-k)/stride+1` (same formula for `wout`).
    pub fn conv2d(&self, x: &Tensor, weight: &Tensor, bias: &Tensor, n_batch: usize, h: usize, w: usize, cin: usize, cout: usize, k: usize, stride: usize, pad: usize) -> (Tensor, usize, usize) {
        let hout = (h + 2 * pad - k) / stride + 1;
        let wout = (w + 2 * pad - k) / stride + 1;
        let out = self.alloc(n_batch * hout * wout, cout);
        let dims = [n_batch as u32, h as u32, w as u32, cin as u32, cout as u32, k as u32, stride as u32, pad as u32, hout as u32, wout as u32, 0u32, 0u32];
        self.dispatch(CONV2D, &[x, weight, bias], &out, bytemuck::cast_slice(&dims), wg1((n_batch * hout * wout * cout) as u32));
        (out, hout, wout)
    }
    /// GroupNorm, channel-last, batched. `x` is `[n_batch*hw, c]`; `gamma`/`beta` are `[1,c]`. Splits
    /// `c` into `groups` equal chunks and normalizes each `(batch, group)` over its channels × every
    /// spatial position, then applies the per-channel affine. Returns the same shape as `x`.
    pub fn group_norm(&self, x: &Tensor, gamma: &Tensor, beta: &Tensor, n_batch: usize, hw: usize, c: usize, groups: usize, eps: f32) -> Tensor {
        let out = self.alloc(n_batch * hw, c);
        let dims = [n_batch as u32, hw as u32, c as u32, groups as u32, eps.to_bits(), 0u32, 0u32, 0u32];
        self.dispatch(GROUP_NORM, &[x, gamma, beta], &out, bytemuck::cast_slice(&dims), wg1((n_batch * groups) as u32)); out
    }
    /// Nearest-neighbor 2x spatial upsample, channel-last, batched: `[n_batch*h*w, c]` → `[n_batch*2h*2w, c]`.
    pub fn upsample2x(&self, x: &Tensor, n_batch: usize, h: usize, w: usize, c: usize) -> Tensor {
        let out = self.alloc(n_batch * h * 2 * w * 2, c);
        let dims = [n_batch as u32, h as u32, w as u32, c as u32, 0u32, 0u32, 0u32, 0u32];
        self.dispatch(UPSAMPLE2X, &[x], &out, bytemuck::cast_slice(&dims), wg1((n_batch * h * 2 * w * 2 * c) as u32)); out
    }
    /// One VAE decoder residual block: `GN→SiLU→conv3x3→GN→SiLU→conv3x3`, added back to `x` (through a
    /// 1x1 conv first if `cin != cout`, otherwise directly). `groups` is the GroupNorm group count
    /// (applied against `cin` for the first norm, `cout` for the second — real VAE decoders keep the
    /// same group count throughout, so the caller just needs one that divides both).
    pub fn vae_resblock(&self, b: &VaeResBlock, x: &Tensor, n_batch: usize, h: usize, w: usize, cin: usize, cout: usize, groups: usize, eps: f32) -> Tensor {
        let h1 = self.silu_t(&self.group_norm(x, &b.gn1_gamma, &b.gn1_beta, n_batch, h * w, cin, groups, eps));
        let (h1c, _, _) = self.conv2d(&h1, &b.conv1_w, &b.conv1_b, n_batch, h, w, cin, cout, 3, 1, 1);
        let h2 = self.silu_t(&self.group_norm(&h1c, &b.gn2_gamma, &b.gn2_beta, n_batch, h * w, cout, groups, eps));
        let (h2c, _, _) = self.conv2d(&h2, &b.conv2_w, &b.conv2_b, n_batch, h, w, cout, cout, 3, 1, 1);
        match &b.skip {
            Some((sw, sb)) => { let (s, _, _) = self.conv2d(x, sw, sb, n_batch, h, w, cin, cout, 1, 1, 0); self.add_t(&h2c, &s) }
            None => self.add_t(&h2c, x),
        }
    }
    /// One VAE decoder upsample stage: nearest 2x upsample, then a conv3x3 (same channel count in/out)
    /// to smooth/mix — the SD/SDXL-family convention. Returns the new (h, w) alongside the tensor.
    pub fn vae_upsample_stage(&self, x: &Tensor, conv_w: &Tensor, conv_b: &Tensor, n_batch: usize, h: usize, w: usize, c: usize) -> (Tensor, usize, usize) {
        let up = self.upsample2x(x, n_batch, h, w, c);
        self.conv2d(&up, conv_w, conv_b, n_batch, h * 2, w * 2, c, c, 3, 1, 1)
    }
    /// Fused SwiGLU activation: silu(gate)*up in one dispatch (saves a SILU+MUL pair).
    pub fn swiglu_t(&self, gate: &Tensor, up: &Tensor) -> Tensor {
        let out = self.alloc(gate.rows, gate.cols); let dims = [gate.len() as u32, 0, 0, 0];
        self.dispatch(SWIGLU, &[gate, up], &out, bytemuck::cast_slice(&dims), wg1(gate.len() as u32)); out
    }
    /// SwiGLU over a packed `[m, 2h]` gate||up buffer (the fused wgate_up output): reads
    /// both halves of one tensor and writes `[m, h]`. Pairs with the fused projection so
    /// the FFN runs one matmul + one activation instead of two matmuls + silu + mul.
    pub fn swiglu_packed(&self, gu: &Tensor) -> Tensor {
        let (m, h) = (gu.rows, gu.cols / 2);
        let out = self.alloc(m, h); let n = (m * h) as u32;
        let dims = [n, h as u32, 0, 0];
        self.dispatch(SWIGLU_PACKED, &[gu], &out, bytemuck::cast_slice(&dims), wg1(n)); out
    }

    // ─────────────── vision-tower primitives (ViT/CLIP) ───────────────
    /// True LayerNorm over each of `rows` rows of width `cols`: subtract the row mean, divide by the
    /// row std (population variance + eps), then per-channel scale `g` and bias `b`. Distinct from
    /// `rmsnorm_t` (no mean-subtraction, no bias) — CLIP/SigLIP/Qwen2-VL vision towers use LayerNorm.
    pub fn layernorm_t(&self, x: &Tensor, g: &Tensor, b: &Tensor, rows: usize, cols: usize, eps: f32) -> Tensor {
        let out = self.alloc(rows, cols); let dims = [rows as u32, cols as u32, eps.to_bits(), 0];
        self.dispatch(LAYERNORM, &[x, g, b], &out, bytemuck::cast_slice(&dims), ((rows as u32).div_ceil(64), 1, 1)); out
    }
    /// tanh-approx GELU: 0.5x(1+tanh(√(2/π)(x+0.044715x³))). The HF/SigLIP `gelu_pytorch_tanh`.
    pub fn gelu_tanh_t(&self, x: &Tensor) -> Tensor {
        let out = self.alloc(x.rows, x.cols); let dims = [x.len() as u32, 0, 0, 0];
        self.dispatch(GELU_TANH, &[x], &out, bytemuck::cast_slice(&dims), wg1(x.len() as u32)); out
    }
    /// QuickGELU: x·sigmoid(1.702x). OpenAI CLIP's activation.
    pub fn gelu_quick_t(&self, x: &Tensor) -> Tensor {
        let out = self.alloc(x.rows, x.cols); let dims = [x.len() as u32, 0, 0, 0];
        self.dispatch(GELU_QUICK, &[x], &out, bytemuck::cast_slice(&dims), wg1(x.len() as u32)); out
    }
    /// Bidirectional (non-causal) multi-head attention — the ViT twin of `attention_mh_t`: every
    /// token attends over ALL `t` tokens (no causal `j<=i` mask). GQA-capable but vision towers pass
    /// `n_kv_heads == n_heads`. Scale 1/√head_dim. Returns `[t, dm]`.
    pub fn attention_mh_full_t(&self, q: &Tensor, k: &Tensor, v: &Tensor, t: usize, dm: usize, n_heads: usize, n_kv_heads: usize) -> Tensor {
        let out = self.alloc(t, dm); let hd = dm / n_heads;
        let dims = [t as u32, hd as u32, n_heads as u32, n_kv_heads as u32];
        self.dispatch(ATTENTION_MH_FULL, &[q, k, v], &out, bytemuck::cast_slice(&dims), (((t * n_heads) as u32).div_ceil(64), 1, 1)); out
    }
    /// One Z-Image/DiT AdaLN-Zero block over `t` patch tokens of width `d`. `cond` is the `[1,d]`
    /// timestep+text conditioning vector (already-summed, pre-SiLU); `ln_one`/`ln_zero` are shared
    /// `[1,d]` all-ones/all-zeros tensors standing in for LayerNorm's (absent) affine — real DiT norms
    /// here are non-affine, AdaLN supplies the scale/shift instead. Attention is bidirectional (no
    /// causal mask — every patch token can see every other patch token). --gate-dit-block validates the
    /// assembly (modulation split, gated-residual placement, non-affine norm) vs a CPU reference.
    pub fn dit_block(&self, b: &DitBlock, x: &Tensor, cond: &Tensor, ln_one: &Tensor, ln_zero: &Tensor, t: usize, d: usize, n_heads: usize, eps: f32) -> Tensor {
        let cact = self.silu_t(cond);
        let modv = b.w_mod.apply(self, &cact, 1); // [1, 6d]: shift1|scale1|gate1|shift2|scale2|gate2
        let shift1 = self.slice_cols(&modv, 0, d);
        let scale1 = self.slice_cols(&modv, d, d);
        let gate1 = self.slice_cols(&modv, 2 * d, d);
        let shift2 = self.slice_cols(&modv, 3 * d, d);
        let scale2 = self.slice_cols(&modv, 4 * d, d);
        let gate2 = self.slice_cols(&modv, 5 * d, d);

        let h1 = self.layernorm_t(x, ln_one, ln_zero, t, d, eps);
        let h1m = self.dit_modulate(&h1, &scale1, &shift1);
        let q = b.wq.apply(self, &h1m, t);
        let k = b.wk.apply(self, &h1m, t);
        let v = b.wv.apply(self, &h1m, t);
        let attn = self.attention_mh_full_t(&q, &k, &v, t, d, n_heads, n_heads);
        let o = b.wo.apply(self, &attn, t);
        let x2 = self.dit_gated_add(x, &o, &gate1);

        let h2 = self.layernorm_t(&x2, ln_one, ln_zero, t, d, eps);
        let h2m = self.dit_modulate(&h2, &scale2, &shift2);
        let f1 = b.fc1.apply(self, &h2m, t);
        let act = self.gelu_tanh_t(&f1);
        let f2 = b.fc2.apply(self, &act, t);
        self.dit_gated_add(&x2, &f2, &gate2)
    }
    /// One pre-norm ViT/CLIP encoder layer over `t` tokens of width `d` with `n_heads` heads.
    /// `quick_gelu` selects OpenAI-CLIP's QuickGELU vs the tanh GELU. Returns [t, d]. All ops are
    /// gated primitives (layernorm_t, apply, attention_mh_full_t, gelu_*_t, add_t); --gate-vit-block
    /// validates the assembly (residual placement, norm order, attention scale) vs a CPU reference.
    pub fn vit_block(&self, b: &VisionBlock, x: &Tensor, t: usize, d: usize, n_heads: usize, eps: f32, quick_gelu: bool) -> Tensor {
        let h1 = self.layernorm_t(x, &b.g1, &b.b1, t, d, eps);
        let q = b.wq.apply(self, &h1, t);
        let k = b.wk.apply(self, &h1, t);
        let v = b.wv.apply(self, &h1, t);
        let attn = self.attention_mh_full_t(&q, &k, &v, t, d, n_heads, n_heads);
        let o = b.wo.apply(self, &attn, t);
        let x2 = self.add_t(x, &o);
        let h2 = self.layernorm_t(&x2, &b.g2, &b.b2, t, d, eps);
        let f1 = b.fc1.apply(self, &h2, t);
        let act = if quick_gelu { self.gelu_quick_t(&f1) } else { self.gelu_tanh_t(&f1) };
        let f2 = b.fc2.apply(self, &act, t);
        self.add_t(&x2, &f2)
    }

    /// One Llama-style transformer block, fully resident (weights uploaded once,
    /// no intermediate leaves the GPU). `n_heads` splits `d` into attention heads.
    pub fn block_forward(&self, w: &BlockWeights, x: &Tensor, ff: &Tensor, t: usize, d: usize, head_dim: usize, n_heads: usize, n_kv_heads: usize, base: f32, eps: f32) -> Tensor {
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd; // ad = attention width (== d for plain MHA)
        let n1 = self.rmsnorm_t(x, &w.g1, t, d, eps);
        let q = self.rope_mh_t(&self.qk_norm(w.wq.apply(self, &n1, t), &w.q_norm, t, ad, hd, eps), ff, t, ad, base, 0, hd);
        let k = self.rope_mh_t(&self.qk_norm(w.wk.apply(self, &n1, t), &w.k_norm, t, kv, hd, eps), ff, t, kv, base, 0, hd);
        let v = w.wv.apply(self, &n1, t);
        let attn = self.attention_mh_t(&q, &k, &v, t, ad, n_heads, n_kv_heads);
        let o = w.wo.apply(self, &attn, t); // wo: [d, ad] projects the attention width back to d
        let x2 = self.add_t(x, &o);
        self.ffn_apply(w, &x2, t, d, eps)
    }

    /// Prefill block: full causal attention over `t` tokens like `block_forward`, but
    /// the per-position K/V are also written into the cache (rows 0..t) so incremental
    /// decode can continue from position `t`. K is rope'd per-row at positions 0..t-1,
    /// which is exactly what sequential decode would have stored.
    fn block_prefill(&self, w: &BlockWeights, x: &Tensor, ff: &Tensor, kc: &Tensor, vc: &Tensor, t: usize, d: usize, head_dim: usize, n_heads: usize, n_kv_heads: usize, base: f32, eps: f32) -> Tensor {
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd;
        let n1 = self.rmsnorm_t(x, &w.g1, t, d, eps);
        let q = self.rope_mh_t(&self.qk_norm(w.wq.apply(self, &n1, t), &w.q_norm, t, ad, hd, eps), ff, t, ad, base, 0, hd);
        let k = self.rope_mh_t(&self.qk_norm(w.wk.apply(self, &n1, t), &w.k_norm, t, kv, hd, eps), ff, t, kv, base, 0, hd);
        let v = w.wv.apply(self, &n1, t);
        self.write_rows(kc, &k, 0, t, kv);
        self.write_rows(vc, &v, 0, t, kv);
        let attn = self.attention_mh_t(&q, &k, &v, t, ad, n_heads, n_kv_heads);
        let o = w.wo.apply(self, &attn, t);
        let x2 = self.add_t(x, &o);
        self.ffn_apply(w, &x2, t, d, eps)
    }

    /// Prefill-extend a block: `x` is the segment `[s, d]` at global positions
    /// `[base, base+s)`; its K/V are rope'd at those positions and appended to the cache
    /// rows `[base, base+s)`, then segment attention runs over the full `[0, base+s)` cache.
    /// Mirrors `block_prefill` but starting from a non-zero base with a populated cache.
    fn block_prefill_ext(&self, w: &BlockWeights, x: &Tensor, ff: &Tensor, kc: &Tensor, vc: &Tensor, base: usize, s: usize, d: usize, head_dim: usize, n_heads: usize, n_kv_heads: usize, base_freq: f32, eps: f32) -> Tensor {
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd;
        let n1 = self.rmsnorm_t(x, &w.g1, s, d, eps);
        let q = self.rope_mh_t(&self.qk_norm(w.wq.apply(self, &n1, s), &w.q_norm, s, ad, hd, eps), ff, s, ad, base_freq, base, hd);
        let k = self.rope_mh_t(&self.qk_norm(w.wk.apply(self, &n1, s), &w.k_norm, s, kv, hd, eps), ff, s, kv, base_freq, base, hd);
        let v = w.wv.apply(self, &n1, s);
        self.write_rows(kc, &k, base, s, kv);
        self.write_rows(vc, &v, base, s, kv);
        let attn = self.attention_mh_ext(&q, kc, vc, base, s, ad, n_heads, n_kv_heads);
        let o = w.wo.apply(self, &attn, s);
        let x2 = self.add_t(x, &o);
        self.ffn_apply(w, &x2, s, d, eps)
    }
    /// Batched prefill: run the whole prompt through every layer in one `[t,d]` pass,
    /// filling the KV cache, and return the LAST token's logits `[vocab]`. Equivalent
    /// to T sequential `decode_step`s but a single pass — and it projects only the last
    /// row to logits instead of all T (the per-token output projection sequential
    /// prefill wastes on the discarded rows).
    pub fn prefill_batched(&self, m: &Model, c: &KvCache, prompt: &[u32]) -> Vec<f32> {
        let t = prompt.len();
        if t == 0 { return Vec::new(); }
        let mut xh = Vec::with_capacity(t * m.d);
        for &id in prompt { let s = id as usize * m.d; xh.extend_from_slice(&m.embed[s..s + m.d]); }
        let mut x = self.upload(&xh, t, m.d);
        for li in 0..m.n_layers {
            x = self.block_prefill(&m.layers[li], &x, &m.rope_factors, &c.k[li], &c.v[li], t, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps);
        }
        let xn = self.rmsnorm_t(&x, &m.out_norm, t, m.d, m.eps);
        let last = self.alloc(1, m.d);
        self.copy_row(&last, 0, &xn, t - 1, m.d);
        self.download(&m.output.apply(self, &last, 1))
    }

    /// Memory-bounded prefill: process the prompt in blocks of `chunk` via `block_prefill_ext` at
    /// increasing offsets, so the attention scratch peaks at O(chunk · T) instead of the O(T²) a
    /// single `prefill_batched` materializes. A long pasted context (tens of thousands of tokens)
    /// no longer tries to allocate tens of GB and OOM the device. Last-token logits are bit-
    /// identical to `prefill_batched` (each block attends causally to the cache the prior blocks
    /// filled). Only the final block's last row is projected, matching `prefill_batched`.
    pub fn prefill_chunked(&self, m: &Model, c: &KvCache, prompt: &[u32], chunk: usize) -> Vec<f32> {
        let t = prompt.len();
        if t == 0 { return Vec::new(); }
        let chunk = chunk.max(1);
        let mut logits = Vec::new();
        let mut start = 0;
        while start < t {
            let end = (start + chunk).min(t);
            let block = &prompt[start..end];
            let s = block.len();
            let mut xh = Vec::with_capacity(s * m.d);
            for &id in block { let st = id as usize * m.d; xh.extend_from_slice(&m.embed[st..st + m.d]); }
            let mut x = self.upload(&xh, s, m.d);
            for li in 0..m.n_layers {
                x = self.block_prefill_ext(&m.layers[li], &x, &m.rope_factors, &c.k[li], &c.v[li],
                                           start, s, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps);
            }
            if end == t {
                let xn = self.rmsnorm_t(&x, &m.out_norm, s, m.d, m.eps);
                let last = self.alloc(1, m.d);
                self.copy_row(&last, 0, &xn, s - 1, m.d);
                logits = self.download(&m.output.apply(self, &last, 1));
            }
            start = end;
        }
        logits
    }
}

const GEMM: &str = r#"
struct D { m:u32,n:u32,k:u32,_p:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> b: array<f32>;
@group(0)@binding(2) var<storage,read_write> c: array<f32>;
@group(0)@binding(3) var<uniform> d: D;
const TILE:u32=16u;
var<workgroup> as_: array<f32,256>; var<workgroup> bs: array<f32,256>;
@compute @workgroup_size(16,16,1)
fn main(@builtin(local_invocation_id) l:vec3<u32>, @builtin(workgroup_id) w:vec3<u32>){
  let row=w.y*TILE+l.y; let col=w.x*TILE+l.x; var acc=0.0;
  let nt=(d.k+TILE-1u)/TILE;
  for(var t=0u;t<nt;t=t+1u){
    let ak=t*TILE+l.x; let bk=t*TILE+l.y;
    if(row<d.m && ak<d.k){as_[l.y*TILE+l.x]=a[row*d.k+ak];}else{as_[l.y*TILE+l.x]=0.0;}
    if(bk<d.k && col<d.n){bs[l.y*TILE+l.x]=b[bk*d.n+col];}else{bs[l.y*TILE+l.x]=0.0;}
    workgroupBarrier();
    for(var kk=0u;kk<TILE;kk=kk+1u){acc=acc+as_[l.y*TILE+kk]*bs[kk*TILE+l.x];}
    workgroupBarrier();
  }
  if(row<d.m && col<d.n){c[row*d.n+col]=acc;}
}"#;

const LINEAR: &str = r#"
struct D { m:u32,n:u32,k:u32,_p:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> wt: array<f32>;
@group(0)@binding(2) var<storage,read_write> c: array<f32>;
@group(0)@binding(3) var<uniform> d: D;
const TILE:u32=16u;
var<workgroup> as_: array<f32,256>; var<workgroup> ws: array<f32,256>;
@compute @workgroup_size(16,16,1)
fn main(@builtin(local_invocation_id) l:vec3<u32>, @builtin(workgroup_id) w:vec3<u32>){
  let row=w.y*TILE+l.y; let col=w.x*TILE+l.x; var acc=0.0;
  let nt=(d.k+TILE-1u)/TILE;
  for(var t=0u;t<nt;t=t+1u){
    let ak=t*TILE+l.x; let wk=t*TILE+l.y; let wcol=w.x*TILE+l.x;
    if(row<d.m && ak<d.k){as_[l.y*TILE+l.x]=a[row*d.k+ak];}else{as_[l.y*TILE+l.x]=0.0;}
    if(wcol<d.n && wk<d.k){ws[l.y*TILE+l.x]=wt[wcol*d.k+wk];}else{ws[l.y*TILE+l.x]=0.0;}
    workgroupBarrier();
    for(var kk=0u;kk<TILE;kk=kk+1u){acc=acc+as_[l.y*TILE+kk]*ws[kk*TILE+l.x];}
    workgroupBarrier();
  }
  if(row<d.m && col<d.n){c[row*d.n+col]=acc;}
}"#;

// Decode mat-vec (m=1), coalesced: ONE workgroup per output, the 64 threads split the k
// dimension so consecutive lanes read consecutive weights/activations (coalesced — bandwidth-
// optimal, vs the GEMM's per-thread strided rows), then tree-reduce. 2D grid (col = wg.y*gx +
// wg.x) so n>65535 (the vocab projection) fits the per-dimension workgroup limit. The reduction
// reorders the dot sum, so this is numerically equivalent but not bit-identical (~1e-6).
const MATVEC_Q8_0: &str = r#"
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read_write> c: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> red: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x;
  if(col>=d.n){ return; }
  let wbase=col*(d.k/4u); let sbase=col*d.kb;
  var acc=0.0; var kk=tid;
  loop {
    if(kk>=d.k){break;}
    let scale=qs[sbase+(kk/32u)];
    let word=qw[wbase+(kk/4u)];
    let ub=extractBits(word,(kk%4u)*8u,8u);
    let q=select(i32(ub), i32(ub)-256, ub>=128u);
    acc=acc+a[kk]*scale*f32(q);
    kk=kk+64u;
  }
  red[tid]=acc; workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){ if(tid<s){ red[tid]=red[tid]+red[tid+s]; } workgroupBarrier(); }
  if(tid==0u){ c[col]=red[0]; }
}"#;

// Decode small-batch mat-vec (2<=m<=8): ONE workgroup per output, 64 threads split k. Each
// thread reads a weight value ONCE and accumulates it into all m activation rows — so the
// (memory-bound) weight read is amortized across the batch, like the m=1 mat-vec but for a
// small batch (the speculative-verify block, and batched decode at small N). Reduces per row.
const MATVEC_SMALL_Q8_0: &str = r#"
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;   // [m,k]
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read_write> c: array<f32>; // [m,n]
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> red: array<f32,512>; // up to 8 rows x 64 threads
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x;
  if(col>=d.n){ return; }
  let m=d.m;
  let wbase=col*(d.k/4u); let sbase=col*d.kb;
  var acc=array<f32,8>(0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0);
  var kk=tid;
  loop {
    if(kk>=d.k){break;}
    let scale=qs[sbase+(kk/32u)];
    let word=qw[wbase+(kk/4u)];
    let ub=extractBits(word,(kk%4u)*8u,8u);
    let q=f32(select(i32(ub), i32(ub)-256, ub>=128u))*scale; // weight read once
    for(var r=0u;r<m;r=r+1u){ acc[r]=acc[r]+a[r*d.k+kk]*q; }
    kk=kk+64u;
  }
  for(var r=0u;r<m;r=r+1u){ red[r*64u+tid]=acc[r]; }
  workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){
    if(tid<s){ for(var r=0u;r<m;r=r+1u){ red[r*64u+tid]=red[r*64u+tid]+red[r*64u+tid+s]; } }
    workgroupBarrier();
  }
  if(tid==0u){ for(var r=0u;r<m;r=r+1u){ c[r*d.n+col]=red[r*64u]; } }
}"#;

const LINEAR_Q8_0: &str = r#"
struct D { m:u32,n:u32,k:u32,kb:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;   // [m,k] activations
@group(0)@binding(1) var<storage,read> qw: array<u32>;  // [n,k] int8, 4 packed per u32
@group(0)@binding(2) var<storage,read> qs: array<f32>;  // [n,kb] per-block scales
@group(0)@binding(3) var<storage,read_write> c: array<f32>; // [m,n]
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let col=g.x; let row=g.y;
  if(row>=d.m || col>=d.n){return;}
  let wbase=col*(d.k/4u); let sbase=col*d.kb; let arow=row*d.k;
  var acc=0.0;
  for(var b=0u;b<d.kb;b=b+1u){
    let scale=qs[sbase+b]; let k0=b*32u;
    for(var j=0u;j<32u;j=j+1u){
      let kk=k0+j;
      let word=qw[wbase+(kk/4u)];
      let ub=extractBits(word,(kk%4u)*8u,8u);          // unsigned byte (AGX-safe dynamic offset)
      let q=select(i32(ub), i32(ub)-256, ub>=128u);    // sign-extend i8, no underflow branch
      acc=acc+a[arow+kk]*scale*f32(q);
    }
  }
  c[row*d.n+col]=acc;
}"#;

// PREFILL GEMM, shared-memory tiled, with the workgroup tile as a wgpu OVERRIDE CONSTANT (`override
// TILE`) so the autotuner can specialize pipelines per shape without re-authoring WGSL. Each thread owns
// one output and accumulates k IN ORDER — so every TILE value is bit-exact to every other, and all match
// the naive LINEAR_Q8_0 within float tol (the a·(s·q) vs (a·s)·q association). Workgroup arrays are sized
// for the max tile (32 → 1024); a smaller TILE just uses a corner. Dequant happens on the W-tile load.
const LINEAR_Q8_0_TILED: &str = r#"
override TILE: u32 = 16u;
struct D { m:u32,n:u32,k:u32,kb:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;   // [m,k]
@group(0)@binding(1) var<storage,read> qw: array<u32>;  // [n,k] int8, 4 packed per u32
@group(0)@binding(2) var<storage,read> qs: array<f32>;  // [n,kb] per-block scales
@group(0)@binding(3) var<storage,read_write> c: array<f32>; // [m,n]
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> As: array<f32,1024>; // max tile 32×32; smaller tiles use the leading corner
var<workgroup> Ws: array<f32,1024>;
@compute @workgroup_size(TILE,TILE,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let ty=lid.y; let tx=lid.x;
  let row=wg.y*TILE+ty; let col=wg.x*TILE+tx;
  var acc=0.0;
  let ntiles=(d.k+TILE-1u)/TILE;
  for(var t=0u;t<ntiles;t=t+1u){
    let kt=t*TILE;
    let ak=kt+tx;                                   // A[row][kt+tx]
    As[ty*TILE+tx]=select(0.0, a[row*d.k+ak], row<d.m && ak<d.k);
    let wk=kt+ty;                                   // W[col][kt+ty], dequantized
    var wv=0.0;
    if(col<d.n && wk<d.k){
      let scale=qs[col*d.kb+(wk/32u)];
      let word=qw[col*(d.k/4u)+(wk/4u)];
      let ub=extractBits(word,(wk%4u)*8u,8u);
      let q=select(i32(ub), i32(ub)-256, ub>=128u);
      wv=scale*f32(q);
    }
    Ws[ty*TILE+tx]=wv;
    workgroupBarrier();
    for(var j=0u;j<TILE;j=j+1u){ acc=acc+As[ty*TILE+j]*Ws[j*TILE+tx]; } // k in order
    workgroupBarrier();
  }
  if(row<d.m && col<d.n){ c[row*d.n+col]=acc; }
}"#;

// Register-blocked q8 prefill GEMM: each thread computes a 4×4 micro-tile of C (16 outputs), so each value
// loaded from the shared A/W tiles feeds 4 MACs instead of 1 — the arithmetic-intensity lever that takes a
// GEMM from single-digit % of peak toward tens of % ON NVIDIA/AMD. MEASURED RESULT: on the M2 Ultra (AGX)
// it LOST badly — 170 GFLOP/s vs the many-thread tiled@32's 999 — because AGX hides latency with high
// thread counts (tiled@32 = 1024 threads/wg), not per-thread ILP (RB = 256 threads, 24 live registers). So
// tiled stays the prefill default; RB is kept as a bench/portability option (it may still win on discrete
// GPUs that reward register blocking, which is why it's an autotune-eligible candidate elsewhere).
// Fixed params (no `override`) so it compiles on GL too and stays CI-validatable.
const LINEAR_Q8_0_RB: &str = r#"
const BM:u32=64u; const BN:u32=64u; const BK:u32=16u;
struct D { m:u32,n:u32,k:u32,kb:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read_write> c: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> As: array<f32,1024>; // BM*BK
var<workgroup> Ws: array<f32,1024>; // BK*BN
@compute @workgroup_size(16,16,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let brow=wg.y*BM; let bcol=wg.x*BN;
  let tx=lid.x; let ty=lid.y; let tid=ty*16u+tx;
  // 4×4 register tile as four vec4 rows — NAMED (constant-indexed) so they stay in registers, not spilled
  // local memory (a dynamically-indexed array<f32,16> would spill and kill the whole point of blocking).
  var acc0=vec4<f32>(0.0,0.0,0.0,0.0); var acc1=vec4<f32>(0.0,0.0,0.0,0.0);
  var acc2=vec4<f32>(0.0,0.0,0.0,0.0); var acc3=vec4<f32>(0.0,0.0,0.0,0.0);
  let ntiles=(d.k+BK-1u)/BK;
  for(var t=0u;t<ntiles;t=t+1u){
    let kbase=t*BK;
    for(var i=tid;i<1024u;i=i+256u){
      let r=i/BK; let kk=i%BK; let gk=kbase+kk;
      As[i]=select(0.0, a[(brow+r)*d.k+gk], (brow+r)<d.m && gk<d.k);
    }
    for(var i=tid;i<1024u;i=i+256u){
      let kk=i/BN; let cc=i%BN; let gk=kbase+kk; let gc=bcol+cc;
      var wv=0.0;
      if(gc<d.n && gk<d.k){
        let scale=qs[gc*d.kb+(gk/32u)];
        let word=qw[gc*(d.k/4u)+(gk/4u)];
        let ub=extractBits(word,(gk%4u)*8u,8u);
        let q=select(i32(ub), i32(ub)-256, ub>=128u);
        wv=scale*f32(q);
      }
      Ws[i]=wv;
    }
    workgroupBarrier();
    for(var kk=0u;kk<BK;kk=kk+1u){
      let wb=kk*BN+tx*4u;
      let br=vec4<f32>(Ws[wb],Ws[wb+1u],Ws[wb+2u],Ws[wb+3u]);
      let ab=ty*4u;
      let a0=As[(ab+0u)*BK+kk]; let a1=As[(ab+1u)*BK+kk]; let a2=As[(ab+2u)*BK+kk]; let a3=As[(ab+3u)*BK+kk];
      acc0=acc0+a0*br; acc1=acc1+a1*br; acc2=acc2+a2*br; acc3=acc3+a3*br;
    }
    workgroupBarrier();
  }
  let r0=brow+ty*4u; let c0=bcol+tx*4u;
  if(r0<d.m){ if(c0<d.n){c[r0*d.n+c0]=acc0.x;} if(c0+1u<d.n){c[r0*d.n+c0+1u]=acc0.y;} if(c0+2u<d.n){c[r0*d.n+c0+2u]=acc0.z;} if(c0+3u<d.n){c[r0*d.n+c0+3u]=acc0.w;} }
  if(r0+1u<d.m){ let rr=r0+1u; if(c0<d.n){c[rr*d.n+c0]=acc1.x;} if(c0+1u<d.n){c[rr*d.n+c0+1u]=acc1.y;} if(c0+2u<d.n){c[rr*d.n+c0+2u]=acc1.z;} if(c0+3u<d.n){c[rr*d.n+c0+3u]=acc1.w;} }
  if(r0+2u<d.m){ let rr=r0+2u; if(c0<d.n){c[rr*d.n+c0]=acc2.x;} if(c0+1u<d.n){c[rr*d.n+c0+1u]=acc2.y;} if(c0+2u<d.n){c[rr*d.n+c0+2u]=acc2.z;} if(c0+3u<d.n){c[rr*d.n+c0+3u]=acc2.w;} }
  if(r0+3u<d.m){ let rr=r0+3u; if(c0<d.n){c[rr*d.n+c0]=acc3.x;} if(c0+1u<d.n){c[rr*d.n+c0+1u]=acc3.y;} if(c0+2u<d.n){c[rr*d.n+c0+2u]=acc3.z;} if(c0+3u<d.n){c[rr*d.n+c0+3u]=acc3.w;} }
}"#;

const MATVEC_Q4: &str = r#"
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;  // [n,k] nibbles, 8 packed per u32
@group(0)@binding(2) var<storage,read> qs: array<f32>;  // [n,kb] per-block scales
@group(0)@binding(3) var<storage,read_write> c: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> red: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x;
  if(col>=d.n){ return; }
  let wbase=col*(d.k/8u); let sbase=col*d.kb;
  var acc=0.0; var kk=tid;
  loop {
    if(kk>=d.k){break;}
    let scale=qs[sbase+(kk/32u)];
    let word=qw[wbase+(kk/8u)];
    let nib=extractBits(word,(kk%8u)*4u,4u);   // 0..15 (AGX-safe dynamic offset)
    let q=i32(nib)-8;                          // symmetric: value−8 ∈ [−7,7]
    acc=acc+a[kk]*scale*f32(q);
    kk=kk+64u;
  }
  red[tid]=acc; workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){ if(tid<s){ red[tid]=red[tid]+red[tid+s]; } workgroupBarrier(); }
  if(tid==0u){ c[col]=red[0]; }
}"#;

// Residual-fold variants: identical to MATVEC_Q8_0 / MATVEC_Q4 but write c[col]=dot+resid[col],
// folding the post-projection residual add (wo/wdown) into the matmul epilogue — saves one ADD
// dispatch/layer each on the launch-bound decode path. resid is binding(3); c/dims shift to 4/5.
const MATVEC_Q8_0_ADD: &str = r#"
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read> resid: array<f32>;
@group(0)@binding(4) var<storage,read_write> c: array<f32>;
@group(0)@binding(5) var<uniform> d: D;
var<workgroup> red: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x;
  if(col>=d.n){ return; }
  let wbase=col*(d.k/4u); let sbase=col*d.kb;
  var acc=0.0; var kk=tid;
  loop {
    if(kk>=d.k){break;}
    let scale=qs[sbase+(kk/32u)];
    let word=qw[wbase+(kk/4u)];
    let ub=extractBits(word,(kk%4u)*8u,8u);
    let q=select(i32(ub), i32(ub)-256, ub>=128u);
    acc=acc+a[kk]*scale*f32(q);
    kk=kk+64u;
  }
  red[tid]=acc; workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){ if(tid<s){ red[tid]=red[tid]+red[tid+s]; } workgroupBarrier(); }
  if(tid==0u){ c[col]=red[0]+resid[col]; }
}"#;

const MATVEC_Q4_ADD: &str = r#"
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read> resid: array<f32>;
@group(0)@binding(4) var<storage,read_write> c: array<f32>;
@group(0)@binding(5) var<uniform> d: D;
var<workgroup> red: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x;
  if(col>=d.n){ return; }
  let wbase=col*(d.k/8u); let sbase=col*d.kb;
  var acc=0.0; var kk=tid;
  loop {
    if(kk>=d.k){break;}
    let scale=qs[sbase+(kk/32u)];
    let word=qw[wbase+(kk/8u)];
    let nib=extractBits(word,(kk%8u)*4u,4u);
    let q=i32(nib)-8;
    acc=acc+a[kk]*scale*f32(q);
    kk=kk+64u;
  }
  red[tid]=acc; workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){ if(tid<s){ red[tid]=red[tid]+red[tid+s]; } workgroupBarrier(); }
  if(tid==0u){ c[col]=red[0]+resid[col]; }
}"#;

// Small-batch (2≤m≤8) Q4 mat-vec: one workgroup per output column, 64 threads split k, each weight
// nibble read ONCE and accumulated across all m rows (the bandwidth-bound cost stays near m=1 instead
// of riding the strided LINEAR_Q4, which re-reads the weights per tile). Mirrors MATVEC_SMALL_Q8_0; the
// per-term arithmetic (a*scale*qv) matches MATVEC_Q4 exactly, so each row is BIT-EXACT to the m=1 kernel.
// This is what lets Q4-resident (half the weight bytes of int8) compound with speculative/batched decode.
const MATVEC_SMALL_Q4: &str = r#"
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;   // [m,k]
@group(0)@binding(1) var<storage,read> qw: array<u32>;  // [n,k] nibbles, 8 packed per u32
@group(0)@binding(2) var<storage,read> qs: array<f32>;  // [n,kb] per-block scales
@group(0)@binding(3) var<storage,read_write> c: array<f32>; // [m,n]
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> red: array<f32,512>; // up to 8 rows x 64 threads
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x;
  if(col>=d.n){ return; }
  let m=d.m;
  let wbase=col*(d.k/8u); let sbase=col*d.kb;
  var acc=array<f32,8>(0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0);
  var kk=tid;
  loop {
    if(kk>=d.k){break;}
    let scale=qs[sbase+(kk/32u)];
    let word=qw[wbase+(kk/8u)];
    let nib=extractBits(word,(kk%8u)*4u,4u);   // 0..15
    let qv=f32(i32(nib)-8);                     // symmetric, weight read once
    for(var r=0u;r<m;r=r+1u){ acc[r]=acc[r]+a[r*d.k+kk]*scale*qv; }
    kk=kk+64u;
  }
  for(var r=0u;r<m;r=r+1u){ red[r*64u+tid]=acc[r]; }
  workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){
    if(tid<s){ for(var r=0u;r<m;r=r+1u){ red[r*64u+tid]=red[r*64u+tid]+red[r*64u+tid+s]; } }
    workgroupBarrier();
  }
  if(tid==0u){ for(var r=0u;r<m;r=r+1u){ c[r*d.n+col]=red[r*64u]; } }
}"#;

const LINEAR_Q4: &str = r#"
struct D { m:u32,n:u32,k:u32,kb:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;   // [m,k] activations
@group(0)@binding(1) var<storage,read> qw: array<u32>;  // [n,k] nibbles, 8 packed per u32
@group(0)@binding(2) var<storage,read> qs: array<f32>;  // [n,kb] per-block scales
@group(0)@binding(3) var<storage,read_write> c: array<f32>; // [m,n]
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let col=g.x; let row=g.y;
  if(row>=d.m || col>=d.n){return;}
  let wbase=col*(d.k/8u); let sbase=col*d.kb; let arow=row*d.k;
  var acc=0.0;
  for(var b=0u;b<d.kb;b=b+1u){
    let scale=qs[sbase+b]; let k0=b*32u;
    for(var j=0u;j<32u;j=j+1u){
      let kk=k0+j;
      let word=qw[wbase+(kk/8u)];
      let nib=extractBits(word,(kk%8u)*4u,4u);
      let q=i32(nib)-8;
      acc=acc+a[arow+kk]*scale*f32(q);
    }
  }
  c[row*d.n+col]=acc;
}"#;
// Decode mat-vec, TILED: one workgroup computes `tile` (≤8) output columns, 64 threads split k.
// Each thread reads each activation a[kk] ONCE and reuses it across all `tile` columns — so the
// activation-reread bandwidth (which dominates the FFN mat-vecs, where the vector is far larger than
// one weight row) drops by `tile`×, while every weight is still read exactly once. Per-column
// accumulation is identical to MATVEC_Q8_0, so results are bit-identical to the untiled kernel.
const MATVEC_Q8_0_TILED: &str = r#"
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32,tile:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read_write> c: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> red: array<f32,512>; // 8 columns x 64 threads
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col0=(wg.y*d.gx+wg.x)*d.tile; let tid=lid.x;
  if(col0>=d.n){ return; }
  var acc=array<f32,8>(0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0);
  var kk=tid;
  loop {
    if(kk>=d.k){break;}
    let av=a[kk];                    // ONE activation read, reused across the tile
    let blk=kk/32u; let widx=kk/4u; let bytepos=(kk%4u)*8u;
    for(var ci=0u;ci<d.tile;ci=ci+1u){
      let col=col0+ci;
      if(col<d.n){
        let scale=qs[col*d.kb+blk];
        let word=qw[col*(d.k/4u)+widx];
        let ub=extractBits(word,bytepos,8u);
        let q=select(i32(ub), i32(ub)-256, ub>=128u);
        acc[ci]=acc[ci]+av*scale*f32(q);
      }
    }
    kk=kk+64u;
  }
  for(var ci=0u;ci<d.tile;ci=ci+1u){ red[ci*64u+tid]=acc[ci]; }
  workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){
    if(tid<s){ for(var ci=0u;ci<d.tile;ci=ci+1u){ red[ci*64u+tid]=red[ci*64u+tid]+red[ci*64u+tid+s]; } }
    workgroupBarrier();
  }
  if(tid==0u){ for(var ci=0u;ci<d.tile;ci=ci+1u){ let col=col0+ci; if(col<d.n){ c[col]=red[ci*64u]; } } }
}"#;

// V3 decode mat-vec: 128-bit (vec4<u32>) wide loads — each thread pulls 4 words (16 q8 values) per memory
// transaction instead of one. The profile says decode is latency/occupancy-bound (≈6% BW, ≈1% ALU — the
// GPU idles on memory latency), and wider loads put more bytes in flight per request, the textbook fix.
// Requires k % 16 == 0 (always true for q8: k is a multiple of the 32-wide block). Same math as the others;
// results differ only by f32 reassociation.
const MATVEC_Q8_0_TILED_V3: &str = r#"
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32,tile:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw4: array<vec4<u32>>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read_write> c: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> red: array<f32,512>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col0=(wg.y*d.gx+wg.x)*d.tile; let tid=lid.x;
  if(col0>=d.n){ return; }
  var acc=array<f32,8>(0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0);
  let words4=d.k/16u;               // vec4<u32> per column-row (16 q8 values each)
  var w=tid;
  loop {
    if(w>=words4){break;}
    let kk0=w*16u;
    let blk=kk0/32u;                  // all 16 q8 in a vec4 load share ONE 32-block → ONE scale index
    for(var ci=0u;ci<d.tile;ci=ci+1u){
      let col=col0+ci;
      if(col<d.n){
        let v=qw4[col*words4+w];     // ONE 128-bit load = 16 q8 values
        let base=col*d.kb;
        let sc=qs[base+blk];        // ONE scale load, reused across all 16 lanes (was 16 identical loads)
        for(var jj=0u;jj<4u;jj=jj+1u){
          let word=v[jj];
          let kkw=kk0+jj*4u;
          let u0=extractBits(word,0u,8u);  let u1=extractBits(word,8u,8u);
          let u2=extractBits(word,16u,8u); let u3=extractBits(word,24u,8u);
          let q0=select(i32(u0),i32(u0)-256,u0>=128u);
          let q1=select(i32(u1),i32(u1)-256,u1>=128u);
          let q2=select(i32(u2),i32(u2)-256,u2>=128u);
          let q3=select(i32(u3),i32(u3)-256,u3>=128u);
          acc[ci]=acc[ci]
            + a[kkw]*sc*f32(q0)
            + a[kkw+1u]*sc*f32(q1)
            + a[kkw+2u]*sc*f32(q2)
            + a[kkw+3u]*sc*f32(q3);
        }
      }
    }
    w=w+64u;
  }
  for(var ci=0u;ci<d.tile;ci=ci+1u){ red[ci*64u+tid]=acc[ci]; }
  workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){
    if(tid<s){ for(var ci=0u;ci<d.tile;ci=ci+1u){ red[ci*64u+tid]=red[ci*64u+tid]+red[ci*64u+tid+s]; } }
    workgroupBarrier();
  }
  if(tid==0u){ for(var ci=0u;ci<d.tile;ci=ci+1u){ let col=col0+ci; if(col<d.n){ c[col]=red[ci*64u]; } } }
}"#;

// V2-T16: v2 with acc/red sized for tile up to 16 (more columns share each weight-word read). Costs 4 KiB
// workgroup memory (red[16*64]) — may lower occupancy, so it's a bench candidate vs v2@8, not a given win.
const MATVEC_Q8_0_TILED_V2_T16: &str = r#"
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32,tile:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read_write> c: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> red: array<f32,1024>; // tile(≤16) x 64 threads
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col0=(wg.y*d.gx+wg.x)*d.tile; let tid=lid.x;
  if(col0>=d.n){ return; }
  var acc=array<f32,16>(0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0);
  let words=d.k/4u;
  var w=tid;
  loop {
    if(w>=words){break;}
    let kk0=w*4u;
    let a0=a[kk0]; let a1=a[kk0+1u]; let a2=a[kk0+2u]; let a3=a[kk0+3u];
    let b0=kk0/32u; let b1=(kk0+1u)/32u; let b2=(kk0+2u)/32u; let b3=(kk0+3u)/32u;
    for(var ci=0u;ci<d.tile;ci=ci+1u){
      let col=col0+ci;
      if(col<d.n){
        let word=qw[col*words+w];
        let u0=extractBits(word,0u,8u);  let u1=extractBits(word,8u,8u);
        let u2=extractBits(word,16u,8u); let u3=extractBits(word,24u,8u);
        let q0=select(i32(u0),i32(u0)-256,u0>=128u); let q1=select(i32(u1),i32(u1)-256,u1>=128u);
        let q2=select(i32(u2),i32(u2)-256,u2>=128u); let q3=select(i32(u3),i32(u3)-256,u3>=128u);
        let base=col*d.kb;
        acc[ci]=acc[ci]+a0*qs[base+b0]*f32(q0)+a1*qs[base+b1]*f32(q1)+a2*qs[base+b2]*f32(q2)+a3*qs[base+b3]*f32(q3);
      }
    }
    w=w+64u;
  }
  for(var ci=0u;ci<d.tile;ci=ci+1u){ red[ci*64u+tid]=acc[ci]; }
  workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){
    if(tid<s){ for(var ci=0u;ci<d.tile;ci=ci+1u){ red[ci*64u+tid]=red[ci*64u+tid]+red[ci*64u+tid+s]; } }
    workgroupBarrier();
  }
  if(tid==0u){ for(var ci=0u;ci<d.tile;ci=ci+1u){ let col=col0+ci; if(col<d.n){ c[col]=red[ci*64u]; } } }
}"#;

// V2-WG128: v2 with 128-thread workgroups — more threads per column split the k-reduction, exposing more
// memory parallelism per workgroup to hide latency (at the cost of a 7-step reduction and fewer workgroups).
const MATVEC_Q8_0_TILED_V2_WG128: &str = r#"
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32,tile:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read_write> c: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> red: array<f32,1024>; // tile(≤8) x 128 threads
@compute @workgroup_size(128,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col0=(wg.y*d.gx+wg.x)*d.tile; let tid=lid.x;
  if(col0>=d.n){ return; }
  var acc=array<f32,8>(0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0);
  let words=d.k/4u;
  var w=tid;
  loop {
    if(w>=words){break;}
    let kk0=w*4u;
    let a0=a[kk0]; let a1=a[kk0+1u]; let a2=a[kk0+2u]; let a3=a[kk0+3u];
    let blk=kk0/32u; // 4 consecutive k (kk0..kk0+3) always share ONE 32-block, so ONE scale index
    for(var ci=0u;ci<d.tile;ci=ci+1u){
      let col=col0+ci;
      if(col<d.n){
        let word=qw[col*words+w];
        let u0=extractBits(word,0u,8u);  let u1=extractBits(word,8u,8u);
        let u2=extractBits(word,16u,8u); let u3=extractBits(word,24u,8u);
        let q0=select(i32(u0),i32(u0)-256,u0>=128u); let q1=select(i32(u1),i32(u1)-256,u1>=128u);
        let q2=select(i32(u2),i32(u2)-256,u2>=128u); let q3=select(i32(u3),i32(u3)-256,u3>=128u);
        let sc=qs[col*d.kb+blk]; // ONE scale load, reused across all 4 lanes (was 4 identical loads)
        acc[ci]=acc[ci]+a0*sc*f32(q0)+a1*sc*f32(q1)+a2*sc*f32(q2)+a3*sc*f32(q3);
      }
    }
    w=w+128u;
  }
  for(var ci=0u;ci<d.tile;ci=ci+1u){ red[ci*128u+tid]=acc[ci]; }
  workgroupBarrier();
  for(var s=64u;s>0u;s=s>>1u){
    if(tid<s){ for(var ci=0u;ci<d.tile;ci=ci+1u){ red[ci*128u+tid]=red[ci*128u+tid]+red[ci*128u+tid+s]; } }
    workgroupBarrier();
  }
  if(tid==0u){ for(var ci=0u;ci<d.tile;ci=ci+1u){ let col=col0+ci; if(col<d.n){ c[col]=red[ci*128u]; } } }
}"#;

// V2-WG128-SG: v2-wg128 with a SUBGROUP reduction. subgroupAdd collapses each 32-lane subgroup in one op
// (no barriers), and only the per-subgroup partials (nsg × tile) touch workgroup memory — vs the full
// red[tile*128] array + 7-step barrier tree. That frees shared memory and removes barriers, lifting
// occupancy — the diagnosed decode bottleneck. Requires the SUBGROUP device feature (Gpu.subgroups).
const MATVEC_Q8_0_TILED_V2_WG128_SG: &str = r#"
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32,tile:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read_write> c: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> part: array<f32,64>; // nsg(≤8) x tile(≤8) partials — tiny vs red[tile*128]
@compute @workgroup_size(128,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>,
        @builtin(subgroup_invocation_id) sglid:u32, @builtin(subgroup_size) sgsz:u32){
  let col0=(wg.y*d.gx+wg.x)*d.tile; let tid=lid.x;
  if(col0>=d.n){ return; }
  var acc=array<f32,8>(0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0);
  let words=d.k/4u;
  var w=tid;
  loop {
    if(w>=words){break;}
    let kk0=w*4u;
    let a0=a[kk0]; let a1=a[kk0+1u]; let a2=a[kk0+2u]; let a3=a[kk0+3u];
    let blk=kk0/32u; // 4 consecutive k share ONE 32-block → ONE scale index (was b0..b3, all equal)
    for(var ci=0u;ci<d.tile;ci=ci+1u){
      let col=col0+ci;
      if(col<d.n){
        let word=qw[col*words+w];
        let u0=extractBits(word,0u,8u);  let u1=extractBits(word,8u,8u);
        let u2=extractBits(word,16u,8u); let u3=extractBits(word,24u,8u);
        let q0=select(i32(u0),i32(u0)-256,u0>=128u); let q1=select(i32(u1),i32(u1)-256,u1>=128u);
        let q2=select(i32(u2),i32(u2)-256,u2>=128u); let q3=select(i32(u3),i32(u3)-256,u3>=128u);
        let sc=qs[col*d.kb+blk]; // ONE scale load, reused across all 4 lanes
        acc[ci]=acc[ci]+a0*sc*f32(q0)+a1*sc*f32(q1)+a2*sc*f32(q2)+a3*sc*f32(q3);
      }
    }
    w=w+128u;
  }
  let sgid=tid/sgsz;            // which subgroup
  let nsg=128u/sgsz;           // subgroups per workgroup (4 at size 32)
  for(var ci=0u;ci<d.tile;ci=ci+1u){
    let s=subgroupAdd(acc[ci]); // reduce this subgroup's 32 lanes in one op, no barrier
    if(sglid==0u){ part[sgid*d.tile+ci]=s; }
  }
  workgroupBarrier();
  if(tid==0u){
    for(var ci=0u;ci<d.tile;ci=ci+1u){
      var sum=0.0;
      for(var g=0u;g<nsg;g=g+1u){ sum=sum+part[g*d.tile+ci]; }
      let col=col0+ci; if(col<d.n){ c[col]=sum; }
    }
  }
}"#;

// V2 decode mat-vec: each thread owns WHOLE 32-bit weight words (4 quantized values), reading each word
// exactly once and using all four bytes — vs the original, where 4 threads read the SAME word (widx=kk/4)
// and extract one byte each. On drivers that don't coalesce same-address subgroup loads (a real risk on
// Asahi/AGX), that was up to 4× redundant weight traffic — the leading suspect for the decode running at
// ~6% of memory bandwidth. Fully coalesced (64 threads → 64 consecutive words). Bit-results differ from the
// original only by f32 reassociation (4-per-word accumulation order), far below q8 quant error.
const MATVEC_Q8_0_TILED_V2: &str = r#"
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32,tile:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read_write> c: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
var<workgroup> red: array<f32,512>; // tile(≤8) x 64 threads
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col0=(wg.y*d.gx+wg.x)*d.tile; let tid=lid.x;
  if(col0>=d.n){ return; }
  var acc=array<f32,8>(0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0);
  let words=d.k/4u;             // 32-bit words per column-row (4 q8 values each)
  var w=tid;                    // each thread owns whole words → one global read per word, no redundancy
  loop {
    if(w>=words){break;}
    let kk0=w*4u;
    let a0=a[kk0]; let a1=a[kk0+1u]; let a2=a[kk0+2u]; let a3=a[kk0+3u];
    let b0=kk0/32u; let b1=(kk0+1u)/32u; let b2=(kk0+2u)/32u; let b3=(kk0+3u)/32u; // block index per value
    for(var ci=0u;ci<d.tile;ci=ci+1u){
      let col=col0+ci;
      if(col<d.n){
        let word=qw[col*words+w];         // ONE read, all four bytes consumed
        let u0=extractBits(word,0u,8u);  let u1=extractBits(word,8u,8u);
        let u2=extractBits(word,16u,8u); let u3=extractBits(word,24u,8u);
        let q0=select(i32(u0),i32(u0)-256,u0>=128u);
        let q1=select(i32(u1),i32(u1)-256,u1>=128u);
        let q2=select(i32(u2),i32(u2)-256,u2>=128u);
        let q3=select(i32(u3),i32(u3)-256,u3>=128u);
        let base=col*d.kb;
        acc[ci]=acc[ci]
          + a0*qs[base+b0]*f32(q0)
          + a1*qs[base+b1]*f32(q1)
          + a2*qs[base+b2]*f32(q2)
          + a3*qs[base+b3]*f32(q3);
      }
    }
    w=w+64u;
  }
  for(var ci=0u;ci<d.tile;ci=ci+1u){ red[ci*64u+tid]=acc[ci]; }
  workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){
    if(tid<s){ for(var ci=0u;ci<d.tile;ci=ci+1u){ red[ci*64u+tid]=red[ci*64u+tid]+red[ci*64u+tid+s]; } }
    workgroupBarrier();
  }
  if(tid==0u){ for(var ci=0u;ci<d.tile;ci=ci+1u){ let col=col0+ci; if(col<d.n){ c[col]=red[ci*64u]; } } }
}"#;

const DEQUANT_Q4_0: &str = r#"
struct D { nw:u32,_a:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> q: array<u32>;
@group(0)@binding(1) var<storage,read> s: array<f32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let i=g.x; if(i>=d.nw){return;}
  let blk=i/32u; let within=i%32u;
  // Plain branches (no `select`): the false-branch of a `select` is still
  // evaluated, and `within-16u` underflows for within<16 — harmless on llvmpipe
  // but mis-lowered on some younger Vulkan drivers (e.g. Mesa's Asahi/AGX
  // Apple-GPU backend). Guarding the subtraction behind the condition is portable.
  var j:u32; var hi:bool;
  if(within<16u){ j=within; hi=false; } else { j=within-16u; hi=true; }
  let gb=blk*16u+j;
  let word=q[gb/4u];
  // extractBits (canonical WGSL bitfield op → NIR bitfield_extract) instead of
  // hand-rolled >>/& — the manual dynamic shift is the only such construct in
  // these kernels and a prime suspect for the Asahi/Mesa (AGX) mismatch.
  let byte=extractBits(word, (gb%4u)*8u, 8u);
  var nib:u32;
  if(hi){ nib=extractBits(byte, 4u, 4u); } else { nib=extractBits(byte, 0u, 4u); }
  o[i]=(f32(nib)-8.0)*s[blk];
}"#;

const SOFTMAX: &str = r#"
struct D { rows:u32,cols:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let r=g.x; if(r>=d.rows){return;}
  let base=r*d.cols; var mx=-3.4e38;
  for(var c=0u;c<d.cols;c=c+1u){mx=max(mx,x[base+c]);}
  var sum=0.0;
  for(var c=0u;c<d.cols;c=c+1u){let e=exp(x[base+c]-mx); o[base+c]=e; sum=sum+e;}
  for(var c=0u;c<d.cols;c=c+1u){o[base+c]=o[base+c]/sum;}
}"#;

const RMSNORM: &str = r#"
struct D { rows:u32,cols:u32,eps:f32,_p:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> wt: array<f32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let r=g.x; if(r>=d.rows){return;}
  let base=r*d.cols; var ss=0.0;
  for(var c=0u;c<d.cols;c=c+1u){let v=x[base+c]; ss=ss+v*v;}
  let inv=1.0/sqrt(ss/f32(d.cols)+d.eps);
  for(var c=0u;c<d.cols;c=c+1u){o[base+c]=x[base+c]*inv*wt[c];}
}"#;

const ATTENTION: &str = r#"
struct D { t:u32,d:u32,causal:u32,_p:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> k: array<f32>;
@group(0)@binding(2) var<storage,read> v: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let i=g.x; if(i>=dd.t){return;}
  let dim=dd.d; let scale=1.0/sqrt(f32(dim));
  let last=select(dd.t-1u, i, dd.causal==1u);
  var mx=-3.4e38;
  for(var jj=0u;jj<=last;jj=jj+1u){
    var dot=0.0;
    for(var c=0u;c<dim;c=c+1u){dot=dot+q[i*dim+c]*k[jj*dim+c];}
    mx=max(mx,dot*scale);
  }
  for(var c=0u;c<dim;c=c+1u){o[i*dim+c]=0.0;}
  var sum=0.0;
  for(var jj=0u;jj<=last;jj=jj+1u){
    var dot=0.0;
    for(var c=0u;c<dim;c=c+1u){dot=dot+q[i*dim+c]*k[jj*dim+c];}
    let p=exp(dot*scale-mx); sum=sum+p;
    for(var c=0u;c<dim;c=c+1u){o[i*dim+c]=o[i*dim+c]+p*v[jj*dim+c];}
  }
  for(var c=0u;c<dim;c=c+1u){o[i*dim+c]=o[i*dim+c]/sum;}
}"#;

const ROPE: &str = r#"
struct D { t:u32,d:u32,base:f32,pos0:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let half=dd.d/2u; let p=g.x; if(p>=dd.t*half){return;}
  let row=p/half; let i=p%half;
  let freq=f32(row+dd.pos0)/pow(dd.base,(2.0*f32(i))/f32(dd.d));
  let c=cos(freq); let s=sin(freq);
  let d0=row*dd.d+2u*i;
  let x0=x[d0]; let x1=x[d0+1u];
  o[d0]=x0*c-x1*s;
  o[d0+1u]=x0*s+x1*c;
}"#;

// ===== MoE v2 decode kernels: GPU-side routing (zero readbacks) =====
// MOE_TOPK: serial top-k over the router logits (n<=512, k<=16 — trivial work, the win is staying on-GPU).
// Selection order = descending logit; weights = softmax over the SELECTED logits (bit-faithful to
// moe::route's Softmax path: global softmax renormalized to the top-k == softmax over the selected).
// Output selw: [0..k) = expert index (u32), [k..2k) = combine weight (f32 bitcast to u32).
const MOE_TOPK: &str = r#"
struct D { n:u32,k:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> logits: array<f32>;
@group(0)@binding(1) var<storage,read_write> selw: array<u32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(1,1,1)
fn main(){
  var idx: array<u32,16>; var lg: array<f32,16>;
  for(var s=0u;s<d.k;s=s+1u){
    var bi=0u; var bv=-3.4e38;
    for(var i=0u;i<d.n;i=i+1u){
      var taken=false;
      for(var t=0u;t<s;t=t+1u){ if(idx[t]==i){ taken=true; } }
      if(!taken && logits[i]>bv){ bv=logits[i]; bi=i; }
    }
    idx[s]=bi; lg[s]=bv;
  }
  // softmax over the selected (selection is descending, so lg[0] is the max — hoist it BEFORE the loop
  // mutates lg in place; using lg[0] inside the loop reads exp(0)=1.0 for s>=1, mis-weighting slot 0 by
  // exp(max-1) after normalization — the exact wgtΔmax the differential checker caught)
  let mx=lg[0];
  var sum=0.0;
  for(var s=0u;s<d.k;s=s+1u){ lg[s]=exp(lg[s]-mx); sum=sum+lg[s]; }
  for(var s=0u;s<d.k;s=s+1u){ selw[s]=idx[s]; selw[d.k+s]=bitcast<u32>(lg[s]/sum); }
}"#;

// MOE_MATVEC_Q8 / MOE_MATVEC_Q4: the base MATVEC_Q8_0/MATVEC_Q4 kernels with the weight row indexed
// through a GPU-side expert selection — row = sel[slot]*rstride + col — so the CPU never learns which
// experts won (no readback). `xper=1` reads a per-slot activation row (the down projection consumes
// hmid[slot]); `xper=0` broadcasts one activation row (gate_up consumes n2). Output c[slot*n + col].
const MOE_MATVEC_Q8: &str = r#"
struct D { n:u32,k:u32,kb:u32,gx:u32,_slot:u32,rstride:u32,xper:u32,_p:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read> selw: array<u32>;
@group(0)@binding(4) var<storage,read_write> c: array<f32>;
@group(0)@binding(5) var<uniform> d: D;
var<workgroup> red: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x; let slot=wg.z;
  if(col>=d.n){ return; }
  let row=selw[slot]*d.rstride+col;
  let wbase=row*(d.k/4u); let sbase=row*d.kb;
  let abase=select(0u, slot*d.k, d.xper==1u);
  var acc=0.0; var kk=tid;
  loop {
    if(kk>=d.k){break;}
    let scale=qs[sbase+(kk/32u)];
    let word=qw[wbase+(kk/4u)];
    let ub=extractBits(word,(kk%4u)*8u,8u);
    let q=select(i32(ub), i32(ub)-256, ub>=128u);
    acc=acc+a[abase+kk]*scale*f32(q);
    kk=kk+64u;
  }
  red[tid]=acc; workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){ if(tid<s){ red[tid]=red[tid]+red[tid+s]; } workgroupBarrier(); }
  if(tid==0u){ c[slot*d.n+col]=red[0]; }
}"#;

const MOE_MATVEC_Q4: &str = r#"
struct D { n:u32,k:u32,kb:u32,gx:u32,_slot:u32,rstride:u32,xper:u32,_p:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read> selw: array<u32>;
@group(0)@binding(4) var<storage,read_write> c: array<f32>;
@group(0)@binding(5) var<uniform> d: D;
var<workgroup> red: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x; let slot=wg.z;
  if(col>=d.n){ return; }
  let row=selw[slot]*d.rstride+col;
  let wbase=row*(d.k/8u); let sbase=row*d.kb;
  let abase=select(0u, slot*d.k, d.xper==1u);
  var acc=0.0; var kk=tid;
  loop {
    if(kk>=d.k){break;}
    let scale=qs[sbase+(kk/32u)];
    let word=qw[wbase+(kk/8u)];
    let nib=extractBits(word,(kk%8u)*4u,4u);
    let q=i32(nib)-8;
    acc=acc+a[abase+kk]*scale*f32(q);
    kk=kk+64u;
  }
  red[tid]=acc; workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){ if(tid<s){ red[tid]=red[tid]+red[tid+s]; } workgroupBarrier(); }
  if(tid==0u){ c[slot*d.n+col]=red[0]; }
}"#;

// MOE_WSUM: out[col] = resid[col] + Σ_s weight[s] · down[s·d + col] — the routed-expert combine, folded
// with the residual add. Weights come from MOE_TOPK's selw (f32 bitcast at [k..2k)).
const MOE_WSUM: &str = r#"
struct D { n:u32,k:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> src: array<f32>;
@group(0)@binding(1) var<storage,read> selw: array<u32>;
@group(0)@binding(2) var<storage,read> resid: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; if(i>=d.n){return;}
  var acc=resid[i];
  for(var s=0u;s<d.k;s=s+1u){ acc=acc+bitcast<f32>(selw[d.k+s])*src[s*d.n+i]; }
  o[i]=acc;
}"#;

// Fused QK-Norm + RoPE for the decode step (t=1): ONE dispatch replaces the 4-chain
// (qk_norm(q) → qk_norm(k) → rope(q) → rope(k)) on the Qwen path. One workgroup per HEAD (q heads then
// k heads): 64 threads reduce the head's RMS (hd=128 → 2 elems/thread), normalize with the head's own
// weight (q_norm for q heads, k_norm for k heads), then each thread rotates exactly one pair (hd/2=64
// pairs) with the branch-free NORM/NEOX arithmetic. Output layout identical to the old chain:
// [0,ad)=roped q, [ad,ad+kv)=roped k. Float-tolerance note: the RMS reduction order differs from the
// standalone RMSNORM kernel → last-ulp differences, same tolerance class as the MoE v2 kernels.
const QKNORM_ROPE: &str = r#"
struct D { nq:u32,nk:u32,hd:u32,base:f32,pos:u32,neox:u32,eps:f32,msc:f32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> qn: array<f32>;
@group(0)@binding(2) var<storage,read> kn: array<f32>;
@group(0)@binding(3) var<storage,read> ff: array<f32>;
@group(0)@binding(4) var<storage,read_write> o: array<f32>;
@group(0)@binding(5) var<uniform> d: D;
var<workgroup> red: array<f32,64>;
var<workgroup> nx: array<f32,128>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let head=wg.x; let tid=lid.x; let hd=d.hd; let half=hd/2u;
  let off=head*hd; // heads are contiguous: q heads [0,nq), then k heads
  // per-head sum of squares (each thread covers hd/64 elems)
  var ss=0.0; var i=tid;
  loop { if(i>=hd){break;} let v=x[off+i]; ss=ss+v*v; i=i+64u; }
  red[tid]=ss; workgroupBarrier();
  for(var s2=32u;s2>0u;s2=s2>>1u){ if(tid<s2){ red[tid]=red[tid]+red[tid+s2]; } workgroupBarrier(); }
  let inv=inverseSqrt(red[0]/f32(hd)+d.eps);
  // normalize with the head-family weight into workgroup memory
  i=tid;
  loop { if(i>=hd){break;}
    let w=select(kn[i], qn[i], head<d.nq);
    nx[i]=x[off+i]*inv*w; i=i+64u; }
  workgroupBarrier();
  // one rotation pair per thread (branch-free NORM/NEOX pairing, same arithmetic as ROPE_MH)
  if(tid<half){
    let freq=f32(d.pos)/(pow(d.base,(2.0*f32(tid))/f32(hd))*ff[tid]);
    let c=cos(freq)*d.msc; let s=sin(freq)*d.msc;
    let j0=tid*(2u-d.neox);
    let j1=j0 + 1u + d.neox*(half-1u);
    let x0=nx[j0]; let x1=nx[j1];
    o[off+j0]=x0*c-x1*s;
    o[off+j1]=x0*s+x1*c;
  }
}"#;

const GATHER_ROWS: &str = r#"
struct D { n:u32,c:u32,_b:u32,_d:u32 };
@group(0)@binding(0) var<storage,read> src: array<f32>;
@group(0)@binding(1) var<storage,read> idx: array<u32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; if(i>=d.n*d.c){return;}
  let r=i/d.c; let col=i%d.c; o[i]=src[idx[r]*d.c+col]; }"#;

const SCATTER_ADD_W: &str = r#"
struct D { n:u32,c:u32,_b:u32,_d:u32 };
@group(0)@binding(0) var<storage,read> src: array<f32>;
@group(0)@binding(1) var<storage,read> idx: array<u32>;
@group(0)@binding(2) var<storage,read> wgt: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; if(i>=d.n*d.c){return;}
  let r=i/d.c; let col=i%d.c; o[idx[r]*d.c+col]=o[idx[r]*d.c+col]+wgt[r]*src[i]; }"#;

const ADD: &str = r#"
struct D { n:u32,_a:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> b: array<f32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){ let i=g.y*nwg.x*64u+g.x; if(i>=d.n){return;} o[i]=a[i]+b[i]; }"#;

const ADD_BIAS: &str = r#"
struct D { n:u32,c:u32,_b:u32,_d:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> bias: array<f32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){ let i=g.y*nwg.x*64u+g.x; if(i>=d.n){return;} o[i]=a[i]+bias[i%d.c]; }"#;

const MUL: &str = r#"
struct D { n:u32,_a:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> b: array<f32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){ let i=g.y*nwg.x*64u+g.x; if(i>=d.n){return;} o[i]=a[i]*b[i]; }"#;

// --- DiT (Z-Image-style diffusion transformer) primitives -------------------------------------------

/// AdaLN-Zero modulation: `o[r,c] = x[r,c] * (1 + scale[c]) + shift[c]`. `scale`/`shift` are a single
/// [1,cols] row derived from the timestep+conditioning embedding and broadcast over every token row —
/// this is what replaces the usual learned-affine LayerNorm/RMSNorm gain in a DiT block.
const DIT_MODULATE: &str = r#"
struct D { rows:u32,cols:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> scale: array<f32>;
@group(0)@binding(2) var<storage,read> shift: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; if(i>=d.rows*d.cols){return;}
  let c=i%d.cols; o[i]=x[i]*(1.0+scale[c])+shift[c];
}"#;

/// AdaLN-Zero gated residual: `o[r,c] = x[r,c] + gate[c] * y[r,c]`. `gate` starts at ~0 at
/// initialization (the "-Zero" in AdaLN-Zero) so a freshly-added DiT block starts as a no-op.
const DIT_GATED_ADD: &str = r#"
struct D { rows:u32,cols:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> y: array<f32>;
@group(0)@binding(2) var<storage,read> gate: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; if(i>=d.rows*d.cols){return;}
  let c=i%d.cols; o[i]=x[i]+gate[c]*y[i];
}"#;

/// Patchify: an [H,W,C] image (row-major, channel-last) → [num_patches, patch*patch*C] tokens, one
/// row per non-overlapping `patch x patch` block, patches scanned left-to-right/top-to-bottom and each
/// patch's contents flattened row-major-within-patch then channel-last (matches the ViT/DiT convention
/// so a plain linear projection turns a patch row directly into a token embedding).
const PATCHIFY: &str = r#"
struct D { h:u32,w:u32,c:u32,psz:u32 };
@group(0)@binding(0) var<storage,read> img: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let pw=d.w/d.psz; let ph=d.h/d.psz; let n=ph*pw; let plen=d.psz*d.psz*d.c;
  let i=g.y*nwg.x*64u+g.x; if(i>=n*plen){return;}
  let p=i/plen; let within=i%plen;
  let py=p/pw; let px=p%pw;
  let pc=within%d.c; let pp=within/d.c; let pyy=pp/d.psz; let pxx=pp%d.psz;
  let sy=py*d.psz+pyy; let sx=px*d.psz+pxx;
  o[i]=img[(sy*d.w+sx)*d.c+pc];
}"#;

/// Unpatchify: the exact inverse of `PATCHIFY` — [num_patches, patch*patch*C] tokens → [H,W,C] image.
const UNPATCHIFY: &str = r#"
struct D { h:u32,w:u32,c:u32,psz:u32 };
@group(0)@binding(0) var<storage,read> tok: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let pw=d.w/d.psz; let ph=d.h/d.psz; let n=ph*pw; let plen=d.psz*d.psz*d.c;
  let i=g.y*nwg.x*64u+g.x; if(i>=n*plen){return;}
  let p=i/plen; let within=i%plen;
  let py=p/pw; let px=p%pw;
  let pc=within%d.c; let pp=within/d.c; let pyy=pp/d.psz; let pxx=pp%d.psz;
  let sy=py*d.psz+pyy; let sx=px*d.psz+pxx;
  o[(sy*d.w+sx)*d.c+pc]=tok[i];
}"#;

const SILU: &str = r#"
struct D { n:u32,_a:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){ let i=g.y*nwg.x*64u+g.x; if(i>=d.n){return;} let v=x[i]; o[i]=v/(1.0+exp(-v)); }"#;

// --- VAE decoder primitives (batched: every tensor carries an explicit N dimension) ------------------

/// 2D convolution, channel-last, batched: `x[N,H,W,Cin] * w[Cout,Cin,K,K] + bias[Cout] -> o[N,Hout,Wout,Cout]`.
/// Direct (untiled) accumulation — correctness first, matching how `block_decode`/`block_decode_at`
/// (the non-fused paths) came before their fused/tiled speed-ups elsewhere in this file. `Hout =
/// (H+2*pad-K)/stride+1` (same for W); the caller computes and passes these so the shader stays a flat
/// index walk. One invocation per output element (n,ho,wo,co).
const CONV2D: &str = r#"
struct D { n_batch:u32,h:u32,w:u32,cin:u32,cout:u32,k:u32,stride:u32,pad:u32,hout:u32,wout:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> wt: array<f32>;
@group(0)@binding(2) var<storage,read> bias: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; let total=d.n_batch*d.hout*d.wout*d.cout; if(i>=total){return;}
  let co=i%d.cout; let rest1=i/d.cout;
  let wo=rest1%d.wout; let rest2=rest1/d.wout;
  let ho=rest2%d.hout; let n=rest2/d.hout;
  var acc=bias[co];
  for(var ci=0u;ci<d.cin;ci=ci+1u){
    for(var kh=0u;kh<d.k;kh=kh+1u){
      let hi_s=i32(ho*d.stride+kh)-i32(d.pad); if(hi_s<0||hi_s>=i32(d.h)){continue;}
      let hi=u32(hi_s);
      for(var kw=0u;kw<d.k;kw=kw+1u){
        let wi_s=i32(wo*d.stride+kw)-i32(d.pad); if(wi_s<0||wi_s>=i32(d.w)){continue;}
        let wi=u32(wi_s);
        let xv=x[((n*d.h+hi)*d.w+wi)*d.cin+ci];
        let wv=wt[((co*d.cin+ci)*d.k+kh)*d.k+kw];
        acc=acc+xv*wv;
      }
    }
  }
  o[i]=acc;
}"#;

/// GroupNorm, channel-last, batched: split `C` channels into `groups` equal chunks, normalize each
/// (n, group) over {its channels} × {every spatial position}, then apply the per-channel affine
/// `gamma`/`beta`. One invocation per (n, group) — each walks the whole group's spatial×channel extent
/// twice (stats, then write), same two-pass shape as `RMSNORM`'s per-row walk.
const GROUP_NORM: &str = r#"
struct D { n_batch:u32,hw:u32,c:u32,groups:u32,eps:f32,_a:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> gamma: array<f32>;
@group(0)@binding(2) var<storage,read> beta: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; if(i>=d.n_batch*d.groups){return;}
  let gi=i%d.groups; let n=i/d.groups;
  let cpg=d.c/d.groups; let c0=gi*cpg;
  var mean=0.0; var cnt=0.0;
  for(var p=0u;p<d.hw;p=p+1u){ for(var cc=0u;cc<cpg;cc=cc+1u){ mean=mean+x[(n*d.hw+p)*d.c+c0+cc]; cnt=cnt+1.0; } }
  mean=mean/cnt;
  var v=0.0;
  for(var p=0u;p<d.hw;p=p+1u){ for(var cc=0u;cc<cpg;cc=cc+1u){ let dd=x[(n*d.hw+p)*d.c+c0+cc]-mean; v=v+dd*dd; } }
  v=v/cnt; let inv=1.0/sqrt(v+d.eps);
  for(var p=0u;p<d.hw;p=p+1u){
    for(var cc=0u;cc<cpg;cc=cc+1u){
      let idx=(n*d.hw+p)*d.c+c0+cc;
      o[idx]=(x[idx]-mean)*inv*gamma[c0+cc]+beta[c0+cc];
    }
  }
}"#;

/// Nearest-neighbor 2x spatial upsample, channel-last, batched: `x[N,H,W,C] -> o[N,2H,2W,C]`. The VAE
/// decoder's upsample stages use this (not a learned transposed-conv) — a plain repeat, followed by an
/// ordinary `conv2d` (stride 1, pad 1) to smooth/mix, matching the SD/SDXL-family VAE decoder design.
const UPSAMPLE2X: &str = r#"
struct D { n_batch:u32,h:u32,w:u32,c:u32,_a:u32,_b:u32,_c:u32,_d:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; let h2=d.h*2u; let w2=d.w*2u; let total=d.n_batch*h2*w2*d.c; if(i>=total){return;}
  let cc=i%d.c; let rest1=i/d.c;
  let wo=rest1%w2; let rest2=rest1/w2;
  let ho=rest2%h2; let n=rest2/h2;
  let hi=ho/2u; let wi=wo/2u;
  o[i]=x[((n*d.h+hi)*d.w+wi)*d.c+cc];
}"#;

// True LayerNorm (mean-subtract + population variance + eps), then per-channel scale `gt` and bias
// `bt`. One row per invocation, mirroring RMSNORM's launch. Vision towers (CLIP/SigLIP/Qwen2-VL).
const LAYERNORM: &str = r#"
struct D { rows:u32,cols:u32,eps:f32,_p:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> gt: array<f32>;
@group(0)@binding(2) var<storage,read> bt: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let r=g.x; if(r>=d.rows){return;}
  let base=r*d.cols; var mean=0.0;
  for(var c=0u;c<d.cols;c=c+1u){mean=mean+x[base+c];}
  mean=mean/f32(d.cols);
  var vv=0.0;
  for(var c=0u;c<d.cols;c=c+1u){let z=x[base+c]-mean; vv=vv+z*z;}
  let inv=1.0/sqrt(vv/f32(d.cols)+d.eps);
  for(var c=0u;c<d.cols;c=c+1u){o[base+c]=(x[base+c]-mean)*inv*gt[c]+bt[c];}
}"#;

// tanh-approx GELU: 0.5x(1+tanh(√(2/π)(x+0.044715x³))).
const GELU_TANH: &str = r#"
struct D { n:u32,_a:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){ let i=g.y*nwg.x*64u+g.x; if(i>=d.n){return;} let v=x[i]; let inner=0.7978845608028654*(v+0.044715*v*v*v); o[i]=0.5*v*(1.0+tanh(inner)); }"#;

// QuickGELU: x·sigmoid(1.702x) (OpenAI CLIP).
const GELU_QUICK: &str = r#"
struct D { n:u32,_a:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){ let i=g.y*nwg.x*64u+g.x; if(i>=d.n){return;} let v=x[i]; o[i]=v/(1.0+exp(-1.702*v)); }"#;

// Bidirectional multi-head attention — the ViT twin of ATTENTION_MH: identical layout & GQA mapping,
// but each query attends over ALL tokens (loop j in 0..t, no causal `j<=i` bound).
const ATTENTION_MH_FULL: &str = r#"
struct D { t:u32,hd:u32,nh:u32,nkv:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> k: array<f32>;
@group(0)@binding(2) var<storage,read> v: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let id=g.x; if(id>=dd.t*dd.nh){return;}
  let i=id/dd.nh; let hh=id%dd.nh;
  let dm=dd.nh*dd.hd; let kvdm=dd.nkv*dd.hd;
  let b=hh*dd.hd; let kb=(hh/(dd.nh/dd.nkv))*dd.hd;  // GQA: query head -> shared KV head
  let scale=1.0/sqrt(f32(dd.hd));
  var mx=-3.4e38;
  for(var j=0u;j<dd.t;j=j+1u){ var dot=0.0; for(var c=0u;c<dd.hd;c=c+1u){dot=dot+q[i*dm+b+c]*k[j*kvdm+kb+c];} mx=max(mx,dot*scale); }
  for(var c=0u;c<dd.hd;c=c+1u){o[i*dm+b+c]=0.0;}
  var sum=0.0;
  for(var j=0u;j<dd.t;j=j+1u){
    var dot=0.0; for(var c=0u;c<dd.hd;c=c+1u){dot=dot+q[i*dm+b+c]*k[j*kvdm+kb+c];}
    let p=exp(dot*scale-mx); sum=sum+p;
    for(var c=0u;c<dd.hd;c=c+1u){o[i*dm+b+c]=o[i*dm+b+c]+p*v[j*kvdm+kb+c];}
  }
  for(var c=0u;c<dd.hd;c=c+1u){o[i*dm+b+c]=o[i*dm+b+c]/sum;}
}"#;

// Fused SwiGLU: silu(gate)*up in one dispatch (replaces a separate SILU + MUL).
const SWIGLU: &str = r#"
struct D { n:u32,_a:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> gate: array<f32>;
@group(0)@binding(1) var<storage,read> up: array<f32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){ let i=g.y*nwg.x*64u+g.x; if(i>=d.n){return;} let v=gate[i]; o[i]=(v/(1.0+exp(-v)))*up[i]; }"#;

// SwiGLU over a packed gate||up buffer: input row = [gate(h) | up(h)] (the fused
// wgate_up matmul output), so this reads both halves of one [t,2h] tensor and writes
// [t,h] = silu(gate)*up — pairing with the fused gate+up projection.
const SWIGLU_PACKED: &str = r#"
struct D { n:u32,h:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> gu: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){ let i=g.y*nwg.x*64u+g.x; if(i>=d.n){return;} let r=i/d.h; let c=i%d.h; let base=r*2u*d.h; let v=gu[base+c]; o[i]=(v/(1.0+exp(-v)))*gu[base+d.h+c]; }"#;

const GATHER: &str = r#"
struct D { n:u32,d:u32,id_off:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> table: array<f32>;
@group(0)@binding(1) var<storage,read> ids: array<u32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let p=g.x; if(p>=dd.n*dd.d){return;}
  let i=p/dd.d; let c=p%dd.d;
  o[p]=table[ids[dd.id_off+i]*dd.d+c];
}"#;
// GATHER from an int8-resident (Q8_0-style) table: dequantize ONE row selected by ids[id_off].
// Dequant expression verbatim from MATVEC_Q8_0 (extractBits + select for AGX-safe signed bytes).
const GATHER_Q8_AT: &str = r#"
struct D { k:u32, id_off:u32, kb:u32, _c:u32 };
@group(0)@binding(0) var<storage,read> qw: array<u32>;
@group(0)@binding(1) var<storage,read> qs: array<f32>;
@group(0)@binding(2) var<storage,read> ids: array<u32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let c=g.x; if(c>=d.k){return;}
  let row=ids[d.id_off];
  let word=qw[row*(d.k/4u)+c/4u];
  let ub=extractBits(word,(c%4u)*8u,8u);
  let q=select(i32(ub), i32(ub)-256, ub>=128u);
  o[c]=qs[row*d.kb+(c/32u)]*f32(q);
}"#;
// GPU argmax over a [1, n] row → index of the max into a 1-element u32 buffer. Single workgroup:
// grid-stride scan + shared-memory reduction. Tie-break MATCHES the CPU `argmax` (lowest index wins):
// strict `>` keeps the earlier index within a thread, and the reduction prefers the smaller index on
// equal values. Lets greedy decode pick the next token on-device — read back 4 bytes, not [vocab].
const ARGMAX: &str = r#"
struct D { n:u32, out_slot:u32, _b:u32, _c:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<u32>;
@group(0)@binding(2) var<uniform> d: D;
var<workgroup> sv: array<f32,256>;
var<workgroup> si: array<u32,256>;
@compute @workgroup_size(256,1,1)
fn main(@builtin(local_invocation_id) lid:vec3<u32>){
  let t = lid.x;
  var bv = -3.4028235e38;
  var bi = 0u;
  var i = t;
  loop {
    if (i >= d.n) { break; }
    let v = x[i];
    if (v > bv) { bv = v; bi = i; }
    i = i + 256u;
  }
  sv[t] = bv; si[t] = bi;
  workgroupBarrier();
  var stride = 128u;
  loop {
    if (stride == 0u) { break; }
    if (t < stride) {
      let ov = sv[t + stride]; let oi = si[t + stride];
      if (ov > sv[t] || (ov == sv[t] && oi < si[t])) { sv[t] = ov; si[t] = oi; }
    }
    workgroupBarrier();
    stride = stride >> 1u;
  }
  if (t == 0u) { o[d.out_slot] = si[0]; }
}"#;

// Full CPU `pick()` on the GPU, depositing like ARGMAX: repeat-penalty → top-k → temperature
// softmax → min-p → top-p → uniform CDF walk, writing the chosen id into o[out_slot] AND into a
// rolling recent-context ring (o[kids..kids+rcap]) that the NEXT step's penalty reads — so a whole
// window of sampled steps runs with no CPU round-trip (greedy already had this via ARGMAX; sampled
// decode — what chat actually runs — did a full-vocab readback + a 151k-element CPU sort per token).
// The uniform draw comes from a CPU-precomputed xorshift01 stream (uni[out_slot]) so behavior is
// deterministic given the seed and directly comparable against the CPU sampler.
//
// Selection strategy (the part that must scale to a 151k vocab in ONE workgroup): penalties only
// LOWER logits (x/p keeps sign and shrinks; negative×p sinks further), so every member of the true
// post-penalty top-k lies within the RAW top-(top_k + n_penalized). We iterate raw maxima in exact
// descending (value, then ascending index) order — which makes exclusion of already-picked elements
// a single compare against the LAST pick (v < last_v || (v == last_v && i > last_i)) instead of any
// membership test — collect top_k+n_over candidates, then thread 0 applies the (≤64-entry) penalty
// override table, re-sorts the candidates by (post value desc, index asc) — exactly the CPU's
// stable descending sort semantics — truncates to top_k, and replays the CPU's serial float-op
// order for softmax/min-p/top-p/CDF. Host guarantees temperature>0 and 2 ≤ top_k ≤ 64.
const SAMPLE_AT: &str = r#"
struct D { n:u32, out_slot:u32, top_k:u32, rlast:u32,
           temp:u32, top_p:u32, min_p:u32, penalty:u32,
           seeded:u32, rcap:u32, kids:u32, abs_step:u32,
           _p0:u32, _p1:u32, _p2:u32, _p3:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> uni: array<f32>;
@group(0)@binding(2) var<storage,read_write> o: array<u32>;
@group(0)@binding(3) var<uniform> d: D;
var<workgroup> sv: array<f32,256>;
var<workgroup> si: array<u32,256>;
var<workgroup> cand_v: array<f32,128>;
var<workgroup> cand_i: array<u32,128>;
var<workgroup> ov_i: array<u32,64>;
var<workgroup> ov_v: array<f32,64>;
var<workgroup> n_over: u32;
var<workgroup> last_v: f32;
var<workgroup> last_i: u32;
@compute @workgroup_size(256,1,1)
fn main(@builtin(local_invocation_id) lid:vec3<u32>){
  let t = lid.x;
  let temp = bitcast<f32>(d.temp);
  let pen  = bitcast<f32>(d.penalty);
  // ── phase 1 (thread 0, serial): replay the CPU penalty over the last rlast ring entries ──
  if (t == 0u) {
    n_over = 0u;
    let valid = min(d.rcap, d.seeded + d.abs_step);
    let neff = min(d.rlast, valid);
    if (pen != 1.0 && neff > 0u) {
      let head = d.seeded + d.abs_step; // next write position (ring index modulo rcap)
      var j: u32 = neff;
      loop { // oldest → newest, exactly the CPU slice order (order is commutative but kept anyway)
        if (j == 0u) { break; }
        let tok = o[d.kids + ((head - j) % d.rcap)];
        if (tok < d.n) {
          // current value: staged override if present, else the raw logit
          var cur = x[tok]; var slot = n_over;
          var q = 0u;
          loop { if (q >= n_over) { break; }
                 if (ov_i[q] == tok) { cur = ov_v[q]; slot = q; break; } q = q + 1u; }
          if (cur > 0.0) { cur = cur / pen; } else { cur = cur * pen; }
          ov_i[slot] = tok; ov_v[slot] = cur;
          if (slot == n_over) { n_over = n_over + 1u; }
        }
        j = j - 1u;
      }
    }
    last_v = 3.4028235e38; last_i = 0xffffffffu; // sentinel: nothing picked yet
  }
  workgroupBarrier();
  // ── phase 2: iterate raw maxima in exact (value desc, index asc) order ──
  let want = min(d.top_k + n_over, min(128u, d.n));
  var c: u32 = 0u;
  loop {
    if (c >= want) { break; }
    var bv = -3.4028235e38;
    var bi = 0xffffffffu;
    var i = t;
    loop {
      if (i >= d.n) { break; }
      let v = x[i];
      // exclusion by order: already-picked ⇔ NOT (v < last_v || (v == last_v && i > last_i))
      if (v < last_v || (v == last_v && i > last_i)) {
        if (v > bv || (v == bv && i < bi)) { bv = v; bi = i; }
      }
      i = i + 256u;
    }
    sv[t] = bv; si[t] = bi;
    workgroupBarrier();
    var stride = 128u;
    loop {
      if (stride == 0u) { break; }
      if (t < stride) {
        let ovv = sv[t + stride]; let oii = si[t + stride];
        if (ovv > sv[t] || (ovv == sv[t] && oii < si[t])) { sv[t] = ovv; si[t] = oii; }
      }
      workgroupBarrier();
      stride = stride >> 1u;
    }
    if (t == 0u) {
      cand_v[c] = sv[0]; cand_i[c] = si[0];
      last_v = sv[0]; last_i = si[0];
    }
    workgroupBarrier();
    c = c + 1u;
  }
  // ── phase 3 (thread 0, serial): overrides → stable re-sort → the CPU sampler tail verbatim ──
  if (t == 0u) {
    // apply penalty overrides to the candidate set
    var a: u32 = 0u;
    loop { if (a >= want) { break; }
      var q = 0u;
      loop { if (q >= n_over) { break; }
        if (ov_i[q] == cand_i[a]) { cand_v[a] = ov_v[q]; break; } q = q + 1u; }
      a = a + 1u; }
    // insertion sort by (value desc, index asc) — the CPU's stable descending order
    a = 1u;
    loop { if (a >= want) { break; }
      let vv = cand_v[a]; let ii = cand_i[a];
      var b = a;
      loop { if (b == 0u) { break; }
        let pv = cand_v[b - 1u]; let pi = cand_i[b - 1u];
        if (pv > vv || (pv == vv && pi < ii)) { break; }
        cand_v[b] = pv; cand_i[b] = pi; b = b - 1u; }
      cand_v[b] = vv; cand_i[b] = ii;
      a = a + 1u; }
    let k = min(d.top_k, want);
    // temperature softmax over the k picks (same op order as CPU: exp((v-mx)/T), sum, divide)
    let mx = cand_v[0];
    var sum = 0.0;
    var pi2: u32 = 0u;
    loop { if (pi2 >= k) { break; }
      let e = exp((cand_v[pi2] - mx) / temp);
      cand_v[pi2] = e; sum = sum + e; pi2 = pi2 + 1u; }
    pi2 = 0u;
    loop { if (pi2 >= k) { break; } cand_v[pi2] = cand_v[pi2] / sum; pi2 = pi2 + 1u; }
    var cut = k;
    let minp = bitcast<f32>(d.min_p);
    if (minp > 0.0) {
      let thresh = minp * cand_v[0];
      var f = cut; pi2 = 0u;
      loop { if (pi2 >= cut) { break; }
        if (cand_v[pi2] < thresh) { f = pi2; break; } pi2 = pi2 + 1u; }
      cut = max(f, 1u);
      var s2 = 0.0; pi2 = 0u;
      loop { if (pi2 >= cut) { break; } s2 = s2 + cand_v[pi2]; pi2 = pi2 + 1u; }
      pi2 = 0u;
      loop { if (pi2 >= cut) { break; } cand_v[pi2] = cand_v[pi2] / s2; pi2 = pi2 + 1u; }
    }
    let topp = bitcast<f32>(d.top_p);
    if (topp < 1.0) {
      var cum = 0.0; var f = cut; pi2 = 0u;
      loop { if (pi2 >= cut) { break; }
        cum = cum + cand_v[pi2];
        if (cum >= topp) { f = pi2 + 1u; break; } pi2 = pi2 + 1u; }
      cut = f;
      var s3 = 0.0; pi2 = 0u;
      loop { if (pi2 >= cut) { break; } s3 = s3 + cand_v[pi2]; pi2 = pi2 + 1u; }
      pi2 = 0u;
      loop { if (pi2 >= cut) { break; } cand_v[pi2] = cand_v[pi2] / s3; pi2 = pi2 + 1u; }
    }
    let r = uni[d.out_slot];
    var picked = cand_i[cut - 1u];
    var cum2 = 0.0; pi2 = 0u;
    loop { if (pi2 >= cut) { break; }
      cum2 = cum2 + cand_v[pi2];
      if (r < cum2) { picked = cand_i[pi2]; break; } pi2 = pi2 + 1u; }
    o[d.out_slot] = picked;
    o[d.kids + ((d.seeded + d.abs_step) % d.rcap)] = picked; // extend the ring for the NEXT step
  }
}"#;

// ── model: load a GGUF into resident weights and run a stacked forward pass ──
/// A loaded model: embedding (host, for lookup), N resident transformer blocks,
/// final norm, and output projection. Single attention head per block for now.
pub struct Model {
    embed: Vec<f32>,
    layers: Vec<BlockWeights>,
    out_norm: Tensor,
    output: Lin,
    /// Per-rotary-dimension RoPE frequency factors (length head_dim/2). Encodes the
    /// llama3 long-context scaling from `rope_freqs.weight`; all 1.0 when absent.
    rope_factors: Tensor,
    pub d: usize, pub h: usize, pub vocab: usize, pub n_layers: usize, pub n_heads: usize,
    pub n_kv_heads: usize, pub head_dim: usize,
    pub base: f32, pub eps: f32,
}
impl Model {
    /// Fingerprint a few GPU-resident weights (LOGSCOPE_WEIGHT_CHECK=1): downloads small tensors and
    /// prints first values + abs-sums. The same GGUF must produce IDENTICAL lines on every machine —
    /// a mismatch between boxes means the UPLOAD path corrupted weights on one of them (e.g. the UMA
    /// zero-copy mapped write on one backend), independent of any kernel. Cheap: a few KB downloaded.
    pub fn weight_probe(&self, gpu: &Gpu) -> String {
        let f = |t: &Tensor, n: usize| -> (Vec<f32>, f64) {
            let v = gpu.download(t);
            let head: Vec<f32> = v.iter().take(n).cloned().collect();
            let sum: f64 = v.iter().map(|x| x.abs() as f64).sum();
            (head, sum)
        };
        let (g1h, g1s) = f(&self.layers[0].g1, 4);
        let (onh, ons) = f(&self.out_norm, 4);
        let (rfh, rfs) = f(&self.rope_factors, 4);
        let qn = self.layers[0].q_norm.as_ref().map(|t| f(t, 4));
        format!("weight-probe: blk0.g1 {:?} Σ|{:.4e}| | out_norm {:?} Σ|{:.4e}| | rope_ff {:?} Σ|{:.4e}|{}",
            g1h, g1s, onh, ons, rfh, rfs,
            qn.map(|(h, s)| format!(" | q_norm {h:?} Σ|{s:.4e}|")).unwrap_or_default())
    }

    /// Width of one K/V cache row = n_kv_heads × head_dim (== d for plain MHA, but NOT for Qwen3 where
    /// head_dim is decoupled from d/n_heads).
    pub fn kv_dim(&self) -> usize { self.n_kv_heads * self.head_dim }
    /// The width Q and attention operate on = n_heads × head_dim (== d for plain MHA).
    pub fn attn_dim(&self) -> usize { self.n_heads * self.head_dim }
    /// The per-rotary-pair RoPE frequency factors (llama3 scaling); all-ones if none loaded.
    pub fn rope_factors(&self) -> &Tensor { &self.rope_factors }
    /// Whether the output projection is kept resident as int8.
    pub fn output_is_quantized(&self) -> bool { self.output.is_quantized() }
}

impl Gpu {
    /// Parse a GGUF image and upload every weight into resident GPU tensors (f32).
    pub fn load_gguf(&self, bytes: &[u8]) -> Result<Model, String> { self.load_gguf_opt(bytes, false) }
    /// Same, but keep the large projection weights resident as int8 (4× less weight
    /// bandwidth). Norm gains stay f32; any matrix whose inner dim isn't a multiple
    /// of 32 falls back to f32 automatically.
    /// Load quantized weights, preferring int8-resident; if the GPU can't allocate it (Asahi reports
    /// ample system RAM but can't map it all — the "BO VMA" failure), auto-retry q4-resident (≈half).
    /// If even q4 won't fit, return a clean error (NOT a panic) so the worker stays alive and the app can
    /// fall back to the llama.cpp backend. Both attempts run inside an OOM error scope so the wgpu
    /// allocation failure is captured instead of aborting the process.
    pub fn load_gguf_quantized(&self, bytes: &[u8]) -> Result<Model, String> {
        let forced_q4 = std::env::var("LOGSCOPE_Q4_RESIDENT").is_ok();
        match self.try_load_resident(bytes)? {
            Some(m) => return Ok(m),
            None if forced_q4 => return Err(Self::too_big_msg(bytes.len())), // already q4 — can't shrink further
            None => {}
        }
        // int8 attempt hit GPU OOM → free partials and retry at q4-resident.
        self.pool.free.lock().unwrap().clear();
        eprintln!("wgpu-engine: int8-resident exceeded the GPU's allocatable memory — retrying as q4-resident (≈half the footprint)…");
        std::env::set_var("LOGSCOPE_Q4_RESIDENT", "1");
        let r = self.try_load_resident(bytes);
        std::env::remove_var("LOGSCOPE_Q4_RESIDENT");
        match r? {
            Some(m) => Ok(m),
            None => Err(Self::too_big_msg(bytes.len())),
        }
    }
    fn too_big_msg(file_bytes: usize) -> String {
        let gib = file_bytes as f64 / (1u64 << 30) as f64;
        format!(
            "this ~{gib:.0} GiB model exceeds the GPU's allocatable memory even at q4-resident — too big for \
             the wgpu engine on this GPU (its resident copy of the weights won't fit). Use the llama.cpp / \
             ollama backend for this model (it mmaps weights into unified memory and fits where the wgpu \
             engine can't), or run a smaller model."
        )
    }
    /// Load inside an OutOfMemory error scope. Ok(Some) = loaded; Ok(None) = GPU OOM (captured, the
    /// process is NOT aborted); Err = a genuine (non-OOM) load error.
    fn try_load_resident(&self, bytes: &[u8]) -> Result<Option<Model>, String> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let res = self.load_gguf_opt(bytes, true);
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let oom = pollster::block_on(scope.pop());
        if oom.is_some() {
            drop(res.ok()); // discard any partial/poisoned buffers from the failed attempt
            return Ok(None);
        }
        res.map(Some)
    }
    pub fn load_gguf_opt(&self, bytes: &[u8], q: bool) -> Result<Model, String> {
        let g = gguf::parse(bytes)?;
        // Data-integrity gate (unconditional, ~8 KB): read a small F32 tensor's RAW bytes straight from the
        // mapped file — before any dequant, upload, or GPU involvement. An all-zeros norm weight is
        // impossible in a real checkpoint; it means the file's DATA section is corrupt (partial download /
        // bad copy — metadata parses fine, so size checks and dumps look healthy). This exact failure
        // burned a full debugging cycle on the M4 (weights probed zero on GPU; every engine path was
        // exonerated one by one). Refuse loudly with the fix instead of generating symbol soup.
        if let Some(t) = g.tensor("blk.0.attn_norm.weight").filter(|t| t.ggml_type == 0) {
            let off = (g.data_offset + t.offset) as usize;
            let n = (t.dims.iter().product::<u64>() as usize * 4).min(8192);
            if let Some(raw) = bytes.get(off..off + n) {
                if raw.iter().all(|&b| b == 0) {
                    return Err(format!(
                        "GGUF data section is corrupt: blk.0.attn_norm.weight is ALL ZEROS in the file                          (raw bytes at offset {off}). The header parses fine, so the file LOOKS healthy —                          this is a partial download or bad copy. Re-download or re-copy the file                          (verify with a sha256 checksum against a known-good copy)."));
                }
                if std::env::var("LOGSCOPE_WEIGHT_CHECK").is_ok() {
                    let head: Vec<f32> = raw.chunks_exact(4).take(4)
                        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
                    let sum: f64 = raw.chunks_exact(4)
                        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]).abs() as f64).sum();
                    eprintln!("wgpu-engine: raw-file-probe blk0.attn_norm {head:?} Σ|{sum:.4e}| (read from disk, pre-GPU)");
                }
            }
        }
        // Guard the LLM path: a diffusion checkpoint (z-image, flux, SD) or an mmproj has no tokenizer and
        // would die deep in tokenizer loading with a cryptic error. Classify up front and refuse with a
        // message that points at the right subsystem (the image pipeline) instead.
        let kind = crate::model_kind::classify(&g);
        // Self-service GGUF introspection (llama.cpp's gguf-dump may not build on the box): dump the tensor
        // table on request. LOGSCOPE_GGUF_DUMP=1 dumps all to stderr; =blk.0. filters to one layer; a value
        // containing '/' is treated as a FILE PATH and the full dump is written there (easiest to share).
        if let Ok(v) = std::env::var("LOGSCOPE_GGUF_DUMP") {
            if v.contains('/') {
                match std::fs::write(&v, g.dump_tensors("")) {
                    Ok(()) => eprintln!("wgpu-engine: wrote GGUF tensor dump → {v} ({} tensors)", g.tensors.len()),
                    Err(e) => eprintln!("wgpu-engine: GGUF dump write to {v} failed: {e}"),
                }
            } else {
                let prefix = if v == "1" || v.is_empty() { "" } else { v.as_str() };
                eprintln!("{}", g.dump_tensors(prefix));
            }
        }
        if !kind.is_llm() {
            return Err(format!("not a language model: {}", kind.guidance()));
        }
        // Architecture support is decided by the arch registry (archcfg): llama, Qwen2 (QKV bias), and
        // Qwen3/Qwen3.5 (QK-Norm before RoPE) are wired; gemma2/phi3/unknown are refused with guidance.
        // arch() resolves the metadata-key prefix robustly: general.architecture when its own keys back
        // it up, else the prefix of whatever *.embedding_length key the file DOES carry (recovering a
        // converter's mislabel, e.g. an Ornith GGUF missing general.architecture), else "llama". So a
        // mislabeled-but-supported model loads, and a genuinely unsupported one is named correctly below.
        let arch = g.arch();
        let acfg = crate::archcfg::for_arch(&arch);
        if !acfg.supported {
            // MoE-aware escape: some architectures (e.g. Qwen4) are only PARTIALLY unsupported — their MoE
            // routing kernel isn't wired, but a DENSE checkpoint of the same arch runs on the existing
            // forward path. Read the expert count from metadata: 0/absent ⇒ dense ⇒ allow; >0 ⇒ real MoE ⇒
            // refuse with the arch's precise note. Architectures with no dense form (Llama4) never escape.
            let expert_count = g.get_u32(&format!("{}.expert_count", acfg.prefix)).unwrap_or(0);
            let dense_ok = acfg.dense_supported && expert_count == 0;
            if !dense_ok {
                let moe_hint = if expert_count > 0 { format!(" (this is a {expert_count}-expert MoE checkpoint)") } else { String::new() };
                // Append layer-0's tensors so the exact expert stacking layout is visible right here — the
                // one thing needed to wire the MoE forward, without a separate dump run.
                let layout = g.dump_tensors("blk.0.");
                return Err(format!("architecture '{arch}' isn't supported by the wgpu engine yet{moe_hint} — {}. \
                    Use the llama.cpp backend for this model.\n{layout}", acfg.note));
            }
            eprintln!("loader: '{arch}' is a DENSE checkpoint (no experts) — loading via the {} forward path", acfg.prefix);
        }
        // Metadata key prefix. Normally the registry's ("llama"/"qwen3"/...), but some conversions write
        // general.architecture with one spelling and the hparam keys under ANOTHER (seen live: a Qwen3.5
        // GGUF resolving to 'qwen3_5' with ZERO keys under that prefix). Rather than guess spellings,
        // SNIFF the real prefix: any '<pfx>.embedding_length' key in the metadata names it authoritatively.
        let pfx: &str = {
            let reg = acfg.prefix;
            if g.metadata.contains_key(&format!("{reg}.embedding_length")) { reg } else {
                match g.metadata.keys().find_map(|k| k.strip_suffix(".embedding_length")) {
                    Some(real) => {
                        eprintln!("loader: hparam prefix is '{real}.' (registry expected '{reg}.') — using the file's prefix");
                        // leak: one tiny string per load, avoids threading a String through 30 format! sites
                        Box::leak(real.to_string().into_boxed_str())
                    }
                    None => reg,
                }
            }
        };
        let has_qk_norm = acfg.attn_qk_norm; // Qwen3/3.5: load blk.N.attn_{q,k}_norm.weight below
        // RoPE pairing style is per-FAMILY: llama rotates adjacent pairs (NORM); qwen/gemma/phi rotate
        // split halves (NEOX). Angles are identical — only the pairing differs — so the wrong style yields
        // real-word, positionally-scrambled output (the dense-Qwen salad). Set from the arch registry.
        self.rope_neox.store(!acfg.rope_interleaved, std::sync::atomic::Ordering::Relaxed);
        if !acfg.rope_interleaved { eprintln!("loader: NEOX (split-half) RoPE pairing for '{arch}'"); }
        let d = g.get_u32(&format!("{pfx}.embedding_length"))
            .ok_or_else(|| {
                let key = format!("{pfx}.embedding_length");
                if g.metadata.contains_key(&key) {
                    // key IS present but get_u32 couldn't read it — a non-integer or out-of-range type
                    format!("'{key}' is present but not a readable u32 dimension (unexpected metadata value \
                        type or out-of-range) — the GGUF's hyperparameter encoding is non-standard")
                } else {
                    let mut present: Vec<&str> = g.metadata.keys()
                        .filter(|k| k.starts_with(&format!("{pfx}.")))
                        .map(|s| s.as_str()).collect();
                    present.sort_unstable();
                    let listed = if present.is_empty() { "(none)".to_string() } else { present.join(", ") };
                    // also name the prefixes that ARE in the file, so a spelling mismatch is visible at a glance
                    let mut pfxs: Vec<&str> = g.metadata.keys().filter_map(|k| k.split('.').next()).collect();
                    pfxs.sort_unstable(); pfxs.dedup();
                    format!("missing embedding_length: the GGUF has no '{key}' key (resolved arch '{pfx}'); \
                        keys under '{pfx}.' present: {listed}; metadata prefixes in the file: {} — the file may be truncated, non-standard, or not a supported LLM", pfxs.join(", "))
                }
            })? as usize;
        // MoE (qwen3moe): expert config from metadata. is_moe additionally requires the stacked expert
        // tensors to actually be present so a mislabeled dense file falls through to the dense path.
        let n_expert = g.get_u32(&format!("{pfx}.expert_count")).unwrap_or(0) as usize;
        let n_expert_used = g.get_u32(&format!("{pfx}.expert_used_count")).unwrap_or(8) as usize;
        let expert_ff = g.get_u32(&format!("{pfx}.expert_feed_forward_length")).unwrap_or(0) as usize;
        let is_moe = acfg.moe && n_expert > 0 && expert_ff > 0 && g.tensor("blk.0.ffn_gate_exps.weight").is_some();
        let h = match g.get_u32(&format!("{pfx}.feed_forward_length")) {
            Some(v) => v as usize,
            None if is_moe => expert_ff, // MoE GGUFs may omit the dense FFN width entirely
            None => return Err("missing feed_forward_length".into()),
        };
        let n = g.get_u32(&format!("{pfx}.block_count")).ok_or("missing block_count")? as usize;
        // vocab_size is often absent in real GGUFs; derive it from the embedding below.
        let vocab_meta = g.get_u32(&format!("{pfx}.vocab_size")).map(|v| v as usize);
        let base = g.get_f32(&format!("{pfx}.rope.freq_base")).unwrap_or(10000.0);
        let eps = g.get_f32(&format!("{pfx}.attention.layer_norm_rms_epsilon")).unwrap_or(1e-5);
        let n_heads = g.get_u32(&format!("{pfx}.attention.head_count")).unwrap_or(1) as usize;
        let n_kv_heads = g.get_u32(&format!("{pfx}.attention.head_count_kv")).unwrap_or(n_heads as u32) as usize;
        // head_dim is NOT always d/n_heads (Qwen3: head_dim=128 while d/n_heads can be 64/80). Prefer the
        // explicit key_length metadata; else derive from the real attn_q tensor width (output/n_heads); else
        // fall back to d/n_heads. attn_dim = n_heads*head_dim is the width Q and attention operate on (== d
        // for plain MHA, so dense/llama models are unchanged); attn_output projects it back down to d.
        let head_dim = g.get_u32(&format!("{pfx}.attention.key_length")).map(|v| v as usize)
            .or_else(|| g.tensor("blk.0.attn_q.weight").map(|t| (t.dims.iter().product::<u64>() as usize / d) / n_heads))
            .filter(|&hd| hd > 0)
            .unwrap_or_else(|| d / n_heads.max(1));
        let attn_dim = n_heads * head_dim;      // Q / attention width (e.g. 4096 for Qwen3-4B, while d=2560)
        let kv = n_kv_heads * head_dim;         // K/V projection + cache row width (GQA when < attn_dim)
        // Bigger-model memory pre-flight: estimate the resident footprint from metadata (cheap — tensor
        // dims only, no dequant) and refuse gracefully WITH the number instead of OOMing mid-load.
        let vocab_est = vocab_meta.unwrap_or_else(|| g.tensor("token_embd.weight")
            .map(|t| (t.dims.iter().product::<u64>() as usize) / d).unwrap_or(0));
        // Residency planning (memplan): prefer int8; auto-fall-back to q4 when int8 won't fit the budget;
        // refuse (with a number) only when even q4 won't fit. LOGSCOPE_Q4_RESIDENT forces q4 regardless.
        let mut auto_q4 = false;
        if vocab_est > 0 {
            if let Some(budget) = store::resident_budget_bytes() {
                // MoE: the FFN weight mass is the EXPERTS — n_expert stacked [expert_ff, d] matrices per
                // kind — not the dense feed_forward_length. Sizing with `h` alone made plan() see the 30B
                // MoE as a ~5 GiB model → int8 → ~31 GiB real → BO VMA death. Effective width fixes the est.
                let ffn_eff = if is_moe { n_expert * expert_ff } else { h };
                let shape = crate::memplan::ModelShape { d, ffn: ffn_eff, layers: n, vocab: vocab_est, n_heads, n_kv_heads, tied_embed: g.tensor("output.weight").is_none() };
                if q {
                    use crate::memplan::{Fit, Residency, gib, plan, gpu_buffer_budget};
                    // Weights live in GPU BUFFERS, whose total is capped far below RAM on unified-memory
                    // drivers (Asahi/Honeykrisp). Plan against that cap, not RAM — otherwise int8 for a 70B
                    // (~70 GiB) "fits" the ~170 GiB RAM budget and then the driver refuses the allocation
                    // mid-upload ("Failed to allocate BO VMA"). The cap makes it pick q4 (or refuse) up front.
                    let gpu_budget = gpu_buffer_budget(budget, self.conservative_vma);
                    match plan(&shape, gpu_budget, 4096, 2, 256 * (1 << 20)) {
                        Fit::Resident { fmt: Residency::Int8, .. } => {} // int8 fits — the default
                        Fit::Resident { fmt: Residency::Q4, weight_bytes, .. } => {
                            auto_q4 = true;
                            eprintln!("loader: int8 won't fit the ~{:.1} GiB GPU-buffer budget → auto-selecting q4-resident (~{:.1} GiB)", gib(gpu_budget), gib(weight_bytes));
                        }
                        Fit::Resident { fmt: Residency::F32, .. } => {}
                        Fit::Streaming { .. } => {
                            // report what streaming WOULD do so it's actionable, even though execution isn't wired
                            let msg = match crate::memplan::plan_streaming(&shape, gpu_budget, Residency::Q4, 4096, 2, 256 * (1 << 20)) {
                                Some(p) => format!("won't fit resident in ~{:.1} GiB — layer streaming would pin {}/{} blocks and stream {} per token (~{:.0} ms/tok at 100 GiB/s); streaming isn't implemented yet, use the llama.cpp backend or raise LOGSCOPE_MAX_GPU_GB.",
                                    gib(gpu_budget), p.pinned_layers, p.total_layers, p.streamed_layers(), p.decode_ms_per_token(100.0)),
                                None => format!("won't fit even a streaming ring in ~{:.1} GiB; use the llama.cpp backend.", gib(gpu_budget)),
                            };
                            return Err(msg);
                        }
                        Fit::TooBig { min_needed } => return Err(format!("model won't fit at any quant — needs ~{:.1} GiB, GPU-buffer budget is ~{:.1} GiB (raise LOGSCOPE_MAX_GPU_GB to override).", gib(min_needed), gib(gpu_budget))),
                    }
                } else {
                    use crate::memplan::{gib, kv_cache_bytes, resident_weight_bytes, Residency};
                    let need = resident_weight_bytes(&shape, Residency::F32) + kv_cache_bytes(&shape, 4096, 2);
                    if need > budget { return Err(format!("model needs ~{:.1} GiB f32 resident + KV, but only ~{:.1} GiB is available (load quantized, or set LOGSCOPE_MAX_RESIDENT_GB to override).", gib(need), gib(budget))); }
                }
            }
        }
        let fetch = |name: &str| -> Result<Vec<f32>, String> {
            let t = g.tensor(name).ok_or(format!("missing tensor {name}"))?;
            let count = t.dims.iter().product::<u64>() as usize;
            let nbytes = gguf::type_nbytes(t.ggml_type, count).ok_or(format!("type {} unsupported for {name}", t.ggml_type))?;
            let start = (g.data_offset + t.offset) as usize;
            if start + nbytes > bytes.len() { return Err(format!("tensor {name} data out of range")); }
            gguf::dequantize(t.ggml_type, &bytes[start..start + nbytes], count)
        };
        let embed = fetch("token_embd.weight")?;
        // Derive vocab from the embedding matrix (rows = vocab) when metadata omits it.
        let vocab = vocab_meta.unwrap_or(embed.len() / d);
        // Build a resident linear, preferring the DIRECT q8_0 → int8 path (no f32 round-trip) when the
        // source tensor is q8_0 and we want int8 residency; otherwise dequant-then-quantize as before.
        let mut q4_mode = std::env::var("LOGSCOPE_Q4_RESIDENT").is_ok() || auto_q4;
        // MoE ⇒ q4 residency, always. int8-resident MoE is the ONE configuration never validated anywhere
        // (the M2's 24 GiB budget always auto-selects q4 for the 30B) — and when the M4's raised budget
        // finally ran it, GPU-resident weights probed ZERO while the raw file bytes were real, producing
        // symbol soup; the SAME machine + file on q4 generates correctly (4.81 tok/s). q4 experts are also
        // simply the right call (half the residency of int8). The broken int8 path stays reachable via
        // LOGSCOPE_MOE_INT8=1 for a dedicated hunt when it ever matters.
        if is_moe && !q4_mode && std::env::var("LOGSCOPE_MOE_INT8").is_err() {
            eprintln!("loader: MoE ⇒ q4-resident (int8-MoE residency is unvalidated/broken — override with LOGSCOPE_MOE_INT8=1)");
            q4_mode = true;
        }
        let q4_mode = q4_mode;
        // Optional arena packing: route resident weights into a few large slabs instead of one buffer
        // per tensor. Guard clears the bump state when this load returns/unwinds (slabs persist via the
        // weight tensors). Off by default — opt in with LOGSCOPE_ARENA (slab MiB via LOGSCOPE_ARENA_SLAB_MB).
        let _arena_guard = if std::env::var("LOGSCOPE_ARENA").is_ok() {
            let max_buf = self.device.limits().max_buffer_size;
            let slab = std::env::var("LOGSCOPE_ARENA_SLAB_MB").ok().and_then(|m| m.trim().parse::<u64>().ok())
                .map(|mb| mb << 20).unwrap_or(2 << 30).min(max_buf).max(256 << 20);
            // Align the slab size DOWN to 256 (≥ COPY_BUFFER_ALIGNMENT=4). A UMA zero-copy slab is created
            // mapped_at_creation, which requires size % 4 == 0 — and some drivers report max_buffer_size as
            // an odd value (Honeykrisp/Asahi: ~2047 MiB = 2^31-1), so `.min(max_buf)` can land on an
            // unaligned size. Rounding DOWN (not up) keeps it within the device limit.
            let slab = slab & !255;
            eprintln!("wgpu-engine: arena ON — packing resident weights into ≤{} MiB slabs (fewer buffer objects)", slab >> 20);
            *self.arena.lock().unwrap() = Some(ResidentArena { slabs: Vec::new(), slab_size: slab, used: 0, mapped: self.unified });
            Some(ArenaGuard(&self.arena))
        } else { None };
        let lin = |name: &str, n: usize, k: usize, quantize: bool| -> Result<Lin, String> {
            let t = g.tensor(name).ok_or(format!("missing tensor {name}"))?;
            if quantize && !q4_mode && t.ggml_type == 8 && k % 32 == 0 {
                let count = t.dims.iter().product::<u64>() as usize;
                let nbytes = gguf::type_nbytes(8, count).ok_or("q8_0 size")?;
                let start = (g.data_offset + t.offset) as usize;
                if start + nbytes > bytes.len() { return Err(format!("tensor {name} out of range")); }
                return Ok(Lin::q8(self.q8_0_to_resident(&bytes[start..start + nbytes], n, k)));
            }
            Ok(self.make_lin(&fetch(name)?, n, k, quantize, q4_mode))
        };
        let mut layers = Vec::with_capacity(n);
        for i in 0..n {
            // Qwen2 carries a bias on Q/K/V (llama doesn't). Fetch the f32 bias once so it can both
            // attach to the separate projections AND concatenate for the fused wqkv bias.
            let has_qkv_bias = g.tensor(&format!("blk.{i}.attn_q.bias")).is_some();
            let (fbq, fbk, fbv) = if has_qkv_bias {
                (fetch(&format!("blk.{i}.attn_q.bias"))?, fetch(&format!("blk.{i}.attn_k.bias"))?, fetch(&format!("blk.{i}.attn_v.bias"))?)
            } else { (Vec::new(), Vec::new(), Vec::new()) };
            let bq = if has_qkv_bias { Some(self.upload(&fbq, 1, d)) } else { None };
            let bk = if has_qkv_bias { Some(self.upload(&fbk, 1, kv)) } else { None };
            let bv = if has_qkv_bias { Some(self.upload(&fbv, 1, kv)) } else { None };
            // Fused Q||K||V projection [d+2kv, d] (mirrors the wgate_up fusion): concatenate the q/k/v
            // weight ROWS then quantize — per-row quant of the concatenation is identical to quantizing
            // each separately (proven bit-exact by --gate-qkv). The bias concatenates the same way.
            let wqkv = {
                let mut qkv = fetch(&format!("blk.{i}.attn_q.weight"))?;
                qkv.extend_from_slice(&fetch(&format!("blk.{i}.attn_k.weight"))?);
                qkv.extend_from_slice(&fetch(&format!("blk.{i}.attn_v.weight"))?);
                let fused = self.make_lin(&qkv, attn_dim + 2 * kv, d, q, q4_mode);
                if has_qkv_bias {
                    let mut b = Vec::with_capacity(attn_dim + 2 * kv);
                    b.extend_from_slice(&fbq); b.extend_from_slice(&fbk); b.extend_from_slice(&fbv);
                    fused.with_bias(Some(self.upload(&b, 1, attn_dim + 2 * kv)))
                } else { fused }
            };
            layers.push(BlockWeights {
                g1: self.upload(&fetch(&format!("blk.{i}.attn_norm.weight"))?, 1, d),
                wqkv,
                wq: lin(&format!("blk.{i}.attn_q.weight"), attn_dim, d, q)?.with_bias(bq),
                wk: lin(&format!("blk.{i}.attn_k.weight"), kv, d, q)?.with_bias(bk),
                wv: lin(&format!("blk.{i}.attn_v.weight"), kv, d, q)?.with_bias(bv),
                wo: lin(&format!("blk.{i}.attn_output.weight"), d, attn_dim, q)?,
                g2: self.upload(&fetch(&format!("blk.{i}.ffn_norm.weight"))?, 1, d),
                // gate and up fused into one [2h, d] projection: a single matmul emits
                // [t, 2h] = [gate(h) | up(h)] per row, which SWIGLU_PACKED then consumes.
                // Halves the FFN projection dispatches/weight-binds; per-row int8 quant of
                // the concatenation is identical to quantizing the two weights separately.
                // MoE layers have no dense FFN tensors — tiny F32 placeholders, never applied
                // (ffn_apply branches to the moe field first).
                wgate_up: if is_moe { self.make_lin(&vec![0.0f32; 32], 1, 32, false, false) } else {
                    let gt = fetch(&format!("blk.{i}.ffn_gate.weight"))?;
                    let u = fetch(&format!("blk.{i}.ffn_up.weight"))?;
                    let mut gu = Vec::with_capacity(gt.len() + u.len());
                    gu.extend_from_slice(&gt); gu.extend_from_slice(&u);
                    self.make_lin(&gu, 2 * h, d, q, q4_mode)
                },
                wdown: if is_moe { self.make_lin(&vec![0.0f32; 32], 1, 32, false, false) } else { lin(&format!("blk.{i}.ffn_down.weight"), d, h, q)? },
                moe: if is_moe {
                    // Router stays F32 (small: [n_expert, d]) — routing precision decides WHICH weights run.
                    let router = self.make_lin(&fetch(&format!("blk.{i}.ffn_gate_inp.weight"))?, n_expert, d, false, false);
                    // Stacked expert tensors: expert dim OUTERMOST, so expert e's matrix is the contiguous
                    // f32 slice [e·rows·cols, (e+1)·rows·cols) after fetch's dequant (which reads each
                    // tensor's ACTUAL ggml type — Q4_K vs Q6_K varies per layer). gate/up: [ff, d]; down: [d, ff].
                    // v2 layout: ONE quantized Lin per kind for the whole expert stack (interleaved
                    // [e: gate||up] for the fused projection), then per-expert Lins are zero-copy row
                    // VIEWS into it. One quant pass instead of n_expert (faster load), and the stacked
                    // buffer is what the GPU-routed decode kernels index (row = sel·rstride + col).
                    let gsrc = fetch(&format!("blk.{i}.ffn_gate_exps.weight"))?;
                    let usrc = fetch(&format!("blk.{i}.ffn_up_exps.weight"))?;
                    let ec = expert_ff * d;
                    let mut inter = Vec::with_capacity(2 * ec * n_expert);
                    for e in 0..n_expert {
                        inter.extend_from_slice(&gsrc[e * ec..(e + 1) * ec]);
                        inter.extend_from_slice(&usrc[e * ec..(e + 1) * ec]);
                    }
                    drop(gsrc); drop(usrc);
                    let stacked_gu = self.make_lin(&inter, n_expert * 2 * expert_ff, d, q, q4_mode);
                    drop(inter);
                    let dsrc = fetch(&format!("blk.{i}.ffn_down_exps.weight"))?;
                    let stacked_down = self.make_lin(&dsrc, n_expert * d, expert_ff, q, q4_mode);
                    drop(dsrc);
                    let gate_up: Vec<Lin> = (0..n_expert)
                        .filter_map(|e| lin_row_view(&stacked_gu, e * 2 * expert_ff, 2 * expert_ff)).collect();
                    let down: Vec<Lin> = (0..n_expert)
                        .filter_map(|e| lin_row_view(&stacked_down, e * d, d)).collect();
                    if gate_up.len() != n_expert || down.len() != n_expert {
                        return Err("MoE expert stack isn't row-viewable (unexpected residency format)".into());
                    }
                    // Shared expert (ffn_{gate,up,down}_shexp.weight — GLM/DeepSeek/Qwen2-MoE):
                    // width read from the tensor itself (elems/d), order-of-dims-proof; fused
                    // gate||up concat mirrors the dense wgate_up layout SWIGLU_PACKED consumes.
                    let shared = if g.tensor(&format!("blk.{i}.ffn_gate_shexp.weight")).is_some()
                        && g.tensor(&format!("blk.{i}.ffn_up_shexp.weight")).is_some()
                        && g.tensor(&format!("blk.{i}.ffn_down_shexp.weight")).is_some() {
                        let gs = fetch(&format!("blk.{i}.ffn_gate_shexp.weight"))?;
                        let sff = gs.len() / d;
                        let us = fetch(&format!("blk.{i}.ffn_up_shexp.weight"))?;
                        let mut cat = Vec::with_capacity(2 * sff * d);
                        cat.extend_from_slice(&gs); cat.extend_from_slice(&us);
                        drop(gs); drop(us);
                        let sgu = self.make_lin(&cat, 2 * sff, d, q, q4_mode);
                        drop(cat);
                        let sdn = self.make_lin(&fetch(&format!("blk.{i}.ffn_down_shexp.weight"))?, d, sff, q, q4_mode);
                        if i == 0 { eprintln!("wgpu-engine: shared-expert MoE — shexp ff={sff} on top of {n_expert} routed"); }
                        Some((sgu, sdn))
                    } else { None };
                    Some(Box::new(MoeLayer { router, gate_up, down, stacked_gu, stacked_down, shared,
                        cfg: crate::archcfg::moe_config(&acfg, n_expert as u32, n_expert_used as u32, 0) }))
                } else { None },
                // Qwen3/3.5 per-head QK-Norm ([head_dim] each), applied before RoPE in the block forward.
                q_norm: if has_qk_norm { Some(self.upload(&fetch(&format!("blk.{i}.attn_q_norm.weight"))?, 1, head_dim)) } else { None },
                k_norm: if has_qk_norm { Some(self.upload(&fetch(&format!("blk.{i}.attn_k_norm.weight"))?, 1, head_dim)) } else { None },
            });
        }
        let out_norm = self.upload(&fetch("output_norm.weight")?, 1, d);
        // Llama-3.2 ties the output projection to the embedding (no `output.weight`
        // tensor); reuse the embedding matrix [vocab, d] in that case.
        let output = if g.tensor("output.weight").is_some() {
            lin("output.weight", vocab, d, q)?
        } else {
            self.make_lin(&embed, vocab, d, q, q4_mode)
        };
        // RoPE frequency factors (llama3 long-context scaling). GGUF ships them as
        // `rope_freqs.weight` (length head_dim/2); absent → all 1.0 (plain RoPE).
        let hd_half = head_dim / 2;
        let rope_vals = match g.tensor("rope_freqs.weight") {
            Some(_) => { let v = fetch("rope_freqs.weight")?; if v.len() == hd_half { v } else { vec![1.0; hd_half] } }
            None => vec![1.0; hd_half],
        };
        // YaRN long-context scaling (`rope.scaling.type = "yarn"` — Qwen 128K+ checkpoints, GLM,
        // DeepSeek): the per-dim frequency factors fold into the SAME ff buffer every rope kernel
        // already divides by — zero kernel changes for the frequencies — and the attention mscale
        // rides the Gpu-level `rope_mscale` (multiplies the rotated output, both q and k: the ggml
        // formulation, so scores scale by mscale²). Stored per load like `rope_neox`.
        let mut rope_vals = rope_vals;
        let mut mscale = 1.0f32;
        if g.get_str(&format!("{pfx}.rope.scaling.type")) == Some("yarn") {
            let sf = g.get_f32(&format!("{pfx}.rope.scaling.factor")).unwrap_or(1.0);
            let orig = g.get_u32(&format!("{pfx}.rope.scaling.original_context_length")).unwrap_or(0);
            let attnf = g.get_f32(&format!("{pfx}.rope.scaling.attn_factor")).unwrap_or(1.0);
            if sf > 1.0 && orig > 0 {
                let (ffv, ms) = yarn_factors(head_dim, base, sf, orig, 32.0, 1.0, attnf);
                rope_vals = ffv; mscale = ms;
                eprintln!("wgpu-engine: YaRN rope scaling ON — {sf}× over {orig} native ctx (mscale {mscale:.4})");
            }
        }
        self.rope_mscale.store(mscale.to_bits(), std::sync::atomic::Ordering::Relaxed);
        let rope_factors = self.upload(&rope_vals, 1, hd_half);
        Ok(Model { embed, layers, out_norm, output, rope_factors, d, h, vocab, n_layers: n, n_heads, n_kv_heads, head_dim, base, eps })
    }

    // ── int8-resident weight cache ───────────────────────────────────────────────────────────
    // The f32 dequant round-trip (GGUF → f32 → int8) is the bulk of load time and runs on every
    // launch/crash-recovery. After the first load we write the READY int8 weights to a sidecar
    // blob; later loads read it straight into GPU buffers, skipping the dequant entirely. On a
    // unified-memory device this is the compact "GPU object" worth keeping resident — and a page-
    // cache-warm reload after a crash costs a buffer upload, not a multi-GB dequant. Validated
    // bit-exact vs a fresh GGUF load (kernel-validate `int8 cache`). Hand-rolled LE; no deps.
    fn dump_tensor(&self, o: &mut Vec<u8>, t: &Tensor) {
        o.extend_from_slice(&(t.rows as u32).to_le_bytes());
        o.extend_from_slice(&(t.cols as u32).to_le_bytes());
        let v = self.download(t);
        o.extend_from_slice(&(v.len() as u64).to_le_bytes());
        o.extend_from_slice(bytemuck::cast_slice(&v));
    }
    fn dump_lin(&self, o: &mut Vec<u8>, l: &Lin) {
        match &l.kind { // biases are never cached (biased models skip the cache write, see save_int8_cache)
            LinKind::F32(t) => { o.push(0); self.dump_tensor(o, t); }
            LinKind::Q8(q) => { o.push(1); o.extend_from_slice(&(q.n as u32).to_le_bytes()); o.extend_from_slice(&(q.k as u32).to_le_bytes());
                self.dump_tensor(o, &q.qw); self.dump_tensor(o, &q.qs); }
            LinKind::Q4(q) => { o.push(2); o.extend_from_slice(&(q.n as u32).to_le_bytes()); o.extend_from_slice(&(q.k as u32).to_le_bytes());
                self.dump_tensor(o, &q.qw); self.dump_tensor(o, &q.qs); }
        }
    }
    /// Write the resident model to `path`, tagged with the source GGUF's (size, mtime) so a changed
    /// model invalidates the cache.
    /// Returns Ok(true) if a cache was written, Ok(false) if this model isn't cacheable (so the caller
    /// can log honestly — "wrote" was previously printed even on skip).
    pub fn save_int8_cache(&self, m: &Model, path: &std::path::Path, fp: (u64, u64)) -> std::io::Result<bool> {
        // Biased (Qwen2), QK-Norm (Qwen3/3.5), MoE (expert weights aren't in the layout), and 4-bit-resident
        // projections aren't in the int8 cache layout, so skip the cache — they reload from the GGUF.
        let q4_mode = std::env::var("LOGSCOPE_Q4_RESIDENT").is_ok();
        if q4_mode || m.layers.iter().any(|b| b.wq.has_bias() || b.wk.has_bias() || b.wv.has_bias() || b.q_norm.is_some() || b.moe.is_some()) { return Ok(false); }
        let mut o = Vec::new();
        o.extend_from_slice(b"LSI8C\x03"); // v3: bumped after the head_dim/attention surgery — invalidates any cache written mid-refactor so weights regenerate from the GGUF with the current loader
        o.extend_from_slice(&fp.0.to_le_bytes()); o.extend_from_slice(&fp.1.to_le_bytes());
        for v in [m.d, m.h, m.vocab, m.n_layers, m.n_heads, m.n_kv_heads] { o.extend_from_slice(&(v as u32).to_le_bytes()); }
        o.extend_from_slice(&m.base.to_le_bytes()); o.extend_from_slice(&m.eps.to_le_bytes());
        o.extend_from_slice(&(m.embed.len() as u64).to_le_bytes()); o.extend_from_slice(bytemuck::cast_slice(&m.embed));
        self.dump_tensor(&mut o, &m.rope_factors);
        self.dump_tensor(&mut o, &m.out_norm);
        self.dump_lin(&mut o, &m.output);
        for b in &m.layers {
            self.dump_tensor(&mut o, &b.g1);
            for l in [&b.wqkv, &b.wq, &b.wk, &b.wv, &b.wo, &b.wgate_up, &b.wdown] { self.dump_lin(&mut o, l); }
            self.dump_tensor(&mut o, &b.g2);
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &o)?;
        std::fs::rename(&tmp, path)?;  // never leave a half-written cache
        Ok(true)
    }
    /// Load the resident model from `path` iff it exists and its tag matches `fp`. None on any
    /// mismatch/corruption — the caller falls back to a fresh GGUF load.
    pub fn load_int8_cache(&self, path: &std::path::Path, fp: (u64, u64)) -> Option<Model> {
        // In q4 mode the int8 cache (keyed only on the GGUF) would hand back q8 weights — bypass it.
        if std::env::var("LOGSCOPE_Q4_RESIDENT").is_ok() { return None; }
        let data = std::fs::read(path).ok()?;
        let mut r = Rd { d: &data, p: 0 };
        if r.take(6)? != b"LSI8C\x03" || (r.u64()?, r.u64()?) != fp { return None; }
        let (d, h, vocab) = (r.u32()? as usize, r.u32()? as usize, r.u32()? as usize);
        let (n_layers, n_heads, n_kv_heads) = (r.u32()? as usize, r.u32()? as usize, r.u32()? as usize);
        let (base, eps) = (r.f32()?, r.f32()?);
        let embed = r.f32s()?;
        let rope_factors = self.read_tensor(&mut r)?;
        let out_norm = self.read_tensor(&mut r)?;
        let output = self.read_lin(&mut r)?;
        let mut layers = Vec::with_capacity(n_layers);
        for _ in 0..n_layers {
            let g1 = self.read_tensor(&mut r)?;
            let wqkv = self.read_lin(&mut r)?;
            let (wq, wk, wv) = (self.read_lin(&mut r)?, self.read_lin(&mut r)?, self.read_lin(&mut r)?);
            let (wo, wgate_up, wdown) = (self.read_lin(&mut r)?, self.read_lin(&mut r)?, self.read_lin(&mut r)?);
            let g2 = self.read_tensor(&mut r)?;
            layers.push(BlockWeights { wqkv, wq, wk, wv, wo, wgate_up, wdown, g1, g2, q_norm: None, k_norm: None, moe: None });
        }
        // Cached models are llama-family only (bias/QK-Norm/MoE are never cached) → adjacent-pair RoPE.
        self.rope_neox.store(false, std::sync::atomic::Ordering::Relaxed);
        Some(Model { embed, layers, out_norm, output, rope_factors, d, h, vocab, n_layers, n_heads, n_kv_heads, head_dim: d / n_heads.max(1), base, eps })
    }
    fn read_tensor(&self, r: &mut Rd) -> Option<Tensor> {
        let (rows, cols) = (r.u32()? as usize, r.u32()? as usize);
        let v = r.f32s()?;
        (v.len() == rows * cols).then(|| self.upload(&v, rows, cols))
    }
    fn read_lin(&self, r: &mut Rd) -> Option<Lin> {
        match r.u8()? {
            0 => Some(Lin::f32(self.read_tensor(r)?)),
            1 => {
                let (n, k) = (r.u32()? as usize, r.u32()? as usize);
                let (rows, cols) = (r.u32()? as usize, r.u32()? as usize);
                let qwf = r.f32s()?;
                if qwf.len() != rows * cols { return None; }
                let qw_u32: Vec<u32> = qwf.iter().map(|f| f.to_bits()).collect(); // recover packed int8
                let qw = self.upload_u32(&qw_u32);
                let qs = self.read_tensor(r)?;
                Some(Lin::q8(QWeight { qw, qs, n, k }))
            }
            2 => {
                let (n, k) = (r.u32()? as usize, r.u32()? as usize);
                let (rows, cols) = (r.u32()? as usize, r.u32()? as usize);
                let qwf = r.f32s()?;
                if qwf.len() != rows * cols { return None; }
                let qw_u32: Vec<u32> = qwf.iter().map(|f| f.to_bits()).collect(); // recover packed nibbles
                let qw = self.upload_u32(&qw_u32);
                let qs = self.read_tensor(r)?;
                Some(Lin::q4(Q4Weight { qw, qs, n, k }))
            }
            _ => None,
        }
    }

    /// Embedding lookup (host) → stacked resident blocks → final RMSNorm → logits.
    /// Returns logits [tokens × vocab].
    pub fn model_forward(&self, m: &Model, ids: &[u32]) -> Vec<f32> {
        let t = ids.len();
        let mut xh = Vec::with_capacity(t * m.d);
        for &id in ids { let s = id as usize * m.d; xh.extend_from_slice(&m.embed[s..s + m.d]); }
        let mut x = self.upload(&xh, t, m.d);
        for layer in &m.layers { x = self.block_forward(layer, &x, &m.rope_factors, t, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps); }
        let xn = self.rmsnorm_t(&x, &m.out_norm, t, m.d, m.eps);
        let logits = m.output.apply(self, &xn, t);
        self.download(&logits)
    }

    /// Text-encoder forward for DiT conditioning (e.g. Z-Image-Turbo's Qwen3 text encoder): identical to
    /// `model_forward` through the final norm, but returns the `[t, d]` hidden states directly instead of
    /// projecting through the LM head — there's no next-token prediction here, just an embedding of the
    /// prompt for `dit_conditioning`/cross-attention to consume. Every real Qwen3 weight (including
    /// QK-Norm) is exercised identically to normal text generation; only the last step differs.
    pub fn text_forward_hidden(&self, m: &Model, ids: &[u32]) -> Tensor {
        let t = ids.len();
        let mut xh = Vec::with_capacity(t * m.d);
        for &id in ids { let s = id as usize * m.d; xh.extend_from_slice(&m.embed[s..s + m.d]); }
        let mut x = self.upload(&xh, t, m.d);
        for layer in &m.layers { x = self.block_forward(layer, &x, &m.rope_factors, t, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps); }
        self.rmsnorm_t(&x, &m.out_norm, t, m.d, m.eps)
    }

    /// Extract row `row` of a `[rows, cols]` tensor as a fresh `[1, cols]` tensor (a buffer copy, not a
    /// dispatch). Used to pool a text encoder's hidden states down to the single vector `dit_conditioning`
    /// needs — by convention the LAST token's hidden state (the `t-1` row), matching how embedding /
    /// conditioning models typically read a causal encoder's summary of the whole sequence.
    pub fn row_at(&self, x: &Tensor, row: usize, cols: usize) -> Tensor {
        let out = self.alloc(1, cols);
        self.copy_buf(x, (row * cols * 4) as u64, &out, 0, (cols * 4) as u64);
        out
    }

    /// Build the `[1, d_dit]` conditioning vector `dit_block` expects: a timestep embedding pushed
    /// through its own 2-layer SiLU MLP, plus the text encoder's pooled (last-token) hidden state pushed
    /// through a projection `Lin` (needed since the text encoder's width and the DiT's width generally
    /// differ), summed elementwise. `text_hidden` is `text_forward_hidden`'s `[t, d_text]` output.
    pub fn dit_conditioning(&self, text_hidden: &Tensor, d_text: usize, timestep: f32, ts_dim: usize, ts_fc1: &Lin, ts_fc2: &Lin, text_proj: &Lin) -> Tensor {
        let t_emb = timestep_embedding(timestep, ts_dim, 10000.0);
        let t_emb_t = self.upload(&t_emb, 1, ts_dim);
        let t_h = ts_fc2.apply(self, &self.silu_t(&ts_fc1.apply(self, &t_emb_t, 1)), 1);
        let pooled = self.row_at(text_hidden, text_hidden.rows - 1, d_text);
        let txt_h = text_proj.apply(self, &pooled, 1);
        self.add_t(&t_h, &txt_h)
    }

    /// Like `model_forward` but prints the RMS and max-abs of the LAST token's hidden state
    /// after the embedding and after every block. A healthy stack grows/holds a stable norm
    /// and stays input-sensitive; a collapse or explosion at layer k localizes the fault.
    pub fn model_forward_trace(&self, m: &Model, ids: &[u32], ff: &Tensor) -> Vec<f32> {
        let t = ids.len();
        let stat = |v: &[f32]| -> f32 {
            let row = &v[(t - 1) * m.d..t * m.d];
            (row.iter().map(|x| x * x).sum::<f32>() / m.d as f32).sqrt()
        };
        let mut xh = Vec::with_capacity(t * m.d);
        for &id in ids { let s = (id as usize).min(m.vocab - 1) * m.d; xh.extend_from_slice(&m.embed[s..s + m.d]); }
        eprintln!("    eps={:.2e} embed rms={:.4}", m.eps, stat(&xh));
        let mut x = self.upload(&xh, t, m.d);
        let hd = m.head_dim; let kv = m.n_kv_heads * hd; let ad = m.n_heads * hd;
        for (li, w) in m.layers.iter().enumerate() {
            // inline of block_forward, measuring each contribution's last-token RMS
            let n1 = self.rmsnorm_t(&x, &w.g1, t, m.d, m.eps);
            let q = self.rope_mh_t(&self.qk_norm(w.wq.apply(self, &n1, t), &w.q_norm, t, ad, hd, m.eps), ff, t, ad, m.base, 0, hd);
            let k = self.rope_mh_t(&self.qk_norm(w.wk.apply(self, &n1, t), &w.k_norm, t, kv, hd, m.eps), ff, t, kv, m.base, 0, hd);
            let v = w.wv.apply(self, &n1, t);
            let attn = self.attention_mh_t(&q, &k, &v, t, ad, m.n_heads, m.n_kv_heads);
            let o = w.wo.apply(self, &attn, t);
            let x2 = self.add_t(&x, &o);
            // MoE layers: use the real routed FFN for correctness (the gu-cracking diag below only runs dense)
            if w.moe.is_some() { x = self.ffn_apply(w, &x2, t, m.d, m.eps); continue; }
            let n2 = self.rmsnorm_t(&x2, &w.g2, t, m.d, m.eps);
            let gu = w.wgate_up.apply(self, &n2, t);
            let hmid = self.swiglu_packed(&gu);
            let down = w.wdown.apply(self, &hmid, t);
            x = self.add_t(&x2, &down);
            // crack open the FFN for the last token: gate/up are the two halves of `gu`.
            let gud = self.download(&gu);
            let ff = gud.len() / t / 2;
            let rms_of = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32).sqrt();
            let gate_rms = rms_of(&gud[(t - 1) * 2 * ff..(t - 1) * 2 * ff + ff]);
            let up_rms = rms_of(&gud[(t - 1) * 2 * ff + ff..t * 2 * ff]);
            let hmid_rms = { let h = self.download(&hmid); rms_of(&h[(t - 1) * ff..t * ff]) };
            eprintln!("    blk {:>2}: n1={:.3} attn={:.3} | n2={:.3} gate={:.3} up={:.3} hmid={:.3} ffn={:.3} -> resid={:.4}",
                li, stat(&self.download(&n1)), stat(&self.download(&o)),
                stat(&self.download(&n2)), gate_rms, up_rms, hmid_rms, stat(&self.download(&down)), stat(&self.download(&x)));
        }
        let xn = self.rmsnorm_t(&x, &m.out_norm, t, m.d, m.eps);
        eprintln!("    out_norm rms={:.4}", stat(&self.download(&xn)));
        self.download(&m.output.apply(self, &xn, t))
    }
}

const ROPE_MH: &str = r#"
struct D { t:u32,dm:u32,base:f32,pos0:u32,hd:u32,neox:u32,msc:f32,_c:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> ff: array<f32>;   // per-dim-pair freq factors (len hd/2)
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let half=dd.hd/2u; let per=dd.dm/2u; let p=g.y*nwg.x*64u+g.x; if(p>=dd.t*per){return;}
  let tok=p/per; let within=p%per; let head=within/half; let i=within%half;
  let off=tok*dd.dm + head*dd.hd;
  let freq=f32(tok+dd.pos0)/(pow(dd.base,(2.0*f32(i))/f32(dd.hd))*ff[i]);
  let c=cos(freq)*dd.msc; let s=sin(freq)*dd.msc;
  // Pairing differs by family, angles do not. llama (NORM): rotate adjacent (2i, 2i+1).
  // qwen/neox (NEOX): rotate split halves (i, i+hd/2). Wrong pairing = real words, scrambled meaning.
  // Branch-free arithmetic pairing (no select — removes any backend-specific miscompile surface):
  //   j0 = i*(2-neox)             -> NORM: 2i     NEOX: i
  //   j1 = j0 + 1 + neox*(half-1) -> NORM: 2i+1   NEOX: i+half
  let j0=off + i*(2u-dd.neox);
  let j1=j0 + 1u + dd.neox*(half-1u);
  let x0=x[j0]; let x1=x[j1];
  o[j0]=x0*c-x1*s;
  o[j1]=x0*s+x1*c;
}"#;

// Per-row-position RoPE: like ROPE_MH but each row reads its own absolute position
// from `pos[row]` instead of pos0+row. Needed for batched decode, where the N rows
// are N different sessions at N different positions.
const ROPE_MH_ROWS: &str = r#"
struct D { t:u32,dm:u32,base:f32,_p:u32,hd:u32,neox:u32,msc:f32,_c:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> ff: array<f32>;
@group(0)@binding(2) var<storage,read> pos: array<u32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let half=dd.hd/2u; let per=dd.dm/2u; let p=g.y*nwg.x*64u+g.x; if(p>=dd.t*per){return;}
  let tok=p/per; let within=p%per; let head=within/half; let i=within%half;
  let off=tok*dd.dm + head*dd.hd;
  let freq=f32(pos[tok])/(pow(dd.base,(2.0*f32(i))/f32(dd.hd))*ff[i]);
  let c=cos(freq)*dd.msc; let s=sin(freq)*dd.msc;
  // Branch-free arithmetic pairing (no select — removes any backend-specific miscompile surface):
  //   j0 = i*(2-neox)             -> NORM: 2i     NEOX: i
  //   j1 = j0 + 1 + neox*(half-1) -> NORM: 2i+1   NEOX: i+half
  let j0=off + i*(2u-dd.neox);
  let j1=j0 + 1u + dd.neox*(half-1u);
  let x0=x[j0]; let x1=x[j1];
  o[j0]=x0*c-x1*s;
  o[j1]=x0*s+x1*c;
}"#;

const ATTENTION_MH: &str = r#"
struct D { t:u32,hd:u32,nh:u32,nkv:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> k: array<f32>;
@group(0)@binding(2) var<storage,read> v: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let id=g.x; if(id>=dd.t*dd.nh){return;}
  let i=id/dd.nh; let hh=id%dd.nh;
  let dm=dd.nh*dd.hd; let kvdm=dd.nkv*dd.hd;
  let b=hh*dd.hd; let kb=(hh/(dd.nh/dd.nkv))*dd.hd;  // GQA: query head -> shared KV head
  let scale=1.0/sqrt(f32(dd.hd));
  var mx=-3.4e38;
  for(var j=0u;j<=i;j=j+1u){ var dot=0.0; for(var c=0u;c<dd.hd;c=c+1u){dot=dot+q[i*dm+b+c]*k[j*kvdm+kb+c];} mx=max(mx,dot*scale); }
  for(var c=0u;c<dd.hd;c=c+1u){o[i*dm+b+c]=0.0;}
  var sum=0.0;
  for(var j=0u;j<=i;j=j+1u){
    var dot=0.0; for(var c=0u;c<dd.hd;c=c+1u){dot=dot+q[i*dm+b+c]*k[j*kvdm+kb+c];}
    let p=exp(dot*scale-mx); sum=sum+p;
    for(var c=0u;c<dd.hd;c=c+1u){o[i*dm+b+c]=o[i*dm+b+c]+p*v[j*kvdm+kb+c];}
  }
  for(var c=0u;c<dd.hd;c=c+1u){o[i*dm+b+c]=o[i*dm+b+c]/sum;}
}"#;

// Prefill-extend attention: a segment of `s` queries at global positions [base, base+s)
// attends over a cache already holding [0, base+s) rows (parent prefix + this segment),
// causal by global position. Lets a shared segment be prefilled on top of a parent cache
// in O(s·(base+s)) instead of recomputing the parent — the batched `extend_prefix`.
const ATTENTION_MH_EXT: &str = r#"
struct D { s:u32,hd:u32,nh:u32,nkv:u32,base:u32,_a:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> kc: array<f32>;
@group(0)@binding(2) var<storage,read> vc: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let id=g.x; if(id>=dd.s*dd.nh){return;}
  let i=id/dd.nh; let hh=id%dd.nh;
  let dm=dd.nh*dd.hd; let kvdm=dd.nkv*dd.hd;
  let b=hh*dd.hd; let kb=(hh/(dd.nh/dd.nkv))*dd.hd;  // GQA: query head -> shared KV head
  let scale=1.0/sqrt(f32(dd.hd));
  let qpos=dd.base+i;                                 // global position of this query row
  var mx=-3.4e38;
  for(var j=0u;j<=qpos;j=j+1u){ var dot=0.0; for(var c=0u;c<dd.hd;c=c+1u){dot=dot+q[i*dm+b+c]*kc[j*kvdm+kb+c];} mx=max(mx,dot*scale); }
  for(var c=0u;c<dd.hd;c=c+1u){o[i*dm+b+c]=0.0;}
  var sum=0.0;
  for(var j=0u;j<=qpos;j=j+1u){
    var dot=0.0; for(var c=0u;c<dd.hd;c=c+1u){dot=dot+q[i*dm+b+c]*kc[j*kvdm+kb+c];}
    let p=exp(dot*scale-mx); sum=sum+p;
    for(var c=0u;c<dd.hd;c=c+1u){o[i*dm+b+c]=o[i*dm+b+c]+p*vc[j*kvdm+kb+c];}
  }
  for(var c=0u;c<dd.hd;c=c+1u){o[i*dm+b+c]=o[i*dm+b+c]/sum;}
}"#;

// Flash-style SPLIT-K decode attention, pass 1 of 2 — the long-context kernel. One workgroup per
// (query head, KV chunk): each computes its chunk's LOCAL softmax pieces — running max, exp-sum,
// and the unnormalized V-weighted accumulation — into a partials buffer [nh][nch][hd+2]. Pass 2
// combines chunks with the standard online-softmax rescale. Parallelism grows with context
// (heads × chunks workgroups) instead of saturating at n_heads; nothing needs scores[l] in shared
// memory, so there is no 4096-row cap. Numerics: per-chunk max + rescale reorders the summation
// vs the monolithic kernels — same tolerance class as the other fused paths (~1e-6 rel).
const DECODE_ATTN_FLASH_P1: &str = r#"
struct D { l:u32,hd:u32,nh:u32,nkv:u32,chunk:u32,nch:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> kc: array<f32>;
@group(0)@binding(2) var<storage,read> vc: array<f32>;
@group(0)@binding(3) var<storage,read_write> part: array<f32>;
@group(0)@binding(4) var<uniform> dd: D;
var<workgroup> sc: array<f32,2048>;
var<workgroup> red: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let hh=wg.x; let ch=wg.y; let tid=lid.x;
  let kvdm=dd.nkv*dd.hd;
  let b=hh*dd.hd; let kb=(hh/(dd.nh/dd.nkv))*dd.hd;  // GQA: query head -> shared KV head
  let scale=1.0/sqrt(f32(dd.hd));
  let j0=ch*dd.chunk; let j1=min(dd.l, j0+dd.chunk);
  let pbase=(hh*dd.nch+ch)*(dd.hd+2u);
  var lmax=-3.4e38; var j=j0+tid;
  loop { if(j>=j1){break;} var dot=0.0; for(var c=0u;c<dd.hd;c=c+1u){dot=dot+q[b+c]*kc[j*kvdm+kb+c];} let s=dot*scale; sc[j-j0]=s; lmax=max(lmax,s); j=j+64u; }
  red[tid]=lmax; workgroupBarrier();
  for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ red[tid]=max(red[tid],red[tid+st]); } workgroupBarrier(); }
  let mx=red[0]; workgroupBarrier();
  var lsum=0.0; j=j0+tid;
  loop { if(j>=j1){break;} let p=exp(sc[j-j0]-mx); sc[j-j0]=p; lsum=lsum+p; j=j+64u; }
  red[tid]=lsum; workgroupBarrier();
  for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ red[tid]=red[tid]+red[tid+st]; } workgroupBarrier(); }
  let sum=red[0]; workgroupBarrier();
  var c=tid;
  loop { if(c>=dd.hd){break;} var acc=0.0; for(var jj=j0;jj<j1;jj=jj+1u){ acc=acc+sc[jj-j0]*vc[jj*kvdm+kb+c]; } part[pbase+c]=acc; c=c+64u; }
  if(tid==0u){ part[pbase+dd.hd]=mx; part[pbase+dd.hd+1u]=sum; }
}"#;

// Split-K decode attention, pass 2: one workgroup per head folds its chunks — global max M,
// rescaled sum S = Σ sᵢ·exp(mᵢ−M), and o = Σ oᵢ·exp(mᵢ−M) / S.
const DECODE_ATTN_FLASH_P2: &str = r#"
struct D { hd:u32,nch:u32,nh:u32,_a:u32 };
@group(0)@binding(0) var<storage,read> part: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> dd: D;
var<workgroup> wm: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let hh=wg.x; let tid=lid.x;
  var m=-3.4e38; var i=tid;
  loop { if(i>=dd.nch){break;} m=max(m, part[(hh*dd.nch+i)*(dd.hd+2u)+dd.hd]); i=i+64u; }
  wm[tid]=m; workgroupBarrier();
  for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ wm[tid]=max(wm[tid],wm[tid+st]); } workgroupBarrier(); }
  let M=wm[0]; workgroupBarrier();
  var s=0.0; i=tid;
  loop { if(i>=dd.nch){break;} let pb=(hh*dd.nch+i)*(dd.hd+2u); s=s+part[pb+dd.hd+1u]*exp(part[pb+dd.hd]-M); i=i+64u; }
  wm[tid]=s; workgroupBarrier();
  for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ wm[tid]=wm[tid]+wm[tid+st]; } workgroupBarrier(); }
  let S=wm[0];
  var c=tid;
  loop { if(c>=dd.hd){break;}
    var acc=0.0;
    for(var ii=0u;ii<dd.nch;ii=ii+1u){ let pb=(hh*dd.nch+ii)*(dd.hd+2u); acc=acc+part[pb+c]*exp(part[pb+dd.hd]-M); }
    o[hh*dd.hd+c]=acc/S; c=c+64u; }
}"#;

const DECODE_ATTN_MH_PAR: &str = r#"
struct D { l:u32,hd:u32,nh:u32,nkv:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> kc: array<f32>;
@group(0)@binding(2) var<storage,read> vc: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> dd: D;
var<workgroup> sc: array<f32,4096>;
var<workgroup> red: array<f32,64>;
// One workgroup per query head (so the GPU's many cores all work, vs the serial kernel's
// single workgroup). 64 threads split the KV rows for QK + softmax (shared-memory reduction),
// then split head_dim for the V-weighted sum (coalesced). Caller uses this only when l<=4096.
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let hh=wg.x; let tid=lid.x;
  let kvdm=dd.nkv*dd.hd;
  let b=hh*dd.hd; let kb=(hh/(dd.nh/dd.nkv))*dd.hd;  // GQA: query head -> shared KV head
  let scale=1.0/sqrt(f32(dd.hd));
  var lmax=-3.4e38; var j=tid;
  loop { if(j>=dd.l){break;} var dot=0.0; for(var c=0u;c<dd.hd;c=c+1u){dot=dot+q[b+c]*kc[j*kvdm+kb+c];} let s=dot*scale; sc[j]=s; lmax=max(lmax,s); j=j+64u; }
  red[tid]=lmax; workgroupBarrier();
  for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ red[tid]=max(red[tid],red[tid+st]); } workgroupBarrier(); }
  let mx=red[0]; workgroupBarrier();
  var lsum=0.0; j=tid;
  loop { if(j>=dd.l){break;} let p=exp(sc[j]-mx); sc[j]=p; lsum=lsum+p; j=j+64u; }
  red[tid]=lsum; workgroupBarrier();
  for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ red[tid]=red[tid]+red[tid+st]; } workgroupBarrier(); }
  let sum=red[0]; workgroupBarrier();
  var c=tid;
  loop { if(c>=dd.hd){break;} var acc=0.0; for(var jj=0u;jj<dd.l;jj=jj+1u){ acc=acc+sc[jj]*vc[jj*kvdm+kb+c]; } o[b+c]=acc/sum; c=c+64u; }
}"#;

const DECODE_ATTN_MH: &str = r#"
struct D { l:u32,hd:u32,nh:u32,nkv:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> kc: array<f32>;
@group(0)@binding(2) var<storage,read> vc: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let hh=g.x; if(hh>=dd.nh){return;}
  let dm=dd.nh*dd.hd; let kvdm=dd.nkv*dd.hd;
  let b=hh*dd.hd; let kb=(hh/(dd.nh/dd.nkv))*dd.hd;  // GQA: query head -> shared KV head
  let scale=1.0/sqrt(f32(dd.hd));
  var mx=-3.4e38;
  for(var j=0u;j<dd.l;j=j+1u){ var dot=0.0; for(var c=0u;c<dd.hd;c=c+1u){dot=dot+q[b+c]*kc[j*kvdm+kb+c];} mx=max(mx,dot*scale); }
  for(var c=0u;c<dd.hd;c=c+1u){o[b+c]=0.0;}
  var sum=0.0;
  for(var j=0u;j<dd.l;j=j+1u){
    var dot=0.0; for(var c=0u;c<dd.hd;c=c+1u){dot=dot+q[b+c]*kc[j*kvdm+kb+c];}
    let p=exp(dot*scale-mx); sum=sum+p;
    for(var c=0u;c<dd.hd;c=c+1u){o[b+c]=o[b+c]+p*vc[j*kvdm+kb+c];}
  }
  for(var c=0u;c<dd.hd;c=c+1u){o[b+c]=o[b+c]/sum;}
}"#;

// ── int8 KV cache: per-row-quantized K/V (4x smaller than f32) ───────────────
// Per-row absmax scale: scale = max|x|/127 over each row of [n,c].
const ROW_ABSMAX: &str = r#"
struct D { n:u32,c:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> scales: array<f32>;
@group(0)@binding(2) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let row=g.x; if(row>=dd.n){return;}
  var mx=0.0;
  for(var c=0u;c<dd.c;c=c+1u){ mx=max(mx, abs(x[row*dd.c+c])); }
  scales[row]=mx/127.0;
}"#;
// Quantize+pack [n,c] f32 into [n,c/4] u32 (4 int8 per word) using per-row scales.
const QUANT_PACK: &str = r#"
struct D { n:u32,c:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> scales: array<f32>;
@group(0)@binding(2) var<storage,read_write> q: array<u32>;
@group(0)@binding(3) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let words=dd.c/4u; let p=g.x; if(p>=dd.n*words){return;}
  let row=p/words; let w=p%words;
  let sc=scales[row]; let inv=select(1.0/sc, 0.0, sc==0.0);
  var packed=0u;
  for(var b=0u;b<4u;b=b+1u){
    let qi=clamp(i32(round(x[row*dd.c + w*4u + b]*inv)), -127, 127);
    packed=packed | ((u32(qi) & 0xffu) << (b*8u));
  }
  q[p]=packed;
}"#;
// Inverse of QUANT_PACK: unpack [n,c/4] u32 + per-row scales back to [n,c] f32. Promotes
// a demoted (int8) prefix cache to f32. AGX-safe signed extract (extractBits + select).
const DEQUANT_Q8_ROWS: &str = r#"
struct D { n:u32,c:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> q: array<u32>;
@group(0)@binding(1) var<storage,read> scales: array<f32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; if(i>=dd.n*dd.c){return;}
  let row=i/dd.c; let col=i%dd.c;
  let word=q[row*(dd.c/4u)+(col/4u)];
  let raw=extractBits(word, (col%4u)*8u, 8u);
  let val=select(f32(raw), f32(raw)-256.0, raw>127u);
  o[i]=val*scales[row];
}"#;
// GQA decode attention reading int8-packed K/V + per-row scales (dequant on the fly).
const DECODE_ATTN_MH_Q8: &str = r#"
struct D { l:u32,hd:u32,nh:u32,nkv:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> kq: array<u32>;
@group(0)@binding(2) var<storage,read> ks: array<f32>;
@group(0)@binding(3) var<storage,read> vq: array<u32>;
@group(0)@binding(4) var<storage,read> vs: array<f32>;
@group(0)@binding(5) var<storage,read_write> o: array<f32>;
@group(0)@binding(6) var<uniform> dd: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let hh=g.x; if(hh>=dd.nh){return;}
  let dm=dd.nh*dd.hd; let kvdm=dd.nkv*dd.hd; let kvw=kvdm/4u;
  let b=hh*dd.hd; let kb=(hh/(dd.nh/dd.nkv))*dd.hd;
  let scale=1.0/sqrt(f32(dd.hd));
  var mx=-3.4e38;
  for(var j=0u;j<dd.l;j=j+1u){
    let ksc=ks[j]; var dot=0.0;
    for(var c=0u;c<dd.hd;c=c+1u){
      let idx=kb+c; let word=kq[j*kvw + idx/4u];
      let ub=extractBits(word,(idx%4u)*8u,8u); let qi=select(i32(ub), i32(ub)-256, ub>=128u);
      dot=dot+q[b+c]*(f32(qi)*ksc);
    }
    mx=max(mx,dot*scale);
  }
  for(var c=0u;c<dd.hd;c=c+1u){o[b+c]=0.0;}
  var sum=0.0;
  for(var j=0u;j<dd.l;j=j+1u){
    let ksc=ks[j]; let vsc=vs[j]; var dot=0.0;
    for(var c=0u;c<dd.hd;c=c+1u){
      let idx=kb+c; let word=kq[j*kvw + idx/4u];
      let ub=extractBits(word,(idx%4u)*8u,8u); let qi=select(i32(ub), i32(ub)-256, ub>=128u);
      dot=dot+q[b+c]*(f32(qi)*ksc);
    }
    let p=exp(dot*scale-mx); sum=sum+p;
    for(var c=0u;c<dd.hd;c=c+1u){
      let idx=kb+c; let word=vq[j*kvw + idx/4u];
      let ub=extractBits(word,(idx%4u)*8u,8u); let qi=select(i32(ub), i32(ub)-256, ub>=128u);
      o[b+c]=o[b+c]+p*(f32(qi)*vsc);
    }
  }
  for(var c=0u;c<dd.hd;c=c+1u){o[b+c]=o[b+c]/sum;}
}"#;

/// Per-layer key/value cache (resident [max_seq, d] buffers).
pub struct KvCache { pub k: Vec<Tensor>, pub v: Vec<Tensor> }

/// int8-quantized KV cache: K/V stored per-row as packed int8 (4x smaller than f32)
/// plus one f32 scale per row. Raises the concurrency and context ceilings (KV memory
/// is the cap once compute scales). ~4x smaller than the f32 cache.
pub struct QKvCache { pub kq: Vec<Tensor>, pub ks: Vec<Tensor>, pub vq: Vec<Tensor>, pub vs: Vec<Tensor> }

/// A generation session backed by the int8 KV cache (4x smaller). Mirrors `Session`.
pub struct SessionQ8 {
    pub cache: QKvCache,
    pub pos: usize,
    pub logits: Vec<f32>,
    pub generated: Vec<u32>,
    pub rng: u64,
    pub done: bool,
}

/// One independent generation session: its own KV cache, stream position and RNG,
/// sharing the `Model`'s resident weights. Many `Session`s run concurrently against
/// a single resident `Model` — the per-session GPU cost is just the KV cache.
pub struct Session {
    pub cache: KvCache,
    pub pos: usize,
    pub logits: Vec<f32>,
    pub generated: Vec<u32>,
    pub rng: u64,
    pub done: bool,
}

/// A set of workers that share a common prompt prefix (e.g. the file under repair, or a
/// shared system prompt). The prefix is prefilled once and its KV cache is propagated to
/// every worker; each worker's `suffix` is only the unique tail after the shared prefix.
/// `workers` entries are `(id, suffix_tokens, max_new)`. Consumed by `Gpu::run_fleet`.
pub struct PrefixGroup {
    pub prefix: Vec<u32>,
    pub workers: Vec<(usize, Vec<u32>, usize)>,
}

/// A single fleet worker: its full prompt tokens and generation budget. Unlike
/// `PrefixGroup`, callers don't pre-group these — `run_fleet_trie` discovers the shared
/// structure across all requests automatically (a radix trie over the token sequences),
/// so workers that share a *path* (same file, then same function, then diverge) reuse
/// cache at every shared boundary, not just a single root prefix.
pub struct FleetRequest { pub id: usize, pub tokens: Vec<u32>, pub max_new: usize }

/// Radix-trie node over worker token sequences. `seg` is the edge label from the parent;
/// `enders` are worker indices whose full token sequence ends exactly here. `cum`/`count`/
/// `rep` (cumulative length, subtree worker count, a representative worker index) are
/// filled after the trie is built.
struct TrieNode {
    seg: Vec<u32>,
    children: Vec<usize>,
    enders: Vec<usize>,
    parent: usize, // usize::MAX for the root
    cum: usize,
    count: usize,
    rep: usize,
}

fn lcp_len(a: &[u32], b: &[u32]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

/// Insert worker `w`'s token sequence into the radix trie, splitting an edge where the
/// sequence diverges from an existing one.
fn trie_insert(nodes: &mut Vec<TrieNode>, tokens: &[u32], w: usize) {
    let (mut cur, mut i) = (0usize, 0usize);
    loop {
        if i == tokens.len() { nodes[cur].enders.push(w); return; }
        let child = nodes[cur].children.iter().copied().find(|&c| nodes[c].seg[0] == tokens[i]);
        match child {
            None => {
                let leaf = nodes.len();
                nodes.push(TrieNode { seg: tokens[i..].to_vec(), children: vec![], enders: vec![w],
                                      parent: cur, cum: 0, count: 0, rep: w });
                nodes[cur].children.push(leaf);
                return;
            }
            Some(c) => {
                let common = lcp_len(&nodes[c].seg, &tokens[i..]);
                if common == nodes[c].seg.len() { cur = c; i += common; continue; }
                // split edge `c` at `common`: insert a middle node carrying the shared head.
                let mid = nodes.len();
                let head = nodes[c].seg[..common].to_vec();
                let tail = nodes[c].seg[common..].to_vec();
                nodes.push(TrieNode { seg: head, children: vec![c], enders: vec![], parent: cur,
                                      cum: 0, count: 0, rep: w });
                nodes[c].seg = tail; nodes[c].parent = mid;
                let pos = nodes[cur].children.iter().position(|&x| x == c).unwrap();
                nodes[cur].children[pos] = mid;
                if i + common == tokens.len() { nodes[mid].enders.push(w); }
                else {
                    let leaf = nodes.len();
                    nodes.push(TrieNode { seg: tokens[i + common..].to_vec(), children: vec![],
                                          enders: vec![w], parent: mid, cum: 0, count: 0, rep: w });
                    nodes[mid].children.push(leaf);
                }
                return;
            }
        }
    }
}

/// A cached prefix in one of two compression tiers: hot f32 (fast to seed) or warm int8
/// (~4x smaller, lossy on promotion).
pub enum Tier { Hot(KvCache), Warm(QKvCache) }

struct CacheEntry { key: u64, len: usize, tier: Tier, hits: u32, last: u64 }

/// Size-bounded, value-ranked store of prefix KV caches for scaling fleets. While there's
/// room, prefixes are kept hot (f32). Under memory pressure the lowest-value entries are
/// first *demoted* to int8 (the warm tier) and only dropped if still over budget — a two-
/// layer compression policy rather than a hard LRU evict.
///
/// Value is a GDSF-style score: a prefix's rebuild cost is ~quadratic in its length
/// (prefill attention is O(len²)) while its memory is linear, so value-per-byte ∝
/// (hits+1)·len. A big, frequently-shared prefix (a hot file many workers touch) outranks
/// a small one-off; recency breaks ties. So eviction sheds the cheapest-to-rebuild,
/// least-shared caches first — what you want when a fleet is churning through many files.
pub struct PrefixStore {
    entries: Vec<CacheEntry>,
    budget_bytes: usize,
    clock: u64,
    n_layers: usize,
    kv_dim: usize,
}
impl PrefixStore {
    pub fn new(budget_bytes: usize, n_layers: usize, kv_dim: usize) -> Self {
        Self { entries: vec![], budget_bytes, clock: 0, n_layers, kv_dim }
    }
    fn hot_bytes(&self, len: usize) -> usize { 2 * len * self.kv_dim * 4 * self.n_layers }
    fn warm_bytes(&self, len: usize) -> usize { 2 * self.n_layers * (len * self.kv_dim + len * 4) }
    fn entry_bytes(&self, e: &CacheEntry) -> usize {
        match e.tier { Tier::Hot(_) => self.hot_bytes(e.len), Tier::Warm(_) => self.warm_bytes(e.len) }
    }
    pub fn usage(&self) -> usize { self.entries.iter().map(|e| self.entry_bytes(e)).sum() }
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    // value-per-byte ∝ (hits+1)·len; recency (last) breaks ties. Higher = keep.
    fn score(&self, e: &CacheEntry) -> (i64, u64) { ((e.hits as i64 + 1) * e.len as i64, e.last) }
    /// Peek a cached prefix's tier without changing its ranking.
    pub fn tier_of(&self, key: u64) -> Option<&Tier> {
        self.entries.iter().find(|e| e.key == key).map(|e| &e.tier)
    }
    /// Look up a cached prefix, bumping its frequency + recency. Hot ⇒ seed directly; Warm
    /// ⇒ `promote_cache` then seed (lossy).
    pub fn get(&mut self, key: u64) -> Option<&Tier> {
        self.clock += 1; let now = self.clock;
        if let Some(e) = self.entries.iter_mut().find(|e| e.key == key) {
            e.hits += 1; e.last = now; Some(&e.tier)
        } else { None }
    }
    /// Insert a freshly-built hot prefix cache, then enforce the budget.
    pub fn insert(&mut self, gpu: &Gpu, key: u64, len: usize, cache: KvCache) {
        self.clock += 1; let now = self.clock;
        if let Some(e) = self.entries.iter_mut().find(|e| e.key == key) {
            e.tier = Tier::Hot(cache); e.len = len; e.last = now;
        } else {
            self.entries.push(CacheEntry { key, len, tier: Tier::Hot(cache), hits: 0, last: now });
        }
        self.enforce(gpu);
    }
    /// Bring usage under budget: demote the lowest-value hot entries to int8 first; only if
    /// still over, drop the lowest-value entries outright. Returns (demoted, evicted).
    pub fn enforce(&mut self, gpu: &Gpu) -> (usize, usize) {
        let (mut demoted, mut evicted) = (0usize, 0usize);
        while self.usage() > self.budget_bytes {
            let pick = self.entries.iter().enumerate()
                .filter(|(_, e)| matches!(e.tier, Tier::Hot(_)))
                .min_by_key(|(_, e)| self.score(e)).map(|(i, _)| i);
            match pick {
                Some(i) => {
                    let len = self.entries[i].len;
                    if let Tier::Hot(c) = &self.entries[i].tier {
                        let q = gpu.demote_cache(c, len, self.kv_dim);
                        self.entries[i].tier = Tier::Warm(q); demoted += 1;
                    }
                }
                None => break,
            }
        }
        while self.usage() > self.budget_bytes && !self.entries.is_empty() {
            let i = self.entries.iter().enumerate().min_by_key(|(_, e)| self.score(e)).map(|(i, _)| i).unwrap();
            self.entries.remove(i); evicted += 1;
        }
        (demoted, evicted)
    }
}

/// Sinusoidal timestep embedding (the DiT/DDPM convention): for a scalar timestep `t` and embedding
/// width `dim`, returns `concat(cos(t*freq_i), sin(t*freq_i))` for `freq_i = max_period^(-i/half)`,
/// `i in 0..half`, `half = dim/2` (a trailing zero pad if `dim` is odd). This feeds a small
/// Linear→SiLU→Linear MLP (ordinary GEMM, already covered by existing kernels) whose output is the
/// conditioning vector that `dit_modulate`/`dit_gated_add` broadcast into every DiT block.
pub fn timestep_embedding(t: f32, dim: usize, max_period: f32) -> Vec<f32> {
    let half = dim / 2;
    let mut emb = vec![0.0f32; dim];
    for i in 0..half {
        let freq = max_period.powf(-(i as f32) / half as f32);
        let arg = t * freq;
        emb[i] = arg.cos();
        emb[half + i] = arg.sin();
    }
    emb
}

/// Workgroup grid for a 1-D dispatch of `n_elems` (workgroup_size 64), splitting the
/// count across the y dimension when the x count would exceed the 65535 per-dim limit.
/// Shaders using this compute their flat index as `g.y * nwg.x * 64 + g.x`.
fn wg1(n_elems: u32) -> (u32, u32, u32) {
    let wg = n_elems.div_ceil(64);
    if wg <= 65535 { (wg, 1, 1) } else { (65535, wg.div_ceil(65535), 1) }
}

/// Streaming-window KV slot policy (attention sinks + sliding window). For logical
/// position `p`, returns `(physical_slot, attend_len)`: while the cache is below the
/// `sink+window` cap it fills linearly; once full, the new token overwrites the slot of
/// the oldest window token (logical `p-window`) — an O(1) ring eviction with no shifting
/// and no per-token score bookkeeping, so the KV stays bounded at `sink+window` rows for
/// an arbitrarily long stream. The `sink` pinned tokens preserve the attention sinks that
/// keep quality from collapsing under a pure sliding window.
pub fn stream_slot(p: usize, sink: usize, window: usize) -> (usize, usize) {
    if p < sink + window { (p, p + 1) } else { (sink + (p - sink) % window, sink + window) }
}

pub fn argmax(v: &[f32]) -> u32 {
    let mut bi = 0usize; let mut bv = f32::MIN;
    for (i, &x) in v.iter().enumerate() { if x > bv { bv = x; bi = i; } }
    bi as u32
}

/// How far a candidate logit vector diverges from a reference one. `argmax_match` is the
/// greedy accept test (verify-then-accept for speculation / lossy KV); `cosine` and `kl`
/// (of the softmaxes) gauge distribution drift for sampled decoding. A precision/eviction
/// policy thresholds on these to decide whether a lossy or speculated result is usable.
pub struct LogitCmp { pub argmax_match: bool, pub max_abs: f32, pub cosine: f32, pub kl: f32 }

/// Compare a candidate logit vector against a reference. See `LogitCmp`.
pub fn compare_logits(reference: &[f32], candidate: &[f32]) -> LogitCmp {
    let am = |v: &[f32]| { let (mut bi, mut bv) = (0usize, f32::MIN); for (i, &x) in v.iter().enumerate() { if x > bv { bv = x; bi = i; } } bi };
    let max_abs = reference.iter().zip(candidate).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
    let (mut dot, mut na, mut nb) = (0f64, 0f64, 0f64);
    for (a, b) in reference.iter().zip(candidate) {
        dot += *a as f64 * *b as f64; na += (*a as f64).powi(2); nb += (*b as f64).powi(2);
    }
    let cosine = (dot / (na.sqrt() * nb.sqrt() + 1e-12)) as f32;
    let softmax = |v: &[f32]| { let mx = v.iter().cloned().fold(f32::MIN, f32::max);
        let ex: Vec<f64> = v.iter().map(|x| ((x - mx) as f64).exp()).collect();
        let s: f64 = ex.iter().sum(); ex.into_iter().map(|e| e / s).collect::<Vec<f64>>() };
    let (p, q) = (softmax(reference), softmax(candidate));
    let mut kl = 0f64;
    for i in 0..p.len() { if p[i] > 1e-12 { kl += p[i] * ((p[i] + 1e-12) / (q[i] + 1e-12)).ln(); } }
    LogitCmp { argmax_match: am(reference) == am(candidate), max_abs, cosine, kl: kl as f32 }
}

impl Gpu {
    /// RoPE with an absolute position offset (decode applies it at `pos0`, not row 0).
    pub fn rope_at_t(&self, x: &Tensor, t: usize, d: usize, base: f32, pos0: usize) -> Tensor {
        let out = self.alloc(t, d); let dims = [t as u32, d as u32, base.to_bits(), pos0 as u32];
        self.dispatch(ROPE, &[x], &out, bytemuck::cast_slice(&dims), (((t * d / 2) as u32).div_ceil(64), 1, 1)); out
    }
    pub fn new_cache(&self, n_layers: usize, max_seq: usize, d: usize) -> KvCache {
        KvCache { k: (0..n_layers).map(|_| self.alloc(max_seq, d)).collect(),
                  v: (0..n_layers).map(|_| self.alloc(max_seq, d)).collect() }
    }
    /// Buffer-to-buffer copy that records into the open frame if one is active (so cache
    /// writes batch with the token's compute), else submits on its own.
    fn copy_buf(&self, src: &Tensor, src_off: u64, dst: &Tensor, dst_off: u64, bytes: u64) {
        if let Some(frame) = self.frame.lock().unwrap().as_mut() {
            frame.enc.copy_buffer_to_buffer(&src.buf, src_off, &dst.buf, dst_off, bytes);
        } else {
            let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            enc.copy_buffer_to_buffer(&src.buf, src_off, &dst.buf, dst_off, bytes);
            self.submit(enc);
        }
    }
    /// Copy a [1, cols] row tensor into `cache` at `row` (GPU buffer-to-buffer).
    fn write_row(&self, cache: &Tensor, src: &Tensor, row: usize, cols: usize) {
        self.copy_buf(src, 0, cache, (row * cols * 4) as u64, (cols * 4) as u64);
    }
    /// Copy `n` contiguous rows from the start of `src` into `cache` at `start_row`.
    fn write_rows(&self, cache: &Tensor, src: &Tensor, start_row: usize, n: usize, cols: usize) {
        if n == 0 { return; }
        self.copy_buf(src, 0, cache, (start_row * cols * 4) as u64, (n * cols * 4) as u64);
    }
    /// Copy one row `src_row` of `src` into row `dst_row` of `dst` (both [*, cols]).
    fn copy_row(&self, dst: &Tensor, dst_row: usize, src: &Tensor, src_row: usize, cols: usize) {
        self.copy_buf(src, (src_row * cols * 4) as u64, dst, (dst_row * cols * 4) as u64, (cols * 4) as u64);
    }
    /// RoPE applied per head (split-half within each head_dim slice) at position `pos0`.
    pub fn rope_mh_t(&self, x: &Tensor, ff: &Tensor, t: usize, dm: usize, base: f32, pos0: usize, head_dim: usize) -> Tensor {
        let out = self.alloc(t, dm);
        let neox = self.rope_neox.load(std::sync::atomic::Ordering::Relaxed) as u32;
        let msc = self.rope_mscale.load(std::sync::atomic::Ordering::Relaxed);
        let dims = [t as u32, dm as u32, base.to_bits(), pos0 as u32, head_dim as u32, neox, msc, 0];
        self.dispatch(ROPE_MH, &[x, ff], &out, bytemuck::cast_slice(&dims), wg1((t * dm / 2) as u32)); out
    }

    /// Qwen3/3.5 QK-Norm: per-head RMSNorm over `head_dim`, applied to a projected `[t, width]` tensor
    /// (width = heads·head_dim) BEFORE RoPE. `norm` is the [head_dim] weight; `None` → pass-through (llama/
    /// qwen2). Since head_dim is contiguous per head, the [t, width] buffer viewed as [t·heads, head_dim]
    /// rows makes this a plain rmsnorm_t — no new kernel — then we restore the [t, width] shape for RoPE.
    fn qk_norm(&self, x: Tensor, norm: &Option<Tensor>, t: usize, width: usize, head_dim: usize, eps: f32) -> Tensor {
        match norm {
            Some(w) => {
                let heads = width / head_dim;
                let mut n = self.rmsnorm_t(&x, w, t * heads, head_dim, eps);
                n.rows = t; n.cols = width; // restore logical shape; buffer order is identical
                n
            }
            None => x,
        }
    }
    /// Causal multi-head attention (prefill). `q` is [t, n_heads*hd]; `k`/`v` are
    /// [t, n_kv_heads*hd]. With n_kv_heads < n_heads this is grouped-query attention.
    pub fn attention_mh_t(&self, q: &Tensor, k: &Tensor, v: &Tensor, t: usize, dm: usize, n_heads: usize, n_kv_heads: usize) -> Tensor {
        let out = self.alloc(t, dm); let hd = dm / n_heads;
        let dims = [t as u32, hd as u32, n_heads as u32, n_kv_heads as u32];
        self.dispatch(ATTENTION_MH, &[q, k, v], &out, bytemuck::cast_slice(&dims), (((t * n_heads) as u32).div_ceil(64), 1, 1)); out
    }
    /// Prefill-extend attention: `s` segment queries (q is `[s, dm]`) at global positions
    /// `[base, base+s)` attend over the cache `kc`/`vc` already holding `base+s` rows,
    /// causal by global position. Output is `[s, dm]`.
    fn attention_mh_ext(&self, q: &Tensor, kc: &Tensor, vc: &Tensor, base: usize, s: usize, dm: usize, n_heads: usize, n_kv_heads: usize) -> Tensor {
        let out = self.alloc(s, dm); let hd = dm / n_heads;
        let dims = [s as u32, hd as u32, n_heads as u32, n_kv_heads as u32, base as u32, 0, 0, 0];
        self.dispatch(ATTENTION_MH_EXT, &[q, kc, vc], &out, bytemuck::cast_slice(&dims), (((s * n_heads) as u32).div_ceil(64), 1, 1)); out
    }
    /// Multi-head/GQA decode attention: single query row over a cache of `l` rows.
    fn decode_attention_mh(&self, q: &Tensor, kc: &Tensor, vc: &Tensor, l: usize, dm: usize, n_heads: usize, n_kv_heads: usize) -> Tensor {
        let out = self.alloc(1, dm); let hd = dm / n_heads;
        let dims = [l as u32, hd as u32, n_heads as u32, n_kv_heads as u32];
        // Rule-based by shape: the parallel kernel (one workgroup per head, KV rows split across
        // threads) uses the whole GPU and dominates at long context, but needs scores[l] in
        // shared memory (cap 4096). Above that: the flash split-K pair (parallelism grows with
        // context — heads × chunks workgroups — no shared-memory cap); LOGSCOPE_FLASH_ATTN=0
        // restores the old single-workgroup-per-head serial fallback.
        if l <= 4096 {
            self.dispatch(DECODE_ATTN_MH_PAR, &[q, kc, vc], &out, bytemuck::cast_slice(&dims), (n_heads as u32, 1, 1));
        } else if flash_attn_enabled() {
            return self.decode_attention_flash(q, kc, vc, l, dm, n_heads, n_kv_heads);
        } else {
            self.dispatch(DECODE_ATTN_MH, &[q, kc, vc], &out, bytemuck::cast_slice(&dims), ((n_heads as u32).div_ceil(64), 1, 1));
        }
        out
    }

    /// Flash-style split-K decode attention (see DECODE_ATTN_FLASH_P1/P2): pass 1 computes local
    /// softmax pieces per 2048-row KV chunk, pass 2 combines with the online-softmax rescale.
    /// The structural long-context decode kernel — per-token attention cost parallelizes across
    /// the sequence instead of one workgroup per head crawling all of KV.
    pub fn decode_attention_flash(&self, q: &Tensor, kc: &Tensor, vc: &Tensor, l: usize, dm: usize, n_heads: usize, n_kv_heads: usize) -> Tensor {
        let hd = dm / n_heads;
        let chunk = 2048usize;
        let nch = l.div_ceil(chunk);
        let part = self.alloc(1, n_heads * nch * (hd + 2));
        let dims = [l as u32, hd as u32, n_heads as u32, n_kv_heads as u32, chunk as u32, nch as u32, 0, 0];
        self.dispatch(DECODE_ATTN_FLASH_P1, &[q, kc, vc], &part, bytemuck::cast_slice(&dims), (n_heads as u32, nch as u32, 1));
        let out = self.alloc(1, dm);
        let dims2 = [hd as u32, nch as u32, n_heads as u32, 0];
        self.dispatch(DECODE_ATTN_FLASH_P2, &[&part], &out, bytemuck::cast_slice(&dims2), (n_heads as u32, 1, 1));
        out
    }

    /// Quantize `[n,c]` f32 rows to packed int8 `[n,c/4]` (u32) + per-row f32 scales.
    /// `c` must be a multiple of 4. Returns (packed, scales).
    pub fn quantize_rows(&self, x: &Tensor, n: usize, c: usize) -> (Tensor, Tensor) {
        let scales = self.alloc(n, 1);
        let dims = [n as u32, c as u32, 0u32, 0u32];
        self.dispatch(ROW_ABSMAX, &[x], &scales, bytemuck::cast_slice(&dims), ((n as u32).div_ceil(64), 1, 1));
        let q = self.alloc(n, c / 4); // u32 buffer (same byte size as [n, c/4] f32)
        self.dispatch(QUANT_PACK, &[x, &scales], &q, bytemuck::cast_slice(&dims), (((n * c / 4) as u32).div_ceil(64), 1, 1));
        (q, scales)
    }
    /// Inverse of `quantize_rows`: unpack int8 `[n,c/4]` + per-row scales to f32 `[n,c]`.
    pub fn dequant_rows(&self, q: &Tensor, scales: &Tensor, n: usize, c: usize) -> Tensor {
        let out = self.alloc(n, c); let dims = [n as u32, c as u32, 0u32, 0u32];
        self.dispatch(DEQUANT_Q8_ROWS, &[q, scales], &out, bytemuck::cast_slice(&dims), wg1((n * c) as u32)); out
    }
    /// Demote an f32 prefix cache (first `len` rows) to int8 — the warm tier, ~4x smaller.
    pub fn demote_cache(&self, c: &KvCache, len: usize, kv_dim: usize) -> QKvCache {
        let mut q = QKvCache { kq: vec![], ks: vec![], vq: vec![], vs: vec![] };
        for li in 0..c.k.len() {
            let (qk, sk) = self.quantize_rows(&c.k[li], len, kv_dim); q.kq.push(qk); q.ks.push(sk);
            let (qv, sv) = self.quantize_rows(&c.v[li], len, kv_dim); q.vq.push(qv); q.vs.push(sv);
        }
        q
    }
    /// Promote an int8 prefix cache (first `len` rows) back to f32 for seeding (lossy).
    pub fn promote_cache(&self, q: &QKvCache, len: usize, kv_dim: usize) -> KvCache {
        let mut c = KvCache { k: vec![], v: vec![] };
        for li in 0..q.kq.len() {
            c.k.push(self.dequant_rows(&q.kq[li], &q.ks[li], len, kv_dim));
            c.v.push(self.dequant_rows(&q.vq[li], &q.vs[li], len, kv_dim));
        }
        c
    }
    /// int8-KV decode attention: single query row over `l` packed int8 rows + scales.
    fn decode_attention_mh_q8(&self, q: &Tensor, kq: &Tensor, ks: &Tensor, vq: &Tensor, vs: &Tensor, l: usize, dm: usize, n_heads: usize, n_kv_heads: usize) -> Tensor {
        let out = self.alloc(1, dm); let hd = dm / n_heads;
        let dims = [l as u32, hd as u32, n_heads as u32, n_kv_heads as u32];
        self.dispatch(DECODE_ATTN_MH_Q8, &[q, kq, ks, vq, vs], &out, bytemuck::cast_slice(&dims), ((n_heads as u32).div_ceil(64), 1, 1)); out
    }
    pub fn new_qcache(&self, n_layers: usize, max_seq: usize, kv_dim: usize) -> QKvCache {
        QKvCache {
            kq: (0..n_layers).map(|_| self.alloc(max_seq, kv_dim / 4)).collect(),
            ks: (0..n_layers).map(|_| self.alloc(max_seq, 1)).collect(),
            vq: (0..n_layers).map(|_| self.alloc(max_seq, kv_dim / 4)).collect(),
            vs: (0..n_layers).map(|_| self.alloc(max_seq, 1)).collect(),
        }
    }
    /// One decode block using the int8 KV cache: compute q/k/v (f32), quantize this
    /// token's k/v row into the cache, attend the cache (dequant on the fly).
    fn block_decode_q8(&self, w: &BlockWeights, x: &Tensor, ff: &Tensor, kq: &Tensor, ks: &Tensor, vq: &Tensor, vs: &Tensor, pos: usize, d: usize, head_dim: usize, n_heads: usize, n_kv_heads: usize, base: f32, eps: f32) -> Tensor {
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd;
        let n1 = self.rmsnorm_t(x, &w.g1, 1, d, eps);
        let q = self.rope_mh_t(&self.qk_norm(w.wq.apply(self, &n1, 1), &w.q_norm, 1, ad, hd, eps), ff, 1, ad, base, pos, hd);
        let kr = self.rope_mh_t(&self.qk_norm(w.wk.apply(self, &n1, 1), &w.k_norm, 1, kv, hd, eps), ff, 1, kv, base, pos, hd);
        let vr = w.wv.apply(self, &n1, 1);
        let (kqr, ksr) = self.quantize_rows(&kr, 1, kv);
        let (vqr, vsr) = self.quantize_rows(&vr, 1, kv);
        self.copy_row(kq, pos, &kqr, 0, kv / 4); self.copy_row(ks, pos, &ksr, 0, 1);
        self.copy_row(vq, pos, &vqr, 0, kv / 4); self.copy_row(vs, pos, &vsr, 0, 1);
        let attn = self.decode_attention_mh_q8(&q, kq, ks, vq, vs, pos + 1, ad, n_heads, n_kv_heads);
        let x2 = w.wo.apply_add(self, &attn, 1, x);
        self.ffn_apply(w, &x2, 1, d, eps)
    }
    /// Fused-QKV counterpart of block_decode_q8 (int8 KV cache). Same slice-and-quantize: one wqkv
    /// matmul, q/k/v sliced from the packed output, then K and V quantized into the int8 cache exactly
    /// as before. Bit-exact to block_decode_q8 (the fusion never changes the values being quantized).
    fn block_decode_q8_fused(&self, w: &BlockWeights, x: &Tensor, ff: &Tensor, kq: &Tensor, ks: &Tensor, vq: &Tensor, vs: &Tensor, pos: usize, d: usize, head_dim: usize, n_heads: usize, n_kv_heads: usize, base: f32, eps: f32) -> Tensor {
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd;
        let n1 = self.rmsnorm_t(x, &w.g1, 1, d, eps);
        let qkv = w.wqkv.apply(self, &n1, 1);
        let (q, kr) = if let (Some(qnw), Some(knw)) = (&w.q_norm, &w.k_norm) {
            // Fused QK-Norm+RoPE (see block_decode_fused) — one dispatch for the old 4-chain.
            let qk = self.qknorm_rope_fused(&qkv, qnw, knw, ff, n_heads, n_kv_heads, hd, base, pos, eps);
            (self.slice_cols(&qk, 0, ad), self.slice_cols(&qk, ad, kv))
        } else if w.q_norm.is_some() || w.k_norm.is_some() {
            let qn = self.qk_norm(self.slice_cols(&qkv, 0, ad), &w.q_norm, 1, ad, hd, eps);
            let kn = self.qk_norm(self.slice_cols(&qkv, ad, kv), &w.k_norm, 1, kv, hd, eps);
            let qr = self.rope_mh_t(&qn, ff, 1, ad, base, pos, hd);
            let kr = self.rope_mh_t(&kn, ff, 1, kv, base, pos, hd);
            (qr, kr)
        } else {
            // RoPE q and k together (contiguous, head-aligned) — one dispatch, bit-exact to roping separately.
            let qk = self.rope_mh_t(&self.slice_cols(&qkv, 0, ad + kv), ff, 1, ad + kv, base, pos, hd);
            (self.slice_cols(&qk, 0, ad), self.slice_cols(&qk, ad, kv))
        };
        let vr = self.slice_cols(&qkv, ad + kv, kv);
        let (kqr, ksr) = self.quantize_rows(&kr, 1, kv);
        let (vqr, vsr) = self.quantize_rows(&vr, 1, kv);
        self.copy_row(kq, pos, &kqr, 0, kv / 4); self.copy_row(ks, pos, &ksr, 0, 1);
        self.copy_row(vq, pos, &vqr, 0, kv / 4); self.copy_row(vs, pos, &vsr, 0, 1);
        let attn = self.decode_attention_mh_q8(&q, kq, ks, vq, vs, pos + 1, ad, n_heads, n_kv_heads);
        let x2 = w.wo.apply_add(self, &attn, 1, x);
        self.ffn_apply(w, &x2, 1, d, eps)
    }
    /// One token through all layers using the int8 KV cache; returns logits [vocab].
    pub fn decode_step_q8(&self, m: &Model, c: &QKvCache, token: u32, pos: usize) -> Vec<f32> {
        self.begin_frame();
        let s = token as usize * m.d;
        let mut x = self.upload(&m.embed[s..s + m.d], 1, m.d);
        let fused = fused_qkv_enabled();
        for li in 0..m.n_layers {
            let l = &m.layers[li]; let (kq, ks, vq, vs) = (&c.kq[li], &c.ks[li], &c.vq[li], &c.vs[li]);
            x = if fused { self.block_decode_q8_fused(l, &x, &m.rope_factors, kq, ks, vq, vs, pos, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps) }
                else { self.block_decode_q8(l, &x, &m.rope_factors, kq, ks, vq, vs, pos, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps) };
        }
        let xn = self.rmsnorm_t(&x, &m.out_norm, 1, m.d, m.eps);
        self.download(&m.output.apply(self, &xn, 1))
    }
    /// Run `tokens` through a fresh int8 KV cache; returns last-position logits.
    pub fn forward_kv_q8(&self, m: &Model, tokens: &[u32]) -> Vec<f32> {
        let c = self.new_qcache(m.n_layers, tokens.len().max(1), m.kv_dim());
        let mut logits = Vec::new();
        for (pos, &t) in tokens.iter().enumerate() { logits = self.decode_step_q8(m, &c, t, pos); }
        logits
    }
    /// Fused-QKV counterpart of forward_kv_q8 (int8 KV cache); bit-exact to it. See --gate-qkv-e2e.
    pub fn forward_kv_q8_fused(&self, m: &Model, tokens: &[u32]) -> Vec<f32> {
        let c = self.new_qcache(m.n_layers, tokens.len().max(1), m.kv_dim());
        let mut logits = Vec::new();
        for (pos, &t) in tokens.iter().enumerate() {
            self.begin_frame();
            let s = t as usize * m.d;
            let mut x = self.upload(&m.embed[s..s + m.d], 1, m.d);
            for li in 0..m.n_layers { x = self.block_decode_q8_fused(&m.layers[li], &x, &m.rope_factors, &c.kq[li], &c.ks[li], &c.vq[li], &c.vs[li], pos, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps); }
            let xn = self.rmsnorm_t(&x, &m.out_norm, 1, m.d, m.eps);
            logits = self.download(&m.output.apply(self, &xn, 1));
        }
        logits
    }
    /// One block for a single token at absolute `pos`, attending to its KV cache.
    fn block_decode(&self, w: &BlockWeights, x: &Tensor, ff: &Tensor, kc: &Tensor, vc: &Tensor, pos: usize, d: usize, head_dim: usize, n_heads: usize, n_kv_heads: usize, base: f32, eps: f32) -> Tensor {
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd;
        let n1 = self.rmsnorm_t(x, &w.g1, 1, d, eps);
        let q = self.rope_mh_t(&self.qk_norm(w.wq.apply(self, &n1, 1), &w.q_norm, 1, ad, hd, eps), ff, 1, ad, base, pos, hd);
        let kr = self.rope_mh_t(&self.qk_norm(w.wk.apply(self, &n1, 1), &w.k_norm, 1, kv, hd, eps), ff, 1, kv, base, pos, hd);
        let vr = w.wv.apply(self, &n1, 1);
        self.write_row(kc, &kr, pos, kv);
        self.write_row(vc, &vr, pos, kv);
        let attn = self.decode_attention_mh(&q, kc, vc, pos + 1, ad, n_heads, n_kv_heads);
        let x2 = w.wo.apply_add(self, &attn, 1, x);
        self.ffn_apply(w, &x2, 1, d, eps)
    }
    /// Extract columns [col_off, col_off+width) of a single-row [1, N] tensor as a fresh [1, width],
    /// via a buffer copy (a transfer, NOT a compute dispatch). Splits the fused QKV projection's
    /// [1, d+2kv] output into q/k/v slices for the decode path. (t=1 only — the row is contiguous.)
    fn slice_cols(&self, src: &Tensor, col_off: usize, width: usize) -> Tensor {
        let out = self.alloc(1, width);
        self.copy_buf(src, (col_off * 4) as u64, &out, 0, (width * 4) as u64);
        out
    }
    /// block_decode with the three Q/K/V projections fused into one wqkv matmul (one weight-bind, one
    /// dispatch instead of three). The packed [1, d+2kv] output is split by buffer copies — q and k are
    /// rope'd together in ONE dispatch (they're contiguous and the q|k split lands on a head boundary,
    /// so it's bit-exact to roping them separately), v is copied straight into the V-cache row.
    /// Bit-exact to block_decode (--gate-qkv); saves 2 projection + 1 RoPE dispatch/layer.
    fn block_decode_fused(&self, w: &BlockWeights, x: &Tensor, ff: &Tensor, kc: &Tensor, vc: &Tensor, pos: usize, d: usize, head_dim: usize, n_heads: usize, n_kv_heads: usize, base: f32, eps: f32) -> Tensor {
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd;
        let n1 = self.rmsnorm_t(x, &w.g1, 1, d, eps);
        let qkv = w.wqkv.apply(self, &n1, 1);                          // [1, ad+2kv] — one matmul (was three)
        let (q, kr) = if let (Some(qnw), Some(knw)) = (&w.q_norm, &w.k_norm) {
            // Qwen3/3.5 QK-Norm: FUSED norm+rope — one dispatch replaces the old 4-chain
            // (norm q → norm k → rope q → rope k); output layout identical (q ‖ k).
            let qk = self.qknorm_rope_fused(&qkv, qnw, knw, ff, n_heads, n_kv_heads, hd, base, pos, eps);
            let qr = self.slice_cols(&qk, 0, ad);
            let kr = self.slice_cols(&qk, ad, kv);
            self.write_row(kc, &kr, pos, kv);
            (qr, kr)
        } else if w.q_norm.is_some() || w.k_norm.is_some() {
            // single-sided norm (no known arch does this) — keep the unfused chain for correctness
            let qn = self.qk_norm(self.slice_cols(&qkv, 0, ad), &w.q_norm, 1, ad, hd, eps);
            let kn = self.qk_norm(self.slice_cols(&qkv, ad, kv), &w.k_norm, 1, kv, hd, eps);
            let qr = self.rope_mh_t(&qn, ff, 1, ad, base, pos, hd);
            let kr = self.rope_mh_t(&kn, ff, 1, kv, base, pos, hd);
            self.write_row(kc, &kr, pos, kv);
            (qr, kr)
        } else {
            // RoPE q and k in one pass: [0,ad)=q heads, [ad,ad+kv)=k heads are contiguous and head-aligned.
            let qk = self.rope_mh_t(&self.slice_cols(&qkv, 0, ad + kv), ff, 1, ad + kv, base, pos, hd);
            let q = self.slice_cols(&qk, 0, ad);
            self.copy_buf(&qk, (ad * 4) as u64, kc, (pos * kv * 4) as u64, (kv * 4) as u64);   // roped k → K-cache row
            (q, self.slice_cols(&qk, ad, kv))
        };
        let _ = &kr; // k already written to cache above in both branches
        self.copy_buf(&qkv, ((ad + kv) * 4) as u64, vc, (pos * kv * 4) as u64, (kv * 4) as u64); // v → V-cache row
        let attn = self.decode_attention_mh(&q, kc, vc, pos + 1, ad, n_heads, n_kv_heads);
        let x2 = w.wo.apply_add(self, &attn, 1, x);
        self.ffn_apply(w, &x2, 1, d, eps)
    }
    /// Per-row-position RoPE: each row uses its own absolute position from `pos`.
    pub fn rope_mh_rows(&self, x: &Tensor, ff: &Tensor, pos: &Tensor, t: usize, dm: usize, base: f32, head_dim: usize) -> Tensor {
        let out = self.alloc(t, dm);
        let neox = self.rope_neox.load(std::sync::atomic::Ordering::Relaxed) as u32;
        let msc = self.rope_mscale.load(std::sync::atomic::Ordering::Relaxed);
        let dims = [t as u32, dm as u32, base.to_bits(), 0u32, head_dim as u32, neox, msc, 0];
        self.dispatch(ROPE_MH_ROWS, &[x, ff, pos], &out, bytemuck::cast_slice(&dims), wg1((t * dm / 2) as u32)); out
    }
    /// Process one token through all layers using the cache; returns logits [vocab].
    /// Decode one token writing K/V to an explicit physical `slot` and attending over the
    /// first `attend_len` cache rows, with RoPE applied at `rope_pos`. Because K is rope'd
    /// at its true position before storage, attention is order-independent across rows —
    /// so a ring buffer can overwrite an evicted slot with no shifting. `block_decode` is
    /// the special case `slot == rope_pos`, `attend_len == rope_pos+1`.
    fn block_decode_at(&self, w: &BlockWeights, x: &Tensor, ff: &Tensor, kc: &Tensor, vc: &Tensor, rope_pos: usize, slot: usize, attend_len: usize, d: usize, head_dim: usize, n_heads: usize, n_kv_heads: usize, base: f32, eps: f32) -> Tensor {
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd;
        let n1 = self.rmsnorm_t(x, &w.g1, 1, d, eps);
        let q = self.rope_mh_t(&self.qk_norm(w.wq.apply(self, &n1, 1), &w.q_norm, 1, ad, hd, eps), ff, 1, ad, base, rope_pos, hd);
        let kr = self.rope_mh_t(&self.qk_norm(w.wk.apply(self, &n1, 1), &w.k_norm, 1, kv, hd, eps), ff, 1, kv, base, rope_pos, hd);
        let vr = w.wv.apply(self, &n1, 1);
        self.write_row(kc, &kr, slot, kv);
        self.write_row(vc, &vr, slot, kv);
        let attn = self.decode_attention_mh(&q, kc, vc, attend_len, ad, n_heads, n_kv_heads);
        let x2 = w.wo.apply_add(self, &attn, 1, x);
        self.ffn_apply(w, &x2, 1, d, eps)
    }

    /// Fused-QKV counterpart of block_decode_at (streaming/ring f32 KV cache). Same as
    /// block_decode_fused but K/V are written at physical `slot` and attention spans `attend_len`.
    fn block_decode_at_fused(&self, w: &BlockWeights, x: &Tensor, ff: &Tensor, kc: &Tensor, vc: &Tensor, rope_pos: usize, slot: usize, attend_len: usize, d: usize, head_dim: usize, n_heads: usize, n_kv_heads: usize, base: f32, eps: f32) -> Tensor {
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd;
        let n1 = self.rmsnorm_t(x, &w.g1, 1, d, eps);
        let qkv = w.wqkv.apply(self, &n1, 1);
        let q = if let (Some(qnw), Some(knw)) = (&w.q_norm, &w.k_norm) {
            // Fused QK-Norm+RoPE (see block_decode_fused) — one dispatch for the old 4-chain.
            let qk = self.qknorm_rope_fused(&qkv, qnw, knw, ff, n_heads, n_kv_heads, hd, base, rope_pos, eps);
            let qr = self.slice_cols(&qk, 0, ad);
            let kr = self.slice_cols(&qk, ad, kv);
            self.write_row(kc, &kr, slot, kv);
            qr
        } else if w.q_norm.is_some() || w.k_norm.is_some() {
            let qn = self.qk_norm(self.slice_cols(&qkv, 0, ad), &w.q_norm, 1, ad, hd, eps);
            let kn = self.qk_norm(self.slice_cols(&qkv, ad, kv), &w.k_norm, 1, kv, hd, eps);
            let qr = self.rope_mh_t(&qn, ff, 1, ad, base, rope_pos, hd);
            let kr = self.rope_mh_t(&kn, ff, 1, kv, base, rope_pos, hd);
            self.write_row(kc, &kr, slot, kv);
            qr
        } else {
            // RoPE q and k together (contiguous, head-aligned) — one dispatch, bit-exact to roping separately.
            let qk = self.rope_mh_t(&self.slice_cols(&qkv, 0, ad + kv), ff, 1, ad + kv, base, rope_pos, hd);
            let q  = self.slice_cols(&qk, 0, ad);
            self.copy_buf(&qk, (ad * 4) as u64, kc, (slot * kv * 4) as u64, (kv * 4) as u64);   // roped k → slot
            q
        };
        self.copy_buf(&qkv, ((ad + kv) * 4) as u64, vc, (slot * kv * 4) as u64, (kv * 4) as u64);
        let attn = self.decode_attention_mh(&q, kc, vc, attend_len, ad, n_heads, n_kv_heads);
        let x2 = w.wo.apply_add(self, &attn, 1, x);
        self.ffn_apply(w, &x2, 1, d, eps)
    }
    /// Streaming-window decode: keep `sink` attention-sink tokens (the first few, pinned)
    /// plus a ring of the last `window` tokens, bounding the KV cache to `sink+window` rows
    /// no matter how long the stream runs. Identical to full decode until the cap is hit,
    /// then each new token overwrites the oldest window slot (O(1), see `stream_slot`).
    pub fn decode_step_stream(&self, m: &Model, c: &KvCache, token: u32, pos: usize, sink: usize, window: usize) -> Vec<f32> {
        self.begin_frame();
        let (slot, attend_len) = stream_slot(pos, sink, window);
        let s = token as usize * m.d;
        let mut x = self.upload(&m.embed[s..s + m.d], 1, m.d);
        let fused = fused_qkv_enabled();
        for li in 0..m.n_layers {
            let (l, k, v) = (&m.layers[li], &c.k[li], &c.v[li]);
            x = if fused { self.block_decode_at_fused(l, &x, &m.rope_factors, k, v, pos, slot, attend_len, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps) }
                else { self.block_decode_at(l, &x, &m.rope_factors, k, v, pos, slot, attend_len, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps) };
        }
        let xn = self.rmsnorm_t(&x, &m.out_norm, 1, m.d, m.eps);
        self.download(&m.output.apply(self, &xn, 1))
    }

    pub fn decode_step(&self, m: &Model, c: &KvCache, token: u32, pos: usize) -> Vec<f32> {
        self.begin_frame();
        let s = token as usize * m.d;
        let mut x = self.upload(&m.embed[s..s + m.d], 1, m.d);
        let fused = fused_qkv_enabled();
        for li in 0..m.n_layers {
            let (w, k, v) = (&m.layers[li], &c.k[li], &c.v[li]);
            x = if fused { self.block_decode_fused(w, &x, &m.rope_factors, k, v, pos, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps) }
                else { self.block_decode(w, &x, &m.rope_factors, k, v, pos, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps) };
        }
        let xn = self.rmsnorm_t(&x, &m.out_norm, 1, m.d, m.eps);
        self.download(&m.output.apply(self, &xn, 1))
    }

    /// Token-level generation: prefill `prefix`, then decode up to `max_new` tokens, sampling each
    /// with `sp`/`seed` and halting on any id in `stop`. Returns just the generated tokens (no prefix,
    /// no stop token). This is the unit a fabric `Worker` runs — tokens in, tokens out — and it takes
    /// `&self` with a local cache + rng so a single resident model can serve many concurrent prefixes.
    pub fn run_tokens(&self, m: &Model, prefix: &[u32], max_new: usize, sp: &SamplerParams, stop: &[u32], seed: u64) -> Vec<u32> {
        let ids: Vec<u32> = if prefix.is_empty() { vec![0] } else { prefix.to_vec() };
        let cap = (ids.len() + max_new + 1).max(1);
        let cache = self.new_cache(m.n_layers, cap, m.kv_dim());
        let mut logits = self.prefill_chunked(m, &cache, &ids, 512);
        let mut rng = seed;
        let mut pos = ids.len();
        let mut out = Vec::with_capacity(max_new);
        for _ in 0..max_new {
            let next = sample(&logits, sp, &mut rng);
            if stop.contains(&next) { break; }
            out.push(next);
            logits = self.decode_step(m, &cache, next, pos);
            pos += 1;
        }
        out
    }


    /// shared linears (q/k/v/o projections, FFN, output) run as `[N,*]` matmuls — each
    /// weight is read once for all N rows, which is the throughput win over N separate
    /// `decode_step`s. RoPE uses per-row positions; cache writes and attention are
    /// per-session (each attends its own cache at its own position). On return each
    /// session's `logits` hold its next-token distribution and `pos` is advanced.
    pub fn decode_batched(&self, m: &Model, sessions: &mut [Session], tokens: &[u32]) {
        let n = sessions.len();
        if n == 0 { return; }
        self.begin_frame();
        let d = m.d; let hd = m.head_dim; let kv = m.kv_dim(); let ad = m.n_heads * hd;
        let mut xh = Vec::with_capacity(n * d);
        for &t in tokens { let s = t as usize * d; xh.extend_from_slice(&m.embed[s..s + d]); }
        let mut x = self.upload(&xh, n, d);
        let posv: Vec<u32> = sessions.iter().map(|s| s.pos as u32).collect();
        let pos = self.upload_u32(&posv);
        for li in 0..m.n_layers {
            let w = &m.layers[li];
            let n1 = self.rmsnorm_t(&x, &w.g1, n, d, m.eps);
            let qb = self.rope_mh_rows(&w.wq.apply(self, &n1, n), &m.rope_factors, &pos, n, ad, m.base, hd);
            let kb = self.rope_mh_rows(&w.wk.apply(self, &n1, n), &m.rope_factors, &pos, n, kv, m.base, hd);
            let vb = w.wv.apply(self, &n1, n);
            // per-session: append this layer's K/V to each cache, attend each cache
            let attn = self.alloc(n, ad);
            for i in 0..n {
                let p = sessions[i].pos;
                self.copy_row(&sessions[i].cache.k[li], p, &kb, i, kv);
                self.copy_row(&sessions[i].cache.v[li], p, &vb, i, kv);
                let q1 = self.alloc(1, ad);
                self.copy_row(&q1, 0, &qb, i, ad);
                let a = self.decode_attention_mh(&q1, &sessions[i].cache.k[li], &sessions[i].cache.v[li], p + 1, ad, m.n_heads, m.n_kv_heads);
                self.copy_row(&attn, i, &a, 0, ad);
            }
            let o = w.wo.apply(self, &attn, n);
            let x2 = self.add_t(&x, &o);
            x = self.ffn_apply(w, &x2, n, d, m.eps);
        }
        let xn = self.rmsnorm_t(&x, &m.out_norm, n, d, m.eps);
        let logits = self.download(&m.output.apply(self, &xn, n));
        for i in 0..n {
            sessions[i].logits = logits[i * m.vocab..(i + 1) * m.vocab].to_vec();
            sessions[i].pos += 1;
        }
    }

    /// Prefill a shared prompt prefix once into its own cache, to be reused across many
    /// sessions (e.g. an identical system prompt + tool schemas). Returns a cache filled
    /// in rows 0..prefix.len().
    pub fn prefill_prefix(&self, m: &Model, prefix: &[u32]) -> KvCache {
        let c = self.new_cache(m.n_layers, prefix.len().max(1), m.kv_dim());
        if !prefix.is_empty() { self.prefill_batched(m, &c, prefix); }
        c
    }
    /// Seed a session from a precomputed prefix cache: copy the prefix K/V into the
    /// session's cache (one buffer copy per layer) and set its position. The session
    /// then prefills only its unique suffix, skipping recomputation of the shared
    /// prefix — a copy (bandwidth) instead of a full forward (compute) per session.
    pub fn seed_from_prefix(&self, m: &Model, session: &mut Session, prefix: &KvCache, prefix_len: usize) {
        let kv = m.kv_dim();
        for li in 0..m.n_layers {
            self.write_rows(&session.cache.k[li], &prefix.k[li], 0, prefix_len, kv);
            self.write_rows(&session.cache.v[li], &prefix.v[li], 0, prefix_len, kv);
        }
        session.pos = prefix_len;
    }

    /// Continuous-batching generation. Processes a queue of requests keeping up to
    /// `max_batch` sessions decoding together via `decode_batched`; finished sessions
    /// (eos or `max_new`) are evicted and pending ones backfill the freed slots without
    /// waiting for the rest of the batch. Optional `prefix` (a cache from
    /// `prefill_prefix` + its length) is reused across all sessions — each request's
    /// tokens are then treated as the suffix after the shared prefix.
    ///
    /// `queue` items are `(id, tokens, max_new)`; returns `(id, generated_tokens)` sorted
    /// by id. Greedy (temperature 0) output is identical to per-request `generate_kv`.
    pub fn run_batched(
        &self, m: &Model, prefix: Option<(&KvCache, usize)>,
        mut queue: Vec<(usize, Vec<u32>, usize)>, sampler: &SamplerParams, eos: Option<u32>,
        max_batch: usize, max_seq: usize,
    ) -> Vec<(usize, Vec<u32>)> {
        queue.reverse(); // pop() from the end == FIFO admission
        let mut sess: Vec<Session> = Vec::new();
        let mut ids: Vec<usize> = Vec::new();
        let mut maxn: Vec<usize> = Vec::new();
        let mut out: Vec<Vec<u32>> = Vec::new();
        let mut done: Vec<(usize, Vec<u32>)> = Vec::new();
        loop {
            // admit pending requests into free slots (prefill, reusing the prefix if set)
            while sess.len() < max_batch {
                match queue.pop() {
                    Some((id, prompt, mx)) => {
                        let mut s = self.session_new(m, max_seq, 0x1000 + id as u64);
                        if let Some((pc, plen)) = prefix { self.seed_from_prefix(m, &mut s, pc, plen); }
                        self.session_prefill(m, &mut s, &prompt);
                        sess.push(s); ids.push(id); maxn.push(mx); out.push(Vec::new());
                    }
                    None => break,
                }
            }
            if sess.is_empty() { break; }
            // sample one token per active session from its current logits
            let toks: Vec<u32> = sess.iter_mut().map(|s| {
                if sampler.temperature > 0.0 { sample(&s.logits, sampler, &mut s.rng) } else { argmax(&s.logits) }
            }).collect();
            // evict finished (eos / reached max_new); the rest carry their token to decode
            let mut ks = Vec::new(); let mut kid = Vec::new(); let mut kmx = Vec::new();
            let mut kout: Vec<Vec<u32>> = Vec::new(); let mut ktok = Vec::new();
            for ((((s, id), mx), mut o), t) in sess.drain(..).zip(ids.drain(..)).zip(maxn.drain(..)).zip(out.drain(..)).zip(toks) {
                if Some(t) == eos { done.push((id, o)); continue; }
                o.push(t);
                if o.len() >= mx { done.push((id, o)); }
                else { ks.push(s); kid.push(id); kmx.push(mx); kout.push(o); ktok.push(t); }
            }
            sess = ks; ids = kid; maxn = kmx; out = kout;
            if !sess.is_empty() { self.decode_batched(m, &mut sess, &ktok); }
        }
        done.sort_by_key(|r| r.0);
        done
    }

    /// Compile every pipeline the model's decode path uses, up front, by running one
    /// throwaway prefill+decode and discarding the result. After this the in-memory
    /// pipeline cache is hot, so a real worker's first token pays only compute — not
    /// shader compilation. This is the "the fleet is ready" signal: do it once after
    /// load, then every `Session` against the shared `Arc<Model>` starts warm.
    /// Returns the number of distinct pipelines now resident.
    pub fn warm(&self, m: &Model) -> usize {
        let mut s = self.session_new(m, 4, 0);
        self.session_prefill(m, &mut s, &[0u32]);
        let sp = SamplerParams { temperature: 0.0, top_k: 0, top_p: 1.0, repeat_penalty: 1.0, repeat_last_n: 0, min_p: 0.0 };
        let _ = self.session_step(m, &mut s, &sp, None);
        self.pipeline_count()
    }

    /// Run a fleet of workers grouped by shared prefix, propagating each prefix's KV
    /// cache to every worker that shares it instead of recomputing it per worker.
    ///
    /// Each `PrefixGroup` is a shared prefix (e.g. the file under repair, or a common
    /// system prompt) plus the workers whose prompts begin with it; a worker's `suffix`
    /// is only its unique tail. For each group the prefix is prefilled exactly once,
    /// then `run_batched` seeds it into every member — so ten workers on one file pay
    /// for that file's forward pass a single time and share the result.
    ///
    /// Groups are processed largest-first (most workers ⇒ most reuse), so under any
    /// throughput budget the highest-payoff cache propagation happens first. Output is
    /// `(id, generated_tokens)` over all workers, identical to running each worker
    /// independently (greedy).
    pub fn run_fleet(
        &self, m: &Model, mut groups: Vec<PrefixGroup>, sampler: &SamplerParams,
        eos: Option<u32>, max_batch: usize, max_seq: usize,
    ) -> Vec<(usize, Vec<u32>)> {
        // largest shared prefix first: propagate the most-reused cache before the rest.
        groups.sort_by(|a, b| b.workers.len().cmp(&a.workers.len()));
        let mut out: Vec<(usize, Vec<u32>)> = Vec::new();
        for g in groups {
            let cache = self.prefill_prefix(m, &g.prefix);
            let queue: Vec<(usize, Vec<u32>, usize)> = g.workers;
            let mut r = self.run_batched(
                m, Some((&cache, g.prefix.len())), queue, sampler, eos, max_batch, max_seq);
            out.append(&mut r);
        }
        out.sort_by_key(|r| r.0);
        out
    }

    /// Extend a prefix KV cache by a segment without recomputing the parent: copy the
    /// parent's `parent_len` rows into a fresh cache, then prefill `seg` in one batched
    /// pass at positions `[parent_len, parent_len+|seg|)` (each segment token attends over
    /// the full cache up to its position). Produces a cache covering `parent_len+|seg|`
    /// tokens — the primitive that lets the trie cache each shared segment exactly once.
    pub fn extend_prefix(&self, m: &Model, parent: &KvCache, parent_len: usize, seg: &[u32]) -> KvCache {
        let s = seg.len();
        let total = (parent_len + s).max(1);
        let c = self.new_cache(m.n_layers, total, m.kv_dim());
        let kv = m.kv_dim();
        for li in 0..m.n_layers {
            self.write_rows(&c.k[li], &parent.k[li], 0, parent_len, kv);
            self.write_rows(&c.v[li], &parent.v[li], 0, parent_len, kv);
        }
        if s == 0 { return c; }
        let mut xh = Vec::with_capacity(s * m.d);
        for &id in seg { let st = id as usize * m.d; xh.extend_from_slice(&m.embed[st..st + m.d]); }
        let mut x = self.upload(&xh, s, m.d);
        for li in 0..m.n_layers {
            x = self.block_prefill_ext(&m.layers[li], &x, &m.rope_factors, &c.k[li], &c.v[li],
                                       parent_len, s, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps);
        }
        c
    }

    /// Fleet generation with automatic nested-prefix sharing. Builds a radix trie over all
    /// workers' token sequences, caches every shared boundary once (each non-root segment
    /// extends its parent's cache rather than recomputing it), then runs each worker
    /// against the deepest cache it shares with another worker. A worker only ever prefills
    /// its own unique tail. `min_share` is the smallest subtree worker-count worth caching
    /// (2 = cache anything shared by ≥2 workers).
    ///
    /// Output is `(id, generated_tokens)` over all workers, identical to running each one
    /// independently (greedy) — validated bit-exact against per-worker `generate_kv`.
    pub fn run_fleet_trie(
        &self, m: &Model, requests: Vec<FleetRequest>, sampler: &SamplerParams,
        eos: Option<u32>, max_batch: usize, max_seq: usize, min_share: usize,
    ) -> Vec<(usize, Vec<u32>)> {
        use std::collections::{HashMap, HashSet, VecDeque};
        if requests.is_empty() { return Vec::new(); }
        // build the trie (root = node 0, empty segment)
        let mut nodes: Vec<TrieNode> = vec![TrieNode { seg: vec![], children: vec![], enders: vec![],
                                                       parent: usize::MAX, cum: 0, count: 0, rep: 0 }];
        for (w, r) in requests.iter().enumerate() { trie_insert(&mut nodes, &r.tokens, w); }
        // cumulative lengths, top-down
        let mut q = VecDeque::from([0usize]);
        while let Some(n) = q.pop_front() {
            let base = if nodes[n].parent == usize::MAX { 0 } else { nodes[nodes[n].parent].cum };
            nodes[n].cum = base + nodes[n].seg.len();
            for c in nodes[n].children.clone() { q.push_back(c); }
        }
        // subtree counts + a representative worker, bottom-up (reverse BFS)
        let bfs: Vec<usize> = {
            let mut o = vec![]; let mut q = VecDeque::from([0usize]);
            while let Some(n) = q.pop_front() { o.push(n); for c in nodes[n].children.clone() { q.push_back(c); } }
            o
        };
        for &n in bfs.iter().rev() {
            let mut cnt = nodes[n].enders.len();
            let mut rep = nodes[n].enders.first().copied();
            for c in nodes[n].children.clone() { cnt += nodes[c].count; if rep.is_none() { rep = Some(nodes[c].rep); } }
            nodes[n].count = cnt;
            nodes[n].rep = rep.unwrap_or(0);
        }
        // deepest shared cache node per worker: walk up from the ender's parent to the
        // first ancestor shared by ≥ min_share workers (cum>0). None ⇒ no reuse.
        let mut cache_node = vec![None; requests.len()];
        for n in 0..nodes.len() {
            for ei in 0..nodes[n].enders.len() {
                let w = nodes[n].enders[ei];
                let mut cur = nodes[n].parent;
                while cur != usize::MAX {
                    if nodes[cur].cum > 0 && nodes[cur].count >= min_share { cache_node[w] = Some(cur); break; }
                    cur = nodes[cur].parent;
                }
            }
        }
        // build caches for the used nodes, parents before children (cum ascending)
        let mut used: Vec<usize> = cache_node.iter().filter_map(|&x| x).collect();
        used.sort_unstable(); used.dedup();
        used.sort_by_key(|&n| nodes[n].cum);
        let used_set: HashSet<usize> = used.iter().copied().collect();
        let mut caches: HashMap<usize, KvCache> = HashMap::new();
        for &node in &used {
            let cum = nodes[node].cum;
            let toks = requests[nodes[node].rep].tokens.clone();
            // nearest already-built cache ancestor
            let mut anc = nodes[node].parent; let mut pa = None;
            while anc != usize::MAX { if used_set.contains(&anc) { pa = Some(anc); break; } anc = nodes[anc].parent; }
            let c = match pa {
                None => self.prefill_prefix(m, &toks[..cum]),
                Some(pa) => { let pcum = nodes[pa].cum; self.extend_prefix(m, &caches[&pa], pcum, &toks[pcum..cum]) }
            };
            caches.insert(node, c);
        }
        // group workers by their cache node (None = no shared prefix), run each group
        let mut groups: HashMap<Option<usize>, Vec<(usize, Vec<u32>, usize)>> = HashMap::new();
        for (w, r) in requests.iter().enumerate() {
            let suffix = match cache_node[w] { Some(node) => r.tokens[nodes[node].cum..].to_vec(), None => r.tokens.clone() };
            groups.entry(cache_node[w]).or_default().push((r.id, suffix, r.max_new));
        }
        // largest-reuse group first (most workers sharing a cache)
        let mut order: Vec<Option<usize>> = groups.keys().copied().collect();
        order.sort_by(|a, b| {
            let ca = a.map(|n| nodes[n].count).unwrap_or(0);
            let cb = b.map(|n| nodes[n].count).unwrap_or(0);
            cb.cmp(&ca)
        });
        let mut out: Vec<(usize, Vec<u32>)> = Vec::new();
        for cn in order {
            let queue = groups.remove(&cn).unwrap();
            let prefix = cn.map(|node| (caches.get(&node).unwrap(), nodes[node].cum));
            let mut r = self.run_batched(m, prefix, queue, sampler, eos, max_batch, max_seq);
            out.append(&mut r);
        }
        out.sort_by_key(|r| r.0);
        out
    }
    pub fn session_q8_new(&self, m: &Model, max_seq: usize, seed: u64) -> SessionQ8 {
        SessionQ8 { cache: self.new_qcache(m.n_layers, max_seq.max(1), m.kv_dim()),
                    pos: 0, logits: Vec::new(), generated: Vec::new(), rng: seed | 1, done: false }
    }
    pub fn session_q8_prefill(&self, m: &Model, s: &mut SessionQ8, prompt: &[u32]) {
        for &t in prompt { s.logits = self.decode_step_q8(m, &s.cache, t, s.pos); s.pos += 1; }
    }
    /// Prefill a shared prefix once into its own int8 cache, to be KV-forked across q8 sessions
    /// (the int8 analogue of `prefill_prefix`). Sequential decode_step_q8 — done once per prefix.
    pub fn prefill_prefix_q8(&self, m: &Model, prefix: &[u32]) -> QKvCache {
        let c = self.new_qcache(m.n_layers, prefix.len().max(1), m.kv_dim());
        for (pos, &t) in prefix.iter().enumerate() { let _ = self.decode_step_q8(m, &c, t, pos); }
        c
    }
    /// Copy a prefilled int8 prefix cache's first `prefix_len` rows into a q8 session (a buffer copy
    /// per layer, no recompute) and set its position. The int8 analogue of `seed_from_prefix`.
    pub fn seed_from_prefix_q8(&self, m: &Model, s: &mut SessionQ8, prefix: &QKvCache, prefix_len: usize) {
        let kv = m.kv_dim();
        for li in 0..m.n_layers {
            self.write_rows(&s.cache.kq[li], &prefix.kq[li], 0, prefix_len, kv / 4);
            self.write_rows(&s.cache.ks[li], &prefix.ks[li], 0, prefix_len, 1);
            self.write_rows(&s.cache.vq[li], &prefix.vq[li], 0, prefix_len, kv / 4);
            self.write_rows(&s.cache.vs[li], &prefix.vs[li], 0, prefix_len, 1);
        }
        s.pos = prefix_len;
    }
    /// Build a child int8 prefix cache = `parent`'s first `parent_len` rows + `seg` prefilled at
    /// offset `parent_len`. The int8 analogue of `extend_prefix` — the trie edge that lets the prefix
    /// trie run on int8-KV (combining the trie's shared-prefill saving with C's ~4× concurrency).
    /// Segment prefill is sequential `decode_step_q8` (one-time, off the hot path).
    pub fn extend_prefix_q8(&self, m: &Model, parent: &QKvCache, parent_len: usize, seg: &[u32]) -> QKvCache {
        let total = (parent_len + seg.len()).max(1);
        let c = self.new_qcache(m.n_layers, total, m.kv_dim());
        let kv = m.kv_dim();
        for li in 0..m.n_layers {
            self.write_rows(&c.kq[li], &parent.kq[li], 0, parent_len, kv / 4);
            self.write_rows(&c.ks[li], &parent.ks[li], 0, parent_len, 1);
            self.write_rows(&c.vq[li], &parent.vq[li], 0, parent_len, kv / 4);
            self.write_rows(&c.vs[li], &parent.vs[li], 0, parent_len, 1);
        }
        for (i, &t) in seg.iter().enumerate() { let _ = self.decode_step_q8(m, &c, t, parent_len + i); }
        c
    }
    /// Greedy decode with an int8 KV cache (reference for the batched int8 path).
    pub fn generate_kv_q8(&self, m: &Model, prompt: &[u32], n_new: usize) -> Vec<u32> {
        let c = self.new_qcache(m.n_layers, prompt.len() + n_new, m.kv_dim());
        let mut logits = Vec::new();
        for (pos, &t) in prompt.iter().enumerate() { logits = self.decode_step_q8(m, &c, t, pos); }
        let mut out = Vec::new();
        for step in 0..n_new {
            let next = argmax(&logits); out.push(next);
            logits = self.decode_step_q8(m, &c, next, prompt.len() + step);
        }
        out
    }
    /// Batched decode over int8-KV sessions: shared linears run `[N,*]`; this token's
    /// K/V is quantized once for the whole batch and scattered into each session's int8
    /// cache; attention dequantizes on the fly. Mirrors `decode_batched`.
    pub fn decode_batched_q8(&self, m: &Model, sessions: &mut [SessionQ8], tokens: &[u32]) {
        let n = sessions.len();
        if n == 0 { return; }
        let d = m.d; let hd = m.head_dim; let kv = m.kv_dim(); let ad = m.n_heads * hd;
        let mut xh = Vec::with_capacity(n * d);
        for &t in tokens { let s = t as usize * d; xh.extend_from_slice(&m.embed[s..s + d]); }
        let mut x = self.upload(&xh, n, d);
        let posv: Vec<u32> = sessions.iter().map(|s| s.pos as u32).collect();
        let pos = self.upload_u32(&posv);
        for li in 0..m.n_layers {
            let w = &m.layers[li];
            let n1 = self.rmsnorm_t(&x, &w.g1, n, d, m.eps);
            let qb = self.rope_mh_rows(&w.wq.apply(self, &n1, n), &m.rope_factors, &pos, n, ad, m.base, hd);
            let kb = self.rope_mh_rows(&w.wk.apply(self, &n1, n), &m.rope_factors, &pos, n, kv, m.base, hd);
            let vb = w.wv.apply(self, &n1, n);
            // quantize this layer's K/V for the whole batch once, then scatter per session
            let (kqb, ksb) = self.quantize_rows(&kb, n, kv);
            let (vqb, vsb) = self.quantize_rows(&vb, n, kv);
            let attn = self.alloc(n, ad);
            for i in 0..n {
                let p = sessions[i].pos; let c = &sessions[i].cache;
                self.copy_row(&c.kq[li], p, &kqb, i, kv / 4); self.copy_row(&c.ks[li], p, &ksb, i, 1);
                self.copy_row(&c.vq[li], p, &vqb, i, kv / 4); self.copy_row(&c.vs[li], p, &vsb, i, 1);
                let q1 = self.alloc(1, ad); self.copy_row(&q1, 0, &qb, i, ad);
                let a = self.decode_attention_mh_q8(&q1, &c.kq[li], &c.ks[li], &c.vq[li], &c.vs[li], p + 1, ad, m.n_heads, m.n_kv_heads);
                self.copy_row(&attn, i, &a, 0, ad);
            }
            let o = w.wo.apply(self, &attn, n);
            let x2 = self.add_t(&x, &o);
            x = self.ffn_apply(w, &x2, n, d, m.eps);
        }
        let xn = self.rmsnorm_t(&x, &m.out_norm, n, d, m.eps);
        let logits = self.download(&m.output.apply(self, &xn, n));
        for i in 0..n {
            sessions[i].logits = logits[i * m.vocab..(i + 1) * m.vocab].to_vec();
            sessions[i].pos += 1;
        }
    }
    /// Continuous-batching serving loop on int8-KV sessions. Mirrors `run_batched`
    /// (without the shared-prefix option). Greedy output matches `generate_kv_q8`.
    pub fn run_batched_q8(
        &self, m: &Model, mut queue: Vec<(usize, Vec<u32>, usize)>, sampler: &SamplerParams,
        eos: Option<u32>, max_batch: usize, max_seq: usize,
    ) -> Vec<(usize, Vec<u32>)> {
        queue.reverse();
        let mut sess: Vec<SessionQ8> = Vec::new();
        let mut ids: Vec<usize> = Vec::new();
        let mut maxn: Vec<usize> = Vec::new();
        let mut out: Vec<Vec<u32>> = Vec::new();
        let mut done: Vec<(usize, Vec<u32>)> = Vec::new();
        loop {
            while sess.len() < max_batch {
                match queue.pop() {
                    Some((id, prompt, mx)) => {
                        let mut s = self.session_q8_new(m, max_seq, 0x1000 + id as u64);
                        self.session_q8_prefill(m, &mut s, &prompt);
                        sess.push(s); ids.push(id); maxn.push(mx); out.push(Vec::new());
                    }
                    None => break,
                }
            }
            if sess.is_empty() { break; }
            let toks: Vec<u32> = sess.iter_mut().map(|s| {
                if sampler.temperature > 0.0 { sample(&s.logits, sampler, &mut s.rng) } else { argmax(&s.logits) }
            }).collect();
            let mut ks = Vec::new(); let mut kid = Vec::new(); let mut kmx = Vec::new();
            let mut kout: Vec<Vec<u32>> = Vec::new(); let mut ktok = Vec::new();
            for ((((s, id), mx), mut o), t) in sess.drain(..).zip(ids.drain(..)).zip(maxn.drain(..)).zip(out.drain(..)).zip(toks) {
                if Some(t) == eos { done.push((id, o)); continue; }
                o.push(t);
                if o.len() >= mx { done.push((id, o)); }
                else { ks.push(s); kid.push(id); kmx.push(mx); kout.push(o); ktok.push(t); }
            }
            sess = ks; ids = kid; maxn = kmx; out = kout;
            if !sess.is_empty() { self.decode_batched_q8(m, &mut sess, &ktok); }
        }
        done.sort_by_key(|r| r.0);
        done
    }
    pub fn generate(&self, m: &Model, prompt: &[u32], n_new: usize) -> Vec<u32> {
        let mut toks = prompt.to_vec(); let mut out = Vec::new();
        for _ in 0..n_new {
            let logits = self.model_forward(m, &toks);
            let next = argmax(&logits[logits.len() - m.vocab..]);
            out.push(next); toks.push(next);
        }
        out
    }
    /// Greedy decode with the KV cache: O(1) attention growth per step.
    pub fn generate_kv(&self, m: &Model, prompt: &[u32], n_new: usize) -> Vec<u32> {
        let c = self.new_cache(m.n_layers, prompt.len() + n_new, m.kv_dim());
        let mut logits = Vec::new();
        for pos in 0..prompt.len() { logits = self.decode_step(m, &c, prompt[pos], pos); }
        let mut out = Vec::new();
        for step in 0..n_new {
            let next = argmax(&logits); out.push(next);
            logits = self.decode_step(m, &c, next, prompt.len() + step);
        }
        out
    }
    /// Decode one token whose input ACTIVATION is already a GPU tensor `x` [1, d] (e.g. gathered on
    /// device): runs the layers + final norm + output projection and returns logits [1, vocab] as a
    /// tensor — no begin_frame, no readback (caller owns the frame). Honors LOGSCOPE_FUSED_QKV.
    fn decode_layers_t(&self, m: &Model, c: &KvCache, mut x: Tensor, pos: usize) -> Tensor {
        let fused = fused_qkv_enabled();
        for li in 0..m.n_layers {
            let (w, k, v) = (&m.layers[li], &c.k[li], &c.v[li]);
            x = if fused { self.block_decode_fused(w, &x, &m.rope_factors, k, v, pos, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps) }
                else { self.block_decode(w, &x, &m.rope_factors, k, v, pos, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps) };
        }
        let xn = self.rmsnorm_t(&x, &m.out_norm, 1, m.d, m.eps);
        m.output.apply(self, &xn, 1)
    }
    /// Greedy generation that keeps the token id and input embedding ON THE GPU across steps: argmax
    /// and embedding gather are kernels, so each step reads back only the 4-byte id instead of [vocab]
    /// logits, and the next input is gathered on-device. Bit-exact to generate_kv (--gate-gpusample);
    /// the prerequisite for windowed (K-steps-per-submit) decode.
    pub fn generate_kv_gpu(&self, m: &Model, prompt: &[u32], n_new: usize) -> Vec<u32> {
        let embed_t = self.upload(&m.embed, m.vocab, m.d);
        let c = self.new_cache(m.n_layers, prompt.len() + n_new, m.kv_dim());
        // Prefill all but the last prompt token via the normal (downloaded) path; keep the LAST prompt
        // token's logits on the GPU to seed the on-device loop.
        for pos in 0..prompt.len().saturating_sub(1) { let _ = self.decode_step(m, &c, prompt[pos], pos); }
        let last = prompt.len() - 1;
        self.begin_frame();
        let x0 = self.upload(&m.embed[prompt[last] as usize * m.d..prompt[last] as usize * m.d + m.d], 1, m.d);
        let mut logits_t = self.decode_layers_t(m, &c, x0, last);
        let mut out = Vec::with_capacity(n_new);
        for step in 0..n_new {
            let id_t = self.argmax_t(&logits_t);             // on-GPU argmax, recorded into the open frame
            let id = self.download_u32(&id_t)[0];            // flush + 4-byte readback (not [vocab])
            out.push(id);
            if step + 1 == n_new { break; }
            self.begin_frame();
            let x = self.gather_t(&embed_t, &id_t, 1, m.d);  // next input gathered on-device from id_t
            logits_t = self.decode_layers_t(m, &c, x, prompt.len() + step);
        }
        out
    }
    /// Locked-pool WINDOWED decode: record `k` decode steps into ONE submit, then read back the whole
    /// window's `k` token ids at once. Within the window each step's gather reads the id-buffer slot the
    /// previous step's argmax wrote (the in-frame write→read dependency), so the id→embedding→logits→id
    /// cycle never returns to the CPU until the window closes. Bit-exact to generate_kv (--gate-window);
    /// the maximal "don't sync per token" decode (pin LOGSCOPE_FLUSH_EVERY high to keep a window in one
    /// submit). EOS can't halt mid-window — generate to k then trim (no over-gen here: fixed n_new).
    pub fn generate_kv_window(&self, m: &Model, prompt: &[u32], n_new: usize, k: usize) -> Vec<u32> {
        let k = k.max(1);
        let embed_t = self.upload(&m.embed, m.vocab, m.d);
        let c = self.new_cache(m.n_layers, prompt.len() + n_new, m.kv_dim());
        let ids_buf = self.alloc(1, k);                       // [1, k] u32 — one slot per windowed step
        let l = prompt.len();
        for pos in 0..l.saturating_sub(1) { let _ = self.decode_step(m, &c, prompt[pos], pos); }
        self.begin_frame();
        let x0 = self.upload(&m.embed[prompt[l - 1] as usize * m.d..prompt[l - 1] as usize * m.d + m.d], 1, m.d);
        let mut logits_t = self.decode_layers_t(m, &c, x0, l - 1);
        let mut out = Vec::with_capacity(n_new);
        let mut step = 0;
        while step < n_new {
            let w = (n_new - step).min(k);
            for j in 0..w {
                self.argmax_at(&logits_t, &ids_buf, j as u32);            // token (step+j) → ids_buf[j]
                if step + j + 1 < n_new {                                  // skip the final unneeded decode
                    let x = self.gather_at(&embed_t, &ids_buf, j as u32, m.d);
                    logits_t = self.decode_layers_t(m, &c, x, l + step + j);
                }
            }
            let win = self.download_u32(&ids_buf);                         // flush window; one readback for w tokens
            out.extend_from_slice(&win[..w]);
            step += w;
            if step < n_new { self.begin_frame(); }
        }
        out
    }

    /// SAMPLED twin of `generate_kv_window`: k tokens per readback with the FULL sampler running
    /// GPU-side (`sample_at`). This removes, per token: the full-vocab logits readback (a hard GPU
    /// sync + ~600 KB copy on a 151k vocab), and the CPU's O(vocab·log vocab) descending sort in
    /// `sample()`. The rng stays on the CPU — each window uploads its k xorshift01 draws — so a
    /// given seed consumes the identical uniform stream `generate_kv_stream` would; `--gate-sample`
    /// checks agreement per step and end-to-end. EOS can't halt mid-window (fixed n_new here; the
    /// engine's streaming adopter trims at the stop and rewinds `pos`, which is safe because
    /// attention length is pos-derived and stale rows get overwritten).
    pub fn generate_kv_window_sampled(&self, m: &Model, prompt: &[u32], n_new: usize, k: usize,
                                      sp: &SamplerParams, seed: u64) -> Vec<u32> {
        let k = k.max(1);
        let rcap: usize = 64; // ring capacity == the kernel's override-table bound (rlast ≤ 64)
        let embed_t = self.upload(&m.embed, m.vocab, m.d);
        let c = self.new_cache(m.n_layers, prompt.len() + n_new, m.kv_dim());
        // o = [k deposit slots | rcap ring]; seed the ring with the prompt tail (what `seen` ends with)
        let mut o_init = vec![0u32; k + rcap];
        let tail = prompt.len().min(rcap);
        for (j, &tkn) in prompt[prompt.len() - tail..].iter().enumerate() { o_init[k + j] = tkn; }
        let o = self.upload_u32(&o_init);
        let seeded = tail as u32;
        let l = prompt.len();
        for pos in 0..l.saturating_sub(1) { let _ = self.decode_step(m, &c, prompt[pos], pos); }
        let mut rng = seed | 1;
        self.begin_frame();
        let x0 = self.upload(&m.embed[prompt[l - 1] as usize * m.d..prompt[l - 1] as usize * m.d + m.d], 1, m.d);
        let mut logits_t = self.decode_layers_t(m, &c, x0, l - 1);
        let mut out = Vec::with_capacity(n_new);
        let mut step = 0usize;
        while step < n_new {
            let w = (n_new - step).min(k);
            let us: Vec<f32> = (0..w).map(|_| xorshift01(&mut rng)).collect();
            let uni = self.upload(&us, 1, w);
            for j in 0..w {
                self.sample_at(&logits_t, &o, &uni, sp, j as u32, seeded, rcap as u32, k as u32, (step + j) as u32);
                if step + j + 1 < n_new {
                    let x = self.gather_at(&embed_t, &o, j as u32, m.d);
                    logits_t = self.decode_layers_t(m, &c, x, l + step + j);
                }
            }
            let win = self.download_u32(&o);
            out.extend_from_slice(&win[..w]);
            step += w;
            if step < n_new { self.begin_frame(); }
        }
        out
    }

    /// Forward `tokens` as a contiguous block at absolute positions `start..start+len`, appending
    /// their K/V into the existing cache `c` and returning logits for EVERY position ([len,vocab]).
    /// This is the speculative-decode verify primitive: the target consumes a whole drafted block
    /// in one pass (causal among the new tokens, attending the full prefix) instead of one
    /// decode_step per token. Built on block_prefill_ext (the offset-append forward).
    pub fn verify_forward(&self, m: &Model, c: &KvCache, tokens: &[u32], start: usize) -> Vec<f32> {
        let s = tokens.len();
        if s == 0 { return Vec::new(); }
        let mut xh = Vec::with_capacity(s * m.d);
        for &id in tokens { let st = id as usize * m.d; xh.extend_from_slice(&m.embed[st..st + m.d]); }
        let mut x = self.upload(&xh, s, m.d);
        for li in 0..m.n_layers {
            x = self.block_prefill_ext(&m.layers[li], &x, &m.rope_factors, &c.k[li], &c.v[li],
                                       start, s, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps);
        }
        let xn = self.rmsnorm_t(&x, &m.out_norm, s, m.d, m.eps);
        self.download(&m.output.apply(self, &xn, s)) // all `s` rows projected
    }

    /// Greedy speculative decode. `draft` proposes `k` tokens; `target` verifies them in ONE
    /// forward; the longest matching prefix is accepted, the target corrects at the first
    /// divergence (plus one free bonus token when all `k` are accepted). The output is
    /// bit-identical to the target's own greedy `generate_kv` — the draft only changes speed,
    /// never the result. `draft` and `target` must share a vocabulary. Returns the `n_new`
    /// generated tokens (same convention as `generate_kv`).
    pub fn speculative_generate(&self, target: &Model, draft: &Model, prompt: &[u32], n_new: usize, k: usize) -> Vec<u32> {
        let mut st = SpecStats::default();
        self.speculative_generate_stats(target, draft, prompt, n_new, k, &mut st)
    }

    /// As `speculative_generate`, recording per-run stats (rounds, accepted drafts, emitted) so a
    /// bench can report acceptance rate and effective tokens-per-target-forward.
    pub fn speculative_generate_stats(&self, target: &Model, draft: &Model, prompt: &[u32], n_new: usize, k: usize, st: &mut SpecStats) -> Vec<u32> {
        let k = k.max(1);
        let max_seq = prompt.len() + n_new + 2 * k + 8;
        let tc = self.new_cache(target.n_layers, max_seq, target.kv_dim());
        // prefill the target like generate_kv (per-position) so the running logits match bit-for-bit.
        let mut prev = Vec::new();
        for pos in 0..prompt.len() { prev = self.decode_step(target, &tc, prompt[pos], pos); }
        let mut pos = prompt.len();
        // PERSISTENT draft session: prefilled once, then resynced incrementally each round.
        let mut ds = self.session_new(draft, max_seq, 7);
        self.session_prefill(draft, &mut ds, prompt);
        let greedy = SamplerParams { temperature: 0.0, top_k: 0, top_p: 1.0, repeat_penalty: 1.0, repeat_last_n: 0, min_p: 0.0 };
        let mut gen: Vec<u32> = Vec::new();
        while gen.len() < n_new {
            let committed = prompt.len() + gen.len(); // == pos == ds.pos at round start
            // draft k tokens from the persistent draft (advances ds.pos by k)
            let mut draft_toks = Vec::with_capacity(k);
            for _ in 0..k {
                match self.session_step(draft, &mut ds, &greedy, None) { Some(t) => draft_toks.push(t), None => break }
            }
            st.rounds += 1;
            if draft_toks.is_empty() { // draft hit eos: plain target step
                let t = argmax(&prev); gen.push(t); st.emitted += 1;
                prev = self.decode_step(target, &tc, t, pos); pos += 1;
                ds.pos = committed; self.session_feed(draft, &mut ds, t);
                continue;
            }
            // verify the whole drafted block in one target forward
            let vlog = self.verify_forward(target, &tc, &draft_toks, pos);
            let kk = draft_toks.len();
            let mut cur = prev.clone();
            let mut accepted = 0usize; let mut emit: Vec<u32> = Vec::new(); let mut mism = false;
            for j in 0..kk {
                let tt = argmax(&cur); emit.push(tt);
                if draft_toks[j] == tt { accepted += 1; cur = vlog[j * target.vocab..(j + 1) * target.vocab].to_vec(); }
                else { mism = true; break; }
            }
            if !mism { emit.push(argmax(&cur)); } // all kk accepted → free bonus token
            st.accepted += accepted;
            // commit: accepted drafts' K/V already in the target cache; forward the correction to
            // write its K/V + next logits (rejected positions beyond pos+accepted are ignored).
            let correction = *emit.last().unwrap();
            prev = self.decode_step(target, &tc, correction, pos + accepted);
            pos += accepted + 1;
            // resync the persistent draft: roll back to the accepted length, feed the correction.
            ds.pos = committed + accepted;
            self.session_feed(draft, &mut ds, correction);
            for t in emit { if gen.len() >= n_new { break; } gen.push(t); st.emitted += 1; }
        }
        gen.truncate(n_new);
        gen
    }

    /// Run `tokens` through a fresh KV cache and return the last position's logits
    /// (for cross-checking the cached path against the full forward, numerically).
    pub fn forward_kv(&self, m: &Model, tokens: &[u32]) -> Vec<f32> {
        let c = self.new_cache(m.n_layers, tokens.len(), m.kv_dim());
        let mut logits = Vec::new();
        for (pos, &t) in tokens.iter().enumerate() { logits = self.decode_step(m, &c, t, pos); }
        logits
    }
    /// Fused-QKV counterpart of forward_kv (always uses block_decode_fused, independent of the env
    /// flag). Bit-exact to forward_kv — see --gate-qkv-e2e. The dispatch saving is the tok/s lever.
    pub fn forward_kv_fused(&self, m: &Model, tokens: &[u32]) -> Vec<f32> {
        let c = self.new_cache(m.n_layers, tokens.len(), m.kv_dim());
        let mut logits = Vec::new();
        for (pos, &t) in tokens.iter().enumerate() {
            self.begin_frame();
            let s = t as usize * m.d;
            let mut x = self.upload(&m.embed[s..s + m.d], 1, m.d);
            for li in 0..m.n_layers { x = self.block_decode_fused(&m.layers[li], &x, &m.rope_factors, &c.k[li], &c.v[li], pos, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps); }
            let xn = self.rmsnorm_t(&x, &m.out_norm, 1, m.d, m.eps);
            logits = self.download(&m.output.apply(self, &xn, 1));
        }
        logits
    }
    /// Streaming KV-cache decode: prefill `prompt`, then emit up to `n_new` tokens
    /// one at a time through `on_token` (O(1) work per token, not a recompute).
    /// Greedy when `sampler.temperature <= 0`, else temperature/top-k/top-p. Stops
    /// early when `eos` is produced or `on_token` returns false (e.g. cancellation).
    /// Returns the number of tokens emitted.
    pub fn generate_kv_stream(
        &self, m: &Model, prompt: &[u32], n_new: usize, eos: Option<u32>,
        sampler: &SamplerParams, rng: &mut u64, cancel: &dyn Fn() -> bool,
        mut on_token: impl FnMut(u32) -> bool,
    ) -> usize {
        let cap = (prompt.len() + n_new).max(1);
        let c = self.new_cache(m.n_layers, cap, m.kv_dim());
        let mut logits;
        let mut pos = prompt.len();
        // Batched prefill: one [T,d] pass that fills the cache, vs T sequential steps.
        if cancel() { return 0; }
        logits = self.prefill_batched(m, &c, prompt);
        let mut seen: Vec<u32> = prompt.to_vec();
        let mut produced = 0usize;
        for _ in 0..n_new {
            if logits.is_empty() || cancel() { break; }
            let next = pick(&logits, sampler, &seen, rng);
            if Some(next) == eos { break; }
            produced += 1;
            seen.push(next);
            let cont = on_token(next);
            if !cont { break; }
            if pos >= cap { break; }
            logits = self.decode_step(m, &c, next, pos);
            pos += 1;
        }
        produced
    }

    // ── multi-session (concurrency) primitives ───────────────────────────────
    /// Allocate a session against a resident model. Only the KV cache is per-session;
    /// the weights stay shared.
    pub fn session_new(&self, m: &Model, max_seq: usize, seed: u64) -> Session {
        Session { cache: self.new_cache(m.n_layers, max_seq.max(1), m.kv_dim()),
                  pos: 0, logits: Vec::new(), generated: Vec::new(), rng: seed | 1, done: false }
    }
    /// Prefill a session's prompt into its cache (logits left at the last token).
    pub fn session_prefill(&self, m: &Model, s: &mut Session, prompt: &[u32]) {
        for &t in prompt { s.logits = self.decode_step(m, &s.cache, t, s.pos); s.pos += 1; }
    }
    /// Advance a session by exactly one token. Returns the token, or None if the
    /// session is finished (eos / no logits / cache full). This is the unit a
    /// scheduler interleaves across concurrent sessions.
    pub fn session_step(&self, m: &Model, s: &mut Session, sampler: &SamplerParams, eos: Option<u32>) -> Option<u32> {
        if s.done || s.logits.is_empty() { return None; }
        let next = pick(&s.logits, sampler, &s.generated, &mut s.rng);
        if Some(next) == eos { s.done = true; return None; }
        s.generated.push(next);
        s.logits = self.decode_step(m, &s.cache, next, s.pos);
        s.pos += 1;
        Some(next)
    }

    /// Advance a session by a SPECIFIC token (vs `session_step`, which samples one). Used by
    /// speculative decode to resync the persistent draft cache to the committed sequence: roll
    /// `s.pos` back to the accepted length, then feed the correction token. Positions beyond the
    /// new `pos` hold stale rejected drafts and are ignored by future attention.
    pub fn session_feed(&self, m: &Model, s: &mut Session, token: u32) {
        s.generated.push(token);
        s.logits = self.decode_step(m, &s.cache, token, s.pos);
        s.pos += 1;
    }

    /// Compile every pipeline the prefill AND decode paths use, before the first real
    /// token. Decode uses kernels (the `DECODE_ATTN` variants) that prefill never touches,
    /// so a 2-token prefill plus one decode_step on a throwaway cache warms them all. This
    /// moves one-time WGSL shader compilation into the model-load window instead of the
    /// user's first token (the slow 0.1 tok/s start). Returns the cached-pipeline count —
    /// the "warm" signal a scheduler can read to know an instance is ready to serve.
    pub fn prewarm(&self, m: &Model) -> usize {
        let c = self.new_cache(m.n_layers, 4, m.kv_dim());
        let _ = self.prefill_batched(m, &c, &[0u32, 0]);
        let _ = self.decode_step(m, &c, 0, 2);
        self.pipeline_count()
    }

    /// Stream up to `n_new` tokens from an already-prefilled session, calling `on_token`
    /// (false to stop). Unlike `generate_kv_stream` this works on a caller-owned session
    /// whose cache may have been seeded from a shared prefix (anchor) via `seed_from_prefix`
    /// — so the shared prefix is never recomputed. `seen` is the running context for the
    /// repeat penalty (seed it with the full prompt); generated tokens are appended to it.
    pub fn stream_session(
        &self, m: &Model, s: &mut Session, seen: &mut Vec<u32>, n_new: usize, eos: Option<u32>,
        sampler: &SamplerParams, rng: &mut u64, cancel: &dyn Fn() -> bool,
        mut on_token: impl FnMut(u32) -> bool,
    ) -> usize {
        let mut produced = 0usize;
        for _ in 0..n_new {
            if s.logits.is_empty() || cancel() { break; }
            let next = pick(&s.logits, sampler, seen, rng);
            if Some(next) == eos { break; }
            produced += 1;
            seen.push(next);
            s.generated.push(next);
            if !on_token(next) { break; }
            s.logits = self.decode_step(m, &s.cache, next, s.pos);
            s.pos += 1;
        }
        produced
    }

    /// WINDOWED `stream_session`: k sampled tokens per GPU round-trip. The whole sample→embed→
    /// decode cycle runs on-device (`sample_at` + `gather_at`/`gather_q8_at` + `decode_layers_t`);
    /// the CPU reads back k token ids per window instead of a full-vocab logits row per token, and
    /// skips the per-token O(v·log v) sort in `sample()`. Semantics mirror `stream_session`
    /// EXACTLY, replayed from the window readback: eos halts uncounted with `s.logits` left at the
    /// distribution that produced it; an `on_token` false halts with the token counted and
    /// `s.logits`/`s.pos` in the same state the CPU path leaves (its decode never ran); the rng
    /// advances by exactly the draws the CPU path would have made (uniforms are drawn from a
    /// snapshot and committed post-hoc), so a mid-window stop keeps the streams aligned for any
    /// continuation. Stale KV rows past a rewound `s.pos` are harmless: attention length is
    /// pos-derived and the rows get overwritten. Cancellation is per-window (k-token granularity).
    /// `--gate` coverage: exact-match vs `stream_session` (EmbedT::F32) incl. mid-window stops.
    pub fn stream_session_window(
        &self, m: &Model, s: &mut Session, seen: &mut Vec<u32>, n_new: usize, eos: Option<u32>,
        sampler: &SamplerParams, rng: &mut u64, k: usize, embed: &EmbedT<'_>,
        cancel: &dyn Fn() -> bool, mut on_token: impl FnMut(u32) -> bool,
    ) -> usize {
        let k = k.max(2);
        let rcap = 64usize;
        if s.logits.is_empty() { return 0; }
        let cache_rows = s.cache.k.first().map(|t| t.rows).unwrap_or(0);
        let mut logits_t = Some(self.upload(&s.logits, 1, s.logits.len()));
        let mut produced = 0usize;
        let mut halted = false;
        while produced < n_new {
            if cancel() { break; }
            let w = (n_new - produced).min(k).min(cache_rows.saturating_sub(s.pos));
            if w == 0 { break; }
            // Ring ← the CPU-truth trailing context, re-seeded each window (rewinds stay trivial).
            let mut o_init = vec![0u32; k + rcap];
            let tail = seen.len().min(rcap);
            for (j, &tk) in seen[seen.len() - tail..].iter().enumerate() { o_init[k + j] = tk; }
            let o = self.upload_u32(&o_init);
            // Uniforms from a SNAPSHOT of the rng; the real rng is committed after replay by the
            // number of draws the CPU path would actually have consumed.
            let rng0 = *rng;
            let mut rtmp = rng0;
            let us: Vec<f32> = (0..w).map(|_| xorshift01(&mut rtmp)).collect();
            let uni = self.upload(&us, 1, w);
            self.begin_frame();
            // pre[j] = the distribution BEFORE slot j; pre[w] = the decode after the last slot.
            let mut pre: Vec<Tensor> = Vec::with_capacity(w + 1);
            pre.push(logits_t.take().unwrap());
            for j in 0..w {
                self.sample_at(&pre[j], &o, &uni, sampler, j as u32, tail as u32, rcap as u32, k as u32, j as u32);
                let x = match embed {
                    EmbedT::F32(t) => self.gather_at(t, &o, j as u32, m.d),
                    EmbedT::Q8(q) => self.gather_q8_at(q, &o, j as u32),
                };
                pre.push(self.decode_layers_t(m, &s.cache, x, s.pos + j));
            }
            let win = self.download_u32(&o);
            // Replay with exact stream_session semantics.
            let mut draws = w;
            for j in 0..w {
                let next = win[j];
                if Some(next) == eos {
                    draws = j + 1;
                    s.logits = self.download(&pre[j]);
                    s.pos += j;
                    halted = true;
                    break;
                }
                produced += 1; seen.push(next); s.generated.push(next);
                if !on_token(next) {
                    draws = j + 1;
                    s.logits = self.download(&pre[j]);
                    s.pos += j;
                    halted = true;
                    break;
                }
            }
            let mut rc = rng0;
            for _ in 0..draws { let _ = xorshift01(&mut rc); }
            *rng = rc;
            if halted { break; }
            s.pos += w;
            logits_t = pre.pop(); // pre[w]
        }
        if !halted {
            if let Some(t) = logits_t { s.logits = self.download(&t); }
        }
        produced
    }
}

/// Embedding-table handle for windowed decode: `F32` is the bit-exact tier (test/gate path, or a
/// model small enough to keep an f32 copy), `Q8` is the int8-resident tier a big model actually
/// uses (~4× smaller; same tolerance class as its int8-resident weights).
pub enum EmbedT<'a> { F32(&'a Tensor), Q8(&'a QWeight) }

/// Sampling parameters. Greedy when `temperature <= 0` or `top_k == 1`. `repeat_penalty`
/// (1.0 = off) divides the logit of recently-seen tokens to curb the repetition/looping a
/// small model falls into; `repeat_last_n` is how many recent tokens it applies over.
/// `min_p` (0.0 = off) keeps only tokens with prob >= min_p * top_prob — a flatter, more
/// robust nucleus than top_p for small models, especially at low temperature.
#[derive(Clone, Copy)]
pub struct SamplerParams { pub temperature: f32, pub top_k: usize, pub top_p: f32, pub repeat_penalty: f32, pub repeat_last_n: usize, pub min_p: f32 }
impl Default for SamplerParams { fn default() -> Self { Self { temperature: 0.8, top_k: 40, top_p: 0.95, repeat_penalty: 1.1, repeat_last_n: 64, min_p: 0.05 } } }

/// Apply a repetition penalty to `logits` for each token in `recent` (the standard
/// llama.cpp form: positive logits are divided, negative multiplied, by `penalty`).
pub fn apply_repeat_penalty(logits: &mut [f32], recent: &[u32], penalty: f32) {
    if penalty == 1.0 { return; }
    for &t in recent {
        let i = t as usize;
        if i < logits.len() {
            if logits[i] > 0.0 { logits[i] /= penalty; } else { logits[i] *= penalty; }
        }
    }
}

/// Choose the next token: apply the repetition penalty over the recent window (if enabled),
/// then sample (temperature/top-k/top-p) or argmax. With `repeat_penalty == 1.0` this is
/// exactly `sample`/`argmax`, so greedy paths stay bit-exact.
pub fn pick(logits: &[f32], sampler: &SamplerParams, recent: &[u32], rng: &mut u64) -> u32 {
    if sampler.repeat_penalty != 1.0 && !recent.is_empty() {
        let mut l = logits.to_vec();
        let n = sampler.repeat_last_n.min(recent.len());
        apply_repeat_penalty(&mut l, &recent[recent.len() - n..], sampler.repeat_penalty);
        return if sampler.temperature > 0.0 { sample(&l, sampler, rng) } else { argmax(&l) };
    }
    if sampler.temperature > 0.0 { sample(logits, sampler, rng) } else { argmax(logits) }
}

/// YaRN per-dimension rope factors + attention mscale (pure; unit-checked in --validate).
/// Kernels compute theta = pos·base^(−2i/hd) / ff[i], so ff[i] = 1 / ((1−γᵢ)/s + γᵢ) where γᵢ is
/// the extrapolation weight from the β-ramp: γ=1 for high-frequency dims (many rotations inside
/// the original context — left untouched), γ=0 for low-frequency dims (full 1/s interpolation),
/// linear between the correction bounds. Bounds follow ggml's find_correction_range:
/// dim(β) = hd·ln(orig/(β·2π)) / (2·ln base), low = ⌊dim(β_fast)⌋, high = ⌈dim(β_slow)⌉ (defaults
/// 32/1). mscale = attn_factor·(1 + 0.1·ln s) for s>1, multiplying the ROTATED output of both q
/// and k (the ggml formulation → attention scores scale by mscale²).
pub fn yarn_factors(hd: usize, base: f32, s: f32, orig_ctx: u32, beta_fast: f32, beta_slow: f32,
                    attn_factor: f32) -> (Vec<f32>, f32) {
    let half = hd / 2;
    let dim_of = |beta: f32| -> f32 {
        (hd as f32) * (orig_ctx as f32 / (beta * 2.0 * std::f32::consts::PI)).ln() / (2.0 * base.ln())
    };
    let low = dim_of(beta_fast).floor().max(0.0);
    let high = dim_of(beta_slow).ceil().min(hd as f32 - 1.0);
    let ff = (0..half).map(|i| {
        let y = (i as f32 - low) / (high - low).max(0.001);
        let gamma = 1.0 - y.clamp(0.0, 1.0);
        1.0 / ((1.0 - gamma) / s + gamma)
    }).collect();
    let mscale = if s > 1.0 { attn_factor * (1.0 + 0.1 * s.ln()) } else { attn_factor };
    (ff, mscale)
}

fn xorshift01(state: &mut u64) -> f32 {
    let mut x = *state; x ^= x << 13; x ^= x >> 7; x ^= x << 17; *state = x;
    ((x >> 11) as f64 / (1u64 << 53) as f64) as f32
}

/// Temperature / top-k / top-p (nucleus) sampling from logits. `rng` is an
/// xorshift64 state the caller threads across steps.
pub fn sample(logits: &[f32], p: &SamplerParams, rng: &mut u64) -> u32 {
    if p.temperature <= 0.0 || p.top_k == 1 { return argmax(logits); }
    let mut idx: Vec<usize> = (0..logits.len()).collect();
    idx.sort_by(|&a, &b| logits[b].total_cmp(&logits[a]));
    let k = if p.top_k == 0 { idx.len() } else { p.top_k.min(idx.len()) };
    idx.truncate(k);
    let mx = logits[idx[0]];
    let mut probs: Vec<f32> = idx.iter().map(|&i| ((logits[i] - mx) / p.temperature).exp()).collect();
    let sum: f32 = probs.iter().sum();
    for q in &mut probs { *q /= sum; }
    if p.min_p > 0.0 {
        // probs are in descending order (idx sorted by logit desc), so probs[0] is the peak.
        let thresh = p.min_p * probs[0];
        let cut = probs.iter().position(|&pr| pr < thresh).unwrap_or(probs.len()).max(1);
        idx.truncate(cut); probs.truncate(cut);
        let s: f32 = probs.iter().sum(); for q in &mut probs { *q /= s; }
    }
    if p.top_p < 1.0 {
        let mut cum = 0.0; let mut cut = probs.len();
        for (i, &pr) in probs.iter().enumerate() { cum += pr; if cum >= p.top_p { cut = i + 1; break; } }
        idx.truncate(cut); probs.truncate(cut);
        let s: f32 = probs.iter().sum(); for q in &mut probs { *q /= s; }
    }
    let r = xorshift01(rng);
    let mut cum = 0.0;
    for (i, &pr) in probs.iter().enumerate() { cum += pr; if r < cum { return idx[i] as u32; } }
    idx[idx.len() - 1] as u32
}

/// Abstracts the two KV-cache backings a pool can run on — f32 `Session` and int8 `SessionQ8` — so
/// the coordinator (admission, batching, KV budget, park/revive) is written once. Running it on
/// `SessionQ8` gives ~4× the budget ceiling for free: its KV is ~¼ the bytes (packed int8 K/V + one
/// scale per row). Greedy output per backing matches that backing's own `generate_kv`/`generate_kv_q8`.
pub trait KvSession: Sized {
    type Prefix;
    /// Bytes of KV one session reserves for `max_seq` rows — the per-instance cost that sets the ceiling.
    fn kv_per_session(m: &Model, max_seq: usize) -> usize;
    /// Bytes a shared prefix cache of `prefix_len` rows holds.
    fn prefix_kv(m: &Model, prefix_len: usize) -> usize;
    /// Prefill a shared prefix exactly once (None if empty); returns (cache, len).
    fn make_prefix(gpu: &Gpu, m: &Model, prefix: &[u32]) -> (Option<Self::Prefix>, usize);
    /// Build a child prefix cache = `parent`'s first `parent_len` rows + `seg` prefilled at that
    /// offset — the trie edge that lets a longer shared prefix reuse a shorter one's KV.
    fn extend(gpu: &Gpu, m: &Model, parent: &Self::Prefix, parent_len: usize, seg: &[u32]) -> Self::Prefix;
    /// Create a session and KV-fork the shared prefix WITHOUT prefilling the suffix — its tokens are
    /// streamed in via `prefill_chunk` so a long prompt overlaps the running batch's decode (item E).
    fn make_empty(gpu: &Gpu, m: &Model, max_seq: usize, seed: u64, prefix: Option<&Self::Prefix>, prefix_len: usize) -> Self;
    /// Prefill a block of suffix tokens at the current position (advances pos, sets logits).
    fn prefill_chunk(&mut self, gpu: &Gpu, m: &Model, chunk: &[u32]);
    /// Sample this session's next token (reads logits + advances rng internally — disjoint fields).
    fn sample_next(&mut self, sampler: &SamplerParams) -> u32;
    /// One batched decode step over a contiguous slice of sessions.
    fn decode_batch(gpu: &Gpu, m: &Model, sess: &mut [Self], toks: &[u32]);
}
impl KvSession for Session {
    type Prefix = KvCache;
    fn kv_per_session(m: &Model, max_seq: usize) -> usize { m.n_layers * m.kv_dim() * 2 * 4 * max_seq }
    fn prefix_kv(m: &Model, plen: usize) -> usize { m.n_layers * m.kv_dim() * 2 * 4 * plen }
    fn make_prefix(gpu: &Gpu, m: &Model, prefix: &[u32]) -> (Option<KvCache>, usize) {
        if prefix.is_empty() { (None, 0) } else { (Some(gpu.prefill_prefix(m, prefix)), prefix.len()) }
    }
    fn extend(gpu: &Gpu, m: &Model, parent: &KvCache, parent_len: usize, seg: &[u32]) -> KvCache { gpu.extend_prefix(m, parent, parent_len, seg) }
    fn make_empty(gpu: &Gpu, m: &Model, max_seq: usize, seed: u64, prefix: Option<&KvCache>, prefix_len: usize) -> Session {
        let mut s = gpu.session_new(m, max_seq, seed);
        if let Some(pc) = prefix { gpu.seed_from_prefix(m, &mut s, pc, prefix_len); }
        s
    }
    fn prefill_chunk(&mut self, gpu: &Gpu, m: &Model, chunk: &[u32]) { gpu.session_prefill(m, self, chunk); }
    fn sample_next(&mut self, sp: &SamplerParams) -> u32 {
        if sp.temperature > 0.0 { sample(&self.logits, sp, &mut self.rng) } else { argmax(&self.logits) }
    }
    fn decode_batch(gpu: &Gpu, m: &Model, sess: &mut [Session], toks: &[u32]) { gpu.decode_batched(m, sess, toks); }
}
impl KvSession for SessionQ8 {
    type Prefix = QKvCache;
    // kq/vq: packed int8 = kv_dim bytes/row; ks/vs: one f32 scale/row. ≈ ¼ the f32 cache.
    fn kv_per_session(m: &Model, max_seq: usize) -> usize { m.n_layers * max_seq * (2 * m.kv_dim() + 8) }
    fn prefix_kv(m: &Model, plen: usize) -> usize { m.n_layers * plen * (2 * m.kv_dim() + 8) }
    fn make_prefix(gpu: &Gpu, m: &Model, prefix: &[u32]) -> (Option<QKvCache>, usize) {
        if prefix.is_empty() { (None, 0) } else { (Some(gpu.prefill_prefix_q8(m, prefix)), prefix.len()) }
    }
    fn extend(gpu: &Gpu, m: &Model, parent: &QKvCache, parent_len: usize, seg: &[u32]) -> QKvCache { gpu.extend_prefix_q8(m, parent, parent_len, seg) }
    fn make_empty(gpu: &Gpu, m: &Model, max_seq: usize, seed: u64, prefix: Option<&QKvCache>, prefix_len: usize) -> SessionQ8 {
        let mut s = gpu.session_q8_new(m, max_seq, seed);
        if let Some(pc) = prefix { gpu.seed_from_prefix_q8(m, &mut s, pc, prefix_len); }
        s
    }
    fn prefill_chunk(&mut self, gpu: &Gpu, m: &Model, chunk: &[u32]) { gpu.session_q8_prefill(m, self, chunk); }
    fn sample_next(&mut self, sp: &SamplerParams) -> u32 {
        if sp.temperature > 0.0 { sample(&self.logits, sp, &mut self.rng) } else { argmax(&self.logits) }
    }
    fn decode_batch(gpu: &Gpu, m: &Model, sess: &mut [SessionQ8], toks: &[u32]) { gpu.decode_batched_q8(m, sess, toks); }
}

/// A stateful, tick-driven pool of decode workers ("branches") that share one resident model
/// and an optional prefilled prefix. Where `run_batched` is one-shot (runs a queue to completion),
/// the pool is driven a step at a time by an external loop — the egui session or the voice/voting
/// bridge — so between ticks the caller can read partial output, admit new branches, PARK a losing
/// branch warm (its KV kept alive) and later REVIVE it cheaply instead of recomputing, or retire it.
/// Active branches are stepped together via `decode_batched`, so N branches cost sub-linearly
/// (shared weight reads). Greedy output per branch is identical to `run_batched` / `generate_kv`;
/// this is the seam where the sub-linear floor meets everything above it.
/// A cached shared-prefix KV node in the pool's trie: its token sequence, the prefilled cache, and
/// its length. A branch forks from a node by KV-copying it (no recompute) and prefilling only its tail.
struct PrefixNode<S: KvSession> { toks: Vec<u32>, cache: S::Prefix, len: usize }

pub struct WorkerPool<'a, S: KvSession = Session> {
    gpu: &'a Gpu,
    model: &'a Model,
    sampler: SamplerParams,
    eos: Option<u32>,
    max_batch: usize,
    max_seq: usize,
    nodes: Vec<PrefixNode<S>>,      // trie of cached shared-prefix KV — branches fork from any node (F)
    global: Option<usize>,         // node id of the prefix given to new() (the default fork for admit())
    active: Vec<Branch>,           // metadata, parallel by index to `active_sess`
    active_sess: Vec<S>,           // contiguous so decode_batch can take &mut [S]
    warm: Vec<(Branch, S)>,        // paused but alive — revive is a pointer move, no recompute
    pending: Vec<(Branch, Vec<u32>, Option<usize>)>, // (branch, suffix, node id to fork from); FIFO
    admitting: Vec<(Branch, S, Vec<u32>, usize)>, // mid-prefill: (branch, session, suffix, cursor) —
                                       // prefilled PREFILL_CHUNK tokens/tick so a long prompt overlaps
                                       // the running batch's decode instead of stalling it (item E)
    done: Vec<Branch>,
    // Plan item D — memory-bounded admission. `kv_budget` (bytes; 0 = unbounded) caps how many
    // sessions decode at once by KV memory, not just `max_batch` count: a session's KV cache is
    // allocated up front for `max_seq` rows, so concurrency is paid in `kv_per_session`. On unified
    // memory this is the real ceiling — admission stops before the device OOMs.
    kv_budget: usize,
    kv_per_session: usize,
    prefix_kv: usize,
    peak_active: usize,            // high-water mark of concurrent sessions (for the scaling bench)
}

/// One worker/branch in the pool. `emitted` is the tokens it has produced so far.
pub struct Branch {
    pub id: u64,
    pub label: String,
    pub emitted: Vec<u32>,
    pub max_new: usize,
    pub started: std::time::Instant,
}

/// What happened in one `step()` — enough for a UI to render incrementally and for the bridge
/// to react (surface a finished branch, flag a decision point, etc.).
#[derive(Default)]
pub struct StepReport {
    pub admitted: Vec<u64>,        // branches that became active this tick
    pub emitted: Vec<(u64, u32)>,  // (branch id, token) produced this tick
    pub finished: Vec<u64>,        // branches that completed this tick
}

/// Per-run speculative-decode stats. `accepted/rounds` is the mean accepted draft length (how
/// well the draft predicts the target); `emitted/rounds` is tokens per target verify-forward.
#[derive(Default)]
pub struct SpecStats { pub rounds: usize, pub accepted: usize, pub emitted: usize }

impl<'a, S: KvSession> WorkerPool<'a, S> {
    /// Create a pool over a shared model. A non-empty `prefix` is prefilled exactly once and
    /// KV-forked into every branch (so the common context is computed a single time).
    pub fn new(gpu: &'a Gpu, model: &'a Model, sampler: SamplerParams, eos: Option<u32>,
               max_batch: usize, max_seq: usize, prefix: &[u32]) -> Self {
        let (pc, plen) = S::make_prefix(gpu, model, prefix);
        let (nodes, global, prefix_kv) = match pc {
            Some(c) => (vec![PrefixNode { toks: prefix.to_vec(), cache: c, len: plen }], Some(0), S::prefix_kv(model, plen)),
            None => (Vec::new(), None, 0),
        };
        let kv_per_session = S::kv_per_session(model, max_seq.max(1));
        Self { gpu, model, sampler, eos, max_batch, max_seq, nodes, global,
               active: Vec::new(), active_sess: Vec::new(), warm: Vec::new(),
               pending: Vec::new(), admitting: Vec::new(), done: Vec::new(),
               kv_budget: 0, kv_per_session, prefix_kv,
               peak_active: 0 }
    }

    /// Cap concurrent sessions by a KV-memory budget (bytes), not just `max_batch` count — admission
    /// stops when the next session's KV wouldn't fit. 0 leaves it count-bounded only. Builder style:
    /// `WorkerPool::new(..).with_kv_budget(2 << 30)`.
    pub fn with_kv_budget(mut self, bytes: usize) -> Self { self.kv_budget = bytes; self }
    /// Bytes of KV one session reserves (n_layers · kv_dim · 2 · 4 · max_seq) — the per-instance cost
    /// that sets the concurrency ceiling on unified memory.
    pub fn kv_per_session(&self) -> usize { self.kv_per_session }
    /// Highest number of sessions that decoded together over the pool's life (the scaling readout).
    pub fn peak_active(&self) -> usize { self.peak_active }
    /// Whether one more session fits the KV budget (always true when unbounded, and always true for
    /// the FIRST session so a tight budget degenerates to sequential admission rather than deadlock).
    fn can_admit(&self) -> bool {
        if self.kv_budget == 0 { return true; }
        let n = self.active.len() + self.warm.len() + self.admitting.len();
        if n == 0 { return true; } // guarantee forward progress
        let used = self.prefix_kv + n * self.kv_per_session;
        used + self.kv_per_session <= self.kv_budget
    }
    pub const PREFILL_CHUNK: usize = 256;

    /// Queue a branch that forks from the pool's global prefix (the one given to `new`). Its `suffix`
    /// is the unique prompt tail after that prefix; it's prefilled when the branch is admitted.
    pub fn admit(&mut self, id: u64, label: impl Into<String>, suffix: Vec<u32>, max_new: usize) {
        let node = self.global;
        self.pending.push((Branch { id, label: label.into(), emitted: Vec::new(), max_new,
                                    started: std::time::Instant::now() }, suffix, node));
    }

    /// Cache `tokens` as a shared-prefix node and return its id, reusing the longest existing node
    /// that is a prefix of `tokens` (a trie extend — only the new segment is prefilled) or prefilling
    /// from scratch. Cache the shared conversation once, then `admit_at` each branch from it. The
    /// branch/voting substrate inside the stateful coordinator. `tokens` must be non-empty.
    pub fn cache_prefix(&mut self, tokens: &[u32]) -> usize {
        let mut best: Option<usize> = None;
        for (i, nd) in self.nodes.iter().enumerate() {
            if nd.len <= tokens.len() && tokens[..nd.len] == nd.toks[..]
               && best.map_or(true, |b| nd.len > self.nodes[b].len) { best = Some(i); }
        }
        let cache = match best {
            Some(b) if self.nodes[b].len == tokens.len() => return b, // exact node already cached
            Some(b) => { let pl = self.nodes[b].len; S::extend(self.gpu, self.model, &self.nodes[b].cache, pl, &tokens[pl..]) }
            None => S::make_prefix(self.gpu, self.model, tokens).0.expect("cache_prefix: tokens must be non-empty"),
        };
        self.prefix_kv += S::prefix_kv(self.model, tokens.len());
        self.nodes.push(PrefixNode { toks: tokens.to_vec(), cache, len: tokens.len() });
        self.nodes.len() - 1
    }

    /// Queue a branch that forks from a specific cached node (`cache_prefix`); `suffix` is its
    /// divergent tail after that node. Many branches fork from one cached conversation, each
    /// KV-copying it and prefilling only its own continuation — the branch/voting primitive.
    pub fn admit_at(&mut self, id: u64, label: impl Into<String>, node: usize, suffix: Vec<u32>, max_new: usize) {
        self.pending.push((Branch { id, label: label.into(), emitted: Vec::new(), max_new,
                                    started: std::time::Instant::now() }, suffix, Some(node)));
    }

    /// Move pending branches into free batch slots (prefilling each), up to `max_batch`.
    /// Move pending branches into the admitting (mid-prefill) queue while the KV budget and
    /// `max_batch` (over active+admitting) allow. Their suffix is prefilled later, in chunks.
    fn fill(&mut self) {
        while !self.pending.is_empty() && self.can_admit()
              && self.active.len() + self.admitting.len() < self.max_batch {
            let (b, suffix, node) = self.pending.remove(0); // FIFO
            let (pc, plen) = match node { Some(id) => (Some(&self.nodes[id].cache), self.nodes[id].len), None => (None, 0) };
            let s = S::make_empty(self.gpu, self.model, self.max_seq, 0x1000 + b.id, pc, plen);
            self.admitting.push((b, s, suffix, 0));
        }
    }

    /// Advance mid-prefill sessions by up to one `PREFILL_CHUNK` of total work this tick (so a long
    /// prompt is spread over several ticks while the active batch keeps decoding — item E). A session
    /// whose suffix is fully prefilled joins the active batch (reported as admitted). Small prompts
    /// finish within the chunk budget so several can admit in one tick; a big one takes one chunk/tick.
    fn advance_prefill(&mut self, rep: &mut StepReport) {
        let mut budget = Self::PREFILL_CHUNK;
        while budget > 0 && !self.admitting.is_empty() {
            let (b, mut s, suffix, cursor) = self.admitting.remove(0);
            let take = (suffix.len() - cursor).min(budget);
            s.prefill_chunk(self.gpu, self.model, &suffix[cursor..cursor + take]);
            budget -= take;
            let nc = cursor + take;
            if nc >= suffix.len() {
                rep.admitted.push(b.id);
                self.active.push(b);
                self.active_sess.push(s);
            } else {
                self.admitting.insert(0, (b, s, suffix, nc)); // resume next tick (budget exhausted here)
            }
        }
        self.peak_active = self.peak_active.max(self.active.len());
    }

    /// Advance one tick: admit pending into free slots, emit one token per active branch, retire
    /// finished branches, and batch-decode the survivors together. Mirrors `run_batched`'s
    /// per-branch semantics exactly (eos token is not recorded; max_new includes the final token).
    pub fn step(&mut self) -> StepReport {
        let mut rep = StepReport::default();
        self.fill();                    // pending → admitting (budget/count bounded)
        self.advance_prefill(&mut rep); // chunked prefill; completed sessions join `active`
        if self.active.is_empty() { return rep; }
        let sampler = self.sampler;
        let eos = self.eos;
        let toks: Vec<u32> = self.active_sess.iter_mut().map(|s| s.sample_next(&sampler)).collect();
        let mut sb = Vec::new(); let mut ss = Vec::new(); let mut st = Vec::new();
        for ((mut b, s), t) in self.active.drain(..).zip(self.active_sess.drain(..)).zip(toks) {
            if Some(t) == eos { rep.finished.push(b.id); self.done.push(b); continue; }
            b.emitted.push(t);
            rep.emitted.push((b.id, t));
            if b.emitted.len() >= b.max_new { rep.finished.push(b.id); self.done.push(b); }
            else { sb.push(b); ss.push(s); st.push(t); }
        }
        if !ss.is_empty() { S::decode_batch(self.gpu, self.model, &mut ss, &st); }
        self.active = sb; self.active_sess = ss;
        rep
    }

    /// Pause an active branch, keeping its KV cache alive in `warm` for cheap revival.
    pub fn park(&mut self, id: u64) -> bool {
        if let Some(i) = self.active.iter().position(|b| b.id == id) {
            let b = self.active.remove(i); let s = self.active_sess.remove(i);
            self.warm.push((b, s)); true
        } else { false }
    }
    /// Bring a warm branch back into the active batch (if a slot is free). No recompute — the
    /// branch resumes from exactly where it was parked.
    pub fn revive(&mut self, id: u64) -> bool {
        if self.active.len() >= self.max_batch { return false; }
        if let Some(i) = self.warm.iter().position(|(b, _)| b.id == id) {
            let (b, s) = self.warm.remove(i);
            self.active.push(b); self.active_sess.push(s); true
        } else { false }
    }
    /// Drop a branch entirely (active, warm, or pending), freeing its KV.
    pub fn retire(&mut self, id: u64) -> bool {
        if let Some(i) = self.active.iter().position(|b| b.id == id) { self.active.remove(i); self.active_sess.remove(i); return true; }
        if let Some(i) = self.admitting.iter().position(|(b, _, _, _)| b.id == id) { self.admitting.remove(i); return true; }
        if let Some(i) = self.warm.iter().position(|(b, _)| b.id == id) { self.warm.remove(i); return true; }
        if let Some(i) = self.pending.iter().position(|(b, _, _)| b.id == id) { self.pending.remove(i); return true; }
        false
    }

    /// Collect branches that have finished since the last drain.
    pub fn drain_done(&mut self) -> Vec<Branch> { std::mem::take(&mut self.done) }
    /// True when nothing is decoding, mid-prefill, or waiting (warm branches don't count — they're
    /// paused on purpose).
    pub fn is_idle(&self) -> bool { self.active.is_empty() && self.pending.is_empty() && self.admitting.is_empty() }
    pub fn active_ids(&self) -> Vec<u64> { self.active.iter().map(|b| b.id).collect() }
    pub fn warm_ids(&self) -> Vec<u64> { self.warm.iter().map(|(b, _)| b.id).collect() }
    pub fn counts(&self) -> (usize, usize, usize, usize) { (self.active.len(), self.warm.len(), self.pending.len(), self.done.len()) }
    /// Tokens a branch has emitted so far (searches active, then warm, then done).
    pub fn branch_emitted(&self, id: u64) -> Option<&[u32]> {
        if let Some(b) = self.active.iter().find(|b| b.id == id) { return Some(&b.emitted); }
        if let Some((b, _)) = self.warm.iter().find(|(b, _)| b.id == id) { return Some(&b.emitted); }
        if let Some(b) = self.done.iter().find(|b| b.id == id) { return Some(&b.emitted); }
        None
    }
}

#[cfg(test)]
mod decode_cfg_tests {
    use super::*;

    /// The scale-hoist in the WG128 / SG / V3 decode kernels replaces N per-lane `qs[base + k/32]` loads
    /// with ONE `qs[base + kk0/32]`. That is only bit-identical if every k in the group shares one 32-block.
    /// This test proves that invariant over the full k range, so a future edit to the kernels' k-stride
    /// can't silently break the assumption.
    #[test]
    fn scale_hoist_block_alignment_holds() {
        // WG128 / SG: each thread's word covers 4 consecutive k starting at a multiple of 4.
        for w in 0u32..100_000 {
            let kk0 = w * 4;
            let b = kk0 / 32;
            for j in 0..4u32 { assert_eq!((kk0 + j) / 32, b, "wg128/sg: k {} left block {}", kk0 + j, b); }
        }
        // V3: each thread's vec4 covers 16 consecutive k starting at a multiple of 16.
        for w in 0u32..100_000 {
            let kk0 = w * 16;
            let b = kk0 / 32;
            for j in 0..16u32 { assert_eq!((kk0 + j) / 32, b, "v3: k {} left block {}", kk0 + j, b); }
        }
    }

    #[test]
    fn decode_config_pack_roundtrips() {
        for kernel in [MatvecKernel::V1, MatvecKernel::V2, MatvecKernel::V2Wg128] {
            for tile in [1u32, 2, 4, 8, 16] {
                let c = DecodeConfig { kernel, tile };
                let u = DecodeConfig::unpack(c.pack());
                assert_eq!(u.kernel, kernel);
                assert_eq!(u.tile, tile);
            }
        }
        // legacy bare tiles (no kernel bits) decode as V2 (the pre-wg128 default), tile preserved
        assert_eq!(DecodeConfig::unpack(4), DecodeConfig { kernel: MatvecKernel::V2, tile: 4 });
        // every profile in the search matrix round-trips
        for &cfg in DECODE_PROFILES { assert_eq!(DecodeConfig::unpack(cfg.pack()), cfg); }
    }

    #[test]
    fn subgroup_kernel_is_valid_wgsl() {
        // naga must accept the subgroup reduction shader (enable subgroups; subgroupAdd; subgroup builtins).
        // This proves it COMPILES even though llvmpipe here can't RUN subgroups — the M2 validates results.
        let (ok, msg) = crate::wgsl_validate::check_wgsl(MATVEC_Q8_0_TILED_V2_WG128_SG);
        assert!(ok, "subgroup kernel failed naga validation:\n{msg}");
    }

    // Qwen3/3.5 QK-Norm: the GPU qk_norm (rmsnorm_t over [t*heads, head_dim] + shape restore) must match an
    // independent per-head RMSNorm reference. Validates the buffer-layout reshape on llvmpipe; the M2 runs
    // the same kernel. None → pass-through.
    #[test]
    fn qk_norm_matches_per_head_reference() {
        let gpu = Gpu::new();
        let (t, heads, hd) = (3usize, 4usize, 8usize);
        let d = heads * hd;
        let eps = 1e-6f32;
        // distinct values per position/head/dim, and a non-trivial per-dim norm weight
        let q: Vec<f32> = (0..t * d).map(|i| ((i % 13) as f32 - 6.0) * 0.5).collect();
        let w: Vec<f32> = (0..hd).map(|j| 1.0 + 0.1 * j as f32).collect();
        // reference: independent per-head RMSNorm over head_dim
        let mut want = vec![0.0f32; t * d];
        for row in 0..t * heads {
            let b = row * hd;
            let ms = (0..hd).map(|j| q[b + j] * q[b + j]).sum::<f32>() / hd as f32;
            let inv = 1.0 / (ms + eps).sqrt();
            for j in 0..hd { want[b + j] = q[b + j] * inv * w[j]; }
        }
        let qt = gpu.upload(&q, t, d);
        let wt = gpu.upload(&w, 1, hd);
        let got = gpu.download(&gpu.qk_norm(qt, &Some(wt), t, d, hd, eps));
        assert_eq!(got.len(), want.len());
        for (a, b) in got.iter().zip(&want) { assert!((a - b).abs() < 1e-4, "qk_norm mismatch: {a} vs {b}"); }
        // None → identity pass-through
        let qt2 = gpu.upload(&q, t, d);
        let pass = gpu.download(&gpu.qk_norm(qt2, &None, t, d, hd, eps));
        assert_eq!(pass, q, "no norm weight ⇒ unchanged");
    }
}
