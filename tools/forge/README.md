# forge — generated vehicles and bots for blackstar-rs

Original parametric assets, written to replace the imported library whose
filenames put 57% of it under a rights holder.

    python3 export.py ../../games/blackstar-rs/assets/forge   # 20 GLB
    python3 -c "import web, json; print(web.index_only('...catalogue.forge.bin'))"

## What is here

| file | what |
|---|---|
| `forge.py` | primitives matched to `blackstar-rs/src/meshgen.rs` — `loft` with the same `(z, hw, floor, roof, roundness)` tuple, tube, sphere, torus, a real tyre, wheel arches, and the car detail pass |
| `fleet.py` | 13 vehicles across car / hoverer / flyer / spaceship, each stating the gameplay gap it fills |
| `bots.py` | the six parts `content/part_groups.json` says a bot is made of, plus the seven chassis from `content/chassis.json`, unmodified |
| `spines.py` | ten backpieces, one per entry in `content/signatures.json`, shaped so the silhouette states the stat trade |
| `recipes.py` | per-class fold recipes (which limb becomes which part) |
| `fold.py` | **demonstrator only — do not build on this.** See the note below. |
| `glb.py` / `export.py` | GLB writer with NAMED nodes |
| `web.py` | `FORGE\x01` — a parts web holding parameters instead of geometry |
| `look.py` | software rasteriser, so a model can be checked by eye |
| `tuning.py` | emits a `model_tuning.json` and checks it with blackstar's own rules |

## Named nodes are the point

`blackstar-rs/src/model.rs` spends most of 1500 lines on `find_orientation`,
`find_upright`, `wheel_score`, the geometric wheel split and `partdetect`,
and says why:

> Named nodes solve it when they exist; these do not have them, so find the
> wheels geometrically instead.

A generated model HAS them. The axle is the node translation because the
generator put it there; the hip joint likewise; which way is up is known.
Wheels are named in `detect_tag`'s existing `-FL/-FR/-BL/-BR` convention so
they tag correctly with no change to `model.rs`. Every other role
(`torso`, `thigh_l`, `shoulder_r`, `wing_l_0`) falls through to `inherited`,
i.e. current behaviour.

Verified: all 13 vehicles reconstruct to the exact span and vertex count of
the in-memory build, by replaying `collect_node`'s own `world = parent *
local` maths.

## fold.py is a demonstrator, not the fold

It interpolates two poses with a smoothstep, which is precisely what
`actuator.rs` already calls out:

> a `cyber::Muscle` that only responds to torque is decoration, because the
> fold is still a scripted lerp underneath it.

The real fold is `skeleton::fold_pack` -> `FoldStep{rot, pivot, shift}` driven
by `actuator`. What survives from `fold.py` is the idea that both poses are
DERIVED from the nine chassis numbers rather than authored, so a
transformation costs no third artifact. Forge knows every hinge exactly and
should emit `FoldStep` directly rather than poses for something else to
interpolate.

## Known rough edges

* Car sills still read as protruding horizontal planes; the glasshouse and
  splitter are fixed, the sills are not.
* Backpieces want roughly 1.3x their current scale to carry at play distance.
* The bot chest reads as a slab from the front — wants a centre crease.
* `sentinel -> SPAN` is an out-of-range pairing (a 6.62 m wing from a 2.2 m
  bot); `fit_to` clamps and reports rather than chasing it.
