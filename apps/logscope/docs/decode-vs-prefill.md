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

---

# Colibri hides FMA latency with two accumulator chains; our decode matvec has one

`glm.c`'s int8 matvec, NEON path:

```c
float32x4_t ac0 = vdupq_n_f32(0), ac1 = vdupq_n_f32(0);
for (; i+8 <= I; i += 8) {
    int16x8_t w16 = vmovl_s8(vld1_s8(w+i));
    ac0 = vfmaq_f32(ac0, vld1q_f32(xs+i),   ...low half...);
    ac1 = vfmaq_f32(ac1, vld1q_f32(xs+i+4), ...high half...);
}
a = vaddvq_f32(vaddq_f32(ac0, ac1));
```

**Two independent accumulators**, summed once at the end. `MATVEC_Q4` has one:

```wgsl
var acc = 0.0;
...
acc = acc + a[kk] * scale * f32(q);
```

Every iteration waits on the previous one's result. On a unit with multi-cycle FMA latency that
serialises the whole loop to `iterations × latency`, regardless of how much bandwidth is available —
which is the same shape as the 14× measured above, arriving one level down. Prefill hides latency with
**more rows**; two chains hide it with **more independent work per row**, and decode has neither.

## Why this is worth more here than in the reference

Colibri does it on a CPU where the FMA latency is 3–4 cycles and the win is modest. On a GPU thread the
dependency chain is the same shape but the loop is longer — `k` is 4096 or 14336 — so the serialised
cost is proportionally larger, and there is no out-of-order engine to find the parallelism for you.

## The change

Split the accumulator, unroll the inner loop by two, sum at the end:

```wgsl
var acc0 = 0.0; var acc1 = 0.0;
// ... two loads and two FMAs per iteration, independent ...
let acc = acc0 + acc1;
```

Bit-exactness does **not** hold: floating-point addition is not associative, so the result differs by
reassociation exactly as `linear_q8_0_gemm_rb` already notes about its own tiling. That is acceptable
for the same reason it is there, and it means the gate must compare against a tolerance rather than
byte-for-byte.

## Where it sits in the order

Below `--gate-bytes` — if the invocation index is wrong, kernel work is premature. But **above** both
the q4 prefill GEMM and speculative verify, because it is the smallest change of the three, needs no
draft model, and attacks decode's latency directly rather than around it.

Also unlike those two, it applies to **every** decode kernel and quantization at once — `MATVEC_Q4`,
`MATVEC_Q8_0`, and the MoE variants all have the same single-accumulator shape.

**Written from Colibri's source, not measured here** — the container has no Rust toolchain after a
reset, so this is a reading of two implementations rather than a benchmark of either.

---

# Fan-out already has the batch that decode is missing, and decodes it serially

Colibri's `decode_batch.h` names the technique in its header: *"`base` belongs to one sequence's KV
state. Keeping this arithmetic in a model-independent seam makes **ragged decode row ownership**
directly testable."* Multiple sequences decoding together, each contributing one row.

**This engine already produces those sequences.** Fan-out runs N branches over a shared prefix — the
loader computes the longest common token prefix across every branch and prefills it once. And then
`lanes.rs` says what happens next:

> two long-running director tasks serialize (the fan's "round-robin")

Each branch decodes **alone, at m=1**, one after another.

## What that costs

For an N-branch fan, the weights are read N times per token-step instead of once. `MATVEC_SMALL_Q4`
already handles m≤8 by reading each nibble once and accumulating all m rows, and each row is bit-exact
to the m=1 kernel. A 4-branch fan is a 4× reduction in decode weight traffic for a kernel that exists,
is tested, and is currently reached by nothing.

## Why this is the better first batching target than speculation

Speculative verify (`specverify.rs`) needs a draft model, an accept rule, and a policy for how many
tokens to draft — and a fully-rejected round wastes the draft's time. **Fan-out needs none of that:**

* the sequences are all **real**, so every row of the batch is work someone asked for;
* there is no accept/reject, so no round is ever wasted;
* the shared prefix is already computed, so the KV states are already siblings;
* and the batch size is whatever the user chose, not something to tune.

It is the same lever — more rows in flight to cover memory latency — with none of the machinery.

## What it needs

The round-robin replaced by a step that advances all live branches together: gather one activation row
per branch, call `apply(.., n_live)` once per layer, scatter the results back to each branch's KV.
`decode_batch.h`'s seam is exactly that — per-sequence KV row arithmetic kept separate from the model,
so ownership stays testable when the rows are interleaved.

Branches finish at different lengths, which is the "ragged" part: the batch shrinks as sequences hit
their stop token, and m must follow it down rather than padding.

**Read from Colibri and from this tree's own comments, not measured** — the container has no toolchain
after a reset.
