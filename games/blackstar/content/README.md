# Content system — multi-relational, data-driven

This folder is the game's **content layer**. Instead of every troop / map / mission /
weapon being hardcoded in C tables, they're described here as **JSON relations** that
reference each other by id, and a compiler resolves + validates the whole graph.

The point: **rearrange and vary content without touching C or the art.** Author a new
troop by pointing at an existing chassis + signature + weapon + abilities; the
compiler checks every reference resolves and every mesh exists.

## The graph

```
part_groups.json ── exposes the IQM meshes as named groups (head, cannon, backpiece…)
        ▲   ▲
chassis │   │ signatures ── backpiece index → part_groups.backpiece
weapons │   │ (fire_modes → part_groups.cannon;  addons → fire_modes)
abilities   │
        └─ troops.json ── chassis + signature + fire_mode + abilities  (the rearrange layer)
                              ▲
maps.json ── modes.json ── missions.json ── map + objective + champion(troop)
animations.json ── gait / transform / idle / lean params
```

## Files

| file | what it defines | key relations |
|------|-----------------|---------------|
| `part_groups.json` | IQM meshes grouped by role, with slot + origin | meshes must exist on disk |
| `chassis.json` | 7 body frames (proportions, fold, detail) | mirrors `chassis.c` |
| `signatures.json` | 10 signature identities | `backpiece` → part_groups |
| `weapons.json` | 10 fire modes + 16 addons | `cannon` → part_groups; addon `converts_to` → fire mode |
| `abilities.json` | 9 abilities (perk vs active) | — |
| `troops.json` | player classes, enemies, **variants** | chassis + signature + fire_mode + abilities |
| `maps.json` | 14 map recipes | `energon`, `structures`, atmo |
| `modes.json` | 4 skirmish/activity modes | rules flags |
| `missions.json` | the 8-mission campaign chain | map + objective + champion troop |
| `animations.json` | gait / transform / idle / lean params | drives part-group slots |

## Build / validate

```sh
python3 tools/content_build.py         # resolve + validate, writes content/_manifest.json
python3 tools/test_content.py          # self-test (valid passes, broken refs caught)
```

`content_validate` also runs as part of `ctest`, so a dangling reference or a mesh
that isn't on disk **fails the build**. The compiler additionally cross-checks every
`enum` field against the C headers, so if the JSON and the C enums drift apart you get
a warning naming the exact symbol (this already caught `EX_SIG_WING` →
`EX_SIG_WING_CANNON`).

`_manifest.json` is the **resolved** graph — each troop flattened to its concrete
`cannon_mesh` / `backpiece_mesh` — ready for a future runtime loader or a codegen step
that emits the C tables directly from JSON.

## Adding content (the whole point)

**A new troop variant** — no art, no C:
```json
"ashfall_lancer": { "kind": "variant", "of": "blackstar_raider",
  "chassis": "raptor", "signature": "solar", "fire_mode": "flak",
  "abilities": ["dash","overclock"], "tint": [255,120,60] }
```
Run the compiler; if `raptor`/`solar`/`flak`/`dash`/`overclock` all exist, it resolves.

**A new mission** — add an entry to `missions.json` with a `map` + `objective` +
optional `champion`; keep `order` a clean 1..N.

## Design directions seeded here

- **Art direction** — `part_groups` is the seam for restyling: swap the mesh list
  under `backpiece` or `cannon` and every troop that indexes it re-skins at once.
  New props (`deco_slab/pipe/antenna/crystal`) and structures (`arch/column/dome`)
  are exposed as groups for map dressing.
- **Campaign direction** — `missions.json` makes the arc replannable as data: the
  current chain runs the solar system inward (Pluto → outer rim → Mars → beach →
  vault → foundry → trench → Pacific). New beats are inserted here first, then
  mirrored into `campaign.c`.
- **Weapon direction** — `fire_modes` × `addons` is a matrix: any `*_core` addon
  re-modes any form, so "directions" are combinations (e.g. suppress-core + solar
  signature = a long-range sun-lance). New modes get one entry + one cannon mesh.
- **Vehicle direction** — troops carry an alt-mode intent via chassis + fire_mode;
  the variant troops here (ashfall lancer, hoarfrost sentinel, magma breaker) are
  starting points for faction rosters built entirely by recombination.
