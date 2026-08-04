# Working around fast-math, properly

## What just happened, in one line

`amax / 7.0` on the CPU and `amax * f32(1/7)` on the GPU differ by one ULP, because Metal's default
fast-math lowers float division to a reciprocal multiply. Seven source-reading theories missed it; a
four-line probe found it.

## Why "normalize afterwards" cannot work

The instinct is to add a post-math step that fixes the result up. It cannot: **if the GPU's value is
one ULP off, no later step recovers the CPU's value without already knowing it** — and if you know it,
you should have uploaded it instead of computing it twice.

Post-processing can only remove information both sides already agree on.

## What does work: make the difference unrepresentable

Store the q4 block scale as **f16** rather than f32.

```
cpu scale f32 0x3EA16912  ->  f16 0x350B
gpu scale f32 0x3EA16913  ->  f16 0x350B    same
```

One ULP of f32 disappears under a 10-bit mantissa. And it costs nothing: **a 4-bit quantizer cannot
use f32 scale precision** — the levels are ±7, so scale precision far below the quantization step is
already wasted. Q4_K itself stores its scales as f16 for exactly this reason.

This is the general form of the fix:

> **Where two implementations must agree, choose a representation coarse enough that they cannot
> disagree within it.**

It is stronger than matching expressions, because it survives the next driver quirk. There will be
others — `rsqrt`, `exp`, `pow` and `inversesqrt` all have relaxed-precision variants under fast-math,
and none of them announce themselves.

## The three tiers, in order of durability

1. **Do not compute it twice.** Upload the CPU's scale, or read the file's blocks directly
   (`MATVEC_Q4K`). A value that exists once cannot diverge. Strongest, largest change.
2. **Coarsen until divergence is unrepresentable.** f16 scales. Cheap, local, survives future quirks.
3. **Match the expression.** Explicit `fma`, explicit reciprocal. Works until the next operation with
   a relaxed-precision lowering, which you will find the same expensive way.

The reciprocal fix in `_584` is tier 3. It is correct and it is the least durable of the three.

## What this does not touch

The grid-scale failures — 297430 words at 70000 super-blocks, 2771 across chunk boundaries — are a
separate bug. An ULP flips the handful of nibbles sitting on a quantization boundary, which is why the
small tensor differs by **zero** words with the same ULP present. A count that grows with grid size is
a dispatch or chunk-offset error and will not be fixed by anything on this page.
