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


---

# The batched decode kernels exist and decode never reaches them

The 14× says decode needs rows in flight. The engine already has the kernel for that:

```
m == 1        MATVEC_Q4          one row
m <= 8        MATVEC_SMALL_Q4    reads each nibble ONCE, accumulates all m rows
m >  8        LINEAR_Q4          strided
```

`MATVEC_SMALL_Q4`'s own comment names its purpose: *"small batch (speculative verify block / batched
decode): read each weight nibble once, accumulate all m rows — keeps Q4's halved weight-bandwidth
advantage."* Each row is bit-exact to the m=1 kernel.

**Decode passes m=1.** `Lin::apply(gpu, a, m)` routes by m and the decode loop produces one token at a
time, so the m≤8 branch is reached only by the speculative-verify path — and nothing drives it.

`windowed sampling ON (k=4)` is not this. The window is for sampling — a resident int8 embed table for
top-k — and does not batch the matvec.

## What this is worth

At m=4, each weight read serves four rows. The 2.6 GiB/token becomes 2.6 GiB per *four* tokens, and
more importantly there are four independent rows of work in flight to cover the memory latency that
decode currently has nothing to hide behind. That is the same mechanism prefill uses to be 14× faster
per token, applied at a batch size the existing kernel already supports.

It does not require the subgroup kernel, a new shader, or the driver question `--gate-bytes` is chasing.

## What is missing

A draft model producing k tokens, and a verify step calling `apply(.., k)` and accepting the longest
matching prefix. The `Deploy` panel already models the pairing — `tree_ready()` is true when a target
and a smaller draft are both selected, and renders "speculation on" — so the configuration exists and
the runtime does not.

## Against the references

llama.cpp's speculative decoding is a separate example binary driving two contexts, with the same
accept-longest-prefix rule; the batching it needs from the backend is exactly `mul_mat` at m=k, which
this engine has. Colibri does not do speculation at all — it is single-model by design — so the shape
here comes from llama.cpp rather than from both.

## Order, revised

1. `--gate-bytes` — still first, because if the invocation index is wrong it invalidates kernel work.
2. **Speculative verify** — new here, and ahead of V3 and the q4 GEMM: the kernel is written and
   tested, the failure mode is loud (a wrong verify produces wrong tokens immediately), and it attacks
   the 14× directly rather than the constant in front of it.
3. V3's autotune result on hardware.
4. The q4 prefill GEMM.
