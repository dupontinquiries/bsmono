# The prefill kernel gap: q8_0 has a GEMM, q4 does not

Measured on an M2 Ultra: Qwen3-30B-A3B, 1625-token prompt, **31.72s prefill** — about 51 tok/s.

## What the two paths actually are

```
m == 1        MATVEC_Q4          one row
m <= 8        MATVEC_Q4_SMALLB   reads each nibble once, accumulates all m rows
m >  8        LINEAR_Q4          strided — RE-READS THE WEIGHTS PER TILE
```

Prefill runs at `m = 1024`, so it takes the third. The in-tree comment on the small-batch kernel says
exactly what the third one costs: it exists to keep *"Q4's halved weight-bandwidth advantage instead of
the strided LINEAR_Q4 re-reading weights per tile"* — and then that advantage is given back above m=8.

Meanwhile `q8_0` has `linear_q8_0_gemm_rb`, a register-blocked GEMM measured at **2795 GFLOP/s**,
described in-tree as **2.8× the tiled kernel and 5.6× naive**, and it is the *default* — the tiled
variant is the opt-in escape, not the fast path.

**So a Q4_K model prefills on the kernel a Q8_0 model escaped from.** Q4_K_M is the format almost every
checkpoint ships in, which makes this the common case rather than an edge.

## Why it looks fine

Nothing is broken. The output is correct, the profile reports the GPU busy, and decode — which runs at
m=1 and uses `MATVEC_Q4` — is unaffected. The cost is confined to prefill, appears once per turn, and
is easy to attribute to "long prompt" rather than to the kernel.

It also does not show in the submit-cause breakdown, because the strided kernel is not doing *more
submits* — it is doing more *bandwidth per submit*.

## What the references do

llama.cpp does not have this split: its `mul_mat` dispatches by quantization type into the same
batched shape for every format, so Q4_K prefill and Q8_0 prefill differ in dequantization cost and not
in memory traffic pattern.

Colibri holds everything as int8-per-row and dequantizes on use inside one matvec — a single path, so
the question does not arise. That is the design consequence of converting to ONE canonical compute
format at load, which this engine deliberately does not do (it keeps q4 to halve the bytes).

Keeping q4 is the right call for decode, which is bandwidth-bound at m=1. The gap is that the same
decision was never revisited for prefill, which is compute-bound at m=1024 and has the opposite
tradeoff.

## The fix, and its size

A register-blocked q4 GEMM, mirroring `linear_q8_0_gemm_rb`: hold a tile of `a` in registers, read each
weight nibble once, accumulate across the tile. The q8_0 version already proves the shape works on this
hardware and compiles on every backend — the q4 version differs only in unpacking two weights per byte.

Gate it against the existing `MATVEC_Q4` reference the way `--gate-matvec` already does, and **run that
gate on hardware**: this codebase has twice shipped a kernel that was bit-exact on lavapipe and wrong on
an M2.

Expected: prefill moves from bandwidth-per-tile to bandwidth-once, so the ceiling becomes the same
compute bound q8_0 already reaches. Whether it lands near 2795 GFLOP/s is a measurement, not a promise.
