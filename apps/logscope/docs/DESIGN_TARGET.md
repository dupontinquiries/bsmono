# Logscope spatial UI — design target

Distilled from the level-artist spatial brief (`physical-descriptions.txt`) and three synthesized UI mockups
(Archive forest, Mistfall gorge ×2). This is the north star; individual turns chip toward it.

## The one principle that governs everything

> The room tells the story before any object does. Every scene is organized around a **single, emotionally
> significant centerpiece**, and every path, light beam, curve, and atmospheric effect exists to reinforce it.

We were building *rooms full of objects*. The target is *one center of gravity per scene, with the
conversation nested in its focal void*. Composition first, objects second.

## The five scenes (centerpiece · atmosphere)

1. **Celestial Archive Dome** — a 55–70 m circular hall under a 12–15 m **oculus** that opens onto a marbled
   oil-in-water sky. Concentric rings (floor → tables → bookshelves → columns → arches → dome). A near-vertical
   sun cone full of dust. Gold radial floor inlays that *"almost resemble graph edges"* and guide the eye out
   from center.
2. **Stone Arch + Floating Islands** — 20% foreground (an eroded, moss-cracked granite arch on a cliff), 80%
   landscape: floating islands at staggered heights, waterfalls into cloud, layered fog, late-afternoon gold on
   one cliff face / cool blue on the other. Passing *through* the arch separates the human world from the
   impossible one.
3. **Moss Basin** — intimate 20×20 m. Rounded mossy stones, crystal-shallow water snaking between them with
   caustics, ground fog 15–40 cm that **moves around stones like a liquid**. Morning light, beauty from density.
4. **Cave Lake Tree** — silence. A 70–90 m cave, a near-perfect mirror-black lake, **one** mossy island with
   **one** ancient tree. Only 2–3 ceiling openings; their volumetric shafts intersect the tree *and nowhere
   else*. Mist, drips, reflections.
5. **Circular Grove Library** — nature encloses knowledge. A 40–50 m bookshelf-wall circle around a central
   18–22 m tree whose roots crack through the stone floor. A circular skylight whose sun rotates over the day.
   **Translucent leaves whose veins glow, and "words seem to dance across them as sunlight refracts through tiny
   films of moisture — never static, flickering with every breeze, as though the tree itself is quietly reading."**

## The conversation graph IS the scene's system (not a line through it)

The mockups float the node graph inside the centerpiece's void (inside the stone portal-ring; in the sunbeam
clearing). Per theme the nodes/text should *condense out of the medium*, three states — drift → aggregate →
resolve-into-text:

- **Mistfall** — mist twirls off the pond and condenses into rune-text hovering over the water; rune-rocks
  (cairns) glow cyan, suspended/linked by water rather than drawn edges.
- **Canopy / Grove** — sun through translucent leaves forges golden-hour text on the leaf; veins glow.
- **Archive** — gold floor inlays *are* the edges; nodes are warm reading-lamp pools.
- **Folding@home** — molecular/lattice nodes in a luminous chain.

## Chrome language (from the mockups)

The interactive chrome is **carved-material framing over translucent cores**, theme-adaptive, with glowing
rune accents — not flat panels, and not egui's default look:

- **Frames**: thick weathered stone (Mistfall) / dark walnut (Archive) borders with beveled relief that catches
  the scene's light; moss/grain in the material; rounded, ancient, Mayan-monument weight.
- **Cores**: dark, slightly translucent, so the scene reads faintly through — but text stays readable.
- **Accents**: small etched **runes** glow in the theme accent (cyan in Mistfall, amber/gold in Archive),
  breathing gently.
- **Buttons / tabs / input**: the same carved-slab vocabulary at smaller scale; the input is a carved trough.

## What our current renderer can hit now vs. the photoreal last-mile

Most of the *look* is reachable on the current glow renderer: **composition** (one centerpiece + graph in its
void), **atmosphere** (we have fog, bloom, god-rays, a roaming sun), and **carved-frame chrome** (2D relief +
grain + runes, driven by `theme::Skin`). These are all renderer-fork-independent — they port to anything.

The remaining ~20% — true mirror reflections (mineral pool, mirror lake), volumetric light shafts that pick out
only the tree, translucent leaf sub-surface with text inside, PBR materials with environment reflections, and
real anti-aliasing — needs a rendering upgrade (three-d on the same glow stack, or our own PBR passes). That's
the deferred decision; it's the last-mile polish, not the foundation.

## Build order

1. **Carved-frame chrome** (started) — panels, then buttons/tabs/input, all via `Skin`. ✓ panels
2. **Composition** — rebuild each scene around its single centerpiece; nest the graph in the focal void.
3. **Graph-as-system** — per-theme drift→aggregate→text nodes.
4. **Photoreal last-mile** — reflections, volumetric shafts, translucent leaves, PBR + AA (renderer decision).
