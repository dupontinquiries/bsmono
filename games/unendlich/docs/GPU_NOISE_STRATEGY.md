# GPU Noise Strategy

This document captures the noise-function strategy decisions made during
v0.18.1 for future reference. v0.18.1 took the smallest path (direct
FP32 port) to unblock the immediate Honeykrisp problem. The strategies
below are filed for when GPU worldgen comes back into focus and the
direct port's coordinate-precision ceiling becomes a problem.

## The setup

GPU worldgen (`chunk_generation.comp`) needs to produce deterministic
noise from world coordinates. The constraint:

- No FP64 (Apple GPUs, mobile, RISC-V dev boards, most Intel SoCs all
  lack `shaderFloat64`).
- Determinism: `noise(p)` must produce the same value regardless of
  player history, chunk eviction order, or render distance.
- Continuity: terrain shouldn't have visible seams.
- Range: at least ±10⁶ blocks per axis (Minecraft-scale), preferably
  unbounded.

FP32 has ~24 bits of mantissa, so it represents integers exactly to
2²⁴ = 16,777,216. Direct FP32 noise works fine inside this range. Above
it, `floor(p)` rounds to even integers and lattice cells get skipped.

## v0.18.1 decision: direct FP32 port

Just `double` → `float` in the existing shader. Same algorithm, same
hash, same lattice. Sufficient for any coordinate inside the playtest
range.

**Pros**: minimal diff, ships in the same increment as the unblock.
Risk is bounded — at worst the world looks subtly different, the user
already accepted that.

**Cons**: doesn't scale beyond 16M blocks. World identity changes
(seed produces different terrain than v0.17.x).

This is good enough for now. It's not the long-term answer.

## Strategy A: anchor-blend FP32 noise (user's proposal)

Two-level coordinate system for noise sampling.

### How it works

Define **anchors** at a coarse grid — every 32 chunks (1024 blocks)
in 3D, so anchor positions are at all `(x*1024, y*1024, z*1024)`.
The anchor a world point belongs to is determined by integer math:
`anchor = (world_coord >> 10) << 10`.

Within a 1024×1024×1024 region, noise samples are taken **relative
to the corner anchors**, never in absolute world coords. The relative
coord stays in `[0, 1024)` per axis — well inside FP32 precision.

### The discontinuity problem and the fix

A naive implementation would have hard seams every 1024 blocks (the
noise function snaps from `noise(1023)` to `noise(0)` at boundaries).
The fix: each world point evaluates noise at **multiple nearby
anchors** and blends between them by relative distance.

- **Heightmap noise (XZ-only)**: 4-anchor blend at the corners of
  the surrounding 1024×1024 cell. Trilinear becomes bilinear.
- **3D noise (caves, density fields)**: 8-anchor blend at the corners
  of the surrounding 1024×1024×1024 cube. Standard trilinear.

The user's specific note from the design conversation:
> we have to smooth the world with a smoothing pass for other more
> advanced terrain in the future most likely so we could do this and
> break the continuity plus every 32 chunks of 32 blocks is a long
> time so not too much smoothing work
>
> you could also average the quad of nearby anchors too as a cheap
> pass

The "smoothing pass aligns with future direction" point matters: the
engine is going to need post-gen smoothing for plant deformation,
fluids, weathering, erosion. The anchor-blend's micro-discontinuities
get absorbed into the same pass.

### Cost

8× simplex evaluations per 3D noise sample (4× for heightmap). Each
evaluation is FP32 instead of FP64, so on hardware that was emulating
FP64 in software (everything except discrete workstation GPUs), this
might actually be a net **speedup**.

### What it doesn't preserve

The world will look different than the same-seed CPU FP64 noise. The
character of the noise field changes — the algorithm is fundamentally
two-level. Saved chunks from a non-anchor world won't align with
regen at the edges if the noise function gets swapped under them.
File this as: **anchor-blend is a clean break, not an upgrade path**
from the v0.18.1 FP32 port. Pick a transition session.

## Strategy B: split-coord noise

Pass coordinates as `(ivec3 chunk, vec3 local)` separately. Inside
the noise function:

```
int xi = chunk.x * 32 + int(floor(local.x));   // exact integer
float xf = fract(local.x);                     // [0, 1)
```

The integer lattice corner is computed in integer space (exact up
to ~67M chunks = ~2.1B blocks). The within-cell fractional position
stays in `[0, 1)` where FP32 has 22+ bits of precision. The hash
function takes integers directly, so noise output is byte-identical
to FP64 simplex inside FP32-representable ranges, and graceful
beyond.

### Cost

One noise evaluation per sample (same as v0.18.1 direct port). The
per-call signature changes — every call site has to pass the split.
Touches the worldgen call graph everywhere noise is used.

### What it preserves

Byte-equality with the current CPU noise output. Same world, same
visual identity. This is an in-place upgrade, not a clean break.

## Strategy C: integer-native value noise

Build value noise on `hash(ivec3 corner) → float in [0,1]`, then
trilinear interpolate using the local fractional position. No FP64
anywhere, unlimited range, no anchor-blend complexity.

This is the most modern GPU-noise approach (shadertoy-style). Output
is fundamentally different from simplex noise — different visual
character — but mathematically clean.

### Cost

Replace the noise function entirely. Different output shape
(value noise has more visible lattice artifacts than simplex unless
you blend octaves carefully).

## Decision matrix

| | A (anchor-blend) | B (split-coord) | C (integer value noise) |
|---|---|---|---|
| Range | unbounded | ±2B blocks | unbounded |
| Visual continuity at scale | needs blend | byte-equal to FP64 | algorithm-dependent |
| World identity preserved | NO | YES | NO |
| Engineering scope | medium-large | small (sig change everywhere) | medium |
| Aligns with future smoothing pass | YES (per user) | neutral | neutral |
| Cost on FP64-poor hardware | ~8× FP32 evals | ~1× FP32 eval | ~1× FP32 eval |

## Recommendation for when this comes back

Pick based on whether you've grown attached to the current world:

- **You're OK with another world identity change**: Strategy A.
  Aligns with smoothing pass, simplest mental model long-term, scales
  to RISC-V / mobile naturally.
- **The world has settled and you want it preserved**: Strategy B.
  Surgical change, preserves everything, just needs careful
  refactoring of every noise call site to take split coords.
- **You want to redesign worldgen anyway** (e.g., AI-driven
  generation per the long-term wishlist): Strategy C, since you'd
  be replacing the noise function regardless.

## Not recommended

- **"Just use FP32 directly forever"** (current v0.18.1 state).
  Hits the 16M-block ceiling. Fine until you have players who walk
  past it, which probably won't happen for a while but is a known
  cliff.
- **Per-player rebasing of noise origin**. Breaks determinism;
  chunks change content based on player walking history. The user
  asked about this in the v0.18.1 design conversation; this section
  is here so the dead end is documented and not re-derived.
