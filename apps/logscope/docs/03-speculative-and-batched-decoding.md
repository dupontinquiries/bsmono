# 3. Speculative and batched decoding

```
speculative(K=1) == target greedy generate_kv = true
speculative(K=2) == target greedy generate_kv = true
speculative(K=4) == target greedy generate_kv = true
speculative decode == target greedy            max|gpu-cpu| = 0.000e0   OK

kv-budget tight serializes (peak==1)           OK
kv-budget roomy runs all concurrently (peak==3) OK
int8-KV ~quadruples budget (KV >=3.4x smaller) OK
continuous batching == per-request             max|gpu-cpu| = 0.000e0   OK
WorkerPool == run_batched                      max|gpu-cpu| = 0.000e0   OK
```

## Speculative decoding: AI accelerating AI

Generating tokens one at a time is slow because each token needs a full forward pass, and a forward
pass barely uses the GPU — you're memory-bound, not compute-bound. Speculative decoding exploits the
slack. A cheap **draft** proposes the next K tokens; the expensive **target** then verifies all K *in
a single batched pass* (which costs almost the same as verifying one), and accepts the longest prefix
the target agrees with. When the draft is right — which for easy tokens it usually is — you got K
tokens for the price of one pass.

The beautiful part is that **it changes nothing about the output**. The test proves
`speculative(K=1,2,4) == target greedy` to `0.0` — bit-identical. It is not an approximation, a
sampling trick, or a quality tradeoff; the target has the final say on every token, so the result is
exactly what plain greedy decoding would have produced, just faster. This is the cleanest example of
the suite's whole philosophy: a clever fast path, proven equal to the simple slow path.

## Batching: many conversations, one GPU

The "many cheap samples" idea (next chapter) only works if you can actually *run* many generations at
once without falling over. That's what the serving layer does, and it's tested the same way —
equivalence to running each request independently.

- **Continuous batching** packs multiple in-flight conversations into one set of GPU dispatches, and
  proves the batched output equals per-request output (`0.0`).
- The **KV-budget coordinator** is admission control for memory. The KV cache is the thing that runs
  out, so the coordinator decides how many sequences can run concurrently given the budget: under a
  *tight* budget it serializes (the test confirms a peak concurrency of 1), under a *roomy* budget it
  runs all three at once (peak 3) — and the output is identical either way. Backpressure that doesn't
  change answers.
- **int8 KV** stores the cache in 8-bit, which is `>=3.4×` smaller, so the same hardware holds
  roughly four times as many concurrent sequences. (The cache is lossy here — that's covered in the
  quantization chapter — but the budget math is exact.)

Put speculative decoding and batching together and you get the engine the thesis needs: a small model
that decodes fast *and* fans out to many simultaneous branches, all on one laptop-class GPU, all
proven to produce the same tokens they would have one-at-a-time.

## Reaching it from the CLI: batched fan-out

The pieces above are engine capabilities; the headless CLI now actually calls them. `Backend` grew a
`generate_batched(prompts) -> replies` method (default: a sequential loop, so the mock and any other
backend stay correct), and the wgpu engine overrides it to run the prompts through `run_batched` — it
takes their longest common token prefix, prefills that once, and decodes every branch's divergent
suffix together. The runner's `:fa` (fan-out) and `:ca` (continue-all) collect a whole round's branches
and make a single batched call instead of looping one generation at a time. Since a batch-1 decode
barely uses the GPU, decoding eight fan branches together costs little more than decoding one — which
is the point of a branching harness. The output equals the sequential fan (the engine proves
batched == per-request to `0.0`); at temperature 0 it is bit-identical, and above it each branch draws
from its own RNG so the tokens differ while the distribution does not. Speculative decoding is the
remaining lever and now has its substrate — the multi-model pool — but is not yet wired to a command.
