# UNWIRING THE DEMO MAPS

The eighteen skydribble map styles are deletable. This is the survey of what
comes out with them and what must not, done before anything is removed —
because the last time I assumed something was load-bearing I was wrong about
it in the other direction.

## The correction that prompted this

`gravity.rs` ends by saying the engine's well formula cannot be fixed because
`world.rs`'s constants are "load-bearing for eighteen grounded maps where the
flat near-field is what makes a carved deck driveable." That was true and it
is now irrelevant: **the maps are going.** With them goes the only reason to
keep a gravity model that is wrong in four separate ways.

## The good news, measured

**The campaign stack is already independent of `world.rs`.** Not loosely — it
does not import it at all:

| module | imports |
|---|---|
| `terrain` | *(nothing)* |
| `survival` | `sun` |
| `foothold` | `approach`, `survival` |
| `procgen` | `foothold`, `raid`, `sun`, `survival` |
| `raid` | `damage`, `foothold`, `hydro`, `loadout`, `loot`, `sun`, `survival`, `voyage` |
| `mapsim` | `depth`, `patrol`, `procgen`, `scale`, `sun`, `survival`, `terrain` |

`procgen::World` is a **different type** from `world::World` and shares no
code with it. `Weather` lives in `survival.rs`. `Material` does not appear in
`world.rs` even once.

So the three things worth keeping are already outside the thing being deleted.

## What `world.rs` actually is

4,835 lines, and **44% of it (2,156 lines, the `focal` match arms at lines
813–2969) is per-map content** — the eighteen builders that place rings,
pads, arenas, the Kuiper field. That is the deletable half.

The other half is the skydribble *sandbox*: SDF evaluation, the shape grid,
portals, tiles, currents, movers, the ball and goal machinery, and the
gravity nodes. Not campaign infrastructure — sandbox infrastructure.

## Who still holds a reference

`world::World` is imported by eight modules: `main`, `sim`, `physics`, `hud`,
`fx`, `camera`, `bots`, `mapsim`. Those are the renderer and the sandbox loop.
`sim.rs` alone has 86 references and is the largest single consumer.

**`mapsim` is the one to check first.** It imports both `world::World` and the
campaign stack, which makes it the seam — and the only place the two worlds
currently meet.

## Order of work

1. **Take the gravity fix now.** With the maps deletable, the shell-theorem
   model in `gravity.rs` can simply become the model: surface pull
   proportional to radius, `1/d²` above the surface, linear to zero below it,
   no ambient floor on a space map, and the field summed over wells rather
   than taken from the nearest. `--gravity` has all four errors measured.
2. **Fix the tube profile while you are there.** It is inverted — see below.
3. **Delete the focal arms** for the seventeen styles that are not Kuiper, and
   the style table rows with them. That is the 2,156 lines and it removes the
   only callers of a lot of the sandbox scaffolding, which will then surface
   as dead code and can be taken out in a second pass rather than guessed at.
4. **Keep Kuiper** until the space map replaces it, then delete its arm too.
5. **`mapsim` last**, once nothing else bridges the two worlds.

## THE TUBES

They are 147 mentions spread from line 25 to line 4635. That is not a system,
it is a thread woven through everything, which is a large part of why they
have never felt right.

But there is also a specific bug, and it is worth fixing rather than
rewriting blind: **the gravity profile is inverted.**

Spin gravity is `a = ω²r` — **zero on the axis, maximum at the wall.** The
engine computes `0.50 + 0.95·(1 − d/(r/1.45))`, which is **maximum on the axis
and weakest at the wall.**

The comment history in `world.rs` records both symptoms and neither diagnosis:

> a car flung into a wide pipe simply flew up the middle at constant speed,
> never touching anything

That is *correct*. You are weightless on the axis of a spinning habitat. It
was read as a failure, and the fix made the axis the strongest point — after
which:

> at 1.7x it pinned

Of course it did. It was flinging you outward hardest from the one place that
should be still.

`gravity::true_tube` is the right profile, and it is the same shape as the
interior of a solid sphere for the same reason: nothing outside your radius
contributes. Three tests pin it.

A rewrite should start from that, and from putting the tube in one place
instead of a hundred and forty-seven.

## And `sim.rs` turns out to be the easy case

I flagged its reference count as the thing that decides whether this is
mechanical or a rewrite. Measured: **115 references, none of them in a
`#[cfg(test)]` region** — and `sim.rs` is not the game loop. It is the
headless bot-AI harness behind `--sim`, "the real physics at a fixed 120 Hz
with no window, no GPU, no audio", which exists so the striker's air-dribble
machine can be tuned by numbers instead of vibes.

So it is a *driver*, and it needs exactly one world to run in, not eighteen.
Point it at whatever survives and its 115 references cost nothing.

That reframes the whole job: the eight importers of `world::World` are the
renderer (`camera`, `fx`, `hud`, `bots`, `physics`, `main`), one headless
driver (`sim`), and one bridge (`mapsim`). None of them is campaign logic.

## What I still have not checked

Whether the sandbox scaffolding the focal arms use — tiles, portals,
currents, movers, the goal machinery — has any consumer outside those arms.
It probably does not, which would make the second pass much larger than the
first, but that is a guess and it should be a count.
