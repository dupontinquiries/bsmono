# r79 — overworld spawn rebalance (operator: remove big mobs, add little sheep + lauschers)

## What the operator saw
The world was full of big tan long-necked creatures. Two causes:
1. The run command set `UNENDLICH_SPAWN_ARCHE=dragon`, which forces EVERY ambient spawn into a
   dragon body plan (observation mode). Drop that env var to get the real ambient ecosystem.
2. Even without it, the ambient roll put `CreatureKind::Dinosaur` (great reptilian megafauna,
   the long-necks) on the surface, while `Lauscher` was cave-only and `Fauna` (the little
   surface staple — the "little sheep") was the plurality.

## Change (world/ecosystem.rs habitat_weight)
- `Dinosaur` → 0 in every biome: removed from ambient spawns entirely. The kind still exists
  for spawn eggs + observation mode; it just no longer roams the overworld.
- `Lauscher` → given surface weights (Plains/Beach 5, forests 5, swamp/canyon 3, highlands 2,
  harsh edges 1) on top of its cave weight (6). It is now the gentle air-attuned surface
  listener its `item.rs` description always called it, not a cave-only horror.
- `Fauna` unchanged — still the plains/forest plurality (the little sheep).
Result on plains: Fauna 9 (lead) · Lauscher 5 · Spaeher 4 · Dinosaur 0 · Dragon 0.
Tests updated to the new intent (9 ecosystem/census tests). Dragon ambient unchanged (sparse,
peaks/canyons only — it was never the lowland ambient; the screenshot's dragons were the env force).
