# Which models this engine can actually run, and which are worth running

Written against the operator's two machines — M2 Ultra 192 GB (Asahi) and M4 Max 64 GB (macOS) —
and against what the arch registry actually supports, rather than a leaderboard.

## The correction that changes the plan: GLM-5.2 does not fit

GLM-5.2 is ~753B. Unsloth's 2-bit dynamic GGUF is ~239 GB, which needs a 256 GB Mac Studio; the
IQ1_S referenced in `glm52-fit.md` is 201.8 GB against 192 GB of RAM, and 2-bit is a real quality cut
on top of not fitting. **There is no quant of GLM-5.2 that runs on a 192 GB box.** The MLA loader
work (c4-787) is still worth having — DeepSeek-V4 and other MLA models exist — but "get GLM-5.2
working" is chasing a model that will not load, and should be retired as a goal unless a
REAP-pruned variant is on the table.

## What the registry supports today

Supported: `llama`, `llama4`, `qwen2`, `qwen3`, `qwen3_5`, `qwen3moe`, `glm4moe`, `gemma3`,
`llada2`, `rnd1`.

Not supported: `deepseek2`, `gemma2`, `gemma4`, `glm-dsa`, `phi3`, `qwen4`.

Two of those "not supported" entries are misleading and one matters:

* **`qwen4`** — the note claimed a MoE checkpoint needs routing and expert-gather kernels this
  engine lacks. It does not: a qwen3moe fixture rebranded as qwen4 loads and produces BIT-IDENTICAL
  stats (c4-799). The flag stays false because it is earned on a real checkpoint, but the blocker is
  a download, not code.
* **`gemma4`** — genuinely unsupported, and Gemma 4 12B/31B QAT are among the better local coders,
  so this is a real gap if that family is wanted.

## The models worth trying, in order

**Qwen3-Coder-Next (80B MoE, 3B active)** is the strongest fit for the M2 Ultra. It is ~35-40 GB at
Q4, so it fits with enormous headroom; 3B active means it decodes at small-model speed; context is
256K extendable to 1M via YaRN, which is the same axis as the repo-reading work; and it reports
SWE-rebench Pass@5 64.6%. Most importantly it is almost certainly `qwen3moe`, the one MoE arch this
engine has confirmed against a real checkpoint (Qwen3-Coder-30B-A3B: d=2048, 48 layers, 128 experts,
top-8, no shared expert). Expect it to load.

**Qwen3.6-27B dense** is the strongest fit for the M4 Max. SWE-bench Verified 77.2 on Qwen's own
card — Sonnet-class on GitHub-issue resolution, with the usual caveat that a vendor benchmark is a
model card and not independent verification. Dense at 27B is ~16 GB at Q4 and comfortably fast on
either machine, and dense Qwen loads on the existing QK-Norm path.

**Qwen3.6-35B-A3B** is the reported best all-rounder at 32 GB, and 3B active makes it quick.

**Muse Glimmer 30B** (Meta, 10 Aug 2026) trades wins with Qwen3.6-27B and reportedly clears it on
tool use, which is the axis this system cares about most. Unscored by Artificial Analysis at the time
of writing. The operator already has the methodology PDF.

## How to decide between them rather than guessing

`agent-score` (c4-788) exists for exactly this: 15 decisions with known-correct answers, measuring
protocol correctness, hallucinated state, and narration share. Coupled with `diffuse-chat`-style
runners it turns "feels better" into a number, and tool-use reliability is the property that
separates these models more than raw coding benchmarks do.

## Why the leaderboards mislead here

SWE-bench numbers depend heavily on the scaffold — the same model scores differently under a
different harness. Since this project IS the harness, a model's published score is a weak predictor
of its score here, and the local 15-case suite is the stronger signal.
