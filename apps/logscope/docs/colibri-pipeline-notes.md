# What Colibri and llama.cpp do in the expert pipeline, and what we can take

Checked while testing was running. The hoped-for speedup is not there; three real findings are.

## Colibri does not convert between quant formats

Its weights are held **int8 per-row plus a scale**, and dequantized *inside* the matvec:

```
 384: y[S,O] = x[S,I] @ W^T con W quantizzato int8 per-riga + scala[O] (dequant-on-use)
2291: acc[0..I) += coef * W[row,:]   (dequant al volo)
```

The tempting read is that this avoids a load-time conversion. **It does not** — Colibri still converts
the source format to int8-per-row once at load, exactly as we convert to our q4 resident layout. Both
pay one pass. So there is no free win here, and saying otherwise would have sent us rewriting the fill
path for nothing.

What *is* different is **scale granularity**. Colibri uses one scale per ROW; Q4_K and our resident
layout use one per 32 weights. A per-row scale means the inner loop does one scale multiply per row
instead of one per block — the same lesson as the integer dot in `cpumatvec`, where accumulating
int8×int8 in i32 and applying the scale once was worth 2x.

Whether that transfers depends on quality: per-row scales are coarser, and on a 3B-active MoE the error
does not average across a wide active set. It is a measurement, not a decision.

## A gate was claiming more than it tested

`--gate-q4k` reported:

> GPU reads native Q4_K exactly ✓ — **the loader can stop requantizing**

It exercises `dequant_q4k`, which proves exact **Q4_K → f32 conversion**. That is not a native matvec
over Q4_K blocks, and the loader cannot stop requantizing on that evidence. I acted on this message for
several commits, including when planning the Q6_K file-view work.

**A gate that overstates its scope is worse than a missing gate**: it is a green light nobody
re-derives. Reworded to say what it tests.

## The remaining Q6_K work is unchanged, and correctly scoped

The on-GPU Q6_K→q4 fill shader is still the missing piece, and Colibri offers no shortcut around it —
it would need the same conversion in the same place. What it does confirm is the *shape*: a single
dispatch that reads the source block and writes the compute layout, rather than two passes with an f32
buffer between them.

## What not to take

llama.cpp's `-ot` tensor placement and Colibri's expert streaming both assume the host copy we are
trying to remove. Neither is a model for a zero-copy path, because neither has one — llama.cpp reads
native blocks in its kernels instead, which is the thing our `dequant_q4k` gate was mistaken for.

## Reference-check log

Running tally, because the pattern in it is more useful than any single result.

| checked | outcome |
|---|---|
| multi-accumulator reductions | **corrected me** — Colibri uses no `-ffast-math`; the technique is accumulators, not a flag |
| `-ot` tensor-class placement | **corrected me** — llama.cpp splits by class, not by layer |
| routed-scale ordering | **corrected me** — normalize *then* scale; reversed, the scale cancels silently |
| GLM sparse indexer | **corrected me** — a no-op at `seq ≤ index_topk`, so dense is exact below 2048 |
| `expert_weights_norm` (Llama-4) | **corrected me** — architectural, hardcoded `false` |
| `expert_weights_norm` (Qwen3-MoE) | **corrected me** — architectural, hardcoded `true`; my Llama-4 fix was half a fix |
| `expert_weights_scale` | **confirmed** — read from hparams in both builders, so metadata-driven is right |
| QK-Norm vs RoPE ordering | **confirmed** — norm first, per head_dim; ours matches |

**The pattern:** six corrections, two confirmations — and *five of the six corrections were of my own recent
changes*, not of old code. Checking a reference right after touching an area catches far more than a
periodic sweep, because the thing most likely to be wrong is the thing most recently reasoned about
without one.

**A second pattern worth naming:** three of the corrections were *silent* failures — the scale
cancelling, and both `expert_weights_norm` directions. Each would have produced a model that loads,
runs, and answers plausibly. None would have shown up in a gate. That is the class where a reference
implementation is worth more than any test we can write ourselves, because the test would encode the
same misunderstanding.
