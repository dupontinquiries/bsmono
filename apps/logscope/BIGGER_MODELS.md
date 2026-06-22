# Bigger-model support — design scratchwork (2026-06-12)

Groundwork while the M2 is unavailable. Pure, tested pieces are in `crates/gpu-gemm/src/memplan.rs`
and `archcfg.rs`; everything that touches the GPU hot path is flagged as **M2-gated** and not started.

## Where the engine is today
- **Architecture: llama-family only.** `load_gguf_opt` hard-refuses any `general.architecture != "llama"`.
  Quant dequant is already broad (F32, F16, Q4_0/1, Q5_0/1, Q8_0, Q4_K, Q5_K, Q6_K) — *quant coverage
  is not the blocker.*
- **Memory: materialize-then-resident, no planner.** Every tensor is dequantized to f32, then the
  projections are kept **int8-resident** (~1 B/param); the embedding stays f32 (lookup + tied output).
  There is no up-front fit check, so an oversized model fails late — the "70B refused" case.

## The three axes (and which actually bite)
1. **Memory** — the real ceiling on a 39.5 GiB M2. int8-resident caps out around ~32B; 70B needs q4 or
   streaming. *Biggest lever.*
2. **Architecture** — gates *which* bigger models load at all. Qwen2 (0.5–72B) is one bias add away from
   llama; that's the highest-value unlock. Gemma2/Phi-3 need more.
3. **Quant** — already handled for loading. (A *q4-resident* path is a memory lever, not a coverage one.)

## What the planner says (computed by `memplan`, real output)
```
model          params   int8 GiB    q4 GiB     kv@8k   M2 plan (39.5 GiB)
SmolLM2-360M     0.4B       0.5       0.3       0.31    Resident int8
Llama-3.2-1B     1.2B       1.9       1.5       0.25    Resident int8
Llama-3.1-8B     8.0B       8.9       5.9       1.00    Resident int8
Qwen2-14B       14.8B      15.9      10.2       1.50    Resident int8
Qwen2-32B       32.8B      32.7      19.7       2.00    Resident int8
Llama-70B       70.6B      68.6      40.3       2.50    Streaming (layers from disk)
```
Takeaways: **up to ~32B already fits int8** on the M2 (the engine just needs to *try* — the refusal is
the only thing stopping a 14B/32B llama-family model today). 70B needs q4-resident (~40 GiB, borderline)
or layer streaming. SmolLM2's 0.5 GiB matches the real M2 log (calibration anchor).

## Delivered scratchwork (pure, tested in-sandbox)
- **`memplan.rs`** — `ModelShape` → param counts, `resident_weight_bytes(fmt)`, `kv_cache_bytes(ctx)`,
  `peak_stage_bytes`, and `plan(budget, ctx) -> Fit` = `Resident{fmt} | Streaming | TooBig{min}`. Prefers
  the best quality that fits (int8 → q4 → stream → refuse-with-a-number). 6/6 tests (param counts for
  360M/8B/70B; int8 footprint matches the M2 log; 8B fits int8; 70B does NOT fit int8; kv scales with ctx).
- **`archcfg.rs`** — `for_arch(str) -> ArchConfig` registry recording, per arch, the metadata key prefix,
  QKV bias, norm/activation variant, RoPE style, logit/attn soft-cap, tied-embed default, and a
  `supported` flag + a `note` saying exactly what's missing. Plus `dim_keys` (generalizes the hardcoded
  `llama.*` keys) and `qkv_bias_tensors`. Covers llama (supported), qwen2/gemma2/phi3 (scaffolded). 5/5 tests.

## Recommended incremental path
1. **✅ DONE — `memplan` wired into the loader.** Pre-flight `plan(...)` runs on metadata (no dequant) and
   replaces the silent 70B fail with a graded message + the GiB figure. M2-green via `--gate-memplan`.
2. **✅ DONE — refusal gated on fit, not size.** The loader guard only refuses when the resident estimate
   exceeds budget, so an in-budget 14B/32B llama-family model loads (same kernels). (M2: confirm a 14B.)
3. **✅ DONE (correctness) — Qwen2.** The loader reads `qwen2.*` metadata by prefix (incl. its RoPE θ),
   the refusal is lifted for `qwen2`, and the Q/K/V bias is loaded (`blk.N.attn_{q,k,v}.bias`) and applied
   inside `Lin::apply` via a new broadcast `ADD_BIAS` kernel — so every forward path (prefill/decode/
   batched) gets it for free. `--gate-qwen2` validates in-sandbox: the bias-add kernel is bit-exact; a
   qwen2 GGUF loads; zero-bias qwen2 == the llama path; nonzero bias changes the logits; int8-resident
   qwen2 matches f32; unsupported arches still refuse. **M2-gated remainder:** bit-exact vs a real
   Qwen2-0.5B/1.5B (converter quirks, real weights) — the synthetic gate can't catch those.
4. **Memory reduction for 70B (M2-gated).** Two levers; the planner says *when* each is needed.
   - **✅ DONE (correctness) — q4-resident matmul.** A 4-bit resident weight (`Q4Weight`: per-32-block
     f32 scale + symmetric nibbles, 8× smaller than f32, 2× smaller than int8) with two new kernels
     (`MATVEC_Q4` for decode, `LINEAR_Q4` for prefill) wired through `Lin::Q4` + the int8-cache
     serialization (tag 2). Opt-in via `LOGSCOPE_Q4_RESIDENT=1`; the planner already computed the q4
     footprints, so a borderline model that won't fit int8 can now fit. `--gate-q4resident` validates
     in-sandbox: both kernels bit-exact vs a CPU dequant+matmul, q4 lossy-but-bounded, 70B int8 68.6 GiB
     → q4 40.3 GiB, and a full model loads q4-resident through the loader tracking f32 (cos ≈ 0.91).
     **M2-gated remainder:** the speed of the q4 mat-vec on AGX (it's a memory-bound decode kernel —
     correctness is proven, throughput is hardware-shaped), and whether 40 GiB actually fits the 39.5 GiB
     budget with KV + compute headroom (borderline — may need a smaller ctx).
   - **Layer streaming from mmap** (still TODO, heavier) — the `Streaming` plan for models too big even
     at q4: keep a working set of layers resident, page the rest. Real mmap/IO timing + decode-loop
     integration; correctness *and* perf are hardware-shaped, so it's genuinely M2-gated.
5. **Gemma2 / Phi-3 (later).** Gemma2 = geglu + (1+w) norm + tanh soft-cap on attn & logits; Phi-3 = fused
   QKV + partial-rotary. Both noted in `archcfg`; both are multi-kernel and M2-gated. (The `ADD_BIAS`
   kernel + the `Lin`-carries-bias plumbing from step 3 are reusable building blocks.)

## What needs the M2 (explicitly not started)
- Any **new kernel** (qkv bias, geglu, soft-cap, partial-rotary, q4-resident matmul) — must be checked
  bit-exact against a reference on the real adapter, like the existing engine gates.
- **Layer streaming** — depends on real mmap/IO timing and the decode loop; correctness *and* perf are
  hardware-shaped.
- Validation models to fetch on the M2: Qwen2-0.5B/1.5B (quick bit-exact), Gemma2-2B, a 14B llama-family
  (confirm the in-budget path), and a 70B (confirm the planner's Streaming/q4 verdict end-to-end).

## Suggested next gate
`--gate-memplan`: assert the planner's verdicts for a table of known shapes against measured resident
footprints on the M2 (extend the SmolLM2 anchor to 1B/8B), so the estimate stays honest as the loader
evolves.
