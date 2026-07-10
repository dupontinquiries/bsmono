## ⌖ SHARED vs PER-SCENE LEDGER (read before editing — avoid cross-scene breakage)

SHARED CODE (a change here hits MULTIPLE scenes — only touch with a per-scene guard/param):
- `tree_branch(s, origin, dir, LEN, RAD, depth, seed)` — used by Forest, Treehouse, Canopy. Callers pass their OWN
  len/rad/depth, so changing a CALL's args is scene-local (e.g. Canopy trunk). But tree_branch's BAKED green MOSS
  leaves are shared internals → changing them (e.g. for Forest winter barrenness) hits all 3. To vary, add a param
  with a default that preserves others.
- `leaf_cluster`, `leaf_single` — foliage + conversation NODES everywhere.
- `critter` / `critter_m(motion)` — Forest + Treehouse.
- `contact_shadow` — every scene.
- `dome_cap` — Atlantis + Mansion Garden. `room_column` — Atlantis + Mansion. `bookcase` — Old Library (+ Atlantis once coral-shelf lands). `waterfall`/`standing_stone`/`stalactite` — Mistfall only (currently).
- `petrified_wall`/`petrified_arc` — Canopy only (currently).
- THE SHADER `gpuscene.rs` SHADER_VS/FS — shared by ALL scenes; per-scene effects are keyed by mat id + emissive
  value (NOT theme). Reserved signatures so far:
    * MOSS(4) + emissive 1.6-2.0 = firefly twinkle; >2.0 = lightning strobe.
    * emissive 4.5-6.0 = Mansion GOLD-VEIN pulse.   * emissive >6.5 = Mansion GOLDEN RAINDROP (VS fall + FS glow).
    * STONE(2) + emissive≈0.22 = Mansion MEMORY MARBLE.   (pick a fresh band for any new effect.)
- Per-scene-but-safe tables (keyed by theme name, edit freely): `scene_light()` + the `bloom_amt` match (chat.rs),
  cord settings (cord_r/amp/glow/seg), NodeStyle/strand_mat.

EXPLICIT PER-SCENE OVERRIDES the user asked for (do NOT propagate to other scenes):
- CANOPY: natural GREEN non-emissive canopy (never calcite balls/emissive); SHORT+WIDE trunk (tree_branch 6.0/1.3);
  curved shelves (petrified_arc); wide courtyard; air-filament connector cord; stepping stones; shelves +25%/2x deep.
- MANSION GARDEN: OCCASIONAL golden rain; memory marble; NO centre rings; toned brightness/bloom; GENTLE wind; sc scale.
- MISTFALL: water LOW (-4.4) + bounded, everything floats; spread mossy stones; turbulent waterfalls; ceiling+stalactites.
- READING ROOM: bloom 0.5 + lexp 0.82 (toned, no blown whites).
- FOREST: season system (gentle elsewhere unaffected); logs/skunk/rocks.
- TREEHOUSE: creatures grounded + bridge sway; gentle.   CLOUDSCAPE: moonlit + particleized.
- ATLANTIS: self-enclosing (excluded from generic shell); banners; perimeter soffit.


## 2026-07-09 (pass 69) — CHAT STATUS BAR: live fan/queue HUD + long-horizon persona badge

GUI pass on the chat tab's status line (`modal_banner`), two small always-on indicators in the right-aligned
cluster (beside the `?` button):

- **Live activity HUD (fan commands).** A running `:Nca`/`:Nfa`/`:fuzz` fan queues dozens of generations over
  minutes, but its progress only flashed once through `self.note` and got overwritten by the next message. Added
  `Chat::activity_status() -> Option<String>` (pure) summarizing in-flight work every frame — "generating ·
  N queued · M rounds left" (singular/plural grammar) — rendered in cyan with a **⏹ stop** button that clears the
  queue + cancels the fan (same effect as `:stop`). None when idle. Headless render hook `LOGSCOPE_FAN_DEBUG=<text>`
  forces the HUD (mirrors `LOGSCOPE_MIC_PEARL_DEBUG`).
- **Persona badge (long-horizon).** The active system persona was invisible in the GUI. Added
  `Chat::active_persona_label()` (pure) — matches the live system prompt against the `PERSONAS` table
  ("Long-horizon engineer" by default, "Logscope · quick log fix" for the downshift persona, else "custom ·
  system") — shown faint next to the HUD, hover points to the Context page.

Verification: 2 new unit tests (activity_status grammar/combination + persona matching); suite 467/467 (was 465).
New headless harness `src/bin/status-view.rs` drives the REAL `Chat::ui`, so the screenshots are the production
widgets. Rendered idle vs. forced-fan under Xvfb+llvmpipe; OCR of the status band confirms the HUD text appears
only in the fan state and the persona badge appears in both. No scene geometry touched.

NEXT (same GPU-only scene frontier as pass 68): Atlantis rings/ceiling (needs user pinpoint), Mistfall
anti-gravity waterfall polish, Cloudscape night tuning, Forest stateful leaf decay + critter wander, Treehouse
wood-grain shader.

### pass 69b — status-bar affordances made interactive (fan launcher + clickable persona)
Follow-on landed the same day: the read-only badge became a **click-to-switch persona popup** (wires the
formerly-parked `persona_by_key` seam; same effect as the Context page, now inline), and a **⑃ fan launcher**
popup appears when idle so the fan commands (`:ca` · `:3ca` · `:fuzz 8` · `:fa 4`) are reachable from the GUI
without dropping to the `:` cmdline — each item calls the same `run_command` path the cmdline uses. Both use
the shipped `popup_below_widget` + `selectable_label` pattern (from `model::folder_popup`). +1 round-trip test
(persona_by_key ↔ badge label); suite 470/470. Note: the popup *contents* are covered by unit tests + the reused
pattern + a clean compile of the nested closures; the live popup open is best eyeballed on-GPU (a hover/click
popup over the animated scene resists headless screenshotting). Buttons + badge render (OCR-confirmed on the
status row).





Housekeeping/verification pass; no scene geometry changed. Rust 1.96 toolchain re-provisioned from the
offline bundle; software-GL render loop (Xvfb + llvmpipe, GL 4.5 / GLSL 330) confirmed working.

Fixes (all headless, root-caused):
- **egui lib suite was 2 red → now 465/465** (core `logscope` 166/166). Both failures dated to the default
  chat persona switching from the short inline BASE_SYSTEM to the file-based LONG_HORIZON:
  * `prompts/long_horizon.md`: `LogScope` → `Logscope` (lone camelCase outlier: 47 `Logscope` vs 1 in the
    suite; it was also the sentinel `fix_mode_appends_only_when_on` + `log_block_injected_and_bounded` use to
    assert the base persona is present in `full_system()`).
  * `log_block_injected_and_bounded`: bound the LOG BLOCK *delta* over the base persona, not the whole prompt.
    The old `fs.len() < 4000` measured base+block and the 7.7 KB long-horizon persona blew it, even though the
    block itself summarizes 1000 lines to ~0.3 KB. New bound (<1500) still catches a raw dump (~20 KB) but is
    independent of persona size.
- `ui.rs::carved_skin_tokens_are_sane`: replaced a dead `s.fill_down <= 255` (always true for a u8 — a
  "comparison is useless due to type limits" warning) with the real pressed-state invariant
  `fill_off <= fill_down <= fill_hover` (the press sits inside the [off,hover] opacity envelope; carved()'s
  70/165/174 satisfy it, a broken skin wouldn't).

Mic-pearl SHAPE derivative (pass-67 NEXT) spot-checked via `mic-pearl-view` (DEBUG=active, SCALE=6) across
one scene per shape + both bespoke mounts — Atlantis (Pearl→scallop shell), Fractal Spires (Gem→gold claw
cradle), Mistfall (Stone→rune plinth), Old Library (bespoke book-lectern), Forest (Leaf→dew stem),
Folding@home (bespoke orbital cradle). All read as distinct, scene-appropriate placed objects; the logic side
is already total over PALETTES (`mic_shape_is_a_coherent_per_scene_derivative`). No change needed — the
feature is complete. Montage archived alongside this bundle.

NEXT (unchanged real frontier, from the §8 backlog): per-SHAPE second axis is done; remaining scene work is
GPU-only (Atlantis ceiling gaps / mid-area rings — needs the user to pinpoint which rings; Mistfall
anti-gravity waterfalls; Cloudscape storms + moonlit night; Forest persistent-leaf stateful system + darting
critters; Treehouse wood-grain shader). Each is its own turn, built one at a time as a reviewable patch.


## 2026-07-02 (pass 67) — MIC PEARL: marble-sheen voice indicator, derived from the panel idiom table

A small in-scene orb beside the message panel (billboarded, camera-facing) showing live voice state:
  - asleep (mic off) — dim grey mote, barely breathing.
  - armed (mic on, no speech yet) — a warm marble shell, slow breathing glow in the scene's own accent
    colour (reuses the panel's (pc,tc,tglow,pmat) idiom table verbatim — so every scene's mic pearl is a
    FREE, coherent derivative of its message-panel idiom, satisfying the 'plan derivatives for other
    scenes' ask by construction rather than a bespoke per-world table).
  - active (hearing an utterance, i.e. voice.partial() non-empty) — brighter/faster pulse + two orbiting
    cyan-tier sparkle motes, so 'catching your voice' reads before whisper even finalizes.
Deliberately a DIFFERENT physical object from the flat resonance-cell text (glossy sphere vs glyph cells)
per the 'behaves differently to the text pearl' ask.

VISUALLY VERIFIED headless (Xvfb + llvmpipe), not just code-reviewed — built a new debug harness
`src/bin/mic-pearl-view.rs` (constructs a real `Chat`, calls the production `scene_ui`, loads a sample
.logscope so the panel/pearl actually populate — Chat::default() alone shows the empty-state placeholder).
LOGSCOPE_MIC_PEARL_DEBUG=armed|active forces the state (the `voice` feature can't build in-sandbox, so this
is the verification path — kept as a permanent debug hook for checking derivatives on the remaining scenes).
Atlantis render: 6612 bright(>200) pixels in the panel band, sample colours (121,250,255)/(156,239,255) —
near-exact match to Atlantis's tc=(156,238,255) accent, confirming the pearl picks up the scene tint
correctly. Canopy render showed a different bright-pixel count (3406, real per-scene geometry variation,
not a cached/identical frame). Default build + gpu-scene bin compile; chat (15) tests green.

NEXT for the derivatives plan: spot-check the mic pearl on the remaining scenes (Mistfall carved-rune,
Archive/Reading ink idiom, Mansion Garden marble+gold, wood-board scenes) the same way, and consider a
per-scene SHAPE variant (not just colour) — e.g. a faceted gem for Fractal Spires/Recursive Citadel vs a
smooth pearl for Atlantis/Mistfall — as a second derivative axis once the colour pass reads well.

## 2026-06-30 (pass 66) — FREECAM: free-flight camera (step 3, part 1)

Added a free-flight camera mode to the spatial view, reusing OrbitCam (no parallel camera).
  - Toggle: “✜ Freecam” / “■ Exit freecam” button next to Tour; mutually exclusive with Tour, and it turns
    off spatial_auto (auto-frame) so the user has full control. Exiting hands back to Overview.
  - Controls: WASD move along the look basis (fly where you look), Q/E down/up, Shift boosts (16→42 u/s),
    drag looks (existing orbit drag), scroll zooms (existing). Movement = translate orbit.target along
    fwd=(cp*sy,-sp,-cp*cy)/right=(cy,0,sy) (same basis as the fly eye), eye follows (dist held).
  - Gated on canvas hover/drag so WASD never leaks into the composer; repaints continuously to poll keys.
App compiles; chat (15) tests green. Runtime egui path — verify visually with the Freecam button.

NEXT (step 3 remainder):
3b. Perf pass, UI overlay (freecam HUD / controls hint), whisper STT.

## 2026-06-30 (pass 65) — CAMERA: AABB clamp completes the bounded spline traversal (step 2)

Step 2 was mostly already in place: the Tour fly (fly_auto / LOGSCOPE_FLY) ALREADY splines the camera along the
strand via Catmull-Rom through the 4 surrounding node positions (here + a look-ahead on the same spline), with
a cinematic-shot director and collider-based evasion (out of Pillars, inside the perimeter Bound, between
floor/ceil). The missing piece was the “clamped to the mind-palace AABB” guarantee: the perimeter Bound
deliberately OPENS at gates, so the fly could slip out a hallway mouth and sail past the hall end into open
ocean (the pass-63 overshoot the virtual end-cap only partly caught).
Added (chat.rs, fly block): a HARD OUTER BOUND. Extend the floor/ceil collider pass to also accumulate the
horizontal collider extent (Pillars = halls+wings, Bound = rotunda), union it with the strand NODE extent (so
the clamp can never fight the spline or the ESTABLISH pull-back — the eye can always reach every node), and
clamp the eye into that AABB +4u. Applied through the eased fly_push (shifts target+eye together), so it eases
in smoothly, not a snap. App compiles; scene3d (7) + chat (15) tests green.
Note: runtime egui path, so not unit-tested headless; verify visually with the Tour button / LOGSCOPE_FLY_AUTO.

NEXT:
3. Freecam (WASD/drag free flight), perf pass, UI overlay, whisper STT.

## 2026-06-30 (pass 64) — CLEAR THE CENTRAL LANE in Atlantis (step 1 of pass 63's NEXT)

Done: the rotunda colonnade now leaves a ±4u corridor along the x-axis (the routed strand's lane), so the
strand threads OPEN space instead of the columned centre. In atlantis()'s colonnade loop, after computing the
column (cx,cz), skip when cz.abs() <= min(4.0, r_col*sz*0.6) — the cap keeps the OUTER columns on small halls so
the colonnade never fully vanishes. Atlantis-only (inside atlantis()); Mansion's shared room_column untouched.
Regression test scene3d::tests::atlantis_central_lane_cleared_for_strand: colonnade (r=0.8 pillars) still ≥6
columns, 0 in the lane. Headless/CPU (with_env + env_colliders), so it runs without a GPU.
Note: a lone WING/region landmark (r=1.1) can still sit in the lane far down a hall — that's the obstacle
FLEX's job (pass 62), which pays off now that the dense colonnade is out of the lane. Not the colonnade.
VERIFY ON HW: LOGSCOPE_GEOM_DUMP=1 then tools/geom_check.py --all — expect Atlantis obj_clip to drop from ~72%
while strand_aspect stays high (couldn't run the full geom dump headless: node positions need the egui layout).

NEXT (unchanged from pass 63, now that step 1 is done):
2. CAMERA spline (Catmull-Rom through nodes, clamped to the mind-palace AABB) for smooth bounded traversal.
3. Then freecam, perf, UI overlay, whisper STT.

## 2026-06-22 (pass 63) — hall-routing makes Atlantis a STRAND + strand_aspect metric

Continued harness-driven. Key result: the user's "not aligned with the hallways" is about strand SHAPE (follow the
corridor) more than literal clipping. Added:
- HALL ROUTING (chat.rs, Atlantis-only): compress the strand laterally (z*0.42) so it runs DOWN the hall axis (x)
  as a strand instead of ballooning into the rotunda. Top-down it now reads as a left-right band with the side
  regions at the hall ends.
- New harness metric STRAND_ASPECT (x-extent / z-extent): >2 = strand, ~1 = blob. Atlantis went blob(~1) ->
  strand(2.78) with routing. So the harness now captures the alignment goal, not just clipping.
- TRADEOFF (measured + accepted): routing RAISES obj_clip slightly (69%->72%) because Atlantis HALLWAYS are
  themselves colonnades — the central lane is columned, so a tighter strand packs onto those columns. The clip
  metric and the alignment goal diverge here; alignment (the stated concern) wins.

NEXT (now precisely scoped by the harness):
1. CLEAR A CENTRAL LANE in Atlantis: drop colonnade columns within ~±4u of the x-axis so the routed strand sits in
   a genuinely open corridor -> strand_aspect stays high AND obj_clip drops. (Then the flex handles the rest.)
   Verify both metrics with geom_check.py.
2. CAMERA spline (Catmull-Rom through nodes, clamped to the mind-palace AABB).
3. Then freecam, perf, UI overlay, whisper STT.

Harness usage recap: LOGSCOPE_GEOM_DUMP=1 -> /tmp/scene_geom_<theme>.json; `python3 tools/geom_check.py --all`.
Metrics: below_floor, above_ceiling, obj_clip, out_bounds, spine_axis, strand_aspect.

## 2026-06-22 (pass 62) — harness-driven obstacle-avoidance experiment (BIG diagnostic)

Built on the geom self-check harness to actually fix the Atlantis graph<->scene clip. Findings (all MEASURED):
- TUNING is a dead end: clear/breathe/height knobs leave Atlantis obj_clip ~71% (no-breathe = WORSE 83%, tight
  lane = WORSE 91%). Don't pursue tuning.
- OBSTACLE-AVOIDANCE FLEX added (chat.rs, cached per (node-count, env)): extract mid-size solid object AABBs via
  with_env, push each node out of overlaps along the smaller x/z penetration (z-biased to keep the time axis),
  clamped near its spine spot. The harness shows it MOVES ~284/412 Atlantis nodes but obj_clip stays ~69% — they
  just re-clip. Tightening obstacles to SUBSTANTIAL solids (vy>1.5, footprint>0.4) cut Atlantis 4639->472
  obstacles, but clip held. Conclusion: the Atlantis ROTUNDA is genuinely too dense for local avoidance — the
  graph is a BLOB filling it, not a strand. Other scenes are already clean: Old Library 5 obstacles/0% clip,
  Archive 28/1%, Reading Room 128/1.2%. **Atlantis is the lone outlier.**
- The flex is kept as a FOUNDATION (sound primitive, cached, doesn't scatter, no-op on clean scenes). It only
  pays off once the graph is ROUTED out of the dense rotunda.
- Harness obstacle filter tightened to match (substantial solids), so obj_clip now measures meaningful clipping.

THE REAL FIX (next, now precisely targeted by the harness):
1. ROUTE the Atlantis strand along the OPEN HALL AXIS instead of ballooning into the rotunda: spread the graph
   long on x (down the colonnade gaps / hall openings) and NARROW on z (stay in the central lane), so it threads
   the open corridor rather than filling the furnished rotunda. Then the existing flex cleans up residual column
   overlaps. Verify obj_clip drop with geom_check.py. (This is scene-aware layout, the "scene flex" the user wants.)
2. CAMERA spline (Catmull-Rom through nodes, clamped to the mind-palace AABB) for smooth bounded traversal.
3. Then freecam, perf, UI overlay, whisper STT.

## 2026-06-22 (pass 61) — feedback: scene dropdown, graph floor-lift, Mistfall pool, mega example

- SCENE-SELECT DROPDOWN in scene-view (ScenePreview::ui): an egui ComboBox (top-left overlay) over theme::PALETTES;
  on pick -> theme::install_palette, the scene rebuilds from theme::active() next frame. Verified switching works.
- GRAPH "INTO THE GROUND" fix: replaced the Atlantis-only q.y+=1.9 hack with a general FLOOR-CLEARING lift in
  chat.rs — compute the graph's min y and lift the whole strand so its lowest node sits ~-1.6 (clear of typical
  floors ~-3.2), ONLY ever lifting up (open scenes unaffected). Atlantis strand now floats above the floor.
- MISTFALL water: the -5.8 disc (radius ~78) read as a high frame-filling SEA-horizon; made it a low contained
  POOL (-6.3, radius 22*ms, alpha 104) + deep bed -6.8/24*ms. (Stalactites hang at y 5-13, nowhere near it — the
  "water at the stalactites" was the sea-horizon perspective.)
- MEGA example: examples/ladder/AA-mega-spine.logscope (180 turns + branches/widening -> 359 nodes ≈ 2.5x the
  marathon) to stress BIGGER rooms. scene-view now DEFAULTS to it (override via LOGSCOPE_CONVO).
- 259 tests pass.

STILL OPEN — the architectural core the user keeps flagging:
1. GRAPH<->SCENE ALIGNMENT ("graph matching not finalized"): the strand should run ALONG the hallways/corridors
   (Atlantis x-halls, Old Library aisles) and FLEX around shelves/objects, not cluster in the rotunda. Needs the
   layout to take scene structure (hall bearings, walkable lanes, object positions) as input — thread these into
   spatial::flow (or a post-pass) so the spine follows the main axis + branches follow door bearings + nodes avoid
   object footprints. This subsumes the per-object avoidance note.
2. CAMERA too stiff / ricochets: replace the segment-to-segment pan with a smooth spline (Catmull-Rom through the
   node positions) clamped to the room bounds (mind-palace AABB) — traverse the graph smoothly without overshoot.
   Then: freecam, perf, UI overlay, whisper STT.

## 2026-06-22 (pass 60) — feedback batch: Wayland ghosting, Mistfall water, Forest, Treehouse critters

User feedback batch. Done:
- WAYLAND GHOSTING: app windows behind logscope showed through on KWin/Wayland. Cause: window framebuffer
  presented with alpha<1 where the scene/UI didn't paint opaque (renderer clears offscreen to alpha 0). Fix:
  override eframe App::clear_color to force alpha 1.0 (LogscopeApp + ScenePreview) + .with_transparent(false) on
  both viewports. (Verify on device.)
- MISTFALL water: surface -5.2 -> -5.8 and alpha 172 -> 132 (more translucent) so the depth bed + low objects
  read instead of being occluded.
- FOREST: ground litter + hero trees + tall-thin trees now CONFORM to the displaced terrain (was flat yf -> sank
  under raised ground = "grass under terrain"). Upper-canopy leaves shrunk: tall-thin high crowns 1.0-1.5 ->
  0.55-0.85; hero canopy puffs 1.5 -> 1.15.
- TREEHOUSE critters reworked: rabbits + cats now on the FOREST FLOOR (y=-8) not perched on decks; hanging
  creatures now dangle from procedural ROPES dropped from the tree canopy ABOVE the walk (not platform rails);
  bridge-walkers kept (rabbits/cats riding the plank sway) + a NEW traverse pose: a hanging creature CROSSES a
  bridge gripping the handrail rope, feet skimming the plank.
- 258 tests pass + the known offline network flake (normal).

STILL OPEN (next): RINGS the user wants gone — 212/215 in Atlantis are door/column parts, NOT rings; need the
user to say which SCENE. MANSION GARDEN — keep working. READING ROOM double-floor "colosseum" shape — keep
working (user will keep giving feedback). Then the TODO backlog (flythrough/freecam, perf, UI overlay, whisper STT).

## 2026-06-22 (pass 59) — space out centerpieces + glow-pool grounding (per user)

User: "space out the centerpieces more" + keep pushing the maps. Done:
- SPACING: pushed satellites well clear of the central piece across all four maps. Old Library 6.5+i*0.6 ->
  10.5+i*2.4 (deep into the diagonal gaps); Reading Room 5.2*rs -> (7.0+i*0.3)*rs (out toward the colonnade);
  Archive nave step depth*1.6/N -> (2*depth-3)/N (spread down the FULL nave) + wider px alternation (±3.6);
  Mistfall rad (4.5+h*4) -> (7.0+h*5) and height spread ±4 -> ±5 (still fog-readable).
- GLOW POOLS: each Old Library lectern now drops a soft emissive disc (0.10, tinted to its tome) — grounds it as
  a lit "station of thought"; reads great at night. (Archive keeps its contact shadows; Reading Room rug + Mistfall
  water left clean.)
- 259 tests pass.

NEXT (still maps, per user, before TODO/backlog): possible per-OBJECT graph avoidance now that centerpieces are
multiple + spread (thread positions into the layout); centerpiece FORM variety (not just scaled clones);
Atlantis hall junction (needs user pinpoint). THEN flythrough/freecam, perf, UI overlay, whisper STT.

## 2026-06-22 (pass 58) — multi-centerpiece extended to Mistfall / Archive / Reading Room

Applied the Old-Library multi-centerpiece pattern (N = clamp(nodes/22, 1, 5); main at native spot + satellites
placed clear of structure; per-station seed variation) to the remaining single-centerpiece maps. Verified big
(marathon) vs small (lru-fill) via LOGSCOPE_CONVO:
- MISTFALL: one portal ring -> a vertical ARCHIPELAGO of standing portals; satellites at varied angle/height/size
  (rad (4.5+h*4)*(1+(ms-1)*0.5), kept near so they read through the fog), hue from a 5-colour palette, varied
  amp/phase. (Subtle in the moody scene but present.)
- ARCHIVE: one lectern at the far end -> lecterns stepping DOWN the nave (lz = depth-1.6 - i*(depth*1.6/N)),
  alternating gently off the central aisle, emissive varied per station + contact shadows.
- READING ROOM: one central table -> central + satellite tables in the diagonal gaps (rad 5.2*rs), smaller,
  emissive varied per station — sits on the (colour-salience-driven) rug.
- 259 tests pass.

NEXT: Atlantis hall junction (needs user pinpoint), then flythrough/freecam, perf, UI overlay panels, whisper STT.
The multi-centerpiece + content-driven-colour patterns are now the template for "scene reacts to the conversation".

## 2026-06-22 (pass 57) — procedural: multi-centerpiece scaling (Old Library) + Atlantis big-chat repro

Found scene-view can load any conversation via LOGSCOPE_CONVO (default examples/ladder/99-fullspine-marathon =
big chat; examples/02-lru-fill = tiny) — so big-vs-small is now reproducible/verifiable.
- MULTI-CENTERPIECE (per user "port single centerpiece -> multi by graph size"): Old Library's single lectern is
  now N = clamp(nodes/22, 1, 5) lecterns. Main at the heart + satellites in the DIAGONAL gaps between the four
  bookshelf corridors (never block an aisle), each smaller. Each lectern VARIES by a per-station seed (tome glow
  hue from a 5-tint palette + emissive 0.45..0.67 + page count) so they read as distinct "stations of thought"
  ("more envparams to distinguish by surrounding context"). Verified: marathon -> several glowing tomes; lru-fill
  -> one. 259 tests pass. Pattern is reusable for Reading Room (table), Archive (lectern), Mistfall (portal).
- ATLANTIS halls: reproduced the big chat (marathon) top-down + oblique. The side WINGS actually sit ~near the
  graph reach (dist ~32 vs ext ~34), so it's NOT a gross wing-distance cap. The specific "don't line up" is likely
  the hallway<->rotunda junction or the two x-axis halls' collinearity — needs the user to point at the exact
  spot (or diagnose in the other chat). Deferred rather than edit blindly.

NEXT: apply multi-centerpiece to Reading Room / Archive / Mistfall; get Atlantis misalignment specifics; then
flythrough/freecam, perf, UI overlay, whisper STT.

## 2026-06-22 (pass 56) — procedural-gen polish: graph breathing/force model + remove concentric rings (per user)

Tagged handoff-2026-06-22 (user will patch/continue in another chat). Roadmap from user: more procedural gen now,
then flythrough cam (graph-tied) + freecam, perf, UI overlay panels, voice STT (whisper, maybe another Claude).
- BREATHING-ROOM FORCE MODEL in spatial::flow(): after the time/sibling placement, a gentle relaxation — z/y-only
  node repulsion (radius ~2u, so it de-clumps crowding turns WITHOUT exploding the flow line; x/time stays anchored
  so reading order is preserved) + soft center-clearing (push nodes out of the central zone where a scene's
  centerpiece sits) → the conversation flows AROUND objects. Knobs: LOGSCOPE_TUNE_BREATHE (default 1.0),
  LOGSCOPE_TUNE_CLEAR (central radius, default 4.5). Spatial tests (separation invariants) still pass.
- REMOVED the 3 concentric floor TERRACES from shared enclose() (the "brief's nested rings"): confirmed via
  LOGSCOPE_DEBUG_LABELS they were big origin-centred nested annuli (radii 260/350/439 in a large Slate) dominating
  the floor — the "215/249/lower-200s concentrics" the user flagged (indices shift with chat size). Floor disc +
  central platform remain; floor now reads clean. (Oculus rim torus was already gone.)
- 259 tests pass.

NEXT (still procedural, per user): Atlantis perimeter HALLS don't line up on bigger chats (hall_dims/wing
placement vs large graphs) — fix alignment; single-CENTERPIECE rooms -> MULTI-centerpiece scaled by graph size;
add more EnvParams so scenes distinguish visually by surrounding context. Then flythrough/freecam, perf, UI, STT.
True per-OBJECT graph avoidance (vs current center-clearing) is a later upgrade (needs object positions in layout).

## 2026-06-20 (pass 55) — CONTENT-DRIVEN colour: Reading Room rug reacts to the conversation (per user)

User: mute the rug colours and only show them when the chat mentions "colour", scaling up with use — mixing raw
frequency + percentage + basic NLP. Now the ENVIRONMENT reflects the conversation's CONTENT, not just its shape.
- chat_color_salience(texts) -> [0,1] (chat.rs): lowercase tokenise + stem-match a colour lexicon
  (color/colour/hue/palette/pigment/chromatic), then mix raw-frequency (saturating 1-e^(-raw/5)) with
  term-frequency % (tf*120 clamped), 50/50. Unit-tested (0 with no mentions, strictly rising with use, ≤1).
- Threaded via EnvParams.color_salience (new f32 field; dropped Eq derive; fixed 4 test literals) set in chat.rs
  alongside nodes/seed/etc., read by reading_room().
- reading_room() rug: lerp each band mix_rgb(neutral_taupe, jewel, salience) + emissive 0.02+0.04*sal. salience 0
  = uniform muted mat (hue 0.02); 0.5 = colours emerging (hue 0.56); 1.0 = full jewel medallion (hue 0.79).
  LOGSCOPE_COLOR_SALIENCE env override for testing/tuning.
- 259 tests pass. Verified before/after renders (muted taupe vs jewel medallion).

This is a reusable pattern: drive other scene attributes from content signals (EnvParams + a salience fn). NEXT:
maybe extend to other lexicons/scenes (e.g. "water"→Atlantis caustics, "growth"→Forest density); keep mapping
DENSITY/PITCH; Slate hue tint; Mansion dawn ambient floor.

## 2026-06-20 (pass 54) — act on the map: HUE is the systemic weakest metric → lift it where it's worst

The rule-map sweeps showed HUE is the recurring weakest sub-metric across most scenes. Two GPU-independent fixes
(saturated colour reads regardless of llvmpipe brightness):
- SHARED bookcase_fill: the 10 spine colours were dark/desaturated → barely weighted in the sat-weighted hue
  entropy. Saturated them to richer jewel tones. Lifted Archive (hue_sub 0.31, score 0.78) + Old Library
  (0.56 / 0.69) at once. Reading Room unaffected (its books are peripheral; bright marble dominates the frame).
- READING ROOM (the weakest scene, ~0.63 even at its safe evening hour, hue ~0.0): added a concentric jewel-tone
  RUG in the central frame. hue_entropy 0.019 -> 0.887, score 0.63 -> 0.74. First pass was garish (primary-colour
  dartboard) so HAND-TUNED to deeper, muted medallion tones (hue 1.0 -> 0.85, classier) — traded a little score
  for taste.
- 258 tests pass.

NEXT: keep mapping (DENSITY x scene, PITCH x scene) then run rule-aware chains on remaining weak scenes; consider
muted accent colour for Slate (grey, hue-weak) e.g. lichen/mineral tints; the Mansion dawn-black ambient floor.

## 2026-06-20 (pass 53) — MAP THE RULES: sweep_matrix tool → what breaks scenes (+ feed back into search)

Per user: use the tools to test higher-level abstractions/relationships and MAP what breaks things vs what doesnt.
- tools/sweep_matrix.py: sweep one param across many scenes, render WORLD-ONLY, score, record score+failure+weakest
  metric to candidates/_matrix/<param>.csv. Reuses one Xvfb (fast) + resumable. RULE_MAP.md holds the findings;
  hour_heatmap.png visualises them.
- RELATIONSHIP 1 (HOUR x scene): material brightness is the breaker. FRAGILE bright-material scenes wash out in
  daylight — Mansion (BLACK@7 dawn, WASHOUT@10-13, best 22), Reading Room (WASHOUT@10-16, best 19), Atlantis
  (not_blown dips midday, best dawn/eve). ROBUST dark/matte scenes never blow out and peak at midday — Mistfall
  (spread 0.07), Slate (0.08), Folding (0.10). So bright=time-fragile, dark=robust.
- RELATIONSHIP 2 (SCALE x scene): scaling UP does NOT empty these scenes (scatter tracks map_radius) — Slate
  IMPROVES 0.74->0.85 (more menhirs), Mistfall flat, Folding mild+. Corrects the old "bigger=emptier" assumption;
  scale is a SAFE knob where density tracks room size.
- FED BACK INTO SEARCH: markov_search.py SCENE_HOUR_RANGE constrains FRAGILE scenes to their safe evening window
  (18-23) so the chain stops sampling washouts/blackouts. Verified Mansion chain now pins hour to [18,23].
- 258 tests pass. Genuine bug noted (not fixed — shared-light risk): Mansion blacks out at low sun; wants an
  ambient floor.

NEXT: map DENSITY x scene (where does clutter start hurting?) + PITCH x scene with the same tool; then run the
now rule-aware chains on the weak scenes.

## 2026-06-20 (pass 52) — fix: Slate had no animated geometry (float test flaked) -> drifting dust motes

scenes_build_and_float asserts each scene has something floating after animate(). Slate was ALL Float::STILL
(static stone) + enclose() is also all STILL -> Slate had zero floating geometry, so the assert was borderline and
tipped to FAILING (256/2) under parallel tests sharing process-env reads. Fix improves the scene too: added ~18
DRIFTING DUST MOTES over the henge (faint warm glints, slow rise/fall, density-scaled). Now 258/258 pass
deterministically (3x), incl. the usual sandbox test when network is up. Slate scores 0.69 and reads less dead.
Lesson: every scene needs at least one animated element; check the float test after adding static-only scenes.

## 2026-06-20 (pass 51) — Rust-vs-Three.js CALL + warmup speedup + TUNE_DENSITY wired + longer chain

User floated a Three.js bridge (screenshot 3js for fast iteration, object-separation/saliency over many candidates,
egui for final colour) but said Rust may be fast enough — my call. DECISION: STAY IN RUST. (1) A headless-browser
WebGL stack is ALSO software-rendered here (no real GPU), so no faster; (2) the bottleneck was WARMUP not frame
time; (3) object-separation/saliency can be done Rust-side from the mesh pipeline; (4) a 2nd renderer would drift
from the egui final, defeating the "tune in egui" step.
- WARMUP SPEEDUP (measured): score IDENTICAL at sleep 4/6/8/11/15 (~0.739) — the 15-20s sleep was padding; render
  reaches steady state in ~4s. Cut tune_harness.sh 16->6 + Xvfb 3->2, markov_search.py 15->6 + 3->2. ~2x faster
  end-to-end (13-render chain now ~140s vs ~250s).
- TUNE_DENSITY wired: tune_density() helper (clamp 0.3..3.0) + applied to Mistfall counts (boulders, islands,
  bioluminescence colonies, coral). Verified it bites (density 0.5->1.8 raises structure 0.20->0.23, hue 0.22->0.25).
  Other scenes still to wire.
- LONGER CHAIN (12 steps, fast loop): improved Mistfall 0.7261 -> 0.7299 at step 9 (scale 1.16, height 1.18,
  DENSITY 1.37, hour 11.6, yaw 1.15, pitch 0.35) — chain found higher density helps. Rendered winner as a hero.
- 257 tests (+sandbox flake).

NEXT: wire TUNE_DENSITY into the other detailed scenes; run resumable chains on the weakest scenes; optionally add
a Rust-side object-separation/saliency metric (per-material masks) as the user suggested — now cheap with fast loop.

## 2026-06-20 (pass 50) — WORLD-ONLY matching + pixel SSIM + MARKOV-CHAIN search (per user)

User: match only the WORLD pixels (mask the UI), pixel-for-pixel, with vastly wider fuzzing via "trajectory markovs".
- WORLD-ONLY render mode: LOGSCOPE_WORLD_ONLY=1 (chat.rs) suppresses the whole conversation overlay (clears proj →
  no 2D dots/cords/runes/labels; gates `graph` pack → no 3D node-bodies or composer caption). Renders ONLY the
  environment — the clean surface for matching. Verified on Mistfall (pure grotto, bioluminescence reads, no dots).
- PIXEL matching: score_render.py `--pixel --ref <world.png>` = global SSIM + (1-MSE) on common-size gray (true
  pixel-for-pixel for ALIGNED views; e.g. target screenshot or golden-render regression). Validated: self SSIM=1.00,
  different-scene SSIM=0.07. Unaligned concept art still uses color/layout/depth ref_similarity.
- MARKOV-CHAIN search: tools/markov_search.py — Metropolis/simulated-annealing walk over a WIDE continuous param
  space (tune_scale/height/density, hour, yaw, pitch); proposes Gaussian steps, renders WORLD-ONLY, scores, accepts
  better / probabilistically-worse with a cooling T, tracks best, CHECKPOINTS to mcmc_state.json (resumes across
  calls since rendering is slow). Demoed on Mistfall (5 steps): accepted/rejected correctly, centered init held as
  best (good start + short walk). Scratch dir only; tree untouched.
- 257 tests (+sandbox flake).

NEXT: wire TUNE_DENSITY per-scene so the chain can search detail density too; run longer chains (resumable) on the
weak scenes; capture aligned world "golden" refs to use --pixel for regression + targeted matching.

## 2026-06-20 (pass 49) — harness-DRIVEN fix: Mistfall monochrome -> bioluminescent colour (scorer-judged)

Working from the scorer (user can't check their GPU now) and optimising the GPU-INDEPENDENT metrics (composition/
hue), not brightness (llvmpipe-confounded). Clearest flag: Mistfall hue_entropy=0.10 (monochrome blue).
- RECALIBRATED the hue band: [1.4,2.6] was unreachable (best real scenes ~1.0). Now [0.85,1.5] (measured: Mansion
  1.02, Slate 0.81, Folding 0.55, monochrome 0.10). Makes the hue metric meaningful for all scenes.
- MISTFALL bioluminescence: replaced tiny motes (didn't move the histogram) with glowing COLONIES (5-bulb clusters)
  + larger glowing CORAL FANS in 5 varied hues (amber/jade/magenta/cyan/lavender) + warm lanterns. hue_entropy
  0.10 -> 0.97 (sub 0.00 -> 1.00). Reads as a living grotto; saturated colour registers regardless of llvmpipe
  brightness, so it's a real GPU-independent gain. Hero shot picked by a 4-way hour/angle sweep (h13, side angle
  scored 0.79 — water dominates less there).
- 257 tests (+sandbox flake). Presenting fewer/larger images per user.

NEXT (harness-driven, GPU-independent): scan all scenes' hue/coverage/structure/vspread for the next weakest,
fix + re-score; wire TUNE_DENSITY; widen sweeps. Avoid optimising brightness against llvmpipe.

## 2026-06-20 (pass 48) — REFERENCE-IMAGE scoring added to the harness (OpenCV)

Per user: feed reference images into the scorer (opencv color/pixel similarity + a depth-ish cue).
- score_render.py + `--ref <img> [--ref-weight 0.3]`: ref_sim = 0.70*COLOR (HSV Hue-Sat hist correlation,
  cv2.compareHist) + 0.20*LAYOUT (64x48 gray normalized cross-correlation) + 0.10*DEPTH (per-row luminance profile
  correlation = vertical value-layering / atmospheric-perspective proxy). final = (1-w)*rule + w*ref_sim; hard-fail
  renders gated so a palette match can't rescue a broken frame. (True MiDaS depth omitted — model weights aren't
  downloadable in-sandbox; row-profile is the practical depth cue for horizon-layered scenes.)
- tools/refs/<scene_key>.png = per-scene target; harness auto-passes it. Seeded refs/mansion_garden.png from the
  garden concept. Drop a <key>.png to supervise any scene.
- VALIDATED: vs the garden ref, Mansion REF=0.16 > Forest/Slate REF=0.00 (color correctly IDs the on-theme scene).
  Mansion sweep ranked afternoon (h16.5) candidates 0.44-0.49 and flagged mid-morning (h10.5) as WASHOUT 0.06 —
  the harness caught that hour 10.5 blows Mansion's marble out. color dominates because concept-art vs llvmpipe
  styles differ; ref_sim is best as a within-scene relative signal. 257 tests (+flake).

NEXT: add more per-scene refs as we get them; consider masking the UI caption/dots out of the ref crop for a
cleaner color match; wire TUNE_DENSITY; then run real sweeps to redo weak scenes (score picks finalists, I merge).

## 2026-06-20 (pass 47) — SCORED ITERATION HARNESS + design rules (new workflow, per user)

The bottleneck is iteration count. New loop: the harness sweeps many candidate parameterizations, SCORES each
render against measurable rules, ranks them, and montages the top few; I (agent) am the HAND-TUNER who reviews the
finalists + merges the winner. Candidates are pure ENV OVERRIDES rendered to a SCRATCH dir
(/home/claude/candidates/<scene>/) — the committed scene code is NEVER overwritten by the harness.
- SCENE_RULES.md (repo root): 7 weighted, measurable rules (exposure-in-band, not-blown, contrast, coverage/fill,
  hue variety, structure/edges, VERTICAL SPREAD) + hard-fails (black/void/washout) + the candidate param space.
  Weights/bands live in a json block the scorer reads verbatim — tune the spec there.
- tools/score_render.py: numpy/PIL image scorer -> weighted score + per-rule breakdown (JSON or one-liner).
  Validated: flags blown Reading Room tile as WASHOUT (0.20), penalizes monochrome Mistfall on hue (0.0), ranks
  rich tiles 0.73-0.76. UI chrome cropped; bg = top-corner median.
- tools/tune_harness.sh <Scene> [cand_file]: renders each candidate (one Xvfb/shot, 560x440) -> scores -> ranks ->
  scores.csv + top.png montage with score labels. Demoed on Slate (6 cands): height=1.7 edged height=1.0 (scorer
  rewards vspread), scale=1.4 scored lower (bigger room -> emptier -> lower coverage). Good signal.
- tools/gen_candidates.py: writes cand_<scene>.txt sweeps (seeds x heights x scales x hours x angles).
- NON-INVASIVE KNOBS wired: LOGSCOPE_TUNE_SCALE (map_radius mult), LOGSCOPE_TUNE_HEIGHT (flow vertical mult).
  LOGSCOPE_TUNE_DENSITY reserved (wire per-scene scatter counts next).
- 257 tests (+sandbox flake).

WORKFLOW each scene: gen_candidates.py -> tune_harness.sh -> review top.png + scores -> HAND-MERGE winning params
into the builder (only tree edit) -> re-score to confirm. NEXT: wire TUNE_DENSITY into the detailed scenes so the
harness can also explore detail density; widen sweeps; use the harness to redo the scenes that "were not great".

## 2026-06-20 (pass 46) — DETAIL pass: Folding@home (compute cell) + Archive (scaled nave)

- FOLDING@HOME rebuilt rich: ms-scaled membrane + lipid-bilayer speckle; a central tall double-helix PLUS satellite
  helices at varied positions/heights (helix factored into a closure); BETA-SHEET ribbon stacks; folded GLOBULE
  clusters; translucent VESICLES at varied heights; a faint distributed-COMPUTE LATTICE (sparse emissive work-unit
  nodes each linked to a pseudo-neighbour) filling the upper volume = theme + height; more free molecules. Reads as
  a busy molecular cell now.
- ARCHIVE scaled + enriched: nave footprint half/depth*asf=(map_radius()/40).clamp(1,2.4), column rows grow
  6->14; added hanging CHANDELIERS (emissive, varied heights) down the nave, floor TOMBS/reliquary chests along
  the aisles, and deep-warm BANNERS hanging between the columns. Bigger, grander cathedral.
- 257 tests (+sandbox flake). rustcheck --fast clean.

Detail pass has now hit the 5 sparse scenes (Slate, Reading Room, Mistfall, Folding@home, Archive) + the systemic
double/dynamic/height. NEXT: re-shoot the 7x7 grid; then a 2nd pass enriching the already-rich scenes + per-scene
terrain height where safe.

## 2026-06-20 (pass 45) — DETAIL pass: Reading Room (multi-level) + Mistfall (vertical archipelago)

Both scenes' fit-out was tiny inside the now-doubled enclose shell → scaled to the room + a lot more content + height.
- READING ROOM rebuilt: fit-out scaled by rs=(map_radius()/40).clamp(1,2.8); raised floor + inlaid ring; ~28 drifting
  tomes spread/varied-height; GROUND colonnade + an UPPER MEZZANINE gallery (balcony annulus + railing + upper
  colonnade) = real height variation; PERIMETER BOOKSHELVES ringing the wall on BOTH levels (bookcase_fill near
  map_radius*0.8/0.86); 8 reading desks; hanging LAMPS at varied heights (emissive globes on cords); book stacks;
  central reading table w/ glowing tome. Reads as a deep two-storey library now.
- MISTFALL enriched: ms=(map_radius()/40).clamp(1,2.6) scales the water bed/surface + submerged boulders + standing-
  stone ring to the room; islands → a VERTICAL ARCHIPELAGO (count grows with ms, spread wider, heights -1..+6.5);
  added drifting MIST LAYERS (faint translucent sheets at varied heights) for depth/atmosphere. Moody/dark so it
  reads subtly on llvmpipe — verify on real GPU.
- 257 tests (+sandbox flake). rustcheck --fast clean each edit.

DETAIL BACKLOG remaining: Folding@home, Archive next (then enrich the rich scenes further). Per-scene TERRAIN
HEIGHT (undulating floors) still pending. Re-shoot 7x7 grid after a couple more scenes.

## 2026-06-20 (pass 44) — graph-dynamic sizing extended to self-enclosing scenes

- CLOUDSCAPE: deck (30) + main cloud spread + far-cloud ring now * `ms = (map_radius()/56).clamp(1,2.4)` — the
  deck and cloud field grow with the graph (gentler factor than an enclosed room).
- MANSION GARDEN: sc 1.6 -> 1.6 * (map_radius()/64).clamp(1,2.2) — conservatory grows with the graph.
- Coverage now: enclose scenes (Slate/Archive/Reading Room/Folding/Old Library/Mistfall/Islands) via map_radius;
  Cloudscape + Mansion via ms; Atlantis already scales to spine_reach; Treehouse + Canopy already enlarged; Forest
  already uses env_params. So map size is doubled-ish + graph-dynamic across the board now. 257 tests (+flake).
- Verified renders: Cloudscape + Mansion bigger, graph arcs with height + fits.

STILL TODO: confirm/boost Forest dynamic extent; per-scene TERRAIN HEIGHT (undulating floors) for the flat enclosed
scenes (height variation beyond the graph); continue DETAIL pass (Folding@home, Mistfall, Archive, Reading Room);
re-shoot the 7x7 grid once the sizing + detail settle.

## 2026-06-20 (pass 43) — SYSTEMIC: doubled + graph-dynamic map size + more height variation (per user vibes)

User vibes: height variation lacking; map-fit-to-graph not great; double map size for all + make it dynamic to graph.
- `map_radius()` (scene3d): DOUBLED base that GROWS with the graph — reach = max(ext_x,ext_z, nodes*0.55),
  radius = (reach*2.4 + 40).clamp(80,260). Applied to the `enclose()` call → Slate, Archive, Reading Room,
  Folding@home, Old Library, Mistfall, Islands all now ~2x and scale up as the chat grows (enclose already scales
  its internals by r/40, so rooms grow proportionally). Graph now fits with margin; big chats won't spill the wall.
- HEIGHT VARIATION: `flow` layout vertical meander boosted from a flat ±1.1 to ±~3.4 (2.4·sin + 1.0·sin broad
  roll), bounded above the -4.4 floor. Node chain now arcs up/over in 3D instead of lying flat. (Shared → all scenes.)
- Verified: Slate/Old Library/Reading Room render bigger, graph arcs with height + fits the frame. 258 tests.

STILL TODO for the vibes (next): apply map_radius-style DYNAMIC sizing to the SELF-ENCLOSING scenes too (Forest,
Cloudscape, Mansion need it; Atlantis already scales to spine_reach; Treehouse/Canopy already enlarged) — "for ALL".
Per-scene TERRAIN HEIGHT (undulating floors like Forest/Atlantis) for the flat scenes. Continue DETAIL pass
(Folding@home, Mistfall, Archive, Reading Room). Then re-shoot the 7x7 grid.

## 2026-06-20 (pass 42) — 7x7 scene survey + git backup + DETAIL pass begins (Slate)

- GIT BACKUP: `git bundle --all` -> /mnt/user-data/outputs/logscope-history.bundle (full 48-commit history,
  portable) + tag backup-<date>. Tree zip still also produced.
- 7x7 SURVEY GRID (new standard): render_grid.sh renders 49 tiles varied by THEME x TIME (LOGSCOPE_HOUR
  7/10.5/13/16.5/19/22/3.5) x SEASON (Forest) x ANGLE x SEED, annotated, montaged to scenes-7x7-survey.png.
  Render in batches of ~7 (one fresh Xvfb per tile; heavy scenes sleep 20). All 49 non-black. grid_cfg.txt holds
  the configs; regenerate/extend as needed.
- SURVEY READ: richest = Atlantis/Forest/Canopy/Mansion/Cloudscape. SPARSE (detail targets): Slate, Folding@home,
  Mistfall, Archive, Reading Room.
- DETAIL — SLATE (was just islands()+enclose, nearly empty): NEW `slate()` builder + "Slate" dispatch arm. A
  standing-stone moor: HENGE of menhirs, scattered BOULDERS + scree skirts, tapered CAIRNS, stepped STRATA
  LEDGES at the rim, glowing QUARTZ VEINS (emissive toned 0.32/0.4 so they read as seams not lasers on a real
  GPU), central cracked slab w/ vein cross. Density scales with pr.nodes (expand-with-graph nod). 257 tests
  (+sandbox env-flake). Verified render: floor now full of geology.

DETAIL BACKLOG (add a ton to EACH, per user; broad vibes to come): Folding@home, Mistfall, Archive, Reading Room
next (sparsest), then enrich the already-good scenes. Then per-scene light tuning, Atlantis overhaul, expand-
with-graph generalization, terrain halls for Forest/Mistfall.

## 2026-06-20 (pass 41) — terrain-conforming Atlantis HALLS with stepdowns (done) + rustcheck tool

- TOOL: added `toolkit/rustcheck` (user-provided fast Rust+shader checker). Tier-0 static (~instant, no toolchain)
  catches structural breaks incl. the recurring brace-drop bug; tier-1 = scoped cached cargo check; exit 2 =
  BLOCKED/env (not a code FAIL). Now run `rustcheck --fast <file>` after edits before the full cargo build.
- ATLANTIS HALLS now respect the sea-floor dip: each perimeter WING is seated on the terrain height
  (seafloor_h at the wing centre, clamped to y_floor-7) instead of floating at the flat interior y_floor, and a
  STAIR FLIGHT steps DOWN from the door to it. New `hall_steps()` builds tread+riser panels along the radial ray
  (z scaled by sz) with a threshold SILL at the top and a LANDING slab at the foot (the architectural joints).
  Verified via the label dump: treads/risers descend y -3.6 -> -6.9 stepping outward r12->14, landing + wing disc
  at -6.9 with columns rising from there. Build clean; 258 tests. (Visual best seen orbiting out a door on real HW;
  interior auto-framing hides the wings.)
- TODO: the user noted OTHER MAPS may need terrain-conforming halls too — Forest/Mistfall have height fields;
  generalize hall_steps + a per-scene terrain sampler later. Also: expose fill-light strength; scene-bounds shadow.

QUEUED REMAINING: per-scene LIGHT fine-tuning (shadows add contrast now); ATLANTIS OVERHAUL (coral/prismarine/
pearls shelving, fish/plants, ceiling joiner, double+currents, door gap); EXPAND-WITH-GRAPH (maps grow w/ nodes).

## 2026-06-20 (pass 40) — smooth camera pan along the GRAPH SHORTEST PATH (done)

- Clicking a node no longer JUMPS the orbit target — it now GLIDES along the conversation graph's own shortest
  path (parent links via LCA: from -> ... -> LCA -> ... -> to), Catmull-Rom through the node positions, smoothstep
  eased over ~0.3s + 0.1s/hop. Mirrors the verified LOGSCOPE_FLY fly-through spline.
  * chat.rs: `graph_path(g, from, to)` LCA helper; Chat gains cam_pan_path + cam_pan_t0; per-frame glide block
    after the drag/zoom handling sets orbit.target along the path; click handler starts the pan (was a jump).
  * Interactive (click-driven) → verify the glide feel on real HW. Build clean; scene+spatial tests pass (16+10).
- TEST NOTE: the lone failing test is the documented ENV FLAKE sandbox::offline_first_reports_deps_unavailable_
  without_network (cargo/rustc target probe fails in the sandbox) — unrelated to scene/camera code; 257 others pass.

QUEUED REMAINING (user): terrain-conforming Atlantis HALLS with STEPDOWNS (stairs + architectural joints; other
maps too). Then per-scene light fine-tuning, Atlantis overhaul, expand-with-graph.

## 2026-06-20 (pass 39) — on-screen TOP-10-BY-DISTANCE debug labeler (done)

- LOGSCOPE_DEBUG_LABELS=1 now ALSO draws an in-view overlay: projects every static-scene mesh centroid through
  view_cam.project, sorts by view depth, takes the NEAREST 10, and draws a red marker + "#<idx> <maxdim>u" label.
  Cross-references the /tmp/scene_labels_<theme>.txt dump (same idx). chat.rs, right after the rune block; rebuilds
  the env once/frame via with_env (debug-only cost). Verified on Slate (dome/wall = 80u, stones = 2u, terrace 24u).
  258 tests.

QUEUED REMAINING (user): terrain-conforming Atlantis HALLS with STEPDOWNS (stairs + architectural joints; other
maps too); SMOOTH CAMERA pan between nodes along the graph SHORTEST PATH. Then per-scene light fine-tuning,
Atlantis overhaul, expand-with-graph.

## 2026-06-20 (pass 38) — camera-centric shadow box + colored fill light

- SHADOW BOX is now CAMERA-CENTRIC and DOUBLED: light_view_proj centered on view_cam.target (follows the look
  point) with radius 64 -> 128. Shadows track wherever you look + cover a much wider area (chat.rs).
- COLORED LIGHTING: confirmed the base was already colored (per-theme sun via theme.light + per-scene colored
  hemispherical ambient via scene_light(): Atlantis aqua, Forest woodland green, Cloudscape moonlit blue, etc.,
  time-of-day graded). ADDED a COLORED SECONDARY FILL light to make it pronounced: a soft directional fill from
  the off-sun side, tinted by the scene's sky-ambient hue (fill_col = c3(lsky)*0.55), fill_dir ~opposite the sun.
  Now shadowed faces pick up the scene hue instead of going flat-grey — pairs with the new shadow pass.
  * gpuscene FS: u_fill_dir + u_fill_col + `col += albedo * max(dot(N,fill_dir),0) * fill_col`.
  * glow_scene: Uniforms.fill_dir/fill_col + uploaded in paint(). chat.rs sets them per scene.
  * Verified: Forest reads as green canopy light, Atlantis as aqua underwater fill; columns/trees cast shadows
    across the wider camera-centric box. 258 tests.
- TUNE KNOBS available: LOGSCOPE_SHADOWS (0 off), LOGSCOPE_SHADOW_SIZE (default 2048). Fill strength 0.55 hard-
  coded for now (could expose). Doubling the box halved shadow texel density — raise SHADOW_SIZE if crisper needed.

## 2026-06-20 (pass 37) — SHADOW MAPPING (directional) + queued follow-ups

- Implemented a real DIRECTIONAL SHADOW MAP pass (the queued shadows/lighting work):
  * gpuscene.rs: `light_view_proj(sun_dir, center, radius)` (look_at+ortho, column-major) + SHADOW_VS/SHADOW_FS
    (depth-only; bright emitters a_emissive>0.8 are pushed out of the frustum so skylights/lanterns/pearls/gold
    don't cast). FS: u_light_viewproj + u_shadow + u_shadow_on; 3x3 PCF compare, softened (shadowed direct light
    floors at ~0.28x, ambient/bounce preserved). Applied to the diffuse + specular direct terms.
  * glow_scene.rs: Targets gains a fixed-size depth-only shadow FBO (LOGSCOPE_SHADOW_SIZE, default 2048). paint()
    uploads the VBO once, runs a depth-only shadow pre-pass (POLYGON_OFFSET 2/4 for acne) before the scene pass,
    binds the shadow map on unit 2. Uniforms gain light_viewproj + shadow_on.
  * chat.rs: light_view_proj([sun], center (0,6,0), radius 64); shadow_on default 1, LOGSCOPE_SHADOWS=0 disables.
  * Verified on llvmpipe (geometric → renders headless): Reading Room columns + Atlantis colonnade/statue cast
    directional shadows on the floor; scene mean drops ~18% from real shading. 258 tests.
  * NOTE shadow box is centered (0,6,0) r64 — covers interiors; very large maps (doubled Treehouse, wide
    Cloudscape) get shadows only in the central radius for now. Tunable; could derive from scene bounds later.

QUEUED (user, pass 37):
- ON-SCREEN DEBUG LABELER: top-10 meshes BY DISTANCE only (project centroids + draw ids; the file-dump labeler
  from pass 35 exists, this is the in-view version, capped at nearest 10).
- ATLANTIS HALLS don't respect terrain dip — need STEPDOWNS (stairs + architectural joints) where the sea floor
  changes height; other maps may need the same.
- CAMERA should pan SMOOTHLY between nodes along the GRAPH's own SHORTEST PATH.

STILL TODO after those: per-scene LIGHTING tuning now that shadows add contrast; Atlantis overhaul (coral/
prismarine/pearls, fish/plants, ceiling joiner, double+currents, door gap); expand-with-graph.

## 2026-06-20 (pass 36) — the REAL "three skinny room-sized rings": gravitational-ripple node rings (removed)

- User uploaded 3 images: the rings are super-skinny, room-sized-diameter concentrics passing through the floor in
  EVERY scene. Root cause found: chat.rs node overlay "gravitational air ripple" — for hot/token-heavy nodes it
  drew `for r in 0..3` concentric `circle_stroke` rings expanding to radius*(3..7) (reach = radius*7), 1px stroke,
  as a 2D overlay (over the floor), in all scenes. For a near node these became huge thin rings = exactly the 3 the
  user circled (image 1 = the 3 arcs centred on a node below frame; image 2 = them around the pearls).
- REMOVED the whole ripple block (3 rings + 3 orbiting motes). Node significance still reads via size/glow. acc_a
  closure kept as _acc_a (silenced). 258 tests. This is a SHARED overlay fix → gone in every scene.
- The enclosure oculus torus (pass 35) was a real separate ring too — stays removed.
- OPEN: a dark matte CURVED TUBE still sweeps across the Atlantis floor (visible in renders + user image 3). Likely
  the cord-REFLECTION strand on the floor plane (chat.rs ~1695, Atlantis emissive 0.32) tracing the node path.
  NOT removed yet — confirm with user whether that floor reflection should go too.

STILL TODO: Atlantis overhaul (coral/prismarine/pearls, fish/plants, ceiling joiner, double+currents, door gap)
→ expand-with-graph → lighting & shadows.

## 2026-06-20 (pass 35) — fix the RIGHT ring (enclosure oculus) + DEBUG SCENE LABELER

- The "wide thin concentric ring that passes through the floor" (multiple enclosed scenes) = the generic
  `enclose()` OCULUS rim TORUS (wide ring at the dome apex). REMOVED it (the bright skylight disc stays as the
  bloom/god-ray source). User confirmed via the Slate render. Floor mosaic (Atlantis) stays restored; wall BANDS
  restored (they were a wrong guess, not the culprit).
- NEW DEBUG LABELER: `LOGSCOPE_DEBUG_LABELS=1` dumps a per-mesh inventory (idx, centroid, bbox w/h/d, verts, mat,
  emissive, shape guess incl. "WIDE-FLAT ring/disc") to /tmp/scene_labels_<theme>.txt + stderr summary. Lets any
  element be identified by eye without guessing. `debug_dump_labels()` in scene3d.rs, called at build() end.
  TODO(optional): on-screen/click version (project centroids + draw ids in the egui overlay).
- NOTE: Atlantis has its OWN dome oculus rim + a glowing cyan HALO (overhead, intentional dome design) — analogous
  rings but NOT removed (user pointed at the enclosure oculus). Offer to remove/tone if they want consistency.

STILL TODO: Atlantis overhaul (coral/prismarine/pearls shelving, fish/plants, ceiling joiner, double+currents,
door-side distortion gap) → expand-with-graph → lighting & shadows.

## 2026-06-20 (pass 34) — Atlantis: removed the giant floor rings (start of the Atlantis overhaul)

- Removed the concentric FLOOR MOSAIC rings (7 annuli + med_ring + tesserae) that read as giant rings cutting the
  interior floor; kept a small central medallion. Floor is now a clean caustic-lit surface. 258 tests (the
  sandbox offline_first test flaked once then passed — known env flake, not scene code).

ATLANTIS OVERHAUL — remaining parts (own pass(es), big additive job):
- CEILING JOINER: fill the gap above the bookshelves with a curved cove (between shelf-top and the architrave/
  balcony at y_cap). Rotunda layout: columns y_floor→y_cap(6.2-8.6) → architrave torus → balcony y_bal → clerestory
  drum y_cler0..y_cler1 (windows/stained glass). Bookshelves sit on the lower wall between columns.
- DOOR-SIDE OCEAN-DISTORTION GAP: find the hardcoded demo hallway/opening where the ocean shows undistorted; the
  hallway() fn (~1974) + region_door (~1797) build perimeter openings — check which side lacks the WATER backdrop.
- CORAL-ROCK shelving replacing WOOD bookcases + PRISMARINE tablets + PEARLS in GOLDEN HOLDERS.
- more FISH + PLANTS; DOUBLE map + water DIRT patches + oceanic CURRENTS (drift varies by region).

LAST scene item: ★ ALL expand base dims as the graph grows (growth() from EnvParams).
THEN: lighting + shadows.

## 2026-06-20 (pass 33) — Old Library: empty shelves where the graph isn't filled + parallel aisles

- Refactored `bookcase` → thin wrapper over `bookcase_fill(.., fill: f32)` (fill 0..1 = packed..bare); existing
  callers unchanged (wrapper passes 1.0). Per-spine presence gated by `rnd(..) <= fill`.
- Old Library aisles extended to 12 cases each: first `filled` cases packed, the rest EMPTY (bare wooden cases).
  `filled = (pr.nodes/6).clamp(3,9)` → derived from the graph, so the stacks fill as the conversation grows.
- Added PARALLEL flanking aisles per hall (offset ±(2cw+3.2)), entirely EMPTY — the library has room the graph
  hasn't reached. Floor widened r20→34. 258 tests; verified filled→empty transition on llvmpipe.
- NOTE (recurring): when a str_replace's old_str ends at `...}\n s\n}`, KEEP the loop's last stmt + `}` in new_str.

REMAINING scene updates before lighting/shadows:
- ATLANTIS: door-side ocean-distortion GAP (hardcoded demo hallway?) + overhaul (coral/prismarine/pearls, fish/
  plants, double+currents, ceiling joiner above shelves, remove giant floor rings).
- ★ ALL: expand base dims as the graph grows (growth() from EnvParams; Old Library already ties fill to nodes).
THEN: lighting + shadows.

## 2026-06-20 (pass 32) — Mansion Garden: tropical overgrowth + vertical growth

- 30 broad-leaf TROPICAL PLANTS fanning from the floor; VINES spiralling up ~half the colonnade columns (vertical
  growth) with leaves; 10 SUB-PILLARS (room_column, shorter) each with a planter bowl + tropical tuft on top;
  6 HALF-GLASS-DOME cloches (dome_cap, translucent) over plant clusters with a rim ring; 6 elongated WATER CHANNELS
  (scale_xz'd pools) linking the round pools into varied pathways. Reads lush + less barren + dimmer. 258 tests.
  (twice now the loop's last s.add + closing brace got dropped by a str_replace whose old_str ended at `}\n s\n}`
  — RESTORE those lines in new_str. Also: usize loop index needs `as i32` for hash().)

REMAINING scene updates before lighting/shadows:
- OLD LIBRARY: DONE pass 33 (graph-filled aisles fade to empty + parallel empty aisles).
- ATLANTIS: door-side ocean-distortion GAP (hardcoded demo hallway?) + overhaul (coral/prismarine/pearls, fish/
  plants, double+currents, ceiling joiner, remove floor rings).
- ★ ALL: expand base dims as the graph grows (growth() helper from EnvParams).
THEN: lighting + shadows.

## 2026-06-20 (pass 31) — Cloudscape (more/far clouds, merge-diverge, 2x map), Treehouse (2x map), Forest (grass/terrain)

DONE (committed, 258 tests; rendered each in its OWN fresh Xvfb — heavy scenes black out the shared-Xvfb loop):
- CLOUDSCAPE: deck 15→30 (double diameter); clouds 26→42 with per-PARTICLE drift (morph/deform, not rigid blobs)
  + wide alpha 55-225 (DITHERED density) + varied per-cloud drift (groups MERGE & DIVERGE); NEW far-cloud band (40
  dimmer masses rad 34-64); wisps rad 3+16→6+32.
- TREEHOUSE: doubled map — floor 24→48, plats/trees rad 7+18→14+36, fireflies 22→44, distant trees 28+18→56+36.
- FOREST: rolling TERRAIN (closure `terrain(x,z)` = layered sines, ±~0.56) displacing the ground verts; GRASS TUFTS
  (220, thin-tall scaled leaf_cluster, season-tinted, terrain-seated); BUSH TUFTS (34, low mounds, terrain-seated).
  NOTE: terrain kept gentle so trees still read seated; grass/bush/terrain share the closure (trees stay at yf).

REMAINING scene updates before lighting/shadows:
- MANSION: tropical OVERGROWTH + VERTICAL growth (vines, sub-columns/pillars, scattered half-glass-domes) + varied pools.
- OLD LIBRARY: DONE pass 33.
- ATLANTIS: door-side ocean-distortion GAP (hardcoded demo hallway?) + big overhaul (coral/prismarine/pearls,
  fish/plants, double+currents, ceiling joiner, remove floor rings).
- ★ ALL: expand base dims as the graph grows (growth() from EnvParams nodes/extent → scale each scene's radii).
THEN: lighting + shadows pass.

## 2026-06-20 (pass 30) — batch: Canopy courtyard+lean, Mistfall water/moss/mist, Mansion ring+bloom

DONE (committed, 258 tests):
- CANOPY: inner courtyard widened ~2x (rings 8.5/12.4/16.3 → 16/20.5/25, outer 19→28, floor 20→30, stones spread
  to 26); trunk LEAN ~24° off axis (tdir 0.45,1,0.12) with canopy cc riding the lean.  [Canopy-only.]
- MISTFALL: water dropped -4.4→-5.2 (+deep -6.6, boulders -6.4, waterfalls reland -5.1) so everything floats above;
  MOSS on standing stones rebuilt as flattened uv-sphere MOUNDS hugging the stone (was leaf_cluster reading as
  stray leaves / detached); waterfall MIST 8→18 puffs, taller/denser.
- MANSION GARDEN: removed the central MOAT RING (the "big ring") → scattered pools only; bloom 0.4→0.26 +
  scene_light lexp 0.80→0.70 + marble darker again (still-too-bright complaint).

QUEUED (this batch's remainder + prior):
- MANSION: DONE pass 32 (overgrowth/vines/sub-pillars/half-domes/channels). // was: too BARREN → tropical OVERGROWTH + VERTICAL growth (vines, sub-columning/pillars, half-glass-domes
  scattered); + more varied pool pathways. (bigger add — next Mansion pass.)
- CLOUDSCAPE: cloud MERGE/DIVERGE; more clouds; DOUBLE map diameter; more far clouds; (+earlier: dither + morph/mist).
- FOREST: GRASS + BUSH TUFTS on ground; TERRAIN HEIGHT VARIATION (large-scale perlin / displaced ground).
- TREEHOUSE: DOUBLE map size (scale all XZ radii: floor 24, plats rad 7+18, fireflies 22, distant 28+18 ... ×2).
- OLD LIBRARY: parallel hallways with EMPTY shelves where the graph isn't filled yet (ties to graph-growth).
- ATLANTIS: the ocean water-distortion GAP on the door side — likely a hardcoded demo hallway; investigate+fix.
  (+ earlier big Atlantis overhaul: coral/prismarine/pearls, fish/plants, double+currents, ceiling joiner, floor rings.)
- ★ ALL: maps EXPAND from these base values AS THE GRAPH GROWS — systemic. Plan: derive a growth factor from
  EnvParams (nodes / ext_x / ext_z) and scale each scene's base radii/extent by it (clamped), so a bigger
  conversation = a bigger room. Centralise as a `growth()` helper; thread into every scene's base dims.

## 2026-06-20 (pass 29) — Canopy: shorter + wider centre trunk (Canopy-only); shared/per-scene ledger added

- CANOPY centre tree: tree_branch call args len 7.4→6.0, rad 0.7→1.3 (shorter, thicker/wider "branching trunk");
  canopy cc lowered yf+8.0→yf+6.6 to sit on the shorter trunk. ONLY the Canopy call changed → Forest/Treehouse trees
  untouched (tree_branch internals unchanged). 258 lib tests pass.
- Added the SHARED vs PER-SCENE LEDGER at the top of this file (per user request) to track what is shared vs the
  explicit per-scene overrides, so shared edits get a guard/param instead of leaking across scenes.
- NEXT FOCUS (user: "lighting and shadows soon"): the queued GENERIC SHADOW PASS + a lighting pass. Will likely be
  the next 1-2 passes (depth shadow-map from key light, PCF in SHADER_FS; then per-scene light tuning).

## 2026-06-20 (pass 28) — Old Library: rectangular wooden bookshelf hallways

- Floor extended r12→20. Four HALLWAYS radiate from the lectern (±X, ±Z); each is two facing rows of 6 `bookcase()`
  wooden stacks (full of coloured book spines) flanking a corridor, plus an end-cap stack across the far end →
  reads as rectangular library aisles in the wooden style. 258 lib tests pass; VERIFIED on llvmpipe.
  (fixed a self-inflicted brace bug mid-edit: the loose-pages s.add + loop close had been dropped.)

## 2026-06-20 (pass 27) — Forest: seasons + fallen logs + skunk cabbage + rocks

- SEASON system: LOGSCOPE_SEASON (spring|summer|autumn|winter) or seed-derived (pr.seed%4). Drives crown colour
  via per-season `crown_pal` (spring fresh+blossom / summer deep green / autumn amber-gold-red mix / winter
  brown+snow) applied to all 3 crown loops (hero, tall-thin, distant-LOD) with a `shade()` hi/lo derive; litter
  palette also season-aware. WINTER BARRENNESS via keep_p=0.28 (drops ~72% of leaf_cluster puffs).
  CAVEAT: tree_branch bakes its OWN small green leaves (shared with Treehouse/Canopy), so winter is only PARTIALLY
  barren — fully bare winter needs tree_branch to take a foliage-density arg (defer; pass 1.0 elsewhere).
- FALLEN LOGS (8): mossy WOOD tubes on the floor + a mossy top patch + contact shadow.
- SKUNK CABBAGE (occasional, ~5/10): clumps of broad upright blades (scaled leaf_cluster) + a purple spathe.
- ROCKS (14): weathered STONE rocks scattered, flattened, contact-shadowed.
- 258 lib tests pass. VERIFIED autumn/winter/summer on llvmpipe.

## 2026-06-20 (pass 26) — BIG multi-scene batch from user. DONE so far + full backlog below.

### DONE this pass (committed, 258 tests):
- Reading Room: bloom 0.7→0.5 + scene_light lexp 0.92→0.82 (+ dimmer light/ambient) — whites no longer blow out.
- Mansion Garden: removed the central dais RIM RINGS; toned marble (mlo/mhi darker) + bloom 0.4 + lexp 0.96→0.80
  (was too bright/bloomy); GOLDEN RAIN now OCCASIONAL (VS gates drops to ~38% of a 55s cycle, parked otherwise);
  MEMORY-MARBLE stains made sparser/briefer so the floor is mostly white and returns to white.  [fixed a GLSL
  reserved-word bug: `active` → `stainable`.]
- Mistfall: water level lowered -3.6→-4.4 + bounded (r36→30) so stones/structure FLOAT above it; waterfalls reland.

### REMAINING BACKLOG from this batch (per scene, queued for following passes):
- ARCHIVE: open-ended "anything to add?" — propose+add (e.g. card-catalog drawers, hanging index tags, reading
  lecterns, a ladder on a rail, dust motes in light shafts).
- SLATE: improve the WATER first, then add little BONSAI on islands in the water. (bonsai gated by better water.)
- OLD LIBRARY: DONE pass 28 (4 radiating wooden bookshelf hallways).
- CANOPY: DONE pass 29 (short+wide trunk, Canopy-only). // was: shorten + WIDEN the centre tree's branching TRUNK — Canopy-specific (do NOT change shared tree_branch;
  Treehouse/Forest trees stay as-is). Likely a `trunk_scale`/dedicated build for Canopy only.
- MISTFALL: stalactite ends too small + look like CG (aliasing/texturing) → thicker/rounded tips, better shading,
  maybe more facets / a tip taper that reads. (water done.)
- ATLANTIS (big — split across passes):
  * fill the ceiling gap above the bookshelves with a CURVED JOINER.
  * remove the giant RINGS cutting through the interior floor (investigate: mosaic annuli? a stray torus?).
  * add more FISH + PLANTS.
  * DOUBLE the map size + add water DIRT patches + oceanic CURRENTS (drift dir varies by region).
  * replace WOOD bookshelves with CORAL-ROCK shelving + PRISMARINE tablets + PEARLS in GOLDEN HOLDERS.
- CLOUDSCAPE: DITHER the cloud texture + more morphing/deformation + misting.
- FOREST: DONE pass 27 (logs/seasons/skunk/rocks). Follow-up: full winter barrenness (tree_branch foliage-density arg).
- TREEHOUSE: collision boxes on all objects + repulsion/warp vectors so near objects push apart + bend (WORKSHOP
  with user — start simple: per-object AABB + a soft separation offset).
- MANSION GARDEN: add VINES + more tropical vegetation + more varied POOL PATHWAYS (replace the plain moat ring);
  expand map 100x + add camera-distance LODs. (rings/rain/brightness done.)

## 2026-06-20 (pass 25) — Mansion Garden: bigger, better palm crowns + GOLDEN RAIN + MEMORY MARBLE; shadow pass queued

- SIZE: added scale const `sc` (=1.6) through the whole scene → grander conservatory. (User said "16X"; a literal
  16x uniform scale pushes the dome/columns outside the camera's framing of the centred graph → a vast empty plain,
  so used a grander factor that keeps architecture in frame. sc is one constant → trivially pushable bigger.)
- PALM CROWNS: were 5 size-1.2 fronds → now 9 smaller (0.7) clusters in two tight tiers → fuller, more "connected"
  canopy. Foliage dappling already comes from the MOSS forward-scatter/backlit shader. WIND made GENTLE in this
  scene (frond Float amp 0.05→0.03 drift 0.08→0.04; ferns amp 0.025 drift 0.035).
- GOLDEN RAIN: 150 thin gold streaks (emissive 7.0 = raindrop flag). Vertex shader now takes u_time and LOOPS them
  downward: wp.y = a_pos.y - mod(u_time*6 + cellPhase*H, H) (per ~2-unit cell so a streak's verts fall together).
  FS: emissive>6.5 → bright gold streak.
- MEMORY MARBLE: floor + dais steps flagged emissive 0.22. FS branch (mat STONE + emissive≈0.22): per-cell golden
  stain that blooms on impact then fades over a long cycle back to the marble/white — exp(-ph*3.2)*smoothstep, cells
  on independent schedules (rnd phase). "golden raindrops re-texture the marble, material memory reforms, returns to
  white." (True coupling drop->exact impact + real cast shadows = shadow pass, queued.)
- Gold-vein pulse branch narrowed to emissive 4.5..6.0 so it no longer catches the 7.0 raindrops.
- 258 lib tests pass. VERIFIED on llvmpipe: rain falls, crowns fuller, veins/patches read (marble+rain over-bright on
  software GL → cream/gold on real GPU; memory bloom/fade + pulse travel only visible animated → confirm on GPU).

## 2026-06-20 (pass 24) — NEW SCENE: Garden Marble Planet ("Mansion Garden") v1 + gold-vein light pulses

### ★ QUEUED (prioritised): GENERIC SHADOW PASS
Many scenes now want real cast shadows (Mansion Garden gold-vein pulses + golden rain 'interesting shadows',
Treehouse/Forest creatures, Canopy tree, Atlantis colonnade, Mistfall stones). Plan: a depth-only shadow-map
pass from the key light (scene_light direction) into a depth texture, sampled in SHADER_FS with a PCF tap to
darken lit fragments. Scope: add a 2nd FBO + light-space matrix uniform; render scene depth; sample + bias in
the main pass. Parked items it unblocks: Mansion Garden moving shadows that follow the gold pulse, contact
realism everywhere. Verify on real GPU (llvmpipe depth ok but bias/PCF tuning needs the GPU).


Built the user-proposed scene. New theme "Mansion Garden" registered: theme.rs MANSION_GARDEN palette (light,
cream/gold/turquoise) + added to PALETTES; scene3d build() dispatch + added to the self-enclosing exclusion list
(builds its own dome, no generic shell); chat.rs scene_light entry (sunlit cream marble, warm bounce).
`mansion_garden()`: cream-marble floor + deep-water layer; 3-step central DAIS (low, graph floats above); turquoise
MOAT ring + 7 scattered POOLS (WATER, translucent); 16-column marble COLONNADE (one entrance bay open) + architrave;
ribbed GLASS DOME (dome_cap, translucent, 16 ribs); GOLD VEINS — 16 radial + climbing-column emissive gold tubes at
emissive 5.5; 18 PALMS (leaning trunk + leaf_cluster fronds) + 24 ground ferns around the edges (centre kept clear).
Gold-vein PULSE in gpuscene.rs fragment: `if (v_emissive > 4.5)` → a travelling band sweeps a diagonal axis
(pow(max(sin(along*0.7 - time*2.2),0),6)) → dim gold base + bright moving pulse → "light pulses in lines at night".
258 lib tests pass; VERIFIED geometry/hue on llvmpipe (marble over-bright on software GL — defer absolute
brightness to real GPU; pulse motion only visible animated → verify on GPU).

REMAINING for this scene: the "interesting MOVING SHADOWS" the pulses should cast need the parked shadow-map pass
(the forward renderer can't cast geometry shadows from the emissive pulse) — a swept fake/contact-shadow that
follows the pulse is the cheap option. Tuning: vein base brightness / pulse width / colour once seen on GPU.

STILL OPEN ELSEWHERE: Canopy connector HEAT DISTORTION (screen-space refraction shader pass).

## 2026-06-20 (pass 23) — defer heat-distortion (TODO) + new GARDEN MARBLE PLANET concept + bundle ref docs

### ▶ DEFERRED / FORMAL TODO: Canopy connector HEAT DISTORTION
The "vibrational field of air that holds the leaves" currently ships as a geometry approximation (thin, faint,
fast-vibrating translucent PAPER air-filament cords, pass 20). The TRUE effect — screen-space heat-haze — is
still TODO. Plan: a composite-shader pass in gpuscene.rs/glow_scene.rs that, for fragments near a Canopy cord,
samples the scene colour with a small animated UV offset (sin(time + worldY*k) * strength) masked to the cord
region (render cord mask to a buffer or reconstruct from cord screen positions). Must verify on a real GPU —
llvmpipe will not show refraction faithfully. Keep strength low so it shimmers, not smears.

### ▶ NEW SCENE CONCEPT (user-proposed, ref: epic_garden_planet_v1 mockup): "GARDEN MARBLE PLANET"
A marble conservatory/garden world — domed glass rotunda, cream-marble columns + tiered platforms, lush palms
and ferns, and LOTS of turquoise WATER POOLS / channels winding through (the mockup's MANSION GARDEN look).
Signature mechanic the user wants: the marble's GOLD STREAKS (veining) EMIT LIGHT PULSES that travel in LINES
at night, lighting the scene and throwing interesting moving SHADOWS.
Build notes when we do it:
- Reuse Atlantis bones (dome_cap glass dome, colonnade, mosaic/marble floor, water annuli/caustics) as a start.
- Gold veins: emissive line-strips baked into the marble (curves across columns/floor); animate a bright PULSE
  travelling along each vein (param along the strip vs time → a moving emissive band). At night (low tod) these
  become the key light. "interesting shadows" implies the pulses should drive a light direction → would want a
  cheap projected/au fake shadow that sweeps as the pulse moves (or normal-mapped relief that the moving emissive
  grazes). Real moving shadows = shadow-map pass (parked) OR a faked contact-shadow that follows the pulse.
- Water pools everywhere: turquoise WATER discs/channels with caustics + the deeper-water trick from Mistfall;
  reflective marble edges.
- Palms/ferns: leaf_cluster foliage (small, per the leaf rules); keep the centre clear for the graph.

### ▶ Reference docs bundled into the output zip (pass 23)
Added a top-level `reference-docs/` to logscope-suite-tree.zip: HANDOFF-imagegen.md, HANDOFF.md,
dragon_forge_3d.html, character_forge_3d.html, turn9-vit-block.patch, and the epic_garden_planet_v1 mockup PNG.
(The 48MB logscope-suite.zip + 121MB rustup.zip originals are intentionally NOT bundled — toolchain/bundle, not docs.)


## 2026-06-20 (pass 22) — Canopy: kill white-bloom leaves, widen/shorten tree, deeper/taller shelves, stepping stones

User: the central tree had TWO leaf groups — the high tree_branch canopy looked fine (green), but a second group
(my restored calcite leaf_clusters) was emissive and blew out FULLY WHITE/bloomy at night (user @ 9:30pm),
clumped at the lower trunk divergence. Fixes:
- Canopy leaves → natural GREEN non-emissive MOSS foliage (was WATER + emissive 0.7), spread WIDER (r2.2→3.4),
  raised ABOVE the fork (+0.4, positive tiers), 16→22 clusters. No more white bloom; "widen the tree canopy" done.
- Tree 10% shorter (tree_branch 8.2→7.4); contact shadow r2→3.
- Courtyard widened + hallways ~10% wider: rings 7/10.5/14 → 8.5/12.4/16.3, outer 16.5→19.0, floor 17.5→20.0.
- Shelves 25% TALLER (h ×1.25; outer 9→11.25) and 2× DEEPER (petrified_wall thick 0.5→1.0). [note: user first said
  "shelves 10% shorter" then "actually raise shelf height 25%" → applied +25%.]
- STEPPING STONES: 40 flat scaled stone discs scattered through corridors + courtyard to walk on.
- 258 lib tests pass.

REMAINING: true screen-space HEAT DISTORTION around the Canopy connectors (focused composite-shader pass).
## 2026-06-20 (pass 21) — Canopy: restore the nice LEAF-shaped foliage (revert the crystal-ball swap)

User: the central tree's leaves had become "big glowing white balls" — wanted the nice leaves from a few turns
back. Those balls were the `uv_sphere` calcite CRYSTAL clusters (pass-14/take-3). Restored take-2's leaf-shaped
foliage WITHOUT rebasing (which would have unwound the later curved-shelf/courtyard/air-cord/Mistfall/Atlantis/
Cloudscape work): pulled the exact block from commit 58aefc8 — `tree_branch` crown + 16 `leaf_cluster(p,1.5,..)`
calcite-tinted leaves (WATER "glassy calcite coat", emissive 0.7). Curved shelves / wider courtyard / air-filament
connector all preserved. 258 lib tests pass.

REMAINING (one focused shader pass): true screen-space HEAT DISTORTION around the Canopy connectors.
## 2026-06-20 (pass 20) — Canopy rework: wider courtyard + CURVED shelves + air-filament connector

- WIDER COURTYARD: maze rings pushed out 5.5/8.5/11.5 → 7/10.5/14, outer ring 13.5 → 16.5, floor 14 → 17.5.
  Central tree + graph now have room (ties into "central objects make space for graphs").
- CURVED SHELVES: `petrified_wall` gained a `deco` flag (decorations gated); new `petrified_arc()` builds each
  ring/outer segment as an ARC faceted into short tangent slabs hugging the circle (deco lands once, mid-arc).
  VERIFIED: shelves read as smooth concentric curves.
- AIR-FILAMENT CONNECTOR (geometry approximation of "vibrational field of air that holds the leaves"): Canopy
  cord → thin (r0.02), high-vibration (amp 0.5), fine (14 seg), faint glow (0.30), PAPER mat, pale air colour
  (0xd8e4e8), translucent (alpha 85). Reads as a faint vibrating air filament instead of a wood root/branch cord.
- (nodes → single large leaves already done via leaf_single in pass 16.)
- 258 lib tests pass.

REMAINING (one focused shader pass): true screen-space HEAT DISTORTION around the Canopy connectors — a composite-
shader refraction (sample the scene with an animated UV offset masked to the cord region). Deferred because it
touches gpuscene/composite + can't be trusted on llvmpipe (verify on real GPU). Optional polish: rising shimmer
motes along the filament.

## 2026-06-20 (pass 19) — Cloudscape: particleized clouds

- Each of the 26 cloud masses: the 4 solid uv_sphere puffs → a MASS of 24-36 small translucent droplets in a
  flattened ellipsoid (core-weighted radius via powf 0.6), varied size + alpha, all drifting on the shared Float.
  Reads as granular particle-clouds instead of smooth puffs; keeps the moonlit silver-blue. VERIFIED on llvmpipe.

ONLY REMAINING from the big batch: Canopy structural rework — widen the courtyard, CURVE the shelves into the
circle, and replace the root/branch node connector with a "vibrational field of air that holds the leaves +
heat distorts around it" (the heat-shimmer is the novel part — likely a screen-space/shader refraction effect,
its own focused pass). Plus: propagate "clear the centre for the graph" to Canopy/Forest.

## 2026-06-20 (pass 18) — Mistfall: stones/moss, turbulent waterfalls, deep water, stalactites+ceiling, cords

- STONES spread + randomized: ring of 9 even → 14 at random angles, radius sqrt-spread 5..16. MOSS added inside
  `standing_stone` (crown cap + seed-gated side clumps).
- WATERFALL rebuilt: 3 overlapping out-of-phase translucent strands (turbulent) + 14 tumbling foam BUBBLES +
  plunge foam ring + 8 rising MIST puffs.
- DEEPER cave water: dark deep bed disc at -5.4 + 7 submerged boulders; surface water made translucent
  (alpha 172) so the depth reads through.
- STALACTITES now read: added a flat dark cavern CEILING (disc at y13.5, normals flipped down) + new `stalactite()`
  cone primitive (n-gon top → sharp apex), 30 hung longer/pointier/darker from the ceiling. (was blobby uv_spheres
  that didn't read.)
- NODE-CONNECTOR laminar distortion increased: Mistfall cord_amp 0.20→0.34 + per-scene cord_seg (Mistfall 13 vs 7)
  threaded into m3::strand → finer, stronger ripples. ("occasional ripple pulses" = softer follow-up if wanted.)
- 257/258 lib tests pass (same intermittent sandbox-rustc env flake; all scene tests pass).

STILL PENDING: Canopy (widen courtyard; CURVED shelves; root/branch connector → vibrational air field + heat
distortion); Cloudscape (particleize clouds). Propagate "clear centre" to Canopy/Forest.

## 2026-06-20 (pass 17) — Treehouse creatures grounded + bridge sway; Atlantis banners + ceiling soffit

Treehouse:
- FLOATING cats/rabbits fixed: the bridge critters used a wrong sag (-0.35 vs the real droop sag*4*t*(1-t)≈1.3 at
  midspan) so they hovered ~1m up. Now grounded on the actual plank surface, facing along the span.
- `critter` split into a thin wrapper + `critter_m(.., motion: Float)`; bridge critters ride the SAME sway Float
  as bridge i (seed i*31) so they stay planted on the swinging span. Platform critters sit on the deck (feet=py).
- Swingers now hang off a real fixed point — gripping a platform RAIL (py+0.72, edge) and dangling below.
- Bridge sway boosted (amp 0.09→0.12, drift 0.35→0.6) for visible swing.

Atlantis:
- `banner()` — hanging heraldic banner that WAVES: 5 stacked cloth segments (PAPER mat), each phase-staggered +
  swaying more toward the free end (drift 0.05→0.21) so a ripple travels down; gold stripe alternation; on a
  crossbar. Hung in the bays between columns off the architrave (skips hallway mouths). VERIFIED: banners read.
- Cover outer-ceiling gaps: a down-facing stone SOFFIT annulus (r_hall-0.5 .. r_hall+3) at the dome base seals
  the open perimeter ring between the dome and the outer wall. (User to confirm it was the right gap.)
- 258 lib tests pass (the earlier sandbox-rustc failure was a transient; back to green).

STILL PENDING: Canopy (widen courtyard; CURVED shelves; root/branch connector → vibrational air field + heat
distortion); Mistfall (stones spread/moss, turbulent bubbly waterfalls + misting, deeper cave water, node-
connector laminar distortion + ripples; stalactites); Cloudscape (particleize clouds). Propagate "clear centre"
to Canopy/Forest.

## 2026-06-20 (pass 16) — general leaf/graph-space rules + Treehouse + Cloudscape moonlight

General rules:
- "leaf orbs only one leaf": new `leaf_single()` primitive (one cupped blade, uses colour directly); NodeStyle::Leaf
  now renders ONE leaf per turn (was leaf_cluster orb). (= Canopy "nodes → single large leaves".)
- "leaves on trees are small": tree_branch baked foliage len*1.6 → len*0.85 (global, all trees shrink).
- "central objects make space for graphs": applied to Treehouse (below); to propagate to Canopy/Forest centres.

Treehouse:
- removed the CENTRAL (0,0) hero tree → the graph runs through clear air.
- hero canopy puffs 1.9→1.0 & 8→6; surrounding puffs 1.6→0.9 (visibility up, less obscuring the nodes).
- MORE TREEHOUSES: extra `tree_platform`s perched on ~a third of the surrounding trees.
- creatures moved UP onto the walkways: a critter on each platform + some on the bridge spans; only 2 swingers
  still hang. VERIFIED on llvmpipe: small leaves, clear centre, readable strand.

Cloudscape:
- "night looked like daylight": clouds recoloured to MOONLIT silver-blue (deck 0xe2eaf2→0x9aa6c0, puffs
  0xf0f4fa→0xaebad2, wisps cooler), emissives trimmed; scene_light → night palette (sky 0x4e5e82, ground
  0x20283a, ambient 0.50, lexp 0.60). VERIFIED: reads as moonlit night now.

‼️ TEST NOTE: `sandbox::tests::offline_first_reports_deps_unavailable_without_network` now FAILS with "failed to
run rustc to learn about target-specific information" — an offline-toolchain/sandbox environment issue (the test
shells out to cargo/rustc), NOT related to any scene/render code changed this session. 257/258 lib tests pass.

STILL PENDING: Canopy (widen courtyard; CURVED shelves; root/branch connector → vibrational air field + heat
distortion; [nodes→single leaf DONE]); Mistfall (stones spread/moss, turbulent bubbly waterfalls + misting,
deeper cave water, node-connector laminar distortion + ripples; stalactites); Atlantis (waving banners; ceiling
gaps); Cloudscape (particleize clouds). Propagate "clear the centre" to Canopy/Forest.

## 2026-06-20 (pass 15) — in-app feedback batch (lighting/bloom + Forest/Treehouse/Cloudscape)

Done + committed (258 tests pass):
- NIGHT too bright (Reading Room @ 00:24): the tod exposure swing was tiny (0.82+0.20*tod → ~0.84 at night).
  Deepened to (0.55 + 0.48*tod_strength) → night ~0.61, day ~1.03. Also sun_clock ambient 0.85+0.35*day →
  0.56+0.62*day (dimmer night fill). Reading Room scene_light ambient 0.74→0.66, lexp 0.96→0.92. (Absolute
  night brightness must be confirmed on the user's GPU — llvmpipe over-brightens.)
- Per-scene BLOOM trim (bloom_amt * match name): Old Library 0.5, Cloudscape 0.42, Reading Room 0.7.
- Cloudscape too bright: scene_light lexp 0.94→0.72, ambient 0.82→0.74. VERIFIED on llvmpipe: cloud detail now
  survives (was a white blowout). Lightning "flicker in and out": strobe base 0.06→~0.02 + rapid sub-flicker
  (0.02 + 1.9*strike*flick) so the bolt drops to ~0 between strikes.
- Treehouse outskirt leaves too big: distant LOD puff 1.8+0.7 (max 2.5) → 0.9+0.5 (max 1.4).
- Forest DOUBLED: ground 54→100; tall-thin count 46→80 & rad ×2 (8+sqrt*56); LOD trees 40→64 & rad 56+42 (to
  ~98) with smaller distant crowns (1.4+0.6 → 0.8+0.5 = further LOD dropoff); litter 240→320 rad*72; bushes
  rad*48; grass 70→110 rad*66; flowers rad*52. VERIFIED: extent doubled, distant trees fade.

STILL PENDING from this batch (bigger/creative — next):
- Canopy: widen courtyard; CURVED shelves (lean into the circle); root/branch connector → a "vibrational field
  of air that holds the leaves + heat distorts around it"; NODES → single large leaves (chat.rs node layer).
- Mistfall: spread/randomize stones + grow moss on them; turbulent bubbly waterfalls + misting; deeper cave-floor
  water; increase laminar-flow node-connector distortion + occasional ripples. (carried: stalactites don't read.)
- Atlantis: waving banners; cover the main-room outer-ceiling gaps.
- Cloudscape: particleize the clouds.

## 2026-06-20 (pass 14) — Canopy: cozy STONE chamber + petrified deco + small shafts (workshop take 3)

User redirect: "smaller occasional shafts" + "deeper stronger walls of STONE with petrified DECORATIONS instead".
- `petrified_wall` reworked: DEEP STRONG STONE slab (slate, thick 0.5) + heavy stone ledges + PETRIFIED-WOOD
  DECORATIONS proud of the inner face (vertical agate inlay strip + agate cabochon medallion(s); `natural`
  segments get a 2nd larger one, seed-driven). VERIFIED: reads as a cozy tiered stone shelf-wall chamber (good).
- Big rose-window skylight REPLACED by `light_shaft()` x(3-4): small colored glowing column + bright aperture +
  glow pool; hue from glass palette by seed. (Couldn't frame/verify the shafts cleanly in the harness.)
- Central tree: built a BARE trunk (tree_branch bakes green MOSS foliage) + CALCITE crystal clusters (uv_sphere,
  pale, emissive — STONE mat; WATER green-shifted, leaf_cluster bakes green vcol).
- GOTCHA discovered: the green "leaves" along the strand are the CONVERSATION NODES — Canopy decorates each node
  with a green leaf in the chat.rs node layer (NOT the scene tree). To make ALL foliage calcite/petrified, that
  node decoration must change too (chat.rs, per-scene NodeStyle). Open question for user: just the central tree,
  or the per-turn node leaves too?
- 258 lib tests pass. AWAITING user read: stone walls direction? node-leaves → calcite? shaft look?

NOTE carried: Mistfall stalactites don't read (longer/pointier/darker off a ceiling).

## 2026-06-20 (pass 13) — Canopy: COZY PETRIFIED CHAMBER (workshop take 2) — walls land, skylight WIP

User redirect: not the open ruin — wants COZY + TIGHT. "tall shelves + glass skylights with stained angels +
light play + central tree with calcite-coated petrified leaves whose emissives reveal through the dapples;
petrified wood smoothed + ground into the tall walls, occasional natural faces as envparams."
- `box_slab()` (6 flat panels) + `petrified_wall()`: TALL petrified-wood shelf-walls — smoothed = FLAT box slabs
  with protruding shelf ledges + agate vertical banding (WOOD mat, amber); ~1-in-8 segments are raw "natural
  faces" = rougher rounded `rock`, selected off env_params().seed. Tight maze (rings r5.5/8.5/11.5, few gaps) +
  tall outer wall ring r13.5 → cozy enclosed chamber. **VERIFIED: reads as tall flat petrified shelves (good).**
- Central tree taller; leaves are CALCITE clusters (pale, WATER mat, emissive 0.7–1.0) → glow.
- Stained-glass ANGEL skylight overhead: ring of coloured translucent emissive panels (every 4th = pale angel)
  + central pale rose; coloured dapple discs on the floor. **PROBLEM: skylight renders as a DARK disc from below
  — emissive translucent WATER panels aren't compositing as glowing glass; double-siding didn't fix. TODO: debug
  (try MOSS mat like the lanterns/fireflies which DO glow, or opaque emissive glass; verify dapples + calcite
  glow too — framing also fights this since Overview centres the graph, not the ceiling).**
- 258 lib tests pass. Awaiting user read on direction before more iteration.

NOTE carried: Mistfall stalactites don't read (make longer/pointier/darker off a visible ceiling).

## 2026-06-20 (pass 12) — Canopy: OVERGROWN ANCIENT RUIN (workshop take 1)

Chosen direction after backtracking the hedge maze. First take:
- `ruin_wall()` — maze rings/spokes are now rows of LOW broken weathered-stone masonry blocks (varying low
  heights, crumbled gaps, slight per-block lean), moss caps + trailing ivy. Low enough to see over → open
  sightlines, calm not trapping. Wider gaps (0.34).
- `toppled_column()` — fallen drum + broken standing stub, mossy; 5 scattered. + 7 taller `standing_stone`
  monoliths. Central tree grows up through the ruin (reclaimed by forest).
- soft warm SHRINE-LIGHTS (low glowing stones), 22 drifting fireflies, 160 fallen leaves, ferns/moss.
- Verified on llvmpipe: low mossy broken walls + columns + monoliths + warm motes read; calm overgrown feel.
- TUNABLE next (await user reaction): wall height/density, how ruined (more toppled/broken), light mood
  (warmer/dimmer), path layout (could go unicursal-labyrinth if they want zero dead-ends). 258 tests pass.

NOTE (Mistfall): user reports the STALACTITES don't read in the render (stones do) — make them longer/pointier
+ darker, or hang from a visible rough ceiling, next time in Mistfall.

## 2026-06-20 (pass 11 — BACKTRACKED) — Canopy hedge maze reverted, concept being workshopped

User didn't like the hedge-maze concept → reverted canopy() to the original stone shelf-wall maze; removed the
`hedge_wall` helper. Peaceful-maze direction is now an open design question (workshopping). Ambiance bits tried
(lanterns, reflecting pool, fireflies, flowers, wider gaps) are easy to layer back onto whatever concept we pick.

## (superseded) 2026-06-20 (in-app feedback pass 11) — Canopy: peaceful HEDGE maze

"Make the minotaur maze more peaceful." Reworked the austere stone shelf-wall maze into a soft garden:
- **Hedge walls**: new `hedge_wall()` replaces the maze's `shelf_wall` arcs with overlapping leaf clusters
  stacked two high, gently rustling — green garden hedges instead of looming shelves.
- **Wider paths**: gap threshold 0.26→0.40 (rings) / 0.3→0.4 (spokes), so the corridors read as open and
  strollable, not trapping/claustrophobic.
- **Warm lanterns**: slim posts with a soft emissive lamp along the paths — pools of light make the maze feel
  safe rather than threatening.
- **Reflecting pool**: a calm still-water disc + stone rim in an off-centre nook.
- **Fireflies + butterflies + more flowers**: 30 drifting fireflies, 20 flowers (5 colours) → gentle, alive.
- Verified on llvmpipe WITH post: lush green hedge maze, lantern glows, pool, fireflies all read; the path-level
  view is leafy corridors. 258 lib tests pass.

** All eight themed scenes in the feedback batch have now been through a pass. ** Remaining work is the
cross-cutting / stateful follow-ups, not whole scenes:
- node-dots → twinkling fireflies (2D node-marker layer in chat.rs)
- stateful: Forest 30-min decaying leaves → ragdoll pileup; critter/swinger random-WANDER AI
- wind gusts + trickle rain (Treehouse); spawning/despawning lightning bolts (Cloudscape)
- deeper wood grain / surface deformation (Treehouse)
- bake user's preferred SAT/EXP/FOGM/SHOULDER once reported → defaults

---

## 2026-06-20 (in-app feedback pass 10) — Mistfall cavern + antigravity loops; lightning STROBE split

- **Lightning strobe vs firefly twinkle**: the scene FS emissive hook already twinkled MOSS+emissive>1.6 (so the
  fireflies twinkle and the Cloudscape bolts already pulsed). Split the behaviour: emissive>2.0 → sharp
  intermittent STROBE (`0.06 + 1.7·pow(max(sin(t·0.85+ph·0.4),0),16)`) so lightning flashes on/off like a real
  strike; 1.6–2.0 → the gentle firefly twinkle. This delivers "flashing lightning" + "twinkling fireflies" from
  the existing hook (no new vertex attribute). NOTE: only the node-dots→fireflies (2D layer) remains of that ask.
- **Mistfall** got the cavern treatment:
  - **Water all over the ground**: pool disc 26→36 + faint emissive 0.04 so the wet floor reads in shadow.
  - **Cave walls**: 30 tall rough rock buttresses ringing r28–34 (rock built at ORIGIN → scale3 y=h/(2·rock_r)
    → translate to place — scale3 is origin-pivot!), tinted by height; the Mistfall fog blots the far side so
    there's no hard wall. + 22 hanging STALACTITES around the upper ring (dark cavern roof implied).
  - **Antigravity looping waterfalls**: new `antigrav_fall(cx,cz,base_y,r,face_ang,seed,col)` — a vertical LOOP
    of water tube segments (thinning toward the bottom) + streaming droplets + a bright "recondense" knot at the
    crown. `nfalls = 4 + seed%3` placed far out (r16–26) in the mist → they fall away and loop back on
    themselves. Verified on llvmpipe: the big translucent loops + ringing cave walls + caustic water floor read.
- 258 lib tests pass.

DEFERRED: node-dots→fireflies (2D marker layer); spawning/despawning lightning (vs static strobing geometry);
the cave walls are rounded rock blobs (could be carved into more wall-like slabs later).

Last untouched scene: Canopy (peaceful minotaur maze). Cross-cutting deferred: stateful wander AI, 30-min
decaying leaves + ragdoll, wind/rain, deeper wood grain.

---

## 2026-06-20 (in-app feedback pass 9) — Cloudscape: wisps + content-tuned storm lightning + moon/stars

Cloudscape was just a deck + cloud puffs. Added the weather system:
- **Wispier fog**: +18 thin, stretched, translucent (`set_alpha 120`) cloud VEILS drifting on big slow drift.
- **Storm cells + content-tuned lightning**: new `lightning_arc(from,to,col,seed)` — a perpendicular-jittered
  jagged polyline of bright emissive tube segments (zero jitter at ends, max mid-span) + occasional forks. 3–4
  darker storm heads each throw 2–3 downward strikes. Bolt HUE is graph-derived (one storm palette per
  conversation, from a 5-colour palette) → different logs storm in different colours. Verified at low exposure:
  jagged bolts descend from the cells; emissive tuned 3.4→2.3 so colour reads instead of clipping white.
- **Night / moonlight**: pale cool moon disc + soft halo + 60 faint stars; moonlight TINT comes from the
  existing time-of-day grade (cool at night).
- 258 lib tests pass.

DEFERRED (Cloudscape): bolts that FLASH (timed strobe) — needs shader emissive animation; content-driven day↔night.

Remaining scenes: Canopy (peaceful maze); Mistfall (ground water + cave walls + antigravity looping waterfalls).
Cross-cutting deferred: shader emissive-animation hook (flashing lightning / twinkling fireflies / node-dots),
stateful systems (Forest 30-min decaying leaves + ragdoll; critter/swinger wander AI), wind/rain.

---

## 2026-06-20 (in-app feedback pass 8) — Treehouse: swinging creatures + LOD + denser fireflies

Audited first (handoff was right — much already existed): rope bridges already SWING, planks are already THICK
with procedural HOLES, procedural MOSS clusters and ambient FIREFLIES already present. So this pass added the
genuinely-missing pieces:
- **Swinging fuzzy creatures**: new `swinger(anchor, drop, seed)` — mossy round body + head + ears + dangling
  legs + a long curled tail + bioluminescent eyes, hanging by its arms from a branch and swaying like a pendulum
  (shared Float w/ drift). 6 hung from the canopy near the platforms. Debug: `LOGSCOPE_CRITTER_DEBUG=1` (now
  shows rabbit · cat · swinger).
- **LOD blot-out**: a far ring of +30 slim trees (trunk + one dim puff) at r28–46 dissolving into the fog, so the
  walk sits in a deep wood with no hard edge (matches the Forest treatment).
- **Denser fireflies**: 40→72, over a wider/taller volume — more of a living dusk swarm.
- Verified WITH post: colourful canopy (clamp fix holds), LOD ring + fireflies read. Creatures compile + placed
  but (like the Forest critters) too small to isolate cleanly in the headless harness → confirm look on GPU.
  258 lib tests pass.

DEFERRED (Treehouse): fireflies that TWINKLE replacing the white NODE dots at night (needs shader-side emissive
animation + a change to the 2D node-marker layer); wind GUSTS + trickle RAINSTORMS (animation/weather); deeper
wood GRAIN + surface deformation on trunks/planks. Critter/swinger random-WANDER + swing-to-swing is the stateful
follow-up shared with Forest.

Still open scenes: Canopy (peaceful maze), Mistfall (water/cave/antigravity waterfalls), Cloudscape (wispier fog
+ content-tuned coloured lightning + moonlit night).

---

## 2026-06-20 (in-app feedback pass 7) — Forest: density, tall thin trees, litter, bushes, LOD, critters

Grey confirmed fixed on the user's GPU ("I can see again"), back to per-scene work. Forest got the big expansion:
- **Way more tall thin trees**: +46 slender trees (height 8–14, trunk 0.18–0.34) across a larger radius, plus
  the 7 hero canopies. Trees use sqrt-radius scatter for even areal density.
- **Extent + LOD blot-out (no walling-in)**: ground 17→54; a far band of +40 simple slim "LOD" trees (trunk +
  one dim puff) at r36–52 that shrink and dissolve into the fog, so the wood has no hard edge. (Forest was
  already excluded from the `enclose` shell, so the abrupt edge was just the too-small grove.)
- **Ground leaf litter**: 240 flat, slightly-elongated leaf discs scattered (greens → autumn ambers/browns).
- **Bushes**: 18 low leafy mounds (3 clusters each) — the cover the critters sit by.
- **Critters**: new `critter(kind 0 rabbit / 1 cat)` — ellipsoid body + head + ears (long upright vs short
  pointed) + tail (puff vs curved) + idle/hop bob; 8 placed beside bushes. (Basic placement; random WANDER
  between bushes is a later stateful pass.) Debug: `LOGSCOPE_CRITTER_DEBUG=1`.
- Verified on llvmpipe WITH post: dense vivid forest (the clamp fix shows — lush greens, brown trunks, litter,
  bushes all read). Critters compile + placed but too small to isolate cleanly in the headless harness → confirm
  their close-up look on GPU. 258 lib tests pass.

DEFERRED (Forest): persistent fallen leaves with the 30-min timer → decay + ragdoll pileup (stateful + physics);
rabbit/cat random-wander AI between bushes (stateful path behaviour). Other scenes still open: Canopy maze,
Mistfall water/antigravity waterfalls, Cloudscape weather+lightning, Treehouse creatures/wind/rain/fireflies.

---

## 2026-06-20 (in-app feedback pass 6) — THE GREY WAS THE CLAMP (per-channel shoulder desaturated)

User: "those three knobs didn't fix it, I think it's the clamp." Correct. The highlight SHOULDER was rolling off
**per channel**: for a bright saturated colour it compresses the large channel more than the small ones, pulling
them together → washing every bright pixel toward white/grey. The pass-5 saturation ran BEFORE it, so the
shoulder un-did the vibrance on anything bright — which is why SAT/EXP/FOGM couldn't win.
- **Rewrote the shoulder to be LUMINANCE-based** (chroma-preserving): only luma rolls off, RGB is scaled to
  match, so hue/saturation survive into the highlights. Added `u_shoulder` (Uniforms.shoulder default 1.0) +
  `LOGSCOPE_SHOULDER` env (0 = off/linear, for A/B). NO_POST forces 0.
- **Verified on llvmpipe WITH post**: Atlantis at SAT 1.3 now renders vivid (cyan water, green algae, colour
  stained glass, blue pearls) instead of grey-white — both shoulder 0 and 1 keep full hue; shoulder 1 only tames
  the very brightest highlights. The grey is gone in-engine.
- 258 lib tests pass. (Ask user to confirm on their GPU; then bake final SAT/EXP/FOGM/SHOULDER defaults.)

---

## 2026-06-20 (in-app feedback pass 5) — the GREY (vibrance + live knobs) + Atlantis

The "all scenes super grey" persisted after pass 2's exposure/fog tweaks, so the real cause was traced: there
was **no saturation/vibrance stage anywhere** — once distance-fog mixes everything toward the (desaturated)
theme bg, nothing pulls colour back, so it reads flat grey regardless of brightness.
- **Added a vibrance stage** in the composite FS (`u_saturation`, applied around luma before the shoulder).
  `Uniforms.saturation` default 1.18; neutral (1.0) under NO_POST. **Verified on llvmpipe** (saturation is
  chroma, which llvmpipe shows reliably): SAT 0.4 = washed/grey, 1.6 = vivid → the control works.
- **Live tuning knobs** so the user can dial on their real GPU and report back: `LOGSCOPE_SAT` (default 1.18),
  `LOGSCOPE_EXP` (×exposure), `LOGSCOPE_FOGM` (×fog). NEXT: bake the user's preferred values as defaults.

Atlantis (geometry verified; brightness/pearl-sheen for GPU):
- **Flying pearls → smooth slowly-floating pearls.** The node pearls were `cracked_pearl` (a faceted gem that
  read as a foreign asset / wrong texture) → now smooth `uv_sphere` with the WATER/nacre sheen, plus a slow
  per-pearl drift offset (gentle current bob) instead of sitting rigid on the beams.
- **Ceiling gaps:** added the dome SPRING-LINE ring (i=0 — closes the gap where the dome meets the drum) and
  thickened the meridian ribs (0.09→0.13) + rings (0.07→0.09) so the cap reads as solid structure.
- **Rings cutting the middle:** removed the inner balcony RAIL torus + balusters (the mid-height ring at
  r_col-1.7 the code itself flagged as possibly caging the centre). NOTE: this is my best read of "rings cutting
  the middle"; if they meant the floor-mosaic concentric rings or the architrave, easy to switch.
- 258 lib tests pass.

STILL OPEN (big per-scene systems, deferred): Canopy (more peaceful minotaur maze); Mistfall (ground water +
cave walls + antigravity looping/recondensing far waterfalls); Cloudscape (wispier fog + content-tuned coloured
lightning storms + night/moonlight); Forest (more tall thin trees, ground + persistent decaying leaves w/
ragdoll pileup, rabbits/cats AI, extend size + heavy LOD blot-out); Treehouse (swinging fuzzy creatures, LOD,
night fireflies replacing white dots, wood grain + deformation, thicker holed planks, procedural moss, wind
gusts + trickle rain — NOTE some Treehouse items may already exist from the earlier polish pass; verify first).

---

## 2026-06-20 (in-app feedback pass 4) — composite (multi-opening) arches + Slate variation

- **Composite windows** (the arch styles a single crown can't make): new `m3::composite_window(kind, base,…)`
  returning `(mesh, is_glass)` parts — 15 **Serliana/Palladian** (central round arch + two flat-top side lights
  + mullions), 16 **Venetian** (central arch + two smaller round arches), 17 **Triumphal** (tall central arch +
  two short side arches + heavy piers + entablature band). Wired into Archive: `arch_style >= 15` builds the
  composite (glass panes emissive, mullions/piers dark stone), else the single styled arch. Archive seed now
  selects mod 18; `LOGSCOPE_ARCH_STYLE=15/16/17`. Layouts verified by plotting the opening geometry directly.
  So the full set is now 18 (15 single-crown + 3 composite) of the user's ~21-row table.
- **Slate** read "too uniform" → stone NODE size now varies by the turn's weight (tokens), 0.17–0.45, on top of
  the per-seed `rock()` shape variation (other scenes keep the steady 0.26). Fog already eased in pass 2.
- 258 lib tests still pass.

STILL OPEN: the remaining table rows are mostly already covered or are minor variants (Roman==Round, Round,
Equilateral==Pointed, Three-centered done, Basket-handle==Elliptical done); Forest expand + heavier LOD is the
main untouched item; Slate's scene-level plainness (vs just the nodes) could get a dedicated builder; and the
pass-2 brightness changes still want an on-GPU confirmation.

---

## 2026-06-20 (in-app feedback pass 3) — Old Library node forms + arch set expanded to 15

Continuing the in-app feedback batch (pass 2 did the systemic grey-out/fog fix, Archive leaf-balls + first 8
arches, and the muted button skins):

- **Old Library nodes now follow content** (the flagship ask). New `m3::library_node(kind, …, unroll, …)` (one
  merged mesh, PAPER): kind 0 **page** (thin card, short snippets) · 1 **book** (closed tome, normal prose
  turns) · 2 **scroll** (code/file/long turns — the sheet **unrolls further the longer the content**). Wired in
  the chat.rs node loop for "Old Library": short `tokens` → page, `tokens>180` or code-ish preview (fences /
  braces / `;` / `fn `/`def `/`()`/`::` / a path-with-extension) → scroll with unroll ∝ tokens, else book.
  Verified by unit test `library_node_forms_are_distinct_and_unroll_extends_scroll` (page slimmer than book by
  bbox volume; book is a closed box; scroll's y-extent + min-y grow with unroll). Inspector:
  `LOGSCOPE_LIBNODE_DEBUG=<scale>`. (PAPER blows out on llvmpipe + the 2D node dots overlay, so the clean look
  is for the GPU; the FORM is unit-verified.)
- **Arches expanded 8 → 15.** Added Keyhole, Trefoil, Cinquefoil, Elliptical/Basket-handle, Parabolic, Catenary,
  Three-centered (all silhouette-verified by plotting `arch_crown` directly). Archive seed now selects mod 15;
  `LOGSCOPE_ARCH_STYLE=<0-14>`.
- All 258 lib tests pass.

STILL OPEN (next): Slate "too uniform" (fog eased pass 2; wants per-node/scene variation — own builder or stone
size/shape/colour spread); Forest expand + heavier LOD; composite arches that need MULTI-opening window structure
(Venetian, Serliana/Palladian, Triumphal) — these aren't single-crown so they need a new windowed-arch assembly,
not just an `arch_crown` arm. Brightness changes from pass 2 still want an on-GPU look.

---

## 2026-06-20 (in-app feedback pass 2) — systemic grey-out + Archive + Reading Room

The widespread "greyed out / can't see anything" (Canopy, Mistfall, Atlantis, Cloudscape, Forest, Treehouse) +
"too much fog" (Archive, Slate, Old Library) traced to TWO systemic dimmers — **fixed, but BRIGHTNESS NEEDS
ON-GPU VERIFICATION** (llvmpipe over-brightens, can't judge here):
- **Time-of-day exposure floor was crushing everything.** chat.rs exposure had `*(0.52 + 0.42*tod_strength)`
  with `sun_clock()` strength ∈ [0.10,1.0] → the WHOLE palace ran 0.56× (night) to only 0.94× (noon), tied to
  the user's wall clock. Raised to `*(0.82 + 0.20*tod_strength)` → 0.84×..1.02×: keeps a day/night mood without
  going dark. (This alone should clear most grey-out, since the user tests in the evening.)
- **Fog densities** (mixed toward the grey theme bg) lowered: default 0.017→0.011 (Archive/Slate/Old Library/
  Reading Room), Mistfall 0.055→0.044, Treehouse 0.030→0.022. Atlantis (0.009) untouched — its grey was the
  exposure floor + underwater tint, not fog.
- Highlight shoulder knee 0.78→0.85 (less compression of bright scenes toward grey).

Per-scene (VERIFIED on llvmpipe — geometry/hue, trustworthy):
- **Archive:** removed the drifting autumn leaf-clusters ("floating leaf balls"). Added **arch-style variety** —
  generalized `arch_window` into `arch_window_styled` + `arch_crown(style,…)` with 8 profiles (0 Round · 1
  Segmental · 2 Pointed · 3 Lancet · 4 Tudor · 5 Ogee · 6 Horseshoe · 7 Multifoil), all silhouette-verified;
  the Archive picks one from `env_params().seed` (override `LOGSCOPE_ARCH_STYLE=<0-7>`). Fewer-fog from above.
- **Reading Room / Archive / Canopy / Old Library:** per-scene button SKIN — the default dark egui buttons read
  as harsh dark contrast on the bright/warm scenes; gave them a lightened, muted warm fill (Reading Room =
  terracotta on warm paper; Archive = amber-brown; Canopy/Old Library = mossy wood) with low-contrast ink, and
  tinted the carved Overview button to match.

STILL OPEN from this feedback (next batch): Old Library nodes → books/pages/scrolls by content (pages for short
snippets, bundled scrolls/draft docs for code/files); Slate "too uniform" (add variation, less fog — fog done);
Forest expand + heavier LOD; Mistfall legibility (fog eased; recheck on GPU); the bigger arch table beyond the 8
(Venetian/Serliana/Triumphal/Basket-handle/Elliptical/Parabolic/Catenary/Trefoil-vs-Cinquefoil) if wanted.

---

## 2026-06-20 (vision tower) — gpu-gemm ViT/CLIP encoder block — **APPLIED + GATE-VALIDATED**

Toward the parked img2img / generative-scenes path ("text → image → encode → geometry → mind palace"):
applied turn9-vit-block.patch to `crates/gpu-gemm/src/lib.rs` (the standalone engine crate, lib `gpu_kernels`,
EXCLUDED from the main workspace — build with `cd crates/gpu-gemm && cargo build`).

The patch was cut against an older lib.rs (line numbers drifted; `git apply` would reject) so it was applied by
hand: added `pub struct VisionBlock`, made `Lin::with_bias` pub, added `Gpu::vit_block` (pre-norm: LN → full MHA
→ residual → LN → MLP(fc1→GELU→fc2) → residual).

vit_block depended on 5 primitives NOT yet in the tree — implemented them (kernels register automatically; the
pipeline cache is keyed by each `&'static` WGSL const's address):
- `layernorm_t` + LAYERNORM — TRUE LayerNorm (mean-subtract + population var + eps, per-channel scale+bias).
  Distinct from the existing `rmsnorm_t` (no mean, no bias) — vision towers need LayerNorm.
- `gelu_tanh_t` + GELU_TANH — 0.5x(1+tanh(√(2/π)(x+0.044715x³))) (SigLIP `gelu_pytorch_tanh`).
- `gelu_quick_t` + GELU_QUICK — x·σ(1.702x) (OpenAI CLIP).
- `attention_mh_full_t` + ATTENTION_MH_FULL — bidirectional twin of `attention_mh_t` (loop j in 0..t, no causal
  `j<=i` mask); same GQA layout, scale 1/√head_dim.
- (`add_t`/`add_bias_t` already existed; `im2col` referenced in the patch as an anchor isn't present in this tree
  version — it's a separate earlier-turn primitive, not needed for vit_block to compile.)

Wrote `--gate-vit-block` (main.rs / kernel-validate): builds a small block (t=5,d=16,4 heads,mlp=32) with biases
on all 6 linears and checks GPU `vit_block` against a pure-f32 CPU reference for BOTH activations.
RESULT on llvmpipe (GL backend): tanh-GELU & QuickGELU both max|Δ|=2.98e-8 (f32 rounding) ✓. This is pure
compute — exact arithmetic — so llvmpipe IS trustworthy here (unlike the render-brightness caveat). Regression:
`--gate-matvec` and `--gate-qkv` still bit-exact.

Forward path (the rest of the vision tower, each ≈ a turn): patch-embed (conv/im2col + class token + pos embed)
→ stack N vit_blocks + final LN → projector into the LLM embed space; then a vision LOADER (GGUF→VisionBlock
name-map per CLIP/SigLIP/Qwen2-VL). "Back to mind palace": image embedding → RoomSpec / point-cloud generator
(DESIGN_generative_scenes.md) feeding scene3d.

---

## 2026-06-20 (later) — dragon forge — **BUILT + RENDER-VERIFIED on llvmpipe** (supersedes the UNBUILT note below)

Toolchain restored (rustup.zip re-uploaded): offline Rust 1.96.0 → ~/.local via the bundle's INSTALL.sh,
`apt install mold`, and env.sh exports PATH **and** LD_LIBRARY_PATH=.../rustlib/.../lib (prefer-dynamic links
libstd dynamically, so the bin won't start without it). Clean build 4m44s; incremental ~4s (mold).

Verified (geometry/silhouette — what-exists only, per the llvmpipe caveat):
- `dragon_head()` compiles/links/runs; Atlantis renders fine (Mesa llvmpipe, GLSL 330). Only the 3 pre-existing
  deprecation warnings (chat.rs/app.rs).
- Poseidon statues are dragon-faced: snout+jaw mass forward above the shoulders, horns/frill reading, trident intact.
- Isolated head (new `LOGSCOPE_DRAGON_DEBUG=<scale>` inspector — one head on a plain floor, lifted clear of the
  node strand) reads as a sea-dragon in profile: cranium + snout/lower-jaw w/ mouth gap + 2 swept horns + picket
  frill crest.
- Gargoyles: original sc=0.5 was too small AND sat just outside the architrave (occluded from inside) → **bumped to
  sc=1.1 and seated ON the ring** (r_g=r_col, y_g=y_cap+0.55). Now perch visibly on the architrave, facing outward.

Deferred to real GPU (llvmpipe blows STONE to near-white, even NO_POST): absolute brightness, surface/teeth
detail, and the emissive cyan **eye-glow** — none judgeable here.

Tuning candidates (next):
- Cranium reads a touch bulbous / snout slightly stubby vs the prototype's longer-muzzle morphospace.
- Prototype intent is a parametric **genome** (muzzle ML / jaw JD,JW / brow / cheek / crest vary per conversation);
  mine currently varies only eye hue by seed → wire seed→genome so different turns grow different dragons.
- Prototype shows cheek **shields** (armor plates); §7 spec said gill-fins (I built gill-fins) — confirm which.

---

## 2026-06-20 — dragon forge (sea-dragon head) — CODE WRITTEN, **UNBUILT / UNRENDERED**

- Added `dragon_head(pos, face, sc, seed, col, col_lo) -> Vec<(Mesh3,Float)>` in scene3d.rs (before
  poseidon_statue). Sea-dragon skull: cranium(orb pair) + 3-seg drooping snout + dropped lower jaw + teeth
  (ivory) + brow ridges + glowing cyan/amber eyes (emissive 1.8) + 2 swept-back horns + 6-spine midline frill
  + 3-fin cheek gill-fans. Built ALONG the face vector (rotation-safe), STONE material.
- Returns a Vec (not &mut Scene) on purpose: poseidon_statue's `put` closure holds a mutable borrow of the
  scene across the head position, so the head is captured then added at the end of the fn.
- poseidon_statue: replaced the human head+beard (the two `ball()` lines) with `dragon_head(head, face, ...)`.
- atlantis(): added an outward-facing GARGOYLE ring on the architrave/cornice (r_col+0.25, y_cap+0.45), count
  (cols/2).clamp(6,14), gapped at hallway mouths via near_axis, each on a short stone corbel; reuses
  dragon_head at sc=0.5.

⚠️ BLOCKER this session — could NOT build or run the §3 render loop:
  - No Rust toolchain in the env and no env.sh. The upload manifest listed rustup.zip + dragon_forge_3d.html +
    character_forge_3d.html but only HANDOFF.md + logscope-suite.zip actually landed on disk.
  - apt only offers rustc/cargo 1.75 (too old for egui 0.34.3 / wgpu 29 / naga 29 / winit 0.30 MSRVs); the
    .cargo/config requires the mold linker (absent); official rust download domains aren't in the allow-list.
  - So this change is written against the real primitives and reviewed by hand (coord-order bug fixed), but is
    UNCOMPILED and UNRENDERED. NEXT SESSION: restore the toolchain (re-upload rustup.zip), `cargo build
    --features gpu-scene --bin scene-view`, then render Atlantis close on a statue + on the dome rim
    (LOGSCOPE_THEME=Atlantis, POS_ANGLE_INIT tuned to a statue), judge proportions, and tune. The
    dragon_forge_3d.html prototype (if re-uploaded) is the intended look to match.

# logscope-suite — prioritized TODO

Status tags: ✅ built+validated (needs wiring) · 🔧 wire into app · 🐛 bug · 🖥️ M2-measured ·
📦 needs LAN/boxes · 🎨 egui render · 🧪 design/experiment

---

## BATCH 0 — Correctness: the looping/termination bugs (do first, blocks real use)

- 🔧✅ **Stop on the EOG *set*, not one id.** Wire `genguard::stop_token_set` into the app's eos
  resolution + decode-loop break. Read GGUF `tokenizer.ggml.eos_token_id` first (like the harness),
  then add every stop special. For DeepSeek-8B the set is `{128001, 128008, 128009}` — llama.cpp's
  EOG list confirms it. *Root cause of the DeepSeek whole-answer loop (app watched only 128009).*
- 🔧 **Port `build_prompt` ChatML/llama3 detection to the app prompt builder.** smollm2 got Llama-3
  text-mode garble because the app didn't frame it in ChatML → never reached its real `<|im_end|>`(2).
- 🔧✅ **Wire `genguard::runaway_stop` into the decode loop.** Backstop: cut on 4-gram repeat or a
  leaked pseudo-marker so nothing runs to the cap again (the 1134-tok / 88s and 1346-tok / 327s runs).
- 🔧 **Add `repeat_penalty` to the app sampler** (engine has `apply_repeat_penalty`) so models can
  *escape* loops, not just be stopped in them. temp 0.35 alone locks them in.
- 🐛 **llama.cpp `BackendAlreadyInitialized`.** Backend init is a global one-shot; the app calls it
  per model load → 2nd model fails. Init once at startup, reuse for all loads.

## BATCH 1 — Storage unification  ✅ DONE (content-addressed central cache + eviction; models_root resolver)

- ✅-idea **Don't persist derived artifacts; reference + regenerate.** The `.int8cache` and `.tmp`
  files aren't a small *diff* of the model — they're a deterministic *re-quantization* of it. So key
  them by the model's content hash + transform params, regenerate on demand, and evict under pressure
  (the vault's "version inputs, re-derive outputs" model). That's the storage win, not a byte-diff.
- **Unified models folder, content-addressed + deduped across projects.** One `models/` store keyed
  by blob hash so the same GGUF isn't copied per project. Note: GGUF is already quantized ≈
  incompressible, so the win is **dedup**, not extra compression — don't over-invest in compressing
  the blobs; compress the *metadata DB* instead.
- **OS defaults for the unified folder** (answers below): Linux `~/.local/share/logscope/models`,
  macOS `~/Library/Application Support/logscope/models`, with `LOGSCOPE_MODELS_DIR` override and
  auto-detect of `~/.ollama/models` for ollama unification.

## BATCH 2 — Memory  ◐ footprint log done; q4-resident + mmap deferred (M2/space)

- **Don't hold the whole model resident for big models.** 30B as int8 ≈ 30GB → won't fit 32GB UMA.
  Use the **q4-resident path** (≈17GB for 30B, dequant-per-op via existing q4 kernels) instead of the
  int8cache for models that don't fit as int8. Pick the path by model size at load.
- **mmap the GGUF** (llama.cpp does; log shows `mmap=true`) so the OS pages weights rather than RAM-
  loading them. On UMA the GPU buffer *is* system memory, so this pairs with the q4-resident choice.
- **30B model support** — lands once the above two do.

## BATCH 3 — Backend performance  ◐ per-generate dispatch profile done; fused QKV designed, deferred (measure first)

- 🖥️ **Dispatch fusion** — fused QKV (1 matmul, mirrors the loader's gate+up concat), fused decode
  block to cut the ~210 dispatches/token. (m=1 matvec + pipeline caching already exist.) This is the
  ~5× headroom; correctness-gateable here, perf measured on M2.
- 🖥️ **Profile our backend vs llama.cpp CPU.** llama.cpp wins because it's compute-bound and
  saturating (mature SIMD/flash-attn/fused ops), while ours is *dispatch-bound* (the floor we
  measured). Profiling confirms where the dispatch time goes before/after fusion.
- 🖥️ **Streamed/flash attention in prefill** (no seq×seq materialization) — folds into the cliff fix.

## BATCH 4 — Backend integration & hybrid

- **Ollama as a backend** + file passing logscope↔ollama (the unification you were poking at).
- **Hybrid: llama.cpp small-model summaries while our backend does the main work.** llama.cpp's speed
  bonus on small models = live summaries/side-channel while our engine runs the primary generation.
- 🔧✅ **`LocalWorker`** — wrap the real engine behind the `Worker` trait so the fabric dispatches
  actual tokens, not a mock. Highest-leverage single wiring step.
- 📦✅ **rustls `Connect`** — the HTTPS transport impl on a box (the only un-mocked seam in the fabric).

## BATCH 5 — Retrieval / context graph (mostly built, needs wiring + data)

- 🔧✅ **Wire `pipeline::dispatch_with_context`** so dispatched branches actually assemble minimal
  context under the asymmetric objective.
- **Populate the context graph from a real conversation/corpus** so minimal-context + bridge have
  substance to retrieve.
- 🧪 **Semantic/embedding edge layer** — the fuzzy counterpart to the literal bridge (needs a small
  embedder). Pairs with the exact layer for recall.
- 🧪 **Fuzzy proposer** — small model scores frontier swaps for `accept_update` to filter.
- **DAWG-minimization** of the context graph for size at scale.

## BATCH 6 — UI / egui (needs the running app to judge)

- 🎨 **Generated-files panel → right sidebar** (from the screenshot). Layout reflow.
- 🎨 **Firefox-style scroll** — momentum+friction for wide trackpad sweeps, small fixed increments for
  precision wheel ticks, velocity threshold between. *Feel must be tuned on the live app; expect a
  round or two of "more/less friction."*
- 🎨 **`<think>` hide/expand toggle** — `Seg::Think` already renders; add collapse (default collapsed).
- 🎨 **Convo selector** — load saved convos / suite / vault nodes into the chat view.
- 🎨 **Tree view polish** — prune/delete, hover preview with smart scroll, smooth text-lerp +
  redline highlight (uses `diff::word_diff`/`redline`), crossout-minimize.
- 🎨✅ **"Merge this as a commit/branch?" suggestion** — detect partial-edit files (the
  `// ... existing code ...` / `// ... rest ...` markers, fenced regions) via the vertex + diff
  primitives, offer to stitch into the full file as a vault commit/branch.

## BATCH 7 — Model support

- 🧪 **Qwen2 Tier-1 arch** (read `qwen2.*` metadata, Q/K/V bias, parameterize RoPE θ) → unlocks the
  Qwen R1 distills + Qwen2.5/SmolLM3 that currently panic "missing embedding_length".

## BATCH 8 — Experiments / longer-term

- 🖥️ **Run the branch/K experiment on M2** (smollm2 first — degenerates → signal-rich).
- 🧪 **Speculative file fast-forward** — self-spec decoding using the model's own `<think>`-block draft
  as the speculation (verify-and-accept). The better of the two thinking speedups.
- 🧪 **Reasoning-dims metric** on `ast.rs` (file a/b, v1/v2, task A/B over Item × vault node).
- 🧪 **Phase-2 coordinator ↔ live tree** parallel branches across boxes (the fabric, wired to the trie).
- 🧪 **FSM context-assembler tier-1** — interaction-mode state machine driving per-mode vault queries.

---

## Suggested first sprint (one batchable pass)
Batch 0 (all five — they're one decode-loop + prompt-builder edit) → unblocks every model.
Then Batch 1 storage refs/dedup (your stated top win) and the `LocalWorker` wiring from Batch 4,
since those two together let you load the 30B and dispatch it for real.

---

## BATCH 9 — Wide doc-orchestration (parallel small model + attention-graph scheduling)

Spend the fabric's spare throughput on a parallel SMALL model that documents and watches the tree,
surfacing non-blocking suggestions, while a tree-similarity (attention-graph) score decides which
branches schedule first. Design spec = the dogfooded `inputs.zip` corpus (unendlich voxel engine,
parallel Opus/Haiku workers): each worker OWNS a file set (stays in lane, others off-limits),
VERIFIES against the tree (`cargo check/test` → gate green), reconciles tree-vs-brief drift,
composes pure building blocks instead of mutating them, emits 1-line coordinator wiring, and tags
what it couldn't run UNVERIFIED. The `new-documentation/` output (incl. `conflicting-menus/`) is the
target behavior.

- **Background doc/watcher worker** [dep: Batch 4 hybrid + LocalWorker] — a small fast model
  (llama.cpp speed bonus) runs alongside the main worker(s), drafting docs from the live
  transcript/tree and computing suggestion triggers, WITHOUT injecting into the active worker's
  context. Suggestions queue for the human and merge in only on accept → no mid-task context
  disruption.
- **Suggestion triggers (ambient, non-blocking popups):**
  - file crosses ~1K LOC → "you just crossed 1K+ LOC — encourage helper libraries?" (+ link a
    generated helper-library guide, cf. `llm_helper_library_guide.docx`: grep-first scan, reuse
    ratios, pure-fn extraction).
  - novelty/discovery in the transcript (new cross-refs, repeated reasoning) → "important
    discoveries — document them?"
  - two branches edit the same surface (the `conflicting-menus` case) → "reconcile?" (diff/redline).
- **Attention-graph scheduling (the prioritization)** [dep: Batch 5 context_graph + pipeline] — rank
  branches/workers by tree-similarity to the accepted docs/anchors (context_graph bridge / minimal-
  context overlap). Branches that START from high-scoring docs schedule FIRST; workers that hit
  perf/test failures (gate red / UNVERIFIED) get deprioritized. This is the novel weight: a doc that
  scores higher on tree similarity pulls priority toward the branches aligned with it. Wire into
  `fabric_net::rank_entries` / the pipeline scheduler.
- **Worker contract on the fabric** — formalize the corpus pattern: each dispatched worker declares
  OWNED files (lane), runs a gate, emits wiring seams + UNVERIFIED tags as provenance. Feeds
  merge-as-commit (Batch 6) and the vault.
- **Doc artifacts → vault, scored** — generated docs land as vault nodes carrying their tree-
  similarity score, so the selector/tree surfaces the highest-attention docs first.

Mostly an orchestration layer ON TOP of primitives that already exist (fabric, context_graph,
pipeline, vault, diff). The genuinely new build is: the scheduler weight, the trigger monitors, and
the ambient popup queue that defers context-injection until accept.

---

## Frontend / backend separation + a headless TUI [larger refactor]

Right now the egui app (`logscope-egui`) links the engine (`gpu_kernels`) directly and owns both the
inference loop and all the conversation-state logic (the tree, branching, pruning, the modal layer,
exports). That couples a GPU+windowing frontend to logic that has nothing to do with either. Pull them
apart so the same engine can drive more than one frontend — in particular a terminal UI for headless
boxes (a remote M2 over SSH, a CI runner, a server with no display).

The seam already half-exists: the engine ships `logscope-serve` (the netserve loopback + auth gate +
routing are gate-tested), so there's a server surface to talk to. The work is to make the *conversation
model* frontend-agnostic and then build a thin TUI on top.

- **Lift the conversation core out of the egui app** into a plain crate (no eframe/egui/glow dep):
  the `Msg`/tree/branching/prune model (convograph), the prompt-assembly + window selection, the
  estimate/consensus/fuzz/treeexport/memplan glue, and the modal *logic* (vimmode is already pure —
  good template). Leave only rendering + input in `logscope-egui`. This crate is the shared brain.
- **Define one backend boundary** the frontends call: either in-process (`Inference` trait over the
  local `gpu_kernels` worker) or over the wire (`logscope-serve` JSON/stream). Same API both ways so a
  frontend doesn't care whether the GPU is local or on another machine.
- **TUI frontend** (`logscope-tui`, ratatui + crossterm): the linear chat + a box-drawing tree pane
  (treeexport::ascii is literally already the renderer), the modal keybinds (vimmode maps 1:1 to a
  terminal — no focus-handoff fight, which is the egui pain point), `:`-commands, streaming tokens to
  the pane. The spatial 3D view doesn't port, but the ascii tree + jump keys cover headless navigation.
- **Why it's worth it:** runs on a headless remote M2 with zero display stack; the modal layer is
  cleaner in a terminal than in egui; and forcing the logic out of the GUI is good hygiene regardless.
  The pure-module discipline we've been keeping (every load-bearing piece tested standalone via
  `rustc --test`) means most of the brain is *already* frontend-free — this refactor mostly moves files
  and writes one trait.

### Wiring the mount/bundle flow into the app [follow-up to mount.rs]
mount.rs (detect → isolate → snapshot → bundle → apply_hint) is built + validated end-to-end with real
git (the roundtrip test clones the bundle AND applies the patch onto a fresh tree, with the source
proven untouched). What remains is the app glue:
- `:mount <path>` — detect + isolate into `tmp/logscope/{uuid}`, show the workspace + kind in the status
  bar. A conversation/branch then targets that workspace.
- Point the compiler oracle (sandbox::verify) at `ws.dir` so `cargo check/test` runs in the scratch —
  the agent's edits land there, never in the user's tree.
- `:bundle` — snapshot + produce the `.bundle` + `.patch` into the exports dir, print apply_hint.
- Map a conversation BRANCH ↔ a git branch in the scratch (fork the chat → `git checkout -b` a sibling
  work branch), so the tree of conversations becomes a tree of reviewable bundles.
- NOTE: mount.rs + sandbox.rs are both frontend-free backend logic — move them into the shared core
  crate in the frontend/backend split above (they're only in logscope-egui for now).

## :foreach command — map-reduce / fan-out + ordered merge over a collection (BATCH LATER)
A `.logscope` directive + runner orchestration. Shape: SPLIT (chapters / files / sections / child
branches) → apply a TASK TEMPLATE to each item in its own sub-context (fan-out workers) → ORDERED
MERGE handed to a separate merge-agent. Reuses existing fan-out directives (`:Nca`/`:fa`) and the
coordinator/consensus/multiparty modules; mostly a new directive + a dispatch pass.
Use cases: "extend each chapter" (split-by-chapter → expand → merge in order); "per file, decide if it
needs changes for feature X + summarize" (split-by-file → analyze → merge into one plan another model
executes). Buildable + unit-testable NOW against the stub backend; live runs need a model (the engine we
decoupled for the headless loop). Grammar sketch: `:foreach <split> { <task template> } :merge <merge-prompt>`.
Batch candidates to bundle with it: split strategies (file glob / heading split / branch children),
worker-pool dispatch, deterministic stub outputs for tests, merge-agent prompt, partial-failure handling.

## MIND-PALACE REGIONS BEHIND THE DOORS + 3 INBOUND MERGES (built, compiled, tested, rendered) — 2026-06-19
Toolchain note: built with a real Rust 1.96 + Mesa llvmpipe headless this session, so everything below is
COMPILE- and TEST-verified (not just eyeballed). `cargo test -p logscope-egui --lib` = **256 passed, 0 failed**.

REGIONS (the graph→mind-space program — the long-standing NEXT item; steps 1·2·3 + geometry of 4 + 5):
  - scene3d.rs: new `RegionType {Rotunda, Stairs, Terrace, Chamber, SealedCave}` + `Region` descriptor +
    `region_plan(pr, doors, r_hall)` (one wing per branch). Space TYPE is chosen from the branch's graph
    shape (loop→rotunda, deep→stairs, long→terrace, token-heavy→chamber, pruned→sealed cave). The shape
    signal is currently a DETERMINISTIC per-(seed,branch-index) stand-in — stable per conversation, varied
    across — clearly marked as the seam where real per-branch graph shape drops in once that descriptor is
    plumbed through chat.rs (left untouched here while perf work was inbound).
  - five seeded generators (`rotunda`/`stair_descent`/`terrace`/`chamber`/`sealed_cave`), each reusing the
    element library (columns, arches, statues, sea-garden, rocks) and EMITTING COLLIDERS.
  - atlantis() door block rewritten: doors are now driven by `region_plan`, sit IN the perimeter wall, and
    the wing is generated BEYOND it. New mesh3d helper `wall_ring_gapped()` OPENS the perimeter at each door
    (real passable openings, not a glow on solid stone). `region_door()` emits jamb-only colliders (clear
    centreline). Verified by render: a Chamber wing and a Rotunda wing visibly attach at perimeter doors of
    the 136-node marathon (renders: wings_low.png = chamber, wings_low2.png = rotunda, wings_birdseye.png =
    dome+skylight). 6 new unit tests (plan count/stability, kind-mix variety, branchy build emits wing
    geometry+colliders, gapped wall removes quads).
  - NEXT (unchanged, now unblocked): (a) THREAD THE FLY THROUGH — gap the perimeter `Bound` per door + thread
    the fly camera (chat.rs); (b) real per-branch shape → `region_plan` (replace the stand-in); (c) route
    door bearings to the branch's actual direction; (d) per-type visual polish (hue/scale) + camera hero-shot
    hooks per wing. Region geometry slots into the existing env cache (rebuilds only when EnvParams change).

MERGES (two clean patch bundles from other chats, applied + committed; full diff base = git):
  - PERF (gpu-gemm): 6 bit-exact patches — fused QKV (loader builds `wqkv`, decode/q8-stream paths behind
    `LOGSCOPE_FUSED_QKV`), GPU argmax+gather greedy sampling, K-per-submit windowed decode. COMPILES clean
    (fixed one thing the patch set missed: the resident-block test ctor in main.rs needed the new `wqkv`
    field). GPU correctness gates (`--gate-qkv*`, `--gate-gpusample`, `--gate-window`) still need the real
    Asahi/M2 box — run them there; this box has no Vulkan.
  - BACKEND/CMD (logscope-egui + gpu-gemm/store.rs): KV-tree reuse default 1→3 (`LOGSCOPE_KV_TREE`/`_OFF`
    overrides); int8-cache `make_room_for` (evict-before-write) + `cache_size_bytes`/`purge` helpers;
    composer message-EDIT box given a stable id (last unprotected TextEdit); NEW `coach.rs` onboarding module
    (registered, 9 tests pass) — UI glue (~5 lines in chat.rs) still TODO; `devloop.rs` copied as an
    UNREGISTERED scaffold (its mount/decompose/sandbox/bestregion/merge deps all exist, so it could be wired,
    but its API usage is unvalidated — leave per author until checked).

Z-AI "more model support" bundle — REJECTED (see docs/handoffs/z-ai-eval.md). It's a divergent-base full-tree
snapshot that would clobber the palace + pearl-input + anamorph work and delete existing files; its
model-support core is a 903-line gpu-gemm/lib.rs rewrite that collides with the just-merged fused-QKV work and
is explicitly M2-unvalidated (flips Gemma2/Phi3 `supported:true` with no bit-exact gate). Quarantined; salvage
path = re-cut ONLY the arch content against the current tree, keep `supported:false` until per-arch gates pass.

## Backdrop garbage band fix + the gpu-scene feature gotcha — 2026-06-19
User ran `LOGSCOPE_GLASS=1 cargo r --release --features wgpu-engine,llamacpp` and saw a GARBAGE band across
the frosted top. TWO findings:
  - ROOT CAUSE: the full-window backdrop had TRANSPARENT regions (open sky beyond geometry); behind the
    Background layer is undefined framebuffer memory -> garbage on their driver (same family as the magenta).
    FIX: make the backdrop OPAQUE. GPU composite gained u_opaque + u_bg (fill empty regions w/ theme bg, force
    a=1) + clamped frost-blur sample coords; AND a painter rect_filled(bg) under the backdrop canvas so the
    CPU path is covered too. Verified CPU-path backdrop is garbage-free.
  - FEATURE GOTCHA: --features wgpu-engine,llamacpp does NOT enable gpu-scene (that's a separate feature:
    gpu-scene=[gui]; wgpu-engine is just gpu-gemm). So they were on the CPU software renderer -> NONE of the
    GLSL work (pearl, frost, bloom, underwater, the magenta SCISSOR fix, LODs) was compiled in. They must add
    `gpu-scene` to the feature list to get all of it. Both configs build; 257 gpu-scene tests green.
  - FOLLOW-UP: consider making gpu-scene part of a default/recommended GUI feature set so this isn't a footgun.

## IN-APP FEEDBACK (on-GPU) — pass 1 done + big backlog — 2026-06-19
  DONE this pass: highlight soft-shoulder (composite); Reading Room/Cloudscape/Atlantis brightness down;
    removed Atlantis flying books; Mistfall water-over-ground + heavier fog.
  QUEUED (by scene), from in-app testing:
  - GLOBAL: confirm brightness on GPU after shoulder; possibly per-scene shoulder knee.
  - CANOPY redesign: DONE structurally — central tree + open mossy circle + fanning maze (concentric broken
    arcs + radial spokes) of moss stone shelf walls w/ chunky book-slates. NEEDS on-GPU look check: llvmpipe
    blows the flat stone walls to white so brightness/contrast/shelf-detail must be judged in-app; likely
    follow-ups: tune wall brightness, more slate contrast, maybe thinner/finer shelves, add colliders.
  - MISTFALL: cane walls; far-off ANTIGRAVITY waterfalls that fall away into mist then loop back + recondense.
  - ATLANTIS: which 'rings cut through the middle'? (ask) ; SEA-DRAGON statue faces via a procedural
    'dragon forge' generator (reuse for GARGOYLES on top); fix ceiling-structure gaps; (books done).
    Also: clamp/ease whites done via shoulder — verify.
  - CLOUDSCAPE: more wispy fog; occasional STORMS w/ multi-colour LIGHTNING arcs tuned globally to content;
    NIGHT variation w/ moonlight.
  - FOREST: many more tall thin trees; ground leaves + PERSISTENT fallen leaves (30min then decay + ragdoll
    pileup -> needs a timed/stateful system); RABBITS + CATS darting between bushes; extend size + heavy LOD
    fog blot-out (not walls).
  - TREEHOUSE: DONE this pass -> twinkling bioluminescent fireflies (shader), denser trees + fog LOD blot-out,
    thick planks + procedural holes, procedural moss on trunks/platforms. STILL TODO -> fuzzy creatures
    swinging; wood GRAIN + surface deformation (shader/material); varied wind gusts + trickle rain.
  - FOLDING@HOME concept: DNA strand that SPITS OUT proteins as the graph nodes.
  NOTE: persistent-decay leaves, running animals, storms, sea-dragon forge, Canopy maze = each its own turn.

## Treehouse global height factor + UI/Atlantis panel — 2026-06-19
  - Treehouse canopy height = GLOBAL graph-derived factor (canopy_lift/spread off env seed, like Mistfall rune).
    Drives walk base height + platform stagger + tree heights. (Subtle across similar synthetic seeds; varies
    more for differently-shaped real conversations.)
  - UI: label_cells cap 16->32; message entry LEFT-ALIGNED pixel-stable + font -12% + wider plaque + blinking
    caret. Full prompt tail now shows (was truncated at 16 + centred/sliding).
  - Atlantis panel: glassier + brighter pearl/cyan + glowing cyan bezel frame.
  - Possible next: true pixel-perfect overlay of the real egui input onto the 3D text (would need an aligned
    invisible TextEdit); more Atlantis panel iteration; Folding@home concept; Weather+sway().

## Treehouse: denser forest + hanging wood-ball nodes — 2026-06-19
  - +16 platform-less trees (denser forest, ~21 total).
  - Treehouse node style: carved WOOD BALLS (wood mat, brown tones) each hung by a rope to the canopy overhead;
    labels stay as inscription. Treehouse strand/node mat = WOOD.

## New scene: Treehouse canopy walk — 2026-06-19
  - Morris-Arboretum-'Out on a Limb'-inspired: tall trees from a dim floor far below, 4 treehouse platforms
    (deck+railing+hut roof+lantern) up in the canopy, swinging rope bridges between them, fireflies at dusk.
  - Helpers: firefly(), tree_platform(), rope_bridge(catenary side-ropes + planks, shared Float sway).
  - TREEHOUSE theme registered (warm dusk palette + scene_light + open/no-enclose).
  - Backlog unchanged: Folding@home concept; shadow-MAP; Weather+sway(); per-room lighting; RoomSpec+sanitize.

## Shadows + UI bezel + bees (polish 3/3) — 2026-06-19
  - SHADOWS: contact_shadow() now directional (offset anti-sun + enlarged) -> cast-shadow look everywhere.
  - UI: bezel frame behind the message plaque + left-edge glowing accent strip (reads as a physical tablet).
  - BEES: bee() helper; scattered near flowers in Forest + Canopy.
  - The 3 requested polish items (DOF, materials, shadows/UI/bees) are DONE.
  - Remaining backlog: Folding@home concept; full shadow-MAP pass (vs current blobs); Weather+sway() system
    (kelp/seaweed/branches/leaves adopt one helper); per-room lighting; RoomSpec JSON+sanitize -> parked
    LLM/image/point-cloud generators (user resumes those at the physical program).

## Material variations (polish 2/3) + DOF tune — 2026-06-19
  - MATERIALS (scene FS by v_mat): MOSS leaf TRANSLUCENCY (backlit glow, research-backed), PAPER thin-glow,
    GLASS fresnel rim (transparent water). Vary per scene via per-scene light.
  - DOF tuned softer + wider per feedback (x1.5, blur 1.3x..2.4x focus). Screenshots rendered DOF off for pixel-peeping.
  - NEXT (polish 3/3): shadows + UI polish + BEES.

## Depth-of-field (polish 1/3) — 2026-06-19
  - Scene-pass DEPTH renderbuffer -> sampleable DEPTH_COMPONENT24 TEXTURE (the dependency). Composite reads it,
    linearizes (NEAR/FAR 0.1/400), CoC for surfaces beyond the focal plane (u_focus=orbit dist), reuses
    scene_frosted() disk-blur. FAR-FIELD ONLY -> content + near UI stay sharp. LOGSCOPE_DOF tunable (def 0.7).
  - Verified on Atlantis (distant columns/dome bokeh, strand+UI crisp).
  - NEXT (polish 2/3): MATERIAL VARIATIONS for leaves/pages/glass per scene ('the maps').
  - THEN (polish 3/3): shadows + UI polish + BEES.

## Fix ocean-in-library + Forest + grass/flowers/birds — 2026-06-19
  - FIX: Atlantis kelp was INSIDE the air dome (looked flooded) -> moved to the open sea OUTSIDE the glass.
  - HELPERS: grass_tuft, flower, bird (reusable across scenes).
  - NEW SCENE: Forest (open sunlit woodland; trees+grass+flowers+birds). FOREST theme registered everywhere.
  - CANOPY: added grass tufts + wildflowers + passing birds.
  - STILL OPEN (user asked, queued next): DOF (needs scene depth as a texture in the composite — check
    glow_scene FBO depth attachment), real shadow pass, UI polish, MATERIAL VARIATIONS for leaves/pages/glass
    per scene ('the maps'), and BEES. Plus Folding@home concept, Weather+sway(), per-room lighting.

## Build-out: Mistfall ruins + Reading Room desks + Atlantis kelp — 2026-06-19
  - MISTFALL: ring of standing-stone megaliths (standing_stone(): rough rock squeezed thin + contact shadow),
    tall/broken/half-sunk — ancient-site feel. (Read better at eye-level than top-down.)
  - READING ROOM: reading_desk() pedestal tables w/ glowing tome + book stacks (4 nooks) + floor stacks.
  - ATLANTIS: kelp beds (strand fronds, height-tinted MOSS, Float-drift sway) on the sea floor.
  - NOTE: Reading Room reads over-bright on llvmpipe; verify on GPU, may trim its light profile if blown there.
  - Remaining build-out: Folding@home concept; Mistfall waterfall polish; per-scene eye-level hero shots.
  - Queued systems: Weather+sway() (kelp/seaweed/branches will adopt it), per-room lighting, shadow pass,
    RoomSpec JSON+sanitize (then the parked LLM/image/point-cloud generators).

## Build-out: Archive cathedral + Canopy understory — 2026-06-19
  - ARCHIVE is now a RECTANGULAR MARBLE CATHEDRAL (nave): marble floor + warm aisle runner, two tall column
    rows, arched clerestory windows (emissive warm light), vault ribs -> central ridge, glowing lectern+tome,
    autumn-leaf signature drifting through. Marble outer shell, no shelves.
  - CANOPY: understory fern/shrub clusters (contact-shadowed) + a 2nd sapling -> layered forest depth.
  - NEXT scene build-out candidates: Mistfall (more ruins/standing stones + tune the waterfalls), Reading Room
    (reading desks/stacks — it's still sparse origami), Folding@home concept, Atlantis seaweed/kelp.
  - Deferred systems still queued: Weather+sway(), per-room lighting, shadow pass, RoomSpec JSON+sanitize.

## Per-scene lighting + Cloudscape + generative design — 2026-06-19
  - PER-SCENE LIGHTING (the immediate ask): scene_light(name) -> sky/ground/ambient/exposure per scene (was a
    single hardcoded fill). Each space now has a distinct mood. Rooms inherit their LOGSCOPE_THEME profile
    (per-ROOM lighting = next: thread a room light profile through).
  - CLOUDSCAPE: new open/airy theme+scene (cloud-deck + drifting cumulus, shares Mistfall's globule idea but
    bright). Skips enclose. In PALETTES + dispatch + scene_light.
  - DESIGN_generative_scenes.md: reference-image reskin / local-LLM components / point-cloud rooms, all via a
    sanitized RoomSpec. Suggested build order: RoomSpec JSON+sanitize, then LLM(GBNF), reskin(palette), point
    cloud(RANSAC+heuristics), then the model-heavy tier.
  - 16 survey rooms kept in the loop (LOGSCOPE_ROOM); user likes Greenhouse/Crypt/open scenes.
  - NEXT: implement Weather struct + sway() (research note), per-room lighting, RoomSpec JSON+sanitize, shadow pass.

## Room-variety engine (16 rooms) + weather/light research — 2026-06-19
  - ROOM ENGINE: build_room() interprets a spec (floor/perimeter/roof/focal/accent/props); ROOMS[16] =
    Cathedral(=Archive cathedral target, rect nave+vault), Observatory, Greenhouse, Crypt, Scriptorium,
    Foundry, Zen Garden, Lighthouse, Temple, Conservatory, Cavern, Planetarium, Dojo, Vault, Atrium,
    Hearth Hall. LOGSCOPE_ROOM=<n>. Installment 1 of up to 24 — surveyed in two 4x2 grids for selection.
  - RESEARCH note RESEARCH_weather_light.md: leaf transmission (Habel 2007, forward-scatter back-light term),
    ocean via Gerstner waves (wind-driven, foam at steepness) vs Tessendorf-FFT (heavy), canopy rain/sun/wind,
    a shared Weather struct, and one sway() helper for seaweed/fabric/thin-branch/leaf bending.
  - NEXT (informed by research): Weather struct + sway() helper; Gerstner ocean for Atlantis; leaf transmission
    for Canopy silhouette; the deferred shadow/light pass; more rooms toward 24 once user picks favourites.

## Branched all themes (per-theme detail pass) — 2026-06-19
  - UI CAST theme-aware: Atlantis pearl · Canopy leaf-glow · Mistfall blue rune · Archive/Reading ink-on-paper.
  - MISTFALL nodes: unified carved-blue-rune glow (env-variant color, faint pulse).
  - CANOPY leaves: cupped/curved blades + per-vertex normals (were flat/polygonal). Crown reads less faceted.
  - ARCHIVE + READING ROOM: colonnade_ring() (Atlantis columns re-skinned: warm WOOD / pale STONE) + contact
    shadows + colliders — main-hall seed.
  - MISTFALL water: waterfall() sheets + foam feeding the lake disc.
  - DEFERRED (intentional): Canopy text-as-light-beam-silhouette + leaf shadow depth -> the shared SHADOW/LIGHT
    pass (tune across scenes); deeper flowing-water TEXTURE; Folding@home concept.
  - User will adapt Archive/Reading hall layout from seeing these pillars in 3D.

## Framework to all scenes + smaller text + overlays — 2026-06-19
  - In-view UI panel + message-entry resonance text now render in EVERY scene (was Atlantis-only). Universal
    chrome substrate. Verified in a 2x3 scene grid (Atlantis/Canopy/Mistfall/Archive/Reading Room/Folding).
  - Text sized DOWN (label_cells 0.052->0.034; panel 3.4x0.78; 22-char window).
  - Top/bottom (header+tab) are frosted TRANSLUCENT overlays now (alpha 196), floating over the scene.
  - STILL: 1b real interactive in-world chrome (hit-test 3D panels); 2-full sun shadow-MAP + lantern shadows;
    3 fullscreen + pause-off-tab; procedural-detail enrichment of the non-Atlantis scenes (they read sparse
    next to Atlantis — add architecture/props per theme).

## Immersive-UI plan IN PROGRESS (1a + shadows step) — 2026-06-19
Ordering: (1) text-on-panel + UI port, (2) shadows/light-tracing, (3) fullscreen+pause.
  - 1a DONE: message entry renders ON the nacre panel via m3::label_cells (3D resonance-cell glyphs) — dim
    'speak into the pearl' prompt when empty, live bright tail when typing. No RTT needed; cells are real
    depth-sorted scene geometry.
  - 2 STEP DONE: soft planar contact_shadow() discs (radial alpha falloff, lifted above the mosaic inlay)
    ground columns/statues/plinth via the existing alpha blend.
  - NEXT: 1b port header/tab controls onto in-scene panels; 2-full sun shadow-MAP depth pass + lantern point
    shadows; 3 fullscreen immersive + pause repaint off-tab.

## Nacre panel + pearls; godrays/bloom restored — 2026-06-19
  - NACRE: added a view-INDEPENDENT pearl sheen (rainbow flowing by world-pos+time) to water/glass so flat
    head-on facets (the UI panel) shimmer; u_nacre 1.0->1.4. Panel is now pale pearl-white (emissive .20,
    alpha 132). Pearls/dome/floor get a touch. Background warp kept.
  - LIGHTING: god-ray 0.6->0.9, bloom x.6->x.85, threshold x1.12->x1.0.
  - DEFERRED — the immersive-UI cluster (each a real piece of work, do next):
    1. TEXT-ON-PANEL for the message entry: render the composer text to an offscreen texture and sample it on
       the panel quad (render-to-texture), or screen-align an overlay to the panel's projected rect.
    2. PORT the chrome (header/tabs/controls) onto in-scene panels.
    3. FULLSCREEN immersive + animation bake + PAUSE frames when not on the scene tab (gate the repaint).
    4. SHADOWS / light tracing for sun + lanterns (shadow-map depth pass, or a cheap contact-AO approximation).
  - STILL OPEN: banding (z-fighting) + bookshelf holes/gaps — pending user specifics on the matching GUI.

## Physical UI panel (step 1) + water-warp hallway fix — 2026-06-19
  - WATER WARP fix: the underwater ramp keyed off horizontal distance past the main Bound, but procedural
    hallways/wings (Gate bearings) sit outside that radius and are AIR -> false submerge. Now: past the
    perimeter, suppress the warp inside any Gate corridor (angular wedge + margin for the ellipse skew). Only
    true open water warps. (User: confirm by flying down a hallway — should stay dry.)
  - PHYSICAL UI PANEL step 1: frosted translucent slab billboarded in front of the camera (lower-centre), the
    'blurred panel' substrate. NEXT: nacre/pearl skin (set the panel mat to the nacre path + thin-film), then
    map the actual composer/controls onto it (render-to-texture or screen-aligned overlay).
  - DOME Q (answered): the see-through look is MINE (set_alpha 110 + blend last turn); the dome SIZE (r_hall =
    13+sqrt(nodes)) and stained-glass pattern (pr.seed) are procedural fingerprinting. Geometry unchanged.
  - STILL TODO: banding (z-fighting) + bookshelf holes/gaps — pending user pointing at specifics on the
    now-matching GUI.

## ROOT-CAUSED scene-view vs GUI discrepancy: gpu-scene wasn't default — 2026-06-19
THE answer to 'I don't see what you show me': scene-view has required-features=[gpu-scene] (always GPU GLSL),
but logscope-egui default was [gui] only -> the GUI fell back to the CPU software renderer (render_env +
m3::render). All my shader ports (pearl/frost/exposure/bloom/underwater) are GPU-only, so they never showed
in the user's GUI. FIX: gpu-scene is now DEFAULT (verified: GL provenance prints from a plain cargo build).
CPU-only: --no-default-features --features gui.
BATCHED scene work (now visible in the GUI):
  - UNDERWATER 3D-AWARE: u_cam_warp = camera (yaw,pitch) feeds the ripple+caustic field so rotating flows the
    refraction through world-locked water + edge parallax (was pure screen-space slide).
  - STAINED-GLASS TRANSPARENCY: scene pass alpha-blends (already painter-sorted back-to-front); dome/clear
    panes/leaded mosaic get alpha (110/120/175) so the ocean reads through; opaque (a=1) unaffected.
  - STILL TODO (need user's now-matching GUI to point at specifics): texture-overlap BANDING (z-fighting),
    bookshelf positioning + proper holes/gaps. Deferred rather than guessing blind.

## Fixed the glass TRAP; removed the broken full-window backdrop — 2026-06-19
User report: LOGSCOPE_GLASS=1 -> 'fully blocked by the foldingathome world', and my shown images don't match
their GPU. ROOT CAUSE of the block: the full-window scene was an egui Area in Order::Background, but egui
PANELS are also Order::Background with nothing beneath -> the backdrop painted OVER the chrome (invisible UI),
and with the default (Folding@home) theme that read as being trapped in that world.
  - FIX: removed the backdrop Area. Glass = frosted visuals + SOLID header & tab bar (always clickable escape
    hatch) + Esc-to-exit + the mind-palace in the central area (the chat's normal spatial scene) + opens in
    Spatial. In-app Glass header toggle means LOGSCOPE_GLASS is never required. Auto-seed guarded (is_empty).
  - TRADE-OFF: the scene no longer extends BEHIND the header/tabs (that overlay was the bug). To get a true
    full-window backdrop the right way, render the scene as a PRE-PASS before egui paints (eframe clear/GL
    hook), NOT as an egui layer. Deferred.
  - RENDER MISMATCH: llvmpipe headless != user's GPU (their normal-mode scene looks great; mine blow out the
    centre). Stop tuning brightness to my renders; trust user screenshots.

## Glass mode is now a LIVE TOGGLE (fixes 'don't see the changes') — 2026-06-19
The glass/frost/full-window work was all gated behind LOGSCOPE_GLASS, so a normal launch (the user's machine,
Vulkan/llama.cpp) showed none of it. Now there's a 'Glass' switch in the header bound to self.glass.
  - Toggling it live: frosted visuals + full-window backdrop + CentralPanel Frame::NONE (transparent) so the
    backdrop shows through; chat.set_glass(true) makes spatial_ui SKIP its GPU scene callback when
    glass_host && !backdrop -> the backdrop is the only scene render (no double-render seam).
  - LOGSCOPE_GLASS still seeds the initial value. Built + 257 green + verified (off = normal app w/ the toggle;
    on = frosted full-window mind-palace).
  - FOLLOW-UPS: glass toggle persistence (Settings); when toggled on with no convo, auto-seed (currently only
    the env-launch path seeds); 2D node hit-testing alignment in glass (the canvas is transparent but still the
    interaction surface); central plume brightness.

## Band-gated frost — clean viewport, frosted chrome only — 2026-06-19
Built + 257 green + rendered (logscope-egui glass: header band frosted, central viewport crisp).
  - The whole-backdrop blur is gone; frost is now BAND-GATED in the composite: u_frost_a (y≈0 edge) +
    u_frost_b (y≈1 edge) set the band thickness; scene_frosted(uv, def) blurs per-pixel only inside the bands
    (top = header+toolbar, bottom = composer+switcher), sharp in the middle viewport. defocus 0.85.
  - app.rs renders the backdrop Area at the END of the frame (Order::Background still puts it behind), so it
    has THIS frame's header/switcher rects to size the bands (header.h+46, switcher.h+108, /screen.h).
  - Orientation resolved: top chrome = frost_b (y≈1), bottom = frost_a (y≈0).
  - NEXT polish: bands are full-width horizontal; if the Panels drawer (side) or wide cards need frosting,
    pass their rects too. Central plume still the brightest element. Then: window transparency; toggles into
    Settings; pearl anchored to composer + cast-on-send.

## Frosted-glass UI + map-port continuation — 2026-06-19
Built + rendered (logscope-egui glass: readable frosted chrome over a blurred mind-palace, auto-seeded).
  - FROSTED GLASS: panels were too transparent -> frosted cool-tinted fills at higher alpha (~188-232) so the
    UI is readable, AND the backdrop scene is BLURRED behind them: new u_defocus disk blur (13-tap golden-
    angle kernel) in the composite, threaded through Uniforms; the backdrop sets defocus=0.7.
  - MAP PORT: glass mode AUTO-SEEDS the demo mind-palace when launched without a convo (no LOGSCOPE_SEED
    needed) -> the full-window backdrop always shows a map.
  - BRIGHTNESS (flagged again): oculus sky 2.3->1.5, halo 1.1->0.7, Atlantis strand beam 1.7->1.1. Central
    crystal plume is the brightest bit remaining at close camera angles — tone its emissive next if needed.
  - NOTE: 1 lib test fails (sandbox::offline_first) — it shells out to rustc for target probing, an env flake
    in the sandbox; sandbox.rs was not touched. Other 256 pass.
  - NEXT: per-panel backdrop blur (true frost only behind chrome, scene sharp elsewhere) if the global blur is
    too soft; window transparency; glass/pearl toggles into Settings; pearl anchored to composer + cast-on-send.

## FULL-WINDOW mind-palace backdrop (real show-through) — 2026-06-19
Built + 257 green + verified in the logscope-egui bin (scene fills the whole window; header/toggles/hotkey
line float translucently over it; conversation nodes in-scene).
  - ChatTab::scene_backdrop(ui): renders ONLY the 3D scene. New `backdrop` flag gates the Overview/Prune
    control row, the empty-state hint, and the focused-node preview strip; canvas fills the whole rect;
    spatial_view is saved/restored so the chat tab's own (linear) content doesn't double-render the scene.
  - app.rs glass mode: paints scene_backdrop into a screen-filling egui Area at Order::Background BEFORE the
    header/central/switcher panels (borrow chat out, Area::show closure). Translucent visuals already in place
    -> every panel floats over the living mind-palace.
  - Toned the aquatic god-ray 0.95 -> 0.6 (central blowout). Central SPIRE plume is now the brightest element
    left — tone its emissive next if still too hot.
  - NEXT: glass mode currently needs a convo (LOGSCOPE_SEED) for backdrop content — auto-seed a demo in glass
    if empty; true window transparency (with_transparent + clear_color); move glass/pearl toggles to Settings;
    anchor the in-scene pearl to the composer + cast-to-node on send.

## Composer dot + bloom + viewport + PEARL CONTROLS — 2026-06-19
Built (both bins) + 257 green + rendered (4 pearl variants; GUI shot). Addressed the screenshot feedback:
  - removed the cheap white specular DOT on the 2D composer pearl (read flat/2D).
  - TONED DOWN the blowout: exposure *0.52 (was *0.62 base), bloom_amt *0.6, bright threshold *1.12.
  - EXPANDED the spatial viewport: canvas reserve 92px -> 30px (the scene fills much more of the panel).
  - PEARL CONTROLS wired to the renderer: a toolbar TOGGLE (pearl_on) + VARIANT DROPDOWN (pearl_variant)
    -> new u_pearl_variant uniform; 4 presets (balanced/fine-crackle/milky/clear-glass) modulate whiteMix,
    crack frequency, chromatic dispersion + refraction strength. env still overrides for headless preview.
  - SHOW-THROUGH: partial (bigger viewport + glassy chrome). STILL TODO the real one: render the 3D scene as a
    FULL-WINDOW BACKGROUND LAYER (egui Background order) behind header/tabs/cards + then window transparency;
    and: position the in-scene pearl at the composer / cast-on-send; move glass+pearl toggles into Settings.

## WATER warp + GLASS-UI start in the main bin — 2026-06-19
Built (scene-view + logscope-egui) + 257 green + rendered (inside-vs-outside warp; glass vs baseline GUI).
  - UNDERWATER WARP: composite u_underwater — eye outside the air dome (outside perimeter Bound, aquatic) ->
    layered sinusoidal UV ripple (refraction) + aqua absorption tint + caustic shimmer + depth vignette.
    chat.rs ramps it from sqrt(eye.x^2+eye.z^2) vs the Bound radius. Verified: warps outside, clear inside.
  - GLASS MODE (LOGSCOPE_GLASS) in the logscope-egui bin (app.rs): translucent chrome (alpha-reduced panel/
    window/widget fills), skip the opaque 2D ambience, land on the Chat/spatial tab. Confirmed the 3D scene
    + pearl composer render in the MAIN app (glow_scene GL provenance line logged at startup).
  - NEXT (the real port): render the 3D mind-palace as a FULL-WINDOW BACKGROUND LAYER (egui Background order)
    so it sits behind the header/tabs/cards too, not just the central panel; then true window transparency
    (ViewportBuilder::with_transparent + App::clear_color) is optional. Also: a glass toggle in Settings
    instead of env; carry the underwater warp into the GUI camera modes.

## FIX — Asahi/Apple magenta tint = leaked GL_SCISSOR_TEST — 2026-06-19
Root-caused from the user's M2 Max report (GL 4.6 Core, GLSL 330; magenta over most of frame except a
horizontal band; NO_POST/NACRE/GL_ES all identical -> upstream of post/nacre/profile).
  - CAUSE: egui calls the paint callback with GL_SCISSOR_TEST ENABLED (box = widget rect). Our offscreen-FBO
    glClears were clipped to it, so most of each target kept UNDEFINED tile memory = magenta on a tiler GPU
    (Apple/Asahi). llvmpipe read it ~black so the bug was invisible there. The correct 'horizontal bar' was
    the scissored region. Toggles did nothing because they're downstream of the half-cleared scene texture.
  - FIX: disable GL_SCISSOR_TEST for the offscreen passes (full clear), restore before the composite so it
    still honours egui's widget clip. Verified non-regressing on llvmpipe; awaiting user confirm on M2.
  - The earlier diagnostics (GL_ES/NO_POST/NACRE toggles, GL provenance line, zero-init, NaN-clamp) stay as
    defence-in-depth + future debugging.

## RENDERER cross-driver compat + magenta-tint diagnostics — 2026-06-19
For the Asahi (Apple-silicon Mesa) magenta tint in scene-view (can't repro here — only llvmpipe). All in the
shared renderer, so the GUI bin benefits too. Built + 257 green + verified on llvmpipe (incl. GLES path).
  - HARDENING (any of these may be the fix): zero-init FBO colour textures (undefined tiler target memory ->
    garbage/magenta; also matters now god-rays can be LOD-skipped); identity RGBA texture swizzle; composite
    output NaN-guard + clamp to [0,1].
  - DIAGNOSTIC TOGGLES (work in scene-view AND the GUI; scene_ui routes through chat.rs): LOGSCOPE_GL_ES
    (native #version 300es), LOGSCOPE_NO_POST (bloom/godray/pearl off -> scene vs post), LOGSCOPE_NACRE +
    u_nacre uniform (scale/kill the water thin-film -> prime suspect for a magenta cast), + a one-time
    GL_VENDOR/RENDERER/VERSION/GLSL provenance line at startup.
  - docs/handoffs/driver-compat.md: the bisection table + what each result implicates.
  - IF it survives NO_POST and NACRE=0 -> cast is in base lighting / sampler2DArray material sampling; next
    test a flat-material pass. Likely top fix: the zero-init (tiler) or the NaN/clamp guard.

## PERF — render-resolution + optical-effect LODs (transferable, in glow_scene) — 2026-06-19
Built + 257 tests green + verified (native vs 0.6x/0.5x, with+without pearl, godrays on/off — clean, no
artefacts). All in the SHARED renderer (glow_scene/gpuscene) + scene3d, so the GUI bin (chat.rs renders
through the same glow_scene::paint) gets them for free.
  - RENDER-RESOLUTION LOD: scene+bloom FBOs sized rs*native; composite upscales (samples by UV). Pearl + UI
    + composite stay native-crisp (only the 3D scene downsamples). Cost ~rs^2 of scene fragments. Targets
    gained sw/sh/rs; ensure() recreates on scale change. render_scale() reads LOGSCOPE_RENDER_SCALE else
    LOGSCOPE_QUALITY (clamped <=1).
  - OPTICAL-EFFECT LOD: at rs<0.7 skip the god-ray fullscreen pass and zero its composite contribution.
  - MESH-DENSITY LOD: scene3d::render_quality() already scales the terrain grid (extend to other scatter).
  - CACHING (confirmed, pre-existing): with_env caches the baked Scene; only rebuilds on palette/variant/
    EnvParams change; per-frame only Float offsets update.
  - GUI WIRING (later, easy): replace the env reads in render_scale()/render_quality() with a value threaded
    from a Settings quality slider (one helper each) — no structural change.
  - REMAINING (the 3rd axis): FRAME-UPDATE LOD — cache the packed vertex buffer and skip scene.animate +
    re-pack when the camera is idle and floats haven't moved (today chat.rs re-packs all verts every frame);
    advance float-time at a capped rate when idle/low quality.

## SEAFLOOR TERRAIN + env palette variation + first LOD lever — 2026-06-19
Built + 257 tests green + rendered (seabed hills, reefs on hillside, look-out, second log for stone var).
  - TERRAIN: seafloor() = displaced radial heightfield (rings x spokes) outside the dome. Depth via a radial
    downward slope (deeper at the rim); hillsides via value-noise fbm (broad hills + finer ridges); sand in
    the hollows grading to rock on the mounds (per-vertex height colour); smooth (area-weighted) normals.
    seafloor_h() is shared so reefs/gardens/arches REST ON the hills instead of hovering.
  - ENV VARIATION: stone FAMILY per conversation from hh(2) — teal-basalt / sandstone / limestone / slate —
    each carrying its own sand+rock tones for the seabed. (Adds to the existing warm cool<->kelp bias.)
  - LOD (first lever): render_quality() reads LOGSCOPE_QUALITY (0.6..1.3) and scales the terrain grid
    density (mesh-density LOD). STILL TODO — the other two LOD axes the user asked for:
      * frame-update LOD: rebuild/relayout + Float updates less often when idle/far; cache the static scene
        mesh and only re-stream on change (today atlantis() rebuilds each set_env_params).
      * optical-effect LOD: scale post chain (bloom/godray iterations, caustics, pearl dispersion/cracks) by
        quality + distance; drop the refractive-pearl crack second-layer + dispersion at low quality.
      * render-resolution LOD: render the scene FBO at a fraction of native and upscale in composite.
  - DEPTH-MODULATION approaches (for reference / further work): quantised height -> terraces/plateaus; ridged
    (1-|noise|) -> canyons/ridges; negative ridged -> trenches; domain warping -> organic coastlines; a raised
    plateau under the dome; amplitude-by-region (flat near the hall, hillier far out).
  - ARCH VARIATION next: vary column roundness/flutes, dome profile, arch shapes, hall/loop/dome dimensions
    per env (roundness + geometric-pattern knobs).

## PROCGEN — floor mosaic + hallway windows + reefs/arches — 2026-06-19
Built + 257 tests green + rendered (overhead mosaic, exterior overview, hall fly, interior). More of the
world generated:
  - FLOOR MOSAIC (rotunda): 7 concentric inlaid rings (jewel palette, alternating emissive) + a gold central
    medallion with a lapis ring + a ring of 40 tesserae at mid-radius; all scaled to the floor ellipse, low
    emissive so the caustics play over polished inlay.
  - HALLWAY WINDOWS: each hall bay's upper band (above the stacks, below the lintels) is glazed — stained
    glass (3x4, jewel palette) on alternating bays, clear aqua panels between, each under a stone arch frame.
  - OCEAN FORMATIONS: reef() = mounded boulders + coral knobs + kelp fronds; rock_arch() = two leaning rock
    piers bridged by a keystone span (fish thread through). Scattered with the sea gardens in the outside
    water band (counts follow nodes/branches).
  - NEXT (mind-palace): more wing-interior variety; align node path with doors; live 'cast pearl' flow.

## PEARL fractures finer + camera path-follow + floating books — 2026-06-19
Built + 257 tests green + rendered (dense-fracture pearl 2x2 + fly 2x2 + interior). Three things:
  - FRACTURES denser+tinier: worley seam frequency 5.5→12.5, seam threshold 0.10→0.055, plus a second
    finer crack layer (2.3x freq) → a fine crazed-glaze crackle instead of a few big facets.
  - CAMERA follows the strand, less wall-frying: fly follow-distance 7.5→5.2 (hugs the path); collision
    relaxed over 3 passes so the eye settles in the gap between columns instead of tunnelling; floor/ceiling
    clamp (from pillar y0/y1) keeps the eye between floor and dome.
  - PROCGEN: floating_books() scatters open books (pale pages on a coloured cover, two-leaf V) drifting on
    their own Floats in the central air — count from nodes.
  - NEXT: live 'cast into the scene' composer (drive pearl_pos/r + text on the pearl + cast animation);
    align node path better with architecture so transit stays in corridors; more procgen (chandelier, mosaic).

## PEARL — real in-scene REFRACTIVE pearl (shader) — toward 'cast into the scene' — 2026-06-19
Built + 257 tests green + rendered (in-scene pearl 2x2 incl. off-reference). Per feedback (the 2D painter
had hit its ceiling: no real refraction/fresnel/thin-film, too dark, scene didn't scatter through, depth
unconvincing), moved the pearl INTO the scene's shader pipeline:
  - SHADER_COMPOSITE_FS now renders a screen-space pearl that SAMPLES u_scene to REFRACT the rendered scene
    through a pale pearl body — chromatic DISPERSION (R/G/B sampled at different offsets), FRESNEL rim,
    THIN-FILM nacre interference, WORLEY crack seams that add bend+rainbow along the cracks, specular
    hotspot, soft outer halo. Pale/white substance (mix toward 0.9,0.95,1.0) — luminous, not dark.
  - Uniforms gained pearl/pearl_pos/pearl_r/aspect; set in glow_scene::paint composite; env-driven
    (LOGSCOPE_PEARL / _POS / _R) for headless preview via scene-view. Shader still compiles 300es + 330.
  - This is the foundation for the 'cast your next pearl into the scene' flow: drive pearl_pos/r/presence
    live from the composer, resolve the typed text on the pearl's surface, and on send ANIMATE it flying to
    the new node's position (cast) — replacing the 2D pearl_input box.
  - NEXT: wire live composer→in-scene pearl + text-on-pearl + cast animation; retire/great-reduce the 2D box.

## PEARL COMPOSER — photographic pass (close the 'webpage vs photo' gap) — 2026-06-19
Built + rendered (composer 2x2). Reworked pearl_input::show from a flat panel into a lit object, per
feedback (boxiness / opaqueness / no light-bleed / uniformity / no DOF / photoreal gap):
  - DE-BOXED: stadium body (corner = h/2) + perimeter + rounded-end VIGNETTE so edges fall off into the
    scene instead of a hard UI border.
  - TRANSLUCENT: glassy low-alpha fill you can see INTO; subsurface interior glow + domed gloss + a sharp
    specular HOTSPOT + counter-sheen → a rounded, lit form (egui has no gradients/blur, so faked by
    stacking faded circles).
  - LIGHT BLEED: outer bloom rings; bloomy multi-halo cells.
  - DOF: the cracked nacre facet web is thrown OUT OF FOCUS (each edge smeared across 5 offset passes, dim)
    so it sits behind the focal text.
  - PHOTO ARTEFACTS: animated film grain (luminance noise), lens CHROMATIC ABERRATION (red/blue fringes
    growing toward the edges), non-uniform cell size/brightness.
  - NEXT: maybe a hair of blur on the text itself for full focal integration; carry the same treatment to
    the 3D node pearls (bloom/DOF/grain) so scene + composer match; iterate hotspot placement.

## PEARLS — 3D node pearls cracked/faceted to match the composer — 2026-06-19
Built + 257 tests green + rendered (pearl close-up 2x2 + composer 2x2). Unifies the node pearls with the
cracked-pearl composer design:
  - mesh3d::cracked_pearl(center,r,seed,color): a faceted, seeded radially-jittered (±10%) sphere, FLAT-
    shaded (per-face normals, duplicated verts; positions from the shared jittered sphere so facets still
    meet) with a per-facet NACRE-spectral vcol blended onto the base colour — each facet catches the
    iridescence at its own angle (the 3D echo of the composer's cracked pearl). Wired as NodeStyle::Pearl.
  - pearl inner glow eased 0.22→0.14 so the faceted body reads under bloom; speckle + text-on-approach
    resonance still resolve up close.
  - NEXT: optional per-pearl coalesce/disperse on (de)focus; push facet relief if more 'cracked' wanted;
    wing interiors; fish schooling.

## SCENE — hallway stacks + fly end-cap + lamp tone — 2026-06-19
Built + 257 tests green + rendered (fly 2x2 + interior 2x2). Polish/closeout:
  - HALLWAY STACKS: hallway() now lines each bay with low bookcases (between the columns, height 0.6× so the
    upper colonnade stays open to the ocean) — travelling a hall feels like walking the library stacks.
  - FLY END-CAP: a virtual pillar just past each hall's end archway caps the fly so it stops at the
    continuation instead of sailing into open ocean (closes the overshoot noted last pass).
  - LAMP TONE: nook lamp 1.5→1.12, hanging lantern 1.25→1.02, floor pools dimmer — no interior angle blows
    out now (fixes the over-bright cluster).
  - DISK: re-init ai-git LEAN (binaries gitignored) — 4.3G→3M, ~5.3G free; it won't re-bloat.
  - NEXT: cracked-pearl coalesce/disperse for the 3D scene node pearls (unify with the composer); more fish
    species variety / schooling; wing interiors.

## UI — PEARL COMPOSER cracked-pearl pass + 2x2 grids — 2026-06-19
Built + 257 tests green + rendered (pearl 2x2 + scene 2x2). The composer (pearl_input.rs, already wired in
chat.rs) now matches the chosen cracked-pearl node design:
  - CRACKED-PEARL SURFACE: a jittered, slowly-modulating triangulated facet web with NACRE-iridescent edges
    (cos-spectral, time-shifting) drawn faint behind the temporally-stable resonance text — same cracked
    pearl that holds each turn in the 3D scene.
  - SEND RIPPLE: PearlInput::pulse() (called from chat.rs on enter) fires an expanding nacre ring.
  - harness pearl-input-view rewritten: LOGSCOPE_PEARL_TEXT + LOGSCOPE_PEARL_PULSE, periodic re-pulse.
  - COLLAGES now 2x2: tools/grid.sh (interior eye-level), tools/flygrid.sh (fly-through), tools/pearlgrid.sh
    (composer states). Upload 2-3 2x2 grids per turn.
  - NOTE: disk hit 100% (ai-git grew to 4.3G from committed binaries); cargo clean + gc reclaimed to ~2.4G.
  - NEXT: cracked-pearl coalesce/disperse for the 3D scene pearls too; soft hallway end-cap; lamp cluster tone.

## SCENE — wings routed to real branch bearings + camera through doors/hallways — 2026-06-19
Built + 257 tests green + fly-through collage. Closes two long-standing NEXT items:
  - REAL BEARINGS: EnvParams gained `bear: [u16;6]`. chat.rs computes, per FORK (node with >1 child), the
    direction from the fork to the centroid of its NON-main-line descendants (main line = follow the
    largest-subtree child from the root) and encodes it as a turn fraction. `region_plan` now routes each
    wing to its branch's bear[d] (falling back to even spread when unset), nudged >=0.42 rad clear of the
    ±x hallway axis so wings and hallway mouths don't collide. Hallways already follow the spine (flow lays
    it along x), so they're correctly 'wired' too.
  - CAMERA THROUGH OPENINGS: new `Collider::Gate{ang,half}` emitted at every door + hallway mouth. The
    fly-camera collision in chat.rs pre-collects gates and lets the perimeter `Bound` OPEN where the eye's
    bearing falls in a gate — so the fly travels down hallways + through doors into the wings instead of
    being trapped in the rotunda. Lateral guidance past the wall is the hallway columns' Pillar colliders.
  - tools/flygrid.sh: 9-frame LOGSCOPE_FLY 0->1 montage (the camera journey).
  - NEXT: physical-UI pearl composer (pearl_input.rs) + cracked-pearl surface modulator; maybe a soft
    end-cap so the fly doesn't sail past the hallway end into open ocean.

## SCENE — immersion pass: reading nooks · alcoves · lantern warmth · upper-wall detail — 2026-06-19
Built + 257 tests green + 3x3 collage (interior-weighted). Toward 'immersive from any interior point':
  - READING NOOKS: `reading_nook()` = round pedestal table + warm lamp (bloom source + faint floor pool) +
    chair + book stack, scattered in the moat facing the shelves (count from nodes; Pillar colliders).
  - WALL ALCOVES: `wall_alcove()` = recessed arched niche + plinth + glowing urn, interleaved every 4th bay
    into the bookshelf ring for warm punctuation.
  - LANTERN WARMTH: lanterns brighter+warmer (emissive 0.85→1.25, larger) and each casts a faint warm pool
    on the water below.
  - UPPER-WALL DETAIL: pilasters (engaged columns) between the clerestory windows + moulded cornice torus
    bands at the drum base/spring line, so the upper wall behind the balcony isn't blank stone.
  - NEXT: per-branch bearings for hallways/wings; fly-through (Bound-gapping in chat.rs); maybe tone one
    over-bright lamp cluster; physical-UI pearl composer.

## SCENE — library BOOKSHELVES + 3x3 collage harness — 2026-06-19
Built + 257 tests green + rendered as a 3x3 collage. Toward 'immersive from any interior point':
  - BOOKSHELVES line the inside of the perimeter (the library identity): `bookcase()` = wood case + side
    posts + horizontal shelf boards packed with procedurally coloured book SPINES (varied width/height,
    warm palette). A ring of cases gapped at doors + hallway mouths; the moat band is now wood too. Lower
    walls now read as a flooded archive from every interior angle.
  - tools/grid.sh: renders 9 views (interior→overview, varied yaw/pitch/dist, each labelled) and montages a
    3x3 collage — the 'follow along' aid. Use: tools/grid.sh CONVO [HOUR] [OUT] [WAIT].
  - test: branchy_atlantis now asserts on wing COLLIDERS (door-gapped bookshelves confound total mesh count).
  - NEXT toward full immersion: reading nooks (desk/chair/lamp pools), arched wall alcoves, warmer lantern
    light, ceiling/upper-wall detail behind the balcony; then per-branch bearings + fly-through.

## SCENE — fish BESTIARY (many species) + OBJ import path — 2026-06-19
Built + 257 tests green + rendered. Upgrades the sea life from torpedo-blobs to real fish:
  - `FishSpecies` + `fish_species()` bestiary of 8 archetypes (tuna, angelfish, clownfish, eel, grouper,
    ray/manta, lanternfish[glows], whale). `add_fish` now builds in a LOCAL frame (+x fwd/+y up/+z lateral):
    a chain of laterally-compressed tapering ellipsoid body segments (countershaded belly→back) + a forked
    vertical caudal (or whale's horizontal fluke) + dorsal/anal/pectoral fins + eyes, then transforms to
    world. School species + scale vary per fish from the seed.
  - mesh tooling: `mesh3d::scale3` (3-axis, normal-correct) + a minimal Wavefront **OBJ importer**
    `mesh3d::load_obj_str` (v/vn/f, fan-triangulation, computed normals) — unit-tested. This is the hook to
    drop in CC0 models. Sources researched + written up in docs/handoffs/external-models.md (Quaternius CC0
    fish pack, Poly Pizza CC0 + API, Kenney, OpenGameArt; CGTrader/TurboSquid 'free' != CC0).
  - NEXT: wire real CC0 OBJ fish in (when provided) via load_obj_str+scale3; per-branch bearings for
    hallways/wings; fly-through; physical-UI pearl composer.

## SCENE — sea life beyond the boundary (fish schools + whales) — 2026-06-19
Built + 256 tests green + rendered. Adds the reference's depth-life: `add_fish()` (tapered body + nose +
vertical tail fluke + dorsal fin, all sharing one Float so it sways as a rigid body) and `sea_life()` which
scatters a fish SCHOOL (count from nodes, 4-colour palette) + 1-2 WHALE silhouettes in the water OUTSIDE
the perimeter, at heights visible through the colonnade / clear windows / open hallway sides. Air interior
stays clear. drift+amp on the Float makes them mill/bob in place.
  - NEXT (unchanged): route hallways/wings to ACTUAL branch bearings (needs per-branch dirs into EnvParams);
    fly-through down hallways + through doors (chat.rs Bound-gapping); physical-UI pearl composer.

## SCENE — un-blocked overview · HALLWAYS for continuation · balcony+larger stained glass · ribbed dome — 2026-06-19
Built + 256 tests green + rendered on llvmpipe. Reference saved to docs/refs/atlantis-direction/ (the user
is branching the direction but keeps the reference DOME). Addresses the review notes:
  - UNBLOCKED OVERVIEW: Atlantis no longer gets the generic `enclose()` outer room — that second shell
    (scaled huge to clear the wings) was the 'giant structure' dominating every overview. Atlantis is
    self-enclosing (its own ribbed dome + perimeter + open ocean), so overviews are clear now.
  - HALLWAYS (the missing 'linear runs → halls/colonnades'): a COMPACT domed rotunda holds the heart; the
    spine's linear CONTINUATION now extends OUT along ±x into colonnaded HALLWAYS (new `hallway()` gen —
    two column rows + water-floor causeway + glass roof + side lintels + an end archway, open sides to the
    ocean, column colliders). The rotunda colonnade + perimeter are OPENED at the hallway mouths. Length =
    how far the spine reaches past the rotunda (from ext_x/ext_z). The graph flows rotunda↔hallway.
  - TOP BALCONY (implemented) + LARGER STAINED GLASS: a railed upper gallery walkway rings the architrave
    (high — doesn't cage the central graph), and above it a tall arcade of BIG arched windows the dome
    springs from — now MOSTLY large stained-glass mosaics (clear only every 3rd → ocean peeks).
  - RIBBED GLASS DOME (reference-adapted): transparent cap + radial meridian RIBS + latitudinal RINGS +
    oculus/skylight; lower emissive so the ocean reads through. (mesh: dome_cap + tube ribs + torus rings.)
  - NEXT: route hallways/wings to ACTUAL branch directions; thread the fly camera down hallways + through
    doors (chat.rs Bound-gapping); fish/whales swimming OUTSIDE the dome glass; physical-UI pearl composer.

## SCENE — oval rotunda · de-caged · ocean windows · nacre pearls · more EnvParams detail — 2026-06-19
Built + tested (256 lib tests green) + rendered on llvmpipe. Addresses live review notes:
  - DE-CAGED: removed the legacy interior GALLERY RING (a torus at strand height that clipped peripheral
    nodes) and the upper-balcony RAILING + BALUSTER CAGE that boxed in the hall. Kept a thin cornice ledge.
  - OVAL ROTUNDA (fit the graph shape): EnvParams `extent`→`ext_x`,`ext_z` (per-axis half-spans from
    fold_pos). New `hall_dims()` → semi-axes (rx along the spine, rz across), aspect clamped ≤2.6:1. The
    whole shell is built circular at rx then COMPRESSED in z (new `mesh3d::scale_xz`) → an oval rotunda
    elongated along the conversation spine; columns/doors/wings/statues/lanterns placed at compressed
    centres with round cross-sections (no squished columns). enclose() already scales to contain it.
  - OCEAN WINDOWS: clerestory windows are now big and mostly CLEAR glass (WATER, low-emissive) set in
    OPENINGS cut through the drum (gapped at the window angles) so you see the ocean/sea-life outside; a
    few stained-glass mosaics remain as colour accents. Width/height/clear-vs-stained mix are EnvParams-
    driven (win_w from seed, mix cadence from branches).
  - NACRE PEARL SHADER: scene FS gained thin-film iridescence on water/glass — a fresnel-gated, view- &
    time-shifting spectral rim → mother-of-pearl shimmer on the dome, crystal, clear windows, and water
    pearls. (More pearl/physical-UI work — the interactive pearl_input composer, cracked-pearl surface
    modulator — is the next pearl-side step; this pass did the glass/pearl SHEEN.)
  - ENVPARAMS DETAIL: clerestory height, window size, window clear/stained mix, sea-garden patch count
    (vegetation density) now vary with the conversation, on top of the existing size/colonnade/tint/oval.

## SCENE POLISH — contain-the-graph view space · exterior vegetation · muscular statues — 2026-06-19
From live M2 feedback (smollm2 loads+outputs, gpu-scene renders) + review notes:
  - VIEW SPACE: the flow was breaking out of the hall (a 136-node spine spans ~±68 but r_hall capped at
    30). EnvParams gained `extent` (graph half-span, set from fold_pos in chat.rs); `hall_radius()` now
    sizes the hall to CONTAIN the flow, and `enclose()` is parametrized by `outer_r` (= wing_reach+margin)
    so the room always clears the bigger hall + its door-wings. Verified: strand sits inside the dome now.
  - AIR INTERIOR: sea gardens moved OUT of the air hall into the flooded ring OUTSIDE the perimeter (seen
    through the glass), and terraces now OVERLOOK a garden beyond their rim rather than growing one on the
    (air) platform. Matches air-inside/water-outside + the 'kelp outside the dome glass' note.
  - STATUES: poseidon_statue rebuilt from blobby tubes into an anatomical figure — planted contrapposto
    legs (thigh/calf/knee/feet), V-taper torso (broad chest→waist) with pectorals + abdomen, broad
    deltoid shoulders, neck, beard, two-segment arms (upper+forearm) with bicep/elbow/fist, trident in the
    raised fist. Pose varies by seed. NEXT: still slender — could add lats/thigh mass + drapery for heft.
  - PACKAGING: ai deliverable ships its history as `ai-git/` (not `.git/`) so `unzip -o` can't clobber a
    real tree's `.git` — use `git --git-dir=ai-git log`.

## SCENE BATCH — mind-palace depth/identicon theming + new worlds (queued 2026-06-16)
Headless multi-angle capture now works: `POS_ANGLE_INIT="yaw,pitch,dist"` env on scene-view (keeps target
on the graph centroid, honours dist so small values go INSIDE). Render via render-scene helper / Xvfb+llvmpipe.

1. IDENTICON-DRIVEN MIND-PALACE THEMING (the memorable effect). Identicon runes are already inscribed on
   pearls. Take further: run a CONNECTED DEPTH ANALYSIS over the graph and merge identicon data into a
   per-conversation theme seed that deterministically varies architecture — marble/stone COLOR, ARCHWAY
   geometry, WINDOW roundness / frequency / spacing, etc. Same conversation → same palace (brain-sticky,
   navigable from memory). Anchor all variation to the bible rules (enclosure, one centerpiece, transcendence
   light, concentric). This subsumes the earlier "topology-driven halls/rooms" item.
2. ATLANTIS polish: better GOD RAYS (need a bright oculus source for the existing post-pass to streak), add
   FISH and KELP swimming/swaying OUTSIDE the dome glass (depth life beyond the boundary).
3. NEW SCENE — CIRCULAR TREE LIBRARY: big halls, tall shelves full of books, SKYLIGHTS of indirect light
   with woven patterns, occasional STAINED-GLASS artworks that play with colored light + visual patterns.
   For now use identicons rendered as ULTRA-REALISTIC GLASS ART; later optionally hook an image-gen model to
   add real content to the mindmap.
4. COURTYARD (Canopy) LEAF-TEXT EFFECT: light through leaves magically coalesces into READABLE TEXT via a
   sampling algorithm that aligns individual "sun through leaves" rays into overlapping regions of
   bright / dark (dappled light spells the node's text). Research: dappled-light caustic-as-glyph sampling.
5. LODs: level-of-detail for nodes/architecture so large scenes stay fast (tessellation + culling by depth).
6. FIX (quick, from multi-angle capture): reflection ghosts render as solid dark beads — should be faint/
   translucent (mesh renderer ignores ghost alpha). Gate or emissive-fade for pearl scene. Also angle-B
   blowout: clamp exposure / min camera distance when pitched up into a caustic surface.

## :mansplain command (TODO) — small-AI response clarifier
A command that asks a SMALL/cheap AI to rewrite a response so it maps more directly and clearly to the
question it answers. Template (from mansplain-template.md): "take the question by the chat bot and make my
answer map more directly and clearly to it; use formal language that is open-ended and uses pseudocode
sparingly," fed { chatbot_question, my_response } → clearer rewrite. Give it more context than the bare
template (surrounding turns / the active node). Pairs with the :foreach batch (both are runner directives;
both testable against the stub backend, live runs need a model).

## PHYSICAL UI — pearl-as-message-box (design locked, build pending)
Message boxes are too small/flat; make the pearl itself the physical UI. On focus (click / walk-up):
PULL the pearl off the graph path toward the camera, ZOOM with depth-of-field focal blur on everything
else, and present the turn's text on it. Recommended text approach (two-tier — DON'T wrap arbitrary prose
around a sphere in 3D; our renderer has no in-scene font system and a sphere distorts text):
  TIER 1 (ambient, already done): identicon RUNES inscribed on every pearl — a unique, memorable glyph
    signature (NOT meant to be read). Optionally add a decorative english→glyph "script" for atmosphere.
  TIER 2 (focused, readable): two build options, ship in order —
    (a) BILLBOARD CARD first (cheapest, immediately readable): pulled pearl + DoF behind, crisp egui text
        card billboarded just in front. Physical via the pull/blur; readable because text stays 2D/sharp.
    (b) BAKED FACET upgrade ("truly inscribed"): give the focused pearl a flat medallion FACET facing the
        camera; render egui text to a texture and map it onto the facet (tablet-set-in-a-pearl). Consider
        MORPHING sphere→lens/coin on focus so the reading surface is flat (the user's "flatter surface" idea
        — this is the right call for legible inscription).
  A glyph transpiler is good for Tier-1 flavor only; it defeats Tier-2 readability.
Needs: focus state + pull/return animation, DoF post-pass (sum with bloom chain), egui→texture text bake.

## CINEMATIC CAMERA — walking-sim × content-framing blend (design, build pending)
Stop dead-centering the subject. Compute the shot as a blend of two anchors:
  WALK anchor: eye-height (~1.6 world-units above the floor plane), positioned a comfortable distance back
    ALONG the conversation path (as if a person stood there) — gives grounded, human framing.
  CONTENT anchor: frame the focused node/subgraph by rule-of-thirds (subject ~1/3 in, NOT centered),
    distance from a comfort range scaled by the subject's projected extent, slight elevation/Dutch for drama.
  Final = damp/slerp between the two (a "weight" param: 0 = pure walk POV, 1 = pure coverage shot), with
  smooth easing between shots so it reads like a Steadicam, not a snap. Reuse set_cam plumbing; add a target
  OFFSET (off-center bias) + eye-height + a path-tangent for the walk direction.
Quick first step available: add an off-center target bias + eye-height to set_cam so content isn't centered.

## VISUAL follow-ups (this-pass leftovers)
- GOD RAYS still not streaking as bright shafts — the radial post-pass needs the bright source visible
  on-screen and the shaft geometry isn't reading. Tune godray pass + make the oculus disc the actual
  screen-space source; the descending shaft geometry currently renders dark.
- TONEMAP/EXPOSURE clamp: up-angles into the bright caustic surface blow out to white. Add a soft tonemap
  (e.g. ACES/Reinhard) or clamp exposure when the camera faces the lit surface.

## NODE DESIGN — RESOLVED: cracked-pearl + coalesce/disperse surface modulator — 2026-06-16
(docs/prototypes/anamorph `cargo run --bin cracked -- "<text>"`) The synthesis that resolves legibility VS
obfuscation AND paragraph density. TWO layers:
  - INTERIOR FACETS: the depth-spread beam lattice, kept DIM + hue-shifted = a cracked-crystal body that is
    always lit and obscures from every angle (gorgeous streaked facets off-axis).
  - SURFACE MODULATOR: clean, sharp glyph coverage (legible even for full paragraphs — no white-out, no
    lattice noise) whose points COALESCE into the text at the reveal detent and DISPERSE back into the facet
    body off-axis. c = align^3 drives the lerp(scatter→home) + colour(body hue→white) + brightness.
KEY FINDING: brightness-gating ALONE can't hide coherent text (readable even when dim) → must RECONFIGURE
(scatter), not just fade. Verified: reveal = clean full paragraph; side = fully dispersed crystal orb, zero
readable text. Solves paragraphs (clean surface) + obfuscation (dispersal) + lit-from-all-angles (facets).
This SUPERSEDES the pure laser-array node look (which was illegible/white-out for paragraphs). Laser lattice
lives on as the INTERIOR facet generator.

## STAINED-GLASS REDONE (mosaic, fixed "christmas lights") + 4x4 VARIATION COLLAGES — 2026-06-16
WINDOWS FIX: the flat bright blue/gold panes read as cheap christmas lights. Rebuilt as a real leaded MOSAIC.
New shared helpers mesh3d::tri() (single facet) + mesh3d::stained_glass(center,right,up,w,h,cols,rows,palette,
seed): a dark LEADING backing + a grid of small glass TILES (rect body) + a fan of inset WEDGE tiles (the arch),
each tile a different shade picked from a multi-colour palette, with thin leading gaps and MODERATE, per-tile
VARIED glow (0.35..0.85) — backlit coloured glass, not a uniform blob. atlantis() now feeds a sea-BLUE palette
(navy/royal/sky/pale/teal + amber accent) and a temple-GOLD palette (bronze/gold/amber/pale + blue accent),
alternating per window; facet grid = segments × (2+branches). Verified: reads as leaded stained glass now.
COLLAGE TOOL: /tmp/collage.sh renders 16 frames across CONVERSATIONS (env variation: hall size, window/statue
counts — churn 136/42/23/18/…), ANGLES, and TIMES (LOGSCOPE_HOUR 7/9/12/16/18/20), then ImageMagick `montage
-tile 4x4` → atlantis_variation_4x4.png. Frames saved under docs/.../renders/collage/. Repeatable for any batch.
NEXT: (1) curate collage angles (a few framings landed on open water — bias toward eye-level + 3/4 hero angles);
(2) keep building components — regions BEHIND the doors (one space type + fly-through), STAIRS generator,
loop→rotunda; (3) tie region type to branch graph-shape; (4) refine statue silhouettes + kelp/coral hue.
## PROCEDURAL POSEIDON STATUES + SEA GARDEN (program cont'd) — 2026-06-16
Populated the hall with two of the vision's region building-blocks, both reusable generators in scene3d, seeded
from the conversation:
  - poseidon_statue(s, base, face, sc, seed): a MARBLE trident-wielder from primitives — pedestal + cap, flared
    robe skirt + torso, sphere head, two arms (one raised holding the trident), and a TRIDENT (staff + 3 prongs +
    crossbar). Pose/robe-length/which-arm vary with seed. Verified: clear marble figures with readable tridents.
  - sea_garden(s, center, radius, seed): scattered vegetation — swaying KELP strands (green, animated), branching
    CORAL clusters (warm), tentacled ANEMONES. MOSS material. Verified: green kelp + gold coral on the floor.
PLACEMENT in atlantis(): statues = (2 + seed%3 + r_hall/12).clamp(2..5) around the colonnade facing centre, each
with a Pillar collider (fly evades them); one sea-garden patch off to one side (radius ~0.52·r_hall), seeded.
These join last turn's DOORS (per branch) + LEADED STAINED GLASS. The vision's element library now has: doors,
leaded stained glass, marble statues, sea-garden vegetation, + shared helpers (panel, arch_window, leaded_arch).
NEXT: (1) regions BEHIND the doors — make a door an opening you can pass, generate a sub-space beyond it (start
with one type: sea-garden terrace or looping rotunda), thread the fly path through; (2) tie region TYPE to the
actual branch's graph shape (loop→rotunda, deep→stairs, long→terrace) — needs per-branch data into EnvParams or a
new region descriptor; (3) STAIRS generator for elevation; (4) refine statue forms (less blobby) + kelp/coral hue.
## LEADED STAINED-GLASS (wide arches, facets from EnvParams) + DOORS to regions (program START) — 2026-06-16
STAINED GLASS redo: new shared helper mesh3d::leaded_arch() — a WIDE glass arch (w 3.4 vs straight h 1.5 → the
rounded lunette dominates, "more arch than tall+arch") tinted glass→lo, divided into TILES by a grid of thin
metal CAMES + an outer frame + spring-line came + radial cames fanning across the arch. Facet density is
PROCEDURAL: cols = segments.clamp(2..5), rows = (1+branches).clamp(2..4). Blue (sea) / gold (temple) variants
alternate. Verified midday (palace_leaded_stainedglass.png).
DOORS — architectural-program START (my pick: the connector skeleton everything hangs off). Each BRANCH point
opens a DOORWAY around the hall: count = branches.clamp(1..6); a larger STONE arch surround (arch_window) set in
the wall + a glowing WATER threshold (the way through to the region beyond) + stone jambs + a collider. Spread
around the perimeter for now. Verified (palace_doors_to_regions.png).
NEXT (continue the program): (1) REGION PARTITION — split the graph into regions (mainline segments / branch
subtrees / loops) and assign each a space type; (2) place regions in 3D and route the door of each branch to its
region (door angle from the branch's actual node direction, not just even spread); (3) per-region seeded
generators — sea garden (kelp/coral/anemone), looping rotunda, staired descent, terrace via water-cave pool;
(4) thread the fly path through the doors; (5) marble Poseidon trident statues (primitive figure + trident) as
region landmarks. leaded_arch + arch_window + panel + collider pipeline are the reusable building blocks.
## ARCH-WINDOW FIXES · DAY-BRIGHTNESS · TIME-STAMPED RENDERS · FOOT CAPS · (mind-space roadmap) — 2026-06-16
FIXES:
  - WINDOWS: now WIDE (2.6) + TALL (3.2 straight + 1.3 arch ≈ 4.5) with a clear ROUNDED arch top, in a taller
    clerestory (5.0). Rendered as STAINED GLASS alternating BLUE (sea) and GOLD (temple), emissive 1.15 + slim
    stone mullions. Rounded crown reads clearly (esp. at night vs the dark drum).
  - DAY BRIGHTNESS confirmed: the "night shot" was REAL — the headless box's clock had advanced so TZ=Asia/
    Singapore was evening. Added LOGSCOPE_HOUR override to mesh3d::local_hour() for deterministic time. At
    hour 12 the hall is full daylight; hour 20 is dark/cool. Day cycle verified.
  - RENDER NAMING: filenames now stamp the forced hour (palace_h12_*, palace_h20_*).
  - PILLAR FEET: tube() is open-ended → added disc caps on the base feet AND capitals (the missing top face).
MIND-SPACE ROADMAP (user: "no doors to the procedural parts… take the graph shape+context and conform it to a
mind space" — work next). Mapping conversation GRAPH → spatial PROGRAM:
  - linear runs → halls/colonnades; BRANCHES (forks) → DOORS to side-wings; LOOPS (back-refs) → rotundas / ring
    corridors that return; deep threads → STAIRED descents (elevation modulation); long tangents → open spaces,
    TERRACES, SEA GARDENS; pruned/dead ends → sealed doors / submerged caves; token-heavy nodes → larger chambers.
  - elements to generate (seeded per region): water-cave pool-floor entrances to terraces; vegetation (kelp/
    coral/anemone generators) for sea gardens; stained glass blue/gold variants (STARTED); STAIRS for elevation;
    randomly-generated MARBLE POSEIDON-LIKE TRIDENT statues in varied poses/garments.
  - statue plan: marble figure from primitives — sphere head + tapered-tube torso + tube limbs (pose = limb
    angles from a seed) + draped cone garment + a trident (thin tube + 3 short prongs); light-stone material.
  - build path: (1) partition graph into REGIONS (segments/branches/loops) → space type each; (2) lay regions in
    3D, connectors = doors/corridors/stairs; (3) per-region seeded generator builds the space + colliders;
    (4) thread the fly path through the doors. Reuses EnvParams + shared mesh helpers (panel/arch_window) +
    collider pipeline already in place.
NOTE: arch top can be made even more prominent (lower the windows a touch / grow arch radius) if wanted.
## ARCH WINDOWS · EYE-LEVEL ORBS · BROKEN CAUSTICS · SMOOTH COLUMNS · LIGHT-BEAM CONNECTOR · BASIC REFLECTIONS — 2026-06-16
SHARED HELPERS (mesh3d): added panel(center,right,up,w,h) and arch_window(center,right,up,w,h) — a tall pane +
semicircular arch as one mesh, reusable by any theme.
ATLANTIS:
  - ARCH WINDOWS: replaced the ring of small portholes with a few TALL arch windows (h≈2.6 + arch ≈3.3 total,
    taller than a person), count = (cols/2).clamp(6..16) (procedural, scales with segments), each with slim
    stone mullions, in a taller clerestory. Glowing WATER.
  - SMOOTHER COLUMNS: shaft 12→26 segs, base/cap 12→22, + torus COLLAR mouldings at base & capital to soften
    the hard tube edges into a natural finish (fills the faceted-edge gaps).
  - LIGHT-BEAM CONNECTOR: Atlantis cords are now thin (r .024), near-straight (amp .05), bright (emissive 1.7,
    near-white-cyan) → read as beams of light caught by bloom. Mistfall keeps the soft WATER cord; others plain.
  - EYE-LEVEL ORBS: pos.y += 1.9 on Atlantis so the strand floats at walking head-height; the fly camera rides
    it through the hall at eye level instead of skimming the floor.
SHADER (gpuscene.rs, applies to ALL scenes):
  - CAUSTICS broken up: added value-noise vnoise(); caustic() now torn by two drifting noise fields (tear +
    grain) so the dapple is discontinuous/irregular, not a clean mesh.
  - BASIC REFLECTIONS/BOUNCE: a single analytic bounce toward the skylight — glossy surfaces catch a sharp
    sky-light reflection (tighter the glossier) + an indirect ground-bounce fill. Cheap reflection-trace stand-in.
  (prior: WATER warp + depth murk for air-inside/water-outside also shared.)
VERIFIED: palace_arch_windows.png (tall arches + smooth columns + lifted strand), palace_light_beams_eyelevel
.png (walking-height fly, bright beam cords, broken caustics).
DEFERRED (cross-scene): the SHADER gains (caustics, reflection/bounce, water warp) already apply to every
theme; remaining is per-theme GEOMETRY polish (smooth columns/collars, arch openings where apt) for Canopy/
Archive/Reading-Room/etc., and routing each theme's connector through the shared beam-vs-cord choice. Also keep
pushing EnvParams into window size/vegetation/terrain.
## UPPER BALCONY + WINDOWS · SKYLIGHT EMPHASIS · AIR-INSIDE/WATER-OUTSIDE · LIFELIKE WATER + VARIED CAUSTICS — 2026-06-16
ARCHITECTURE (scene3d::atlantis):
  - UPPER BALCONY: walkway annulus above the architrave (inside the colonnade) + railing torus + balusters
    (count from cols).
  - CLERESTORY drum above the balcony pierced by a ring of small WINDOWS (glowing WATER portholes, alternating
    sizes for rhythm) patterning around the upper level — count scales with cols (≈conversation segments).
  - SKYLIGHT EMPHASIS: dome now springs from the clerestory and narrows to a raised STONE cupola + a bigger,
    brighter skylight disc (emissive 2.3) + a glowing halo torus — the dramatic light crown.
RENDER (gpuscene.rs scene FS + chat.rs):
  - AIR INSIDE / WATER OUTSIDE: WATER material (layer 3 = dome, windows, outer wall, floor sheen) now gets a
    refraction WARP (animated normal wobble → shimmering reflections/highlights) and a distance MURK (mix to
    fog_col+caustic with depth) so the world OUTSIDE the glass reads as water; the dry AIR interior (stone) is
    left crisp and the Atlantis global fog was lowered 0.017→0.009 so the interior is clear.
  - CAUSTICS more lifelike: caustic() rewritten — 5-wave folded filament cores + a finer high-freq sparkle
    layer + a slow large-scale DENSITY field (bright pools & calm gaps), so the dapple is patchy/organic, not a
    uniform grid. Full on water surfaces; a gentler skylight dapple on dry above-facing stone (air interior).
  VERIFIED: palace_balcony_windows.png (window ring + balcony + varied floor caustics), palace_air_water_
  caustics.png (eye-level: crisp air columns vs water floor/crystal/windows, organic caustics).
NOTE/NEXT: the emphasized skylight DOME/cupola is best seen looking UP — the orbit cam (target=floor centroid)
can't easily look up; add a "gaze up" camera hook to showcase it. Render helper must NOT pass POS_ANGLE_INIT=
"0,0,0" with FLY (dist 0 → black); use FLY+FLY_DIST alone.
## ENVPARAMS→COLLIDERS + CAMERA BOUNDING-SPHERE EVASION — 2026-06-16
The procedural geometry and the camera now share ONE source of truth: the same EnvParams that build the hall
also emit COLLISION VOLUMES, and the fly camera carries a bounding sphere that pushes off them — fixing the
black frames where the path ran through the central plinth / a column.
  - scene3d::Collider { Pillar{x,z,r,y0,y1} (stay OUTSIDE), Bound{r} (stay INSIDE) } + thread-local COLLIDERS,
    env_colliders(), clear_colliders() (called at the top of build()), push_collider().
  - atlantis() emits: one Pillar per colonnade column (r 0.8, y_floor..y_cap), one Pillar for the central
    plinth+crystal (r 2.7, y_floor..+5.4), and a Bound at r_hall+1.9 (the perimeter wall). All derived from the
    same params/positions that build the meshes, so they scale with the conversation.
  - chat.rs fly block: after easing toward the path pose, compute the EYE (target + dist·(-cp·sy, sp, cp·cy)),
    test it against env_colliders with cam_r=0.85 — push out of any Pillar it intrudes, pull inside the Bound —
    then shift the WHOLE camera (target += push) so the look direction is preserved.
  VERIFIED: FLY≈0.45 and 0.50 (center of path, the ones that went black) now render clean interiors — camera
  sits among the columns with the plinth crystal pushed clear (camera_evades_plinth/columns.png).
NEXT/TUNING: a column can still graze the very edge of frame (bump cam_r or add a soft look-ahead probe so it
eases around solids a beat earlier); extend EnvParams to drive MORE architecture (window/arch openings, kelp
vegetation density, floor/terrain undulation, curve profiles) now that the params+collider pipeline is in.
## FONT lowercase+punct · CURSOR fix · PROCEDURAL hall from conversation — 2026-06-16
1) FONT: mesh3d::glyph5x7 no longer force-uppercases; added full a-z lowercase (with descenders) + punctuation
   ! ? , : ; ' " ( ) [ ] < > + = * / \ @ & % # $ _ — – . and · . Verified (pearl_font_lowercase.png:
   "Ship the pearl! Does it read? yes, clearly — Build r178.").
2) CURSOR: the egui TextEdit caret sat at the transparent edit's content origin (up/left of the cells). Now
   suppressed (ui.visuals_mut().text_cursor.stroke = NONE, selection fill dimmed) so only the pearl's OWN
   caret shimmer shows, aligned to the resonance cells.
3) PROCEDURAL LAYOUT FROM THE CONVERSATION: scene3d::EnvParams{nodes,branches,segments,seed} + set_env_params,
   folded into the env cache key (re-bakes when the conversation shape changes). atlantis() now GENERATES from
   it: r_hall grows with sqrt(nodes) [14..30], colonnade `cols` from topic-segments [12..34], `lanterns` from
   branch count, bubble count from hall size, and a per-conversation seed biases the stone/glass TINT (teal↔
   kelp-green). Fed from spatial_ui (n, segment count, fork count, content hash). Verified: large (136-node)
   conversation builds a big dense hall (procedural_large_hall.png) vs the 10-node conversation's tiny strand on
   open floor at the same camera (procedural_small_contrast.png) — header shows churn 136 vs 10.
KNOWN/NEXT: the bigger procedural hall's CENTRAL PLINTH can clip a center-of-path fly (FLY≈0.45 went black) —
offset the plinth off the conversation spine or route the path around it; small-hall overview wants dist scaled
to r_hall; mid-string insert in the pearl still reflows (slot-index keying). Parked extension: drive MORE of the
architecture from the runes (window sizes, curves, vegetation, hallway layout, terrain elevation).
## PHYSICAL PEARL MESSAGE BOX (temporally stable) — 2026-06-16
The composer is now a PEARL. New module pearl_input.rs (PearlInput::show): draws a rounded pearl body
(sheen rings + ambient speckle dust twinkle) and renders the typed text as glowing 5×7 resonance CELLS
(cyan + ~26% white star-cells) with a caret shimmer at the edit point — same visual language as the 3D pearls.
TEMPORAL STABILITY (the hard part): a persistent cell POOL keyed by (slot, glyph-x, glyph-y). Each frame we
DIFF desired-vs-pool: shared cells keep position+phase, removed cells ease target→0 and disperse out, new cells
coalesce in from a small offset; positions EASE toward home so edits reflow smoothly. So only the DELTA animates
— typing/backspace never re-flickers the whole field (mesh3d::glyph5x7 made pub for reuse).
INTEGRATION (chat.rs composer): pearl_input.show() draws the band, then a real egui TextEdit is ui.put() over
the SAME rect with Frame::NONE + TRANSPARENT text — so it still owns input, caret, selection and IME, and the
Plasma-Mobile focus fix still applies; the pearl cells are the only visible text.
PREVIEW: new bin src/bin/pearl-input-view.rs (auto-types a sample) → pearl_message_box.png. Builds clean under
gpu-scene. NOTE build the preview with --features gpu-scene (bare `gui` pulls rfd→gdk-pixbuf-sys which needs the
GTK system lib; gpu-scene⊃gui is the known-good set here).
V1 LIMITATIONS / NEXT: the invisible TextEdit's internal layout differs from the cell layout, so click-to-place
caret + selection are approximate (typing/append/backspace are exact); mid-string insert reflows rather than
holding perfectly (slot-index keying) — could switch to content-anchored keys later. Optional: scroll-up overflow,
send-burst animation (cells fly toward the new node), and matching the pearl tint to the active theme.
## MOBILE INPUT FIX · RUNE HANDOFF · TIME-OF-DAY · (msgbox plan) — 2026-06-16
1) PLASMA MOBILE one-char input bug: targeted fix in the composer — on the IME-commit-drops-focus signature
   (resp.changed() THIS frame but !has_focus() the SAME frame) we re-assert focus + force Insert mode. Inert on
   desktop (focus retained) and when navigating away (no text change). Best-effort; cannot test on-device —
   hypothesis: Wayland+Maliit re-targets the IME on the empty→non-empty layout change after char 1.
2) QUIET RUNES ON APPROACH + 2D LABEL HANDOFF: rune glow curve INVERTED — was brightest when nearest
   ((1-frac)); now `approach=((frac-0.05)/0.16)` so runes are strongest mid-approach and QUIET to nothing up
   close (skip if <0.03), handing off to the 3D resonance text. 2D caption now lives in a mid band
   (fade-in ~t .12-.22, fade-out by ~.42) so it too hands off to the pearl's own text as t→0. Verified: near
   pearls show 3D text, runes/captions only on mid-distance pearls.
3) TIME-OF-DAY SYNC: sun_dir_3d now uses chrono::Local (real local clock, added chrono dep). New mesh3d::
   sun_clock()->(strength,warmth,ambient): peaks midday, ~0 at night; warm low sun at dawn/dusk, cool at night.
   Folded into the GPU uniforms — sun_col lerped to a warm/cool tod target, exposure*(0.62+0.5*strength),
   ambient*tod_amb. Verified: midday bright/high-key vs night dim/cool (palace_midday vs
   palace_night_texthandoff).
DEFERRED (user said later) — PHYSICAL PEARL MESSAGE BOX with input temporal stability: the composer becomes a
pearl whose text is rendered as resonance cells. KEY REQUIREMENT: NOT a basic recompute per keystroke. Plan: a
persistent cell POOL keyed by (char_index, glyph_pixel); on each edit, DIFF against the previous text — shared
cells keep their position+twinkle phase, removed cells fade/disperse out, added cells coalesce in, with a
caret/“hot” shimmer at the edit point. That gives temporal stability (cells persist, only the delta animates)
instead of the whole field flickering. Wire as an egui widget over the real TextEdit (keep the TextEdit for IME
+ the mobile fix; draw the pearl cells from its buffer).
ALSO PARKED (big): RUNES→MAP procedural theming — use node/identifier hashes to drive texture colour variation,
Atlantis architecture pattern, curve/window/vegetation/hallway/terrain-elevation variation (the user's idea).
## LABELS SMALLER+OCCLUSION DIM · PEARL STARS · LANTERNS · SMOOTH CAMERA — 2026-06-16
Driver CONFIRMED: user reports LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe fixes the magenta → it is the
hw GL→Zink→Honeykrisp path swizzling; software path is the workaround (or MESA_LOADER_DRIVER_OVERRIDE=asahi).
LABELS: label_cells scale 0.045→0.034 (smaller). Added OCCLUSION DIMMING in spatial_ui: for each labeled near
pearl, count NEARER proj nodes within ~78px screen and lit/=1+1.5*occ → a label behind another fades so the
front one reads (declutters stacked labels along the strand). Verified at close range (BUILD/OK/FIX resolve,
smaller, back ones dim).
PEARLS: speckle_cells now burns ~28% of cells white-hot + slightly larger (stars) among the cyan → the
starry-speckle look the user liked.
MAP: warm hanging LANTERNS below the architrave (cord + emissive sphere, gentle bob) for ambient counter-light
+ bloom anchors; (perimeter wall + moat from last pass).
CAMERA SMOOTH: fly path is now Catmull-Rom through the 4 surrounding nodes (smooth curve, no polyline kinks);
look point sampled a touch ahead on the same spline; and the persistent orbit is EASED toward the desired
pose each frame (target lerp + shortest-angle yaw + pitch/dist lerp, k=0.12) → smooth glide that converges in
the headless window and glides continuously under LOGSCOPE_FLY_AUTO. Renders: palace_hall_overview (colonnade
+ plinth + wall + strand), pearl_text_smaller (close text resolve).
STILL PENDING: quiet the 2D rune/label overlay for near nodes (still doubles with the 3D text); tonemap clamp
(skylight over-exposes on up-angles); optional continuous in-app fly toggle UI.
## FINISHED THE BALL (text-on-approach) + MAP BUILD-OUT + driver note — 2026-06-16
THE BALL: pearls now resolve their turn label into glowing TEXT cells as the camera approaches (the resonance
reveal, in-engine). Added mesh3d::glyph5x7 (compact 5x7 font) + mesh3d::label_cells(center,right,up,
toward_cam,text,scale,t,col,lit,scroll): rasterizes nd.preview into emissive camera-facing cells floated in
front of the pearl, brightening with `lit` (near>0.80 → resolves), gentle scroll-up drift, twinkle. Wired in
spatial_ui for the nearest pearls using view_cam yaw/pitch to build the billboard basis (right=(cyaw,0,syaw),
up=(syaw*sp,cp,-cyaw*sp), toward_cam=-viewDir). VERIFIED: render shows "BUILD…", "SPAWN…" legible in front of
near pearls (pearl_text_on_approach.png).
MAP BUILD-OUT (scene3d::atlantis): added an outer perimeter wall_ring (the back wall caustics wash across) +
a sunken annulus moat-walkway between columns and wall — the hall now reads as an enclosed place.
DRIVER (Asahi Honeykrisp magenta-for-blue): almost certainly a young-driver swizzle/format issue, not the app
(scene-view is eframe Glow→ likely Zink→Honeykrisp). Confirm via LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=
llvmpipe (blue correct there ⇒ hw GL path); try MESA_LOADER_DRIVER_OVERRIDE=asahi vs zink. Not app-fixable
cleanly unless it's a shader-compile fallback (would repro under llvmpipe).
NEXT: quiet the 2D rune/label overlay for near nodes (currently doubles with the 3D text); tonemap clamp in
glow_scene (skylight over-exposes); tune near-threshold + camera path to avoid column near-clips.
## CENTERPIECE BUILD-OUT + FORWARD FLY CAMERA — 2026-06-16
User: do camera work + build expansion (the mind-palace centerpiece had no glass/stone yet).
BUILD EXPANSION (scene3d::atlantis): replaced the small 4u dome+drum with a glass-and-stone HALL —
  - 18 STONE columns (shaft+base+capital) in a ring carrying a stone architrave ring;
  - a great GLASS dome (dome_cap, WATER mat + emissive) narrowing to a STONE oculus + bright skylight disc
    (bloom/god-ray source);
  - a central STONE plinth + step crowned by a glowing faceted GLASS crystal (stacked rocks, WATER mat,
    emissive, gentle bob) — the mind-palace heart;
  - gallery ring + bubbles retained, sized to the larger hall (r_hall=16).
CAMERA WORK (chat.rs fly block): the fly-through now FACES its direction of travel — yaw/pitch derived from
the path tangent (pitch=asin(-ty)+downbias, yaw=atan2(tx,-tz), matching OrbitCam::project), looking a couple
nodes ahead so we fly INTO the strand. LOGSCOPE_FLY=s manual; LOGSCOPE_FLY_AUTO=speed animates s from the
clock for a continuous glide in the live app. Builds clean (~37s). Renders: palace_hall_columns (colonnade +
caustic floor close up), palace_hall_flythrough (forward glide down the pearl strand, hall around).
KNOWN: looking toward the skylight over-exposes (tonemap clamp still pending in glow_scene); pearl message
TEXT (resonance-on-approach) still the next wiring step; could lift camera slightly to clear column near-clips.
## WIRED INTO logscope-egui — pearl speckle + fly-through camera (first real result) — 2026-06-16
Pivot from HTML demos to the live Rust scene (user: use pearl-reader, wire into logscope-egui, move through the
mind palace via smooth camera). DONE this pass:
  - mesh3d::speckle_cells(center,r,seed,t,col,n,lit): low-poly EMISSIVE twinkling cells (the pearl-reader
    starry speckle / "bacteria") caught by the bloom pass; lit = resonance brightness.
  - tabs/chat.rs spatial_ui: inject speckle into NEAR pearls only (LOD by projected depth, count & brightness
    grow on approach = resonance) for the Atlantis theme.
  - FLY-THROUGH camera: LOGSCOPE_FLY=s (0..1) travels orbit.target ALONG the conversation path (id order) and
    sits inside the palace (LOGSCOPE_FLY_DIST, default 9), spatial_auto off — so we move THROUGH the palace,
    not orbit it. yaw/pitch still from POS_ANGLE_INIT.
  - Builds clean (gpu-scene, ~34s). Rendered fly=0.30 and 0.55 (renders/palace_flythrough_a/b.png): camera
    travels the strand, near pearls alive with twinkling speckle inside the Atlantis dome.
NEXT (the real pearl reader): (1) TEXT cells — place each message's glyph points as resonant cells that
resolve to legible text on approach (LOD-as-approach), not just speckle; (2) the starry "stars fade into text"
look (the bit the user loved); (3) continuous animated fly path in the live app (not just headless frames) +
the text-scroll-up; (4) quiet the 2D rune/label overlay while flying so the 3D pearls read clean.
## BEAM-READER-PHYSICAL — light-physics ruleset guides beams into the bloom — 2026-06-16
beam-reader-physical.html. User: beam reader is the best line; wants tighter lines, more line breathing, input
beams with play from a flickering source, and a more PHYSICALLY GROUNDED fixed ruleset of light properties
that LITERALLY GUIDES the beam into the bloomy text. Model: a flickering light SOURCE emits thin beam
particles into the pearl; a GUIDANCE FIELD derived from each block's glyph (blurred coverage -> gradient =
refraction toward strokes; tangent = flow ALONG strokes) bends each beam; damping settles them onto the
letters where they pile up and BLOOM (white core on-stroke), with chromatic DISPERSION (R/G/B refract
differently), boundary REFLECTION (bounce inside), and ABSORPTION (life decay/respawn). Soft frame-fade =
glow trails. Flicker gives the input beams play + breathes the line thickness/refraction. Tight 1px-ish lines
that widen slightly only where blooming. Scroll/drag changes the active block -> beams REROUTE and re-bloom
into the new text (physical transition, no gated swap). Levers: refract, flow, flicker, breathe, thickness,
dispersion, absorb. Named version for A/B alongside volume-reader-2 + pearl-reader. Evaluate the bloom
legibility + whether the flow-into-letters reads as light being guided.
## VERSIONING DEMOS FOR A/B — 2026-06-16
User wants to A/B the reveal designs by loading named files (stop overwriting one). Preserving distinct named
versions in outputs + docs/prototypes/anamorph:
  - volume-reader-2.html : unified interior, blocks at equator longitudes, coherent rotation (the one whose
    rotation felt right; legibility was the open issue).
  - pearl-reader.html    : resonance pearl (latching "bacteria" cells driven to resonance by entry beams);
    "interesting but not quite it."
Going forward: each new reveal idea gets its own stable filename so they accumulate for comparison.
## RESONANCE PEARL — latching "bacteria" cells driven to resonance by entry beams — 2026-06-16
pearl-reader.html. Rejected the passing/sweep reveal. NEW (user): little "bacteria" cells in the pearl that
LIGHT UP AND STAY ON; a slowly modulating set of ENTRY BEAMS that, at a block's RESONANCE, drive its cells to
light together and HOLD (stability), not pass by. Model: each cell has a resonant tune phi; text cells sit at
upright glyph positions (legible, no transform) sharing their block's phi; background cells random pos+phi
(the living pearl). A drive value "tune" (entry-beam modulation) slowly drifts + can be steered; near a block
it LOCKS/dwells (resonance + stability). Excitation E=exp(-(tune-phi)^2/2w^2); cell L = latch with slow decay
(L=L*persistence + E*rise) => cells light up and STAY ON, whiten when fully stable -> crisp upright text.
Off-resonance the pearl twinkles (alive). Levers: tune, drift, lock, persistence, width, glow. This holds the
text as a stable lit state instead of sweeping it. Evaluate: does the lock-on read as resonance + hold, and is
the lit text legible/stable?
## UPRIGHT TEXT + RHYTHMIC BEAM SWEEP (corrected intent) — 2026-06-16
volume-reader.html v4. Correction: "add axis tilt to separate" meant TILT SELECTS/SEPARATES blocks while text
stays UPRIGHT left-to-right — NOT bake text to an oblique reveal angle (that made it unalignable + depth
killed it via a razor-thin oblique window). Also user idea: a "pulsating/rhythmic beam transition" for
legibility. NEW: blocks are upright text stacked vertically; tilt/scroll moves between them (centre = active,
separation by gaussian falloff). The active block CONDENSES out of the swirling interior cloud via a rhythmic
LEFT-TO-RIGHT beam sweep that "prints" the text, holds, dissolves, repeats. Home = upright front plane
(legible regardless of swirl); cloud = swirling depth-spread scatter (depth lever only stirs the cloud, never
the readable text -> fixes "depth destroys text"). Legibility now comes from the rhythm + upright home, not a
precise angle. Levers: rhythm, hold, tilt/scroll, swirl, depth, glow. Keeps ONE interior. Evaluate: text
legible during print/hold? rhythm pleasant? tilt separation clean?
## 2D CONSTELLATION + AXIS TILT — blocks separated over yaw AND pitch — 2026-06-16
volume-reader.html v3. Blocks now sit at a 2D constellation of reveal axes (yaw,pitch) within the ONE
interior; convergence = max(0, dot(camdir(yaw,pitch), blockAxis))^POW, so a block resolves only when BOTH the
yaw and the tilt match it. Tilt (axis pitch) genuinely SEPARATES the blocks (e.g. top row vs bottom row), so
fewer foreign points cross any given block's reveal -> cleaner text. Points baked via invrot at each block's
(yaw,pitch); depth z still gives volume/parallax. Same continuous interior (no swap); steer yaw+tilt to each
block. This is the pages.rs 2D-detent model brought into the volumetric interior. Knobs: reveal power (tighter
= more separation), depth (parallax/volume; rebuilds cloud on change), ambient.
## UNIFIED INTERIOR — one cloud, all blocks at once (no gated swap) — 2026-06-16
volume-reader.html v2. Feedback: section-to-section must NOT be a gated switch that changes the interior —
"they have to be the same interior." FIX: build ONE point cloud containing EVERY text block simultaneously,
each point baked into the volume at ITS block's reveal longitude (phi). All points are ALWAYS rendered; a
point converges onto its letter only along its own reveal axis (conv=max(0,cos(totYaw+phi))^POW), so ROTATION
selects which block resolves while the others remain the surrounding cloud. The interior never switches —
same crystal, read from different angles (this is the 5-paras-on-one-sphere principle restored, vs the
per-section swap). Sprite-based glow blobs (perf), atmospheric depth (warm/near, cool/far), constant interior
threads, chromatic fringe converging to white. 'ambient' lever = how present the non-resolving blocks/cloud
are between angles. Acceptance: rotating moves smoothly block->cloud->block with a single continuous interior.
## VOLUMETRIC DEPTH-CONVERGENCE — text from the interior depth (utilize the inside) — 2026-06-16
volume-reader.html. Feedback on flow-grain: couldn't read text AND it wasted the sphere (surface-only; the
INNER section should be interesting/utilized). NEW: text lives in the sphere's DEPTH. Each glyph point keeps
its (x,y)=letter but gets a free z across the sphere -> along the reveal sightline the depths project onto the
letters (legible text); off-axis they PARALLAX-SHEAR apart (you see INTO the volume, depths sliding through
each other = the interesting interior). Rendered as a translucent volume you look into: atmospheric depth
(near=big/bright/warm-white, far=small/dim/cool-blue), additive glow blobs, dim interior structure threads for
depth cue, chromatic dispersion fringe near convergence fusing to white at it. Continuous spin + tilt; nearest
reveal axis = active section, converges as it faces the eye. Levers: spin, tilt, depth(parallax spread),
reveal(power), glow, structure. This re-uses the depth/volume (vs surface grain) and brings back the
directional convergence the user liked. Evaluate legibility-at-convergence + whether the interior parallax
reads as "looking into" an interesting body.
## NEW DIRECTION — FLOW-GRAIN emergence (pareidolia, not a region) — 2026-06-16
grain-reader.html. Feedback: the clustered-beam demo disconnected because the text was a denser/brighter
REGION dropped in (resolution jump, "no hint why"); the goal is pareidolia — "like seeing a shape in a cloud,
a pattern emerges when looked at the right way." NEW PRINCIPLE: the grain is UNIFORM density everywhere; only
its DIRECTION carries the text. A Fibonacci-sphere fur of short beams; on the letters the grain aligns to the
stroke-tangent flow (gradient-perp of the glyph field), off the letters it is incoherent. At the reveal angle
the aligned flow reads as letters (brushed-metal/grass-combed-into-a-shape); off-angle = uniform noise. NO
density jump. Revealed by CONTINUOUS ROTATION (user idea: spin at rad/sec + tilt axis to bring next sections);
nearest section to front is active; letters surface as it turns to face you and dissolve back into grain.
Levers: spin speed, tilt axis, focus (subtly lengthens/whitens coherent grain — pareidolia tuning, 0=pure),
length, density, hue. ALTERNATES noted if this isn't it: moiré (two gratings beat into text), POV-spin.
This is an exploration away from the region-based node; evaluate whether emergent-flow reads as text.
## BEAMS-ARE-THE-TEXT — clustered text-beams glowing/diffusing into the body — 2026-06-16
beam-reader.html rebuilt per feedback ("beams ... creating the illusion rather than just noise around it ...
stronger beams placed where the text will be, glow and diffuse into the rest ... peek into the blurry pearl
reveals the inner shape is the text ... clustering and projection directional mapping").
CHANGE: dropped the glyph-image glow billboard. Now an ALIGNMENT/CLUSTERING pass bins each paragraph's stroke
points into strong cluster-nodes that sit exactly ON the letters, and connects adjacent clusters into BEAMS
that trace the letterforms -> the beams ARE the text. They are rendered STRONG (bright core) + a wide soft
DIFFUSE glow that blooms into the dimmer body material; with per-channel chromatic fringe that separates
off-axis and fuses to a white legible core at the projection angle (directional caustic from prior turn).
Body = dim random beams with directional sparkle (the pearl interior the text diffuses into) + translucent
pearl fill so you "peek in". Legibility now emerges from the clustered beams + their bloom, not an overlay.
NEXT: stronger clustering (k-means / skeletonize for cleaner letter tracing); input/output directional
routing of the beam sets (ACO from edge-in to edge-out) so the beams visibly flow through the letters; tune
glow spread vs legibility; carry into Rust as instanced glowing line mesh on clustered glyph nodes.
## DIRECTIONAL-SCATTER REVEAL — material refracts the beam into the eye (the "wow") — 2026-06-16
beam-reader.html upgraded from an alpha-fade to a real optical model (user: the material must "diffuse the
light into the eye to show directional text like a projection ... how the physical makeup interacts with the
light beam scattering through it ... bridges in a direction certain hues").
MODEL: explicit source beam Ldir + eye Vdir; half-vector H=norm(L+V). Each page's micro-facets have a normal
that EQUALS H at that page's detent, so rotating the sphere swings the facets through H and the page reflects
the beam straight at the eye -> the text CAUSTIC projects. CHROMATIC DISPERSION: R/G/B sampled against
slightly offset half-vectors -> each wavelength peaks at a slightly different rotation, so the hues SEPARATE
as you rotate off the projection angle and FUSE into white legible text exactly at it (the "hues bridging in a
direction"). Body beams sparkle directionally too (glint when their radial facet aligns with H) so the whole
crystal moves the light like a real gem. The SOURCE BEAM is drawn entering along Ldir so the physical cause is
on-screen. Legibility now comes from the converged white core at the projection angle.
This replaces the diffuse alpha-glow. PORT NOTE for Rust gpu-scene: this is Blinn-style specular with
per-channel dispersion on the node facets, lit by the Atlantis oculus/god-ray as the source — i.e. the reveal
is real shading, not a fade. Carry H-alignment + chromatic offset into the node shader.
## BEAM-BASED INTERACTIVE PROTOTYPE — white/split/rebound/blocked as levers (testable) — 2026-06-16
docs/prototypes/anamorph/beam-reader.html (+ outputs). Replaces the moving-DOTS surface with BEAMS as the
primitive (user: "the beams ... are the real levers to pull"). Volume node lattice + kNN edges = light beams.
Each edge has a STATE:
  - WHITE: edges whose path lands on a glyph stroke at the active page's detent light white -> the text.
    Rotating the page into the beam aligns them (coalescence = max(0,cos(yaw+phi))^revealPow); rotating away
    drops them back to body state. (No dot motion.)
  - SPLIT: prism dispersion, R/G/B offset copies.
  - REBOUND: reflected, bends at a midpoint into a 2-segment path.
  - BLOCKED: occluded, faint + broken (middle dropped).
Live SLIDERS for white/split/rebound/blocked intensities + reveal sharpness = the levers, pullable in-app.
Scroll/drag rotates paragraphs through the god-ray beam; conveyor advances spheres. SUPERSEDES sphere-reader
(dots). Next tune: beam density + on-stroke selection for white-text legibility; per-state animation as beams
rotate in/out of the white state; carry the four states into the Rust gpu-scene cord renderer.
## INTERACTIVE PROTOTYPE — scroll-rotate-reveal + N-sphere conveyor (testable NOW) — 2026-06-16
docs/prototypes/anamorph/sphere-reader.html (also shipped to outputs). Browser/Canvas port of the validated
model so the ROTATION-REVEAL interaction can be tested live (headless Rust stills can't be "felt"):
  - message box REPLACED by a cracked-pearl sphere: point-cloud body + connecting light beams, hue-shifted.
  - SCROLL/drag/arrows -> sphere rotation; each paragraph sits at a longitude DETENT; rotating it into the
    fixed LIGHT BEAM coalesces it (coh=max(0,axis.viewer)^COALESCE_POW) from the crystal into clean text,
    disperses past it. Detent-glide easing on scroll.
  - N SPHERES in a conveyor (the conversation): active sphere centered/full + spinning through its pages;
    neighbours smaller/dim (LOD). Scrolling past a message's pages slides the next sphere into the beam.
Use it to tune BEFORE the Rust build: reveal-window width (COALESCE_POW), detent spacing feel, conveyor
advance rhythm, coalesced-text legibility at size, breathing amount.
PORT TO RUST/gpu-scene (production, next): replace chat message rendering with this node; map app SCROLL to
(yaw,pitch) + active-sphere advance; beams -> 3D line mesh (cord renderer); text glyph points from embedded
font (fontdue+include_bytes); coalesce/disperse computed CPU-side per frame -> mesh emissive/position; LOD by
distance/focus; the fixed beam = the Atlantis oculus/god-ray as the reveal light. Acceptance: scroll the chat
-> each message-sphere rotates its paragraphs through the beam and resolves to readable text, disperses off.
## 5 PARAGRAPHS ON ONE SPHERE — validated — 2026-06-16
(docs/prototypes/anamorph `cargo run --bin pages`) Five full paragraphs coexist on a single node as a
CONSTELLATION of reveal axes (center + 4 ring points spread across BOTH yaw and pitch). clarity power raised
to ^7 so only the nearest detent resolves (no bleed between neighbours). Verified: each detent surfaces its
own clean 4-line paragraph; the overview (off every detent) = one lit chromatic crystal, nothing readable
(all 5 dispersed). 5 pages = ~52k surface pts. Scales to many paragraphs via a Fibonacci/spiral set of axes;
detent spacing vs clarity-power is the packing knob (closer axes need higher power / risk bleed).
## PER-PAGE TEXT LENGTH — tunable (px + wrap) — 2026-06-16
Bumped each page from ~1 sentence to ~4 lines / ~15 words; reads cleanly (surface modulator, not the lattice,
so no white-out). Limit: more lines → smaller glyphs → legibility falls off as the block fills the disk.
~4 lines is the comfortable ceiling per detent. Beyond that, prefer MORE PAGES (more detents) over a denser
page. The two knobs trade off: text-per-detent (px/wrap) vs number-of-detents; paginated rotation scales
total text either way. (docs/prototypes/anamorph `cargo run --bin pages`, pages_long_*.png)

## PAGINATED-BY-ROTATION — scroll→rotate, 2D clarity detents on the sphere (validated) — 2026-06-16
(docs/prototypes/anamorph `cargo run --bin pages -- "p1 | p2 | p3 | p4"`) Generalizes scroll-detents to BOTH
spherical axes. A long message is split into PAGES; each page gets a distinct reveal axis (yaw_i,pitch_i) on
the sphere, its surface text built upright in that camera plane (via inverse-rotation of screen coords).
Scroll → rotation over (yaw,pitch). Clarity field over the 2 angular axes: clarity(yaw,pitch)=
max_i dot(camdir(yaw,pitch), axis_i)^k; DETENTS = the page axes (local maxima). The minima/relaxation search
snaps the rotation to the nearest page detent → that page COALESCES into clean upright text while all others
DISPERSE into the cracked-facet crystal. Verified: page1 axis="Build the file watcher.", between=nothing
readable, page2 axis="Track the byte offset." — distinct pages at distinct rotations of ONE node.
So a long response = a CONSTELLATION of reveal axes on the node's sphere; reading = rotating (scroll) between
detents. Pages can be laid on a Fibonacci/spiral set of axes for many pages. The cinematic camera + scroll
both feed (yaw,pitch); detent snapping gives the page-turn feel. Ties together: cracked-pearl body, surface
coalesce/disperse, LOD-as-approach, and now multi-page navigation — all one parameterization.
TODO for live: distribute N page axes (Fibonacci), clarity-field snap (spring to nearest detent), animate
coalesce/disperse on page turn, couple to scroll + orbit input.

## TEMPORAL RECONFIGURATION ON SCROLL — detent-bound clarity search (design, pairs with above)
The coalesce/disperse parameter c (above) is driven by VIEW ANGLE for short labels; for LONG messages drive c
(and which chunk is "home") by SCROLL. Each readable chunk has a DETENT = a local maximum of clarity (full
coalescence). Scrolling performs a detent-bound local-minima search on an illegibility cost and SNAPS to the
nearest clarity optimum (spring/detent feel), assembling that chunk into clean text while neighbours stay
dispersed in the crystal. Reveal axis == scroll detent == clarity optimum (one concept). Implements the
paragraph LOD: never show all lines at once — scroll surfaces each chunk at its detent (ties to DoF
focus-slice). TODO: define the clarity cost, precompute detents per chunk, spring-snap scroll, animate
coalesce/disperse transitions (this is also the "breathing/relaxation" motion).

## PARAGRAPH STRESS TEST of the laser array — finding — 2026-06-16
(`cargo run --bin laser -- "<paragraph>"` now wraps to lines, square 150x150 lattice.) An 8-line paragraph =
~20k text nodes, ~82k beams, ~26k on-stroke. REVEAL: structure + line rhythm read, several words legible, but
per-word legibility is at the edge (lattice triangulation + thickened detection = scratchy small letters).
OBFUSCATION: works (turn → unreadable smear). PROBLEM: ~26k bright on-stroke beams WHITE OUT the sphere
off-axis — it becomes a white blob, losing the lit-colored-orb read. Confirms the density wall.
CONCLUSION: laser lattice is the node look for SHORT text (label / key phrase / first line) — clean reveal +
true obfuscation + lit orb. FULL PARAGRAPHS must NOT be brute-forced as a lattice. Use LOD-AS-APPROACH:
node shows a short label/first line as the clean lattice; on deep focus, EXPAND with optical grow/focus
(caustic+bloom) + DoF FOCUS-SLICE to reveal the paragraph LINE-BY-LINE, and run the ACO legibility pass to
THIN the on-stroke beam set (sharper letters, no white-out). Tie to the substring cache so expansion is cheap.

## NODE LOOK (FINAL) — laser-splitting array: depth-spread beam lattice, obfuscated off-axis — 2026-06-16
(docs/prototypes/anamorph `cargo run --bin laser -- "WORD"`) THE look. A 3D lattice of light BEAMS (lines
connecting nodes) filling a sphere:
  - TEXT NODES sit at glyph (x,y) in the reveal plane but at RANDOM FULL DEPTH (±sqrt(R^2-rho^2)) → on-stroke
    lattice edges align into a near-grid spelling the word ONLY from the reveal axis; any rotation scatters
    them by parallax → genuinely OBSCURED (verified: reveal=BUILD, 31deg=smear, 69deg=chaos). This depth
    spread is the obfuscation the thin-plane versions lacked.
  - LIT FROM ALL ANGLES: off-stroke lattice edges + body kNN beams glow dim, hue-shifted by height
    (yellow→orange→magenta) so the sphere reads as a lit orb from every angle even when text is hidden.
  - LASER SPLITS: occasional colored beams on the strokes (the sketch's green/red/yellow/cyan); beams fan
    from shared lattice nodes. External GOD-RAY shafts cross the sphere (sketch's hatching).
  - LIGHT BEAMS = lines connecting points (the connective array), doubling as the flow/edge representation.
POLISH: reveal letters are a touch dense/noisy (full lattice triangulation over strokes) — OPTIMIZE the
on-stroke beam SUBSET (ACO / legibility objective, the "optimized to reveal a near-grid") for cleaner letters
+ widen reveal tolerance slightly without losing obfuscation; tie hue motion to caustics; breathing/relaxation.
This SUPERSEDES the flat-mid-plane cloud (which stayed readable off-axis). The depth-spread laser lattice is
the node representation to port to the live Atlantis gpu-scene (beams → 3D line mesh; reveal axis from edge
vectors; LOD-as-approach grows the lattice in on focus).

## NODE LOOK (CORRECTED) — point-cloud sphere + flat mid-plane text + occluder knobs — 2026-06-16
NOT a wireframe-on-sphere (that was a misread). The node is the POINT-CLOUD sphere (decoy points = body),
with the flat TEXT living in the MID-PLANE through the middle of the cloud (the anamorphic glyph plane).
(docs/prototypes/anamorph `cargo run --bin cloud -- "WORD"`)
  - BODY: dim point cloud filling the ball, HUE-SHIFTED by position (blues/teals → greens/reds) — a soft
    chromatic nebula that reads as a sphere.
  - TEXT: WHITE points in the thin mid-plane slab, intensified along the BEAM TRAJECTORY routed input→output
    (L→R gradient stands in for the edge-routed beam sweeping/whitening line-shaped stroke clusters).
  - OCCLUDER KNOBS so the flat text appears cleanly against body + scene background: body points that are in
    FRONT of the mid-plane AND over a stroke are (a) dimmed, (b) hue-pushed, and (c) BOUNCED perpendicular to
    the camera (slide out of the text footprint). This is the "hide the regions the text needs off" mechanism.
Verified: reveal reads as colored point-cloud sphere with crisp white BUILD through the middle.
POLISH: off-axis the dense flat text plane collapses to a bright slab → fade white harder off-axis
(align^4) and/or denser body so the orb dominates from the side; make the L→R beam an actual routed/animated
sweep that intensifies line-clusters (tie to edge input/output ports); breathing + relaxation per frame.
RENDERING PLAN (current/correct): NODE = point-cloud sphere (hue body + flat white mid-plane text, occluders
bounced); reveal axis from edge vectors; beam routing intensifies the text; flow cords = beams at edge ports;
LOD-as-approach grows text in on focus. (The beam-lattice + sphere-wireframe were stepping stones; the cloud
is the target look.)

## BEAM-NETWORK TEXT — validated; chosen primitive for dense/focused text — 2026-06-16
(docs/prototypes/anamorph `cargo run --bin beams -- "WORD"`) Instead of points on/off, render text as the
bright subset of a BEAM NETWORK: a random-walk × A* (ant-colony) traversal connects input→output edges across
a node lattice; deposit ∝ glyph alignment, so successive walks concentrate brightness onto letter-tracing
beams. Dim web = the flow connections; bright beams = the text. Result for "BUILD": 193 active beams
(~1.8% of edges), 100% of bright beams on-stroke, fully legible — line segments not dots, ~10-100× fewer
primitives than per-pixel. Guided not emergent (glyph is the explicit objective — the fix for the failed
transport-field approach). UNIFIES text + flow: the web IS the inbound/outbound edge connections.
Knobs: grid resolution (legibility vs beam count; regular grid → faint low-poly strokes, jitter/finer = smoother),
alignment threshold, ACO iters/ants, evaporation. CACHEABLE per token via the substring cache (pay ACO once).
"Laser splitting": ants fork at nodes for coverage. This is the fiber-optic/laser aesthetic wanted throughout.
NEXT for this primitive: (a) paragraph at a finer grid (still ≪ per-pixel) + cache; (b) render beams as the
glowing cords in 3D (beams already ARE line segments → map straight onto the cord/strand renderer); (c) make
the unfocused node a pearl, the focused node resolve into its beam-lattice text (LOD-as-approach).
RENDERING PLAN UPDATE: focused-node text = beam lattice (this); ambient = pearl + identicon; flow cords reuse
the same beam primitive. Supersedes the point-cloud glyph layer for legible/dense text.

## DENSITY — paragraphs / full LLM responses + "grow text from fewer particles" (findings) — 2026-06-16
Experiment (docs/prototypes/anamorph `cargo run --bin paragraph`): a paragraph = ~35k stroke pixels at
per-pixel density → stops being a pearl cluster, is just solid text, unaffordable in 3D. A single optical
SCATTER/BLOOM pass let ~9x FEWER particles (3.9k) coalesce into legible text, with a nicer beaded look.
Optical ways to grow text from fewer particles (best→deepest):
  1. BLOOM/light-scatter (demoed): cheap, ~9x, reuses the scene bloom pass; beads-joined-by-glow aesthetic.
  2. CAUSTIC FOCUSING (most on-theme): refraction focuses sparse emitters into THIN SHARP stroke-lines —
     sharper than emitter spacing, beats splat-merge. Atlantis already has caustics → reuse.
  3. DoF FOCUS-SLICE + depth stacking: stack lines at stepped depths, only focused plane sharp; scrub focus
     to read a long response line-by-line.
  4. HOLOGRAPHIC/diffraction (CGH): phase pattern reconstructs full-res image under light — decouples text
     resolution from particle count entirely. Deepest; needs FFT phase + reconstruction shader.
  5. METABALL/SDF merge: sparse particles fuse into smooth continuous strokes.
RECOMMENDATION: bloom-grow + caustic focusing + LOD-AS-APPROACH — far node = 1 pearl / first line; as the
cinematic camera nears, particle count + bloom GROW and the paragraph resolves into being (and dissolves on
leaving). The reveal IS the density solution. DoF focus-slice drives reading down a long response.

## REFRACTIVE PEARL ARRAY — prototyped (pathing + optics + substring cache) — 2026-06-16
All algorithm pieces proven in docs/prototypes/anamorph (multi-node, builds in seconds):
  - PATHING: port(cluster, dir) = surface point nearest an edge vector → flow ENTERS each pearl at the point
    nearest its parent, REFRACTS through toward the onward children, EXITS at the point nearest each child.
  - OPTICAL RULES (variety): prism (RGB lateral dispersion), occlusion (dim in dense cores), refractive bend.
    Easy to extend per-edge (caustic flicker, attenuation, blocker nodes) for "cool patterns."
  - REVEAL AXIS from edge vectors: axis = normalize(flow_tangent × up) so text faces ACROSS local flow;
    camera-dependent reveal then falls out of topology (spine reads, branch scrambles from the same camera).
  - SUBSTRING CACHE ("greatest impact"): tokens ranked by freq×length, top-K layouts precomputed, rest
    composed from a per-glyph cache. Verified reuse (hits>misses); eases global compute of building N pearls.

## >>> NEXT BIG BUILD: wire refractive array into the live gpu-scene of Atlantis
The remaining ask. Prototype de-risked it; the GL port is the heavy part:
  1. FONTS in-app: the scene needs glyph points without the standalone DejaVu read. Options: add fontdue +
     include_bytes! an embedded TTF, OR sample epaint's Galley tessellation. (fontdue+embed is simplest.)
  2. CLUSTER→MESH: the scene shader rasterizes triangles, not GL_POINTS. Represent each particle as a tiny
     billboard/quad or low-poly icosphere. Budget: thousands/particle → only viable per-node with LOD.
  3. LOD (required): far node = 1 pearl mesh (today's Pearl); near = decoy haze; FOCUSED = full glyph+decoy
     cluster. Switch by projected size / focus. Ties into the LOD TODO.
  4. ALIGNMENT-GATED reveal: glyph emissive = dot(view, node.axis)^3, computed CPU-side per frame (we have
     the camera) and written into the mesh emissive, OR as a shader uniform per cluster.
  5. FLOW cords: replace the current strand mesh near a focused node with the refracted/prism/occluded path.
  6. "see each graph section": cinematic camera (already batched) swings onto a node's reveal axis to read it.
Acceptance: load arbitrary text → orbit to a node → it resolves into readable text; orbit away → glittering pearl.

## ANAMORPHIC PEARL-TEXT — iteration 2 DONE (reveal + sphere both work) — 2026-06-16
Resolved all three iter-1 issues. Technique now: TWO particle populations + alignment-gated glyph.
  - GLYPH layer: in-plane fixed by stroke, THIN depth band (~0.14R) → readable over a wide arc.
  - DECOY layer: ~2600 pearls rejection-sampled in the ball → the orb body from any off-axis angle.
  - REVEAL = geometry, not a probability gate: glyph brightness = (cos angle-to-reveal-axis)^3, so the
    word BLAZES on-axis and FADES into the orb off-axis. Verified: 0deg crisp text-in-orb, 18deg still
    readable, 45deg fading, 82deg a clean glittering pearl with the rays crossing. No collapse, no solver.
Remaining = GRAPH INTEGRATION (next): per-node cluster; reveal axis = f(inbound, outbound edge vectors)
so text faces across local flow; LOD (far node = 1 pearl, focused = full cluster); cinematic camera swings
onto a node's reveal axis to read it; port the dot-splat look into the GL pearl material (emissive points).

## ANAMORPHIC PEARL-TEXT — VALIDATED approach (prototype in docs/prototypes/anamorph)
Supersedes the billboard/facet text plan AND the failed "optical-transport/Markov/RNN emergence" path
(refs/failed-emergent-approach.md — that frame is a trap: text-as-diffusion-equilibrium is underdetermined
→ collapse blobs, and "camera as probability gate" never becomes real geometry).
CORE (proven deterministic, ~120 lines, no solver): a camera projection fixes 2 of each particle's 3 coords
(its in-plane landing on a glyph stroke); DEPTH is free → spend it filling a sphere bounded by
±sqrt(R^2 - rho^2). Reveal axis → reads as the word; any other axis → glittering pearl-cloud. Input/target
"rays" are just lines drawn along the edge vectors.
INTEGRATION into the graph: each NODE becomes such a cluster; reveal axis derived from the node's inbound
(parent→node) + outbound (node→children) edge vectors (text faces across the local flow); cluster oriented
+ positioned on the strand; cinematic camera swings to a node's reveal axis to "read" it.
OPEN (from prototype renders):
  1. SPHERE READ off-axis ("bounded/scored by sphere geometry"): wide words become a flat band, not a ball.
     Fix: compact multi-line / arced glyph layout to fill the projected disk; bias edge-point depths toward
     the shell; optional dim sphere-shell decoy particles. The "score" = legibility(projection on-glyph) ×
     volume-fill(sphere-ness).
  2. REVEAL TOLERANCE: scrambles by ~35°; widen the readable band (more particles/stroke, slight depth
     compression near strokes) so the reveal has a comfortable sweet spot.
  3. LOD: cluster particle budget scales with projected size / focus (far nodes = single pearl, focused = full
     cluster). Ties into the LOD TODO.
Run the prototype: `cd docs/prototypes/anamorph && cargo run --release -- "WORD"` → out_{reveal,mid,off}.ppm.
