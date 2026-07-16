# Asahi / AGX Vulkan findings for kBM3's GPU trainers

Distilled from real, hard-won findings in the monorepo's `apps/logscope` (a
wgpu/Vulkan LLM inference engine) and `games/unendlich` (a Vulkan voxel game),
both tested on Apple Silicon under Asahi Linux (Honeykrisp). Sources cited
inline. Everything here is filtered for what actually transfers to a
Burn/CubeCL compute-training workload like `beatspace-gpu`/`vocoder`/
`population` — the render-pass-specific findings (shadow maps, framebuffers,
TBDR loadOp costs) are real but don't apply to pure compute, so they're
omitted here even though they're excellent reading in their own right
(`games/unendlich/docs/PLATFORM_NOTES.md`).

## 1. AGX has a single Vulkan queue family — don't design for async compute

> "AGX exposes only one Vulkan queue family on Honeykrisp... The compute queue
> handle is identical to the graphics queue handle after `vkGetDeviceQueue`.
> At the hardware level AGX has a unified command queue (matching Apple
> Metal's design); the driver doesn't fake multi-queue."
> — `games/unendlich/docs/PLATFORM_NOTES.md`, Finding 1

**Implication for kBM3**: don't spend effort trying to overlap independent
work (e.g. running the population trainer's many (G,D) pairings on separate
command streams to get hardware parallelism) — everything serializes on the
one queue regardless of how it's submitted. The only real levers are (a)
fewer, larger dispatches, and (b) bigger batches per dispatch, not more
concurrent dispatches.

## 2. Pipeline/kernel compile cache is a spin-up win, not a throughput win

> "Pipeline/kernel cache = a SPIN-UP win, not a per-token win... it shaves
> cold start; it does nothing for decode throughput... for the architecture
> [with] ONE process, ONE `Gpu`, `Arc<Model>`, many `Session`s — the kernel
> cache is already effectively done in memory."
> — `apps/logscope/PREWARM.md`

**Matches what we already observed empirically**: every one of our three
Rust binaries (`beatspace-gpu`, `vocoder`, `population`) shows a slow first
iteration/round (tens of seconds to minutes — SPIR-V compilation) followed by
a sharp drop to a stable steady-state rate. That's expected, not a problem to
fix — it's the same in-process pipeline-cache behavior this doc describes.
A persistent on-disk pipeline cache (`Device::create_pipeline_cache`, needs
the `PIPELINE_CACHE` adapter feature) only pays off if training is restarted
as many short-lived processes; for one long-running training process it's
"plumbing with no payoff" (their words) — so there's no action item here
unless we later run training as many short processes (e.g. one process per
population round from an external scheduler).

## 3. Kernel fusion is the real per-step lever, especially at small batch

> "Kernel fusion = the actual per-token / per-batch win. Fewer dispatches per
> token is fewer launch + barrier round-trips, which is exactly what's left
> to claw back on dispatch-bound backends (lavapipe today, and AGX feels it
> too at small batch)."
> — `apps/logscope/PREWARM.md`

**Directly relevant**: our training batches are small (8-32), exactly the
regime called out. We reviewed `gpu/src/model.rs`'s `Generator`/
`Discriminator` forward passes for avoidable extra dispatches (needless
reshapes/clones between conv → batchnorm → activation chains) and didn't find
any low-risk fusion opportunity to apply without hand-writing custom CubeCL
kernels, which is a larger undertaking than this pass covers. Worth
revisiting if/when real per-step timings on actual AGX hardware (not
lavapipe, which has different performance characteristics) show a specific
bottleneck — profile first, then fuse the dispatch that's actually hot,
rather than fusing speculatively.

## 4. Coalesced memory access matters a lot on AGX — same-address multi-thread reads are expensive

> "V2 decode mat-vec: each thread owns WHOLE 32-bit weight words... vs the
> original, where 4 threads read the SAME word and extract one byte each. On
> drivers that don't coalesce same-address subgroup loads (a real risk on
> Asahi/AGX), that was up to 4x redundant weight traffic — the leading
> suspect for the decode running at ~6% of memory bandwidth. Fully coalesced
> (64 threads -> 64 consecutive words)."
> — `apps/logscope/crates/gpu-gemm/src/lib.rs`, comment above `MATVEC_Q8_0_TILED_V2`

**Implication for kBM3**: if we ever hand-write custom CubeCL/WGSL kernels
(e.g. a fused SparseBlock-style gate, or a hand-tuned version of the
vocoder's DFT-matmul STFT), make each thread read a distinct, contiguous
address rather than having multiple threads share one load and split it into
bytes/fields. This is a real, measured 4x effect on this hardware, not a
theoretical concern.

## 5. Don't trust `device.limits()` alone for workgroup/tile sizing — validate empirically

> "Workgroup-tile candidates... filtered to what this device's limits allow...
> Once the load probe has run, restrict to tiles that ACTUALLY compiled here
> (the limit query can over-report — GL accepts the 32x32 limit but rejects
> the shader). Empty valid list = pre-probe."
> — `apps/logscope/crates/gpu-gemm/src/lib.rs`, `gemm_tile_candidates`/`gemm_tile_for`

**Implication for kBM3**: we don't currently hand-select workgroup/tile sizes
(Burn/CubeCL picks kernel configuration for us), so this doesn't apply today.
If we ever add custom kernels with configurable tile sizes, adopt this
pattern: generate candidates from queried limits, then empirically confirm
each candidate actually compiles/runs on the real device before trusting it,
rather than trusting `device.limits()` reports at face value.

## 6. Subgroup operations are a real, usable fast path on this hardware

`subgroupAdd` and friends are used in the gpu-gemm kernels for in-workgroup
reductions with no explicit barrier — a hardware-accelerated primitive that
works on AGX via Vulkan. Not something Burn/CubeCL exposes to us directly
today, but worth knowing subgroup ops are a real, working option if we ever
write custom kernels for a hot reduction (e.g. a hand-written STFT or a
custom loss reduction).

## What we didn't act on and why

`games/unendlich/docs/PLATFORM_NOTES.md` has several more findings (TBDR
`loadOp=LOAD` cost, shadow-pass vertex/draw-bound behavior, cascaded
framebuffer regressions) that are real and well-evidenced but specific to
graphics render passes with attachments/framebuffers — none of which our
compute-only training binaries use. Listed here for completeness in case a
future kBM3 component does rendering (e.g. a live spectrogram visualizer
running on-device rather than in the browser), where they'd become directly
relevant again.
