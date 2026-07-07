# Bench results — chronological log

Single canonical reference for all hardware measurements we've taken
across the unendlich engine's development arcs. New measurements
appended at the bottom; do not edit older entries.

Each entry: date, version, hardware/profile, config, raw numbers,
interpretation, what it caused us to do next.

---

## Hardware

All measurements on a single test system unless noted otherwise:

- **Machine:** Apple M2 Max (G14C B1)
- **OS:** Asahi Linux (Fedora variant)
- **GPU driver:** Honeykrisp Mesa 25.3.6
- **Window manager:** KWin Wayland
- **Build:** `--release --features="render,gpu,hot-shaders"`

CPU power profile noted per-run when relevant.

---

## Chug arc measurements (v0.22 → v0.24.3, summarized)

The chug arc spanned increments 42-48 (`chug-debug-1` through
`shadow-stride`). Full per-increment details in `DESIGN_NOTES.md`;
this section captures the headline findings.

### Setup
- `chug_capture_rd8.unendtest` spec
- Render distance 8 (rd=8)
- Shadow map 2048×2048 by default

### Findings

**Shadow rendering is the dominant cost** at high render distances:
- Shadow GPU time exceeded 8.3ms budget on 96% of frames at rd=8
- Reducing shadow map area 4× yielded only 5% time reduction →
  NOT fragment-bound
- Reducing upload bandwidth didn't help → NOT upload-bound
- v0.24.3-cull-count measured shadow culling vs main: **shadow rendered
  16× more chunks than main** (16:1 median ratio; worst frames showed
  44:1)

**Shadow stride (v0.24.3-shadow-stride) was attempted** to amortize
the cost across frames. Result on AGX: dispatch floor visible at
all batch sizes. **TBDR + AGX firmware constraints make the
temporal-stride techniques common on immediate-mode GPUs unworkable.**

### What this caused

Architectural pivot from "fix the chug" to "GPU-resident world." See
`architecture-v1.md` and `architecture-v1-addendum.md`.

---

## v0.24.6 — first GPU sparse mesh bench

### Date
2026-05-15 early morning.

### Config
- `gpu-mesh-bench` binary, 36 chunks (3 × 3 × 4 grid)
- v1 path: one chunk per `mesh_chunk` call, one submit each
- CPU naive baseline: `mesh_chunk_naive` single-threaded sequential

### Raw numbers

```
Total: 36 chunks, 149494 records.
CPU total: 11752 us  (avg 326 us/chunk)
GPU total: 22917 us  (avg 636 us/chunk)
Overall speedup: 0.51×
Topology matches: 36/36 chunks
```

Notable per-chunk: `(1, 2, 1)` with 1 record took 549us GPU. The
empty chunks took ~26us each (early-return overhead only). This
**identified the dispatch floor at ~550us per submit on AGX.**

### Interpretation

- Per-chunk: GPU about 2× slower than CPU naive
- Dispatch floor dominates per-chunk cost (550us of the 636us avg)
- Variance high (600-1700us at similar record counts) — AGX
  firmware scheduling unpredictability

### What this caused

v0.24.7: batched dispatch to amortize the floor across many chunks
per submission.

---

## v0.24.7 — batched dispatch

### Date
2026-05-15 morning.

### Config
- `MAX_BATCH_CHUNKS = 16`
- Same 36 chunks
- 3 internal submissions (16/16/4)

### Raw numbers

```
v1 path: 24196 us total (672 us/chunk avg) — 0.45×
v2 path: 4619 us total (128 us/chunk avg)
v2 vs v1 speedup: 5.24×
v2 vs CPU speedup: 2.37×
Topology matches: 36/36 chunks
```

### Interpretation

- **Batching works.** 5.24× speedup vs v1 confirms AGX firmware
  pipelines the 16 dispatches within one cmd buffer (not serializes).
- Per-chunk dropped from 672us to 128us — most of the floor amortized.
- Per-chunk floor not fully eliminated; further batching might help.

### What this caused

v0.24.8: extend bench to measure against the real production
baselines (parallel CPU, MeshingPool) instead of just single-threaded.

---

## v0.24.8 — three runs comparing GPU v2 to parallel CPU

### Date
2026-05-15 mid-morning.

### Config
- Same 36 chunks
- POOL_WORKERS = 4 (matches production `MESH_WORKERS`)
- 3 separate bench runs, ONE per CPU power profile:
  - Run 1: **performance** profile
  - Run 2: **balanced** profile
  - Run 3: **powersave** profile

### Raw numbers

| Path | Run 1 (perf) | Run 2 (bal) | Run 3 (save) |
|---|---|---|---|
| CPU naive sequential | 10934 us | 10889 us | 10977 us |
| CPU naive parallel (4) | 4079 us | 3813 us | 5459 us |
| MeshingPool (4 workers, greedy+sky) | 22154 us | 16784 us | 17125 us |
| GPU sparse v1 (one per submit) | 28570 us | 29868 us | 30430 us |
| GPU sparse v2 (batched) | 5377 us | 5821 us | 5927 us |
| v2 vs CPU naive sequential | 2.03× | 1.87× | 1.85× |
| v2 vs CPU naive parallel | 0.76× | 0.66× | 0.92× |
| v2 vs MeshingPool | 4.12× | 2.88× | 2.89× |

### Interpretation

**Three surprises:**

1. **GPU v2 is stable across power profiles** (5377/5821/5927 us; ~10%
   spread). GPU has its own DVFS; CPU power profile doesn't affect it
   much.

2. **CPU naive parallel scales with profile** (4079/3813/5459).
   Performance and balanced are similar; powersave 40% slower.

3. **MeshingPool Run 1 (perf) is anomalously slow at 22154 us.**
   Runs 2/3 are 16784/17125 — much more representative. Cause
   unknown; possibly cold caches, pool worker thread spawn timing,
   or PMU profile transition overhead. **Treat the 17ms as
   representative, not 22ms.**

**Architectural read:**

- **GPU v2 LOSES to CPU naive parallel** by ~25% in performance/
  balanced modes, ties in powersave.
- **GPU v2 wins by ~3× over MeshingPool** — but only because the
  pool runs greedy+sky-lit (more work per chunk than v2's flat-color
  naive).
- The fair comparison "same algorithm, different substrate" is
  GPU v2 vs CPU naive parallel — and GPU loses.

### What this caused

Reorientation. Pure naive on GPU doesn't beat parallel CPU naive.
The path forward requires reducing per-chunk GPU work, which means
either:
- Greedy on GPU (parallel-hostile; complex)
- 1D row-merging via SurfaceRun (parallel-friendly; simpler)

v0.24.9 introduced SurfaceRun. v0.24.10 the GPU kernel for runs.
v0.24.11 larger batch size to keep the dispatch-floor pressure
low at higher chunk counts.

---

## v0.24.9 — SurfaceRun representation (CPU-only)

### Date
2026-05-15 late morning.

### Status

CPU-only ship. Adds `SurfaceRun` struct, `extract_surface_runs`
function, 8 tests. Bench reports records vs runs compression but no
GPU kernel for runs yet (that's v0.24.10).

### Numbers reported by bench

The compression analysis section. Hardware measurement pending.
Predicted: 3-5× compression for natural terrain.

---

## v0.24.10 — GPU runs kernel

### Date
2026-05-15 late morning (overnight session work).

### Config
- New compute shader `chunk_mesh_runs.comp` (one thread per run,
  one quad per run)
- `mesh_chunks_batch_runs` API
- Same buffer layout as v0.24.7 (sparse mesher's existing buffers)
- MAX_BATCH_CHUNKS still 16

### Hardware measurement

Not measured at v0.24.10 specifically. The runs kernel results
were captured in the v0.24.11 measurement run (which kept the
runs kernel unchanged and only widened MAX_BATCH_CHUNKS).

### Topology validation

Passed first try on hardware (v0.24.11 run). 36/36 chunks.

### What this caused

The runs kernel is the foundation of v3. v0.24.11's batch size
increase amplifies its win further.

---

## v0.24.11 — MAX_BATCH_CHUNKS raised to 64

### Date
2026-05-15 morning (overnight session work, tested mid-morning).

### Config
- `MAX_BATCH_CHUNKS = 64` (up from 16)
- Buffers: ~96 MB persistent HOST_VISIBLE
- New env var `UNENDLICH_GPU_MESH_BATCH` (1..=64) to cap actual
  chunks per submission at runtime
- All other config unchanged

### Hardware measurement

| Path | total_us | us/chunk |
|---|---|---|
| CPU naive sequential | 11,429 | 317 |
| CPU naive parallel (4) | 5,261 | 146 |
| MeshingPool (4 workers, greedy+sky) | 27,995 | 777 |
| GPU sparse v1 (one per submit) | 30,724 | 853 |
| GPU sparse v2 (batched) | **4,937** | 137 |
| GPU runs v3 (batched, 1D-merged) | **1,829** | 50 |

Speedup ratios:
- v3 vs v1: 16.80×
- v3 vs v2: 2.70×
- v3 vs CPU naive sequential: 6.25×
- v3 vs CPU naive parallel: **2.88×**
- v3 vs MeshingPool: 15.31×
- v2 vs CPU naive parallel: 1.07×

### Compression
- Records: 149,494 (one per emitting block-face)
- Runs: 18,274 (1D-merged along U axis)
- **Compression ratio: 8.18× (records / runs)**
- Average run length: 9.04 blocks

The compression is higher than predicted (3-5×). Natural terrain
has long horizontal runs (floors, walls, terraces) that merge
aggressively in 1D.

### Topology validation

- v1 (per-submit): 36/36 chunks match CPU naive quad counts
- v2 (batched records): 36/36 chunks match
- v3 (batched runs): 36/36 chunks pass BOTH checks
  (`gpu_quads == runs.len()` AND `sum(run.length) == cpu_naive_quad_count`)

The face-direction quad-corner math in `chunk_mesh_runs.comp` is
correct on first try. No winding bugs detected.

### Interpretation

**v3 wins cleanly against every CPU baseline including the production
MeshingPool by 15.31×.** This is the result that justifies Layer A.3
production wiring as a viable next step.

Caveats:
- v3 emits flat unlit colors; MeshingPool emits greedy + sky-lit.
  Not visually equivalent. Layer A.3 with v3 alone would regress
  visual quality.
- MeshingPool wall-clock was 27,995us this run — slower than the
  ~17ms seen in v0.24.8. Variance is real on the production path.
  Even at MeshingPool's best-case ~16ms, v3 wins by ~9×.

### Bench timing bug discovered

The "extract: X us total" lines reported ~115ms per chunk —
**that's the chunk generator running, not extraction**. Each
extractor loop regenerated chunks from scratch. The compression
ratio and run-length numbers are still correct (data, not timing),
but the extract-time numbers are meaningless until v0.24.12.

### What this caused

v0.24.12: fix the bench timing bug (pre-generate chunks once).

Layer A.3 prep is now justified by performance numbers. Still
gated on the visual-quality question: ship A.3 behind a feature
flag, or wait for GPU greedy+lighting to come first.

---


## How to read this doc

When new measurements come in, append a section with the same
shape (date, config, raw numbers, interpretation, what next).
Don't edit older entries — they're the historical record.

The "headline" claim at each version's date is: GPU v3 beats CPU
naive parallel (TBD), MeshingPool comparison is unfair because of
algorithmic difference (lighting), and the path to actual
production wiring (Layer A.3) requires GPU greedy+lighting parity
with the MeshingPool.

Architecture context: see `architecture-v1.md` for destination
shape; `architecture-v1-addendum.md` for items/scheduling/
determinism additions; `DESIGN_NOTES.md` increment 49+ for the
specific increments that produced each measurement.

## v0.24.12 — bench extract-timing fix

### Date
2026-05-15 late morning.

### Status
Surgical fix to the bench. Engine code unchanged. Pre-generates
chunks once at the start of the compression analysis section,
then times only the extraction (`extract_surface` and
`extract_surface_runs`) on already-generated chunks.

### Why this matters

v0.24.11's "extract: 4,138,458 us total" was actually measuring
36 × `gen.generate_chunk` calls inside the extract loop. The real
extraction is much faster than that. Until we measure it
correctly we can't decide whether extract overhead is a concern
for the GPU path's total cost.

### Hardware measurement (post-fix)

```
records:  149,494   (extract:  7,131 us total,  198 us/chunk avg)
runs:      18,274   (extract: 18,460 us total,  512 us/chunk avg)
compression ratio (records/runs): 8.18×
avg run length: 9.04 blocks
runs extract overhead vs records: 2.59×
```

### Interpretation: honest total cost of the GPU runs path

The "GPU dispatch only" numbers we've been celebrating
(v3 = 1,549 us) tell only part of the story. The GPU runs path's
real total cost is `extract + dispatch`:

| Component | Time |
|---|---|
| extract_surface_runs (CPU, sequential) | 18,460 us |
| mesh_chunks_batch_runs (GPU dispatch) |  1,549 us |
| **GPU runs path total** | **20,009 us** |

Apples-to-apples vs production MeshingPool (28,980 us): **1.45×
win.** Not 17×.

With 4-way parallel extraction (matching MeshingPool worker count):
- Extract: ~4,615 us (assuming linear scaling)
- Dispatch: 1,549 us
- Total: ~6,164 us
- vs MeshingPool: ~4.7× win

The 17× "headline" number compared GPU dispatch only against the
entire MeshingPool wall-clock. Real win is 1.5× sequential / 4.7×
parallel. Still a win, just much smaller than the headline.

Also notable: **v2 (records path) is actually faster than v3 in
total cost.** v2 dispatch is 4,833 us but its extract is only
7,131 us → total 11,964 us. v3's higher compression doesn't make
up for its 2.6× more expensive extraction. v2 vs v3 total: v2
wins by 1.67×.

This flips the v2-vs-v3 decision for production wiring. v3 is
better for downstream rendering (9× fewer quads to draw) but v2 is
faster to produce.

### What this caused

v0.24.13: optimize the run extractor. The 6-pass design is wasteful;
single-pass should reduce extraction cost ~3×, putting v3 back in
the lead.

---



## v0.24.13 — single-pass run extractor

### Date
2026-05-15 midday.

### Config
- `extract_surface_runs` rewritten as single-pass `(y, z, x)` walk
- Old 6-pass version retained as `extract_surface_runs_six_pass`,
  used as test oracle and side-by-side bench reference
- Engine + bench source changes; kernel unchanged from v0.24.12

### Algorithmic change

v0.24.12 walked the chunk 6 times (once per face direction). Each
cell was read 6 times, neighbor checks done 6 times. v0.24.13 walks
once. Per cell:
- 1 chunk.get for `here`
- 6 get_local for neighbors (one per face direction)
- 6 face-emission checks
- Update 6 open-run states (4 for U=X faces, 2 for U=Y faces)

U=X face open-runs are scalars that reset at start of each (y, z)
row. U=Y face open-runs need a 32×32 state matrix per face because
they span y-iterations.

Total cell reads: 32³ × 7 = ~230K vs old 32³ × 6 × 2 = ~400K.
Net: ~1.7× fewer reads. The bigger win comes from removing the
generic axis-indexing arithmetic (`coord[slice_ax] = ...` was
indexed array writes that the compiler couldn't optimize).

### Equivalence verification

`runs_v2_matches_six_pass` test compares output of single-pass and
six-pass extractors on a messy chunk (mixed types, edges, corners,
boundary blocks). Plus dedicated tests for empty, fully solid, and
single-block-at-corner chunks.

Both extractors produce IDENTICAL output (sorted) for all test cases.

### Hardware measurement

```
records:          149,494  (extract:  7,157 us, 198 us/chunk)
runs (1-pass):     18,274  (extract: 10,017 us, 278 us/chunk)  [production]
runs (6-pass):     18,274  (extract: 18,764 us, 521 us/chunk)  [reference]
1-pass speedup over 6-pass: 1.87×
runs (1-pass) extract overhead vs records: 1.40×

v3 dispatch:        1,961 us
v3 honest total:   11,978 us (extract + dispatch)
```

### Interpretation

**Single-pass extractor is 1.87× faster than 6-pass.** Less than my
3× prediction. The compiler didn't optimize away as much per-cell
work as I'd hoped — probably the 32×32 state-matrix indexing for
U=Y faces still has memory-bound elements that don't collapse to
registers.

But the runs-extractor overhead vs records-extractor dropped from
2.59× to 1.40×. That's the meaningful number: runs extraction is
now only 40% more expensive than records extraction.

### v2 vs v3 honest comparison (post-optimization)

| Path | Extract | Dispatch | Total |
|---|---|---|---|
| GPU v2 (records) |  7,157 | 4,868 | 12,025 us |
| GPU v3 (runs)    | 10,017 | 1,961 | 11,978 us |

**Effectively tied.** v3's higher compression now exactly offsets
its more expensive extraction. Pick either on perf grounds; pick v3
on downstream rendering grounds (9× fewer quads to draw later).

### vs CPU baselines (honest totals)

- v3 honest total: 11,978 us
- CPU naive parallel: 5,461 us → **CPU still wins by 2.19×**
- MeshingPool: 27,560 us → v3 wins by 2.30×

The CPU naive parallel mesher inlines the surface check into its
own loop. It has no separate "extract" step. The GPU path's
CPU-side framing cost (~10ms extract, ~2ms dispatch) is more than
the entire CPU mesher does.

### Architectural read

Pure throughput argument for GPU mesher is weak. CPU naive parallel
is fastest on this workload by 2.2×. The GPU path's structural
overhead (extract → upload → compute → readback) doesn't disappear
with kernel optimization alone.

The unmeasured next moves that could flip this:
- **Parallel extract**: 4-worker extract drops to ~2.5ms; v3 total
  ~4.5ms. Beats CPU naive parallel by ~1.2×.
- **Zero-copy output**: eliminate readback by binding GPU output
  buffers directly to renderer vertex source. Saves whatever the
  current readback costs (not measured separately).
- **GPU-side extraction**: kernel that walks the chunk and emits
  runs directly. Eliminates the CPU-side extract entirely. Real
  win but requires chunk to be in GPU memory (Layer C territory).

### Test counts

- 600 engine + 70 shared + 28 launcher = **698 fast tests pass**.
- Added 4 new tests (equivalence + edge cases).

### Arc summary

This is the natural pause point for the GPU sparse mesher arc.
v0.24.6 → v0.24.13 took the mesher from "broken (0.5× CPU)" to
"comparable to MeshingPool, 2.2× slower than fastest CPU baseline,
topology validated, single-pass extract optimized."

Layer A.3 production wiring deferred — needs parallel extract or
zero-copy first to be a clear performance win on top of the
strategic case.


## v0.24.15 — parallel extract experiment

### Date
2026-05-15 afternoon.

### Config
- `extract_surface` and `extract_surface_runs` (single-pass) both
  parallelized in the bench using POOL_WORKERS (4) threads with
  atomic-counter work distribution
- Engine code unchanged; bench-only ship
- Asserts at runtime that parallel and sequential extractors produce
  identical run/record counts (canary if a future optimization breaks
  thread-safety)

### Why

v0.24.13's hardware result showed v3 honest total (12 ms) loses to
CPU naive parallel (5.5 ms) by 2.2×. The hypothesis: extract scales
linearly with worker count. If it does, v3 honest total drops to
~4-5 ms, putting it ahead of CPU naive parallel and ~6× ahead of
MeshingPool.

### Hardware measurement

```
records:                149,494  (extract:  7,117 us, 197 us/chunk)
records (4-worker par):           (extract:  3,496 us)
runs (1-pass):           18,274  (extract:  9,791 us, 271 us/chunk)
runs (1-pass, 4-worker par):      (extract:  4,514 us)

records parallel speedup (4 workers): 2.04×
runs parallel speedup (4 workers):    2.17×
```

### Honest totals

```
GPU v2 honest (seq extract):  11,999 us =  7,117 +  4,882
GPU v2 honest (par extract):   8,378 us =  3,496 +  4,882
GPU v3 honest (seq extract):  11,820 us =  9,791 +  2,029
GPU v3 honest (par extract):   6,543 us =  4,514 +  2,029
```

| Path | vs CPU naive par (5,356) | vs MeshingPool (29,694) |
|---|---|---|
| v2 honest (seq) | 0.45× | 2.47× |
| v2 honest (par) | 0.64× | 3.54× |
| v3 honest (seq) | 0.45× | 2.51× |
| **v3 honest (par)** | **0.82×** | **4.54×** |

### Interpretation

**Parallel extract scales ~2×, not 4× as I'd hypothesized.** Both
extractors hit the same ceiling: 4 workers → ~2.1× speedup. For
comparison, the CPU naive parallel mesher also scales ~2.15× from
sequential (11,538 → 5,356), so this is a general "short workload,
4-worker scaling penalty" effect — likely cache + thread-launch
overhead — not a GPU-path-specific issue.

**v3 honest (par extract) vs CPU naive parallel: 0.82×.** GPU
still loses to the best CPU baseline by 1.22×. The structural
overhead of extract → upload → dispatch → readback isn't
eliminable through parallelism alone.

**v3 honest (par extract) vs MeshingPool: 4.54×.** This IS the
"would Layer A.3 wiring beat what's there today" comparison. GPU
wins clearly. But MeshingPool produces greedy + sky-lit output;
GPU v3 produces flat-color naive. Visual quality regresses if A.3
ships without kernel improvements.

### What this caused

End of the parallel-extract autopilot run. The remaining items on
the "concrete unblocked work" list (zero-copy output, GPU-side
extraction, greedy + lighting kernel, Layer A.3 production wiring)
either require design decisions or have hard prerequisites and
aren't autopilot-able.

The GPU sparse mesher arc reaches its natural pause. Open decisions
for next session:
- Ship A.3 strategically (behind a flag), accepting visual
  regression in exchange for architecture progress
- Wait for greedy + lighting kernel work before A.3
- Pivot to architecture-v2 / different direction

