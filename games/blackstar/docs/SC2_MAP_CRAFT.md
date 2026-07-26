# StarCraft 2 map-making craft -> applied to our box-collider maps
## Research pass (web), with the transferable principles and where we used them

Sourced from Blizzard's "Mastering Mapmaking" (news.blizzard.com), the Envato
Tuts+ SC2 aesthetic-design tutorial, the Hive Workshop terrain guide, the
SC2Mapster wiki, and community tip threads (staredit, nibbits). SC2 terrain
tools (texture alpha-blending, doodads, foliage) don't map 1:1 onto our IQM
box-collider maps, but the DESIGN principles do.

## The transferable principles

1. **Height levels must READ.** Blizzard: use contrasting cliff colour values
   so players can tell they're approaching high or low ground. The single most
   important readability rule. -> our `tier_pad` helper: a raised deck with a
   contrasting glowing RIM around its top edge, so elevation reads at a glance.
2. **Lead the eye.** Blizzard: trails + doodads guide players to important
   areas. -> our existing `eye_trail` helper (a line of low lit markers), now
   used to point at landmarks on more maps.
3. **Reinforce important objects.** Nibbits: paint dark ground under key
   landmarks so they pop from the floor. -> `tier_pad` lays a darker ground
   decal ringing the base of the landmark.
4. **Height variation ~ texture variation in importance** (Hive). A flat field
   reads as dull. -> tier_pads give genuine cliff LEVELS the player fights over.
5. **Differentiate each area** with distinct texture/doodad style (multiple
   sources). -> already a core habit here (every map has its own palette);
   reinforced by giving landmarks map-specific rim/ground materials.
6. **Motivation over randomness** (Envato): every placement has a reason. ->
   tier_pads/trails are placed AT landmarks and along routes, never scattered.
7. **Performance-conscious detail** (Blizzard: particle overdraw). -> our
   dressing is all collide=False boxes reusing existing textures; no overdraw,
   and the collider score is verified neutral so gameplay pathing is unchanged.

## Applied this pass (Update 223)

- New `Builder.tier_pad(mat, cx, cz, hx, hz, h, rim, ground)`: a raised
  collidable deck + contrasting rim edge (readability) + dark ground decal
  (landmark reinforcement). Encodes principles 1, 3, 4.
- **enceladus** (was the flattest map, vert 0.45): two raised survey
  tier-pads with glowing riftcrystal rims as genuine high-ground over the ice,
  plus an eye_trail leading to the survey outpost. Collider score held (A);
  the win is readability, not the metric -- enceladus is thematically a flat
  ice field, so we didn't force fake verticality.
- **io**: the central forge-plinth (the map's key landmark) rebuilt as a
  readable tier_pad -- hazard-yellow rim so the raised forge reads, dark
  obsidian ground decal so it pops from the sulfur floor -- with two eye_trails
  leading inward to it. Score held at 81.8 A.

## Honest notes

- The map SCORER measures gameplay geometry (cover/spread/vert/open/scale),
  not aesthetic readability, so these readability wins are score-neutral by
  design -- the value is on-screen, a play-test/screenshot call like all
  render-side work.
- Not every flat map should be made vertical; forcing tall structures onto a
  thematically flat field (enceladus ice) fights its identity. The SC2 "distinct
  cliff levels" lesson is best spent on maps where high-ground fits the fiction.
- Next: extend tier_pad landmark treatment to other maps' key objects
  (forges, relays, boss plinths), and add texture-contrast rims to existing
  raised structures so every height change on every map reads consistently.

---

# Beyond SC2: Destiny / open-world combat-space design (Update 224)

The SC2 principles above are tuned for competitive RTS -- a fixed top-down
camera and clean read. A third-person open-world combat game needs MORE:
interactivity, varied doodads, and destructibles that break the "clean margin"
rules on purpose. Researched Destiny's approach (Bungie env-design coverage +
Max Nichols' ex-Bungie encounter-design writeup at namelessquality.com).

## The Destiny principles (and how they differ from SC2)

1. **Owning space / front-middle-back lines.** Destiny encounters are about
   the player pushing forward and CLAIMING space; enemies hold zones and fall
   back. (Bigger than a map helper -- a horde-AI change, noted for a future
   pass; our spawns currently scatter rather than zone.)
2. **Anchors project danger.** Big, tanky units that "own space" and are
   ALWAYS physically large so the player can see them. We already have this
   (APEX/champions). The map corollary: landmarks should be big enough to
   read as points-of-interest and anchor a fight.
3. **Direct player attention** via brightness, landmarks, paths, enemy
   movement. (Overlaps SC2 lead-the-eye; we have eye_trail + tier_pad.)
4. **Combat legibility.** The player must be able to READ the space and the
   fight. Clutter, overlapping props, and illegible density are the enemy.
   Well-SPACED, distinct cover is legible cover.
5. **Interactive / destructible geometry** (Destiny/Battlefield): cover you
   can alter, props you weave through -- immersion + emergent tactics, NOT
   just decoration.

## Applied this pass (Update 224)

- New `Builder.varied_scatter(ground, props, ...)`: where the old
  ground_cluster repeated ONE prop (monotony), this mixes a LIST of prop
  meshes at varied sizes/footprints/caps with enforced spacing (legibility --
  no overlap clumps), and makes a fraction SOLID destructible-style cover
  (interactivity -- you weave through it and use it). Encodes principles 4+5.
- **wastes** (the scrapyard): three varied_scatter clusters of mixed scrap
  (rust/wall/brushmetal/plate/rock), ~40% solid cover -- turning a flat
  rubble field into a varied, interactive, legible junk battlefield for the
  patrol experience. Score rose to 85.0 A; verified LEGIBLE via the new
  top-down map_view.py renderer (well-spaced, clear lanes, cover distinct).
- New `tools/map_view.py`: a top-down collider renderer (height-shaded) so map
  spatial layouts can be INSPECTED for the legibility/spacing principle
  instead of guessed -- the map analogue of the bot silhouette renderer.

## Honest notes + next

- The biggest Destiny idea -- owned-space front/middle/back firing zones --
  is a horde-AI change, not a map-builder change, so it's deliberately NOT in
  this pass. It's the highest-value future direction for encounter feel.
- Destructible-cover here means solid map colliders the player uses tactically;
  full shoot-to-destroy on themed-map props would need wiring the .col loader
  into the material/chunk destructibility system (which exists for the
  procedural arena). A clean follow-on.
- varied_scatter is currently applied to wastes; rolling it across the other
  large open maps (io sulfur field, derelict, driftfield) is the next sweep.

## Update 225 addendum -- where varied_scatter fits (a negative result)

Refined varied_scatter so SOLID props are eye-height cover (1.0-3.0 tall, real
footprint) vs low decorative debris. Then learned its LIMITS by testing it on
io: it regressed the score (81.8 A -> ~79 B) and was reverted.

Rule of thumb: **apply varied_scatter where a map has repeated-single-prop
monotony** (wastes: 16 identical scrap boxes -> mixed interactive field, clear
win). **Don't apply it to sparse maps whose height contrast comes from a few
tall structures** (io: forge + volcanoes + mountains); filling the gaps with
medium cover flattens the height variance the map relied on, and the original
low-spread rubble scored better on density/balance anyway.

The helper is a scalpel, not a paint-roller. Maps to consider next: only those
with an obvious repeated doodad loop, checked one at a time against map_score
+ map_view before committing.
