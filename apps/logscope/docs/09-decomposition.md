# 09 · Prompt blending, sub-function chainloading, and multi-model coordination

*This is the "reason the rest into something interesting" doc. It takes the idea — get a model to write
sub-functions you chainload through shorter sidechannels; a large model scaffolds, a small model fills;
maybe coordinate multiple models — and sorts it into what's buildable now (some of it is, and it's in this
build), what's a known technique, and what's a genuine research problem. The throughline: this is the same
"small model + structure" bet, and the KV prefix tree (docs/08) turns out to be the substrate that makes
it cheap.*

## 1. The sidechannel intuition is right — and the KV tree is what realizes it

"Chainload sub-functions through shorter sidechannels." The tension you're feeling is: a model writing a
whole program holds one long, growing context; that's expensive and the small model can't carry it. The
resolution isn't to *truncate* each sub-task's context — it's to make the shared part **free**.

That's exactly what the prefix tree does. Decompose a skeleton into one sub-task per hole; every sub-task
prompt embeds the *same* skeleton and differs only in a short trailing "implement THIS body" line. The
skeleton is the shared prefix → prefilled once, cached, reused; each sub-task's actual compute is just the
short divergent suffix. So the **sidechannel is short (cheap compute) even though each sub-task still sees
the full context (good fills)**. You don't trade context for cost — the KV tree buys you both. This is now
real: `:fill` (decompose.rs). It finds the `todo!()` holes, fans a sub-task per hole (KV-shared), and
`:assemble` splices the filled bodies back. The pure parts are tested; the fan + splice ride the machinery
we already built.

## 2. Large scaffolds, small fills — why the scoping is the whole point

Your GUI example: the large model writes `main`, the struct defs, the trait, the loop *shapes* — the hard
global-coherence part — leaving `todo!()` bodies. The small model then gets, per hole, a task with every
type and signature in view and exactly one body to produce. A 360M model that cannot write a coherent
1000-line Rust GUI **can** often write "the body of `fn render_row(&self, ui: &mut Ui, row: &Row)`" when
the surrounding types are fixed. Structure converts a problem the small model fails at into a list of
problems it can do. That's the thesis in one sentence, and decomposition is its sharpest form: the large
model spends its capability on *structure*, the small model spends throughput on *filling*.

This is a real, named pattern (program-sketching / skeleton-guided synthesis); what's new-ish here is doing
it over a live conversation graph with a KV-shared fan and a cheap local model, rather than one big model
doing both jobs.

## 3. "Loop and keep track of outputs" — the orchestration

The loop is already here in pieces: the queue fans sub-tasks, the fan state machine (`:Nca`/`:Nfa`) deepens
a frontier round by round, and `:fill`/`:assemble` track holes → bodies. The missing closing piece is
*judgement in the loop*: after a fill, does it compile? The natural shape is

```
skeleton → fan one sub-task per hole → (each body) cargo check the spliced result
        → keep the bodies that compile, re-fan the holes that don't (different sample / the large model)
        → assemble when every hole is green
```

That "keep the green, retry the red" is the same verify-all `cargo check` oracle that's queued for `:best`
and for bounding `:fa`'s width×depth blow-up. One oracle unlocks three things: real `:best` regions,
top-K pruning for fan-out, and per-hole retry for fill. That's the single highest-leverage next build, and
it's what turns this from "fan and hope" into a loop that actually converges.

## 4. "A hybrid model with more attention pathways coordinating between models" — the honest split

This is the part to be careful about, because it bundles a tractable idea with a hard research problem.

**Tractable now — coordinate at the token/context level.** Two models don't need to share attention to
cooperate; they can hand off through shared *text/tokens*. Model A emits a skeleton (tokens); model B
consumes those tokens and emits bodies. The engine already supports this: it can load and switch models,
and the worker fabric (`run_tokens`) is "tokens in → tokens out, KV stays local," which is precisely a
clean hand-off boundary. A coordinator/router that dispatches each sub-task to whichever model suits it
(small for easy fills, large for the holes that failed verification) is buildable on what exists — the KV
tree is the shared substrate, the token stream is the interface. This is the pragmatic version of "multiple
models coordinating," and it's where I'd build.

**Known technique — speculative decoding.** The token-level form of large+small: the small model drafts a
chunk of tokens, the large model verifies them in one batched forward and accepts the longest correct
prefix. Same quality as the large model, much of the speed of the small one. It's the dual of
decomposition (there, large scaffolds / small fills; here, small drafts / large checks) and it's a real,
well-understood algorithm — a strong candidate engine feature, and it composes with the KV cache.

**Research-hard — actual cross-model attention.** "More attention pathways that span models" means letting
model B attend into model A's hidden states. The obstacle is representation alignment: two independently
trained models have *different, unaligned* latent spaces — B's attention can't read A's K/V because they
don't mean the same thing. Making that work is an open area, and the things that *do* exist are essentially
ways to dodge or learn the alignment:
- **Mixture-of-Experts** — many expert FFNs under one model with a router (one shared latent space, so
  attention is shared by construction). This is the realistic "more pathways, coordinated" design, and
  it's already the highest-value model target (Qwen3-Coder-30B-A3B, docs/07).
- **Model merging / weight averaging** — fuse models in weight space (needs compatible architectures).
- **Cross-attention adapters / bridges** — *train* a small module that projects A's states into B's space.
  This is the literal "hybrid with extra attention pathways," but it requires training, not just wiring.

So: coordinate at the token level now (router + fabric + KV tree), reach for MoE for the "one model, many
coordinated pathways" version, and treat literal cross-model attention as a training project, not a config
change. The interesting, buildable frontier is #3's verify-in-the-loop plus token-level model dispatch —
everything there is reachable from what this repo already has.

## What's in this build vs. what's next

- **Built:** `:fill` / `:assemble` (decompose.rs, pure parts tested) — skeleton → KV-shared sub-task fan →
  splice. Rides the prefix tree so the shared skeleton is prefilled once.
- **Next (one oracle, three payoffs):** per-node `cargo check` → keep-green/retry-red for `:fill`, top-K
  pruning for `:fa`, real verdicts for `:best`.
- **Then:** speculative decoding (small drafts / large verifies) and a token-level model router over the
  fabric. MoE as the model-level "coordinated pathways."
