# Scout's 99.9% stall: what each finding contributes, and the chain that connects them

## The measurements, consolidated

```
turn 1   511 submits/tok   sync  74%   ceiling 3.90x
turn 2   972 submits/tok   sync  91%   ceiling 11.63x
turn 3  1915 submits/tok   sync 112%   ceiling unbounded
expert cache 46% -> 57% -> 69% as the pin set warms — and it bought nothing
```

Two facts do most of the work. **The stall is one blocking readback per layer**, from `moe_ffn_v1`
downloading router logits to the CPU. And **submits per token roughly double each turn**, which is
context growth, not routing — so the stall is a multiplier on conversation length rather than a fixed
tax.

The cache improving from 46% to 69% while nothing got faster is the cleanest evidence available that
caching is the wrong lever: **a cache does not shorten a stall.**

## What llama.cpp and Colibri contribute

**llama.cpp's `-ot "\.ffn_.*_exps\.=CPU"`** keeps expert FFNs on the CPU while attention goes to the
GPU. On unified memory the usual PCIe objection disappears, and — the part that matters — **the router
readback stops being waste, because the CPU needs those logits to do its own work.** The critical path
becomes a pipeline instead of a round trip.

**Colibri** is cited in this tree for tier scoring, `decode_batch.h`'s ragged shapes, and the GLM
indexer. Nothing about hybrid placement; claiming otherwise would be inventing it.

## The guard, and what it actually requires

```rust
t == 1 && m.stream.is_none() && !m.cfg.weight_before
```

Scout fails both conditions.

**`weight_before` is solved.** Llama-4 scales the expert INPUT and SwiGLU is nonlinear, so output
scaling cannot recover it — `MOE_PRESCALE` writes per-slot pre-scaled activations and the shader's
existing `xper` flag reads them. Behind `LOGSCOPE_MOE_PRESCALE=1`, unvalidated.

**`stream.is_none()` was thought to be structural. It is not.** The reason for it is that the routed
path selects on-device, so the CPU never learns which experts were chosen — and a streaming model
needs the CPU to know, to fetch them.

But that is a property of the **model**, and the real constraint is a property of the **layer**: *if
every expert for this layer is already pinned, any selection is a hit.* Nothing to fetch, nothing for
the CPU to learn. `layer_can_route` is that check, and the benefit is proportional — each qualifying
layer removes one readback.

## The chain

```
IQ1_S experts (2.88x smaller)
    -> the same pin budget holds all 16 instead of 6
    -> every layer fully resident
    -> layer_can_route passes
    -> on-device routing, no readback
    -> the 99.9% stall goes away
```

**The memory saving is the means, not the point.** Scout at UD-IQ1_S is 30.3 GiB against a 31 GiB
ceiling, which is interesting; the reason to want it is that it makes every layer routable.

## Order of work

1. **`LinKind::Iq1` end-to-end** — the variant, `MOE_MATVEC_IQ1_S` and the loader. Two of three exist.
2. **Wire `layer_can_route` into the guard**, per layer rather than per model.
3. **Validate the pre-scale** with `LOGSCOPE_MOE_V2_CHECK=1` on a weight_before model that fits.
4. **Then** hybrid CPU experts, if the stall survives all of that.

Step 4 is the llama.cpp idea and it is last on purpose: it is the largest change and the first three
may make it unnecessary.
