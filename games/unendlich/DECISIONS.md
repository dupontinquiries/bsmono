# DECISIONS.md

Non-obvious design choices in this repo. Each entry carries an
epistemic-status tag: **anchored** (tied to an external fact),
**considered** (alternatives evaluated), or **default** (first
thing tried, works, nobody investigated alternatives). `default`
entries invite revisiting; `considered` entries discourage casual
change.

This file complements but does not replace README.md or other
docs. Where a topic is explained more fully elsewhere, an entry
here is a short pointer.

> Coordinator note: these entries cover decisions made during the multi-thread
> reconcile + the parallel-worker workflow. Per-module decisions for the worker
> cores (round 4+) arrive as `DECISIONS-T0N.md` fragments and get merged in here.
> Many deep engine constants below are the original engine's; where I couldn't
> confirm the rationale I marked `default` rather than invent one.

## Performance Constants

## Octree GPU pool fixed at 256 MB
**Status**: default
**Location**: flag `UNENDLICH_OCTREE_POOL_MB` (world_renderer pool setup)
The octree residency pool defaults to 256 MB. Run logs show it full (253–254/256)
and fragmented (largest free span ~33–72 KB) at render distance ~10, with span-allocator
exhaustion / OOM fallbacks driving the frame spikes. No anchor ties it to a specific VRAM
budget. Revisit via `world/device_tier` (set it per machine) and the SurfaceRun routing —
raising it is the immediate stopgap; the structural fix is not keeping far chunks in octree.

## Frame target 16.67 ms; tail metric at 8.3 ms
**Status**: anchored
**Location**: flags `UNENDLICH_FRAME_TARGET_MS` (16.67), the perf timer `over_8.3ms`
16.67 ms = 60 Hz (the frame target). The `over_8.3ms` tail counter is a stricter 120 Hz
(8.3 ms) budget used to surface stutter, not the shipping target. (Author has expressed a
longer-term 500 Hz aspiration; that is not what these numbers encode today.)

## LOD bands at 12 / 24 / 48
**Status**: default
**Location**: flag `UNENDLICH_LOD_BANDS=12,24,48`
Near ≤12, Mid 12–24, Far 24–48, VFar >48 (chunk radii). No documented anchor. These
boundaries directly gate `upload_repr::classify` (Mid→runs, Far→procedural), so the
SurfaceRun memory win only appears past the Near band — see `handoff/SURFACERUN_KIT.md`.
Revisit when scaling render distance or tuning `device_tier`.

## Octree per-chunk cost held flat (~164–176 KB) in the residency projection
**Status**: default
**Location**: handoff/surfacerun/surfacerun_telemetry.rs
The projection multiplies octree chunks by a flat measured per-chunk size. Real octree
cost varies (uniform chunks are far cheaper), so the projected octree pool is an upper
bound. Revisit by running `upload_repr::project_residency` on the live loaded set
(SURFACERUN_KIT Step 1) for ground-truth numbers.

## Rendering / GPU contracts

## Fog approach selector packed into CameraUbo.sky_color.a
**Status**: considered
**Location**: harness/RULESET.md (R3/R9/R16 + slot registry), world_renderer fog path
A free, documented-unused UBO component carries the 0..N fog-approach selector instead of
adding a new UBO field or descriptor binding. A new field/binding was rejected because a
Rust↔GLSL layout mismatch is silent UB / a validation crash, not a compile error. See the
RULESET slot registry; the SurfaceRun/fog wiring is staged in `handoff/`.

## debug_settings decoupled from the renderer via LookParams
**Status**: considered
**Location**: crates/unendlich-engine/src/debug_settings.rs
`apply_look` operates on an owned `LookParams` (5 atmosphere fields) the caller copies
in/out, NOT on the renderer's `AtmosphereParams`. The coupled form was tried first and
broke when the render tree's layout moved, and it fails the R1 purity check (couldn't be
unit-tested standalone). Decoupling keeps the module pure + layout-independent. See
harness/RULESET.md R1.

## `lerp3` helper gated `#[cfg(any(feature = "render", test))]`
**Status**: anchored
**Location**: crates/unendlich-engine/src/debug_settings.rs
`apply_look_inner` calls `lerp3` in render builds and in tests, so the helper must be
compiled in both configs. A `#[cfg(test)]`-only gate (the bug we hit) compiles under
`cargo test` but vanishes under `--features render`, breaking the render build while the
adjacent `mul3` survives. run.sh now typechecks the pure set under `--features render` to
catch this class.

## Module architecture / purity

## "Pure module" discipline: logic/sim modules import no render/Vulkan types
**Status**: considered
**Location**: harness/RULESET.md (R1/R6/R7), harness/coupling_check.py
Sim/logic modules must not import `crate::render`/`ash`/`vk`. This keeps them unit-testable
standalone under `rustc --test` (the sandbox tier) and layout-independent, which is what
makes parallel worker contributions merge additively. The alternative (let modules reach
into render for convenience) was rejected after it caused the debug_settings break. See
RULESET R7 for the layout-independent vs hand-merge split.

## Reconcile is additive 3-way; layout-specific files hand-merged, never overwritten
**Status**: considered
**Location**: RECONCILE_REPORT.md, harness/RULESET.md R7
Three divergent worktrees were merged by taking layout-independent files (pure modules,
harness, handoff) wholesale and hand-merging layout-specific ones (the bin, world_renderer,
lib.rs, shaders). Blind full-zip overwrite was rejected because it had previously clobbered
ahead work and reverted the lead tree. 649/667 files were identical; only 18 differed.

## Dev workflow & harness

## Two-tier harness: sandbox (no GPU) vs integration (your machine)
**Status**: anchored
**Location**: harness/RULESET.md, harness/run.sh
The sandbox tier (delimiter/purity/hygiene/unit+property tests/winding/UBO-parity/render-cfg
typecheck) runs with no Vulkan and no full cargo build, because the dev/CI sandbox has no
GPU. Everything needing Vulkan or the full crate graph (cargo build/test, in-game A/B, perf)
is the integration tier, run by the human. `run.sh` must print HARNESS GREEN before ship.

## Worker contributions = additive pure cores, disjoint dirs, one mod file each
**Status**: considered
**Location**: THREADED_CHAT_SETUP.md, OWNERSHIP.md
Each parallel worker adds files only under its own area and appends declarations to a single
area-unique module file (sim/mod.rs vs world/mod.rs vs lib.rs), so patches never touch the
same file and merge without conflict. This was chosen after manual cross-thread coordination
got error-prone; the alternative (workers editing shared files) reintroduces merge conflicts.

## Worker patch names stamped from WORKER_ID + ROUND files
**Status**: considered
**Location**: RETURN_KIT.sh (reads WORKER_ID, ROUND → CHANGES-T0N-RM.patch)
The return helper derives the patch filename from files the coordinator bakes into each zip,
rather than the worker/operator naming it by hand. Hand-naming was tried and produced a
mislabeled round-1-as-round-2 upload; stamping at the source makes the round self-evident
and the mismatch visible.

## SurfaceRun rollout sequenced: pool bump → telemetry → SurfaceRun → Procedural
**Status**: considered
**Location**: handoff/SURFACERUN_KIT.md
The far-render memory fix is staged so each step is independently revertible and the
projection de-risks it before any GPU change. Wiring all three paths at once was rejected as
unverifiable in the sandbox and risky on a hot path. The classify thresholds themselves
(`surface_run_from=Mid`, `procedural_from=Far` held one band out, `max_worst_col_runs=24`)
are explained in `crates/unendlich-engine/src/world/upload_repr.rs` rustdoc.

---

## ── Round-4 module decisions (merged from thread fragments) ──

### Thread 01 · SIM

## Rest the body on the highest footprint contact, not the average
**Status**: considered
**Location**: crates/unendlich-engine/src/sim/large_body_grounding.rs
The brief's bug is big mobs "coming off the world." I weighed mean height (smooth
but lets a foot clip underground on a lip) vs. min (sinks the body) vs. max. Max
is the only one of the three that guarantees no sample ends up below the rest
height, so a foot never clips through terrain — the body sits on its highest
contact and the plane fit handles the tilt. Hence `height = max(samples)`.
---

## Use a least-squares plane fit for the lean, degenerate ⇒ flat
**Status**: default
**Location**: crates/unendlich-engine/src/sim/large_body_grounding.rs
The tilt comes from fitting `ground ≈ A·x + B·z + C` over the footprint and taking
`atan` of the slopes. With <3 samples or collinear points the 2×2 covariance is
singular; rather than risk a NaN I return zero slope (level). Could have used a
3-point cross-product normal, but least-squares degrades more gracefully with many
noisy footprint taps.
---

## 16 heading bins for the pathing compass
**Status**: default
**Location**: crates/unendlich-engine/src/sim/path_memory.rs
22.5° per bin. Fine enough to route around an obstacle, coarse enough that scores
accumulate fast and the table stays tiny. 8 would be too blunt to thread a gap, 32
too slow to learn; 16 is a middle pick, not tied to any measured fact.
---

## Multiplicative penalty (×0.45), additive reward (+0.10)
**Status**: default
**Location**: crates/unendlich-engine/src/sim/path_memory.rs
Blocks should shun a heading fast (one wall hurts) while clear runs reward slowly,
so the creature commits to a working detour instead of oscillating. Multiplicative
decay on penalty gets that asymmetry cheaply. The constants are feel-based
defaults; the convergence test only depends on penalty < reward-recovery, not the
exact values.
---

## Clearance is capped at 4× its base
**Status**: default
**Location**: crates/unendlich-engine/src/sim/path_memory.rs
Stuck creatures widen their clearance estimate so they stop trying to thread tight
gaps, but an unbounded estimate would make a cornered mob refuse everything. 4×
base is an arbitrary-but-bounded ceiling that keeps the "grows-but-bounded"
property; no named fact behind the 4.
---

## Touch-ray normal by central difference, fall back to reversed ray
**Status**: considered
**Location**: crates/unendlich-engine/src/sim/touch_rays.rs
A contact normal could just be the reversed ray direction (cheap, always defined)
or a finite-difference of the solidity field (truer to the surface). I do the
finite difference — it gives a real surface normal for sliding/pain later — but
fall back to `-ray` when the field is locally uniform (e.g. inside a solid), which
avoids a zero/NaN normal. Six solidity samples per hit is acceptable since hits
are rare per tick.
---

### Thread 02 · ATMO/RENDER

## Cap footprint impression depth at 12 cm
**Status**: default
**Location**: crates/unendlich-engine/src/footprint_decal.rs
Depth follows a `sqrt(mass)` pressure law scaled by ground softness, then clamps
at `MAX_DEPTH_M = 0.12`. The sqrt gives diminishing returns (a 4× heavier mob is
2× deeper, not 4×) and the cap stops a very heavy step producing an absurd
crater. The numbers (`0.006 m / sqrt(kg)`, `0.12 m`) are unverified eyeball
values, not tuned against art — a placeholder the coordinator can retune.
---

## Fade footprints with an exponential decay accelerated by weather
**Status**: default
**Location**: crates/unendlich-engine/src/footprint_decal.rs
Opacity decays as `exp(-age * rate)` with `rate = (1 + weather*6)/600s`. Chosen
because exponential decay is trivially monotone in both age and weather (the two
required properties) and never goes negative. The 600 s lifetime and 6× weather
acceleration are guesses, not measured.
---

## Use 5 LOD bands keyed to apparent pixel size, not raw distance
**Status**: default
**Location**: crates/unendlich-engine/src/mob_lod.rs
LOD is selected from on-screen pixel diameter (`apparent_px`) rather than world
distance, so the choice is resolution- and FOV-correct (a mob keeps detail
longer on a 4K screen). Bands at `[320,120,48,16]` px give 5 levels (0..=4). The
pixel thresholds are placeholders; the *approach* (pixel-size driven) is the
deliberate part, the exact numbers are `default`.
---

## Drop the smallest mob parts first, threshold scaling linearly with LOD
**Status**: default
**Location**: crates/unendlich-engine/src/mob_lod.rs
`keep_parts` keeps a part while its radius `>= base_radius * (lod+1)`. Linear
scaling makes the kept-set strictly nest across levels (a finer LOD keeps a
superset), which is the property the batcher needs. Linear vs some other curve
was not evaluated — picked for the clean nesting guarantee.
---

## Pin PBR LUT endpoints exactly rather than trusting the sampled curve
**Status**: default
**Location**: crates/unendlich-engine/src/pbr_terms.rs
`fresnel_lut` overwrites entry 0 to `1.0` (grazing) and the last to `f0` (normal
incidence) after sampling Schlick's `^5` approximation, so the endpoints are
exact regardless of table size or float drift. Schlick's exponent of 5 is the
standard approximation; I did not compare against the exact Fresnel equations,
so this is `default`, not `considered`.
---

### Thread 03 · WORLD/PERF

## Use best-fit (not first-fit) for the span allocator
**Status**: considered
**Location**: crates/unendlich-engine/src/world/span_alloc.rs
The perf logs show the octree pool fragmenting — ~2.6 MB free but the largest
contiguous span only ~72 KB, so a full octree upload fails. First-fit tends to
carve into large holes early, accelerating exactly that. Best-fit packs small
allocations into the smallest hole that fits, leaving large holes intact for
large octree uploads. Carving from the front of the chosen span keeps the math
trivial; the cost is an O(n) scan over the free list, which is fine at pool
fragment counts.
---

## Keep the free list permanently coalesced and sorted by offset
**Status**: default
**Location**: crates/unendlich-engine/src/world/span_alloc.rs
`free()` merges with both neighbors on every release, so the list is always
sorted, disjoint, and adjacency-free. This makes `largest_free_span` an O(n)
max and makes the "full free restores the empty arena" invariant hold exactly,
which is what the property tests lean on. The alternative (lazy coalescing) was
not needed at these sizes.
---

## Cap coral mutation at ±0.08 per gene
**Status**: default
**Location**: crates/unendlich-engine/src/world/coral.rs
`MUT_MAX = 0.08` is a small, round drift that keeps offspring visibly related to
parents over a few generations. No fact pins it; it's a starting value to tune
once reefs are visible in-game.
---

## Model coral as a 3-gene haploid genome
**Status**: default
**Location**: crates/unendlich-engine/src/world/coral.rs
The engine's plant `genetics` is diploid; this core is a self-contained haploid
(hue / branchiness / spread) so it compiles and tests standalone under
`rustc --test`. Breeding is a within-range interpolation rather than Mendelian
allele selection — enough to exercise the "bred within parents' range" property;
the coordinator maps it onto the real diploid model when wiring.
---

## Score structure placement by mean flatness under the whole footprint
**Status**: default
**Location**: crates/unendlich-engine/src/world/structure_template.rs
A multi-cell structure should sit on ground that's flat *everywhere it covers*,
so the score is the mean flatness over the footprint rather than a single anchor
cell. Mean (not min) keeps the score monotone in the underlying flatness — the
property the test checks — while still penalizing partly-rough sites.
---

## Adopt a two-tier brain: cheap reactive FSM every tick, rare budgeted experience tier
**Status**: anchored
**Location**: docs/experience-brain/EXPERIENCE_BRAIN_GUIDE.md
Mob behaviour is split into a reactive tier (deterministic, runs every banded tick,
never simulates) and an experience tier (search/planning on a surrogate, runs rarely
for few near-band creatures, distills results back into reactive params). Anchored to
the frame budget: a real behaviour evaluation on the source project cost ~18 s vs a
~16 ms frame, so per-creature per-frame microsim is infeasible. The built cores slot
in: mob_memory = objective store; touch_rays → Signals; path_memory = a reactive
learner; perception (r5) adds the "am I being looked at" signal the guide needs.
---

## ── Round-5 module decisions (merged from thread fragments) ──

### Thread 01 · SIM

## Blind-spot cone widens linearly with body size
**Status**: considered
**Location**: crates/unendlich-engine/src/sim/perception.rs
The brief wants big bodies that can't see behind/beneath them. I weighed a fixed
rear cone (same for all) vs. one that scales with size. Fixed would make a dragon
no blinder than a rabbit, defeating the "blind cave dragon" use; so the rear cone
half-angle grows `base + per_size·(size−1)`, and a large body is additionally
blind to points more than `size` below it. Linear (not quadratic) keeps a huge
dragon from going fully blind.
---

## Awareness = noise (always) + sight (gated), weighted 0.5 / 0.7
**Status**: default
**Location**: crates/unendlich-engine/src/sim/perception.rs
Hearing should work even in the blind spot (you hear what you can't see), so the
noise term is unconditional while the sight term is gated by range/LOS/blind-spot.
The 0.5 / 0.7 weights are feel-based defaults; the tests only depend on each term
being positive and monotone, not the exact split.
---

## 16 heading bins in nav_query, matching path_memory
**Status**: anchored
**Location**: crates/unendlich-engine/src/sim/nav_query.rs
`BINS = 16` is duplicated from `path_memory::BINS` so the flow-field reader and the
learned-bin writer index the same compass — a mismatch would misalign blocked/
scored bins against the chosen heading. Anchored to that existing constant, not a
free pick; if path_memory's bin count changes, this must change with it.
---

## Step allowance: 0.6 base + 0.55/size, capped at 4.0
**Status**: default
**Location**: crates/unendlich-engine/src/sim/step_height.rs
Replaces a single baked step height. The numbers give a baseline body ~0.6 blocks
and a size-3 body ~1.7, capped so nothing teleports up a cliff. All three are
feel-based defaults; the property test only relies on monotone-in-size and the cap.
---

## Wet ⇒ Lightning/Dust, never Fire; cover ⇒ arcing element
**Status**: default
**Location**: crates/unendlich-engine/src/sim/dragon_elemental.rs
A readable, total policy: water douses fire (so wet switches to a conducting bolt
or close-range dust), and a bolt is easy to hide from behind cover (so cover
prefers area/arcing Air/Dust). These are intuitive game-logic defaults, not tuned
against measured outcomes; the table is exhaustive so every situation maps to one
element.
---

### Thread 02 · ATMO/RENDER

## Advect the storm field with first-order donor-cell (upwind), not semi-Lagrangian
**Status**: considered
**Location**: crates/unendlich-engine/src/storm_field.rs
The required property is "no spontaneous airborne gain". Semi-Lagrangian
backtrace sampling (used by `moisture_field`) is not strictly conservative and
can locally gain mass, which would make that property flaky. Donor-cell upwind
is exactly conservative except for boundary outflow, so total water is provably
non-increasing. The trade-off (more numerical diffusion / smearing) is fine for a
coarse cloud field, so I went with upwind here even though the sibling moisture
field uses MacCormack.
---

## Clamp the storm CFL number to 1 and keep airborne fields in [0,1]
**Status**: default
**Location**: crates/unendlich-engine/src/storm_field.rs
`wind*dt/cell` is clamped to `[-1,1]` so a single step never advects more than
one cell (upwind stability limit), and humidity/cloud/charge are clamped to
`[0,1]`. Bounds are nominal "fraction of saturation" units, not physical — a
placeholder scale the coordinator can rescale.
---

## Generate lightning with splitmix64 seeded per call, segments capped at 256
**Status**: default
**Location**: crates/unendlich-engine/src/lightning_path.rs
A tiny inlined splitmix64 gives deterministic-per-seed geometry with no std RNG
and no clock (R11). The trunk descends in fixed `step` increments with the final
node pinned exactly to the ground plane so the "reaches ground" property is
exact. The 256-segment cap and 0.25 branch chance are eyeball values for "looks
like a bolt", not tuned.
---

## Seam-smooth with a clamped one-ring Laplacian (lambda in [0,1])
**Status**: default
**Location**: crates/unendlich-engine/src/mesh_smooth.rs
Blending each vertex toward the *mean* of its one-ring by a factor clamped to
`[0,1]` makes every blended colour/position a convex combination of its inputs,
which guarantees the "no overshoot" property for free. Normals are blended then
renormalized, with a fallback to the original normal when the blend cancels to
near-zero (opposing normals) to avoid a NaN.
---

## Model the tornado as a Rankine vortex
**Status**: considered
**Location**: crates/unendlich-engine/src/tornado_vortex.rs
Tangential speed ramps linearly to a peak at the core radius then falls as `1/r`
(Rankine), rather than a Gaussian/Lamb-Oseen profile. Rankine was chosen because
the required property — tangential speed falls with radius outside the core — is
exact and trivially testable, and it is cheap. Inflow and lift simply fade as
`core/r` outside the core; those falloffs are `default` guesses.
---

### Thread 03 · WORLD/PERF

## Use 4-connected BFS for the flow field
**Status**: considered
**Location**: crates/unendlich-engine/src/world/flow_field.rs
The brief allowed BFS or eikonal. I chose 4-connected BFS: it gives integer,
exactly-monotone step costs (each cell's flow neighbor is exactly cost-1), which
makes the "strictly downhill to a goal" property checkable and the GPU port a
plain wavefront. Eikonal / 8-connectivity would give smoother diagonal paths but
fractional costs and a fuzzier monotonicity guarantee; not worth it for a nav
pre-map a mob just descends.
---

## Scale water outflow by available water (two-pass step)
**Status**: default
**Location**: crates/unendlich-engine/src/world/water_cell.rs
Outflows are computed against a frozen surface snapshot, then each cell's total
outflow is scaled down if it would exceed the water present. This guarantees
non-negativity and exact conservation (every move is a paired out/in) without
ordering effects. The alternative (clamp after the fact) can lose or duplicate
mass; this doesn't.
---

## Per-neighbor water move = flow_factor · 0.25 · surface_diff
**Status**: default
**Location**: crates/unendlich-engine/src/world/water_cell.rs
The 0.25 keeps a cell's per-step move to any neighbor at or under a quarter of
the surface difference (well under the half-difference overshoot bound even with
all four neighbors active), so surfaces settle monotonically toward flat instead
of ringing. `flow_factor` is the single knob to speed/slow settling.
---

## Clamp source efficiency to [0,1] so replenish ≤ rain
**Status**: default
**Location**: crates/unendlich-engine/src/world/river_source.rs
Efficiency is clamped to [0,1] before use, which is what makes "replenish never
exceeds the rain rate" hold as a hard property (in addition to the explicit
max_rate cap and the capacity clamp on the stored level). A misconfigured
efficiency > 1 can't manufacture water.
---

## ── Round-6 module decisions (T02+T03; T01 pending) ──

### Thread 02 · UI/CONFIG

## from_kv never fails — defaults missing/garbage fields then clamps
**Status**: default
**Location**: crates/unendlich-engine/src/settings_schema.rs
`Settings::from_kv` returns a plain `Settings`, not a `Result`: any missing or
unparseable key falls back to the default for that field and the whole thing is
then clamped, so a partial/corrupt settings file always loads to something valid
rather than erroring the user out of the menu. The setting ranges (render
distance 2..=64, shadow 512..=4096, etc.) are placeholder limits, not tuned
against the renderer.
---

## Normalize biome weights to sum to 1; all-zero falls back to uniform
**Status**: default
**Location**: crates/unendlich-engine/src/world_params.rs
`normalized()` clamps weights non-negative and divides by their sum so the mix is
a probability distribution `validate()` can check exactly. The degenerate
all-zero case maps to a uniform `1/n` mix rather than erroring, so the wizard
can't produce an unusable empty world from a slider-reset. An empty biome list
(n=0) is the one case left invalid on purpose.
---

## Back the whitelist with BTreeSet, not HashSet
**Status**: considered
**Location**: crates/unendlich-engine/src/whitelist.rs
A `BTreeSet<String>` gives deterministic iteration/serialization order for free,
which the parse↔serialize round-trip property and R11 (determinism) both need;
`HashSet` would require sorting at serialize time and has nondeterministic
iteration. The small ordered-set overhead is irrelevant at whitelist sizes, so
BTreeSet won.
---

## server_connect takes an allow-list slice, not the Whitelist type
**Status**: considered
**Location**: crates/unendlich-engine/src/server_connect.rs
`is_allowed` accepts `&[String]` rather than importing `whitelist::Whitelist`.
The alternative (depend on the whitelist module) would couple the two and break
standalone sandbox testing of this module. Passing the slice (the coordinator
hands it `Whitelist::entries()`) keeps it self-contained and R1-pure.
---

## Default server port 25565, conservative host charset
**Status**: default
**Location**: crates/unendlich-engine/src/server_connect.rs
`DEFAULT_PORT = 25565` (the familiar Minecraft-style default) and the host
validator only allows `[A-Za-z0-9.-]`. This deliberately rejects IPv6 literals
and unusual hostnames — a conservative placeholder for the connect field that
the coordinator can widen if real deployments need it.
---

### Thread 03 · WORLD

## Report the face entered, with the normal pointing back toward the ray
**Status**: default
**Location**: crates/unendlich-engine/src/world/voxel_raycast.rs
The raycast returns the face it crossed *into* the solid voxel, and
`face_normal` points back toward where the ray came from. That makes the
break/place split trivial and unambiguous: break removes `hit.cell`, place uses
`hit.cell + face_normal` (the empty cell the ray was in). This is the standard
convention; calling it out so the wiring side doesn't invert the normal.
---

## DDA tie-break order X → Y → Z on exact voxel corners
**Status**: default
**Location**: crates/unendlich-engine/src/world/voxel_raycast.rs
When a ray crosses an exact edge/corner (two or three `t_max` equal), the
strict-`<` comparisons resolve to X first, then Y, then Z. The choice is
arbitrary but must be fixed so the pick is deterministic across the CPU here and
any later GPU port; documenting it as the canonical order.
---

## Per-region permille gate for giant caves (default 12‰)
**Status**: default
**Location**: crates/unendlich-engine/src/world/giant_cave.rs
Spawn rarity is a hash of `(region, seed)` mod 1000 compared against a permille
chance, so it's O(1) per region and seed-stable. 12‰ (~1.2%) is a starting
rarity with no fact pinning it; it's a tuning knob for once caves are walkable
in-game. The radii are clamped into `[min_radius, max_radius]` so a roll can't
produce a cavern larger than the region budget allows.
---

## Manhattan distance + (y, x) tiebreak for /find
**Status**: default
**Location**: crates/unendlich-engine/src/world/locate.rs
`/find` uses Manhattan distance (grid steps, matches how a player reads "N tiles
away") and breaks ties by lowest row then column, so the answer is stable. A
true minimum is asserted by a brute-force property test; Chebyshev or Euclidean
would change which equidistant cell wins but not the nearest result.
---

## ── Round-6 Thread-01 decisions (completes R6) ──

## Each new sim module carries its own ItemId / ItemStack
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/{inventory_grid,crafting,recipe_book}.rs`
The purity gate (`harness/pure_test.sh`) compiles every module standalone with
`rustc --test`, so no module may `use crate::` another. That rules out a shared
item type for now, so `inventory_grid`, `crafting`, and `recipe_book` each define
a local `ItemId(u16)` / `ItemStack`. Unifying these onto one engine-wide item type
(and re-exporting it from `sim`) is the obvious follow-up once they stop being
compiled in isolation; until then consumers reach each by path.

---

## Shaped recipes match by trimmed occupied bounding box, not absolute cells
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/crafting.rs`
A shaped recipe is stored as a trimmed pattern and matched against the trimmed
bounding box of the occupied cells in the grid, so the same shape crafts whether
it sits in the top-left or center of the 3×3. The alternative — matching absolute
cell coordinates — was rejected because it would force one recipe entry per
placement. Internal gaps still matter (they change the trimmed shape), which is
the conventional shaped-crafting behaviour players expect.

---

## Recipe index uses BTreeMap, not HashMap
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/recipe_book.rs`
`RecipeBook` keys its ingredient→recipes index on a `BTreeMap`. Standard-library
`HashMap` has deliberately randomized iteration order, which would make index
build order and any iteration-derived output non-deterministic and so violate the
determinism rule (R11). `BTreeMap` gives a fixed key order at no real cost for a
table this size; the per-ingredient lists are additionally sorted and deduped.

---

## add() returns the leftover stack instead of rejecting an overflowing insert
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/inventory_grid.rs`
When an insert can only be partially placed, `add` deposits what fits and returns
the remainder as a leftover `ItemStack` rather than failing the whole operation.
A reject-whole-on-overflow design would also conserve item totals (it moves
nothing), but leftover-return conserves *and* still makes progress, leaving the
caller to decide what to do with the remainder. This keeps the "total count is
conserved" property total across partial adds.

---

## Per-item stack limits come from a placeholder tier table
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/inventory_grid.rs` (`max_stack`)
`max_stack` returns 1 for ids ≥ 200, 16 for ids in 100..200, and 64 otherwise —
loose tiers for unstackable / small-stack / normal items. There is no real item
registry yet, so the thresholds are reasonable-but-arbitrary and were not weighed
against any data. When a registry exists this should read per-item limits from it;
the function is the single point that has to change.

---

## ── Round-7 module decisions ──

### Thread 01 · SIM

## emissive uses its own `Element` set with `Storm`, not the engine's `Lightning`
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/emissive.rs`
The brief specifies a self-contained palette `{Wood, Water, Air, Dust, Fire,
Storm}`, which differs from the canonical `sim::elemental::Element`
(`{Earth, Dust, Fire, Lightning, Air, Water, Wood}` — `Lightning`, no `Storm`).
The purity gate also forbids `use crate::`, so a local enum is required either
way. I followed the brief verbatim; reconciling `Storm` against the engine's
`Lightning` and sharing one element type is a follow-up for the merge, not
something to guess at here.

---

## emissive intensity is the clamped product of the two inputs
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/emissive.rs` (`emissive_for`)
Intensity is `clamp01(arousal) * clamp01(elem_strength)`. A plain product is the
simplest form that satisfies every required property at once — zero when either
input is zero, monotonic in each, and already within `[0,1]` before the final
clamp. A weighted or curved blend would also work but nothing motivated the
extra knobs yet, so this is a default to revisit if the glow needs shaping.

---

## survival warmth eases with a clamped fraction so large dt can't overshoot
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/survival.rs` (`tick`)
Warmth moves toward ambient by `(ambient - warmth) * clamp01(ease_rate * dt)`.
Clamping the ease fraction to `[0,1]` was the deciding choice over a raw
`ease_rate * dt`: the raw form overshoots (and oscillates) once `dt` is large,
whereas the clamped form asymptotes to ambient for any `dt`, which the
no-overshoot property test pins down. Hunger/thirst likewise reset fully to `0`
on eat/drink rather than decrementing, matching the brief's "reset" wording.

---

## fall_impact softens by `1/(1+resilience)` and gates knockdown on a damage cap
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/fall_impact.rs` (`impact_for`, `tuning`)
Damage is `excess_speed * k * mass_factor / (1 + resilience)`, zero at or below
a safe speed; knockdown fires only once damage crosses a fixed cap. The divide
guarantees resilience can only ever reduce damage (never amplify) and keeps
damage non-negative, while the speed-excess term keeps it monotonic in speed.
The specific constants (safe speed, per-speed scale, knockdown cap) are
placeholder tuning gathered in one `tuning` module; no playtest data backs the
numbers yet.

---

### Thread 02 · RENDER

## Skirt runs along +X at z=0, two vertices per edge sample
**Status**: default
**Location**: crates/unendlich-engine/src/lod_skirt.rs
`skirt_strip` emits `top` then `bottom` per sample at `x = i*cell, z = 0`, so the
output is a flat top/bottom interleave the mesher can turn into a quad strip. The
edge axis (X) and the interleave order are conventions the integration mesher
must agree with; chosen for the simplest index math, not weighed against a
z-axis or bottom-first layout.
---

## Soft-knee bloom prefilter via a C1-continuous quadratic
**Status**: considered
**Location**: crates/unendlich-engine/src/bloom_math.rs
`soft_threshold` uses the quadratic `x²/(4k)` over `[t-k, t+k]`, which is both
value- and slope-continuous with the `l-t` line above the knee — that's what
makes the required "no jump at the threshold" property hold exactly. The
alternative hard-threshold (a `max(l-t,0)` step) is the `knee<=0` branch; I kept
it as a fast path but defaulted to the soft curve because the test demands
continuity.
---

## Clamp luminance to f32::MAX so bloom is Inf-safe on any finite input
**Status**: default
**Location**: crates/unendlich-engine/src/bloom_math.rs
`emissive * exposure` can overflow f32 to `inf` even for finite inputs; every
luminance is `.min(f32::MAX)` and the Reinhard `x/(1+x)` then maps it into
`[0,1)`. Combined with `f32::max/min` dropping NaN, this makes every output
finite for any finite (and even NaN) input. The choice is just defensive
clamping, not a considered model.
---

## Reflect SPIR-V by scanning OpDecorate only; missing set ⇒ set 0
**Status**: considered
**Location**: crates/unendlich-engine/src/spirv_reflect_check.rs
A full SPIR-V parser is overkill for binding parity — only `OpDecorate`
Binding/DescriptorSet pairs are needed, so `reflect` walks instructions by their
word-count header and collects just those, stopping cleanly on a truncated
stream. A resource decorated with `Binding` but no `DescriptorSet` maps to set 0,
matching GLSL's default, rather than being dropped. Considered (vs pulling in a
reflection crate / full parse) and chose the minimal scan for R1 purity + zero
deps.
---

### Thread 03 · WORLD

## Center block predicates on `is_opaque`, with three independent axes
**Status**: considered
**Location**: crates/unendlich-engine/src/world/block_predicates.rs
`is_solid` was overloaded. The split makes opacity (light/occlusion), collision,
and meshability *independent* — the alternative (a single solidity flag with
special-cases) is exactly what broke leaves (collision-solid but light-passing).
Leaf and Water prove the axes are independent (same opacity, different
collision), so the predicate set keeps them separate rather than deriving one
from another.
---

## `produces_face_against` is asymmetric and suppresses same-kind transparent faces
**Status**: default
**Location**: crates/unendlich-engine/src/world/block_predicates.rs
A face draws unless the neighbour is opaque (which seals it), and two identical
transparent blocks (leaf↔leaf, water↔water, glass↔glass) draw no internal face.
Generalizing the "no leaf↔leaf double-draw" rule to all same-kind transparent
blocks avoids z-fighting interior sheets in water and glass too. Different
transparent neighbours still draw.
---

## Remerge over an owned Box-tree, collapsing only all-leaf uniform branches
**Status**: default
**Location**: crates/unendlich-engine/src/world/octree_remerge.rs
A branch collapses only when all 8 children are leaves of one value; nested
uniform branches collapse bottom-up first, so a fully-uniform subtree reduces to
a single leaf in one pass. The owned-`Box` model is for standalone testing; the
coordinator maps it onto the flat `OctreeArray`. Collapse never changes any
sampled cell (value-preserving), which the tests assert by full-grid sampling.
---

## Coalesce rebuilds by cadence, not by edit count
**Status**: default
**Location**: crates/unendlich-engine/src/world/rebuild_coalescer.rs
`should_rebuild` fires on `dirty && elapsed >= cadence`, so a burst of N edits in
one window produces exactly one rebuild regardless of N (pending is kept only as
a diagnostic). The cadence clock resets on `after_rebuild`, bounding rebuilds to
at most one per cadence window — the frequency cap that complements
recompaction's cost cap.
---

## ── Round-8 module decisions ──

### Thread 01 · SIM

## predator_prey: fixed flee-then-hunt priority, ties to the earlier neighbour
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/predator_prey.rs` (`decide`)
The brief fixes the priority order — flee the nearest threat first, only then
hunt the nearest edible-and-not-bigger prey when hungry, else ignore — so the
control flow is anchored to it, not a free choice. The only discretionary part
is tie-breaking: equal distances resolve to the earlier slice entry (a `<=`
guard that keeps the incumbent), which makes the decision deterministic without
needing a secondary key. `HUNGRY_AT` (0.5) and the "size ≤ mine" out-sized test
are the two concrete thresholds the prose left to the implementer.

---

## egg_genome: breed picks per trait from a SplitMix64 hash of (seed, index)
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/egg_genome.rs` (`breed`, `mix64`)
The brief allowed either a per-trait parent pick or a per-trait average; I chose
the pick because it keeps every child byte equal to one parent (a cleaner,
exactly-invertible-feeling inheritance) and avoids rounding questions. Selection
bits come from a SplitMix64 finalizer over `seed ^ index*GOLDEN` rather than a
plain LCG, so adjacent trait indices and nearby seeds decorrelate and "different
seed may differ" holds in practice. Egg packing is a byte copy, making the
`to_egg`/`from_egg` round-trip identity by construction.

---

## feeding_select: single linear value scale, highest wins, ties to lowest index
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/feeding_select.rs` (`value`, `pick`)
Foods are ranked on one ascending scale (Grass 1 < Leaf 2 < Fruit 3 < Meat 4)
and `pick` returns the highest-valued cell the diet can eat, ties going to the
lowest index. A single global scale was the simplest thing that satisfies every
required ordering (e.g. fruit over grass) and needs no per-diet preference
tables yet; per-diet value curves are the obvious extension if a carnivore
should ever rank, say, fresh over stale meat. Hunger gates the whole thing at
the shared `HUNGRY_AT` (0.5) threshold.

---

### Thread 02 · RENDER

## LOD band = count of thresholds the *effective* distance exceeds, clamped
**Status**: default
**Location**: crates/unendlich-engine/src/lod_band.rs
`band` divides distance by `size * quality` so a bigger or higher-quality entity
keeps detail longer, then counts how many ascending thresholds the result clears
and clamps to `bands.len()-1`. Clamping to `len-1` (rather than `len`) means N
thresholds give N bands, collapsing the would-be top bucket — a deliberate match
to the brief's stated clamp. The divide-by-(size*quality) shaping is a `default`
pick, not weighed against e.g. a screen-projection formula.
---

## Cost model distributes each archetype's instances by lod_counts proportions
**Status**: considered
**Location**: crates/unendlich-engine/src/entity_cost_model.rs
`frame_cost` splits every archetype's `instances` across the 4 LODs in proportion
to `lod_counts` (integer floor), counting one instanced draw per non-empty
(arch, LOD) bucket. The alternative — treating `lod_counts` as absolute per-LOD
instance totals independent of each arch — was rejected because it ignores
`Arch::instances` and breaks "cost monotonic in instances". All arithmetic is u64
+ saturating so huge counts can't overflow or panic.
---

## VERTS_PER_PART=24, FRAG_WEIGHT=[8,4,2,1]
**Status**: default
**Location**: crates/unendlich-engine/src/entity_cost_model.rs
A "part" is costed at 24 vertices (a cuboid-ish cluster) and fragment work is
weighted 8/4/2/1 by LOD so nearer instances (more screen coverage) cost more
frag — which is what makes the "far LOD cheaper" property hold for both verts and
frag. The exact numbers are placeholders for the integration profiler to
calibrate.
---

## alloc caps the next-free search at 16 bindings/set; reserve is uncapped
**Status**: considered
**Location**: crates/unendlich-engine/src/descriptor_registry.rs
`alloc` scans `0..MAX_AUTO_BINDINGS_PER_SET (16)` for the smallest free binding so
a "full set" is detectable (returns `None`), while explicit `reserve` accepts any
`u32` binding (sparse high numbers are legal in Vulkan). 16 is a conservative,
testable ceiling, not a hard device limit; chosen over an unbounded search so the
full-set case has a defined answer. Slots are kept sorted for O(log n) lookup and
deterministic iteration.
---

### Thread 03 · WORLD

## All-zero biome weights fall back to a uniform average
**Status**: default
**Location**: crates/unendlich-engine/src/world/biome_blend.rs
When the weight vector sums to ≤ 0 (all zero, or all negative/NaN scrubbed to
zero), `blend` averages the biomes equally rather than returning a zeroed or
arbitrary default. This keeps the output a valid convex combination of real
biomes for any input, so a degenerate weight sample at a seam still yields sane
terrain instead of a flat/black patch.
---

## D8 drainage with high→low topological accumulation
**Status**: default
**Location**: crates/unendlich-engine/src/world/flow_accumulation.rs
Each cell drains to its single lowest 8-neighbor (D8); accumulation is computed
by processing cells in descending height so each cell's upstream total is final
before it contributes downstream — one O(n log n) pass, no iteration to a fixed
point. D8 (vs 4-connectivity) lets channels run diagonally, which reads more
naturally as river valleys. Cells with no lower neighbor are pits; their
accumulations partition the grid (sum to w·h), which the tests assert.
---

## Cap L-system expansion at 65 536 characters
**Status**: default
**Location**: crates/unendlich-engine/src/world/structure_grammar.rs
A growth rule like `A→AA` is exponential, so `expand` stops as soon as the
working string reaches `MAX_LEN` (and also bails early at a fixed point). The cap
is a memory-safety bound, not a tuned value; structures use far less. The turtle
walk derives `kind` from `seed ⊕ index` so positions are seed-independent and
only the decorative kind varies.
---

## ── Round-9 module decisions ──

### Thread 01 · SIM

## nav3d picks the single best unblocked neighbour cell, not a blended vector
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/nav3d.rs` (`step3d`)
`step3d` scores the 26 neighbour offsets by alignment with the desired heading
and returns the unit vector of the single best *unblocked* one, rather than
summing a weighted blend of open directions. The discrete pick was chosen
because it makes the "never head into a blocked cell" guarantee crisp — the
returned direction always snaps back to the open cell it came from — whereas a
blended resultant can point at a blocked cell flanked by open ones. Movement is
gated on strictly-positive alignment, so being hemmed in or having no
goal yields a clean zero. Water ignores `altitude_err`; Air folds a clamped
altitude pull into the vertical axis.

---

## hybrid_rules canonicalizes the pair by element index before lookup
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/hybrid_rules.rs` (`cross`, `order`)
Symmetry (`cross(a,b) == cross(b,a)`) is a required property, so rather than
trusting two mirrored table entries to stay in sync, the pair is ordered by a
fixed element index and only the `(lo, hi)` arm exists. This makes symmetry
structural — impossible to break by editing one direction — at the cost of one
extra comparison. Same-element crossings short-circuit to the pure element at
potency 1.0 before the table; the allowed set itself is a small hand-picked
default open to tuning.

---

## bond_tame keeps trust as the state and steps the stage one notch per event
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/bond_tame.rs` (`advance`, `band`)
Trust (a `0..1` scalar) is treated as the real state; the `Bond` stage is just
the band trust sits in. On each event trust moves by a fixed per-event delta and
the stage steps at most one notch toward the new band. The one-notch rule (using
the incoming stage, so it isn't a dead parameter) is what makes taming a
sustained quest rather than a single lucky feed, and makes a betrayal knock a
tamed creature back out; reaching `Tamed` still requires trust ≥
`TAME_THRESHOLD` (0.8). The deltas and thresholds are placeholder tuning.

---

### Thread 02 · RENDER

## Silhouette LOD keyed on size-normalized distance, breath tied to impostor_scale
**Status**: default
**Location**: crates/unendlich-engine/src/silhouette_lod.rs
`silhouette` divides distance by `size` so a bigger creature keeps real parts and
reads at a longer absolute range, switching to an impostor over a normalized
20→60 band. Breath amplitude is driven by `impostor_scale` (0.05 near → 0.5 far)
so the collapsed impostor still visibly breathes. The thresholds and the breath
range are placeholder feel values, not weighed against alternatives.
---

## Ribbon = 2 verts/point, perpendicular from tangent×up with a +Z fallback
**Status**: considered
**Location**: crates/unendlich-engine/src/ribbon_mesh.rs
The strip emits left/right vertices per path point offset along `normalize(tangent
× up)`. When the path is vertical (tangent ∥ up) the cross product vanishes, so it
falls back to +Z to avoid a NaN — the alternative (an arbitrary stable frame /
parallel-transport) is more correct for twisting ribbons but overkill for a short
breath puff, so I took the cheap fallback. Width tapers linearly to
`width*(1-taper)` at the tail.
---

## Logarithmic froxel slicing, layout (z*ny+y)*nx+x
**Status**: considered
**Location**: crates/unendlich-engine/src/froxel_index.rs
`slice_of` uses `floor(ln(d/near)/ln(far/near) * slices)` (Doom-2016-style
clustered shading) so near slices are thinner — weighed against uniform linear
slicing and chosen because volumetric density detail matters most close to the
camera. Depth is clamped to `[near, far]` so the near plane is slice 0 and the far
plane is the last slice exactly; the flat index is z-major so same-depth froxels
are contiguous.
---

### Thread 03 · WORLD

## River channels grow with sqrt(accumulation), capped
**Status**: default
**Location**: crates/unendlich-engine/src/world/river_carve.rs
Width and depth scale with `sqrt(accum - threshold)` and clamp to a max, so a
basin with 100× the catchment carves a wider valley but not a 100×-deeper
trench. Sub-linear+capped keeps big rivers broad and shallow rather than
bottomless, and the concavity is what the "diminishing gains" test checks. The
base/scale/max constants are placeholders to tune against real terrain.
---

## Cave network is a Prim MST over squared distance
**Status**: default
**Location**: crates/unendlich-engine/src/world/cave_network.rs
A minimum spanning tree connects every cave with exactly `n-1` tunnels and no
redundant links; Prim's grows it from seed 0, always adding the nearest
outside node. Squared distance (i128) avoids a sqrt and stays exact, and
lowest-index tiebreaks make the tree deterministic when distances coincide.
Edges are emitted ordered `(a<b)` so callers can dedup trivially.
---

## Ore is a stateless hash: spawn falls, richness rises with depth
**Status**: default
**Location**: crates/unendlich-engine/src/world/ore_distribution.rs
`ore_at` is a pure hash of `(x,y,z,seed)` with no RNG state, so it's order-free
and reproducible. Spawn chance per band is `SURFACE/(band+1)` (rarer deeper,
floored so the deepest bands aren't empty) and richness is `0.1 + 0.15·band`
capped at 1 (richer deeper). Richness depends only on the band, not the spawn
roll, so "deep favors rich" holds cleanly regardless of which cells roll ore.
---

## ── Round-10 module decisions ──

### Thread 01 · SIM

## sound_genome builds formants cumulatively to keep the decode injective and ascending
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/sound_genome.rs` (`voice_from`)
Two required properties pull against each other: formants must strictly ascend,
and distinct genomes must give distinct voices. Sorting three formant bytes
would guarantee ascending but collapse byte-order permutations to the same voice
(breaking distinctness). Instead each formant is `previous + fixed_gap +
byte*scale`, which ascends by construction and stays injective in every byte
(the cumulative differences recover the bytes). `mimic` interpolates from a
fixed base voice, and `lerp` returns its endpoints exactly so a fully-skilled
mimic matches the target bit-for-bit rather than a ULP off.

## landmark_memory evicts the weakest only when the newcomer beats it
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/landmark_memory.rs` (`observe`)
At capacity, `observe` finds the least-salient landmark and replaces it only if
the new observation is more salient, so the set always holds the top-N by
salience and a trivial sighting can't displace an important one. Re-observing a
known place reinforces it to the stronger salience rather than duplicating, and
`decay` forgets anything that falls to or below the floor. A linear scan over a
small `Vec` (cap 8) was chosen over a heap or sorted structure — at this size
the simplicity is worth more than the asymptotics.

## grove_decay is a single clamped vitality bar with a separate death floor
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/grove_decay.rs` (`tick`, `alive`)
Vitality is one `0..1` scalar that rises at `GROW_PER_SEC` with the grower
present and falls at `DECAY_PER_SEC` otherwise, clamped each tick; `alive` is a
separate threshold test against `DEATH_FLOOR` rather than treating `0` as death.
Keeping "dead" as a floor above zero (and distinct from the clamp) means a grove
can sit visibly near-dead before crossing the line, and leaves headroom to tune
the floor without touching the growth math. Negative `dt` clamps to zero so a
tick never runs the grove backwards. Rates and floor are placeholder tuning.

### Thread 02 · RENDER

## Impostor captures span only [0, π]; the back half reuses a flipped capture
**Status**: considered
**Location**: crates/unendlich-engine/src/impostor_card.rs
`card` exploits a creature's rough left/right symmetry: it captures `n_angles`
views over the half-circle `[0, π]` and covers the back half by reflecting the
yaw and returning `flip = true`, halving the captures needed for full-circle
coverage. Weighed against capturing the full `2π` (simpler indexing, no flip) and
chose the flip scheme because impostor atlas memory is the real cost. Captures are
placed at the interval endpoints (`0` and `π` inclusive) so head-on and profile
views are exact.
---

## Gradient sky: brightness = max(sin(sun_elev),0), warmth peaks within 0.4 rad of horizon
**Status**: default
**Location**: crates/unendlich-engine/src/sky_gradient.rs
Brightness is driven by `max(sin(sun_elev), 0)` so it's 0 at/below the horizon and
rises monotonically to the zenith; the horizon tint lerps from pale blue to sunset
orange as the sun comes within ~0.4 rad of the horizon, nudged by turbidity. This
is a look-driven gradient, not a Preetham/Hosek physical model — the palette
constants and the 0.4 rad sunset window are tunable placeholders.
---

## God-ray as a decay-weighted *average* of sun-proximity (bounded by construction)
**Status**: considered
**Location**: crates/unendlich-engine/src/godray_math.rs
`godray` accumulates per-sample sun proximity weighted by `(1-decay)^i` and
divides by the summed weights, making the result a weighted mean in `[0,1]` — so
it can't blow up with sample count, unlike a raw additive accumulation. Because
the march goes *toward* the sun (later samples brighter), attenuating later
samples means a higher `decay` yields less shaft, matching the brief's
convention. The raw-sum alternative was rejected for needing an extra,
sample-count-dependent normalization to stay bounded.
---

### Thread 03 · WORLD

## Thermal erosion: 4-connected, move 0.5·(steepest excess), double-buffered
**Status**: default
**Location**: crates/unendlich-engine/src/world/hydraulic_erosion.rs
Each iteration moves `0.5 × (steepest_drop − talus)` out of a cell, split among
its orthogonal lower-than-talus neighbors in proportion to their drops, all from
one snapshot. The 0.5 factor stays under the overshoot bound so the global peak
only ever falls (the monotonicity the tests check), and orthogonal-only
neighbors keep the talus threshold a true height drop rather than a diagonal
one. Mass is conserved exactly because every move is a paired out/in.
---

## Dungeon rooms by rejection sampling, corridors by MST
**Status**: default
**Location**: crates/unendlich-engine/src/world/dungeon_layout.rs
Rooms are placed by hashing the seed with the room index and attempt (up to 16
tries each), keeping a candidate only if it clears existing rooms by ≥ 1 cell —
so the result is ≤ n rooms with guaranteed spacing, no global solver. Corridors
are a Prim MST over room centers: every room connected, no redundant tunnels.
Both are seed-deterministic.
---

## Mine time = hardness / (tier + 1), gated by a required tier
**Status**: default
**Location**: crates/unendlich-engine/src/world/mineral_hardness.rs
A tool below the block's `required_tier` returns `None` (can't harvest); above
it, time is `hardness / (tier + 1)`, which is strictly positive, rises with
hardness, and falls monotonically with tier. Hardness/required-tier live in a
small match with a mid-value default for unknown ids, so any `u8` block behaves
sensibly. Values are placeholders to balance later.
---

## ── Round-11 module decisions ──

### Thread 01 · SIM

## boids reduces each rule to a unit direction before applying its gain
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/boids.rs` (`step`)
Each of separation/alignment/cohesion is normalized to a unit direction (or
zero) and then scaled by its gain, so the total acceleration is bounded by
`sep + ali + coh` no matter how the flock is arranged. The alternative — summing
raw inverse-distance separation forces — gives more physically graded crowding
but goes unbounded as two boids overlap, which is exactly the runaway the
"bounded" property forbids. Closeness still matters: it weights the separation
direction before that vector is normalized. Empty neighbours return zero so a
lone boid coasts.

## dragon_language packs intent in the high byte and a 16-bucket mood in the low nibble
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/dragon_language.rs` (`encode`, `similar`)
A symbol is `intent << 8 | mood_bucket`, with mood quantized to 16 buckets so
nearby moods speak the same `u16` (dragons don't transmit infinite precision)
and the round-trip is exact in intent, bucket-exact in mood. `similar` splits
its score 50/50 between intent match and mood closeness, which makes
`similar(x,x)=1` and caps any cross-intent pair below a full match — the
cheapest scheme meeting every required property. The 16-bucket count and the
50/50 split are placeholder tuning.

## input_prediction repeats the held input up to a cap, then relaxes to neutral
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/input_prediction.rs` (`predict`, `InputBuffer`)
Prediction repeats the last confirmed input — the standard rollback assumption
that a player keeps holding what they held — but only up to `MAX_HELD_TICKS`,
after which it relaxes to neutral rather than holding a button forever (the
alternative, repeat-indefinitely, mispredicts badly on a dropped release and
costs extra rollbacks). The buffer is a fixed `[Option<(tick, Input)>;
BUFFER_TICKS]` ring; storing a tick whose slot holds an older tick wins, and
`get` confirms the slot's tick matches so a wrapped-out tick reads as missing
(then predicted). Fixed array, integer-only — bounded and replay-exact.

### Thread 02 · RENDER

## Cascade splits use the PSSM log/uniform blend (lambda)
**Status**: considered
**Location**: crates/unendlich-engine/src/shadow_cascade.rs
`splits` blends a uniform split (even in view depth) with a logarithmic split
(even in perspective) via `lambda`, the standard practical-split scheme — log
alone wastes texels far away, uniform alone wastes them up close, so the blend is
the usual compromise. I return the `n` cascade far-distances (last == far) and
`cascade_of` picks the first split `>= depth`, which keeps the depth→cascade map
trivially monotone. Considered pure-log and pure-uniform (they're just lambda
1/0) and exposed the blend instead of picking one.
---

## FBM normalized by summed amplitude so it's bounded for any octave count
**Status**: default
**Location**: crates/unendlich-engine/src/cloud_noise.rs
The value-noise basis is in `[0,1]` and FBM divides by the sum of octave
amplitudes, so the result is a weighted average that stays in `[0,1]` no matter
how many octaves — that's what the "more octaves still bounded" property needs.
Smoothstep interpolation gives C1 continuity (small position change → small
density change). The splitmix-style lattice hash and the 16-octave cap are
default picks, not benchmarked.
---

## SSR distance fade is linear; step_size = max_dist/steps
**Status**: default
**Location**: crates/unendlich-engine/src/ssr_params.rs
`fade` falls linearly from 1 at the camera to 0 at `max_dist` (a smoothstep/quad
would also satisfy the props but linear makes the monotone+endpoint behaviour
obvious and is cheap). `march` divides `max_dist` evenly into `steps`, treating
`steps==0` as one step so `step_size` stays finite, and clamps thickness
non-negative. All default conveniences, nothing weighed.
---

### Thread 03 · WORLD

## Tunnels are capsule SDFs with a parabolic mid-bulge
**Status**: default
**Location**: crates/unendlich-engine/src/world/cave_carve.rs
`carved` tests point-to-segment distance against the radius — a capsule, so the
tunnel has rounded end caps that join cleanly where network edges meet at a
shared node. `radius_at` adds `bulge·(1 − (2t−1)²)`, a parabola that's zero at
the endpoints and `bulge` at the middle, so bores widen into chambers without a
discontinuity at the joins. A degenerate (zero-length) segment falls back to a
sphere, which the tests confirm.
---

## Chunk priority = 1/(1+dist) plus fixed frustum/dirty boosts, stable sort
**Status**: default
**Location**: crates/unendlich-engine/src/world/chunk_priority.rs
Distance maps through `1/(1+dist)` into `(0,1]` (nearer = higher, bounded, no
divide-by-zero), and frustum/dirty add constant boosts so a visible or
just-edited chunk jumps the queue. `order` uses a stable descending sort, so any
secondary ordering the caller already applied (e.g. by chunk id) survives ties —
the property the stability test pins.
---

## Decimation forces the last kept slot to the true endpoint
**Status**: considered
**Location**: crates/unendlich-engine/src/world/mesh_decimate.rs
Striding by `1<<lod` yields `ceil(n/stride)` vertices but its last pick is rarely
the real last vertex, which would crack LOD seams. Rather than append the
endpoint (which would break the exact `ceil(n/stride)` count the brief asks for),
the final kept slot is overwritten with `verts[n-1]`. That preserves both the
count and the endpoint; the alternative of appending was rejected for breaking
the count contract.
---

## ── Round-12 module decisions ──

### Thread 01 · SIM

## mimic_learn keeps a weighted sum and blends neutral→mean by a saturating confidence
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/mimic_learn.rs` (`observe`, `rendered`)
The Mimic stores only an exposure-weighted running sum and total weight, not a
history; `rendered` is `lerp(neutral, weighted_mean, weight/(weight+HALF))`. The
saturating confidence is the key choice over a plain running mean: a plain mean
would snap to a target after a single observation (mean of one sample is that
sample), giving no "learns gradually" behaviour, whereas the confidence term
makes a briefly-heard voice only faintly imitated and a long-heard one nearly
exact. Voices heard longer dominate the mean; neutral is the all-zero blank
slate. `HALF_EXPOSURE` is placeholder tuning.

## pollination_route compares squared distance and caps tracking at 64 flowers
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/pollination_route.rs` (`next_flower`)
The nearest unvisited bloom is found by comparing squared Euclidean distance
(no `sqrt` needed to order distances), with ties broken to the lowest index for
determinism. The `visited` set is a `u64` bitmask, so only the first 64 flowers
are individually trackable — indices ≥ 64 are treated as never-visited rather
than risking a shift overflow. This matches the brief's chosen type and is fine
for the small flower clusters a carrier routes within; a larger world would need
a wider mask.

## combat_resolve divides by defence and clamps knockback, with a crit threshold
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/combat_resolve.rs` (`resolve`)
Damage is `atk / (1 + def)` — monotonic up in attack, down in defence, and never
negative without needing a separate floor — multiplied by `CRIT_MULT` when the
crit roll clears `CRIT_THRESHOLD`. Knockback scales with damage but is clamped to
`KNOCKBACK_MAX` so an extreme hit can't launch a creature arbitrarily far. The
divide form was taken over subtractive mitigation (`atk - def`) because it can't
go negative and degrades gently; it is PvE-only by construction (no
player-as-target path), so it can't be turned into a griefing tool. Constants are
placeholder tuning.

### Thread 02 · RENDER

## Atmosphere: Rayleigh (1+cos²θ) + Henyey–Greenstein Mie (g=0.76)
**Status**: considered
**Location**: crates/unendlich-engine/src/atmosphere_scatter.rs
The in-scatter is a phase-weighted sum: the analytic Rayleigh phase `(1+cos²θ)`
(blue-heavy tint, naturally even in θ so the symmetry property holds) plus a
Henyey–Greenstein Mie lobe with `g=0.76` for the forward haze peak, its weight
scaled by turbidity. HG was chosen over the cheaper Schlick or Cornette–Shanks
approximations because it gives the cleanest forward-peak property with one
parameter; the coefficients are a normalized look, not radiometric.
---

## Vignette = 1 - strength·r² (normalized corner radius), grain from a bit-hash
**Status**: default
**Location**: crates/unendlich-engine/src/vignette_grain.rs
Vignette uses a normalized squared radius (0 at centre, 1 at a corner) so the
centre is exactly 1 and strength scales the corner darkening, clamped to `[0,1]`.
Grain hashes the raw float bits of `(uv, t)` with a splitmix mixer into
`[-amount, amount]`, which is deterministic per pixel/frame without any RNG state
or clock. Both are default conveniences.
---

## Caustics: integer t-frequencies for an exact 2π period
**Status**: considered
**Location**: crates/unendlich-engine/src/water_caustics.rs
The pattern sums four moving sine wavefronts and squares `|sum|` for the ridged
filaments. I deliberately used only integer time-frequency multipliers (±1, ±2)
so the whole field is exactly `2π`-periodic in `t` — the alternative
(incommensurate frequencies for a less repetitive look) makes the "periodic-ish"
property only approximate, so I traded a little visual variety for a clean,
testable period.
---

### Thread 03 · WORLD

## Scatter density = biome_base · moisture · (1 − slope)
**Status**: default
**Location**: crates/unendlich-engine/src/world/vegetation_scatter.rs
A multiplicative model keeps density in [0,1] for free and makes each factor an
independent veto: a barren biome (base 0), bone-dry ground (moisture 0), or a
cliff (slope 1) each drive density to zero. `place` then thresholds a per-cell
hash against the density, so the realized fraction of planted cells tracks the
density (the test checks ~0.3 of cells plant at density 0.3).
---

## Smoothstep biome transition over a width-scaled zone
**Status**: default
**Location**: crates/unendlich-engine/src/world/biome_transition.rs
`edge_dist` is remapped to `t ∈ [0,1]` across `[-width/2, +width/2]` and run
through `t²(3−2t)`, so the blend eases in/out (zero-derivative at both ends)
instead of a linear crease. Width scales the zone: the same distance is "less
far along" the transition in a wider zone. A non-positive width degrades to a
hard edge at 0 rather than dividing by zero.
---

## Vein length counts the start; veins are 6-dir unit-step walks
**Status**: default
**Location**: crates/unendlich-engine/src/world/mineral_vein_trace.rs
`trace` returns `max(1, len)` cells: `len` includes the start, so `len=0` and
`len=1` both yield just the start and every consecutive pair is axis-adjacent
(connectivity is structural, not checked at runtime). Steps keep the current
heading ~75% of the time and otherwise turn to a hash-chosen axis, giving a
wandering seam; the ~25% turn rate is a feel knob, not a pinned value.
---

## ── Round-13 module decisions ──

### Thread 01 · SIM

## batch_perception/settle/boids: parity guaranteed by a shared inline kernel
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/batch_perception.rs`, `batch_settle.rs`, `batch_boids.rs`
The round's hard requirement is that each batched core be bit-for-bit identical
to its scalar reference. Rather than maintain two arithmetic implementations and
hope they stay in lockstep, each scalar `*_one` and its `*_batch` route through
one `#[inline]` kernel (for `batch_boids`, the batch calls `boid_one` directly),
so identical floating-point results are structural, not a coincidence to be
re-verified after every edit. The parity tests compare `f32::to_bits()`, not an
epsilon, to catch any drift exactly. The microopt therefore lives entirely in
the call/iteration shape — contiguous slice in, contiguous `Vec` out, one
streaming pass — never in the math.

---

## batch_boids defines a flock as all-other-agents, summed in index order
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/batch_boids.rs` (`boid_batch`)
`boid_batch` interprets "the batch" as one flock where each agent's neighbours
are every other agent, gathered in ascending index order before calling the
scalar kernel. Index-order gathering is the deciding detail: boid math sums
neighbour contributions, and float addition isn't associative, so a different
neighbour order would change low bits and break parity with a same-order scalar
reference. Fixing the order makes the batch both deterministic and bit-identical
to the obvious scalar loop. Pairwise forces come out exact negations (`a-b` is
the exact IEEE negation of `b-a`, preserved through normalize), which the
symmetric-pair test checks with `==`.

### Thread 02 · RENDER

## Pipeline key layout: lod(4)|pass(4)|material(8)|double_sided(1), encode returns Option
**Status**: considered
**Location**: crates/unendlich-engine/src/pipeline_key.rs
Fields are packed lod-major so a raw key sort already groups by LOD then pass then
material — useful for building the pipeline table. `encode` returns `Option<u64>`
(None on invalid) rather than a silently-masked key, so an out-of-range lod or a
pass `>= NUM_PASSES` is a hard reject at encode time. Considered a fixed-width
checked struct vs a packed int and chose the packed int because the same bits
double as a sort key in `draw_key`.
---

## Batched emissive routes through the scalar fn to guarantee bit-parity
**Status**: considered
**Location**: crates/unendlich-engine/src/batch_emissive.rs
`emissive_batch` deliberately calls `emissive_one` per element instead of
re-deriving the arithmetic in a fused loop. The brief demands bit-parity with the
scalar reference; re-implementing the math (even "equivalently") risks fma/reassoc
drift, so the only safe microopt is amortizing call/allocation overhead while
keeping one source of truth for the per-element computation. The float path
(Reinhard + clamped glow) mirrors the R7 bloom convention.
---

## Draw key: depth bucket high (24b), pipeline low (40b), near→0
**Status**: default
**Location**: crates/unendlich-engine/src/draw_key.rs
Depth is quantized to 24 bits in the high word (nearest = 0) and the pipeline key
masked into the low 40 bits, so an ascending sort yields near→far ordering for
early-Z with equal-depth draws grouped by pipeline state. The 24/40 split and the
linear depth quantization are default picks; a logarithmic depth bucket (à la the
froxel slicing) would pack near detail better but wasn't needed for sort
correctness.
---

### Thread 03 · WORLD

## Edit dirty AABB is the cell ± 1, clamped; OOB is a silent no-op
**Status**: default
**Location**: crates/unendlich-engine/src/world/block_edit.rs
Changing a block can flip the visible faces of its six neighbors, so the remesh
bounds are the edited cell expanded by one in every direction, clamped to the
slab. An out-of-bounds position (or an undersized slab) returns `changed=false`
with no write and no panic, so a stray pick can never corrupt or crash — the
caller treats `changed=false` as "nothing to remesh".
---

## Coalesce merges overlap-or-touch (gap 0) and sorts the output
**Status**: considered
**Location**: crates/unendlich-engine/src/world/edit_coalesce.rs
Two AABBs merge when, on every axis, they overlap or sit with no empty cell
between them (`b.min ≤ a.max+1`); a one-cell gap keeps them separate. The merge
runs to a fixpoint (a union can create new adjacencies), and the result is
sorted so it's identical regardless of input order. The alternative — merging
only on strict overlap — was rejected because face-adjacent edits would remesh
as two boxes sharing a seam, which is exactly the redundancy this removes.
---

## Minimality = no branch of 8 same-valued leaves
**Status**: default
**Location**: crates/unendlich-engine/src/world/octree_invariants.rs
With the fixed 8-ary `Node`, structure is always well-formed, so the only audit
that matters is minimality: a branch whose eight children are leaves of one
value should have been collapsed. `check` recurses children first (so a nested
violation is reported), then tests the branch — returning the first failure
deterministically rather than collecting all of them.
---

## ── Round-14 module decisions ──

### Thread 01 · SIM

## breathe oscillates with a sine and wraps phase by rem_euclid for a seamless loop
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/breathe.rs` (`breathe`, `scale_at`)
Torso scale is `1 + depth·sin(2π·phase)` with phase advanced and wrapped by
`rem_euclid(1.0)`. The sine was chosen over a triangle/sawtooth specifically for
loop continuity: `sin` makes `phase 0` and `phase 1` produce the same scale *and*
the same slope, so the cycle has no pop or velocity discontinuity at the wrap
(the continuity test pins this across the boundary). `rem_euclid` keeps the phase
in `[0,1)` even for negative `rate·dt`, and `depth` is clamped to `[0,1]` so the
torso scale stays non-negative. It uses `f32::sin`, which is deterministic
per-platform (consistent with this tier's other `f32` cores).

## vocal_pose drives jaw and neck from one amp×expressiveness product, neck at 0.8×
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/vocal_pose.rs` (`vocal_pose`)
Both outputs come from a single `drive = clamp01(amp)·clamp01(expressiveness)`,
with the neck raised at `0.8×` the jaw opening so speech reads as mostly mouth
with a supporting sway. Sharing one drive term makes silence→neutral, monotone-
in-amplitude, and expressiveness-scaling all fall out together and stay
consistent between the two joints. The `0.8` neck factor is placeholder animation
tuning, not derived from anything.

## batch_gene shares one inline kernel and zero-fills short genomes for parity
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/batch_gene.rs` (`kernel`, `express_one`, `express_batch`)
As with the round-13 batch cores, bit-for-bit parity between `express_one` and
`express_batch` is made structural: both call one `#[inline]` kernel, so they
can't diverge on a later edit (the test compares `to_bits()`). Genomes are read
by fixed byte index with `get(i).unwrap_or(0)`, so a short or empty genome
zero-fills rather than panicking — important when a spawner feeds a whole brood
of possibly-truncated genomes in one batch. The expression is a streaming pass
over a slice of slices into a contiguous `Vec`, the actual spawn-speed win.

### Thread 02 · RENDER

## Armor: linear reflectivity 0.02→0.87, specular exponent hardens + gloss micro-spec
**Status**: default
**Location**: crates/unendlich-engine/src/armor_reflect.rs
Reflectivity lerps from a matte dielectric base (~0.02) to ~0.87 with armor, and
the specular exponent goes 4→124 with armor plus up to +120 from gloss
(micro-spec), capped at 256. The linear maps and the 4/120/256 constants are
placeholder feel values, not weighed against a physically-based metalness/
roughness parameterization — that conversion is the integration shader's job.
---

## Wing: alpha 0.08→1.0, leather_mix == gene
**Status**: default
**Location**: crates/unendlich-engine/src/wing_opacity.rs
Alpha lerps from a glassy 0.08 baseline (so the membrane is faintly visible even
at gene 0) up to fully opaque at gene 1, and `leather_mix` is just the clamped
gene so the tint tracks opacity 1:1. Both are simple linear genes; the 0.08
floor is the only non-obvious number and it's a default look choice.
---

## Cadence N = floor((1 + d/16)/(1 + 4·speed)), capped at 8; exact lerp endpoints
**Status**: considered
**Location**: crates/unendlich-engine/src/anim_cadence.rs
N grows with distance and shrinks with speed via a single ratio, floored and
clamped to `[1, 8]`, so fast motion collapses to full-rate (N=1) without a special
case while slow/distant motion downsamples. `downsampled_lerp` special-cases
`sub==0`/`sub==1` to return `prev`/`target` *exactly* (a bare `a+(b-a)·t` can drift
off the endpoint in float), which the endpoint-exactness property requires. The
ratio's 16/4 constants are default tuning.
---

### Thread 03 · WORLD

## Smoke is an advected Gaussian puff: exp fade × gaussian falloff
**Status**: default
**Location**: crates/unendlich-engine/src/world/smoke_field.rs
Density is `exp(-age·rate)` (overall fade) times a gaussian in distance whose
center advects downwind by `|wind|·age` and whose radius grows with age. This
gets all three behaviors — rise/fade, spread, wind-shift — from one closed form
with no state, so the renderer can sample any `(age, dist)` directly. The scalar
`dist_from_source` is treated as radial; wind enters only as the peak's
downwind offset.
---

## Embers: seeded per-particle lifetime, sinusoidal flicker × linear death-fade
**Status**: default
**Location**: crates/unendlich-engine/src/world/ember_field.rs
Each ember hashes its seed into a lifetime in [2,4)s, a spawn offset, a drift,
and a flicker phase, so a whole spray animates from one shared time value with
no per-particle state. Glow is a bounded sine flicker multiplied by a linear
fade to zero at death; past the lifetime the ember reports `alive=false` and
`glow=0` so the renderer can cull it.
---

## Dust motes are multiplicative: biome × light × height-falloff
**Status**: default
**Location**: crates/unendlich-engine/src/world/dust_motes.rs
A product keeps the result in [0,1] and makes darkness a hard veto (light 0 →
0), which is the one behavior that must be exact — you never see motes in
unlit air. Height uses a `1/(1+h·k)` falloff so motes thin out gradually with
altitude rather than cutting off. Per-biome dustiness is a small table with a
mid default.
---

## ── Round-15 module decisions ──

### Thread 01 · SIM

## STRUCTURES_creature design doc: builds flow through the world-edit event path
**Status**: anchored
**Location**: `docs/design/STRUCTURES_creature.md`
The doc anchors creature-building on the existing world-edit path: every block a
builder places/removes is emitted as a deterministic event into the input stream
and applied by `world_authority`, never as direct mutation. That choice is what
makes building rollback-safe and gives a single choke point for capability bounds
(per-tick/lifetime caps, reach, block-kind allow-list), and it lets keystone
groves reuse `grove_decay` (r10) for vitality/reclaim. It's a design doc, not
code, so it's purely additive; the open questions list flags what's still unsettled.

---

## batch_breathe shares one inline kernel with breathe_one for structural parity
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/batch_breathe.rs`
Same approach as the r13/r14 batch cores: `breathe_one` and `breathe_batch` both
call one `#[inline]` kernel (the exact r14 breathe math), so bit-for-bit parity is
structural rather than a coincidence to re-verify, and the test compares
`to_bits()` on both tuple fields. The batch shares one `dt` across a contiguous
slice of `(phase, depth, rate)` — the per-frame herd shape — so the win is in
iteration, not arithmetic.

---

## claw_curl lets grip dominate via a (1-grip)-weighted gait term
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/claw_curl.rs` (`claw_curl`)
Per toe the curl is `clamp01(grip + (1-grip)·gait_i)`. This specific blend was
chosen over a sum or a max because it satisfies two requirements at once that a
naive `grip + gait` cannot: at full grip every toe reads exactly `1.0` regardless
of gait (the gait term is gated to zero by `1-grip`), and curl is monotone in grip
since the weight `1-grip` is non-negative. Toes are offset across the gait cycle by
`i/toes` so the curl rolls across the foot instead of snapping in unison; `toes=0`
yields an empty vector. The roll offset and sine wave are placeholder animation tuning.

### Thread 02 · RENDER

## STRUCTURES_render.md frames structures as reusing the creature LOD/impostor path
**Status**: considered
**Location**: docs/design/STRUCTURES_render.md
The doc deliberately routes static structures through the *same* lod_band /
silhouette_lod / impostor_card / impostor_uv chain built for creatures rather than
proposing a separate structure pipeline, because structures are easier impostor
candidates (static captures stay valid). The main divergences I flag are
shadow-only LOD and static shadow caching, plus open questions on cluster-level
impostors and a yaw×pitch atlas. Considered a bespoke structure path and rejected
it as duplicated machinery.
---

## batch_draw_key routes through the scalar draw_key_one for bit-parity
**Status**: considered
**Location**: crates/unendlich-engine/src/batch_draw_key.rs
Like R13's batch_emissive, the batched generator calls the scalar reference per
item instead of re-deriving the pack in a fused loop, so bit-parity is guaranteed
(the depth→bucket float quantize is identical per call). The only microopt is
hoisting the near/far clamp setup out of the loop; the actual per-item math stays
single-sourced. Mirrors the R13 draw_key layout (depth high 24b, pipeline low
40b) rather than importing it, to keep the module standalone-testable.
---

## impostor_uv: row-major atlas, ceil rows, idx clamped to last cell
**Status**: default
**Location**: crates/unendlich-engine/src/impostor_uv.rs
The atlas is laid out row-major over `atlas_cols` with `rows = ceil(n/cols)`, cell
size `1/cols × 1/rows`, which makes cells exactly tiling and non-overlapping. An
out-of-range `angle_idx` clamps to the last cell (stays in `[0,1]`) rather than
wrapping or panicking. Row-major + ceil are the obvious defaults; a 2D yaw×pitch
atlas is left as an open question in the design doc.
---

### Thread 03 · WORLD

## Thin CPU mesh ring is edited + innermost only; the rest is GPU
**Status**: anchored
**Location**: crates/unendlich-engine/src/world/mesh_tier.rs
Anchored to the RD=24 profile in MESH_TIER_PLAN.md: the CPU `near` mesher is the
bottleneck (p99=242ms, max=519ms) because it's the only high-detail tier.
`mesh_tier` keeps the CPU ring to just the innermost band and chunks edited
recently (synchronous remesh for instant block-edit feedback), routing all other
close chunks to a new full-detail GPU tier. `!gpu_ok` collapses every renderable
tier back to CPU; at/after the last band a chunk is `Skip`. The tier index is
the count of band thresholds the distance has crossed, so band-count drives how
many GPU detail levels exist.
---

## Batched priority reuses the exact scalar formula for bit-parity
**Status**: default
**Location**: crates/unendlich-engine/src/world/batch_chunk_priority.rs
`priority_batch` maps the identical `priority_one` over its inputs rather than
re-deriving a "vectorized" expression, so scores are bit-for-bit equal to the
scalar reference (the test asserts `to_bits()` equality, including the NaN/
negative clamp). The streamer compares scores for ordering, so any reassociation
drift between a scalar fallback and a batch path would shuffle work
nondeterministically; reusing the scalar closes that off.
---

## ── Round-16 module decisions ──

### Thread 01 · SIM

## blink places one hashed pulse per interval so blinks are occasional but seeded
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/blink.rs` (`blink`)
Each `1/rate_hz` interval contains exactly one half-sine shut→open pulse, started
at `hash(seed, interval)·(1-w)` so it always fits inside the interval. This was
chosen over a continuous noise/oscillator approach because it makes "mostly open,
occasional blink" exact (the pulse width `w = BLINK_DUR·rate` is small) while
still being a pure hash of `(seed, interval)` — deterministic, per-creature
distinct, and free of any RNG state to roll back. `rate_hz <= 0` short-circuits to
always-open.

## appendage_ripple ties amplitude to vel/(1+stiffness) and frequency to (1+stiffness)
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/appendage_ripple.rs` (`ripple`)
Amplitude is `|vel|·scale/(1+stiffness)` (clamped) and frequency is
`base·(1+stiffness)`, so faster movement swings more and a stiffer appendage
swings less but oscillates faster — the "stiff spring settles quicker" reading of
the brief. There's no time-since-impulse input, so true damping isn't modelled;
"faster settle" is expressed as higher frequency, which the test checks via zero
crossings. Output is clamped so an extreme velocity can't fling the part away.

## mob_spawn_table rotates the roll by a context hash to keep weight proportions
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/mob_spawn_table.rs` (`pick`)
The candidate list carries only `(species, weight)` with no per-species biome/
light affinity, so context can't reweight individual species. Instead `(biome,
light)` hash into a rotation added to the roll before indexing the cumulative
weights. Rotating a uniform roll stays uniform, so a full sweep still lands on
every positive-weight species in exact proportion (the normalized-proportions and
coverage tests hold), yet the same roll spawns different species in different
contexts. Zero-weight entries are skipped entirely.

## loot_roll rolls each row independently from a hash of (seed, row index)
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/loot_roll.rs` (`roll`)
Each row drops iff `hash(seed, index) < clamp01(chance)`. Hashing the row index
(not just the seed) decorrelates rows so a repeated item id at different table
positions rolls independently, and using `[0,1)` rolls makes chance `1` always
drop and chance `0` never drop without special cases. It's a pure function of
`(table, seed)`, so a chest opens identically on every peer and across rollback.

## item_durability uses a saturating subtract then clamp, broke = (new == 0)
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/item_durability.rs` (`apply_wear`)
`delta` is signed (positive wears, negative repairs); the new value is
`saturating_sub` then clamped to `[0, max]`, with `max` floored at 0. The
saturating subtract is the deliberate bit: it keeps the function total on absurd
inputs (no overflow, no panic) which matters for a path that runs on every item
hit. `broke` is simply whether durability is now 0, so an already-zero item reads
broken even on a no-op.

## batch_blink shares one inline kernel with blink_one for structural parity
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/batch_blink.rs`
Same pattern as the earlier batch cores: `blink_one` and `blink_batch` both call
one `#[inline]` kernel (the exact `blink` math, copied since cross-module imports
are disallowed), so bit-for-bit parity is structural and checked with `to_bits()`.
The batch shares one frame `t` across a contiguous `(rate_hz, seed)` slice — the
per-frame crowd shape — so the win is in iteration, not arithmetic.

### Thread 02 · RENDER

## icon_atlas: explicit cols×rows, row-major, idx clamped to last
**Status**: default
**Location**: crates/unendlich-engine/src/icon_atlas.rs
Same row-major tiling as impostor_uv (R15) but with explicit `rows` (a UI atlas is
a fixed grid, not derived from a capture count). Cells are `1/cols × 1/rows`, so
they tile exactly and never overlap; an out-of-range index clamps to the last cell
instead of wrapping or panicking. All default conventions.
---

## held_item_pose: 6-class lookup table, scale multiplies offset only, consistent fallback
**Status**: default
**Location**: crates/unendlich-engine/src/held_item_pose.rs
A small per-class pose table (sword/tool/bow/food/shield/rod) with one shared
fallback for unknown classes; `scale` multiplies the offset but leaves angles
alone (a bigger sword sits farther out but isn't rotated differently). The
specific offsets/angles are placeholder feel values; the only real decision is
that all unknown classes collapse to one deterministic fallback rather than
erroring.
---

## mob_tint: seed 0 = baseline [1,1,1]; value scales all channels, hue skews R↑/B↓
**Status**: considered
**Location**: crates/unendlich-engine/src/mob_tint.rs
Seed 0 is special-cased to the exact baseline `[1,1,1]` so "the canonical
individual" has no variation, distinct from "ranges are 0" (which is also
identity). The variation model — a value shift scaling all channels plus a hue
shift pushing red up / blue down — was chosen over a full HSV round-trip because
it keeps the result an RGB multiplier near 1 with a trivial `±(hue+val)` bound the
test can check, and stays cheap. Channels are clamped non-negative.
---

## damage_flash: linear 1→0 decay, duration<=0 yields 0
**Status**: default
**Location**: crates/unendlich-engine/src/damage_flash.rs
Linear decay from 1 at the hit to 0 at `duration` (a squared/ease curve would
also satisfy the props; linear makes peak/zero/monotone obvious). A non-positive
duration returns 0 (no flash window) rather than dividing by zero. Default.
---

## batch_pipeline_key routes through pipeline_key_one for bit-parity
**Status**: considered
**Location**: crates/unendlich-engine/src/batch_pipeline_key.rs
Like the R13/R15 batched cores, the batch calls the scalar encoder per element so
keys (and the `None` for invalid descriptors) match bit-for-bit; the microopt is
purely `with_capacity` + a tight contiguous loop, never re-deriving the packing.
Mirrors R13 pipeline_key's layout/validity locally to stay standalone-testable
rather than importing it.
---

### Thread 03 · WORLD

## Domain warp is a low-frequency value-noise offset bounded by amp
**Status**: default
**Location**: crates/unendlich-engine/src/world/terrain_warp.rs
Two value-noise channels (different seed salts) give an `(dx, dz)` offset in
`[-amp, amp]` sampled at a low frequency, so terrain features meander over large
scales without the warp itself introducing high-frequency noise. `amp == 0` is a
hard identity so callers can disable warping cheaply.
---

## Ridges via `1 − |2n − 1|`, amplitude-weighted and normalized
**Status**: default
**Location**: crates/unendlich-engine/src/world/ridge_noise.rs
Folding value noise about ½ creates the crease that reads as a ridgeline; summing
octaves with halving amplitude and dividing by the total amplitude keeps the
result in [0,1] for any octave count (clamped to 12). Normalizing means octave
count changes detail, not overall height range.
---

## Thermal erosion moves rate·0.5·(excess) over a frozen snapshot
**Status**: default
**Location**: crates/unendlich-engine/src/world/thermal_erosion.rs
One in-place step: orthogonal drops above `talus` shed `rate·0.5·(steepest −
talus)`, split among qualifying neighbors, computed from a snapshot and applied
together so mass is conserved exactly and the result is order-independent. `rate`
is clamped to [0,1] so a single step can't overshoot; the caller chooses how many
passes. Same model as `hydraulic_erosion` (R10), exposed with the brief's
`erode_step` signature.
---

## Strata index = bands crossed by (depth + noise jitter)
**Status**: default
**Location**: crates/unendlich-engine/src/world/strata.rs
The tier is the count of ascending depth bands the depth has passed, after the
noise waves the boundary by ±`BOUNDARY_JITTER`. This is monotone in depth for a
fixed noise (the property the test pins) while still giving undulating layers
across space. Empty bands return tier 0 rather than erroring.
---

## Terrace blend ramps the top `blend·step` with a smoothstep
**Status**: default
**Location**: crates/unendlich-engine/src/world/terrace.rs
Each terrace is flat for the lower `(1−blend)·step` then smoothsteps up to the
next level over the top `blend·step`, so the function is continuous for any
`blend > 0` (riser joins both the flat below and the next terrace above) and
collapses to hard steps at `blend = 0`. `step ≤ 0` is an identity guard.
---

## Batched mesh-tier reuses the scalar reference for exact parity
**Status**: default
**Location**: crates/unendlich-engine/src/world/batch_mesh_tier.rs
`mesh_tier_batch` maps the identical `mesh_tier_one` over its items, sharing the
per-frame `gpu_ok`/`bands`, so every tier is identical to the per-chunk call
(including the no-GPU fallback and Skip). Reusing the scalar — rather than a
parallel "optimized" classifier — guarantees a batched fast path and a scalar
fallback can never disagree and reorder the streamer's work.
---

## ── Round-17 module decisions ──

### Thread 01 · SIM

## body_arc distributes the turn by head-weighted normalized weights
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/body_arc.rs` (`arc`)
The turn is split across segments by weights `1 + spread·(1 - i/(n-1))`, head
heaviest, then normalized so the per-segment yaws sum to exactly the commanded
turn. Normalizing (rather than assigning absolute per-segment angles) is the key
choice: it guarantees the body ends up facing the right way regardless of `n` or
`size`, while the decreasing weights give the head-leads/tail-lags look and
`spread ∝ size` makes a longer body curve more. Each share is `< 1` of the total,
so no segment exceeds the turn; `n=1` is the whole turn.

## segment_lerp adds a saturating leg-distance term and a clamped cliff term
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/segment_lerp.rs` (`seg_amp`)
Follow amplitude is `clamp01(d/(d+LEG_HALF) + CLIFF_WEIGHT·clamp01(cliff_gap))`.
The saturating `d/(d+half)` form gives "0 at a leg, rising toward 1 at mid-span"
with a single monotone curve and no separate clamp for the high end, and the
additive clamped cliff term keeps the whole thing monotone in both inputs and
bounded. The half-distance and cliff weight are placeholder tuning.

## threat_assess scores power/(1+dist) and breaks ties to the lowest index
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/threat_assess.rs` (`threat_score`, `top_threat`)
A single `power/(1+dist)` score captures "nearer and stronger is scarier" without
needing separate thresholds, stays non-negative and bounded by power, and never
divides by zero (distance floored at 0, denominator `≥1`). `top_threat` keeps the
incumbent on an equal score, so ties resolve to the lowest index and the pick is
stable and deterministic.

## flee_decision compares threat power to hp+courage with a strict inequality
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/flee_decision.rs` (`should_flee`)
Flee iff `threat_power > own_hp + courage` (inputs floored at 0). Treating resolve
as the simple sum of health and courage makes all the required monotonicities fall
out at once — sooner when hurt, longer when brave, always under overwhelming power
— and the strict `>` means the boundary holds the line on an exact tie, which is
what keeps the fight/flee edge stable and replayable.

## herd_cohesion weights separation above cohesion and ramps it by closeness
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/herd_cohesion.rs` (`herd_steer`)
Cohesion is a unit pull toward the centroid; separation is a unit push from the
nearest neighbour scaled by `(sep_r - d)/sep_r` and weighted `2×` cohesion. The
`2×` weight plus the closeness ramp is what makes separation actually dominate as
neighbours approach (so they never overlap) while still letting cohesion win once
spacing is comfortable — a plain equal-weight sum wouldn't guarantee the inside-
radius dominance the brief asks for. Safe normalization yields zero at rest, and
the output is bounded by `COH_W + SEP_W`.

## batch_appendage_ripple shares one inline kernel with ripple_one for parity
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/batch_appendage_ripple.rs`
Same pattern as the earlier batch cores: `ripple_one` and `ripple_batch` both call
one `#[inline]` kernel (the exact r16 `appendage_ripple` math, copied since
cross-module imports are disallowed), so bit-for-bit parity is structural and
checked with `to_bits()`. The batch shares one frame `t` across a contiguous
`(vel, stiffness)` slice — the per-frame per-creature appendage set — so the win is
in iteration, not arithmetic.

### Thread 02 · RENDER

## outline_width: 1/(1+d/16) falloff with a 25%-of-base floor
**Status**: default
**Location**: crates/unendlich-engine/src/outline_width.rs
Thickness falls off as `base/(1 + d/16)` and is clamped to never go below 25% of
base, so distant selections stay visible without a hard cutoff. Both the falloff
rate and the floor fraction are placeholder feel values; the floor is applied
multiplicatively so the whole function stays linear in `base` (which the
"base scales" property needs).
---

## health_bar_uv: left-anchored fill, x=0/y=0/h=1, w=clamp(frac)
**Status**: default
**Location**: crates/unendlich-engine/src/health_bar_uv.rs
The bar fills left-to-right: the rect is `[0, 0, clamp(frac), 1]`. Width tracks the
clamped fraction directly so `width ∝ frac` is exact. A centre- or right-anchored
fill would need a non-zero x; left-anchored is the conventional default and keeps
the rect trivially in `[0,1]`.
---

## nameplate_fade: linear ramp, fade_end guarded to exceed fade_start
**Status**: default
**Location**: crates/unendlich-engine/src/nameplate_fade.rs
Alpha is 1 up to `fade_start`, linear to 0 by `fade_end`, 0 beyond. `fade_end` is
clamped to at least `fade_start + 1e-4` so a caller passing `end <= start` can't
divide by zero — it degenerates to a near-instant cutoff at `start` rather than
panicking. Linear (vs smoothstep) chosen for the obvious endpoints.
---

## spec_highlight: clamp(n·h,0,1)^power (Blinn-Phong)
**Status**: default
**Location**: crates/unendlich-engine/src/spec_highlight.rs
Straight Blinn-Phong: clamp the half-vector alignment to `[0,1]` then raise to
`power`. Clamping the base (not just the result) makes back-facing `n·h < 0`
yield 0 and keeps the output in `[0,1]`; `power` is clamped non-negative. No
alternative weighed — this is the reference term.
---

## batch_icon_atlas routes through icon_rect_one for bit-parity
**Status**: considered
**Location**: crates/unendlich-engine/src/batch_icon_atlas.rs
As with the prior batched cores, the batch calls the scalar reference per index so
the rects match bit-for-bit; the microopt is only the output `with_capacity` +
tight loop. Re-deriving the cell math in the loop risked float divergence, so the
per-element computation stays single-sourced. Mirrors R16 icon_atlas locally to
remain standalone-testable.
---

### Thread 03 · WORLD

## Cave room is a per-axis ellipsoid; zero radius degenerates that axis
**Status**: default
**Location**: crates/unendlich-engine/src/world/cave_room.rs
`Σ((p−center)/radii)² ≤ 1` lets chambers be tall, wide, or flat with one test. A
zero radius isn't a divide-by-zero — that axis instead requires an exact match
on the center, so a single zero collapses a plane and all-zero radii collapse to
the center point. Keeps the boundary inclusive (`≤ 1`) so it tiles cleanly with
`cave_carve` capsules.
---

## River mouth flares as base + extra/(1 + dist/scale)
**Status**: default
**Location**: crates/unendlich-engine/src/world/river_mouth.rs
Width is the base plus an extra that decays inversely with distance to the sea,
so it's widest at the mouth, strictly narrowing upstream, and never below base.
The flare ceiling scales with base width (plus a floor) so small streams still
get a visible delta. A reciprocal (not exponential) decay gives a long, gentle
delta tail.
---

## Moisture is strength × exponential distance falloff
**Status**: default
**Location**: crates/unendlich-engine/src/world/moisture_spread.rs
`strength · exp(−dist/FALLOFF)` keeps moisture in [0,1] (strength clamped),
maximal at the water's edge and fading smoothly inland. Multiplying by strength
(rather than adding) means a weak source can't reach as far, which is what makes
small ponds local and the sea broadly wet.
---

## Ore richness is a depth Gaussian × (1 − rarity)
**Status**: default
**Location**: crates/unendlich-engine/src/world/ore_vein_richness.rs
A Gaussian in depth gives each ore a rich "sweet-spot" band (rising to a peak,
tapering past it) rather than monotonic-with-depth, and rarity scales the whole
curve down so rarer ores read as leaner hits. Both factors are in [0,1] so the
product is too; `rarity = 0` reaches the full peak of 1.
---

## Clustering blends a uniform base toward a cluster field, preserving the mean
**Status**: considered
**Location**: crates/unendlich-engine/src/world/scatter_cluster.rs
Place probability is `lerp(base, 2·cluster_field·base, bias)`. Because the
cluster field averages ≈ ½, the `2·field·base` term averages ≈ base, so the mean
planted fraction stays ≈ base for any bias while the *variance* (clumping) rises
with bias. The alternative — multiplying the base by the field directly — also
clumps but drifts the overall fraction with bias; preserving the mean keeps
density tuning independent of clumpiness.
---

## Batched ridge sampling reuses the scalar for bit-parity
**Status**: default
**Location**: crates/unendlich-engine/src/world/batch_ridge_noise.rs
`ridge_batch` maps the exact `ridge_one` (a verbatim mirror of R16 `ridge`) over
its points, sharing the per-call octaves/seed. Results are bit-identical
(`to_bits()`) to per-point calls, including the octave clamp, so a batched
terrain-gen fast path and any scalar fallback produce the same world.
---

## ── Round-18 module decisions ──

### Thread 01 · SIM

## item_modifier scales affix count by rarity and draws each slot weighted
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/item_modifier.rs` (`roll_mods`)
Affix count is `floor(clamp01(rarity) · MAX_MODS)` and each slot is an independent
weighted draw keyed by `hash(seed, slot)`. Hashing per slot (not once per item)
decorrelates the slots and keeps the whole roll a pure function of `(rarity, pool,
seed)`, so a drop is identical on every peer yet seed-sensitive. Zero-weight pool
entries are skipped in the cumulative walk, so they're never rolled. Slots may
repeat the same affix; de-duping is left to downstream item assembly.

## stack_merge sums in u64 and saturates the overflow into u32
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/stack_merge.rs` (`merge`)
`combined = min(a+b, max)` and `overflow = (a+b) - combined`, computed in `u64`
so `a+b` can't wrap, with the overflow saturated back into `u32`. The saturating
return is the deliberate bit: it keeps the function total and panic-free even at
`u32::MAX + u32::MAX` into `max 0`, where the true overflow exceeds `u32`. `max=0`
falls out naturally (everything overflows). Conservation `combined+overflow == a+b`
holds at normal scales.

## xp_curve is A·level² + B·level in saturating u64
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/xp_curve.rs` (`xp_for_level`)
A quadratic was chosen over an exponential curve: it's superlinear (growing
per-level gaps, the second difference is `2A`) so leveling slows down, but it
grows polynomially rather than explosively, so it stays meaningful across the full
`u32` level range instead of saturating almost immediately. All three operations
are `saturating`, so `xp_for_level(u32::MAX)` returns `u64::MAX` rather than
overflowing. `A` and `B` are placeholder tuning; level 0 is the 0-XP baseline.

## status_effect applies before decaying and drops whole stacks per second
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/status_effect.rs` (`tick`)
The applied amount uses the *current* stacks (`max(0, magnitude·stacks)`) before
the stacks decay, so the tick a stack is lost still counts that tick — the natural
DoT/buff ordering. Decay drops whole stacks (`floor(DECAY_PER_SEC·dt)`,
`saturating_sub`) since stacks are an integer `u8`, so `dt=0` removes nothing and
large `dt` clears the effect. The applied value is floored at 0 so a negative
magnitude can't heal what it should harm.

## aggro_decay subtracts a linear time·distance term then clamps to [0, aggro]
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/aggro_decay.rs` (`decay`)
Decay is `aggro - BASE_RATE·dt·(1 + dist·DIST_WEIGHT)`, clamped to `[0, aggro]`.
The `(1 + dist·w)` factor is what makes a fleeing target lose the mob's interest
faster while a point-blank target decays at the base rate, and clamping at both
ends gives the `[0, aggro]` bound and the `dt=0`-unchanged behaviour for free.
Rates are placeholder tuning.

## batch_loot_roll shares one inline kernel with loot_one for parity
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/batch_loot_roll.rs`
Same pattern as the earlier batch cores: `loot_one` and `loot_batch` both call one
`#[inline]` `roll_table` kernel (the exact r16 `loot_roll` math, copied since
cross-module imports are disallowed), so each batch entry is identical to the
scalar roll for that seed (output is integer-only, so `==` is exact). The batch
borrows one table and rolls it across a contiguous seed slice — the "pack of the
same mob" shape — so the win is in iteration, not arithmetic.

### Thread 02 · RENDER

## lod_crossfade: smoothstep across the band, band_hi guarded > band_lo
**Status**: default
**Location**: crates/unendlich-engine/src/lod_crossfade.rs
The blend is a smoothstep (C1, eases at both ends) across `[band_lo, band_hi]`,
which reads better than a linear ramp for a dither/dissolve LOD swap. `band_hi` is
clamped to at least `band_lo + 1e-4` so a degenerate band can't divide by zero.
Smoothstep vs linear is the only real choice and it's a default look pick.
---

## billboard_face: yaw = atan2(z, x), +X→0 / +Z→+π/2
**Status**: considered
**Location**: crates/unendlich-engine/src/billboard_face.rs
Facing yaw is `atan2(to_cam.z, to_cam.x)`, fixing the convention +X→0, +Z→+π/2 and
the natural `(-π, π]` range. The alternative (`atan2(x, z)`, a +Z→0 convention)
is equally valid; I pinned the +X→0 convention to match the impostor_card yaw
handling. `atan2(0,0)=0` makes the zero vector safe with no special case. The
only discontinuity is the expected ±π branch cut.
---

## ao_corner: classic both-sides-occlude rule, four {0,1/3,2/3,1} levels
**Status**: default
**Location**: crates/unendlich-engine/src/ao_corner.rs
The standard voxel vertex-AO: if both edge neighbours are solid the vertex is
fully occluded (0), else `(3 - occluder_count)/3`. This is the well-known "0fps"
rule, not a weighed choice — reproduced exactly so meshing matches the canonical
look (and the four discrete levels the property checks).
---

## fog_blend: GL_EXP2 form 1 - 2^(-(density·dist)²)
**Status**: considered
**Location**: crates/unendlich-engine/src/fog_blend.rs
Uses the exp2 (GL_EXP2) squared-distance falloff rather than linear or plain
GL_EXP fog, because squared falloff keeps near geometry crisp while saturating
distant geometry — the look the shader roadmap calls for. Implemented with base-2
`exp2` as named; negative inputs clamp to 0 so it stays in `[0,1]` and monotone.
---

## batch_held_item_pose routes through held_pose_one for bit-parity
**Status**: considered
**Location**: crates/unendlich-engine/src/batch_held_item_pose.rs
Same pattern as the prior batched cores: the batch calls the scalar reference per
item so poses (including the out-of-range fallback) match bit-for-bit; the microopt
is purely `with_capacity` + a tight loop. The R16 pose table is mirrored locally to
stay standalone-testable rather than importing held_item_pose.
---

### Thread 03 · WORLD

## Biome is a 3×3 Whittaker grid with boundaries at the thirds
**Status**: default
**Location**: crates/unendlich-engine/src/world/biome_temp_map.rs
Temperature and moisture each split into three bands at 1/3 and 2/3, giving nine
ids via `temp_band*3 + moisture_band`. A discrete grid is the right shape because
the smoothing happens downstream in `biome_blend`/`biome_transition`; this just
names the cells. Boundaries at even thirds keep the bands equal-width and the
four corners distinct (ids 0/2/6/8).
---

## Cliff test is gradient magnitude vs threshold
**Status**: default
**Location**: crates/unendlich-engine/src/world/cliff_detect.rs
A cell is a cliff when `√(dh_dx² + dh_dz²)` exceeds the threshold — the true
steepest-ascent rate, so a diagonal slope correctly reads as steeper than either
axis alone (3,4 → 5). The threshold is `>` (not `≥`) and clamped non-negative,
so a perfectly flat cell with threshold 0 is not a cliff.
---

## Beach is a linear band above the water line only
**Status**: default
**Location**: crates/unendlich-engine/src/world/beach_band.rs
Sand fades linearly from 1 at the water line to 0 at `water + band`, and is 0
both below water (submerged) and above the band. Anchoring the full-strength end
at the water line (rather than centering the band on it) means beaches sit just
above the shore where sand actually accumulates, and the width is a single
tunable. `band ≤ 0` degenerates to an exact water-line match.
---

## Formation length = MAX · falloff(distance) · per-seed jitter
**Status**: default
**Location**: crates/unendlich-engine/src/world/cave_decoration.rs
Stalactite length keys off distance-to-ceiling and stalagmite off
distance-to-floor through `1/(1+d/scale)`, so each grows from its own surface and
a mid-air cell (far from both) grows almost nothing. A per-seed jitter in
[0.5,1) varies lengths without ever exceeding `MAX_LEN`, keeping output bounded.
---

## Batched strata reuses the scalar reference for bit-parity
**Status**: default
**Location**: crates/unendlich-engine/src/world/batch_strata.rs
`stratum_batch` maps the exact `stratum_one` (verbatim mirror of R16 `strata`)
over its samples, sharing the `bands` slice. Ids are identical to per-sample
calls (including the empty-bands and clamp cases), so a batched generation path
can't disagree with a scalar fallback on which rock a voxel is.
---

## ── Round-19 module decisions ──

### Thread 01 · SIM

## pack_hunt places members on an evenly-spaced ring, member 0 at +x
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/pack_hunt.rs` (`flank_pos`)
Each member sits at angle `2π·idx/pack_n` on a circle of `radius` around the prey.
Even angular spacing is the simplest arrangement that encircles the target with
distinct, deterministic slots and the right ring distance for any pack size, and
`pack_n=0` is guarded to 1. There's no prey-facing input, so "behind" is just the
fixed `+x` anchor (member 0); when a facing direction exists later, rotating all
angles by it is the one-line extension.

## territory is a clamped linear ramp from 1 at home to 0 at the radius
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/territory.rs` (`intrusion`)
Intrusion is `clamp01(1 - dist/radius)`: 1 at home, linearly down to 0 at the
edge, 0 beyond. The linear ramp gives the required at-home/outside/monotone/
radius-scaling behaviour in one expression, and a non-positive radius returns 0
(no territory) so it can't divide by zero. A smoother falloff could be swapped in
later, but nothing motivated it.

## sleep_cycle is a raised cosine, inverted for nocturnal species
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/sleep_cycle.rs` (`activity`)
Diurnal activity is `0.5 - 0.5·cos(2π·t)`, nocturnal is its `1 -` complement. A
cosine was chosen over a triangle/box so the cycle is smooth *and* seamless at the
day boundary (`activity(0) == activity(1)` in both value and slope), avoiding a
midnight pop — the continuity test checks across the wrap. `rem_euclid` makes it
periodic for any time input, and diurnal/nocturnal summing to 1 falls out of the
complement.

## hunger_drive is one clamped linear step on the single hunger axis
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/hunger_drive.rs` (`tick`)
`new = clamp01(hunger + HUNGER_RATE·dt − ate)`, with `dt` and `ate` floored at 0 so
time only adds and food only removes. This is deliberately the round-7 `survival`
hunger axis in isolation — a single scalar the mob-AI cores can drive without
pulling in thirst/warmth — and the clamp gives the bound and the dt0/ate0-unchanged
behaviour for free. Rate is placeholder tuning.

## mate_select scores 1 − mean|Δ| over the shared genome prefix
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/mate_select.rs` (`compat`)
Compatibility is `clamp01(1 - mean|a_i - b_i|)` over `min(len)` traits. Mean
absolute difference (rather than, say, `1/(1+sq_dist)`) was chosen because it maps
cleanly onto the required endpoints — identical → exactly 1, fully opposite (Δ=1
per trait) → exactly 0 — and is symmetric by construction. Comparing only the
shared prefix length-guards mismatched genomes; an empty overlap returns 0 (no
basis to mate). This models "like prefers like"; an inbreeding penalty (preferring
*some* difference) would be a different curve and is left as a future option.

## batch_threat_assess shares one inline kernel with threat_one for parity
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/batch_threat_assess.rs`
Same pattern as the earlier batch cores: `threat_one` and `threat_batch` both call
one `#[inline]` kernel (the exact r17 `threat_score` math), so bit-for-bit parity
is structural and checked with `to_bits()`. The batch returns the per-threat score
vector for a contiguous `(dist, power)` slice — the scoring an AI runs over every
nearby entity each tick — and taking the argmax over it reproduces r17's
`top_threat`. The win is in iteration, not arithmetic.

### Thread 02 · RENDER

## lod_select: count thresholds on a fidelity-scaled distance, clamp to last
**Status**: default
**Location**: crates/unendlich-engine/src/lod_select.rs
Mirrors the lod_band (R9) shaping: `eff = dist / fidelity`, then count how many
ascending bands `eff` clears, clamped to `bands.len()-1`. Dividing by fidelity
(rather than multiplying thresholds) keeps the "higher fidelity → lower index"
property exact for any band set. Empty bands → index 0. Default.
---

## contact_shadow: linear darkening 1→0 over max_h, guarded
**Status**: default
**Location**: crates/unendlich-engine/src/contact_shadow.rs
Linear falloff from 1 at ground contact to 0 at `max_h`; `max_h` clamped to a tiny
positive so a zero/negative falloff can't divide by zero (degenerates to "dark
only exactly at the ground"). A squared falloff would look softer but linear makes
the monotone+endpoint properties obvious. Default.
---

## rim_light: Fresnel (1 - n·v)^power
**Status**: default
**Location**: crates/unendlich-engine/src/rim_light.rs
Standard Schlick-style rim: clamp `n·v` to `[0,1]`, take `(1 - n·v)^power`. Bright
at grazing angles, dark facing — the canonical silhouette glow. `power` clamped
non-negative; output in `[0,1]`. No alternative weighed.
---

## uv_scroll: wrap via v - floor(v) into [0,1)
**Status**: considered
**Location**: crates/unendlich-engine/src/uv_scroll.rs
Wrapping uses `v - v.floor()`, which maps any finite value (including negatives,
e.g. backward scroll) into `[0,1)` in one branch-free step, with a guard turning a
floating `1.0` into `0.0`. Considered `rem_euclid(1.0)` and chose `v - floor(v)`
because it's the same result with one fewer special case and matches how GPU
samplers wrap. Identity at `t=0` for already-normalized UVs.
---

## batch_spec_highlight routes through spec_one for bit-parity
**Status**: considered
**Location**: crates/unendlich-engine/src/batch_spec_highlight.rs
Same pattern as the prior batched cores: the batch calls the scalar reference per
item so `clamp.powf` results match bit-for-bit; the microopt is purely
`with_capacity` + a tight loop, never re-deriving the math. Mirrors R17
spec_highlight locally to stay standalone-testable.
---

### Thread 03 · WORLD

## Flow routes to the lowest strictly-lower neighbor; ties to lowest index
**Status**: default
**Location**: crates/unendlich-engine/src/world/flow_route.rs
The cell sends water to its single lowest neighbor that is strictly below it; if
none is lower it's a sink (`4`). Using strict `<` while scanning N,E,S,W means
an equal-height neighbor never displaces an earlier pick, so ties resolve to the
lowest index deterministically — the property `flow_accumulation` relies on for
a stable drainage graph.
---

## Lake level fills from the floor and clamps at the spill
**Status**: default
**Location**: crates/unendlich-engine/src/world/lake_fill.rs
Level is `floor + inflow` clamped to the spill height, so a dry basin sits at the
floor, more inflow raises the surface, and it can never rise above where it would
overflow. `spill` is clamped to be ≥ floor so a malformed basin can't invert. The
linear fill rate is a placeholder; the cap is the load-bearing behavior.
---

## Snow uses a temperature-lifted line with a soft altitude band
**Status**: default
**Location**: crates/unendlich-engine/src/world/snow_line.rs
The effective snow line is the base line plus `temp · TEMP_LIFT`, so warmth
pushes snow strictly higher; coverage then ramps linearly from 0 to 1 across a
`BLEND`-wide band centered on it, giving a soft transition instead of a hard cap
line. Both factors keep the result in [0,1].
---

## Deposit: shed excess over capacity, else settle a slope-based fraction
**Status**: default
**Location**: crates/unendlich-engine/src/world/sediment_deposit.rs
Two regimes, both mass-conserving: above capacity the excess drops immediately
and the capacity carries on; within capacity a fraction settles, larger on gentle
slopes (`SETTLE_MAX/(1+slope·k)`) because slow water drops its load. Returning
`(deposited, remaining)` with `deposited + remaining == carried` lets the eroder
move the remainder downstream without bookkeeping drift.
---

## Batched warp reuses the scalar reference for bit-parity
**Status**: default
**Location**: crates/unendlich-engine/src/world/batch_terrain_warp.rs
`warp_batch` maps the exact `warp_one` (verbatim mirror of R16 `terrain_warp`)
over its points, sharing `amp`/`seed`. Warped coordinates are bit-identical
(`to_bits()`) to per-point calls, including the `amp == 0` identity, so a batched
terrain-gen path produces the same world as a scalar fallback.
---

## ── Round-20 module decisions ──

### Thread 01 · SIM

## horn_type derives tier from gene but gates the exotic tier behind a rare roll
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/horn_type.rs` (`horn_type`, `band`)
Tier comes from the gene (`floor(gene·4)`), but the exotic tier (3) is downgraded
to rare (2) unless `rarity_roll ≥ EXOTIC_RARITY` — so exotic horns can't be reached
by gene alone, only by a lucky drop, which is the design intent. Pairs are banded
per tier and scaled by the gene within the band; the bands were chosen so they
overlap upward (each tier's range starts inside the previous), which keeps pair
count monotone-ish in gene across tier boundaries rather than jumping down. Bands
and the exotic threshold are placeholder tuning.

## horn_charge clamps accrual to [0,capacity] and discharges only at full
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/horn_charge.rs` (`charge`, `try_discharge`)
`charge` is `clamp(cur + rate·dt, 0, capacity)` (capacity floored at 0), so it
fills or drains toward the cap and `dt=0`/`capacity=0` fall out for free.
`try_discharge` returns `Some` only when `capacity > 0 && cur >= capacity`,
returning the released energy for the caller to zero out — splitting "fire?" from
"reset" keeps this function pure and lets the caller decide the post-discharge
state. Rates/capacity are owned by the archetype, not this core.

## obsession picks a dominant drive by gene band, then normalizes with context
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/obsession.rs` (`drives`)
The gene's band gets a large additive bump over a small base on all five drives;
context adds (floored at 0) per drive; then the whole vector is normalized to sum
1. Additive-then-normalize was chosen over, say, a softmax because it makes the
required properties exact and cheap: weights are trivially non-negative and sum to
1, the band is dominant with empty context, and a strong context entry can flip the
dominant drive while the result stays a valid distribution. Base/bump are
placeholder tuning.

## scent_track follows the strongest neighbour only if it climbs the gradient
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/scent_track.rs` (`step_toward`)
It returns the strongest neighbour's index, but only when that neighbour exceeds
both the current cell and a noise floor; otherwise `4` (lost). Gating on
`> scent_here` (gradient ascent) rather than just `> EPS` is the deliberate choice:
it uses the current-cell reading to avoid stepping downhill and treats a local peak
or a fully-decayed field as "lost", which is the correct behaviour for following a
*decaying* trail. Ties break to the lowest index for stability.

## batch_status_effect shares one inline kernel with status_one for parity
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/batch_status_effect.rs`
Same pattern as the earlier batch cores: `status_one` and `status_batch` both call
one `#[inline]` kernel (the exact r18 `status_effect::tick` math), so bit-for-bit
parity is structural and checked with `to_bits()` on the float field. The batch
ticks a contiguous `(stacks, magnitude)` slice at one shared `dt` — an entity's
active-effect set per frame — so the win is in iteration, not arithmetic.

### Thread 02 · RENDER

## horn_fractal: build the left horn, mirror it across X; binary fork, capped at 4096
**Status**: considered
**Location**: crates/unendlich-engine/src/horn_fractal.rs
Each pair's right horn is produced by negating the left horn's X coordinates
rather than re-running the recursion with flipped sweep signs. This guarantees
*exact* mirror symmetry regardless of sweep (the alternative — sweeping both sides
independently — drifts out of symmetry because a Z-rotation isn't symmetric under
X-mirror). Each branch forks into two swept children to `depth`; total is capped
at 4096 so a deep recursion can't explode. The 0.3 pair spacing and base
direction are placeholder values.
---

## ribbon_trail: 2 verts/point, perpendicular tangent×up, +Z fallback, width0=centerline
**Status**: default
**Location**: crates/unendlich-engine/src/ribbon_trail.rs
Same perpendicular construction as ribbon_mesh (R9) minus the taper: offset each
point by `±width/2` along `normalize(tangent × up)`, falling back to +Z when the
trail is vertical. `width == 0` collapses both verts onto the point (a degenerate
centerline) rather than special-casing. Default.
---

## particle_fade: linear alpha 1→0, size puff 4·t·(1-t)
**Status**: default
**Location**: crates/unendlich-engine/src/particle_fade.rs
Alpha fades linearly; size uses the parabola `4·t·(1-t)` so the puff grows in and
shrinks out, peaking at mid-life, bounded to `[0,1]`. `life <= 0` returns `(0,0)`
to avoid dividing by zero. The grow-then-shrink size curve is the one mild choice;
both are otherwise defaults.
---

## decal_uv: local/size + 0.5 clamped, plus a covers() flag
**Status**: default
**Location**: crates/unendlich-engine/src/decal_uv.rs
UV is `local/size + 0.5` clamped to `[0,1]`, so centre→0.5 and the `±size/2`
edges→0/1. Rather than overload the return type to "flag outside", I added a
separate `covers()` predicate so the caller can discard out-of-footprint fragments
instead of smearing the clamped border. `size` guarded positive. Default.
---

## batch_fog_blend routes through fog_one for bit-parity
**Status**: considered
**Location**: crates/unendlich-engine/src/batch_fog_blend.rs
Same pattern as the prior batched cores: the batch calls the scalar reference per
item so the `exp2` fog factors match bit-for-bit; the microopt is purely
`with_capacity` + a tight loop. Mirrors R18 fog_blend's GL_EXP2 form locally to
stay standalone-testable.
---

### Thread 03 · WORLD

## Foam is a linear band keyed on depth below the water line
**Status**: default
**Location**: crates/unendlich-engine/src/world/foam_edge.rs
Foam peaks at the shore (depth 0) and fades to 0 by `foam_width` deep; negative
depth (terrain above water) is 0. Keying on depth-below-water rather than
horizontal distance means the band automatically hugs the actual waterline
around any coastline shape. `foam_width ≤ 0` degenerates to an exact-edge match.
---

## Ripple is a Gaussian crest at speed·t with time-decaying amplitude
**Status**: default
**Location**: crates/unendlich-engine/src/world/ripple_ring.rs
The crest radius advances at `speed·t`; displacement is a Gaussian in
distance-from-crest scaled by `1/(1+t/DECAY)`, so the ring expands, thins, and
fades, and water far ahead of the crest is ~0. A single closed form (no
per-ripple state) lets the renderer sample any `(dist, t)` directly. Output is
clamped to [0,1] as a height-envelope, not an oscillating wave.
---

## Moss = moisture · (1 − light), jittered per seed
**Status**: default
**Location**: crates/unendlich-engine/src/world/cave_moss.rs
A product makes dryness a hard veto (moisture 0 → no moss) and light a
suppressor (bright → less), matching moss preferring damp shade; the seed jitter
in [0.7,1) gives patchy coverage without ever exceeding 1. Multiplicative (not
additive) so a single missing factor zeroes coverage rather than averaging it
in.
---

## Settle relaxes slopes above the repose angle, half the excess per step
**Status**: default
**Location**: crates/unendlich-engine/src/world/erosion_settle.rs
Identical mass-conserving, double-buffered relaxation as the thermal pass, but
framed for loose material: drops above `repose` shed `0.5·(steepest − repose)`
to steep neighbors, so a pile slumps toward a stable cone over repeated steps
while slopes at or under repose stay put. Single-cell and ragged inputs are
no-ops.
---

## Batched cliff detection reuses the scalar reference for parity
**Status**: default
**Location**: crates/unendlich-engine/src/world/batch_cliff_detect.rs
`cliff_batch` maps the exact `cliff_one` (verbatim mirror of R18 `cliff_detect`)
over its gradients, sharing the threshold. Boolean results are identical to
per-cell calls, so a batched material-classification path and a scalar fallback
agree on every cliff cell.
---

## ── Round-21 module decisions ──

### Thread 01 · SIM

## ability_cast maps element→kind and scales power/radius with charge, capped
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/ability_cast.rs` (`cast`)
A discharge of `charge <= 0` returns the reserved `KIND_NONE` effect; otherwise the
kind is `element + 1` (so the six known elements map to kinds 1..=6) with an
out-of-range element falling back to the basic kind rather than panicking, and
power/radius scale linearly with charge up to caps. Reserving kind 0 for "no
effect" keeps the inert case unambiguous, and the caps stop an overcharged horn
from producing an unbounded blast. Scales/caps are placeholder tuning; this pairs
with `horn_charge` (r20), which produces the `charge` released here.

## gem_socket copies the base block and adds each gem, ignoring bad indices
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/gem_socket.rs` (`apply_gems`)
The result is a clone of `base` with each gem's amount added at its stat index;
gems whose index is out of range are skipped. Additive stacking with a bounds
check (rather than resizing or erroring) is the natural fit: it keeps the output
length equal to the item's stat block, lets multiple gems on one stat sum, and
makes a malformed gem a harmless no-op — important for a function fed by
player-built loadouts.

## set_bonus returns the greatest met bonus, floored at 0, for monotonicity
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/set_bonus.rs` (`bonus`)
Among thresholds whose required count is met, the function returns the largest
bonus (floored at 0). Taking the max of met bonuses — rather than the
highest-*required* threshold — was the deliberate choice: it makes the result
provably monotone non-decreasing in `set_count` for any (non-negative) threshold
list, regardless of order, while still coinciding with "highest tier wins" for the
normal case of increasing tables. Empty or all-unmet thresholds give 0.

## migrate is a cosine along the warmth axis, scaled by warm_pref
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/migrate.rs` (`migrate_dir`)
Direction is `[0, -warm_pref·cos(2π·season01)]`: toward warmth (`-y`) in winter,
reversing to `+y` in summer, with the equinoxes passing through zero. A cosine was
chosen so the seasonal urge is smooth and periodic (no discontinuity at the year
boundary) and naturally eases through the transition seasons, and scaling by
`warm_pref` (clamped `0..1`) makes an indifferent creature stay put and bounds the
magnitude by 1 ("unit-ish"). Migration runs purely along the warmth axis, so `x`
is 0; a future climate map could rotate this.

## batch_aggro_decay shares one inline kernel with aggro_one for parity
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/batch_aggro_decay.rs`
Same pattern as the earlier batch cores: `aggro_one` and `aggro_batch` both call
one `#[inline]` kernel (the exact r18 `aggro_decay::decay` math), so bit-for-bit
parity is structural and checked with `to_bits()`. The batch decays a contiguous
`(aggro, dist)` slice at one shared frame `dt` — the angry-mob set per frame — so
the win is in iteration, not arithmetic.

### Thread 02 · RENDER

## heat_haze: inverse-square falloff × two-frequency shimmer, clamped
**Status**: default
**Location**: crates/unendlich-engine/src/heat_haze.rs
Strength is `intensity × 1/(1+(d/8)²) × shimmer`, where the shimmer is two summed
sines (7 and 13 Hz-ish) remapped to `[0,1]`. Inverse-square falloff and a
two-frequency flicker are the cheap defaults that give a believable rising-heat
look; the constants are placeholders. Output clamped to `[0,1]`.
---

## sss_approx: back-lit term × exp(-thickness)
**Status**: considered
**Location**: crates/unendlich-engine/src/sss_approx.rs
Translucency is `max(-n·l, 0) × exp(-thickness)`: only back-lighting transmits,
and Beer-Lambert `exp(-thickness)` attenuates with depth. Chose the exponential
over a linear `1/(1+thickness)` falloff because it's the physically-flavoured form
and makes "monotone in thinness" exact, while staying one cheap `exp`. Pairs with
wing_opacity's leather/glass blend.
---

## spark_burst: uniform-sphere direction from hash, X/Z pure radial, Y under gravity
**Status**: default
**Location**: crates/unendlich-engine/src/spark_burst.rs
Each spark's direction is a uniform-sphere sample from a splitmix hash of
`(idx, seed)`; horizontal travel is pure radial (`∝ t`) so the "spreads radially"
property is exact, while Y also falls under a simple `½gt²` gravity. Alpha fades
linearly over a fixed 1.5s life. Hashing per-index keeps it deterministic with no
stored state. Constants are default tuning.
---

## edge_highlight: max neighbour depth-gap / threshold, ramped and abs (symmetric)
**Status**: default
**Location**: crates/unendlich-engine/src/edge_highlight.rs
Edge strength is the largest absolute centre-vs-neighbour depth gap divided by the
threshold, clamped to `[0,1]`. Using the absolute gap makes it symmetric in sign
(a nearer or farther neighbour reads the same), and ramping (rather than a hard
step) gives an anti-aliased outline. Default.
---

## batch_lod_crossfade routes through fade_one for bit-parity
**Status**: considered
**Location**: crates/unendlich-engine/src/batch_lod_crossfade.rs
Same pattern as the prior batched cores: the batch calls the scalar reference per
distance so the smoothstep weights match bit-for-bit; the microopt is purely
`with_capacity` + a tight loop over a shared band. Mirrors R18 lod_crossfade
locally to stay standalone-testable.
---

### Thread 03 · WORLD

## Waterfall intensity is the product of saturating flow and drop terms
**Status**: default
**Location**: crates/unendlich-engine/src/world/waterfall.rs
Each input passes through `x/(x+k)` (rises to 1, never exceeds it) and the two
are multiplied, so a waterfall needs both water and a drop — either at zero kills
it — and neither can blow past 1. The product (not a sum) is what makes both
factors mandatory rather than merely additive.
---

## Cave pool capacity is min(dip, rim); fills with inflow
**Status**: default
**Location**: crates/unendlich-engine/src/world/cave_pool.rs
A pool can hold at most the shallower of the depression depth and the rim
height, and fills from dry as inflow accumulates up to that cap. Clamping to
`min(dip, rim)` means neither a deep dip with a low rim nor a high rim over a
shallow dip can overfill — the binding constraint wins.
---

## Frost = (1 − temp) · humidity
**Status**: default
**Location**: crates/unendlich-engine/src/world/frost_spread.rs
A product makes warmth a hard veto (temp 1 → no frost) and dryness a suppressor,
matching frost forming only on cold, humid surfaces. Multiplicative so a single
missing condition zeroes coverage; both factors clamped to [0,1] keep the result
in range.
---

## Scree ramps as (slope − repose)/repose, clamped
**Status**: considered
**Location**: crates/unendlich-engine/src/world/gravel_slide.rs
Normalizing the over-repose excess by the repose angle (rather than a fixed
width) makes the scree onset scale with the material: a higher repose tolerates a
proportionally steeper slope before scree appears, reaching full cover near twice
repose. The alternative — a fixed-width ramp — wouldn't couple onset to material
stability, so the same slope would scree identically for loose sand and packed
gravel.
---

## Batched biome map reuses the scalar reference for id-parity
**Status**: default
**Location**: crates/unendlich-engine/src/world/batch_biome_temp_map.rs
`biome_batch` maps the exact `biome_one` (verbatim mirror of R18 `biome_temp_map`)
over its samples. Ids are identical to per-sample calls (including the clamp), so
a batched generation path classifies every column the same as a scalar fallback.
---

## ── Round-22 module decisions (GPU_COMPUTE_PLAN refs) ──

### Thread 01 · SIM

## popdyn_step uses the logistic map with r = birth - death
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/popdyn_step.rs` (`step`)
`new = pop + (birth - death)·pop·(1 - pop/carrying)`, floored at 0. Folding birth
and death into a single net rate `r` and using the textbook logistic term is what
makes the three required behaviours exact: the `(1 - pop/carrying)` factor is
positive below carrying, negative above, and exactly zero at carrying (so carrying
is a true fixed point). A non-positive carrying returns 0 (no environment, no
division by zero). Chosen over carrying-dependent birth minus flat death, which
would not equilibrate exactly at carrying.

## pollen_spread is a double-buffered upwind-biased gather (convex blend)
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/pollen_spread.rs` (`spread_step`)
Each cell becomes `(1-rate)·self + rate·(wind-weighted neighbour average)`, written
into a second buffer so all reads see the old field. The gather/convex-blend form
was chosen over a flux/Laplacian update because it is unconditionally stable and
bounded — the result is always within the field's existing range, so no negative
pollen ever appears for any `rate ∈ [0,1]` — and it conserves mass exactly in the
still-air interior (the no-wind neighbour matrix is doubly stochastic). Wind biases
the gather toward the upwind side, which drifts the pattern downwind; this makes
mass only approximately conserved under wind (acceptable, and matches "~conserved").

## plant_gene_express shares one inline kernel for batch parity, zero-filling
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/plant_gene_express.rs`
`express_one` and `express_batch` both call one `#[inline]` `kernel`, so the batch
is bit-for-bit identical to per-part expression (checked with `to_bits()` on the
float traits). Genome bytes are read through a helper that returns 0 past the end,
so any genome length is safe — important because sub-part genomes vary in length.
The batch decodes a contiguous slice of genomes independently, which is exactly the
one-thread-per-sub-part shape the GPU kernel will dispatch.

## ability_aura normalizes distance then applies a selectable falloff curve
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/ability_aura.rs` (`aura`)
Intensity is `curve(1 - clamp01(dist/radius))` with the curve picked by `falloff`
(linear, quadratic, smoothstep, hard cutoff). Normalizing distance by radius first
makes "1 at centre, 0 at/after radius" hold for every mode and every radius in one
place, and each curve is monotone non-increasing so the aura never strengthens with
distance. A non-positive radius degrades to a point source; unknown selectors fall
back to linear. Pairs with `ability_cast` (r21), which supplies the radius.

## batch_horn_charge shares one inline kernel with charge_one for parity
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/batch_horn_charge.rs`
Same pattern as the earlier batch cores: `charge_one` and `charge_batch` both call
one `#[inline]` kernel (the exact r20 `horn_charge::charge` accrual math), so
bit-for-bit parity is structural and checked with `to_bits()`. The batch advances a
contiguous `(cur, rate, capacity)` slice at one shared frame `dt` — a herd's horns
per frame — so the win is in iteration, not arithmetic.

### Thread 02 · RENDER

## region_cull: p-vertex AABB-vs-frustum, inward-normal planes, octant child mask
**Status**: considered
**Location**: crates/unendlich-engine/src/region_cull.rs
Visibility uses the conservative p-vertex test (the single corner farthest along
each inward plane normal) — one dot product per plane, no per-corner loop — and a
box is culled only when even that corner is outside, so straddling regions stay
visible (correct for a coarse hierarchy that descends further). The plane
convention is inward-normal, inside ⇔ `dot+ d >= 0`. `child_mask` splits at the
midpoint and tests all 8 octants for the next hierarchy level. Considered the
n-vertex (exact, both corners) test and chose p-vertex only — at the coarse level
false-positives are fine and cheaper.
---

## lod_morph: smoothstep over [lo,hi], hi guarded > lo
**Status**: default
**Location**: crates/unendlich-engine/src/lod_morph.rs
Geomorph weight is a smoothstep (same shape as lod_crossfade R18) so the
vertex-position blend eases in/out without a velocity discontinuity at the band
edges. `hi` clamped to at least `lo + 1e-4` to avoid a zero-width divide. Default.
---

## light_falloff: inverse-square core × (1-(d/r)^4)^2 window to 0 at radius
**Status**: considered
**Location**: crates/unendlich-engine/src/light_falloff.rs
Attenuation multiplies an inverse-square-ish core `1/(1+d²)` by a smooth windowing
term that reaches exactly 0 at `radius` (the UE4-style windowed falloff), giving
bounded light influence with no hard clip seam. Considered pure `1/(1+d²)` (never
reaches 0, so lights have unbounded tails) and rejected it for the bounded-radius
property. `radius` guarded `> 0`.
---

## batch_rim_light routes through rim_one for bit-parity
**Status**: considered
**Location**: crates/unendlich-engine/src/batch_rim_light.rs
Same pattern as the prior batched cores: the batch calls the scalar reference per
item so the `(1-n·v)^power` rim values match bit-for-bit; the microopt is purely
`with_capacity` + a tight loop. Mirrors R19 rim_light locally to stay
standalone-testable.
---

### Thread 03 · WORLD

## Prefetch reach scales with speed; stationary falls back to a ring
**Status**: default
**Location**: crates/unendlich-engine/src/world/prefetch_predict.rs
The path is sampled at `n` evenly-spaced points out to `lookahead·speed`, so a
faster player prefetches proportionally farther ahead (hiding more gen latency)
while the points stay ordered closest-first. Below a small speed threshold there
is no meaningful direction, so it returns expanding Chebyshev shells around the
player instead — the chunks most likely to be needed when standing still.
---

## Schedule is a stable priority sort truncated to budget
**Status**: default
**Location**: crates/unendlich-engine/src/world/chunk_schedule.rs
Candidates are stably sorted by descending priority and cut to `budget`, so ties
keep the caller's input order (e.g. a distance pre-sort) and the result is
deterministic. Budget is treated as a chunk count — the simplest budget unit;
`gen_estimate` exists for callers that want a cost-weighted budget instead.
---

## Field step is symmetric per-edge flux (exact conservation)
**Status**: considered
**Location**: crates/unendlich-engine/src/world/field_step.rs
Diffusion is done as per-edge flux (each +x/+y edge moves the same amount out of
one cell and into its neighbor) rather than the common "relax toward neighbor
average" form. Per-edge flux conserves total field exactly even at boundaries,
which is the invariant a GPU compute kernel must mirror bit-for-bit; the
average-form alternative leaks mass at edges and would desync CPU vs GPU.
---

## Gen cost = floor + complexity·distance-LOD falloff
**Status**: default
**Location**: crates/unendlich-engine/src/world/gen_estimate.rs
A fixed floor covers per-chunk overhead; the detail term is `complexity ×
COMPLEXITY_MAX × 1/(1+dist/scale)`, so cost is highest for near complex chunks
and decays monotonically with distance (far chunks generate coarser). Zero
complexity always costs exactly the floor, giving the scheduler a clean lower
bound.
---

## Batched flow routing reuses the scalar reference for parity
**Status**: default
**Location**: crates/unendlich-engine/src/world/batch_flow_route.rs
`flow_batch` maps the exact `flow_one` (verbatim mirror of R19 `flow_route`) over
its cells. Directions are identical to per-cell calls (including the tie→lowest-
index and sink rules), so a batched drainage-graph build can't reroute water
differently from a scalar fallback.
---

## ── Round-R23 module decisions ──

### Thread 01 · SIM

## crit_roll compares roll<chance and floors the multiplier at 1
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/crit_roll.rs` (`crit`)
Returns the multiplier when `roll < chance`, else `1.0`, with `chance >= 1`
short-circuiting to always-crit and `chance <= 0` to never-crit so those edges
hold regardless of the roll value. The multiplier is floored at 1 so a crit can
never reduce damage — a crit is upside-only — which also bounds the result to
`[1, mult]`. The caller supplies the roll, keeping this a pure decision function
with no embedded RNG.

## status_stack saturates the add before capping
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/status_stack.rs` (`add_stack`)
`min(cur.saturating_add(add), cap)`. The saturating add is the deliberate bit: it
keeps the function total on `u8` (e.g. `250 + 250`) before the cap clamps it, so
no overflow is possible for any inputs. `cap = 0` and already-over-cap inputs both
resolve to the cap. Feeds the status-effect tick cores (r18/r20), which consume
the resulting stack count.

## threat_share weights by 1/(1+dist) and normalizes to the total
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/threat_share.rs` (`share`)
Each member's weight is `1/(1+dist)` and shares are `total · w_i / Σw`. The
`1/(1+dist)` form was chosen over `1/dist` precisely so a member at distance 0
gets a large but finite weight (no division by zero, no infinity), while still
giving nearer members strictly more. Normalizing guarantees the shares sum back to
`total` and equal distances split evenly. Empty packs return empty.

## spawn_budget takes the min of pending, budget, and saturating cap-room
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/spawn_budget.rs` (`allowed`)
`min(pending, budget, cap.saturating_sub(active))`. Folding the three independent
limits into a single min is the whole job; the only subtlety is computing room as
a *saturating* subtract so an over-cap world (`active > cap`) yields 0 room rather
than wrapping to a huge `u32`. This makes "respects budget", "respects cap", and
"zero when full" all fall out of one expression.

## batch_ability_cast shares one inline kernel with cast_one for parity
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/batch_ability_cast.rs`
Same pattern as the earlier batch cores: `cast_one` and `cast_batch` both call one
`#[inline]` kernel (the exact r21 `ability_cast::cast` math and constants, copied
since cross-module imports are disallowed), so bit-for-bit parity is structural and
checked with `to_bits()` on the float fields plus `==` on the kind. The batch
resolves a contiguous `(element, charge)` slice — a volley or a tick's worth of
casts — so the win is in iteration, not arithmetic.

### Thread 02 · RENDER

## depth_fade: linear gap/fade ramp, larger-z = farther convention
**Status**: default
**Location**: crates/unendlich-engine/src/depth_fade.rs
Opacity ramps linearly from 0 (particle touching the scene surface) to 1 once the
depth gap reaches `fade`, using the convention that larger depth is farther so
`scene_z - particle_z > 0` means the particle is in front. `fade` guarded `> 0`.
Linear (vs a smoothstep) keeps the touching⇒0 / far⇒1 endpoints obvious. Default.
---

## motion_blur_vec: (cur-prev)*scale clamped to ±1 per component
**Status**: default
**Location**: crates/unendlich-engine/src/motion_blur_vec.rs
Velocity is the scaled clip-space delta, clamped per component to `±MAX_VEL (1.0)`
so one fast object can't smear across the frame. Per-component clamp (vs clamping
the vector magnitude) is the cheaper default and is what the bounded-clamp +
symmetry properties expect. Default.
---

## tonemap_curve: Narkowicz ACES filmic approximation
**Status**: considered
**Location**: crates/unendlich-engine/src/tonemap_curve.rs
Uses the Narkowicz ACES rational `((v(av+b))/(v(cv+d)+e))` with an exposure
multiplier, chosen over plain Reinhard `x/(1+x)` because it has the filmic
toe/shoulder that matches the tonemap.glsl shader path while still being monotone
on `[0,∞)` and saturating to ~1. Clamped to `[0,1]`. Considered Reinhard and
rejected it for the flatter look.
---

## batch_light_falloff routes through atten_one for bit-parity
**Status**: considered
**Location**: crates/unendlich-engine/src/batch_light_falloff.rs
Same pattern as the prior batched cores: the batch calls the scalar reference per
distance so attenuation matches bit-for-bit; the microopt is purely
`with_capacity` + a tight loop over a shared radius. Mirrors R22 light_falloff's
windowed inverse-square locally to stay standalone-testable.
---

### Thread 03 · WORLD

## Sediment advects to its single flow-direction target; sinks/edges retain
**Status**: default
**Location**: crates/unendlich-engine/src/world/erosion_advect.rs
Each cell pushes a `rate` fraction of its sediment to the one cell its flow
direction points at (paired delta out/in), so transport follows the precomputed
drainage graph from `flow_route`. Sink cells and off-grid targets keep their
load rather than dropping it, which conserves total sediment exactly — the eroder
can run many steps without bookkeeping drift.
---

## Cave breeze is pressure/area (Venturi), capped
**Status**: default
**Location**: crates/unendlich-engine/src/world/cave_breeze.rs
Speed is `k · pressure / cross_section`, so a pinch point blows faster for the
same pressure (continuity) and zero pressure gives zero wind. Cross-section is
floored to avoid a divide-by-zero at a fully closed passage and the result is
capped, keeping the breeze bounded for the particle/wind systems that consume it.
---

## Water table is bedrock + clamped net recharge
**Status**: default
**Location**: crates/unendlich-engine/src/world/aquifer.rs
The table is the bedrock floor plus `max(0, rain − drainage)` scaled and capped,
so it rises with rain, falls with drainage, never sinks below bedrock, and
saturates at `bedrock + MAX_RISE`. Clamping the net recharge at zero (rather than
letting drainage push below the floor) is what guarantees the `≥ bedrock`
property.
---

## Batched gen estimate reuses the scalar reference for bit-parity
**Status**: default
**Location**: crates/unendlich-engine/src/world/batch_gen_estimate.rs
`gen_batch` maps the exact `gen_one` (verbatim mirror of R22 `gen_estimate`) over
its chunks. Costs are identical to per-chunk calls (including the clamp and
floor), so a batched budgeting pass admits exactly the same chunk set as a scalar
fallback within a given budget.
---


## ── Round-R24 module decisions ──

### Thread 01 · SIM

## enchant_apply adds level·per_level then clamps to [0, cap]
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/enchant_apply.rs` (`apply`)
`clamp(base + level·per_level, 0, max(0,cap))`. Treating `cap` as the stat
*ceiling* (clamping the total, not just the bonus) gives the cleanest "capped"
semantics and a single bound on the result; `level=0` and `per_level=0` both leave
the base untouched by the arithmetic, and the `0` floor keeps a stat from going
negative under an odd per-level. Cap floored at 0 so a negative cap can't invert
the clamp range. Per-level/cap are placeholder tuning.

## school_fish blends cohesion-to-centroid with the current heading
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/school_fish.rs` (`steer`)
With neighbours present the steer is `normalize(cohesion_dir + heading_dir)`;
empty neighbours return the velocity unchanged. Because `nbrs` carries only
positions (no neighbour velocities), classic velocity-alignment isn't available,
so "alignment" is modelled as retaining the fish's own heading — which also makes
a symmetric school (centroid at the fish) leave the heading unchanged, the
symmetry the brief asks for. Using the mean position for cohesion makes the result
order-independent. Water confinement has no input here and is left to the caller.

## scent_decay is strength·2^(-age/half_life)
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/scent_decay.rs` (`remaining`)
Exponential half-life decay, expressed as `2^(-age/half_life)` via `exp2`. The
half-life form is the natural, physically-anchored model of a fading trail and
makes the "half at one half-life, quarter at two" tests exact; it's monotone
non-increasing and bounded in `[0, strength]` for non-negative inputs. A
non-positive half-life decays instantly (full only at age 0) and negative age is
treated as fresh, so there's no division by zero. Follow-on to `scent_track`
(r20).

## ability_cooldown is clamp(cooldown - (now - last_cast), 0, cooldown)
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/ability_cooldown.rs` (`cd_remaining`)
The remaining time is the cooldown minus elapsed, clamped to `[0, cooldown]`.
Clamping at both ends is the deliberate bit: the lower clamp gives the 0-means-
ready signal once enough time passes, and the upper clamp means clock skew (a
`now` before `last_cast`) reports at most a full cooldown rather than an inflated
timer. A cooldown of 0 is always ready. The caller passes `now`, keeping it pure.

## batch_crit_roll shares one inline kernel with crit_one for parity
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/batch_crit_roll.rs`
Same pattern as the earlier batch cores: `crit_one` and `crit_batch` both call one
`#[inline]` kernel (the exact r23 `crit_roll::crit` logic), so bit-for-bit parity
is structural and checked with `to_bits()`. The batch resolves a contiguous
`(roll, chance, mult)` slice — a tick's worth of hits — so the win is in iteration,
not arithmetic; rolls are supplied by the caller, keeping it deterministic.

### Thread 02 · RENDER

## dof_blur: linear CoC = |depth-focus|/range, symmetric
**Status**: default
**Location**: crates/unendlich-engine/src/dof_blur.rs
Circle-of-confusion is the absolute depth offset from focus normalized by `range`
and clamped to `[0,1]`. Using `|depth-focus|` makes it symmetric (near and far
blur equally for the same offset) — a simplification vs a physically asymmetric
near/far CoC, chosen because the property set asks for symmetry and the look is a
post effect, not a lens sim. `range` guarded `> 0`.
---

## chromatic_aberration: radial offset = (uv-centre)·strength, per-component clamp ±0.5
**Status**: default
**Location**: crates/unendlich-engine/src/chromatic_aberration.rs
The offset is just `(uv - 0.5) * strength`, which is exactly radial (parallel to
the centre vector) and zero at the centre, clamped per component to `±0.5` so a
huge strength can't sample off-texture. Linear-in-radius is the cheap default; a
higher-order radial curve would concentrate the split at the very edge but wasn't
needed. Caller samples R at `+offset`, B at `-offset`.
---

## lens_dirt: mask × bloom × gain, clamped
**Status**: default
**Location**: crates/unendlich-engine/src/lens_dirt.rs
Straight product of the dirt mask, incident bloom and a gain, clamped to `[0,1]`,
so dirt is invisible without bloom and scales monotonically with each input. No
alternative weighed — this is the standard lens-dirt modulation.
---

## batch_depth_fade routes through soft_one for bit-parity
**Status**: considered
**Location**: crates/unendlich-engine/src/batch_depth_fade.rs
Same pattern as the prior batched cores: the batch calls the scalar reference per
item so the soft-fade ramp matches bit-for-bit; the microopt is purely
`with_capacity` + a tight loop over a shared `fade`. Mirrors R23 depth_fade
locally to stay standalone-testable.
---

### Thread 03 · WORLD

## Lava flow = amount · saturating-slope · inverse-viscosity, capped at amount
**Status**: default
**Location**: crates/unendlich-engine/src/world/lava_flow.rs
Spread is proportional to the lava present, scaled by a slope term `s/(s+k)`
(zero on flat ground, saturating when steep) and an inverse-viscosity term
`1/(1+v·k)` (thin flows freely, thick barely moves). Both terms live in [0,1], so
the product can never spread more lava than the cell holds — the conservation
guard the surface stepper needs.
---

## Snow step: accumulate below freezing, melt above, clamp at zero
**Status**: default
**Location**: crates/unendlich-engine/src/world/snow_accumulate.rs
A three-way branch on temperature vs the freezing point: below adds `precip·dt`,
above subtracts `melt_rate·(temp−freeze)·dt`, exactly at freezing holds steady;
the result is floored at zero. Making melt proportional to the degrees above
freezing (not a flat rate) means a warm snap clears snow faster than a thaw,
while a cold dry cell stays put.
---

## Settling = suspended · rate · still-factor 1/(1+flow·k)
**Status**: default
**Location**: crates/unendlich-engine/src/world/sediment_settle.rs
The still-factor is 1 in slack water and falls toward 0 as flow rises, so
sediment drops out when the current slows and stays suspended when it's fast.
Multiplying by `rate` and the suspended amount (both bounded) guarantees the
deposit never exceeds what's actually in the column — pairs cleanly with the
advection step that routed it there.
---

## Batched advection mirrors the R23 per-cell amount for bit-parity
**Status**: default
**Location**: crates/unendlich-engine/src/world/batch_erosion_advect.rs
`advect_one` reproduces R23 `erosion_advect`'s exact inner term —
`suspended · clamp01(rate)`, with non-positive results zeroed — and `advect_batch`
maps it over a slice sharing one pre-clamped rate. Amounts are bit-identical
(`to_bits()`) to the grid stepper, so a batched advection path routes the same
mass as a scalar fallback.
---


## ── Round-R25 module decisions ──

### Thread 01 · SIM

## taming gains affinity·feed_quality per feed, clamped to [0,1]
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/taming.rs` (`progress`)
`clamp01(cur + affinity·feed_quality)`, with both factors floored at 0. Making the
gain a product means either a wild creature (affinity 0) or worthless food
(quality 0) produces no progress — both "kept on 0 inputs" cases — while a high
affinity and a good treat compound for the fastest taming. The clamp gives the
bound and the success-at-1 condition for free. Pairs with the round-9 `bond_tame`
trust work as the per-feed progression on top of it.

## mount_stamina drains while sprinting, regens while resting, clamped [0,max]
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/mount_stamina.rs` (`stamina_step`)
A boolean `sprinting` picks `-drain·dt` vs `+regen·dt`; the result clamps to
`[0, max]`. Branching on the sprint flag (rather than a single signed rate) keeps
drain and regen as independent positive tunables, and flooring `dt`/`drain`/`regen`
at 0 plus clamping both ends gives the non-negative, capped, dt-scaling behaviour
in one place. Rates are placeholder tuning.

## ability_combo grows base by per-link, capped, never below base
**Status**: default
**Location**: `crates/unendlich-engine/src/sim/ability_combo.rs` (`combo_mult`)
`clamp(base + chain·per, base, max(base, cap))`. Flooring at `base` is the
deliberate bit: a combo should only ever reward, so even a negative `per` can't
drop the multiplier below the base hit, and clamping the ceiling at `max(base,cap)`
means a cap accidentally set below base can't invert the range. `chain=0`/`per=0`
both yield base. Same shape as `enchant_apply`/`set_bonus` but floored at base
rather than 0.

## scent_gradient climbs only toward neighbours stronger than the current cell
**Status**: considered
**Location**: `crates/unendlich-engine/src/sim/scent_gradient.rs` (`gradient_dir`)
`x = pos(e-here) - pos(w-here)`, `y = pos(n-here) - pos(s-here)`, clamped to unit
length. Using `pos(neighbour - here)` rather than a plain central difference makes
`here` meaningful: only neighbours that actually exceed the current cell pull, so a
creature at a local peak (here ≥ all neighbours) gets a zero vector instead of
being dragged toward the "least weak" side. Opposing equal pulls still cancel
(uniform/ties → 0), and the unit clamp keeps it a bounded steering hint. Follows on
from `scent_track` (r20) and `scent_decay` (r24).

## batch_status_stack shares one inline kernel with stack_one for parity
**Status**: anchored
**Location**: `crates/unendlich-engine/src/sim/batch_status_stack.rs`
Same pattern as the earlier batch cores: `stack_one` and `stack_batch` both call
one `#[inline]` kernel (the exact r23 `status_stack::add_stack` math), so the batch
is bit-for-bit identical to per-effect stacking (output is integer `u8`, so `==` is
exact). The batch applies one stack add across a contiguous `(cur, add, cap)`
slice — an AoE stacking a debuff on every target — so the win is in iteration, not
arithmetic.

### Thread 02 · RENDER

## exposure_adapt: target EV = -log2(lum), clamped-fraction step toward it
**Status**: considered
**Location**: crates/unendlich-engine/src/exposure_adapt.rs
Target EV is `-log2(scene_lum)` (brighter ⇒ lower EV), and each call moves a
`clamp(speed·dt, 0, 1)` fraction of the gap, so the step is bounded and never
overshoots — exponential-style smoothing without storing a time constant.
Considered a fixed EV-per-second slew (linear approach, can overshoot/oscillate)
and chose the proportional fraction for the no-overshoot + stable-at-target
properties.
---

## fxaa_edge: luma contrast (max-min) thresholded then ramped
**Status**: default
**Location**: crates/unendlich-engine/src/fxaa_edge.rs
Edge factor is the local luma range `max-min` over the centre+4 cross neighbours;
below `thresh` it's 0, above it ramps `(range-thresh)/(1-thresh)` to 1. Using
min/max makes it permutation-symmetric in the neighbours for free. This is the
contrast-detection front half of FXAA; the directional blend is the shader's job.
Default.
---

## refraction_offset: normal_xy · strength · 1/(1+depth), clamped ±0.5
**Status**: default
**Location**: crates/unendlich-engine/src/refraction_offset.rs
The background sample is bent by the screen-space normal tilt scaled by strength,
attenuated `1/(1+depth)` so deeper/farther refractors bend less, clamped per
component to `±0.5`. Linear-in-normal and inverse-linear depth attenuation are the
cheap defaults; clamp prevents off-texture sampling.
---

## batch_motion_blur_vec routes through vel_one for bit-parity
**Status**: considered
**Location**: crates/unendlich-engine/src/batch_motion_blur_vec.rs
Same pattern as the prior batched cores: the batch calls the scalar reference per
item so the clamped scaled deltas match bit-for-bit; the microopt is purely
`with_capacity` + a tight loop over a shared `scale`. Mirrors R23 motion_blur_vec
locally to stay standalone-testable.
---

### Thread 03 · WORLD

## Meander offset = amplitude · saturating(flow) · saturating(|curvature|), signed
**Status**: default
**Location**: crates/unendlich-engine/src/world/river_meander.rs
The lateral push is the product of an amplitude cap, a saturating flow term, and
a saturating curvature term, then given the sign of the curvature — so a
straight reach (curvature 0) yields exactly zero, bends deepen in the direction
they already lean, and the magnitude can never exceed the amplitude. Saturating
terms keep extreme flow/curvature from blowing the offset past the channel
budget.
---

## Glacier carve = ice · (1 + slope·gain) / hardness · dt, capped
**Status**: default
**Location**: crates/unendlich-engine/src/world/glacier_carve.rs
Carving scales linearly with ice thickness and time, divides by a hardness
floored away from zero, and gets a mild slope speed-up (faster ice flow erodes
more). No ice gives exactly zero; the per-step cap prevents a thick-ice/soft-rock
combo from cutting an absurd depth in one tick. The slope term is `≥1` so it only
ever speeds carving, never reverses it.
---

## Dune migration gates on both wind and sand supply
**Status**: default
**Location**: crates/unendlich-engine/src/world/dune_migrate.rs
Migration is `speed · windTerm · sandTerm · dt` with both terms saturating in
[0,1): no wind or no sand each drive distance to zero, and thin sand throttles an
otherwise windy step. Treating sand as a supply gate (low sand → slow) follows
the brief's framing rather than the inverse grain-size relation, since the input
is supply, not dune size.
---

## Batched frost mirrors the R21 scalar for bit-parity
**Status**: default
**Location**: crates/unendlich-engine/src/world/batch_frost_spread.rs
`frost_one` reproduces `clamp01((1 − clamp01(temp)) · clamp01(humidity))` exactly
from R21 `frost_spread`; `frost_batch` maps it over a sample slice. Coverage is
bit-identical (`to_bits()`) to per-cell calls, so a batched winter-material pass
renders the same frost as a scalar fallback.
---

## ── Round-26 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (combat stance — handoff/COMBAT_STANCE.md)
- guard_side [considered]: present the HIGHER-HP side to the threat (expose healthy limbs, keep the
  weak side from being finished off — per user). Hysteresis band so it doesn't jitter near-equal.
- vital_cover [default]: |sin(angle between facing and threat)| — 0 head-on (vitals forward), 1 side-on
  (flank shields torso). Simple, periodic, symmetric.
- engage_distance [default]: base·(1 + exposure + 0.5·threat), floored at melee reach.
- flank_turn_rate [considered]: threat-scaled urgency × err/(err+0.2) damping → fast under threat,
  slows near the target so it doesn't overshoot the stance.
- batch_spawn_budget [default]: in-file scalar allowed_one (min(pending,budget,cap−active)) + batch; bit-parity.
### Thread 02 · RENDER (part-aware meshing — handoff/BODY_PART_MESHING.md)
- part_mesh_lod [considered]: distance/32m steps coarser; damage relieves levels; recently_hit hard-forces
  finest (0) so an attacked part is always destructible.
- seam_skirt [default]: (2^Δlevel − 1)·voxel_size; equal levels ⇒ 0. Paired with the one-level-max-diff
  rule (part_mesh_lod neighbors) this keeps skirts small while staying watertight.
- voxel_greedy_run [default]: lossless run merge (Σ lengths == nonzero count); 0 breaks runs.
- batch_chromatic_aberration [default]: in-file ca_one ((uv−0.5)·strength, clamp ±0.5) + batch; bit-parity.
### Thread 03 · WORLD (landforms)
- canyon_carve [default]: quadratic ramp md·t², t=1−d/width; 0 beyond width.
- delta_fan [default]: flow·exp(−d/spread); peaks at mouth.
- karst_sink [default]: saturating 1−exp(−limestone·flow·time·k), reaches 1.0 at extreme input (underflow).
- batch_lava_flow [default]: in-file flow_one (clamp01(slope·amount/(1+viscosity))) + batch; bit-parity.

## ── Round-27 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (carnage/damage — handoff/BODY_PART_MESHING.md + stance_bias)
- limb_detail_flag [default]: fine if HP fraction < threshold OR hit within `cooldown` ticks (the
  cooldown stops a part flicking back to coarse between blows mid-fight).
- damage_shard [default]: shard count = floor(damage·scale), capped at max.
- dismember_check [considered]: detaches when (hp − incoming) ≤ −dismember_thresh — a margin past zero,
  so only real overkill severs a limb, not a hit that merely zeroes it.
- stance_bias [default]: 2·(hp/max) − 1 ∈ [−1,1]; full ⇒ aggressive, empty ⇒ defensive.
- batch_scent_decay [default]: in-file decay_one (strength·exp(−rate·dt)) + batch; bit-parity.
### Thread 02 · RENDER (mesh polish — handoff/BODY_PART_MESHING.md)
- normal_smooth [default]: normalized lerp of two face normals; falls back to n_a if near-opposite
  (no NaN) — hides faceting on greedy-merged quads.
- specular_aa [considered]: Toksvig-style √(r²+variance), clamped, never below input roughness — kills
  spec shimmer where merged/LOD normals vary.
- crack_fill [default]: push half the measured gap, capped at one voxel so it never over-pulls.
- batch_dof_blur [default]: in-file coc_one (clamp01(|depth−focus|/range)) + batch; bit-parity.
### Thread 03 · WORLD (sculpted landforms)
- mesa_form [default]: flat cap to 80% radius, cliff ramp to 0 at radius.
- hoodoo_carve [default]: piecewise base→neck(mid)→cap; narrowest at mid-height.
- fjord_cut [default]: depth = max·across(axis)·inland(coast); 0 past half-width.
- batch_river_meander [default]: in-file meander_one (amp·sat(flow)·sat(|curv|), signed) + batch; bit-parity.

## ── Round-28 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (pack combat AI — handoff/COMBAT_STANCE.md open list)
- pack_flank [considered]: spread members over a [-PI/2,PI/2] frontal arc, shifted ±PI/4 toward the
  enemy's EXPOSED side (opposite its guard). Bounds keep approach angles inside ±PI.
- cover_seek [default]: weak_exposure · (1 − dist/range); 0 past range — pull toward cover scales with
  how exposed the weak side is and how close cover is.
- focus_fire [considered]: pick min effective HP, exposure discounting HP up to 50% — pack ganks the
  most-finishable enemy, not just the lowest raw HP.
- disengage [default]: resolve = (hp + pack_strength)/2 + courage/4; retreat below 0.35.
- batch_guard_side [default]: in-file present_one + batch; bit-parity i8.
### Thread 02 · RENDER
- emissive_pulse [default]: base·(1 + depth·½(1+sin(2π·freq·t))) — glow breathing for dragons/lava, ≥0.
- parallax_offset [default]: view·height·scale, clamped ±0.25 so the texture doesn't swim.
- contact_ao [default]: visibility = clamp(gap/radius); 0 touching → 1 past radius.
- batch_specular_aa [default]: in-file clamp_one + batch; bit-parity.
### Thread 03 · WORLD
- talus_slope [default]: (slope−repose)⁺ · supply, capped at supply — scree below the angle of repose.
- oxbow_lake [default]: sat(curvature) · (1−exp(−age·k)); tight + old ⇒ cutoff lake.
- alluvial_fan [default]: gradient_drop · exp(−d/spread); apex-thickest mountain-front fan.
- batch_canyon_carve [default]: in-file depth_one + batch; bit-parity.

## ── Round-29 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (dragon flight dynamics)
- lift_force [default]: ½ρv²·area·CL, ρ folded into K=0.6125 — quadratic in airspeed.
- drag_force [default]: ½ρv²·area·CD, same K.
- glide_ratio [default]: L/D, drag floored at 1e-6 (no inf/NaN in level/zero-drag).
- bank_radius [considered]: v²/(g·tan bank); bank clamped < π/2; bank→0 returns ~1e9 (straight) to avoid
  a divide blowup.
- batch_flap_thrust [default]: in-file thrust_one (rate²·span·effort) + batch; bit-parity.
### Thread 02 · RENDER (sky / atmosphere scattering)
- rayleigh_phase [default]: (3/16π)(1+cos²θ) — symmetric, min perpendicular.
- mie_phase [considered]: Henyey–Greenstein (1−g²)/(4π(1+g²−2g·cosθ)^1.5); g clamped ±0.99 to keep the
  denominator finite; forward-scatter for g>0.
- sun_disk [default]: hard core to angular_radius + 30% penumbra falloff to 0.
- batch_mie_phase [default]: in-file mie_one + batch over cosines at fixed g; bit-parity.
### Thread 03 · WORLD (climate)
- altitude_temp [default]: base − altitude·lapse_rate (linear).
- rain_shadow [default]: windward·exp(−peak·dryness) — orographic dry-down on the lee side.
- tree_line [default]: base + 1500·warmth − 2000·latitude, floored at 0.
- batch_altitude_temp [default]: in-file temp_one + batch; bit-parity.

## ── Round-30 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (genetics / breeding — feeds the viewer genome export → egg breeding)
- gene_crossover [default]: lerp(parentA, parentB, mix), clamped — blends two genome vectors.
- gene_mutate [considered]: splitmix64 hash of seed → signed jitter ±rate; fully deterministic (no RNG
  state) so breeding/worldgen stay reproducible (R11).
- trait_dominance [considered]: lerp(avg, max, dominance) — codominant at 0, larger allele wins at 1.
- hatch_time [default]: base·(1+complexity)·(1−0.5·warmth), floored at 10% of base.
- batch_gene_crossover [default]: in-file cross_one + batch over a genome vector; bit-parity.
### Thread 02 · RENDER (PBR scale material)
- fresnel_schlick [default]: F0 + (1−F0)(1−cosθ)^5.
- ggx_distribution [considered]: GGX NDF; denominator floored at 1e-12 because at near-zero roughness +
  perfect alignment (a²−1) loses a² in f32 and d underflows to 0 → inf; floor keeps it finite (realistic
  roughness ≥0.1 is unaffected — exact GGX there).
- iridescence [default]: hue = frac((1−cosθ)·shift) — thin-film shimmer, wraps in [0,1).
- batch_fresnel [default]: in-file fresnel_one + batch; bit-parity.
### Thread 03 · WORLD (ecology / resources)
- ore_grade [default]: (1−exp(−depth/200))·noise·richness — deeper + richer ⇒ better.
- forest_density [default]: moisture·gaussian(temp, opt 0.6)·soil — temperate-moist optimum.
- spawn_density [default]: suitability·(0.3 + 0.7·activity), activity = nocturnality at night else inverse.
- batch_ore_grade [default]: in-file grade_one + batch; bit-parity.

## ── Round-31 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (elemental / status combat)
- elemental_multiplier [considered]: six-element cycle (Wood→Water→Air→Dust→Fire→Storm); beats next ×2,
  weak to prev ×0.5; mod-6 so it wraps. Simple, symmetric, extensible.
- burn_tick [default]: stacks · base · dt (linear DoT).
- freeze_slow [default]: 1 − max_slow·(1−exp(−0.4·stacks)); diminishing, floored at 0.05.
- resist_apply [default]: damage·(1−resist).
- batch_burn_tick [default]: in-file tick_one + batch; bit-parity.
### Thread 02 · RENDER (water/ocean surface)
- gerstner_wave [default]: (q·a·cos φ, a·sin φ) — horizontal pinch + vertical height.
- foam_threshold [default]: foam = clamp((cutoff − jacobian)/cutoff); folds/negatives ⇒ full foam.
- water_depth_color [default]: 1 − exp(−depth/scale) shallow→deep blend.
- batch_gerstner [default]: in-file wave_one + batch (both components); bit-parity.
### Thread 03 · WORLD (ocean / hydrology)
- wave_height [default]: 0.001·wind²·(1−exp(−fetch/5000)) — wind+fetch limited.
- tide_level [default]: amplitude·sin(2π·t/period).
- salinity [default]: 0.5 − 0.3·freshwater + 0.3·evaporation, clamped.
- batch_tide_level [default]: in-file level_one + batch; bit-parity.

## ── Round-32 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (crafting / economy)
- craft_quality [default]: 0.5·skill + 0.4·material + 0.1·luck, clamped.
- market_price [default]: base·(1+0.5·tier)·demand, tier capped at 10, floored at 1.
- skill_gain [considered]: max_gain·difficulty·(1−skill) — diminishing toward mastery.
- repair_cost [default]: (max−current)·rate.
- batch_market_price [default]: in-file price_one + batch; bit-parity.
### Thread 02 · RENDER (UI / HUD)
- healthbar_fill [default]: clamp(current/max), guards max≤0.
- radial_sweep [default]: clamp(value/max)·2π — cooldown clock.
- minimap_project [default]: world XZ → [0,1]² centered on camera, off-map clamps to edge.
- batch_healthbar_fill [default]: in-file fill_one + batch; bit-parity.
### Thread 03 · WORLD (atmosphere / weather)
- cloud_coverage [default]: humidity·(1−0.5·temp)·(1−0.4·altitude) — cool+humid clouds, thin aloft.
- wind_shear [default]: base·(1 + shear·(1−exp(−alt/1000))) — stronger aloft.
- precip_rate [default]: (cloud·humidity)² — needs both, superlinear.
- batch_cloud_coverage [default]: in-file cover_one + batch; bit-parity.

## ── Round-33 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (pathfinding / navigation)
- heuristic_octile [default]: √2·min + (max−min) — admissible 8-way grid A* heuristic.
- path_cost [default]: base·terrain·(1+slope), floored at 0.01 so A* terminates.
- step_toward [default]: clamped seek; arrives exactly within max_step, else unit step.
- turn_penalty [considered]: weight·(turnangle/π) — discourages zig-zag paths; 0 straight, w at reversal.
- batch_path_cost [default]: in-file cost_one + batch; bit-parity.
### Thread 02 · RENDER (particle / VFX curves)
- particle_size_curve [default]: lerp(birth,death) over age/lifetime.
- particle_alpha_curve [default]: fade-in 0–15%, hold, fade-out 70–100%.
- particle_color_ramp [default]: per-channel lerp(start,end) over life.
- batch_particle_alpha [default]: in-file alpha_one + batch; bit-parity.
### Thread 03 · WORLD (world-space sound)
- sound_attenuation [default]: 1 within reference, then r/(r+rolloff·(d−r)).
- doppler_shift [considered]: c/(c−v), v clamped to ±0.9c to dodge the singularity.
- reverb_wetness [default]: enclosure·(0.3+0.7·room_size).
- batch_sound_attenuation [default]: in-file atten_one + batch; bit-parity.

## ── Round-34 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (dragon breath abilities)
- breath_cone [default]: (1 − angle/half)² inside the cone, 0 outside — smooth edge falloff.
- breath_damage [default]: base·(1 − distance/range) inside range.
- ability_charge_time [default]: base·(1+power)·(1−haste), haste capped 90%, floored 0.1s.
- mana_cost [default]: base·(1+0.2·level)·(1−efficiency), efficiency capped 90%.
- batch_breath_damage [default]: in-file dmg_one + batch; bit-parity.
### Thread 02 · RENDER (day/night lighting)
- sun_elevation_color [default]: warmth = 1 − elevation/(π/2); warm low sun, neutral zenith.
- ambient_intensity [default]: night floor 0.05, lerped to 1.0 by elevation/(π/2).
- shadow_length [default]: 1/tan(elevation), elevation floored at 0.05 to cap low-sun length.
- batch_ambient_intensity [default]: in-file intensity_one + batch; bit-parity.
### Thread 03 · WORLD (seasons / time)
- season_phase [default]: −cos(2π·day/year) ∈ [−1,1]; day 0 winter, mid-year summer.
- daylight_hours [default]: 12 + phase·latitude·10, clamped [0,24]; equator ~12h.
- seasonal_temp_offset [default]: phase·amplitude.
- batch_seasonal_temp_offset [default]: in-file off_one + batch; bit-parity.

## ── Round-35 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (AI senses)
- sight_detect [default]: in-FOV (dot≥fov_cos) + in-range, strength = 1−d/range.
- hearing_radius [default]: (sound−ambient)⁺ · (1−d/range) — signal over the noise floor, distance-faded.
- scent_strength [considered]: exp(−d·decay) · downwind factor (0 upwind → 1 downwind) — wind matters.
- awareness_blend [considered]: probabilistic OR 1−∏(1−sense) — senses stack, any strong one alerts.
- batch_sight_detect [default]: in-file detect_one + batch; bit-parity.
### Thread 02 · RENDER (decal / projection)
- decal_fade [default]: 1−|dist|/thickness — keeps decals on-surface.
- projector_uv [default]: box-projector world→[0,1]², edge-clamped.
- detail_normal_blend [default]: UDN-style base+detail·strength, renormalized, degenerate→base.
- batch_decal_fade [default]: in-file fade_one + batch; bit-parity.
### Thread 03 · WORLD (territory / nesting)
- nest_suitability [default]: gaussian(elevation,opt 0.7)·shelter·prey — high sheltered crags with prey.
- territory_overlap [default]: (sum_radii − dist)/sum_radii — boundary overlap as conflict.
- resource_regen [default]: logistic r·cur·(1−cur/max)·dt, tiny seed so empty nodes recover.
- batch_nest_suitability [default]: in-file suit_one + batch; bit-parity.

## ── Round-36 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (inventory / equipment + status)
- stat_total [default]: (base + equipment) × buff_mult.
- carry_weight [default]: full speed under capacity (strength·10), else cap/weight floored at 0.1.
- poison_tick [considered]: % of MAX HP per stack·dt (scales with target, unlike flat burn).
- stun_duration [default]: power·(1−resistance), floored at 0.
- batch_stat_total [default]: in-file total_one + batch; bit-parity.
### Thread 02 · RENDER (camera)
- camera_fov_zoom [default]: base_fov/zoom, clamped — zoom in narrows.
- camera_shake [considered]: amp = trauma² (subtle small shakes), direction from a deterministic (t,seed)
  hash so playback is reproducible.
- depth_to_view_z [default]: n·f/(f − d·(f−n)) — linearize non-linear depth.
- batch_depth_to_view_z [default]: in-file z_one + batch; bit-parity.
### Thread 03 · WORLD (vegetation)
- grass_sway [default]: wind·sin(π·t + phase) — bounded bend. (test fix: compare peak vs trough, not two zero-crossings.)
- vegetation_density [default]: moisture·gaussian(temp,0.65)·sunlight.
- snow_cover [default]: (1−temp)·precip·(1−slope) — cold flat ground holds snow, steep sheds.
- batch_vegetation_density [default]: in-file dens_one + batch; bit-parity.

## ── Round-37 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (projectiles / physics)
- projectile_drop [default]: vy − g·dt (one Euler step).
- ballistic_range [default]: v²·sin(2θ)/g; max at 45°, symmetric about it.
- knockback_impulse [default]: (damage/mass)·(1−resist).
- terminal_velocity [default]: √(2mg/(ρ·area)), ρ≈1.225.
- batch_knockback_impulse [default]: in-file imp_one + batch; bit-parity.
### Thread 02 · RENDER (volumetric / fog)
- beer_lambert [default]: exp(−density·distance) transmittance.
- height_fog [default]: full below fog_top, exp falloff above.
- scatter_inscatter [default]: (1−transmittance)·light.
- batch_beer_lambert [default]: in-file trans_one + batch; bit-parity.
### Thread 03 · WORLD (hydraulic erosion)
- flow_velocity [default]: √(drop/distance), downhill only.
- erosion_capacity [default]: min(flow·slope, max) — faster steeper water carries more.
- deposition [default]: (suspended−capacity)·rate (>0 deposit, <0 erode).
- batch_flow_velocity [default]: in-file vel_one + batch; bit-parity.

## ── Round-38 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (AI memory / learning)
- memory_decay [default]: strength·exp(−ln2/half_life·dt) — exponential forgetting.
- threat_memory [default]: EMA toward observation, current+(observed−current)·lr.
- habituation [default]: base·exp(−rate·exposures) — repeated stimulus dulls response.
- reward_estimate [default]: Q-update prior+(reward−prior)·lr.
- batch_memory_decay [default]: in-file decay_one + batch; bit-parity.
### Thread 02 · RENDER/PHYS (cloth / soft-body)
- verlet_step [default]: new = pos+(pos−prev)+accel·dt² (velocity is implicit pos−prev).
- constraint_correction [default]: −0.5·(len−rest)·stiffness — half-error per point (cloth/mass-spring).
- cloth_wind_force [default]: |normal·wind|·strength·area — edge-on patches catch little.
- batch_verlet_step [default]: in-file step_one + batch; bit-parity.
### Thread 03 · WORLD (structures)
- arch_height [default]: 4x(1−x)·max_rise parabola — arches/bridges.
- tower_taper [default]: linear base→top radius by height.
- wall_crenellation [default]: periodic merlon/crenel via rem_euclid phase.
- batch_arch_height [default]: in-file h_one + batch; bit-parity.

## ── Round-39 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (equipment affixes / sets)
- affix_count [default]: item_level/20 + luck·max, capped at max_affixes.
- affix_magnitude [default]: base·(1+level/100)·(0.5+0.5·roll) — roll spans 50–100%.
- rarity_tier [default]: quality score → 0..5 (common…mythic) thresholds.
- set_tier [default]: pieces/step — set bonuses unlock at step multiples.
- batch_affix_magnitude [default]: in-file mag_one + batch; bit-parity.
### Thread 02 · RENDER/PHYS (rope / spring)
- spring_force [default]: Hooke F = −k·displacement.
- damping_force [default]: viscous F = −c·velocity.
- catenary_sag [default]: parabolic 4x(1−x)·depth hang approximation.
- batch_spring_force [default]: in-file force_one + batch; bit-parity.
### Thread 03 · WORLD (dungeon / structure layout)
- room_fits [default]: room + 2·margin ≤ region on both axes.
- corridor_length [default]: Manhattan |dx|+|dy| for axis-aligned corridors.
- stair_steps [default]: ceil(rise/step_height).
- batch_corridor_length [default]: in-file len_one + batch; bit-parity.

## ── Round-40 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (dragon breath / abilities)
- breath_falloff [default]: linear 1→0 over max_range.
- breath_recharge [default]: current + rate·dt, capped at 1.
- roar_fear [default]: proximity·power·(1−resolve).
- wing_buffet_force [default]: power·proximity AoE knockback.
- batch_breath_falloff [default]: in-file falloff_one + batch; bit-parity.
### Thread 02 · RENDER (projectile trails / VFX)
- trail_alpha_fade [default]: linear opacity 1→0 over lifetime.
- tracer_width [default]: base + speed·stretch, capped — motion streaks.
- muzzle_flash_intensity [default]: exp(−rate·age) quick flash.
- batch_trail_alpha_fade [default]: in-file alpha_one + batch; bit-parity.
### Thread 03 · WORLD (caves / biomes)
- cave_tunnel_radius [default]: base·(1+noise·variation), floored — pinch/bulge tunnels.
- stalactite_length [default]: drip·√age (diminishing accretion), capped.
- biome_blend_weight [default]: smoothstep across a blend band at the boundary.
- batch_cave_tunnel_radius [default]: in-file radius_one + batch; bit-parity.

## ── Round-41 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (loot / drop rates + status)
- drop_chance [default]: base·(1+luck)·(1−rarity_penalty), clamped.
- loot_quantity [default]: enemy_tier + luck·max, capped.
- magic_find_weight [default]: base·(1 + magic_find·tier) — shifts toward rarer tiers.
- status_combo_multiplier [default]: 1 + (active−1)·per_combo, capped (elemental synergy).
- batch_drop_chance [default]: in-file chance_one + batch; bit-parity.
### Thread 02 · RENDER (status / elemental VFX)
- burn_glow_intensity [default]: stack ratio × ~6 Hz flicker.
- freeze_tint [default]: stacks/saturation, clamped.
- shock_arc_jitter [default]: deterministic (t,seed) hash in [−amp,amp].
- batch_burn_glow_intensity [default]: in-file intensity_one + batch; bit-parity.
### Thread 03 · WORLD (rivers / lakes / moisture)
- lake_fill_level [default]: current + inflow − outflow, clamped to capacity.
- river_width [default]: scale·√flow_accumulation (hydraulic geometry), capped.
- moisture_from_water [default]: linear 1→0 over max_dist from shore.
- batch_river_width [default]: in-file width_one + batch; bit-parity.

## ── Round-42 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (status interactions)
- wet_lightning_bonus [default]: dmg·(1+wet·mult) — element synergy.
- freeze_shatter_bonus [default]: dmg·(1+freeze·mult) — physical vs frozen.
- diminishing_returns_resist [default]: dr_factor^applications — repeated CC less effective.
- debuff_duration_scale [default]: base·clamp(1−0.05·levelDiff, 0.25, 2.0).
- batch_wet_lightning_bonus [default]: in-file bonus_one + batch; bit-parity.
### Thread 02 · RENDER/PHYS (dragon flight dynamics)
- stall_factor [default]: 1 below stall angle, linear collapse to 0 past it.
- climb_rate [default]: (thrust−drag)/weight.
- turn_g_force [default]: (v²/r)/9.81 g-load.
- batch_stall_factor [default]: in-file stall_one + batch; bit-parity.
### Thread 03 · WORLD (settlement layout)
- building_spacing [default]: plot_size + road_width.
- plaza_radius [default]: √(pop·per_capita/π), capped.
- wall_perimeter [default]: 2·√(π·area) circular enclosure.
- batch_plaza_radius [default]: in-file radius_one + batch; bit-parity.

## ── Round-43 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (combat resolution)
- hit_chance [default]: acc/(acc+eva), 0.5 fallback.
- damage_after_armor [default]: raw·k/(armor+k) — diminishing mitigation.
- block_reduction [default]: max(incoming−block, 0).
- overkill_damage [default]: max(damage−remaining_hp, 0) for cleave.
- batch_damage_after_armor [default]: in-file mitigated_one + batch; bit-parity.
### Thread 02 · RENDER (combat-feedback VFX)
- hit_flash_intensity [default]: linear 1→0 over duration.
- damage_number_scale [default]: lerp(min,max) by damage/ref, capped.
- low_health_vignette [default]: rises below onset HP toward 1 at 0 HP.
- batch_damage_number_scale [default]: in-file scale_one + batch; bit-parity.
### Thread 03 · WORLD (road / path networks)
- road_grade_penalty [default]: 1 + |grade|·cost, capped.
- switchback_count [default]: ceil(gain/(max_grade·seg_len)) − 1.
- bridge_needed [default]: gap > max_fillable.
- batch_road_grade_penalty [default]: in-file penalty_one + batch; bit-parity.

## ── Round-44 module decisions (coordinator-built, single-thread mode) ──
### Thread 01 · SIM (dragon AI behavior)
- aggression_level [default]: hp · ally-advantage · 2, clamped.
- flee_threshold [default]: flee if hp < 0.3·(1−courage).
- target_priority [default]: threat · 1/(1+dist) · finisher(low-hp bonus).
- patrol_wait_time [default]: lerp(max,min) by alertness.
- batch_target_priority [default]: in-file priority_one + batch; bit-parity.
### Thread 02 · RENDER (weather)
- cloud_shadow_factor [default]: 1 − coverage·drift(t) ground darkening.
- rain_streak_alpha [default]: intensity · distance-fade.
- lightning_flash [default]: exp(−rate·age) sky flash.
- batch_rain_streak_alpha [default]: in-file alpha_one + batch; bit-parity.
### Thread 03 · WORLD (weather fronts)
- front_temperature_gradient [default]: smoothstep warm→cold across front width.
- pressure_gradient_wind [default]: coeff·|Δp|/distance (geostrophic).
- storm_intensity [default]: humidity·instability product.
- batch_storm_intensity [default]: in-file intensity_one + batch; bit-parity.

## Not yet documented

R26–R44 built single-threaded (~486 cores over 44 rounds). Dragon AI + weather render + weather fronts added.
