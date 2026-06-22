# 2. Prefix reuse and the conversation graph

```
anchor: prefix-fork logits == full prefill          max|gpu-cpu| = 1.907e-6   OK
anchor across prefill→decode boundary: reuse == full max|gpu-cpu| = 7.153e-7   OK
fleet trie (nested prefix) == per-worker             max|gpu-cpu| = 0.000e0    OK
stateful trie: cached 2 nodes, forked 4 branches, output==independent: true
```

## The idea

When a transformer reads a prompt, it computes a key and a value vector for every token at every
layer — the "KV cache." Here's the fact the whole conversation graph is built on: **those vectors
depend only on the tokens up to that position, not on what comes after.** So if two conversations
share the first 200 tokens, the KV cache for those 200 tokens is *identical* in both. You can compute
it once and reuse it.

The `anchor` test proves exactly this. Take a prompt, fork it after a shared prefix, and compute only
the new suffix on top of the cached prefix. The resulting logits match a full from-scratch recompute
to `1.9e-6` — the noise floor. The shortcut is the honest computation. There's a second test proving
it still holds across the prefill→decode boundary (the transition from "process the prompt" to
"generate one token at a time"), which is the place this kind of optimization usually breaks.

Then it goes one step further: a **trie** of prefixes. If you have a whole tree of conversations that
branch and re-branch, you cache the shared prefixes at the fork points and every branch reuses
everything above it. The `fleet trie (nested prefix)` test confirms a nested tree of shared prefixes
produces the same output as running each branch independently — to `0.0`.

## Why the helix is real

The 3D spatial view in the app — the protein-fold helix of branching turns — is not a metaphor laid
over a flat chat log. **Each branch in that tree literally reuses its ancestors' KV cache**, and the
trie tests prove that reuse is free of charge: forking costs only the new tokens, not a recomputation
of the shared history.

That's what makes the central experiment affordable. "Explore ten branches of a conversation with a
small local model" sounds expensive — ten conversations. But nine of those ten branches share most of
their history with the others, and the trie means you pay for the shared part once. Branching, in
this engine, is closer to `O(new tokens)` than `O(whole conversation × branches)`. The prune button
(✂) is the other half: when you cut a dead branch, you free its slice of the cache and its budget.

So the graph isn't a visualization choice. It's the data structure the KV cache already wants to be,
surfaced to the human who can then steer it — fork the promising line, prune the dead one, and never
pay twice for the shared trunk.
