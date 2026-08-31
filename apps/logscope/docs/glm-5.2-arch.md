# GLM-5.2 (`glm-dsa`) — requirements, from the shipped metadata

Transcribed from `GLM-5_2-UD-IQ1_S-00001-of-00006.gguf` (a metadata-only first shard: 0 tensors,
69 KV pairs, 1809 tensors across 6 files). Nothing here is inferred from the family name.

## It is not GLM-4

The architecture string is **`glm-dsa`**, not `glm4moe`. Treating them as one would apply the wrong
forward pass silently.

| | value |
|---|---|
| layers | 79 (`leading_dense_block_count` 3 dense, rest MoE) + 1 `nextn_predict` MTP block |
| d_model | 6144 |
| heads | 64, `head_count_kv` 1 |
| MLA | `q_lora_rank` 2048, `kv_lora_rank` 512, `key_length` 576 / `key_length_mla` 256 |
| experts | 256 routed, 8 used, 1 shared, `expert_feed_forward_length` 2048 |
| gating | `expert_gating_func` = 2 (**sigmoid**), `expert_weights_norm` = true |
| **expert_weights_scale** | **2.5** |
| **indexer** | 32 heads, `key_length` 128, **`top_k` 2048** |
| context | 1 048 576 |
| rope | `freq_base` 8e6, `dimension_count` 64 |
| vocab | 154 880 |

## What we already have

MLA (wired, never run on a real checkpoint), shared experts, `leading_dense_block_count`, MTP-block
skipping, and both `*_mla` dim spellings.

## What is missing, in order of how quietly it fails

1. **`expert_weights_scale` = 2.5** — never read; effectively hardcoded to 1. Every MoE layer would
   contribute 40% of what it should. This does not error and does not produce noise: it produces a model
   that is *quietly worse*, which is the hardest kind of wrong to notice. Cheapest to fix and the most
   dangerous to leave.
2. **`expert_weights_norm`** — renormalize the top-8 weights to sum to 1. `moe::route` does this for
   softmax but not for the sigmoid path, which is exactly the combination GLM-5.2 uses.
3. **Sparse attention indexer** — STRUCTURAL. A separate 32-head indexer scores keys and full attention
   runs only over its top 2048. There is no dense fallback that is merely slower: attending to a 1M
   context in full is not something the engine can do.

## Confirmed by an actual load attempt

The engine unified all 6 shards (1809 tensors, **201.8 GiB**) before refusing, so the shard handling and
the tensor index are fine. The block-0 dump confirms the metadata and adds shapes the KV pairs did not:

```
blk.0.attn_k_b.weight        [192x512x64]  Q8_0   <- 3-D: per-head MLA reconstruction
blk.0.attn_v_b.weight        [512x256x64]  Q8_0   <- 3-D
blk.0.attn_kv_a_mqa.weight   [6144x576]    Q8_0
blk.0.attn_q_a.weight        [6144x2048]   Q5_K
blk.0.indexer.attn_k.weight  [6144x128]    Q8_0   <- the sparse-attention indexer
blk.0.indexer.attn_q_b.weight[2048x4096]   Q8_0
blk.0.indexer.k_norm.weight  [128]         F32
blk.0.indexer.proj.weight    [6144x32]     F32
```

Two things that were not visible from the KV metadata alone:

* **`attn_k_b` and `attn_v_b` are 3-D** (`[head_dim x lora x n_head]`). Our MLA loader expects 2-D
  projections, so even the implemented MLA path needs a shape change to read these.
* The **indexer is four tensors per block**, all present and all currently unread.

Quant mix is Q8_0 / Q5_K / Q6_K — no IQ1_S in block 0 despite the file name, so the 1-bit quantization is
applied selectively to the expert tensors.

## Practical note

It does not fit an M2 Ultra. The 1-bit quant is ~223 GB against a 192 GB ceiling. This document exists so
the architecture can be implemented and unit-tested without the weights — the metadata is the part that
was missing, and it is now recorded.

## STATUS UPDATE (c4-785) — the blocker list is shorter than this document claims

Two of the three "missing" items above are CLOSED and this file went stale. `expert_weights_scale`
and `expert_weights_norm` are both read from metadata and applied (archfeat.rs); the LLaDA2 loads in
the operator's logs print `scale 2.5 (file)` and `normalize true` through the same code path. What
remains is the indexer, and the upstream picture changes what that means.

### llama.cpp does not implement DSA either — it runs GLM-5.2 DENSE

Issues #24730 (GLM-5.2) and #20363 (DSA lightning indexer) are both open; mainline runs the model
with a dense-attention fallback. So the indexer is NOT a prerequisite for first tokens, only for
long-context efficiency. Our own note here — "there is no dense fallback that is merely slower" —
is true of a 1M context and false of a 32K one, which is the context anyone would actually use on a
workstation. Running dense at modest context is the same bargain mainline ships.

### IndexShare is the trap, and it is a LOADER trap

GLM-5.2's change over 5.1 is `index_skip_topk_offset: 3` and `index_topk_freq: 4`: the indexer is
recomputed once every four layers after the first three, so only about one layer in four CARRIES
indexer tensors and roughly 57 of 78 layers have none. Stock llama.cpp demands an indexer tensor on
every layer and therefore FAILS TO LOAD converted GGUFs — a third-party conversion (0xSero) needed a
custom patch for exactly this. Our loader must treat indexer tensors as optional and per-layer, or it
will reproduce that failure with a confusing message.

### The shape, confirmed against three sources

744B total / ~40B active, 78 layers, d 6144, 256 routed + 1 shared, 8 active, 3 dense lead, MLA +
DSA, context 1048576. Matches the metadata transcribed above.

### Order of work to first tokens on a 192 GB M2 Ultra

1. Treat indexer tensors as OPTIONAL and skip them; run dense MLA. This is what mainline does.
2. Cap the context to what dense attention can afford (32K, not 1M) and say so at load.
3. Fit. IQ1_S is 201.8 GiB against 192 GB and does not fit; IQ3_XXS at 151.6 GiB does, but needs the
   IQ3 reader we do not have. The MoE-residency lever in glm52-fit.md is the alternative and needs no
   new quant support — 8 of 256 experts means a small hot set, and `prefetch_mapped` already exists.
4. Only then the indexer, for long context.

### Alternatives worth weighing before building any of it

* `unsloth/GLM-5.2-GGUF` is the de-facto artifact; a REAP expert-pruned 504B GGUF also exists and is
  materially smaller, at the cost of pruned experts.
* Two M3 Ultra 256 GB boxes run UD-Q4_K_S over llama.cpp RPC — decode there is latency-bound on the
  link, not compute-bound, which is worth knowing before treating distribution as a fix.

## STATUS UPDATE 2 (c12-140+) — loading closed; upstream now ships DSA

The MLA loading gap is closed: per-block routing in the resident loader, both kv_b layouts, the
split 3-D per-head fuse, and MoE+MLA in one block — the gen_glmdsa.py fixture loads and decodes
finite logits (--gate-mla-load, --probe). IndexShare is a non-issue for this loader (unread
tensors are simply not fetched; the fixture puts indexer tensors on 1 of 3 layers and loads).

The upstream picture moved: llama.cpp master (Aug 2026 snapshot, vendored in refs) now SHIPS a
real DSA implementation — src/models/glm-dsa.cpp + llama-kv-cache-dsa.h, a 32-head lightning
indexer with per-layer `indexer_types` (IndexShare handled via a per-layer full/shared flag,
GLM-5.2's default pattern hardcoded from its config.json) and indexer tensors TENSOR_NOT_REQUIRED.
That is the reference to transcribe if long-context efficiency ever matters here; dense fallback
remains exact at or below index_topk (2048). Remaining real blocker for the full model is
unchanged: 201.8 GiB of IQ1_S against a 192 GB ceiling.
