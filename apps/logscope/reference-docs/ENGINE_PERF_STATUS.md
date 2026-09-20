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

## 4b. A later session's additions (commits aa9dec1b9..f080c720f, lavapipe-validated)

Also section 5 item 1's territory — RoPE fusion, plus two bugs found in the counters
used to apportion everything above, and the PASS_FUSE retraction re-examined against
llama.cpp's own Vulkan backend.

- `aa9dec1b9` **GPU_DISPATCHES was double-counted** — `mark_dispatch` (the bind-group-
  miss path) and `record_dispatch` both incremented it. With the bind-group cache off
  (the default), every dispatch counted twice, so every "~210 dispatches/token" figure
  above, and every "N dispatches/pass" ratio derived from it, reads 2x the true count.
  The `--bench` output's shape does not change; the absolute dispatch numbers in this
  doc predate the fix and should be halved before comparing against a post-fix run.
- `701924b52` **GPU_COPIES** — buffer copies (`copy_buf`: KV-cache row writes,
  `slice_cols`) were never counted anywhere. On the app's decode path they ran ~45% of
  a layer's GPU commands. On Honeykrisp a copy is a compute job (no blit engine), so
  this was invisible cost, not zero cost.
- `e8cedaaf4` **decode-census** (`gpu-gemm/src/bin/`) — per-token dispatch/pass/
  submit/copy/poll census plus a logits hash, mirroring the app's real decode path
  (stream_session -> decode_step), so a lever A/B can be checked bit-exact and
  counted in one run instead of cross-referencing --bench against a gate.
- `2d33e93c2`, `f080c720f` **ROPE_KV_APPEND / QKNORM_ROPE_KV_APPEND** — this is section
  5 item 1's "RoPE(q)+RoPE(k) merged (-1/layer)", done, and further: rope q, and write
  roped k and raw v STRAIGHT INTO the KV-cache rows, in one dispatch. Removes the copies
  found above, not just a rope dispatch: per layer, f32-KV path 6 dispatches + 5 copies
  -> 6 + 0; QK-norm (Qwen3) path 7 + 4 -> 7 + 0. Default-on lever
  (LOGSCOPE_ROPE_KV_APPEND), --gate-rope-kv (logits AND every K/V cache row bit-exact
  vs the copy path, on two fixture families, three deliberate kernel breaks caught).
  f080c720f extends the same idea to decode_batched (the N-session fan path): row
  views into the batched KV tensor instead of per-session copies.
- `21a82e432` **PASS_FUSE re-examined, still Off by default.** The retraction's own
  in-sandbox reproduction (probe-decode-parity) is 0/6 wrong with fusion on as of this
  tree — the copy_buf/zero_tensor drain fix that predates it landed AFTER the hardware
  run that produced the retraction, and no hardware run has been made since. llama.cpp's
  Vulkan backend and wgpu-core's own barrier semantics both permit compute->compute
  barriers inside one command buffer, which is what fusion needs — the driver was never
  the right suspect. With ROPE_KV_APPEND, a fused token drops to 1 pass (was 14
  unfused, 5 fused-with-copies) with an unchanged logits hash on lavapipe. Left Off
  pending an M2 run: bit-exact on lavapipe is not bit-exact on a driver where FMA
  contraction differs, and the lever registry entry says so with the reproduction
  steps rather than asserting either way.
- `62e80e61e` a fixture-generator bug the above work exposed: make-tiny-gguf.py
  restarted its xorshift from one fixed seed per tensor, so any two same-length
  tensors — attn_q_norm and attn_k_norm in the qwen3 fixture — were byte-identical. A
  QK-norm kernel that normed k with the q weight passed a bit-exact gate undetected
  until a deliberate break test (CLAUDE.md section 5b) found it inert. Seeded per
  tensor index now; every emitter that shares write() picks this up.

## 4c. Tool-calling (unrelated axis, same session as 4b)

`d5986ee76` — the app-side tool-call parser now accepts the formats base models are
actually trained on (bare JSON, fenced json, XML-ish tags) in addition to the
harness's own wire format, rather than only the latter. Orthogonal to everything
above; noted here only so this doc's commit range is a complete account of the
session.

## 5. Remaining work, priority order

1. ~~**Finish fusion (the 84% lever).**~~ RoPE(q)+RoPE(k)+KV-append is done (4b).
   Still open from the original plan: residual-add folded into matmul epilogue
   (−2/layer), RMSNorm folded into the next matmul's load (−2/layer) — FUSE_NORM in
   `enginelevers.rs` may already cover part of this, not cross-checked against this
   doc. Then **default `LOGSCOPE_PASS_FUSE=1`** once an M2 run confirms 4b's lavapipe
   result, extend fusion to prefill (`block_prefill*`, needs strided q/k extraction
   at t>1).
2. **Memory cleanup.** Once fused is the only consumer, drop `wq/wk/wv` from
   `BlockWeights` + the int8 cache (reclaims the duplicate-resident cost that blocks
   fused-default today). Cache the resident embedding — `generate_kv_gpu` re-uploads
   a ~1 GB f32 table per call.
3. **N×K locked pool** — run `decode_batched` (N rows) inside the windowed frame →
   N·K tokens/submit (the original "pool" goal). 4b's row-view change is a step
   toward this (fewer per-session copies in the same loop) but does not itself add
   the K-step windowing.
4. **App-side Q4 wiring** — `logscope-egui` `backend.rs` has its own int8-only budget
   pre-check; relax it to defer to the loader's auto-select so the 70B loads in the app.
5. **Measure on M2** via `tools/bench-tps.sh` (restored this session): baseline,
   FUSED_QKV on/off, ROPE_KV_APPEND on/off, PASS_FUSE on/off (the one this doc most
   needs — 4b's lavapipe numbers say it should work now), Q4_RESIDENT on/off, workers
   sweep, spec. That single pass apportions every change above, including 4b's, and
   finds the batched saturation knee. No hardware run has happened since `aa9dec1b9`;
   every dispatch-count figure quoted anywhere before that commit is 2x the true value
   (the double-count bug) and should not be trusted for a before/after comparison
   without re-deriving from a post-fix build.

## 6. Source chats (no single prior handoff existed)

- "Testing with llavapipe feature paths" — lavapipe baseline; 8 gates bit-exact.
- "c4 - parallel - Increasing tokens per second performance" — `DECODE_THROUGHPUT_PLAN.md`,
  the 20.66 tok/s breakdown, fused-QKV/gpusample/windowed patches, `bench-tps.sh`.
- "c4 logscope mind palace work round 3" — merge + further gates.
- This session (c5) — upload-locking, Q4 bandwidth/auto-select, lavapipe runner.
- A later session (c5b, this doc's 4b/4c) — the dispatch-count bug, GPU_COPIES,
  decode-census, ROPE_KV_APPEND/QKNORM_ROPE_KV_APPEND, the PASS_FUSE re-examination,
  the fixture-seeding bug, and (separately) tool-call format acceptance. Commits
  `aa9dec1b9`..`f080c720f` plus `d5986ee76`; see those commit messages for the full
  reasoning and verification each carries.

## Prompt processing: hybrids (Qwen3.5 / Ornith), and what prefill was not doing (2026-09)

Validated on lavapipe only — structure and bit-level agreement, never tok/s. No real GPU, no real
checkpoint. `kernel-validate --gate-prefill-parity` is the gate for everything in this section.

**Prefill and decode were two different models for several families.** `decode_step` routes per
layer and implements every variant; the batched prefill blocks were an older body nobody held to it,
and the only block fns with no refuse-by-name guard. Measured on the then-current engine: a Qwen3.5
hybrid PANICKED on its first decode step (its recurrent state was never advanced — the sequential
route d297e4df5 added was lost in a merge), Gemma-4's prompt logits were off by 115% of the logit
scale, MLA's by 100%. Now `Model::prefill_gap` names what the prefill blocks lack and such a model is
prefilled token by token (`prefill_sequential`); the blocks refuse those layers by name. Still on the
gap list: MLA, post-attention norm (Gemma 2/3), V-norm / V-from-K (Gemma 4), CPU-routed DeltaNet.
Left the list, each with a fixture that has it and nothing else: the embedding scale, the attention
output gate, GPU-resident DeltaNet.

**Chunked DeltaNet prefill.** Of a DeltaNet block's five dispatches, three were never sequential (the
stacked projection, the output projection, the FFN). The two that carry state take the token loop
INSIDE the kernel — `DELTANET_CONV_T`, `DELTANET_STEP_T` — where it is bit-identical to running the
single-token kernels T times (a channel's conv ring and a head's state rows are thread-private; the
loop bound is a uniform, so the barriers stay legal). Per prompt token, hybrid fixture,
quantized-resident as the app loads:

| route | dispatches | submits | polls |
|---|---|---|---|
| sequential, `decode_step` per token (what came back first) | 30.00 | 2.00 | 1.00 |
| sequential, layers only, 8-token windows (`LOGSCOPE_PREFILL_SEQ_LOGITS=1` restores the row above) | 28.06 | 1.03 | 0.12 |
| chunked, one chunk | 1.33 | 0.09 | 0.06 |
| chunked, chunks of 7 | 6.42 | 0.33 | 0.18 |

Chunked agrees with sequential to ~4e-7 on a logit scale of 1.8 — on the prompt's last token AND on
the next decode step, which reads the state and ring the prefill left — including across five chunk
boundaries with a ragged last chunk. `LOGSCOPE_PREFILL_SEQUENTIAL=1` forces the token-by-token route
for any model: the A/B lever for real hardware, and the first thing to flip if a prompt reads wrong.

The sequential route's layers-only form matters for the models still on the gap list: it skips the
final norm, the LM head (248,320 x 4096 on Qwen3.5 — about a billion weights) and the logits readback
for every prompt token but the last. The submits that remain are `flush_every`'s (SUBMIT_BY_PASSES),
by design.

**A fixture lesson worth keeping.** `main.rs`'s usual weight generator is `u/2^23 - 1`, i.e. [-amp, 0):
every weight negative, so every token embeds to nearly the same vector. Two break tests passed on it
that should not have — a chunked kernel reading TOKEN 0's projection, or token 0's decay/beta, for
every token in the chunk. Anything per-token needs `write_gguf_qwen35moe_hybrid_full(.., centered)`.

**Not done:** real-hardware numbers for any of this; MLA / Gemma prefill variants (each is one entry
to remove from `prefill_gap`, with a fixture); speculative verify on hybrids (recurrent state cannot
roll back — refused by name).

## The CPU side of a token (2026-09)

`sample()` — default temperature 0.8, top_k 40 — fully sorted the vocabulary every token: 10.2 ms at
Qwen3.5's 248,320 entries, -O, x86-64. `topk.rs` is an exact O(n) threshold scan (0.11 ms, same draws
for the same rng). The idiom used there and in `blockquant.rs` (load-time quantizer, 2.7x): a
reduction the vectorizer will not take — an early-exit `any`, a max that carries its index, a float
max (`llvm.maxnum` is not reassociated without fast-math, on ANY architecture) — becomes a
fixed-width fold with no exit plus a scalar body for the rare hit. `cpumatvec`'s Q4_K dot was starved
by scalar bookkeeping, not SIMD-bound (2x from a straight-line scale unpack). Its int8 dots are now
runtime-dispatched: AVX2 on x86 (on, tested), `dotprod` on AArch64 behind the `arm-dotprod` feature —
which has NEVER BEEN COMPILED (no AArch64 std in the sandbox that wrote it). The logscope build sets
no AArch64 rustflags at all, so without that feature the dots build for ARMv8.0.
