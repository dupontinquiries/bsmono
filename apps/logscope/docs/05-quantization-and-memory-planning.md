# 5. Quantization and the memory planner

```
int8-resident model forward ~ f32        max|gpu-cpu| = 1.449e-2   OK
KV f32 vs int8: argmax_match=true  cos=0.99993  KL=3.141e-4
fused q8_0 linear: int8 weights resident, 4× smaller than f32 (2560 vs 10240 bytes)
q4_0 round-trip (bounded)                max|gpu-cpu| = 6.173e-2   OK

gate-memplan: planner checks ✓ | fit table:
    8B: 8.0B params | int8 8.9 GiB | q4 5.9 GiB | plan(39.5GiB)=Resident{Int8}
   70B: 70.6B params | int8 68.6 GiB | q4 40.3 GiB | plan(39.5GiB)=Streaming
```

## Quantization: smaller weights, same answer

A model's weights are the bulk of its memory, and full `f32` is wasteful. Quantization stores them in
fewer bits: `int8` (8-bit) is 4× smaller than f32, `q4` (4-bit, in various block formats — q4_0,
q4_K, q5_K, q6_K, all with bit-exact dequant tests) is 8× smaller. The question is whether shrinking
the weights changes the model's behavior.

The tests answer it precisely, and the precision is the point. The int8-resident model's logits drift
from f32 by about `1.4e-2` — *not* the noise floor; quantization genuinely loses information. But the
thing that matters is preserved: **the argmax matches** — the model picks the same next token. The
int8 KV-cache comparison spells it out with real metrics: argmax matches, cosine similarity `0.99993`,
KL divergence `3e-4`. So it's "lossy but behaviorally faithful," and the tests draw that line exactly
instead of hand-waving "close enough." (Contrast the speculative-decoding chapter, where the claim was
`0.0` — there the output is *identical*; here it's faithful-but-not-identical, and the suite is honest
about which is which.)

This is what lets a real model run on a laptop GPU at all. The `fused q8_0 linear` test shows the
weights staying int8 *resident* — 4× less memory — while the matmul still matches f32 to `1.6e-7`.

## The memory planner: what actually fits

Quantization sets how big a model is in memory; `memplan` decides whether it fits *before* you try to
load it. The reasoning is just arithmetic, but it's the arithmetic that separates "the model" from
"the model you can actually run":

```
resident weights  = embedding + (per-layer params × layers) × bytes-per-param(format)
+ KV cache         = 2 × layers × kv_dim × context × bytes      (grows linearly with context)
vs. budget         = LOGSCOPE_MAX_RESIDENT_GB, or 90% of free RAM
→ verdict          = fits int8 / would fit q4 / must stream / too big
```

The fit table makes the consequences concrete on a 39.5 GiB machine: an **8B model fits resident in
int8** (8.9 GiB of weights + a 1 GiB KV cache) — comfortably. A **70B model can't** — its int8 weights
alone are 68.6 GiB — so the planner says *stream the layers from disk* rather than pretend it fits.
The int8 estimate is calibrated against a real measurement (the M2 reports ~0.56 GiB resident for
SmolLM2-360M, and the gate asserts the estimate lands there).

The payoff is behavioral: the loader runs this check on metadata alone (tensor dimensions, no
dequantization — cheap) and **refuses gracefully with the number** — "needs ~68.6 GiB int8 but only
~39.5 GiB is available; it would fit at q4 (~40.3 GiB), which isn't implemented yet" — instead of
beginning a multi-gigabyte load and getting killed by the OOM reaper halfway through. For the small-
local-model thesis this is the unglamorous but essential part: knowing your limits exactly, so you
spend the hardware you have on the largest model that genuinely fits and decline the rest with a clear
reason rather than a crash.
