# Concurrency & scaling plan — wgpu engine

## What exists now

- A `Session` abstraction in `gpu_kernels` (`session_new` / `session_prefill` /
  `session_step`). A session is just its own KV cache + stream position + RNG; it
  shares the model's resident weights. The per-session GPU cost is the KV cache only.
- A sweep harness: `kernel-validate --concurrency`. It stands up a DialoGPT-small-
  shaped synthetic model (d=768, L=12, H=12, ff=3072, vocab=50257, int8-resident —
  random weights, since this measures throughput, not text) and runs N sessions
  round-robin, reporting aggregate tok/s.

## Measured baseline (lavapipe, in-sandbox)

```
   N    tokens    wall(s)    agg tok/s    per-sess   speedup
   1         4       2.45         1.63        1.63     1.00x
   2         8       4.93         1.62        0.81     1.00x
   4        16       9.84         1.63        0.41     1.00x
   8        32      19.58         1.63        0.20     1.00x
```

Aggregate throughput is flat; per-session falls as 1/N. **Round-robin scheduling of
independent sessions buys nothing** — N sessions are N separate single-row decode
passes, so it's N× the work for the same aggregate rate. This is the number to beat.

(lavapipe is a single-CPU software rasterizer, so the work is strictly serialized and
the flatness is exact. On the M2's AGX GPU a single-row decode leaves most cores idle,
so the *opportunity* for batching is larger there than this bound suggests — but the
shape of the result, no scaling from independent sessions, is the same.)

## Why it doesn't scale, and the lever

Each `session_step` runs `decode_step`: a chain of `[1, d]` matmuls (q/k/v/o
projections, the FFN, and the big `[1, 768]·[768, 50257]` output projection). A
one-row matmul reads the entire weight matrix to produce one row of output — it is
bandwidth-bound and leaves the GPU's lanes mostly empty. Doing this N times reads
every weight N times.

The lever is **batched (continuous-batching) decode**: gather the current token from
each of the N active sessions into one `[N, d]` activation and run the *shared* parts
as `[N, d]` matmuls. The `LINEAR` / `LINEAR_Q8_0` kernels already process `m` rows, so
this is one dispatch that reads each weight **once** for all N rows. On a real GPU
`[N, d]` is nearly as cheap as `[1, d]` until the matmul becomes compute-bound, so
aggregate throughput scales ~linearly with N up to a saturation knee.

## Batched-decode design

1. **Batch the linears.** Stack the N sessions' post-norm activations into `[N, d]`;
   call `wq/wk/wv/wo` and `wgate/wup/wdown` with `m = N` (already supported). The
   output projection `[N, d]·[d, vocab]` is the single biggest win — it's the largest
   matmul and currently runs N separate times.
2. **Per-session attention.** Each session attends its own KV cache at its own
   position, so attention can't be a single packed call. Either (a) loop the attention
   sub-step over the N sessions (cheap relative to the linears), or (b) add a batched
   decode-attention kernel keyed by per-session `(pos, len)`. Start with (a).
3. **Per-row RoPE / cache writes.** Apply RoPE per row at each session's position and
   write each session's new k/v row into its own cache (the write_row path already
   exists; do it per session inside the batched step).
4. **Scheduler.** Keep the batch full: admit waiting sessions, evict on eos/length,
   backfill. Mixed prefill+decode in one batch (true continuous batching) is the
   later refinement; first land fixed-N batched decode.

## Expected scaling and what to measure on the M2

- **Single-session decode tok/s** at this shape (baseline). For reference, the 1.24B
  Llama gave 4.8 tok/s; a 117M shape should be much faster.
- **Batched aggregate tok/s vs N** (N = 1, 2, 4, 8, 16, 32). The headline is the N=8
  aggregate ÷ the N=1 rate. On AGX I'd expect a clear super-1× curve until it saturates;
  the saturation knee is where to cap concurrency.
- **KV-cache memory ceiling.** At full 1024 context this shape is ~72 MiB/session
  (`L·2·seq·kv_dim·4`); at 256 ctx ~18 MiB. With 32 GB free, memory allows hundreds of
  sessions, so the cap will be set by compute saturation, not memory — confirm on-device.

## Caveats

- **Architecture.** DialoGPT is GPT-2 (LayerNorm+bias, learned absolute position
  embeddings, GELU non-gated FFN, fused QKV, Q2_K/Q3_K). Producing correct *text* from
  that file needs a GPT-2 port + Q2_K/Q3_K dequant. The scaling work above is
  architecture-agnostic and is validated here on a same-shape synthetic model; the
  per-token GPU workload (matmul sizes, KV layout) is what determines scaling, and that
  matches the real model.
- **Prefill** is a separate, related lever: batch a prompt's tokens into one `[T, d]`
  pass instead of T sequential steps (the same `m`-row trick). It compounds with
  batched decode for multi-session serving.

## Update: batched prefill landed
`prefill_batched` runs the whole prompt through every layer in one `[T,d]` pass,
filling the KV cache (via `block_prefill` + `write_rows`), and projects only the
last row to logits. It replaces the T sequential `decode_step`s inside
`generate_kv_stream`. Validated bit-exact against the sequential path
(`batched prefill == sequential` = 0.000e0; the end-to-end `kv-cache stream ==
generate_kv` check also passes since the stream now prefills batched).

Measured on lavapipe (a floor — it's compute-bound and serialized, so this is
the *minimum* win):

```
  prompt  32 tok: sequential 19.52s | batched 13.32s | 1.47x
  prompt  96 tok: sequential 58.68s | batched 39.30s | 1.49x
```

On AGX the `[T,d]` matmuls use parallelism single-row decode can't, so expect a
larger factor there. As a bonus this also makes Stop responsive: a long prompt is
now one fast pass instead of an uninterruptible T-step loop.

Next levers, in order: (1) **batched/continuous decode** — many sessions' tokens in
one `[N,d]` pass (the multi-instance throughput win, and the verify step for
speculative decoding); (2) **prefix KV reuse** — compute the shared prompt prefix
(system + tool schemas) once and share its cache across sessions; (3) **KV
compression** — int8 KV cache + prefix dedup to raise the concurrency ceiling.

## Update: batched decode landed + loader validated on the real model
**Loader validated against the real Llama-3.2-1B-Instruct Q4_K_M** (via
`kernel-validate --inspect <file>`): our gguf.rs parses it (arch=llama, d=2048,
16 layers, 32/8 GQA, ff=8192, rope_base=500000, 147 tensors), the byte-level BPE
tokenizes `"Hello, world!"` -> `[9906, 11, 1917, 0]` and round-trips, `<|eot_id|>`=128009
matches, and Q4_K/Q6_K dequant on the real tensors yields finite, sane-range weights.
(Full resident load stays M2-only — int8 weights + f32 embeddings exceed sandbox RAM.)

**`decode_batched`** advances N sessions one token each in a single pass: the shared
linears (q/k/v/o, FFN, output) run as `[N,*]` matmuls (each weight read once for all
N); RoPE uses per-row positions (`ROPE_MH_ROWS`); cache writes and attention are
per-session. Validated bit-exact: `batched decode == per-session` = 0.000e0 (three
sessions stepped together == each stepped alone).

Throughput on lavapipe (a floor — compute-bound and serialized):

```
   N   agg tok/s   vs round-robin
   1      1.66        1.02x
   2      1.74        1.06x
   4      1.79        1.09x
   8      1.82        1.12x
```

The win grows with N (more overhead amortized) even where it's compute-bound; on AGX
the `[N,d]` matmuls fill lanes single-row decode leaves idle, so expect a much steeper
curve. `decode_batched` is also the verify step for speculative decoding.

Still ahead: a continuous-batching scheduler (admit/evict, mixed prefill+decode),
prefix KV reuse, and int8 KV compression.

## Update: prefix KV reuse landed
`prefill_prefix` computes a shared prompt prefix's KV cache once; `seed_from_prefix`
copies it into a session's cache (one buffer copy per layer) and sets the position,
so the session prefills only its unique suffix. Validated bit-exact:
`prefix KV reuse == full prefill` = 0.000e0.

Measured on lavapipe (3 sessions, prefix 32 + suffix 4 tok): re-prefilling the whole
prompt per session 44.52s vs prefix-once + suffix 20.21s = **2.20x**. The factor grows
with the session count (prefix computed once regardless of N) and the prefix length —
for a fan-out of hundreds of sessions sharing a long tool-schema prefix it approaches
prefilling the schema once instead of N times. This is a copy (bandwidth) replacing a
forward (compute); true zero-copy sharing via paged attention is the later refinement.

See LONG_CONTEXT.md for how the same KV-compression/paging track also unlocks long
context (the position side is a metadata-driven RoPE-factor computation reusing the
existing ff[] kernel; the cost side is the KV memory + prefill compute we're building).

## Update: continuous-batching scheduler landed
`run_batched` is the serving loop that ties the primitives together: it admits
requests from a queue into up to `max_batch` slots (prefilling each, reusing an
optional shared prefix via `seed_from_prefix`), decodes all active sessions together
with `decode_batched`, evicts finished ones (eos / max_new), and backfills freed
slots from the queue without waiting for the rest of the batch — i.e. continuous
batching. Greedy output is identical to per-request `generate_kv`.

Validated bit-exact: `continuous batching == per-request` = 0.000e0 (with max_batch=2
< 3 requests, forcing admit/evict/backfill) and `continuous batching + prefix reuse`
= 0.000e0 (the full composition: shared-prefix reuse + continuous batching + batched
decode all agree with the sequential single-session path).

This is the multi-session entry point to point a fan-out at: queue N requests sharing
a tool-schema prefix, get N completions back, with the prefix prefilled once and the
decode steps batched. Remaining levers: int8 KV compression (memory ceiling, shared
with long context) and chunked/flash prefill.

## Update: int8 KV cache compression landed
`QKvCache` stores K/V per-row as packed int8 (4 values per u32) + one f32 scale per
row. New kernels: `ROW_ABSMAX` + `QUANT_PACK` (quantize/pack on write, the
`quantize_rows` method) and `DECODE_ATTN_MH_Q8` (GQA decode attention that unpacks +
dequantizes int8 K/V on the fly, so the cache never has to be materialized back to
f32). `decode_step_q8` / `forward_kv_q8` are the int8-cache decode path.

Validated: vs the f32 cache it's lossy but close — rel logit error 2.1e-2, and the
greedy top token is preserved (argmax matches even on random weights). Size: ~4x
smaller for real models — Llama-3.2-1B kv_dim 512 is 2048 -> 516 bytes/row (3.97x);
the validator's tiny synthetic kv_dim shows 3.2x because the per-row scale overhead
is proportionally larger there.

Impact: 4x more sessions fit, or 4x longer context, for the same KV memory (see
LONG_CONTEXT.md — at 1M tokens a 1B model's f32 KV is 32 GB; int8 brings it to ~16 GB,
int4 would reach ~8 GB). Next: make `Session`/`decode_batched`/`run_batched` generic
over the cache type so the serving loop uses int8 KV by default (currently the int8
path is validated single-session; batched integration is the remaining plumbing).

## Update: int8 KV wired through the serving loop
The int8 cache is now first-class in the batched/serving path, not just single
session: `SessionQ8` (int8-cache session), `decode_batched_q8` (batched decode that
quantizes this layer's K/V once for the whole batch and scatters it per session,
attention dequantizing on the fly), and `run_batched_q8` (the continuous-batching
scheduler on int8 sessions). `generate_kv_q8` is the per-request int8 greedy reference.

Both validated bit-exact (batching is exact; int8 is the only approximation and it's
applied identically on both sides): `int8 batched decode == per-session` = 0.000e0 and
`int8 scheduler == per-request int8` = 0.000e0. Combined with the primitive-level
result (int8 tracks f32 at 2.1% logit error, greedy token preserved), the whole serving
loop now runs on 4x-smaller KV — i.e. ~4x more concurrent sessions or ~4x longer
context for the same memory, end to end.

Validated stack now: batched prefill, prefix KV reuse, batched decode, continuous-
batching scheduler, int8 KV — each in both f32 and (for the decode/serving path) int8.
Remaining: int8 prefix reuse (a q8 seed/prefill variant), paged KV, chunked/flash
prefill, and RoPE-scaling-from-metadata for long context.
