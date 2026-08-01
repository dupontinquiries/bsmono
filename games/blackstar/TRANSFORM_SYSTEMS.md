# Why nothing transforms: five systems, none of them fed

Testing the browser showed flyers and others cycling fine and never
transforming. The cause is not a missing feature — it is **five** transform
systems, each waiting on data the corpus does not have.

| system | where | needs | has |
|---|---|---|---|
| `transform.h/.c` state machine | `exengine/` | two meshes per character (robot + vehicle) | assigned only for the playable avatar |
| crush-spin-swap | `game.c` `imp_transform_step` | `g_imp_pair` partner | **3 stems of 581** |
| per-part morph (atag 20) | `game.c` ~7145 | `g_tmodels` entries | **table is empty** |
| `g_morph` disperse/reassemble | `game.c` | procedural bot parts | works, but is an *effect*, not a transform |
| avatar fold blend | `game.c` ~6690 | procedural part poses | works, and lerps **scale** (see `TRANSFORM_OVERHAUL.md`) |

## The measurement that settles it

`tools/analyze_models.py` now pairs models by stripping form words
(`tank`/`vehicle`/`rb` vs `robot`/`bot`/`lmb`) and matching the remaining stem.
Unmarked names count as robot, because most characters export as `<name>` and
`<name>_vehicle` — only the alt mode gets labelled, and requiring an explicit
robot word found 2 partners in 400 models.

Result over the whole list: **3 stems have both forms.**

* `brawl_transformers_human_alliance`
* `fall_of_cybertron_bumble_bee`
* `fall_of_cybertron_optimus_prime`

75 stems are vehicle-only — and most of those are ordinary cars (Alfa Romeo,
Barracuda) that never had a robot form. The rest are robots with no vehicle.

**So a swap-to-partner transform cannot scale past three characters**, however
well it is implemented. That is a property of the asset library, not of the code.

## The one path that can work

A transform derived from a **single mesh**, needing no partner, no parts table,
and no second export. Everything for it now exists:

* `model_analysis.h` gives each model its measured up axis, mirror axis, pair
  count and paired fraction — 74–85% on Brawl, the t4b mech and scorponok.
* `anatomy::body_plan` finds the limb pairs; `joints::find` finds where they
  meet; `joints::reverse_plan` orders the fold parent-before-child.
* `motion::schedule` sequences it so parts move in turn rather than all at once,
  and `motion::axis_angle` hinges about any axis rather than one plane.

What is missing is the **wire**: the engine folds `ex_model_t` by position,
rotation and scale, which cannot bend a single mesh. It needs either per-region
vertex transforms on the CPU or a small set of hinge uniforms in `forward.glsl`.
That is the next piece of engine work, and it is the only one that makes the
other 1,009 models transform.

## What to remove, and what not to

The empty `g_tmodels` path is already inert — `tmodel_for()` returns null, so
every call falls through. It costs nothing at runtime and removing it is a
cleanup, not a fix.

`g_morph` is a **disperse-and-reassemble effect** on the built bot, not a
transform, and it works. It should be renamed rather than deleted, because its
current name invites exactly the confusion of thinking it is the transform.

The avatar fold blend works and is the one to overhaul first — it is the only
system that animates anything today, and `TRANSFORM_OVERHAUL.md` has the four
specific defects.
