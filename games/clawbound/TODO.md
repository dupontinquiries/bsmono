# CLAWBOUND-RS — build plan (native Rust + wgpu, paperbound style)

Rendered/verified headless via lavapipe (software Vulkan) → PNG.

## This pass (graphics detail)
- [x] Ground reads as GREEN (was washing out to paper/white) — regrade biome palette + cut dry wash
- [x] Grass tufts — dense, paper-blade style, on grassy biomes
- [x] Rocks — faceted paper boulders scattered
- [x] Bushes — denser + more lumps (stay opaque/hideable)
- [x] Tree leaves — individual leaf cards on near/hero trees (higher LOD)

## Foundation (done)
- [x] wgpu headless pipeline (Vulkan/lavapipe → PNG), paper/ink/grain/vignette post
- [x] domain-warp terrain + Whittaker biomes + genetic trees (ported)
- [x] procedural branches per tree type + branch mass/snap physics + perch/climb basis
- [x] leaf vs bush transparency (screen-door dither)


## Gameplay (6v1)
- [x] 6-cats-vs-1-hunter sim: cat AI (seek/flee/hide-in-cover), hunter sense+FSM pursuit, catch/flush, per-archetype balance (Fox1 Dog3 Badger4 Hawk4 Snake0 caught/20s). Unit-tested + `game` render mode.
- [x] Wire sim into --play: WASD-driven cat 0, follow-cam, static world + per-frame entity buffer, hunter/cats live (compiles --features app; run on GPU)
- [x] Scent trails + wind: cats drop scent that drifts downwind, hunter smells/tracks upwind (smell stat matters), flushes hidden cats it noses out
- [x] Climbing to evade: cats scramble up trees (safe from ground hunters, exposed to the hawk); cats pick tree vs bush by hunter type; player climbs with crouch at a trunk
- [x] Snake water-ambush: lurks at nearest water to the objective, near-blind but feels footfalls, erupts with a 1.8x strike-burst when prey nears the pool (distinct positional playstyle)
- [x] --play feel: floating stamina bar + 'you' marker over the player cat, win/lose screen tint (green=cats, red=hunter), auto round-restart after 3s or on R
- [x] Dodge/roll: Space does a burst with i-frames (juke a lunge) on a 1.5s cooldown, costs stamina — player skill move (AI relies on cover)
- [x] Survive mode (outlast the hunter, no escape point; hunter roams the pack's ground) + smell-scaled flush so cover isn't absolute (badger roots out hiders, hawk sees them from the air)
- [x] In --play: number keys 1-5 pick the hunter archetype, M toggles Rendezvous/Survive (restarts the match)
- [ ] Textual HUD (timer/cats-left); netcode transport for a human hunter


- [x] SHADOW enemy #6 (afterlife dark-warrior; Silent Hill + Freddy + Tigerstar): teleport-blink to its quarry, otherworld SHIFT that reveals hiders/reaches climbers/empowers it; dark spectral model + otherworld screen tint; playable via key 6 (nightmare tier: catches 5-6/6)


- [x] AI vision cone hidden by default (debug-only, toggle key V / Settings); terrain given fuzzy high-freq colour noise so translucent overlays melt into the ground
- [x] egui menu (egui 0.28 + egui-winit 0.28 + hand-rolled wgpu renderer, unendlich-style): Title (Single Player/Multiplayer/Settings/Keybinds/Quit), hunter+mode pick, settings (cone/volume/difficulty), rebindable keys; menu.rs logic unit-tested; Esc opens, game pauses


- [x] Nightmare/otherworld atmospherics (Shadow shift): near-black violet sky, stars snuffed out (robust hash), long shadows crawling up from the trees, spectral cats drifting in the underworld, sickly desaturation + closing vignette. `tod=nightmare` render + driven live by the shift in --play (post fx uniform)

## Roadmap (multi-turn)
-7. [x] Ported the three three.js-only mechanics into the native sim (sim.rs), unit-
   tested, wired into --play (app.rs) and the renderer (main.rs):
   (1) **Leap** — a committed forward bound on `F`/`E`: longer and airborne, so it's
   uncatchable mid-flight and clears more ground than a run (distinct from the snappy
   Space roll). Arc + leap pose in the render. Test: covers more ground than a run and
   survives a point-blank frame.
   (2) **Hawk grab & carry** — the hawk no longer kills outright; it hoists open prey
   aloft for CARRY_DUR while hovering (hunting no one else — a window for the pack). The
   carried cat struggles: ~12% it wrenches free and limps off crippled, else it's taken.
   Deterministic coin-flip via a local hash. Rendered dangling from the talons. Tests:
   grab→carry→resolve, and the climbing-cat test updated (the hawk now plucks-by-grab).
   (3) **Prey pounce** — a cat mid-burst (leap or roll) snatches an ambient critter it
   lands on, topping up stamina and tallying `prey_caught` — ties the ecology into play.
   Skill catch (fleeing prey are hard to corner). Test: a leap onto a critter removes it
   and feeds the cat. HUD shows leaping/carried/limping states; controls hint adds "F leap".
-6. [x] Bug hunt (round 3): no new crash found — this round was audit + hardening.
   Added a per-tick **invariant property test** (all six hunters × both modes): stamina
   stays in [0,1], no cat is ever both safe and caught, actors stay on the map, positions
   stay finite, escaped/caught counts match reality and never regress, and a finished
   round stops simulating. Read the WGSL scene/post shaders (depth-in-alpha, bayer
   dither, star/nightmare paths) — correct; only a cosmetic nit (stars can faintly bleed
   onto far-horizon geometry) left untouched rather than risk the verified star field
   without a visual check. Audited net `Server` (delta encode/apply round-trips,
   `next_id` can't overflow behind the MAX_PLAYERS cap), `world::height_at` (bilinear
   indices bounds-safe, clean out-of-grid fallback), and `app::resize` (rebuilds the
   post bind group after recreating targets — no stale binding). All clean.
-5. [x] Bug hunt (round 2): **net.rs crash fix** — the wire cursor read bytes with
   unchecked indexing (`self.d[self.p]`, `self.d[p..p+4]`), so a truncated or malicious
   packet panicked the receiver even though `unpack_*` claimed to return `Option`. Made
   the cursor bounds-checked and fallible (reads yield `None`); `unpack_snapshot` /
   `unpack_delta` now propagate with `?`. Fuzz test feeds every prefix of a valid packet
   plus a header lying about its entity count — all reject cleanly, no panic. Also swept
   all 48 render configs (every mode × TOD, all six hunters × both game modes) and a
   12-game sim soak (full rounds, live input, ecology at scale) — panic-free and
   terminating. Audited math/menu/physics/mapeval/cat/CLI-args: divisions guarded,
   menu indices clamped, no out-of-range indexing.
-4. [x] Bug hunt: (a) **crash fix** — a fleeing AI cat with no tree (Hawk forbids
   climbing) and no bush in range hit `tree.unwrap()` on None and panicked the whole
   sim; now guarded (`tree.is_some() && td<=bd`), regression test with Hawk + cleared
   cover. (b) Ambient critters could roam to SIZE·1.4 (past the terrain edge into the
   void) — clamped to SIZE·0.9. (c) Land critters (hare/deer/squirrel/owl/grouse) could
   wander into deep water and render under the surface — they now turn back at the
   shoreline (ducks/herons/frogs still allowed), only blocking land→water so an
   already-wet critter isn't trapped; regression test on a real baked shoreline.
   (d) Hardened the Shadow phase-shift gate from a `name=="Shadow"` string compare to
   `is_shadow()`. (e) Removed dead code (Whittaker `band3`, two unused imports) — the
   tree now builds warning-clean.
-3. [x] Liquid water (world.rs build_water): the flat slab is now an undulating sheet —
   corners displaced by a layered directional-sine + noise swell (tapered to nothing at
   the bank), so the per-face normals catch light like ripples. Crest sheen, a foamy
   paling in the shallows, and opacity that feathers to zero at the waterline dissolve
   the blocky rim. Purely cosmetic — gameplay still treats WATER_Y as the logical level.
   (Swell is a static instant; live animation would need a time uniform in the scene
   shader — noted for later.)
-2. [x] Biome colour blend (world.rs biome_color): terrain ground colour + gloom are
   now a weighted blend sampled across a small noise-jittered 9-tap kernel, so the
   palette crossfades across a biome seam (~7-unit band) instead of hard-cutting.
   Biome interiors and all placement (trees/rocks/reeds via biome_at) stay crisp.
   Adapted from unendlich's biome_blend convex-combination model. Unit test scans the
   map and asserts the worst adjacent colour jump drops to under half the hard version.
-1. [x] Playable-loop fixes from M2 playtest (app.rs): (a) movement was applied in
   world space under a fixed diagonal camera, so WASD felt rotated ~30° off — input
   is now camera-relative via move_world(yaw,..), unit-tested against the right-handed
   look_at so W = into-screen and A/D aren't swapped; (b) added mouse-look — yaw/pitch
   state driven by raw DeviceEvent::MouseMotion with the cursor locked+hidden during
   play (freed in menus / on game-over), camera now orbits the cat instead of a hard
   offset; (c) in-game HUD (hunter+mode, cats remaining, stamina bar, your state, a
   controls hint); (d) real game-over screen (dim + verdict + survivors + "press R"),
   plus a slow-motion beat, red flash and grim desaturate when the player is caught;
   (e) fixed an entity-buffer overflow that blanked every entity for a frame (now the
   buffer grows) — a source of the on-screen glitchiness.
0. [x] Fix garbled menu text (egui_render.rs): the hand-rolled renderer mis-routed
   egui's PARTIAL font-atlas updates (new glyphs rasterized when opening a submenu)
   through the create-new-texture path, replacing the whole atlas with the tiny
   glyph patch and leaving every prior glyph with stale UVs. Now routes on delta.pos:
   None = full (re)create, Some(pos) = in-place sub-rect write into the existing
   atlas. Headless GPU regression test reads the atlas back after a full+partial
   sequence and asserts the patch lands without discarding the rest.
1. [x] Six named biomes (Stony Mountains, Lake Shore, River Reeds, Shadowy Forest, Oak Tree Forest, Moor Plains) selected by elevation/water-proximity/moisture — per-biome palette, tree kind (oak vs conifer), gloom, reeds at the water fringe, scree in the peaks. Census: all six present (Oak 48%, Lake Shore 19%, Moor 15%, Reeds 8%, Shadowy 6%, Mountains 5%).
2. [x] Ambient ecology (ecology.rs): 8 species (hare/deer/squirrel/duck/frog/heron/owl/grouse) seeded by per-biome habitat weight × day-night activity (from unendlich's ecosystem model), each with a graze/wander/alert/flee micro-FSM (sense→transition→act, adapted from early-fps). Critters bolt from the hunter and cats; rendered as posed papercraft creatures. 6 unit tests.
3. [x] Cat animation rig + gaits + climb-to-safe-reach (pose sheet + motion strip); live input-driven version lands with the playable loop along Branch up to safe_reach
2. [x] Hunter senses (cascading cone + omni, 5 archetypes) + micro-FSM (adapted from early-fps enemy.rs); AI movement wiring lands with playable loop
3. [x] Map fairness via adapted early-fps mapeval (openness/flow/cover/vert + S-F grade + hunter/cat fairness)
4. [x] Water + talus erosion: baked heightfield, ported thermal_erosion erode_step, river carve, basin water level (depth-tinted translucent sheet). TODO deeper: cellular water_cells/flow_tiles + caustics/waves in post
5. [x] Day/night lighting + objectives (Reach/Rendezvous/Survive/Forage, fairness-placed beacons)
6. [~] WASM/WebGPU: core refactored into lib (web-ready); BUILD_WEB.md + web/index.html kit ready. Final wasm compile is local (sandbox toolchain lacks wasm32 std)
7. [x] Netcode core (up to 6): authoritative server, snapshot delta encode/apply, wire (de)serialize, interpolation — 6 unit tests pass. Transport (WebRTC/UDP) wraps it next.

## Style/perf notes
- keep total verts under ~1.5M for lavapipe headless shots
- individual leaves/grass only near camera / hero tree (LOD by distance later)
