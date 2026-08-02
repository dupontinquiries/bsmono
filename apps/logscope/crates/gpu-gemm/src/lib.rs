//! Transformer GPU kernels on wgpu (Vulkan on Asahi/Linux). Each kernel is a
//! WGSL compute shader dispatched through one shared helper; correctness is
//! checked against CPU references in the validator binary. These are the ops a
//! wgpu/Burn inference backend composes: linear (nn.Linear with transposed
//! weights), Q4_0 dequant, row softmax, RMSNorm, and scaled-dot-product attention.
use wgpu::util::DeviceExt;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;

pub mod gguf;
pub mod cpumatvec;
pub mod cpupool;
pub mod cpuattn;
pub mod cpudecode;
pub mod perfverdict;
pub mod qkvsplit;
pub mod prefetch;
pub mod mladims;
pub mod rolequant;
pub mod ragged;
pub mod specverify;
pub mod lookupdraft;
pub mod specloop;
pub mod gateclass;
pub mod tensornames;
pub mod cpuops;
pub mod model_kind; // classify a gguf (LLM / diffusion / vision-projector) before the LLM loader tries it
#[allow(dead_code)] pub mod memplan; // scratchwork: footprint + memory-fit planner for bigger models
pub mod enginelevers;
pub mod archfeat;

/// Engine tuning knobs a host application may set before the first `Gpu::new()`.
///
/// Defined HERE rather than taken from `logscope::tuneprofile` so gpu-gemm stays independent of the app
/// layer — the dependency runs app -> engine, and reversing it to share a config struct would be the
/// wrong trade. The app maps its profile onto this at startup.
///
/// Environment variables still win over anything set here: a knob exists to be turned during a
/// bisection, and a config file that silently overrode `LOGSCOPE_MATVEC_SG=1` would make that impossible.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineConfig {
    /// Allow the subgroup matvec kernel. Off on drivers where it can hang large matmuls.
    pub subgroup_matvec: bool,
    /// Q4_K super-blocks converted per dispatch — bounds GPU work per submit against a firmware watchdog.
    pub convert_chunk_blocks: usize,
}

impl Default for EngineConfig {
    fn default() -> Self { EngineConfig { subgroup_matvec: true, convert_chunk_blocks: 16384 } }
}

static ENGINE_CONFIG: std::sync::OnceLock<EngineConfig> = std::sync::OnceLock::new();

/// Set the engine configuration. Only the FIRST call takes effect — the values are read during device
/// setup, so a later change would apply to some decisions and not others, which is worse than ignoring
/// it. Returns whether this call was the one that set it.
pub fn set_engine_config(c: EngineConfig) -> bool { ENGINE_CONFIG.set(c).is_ok() }

/// The active configuration, defaults if never set.
pub fn engine_config() -> EngineConfig { *ENGINE_CONFIG.get().unwrap_or(&EngineConfig::default()) }

#[cfg(test)]
mod engine_config_tests {
    use super::*;

    #[test]
    fn defaults_are_permissive_and_only_the_first_set_wins() {
        // An unset config must not disable anything: a host that never configures the engine should get
        // the same behaviour as before this existed.
        let d = EngineConfig::default();
        assert!(d.subgroup_matvec, "default must not disable a kernel the driver supports");
        assert!(d.convert_chunk_blocks > 0);

        // Only the first set takes effect — the values are read during device setup, so a later change
        // would apply to some decisions and not others, which is worse than ignoring it.
        let first = set_engine_config(EngineConfig { subgroup_matvec: false, convert_chunk_blocks: 99 });
        let second = set_engine_config(EngineConfig { subgroup_matvec: true, convert_chunk_blocks: 1 });
        if first {
            assert!(!second, "a second set must be rejected, not silently applied");
            assert_eq!(engine_config().convert_chunk_blocks, 99);
            assert!(!engine_config().subgroup_matvec);
        }
    }
}

pub mod swa;
pub mod iq1s_grid;
#[allow(dead_code)] pub mod zerocopy; // scoped experiment: UMA zero-copy weight import (gated `zerocopy`)
#[allow(dead_code)] pub mod oom; // detect GPU allocation OOM → downgrade residency instead of crashing
pub mod governor; // backend-agnostic GPU resource governor: per-device budget + adaptive window k
#[allow(dead_code)] pub mod kvstage; // staging policy for the hot(f32)/warm(int8) KV cache — speed + 1M ctx
pub mod wgsl_validate; // in-process WGSL fitness gate (naga parse + validate)
#[allow(dead_code)] pub mod sparse; // sparse-residency planner (+ gated binder) — the arena's architectural replacement
#[allow(dead_code)] pub mod archcfg; // scratchwork: multi-architecture config registry (qwen2/gemma2…)
#[allow(dead_code)] pub mod moe;     // mixture-of-experts routing (Qwen4/Llama4 decode): top-k + weights
#[allow(dead_code)] pub mod mla;     // Multi-head Latent Attention (DeepSeek/GLM) compressed-KV — CPU reference
#[allow(dead_code)] pub mod dsa;     // DeepSeek Sparse Attention lightning indexer — CPU reference
#[allow(dead_code)] pub mod ngram;   // prompt-lookup (n-gram) speculative draft source — lossless speedup
#[allow(dead_code)] pub mod expert_predict; // context-conditioned MoE expert predictor for SSD-streaming prefetch
#[allow(dead_code)] pub mod expert_cache; // MoE expert-streaming policy: pin store + per-layer LRU slots + prefetch
#[allow(dead_code)] pub mod llama4;  // Llama 4 iRoPE attention temperature tuning (long-context length gen)
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

/// Process-lifetime map registry, mirroring Colibri's `g_maps` / `map_of_fd`: a model's file bytes are
/// mapped ONCE and kept for as long as the process might reference them, reused by path on a repeat load
/// rather than mapped again.
///
/// This exists so weights can be VIEWS into the file instead of copies. Colibri's expert cache depends on
/// exactly this ("gli expert diventano VISTE dentro mmap … la page cache del kernel È la cache") — you
/// cannot hand out `&[u8]` into a mapping that gets dropped when the loader returns, which is what ours
/// does today and why the MoE store has to copy ~50 GiB instead of pointing at the file.
///
/// Deliberately never unmapped, like Colibri's. The kernel reclaims file-backed pages under pressure on
/// its own, so an idle mapping costs address space rather than memory; reusing by path keeps repeated
/// loads of the same model from accumulating mappings.
static RETAINED_MAPS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, &'static FileBytes>>>
    = std::sync::OnceLock::new();

pub fn retain_map(path: &str) -> std::io::Result<&'static [u8]> {
    let reg = RETAINED_MAPS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Some(b) = reg.lock().unwrap().get(path) { return Ok(*b); }
    let fb = file_bytes(path)?;
    // Leak the mapping so the byte slice is 'static. This is the point of the registry: the borrow has to
    // outlive the loader for weights to reference the file rather than copy it.
    let leaked: &'static FileBytes = Box::leak(Box::new(fb));
    // Expert fills touch scattered blocks, not a stream — tell the kernel so it stops reading ahead pages
    // we will never look at.
    if let FileBytes::Mapped(m) = leaked { let _ = m.advise(memmap2::Advice::Random); }
    let bytes: &'static [u8] = leaked;
    reg.lock().unwrap().insert(path.to_string(), leaked);
    Ok(bytes)
}

/// Drop the resident pages of every retained map, keeping the mappings themselves.
///
/// The load pass reads every byte of the model, so afterwards the whole file sits in page cache —
/// ~61 GiB for Scout, which dominates RSS and looks alarming even though it is clean, file-backed and
/// reclaimable. Releasing it hands that decision back to the kernel immediately instead of leaving it to
/// memory pressure. Nothing is lost: a later expert fill re-faults the pages it needs, which is precisely
/// the streaming design ("the kernel page cache IS the cache").
/// Set when any weight is held as a VIEW into a retained mapping, rather than copied out of it.
///
/// Releasing page residency under a live view turns every subsequent read of that weight into a disk
/// fault. For a streamed MoE that is one fault storm per expert fill — the exact path the view mode
/// exists to make cheap.
pub static MAP_VIEWS_LIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Declare that a retained mapping is being VIEWED, not just read during load.
pub fn mark_map_views_live() {
    MAP_VIEWS_LIVE.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Drop page residency for every mapping EXCEPT the one currently in use.
///
/// The registry is deliberately leak-on-purpose — weights hold `&'static [u8]` into it, so a mapping can
/// never be unmapped while a model that references it is alive. But nothing evicted a mapping when the
/// model was REPLACED, and `autoload_if_changed` fires on every selection change: a session that touches
/// three models ends up holding three models' worth of page cache, only one of which is live.
///
/// Observed: DeepSeek-8B, GLM-4.7-Flash and Qwen3-30B all mapped in one run, ~110 GiB of address space
/// for one model's worth of work. The address space itself is cheap; the PAGE CACHE is not, and on a
/// machine where the GPU budget is already the binding constraint it is competing for exactly the memory
/// the live model needs.
///
/// This releases residency rather than unmapping, because unmapping would invalidate slices that a
/// still-referenced model may hold. The pages come back on demand if anything does touch them.
/// Ask the kernel to start faulting in part of a retained mapping, before it is needed.
///
/// The CPU analogue of Colibri's `couple_prefetch`: a 60 GiB model faults every page exactly once, and
/// without this that happens during the first token, serialised against the arithmetic that wants them.
/// `WillNeed` starts the work asynchronously.
///
/// Deliberately not a blocking populate — that would move the stall rather than hide it. Advisory, so
/// this reports whether the call was made rather than pretending pages are resident afterwards.
pub fn prefetch_mapped(path: &str, offset: usize, len: usize) -> bool {
    let Some(reg) = RETAINED_MAPS.get() else { return false };
    let g = reg.lock().unwrap();
    let Some(fb) = g.get(path) else { return false };
    if let FileBytes::Mapped(m) = *fb {
        if offset >= m.len() || len == 0 { return false; }
        let n = len.min(m.len() - offset);
        return m.advise_range(memmap2::Advice::WillNeed, offset, n).is_ok();
    }
    false
}

pub fn release_stale_map_residency(keep: &str) -> usize {
    if MAP_VIEWS_LIVE.load(std::sync::atomic::Ordering::Relaxed) { return 0; }
    let Some(reg) = RETAINED_MAPS.get() else { return 0 };
    let g = reg.lock().unwrap();
    let mut n = 0usize;
    for (path, fb) in g.iter() {
        if path == keep { continue; }
        if let FileBytes::Mapped(m) = *fb {
            // SAFETY: same argument as release_map_residency — read-only, file-backed, shared mapping of
            // a file we never write, so discarding resident pages loses nothing but the cache.
            if unsafe { m.unchecked_advise(memmap2::UncheckedAdvice::DontNeed) }.is_ok() { n += 1; }
        }
    }
    n
}

pub fn release_map_residency() -> usize {
    // GUARD. This was unconditional, and it is safe ONLY when the load pass has finished reading and
    // nothing holds a view into the mapping. When experts are file VIEWS (`RawExpertSrc`), the mapping is
    // live for the process lifetime and MADV_DONTNEED turns every fill into a page fault from disk —
    // converting the view path's whole advantage into its opposite, and stalling the CPU inside the
    // write_buffer that a GPU dispatch is waiting on.
    //
    // Scout does not hit this today because its Q6_K expert tensor forces the host-copy path. Any
    // all-Q4_K MoE checkpoint would, which is precisely the case the view path was built for.
    if MAP_VIEWS_LIVE.load(std::sync::atomic::Ordering::Relaxed) {
        eprintln!("wgpu-engine: keeping page cache — expert weights are VIEWS into the mapped file, so \
dropping residency would fault every fill from disk");
        return 0;
    }
    let Some(reg) = RETAINED_MAPS.get() else { return 0 };
    let g = reg.lock().unwrap();
    let mut n = 0;
    for fb in g.values() {
        if let FileBytes::Mapped(m) = *fb {
            // SAFETY: memmap2 gates MADV_DONTNEED because on a PRIVATE mapping it discards written data.
            // Ours is a read-only, file-backed, shared mapping of a model file we never write, so dropping
            // resident pages loses nothing — any later read re-faults them from the file.
            if unsafe { m.unchecked_advise(memmap2::UncheckedAdvice::DontNeed) }.is_ok() { n += 1; }
        }
    }
    n
}

/// Process memory split from /proc/self/smaps_rollup: (anonymous KiB, file-backed KiB).
///
/// Worth reporting because the two are not comparable: anonymous memory is real pressure that can get the
/// process OOM-killed, while clean file-backed pages are reclaimable and mostly cosmetic. A single RSS
/// number conflates them and makes a healthy run look like a leak.
pub fn memory_split_kb() -> Option<(u64, u64)> {
    let s = std::fs::read_to_string("/proc/self/smaps_rollup").ok()?;
    let get = |key: &str| -> Option<u64> {
        s.lines().find(|l| l.starts_with(key))?
            .split_whitespace().nth(1)?.parse().ok()
    };
    Some((get("Anonymous:")?, get("Pss_File:").or_else(|| get("RssFile:"))?))
}

/// How many files are currently mapped for the process lifetime, and their total size.
pub fn gguf_retained_stats() -> (usize, u64) { retained_map_stats() }

pub fn retained_map_stats() -> (usize, u64) {
    match RETAINED_MAPS.get() {
        None => (0, 0),
        Some(m) => { let g = m.lock().unwrap(); (g.len(), g.values().map(|b| b.len() as u64).sum()) }
    }
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
pub static BG_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static BG_MISSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// SUBMIT ATTRIBUTION — which cause drained the frame. `GPU_SUBMITS` says how many; these say WHY,
/// which is the question that actually leads to a fix. Added because a Metal run showed 1119
/// submits/token at 2.0 ms each while `flush_every` was 128 — meaning ~5 passes per submit, so
/// something other than the pass counter was doing the flushing and no amount of reasoning from the
/// outside could tell which.
pub static SUBMIT_BY_PASSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static SUBMIT_BY_RETAIN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static SUBMIT_BY_DOWNLOAD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static SUBMIT_BY_DIMS_WRAP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static SUBMIT_BY_END_FRAME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Dispatches that ran with NO frame open, each becoming its own command buffer. This is the expensive
/// one and it was the only path left untagged — a run showed 4256 submits against 729 attributed, and
/// the missing 3500 had to be here.
pub static SUBMIT_BY_NO_FRAME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Nanoseconds inside `queue.submit` — encoder finish plus the driver handoff.
///
/// The profile split wall time into setup (bind groups) and sync (waiting), and SUBMIT ITSELF WAS
/// NEITHER. At 15 submits per token over thousands of tokens that is a third bucket, and on a driver
/// whose completion callbacks are slow enough to disable the governor — your log reports "submit
/// stalled 739ms" — it is exactly where time would hide.
///
/// Added after finding that 16 of 17 polls were uncounted: the lesson there was that an unmeasured
/// path reads as an unexplained one, so the remaining unmeasured path is worth closing before drawing
/// any more conclusions from the split.
pub static GPU_SUBMIT_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Nanoseconds in host-side sampling: logits post-processing, penalties, top-k/p, argmax.
///
/// The fourth thing a generation does, and the last one nothing timed. With a 128k vocabulary this is
/// half a megabyte of logits per token walked on the CPU, and none of it appears in setup, submit or
/// sync — so it would land in the same unaccounted bucket that three commits have now been spent
/// emptying.
///
/// Counted here rather than in the caller so every sampling path shares one definition; a timer per
/// call site is how two of them end up measuring different spans.
pub static SAMPLE_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Time a host-side sampling step.
#[inline]
pub fn timed_sample<T>(f: impl FnOnce() -> T) -> T {
    let t = std::time::Instant::now();
    let r = f();
    SAMPLE_NS.fetch_add(t.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    r
}

pub fn sample_ns() -> u64 { SAMPLE_NS.load(std::sync::atomic::Ordering::Relaxed) }

/// One line attributing every submit to its cause, for the profile report.
/// A snapshot of the submit-cause counters, for reporting a DELTA rather than a lifetime total.
///
/// The profile measures submits as a delta from a snapshot and read the causes as absolutes, so one
/// report carried two clocks: your log showed "349 submits" beside causes summing to 4132, and the
/// causes grew across turns while the submit count did not.
///
/// gpu_counters_reset would also fix it and is only called by the validator; adding a second reset
/// from the app would race with that one. A snapshot has no such conflict — two readers can each hold
/// their own baseline.
#[derive(Clone, Copy, Default)]
pub struct SubmitCauseSnapshot {
    passes: u64, retain: u64, download: u64, dims_wrap: u64, end_frame: u64, no_frame: u64,
}

impl SubmitCauseSnapshot {
    /// Downloads since an earlier snapshot.
    ///
    /// Exposed so the profile can check itself: every download maps a buffer and every map polls, so
    /// a poll count below the download count means one of the two counters is not measuring what its
    /// name says.
    pub fn downloads_since(&self, since: &SubmitCauseSnapshot) -> u64 {
        self.download.saturating_sub(since.download)
    }

    pub fn now() -> SubmitCauseSnapshot {
        use std::sync::atomic::Ordering::Relaxed;
        SubmitCauseSnapshot {
            passes: SUBMIT_BY_PASSES.load(Relaxed), retain: SUBMIT_BY_RETAIN.load(Relaxed),
            download: SUBMIT_BY_DOWNLOAD.load(Relaxed), dims_wrap: SUBMIT_BY_DIMS_WRAP.load(Relaxed),
            end_frame: SUBMIT_BY_END_FRAME.load(Relaxed), no_frame: SUBMIT_BY_NO_FRAME.load(Relaxed),
        }
    }
}

/// Attribution since `since`, so the causes share the profile's clock.
pub fn submit_attribution_since(since: SubmitCauseSnapshot, actual: u64) -> String {
    use std::sync::atomic::Ordering::Relaxed;
    let now = SubmitCauseSnapshot::now();
    let d = |a: u64, b: u64| a.saturating_sub(b);
    let (p, r, dl) = (d(now.passes, since.passes), d(now.retain, since.retain),
                      d(now.download, since.download));
    let (w, e, nf) = (d(now.dims_wrap, since.dims_wrap), d(now.end_frame, since.end_frame),
                      d(now.no_frame, since.no_frame));
    let _ = Relaxed;
    let counted = p + r + dl + w + e + nf;
    let total = counted.max(1);
    let gap = actual.saturating_sub(counted);
    format!("submit causes — no-frame {} ({:.0}%), passes {} ({:.0}%), download {} ({:.0}%), end_frame {} ({:.0}%), dims-wrap {}, retain {} | {} of {} submits attributed{} (shares are of the attributed subset)",
        nf, nf as f64 * 100.0 / total as f64, p, p as f64 * 100.0 / total as f64,
        dl, dl as f64 * 100.0 / total as f64, e, e as f64 * 100.0 / total as f64,
        w, r, counted, actual,
        if gap > 0 { format!(" — {gap} UNATTRIBUTED") } else { String::new() })
}

pub fn submit_attribution() -> String {
    use std::sync::atomic::Ordering::Relaxed;
    let (p, r, d, w, e, nf) = (SUBMIT_BY_PASSES.load(Relaxed), SUBMIT_BY_RETAIN.load(Relaxed),
        SUBMIT_BY_DOWNLOAD.load(Relaxed), SUBMIT_BY_DIMS_WRAP.load(Relaxed),
        SUBMIT_BY_END_FRAME.load(Relaxed), SUBMIT_BY_NO_FRAME.load(Relaxed));
    let counted = p + r + d + w + e + nf;
    let actual = GPU_SUBMITS.load(Relaxed);
    // PERCENTAGES ARE OF THE ATTRIBUTED SUBSET, not of all submits, and the line now says so. Sharing a
    // sentence with "N of M attributed" while using a different denominator is unreadable: your log
    // showed "817 submits" in the profile line and causes summing to 4269 in the next, and no reader
    // can reconcile those without knowing which clock each is on.
    //
    // They were also on different clocks in fact, until this commit: GPU_SUBMITS was reset per run and
    // the SUBMIT_BY_* counters never were.
    let total = counted.max(1);
    // Report the SHORTFALL too. The first version of this line accounted for 729 of 4256 submits and
    // said nothing about the other 3527, which made a partial answer look like a complete one.
    let gap = actual.saturating_sub(counted);
    format!("submit causes — no-frame {} ({:.0}%), passes {} ({:.0}%), download {} ({:.0}%), \
end_frame {} ({:.0}%), dims-wrap {}, retain {} | {} of {} submits attributed{} (shares are of the attributed subset)",
        nf, nf as f64 * 100.0 / total as f64, p, p as f64 * 100.0 / total as f64,
        d, d as f64 * 100.0 / total as f64, e, e as f64 * 100.0 / total as f64, w, r,
        counted, actual,
        if gap > 0 { format!(" — {gap} UNATTRIBUTED, a submit path is still untagged") } else { String::new() })
}

/// Max retained (submitted-but-not-freed) keep-buffers before a mid-window flush forces a poll+free.
/// Bounds live uniform buffers so a big sampler window can't exhaust Metal's pool. ~8k tiny uniforms
/// is a few hundred KB — well under any limit — while still amortizing the poll across many dispatches.
const DIMS_RETAIN_CAP: usize = 8192;
/// Largest batch allowed after a readback on a driver whose submit governor cannot run.
///
/// UNUSED. Kept with its history because the idea recurs and the implementation was worse than the
/// problem: applied as a permanent `store` it ratcheted the tuner's 128 down to 32 on the first
/// readback of the first token and never let it back up. If pacing is ever needed here again, it must
/// bound a single batch and restore the tuner's value afterwards — `stream_session_window` already does
/// exactly that with a `saved_flush`, and that is the pattern to copy.
#[allow(dead_code)]
const POST_READBACK_FLUSH_CAP: u32 = 32;
/// Windowed-decode mid-loop flush cadence: flush+poll every N sampler slots so a large window's
/// per-slot buffers (bind groups, dims, decode intermediates) free instead of accumulating in one
/// pinned frame until the device runs out of buffer objects. Small enough to bound live buffers on
/// Metal, large enough that the flush cost amortizes over many dispatches. The `pre` logits the replay
/// needs are held by the caller, not the frame, so they survive the flush.
const SLOT_FLUSH: usize = 16;
/// Content-cache cap: distinct (shape) dims that get their own permanent buffer. Sized to comfortably
/// hold every stable per-shape uniform a model uses after warmup; per-position dims spill to the ring.
const DIMS_CACHE_CAP: usize = 4096;
/// Cache cap for bind groups (opt-in bg cache). Sized for a large-layer MoE decode's distinct
/// (pipeline, buffers, dims) combos across the recycled activation set; once full it falls back to
/// create-without-cache, so it's never worse than the old always-create path.
const BG_CACHE_CAP: usize = 8192;
/// Bind-group cache key: pipeline identity + the ordered buffer handles (inputs, then output) + the dims
/// buffer handle. wgpu::Buffer is Eq/Hash on its inner handle, so equal keys ⇒ identical bindings.
/// (pipeline, [(input buffer, view offset, view size)…], output buffer, dims buffer).
///
/// The view span is PART OF THE KEY rather than a reason to skip the cache. A bind group binds a buffer
/// range, so two calls with the same buffers and the same ranges produce the same bind group — and
/// streaming MoE weights are all views into slot buffers, which is precisely the workload that was
/// bypassing the cache. A view whose offset is not in the key would be a correctness bug (one range's
/// bind group reused for another); including it makes the reuse sound.
/// (pipeline, inputs as (buffer, offset, size), OUTPUT as (buffer, offset, size), dims buffer).
///
/// The output carries its offset and size for the same reason the inputs do (_220): a cached bind group
/// reused with a different view writes to the wrong place. This was missed when views were added to the
/// inputs, and the output was bound with `as_entire_binding()` — so a chunked conversion wrote every
/// chunk at offset 0. Fixing the binding without fixing the KEY would have been worse than leaving it:
/// the first chunk's bind group would be reused for all of them, silently.
type BgKey = (usize, Vec<(wgpu::Buffer, u64, u64)>, (wgpu::Buffer, u64, u64), wgpu::Buffer);
/// Reusable dims-ring size: how many uniform buffers the cache-full path rotates through. Bounds the
/// TOTAL live dims buffer count (cache cap + ring) so Metal's buffer-object ceiling is never hit no
/// matter how long generation runs. The ring rewrites via write_buffer; a flush+poll fences a wrap so
/// a slot is never rewritten while an in-flight dispatch still reads it.
const DIMS_RING_SIZE: usize = 2048;

/// ## A structural difference from llama.cpp, measured in buffer objects
///
/// Every dispatch here needs four u32s of shape, and we pass them in a UNIFORM BUFFER — cached by
/// content up to `DIMS_CACHE_CAP` with a `DIMS_RING_SIZE` ring behind it. **Up to 6144 buffer objects
/// holding sixteen bytes each.** The governor's own model puts `baseline_objects` at 6400 against a
/// `soft_object_cap` of 8192, so the engine starts at 78% of its object budget before allocating a
/// single weight.
///
/// llama.cpp's Vulkan backend passes the same parameters as PUSH CONSTANTS: no buffer, no cache, no
/// ring, no wrap fence, no retain list. Vulkan guarantees at least 128 bytes; our dims are 16.
///
/// Adopting that would DELETE rather than shrink:
/// * the dims cache and its 4096 buffers,
/// * the 2048-entry ring and the wrap that calls `flush_and_wait`,
/// * `DIMS_RETAIN_CAP` and the retained-keep list,
/// * the `dims_alloc_failed` path and the M4 "Buffer 'dims' invalid" crash it exists to survive,
/// * the `SUBMIT_BY_DIMS_WRAP` attribution category, because it would not exist.
///
/// NOT ATTEMPTED HERE, and the capability is unverified: wgpu 29 does not expose `PUSH_CONSTANTS` or
/// `max_push_constant_size` under the names this crate would need, so whether it is reachable through
/// this abstraction is an open question rather than a known yes. Recorded because the object-count
/// arithmetic is the strongest argument yet for why our memory behaviour differs from llama.cpp's, and
/// it does not depend on the answer.
pub const DIMS_PUSH_BYTES: u32 = 16;
/// Fixed uniform-buffer size for every dims buffer (largest dims payload is 16 u32 = 64 bytes; round up).
const DIMS_BUF_SIZE: u64 = 256;
/// How many times the retain-cap forced a mid-window poll+free (test/telemetry).
pub static DIMS_RETAIN_POLLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn dims_retain_polls() -> u64 { DIMS_RETAIN_POLLS.load(std::sync::atomic::Ordering::Relaxed) }pub fn dims_cache_hits() -> u64 { DIMS_HITS.load(std::sync::atomic::Ordering::Relaxed) }
pub fn dims_cache_misses() -> u64 { DIMS_MISSES.load(std::sync::atomic::Ordering::Relaxed) }
/// Buffer-pool accounting: a MISS is a real `create_buffer` (allocation churn — the other half of
/// "upload locking"); a HIT recycles a same-size buffer freed by a dropped Tensor. Hardware-independent.
pub static POOL_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static POOL_MISSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn pool_hits() -> u64 { POOL_HITS.load(std::sync::atomic::Ordering::Relaxed) }
pub fn pool_misses() -> u64 { POOL_MISSES.load(std::sync::atomic::Ordering::Relaxed) }
pub fn gpu_submits() -> u64 { GPU_SUBMITS.load(std::sync::atomic::Ordering::Relaxed) }
pub fn gpu_polls() -> u64 { GPU_POLLS.load(std::sync::atomic::Ordering::Relaxed) }
/// Nanoseconds spent inside queue.submit this run.
pub fn gpu_submit_ns() -> u64 { GPU_SUBMIT_NS.load(std::sync::atomic::Ordering::Relaxed) }

pub fn gpu_setup_ns() -> u64 { GPU_SETUP_NS.load(std::sync::atomic::Ordering::Relaxed) }
pub fn gpu_sync_ns() -> u64 { GPU_SYNC_NS.load(std::sync::atomic::Ordering::Relaxed) }
/// Reset every per-run counter.
///
/// THE SUBMIT-CAUSE COUNTERS WERE MISSING, and the symptom was visible in every profile line: across
/// four consecutive runs `no-frame` read 1480 each time while its share moved from 35% to 7%. The
/// numerator was cumulative and the denominator was per-run, so the percentage described nothing —
/// and it is the number an operator would tune against.
///
/// Adding a counter and forgetting the reset is easy precisely because the counter WORKS: it counts,
/// the line prints, and only a comparison across runs shows it never moved.
/// Count and time a blocking device poll.
///
/// SEVENTEEN CALL SITES, ONE COUNTED. The profile reported "1 polls" in every run — at 349 submits
/// and at 8063 — because only the download path incremented, and it runs once per generation.
///
/// The same site is the only one that adds to GPU_SYNC_NS, which is why sync reads 0 ms of 19860 ms:
/// the profile was timing one poll out of seventeen and reporting the result as though it covered all
/// waiting. That is what made "UNACCOUNTED, 0.2% accounted" look like a mystery rather than a
/// measurement gap — 99.8% of the time was in waits nobody instrumented.
///
/// Wrapping them means the split between setup and sync finally describes the whole generation.
#[inline]
pub fn counted_poll(device: &wgpu::Device) {
    use std::sync::atomic::Ordering::Relaxed;
    let t = std::time::Instant::now();
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    GPU_POLLS.fetch_add(1, Relaxed);
    GPU_SYNC_NS.fetch_add(t.elapsed().as_nanos() as u64, Relaxed);
}

pub fn gpu_counters_reset() {
    for c in [&GPU_SUBMITS, &GPU_POLLS, &GPU_SETUP_NS, &GPU_SYNC_NS,
              &DIMS_HITS, &DIMS_MISSES, &POOL_HITS, &POOL_MISSES,
              &SUBMIT_BY_PASSES, &SUBMIT_BY_RETAIN, &SUBMIT_BY_DOWNLOAD,
              &SUBMIT_BY_DIMS_WRAP, &SUBMIT_BY_END_FRAME, &SUBMIT_BY_NO_FRAME, &GPU_SUBMIT_NS,
              &SAMPLE_NS] {
        c.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

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
    /// Bind-group cache (opt-in via LOGSCOPE_BG_CACHE): a fresh bind group was created on EVERY dispatch,
    /// which the Asahi profile shows as a dispatch-bound (CPU-setup) hotspot — ~480 creations/token on a
    /// 48-layer MoE decode. A bind group only encodes WHICH buffers bind to WHICH slots (not their
    /// contents), so it's safe to reuse when the same pipeline runs on the same buffer identities; the
    /// pool recycles a small set of activation buffers so those identities repeat across tokens. Key =
    /// (pipeline ptr, input+out buffer handles, dims buffer handle); wgpu::Buffer is Eq/Hash on its inner
    /// handle. Bit-identical by construction (same buffers, same bindings). Capped like the dims cache.
    bg_cache: Mutex<HashMap<BgKey, std::sync::Arc<wgpu::BindGroup>>>,
    /// Free-list recycling per-op output buffers across ops/tokens (see BufferPool).
    pool: std::sync::Arc<BufferPool>,
    arena: Mutex<Option<ResidentArena>>,
    /// Q4_K conversions recorded during load and run in batches at the end (see `flush_q4k`).
    pending_q4k: Mutex<Vec<PendingQ4k>>,
    /// Persistent staging buffer for expert-slot fills, so decode never allocates.
    moe_stage: Mutex<Option<Tensor>>,
    /// Monotonic fill counter selecting the staging ring slot. See `STAGE_RING`.
    stage_cursor: Mutex<usize>,
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
    /// ROUTE_TRACE (colibrì borrow): when Some, the v1 MoE path appends each token's per-layer
    /// routed-expert set here for offline coupling analysis (does this model's routing predict the
    /// next layer's, like GLM's 71.6%?). The current decode layer is stashed here by the block loop;
    /// -1 = not tracing. Measurement only — zero effect on computation, and only wired on the v1
    /// readback path (which already has the CPU-side routing) so it never adds a sync.
    pub trace_layer: std::sync::atomic::AtomicI32,
    /// Current token id for the route trace's lexical signal (context-conditioned expert prediction). Set
    /// alongside trace_layer during a traced decode; -1 when not tracing / unknown (e.g. prefill batch).
    pub trace_tok: std::sync::atomic::AtomicI64,
    /// Keep-alive for buffers (dims uniforms, bind groups) whose frames have been SUBMITTED but not
    /// yet completed. submit() is async; wgpu 29 does not tie buffer lifetime to in-flight work, so a
    /// mid-window flush that drops its frame.keep recycles dims buffers the GPU is still reading →
    /// "Buffer 'dims' is invalid". Flush points move their keep here instead of dropping it; the next
    /// poll (which waits for GPU completion) clears it. Bounded: cleared every readback.
    pending_keep: Mutex<Vec<Box<dyn std::any::Any + Send>>>,
    /// Reusable dims-uniform ring (see ring_dims): a fixed set of UNIFORM buffers rewritten in rotation
    /// for the per-position dims that would otherwise create a new Metal buffer every dispatch. `(buffers,
    /// next_index, writes_since_fence)`. Lazily filled up to DIMS_RING_SIZE. Caps total dims buffer count.
    dims_ring: Mutex<(Vec<std::sync::Arc<wgpu::Buffer>>, usize, usize)>,
    /// Backend-agnostic resource governor: knows this device's budget shape (Metal buffer-objects /
    /// Vulkan allocation-count / …) and self-tunes the safe windowed-decode k from live pressure, so the
    /// window never marches into the device's ceiling. Replaces the hardcoded LOGSCOPE_WINDOW_MAX guess.
    governor: Mutex<crate::governor::Governor>,
    /// Set when a dims-uniform allocation was rejected by the device (validation error captured, not
    /// aborted). The windowed generate path checks it to fall back to leaner per-token decode.
    dims_alloc_failed: std::sync::atomic::AtomicBool,
    /// Generic fault→degrade latch (cf. Colibri: GPU is a try-and-return shortcut in front of a safe
    /// path). Monotonic, only ever raised: 0 = fast path; >=1 = a GPU op faulted, so future ops prefer the
    /// conservative kernels (serial attention, bounded submits) that can't wedge the driver. This makes an
    /// unknown driver quirk a one-time slowdown instead of a hang — we catch the fault once and stay safe.
    degrade_level: std::sync::Arc<std::sync::atomic::AtomicU32>,
    /// Set by the device uncaptured-error handler when an OOM fires (shared with degrade_level's Arc via a
    /// separate flag the handler can flip without the whole Gpu). A decode step checks it and aborts the
    /// step gracefully instead of the process panicking through wgpu's default handler.
    oom_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
pub enum MatvecKernel { V1 = 0, V2 = 1, V2Wg128 = 2, V2WgSg = 3,
    /// 128-bit `vec4<u32>` loads — four words per memory transaction instead of one.
    ///
    /// WAS UNREACHABLE. The kernel existed, was benchmarked in main.rs, and was selectable only via
    /// LOGSCOPE_MATVEC_V3 — so the per-shape autotune searched every OTHER kernel and picked V2Wg128.
    /// I read that as "kernel choice is exhausted"; it wins the search it was entered in.
    ///
    /// Its own comment says why it should win a latency-bound decode: the profile is ~6% bandwidth and
    /// ~1% ALU, the GPU idling on memory, and wider loads put more bytes in flight per request. Both
    /// references treat 4-wide as the baseline — ggml-metal loads `device const float4*` throughout.
    V3 = 4 }
impl MatvecKernel {
    fn from_id(id: u32) -> MatvecKernel { match id { 0 => MatvecKernel::V1, 4 => MatvecKernel::V3, 2 => MatvecKernel::V2Wg128, 3 => MatvecKernel::V2WgSg, _ => MatvecKernel::V2 } }
    pub fn id(self) -> u32 { self as u32 }
    pub fn label(self) -> &'static str { match self { MatvecKernel::V3 => "v3-vec4", MatvecKernel::V1 => "v1", MatvecKernel::V2 => "v2", MatvecKernel::V2Wg128 => "v2-wg128", MatvecKernel::V2WgSg => "v2-wg128-sg" } }
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
    // V3 FIRST AMONG THE NEW ENTRIES: it is the kernel the profile's own analysis expects to win a
    // latency-bound decode, and it was searched by nobody. Tiles 2/4/8 mirror what the others probe.
    DecodeConfig { kernel: MatvecKernel::V3, tile: 2 }, DecodeConfig { kernel: MatvecKernel::V3, tile: 4 },
    DecodeConfig { kernel: MatvecKernel::V3, tile: 8 },
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
/// Bind-group caching in dispatch_with. OPT-IN (default off) while it proves out on hardware: reuses the
/// bind group for a repeated (pipeline, buffers, dims) instead of recreating it per dispatch — targets the
/// Asahi dispatch-bound (CPU command-setup) cost. LOGSCOPE_BG_CACHE=1 enables. Bit-identical either way.
/// Umbrella flag: LOGSCOPE_FAST_DECODE=1 turns on ALL the validated (opt-in) dispatch-reduction levers at
/// once — bind-group cache + the two MoE fusions — so a user testing decode speed on the M2 Ultra flips one
/// switch instead of three. Each individual flag still works on its own; this just ORs in. All three are
/// proven bit-identical to the default path, so the umbrella changes speed only, never output.
/// Magical dispatch-lever autotune: rather than leaving the (bit-identical, opt-in) dispatch levers off by
/// default, MEASURE on the first real decode tokens whether they help THIS device+model and switch on
/// mid-session if they do — no restart, no env vars. Because all three levers are proven bit-identical to
/// the default path, flipping this state mid-generation changes speed only, never output; that's what makes
/// a silent mid-session switch safe. Persisted per-GPU (like the tile cache) so later loads skip re-probing.
/// State: 0=unmeasured, 1=measuring-OFF phase, 2=measuring-ON phase, 3=decided ON, 4=decided OFF.
pub static FAST_AB_STATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
const AB_UNMEASURED: u8 = 0;
const AB_MEASURING_OFF: u8 = 1;
const AB_MEASURING_ON: u8 = 2;
const AB_DECIDED_ON: u8 = 3;
const AB_DECIDED_OFF: u8 = 4;

/// Whether the dispatch levers should be active right now. Precedence: an explicit env override always wins
/// (user intent); otherwise the A/B state decides (ON during the measuring-ON phase and once decided ON).
fn dispatch_levers_active() -> bool {
    match std::env::var("LOGSCOPE_FAST_DECODE").as_deref() {
        Ok("1") => return true,
        Ok("0") => return false, // hard-disable: user opted out, skip the A/B entirely
        _ => {}
    }
    matches!(FAST_AB_STATE.load(std::sync::atomic::Ordering::Relaxed), AB_MEASURING_ON | AB_DECIDED_ON)
}

/// Pure A/B decision core for the magical dispatch-lever autotune. Fed one decode-token duration at a time,
/// it runs the state machine: warm-up (discard the first few tokens — pipeline fill, pipeline compiles),
/// then a window with levers OFF, then a window with levers ON, then decides. Median (not mean) per phase so
/// one GC/scheduler hiccup can't skew it. Decides ON only if ON is meaningfully faster (beats OFF by a
/// margin), so noise never flips it on when it doesn't help. Entirely testable without a GPU.
#[derive(Clone, Debug)]
pub struct FastAb {
    warmup: usize,           // tokens to discard before measuring (pipeline fill)
    window: usize,           // tokens to time in each phase
    seen: usize,             // total tokens observed
    off_ms: Vec<f32>,        // per-token ms with levers OFF
    on_ms: Vec<f32>,         // per-token ms with levers ON
    margin: f32,             // ON must be at least this fraction faster to win (e.g. 0.05 = 5%)
    decided: Option<bool>,   // Some(true)=ON, Some(false)=OFF once resolved
}

impl Default for FastAb {
    fn default() -> Self { FastAb { warmup: 3, window: 8, seen: 0, off_ms: Vec::new(), on_ms: Vec::new(), margin: 0.05, decided: None } }
}

/// What phase the A/B wants the levers in for the NEXT token (drives FAST_AB_STATE).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AbPhase { Warmup, MeasuringOff, MeasuringOn, DecidedOn, DecidedOff }

impl FastAb {
    pub fn new(warmup: usize, window: usize, margin: f32) -> Self {
        FastAb { warmup, window, margin, ..Default::default() }
    }
    fn median(v: &[f32]) -> f32 {
        if v.is_empty() { return f32::INFINITY; }
        let mut s = v.to_vec(); s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        s[s.len() / 2]
    }
    /// The phase the NEXT token should run in (before observing it). The harness sets FAST_AB_STATE from
    /// this so the token is generated in the right lever configuration, then calls `observe` with its time.
    pub fn phase(&self) -> AbPhase {
        if let Some(d) = self.decided { return if d { AbPhase::DecidedOn } else { AbPhase::DecidedOff }; }
        if self.seen < self.warmup { return AbPhase::Warmup; }
        if self.off_ms.len() < self.window { return AbPhase::MeasuringOff; }
        if self.on_ms.len() < self.window { return AbPhase::MeasuringOn; }
        // both windows full but not yet folded into a decision — decide now (see resolve)
        AbPhase::MeasuringOn
    }
    /// Record one decode token's wall-time (ms). Returns Some(decision) on the token that resolves the A/B.
    pub fn observe(&mut self, ms: f32) -> Option<bool> {
        if self.decided.is_some() { return None; }
        let phase = self.phase();
        self.seen += 1;
        match phase {
            AbPhase::Warmup => {}
            AbPhase::MeasuringOff => self.off_ms.push(ms),
            AbPhase::MeasuringOn => self.on_ms.push(ms),
            _ => {}
        }
        if self.off_ms.len() >= self.window && self.on_ms.len() >= self.window {
            let (off, on) = (Self::median(&self.off_ms), Self::median(&self.on_ms));
            // ON wins only if it beats OFF by the margin (lower ms = faster).
            let decision = on < off * (1.0 - self.margin);
            self.decided = Some(decision);
            return Some(decision);
        }
        None
    }
    pub fn decided(&self) -> Option<bool> { self.decided }
    /// Median ms of each phase (for logging the measured delta). (off, on) — INFINITY if a phase is empty.
    pub fn medians(&self) -> (f32, f32) { (Self::median(&self.off_ms), Self::median(&self.on_ms)) }
}

/// Push a FastAb phase into the process-global FAST_AB_STATE the lever flags read. Called by the harness
/// BEFORE generating each measured token so the token runs in the intended configuration.
pub fn ab_apply_phase(p: AbPhase) {
    let s = match p {
        AbPhase::Warmup | AbPhase::MeasuringOff => AB_MEASURING_OFF, // levers OFF
        AbPhase::MeasuringOn => AB_MEASURING_ON,                     // levers ON
        AbPhase::DecidedOn => AB_DECIDED_ON,
        AbPhase::DecidedOff => AB_DECIDED_OFF,
    };
    FAST_AB_STATE.store(s, std::sync::atomic::Ordering::Relaxed);
}

/// Force the A/B to a resolved state (used when a persisted decision is loaded, so a returning model skips
/// re-measuring). true → levers ON, false → OFF.
pub fn ab_set_decided(on: bool) {
    FAST_AB_STATE.store(if on { AB_DECIDED_ON } else { AB_DECIDED_OFF }, std::sync::atomic::Ordering::Relaxed);
}
/// Whether the A/B has already resolved (so the harness stops feeding it timings).
pub fn ab_is_decided() -> bool {
    matches!(FAST_AB_STATE.load(std::sync::atomic::Ordering::Relaxed), AB_DECIDED_ON | AB_DECIDED_OFF)
}

fn fast_decode_enabled() -> bool {
    dispatch_levers_active()
}
fn bind_group_cache_enabled() -> bool {
    fast_decode_enabled() || std::env::var("LOGSCOPE_BG_CACHE").as_deref() == Ok("1")
}
/// Fuse the MoE gate‖up matvec with SwiGLU into one kernel (one fewer dispatch per MoE layer, no 2ff
/// scratch). Bit-identical to the unfused path; enabled by the magical A/B (or LOGSCOPE_FUSE_GLU=1).
fn moe_glu_fused() -> bool {
    fast_decode_enabled() || std::env::var("LOGSCOPE_FUSE_GLU").as_deref() == Ok("1")
}
/// Fuse the MoE down-projection with the weighted combine + residual into one kernel (one fewer dispatch
/// per MoE layer, no [slots,d] scratch). Bit-identical to the unfused path; enabled by the A/B (or _DOWN=1).
fn moe_down_fused() -> bool {
    fast_decode_enabled() || std::env::var("LOGSCOPE_FUSE_DOWN").as_deref() == Ok("1")
}
/// Fused RMSNorm+matvec for the m=1 decode norm→matmul pairs (LOGSCOPE_FUSE_NORM=0 → the explicit
/// two-dispatch rmsnorm then matvec). One fewer dispatch per pair, twice per layer.
fn fuse_norm_enabled() -> bool {
    std::env::var("LOGSCOPE_FUSE_NORM").as_deref() != Ok("0")
}

/// Whether ROUTE_TRACE is armed (env set + v1 path forced so routing is CPU-visible). Cached once.
fn route_trace_armed() -> bool {
    use std::sync::OnceLock;
    static ARMED: OnceLock<bool> = OnceLock::new();
    *ARMED.get_or_init(|| std::env::var("LOGSCOPE_ROUTE_TRACE").is_ok())
}

/// ROUTE_TRACE writer (colibrì borrow, measurement only). Appends one line per (token, layer):
/// `<call> <layer> <tokid> <e0> <e1> …` — the routed-expert ids the CPU router just selected, plus the
/// vocab token id (or -1 if unknown) so an offline tool can measure CONTEXT-CONDITIONED prediction (the
/// lexical map) on top of one-layer-ahead coupling. Opened once; guarded so a trace-off run pays nothing.
fn route_trace(layer: i32, experts: &[u32]) {
    use std::io::Write;
    use std::sync::OnceLock;
    static FP: OnceLock<Option<std::sync::Mutex<std::fs::File>>> = OnceLock::new();
    let slot = FP.get_or_init(|| {
        std::env::var("LOGSCOPE_ROUTE_TRACE").ok().and_then(|p| {
            std::fs::File::create(&p).ok().map(std::sync::Mutex::new)
        })
    });
    if let Some(m) = slot {
        static CALL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let call = CALL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut f) = m.lock() {
            let mut line = format!("{call} {layer}");
            for e in experts { line.push_str(&format!(" {e}")); }
            let _ = writeln!(f, "{line}");
        }
    }
}
/// Route-trace writer that also records the current token id (the lexical signal). `tok` is the vocab id,
/// or a negative value when unknown. Trace grammar: `<call> <layer> <tok> <e0> <e1> …`.
fn route_trace_tok(layer: i32, tok: i64, experts: &[u32]) {
    use std::io::Write;
    use std::sync::OnceLock;
    static FP: OnceLock<Option<std::sync::Mutex<std::fs::File>>> = OnceLock::new();
    let slot = FP.get_or_init(|| {
        std::env::var("LOGSCOPE_ROUTE_TRACE").ok().and_then(|p| {
            std::fs::File::create(&p).ok().map(std::sync::Mutex::new)
        })
    });
    if let Some(m) = slot {
        static CALL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let call = CALL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut f) = m.lock() {
            let mut line = format!("{call} {layer} {tok}");
            for e in experts { line.push_str(&format!(" {e}")); }
            let _ = writeln!(f, "{line}");
        }
    }
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
        // Catch OTHERWISE-FATAL device errors (notably OOM: "Failed to allocate BO VMA" on the tight
        // Apple-Vulkan budget) here instead of letting wgpu's default handler panic the whole process.
        // The handler flips shared flags the decode loop checks so it can abort THIS step and degrade,
        // turning a crash into a recoverable "out of memory, backing off" event. Seen on a q4-30B MoE
        // second-prompt prefill where per-layer expert scratch piled up in one unflushed window submit.
        let degrade_level = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let oom_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let dl = degrade_level.clone(); let of = oom_flag.clone();
            device.on_uncaptured_error(std::sync::Arc::new(move |e: wgpu::Error| {
                let msg = format!("{e}");
                if matches!(e, wgpu::Error::OutOfMemory { .. }) || msg.contains("Out of Memory") || msg.contains("VMA") {
                    of.store(true, std::sync::atomic::Ordering::Relaxed);
                    dl.fetch_max(1, std::sync::atomic::Ordering::Relaxed);
                    eprintln!("wgpu-engine: DEVICE OOM caught (VMA exhausted) — degrading + backing off this step instead of crashing. Free VRAM or lower context/window.");
                } else {
                    // non-OOM validation/internal errors: log loudly (these were the old default-panic set)
                    eprintln!("wgpu-engine: device error (non-fatal, logged): {msg}");
                }
            }));
        }
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
        // Ceiling raised to 65536: windowed sampled decode legitimately wants a whole window
        // (k·~36 dispatches) in ONE submit, since on Metal encoder setup — not GPU sync — is the
        // wall. A pinned FLUSH_EVERY=8192 used to be silently clamped to 1024 (≈28 tokens), so a
        // 128-window still flushed ~5×; the higher ceiling lets the window actually be one submit.
        STALL_MS.store(std::env::var("LOGSCOPE_STALL_MS").ok()
            .and_then(|v| v.parse::<u64>().ok()).unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
        PROGRESS_EVERY.store(std::env::var("LOGSCOPE_PROGRESS_EVERY").ok()
            .and_then(|v| v.parse::<u64>().ok()).unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
        let flush_every = std::sync::atomic::AtomicU32::new(
            std::env::var("LOGSCOPE_FLUSH_EVERY").ok().and_then(|v| v.parse::<u32>().ok()).map(|n| n.clamp(1, 65536)).unwrap_or(FLUSH_EVERY_DEFAULT));
        let inflight_cap = std::sync::atomic::AtomicU32::new(
            std::env::var("LOGSCOPE_SUBMIT_INFLIGHT").ok().and_then(|v| v.parse::<u32>().ok()).map(|n| n.min(64)).unwrap_or(SUBMIT_INFLIGHT_DEFAULT));
        // Governor: map the wgpu backend into the governor's budget model so the windowed-decode k is
        // tuned to THIS device's ceiling (Metal buffer-objects / Vulkan allocation-count / …) instead of a
        // hardcoded guess. apple_vulkan (conservative_vma) = the tight Honeykrisp path.
        let governor = {
            let gb = match backend {
                wgpu::Backend::Metal => crate::governor::WgpuBackend::Metal,
                wgpu::Backend::Vulkan => crate::governor::WgpuBackend::Vulkan,
                wgpu::Backend::Dx12 => crate::governor::WgpuBackend::Dx12,
                wgpu::Backend::Gl => crate::governor::WgpuBackend::Gl,
                _ => crate::governor::WgpuBackend::Other,
            };
            Mutex::new(crate::governor::Governor::new(crate::governor::Budget::for_backend(gb, unified, conservative_vma)))
        };
        let gpu = Self { device, queue, adapter_name, pipelines: Mutex::new(HashMap::new()), frame: Mutex::new(None), profile, profile_forced, shape_tiles: std::sync::RwLock::new(std::collections::HashMap::new()), gemm_tiles: std::sync::RwLock::new(std::collections::HashMap::new()), flush_every, inflight_cap, pending_submits: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)), governor_dead: std::sync::atomic::AtomicBool::new(false), dims_cache: Mutex::new(HashMap::new()), bg_cache: Mutex::new(HashMap::new()), pool: std::sync::Arc::new(BufferPool::default()), arena: Mutex::new(None), pending_q4k: Mutex::new(Vec::new()), moe_stage: Mutex::new(None), stage_cursor: Mutex::new(0), subgroups, unified, sg_ok: std::sync::atomic::AtomicBool::new(false), gemm_ok: std::sync::atomic::AtomicBool::new(false), valid_gemm_tiles: std::sync::RwLock::new(Vec::new()), rope_neox: std::sync::atomic::AtomicBool::new(false), rope_mscale: std::sync::atomic::AtomicU32::new(1.0f32.to_bits()), conservative_vma, trace_layer: std::sync::atomic::AtomicI32::new(-1), trace_tok: std::sync::atomic::AtomicI64::new(-1), pending_keep: Mutex::new(Vec::new()), dims_ring: Mutex::new((Vec::new(), 0, 0)), governor, dims_alloc_failed: std::sync::atomic::AtomicBool::new(false), degrade_level, oom_flag };
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
            // Asahi/Honeykrisp (Apple GPU on Linux via Vulkan) advertises the SUBGROUP feature, but its
            // subgroup support is incomplete enough that the subgroup mat-vec kernel can HANG the GPU on
            // large projections (e.g. the n=128256 output matmul) — the firmware watchdog then times out
            // and halts the GPU ("GPU timeout" in dmesg). The tiny load-time self-check passes (it's a
            // correctness check, not a scale/occupancy check), so it doesn't catch this. Be conservative:
            // on this driver, skip the subgroup kernel by default and use the barrier-reduction kernel,
            // which is robust here. LOGSCOPE_MATVEC_SG=1 forces it back on for experimentation.
            let asahi_vulkan = conservative_vma; // set above iff Vulkan + adapter name contains "Apple"
        // MOBILE GPUs TOO. This guard was keyed on Apple-Vulkan alone, so an Adreno or Mali took the
        // subgroup path with no check that the driver actually provides enough invocations per
        // workgroup. Reported from a Fairphone: a load failure about "not enough wavelets" — a driver
        // term for waves/subgroups, and not a string in this codebase, so it came from the driver
        // refusing a workgroup that demanded more subgroups than it can schedule.
        //
        // Detected by ADAPTER NAME rather than by a feature bit, because there is no bit for "this is a
        // phone": the subgroup extension is present and honest, and the limit is on how many the
        // hardware runs at once. Names are a blunt instrument and a false positive here costs a slower
        // kernel, while a false negative costs a load failure the user cannot diagnose.
        let mobile_gpu = {
            let n = gpu.adapter_name.to_ascii_lowercase();
            ["adreno", "mali", "powervr", "immortalis", "xclipse", "vivante"]
                .iter().any(|m| n.contains(m))
        };
        let asahi_vulkan = asahi_vulkan || mobile_gpu;
            let force_sg = std::env::var_os("LOGSCOPE_MATVEC_SG").is_some();
            // The host may also disable it via EngineConfig (from a tuning profile). The env var still
            // wins: a knob exists to be turned during a bisection, and a config file that silently
            // overrode it would make that impossible.
            let cfg_off = !engine_config().subgroup_matvec;
            if (asahi_vulkan || cfg_off) && !force_sg {
                let why = if mobile_gpu { "mobile GPUs often cannot schedule enough subgroups per workgroup for this kernel" }
                else if asahi_vulkan { "Asahi/Honeykrisp subgroup support can hang large matmuls" }
                          else { "disabled by engine config" };
                eprintln!("wgpu-engine: subgroup decode kernel DISABLED on {} ({why}; LOGSCOPE_MATVEC_SG=1 re-enables)", gpu.adapter_name);
                gpu.sg_ok.store(false, std::sync::atomic::Ordering::Relaxed);
            } else {
                let ok = gpu.self_check_sg();
                gpu.sg_ok.store(ok, std::sync::atomic::Ordering::Relaxed);
                if !ok { eprintln!("wgpu-engine: subgroup decode kernel failed its load-time self-check on {} — using the barrier-reduction kernel instead", gpu.adapter_name); }
            }
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
    /// Does the subgroup matvec agree with the reference kernel on THIS driver?
    ///
    /// Checked at MODEL SCALE, not at a token size. The original check used k=256, n=128 and passed on a
    /// driver whose subgroup kernel is wrong by rel 2.5 at k=2048 — because a small k fits in one
    /// subgroup-reduction step and never exercises the cross-wave accumulation where the bug lives. A
    /// self-check that cannot fail on a broken driver is worse than none: it converts "unknown" into a
    /// false "verified", and the engine then enables a kernel that silently corrupts every projection.
    ///
    /// The sizes below are a real attention projection and a real FFN width, so a driver that passes has
    /// been asked the question that matters. Two shapes rather than one: the failure observed was
    /// size-dependent, so a single shape could still be lucky.
    fn self_check_sg(&self) -> bool {
        for (k, n) in [(2048usize, 2048usize), (8192, 1024)] {
            let mut rng = 0x00C0_FFEE_1234_5678u64;
            let mut rnd = || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; (rng >> 40) as f32 / (1u64 << 24) as f32 - 0.5 };
            let w: Vec<f32> = (0..n * k).map(|_| rnd()).collect();
            let act: Vec<f32> = (0..k).map(|_| rnd()).collect();
            let qw = self.quantize_q8_resident(&w, n, k);
            let a = self.upload(&act, 1, k);
            let base = self.download(&self.matvec_decode_v1(&a, &qw, 1));
            let sg = self.download(&self.matvec_decode_v2sg(&a, &qw, 4));
            if base.len() != sg.len() || base.is_empty() { return false; }
            let scale = base.iter().map(|x| x.abs()).fold(0.0f32, f32::max).max(1e-6);
            let worst = base.iter().zip(&sg).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
            if !(worst / scale < 1e-3) {
                eprintln!("wgpu-engine: subgroup matvec self-check FAILED at k={k} n={n} \
(rel {:.2e}) — using the barrier-reduction kernel instead", worst / scale);
                return false;
            }
        }
        true
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
        // TAGGED: a readback: copy_buffer_to_buffer into staging. Six of eight submit callers incremented nothing, which is
        // where the profile's "N UNATTRIBUTED, a submit path is still untagged" came from.
        SUBMIT_BY_DOWNLOAD.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.submit(enc);
        let slice = staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        dev.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = slice.get_mapped_range();
        let bytes = data.to_vec();
        drop(data); staging.unmap();
        self.clear_retained();
        bytes
    }

    fn run_f32(&self, shader: &str, inputs: &[&[u8]], out_len: usize, dims: &[u8], wg: (u32, u32, u32)) -> Vec<f32> {
        let bytes = self.run(shader, inputs, (out_len * 4) as u64, dims, wg);
        bytemuck::cast_slice(&bytes).to_vec()
    }

    /// As `run_f32` but the output is read back as raw u32 words — needed when the buffer holds packed
    /// bit patterns (quantized nibbles) rather than numbers, where a round-trip through f32 could alter
    /// NaN payloads.
    fn run_u32(&self, shader: &str, inputs: &[&[u8]], out_len: usize, dims: &[u8], wg: (u32, u32, u32)) -> Vec<u32> {
        let bytes = self.run(shader, inputs, (out_len * 4) as u64, dims, wg);
        bytemuck::cast_slice(&bytes).to_vec()
    }

    /// Convert from a PRE-UPLOADED source buffer — no allocation, no upload. The inference-path twin of
    /// `q4k_convert_into`.
    ///
    /// This exists because the allocating version must never be called during decode. Every fill would
    /// create a fresh source buffer, and on a VMA-constrained driver a few thousand of those exhaust the
    /// address space and the load dies with "Failed to allocate BO VMA" mid-generation. The memory spec's
    /// rule — never allocate during inference — is not a style preference here; it is the difference
    /// between running and crashing.
    pub fn q4k_convert_from(&self, src: &Tensor, out: &Tensor, n: usize, k: usize,
                            qw_base_words: usize, qs_base_words: usize) {
        let nblk = n * k / 256;
        let (gx, gy) = wrapped_grid(nblk);
        let dims = [nblk as u32, qs_base_words as u32, qw_base_words as u32, gx];
        self.dispatch(Q4K_TO_Q4, &[src], out, bytemuck::cast_slice(&dims), (gx, gy, 1));
    }

    /// How many fills the staging ring holds before it must wrap.
    ///
    /// This is the renderer's answer to the problem _181 solved with a submit per fill. A single staging
    /// buffer forces `end_frame()` before and after every fill — because queued writes all land before any
    /// command buffer in a submit, so the next fill's bytes would overwrite this fill's source. That cost
    /// TWO submits per fill: at 12 cold experts across 48 layers it is 1152 submits/token, which is
    /// exactly what the profile measured.
    ///
    /// A RING removes the conflict instead of serialising around it: consecutive fills write to different
    /// regions, so many fills ride in one submit and `end_frame` is needed only when the ring wraps.
    /// Games do this for every streaming upload; it is the same reason a renderer keeps N frames of
    /// uniform storage rather than one.
    pub const STAGE_RING: usize = 8;

    /// The shared, persistent upload staging buffer for expert fills: allocated once, grown only if some
    /// layer needs more, and reused for every fill thereafter. Decode is sequential over layers, so one
    /// buffer serves the whole model rather than one per layer.
    pub fn moe_stage(&self, bytes: usize) -> Tensor {
        let mut g = self.moe_stage.lock().unwrap();
        let need = bytes.div_ceil(4);
        let big_enough = g.as_ref().map(|t: &Tensor| t.cols >= need).unwrap_or(false);
        if !big_enough {
            *g = Some(self.alloc_words(need.max(1)));
        }
        let t = g.as_ref().unwrap();
        Tensor { buf: t.buf.clone(), rows: 1, cols: t.cols, pool: None, view: None }
    }

    /// Record a Q4_K conversion to run later. Returns immediately with the (as yet unwritten) output
    /// buffer, so callers can build weights that reference it; `flush_q4k` fills them before load returns.
    pub fn q4k_defer(&self, out: &Tensor, raw: &'static [u8], n: usize, k: usize,
                     qw_base_words: usize, qs_base_words: usize) {
        self.pending_q4k.lock().unwrap().push(PendingQ4k {
            raw, out: out.buf.clone(), n, k, qw_base_words, qs_base_words,
        });
    }

    pub fn pending_q4k_count(&self) -> usize { self.pending_q4k.lock().unwrap().len() }

    /// Run every recorded conversion, batched into as few submits as the source budget allows.
    ///
    /// Two things happen here that cannot happen inline. First the arena's open slab is SEALED (unmapped),
    /// because a submit is illegal while one is mapped — this is what lets arena packing and GPU conversion
    /// coexist instead of one having to be turned off. Second, many dispatches share one command encoder
    /// and one submit: on a driver where firmware submission is the dominant cost, converting 300 tensors
    /// in a handful of submits rather than 300 is the whole point.
    ///
    /// Source uploads are held alive until their submit completes, so batches are bounded by bytes rather
    /// than count.
    pub fn flush_q4k(&self) -> usize {
        let pending: Vec<PendingQ4k> = std::mem::take(&mut *self.pending_q4k.lock().unwrap());
        if pending.is_empty() { return 0; }
        // seal the arena: no submit is legal while a slab is mapped
        if let Ok(mut g) = self.arena.lock() {
            if let Some(a) = g.as_mut() {
                if a.mapped {
                    if let Some(last) = a.slabs.last() { last.unmap(); }
                    a.mapped = false;
                    a.used = a.slab_size; // any later allocation starts a fresh slab
                }
            }
        }
        // Encode every conversion through the normal dispatch path: the engine's frame batching already
        // groups passes into submits (the autotuned submit batch), so this inherits that rather than
        // hand-rolling an encoder — hundreds of conversions leave as a handful of submits.
        // CHUNKED, and this is not optional: a single tensor can be enormous — Scout's token embedding is
        // 202048x5120, over 4 MILLION super-blocks — and one dispatch that large trips the GPU watchdog.
        // The driver kills the job and the log fills with timeouts. Super-blocks are independent and map
        // linearly onto the output, so a chunk is just an offset into src plus shifted qw/qs bases.
        let chunk_blocks: usize = engine_config().convert_chunk_blocks.max(1);
        const CHUNKS_PER_SUBMIT: usize = 8;  // bound how much work rides on one submit
        let mut done = 0usize;
        let mut since_submit = 0usize;
        for p in pending {
            let total = p.n * p.k;
            let nblk = total / 256;
            let mut b0 = 0usize;
            while b0 < nblk {
                let cnt = chunk_blocks.min(nblk - b0);
                let byte0 = b0 * 144;
                let need = cnt * 144;
                let avail = p.raw.len().saturating_sub(byte0);
                let mut padded = p.raw[byte0..byte0 + need.min(avail)].to_vec();
                padded.resize(need.div_ceil(4) * 4, 0);
                let words: Vec<u32> = padded.chunks_exact(4)
                    .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
                let src = self.upload_u32_resident(&words);
                let (gx, gy) = wrapped_grid(cnt);
                // 32 qw words and 8 qs scales per super-block
                let dims = [cnt as u32,
                            (p.qs_base_words + b0 * 8) as u32,
                            (p.qw_base_words + b0 * 32) as u32,
                            gx];
                // the bind group holds the source buffer alive past this scope, so no manual keep-list
                let out_t = Tensor { buf: p.out.clone(), rows: 1, cols: 1, pool: None, view: None };
                self.dispatch(Q4K_TO_Q4, &[&src], &out_t, bytemuck::cast_slice(&dims), (gx, gy, 1));
                b0 += cnt;
                since_submit += 1;
                if since_submit >= CHUNKS_PER_SUBMIT {
                    self.end_frame();
                    crate::counted_poll(&self.device);
                    since_submit = 0;
                }
            }
            done += 1;
        }
        self.end_frame();
        crate::counted_poll(&self.device);
        done
    }

    /// Is an arena slab currently mapped? While one is, NO submit is legal — wgpu rejects
    /// `Queue::submit` with "Buffer with 'arena-slab' label is still mapped". The arena's own contract says
    /// nothing inside `load()` dispatches on arena weights; the GPU Q4_K conversion broke that assumption,
    /// so it asks first and falls back to the CPU route for the duration rather than emitting submits the
    /// driver discards (which would leave the converted buffer UNWRITTEN — silent garbage weights).
    pub fn arena_mapped(&self) -> bool {
        self.arena.lock().ok().and_then(|g| g.as_ref().map(|a| a.mapped)).unwrap_or(false)
    }

    /// Convert raw Q4_K bytes straight into our resident q4 layout ON THE GPU, returning the qw‖qs blob as
    /// words (`qs` starts at word `n*k/8`). No host dequantization and no f32 intermediate — this is what
    /// lets the loader stop building a ~50 GiB host copy and lets an expert slot be filled from mmap'd
    /// file bytes. `k` must be a multiple of 256 (one Q4_K super-block).
    pub fn q4k_to_q4_words(&self, raw: &[u8], n: usize, k: usize) -> Vec<u32> {
        assert!(k % 256 == 0, "q4_k conversion needs k % 256 == 0 (got {k})");
        let total = n * k;
        let nblk = total / 256;
        let need = nblk * 144;
        let mut padded = raw[..need.min(raw.len())].to_vec();
        padded.resize(need.div_ceil(4) * 4, 0);
        let (qw_words, qs_words) = (total / 8, total / 32);
        let (gx, gy) = wrapped_grid(nblk);
        let dims = [nblk as u32, qw_words as u32, 0u32, gx]; // qw_base = 0
        self.run_u32(Q4K_TO_Q4, &[bytemuck::cast_slice(&padded)], qw_words + qs_words,
                     bytemuck::cast_slice(&dims), (gx, gy, 1))
    }

    /// As `q4k_to_q4_words` but the result never leaves the GPU: raw Q4_K bytes go up, the converted q4
    /// weights stay in device memory, and the returned `Q4Weight` is two views into that one buffer.
    ///
    /// This is the piece that removes the load pass. The old route was fetch → CPU dequantize to f32 →
    /// CPU requantize → upload, which for Scout is ~50 GiB of host copies and many minutes of CPU. Here the
    /// only host traffic is the file's own bytes, which are already mmap'd.
    ///
    /// The qs region is placed on a 256-byte boundary so it can be bound as a storage view; the shader is
    /// told where it starts, so alignment padding costs a few bytes and no special-casing.
    /// Convert a raw expert tensor of EITHER Q4_K or Q6_K into the resident q4 layout.
    ///
    /// ## Why this unblocks every Q4_K_M model
    ///
    /// `Q4_K_M` is a MIXTURE: most tensors are Q4_K, and `ffn_down` is Q6_K. The file-view path checked
    /// `ggml_type != 12` and rejected the whole model on that one tensor, falling back to the host-copy
    /// path — which dequantizes and requantizes every expert into RAM. On a 64 GiB machine that host
    /// store was 15.2 GiB and the load was OOM-killed.
    ///
    /// So a single stale type check cost the entire zero-copy path for every model in common use.
    ///
    /// Q6_K goes through `q6k_dequant` — bit-exact against the CPU reference since the gate went green
    /// — and then the same quantizer the resident path already uses. That is two passes rather than
    /// one, paid ONCE at load, against a host copy paid in RAM for the whole session.
    pub fn raw_expert_to_q4_resident(&self, raw: &[u8], ggml_type: u32, n: usize, k: usize)
        -> Option<Q4Weight> {
        match ggml_type {
            12 => Some(self.q4k_to_q4_resident(raw, n, k)),
            14 => {
                // Q6_K: dequantize exactly, then quantize to the resident layout.
                let f = self.download(&self.q6k_dequant(raw, n * k).ok()?);
                Some(self.quantize_q4_resident(&f, n, k))
            }
            // Anything else is refused rather than guessed. Reading a type we have not verified would
            // produce plausible weights and a quietly worse model — the failure this codebase keeps
            // paying for.
            _ => None,
        }
    }

    /// Bytes one tensor of `count` weights occupies in its source format.
    ///
    /// Q4_K is 144 bytes per 256-weight super-block; Q6_K is 210. The file-view slicing was hardcoded
    /// to 144, so a Q6_K tensor would have been sliced at the wrong stride even if the type check had
    /// let it through — the two changes are only useful together.
    /// Bytes occupied by `count` weights of a K-quant source type.
    ///
    /// **None when `count` is not a whole number of 256-weight blocks.** The integer division
    /// otherwise truncates silently, and this figure is used as a PER-EXPERT STRIDE — so a short
    /// stride starts expert 1 partway into expert 0's data and every expert after it drifts further.
    /// The bytes still decode, the shapes are still right, and the output is gibberish.
    ///
    /// Qwen3-Coder is safe by luck rather than by check: its ff is 5472, which is 21.375 blocks, and
    /// only `d = 2048` makes `ff * d` divisible. A model with an odd ff and an odd d would truncate,
    /// and nothing would have said so.
    ///
    /// Refusing is right here because every caller can act on it — the loader has a type-aware path
    /// and the planner wants a size it can trust. A wrong number is worse than no number when the
    /// number is an offset.
    pub fn source_block_bytes(ggml_type: u32, count: usize) -> Option<usize> {
        if count % 256 != 0 { return None; }
        match ggml_type {
            // Q4_K and Q6_K only. Q5_K is NOT here on purpose: a test asserts an unverified source
            // type has no block size, because this figure feeds the file-VIEW path and a size for a
            // layout nobody has read end-to-end grants a capability rather than measuring one.
            //
            // I added Q5_K here and the test caught it — mixed presets do promote tensors to Q5_K, so
            // the number is *correct*, and correct is not the bar. The bar is verified.
            12 => Some(count / 256 * 144),   // Q4_K
            14 => Some(count / 256 * 210),   // Q6_K
            _ => None,
        }
    }

    pub fn q4k_to_q4_resident(&self, raw: &[u8], n: usize, k: usize) -> Q4Weight {
        assert!(k % 256 == 0, "q4_k conversion needs k % 256 == 0 (got {k})");
        let total = n * k;
        let nblk = total / 256;
        let need = nblk * 144;
        let mut padded = raw[..need.min(raw.len())].to_vec();
        padded.resize(need.div_ceil(4) * 4, 0);
        let words: Vec<u32> = padded.chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let src = self.upload_u32_resident(&words);
        let qw_words = total / 8;
        let qs_words = total / 32;
        // round the qw region up to 256 bytes (64 words) so the qs view is bindable
        let qs_off_words = (qw_words * 4).div_ceil(256) * 256 / 4;
        // resident weights, NOT scratch: qw/qs views below outlive this handle, so the buffer must
        // never return to the pool (see alloc_resident_words)
        let out = self.alloc_resident_words(qs_off_words + qs_words);
        let (gx, gy) = wrapped_grid(nblk);
        let dims = [nblk as u32, qs_off_words as u32, 0u32, gx]; // qw_base = 0
        self.dispatch(Q4K_TO_Q4, &[&src], &out, bytemuck::cast_slice(&dims), (gx, gy, 1));
        let qw = Tensor { buf: out.buf.clone(), rows: n, cols: k, pool: None,
                          view: Some((0, (qw_words * 4) as u64)) };
        let qs = Tensor { buf: out.buf.clone(), rows: n, cols: k / 32, pool: None,
                          view: Some(((qs_off_words * 4) as u64, (qs_words * 4) as u64)) };
        Q4Weight { qw, qs, n, k }
    }

    /// Convert raw Q4_K into an EXISTING output buffer at word offsets `qw_base` / `qs_base`.
    ///
    /// This is what a streamed expert slot needs. An expert's gate‖up matrix is [2ff, d], but the file
    /// stores `ffn_gate_exps` and `ffn_up_exps` as SEPARATE tensors — so a fill converts each half into one
    /// slot buffer, gate into rows 0..ff and up into rows ff..2ff. That works because our q4 layout is
    /// row-major with per-row blocks: concatenating two converted halves row-wise IS the conversion of the
    /// stacked matrix, for both the nibbles and the scales.
    ///
    /// No allocation and no host round-trip — with the file mapped for the process lifetime, the `raw`
    /// slice is a view into the GGUF itself, which is the whole point of Colibri's arrangement.
    /// A raw word-addressed SCRATCH buffer. Pooled: it returns to the free list when dropped.
    pub fn alloc_words(&self, words: usize) -> Tensor { self.alloc(1, words) }

    /// A word-addressed buffer that will be held by RESIDENT WEIGHTS, and must never return to the pool.
    ///
    /// This distinction is not cosmetic. `Tensor::drop` puts a pooled buffer back on the free list, so if
    /// a conversion allocates its output from the pool, hands out `qw`/`qs` VIEWS of it, and then drops
    /// the owning handle, the weight buffer is recycled into the next allocation and the weights are
    /// silently overwritten. The views keep working — they just read whatever was written there next.
    /// That produced fluent garbage on every model taking the GPU Q4_K path.
    pub fn alloc_resident_words(&self, words: usize) -> Tensor {
        let mut t = self.alloc(1, words);
        t.pool = None; // detach from the pool: the weights outlive this handle
        t
    }

    /// Read a word-addressed buffer back as u32s.
    pub fn download_words(&self, t: &Tensor) -> Vec<u32> {
        let f = self.download(t);
        f.iter().map(|v| v.to_bits()).collect()
    }

    pub fn q4k_convert_into(&self, out: &Tensor, raw: &[u8], n: usize, k: usize,
                            qw_base_words: usize, qs_base_words: usize) {
        assert!(k % 256 == 0, "q4_k conversion needs k % 256 == 0 (got {k})");
        let total = n * k;
        let nblk = total / 256;
        let need = nblk * 144;
        let mut padded = raw[..need.min(raw.len())].to_vec();
        padded.resize(need.div_ceil(4) * 4, 0);
        let words: Vec<u32> = padded.chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let src = self.upload_u32_resident(&words);
        let (gx, gy) = wrapped_grid(nblk);
        let dims = [nblk as u32, qs_base_words as u32, qw_base_words as u32, gx];
        self.dispatch(Q4K_TO_Q4, &[&src], out, bytemuck::cast_slice(&dims), (gx, gy, 1));
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
        // If the device already signalled OOM this step, the staging buffer below may fail to allocate and
        // mapping an INVALID buffer is a FATAL wgpu error (get_mapped_range panics past the uncaptured-error
        // handler — seen on the q4-30B long second prompt). Bail early with an empty result; the caller
        // treats empty as "step failed, back off" rather than crashing.
        if self.oom_backoff() { return Vec::new(); }
        let bytes = (t.len() * 4) as u64;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("dl_u32"), size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let _keep = if let Some(mut frame) = self.frame.lock().unwrap().take() {
            frame.enc.copy_buffer_to_buffer(&t.buf, 0, &staging, 0, bytes);
            // TAGGED: a readback, flushing the open frame first. Six of eight submit callers incremented nothing, which is
            // where the profile's "N UNATTRIBUTED, a submit path is still untagged" came from.
            SUBMIT_BY_DOWNLOAD.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.submit(frame.enc);
            self.retain_keep(frame.keep);
            None::<()>
        } else {
            let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            enc.copy_buffer_to_buffer(&t.buf, 0, &staging, 0, bytes);
            // TAGGED: a readback: copy_buffer_to_buffer into staging. Six of eight submit callers incremented nothing, which is
            // where the profile's "N UNATTRIBUTED, a submit path is still untagged" came from.
            SUBMIT_BY_DOWNLOAD.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.submit(enc);
            None
        };
        let slice = staging.slice(..);
        // MEASURED. This poll was recorded by nothing — not GPU_POLLS, not GPU_SYNC_NS — and it is
        // the DECODE loop's readback, so the one wait that dominates decode was the one wait the
        // profile could not see.
        //
        // That is why a real run reported "1 polls" and "sync 0ms of 19860ms" while a separate phase
        // breakdown of the same run measured 15.5% sync: the profile was not wrong about the paths it
        // watched, it was watching the wrong path. Every conclusion drawn from "setup 0.2% accounted"
        // rested on a denominator missing its largest term.
        // The callback SIGNALS rather than discarding its result, so the bounded poll below has
        // something to wait on. Without it there is no way to tell "not finished yet" from
        // "finished while we slept", and the loop would exit on its first pass every time.
        let mapped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let m2 = mapped.clone();
        slice.map_async(wgpu::MapMode::Read, move |_| {
            m2.store(true, std::sync::atomic::Ordering::Release);
        });
        let t_sync = std::time::Instant::now();
        GPU_POLLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // BOUNDED, NOT INDEFINITE. This is the decode loop's readback, and on a driver whose watchdog
        // kills a long dispatch the work never completes — so an indefinite wait blocks the CPU
        // forever on a job the GPU already abandoned. That is a HANG: no error, no output, and a log
        // that simply stops after "windowed sampling ON".
        //
        // Polling in bounded steps turns it into a diagnosable failure. The GPU is not going to
        // finish; the only question is whether anyone finds out.
        const DECODE_POLL_BUDGET_S: u64 = 20;
        let mut waited = std::time::Duration::ZERO;
        let step = std::time::Duration::from_millis(50);
        loop {
            let _ = self.device.poll(wgpu::PollType::Poll);
            if mapped.load(std::sync::atomic::Ordering::Acquire) { break; }
            if waited.as_secs() >= DECODE_POLL_BUDGET_S {
                eprintln!("wgpu-engine: decode readback did not complete within \
{DECODE_POLL_BUDGET_S}s — the GPU has almost certainly had this dispatch killed by its watchdog \
(check `journalctl -b | grep 'GPU timeout'`). Lower LOGSCOPE_PREFILL_CHUNK or the decode window; \
waiting longer will not help because the work is gone");
                break;
            }
            std::thread::sleep(step);
            waited += step;
        }
        GPU_SYNC_NS.fetch_add(t_sync.elapsed().as_nanos() as u64,
                              std::sync::atomic::Ordering::Relaxed);
        let data = slice.get_mapped_range();
        let v = bytemuck::cast_slice::<u8, u32>(&data).to_vec();
        drop(data); staging.unmap();
        self.clear_retained();
        v
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

/// A Q4_K conversion recorded during load and executed later, in a batch.
///
/// Deferring solves two problems at once. The arena keeps a slab MAPPED for most of a load and no submit
/// is legal while it is, so a conversion cannot dispatch inline — recording it instead lets the arena and
/// the GPU converter coexist rather than one disabling the other. And because the work piles up, it can
/// then go out as a handful of large submits instead of one per tensor, which is the difference that
/// matters on a driver where firmware submission dominates.
///
/// `raw` is a view into the retained model file, so a pending entry costs a pointer, not the weights.
pub struct PendingQ4k {
    raw: &'static [u8],
    out: wgpu::Buffer,
    n: usize, k: usize,
    qw_base_words: usize, qs_base_words: usize,
}
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


/// Holds the per-token scratch arena open; releases it on drop. See `Gpu::scratch_scope`.

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
    /// Llama4 Scout: WEIGHTLESS QK-Norm (≡ all-ones weights) on RoPE layers only — the loader
    /// injects ones tensors there and leaves NoPE layers None.
    pub q_norm: Option<Tensor>, pub k_norm: Option<Tensor>,
    /// Llama4 iRoPE: this layer uses NO positional encoding (every 4th layer). Block fns map this
    /// to the rope hosts' base==0.0 identity sentinel, so K enters the cache unroped and Q stays
    /// unrotated — attention itself is unchanged.
    pub nope: bool,
    /// Llama 4 iRoPE attention temperature tuning: on NoPE layers, the query is scaled by a per-position
    /// factor (grows logarithmically past floor_scale) to keep long-context attention sharp. `None` =
    /// disabled (all non-Llama4 archs, and Llama4 RoPE layers). See llama4::AttnTemp.
    pub attn_temp: Option<crate::llama4::AttnTemp>,
    /// Llama-4 `use_kq_norm` (Llama4TextL2Norm): a WEIGHTLESS RMS norm applied to Q and K *after* rope,
    /// on rope layers only. Stored as an all-ones weight so it reuses the existing per-head rmsnorm.
    /// llama.cpp sets this for every Llama-4 except the 128-expert Maverick (src/models/llama4.cpp).
    /// Distinct from `q_norm`/`k_norm`, which are LEARNED weights applied BEFORE rope (the Qwen3 shape) —
    /// both the weight and the ordering differ, which is why this could not reuse them.
    pub kq_norm: Option<Tensor>,
    /// MLA projections for this layer, when the checkpoint ships them (DeepSeek-V3 / GLM-MoE). Present
    /// means the compressed-latent attention path can run for this layer; the GPU forward it feeds is
    /// already validated against the CPU reference by --gate-mla.
    pub mla: Option<crate::mla::MlaWeightsT>,
    /// GELU-gated FFN instead of SiLU-gated (Gemma). Per block so a mixed model would still work.
    pub geglu: bool,
    /// Gemma applies a norm to each sublayer's OUTPUT, before the residual add — distinct from the
    /// pre-norms `g1`/`g2`. `post_attn` follows attention, `post_ffn` follows the FFN.
    pub post_attn: Option<Tensor>,
    pub post_ffn: Option<Tensor>,
    /// Rope frequency base for THIS layer, when it differs from the model's.
    ///
    /// Gemma reads `rope.freq_base_swa` and uses it on windowed layers only, so a model alternating
    /// windowed and full attention rotates them at different frequencies. `None` means use the model's
    /// base — which is every other architecture.
    pub rope_base: Option<f32>,
    /// Sliding-window attention for this layer: (window, rule). None = attend to the whole context.
    /// Set per layer because these architectures alternate windowed and full-attention layers.
    pub swa: Option<(u32, crate::swa::SwaType)>,
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
    /// STREAMING (LOGSCOPE_MOE_STREAM): when set, the routed experts are NOT all resident — cold experts'
    /// q4 bytes live host-side here and are filled into GPU slots on demand at decode. None = fully resident
    /// (unchanged path). Built by the loader only when streaming is enabled and the model is over-ceiling.
    pub stream: Option<std::sync::Mutex<MoeStream>>,
}

/// Host-side streaming store for one MoE layer's routed experts. Holds each expert's already-quantized q4
/// bytes (gate_up + down) host-resident, a GPU slot pool (StreamSlots) sized to hold the cold working set,
/// and the LRU policy (expert_cache::LayerSlots) that decides which slot each cold expert lands in. Leans
/// on Colibri's pin∪LRU∪load tiering: pinned hot experts stay resident in `pinned_gu`/`pinned_dn`; the rest
/// stream through the slots. Filled per token in moe_ffn_v1.
/// Raw Q4_K expert weights as VIEWS into the retained model file — Colibri's arrangement, where an expert
/// is a window into the mmap and "la page cache del kernel È la cache" rather than a copy we own.
///
/// The alternative, which this replaces, is requantizing every expert into host memory at load: ~50 GiB
/// and many minutes for Scout. Here nothing is copied and nothing is converted until a slot actually needs
/// the expert, at which point the conversion happens on the GPU.
///
/// Spans are per-expert slices of the three expert tensors. GGUF stores them as [d, ff, n_expert] (gate/up)
/// and [ff, d, n_expert] (down) with the expert index slowest, so each expert's block is contiguous.
pub struct RawExpertSrc {
    pub gate: &'static [u8],
    pub up: &'static [u8],
    pub down: &'static [u8],
    /// Bytes per expert in gate/up (each an [ff, d] matrix) and in down (a [d, ff] matrix).
    pub gu_expert_bytes: usize,
    pub dn_expert_bytes: usize,
    /// Source ggml type of gate/up, and of down SEPARATELY.
    ///
    /// They differ in practice: Q4_K_M stores gate and up as Q4_K and ffn_down as Q6_K, which is the
    /// mixture that made a single-type check reject every such model.
    pub gu_type: u32,
    pub dn_type: u32,
    pub ff: usize,
    pub d: usize,
}

impl RawExpertSrc {
    /// Byte span of expert `e` in the gate tensor, or None if it escapes the mapping (a missing shard).
    pub fn gate_of(&self, e: usize) -> Option<&'static [u8]> {
        self.gate.get(e * self.gu_expert_bytes..(e + 1) * self.gu_expert_bytes)
    }
    pub fn up_of(&self, e: usize) -> Option<&'static [u8]> {
        self.up.get(e * self.gu_expert_bytes..(e + 1) * self.gu_expert_bytes)
    }
    pub fn down_of(&self, e: usize) -> Option<&'static [u8]> {
        self.down.get(e * self.dn_expert_bytes..(e + 1) * self.dn_expert_bytes)
    }
    /// Slot layout for the gate‖up blob: (qs byte offset, total bytes). qs is 256-aligned so it binds.
    pub fn gu_layout(&self) -> (usize, usize) {
        let qw = 2 * self.ff * self.d / 2;              // 4 bits per weight
        let qs_off = qw.div_ceil(256) * 256;
        (qs_off, qs_off + 2 * self.ff * (self.d / 32) * 4)
    }
    /// Slot layout for the down blob.
    pub fn dn_layout(&self) -> (usize, usize) {
        let qw = self.d * self.ff / 2;
        let qs_off = qw.div_ceil(256) * 256;
        (qs_off, qs_off + self.d * (self.ff / 32) * 4)
    }
}

pub struct MoeStream {
    pub layer: usize,                      // this layer's index — the key for cross-layer coupling prediction
    pub n_expert: usize,
    pub gu_rows: usize, pub gu_k: usize,   // gate_up expert shape [2ff, d]
    pub dn_rows: usize, pub dn_k: usize,   // down expert shape [d, ff]
    // per-expert host q4 bytes (qw ‖ qs concatenated per matrix); indexed [expert] → (gu_bytes, dn_bytes)
    pub host_gu: Vec<Vec<u8>>,
    pub host_dn: Vec<Vec<u8>>,
    pub gu_qs_off: usize,                   // byte offset where qs begins within a gu blob (qw len)
    pub dn_qs_off: usize,
    // GPU slots: 2 per ring entry (gate_up + down). policy tracks expert→slot-index.
    pub gu_slots: StreamSlots,
    pub dn_slots: StreamSlots,
    pub policy: crate::expert_cache::LayerSlots,
    // pinned hot experts resident (Colibri's hot-store): expert id → (qw buffer, qs buffer) for gate_up/down
    pub pinned_gu: std::collections::HashMap<u32, (wgpu::Buffer, wgpu::Buffer)>,
    pub pinned_dn: std::collections::HashMap<u32, (wgpu::Buffer, wgpu::Buffer)>,
    /// REPIN bookkeeping: how many times this layer's pin set has been corrected from observed heat, and
    /// the access counter that gates how often we even look (Colibri checks at a ~16-token safe point).
    pub repins: u64,
    pub visits: u64,
    /// When present, expert weights are VIEWS into the model file and `host_gu`/`host_dn` are unused: a
    /// fill converts straight from the file on the GPU. When None, the legacy host-copy path is used.
    pub raw: Option<RawExpertSrc>,
    /// Pinned experts under the view path: (gate‖up blob, down blob), each a single resident buffer laid
    /// out like a slot. Converted once from the file; a repin re-converts in place, so a pin swap costs one
    /// GPU pass rather than a host round-trip.
    pub pinned_raw: std::collections::HashMap<u32, (wgpu::Buffer, wgpu::Buffer)>,
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
        LinKind::F32(t) => {
            // f32 expert stack (small models, or dims not ÷32 so quant fell back to f32): slice `rows`
            // rows of `k = cols` floats out of the [n, k] tensor as a byte-offset view.
            let k = t.cols;
            Some(Lin { kind: LinKind::F32(tview(t, (row0 * k * 4) as u64, (rows * k * 4) as u64, rows, k)), bias: None })
        }
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

/// The GPU-resident embedding table, either full f32 or int8-quantized (q8). Built by
/// `Gpu::resident_embed`; `gather_row` reads one row by id regardless of format, so the windowed-decode
/// loop is identical in both cases. Q8 is used under MoE streaming to reclaim ~2.9 GiB of VMA budget.
pub enum ResidentEmbed { F32(Tensor), Q8(QWeight) }
impl ResidentEmbed {
    /// Gather the row for the id at `ids[id_off]` → [1, d]. f32 path is byte-identical to the old
    /// `gather_at(&embed_t, …)`; q8 path uses the validated `gather_q8_at` twin.
    pub fn gather_row(&self, gpu: &Gpu, ids: &Tensor, id_off: u32, d: usize) -> Tensor {
        match self {
            ResidentEmbed::F32(t) => gpu.gather_at(t, ids, id_off, d),
            ResidentEmbed::Q8(q) => gpu.gather_q8_at(q, ids, id_off),
        }
    }
}

/// Cross-layer routing history driving speculative expert prefetch — Colibri's `couple_prefetch`
/// (glm.c: "the routing of layer L predicts layer L+1's experts"). One shared object for the whole model:
/// each MoE layer, once its routing is known, OBSERVES (prev-layer experts → this layer's experts) into the
/// coupling table, and the NEXT layer PREDICTS from it before its own router runs.
///
/// A process-global (like Colibri's `g_pilot_*`) rather than a Gpu field: decode is one stream of layers,
/// and this keeps the change off the Gpu struct and its constructors entirely.
pub struct MoeRouteHistory {
    pub map: crate::expert_predict::ExpertMap,
    pub prev_experts: Vec<u32>,   // the previous MoE layer's routed set (the coupling key)
    pub prev_layer: i32,
    pub prefetched: u64,          // speculative fills actually ISSUED (predicted && not already resident)
    pub prefetch_hits: u64,       // of those issued fills, ones the layer then routed to (the fills that paid)
    pub pred_total: u64,          // predictions made (whether or not a fill was needed)
    pub pred_correct: u64,        // of those, ones the layer routed to (raw predictor accuracy)
}

static MOE_ROUTE_HIST: std::sync::OnceLock<std::sync::Mutex<MoeRouteHistory>> = std::sync::OnceLock::new();

/// How often (in per-layer visits ≈ tokens) to consider correcting the pin set. Colibri checks at a
/// ~16-token safe point; a swap costs a full expert-layer re-upload, so checking every token would spend
/// more bandwidth than the better pin set saves.
const REPIN_EVERY: u64 = 16;

/// Live residency telemetry for expert streaming — the numbers that decide whether streaming is
/// affordable, gathered on the real run rather than inferred. Atomics rather than a mutex so they can be
/// touched from inside the per-expert path without any lock-ordering concern.
///
/// `MOE_HITS / (MOE_HITS + MOE_FILLS)` is the hit rate the whole break-even analysis turns on (the M2
/// probe put break-even at >43%), and `MOE_BYTES` is what that costs in bytes actually moved. Printed
/// periodically under LOGSCOPE_MOE_STATS=1.
pub static MOE_FILLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// A LOGICAL POSITION marker, printed periodically so a GPU watchdog entry can be placed in the sequence
/// rather than only on the wall clock.
///
/// The distinction matters and I got it wrong once: a watchdog fires a second or two AFTER the GPU
/// hangs, so the journal timestamp says when the fault was NOTICED, not where the offending work sat.
/// "43% through the run" is compatible with "the first fill of a phase that starts at 43%" — and with
/// only two faults, a boundary is at least as likely as a coincidence. Wall time alone cannot separate
/// them; a periodic (layer, fill, phase) breadcrumb can.
pub static PROGRESS_EVERY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// `HH:MM:SS.mmm` local time, for prefixing engine log lines.
///
/// Every correlation in this project has been done by hand because the engine's own output carries no
/// clock — the diagnostic report has stamps, but the terminal log that people actually paste does not.
/// Local rather than UTC because journalctl is local, and a stamp you have to convert is a stamp you
/// will convert wrongly at least once.
pub fn stamp() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let (secs, ms) = (now.as_secs() as i64 + local_offset_secs(), now.subsec_millis());
    let t = secs.rem_euclid(86400);
    format!("{:02}:{:02}:{:02}.{:03}", t / 3600, (t % 3600) / 60, t % 60, ms)
}

/// Seconds to add to UTC for local time, read once from `date +%z`.
fn local_offset_secs() -> i64 {
    static OFF: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| {
        std::process::Command::new("date").arg("+%z").output().ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                let t = s.trim();
                if t.len() < 5 { return None; }
                let sign = if t.starts_with('-') { -1i64 } else { 1 };
                Some(sign * (t[1..3].parse::<i64>().ok()? * 3600 + t[3..5].parse::<i64>().ok()? * 60))
            }).unwrap_or(0)
    })
}

/// Timestamped engine log line. Use for anything that may need correlating with a kernel log.
#[macro_export]
macro_rules! elog {
    ($($a:tt)*) => { eprintln!("[{}] {}", $crate::stamp(), format!($($a)*)) };
}

/// The last dispatch recorded, as (shader source pointer, workgroups, dims[0]).
///
/// Kept as raw atomics rather than a String because it is written on EVERY dispatch — tens of thousands
/// per token — and anything allocating would change the timing of the thing being measured.
pub static LAST_SHADER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static LAST_WG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static LAST_DIM0: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Dispatches recorded so far, so a stall can be located in the sequence.
pub static DISPATCH_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Stalls observed (a poll that took longer than the threshold).
pub static STALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Milliseconds a device poll may take before it is reported as a stall. 0 disables the check.
///
/// A GPU hang shows up on our side as a poll that takes SECONDS instead of milliseconds — the kernel
/// recovers the device and we carry on, so the only trace we normally keep is a journal line with no
/// idea what was running. Timing the poll ourselves means the fault names the exact operation that
/// caused it, and two runs answer "is it deterministic" without needing the journal at all.
pub static STALL_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Record the dispatch about to be submitted, so a stall can name it.
#[inline]
pub fn mark_dispatch(pipe_id: usize, wg: (u32, u32, u32), dim0: u32) {
    use std::sync::atomic::Ordering::Relaxed;
    if STALL_MS.load(Relaxed) == 0 { return; }
    // The PIPELINE identity, not a name: pipelines are cached for the process lifetime so the value is
    // stable and comparable across a run, and storing it is one atomic write on a path taken tens of
    // thousands of times per token. A String here would allocate and change the timing of the thing
    // being measured.
    LAST_SHADER.store(pipe_id, Relaxed);
    LAST_WG.store(((wg.0 as u64) << 32) | (wg.1 as u64) << 16 | wg.2 as u64, Relaxed);
    LAST_DIM0.store(dim0, Relaxed);
    DISPATCH_SEQ.fetch_add(1, Relaxed);
}

/// Check whether a poll stalled, and if so report exactly what was in flight.
///
/// NOT OBSERVED FIRING in the container: lavapipe's polls on the small gate workloads stay under any
/// useful threshold, so there is nothing here to detect. The wiring is at both blocking polls
/// (flush_and_wait and the download map) and is gated, but "it compiles and is in the right place" is a
/// weaker claim than "it fired", and the difference is worth stating.
///
/// Reports the SEQUENCE NUMBER as well as the shape: "the 41,203rd dispatch" is what makes two runs
/// comparable. If the same sequence number stalls twice, the hang is deterministic and can be bisected;
/// if it moves, it is a race and bisection would waste a day.
pub fn check_stall(started: std::time::Instant, _unused: usize) {
    use std::sync::atomic::Ordering::Relaxed;
    let thresh = STALL_MS.load(Relaxed);
    if thresh == 0 { return; }
    let ms = started.elapsed().as_millis() as u64;
    if ms < thresh { return; }
    STALLS.fetch_add(1, Relaxed);
    let wg = LAST_WG.load(Relaxed);
    elog!("wgpu-engine: STALL {ms} ms at dispatch #{} — wg=({},{},{}) dim0={} pipe={:#x} \
(a poll this long is a GPU hang; the kernel recovers and we continue)",
        DISPATCH_SEQ.load(Relaxed), (wg >> 32) as u32, ((wg >> 16) & 0xFFFF) as u32, (wg & 0xFFFF) as u32,
        LAST_DIM0.load(Relaxed), LAST_SHADER.load(Relaxed));
}

/// Emit a positional breadcrumb every `n` fills. 0 disables. Set from LOGSCOPE_PROGRESS_EVERY.
pub fn progress_tick(phase: &str, layer: usize, extra: &str) {
    use std::sync::atomic::Ordering::Relaxed;
    let every = PROGRESS_EVERY.load(Relaxed);
    if every == 0 { return; }
    let f = MOE_FILLS.load(Relaxed);
    if f % every != 0 { return; }
    elog!("wgpu-engine: PROGRESS {phase} layer={layer} fills={f} bytes={:.1}GiB {extra}",
        MOE_BYTES.load(Relaxed) as f64 / (1u64 << 30) as f64);
}
pub static MOE_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static MOE_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static MOE_REPINS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Reset the residency counters — for a caller that wants per-generation rather than per-process numbers.
pub fn moe_stream_stats_reset() {
    use std::sync::atomic::Ordering::Relaxed;
    MOE_FILLS.store(0, Relaxed); MOE_HITS.store(0, Relaxed);
    MOE_BYTES.store(0, Relaxed); MOE_REPINS.store(0, Relaxed);
}

/// Snapshot of the streaming residency counters: (hits, fills, bytes moved, repins, hit rate).
pub fn moe_stream_stats() -> (u64, u64, u64, u64, f64) {
    use std::sync::atomic::Ordering::Relaxed;
    let (h, f) = (MOE_HITS.load(Relaxed), MOE_FILLS.load(Relaxed));
    let rate = if h + f == 0 { 0.0 } else { h as f64 / (h + f) as f64 };
    (h, f, MOE_BYTES.load(Relaxed), MOE_REPINS.load(Relaxed), rate)
}
pub fn moe_route_hist() -> &'static std::sync::Mutex<MoeRouteHistory> {
    MOE_ROUTE_HIST.get_or_init(|| std::sync::Mutex::new(MoeRouteHistory {
        map: crate::expert_predict::ExpertMap::new(),
        prev_experts: Vec::new(), prev_layer: -1, prefetched: 0, prefetch_hits: 0, pred_total: 0, pred_correct: 0,
    }))
}

/// GPU-side stream slots: the MECHANISM half of MoE expert streaming (the policy half is the pure
/// `expert_cache::LayerSlots`). A fixed pool of resident buffers, each holding one streamed (cold) expert's
/// weight bytes. When the policy returns Miss(slot), `fill(slot, bytes)` writes that expert's q4 bytes into
/// the slot via `write_buffer` — the path the M2 probe measured fastest (~1-2 ms for a 32-64 MiB
/// expert-layer). The slot buffer is then bound like any resident weight for the expert's matvec.
///
/// Deliberately isolated from the live decode path: this is milestone-1 of the streaming mechanism, built
/// and round-trip tested on its own before any wiring into moe_ffn. slot_bytes = one expert-layer at
/// residency (q4); n_slots ≥ the streaming ring (top_k + a prefetch spare).
pub struct StreamSlots {
    slots: Vec<wgpu::Buffer>,
    slot_bytes: u64,
}

impl StreamSlots {
    /// Allocate `n_slots` resident buffers of `slot_bytes` each. Each buffer is STORAGE (bound for the
    /// matvec) + COPY_DST (filled by write_buffer) + COPY_SRC (readback for verification). slot_bytes must
    /// not exceed the device max_buffer_size (an expert-layer is far under it; caller sizes to ~32-64 MiB).
    pub fn new(gpu: &Gpu, n_slots: usize, slot_bytes: u64) -> Self {
        let cap = gpu.device.limits().max_buffer_size;
        let mut slot_bytes = slot_bytes.min(cap).max(4);
        // EXPERIMENTAL (LOGSCOPE_SLOT_PAGE=<KiB>, default off): round each slot up to a page multiple, per
        // the memory spec's "never allocate arbitrary sizes — round upward". The theory is that uniform,
        // page-aligned allocations fragment the driver's VMA less and make slots interchangeable, which
        // matters more here than usual because the Honeykrisp ceiling is a VMA limit rather than a capacity
        // one. Off by default because it is unmeasured on real hardware and costs a little residency
        // (rounding 67.5 MiB up to a 1 MiB multiple wastes ~0.5 MiB per slot); turn it on and watch whether
        // the load ceiling moves.
        if let Ok(kib) = std::env::var("LOGSCOPE_SLOT_PAGE").map(|s| s.parse::<u64>().unwrap_or(0)) {
            if kib > 0 {
                let page = kib * 1024;
                let rounded = slot_bytes.div_ceil(page) * page;
                if rounded <= cap { slot_bytes = rounded; }
            }
        }
        let slots = (0..n_slots).map(|_i| gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("expert-stream-slot"),
            size: slot_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })).collect::<Vec<_>>();
        StreamSlots { slots, slot_bytes }
    }

    pub fn n_slots(&self) -> usize { self.slots.len() }
    pub fn slot_bytes(&self) -> u64 { self.slot_bytes }

    /// Fill slot `idx` with a cold expert's bytes (the M2 fast path: write_buffer). `bytes` is one
    /// expert-layer's q4 weights, mmap'd straight from the GGUF — no intermediate copy. Bytes longer than
    /// the slot are truncated (shouldn't happen: slots are sized to the expert-layer); shorter is fine.
    pub fn fill(&self, gpu: &Gpu, idx: usize, bytes: &[u8]) {
        let Some(buf) = self.slots.get(idx) else { return };
        let n = (bytes.len() as u64).min(self.slot_bytes) as usize;
        if n == 0 { return; }
        gpu.write_chunked(buf, 0, &bytes[..n]);   // expert slot fills are ~68 MiB
    }

    /// The whole slot as a word-addressed Tensor, for a kernel that writes packed data into it.
    pub fn tensor_full(&self, idx: usize) -> Tensor {
        let buf = self.slots[idx.min(self.slots.len().saturating_sub(1))].clone();
        Tensor { buf, rows: 1, cols: (self.slot_bytes / 4) as usize, pool: None, view: None }
    }

    /// The raw slot buffer handle (cloned; wgpu buffers are Arc-backed) for binding as a weight.
    pub fn slot_buf(&self, idx: usize) -> wgpu::Buffer { self.slots[idx.min(self.slots.len().saturating_sub(1))].clone() }

    /// The slot buffer, wrapped as a Tensor view so it binds in a kernel like any resident weight. `rows`,
    /// `cols` describe the expert weight matrix living in the slot.
    pub fn tensor(&self, idx: usize, rows: usize, cols: usize) -> Option<Tensor> {
        let buf = self.slots.get(idx)?.clone();
        Some(Tensor { buf, rows, cols, pool: None, view: None })
    }

    /// Read `len` bytes back from slot `idx` — for verification/tests (fills must land intact).
    pub fn readback(&self, gpu: &Gpu, idx: usize, len: usize) -> Vec<u8> {
        let Some(buf) = self.slots.get(idx) else { return Vec::new() };
        let len = (len as u64).min(self.slot_bytes) as usize;
        let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: Some("slot-readback"),
            size: len.max(4) as u64, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let mut enc = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_buffer_to_buffer(buf, 0, &staging, 0, len as u64);
        gpu.queue.submit(Some(enc.finish()));
        let slice = staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        crate::counted_poll(&gpu.device);
        let data = slice.get_mapped_range().to_vec();
        staging.unmap();
        data
    }
}

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
    /// Is this weight held as resident q4? Used to key tuning verdicts by precision.
    pub fn is_q4(&self) -> bool { matches!(self.kind, LinKind::Q4(_)) }
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

    /// Apply the linear to the RMSNorm of `a` (norm weight `nw`, `eps`) in ONE fused dispatch for the
    /// m=1 Q8 decode case — the norm→matmul pair that fires twice per layer. Any other case (F32/Q4
    /// weight, prefill m>1, biased, or the fusion disabled via LOGSCOPE_FUSE_NORM=0) falls back to an
    /// explicit rmsnorm_t then apply(), so this is a pure speed swap with identical output.
    pub fn norm_apply(&self, gpu: &Gpu, a: &Tensor, nw: &Tensor, cols: usize, eps: f32, m: usize) -> Tensor {
        if m == 1 && self.bias.is_none() && fuse_norm_enabled() {
            if let LinKind::Q8(q) = &self.kind {
                return gpu.linear_q8_0_norm(a, nw, q, cols, eps);
            }
        }
        let n1 = gpu.rmsnorm_t(a, nw, m, cols, eps);
        self.apply(gpu, &n1, m)
    }
}

impl Gpu {
    pub fn upload(&self, data: &[f32], rows: usize, cols: usize) -> Tensor {
        let bytes: &[u8] = bytemuck::cast_slice(data);
        Tensor { buf: self.upload_bytes(bytes, "tensor"), rows, cols, pool: None, view: None }
    }

    /// EXPERT-STREAM PROBE (the M2 go/no-go), corrected. The first cut asked "does the copy overlap
    /// compute?" and used write_buffer — but write_buffer STAGES (copies host→staging belt→device), which on
    /// unified memory is the wrong path to *assume* is slow: MEASURED on the M2 Ultra it's actually the FAST
    /// path at a specific size. Key hardware findings (M2 Ultra / Honeykrisp, expert-layer slot fills):
    ///   * write_buffer @ 64 MiB = ~32 GiB/s, 1.9 ms/fill — a sharp staging-belt sweet spot.
    ///   * write_buffer @ 32/96/128+ MiB = ~10 GiB/s (3× slower) — 64 MiB is special.
    ///   * create(mapped_at_creation)+memcpy+unmap PER SWAP = 4-14 GiB/s — SLOWER than staged, because the
    ///     per-swap buffer CREATION cost dominates the memcpy. (My prediction that this UMA path would hit
    ///     memory bandwidth was WRONG; mapped-at-creation only wins for load-once resident weights, not for
    ///     re-filled streaming slots.) So the streaming mechanism should fill via write_buffer in 64 MiB
    ///     chunks, NOT recreate mapped buffers.
    /// The probe measures both paths so the mechanism picks the empirically-fast one. Returns
    /// (staged_ms, mapped_ms, actual_mib); bandwidth = actual_mib/ms.
    /// Companion to probe_expert_stream: time the COMPUTE side an expert-layer fill hides under — a real
    /// per-expert q4 FFN at decode (batch=1). One expert = gate[ff,d] + up[ff,d] + down[d,ff] matvecs. Given
    /// the model's (d, ff, layers, experts_per_token) this estimates the per-token compute, so the fill cost
    /// from probe_expert_stream can be judged against a MEASURED budget instead of a guessed --token-ms.
    /// Returns ms for ONE expert's 3 matvecs. Scout dims: d=5120, ff=8192.
    pub fn probe_expert_compute(&self, d: usize, ff: usize, iters: usize) -> f64 {
        let gate = self.quantize_q4_resident(&vec![0.05f32; ff * d], ff, d);
        let up = self.quantize_q4_resident(&vec![0.05f32; ff * d], ff, d);
        let down = self.quantize_q4_resident(&vec![0.05f32; d * ff], d, ff);
        let x = self.upload(&vec![0.1f32; d], 1, d);
        let xff = self.upload(&vec![0.1f32; ff], 1, ff);
        let wait = || { self.queue.submit(std::iter::empty()); crate::counted_poll(&self.device); };
        for _ in 0..2 { let _ = self.linear_q4_0_t(&x, &gate, 1); let _ = self.linear_q4_0_t(&x, &up, 1); let _ = self.linear_q4_0_t(&xff, &down, 1); }
        wait();
        let t = std::time::Instant::now();
        for _ in 0..iters {
            let _g = self.linear_q4_0_t(&x, &gate, 1);
            let _u = self.linear_q4_0_t(&x, &up, 1);
            let _o = self.linear_q4_0_t(&xff, &down, 1);
            wait();
        }
        t.elapsed().as_secs_f64() * 1000.0 / iters as f64
    }

    pub fn probe_expert_stream(&self, slot_mib: usize, iters: usize) -> (f64, f64, usize) {
        let max_buf = self.device.limits().max_buffer_size as usize;
        let bytes = (slot_mib * (1 << 20)).min(max_buf & !255).max(4);
        let actual_mib = bytes / (1 << 20);
        let host = vec![0u8; bytes];
        let wait = || { self.queue.submit(std::iter::empty()); crate::counted_poll(&self.device); };

        // (1) STAGED fill: re-fillable STORAGE buffer via write_buffer (the discrete-GPU path).
        let staged_slot = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stream-slot-staged"), size: bytes as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        for _ in 0..2 { self.queue.write_buffer(&staged_slot, 0, &host); }
        wait();
        let t = std::time::Instant::now();
        for _ in 0..iters { self.queue.write_buffer(&staged_slot, 0, &host); wait(); }
        let staged_ms = t.elapsed().as_secs_f64() * 1000.0 / iters as f64;

        // (2) MAPPED fill: the UMA direct path — create the slot mapped_at_creation, memcpy the expert bytes
        // straight into its device-visible backing, unmap. This is what the arena does for resident weights;
        // a streamed slot recreates a fresh mapped buffer per swap. On UMA the memcpy runs at ~memory
        // bandwidth (no staging belt). Includes the create+unmap cost (a real per-swap cost).
        for _ in 0..2 {
            let b = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("warm"), size: bytes as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: true });
            b.slice(..).get_mapped_range_mut().copy_from_slice(&host); b.unmap();
        }
        let t = std::time::Instant::now();
        for _ in 0..iters {
            let b = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("stream-slot-mapped"), size: bytes as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: true });
            b.slice(..).get_mapped_range_mut().copy_from_slice(&host);
            b.unmap();
        }
        let mapped_ms = t.elapsed().as_secs_f64() * 1000.0 / iters as f64;

        (staged_ms, mapped_ms, actual_mib)
    }
    /// Upload `bytes` as a resident STORAGE buffer, zero-copy on unified memory. On UMA (Apple/Asahi) the
    /// buffer's backing IS host RAM, so `mapped_at_creation` + one memcpy writes straight into device memory
    /// — no staging-belt copy, no transient staging allocation (which is what doubled the 70B load peak).
    /// On discrete GPUs we keep `create_buffer` + `write_buffer`: staging is required there, and write_buffer
    /// records a CATCHABLE OutOfMemory instead of the fatal get_mapped_range abort a failed map-on-create
    /// gives. The UMA map path is safe because weight loads memplan-preflight the fit before any upload.
    fn upload_bytes(&self, bytes: &[u8], label: &'static str) -> wgpu::Buffer {
        // ROUND UP TO A WHOLE WORD. Shaders view these buffers as `array<u32>`, so the last partial word
        // is unreadable if the buffer ends mid-word: a WGSL index past the array length yields zero, not
        // an error.
        //
        // Found via Q6_K, whose super-block is 210 bytes — 52.5 words. Its f16 scale lives at bytes
        // 208..209, inside word 52, which spans 208..211 and lies past a 210-byte buffer. The scale read
        // as ZERO, so every weight in the block came out zero, and the gate reported max|diff| 3.3e5
        // against the CPU path. Any quant whose block size is not a multiple of 4 has the same exposure;
        // Q4_K (144) and Q8_0 (34→ padded) happen to be safe or already padded, which is why this
        // survived until a 210-byte format arrived.
        let size = (bytes.len().max(4).div_ceil(4) * 4) as u64;
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
        // Post-OOM safety (see download_u32): mapping a buffer that failed to allocate is a fatal wgpu
        // error, so bail early with an empty result once the device has signalled OOM this step.
        if self.oom_backoff() { return Vec::new(); }
        let bytes = (t.len() * 4) as u64;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("dl"), size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        // If a frame is open, append the readback copy to it and flush the whole frame in one
        // submit. frame.keep (dims uniforms + bind groups) is RETAINED past the submit — it's async,
        // and dropping it before the poll below recycles buffers the GPU is still reading → "Buffer
        // 'dims' is invalid" on Metal. clear_retained() after the poll frees them safely.
        // Remember whether a frame was OPEN, because taking it here is what silently ends batching for
        // everything that follows. See the reopen at the end of this function.
        let had_frame = self.frame.lock().unwrap().is_some();
        if let Some(mut frame) = self.frame.lock().unwrap().take() {
            SUBMIT_BY_DOWNLOAD.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            frame.enc.copy_buffer_to_buffer(&t.buf, 0, &staging, 0, bytes);
            self.submit(frame.enc);
            self.retain_keep(frame.keep);
        } else {
            let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            enc.copy_buffer_to_buffer(&t.buf, 0, &staging, 0, bytes);
            // TAGGED: a readback: copy_buffer_to_buffer into staging. Six of eight submit callers incremented nothing, which is
            // where the profile's "N UNATTRIBUTED, a submit path is still untagged" came from.
            SUBMIT_BY_DOWNLOAD.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.submit(enc);
        }
        let slice = staging.slice(..);
        let t_dl_poll = std::time::Instant::now();
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let t_sync = std::time::Instant::now();
        GPU_POLLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed); self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        GPU_SYNC_NS.fetch_add(t_sync.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        let data = slice.get_mapped_range();
        let v = bytemuck::cast_slice(&data).to_vec();
        drop(data); staging.unmap();
        check_stall(t_dl_poll, 0);   // a readback that waited seconds is a hang, not a slow kernel
        self.clear_retained(); // poll waited for completion → retained dims/bind-groups are safe to free
        // REOPEN THE FRAME. Taking it above is necessary — the readback has to be submitted to be read —
        // but leaving it closed meant every subsequent dispatch took `dispatch`'s no-frame branch, which
        // submits ONE COMMAND BUFFER PER DISPATCH. In a MoE decode the router readback happens once per
        // layer, so the frame died at the first layer and never came back: measured at 1119 submits/token
        // on Metal (48 layers x ~23 dispatches after each readback) at 2.0 ms each, which was the entire
        // step time. `flush_every` looked ignored because the code path it governs was not being used.
        //
        // Only reopened if one was open on entry: a caller that downloads without a frame does not want
        // one opened behind its back.
        if had_frame {
            self.begin_frame();
            // PACING. Reopening restores batching, but it also removes the pacing that
            // one-submit-per-dispatch accidentally provided: with the submit governor disabled — which it
            // is on Honeykrisp, where completion callbacks are slow or absent — nothing else bounds how
            // much work piles into a single submit. Measured consequence: GPU watchdog timeouts and a
            // visibly stuttering desktop, because the compositor shares this GPU and never gets between
            // the batches.
            //
            // So on a driver with no working governor, cap the batch after a readback. This gives back
            // most of the submit reduction while leaving gaps the display can be serviced in. Drivers
            // with a functioning governor keep the full batch, because there the governor already does
            // this job properly.
            // NOTE: this used to STORE a reduced flush_every here, which was a one-way ratchet — the
            // submit tuner picks 128, the first readback lowers it to 32, and nothing ever restores it,
            // so every token after the first ran at four times the submit count the tuner chose. The
            // pacing was meant to be per-batch and was implemented as a permanent mutation.
            //
            // Removed rather than repaired: the readback cadence already paces this path. A MoE decode
            // reads the router back once per layer, so the frame is drained ~48 times per token no
            // matter what flush_every says, and the compositor gets its gaps from that. Adding a second
            // pacing mechanism on top only destroyed the tuner's measurement.
        }
        v
    }

    /// Current passes-per-submit (the submit-overhead knob).
    /// Current degrade level (0 = fast path). Kernels consult this to pick a conservative variant after a
    /// prior GPU fault, so one caught fault protects the rest of the session (cf. Colibri's per-block CPU
    /// fallback — here we degrade to the safe GPU kernels rather than CPU, since our whole engine is GPU).
    pub fn degrade_level(&self) -> u32 { self.degrade_level.load(std::sync::atomic::Ordering::Relaxed) }
    /// Whether the device signalled OOM since the last clear (set by the uncaptured-error handler). The
    /// decode loop checks this to abort a step gracefully instead of pushing more allocations at a device
    /// that just failed one. Cleared by `clear_oom` once the caller has backed off (freed a window / cache).
    /// Devices whose driver kills a long-running compute dispatch.
    ///
    /// Apple's Honeykrisp does, and so does every mobile driver — Android's GPU watchdog is the same
    /// mechanism under a different name. `conservative_vma` was standing in for this and means "Vulkan
    /// on Apple", so a phone took the unbounded path and would trip the very watchdog the guard exists
    /// to avoid.
    ///
    /// Named for the property rather than the vendor, which is the whole lesson of the last four fixes.
    pub fn watchdog_prone(&self) -> bool {
        self.conservative_vma || watchdog_prone_name(&self.adapter_name)
    }

    /// Did the subgroup matvec pass its load-time self-check on this driver?
    ///
    /// Exposed so a CACHED kernel choice can be re-checked against it. The autotune search already
    /// honours this; the cache-load path did not, which is how a disabled kernel gets selected.
    pub fn subgroup_kernel_ok(&self) -> bool {
        self.sg_ok.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn oom_backoff(&self) -> bool { self.oom_flag.load(std::sync::atomic::Ordering::Relaxed) }
    pub fn clear_oom(&self) { self.oom_flag.store(false, std::sync::atomic::Ordering::Relaxed); }
    /// Test/validation hook: force the OOM-backoff flag so the download guards can be exercised without an
    /// actual device OOM (which can't be provoked deterministically in-container).
    pub fn set_oom_for_test(&self, v: bool) { self.oom_flag.store(v, std::sync::atomic::Ordering::Relaxed); }
    /// Whether we should avoid the fragile fast-path kernels (barrier-heavy attention, collapsed submits).
    pub fn degraded(&self) -> bool { self.degrade_level() >= 1 }
    /// Raise the degrade latch (monotonic — only ever increases). Logged once per level so a degrade is
    /// visible, not silent. Called when a guarded GPU op faults.
    pub fn raise_degrade(&self, reason: &str) {
        let prev = self.degrade_level.fetch_max(1, std::sync::atomic::Ordering::Relaxed);
        if prev < 1 {
            eprintln!("wgpu-engine: DEGRADED to safe kernels — {reason} (a GPU op faulted; using conservative paths for the rest of the session to avoid a hang)");
        }
    }
    /// Run a GPU closure inside a validation+OOM error scope and a bounded device poll, returning whether
    /// it completed cleanly. On a captured error this raises the degrade latch and returns false, so the
    /// CALLER can fall through to a safe path — the Colibri "GPU is a try-and-return shortcut in front of
    /// the safe path" shape. `poll_ms` bounds the wait so a wedged submit surfaces as a fault instead of an
    /// indefinite hang (0 = wait indefinitely, for backends where that's safe).
    pub fn run_guarded<R>(&self, what: &str, f: impl FnOnce() -> R) -> Option<R> {
        let vscope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let r = f();
        // flush any open frame so the work is actually submitted before we poll for completion
        self.end_frame();
        crate::counted_poll(&self.device);
        let verr = pollster::block_on(vscope.pop());
        if verr.is_some() {
            self.raise_degrade(what);
            return None;
        }
        Some(r)
    }
    /// Find the largest total GPU allocation this device will actually HONOUR.
    ///
    /// The Asahi budget of 24 GiB is an empirical figure from the staging upload path, measured before
    /// the map-at-creation arena existed. Nobody has measured what the device really allows, and the
    /// loader's own message says "raise LOGSCOPE_MAX_GPU_GB to probe the real ceiling" — so this probes
    /// it rather than leaving that to a person with a env var and an afternoon.
    ///
    /// ## Allocation succeeding is not the answer
    ///
    /// A driver can return a buffer it cannot back, and the failure then appears later as wrong results
    /// rather than an error. So every step WRITES a pattern and READS IT BACK. A step counts only if the
    /// data survives; anything else is treated as failure, because an overcommitted allocation that
    /// reports success is worse than one that refuses.
    ///
    /// Allocates in `slab` chunks and keeps them all live, because the ceiling is on TOTAL resident
    /// bytes rather than any single buffer — one 30 GiB buffer failing says nothing about thirty 1 GiB
    /// ones, and the arena packs into ≤2047 MiB slabs anyway.
    ///
    /// Returns the highest verified total in bytes. Everything is dropped before returning.
    pub fn probe_vram_ceiling(&self, slab_bytes: u64, max_bytes: u64) -> u64 {
        let slab = slab_bytes.max(1 << 20);
        let words = (slab / 4) as usize;
        let mut held: Vec<Tensor> = Vec::new();
        let mut verified = 0u64;
        // A recognisable pattern: zeros would pass against a buffer that was never written.
        let pattern: Vec<f32> = (0..1024).map(|i| (i as f32) * 3.5 + 1.0).collect();
        while verified + slab <= max_bytes {
            let step = self.run_guarded("vram-probe", || {
                let t = self.alloc_resident_words(words);
                self.queue.write_buffer(&t.buf, 0, bytemuck::cast_slice(&pattern));
                t
            });
            let Some(t) = step else { break };
            // Read it back. An allocation the driver cannot honour often accepts the write and returns
            // garbage — which is the case this whole probe exists to catch.
            let got = self.download(&self.view_pub(&t, 0, pattern.len()));
            if got.len() < pattern.len() || got != pattern { break; }
            held.push(t);
            verified += slab;
        }
        drop(held);
        crate::counted_poll(&self.device);
        verified
    }

    /// GPU keep-alive spin (cf. Colibri's COLI_METAL_SPIN). Decode is dispatch-bound: our own profile says
    /// "encoder setup, not GPU sync, is the wall", and on Apple Silicon the GPU ramps its clock DOWN
    /// between the short per-token bursts, so the next burst starts on a cold clock (Colibri measured this
    /// tripling attention-kernel time). A background thread submitting a trivial dispatch in a tight loop
    /// keeps the clock warm. Opt-in (LOGSCOPE_GPU_SPIN=1) since it costs power; detached so it never blocks
    /// exit; self-throttled with a tiny sleep so it doesn't flood the queue. No correctness effect (writes
    /// a 4-float scratch buffer nothing reads).
    pub fn maybe_start_gpu_spin(&self) {
        if std::env::var_os("LOGSCOPE_GPU_SPIN").is_none() { return; }
        static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) { return; }
        let device = self.device.clone();
        let queue = self.queue.clone();
        eprintln!("wgpu-engine: GPU keep-alive spin ON (LOGSCOPE_GPU_SPIN) — holds the GPU clock warm between decode bursts (costs power)");
        std::thread::spawn(move || {
            const KA: &str = "@group(0)@binding(0) var<storage,read_write> b: array<f32>;\n@compute @workgroup_size(4,1,1)\nfn main(@builtin(global_invocation_id) g:vec3<u32>){ b[g.x]=b[g.x]*1.0+f32(g.x); }";
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("keepalive"), source: wgpu::ShaderSource::Wgsl(KA.into()) });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("keepalive"), layout: None, module: &module, entry_point: Some("main"),
                compilation_options: Default::default(), cache: None });
            let buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("ka"), size: 16,
                usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false });
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() }] });
            loop {
                let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                { let mut p = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
                  p.set_pipeline(&pipeline); p.set_bind_group(0, &bg, &[]); p.dispatch_workgroups(1, 1, 1); }
                queue.submit(Some(enc.finish()));
                // ~1ms cadence: frequent enough to hold the clock, cheap enough to not contend with decode.
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });
    }
    pub fn flush_every(&self) -> u32 { self.flush_every.load(std::sync::atomic::Ordering::Relaxed) }
    /// Set passes-per-submit. Clamped to [1,1024]. Ignored when LOGSCOPE_FLUSH_EVERY pins it, so a manual
    /// override always wins over the adaptive tuner. Returns the value in effect after the call.
    pub fn set_flush_every(&self, n: u32) -> u32 {
        if std::env::var_os("LOGSCOPE_FLUSH_EVERY").is_none() {
            self.flush_every.store(n.clamp(1, 65536), std::sync::atomic::Ordering::Relaxed);
        }
        self.flush_every()
    }
    /// Whether LOGSCOPE_FLUSH_EVERY pins the knob (then the tuner should not run).
    pub fn flush_every_pinned(&self) -> bool { std::env::var_os("LOGSCOPE_FLUSH_EVERY").is_some() }

    /// Create + fill one dims UNIFORM buffer, capturing any device error instead of letting it abort.
    /// The M4 "Buffer 'dims' invalid" crash is a validation error raised at write_buffer against a
    /// buffer whose creation actually failed; wrapping both in a Validation error scope lets us LOG the
    /// real first error (once) and continue, turning a hard abort into a diagnosable, survivable event.
    fn make_dims_buffer(&self, dims: &[u8]) -> std::sync::Arc<wgpu::Buffer> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let b = std::sync::Arc::new(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dims"), size: DIMS_BUF_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false }));
        self.queue.write_buffer(&b, 0, dims);
        if let Some(err) = pollster::block_on(scope.pop()) {
            self.dims_alloc_failed.store(true, std::sync::atomic::Ordering::Relaxed);
            static REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !REPORTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                let cc = self.dims_cache.try_lock().map(|c| c.len()).unwrap_or(usize::MAX);
                eprintln!("wgpu-engine: DIMS BUFFER ALLOC FAILED — {err:?}\n  \
                    (dims_cache≈{cc}; the device rejected a tiny uniform alloc → memory/handle pressure. \
                    Falling back to per-token decode for this turn.)");
            }
        }
        b
    }
    /// Whether a dims-buffer allocation has failed (device memory/handle pressure). The engine checks
    /// this after a windowed attempt and retries the turn on the leaner per-token path if set.
    pub fn dims_alloc_failed(&self) -> bool { self.dims_alloc_failed.load(std::sync::atomic::Ordering::Relaxed) }
    pub fn clear_dims_alloc_failed(&self) { self.dims_alloc_failed.store(false, std::sync::atomic::Ordering::Relaxed); }
    /// Return a reusable dims UNIFORM buffer holding `dims`, rotating a bounded ring so the total live
    /// dims-buffer count stays capped (fixes the M4 buffer-count exhaustion). The ring fills lazily to
    /// DIMS_RING_SIZE; once full it rewrites the oldest slot. Rewriting a slot whose previous use is
    /// still in-flight would corrupt that dispatch, so when the ring WRAPS (returns to slot 0) we first
    /// flush the open frame and poll to completion — a rare fence (~once per DIMS_RING_SIZE dispatches)
    /// that guarantees every slot's prior read is done before it's reused.
    fn ring_dims(&self, dims: &[u8]) -> std::sync::Arc<wgpu::Buffer> {
        let mut ring = self.dims_ring.lock().unwrap();
        let (buffers, next, _writes) = &mut *ring;
        if buffers.len() < DIMS_RING_SIZE {
            // still growing the ring — allocate a new reusable slot (bounded by DIMS_RING_SIZE total)
            let b = self.make_dims_buffer(dims);
            buffers.push(b.clone());
            *next = buffers.len() % DIMS_RING_SIZE;
            return b;
        }
        // ring full: reuse slot `next`. If we're about to wrap to slot 0, fence first so no in-flight
        // dispatch is still reading a slot we're about to overwrite.
        let idx = *next;
        if idx == 0 {
            drop(ring); // release the ring lock before touching the frame/queue
            SUBMIT_BY_DIMS_WRAP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.flush_and_wait();
            ring = self.dims_ring.lock().unwrap();
        }
        let (buffers, next, _writes) = &mut *ring;
        let b = buffers[idx].clone();
        self.queue.write_buffer(&b, 0, dims);
        *next = (idx + 1) % DIMS_RING_SIZE;
        b
    }
    /// Total live dims UNIFORM buffers (content cache + reuse ring). Bounded by DIMS_CACHE_CAP +
    /// DIMS_RING_SIZE regardless of generation length — the invariant the M4 buffer-count fix rests on.
    pub fn dims_buffer_count(&self) -> usize {
        self.dims_cache.lock().unwrap().len() + self.dims_ring.lock().unwrap().0.len()
    }
    /// A rough count of live tensor-storage buffers for the governor's headroom sum: the pool's free-list
    /// holds recyclable buffers; net allocated ≈ misses (created) − current free (returned). This is an
    /// estimate (KV + resident weights aren't pooled), not exact — the governor treats it as a lower
    /// bound and its baseline_objects covers the fixed remainder.
    pub fn pool_live_estimate(&self) -> usize {
        let free: usize = self.pool.free.lock().unwrap().values().map(|v| v.len()).sum();
        (POOL_MISSES.load(std::sync::atomic::Ordering::Relaxed) as usize).saturating_sub(free)
    }
    /// Flush any open frame and block until the GPU has drained it — used to fence ring reuse.
    fn flush_and_wait(&self) {
        if let Some(frame) = self.frame.lock().unwrap().take() {
            // TAGGED: the explicit end_frame path. Six of eight submit callers incremented nothing, which is
            // where the profile's "N UNATTRIBUTED, a submit path is still untagged" came from.
            SUBMIT_BY_END_FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.submit(frame.enc);
            self.retain_keep(frame.keep);
        }
        let t_poll = std::time::Instant::now();
        crate::counted_poll(&self.device);
        check_stall(t_poll, 0);
        self.clear_retained();
    }

    /// Move a flushed frame's keep-list into the pending set: buffers referenced by a submitted-
    /// but-not-completed frame must outlive the async GPU work. Drained by `clear_retained` after
    /// the next poll (which waits for completion).
    fn retain_keep(&self, keep: Vec<Box<dyn std::any::Any + Send>>) {
        if keep.is_empty() { return; }
        self.pending_keep.lock().unwrap().extend(keep);
    }
    /// Drop everything retained by `retain_keep`. Call ONLY after a poll that waited for GPU
    /// completion — at that point the submitted work is done and its buffers are safe to free.
    fn clear_retained(&self) {
        self.pending_keep.lock().unwrap().clear();
    }

    /// Single point through which every command buffer is flushed, so submits can be
    /// counted (see GPU_SUBMITS). Each call is one CPU→GPU flush.
    fn submit(&self, enc: wgpu::CommandEncoder) {
        use std::sync::atomic::Ordering::Relaxed;
        GPU_SUBMITS.fetch_add(1, Relaxed);
        let t_sub = std::time::Instant::now();
        self.queue.submit([enc.finish()]);
        GPU_SUBMIT_NS.fetch_add(t_sub.elapsed().as_nanos() as u64,
                                std::sync::atomic::Ordering::Relaxed);
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
            // Safety escape: if callbacks arrive slowly or never (a driver quirk — seen on some Vulkan
            // drivers where on_submitted_work_done lags the actual GPU completion), don't stall the decode
            // waiting on them. A healthy submit clears in well under 250ms; if the queue stays full past
            // that, the callback pipeline is unreliable on THIS driver, so disable the governor and run
            // unthrottled (the poll-on-readback path still bounds memory). Fast bail keeps a laggy driver
            // from turning every submit into a multi-second stall that reads as a hang.
            if t0.elapsed().as_millis() >= 250 {
                self.governor_dead.store(true, Relaxed);
                eprintln!("wgpu-engine: submit governor DISABLED — completion callbacks slow/absent on this driver (submit stalled {}ms; LOGSCOPE_SUBMIT_INFLIGHT=0 silences this)", t0.elapsed().as_millis());
                // don't leave this submit counted as forever-pending: the work will still complete, but
                // the governor is off now, so clear our bookkeeping so a stale count can't wedge later.
                pending.store(0, Relaxed);
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
        // TAGGED: flushing a previous frame is an end_frame in all but name. Six of eight submit callers incremented nothing, which is
        // where the profile's "N UNATTRIBUTED, a submit path is still untagged" came from.
        // TAG INSIDE THE CONDITIONAL. This counted a submit whether or not one happened: when the
        // frame was already empty, fr.take() yields None and nothing is flushed, but the counter had
        // already advanced. Your log showed the result — "9824 of 9642 submits attributed", more
        // causes than submits, which is arithmetically impossible and was mine from _406.
        if let Some(prev) = fr.take() {
            SUBMIT_BY_END_FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.submit(prev.enc);
        }
        *fr = Some(Frame { enc: self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") }), keep: Vec::new(), passes: 0 });
    }
    /// Flush the open frame (one submit for all its recorded ops). No-op if none is open.
    pub fn end_frame(&self) {
        if let Some(frame) = self.frame.lock().unwrap().take() {
            SUBMIT_BY_END_FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.submit(frame.enc); self.retain_keep(frame.keep);
        }
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
        let pipe_ptr = std::sync::Arc::as_ptr(pipe) as usize; // stable identity for this pipeline
        // Reuse the dims UNIFORM buffer for this op's byte-identical (m,k,n,grid) instead of
        // creating+uploading a fresh one every dispatch — the per-dispatch "upload locking" fix.
        let dbuf = {
            let mut cache = self.dims_cache.lock().unwrap();
            if let Some(b) = cache.get(dims) {
                DIMS_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                b.clone()
            } else {
                DIMS_MISSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if cache.len() < DIMS_CACHE_CAP {
                    let b = self.make_dims_buffer(dims);
                    cache.insert(dims.to_vec(), b.clone());
                    b
                } else {
                    drop(cache);
                    self.ring_dims(dims)
                }
            }
        };
        let n = inputs.len() as u32;
        let (bgl, pipeline) = (&pipe.0, &pipe.1);
        // Opt-in bind-group cache: reuse the bind group for this exact (pipeline, buffers, dims) instead of
        // recreating it every dispatch (the Asahi dispatch-bound hotspot). Views are now CACHEABLE: their
        // (offset, size) is part of the key, so the same buffer bound at the same range hits, and a
        // different range misses instead of silently reusing the wrong binding. This matters because
        // streaming MoE weights are all views into slot buffers — the exact traffic that used to bypass
        // the cache and left it at 18% reuse. Bit-identical: the key pins the same ranges to the same slots.
        let bg_cache_on = bind_group_cache_enabled();
        let cache_key: Option<BgKey> = if bg_cache_on {
            let bufs: Vec<(wgpu::Buffer, u64, u64)> = inputs.iter()
                .map(|t| { let (o, l) = t.view.unwrap_or((0, 0)); (t.buf.clone(), o, l) }).collect();
            let ov = out.view.unwrap_or((0, out.buf.size()));
            Some((pipe_ptr, bufs, (out.buf.clone(), ov.0, ov.1), (*dbuf).clone()))
        } else { None };
        if let Some(key) = &cache_key {
            if let Some(bg) = self.bg_cache.lock().unwrap().get(key).cloned() {
                BG_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                GPU_SETUP_NS.fetch_add(t_setup.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
                self.record_dispatch(pipeline, &bg, wg, None);
                return;
            }
        }
        let mut bg_entries = Vec::new();
        for (i, t) in inputs.iter().enumerate() {
            // LOCALISE THE SHORT BINDING. --gate-bytes showed reads past the first u32 returning zero
            // on real hardware, and the two candidates are indistinguishable from source: a view
            // arriving where none should, or the dims uniform cutting the shader's loop short.
            //
            // This prints what is ACTUALLY bound, which separates them in one run: a view line here
            // means the tensor carried one; no view line plus a short result means dims.
            if std::env::var_os("LOGSCOPE_TRACE_BIND").is_some() {
                let sz = t.buf.size();
                match t.view {
                    None => eprintln!("  bind[{i}] whole buffer, {sz} bytes ({} u32)", sz / 4),
                    Some((off, len)) => eprintln!(
                        "  bind[{i}] VIEW offset {off} len {len} of a {sz}-byte buffer ({} u32) \
— if this input should be whole, the view is the fault", len / 4),
                }
            }
            let resource = match t.view {
                None => t.buf.as_entire_binding(),
                Some((off, len)) => wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &t.buf, offset: off, size: std::num::NonZeroU64::new(len) }),
            };
            bg_entries.push(wgpu::BindGroupEntry { binding: i as u32, resource });
        }
        // Honour the OUTPUT's view, exactly as the inputs above do. Without this a chunked dispatch
        // writes every chunk at offset 0 — the Q6_K conversion produced a correct first chunk and then
        // overwrote it 37 times.
        let out_res = match out.view {
            None => out.buf.as_entire_binding(),
            Some((off, len)) => wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &out.buf, offset: off, size: std::num::NonZeroU64::new(len) }),
        };
        bg_entries.push(wgpu::BindGroupEntry { binding: n, resource: out_res });
        bg_entries.push(wgpu::BindGroupEntry { binding: n + 1, resource: dbuf.as_entire_binding() });
        let bg = dev.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: bgl, entries: &bg_entries });
        let bg = std::sync::Arc::new(bg);
        if let Some(key) = cache_key {
            BG_MISSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut cache = self.bg_cache.lock().unwrap();
            if cache.len() < BG_CACHE_CAP { cache.insert(key, bg.clone()); }
        }
        GPU_SETUP_NS.fetch_add(t_setup.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        mark_dispatch(pipe_ptr, wg, dims.first().map(|b| *b as u32).unwrap_or(0));
        self.record_dispatch(pipeline, &bg, wg, Some(&dbuf));
    }

    /// Record one compute dispatch into the open frame (or submit standalone if no frame is open). The bind
    /// group is passed as an Arc so a CACHED bind group can be recorded without recreating it; the Arc (and
    /// the dims uniform, if any) are pushed onto the frame keep-list so neither is freed while the GPU may
    /// still read them — the same retain discipline as before, just with a shared bind group. `dbuf` is None
    /// on the cache-hit fast path (already pinned by an earlier miss's keep or the dims cache).
    fn record_dispatch(&self, pipeline: &wgpu::ComputePipeline, bg: &std::sync::Arc<wgpu::BindGroup>, wg: (u32, u32, u32), dbuf: Option<&std::sync::Arc<wgpu::Buffer>>) {
        let dev = &self.device;
        let mut fr = self.frame.lock().unwrap();
        if let Some(frame) = fr.as_mut() {
            {
                let mut pass = frame.enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
                pass.set_pipeline(pipeline); pass.set_bind_group(0, bg.as_ref(), &[]);
                pass.dispatch_workgroups(wg.0, wg.1, wg.2);
            }
            frame.keep.push(Box::new(bg.clone())); // Arc<BindGroup> kept until flush (cached or fresh)
            if let Some(d) = dbuf { frame.keep.push(Box::new(d.clone())); } // AND its dims uniform Arc
            frame.passes += 1;
            let over_flush = frame.passes >= self.flush_every.load(std::sync::atomic::Ordering::Relaxed);
            let over_retain = frame.keep.len() >= DIMS_RETAIN_CAP;
            if over_flush || over_retain {
                if over_flush { SUBMIT_BY_PASSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
                else { SUBMIT_BY_RETAIN.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
                let old = fr.take().unwrap();
                self.submit(old.enc);
                self.retain_keep(old.keep);
                *fr = Some(Frame { enc: dev.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") }), keep: Vec::new(), passes: 0 });
                drop(fr);
                if over_retain && self.pending_keep.lock().unwrap().len() >= DIMS_RETAIN_CAP {
                    crate::counted_poll(&self.device);
                    self.clear_retained();
                    DIMS_RETAIN_POLLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
        } else {
            drop(fr);
            let mut enc = dev.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            {
                let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
                pass.set_pipeline(pipeline); pass.set_bind_group(0, bg.as_ref(), &[]);
                pass.dispatch_workgroups(wg.0, wg.1, wg.2);
            }
            SUBMIT_BY_NO_FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
                crate::counted_poll(&self.device);
            }
            // size % COPY_BUFFER_ALIGNMENT (4) == 0 is REQUIRED for a mapped_at_creation buffer; both inputs
            // above are 256-aligned so this is a no-op today, but it guards any future unaligned size source.
            let size = a.slab_size.max(len.next_multiple_of(ALIGN)).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
            // mapped slabs are filled by memcpy into their mapped range (no COPY_DST); staged slabs need COPY_DST
            let usage = if a.mapped { wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC }
                        else { wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST };
            let slab = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("arena-slab"), size,
                usage, mapped_at_creation: a.mapped });
            // A FAILED ALLOCATION MUST NOT BE MAPPED. create_buffer returns a Buffer even when the
            // driver could not back it — the failure arrives through the uncaptured-error handler and
            // the buffer is INVALID. Mapping an invalid buffer panics inside wgpu:
            //
            //   Failed to allocate BO VMA / BO creation failed
            //   panicked at wgpu_core.rs: Error in Buffer::get_mapped_range
            //   Caused by: Buffer with 'arena-slab' label is invalid
            //
            // That is a Llama-3.3-70B load on a 48 GiB budget, and a crash is the worst possible
            // outcome for a condition the memory planner exists to report. Polling flushes the
            // pending device error so oom_flag is set before anything touches the buffer.
            let _ = self.device.poll(wgpu::PollType::Poll);
            if self.oom_backoff() {
                // Leave the slab out of the arena and hand back a tensor over a minimal buffer. The
                // OOM flag is already set, so the enclosing load discards this and reports device OOM
                // through the path built for it — rather than aborting the process here.
                eprintln!("wgpu-engine: arena slab allocation FAILED ({} MiB) — the driver could not \
back it. Reporting device OOM rather than mapping an invalid buffer",
                    a.slab_size / (1 << 20));
                let dummy = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("arena-slab-oom"), size: 4, usage,
                    mapped_at_creation: false });
                return Tensor { buf: dummy, rows, cols, pool: None, view: None };
            }
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
            self.write_chunked(&slab, off, bytes);   // arena slabs are up to 2 GiB
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

    /// The GPU-resident embedding table for windowed decode. Under MoE streaming (tight VMA budget) the
    /// table is q8 — ~4× smaller than f32, saving ~2.9 GiB on a Scout-class vocab, at ~0.5% RMSE (the
    /// input lookup is robust to it; logits use the separate output projection, untouched). Otherwise f32,
    /// byte-identical to before. Kept in sync with the memplan streaming preflight, which counts the embed
    /// q8 under the same flag so the budget matches what actually gets allocated.
    pub fn resident_embed(&self, m: &Model) -> ResidentEmbed {
        if std::env::var("LOGSCOPE_MOE_STREAM").is_ok() {
            ResidentEmbed::Q8(self.embed_q8(m))
        } else {
            ResidentEmbed::F32(self.upload(&m.embed, m.vocab, m.d))
        }
    }

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

    /// Quantize to q4 and return the RESIDENT BYTES host-side (qw ‖ qs) instead of uploading — for the
    /// streaming store, which keeps cold experts host-resident and fills them into GPU slots on demand.
    /// Layout: [qw: n*k/8 u32 little-endian][qs: n*kb f32 little-endian]. qs_off = qw byte length.
    pub fn quantize_q4_host_bytes(&self, w: &[f32], n: usize, k: usize) -> (Vec<u8>, usize) {
        let q = self.quantize_q4_resident_cpu(w, n, k);
        let qw_bytes: &[u8] = bytemuck::cast_slice(&q.0);
        let qs_bytes: &[u8] = bytemuck::cast_slice(&q.1);
        let qs_off = qw_bytes.len();
        let mut out = Vec::with_capacity(qw_bytes.len() + qs_bytes.len());
        out.extend_from_slice(qw_bytes);
        out.extend_from_slice(qs_bytes);
        (out, qs_off)
    }

    /// Apply a REPIN on the GPU: the pinned buffers holding `evict` are refilled in place with `admit`'s
    /// bytes and re-keyed. Pinned buffers for a layer are all one expert-layer in size, so the swap is a
    /// `write_buffer` into the EXISTING allocation — no new BO, which is what makes this affordable under a
    /// VMA ceiling. Queue ordering guarantees the refill lands after any previously submitted dispatch that
    /// read those buffers.
    fn stream_repin(&self, sm: &std::sync::Mutex<MoeStream>, evict: u32, admit: u32) -> bool {
        let mut s = sm.lock().unwrap();
        let (Some((gqw, gqs)), Some((dqw, dqs))) = (s.pinned_gu.remove(&evict), s.pinned_dn.remove(&evict)) else {
            return false; // nothing pinned under that id — leave the policy untouched
        };
        let (gu_off, dn_off) = (s.gu_qs_off, s.dn_qs_off);
        let gu = std::mem::take(&mut s.host_gu[admit as usize]);
        let dn = std::mem::take(&mut s.host_dn[admit as usize]);
        self.queue.write_buffer(&gqw, 0, &gu[..gu_off]);
        self.queue.write_buffer(&gqs, 0, &gu[gu_off..]);
        self.queue.write_buffer(&dqw, 0, &dn[..dn_off]);
        self.queue.write_buffer(&dqs, 0, &dn[dn_off..]);
        s.host_gu[admit as usize] = gu;
        s.host_dn[admit as usize] = dn;
        s.pinned_gu.insert(admit, (gqw, gqs));
        s.pinned_dn.insert(admit, (dqw, dqs));
        s.policy.repin(evict, admit);
        s.repins += 1;
        true
    }

    /// Speculatively bring expert `e` resident in this layer's slots WITHOUT counting a demand miss —
    /// Colibri's `couple_prefetch`. Returns true if a fill was issued. Called BEFORE the layer's router
    /// matvec/readback, so the fill occupies the queue during the CPU-side routing stall (the window where
    /// the GPU would otherwise sit idle waiting on the logits download). A mispredicted prefetch costs one
    /// fill's bandwidth and is the first LRU victim, so it self-corrects.
    fn stream_prefetch(&self, sm: &std::sync::Mutex<MoeStream>, e: u32) -> bool {
        let mut s = sm.lock().unwrap();
        if e as usize >= s.n_expert { return false; }
        let Some(slot) = s.policy.prefetch_spare(e) else { return false }; // resident, or no spare slot → skip
        let slot = slot as usize;
        let gu_bytes = std::mem::take(&mut s.host_gu[e as usize]);
        let dn_bytes = std::mem::take(&mut s.host_dn[e as usize]);
        s.gu_slots.fill(self, slot, &gu_bytes);
        s.dn_slots.fill(self, slot, &dn_bytes);
        s.host_gu[e as usize] = gu_bytes;
        s.host_dn[e as usize] = dn_bytes;
        true
    }

    /// Resolve one routed expert to its (gate_up, down) q4 Lins via the streaming store: pinned → resident
    /// Lin; else LRU slot (fill from host bytes on a miss, reuse on a hit). This is the per-expert fill-on-
    /// demand that lets an over-ceiling MoE decode on Asahi. Leans on Colibri's pin∪LRU∪load tiering.
    fn stream_expert_lins(&self, sm: &std::sync::Mutex<MoeStream>, e: u32) -> (Lin, Lin) {
        use std::sync::atomic::Ordering::Relaxed;
        let mut s = sm.lock().unwrap();
        let (gu_rows, gu_k, dn_rows, dn_k) = (s.gu_rows, s.gu_k, s.dn_rows, s.dn_k);
        // pinned under the VIEW path: a single converted blob per matrix, bound like a slot
        if let Some((gb, db)) = s.pinned_raw.get(&e).cloned() {
            let (gu_off, dn_off) = match &s.raw {
                Some(r) => (r.gu_layout().0, r.dn_layout().0),
                None => (s.gu_qs_off, s.dn_qs_off),
            };
            let _ = s.policy.access(e);
            MOE_HITS.fetch_add(1, Relaxed);
            return (self.q4_lin_from_slots(&gb, gu_off, gu_rows, gu_k, None),
                    self.q4_lin_from_slots(&db, dn_off, dn_rows, dn_k, None));
        }
        // pinned hot expert → resident buffers (host-copy path)
        if let (Some((gqw, gqs)), Some((dqw, dqs))) = (s.pinned_gu.get(&e).cloned(), s.pinned_dn.get(&e).cloned()) {
            let _ = s.policy.access(e);
            MOE_HITS.fetch_add(1, Relaxed);
            return (self.q4_lin_from_bufs(&gqw, &gqs, gu_rows, gu_k), self.q4_lin_from_bufs(&dqw, &dqs, dn_rows, dn_k));
        }
        let (gu_off, dn_off) = (s.gu_qs_off, s.dn_qs_off);
        let acc = s.policy.access(e);
        let slot = match acc { crate::expert_cache::Access::Hit(x) | crate::expert_cache::Access::Miss(x) => x as usize, crate::expert_cache::Access::Pinned => 0 };
        if matches!(acc, crate::expert_cache::Access::Miss(_)) {
            MOE_FILLS.fetch_add(1, Relaxed);
            if let Some(r) = &s.raw {
                // VIEW PATH (Colibri): the expert's bytes are a window into the model file. Convert them
                // on the GPU straight into the slot — gate into rows 0..ff, up into rows ff..2ff — so
                // nothing was ever copied into host memory and nothing is dequantized to f32.
                let (ff, d) = (r.ff, r.d);
                let (gu_qs_off, _) = r.gu_layout();
                let (dn_qs_off, _) = r.dn_layout();
                let (gate, up, down) = (r.gate_of(e as usize), r.up_of(e as usize), r.down_of(e as usize));
                let half_qw_words = ff * d / 8;
                let half_qs_vals = ff * (d / 32);
                let gu_buf = s.gu_slots.tensor_full(slot);
                let dn_buf = s.dn_slots.tensor_full(slot);
                if let (Some(g), Some(u), Some(dw)) = (gate, up, down) {
                    MOE_BYTES.fetch_add((g.len() + u.len() + dw.len()) as u64, Relaxed);
                    // ONE persistent staging buffer, written and converted three times. Allocating a source
                    // buffer per fill (what this used to do) exhausts the driver's VMA within a few
                    // thousand fills and kills the run mid-generation.
                    // THREE REGIONS per fill, in a RING of STAGE_RING fills. Queued writes all land
                    // before any command buffer in a submit, so a fill must not reuse a region another
                    // fill in the same submit is still reading. _181 solved that with end_frame() before
                    // and after every fill — correct, but two submits per fill, which is where
                    // 1152 submits/token came from. The ring gives each fill its own regions, so many
                    // fills share one submit and we only close the frame when the ring WRAPS.
                    let al = |x: usize| x.div_ceil(256) * 256;
                    let per_fill = al(g.len()) + al(u.len()) + al(dw.len());
                    let slot = {
                        let mut c = self.stage_cursor.lock().unwrap();
                        let s = *c % Self::STAGE_RING;
                        *c += 1;
                        // Wrapping means slot 0 is about to be rewritten while an earlier fill may still
                        // be queued against it — that is the one point where the frame must close.
                        // DO NOT REOPEN HERE. Reopening was tried in _274 and CHANGED THE MODEL'S OUTPUT
                        // on hardware, which a submission-structure change must never do.
                        //
                        // This end_frame is a FENCE, not a flush. `queue.write_buffer` data is applied at
                        // the START of a submit, before every command buffer in it — so if the frame stays
                        // open, a later fill's staging writes join the same submit as an earlier fill's
                        // dispatches and land BEFORE them. The earlier fill then reads the later expert's
                        // bytes. That is the hazard _181 fixed, and reopening reintroduced it.
                        //
                        // The cost is real: this is most of the 99% no-frame submits. The fix is to make
                        // the fence cheaper (fewer fills, or converting Q6_K so experts can be file VIEWS
                        // and need no staging at all), not to remove it.
                        //
                        // AND IT MUST FENCE ON COMPLETION, NOT SUBMISSION. end_frame() only submits, which
                        // orders the commands but does not wait for them. Slot 0 can therefore be
                        // rewritten while an earlier fill's dispatches are still IN FLIGHT — and on
                        // Honeykrisp nothing throttles queue depth, because the submit governor disables
                        // itself when completion callbacks are slow or absent. In-flight submits then
                        // accumulate until more than STAGE_RING fills overlap and a slot is overwritten
                        // underneath a running dispatch.
                        //
                        // That matches the observed failure shape better than resource accumulation does:
                        // thousands of fills succeed, then timeouts begin and continue — which is what a
                        // hazard that needs deep queues to manifest looks like, not what a leak looks
                        // like. The dims ring already fences with flush_and_wait for exactly this reason;
                        // the staging ring using the weaker primitive was an inconsistency, not a choice.
                        //
                        // LOGSCOPE_STAGE_FENCE=submit restores the old behaviour for A/B, because the wait
                        // costs real time and the hypothesis deserves a measurement rather than my word.
                        if s == 0 {
                            // The ring wrap IS a candidate boundary, so it is where a breadcrumb is most
                            // useful: if faults cluster on wrap ticks the ring is implicated, and if they
                            // fall between them it is not.
                            progress_tick("ring-wrap", 0, "fence");
                            if std::env::var("LOGSCOPE_STAGE_FENCE").as_deref() == Ok("submit") {
                                self.end_frame();
                            } else {
                                self.flush_and_wait();
                            }
                        }
                        s
                    };
                    let base = slot * per_fill;
                    let (o0, o1) = (base, base + al(g.len()));
                    let o2 = o1 + al(u.len());
                    let stage = self.moe_stage(Self::STAGE_RING * per_fill);
                    self.write_chunked(&stage.buf, o0 as u64, g);
                    self.write_chunked(&stage.buf, o1 as u64, u);
                    self.write_chunked(&stage.buf, o2 as u64, dw);
                    let view = |off: usize, len: usize| Tensor {
                        buf: stage.buf.clone(), rows: 1, cols: len / 4, pool: None,
                        view: Some((off as u64, al(len) as u64)) };
                    self.q4k_convert_from(&view(o0, g.len()), &gu_buf, ff, d, 0, gu_qs_off / 4);
                    self.q4k_convert_from(&view(o1, u.len()), &gu_buf, ff, d, half_qw_words, gu_qs_off / 4 + half_qs_vals);
                    self.q4k_convert_from(&view(o2, dw.len()), &dn_buf, d, ff, 0, dn_qs_off / 4);
                }
            } else {
                let gu_bytes = std::mem::take(&mut s.host_gu[e as usize]);
                let dn_bytes = std::mem::take(&mut s.host_dn[e as usize]);
                MOE_BYTES.fetch_add((gu_bytes.len() + dn_bytes.len()) as u64, Relaxed);
                s.gu_slots.fill(self, slot, &gu_bytes);
                s.dn_slots.fill(self, slot, &dn_bytes);
                s.host_gu[e as usize] = gu_bytes;
                s.host_dn[e as usize] = dn_bytes;
            }
        } else {
            MOE_HITS.fetch_add(1, Relaxed);
        }
        // Under the view path the qs offset is dictated by the slot layout (256-aligned), not by the
        // host blob's packing.
        let (gu_off, dn_off) = match &s.raw {
            Some(r) => (r.gu_layout().0, r.dn_layout().0),
            None => (gu_off, dn_off),
        };
        let gu_buf = s.gu_slots.slot_buf(slot);
        let dn_buf = s.dn_slots.slot_buf(slot);
        let gu = self.q4_lin_from_slots(&gu_buf, gu_off, gu_rows, gu_k, None);
        let dn = self.q4_lin_from_slots(&dn_buf, dn_off, dn_rows, dn_k, None);
        (gu, dn)
    }

    /// TEST HELPER: build a MoeStream whose experts are VIEWS into raw Q4_K bytes (the Colibri path),
    /// rather than host copies. `gate`/`up`/`down` are the whole expert tensors as they appear in a file.
    pub fn build_moe_stream_raw(&self, gate: &'static [u8], up: &'static [u8], down: &'static [u8],
                                n_expert: usize, ff: usize, d: usize, n_used: usize, ring: usize) -> MoeStream {
        // 144 IS Q4_K's BLOCK SIZE, and this helper hardcoded it — so every MoE streaming test only
        // ever exercised Q4_K experts. A "Q4_K_M" checkpoint is MIXED: llama.cpp ships some tensors as
        // Q6_K (210 bytes per 256), and the production path at build time uses the type-aware
        // `source_block_bytes` precisely because of that.
        //
        // A fixture that can only produce one type cannot catch a stride computed for the wrong one —
        // and a wrong expert stride reads every expert after the first from the wrong offset, which
        // is gibberish rather than an error.
        //
        // Defaults to Q4_K so existing callers are unchanged; `build_moe_stream_raw_typed` takes the
        // ggml types.
        self.build_moe_stream_raw_typed(gate, up, down, n_expert, ff, d, n_used, ring, 12, 12)
    }

    /// As `build_moe_stream_raw`, with the gate/up and down source types given.
    pub fn build_moe_stream_raw_typed(&self, gate: &'static [u8], up: &'static [u8], down: &'static [u8],
        n_expert: usize, ff: usize, d: usize, n_used: usize, ring: usize,
        gu_ty: u32, dn_ty: u32) -> MoeStream {
        let gu_expert_bytes = Self::source_block_bytes(gu_ty, ff * d)
            .expect("a source type this helper can size");
        let dn_expert_bytes = Self::source_block_bytes(dn_ty, d * ff)
            .expect("a source type this helper can size");
        // These are VIEWS into a retained mapping, not copies — so the mapping must stay resident for the
        // process lifetime. Declaring it here rather than at the release site keeps the fact next to the
        // thing that makes it true.
        mark_map_views_live();
        // These slices point INTO the retained mapping and stay live for the process lifetime, so the
        // page cache must not be dropped underneath them. Declaring it here rather than at the release
        // site keeps the fact next to the thing that makes it true.
        mark_map_views_live();
        let raw = RawExpertSrc { gate, up, down, gu_expert_bytes, dn_expert_bytes, ff, d,
                                 gu_type: 12, dn_type: 12 };
        let (_, gu_total) = raw.gu_layout();
        let (_, dn_total) = raw.dn_layout();
        MoeStream {
            layer: 0, n_expert,
            gu_rows: 2 * ff, gu_k: d, dn_rows: d, dn_k: ff,
            host_gu: Vec::new(), host_dn: Vec::new(), gu_qs_off: 0, dn_qs_off: 0,
            gu_slots: StreamSlots::new(self, ring, gu_total as u64),
            dn_slots: StreamSlots::new(self, ring, dn_total as u64),
            policy: crate::expert_cache::LayerSlots::new(&[], ring as u32),
            pinned_gu: std::collections::HashMap::new(),
            pinned_dn: std::collections::HashMap::new(),
            repins: 0, visits: 0, raw: Some(raw), pinned_raw: std::collections::HashMap::new(),
        }
    }

    /// TEST HELPER: build a MoeStream from raw f32 expert weights (same weights a resident MoeLayer would
    /// use), so --gate-moe-stream can verify the streamed decode path is bit-exact to the resident one.
    /// `gus[e]` is expert e's [2ff, d] gate‖up; `dns[e]` is [d, ff] down. `pin` experts are pinned resident.
    pub fn build_moe_stream_from_weights(&self, gus: &[Vec<f32>], dns: &[Vec<f32>], ff: usize, d: usize, n_used: usize, pin: usize) -> MoeStream {
        self.build_moe_stream_ring(gus, dns, ff, d, n_used, pin, None)
    }

    /// As above with an explicit slot-ring size. Ring headroom is the variable that decides whether
    /// speculative prefetch can do anything: at ring == the routed working set there is no spare slot, so a
    /// non-displacing prefetch is necessarily a no-op.
    pub fn build_moe_stream_ring(&self, gus: &[Vec<f32>], dns: &[Vec<f32>], ff: usize, d: usize, n_used: usize, pin: usize, ring_override: Option<usize>) -> MoeStream {
        let n_expert = gus.len();
        let gu_rows = 2 * ff; let dn_rows = d;
        let ring = ring_override.unwrap_or((n_used + 1).max(2));
        let mut host_gu = Vec::with_capacity(n_expert); let mut host_dn = Vec::with_capacity(n_expert);
        let mut gu_qs_off = 0; let mut dn_qs_off = 0;
        for e in 0..n_expert {
            let (gb, goff) = self.quantize_q4_host_bytes(&gus[e], gu_rows, d);
            gu_qs_off = goff; host_gu.push(gb);
            let (db, doff) = self.quantize_q4_host_bytes(&dns[e], dn_rows, ff);
            dn_qs_off = doff; host_dn.push(db);
        }
        let gu_slot_bytes = host_gu[0].len() as u64;
        let dn_slot_bytes = host_dn[0].len() as u64;
        let mut pinned_gu = std::collections::HashMap::new();
        let mut pinned_dn = std::collections::HashMap::new();
        for e in 0..pin.min(n_expert) {
            pinned_gu.insert(e as u32, self.q4_bufs_from_host(&host_gu[e], gu_qs_off));
            pinned_dn.insert(e as u32, self.q4_bufs_from_host(&host_dn[e], dn_qs_off));
        }
        let pinned_ids: Vec<u32> = (0..pin.min(n_expert) as u32).collect();
        MoeStream {
            layer: 0, n_expert, gu_rows, gu_k: d, dn_rows, dn_k: ff,
            host_gu, host_dn, gu_qs_off, dn_qs_off,
            gu_slots: StreamSlots::new(self, ring, gu_slot_bytes),
            dn_slots: StreamSlots::new(self, ring, dn_slot_bytes),
            policy: crate::expert_cache::LayerSlots::new(&pinned_ids, ring as u32),
            pinned_gu, pinned_dn, repins: 0, visits: 0, raw: None, pinned_raw: std::collections::HashMap::new(),
        }
    }

    /// Dequantize raw **Q4_K** super-blocks on the GPU — the enabling step for reading a GGUF's native
    /// blocks instead of requantizing them on load.
    ///
    /// This is the Colibri principle applied to our stack: Colibri never converts on load ("container
    /// pre-quantizzato… niente copia"), because its file format already IS its inference format. We can't
    /// hand wgpu file-backed pages the way Colibri hands them to Metal, but we CAN stop converting: upload
    /// the file's own Q4_K bytes and decode them on the GPU. That removes both the f32 round-trip and the
    /// host-side requantized copy (~50 GiB on Scout).
    ///
    /// Layout is taken from the CPU implementation in `gguf::dequantize` (type 12), which is already
    /// validated by every Q4_K model that loads: 144-byte super-block of 256 weights = f16 `d`, f16 `dmin`,
    /// 12 bytes of packed 6-bit scales/mins, 128 nibble bytes. `w = d·sc·nib − dmin·m`.
    pub fn dequant_q4k(&self, raw: &[u8], n: usize) -> Vec<f32> {
        assert!(n % 256 == 0, "q4_k requires a multiple of 256 weights");
        let nblk = n / 256;
        let need = nblk * 144;
        // pad to a u32 boundary so the shader can read each super-block through a word view
        let mut padded = raw[..need.min(raw.len())].to_vec();
        padded.resize(need.div_ceil(4) * 4, 0);
        let (gx, gy) = wrapped_grid(nblk);
        let dims = [nblk as u32, gx, 0, 0];
        self.run_f32(Q4K_DEQUANT, &[bytemuck::cast_slice(&padded)], n,
                     bytemuck::cast_slice(&dims), (gx, gy, 1))
    }


    pub fn q4_bufs_from_host(&self, blob: &[u8], qs_off: usize) -> (wgpu::Buffer, wgpu::Buffer) {
        let qw = self.upload_bytes(&blob[..qs_off], "pin-qw");
        let qs = self.upload_bytes(&blob[qs_off..], "pin-qs");
        (qw, qs)
    }

    /// Build a q4 Lin from separate qw/qs GPU buffers (pinned experts or filled slots), with the expert
    /// shape [n, k]. Zero copy — wraps the existing buffers as Tensors the q4 kernel binds directly.
    pub fn q4_lin_from_bufs(&self, qw: &wgpu::Buffer, qs: &wgpu::Buffer, n: usize, k: usize) -> Lin {
        let kb = k / 32;
        let qw_len = (n * k / 8 * 4) as u64;
        let qs_len = (n * kb * 4) as u64;
        let qwt = Tensor { buf: qw.clone(), rows: n, cols: k, pool: None, view: Some((0, qw_len)) };
        let qst = Tensor { buf: qs.clone(), rows: n, cols: kb, pool: None, view: Some((0, qs_len)) };
        Lin::q4(Q4Weight { qw: qwt, qs: qst, n, k })
    }

    /// CPU-only q4 quantization (the compute half of quantize_q4_resident, without the GPU upload). Returns
    /// (qw u32 words, qs f32 scales).
    fn quantize_q4_resident_cpu(&self, w: &[f32], n: usize, k: usize) -> (Vec<u32>, Vec<f32>) {
        let kb = k / 32;
        let mut qs = vec![0f32; n * kb];
        let mut nib = vec![0u8; n * k];
        // PARALLEL over rows. Each row's blocks depend only on that row's weights, so this is bit-identical
        // to the sequential version and simply stops one core doing all of it — this runs for every expert
        // on the MoE fallback path and for every tensor that misses the GPU conversion.
        {
            let threads = std::thread::available_parallelism().map(|t| t.get()).unwrap_or(1).min(n.max(1));
            let rows_per = n.div_ceil(threads.max(1));
            std::thread::scope(|sc| {
                for (ti, (qs_c, nib_c)) in qs.chunks_mut(rows_per * kb).zip(nib.chunks_mut(rows_per * k)).enumerate() {
                    let base_row = ti * rows_per;
                    sc.spawn(move || {
                        let rows = nib_c.len() / k;
                        for rr in 0..rows {
                            let r = base_row + rr;
                            for b in 0..kb {
                                let base = r * k + b * 32;
                                let mut amax = 0f32;
                                for j in 0..32 { amax = amax.max(w[base + j].abs()); }
                                let scale = if amax > 0.0 { amax / 7.0 } else { 1.0 };
                                qs_c[rr * kb + b] = scale;
                                for j in 0..32 {
                                    let qv = (w[base + j] / scale).round().clamp(-7.0, 7.0) as i32;
                                    nib_c[rr * k + b * 32 + j] = (qv + 8) as u8;
                                }
                            }
                        }
                    });
                }
            });
        }
        let mut qw = vec![0u32; n * k / 8];
        for (li, word) in qw.iter_mut().enumerate() {
            let i8b = li * 8;
            let mut v = 0u32;
            for b in 0..8 { v |= ((nib[i8b + b] & 0xF) as u32) << (b * 4); }
            *word = v;
        }
        (qw, qs)
    }

    /// Build a q4 Lin whose weights live in two prefilled STREAM SLOTS (qw slot, qs slot). The slots were
    /// filled with quantize_q4_host_bytes output; here we just wrap them as Tensors of the right shape so the
    /// existing linear_q4_0_t kernel binds them exactly like a resident weight. Zero copy — the bytes are
    /// already on the GPU from the fill.
    pub fn q4_lin_from_slots(&self, gu_slot: &wgpu::Buffer, qs_off: usize, n: usize, k: usize, split_buf: Option<&wgpu::Buffer>) -> Lin {
        let kb = k / 32;
        let qw_len = (n * k / 8 * 4) as u64;
        let qs_len = (n * kb * 4) as u64;
        // qw and qs share one slot buffer (qs at byte qs_off) unless a split buffer is given.
        let qw = Tensor { buf: gu_slot.clone(), rows: n, cols: k, pool: None, view: Some((0, qw_len)) };
        let qs = match split_buf {
            Some(b) => Tensor { buf: b.clone(), rows: n, cols: kb, pool: None, view: Some((0, qs_len)) },
            None => Tensor { buf: gu_slot.clone(), rows: n, cols: kb, pool: None, view: Some((qs_off as u64, qs_len)) },
        };
        Lin::q4(Q4Weight { qw, qs, n, k })
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
            // The wide-load kernel, now reachable by autotune rather than only by env var.
            MatvecKernel::V3 => self.matvec_decode_v3(a, w, cfg.tile),
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

    /// Fused RMSNorm + int8 mat-vec (m=1): the RMSNORM_MATVEC_Q8 kernel normalizes `a` (weight `nw`,
    /// `eps`) in its prologue, then does the [n×k] matvec — one dispatch replacing rmsnorm+matvec.
    /// Bit-identical to the two-dispatch chain (same reduction, same accumulation order).
    pub fn linear_q8_0_norm(&self, a: &Tensor, nw: &Tensor, w: &QWeight, cols: usize, eps: f32) -> Tensor {
        debug_assert_eq!(cols, w.k, "fused norm-matvec: norm width must equal the matmul's k");
        let out = self.alloc(1, w.n);
        let gx = (w.n as u32).min(32768);
        let gy = (w.n as u32).div_ceil(gx);
        let kb = (w.k / 32) as u32;
        let dims = [1u32, w.n as u32, w.k as u32, kb, gx, eps.to_bits(), 0, 0];
        self.dispatch(RMSNORM_MATVEC_Q8, &[a, nw, &w.qw, &w.qs], &out, bytemuck::cast_slice(&dims), (gx, gy, 1));
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
        if let Some(m) = w.moe.as_deref() {
            let n2 = self.rmsnorm_t(x2, &w.g2, t, d, eps);
            return self.moe_ffn(m, &n2, t, d, x2);
        }
        // Dense SwiGLU FFN. norm_apply fuses norm2→wgate_up in the m=1 decode case (one dispatch),
        // and falls back to explicit rmsnorm_t+apply for prefill (t>1) — identical output either way.
        let gu = w.wgate_up.norm_apply(self, x2, &w.g2, d, eps, t);
        // Gemma gates with GELU rather than SiLU. The kernel has always existed; selecting it per block
        // is what turns it from dead code into support.
        let hmid = if w.geglu { self.geglu_packed(&gu) } else { self.swiglu_packed(&gu) };
        match &w.post_ffn {
            // Gemma: normalize the FFN's OUTPUT before adding the residual — not the same as the pre-FFN
            // norm already applied above, and skipping it changes every layer's contribution.
            Some(pn) => {
                let h = w.wdown.apply(self, &hmid, t);
                let hn = self.rmsnorm_t(&h, pn, t, d, eps);
                self.add_t(x2, &hn)
            }
            None => w.wdown.apply_add(self, &hmid, t, x2),
        }
    }

    /// MoE FFN over `t` tokens (t=1 decode or t=thousands prefill — same path via expert grouping):
    /// router matvec → READBACK [t×n_expert] logits → CPU top-k (moe::route, tested) → for each expert
    /// that won ≥1 token: GATHER its token rows, run the expert's SwiGLU FFN batched (existing kernels),
    /// then weighted SCATTER-ADD into the output. Correctness-first v1: the per-layer router readback is a
    /// GPU sync (~1-3 ms) — the known perf ceiling (~5-10 tok/s decode) until GPU-side top-k lands.
    fn moe_ffn(&self, m: &MoeLayer, n2: &Tensor, t: usize, d: usize, x2: &Tensor) -> Tensor {
        // Shared expert (GLM / DeepSeek / Llama-4 / Qwen2-MoE): a dense SwiGLU FFN added on top of the
        // routed sum for EVERY token — the standard shared+routed decomposition. qwen3moe: None.
        //
        // ISSUED FIRST, DELIBERATELY. It depends only on `n2`, not on routing, so it is the one piece of
        // this layer that does not wait for the router. `moe_ffn_routed` downloads the router logits to
        // route on the CPU, and that download drains the queue — 48 of them per token on a 48-layer
        // model, measured as 3030 ms of a 4399 ms step (GPU-sync-bound). Recording the shared expert's
        // dispatches BEFORE that download means they are in the same submit and execute while the CPU is
        // blocked, instead of starting after it returns. Same math, same final add order: bit-identical.
        //
        // This is the cheap half of the sync problem. The expensive half is that the routing readback
        // exists at all — llama.cpp keeps top-k in the graph and never leaves the GPU mid-forward, which
        // is what our v2 path does too, but streaming forces v1 because the CPU must know which expert to
        // upload before it can upload it.
        let shared = m.shared.as_ref().map(|(sgu, sdn)| {
            let gu = sgu.apply(self, n2, t);
            let hmid = self.swiglu_packed(&gu);
            sdn.apply(self, &hmid, t)
        });
        let routed = self.moe_ffn_routed(m, n2, t, d, x2);
        match shared {
            None => routed,
            Some(sh) => self.add_t(&routed, &sh),
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
        // Streaming (m.stream) MUST use v1: the stacked weights are placeholders under streaming; experts
        // are served per-expert from the stream store, which the v1 loop intercepts.
        // Sigmoid gating is now handled by MOE_TOPK's mode 1, so it no longer forces v1. `weight_before`
        // still does, and that is the remaining piece for Llama-4: the weight must scale the expert's
        // INPUT rather than its output, and SwiGLU is nonlinear so the two are genuinely different —
        // `silu(w*g)*(w*u) != w*silu(g)*u`. Doing it needs the routed path to build per-slot scaled
        // activations (xper=1 for gate_up) instead of broadcasting one row, which is a real change
        // rather than a flag.
        if t == 1 && m.stream.is_none() && !m.cfg.weight_before
            && std::env::var("LOGSCOPE_MOE_V1").is_err() && !route_trace_armed()
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
        // CROSS-LAYER COUPLING PREFETCH (Colibri couple_prefetch), before the router sync below: the
        // PREVIOUS MoE layer's routed set predicts this layer's, so issue those fills now — they occupy the
        // queue during the logits download + CPU routing, the window the GPU would otherwise idle through.
        // Opt-in (LOGSCOPE_MOE_PREFETCH): a mispredict costs one fill's bandwidth, so it earns its keep
        // only when the coupling signal is real, which is exactly what --gate-moe-prefetch measures.
        let prefetch_on = m.stream.is_some() && std::env::var("LOGSCOPE_MOE_PREFETCH").is_ok();
        let mut predicted: Vec<u32> = Vec::new();  // everything the coupling table proposed
        let mut issued: Vec<u32> = Vec::new();     // of those, the ones that actually needed (and got) a fill
        if let (Some(sm), true) = (&m.stream, prefetch_on) {
            let layer = sm.lock().unwrap().layer as i32;
            let budget = m.cfg.n_used as usize;
            let (sig, prev) = {
                let h = moe_route_hist().lock().unwrap();
                (crate::expert_predict::CtxSig::unigram(0), h.prev_experts.clone())
            };
            if !prev.is_empty() {
                predicted = moe_route_hist().lock().unwrap().map.predict(sig, layer, &prev, budget);
                for &e in &predicted { if self.stream_prefetch(sm, e) { issued.push(e); } }
                let mut h = moe_route_hist().lock().unwrap();
                h.prefetched += issued.len() as u64;
                h.pred_total += predicted.len() as u64;
            }
        }
        // REPIN SAFE POINT (Colibri): periodically re-decide WHICH experts deserve the never-evicted pin
        // slots, from observed heat. The loader had to choose a pin set before it had ever seen the model
        // route, so its choice is a guess; this is where the guess gets corrected. Checked at layer entry
        // (nothing of this layer's is in flight) and rate-limited, since each swap costs a re-upload.
        if let Some(sm) = &m.stream {
            let (do_check, n_expert) = {
                let mut s = sm.lock().unwrap();
                s.visits += 1;
                (s.visits % REPIN_EVERY == 0, s.n_expert as u32)
            };
            if do_check && std::env::var("LOGSCOPE_MOE_NOREPIN").is_err() {
                // Steer the pin set with the coupling predictor as well as observed heat. This is the one
                // place the predictor pays on a capacity-bound model: it cannot speculate into ring slots
                // (no spare slots — measured), but the pin set has to hold SOMETHING, and pinning what is
                // about to be hot beats pinning what was hot. The hint only breaks ties; observed heat
                // still wins outright.
                let predicted: Vec<u32> = {
                    let h = moe_route_hist().lock().unwrap();
                    if h.prev_experts.is_empty() { Vec::new() } else {
                        let layer = sm.lock().unwrap().layer as i32;
                        h.map.predict(crate::expert_predict::CtxSig::unigram(0), layer,
                                      &h.prev_experts, m.cfg.n_used as usize)
                    }
                };
                // START THE READ. The prediction fed the eviction policy and nothing else, so it
                // changed WHICH expert was resident and never WHEN its bytes arrived — and the model
                // mapping is advised Random, so a page is not read until it is faulted on.
                //
                // WILLNEED on the predicted ranges closes that gap: the kernel begins the read now and
                // the fault at use time finds the page warm. Advisory, so a wrong prediction costs
                // pages the kernel would have dropped anyway, and a failure leaves the old behaviour.
                if !predicted.is_empty() {
                    // `raw` lives on MoeStream — present only on the file-VIEW path, which is
                    // exactly where a prefetch can help: the host-copy path has already paid for the
                    // bytes, so hinting them would advise pages that are not the bottleneck.
                    let g = sm.lock().unwrap();
                    if let Some(raw) = g.raw.as_ref() {
                        let (gu, dn) = crate::prefetch::ranges_for(
                            &predicted, n_expert, raw.gu_expert_bytes, raw.dn_expert_bytes);
                        // gate/up share a stride, so one hint per expert covers both when they are
                        // one tensor; down is separate. Hinting the slices directly rather than the
                        // mapping, because these ARE views into it.
                        crate::prefetch::hint_all(raw.gate, &gu, crate::prefetch::BUDGET);
                        crate::prefetch::hint_all(raw.down, &dn, crate::prefetch::BUDGET);
                    }
                }
                let pick = sm.lock().unwrap().policy.pick_repin_hinted(n_expert, &predicted);
                if let Some((evict, admit)) = pick {
                    if self.stream_repin(sm, evict, admit) {
                        MOE_REPINS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
            // Live residency telemetry (LOGSCOPE_MOE_STATS=1): the hit rate and the bytes actually moved,
            // reported from the real run instead of inferred. Emitted from layer 0 only so one line covers
            // the model rather than one per layer.
            if do_check && sm.lock().unwrap().layer == 0 && std::env::var("LOGSCOPE_MOE_STATS").is_ok() {
                let (h, f, bytes, rep, rate) = moe_stream_stats();
                eprintln!("moe-stream: hit {:.1}% ({h} hits / {f} fills), {:.2} GiB moved, {rep} repins{}",
                    rate * 100.0, bytes as f64 / (1u64 << 30) as f64,
                    if rate < 0.43 { "  [below the 43% break-even — expect streaming to cost more than it saves]" } else { "" });
            }
        }
        let logits = self.download(&m.router.apply(self, n2, t)); // [t, n_expert] — the sync
        let ne = m.cfg.n_routed as usize;
        // CPU routing: per token, top-k experts + combine weights; group token indices per expert.
        let mut groups: Vec<Vec<(u32, f32)>> = vec![Vec::new(); ne]; // expert → [(token_row, weight)]
        for tok in 0..t {
            let sels = crate::moe::route(&logits[tok * ne..(tok + 1) * ne], &m.cfg);
            // ROUTE_TRACE: record this token's routed set for the traced layer (measurement only,
            // and only on this readback path so it never adds a sync).
            let tl = self.trace_layer.load(std::sync::atomic::Ordering::Relaxed);
            if tl >= 0 {
                let ids: Vec<u32> = sels.iter().map(|s| s.expert).collect();
                let tok = self.trace_tok.load(std::sync::atomic::Ordering::Relaxed);
                route_trace_tok(tl, tok, &ids);
            }
            for sel in sels {
                groups[sel.expert as usize].push((tok as u32, sel.weight));
            }
        }
        // residual base: out starts as a copy of x2, experts scatter-add on top
        // Feed the coupling table: (previous layer's routed set → this layer's), so the NEXT layer can
        // predict. Also score how many speculative fills this layer actually used.
        if let (Some(sm), true) = (&m.stream, prefetch_on) {
            let layer = sm.lock().unwrap().layer as i32;
            let routed: Vec<u32> = groups.iter().enumerate()
                .filter(|(_, g)| !g.is_empty()).map(|(e, _)| e as u32).collect();
            let mut h = moe_route_hist().lock().unwrap();
            let prev = h.prev_experts.clone();
            h.prefetch_hits += issued.iter().filter(|e| routed.contains(e)).count() as u64;
            h.pred_correct += predicted.iter().filter(|e| routed.contains(e)).count() as u64;
            h.map.observe(crate::expert_predict::CtxSig::unigram(0), layer, &routed, &prev);
            h.prev_experts = routed;
            h.prev_layer = layer;
        }
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
            // Llama4 weight-before-FFN: gate the expert's INPUT rows (then combine with unit
            // weights — the gate has already been applied and must not be applied twice).
            let (rows, wgt) = if m.cfg.weight_before {
                let scaled = self.alloc(n, d);
                self.dispatch(ROW_SCALE, &[&rows, &wgt], &scaled, bytemuck::cast_slice(&dims), wg1((n * d) as u32));
                (scaled, self.upload(&vec![1.0f32; n], 1, n))
            } else { (rows, wgt) };
            // expert FFN, batched over its n tokens. STREAMING: if this layer streams, resolve expert e
            // through the pin∪LRU∪fill store (fills a slot from host q4 bytes on a miss); else use the
            // resident view.
            let dn = if let Some(sm) = &m.stream {
                let (gu_lin, dn_lin) = self.stream_expert_lins(sm, e as u32);
                let gu = gu_lin.apply(self, &rows, n);
                let hmid = self.swiglu_packed(&gu);
                dn_lin.apply(self, &hmid, n)
            } else {
                let gu = m.gate_up[e].apply(self, &rows, n);
                let hmid = self.swiglu_packed(&gu);
                m.down[e].apply(self, &hmid, n)
            };
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
        if raw.len() < 2 * (m.cfg.n_used.clamp(1, 16) as usize) { return v1; } // OOM backoff: skip the debug compare
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
        // mode 1 = sigmoid. Selection is unchanged either way; only the weighting differs.
        let mode = match m.cfg.norm { crate::moe::RouteNorm::Sigmoid => 1u32, _ => 0 };
        let tk = [ne as u32, slots as u32, mode, m.cfg.normalize as u32];
        self.dispatch(MOE_TOPK, &[&logits], &selw, bytemuck::cast_slice(&tk), (1, 1, 1));
        // 3) fused gate||up for each selected expert (broadcast activation)
        // one dispatch across ALL selected experts: wg.z = slot (was 8 dispatches — the dominant
        // per-layer dispatch cost; on Asahi each submit batch costs ~4-5 ms of sync, so dispatch count
        // is the wall clock now that the polls are gone)
        // With LOGSCOPE_FUSE_GLU, gate||up AND SwiGLU run as ONE kernel emitting silu(gate)·up directly
        // ([slots,ff]) — one fewer dispatch per MoE layer and no 2ff scratch. Bit-identical.
        let hmid = if moe_glu_fused() {
            let glu_sh = match &m.stacked_gu.kind { LinKind::Q4(_) => MOE_MATVEC_GLU_Q4, _ => MOE_MATVEC_GLU_Q8 };
            let ff = dn_k; // output width per expert = ff (down's k)
            let hmid = self.alloc(slots, ff);
            let gx = (ff as u32).min(32768);
            let gy = (ff as u32).div_ceil(gx);
            // d.n = ff (output width), rstride = 2ff (gate at col, up at col+ff)
            let dims = [ff as u32, gu_k as u32, (gu_k / 32) as u32, gx, 0u32, ff2 as u32, 0u32, 0u32];
            self.dispatch(glu_sh, &[n2, gu_qw, gu_qs, &selw], &hmid, bytemuck::cast_slice(&dims), (gx, gy, slots as u32));
            hmid
        } else {
            let gu_out = self.alloc(slots, ff2);
            {
                let gx = (ff2 as u32).min(32768);
                let gy = (ff2 as u32).div_ceil(gx);
                let dims = [ff2 as u32, gu_k as u32, (gu_k / 32) as u32, gx, 0u32, ff2 as u32, 0u32, 0u32];
                self.dispatch(sh, &[n2, gu_qw, gu_qs, &selw], &gu_out, bytemuck::cast_slice(&dims), (gx, gy, slots as u32));
            }
            // 4) SwiGLU over the packed [slots, 2ff]
            self.swiglu_packed(&gu_out)
        };
        // 5+6) down projection + weighted combine + residual.
        if moe_down_fused() {
            // ONE fused dispatch: per output column, reduce each expert's down row, combine weight[s]·result,
            // add residual. No [slots,d] scratch, no separate MOE_WSUM. Bit-identical.
            let dn_sh = match &m.stacked_down.kind { LinKind::Q4(_) => MOE_DOWN_COMBINE_Q4, _ => MOE_DOWN_COMBINE_Q8 };
            let out = self.alloc(1, d);
            let gx = (d as u32).min(32768);
            let gy = (d as u32).div_ceil(gx);
            let dims = [d as u32, dn_k as u32, (dn_k / 32) as u32, gx, slots as u32, 0u32, 0u32, 0u32];
            self.dispatch(dn_sh, &[&hmid, dn_qw, dn_qs, &selw, x2], &out, bytemuck::cast_slice(&dims), (gx, gy, 1));
            return Some((out, selw));
        }
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

    /// GeGLU over a packed `[m, 2h]` gate||up buffer — the Gemma-family FFN gate (tanh-approx GELU on the
    /// gate half instead of SiLU). Same layout/contract as `swiglu_packed`, so it drops into the fused-FFN
    /// path with only the activation swapped.
    pub fn geglu_packed(&self, gu: &Tensor) -> Tensor {
        let (m, h) = (gu.rows, gu.cols / 2);
        let out = self.alloc(m, h); let n = (m * h) as u32;
        let dims = [n, h as u32, 0, 0];
        self.dispatch(GEGLU_PACKED, &[gu], &out, bytemuck::cast_slice(&dims), wg1(n)); out
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
        // Per-layer rope base (Gemma: windowed layers use rope.freq_base_swa), then the Llama-4 NoPE
        // sentinel — order matters, since a NoPE layer skips rotation regardless of which base it would
        // otherwise use.
        let base = w.rope_base.unwrap_or(base);
        let base = if w.nope { 0.0 } else { base }; // Llama4 iRoPE: NoPE layers skip rotation (rope hosts' identity sentinel)
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd; // ad = attention width (== d for plain MHA)
        let n1 = self.rmsnorm_t(x, &w.g1, t, d, eps);
        // ORDER VERIFIED against llama.cpp models/qwen3moe.cpp, which does:
        //
        //     97:  Qcur = build_norm(Qcur, attn_q_norm, LLM_NORM_RMS, il);   <- QK-norm FIRST
        //     100: Qcur = ggml_rope_ext(...);                               <- RoPE SECOND
        //
        // Read inside-out, the line below is project -> qk_norm -> rope -> post, which matches. The
        // norm tensor is sized {n_embd_head_k} there and `hd` is passed here, so both are per
        // head_dim rather than over the full projection.
        //
        // Pinned as a comment because the ordering lives inside one nested expression: a refactor
        // that hoists these into statements could reorder them without looking wrong, and swapping
        // norm and rope produces gibberish rather than an error.
        let q = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wq.apply(self, &n1, t), &w.q_norm, t, ad, hd, eps), ff, t, ad, base, 0, hd), w, t, ad, hd, eps);
        let k = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wk.apply(self, &n1, t), &w.k_norm, t, kv, hd, eps), ff, t, kv, base, 0, hd), w, t, kv, hd, eps);
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
        // Per-layer rope base (Gemma: windowed layers use rope.freq_base_swa), then the Llama-4 NoPE
        // sentinel — order matters, since a NoPE layer skips rotation regardless of which base it would
        // otherwise use.
        let base = w.rope_base.unwrap_or(base);
        let base = if w.nope { 0.0 } else { base }; // Llama4 iRoPE: NoPE layers skip rotation (rope hosts' identity sentinel)
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd;
        let n1 = self.rmsnorm_t(x, &w.g1, t, d, eps);
        let q = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wq.apply(self, &n1, t), &w.q_norm, t, ad, hd, eps), ff, t, ad, base, 0, hd), w, t, ad, hd, eps);
        let q = match w.attn_temp {
            Some(at) if w.nope && at.active_at(t.saturating_sub(1)) => self.scale_query_temp_rows(&q, at, 0, t, ad),
            _ => q,
        };
        let k = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wk.apply(self, &n1, t), &w.k_norm, t, kv, hd, eps), ff, t, kv, base, 0, hd), w, t, kv, hd, eps);
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
        let base_freq = w.rope_base.unwrap_or(base_freq);      // Gemma: windowed layers use freq_base_swa
        let base_freq = if w.nope { 0.0 } else { base_freq };   // Llama4 iRoPE NoPE switch
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd;
        let n1 = self.rmsnorm_t(x, &w.g1, s, d, eps);
        let q = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wq.apply(self, &n1, s), &w.q_norm, s, ad, hd, eps), ff, s, ad, base_freq, base, hd), w, s, ad, hd, eps);
        let q = match w.attn_temp {
            Some(at) if w.nope && at.active_at(base + s.saturating_sub(1)) => self.scale_query_temp_rows(&q, at, base, s, ad),
            _ => q,
        };
        let k = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wk.apply(self, &n1, s), &w.k_norm, s, kv, hd, eps), ff, s, kv, base_freq, base, hd), w, s, kv, hd, eps);
        let v = w.wv.apply(self, &n1, s);
        self.write_rows(kc, &k, base, s, kv);
        self.write_rows(vc, &v, base, s, kv);
        // per-row sliding window, from this layer's config (None = causal only)
        let attn = self.attention_mh_ext_swa(&q, kc, vc, base, s, ad, n_heads, n_kv_heads, w.swa);
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
        // BATCH PREFILL INTO A FRAME. Prefill never opened one, so every dispatch took `dispatch`'s
        // no-frame branch and became its own command buffer — the same defect just fixed in decode, in
        // the path that dominates wall time for every MoE model measured (Qwen3-30B: 71 s of 75 s;
        // Scout: 196 s of a 200 s turn). A MoE prefill issues one dispatch group per EXPERT per layer
        // per chunk, so the count is large and the per-submit cost is paid on all of it.
        //
        // The router readbacks inside still flush, but `download` now reopens the frame, so each one
        // ends a batch instead of ending batching.
        self.begin_frame();
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
        // Close the frame we opened: leaving it open would hand a live encoder to whatever runs next,
        // and `begin_frame` would then flush it at an arbitrary point rather than here.
        self.end_frame();
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
// Fused RMSNorm + int8 mat-vec (decode m=1): folds the input row's RMSNorm into the matvec's
// prologue so a norm→matmul pair becomes ONE dispatch. On Metal, where per-dispatch encoder setup
// (not GPU sync) is the decode wall, removing a dispatch is removing real time — and this pair fires
// TWICE per layer (attention-in, FFN-in). Each workgroup independently computes the row's
// inv = 1/sqrt(mean(x²)+eps) via a k-length reduction, then reads a[kk]·inv·nw[kk] in place of
// a[kk]. TRADEOFF: the reduction is recomputed once per output column (workgroup) — k extra input
// reads each — but the matmul's weight traffic is n·k and dominates bandwidth, so the redundant
// 2·k input reads are noise next to saving a whole dispatch's encoder setup. Net win exactly when
// setup cost > the extra reduction, which is the M4/Metal decode regime; LOGSCOPE_FUSE_NORM=0 backs
// out to the two-dispatch path for the A/B. Output is bit-identical to RMSNORM then MATVEC_Q8_0.
const RMSNORM_MATVEC_Q8: &str = r#"
struct D { m:u32,n:u32,k:u32,kb:u32,gx:u32,eps:u32,_a:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;      // raw (un-normed) input row [k]
@group(0)@binding(1) var<storage,read> nw: array<f32>;     // RMSNorm weight [k]
@group(0)@binding(2) var<storage,read> qw: array<u32>;
@group(0)@binding(3) var<storage,read> qs: array<f32>;
@group(0)@binding(4) var<storage,read_write> c: array<f32>;
@group(0)@binding(5) var<uniform> d: D;
var<workgroup> red: array<f32,64>;
var<workgroup> inv_sh: f32;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x;
  // --- RMS reduction over the input row (every workgroup; cheap vs the matmul) ---
  var ss=0.0; var j=tid;
  loop { if(j>=d.k){break;} let v=a[j]; ss=ss+v*v; j=j+64u; }
  red[tid]=ss; workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){ if(tid<s){ red[tid]=red[tid]+red[tid+s]; } workgroupBarrier(); }
  if(tid==0u){ inv_sh = 1.0/sqrt(red[0]/f32(d.k)+bitcast<f32>(d.eps)); }
  workgroupBarrier();
  let inv = inv_sh;
  if(col>=d.n){ return; }
  // --- int8 mat-vec over the normalized row ---
  let wbase=col*(d.k/4u); let sbase=col*d.kb;
  var acc=0.0; var kk=tid;
  loop {
    if(kk>=d.k){break;}
    let scale=qs[sbase+(kk/32u)];
    let word=qw[wbase+(kk/4u)];
    let ub=extractBits(word,(kk%4u)*8u,8u);
    let q=select(i32(ub), i32(ub)-256, ub>=128u);
    let an = a[kk]*inv*nw[kk];              // fused: normalize the activation in-place
    acc=acc+an*scale*f32(q);
    kk=kk+64u;
  }
  red[tid]=acc; workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){ if(tid<s){ red[tid]=red[tid]+red[tid+s]; } workgroupBarrier(); }
  if(tid==0u){ c[col]=red[0]; }
}"#;

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

// NEOX split-half RoPE (DeepSeek/Qwen/Gemma convention): rotates dim i with i+half, unlike ROPE's
// adjacent (2i, 2i+1) pairing. Used by the MLA forward; matches mla.rs's rope_neox reference exactly.
const ROPE_NEOX: &str = r#"
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
  let base=row*dd.d;
  let x0=x[base+i]; let x1=x[base+i+half];
  o[base+i]=x0*c-x1*s;
  o[base+i+half]=x0*s+x1*c;
}"#;

// ===== MoE v2 decode kernels: GPU-side routing (zero readbacks) =====
// MOE_TOPK: serial top-k over the router logits (n<=512, k<=16 — trivial work, the win is staying on-GPU).
// Selection order = descending logit; weights = softmax over the SELECTED logits (bit-faithful to
// moe::route's Softmax path: global softmax renormalized to the top-k == softmax over the selected).
// Output selw: [0..k) = expert index (u32), [k..2k) = combine weight (f32 bitcast to u32).
const MOE_TOPK: &str = r#"
// `mode` selects the WEIGHTING, not the selection: 0 = softmax over the chosen k, 1 = sigmoid of each
// logit. Selection is identical either way because sigmoid is monotonic, so top-k by raw logit picks the
// same experts — which is why this is a small change rather than a second kernel.
struct D { n:u32,k:u32,mode:u32,normalize:u32 };
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
  if (d.mode == 1u) {
    // Sigmoid gating (Llama-4, GLM). Each weight is sigmoid(logit) INDEPENDENTLY — there is no shared
    // denominator unless the model asks for one, so a softmax here would silently renormalize weights
    // the reference leaves alone.
    for(var s=0u;s<d.k;s=s+1u){ lg[s]=1.0/(1.0+exp(-lg[s])); sum=sum+lg[s]; }
  } else {
    for(var s=0u;s<d.k;s=s+1u){ lg[s]=exp(lg[s]-mx); sum=sum+lg[s]; }
  }
  // Softmax is normalized by construction; sigmoid only when the model says so.
  let dnm = select(1.0, sum, d.mode == 0u || d.normalize == 1u);
  for(var s=0u;s<d.k;s=s+1u){ selw[s]=idx[s]; selw[d.k+s]=bitcast<u32>(lg[s]/dnm); }
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

// Fused gate‖up + SwiGLU (Q8). The unfused path was: MOE_MATVEC (write [slots,2ff]) → SWIGLU_PACKED
// (reduce to [slots,ff]) — two dispatches + a 2ff scratch buffer. This does both in ONE dispatch: each
// workgroup owns output column j in [0,ff), reduces BOTH the gate row (col=j) and the up row (col=ff+j)
// against the same activation, and writes silu(gate)·up straight to o[slot·ff + j]. Bit-identical to the
// two-step path (same reductions, same silu(v)=v/(1+exp(-v))). Halves the launch grid (ff not 2ff) and
// drops the intermediate buffer. `d.n` here is ff (the OUTPUT width); gate/up rows live at col and col+ff
// of the stacked weight (row stride rstride = 2ff).
const MOE_MATVEC_GLU_Q8: &str = r#"
struct D { n:u32,k:u32,kb:u32,gx:u32,_slot:u32,rstride:u32,xper:u32,_p:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read> selw: array<u32>;
@group(0)@binding(4) var<storage,read_write> o: array<f32>;
@group(0)@binding(5) var<uniform> d: D;
var<workgroup> rg: array<f32,64>;
var<workgroup> ru: array<f32,64>;
fn dotq8(row:u32, tid:u32, k:u32, kb:u32, abase:u32) -> f32 {
  let wbase=row*(k/4u); let sbase=row*kb;
  var acc=0.0; var kk=tid;
  loop { if(kk>=k){break;}
    let scale=qs[sbase+(kk/32u)];
    let word=qw[wbase+(kk/4u)];
    let ub=extractBits(word,(kk%4u)*8u,8u);
    let q=select(i32(ub), i32(ub)-256, ub>=128u);
    acc=acc+a[abase+kk]*scale*f32(q);
    kk=kk+64u;
  }
  return acc;
}
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x; let slot=wg.z;
  if(col>=d.n){ return; }
  let abase=select(0u, slot*d.k, d.xper==1u);
  let expert=selw[slot];
  let grow=expert*d.rstride+col;         // gate row: column j
  let urow=expert*d.rstride+d.n+col;     // up row:   column ff+j
  rg[tid]=dotq8(grow,tid,d.k,d.kb,abase);
  ru[tid]=dotq8(urow,tid,d.k,d.kb,abase);
  workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){ if(tid<s){ rg[tid]=rg[tid]+rg[tid+s]; ru[tid]=ru[tid]+ru[tid+s]; } workgroupBarrier(); }
  if(tid==0u){ let g=rg[0]; o[slot*d.n+col]=(g/(1.0+exp(-g)))*ru[0]; }
}"#;

// Fused gate‖up + SwiGLU (Q4) — identical structure, 4-bit weights (nibble − 8).
const MOE_MATVEC_GLU_Q4: &str = r#"
struct D { n:u32,k:u32,kb:u32,gx:u32,_slot:u32,rstride:u32,xper:u32,_p:u32 };
@group(0)@binding(0) var<storage,read> a: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read> selw: array<u32>;
@group(0)@binding(4) var<storage,read_write> o: array<f32>;
@group(0)@binding(5) var<uniform> d: D;
var<workgroup> rg: array<f32,64>;
var<workgroup> ru: array<f32,64>;
fn dotq4(row:u32, tid:u32, k:u32, kb:u32, abase:u32) -> f32 {
  let wbase=row*(k/8u); let sbase=row*kb;
  var acc=0.0; var kk=tid;
  loop { if(kk>=k){break;}
    let scale=qs[sbase+(kk/32u)];
    let word=qw[wbase+(kk/8u)];
    let nib=extractBits(word,(kk%8u)*4u,4u);
    let q=i32(nib)-8;
    acc=acc+a[abase+kk]*scale*f32(q);
    kk=kk+64u;
  }
  return acc;
}
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x; let slot=wg.z;
  if(col>=d.n){ return; }
  let abase=select(0u, slot*d.k, d.xper==1u);
  let expert=selw[slot];
  let grow=expert*d.rstride+col;
  let urow=expert*d.rstride+d.n+col;
  rg[tid]=dotq4(grow,tid,d.k,d.kb,abase);
  ru[tid]=dotq4(urow,tid,d.k,d.kb,abase);
  workgroupBarrier();
  for(var s=32u;s>0u;s=s>>1u){ if(tid<s){ rg[tid]=rg[tid]+rg[tid+s]; ru[tid]=ru[tid]+ru[tid+s]; } workgroupBarrier(); }
  if(tid==0u){ let g=rg[0]; o[slot*d.n+col]=(g/(1.0+exp(-g)))*ru[0]; }
}"#;

// Fused down-projection + weighted-combine + residual (Q8). The unfused path was: MOE_MATVEC (down,
// write [slots,d]) → MOE_WSUM (out[col] = resid[col] + Σ_s w[s]·down[s·d+col]) — two dispatches + a
// [slots,d] scratch. This does both in ONE: one workgroup per output column `col` loops over the selected
// experts, reduces each expert's down row against that slot's hmid activation, accumulates weight[s]·result
// (weights are the f32-bitcast tail of selw), adds the residual, and writes o[col]. Bit-identical to the
// two-step path (same reductions, same combine order s=0..slots). Removes 1 dispatch/MoE-layer + scratch.
const MOE_DOWN_COMBINE_Q8: &str = r#"
struct D { d:u32,k:u32,kb:u32,gx:u32,slots:u32,_r:u32,_x:u32,_p:u32 };
@group(0)@binding(0) var<storage,read> hmid: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read> selw: array<u32>;
@group(0)@binding(4) var<storage,read> resid: array<f32>;
@group(0)@binding(5) var<storage,read_write> o: array<f32>;
@group(0)@binding(6) var<uniform> d: D;
var<workgroup> red: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x;
  if(col>=d.d){ return; }
  var total=resid[col];   // match MOE_WSUM order: acc=resid then += weighted terms
  for(var s=0u;s<d.slots;s=s+1u){
    let expert=selw[s];
    let w=bitcast<f32>(selw[d.slots+s]);
    let row=expert*d.d+col;
    let wbase=row*(d.k/4u); let sbase=row*d.kb; let abase=s*d.k;
    var acc=0.0; var kk=tid;
    loop { if(kk>=d.k){break;}
      let scale=qs[sbase+(kk/32u)];
      let word=qw[wbase+(kk/4u)];
      let ub=extractBits(word,(kk%4u)*8u,8u);
      let q=select(i32(ub), i32(ub)-256, ub>=128u);
      acc=acc+hmid[abase+kk]*scale*f32(q);
      kk=kk+64u;
    }
    red[tid]=acc; workgroupBarrier();
    for(var st=32u;st>0u;st=st>>1u){ if(tid<st){ red[tid]=red[tid]+red[tid+st]; } workgroupBarrier(); }
    if(tid==0u){ total=total+w*red[0]; }
    workgroupBarrier();
  }
  if(tid==0u){ o[col]=total; }
}"#;

// Fused down + combine + residual (Q4) — identical structure, 4-bit weights (nibble − 8).
const MOE_DOWN_COMBINE_Q4: &str = r#"
struct D { d:u32,k:u32,kb:u32,gx:u32,slots:u32,_r:u32,_x:u32,_p:u32 };
@group(0)@binding(0) var<storage,read> hmid: array<f32>;
@group(0)@binding(1) var<storage,read> qw: array<u32>;
@group(0)@binding(2) var<storage,read> qs: array<f32>;
@group(0)@binding(3) var<storage,read> selw: array<u32>;
@group(0)@binding(4) var<storage,read> resid: array<f32>;
@group(0)@binding(5) var<storage,read_write> o: array<f32>;
@group(0)@binding(6) var<uniform> d: D;
var<workgroup> red: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let col=wg.y*d.gx+wg.x; let tid=lid.x;
  if(col>=d.d){ return; }
  var total=resid[col];   // match MOE_WSUM order: acc=resid then += weighted terms
  for(var s=0u;s<d.slots;s=s+1u){
    let expert=selw[s];
    let w=bitcast<f32>(selw[d.slots+s]);
    let row=expert*d.d+col;
    let wbase=row*(d.k/8u); let sbase=row*d.kb; let abase=s*d.k;
    var acc=0.0; var kk=tid;
    loop { if(kk>=d.k){break;}
      let scale=qs[sbase+(kk/32u)];
      let word=qw[wbase+(kk/8u)];
      let nib=extractBits(word,(kk%8u)*4u,4u);
      let q=i32(nib)-8;
      acc=acc+hmid[abase+kk]*scale*f32(q);
      kk=kk+64u;
    }
    red[tid]=acc; workgroupBarrier();
    for(var st=32u;st>0u;st=st>>1u){ if(tid<st){ red[tid]=red[tid]+red[tid+st]; } workgroupBarrier(); }
    if(tid==0u){ total=total+w*red[0]; }
    workgroupBarrier();
  }
  if(tid==0u){ o[col]=total; }
}"#;
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

// Row-broadcast scale: o[r,c] = x[r,c]·w[r]. The Llama4 weight-before-FFN step — the router gate
// multiplies the expert's INPUT rows (not equivalent to output-weighting through SwiGLU).
const ROW_SCALE: &str = r#"
struct D { n:u32,c:u32,_b:u32,_d:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read> w: array<f32>;
@group(0)@binding(2) var<storage,read_write> o: array<f32>;
@group(0)@binding(3) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; if(i>=d.n*d.c){return;}
  o[i]=x[i]*w[i/d.c]; }"#;

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

// GeGLU-packed: gemma-family MLP gate. Same [gate|up] packed layout as SWIGLU_PACKED but the gate uses
// tanh-approx GELU (gelu_pytorch_tanh) instead of SiLU: gelu(v)·up. The building block for Gemma2 FFN.
const GEGLU_PACKED: &str = r#"
struct D { n:u32,h:u32,_b:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> gu: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>){
  let i=g.y*nwg.x*64u+g.x; if(i>=d.n){return;} let r=i/d.h; let c=i%d.h; let base=r*2u*d.h;
  let v=gu[base+c];
  let gelu=0.5*v*(1.0+tanh(0.7978845608028654*(v+0.044715*v*v*v)));  // √(2/π)=0.79788456
  o[i]=gelu*gu[base+d.h+c];
}"#;

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
      // NaN/Inf guard (Colibri #369): a poisoned logit must not be selected. Reading non-finite as
      // -inf excludes it from the max so the pick stays a finite, in-range token id (a lone +Inf would
      // otherwise win and skew the softmax; a NaN sentinel could leave bi out of range).
      var v = x[i];
      if (!(v == v) || v > 3.0e38 || v < -3.0e38) { v = -3.4028235e38; }
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
    /// MLA (Multi-head Latent Attention) rank config, present only for DeepSeek-V3/GLM-MoE checkpoints.
    /// When set, the KV cache stores the compressed latent instead of per-head K/V. `None` = standard MHA/GQA.
    pub mla: Option<MlaConfig>,
    /// Multiplier applied to the token embedding before the first block.
    ///
    /// Gemma scales by sqrt(d) (`inpL = ggml_scale(inpL, sqrtf(n_embd))` in every gemma*.cpp). At
    /// d=3840 that is a factor of ~62 at the input, and it cascades — a model missing it does not
    /// produce slightly-off output, it produces nonsense. 1.0 everywhere else, so no other model changes.
    pub embed_scale: f32,
    /// DSA (DeepSeek Sparse Attention) lightning-indexer config, present only when the indexer weights ship.
    /// When set (and seq > index_topk) attention is sparse; `None` = dense attention.
    pub dsa: Option<DsaConfig>,
    /// Number of trailing multi-token-prediction (MTP / nextn_predict) blocks the checkpoint ships (GLM-5.2,
    /// DeepSeek-V3). These aren't standard transformer layers (loaded depth excludes them), but recording
    /// the count marks that a native MTP draft head is PRESENT — the prerequisite for wiring MTP speculative
    /// decode (a lossless speedup, like the n-gram draft) later. 0 = no MTP head.
    pub mtp_layers: usize,
}

/// Multi-head Latent Attention config (DeepSeek-V2/V3, GLM-MoE-DSA). The dims that make MLA's compressed
/// KV cache work: instead of caching `n_kv_heads·head_dim` per token, MLA caches only the down-projected
/// latent (`kv_lora`) plus the shared rotary part (`qk_rope`) — e.g. 512+64=576 vs 32768 values/token —
/// and reconstructs per-head K (nope part) and V from the latent via the kv_b up-projection at use time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MlaConfig {
    /// Rank of the Q down-projection (0 = no Q compression; Q is projected directly). DeepSeek-V3: 1536.
    pub q_lora: usize,
    /// Rank of the KV down-projection — the width of the cached latent. DeepSeek-V3: 512.
    pub kv_lora: usize,
    /// Per-head non-rotary key/query dim (reconstructed from the latent). DeepSeek-V3: 128.
    pub qk_nope: usize,
    /// Per-head rotary key/query dim (RoPE part, shared across heads in the cache). DeepSeek-V3: 64.
    pub qk_rope: usize,
    /// Per-head value dim (reconstructed from the latent). DeepSeek-V3: 128.
    pub v_head: usize,
}

impl MlaConfig {
    /// Cached latent width per token = the compressed KV row. This is the memory win: compare to
    /// `n_kv_heads · head_dim` for standard KV.
    pub fn latent_width(&self) -> usize { self.kv_lora + self.qk_rope }
    /// Full per-head query/key dim (nope + rope), the width attention scores are computed over.
    pub fn qk_head(&self) -> usize { self.qk_nope + self.qk_rope }
}

/// DSA (DeepSeek Sparse Attention) lightning-indexer config, present only for GLM-MoE-DSA / DeepSeek
/// checkpoints that ship the indexer weights. The indexer scores every key with a small separate
/// attention (index_n_heads × index_head_dim) and, when the sequence is long, keeps only the top
/// `index_topk` keys for the main attention — making attention sub-quadratic at long context. Crucially
/// it's a NO-OP for seq ≤ index_topk (selects all keys → dense, exact), so short contexts are unchanged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DsaConfig {
    /// How many keys the indexer keeps per query once the sequence exceeds this length. Below it: all keys.
    pub index_topk: usize,
    /// Indexer attention heads (separate, small).
    pub index_n_heads: usize,
    /// Indexer per-head dim.
    pub index_head_dim: usize,
}
impl DsaConfig {
    /// Whether sparse selection is active at this sequence length. At or below `index_topk` the indexer is
    /// a no-op (all keys selected → dense-and-exact), so DSA can never regress short contexts — the
    /// property that makes it safe to enable unconditionally. Mirrors Colibri glm.c: `seq > index_topk`.
    pub fn sparse_active(&self, seq_len: usize) -> bool { seq_len > self.index_topk }
    /// Keys actually attended at this length: min(seq_len, index_topk) once past the threshold.
    pub fn selected_keys(&self, seq_len: usize) -> usize {
        if self.sparse_active(seq_len) { self.index_topk } else { seq_len }
    }
}
impl Model {
    /// Weight PARAMETERS read for one decoded token.
    ///
    /// This is the numerator of the roofline bound, and it is architecture arithmetic rather than a
    /// stored figure — so it cannot drift from what the model actually is.
    ///
    /// Parameters, not bytes, deliberately: the caller knows the residency format (q4, int8) and this
    /// does not. Multiplying by a bytes-per-parameter guessed here would produce a confident figure
    /// from an assumption, which is the shape of every memory bug in this codebase.
    ///
    /// `n_active_experts` is the routed count for an MoE and 0 for a dense model; the FFN is counted
    /// once per active expert plus any shared one, because that is what decode reads.
    pub fn active_params_per_token(&self, n_active_experts: usize) -> u64 {
        active_params_per_token(self.d, self.h, self.vocab, self.n_layers,
                                self.n_kv_heads, self.head_dim, n_active_experts)
    }
}

/// The same arithmetic as a free function, so it can be tested without constructing a whole `Model`.
///
/// Extracted after the test could not build one: a figure this load-bearing should not be untestable
/// because its container is awkward to make.
pub fn active_params_per_token(d: usize, ff: usize, vocab: usize, n_layers: usize,
                               n_kv_heads: usize, head_dim: usize, n_active_experts: usize) -> u64 {
    {
        let (d, ff) = (d as u64, ff as u64);
        let kv = (n_kv_heads * head_dim) as u64;
        // Attention: q and o are d x d; k and v are d x (n_kv * head_dim), which is SMALLER under GQA
        // and is the whole point of GQA — assuming four d-by-d matrices overstates it by ~40% on a
        // 32/8 model.
        let attn = 2 * d * d + 2 * d * kv;
        // SwiGLU: gate and up are d x ff, down is ff x d.
        let one_ffn = 3 * d * ff;
        let ffn = if n_active_experts == 0 { one_ffn } else { one_ffn * n_active_experts as u64 };
        (attn + ffn) * n_layers as u64
            // The output projection is read every token; the embedding table is a LOOKUP, not a matmul,
            // so only one row of it moves and it does not belong in a per-token bandwidth figure.
            + d * vocab as u64
    }
}

impl Model {

    /// Cache key fragment describing what kind of decode this model performs. MoE and dense decode have
    /// very different dispatch profiles (a MoE layer issues per-expert work; a dense one does not), so
    /// anything tuned per-GPU must also be keyed on this — a verdict measured on a dense model does not
    /// transfer to a MoE one.
    pub fn decode_class(&self) -> &'static str { if self.is_moe() { "moe" } else { "dense" } }

    /// Resident weight format, as a tuning-key component. A lever can pay at one precision and not
    /// another, so a verdict measured at q4 must not be reused for an int8-resident load.
    /// (n_expert, n_used) when this model routes, for prefill chunk sizing. None for dense models.
    ///
    /// Read from the first MoE layer: every routed layer in a checkpoint shares the same counts, and a
    /// model that mixed them would need per-layer chunking, which nothing supports.
    pub fn moe_counts(&self) -> Option<(usize, usize)> {
        self.layers.iter().find_map(|b| b.moe.as_ref())
            .map(|m| (m.cfg.n_routed as usize, m.cfg.n_used as usize))
    }

    /// Does any layer stream its experts from host memory? Changes what the prefill chunk should optimise
    /// for: refill traffic rather than dispatch duration.
    /// Routed experts activated per token, or 0 for a dense model.
    ///
    /// Read from the first MoE layer's own config rather than a constant: this is the number that makes
    /// a 30B-A3B read like a 3B, and getting it from anywhere but the model would put the roofline
    /// bound an order of magnitude out.
    pub fn n_active_experts(&self) -> usize {
        self.layers.iter().find_map(|b| b.moe.as_ref().map(|m| m.cfg.n_used as usize)).unwrap_or(0)
    }

    pub fn streams_experts(&self) -> bool {
        self.layers.iter().any(|b| b.moe.as_ref().is_some_and(|m| m.stream.is_some()))
    }

    pub fn residency_class(&self) -> &'static str {
        if self.layers.iter().any(|b| b.wq.is_q4()) { "q4" } else { "int8" }
    }
    /// True if any block is a routed-MoE layer — the dispatch levers (bind-group cache, MoE fusions) only
    /// bear on MoE decode, so the magical A/B autotune skips the probe entirely on dense models.
    pub fn is_moe(&self) -> bool { self.layers.iter().any(|b| b.moe.is_some()) }

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
    /// Whether the checkpoint ships a native multi-token-prediction draft head (GLM-5.2/DeepSeek-V3). When
    /// true, an MTP speculative-decode path could draft tokens from the head (lossless, verified) — not yet
    /// wired, but the presence is now recorded rather than silently dropped at load.
    pub fn has_mtp(&self) -> bool { self.mtp_layers > 0 }
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
        self.load_gguf_quantized_parts(&[bytes])
    }

    /// Shard-aware twin of [`load_gguf_quantized`], preserving its int8 → q4 OOM fallback. Split
    /// checkpoints must go through here rather than `load_gguf_parts` directly, or a model that needs the
    /// q4 retry would fail outright instead of shrinking.
    pub fn load_gguf_quantized_parts(&self, parts: &[&[u8]]) -> Result<Model, String> {
        self.quantized_parts_impl(parts, None)
    }

    /// Shard-aware AND view-capable: the shards outlive the loader, so MoE experts can point at the file.
    pub fn load_gguf_quantized_parts_static(&self, parts: &[&'static [u8]]) -> Result<Model, String> {
        self.quantized_parts_impl(parts, Some(parts))
    }

    fn quantized_parts_impl(&self, parts: &[&[u8]], stat: Option<&[&'static [u8]]>) -> Result<Model, String> {
        let total: usize = parts.iter().map(|p| p.len()).sum();
        let forced_q4 = std::env::var("LOGSCOPE_Q4_RESIDENT").is_ok();
        match self.try_load_resident_impl(parts, stat)? {
            Some(m) => return Ok(m),
            None if forced_q4 => return Err(Self::too_big_msg(total)),
            None => {}
        }
        self.pool.free.lock().unwrap().clear();
        eprintln!("wgpu-engine: int8-resident exceeded the GPU's allocatable memory — retrying as q4-resident (≈half the footprint)…");
        std::env::set_var("LOGSCOPE_Q4_RESIDENT", "1");
        let r = self.try_load_resident_impl(parts, stat);
        std::env::remove_var("LOGSCOPE_Q4_RESIDENT");
        return match r? {
            Some(m) => Ok(m),
            None => Err(Self::too_big_msg(total)),
        };
    }

    #[allow(dead_code)]
    fn load_gguf_quantized_single(&self, bytes: &[u8]) -> Result<Model, String> {
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
             engine can't), or run a smaller model.\n\n\
             NOTE: the budget that admitted this load is a CONSTANT, not a measurement of what this \
device honours — every load logs that warning and it is the reason a model can pass the fit check and \
then fail allocating. `kernel-validate --gate-vram` finds the real ceiling, and LOGSCOPE_MAX_GPU_GB \
pins it once you know. Until then the planner is guessing high, which turns a clean refusal into a \
failed allocation partway through."
        )
    }
    /// Load inside an OutOfMemory error scope. Ok(Some) = loaded; Ok(None) = GPU OOM (captured, the
    /// process is NOT aborted); Err = a genuine (non-OOM) load error.
    fn try_load_resident(&self, bytes: &[u8]) -> Result<Option<Model>, String> {
        self.try_load_resident_parts(&[bytes])
    }

    fn try_load_resident_parts(&self, parts: &[&[u8]]) -> Result<Option<Model>, String> {
        self.try_load_resident_impl(parts, None)
    }

    fn try_load_resident_impl(&self, parts: &[&[u8]], stat: Option<&[&'static [u8]]>) -> Result<Option<Model>, String> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let res = self.load_gguf_inner(parts, stat, true);
        crate::counted_poll(&self.device);
        let oom = pollster::block_on(scope.pop());
        if oom.is_some() {
            drop(res.ok()); // discard any partial/poisoned buffers from the failed attempt
            return Ok(None);
        }
        res.map(Some)
    }
    pub fn load_gguf_opt(&self, bytes: &[u8], q: bool) -> Result<Model, String> {
        self.load_gguf_parts(&[bytes], q)
    }

    /// Load from a SPLIT checkpoint: `parts` are the shard files' bytes in shard order (a one-element
    /// slice is the ordinary single-file case and behaves identically). The shards are unified into one
    /// tensor namespace by `gguf::ShardSet` — shard 1's metadata describes the whole model, and every
    /// tensor read is routed to the file that actually holds those bytes. This is why a split model does
    /// not need merging on disk: a 62 GB checkpoint would otherwise have to be duplicated to be loadable.
    pub fn load_gguf_parts(&self, parts: &[&[u8]], q: bool) -> Result<Model, String> {
        self.load_gguf_inner(parts, None, q)
    }

    /// As `load_gguf_parts`, but the caller guarantees the shard bytes live for the process (they came
    /// from `retain_map`). Only this form can back MoE experts with VIEWS into the file — a borrow that
    /// dies with the loader cannot be handed to a decode path that runs for hours.
    pub fn load_gguf_parts_static(&self, parts: &[&'static [u8]], q: bool) -> Result<Model, String> {
        self.load_gguf_inner(parts, Some(parts), q)
    }

    fn load_gguf_inner(&self, parts: &[&[u8]], stat: Option<&[&'static [u8]]>, q: bool) -> Result<Model, String> {
        // A second view over the same bytes, carrying the 'static lifetime, used only to cut expert spans.
        let static_set: Option<gguf::ShardSet<'static>> = match stat {
            Some(p) => gguf::ShardSet::new(p.to_vec()).ok(),
            None => None,
        };
        let set = gguf::ShardSet::new(parts.to_vec())?;
        set.completeness()?;
        if set.shards() > 1 {
            eprintln!("wgpu-engine: split checkpoint — {} shards unified ({} tensors, {:.1} GiB of tensor data)",
                set.shards(), set.g.tensors.len(), set.data_len() as f64 / (1u64 << 30) as f64);
        }
        let g = &set.g;
        // Data-integrity gate (unconditional, ~8 KB): read a small F32 tensor's RAW bytes straight from the
        // mapped file — before any dequant, upload, or GPU involvement. An all-zeros norm weight is
        // impossible in a real checkpoint; it means the file's DATA section is corrupt (partial download /
        // bad copy — metadata parses fine, so size checks and dumps look healthy). This exact failure
        // burned a full debugging cycle on the M4 (weights probed zero on GPU; every engine path was
        // exonerated one by one). Refuse loudly with the fix instead of generating symbol soup.
        if let Some(t) = g.tensor("blk.0.attn_norm.weight").filter(|t| t.ggml_type == 0) {
            let off = t.offset;
            let n = (t.dims.iter().product::<u64>() as usize * 4).min(8192);
            if let Some(raw) = set.slice(off, n) {
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
        // If the file DECLARES an architecture we recognise and cannot run, refuse on that name — do not
        // let the key-prefix fallback rescue it into something else. The fallback exists to recover a
        // converter's mislabel, and it is right for an UNKNOWN declaration; but when a file plainly says
        // "gemma2" and we know gemma2 is unsupported, resolving it to "llama" because it happens to carry
        // llama.* keys means running the wrong forward pass and emitting fluent garbage. An explicit,
        // recognised declaration is stronger evidence than an inferred prefix.
        if let Some(declared) = g.declared_arch() {
            if declared != arch {
                let dcfg = crate::archcfg::for_arch(&declared);
                if !matches!(dcfg.arch, crate::archcfg::Arch::Unknown) && !dcfg.supported && !dcfg.dense_supported {
                    return Err(format!("this file DECLARES architecture '{declared}', which the wgpu engine \
does not support ({}). Its metadata keys use the '{arch}' prefix, so the loader could have treated it as \
'{arch}' — but running the wrong forward pass produces plausible-looking nonsense rather than an error, so \
it refuses instead. Use the llama.cpp backend for this model.", dcfg.note));
                }
                eprintln!("wgpu-engine: file declares architecture '{declared}' but its metadata keys use \
the '{arch}' prefix — proceeding as '{arch}' (a converter mislabel); report this if the model is really \
a '{declared}'");
            }
        }
        let mut acfg = crate::archcfg::for_arch(&arch);
        // MLA is detected by TENSOR PRESENCE, not arch name. The glm4moe family ships in two forms: some
        // use Multi-head Latent Attention (attn_kv_a/kv_b down/up projections — not wired yet), but others
        // use ordinary GQA attention (attn_q/k/v, often with QK-norm + bias) plus shared-expert MoE, which
        // we DO support. So only treat this arch as unsupported-MLA when the MLA projection tensors are
        // actually present; otherwise fall through to the standard attention+MoE path.
        let has_mla_tensors = g.tensor("blk.0.attn_kv_a_mqa.weight").is_some()
            || g.tensor("blk.0.attn_kv_a.weight").is_some()
            || g.tensor("blk.0.attn_kv_b.weight").is_some();
        let arch_is_mla_family = matches!(acfg.arch, crate::archcfg::Arch::DeepseekV3 | crate::archcfg::Arch::Glm4Moe);
        // If an MLA-family checkpoint actually ships MLA projection tensors, we can't run it yet (MLA
        // forward unwired) — refuse clearly. If it has standard attention tensors instead, proceed.
        // THE TENSORS DECIDE, NOT THE ARCH NAME. This required BOTH, so a checkpoint shipping MLA
        // projections under an architecture the table calls standard-attention fell through to the
        // dense loader and failed on a missing attn_q — which is what GLM-4.7 Flash does:
        //
        //   blk.0 is Multi-head Latent Attention (attn_q_a / attn_kv_a_mqa present, attn_q absent)
        //   and the MLA path did not engage, so the dense loader ran instead.
        //
        // A family list is a prediction about which architectures use MLA; `has_mla_tensors` is an
        // observation of what this file contains. When they disagree the file is right — the same
        // principle as deriving the latent dims from shapes rather than trusting metadata.
        //
        // Kept in the condition so the arch list still explains WHY when it is the surprising case.
        if has_mla_tensors {
            if !arch_is_mla_family {
                eprintln!("loader: '{arch}' is not in the MLA family list, and this checkpoint ships \
MLA projections anyway — taking the MLA path because the tensors are what the forward pass has to \
match. The family list is a prediction; the file is an observation");
            }
            // No longer a blanket refusal. The MLA forward, the compressed cache, the per-layer
            // projections, the decode block and the per-layer routing are all in place and checked
            // (--gate-mla-block plus the kernel-validation suite). What is still worth refusing is a
            // checkpoint whose MLA metadata is absent, because then the dims are unknown and nothing
            // downstream can be trusted — that is a real missing input, not an unimplemented feature.
            // SPARSE-ATTENTION BOUND, read rather than assumed. archfeat::Feature::indexer_needed_at
            // encodes Colibri's finding — the DSA indexer is a no-op for seq <= index_topk, because
            // top-k of everything is everything — and until now nothing supplied the number, so the
            // conditional was theoretical.
            //
            // Read from the file: a checkpoint that ships a smaller bound than GLM-5.2's 2048 needs
            // the indexer sooner, and one that ships none cannot be reasoned about this way at all.
            if let Some(topk) = g.get_u32(&format!("{}.attention.index_topk", acfg.prefix))
                .or_else(|| g.get_u32(&format!("{}.index_topk", acfg.prefix))) {
                eprintln!("loader: sparse-attention bound index_topk={topk} — dense causal attention \
is EXACT at or below that many tokens (the indexer selects every key there), and an approximation \
above it. Keep prompts under {topk} until the indexer is implemented");
            }
            // DERIVE FROM THE SHAPES WHEN THE METADATA OMITS THE RANK. GLM-5.2's preview ships MLA
            // tensors and no kv_lora_rank, and the refusal for it was honest but unnecessary: the
            // norm vector's LENGTH is the latent rank, and kv_a_mqa's width is rank + rope_dim, so
            // every dimension is recoverable from tensors the file already has.
            //
            // Safe here in a way it was not for the fused-qkv split, because the shapes OVER-determine
            // the answer — four constrain three — so mladims cross-checks them and refuses a file
            // whose tensors disagree rather than loading at a guessed rank.
            let derived = crate::mladims::derive("blk.0.", &|n: &str| {
                g.tensor(n).map(|t| t.dims.clone())
            });
            if let Ok(d) = &derived {
                if g.get_u32(&format!("{}.attention.kv_lora_rank", acfg.prefix)).unwrap_or(0) == 0 {
                    eprintln!("loader: MLA dims DERIVED from tensor shapes (metadata omits \
kv_lora_rank) — kv_lora {} q_lora {} rope_dim {} heads {} v_head_dim {}. The shapes cross-check, so \
this is a reading of the file rather than a guess",
                        d.kv_lora_rank, d.q_lora_rank, d.qk_rope_head_dim, d.n_head, d.v_head_dim);
                }
            }
            let have_cfg = g.get_u32(&format!("{}.attention.kv_lora_rank", acfg.prefix)).unwrap_or(0) > 0
                || derived.is_ok();
            if !have_cfg {
                let layout = g.dump_tensors("blk.0.");
                // The derivation was tried and failed; say WHY rather than repeating that the
                // metadata is silent. "Unknown" sends someone looking for a config key; the real
                // answer is usually a specific tensor that is absent or the wrong shape.
                if let Err(e) = &derived {
                    return Err(format!("architecture '{arch}' ships MLA tensors, the metadata omits \
kv_lora_rank, and the dimensions could not be recovered from the shapes either: {}\n\nTensor layout:\n{}",
                        e.explain(), layout));
                }
                return Err(format!("architecture '{arch}' ships MLA tensors but no kv_lora_rank in its \
metadata, so the latent dimensions are unknown. The MLA path is implemented and validated; this \
particular checkpoint cannot be sized. Use the llama.cpp backend for it.\n{layout}"));
            }
            // Only claim the path is ACTIVE if the weights actually loaded. Saying "per-layer routing
            // active" and then failing two lines later with "the MLA forward isn't wired yet" is a log
            // that contradicts itself, and the reader cannot tell which half to believe.
            // The split form is now READ (k_b + v_b concatenated), so it is no longer a blocker. What
            // remains unverified is whether the concatenation order and the 3-D-as-2-D flattening match
            // what mla.rs expects — the shapes line up and llama.cpp's declaration says they should, but
            // no MLA checkpoint has run end to end here.
            let split_kv_b = g.tensor("blk.0.attn_k_b.weight").is_some();
            eprintln!("wgpu-engine: MLA attention — compressed-latent KV cache, per-layer routing, \
{} kv_b layout. UNVERIFIED on a real checkpoint: the decode block and cache are gated, the weight \
layout is not.", if split_kv_b { "SPLIT k_b+v_b" } else { "fused" });
        }
        // DERIVED, not read off the entry. archfeat can veto a declared-supported architecture — the
        // direction that ships broken output — while a declared-unsupported one stays refused whatever
        // the feature table says, because "no gap I know of" is not "verified on a real checkpoint".
        // Variant = expert count, which distinguishes Llama-4 Scout (16) from Maverick (128). Read
        // from metadata here rather than threaded down, since this is the only place that needs it.
        let variant = g.get_u32(&format!("{}.expert_count", acfg.prefix)).unwrap_or(0);
        let treat_unsupported = !crate::archcfg::is_supported(&arch, variant);
        if treat_unsupported {
            // MoE-aware escape: some architectures (e.g. Qwen4) are only PARTIALLY unsupported — their MoE
            // routing kernel isn't wired, but a DENSE checkpoint of the same arch runs on the existing
            // forward path. Read the expert count from metadata: 0/absent ⇒ dense ⇒ allow; >0 ⇒ real MoE ⇒
            // refuse with the arch's precise note. Architectures with no dense form (Llama4) never escape.
            let expert_count = g.get_u32(&format!("{}.expert_count", acfg.prefix)).unwrap_or(0);
            let dense_ok = acfg.dense_supported && expert_count == 0;
            // BREAK THE DEADLOCK. `supported` flips only once an architecture has completed a forward
            // pass on a real checkpoint — which is the right rule, and it means an arch can never
            // become supported, because attempting it is what produces the evidence and attempting it
            // is what this refuses.
            //
            // GLM-5.2 is exactly there: its two expert-weighting gaps are implemented, its third is a
            // no-op below 2048 context, its MLA layout is fixed, and it cannot be RUN to find out.
            //
            // So: an explicit opt-in, named for what it is. This does not claim the model works — it
            // lets someone find out, which is the only path to the verification the flag demands.
            let forced = std::env::var_os("LOGSCOPE_TRY_UNSUPPORTED_ARCH").is_some();
            if forced {
                eprintln!("loader: '{arch}' is NOT verified — trying it anyway because \
LOGSCOPE_TRY_UNSUPPORTED_ARCH is set. Remaining gaps: {}. Wrong output is the EXPECTED failure here, \
not a surprise; coherent output is the evidence that would flip the flag",
                    crate::archfeat::gap_report(&arch, variant).join("; "));
            }
            if !dense_ok && !forced {
                let moe_hint = if expert_count > 0 { format!(" (this is a {expert_count}-expert MoE checkpoint)") } else { String::new() };
                // Append layer-0's tensors so the exact expert stacking layout is visible right here — the
                // one thing needed to wire the MoE forward, without a separate dump run.
                let layout = g.dump_tensors("blk.0.");
                // If archfeat knows this architecture, say WHAT is missing. "unknown architecture" is an
                // unhelpful answer for a model whose requirements have been transcribed from llama.cpp —
                // the difference between "nobody has looked at this" and "three specific things are
                // missing, one of them structural" is most of what a person needs to decide what to do.
                let known = crate::archfeat::gap_report(&arch, expert_count);
                let detail = if known.is_empty() { acfg.note.to_string() } else { known.join("; ") };
                // trim a trailing period on `detail` before adding our own: notes end with one about half
                // the time and "for now.. Use the" is the kind of sloppiness that makes a reader trust
                // the rest of the message less
                let detail = detail.trim_end_matches('.').to_string();
                return Err(format!("architecture '{arch}' isn't supported by the wgpu engine yet{moe_hint} — {detail}. \
                    Use the llama.cpp backend for this model, or set LOGSCOPE_TRY_UNSUPPORTED_ARCH=1 \
to attempt it anyway — an architecture becomes supported only once it has completed a forward pass on \
a real checkpoint, and this refusal is what prevents that ever happening. Wrong output is the EXPECTED \
failure when forcing it, not a surprise; coherent output is the evidence that would flip the flag.\n\
{layout}"));
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

        // AND STORE IT. The sniff wrote a local `pfx` and left acfg.prefix pointing at the
        // registry's value, so every later read through acfg.prefix silently returned None on a
        // file whose prefix differs — rope base, norm epsilon, expert_weights_scale, all of them.
        // Each defaulted quietly, and a model with the wrong rope base produces exactly the
        // near-random output reported for Qwen3.5 ("doctrine%," for "hi there").
        //
        // The loader NOTICED the mismatch and printed it. Noticing and not acting is worse than
        // not noticing: the log says the file's prefix is being used, and it was not.
        acfg.prefix = pfx;
        // Qwen3/3.5: real blk.N.attn_{q,k}_norm.weight tensors. Llama4: attn_qk_norm is set in the
        // registry but the GGUF carries NO norm-weight tensors — its QK-Norm is WEIGHTLESS (an
        // L2/RMS norm ≡ all-ones weights), applied on RoPE layers only; handled per-layer below.
        let is_llama4 = matches!(acfg.arch, crate::archcfg::Arch::Llama4);
        let has_qk_norm = acfg.attn_qk_norm && !is_llama4;
        // Llama4 iRoPE: every 4th layer uses NO positional encoding (llama.cpp hardcodes the same
        // step). NoPE layers also skip the weightless QK-Norm (it applies with RoPE only).
        // NoPE INTERVAL FROM METADATA, with 4 as the fallback. This was hardcoded to 4 — every fourth
        // layer skips rotation — which is Scout's documented pattern and an ASSUMPTION about every
        // other Llama-4 checkpoint, including Maverick and any future variant.
        //
        // A wrong interval applies RoPE to a layer that should have none and vice versa. The model
        // loads, every tensor is present, every shape matches, and the output is garbled — which is
        // exactly the Scout symptom that has survived every GPU-side fix on both platforms.
        //
        // Same shape as expert_weights_scale being "effectively hardcoded to 1": a value the file
        // carries, read from a constant instead.
        let nope_step = if is_llama4 {
            g.get_u32(&format!("{}.attention.no_rope_interval", acfg.prefix))
                .or_else(|| g.get_u32(&format!("{}.no_rope_interval", acfg.prefix)))
                .map(|v| v as usize)
                .unwrap_or(4)
        } else { 0 };
        if is_llama4 {
            // Printed because a wrong interval is invisible in the output: the model runs and the
            // text is merely bad, so the one place it can be caught is a line stating what was chosen.
            // EVERY VALUE THAT WAS A CONSTANT, AND WHERE IT CAME FROM. Five of these were hardcoded
            // and each produces text that is merely worse with every tensor present and every
            // dimension right — so a wrong one is invisible in the output and this line is the only
            // place it can be caught. Printing the SOURCE matters as much as the value: "4 (default)"
            // and "4 (file)" mean different things when the output is bad.
            // "(default)" IMPLIED THE FILE COULD HAVE SAID OTHERWISE, and for three of these four it
            // cannot: llama.cpp's key table defines `expert_shared_count`, `attention.scale` and
            // `attention.sliding_window_pattern` — and has NO key for the NoPE interval, for qk-norm,
            // or for the temperature floor. It derives all three from the architecture, exactly as our
            // constants did.
            //
            // So the metadata reads I added for those keys can never fire. They are harmless, and the
            // log line was not: an operator debugging Scout's output would read "(default)" as "this
            // checkpoint omitted it" and go looking for a file that could override it. There is none.
            //
            // A value nobody can override is an ARCH CONSTANT, and calling it a default was the same
            // overstatement as every other label this session — a word implying evidence that does not
            // exist.
            let src = |k: &str| if g.get_u32(&format!("{}.{k}", acfg.prefix)).is_some()
                { "file" } else { "default" };
            let arch_const = "arch constant — GGUF defines no key";
            eprintln!("loader: Llama-4 iRoPE — NoPE every {nope_step} layers from layer {} ({}); \
qk-norm {} ({}); shared experts {} ({}); attn-temp floor {} ({})",
                nope_step.saturating_sub(1), arch_const,
                if g.get_u32(&format!("{}.attention.use_qk_norm", acfg.prefix))
                    .map(|v| v != 0)
                    .unwrap_or_else(|| g.get_u32(&format!("{}.expert_count", acfg.prefix))
                        .unwrap_or(0) != 128) { "on" } else { "off" },
                arch_const,
                g.get_u32(&format!("{}.expert_shared_count", acfg.prefix)).unwrap_or(1),
                src("expert_shared_count"),
                g.get_f32(&format!("{}.attention.floor_scale", acfg.prefix)).unwrap_or(8192.0),
                arch_const);
        }
        // Which layers are windowed. llama.cpp's default pattern puts the FULL-attention layer last in
        // each group (dense_first = false), so a period of 4 gives [swa, swa, swa, dense].
        let swa_period = g.get_u32(&format!("{}.attention.sliding_window_pattern", acfg.prefix))
            .unwrap_or(if is_llama4 { 4 } else { 0 });
        let n_swa_layers = g.get_u32(&format!("{}.block_count", acfg.prefix)).unwrap_or(0) as usize;
        let swa_pattern: Vec<bool> = if swa_period > 0 {
            crate::swa::swa_layers(n_swa_layers, swa_period, false)
        } else { vec![true; n_swa_layers] };
        // One line naming any optimization that is NOT in its default state, so a run is reproducible
        // from its own log. Printing all twelve every load would be noise nobody reads; printing the
        // deviations makes an unusual result traceable to the lever that caused it.
        {
            let overridden: Vec<String> = crate::enginelevers::LEVERS.iter().filter_map(|l| {
                std::env::var(l.var).ok().map(|v| format!("{}={v}", l.var))
            }).collect();
            if !overridden.is_empty() {
                eprintln!("wgpu-engine: non-default levers set — {}", overridden.join(" "));
            }
        }
        // Newly enabled architectures say so. Every Gemma-3 requirement is implemented and unit-gated,
        // but no Gemma checkpoint has ever been loaded end to end here — and this project has repeatedly
        // shown that "validated in isolation" and "works on a real file" are different claims. Saying it
        // costs one line and sets the right expectation for the first person to try it.
        // Gemma 3 27B scales Q by 1/sqrt(n_embd/n_head) instead of 1/sqrt(head_dim) — the ONLY Gemma 3
        // where the hparam differs from our hardcoded default. llama.cpp identifies it by block count
        // (62), so we can name it precisely rather than warning every Gemma 3 user about a size they are
        // not running.
        if matches!(acfg.arch, crate::archcfg::Arch::Gemma3)
            && g.get_u32(&format!("{}.block_count", acfg.prefix)).unwrap_or(0) == 62 {
            let nh = g.get_u32(&format!("{}.attention.head_count", acfg.prefix)).unwrap_or(1).max(1);
            let nemb = g.get_u32(&format!("{}.embedding_length", acfg.prefix)).unwrap_or(0);
            return Err(format!("this is Gemma 3 27B, which scales attention by 1/sqrt(n_embd/n_head) = \
1/sqrt({}) rather than 1/sqrt(head_dim) — a hyperparameter the engine hardcodes. Smaller Gemma 3 models \
are unaffected and load fine. Use the llama.cpp backend for the 27B.", nemb / nh));
        }
        if matches!(acfg.arch, crate::archcfg::Arch::Gemma3) {
            eprintln!("wgpu-engine: gemma3 support is NEW — (1+w) norm, GELU gating, post-norms and \
sliding-window attention are each unit-gated, but no Gemma model has been run end to end. Report anything \
that looks wrong.");
        }
        // Declare any architecture requirement we do not implement, BEFORE the first token. Llama-4 cost
        // three rounds because partial support is indistinguishable from a quantization, streaming, or
        // conversion bug once the output is merely wrong rather than absent. The requirements come from
        // llama.cpp's per-architecture files; see archfeat.rs.
        {
            let variant = g.get_u32(&format!("{}.expert_count", acfg.prefix)).unwrap_or(0);
            for line in crate::archfeat::gap_report(&g.arch(), variant) {
                eprintln!("wgpu-engine: ⚠ {line}");
            }
        }
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
        // Expert FFN width. llama.cpp falls back to n_ff / n_expert_used when the key is absent
        // (qwen3moe.cpp:50), and that fallback matters: without it expert_ff is 0, `is_moe` goes false,
        // and a MoE checkpoint loads as DENSE — reading expert tensors as if they were a single FFN.
        // That is a silent structural misread, not a clean failure.
        let expert_ff = {
            let declared = g.get_u32(&format!("{pfx}.expert_feed_forward_length")).unwrap_or(0) as usize;
            if declared > 0 { declared } else {
                let n_used_meta = g.get_u32(&format!("{pfx}.expert_used_count")).unwrap_or(0) as usize;
                let ff_meta = g.get_u32(&format!("{pfx}.feed_forward_length")).unwrap_or(0) as usize;
                if n_used_meta > 0 && ff_meta > 0 {
                    let derived = ff_meta / n_used_meta;
                    eprintln!("loader: {pfx}.expert_feed_forward_length absent — deriving {derived} from \
feed_forward_length/{n_used_meta} (llama.cpp's fallback)");
                    derived
                } else { 0 }
            }
        };
        let is_moe = acfg.moe && n_expert > 0 && expert_ff > 0
            && (0..g.get_u32(&format!("{pfx}.block_count")).unwrap_or(0)).any(|b| g.tensor(&format!("blk.{b}.ffn_gate_exps.weight")).is_some());
        let h = match g.get_u32(&format!("{pfx}.feed_forward_length")) {
            Some(v) => v as usize,
            None if is_moe => expert_ff, // MoE GGUFs may omit the dense FFN width entirely
            None => return Err("missing feed_forward_length".into()),
        };
        let n_blocks = g.get_u32(&format!("{pfx}.block_count")).ok_or("missing block_count")? as usize;
        // GLM4-MoE / DeepSeek-V3 ship `nextn_predict_layers` extra "blocks" for multi-token prediction (MTP)
        // speculative heads — these are counted in block_count but are NOT standard transformer layers (no
        // attn_q/k/v), so loading them as layers fails. Drop them: the real transformer depth is the
        // remainder. Only trust the MTP count if those trailing blocks genuinely lack attention tensors.
        let mtp = g.get_u32(&format!("{pfx}.nextn_predict_layers")).unwrap_or(0) as usize;
        let n = if mtp > 0 && mtp < n_blocks
            && g.tensor(&format!("blk.{}.attn_q.weight", n_blocks - 1)).is_none() {
            eprintln!("loader: {mtp} nextn-predict (MTP draft head) block(s) present, not loaded as layers — {} transformer layers", n_blocks - mtp);
            n_blocks - mtp
        } else { n_blocks };
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
        // HOSTILE-DIMENSION GUARD (cf. Colibri's "absurd n from hostile model files" + matmul shape-overflow
        // checks): a corrupt or malicious GGUF header can carry dims that overflow the size arithmetic we do
        // below (d·vocab for the embed table, etc.) and wrap into a small allocation → out-of-bounds writes.
        // Metadata parses fine, so nothing else catches it. Reject absurd values up front with a clear
        // message instead of computing a wrapped size. Bounds are far above any real model (d≤1M, layers≤
        // 1024, heads≤4096, vocab≤16M) so no legitimate checkpoint is refused. Also require the products we
        // actually allocate to fit in usize via checked_mul.
        {
            let checks: [(&str, usize, usize); 6] = [
                ("embedding_length (d)", d, 1_048_576),
                ("block_count (layers)", n, 1_024),
                ("head_count", n_heads, 4_096),
                ("head_count_kv", n_kv_heads, 4_096),
                ("head_dim", head_dim, 65_536),
                ("feed_forward_length", h, 1_048_576),
            ];
            for (name, val, max) in checks {
                if val == 0 || val > max {
                    return Err(format!("GGUF hyperparameter '{name}' = {val} is out of the sane range (1..={max}) \
                        — the header is corrupt or hostile (a wrapped/absurd dim would overflow the allocation \
                        arithmetic). Re-download or verify the file."));
                }
            }
            if n_kv_heads > n_heads || n_heads % n_kv_heads != 0 {
                return Err(format!("GGUF head_count_kv ({n_kv_heads}) must divide head_count ({n_heads}) for \
                    GQA — the header is inconsistent."));
            }
            // the largest products we feed into allocations must not overflow usize
            let vocab_guess = vocab_meta.unwrap_or(0).max(256);
            if d.checked_mul(vocab_guess).and_then(|x| x.checked_mul(4)).is_none()
                || attn_dim.checked_mul(d).is_none() || h.checked_mul(d).is_none() {
                return Err("GGUF dimensions overflow the address space when sized for allocation — \
                    the header is corrupt or hostile.".into());
            }
        }
        // Bigger-model memory pre-flight: estimate the resident footprint from metadata (cheap — tensor
        // dims only, no dequant) and refuse gracefully WITH the number instead of OOMing mid-load.
        let vocab_est = vocab_meta.unwrap_or_else(|| g.tensor("token_embd.weight")
            .map(|t| (t.dims.iter().product::<u64>() as usize) / d).unwrap_or(0));
        // Residency planning (memplan): prefer int8; auto-fall-back to q4 when int8 won't fit the budget;
        // refuse (with a number) only when even q4 won't fit. LOGSCOPE_Q4_RESIDENT forces q4 regardless.
        let mut auto_q4 = false;
        let mut moe_stream_pin: Option<usize> = None; // Some(pin_count) when MoE expert streaming is planned
        if vocab_est > 0 {
            if let Some(budget) = store::resident_budget_bytes() {
                // MoE: the FFN weight mass is the EXPERTS — n_expert stacked [expert_ff, d] matrices per
                // kind — not the dense feed_forward_length. Sizing with `h` alone made plan() see the 30B
                // MoE as a ~5 GiB model → int8 → ~31 GiB real → BO VMA death. Effective width fixes the est.
                let ffn_eff = if is_moe { n_expert * expert_ff } else { h };
                let shape = crate::memplan::ModelShape { d, ffn: ffn_eff, layers: n, vocab: vocab_est, n_heads, n_kv_heads, tied_embed: g.tensor("output.weight").is_none() };
                if q {
                    use crate::memplan::{Fit, Residency, gib, plan, gpu_buffer_budget_uma};
                    // Weights live in GPU BUFFERS. On a DISCRETE/staging path the total is capped far below
                    // RAM (Asahi BO VMA death on 40–70 GiB staged uploads). On UNIFIED memory the zero-copy
                    // map-at-creation arena doesn't stage, so the cap doesn't apply — the budget scales with
                    // physical RAM and a 62 GiB MoE loads resident on a 192 GiB box (no streaming needed).
                    let gpu_budget = gpu_buffer_budget_uma(budget, self.conservative_vma, self.unified);
                    match plan(&shape, gpu_budget, 4096, 2, 256 * (1 << 20)) {
                        Fit::Resident { fmt: Residency::Int8, .. } => {} // int8 fits — the default
                        Fit::Resident { fmt: Residency::Q4, weight_bytes, .. } => {
                            auto_q4 = true;
                            eprintln!("loader: int8 won't fit the ~{:.1} GiB GPU-buffer budget → auto-selecting q4-resident (~{:.1} GiB)", gib(gpu_budget), gib(weight_bytes));
                        }
                        Fit::Resident { fmt: Residency::F32, .. } => {}
                        Fit::Streaming { .. } => {
                            // MoE EXPERT STREAMING (LOGSCOPE_MOE_STREAM): the model won't fit with every
                            // expert resident, but experts are the whole reason — if we pin the hot ones and
                            // stream the cold ones from host RAM, only (dense + pinned + ring) is resident.
                            // Check whether THAT fits; if so, proceed (q4) and let the MoE loader stream.
                            // MoE EXPERT STREAMING. Auto-enabled: at this point we know the checkpoint is
                            // MoE, that it will not fit resident, and (below) whether streaming would fit.
                            // Requiring an env var to unlock the one strategy that works — while printing
                            // a message saying it is implemented — makes the engine fail at something it
                            // knows how to do. LOGSCOPE_MOE_STREAM=0 opts out.
                            let stream_opt_out = std::env::var("LOGSCOPE_MOE_STREAM").as_deref() == Ok("0");
                            if is_moe && !stream_opt_out {
                                use crate::memplan::{Residency, resident_weight_bytes, kv_cache_bytes, gib};
                                // dense residency = whole model MINUS the expert FFN mass (ffn=0 shape) at q4,
                                // + KV + a transient staging spill. Experts are sized separately.
                                let dense_shape = crate::memplan::ModelShape { d, ffn: 0, layers: n, vocab: vocab_est, n_heads, n_kv_heads, tied_embed: g.tensor("output.weight").is_none() };
                                let kv = kv_cache_bytes(&dense_shape, 4096, 2);
                                // Under streaming the embedding is resident as q8 (resident_embed), not f32:
                                // reclaim the f32→q8 delta (~2.9 GiB on a Scout-class vocab) from the budget.
                                let embed_params = (vocab_est as u64) * (d as u64);
                                let embed_saving = embed_params * 4 - embed_params * 9 / 8; // f32 − (i8 + f32 scales/32)
                                let dense = (resident_weight_bytes(&dense_shape, Residency::Q4) + kv + 256 * (1 << 20)).saturating_sub(embed_saving);
                                // IMPLEMENTATION GRANULARITY: slots are per expert-LAYER, and EVERY layer keeps
                                // (pin + ring) expert-layers resident. So one "pinned expert" costs
                                // layers × per-expert-layer bytes. Size the pin count to the budget accordingly.
                                let per_expert_layer = ((2 * expert_ff * d) as u64 + (d * expert_ff) as u64) * 9 / 16; // (gu+dn) q4
                                let per_expert_all_layers = (n as u64) * per_expert_layer;
                                let ring = (n_expert_used as u64 + 1).max(2); // per-layer streaming ring
                                let ring_cost = (n as u64) * ring * per_expert_layer; // ring resident across all layers
                                // WOULD IT SIMPLY FIT? This check did not exist: the branch was entered for ANY
                    // MoE model, so Scout streamed 16 experts through a staging ring against a budget
                    // that holds all of them. Streaming costs a host copy of every expert, the ring's
                    // completion fence, and most of the run's submits — all to avoid an allocation
                    // that succeeds.
                    //
                    // The budget rose from 24 to 48 GiB in _340 on llama.cpp's evidence, which is what
                    // makes this reachable now; at 24 the answer was genuinely no.
                    let full_resident = dense + (n_expert as u64) * per_expert_all_layers;
                    if full_resident <= gpu_budget {
                        eprintln!("wgpu-engine: MoE fits FULLY RESIDENT ({:.1} GiB of {:.1} GiB budget) \
— no expert streaming, no host copy, no staging ring. Set LOGSCOPE_MOE_STREAM=1 to force streaming.",
                            gib(full_resident), gib(gpu_budget));
                        auto_q4 = true;
                        // moe_stream_pin stays None, which is what selects the resident path below.
                    } else if dense + ring_cost > gpu_budget {
                                    return Err(format!("MoE won't fit even with expert streaming: dense ~{:.1} GiB + a {}-slot ring ~{:.1} GiB exceeds ~{:.1} GiB.{} Use llama.cpp or Metal.", gib(dense), ring, gib(ring_cost), gib(gpu_budget),
                            // NAME THE FIX, not just the number. "It does not fit" is a dead end;
                            // "it does not fit at 4.5 bits and would at 1.56" names the work that
                            // changes the answer and lets a reader judge whether it is worth doing.
                            //
                            // Only shown when the source format is genuinely cheaper than the resident
                            // one — for Q4_K the conversion is size-neutral and a native path would buy
                            // nothing, so offering it there would send someone after 0 GiB.
                            {
                                let params = (dense + (n_expert as u64) * per_expert_all_layers)
                                    .saturating_mul(8) / 45 * 10;   // undo 4.5 bpw to weights
                                // The expert tensors' own type, looked up rather than assumed: the
                                // dense weights may be a different format from the experts, and it is
                                // the experts that dominate this budget.
                                let et = ["ffn_gate_exps", "ffn_up_exps", "ffn_down_exps"].iter()
                                    .find_map(|n| g.tensor(&format!("blk.0.{n}.weight"))
                                        .map(|t| t.ggml_type))
                                    .unwrap_or(u32::MAX);
                                match crate::memplan::conversion_cost(et, params, 4.5) {
                                    Some((res, nat)) if nat < res => format!(
                                        " The source is {:.2} bpw and the engine converts it to 4.5 \
bpw q4 before running: {:.1} GiB native against {:.1} GiB resident, so a native kernel for this \
format would save ~{:.1} GiB and is what would make this fit.",
                                        crate::memplan::source_bpw(et).unwrap_or(0.0),
                                        gib(nat), gib(res), gib(res.saturating_sub(nat))),
                                    _ => String::new(),
                                }
                            }));
                                }
                                let pin_fit = ((gpu_budget - dense - ring_cost) / per_expert_all_layers) as usize;
                                let pin = std::env::var("LOGSCOPE_MOE_PIN").ok().and_then(|s| s.parse().ok()).unwrap_or(pin_fit).min(pin_fit).min(n_expert);
                                // HOST COST, stated before any work happens. Streaming keeps every routed
                                // expert's requantized weights in host RAM, and the loader also touches the
                                // whole GGUF (so the file lands in page cache). Both are large, and the
                                // failure mode without this check is an OOM kill ~18 minutes into a load —
                                // after the expensive dequant/requant pass, with nothing to show for it.
                                // ZERO WHEN THE VIEW PATH CAN TAKE IT. This figure is the cost of the HOST-COPY
                    // path — dequantize every expert and requantize it into anonymous memory. The
                    // file-view path reads experts straight from the mmap and converts Q4_K to q4 on
                    // the GPU, exactly as the dense weights already are, and costs no host RAM at all.
                    //
                    // The check ran before that path was attempted, so Scout was refused for 50.6 GiB
                    // it may never allocate. Asking the cheap question first — are the expert tensors
                    // a type the view path reads, and do gate and up agree — costs a tensor-table
                    // lookup and no memory.
                    //
                    // Conservative by construction: a false yes only means the load proceeds to a more
                    // precise refusal at the real attempt, while a false no refuses a model that would
                    // have run.
                    let ty = |n: &str| g.tensor(&format!("blk.0.{n}.weight")).map(|t| t.ggml_type);
                    let views_ok = crate::memplan::file_views_plausible(
                        ty("ffn_gate_exps"), ty("ffn_up_exps"), ty("ffn_down_exps"),
                        std::env::var_os("LOGSCOPE_Q6K_VIEWS").is_some());
                    if views_ok {
                        eprintln!("loader: MoE experts are a file-view type — reading them as views \
into the mmap and converting on the GPU, so the host-copy cost below does not apply");
                    }
                    let host_store = if views_ok { 0 } else {
                        (n_expert as u64) * per_expert_all_layers
                    };
                                let set_data_len = set.data_len();
                                let ram = crate::memplan::system_ram_bytes();
                                eprintln!("loader: MoE streaming will hold ~{:.1} GiB of expert weights in HOST RAM \
(plus ~{:.1} GiB of this file in page cache while loading)", gib(host_store), gib(set_data_len));
                                // ON UNIFIED MEMORY THE GPU BUDGET IS ALSO HOST RAM. This check
                                // counted the host expert store and the page cache and omitted the
                                // RESIDENT allocation — which is a separate pool on a discrete GPU and
                                // the same physical memory on an M-series machine.
                                //
                                // Measured failure: 64 GiB M2 Max, Qwen3-30B. host 15.2 + cache 17.3 =
                                // 32.5 against a 48 GiB threshold, so the plan was approved — and then
                                // ~15.0 GiB of resident experts landed in the SAME RAM and the process
                                // was OOM-killed at layer 36 of 48. Being killed is far worse than a
                                // refusal: seventeen seconds spent and no information returned.
                                // USE THE REAL SIGNAL. I wrote this as `conservative_vma ||
                                // cfg!(macos)` — which is Apple-only, and misses every OTHER unified
                                // device: an Adreno or Mali phone is an IntegratedGpu with exactly the
                                // same shared pool, and it is the machine least able to absorb the
                                // double-count.
                                //
                                // `self.unified` already exists and is derived from device_type via
                                // zerocopy::is_unified_memory, which is the question actually being
                                // asked. Third instance this session of a guard keyed on a proxy when
                                // the real predicate was in scope.
                                let uma_resident = if self.unified { gpu_budget } else { 0 };
                                let unified = uma_resident > 0;
                                if ram > 0 && !crate::memplan::streaming_fits_host(
                                    host_store, set_data_len, uma_resident, ram, unified) {
                                    return Err(format!(
                                        "MoE expert streaming needs ~{:.1} GiB of host RAM for expert weights, ~{:.1} GiB of \
resident weights that share the SAME memory on this unified-memory machine, plus \
~{:.1} GiB of page cache for the file, against {:.1} GiB of system RAM — this would be OOM-killed partway \
through the load. The engine requantizes every expert to its own q4 layout on load, which is the cost here — AND THIS \
CHECK RUNS BEFORE THE FILE-VIEW PATH IS ATTEMPTED, so the figure assumes a host copy that may not be \
needed at all: when experts can be read as views into the mmap, `raw_expert_to_q4_resident` converts \
Q4_K straight to q4 ON THE GPU with no host copy and no f32 round-trip, exactly as the dense tensors \
already do for the dense weights. If no file-views-unavailable line appears above this one, the path \
was never tried. \
Options: run a smaller checkpoint, use the llama.cpp backend (which reads the file's native Q4_K blocks \
directly and needs no host copy){}.",
                                        gib(host_store), gib(uma_resident), gib(set_data_len), gib(ram),
                                        // NAME THE ESCAPE ONLY WHEN IT WORKS. This said "set
                                        // LOGSCOPE_MOE_STREAM=0 to see the resident-fit error
                                        // instead", which offers a flag as a way to reach a DIFFERENT
                                        // error — so nobody tries it. When the model fits resident it
                                        // is not an escape to another failure, it is the answer.
                                        if full_resident <= gpu_budget {
                                            format!(", or set LOGSCOPE_MOE_STREAM=0 — this model DOES \
fit fully resident ({:.1} GiB against a {:.1} GiB budget), so streaming is not needed at all and \
turning it off avoids the host copy entirely",
                                                gib(full_resident), gib(gpu_budget))
                                        } else {
                                            format!(", or set LOGSCOPE_MOE_STREAM=0 to see the \
resident-fit error instead — though at {:.1} GiB against a {:.1} GiB budget it will not fit that way \
either",
                                                gib(full_resident), gib(gpu_budget))
                                        }));
                                }
                                auto_q4 = true;
                                moe_stream_pin = Some(pin);
                                let resident = dense + ring_cost + (pin as u64) * per_expert_all_layers;
                                eprintln!("loader: MoE won't fit fully resident → EXPERT STREAMING (q4, auto — LOGSCOPE_MOE_STREAM=0 opts out): pin {pin}/{n_expert} experts + {ring}-slot ring per layer, ~{:.1} GiB resident (dense ~{:.1} GiB); {} cold experts stream from host RAM per layer",
                                    gib(resident), gib(dense), n_expert.saturating_sub(pin));
                            } else {
                            // report what streaming WOULD do so it's actionable, even though execution isn't wired
                            let ceiling_note = if self.conservative_vma {
                                " (a CONSTANT for Asahi/Honeykrisp, not a measurement — it was taken from the staging upload path before the arena removed the staging, and `--gate-vram` measured this same device honouring 63 GiB)"
                            } else { "" };
                            let msg = match crate::memplan::plan_streaming(&shape, gpu_budget, Residency::Q4, 4096, 2, 256 * (1 << 20)) {
                                Some(p) => format!("won't fit resident in ~{:.1} GiB{} — layer streaming would pin {}/{} blocks and stream {} per token (~{:.0} ms/tok at 21 GiB/s, the MEASURED upload rate). \
FIRST CHECK WHETHER THIS BUDGET IS REAL: it is a compiled-in constant, and on an M2 Ultra `kernel-validate --gate-vram` measured the device honouring 63 GiB against this figure's 24 — a model this size may simply fit. \
Then: raise LOGSCOPE_MAX_GPU_GB to what the probe found, or use the llama.cpp backend.",
                                    gib(gpu_budget), ceiling_note, p.pinned_layers, p.total_layers, p.streamed_layers(), p.decode_ms_per_token(100.0)),
                                None => format!("won't fit even a streaming ring in ~{:.1} GiB{}; use the llama.cpp backend.", gib(gpu_budget), ceiling_note),
                            };
                            return Err(msg);
                            }
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
            // FUSED-QKV FALLBACK. A checkpoint shipping `attn_qkv` instead of separate q/k/v tensors
            // (Qwen3.5-30B) previously failed here with "missing tensor". qkvsplit::locate resolves
            // the name to the fused tensor and the byte range of the part wanted.
            //
            // Option vs Result carries the distinction: None means no fused tensor exists and the
            // ordinary miss below is correct, while Err means one exists at a shape the metadata does
            // not describe — which must STOP rather than slice something plausible.
            if g.tensor(name).is_none() {
                // GGUF DIMS ARE FASTEST-VARYING FIRST. For the reported `attn_qkv [4096, 8192]` on a
                // d=4096 model, dims[0]=4096 is the INPUT width and dims[1]=8192 is the output rows —
                // so a "row" in the split's sense is dims[0] wide and there are dims[1..] of them.
                //
                // I had these swapped on the first pass, which would have split 4096 rows out of a
                // tensor that has 8192 and sliced at a stride computed from the wrong axis. Both
                // errors point the same way: q's range would have covered part of the tensor that is
                // not q, and the row-count check would not have caught it because the numbers are
                // self-consistent — just about the wrong dimension.
                // Both derived by qkvsplit::dims_to_rows_and_stride, which is where the axis bug was
                // and is now the tested unit — the loader closure could not be tested from here, and
                // every gate passed while this was backwards because none of them loads a fused
                // tensor.
                let derive = |n: &str| g.tensor(n).and_then(|t| {
                    let ty = t.ggml_type;
                    crate::qkvsplit::dims_to_rows_and_stride(&t.dims, |w| gguf::type_nbytes(ty, w))
                });
                let rows_of = |n: &str| derive(n).map(|(r, _)| r);
                let row_bytes = |n: &str| derive(n).map(|(_, s)| s);
                for fused in crate::qkvsplit::FUSED_NAMES {
                    let src = match crate::qkvsplit::resolve(name, fused) { Some((s, _)) => s, None => break };
                    // A None here means the row is not a whole number of quantization blocks, so it
                    // cannot be sliced at a row boundary — q would end mid-block and both q's last
                    // block and k's first would be garbage. Named, because falling through to
                    // "missing tensor" would report the wrong problem entirely.
                    let rb = match row_bytes(&src) {
                        Some(b) => b,
                        None => return Err(format!(
                            "{name}: the fused tensor {src} has a row that is not a whole number of \
quantization blocks, so it cannot be split at a row boundary — q would end mid-block and both q's \
last block and k's first would decode to garbage")),
                    };
                    match crate::qkvsplit::locate(name, &rows_of, n_heads as usize,
                                                  n_kv_heads as usize, head_dim as usize, rb) {
                        Some(Ok((srcname, (b0, b1)))) => {
                            let t = g.tensor(&srcname).ok_or(format!("missing tensor {srcname}"))?;
                            let raw = set.slice(t.offset + b0 as u64, b1 - b0).ok_or(format!(
                                "fused tensor {srcname} data out of range for {name}"))?;
                            let cols = t.dims[0] as usize;      // input width, matching row_bytes
                            let rows = (b1 - b0) / rb;
                            return gguf::dequantize(t.ggml_type, raw, rows * cols);
                        }
                        Some(Err(e)) => return Err(format!("{name}: {}", e.explain())),
                        None => break,
                    }
                }
            }
            let t = g.tensor(name).ok_or(format!("missing tensor {name}"))?;
            let count = t.dims.iter().product::<u64>() as usize;
            let nbytes = gguf::type_nbytes(t.ggml_type, count).ok_or(format!("type {} unsupported for {name}", t.ggml_type))?;
            // A read that escapes the set means a shard is missing — report which tensor rather than
            // decoding a short buffer into plausible garbage.
            let raw = set.slice(t.offset, nbytes).ok_or(format!(
                "tensor {name} data out of range — a shard is missing or truncated ({} shard(s) loaded)", set.shards()))?;
            gguf::dequantize(t.ggml_type, raw, count)
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
        // Arena packing: route resident weights into a few large slabs instead of one buffer PER TENSOR.
        // One-BO-per-tensor is what kills Honeykrisp/Asahi on big MoE models — 16 experts × N layers ×
        // several matrices = thousands of buffer objects, and the driver runs out of BO VMA space
        // ("Failed to allocate BO VMA") long before it runs out of bytes. Packing into ≤max_buffer_size
        // slabs cuts that to a couple dozen BOs. AUTO-ON when the driver is BO-constrained (conservative_vma
        // = Asahi/Honeykrisp) OR explicitly requested; opt out with LOGSCOPE_ARENA=0. Guard clears the bump
        // state on unwind (slabs persist via the weight tensors).
        let arena_env = std::env::var("LOGSCOPE_ARENA").ok();
        let arena_on = match arena_env.as_deref() {
            Some("0") | Some("false") => false,     // hard opt-out
            Some(_) => true,                        // explicit opt-in
            // ON WHERE OBJECT COUNT IS THE BINDING CONSTRAINT, which is what the comment above always
            // said and what the code did not check. `conservative_vma` means "Vulkan on Apple"; the
            // governor already classifies the limit KIND, and both BufferObjects (Metal) and
            // AllocationCount (all Vulkan, commonly 4096 on desktop and tighter on mobile) are exactly
            // the case the arena exists for.
            //
            // The practical miss was a phone: an Adreno or Mali device has the tightest object budget
            // of anything we run on, and defaulted the arena OFF because it is not Apple.
            None => {
                let kind = self.governor.lock().unwrap().budget_kind();
                matches!(kind, crate::governor::LimitKind::BufferObjects
                             | crate::governor::LimitKind::AllocationCount)
            }
        };
        let _arena_guard = if arena_on {
            let max_buf = self.device.limits().max_buffer_size;
            let slab = std::env::var("LOGSCOPE_ARENA_SLAB_MB").ok().and_then(|m| m.trim().parse::<u64>().ok())
                .map(|mb| mb << 20).unwrap_or(2 << 30).min(max_buf).max(256 << 20);
            // Align the slab size DOWN to 256 (≥ COPY_BUFFER_ALIGNMENT=4). A UMA zero-copy slab is created
            // mapped_at_creation, which requires size % 4 == 0 — and some drivers report max_buffer_size as
            // an odd value (Honeykrisp/Asahi: ~2047 MiB = 2^31-1), so `.min(max_buf)` can land on an
            // unaligned size. Rounding DOWN (not up) keeps it within the device limit.
            let slab = slab & !255;
            eprintln!("wgpu-engine: arena ON ({}) — packing resident weights into ≤{} MiB slabs (fewer buffer objects)",
                if arena_env.is_some() { "requested" } else { "auto: BO-VMA-constrained driver" }, slab >> 20);
            *self.arena.lock().unwrap() = Some(ResidentArena { slabs: Vec::new(), slab_size: slab, used: 0, mapped: self.unified });
            Some(ArenaGuard(&self.arena))
        } else { None };
        let lin = |name: &str, n: usize, k: usize, quantize: bool| -> Result<Lin, String> {
            let t = g.tensor(name).ok_or(format!("missing tensor {name}"))?;
            if quantize && !q4_mode && t.ggml_type == 8 && k % 32 == 0 {
                let count = t.dims.iter().product::<u64>() as usize;
                let nbytes = gguf::type_nbytes(8, count).ok_or("q8_0 size")?;
                let raw = set.slice(t.offset, nbytes).ok_or(format!(
                    "tensor {name} out of range — a shard is missing or truncated"))?;
                return Ok(Lin::q8(self.q8_0_to_resident(raw, n, k)));
            }
            // DIRECT Q4_K → q4-resident, the twin of the q8_0 fast path above and the reason the loader no
            // longer needs a host copy. The file's own super-blocks are uploaded and converted on the GPU
            // (--gate-q4k proves the conversion is bit-identical to the CPU route it replaces), so the
            // dequantize-to-f32 and requantize steps disappear entirely. LOGSCOPE_NO_Q4K_GPU falls back to
            // the CPU path if this ever needs to be bisected against.
            if quantize && q4_mode && t.ggml_type == 12 && k % 256 == 0
                && std::env::var("LOGSCOPE_NO_Q4K_GPU").is_err() {
                let count = t.dims.iter().product::<u64>() as usize;
                if count == n * k {
                    if let (Some(nbytes), Some(sset)) = (gguf::type_nbytes(12, count), static_set.as_ref()) {
                        if let Some(raw) = sset.slice(t.offset, nbytes) {
                            // DEFERRED: recorded now, converted in a batch once the arena is sealed. This
                            // is what lets arena packing and GPU conversion coexist — dispatching here
                            // would be illegal while a slab is mapped.
                            let total = n * k;
                            let (qw_words, qs_words) = (total / 8, total / 32);
                            let qs_off_words = (qw_words * 4).div_ceil(256) * 256 / 4;
                            let out = self.alloc_resident_words(qs_off_words + qs_words);
                            self.q4k_defer(&out, raw, n, k, 0, qs_off_words);
                            let qw = Tensor { buf: out.buf.clone(), rows: n, cols: k, pool: None,
                                              view: Some((0, (qw_words * 4) as u64)) };
                            let qs = Tensor { buf: out.buf.clone(), rows: n, cols: k / 32, pool: None,
                                              view: Some(((qs_off_words * 4) as u64, (qs_words * 4) as u64)) };
                            return Ok(Lin::q4(Q4Weight { qw, qs, n, k }));
                        }
                    }
                }
            }
            Ok(self.make_lin(&fetch(name)?, n, k, quantize, q4_mode))
        };
        let mut layers = Vec::with_capacity(n);
        for i in 0..n {
            // Per-layer MoE detection. GLM4-MoE (and DeepSeek) use `leading_dense_block_count` dense blocks
            // before the MoE blocks start, so MoE-ness is PER LAYER, not model-wide: block 0 may be dense
            // (ffn_gate/up/down) while block 1+ are MoE (ffn_gate_exps + router). Detect by this layer's
            // expert tensor rather than trusting block 0. Falls back to the model-wide flag for archs where
            // every layer is uniform.
            // Llama-4 style INTERLEAVED MoE: expert tensors are not present on every block, so a layer is
            // MoE only if it carries the WHOLE expert set. Testing just ffn_gate_exps mis-classifies a
            // block that has some but not all of them, and the mistake surfaces far downstream as a bare
            // "missing tensor blk.N.ffn_up_exps.weight" with no indication that the block was never MoE.
            let exp_names = ["ffn_gate_exps", "ffn_up_exps", "ffn_down_exps"];
            let present: Vec<&str> = exp_names.iter().copied()
                .filter(|n| g.tensor(&format!("blk.{i}.{n}.weight")).is_some()).collect();
            let layer_is_moe = is_moe && present.len() == exp_names.len();
            if is_moe && !present.is_empty() && present.len() != exp_names.len() {
                let missing: Vec<&str> = exp_names.iter().copied().filter(|n| !present.contains(n)).collect();
                eprintln!("wgpu-engine: blk.{i} carries a PARTIAL expert set — has {present:?}, missing \
{missing:?}; treating this block as DENSE. If the model is meant to be MoE here, its expert tensors use \
names this loader does not know (run --gate-quant-survey and dump blk.{i}.* to see them).");
            }
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
                // REFUSE CLEANLY when this is an MLA checkpoint. MLA models have no `attn_q` — they
                // have the low-rank pair attn_q_a/attn_q_b — so falling through to the dense path here
                // reports "missing tensor blk.0.attn_q.weight", which sends the reader looking for a
                // corrupt file. The tensor is not missing; the loader is reading the wrong architecture.
                if g.tensor(&format!("blk.{i}.attn_q_a.weight")).is_some()
                    || g.tensor(&format!("blk.{i}.attn_kv_a_mqa.weight")).is_some() {
                    return Err(format!("blk.{i} is Multi-head Latent Attention (attn_q_a / attn_kv_a_mqa \
present, attn_q absent) and the MLA path did not engage, so the dense loader ran instead. This is a \
LOADER gap, not a bad checkpoint — see the MLA line earlier in this log for which part is missing"));
                }
                // DIAGNOSE rather than just report a name. "missing tensor blk.0.attn_q.weight" sends
                // the reader to look for a corrupt file; the tensor is usually not missing at all, the
                // model just names its attention weights differently — fused attn_qkv, MLA's low-rank
                // pair, or a convention this loader has never seen. Listing what the block ACTUALLY has
                // turns an hour of guessing into one line.
                if g.tensor(&format!("blk.{i}.attn_q.weight")).is_none() {
                    let mut found: Vec<String> = Vec::new();
                    for suffix in ["attn_qkv", "attn_q_a", "attn_q_b", "attn_kv_a_mqa", "attn_k", "attn_v",
                                   "attn_output", "attn_norm", "wqkv", "query_key_value"] {
                        let n = format!("blk.{i}.{suffix}.weight");
                        if let Some(t) = g.tensor(&n) {
                            found.push(format!("{suffix} {:?}", t.dims));
                        }
                    }
                    let hint = if found.iter().any(|f| f.starts_with("attn_qkv") || f.starts_with("wqkv")
                                                   || f.starts_with("query_key_value")) {
                        // Says what the shapes WOULD have to be, so a reader can tell in one glance
                    // whether the file matches the q|k|v blocked layout qkvsplit implements — or
                    // whether it is something else and the split would produce attention over the
                    // wrong projections. A message naming only the symptom leaves that unanswerable.
                    " — this looks like a FUSED qkv layout. qkvsplit::split() implements the q|k|v \
BLOCKED form (n_head*head_dim rows of q, then n_kv*head_dim of k, then the same of v); what remains \
is wiring it here and checking that those rows sum to the tensor's own row count, because a layout \
that is interleaved instead would slice plausibly and be wrong"
                    } else if found.iter().any(|f| f.starts_with("attn_q_a")) {
                        " — this is Multi-head Latent Attention"
                    } else { "" };
                    return Err(format!("blk.{i} has no `attn_q.weight`. What it DOES have: [{}]{hint}. \
The file is not corrupt; this loader does not recognise its attention layout",
                        if found.is_empty() { "nothing recognisable".into() } else { found.join(", ") }));
                }
                // The loader's internal layout IS q ++ k ++ v, so a checkpoint that ships them already
                // fused needs no rearrangement — only a shape check. Guarded rather than assumed: some
                // architectures interleave qkv PER HEAD instead of as three blocks, and those have the
                // same total row count while being a completely different matrix. Matching rows is
                // necessary, not sufficient, so this is accepted only when the split form is absent and
                // the total is exactly right.
                let want_rows = (attn_dim + 2 * kv) as u64;
                let mut qkv = if g.tensor(&format!("blk.{i}.attn_q.weight")).is_none() {
                    let name = format!("blk.{i}.attn_qkv.weight");
                    let t = g.tensor(&name).ok_or_else(|| format!("blk.{i}: neither attn_q nor attn_qkv"))?;
                    let rows = t.dims.last().copied().unwrap_or(0).max(t.dims.first().copied().unwrap_or(0));
                    if rows != want_rows {
                        return Err(format!("blk.{i}.attn_qkv has {rows} rows but this model needs \
{want_rows} (attn_dim {attn_dim} + 2 x kv {kv}). A fused qkv with the wrong row count is a DIFFERENT \
layout — probably per-head interleaved — and reading it as three blocks would produce fluent nonsense"));
                    }
                    eprintln!("wgpu-engine: blk.{i} ships a FUSED attn_qkv ({rows} rows, as expected) — \
using it directly. UNVERIFIED against a real generation: the row count matching does not prove the \
q/k/v block ORDER matches");
                    fetch(&name)?
                } else {
                    let mut v = fetch(&format!("blk.{i}.attn_q.weight"))?;
                    v.extend_from_slice(&fetch(&format!("blk.{i}.attn_k.weight"))?);
                    v.extend_from_slice(&fetch(&format!("blk.{i}.attn_v.weight"))?);
                    v
                };
                let fused = self.make_lin(&qkv, attn_dim + 2 * kv, d, q, q4_mode);
                if has_qkv_bias {
                    let mut b = Vec::with_capacity(attn_dim + 2 * kv);
                    b.extend_from_slice(&fbq); b.extend_from_slice(&fbk); b.extend_from_slice(&fbv);
                    fused.with_bias(Some(self.upload(&b, 1, attn_dim + 2 * kv)))
                } else { fused }
            };
            layers.push(BlockWeights {
                g1: self.upload(&gemma_norm(&fetch(&format!("blk.{i}.attn_norm.weight"))?, &acfg), 1, d),
                wqkv,
                wq: lin(&format!("blk.{i}.attn_q.weight"), attn_dim, d, q)?.with_bias(bq),
                wk: lin(&format!("blk.{i}.attn_k.weight"), kv, d, q)?.with_bias(bk),
                wv: lin(&format!("blk.{i}.attn_v.weight"), kv, d, q)?.with_bias(bv),
                wo: lin(&format!("blk.{i}.attn_output.weight"), d, attn_dim, q)?,
                g2: self.upload(&gemma_norm(&fetch(&format!("blk.{i}.ffn_norm.weight"))
                    .or_else(|_| fetch(&format!("blk.{i}.post_attention_norm.weight")))?, &acfg), 1, d),
                // gate and up fused into one [2h, d] projection: a single matmul emits
                // [t, 2h] = [gate(h) | up(h)] per row, which SWIGLU_PACKED then consumes.
                // Halves the FFN projection dispatches/weight-binds; per-row int8 quant of
                // the concatenation is identical to quantizing the two weights separately.
                // MoE layers have no dense FFN tensors — tiny F32 placeholders, never applied
                // (ffn_apply branches to the moe field first).
                wgate_up: if layer_is_moe { self.make_lin(&vec![0.0f32; 32], 1, 32, false, false) } else {
                    let gt = fetch(&format!("blk.{i}.ffn_gate.weight"))?;
                    let u = fetch(&format!("blk.{i}.ffn_up.weight"))?;
                    let mut gu = Vec::with_capacity(gt.len() + u.len());
                    gu.extend_from_slice(&gt); gu.extend_from_slice(&u);
                    self.make_lin(&gu, 2 * h, d, q, q4_mode)
                },
                wdown: if layer_is_moe { self.make_lin(&vec![0.0f32; 32], 1, 32, false, false) } else { lin(&format!("blk.{i}.ffn_down.weight"), d, h, q)? },
                moe: if layer_is_moe {
                    // Router stays F32 (small: [n_expert, d]) — routing precision decides WHICH weights run.
                    let router = self.make_lin(&fetch(&format!("blk.{i}.ffn_gate_inp.weight"))?, n_expert, d, false, false);
                    // Stacked expert tensors: expert dim OUTERMOST, so expert e's matrix is the contiguous
                    // f32 slice [e·rows·cols, (e+1)·rows·cols) after fetch's dequant (which reads each
                    // tensor's ACTUAL ggml type — Q4_K vs Q6_K varies per layer). gate/up: [ff, d]; down: [d, ff].
                    // v2 layout: ONE quantized Lin per kind for the whole expert stack (interleaved
                    // [e: gate||up] for the fused projection), then per-expert Lins are zero-copy row
                    // VIEWS into it. One quant pass instead of n_expert (faster load), and the stacked
                    // buffer is what the GPU-routed decode kernels index (row = sel·rstride + col).
                    let moe_streaming = moe_stream_pin.is_some() && q4_mode;
                    let gsrc = fetch(&format!("blk.{i}.ffn_gate_exps.weight"))?;
                    let usrc = fetch(&format!("blk.{i}.ffn_up_exps.weight"))?;
                    let ec = expert_ff * d;
                    // STREAMING: skip the resident expert stack entirely (it's the mass that busts the BO-VMA
                    // ceiling). Build tiny placeholder Lins for the resident fields; decode goes through the
                    // stream store, which holds cold experts host-side and fills GPU slots on demand.
                    let (stacked_gu, stacked_down, gate_up, down) = if moe_streaming {
                        let ph = || self.make_lin(&vec![0.0f32; 32], 1, 32, false, false);
                        (ph(), ph(), Vec::new(), Vec::new())
                    } else {
                        let mut inter = Vec::with_capacity(2 * ec * n_expert);
                        for e in 0..n_expert {
                            inter.extend_from_slice(&gsrc[e * ec..(e + 1) * ec]);
                            inter.extend_from_slice(&usrc[e * ec..(e + 1) * ec]);
                        }
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
                        (stacked_gu, stacked_down, gate_up, down)
                    };
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
                    // STREAMING store: cold experts stay host-side (already-quantized q4 bytes), hot experts
                    // pinned resident, cold ones stream into GPU slots on demand at decode (moe_ffn_v1).
                    let stream = if let Some(pin) = moe_stream_pin.filter(|_| q4_mode) {
                        let pin = pin.min(n_expert);
                        let ring = (n_expert_used + 1).max(2);
                        let gu_rows = 2 * expert_ff; let dn_rows = d;
                        // VIEW PATH (Colibri): if the three expert tensors are Q4_K with the shapes we
                        // expect, point at the file instead of copying. This is what removes the ~50 GiB
                        // host store and the multi-minute requantize pass — nothing is read here at all,
                        // only sliced.
                        // Returns Err with the REASON when it cannot use views. Silently choosing the slow
                        // path is what cost a 135-second load with no explanation in the log.
                        let raw_try = (|| -> Result<RawExpertSrc, String> {
                            if std::env::var("LOGSCOPE_NO_MOE_VIEWS").is_ok() {
                                return Err("LOGSCOPE_NO_MOE_VIEWS is set".into());
                            }
                            if d % 256 != 0 || expert_ff % 256 != 0 {
                                return Err(format!("conversion needs d and ff to be multiples of 256 (d={d}, ff={expert_ff})"));
                            }
                            let sset = static_set.as_ref()
                                .ok_or("shards were not retained for the process (loader called without the static entry point)")?;
                            let names = [format!("blk.{i}.ffn_gate_exps.weight"),
                                         format!("blk.{i}.ffn_up_exps.weight"),
                                         format!("blk.{i}.ffn_down_exps.weight")];
                            let mut spans = Vec::new();
                            // Captured per tensor: gate/up and down differ in Q4_K_M.
                            let mut types = [0u32; 3];
                            for (idx, nm) in names.iter().enumerate() {
                                let t = g.tensor(nm).ok_or(format!("{nm} not found"))?;
                                // Q4_K and Q6_K both. The check was `!= 12`, and Q4_K_M stores
                                // ffn_down as Q6_K — so one tensor rejected the whole model and sent
                                // it to the host-copy path. --gate-q6k has been bit-exact since it
                                // went green; the check simply never learned.
                                // GATE HELD AT Q4_K FOR NOW, deliberately. The type plumbing below is
                                // Q6_K-ready — strides, per-tensor types, and a load-time conversion —
                                // but the streaming FILL converts on-GPU via q4k_convert_from, and the
                                // Q6_K equivalent needs a shader that does not exist yet
                                // (Q6K_DEQUANT produces f32; quantize_q4_resident is CPU).
                                //
                                // Opening the gate without it would take the file-view path and then
                                // produce garbage at fill time, which is strictly worse than the host
                                // copy it replaced. LOGSCOPE_Q6K_VIEWS=1 opts in for anyone wiring the
                                // shader.
                                let q6_ok = std::env::var("LOGSCOPE_Q6K_VIEWS").is_ok();
                                let allowed: &[u32] = if q6_ok { &[12, 14] } else { &[12] };
                                if !allowed.contains(&t.ggml_type) {
                                    return Err(format!("{nm} is {} — the file-view path reads Q4_K \
today. Q6_K is bit-exact (--gate-q6k) and the loader is plumbed for it; what is missing is the on-GPU \
Q6_K->q4 fill conversion. Until that exists this falls back to the host copy rather than reading a \
format the fill path cannot convert",
                                        gguf::ggml_type_name(t.ggml_type)));
                                }
                                let count = t.dims.iter().product::<u64>() as usize;
                                let want = if idx == 2 { d * expert_ff * n_expert } else { expert_ff * d * n_expert };
                                if count != want { return Err(format!("{nm} has {count} elements, expected {want}")); }
                                types[idx] = t.ggml_type;
                                let nbytes = gguf::type_nbytes(t.ggml_type, count)
                                    .ok_or("source block size")?;
                                spans.push(sset.slice(t.offset, nbytes).ok_or(format!("{nm} span escapes the shards"))?);
                            }
                            // gate and up must match each other; down may differ.
                            if types[0] != types[1] {
                                return Err(format!("gate is {} and up is {} — the streaming path packs \
them as one blob, so they must share a format",
                                    gguf::ggml_type_name(types[0]), gguf::ggml_type_name(types[1])));
                            }
                            let (gu_ty, dn_ty) = (types[0], types[2]);
                            Ok(RawExpertSrc {
                                gate: spans[0], up: spans[1], down: spans[2],
                                // Per-tensor strides: Q4_K is 144 bytes per 256 weights and Q6_K is
                                // 210, so a hardcoded 144 would slice a Q6_K tensor at the wrong
                                // offset even once the type check let it through.
                                gu_expert_bytes: Self::source_block_bytes(gu_ty, expert_ff * d)
                                    .ok_or_else(|| format!("no block size for type {gu_ty}"))?,
                                dn_expert_bytes: Self::source_block_bytes(dn_ty, d * expert_ff)
                                    .ok_or_else(|| format!("no block size for type {dn_ty}"))?,
                                gu_type: gu_ty, dn_type: dn_ty,
                                ff: expert_ff, d,
                            })
                        })();
                        if i == 0 {
                            if let Err(why) = &raw_try {
                                eprintln!("wgpu-engine: MoE file-views unavailable — {why}; falling back to the \
host-copy path (dequantize + requantize every expert into RAM)");
                            }
                        }
                        let raw_src = raw_try.ok();

                        if let Some(r) = raw_src {
                            let (gu_qs, gu_total) = r.gu_layout();
                            let (dn_qs, dn_total) = r.dn_layout();
                            // Pinned experts: converted ONCE from the file into a resident blob. A later
                            // repin re-converts in place, so correcting the pin set costs a GPU pass.
                            let mut pinned_raw = std::collections::HashMap::new();
                            for e in 0..pin {
                                // pinned expert blobs are resident weights, not scratch
                                let gb = self.alloc_resident_words(gu_total / 4);
                                let db = self.alloc_resident_words(dn_total / 4);
                                if let (Some(gt), Some(ut), Some(dt)) = (r.gate_of(e), r.up_of(e), r.down_of(e)) {
                                    // deferred, like the dense weights, so the arena can stay mapped
                                    self.q4k_defer(&gb, gt, expert_ff, d, 0, gu_qs / 4);
                                    self.q4k_defer(&gb, ut, expert_ff, d, expert_ff * d / 8, gu_qs / 4 + expert_ff * (d / 32));
                                    self.q4k_defer(&db, dt, d, expert_ff, 0, dn_qs / 4);
                                }
                                pinned_raw.insert(e as u32, (gb.buf.clone(), db.buf.clone()));
                            }
                            let pinned_ids: Vec<u32> = (0..pin as u32).collect();
                            if i == 0 {
                                eprintln!("wgpu-engine: MoE STREAMING on (FILE VIEWS — no host expert copy) — \
{n_expert} experts, {pin} pinned, ring={ring} slots/layer");
                            }
                            Some(std::sync::Mutex::new(MoeStream {
                                layer: i, n_expert, gu_rows, gu_k: d, dn_rows, dn_k: expert_ff,
                                host_gu: Vec::new(), host_dn: Vec::new(), gu_qs_off: gu_qs, dn_qs_off: dn_qs,
                                gu_slots: StreamSlots::new(self, ring, gu_total as u64),
                                dn_slots: StreamSlots::new(self, ring, dn_total as u64),
                                policy: crate::expert_cache::LayerSlots::new(&pinned_ids, ring as u32),
                                pinned_gu: std::collections::HashMap::new(),
                                pinned_dn: std::collections::HashMap::new(),
                                repins: 0, visits: 0, raw: Some(r), pinned_raw,
                            }))
                        } else {
                        // LEGACY host-copy path: only reached when the experts are not Q4_K (or views are
                        // disabled). Costs a full requantize into host memory.
                        let dsrc = fetch(&format!("blk.{i}.ffn_down_exps.weight"))?;
                        let mut host_gu = Vec::with_capacity(n_expert); let mut host_dn = Vec::with_capacity(n_expert);
                        let mut gu_qs_off = 0; let mut dn_qs_off = 0;
                        // PARALLEL over experts. Each expert's dequantize+requantize is independent and
                        // pure CPU, so this is embarrassingly parallel — it was costing ~2.8s per layer
                        // single-threaded (135s for 48 layers) while the other cores sat idle.
                        let mut slots: Vec<Option<(Vec<u8>, usize, Vec<u8>, usize)>> =
                            (0..n_expert).map(|_| None).collect();
                        {
                            let nthreads = std::thread::available_parallelism()
                                .map(|n| n.get()).unwrap_or(4).min(n_expert).max(1);
                            let per = n_expert.div_ceil(nthreads);
                            let (gs, us, ds) = (&gsrc, &usrc, &dsrc);
                            std::thread::scope(|sc| {
                                for (ti, chunk) in slots.chunks_mut(per).enumerate() {
                                    let base = ti * per;
                                    sc.spawn(move || {
                                        for (j, slot) in chunk.iter_mut().enumerate() {
                                            let e = base + j;
                                            let mut cat = Vec::with_capacity(2 * ec);
                                            cat.extend_from_slice(&gs[e * ec..(e + 1) * ec]);
                                            cat.extend_from_slice(&us[e * ec..(e + 1) * ec]);
                                            let (gb, goff) = self.quantize_q4_host_bytes(&cat, gu_rows, d);
                                            let (db, doff) = self.quantize_q4_host_bytes(
                                                &ds[e * d * expert_ff..(e + 1) * d * expert_ff], dn_rows, expert_ff);
                                            *slot = Some((gb, goff, db, doff));
                                        }
                                    });
                                }
                            });
                        }
                        for slot in slots {
                            let (gb, goff, db, doff) = slot.expect("every expert quantized");
                            gu_qs_off = goff; dn_qs_off = doff;
                            host_gu.push(gb); host_dn.push(db);
                        }
                        drop(dsrc);
                        let gu_slot_bytes = host_gu[0].len() as u64;
                        let dn_slot_bytes = host_dn[0].len() as u64;
                        let mut pinned_gu = std::collections::HashMap::new();
                        let mut pinned_dn = std::collections::HashMap::new();
                        for e in 0..pin {
                            pinned_gu.insert(e as u32, self.q4_bufs_from_host(&host_gu[e], gu_qs_off));
                            pinned_dn.insert(e as u32, self.q4_bufs_from_host(&host_dn[e], dn_qs_off));
                        }
                        let pinned_ids: Vec<u32> = (0..pin as u32).collect();
                        if i == 0 { eprintln!("wgpu-engine: MoE STREAMING on — {n_expert} experts host-resident (q4), {pin} pinned, ring={ring} slots/layer"); }
                        // Progress. This pass dequantizes every expert from the file's Q4_K and requantizes
                        // it to our q4 layout — tens of GiB of CPU work that otherwise runs silently for
                        // many minutes, which reads as a hang and gets killed before it finishes.
                        {
                            static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
                            let t0 = *T0.get_or_init(std::time::Instant::now);
                            if i == 0 || (i + 1) % 4 == 0 || i + 1 == n {
                                let el = t0.elapsed().as_secs_f32();
                                let done = i + 1;
                                let eta = if done > 0 { el / done as f32 * (n.saturating_sub(done)) as f32 } else { 0.0 };
                                eprintln!("wgpu-engine: MoE streaming — quantized {done}/{n} layers ({el:.0}s elapsed, ~{eta:.0}s left)");
                            }
                        }
                        Some(std::sync::Mutex::new(MoeStream {
                            layer: i, n_expert, gu_rows, gu_k: d, dn_rows, dn_k: expert_ff,
                            host_gu, host_dn, gu_qs_off, dn_qs_off,
                            gu_slots: StreamSlots::new(self, ring, gu_slot_bytes),
                            dn_slots: StreamSlots::new(self, ring, dn_slot_bytes),
                            policy: crate::expert_cache::LayerSlots::new(&pinned_ids, ring as u32),
                            pinned_gu, pinned_dn, repins: 0, visits: 0, raw: None, pinned_raw: std::collections::HashMap::new(),
                        }))
                        }
                    } else { None };
                    drop(gsrc); drop(usrc);
                    // PRINT THE ROUTING CONFIG AND ITS SOURCE, once. Five Llama-4 constants
                    // turned out to be values the file carried, and each produced text that was
                    // merely worse with every tensor present — the log line is where a wrong one
                    // becomes visible. The same values decide MoE routing here: a normalize flag
                    // read the wrong way, or a scale defaulted when the file specifies one,
                    // rescales every expert's contribution and reads as a confused model.
                    {
                        static LOGGED: std::sync::atomic::AtomicBool =
                            std::sync::atomic::AtomicBool::new(false);
                        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                            let norm_key = format!("{}.expert_weights_norm", acfg.prefix);
                            let scale_key = format!("{}.expert_weights_scale", acfg.prefix);
                            let c = crate::archcfg::moe_config(&acfg, n_expert as u32,
                                n_expert_used as u32,
                                g.get_u32(&format!("{}.expert_shared_count", acfg.prefix)).unwrap_or(0),
                                g.get_f32(&scale_key), g.get_u32(&norm_key).map(|v| v != 0));
                            eprintln!("loader: MoE routing — {} of {} experts, normalize {} ({}), \
scale {} ({}), shared {}, weight applied to {}",
                                n_expert_used, n_expert,
                                c.normalize,
                                if g.get_u32(&norm_key).is_some() { "file" } else { "arch default" },
                                c.routed_scale,
                                if g.get_f32(&scale_key).is_some() { "file" } else { "default 1.0" },
                                c.n_shared,
                                if c.weight_before { "the expert's INPUT" } else { "its output" });
                        // CROSS-CHECK THE ROUTING AGAINST THE TENSORS. Scout reports every one of these as
                        // (default) — its file carries none of the keys — so expert_count, expert_used_count and
                        // expert_shared_count are taken on trust. Two of the three are checkable: the stacked
                        // tensor names the expert count, and a shared expert either has _shexp tensors or does not
                        // exist. The third, top-k, is constrained by nothing in the file and cannot be checked.
                        {
                            let axis = g.tensor("blk.0.ffn_gate_exps.weight")
                                .and_then(|t| t.dims.get(2).copied()).map(|n| n as usize);
                            let shexp = g.tensor("blk.0.ffn_gate_shexp.weight").is_some();
                            for d in crate::moe::check_routing(n_expert, n_expert_used, c.n_shared as usize, axis, shexp) {
                                eprintln!("loader: MoE ROUTING DISAGREEMENT — {}", d.explain());
                            }
                        }
                        }
                    }
                    Some(Box::new(MoeLayer { router, gate_up, down, stacked_gu, stacked_down, shared, stream,
                        // SHARED EXPERT COUNT FROM METADATA. This was a literal 0, and it worked for
                        // Scout only because moe_config rescues an arch that declares
                        // `moe_shared_expert` with `.max(1)`. Correct by accident: an architecture
                        // with two shared experts, or one whose flag disagrees with its file, loses
                        // them silently — and a missing always-on expert degrades output without
                        // breaking a shape, which is the failure mode Scout has been showing.
                        //
                        // Fifth value this session that the file carries and the loader took from a
                        // constant, after the NoPE interval, the attention temperature, qk-norm and
                        // expert_weights_scale.
                        cfg: crate::archcfg::moe_config(&acfg, n_expert as u32, n_expert_used as u32,
                            g.get_u32(&format!("{}.expert_shared_count", acfg.prefix)).unwrap_or(0),
                            g.get_f32(&format!("{}.expert_weights_scale", acfg.prefix)),
                            g.get_u32(&format!("{}.expert_weights_norm", acfg.prefix)).map(|v| v != 0)) }))
                } else { None },
                // Qwen3/3.5 per-head QK-Norm ([head_dim] each), applied before RoPE in the block forward.
                q_norm: if has_qk_norm { Some(self.upload(&fetch(&format!("blk.{i}.attn_q_norm.weight"))?, 1, head_dim)) }
                        else if is_llama4 && !(nope_step > 0 && (i + 1) % nope_step == 0) { Some(self.upload(&vec![1.0f32; head_dim], 1, head_dim)) }
                        else { None },
                k_norm: if has_qk_norm { Some(self.upload(&fetch(&format!("blk.{i}.attn_k_norm.weight"))?, 1, head_dim)) }
                        else if is_llama4 && !(nope_step > 0 && (i + 1) % nope_step == 0) { Some(self.upload(&vec![1.0f32; head_dim], 1, head_dim)) }
                        else { None },
                nope: nope_step > 0 && (i + 1) % nope_step == 0,
                // Llama 4 iRoPE attention temperature tuning on NoPE layers (Meta defaults floor_scale
                // 8192, attn_scale 0.1). RoPE layers and non-Llama4 archs get None (identity).
// LLAMA-4 GAP (verified against llama.cpp src/models/llama4.cpp): Scout additionally needs
                // `use_kq_norm` — a WEIGHTLESS RMS norm (Llama4TextL2Norm) applied to Q and K *after* rope,
                // on rope layers only. llama.cpp sets use_kq_norm = (type != 17B_128E), so it is ON for
                // Scout (17B_16E) and OFF for Maverick. We do not implement it: our qk_norm is driven by a
                // learned weight (None here) and runs BEFORE rope, which is the Qwen3 arrangement, not this
                // one. Missing it produces coherent-looking garbage rather than an error.
                // Llama-4 use_kq_norm: an all-ones weight so the existing per-head rmsnorm becomes the
                // weightless Llama4TextL2Norm. ON for every Llama-4 except the 128-expert Maverick, which
                // is exactly how llama.cpp gates it (use_kq_norm = type != LLM_TYPE_17B_128E). Applied
                // after rope by `kq_post`; rope layers only.
                // Per-layer rope base: Gemma rotates its WINDOWED layers at rope.freq_base_swa and its
                // full-attention layers at the model base. Only set when the file actually carries the
                // key AND this layer is windowed — otherwise None, and every other model is untouched.
                rope_base: {
                    let swa_base = g.get_f32(&format!("{}.rope.freq_base_swa", acfg.prefix));
                    match (swa_base, swa_pattern.get(i).copied().unwrap_or(false)) {
                        (Some(b), true) if b > 0.0 => Some(b),
                        _ => None,
                    }
                },
                // Gemma gates with GELU. Selected from the arch config rather than sniffed from tensors.
                geglu: matches!(acfg.act, crate::archcfg::Act::GeGlu),
                // Gemma's post-attention / post-FFN norms. Absent on every other architecture, so None
                // leaves those paths untouched. Loaded through gemma_norm so the (1+w) form is applied
                // here too — a post-norm with the wrong scaling is as wrong as a missing one.
                post_attn: fetch(&format!("blk.{i}.post_attention_norm.weight")).ok()
                    .map(|v| self.upload(&gemma_norm(&v, &acfg), 1, d)),
                post_ffn: fetch(&format!("blk.{i}.post_ffw_norm.weight")).ok()
                    .map(|v| self.upload(&gemma_norm(&v, &acfg), 1, d)),
                // MLA projections, when present. Loaded as f32 (not quantized): these are the small
                // low-rank factors, and quantizing them is a separate question from quantizing the big
                // weight matrices. GLM-5.2 (glm-dsa) names the reconstructed dims *_mla; both spellings
                // of the tensor names are accepted so one loader covers DeepSeek-V3 and GLM.
                mla: {
                    let t = |a: &str, b: &str| -> Option<String> {
                        let na = format!("blk.{i}.{a}.weight");
                        if g.tensor(&na).is_some() { return Some(na); }
                        let nb = format!("blk.{i}.{b}.weight");
                        if g.tensor(&nb).is_some() { return Some(nb); }
                        None
                    };
                    (|| -> Option<crate::mla::MlaWeightsT> {
                        let (qa, qaln, qb) = (t("attn_q_a", "attn_q_a_proj")?, t("attn_q_a_norm", "attn_q_a_layernorm")?, t("attn_q_b", "attn_q_b_proj")?);
                        let (kva, kvaln) = (t("attn_kv_a_mqa", "attn_kv_a")?, t("attn_kv_a_norm", "attn_kv_a_layernorm")?);
            // kv_b comes in TWO forms and only the legacy one is fused.
            //
            // llama.cpp PR #12801 declares the modern pair as 2-D:
            //     wk_b = {qk_nope,  n_head * kv_lora}
            //     wv_b = {kv_lora,  n_head * v_head_mla}
            // and GGUF stores them 3-D — `attn_k_b [192, 512, 20]`, `attn_v_b [512, 256, 20]`. Those are
            // THE SAME BYTES: ne0 varies fastest, so [a, b, c] is [a, b*c] without reshuffling. Reading
            // the split form therefore needs a concatenation, not a transpose or a re-layout.
            //
            // "only old legacy GGUF files will have the unsplit wkv_b tensor" — so the fused name must
            // stay supported, but it is the fallback, not the expectation.
            let kvb_split = (t("attn_k_b", "attn_k_b_trans"), t("attn_v_b", "attn_v_b_proj"));
            let kvb_fused = t("attn_kv_b", "attn_kv_b_proj");
                        let o = t("attn_output", "attn_out")?;
                        let g1 = |n: &str| fetch(n).ok();
                        Some(crate::mla::MlaWeightsT {
                            q_a: self.upload_flat(&g1(&qa)?), q_a_ln: self.upload_flat(&g1(&qaln)?),
                            q_b: self.upload_flat(&g1(&qb)?),
                            kv_a: self.upload_flat(&g1(&kva)?), kv_a_ln: self.upload_flat(&g1(&kvaln)?),
                            kv_b: match (&kvb_split, &kvb_fused) {
                    // Split form: concatenate k_b then v_b into the single [n_head*(qk_nope+v_head),
                    // kv_lora] matrix mla.rs expects. Order matters and is k THEN v, matching the fused
                    // layout llama.cpp splits FROM.
                    ((Some(kb), Some(vb)), _) => {
                        // PER-HEAD, not two blocks. mla.rs indexes kvb[head * (qk_nope + v_head)..], so
                        // appending every k row then every v row is correct only at n_heads == 1 — and
                        // head 0's k lands right by coincidence in BOTH layouts, so the wrong version loads,
                        // runs, and produces text with every head's V read from the wrong rows.
                        let (a, b) = (g1(kb)?, g1(vb)?);
                        // Dims from metadata plus the arrays themselves: n_heads and kv_lora are
                        // declared, and the per-head row counts follow from the lengths. Deriving
                        // rather than reaching for an outer binding keeps this correct if the
                        // checkpoint disagrees with a config default.
                        let nh = g.get_u32(&format!("{}.attention.head_count", acfg.prefix))? as usize;
                        let lora = g.get_u32(&format!("{}.attention.kv_lora_rank", acfg.prefix))? as usize;
                        if nh == 0 || lora == 0 { return None; }
                        let (nope, vh) = (a.len() / (nh * lora), b.len() / (nh * lora));
                        let fused = crate::mla::fuse_kv_b(&a, &b, nh, nope, vh, lora)?;
                        self.upload_flat(&fused)
                    }
                    (_, Some(f)) => self.upload_flat(&g1(f)?),
                    _ => return None,
                },
                            o: self.upload_flat(&g1(&o)?),
                        })
                    })()
                },
                // Sliding window, per layer. Read from metadata rather than assumed: the window size is
                // `<arch>.attention.sliding_window`, and the layer pattern decides which layers use it.
                // Llama-4 is CHUNKED (aligned blocks); Gemma is STANDARD (last n positions) — different
                // rules, so the type comes from the architecture, not from the presence of a window.
                swa: {
                    let n_swa = g.get_u32(&format!("{}.attention.sliding_window", acfg.prefix)).unwrap_or(0);
                    let ty = if is_llama4 { crate::swa::SwaType::Chunked } else { crate::swa::SwaType::Standard };
                    let windowed = swa_pattern.get(i).copied().unwrap_or(false);
                    // iRoPE PAIRS THESE TWO PATTERNS, and they are computed independently here — the
                    // NoPE set from `no_rope_interval`, the windowed set from
                    // `sliding_window_pattern`. In Llama-4 they must be COMPLEMENTARY: rotary layers
                    // take chunked LOCAL attention, and the NoPE layers take GLOBAL attention with
                    // length-dependent temperature instead. That pairing is the whole mechanism.
                    //
                    // If the two metadata keys disagree, a layer gets local attention AND no positional
                    // signal at all, or global attention with rotation — either way the model loads,
                    // every shape matches, and the text degrades. Warned rather than corrected, because
                    // the file is the authority and silently overriding it would hide a real
                    // disagreement.
                    if is_llama4 {
                        let is_nope = nope_step > 0 && (i + 1) % nope_step == 0;
                        if n_swa > 0 && windowed == is_nope {
                            static WARNED: std::sync::atomic::AtomicBool =
                                std::sync::atomic::AtomicBool::new(false);
                            if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                eprintln!("loader: Llama-4 layer {i} is {}windowed and {}NoPE — iRoPE \
expects these COMPLEMENTARY (rotary layers local, NoPE layers global). The two patterns come from \
different metadata keys and disagree here; output will be degraded rather than broken",
                                    if windowed { "" } else { "not " },
                                    if is_nope { "" } else { "not " });
                            }
                        }
                    }
                    if n_swa > 0 && windowed { Some((n_swa, ty)) } else { None }
                },
                // USE_QK_NORM FROM METADATA, not the expert count. This read `expert_count != 128`,
                // using "has 128 experts" as a stand-in for "is Maverick" — the variant that disables
                // post-rope Q/K norm. Scout has 16 and gets it; Maverick has 128 and does not.
                //
                // That proxy is the fifth of its kind this session, and the same failure: the name says
                // one thing and the value encodes another. A future Llama-4 with 128 experts that DOES
                // use qk-norm silently loses it, and a wrong qk-norm degrades text without breaking a
                // single shape.
                //
                // The expert count stays as the FALLBACK, because it is the observed rule and files may
                // omit the key.
                kq_norm: if is_llama4 && g.get_u32(&format!("{}.attention.use_qk_norm", acfg.prefix))
                    .map(|v| v != 0)
                    .unwrap_or_else(|| g.get_u32(&format!("{}.expert_count", acfg.prefix)).unwrap_or(0) != 128) {
                    Some(self.upload(&vec![1.0f32; head_dim], 1, head_dim))
                } else { None },
                // ATTENTION TEMPERATURE FROM METADATA. floor_scale 8192 and attn_scale 0.1 are
                // Llama-4's documented defaults, hardcoded here where the file carries them — the same
                // shape as nope_step, and the same failure: a wrong scale produces text that is merely
                // worse, with every shape and tensor correct.
                //
                // It applies to NoPE layers deliberately: in iRoPE the rotary layers get position from
                // RoPE and the NoPE layers get it from length-dependent temperature instead, so this
                // condition is the design rather than a coincidence of the same modulus.
                attn_temp: if is_llama4 && nope_step > 0 && (i + 1) % nope_step == 0 {
                    let mut t = crate::llama4::AttnTemp::default();
                    if let Some(v) = g.get_f32(&format!("{}.attention.floor_scale", acfg.prefix)) {
                        t.floor_scale = v;
                    }
                    if let Some(v) = g.get_f32(&format!("{}.attention.scale", acfg.prefix)) {
                        t.attn_scale = v;
                    }
                    Some(t)
                } else { None },
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
        // MLA rank config (DeepSeek-V3 / GLM-MoE): read the compressed-attention dims when present. llama.cpp's
        // deepseek2 GGUF convention: attention.{kv_lora_rank,q_lora_rank,key_length(qk_nope),value_length},
        // rope.dimension_count (qk_rope). Only build MlaConfig when the KV latent rank is present — that's the
        // MLA signature. Validated by the same hostile-dim bounds as the other hyperparameters.
        let mla = {
            let kv_lora = g.get_u32(&format!("{pfx}.attention.kv_lora_rank")).unwrap_or(0) as usize;
            if kv_lora > 0 {
                let q_lora = g.get_u32(&format!("{pfx}.attention.q_lora_rank")).unwrap_or(0) as usize;
                let qk_rope = g.get_u32(&format!("{pfx}.rope.dimension_count")).unwrap_or(0) as usize;
                // The per-head reconstructed key/value dims. GLM-5.2 (glm-dsa) names them *_mla and uses
                // `key_length` for the FULL qk_head (nope+rope) — e.g. key_length=576 = 512-latent?/… no:
                // key_length=576 is qk_head, key_length_mla=256 is the reconstructed nope part, rope=64.
                // Prefer the explicit *_mla keys; else derive nope = key_length - qk_rope; else fall back.
                let key_len = g.get_u32(&format!("{pfx}.attention.key_length")).map(|v| v as usize);
                let qk_nope = g.get_u32(&format!("{pfx}.attention.key_length_mla")).map(|v| v as usize)
                    .or_else(|| key_len.map(|kl| kl.saturating_sub(qk_rope)))
                    .unwrap_or(head_dim.saturating_sub(qk_rope));
                let v_head = g.get_u32(&format!("{pfx}.attention.value_length_mla")).map(|v| v as usize)
                    .or_else(|| g.get_u32(&format!("{pfx}.attention.value_length")).map(|v| v as usize))
                    .unwrap_or(qk_nope);
                // CROSS-CHECK THE FIVE INTERLOCKING DIMS AGAINST THE TENSORS. Each is redundant with a
                // shape in the file, and MLA is the architecture where reading one wrong still multiplies
                // out — the model runs on the wrong geometry and produces text rather than an error.
                // Agreement is evidence; disagreement localises the fault to a single hparam.
                {
                    let dims_of = |n: &str| g.tensor(&format!("blk.0.{n}.weight")).map(|t| t.dims.clone());
                    let q_b_w = dims_of("attn_q_b").and_then(|d| d.get(1).copied()).unwrap_or(0) as usize;
                    let out_w = dims_of("attn_output").map(|d| d[0] as usize).unwrap_or(0);
                    let n_head = g.get_u32(&format!("{pfx}.attention.head_count")).unwrap_or(0) as usize;
                    if n_head > 0 && q_b_w > 0 && out_w > 0 {
                        let (kb, vb) = (dims_of("attn_k_b"), dims_of("attn_v_b"));
                        for d in crate::mla::check_shapes(q_b_w, out_w, kb.as_deref(), vb.as_deref(),
                                                          n_head, qk_nope, qk_rope, kv_lora, v_head) {
                            eprintln!("loader: MLA SHAPE DISAGREEMENT — {}", d.explain());
                        }
                    }
                }
                // sanity-bound the MLA ranks (cf. the hostile-dim guard) before they size the latent cache
                for (name, val, max) in [("kv_lora_rank", kv_lora, 65_536usize), ("q_lora_rank", q_lora, 65_536),
                                         ("qk_rope", qk_rope, 4_096), ("qk_nope", qk_nope, 4_096), ("v_head", v_head, 4_096)] {
                    if val > max || (name != "q_lora_rank" && val == 0) {
                        return Err(format!("MLA rank '{name}' = {val} is out of range (max {max}) — corrupt/hostile header"));
                    }
                }
                Some(MlaConfig { q_lora, kv_lora, qk_nope, qk_rope, v_head })
            } else { None }
        };
        if let Some(m) = &mla {
            eprintln!("wgpu-engine: MLA config detected — kv_lora={} q_lora={} qk_nope={} qk_rope={} v_head={} (latent {} vs {} per-token KV) — forward not yet wired",
                m.kv_lora, m.q_lora, m.qk_nope, m.qk_rope, m.v_head, m.latent_width(), n_kv_heads * head_dim);
        }
        // DSA (DeepSeek Sparse Attention) indexer config: present only when index_topk is declared AND the
        // indexer weights ship. Detected here so it's visible; the sparse selection forward isn't wired yet.
        let dsa = {
            // GLM-5.2 (glm-dsa) key names: attention.indexer.{top_k,head_count,key_length}.
            let index_topk = g.get_u32(&format!("{pfx}.attention.indexer.top_k"))
                .or_else(|| g.get_u32(&format!("{pfx}.attention.index_topk"))).unwrap_or(0) as usize;
            let index_n_heads = g.get_u32(&format!("{pfx}.attention.indexer.head_count"))
                .or_else(|| g.get_u32(&format!("{pfx}.attention.index_head_count"))).unwrap_or(0) as usize;
            let index_head_dim = g.get_u32(&format!("{pfx}.attention.indexer.key_length"))
                .or_else(|| g.get_u32(&format!("{pfx}.attention.index_head_dim"))).unwrap_or(0) as usize;
            if index_topk > 0 && index_n_heads > 0 && index_head_dim > 0 && index_head_dim <= 256 {
                eprintln!("wgpu-engine: DSA indexer detected — index_topk={index_topk} heads={index_n_heads} head_dim={index_head_dim} (sparse above {index_topk} tokens, dense at/below) — forward not yet wired");
                Some(DsaConfig { index_topk, index_n_heads, index_head_dim })
            } else { None }
        };
        // Run every deferred Q4_K conversion before the model escapes. Until this returns, the weights
        // recorded above reference buffers that have been ALLOCATED but not written — so this must happen
        // here, not lazily at first use. Sealing the arena is part of the flush.
        // Where the memory actually went, split anonymous vs file-backed, plus what we are keeping mapped.
        {
            let (nmaps, mapped) = crate::gguf_retained_stats();
            eprintln!("wgpu-engine: {}", crate::memplan::rss_report());
            if nmaps > 0 {
                eprintln!("wgpu-engine: {nmaps} model file(s) retained mapped ({:.1} GiB of address space; \
file-backed pages, not anonymous memory)", crate::memplan::gib(mapped));
            }
        }
        // Report any silent tokenizer losses. Zero for a well-formed byte-level vocab; non-zero means
        // the prompt differs from what was typed, and no amount of engine debugging downstream will
        // explain it.
        if let Some(r) = crate::tokenizer::drop_report() { eprintln!("wgpu-engine: {r}"); }
        let converted = self.flush_q4k();
        // The load pass read the whole file; hand that page cache back rather than holding ~61 GiB
        // resident for the life of the process.
        let released = release_map_residency();
        if released > 0 {
            if let Some((anon, file)) = memory_split_kb() {
                eprintln!("wgpu-engine: released page cache from {released} mapped file(s) — RSS now anon {:.1} GiB + file {:.1} GiB \
(anon is the real footprint; file-backed is reclaimable)",
                    anon as f64 / (1 << 20) as f64, file as f64 / (1 << 20) as f64);
            }
        }
        if converted > 0 {
            eprintln!("wgpu-engine: Q4_K → q4 on GPU — {converted} tensors converted in batch (no host copy, no f32 round-trip)");
        }
        // Gemma multiplies the token embedding by sqrt(d) before the first block (every gemma*.cpp:
        // `inpL = ggml_scale(inpL, sqrtf(n_embd))`). Everything else uses 1.0.
        let embed_scale = if matches!(acfg.norm, crate::archcfg::Norm::RmsGemma) { (d as f32).sqrt() } else { 1.0 };
        if embed_scale != 1.0 {
            eprintln!("loader: embedding scaled by sqrt(d) = {embed_scale:.2} (Gemma)");
        }
        Ok(Model { embed, layers, out_norm, output, rope_factors, d, h, vocab, embed_scale, n_layers: n, n_heads, n_kv_heads, head_dim, base, eps, mla, dsa, mtp_layers: mtp })
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
            layers.push(BlockWeights { wqkv, wq, wk, wv, wo, wgate_up, wdown, g1, g2, q_norm: None, k_norm: None, nope: false, attn_temp: None, kq_norm: None, swa: None, mla: None, geglu: false, post_attn: None, post_ffn: None, rope_base: None, moe: None });
        }
        // Cached models are llama-family only (bias/QK-Norm/MoE are never cached) → adjacent-pair RoPE.
        self.rope_neox.store(false, std::sync::atomic::Ordering::Relaxed);
        Some(Model { embed, layers, out_norm, output, rope_factors, d, h, vocab, embed_scale: 1.0, n_layers, n_heads, n_kv_heads, head_dim: d / n_heads.max(1), base, eps, mla: None, dsa: None, mtp_layers: 0 })
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
            let q = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wq.apply(self, &n1, t), &w.q_norm, t, ad, hd, m.eps), ff, t, ad, m.base, 0, hd), w, t, ad, hd, m.eps);
            let k = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wk.apply(self, &n1, t), &w.k_norm, t, kv, hd, m.eps), ff, t, kv, m.base, 0, hd), w, t, kv, hd, m.eps);
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

// MLA attention with SEPARATE score dim (qk) and value dim (vh) — MLA's qk_head (nope+rope) and v_head
// generally differ (DeepSeek-V3: 192 vs 128), so the standard ATTENTION_MH (single hd) can't be reused.
// One query row (decode step) at position `p` = t-1, causal over t cached rows. K is [t, nh·qk] with each
// head's [k_nope | k_rot] already materialized; V is [t, nh·vh]; Q is [1, nh·qk]. One thread per head.
// Pack the kv_b reconstruction [t, nh·(qk_nope+v_head)] + shared rot cache [t, qk_rope] into full per-head
// K [t, nh·qk_head] (each head = [k_nope | k_rot]) and V [t, nh·v_head]. One thread per (row, head).
// NEOX-rope the rope-part [qk_nope, qk_head) of each head of an MLA query [1, nh*qk_head] at position pos.
const MLA_QROPE: &str = r#"
struct D { nh:u32, qk_nope:u32, qk_rope:u32, pos:u32, base:f32, _a:u32, _b:u32, _c:u32 };
@group(0)@binding(0) var<storage,read> x: array<f32>;
@group(0)@binding(1) var<storage,read_write> o: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let qk=d.qk_nope+d.qk_rope; let half=d.qk_rope/2u;
  let n=d.nh*qk;
  let id=g.x; if(id>=n){return;}
  let hh=id/qk; let c=id%qk;
  // copy through by default; overwrite the rope-half pairs
  o[id]=x[id];
  if(c>=d.qk_nope && c<d.qk_nope+half){
    let i=c-d.qk_nope;
    let base_i=hh*qk+d.qk_nope;
    let freq=f32(d.pos)/pow(d.base,(2.0*f32(i))/f32(d.qk_rope));
    let cs=cos(freq); let sn=sin(freq);
    let x0=x[base_i+i]; let x1=x[base_i+i+half];
    o[base_i+i]=x0*cs-x1*sn;
    o[base_i+i+half]=x0*sn+x1*cs;
  }
}"#;

// Pack the kv_b reconstruction [t, nh·(qk_nope+v_head)] into full per-head K [t, nh·qk_head] by
// concatenating the shared rot cache [t, qk_rope] onto each head's k_nope. One thread per (row, head).
const MLA_PACK_K: &str = r#"
struct D { t:u32, nh:u32, qk_nope:u32, qk_rope:u32, v_head:u32, _a:u32, _b:u32, _c:u32 };
@group(0)@binding(0) var<storage,read> rec: array<f32>;
@group(0)@binding(1) var<storage,read> rot: array<f32>;
@group(0)@binding(2) var<storage,read_write> kout: array<f32>;
@group(0)@binding(3) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let id=g.x; let n=d.t*d.nh; if(id>=n){return;}
  let row=id/d.nh; let hh=id%d.nh;
  let recw=d.nh*(d.qk_nope+d.v_head); let recb=row*recw+hh*(d.qk_nope+d.v_head);
  let qk=d.qk_nope+d.qk_rope; let kb=row*(d.nh*qk)+hh*qk;
  for(var c=0u;c<d.qk_nope;c=c+1u){ kout[kb+c]=rec[recb+c]; }
  for(var c=0u;c<d.qk_rope;c=c+1u){ kout[kb+d.qk_nope+c]=rot[row*d.qk_rope+c]; }
}"#;

const MLA_PACK_V: &str = r#"
struct D { t:u32, nh:u32, qk_nope:u32, qk_rope:u32, v_head:u32, _a:u32, _b:u32, _c:u32 };
@group(0)@binding(0) var<storage,read> rec: array<f32>;
@group(0)@binding(1) var<storage,read_write> vout: array<f32>;
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let id=g.x; let n=d.t*d.nh; if(id>=n){return;}
  let row=id/d.nh; let hh=id%d.nh;
  let recw=d.nh*(d.qk_nope+d.v_head); let recb=row*recw+hh*(d.qk_nope+d.v_head);
  let vb=row*(d.nh*d.v_head)+hh*d.v_head;
  for(var c=0u;c<d.v_head;c=c+1u){ vout[vb+c]=rec[recb+d.qk_nope+c]; }
}"#;

const MLA_ATTN: &str = r#"
struct D { t:u32, nh:u32, qk:u32, vh:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> k: array<f32>;
@group(0)@binding(2) var<storage,read> v: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let hh=g.x; if(hh>=d.nh){return;}
  let qkdm=d.nh*d.qk; let vdm=d.nh*d.vh;
  let qb=hh*d.qk; let vb=hh*d.vh;
  let scale=1.0/sqrt(f32(d.qk));
  var mx=-3.4e38;
  for(var j=0u;j<d.t;j=j+1u){ var dot=0.0; for(var c=0u;c<d.qk;c=c+1u){dot=dot+q[qb+c]*k[j*qkdm+qb+c];} mx=max(mx,dot*scale); }
  for(var c=0u;c<d.vh;c=c+1u){o[vb+c]=0.0;}
  var sum=0.0;
  for(var j=0u;j<d.t;j=j+1u){
    var dot=0.0; for(var c=0u;c<d.qk;c=c+1u){dot=dot+q[qb+c]*k[j*qkdm+qb+c];}
    let pw=exp(dot*scale-mx); sum=sum+pw;
    for(var c=0u;c<d.vh;c=c+1u){o[vb+c]=o[vb+c]+pw*v[j*vdm+vb+c];}
  }
  for(var c=0u;c<d.vh;c=c+1u){o[vb+c]=o[vb+c]/sum;}
}"#;

// DSA lightning-indexer scoring (see dsa.rs::indexer_scores). One thread per cached key t computes
//   score[t] = (1/√nh) · Σ_h w[h] · ReLU( (q_idx[h]·k_idx[t]) / √hd )
// q_idx is [nh·hd] (rope'd), k_idx is [nk, hd] (one hd-wide key per position, shared across heads), w is
// [nh]. Bit-exact to the CPU reference: same per-head ReLU-then-weight, same √hd / √nh scaling, same order
// (heads inner, so the fp accumulation matches). This is the score that ranks keys for top-k selection.
const DSA_INDEXER: &str = r#"
struct D { nk:u32, nh:u32, hd:u32, _p:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;   // [nh*hd]
@group(0)@binding(1) var<storage,read> k: array<f32>;   // [nk*hd]
@group(0)@binding(2) var<storage,read> w: array<f32>;   // [nh]
@group(0)@binding(3) var<storage,read_write> o: array<f32>; // [nk]
@group(0)@binding(4) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let t=g.x; if(t>=d.nk){return;}
  let wsc=1.0/sqrt(f32(d.nh));
  let rs=1.0/sqrt(f32(d.hd));
  let kbase=t*d.hd;
  var a=0.0;
  for(var h=0u;h<d.nh;h=h+1u){
    let qbase=h*d.hd;
    var d0=0.0;
    for(var i=0u;i<d.hd;i=i+1u){ d0=d0+q[qbase+i]*k[kbase+i]; }
    d0=d0*rs;
    if(d0>0.0){ a=a+w[h]*d0; }   // ReLU on the per-head score, then weight
  }
  o[t]=a*wsc;
}"#;

// DSA masked attention (see dsa.rs::masked_attention). One thread per value-dim column; a single query
// attends over only the keys whose mask[t] != 0, softmax renormalized over the selected set. q is [dk], k
// is [nk·dk], v is [nk·dv], mask is [nk]. With an all-ones mask this is exactly dense attention (the
// dense-equivalence anchor). Each thread owns one output column c and streams the selected keys twice
// (max, then weighted sum) so no shared memory is needed and it's bit-comparable to the CPU reference.
const DSA_MASKED_ATTN: &str = r#"
struct D { nk:u32, dk:u32, dv:u32, _p:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;    // [dk]
@group(0)@binding(1) var<storage,read> k: array<f32>;    // [nk*dk]
@group(0)@binding(2) var<storage,read> v: array<f32>;    // [nk*dv]
@group(0)@binding(3) var<storage,read> mask: array<f32>; // [nk]
@group(0)@binding(4) var<storage,read_write> o: array<f32>; // [dv]
@group(0)@binding(5) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let c=g.x; if(c>=d.dv){return;}
  let scale=1.0/sqrt(f32(d.dk));
  // pass 1: max over selected keys (same score each thread recomputes — cheap, dk small; keeps it
  // one-output-per-thread with no cross-thread reduction).
  var mx=-3.4e38;
  for(var t=0u;t<d.nk;t=t+1u){
    if(mask[t]==0.0){continue;}
    var dot=0.0; let kb=t*d.dk; for(var i=0u;i<d.dk;i=i+1u){dot=dot+q[i]*k[kb+i];}
    let s=dot*scale; if(s>mx){mx=s;}
  }
  // pass 2: weighted sum of this column
  var denom=0.0; var acc=0.0;
  for(var t=0u;t<d.nk;t=t+1u){
    if(mask[t]==0.0){continue;}
    var dot=0.0; let kb=t*d.dk; for(var i=0u;i<d.dk;i=i+1u){dot=dot+q[i]*k[kb+i];}
    let p=exp(dot*scale-mx); denom=denom+p;
    acc=acc+p*v[t*d.dv+c];
  }
  o[c]=acc/denom;
}"#;

// DSA top-k selection as a MASK (see dsa.rs::select_topk). One thread per key t sets mask[t]=1 iff key t is
// among the top `keep` by score, breaking ties by position (earlier wins) — exactly the reference's
// "> thr then == thr in ascending position order" rule, computed as a stable rank:
//   rank(t) = #{j: score[j] > score[t]} + #{j: score[j] == score[t] AND j < t};  mask[t] = rank(t) < keep.
// When keep >= nk every rank < nk <= keep, so all keys select (the no-op-below-threshold case) with no
// special branch. O(nk²) but nk is bounded by index_topk; matches the CPU set bit-for-bit incl. ties.
const DSA_TOPK_MASK: &str = r#"
struct D { nk:u32, keep:u32, _a:u32, _b:u32 };
@group(0)@binding(0) var<storage,read> s: array<f32>;        // [nk] scores
@group(0)@binding(1) var<storage,read_write> mask: array<f32>; // [nk]
@group(0)@binding(2) var<uniform> d: D;
@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>){
  let t=g.x; if(t>=d.nk){return;}
  let st=s[t];
  var rank=0u;
  for(var j=0u;j<d.nk;j=j+1u){
    let sj=s[j];
    if(sj>st){ rank=rank+1u; }
    else if(sj==st && j<t){ rank=rank+1u; }   // stable tie-break: earlier position ranks higher
  }
  if(rank<d.keep){ mask[t]=1.0; } else { mask[t]=0.0; }
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
// PARALLEL prefill attention: one workgroup per (query row, head), 64 threads splitting the causal KV
// loop [0, qpos] with a shared-memory softmax reduction — instead of ATTENTION_MH_EXT's single serial
// thread crawling the whole cache per row. Same math, same causal mask, but the per-row O(qpos·hd) work
// is spread across the GPU. This is what keeps each prefill dispatch SHORT (no watchdog trip) and fast.
//
// Shared score buffer sized at 2048 f32 = 8 KB, NOT the 32 KB max. This is deliberate, grounded in the
// Apple-GPU occupancy rule: every core has ~32 KB of threadgroup memory, and a workgroup's allocation
// divides how many run concurrently — 8 KB ⇒ 4 groups/core, 16 KB ⇒ 2, 32 KB ⇒ 1 (a matmul benchmark
// measured 400 vs 250 GFLOPS for 8 KB vs 32 KB for exactly this reason). 8 KB keeps occupancy high while
// still covering causal rows up to 2048 in one workgroup; longer rows fall back to the serial kernel
// (correct, slower) until a split-K causal prefill lands. `SCTILE` must match the array size below.
const ATTENTION_MH_EXT_PAR: &str = r#"
// `nswa`/`swaty` add sliding-window attention. Unlike decode — where a single query position gives one
// contiguous range — every query ROW here has its own window, so the bound is computed per row from that
// row's global position. It is still a range rather than a mask, because both windowing rules are
// contiguous: the tile loop simply starts at the window instead of at 0. swaty: 0 none, 1 standard
// (Gemma, last n positions), 2 chunked (Llama-4, current aligned chunk).
struct D { s:u32,hd:u32,nh:u32,nkv:u32,base:u32,nswa:u32,swaty:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> kc: array<f32>;
@group(0)@binding(2) var<storage,read> vc: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> dd: D;
var<workgroup> sc: array<f32,2048>;
var<workgroup> red: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let row=wg.x;                                       // which (query row, head) this workgroup handles
  let i=row/dd.nh; let hh=row%dd.nh; let tid=lid.x;
  let dm=dd.nh*dd.hd; let kvdm=dd.nkv*dd.hd;
  let b=hh*dd.hd; let kb=(hh/(dd.nh/dd.nkv))*dd.hd;   // GQA: query head -> shared KV head
  let scale=1.0/sqrt(f32(dd.hd));
  let qpos=dd.base+i;                                 // causal bound: attend to [0, qpos]
  let l=qpos+1u;
  let TILE=2048u;                                     // KV rows per shared-memory tile (matches sc size)
  // Online-softmax over KV TILES so any causal length works at 8 KB shared memory (high occupancy):
  // per tile, load scores into sc, then fold the tile into a running (max m, denom d, acc[hd]) with the
  // standard rescale. acc for this row's head_dim is kept in registers, strided across the 64 threads.
  var m=-3.4e38; var den=0.0;
  // per-thread partial accumulator over head_dim columns this thread owns (c = tid, tid+64, …)
  var acc0=0.0; var acc1=0.0; var acc2=0.0;           // hd<=192 for tid-strided (hd/64) cols; 128→2 cols
  // sliding window: this row's earliest visible KV position (0 when unwindowed)
  var lo=0u;
  if(dd.nswa>0u){
    if(dd.swaty==1u){ if(l>dd.nswa){ lo=l-dd.nswa; } }        // standard: keep the last nswa positions
    else if(dd.swaty==2u){ lo=(qpos/dd.nswa)*dd.nswa; }        // chunked: start of qpos's aligned chunk
  }
  var t0=lo;
  loop {
    if(t0>=l){break;}
    let t1=min(l, t0+TILE);
    // scores for this tile → sc[0 .. t1-t0), and this tile's local max
    var lmax=-3.4e38; var j=t0+tid;
    loop { if(j>=t1){break;} var dot=0.0; for(var c=0u;c<dd.hd;c=c+1u){dot=dot+q[i*dm+b+c]*kc[j*kvdm+kb+c];} let sv=dot*scale; sc[j-t0]=sv; lmax=max(lmax,sv); j=j+64u; }
    red[tid]=lmax; workgroupBarrier();
    for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ red[tid]=max(red[tid],red[tid+st]); } workgroupBarrier(); }
    let tilemax=red[0]; workgroupBarrier();
    // new running max and the rescale factor for the accumulator/denominator so far
    let newm=max(m, tilemax);
    let resc=exp(m-newm);
    // exponentiate this tile's scores against newm; tile denom
    var tden=0.0; j=t0+tid;
    loop { if(j>=t1){break;} let p=exp(sc[j-t0]-newm); sc[j-t0]=p; tden=tden+p; j=j+64u; }
    red[tid]=tden; workgroupBarrier();
    for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ red[tid]=red[tid]+red[tid+st]; } workgroupBarrier(); }
    let tilesum=red[0]; workgroupBarrier();
    den=den*resc+tilesum;
    // fold tile's V-weighted contribution into the register accumulators (rescaled), cols strided by 64
    acc0=acc0*resc; acc1=acc1*resc; acc2=acc2*resc;
    var c=tid; var slot=0u;
    loop { if(c>=dd.hd){break;}
      var a=0.0; for(var jj=t0;jj<t1;jj=jj+1u){ a=a+sc[jj-t0]*vc[jj*kvdm+kb+c]; }
      if(slot==0u){acc0=acc0+a;} else if(slot==1u){acc1=acc1+a;} else {acc2=acc2+a;}
      c=c+64u; slot=slot+1u;
    }
    m=newm; t0=t0+TILE;
    workgroupBarrier();
  }
  // write normalized output: each thread writes the cols it owns
  var c=tid; var slot=0u;
  loop { if(c>=dd.hd){break;}
    var a=acc0; if(slot==1u){a=acc1;} else if(slot==2u){a=acc2;}
    o[i*dm+b+c]=a/den; c=c+64u; slot=slot+1u;
  }
}"#;

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

// Two-span attention: the WARM (int8) span emitted as flash pass-1 partials. Same partial layout as
// DECODE_ATTN_FLASH_P1 ([hd accumulators | local_max | local_sum] per (head, chunk)) so the SAME pass-2
// combine folds warm chunks together with the hot f32 chunks — one online-softmax over the union. Reads
// int8 K/V (packed u32 + per-row f32 scale) with inline dequant, matching DECODE_ATTN_MH_Q8's layout.
// Writing into a shared `part` buffer at a chunk offset means warm and hot partials coexist and pass-2
// needs no changes. `coff` = the chunk index where this span's chunks start (hot span occupies [0,coff)).
const DECODE_ATTN_FLASH_P1_Q8: &str = r#"
struct D { l:u32,hd:u32,nh:u32,nkv:u32,chunk:u32,nch:u32,coff:u32,_b:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> kq: array<u32>;
@group(0)@binding(2) var<storage,read> ks: array<f32>;
@group(0)@binding(3) var<storage,read> vq: array<u32>;
@group(0)@binding(4) var<storage,read> vs: array<f32>;
@group(0)@binding(5) var<storage,read_write> part: array<f32>;
@group(0)@binding(6) var<uniform> dd: D;
var<workgroup> sc: array<f32,2048>;
var<workgroup> red: array<f32,64>;
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let hh=wg.x; let ch=wg.y; let tid=lid.x;
  let kvdm=dd.nkv*dd.hd; let kvw=kvdm/4u;
  let b=hh*dd.hd; let kb=(hh/(dd.nh/dd.nkv))*dd.hd;
  let scale=1.0/sqrt(f32(dd.hd));
  let j0=ch*dd.chunk; let j1=min(dd.l, j0+dd.chunk);
  // this span's chunks are placed AFTER the hot span's chunks (coff), so pass-2 sees one chunk list
  let pbase=(hh*dd.nch+(dd.coff+ch))*(dd.hd+2u);
  var lmax=-3.4e38; var j=j0+tid;
  loop { if(j>=j1){break;}
    let ksc=ks[j]; var dot=0.0;
    for(var c=0u;c<dd.hd;c=c+1u){
      let idx=kb+c; let word=kq[j*kvw + idx/4u];
      let ub=extractBits(word,(idx%4u)*8u,8u); let qi=select(i32(ub), i32(ub)-256, ub>=128u);
      dot=dot+q[b+c]*(f32(qi)*ksc);
    }
    let s=dot*scale; sc[j-j0]=s; lmax=max(lmax,s); j=j+64u; }
  red[tid]=lmax; workgroupBarrier();
  for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ red[tid]=max(red[tid],red[tid+st]); } workgroupBarrier(); }
  let mx=red[0]; workgroupBarrier();
  var lsum=0.0; j=j0+tid;
  loop { if(j>=j1){break;} let p=exp(sc[j-j0]-mx); sc[j-j0]=p; lsum=lsum+p; j=j+64u; }
  red[tid]=lsum; workgroupBarrier();
  for(var st=32u; st>0u; st=st>>1u){ if(tid<st){ red[tid]=red[tid]+red[tid+st]; } workgroupBarrier(); }
  let sum=red[0]; workgroupBarrier();
  var c=tid;
  loop { if(c>=dd.hd){break;}
    var acc=0.0;
    for(var jj=j0;jj<j1;jj=jj+1u){
      let idx=kb+c; let word=vq[jj*kvw + idx/4u];
      let ub=extractBits(word,(idx%4u)*8u,8u); let qi=select(i32(ub), i32(ub)-256, ub>=128u);
      acc=acc+sc[jj-j0]*(f32(qi)*vs[jj]);
    }
    part[pbase+c]=acc; c=c+64u; }
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
// `start` is the first KV row to attend to — sliding-window attention for a single query position is a
// contiguous range, so it is a loop bound rather than a mask. Passed as an offset instead of binding a
// sub-view because the q8 cache keeps ONE SCALE PER ROW in a separate tensor, and a view into that would
// need 64-row alignment which a sliding window almost never has. start = 0 is the unwindowed case.
struct D { l:u32,hd:u32,nh:u32,nkv:u32,start:u32 };
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
  for(var j=dd.start;j<dd.l;j=j+1u){
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
  for(var j=dd.start;j<dd.l;j=j+1u){
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
pub struct KvCache {
    pub k: Vec<Tensor>, pub v: Vec<Tensor>,
    /// Compressed MLA cache, allocated only for MLA models. Held ALONGSIDE k/v rather than replacing
    /// them: a checkpoint may mix MLA and standard attention layers, and the per-layer routing decides
    /// which is used. For a non-MLA model this is None and nothing about the standard path changes.
    pub mla: Option<crate::mla::MlaCache>,
}

/// int8-quantized KV cache: K/V stored per-row as packed int8 (4x smaller than f32)
/// plus one f32 scale per row. Raises the concurrency and context ceilings (KV memory
/// is the cap once compute scales). ~4x smaller than the f32 cache.
pub struct QKvCache { pub kq: Vec<Tensor>, pub ks: Vec<Tensor>, pub vq: Vec<Tensor>, pub vs: Vec<Tensor> }

/// A STAGED KV cache: a hot f32 window (recent tokens, full precision) over a warm int8 tail (older
/// tokens, 4× smaller). Combines `KvCache` (hot) + `QKvCache` (warm) at a moving boundary owned by a
/// `kvstage::StagePolicy`. Recent tokens dominate attention, so they stay f32; the cold tail is demoted.
/// This is the middle ground between the all-f32 `KvCache` (best quality, biggest) and the all-int8
/// `QKvCache` (smallest, lower quality), and the substrate 1M context + DSA build on.
///
/// Bit-exactness contract: with `policy = StagePolicy::all_hot()` NOTHING is ever demoted — the warm
/// tail stays empty and this behaves exactly like a plain `KvCache`. That is the no-op validation gate.
pub struct StagedKvCache {
    /// Hot tier: f32 K/V for tokens `[policy.hot_start, pos)`. Written every step like the f32 cache.
    pub hot: KvCache,
    /// Warm tier: int8 K/V for tokens `[0, policy.hot_start)`. Empty until the first block demotes.
    pub warm: QKvCache,
    /// The staging boundary + demotion rules (pure; see kvstage.rs).
    pub policy: crate::kvstage::StagePolicy,
    /// Warm tail length in tokens (== policy.hot_start; cached to avoid recomputm). Kept in sync on demote.
    pub warm_len: usize,
    /// Row count of each demoted warm block, in demote order (same for every layer). Lets multi-block
    /// attention size each block's flash chunks. `warm_block_rows.iter().sum() == warm_len`.
    pub warm_block_rows: Vec<usize>,
}

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
    // Skip non-finite entries (NaN/±Inf): a single poisoned logit must not pin the argmax. `x > bv` is
    // false for NaN, so a leading NaN with `bv=f32::MIN` would silently return index 0 forever (the
    // Colibri #369 failure). Seed from the first FINITE value and only compare finite entries.
    let mut bi = 0usize; let mut bv = f32::NEG_INFINITY; let mut seen = false;
    for (i, &x) in v.iter().enumerate() {
        if x.is_finite() && (!seen || x > bv) { bv = x; bi = i; seen = true; }
    }
    // all-non-finite (fully poisoned row): fall back to index 0 rather than an out-of-range/garbage pick.
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
    /// NEOX split-half RoPE at position `pos0` over [t, d] (rotates i with i+d/2). Matches mla.rs.
    pub fn rope_neox_at_t(&self, x: &Tensor, t: usize, d: usize, base: f32, pos0: usize) -> Tensor {
        let out = self.alloc(t, d); let dims = [t as u32, d as u32, base.to_bits(), pos0 as u32];
        self.dispatch(ROPE_NEOX, &[x], &out, bytemuck::cast_slice(&dims), (((t * d / 2) as u32).div_ceil(64), 1, 1)); out
    }

    /// GPU MLA attention for one new query token at `pos`, over a compressed latent cache. Composes the
    /// already-validated primitives (linear_t / rmsnorm_t / rope_neox / softmax) so it can be checked
    /// bit-for-bit against the mla.rs CPU reference. Un-absorbed form: reconstruct per-head K(nope)+V from
    /// each cached latent via kv_b, then ordinary attention. Inputs are uploaded weights + the cache latent
    /// [pos+1, kv_lora] and rot [pos+1, qk_rope]; returns [1, d] (post o-proj). NOT wired into decode yet —
    /// this proves the GPU forward matches the reference before it's threaded into a live model.
    #[allow(clippy::too_many_arguments)]
    pub fn mla_attention_step_t(&self, x: &Tensor, w: &crate::mla::MlaWeightsT, dm: &crate::mla::MlaDims,
                                c_kv: &Tensor, k_rot: &Tensor, pos: usize) -> Tensor {
        let h = dm.n_heads; let qk = dm.qk_head(); let rec = dm.qk_nope + dm.v_head;
        // Q: x[1,d] → q_a[q_lora] → norm → q_b[h·qk] ; RoPE the rope tail of each head.
        let qa = self.linear_t(x, &w.q_a, 1, dm.q_lora, dm.d);
        let qa = self.rmsnorm_t(&qa, &w.q_a_ln, 1, dm.q_lora, dm.eps);
        let q = self.linear_t(&qa, &w.q_b, 1, h * qk, dm.q_lora);
        // rope each head's [qk_nope..qk_head) slice. Do it on CPU-shaped slices via a gather? Simpler: pull
        // the q row, rope the rope-parts per head to match the reference, re-upload. (Small: h·qk floats.)
        let mut qh = self.download(&q);
        for head in 0..h {
            let off = head * qk + dm.qk_nope;
            let slice = self.upload(&qh[off..off + dm.qk_rope].to_vec(), 1, dm.qk_rope);
            let roped = self.download(&self.rope_neox_at_t(&slice, 1, dm.qk_rope, dm.base, pos));
            qh[off..off + dm.qk_rope].copy_from_slice(&roped);
        }
        // reconstruct K(nope)+V for all cached positions: kv_b · c_kv → [pos+1, h·rec]
        let kvb = self.linear_t(c_kv, &w.kv_b, pos + 1, h * rec, dm.kv_lora);
        let kvb_host = self.download(&kvb);
        let krot_host = self.download(k_rot);
        let scale = 1.0 / (qk as f32).sqrt();
        let mut out = vec![0f32; h * dm.v_head];
        for head in 0..h {
            let qslice = &qh[head * qk..head * qk + qk];
            let mut scores = vec![0f32; pos + 1];
            for t in 0..=pos {
                let k_nope = &kvb_host[t * h * rec + head * rec..t * h * rec + head * rec + dm.qk_nope];
                let mut s = 0f32;
                for i in 0..dm.qk_nope { s += qslice[i] * k_nope[i]; }
                for i in 0..dm.qk_rope { s += qslice[dm.qk_nope + i] * krot_host[t * dm.qk_rope + i]; }
                scores[t] = s * scale;
            }
            let mx = scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let mut denom = 0f32;
            for s in scores.iter_mut() { *s = (*s - mx).exp(); denom += *s; }
            for s in scores.iter_mut() { *s /= denom; }
            for t in 0..=pos {
                let v = &kvb_host[t * h * rec + head * rec + dm.qk_nope..t * h * rec + head * rec + dm.qk_nope + dm.v_head];
                let a = scores[t];
                for i in 0..dm.v_head { out[head * dm.v_head + i] += a * v[i]; }
            }
        }
        // o-proj [d, h·v_head]
        let out_t = self.upload(&out, 1, h * dm.v_head);
        self.linear_t(&out_t, &w.o, 1, dm.d, h * dm.v_head)
    }

    /// Fully-on-GPU MLA attention step: same math as `mla_attention_step_t` but the score/softmax/ΣV inner
    /// loop and the query-rope + KV-pack run as GPU kernels (no CPU readback mid-attention). Validated
    /// bit-for-bit against the CPU reference. This is the MLA perf path.
    #[allow(clippy::too_many_arguments)]
    pub fn mla_attention_step_gpu(&self, x: &Tensor, w: &crate::mla::MlaWeightsT, dm: &crate::mla::MlaDims,
                                  c_kv: &Tensor, k_rot: &Tensor, pos: usize) -> Tensor {
        let h = dm.n_heads; let qk = dm.qk_head(); let rec = dm.qk_nope + dm.v_head; let t = pos + 1;
        // Q: x → q_a → norm → q_b → per-head q; NEOX-rope the rope part of each head (GPU).
        let qa = self.linear_t(x, &w.q_a, 1, dm.q_lora, dm.d);
        let qa = self.rmsnorm_t(&qa, &w.q_a_ln, 1, dm.q_lora, dm.eps);
        let q = self.linear_t(&qa, &w.q_b, 1, h * qk, dm.q_lora);
        let q_roped = self.alloc(1, h * qk);
        let qd = [h as u32, dm.qk_nope as u32, dm.qk_rope as u32, pos as u32, dm.base.to_bits(), 0, 0, 0];
        self.dispatch(MLA_QROPE, &[&q], &q_roped, bytemuck::cast_slice(&qd), (((h * qk) as u32).div_ceil(64), 1, 1));
        // Reconstruct per-head K_nope+V from the whole latent cache (GPU matmul): [t, h·rec].
        let recon = self.linear_t(c_kv, &w.kv_b, t, h * rec, dm.kv_lora);
        // Pack into full per-head K [t, h·qk] and V [t, h·vh] (GPU, two single-output kernels).
        let k_full = self.alloc(t, h * qk);
        let v_full = self.alloc(t, h * dm.v_head);
        let pd = [t as u32, h as u32, dm.qk_nope as u32, dm.qk_rope as u32, dm.v_head as u32, 0, 0, 0];
        self.dispatch(MLA_PACK_K, &[&recon, k_rot], &k_full, bytemuck::cast_slice(&pd), (((t * h) as u32).div_ceil(64), 1, 1));
        self.dispatch(MLA_PACK_V, &[&recon], &v_full, bytemuck::cast_slice(&pd), (((t * h) as u32).div_ceil(64), 1, 1));
        // MLA attention over the packed K/V (GPU): one query row, causal over t.
        let ctx = self.alloc(1, h * dm.v_head);
        let ad = [t as u32, h as u32, qk as u32, dm.v_head as u32];
        self.dispatch(MLA_ATTN, &[&q_roped, &k_full, &v_full], &ctx, bytemuck::cast_slice(&ad), ((h as u32).div_ceil(64), 1, 1));
        // o-proj.
        self.linear_t(&ctx, &w.o, 1, dm.d, h * dm.v_head)
    }

    /// Allocate a compressed MLA cache: per-layer latent [max_seq, kv_lora] + rot [max_seq, qk_rope]. This
    /// is the long-context memory win — the cache is O(kv_lora+qk_rope)/token instead of O(n_kv_heads·hd).
    pub fn mla_new_cache(&self, n_layers: usize, max_seq: usize, dm: &crate::mla::MlaDims) -> crate::mla::MlaCache {
        crate::mla::MlaCache {
            latent: (0..n_layers).map(|_| self.alloc(max_seq, dm.kv_lora)).collect(),
            rot: (0..n_layers).map(|_| self.alloc(max_seq, dm.qk_rope)).collect(),
            kv_lora: dm.kv_lora, qk_rope: dm.qk_rope, max_seq,
        }
    }

    /// Compress one token's hidden state into its MLA cache row and append at `pos` in the given layer:
    /// x → kv_a → [latent | rot]; RMSNorm the latent, NEOX-rope the rot; write both into the cache. Returns
    /// nothing — the cache holds the compressed row. Mirrors mla.rs::compress_kv on-GPU.
    pub fn mla_compress_append(&self, x: &Tensor, w: &crate::mla::MlaWeightsT, dm: &crate::mla::MlaDims,
                               cache: &crate::mla::MlaCache, layer: usize, pos: usize) {
        let kva = self.linear_t(x, &w.kv_a, 1, dm.kv_lora + dm.qk_rope, dm.d);
        // latent part [0,kv_lora) → RMSNorm → cache.latent[layer][pos]
        let latent = self.slice_cols(&kva, 0, dm.kv_lora);
        let latent = self.rmsnorm_t(&latent, &w.kv_a_ln, 1, dm.kv_lora, dm.eps);
        self.write_row(&cache.latent[layer], &latent, pos, dm.kv_lora);
        // rot part [kv_lora, kv_lora+qk_rope) → NEOX rope at pos → cache.rot[layer][pos]
        let rot = self.slice_cols(&kva, dm.kv_lora, dm.qk_rope);
        let rot = self.rope_neox_at_t(&rot, 1, dm.qk_rope, dm.base, pos);
        self.write_row(&cache.rot[layer], &rot, pos, dm.qk_rope);
    }

    /// DSA lightning-indexer scores on GPU: one score per cached key, bit-exact to dsa.rs::indexer_scores.
    /// `q_idx` [nh·hd], `k_idx` [nk·hd], `w` [nh] → [nk] scores. These rank keys for the top-k sparse
    /// selection; the selection is then fed to the main attention mask (above the index_topk threshold).
    pub fn dsa_indexer_scores_t(&self, q_idx: &Tensor, k_idx: &Tensor, w: &Tensor, nk: usize, nh: usize, hd: usize) -> Tensor {
        let out = self.alloc(1, nk);
        let dims = [nk as u32, nh as u32, hd as u32, 0u32];
        self.dispatch(DSA_INDEXER, &[q_idx, k_idx, w], &out, bytemuck::cast_slice(&dims), ((nk as u32).div_ceil(64), 1, 1));
        out
    }

    /// DSA masked attention on GPU: single query attends over only the keys with mask[t] != 0, bit-exact to
    /// dsa.rs::masked_attention. q [dk], k [nk·dk], v [nk·dv], mask [nk] → [dv]. All-ones mask == dense
    /// attention (the dense-equivalence anchor). This is where the DSA selection meets the main attention.
    pub fn dsa_masked_attention_t(&self, q: &Tensor, k: &Tensor, v: &Tensor, mask: &Tensor, nk: usize, dk: usize, dv: usize) -> Tensor {
        let out = self.alloc(1, dv);
        let dims = [nk as u32, dk as u32, dv as u32, 0u32];
        self.dispatch(DSA_MASKED_ATTN, &[q, k, v, mask], &out, bytemuck::cast_slice(&dims), ((dv as u32).div_ceil(64), 1, 1));
        out
    }

    /// DSA top-k selection as a mask on GPU: mask[t]=1 iff key t is among the top `keep` scores (ties by
    /// position). `keep` = min(nk, index_topk); when keep>=nk all keys select (no-op below threshold).
    /// Bit-exact to dsa.rs::select_topk (as a mask). This is the selection the masked attention consumes.
    pub fn dsa_topk_mask_t(&self, scores: &Tensor, nk: usize, keep: usize) -> Tensor {
        let out = self.alloc(1, nk);
        let dims = [nk as u32, keep as u32, 0u32, 0u32];
        self.dispatch(DSA_TOPK_MASK, &[scores], &out, bytemuck::cast_slice(&dims), ((nk as u32).div_ceil(64), 1, 1));
        out
    }
    pub fn new_cache(&self, n_layers: usize, max_seq: usize, d: usize) -> KvCache {
        KvCache { k: (0..n_layers).map(|_| self.alloc(max_seq, d)).collect(),
                  v: (0..n_layers).map(|_| self.alloc(max_seq, d)).collect(),
                  mla: None }
    }
    /// A cache for `m`, adding the compressed MLA side when the model needs it.
    ///
    /// The standard k/v vectors are still allocated for MLA models. They cost little relative to the
    /// weights and a mixed checkpoint — some layers MLA, some standard — would otherwise need a second
    /// cache type threaded everywhere. Correctness first; trimming the unused allocation is an
    /// optimisation with a measurement attached, not a guess.
    pub fn new_cache_for(&self, m: &Model, max_seq: usize) -> KvCache {
        let mut c = self.new_cache(m.n_layers, max_seq, m.kv_dim());
        if let Some(cfg) = &m.mla {
            let dm = self.mla_dims(cfg, m.d, m.n_heads, m.base, m.eps);
            c.mla = Some(self.mla_new_cache(m.n_layers, max_seq, &dm));
        }
        c
    }

    /// Buffer-to-buffer copy that records into the open frame if one is active (so cache
    /// writes batch with the token's compute), else submits on its own.
    fn copy_buf(&self, src: &Tensor, src_off: u64, dst: &Tensor, dst_off: u64, bytes: u64) {
        if let Some(frame) = self.frame.lock().unwrap().as_mut() {
            frame.enc.copy_buffer_to_buffer(&src.buf, src_off, &dst.buf, dst_off, bytes);
        } else {
            let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            enc.copy_buffer_to_buffer(&src.buf, src_off, &dst.buf, dst_off, bytes);
            // A device-side buffer copy, grouped with downloads as DATA MOVEMENT rather than
            // compute. Not a readback — worth saying, because a counter named for one thing and
            // counting another is the mislabel class this codebase keeps paying for. If the two
            // ever need separating, this is the line to split.
            SUBMIT_BY_DOWNLOAD.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        // base == 0.0 is the NoPE sentinel (Llama4 iRoPE layers): identity pass-through, so every
        // block fn keeps one shape/flow and the per-layer switch is a single line at its top.
        if base == 0.0 { self.copy_buf(x, 0, &out, 0, ((x.rows * x.cols).min(t * dm) * 4) as u64); return out; }
        let neox = self.rope_neox.load(std::sync::atomic::Ordering::Relaxed) as u32;
        let msc = self.rope_mscale.load(std::sync::atomic::Ordering::Relaxed);
        let dims = [t as u32, dm as u32, base.to_bits(), pos0 as u32, head_dim as u32, neox, msc, 0];
        self.dispatch(ROPE_MH, &[x, ff], &out, bytemuck::cast_slice(&dims), wg1((t * dm / 2) as u32)); out
    }

    /// Qwen3/3.5 QK-Norm: per-head RMSNorm over `head_dim`, applied to a projected `[t, width]` tensor
    /// (width = heads·head_dim) BEFORE RoPE. `norm` is the [head_dim] weight; `None` → pass-through (llama/
    /// qwen2). Since head_dim is contiguous per head, the [t, width] buffer viewed as [t·heads, head_dim]
    /// rows makes this a plain rmsnorm_t — no new kernel — then we restore the [t, width] shape for RoPE.
    /// Llama-4's post-rope Q/K normalization. No-op unless the layer carries `kq_norm`, and skipped on
    /// NoPE layers — those take the attention-temperature path instead, exactly as llama.cpp gates it with
    /// `if (use_rope && hparams.use_kq_norm)`.
    fn kq_post(&self, x: Tensor, w: &BlockWeights, t: usize, width: usize, head_dim: usize, eps: f32) -> Tensor {
        if w.nope || w.kq_norm.is_none() { return x; }
        self.qk_norm(x, &w.kq_norm, t, width, head_dim, eps)
    }

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
        self.attention_mh_ext_swa(q, kc, vc, base, s, dm, n_heads, n_kv_heads, None)
    }

    /// As above, with an optional sliding window applied per query row.
    ///
    /// Only the PARALLEL kernel implements the window. The serial fallback (hd > 192) is left causal-only
    /// and this refuses to pretend otherwise — silently ignoring the window there would give a model with
    /// an unusual head_dim quietly wrong prefill, which is the failure mode this project keeps paying for.
    pub fn attention_mh_ext_swa(&self, q: &Tensor, kc: &Tensor, vc: &Tensor, base: usize, s: usize, dm: usize,
                                n_heads: usize, n_kv_heads: usize,
                                swa: Option<(u32, crate::swa::SwaType)>) -> Tensor {
        let out = self.alloc(s, dm); let hd = dm / n_heads;
        let (nswa, swaty) = match swa {
            Some((n, crate::swa::SwaType::Standard)) => (n, 1u32),
            Some((n, crate::swa::SwaType::Chunked)) => (n, 2u32),
            _ => (0, 0),
        };
        if nswa > 0 && hd > 192 {
            eprintln!("wgpu-engine: sliding window NOT applied in prefill for head_dim={hd} (the serial \
fallback kernel is causal-only) — long prompts on this model will attend outside the window");
        }
        let dims = [s as u32, hd as u32, n_heads as u32, n_kv_heads as u32, base as u32, nswa, swaty, 0];
        // The parallel kernel now TILES the KV loop (online softmax over 2048-row tiles at 8 KB shared
        // memory), so it handles ANY causal length at high occupancy — no length cliff. Its per-thread
        // register accumulator holds ceil(hd/64) head-dim cols, so it's valid for hd<=192 (head_dim=128
        // → 2 cols). For the rare hd>192 model, fall back to the serial kernel (correct, slow).
        if hd <= 192 {
            self.dispatch(ATTENTION_MH_EXT_PAR, &[q, kc, vc], &out, bytemuck::cast_slice(&dims), ((s * n_heads) as u32, 1, 1));
        } else {
            self.dispatch(ATTENTION_MH_EXT, &[q, kc, vc], &out, bytemuck::cast_slice(&dims), (((s * n_heads) as u32).div_ceil(64), 1, 1));
        }
        out
    }
    /// Multi-head/GQA decode attention: single query row over a cache of `l` rows.
    /// Test hook: the same decode attention, callable from a gate.
    pub fn decode_attention_mh_pub(&self, q: &Tensor, kc: &Tensor, vc: &Tensor, l: usize, dm: usize,
                                   n_heads: usize, n_kv_heads: usize) -> Tensor {
        self.decode_attention_mh(q, kc, vc, l, dm, n_heads, n_kv_heads)
    }

    /// Upload a flat f32 slice as a [1, n] tensor — for weights whose 2-D shape is carried in MlaDims
    /// rather than in the tensor itself.
    pub fn upload_flat(&self, v: &[f32]) -> Tensor { self.upload(v, 1, v.len()) }

    /// Largest single `write_buffer` that stays on wgpu's fast staging path.
    ///
    /// MEASURED on Apple M2 Ultra / Honeykrisp, and the curve is backwards from what you would expect:
    ///     16 MiB  21.8 GiB/s
    ///     64 MiB   6.2 GiB/s
    ///    256 MiB   6.0 GiB/s
    /// Bigger transfers are ~3.5x SLOWER, because a write that does not fit the staging belt falls off
    /// to a slower path. Every large upload here — expert fills at ~68 MiB, arena slabs up to 2 GiB —
    /// was landing in the slow band.
    ///
    /// CHUNKING RECOVERS ABOUT HALF OF IT, not all, which is not what I predicted:
    ///     64 MiB chunked  10.3 GiB/s   (vs 6.2 unchunked, vs 21.8 for a lone 16 MiB write)
    ///    256 MiB chunked   9.8 GiB/s
    /// So four 16 MiB writes in one frame are not equivalent to one 16 MiB write. The belt is a fixed
    /// budget per submission, and chunks queued together still exhaust it — just less abruptly than one
    /// oversized write does. `write_chunked_flushed` tests whether closing the frame between chunks
    /// recovers the rest; until that is measured on hardware, 1.6x is what this is worth and the claim
    /// stops there.
    pub const UPLOAD_CHUNK: usize = 16 << 20;

    /// As `write_chunked`, closing the frame between chunks so the staging belt can recycle.
    ///
    /// Costs a submit per chunk, which is only worth it if belt exhaustion is really the limit — hence a
    /// separate function to be measured against the plain one rather than a change to it. If the numbers
    /// come back equal, the ceiling is elsewhere and this should be deleted rather than kept "just in
    /// case".
    pub fn write_chunked_flushed(&self, buf: &wgpu::Buffer, offset: u64, bytes: &[u8]) {
        let mut off = 0usize;
        while off < bytes.len() {
            let end = (off + Self::UPLOAD_CHUNK).min(bytes.len());
            self.queue.write_buffer(buf, offset + off as u64, &bytes[off..end]);
            self.end_frame();   // submit so the belt is released before the next chunk is staged
            off = end;
        }
    }

    /// Test hook for the flushed variant.
    pub fn write_chunked_flushed_pub(&self, dst: &Tensor, src: &[f32]) {
        self.write_chunked_flushed(&dst.buf, 0, bytemuck::cast_slice(src));
    }

    /// `write_buffer`, split so each piece stays on the fast path.
    ///
    /// Semantically identical to one large write: `write_buffer` writes are applied in submission order,
    /// so N sequential chunks land exactly as one call would. What changes is only which internal path
    /// wgpu takes.
    pub fn write_chunked(&self, buf: &wgpu::Buffer, offset: u64, bytes: &[u8]) {
        if bytes.len() <= Self::UPLOAD_CHUNK {
            self.queue.write_buffer(buf, offset, bytes);
            return;
        }
        let mut off = 0usize;
        while off < bytes.len() {
            let end = (off + Self::UPLOAD_CHUNK).min(bytes.len());
            self.queue.write_buffer(buf, offset + off as u64, &bytes[off..end]);
            off = end;
        }
    }

    /// Test hook: raw host->device write, for measuring upload bandwidth.
    pub fn queue_write_pub(&self, dst: &Tensor, src: &[f32]) {
        self.queue.write_buffer(&dst.buf, 0, bytemuck::cast_slice(src));
    }

    /// Dequantize raw Q6_K bytes to f32 on the GPU.
    ///
    /// 210-byte super-block of 256 weights: 128 low-nibble bytes, 64 high-2-bit bytes, 16 SIGNED int8
    /// sub-block scales, then an f16 `d`. A quant is `nib | (hi2 << 4)`, recentred by -32.
    ///
    /// Exists so Scout's experts can be FILE VIEWS: its `ffn_down_exps` is Q6_K, and one
    /// non-convertible tensor forces the whole model onto the host-copy path — 200 GiB of traffic per
    /// generation plus the staging fence that costs most of its submits.
    ///
    /// ## WARNING: GREEN ON LAVAPIPE, RED ON APPLE HARDWARE
    ///
    /// Everything below was verified on a software rasterizer. On an M2 Max it fails:
    ///
    /// ```text
    ///   hand-checked block:  PASSES (first 4 weights exact)
    ///   1 block:             rel 1.75    gpu non-zero: 64 of 256
    ///   first divergence at index 4, then a repeated constant
    /// ```
    ///
    /// ## The signature, now that the fixtures are clean (bundle _382)
    ///
    /// With plausible f16 scales the values are sane and the failure is unambiguous:
    ///
    /// ```text
    ///   cpu: [6.298, 0.899, 9.897, 12.147, 13.946,  8.547, 10.347, -9.447]
    ///   gpu: [6.298, 0.899, 9.897, 12.147, 14.396, 14.396, 14.396, 14.396]
    /// ```
    ///
    /// **Four outputs correct, then a constant.** Four is exactly one `u32` of source bytes. Output
    /// `l` reads `ql[l]`, so outputs 0..3 come from source word 0 and output 4 needs word 1 — and
    /// `byte_at` stops advancing there. The repeated value is that stale word re-decoded, which is why
    /// the results REPEAT rather than going to zero.
    ///
    /// `byte_at(i) = (src[i >> 2u] >> ((i & 3u) * 8u)) & 0xFFu`, and `src` is `array<u32>`. The
    /// arithmetic is correct WGSL, so the suspect is the BINDING: if the view exposes one word, every
    /// read past it clamps to the last valid index on this driver rather than returning zero. That is
    /// the _297 class — a binding range wrong in a way a reference rasterizer forgives — reappearing
    /// where only real hardware can see it.
    ///
    /// Note the negative-scale check fails at 1.008e3 while the others sit near 1e2, so it is a
    /// SEPARATE fault rather than the same one scaled.
    ///
    /// The block is 2 halves x 32 iterations x 4 writes = 256. EXACTLY 64 LAND — one write per
    /// iteration instead of four — and the values REPEAT rather than being zero, so the writes are
    /// colliding rather than being skipped.
    ///
    /// A hand-checked single block passes because it exercises only the first few indices. The
    /// cheapest check is the one that cannot see this.
    ///
    /// THE LESSON IS NOT ABOUT Q6_K. Lavapipe is a reference rasterizer — more permissive and more
    /// nearly correct than a real driver — so it validates arithmetic and hides every codegen and
    /// scheduling difference. Four faults were found and fixed against it, all four real, and the
    /// kernel is still wrong on the only hardware that matters. See docs/gate-validity.md.
    ///
    /// STATE: `--gate-q6k` was green ON LAVAPIPE — bit-exact against the CPU reference at 1, 37 and 5000 blocks,
    /// including the negative-scales case. Four faults, found in this order:
    ///
    ///   1. `d` at bytes 208..209 fell outside the binding. A 210-byte `array<u32>` exposes 52 whole
    ///      words = 208 bytes, and WGSL reads out of range as ZERO, so every weight was `0 * s * q`. It
    ///      produced NOTHING rather than something wrong, which is why the arithmetic looked correct.
    ///   2. `dispatch` ignored the OUTPUT's view, so chunks overwrote at offset 0. Fixed there, with the
    ///      bind-group cache key extended to match — a latent bug for every chunked dispatch, not just
    ///      this one.
    ///   3. `wg1` takes an ELEMENT count and divides by 64 itself; it was called pre-divided, so 65
    ///      blocks got ONE workgroup and block 64 onward was never dispatched. A size sweep found the
    ///      64->65 cliff immediately after two commits of reasoning about arithmetic that was fine.
    ///   4. The f16 SUBNORMAL path seeded the exponent at -1 and biased by 127-15, which is exactly the
    ///      bug `gguf::f16_to_f32`'s own comment documents as previously fixed there — "landing 2 too
    ///      low ... every subnormal came out 4x small". The wrong version was transcribed into the
    ///      shader FROM the comment explaining why it was wrong.
    ///
    /// The lesson worth keeping is (4): a reference implementation's comments describe bugs it no longer
    /// has, and copying code that sits next to such a comment reintroduces them. The other three were
    /// all size- or binding-related and invisible to a single-block test.
    pub fn q6k_dequant(&self, raw: &[u8], n_weights: usize) -> Result<Tensor, String> {
        if n_weights % 256 != 0 { return Err("q6_k: element count not a multiple of 256".into()); }
        let nblk = n_weights / 256;
        if raw.len() < nblk * 210 { return Err("q6_k: data truncated".into()); }
        // PAD THE BUFFER to whole words as well as the view. Rounding the view up without padding the
        // buffer asks for a range past its end — the view length is what the shader sees, but the
        // buffer length is what bounds the view.
        let src_bytes = (nblk * 210).div_ceil(4) * 4;
        let mut padded = raw[..nblk * 210].to_vec();
        padded.resize(src_bytes, 0);
        let srcbuf = self.upload_bytes(&padded, "q6k-src");
        let src = Tensor { buf: srcbuf, rows: 1, cols: src_bytes / 4, pool: None, view: None };
        let out = self.alloc_resident_words(n_weights);
        let chunk = engine_config().convert_chunk_blocks.max(1);
        let mut b0 = 0usize;
        while b0 < nblk {
            let n = chunk.min(nblk - b0);
            let byte_off = b0 * 210;
            // The VIEW length bounds the shader's `array<u32>`, so it must cover whole words. A
            // 210-byte block is 52.5 words; a 210-byte binding exposes 52, and the f16 scale at bytes
            // 208..209 sits in word 52 — out of bounds, which WGSL reads as ZERO rather than faulting.
            // Every weight then came out zero and the gate reported max|diff| 3.3e5.
            //
            // Padding the BUFFER was not enough: the binding range is what the shader sees.
            let vlen = ((n * 210).div_ceil(4) * 4).min(src_bytes - byte_off);
            let view = Tensor { buf: src.buf.clone(), rows: 1, cols: vlen / 4,
                                pool: None, view: Some((byte_off as u64, vlen as u64)) };
            let dstv = Tensor { buf: out.buf.clone(), rows: 1, cols: n * 256,
                                pool: None, view: Some(((b0 * 256 * 4) as u64, (n * 256 * 4) as u64)) };
            let dims = [n as u32, 0u32, 0u32, 0u32];
            // wg1 takes the ELEMENT count and divides by 64 itself. Passing an already-divided value
            // divides twice: 65 blocks asked for wg1(2) = ONE workgroup = 64 threads, so block 64 onward
            // was never dispatched and kept whatever the allocator left. That is the 64->65 cliff in the
            // size sweep — everything up to one workgroup looked perfect.
            self.dispatch(Q6K_DEQUANT, &[&view], &dstv, bytemuck::cast_slice(&dims), wg1(n as u32));
            b0 += n;
        }
        Ok(out)
    }

    /// Read `n` bytes back through the shader's own `byte_at`, to test the reader in isolation.
    ///
    /// Lives here rather than in the gate so `upload_bytes` and `dispatch` stay private — a diagnostic
    /// is not a reason to widen an API surface.
    /// As `byte_probe`, but the shader reads `array<vec4<u32>>`.
    ///
    /// Confirms or refutes the 16-byte-stride diagnosis in one run: if this reads correctly where the
    /// scalar probe does not, `array<u32>` is being laid out with std140 padding.
    /// `dst[i] = f32(i)` — tests the invocation index alone.
    pub fn id_probe(&self, n: usize) -> Vec<f32> {
        self.byte_probe_with(&vec![0u8; n], ID_PROBE)
    }

    pub fn byte_probe_vec4(&self, raw: &[u8]) -> Vec<f32> {
        self.byte_probe_with(raw, BYTE_PROBE_VEC4)
    }

    /// Native IQ1_S mat-vec: `c[n] = w[n,k] · a[k]`, reading the checkpoint's 50-byte super-blocks.
    ///
    /// Uploads the 2048-entry ternary codebook alongside the weights. 16 KiB, once per call here —
    /// a resident path would hold it for the model's life, since it is the same grid for every IQ1_S
    /// tensor and every layer.
    pub fn matvec_iq1_s(&self, a: &[f32], raw: &[u8], n: usize, k: usize) -> Vec<f32> {
        let at = self.upload(a, 1, k);
        let wt = self.upload_bytes(raw, "iq1s-weights");
        // The grid as u32 pairs: entry i is (low 4 signed bytes, high 4). Packed here rather than in
        // the shader because it is constant for the process and unpacking it per dispatch would be
        // 16 KiB of work per matvec.
        let mut grid = Vec::with_capacity(4096);
        for e in crate::iq1s_grid::IQ1S_GRID.iter() {
            grid.push((*e & 0xFFFF_FFFF) as u32);
            grid.push((*e >> 32) as u32);
        }
        let gt = self.upload_bytes(bytemuck::cast_slice(&grid), "iq1s-grid");
        let out = self.alloc(1, n);
        let gx = (n as u32).min(32768);
        let gy = (n as u32).div_ceil(gx);
        let dims = [1u32, n as u32, k as u32, (k / 256) as u32, gx, 0, 0, 0];
        let wten = Tensor { buf: wt, rows: 1, cols: raw.len().div_ceil(4), pool: None, view: None };
        let gten = Tensor { buf: gt, rows: 1, cols: grid.len(), pool: None, view: None };
        self.dispatch(MATVEC_IQ1_S, &[&at, &wten, &gten], &out,
                      bytemuck::cast_slice(&dims), (gx, gy, 1));
        self.download(&out)
    }

    pub fn byte_probe(&self, raw: &[u8]) -> Vec<f32> { self.byte_probe_with(raw, BYTE_PROBE) }

    /// `dst[i] = f32(src[i])` over `n` words — distinct index per thread.
    pub fn word_probe(&self, words: &[u32]) -> Vec<f32> {
        self.byte_probe_with(bytemuck::cast_slice(words), WORD_PROBE)
    }

    /// Upload bytes and read them straight back, with NO shader involved.
    ///
    /// The scalar and vec4 probes fail IDENTICALLY on hardware — only the first 4 bytes of every 16
    /// are readable either way. That rules out the element size, and it points at the buffer rather
    /// than the read: if only 4 of every 16 bytes ever arrived, both shaders are reading correctly and
    /// reporting what is there.
    ///
    /// This settles it. A round trip through `upload_bytes` and a copy back to the host touches no
    /// shader at all, so a mismatch is unambiguously the WRITE path — and a match means the upload is
    /// fine and the fault is somewhere between binding and execution after all.
    pub fn upload_readback_probe(&self, raw: &[u8]) -> Vec<u8> {
        let mut padded = raw.to_vec();
        padded.resize(raw.len().div_ceil(4) * 4, 0);
        let src = self.upload_bytes(&padded, "readback-probe");
        let t = Tensor { buf: src, rows: 1, cols: padded.len() / 4, pool: None, view: None };
        // download() reads a buffer back as f32 words; reinterpreting them as bytes recovers exactly
        // what was uploaded, because nothing has interpreted them in between.
        let words = self.download(&t);
        let mut out = Vec::with_capacity(words.len() * 4);
        for w in &words { out.extend_from_slice(&w.to_bits().to_le_bytes()); }
        out.truncate(raw.len());
        out
    }

    fn byte_probe_with(&self, raw: &[u8], shader: &str) -> Vec<f32> {
        let n = raw.len();
        let mut padded = raw.to_vec();
        padded.resize(n.div_ceil(4) * 4, 0);
        let src = self.upload_bytes(&padded, "byte-probe-src");
        let srct = Tensor { buf: src, rows: 1, cols: padded.len() / 4, pool: None, view: None };
        let out = self.alloc_resident_words(n);
        let dims = [n as u32, 0u32, 0u32, 0u32];
        self.dispatch(shader, &[&srct], &out, bytemuck::cast_slice(&dims),
                      ((n as u32).div_ceil(64), 1, 1));
        self.download(&out)
    }

    /// The device's per-buffer ceiling. No single allocation may exceed it, whatever the total budget.
    ///
    /// Honeykrisp reports ~2047 MiB (2^31 - 1), which is why the arena packs into slabs of that size and
    /// why a probe using larger ones measures the limit rather than the device.
    pub fn max_buffer_bytes(&self) -> usize { self.device.limits().max_buffer_size as usize }

    /// Test hook: chunked host->device write, to compare against the single-shot path.
    pub fn write_chunked_pub(&self, dst: &Tensor, src: &[f32]) {
        self.write_chunked(&dst.buf, 0, bytemuck::cast_slice(src));
    }

    /// Test hook: a small view of a tensor, to force a sync without downloading all of it.
    pub fn view_pub(&self, t: &Tensor, off: usize, len: usize) -> Tensor {
        Tensor { buf: t.buf.clone(), rows: 1, cols: len, pool: None,
                 view: Some(((off * 4) as u64, (len * 4) as u64)) }
    }

    /// Test hook: copy rows between tensors, for exercising allocator reuse from a gate.
    pub fn copy_rows_pub(&self, src: &Tensor, dst: &Tensor, dst_row: usize, n: usize, cols: usize) {
        self.write_rows(dst, src, dst_row, n, cols);
    }

    /// Test hook: a sub-range view of a tensor, for exercising windowed reads from a gate.
    pub fn tensor_view(&self, t: &Tensor, off: u64, size: u64, rows: usize, cols: usize) -> Tensor {
        Tensor { buf: t.buf.clone(), rows, cols, pool: None, view: Some((off, size)) }
    }

    fn decode_attention_mh(&self, q: &Tensor, kc: &Tensor, vc: &Tensor, l: usize, dm: usize, n_heads: usize, n_kv_heads: usize) -> Tensor {
        let out = self.alloc(1, dm); let hd = dm / n_heads;
        let dims = [l as u32, hd as u32, n_heads as u32, n_kv_heads as u32];
        // Rule-based by shape: the parallel kernel (one workgroup per head, KV rows split across
        // threads) uses the whole GPU and dominates at long context, but needs scores[l] in
        // shared memory (cap 4096). Above that: the flash split-K pair (parallelism grows with
        // context — heads × chunks workgroups — no shared-memory cap); LOGSCOPE_FLASH_ATTN=0
        // restores the old single-workgroup-per-head serial fallback.
        // DECODE_ATTN_MH_PAR is barrier-heavy; on Asahi (which hangs barrier/subgroup-heavy kernels)
        // it's the prime suspect for a decode hang. LOGSCOPE_DECODE_ATTN_SERIAL=1 forces the serial
        // kernel at ANY length so that hypothesis can be A/B-tested (and pins a safe path if true).
        // DECODE_ATTN_MH_PAR is barrier-heavy; if a prior GPU op faulted (degraded), or the diagnostic
        // flag is set, use the serial (barrier-free) kernel that can't wedge a fragile driver.
        let decode_serial = self.degraded() || std::env::var_os("LOGSCOPE_DECODE_ATTN_SERIAL").is_some();
        if decode_serial {
            self.dispatch(DECODE_ATTN_MH, &[q, kc, vc], &out, bytemuck::cast_slice(&dims), ((n_heads as u32).div_ceil(64), 1, 1));
        } else if l <= 4096 {
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

    /// Two-span decode attention: one online softmax over a WARM int8 K/V tail (`wl` rows) followed by a
    /// HOT f32 K/V window (`hl` rows) — the staged-KV read. Each span emits flash pass-1 partials into a
    /// shared buffer (hot chunks first, warm chunks after), then the existing pass-2 folds ALL chunks with
    /// the online-softmax rescale — so the result is identical to attention over the concatenated
    /// [warm | hot] sequence, regardless of the precision split. When `wl==0` this is exactly the f32
    /// flash path (the staged no-op gate reduces to it). Query/positions: the caller passes q for the
    /// current token; warm rows are the older positions [0,wl), hot rows the recent [wl, wl+hl).
    pub fn decode_attention_two_span(&self, q: &Tensor,
            kq: &Tensor, ks: &Tensor, vq: &Tensor, vs: &Tensor, wl: usize,      // warm int8 span
            kc: &Tensor, vc: &Tensor, hl: usize,                                 // hot f32 span
            dm: usize, n_heads: usize, n_kv_heads: usize) -> Tensor {
        let hd = dm / n_heads;
        let chunk = 2048usize;
        let hot_nch = hl.div_ceil(chunk).max(1);
        let warm_nch = if wl == 0 { 0 } else { wl.div_ceil(chunk) };
        let nch = hot_nch + warm_nch;
        let part = self.alloc(1, n_heads * nch * (hd + 2));
        // hot span → chunks [0, hot_nch). Reuse the f32 pass-1 unchanged (coff=0 implicit in its layout).
        let dh = [hl as u32, hd as u32, n_heads as u32, n_kv_heads as u32, chunk as u32, nch as u32, 0, 0];
        self.dispatch(DECODE_ATTN_FLASH_P1, &[q, kc, vc], &part, bytemuck::cast_slice(&dh), (n_heads as u32, hot_nch as u32, 1));
        // warm span → chunks [hot_nch, nch). The int8 pass-1 writes at chunk offset coff=hot_nch so both
        // spans' partials live in one buffer and pass-2 needs no changes.
        if warm_nch > 0 {
            let dw = [wl as u32, hd as u32, n_heads as u32, n_kv_heads as u32, chunk as u32, nch as u32, hot_nch as u32, 0];
            self.dispatch(DECODE_ATTN_FLASH_P1_Q8, &[q, kq, ks, vq, vs], &part, bytemuck::cast_slice(&dw), (n_heads as u32, warm_nch as u32, 1));
        }
        let out = self.alloc(1, dm);
        let dims2 = [hd as u32, nch as u32, n_heads as u32, 0];
        self.dispatch(DECODE_ATTN_FLASH_P2, &[&part], &out, bytemuck::cast_slice(&dims2), (n_heads as u32, 1, 1));
        out
    }

    /// Multi-block staged attention: the warm tier is a LIST of int8 blocks (one per demote event), each
    /// `(kq,ks,vq,vs, rows)`. Generalizes `decode_attention_two_span` to any number of warm blocks — each
    /// block contributes its own flash chunks at the right offset, and the single pass-2 folds hot + all
    /// warm chunks in one online softmax. This is the steady-state staged decode read (a long context has
    /// many demoted blocks). Warm blocks are the OLDER positions; hot is the recent window. `warm_blocks`
    /// is ordered oldest→newest but order doesn't affect the softmax (it's over the union).
    #[allow(clippy::too_many_arguments)]
    pub fn decode_attention_multi_span(&self, q: &Tensor,
            warm_blocks: &[(&Tensor, &Tensor, &Tensor, &Tensor, usize)], // (kq,ks,vq,vs,rows) per block
            kc: &Tensor, vc: &Tensor, hl: usize,
            dm: usize, n_heads: usize, n_kv_heads: usize) -> Tensor {
        let hd = dm / n_heads;
        let chunk = 2048usize;
        let hot_nch = hl.div_ceil(chunk).max(1);
        // each warm block contributes ceil(rows/chunk) chunks
        let warm_nchs: Vec<usize> = warm_blocks.iter().map(|&(_, _, _, _, r)| r.div_ceil(chunk)).collect();
        let warm_total: usize = warm_nchs.iter().sum();
        let nch = hot_nch + warm_total;
        let part = self.alloc(1, n_heads * nch * (hd + 2));
        // hot span → chunks [0, hot_nch)
        let dh = [hl as u32, hd as u32, n_heads as u32, n_kv_heads as u32, chunk as u32, nch as u32, 0, 0];
        self.dispatch(DECODE_ATTN_FLASH_P1, &[q, kc, vc], &part, bytemuck::cast_slice(&dh), (n_heads as u32, hot_nch as u32, 1));
        // each warm block → its own chunk range, starting after the hot chunks
        let mut coff = hot_nch;
        for (bi, &(kq, ks, vq, vs, rows)) in warm_blocks.iter().enumerate() {
            let bnch = warm_nchs[bi];
            if bnch == 0 { continue; }
            let dw = [rows as u32, hd as u32, n_heads as u32, n_kv_heads as u32, chunk as u32, nch as u32, coff as u32, 0];
            self.dispatch(DECODE_ATTN_FLASH_P1_Q8, &[q, kq, ks, vq, vs], &part, bytemuck::cast_slice(&dw), (n_heads as u32, bnch as u32, 1));
            coff += bnch;
        }
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
        let mut c = KvCache { k: vec![], v: vec![], mla: None };
        for li in 0..q.kq.len() {
            c.k.push(self.dequant_rows(&q.kq[li], &q.ks[li], len, kv_dim));
            c.v.push(self.dequant_rows(&q.vq[li], &q.vs[li], len, kv_dim));
        }
        c    }

    /// Create a staged KV cache. `max_seq` sizes the hot f32 buffers; `policy` owns the hot/warm split.
    /// With `StagePolicy::all_hot()` this is a plain f32 cache wearing a staged wrapper (the no-op gate).
    pub fn staged_new(&self, n_layers: usize, max_seq: usize, kv_dim: usize, policy: crate::kvstage::StagePolicy) -> StagedKvCache {
        StagedKvCache {
            hot: self.new_cache(n_layers, max_seq, kv_dim),
            warm: QKvCache { kq: vec![], ks: vec![], vq: vec![], vs: vec![] },
            policy,
            warm_len: 0, warm_block_rows: Vec::new(),
        }
    }

    /// Slide the staging boundary if the hot window has overflowed: demote the oldest hot block to the
    /// warm int8 tail and advance the boundary. Called by the decode loop AFTER writing the current token
    /// into the hot cache and advancing `pos`. A no-op when the policy says nothing's due (or all-hot), so
    /// short contexts and the no-op gate never touch the warm tier. Returns tokens demoted (0 = nothing).
    ///
    /// Real demotion (stage 4): for each layer, quantize the hot rows [from, from+len) to int8 and APPEND
    /// them to the per-layer warm stacks as one more block. The warm tier is stored as a list of int8
    /// blocks per layer (kq/ks/vq/vs each a Vec of block tensors); two-span attention reads them in order.
    /// Keeping blocks separate (vs one growing buffer) means a demote is a bounded quantize+push, no
    /// realloc/copy of the whole warm tail each time.
    pub fn staged_slide(&self, cache: &mut StagedKvCache, pos: usize, kv_dim: usize) -> usize {
        match cache.policy.plan(pos) {
            crate::kvstage::StageAction::Nothing => 0,
            crate::kvstage::StageAction::Demote { from, len } => {
                for li in 0..cache.hot.k.len() {
                    // view the block of hot rows [from, from+len) for this layer, quantize, append to warm
                    let kblk = self.rows_view(&cache.hot.k[li], from, len, kv_dim);
                    let vblk = self.rows_view(&cache.hot.v[li], from, len, kv_dim);
                    let (qk, sk) = self.quantize_rows(&kblk, len, kv_dim);
                    let (qv, sv) = self.quantize_rows(&vblk, len, kv_dim);
                    cache.warm.kq.push(qk); cache.warm.ks.push(sk);
                    cache.warm.vq.push(qv); cache.warm.vs.push(sv);
                }
                cache.policy.commit_demote(len);
                cache.warm_len += len; cache.warm_block_rows.push(len);
                len
            }
        }
    }

    /// int8-KV decode attention: single query row over `l` packed int8 rows + scales.
    fn decode_attention_mh_q8(&self, q: &Tensor, kq: &Tensor, ks: &Tensor, vq: &Tensor, vs: &Tensor, l: usize, dm: usize, n_heads: usize, n_kv_heads: usize) -> Tensor {
        self.decode_attention_mh_q8_from(q, kq, ks, vq, vs, 0, l, dm, n_heads, n_kv_heads)
    }

    /// As above, attending only to KV rows `start..l` — the sliding window for this query position.
    pub fn decode_attention_mh_q8_from(&self, q: &Tensor, kq: &Tensor, ks: &Tensor, vq: &Tensor, vs: &Tensor,
                                       start: usize, l: usize, dm: usize, n_heads: usize, n_kv_heads: usize) -> Tensor {
        let out = self.alloc(1, dm); let hd = dm / n_heads;
        let dims = [l as u32, hd as u32, n_heads as u32, n_kv_heads as u32, start as u32, 0, 0, 0];
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
        // Per-layer rope base (Gemma: windowed layers use rope.freq_base_swa), then the Llama-4 NoPE
        // sentinel — order matters, since a NoPE layer skips rotation regardless of which base it would
        // otherwise use.
        let base = w.rope_base.unwrap_or(base);
        let base = if w.nope { 0.0 } else { base }; // Llama4 iRoPE: NoPE layers skip rotation (rope hosts' identity sentinel)
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd;
        let n1 = self.rmsnorm_t(x, &w.g1, 1, d, eps);
        let q = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wq.apply(self, &n1, 1), &w.q_norm, 1, ad, hd, eps), ff, 1, ad, base, pos, hd), w, 1, ad, hd, eps);
        let kr = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wk.apply(self, &n1, 1), &w.k_norm, 1, kv, hd, eps), ff, 1, kv, base, pos, hd), w, 1, kv, hd, eps);
        let vr = w.wv.apply(self, &n1, 1);
        let (kqr, ksr) = self.quantize_rows(&kr, 1, kv);
        let (vqr, vsr) = self.quantize_rows(&vr, 1, kv);
        self.copy_row(kq, pos, &kqr, 0, kv / 4); self.copy_row(ks, pos, &ksr, 0, 1);
        self.copy_row(vq, pos, &vqr, 0, kv / 4); self.copy_row(vs, pos, &vsr, 0, 1);
        // sliding window as a loop bound (see decode_attention_mh_q8_from on why not a view here)
        let attn = self.decode_attention_mh_q8_from(&q, kq, ks, vq, vs, self.swa_start(w, pos), pos + 1, ad, n_heads, n_kv_heads);
        let x2 = w.wo.apply_add(self, &attn, 1, x);
        self.ffn_apply(w, &x2, 1, d, eps)
    }
    /// Fused-QKV counterpart of block_decode_q8 (int8 KV cache). Same slice-and-quantize: one wqkv
    /// matmul, q/k/v sliced from the packed output, then K and V quantized into the int8 cache exactly
    /// as before. Bit-exact to block_decode_q8 (the fusion never changes the values being quantized).
    fn block_decode_q8_fused(&self, w: &BlockWeights, x: &Tensor, ff: &Tensor, kq: &Tensor, ks: &Tensor, vq: &Tensor, vs: &Tensor, pos: usize, d: usize, head_dim: usize, n_heads: usize, n_kv_heads: usize, base: f32, eps: f32) -> Tensor {
        // Per-layer rope base (Gemma: windowed layers use rope.freq_base_swa), then the Llama-4 NoPE
        // sentinel — order matters, since a NoPE layer skips rotation regardless of which base it would
        // otherwise use.
        let base = w.rope_base.unwrap_or(base);
        let base = if w.nope { 0.0 } else { base }; // Llama4 iRoPE: NoPE layers skip rotation (rope hosts' identity sentinel)
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
        // sliding window as a loop bound (see decode_attention_mh_q8_from on why not a view here)
        let attn = self.decode_attention_mh_q8_from(&q, kq, ks, vq, vs, self.swa_start(w, pos), pos + 1, ad, n_heads, n_kv_heads);
        let x2 = w.wo.apply_add(self, &attn, 1, x);
        self.ffn_apply(w, &x2, 1, d, eps)
    }
    /// One token through all layers using the int8 KV cache; returns logits [vocab].
    pub fn decode_step_q8(&self, m: &Model, c: &QKvCache, token: u32, pos: usize) -> Vec<f32> {
        self.begin_frame();
        let s = token as usize * m.d;
        let mut x = self.embed_scaled(m, &m.embed[s..s + m.d]);
        let fused = fused_qkv_enabled();
        let tracing = route_trace_armed();
        if tracing { self.trace_tok.store(token as i64, std::sync::atomic::Ordering::Relaxed); }
        for li in 0..m.n_layers {
            if tracing { self.trace_layer.store(li as i32, std::sync::atomic::Ordering::Relaxed); }
            let l = &m.layers[li]; let (kq, ks, vq, vs) = (&c.kq[li], &c.ks[li], &c.vq[li], &c.vs[li]);
            x = if fused { self.block_decode_q8_fused(l, &x, &m.rope_factors, kq, ks, vq, vs, pos, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps) }
                else { self.block_decode_q8(l, &x, &m.rope_factors, kq, ks, vq, vs, pos, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps) };
        }
        if tracing { self.trace_layer.store(-1, std::sync::atomic::Ordering::Relaxed); }
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
            let mut x = self.embed_scaled(m, &m.embed[s..s + m.d]);
            for li in 0..m.n_layers { x = self.block_decode_q8_fused(&m.layers[li], &x, &m.rope_factors, &c.kq[li], &c.ks[li], &c.vq[li], &c.vs[li], pos, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps); }
            let xn = self.rmsnorm_t(&x, &m.out_norm, 1, m.d, m.eps);
            logits = self.download(&m.output.apply(self, &xn, 1));
        }
        logits
    }
    /// One block for a single token at absolute `pos`, attending to its KV cache.
    /// First KV row this layer may attend to for a query at `pos`. 0 when the layer is unwindowed.
    ///
    /// One definition used by BOTH decode paths, so the f32 view and the q8 loop bound cannot drift apart
    /// — two implementations of one window is how an off-by-one hides.
    pub fn swa_start(&self, w: &BlockWeights, pos: usize) -> usize {
        match w.swa {
            Some((n_swa, ty)) => crate::swa::window_start(n_swa, ty, pos as i64) as usize,
            None => 0,
        }
    }

    /// Restrict a KV cache to this layer's sliding window for a query at `pos`.
    ///
    /// A single query position sees a CONTIGUOUS window under both rules — the last `n_swa` positions
    /// (Gemma) or the current aligned chunk (Llama-4) — so decode needs a range restriction, not a mask.
    /// Prefill is different (every query row has its own window) and still needs the mask path.
    ///
    /// Returns None, meaning "attend to everything", when: the layer is not windowed; the window already
    /// covers the whole context; the cache is itself a view (composing offsets is not worth the risk); or
    /// the row stride would put the view off a 256-byte boundary, which a storage binding requires. The
    /// last case degrades to full attention rather than binding something misaligned.
    fn swa_view(&self, w: &BlockWeights, kc: &Tensor, vc: &Tensor, pos: usize, cols: usize)
        -> Option<(Tensor, Tensor, usize)> {
        w.swa?;
        if kc.view.is_some() || vc.view.is_some() { return None; }
        let start = self.swa_start(w, pos);
        if start == 0 { return None; }
        let row_bytes = cols * 4;
        let off = start * row_bytes;
        if off % 256 != 0 { return None; }
        let len = pos + 1 - start;
        let size = (len * row_bytes) as u64;
        Some((
            Tensor { buf: kc.buf.clone(), rows: len, cols, pool: None, view: Some((off as u64, size)) },
            Tensor { buf: vc.buf.clone(), rows: len, cols, pool: None, view: Some((off as u64, size)) },
            len,
        ))
    }

    /// MlaDims for a layer, from the model's MlaConfig. One conversion point, so the loader's metadata
    /// and the kernels' expectations cannot drift.
    pub fn mla_dims(&self, c: &MlaConfig, d: usize, n_heads: usize, base: f32, eps: f32) -> crate::mla::MlaDims {
        crate::mla::MlaDims {
            d, n_heads,
            q_lora: c.q_lora, kv_lora: c.kv_lora,
            qk_nope: c.qk_nope, qk_rope: c.qk_rope, v_head: c.v_head,
            base, eps,
        }
    }

    /// One decode step for a Multi-head Latent Attention layer.
    ///
    /// Structurally identical to `block_decode` — norm, attention, residual, FFN — but the attention half
    /// is MLA: the token is COMPRESSED into the cache (a [kv_lora] latent plus a shared [qk_rope] rotary
    /// key) and attention reconstructs per-head K/V from it, instead of caching K and V per head. That is
    /// the memory win: `MlaCache::compression_vs_standard` is ~7x on DeepSeek-V3 shapes.
    ///
    /// Every piece it calls — mla_compress_append and mla_attention_step_gpu, including the output
    /// projection — is already checked against the mla.rs CPU reference by the kernel-validation suite.
    /// This is the plumbing that was missing, not new math.
    pub fn block_decode_mla(&self, w: &BlockWeights, mw: &crate::mla::MlaWeightsT, dm: &crate::mla::MlaDims,
                            x: &Tensor, cache: &crate::mla::MlaCache, layer: usize, pos: usize,
                            d: usize, eps: f32) -> Tensor {
        let n1 = self.rmsnorm_t(x, &w.g1, 1, d, eps);
        // append THIS token's compressed row before attending, so the query sees itself — the same
        // ordering as the standard path's write_row before decode_attention.
        self.mla_compress_append(&n1, mw, dm, cache, layer, pos);
        let attn = self.mla_attention_step_gpu(&n1, mw, dm, &cache.latent[layer], &cache.rot[layer], pos);
        let x2 = self.add_t(x, &attn);
        self.ffn_apply(w, &x2, 1, d, eps)
    }

    /// Upload an embedding row, applying the architecture's embedding scale.
    fn embed_scaled(&self, m: &Model, row: &[f32]) -> Tensor {
        if (m.embed_scale - 1.0).abs() < f32::EPSILON { return self.upload(row, 1, m.d); }
        let scaled: Vec<f32> = row.iter().map(|v| v * m.embed_scale).collect();
        self.upload(&scaled, 1, m.d)
    }

    /// Apply the embedding scale to an already-resident gathered row.
    fn scale_embed_t(&self, m: &Model, x: Tensor) -> Tensor {
        if (m.embed_scale - 1.0).abs() < f32::EPSILON { return x; }
        // one row scaled by a constant: ROW_SCALE takes a per-row weight vector, so a 1-element weight
        // is exactly the scalar case and avoids a second kernel for one multiply
        let w = self.upload(&[m.embed_scale], 1, 1);
        let out = self.alloc(1, m.d);
        let dims = [1u32, m.d as u32, 0, 0];
        self.dispatch(ROW_SCALE, &[&x, &w], &out, bytemuck::cast_slice(&dims), wg1(m.d as u32));
        out
    }

    fn block_decode(&self, w: &BlockWeights, x: &Tensor, ff: &Tensor, kc: &Tensor, vc: &Tensor, pos: usize, d: usize, head_dim: usize, n_heads: usize, n_kv_heads: usize, base: f32, eps: f32) -> Tensor {
        // Per-layer rope base (Gemma: windowed layers use rope.freq_base_swa), then the Llama-4 NoPE
        // sentinel — order matters, since a NoPE layer skips rotation regardless of which base it would
        // otherwise use.
        let base = w.rope_base.unwrap_or(base);
        let base = if w.nope { 0.0 } else { base }; // Llama4 iRoPE: NoPE layers skip rotation (rope hosts' identity sentinel)
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd;
        let n1 = self.rmsnorm_t(x, &w.g1, 1, d, eps);
        let q = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wq.apply(self, &n1, 1), &w.q_norm, 1, ad, hd, eps), ff, 1, ad, base, pos, hd), w, 1, ad, hd, eps);
        // Llama 4 iRoPE: on NoPE layers past floor_scale, scale the query by the attention temperature so
        // long-context softmax stays sharp (the length-generalization mechanism). Skipped below the floor
        // (active_at == false) so short contexts pay nothing; RoPE layers carry attn_temp None.
        let q = match w.attn_temp {
            Some(t) if w.nope && t.active_at(pos) => self.scale_query_temp(&q, t.scale(pos), ad),
            _ => q,
        };
        let kr = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wk.apply(self, &n1, 1), &w.k_norm, 1, kv, hd, eps), ff, 1, kv, base, pos, hd), w, 1, kv, hd, eps);
        let vr = w.wv.apply(self, &n1, 1);
        self.write_row(kc, &kr, pos, kv);
        self.write_row(vc, &vr, pos, kv);
        // Sliding window, when this layer has one: attend to the window instead of the whole context.
        let attn = match self.swa_view(w, kc, vc, pos, kv) {
            Some((k2, v2, l2)) => self.decode_attention_mh(&q, &k2, &v2, l2, ad, n_heads, n_kv_heads),
            None => self.decode_attention_mh(&q, kc, vc, pos + 1, ad, n_heads, n_kv_heads),
        };
        // Gemma normalizes the attention OUTPUT before the residual add (post_attention_norm), which is
        // a different operation from the pre-attention norm already applied to the input.
        let x2 = match &w.post_attn {
            Some(pn) => {
                let o = w.wo.apply(self, &attn, 1);
                let on = self.rmsnorm_t(&o, pn, 1, d, eps);
                self.add_t(x, &on)
            }
            None => w.wo.apply_add(self, &attn, 1, x),
        };
        self.ffn_apply(w, &x2, 1, d, eps)
    }
    /// Staged-KV block decode: K/V for the current token go into the HOT f32 cache at its hot-relative
    /// row, and attention reads the WARM int8 tail + the HOT f32 window in one online-softmax pass
    /// (`decode_attention_two_span`). RoPE still uses the ABSOLUTE position `pos` (positions are global;
    /// only storage is split). `hot_start` is the warm/hot boundary; `warm_len == hot_start`. When
    /// `warm_len == 0` (nothing demoted yet — e.g. the HOT_WINDOW=∞ no-op gate) two-span reduces to the
    /// f32 flash path, so this is bit-exact to `block_decode` until demotion turns on.
    #[allow(dead_code, clippy::too_many_arguments)]
    fn block_decode_staged(&self, w: &BlockWeights, x: &Tensor, ff: &Tensor,
            kc: &Tensor, vc: &Tensor, warm: Option<(&Tensor, &Tensor, &Tensor, &Tensor)>, warm_len: usize,
            pos: usize, hot_start: usize, d: usize, head_dim: usize, n_heads: usize, n_kv_heads: usize, base: f32, eps: f32) -> Tensor {
        let base = if w.nope { 0.0 } else { base };
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd;
        let n1 = self.rmsnorm_t(x, &w.g1, 1, d, eps);
        let q = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wq.apply(self, &n1, 1), &w.q_norm, 1, ad, hd, eps), ff, 1, ad, base, pos, hd), w, 1, ad, hd, eps);
        let q = match w.attn_temp {
            Some(t) if w.nope && t.active_at(pos) => self.scale_query_temp(&q, t.scale(pos), ad),
            _ => q,
        };
        let kr = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wk.apply(self, &n1, 1), &w.k_norm, 1, kv, hd, eps), ff, 1, kv, base, pos, hd), w, 1, kv, hd, eps);
        let vr = w.wv.apply(self, &n1, 1);
        // hot-relative write position: the token at absolute `pos` is row `pos - hot_start` of the hot cache
        let hrow = pos - hot_start;
        self.write_row(kc, &kr, hrow, kv);
        self.write_row(vc, &vr, hrow, kv);
        let hot_len = hrow + 1; // hot rows [0, hrow] are live this step
        let attn = match warm {
            Some((kq, ks, vq, vs)) if warm_len > 0 =>
                self.decode_attention_two_span(&q, kq, ks, vq, vs, warm_len, kc, vc, hot_len, ad, n_heads, n_kv_heads),
            // no warm tail → identical to the normal decode attention over the hot cache
            _ => self.decode_attention_mh(&q, kc, vc, hot_len, ad, n_heads, n_kv_heads),
        };
        let x2 = w.wo.apply_add(self, &attn, 1, x);
        self.ffn_apply(w, &x2, 1, d, eps)
    }
    /// Staged block decode with a MULTI-BLOCK warm tail (steady state). Same as `block_decode_staged` but
    /// attention reads all warm int8 blocks + the hot window via `decode_attention_multi_span`. Reduces to
    /// the f32 path when there are no warm blocks (no-op gate still holds).
    #[allow(clippy::too_many_arguments)]
    fn block_decode_staged_multi(&self, w: &BlockWeights, x: &Tensor, ff: &Tensor,
            kc: &Tensor, vc: &Tensor, warm_blocks: &[(&Tensor, &Tensor, &Tensor, &Tensor, usize)], warm_len: usize,
            pos: usize, hot_start: usize, d: usize, head_dim: usize, n_heads: usize, n_kv_heads: usize, base: f32, eps: f32) -> Tensor {
        let base = if w.nope { 0.0 } else { base };
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd;
        let n1 = self.rmsnorm_t(x, &w.g1, 1, d, eps);
        let q = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wq.apply(self, &n1, 1), &w.q_norm, 1, ad, hd, eps), ff, 1, ad, base, pos, hd), w, 1, ad, hd, eps);
        let q = match w.attn_temp {
            Some(t) if w.nope && t.active_at(pos) => self.scale_query_temp(&q, t.scale(pos), ad),
            _ => q,
        };
        let kr = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wk.apply(self, &n1, 1), &w.k_norm, 1, kv, hd, eps), ff, 1, kv, base, pos, hd), w, 1, kv, hd, eps);
        let vr = w.wv.apply(self, &n1, 1);
        // Write at the ABSOLUTE position (like the normal cache), NOT hot-relative: demotion reads old
        // rows [from,from+len) by absolute index, so the hot cache must be absolutely-indexed too. The hot
        // WINDOW is then rows [hot_start, pos] — a sub-range of the absolutely-filled cache. (max_seq sized.)
        self.write_row(kc, &kr, pos, kv);
        self.write_row(vc, &vr, pos, kv);
        let hot_len = pos - hot_start + 1; // hot window rows [hot_start, pos]
        let attn = if warm_len > 0 && !warm_blocks.is_empty() {
            // hot span is the sub-range starting at hot_start; two-span reads it via a row-offset view
            let hot_k = self.rows_view(kc, hot_start, hot_len, kv);
            let hot_v = self.rows_view(vc, hot_start, hot_len, kv);
            self.decode_attention_multi_span(&q, warm_blocks, &hot_k, &hot_v, hot_len, ad, n_heads, n_kv_heads)
        } else {
            // no warm tail → attend over [0, pos] directly (hot_start==0 here), identical to normal decode
            self.decode_attention_mh(&q, kc, vc, pos + 1, ad, n_heads, n_kv_heads)
        };
        let x2 = w.wo.apply_add(self, &attn, 1, x);
        self.ffn_apply(w, &x2, 1, d, eps)
    }
    /// Multiply a single-row `[1, width]` query tensor by a scalar (Llama 4 attention temperature). Uses
    /// the row-broadcast scale kernel with a 1-row weight — one small dispatch, only invoked on NoPE
    /// layers past the temperature floor (long context), so it's off the hot path in the common case.
    pub fn scale_query_temp(&self, q: &Tensor, s: f32, width: usize) -> Tensor {
        let out = self.alloc(1, width);
        let wgt = self.upload(&[s], 1, 1);
        let dims = [1u32, width as u32, 0, 0];
        self.dispatch(ROW_SCALE, &[q, &wgt], &out, bytemuck::cast_slice(&dims), wg1(width as u32));
        out
    }
    /// Per-ROW query temperature for prefill NoPE layers: query row `r` (at absolute position `base+r`)
    /// is scaled by AttnTemp::scale(base+r). `rows` query rows of `width` cols; uses the row-broadcast
    /// ROW_SCALE with a per-row weight vector. Callers guard with `t.active_at(base+rows-1)` so this only
    /// runs when some row is past the floor (long prompts) — short prompts skip it and pay nothing.
    pub fn scale_query_temp_rows(&self, q: &Tensor, t: crate::llama4::AttnTemp, base: usize, rows: usize, width: usize) -> Tensor {
        let weights: Vec<f32> = (0..rows).map(|r| t.scale(base + r)).collect();
        let wgt = self.upload(&weights, rows, 1);
        let out = self.alloc(rows, width);
        let dims = [rows as u32, width as u32, 0, 0];
        self.dispatch(ROW_SCALE, &[q, &wgt], &out, bytemuck::cast_slice(&dims), wg1((rows * width) as u32));
        out
    }
    /// Extract columns [col_off, col_off+width) of a single-row [1, N] tensor as a fresh [1, width],
    /// via a buffer copy (a transfer, NOT a compute dispatch). Splits the fused QKV projection's
    /// [1, d+2kv] output into q/k/v slices for the decode path. (t=1 only — the row is contiguous.)
    fn slice_cols(&self, src: &Tensor, col_off: usize, width: usize) -> Tensor {
        let out = self.alloc(1, width);
        self.copy_buf(src, (col_off * 4) as u64, &out, 0, (width * 4) as u64);
        out
    }
    /// Copy a contiguous range of `len` rows starting at `row0` from an f32 `[rows, cols]` tensor into a
    /// fresh `[len, cols]` tensor. Used by staged-KV demotion to isolate the hot block being quantized.
    /// A real copy (like slice_cols) rather than a view, so the quantize kernel reads a clean dense buffer.
    fn rows_view(&self, src: &Tensor, row0: usize, len: usize, cols: usize) -> Tensor {
        let out = self.alloc(len, cols);
        self.copy_buf(src, (row0 * cols * 4) as u64, &out, 0, (len * cols * 4) as u64);
        out
    }
    /// block_decode with the three Q/K/V projections fused into one wqkv matmul (one weight-bind, one
    /// dispatch instead of three). The packed [1, d+2kv] output is split by buffer copies — q and k are
    /// rope'd together in ONE dispatch (they're contiguous and the q|k split lands on a head boundary,
    /// so it's bit-exact to roping them separately), v is copied straight into the V-cache row.
    /// Bit-exact to block_decode (--gate-qkv); saves 2 projection + 1 RoPE dispatch/layer.
    fn block_decode_fused(&self, w: &BlockWeights, x: &Tensor, ff: &Tensor, kc: &Tensor, vc: &Tensor, pos: usize, d: usize, head_dim: usize, n_heads: usize, n_kv_heads: usize, base: f32, eps: f32) -> Tensor {
        // Per-layer rope base (Gemma: windowed layers use rope.freq_base_swa), then the Llama-4 NoPE
        // sentinel — order matters, since a NoPE layer skips rotation regardless of which base it would
        // otherwise use.
        let base = w.rope_base.unwrap_or(base);
        let base = if w.nope { 0.0 } else { base }; // Llama4 iRoPE: NoPE layers skip rotation (rope hosts' identity sentinel)
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd;
        // Fused norm→QKV: RMSNorm(x) folds into the wqkv matvec (one dispatch instead of two).
        let qkv = w.wqkv.norm_apply(self, x, &w.g1, d, eps, 1);            // [1, ad+2kv]
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
        // Llama 4 iRoPE attention temperature on NoPE layers past the floor (see block_decode).
        let q = match w.attn_temp {
            Some(t) if w.nope && t.active_at(pos) => self.scale_query_temp(&q, t.scale(pos), ad),
            _ => q,
        };
        // Sliding window, when this layer has one: attend to the window instead of the whole context.
        let attn = match self.swa_view(w, kc, vc, pos, kv) {
            Some((k2, v2, l2)) => self.decode_attention_mh(&q, &k2, &v2, l2, ad, n_heads, n_kv_heads),
            None => self.decode_attention_mh(&q, kc, vc, pos + 1, ad, n_heads, n_kv_heads),
        };
        let x2 = w.wo.apply_add(self, &attn, 1, x);
        self.ffn_apply(w, &x2, 1, d, eps)
    }
    /// Per-row-position RoPE: each row uses its own absolute position from `pos`.
    pub fn rope_mh_rows(&self, x: &Tensor, ff: &Tensor, pos: &Tensor, t: usize, dm: usize, base: f32, head_dim: usize) -> Tensor {
        let out = self.alloc(t, dm);
        if base == 0.0 { self.copy_buf(x, 0, &out, 0, ((x.rows * x.cols).min(t * dm) * 4) as u64); return out; } // NoPE sentinel
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
        // Per-layer rope base (Gemma: windowed layers use rope.freq_base_swa), then the Llama-4 NoPE
        // sentinel — order matters, since a NoPE layer skips rotation regardless of which base it would
        // otherwise use.
        let base = w.rope_base.unwrap_or(base);
        let base = if w.nope { 0.0 } else { base }; // Llama4 iRoPE: NoPE layers skip rotation (rope hosts' identity sentinel)
        let hd = head_dim; let kv = n_kv_heads * hd; let ad = n_heads * hd;
        let n1 = self.rmsnorm_t(x, &w.g1, 1, d, eps);
        let q = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wq.apply(self, &n1, 1), &w.q_norm, 1, ad, hd, eps), ff, 1, ad, base, rope_pos, hd), w, 1, ad, hd, eps);
        let kr = self.kq_post(self.rope_mh_t(&self.qk_norm(w.wk.apply(self, &n1, 1), &w.k_norm, 1, kv, hd, eps), ff, 1, kv, base, rope_pos, hd), w, 1, kv, hd, eps);
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
        // Per-layer rope base (Gemma: windowed layers use rope.freq_base_swa), then the Llama-4 NoPE
        // sentinel — order matters, since a NoPE layer skips rotation regardless of which base it would
        // otherwise use.
        let base = w.rope_base.unwrap_or(base);
        let base = if w.nope { 0.0 } else { base }; // Llama4 iRoPE: NoPE layers skip rotation (rope hosts' identity sentinel)
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
        let mut x = self.embed_scaled(m, &m.embed[s..s + m.d]);
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
        let mut x = self.embed_scaled(m, &m.embed[s..s + m.d]);
        let fused = fused_qkv_enabled();
        let tracing = route_trace_armed();
        if tracing { self.trace_tok.store(token as i64, std::sync::atomic::Ordering::Relaxed); }
        for li in 0..m.n_layers {
            if tracing { self.trace_layer.store(li as i32, std::sync::atomic::Ordering::Relaxed); }
            let (w, k, v) = (&m.layers[li], &c.k[li], &c.v[li]);
            // Per-LAYER routing, not per-model: a checkpoint may mix MLA and standard attention, and a
            // layer takes the MLA path only when it has both the projections and a cache to write into.
            x = match (&w.mla, &c.mla, &m.mla) {
                (Some(mw), Some(mc), Some(cfg)) => {
                    let dm = self.mla_dims(cfg, m.d, m.n_heads, m.base, m.eps);
                    self.block_decode_mla(w, mw, &dm, &x, mc, li, pos, m.d, m.eps)
                }
                _ if fused => self.block_decode_fused(w, &x, &m.rope_factors, k, v, pos, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps),
                _ => self.block_decode(w, &x, &m.rope_factors, k, v, pos, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps),
            };
        }
        if tracing { self.trace_layer.store(-1, std::sync::atomic::Ordering::Relaxed); }
        let xn = self.rmsnorm_t(&x, &m.out_norm, 1, m.d, m.eps);
        self.download(&m.output.apply(self, &xn, 1))
    }

    /// Staged-KV decode of one token: writes K/V into the hot f32 window and reads warm int8 tail + hot
    /// window via two-span attention. Bit-exact to `decode_step` while the warm tail is empty (all-hot),
    /// which is the no-op gate. The caller runs `staged_slide` after this to demote a block when due.
    pub fn decode_step_staged(&self, m: &Model, sc: &StagedKvCache, token: u32, pos: usize) -> Vec<f32> {
        self.begin_frame();
        let s = token as usize * m.d;
        let x = self.upload(&m.embed[s..s + m.d], 1, m.d);
        let out = self.decode_layers_staged(m, sc, x, pos);
        self.download(&out)
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
            let rb = if w.nope { 0.0 } else { m.base }; // Llama4 iRoPE NoPE switch (rows path)
            let qb = self.rope_mh_rows(&w.wq.apply(self, &n1, n), &m.rope_factors, &pos, n, ad, rb, hd);
            let kb = self.rope_mh_rows(&w.wk.apply(self, &n1, n), &m.rope_factors, &pos, n, kv, rb, hd);
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
            let rb = if w.nope { 0.0 } else { m.base }; // Llama4 iRoPE NoPE switch (rows path)
            let qb = self.rope_mh_rows(&w.wq.apply(self, &n1, n), &m.rope_factors, &pos, n, ad, rb, hd);
            let kb = self.rope_mh_rows(&w.wk.apply(self, &n1, n), &m.rope_factors, &pos, n, kv, rb, hd);
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
    fn decode_layers_staged(&self, m: &Model, sc: &StagedKvCache, mut x: Tensor, pos: usize) -> Tensor {
        let hot_start = sc.policy.hot_start;
        let warm_len = sc.warm_len;
        let nblocks = sc.warm_block_rows.len();
        for li in 0..m.n_layers {
            let (w, k, v) = (&m.layers[li], &sc.hot.k[li], &sc.hot.v[li]);
            // gather this layer's warm blocks: block bi, layer li is at index (bi*n_layers + li)
            let warm_blocks: Vec<(&Tensor, &Tensor, &Tensor, &Tensor, usize)> = (0..nblocks)
                .map(|bi| {
                    let idx = bi * m.n_layers + li;
                    (&sc.warm.kq[idx], &sc.warm.ks[idx], &sc.warm.vq[idx], &sc.warm.vs[idx], sc.warm_block_rows[bi])
                }).collect();
            x = self.block_decode_staged_multi(w, &x, &m.rope_factors, k, v, &warm_blocks, warm_len,
                pos, hot_start, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps);
        }
        let xn = self.rmsnorm_t(&x, &m.out_norm, 1, m.d, m.eps);
        m.output.apply(self, &xn, 1)
    }

    fn decode_layers_t(&self, m: &Model, c: &KvCache, mut x: Tensor, pos: usize) -> Tensor {
        // Optional scratch-bounding for the memory-tight path (q4-30B OOMs on big prompts). DEFAULT OFF:
        // the confirmed-good decode path is unchanged unless LOGSCOPE_DECODE_FLUSH_LAYERS is set > 0.
        //
        // The earlier attempt (reverted in _111) corrupted output because it end_frame()'d mid-loop and let
        // the next layer's alloc recycle pool buffers whose async GPU work was STILL RUNNING (no fence), and
        // left the frame closed so later layers submitted unordered. The correct sequence flushes THEN WAITS
        // for completion before anything recycles, then re-opens the frame so ordering is preserved:
        //   end_frame (submit) → poll(wait) → clear_retained → begin_frame
        // After the wait, `x` (held live here) contains valid data and the dropped per-layer scratch buffers
        // are safe to return to the pool — no race. Cost: one full GPU sync per `flush` layers (only on this
        // opt-in path), trading throughput for a bounded live-buffer set that fits the VMA budget.
        // Still OPT-IN + hardware-untested for the windowed-frame interaction — enable to test the OOM case.
        let flush = std::env::var("LOGSCOPE_DECODE_FLUSH_LAYERS").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
        let fused = fused_qkv_enabled();
        let tracing = route_trace_armed();
        for li in 0..m.n_layers {
            if tracing { self.trace_layer.store(li as i32, std::sync::atomic::Ordering::Relaxed); }
            let (w, k, v) = (&m.layers[li], &c.k[li], &c.v[li]);
            // Per-LAYER routing, not per-model: a checkpoint may mix MLA and standard attention, and a
            // layer takes the MLA path only when it has both the projections and a cache to write into.
            x = match (&w.mla, &c.mla, &m.mla) {
                (Some(mw), Some(mc), Some(cfg)) => {
                    let dm = self.mla_dims(cfg, m.d, m.n_heads, m.base, m.eps);
                    self.block_decode_mla(w, mw, &dm, &x, mc, li, pos, m.d, m.eps)
                }
                _ if fused => self.block_decode_fused(w, &x, &m.rope_factors, k, v, pos, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps),
                _ => self.block_decode(w, &x, &m.rope_factors, k, v, pos, m.d, m.head_dim, m.n_heads, m.n_kv_heads, m.base, m.eps),
            };
            if flush != 0 && (li + 1) % flush == 0 && li + 1 < m.n_layers {
                // FENCED flush: submit, WAIT for completion (so x is valid + scratch is done), release
                // retained bind-groups, re-open the frame. The wait is what makes recycling safe.
                self.end_frame();
                crate::counted_poll(&self.device);
                self.clear_retained();
                self.begin_frame();
            }
        }
        if tracing { self.trace_layer.store(-1, std::sync::atomic::Ordering::Relaxed); }
        let xn = self.rmsnorm_t(&x, &m.out_norm, 1, m.d, m.eps);
        m.output.apply(self, &xn, 1)
    }
    /// Greedy generation that keeps the token id and input embedding ON THE GPU across steps: argmax
    /// and embedding gather are kernels, so each step reads back only the 4-byte id instead of [vocab]
    /// logits, and the next input is gathered on-device. Bit-exact to generate_kv (--gate-gpusample);
    /// the prerequisite for windowed (K-steps-per-submit) decode.
    pub fn generate_kv_gpu(&self, m: &Model, prompt: &[u32], n_new: usize) -> Vec<u32> {
        self.clear_oom(); // fresh generation: a prior step's OOM must not poison this run's downloads
        let embed_t = self.resident_embed(m);
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
            let idv = self.download_u32(&id_t);              // flush + 4-byte readback (not [vocab])
            let id = match idv.first() { Some(&id) => id, None => break }; // empty ⇒ OOM backoff: stop cleanly
            out.push(id);
            if step + 1 == n_new { break; }
            self.begin_frame();
            let x = self.scale_embed_t(m, embed_t.gather_row(self, &id_t, 0, m.d));  // gathered on-device
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
        let embed_t = self.resident_embed(m);
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
                    let x = self.scale_embed_t(m, embed_t.gather_row(self, &ids_buf, j as u32, m.d));
                    logits_t = self.decode_layers_t(m, &c, x, l + step + j);
                }
            }
            let win = self.download_u32(&ids_buf);                         // flush window; one readback for w tokens
            if win.len() < w { break; } // empty/short ⇒ OOM backoff: stop cleanly instead of panicking
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
        let embed_t = self.resident_embed(m);
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
                    let x = embed_t.gather_row(self, &o, j as u32, m.d);
                    logits_t = self.decode_layers_t(m, &c, x, l + step + j);
                }
            }
            let win = self.download_u32(&o);
            if win.len() < w { break; } // OOM backoff: stop cleanly
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

    /// Model-free speculative decode: draft up to `k` tokens by prompt-lookup (ngram::ngram_draft) on the
    /// running token history instead of a draft model, then verify the block in ONE target forward exactly
    /// like `speculative_generate`. Because the verify accepts only the target's own greedy tokens, the
    /// output is BIT-IDENTICAL to `generate_kv` — the n-gram draft only changes speed (fewer target
    /// forwards when it guesses right on repetitive/structured text). No second set of weights, no draft
    /// session. Returns the `n_new` generated tokens.
    pub fn speculative_generate_ngram(&self, target: &Model, prompt: &[u32], n_new: usize, k: usize) -> Vec<u32> {
        let mut st = SpecStats::default();
        self.speculative_generate_ngram_stats(target, prompt, n_new, k, &mut st)
    }

    /// As `speculative_generate_ngram`, recording acceptance stats.
    pub fn speculative_generate_ngram_stats(&self, target: &Model, prompt: &[u32], n_new: usize, k: usize, st: &mut SpecStats) -> Vec<u32> {
        let k = k.max(1);
        let max_seq = prompt.len() + n_new + 2 * k + 8;
        let tc = self.new_cache(target.n_layers, max_seq, target.kv_dim());
        let mut prev = Vec::new();
        for pos in 0..prompt.len() { prev = self.decode_step(target, &tc, prompt[pos], pos); }
        let mut pos = prompt.len();
        // running token history the draft looks up (prompt + everything emitted so far)
        let mut hist: Vec<u32> = prompt.to_vec();
        let mut gen: Vec<u32> = Vec::new();
        while gen.len() < n_new {
            // draft from the history via prompt-lookup (may be empty → plain target step)
            let draft_toks = crate::ngram::ngram_draft(&hist, k);
            st.rounds += 1;
            if draft_toks.is_empty() {
                let t = argmax(&prev); gen.push(t); hist.push(t); st.emitted += 1;
                prev = self.decode_step(target, &tc, t, pos); pos += 1;
                continue;
            }
            // verify the whole drafted block in one target forward (identical to the model-draft path)
            let vlog = self.verify_forward(target, &tc, &draft_toks, pos);
            let kk = draft_toks.len();
            let mut cur = prev.clone();
            let mut accepted = 0usize; let mut emit: Vec<u32> = Vec::new(); let mut mism = false;
            for j in 0..kk {
                let tt = argmax(&cur); emit.push(tt);
                if draft_toks[j] == tt { accepted += 1; cur = vlog[j * target.vocab..(j + 1) * target.vocab].to_vec(); }
                else { mism = true; break; }
            }
            if !mism { emit.push(argmax(&cur)); } // all accepted → free bonus token
            st.accepted += accepted;
            let correction = *emit.last().unwrap();
            prev = self.decode_step(target, &tc, correction, pos + accepted);
            pos += accepted + 1;
            for t in emit { if gen.len() >= n_new { break; } gen.push(t); hist.push(t); st.emitted += 1; }
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
            let mut x = self.embed_scaled(m, &m.embed[s..s + m.d]);
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
        // GOVERNOR: derive a device-safe window from the live buffer-object count instead of a hardcoded
        // cap. The governor knows this backend's ceiling shape (Metal buffer-objects / Vulkan allocation-
        // count) and self-tunes: a clean window nudges its cap up toward the request; any pressure signal
        // ratchets it down and latches. LOGSCOPE_WINDOW_MAX still hard-caps on top for manual control.
        let live_objs = self.dims_buffer_count() + self.pool_live_estimate();
        // LOGSCOPE_WINDOW_PIN=1 makes the requested window an exact value (governor won't probe up);
        // otherwise the request is a hint and the governor autotunes toward the device's real ceiling.
        let pinned = std::env::var("LOGSCOPE_WINDOW_PIN").ok().as_deref() == Some("1");
        let gov_k = self.governor.lock().unwrap().window_for(k, live_objs, pinned);
        let hard_cap = std::env::var("LOGSCOPE_WINDOW_MAX").ok().and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(usize::MAX).max(2);
        let k_req = k;
        let k = k.max(2).min(gov_k).min(hard_cap);
        // Surface the governor's decision once per session when it actually trims the request, so a
        // clamped window is visible (not a silent slowdown) — the "alert" half of the governor.
        {
            static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if k < k_req && !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                let g = self.governor.lock().unwrap();
                let b = g.budget();
                eprintln!("wgpu-engine: governor — requested window k={k_req} clamped to k={k} ({:?} budget: soft cap {} objects, ~{} live); it will self-tune upward if the device has headroom",
                    b.kind, b.soft_object_cap, live_objs);
            }
        }
        let rcap = 64usize;
        if s.logits.is_empty() { return 0; }
        let cache_rows = s.cache.k.first().map(|t| t.rows).unwrap_or(0);
        // The window's whole point is ONE submit per k tokens (on Metal, encoder setup — not GPU
        // sync — is the wall: ~36 dispatches/token, each a separate submit if flush fires mid-window).
        // A window is w·(dispatches/token) passes; pin flush above that ceiling so `begin_frame`
        // batches the entire window into one encoder, then restore the caller's flush. Env-pinned
        // flush (LOGSCOPE_FLUSH_EVERY) is respected as a floor — the user set it deliberately.
        let saved_flush = self.flush_every();
        if !self.flush_every_pinned() {
            let per_tok = 64u32; // generous upper bound on dispatches/token; keeps a full window in one frame
            // On the conservative (Asahi/Honeykrisp) path, completion callbacks are unreliable — the submit
            // governor disables itself, leaving NO backpressure on queue depth. Batching a whole window into
            // ONE unflushed submit then hands the firmware one huge job the watchdog can kill (a decode hang
            // with no single-kernel cause — serial-forcing the decode attention didn't clear it). So here we
            // DON'T collapse the window into one submit; we keep a bounded flush (split the work — the same
            // watchdog fix as prefill chunking) and accept a little per-token submit overhead. This is
            // proactive (not gated on governor_dead, which only latches AFTER the first stalled submit — too
            // late for the first window). LOGSCOPE_WINDOW_ONE_SUBMIT=1 restores the collapse for A/B.
            let force_collapse = std::env::var_os("LOGSCOPE_WINDOW_ONE_SUBMIT").is_some();
            // WATCHDOG-PRONE, not Apple. Mobile drivers kill long dispatches the same way Honeykrisp
            // does, and this guard bounds the decode window precisely to avoid that — so keying it on
            // an Apple-only flag left a phone taking the unbounded path.
            if (self.watchdog_prone() || self.degraded()) && !force_collapse {
                self.set_flush_every(saved_flush.clamp(1, 64));
            } else {
                self.set_flush_every((k as u32).saturating_mul(per_tok).clamp(saved_flush, 65536));
            }
        }
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
                // If a dims uniform allocation was rejected by the device (buffer-object/VMA ceiling —
                // the M4 windowed crash surfaces at the FIRST sample_at, slot 0), the frame is poisoned:
                // the next readback would map_async an invalid buffer and abort the process. Bail the
                // window HERE — every slot, not just at the flush cadence — before download_u32, so the
                // caller falls back to per-token decode (which doesn't hit the ceiling). Restore flush.
                // A DEVICE OOM IS PRESSURE TOO, and it was the only kind this hardware actually
                // reports. observe_pressure had exactly one trigger — dims_alloc_failed — while the
                // failure your log shows is "DEVICE OOM caught (VMA exhausted)", handled elsewhere and
                // never fed back. So the window climbed 8→10→12→15→18→22, hit VMA exhaustion, was
                // degraded for that step, and CLIMBED STRAIGHT BACK on the next one, because the
                // ceiling latch never learned.
                //
                // A ratchet whose only sensor is the failure mode this device does not produce is a
                // ratchet that does not ratchet.
                if self.oom_backoff() {
                    self.governor.lock().unwrap().observe_pressure(k);
                    self.clear_oom();
                }
                if self.dims_alloc_failed() {
                    self.end_frame();
                    crate::counted_poll(&self.device);
                    self.clear_retained();
                    if !self.flush_every_pinned() { self.set_flush_every(saved_flush); }
                    // Tell the governor this k caused real device pressure so it ratchets the cap down and
                    // latches — the next window (this or a later turn) won't climb back to a failing size.
                    self.governor.lock().unwrap().observe_pressure(k);
                    return usize::MAX; // sentinel: windowed decode aborted on device pressure
                }
                // Bound live buffers WITHIN the window. Pinning flush high batches the window into one
                // frame for throughput, but on Metal that lets a big k's per-slot dispatches (~36×k bind
                // groups + dims + decode intermediates) accumulate until the device runs out of buffer
                // OBJECTS. Every SLOT_FLUSH slots, flush the frame and poll so those buffers free; the
                // `pre` logits (needed for the replay) are held by us, not the frame, so they survive.
                if (j + 1) % SLOT_FLUSH == 0 {
                    self.end_frame();
                    crate::counted_poll(&self.device);
                    self.clear_retained();
                    self.begin_frame();
                }
            }
            let win = self.download_u32(&o);
            if win.len() < w { break; } // OOM backoff: stop cleanly instead of indexing an empty readback
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
        if !self.flush_every_pinned() { self.set_flush_every(saved_flush); } // restore the caller's pacing
        // The window ran to completion without device pressure — let the governor nudge its safe-k cap
        // up toward the request (bounded, one step) so a device with headroom converges to a bigger,
        // faster window over a few turns instead of staying pinned at the conservative start.
        {
            let mut g = self.governor.lock().unwrap();
            let before = g.learned_cap();
            g.observe_clean(k);
            let after = g.learned_cap();
            if after > before {
                eprintln!("wgpu-engine: governor — window autotuned up {before}→{after} (device has headroom; clean run at k={k})");
            }
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
    // NaN/Inf guard (Colibri #369): one +Inf becomes mx → exp(NaN) → every prob NaN → sum NaN → the
    // draw silently collapses to token 0 forever. If any logit is non-finite, fall back to the
    // finite-aware argmax (a valid token) instead of sampling from a poisoned distribution.
    if logits.iter().any(|x| !x.is_finite()) { return argmax(logits); }
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
        // TIMED. Host-side sampling is the fourth thing a generation does and the last one
        // nothing measured — half a megabyte of logits walked per token at a 128k vocabulary,
        // appearing in neither setup, submit nor sync. It would have landed in the same
        // unaccounted bucket three commits have been spent emptying.
        crate::timed_sample(|| if sp.temperature > 0.0 { sample(&self.logits, sp, &mut self.rng) } else { argmax(&self.logits) } )
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
        // TIMED. Host-side sampling is the fourth thing a generation does and the last one
        // nothing measured — half a megabyte of logits walked per token at a 128k vocabulary,
        // appearing in neither setup, submit nor sync. It would have landed in the same
        // unaccounted bucket three commits have been spent emptying.
        crate::timed_sample(|| if sp.temperature > 0.0 { sample(&self.logits, sp, &mut self.rng) } else { argmax(&self.logits) } )
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
mod fast_ab_tests {
    use super::*;

    // Feed a stream of per-token times; the harness would set the phase before each. Here we just drive
    // observe() in order — phase() advances warmup → off-window → on-window → decided deterministically.
    fn run(ab: &mut FastAb, off_time: f32, on_time: f32) -> Option<bool> {
        let mut decision = None;
        for _ in 0..64 {
            if ab.decided().is_some() { break; }
            let ms = match ab.phase() {
                AbPhase::MeasuringOn => on_time,
                _ => off_time, // warmup + off-window both run at the OFF speed
            };
            if let Some(d) = ab.observe(ms) { decision = Some(d); }
        }
        decision
    }

    #[test]
    fn turns_on_when_levers_are_faster() {
        let mut ab = FastAb::new(3, 8, 0.05);
        // ON is 20% faster (16ms vs 20ms) → decide ON
        assert_eq!(run(&mut ab, 20.0, 16.0), Some(true));
        assert_eq!(ab.phase(), AbPhase::DecidedOn);
    }

    #[test]
    fn stays_off_when_no_real_gain() {
        let mut ab = FastAb::new(3, 8, 0.05);
        // ON only 2% faster (19.6 vs 20) — under the 5% margin → stay OFF (don't flip on noise)
        assert_eq!(run(&mut ab, 20.0, 19.6), Some(false));
        assert_eq!(ab.phase(), AbPhase::DecidedOff);
    }

    #[test]
    fn stays_off_when_levers_are_slower() {
        let mut ab = FastAb::new(3, 8, 0.05);
        assert_eq!(run(&mut ab, 20.0, 24.0), Some(false)); // ON slower → OFF
    }

    #[test]
    fn median_ignores_a_single_spike() {
        let mut ab = FastAb::new(0, 5, 0.05);
        // OFF window: one huge GC spike among fast tokens; median must ignore it
        for &t in &[20.0, 20.0, 200.0, 20.0, 20.0] { ab.observe(t); } // off median = 20
        // ON window: steadily faster
        let mut dec = None;
        for &t in &[16.0, 16.0, 16.0, 16.0, 16.0] { if let Some(d) = ab.observe(t) { dec = Some(d); } }
        assert_eq!(dec, Some(true), "spike in OFF must not prevent an honest ON win");
        let (off, on) = ab.medians();
        assert_eq!((off, on), (20.0, 16.0));
    }

    #[test]
    fn warmup_tokens_are_discarded() {
        let mut ab = FastAb::new(3, 4, 0.05);
        // first 3 tokens (warmup) are absurdly slow (pipeline compile) but must NOT count as OFF timing
        for _ in 0..3 { assert_eq!(ab.phase(), AbPhase::Warmup); ab.observe(999.0); }
        assert_eq!(ab.phase(), AbPhase::MeasuringOff);
        for _ in 0..4 { ab.observe(20.0); }
        assert_eq!(ab.phase(), AbPhase::MeasuringOn);
        let (off, _) = ab.medians();
        assert_eq!(off, 20.0, "warmup 999ms tokens excluded from the OFF median");
    }

    #[test]
    fn phase_to_atomic_bridge_sets_levers() {
        // ab_apply_phase drives the global the lever flags read (env must be unset for this to be meaningful)
        if std::env::var("LOGSCOPE_FAST_DECODE").is_err() {
            ab_apply_phase(AbPhase::MeasuringOn);
            assert!(dispatch_levers_active(), "measuring-ON phase turns levers on");
            ab_apply_phase(AbPhase::MeasuringOff);
            assert!(!dispatch_levers_active(), "measuring-OFF phase turns levers off");
            ab_set_decided(true);
            assert!(dispatch_levers_active() && ab_is_decided());
            ab_set_decided(false);
            assert!(!dispatch_levers_active() && ab_is_decided());
            FAST_AB_STATE.store(AB_UNMEASURED, std::sync::atomic::Ordering::Relaxed); // reset for other tests
        }
    }
}

#[cfg(test)]
mod decode_cfg_tests {
    use super::*;

    #[test]
    fn mla_config_matches_real_glm52_dims() {
        // Pinned from the real GLM-5.2 (glm-dsa) GGUF: kv_lora=512, q_lora=2048, qk_nope(=key_length_mla)=
        // 256, qk_rope(=rope.dimension_count)=64, v_head(=value_length_mla)=256. The cached latent is
        // kv_lora+qk_rope=576 (== the file's attention.key_length), and without MLA a 64-head model would
        // cache 64·(256+256)=32768 values/token → the ~57x reduction Colibri cites.
        let m = MlaConfig { q_lora: 2048, kv_lora: 512, qk_nope: 256, qk_rope: 64, v_head: 256 };
        assert_eq!(m.latent_width(), 576);
        assert_eq!(m.qk_head(), 320);
        let standard = 64 * (m.qk_nope + m.v_head); // per-head K(nope)+V across 64 heads
        assert!(standard / m.latent_width() >= 50, "MLA should be ~57x smaller than per-head KV here");
    }

    #[test]
    fn mla_config_latent_width_is_the_memory_win() {
        // DeepSeek-V3 dims: the cached latent is kv_lora + qk_rope = 512 + 64 = 576, vs a standard
        // GQA cache of n_kv_heads·head_dim. The whole point of MLA is that 576 << the per-head KV.
        let m = MlaConfig { q_lora: 1536, kv_lora: 512, qk_nope: 128, qk_rope: 64, v_head: 128 };
        assert_eq!(m.latent_width(), 576);
        assert_eq!(m.qk_head(), 192); // nope + rope, the score width
        // a standard 128-kv-head × 128-dim cache would be 16384/token — MLA caches 28× less here
        assert!(m.latent_width() * 28 < 128 * 128);
    }

    #[test]
    fn dsa_is_a_noop_below_threshold_then_sparse() {
        // The property that makes DSA safe to enable unconditionally: at or below index_topk it selects ALL
        // keys (dense, exact — no regression on short contexts); only past it does it go sparse.
        let d = DsaConfig { index_topk: 2048, index_n_heads: 4, index_head_dim: 128 };
        assert!(!d.sparse_active(100), "short context must be dense");
        assert!(!d.sparse_active(2048), "at the threshold it's still dense (<=, not <)");
        assert!(d.sparse_active(2049), "just past the threshold it goes sparse");
        assert_eq!(d.selected_keys(100), 100, "dense: attend all keys");
        assert_eq!(d.selected_keys(8192), 2048, "sparse: attend only index_topk keys");
        assert!(d.selected_keys(1_000_000) == 2048, "long context: bounded key count (sub-quadratic)");
    }

    #[test]
    fn argmax_skips_nonfinite_and_never_pins_to_zero() {
        // a leading NaN must NOT pin the pick to index 0 (the Colibri #369 failure)
        assert_eq!(argmax(&[f32::NAN, 1.0, 5.0, 2.0]), 2);
        // +Inf is non-finite → skipped; the finite max wins (we never emit an Inf "token")
        assert_eq!(argmax(&[1.0, f32::INFINITY, 3.0]), 2);
        // normal case unchanged
        assert_eq!(argmax(&[0.1, 0.9, 0.3]), 1);
        // all non-finite → falls back to 0 (a valid in-range index), not garbage
        assert_eq!(argmax(&[f32::NAN, f32::NAN]), 0);
    }

    #[test]
    fn sample_falls_back_to_argmax_on_poisoned_logits() {
        let sp = SamplerParams { temperature: 0.8, top_k: 40, top_p: 0.95, repeat_penalty: 1.0, repeat_last_n: 64, min_p: 0.0 };
        let mut rng = 12345u64;
        // one +Inf would poison the softmax → all-NaN probs → silent token-0 forever. Guard must return a
        // valid finite-argmax token instead.
        let poisoned = vec![0.2f32, f32::INFINITY, 0.5, 0.1];
        let t = sample(&poisoned, &sp, &mut rng);
        assert!((t as usize) < poisoned.len(), "must return an in-range token");
        // a NaN anywhere also triggers the fallback (deterministic argmax of the finite entries)
        let nanny = vec![1.0f32, f32::NAN, 9.0, 3.0];
        assert_eq!(sample(&nanny, &sp, &mut rng), 2);
    }

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

/// Gemma's RMS norm is `x/rms(x) · (1 + w)` where every other architecture uses `x/rms(x) · w`.
///
/// Rather than a second kernel, the `1 +` is folded into the WEIGHT at load: `(1+w) · x/rms(x)` is
/// exactly what the existing norm computes given `1+w`. One arithmetic change at load, no new shader, no
/// per-call branch in the hot path — and nothing to keep in sync between two implementations.
///
/// The `1 +` is not cosmetic: Gemma ships norm weights centred near ZERO, so omitting it scales every
/// normalized value by roughly w instead of 1+w, which is not a small error.
fn gemma_norm(w: &[f32], acfg: &crate::archcfg::ArchConfig) -> Vec<f32> {
    match acfg.norm {
        crate::archcfg::Norm::RmsGemma => w.iter().map(|v| 1.0 + v).collect(),
        crate::archcfg::Norm::Rms => w.to_vec(),
    }
}

/// Wrap a 1-D workgroup count into a 2-D grid under the per-dimension dispatch limit.
///
/// Every backend caps a dispatch dimension at 65535. A workgroup-per-super-block is natural for Q4_K but
/// overflows that on any real weight matrix — a 26M-weight tensor needs 102400 — and wgpu reports it as a
/// validation error the loader logs and continues past, leaving the output buffer UNWRITTEN. Silent
/// garbage weights, in other words, which is why this is a hard cap rather than a tuning knob.
pub fn wrapped_grid(units: usize) -> (u32, u32) {
    const W: usize = 32768; // comfortably under 65535, power of two
    if units <= W { return (units as u32, 1); }
    (W as u32, units.div_ceil(W) as u32)
}

/// Q4_K super-block dequantization. One workgroup per 144-byte super-block (256 weights).
///
/// Ported line-for-line from the CPU `gguf::dequantize` type-12 arm, which is validated by every Q4_K
/// model that loads today — so this shader has a reference to be checked against rather than a spec I
/// reconstructed. `--gate-q4k` asserts they agree exactly.
///
/// Bytes are read through a u32 view: super-block b starts at byte b*144, i.e. word b*36. The 6-bit
/// scale/min packing (`get_scale_min_k4`) is reproduced exactly, including the j>=4 branch that stitches
/// the high 2 bits from an earlier byte.

/// Q6_K -> f32 on the GPU, mirroring `gguf::dequantize` type 14 exactly.
///
/// 210-byte super-block of 256 weights: 128 low-nibble bytes `ql`, 64 high-2-bit bytes `qh`, 16 int8
/// sub-block scales, then an f16 `d`. A 6-bit quant is `nib | (hi2 << 4)`, recentred by -32, and the
/// weight is `d * sc * (q - 32)`. Processed in two 128-wide halves, same as the CPU path.
///
/// This exists so Scout's experts can be FILE VIEWS. Its `ffn_down_exps` is Q6_K, and the view path
/// requires every expert tensor to be GPU-convertible — one Q6_K tensor forced the whole model onto the
/// host-copy path, which is 200 GiB of traffic per generation and the staging fence that costs 99% of
/// submits. Converting Q6_K removes the copies rather than making them faster.
/// Extract a WGSL function body from a shader constant, normalised for comparison.
///
/// Used by the drift test below. Strips comments and collapses whitespace, so two copies that differ
/// only in commentary compare equal while a changed constant does not.
#[cfg(test)]
fn wgsl_fn_body(src: &str, sig: &str) -> Option<String> {
    let i = src.find(sig)?;
    let b = src[i..].find('{')? + i;
    let (mut depth, mut j) = (0i32, b);
    for c in src[b..].chars() {
        if c == '{' { depth += 1; } else if c == '}' { depth -= 1; if depth == 0 { break; } }
        j += c.len_utf8();
    }
    let body = &src[b..=j];
    let no_comments: String = body.lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>().join(" ");
    Some(no_comments.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// A shader that does NOTHING but read bytes back, so `byte_at` can be tested in isolation.
///
/// Five rounds of inference from dequantizer output have not settled whether `byte_at` advances past
/// the first `u32`. The dequantizers do too much — scales, nibbles, sub-block indexing — for their
/// output to name one cause. This does one thing, so its output is the answer rather than evidence for
/// an argument.
///
/// `dst[i] = f32(byte_at(i))`. A correct run returns the source bytes. The failure modes are then
/// distinguishable at a glance:
///
/// * `[0,1,2,3, 4,5,6,7, ...]` — `byte_at` is fine and the fault is in the dequantizers.
/// * `[0,1,2,3, 0,1,2,3, ...]` — the index wraps within word 0.
/// * `[0,1,2,3, 3,3,3,3, ...]` — reads past word 0 clamp to the last valid byte.
/// * `[0,1,2,3, 0,0,0,0, ...]` — reads past the binding return zero, so the BINDING is short.
///
/// The last two are the ones the Q6_K signature is consistent with, and they need opposite fixes: a
/// clamp means the binding is right and the index is wrong; a zero means the binding is short.
/// Does this adapter's driver kill long-running compute dispatches?
///
/// PURE, so it can be cross-checked against `tuneprofile::device_defaults`, which encodes the same
/// decision in a separate crate. That duplication already went wrong once: _358 widened the engine's
/// guard to mobile GPUs and the tuning table stayed Apple-only for four days, because a policy
/// expressed in two places gets corrected in one and the survivor looks right.
pub fn watchdog_prone_name(adapter: &str) -> bool {
    let n = adapter.to_ascii_lowercase();
    ["adreno", "mali", "powervr", "immortalis", "xclipse", "vivante", "apple", "agx", "honeykrisp"]
        .iter().any(|m| n.contains(m))
}

/// The same probe, reading through `array<vec4<u32>>` instead of `array<u32>`.
///
/// ## The diagnosis this confirms
///
/// On an M2 Ultra the scalar probe returned correct bytes at exactly `i % 16 < 4` — that is,
/// `src[0]`, `src[4]`, `src[8]`, `src[12]` read and every element between them returned zero.
///
/// **Only every fourth `u32` is reachable.** That is a 16-byte stride on an `array<u32>`: std140
/// scalar-array padding applied to a storage buffer that should be std430 and tightly packed. Each
/// scalar occupies a 16-byte slot and only its first 4 bytes hold data.
///
/// If this vec4 variant reads the source correctly while the scalar one does not, the layout is the
/// cause and the fix is mechanical: every `array<u32>` storage binding becomes `array<vec4<u32>>`
/// with the component selected by hand. If BOTH fail, the stride theory is wrong and the fault is
/// elsewhere — which is why this ships as a probe rather than as a rewrite of every kernel.
pub const BYTE_PROBE_VEC4: &str = r#"
@group(0) @binding(0) var<storage, read> src: array<vec4<u32>>;
@group(0) @binding(1) var<storage, read_write> dst: array<f32>;
@group(0) @binding(2) var<uniform> dims: vec4<u32>;

// Same byte address, reached through a vec4 element: word i>>2 is component (i>>2)&3 of vec4 i>>4.
fn byte_at(i: u32) -> u32 {
    let w = src[i >> 4u][(i >> 2u) & 3u];
    return (w >> ((i & 3u) * 8u)) & 0xFFu;
}

@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>) {
    let i = g.x;
    if (i >= dims.x) { return; }
    dst[i] = f32(byte_at(i));
}
"#;

/// Writes `dst[i] = f32(i)`. NO memory read, no source buffer, nothing but the builtin.
///
/// ## Why this is the probe that was missing
///
/// The upload is byte-exact and both element sizes fail identically, so the buffer is right and the
/// read is right. What is left is the INDEX — and the set that comes back correct is
/// `0,1,2,3, 16,17,18,19, 32,33,34,35, 48,49,50,51`.
///
/// That is not a memory pattern. It is four consecutive values every sixteen, which is what
/// `global_invocation_id.x` looks like if the workgroup's x extent used in the builtin does not match
/// the declared `@workgroup_size(64,1,1)`.
///
/// If this probe shows the same 4-of-16 pattern, nothing is wrong with any read: the invocations
/// themselves are landing on the wrong indices, and every kernel that indexes by `global_invocation_id`
/// is affected — which is all of them.
pub const ID_PROBE: &str = r#"
@group(0) @binding(0) var<storage, read> src: array<u32>;
@group(0) @binding(1) var<storage, read_write> dst: array<f32>;
@group(0) @binding(2) var<uniform> dims: vec4<u32>;

@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>) {
    let i = g.x;
    if (i >= dims.x) { return; }
    dst[i] = f32(i);
}
"#;

/// IQ1_S mat-vec, reading the checkpoint's own 50-byte super-blocks.
///
/// ## Why a native kernel rather than a conversion
///
/// The engine converts every source format to q4 before running. For Q4_K that is size-neutral. For
/// IQ1_S it is a **2.88× expansion** — 25 GiB of GLM-5.2 becomes 71.5 GiB — which is the entire
/// reason that model does not fit on a 63 GiB machine. Converting up cannot recover quality the
/// checkpoint already discarded; it only spends the memory.
///
/// ## Layout, from `gguf.rs`'s dequantizer rather than from the spec
///
/// 50 bytes per 256 weights: `d` as f16, 32 bytes of `qs`, then eight `u16` of `qh`. Each group of 8
/// weights takes an **11-bit** codebook index — 8 bits from `qs`, 3 more from `qh` at a per-group
/// offset — into 2048 ternary entries. Per 32-weight sub-block `qh` also carries a 3-bit scale
/// (`dl = d·(2s+1)`) and a sign bit selecting `±IQ1S_DELTA`.
///
/// The grid arrives as a storage buffer of `u32` pairs: entry `i` is `grid[2i]` (values 0..3) and
/// `grid[2i+1]` (values 4..7), each byte a signed ternary value.
///
/// ## Activations stay f32
///
/// The weights are ternary, so the product is a select-and-add and there is nothing for an integer
/// path to accelerate. That is the opposite of the Q8_0 case where the integer dot was worth 2×.
pub const MATVEC_IQ1_S: &str = r#"
struct D { m:u32, n:u32, k:u32, kb:u32, gx:u32, pad0:u32, pad1:u32, pad2:u32 };
@group(0) @binding(0) var<storage, read> a: array<f32>;      // activations [k]
@group(0) @binding(1) var<storage, read> w: array<u32>;      // IQ1_S blocks, 50 B per 256
@group(0) @binding(2) var<storage, read> grid: array<u32>;   // 2048 entries, 2 u32 each
@group(0) @binding(3) var<storage, read_write> c: array<f32>;
@group(0) @binding(4) var<uniform> d: D;

// Byte at absolute offset `i` of the weight blob. IQ1_S rows are 50-byte multiples, which is NOT a
// multiple of 4, so a row does not start on a word boundary and every read must go through here.
fn wbyte(i: u32) -> u32 { return (w[i >> 2u] >> ((i & 3u) * 8u)) & 0xFFu; }
fn whalf(i: u32) -> u32 { return wbyte(i) | (wbyte(i + 1u) << 8u); }

// f16 -> f32. The scale is the only float in the block and it is half-precision.
fn f16(h: u32) -> f32 {
    let sgn = f32(1 - i32((h >> 15u) & 1u) * 2);
    let exp = (h >> 10u) & 0x1Fu;
    let man = h & 0x3FFu;
    if (exp == 0u) { return sgn * f32(man) * 5.9604645e-8; }
    if (exp == 31u) { return sgn * 65504.0; }
    return sgn * exp2(f32(i32(exp) - 15)) * (1.0 + f32(man) / 1024.0);
}

@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
    let row = g.x + g.y * d.gx;
    if (row >= d.n) { return; }
    let nblk = d.k / 256u;
    let rowbase = row * nblk * 50u;          // 50 bytes per 256 weights
    // Two accumulator chains: every FMA otherwise waits on the previous one, which serialises the
    // loop to iterations x latency regardless of available bandwidth. Colibri's int8 matvec does the
    // same on NEON, and here k is thousands rather than tens.
    var acc0 = 0.0;
    var acc1 = 0.0;
    for (var b = 0u; b < nblk; b = b + 1u) {
        let blk = rowbase + b * 50u;
        let dsc = f16(whalf(blk));
        for (var ib = 0u; ib < 8u; ib = ib + 1u) {
            let h = whalf(blk + 34u + 2u * ib);
            let dl = dsc * f32(2u * ((h >> 12u) & 7u) + 1u);
            var delta = 0.125;
            if ((h & 0x8000u) != 0u) { delta = -0.125; }
            for (var l = 0u; l < 4u; l = l + 1u) {
                // 11-bit index: 8 bits from qs, 3 from qh at this group's offset.
                let idx = wbyte(blk + 2u + ib * 4u + l) | (((h >> (3u * l)) & 7u) << 8u);
                let lo = grid[idx * 2u];
                let hi = grid[idx * 2u + 1u];
                let base = b * 256u + ib * 32u + l * 8u;
                for (var j = 0u; j < 4u; j = j + 1u) {
                    let tl = f32(i32((lo >> (8u * j)) & 0xFFu) << 24u >> 24u);
                    let th = f32(i32((hi >> (8u * j)) & 0xFFu) << 24u >> 24u);
                    acc0 = acc0 + a[base + j] * (tl + delta) * dl;
                    acc1 = acc1 + a[base + 4u + j] * (th + delta) * dl;
                }
            }
        }
    }
    c[row] = acc0 + acc1;
}
"#;

/// `dst[i] = f32(src[i])` — one distinct word per thread, no shift, no shared index.
///
/// ## What this isolates
///
/// `byte_at` reads `src[i >> 2]`, so **four adjacent threads read the same word** and each extracts a
/// different byte. That is the one property the real matvecs do not share: `MATVEC_Q4`'s inner loop
/// walks distinct indices per thread.
///
/// The observed fault is exact and size-independent — only `u32` indices ≡ 0 (mod 4) return data, and
/// in the vec4 form only component `.x` of each element. That is what scalar loads look like when the
/// compiler folds them into a `vec4` load and uses only the first lane.
///
/// If this probe reads correctly while `byte_at` does not, the fault is in the *shared-index* access
/// pattern rather than in storage reads at all — and every kernel that works, works because it does
/// not do that.
pub const WORD_PROBE: &str = r#"
@group(0) @binding(0) var<storage, read> src: array<u32>;
@group(0) @binding(1) var<storage, read_write> dst: array<f32>;
@group(0) @binding(2) var<uniform> dims: vec4<u32>;

@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>) {
    let i = g.x;
    if (i >= dims.x) { return; }
    dst[i] = f32(src[i]);
}
"#;

pub const BYTE_PROBE: &str = r#"
@group(0) @binding(0) var<storage, read> src: array<u32>;
@group(0) @binding(1) var<storage, read_write> dst: array<f32>;
@group(0) @binding(2) var<uniform> dims: vec4<u32>;

fn byte_at(i: u32) -> u32 { return (src[i >> 2u] >> ((i & 3u) * 8u)) & 0xFFu; }

@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>) {
    let i = g.x;
    if (i >= dims.x) { return; }
    dst[i] = f32(byte_at(i));
}
"#;

pub const Q6K_DEQUANT: &str = r#"
struct D { n:u32, _a:u32, _b:u32, _c:u32 };
@group(0)@binding(0) var<storage,read> src: array<u32>;
@group(0)@binding(1) var<storage,read_write> dst: array<f32>;
@group(0)@binding(2) var<uniform> dims: D;

fn byte_at(i: u32) -> u32 { return (src[i >> 2u] >> ((i & 3u) * 8u)) & 0xFFu; }

fn f16(h: u32) -> f32 {
    let sign = (h >> 15u) & 1u;
    let exp  = (h >> 10u) & 0x1Fu;
    let mant = h & 0x3FFu;
    var bits: u32;
    if (exp == 0u) {
        if (mant == 0u) {
            bits = sign << 31u;
        } else {
            // subnormal: normalize until bit 10 is the implicit leading 1
            var e: i32 = -14; var m: u32 = mant;
            loop { if ((m & 0x400u) != 0u) { break; } m = m << 1u; e = e - 1; }
            bits = (sign << 31u) | (u32(e + 127) << 23u) | ((m & 0x3FFu) << 13u);
        }
    } else if (exp == 0x1Fu) {
        bits = (sign << 31u) | (0xFFu << 23u) | (mant << 13u);   // inf / nan, preserved
    } else {
        bits = (sign << 31u) | (u32(i32(exp) + 127 - 15) << 23u) | (mant << 13u);
    }
    return bitcast<f32>(bits);
}

// int8 from a byte: the scales are SIGNED, and reading them as unsigned silently doubles half of them.
fn i8_at(i: u32) -> i32 {
    let b = byte_at(i);
    if (b >= 128u) { return i32(b) - 256; }
    return i32(b);
}

@compute @workgroup_size(64,1,1)
fn main(@builtin(global_invocation_id) g:vec3<u32>, @builtin(num_workgroups) nwg:vec3<u32>) {
    let blk = g.x + g.y * nwg.x * 64u;
    if (blk >= dims.n) { return; }
    let base = blk * 210u;
    let d = f16(byte_at(base + 208u) | (byte_at(base + 209u) << 8u));
    let out = blk * 256u;
    for (var half: u32 = 0u; half < 2u; half = half + 1u) {
        let ql = base + half * 64u;
        let qh = base + 128u + half * 32u;
        let sc = base + 192u + half * 8u;
        let o  = out + half * 128u;
        for (var l: u32 = 0u; l < 32u; l = l + 1u) {
            let is = l / 16u;
            let a = byte_at(ql + l);
            let b = byte_at(ql + l + 32u);
            let h = byte_at(qh + l);
            let q1 = i32((a & 0xFu) | (((h >> 0u) & 3u) << 4u)) - 32;
            let q2 = i32((b & 0xFu) | (((h >> 2u) & 3u) << 4u)) - 32;
            let q3 = i32((a >> 4u)  | (((h >> 4u) & 3u) << 4u)) - 32;
            let q4 = i32((b >> 4u)  | (((h >> 6u) & 3u) << 4u)) - 32;
            // Bind (d * scale) to a NAMED value before multiplying by the quant. The CPU reference
            // computes `dsc = d * sc` once per sub-block and then `dsc * q`; writing it as a single
            // expression lets the compiler contract the chain into an FMA, which rounds ONCE instead of
            // twice and lands 1-2 ULPs away. More accurate, and not what the reference does — and a
            // dequantizer that disagrees with the reference by a ULP compounds across 48 layers.
            let d1 = d * f32(i8_at(sc + is));
            let d2 = d * f32(i8_at(sc + is + 2u));
            let d3 = d * f32(i8_at(sc + is + 4u));
            let d4 = d * f32(i8_at(sc + is + 6u));
            dst[o + l]       = d1 * f32(q1);
            dst[o + l + 32u] = d2 * f32(q2);
            dst[o + l + 64u] = d3 * f32(q3);
            dst[o + l + 96u] = d4 * f32(q4);
        }
    }
}
"#;

pub const Q4K_DEQUANT: &str = r#"
struct D { n:u32, grid_w:u32, _b:u32, _c:u32 };
@group(0)@binding(0) var<storage,read> src: array<u32>;     // raw Q4_K bytes, viewed as words
@group(0)@binding(1) var<storage,read_write> dst: array<f32>;
@group(0)@binding(2) var<uniform> dims: D;                  // n = super-block count

fn byte_at(i: u32) -> u32 { return (src[i >> 2u] >> ((i & 3u) * 8u)) & 0xFFu; }

// half -> f32 (subnormals and inf/nan handled the same way the CPU helper does)
// Bit-for-bit twin of the CPU gguf::f16_to_f32: construct the f32 bit pattern and bitcast, rather than
// computing a value. Approximating here is not safe — mapping Inf/NaN to a finite number diverges from the
// reference exactly where a corrupt or synthetic block carries one, and the divergence shows up as
// slightly wrong weights rather than an error.
fn f16(h: u32) -> f32 {
    let sign = (h >> 15u) & 1u;
    let exp  = (h >> 10u) & 0x1Fu;
    let mant = h & 0x3FFu;
    var bits: u32;
    if (exp == 0u) {
        if (mant == 0u) {
            bits = sign << 31u;
        } else {
            // subnormal: normalize until bit 10 is the implicit leading 1
            var e: i32 = -14; var m: u32 = mant;
            loop { if ((m & 0x400u) != 0u) { break; } m = m << 1u; e = e - 1; }
            bits = (sign << 31u) | (u32(e + 127) << 23u) | ((m & 0x3FFu) << 13u);
        }
    } else if (exp == 0x1Fu) {
        bits = (sign << 31u) | (0xFFu << 23u) | (mant << 13u);   // inf / nan, preserved
    } else {
        bits = (sign << 31u) | (u32(i32(exp) + 127 - 15) << 23u) | (mant << 13u);
    }
    return bitcast<f32>(bits);
}

// the 6-bit scale/min unpack, byte-for-byte with get_scale_min_k4
fn scale_min(j: u32, base: u32) -> vec2<f32> {
    var sc: u32; var mn: u32;
    if (j < 4u) {
        sc = byte_at(base + j) & 63u;
        mn = byte_at(base + j + 4u) & 63u;
    } else {
        sc = (byte_at(base + j + 4u) & 0x0Fu) | ((byte_at(base + j - 4u) >> 6u) << 4u);
        mn = (byte_at(base + j + 4u) >> 4u) | ((byte_at(base + j) >> 6u) << 4u);
    }
    return vec2<f32>(f32(sc), f32(mn));
}

@compute @workgroup_size(64)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    // 2D grid: a workgroup-per-super-block overflows the 65535 per-dimension dispatch limit on any
    // large tensor (a 26M-weight matrix needs 102400 of them), so the grid wraps at grid_w.
    let blk = wg.y * dims.grid_w + wg.x;
    if (blk >= dims.n) { return; }
    let bbase = blk * 144u;                       // byte offset of this super-block
    let d    = f16(byte_at(bbase) | (byte_at(bbase + 1u) << 8u));
    let dmin = f16(byte_at(bbase + 2u) | (byte_at(bbase + 3u) << 8u));
    let sbase = bbase + 4u;                       // 12 scale bytes
    let qbase = bbase + 16u;                      // 128 nibble bytes
    // 64 lanes each handle 4 of the 256 outputs, mirroring the CPU's 4 groups of 64
    for (var t = lid.x; t < 256u; t = t + 64u) {
        let grp = t / 64u;                        // which of the 4 groups
        let idx = t % 64u;                        // position inside the group
        let half = idx / 32u;                     // low nibbles then high nibbles
        let l = idx % 32u;
        let sm = scale_min(grp * 2u + half, sbase);
        let q = byte_at(qbase + grp * 32u + l);
        let nib = select(q & 0xFu, q >> 4u, half == 1u);
        dst[blk * 256u + grp * 64u + half * 32u + l] = d * sm.x * f32(nib) - dmin * sm.y;
    }
}
"#;

/// Q4_K → our resident q4 layout, entirely on the GPU.
///
/// This is what removes the CPU load pass. Today the loader does Q4_K → f32 (CPU) → our q4 (CPU) → upload,
/// which for Scout is ~50 GiB of host copies and many minutes. With this kernel the raw file blocks go up
/// as-is and the conversion happens on the GPU, so nothing is materialized host-side and nothing is
/// dequantized to f32 at any point.
///
/// Output is a single buffer laid out qw‖qs, matching `quantize_q4_host_bytes` — so `q4_lin_from_slots`
/// wraps it unchanged. One workgroup per 256-weight super-block; its 8 lanes each own one 32-element
/// block, computing that block's absmax scale and packing 8 nibbles per word exactly as the CPU
/// `quantize_q4_resident_cpu` does (scale = amax/7, nibble = round(w/scale) + 8).
pub const Q4K_TO_Q4: &str = r#"
struct D { nblk:u32, qs_off:u32, qw_base:u32, grid_w:u32 };
@group(0)@binding(0) var<storage,read> src: array<u32>;      // raw Q4_K bytes as words
@group(0)@binding(1) var<storage,read_write> dst: array<u32>; // qw words, then qs (bitcast f32)
@group(0)@binding(2) var<uniform> dims: D;

fn byte_at(i: u32) -> u32 { return (src[i >> 2u] >> ((i & 3u) * 8u)) & 0xFFu; }

// Bit-for-bit twin of the CPU gguf::f16_to_f32 (see the note on the dequant kernel): construct the f32
// bit pattern and bitcast, so Inf/NaN survive instead of being approximated away.
fn f16(h: u32) -> f32 {
    let sign = (h >> 15u) & 1u;
    let exp  = (h >> 10u) & 0x1Fu;
    let mant = h & 0x3FFu;
    var bits: u32;
    if (exp == 0u) {
        if (mant == 0u) {
            bits = sign << 31u;
        } else {
            var e: i32 = -14; var m: u32 = mant;
            loop { if ((m & 0x400u) != 0u) { break; } m = m << 1u; e = e - 1; }
            bits = (sign << 31u) | (u32(e + 127) << 23u) | ((m & 0x3FFu) << 13u);
        }
    } else if (exp == 0x1Fu) {
        bits = (sign << 31u) | (0xFFu << 23u) | (mant << 13u);
    } else {
        bits = (sign << 31u) | (u32(i32(exp) + 127 - 15) << 23u) | (mant << 13u);
    }
    return bitcast<f32>(bits);
}

fn scale_min(j: u32, base: u32) -> vec2<f32> {
    var sc: u32; var mn: u32;
    if (j < 4u) { sc = byte_at(base + j) & 63u; mn = byte_at(base + j + 4u) & 63u; }
    else {
        sc = (byte_at(base + j + 4u) & 0x0Fu) | ((byte_at(base + j - 4u) >> 6u) << 4u);
        mn = (byte_at(base + j + 4u) >> 4u) | ((byte_at(base + j) >> 6u) << 4u);
    }
    return vec2<f32>(f32(sc), f32(mn));
}

/// weight at position `t` (0..256) of super-block `blk`
fn wq(blk: u32, t: u32) -> f32 {
    let bbase = blk * 144u;
    let d    = f16(byte_at(bbase) | (byte_at(bbase + 1u) << 8u));
    let dmin = f16(byte_at(bbase + 2u) | (byte_at(bbase + 3u) << 8u));
    let grp = t / 64u; let idx = t % 64u; let half = idx / 32u; let l = idx % 32u;
    let sm = scale_min(grp * 2u + half, bbase + 4u);
    let q = byte_at(bbase + 16u + grp * 32u + l);
    let nib = select(q & 0xFu, q >> 4u, half == 1u);
    return d * sm.x * f32(nib) - dmin * sm.y;
}

@compute @workgroup_size(8)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    // see the note on the dequant kernel: the grid wraps at grid_w to stay under 65535 per dimension
    let blk = wg.y * dims.grid_w + wg.x;
    if (blk >= dims.nblk) { return; }
    let j = lid.x;                       // which 32-element block inside this super-block
    let t0 = j * 32u;
    // absmax over the 32 weights, then the same scale rule as the CPU quantizer
    var amax = 0.0;
    for (var l = 0u; l < 32u; l = l + 1u) {
        let v = abs(wq(blk, t0 + l));
        if (v == v) { amax = max(amax, v); }   // NaN-skipping, matching Rust's f32::max
    }
    var scale = 1.0;
    if (amax > 0.0) { scale = amax / 7.0; }
    let bi = blk * 8u + j;               // global index of this 32-block
    dst[dims.qs_off + bi] = bitcast<u32>(scale);
    // pack 8 nibbles per word, 4 words per 32-block
    for (var w = 0u; w < 4u; w = w + 1u) {
        var acc = 0u;
        for (var b = 0u; b < 8u; b = b + 1u) {
            let v = wq(blk, t0 + w * 8u + b);
            // Two deliberate mismatches with the naive spelling, both to match the CPU reference:
            //  * Rust's `f32 as i32` saturates NaN to 0; WGSL's i32() of NaN is undefined.
            //  * Rust's f32::round() is half-AWAY-from-zero; WGSL's round() is half-to-even.
            var qv = 0.0;
            if (v == v) {
                let x = v / scale;
                let r = sign(x) * floor(abs(x) + 0.5);
                qv = clamp(r, -7.0, 7.0);
            }
            let nib = u32(i32(qv) + 8) & 0xFu;
            acc = acc | (nib << (b * 4u));
        }
        dst[dims.qw_base + bi * 4u + w] = acc;
    }
}
"#;

#[cfg(test)]
mod shader_drift_tests {
    /// V3 must be reachable by the autotune, not only by an env var.
    ///
    /// It existed, was benchmarked, and was selectable only through LOGSCOPE_MATVEC_V3 — so the
    /// per-shape search compared every OTHER kernel and picked V2Wg128. I read that as "kernel choice
    /// is exhausted". It wins the search it was entered in.
    #[test]
    fn the_autotune_candidate_list_includes_the_wide_load_kernel() {
        use super::{DECODE_PROFILES, MatvecKernel};
        assert!(DECODE_PROFILES.iter().any(|c| c.kernel == MatvecKernel::V3),
            "V3 is not searched, so it cannot be chosen however fast it is");
        // And every other kernel stays in, since removing a candidate is how a search silently narrows.
        for k in [MatvecKernel::V1, MatvecKernel::V2, MatvecKernel::V2Wg128, MatvecKernel::V2WgSg] {
            assert!(DECODE_PROFILES.iter().any(|c| c.kernel == k), "{k:?} dropped from the search");
        }
    }

    /// A cached id must resolve to the kernel it was written for.
    ///
    /// V3 took id 4, an unused value, precisely so an existing cache cannot re-resolve to it. Reusing
    /// an id would make every previously-cached choice silently mean something else — the stale-cache
    /// class, self-inflicted.
    #[test]
    fn kernel_ids_are_stable_across_the_addition() {
        use super::MatvecKernel;
        assert_eq!(MatvecKernel::V1.id(), 0);
        assert_eq!(MatvecKernel::V2.id(), 1);
        assert_eq!(MatvecKernel::V2Wg128.id(), 2);
        assert_eq!(MatvecKernel::V2WgSg.id(), 3);
        assert_eq!(MatvecKernel::V3.id(), 4, "a NEW id, so no existing cache re-resolves");
    }

    #[test]
    fn V3_does_not_need_subgroups() {
        // The point of wiring it now: it is the one decode speedup that does not wait on the subgroup
        // chain, which is blocked behind the short-binding finding.
        use super::MatvecKernel;
        assert!(!MatvecKernel::V3.needs_subgroups());
        assert!(MatvecKernel::V2WgSg.needs_subgroups());
    }

    /// Every caller of `submit` must tag a cause.
    ///
    /// The profile reported "N UNATTRIBUTED, a submit path is still untagged" for months. There is one
    /// submit site and eleven callers, and SIX of them incremented nothing — four buffer readbacks and
    /// two frame flushes. The counters were correct; nobody was calling them.
    ///
    /// Source-read rather than behavioural for the same reason as the reset-list test: an untagged
    /// caller produces a number that is correct in isolation, and only the shortfall line reveals it —
    /// which is exactly the signal everyone had learned to ignore.
    #[test]
    fn every_submit_caller_tags_a_cause() {
        let src = include_str!("lib.rs");
        let mut untagged = Vec::new();
        let lines: Vec<&str> = src.lines().collect();
        for (n, l) in lines.iter().enumerate() {
            // Skip this test's own source, which contains the pattern it searches for.
            if l.contains("l.contains") { continue; }
            if !l.contains("self.submit(") { continue; }
            // A cause must be incremented within the few lines above the flush.
            let lo = n.saturating_sub(8);
            let tagged = lines[lo..n].iter().any(|w| w.contains("SUBMIT_BY_"));
            if !tagged { untagged.push(n + 1); }
        }
        assert!(untagged.is_empty(),
            "submit callers at lines {untagged:?} tag no cause — the profile's attribution shortfall comes from exactly this, and a new one is invisible until someone reads the UNATTRIBUTED count");
    }

    /// A packed decode config must not be readable as a tile.
    ///
    /// Both machines' logs printed "decode tile = 2147549192" on every load — 0x80010008, the packed
    /// (kernel, tile) form with the high marker bit, shown raw. Tiles are 1/2/4/8.
    ///
    /// Harmless to the engine and corrosive to the log: a number nobody can read is a line nobody
    /// checks, and it is the line you consult when a shape is slow.
    #[test]
    fn the_packed_form_round_trips_and_is_NOT_a_plausible_tile() {
        for kernel in [super::MatvecKernel::from_id(1), super::MatvecKernel::from_id(3)] {
            for tile in [1u32, 2, 4, 8] {
                let packed = super::DecodeConfig { kernel, tile }.pack();
                assert!(packed > 0x8000_0000, "the marker bit is what makes it unreadable raw");
                let back = super::DecodeConfig::unpack(packed);
                assert_eq!(back.tile, tile);
                assert_eq!(back.kernel, kernel);
            }
        }
        // The exact values from the two logs.
        assert_eq!(super::DecodeConfig::unpack(2147549192).tile, 8, "M2 Max");
        assert_eq!(super::DecodeConfig::unpack(2147680264).tile, 8, "M2 Ultra");
    }

    #[test]
    fn a_legacy_bare_tile_still_unpacks() {
        // Caches written before the marker bit existed hold a plain tile. Reading one as a packed
        // value would give kernel garbage and a tile of 0.
        let d = super::DecodeConfig::unpack(4);
        assert_eq!(d.tile, 4);
    }

    /// The active-set arithmetic, checked against models whose size is known independently.
    #[test]
    fn active_params_matches_a_known_dense_model() {
        // DeepSeek-R1-Distill-Llama-8B, from the loader's own line: d=4096 layers=32 heads=32/8
        // head_dim=128 ff=14336 vocab=128256. An 8B dense model reads roughly all of itself per token,
        // so the figure must land near 8e9 — and the GQA term is what makes it land there.
        let p = super::active_params_per_token(4096, 14336, 128256, 32, 8, 128, 0);
        assert!((7.0e9..9.0e9).contains(&(p as f64)), "got {:.2e}, expected ~8e9", p as f64);
    }

    #[test]
    fn GQA_is_counted_and_NOT_four_square_matrices() {
        // Assuming q, k, v and o are all d x d overstates attention on a 32/8 model. The roofline
        // bound would then say the run was closer to the limit than it is — telling an operator to
        // stop optimising a run that had headroom.
        let gqa = super::active_params_per_token(4096, 14336, 128256, 32, 8, 128, 0);
        let mha = super::active_params_per_token(4096, 14336, 128256, 32, 32, 128, 0);
        assert!(mha > gqa, "full attention reads more: {mha} vs {gqa}");
    }

    #[test]
    fn an_MoE_counts_only_the_ACTIVE_experts() {
        // Qwen3-30B-A3B is 30B total and ~3B active. Counting all 128 experts would claim ten times
        // the bandwidth the run needs, and the roofline verdict would then report a run as nowhere
        // near the limit when it is at it.
        let active8 = super::active_params_per_token(2048, 768, 151936, 48, 4, 128, 8);
        let all128 = super::active_params_per_token(2048, 768, 151936, 48, 4, 128, 128);
        assert!(all128 > active8 * 5, "routing is the whole difference: {all128} vs {active8}");
    }

    /// The watchdog guard must fire on every device whose driver kills long dispatches.
    ///
    /// It was keyed on `conservative_vma` — "Vulkan on Apple" — while bounding the decode window to
    /// avoid a GPU watchdog. Android drivers have the same watchdog, so a phone took the unbounded
    /// path and would trip the very thing the guard exists to prevent.
    ///
    /// Tested on the name list directly because `watchdog_prone` needs a live Gpu, and the list is the
    /// part that goes stale when a vendor is added.
    #[test]
    fn the_watchdog_list_covers_mobile_AND_apple() {
        let matches = |s: &str| {
            let l = s.to_ascii_lowercase();
            ["adreno", "mali", "powervr", "immortalis", "xclipse", "vivante", "apple", "agx"]
                .iter().any(|m| l.contains(m))
        };
        for a in ["Adreno (TM) 740", "Mali-G715", "Immortalis-G720", "Samsung Xclipse 940",
                  "Apple M2 Max (G14C B1) [Vulkan]", "Apple M2 Ultra (G14D B1) [Vulkan]"] {
            assert!(matches(a), "{a} kills long dispatches and must be bounded");
        }
        // Desktop parts must NOT be bounded, or every one of them pays per-token submit overhead for
        // a watchdog they do not have.
        for a in ["NVIDIA GeForce RTX 4090", "AMD Radeon RX 7900 XTX", "Intel Arc A770",
                  "llvmpipe (LLVM 20.1.2, 256 bits)"] {
            assert!(!matches(a), "{a} has no watchdog; bounding it costs speed for nothing");
        }
    }

    /// Every per-run counter must be reset together.
    ///
    /// Observed across four consecutive runs: `no-frame 1480` each time, while its reported share moved
    /// from 35% to 7%. The numerator was cumulative and the denominator per-run, so the percentage
    /// described nothing — and it is the number an operator tunes against.
    ///
    /// The test reads the SOURCE of gpu_counters_reset rather than exercising it, because a counter
    /// left out is invisible to a behavioural test: it counts correctly, the line prints, and only a
    /// comparison across runs reveals it never moved. Checking the reset list directly is the only
    /// thing that catches the omission at the moment someone adds a counter.
    #[test]
    fn every_submit_cause_counter_is_in_the_reset_list() {
        let src = include_str!("lib.rs");
        let start = src.find("pub fn gpu_counters_reset()").expect("reset fn");
        let body = &src[start..start + 600];
        for name in ["SUBMIT_BY_PASSES", "SUBMIT_BY_RETAIN", "SUBMIT_BY_DOWNLOAD",
                     "SUBMIT_BY_DIMS_WRAP", "SUBMIT_BY_END_FRAME", "SUBMIT_BY_NO_FRAME"] {
            assert!(body.contains(name),
                "{name} is declared but never reset — its share of the profile will be nonsense");
        }
        // And the counters that were already there stay there.
        for name in ["GPU_SUBMITS", "GPU_POLLS", "GPU_SETUP_NS", "GPU_SYNC_NS"] {
            assert!(body.contains(name), "{name} dropped from the reset list");
        }
    }

    /// Source strides must follow the tensor's own type.
    ///
    /// Q4_K is 144 bytes per 256-weight super-block and Q6_K is 210. The file-view slicing was
    /// hardcoded to 144, so even with the type check lifted a Q6_K tensor would have been sliced at
    /// the wrong offset — every expert after the first reading from the middle of its predecessor.
    /// The two changes are only useful together, which is why the stride is tested rather than assumed.
    #[test]
    fn source_block_bytes_follows_the_type() {
        let count = 256 * 100;
        assert_eq!(super::Gpu::source_block_bytes(12, count), Some(100 * 144), "Q4_K");
        assert_eq!(super::Gpu::source_block_bytes(14, count), Some(100 * 210), "Q6_K");
        assert_ne!(super::Gpu::source_block_bytes(12, count),
                   super::Gpu::source_block_bytes(14, count),
                   "if these matched, a hardcoded stride would have been harmless and this is noise");
    }

    #[test]
    fn an_unverified_source_type_has_NO_block_size() {
        // Returning a plausible size for a format nobody has checked is how a model loads and produces
        // quietly wrong output. None sends it to the host copy instead.
        for t in [0u32, 8, 13, 19, 99] {
            assert_eq!(super::Gpu::source_block_bytes(t, 256), None, "type {t}");
        }
    }

    /// The mobile-GPU names that disable the subgroup kernel.
    ///
    /// Reported from a Fairphone: llama3.2-1B loaded and then failed with a driver message about "not
    /// enough wavelets" — a driver term for waves/subgroups, and not a string in this codebase. The
    /// guard existed but was keyed on Apple-Vulkan alone, so every mobile GPU took the subgroup path
    /// with no check that the driver can schedule enough invocations per workgroup.
    ///
    /// Matched on ADAPTER NAME because there is no feature bit for "this is a phone" — the extension is
    /// present and honest, and the limit is on how many subgroups run at once. A false positive costs a
    /// slower kernel; a false negative costs a load failure the user cannot diagnose, so the list errs
    /// toward matching.
    #[test]
    fn the_mobile_gpu_list_covers_the_major_vendors() {
        let names = ["adreno", "mali", "powervr", "immortalis", "xclipse", "vivante"];
        let matches = |s: &str| {
            let l = s.to_ascii_lowercase();
            names.iter().any(|m| l.contains(m))
        };
        // Real adapter strings, as reported by their drivers.
        for a in ["Adreno (TM) 740", "Mali-G715", "PowerVR B-Series BXM-8-256",
                  "Immortalis-G720", "Samsung Xclipse 940"] {
            assert!(matches(a), "{a} must be recognised as mobile");
        }
        // And desktop parts must NOT match, or they lose the fast kernel for nothing.
        for a in ["NVIDIA GeForce RTX 4090", "AMD Radeon RX 7900 XTX",
                  "Intel Arc A770", "Apple M2 Ultra (G14D B1)", "llvmpipe (LLVM 20.1.2, 256 bits)"] {
            assert!(!matches(a), "{a} must NOT be treated as mobile");
        }
    }

    /// `flush_every` must not be mutated by anything other than the tuner or an explicit pin.
    ///
    /// A permanent `store` of a reduced value was added here as "pacing after a readback" and acted as a
    /// one-way ratchet: the tuner picks 128, the first readback lowers it to 32, nothing restores it, and
    /// every subsequent token runs at four times the submit count that was measured as best. The symptom
    /// was submits "firing too quickly again" — an autotuned value silently overridden by a second
    /// mechanism that had no way to give it back.
    ///
    /// This is a source check rather than a runtime one because the failure is structural: any `store`
    /// into flush_every outside `set_flush_every` is the same mistake. `stream_session_window` shows the
    /// correct shape — save, cap, restore.
    #[test]
    fn flush_every_is_not_ratcheted_by_a_second_mechanism() {
        let src = include_str!("lib.rs");
        let stores: Vec<&str> = src.lines()
            .filter(|l| l.contains("flush_every.store("))
            .map(|l| l.trim())
            .filter(|l| !l.starts_with("//"))
            // and not this test's own filter line, which contains the string it searches for
            .filter(|l| !l.contains("l.contains("))
            .collect();
        // Exactly one: the setter. Anything else is a second writer with no restore path.
        assert_eq!(stores.len(), 1,
            "flush_every should be written only by set_flush_every; found {} writers:\n{}",
            stores.len(), stores.join("\n"));
    }

    /// The three shader copies of `f16()` must stay semantically identical.
    ///
    /// They are hand-copied into each quant kernel's source, and one of them carried a wrong subnormal
    /// exponent — seeded at -1 and biased by 113 instead of -14 and +127, halving every subnormal — for
    /// an unknown period. The other two were correct, so `--gate-q4k` stayed green throughout and gave
    /// no hint at all. Duplication is what made that possible: three copies, one review each, no
    /// coupling between them.
    ///
    /// Comparing bodies with comments stripped is the cheapest guard that actually holds.
    /// Deduplicating into one shared constant would be better, but `concat!` does not work across const
    /// items and the restructure would touch every kernel — this catches the same drift for a fraction
    /// of the risk.
    #[test]
    fn the_three_shader_f16_helpers_do_not_drift() {
        let srcs = [("Q6K", super::Q6K_DEQUANT), ("Q4K", super::Q4K_DEQUANT),
                    ("Q4K_TO_Q4", super::Q4K_TO_Q4)];
        let bodies: Vec<(&str, String)> = srcs.iter()
            .filter_map(|(n, s)| super::wgsl_fn_body(s, "fn f16(h: u32)").map(|b| (*n, b)))
            .collect();
        assert_eq!(bodies.len(), 3, "expected three copies, found {}", bodies.len());
        for (name, b) in &bodies[1..] {
            assert_eq!(b, &bodies[0].1, "{}'s f16() has drifted from {}'s", name, bodies[0].0);
        }
        // and the subnormal seed must be the CPU's, not a plausible-looking alternative
        assert!(bodies[0].1.contains("-14"), "subnormal exponent seeds at -14: {}", bodies[0].1);
        assert!(bodies[0].1.contains("e + 127"), "and biases by +127");
    }

    /// `byte_at` is duplicated the same way and reads raw quant bytes — a wrong shift there would
    /// misread every weight rather than a rare subnormal, so it is the more dangerous of the two.
    #[test]
    fn the_shader_byte_at_helpers_do_not_drift() {
        let srcs = [super::Q6K_DEQUANT, super::Q4K_DEQUANT, super::Q4K_TO_Q4];
        let bodies: Vec<String> = srcs.iter()
            .filter_map(|s| super::wgsl_fn_body(s, "fn byte_at(i: u32)")).collect();
        assert_eq!(bodies.len(), 3);
        for b in &bodies[1..] { assert_eq!(b, &bodies[0], "byte_at has drifted"); }
    }
}
