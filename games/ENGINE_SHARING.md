# skydribble and blackstar share an engine and no history

Written while importing two bundles that turned out to be the same package.

## What arrived

| bundle | package | version | commits | files | what it is |
|---|---|---|---|---|---|
| `skydribble-history-v1p141` | skydribble | 1.141.0 | 102 | 109 | the engine line |
| `skydribble-blackstar-idk-01` | skydribble | 1.117.0 | 119 | 1004 | the blackstar line |

The second is named "idk" and the uncertainty was warranted — its Cargo package
is `skydribble`, but its source contains `raid.rs`, `loot.rs`, `damage.rs`,
`patrol.rs`, `loadout.rs`, `procgen.rs`, `interior.rs` and `airlock.rs`. It is
**blackstar built on the skydribble engine**, and ~895 of its 1004 files are
`assets/bots/*` model data.

A third bundle, `logscope-c4-569-roster`, was **this port's own last output** —
its HEAD is identical to the monorepo HEAD. There is no logscope backend or
multi-device workspace work in it.

## Imported at new paths, deliberately

`games/skydribble` and `games/blackstar-rs`. **Not** `games/blackstar`: that
path already holds a CMake C++ project with an entirely different tree, and
writing over it would have destroyed an unrelated codebase that happens to share
a name.

Both were imported as subtree merges with history as a second parent rather than
squashed, because the reason to carry them in is that engine work can be traced,
and a flattened import makes "when did this change" unanswerable.

## The engine is shared and the histories are not

`git merge-base` finds **no common ancestor** between the two lines. They have
32 source files in common — including the three that matter:

| file | blackstar | skydribble |
|---|---:|---:|
| `src/sim.rs` | 7,268 | **9,617** |
| `src/render.rs` | 1,135 | **1,280** |
| `src/world.rs` | 4,666 | **4,689** |
| `src/shader.wgsl` | **1,315** | 1,133 |

So skydribble's line is ahead on simulation and rendering, and blackstar's is
ahead on the shader. Neither is a superset.

**This means "propagate skydribble's performance gains to blackstar" is a PORT,
not a merge.** With no shared ancestor git cannot tell an improvement from a
divergence, so a three-way merge would present every one of ~3,200 changed lines
as a conflict with no basis for resolving it. Anyone doing this should work file
by file against the newer `sim.rs`, and the honest first question is which of
those 2,349 extra lines are optimisations and which are gameplay the blackstar
line does not want.

## What plausibly transfers, by the user's own split

* **Performance work — likely.** It lives in `sim.rs` and `render.rs`, both
  shared, and physics/render cost is engine-level rather than game-level.
* **Map work — probably not directly**, but the planets, physics and props sit
  in `world.rs`, which is shared and nearly identical in size between the lines.
  That near-identity is the interesting signal: it suggests `world.rs` has NOT
  diverged much and is therefore the cheapest file to sync.
* **Car body work — unlikely to transfer**, and the file counts agree: the bot
  and car models are in `assets/`, which exists in blackstar's line and not in
  skydribble's at all.

## Not verified: neither builds here

`cargo check --bins` fails in `alsa-sys`'s build script, which needs
`libasound2-dev`. That is a host requirement rather than a code fault — the same
one recorded for `beatspace` in the manifest — so **the imports are unbuilt and
untested in this container**, and that should not be read as either working or
broken.
