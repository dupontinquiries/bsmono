# 6. Autotuning and kernel specialization

```
gate-gemm-spec: GEMM tile candidates (device-limited): [8, 16, 32]
  [✓] override constants yield one distinct pipeline per tile
  [✓] GEMM all tiles match naive within tol [256×128×256] (max|Δ|=1.53e-5)
  [✓] GEMM tiles [8, 16, 32] BIT-EXACT to each other [256×128×256]
  [✓] GEMM tiles persist + reload into a fresh context

gate-autotune-cache:
  [✓] decode_tile_for falls back to the global profile for an unknown shape
  [✓] cached tile routes through decode + is bit-exact to default (speed-only)
```

## One number can't tune a whole model

A GPU kernel has knobs — how many output columns a workgroup handles, how wide its threadgroup is,
how big a tile it stages in shared memory. The right setting depends on the *shape* of the matrix.
A decode mat-vec over a narrow attention projection (k=5120) and one over a wide FFN-down (k=13824)
do not want the same tile: the wider the matrix, the more the activation vector is re-read, and the
more a larger tile (which reuses each activation across several output columns) pays off.

The first autotuner got this wrong in an instructive way. It resolved **one global tile** for the
whole GPU by benchmarking whatever shape it happened to see, and on the small models it was first
exercised with, the winner was `tile=1`. That number then starved the 14B's wide FFN of any reuse —
a setting that was optimal for a 360M model and pessimal for a 14B. The lesson is that autotuning has
to be **keyed by shape**, not by device. A model has only a handful of distinct mat-vec shapes
(attention QKVO, the fused FFN gate/up, FFN-down, the output projection), so the fix is cheap: probe
each distinct shape once at load, cache the best config per `(n, k)`, and look it up on the hot path.

## The config must never change the answer

Here the codebase's central discipline does real work. A tuning knob is only legitimate if it changes
*speed* and nothing else — so the gates assert exactly that. The decode mat-vec is **bit-exact** across
all its tile sizes (`max|Δ| = 0`): tiling reuses activations from shared memory but accumulates the
same products in the same order, so the output is identical bit-for-bit. That is a sharper claim than
"close enough," and it means the autotuner can pick any tile it likes with zero correctness risk —
the cache is a pure performance layer.

Writing that gate also *caught a real bug*. The tiled mat-vec's accumulator was sized for tiles up to
8; nothing enforced it, and an override path briefly allowed 16, which read past the end of the array
and produced `2e24` garbage. It had stayed hidden because the autotuner only ever selected {1,2,4,8},
so the kernel had never run at a larger tile in production — the multi-tile correctness gate was the
first thing to exercise it. The override is now clamped to the kernel's real maximum. This is the
payoff of element-wise checking: a latent out-of-bounds in a kernel that "worked" got surfaced by a
test that simply asked whether every tile agrees with every other.

## Specializing a kernel without rewriting it

Autotuning the prefill GEMM needs *many* versions of one kernel — a 16×16 tile, a 32×32 tile, an 8×8
tile — to benchmark against each other. The wrong way is to string-template the WGSL per tile: the
pipeline cache is keyed by the shader source's address, so a freshly formatted string recompiles every
call and the cache never hits. The right mechanism is a **pipeline-overridable constant**. The kernel
declares `override TILE: u32 = 16u` and uses it in `@workgroup_size(TILE, TILE, 1)` and its loops; the
shader is authored *once*, and each pipeline is specialized at creation time by passing a different
`TILE` value. The workgroup arrays are sized for the largest tile and a smaller tile uses the leading
corner. The gate confirms the mechanism end to end: one distinct pipeline per tile from a single
source, every tile matching the naive GEMM within float tolerance (`1.5e-5` — the only difference is
the order a scalar multiply associates, `a·(s·q)` vs `(a·s)·q`), and — because the k-loop still runs
in order — every tile bit-exact to every other.

## Tune once, reuse everywhere

The choice of tile depends on the GPU and the shape, but *not* on the weights — so the cache is
content-independent and can be written to disk keyed by the adapter, exactly like the int8-weight
cache. A second launch loads the tuned tiles instead of re-probing; and because the key is the shape,
two *different* models that share a layer shape share its tuned config. That last property is what
makes the cache the right substrate for running several models at once: you tune a shape once, and
every model built from that shape benefits.

## What's proven here, and what isn't

The honest line runs straight through this chapter. The software renderer used for in-sandbox tests
produces *correct* numbers but meaningless *timings*, so none of these gates claim a speedup. What
they prove is the machinery: the autotuner enumerates the right shapes, picks a config, the config
routes correctly through the kernel, and it survives a round-trip to disk. *Which* tile is fastest is
measured on the real hardware (Apple M2 via Vulkan) and nowhere else — the autotuner runs its probes
there, logs what it chose, and persists it. Correctness is settled in the sandbox; performance is an
empirical question answered on the metal. Keeping those two apart — never letting a plausible-looking
speedup stand in for a measured one — is the same discipline that lets the whole engine be built and
trusted in the first place.
