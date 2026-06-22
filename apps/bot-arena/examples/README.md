# bot-arena proposer stack — operator guide

This directory holds the *model seam*: the scripts that turn bot-arena from a
random-mutation search into a model-driven coding ecosystem. The engine is
model-agnostic — it speaks one protocol and you plug models in behind it.

## The protocol

Every proposer is a command that reads a JSON **context** on stdin and writes a
**unified diff** on stdout (or nothing, meaning "no change"):

```
stdin  <- {"test_cmd": "...", "failing": ["test_a", ...],
           "files": [{"path": "...", "content": "..."}],
           "knowledge": [...],   // optional, added by a retriever stage
           "plan": "..."}        // optional, added by a planner stage
stdout -> a git-apply-compatible unified diff
```

The engine gates every diff with `git apply --check`, so a malformed or irrelevant
patch is a harmless no-op — never a crash, never corruption.

## The pieces

- **proposer_llamacpp.py** — runs a local llama.cpp GGUF model as a proposer. Set
  `LLAMA_MODEL=path/to/model.gguf` (e.g. Qwen2.5-Coder). This is the strong tier.
- **proposer_tinygpt.py** — runs a small char-level transformer (the `tiny_gpt` in
  `../pipeline`) trained on your own harvested fixes. Fast, local, recalls recurring
  patterns. This is the cheap tier-0.
- **retriever_experience.py** — a context→context enrichment stage (RAG): pulls
  relevant prior fixes from `experience.jsonl` and symbol definitions from the repo
  into a `knowledge` field, so a small model reasons over supplied facts instead of
  thin baked-in memory.
- **experience_to_corpus.py** / **train_local_proposer.sh** — the distillation loop:
  turn harvested wins into a training corpus and train the tinygpt proposer.

## The cascade (race for speed)

Run the cheap model first; escalate to the strong one only where the cheap one
doesn't fully solve it. The engine's own tests are the router, and it reports the
split (`[cascade] tier 0: N%, tier 1: M%`).

```
bot-arena --headless -w ./repo --backend local \
  --proposer-cmd "TINYGPT_TOK=lp/tok.bin TINYGPT_MODEL=lp/model.bin python3 examples/proposer_tinygpt.py" \
  --escalate-cmd "LLAMA_MODEL=qwen2.5-coder-1.5b.gguf python3 examples/proposer_llamacpp.py" \
  --context-cmd "python3 examples/retriever_experience.py" \
  --differential --min-fitness 1.0 --json
```

`--context-cmd` stages run once and feed every tier. To map (but not yet build) a
lead-model planner, add another `--context-cmd` that injects a `plan` field.

## The distillation loop (the cheap tier gets smarter)

1. Run the arena with a strong proposer. Every winning patch is appended to
   `~/.local/share/codebot-arena/experience.jsonl`.
2. `examples/train_local_proposer.sh experience.jsonl lp/` distills those wins into a
   tinygpt model (`lp/tok.bin`, `lp/model.bin`).
3. Add that model as `--proposer-cmd` (tier 0). It now handles recurring patterns for
   free; novel cases escalate to the strong model.
4. Re-run step 2 as `experience.jsonl` grows. The `[cascade]` tier-0 share is a direct
   measure of how much of your recurring work the cheap local model has absorbed.

## Honest notes

- **You supply the real models.** The llama.cpp path needs a GGUF on your box; these
  scripts are verified end-to-end against stubs, with tinygpt actually trained locally.
- **tinygpt is tiny.** It recalls trained patterns, it does not reason about novel
  code. Byte-exact diff reproduction needs the char-level recipe in
  `train_local_proposer.sh` (BPE causes token doubling) and ~1000+ training steps;
  under-training degrades gracefully because the gate escalates.
- **Retrieval precision matters.** Irrelevant `knowledge` pollutes a small model's
  context. Measure with/without `--context-cmd` (compare solve rate and tier split)
  rather than assuming it helps.

## Measuring where training time goes (TINYGPT_PROFILE)

To decide whether GPU matmul offload is worth it for *your* model size, train with
`TINYGPT_PROFILE=1` and read the hot-path breakdown tiny_gpt prints at the end:

    TINYGPT_PROFILE=1 tiny_gpt train corpus/ tok.bin model.bin 1000 <dim> <layers> <heads> <ctx> ...

It reports the **dense-matmul share of compute** and the **Amdahl ceiling** — the most a
GPU could speed up the whole training step even if GEMM became free. Observed here:

| config | matmul share | speedup ceiling |
|---|---|---|
| dim=48, 2 layers, ctx=160 | 51% | 2.0× |
| dim=128, 4 layers, ctx=128 | 81% | 5.2× |

Matmul share rises with model size, so the GPU upside grows with scale — but it is
Amdahl-bounded, not 1000×. The profile also prints the exact per-layer GEMM shapes; feed
those `M N K` to the `gpu-gemm` probe on your GPU to estimate the achievable matmul speedup,
then combine the two numbers: realistic step speedup ≈ ceiling × (matmul fraction actually
captured). The rest (attention, layernorm, softmax, embeddings, Adam) is untouched by a
GEMM-only offload.
