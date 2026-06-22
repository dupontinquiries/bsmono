# HANDOFF — rare-structure wiring (`r332-structures`)

**Worker:** `r332-structures`  **Status:** wiring is *proposal-only* (SHARED_RULES §2 — no
bin/worldgen edits applied). The pure selector below is shipped + gated.

## 0. What shipped (pure, new file, gated)
`world::rare_structure`:
- `RareKind { Obelisk, Shrine, Ruin, Monument }` (+ `ALL`, `name`, `index`, `axiom`).
- `RareEntry { kind, weight, min_spacing, allowed_biomes: u32 }`, `RarityTable { region_rate,
  region_span, entries }` (+ `Default`), `RarePlacement { kind, origin, rotation, axiom }`.
- `select_for_region(region:[i32;2], world_seed:u64, biome:u8, &RarityTable) -> Option<RarePlacement>`
  — deterministic, pure, alloc-free, panic-free.
- mask helpers `biome_bit(id)`, `biomes_mask(&[u8])`.

Guarantees: deterministic; **exact min-spacing** (same-kind hosts sit on a seeded sublattice
of period `min_spacing`, so any two are ≥ spacing apart, Chebyshev); rarity via `region_rate`;
biome-gated; ≤ one rare per region.

## 1. Where worldgen calls it
During region/chunk generation, once per **region** (a coarse cell — pick the existing
region/sector granularity worldgen already iterates; the structure pass is the natural home).
Pseudocode for the generator's structure pass:

```rust
use crate::world::{rare_structure as rs, structure_grammar as sg, structure_place as sp};

// `region` = coarse cell coords; `biome` = dominant biome of the region.
if let Some(p) = rs::select_for_region(region, world_seed, biome as u8, &rarity_table) {
    // 1. Footprint from the kind's axiom (grammar caps expansion length).
    let expanded   = sg::expand(p.axiom, RULES_FOR.get(p.kind), iters_for(p.kind));
    let placements = sg::to_placements(&expanded, world_seed ^ region_mix(region));

    // 2. World origin of the structure: region base + within-region offset, rotated.
    let base = region_to_world(region);             // worldgen's existing region→block map
    let anchor = [base[0] + p.origin[0], base[1] + p.origin[1]];
    let cells  = rotate_placements(&placements, p.rotation, anchor); // p.rotation ∈ 0..4 quarter-turns

    // 3. (Optional) snap to the best ground in the region via structure_place:
    //    build SiteFeatures for the region's cells, then
    //    sp::rank_sites(&features, w, h, &prefs_for(p.kind), 1) → top Candidate as the anchor.
    stamp_template(p.kind, cells /* or ranked site */);
}
```

- `rarity_table`: build once at world setup. `RarityTable::default()` is a sane start;
  `allowed_biomes` are `u32` bitmasks — construct from the real enum with
  `rs::biomes_mask(&[Biome::Plains as u8, Biome::Desert as u8, …])` so the gate tracks the
  `Biome` enum (the selector stays decoupled from it).
- `region_span` should match the within-region offset range you want (in the same units as
  `p.origin` — block cells within a region). Set it to the region's block span (or a margin
  inside it so a rotated footprint fits).
- `p.rotation` is a quarter-turn count (0..4), matching `structure_grammar`'s 4-direction
  turtle — apply it to the `to_placements` output (swap/negate x/z per turn).

## 2. Choosing prefs per kind (optional refinement)
If you want rares to sit on good ground rather than the raw offset, give each `RareKind` a
`StructurePrefs` (flat for monuments/shrines, tolerant for ruins) and run
`structure_place::rank_sites` over the region's `SiteFeatures` to pick the cell; use
`p.origin` only as a fallback when the region has no good site.

## 3. Determinism contract for the caller
Pass a **stable** `world_seed` and integer `region` coords — never a clock/RNG. The same
`(region, seed, biome, table)` must always yield the same placement (that's what makes rares
reproducible across reloads and across clients). `to_placements` is seeded separately; feed it
a region-mixed seed (shown above) so two regions with the same axiom don't stamp identically.

## 4. Tuning notes
- `region_rate` is the master rareness dial (lower = rarer). Spacing controls clustering;
  weight controls the mix among kinds anchored at the same region.
- Because each kind anchors on a `1/min_spacing²` sublattice, very large `min_spacing` makes a
  kind genuinely scarce even before `region_rate` — combine the two for "one monument per few
  thousand regions" feels.

## 5. Verify commands I ran
```
rustc --edition 2021 --test rare_structure.rs -o /tmp/t && /tmp/t   # 8 passed / 0 failed (rustc 1.96)
rustc --edition 2021 --crate-type lib rare_structure.rs            # clean, 0 warnings
git apply --check -p1 CHANGES-r332-structures.patch                # CLEAN
cargo check -p unendlich-engine --lib --tests --features "render-core egui"  # 0 errors (in-tree typecheck)
```
Base note: validated against the `r330b` tree I had on hand (the r332 base wasn't provided);
`rare_structure` is an additive new file whose only shared touch is one `pub mod` line after
`pub mod structure_grammar;`, so it is base-agnostic — the patch applies wherever that anchor
line exists (it predates r332).
