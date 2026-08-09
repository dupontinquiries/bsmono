# LOGSCOPE_TILED_PREFILL — how to use it and how to check it

## What it changes

Prefill attention was dispatching `ATTENTION_MH`: one thread per (row, head), each serially
streaming the whole K and V, no shared memory, no cross-row reuse. That is **quadratic in prompt
length** and it is where a sibling engine measured 188 ms of a 644 ms step at L=512.

`ATTENTION_MH_EXT_PAR` — tiled KV loop, online softmax over 2048-row tiles — was already in this
tree and only decode used it. The flag routes prefill to it. **No kernel was written; one call site
was changed.**

## Using it

```
LOGSCOPE_TILED_PREFILL=1 logscope-egui
```

Applies only when `head_dim <= 192`, which is the tiled kernel's own limit; above that the flag is
ignored and prefill takes the old path, so nothing needs to be conditional at the call site.

Off by default until you have run the gate on your hardware. Honeykrisp departs from the WGSL spec
in four measured ways, so a kernel that agrees on lavapipe is evidence about lavapipe.

## Checking it — correctness first

```
kernel-validate --gate-prefill-attn
```

Runs both kernels on identical deterministic inputs across four shapes — square, GQA 4:1, head_dim
128, and an odd length with MQA — and reports the worst relative difference.

**Bit-exactness is not expected and not the bar.** An online softmax accumulates in a different
order, so the question is whether the difference is at rounding scale or at wrong-answer scale. The
gate passes under 1e-4 and prints which shape was worst when it fails.

On lavapipe here: worst `1.19e-7`, four shapes, PASS. If your run disagrees, the two kernels differ
in their causal mask or in the `base`/`s` mapping, and the flag should stay off — the message says
so rather than leaving you to interpret a number.

## Then measuring — separately

Correctness and speed are different runs and the gate deliberately does not time anything.

```
# same prompt, both settings, compare prefill time in the engine's own log
logscope-egui                                 # baseline
LOGSCOPE_TILED_PREFILL=1 logscope-egui        # tiled
```

Expect the difference to **grow with prompt length** — it is a quadratic term being replaced, so a
200-token prompt may show nothing and a 4000-token one should show a lot. A measurement at one short
length that shows no change is not evidence against it.

## What it does not affect

* **Decode.** Already took the tiled kernel; unchanged.
* **KV cache size.** This is about traffic during prefill, not residency.
* **Models with head_dim > 192.** Falls through to the old path silently, by design.
