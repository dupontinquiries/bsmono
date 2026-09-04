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

**UPDATE (c15unified session): the approach is now PROVEN, the engine wire is
not.** `rust/foldview` (built after this doc, per git history — 2026-08-17 vs
this file's 2026-08-01) is exactly this path: `motion::schedule` +
`motion::axis_angle` folding a single mesh via anatomy detection, no partner
mesh, no parts table. It's real — rigid-motion-verified per sampled timeline
point (`model_verify::verify_pair`), tested against actual imported vehicle
geometry (tank/flyer/racer), not just the procedural bot parts. So the
question this section poses ("can a single-mesh anatomy-based fold actually
look right") has an answer, and the answer is yes.

But foldview is a Rust **viewer/harness**, not the C engine — its own README
calls it "the harness that work renders into," a tool for iterating on the
approach with pixels, not a replacement for `game.c`/`forward.glsl`. Nothing
in `exengine/` or `game.c` calls into foldview or vice versa; they don't
share a build, a binary, or a runtime. So the engine-side gap this section
describes — folding `ex_model_t` still only supports position/rotation/scale,
still can't bend a single mesh — is very likely still exactly as open as when
this was written. What changed is the risk of doing that work: it's now
"port a proven approach" rather than "build and validate a new one blind."
Did not attempt the port itself — real C-engine + GLSL work, unverifiable in
this sandbox (no display, and this session's toolchain is Rust, not the C
engine's own build).

**UPDATE, same session, right after writing the above:** got curious whether
"unverifiable... not the C engine's own build" was actually true, or just
assumed. It wasn't true — `engine_src/`'s own Makefile builds with plain
`make` once `gcc`/`libsdl2-dev`/`libgl1-mesa-dev` are installed. It had never
actually linked in this bundle (60 `.c` files, including `transform.c` and
`robot_parts.c` themselves, were missing from the Makefile's hardcoded
object list — see the engine-build commit). Fixed that, so the C engine now
builds and runs up to SDL/GL window creation. Still can't get a window under
this sandbox's Xvfb (GLX visual mismatch, a real ceiling — see that commit
for what was tried), so the actual port work above is still unverifiable
here, but "the C engine's own build" is no longer a wall between this repo
and testing it — just this sandbox's lack of a real display.

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
