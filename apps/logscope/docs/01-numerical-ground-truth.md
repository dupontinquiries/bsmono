# 1. Numerical ground truth

```
gemm                   max|gpu-cpu| = 1.669e-6   OK
softmax                max|gpu-cpu| = 1.118e-8   OK
rmsnorm                max|gpu-cpu| = 1.192e-7   OK
attention (causal)     max|gpu-cpu| = 1.192e-7   OK
rope (interleaved NORM) max|gpu-cpu| = 2.384e-7  OK
transformer block (e2e) max|gpu-cpu| = 5.722e-6  OK
```

## The idea

Every kernel in the engine has a twin: a plain, readable CPU implementation of the *same* math. The
test runs the GPU kernel and the CPU reference on the same inputs and measures the largest
element-wise difference. The numbers above are all at the floating-point noise floor — `1e-6` to
`1e-8` is just the difference between doing the arithmetic in a different order. They aren't "close
enough," they're *the same function*.

This sounds obvious and isn't. The normal way to know a transformer works is indirect: you train it,
the loss goes down, the samples look plausible. That tells you the *system* is roughly right; it tells
you nothing about whether your attention kernel has an off-by-one in the causal mask, because the
model will happily learn around small bugs. Here the claim is exact and local: this softmax computes
softmax, this RoPE computes RoPE (interleaved, NORM convention — there's a separate test for the
freq-scaling variant), and the whole transformer block end-to-end reproduces the reference to 5e-6.

## Why it's the foundation

The CPU reference is the **oracle**, and once you have an oracle, every harder claim becomes an
equivalence check against it rather than an act of faith. Read the rest of the suite and you'll see
the same move everywhere:

- "prefix reuse == full recompute" — *is the shortcut the same as the honest computation?*
- "int8 model argmax == f32 model argmax" — *did quantization change the answer?*
- "speculative decode == greedy decode" — *did the speedup change the output?*

None of those questions is answerable unless you already trust the base math. Numerical ground truth
is what lets the project be aggressive everywhere else: you can reorder, cache, quantize, batch, and
fuse, and each time you just prove the result still equals the oracle. A subtle 1e-3 drift that would
be invisible in a "looks plausible" eval shows up immediately as a failed bit-exactness check.

It's also what makes a *small model* a viable author for this code. You don't have to trust that the
model got the kernel right — the equivalence test does. Correctness is mechanical, not editorial.
