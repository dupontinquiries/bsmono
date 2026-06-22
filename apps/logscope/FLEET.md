# FLEET.md — the worker-fleet problem space

How to run many small workers against one model on one box, and where the cache lives
at each level. This is the map; the code primitives are noted inline (`gpu_kernels`).

## What a worker is

A worker is **not** a copy of the model. It's a `Session`: a KV cache, a stream
position, and an RNG, pointing at the *shared* resident weights. The weights load once;
spinning up worker number 100 costs one KV cache, not one reload. So "100 workers" is a
memory question about KV caches, and a throughput question about decode compute — never
about 100× the weights.

```
            ┌─────────────────────────── one process ───────────────────────────┐
            │  one Gpu (device+queue, 20 hot pipelines)                          │
            │  Arc<Model>  ── weights, resident once (~1.7 GB for 1B Q4) ────────│
            │     ▲        ▲        ▲                                            │
            │  Session   Session  Session   … each = just a KV cache + pos + rng │
            └────────────────────────────────────────────────────────────────────┘
```

## The cache hierarchy (three tiers, propagated outward)

Caching here is layered. Each tier is computed once and shared by everything below it.

1. **Weights** — load once into `Arc<Model>`, read-only, shared by every worker.
   Propagation = cheap `Arc::clone` (a refcount bump). Never recomputed, never copied.
2. **Pipelines** — `Gpu.pipelines` compiles each of the ~20 kernels once and hands the
   same `Rc` to every dispatch of every worker. `Gpu::warm(&model)` forces all of them
   hot up front (one throwaway prefill+decode) so the first real worker pays compute, not
   shader compilation. This is the in-memory "kernel cache" — already shared across the
   whole single-process fleet; the persistent disk version only matters across *separate*
   processes (see PREWARM.md).
3. **Shared prefix KV** — when many workers begin with the same tokens (the file under
   repair, a system prompt, a tool schema), that prefix's K/V is identical for all of
   them. Prefill it **once** (`prefill_prefix` → `KvCache`) and **propagate** it into
   each worker (`seed_from_prefix`, a buffer copy, not a forward). Per worker we then only
   prefill its unique suffix.

Below the shared prefix, each worker owns its **suffix + generated** KV — genuinely
unique, not shareable.

## Cache propagation: "ten workers on one file, those first"

This is the scheduling rule. Group workers by their shared prefix; process the **largest
group first** so the most-reused cache is computed and propagated before anything else.
Ten workers on one file ⇒ that file's forward pass happens exactly once, and the result
seeds all ten. Under any throughput budget, the highest-payoff propagation lands first.

```
PrefixGroup { prefix: <file A tokens>, workers: [(id, suffix, max_new), …] }
            │
   run_fleet sorts groups by worker-count desc, then per group:
            │  prefill_prefix(file A)         ← one forward for the whole group
            │  run_batched(prefix=(cacheA,len), workers)
            │     └ seed_from_prefix into each worker (copy), prefill only the suffix,
            │       then continuous-batch the decode (backfill as workers finish)
```

API: `Gpu::run_fleet(&model, Vec<PrefixGroup>, sampler, eos, max_batch, max_seq)`.
Validated bit-exact against per-worker standalone generation
(`fleet (grouped prefix propagation) == per-worker` = 0).

### Reuse math (why grouping wins)

For a group of `W` workers sharing a `P`-token prefix, naive per-worker prefill does
`W·P` prefix-token forwards. Grouped does `P` forwards + `W` cache copies. Copy is
bandwidth, forward is compute; the bigger `P` and `W`, the larger the gap. On lavapipe
this measured ~2.2× at W=3 and grows with both W and P.

## Where the walls are

- **Decode compute, not memory, is the cap.** Weights are shared and KV is cheap per
  token; the thing that doesn't scale for free is the per-token matmuls across active
  workers. Levers: bigger `max_batch` (amortize linears across workers via
  `decode_batched`), and **kernel fusion** (fewer dispatches/token — SwiGLU done, gate+up
  matmul and fused residual adds next; see PREWARM.md).
- **KV memory is the context wall.** f32 KV is ~16 KB/token/worker at this width; 100
  workers × 32k ctx would be ~200 GB. int8 KV (`QKvCache`, `run_batched_q8`) is ~4× smaller
  and tracks f32 closely (argmax preserved), which is what makes the high-worker / long-
  context corner fit at all.

## Open problems (the next layer of the map)

- **Partial / nested prefix sharing.** DONE: `run_fleet_trie` takes ungrouped
  `FleetRequest`s, builds a radix trie over their token sequences, and caches every
  shared boundary once — each non-root segment is built by `extend_prefix` (a batched
  prefill-extend that attends over the parent cache, `O(s·(base+s))`) rather than
  recomputing the parent. A worker runs against the deepest cache it shares with any other
  worker and prefills only its unique tail; a worker that shares nothing falls back to a
  plain prefill. So workers on the same file → same function → diverging edits propagate
  cache at every level, not just the root. Validated bit-exact (`fleet trie (nested
  prefix) == per-worker` = 0, `batched extend_prefix == full prefill` = 0).
- **Prefix cache eviction.** DONE: `PrefixStore` is a size-bounded, value-ranked cache of
  prefix KV. Value is GDSF-style — prefill cost is ~O(len²) and memory is O(len), so
  value-per-byte ∝ (hits+1)·len, with recency as the tiebreak. Under pressure the lowest-
  value entries first *demote* f32→int8 (the warm tier, ~4x smaller, via `demote_cache`/
  `promote_cache` ↔ `quantize_rows`/`dequant_rows`) and only drop if still over budget — a
  two-layer compression rather than a hard evict. Validated: the eviction policy keeps the
  small-frequent prefix hot while big-unused ones demote (`smart cache ranked two-tier
  eviction` = 0), and the int8↔f32 roundtrip stays within int8 error (`quantize->dequant`
  = 3.8e-3). Next: wire `PrefixStore` into `run_fleet_trie` so trie nodes are the cache
  keys and promotion-on-hit is automatic.
- **Threaded fan-out.** DONE: `Gpu`'s pipeline cache moved from `Rc`/`RefCell` to
  `Arc`/`Mutex`, so `Gpu` and `Model` are both `Send`/`Sync`. Workers can now fan across
  OS threads sharing one `Gpu` + `Model` by reference; validated that two threaded workers
  produce byte-identical output to the sequential path (`threaded workers == sequential`
  = 0).
- **Per-worker priority.** `run_fleet` orders by reuse; a real fleet also wants
  deadline/priority ordering *within* the budget. (Still open.)

## Quick start for a fleet worker

```rust
let model = std::sync::Arc::new(gpu.load_gguf(&bytes)?); // weights, once
gpu.warm(&model);                                        // pipelines hot, once
let out = gpu.run_fleet(&model, groups, &sampler, eos, max_batch, max_seq);
// groups: workers bucketed by shared prefix; largest bucket propagates first.
```

Or skip the manual bucketing and let the trie find the shared structure (nested):

```rust
// requests: Vec<FleetRequest { id, tokens, max_new }> — full prompts, ungrouped.
// min_share = 2: cache any prefix shared by ≥2 workers, at every depth.
let out = gpu.run_fleet_trie(&model, requests, &sampler, eos, max_batch, max_seq, 2);
```
