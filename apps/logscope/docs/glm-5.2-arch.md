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

## Practical note

It does not fit an M2 Ultra. The 1-bit quant is ~223 GB against a 192 GB ceiling. This document exists so
the architecture can be implemented and unit-tested without the weights — the metadata is the part that
was missing, and it is now recorded.
