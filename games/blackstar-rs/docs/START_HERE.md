# START HERE

Orientation for picking this up cold. `DESIGN.md` is a chronological log of
decisions and mistakes — useful, ten thousand lines, and not where to begin.

## What this is

A Transformers open-world game in Rust, built on the bones of **skydribble**,
a Rocket-League-like that came before it. That inheritance explains most of
what looks odd:

- The player is a **car** with `half_h: 0.19`. In engine units that body
  stands in for a Transformer. See `--rescale`.
- **Eighteen map styles** named MEADOW, CATHEDRAL, FOUNDRY. They are
  skydribble's and they are deletable — `docs/UNWIRING.md`.
- Physics constants are Rocket League's, and the game is at roughly 1/22 real
  scale as a result.

There is also a **retired C engine** at `games/blackstar/`. Ten systems were
ported out of it; `docs/INHERITED_DESIGN.md` harvests the 42 it documented.

## The one thing to know

**`tools/fastcheck` is the fast edit-test loop.** Use it. `cargo build`
without it pulls in the renderer and audio and takes minutes.

It calls `cargo ailite`, which is **vendored at `<repo>/tools/cargo-ailite`**
— see `tools/README.md`. A fresh clone needs no setup; `fastcheck` finds it,
and falls back to plain `cargo` with a warning if it cannot.

The "3 s" in `fastcheck`'s own comments is the COMPILE side. Unfiltered, the
suite alone runs ~14 s at opt-0, so the real unfiltered cycle is about that.
`tools/fastcheck archive::` and friends get back to seconds.

Audio needs `libasound2-dev`; `--no-default-features` builds silent and is
what fastcheck uses.

## How the code is shaped

Almost every module has a **driver** — a `--flag` that runs it headlessly and
prints what it does. `skydribble --help` lists about thirty. They are the
fastest way to understand a system, and several exist because a thing could be
checked without a screen.

That is also the main criticism of the codebase: *verifiable without a screen*
correlated almost perfectly with *not in the game* for most of its life. The
recent work has been closing that, and `docs/PRIORITIES.md` is what remains.

Two stacks that do not talk to each other:

| | |
|---|---|
| **`world.rs`** | skydribble's sandbox — SDF, shapes, portals, the ball. 4,835 lines, 44% of it per-map content. |
| **`procgen` / `terrain` / `foothold` / `survival` / `raid`** | the campaign. `procgen::World` is a DIFFERENT TYPE from `world::World`. |

`mapsim` is the only place they meet.

## Where to start

`docs/PRIORITIES.md`, which is short and current. The headline: everything
underneath is built and tested, and the remaining list is almost entirely
**the renderer boundary** — things that resolve in simulation rather than
being something you do.

## Conventions worth keeping

- **Sizes are in occupant-heights, not metres.** Three bugs in one session
  came from a metre value that did not move when the thing it measured did.
  `--parity` scans for the general case.
- **A regression test written after the fix will pass and prove nothing.** Put
  the bug back and watch it fail. Two tests in this repo did not, and were
  found out by trying.
- **Guesses are labelled.** `--tune guesses` lists balance numbers with
  nothing behind them; several modules say in their doc comment which of their
  constants are invented.
- **Drivers print what they measured, not what they hope.** Where a number is
  a guess or a stopgap, it says so in the output.
