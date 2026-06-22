# 4. Consensus, fuzz, and verification

```
gate-rubric: all cases pass ✓ — scorer deterministic & alt-matching
  [metaphor-consistency] 4/4 (want 4/4) ✓ | ✓tower ✓storm ✓buried ✓triangulat
  [fn-reverse]           1/1 (want 1/1) ✓ | ✓rev
toolpipe FSM driver runs call->emit->done   OK
toolpipe allowlist blocks non-listed binary OK
```

This is the chapter the rest of the engine exists to enable. Everything so far — correct kernels,
cheap branching, fast batched decode — is infrastructure. Here's what it's for.

## Why cheap samples beat one expensive guess

A frontier model gives you one very good answer. The bet here is that **many mediocre answers plus a
way to tell which is right** can match it, and a small local model can produce the many answers nearly
for free (see the batching chapter). The catch is the second half: you need a *judge* that isn't just
another model you have to trust.

The pipeline:

1. **Fuzz** — `fuzz(input, N)` takes a prompt and perturbs its *framing* into N variants (reorder the
   ask, restate the constraint, change the emphasis) without changing what's being asked. In the app
   this is the `:fuzz N` command: it fans those N variants out as branches off the current node,
   queued to generate. Each is a cheap sample; the prefix-reuse trie makes them share history.
2. **Cluster + filter** — `consensus::cluster` groups the N answers by similarity, and `garbage_score`
   throws out the degenerate ones (empty, repetitive, off-topic). Agreement across independently-fuzzed
   prompts is signal: if eight of ten reframings converge on the same answer, that answer is robust to
   how you asked.
3. **Consensus** — pick the answer the cluster agrees on.

This is self-consistency / best-of-N, but built as a *tree* you can see and steer, not a hidden
sampling loop.

## The judges that don't require trust

Consensus is one verifier. The stronger ones are deterministic:

- **The rubric scorer** (`gate-rubric`) grades an output against explicit criteria, and — the
  important detail — it's **alt-matching**: a criterion can be satisfied by any of several acceptable
  phrasings (`triangulat` matches "triangulate" or "triangulation"). The gate proves the scorer is
  *deterministic* and that its alternative-matching is correct, including the negative cases (an
  answer that should score 0/4 does). A judge you can unit-test is a judge you don't have to trust.
- **The compiler** is the ultimate oracle for code. The model's output either compiles and passes the
  tests or it doesn't — no editorial judgment, no second model grading the first. The toolpipe tests
  (`call->emit->done`, the allowlist blocking unlisted binaries, stdin→`wc -l`) are the sandboxed
  execution layer that turns "the model wrote some code" into "the code ran and here's what it did."
- **The human**, finally, is the verifier of last resort. The prune/curate controls in the spatial
  view let a person cut the branches the automated judges couldn't separate, keeping the context lean.

## The shape of the bet

So "the AI" in this system is not one large model reasoning in a single pass. It's a small model
**sampled many times under perturbation**, its outputs **clustered for consensus** and **checked by
deterministic oracles** (a compiler, a testable rubric), with a **human steering the tree**. The
expensive, opaque "reasoning" of a giant model is replaced by a lot of cheap generation wrapped in
verification you can actually inspect.

Whether that fully substitutes for scale is the open question the project is built to probe — but
every piece of it is testable, and the tests above are the pieces.
