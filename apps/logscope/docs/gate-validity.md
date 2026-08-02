# A gate is only as good as the device it ran on

## The evidence

`--gate-q6k` went green on lavapipe in _300 and I reported the kernel bit-exact. On an M2 Max it fails:

```
hand-checked block:  PASSES
1 block:             rel 1.75   gpu non-zero: 64 of 256
first divergence at index 4, then a repeated constant
```

Four faults were found and fixed against lavapipe — an out-of-bounds scale read, an ignored output
view, a double-divided workgroup count, a subnormal f16 exponent. All four were real. **And the kernel
is still wrong on the only hardware that matters.**

## Why lavapipe hides this class

Lavapipe is a *reference* rasterizer. It is more permissive and more nearly correct than a real driver,
so it validates the arithmetic and conceals everything about codegen, scheduling, memory visibility and
workgroup behaviour. The faults it catches are the ones you could have caught by reading; the ones it
misses are the ones you cannot.

This is the same shape as the `--gate-q4k` message that claimed "the loader can stop requantizing" when
it only tested dequantization: **a green result read as broader than its scope.** Two instances in two
days is a pattern, not an accident.

## What follows

* A gate result carries **the adapter it ran on**, and a claim derived from one adapter is a claim about
  that adapter. `gate-q6k`'s output already prints the adapter — it needs to be read.
* **Container-green is a precondition, not a verdict.** It is worth having: it catches arithmetic cheaply
  and fast. It must not close a question.
* The gates already run on hardware and disagree — `gate-matvec`'s subgroup rows fail on lavapipe *and*
  the subgroup kernel is disabled on Asahi and mobile. That disagreement was visible and treated as a
  lavapipe quirk. It was a signal about the class.

## The specific Q6_K signature, for whoever picks it up

`2 halves × 32 iterations × 4 writes = 256`, and exactly **64** land — one write per iteration rather
than four. The values *repeat* instead of being zero, so the writes collide rather than being skipped.
A single hand-checked block passes because it exercises only the first few indices, which is precisely
why that check could not have caught it.
