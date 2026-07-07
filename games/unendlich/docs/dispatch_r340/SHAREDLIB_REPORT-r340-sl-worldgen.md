# SHAREDLIB_REPORT — r340-sl-worldgen

**WID:** `r340-sl-worldgen` · **Cluster:** `world::{cpu_generator, biomes, flora_placement,
vegetation_scatter, structure_place, structure_grammar, plant_blueprint}` (+ `rare_structure`,
absent on this base). **Base built/gated against:** `unendlich-r330b-…-VERIFIED` (the r339 base
wasn't in this bundle; `rare_structure`/`mathx` are absent — see "Base notes"). Line numbers are
against this tree.

## Summary
The worldgen cluster re-implements the same **deterministic-hash → unit-roll → density-curve**
arithmetic in 6+ places, across **four distinct mixer families**. The split that matters for the
RESPECT-determinism rule: some sites are *bit-identical* to the canonical `util::hash_util::mix64`
(safe to route, proven), and some are *different mixers* (routing them onto a different hash would
relocate spawns — forbidden). I **extracted** the parity-safe pieces into a new
`world::worldgen_noise` and **propose** the rest.

## Candidates

### C1 — `bits_to_unit`: hashed-bits → `[0,1)`  · **EXTRACTED**
The idiom `((h >> 40) as f32) / ((1u64 << 24) as f32)` is duplicated **verbatim**:
- `flora_placement.rs:241` (tail of `hash01`)
- `vegetation_scatter.rs:55` (`unit`)
`worldgen_noise::bits_to_unit` is **bit-identical** (proven: `bits_to_unit_matches_sites_and_is_unit`,
5000 fuzzed inputs). Drop-in for both.
→ **Cross-cluster overlap:** this is a generic hash primitive — it belongs in `util::hash_util`
(owned by **sl-math**). Placed in `worldgen_noise` provisionally; coordinator should hoist it.

### C2 — splitmix64 mixer re-impls == `hash_util::mix64`  · **PROPOSE (route, parity-proven)**
Two private mixers are byte-for-byte `hash_util::mix64`:
- `structure_grammar.rs:88` `fn hash(x: u64)`
- `vegetation_scatter.rs:48` `fn mix(x: u64)`
Proven identical to `hash_util::mix64` (`hash_util_mix64_is_bit_identical_to_the_mix_sites`, 5000
inputs). **Propose:** delete both, call `hash_util::mix64`. No output change.
→ **Cross-cluster overlap with sl-math** (owns `hash_util`) — coordinator merges once.

### C3 — `fmix64` (MurmurHash3 finalizer)  · **EXTRACTED (named variant)**
`cpu_generator.rs:984` `tree_origin_hash` uses the murmur3 64-bit finalizer (`0xff51afd7…`,
`0xc4ceb9fe…`, `>>33`) — a **different family** from splitmix64. `worldgen_noise::fmix64` reproduces
that finalizer bit-for-bit (`fmix64_matches_tree_origin_finalizer`). **Propose:** route the finalizer
portion of `tree_origin_hash` (and any other murmur sites) through `fmix64` **without** changing
bytes. Do **NOT** swap it onto `mix64` — that would alter terrain.
→ Also a `util::hash_util` candidate (sl-math).

### C4 — density curve (lushness × moisture × slope)  · **EXTRACTED**
Two near-identical curves with different coefficients:
- `vegetation_scatter.rs:25` `density = clamp01(base * m * (1 - s))`
- `flora_placement.rs:224` `flora_density = clamp01(base * (0.4 + 0.6 m) * (1 - 0.8 s))`
`worldgen_noise::density_curve(base, m, s, moisture_floor, slope_atten)` reproduces **both** exactly:
veg = `(…, 0.0, 1.0)`, flora = `(…, 0.4, 0.8)` (proven bit-for-bit:
`density_curve_reproduces_both_existing_curves`, 3000 fuzzed incl. out-of-range). Drop-in for both.

### C5 — placement roll `roll(seed,x,z) < density`  · **EXTRACTED canonical + PROPOSE**
`vegetation_scatter.rs:35` `place` is `unit(hash2(..)) < clamp01(density)`. `worldgen_noise::{roll01,
place_at}` is the canonical version on `hash_util`. NOTE: the bespoke coordinate-combiners
`flora_placement::hash01` (`:234`, interleaves coord-xors between splitmix stages) and
`vegetation_scatter::hash2` (`:40`) are **NOT** bit-equal to `roll01` — swapping them relocates
placements, so this is **propose-for-new-code / migrate-with-a-regen-test**, not a silent drop-in.

### C6 — distinct, weaker hashes  · **PROPOSE named variants (do NOT force)**
- `plant_blueprint.rs` (`:233,291,308,…`) — repeated `h.wrapping_mul(0x9E37_79B9)` 32-bit step.
- `structure_place.rs:134` — Knuth multiplicative `* 2654435761`.
Different families; report only. **Propose** `hash_util::{mix32, knuth32}` named variants if the
coordinator wants them unified — never folded onto `mix64`.

### C7 — biome lushness table  · **PROPOSE (biomes overlap)**
`flora_placement::biome_density_base(Biome)` (`:185`) and `vegetation_scatter::biome_base(u8)` both
map biome → base lushness. **Propose** one method on `Biome` (e.g. `Biome::flora_density_base()`),
read by both (vegetation via `Biome::try_from(u8)`). Touches the `Biome` enum → **overlaps
T6/biomes**; coordinator owns the `Biome` sweep. (The brief's "route the r336 `Biome::flora_density`
consumers" lands here; note this tree predates r336's `flora_density`, so it's a propose.)

## Cross-cluster overlap flags (for the matrix)
- **sl-math:** C1 `bits_to_unit`, C2 routing to `hash_util::mix64`, C3 `fmix64`, C6 mix32/knuth32 —
  all are `util::hash_util` material; sl-math owns it. Pick the canonical home there.
- **sl-batch:** C4 `density_curve` / C5 `place_at` are the per-cell kernels the `batch_*`/scatter
  loops call — coordinate so the parallel-scatter kernel isn't double-extracted.
- **sl-mesh:** none of mine overlap (FACE_NORMALS/quad live there).
- **biomes (T6):** C7 (the lushness table on `Biome`).

## Extracted module: `world::worldgen_noise`
`bits_to_unit`, `fmix64`, `hash_pos2`, `roll01`, `density_curve`, `place_at`, `scatter_region`.
Built on `util::hash_util` (no new hash family). 7 tests; **bit-parity proven** for every
"drop-in"/"route" claim above.

## PERF BONUS (a) — parallel scatter via `task_lanes`
`scatter_region(seed, cells, min_parallel, density_at)` routes the per-cell `place_at` kernel
through `util::task_lanes::map_pure_threshold`. The kernel is pure (`Send + Sync`, no shared mutable
state). **Proven serial == parallel** bit-for-bit over a 200×200 region
(`parallel_scatter_equals_serial`). This is the data-parallel shape `cpu_generator`'s vegetation/ore
scatter and `vegetation_scatter::place` loops can adopt.

## RESPECT compliance
- No field flattened, no precision changed: this consolidates **hash/roll/density arithmetic** only;
  terrain/climate stay continuous FP fields sampled elsewhere (untouched).
- Exact seeded determinism preserved: every routed/drop-in claim is **bit-parity-tested**; the
  distinct mixers (C3/C5/C6) are reported, not silently merged.
- No Minecraft assumptions: placement is a continuous density × deterministic roll over FP cells,
  not a block-palette lookup.

## Base notes
On the real r339/r340 base, re-confirm the line numbers and that `rare_structure` (absent here) +
the r336 `Biome::flora_density` are included in the C7 routing. `mathx` (r332) absent here — none of
my extractions need it (I use `f32::clamp`).
