# Alley scene — reference imagery (for building out the existing `Alley` theme)

These three images are the visual brief for the **`Alley`** theme. The scene already
exists: `crates/logscope-egui/src/scene3d.rs` → `fn alley()` (dispatched from `build()`
on `"Alley" => alley()`). It currently has: damp/wet asphalt terrain, sky-mirroring
puddles, a procedural crack web, faded centre-line dashes, tar-seam patches, manholes,
oil stains, potholes (`ground_wear`), red-brick paving bands, yellow speedbumps, and
clotheslines with hanging laundry. The job is to **build it out toward the mockup**, not
start from scratch.

These are reference only — **organize now, build later.** No build changes were made when
these were filed.

## The files

### `mockup-alley-garden-ui.png` — the design TARGET (authoritative look)
The "MIND PALACE · ALLEY DISTRICT" / **Alley Garden** concept render. This is what the
finished theme should evoke:
- **Lush green corridor**: ivy/vine-draped walls and fences, overgrown planters, leaves
  everywhere, a tree canopy filtering warm light down the lane.
- **Wet cobble/asphalt** receding to a vanishing point, soft reflections, golden-hour
  shafts (the "non-uniform storytelling light" from the session brief — needs shader
  accent lights, not just emissive geometry).
- **Wooden hanging signboards** = the conversation-graph nodes (ROOT PROMPT → HYPOTHESES →
  EVIDENCE → CORRELATED/INSIGHTS → CONCLUSION), suspended on chains. Maps onto our node
  layer; the alley node style should read as hung wooden plaques.
- **Cats** lounging on benches/planters (the backlog's "rabbits + cats darting" idea, but
  here they're calm residents).
- **Yellow wooden benches + picnic tables**, small chairs, potted plants as props.

### `ref-photo-1-green-alley-sunny.png` — real "ruelle verte", bright/dry
A Montreal green-alley on a bright day. Contributes:
- **Yellow painted edge-bands** running down both sides of the lane (distinct from the
  centre-line — these are continuous shoulder stripes).
- **Concrete planter boxes** lining the edges, packed with shrubs/greenery.
- **Graffiti murals** on the brick end-walls (right side).
- **Overhead wires + strung decorations** across the lane (we already have clotheslines;
  this confirms the look — add bunting/colored hangs).
- **"MAXIMUM 10" speed sign** on a post — a candidate readable-sign prop (needs the glyph
  texturing that's still deferred).
- **Chalk ground drawings** (a compass/sun, a crowned figure) — decal-style flat ground art.
- Receding lane framed by brick triplexes + trees against an open sky.

### `ref-photo-2-wet-alley-stormy.png` — real green-alley, post-rain/overcast
The wet, moody variant. Contributes:
- **Wet ground with a long snaking puddle** down the lane centre, genuinely reflective —
  reference for stronger puddle reflection + the wet-grade we already tint toward.
- **Dense ivy/vine hedge wall** (left) — a green boundary instead of bare brick.
- **Yellow wooden bench + small teal chair** prop cluster (right) — matches the mockup's
  benches; a concrete prop to model.
- **Ivy-covered wooden fence + black garage door** (right) — vertical boundary variety.
- **Scattered twigs/leaf debris** across the asphalt — ground-litter scatter.
- **Storm-grey sky** — supports a "stormy/overcast" lighting variant of the theme.

## How they combine into the build-out (notes for the later turn)
- **Boundaries**: replace/clad the generic enclosure with *ivy walls*, *ivy-on-wood
  fences*, *brick triplex faces with graffiti*, and *garage doors* — variety per side,
  per the photos. (Alley currently skips the standard `enclose()` shell per the dispatch
  table; confirm and build dedicated walls.)
- **Edge treatment**: add the continuous **yellow shoulder bands** + **concrete planters
  with greenery** along both edges (photo 1).
- **Props**: **yellow benches / picnic tables / teal chairs** (both photo 2 + mockup),
  **potted plants**, **resident cats** (mockup).
- **Ground**: keep the wet grade; push **puddle reflectivity**; add **leaf/twig litter
  scatter** and **flat chalk decals** (photo 1). Brightness of wet/reflective surfaces is
  a **GPU-only judgement** — llvmpipe will wash it; defer absolute brightness to the user.
- **Light**: the mockup's **golden shafts through canopy** is the signature; that's the
  deferred "shader accent light" work, not emissive geometry.
- **Nodes**: alley node style → **hung wooden signboard plaques on chains** to match the
  mockup's graph cards.
- **Two moods** are supported by the refs: bright/dry (photo 1) and wet/stormy (photo 2) —
  candidate `LOGSCOPE_HOUR` / weather variants.
