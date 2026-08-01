# Fitting GLM-5.2 in 192 GB

The declared minimum is ~223 GB against 192 GB of RAM. Two levers, and the obvious one is probably the
weaker.

## Lever 1: quantization — works, and costs the model

```
current (q4-ish)   223.0 GB   does not fit
IQ3_XXS            151.6 GB   fits
IQ2_XS             114.5 GB   fits
IQ2_XXS            102.1 GB   fits
```

IQ3_XXS clears 192 GB with room for KV. But per `extreme-quant.md`, we cannot READ IQ2/IQ3 today, the
loss concentrates in exactly the capabilities a coding assistant needs, and a 200B-class model at IQ2 is
not "the same model, smaller" — it is a weaker model with the same parameter count.

## Lever 2: residency — it is an MoE, and it never touches most of itself

This is the one worth trying first, and it is nearly free.

An MoE routes each token to a handful of experts. At DeepSeek-class ratios (8 of 256) the active FFN
fraction is **3%**; even at Llama-4's 1-of-16 it is 6%. The remaining 94–97% of the file is not read for
that token, and possibly not for the next hundred.

`mmap` already exploits this: the file stays on disk, pages fault in as experts are routed, and the
kernel's page cache holds whatever is hot. **The constraint becomes the working set rather than the
file.** A 223 GB model with a 20 GB hot set behaves like a 20 GB model that occasionally stalls.

Three things make this concrete here:

* The loader already maps rather than reads — `retain_map` keeps the mapping and
  `release_stale_map_residency` hands back page cache for models that are no longer active.
* The CPU path reads Q4_K **straight from the mapped bytes** with no dequantization pass and no host
  copy, so an expert that is paged in is immediately usable.
* `prefetch_mapped` can advise `WillNeed` on the next layer's range while the current one computes,
  which is the Colibri `couple_prefetch` idea and turns a fault into an overlap.

### What decides whether it works

**Expert locality across tokens.** If consecutive tokens route to overlapping experts, the hot set stays
small and the page cache absorbs it. If routing is close to uniform, every token touches a different 3%
and the working set approaches the whole file — at which point you are reading from disk at disk speed
and the answer is quantization after all.

That is measurable before anything is built: log the routed expert IDs per layer per token over a few
hundred tokens and compute the reuse rate. `moepolicy` already tracks hit rates for the pinned-expert
cache, so the instrumentation mostly exists.

## Why the GPU cannot do this

Per the driver audit, drm/asahi has **no shrinker** — no purge, no evict, no madvise. GPU buffers are
pinned for life. So the paging trick is CPU-only: the kernel can reclaim page cache backing a CPU
mapping and cannot reclaim a GPU allocation. A model larger than VRAM must stream explicitly on the GPU,
and can simply be mapped on the CPU.

That is an argument for GLM-5.2 being a **CPU-path model** on this machine, at 113 GiB/s measured, rather
than a GPU one — and it is the first case where the CPU path is not a fallback but the only option.

## Order of work

1. Measure expert reuse on a model we can already run (Scout, Qwen3-30B). Cheap, and it decides the rest.
2. If reuse is high: GLM-5.2 on the CPU path via mmap, no new quantization needed.
3. If reuse is low: IQ3_XXS support, with a gate against a CPU reference — and note that Q6_K took four
   faults to get right, three invisible to inspection.


## Colibri runs GLM-5.2, and its glm.c answers all three blockers

`colibri-main/c/glm.c` is 6,657 lines of "Motore GLM-5.2 (architettura glm_moe_dsa)". Its header
comment is an implementation guide, and one line removes the blocker I called structural.

### The sparse attention indexer is a NO-OP at short context

> *"Il DSA indexer e' un NO-OP per seq <= index_topk (seleziona tutte le key): qui si usa attenzione
> causale densa -> output identico all'oracolo su prompt corti."*

The indexer selects the top-k keys. When the sequence is **at or below `index_topk`, top-k is every
key** — so dense causal attention is not an approximation of the sparse path, it is *the same
computation*. Colibri validates this against a transformers oracle and reports identical output.

GLM-5.2's `index_topk` is 2048. So **the whole indexer is unnecessary below 2048 tokens of context**,
and above it the model degrades rather than breaking. That converts "structural blocker, no dense
fallback" into "works today at short context".

### The two weighting knobs, exactly

```c
3036:  if (norm_topk) { sum = Σ w; sum += 1e-20; w[k] /= sum; }   // normalize
3037:  for (k) w[k] *= routed_scale;                              // THEN scale
```

**The order is load-bearing and the wrong order is silent.** Normalizing divides by the sum, which
removes any common factor — so scaling *before* normalizing cancels the scale entirely. The model runs,
looks fine, and `routed_scaling_factor` does nothing.

### Residency is far smaller than the file

> *"gli expert (streaming) e la parte DENSA residente … sono tenuti in int8 per-riga + scala … ~17B
> param residenti a int4 ~= 8.7 GB"*

The routed experts stream per-expert from disk; only the dense part, attention, embeddings, lm_head and
the shared expert stay resident. That is **~17B parameters, not 200B+** — which is the `tensorplace`
split arrived at independently, confirmed by an implementation that runs the model.

## Revised status

| blocker | status |
|---|---|
| `ExpertWeightsNorm` | **implemented** — sum, epsilon, divide |
| `ExpertWeightsScale` | **implemented** — after the norm, with the ordering test |
| `SparseAttentionIndexer` | **not needed below 2048 context**; dense causal is exact there |

What remains before a run: the MLA path (`supported: false`, never verified end to end), and reading
`routed_scaling_factor` out of GGUF metadata rather than defaulting to 1.0.
