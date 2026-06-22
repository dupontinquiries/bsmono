# 08 · What the KV cache is (and the prefix tree on top of it)

*You asked what the KV cache actually is, whether it's "latent mapping," and whether it's GPU-shaped to
the max or CPU-shaped and could be reshaped. This walks through it, grounded in the engine's code and the
numbers from your M2 run.*

## The one-paragraph version

Attention at token *t* needs, for every earlier token, a **Key** and a **Value** vector — linear
projections of that token's hidden state at each layer. Those K/V vectors don't change once a token is
fixed: token 5's keys are the same whether token 6 or token 600 is being generated. So instead of
recomputing them every step, the engine writes each token's K and V into a per-layer buffer — the **KV
cache** — and attention just reads them back. That's the whole idea: *the KV cache is the stored attention
state for the tokens you've already processed, so you never recompute their contribution.* Generating a
new token is then cheap (one new row of K/V + an attention read over the cache); the expensive part is the
first pass that fills the cache for a long prompt — the **prefill**.

## Why reusing a prefix is the big win

If the KV for a run of tokens is already sitting in a cache, and a new prompt **starts with those same
tokens**, you can copy that KV in (a GPU buffer copy, no math) and only prefill the tokens that differ.
You saw exactly this:

```
full prefill in 8.06s        (1087-token prompt, computed from scratch)
reused 1058 anchor tokens + prefilled 29 in 1.37s   (same-size prompt, 29 tokens differed)
```

~8.1 s → ~1.4 s for the prefill, because only 29 of 1087 tokens needed the forward pass. Note what does
**not** change: the decode rate (~15 tok/s on that model) is identical — reuse cuts the *prefill*, not the
per-token decode. For a deeply-branched chat where every continuation shares a long system-prompt + history
prefix, that prefill saving is the dominant lever, which is why it was worth building.

Is it "latent mapping"? Close in spirit, but be precise: the K/V vectors **are** latent (they live in the
model's hidden space), and reuse is a form of caching computation keyed by a token prefix — but there's no
similarity search or embedding lookup. It's exact-prefix reuse: the cached KV is only valid for a prompt
that is byte-identical up to position *l*. (A fuzzy/semantic version — reuse KV for a *similar* prefix — is
not sound in general, because every token's K/V depends on all tokens before it.)

## Single anchor → prefix tree (what got built)

The engine already reused **one** anchor: the last turn's (tokens, KV). Great for linear chat, useless the
moment you branch — a sibling switch or N `:continue-all` continuations off one leaf share a prefix with a
*different* lineage than "the last turn," so it re-prefilled from scratch. The prefix tree keeps a small
**pool** of recent branch tips and reuses whichever shares the longest prefix (`kvtree::pick_anchor`). The
payoff for a fan-out: the first continuation off a leaf prefills its lineage and caches it; the rest seed
from cache → **1 prefill + N decodes**.

- Turn it on with `LOGSCOPE_KV_TREE=N` (keeps N tips warm). Unset → pool size 1 = the original single
  anchor. **Your log didn't print the `KV prefix tree ON — pool cap N` line, so that run was at cap 1** —
  the reuses you saw were the single anchor following a mostly-linear path. To exercise the tree, set
  `LOGSCOPE_KV_TREE=6`, branch (`b`), switch between siblings (`h`/`l`) and continue each — revisiting a
  branch should log `reused …` instead of `full prefill`, where cap 1 would have missed.
- Correctness is proven by `--gate-kvtree` (reusing a cached branch's KV for a *different* lineage matches
  a full prefill, argmax-identical); the prefill-time win is the M2-side measurement.

## Is the cache GPU-shaped to the max, or CPU-shaped? (your reshape question)

**GPU-resident, and right-sized per generation — not pre-allocated to the max context, not CPU-side.**
Concretely (`gpu-gemm/src/lib.rs`):

- `pub struct KvCache { pub k: Vec<Tensor>, pub v: Vec<Tensor> }` — one K tensor and one V tensor **per
  layer**, each a GPU buffer of shape `[max_seq, kv_dim]`.
- `new_cache(n_layers, max_seq, d)` allocates those buffers, and the engine calls it with
  `max_seq = cap = prompt_len + reply_budget` for **this** generation. So a 1087-token prompt with a
  small reply allocates ~that many rows, not the full 2560 window — it grows to what the session needs,
  not to the ceiling.
- `kv_dim = n_kv_heads · (d / n_heads)` — for your SmolLM2-360M load that's `5 · (960/15) = 320` per
  token per layer, ×32 layers, ×2 (K and V). At f32 that's ~80 KB/token; the engine also has an
  **int8 KV cache** (`QKvCache`, per-row-quantized) that's 4× smaller when enabled.

So yes — it can be reshaped, and there are two real directions:

1. **Paged / shared KV (the big one).** Right now the prefix tree *copies* the shared prefix's KV into a
   fresh buffer on reuse (`seed_from_prefix` copies row ranges). A paged design — KV stored in fixed-size
   blocks in one GPU arena, branches **sharing** the blocks of their common ancestor by reference instead
   of copying — makes a fork near-free and lets many branches coexist without N full copies. This is the
   vLLM "PagedAttention" idea; it's the natural next step for the tree and would cut both the per-fork copy
   cost and the pool's memory (today each pool entry holds its own full cache → memory = cap × one cache).
2. **int8 (or lower) KV residency.** The `QKvCache` path already exists; making the tree pool hold int8
   KV would 4× the number of tips you can keep warm for the same memory — directly raising how wide/deep a
   space you can keep resident.

Both are sizeable engine changes (block allocator + attention reading paged/quantized KV), but they're the
right "reshape" once the M2 numbers say the copy cost or the pool memory is the bottleneck — so the thing
to watch in your runs is peak memory at a given `LOGSCOPE_KV_TREE=N` and how the `reused … prefilled K`
prefill time scales with K.
