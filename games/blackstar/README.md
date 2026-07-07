# Cybertron: Arena of Gods

A "Smite meets Fall of Cybertron" transforming-robot arena brawler, built on the
exengine engine (now rolled into this codebase and built with CMake).

## Layout
- engine_src/            the exengine engine, vendored as part of this codebase
  - exengine/            engine core + OUR new game modules:
    - coll_chunk.c/.h     destructible & event-gated collision chunks
    - events.c/.h         pub/sub map-event bus
    - transform.c/.h      robot <-> vehicle transform state machine
    - netcode.c/.h        server-authoritative wire protocol
  - lib/                 bundled PhysFS + MojoAL (no system packages needed)
- tests/                 headless module tests (chunk / transform / netcode)
- CMakeLists.txt         builds libexengine (engine + modules), the game, and tests

## Build
    cmake -B build -S .
    cmake --build build -j

Produces:
- build/libexengine.a    engine + all four Cybertron modules as one static lib
- build/game             the game executable, linked against libexengine
- build/data.ex          the packed asset archive (shaders + textures + models)
- build/{example_usage,test_transform,test_netcode}   module tests

## Run
The engine now locates its asset archive (data.ex) next to the game
executable (via PHYSFS_getBaseDir), so launch location does not matter:

    ./build/game        # from the project root -- works
    cd build && ./game  # from the build dir    -- also works

(data.ex is generated into the build dir by the data_archive target.)

## Test
    cd build && ctest --output-on-failure

All three module tests pass headless (no GPU/display needed) -- suitable for CI
on Mesa desktop and Asahi aarch64 as well.

## Notes on rendering target
The engine currently uses exengine's OpenGL 3.3 (glad) path. glad vendors its own
GL headers, so only a libGL/GLESv2 runtime is needed to link -- keeping this
buildable on Mesa and Asahi (aarch64) without GL dev packages. The planned Vulkan
migration (for the Mesa/Asahi high-fidelity rasterized target) is the remaining
renderer work; all the game-logic modules above are renderer-independent.

## Live module integration (game.c)
The four modules now drive the running scene, not just tests:
- Press T to transform the avatar cube robot<->vehicle (size/height changes, move-speed changes, HUD shows mode+state)
- Walk near the wall and press G to deal a Heavy-class hit; wall shows damage, then is destroyed and fires a "wall_breached" map event (HUD banner)
- HUD (top-left) shows avatar mode/state and wall HP live

## Networking (real UDP)
net_transport.h/.c adds BSD-socket reliable-UDP under the netcode protocol.
Test it with two processes:

    cd build
    ./test_transport server &
    ./test_transport client

The client converges on the server's authoritative chunk-destruction state.
Verified here to survive 40% simulated packet loss (reliable resend works).

## Vulkan scaffold (optional)
    cmake -B build-vk -S . -DUSE_VULKAN=ON
    cmake --build build-vk -j
    ./build-vk/vk_probe          # enumerates Vulkan GPUs

HONEST SCOPE: this brings Vulkan up through a clearing render pass
(instance -> device -> swapchain -> render pass -> present). It's the
foundation of the GL->Vulkan migration, NOT a finished renderer -- the
deferred pipeline (g-buffer/lighting/shadows/SSAO/post) is the large
remaining work. No hardware ray tracing (Mesa/Asahi aarch64 target
doesn't expose it). Vulkan headers are vendored in third_party/.
The default build stays on the working GL path.

## From early-fps: map-FSM + keybinds + Rust FFI
Two ideas ported from the early-fps Rust codebase, plus a Rust bridge:

### mapfsm (src/exengine/mapfsm.h/.c)
Intent-driven map placement from fsm.rs: a feature is an agent that reads a
Site (sightline, slope, high-ground, traffic, wetness) and decides whether
it grows there and how tough it is -- producing a coll_chunk spawn decision.
Features: SightlineBreaker, Vantage, Sentinel, LogisticsSpine, Wreckage,
Shelter. Scatter candidate sites and let each feature gate itself in.

### keybind (src/exengine/keybind.h/.c)
Remappable action->key bindings from early-fps's Bindings design. The game
queries by ACTION (Forward, Transform, Fire...), never by raw scancode, so
rebinding is one call. QWERTY and Dvorak presets included. Wired into
game.c: T=transform and G=fire now go through the binding layer.

### Rust FFI bridge (rust_fsm/)
The REAL fsm.rs gating logic, exposed over a C ABI (cdylib + staticlib) with
#[repr(C)] structs matching mapfsm.h. Build and link it with:

    cmake -B build -S . -DUSE_RUST_FSM=ON
    cmake --build build -j
    ./build/test_mapfsm          # verifies C port and Rust bridge agree

Requires cargo. test_mapfsm asserts the pure-C port and the Rust gating
produce identical decisions on every case. Default build stays pure-C.

## Gameplay loop (arena + combat + enemy)
The demo is now an actual arena fight:

- **arena.c/h** — procedurally populates the level with cover by scattering
  candidate sites and running the map-FSM (mapfsm/fsm.rs) on each. Same seed
  => same layout; new seed => new arena. Wired into game.c (4x4 grid).
- **combat.c/h** — hitscan raycast (ray vs chunk AABB + ray vs enemy sphere),
  returns the nearest hit. Fire (G) now casts from the camera down the crosshair
  at long range, matching the ranged-engagement design; cover blocks shots.
- **enemy.c/h** — a bot with an idle->chase->attack->dead state machine. It
  chases on the XZ plane, only engages with line-of-sight (LOS is a chunk
  raycast), fires on a cooldown at range, and dies when its HP is gone.

In the live window: shoot the bot (it chases and shoots back), or blow up
cover to open new lanes. HUD shows enemy state/HP and your last shot result.

Tested headless: tests/test_gameplay.c (arena determinism, combat hit
resolution incl. cover blocking, full enemy AI lifecycle) -- part of ctest.

## Blackstar lore: elements + the Watch + the Corrupted
Ideas adapted (as original content) from the Blackstar worldbuilding:

- **element.c/h** — seven elemental affinities (Ice, Fire, Air, Electric, Vine,
  Rock, Water) with a rock-paper-scissors advantage cycle (each strong vs two,
  weak vs two) and distinct combat profiles (Fire = burst/short range, Electric
  = long range, Air = fast/mobile, Rock = heavy/slow, etc). Plus **the Watch**:
  a device that lets the player embody any element mid-fight, with a short
  attune delay so you can't hot-swap for free. Wired into game.c: press **B**
  to cycle element; your shot damage scales by element profile x matchup
  advantage vs the target's element.
- **Corrupted enemy** (enemy.c/h, EX_ENEMY_CORRUPTED) — a horror archetype that
  ignores line-of-sight and cover entirely and just relentlessly closes the
  distance (melee-only, fast, always aware), contrasting the ranged Grunt.
  One is mixed into the live arena alongside the ranged bot.

Tested headless: tests/test_element.c (advantage cycle balance, Watch attune
state machine, Corrupted-vs-Grunt LOS behaviour) -- part of ctest (6/6).

## Menus, character builder, Dvorak, real map, perf (this pass)
- **menu.c/h** — TITLE -> BUILDER -> PLAYING <-> PAUSED (Esc). The builder is
  the character builder: pick size class (Mini/Standard/Heavy/Titan), starting
  element, and keyboard layout, then START spawns the match from that loadout.
  Navigate with arrows (WASD/ESDF also work), Enter/Space confirms.
- **Dvorak fixed properly** — SDL scancodes are PHYSICAL positions, so the
  Dvorak preset now binds physical E/S/D/F (which type . o e u on Dvorak).
  Movement/jump/transform/fire ALL route through keybinds now; pick "dvorak"
  in the builder. Left mouse also fires.
- **robot_parts.c/h** — characters are now 7-part bodies (torso/head/arms/
  legs/back). Transforming visibly rearranges the body into a low vehicle
  block; the Corrupted is asymmetric and twitching (tilted head, dragging
  fused arm) per the horror design; size class scales the whole silhouette.
- **data/arena.iqm** — a generated 120x120 space-outpost arena (floor, 8-unit
  perimeter walls, central platform with 4 ramps, rock pillars), ~3x the demo
  map, replacing level.iqm. Regenerate with `python3 tools/gen_arena.py`;
  structurally validated headless by tests/test_arena_iqm.c.
- **Defeat loop** — grunt shots and corrupted melee damage you; at 0 hp you're
  returned to the builder for another run.
- **Perf** — removed per-frame debug-font-atlas draw and per-frame sound_play,
  no shadow casters by default, and the new map is only ~200 tris. If FPS is
  still low on Asahi, drop window_width/height in conf.ini (only settings the
  engine currently reads).


## Render fixes (visibility + third person)
Feedback from first render: "void with brick spheres, no character, no movement."
Fixed:
- **Arena was invisible from the inside** -- you stand inside the walls, but
  single-sided faces get back-face culled when viewed from within, leaving a
  void. The map generator now emits double-sided geometry (tools/gen_arena.py),
  so floor/walls/ramps are visible from any side.
- **Third-person chase camera** -- the camera now sits behind + above the
  player and looks along the aim direction, so you actually see your
  transformer (and watch it morph robot<->vehicle). Shots raycast from the
  player muzzle along the aim, not from the pulled-back camera.
- **Body facing** -- part offsets rotate around the body's vertical axis to
  face the aim/target (was previously not turning; also fixed a
  radians-vs-degrees bug in the per-part rotation).
- **Lighting** -- a bright key light now rides above the player (the old 0.25
  ambient left the 120x120 arena near-black) plus a high fill light, so the
  space reads as a lit arena instead of a void.
- **Spawn** -- lowered to sit on the arena near the central platform.

## Content pass: box asset, full menu, campaign, maps
- **box.iqm (fixes the "spheres")** — cube.iqm is actually a unit *sphere*, so
  robot parts and cover rendered as balls. tools/gen_box.py generates a real
  unit cube; the game now builds bodies and cover from it, so the robot reads
  as a boxy transformer.
- **Full menu tree** — TITLE (Campaign / Skirmish / Settings / Controls / Quit).
  Skirmish -> mode select (arena fight / survival) -> map select -> builder.
  Settings -> resolution, render scale, show-fps. Controls -> shows the current
  bindings and layout (qwerty/dvorak). All driven by the menu state machine in
  menu.c/h; tested headless in test_menu.
- **Campaign + cutscenes** — campaign.c/h: a 4-mission campaign with per-mission
  map, enemy makeup, objective, and intro/outro cutscene text slides (original
  writing, themed after the scripts, not reproduced from them). Missions unlock
  in order; clearing one plays its outro and unlocks the next. Cutscenes render
  as VGA text slides advanced with Enter.
- **Two maps, actually switchable** — data/arena.iqm ("wastes") and data/ice.iqm
  ("frostline"), same geometry/collision, different texture themes; the selected
  map's visual is shown per skirmish/mission. Regenerate via
  `python3 tools/gen_arena.py <name> <floortex> <walltex> <rocktex> <pillars> <seed>`.

Still ahead (bigger efforts): richer per-theme map geometry (not just retexture),
proper multi-mesh art instead of box primitives, applying the graphics settings
live (resolution currently reads from conf.ini at startup), and rebindable keys.

## Combat feel: projectiles + effects (vfx)
Combat was invisible (HP just changed). Added vfx.c/h -- a fixed pool of
short-lived effects mapped onto a pool of unlit models each frame:
- **Bolts** -- firing spawns a muzzle flash + a fast projectile that travels
  to the hit point and bursts (hitscan damage is still instant; the bolt is
  the tracer that sells it). Colored by your active element.
- **Impacts** -- a spark burst where a shot lands.
- **Enemy fire is visible** -- grunts emit a red tracer toward you; the
  Corrupted's melee lands a purple impact burst on you.
- **Death bursts** -- destroyed enemies throw chunky debris that falls and
  settles on the ground.
- **Crosshair** -- a center '+' so third-person aiming is clear.

Tested headless: tests/test_vfx.c (spawn/expire lifetimes, bolt->impact
chaining, debris gravity/ground clamp, pool capacity). ctest 9/9.

## Gait animation, visible VFX, themed maps (adapting unendlich + biomes)
- **Procedural gait (from unendlich pose.rs)** — robot_parts now animates:
  legs swing fore/aft in counter-phase, arms counter-swing, the body bobs at
  2x stride, all amplitudes scaling with speed (standing is still, no marching
  in place). Drives walk + transform readability. Tested in test_gait.
- **VFX now actually visible** — the forward shader had no unlit path, so
  effects rendered near-black and the element tint was never applied. Added an
  emissive u_is_lit/u_tint path + a bright glow.iqm; bolts, muzzle flashes and
  sparks now glow in the active element's colour. Bolts slowed/enlarged so they
  read. (This is why you saw only debris before.)
- **Element-themed bodies** — robot parts are tinted by element using palettes
  adapted from the hero-forge (ice=pale steel-blue, fire=hot iron, etc);
  Corrupted enemies read sick-violet, grunts a duller steel.
- **Real themed maps (from the biome set)** — genuinely different geometry, not
  retextures: rusted **wastes** (toppled slabs, leaning girders, dead relay
  mound), **frostline** (ice spires + frozen dais), **ember pit** (sunken
  obsidian tiers ringing a titan's dais), **crystal vault** (tall crystal
  columns + sealed relic plinth). A shared invisible collision hull (floor +
  walls) handles collision; themed geometry is visual + FSM cover is tactical.
  Generate via `python3 tools/gen_world.py`. Each map carries a story landmark;
  campaign missions are mapped onto them.

## Visibility fixes (darker / white boxes)
Feedback: "white boxes, darker, hard to see." Root causes fixed:
- **Ambient was 0.025** -- nearly black, so anything not right under a point
  light was invisible; the bigger themed maps made this obvious. Raised scene
  ambient to 0.18 so the whole arena is readable.
- **Lit path multiplied by an unset tint** -- a GLSL uniform not set every draw
  reads as 0, so some passes rendered black. Added a guard (near-zero tint ->
  white) and switched robot tint to a clamped blend that colours without ever
  darkening a model.
- **ex_model_copy dropped is_lit/is_shadow/tint/scale** -- copies (robot parts,
  cover, vfx) reverted to defaults, so some rendered unlit/white. Copies now
  inherit these, so lit models stay lit (textured) and only true effects glow.

## Tone, robots, and menu overhaul
Render/tone:
- **White bullets fixed** — the emissive path sampled the (muddy) texture and
  washed the colour to grey/white. Effects now output their PURE tint colour,
  so bolts/sparks/flashes glow cleanly in the element colour.
- **Darker, moodier tone** — ambient dropped to 0.06 and the lit path now runs
  an ACES-approx filmic tonemap so bright lights roll off instead of blowing
  out flat. Lighting is a warm key that follows you + a dim cool fill, so the
  point lights carve shape instead of a flat wash.

Robots (no longer basic blocks):
- 13-part silhouette: chest + abdomen, head + visor, pauldrons, upper/fore
  arms, thighs + shins, back thruster — gait-animated (legs/arms swing, body
  bobs). Added per-axis scale (scale3) to the engine so limbs are properly
  proportioned (thin legs, broad chest) instead of cubes. Element accent colour
  on visor/crest/thruster.
- **Bot customizer** — a builder page for height / bulk / head / shoulders /
  arms / legs / accent colour (hero-forge BuildParams), applied live to the
  body. Reach it from the character builder's "customize bot" row.

Menus:
- **ESC goes back** one screen everywhere (and cancels cutscenes); ESC still
  pauses while playing.
- **Quit match** button in the pause menu (resume / quit match / quit game).
- **Mouse support** — hover highlights a row, left-click activates it.
- **More settings** — volume (applied to master), mouse sensitivity (applied to
  the camera), brightness (scales the key light). Resolution/render-scale are
  still startup-only (engine reads conf.ini at boot).

## Blackstar deep-dive: the Watch, weapons, Corrupted stages, foundry
Pulled from the Blackstar Clones/Jhiaxus design docs:
- **The Watch -- 11 forms + Photon (weapon.c/h)** -- Water, Tsunami, Ice, Fire,
  Magma, Blizzard, Metal, Titanium, Electricity, Wind, Tornado, each with a
  DISTINCT weapon profile (bolt / burst / beam / wave / lob; damage, range,
  cooldown, spread, pellets, chains, slow, colour) mapped onto the element
  advantage cycle. Cycle forms with B / V. **Photon** is the apex form -- the
  unreached ceiling -- unlocked after clearing a match, hitting hardest of all.
- **Star-fire** -- hold C to charge a heavy shot up to 6x damage (Billy's
  star-absorbing power, scaled to a chargeable burst).
- **Quantum gravity leap** -- press L for a silent forward insertion (no flash,
  instant), which schedules a delayed gravity wave that lands ~2s later as an
  indifferent AoE at the arrival point -- exactly the doc's "the wave isn't
  aimed, it just happens" beat.
- **Corrupted 3-stage infection** -- the Corrupted now escalate Stage 1
  (wrong-geometry) -> 2 (mid) -> 3 (full) over time, getting faster, hitting
  harder, and GROWING in Stage 3 as they "absorb metal". Melee transmits
  corrupted energon (stage-scaled damage). HUD shows the current stage.
- **The Foundry map** -- an industrial arena (cargo containers, pipe stacks, a
  central smelter where the apex Corrupted lairs). Per the doc, industrial
  terrain is the worst place to fight the Corrupted because there's so much
  metal to absorb. Campaign mission 2 ("the hunt") now stages there.

Tested headless in test_weapon (form profiles, star-fire charge, gravity-leap
delayed wave, Corrupted stage escalation). ctest 11/11.

## Real arena fights: multi-enemy encounters (encounter.c/h)
Matches were a fixed 1 grunt + 1 Corrupted (their HP scaled up as a stand-in
for counts). Now a match is a real arena fight:
- **ex_encounter_t** manages up to 12 enemies, spawning the mission's actual
  makeup (N grunts + N corrupted) scattered around a ring inside the arena.
  Grunts rotate through six elements for variety.
- **Win conditions** -- ELIMINATE (clear them all) or SURVIVE (outlast a timer);
  campaign SURVIVE objectives and skirmish survival mode both use it.
- The game now loops every live enemy: gait-animated body from a pool, combat
  raycast against the full target list, ranged grunts + stage-scaled Corrupted
  melee all resolve onto the player, and death hides the body.
- **HUD** shows enemies-left (or the survive countdown) and the deepest active
  Corruption stage on the field.

Tested headless in test_encounter (spawn makeup, target list, eliminate + survive
win conditions, capacity cap, find-by-id). ctest 12/12.

## Bug fix + swarm enemies, pickups, bigger maps
- **Shooting fixed** -- the fire-cooldown timer was being set but never
  decremented after the encounter refactor, so you got one shot then nothing.
  Restored the per-frame cooldown recovery.

New content pulled from the Blackstar Trilogy Two framework + early-fps:
- **Hijacks (new enemy)** -- Trilogy Two's swarm: fast, fragile, erratic
  (weaving strafe), never flee, indifferent to their own losses -- deadly only
  in numbers. They appear in later campaign missions and survival mode. Adapted
  the archetype from early-fps's Rusher enemy def.
- **Pickups (pickup.c/h, from early-fps pickups.rs)** -- glowing floor items
  that bob/spin: energon (heal +30), shield (armor buffer that absorbs hits),
  overcharge (10s x1.6 damage). Scattered around each arena; grab them between
  waves. HUD shows shield + overcharge.
- **Bigger arenas** -- expanded from 120x120 to 170x170 for larger fights.
- **Two new lore maps** -- **shield boundary** (deep-space starfield floor, a
  luminous energon barrier wall, floating platforms -- the Watch Bearer's
  perimeter against the Hijacks) and **mining station** (outer-rim: ore
  containers, drill towers, hazard markings, a refinery pad). 7 maps total.
- **6 new textures** -- hazard stripes, blue plating, energon grid, obsidian
  with veins, starfield, rusted metal -- applied across the themed maps.

Tested: test_pickup (bob, collect-once, capacity), test_encounter (now with
hijacks). ctest 13/13.

## The apex Corrupted (boss finale)
The campaign now climaxes against the apex Corrupted -- Jhiaxus's masterpiece
from the design doc: the largest, weeks of absorbed infrastructure, "a shape it
returns to between shifts." A phased boss:
- **Phase 1** (>66% hp): a slow, relentless, huge melee mass that grows as it
  absorbs. **Phase 2** (33-66%): reconfigures faster and adds a ground-slam AoE.
  **Phase 3** (<33%): summons Hijack scouts and speeds up again.
- Boss HP + phase bar in the HUD; spawns on the final campaign mission ("the
  thing he made"), with original cutscene text themed on the architect's hubris.
Tested in test_weapon (phase 1->2->3 progression, slam/summon signals, melee
scaling) and test_encounter (boss spawn/kill). ctest 13/13.

## Deep customization: detail bodies, vehicles, abilities, weapon addons
Four new systems (all tested in test_custom, ctest 14/14):
- **Thousand-part detail bodies (detail.c/h)** -- a procedural greeble
  generator scatters plates, rivets, vents, spikes, antennae and ridges across
  the body's parts, deterministically per seed. It can produce 1000+ greebles;
  the live player body renders a budget of ~420 on top of its base parts for an
  intricate, non-blocky look (budget is a one-line constant, DETAIL_MODELS).
- **Vehicle archetypes (vehicle.c/h)** -- the transform now has 9 distinct
  vehicle forms, each a unique part layout, animated: racer, tank, jet,
  hover-skiff, and creature builds insect (6 legs), spider (8 legs), dragon
  (beating wings + swaying tail), dino (quadruped + tail), raptor (fast biped).
  Flyers (jet/hover/dragon) are flagged. Chosen in the customizer.
- **Ability system (ability.c/h)** -- three equippable slots from hover, turret,
  shield, dash, cloak, overclock, each with cooldown + active duration.
  Triggered with keys 1/2/3 in-match (dash lunges, shield grants armor, hover
  lifts, overclock halves fire cooldown). HUD shows slot readiness.
- **Weapon addon system (weaponmod.c/h)** -- up to 3 addons modify the active
  Watch form's profile: long barrel (+range/dmg), scope (-spread), ext mag
  (-cooldown), underbarrel (+pellet, bolt->burst), stabilizer, amplifier
  (+dmg), chain coil (+chain). Applied live to every shot.

All four are chosen on the bot customizer page (proportions + accent + vehicle +
3 abilities + weapon addon), and applied at match start.

## Completing the abilities: turret, cloak, flight
- **Turret (turret.c/h)** -- the TURRET ability now deploys a real auto-gun at
  your feet that acquires the nearest live enemy in range and fires on a
  cooldown for its duration, then expires. Renders as a glowing emplacement.
- **Cloak** -- while active, grunts and Hijacks lose track of you and stop
  chasing. But the Corrupted and the Apex hunt by SPARK RESONANCE, not sight,
  so cloak does NOT hide you from them -- straight from the Jhiaxus design doc.
  HUD warns "cloaked (not vs corrupted)".
- **Flyer vehicles** -- jet, hover-skiff, and dragon now actually fly: gravity
  is cancelled and jump/crouch ascend/descend while in those vehicle forms.

Tested in test_custom (turret target-pick/fire/expire, cloak fooling grunts but
not spark-resonance hunters). ctest 14/14.

## Rebindable keys
The controls screen is now interactive instead of view-only:
- Navigate the action list (forward/left/back/right/jump/sprint/crouch/
  transform/fire/reload), press enter on one, then press any key to rebind it.
  The live binding (via SDL scancode names) is shown next to each action.
- A layout row toggles qwerty/dvorak; a reset row restores preset defaults.
- Custom binds persist across matches (the preset is only re-applied on a
  layout change or explicit reset, so your rebinds aren't wiped each match).
- ESC cancels an in-progress rebind.
Tested in test_menu (set/reset + controls entering listening mode). ctest 14/14.

## Procedural maps with real collision (mapgen) + film codex
Pulled the level-generation backend from early-fps (mapgraph.rs + blockout.rs):
- **Flow-first generation (mapgen.c/h)** -- each match a map is first a 2D GRAPH
  of nodes (spawn/tower/trench/room/junction/cover) and edges (routes). Graphs
  are tiny, so the game generates ~160 candidates, gates on connectivity
  (flood-fill), and scores flow / route-choice / sightline-variety / flank /
  vantage -- keeping only the best seed. The winner compiles into a BLUEPRINT of
  role-tagged volumes (block / low+high cover / channel / expose / elevate /
  conceal) placed to break sightlines and grant vantage.
- **Solid cover at last** -- every volume renders as a box AND blocks the player
  via an AABB push-out (you can also stand on platforms/low cover). This fixes
  the long-standing "walk through decoration" problem for the procedural layer.
- Cover is themed by the map/film location (plate for foundry/mining/shield,
  rock for wastes/emberpit), and the player now spawns at the graph's spawn node.
- Tested in test_mapgen (connectivity gate scores 0, best-of-N selection,
  deterministic generation, blueprint volume roles). ctest 15/15.

**Film codex (lore.c/h)** -- a menu codex with 10 original entries spanning the
six-film arc (Doomsday / Dominion / Destiny, then First Light / Solar / Photon)
plus the Watch, Corrupted, Hijacks and gravity leap -- our own summaries, no
source text reproduced. Reachable from the title menu; left/right to browse.

## More game, less demo: waves + score, outer-rim & homeworld maps
- **Wave game loop (waves.c/h)** -- skirmish and survival are now a real game
  loop: successive waves of escalating enemies (grunts, then Corrupted from
  wave 2, then Hijacks from wave 3), a running SCORE (grunt 100 / Hijack 60 /
  Corrupted 250 / apex 2000, plus a growing wave-clear bonus), a wave counter,
  and a 3s breather between waves. Skirmish is 8 waves; survival is endless.
  HUD shows wave / enemies / score. Tested in test_waves (escalation, scoring,
  clear bonus, 8-wave completion, endless).
- **Outer rim map** (Dominion) -- asteroid platforms adrift over a nebula, an
  abandoned mining rig with a central drill; deep-space defense.
- **Cybertron map** (Destiny) -- the homeworld: golden-metal ruins, toppled and
  standing towers, a broken ring causeway, a central citadel plinth under a
  shield-gold sky. 9 maps total; 4 new textures (cybertron/asteroid/nebula/
  shield-gold).
- **Reachable elevation** -- procedural tower platforms now get stepped ramps so
  their vantage is actually walkable, not just a sheer block.

ctest 16/16.

## Menu polish + map mood
- **Mouse wheel** now changes the highlighted value left/right across every menu
  (builder, customize, settings, controls layout), and scrolls through Watch
  forms while playing. Left-click confirms everywhere.
- **Controls menu fully mouse-accessible** -- the VGA overlay is a 16-row text
  grid; long lists (controls has 13 rows) were drawing the last item off the
  bottom of the grid, so it was unreachable. The list now shifts up to fit, and
  mouse hover maps exactly onto the grid rows -- every item is selectable.
- **Brighter** -- scene ambient lifted from 0.06 to 0.11 (the maps were a touch
  too dark) while keeping the moody filmic tonemap.
- **Maps look distinct now** -- each of the 9 maps gets its own lighting mood
  (key + fill colour) AND its own background/sky void colour: cold blue for
  frostline, hot red for the ember pit, gold for Cybertron, deep starfield blue
  for the outer rim, electric blue for the shield boundary, and so on. Same
  arena, very different feel.
ctest 16/16.

## Toward a real look: skyboxes, fog, detailed textures
The maps read too flat/"Doom-like"; several atmosphere upgrades:
- **Skyboxes** -- each theme now has a real backdrop: a large textured sky cube
  that follows the camera (infinite-distance feel). Five skies -- deep-space
  nebula+stars, pale-blue aurora ice, red ember smoke, gold Cybertron dusk, and
  dusty waste haze -- picked per map. (512px generated sky art.)
- **Distance fog** -- geometry now melts into a horizon coloured to match the
  sky, added to the forward shader (view-space depth -> exp fog). Kills the hard
  flat-void edges and gives depth/atmosphere.
- **Detailed textures** -- the core surfaces (wall/floor/rock/blue plate/
  Cybertron) were regenerated at 256px with panel seams, rivets, wear streaks,
  tread patterns, cracks, and circuit traces -- so metal reads as plated metal
  and stone as cracked stone, not flat colour noise.
- Ambient lift (0.06->0.11) plus per-theme key/fill light colour and per-theme
  background colour from the previous pass.

Honest scope: this is still a box-primitive OpenGL 3.3 renderer with procedurally
generated (not artist-authored) assets, so it won't match a studio production --
but skybox + fog + detailed textures + per-map lighting move it well beyond flat
greybox. ctest 16/16.

## Smooth geometry (logscope mesh3d) + more Blackstar content
Pulled the mesh library from logscope (monorepo mesh3d.rs):
- **Smooth prop meshes (tools/gen_props.py)** -- ported uv_sphere, the faceted
  rock (a smoothly displaced sphere with averaged vertex normals -> rounded, not
  spiky), and tapered cylinders. Generates rock/boulder/spire/column/pillar/dome
  IQMs with real vertex normals for smooth shading.
- **Procedural cover is now rounded** -- the blueprint's blocks and cover render
  as smooth rocks and its vantage points as tapered spires, instead of boxes
  (walls/platforms stay boxes). Same AABB collision, much better silhouette.
  (Also brought in logscope's seamless-tiling fBm idea for future texturing.)

More Blackstar source pulled in:
- **Low-gravity combat** -- on the outer-rim and shield-boundary maps gravity
  drops to 0.4x with a higher jump, so those fights float -- straight from the
  Dominion asteroid-belt sector ("Low-gravity combat. Debris fields. Isolation.").
- **3 new codex entries** -- the Array (the flagship's crystal detector), the
  Boundary (the light-lattice at the system's edge), and the Deep Outpost (the
  ocean-floor horror descent). 13 codex entries total. All original wording.
ctest 16/16.

## End-of-run results + persistent high score
Wave runs (skirmish/survival) now end on a proper results screen instead of
dumping you back to the builder:
- Shows VICTORY (all waves cleared) or DEFEAT, plus waves reached, enemies
  downed, and final score.
- Tracks a persistent HIGH SCORE saved to disk (cybertron_highscore.dat); beats
  flash "NEW HIGH SCORE", otherwise your best is shown to chase.
- Click / enter to continue back to the builder.
Tested in test_menu (results-state navigation). ctest 16/16.

## Articulated characters + themed map decoration
Applied logscope's smooth meshes to the bodies and the arenas:
- **Articulated character models** -- robot parts now use fitting smooth meshes
  instead of all-boxes: spheres for the head and shoulder ball-joints, tapered
  capped cylinders for arms/thighs/shins, chamfered (spherified) boxes for the
  chest/abdomen/back. The visor stays a crisp box. Player AND every enemy body
  get the upgrade, on top of the existing greeble detail pass.
- **Themed map decoration** -- each arena is now ringed with smooth periphery
  props: scattered rocks/boulders low and spires/crystals standing tall, tinted
  per theme (icy blue on frostline/vault, obsidian-red on the ember pit, gold on
  Cybertron, grey asteroids on the outer rim, rust on the wastes). Deterministic
  per map, so each arena has its own consistent dressing.
ctest 16/16.

## Distinct enemy silhouettes + arch landmarks
- **Per-kind silhouettes** -- enemy body types now read at a glance instead of
  being recolours of one shape:
  - Hijack: small, spindly swarm bug (~0.7x height), oversized sensor head,
    thin arms splayed high, jittery.
  - Corrupted: hunched and warped -- one huge dragging right arm, a withered
    left, lopsided legs, an over-grown asymmetric back mass.
  - Apex boss: towering (head ~2.9 vs player ~1.9) and heavily armoured, with
    massive pauldrons and a broad armour crest.
  Verified in test_gait (apex/hijack heights, corrupted arm asymmetry, apex
  width) -- 16/16.
- **Arch landmarks** -- each arena gets two standing arch monuments (a smooth
  half-torus) at opposite edges, tinted per theme, giving the maps a built
  focal point and a sense of place.
ctest 16/16.

## Hands, feet, and a visible weapon
- **Hands & feet** -- the body part system grew from 13 to 17 parts: rounded
  fists cap the forearms and chamfered feet ground the shins, on every body
  (player, all enemies, and stowed as hull nubs in vehicle mode). Limbs no
  longer end in mid-air.
- **Arm-cannon** -- the player now visibly holds a weapon (breech + tapered
  barrel + muzzle) in the right hand, aimed forward and tinted by the active
  Watch form's colour, so switching forms visibly changes the weapon glow.
ctest 16/16.

## Weapon shapes by form + tower landmarks
- **Weapon reflects the form** -- the arm-cannon now changes SHAPE by fire mode,
  not just colour: a single cannon for bolts, a triple-barrel for bursts, a wide
  focusing emitter for beams, a broad flat emitter for wave pulses, and a stubby
  angled mortar for lobs. Cycling the Watch forms visibly reshapes the weapon.
- **Broken-tower landmarks** -- a second landmark type joins the arches: tall
  stacked-segment towers with jagged broken crowns, three per arena at the far
  ring, themed-tinted. Together with the arches and periphery props the arenas
  now have a real skyline.
ctest 16/16.

## Armed enemies
- **Enemy weapons** -- ranged enemy kinds now visibly hold a weapon aimed at the
  player: grunts carry a bolt cannon, the Apex boss a larger heavier one (purple
  cast). Melee kinds (Corrupted, Hijack) stay empty-handed, so at a glance you
  can read who shoots and who charges. Weapons track the player and hide on
  death. Combat now reads gun-to-gun instead of bolts from empty hands.
ctest 16/16.

## Movie signatures: gated abilities + Clone codex
Ported the film logic that a clone's signature power is tied to its nature:
- **Star-fire is Billy's (the Fire Clone)** -- charging (hold reload) now only
  works in a FIRE-element form (fire/magma); the charge multiplies the next
  shot up to 6x. HUD shows the live multiplier.
- **The gravity leap is Ice-Clone-only** -- "quantum cold." It now works (it was
  previously unwired) and ONLY in an ICE-element form (ice/blizzard): tap K for
  a silent instant insertion forward, then a delayed gravity wave lands ~2s
  later and damages everything near the arrival point -- exactly the movie's
  "the wave isn't aimed, it just happens" design.
- **Photon transcends elements** -- the apex form maps to no element, matching
  the films (light is not a Clone element; the Watch unlocked it).
- 3 new codex entries: The Morpher (the Watch Bearer), Billy (the Fire Clone),
  and Photon. 16 codex entries total.
Form choice is now tactical: pick ice to leap, fire to star-charge. ctest 16/16.

## Cinematic menu: travelling the cosmos (Destiny-inspired)
The menu is no longer a static arena behind text -- it's a voyage:
- **Ship in space** -- a procedurally-built starship (hull, swept wings, twin
  engine nacelles, cockpit) cruises ahead through the space skybox, banking and
  rolling gently with pulsing blue engine glow, while the camera drifts. The
  arena/player/enemies are hidden; only the ship and starfield show.
- **Destinations, not just maps** -- browsing the map screen changes the very
  space you're travelling through (the skybox swaps per highlighted map), and
  each map is now a DESTINATION with a film-flavoured tagline ("the boundary
  lattice - edge of the system", "cybertron - the homeworld, at last"). The map
  screen reads "destination / enter to travel". Straight out of Destiny's "the
  cosmos is out there" ending.
ctest 16/16.

## Combat-drop match intro
Starting a match is now an insertion, bridging the menu voyage into gameplay:
- On launch, a ~2.6s cinematic plays: the camera swoops down from orbit to the
  spawn point while the dropship descends toward the arena, then banks and peels
  away, and control hands over. A "// DEPLOYING //" banner shows the destination.
- The player stands idle at the graph spawn during the drop; the space skybox
  and lighting are already the destination's, so it reads as arriving somewhere.
Ties the Destiny-style menu ("the cosmos is out there") to boots-on-the-ground.
ctest 16/16.

## Field-report fixes (big batch)
Addressed a round of playtest feedback:
- **Menu text was tiny in the corner** -- the whole VGA overlay was drawn into a
  fixed 256x256 top-left quad; it now stretches across the full window, so text
  is large and fills the screen (fixes both small text and "codex off the left").
- **Codex wrapped off-screen** -- codex entries now word-wrap to the grid width.
- **White cube bolts** -> two bugs: the emissive shader path accumulated tint
  across additive light passes (=white), and the bolt mesh was a cube. Fixed the
  shader to emit once (ambient pass only) and sample the texture; made white.png
  actually white and the bolt/glow a sphere. Bolts are now coloured glowing orbs.
- **Missing faces** (characters + mining destructibles) -- procedural meshes had
  mixed triangle winding; backface culling turned that into holes. Culling is now
  off (plenty of FPS headroom), so all faces render.
- **Menu had a white background** -- same emissive-texture bug; the space skybox
  now shows its texture behind the ship.
- **Full-auto firing** -- holding fire (key or left mouse) now shoots continuously
  at the weapon's fire rate. Chaotic and intense.
- **Left-click = Enter** -- a left click anywhere in a menu confirms the current
  selection, so mouse users never reach for the keyboard.
- **Camera clipped geometry** -- the third-person camera now booms behind the
  player with collision pull-in (against cover volumes + arena walls), so it
  never passes through objects.
- **Missing hitboxes** -- decoration props (rocks, spires, towers) now have AABB
  collision, not just the procedural cover.
- **Frame cap in Settings** -- new option: vsync / 60 / 120 / 144 / 240 /
  unlimited, enforced in the engine loop (stops a 240Hz display running at 400fps).
- **Transform animation** -- transforming now spins the body through the change
  with a squash and a spark burst, instead of an instant swap.
ctest 16/16.

## Massive vertical worlds + more vehicles + builder preview
- **Colossal maps** -- the arena grew from 170x170 to 480x480 with 140-tall
  walls. The procedural generator now raises GIANT vertical megastructures:
  multi-tier towers 14-40 units tall with walkable stepped ramps, big walled
  rooms with floating platforms above them (fly/jump up to reach), raised
  junction decks, and a colossal central megastructure with a 60+ unit spire.
  Jump height is boosted and flyers soar -- worlds you fall, climb, and launch
  through. Themed visuals and periphery landmarks scaled out to fill the space.
- **Vehicle repository: 16 archetypes** -- added mech (bipedal walker),
  hover-cycle, gunship (armed flyer), scorpion (stinger tail), wolf (fast
  quadruped), hawk (winged flyer), and crab (clawed skitterer) to the existing
  9. Selectable in the customizer.
- **Builder camera panning** -- the bot builder now shows a live rotating 3D
  preview of your transformer, and the camera smoothly PANS to whatever section
  you're editing: up to the head, in on the shoulders, down to the legs, and it
  even shows your chosen VEHICLE form when you land on that row.
- **Firing recoil** -- the arm-cannon now kicks back with each shot, pairing
  with the new full-auto fire for punchy gunplay.
ctest 16/16.

## Navigation radar + launch pads
- **Radar** -- with the world now enormous, the HUD has a top-right radar
  showing enemy blips (with the Apex marked 'A'), the central megastructure (#),
  and you at centre (@), north-up within a 140-unit range. Also fixed the
  crosshair, which was drawn off-screen.
- **Launch pads** -- glowing pulsing pads scattered across the map fling you high
  into the air (and forward) when you run onto them near the ground -- fast
  vertical traversal to reach the towers and floating platforms.
ctest 16/16.

## Lane map + rotating turrets + speed feel (MOBA DNA)
- **The Reclaim** -- a new map from the films' "organic-metal corridors": an
  overgrown Cybertronian world with three long high-speed LANES separated by
  dense jungle cover, objective platforms at each end, green canopy sky. Built
  for fast vehicle fights.
- **Slow-rotating defensive turrets** -- gun-plinths guard the lanes (and ring
  the megastructure on other maps). They track you SLOWLY and only fire when
  aligned and in range -- so at speed in a vehicle you can flank and outrun their
  aim. Get caught standing in a lane and they punish you. New turret model.
- **Speed FOV** -- the camera widens with velocity (up to +26 deg at full
  vehicle speed) for a real sense of ripping down a lane.
- **Far-plane fix** -- bumped 1000->4000 so the skybox and distant megastructures
  actually render in the massive world (they were being clipped).
- 2 new codex entries (the Reclaim, the Corridors); 18 total.
ctest 16/16.

## Big feedback pass: bolts, UI scale, feel, materials, map detail
- **Bolts fixed for real** -- introduced an `emissive_solid` material flag so
  effects (bolts/pads/glows) output PURE tint while skyboxes keep their texture.
  Bolts are now vivid coloured, elongated streaks aligned to travel, not white.
- **GUI scale** -- the VGA overlay now renders CENTERED and scaled by one knob
  (`ex_vga_scale`); default 68%, with a gui-scale slider (50-100%) in Settings.
  Fixes the "UI too big" regression.
- **Damage feedback** -- taking a hit flashes a red screen border, shows a
  directional arrow to the source, and names what hit you (TURRET / APEX SLAM /
  CORRUPTED / HIJACK / GRUNT) so you know what's killing you.
- **Jump & run feel** -- gravity lowered (100->60) for floaty, cushiony jumps;
  jump made more vertical; a landing squash cushions touchdown; the walk/run gait
  is more pronounced (bigger stride, arm swing, body bob).
- **Bigger play area** -- procedural extent 190->215.
- **Skybox banding fixed** -- all skies regenerated with per-pixel dithering.
- **Shiny metal** -- lit shader now has a metallic specular floor + a fresnel rim
  glint, so metal catches light and edges gleam.
- **Liquid pools** -- shimmering themed liquid surfaces (lava/water/coolant/
  energon) pool across maps, animated emissive.
- **FSM map detail** -- the blueprint's big structures (towers, the central
  megastructure) now get procedural greeble subcomponents (panels/vents/antennae
  /ridges) via the same detail generator as the robots -- intricate surfaces
  instead of plain giant boxes.
ctest 16/16.

## Config persistence, menu polish, planets, builder camera
- **Config persistence** -- keybinds + all graphics settings now save to a
  Linux-friendly XDG path ($XDG_CONFIG_HOME/cybertron/config.ini, else
  ~/.config/cybertron/config.ini) and load at startup. A "store configuration"
  button in the Controls menu writes it on demand; custom binds survive restarts.
- **Top-left UI anchoring** -- the HUD/menu grid anchors to the top-left again
  (was centred); the crosshair is repositioned to true screen centre for any
  gui-scale value.
- **Deep-space menu with planets** -- the front-end no longer shows a ground sky
  with stray boxes. It's now proper deep space: the ship cruises past Earth,
  Jupiter, Cybertron, Mars, and Pluto (procedural planet textures + spheres),
  each slowly spinning. All decoration is hidden.
- **Pause no longer wrecks the map** -- opening the menu mid-battle used to run
  the cinematic backdrop, which relocated and hid map geometry (the "turns white
  and persists" bug). PAUSED/RESULTS now freeze the live world behind the overlay.
- **Builder camera + backdrop** -- the customization camera now frames from a
  cinematic 3/4 angle ABOVE the subject (no more looking up from underneath the
  vehicles), against a space backdrop with Cybertron hanging behind the bot.
- **Vehicle texture stretch** -- moderated the elongated creature-hull proportions
  and doubled bevel-box UV tiling to reduce the stretched-bottom look. (A full fix
  needs triplanar mapping, noted for when it can be visually verified.)
ctest 16/16.

## Articulated transform morph + colored dynamic lights
- **Part-by-part transform** -- transforming no longer teleports parts to the
  new configuration. Both the robot and vehicle poses are computed and every part
  is interpolated between them with STAGGERED per-part timing, so parts fold in
  sequence -- each spinning and pitch-tumbling quickly, then settling into place.
  Parts that only exist in one mode fade in/out. Blend lengthened to 0.7s so the
  cascade reads. (No more "some parts teleport at the finish".)
- **Form-coloured player light** -- a dynamic point light follows the player,
  tinted by the active Watch form's element colour (ice blue, fire orange, etc.),
  and flares brighter on each shot -- a coloured muzzle glow thrown on the world.
- **Apex boss light** -- the Apex boss now casts a pulsing menacing purple light
  while alive, hidden when it dies.
- Gameplay lights are suppressed in the menus so they don't bleed into the
  space/builder backdrops.
ctest 16/16.

## CRITICAL FIX: startup crash (buffer overflows + missing planet load)
Fixed the segfault-on-startup. Three real bugs, found via gdb:
1. **`ex_texture_load` off-by-one** -- the path buffer was
   `char[len + strlen(name)]`, one byte too small for the null terminator, so
   every texture load overflowed by a byte. Latent for ages (this is why the
   game never rendered in the headless sandbox); a newer texture-name length
   shifted the overflow onto something critical and it started segfaulting.
   Fixed to `+1`.
2. **`iqm.c` spec/norm name off-by-one** -- same bug building "spec_<tex>" /
   "norm_<tex>" names. Fixed to `+1`.
3. **Planets were never loaded** -- an earlier edit to add the planet model
   loading silently failed to match (whitespace), so `g_planets[]` was declared
   and referenced in the menu backdrop but never populated -> NULL deref on the
   first frame. Added the load loop.
With these fixed, the game now runs past init and renders the menu (verified in
the sandbox for the first time). ctest 16/16.

## Menu fortify, faster combat, guns, flight/racing, transform flicker
- **Menu controls fixed** -- the mouse->row mapping ignored the GUI scale (grid is
  top-left anchored + scaled), so hover landed on the wrong rows; that also caused
  accidental rebinds (hovering "layout" actually sat on a movement action), which
  is what broke .oeu/wasd. Now scale-correct. Also, hover only moves the selection
  when the mouse actually MOVES, so it no longer fights the arrow keys.
- **Transform box flicker fixed** -- fading transform parts hit scale3=(0,0,0),
  which made the renderer fall back to the uniform scale (1.0) and pop a full-size
  box over the animation. Zero-scale parts are now hidden.
- **Fast, intense combat** -- fire cooldowns roughly halved across all forms, bolt
  speed 65->115, so full-auto rips. **Barrel heat**: the weapon glows from its form
  colour toward white-hot orange under sustained fire and cools when idle.
  **Turret bombardment**: defensive guns fire ~2x faster with spread, saturating
  the area around you with suppressive tracers (only direct hits damage).
- **Better guns** -- fixed a tint bug (weapon colour was 0-255 stuffed into a 0-1
  field, so guns rendered white); added a sight rail, front post, heat vents, and
  grip to the cannon model.
- **Flight + racing** -- flying vehicles (jet/hawk/gunship/dragon/hover/cycle) now
  truly FLY: gravity nearly cancelled, hold jump to climb / crouch to dive, thrust
  along the full aim vector. Ground racers are 1.6x faster with less friction for a
  drifty, high-speed feel.
Deferred: outer_rim themed-mesh collision (needs mesh->collision integration),
character-model rework, and combat SFX (audio assets). ctest 16/16; runs clean.

## Themed-map collision (fixes outer_rim walk-through blockers)
The themed visual maps were collision-less -- you could walk through the big
rectangles/blockers on outer_rim (and every other themed map). Fixed generally:
- The map generator (`Builder.box`) now records every solid blocker as an AABB
  and emits a `<map>.col` manifest alongside each `.iqm` (floors/walls excluded
  -- the base hull already handles those; tiny greebles skipped).
- On match start the game loads the active map's `.col`, scales the AABBs by the
  2.7x themed-map render scale so collision matches the visuals, and resolves the
  player against them (stand-on-top + push-out, same as procedural cover).
Now the baked blockers on all 10 themed maps are solid. ctest 16/16; runs clean.

## Character models: dedicated head + hand meshes
The head and fists were plain spheres. Replaced them with purpose-built meshes:
- **Robot head** -- an angular cranium with a protruding faceplate, brow ridge
  over the visor, a top crest fin, two side antennae, and a jaw. Reads as a real
  transformer head instead of a ball.
- **Mechanical hand** -- a blocky palm with four knuckle segments and a thumb,
  instead of a sphere.
Both are routed via `part_template`, so every robot in the game -- player,
grunts, Corrupted, Hijacks, the Apex boss -- gets the upgrade. Shoulders stay
rounded pauldrons. ctest 16/16; runs clean.

## Combat sound (procedural SFX)
The engine only loaded .ogg; I added a minimal 16-bit PCM **WAV loader** to the
sound module (mono/stereo, chunk-scanned) plus an `ex_sound_gain` helper. Then
generated 8 procedural sound effects (pure-Python synth -> WAV) and wired them to
gameplay:
- **fire** (laser pew, retriggered per shot -> machine-gun stutter on full-auto)
- **turret** (heavier thud from the stationary guns' bombardment)
- **hit** (metallic clank when you take damage)
- **explosion** (low rumble on enemy death)
- **transform** (mechanical servo sweep on robot<->vehicle)
- **launch** (upward whoosh off launch pads)
- **pickup** (chime on energon/shield/overcharge)
- **jump** (soft thump)
Master volume drives them (Settings slider). Verified all 8 load and the game
runs; ctest 16/16.

## Richer map composition: two levels, pillars, objectives, jungle pockets
Big map-design pass -- the procedural blueprint now composes intentional zones
(95 collidable volumes per match, all render + block automatically):
- **Two-level sky decks** -- two big walkable platforms held ~20 units up on four
  giant corner pillars each, reached by a long stepped ramp, with low cover walls
  on top. A real upper level with corners and vantage over the fields.
- **Giant pillars** -- six thick standalone columns carve the open fields into
  corners and break long sightlines.
- **Vantage perches** -- four elevated platforms at the arena corners (reach them
  by flight or a launch pad) for a commanding sniper view.
- **Objective spawn pads** -- three pulsing cyan pads at a forward point and two
  flanks, marking where future capture objectives will spawn; shown on the radar
  as 'O'.
- **Jungle power-up pockets** -- four clusters of pickups (incl. overcharge) out
  in the hazard sections between the main engagement/racing areas -- worth the
  detour.
- **Environmental hazard** -- hot pools (lava/energon) now chip your health while
  you stand in them, tagged HAZARD in the damage feedback.
Open fields and the far lanes remain between these features. ctest 16/16; runs.

## Terrain-traversal smoothness harness
Added an automated harness that stress-tests the character's terrain stepping for
the camera/step "pop" bugs, and refactored the step logic so it's actually tested:
- **Shared `terrain_step` module** -- the character-vs-AABB step resolve (clamp
  onto a surface within the step tolerance, else push out along the least-
  penetration axis) was extracted from game.c into `exengine/terrain_step.c`. The
  game now calls it, so the harness exercises byte-identical logic.
- **`tests/harness_step.c`** -- generates 8 obstacle-course patterns (flat, low
  staircase, fine ramp, tall walls, random blockers, platforms+gaps, tight
  overlapping cluster, descending ledges), then drives a simulated character
  across each along many parallel walk-lines at 3 speeds, recording elevation
  frame-by-frame. It scans that signal for NON-PHYSICAL changes -- motion should
  be parabolic (free-fall) or piecewise-linear (ramps) with small bounded step-
  snaps, so it flags:
    - fall-through (drops below the floor),
    - pops (a 1-2 frame up-then-down spike -- the camera pop),
    - unexplained jumps (a single-frame change bigger than velocity + one legal
      step-snap can account for),
    - jitter (elevation oscillating while grounded).
  It reports peak elevation + anomaly counts per course and exits non-zero on any
  anomaly -- registered as a ctest (now 17 tests). The chase camera's height is
  character_y + constant, so a smooth character elevation == a smooth camera.
- **Current result:** ALL COURSES SMOOTH -- 0 fall-throughs/pops/unexplained/
  jitter; low steps + ramps climb cleanly (peak 3.6/4.0), tall walls correctly do
  NOT (peak 0), and tight overlapping clusters produce no oscillation. Verified
  the detector itself catches an injected 2.0-unit teleport.

## Game-feel / juice pass (toward AAA)
- **Camera trauma shake** -- a trauma value builds on firing (small), taking a
  hit (jolt), nearby enemy kills/explosions (scaled by distance), and hard
  landings, then decays. Applied as a squared-falloff screen-space shake offset
  along camera-right/up, so the framing shakes punchily WITHOUT altering aim.
- **Muzzle flash** -- a bright emissive burst blooms at the barrel tip on each
  shot, sized by recoil.
- **Idle life** -- when standing still, every robot now breathes (slow chest
  rise/fall) and shifts weight side-to-side, instead of freezing as a statue.
- **Turn banking** -- the player's body smoothly leans/rolls into turns based on
  how fast the aim is sweeping.
- **Enemy hit flash** -- enemies blow out toward white for a couple frames when
  damaged (hitscan, chain arcs, and the gravity-wave AoE), giving clear impact
  feedback.
ctest 17/17; runs clean.

## Atmosphere + impact VFX pass
- **Impact sparks** -- every shot that lands (on an enemy or geometry) throws a
  small burst of sparks at the exact hit point, so fire connects visibly.
- **Atmospheric motes** -- 44 slow-drifting emissive particles wrap in a box
  around the player and rise like embers/dust, tinted per theme (ember=orange,
  frost=pale blue, cyber/reclaim=green, outer-rim=cool white, wastes=dust). Gives
  the air depth and motion instead of dead empty space.
- **FOV kick** -- firing briefly punches the FOV in (and the speed-FOV response
  is snappier), selling recoil and velocity.
ctest 17/17; runs clean.

## Movement rework, lighting fix, dark menu, origami transform
- **Movement controller rebuilt** -- the old impulse+friction model accelerated
  inconsistently by direction (the "only certain radial angles" bug) and had no
  clean top speed. Replaced with a target-velocity controller: inputs build a
  normalized wish-direction, and velocity accelerates toward wish*max_speed the
  same in every direction. Clean caps: robot 13, ground racer 20, jet 26 u/s.
  Jet vertical follows aim pitch + jump/crouch. Jump reduced 27->18. Fixes the
  driving AND flying acceleration/rotation feel in one place.
- **White characters fixed** -- the specular had a high metallic floor
  (spec*0.55+0.45, x1.6) that washed surfaces white, worst on big maps where the
  point lights attenuate to nothing far from centre. Lowered the floor/gain and
  added a proper **directional sun** in the ambient pass (no attenuation), so
  characters are shaped consistently everywhere instead of blowing out.
- **Menu is true dark space** -- arena FEATURE models and pickups weren't hidden
  in the menu backdrop (the "map half-loaded" bleed). Now hidden; the front-end
  is clean deep space with the planets.
- **Transform: origami-style folding** -- applying the action-origami principle
  ("a mechanism articulating into a base"), each part now DETACHES outward, arcs,
  crimps (compresses), and refolds into its target with a wider staggered cascade
  and two-axis hinge rotation -- a mechanical fold sequence, not a spin in place.
  Duration lengthened 0.7->1.3s so it reads.
ctest 17/17; runs clean.

## Root-cause: the "everything white" bug (+ menu/UI fixes)
- **Exposure sign fixed (the big one)** -- the final composite (fboshader.glsl)
  computed `1.0 - exp(tex_color / white_point)` -- MISSING the negative sign. For
  any pixel brighter than ~0.1 that goes negative, and the ACES pass after it
  flips negatives past 1.0 -> pure white. It stayed hidden while the scene was
  kept very dark; the new directional sun (and bright cybertron characters)
  pushed brightness over the threshold and exposed it. Corrected to the proper
  `1.0 - exp(-tex_color / white_point)` with a gentle white point (1.6). This
  fixes the white menu AND the white characters in one place and gives the whole
  game a correct tone curve.
- **Startup mouse nav fixed** -- `ex_menu_init` memset rebinding to 0, but the
  code reads `rebinding >= 0` as "currently rebinding action 0", which disabled
  mouse hover until a real rebind reset it to -1. Now initialized to -1.
- **GUI scale floor lowered 50% -> 25%** -- slider and the vga/mouse/crosshair
  clamps all go down to 0.22 so the HUD/menu can shrink much smaller.
ctest 17/17; runs clean.

## Jump-on-stairs, character rotation, programmatic AABBs, 3x maps + interiors
- **Jump on stairs/platforms fixed** -- the shared step-resolve returns whether
  you're standing on a box, but resolve_player_vs_volumes ignored it, so
  `grounded` was never set on stairs and jump (which needs grounded) failed.
  Now standing on any stepped surface counts as grounded.
- **Character faces the camera now** -- mouse-look (ex_fps_camera_update) ran at
  the END of the frame, so the body was posed with last frame's aim while the
  camera used this frame's -> the character lagged/appeared not to rotate. Moved
  the look update to the TOP of the frame so the body faces the current aim.
- **Programmatic bounding boxes** -- the asset generators now compute each mesh's
  local AABB from its vertices and emit `asset_aabb.txt`. At runtime a loader +
  `auto_collider(mesh, pos, scale)` derives a world collider = local_half *
  scale for any placed asset -- so varied/microFSM-tuned meshes get decent
  boxes automatically. Applied first to the 36 arena cover features, which had
  NO player collision before (you walked through them); they're solid now.
- **Maps 3x larger** -- arena hull 240->720 half, camera clamp 235->705,
  blueprint radius 215->645, theme visual scale 2.7->8.1, enemy spawn range 3x,
  and every fixed structure (decks, giant pillars, perches, objective pads,
  jungle pockets) spread x3. Verified: runs, spawns in-bounds, stepping harness
  still smooth.
- **Interior rooms + control panels** -- three enclosed rooms (walls with a
  doorway gap, a cover crate inside) are now part of the blueprint, each with a
  pulsing amber control-panel console. Collidable corridors and little rooms to
  fight through.
ctest 17/17; runs clean.

## Dynamic gait: lean, drift, and head tracking
The walk/run pose now reacts to how the player is actually moving and looking:
- **Forward/back lean** -- the body reads its velocity along the facing direction
  and tips forward when driving ahead, back when braking. Pivots at the feet
  (upper parts shift proportional to height), so the whole silhouette leans.
- **Drift bank (friction feel)** -- lateral velocity relative to facing banks the
  body into the slide. When you whip the camera and change direction, the body
  keeps leaning into the old velocity until friction/steering pulls it in line --
  the drift reads visually. Also the subtle turn-bank into fast camera sweeps.
- **Independent head tracking** -- the head pitches to follow the camera's up/down
  look on its own, so the robot looks where you aim rather than staring level.
- All lean is suppressed during a transform so it doesn't fight the fold anim.
(Ground-angle lean is a no-op until terrain elevation exists -- floors are flat;
noted for when slopes land.) ctest 17/17; runs clean.

## Multiple cameras with separate state (vehicle-select camera fix)
The engine rendered every view through one shared camera whose state carried over
between screens -- so the builder/customize view inherited "whatever was on screen
last" and swept in from a stale pose. Added a per-mode camera system:
- Three modes -- CAM_GAME (in-match), CAM_MENU (front-end), CAM_BUILDER
  (customize/vehicle select) -- each with its own saved position/front/yaw/pitch/
  fov. `use_camera(mode)` saves the outgoing camera and restores the incoming
  one on every transition, so modes never contaminate each other.
- The builder's turntable smoothing snaps to its framing on fresh entry instead
  of sweeping from a leftover pose.
- Each match starts with a clean in-match camera facing the arena centre, never
  inheriting a stale aim from a prior match or menu.
(True separate SCENE graphs remain a larger refactor; the per-mode camera plus
the existing per-mode model hiding already isolates the views.) ctest 17/17.

## Character classes / 4 gated profiles
Added a class system (`exengine/pclass.c`, test_class -> 18 tests) with four
profiles, each gating vehicles, builds, and base stats:
- **VANGUARD** -- heavy ground assault (tank/mech/car/crab), HP x1.40, speed x0.85,
  bulky builds only.
- **INTERCEPTOR** -- aerial flanker (jet/hawk/cycle/hover), HP x0.75, speed x1.30,
  lean builds.
- **TITAN** -- beast bruiser (dragon/dino/raptor/wolf), HP x1.20, speed x1.00.
- **SPECIALIST** -- tech/support (gunship/spider/scorpion/insect), HP x0.90,
  speed x1.05.
Wiring: the customize screen has a `profile` row; switching class re-gates the
vehicle to that class's four forms and clamps the build's bulk into the class
range. Vehicle cycling only steps through the allowed set. Class HP and speed
multipliers apply at match start (Vanguard really is tankier, Interceptor really
is faster), and the class name shows on the HUD. Builds and vehicles a class
can't use simply aren't reachable.
ctest 18/18; runs clean.

## Apex boss choreography (ported from the film's boss-fight design)
Translated the source docs' "timed pulse" boss pattern into an original,
choreographed encounter for the Apex boss:
- **Resonance pulse on a 9s cadence**, telegraphed ~1.2s ahead (warning text +
  the boss light flares white-hot + the boss visibly SWELLS as it charges).
- Four **cycling attack types**, so the fight has a readable rhythm:
  1. RESONANCE FIELD -- a paralysis wave that slows the player to 40% for 2.5s
  2. KINETIC HURL -- a heavy debris projectile flung at the player
  3. PLASMA BURST -- a five-bolt fan sweeping the player's lane
  4. GRAVITON WELL -- yanks the player bodily toward the boss
- Learn the count, dodge the telegraph, punish the recovery. Each pulse kicks the
  camera. Pulse state resets cleanly at match start. New codex entry explains it.
All content original-named (Apex / resonance pulse), no trademarked material.
ctest 18/18; runs clean.

## Distinct Apex boss silhouette
Gave the boss a look to match how it fights:
- **Monstrous proportions** -- the Apex is now hunched (0.30) with broader mass
  (bulk x1.95, shoulders x2.05) and longer reach (arms x1.55), reading as a
  towering brute rather than a scaled-up grunt.
- **Spiked crown** -- a rotating ring of eight blades hovers around the boss's
  shoulders, fanned outward. The spikes stretch and glow hotter (purple) as a
  resonance pulse charges, tying the silhouette to the telegraph. Hidden when the
  boss dies or in menus.
ctest 18/18; runs clean.

## Texture-based emission (self-lit surface detail)
Added a real emission channel to the renderer, pairing with the directional
light for a proper lit + emissive material model:
- **Engine:** meshes gained a `texture_emission` slot; the renderer binds it on
  texture unit 8 (default black = no glow), and the IQM loader auto-loads
  `emit_<texture>` by naming convention (like spec_/norm_).
- **Shader:** the emission map is sampled once in the ambient pass and added
  additively (x2.2), so lit surfaces can have glowing detail that isn't affected
  by scene lighting -- and (now that the exposure bug is fixed) it blooms toward
  white through the tonemap the way real emission should.
- **Maps generated:** `emit_plate_blue` (cyan circuit traces + indicator dots on
  the robot armour), `emit_wall` (panel status lights + seam glow), `emit_hazard`
  (glowing amber hazard stripes). Any texture without an emit_ map simply
  doesn't glow -- fully backward-compatible.
So the robots now carry live glowing tech detail and hazard surfaces read hot,
via a texture channel rather than the all-or-nothing emissive flag.
ctest 18/18; runs clean.

## MicroFSM customization variety: chassis/frame variants
Added a frame system (`exengine/chassis.c`, covered by test_class) so builds vary
in silhouette, transform feel, and glow -- not just slider tweaks:
- **Four frames** -- STANDARD (baseline), ANGULAR (lean, sharp, snappy fast fold),
  JUGGERNAUT (broad, heavy, slow ponderous fold), STRIDER (tall, slim, fluid).
- **Silhouette** -- each frame multiplies the player's build proportions
  (height/bulk/arm/leg/head/shoulders) on top of the user's sliders, so two bots
  with identical sliders still read as different machines.
- **Transform style** -- the frame tunes the microFSM fold: ANGULAR uses a tight
  stagger + big fast hinge spin (snappy), JUGGERNAUT a wide stagger + low spin
  (heavy), etc. Same origami fold system, different personality per frame.
- **Glow** -- the frame scales the emission/accent brightness (ANGULAR glows hot,
  JUGGERNAUT runs dark).
- Selectable via a `frame` row in the customizer, alongside the class `profile`.
ctest 18/18; runs clean.

## Per-model emission strength + themed emission maps
- **Per-model emission strength** -- models gained an `emission_strength` field
  (default 1.0), uploaded as `u_emission_strength` and multiplied into the
  emission term. The chassis frame's glow value now drives the player's emissive
  circuits directly (ANGULAR bots glow hot, JUGGERNAUT run dark), and any model
  can tune its own glow intensity.
- **Themed emission maps** -- added glowing detail to the environments:
  `emit_cybertron` (cyan circuit veins + nodes), `emit_rust` (magma cracks for
  ember/foundry), `emit_energon` (pulsing green energon), `emit_reclaim`
  (bio-luminescent flecks for the organic-metal corridors). Seven emission maps
  total; surfaces without one stay unlit as before.
ctest 18/18; runs clean.

## Story foundation: robot language, cutscenes, 6-film campaign, human scale
- **Procedural robot language** (`exengine/voice.c`) -- text is mapped
  deterministically to a warbling gargle: every letter has fixed phoneme
  characteristics (vowels = two-formant voiced tones, consonants = hums/noise
  bursts), ring-modulated for a wet robotic warble. The SAME word always produces
  byte-identical audio (verified) -- a consistent constructed language, not random
  noise. Four speaker voices (pitch-shifted). Synthesized to PCM at runtime and
  played via a new `ex_sound_pcm` raw-buffer path.
- **Cutscenes + subtitles** -- a cutscene player shows the film's title, one
  subtitle line at a time (auto-advancing on a reading timer, or enter/click to
  skip), and VOICES each line in the robot language as it appears, over the
  deep-space backdrop.
- **6-film campaign** -- expanded the campaign from 4 to 6 missions, one per film
  of the arc (Doomsday -> Dominion -> Destiny -> First Light -> Reclamation ->
  Last Dawn), each opening with its story cutscene. The finale is the Apex boss
  with the resonance-pulse choreography. All script text original.
- **Human-scale foundation** -- an engine mode (toggle H, or set per-mission) that
  shrinks the player to ~0.3x, paces movement to a human stride, tightens/lowers
  the camera, and disables transforming -- the groundwork for the campaign's
  human-scale sections against the titan-scale world.
ctest 18/18; voice determinism verified; runs clean.

## Human character + human-scale physics
Human-scale mode is now a real human, not a shrunk robot:
- **Human body** -- added `EX_BODY_HUMAN`: slender upright proportions (bulk
  x0.42, shoulders x0.68, no hunch) with a muted suit grey-blue palette instead
  of metallic tones. Used automatically whenever human-scale is active.
- **No mechanical greebles** -- the thousand-part detail pass is suppressed on a
  human body (they're not machines).
- **Human physics** -- toggling human scale shrinks the entity's collision
  ellipsoid (0.5,1.0,0.5 -> 0.28,0.5,0.28) and the volume-collision radius
  (0.9 -> 0.35), so a human fits through and stands in human-sized spaces rather
  than colliding like a titan.
- Combined with the earlier pass (0.3x visual scale, human-paced movement,
  tight/low camera, transform locked out), human scale is now a coherent mode.
Still foundational: the head/hands reuse the shared part meshes (humanoid via
proportions, not a bespoke human sculpt), and maps are still titan-scale --
human-sized doorways/cover/interiors are the next step.
ctest 18/18; runs clean.

## Co-play scaling: bigger maps + human-scale cover
Groundwork for humans and bots sharing one map (they'll co-play), which means one
world that reads at both scales:
- **Maps scaled up further** -- arena hull 720->900 half, camera clamp 705->885,
  blueprint radius 645->810, theme visual scale 8.1->10.1, enemy spawn range
  widened. Giant bots get room to manoeuvre; verified spawns in-bounds + stepping
  harness still smooth.
- **Human-scale cover clutter** -- 60 small crates/barriers (1.6-3.4u, human-tall)
  scatter across each map at ground level, collidable via the programmatic AABB
  system. The co-playing HUMANS fight around them for cover; the titan bots simply
  step over. Re-scattered per match (per-match colliders reset above the fixed
  arena features), hidden in menus. Auto-collider cap raised to fit them.
So the same map now carries titan-scale structures (for bots) AND human-scale
cover (for humans) -- the spatial groundwork for mixed-scale play.
ctest 18/18; harness smooth; runs clean.

## Capturable objectives (control/scoring loop)
The three objective pads are now a live control mechanic, turning the arena into
a scoring loop:
- **Capture** -- stand on a pad to fill its capture meter; the pad's colour
  shifts neutral-grey -> cyan as you take it. Capture radius is tighter at human
  scale (co-play aware).
- **Contest** -- an enemy on the pad stalls/reverts the capture; a pad held by
  both flickers amber (contested).
- **Hold to score** -- owned pads accrue a control score over time and
  trickle-heal the holder standing on them. Lose one and it flips back neutral.
- **HUD** -- shows control (pads held / total) and the running control score,
  plus the existing radar 'O' markers.
Groundwork for objective-based win conditions and mixed-scale skirmishes over
map control. ctest 18/18; runs clean.

## Objective win condition (control missions)
Completed the control loop with a real victory path:
- **EX_OBJ_CONTROL** -- a new mission objective: hold the objective pads to
  accrue a control score, and reaching the target (60) wins the mission, exactly
  like clearing all enemies does for eliminate missions.
- **Mission 3 "hold fast"** is now a control mission -- fitting the theme (hold
  the ground) -- so the campaign teaches the capture loop mid-arc.
- **HUD** switches to a "HOLD  ctrl held/total  score/target" readout during
  control missions so the goal is legible.
Capture -> contest -> hold -> WIN is now a complete gameplay mode, and the
objective system is ready for skirmish control modes and mixed-scale point
fights. ctest 18/18; runs clean.

## Mixed-scale co-play: human NPCs share the map with titans
The through-line milestone -- humans and bots now occupy one map at once:
- **Human NPCs** -- 5-8 small figures spawn scattered across each map (more the
  deeper into the campaign). Each is a cheap 4-part humanoid (head/torso/legs/pack)
  posed at human scale with a walking gait, so they read as tiny people among the
  giant robots.
- **Behaviour** -- they idle-wander toward roaming targets, and FLEE directly away
  from the nearest titan (a live bot enemy, or the player when the player is in
  titan scale). When you stomp toward them as a robot, they run.
- Re-scattered per match, hidden in menus. Rendered from the shared part
  templates (sphere head, bevel torso, limb legs) so no new assets.
This proves the mixed-scale world: the same arena now holds titan-scale bots,
human-scale cover, capturable objectives, AND human-scale NPCs simultaneously.
Foundational: NPCs are ambient (no combat/collision yet) -- next is making them
targetable/protectable and wiring them into objectives.
ctest 18/18; runs clean.

## Human NPCs become gameplay actors (protect objective)
The co-play humans are now stakes, not just ambience:
- **Vulnerable** -- an enemy bot that reaches a fleeing human catches it (red
  impact burst, survivor lost). The PLAYER never harms them -- you're the
  protector; kill the pursuers before they close.
- **Survivor HUD** -- "survivors X/Y" tracks how many remain, turning amber as
  you lose them.
- **EX_OBJ_PROTECT** -- new objective. Mission 5 "reclamation" is now a protect
  mission: clear the threat while keeping the survivors alive; lose every
  survivor and the mission is failed.
So the mixed-scale world now has real interaction between scales -- the giant
bots threaten the little people, and the player's job is to stand between them.
ctest 18/18; runs clean.

## Last-second rescue window for survivors
Made the protect loop dramatic instead of punishing:
- **Downed, not dead** -- an enemy reaching a human now knocks it DOWN (prone,
  blinking red distress) and opens a ~5s rescue window instead of an instant loss.
- **Rescue** -- drive the attacker off (get the nearest enemy >14u away, e.g. by
  killing or body-blocking it) within the window and the survivor recovers ("survivor
  rescued"). Fail to clear the threat in time and they're lost.
- Turns each save into a tense sprint -- exactly the protector fantasy of a titan
  shielding the small from other titans.
ctest 18/18; runs clean.

## Vehicle handling feel + robot silhouette
- **Per-vehicle handling** -- each of the 16 archetypes now has its own feel
  (`ex_vehicle_handling`): a speed multiplier, an acceleration/responsiveness
  rate, and a lateral grip. Tanks/mechs are slow and locked (0.68-0.72 speed,
  low accel, full grip); cycles/hawks are fast and nimble (1.26-1.35 speed, high
  accel); hovers float and slide (0.45 grip). Driving a tank now feels nothing
  like a hover.
- **Drift bank** -- low-grip vehicles lean harder into lateral slides, selling
  the drift visually.
- **Speed rush** -- FOV widening and a chase-camera pull-back now build from
  ACTUAL velocity (smoothed), so acceleration reads as accelerating; both scale
  up more for vehicles.
- **Robot silhouette** -- broadened and raised the pauldrons for a more powerful,
  heroic stance (scales with the shoulder build slider, so humans stay narrow).
ctest 18/18; runs clean.

## Vehicle physics: suspension, g-force camera, turn radius, drag
A full physics-feel layer built from acceleration:
- **G-force tracking** -- per-frame acceleration is decomposed into longitudinal
  (accelerate/brake) and lateral (cornering) g, smoothed.
- **Spring-damper suspension** -- weight transfer drives a real spring-damper:
  the body squats/dives under throttle/brake (pitch), rolls out of corners
  (roll), and heaves over bumps. Not a lerp -- actual spring + damping integration.
- **Terrain bumps** -- procedural road jostle (flat ground, simulated) feeds the
  suspension, scaled by speed and position, so fast driving gets rougher.
- **Weighted g-force camera** -- the chase camera lags under load: it shifts
  opposite the acceleration (you feel pushed back / thrown sideways) and BANKS
  the horizon into corners via a rolled up-vector. The weight is a smoothed
  follow, so it loads up and settles rather than snapping.
- **Turn radius** -- fast vehicles can no longer spin their velocity on a dime;
  the heading swings at a limited rate that widens with speed and tightens with
  grip -- a real turning arc.
- **Coasting + aero drag** -- off-throttle vehicles roll down on drag (momentum
  carries) instead of braking; flight has quadratic aerodynamic drag for glide
  and a soft speed cap.

## Bullets: longer range + damage falloff
Weapon reach x1.8 (for the bigger maps), but damage now tapers with distance --
full up close, down to 45% at extreme range -- so positioning matters.
ctest 18/18; runs clean.

## Centralized "feel" system (relate the forces)
Replaced scattered physics magic-numbers with ONE tuning config (`exengine/feel.c`,
test_feel -> 19 tests). A few intuitive base dials drive everything through real
physical relationships, so tuning one value keeps the rest coherent:
- **Base dials:** mass, spring_hz, damp_ratio, g_show, cam_lag, grip, drag, turn.
- **Derived automatically:** the suspension spring constant AND its damping come
  from spring_hz + damp_ratio (critical-damping relation `2*zeta*w`), so a stiffer
  spring stays stable. Mass simultaneously widens turns, deepens suspension travel,
  and adds camera weight. g_show scales the camera g-shift, the body lean, and the
  horizon bank together. drag scales both coast and aero.
- The vehicle physics, suspension, and g-force camera all read the getters now;
  future feel-driven systems use the same config, so everything shares one truth.
- test_feel verifies the relations hold (damping tracks stiffness; heavier = wider
  turns + more travel; a critically-damped spring settles without overshoot).
ctest 19/19; runs clean.

## Campaign: dialogue characters
The six-film cutscenes are now character-driven conversations, not anonymous lines:
- **Named cast** -- WARDEN (your commander, steady voice), LARK (scout companion,
  quick high voice), THE MAW (the devourer/antagonist, deep slow voice), and REYES
  (a human captain ally -- tying the human co-play into the story). Each has a
  distinct procedural voice.
- **Character dialogue** -- each mission opens with a 3-4 line exchange BETWEEN the
  cast (banter, threats, resolve) rather than narration, with the speaker's name
  shown above each subtitle and voiced in their pitch.
- Combined with the per-mission maps (wastes/foundry/emberpit/vault/reclaim/
  cybertron) and objective variety (eliminate/control/protect/boss), each of the
  six levels now has its own place, cast, and goal. All script original.
ctest 19/19; runs clean.

## MicroFSM battle damage + ragdoll debris + repair
Parts can now be damaged, break off, and heal -- turning the microFSM body into a
damage model:
- **Per-part integrity** -- every body part has integrity (1..0). Taking a hit
  chips 1-2 extremity parts (arms, shins, shoulders, head, back, hands) and
  scatters small metal chips.
- **Joint sag** -- a damaged-but-attached part (integrity < 0.6) droops at the
  joint: it hangs forward, sags down, and rolls loose -- visible battle wear.
- **Break-off ragdoll** -- a part driven to 0 integrity SEVERS: it vanishes from
  the body and a tumbling metal chunk spawns as debris.
- **Ragdoll debris pool** -- broken chunks + impact chips fly off with random
  velocity/spin, fall under gravity, bounce off the ground with friction, tumble,
  and shrink out. (48-slot pool, bevel-mesh chunks.)
- **Two-tier repair** -- minor damage SELF-HEALS over time but only partway
  (to 70%); severed parts and full repair need ENERGY. Energon pickups bank
  repair energy (energy intake), and holding objective pads trickles it in
  (map control = a repair foothold). Enough energy regrows a severed part.
- HUD shows repair energy % and broken-part count.
ctest 19/19; runs clean.

## Damaged transforms, death ragdolls/explosions, live feel tuner
- **Damaged transforms** -- the origami fold reconfigures the LIMBS into the
  alt-mode, so a SEVERED structural limb (thigh/shin/arm) blocks the transform:
  you get "CAN'T TRANSFORM: <part> severed" + an error jolt. Damaged-but-attached
  limbs still fold, but JUDDER and lag (ragged transform), so a beaten-up robot's
  change looks strained.
- **Death ragdolls + explosions** -- enemies now explode on death (VFX burst +
  6 debris chunks + shake). The PLAYER's death blows every remaining body part off
  as tumbling ragdoll debris under a big secondary explosion (14 chunks, 2.2x).
- **Live feel tuner** -- F1 opens an in-match overlay listing the g_feel dials;
  `[ ]` selects, `- =` adjusts, and changes hit the physics/camera immediately so
  you can dial the feel in while driving.
ctest 19/19; runs clean.

## Explosive shockwave force
Every explosion now emits a radial shockwave, so big impacts feel big:
- **Force with falloff** -- flings nearby ragdoll debris up and outward, shoves
  enemies away from the blast and damages them (distance falloff), and knocks the
  player back (velocity impulse + a little lift).
- **Visual ring** -- an expanding emissive shockwave ring blooms from each blast
  and fades as it grows (6-slot pool).
- Wired through spawn_explosion, so enemy deaths, the player death ragdoll, and
  any future explosive source all propagate force automatically. Radius/force/
  damage scale with the explosion size.
ctest 19/19; runs clean.

## Hand-to-hand clash (quick-time events)
Close-quarters combat that triggers when an enemy closes to melee range:
- **Trigger** -- when a live enemy gets within ~7u (bot scale), a CLASH begins:
  the world-movement locks and a quick-time prompt appears.
- **QTE** -- a 2-4 step arrow-key "movement pattern" (UP/DN/LT/RT) shows with a
  draining timing bar. Input the sequence in order before it runs out.
- **Win** -- complete the pattern and it's a FINISHER: the enemy is destroyed in
  an explosion + shockwave, big camera punch.
- **Lose** -- a wrong key or a timeout throws the player back (knockback) and
  deals damage.
- Cooldown between clashes so it punctuates fights rather than spamming.
ctest 19/19; runs clean.

## Surface material system (friction + scrape sparks)
A material foundation (`exengine/material.c`, test_material -> 20 tests) that two
systems now hang off:
- **Six materials** -- metal, rock, ice, sand, organic, crystal -- each with
  friction, restitution, hardness, moisture, and grit, plus an impact colour.
  Themes map to a ground material (ice fields -> ice, foundry -> metal, wastes ->
  sand, reclaim -> organic, cybertron -> metal, ...).
- **Friction model** -- `ex_material_grip` combines the surface with the actor's
  MASS (from the feel config), STANCE (foot area: vehicles planted, humans small),
  SPEED, and LEAN, plus the surface's MOISTURE (wet = slick) and GRIT (loose grip
  falls off at speed). Ice widens turns and drifts; sand lets go when you gun it;
  a wide planted vehicle grips hard. Feeds the turn radius, drift, and accel.
- **Scrape/spark collision** -- sliding fast across the ground now throws sparks
  on HARD materials (metal/crystal, in the material's colour, with a grind/ring),
  DUST on gritty ones (sand/rock), and next to nothing on slick ice.
ctest 20/20; runs clean.

## Ground craters + scorch (battlefield scarring)
The achievable slice of "mesh fracture" -- persistent decals rather than true
runtime mesh deformation, but the battlefield now scars:
- **Impact craters** -- big explosions (>=0.9 scale) scorch the ground with a
  flat crater/scorch decal, material-tinted (dark burn on hard metal, paler dust
  crater on gritty sand). 24-slot persistent pool, oldest recycled.
- **Hard-landing craters** -- slamming down after a big drop (descent speed >22)
  cracks the ground, kicks up material-coloured debris, and jolts the camera.
- Craters persist for the whole match and reset between matches -- so a long
  fight leaves a visibly cratered arena.
Honest: these are flat decals laid on the ground, not deformed geometry -- true
mesh craters (sunken, cracked vertices) remain a larger graphics project.
ctest 20/20; runs clean.

## Enemy physics feel + stalker lunge
- **Villains now have weight** -- enemies gained real velocity: they accelerate
  toward the AI's intent with MOMENTUM (heavier kinds are sluggish to turn and
  stop -- Apex mass 3.0, Corrupted 1.6, grunt 1.0, hijack 0.6), slide on slick
  ground via the surface-material grip (they drift on ice too), and -- best of
  all -- get KNOCKED BACK by shockwaves as a proper velocity impulse scaled by
  1/mass, so a blast flings a hijack across the arena while the Apex barely rocks.
  The same feel/material systems that drive the player now give the campaign
  villains physical style.
- **Stalker lunge (horror payoff)** -- in the dread level, a stalker that closes
  to within 16u while UNOBSERVED stops holding still and LUNGES: it rushes the
  player and strikes from the dark (damage + dread spikes to max). The freeze
  now also kills its momentum, so watched stalkers are truly frozen.
ctest 20/20; runs clean.

## Enemy vision + form-tied pathing (mode scoring)
Enemies went from omniscient trackers to sighted hunters:
- **Vision cone + line of sight** -- each enemy faces its heading and only sees
  the player within a forward cone AND with a clear sightline (tall cover blocks
  it, using a 2D segment-vs-volume test). Humans are spotted only at close range.
- **Surprise** -- when the player pops into view (around a corner, out of cover),
  an unaware enemy is STARTLED: it flinches (speed cut), hesitates half a second,
  and flashes -- then engages. Break line of sight for a few seconds and it loses
  you and goes back to searching.
- **Form-tied modes with scoring** -- each frame an aware enemy scores walk / run /
  fly for the situation: flyers (hijackers) take to the air to close big gaps,
  grounded kinds run/drive to approach fast, and everyone drops to a walk/brawl up
  close. Changing mode fires a transform trigger. Fly gains altitude + speed; run
  is quicker; walk is the melee stance. Built on the enemy momentum layer, so mode
  changes carry weight.
ctest 20/20; runs clean.

## Signature ability + movement tuning harness
- **RESONANCE pulse** (new signature ability, ported from the source's Apex
  resonance): tap G to emit a shockwave that flings nearby enemies + debris
  outward and damages them (10s cooldown). Reuses the shockwave/ring systems.
- **Movement sim harness** (`tests/test_movesim.c`, test_movesim -> 21 tests) --
  a headless re-implementation of the movement CORE (target-velocity + turn
  radius + coast/aero drag + material grip + gravity/jump) on the shared feel/
  material/vehicle modules, so we can TUNE against hard metrics without a window.
  Scenarios + example metrics:
  - DRIFT slide: car/metal 47u, car/ice 48u, tank 27u (locked), hover 38u (drifty)
  - FLIGHT: climbs to ~105u, aero-capped terminal ~34 u/s
  - JUMP: apex ~3.3u, run-jump range ~9u
  - SPEED to 200u: cycle 7.5s vs tank 15s
  - NAV (slalom obstacles): arrives in 10.4s, near-optimal 184u path
  - CROSSFIRE (damage avoidance): slow runner 89 hits vs fast 14
  - GROUP: speed-varied packs string out from a 3u cluster to 23-31u
  Each scenario checks a tuning invariant (ice >= metal drift, hover > tank,
  cycle > tank, flight capped, nav arrives, faster = fewer hits). The harness
  already surfaced findings: the material effect on drift is weak, and greedy nav
  needs staggered obstacles -- both real tuning signals.
ctest 21/21; runs clean.

## Skirmish hazards, gravity variants, bigger maps, culling
- **Per-map gravity** (skirmish) -- maps now vary gravity for distinct feel: a
  dense heavy world (1.8x, grounded + fast falls), a low-g world (0.45x, long
  floaty jump arcs), and lighter variants (0.7x). Wired through the movement
  gravity so jumps/falls/flight all respond.
- **Toxic sludge maps** -- pools of green DOT muck: touch one and you're poisoned
  for a FULL MINUTE -- health bleeds and your movement is severely handicapped
  (0.40x) the whole time, even after you leave the sludge. HUD shows the countdown.
- **Thruster-chamber maps** -- huge gale zones that blast the player along a wind
  vector and kick up drifting ash/dust particles, shoving your trajectory.
- **Performance: culling** -- distance + behind-camera culling parks off-screen
  clutter and craters below the world so the renderer skips them. Essential now
  the arenas are far larger; the big static populations no longer all draw.
- **Bigger arenas** -- scaled the hull 900 -> 1440 half (2880u across). Verified
  spawns in-bounds + stepping harness smooth.
  Honest: a literal 5x (4500u) would leave this engine's content density far too
  sparse and is impractical without much more to fill it -- 1.6x is a large,
  playable step, and the scale constants are one place to push further. "Different
  orientations" is realised here as gravity variants (which change the whole feel
  of movement); a truly tilted world needs a gravity-direction vector, a larger
  change flagged for later.
ctest 21/21; harness smooth; runs clean.

## Harness coupling + material-drift tuning (acting on the data)
- **Coupled the harness to the REAL movement** -- extracted the horizontal move
  core into `exengine/move.h` (pure math, no rendering), and BOTH the game and
  `test_movesim` now call `ex_move_horizontal`. Zero runtime cost (it's the same
  computation factored out) and the harness now measures the shipping model, not
  a copy. Rendering paths were never the issue -- the physics was already
  separable, so nothing there had to be abstracted or slowed.
- **Acted on the harness finding** -- it showed the material barely affected drift.
  Added a real grip-based DRIFT term (low grip retains pre-step velocity -> slides
  through turns) and widened grip's influence on turn rate. The sweep now shows the
  correct ordering for every vehicle (ice > sand ~ organic > rock > metal ~ crystal),
  with tanks most affected. Effect is stronger but still moderate (turns are
  speed-throttled) -- the harness makes that tradeoff visible and tunable.
- **Sweep across materials x vehicles + speed table** -- the harness now prints a
  drift matrix (6 materials x 5 vehicles) and a 0-200u speed table, so tuning is
  data-driven. Example: cycle 7.5s vs tank 15s to 200u; tank/ice drift 5.36 vs
  tank/metal 4.86.
ctest 21/21; runs clean.

## Enemy transform visual + horror flicker
- **Enemy transform pop** -- when an enemy switches gait/mode (walk<->run<->fly),
  its body now does a quick squash-and-flare (scale bounce over 0.3s) so the form
  change reads at a glance, paying off the mode-scoring triggers.
- **Horror flicker** -- in the dread level the failing lights now stutter and
  cut to near-dark on a noise pattern that gets worse as the dread rises, on top
  of the heavy fog and heartbeat.
ctest 21/21; runs clean.

## More transforms, abilities, and turret ammo
- **5 new transform types** (handling + names + flight): SENTRY (rooted siege gun,
  0.20 speed/locked), WAR WHEEL (momentum juggernaut, 1.30 speed/0.55 grip -- slides
  and barrels), WRAITH (stealth aircraft, fast + nimble flyer), MANTA (biomimetic
  ray-flyer, gliding + agile), BULWARK (armoured gunship, slow heavy flyer). Each
  plugs into the shared handling/feel/material model and the movement harness.
- **Pulse blast (T)** -- a focused forward-cone shockwave that bursts ~14u ahead
  and knocks enemies back. **Blink (Y)** -- a short-range teleport along the aim
  (stealth reposition), with in/out flashes. Both reuse the shockwave/vfx systems.
- **Turret ammo variants (cycle with U while deployed):** STANDARD auto rounds,
  HEAL beam (mends the player + survivors in range instead of firing), ROCKET
  (explosive rounds with a shockwave), and MINE (emits a shockwave ring on a slow
  cadence). The turret tints by ammo (blue/green/orange/red).
ctest 21/21; runs clean.

### Deliberately deferred to a focused next pass (too big to rush)
- Medic **heal/repair gun** that also weakens enemies (a two-mode weapon).
- **Shield dome** deployable item.
- **Playable clone character types** + the ice-clone **warp jump** with a late-
  arrival impact and an interactive top-down landing view. (The ice gravity-leap
  warp + delayed wave already exist as a seed; the playable roster + top-down
  landing UI is a substantial feature.)

## Warp landing, harness runner, debug overlay, menu polish
- **Interactive top-down warp landing** (ice clones) -- tap K in an ice form to
  enter a TOP-DOWN drop-zone view: the camera cranes high overhead looking down, a
  pulsing reticle ring marks the target, WASD flies it across the map, and click/K
  slams the clone down with a late-arrival IMPACT -- big ice shockwave, double
  rings, a crater, an ice burst + debris, and a camera kick. Movement locks while
  you aim, with an on-screen prompt.
- **Harnesses in the repo** -- added `run_harnesses.sh` (build + ctest + run the
  movement tuning harness + the stepping harness with a summary) and `tests/README.md`
  documenting every harness. One command to exercise + measure the whole thing.
- **Debug/profiler overlay** -- new `debug/prof` toggle in Settings gates an
  in-match overlay: fps + frame ms, live counts (enemies/debris/humans), player
  speed + position, collider count, and current gravity.
- **Menu polish** -- the backdrop planets now each have a distinct world colour
  (earth blue-green, jupiter tan, cybertron steel, mars rust, pluto ice), a tilted
  spin axis, and a faint parallax drift; the hero ship banks harder with a slow
  in/out cruise and pitch bob, and its engine glows pulse hotter and swing with
  the bank.
ctest 21/21; harnesses green; runs clean.

## Bot customization: paint, finish, two-tone
Expanded the customizer's visual identity options:
- **Paint schemes (8)** -- steel, crimson, cobalt, emerald, amber, violet, gunmetal,
  pearl -- a chosen primary that overrides the element-default body colour.
- **Finish (3)** -- matte (darker, flatter), metallic (default), glossy (brighter),
  modulating the whole paint.
- **Two-tone** -- the lower body (thighs/shins/hands) automatically reads a shade
  darker, so a scheme looks intentionally designed rather than a flat coat.
- New `paint` and `finish` rows in the Customize screen; both persist in the
  loadout and apply live in the builder preview and in-match.
Honest: these are the texture/paint/design layer on the existing box-primitive
part meshes -- genuinely new authored part GEOMETRY (a sculpted head/chest) is a
larger asset task still ahead; this pass deepens the colour/finish identity.
ctest 21/21; runs clean.

## Playtest fixes: turning, camera bank, HUD grid
- **FIXED: vehicles couldn't turn (dino titan on mission 1).** Root cause found via
  the movement harness: steering used a chased target, so a low-accel/heavy vehicle
  turned at its ACCEL rate (~0.06 rad/s), not its turn rate (~1.5). Now the turn
  radius rotates the velocity vector DIRECTLY at the turn rate; acceleration only
  governs speed. dino/sand now reverses 180 in ~2s (was never). Added a
  `run_turn180` regression guard to test_movesim. Bonus: material drift is now
  clearly differentiated (car/ice 0.73 vs car/metal 0.54).
- **Camera bank tuned** -- dialed the horizon bank down 15% globally and another
  50% when NOT airborne (the loose bank suits jets, not grounded bots), and scaled
  it by the map's gravity so low-g worlds feel floatier and heavy worlds planted
  (gravity facing for the space/gravity-varying maps).
- **FIXED: debug FPS drawing on the map floor.** The VGA HUD grid is 32x16; the
  debug overlay was printing at rows 16-19 (off-grid), spilling onto the world.
  Repositioned within bounds; also fixed the F1 feel-tuner column overflow.
ctest 21/21; runs clean.

## Playtest fixes round 2
- **FIXED: inverted body rotation** -- the body yaw used a negated aim, so mouse
  left/right spun the model the wrong way. Flipped the sign (robot + vehicle poses).
- **FIXED: white-ball projectiles** -- the emissive path multiplied tint x1.25 and
  ACES tonemapping then desaturated the over-bright result to white. Dropped to
  x0.85 so bolts (and all emissive fx) keep their colour.
- **Walking gait** -- cadence was 2.6 (~5 steps/sec: the fast body jiggle/vibrate).
  Cut to 0.95 for a slow, heavy stride and widened the leg swing (1.05) so the legs
  feel bigger.
- **Flight speed** -- jet top speed 26 -> 34 (the aero drag had made it feel slow).
- **Bulky parts** -- found the offenders programmatically by bounding-box VOLUME:
  the chest dominated (0.104, 3x the next). Slimmed chest (0.104->0.064), abdomen,
  and pauldrons so the torso detail reads instead of being buried in bulk.
- **Crater debris (cinematic pass)** -- debris now falls under the MAP's gravity
  (low-g worlds let it hang and float), fresh craters throw a dust plume + scattered
  chunks, and a new `kick_debris` means the player, bots, AND bullet impacts all
  scatter debris they move through -- so a fight churns up the battlefield.

### Deferred: distance haze + LODs
The atmospheric "distortion haze to signify scale" (distinct from fog/blur) plus
mesh LODs is a rendering pass of its own -- flagged rather than rushed blind.
ctest 21/21; runs clean.

## Distance haze (atmospheric perspective for scale)
Added the deferred scale haze -- distinct from fog and NOT a blur. In the forward
shader, distant geometry (ramping in from ~250u to ~1350u) gradually desaturates
toward its luminance and lifts toward a faint cool tint, so a large arena reads as
VAST distance rather than a crisp miniature. Nearby detail is untouched; the effect
tops out gently (max ~45%). Pairs with the existing distance culling.
Note on LODs: the part/prop meshes are already minimal primitives (boxes, bevels,
spheres), so classic poly-reducing LODs add little here -- the haze + culling carry
the "feels larger" goal instead. True LODs would matter once bespoke high-poly
meshes exist.
ctest 21/21; shaders compile; runs clean.

## Footstep dust (weight for the slowed gait)
A grounded bot now kicks a small material-coloured dust puff at the planting foot
on each step, synced to the (now slow, heavy) gait -- selling the mass of a giant
strider. On-foot + moving + grounded only; uses the ground material's colour.
ctest 21/21; runs clean.

## Playtest fixes round 3
- **Body now faces MOVEMENT, head looks at CAMERA.** The body used to face the
  camera aim; now it smoothly turns to face the velocity/arrow-key direction and
  walks that way, while the head (and visor) turn toward the camera (clamped +-60)
  so you look where the camera points. Aiming/firing still follow the camera.
- **Camera bank** cut another ~50% (mul 0.85 -> 0.42) AND hard-clamped to +-0.18 rad
  so a hard corner can't snowball -- fixes the too-strong, too-exponential tilt.
  Still lively for jets, minimal on grounded bots, gravity-scaled.
- **Jet directional speed** -- top speed now scales with heading: forward ~1.20x,
  strafe ~0.62x, reverse down to ~0.20x, so a jet thrusts ahead and can't slide
  sideways or backpedal as fast (proper flight dynamics).
- **Crater mesh** -- replaced the flat disc with a generated 540-vert noisy bowl
  (raised rim, depressed centre, surface noise), lit for relief and scaled with
  crater size, so impacts read as real gouged ground instead of a flat decal.
ctest 21/21; crater.iqm loads; runs clean.

## Visual-bug hardening + adaptive jump
- **HUD-smear-on-world fix (defensive).** The screenshot showed the HUD baked/tiled
  across the floor -- the classic signature of the 2D overlay being drawn into the
  offscreen scene FBO (so it ends up in the scene texture) and/or the screen buffer
  never being cleared. Hardened three things: `ex_vga_render` now explicitly binds
  the DEFAULT framebuffer (0) before drawing and restores depth-test after, and the
  screen buffer is cleared before the scene is composited. This can't be reproduced
  in the headless sandbox (no display), so it targets the most likely GPU-state
  causes; needs an on-device retest to confirm.
- **Adaptive jump.** Replaced the fixed hop with a jump that reads transform MODE
  and bot SHAPE: legged alt-modes (raptor/wolf/dino/spider/insect/scorpion/crab/
  dragon) bound, wheeled/tracked barely hop; an agile light-class slim frame leaps
  high while a heavy bulky titan clears little; low-gravity maps add air. Camera
  kick scales with the leap height.
ctest 21/21; runs clean.

## Rotation offset, haze/white fix, specialist alt-mode weapons
- **90-deg facing offset** -- the mesh's built-in forward is +Z but the yaw is
  measured from +X, leaving models turned 90 deg. Added a -90 deg body-render
  offset (EX_BODY_FACING_OFF) so the whole assembly + head-look shift together.
  (Direction is a best guess; flip the sign if it overshoots.)
- **Distance haze no longer whitens** -- it blended distant colour toward
  luminance, which blew bright geometry toward white (the "white menu" regression).
  Now it desaturates toward a DARK cool tone instead -- can't push to white.
- **Specialist alt-mode weapons** (fire while transformed):
  - SPIDER -> WEB SHOT: sticky bolt that damages and webs (slows to a crawl +
    sheds white strands) the struck enemy for 3s.
  - DINO -> FIRE BREATH: a short forward cone of flame torching everything in
    front, with a flame plume from the maw.
  - SCORPION -> DUAL CANNONS + STINGER: two angled cannon bolts plus a heavy
    melee stinger finisher on a very close enemy.

### Deferred (bigger, flagged honestly)
- Climbing surfaces with spacebar.
- Body-to-camera detent for some builds vs. free omnidirectional waist rotation
  for others (legs twisting fully at the torso) -- a per-build facing model.
- Vehicle "remnants" in robot mode (a car-former keeps car parts, a dino-former
  dino parts) -- currently the robot silhouette is shared; making it reflect the
  chosen alt-mode is a real per-vehicle modelling pass.
ctest 21/21; runs clean.

## Model detail patterns, cape, wheels, debris persistence, transform flicker
- **Greeble now PATTERNED, not scattered** -- detail snaps to a grid so it forms
  rows/columns, biases to the top + sides (never the underside), mirrors across x
  for symmetry, and favours flat panels; tall spikes/antennas are rarer + shorter
  so nothing juts out distractingly.
- **Racer wheels roll, don't bob** -- the car wheels were oscillating up/down on
  the gait phase, which read as walking. Removed the bob so they sit and roll.
- **Cape** -- a flowing back panel (original geometry) that trails and sways with
  the player's velocity, flutters at idle, lifts with speed, and reads in the
  accent colour. Robot mode only (hidden in vehicle mode + menus).
- **Persistent debris** -- battlefield litter now lasts ~an hour instead of
  fading in seconds; the 48-slot ring buffer evicts the oldest so it stays bounded,
  and kick_debris (bullets/bots/trampling/shockwaves) still re-scatters it.
- **Transform flicker fixed** -- parts leaving/entering the alt-mode had their
  scale hover around the cull threshold, flickering appear/disappear mid-fold. They
  now collapse to exactly zero once nearly faded, so hiding is deterministic.

### Still open (flagged)
- The "giant box" on the racer is the chassis + accumulated greeble; the patterning
  above should help, but a dedicated per-vehicle silhouette pass is the real fix.
- Capes are sourced as a general design element (original geometry), not from any
  copyrighted script text.
- Map bounding boxes / room designs -- ongoing.
ctest 21/21; runs clean.

## Ideas ported from reference codebases (genome + ruleset)
Drew two concepts from the reference projects (implemented as original code):
- **Seedable proportion GENOME** (`exengine/genome.h`, `test_genome`) -- pattern
  from a parametric character-forge: 12 heritable genes (bulk, height, head,
  shoulder, arm, leg, neck, tail, snout, jaw, crest, greeble) each with a
  min/max/default, so a whole distinct bot comes from one 64-bit seed, can be
  nudged by mutation, or bred (blended) from two parents. Enemies now each draw a
  stable seeded genome, so a crowd of the same kind no longer shares one
  silhouette. Foundation for distinct bots + inherited alt-mode traits. 22/22.
- **RULESET harness convention** (`tests/RULESET.md`) -- every bug class we hit
  this session (off-grid HUD, HUD-into-FBO smear, accel-bottlenecked turning,
  transform flicker, genome range safety, shared-core coupling, bounded pools) is
  now a numbered rule with a check, so regressions can't reappear silently.
The other uploads (logscope = an egui/wgpu LLM tool; unendlich = a Rust/Vulkan
voxel client) are further afield; their portable value is engineering practice
(epoch guards for async results, pure-vs-render module separation), noted for
later rather than force-fitted.
ctest 22/22; runs clean.

## Spark-resonance stealth loop + resonance graphics
Drew on the Blackstar docs as design inspiration (the Corrupted hunt by "spark
resonance," drawn to active sparks) -- implemented as an original gameplay system:
- **Spark loudness** (`g_spark_loud`) rises with your activity -- fast movement,
  gunfire (+0.35/shot), and abilities (pulse = full) -- and decays toward quiet
  when you hold still. It's a real stealth dial: go loud and you're a beacon.
- **Resonance hunting** -- Corrupted + Apex enemies now sense a loud spark through
  walls at range (40u quiet .. 130u max loud), not just by line of sight. Going
  quiet lets you slip past a nearby hunter; opening fire calls the whole pack.
- **Spark-signature HUD meter** ("SPRK ||||...."), green->amber->red, so you can
  read how detectable you are and choose when to fight vs. sneak.
- **Resonance feedback (graphics)** -- when you're loud AND a hunter is locked on,
  the screen throbs faintly (something reaching back toward your spark), and your
  spark glow flares brighter in the world with loudness, so the signature is
  visible, not just a number.
ctest 22/22; runs clean.

## Alt-mode remnants: robots echo their vehicle
Robot-mode silhouettes now carry visual echoes of the alt-mode they transform
into, so a lineup reads as distinct transformers instead of one shared body
(uses the genome/alt_mode field the customizer and enemy genomes now set):
- CAR/CYCLE -> stowed wheel on the back + fender shoulders.
- JET/GUNSHIP/HAWK/WRAITH/MANTA/BULWARK -> folded wing shard across the back +
  broader shoulders.
- DINO/RAPTOR/DRAGON/SCORPION/WOLF -> clawed forearms, a raised spine ridge,
  a snouted head.
- TANK/MECH/SENTRY -> heavy siege pauldrons + a wider chest/back plate.
Verified the silhouettes measurably diverge (dino back-vol 0.033 vs jet 0.012).
Enemies draw a stable pseudo alt-mode from their genome, so crowds vary too.
ctest 22/22; runs clean.

## Enclosed room designs + bounding boxes
Upgraded the ROOM map node from three loose slabs into a proper enclosed
structure, all auto-collided (every blueprint volume becomes a step-box):
- Four walls forming a real bounded interior, with a DOORWAY gap on one seeded
  side (two short segments leaving a passable opening) so you can enter/exit.
- A walkable ROOF (elevate platform) over the room -- a rooftop fighting position.
- An interior cover column + a high sky-reward platform above.
Verified: mapgen test 126 volumes (< 150 cap), 21 conceal walls + 52 platforms,
stepping harness ALL COURSES SMOOTH (rooms don't trap the player), game runs clean.
ctest 22/22.

## Cover-seeking AI: rooms become tactical
Closed the loop between the new enclosed rooms and enemy behaviour:
- A HURT grunt or hijacker now breaks toward the NEAREST wall/cover volume
  (conceal/low/high) within ~70u and positions on the side AWAY from the player,
  so the room walls, berms, and cover columns are used tactically instead of
  being scenery.
- Heavy Apex/Corrupted ignore cover and press the attack, keeping their menace.
Behaviour-only (no new pose state), so it's low-risk; 12s soak stable, harnesses
all green (ctest 22/22, assertions pass, stepping courses smooth).

## Genre research applied: rim light + design notes
Researched a decade of reviews/player feedback on comparable transforming-robot
shooters (all paraphrased; original game). Findings in DESIGN_NOTES.md. Acted on:
- **Silhouette rim light** (forward.glsl) -- a cool grazing-angle edge glow,
  tinted per element, so robots + geometry read off same-coloured backdrops. Fixes
  the recurring "bots blend into the metal setting" complaint. Shaders compile clean.
- Confirmed our **barrel-HEAT** model already sidesteps the #1 combat complaint
  (ammo scarcity + slow reloads) -- documented to protect it.
- Confirmed our **10 themes** with per-theme lighting/fog/material already answer
  the "repetitive brown metal" critique better than the reference shipped.
- Our just-added **cover-seeking AI** closes the "enemies don't use cover" gap
  reviewers flagged.
ctest 22/22; runs clean.

## Tonal correction: preserve the dark, bend it cosmic
Corrected an earlier misread: the grim desaturated palette is a WORKING stylistic
choice (the War/Fall era held its audience with it; later brighter entries thinned
the base), not a flaw. So instead of adding vibrance:
- **Rim light retuned** to a faint COLD void-sheen (dim, deep steel-blue) that
  separates silhouettes from the murk without a neon outline fighting the tone.
- **Cosmic-scale atmosphere** -- outer_rim + shield_edge get colder, denser
  void-fog so distance reads as vast + pressing, a distinct dark register bending
  toward the film-script cosmic mood (never brighter).
- DESIGN_NOTES.md updated: dark tone = strength to protect; variation comes from
  material/silhouette/light-temperature/fog/accent, not brightness.
ctest 22/22; runs clean.

## Character mesh detail: sculpted head + paneled chest
Priority shifted to the character DESIGNS themselves (geometry/texture), the real
weak point vs. the parametric-variation work. The part meshes are procedurally
authored (my own generators in tools/gen_props.py); enriched the two parts that
define a robot's read:
- **Head** (phead.iqm, now ~2100 verts) -- added angled cheek panels, a recessed
  visor band (reads as eyes), cheek vents, flanking crown ridges, and a defined
  forward chin, on top of the existing cranium/faceplate/brow/crest/antennae.
- **Chest** (new pchest.iqm, ~1800 verts) -- an armored cockpit: raised angled
  pectoral panels, a central windshield-like plate, a collar ridge, side intake
  vents, and a lower abdominal band -- so the torso reads as a designed chest
  instead of a plain bevel box. Wired to the CHEST part slot.
These lift EVERY bot at once (all share the part templates) and the genome/paint/
alt-mode systems layer on top. Next: sculpted limbs/shoulders + per-vehicle head
variants. ctest 22/22; meshes load; runs clean.

## Body archetypes: proportion silhouettes (from concept direction)
Reshaped the base RIG (not just mesh detail) per the original concept designs:
separate CALF/THIGH/TORSO/stance scalars driven by the alt-mode so silhouettes
actually differ:
- RACER/CYCLE + DINO/RAPTOR/DRAGON/SCORPION/WOLF -> LONG CALF, short stocky torso,
  the wheelbase/haunches folding into the lower legs (coiled, bottom-heavy).
- TANK/MECH/SENTRY/BULWARK -> WIDE-BODY, SHORT-LEG, short torso (squat siege).
- JET/FLYERS -> tall, long-leg, slim torso (lean riser).
- default -> calf-forward balanced humanoid.
Offline layout preview (body_archetypes.png) renders the assembled silhouettes
from the real part placement so proportions are verifiable. ctest 22/22; runs clean.

## Part Editor (tools/part_editor.html) — visual modeling pipeline
Addresses the real design bottleneck: no way to get authored geometry into the
engine. A standalone three.js editor (single HTML file, opens in any browser):
- Drag/scale/rotate PRIMITIVES (box, bevel, cylinder, taper, sphere, wedge) into
  a part, with live 3D preview, click-select, mirror-X, duplicate.
- Exports a game-ready .iqm whose byte layout is IDENTICAL to tools/gen_props.py
  write_iqm and what the engine's IQM loader reads (verified: 124B header, 3
  vertex arrays pos/uv/nrm). Drop the file in engine_src/data/ and point a part
  template at it.
- Also saves/loads .json projects and can copy the design out as gen_props()
  Python so it can live in the procedural pipeline too.
Note: this is primitive ASSEMBLY (a big step up from hand-typing coordinates),
not organic sculpting -- for that you'd model in a DCC tool and convert to IQM.
The reMarkable route was assessed and set aside: it stores 2D pen strokes, not
3D geometry, so it yields reference sketches at best, not meshes.

## Spine Grower (tools/spine_grower.html) — generative sketch instrument
A different creative primitive from the box editor: a spine that never stops
GROWING, steered live by Vim/helix-style keys so form EMERGES from flow.
- Steer: h/l yaw, j/k pitch, H/L roll, [ ] helix twist -/+, 0 straighten.
- Shape the stroke as it grows: w/b thicken/narrow, e ridge, p pinch waist,
  f flare, 1-5 swap cross-section (circle/square/tri/star/hex), ~ wobble flow,
  space drop a bulge node.
- "Seed a curl" starts you mid-motion with an interesting helix so there's
  something to react to instead of a blank line.
- Skins the path into a tube mesh live and exports the SAME game-ready IQM
  (byte layout verified against the engine loader). A good run = a real part.
Inspiration: helical title-ribbon motion + a snake generator; original code.
Pairs with part_editor.html (box assembly) -- two ways to author, one export path.

## Spine Grower v2: branching + symmetry
Extended the generative spine tool so one flowing session can grow a whole
symmetric, multi-limb creature part:
- **t = BRANCH** forks a child spine off the current tip (thinner, angled) and
  switches steering to it; **y = return** to the parent. Stack any depth of
  branches -> limbs, digits, a crown of tendrils from one gesture.
- **m = MIRROR** grows a reflected twin across X in real time, so symmetric
  parts (paired horns, wings, arms) emerge automatically.
- Export bakes EVERY spine + its mirror twin into one combined IQM (byte layout
  still verified against the engine loader), recentred + normalised to a slot.
- HUD shows spine count, branch depth, mirror state. Structural integrity of the
  tool JS verified (balanced scopes, no dead refs).

## Spine Grower: touchscreen mode
Added a touch/tablet mode (auto-enables on coarse-pointer devices; toggle top-right):
- The keyboard sidebar is hidden and replaced by controls DOCKED at the bottom so
  the viewport stays clear -- rethought for portrait where a sidebar obscured it.
- A virtual JOYSTICK (bottom-left) steers yaw+pitch continuously -- better than
  discrete keys for a flow tool -- and captures its own pointer so it never fights
  the drag-to-orbit camera.
- Tap buttons fire every shape op (thick/thin, twist-/+, ridge, pinch, flare,
  wobble, node, section ○□△✦⬡) plus Seed/Reset/Mirror/Branch/Parent/Export,
  with toggles (ridge/wobble/mirror/section) showing active state.
- Safe-area insets respected for notched tablets. Keyboard mode still available
  via the same toggle. Tool JS integrity re-verified.

## Mobile export fix (Spine Grower / Part Editor)
Fixed the "made something cool but couldn't export it on the phone" problem.
Programmatic file downloads fail silently on many mobile browsers, so export now
tries three paths in order:
1. **Native share sheet** (navigator.share with the file) -> Save to Files, AirDrop,
   or message it to yourself. Best on phones/tablets.
2. **File download** (desktop, and mobile where it works).
3. **Always-works fallback:** a copyable BASE64 text box. Copy it, get it to your
   desktop, and run tools/paste_to_iqm.py to turn it back into the real .iqm
   (decoder included). A design can never get stranded on the device again.
Tool JS verified with node --check (valid syntax).
