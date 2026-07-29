# CPU path measured on the M2 Ultra — and what it says about the GPU

First real numbers from the 24-core side. Three of them change decisions.

## The headline: the CPU is within ~2x of the GPU, and 24x of what the GPU delivers

```
GPU matvec (gate-perf, best row) : 258.1 GiB/s
CPU matvec (24 threads)          : 113.2 GiB/s   = 44% of the GPU
memory bandwidth ceiling         : ~745 GiB/s    CPU is at 15% of it
```

Per token on Scout, whose active weights are ~10.5 GiB:

| | ms/token | tok/s |
|---|---|---|
| GPU at its measured best | 40.7 | 24.6 |
| **CPU at 24 threads** | **92.8** | **10.8** |
| GPU as actually delivered | 2232 | 0.45 |

The middle row is the one that matters. **The CPU path as written would be roughly 24x faster than the
GPU path as it currently performs** — not because the CPU is fast, but because the GPU path is losing
almost everything to submit overhead and fences. A decoder at 10.8 tok/s is usable; one at 0.45 is not.

That does not mean abandoning the GPU. It means the CPU is a real fallback rather than a curiosity, and
that the GPU's remaining 24.6 tok/s is worth chasing precisely because it is only 2.3x better than
something already working.

## Single-core says the loop shape is right

```
x86 container (no NEON) : 4.2 GiB/s
M2 Ultra, 1 thread      : 15.5 GiB/s
```

3.7x, which is what a vectorizing NEON backend should give over scalar-ish x86 codegen. The inner loop
is not the problem — **portable code that LLVM can vectorize was the correct call**, and hand-written
intrinsics would have bought a fraction of this at the cost of being untestable off-target.

`loop_shape` confirms the slicing fix travels, though it matters less on ARM: **+63%** there versus
**+200%** on x86. Bounds checks are cheaper on aarch64 — better branch prediction, cheaper compare — but
removing them still wins and costs nothing.

## The 2-thread anomaly

```
 1 thread : 15.5 GiB/s
 2 threads: 11.5 GiB/s   <- SLOWER than one
 4 threads: 61.0 GiB/s
24 threads: 113.2 GiB/s
```

Two threads being slower than one is not noise; it is a scheduling artefact. The M2 Ultra has
performance and efficiency cores, and a two-way split can land both halves on E-cores while the
single-threaded case runs on a P-core. `split_rows` divides evenly and has no idea which core it is
handing work to.

Worth fixing only if it matters in practice: real decode uses all cores, and the anomaly is confined to
a thread count nothing would choose. Recorded because it looks like a bug in the split and is not.

## The pool is vindicated, 18x

```
spawn, 8 threads : 141.3 us/call -> 47.5 ms per token
pool,  8 threads :   7.9 us/call ->  2.6 ms per token
```

This is the measurement the container could not take. In _317 the pool measured *worse* than spawning,
because a one-core box makes every thread contend and the spin loop burn cycles — so it was committed on
llama.cpp's evidence and the structural argument, with the doc saying "if the pool does not win there it
should be deleted".

It wins by **18x**. Keeping it was right, and the reasoning that kept it — trust a well-established
implementation's structure over a measurement the machine cannot take — is the part worth remembering.

## Q4_K is the remaining gap

```
Q8_0 (26.6 MiB): 0.23 ms  113.2 GiB/s
Q4_K (14.1 MiB): 0.80 ms   17.3 GiB/s
```

**6.5x slower per byte.** Q4_K does nibble extraction and carries a second accumulator for the min term,
so some gap is expected — but not this much. The likely cause is that the nibble unpack does not
vectorize: `if half == 0 { b & 0xF } else { b >> 4 }` is a branch inside the inner loop, and llama.cpp
handles the two halves as separate passes precisely to avoid it.

Since every model here ships Q4_K_M, this is now the highest-value CPU change available.

## Allocation, settled on both machines

```
x86 container: 0.01% of the call
M2 Ultra     : 0.01% of the call, 0.08 ms per token
```

No scratch buffer. Same conclusion from both, which is the strongest form this evidence takes.
