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

**`--gate-moe-v2` does not exist — I invented it.** I read `LOGSCOPE_MOE_V2_CHECK` in the source and
wrote down a matching gate name without checking the flag list, which is the same
assert-then-don't-verify move that has cost this session five wrong kernel theories.

The real mechanism is the env var, and it is enough: `LOGSCOPE_MOE_V2_CHECK=1` runs `moe_v2_check`,
which computes `moe_ffn_v1` and the routed path on the same inputs and compares them inline during a
real generation. That is a better check than a gate would be, because it uses the model's own
activations rather than a fixture.

The MoE gates that do exist: `--gate-moe-raw`, `--gate-moe-repin`, `--gate-moe-stream`,
`--gate-moe-fusion`, `--gate-moe-prefetch`, `--gate-moe-cpu`. A wrong weight placement here is not a crash — it is
plausible text from wrong math, which is the failure this codebase has spent the most time on.


---

# Correction: the passing llama4 gate does NOT cover the routed path

I read `moe llama4 gating (raw-sigmoid top-1, weight-before-FFN, + shared) = 9.537e-7 OK` in a full
suite run and said there was already a validated GPU path doing Llama-4's weight-before gating —
"a better starting point than I assumed".

**It runs at `t = 3`.** `let (d, ff, sff, ne, t) = (8, 6, 10, 4, 3);` — three tokens, which is the
prefill-shaped path. The routed decode path is gated on `t == 1`, so the gate never enters it.

So the gate proves that `moe_ffn_t` handles `weight_before` correctly for a multi-token batch. It
proves nothing about the single-token routed path, which is the one Scout spends 99.9% of its step
avoiding.

## But it does give a reference implementation

That is the useful part of the correction. Something in the `t > 1` path already scales the expert
input by the routing weight and matches CPU to 1e-7. **Whatever it does is the thing the routed path
needs to do at `t == 1`** — and it is validated, so it is a specification rather than a guess.

The pre-scale described above should be written against that code, not invented. And
`LOGSCOPE_MOE_V2_CHECK=1` compares v1 to the routed path on real activations, which is the test that
would catch a mistake in porting it.

## The pattern, again

A gate name that says "llama4 gating, weight-before" reads as covering Llama-4's gating with
weight-before. It covers it *at one token count*, and the count is the thing that decides which path
runs. **The name describes the inputs and the coverage depends on a parameter buried in a tuple.**

That is the fourth time this week a green check turned out to be green about something adjacent:
`--gate-vram` measured allocation and was read as residency, `--gate-q6k` was red for a fixture, the
A/B said "inconclusive" when the levers were unreachable, and now this.
