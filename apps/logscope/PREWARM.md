# PREWARM.md — precaching the stack for 10–100 instances on one big box

Goal: on a 192 GB machine, run 10–100 concurrent "instances" of the same model
stack with near-instant spin-up, by doing the expensive work **once** and sharing
it. The key reframe: an "instance" is not a process and not a second copy of the
weights — it's a *session* (a KV cache + position) sharing one resident model, one
device, one pipeline cache, and (optionally) one prefilled prompt prefix.

## What's expensive, and what to cache once

| step | cost (1B on M2) | per instance? | precache strategy |
|---|---|---|---|
| read GGUF + dequant to resident weights | ~5.1 s | **no** — share | load once → `Arc<Model>`; all sessions borrow it (K9: weights are read-only) |
| compile WGSL pipelines | first-dispatch latency ×19 | **no** — share | one `Gpu` (device+queue) → one pipeline cache; a prewarm forward compiles all 19 before serving |
| prefill the shared prompt prefix (system + tool schemas) | grows with prefix length | **no** if identical | `prefill_prefix` once → seed every session with `seed_from_prefix` (a KV copy, not a recompute) |
| allocate a session's KV cache | ~ms | **yes** | the only real per-instance cost; int8 KV makes it 4× smaller |
| decode | the steady-state work | yes | batched across active sessions by `run_batched` (continuous batching) |

The first three are paid once; spinning up instance N+1 after warm-up is just a KV
cache allocation. That's the "precaching all these steps" win.

## The prewarm sequence (once per process)

```
1. gpu   = Gpu::new()                          // one device for all instances
2. model = Arc::new(load_gguf_quantized(gguf)) // weights resident once, read-only
3. prewarm: run one 1-token forward            // compiles all 19 pipelines now,
            (decode_step on a throwaway cache)  // not lazily mid-request
4. prefix = prefill_prefix(model, shared_prompt_tokens)  // if a shared prefix exists
   (cache `prefix` + its length; reuse for every session)
```

## Spinning up an instance (cheap, repeatable)

```
session = session_q8_new(model, max_seq, seed)   // int8 KV: 4× smaller
seed_from_prefix(model, &mut session, &prefix, prefix_len)  // skip prefix recompute
// then prefill only the per-instance suffix and decode
```

Or, for a fan-out of requests, skip manual session management and hand the whole
queue to the scheduler: `run_batched_q8(model, queue, sampler, eos, max_batch, max_seq)`
admits/decodes/evicts/backfills automatically. `max_batch` is the concurrency knob.

## Memory math (192 GB, 1B model, int8 KV)

- Weights, **once**: ~0.7 GB int8 resident + ~1.0 GB f32 embeddings ≈ **1.7 GB**.
- KV per session: `n_layers·2·kv_dim·1 byte/token` = 16·2·512 = **16 KB/token**.
  - 8k context → 128 MB/session → 100 sessions = **12.8 GB**.
  - 32k context → 512 MB/session → 100 sessions = **51 GB**.
- So 100 instances at 32k context ≈ 1.7 + 51 ≈ **53 GB** — fits in 192 GB with room
  to spare. With f32 KV that KV figure is 4× (204 GB) and would *not* fit, so int8 KV
  (K8) is what makes 100 long-context instances viable. **The cap is decode compute,
  not memory** — `max_batch` should be tuned to where aggregate tok/s saturates on the
  box, not to the memory ceiling.

For several *different* models on the box: weights aren't shared, so it's
`192 GB / (per-model resident)` — ~90× the 1B at ~2 GB each. Same-stack (one model,
many sessions) is the cheap case and the intended one.

## What's already built vs. the gaps

**Built and validated** (see RULESET.md, all bit-exact unless noted):
`load_gguf_quantized` (weights resident, int8) · pipeline cache · `prefill_batched`
(batched prefill) · `prefill_prefix` + `seed_from_prefix` (prefix reuse) ·
`decode_batched` (+`_q8`) · `run_batched` (+`_q8`) the continuous-batching scheduler ·
int8 KV cache (`QKvCache`) end to end · loader proven on the real Llama-3.2-1B.

**Gaps for the prewarm worker** (small, well-scoped):
1. **Share the `Model`**: wrap it in `Arc` and have `run_batched`/sessions borrow it,
   so N instances reference one resident copy. The GPU buffers are already shareable;
   this is an ownership change in `backend.rs::Engine`, not new kernels.
2. **Explicit prewarm pass**: a `warm(model)` that runs one tiny `decode_step` to force
   all pipelines to compile before the first real request (avoids first-request stalls).
3. **int8 prefix reuse**: a `prefill_prefix_q8` + `seed_from_prefix_q8` so the int8
   serving loop also gets the shared-prefix win (mirror of the f32 ones).
4. **Persistent pipeline cache** (optional): wgpu `PipelineCache` serialized to disk, so
   a process *restart* skips recompilation too — only matters if instances are separate
   processes rather than sessions in one process (the latter is preferred).

## Harness handoff (matches the game-engine kit)

- Sandbox green gate: `cd crates/gpu-gemm && cargo run --release --bin kernel-validate`
  must print `ALL KERNELS VALIDATED ON GPU ✓` before shipping (the analog of
  `harness/run.sh`). Every rule in RULESET.md has a check there.
- Integration: `cargo r --release --bin logscope-egui --features wgpu-engine`, load the
  GGUF, watch the `wgpu-engine:` logs (adapter, load time, prefill, tok/s, pipelines).
- When a new break class appears on the box, add a numbered K-rule + a `kernel-validate`
  check, same as the game engine's loop.

## Kernel cache vs kernel fusion (the unendlich question)

Two different levers that get conflated. They win in different places.

**Pipeline/kernel cache = a SPIN-UP win, not a per-token win.** This is the
unendlich `.spv`-cache idea lifted to wgpu's level: `Device::create_pipeline_cache`
(needs the `PIPELINE_CACHE` adapter feature), pass the handle into every
`ComputePipelineDescriptor.cache`, then persist `PipelineCache::get_data()` to disk on
shutdown. On the next *process* start the 20 pipelines hydrate from cache instead of
recompiling WGSL->SPIR-V->native. It shaves cold start; it does nothing for decode
throughput.

Critically: for the architecture PREWARM.md recommends — ONE process, ONE `Gpu`,
`Arc<Model>`, many `Session`s — the kernel cache is *already effectively done* in memory.
`Gpu.pipelines: RefCell<HashMap<(shader_ptr,n), Rc<...>>>` compiles each kernel once and
hands the same `Rc` to every session and every dispatch. The persistent disk cache only
earns its keep if you fan workers out across SEPARATE processes; inside one process the
in-memory map already amortizes compilation across the whole fleet. So enable the disk
cache only when (a) the adapter advertises `PIPELINE_CACHE` and (b) workers are
multi-process. Otherwise skip it — it's plumbing with no payoff for the single-process
fleet.

**Kernel fusion = the actual per-token / per-batch win.** Fewer dispatches per token is
fewer launch + barrier round-trips, which is exactly what's left to claw back on
dispatch-bound backends (lavapipe today, and AGX feels it too at small batch). Done:
SwiGLU folds `silu(gate)*up` into one kernel — the FFN dropped from 5 dispatches
(gate, up, silu, mul, down) to 4, validated bit-exact (`fused swiglu == silu*mul` = 0).
Remaining fusion candidates, in rough order of payoff:
- **gate+up as one matmul** -> DONE: load-time concatenation builds one `[2h, d]` weight,
  the projection emits `[t, 2h]`, and SWIGLU_PACKED reads the halves. One matmul + one
  weight-bind per layer instead of two; validated bit-exact (`packed swiglu == split swiglu`
  = 0) across f32, int8, and Q4_0. With the earlier silu+mul fusion the FFN activation+
  projection path is now 3 dispatches/layer (wgate_up, swiglu_packed, wdown) down from 5.
- **fused residual add** -> fold the `x = x + attn_out` / `x = x + ffn_out` adds into the
  tail of the producing kernel (wo / wdown) instead of a standalone ADD.
- **rmsnorm -> matmul** -> normalize inside the load path of the following LINEAR so the
  norm never round-trips to global memory.

Rule of thumb: reach for the cache when the complaint is "spin-up is slow," reach for
fusion when the complaint is "tokens/sec is low."
