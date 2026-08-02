# Decode is 14× slower per token than prefill, on the same weights, in the same run

DeepSeek-8B Q4_K, M2 Ultra, one session:

```
prefill  1634 tokens in 20.51 s  = 12.6 ms/token
decode    109 tokens in 19.90 s  = 182  ms/token
```

Both read the same ~2.6 GiB of weights per token of output. **Prefill is 14× faster per token.**

This is the wrong way round for an engine whose decode is supposed to be bandwidth-bound: decode reads
the weights *once* per token, and prefill reads them once per *chunk* while doing far more arithmetic.
Decode should be the cheaper of the two per token, not the dearer.

## What it rules out

**Not bandwidth.** The same bytes move in both. If the memory system were the limit, the two would be
within a small factor.

**Not the weights' format.** Same q4 residency, same tensors, same layout.

**Not dispatch count in the naive sense.** 15 submits/token with 100% bind-group reuse, setup 49 ms of
19860 ms. The host is not the bottleneck.

## What it points at

Decode runs at **m=1**: one row against the whole weight matrix, every layer, every token. Prefill runs
at **m=109** (auto-lowered for this model's d/ff), amortising each weight read across 109 rows.

So the 14× is close to the arithmetic intensity difference, which means **decode is not achieving the
bandwidth prefill achieves** — it is paying per-token latency that prefill hides behind a batch.

That is the signature the in-tree note already describes for AGX: a workgroup reserving a large
threadgroup-memory reduction buffer pins occupancy, and on a memory-*latency*-bound matvec occupancy is
what covers the latency. Prefill's larger m gives it enough work in flight to cover it anyway; decode
has nothing to hide behind.

## Why this is the same chain as everything else

`V2WgSg` — native `subgroupAdd`, minimal threadgroup memory, the ggml-metal "rows per SIMD group" shape
— is the kernel that would fix exactly this, and it is disabled on this driver because it produces
wrong results. `V3` (128-bit `vec4` loads, wired into the autotune in _415) attacks the same latency
from the other side and has not been measured on hardware yet.

So the ordering is unchanged and now has a number attached to it:

1. **`--gate-bytes`** — if `id_probe` shows the 4-of-16 pattern, the subgroup kernel is not "buggy on
   Asahi", it is a casualty of a dispatch-level fault that affects everything.
2. **`V3` in the autotune** — the next load re-searches with no V3 entries in the cache and the
   `decode tile = N (kernel ...)` line names the winner.
3. Only then a q4 prefill GEMM, which addresses the *other* 14× — prefill's own distance from its
   roofline.

## Correction to `prefix-cache-gap.md`

That document claimed the KV prefix cache never extends and every turn re-prefills the whole prompt.
**That is wrong.** This run shows `reused 1634 anchor tokens + prefilled 135 in 2.24s` — the extend
works, and the second turn did not re-prefill 1769 tokens.

The claim came from an older log in which every turn's prefill scaled with total length. Whether that
log predates the anchor reuse or came from a path that does not take it, I did not check before
writing three paragraphs of consequence. The measurement was real; the conclusion drawn from it was
not.
