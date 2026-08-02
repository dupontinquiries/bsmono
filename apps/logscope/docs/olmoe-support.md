# OLMoE is absent from the architecture table, and Colibri specifies it

`archcfg.rs` has no entry for `olmoe` — zero mentions. A checkpoint of it would be refused as an
unknown architecture.

Colibri implements it in `c/olmoe.c`, and its header states why: *"Obiettivo Stadio A: produrre gli
STESSI token id del riferimento (ref.json) → valida il core prima di scalare a GLM-5.2."* OLMoE was
their validation vehicle for the MoE core before GLM-5.2 — which makes it a well-specified target
rather than a guess, and the same role it could serve here.

## What Colibri's implementation says it needs

Read from `olmoe.c` rather than inferred from the family name:

| property | evidence in `olmoe.c` |
|---|---|
| **QK-norm** | `LD(qn,"self_attn.q_norm.weight"); LD(kn,"self_attn.k_norm.weight")` |
| **softmax routing** | `softmax_row(pr, E)` over the expert logits |
| **`norm_topk_prob`** | read from config as a bool, so weights renormalize to sum to 1 |
| **no shared expert** | nothing loads a `shared_expert` tensor |
| **standard RoPE** | one `theta` for all layers |

That set is **the same forward-pass shape as `qwen3moe`**: QK-norm, softmax over routed experts,
normalized top-k weights, no shared expert, no per-layer rope base.

## Why that matters for the entry

`archcfg` already expresses every one of those, and `moe_config` already takes `normalize` from metadata
with the arch as fallback. So the entry is a table row pointing at the existing Qwen3-MoE forward path
— not new kernel work.

**But "same shape" is exactly the claim this codebase has been burned by.** `glm-dsa` looks like
DeepSeek and is not; Maverick looks like Scout and disables qk-norm. So the entry should be written from
the file's own metadata the way the Llama-4 constants now are, and `archfeat::required` should list what
OLMoE genuinely needs so a mismatch refuses rather than runs.

## What would confirm it

Colibri validates against `ref.json` — the same token ids as its Python reference. The equivalent here
is a load that reports its metadata-derived choices (the loader now prints those for Llama-4) and a
short greedy generation compared against llama.cpp on the same prompt.

## Status of this note

Written from Colibri's source, not from a build. The container reset removed the Rust toolchain and
`static.rust-lang.org` is outside the allowed domains, so no code accompanies this: an `archcfg` entry
is a small change, and a small change I cannot compile is still a change nobody has checked.
