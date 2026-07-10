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
-17. [x] Three fixes from the playtest:
   (a) **Game speed** — the loop stepped a hardcoded dt=1/60 every frame, so on the M2's
   120Hz ProMotion display everything ran at 2× speed. Now it uses real elapsed time
   (clamped to 0.1s so a stall can't fling things), so it plays at the same pace on 60Hz
   and 120Hz alike.
   (b) **Mouse binds** — added left/middle/right mouse buttons to the input. Middle-click
   rolls and left-click leaps (sim/prowl); left-click throws a firecracker and right-click
   jumps (Pop Island). Control hints updated.
   (c) **Feature (mode separation)** — Pop Island **sudden-death overtime**: a tie after
   two periods now goes to a 30s golden-goal window (the HUD shows "SUDDEN DEATH"); the
   next capture wins outright, or it's a draw if no one scores. Test covers the golden
   goal; the invariants soak was widened to cover the overtime window. 68 tests.
-16. [x] Bug hunt on the newer modes (capture/prowl) — no bugs found, but added the
   property coverage they lacked: a **yarn-conservation + invariants soak** over full
   Pop Island matches for all six classes (loose + carried yarn always == the six balls,
   scores only climb, mobs finite and inside the arena, timers never negative, always
   resolves), and a **Prowl invariants soak** over 16 class×hunter combos (stamina in
   [0,1], prowler + hunter finite and on-map, tallies monotonic, no panic). Swept all 18
   capture/prowl render configs — panic-free. Removed a dead `.chain(..take(0))` no-op in
   `respawn_yarn`. 67 tests total. (Also re-hit the recurring test-clobber footgun and
   caught/fixed it before it shipped.)
-15. [x] Smarter Pop Island AI (capture.rs) — replaced the fetch-and-throw-randomly bots
   with role-based play: one **defender** per team orbits its base and runs down enemy
   yarn carriers, while the rest fetch yarn. Firecracker targeting now prefers carriers
   (weighted 0.4× distance, wider 20u lob) and fires whenever loaded and in range instead
   of a 3% coin-flip, and every bot detours for a power-up that's on the way. Result:
   firecrackers actually land (≈4 stuns/match vs near-zero), carriers get contested, both
   teams score. 3 unit tests (throws at a nearby carrier, detours to a close power-up,
   defender chases a distant carrier). 65 tests total.
-14. [x] Wired the **Settings difficulty** into gameplay (it was previously unused). A
   hunter-aggression multiplier (easy 0.82 / normal 1.0 / hard 1.22 via
   `menu::difficulty_mult`) scales both the suspicion build-up rate and the hunter's
   pace, in the 6v1 sim (`Game.difficulty`) and Prowl (`Prowl.difficulty`). The app sets
   it from Settings when a match starts and preserves it across restarts. Verified it
   moves outcomes: over a fixed Survive match a Fox catches 2 on easy → 3 on hard, a Dog
   2 → 3 (the Hawk already sweeps 6/6). Test asserts a harder hunter never catches fewer.
-13. [x] Pop Island polish from the playtest concerns: (a) **bases now land on dry
   ground** — instead of fixed anchors at (0,±70) that could sit underwater on this
   hilly/watery map, each base is nudged to the nearest dry, reasonably flat spot near
   its anchor (`land_near`), so yarn is always deliverable. Bases are instance fields
   with a `base(team)` method (replaced the fixed `base_of`); test asserts both bases are
   above the waterline. (b) **Per-mode camera** — `write_camera(dist)` is parameterized
   so Pop Island pulls back to 34 (from 20) for its wide, 140-unit-long arena while
   Prowl stays tight at 20.
-12. [x] **Pop Island is now playable in the window too** (app.rs) — all four menu modes
   are live. Selecting Game: Pop Island launches a capture session dispatched to a
   self-contained `render_capture` reusing the shared helpers (upload_entities /
   write_camera / present / draw_menu). WASD move, Space throws a firecracker, Shift
   jumps, R restarts. HUD: a Tawny–Silver scoreboard with the period timer, your
   creature + captures/stuns, active power-ups (SPD/SHLD/3X) and the firecracker
   cooldown, a "carrying yarn" banner, and a win/draw screen. Renders both team bases,
   loose yarn, power-ups, in-flight firecrackers, and team-collared creatures. The menu
   now fully drives mode selection → the sim (Rendezvous/Survive), Prowl, and Pop Island
   are each playable; picking one clears the others. (App wiring compile-verified only —
   no display in the sandbox.)
-11. [x] **Prowl is now playable in the window** (app.rs). Selecting Game: Prowl in the
   menu launches a live prowl session: the loop dispatches to a self-contained
   `render_prowl` (input → step → follow-cam → entities → HUD) while the 6v1 sim path is
   left untouched. Camera-relative WASD, mouse-look, Shift hide, Space roll, F leap, R
   restart; the HUD shows the rotating objective, prey/caught/escapes, stamina, and an
   EXPOSED warning (with a violet screen tint) when you're outside the ring. Renders the
   prowler, stalker, critters, POI pillars, objective beacon and the storm-ring wall.
   Factored shared helpers (upload_entities / write_camera / present / draw_menu) so the
   two modes share GPU + menu code without touching the working sim path. Also aligned
   the visible world to 200 bushes so every hiding spot the sim/prowl use is drawn.
   Pop Island in-window is the remaining mode. (App wiring is compile-verified only —
   no display in the sandbox.)
-10. [x] Wired the shared `ring` into **Prowl** as real pressure (it was an unused
   primitive): a shrinking hunt-ring where, once you're *outside*, cover no longer
   conceals you from the hunter and stamina bleeds — forcing the prowl inward. Rendered
   as a translucent violet storm wall at the live radius (starts wide, so it's off the
   close follow-cam early). Test: cover hides you inside the ring, fails outside it.
   Also extended **menu.rs** (tested UI logic) with a game-mode selector — the Play
   screen now cycles Game (Rendezvous / Survive / Prowl / Pop Island) · Hunter · Class,
   and commits either `StartSingle` (the 6v1 sim) or a new `StartMode(game, hunter,
   class)` for Prowl/Capture. App handles the new command (live in-window Prowl/Pop
   Island is the next milestone; sim modes play as before). 2 new tests → 60 total.
-9. [x] Ported the second prototype — **Prowl** (the SMITE-2-inspired mode) as a shared
   lib (prowl.rs), reusing class/hunter/ecology/objectives. You prowl as a chosen class
   while a hunter stalks you, clearing **rotating objectives** (Reach a biome landmark,
   hold a Rendezvous, Survive the hunt, Forage N prey), hunting prey by pouncing
   mid-leap for score, with respawn-on-catch (not permadeath) and escape/catch tallies.
   Movement is terrain-scaled through the class, so the map shapes the chase (an otter
   thrives on this water-heavy map where a squirrel drowns — verified integrated: Otter
   vs Badger cleared+rotated objectives, caught once; Squirrel vs Fox drowned). Objective
   landmarks are named by biome ("Reach the Moor Plains"). Headless `prowl` render mode
   (POI pillars, objective beacon, prowler + stalker). 5 unit tests (reach clears+rotates,
   survive counts down, leap-pounce feeds forage, catch respawns, airborne is uncatchable).
-8. [x] Ported the two new browser prototypes toward the native engine as **shared
   libs**, starting with Pop Island 2 ("Tails of the Wild — Yarn Ball Capture"):
   * **class.rs** (shared) — six terrain-specialised creature classes (Cat/Squirrel/
     Otter/Frog/Goat/Hawk) with land/water/mtn speeds, jump, stun-resist, fly; `speed_at`
     reads the world heightfield so who's fastest depends on land vs water vs mountain.
   * **powerup.rs** (shared) — Speed / Shield (one-hit) / Triple-Toss buffs with a
     `Buffs` struct (grant/tick/absorb_hit/throw_count).
   * **ring.rs** (shared) — a shrinking safe-zone/dome (radius easing r0→r1, damage
     outside, flee-to-safety vector) for the Prowl storm, next.
   * **capture.rs** — the full Pop Island mode as deterministic renderer-free logic:
     two teams (Tawny/Silver), yarn-ball carry-to-base scoring, firecracker throws that
     stun and make carriers drop yarn (shield soaks it), power-up pickups, terrain- and
     buff-scaled movement, simple fetch/throw AI, two timed periods → winner. Headless
     `capture` render mode (team bases, yarn, power-ups, team-collared creatures).
   15 new unit tests (class 4, powerup 3, ring 2, capture 6). Prowl (SMITE-2 mode:
   objectives, jungle camps, blink ability, the ring) is the next pass and will reuse
   class/powerup/ring.
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
