# Llama-4 Scout: first successful load, and what it still gets wrong

```
MoE won't fit fully resident → EXPERT STREAMING (q4, auto): pin 1/16 experts
MoE streaming — quantized 48/48 layers (43s elapsed)
loaded in 52.2s — d=5120 layers=48 heads=40/8 head_dim=128 ff=16384 vocab=202048
produced 7 tokens in 8.7s = 0.81 tok/s
```

**Scout loads and generates.** That is new. `LOGSCOPE_MAX_GPU_GB=16` forced the planner past a
resident plan it could never satisfy, and the streaming path completed all 48 layers.

## What it produced

```
"\n  ( + ." الأمري<|python_end|>"
ids = [198, 220, 330, 584, 97588, 97999, 200017]
```

Still wrong. High vocabulary IDs and unrelated script — the signature of a model running on weights
that are not what the file holds.

## Two confounds in this particular run, both avoidable

**The prompt was translated.** `prompt carried ChatML markers to a Llama4 model and was translated` —
this was the first turn after a model switch, and the prompt is built before the load completes, so the
UI still had the previous model's template. The translation replaces markers unconditionally for
Llama-4, so it should be correct, but a first turn is the one turn where it cannot be ruled out.

**A second message in the same session is the clean test.** By then `self.template` has been updated
from `Evt::Loaded` and no translation happens.

**The GPU budget was 16 GiB against a device that holds ~31.** Streaming pinned 1 of 16 experts, so
almost every expert is fetched per token — which explains 0.81 tok/s and is a plausible source of the
reported timeouts. A budget nearer 28 GiB would pin far more and stream far less.

## What this rules in and out

**Not the resident path** — this run never used it.

**Not the arena's mapped-slab handling** — streaming does not fill 24 GiB of slabs.

**Still open: the Q4_K conversion.** `Q4K_TO_Q4`'s lane-varying shift was fixed in _515, and this run
includes it. So either that was not Scout's problem, or the expert-streaming path converts through a
different route that still has one — worth checking `raw_expert_to_q4_resident` and the streaming
quantizer for the same pattern.

**Still open: the four arch constants.** NoPE interval 4, qk-norm on, floor 8192, 1 shared expert.
GGUF defines no key for three of them, so they cannot be checked against the file — only against
llama.cpp's own values, or by a layer-by-layer diff.

## Next two runs, in order

1. **Second message in the same session** at `LOGSCOPE_MAX_GPU_GB=28`. Removes the template confound
   and pins many more experts, so both the output and the speed become interpretable.
2. If it is still wrong, a **layer-by-layer diff against llama.cpp** on the same prompt — dump `blk.0`
   after one token from both and walk forward. That names the divergent layer directly instead of
   eliminating candidates one at a time, which is what the last six commits have been doing.
