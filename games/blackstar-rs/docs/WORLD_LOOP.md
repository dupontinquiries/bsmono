# THE WORLD LOOP — a window that moves, and why the map must cradle

## The decision

**Minecraft's loop, Destiny's places.** The world is a plane addressed by
world coordinate; the game holds a WINDOW of it (the renderer's baked
tile, ±1300 m) that pages as you move; authored places are pinned to
coordinates on the plane and the space between them is generated,
dense near the places and never empty. Regions survive as content
granularity (a planet is a seed plus a list of sites), not as the loop.

The arithmetic that decided it: a car at 60 mph is 27 m/s and crosses
the whole tile in 97 seconds; this car is faster. A finite region is a
cage you hit inside two minutes whenever driving is the play — which is
the brief. Destiny's zones are small *for sparrows* because sparrows are
transit there. Forza, GTA, Mad Max — every game where driving is the
game — streams.

Why not just stream like Minecraft, outright: `world::World` is one
baked tile — a 221² height grid, a shape grid the shader ray-marches,
a heightmap and reflection cells baked to the GPU at load. Chunk
streaming needs every one of those chunk-local and incrementally
re-bakeable during play, and on a phone the per-chunk bake is the frame
you die in. So the tile stays a tile and becomes a window: rebuilt
whole, centred near the car, with everything window-local shifted by
the delta. That is `Game::page_window`, a tentative system: it hitches,
it prints how long it took, and the double-buffered version that
spreads the bakes over frames is the next step.

What makes a window possible at all is DETERMINISM BY COORDINATE.
`World::generate_at(seed, style, .., origin)` samples the terrain noise
at world coordinates; `district::layout(seed, extent, origin)` places
every feature by a hash of its lattice cell, so a spire at world
(1820, −540) is the same spire from any window that can see it. The
test `the_same_place_from_two_windows` pins that. The sandbox's other
styles scatter their furniture off a sequential rng and are therefore
a different map after a page — paging is district-only until they are
rewritten by cell.

## Speeds and windows

| | |
|---|---|
| tile half-extent | 1300 m |
| page trigger | 45% of it, 585 m from the origin |
| car at 60 mph | 27 m/s: 26 s from origin to trigger, 26 more to the edge |
| page lattice | 100 m, so a page back lands on the same ground |
| draw cull | 640 m — the far field past it is fog anyway |

The budget a background page has is the second 26 seconds. Whole-tile
generation on this box is well under that; the GPU bakes are the part to
spread.

## The cradle — why the district is a lattice and not a plan

The first district was a plan: a boulevard with a hangar capping one
end, kerbs down its length, spires in a picket line, and 900 m of
nothing past the last arch. A map like that cages, and the mechanism is
psychological before it is geometric:

- **Lynch, *The Image of the City*.** People navigate by paths, edges,
  districts, nodes and landmarks. A plan with ends turns its own last
  feature into an EDGE, and an edge that is not also more world reads as
  the end of the world. Landmarks (the spires, the hangar, an arch)
  must be visible from far enough that you are never lost — which is
  the other half of feeling held.
- **Appleton, prospect–refuge.** A body wants to see without being
  seen. A plaza is a refuge with prospect down the boulevard; an arch is
  a frame that promises the road continues; a raised place is prospect
  bought with exposure. A wall is refuge with no prospect: a cell.
- **Minecraft cradles by infinity**, Destiny by horizon and return (the
  Traveler over everything, the Cosmodrome wall you see and never
  reach, a "turn back" that is a countdown and not a fence), Smite by
  making the arena's walls TALLER world — jungle, cliffs, the Titans.
  All three agree: the boundary must be *more* of the place.
- **Repetition is the other cage.** `games/soldier/docs/map-analysis.md`
  gates its whole scenic score on a repetition penalty: a complete kerb
  line reads as a fence, a metronome of arches as a prefab. So a third
  of the posts are missing, arch spacing is a rhythm not a beat, spires
  are jittered and mixed with building masses, and density falls off
  with distance and never reaches zero — city, outskirts, a plain with
  the odd tower, forever (`district::density`, floor 0.12).

Built: the lattice, the falloff, cross-streets every third arch (a grid
to roam, not a corridor), the hangar beside the road, plazas as
refuges, the broken kerb, the pinned tests. Not built, in order: the
terrain SKIRT past the window (the mesh ends at ±1300 m and a mesh edge
is the most literal cage there is; `grid_h` already clamps so the
ground continues flat — the skirt is a coarse mesh of the same noise
out to the fog, no colliders); the HORIZON RING of Planetary-tier
silhouettes at ~540 m from the core (a ridge, the Ark half-buried, a
moon-scale sphere), inside the draw cull, forty percent into the fog;
and the diegetic boundary — past the window's soft edge the ship's
beacon fades and the sortie window burns, so you return rather than
bounce.

## Planets

A planet is `(seed, sites)`. Each site is a coordinate on the plane and
a style column (Earth warm, Pluto cold, Proto-Earth molten — the lore's
three-way contrast, C13 §2e); the district lattice is the Earth column's
generator and the other two want their own. The drop lands you at the
site's coordinate; the window opens there; the field musters at the
nearest plaza. Travel between sites on one planet is driving; between
planets it is the starmap and the drop, which are already the game's
transitions and already read as travel rather than loading.
