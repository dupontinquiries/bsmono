# What a diffusion LLM would need from this engine

From the LLaDA2.X repo (README and tech reports only — **no code, no config, no modeling file**, so
every shape below still has to come from a GGUF or the paper rather than from the drop).

## What the repo does establish

* **LLaDA2.0** — 16B (A1.4B) and 100B (A6.1B), both MoE, both Apache 2.0, weights and training code
  open. The first diffusion LM at the 100B scale.
* **2.1x inference acceleration** from parallel decoding (CAP) on flash. That matters more than the
  headline: it means these checkpoints *tolerate* aggressive parallel unmasking, so throughput
  claims rest on a demonstrated property rather than a hoped-for one.
* **LLaDA2.1** (2026/02) adds token editing; **LLaDA2.2** (2026/07) adds Levenshtein-based agentic
  work — both newer than the 2.0 material usually quoted.

## The arch gap, measured

```
bailing   0 matches in archcfg.rs
ling      0 real matches (the six hits are substrings of "scaling" and "spelling")
llada     0
dream     0
qwen3moe  18   <- present
deepseek2 16   <- present
```

So RND1-30B, being a Qwen3-MoE conversion, is close to free. LLaDA2 is a new architecture.

## The part that is not an arch entry

**A diffusion LM is not causal.** It denoises a masked sequence over several steps, attending
bidirectionally within a block, and that invalidates more of this engine than the arch table
suggests:

| assumption | holds for AR | holds for diffusion |
|---|---|---|
| KV cache accumulates one row per token | yes | **no** — a denoise step rewrites the block |
| `attention_mh_ext_swa` (causal, windowed) is the decode kernel | yes | **no** — `ATTENTION_MH_FULL` is the bidirectional twin and is the one that matters |
| decode is one token per forward | yes | **no** — one forward per denoise step over k tokens |
| `layer_can_route`, expert streaming, `cache_read_share` | yes | **undefined** — they all model per-token accumulation |

`ATTENTION_MH_FULL` already exists in this tree, which is the same pleasant surprise as the tiled
prefill kernel: **the bidirectional path is written and the causal one is what everything calls.**

## Honest ordering

1. **The prefill attention call site.** Benefits every model here today, diffusion or not, and is a
   call change rather than a kernel.
2. **RND1-30B as an arch alias** — qwen3moe plus mask-token metadata plus non-causal attention. The
   smallest real diffusion milestone, and the one that would prove the non-causal path works before
   anything larger depends on it.
3. **LLaDA2 proper**, which needs shapes this repo does not contain.

The tempting reordering — go straight for flash at 100B because the tok/s numbers are exciting — puts
the largest checkpoint on the least-tested path. RND1 first is not caution, it is where the evidence
is cheapest to get.
