# Logscope: Branching Chat + Compact Models

**Starting fresh? Read [`docs/STATUS.md`](STATUS.md)** — a complete overview of what's built, validated, and what's next. It's the handoff document for picking up mid-project.

---

# The ideas behind the tests

This engine has an unusually large test suite — `kernel-validate` alone runs well over a hundred
checks, and there are a dozen `--gate-*` suites on top. That's not test-coverage box-ticking. The
tests are the load-bearing argument for the whole project, and most of them encode an idea that's
worth understanding on its own.

## The thesis

The project is an experiment: **how far can a small local model go if you give it structure instead
of scale?** A frontier model is, in a sense, a monorepo — one enormous artifact doing retrieval,
reasoning, verification, and memory all at once. The bet here is that you can pull those apart and
hand them to cheaper machinery:

- a **conversation graph** (branch, prune, fork) instead of one long linear context,
- **many cheap samples + consensus** instead of one expensive sample,
- **deterministic verification** (a compiler, a rubric) instead of trusting the model's word,
- **a human in the loop** curating what stays in context,
- **tight retrieval** instead of stuffing everything into the prompt.

If that bet pays off, a 360M model on a laptop GPU plus good structure can do work you'd assume needs
something fifty times larger. The tests exist to prove the structure rests on a foundation that's
actually *correct* — because none of the clever stuff means anything if the matrix multiply is subtly
wrong.

## How the tests are built

Almost every test has the same shape: **a fast/cheap/clever path is proven equal to a slow/simple/
obviously-correct path.** The simple path is the oracle. "Prefix reuse gives the same logits as a full
recompute." "Speculative decoding gives the same tokens as plain greedy decode." "The int8 model
picks the same next token as the f32 model." This is a much stronger discipline than the usual ML
"the loss went down" — it's element-wise equivalence, to floating-point rounding (~1e-6).

That discipline is also why the codebase can be built from a small local model in the first place:
every load-bearing piece is a pure function tested in isolation, so correctness is checkable without a
GPU and without trusting the model that wrote it.

## The chapters

1. [Numerical ground truth](01-numerical-ground-truth.md) — why everything is checked against a CPU
   reference, and what that buys you.
2. [Prefix reuse and the conversation graph](02-prefix-reuse-and-branching.md) — the math that makes
   branching cheap, and why the helix you see is real, not decorative.
3. [Speculative and batched decoding](03-speculative-and-batched-decoding.md) — AI accelerating AI,
   and many conversations sharing one GPU.
4. [Consensus, fuzz, and verification](04-consensus-fuzz-and-verification.md) — the heart of the
   thesis: cheap samples + a deterministic judge beat one expensive guess.
5. [Quantization and the memory planner](05-quantization-and-memory-planning.md) — what actually fits
   on the hardware, and how the engine refuses gracefully instead of crashing.
6. [Autotuning and kernel specialization](06-autotuning-and-kernel-specialization.md) — why one tile
   can't tune a whole model, specializing a kernel with override constants, and keeping "fast" and
   "correct" as separate questions.
