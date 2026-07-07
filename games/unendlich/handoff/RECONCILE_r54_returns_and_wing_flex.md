# RECONCILE — r54 worker returns + verlet wing-membrane (merged into r57)

## r54 returns adopted (7 of 9)
All pure, all tests green in-tree (re-run, not just the workers' claims):

| WID                       | module                  | tests | notes |
|---------------------------|-------------------------|-------|-------|
| r54-01-plant-root-system  | `sim::plant_root_system`| 6     | root reach/uptake/anchor; composes plant_growth_env |
| r54-02-plant-propagation  | `sim::plant_propagation`| 6     | seed dispersal + germination; seeded splitmix |
| r54-04-verlet-strand      | `sim::verlet_strand`    | 10    | pinned Verlet point-chain — **drives wing flex below** |
| r54-05-aabb-resolve       | `sim::aabb_resolve`     | 8     | box overlap + MTV + velocity resolve |
| r54-06-pcf-shadow         | `sim::pcf_shadow`       | 15    | PCF kernel + slope-scaled bias (pure math) |
| r54-07-dither-ordered     | `sim::dither_ordered`   | 6     | Bayer dither + dithered quantize |
| r54-08-buoyancy           | `sim::buoyancy`         | 12    | Archimedes float/sink + fluid drag |

No collisions; each is a new file + one guarded `pub mod` append (deduped).
Extracted from the `CHANGES-*.patch` new-file hunks; `mod.rs` appended manually
(patch `mod.rs` hunks ignored — their context didn't match our tail).

### Not returned yet
`r54-03-plant-canopy-light`, `r54-09-aerial-perspective` — still out. Merge when they land.

### Design docs received (parked, NOT code)
`builder_genome_revised.docx`, `dragon_energy_lifestyle.docx`, `gamemode_trait_system.docx`
arrived in the same bundle. These look like design specs, not worker deliverables —
parked for review; flag if you want them turned into briefs/modules.

## verlet wing-membrane — `sim::dragon_wing_flex` (NEW, built here, 6 tests)
Wires `verlet_strand` into the dragon so the wing membrane has soft trailing motion
instead of rigid plates. Stateful (Verlet keeps prev positions), so it lives on the
creature, not in stateless `skin`.

- `WingFlex::new(span, n)` — one strand per wing (n samples root→tip).
- `WingFlex::step(l:(root,tip), r:(root,tip), dt, &FlexParams) -> (offs_l, offs_r)`
  — both wing ends pinned to the rigid wing bone (from the skinned model); mid-span
  membrane lags under inertia + droop, pulled toward the rigid line by `follow01`.
  Returns per-span offsets (endpoints ≈0, lag peaks mid-span).
- `offset_at(offs, span_frac01)` — sample the offset for a wing box at its span fraction.
- `FlexParams::default()` = iters 6, damping 0.15, droop 3.0, follow01 0.35, max_offset 0.6.

### Wiring (creature + entity — small, do on the engine side)
1. Add `wing_flex: dragon_wing_flex::WingFlex` to the dragon creature (build with
   `WingFlex::new(wing_span_world, 6)` at spawn; `span` from the forge model's wing
   extent in world units).
2. In the per-tick creature update, after advancing the rig, step it with the rigid
   wing endpoints. Get those from the skinned model: the wing **root** ≈ the box at
   `J_WING_*` nearest the body, the **tip** ≈ the outermost `J_WING_*` box. In the
   model→engine axis map used by `forge_to_vertices` (model z→fwd X, y→up, x→right Z):
   ```
   let (ol, or_) = creature.wing_flex.step((root_l, tip_l), (root_r, tip_r), dt, &FlexParams::default());
   creature.wing_off_l = ol; creature.wing_off_r = or_;   // cache for the frame
   ```
3. In `world::entity::forge_to_vertices`, for each wing box add the sampled offset
   before emitting it:
   ```
   if matches!(b.joint, J_WING_L | J_WING_R) {
       let offs = if b.joint == J_WING_L { &creature.wing_off_l } else { &creature.wing_off_r };
       let frac = wing_span_fraction(b.center, model);   // 0 at root .. 1 at tip (lateral)
       let d = dragon_wing_flex::offset_at(offs, frac);
       // d is in model space; map to engine local like the box center before push
   }
   ```
   `wing_span_fraction` = normalised |b.center.x − wing_root.x| over the wing half-span.

Net: leading edge + tip stay on the bone; the membrane between them ripples/lags a
fast down-beat and settles when the wing holds — soft wings, no extra primitives.
