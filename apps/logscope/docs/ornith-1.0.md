# Ornith-1.0 — support outlook and how the self-training works

DeepReinforce, released 25 June 2026, MIT licensed. Four sizes: 9B Dense, 31B Dense, 35B MoE, 397B MoE.
FP8, GGUF and bf16 weights published. The Ollama listing shows 9b (5.6 GB) and 35b (21 GB) at 256K
context.

## Support: it depends entirely on the base, and the sources do not say which is which

Ornith is **post-trained on Gemma 4 and Qwen 3.5** — it is not a new architecture. So the loader question
is not "does logscope support Ornith" but "which base does this checkpoint carry", and that answer is in
the GGUF's `general.architecture`, not in any blog post. Every source found says "Gemma 4 and Qwen 3.5"
without mapping sizes to bases.

That distinction matters because the two bases are in very different states here:

**If a variant is Gemma 4** — `archfeat` already declares three gaps, and one is structural:

| feature | status |
|---|---|
| `PerLayerRopeFreqs` | not implemented — one rope base is used for all layers |
| `LayerOutputScale` | not implemented — the tensor is not read |
| `PerLayerHeadDim` | not implemented — head_dim is a single model-wide value, **structural** |

`PerLayerHeadDim` is the blocker. A model-wide `head_dim` is assumed throughout the attention path, and
per-layer head dims are not a flag to set but a change to how every attention tensor is shaped. The other
two are tractable; this one is not a small change.

**If a variant is Qwen 3.5** — unknown. That is post-cutoff and no metadata has been inspected here. Our
Qwen3 support is verified at graph level (_225), but 3.5 is a different model and the same assumption
that made Llama-4 cost three debugging rounds applies: partial support with no way to say so is worse
than refusal.

**The cheap next step** is to point the loader at a downloaded Ornith GGUF. If it refuses, the message
names the arch and the specific missing features — that is exactly what `archfeat` exists for, and it
costs one command rather than a reading of four blog posts.

## How the self-training works

The mechanism, from DeepReinforce's own write-up rather than the coverage:

> Each RL step proceeds in two stages: conditioned on a task and the scaffold previously used for it,
> the model first proposes a refined scaffold; conditioned on that scaffold and the task description, it
> then generates a solution rollout. Reward from the rollout is propagated to **both** stages.

So the harness — memory layout, retry logic, tool orchestration — is a *learnable object* rather than
something engineers hand-write per benchmark category. Scaffolds that induce higher-reward trajectories
survive; weak orchestration is selected out. Per-category strategies emerge without a human maintaining
separate configs.

Training stability comes from **staleness-weighted GRPO**: older off-policy tokens are down-weighted by
age and discarded past a threshold, so long-horizon training does not treat every stale token as equally
valid.

### The part worth reading twice

DeepReinforce describe an explicit trust boundary around the self-modification:

> we fix the outer trust boundary: the environment, the tool surface, and test isolation are immutable
> and outside the model's reach, so the model evolves only the **inner policy scaffold**: its memory,
> error-handling, and orchestration logic.

That is the right shape. A model that could evolve its own *test harness* would optimise the reward
signal rather than the task — reward hacking with extra steps — and the fix is not to hope it does not,
but to place the tests outside what it can touch. The same principle sits under `traceval` here:
execution evidence is trusted precisely because the code under test cannot rewrite the counters.

### What to hold loosely

The benchmark claims (77.5 Terminal-Bench 2.1, 82.4 SWE-Bench Verified for the 397B, beating Claude
Opus 4.7) are **self-reported**, and at least one outlet said explicitly that independent researchers
should treat them as such. Nothing here has been reproduced.
