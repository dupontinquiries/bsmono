# r604 — test-feedback fixes & handoffs

Version bumped: engine crate → `0.98.0-r604-testfixes`.

**Base-tree note:** these were authored against the **r594** source tree; the
tester is on **r604**. For anything that changed in r595–r604 (autotune, the
reworked inventory UI, any buoyancy retuning) reconcile against your current
files — the *reasoning* and the pure cores below hold regardless.

Sandbox-verified pure changes (all `rustc --test` + harness GREEN):
- `sim::fluid::flow_step` — lateral flow made **bidirectional** (cave fill).
- `phys::fluid_forces::swim_control_accel` — mass-independent swim-down.
- `sim::creative_tabs` — added **search** (`name_matches`, `tab_items_matching`, `search_all`).
- `sim::refresh_snap` — **500 Hz** autotune target + snap ladder `[120,240,360,480,500]`.

---

## CHECKLIST

### 1. Autotune target 500 Hz, snaps at 120/240/360/480/500
`sim::refresh_snap` now encodes this as the single source of truth:
`AUTOTUNE_TARGET_HZ = 500`, `SNAP_LADDER = [120,240,360,480,500]`,
`snap_at_or_below(fps)` (hold the highest sustainable rung) and
`nearest_snap(fps)`. **Wire:** point the frame autotuner's target ceiling at
`AUTOTUNE_TARGET_HZ` and route its chosen present rate through
`snap_at_or_below(measured_fps)`. Wherever the old ceiling/ladder was hard-coded
(look for the previous max target and any `[..]` fps rung array in the present-
pacing / adaptive-resolution controller), replace with these constants so it
can't drift out of sync again.

### 2. Version → r604
Done in `crates/unendlich-engine/Cargo.toml`. Bump other crates if your release
process expects a matching workspace version.

---

## BUGS

### A. Plants still growing in the air
Root cause: `world::flora_placement::place_at` decides *whether/which* species per
column but **not the Y or the support check** — the caller stamps at the
heightmap `surface_y`, which floats over carved caves/overhangs (the real top
solid block is lower). **Fix (caller side, `chunk_flora` stamping):** before
placing a plant at `(x, y, z)`, require the block directly below to be solid
ground:

```rust
let below = world.block_at(x, y - 1, z);
if !(below.is_solid() || below == BlockId::Grass || below == BlockId::Dirt) {
    continue; // nothing to root into — skip, don't float
}
// also guard against a carved cell AT the plant position:
if !world.block_at(x, y, z).is_replaceable() { continue; }
```

The r595 `sim::block_placement` core expresses exactly this rule
(`requires_support` + `below_solid` → `PlaceDenied::NoSupport`); the flora stamp
can call it or inline the two checks. Also re-run this check when the supporting
block is removed (a plant whose ground is mined should pop), which the block-
break path should already trigger a re-mesh for.

### B. Bushes under trees: grass texture but a solid block outline obscuring the mesh
Root cause: the bush block is flagged **solid** (and/or opaque), so it (1) gets a
full-cube selection AABB/outline and (2) occludes the cross-quad foliage mesh.
In r594 the standard foliage (`TallGrass`, `Fern`, `DeadBush`, `Flower`, `Vine`,
`Mushroom`, `Lichen`) is correctly excluded from `BlockId::is_solid`; the r604
bush block was added later and missed those lists. **Fix (`world::blocks`):** for
the bush `BlockId`, ensure:
- excluded from `is_solid()` (removes the full-cube collision + selection outline),
- `is_transparent()` returns `true` (so neighbour faces aren't culled and it
  doesn't occlude the mesh behind it),
- it renders via the foliage cross-quad / subvoxel path, not the full-cube face
  emitter (mirror how `Fern`/`TallGrass` are meshed).

The "solid block outline" is the giveaway that it's in the `is_solid` set —
that's the one to change first.

### C. Inventory: "Items" is all blocks; no Food/Weapons/Building/Natural/Stone/Other tabs or search
Two gaps, both now closed at the model level:
- **Tabs already exist as data** (`sim::creative_tabs`, r454: Terrain/Stone/Ores/
  Wood/Building/Plants over `BlockId::ALL`) but aren't wired into the menu — the
  UI still shows one flat list. **Wire:** render `creative_tabs::ALL_TABS` as a
  tab bar and populate each with `creative_tabs::tab_items(tab)`.
- **Non-block item tabs** (Weapons/Tools/Creatures, and Food once food items
  exist) come from r595 `sim::item_category` (`Category` + `category(ItemFacet)`)
  — map `ItemKind` → `ItemFacet` in one line and group by category.
- **Search page** (was missing) is now `creative_tabs::search_all(query, name_of)`
  for a global search and `tab_items_matching(tab, query, name_of)` within a tab.
  Wire a search box that calls these; `name_of` is your existing
  `ItemKind::display_name`/block name fn.

Net: the menu should show block tabs (creative_tabs) + item tabs (item_category)
+ a search box (creative_tabs::search_all). Food stays empty until `ItemKind`
grows a food variant, at which point set `item_category`/`creative_tabs`' food
flag for those items.

### D. Buoyancy works but can't swim DOWN at various sizes
Root cause: `phys::fluid_forces::buoyant_accel` returns an **acceleration**
(mass/size-independent). If the swim-down control is applied as a **force** (then
÷ mass in the integrator), a large/heavy body gets a tiny swim accel while
buoyancy stays full-strength → it floats no matter how hard it paddles; a small
body gets a large accel and dives. That's the "various sizes" tell. **Fix:** make
swim control an acceleration too. New helper:

```rust
// v += swim_control_accel(v.y, target_dive_vy, swim_authority, submersion, dt) * dt;
```

`swim_control_accel(current_vy, target_vy, authority, submersion, dt)` drives the
body toward `target_vy` (negative to dive), capped by `authority` and scaled by
`submersion`, mass-independent — so swim-down behaves identically at every body
size. Replace the force-based swim impulse in `step_character`'s in-fluid branch
with this. (If sizes intentionally differ, scale `authority` by size *explicitly*
rather than letting the ÷mass do it implicitly.)

### E. Water still doesn't flow into caves under/beside oceans
Two parts:
1. **Flow rule (REVERTED):** a bidirectional `flow_step` lateral pass was tried
   but reverted — letting empty cells pull water spread thin 1-unit films, and
   since `emit_water_surface` emits a flat quad per water cell, that showed up
   in-game as stray "broken planes". `fluid.rs` is back to the pristine push-only
   rule. The flow CA change had little benefit for this bug anyway (see below).
2. **Ocean seeding (handoff):** oceans are a *static* sea-level fill, not part of
   `FluidField`, so caves connected to the ocean never receive water even with a
   correct flow rule. **Wire:** at chunk finalize / water settle, flood-fill air
   cells that are (a) below sea level and (b) 6-connected to an ocean/water cell
   → water. A bounded BFS from ocean-adjacent air cells is enough; cap it per
   chunk and let the `flow_step` equalization (now bidirectional) smooth the
   result across chunk seams over subsequent ticks. This fills both *underneath*
   (via the existing gravity settle) and *sideways* (via the fixed lateral rule)
   caves.

---

## F. "Broken planes are back" — flat colored triangles fanning from a point (r604.2)

**This is the foliage sway feature, not water.** Symptom: flat, solid-colored
triangles that start at one point in a chunk and fan out in all directions.
Cause: the r594 per-vertex sway wiring tags foliage tip vertices `Sway` and
`chunk.vert` displaces them; a broken/over-large displacement flings the tips
while the `Anchor` bases stay put, so each plant's quads become a fan from its
base. (My first guess — water `flow_step` — was wrong and has been reverted.)

**Applied fix (CPU kill-switch):** `sim::foliage_sway::FOLIAGE_SWAY_ENABLED =
false`. With it off:
- `block_sway_eligible()` returns `false` (block-face/leaf path emits `None`),
- `push_quad_foliage`'s inline tag closure emits `0` (subvoxel/plant path),
so **every** foliage vertex is untagged and `PackedVertex::from_world` packs it
exactly like solid geometry (bits 30-31 untouched — `pack_tag_into_pos` is never
called). No vertex can be displaced, so the fans cannot occur. Foliage renders
static (the sway animation is off) but correct.

**To re-enable sway later (root-cause the shader, then flip the flag back on):**
1. In `chunk.vert`, find the sway offset applied to vertices whose unpacked tag
   `round(in_pos_unorm.w * 3)` is `Sway`. Verify the displacement is **clamped**
   (e.g. to a fraction of a block) and **NaN/inf-guarded**, and that the strength
   input (`sun_color.a` per r592, F3 slider) is bounded — an unbounded or
   uninitialized strength is the most likely fling source.
2. Confirm `PackedVertex::pack_pos` never lets a foliage local coordinate exceed
   10 bits per axis; if a subvoxel vertex can land >1023 in a channel, its high
   bits collide with the tag bits and `pack_tag_into_pos`'s clear-then-set would
   move the vertex. (Belt-and-suspenders: mask each channel to `& 0x3FF` before
   OR-ing the tag.)
3. Set `FOLIAGE_SWAY_ENABLED = true` and re-test in-game: foliage should sway
   with no fanning.

If the planes persist even with the flag `false`, the artifact is NOT the sway
path — look next at `emit_subvoxel_grid` face emission and `pack_pos` range.
