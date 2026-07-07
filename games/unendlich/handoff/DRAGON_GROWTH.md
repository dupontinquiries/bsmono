# Dragon growth-size wiring (r681)

`sim::dragon_growth_stage` (dormant, 12 tests) maps age + maturation genes →
`GrowthState { stage, size_mul, stat_t }`. r681 adds the two growth-scaling
primitives it was meant to feed, both tested/checked:

- `sim::dragon_blueprint_from_genome::dragon_blueprint_grown(g, size_mul)` —
  scales every blueprint part's offset + half_extents (uniform shrink toward the
  body centre; proportions preserved). `size_mul==1.0` is identity. 3 new tests.
- `spawn::dragon_body_half_extents_grown(g, size_mul)` — the physics-box twin, so
  a hatchling's collider matches its small model.

## Live wiring (NOT done — needs creature age + a render rescale)
1. **Track age.** Roamer/Creature has no birth tick. Add `spawn_frame: u64` (or
   `age_ticks`) to the Roamer, set at `build_roamer`, and derive
   `age = frame_count - spawn_frame`.
2. **Maturation genes.** Pull the two maturation genes from the dragon's genome
   into `dragon_growth_stage::MaturationGenes` (neutral() if not modelled yet).
3. **Compute** `let gs = dragon_growth_stage(age_ticks, mat);` each frame (or on a
   cadence — size changes slowly).
4. **Apply size_mul:**
   - physics: spawn with `dragon_body_half_extents_grown(g, gs.size_mul)` and
     rescale the body AABB as it grows (or step the collider at stage changes);
   - model: scale the forge model by `gs.size_mul` at skin time (multiply the
     ForgeModel box positions+sizes), OR drive the anatomy from
     `dragon_blueprint_grown`.
5. **Stats.** `gs.stat_t` scales combat stats alongside size.

Result: dragons hatch small and grow to full size — the visible payoff of the
growth core. Until then these are pure, tested primitives with no live caller.

## r682 — forge-model scaler added (the missing render primitive)
r681 scaled the blueprint + physics box but NOT the forge model that actually
draws (`world/entity.rs:1189` skins `creature.forge`), so a grown dragon would
have looked adult-sized with a small collider. Added:
- `dragon_forge::scale_model(model, size_mul)` — uniformly scales every ForgeBox
  center+half, the joint pivots, cell_size, and min/max about the model origin
  (feet near y=0 stay grounded). `1.0` = identity; half-extent floor keeps boxes
  solid. Rig/joint indices + colours preserved.
- `dragon_forge::build_grown(g, res, size_mul)` = `scale_model(&build(...), ...)`.
2 tests (identity at 1.0; hatchling smaller + grounded). Now blueprint, box, AND
model scale together, so the growth triad is complete.

### Remaining to make it VISIBLE (unchanged shape, now unblocked)
Only two steps left, both needing per-creature age (still absent):
1. Add `spawn_frame: u64` to Roamer (set in `build_roamer` + the 3 other inline
   `Roamer{}` spawn sites), derive `age = frame_count - spawn_frame`.
2. Either (a) SPAWN VARIETY: pick a growth stage at spawn and build the dragon via
   `spawn_dragon_with_genome_res` using `build_grown` + `dragon_body_half_extents_
   grown` (wild dragons spawn as a mix of juveniles→adults); or (b) GROW OVER TIME:
   each frame compute `dragon_growth_stage(age, mat).size_mul` and re-skin
   `scale_model(&creature.forge, size_mul)` at `world/entity.rs:1189` (+ step the
   collider). (a) is the smaller, safer first cut.

## r684 — spawn-time growth is LIVE (visible)
All dragon spawn paths now apply a growth size: `spawn()` (position-seeded wild +
H-key), `spawn_dragon_arche` (SPAWN_ARCHE/bred), and `spawn_dragon_element` (spawn
eggs) route through the new `spawn_dragon_with_genome_grown(pos, g, res, size_mul)`,
which scales the forge visual (`build_grown`), physics box (`_grown`), anatomy
(`dragon_anatomy_grown`) and mass (∝ s³) together. `spawn_growth_size_mul(seed)`
rolls the size via `dragon_growth_stage` weighted ~74% Adult (1.0) / ~18% Juvenile
(~0.75) / ~8% Hatchling (~0.36), so a wild population reads as a real age range.
To force all-adult, make `spawn_growth_size_mul` return 1.0.

STILL a further step: per-tick *growing up* over a creature's lifetime (a hatchling
maturing into an adult in-world) — needs the age field + a live rescale of the
stored forge Arc + body, per the steps above. Spawn-time age is the visible first
cut; lifetime growth is the sequel.
