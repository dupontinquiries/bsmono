# gpu-gemm decode throughput — consolidated status & handoff

Consolidates scattered work from prior chats (lavapipe baseline, the c4-parallel
throughput track, and this session's upload-locking / bandwidth / memory work) into
one place, since no clean handoff doc existed. Numbers below are **measured on real
hardware** where noted; everything this session added is validated on lavapipe
(software Vulkan — correctness/structure only, never tok/s).

## 1. The measured starting point (Apple M2 Max [Vulkan], Asahi/Mesa)

`kernel-validate --bench llama-3.2-1b-q4 64 128`:

```
20.66 tok/s (48.4 ms/token)
  per token: resource-setup 1.14 ms | gpu-sync 40.62 ms | other 6.64 ms
  1 submit/token; ~210 dispatches/token
```

Batched-decode (`--workers`, real 1B): agg tok/s **19 → 22 → 30 → 37 → 42.5** for
N = 1,2,4,8,16 (2.23× at N=16, still climbing, sub-linear).

## 2. What the bottleneck actually is (settled, not speculative)

**Dispatch granularity, not arithmetic, not weight bandwidth.** gpu-sync is ~84% of
wall, but a 1B Q4 model is ~0.8 GB of weights — reading them once at ~400 GB/s is
single-digit ms, so we sit ~10× above the bandwidth floor. The gap is ~210 kernel
launches/token through MoltenVK. tile=1 beating multi-tile/multiqueue is consistent
with this: the kernels aren't compute- or occupancy-starved, they're launch-starved.

Reconciling the three words that get used for this:
- **"upload locking"** = the per-dispatch cost of getting commands + resources onto
  the GPU (binding state, encoding). Splits into a CPU slice (the 1.14 ms setup +
  part of the 6.64 ms "other") and a driver/GPU slice folded into gpu-sync.
- **"dispatch-bound"** = the same thing measured as ~210 launches/token.
- **"bandwidth"** = the ~10×-lower floor we only reach *after* the launch count drops.

So: today launch/upload-bound; bandwidth is the post-fusion floor. Two independent
attacks — cut the launch **count** (fusion) and cut the per-launch **cost** (resource
reuse) — both help and compound.

## 3. Validated levers (all bit-exact vs the sequential reference)

Landed in-tree, gated:
- **Fused QKV** — `block_decode_fused` (f32-KV + int8-KV/streaming), behind
  `LOGSCOPE_FUSED_QKV`. 3 projections → 1 matmul, −2 compute dispatches/layer
  (~210 → ~178 on the 1B). `--gate-qkv / -qkv-loader / -qkv-e2e`.
- **GPU argmax + embedding gather** — `generate_kv_gpu`, greedy pick + gather
  on-device, reads back 4 bytes not [vocab]. `--gate-gpusample`.
- **Windowed decode** — `generate_kv_window(K)`, K steps in one submit via in-frame
  argmax→gather dependency. `--gate-window`.
- **Batched prefill / batched & continuous decode / prefix-KV reuse / int8-KV cache**
  — the multi-session stack (`decode_batched`, `run_batched`, `SessionQ8`). int8-KV =
  2.1% logit error, greedy token preserved, ~4× sessions/byte. Prefix reuse 2.2× @ 3
  sessions.
- **Speculative decode** — `speculative_generate_stats` + `--spec` bench; wired into
  the app via `LOGSCOPE_SPEC_DRAFT` this session.

Fidelity (so coherence vs llama.cpp is grounded): int8·f32 cos = **0.99987**;
q4·f32 cos = **0.9119**. int8 is near-lossless; q4 trades coherence for memory.

## 4. This session's additions (commits on `master`, lavapipe-validated)

These attack the **per-launch cost** and the **bandwidth/memory** axes — complementary
to fusion, which attacks the launch **count**.

- `640c5fa` **dims-uniform cache** — `dispatch_with` reused the per-op dims UNIFORM
  instead of `create_buffer_init` every dispatch. `--gate-submit`: 30 same-shape
  dispatches → 1 create + 29 reuse.
- `dc82e03` **buffer pool** — `alloc` recycles same-size output buffers freed by
  dropped Tensors (wgpu WAR/WAW tracking makes reuse ordered-safe; `--gate-kvtree`/
  `-prefill` stay argmax-identical). `--gate-submit`: 30 allocs → 2 create + 28 recycle.
- `acd25e1` **MATVEC_SMALL_Q4** — Q4 small-batch (2≤m≤8) now reads each weight nibble
  once across all rows (was strided re-read), each row **bit-exact** to the m=1 kernel
  → Q4-resident keeps its halved bandwidth under speculative/batched decode.
- `fcbc2d6` **Q4 auto-select at load** — loader runs `memplan::plan` and falls back to
  Q4 when int8 won't fit (was: refuse/OOM), refusing only when even Q4 won't. Fixes
  the 70B "won't load" without an env flag. Auto-loaded model bit-identical to forced
  Q4.

Honest apportionment: dims-cache + buffer-pool primarily shrink the ~16% CPU slice
(and possibly some gpu-sync if MoltenVK per-dispatch binding is in the critical path —
**M2-measurable, not knowable on lavapipe**). The 84% gpu-sync is owned by fusion.

## 5. Remaining work, priority order

1. **Finish fusion (the 84% lever).** Make `LOGSCOPE_FUSED_QKV` default; extend
   fusion to prefill (`block_prefill*`, needs strided q/k extraction at t>1); add the
   plan's further cuts — residual-add folded into matmul epilogue (−2/layer),
   RMSNorm folded into the next matmul's load (−2/layer), RoPE(q)+RoPE(k) merged
   (−1/layer). Target ~210 → ~130–150 dispatches/token.
2. **Memory cleanup.** Once fused is the only consumer, drop `wq/wk/wv` from
   `BlockWeights` + the int8 cache (reclaims the duplicate-resident cost that blocks
   fused-default today). Cache the resident embedding — `generate_kv_gpu` re-uploads
   a ~1 GB f32 table per call.
3. **N×K locked pool** — run `decode_batched` (N rows) inside the windowed frame →
   N·K tokens/submit (the original "pool" goal).
4. **App-side Q4 wiring** — `logscope-egui` `backend.rs` has its own int8-only budget
   pre-check; relax it to defer to the loader's auto-select so the 70B loads in the app.
5. **Measure on M2** via `tools/bench-tps.sh` (restored this session): baseline,
   FUSED_QKV on/off, Q4_RESIDENT on/off, workers sweep, spec. That single pass apportions
   every change above and finds the batched saturation knee.

## 6. Source chats (no single prior handoff existed)

- "Testing with llavapipe feature paths" — lavapipe baseline; 8 gates bit-exact.
- "c4 - parallel - Increasing tokens per second performance" — `DECODE_THROUGHPUT_PLAN.md`,
  the 20.66 tok/s breakdown, fused-QKV/gpusample/windowed patches, `bench-tps.sh`.
- "c4 logscope mind palace work round 3" — merge + further gates.
- This session (c5) — upload-locking, Q4 bandwidth/auto-select, lavapipe runner.
