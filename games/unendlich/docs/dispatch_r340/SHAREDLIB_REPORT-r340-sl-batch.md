# SHAREDLIB_REPORT — r340-sl-batch (the `sim::batch_*` kernel cluster)

Cluster surveyed: **36** `sim/batch_*.rs` modules in this base (the brief cites 67;
the rest live in the r339 base — the patterns below generalize, the census method
is identical). Companion helper: `util::batch_kernel` (`map_into`/`map_collect`,
extracted in r330-refactor-batch).

RESPECT check: every `batch_*` here drives **continuous-state** dragons/mobs
(aggro, scent/memory fields sampled per-entity, flocking, perception) — NOT
block-mobs. No consolidation flattens state or assumes discrete block updates;
all parity is asserted bit-for-bit on `f32`/tuple/struct outputs.

## Candidate 1 — the map-over-slice `_batch` boilerplate → `util::batch_kernel`
**Pattern:** every `batch_*` repeats one of two shapes:
- terse: `items.iter().map(|&t| one(t)).collect()`
- verbose: `let mut out = Vec::with_capacity(n); for x in src { out.push(kernel(x)); } out`
Both are exactly `batch_kernel::map_collect(src, |x| kernel(x))` (or `map_into`
for buffer reuse). **Home:** `util::batch_kernel` (already exists).
**Status:** PROPOSE the sweep (coordinator migrates; r330-refactor-batch already
folded 8: aggro_decay, crit_roll, threat_assess, status_stack, scent_decay,
memory_decay, drop_chance, path_cost). **Cross-round overlap flag:** in the r339
base those 8 are likely already on `batch_kernel`; dedupe against that.

### Folds cleanly (1:1 pure map — 34 of 36), `_batch` site → fold:
Scalar `f32` out: affix_magnitude@10, breath_damage@10, breath_falloff@9,
burn_tick@7, damage_after_armor@10, drop_chance@10, engage_distance@21,
flap_thrust@11, gene_crossover@11, knockback_impulse@10, market_price@10,
memory_decay@10, path_cost@10, scent_decay@11, sight_detect@9, stat_total@7,
target_priority@11, wet_lightning_bonus@7, aggro_decay@50, appendage_ripple@44,
blink@61, crit_roll@35, horn_charge@36, threat_assess@30.
Non-scalar out (still a 1:1 map — `map_collect` is generic over `Out`):
guard_side@10 (`i8`), spawn_budget@9 (`u32`), status_stack@31 (`u8`),
breathe@45 (`(f32,f32)`), status_effect@42 (`(u8,f32)`), ability_cast@67
(`Effect`), gene@56 (`Phenotype`, input `&[&[u8]]`), settle@49 (`Contact` in),
perception@63 (captures shared `&Source`), loot_roll@62 (`Vec<(u16,u32)>` per
seed — folds as a map over seeds; note: allocates per item, see Candidate 3).

### KEEP custom (do NOT force onto a 1:1 `map_into` — 2 of 36):
- **batch_body_arc@37** — ragged fan-out: emits `Σ nᵢ` values + a `counts` vec
  (a flat-map, not 1:1). Already has a buffer-reuse `arc_batch_into`. If anything,
  it wants a *separate* `flat_map_into` primitive — proposed below, not forced.
- **batch_boids@83** — O(n²): each output reads every other agent (cross-item
  read). Expressible as a capturing map, but its value is the parallelize target
  (Candidate "perf bonus"), so the gather stays; kernel preserved.

## Candidate 2 — local `clamp`/`nonneg` helper defs → `util::mathx` (CROSS-CLUSTER)
Six modules define their **own** clamp/nonneg helper (true dedup, not std
`.clamp()` calls): ability_cast, aggro_decay (already folded by
r330-refactor-batch into `batch_kernel::nonneg`), appendage_ripple, breathe,
loot_roll, perception. **Home:** `util::mathx` (clamp/lerp/smoothstep) +
`batch_kernel::nonneg`. **Status:** PROPOSE. **OVERLAP FLAG → r340-sl-math** —
this is the same clamp/lerp consolidation sl-math owns; coordinator should dedup
(`batch_kernel::nonneg` should itself fold into `mathx::nonneg`, per the
`// TODO fold into mathx` left in r330-refactor-batch).

## Candidate 3 — vector helpers in `batch_boids` → `util::vecmath`/`simvec` (CROSS-CLUSTER)
`batch_boids` defines local `sub/add/scale/norm3/normalize` (5 fns) over `[f32;3]`.
**Home:** `util::vecmath` (scalar) / `util::simvec` (batched). **Status:** PROPOSE.
**OVERLAP FLAG → r340-sl-math** — vecmath/simvec consolidation; keep FP32 exact.

## Candidate 4 (minor) — ragged `*_into` buffer-reuse → a `flat_map_into` primitive
`batch_body_arc::arc_batch_into` hand-rolls the "reserve `Σnᵢ`, push variable runs,
record counts" shape. If another cluster has the same ragged fan-out, a
`batch_kernel::flat_map_into(src, &mut vals, &mut counts, f)` would home it.
**Status:** PROPOSE only (one instance here — not worth extracting unilaterally;
flagged for the overlap matrix in case sl-mesh/sl-worldgen have ragged emitters).

---

## PERF BONUS (a) — parallelize the heaviest kernel via `util::task_lanes`  [EXTRACTED]
**Target:** `batch_boids` — the only O(n²) kernel in the cluster (each agent sweeps
all others), so the heaviest by far and the one that benefits from threads.
**Done:** added `boid_batch_par` (+ `BOID_MIN_PARALLEL = 256`) routing the
per-agent sweep through `task_lanes::map_pure_threshold`. It's a textbook Class-P
map: read-only over the shared `agents` slice, disjoint owned outputs, no shared
mutable state, no locks. `map_pure_threshold` preserves input order and the
neighbour list is gathered in the SAME index order as the serial `boid_batch`, so
floating-point summation order is unchanged ⇒ **bit-for-bit identical**.
**Parity proof (the merge bar):**
- `boid_batch_par_matches_serial` (in-module): serial vs parallel `.to_bits()`
  equality across n ∈ {0,1,7,64,255,256,257,400,600} — straddles the threshold so
  BOTH the serial and the real parallel branch are exercised.
- Verified against **real rayon** `task_lanes` (the in-crate impl) in a throwaway
  cargo project: `serial_eq_parallel_bit_for_bit`, `forced_parallel_matches_serial_
  random_gains` (n always ≥ threshold, randomized gains), `parallel_preserves_
  order_and_determinism` — all green.
**No physics change:** identical bytes ⇒ identical flocking; the parallelism is in
*scheduling*, not in the numerics.

The same pattern fits any FOLD-CLEAN scalar kernel above (drive `map_collect`
through `map_pure_threshold` when the batch is large) — proposed for the
coordinator to apply per-call-site where batch sizes warrant it.
