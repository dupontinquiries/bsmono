# Long-context notes

Long context is two problems. Extending *positions* past the trained length is cheap
and mostly already supported here; *paying* for the context (KV memory + attention
compute) is the real work, and it's the same track as our batch/compression roadmap.

## Positions — RoPE scaling (largely a load-time change)

A model trained to N tokens has only seen RoPE rotations up to N; beyond that the
positions are out of distribution. The fixes manipulate RoPE:

- **Positional Interpolation (PI)** — scale positions by 1/factor so the extended
  range maps into the trained one.
- **NTK-aware** — raise the RoPE base θ so high-frequency dims stay intact.
- **YaRN** — NTK applied per-dimension plus an attention-temperature term; extends
  cleanly (e.g. Qwen3.5-2B 262k → ~1M at 4×, as in the Soren card).

Our engine already parameterizes both knobs the RoPE kernel needs: the base θ (`m.base`)
and a per-dimension frequency-factor buffer (`m.rope_factors` / the `ff[]` input to
`ROPE_MH` / `ROPE_MH_ROWS`, originally added for Llama-3's `rope_freqs.weight`). PI,
NTK, and YaRN all reduce to *computing those factors differently*. So:

- A YaRN/NTK/PI-extended GGUF that ships `rope_freqs.weight` already works — we read it.
- For models that ship `rope.scaling` metadata (type=yarn/linear, factor,
  original_context_length, beta_fast/beta_slow) instead of a tensor, support is a
  load-time function that fills `rope_factors` from that metadata. **No kernel change.**

## Cost — the real wall

### KV cache memory (grows linearly with length)
Per-token KV = `n_layers · 2 · kv_dim · bytes`. For Llama-3.2-1B (16 layers, kv_dim 512):

| precision | per token | 256k (native) | 1M |
|-----------|-----------|---------------|-----|
| f16       | 32 KB     | 8 GB          | 32 GB |
| int8      | 16 KB     | 4 GB          | 16 GB |
| int4      |  8 KB     | 2 GB          |  8 GB |

At 1M the KV cache exceeds the weights. On a 32 GB M2, f16 1M doesn't fit alongside the
model; **int8/int4 KV is what makes 1M practical**, not an optimization. GQA already
helps (Llama-3.2: 8 KV heads vs 32 query heads = 4× saving, baked in). Bigger planned
models (9B/27B) make this strictly worse — KV quantization + paging become mandatory.

### Attention compute
Prefill attention is O(n²); a 1M-token prefill is huge. Needs **chunked prefill**
(process the prompt in windows) and **flash-style streaming attention** (never
materialize the n×n score matrix — stream keys, keep a running softmax). Decode
attention is already O(n) per step here.

### Bounded attention (cap the effective context)
- **Sliding-window attention** — each token attends only the last W tokens.
- **Attention sinks / StreamingLLM** — keep the first few tokens + a recent window;
  enough to stay coherent at unbounded length without unbounded KV.

## How this maps to our roadmap

- **int8 KV cache** (already listed under multi-instance compression) is the
  highest-impact long-context item — it's what makes long context *fit*.
- **Paged KV** (don't preallocate `max_seq`; allocate in blocks) — needed so long
  contexts don't reserve worst-case memory up front, and it composes with prefix reuse.
- **Chunked + flash prefill** builds on the batched prefill we already have.
- **RoPE scaling from metadata** — small, self-contained; do when a YaRN-only-metadata
  model needs loading.

Net: long context and the batch/compression work converge. Nothing here needs a new
math path on the position side; the engine investment is KV memory + prefill compute.

## Context eviction that doesn't choke at scale

The KV memory wall above assumes you keep every token. Past some length you can't — a
worker chewing through a 157 GB log, or a session that never ends, will outrun any budget.
The question is *which* tokens to drop, cheaply.

**Why not score-based.** H2O / heavy-hitter schemes rank tokens by accumulated attention
mass and keep the "heavy hitters." That needs per-token score state updated every step —
O(seq) bookkeeping (and a selection pass) that itself becomes the bottleneck exactly when
the context is huge. It chokes at the scale where you most need eviction.

**What we do: attention sinks + sliding window, O(1) per step.** Keep the first `sink`
tokens (pinned) and a ring of the last `window` tokens; drop the middle. The sink tokens
matter — a pure sliding window collapses in quality because early tokens act as attention
sinks the model relies on; pinning a handful restores it (the StreamingLLM result). KV is
bounded at `sink+window` rows for an arbitrarily long stream.

Eviction is a single overwrite, no shifting and no score state: `stream_slot(p, sink,
window)` returns the physical slot for logical position `p`. Below the cap it fills
linearly; once full, position `p` reuses the slot of the oldest window token (`p-window`).
The trick that makes the ring free: K is rope'd at its true position *before* it's stored,
so each cached K self-describes its position and attention (a softmax sum over rows) is
order-independent — the ring can overwrite an evicted slot and attention is still exact
over whatever set is resident. `decode_step_stream` is the drop-in decode that uses it.

Validated two ways, which together prove eviction-mode correctness:
- `streaming window (no evict) == full decode` (= 0): when `sink+window ≥ context`, the
  slotting/attention path is byte-identical to full decode — so attention over the written
  rows is exact.
- `streaming window O(1) bounded eviction` (= 0, over 2000 steps): every slot stays
  `< sink+window` and each new token reuses exactly the oldest window token's slot — so the
  resident rows are precisely sinks + last-window.
Exact attention over the written rows + the rows being exactly the resident set ⇒
eviction-mode decode is exact softmax over (sinks ∪ recent window).

**Composes with the rest.** The window can itself be int8 (the warm tier from the prefix
store, `quantize_rows`/`dequant_rows`) for another ~4x on the resident KV. And per-worker
streaming caps compose with the fleet: a hundred workers each bounded at `sink+window`
rows is a flat memory ceiling, not a per-worker context blowup. Scale-safe by construction
— the cost per token is constant in the stream length.

Still load-time, not addressed here: re-mapping rope positions to keep the window's
*relative* distances small (StreamingLLM remaps the kept tokens to contiguous positions).
We keep true positions, which is exact over the resident set but lets relative distances
grow; position-remapping for very long windows is the next refinement.
