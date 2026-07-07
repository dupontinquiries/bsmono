# Fastmath helper libraries — device-parity & performance analysis (r196)

## The question
Would a fastmath-style helper set (approximate transcendentals, fast
inverse-sqrt, polynomial sin/cos/exp) help performance without breaking
the CPU↔GPU determinism this engine depends on?

## Measured surface (grep census, this tree)
Transcendental call sites:
  sqrt 146 · sin 76 · cos 46 · exp 37 · powf 5 · powi 20 · abs 729
— overwhelmingly in CPU `sim/` and `world/` modules.

The GPU compute shader (`chunk_generation.comp`) uses only:
  sqrt 1 · sin 2 · cos 2 · exp 2
and the per-voxel worldgen noise (`noise.rs`, `batch_density_noise.rs`)
uses **NO transcendentals** — only multiply / add / floor / smoothstep
(a polynomial) / lerp.

## The parity conclusion — fastmath is SAFE here, with one rule
Parity only matters where CPU and GPU compute the SAME quantity. That
surface is: the worldgen density/biome/sea-level math. And that math has
**no transcendentals to approximate** — it is already mul/add/floor,
which r195 made bit-identical (f32-first cast + biased floor).

Therefore:
- **CPU-only transcendentals (the 300+ sin/cos/exp/sqrt in sim/) have no
  GPU twin.** Approximating them cannot break device parity — there is
  nothing on the GPU to disagree with. These are free to speed up.
- **The ~7 shader transcendentals are the ONLY parity-critical ones.**
  RULE: fastmath must never be applied to a quantity that also flows
  through the shader unless the shader gets the identical approximation.
  The fastmath module is therefore tagged for *sim/visual* use and is
  explicitly forbidden in worldgen density/biome/sea-level paths (a
  one-line doc contract + the functions live in `util::fastmath`, not in
  `world_model`).

## Where the performance actually is
fastmath does NOT help the worldgen hot path (no transcendentals there).
It helps the **per-frame CPU simulation**, which runs every tick:
  - light_physics, water_waves, swim, camera_tremble, viewmodel,
    cognition/wander, mob_spawn_director, dragon_forge, seed_dispersal.
Dominant pattern: vector-length normalization
  `(x*x+y*y+z*z).sqrt()` then divide — i.e. `norm3`, already centralized
in `util::vecmath` (r193). Replacing its one `sqrt` with a fast inverse
sqrt (and skipping the divide) is the single highest-yield change:
  - removes 1 sqrt + 3 divides per normalize, replaced by 1 rsqrt approx
    (Quake-style + 1 Newton step) + 3 muls.
  - every call site that already routes through vecmath::norm3 benefits
    for free (the r193 reuse ratio pays a second dividend).

## Expected impact (honest bounds)
- Worldgen / chunk gen: **0%** (no transcendentals; don't touch it).
- Per-frame sim normalization & wave/light math: a fast rsqrt is
  ~3–5× cheaper than `sqrt`+divide on scalar f32; sin/cos polynomial
  ~2–4× vs std. But these are not the frame bottleneck (the mesher
  submit is, per the perf advisor). Realistic whole-frame win: small,
  low single-digit %, concentrated in heavy-creature / heavy-water
  scenes. The bigger, measurable win is keeping the SoA noise path
  autovectorizing (already done) and the r191 threading.
- Determinism: **unchanged** by construction (sim approximations are
  deterministic functions of their inputs; parity surface untouched).

## What to ship (this session)
1. `util::fastmath` — std-only, deterministic, documented as SIM/VISUAL
   ONLY (never worldgen-parity): `rsqrt(f32)`, `norm3_fast`,
   `sin_fast`/`cos_fast` (polynomial), `exp_fast`. Each unit-tested for
   max abs error vs std so the approximation budget is explicit.
2. `vecmath::norm3` gains a `norm3_fast` sibling; sim callers opt in.
   `norm3` (exact) stays for anything that could touch parity.
3. A bench example (`fastmath_bench`) timing exact vs fast over N iters
   so the perf claim is measured here, not assumed.

## Non-goals (guide §2.1 discipline)
- No `wide`/`libm`/`glam` dependency (std-only).
- Do not approximate worldgen noise (it has no transcendentals and is
  parity-critical).
- Do not auto-replace exact calls; opt-in per site, sim-only first.

## MEASURED VERDICT (r196 bench, opt-level=3 + thin LTO + cu=1)
examples/fastmath_bench.rs, 20M iters, inputs black-boxed:
  rsqrt:  std 126ms  fast 172ms  -> 0.73x  (SLOWER)
  sin:    std  13ms  fast  13ms  -> 0.97x  (wash)
  exp:    std 112ms  fast 241ms  -> 0.47x  (HALF speed)
examples/noise_bench.rs (worldgen hot path), baseline vs
  RUSTFLAGS=-C target-cpu=native: 172 vs 174 ms/chunk — NO difference.

CONCLUSION: scalar fastmath is a NET LOSS on this hardware. Modern x86-64
(and aarch64 — both have hardware FSQRT/FRSQRTE and vendor libm) plus a
release LLVM beat hand-rolled polynomial approximations. The Quake-era
bit-hack assumptions do not hold on 2020s CPUs. target-cpu=native does
not help the worldgen path either — its cost is the cave SDF + per-voxel
branching + f64, not transcendental throughput or SIMD width.

DECISION: do NOT adopt fastmath. The module ships CORRECT + TESTED (error
budgets verified) but UNUSED — kept only as a ready tool IF a future
profile identifies a genuinely transcendental-bound loop on the M2 target
where the std call is the proven bottleneck. No call sites converted. The
honest engineering answer to "should we do it" is: measured, and no.

DEVICE PARITY: unaffected either way — the parity surface (worldgen
density/biome/sea-level) has no transcendentals and was never a fastmath
candidate. The r195 f32-first determinism stands untouched.

WHERE THE REAL WINS ARE (re-confirmed): the r191 threading plan
(2x2x2 region tick sharding, mesher/apply overlap) and the mesher-submit
batching the perf advisor already names — NOT micro-optimized math.
