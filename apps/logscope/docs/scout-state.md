# Llama-4 Scout: what the loader now reports, and what it still cannot

Scout loads on both platforms and writes nonsense. That has outlived every GPU-side fix — the Q6_K
kernel, the subgroup guard, the memory planner, the arena — because **none of those were wrong for
Scout**. The cause is upstream of the kernels, in what the loader decided from constants.

## Constants that became metadata reads

| value | was | now |
|---|---|---|
| NoPE interval | hardcoded `4` | `{arch}.attention.no_rope_interval`, 4 as fallback |
| attention temperature | `AttnTemp::default()` | `{arch}.attention.floor_scale` / `.scale` |
| `expert_weights_scale` | never read | metadata, applied **after** normalization |
| `expert_weights_norm` | derived from arch | metadata, with Llama-4 vetoing |

Each has the same failure signature: **the model loads, every tensor is present, every shape matches,
and the text is bad.** No shape check can see any of them, which is why they survived a dozen rounds of
kernel work.

## The structural one

**iRoPE pairs two layer sets.** Rotary layers take chunked *local* attention; NoPE layers take *global*
attention with length-dependent temperature. That pairing is how the model gets position on every layer
despite a quarter having no rotation.

The loader derives them from **different metadata keys**, so nothing prevents disagreement. A layer
that is both windowed and NoPE has local attention and **no positional signal at all**. The loader now
warns; it does not correct, because the file is the authority and silently overriding one key with the
other is how the `qwen35` prefix bug hid for weeks.

## What a Scout load prints now

```
loader: Llama-4 iRoPE — every N layers skips rotation (NoPE), starting at layer N-1.
        Read from metadata, or 4 if the file omits it
loader: Llama-4 layer i is {not }windowed and {not }NoPE — iRoPE expects these COMPLEMENTARY   [if wrong]
```

Three answers where there were none: the interval, its provenance, and whether the two patterns agree.

## What was checked and found correct

* **Chunked SWA** — `(p1/n)*n` masks to the query's chunk start, not the last *n* positions. Gemma's
  rule would be wrong here and is not what runs.
* **`kq_norm`** — reads `use_qk_norm`, with the expert-count heuristic only as fallback.
* **Interleaved MoE** — detected per block from the tensors present, not assumed from a pattern.
* **Template translation** — the Llama-4 branch replaces `<|end_header_id|>` unconditionally, unlike
  the ChatML branch which only matched it followed by two newlines (fixed in _409).

## What the loader still cannot tell you

Whether the **values** it read are right. If Scout's file omits `no_rope_interval`, the fallback of 4 is
used and printed as such — and 4 is Scout's documented pattern, so a silent agreement is the expected
case. The warning fires only on an internal contradiction, not on a wrong-but-consistent pair.

**The next datum is a Scout load on either machine.** If the interval prints as 4 with no warning, the
remaining causes are downstream of everything fixed here, and the tokenizer byte-drop counters are the
next place to look.
