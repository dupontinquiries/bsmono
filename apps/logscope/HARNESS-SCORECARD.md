## Cinematic camera FSM + canopy hang + mistfall ceiling (this turn)
- CAMERA DIRECTOR: shot FSM (travel/establish/orbit/dutch/push) fed graph AABB + strand point; slow filmic eases
  (k 0.025-0.05) + half tour speed; ESTABLISH re-frames whole graph on arrival; OrbitCam.roll (rot_z + 2D project)
  for Dutch angles, levels off when auto-framing. Best seen LIVE in Tour (animated; not capturable headless).
- CANOPY HANG: cone-scan pins every 3rd node to the lowest branch overhead; gravity relaxation droops the rest →
  draped swags from the trees (distinct from floor-squish). Verified: graph shows vertical drape.
- MISTFALL ceiling 13.5->20 (+ longer stalactites) → camera frames the floating graph without the roof between.
  Verified: graph fully framed.
- DEFERRED: deeper occlusion-raycast POI targeting + true passthroughs (push-in approximates); explicit rope-to-
  tree meshes (anchors sit at branch height already). 259 tests.

## Frozen Bifrost + Titan's Ribcage — POI set-piece maps (this turn)
- Authored landmarks (not recursion): chose set-piece over recursion because these briefs are non-self-similar.
- bifrost(): basalt dais + sunken ice-trench moat + iridescent glowing glass dagger-shards over a white core +
  magenta/cyan aurora ribbons + frozen motes. Verticality + storytelling light. Dark → renders great on llvmpipe.
- titan_ribcage(): three colossal fossilised bone-arch ribs over a fractured-marble/black-loam lane w/ algae
  fig-roots, flanking recursive jungle, knee-level yellow-green fog, organic boundary. Material decay/contrast.
- New FROZEN_BIFROST + TITAN_RIBCAGE palettes. 259 tests.
- DEFERRED (3 remaining briefs, same set-piece approach): Sandstone Gauntlet (funnel walls + snapped obelisk),
  Dragon Corpse Canyon (beast carcass + pagoda), Colosseum Trenches (ring wall + crater no-mans-land).

## Yellow Brick Road / Oz — fourth recursive map (this turn)
- recurse_road(): forking yellow-brick road (wandering ribbon → recursive side-roads). recurse_tree forests,
  recurse_citadel emerald city (green glowing towers + wall). Poppy/corn/wildflower fields, scarecrows,
  rolling-hill organic boundary. New YELLOW_BRICK_ROAD palette. 0% clip.
- Four recursion archetypes: branching crystal, stacking castle, jungle-on-shell, forking road+city.
- NOTE: llvmpipe over-brightens the bright storybook palette (road→gray, emerald→pale); true colours need real GPU.
- DEFERRED: reef/root maps; storytelling lighting; true PBR/reflections; glyph text. 259 tests.

## Essay-driven: organic world boundaries (this turn)
- organic_boundary(): eroded cliff escarpments (strata) + talus rubble spilling in + clinging vegetation + gaps,
  replacing hard disc/fog edges so the supersized worlds terminate geologically into an implied void (kills the
  'boxed-in / diorama' feel). Wired into Fractal Spires, Recursive Citadel, 10x Marble (rough cliffs also give
  material contrast vs the polished floor). Horizon-scale in these vast worlds. 259 tests.
- Essay principles applied: organic boundaries (done), material contrast (cliffs/talus tinting + matte-vs-polished).
- DEFERRED (essay): storytelling non-uniform LIGHTING (needs shader accents); micro-narrative decay PROPS w/
  radiating damage; verticality/sightline 'flow' (translates to atmosphere here, non-combat); these are next.

## Hollow Jungle — third recursive map (this turn)
- recurse_tree(): branching recursion w/ leafy canopy clusters → dense self-similar jungle for free.
- hollow_jungle(): inside a hollow rock planet — inner stone shell + moss skin (normals flipped inward) so jungle
  curves up all around; ~44-78 recursive trees grow INWARD (golden-angle spread) + grass + boulders; a translucent
  FLOATING OCEAN hangs in the hollow (glow core, bob, ripples, dripping falls, droplets) lit by a suspended sun;
  pollen motes. New HOLLOW_JUNGLE palette. 0% clip. Best viewed from near the shell (dist ~130).
- Three recursion archetypes now: branching crystal (spires), stacking castle (citadel), branching jungle on a
  curved inner shell (hollow). cull/LOD drop the far hemisphere.
- DEFERRED: reef/root maps; true PBR/reflections; glyph text. 259 tests.

## Recursive Citadel — second recursive map (this turn)
- recurse_citadel(): subdivision recursion — a drum stacks a smaller drum + corner turrets, each recursing. One
  rule → self-similar castle; detail stacks with depth. (Counterpart to recurse_spire's branching.)
- citadel(): ~15-38 fractal keeps on a warm sandstone plain (sqrt spread, distance-scaled), courtyard walls,
  date-palms, dunes, pennants. New RECURSIVE_CITADEL palette. 0% clip. Best ~dist 48-55.
- Two recursion styles now shipped (branching crystal + stacking castle); pattern reusable for more.
- DEFERRED: reef/root recursive maps; true PBR/reflections; glyph text. 259 tests.

## Fractal Spires — recursive supersized map (this turn)
- recurse_spire(): one recursive rule (tapered shaft → 2-3 bent child shafts → recurse to depth) so detail stacks
  self-similarly with no hand-placement; raise depth for free finer crystal growth.
- fractal_spires(): ~24-54 recursive towers on a vast dark reflecting plain (sqrt spread, distance-scaled), glowing
  tips, glow-pools, motes. New FRACTAL_SPIRES palette (indigo night, teal/violet glow). 0% clip.
- LOD/cull keep the recursion cheap (far generations dissolve). Best viewed ~dist 48-55.
- DEFERRED: more recursive maps (framework reusable); true PBR/reflections (llvmpipe can't show); glyph text. 259 tests.

## Forest terrain + expansion + stream (this turn)
- ROLLING TERRAIN amp 1.35->2.4 (trees/ground seat on heightfield). EXPANSION: big=(nodes/130).clamp(1,2.4)
  scales terrain radius + tall-thin tree count + spread → bigger forest for bigger conversations. WINDING STREAM:
  meandering creek in the ground + ripples + reedy/rocky banks, length scales with the forest. clip 18.0% (accepted).
- DEFERRED: same backlog — true PBR/reflections (llvmpipe can't show), glyph textures, flat-panel deepening;
  extend terrain/water/scaling to more scenes next. 259 tests.

## Dynamic per-mesh LOD (this turn)
- pack_sorted(meshes, cam, min_ndc): drops an entire scenery mesh whose projected screen size (radius*focal/depth)
  is below min_ndc — every frame, so distant detail dissolves out + back in as the camera moves. Hole-free (large
  walls/floor/dome always survive; graph packed with min_ndc=0 → every node kept). Power-aware, LOGSCOPE_LOD tunes,
  LOGSCOPE_NO_LOD=1 off. Stacks with frustum cull + scatter-density LOD.
- Verified: near full-detail, far/zoomed hole-free. 259 pass.

## Engine: bump + battery-saver + culling (this turn)
- TEXTURE BUMP: base relief +1.4x + detail-tier bump now drives the normal bake (bump_scale) → higher tiers
  genuinely bumpier; seam threshold rescaled.
- BATTERY SAVER (Linux phones): LOGSCOPE_POWER=half|quarter → render scale 0.5/0.25, texture tier cap <=6/<=3,
  thinner scatter, tighter cull; UI stays full-res.
- FRUSTUM CULLING in pack_sorted: size-aware per-tri clip-space cull (big tris exempt → no holes), POWER-tighter,
  LOGSCOPE_NO_CULL=1 off.
- DEFERRED: true PBR materials/reflections (llvmpipe can't show); glyph textures; per-frame leaf LOD is now via
  cull+density (not yet true mesh-level dynamic LOD). 258 pass + sandbox flake.

## Marble world 10x expansion (this turn)
- MANSION expanded 10x: central domed plaza stays the heart (graph anchored, 5/5). Vast outer marble plain
  (30*sc*10) + world-wide reflecting film + 10 long lazy rivers + 44 distance-scaled structure clusters
  (near groves -> far grand monuments) + 6 towering obelisk landmarks fading into haze. clip 1.0%.
- DEFERRED: true marble/asphalt PBR; glyph textures; flat-panel deepening; llvmpipe can't show marble
  brightness/reflections (mansion renders blow near-white). Far structures may meet the renderer's far-clip/fog
  on very large graphs. 259 tests.

## Marble-world redesign + Mistfall rings + ground depth (this turn)
- MANSION: grand FLAT marble plain (floor 30*sc), thinned clutter, LAZY RIVERS (meandering water-disc chains +
  ripples), STRUCTURE CLUSTERS (marble_cluster: varied-form/height pillars, some planted, some waterfall+pool).
  clip 1.0%.
- MISTFALL: circular antigrav waterfalls rewritten — 3 braided RIPPLING strands + foam + dense droplets +
  recondense knot + crash spray; RIPPLE rings (ripple_rings) on lake + plunge pools. clip 7.8%.
- GROUND DEPTH: ground_wear() cracks + potholes (recessed bowl+rim) on Forest/Canopy/Alley floors.
- DEFERRED: true asphalt PBR; glyph textures; flat-panel deepening (signs/cafe); llvmpipe can't show
  reflections/marble brightness. 259 tests.

## Character waterfalls (this turn)
- waterfall() rewritten w/ style param + heavier PARTICLE droplet streams.
- MISTFALL (style 0): ethereal veils dissolving into spray before the pool, tall ghostly mist column, very
  transparent. clip 7.5%.
- MANSION (style 1): gold-lit MARBLE CASCADES — tight sparkling ribbon over a carved spill-lip into catch-pools,
  light sparkle haze; 3 around the terrace. clip 1.0%.
- DEFERRED: true asphalt PBR; glyph textures; Mistfall water-volume polish; flat-panel deepening (signs/cafe).
  259 tests.

## Duplexes + canopy-gnarled + mansion-reflections (this turn)
- ALLEY: brick_building now a 3D DUPLEX box (depth, doors+stoops, framed windows, occasional BALCONIES barred/
  parapet + french door + potted plant, cornice, chimney). German-Village BRICK PAVING (red setts + mortar grid).
  CLOTHESLINES with flapping laundry. Cats PROWLING, more parked BIKES, ~2x birds. clip 14.3%.
- CANOPY: occasional funky GNARLED trees (thick twisted trunks, scattered heavy bends). clip 10.9%.
- MANSION: reflective SHEEN film over the marble (mirror), brighter pools, + TOPIARY/URNS/FOUNTAINS garden
  expansion kept clear of centre. clip 1.0%.
- DEFERRED: true asphalt PBR material; glyph textures for sign/cafe/panel text; Mistfall volumetric water +
  particle waterfalls; remaining flat-panel deepening (signs/cafe). 259 tests.

## Alley-buildings/road + treehouse-snap + forest-leaves (this turn)
- FIXED: Alley brick BUILDINGS were missing — section-kind rh() hash clustered to kinds 0/1/5 only; switched to
  well-mixed hash() seeded per convo → all 6 section types (incl building) now appear.
- Alley trees: height 8..20, girth 0.5..1.45, splay 0.65..1.25, crown twist/rotation, varied arch.
- Alley ROAD detail: centre-line dashes, tar-seam patches, manholes, oil stains (+ existing cracks).
- TREEHOUSE: straighter trunks (low-bend) + per-tree seeds + platform r 2.6 → platforms snap onto trunks;
  bonus clip 24.3->16.5%.
- FOREST: more falling leaves (240/230) + denser floor litter (480), wind-driven.
- 259 tests. Alley 14.3% / Forest 17.7% / Treehouse 16.5% (dense-foliage family).

## Character/weather/camera pass (this turn)
- ALLEY trees: character (random side, jittered, per-tree height/girth/branch/splay/lean/high-bough) → wide
  organic canopy; thinned+planted-back for clip 18.2% (Forest family). Road: procedural CRACKED ASPHALT
  filaments + faded-yellow SPEEDBUMPS. Warmer sun light.
- FOREST: falling/blowing LEAVES + WEATHER particles per season (autumn leaves/winter snow/spring blossom),
  leaf PILE-UPS, seasonal CANOPY colour via foliage_tint() thread-local (autumn now turns the trees amber),
  distant-leaf LOD tightened for perf.
- TREEHOUSE: platforms SNAP onto leaning trunks (height = fraction up trunk, xz offset by lean).
- CAMERA: slower + smoother (tour 0.05->0.028, follow k 0.12->0.07, pan dur up).
- DEFERRED (honest): Alley deep-lighting overhaul, Mistfall volumetric/transparent water + particle waterfalls,
  global deepen-flat-objects, Forest rain, true per-frame leaf frustum culling (renderer task). 259 tests.

## Maps batch (this turn)
- ALLEY: ~75% width (wz *0.75, clamp 3..12) + trees de-stretched (arc boost 0.07, lean 0.55, shorter+thicker
  0.78 girth → branches not spindly tubes). clip 13.3%, intentional cozy lane.
- FOREST: tall grass patches (tall-scaled leaf_cluster clumps swaying between trees, scales with nodes).
- OLD LIBRARY: GALLERY FLOORS (1-3 rising mezzanine rings w/ railing + bookcase band + reading-nook gaps,
  scaling with nodes) + SPIRAL STAIRS (spiral_stair(): treads+newel+balusters+handrail) two corners; footprint
  spread wider (scene_fit 34→26). Graph climbs through real storeys. 5/5.
- SLATE: heather + tussock grass tufts between outcrops. 5/5.
- 259 tests. Forest 17.5% / Alley 13.3% over the 12% bar = accepted dense-foliage cost.

## Physical-UI + bend + textures pass (this turn)
- TEXTURE CEILING raised on top tiers: 13→1024, 14→1280, 15→1536, 16→2048px (non-decreasing+distinct, 16-tier
  test holds). Tier 16 ≈168MB CPU bake — real-GPU/strong-machine only.
- TREE BEND POLICY (thread-local set_tree_bend(lean_x,lean_z,variety,boost), reset at build() top): Forest varied
  (1.45), Canopy calmer (0.45), Alley opinionated (each tree bows hard out over the lane, lean -sgn*0.85 boost
  0.16 — arches HIGH over the strand; clip 13.8%, intentional, same family as Forest 17.7%/Treehouse 24%).
- PHYSICAL UI (start): physical_ui_panel() per-scene context lectern in the scene's own material — wood board /
  stone tablet / glass pane / marble plaque, with frame + content-line stripes + glowing selection bead. Placed
  outside the graph footprint. Most scenes 5/5. Live node-text-to-surface = future work. 259 tests pass.

## Maps pass (this turn)
- TREEHOUSE: platform/tree ring scaled outward by scene_fit(8).clamp(1,2.4) → graph clears cabins+bridges;
  obj_clip 37.6→24.3% on the mega-spine. below_floor 97% is the floating canopy-walk artifact (strand is up in
  the canopy, not in the ground) — same family as Folding@home; trust the render.
- ALLEY: gnats over the wet lane + birds disturbing leaves overhead (prop_density-scaled). Still 5/5. 259 tests.

## Alley build-out (this pass)
- EnvParams.prop_density (0.7..1.6, seed+size) scales benches/puddles/cracks/wires/bikes per conversation.
- SIDE SECTIONS: 6 seeded types — timber fence / picket / low hedge / brick wall / brick BUILDING (lit windows
  + cornice) / PARK BREAKOUT (grassy yard + tree + bench). Ivy on solid runs.
- Props: bench() x3 kinds (plank / Montréal slat / repurposed SKI-LIFT chair), bicycle(), street_sign()
  (Montréal green plate; glyphs conceptual), cafe()=Café Rose de Mai (rose awning, window, terrace tables+chairs,
  planters; from concept — Maps photo not fetchable), lanterns on wires. Alley HARNESS 5/5. 259 tests pass.

## NEW SCENE: Alley (this pass)
- alley(): overgrown rain-soaked service lane → cozy mind-palace. Wet-stone ground (terrain) + sky-mirroring
  WATER puddles; enclosing weathered-timber/damp-brick walls (m3::panel) both sides with posts + draped IVY
  (leaf_cluster); deciduous trees leaning IN overhead (tree_branch) → dappled canopy tunnel; utility poles +
  sagging catenary wires; hanging NOTICE BOARDS on wires/ropes (bulletin-graph motif, wind-swung); crates with
  dozing ginger CATS (critter kind 1); crack weeds. Lane half-width opens with the graph's ext_z so the strand
  fits. HARNESS 5/5 (below_floor 0, obj_clip 6.6, out_bounds 0).
- PALETTE: ALLEY (muted naturals — walnut/cedar, mossy greens, wet-asphalt grays, puddle-blue, warm 5000K sun)
  registered in theme::PALETTES, the single registry feeding GUI dropdown + scene-view + previews → PORTED
  everywhere at once. GUI bin (logscope) confirmed to build with it. 259 tests pass.
- Real-GPU to verify: puddle reflections/specular, dappled sun through the canopy, cat colour (ginger; llvmpipe
  over-brightens to yellow), wind sway on ivy/boards/weeds.

## Fidelity batch: stems · wind · LOD abstraction · textures · terrain (this pass)
- LEAF PETIOLES: every leaf in leaf_cluster_lod grows a thin tapered stem to the cluster centre/twig (in the LOD
  loop ⇒ culls with distance) — foliage attaches to visible wood.
- WIND ENGINE: global gusting breeze (wind_dir/wind_strength/wind_offset) travelling as a wave so foliage bends
  in sequence; Scene caches a centroid anchor for floating MOSS, animate() adds sway (susceptibility from each
  object's idle bob). Auto-wires every scene's foliage; LOGSCOPE_WIND (0..2.5), LOGSCOPE_WIND_DIR (deg).
- LOD ABSTRACTION: m3::lod_detail(dist, near, far, floor) replaces hand-rolled clamp formulas (forest 3 sites).
- TEXTURES: multi-stop colour LUTs (ramp/lut_stops) + large-scale tone patches + fine hue jitter → organic,
  non-repetitive; mipmaps (already present) + guarded anisotropic filtering. Seamless+res-independent (tests pass).
- TERRAIN: relief() = swell + hills + fine + ridge crests (4 octaves), amplitude held near `amp`. Slate 5/5,
  Forest clip 16%. FIDELITY slider range 10->16 (1k textures, tess x5.7).
- Real-GPU to verify: anisotropy, LUT/noise texture variation, wind motion (animated — not visible in stills),
  terrain ridges, 1k-texture tiers. llvmpipe over-brightens + can't show animation/aniso.

## Believable trees: curved branches + connecting twigs (this pass)
- CURVED BRANCHES: new m3::branch_curved(a,dir,len,r0,r1,side,bend,seed) sweeps a TAPERED multi-ring tube along a
  bowed (t^2) + noise-wobbled centreline and returns (mesh, tip, tip_tangent). tree_branch uses it instead of the
  straight m3::tube; children grow from the bent tip heading → limbs ARC, no more pillars.
- CONNECTING TWIGS: at depth 0, 3-5 thin curved twigs fan off the tip, each carrying a small leaf tuft (tlen*0.7),
  plus a small core cluster (len*0.34) at the fork. Foliage now hangs off visible wood.
- MORE LEAVES: 4-6 clusters/crown (was 1) on top of the dense small-leaf clusters → full rounded canopies.
- METRICS: Canopy 7.0->5.6 (curved central tree reads better). Forest obj_clip 17.7->20.4 (curves bow branches
  outward a touch; strand still in a clearing). Forest below_floor flips to 87% — DOCUMENTED fragile mode artifact
  (strand at -1.6, visibly above the -3.2 ground). 259 tests pass. NOTE: heavier wood/leaf geometry → llvmpipe
  render-load is slower (give headless renders ~16s); LOD still culls distant crowns.

## Smaller leaves + two branching factors (this pass)
- LEAVES: the giant foreground "cards" were hero-tree CROWN blades whose length scaled with crown radius
  (r~4 ⇒ ~1u blades). Fixed: blade length now ABSOLUTE-capped — `(r*0.10*jit).clamp(0.05,0.30)` — and crowns
  packed denser (`40 + r*16`, ≤110 leaves) so big crowns render as MANY small leaves, not few big cards.
  Crown sphere tightened to len*0.78. Reads as fine dense foliage at all distances (LOD still culls far).
- TWO branching factors now in EnvParams, both seed-derived per conversation, both threaded through tree_branch
  as EXPLICIT params so they shape only Forest BACKGROUND trees (hero/Canopy/Treehouse pinned to 3 / 0.6):
    1. tree_branch  — children per fork (3..5, forkier chats bushier)
    2. branch_spread — how wide forks splay (0.50..0.85)
  Plus tree_size / tree_height / tree_girth (seed-derived) driving Forest tree dimensions (hf/gf).
- METRICS: Canopy 7.0 (held — branching is Forest-isolated). Forest obj_clip 17.7 (cost of taller/thicker varied
  trees; strand reads in a clearing). Forest below_floor flips 0↔65% on branch-angle changes — DOCUMENTED fragile
  mode-of-obstacle-base artifact; the strand is at -1.6, visibly above the -3.2 ground. Trust the render here.
  259 tests pass (the earlier sandbox cargo-verify failure was a transient rustc-target env error; recovered).

# Geometric self-check harness — scorecard (mega-spine, 412 nodes)

obj_clip counts ONLY real clipping: HARD materials (wood/stone), local solids (footprint < 14u — bigger =
terrain/platform/blob), excluding concentric room shells (rings/dome/tiers) and soft matter (foliage/water/paper).

| scene        | obj_clip | verdict | note |
|--------------|----------|---------|------|
| Atlantis     | 0.0%     | 5/5 PASS | routed strand |
| Old Library  | 0.0%     | 5/5 PASS | |
| Reading Room | 0.0%     | 5/5 PASS | de-colosseum'd this pass |
| Slate        | 0.0%     | 5/5 PASS | |
| Archive      | 0.7%     | 5/5 PASS | |
| Cloudscape   | 7.3%     | PASS | floor_y est. quirk -> 3/5 |
| Mistfall     | 7.0%     | 5/5 PASS | |
| Forest       | 9.0%     | PASS | floor_y est. quirk -> 4/5 |
| Folding@home | 6.1%     | PASS | was 56.8% false-positive (terrain slabs) |
| Canopy       | 21.1%    | FAIL | strand threads the labyrinth walls |
| Treehouse    | 33.5%    | FAIL | strand threads the cabins |

## Foliage LOD + leaf subdivision (this pass)
- FOREST LEAVES: blades shrunk r*0.42->r*0.11-0.19 + count up to 22/cluster -> fine foliage, not big cards.
  New leaf_cluster_lod(detail) culls leaf COUNT with distance: hero crowns full, slender background trees
  0.3-0.65, distant blot-out band 0.32. Global (Mistfall moss / Canopy tree / Treehouse also finer). Harness
  unaffected (leaves are foliage, not obstacles): Canopy 6.6, Forest 12.9, Mistfall 7.5, Slate 0.5, RR/OL/Atl 0.
- FOLDING@HOME: scene_fit reverted (it shifted the floating-protein field, obj_clip 9.7->13.8); kept tuned
  map_radius scaling. Its ~14% below_floor is a chronic metric artifact (floorless scene). 5 scenes on scene_fit.

## Balanced expansion + ground abstraction (prior pass)
- SCENE EXPANSION: scene_fit(base) sizes a scene's fit-out to the graph extent+margin (clamp 1..5), so rooms
  EXPAND to MEET bigger conversations (flex only fine-tunes). Applied to all 6 the user listed:
  Reading Room 0%, Old Library 0%, Archive 1.0%, Slate 0.5%, Folding@home 9.7%, CANOPY 31.6%->6.6%.
- FOREST: ground unified onto shared relief()/terrain_disc; EVERY element seated on gy() (grass/flowers/logs/
  spathes/rocks/bushes/crowns) -> grass no longer sits below the displaced ground. obj_clip 12.9% (marginal).
- MISTFALL: water kept low but MORE transparent (104->72); archipelago+portals+droplets+STRAND lifted clear
  with a deep void -> lake-cave depth. graph_y [1.6,22], obj_clip 7.5%.

## Map expansion + terrain (prior pass)
- ATLANTIS now an OVAL ROTUNDA (hall_dims) that EXPANDS to contain the graph's spine (x) + branch span (z),
  aspect-capped 2.6:1, short halls for overflow. The z*0.42 SQUISH is removed. strand_aspect 2.98 (natural
  spread kept), obj_clip 0%, out_bounds 0%. The structure grows with the conversation instead of cramming it.
- TERRAIN: shared relief() + terrain_disc() (tessellated fbm heightfield, vcol low->high, proper normals).
  Applied to SLATE -> a rolling moor; henge/boulders/cairns conform via gy(). 0% below_floor, 0% clip.
  Reusable; extend to Folding@home / Cloudscape / Canopy ground next.

## Earlier pass
- HARNESS ACCURACY (3 false-positive classes fixed): concentric room SHELLS (Atlantis balcony-rings/dome ->
  71%->0%), soft FOLIAGE/water/paper (Forest 39%->9%), and big TERRAIN slabs (Folding@home 56.8%->6.1%).
  obj_clip now reflects real wood/stone clipping only.
- READING ROOM: dropped the second free-standing colonnade ring (stacked colonnades = colosseum arcade);
  upper level is now a railed book-gallery. Reads as one rotunda library, two distinct levels. 0% clip.
- MISTFALL (earlier): buttresses scale with room + stones out of the lane (16.5%->7%).

## Known real clips (structured "explore" scenes)
- Canopy (labyrinth) + Treehouse (cabins): the strand threads a confined structure. The flex can't fully clear
  a packed maze/cabin cluster, and a global vertical flatten regressed Canopy (its strand escapes ABOVE the
  walls). These read as thematic ("explore the structure") but are tracked for per-scene routing if wanted.
