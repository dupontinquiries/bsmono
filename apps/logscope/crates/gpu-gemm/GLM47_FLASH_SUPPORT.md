# GLM-4.7-Flash on the wgpu engine

GLM-4.7-Flash is a deepseek2-family checkpoint: MoE with a dense leading block,
q-compressed MLA attention (q_lora_rank=768, kv_lora_rank=512), one nextn/MTP
draft block counted in block_count, NEOX rope pairing. This file records what the
engine does with each of those, and the bugs real checkpoints found.

## Loader
- MLA config comes from `deepseek2.attention.{kv_lora_rank,q_lora_rank,...}` —
  kv_lora_rank > 0 is the MLA signature (hostile-dim bounded).
- Per-BLOCK attention routing is sniffed from tensors, not the config: a block
  with no `attn_q.weight` but an `attn_q_a[.weight|_proj.weight]` is a
  q-compressed MLA block. Its attn_q/k/v are never fetched (they don't exist);
  tiny F32 placeholders fill the standard fields, same pattern as MoE's dense-FFN
  placeholders, and the forward matches the MLA weights first.
  DeepSeek-V2-Lite (q_lora_rank=0) keeps its real attn_q and the standard fetch.
- If a block is q-compressed but the MLA tensor set is incomplete, the loader
  fails NAMING the set, not with a misleading "missing tensor attn_q".
- The MTP guard (drops trailing draft blocks from the layer count) now treats a
  block as "genuinely lacking attention" only when BOTH attn_q and attn_q_a are
  absent, so it cannot mistake an MLA block for a draft head.

## The bug this file exists for (2026-07-26)
`worker: load failed: missing tensor blk.0.attn_q.weight` on
GLM-4.7-Flash-Q4_K_S. The MLA math had been gate-validated (--gate-mla-block runs
q_lora=32) before any real q-compressed checkpoint existed, but the LOADER still
required attn_q unconditionally — the placeholder routing above is the fix.
Regression gate: `kernel-validate --gate-mla-load` builds a synthetic 1-layer
deepseek2 GGUF with the exact GLM tensor shape (q_a/q_a_norm/q_b, kv_a_mqa/
kv_a_norm/kv_b, attn_output, NO attn_q/k/v), loads it, and decodes 3 tokens.

## Cache discipline (the second half of the same bug)
`Gpu::new_cache(n_layers, seq, kv_dim)` is the RAW constructor: `c.mla = None`,
and an MLA model decoded against it silently falls to the standard path — with
placeholder QKV, that means wgpu validation errors (a 384-byte copy from a
4-byte placeholder scale buffer) and garbage-but-finite logits. Model-aware code
must use `Gpu::new_cache_for(&model, seq)`, which attaches the latent cache when
the model needs it. The gate covers this too (it decodes through the MLA path).

Debugging aids added while finding it: `LOGSCOPE_DEVICE_ERROR_FATAL=1` turns the
non-fatal uncaptured-error log into a panic (backtrace at the submit), and the
download paths carry debug_asserts comparing a tensor's claimed shape to its
buffer size — the mismatch fires at the enqueue site instead of scrolling by as
a validation error.

## Still open for this model class
- Real-checkpoint numerics: the 16.4 GiB q4-resident load on the M2 Ultra kit is
  the true test (arena/BO budget, MoE routing at 47 layers, MTP drop of 1).
- Tokenizer: specials registered but im_start/im_end unset — chat template use
  should be verified against the reference implementation.
