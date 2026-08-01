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
