# Robustness patterns from Colibri — and where they map in our engine

Colibri (the C inference engine we use as the streaming-residency reference) is a production
Apple-Silicon + CUDA engine that has already hardened the exact failure classes we've been fighting on
Asahi/Vulkan (submit hangs, watchdog kills, driver quirks). This captures its patterns and maps each to a
concrete change in `gpu-gemm` / the engine. Reference: `/home/claude/colibri/colibri-main/c/backend_metal.mm`,
`docs/metal.md`, `c/doctor.py`, `c/resource_plan.py`.

## Pattern 1 — Bounded command buffers, NEVER one giant submit

Colibri's Metal backend scopes every command buffer to a **logical unit**: per-attention, per-layer,
per-GEMM, per-MoE-block, with the batched routed-expert SwiGLU explicitly bounded to "one command buffer,
S≤4" (docs/metal.md). It batches *within* a layer to amortize Metal's ~5 ms submit latency, but never
collapses the whole sequence/decode into a single unbounded submit.

- **Our bug this explains:** the windowed decode path pinned `flush_every` high to batch a WHOLE window
  (k tokens × ~64 dispatches) into ONE submit. On Asahi (unreliable completion callbacks → submit governor
  disables itself → no backpressure) that one huge submit tripped the firmware watchdog = the decode hang.
- **Fix already shipped:** on `conservative_vma` (Asahi), don't collapse the window — keep a bounded flush
  (≤64 dispatches/submit). Colibri validates this: bounded submits are the *default* discipline, not a
  fallback.
- **Further hardening to adopt:** make "bounded submit" the cross-backend invariant, not an Asahi special
  case. A submit should have a size ceiling everywhere; the window optimization should batch UP TO that
  ceiling, never past it. The ceiling can be backend-tuned (Metal tolerates larger; Asahi smaller).

## Pattern 2 — Every GPU path has a CPU fallback on fault (detect-and-degrade)

Every Colibri command buffer does `[cb waitUntilCompleted]` then checks
`cb.status == MTLCommandBufferStatusError`, logs the specific error, and **returns 0 → the whole
block/layer falls back to CPU**. "The numerics are unchanged; every GPU path falls back to the CPU
per-block on any fault" (docs/metal.md). So a GPU fault degrades to a slow-but-correct result instead of
hanging or crashing.

- **What we do now:** detect-and-degrade per *kernel* (subgroup off on Asahi, serial prefill fallback,
  serial decode-attn flag) — but reactively, one kernel at a time, after a hang is diagnosed.
- **The gap:** we have no *general* "GPU submit faulted → fall back for this unit" escape hatch. When a
  dispatch wedges, we hang instead of catching a status and degrading.
- **To adopt (wgpu specifics):** wgpu surfaces device errors via `Device::on_uncaptured_error` and
  `pop_error_scope`, and submission indices can be polled with `Maintain::wait`. We can wrap a decode step
  (or window) in an error scope + a bounded wait, and on error/timeout mark that path degraded (drop to
  serial kernels / smaller submits / CPU where we have a CPU path) for the rest of the session. This turns
  every future novel driver quirk into a graceful slowdown instead of a hang — the single most valuable
  pattern here.

## Pattern 3 — A CPU/GPU threshold: small ops stay on CPU

`COLI_METAL_GEMM_MIN=16` — matmuls under 16 rows aren't dispatched to the GPU at all; the dispatch
overhead isn't worth it (and it avoids tiny fragile dispatches). Robustness + perf in one knob.

- **Map to us:** we dispatch everything to the GPU regardless of size. A minimum-work threshold (below
  which an op runs on CPU or is merged) would cut submit count on the decode hot path and avoid
  pathologically small dispatches. Candidate: the per-slot windowed sampler and tiny norms.

## Pattern 4 — GPU keep-alive spinner to fight clock ramp-down (latency, not correctness)

`COLI_METAL_SPIN`: a background thread on its OWN command queue runs a trivial kernel in a loop so the GPU
doesn't ramp its clock down between the engine's short per-layer bursts — cutting observed submit latency.
On its own queue so it never blocks real work; detached so it never blocks exit.

- **Relevance:** our M2 logs show submit-latency pain (the submit governor, the flush tuner). A keep-alive
  spinner is an orthogonal lever if dispatch latency (not the watchdog) becomes the bottleneck after the
  hang is fixed. Lower priority than Patterns 1–2 but worth a flag (`LOGSCOPE_GPU_SPIN`).

## Pattern 5 — A read-only preflight "doctor"

`c/doctor.py` runs read-only checks BEFORE inference — model readable, config valid, tokenizer present,
engine built, GPUs discovered, memory sufficient — each emitting pass/warn/fail/skip with a summary. Cheap,
structured, catches misconfig with a clear message instead of an opaque mid-run failure.

- **Map to us:** we discover problems (missing template markers, wrong var names like PREFILL_CHUNK vs
  LOGSCOPE_PREFILL_CHUNK, driver quirks) at runtime, often mid-decode. A preflight that logs a structured
  readiness report at load — adapter/backend, detected fragilities (Asahi → subgroups off, bounded
  submits), effective env knobs and whether each override parsed, KV/residency budget vs available — would
  surface these once, visibly, before the first token. (We already added the "effective prefill chunk"
  log; this generalizes that instinct into one preflight block.)

## Pattern 6 — Explicit memory headroom + reserves in the plan

`resource_plan.py`: `ram_budget = available * 0.88` (12% headroom), a hard `reserve = 2 GB` subtracted from
GPU free before computing `usable`, and an 8 GB floor if the budget is too small. Concrete, conservative
constants — not "use it all and hope."

- **Map to us:** our governor already embodies this philosophy (per-backend budget, autotune down under
  pressure). Colibri's constants are a useful cross-check: an explicit fractional headroom (~12%) and a
  fixed GPU reserve (~2 GB) are reasonable defaults to compare our governor's caps against, especially the
  Apple-Vulkan (Honeykrisp) path where allocation pressure is tightest.

## Priority order for adopting these

1. **Pattern 1 (bounded submits everywhere)** — directly fixes the live decode hang; already half-done for
   Asahi, generalize to a cross-backend submit ceiling.
2. **Pattern 2 (fault → degrade, don't hang)** — the highest-leverage general hardening: wrap decode in a
   wgpu error scope + bounded wait, degrade the session on fault. Converts future unknown driver quirks
   from hangs into slowdowns.
3. **Pattern 5 (preflight doctor)** — cheap, high-visibility; catches the class of "silently ignored knob /
   undetected fragility" that cost us test cycles.
4. **Pattern 3 (CPU/GPU threshold)** and **Pattern 6 (headroom constants)** — incremental perf/safety.
5. **Pattern 4 (keep-alive spinner)** — only if latency, not hangs, is the post-fix bottleneck.

## Ported so far (status)

- **Preflight readiness report** — DONE (backend.rs): logs detected fragilities + active env overrides once at load.
- **Generic fault→degrade latch** (`run_guarded`, `degrade_level`) — DONE: a caught GPU fault drops the session to safe kernels (serial attention, bounded submits). Catches recoverable faults; does NOT un-hang a synchronous wait on a watchdog dispatch (prevention stays front-line).
- **Keep-alive spin** (`LOGSCOPE_GPU_SPIN`) — DONE (opt-in): holds the GPU clock warm between dispatch-bound decode bursts.
- **Hostile-dimension guard** — DONE (load): rejects corrupt/absurd GGUF dims before they overflow allocation arithmetic (Colibri "absurd n" + shape-overflow checks).
- **NaN/Inf sampler poison guard (CPU)** — DONE (Colibri #369): argmax skips non-finite, sample() falls back to finite argmax. A single NaN/+Inf logit no longer collapses the draw to token-0-forever.
- **NaN/Inf guard (GPU SAMPLE_AT / argmax_t)** — DONE: SAMPLE_AT's max-selection reads non-finite logits as -inf so a poisoned logit is never picked (bit-identical on finite logits: sample-window still 0.00e0 vs CPU). Closes the windowed-decode half of the #369 vulnerability.
- **Bounded submits cross-backend** — PARTIAL: Asahi + degraded sessions keep bounded window submits; other backends still cap at a high (finite) ceiling. Generalizing the ceiling is low-priority since it's already finite.

## Model support ported

- **GeGLU packed kernel** — DONE (Gemma-family FFN gate, gelu(gate)·up), validated 5.96e-8. Still needs Gemma RMSNorm (1+w) + attention logit soft-capping + a real checkpoint before Gemma2 is end-to-end.
- **MLA (DeepSeek/GLM compressed-KV attention)** — COMPUTE PATH DONE (un-absorbed, on-GPU, validated): config parse → compress+append to a compressed latent cache → reconstruct K/V via kv_b → attention (MLA_ATTN with separate qk/v dims) → o-proj, every stage bit-checked vs a CPU reference (mla.rs). The compressed cache is the long-context memory win (kv_lora+qk_rope per token). NOTE: Colibri itself uses the un-absorbed reconstruct-on-the-fly form (glm.c:173-175), NOT absorption — so our path matches the reference. Remaining: wire into a live decode forward gated on the loader's MlaConfig + validate vs a real DeepSeek checkpoint.
- **DSA (DeepSeek Sparse Attention)** — CONFIG DONE: DsaConfig parsed + the no-op-below-threshold invariant tested (dense/exact at seq≤index_topk, sparse above → sub-quadratic long context). Remaining: the indexer forward + top-k key selection, validated against a dense reference where they must agree (seq≤index_topk).
- **Absorption optimization** — NOT PORTED, deliberately: Colibri does NOT use it (it reconstructs K/V on the fly, which is what keeps the cache small). Building absorption would diverge from the reference; skipped unless a measured need appears.

