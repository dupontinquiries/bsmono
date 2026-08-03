# Putting Scout on the routed path — the question is resolved, the change is not made

Scout spends **99.9% of its step in `sync`**: `moe_ffn_v1` downloads the router logits to pick experts
on the CPU, one blocking readback per layer, 48 per token. The routed path never stalls and is gated:

```rust
if t == 1 && m.stream.is_none() && !m.cfg.weight_before
```

Scout fails two conditions. This resolves the second.

## What the shader does and does not do

`MOE_MATVEC_GLU_Q4` reads `selw` for exactly one purpose:

```wgsl
let expert = selw[slot];                         // the INDEX only
let abase  = select(0u, slot*d.k, d.xper==1u);   // per-slot activation base
```

**The routing weight is never read in the shader.** It is applied downstream, to the expert's output —
which is `weight_after` semantics and correct for Qwen and GLM.

Llama-4 needs the weight on the expert **input**, and SwiGLU is nonlinear, so the two are genuinely
different: `silu(w·g)·(w·u) ≠ w·silu(g)·u`. Scaling the output cannot recover it.

## The half that already exists

`xper == 1` makes each slot read its own activation row at `slot*d.k` instead of sharing row 0. **The
shader is already capable of per-slot inputs** — nothing there needs changing.

What is missing is the caller: it passes one unscaled activation row and lets the weight apply later.

## The change, and why top-1 makes it small

For Llama-4 the routed path needs, before the GLU dispatch:

1. scale the activation row by the routing weight from `selw`;
2. set `xper = 1` so the shader reads the scaled rows;
3. suppress the downstream output weighting, which would otherwise apply it twice.

In general that is one scaled row per slot — the "per-slot scaled activations" the existing comment
calls "a real change rather than a flag".

**Scout routes top-1.** One expert per token per layer, one slot, one weight. So it is a single scale
of a single row: one small dispatch, not a restructuring. The guard should therefore read

```rust
(!m.cfg.weight_before || m.cfg.n_used == 1)
```

with the pre-scale wired for that branch.

## What is still open

`m.stream.is_none()` is the other condition, and it is the harder one: streaming needs to know which
experts to fetch, and the CPU readback is currently what tells it. Device-side selection has to drive
the fetch before streaming can leave that path.

**Both must lift together for Scout.** Removing `weight_before` alone leaves it on `moe_ffn_v1`
because it streams. That is presumably why neither was done — and it means the pre-scale is necessary
but not sufficient, which is worth knowing before building it.

## Do not skip the verification

`--gate-moe-v2` exists to compare `moe_ffn_v1` against the routed path on the same inputs, and
`LOGSCOPE_MOE_V2_CHECK=1` runs it inline. A wrong weight placement here is not a crash — it is
plausible text from wrong math, which is the failure this codebase has spent the most time on.
