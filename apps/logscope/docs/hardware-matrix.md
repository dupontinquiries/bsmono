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
| M2 Ultra | Asahi | `Apple M2 Ultra (G14D B1) [Vulkan]` | 128 GB | 63 GiB (`--gate-vram`) | **31 GiB** (arena, measured) | the two disagree because they measure different resources |
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

## Note on the constant that is not a measurement

`~31.0 GiB (a CONSTANT for Asahi/Honeykrisp)` in the refusal path should be replaced by the arena's
measured clamp, which is computed on the same run a few lines earlier. They agree at 31 GiB today by
coincidence — the constant was chosen from the staging path and the measurement comes from slab
probing — and a machine where they differ will refuse models it could hold.
