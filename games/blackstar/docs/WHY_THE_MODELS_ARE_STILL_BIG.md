# Why 490MB of baked models are still in the tree

Asked why large, recreatable things are still hanging around when the
parts/web work got "all the models down to 34MB or something".

**The compact representation is real and it is here. It covers a quarter of the
corpus.** That is the whole answer, and it is a coverage gap rather than a
packaging oversight.

## What the compact form is

`catalog/*/partlib.json` — **30MB across five catalogs**, which is the ~34MB
figure. Each holds three things:

| key | vehiclecat |
|---|---|
| `parts` | 5,873 entries — signature, tri count, size, which model it came `from` |
| `placements` | 178 named vehicles, as arrangements of those parts |
| `rules` | roadwheel, tread_link, wheel, block, strut, underside_greeble, disc, turret_greeble |

`game-model-tools/python/derive_web.py` is the motion half: it recovers a
character's transformation by matching components across both of its models and
solving each pair with Kabsch, so *"the motion it undergoes is not something to
invent — it is the rigid transform that carries one onto the other"*. Output is
`docs/brawl_web.json`, **3KB** for a full per-part motion list.

Note `partlib.json`'s `parts[].mesh` is a **reference** (`part_wheel_2c6315c6`),
not inline geometry. The 30MB is metadata; the geometry it points at lives in
`catalog/*/meshes/`.

## Why the baked models remain

`engine_src/data` holds 1,133 `.iqm`, of which **1,012 are `imported_*` at
512MB**. They are not dead weight: `game.c` includes `imported_models.h` and
loads them into `g_imported_tpl[EX_N_IMPORTED]` — the player selects among them.

Measured against the placement lists, normalising names:

```
imported models:      1012   (512 MB)
decomposed to parts:   264   (198 MB)
not yet decomposed:    748   (314 MB)
```

The correspondence is exact where it exists — `tank--patton- aka scifitank
patton` in `placements` is `imported_tank_patton_aka_scifitank_patton.iqm` on
disk — which is what proves the parts were extracted *from* this corpus rather
than authored beside it.

So **314MB is genuinely not yet derivable**, and 198MB is derivable in principle
but still shipped baked because nothing regenerates it at build time.

## What would actually shrink this

1. **Extend the decomposition to the remaining 748 models.** This is the real
   lever and the only one that reaches 314MB. It is the existing pipeline run
   further, not a new idea.
2. **Stop shipping the 264 that already decompose.** Generate them from
   `placements` at build time and `.gitignore` them: −198MB, available now,
   costs a build step.
3. **The catalogs' `.iqm`, the same way.** Measured on one part: `.obj` packs to
   4,918 bytes against `.iqm`'s 8,998 — text compresses, packed floats do not.
   Ship the `.obj`, build the `.iqm`. Roughly halves 32MB of catalog mesh.

## The caution that belongs with this

A previous attempt at (3) deleted the catalog `.iqm` without checking, and both
assumptions behind it were wrong: the engine loads `.iqm` at runtime with no
`.obj` path, and `.obj` is the *smaller* of the two in the repo. Regeneration
saved it, but only because `tools/obj_to_iqm.py` exists and works.

The rule that follows: **derived-file removal is safe only alongside a build
step that regenerates it, proven to run.** Deleting first and generating later
is how a repo ends up unable to build itself.
