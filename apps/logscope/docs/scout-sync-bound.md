# Scout is 99.9% CPU-wait, and the reason is one guard

```
sync:  99.9% (198527.7 ms)      submit: 0.1% (118.2 ms)
38638 submits (2414/tok), 537 polls
expert cache 46% resident (2085 hits / 2451 fills, 170.2 GiB moved)
bind-group reuse 15%
```

**2414 submits per token across 48 layers is ~50 submits and a blocking readback per layer.** The GPU
is idle for essentially the whole generation.

## The mechanism

There are two MoE paths. One downloads the router logits to pick experts on the CPU:

```rust
moe_ffn_v1:   let logits = self.download(&m.router.apply(self, n2, t));   // blocking, per layer
```

The other keeps routing on the device and never stalls:

```rust
moe_ffn_gpu_routed_inner:  let logits = m.router.apply(self, n2, 1);      // no download
```

The GPU-routed path is guarded:

```rust
if t == 1 && m.stream.is_none() && !m.cfg.weight_before
```

**Scout fails two of the three conditions.**

`m.stream.is_none()` — Scout streams experts, so it is excluded.

`!m.cfg.weight_before` — Llama-4 scales the expert **input** by the routing weight rather than its
output, and SwiGLU is nonlinear, so `silu(w·g)·(w·u) ≠ w·silu(g)·u`. The routed path broadcasts one
activation row across slots; supporting `weight_before` needs per-slot scaled activations. The comment
in place already says this is "a real change rather than a flag".

## Which reorders every other lever

The expert cache at 46% and bind-group reuse at 15% are both worse than they should be, and **neither
matters while 99.9% of the step is a stall**. A cache does not shorten a wait. The engine's own verdict
line says it: *"the win is removing readbacks, not caching"*.

Two of the three fixes this session — pinning more experts, capping batch width — act on the 0.1%.

## What would actually move it

**Per-slot scaled activations in the routed path**, which lifts the `weight_before` restriction and is
the piece Llama-4 needs. That is a real change to how the routed path builds its inputs, not a flag.

**Routed streaming**, lifting `m.stream.is_none()`. Streaming needs to know which experts to fetch,
which is what the CPU readback currently provides — so this needs the device-side selection to drive
the fetch, which is the harder half.

Either one alone leaves Scout on the stalling path. **Both are required**, which is presumably why
neither was done.

## The 46% expert hit rate, in context

Against a 6/16 pin the uniform baseline is 37.5%, so 46% is barely above chance — the LFRU policy is
*not* tracking the routing well. That is worth fixing, and it is worth perhaps 5% of a step that is
99.9% stall. It is the right answer to the wrong question.
