# Validated hardware, and which warnings are measurements

Every budget message this engine prints is either a MEASUREMENT on the machine in front of it or a
COMPILED-IN CONSTANT. They read identically and mean opposite things, and one of them has already
cost a wrong refusal.

## The two that disagree on the same device

```
loader: GPU budget 48.0 GiB clamped to 31.0 GiB — measured at 1 and 2 GiB slabs, mapped and unmapped
worker: load failed: won't fit resident in ~31.0 GiB (a CONSTANT for Asahi/Honeykrisp, not a
        measurement) — `--gate-vram` measured this same device honouring 63 GiB
```

**Same number, different provenance.** The clamp is real: the arena measured it by allocating.
The refusal's 31 GiB is a constant inherited from the staging-upload path, and the engine says so in
its own message — which is the right behaviour and also means a 40 GiB model may have been refused
for no reason.

## The matrix

| field | how to get it | why it matters |
|---|---|---|
| `uname -a` | as printed | Asahi kernel version gates Landlock, and driver behaviour tracks mesa version |
| adapter string | engine preflight line | `Apple M2 Ultra (G14D B1) [Vulkan]` selects the Honeykrisp path |
| OS | Asahi / macOS | macOS uses Metal directly; the fast-math and subgroup findings below are Honeykrisp-specific |
| physical RAM | `free -g` / `sysctl hw.memsize` | unified memory, so this is also the VRAM ceiling's upper bound |
| `--gate-vram` result | run it | what a **single allocation** can be |
| arena clamp | load line | what **concurrent buffers** can hold — the smaller and the real limit |
| `--gate-bytes` | run it | lane-varying shift correctness |
| `--gate-q4k` | run it | fast-math division and rounding |

### Recorded so far

| machine | OS | adapter | RAM | single alloc | concurrent | notes |
|---|---|---|---|---|---|---|
| `ultra-fed` | Fedora 43 Asahi | `7.1.6-400.asahi.fc43.aarch64+16k` | `Apple M2 Ultra (G14D B1) [Vulkan]` | 128 GB | 63 GiB | **31 GiB** (arena) |

### The `+16k` matters more than it looks

That kernel runs **16 KiB pages**, not 4 KiB, and none of the consequences are cosmetic:

* **mmap granularity is 16 KiB.** A GGUF tensor offset that is 4K-aligned is not page-aligned here,
  so a zero-copy path written assuming 4K either falls back to a copy silently or is wrong.
* **Allocations round up to 16 KiB**, so thousands of small tensors waste four times the slack —
  an argument for the arena independent of the BO-VMA one.
* **The concurrent-buffer ceiling may be page-COUNT bound rather than byte-bound.** If so, 31 GiB
  is specific to this page size and would be wrong on a 4K Asahi kernel.
* **A cached `--gate-residency` result must record it**, or two machines sharing an adapter string
  and differing in page size get each other's answer.

`getconf PAGESIZE` is the check and belongs beside `uname -a` in the required facts.

### Confirmed on hardware, 2026-08

`--gate-quantref` passes on `ultra-fed` — the first time the driver profile has been VERIFIED rather
than inferred. Ties away from zero and reciprocal-product scaling both hold for q2, q3, q4 and q8.

The tie densities it reports (0.040 / 0.124 / 0.270 / 4.976 per unit range) are why a gate must span
a wide enough range to contain one: **a narrow gate passes by never generating the case it exists to
test.**
| M2 Max | Asahi | — | 64 GB | not measured | not measured | laptop; used for the Delegate role |

**The gap between 63 and 31 is not an error.** `--gate-vram` allocates and releases; the arena must
HOLD. A probe that measures the first and a planner that needs the second will disagree forever, and
both numbers are correct about their own question.

## Honeykrisp-specific findings, all measured on the M2 Ultra

| behaviour | spec says | this driver does | cost when assumed |
|---|---|---|---|
| shift by a lane-varying amount | works | **returns zero** for lanes whose shift is non-zero | every Q4_K load produced wrong weights |
| float division | correctly rounded | **multiply by an f32 reciprocal** | 297430 wrong words at grid scale |
| `round()` ties | to even (WGSL spec) | **away from zero** (Metal) | a "fix" made a gate 8.8x worse |
| multiply-add | not contracted unless asked | **contracts, per compilation unit** | identical source rounded differently in two shaders |
| subgroup matvec | supported | **can hang large matmuls** | disabled by default; `LOGSCOPE_MATVEC_SG=1` re-enables |

**None of these appears on lavapipe**, which is why every one of them cost a round trip to find. A
CI that runs only on lavapipe validates nothing about this class.

## What a new machine should report

Run these and paste the output; between them they populate every column above:

```
uname -a
kernel-validate --gate-vram        # single allocation ceiling
kernel-validate --gate-bytes       # lane-varying shifts
kernel-validate --gate-q4k         # division and rounding
kernel-validate --frame-diag       # submit structure
```

Then load any model and record the `arena ON` and `GPU budget` lines, which are the concurrent
figure.

## RESOLVED: the constant was a measurement after all

`--gate-residency` on the M2 Ultra held **31 GiB in 124 one-GiB slabs**, and the 125th failed with
`Failed to allocate BO VMA`. The machine has 177 GiB and the probe was allowed to reach 88.

**So the refusal's 31 GiB was correct**, and the 70B Q4_K_M refused at that ceiling was refused
rightly. I wrote the probe expecting to find a stale constant costing you models, and found the
opposite — which is the more useful outcome, because it means the planner can be trusted rather than
worked around.

What remains true: it is ONE machine's figure. An M2 Max with 64 GB has not been measured and its
ceiling is certainly lower.

## Note on the constant that is not a measurement

`~31.0 GiB (a CONSTANT for Asahi/Honeykrisp)` in the refusal path should be replaced by the arena's
measured clamp, which is computed on the same run a few lines earlier. They agree at 31 GiB today by
coincidence — the constant was chosen from the staging path and the measurement comes from slab
probing — and a machine where they differ will refuse models it could hold.
