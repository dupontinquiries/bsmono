# SHAREDLIB_REPORT — `r340-sl-fields` (physics field operators)

Cluster: `world::{fields, field_step, field_brush}` + `sim::{wind_field, fire_field,
particulate_field, perception_field, canopy_field_shift}` (the other `*_field` modules named in the
brief — `moisture_field`, `storm_field`, `water_field`, `cave_field`, `ember_field`,
`field_diffuse_ref` — are **absent in my base** `r330b`; flagged, see §Base note).

Proposed shared home: **`world::field_ops`** (NEW, shipped + gated this thread).

## Candidate 1 — `clamp01` (clamp to [0,1], NaN→0) — EXTRACTED ⚠ split into two
**The duplication, and the trap.** Two textually different forms exist, and they are **NOT
bit-identical** — they diverge for exactly one input, `-0.0`:
- **branch form** (`if nan{0} else if <0{0} else if >1{1} else {v}`) → keeps `-0.0`:
  - `world/field_step.rs:50` (`clamp01`)
  - `sim/fire_field.rs:226` (`clamp01`)
- **max/min form** (`if nan{0} else {v.max(0).min(1)}`) → normalizes `-0.0`→`+0.0`:
  - `sim/canopy_field_shift.rs:47` (`clamp01`)
  - `sim/particulate_field.rs:31` (`c01`)

Extracted **both**, bit-exact to their own call sites: `field_ops::clamp01` (branch) and
`field_ops::saturate` (max/min). **Do NOT cross-route** — merging all four onto one form would
silently flip the sign of a zero in a field (a RESPECT violation). Proven: 20k-sample fuzz, each
form bit-matches its originals; they agree on every f32 except `-0.0`.
**Cross-cluster overlap:** `r340-sl-math` will almost certainly propose `mathx::clamp`/`saturate`
for the same helper. **Coordinator: dedup here.** If `mathx::saturate` is the max/min form it can
host the canopy/particulate sites; the branch form (field_step/fire) needs the `-0.0`-preserving
variant. Recommend `mathx` exposes BOTH and `field_ops` re-exports, or `field_ops` owns the
field-path variants and `mathx` the general ones — coordinator's call. Flagging the `-0.0`
distinction so neither thread merges them blindly.

## Candidate 2 — clamp-to-bounds accumulate — EXTRACTED
`accumulate(base, delta, lo, hi) = (base+delta).clamp(lo,hi)` — the bounded write every field
brush + per-cell source/decay step shares.
- `world/field_brush.rs:` `accumulate` (already the canonical form; byte-identical extract).
- the per-cell "add then clamp" in `sim/particulate_field.rs` (settle/decay) and the
  source-injection steps follow the same shape.
Extracted as `field_ops::accumulate` (byte-identical to `field_brush::accumulate`, proven).
**Overlap:** also a `mathx` candidate (clamp). Coordinator dedup.

## Candidate 3 — diffusion stencils — EXTRACTED as TWO DISTINCT ops (key finding)
There are **two numerically different** diffusion formulations; they must NOT be collapsed:
- **edge-flux, mass-conserving** (`f = r*0.25*(a-b)`, delta-buffered, +x/+y edges once):
  `world/field_step.rs:` `diffuse_step` — "the GPU-perfect reference". → `field_ops::diffuse_flux`.
- **gathered 4-neighbour** (`acc += diff*(nb-here)` for each in-bounds neighbour, closed edges):
  `sim/particulate_field.rs:` the diffusion sweep in `step`. → `field_ops::diffuse_gather`.
Both extracted **byte-identical** to their source (proven vs verbatim copies on fuzzed grids), and a
test asserts they genuinely differ on the same input — so the coordinator hosts both rather than
merging. `fire_field` has a *reaction-diffusion* front (`fire_field.rs:147 step`) coupling fuel
burndown + ignition — **not** a plain diffusion; left as-is (real per-kernel logic, do not fold).

## Candidate 4 — semi-Lagrangian advection — PROPOSED (not extracted)
`sim/particulate_field.rs:` the advection block (backtrace `sx = x - wu*advect`, bilinear sample)
is a reusable `advect(grid, w, h, wind, amount)`. **Propose** moving it to `field_ops::advect` once
a second advecting field exists (storm/wind drift) to confirm the shared shape — extracting from a
single call site now risks fitting the op to one caller. Bilinear helper (`particulate::bilinear`)
travels with it. Coordinator: extract when `storm_field`/`wind_field` advection lands.

## Candidate 5 — radial brush stencil — ALREADY SHARED (report only)
`world::field_brush` (`Falloff`, `FieldBrush`, `brush_cells`/`brush_columns`, `apply_brush_3d/2d`,
the `ScalarField3/2` traits) is **already the consolidated brush primitive** — no extraction
needed. **Action:** point any field doing its own radial falloff at `field_brush`; none in my base
duplicate it (the elemental-item path already routes through it). Its `Falloff::Smooth`
(`u*u*(3-2u)`) is a smoothstep — overlaps `mathx::smoothstep` (sl-math); coordinator dedup the
smoothstep, keep the brush.

---

## RESPECT compliance (this is the field thread)
- No field flattened to booleans; all ops operate on the `f32` sampled grids.
- No precision change; FP32 throughout, float op order preserved exactly (neighbour accumulation
  `-x,+x,-y,+y` kept identical) so extracts are **bit-for-bit**.
- No Minecraft assumptions: these are PDE-like stencils over sampled continuous fields, not block
  arrays; the two diffusion formulations are kept distinct precisely to preserve field semantics.

## PERF BONUS (a) — parallelize the gathered diffusion via `util::task_lanes`
`field_ops::diffuse_gather_parallel` routes the per-cell kernel (`gather_cell`, pure — reads the
immutable snapshot `src`, writes a disjoint owned output, `Send+Sync`, no shared mutable state)
through `task_lanes::map_pure_threshold`. `task_lanes` preserves input order, and the kernel is a
pure map, so the result is **bit-for-bit identical** to the serial `diffuse_gather` — proven in
`parallel_gather_matches_serial_bit_for_bit` (serial-gated and threshold-0 paths both match). No
locks in the hot path. (In-tree this uses the real rayon `task_lanes`; the standalone gate proves
the wiring with a faithful order-preserving serial shim — see MANIFEST.)

## What I extracted vs proposed
- **Extracted** (in `world::field_ops`, ≥6 tests, bit-parity proven): `clamp01`, `saturate`,
  `accumulate`, `diffuse_flux`, `diffuse_gather`, `diffuse_gather_parallel`.
- **Proposed** (reported, not extracted): `advect` (Candidate 4); brush is already shared (5).
- **Did NOT rewrite call sites** (per the brief — coordinator sweeps once so overlaps merge).
  Call-site → replacement map: field_step::clamp01, fire_field::clamp01 → `field_ops::clamp01`;
  canopy_field_shift::clamp01, particulate::c01 → `field_ops::saturate`; field_brush::accumulate →
  `field_ops::accumulate`; field_step::diffuse_step → `field_ops::diffuse_flux`;
  particulate diffusion sweep → `field_ops::diffuse_gather`.
