# Signature components — what changed (already applied in this tree)

Spine-grown parts (from `tools/gen_spine.py`) are wired as first-class
transformer components: per-class + per-enemy defaults, builder-swappable, posed
in BOTH silhouettes with a staged fold, and now with a gameplay profile. All
changes are already in this tree — just build.

## Build & run
```
mkdir build && cd build
cmake .. -DCMAKE_BUILD_TYPE=Release
cmake --build . -j
./cybertron            # (needs SDL2 + a GL runtime, like the rest of the game)
```

## Run the tests (no GPU needed)
```
cd build && ctest --output-on-failure
ctest -R 'signature|spine_iqm' --output-on-failure
```
All 24 tests pass in-tree (verified), including test_signature and test_spine_iqm.

## What was added / changed
New:
- engine_src/exengine/signature.h/.c — the signature system (see below)
- engine_src/data/spine_{wing_cannon,heavy_cannon,crest,cable,muscle}.iqm
- tools/gen_spine.py (headless spine->IQM) and tools/obj_to_iqm.py (Blender->IQM)
- tests/test_signature.c, tests/test_spine_iqm.c

Changed:
- engine_src/game.c — loads signature templates; swaps the player's BACK mesh to
  the effective signature and each enemy's to its body default; poses BACK from
  ex_signature_pose (two-sided + staged fold for the player); applies the
  signature gameplay stats; shows the signature name on deploy.
- engine_src/exengine/menu.c / menu.h — a new "signature" row in the customizer
  (class default -> wing-cannon -> heavy cannon -> crest -> cable).
- CMakeLists.txt — builds signature.c and registers the two new tests.

## The signature system (signature.c)
- ex_signature_for_class(class) — Vanguard->heavy cannon, Interceptor->wing-cannon,
  Titan->crest, Specialist->cable.
- ex_signature_for_body(kind) — grunt->wing-cannon, corrupted->crest,
  hijack->cable, apex->heavy cannon, human->none.
- ex_signature_pose(rob, veh, sig, vehicle, ...) — robot AND vehicle pose
  (flyer->spread wings, ground->spoiler/turret, beast->spine ridge). This is why
  the component adapts instead of vanishing on transform.
- ex_signature_stats(sig) — gameplay profile (dmg / refire / toughness / speed):
    wing-cannon  fast flanker (quick refire + speed, fragile)
    heavy cannon siege (big hits, tanky, slow)
    crest        bruiser wall (armoured, small damage edge)
    cable        rapid harasser (fastest refire + mobile, low per-hit)
- ex_signature_fold_start() — deploys late in the transform fold.

## Builder override
loadout.signature == -1 uses the class default; any explicit value overrides.
Resolved by player_signature() in game.c.

## Honest scope
Verified headless: all 24 ctests pass in-tree; game.c/menu.c compile clean.
NOT verified here (no GPU/display): the live in-window look and feel. When you
run it, eyeball the signature's scale/orientation per body, how it sits on each
vehicle archetype, the fold timing, the customizer row, and whether the stat
tradeoffs feel right — all plain constants in signature.c / menu.c.

## Update — in-game fixes
- forward.glsl: fixed the silhouette-rim shader (out-of-scope normals/view_dir).
- Vehicle-mode weapon is now MOUNTED ON THE MODEL (chassis for jet/racer/etc,
  HEAD for beast modes); it inherits that part's position + rotation, so it
  rides and rotates with any pose animation. See game.c weapon block.
- Wall CLIMB on foot: look/press toward a wall + hold JUMP to scale it (limited
  budget, refills on landing). Pure probe is ex_step_wall_ahead (terrain_step),
  tested by tests/test_climb.c.
- Signature vehicle-mode poses tightened to hug the hull (they were oversized
  and blanketing small vehicles).

## Update 2 — camera / jump / weapon fixes
- Camera boom now pulls in against ALL collidable boxes (map + deco colliders,
  not just blueprint vols). Previously the third-person camera could boom INTO a
  map/deco box and a giant block filled the screen (world geometry, so it didn't
  move with the bot). See boom_slab() + the camera block in game.c.
- Rock cover visual now stays CENTERED on its collider. It was dropped by half
  its height, so you landed on the collider top floating a half-height above the
  visible rock -- the "jump looks wrong on blocks" issue. Tall spires keep the
  ground offset.
- Vehicle-mode weapon is mounted on the model (chassis / beast head) and
  inherits that part's transform (from Update 1).
- (Wall climb from Update 1 confirmed working in-game.)

## Update 3 — swept step landing (jump onto blocks)
- ex_step_resolve now takes the pre-move Y (swept collision): if the feet were
  above a box top last frame and fell below it this frame, you LAND on top
  instead of being shoved out the side. This is what made fast jumps onto steps
  fail. The upward correction can't exceed one frame's fall (physical, no
  teleport). It also cleaned up pre-existing harness anomalies: the movement
  harness now reports ALL COURSES SMOOTH (0 anomalies) vs 2 before.

## Update 4 — signature combat identity
Each signature now has a distinct ON-HIT effect (on top of its stat profile), so
the class/signature choice changes how you fight, not just your numbers:
- wing-cannon (Interceptor): +1 spread bolt per shot (dual cannon)
- heavy cannon (Vanguard): 16 knockback + 4u splash (siege, staggers clusters)
- crest (Titan): 9 knockback (bruiser shove)
- cable (Specialist): 1.6s root/slow on the target (crowd control)
Implemented at the existing shot-resolution point in game.c using the engine's
own primitives (pellet/spread loop, ex_enemy_damage, g_enemy_vel knockback,
g_enemy_web slow). Data is ex_signature_combat() in signature.c; bounded +
differentiated, covered by test_signature.

## Update 5 — ported Blackstar narrative into the codex
Read the Blackstar screenplay docs (the HTML master + its markdown components)
and ported the characters / factions / locations into the in-game Codex
(engine_src/exengine/lore.c) as original summaries -- no source text reproduced.
Added: jhiaxus, blackstar, optimus, rodimus, aiden, unicron, the technocrats,
cybertron, the clone coalition, the helion alliance, earth, the watch forms, the
matter planet. Codex now holds 32 entries (was 19). test_menu (drives the codex)
passes.

## Update 6 — parsed the master HTML chat, ported more lore
Parsed the 19MB HTML (a ChatGPT export: 175 turns + the outer_rim_cloaks concept
art) for content the markdown extracts didn't have, and ported it to the codex:
cryotek (opening villain), hot rod (-> rodimus via the Matrix), megatron's truce,
the matrix, the decepticons, stratos, pyroclast, the solar engine (system-as-
machine reveal), the reassembler (Unicron's true purpose), sam witwicky, the
asteroid graveyard. Codex now 43 entries. A clean text digest of the chat is in
docs/blackstar_chat_digest.txt.

## Update 7 — linear re-read / canon correction of the codex
Re-read the HTML chat chronologically (it's an evolving script-dev conversation)
and cross-checked the bible markdown to catch superseded early drafts:
- Verified Cryotek and Blackstar are SEPARATE characters in both docs (turn 140:
  "Cryotek's corpse floats... Blackstar arrives"; bible: Cryotek = ice-base
  opener killed by Sam, Blackstar = Decepticon naval warlord). Kept both.
- Removed stratos + pyroclast codex entries: they appeared only at turn 3 and
  were dropped, replaced by the developed cast.
- Added the persistent Outer-Rim cast: the spearcons (Cryotek's lieutenants
  Shardclaw/Icerender + sticky delayed spears) and forgeheart (asteroid-flipping
  autobot). Refined the cryotek entry to the consistent final beat.
Codex holds 43 entries. test_menu passes.

## Update 8 — Ice-Clone quantum leap: the delayed gravity wave
Reworked the warp jump to match the film script's signature idea. The leap is now
a SILENT insertion (instant, no impact -- just a shimmer). Gravity "catches up"
a few seconds later: a telegraphed wave (a tightening, quickening ring at the
landing point) then fires at that spot, hitting friend and foe alike -- you know
it's coming and can clear out, but can't stop or shape it. New pure scheduler
engine_src/exengine/warp_wave.c (arm/tick/progress), covered by tests/test_warp_wave.c.

## Update 9 — Photon Man: the Watch's ceiling (auto last stand)
Implemented the thirteenth Watch form as the script frames it -- NOT a button.
When the bearer is about to be overwhelmed (critically low HP + under real
threat, as a full transformer), the Watch itself decides the moment and steps
in: a short window of overwhelming power -- ~x0.12 damage taken, x1.9 damage,
faster refire, +30% speed, blazing white self-lit body, and radiant pulses that
scour nearby enemies -- pulling the bearer back from the edge. Afterward it goes
cold for the rest of the life (used once). Pure state machine in
engine_src/exengine/photon.c, covered by tests/test_photon.c.

## Update 10 -- Spearcons: Cryotek's sticky countdown charges
The ice faction's signature weapon (Shardclaw & Icerender). Grunts occasionally
lob a sticky charge onto the player's spot; it clings and counts down behind a
shrinking, quickening ice->white disc, then detonates in a small radius -- so you
have to keep MOVING off your position. Throttled so it pressures rather than
spams. Pure pool in engine_src/exengine/spearcon.c (plant/update/progress),
covered by tests/test_spearcon.c.

## Update 11 -- Blackstar's resonance field (enemy coordination)
An aware apex/elite now acts as a COORDINATOR: it projects a resonance field
(pulsing ring) that syncs nearby enemies into acting as one. Synced units
hesitate for a beat (recalibrate), then share the coordinator's fix on you (never
lose track), move ~30% quicker, and take on a cold violet sheen. Kill the
coordinator and the field collapses -- the pack scatters back to individuals.
Pure field geometry in engine_src/exengine/resonance.c (strength + falloff),
covered by tests/test_resonance.c.

## Update 12 -- Corrupted generational drift (Jhiaxus's experiment)
Each Corrupted is now tagged with a GENERATION as it appears (climbing over the
fight/waves), and later generations are more autonomous -- the horror of Jhiaxus's
experiment exceeding its own containment models. Higher gens hesitate less (they
recalibrate faster after being startled), move up to +55% faster, grow up to +40%
larger/more mutated, and darken toward corrupted black. Bounded + monotonic
escalation curve in engine_src/exengine/corrupt.c, covered by tests/test_corrupt.c.

## Update 13 -- three new class archetypes + back-models (from the script)
Expanded the playable roster from 4 to 7 classes (available in skirmish + campaign,
selectable in the builder), each drawn from a script character with its own new
spine-grown back-model, two-sided poses, stat profile, signature combat, and
gated vehicles:
- REAVER (Blackstar) -- outer-rim predator; harpoon: impaling knockback + magnetic
  slow; mobile, a touch fragile; gunship/hawk/cycle/hover. mesh spine_harpoon.iqm.
- WARDEN (the Morpher) -- clone sentinel; sentinel halo: wide suppressive splash;
  a very tanky, steady wall; mech/crab/tank/spider. mesh spine_halo.iqm.
- PYRE (Billy) -- fire clone; solar core: burning nova (knockback + splash); a
  glass cannon (huge damage, very fragile); dragon/jet/hawk/raptor. mesh spine_solar.iqm.
Meshes generated headlessly via gen_spine.py, validated by test_spine_iqm. Signature
enum grew 4->7; ex_signature_for_class, poses (robot + 3 vehicle categories), stats,
combat, names, menu row, and sig_template templates all extended. test_class /
test_signature cover all 7; full suite green.

## Update 14 -- Spearcon-Lieutenant enemy + script-driven campaign
Two content build-outs for skirmish + campaign:

ENEMY MODEL -- Spearcon Lieutenant (Shardclaw & Icerender's kind). Implemented as
a per-slot ROLE overlay on grunts (no new enemy kind, so encounter internals are
untouched): ~1 in 3 grunts is tagged (stable per slot) as a lieutenant -- wears
the harpoon back-signature (EX_SIG_HARPOON), takes a cold steel-blue elite tint,
is x1.7 tougher, and is now the ONLY grunt that plants spearcons (plain grunts no
longer do). Tag assigned once per slot in the physics loop, read in the render/
attack loop, freed on death. Appears in both skirmish and campaign.

CAMPAIGN -- rewrote the 6 missions into this project's "Doomsday" arc (original
terse prose, no external text reproduced), with real maps + varied objectives:
1 the pluto line (frostline, eliminate -- Cryotek's ice crew),
2 the scavenger king (outer_rim, survive 60s -- Blackstar + Spearcon ambush),
3 the beach opens (wastes, protect -- Earth fissure, Unicron stirs),
4 what jhiaxus grew (vault, eliminate -- the Reassembler reveal, Corrupted drift),
5 the mantle dive (foundry, hold the point -- Hot Rod -> Rodimus / the Matrix),
6 beneath the pacific (cybertron, eliminate + boss -- Unicron finale, Megatron's truce).
Added ex_objective_name cases for CONTROL/PROTECT. Boss still auto-spawns on the
last mission. All 30 tests pass; game.c + campaign.c compile clean.

## Update 15 -- Billy's star-fire deepened (held star + nova + cosmic cost)
Reworked the fire-form charged shot from a plain damage multiplier into Billy's
signature power. Holding RELOAD in a fire form now grows a visible HELD STAR out
front (the solar mesh, brightening/expanding with charge). Releasing:
- a PARTIAL charge is a boosted bolt (as before, x1..~5), no cost;
- a FULL charge (>=0.85) releases a real STAR -- a wide radiant nova blasts out
  ahead (huge damage + rings + gold burst), and then star-fire LOCKS OUT for ~6s
  (the cosmic cost: "even the clones are careful when to spend it").
Module extended in weapon.c/.h (cooldown + full-star tier + ready/cooldown_frac);
test_weapon now covers the star tier, the lockout, and partial-vs-full releases.
Ties to the PYRE (Fire Clone) class. All 30 tests pass; game.c compiles clean.

## Update 16 -- Unicron's graviton well (boss pull field)
The apex/Unicron-tier boss now projects a persistent GRAVITON WELL: a steady
inward pull on the player (and loose debris swirls toward it), weak at the rim
and strongest near the center, so the boss fight becomes a fight to hold your
distance. A violet boundary ring pulses to show its reach. The well collapses the
instant the boss dies. Pure pull field in engine_src/exengine/graviton.c
(direction + linear falloff), covered by tests/test_graviton.c. Distinct from the
apex's existing one-shot graviton *pulse* -- this is the constant background tug.

## Update 17 -- the Corrupted absorb metal to grow
Wired the long-dormant enemy `grow` field into the script's "it consumes"
mechanic. A Corrupted now eats loose wreckage within reach: each bite pushes its
grow multiplier up (diminishing returns, capped at 2.2x), so over a debris-heavy
fight it visibly SWELLS -- up to +80% larger, up to +100% melee damage -- and
repairs a little hp per scrap. Stacks with the generational drift, so a late,
well-fed corrupted is a genuine bruiser. Pure bounded curve in
engine_src/exengine/absorb.c (accumulate + size/damage scaling), covered by
tests/test_absorb.c. NUM_DEBRIS scan is cheap (48). All 31 tests pass.

## Update 17 -- the Reassembler consumes debris (built on the existing absorb system)
The Unicron-tier boss now embodies the script's core reveal -- it doesn't only
take damage, it TAKES MATTER. Wreckage dragged into the graviton well (Update 16)
that reaches the core is consumed: it mends a little hp and feeds the boss's
EXISTING grow/absorb stat (ex_absorb_accumulate, cap 2.2), which already scales
the apex's size and melee damage via ex_enemy_visual_radius. So the well + the
absorb loop combine: destroying cover near the boss literally feeds it, and a
drawn-out fight lets it swell. NOTE: I first started a separate "reassemble"
module, then found the pre-existing absorb.h/.c (Corrupted "absorb metal to
grow") already implemented this curve and the apex already scaled by `grow` --
so I removed the duplicate and drove the feature through the existing system
instead. No new module; the change is a debris-consume loop in game.c. All 32
tests pass (existing test_absorb covers the curve).

## Update 18 -- Forgeheart, the Outer-Rim ally (first friendly NPC)
Added the game's first COMPANION: Forgeheart fights alongside you in the campaign
from mission 2 onward (where the Outer-Rim allies join the story). He follows when
it's quiet (leashed to you), wades into the nearest enemy when it isn't, and
"flips wreckage to crush" his target -- solid melee damage + knockback + a debris
burst on a cadence. He carries a heavy demolition rig (heavy-cannon back-sig) and
a friendly gold sheen so he never reads as a threat. If swarmed he goes DOWN
("FORGEHEART DOWN"), then pulls himself back together after ~12s ("BACK UP");
otherwise he self-repairs when clear. Campaign-only so solo skirmish stays a pure
test of your own build. Pure brain (target/engage/follow/cadence/revive) in
engine_src/exengine/ally.c, covered by tests/test_ally.c; body reuses spawn_body/
pose_body. All 33 tests pass; game.c compiles clean.

## Update 18 -- the Tsunami (water-form power; completes the clone triad)
Added the Watch's WATER-form signature power, matching the ice leap and fire star
so each clone element has its own move. In a water form, press V to surge a
TSUNAMI: a directional wall of water that advances forward from you (distinct from
the radial warp wave), drowning + shoving enemies and sweeping debris along its
heading as it crests, then dissipates; 7s cooldown. Pure front-tracker in
engine_src/exengine/tsunami.c (arm/tick/progress -- normalized heading, linear
advance, spends once at range), covered by tests/test_tsunami.c.
NOTE: before building, inventoried existing modules and caught that `ally` (Forgeheart)
and `absorb` were already implemented -- so I did NOT duplicate them; picked tsunami,
which was genuinely absent. All 33 tests pass; game.c compiles clean.

## Update 19 -- bug fixes from the loading log + player report
Analyzed the uploaded loading.log and fixed four issues:
1. BIG BOXES on the player (builder/showcase only): builder_preview() fed RAW
   loadout percentages (bulk=140) into the body layout, which expects normalized
   values (~1.4) -- a ~100x blow-up into giant boxes. Enemies (never shown there)
   and the in-game player (deploy path normalizes) were fine, which is why it was
   player-only. Now normalized the same as the deploy path (/100 * chassis muls).
2. MISSING TEXTURES: the log showed the aux spec_/norm_/emit_ maps missing for
   many textures (notably spec_/norm_plate_blue -- the PLAYER plating, 18x). Fixed
   the iqm.c loader to stop deriving maps-of-maps (killed the spec_spec_*/spec_norm_*
   spam), and generated the 44 spec + 45 norm + 46 emit maps that were absent
   (spec from albedo luminance, subtle Sobel normals, emit=own-color for genuinely
   emissive textures / black for surfaces incl. cybertron+reclaim map floors).
   These live in engine_src/data/textures and get packed into data.ex on build.
3. CLIMB too tight: raised the wall-climb budget 1.1s -> 2.4s (taller climbs) and
   made it SURFACE-DEPENDENT via g_surface_grip -- slick surfaces (ice/frostline,
   grip 0.16) give a weaker hold + drain the budget faster ("CLIMB -- slick!"),
   solid footing climbs full.
4. JUMP-ON-OBJECTS: arena cover features were registered in g_auto_col, which
   build_player_boxes never fed to the player step-resolve -- so you couldn't land
   on (or properly collide with) them. Added the feature colliders (auto_col
   0..feature_base; clutter left out so the titan steps over crates) and enlarged
   the box arrays by MAX_AUTO_COL. The "box" AABB (half 1) matches box.iqm exactly,
   so landings sit on the visible top.
All 34 tests pass; game.c + iqm.c compile clean.

## Update 20 -- the energy siphon (the devourer feeds)
The Unicron/Blackstar-tier boss now FEEDS: linger in close and it siphons your
spark, healing itself with half the take -- strongest point-blank, tapering to
nothing at 40u, with a violet tether beam telegraph and a "SIPHONED -- break
range!" cue. It layers with the graviton well (the well drags you into siphon
range, the siphon drains you there), so camping in the boss's face is punished
and breaking range matters. Pure drain-rate curve in engine_src/exengine/siphon.c
(point-blank max, linear falloff), covered by tests/test_siphon.c.
Inventoried modules first and confirmed siphon/drain/matrix/truce were all absent.
All 35 tests pass; game.c compiles clean.

## Update 21 -- the Matrix of Leadership (claimed -> Rodimus)
Added the script's pivotal Hot Rod -> Rodimus beat as a CLAIMED relic, distinct
from Photon Man's auto defensive last-stand. New EX_PICKUP_MATRIX (radiant gold-
white) -- a rare pickup, always present on the campaign's final/boss mission and
~8% of other matches, placed off-center. Taking it opens the Matrix: a golden
light bursts out (clears the space + heals 35%), then a ~15s RODIMUS state --
x1.7 damage, x0.65 refire, +25% speed, x0.6 damage taken (hardened, NOT the
near-invulnerable Photon), a radiant gold aura, and Matrix-light pulses that
scour nearby enemies (offense, vs Photon's defense). Unlike Photon it's claimed,
longer, and re-claimable (no once-per-life lock). Pure state in
engine_src/exengine/matrix.c, covered by tests/test_matrix.c. Inventoried first --
matrix/rodimus were absent. All 36 tests pass; game.c + pickup.c compile clean.

## Update 22 -- Megatron's truce (enemies defect against the boss)
The script's beat where the devourer threatens everyone and Megatron proposes a
truce. Once the Unicron-tier boss is worn to <=70% HP, the Decepticon GRUNTS turn
(latched for the match, announced "MEGATRON: a truce -- against THAT."): they stop
hunting you, take an allied cyan glow, charge the boss, and their strikes damage
the boss instead of the player. First time an enemy fights ON your side (distinct
from the scripted Forgeheart ally). Implemented as a per-slot g_defected overlay
(no new enemy kind): trigger in truce.c (pure, tested), velocity override to charge
the boss, attack redirect in the enemy-attack block, cyan tint. Reset on deploy/
death. Inventoried first -- truce/defect were absent. All 37 tests pass; game.c clean.

## Update 23 -- two more class archetypes + back-models (Prime, Scout)
Roster grown 7 -> 9 playable classes (selectable in skirmish + campaign), each a
script character with a new spine-grown back-model, two-sided poses, stats,
signature combat, and gated vehicles:
- PRIME (Optimus, the steward) -> EX_SIG_AEGIS (spine_aegis.iqm, a broad guardian
  arch): durable all-rounder -- solid hits, very tanky, slower; guardian shove +
  a rallying splash; vehicles car/tank/mech/hover; hp1.35/spd0.95.
- SCOUT (Bumblebee, recon) -> EX_SIG_RECON (spine_recon.iqm, light swept fins):
  fast + light + fragile; quickest refire; an extra bolt + a sensor-tag slow;
  vehicles car/cycle/hawk/jet; hp0.72/spd1.35.
Signature enum 7->9 (EX_SIG_AEGIS/RECON); ex_signature_for_class, poses (robot +
3 vehicle categories), stats, combat, names, menu row, sig_template templates,
and the g_sig_tpls array all extended. Meshes generated via gen_spine.py,
validated by test_spine_iqm. test_class/test_signature cover all 9 (9 classes ->
9 distinct meshes). All 37 tests pass; game.c compiles clean.

## Update 24 -- Blackstar Raider (territory-themed elite grunt)
Enemy-roster variety: elite grunts are now territory-themed. On Blackstar's
frontier (map id contains "outer_rim" or "frost" -- i.e. skirmish frostline/
outer_rim, and campaign missions 1 "the pluto line" & 2 "the scavenger king")
the ~1-in-3 elite grunt is a BLACKSTAR RAIDER instead of a Spearcon-Lieutenant;
elsewhere it stays a lieutenant. A grunt is at most one. The raider wears the
HALO rig (previously enemy-unused), a scavenger-amber tint, x1.6 HP, and an
aggressive boarding LUNGE (a periodic burst toward you, distinct from the
lieutenant's area-denial spearcons). Implemented as a per-slot g_raider overlay
(mirrors g_lieut, no new enemy kind): captured the current map id at deploy
(g_cur_mapid) to theme the elite; reset on deploy/death; defected (truce) raiders
turn cyan and charge the boss like any grunt. All 37 tests pass; game.c clean.

## Update 25 -- elite variants for Corrupted + Hijack (every kind now has an elite)
Rounded out the enemy roster so each kind has an elite tier (grunt already had
Lieutenant + Raider). Both appear in normal skirmish + campaign encounters (they
spawn via ex_encounter_spawn_makeup), same per-slot overlay pattern (no new enemy
kind):
- CORRUPTED RAVAGER (~1-in-3 Corrupted): Jhiaxus's pre-grown horror -- starts with
  grow>=1.6 (so bigger AND harder-hitting via the existing absorb/grow scaling),
  x1.5 HP, wears the SOLAR rig (an absorbed molten core -- gives solar a home on
  enemies), molten-magma tint.
- SLIPSTREAM (~1-in-3 Hijack): the assassin -- x1.4 HP, wears the RECON rig (sleek
  fins), pale silver-violet, and CIRCLE-STRAFES you (fast flanker) instead of
  charging, distinct from the Raider's head-on lunge.
g_ravager/g_slip overlays, reset on deploy/death, defected (truce) versions defer
to the truce like any enemy. All 37 tests pass; game.c compiles clean.

## Update 26 -- CRYOTEK named ice mini-boss + a reusable boss health bar
The campaign's opening finally has a named boss. On the ice line (frostline map --
campaign mission 1 "the pluto line" + skirmish frostline) the first grunt rises
as CRYOTEK: x8 HP, x2.3 scale, the CREST rig (horned ice-overlord crown), a deep
glacial-ice glow, announced "CRYOTEK holds the ice line". His signature is a
periodic ICE NOVA -- a radial freeze blast (twin shockrings + frost burst) that
shoves you back and bites at close range; his grunts just brawl. Killing him ends
with "CRYOTEK falls -- the ice line breaks". He's the champion (excluded from the
Lieutenant/Raider rank-elite tagging). Territory refined: frostline is now
Cryotek's ice faction (lieutenants), outer_rim stays Blackstar's (raiders).
Added a small REUSABLE boss health bar on the 32x16 HUD grid (name + a 24-cell
bar drawn with ex_vga_print), shown for Cryotek OR the apex devourer when present
-- so the apex now gets a health bar too. Champion via a single g_cryotek_slot
(reset on deploy, cleared + announced on his death). All 37 tests pass; game.c
compiles clean.

## Update 27 -- BLACKSTAR mini-boss + generalized champion system
Generalized the Cryotek champion into a reusable named-mini-boss system
(g_champion_slot + g_champion_kind) and added a SECOND boss so another campaign
mission has a climax. On outer_rim (campaign mission 2 "the scavenger king" +
skirmish outer_rim) the first grunt rises as BLACKSTAR, the Kuiper warlord whose
Raiders already patrol that map: x8 HP, x2.3 scale, the HARPOON rig (barbed
boarding spear), a crimson-void glow, announced "BLACKSTAR -- the scavenger king".
His signature is a HARPOON PULL -- a barbed spear (red beam) that REELS YOU IN
toward him to be boarded (distinct from Cryotek's radial ice nova that shoves you
away). Falls with "BLACKSTAR falls -- the raiders scatter". Shares the boss health
bar (shows CRYOTEK or BLACKSTAR by kind). Cryotek's kind-1 code (crest/ice/nova)
and Blackstar's kind-2 (harpoon/crimson/pull) branch off g_champion_kind -- no
duplicate system. Campaign now: Cryotek (m1 ice), Blackstar (m2 raiders), the
devourer (finale). All 37 tests pass; game.c compiles clean.

## Update 27 -- the failing containment (Pluto boundary survival mechanic)
Grounded in the screenplay's cosmology (digest: "Pluto region becomes a
containment field... it is forced containment"): the technocrats could not destroy
Unicron, only cage him -- Earth for the head, the Outer Rim for the spine, Pluto
for the field -- and that field is now failing. New survival mechanic: a
CONTAINMENT boundary that draws inward over the match; beyond it is the consuming
void, which bites harder the further out you stray (2.5 + 0.35/unit hp/s), forcing
you toward the shrinking core. A pulsing cyan ring marks the current edge; a
"CONTAINMENT BREACH" cue fires when you're outside. Active in survive missions
(closes right as the clock runs out, close=survive_seconds) and on the shield_edge
map (close=75s). Pure geometry in engine_src/exengine/containment.c
(ex_containment_radius contracts r0->r1 clamped/monotonic; ex_containment_breach),
covered by tests/test_containment.c. Also added a "the containment" codex entry
(lore.c) from the same passage. Explored the documentation first to build it
faithfully. All 38 tests pass; game.c + lore.c compile clean.

## Update 28 -- chain lightning (electric-form power; extends the clone set)
Added the Watch's ELECTRIC-form signature move, joining ice (warp leap), fire
(star), and water (tsunami) so a fourth element has its own power. In an electric
form, press F to loose CHAIN LIGHTNING: a bolt that grounds through the nearest
foe then arcs to the nearest not-yet-hit enemy within reach, up to 5 targets,
damage weakening ~0.82x per jump and briefly stunning each; 4.5s cooldown.
Distinct from the other clone powers (chaining target-selection, not a wave or
projectile). Pure targeting in engine_src/exengine/arc.c (nearest-unvisited,
range- and jump-capped, no repeats), covered by tests/test_arc.c.
NOTE: inventoried first -- found the "boundary Shield" idea already exists as the
containment field (containment.c, active on shield_edge + survive missions), so I
did NOT rebuild it; picked chain lightning, which was genuinely absent. All 38
tests pass; game.c compiles clean.

## Update 29 -- the whirlwind (air-form power; 5th elemental clone move)
Added the Watch's AIR-form signature, joining ice/fire/water/electric so five of
the elements now have a power. In an air form, press C to spin up a WHIRLWIND ~18u
ahead of you: for ~2.6s enemies caught inside are swept into a counter-clockwise
ORBIT around its eye (their velocity overridden by the wind field) and battered
off their attack, then flung loose. The orbiting/tangential force is unique --
distinct from every push/pull/wave already in the game. Pure wind field in
engine_src/exengine/cyclone.c (tangential swirl + inward drag, stronger near the
eye, zero past the rim), covered by tests/test_cyclone.c.
NOTE: inventoried first -- a vine "entangle" would have just reskinned the existing
g_enemy_web root/slow, and enemy vertical isn't integrated (a cyclone LIFT wouldn't
show), so I chose a horizontal swirl that's both novel and physics-safe. All 39
tests pass; game.c compiles clean.

## Update 30 -- the thorn field (vine-form power; 6th elemental clone move)
Added the Watch's VINE-form signature, bringing six of the seven elements to a
power (only rock remains). In a vine form, press X and brambles erupt AROUND you,
spreading 0->30u over ~1.2s and holding ~6s: enemies inside are torn (DoT, worse
near the centre) and snared (reuses g_enemy_web). It's the game's first PLAYER-
placed PERSISTENT AREA HAZARD -- zone control / area denial, where the other clone
powers are bursts (and it's planted at your feet, defensive, vs the cyclone's
thrown-ahead vortex). Pure field geometry in engine_src/exengine/thorns.c
(spread-then-hold radius + centre-weighted intensity), covered by tests/test_thorns.c.
Inventoried first: reused the existing web slow for the snare rather than adding a
second root system. All 40 tests pass; game.c compiles clean.

## Update 31 -- weapon customization pass (types, addons, models, menu) + render fix
Weapon expansion drawing on the movie script + WFC/FOC:
- NEW FIRE MODES: EX_FIRE_PIERCE (Titanium -> armour-piercing RAILGUN: hits every
  enemy in a line up to the first wall) and EX_FIRE_FLAK (Fire -> airburst cannon:
  direct hit + a 13u splash). Implemented in the hitscan firing loop.
- NEW ADDONS (weaponmod, WFC/FOC-inspired): energon cell (+30% dmg, +cd), rapid
  coil (-32% cd, -dmg), pierce core (converts any gun to a railshot), scatter
  barrel (+2 pellets, spread -> a scatter blaster). Now 11 addons total.
- MENU ACCESSIBILITY: the loadout customiser now exposes THREE weapon-addon slots
  (was one) -- weapon addon 1/2/3, each cycling all 11 addons; all three apply
  in-game. EX_CUSTOMIZE_ROWS 18->20.
- WEAPON DESIGN: the arm-cannon now reshapes to its addons -- long barrel / pierce
  core lengthen it, scatter / underbarrel fatten it, amplifier / energon cell keep
  it glowing hot even at rest.
- Also RE-APPLIED the emissive-solid/emission_strength init in ex_model_new +
  ex_model_copy (render/model.c) -- the "glitched block on every model" fix, which
  was not present in the working copy.
test_weapon extended to cover the new modes, all four addons, and 3-slot stacking.
All 41 tests pass; game.c compiles clean.

## Update 32 -- plasma/ricochet/suppress + turret ammo + ripple/cryo addons
More weapon variety (script + WFC/FOC):
- 3 NEW FIRE MODES: ELECTRICITY -> PLASMA CANNON (heavy electric ball, big 22u
  blast that shoves the cluster; WFC riot cannon), METAL -> RICOCHET (slugs bounce
  off walls to the nearest foe), ICE -> SUPPRESSION beam (heavy 0.8 slow, long
  reach). Also wired wp.slow to actually apply on hit (g_enemy_web) -- it never was.
- TURRET ammo cycle (U) 4 -> 6: added ARC (zaps target, jumps to 2 nearby) and
  FLAK (airburst splash), each with its own turret tint.
- 2 NEW ADDONS: ripple coil (ricochet conversion) and cryo rounds (adds a chilling
  slow) -> 13 addons total, all selectable across the 3 menu slots.
Arsenal now spans 10 fire modes (bolt/burst/beam/wave/lob/pierce/flak/plasma/
ricochet/suppress). test_weapon updated (variety assertion relaxed since electricity
left BEAM; asserts all 5 new modes + the ripple/cryo addons). All 41 tests pass;
game.c compiles clean.

## Update 32 -- bug hunt: reversed transform + harsh climb
Two real bugs found on disk and fixed:
1. REVERSED TRANSFORM ANIMATION (user-reported). transform.c flips x->mode only at
   blend COMPLETION, so during TRANSFORMING x->mode is the mode being LEFT. game.c's
   morph picked src/dst as if `in_vehicle` were the mode being ENTERED, so the blend
   ran end->start (both directions). Swapped src/dst + nsrc/ndst so it interpolates
   leaving-mode -> entering-mode as bp01 goes 0->1.
2. HARSH CLIMB / too many falls (user-reported). Budget 2.4->4.0s, grip
   0.45+0.55g -> 0.80+0.20g (ice ~0.83 not 0.54), drain /grip (1.85x on ice) ->
   *(1+(1-grip)*0.5) (~1.1x on ice). Slick walls barely slip now.
Also swept -Wall -Wextra across game.c + all modules: the flagged warnings are
benign (multi-statement lines, zero-init omitted fields, unused locals); verified
ex_combat_raycast is safe with 0 targets (the PIERCE geometry cast), encounter
count is bounded by EX_MAX_ENEMIES, g_surface_grip is initialised + updated before
use, and g_build is normalised (no scale-box). All 41 tests pass; game.c clean.

## Update 33 -- bug hunt: infinite mini-bosses + off-screen menu rows
Two real bugs found and fixed:
1. INFINITE NAMED MINI-BOSSES: the champion was designated whenever g_champion_slot<0
   and the death handler reset it to -1, with no "already spawned" guard -- so killing
   Cryotek/Blackstar just promoted the next grunt into a fresh x8-HP named boss, over
   and over until every grunt was dead. Added g_champion_spawned latch (set on
   designation, reset only at deploy) so exactly ONE named mini-boss spawns per match.
2. OFF-SCREEN MENU ROWS: menu_list_start_row clamps the list start to grid row 3, but
   the VGA grid is only 16 rows tall, so any list longer than ~13 rows drew its tail at
   y>=16 (off-screen). The customize screen (20 rows) had paint/finish/signature AND the
   two new weapon-addon slots AND "done" all invisible. Rewrote the menu render to
   SCROLL to the cursor (keeps the highlighted row on-screen) with ^/v overflow hints.
Also swept all sources with -Wmaybe-uninitialized (clean) and verified the new fire
modes don't index g_weapon_models[5] out of bounds (both sites clamp). All 41 tests pass.

## Update 33 -- bug hunt: class HP + speed multipliers were dead
Found a significant gameplay bug: every player class had IDENTICAL health and
movement speed, gutting the tanky-vs-fragile roster identity. Root cause: the
class table (pclass.c) defines hp_mul (0.70 Pyre .. 1.45 Warden) and speed_mul per
class, but game.c never applied them -- g_player_max_hp sat at its 100.0f default
and g_class_speed_mul at 1.0f (both only ever read, never assigned). So a "durable
all-rounder" Prime and a "fast, fragile" Scout had the same 100 HP and same speed.
Fix: at deploy, g_player_max_hp = 100 * ci->hp_mul, g_class_speed_mul =
ci->speed_mul, and g_player_hp starts at the (now class-scaled) max. Warden ~145hp,
Pyre ~70hp; Scout/Interceptor now genuinely faster, Vanguard/Warden slower -- all
already wired downstream (speed at max_spd/jump, hp at every heal/damage clamp).
All 41 tests pass; game.c compiles clean.

## Update 34 -- distinct gun models per fire mode (weapon designs)
The cannon only had 5 meshes (base modes bolt/burst/beam/wave/lob); the five newer
fire modes all fell back to the plain bolt model. Generated 5 new gun silhouettes
via gen_spine.py, each matched to its mode:
- weapon_pierce   -- railgun: square breech + a long thin ridged (coiled) rail
- weapon_flak     -- short hex barrel with a flared muzzle (airburst)
- weapon_plasma   -- bulbous round orb-chamber cannon (WFC riot-cannon vibe)
- weapon_ricochet -- twisted angular square slug barrel
- weapon_suppress -- finned tapering triangular beam emitter
Extended g_weapon_models 5->10, load all ten (order matches ex_fire_mode_t), and
the cannon now indexes the real mode (was clamped to 0..4). Combined with the
addon-driven reshaping already in place (barrel length / girth / heat glow), your
gun now reads its form AND its addons. All meshes structurally validated
(test_spine_iqm); data archive repacks them. All 41 tests pass; game.c clean.

## Update 35 -- IQM pipeline abstractions (gen_spine): sections + radial symmetry
Expanded the headless mesh generator for artistic flexibility -- a force multiplier
for every model/character/prop/weapon:
- FIVE new cross-sections: oct (octagon), diamond (rotated square), blade (wide
  fin), plus (cross), gear (10-tooth cog). Section vocabulary 5 -> 10; also mapped
  into SEC_BY_NUM (6-0).
- RADIAL SYMMETRY op `radial N`: emits N copies of the whole spine fanned around
  the vertical axis (1..24). Enables cogs, turbines, crowns, thruster clusters,
  muzzle brakes -- radial forms the old mirror (2x) couldn't make. Bake refactored
  to compose reflect + rotate per copy.
- Help/--list-presets updated with the new sections + radial.
Used the new sections to upgrade three weapon designs (drop-in, same filenames):
plasma now an octagon orb-chamber, suppress a wide bladed fin-emitter, ricochet a
twisted diamond barrel. Added tools/test_gen_spine.py (self-test: all 10 sections +
radial 3/6/8/12 + all 5 presets bake to valid IQMs) -- passes. No regression on
existing presets. Full game suite (41 tests) passes; data archive repacks the
improved meshes.

## Update 36 -- weapon menu readout (accessibility) + WAVE/LOB implemented (bug)
Two threads:
1. WEAPON ACCESSIBILITY (menus): when customizing a weapon-addon row (11/17/18),
   a live readout now shows on the right of the customize screen -- the resulting
   fire MODE plus net dmg/rate/range multipliers vs a standard gun, pellets/chains
   if any, and the three equipped addon names (dimmed if empty). You can now SEE
   what an addon does before committing.
2. BUG + weapon variety: WAVE and LOB fire modes were never implemented in the
   firing loop -- they fell through to a plain bolt, so TSUNAMI (wave) and MAGMA
   (lob) forms fired wrong. Implemented both: WAVE is a short-range AoE PULSE (a
   forward fan that damages + shoves + slows, no travel); LOB is an arcing lobbed
   shot that lands ahead and BURSTS (16u splash) leaving scorched footing that
   lingers (a slow). (BEAM-as-hitscan was already correct.) All 41 tests pass;
   game.c compiles clean.

## Update 37 -- JHIAXUS mini-boss (mission 4 boss; completes the champion trio) + bug sweep
CHARACTER/CAMPAIGN: added the third named mini-boss via the generalized champion
system (kind 3). On the vault map -- campaign mission 4 "what jhiaxus grew" (1 grunt
+ 3 corrupted) and skirmish vault -- the grunt rises as JHIAXUS, architect of the
Corrupted: x8 HP, x2.3 scale, CABLE rig (reassembly tendrils), corruption-violet
glow, announced "JHIAXUS -- the architect of the corrupted". Signature CORRUPTION
SURGE bites you at range AND mends (+15% hp) + EVOLVES nearby Corrupted (bumps their
drift generation a tier) -- his adds get worse the longer he lives, distinct from
Cryotek's push-nova and Blackstar's pull-harpoon. Falls with "JHIAXUS falls -- the
corruption stills". All champion branches now 3-way. Campaign boss arc:
Cryotek(m1)/Blackstar(m2)/Jhiaxus(m4)/devourer(finale).
BUG SWEEP (no new bugs; verified clean): g_corrupt_gen + g_corrupt_gen_next reset
per match (no difficulty creep), champion designation sets HP/max correctly, the
star multiplier is 1.0 baseline for normal shots (WAVE/LOB damage correct). All 41
tests pass; game.c compiles clean.

## Update 38 -- new map: THE TRENCH (beneath the pacific)
Added an 11th arena theme, script-faithful to mission 6's "beneath the pacific".
New theme block in tools/gen_world.py: obsidian abyssal floor, tall rock walls, a
scattered field of 14 hydrothermal VENT SPIRES (uneven heights, center kept clear),
a ring of 6 tilted buried Cybertronian MONOLITHS, and a central rift dais with a
glowing energon rift-lip (story landmark: the buried devourer's edge). Regenerated
world maps -> trench.iqm (868v, valid) + trench.col (24 blockers, auto-emitted).
Wired everywhere: NUM_THEMES 10->11 + g_theme_ids (game.c), MAP_IDS + MAP_NAMES
("the trench") so it's selectable in skirmish (menu.c), and a distinct atmosphere
(dim abyssal-cyan key light, deep-ocean-black void). All textures pre-existing.
Data archive repacks it. All 41 tests pass; game.c compiles clean.

## Update 39 -- procedural texture pipeline (gen_texture.py) + new trench materials
Added the missing asset type: a procedural, seamlessly TILEABLE texture generator
(tools/gen_texture.py) -- the textures were all hand-authored PNGs before. Six
pattern types, each parameterized by base/accent colour + seed: plating (riveted
panels), hexmesh (tech mesh), circuit (traces + vias), rock, sediment (abyssal
seabed w/ cracks), crystal (toroidal-voronoi facets + a paired glow/emission map).
Tileability is guaranteed: all noise is FFT band-limited so it wraps at the edges;
grid patterns use modular coords sized to divide 256. Self-test tools/test_gen_texture.py
confirms all six tile (edge-seam check) -- passes.
Used it concretely: generated seabed.png / abyssrock.png / riftcrystal.png
(+emit_riftcrystal.png) and re-skinned the TRENCH map with them (abyssal seabed
floor, dark vent-rock spires, a glowing crystal rift) -- so the newest map now runs
on brand-new procedural textures. Data archive repacks them. All 41 tests pass;
game.c compiles clean.

## Update 40 -- new playable class: SIEGE (artillery bombard) + mortar signature
CHARACTER DESIGN: added a 10th player class, WFC/FOC-inspired artillery.
- CLASS SIEGE (pclass): "artillery bombard -- long-range shells, slow but sturdy".
  hp_mul 1.25 / speed_mul 0.75 (durable, ponderous); vehicles tank/gunship/crab/mech;
  heavy bulk (105-155, base 135).
- SIGNATURE EX_SIG_MORTAR: a heavy dorsal mortar, mesh spine_mortar.iqm built with
  the EXPANDED pipeline (gear breech -> flared octagon barrel -> circle muzzle).
  stats {dmg 1.45, refire 1.35 (slow), taken 0.82 (sturdy), speed 0.80}; combat
  {knockback 14, splash 11} -- every shell shoves + airbursts wide.
- 1:1 class:signature invariant preserved -> now 10 classes / 10 distinct signatures.
Wired across all 6 touch-points: signature.h/.c (mesh/for_class/name/stats/combat/
robot-pose; vehicle poses fall to the valid category defaults), pclass.h/.c (enum +
table row), game.c (g_tpl_mortar load + g_sig_tpls[11] + sig_template case), menu.c
(SN name array). test_signature: 10 distinct sigs, 210 pose pairs verified;
test_class: 10 profiles. All 41 tests pass; game.c compiles clean.

## Update 41 -- bug hunt: the ability loadout was a dead parallel system
Traced a real disconnect. The menu lets you pick 3 abilities (loadout.abil[]) ->
loaded into g_abilities, but g_abilities was consulted in exactly ONE place: an
OVERCLOCK check that used ex_ability_active_any (true only while active_timer>0).
Nothing ever activates a loadout ability and ex_loadout_update was never called, so
that check could never fire -- the menu ability selection did nothing. (The actual
in-game abilities -- blink=Y, pulse, turret=U, dash, overcharge pickup -- are
separate hardcoded always-on systems.) So verified: g_sig_combat + g_sig_stats all
applied, SIEGE vehicle indices valid, trench spawn clear of its colliders,
MAX_MAP_COL fine.
FIX (makes menu picks matter, safe + additive): added ex_loadout_has() (equipped
check), and turned two selectable perks live -- OVERCLOCK equipped = permanent +25%
fire rate (was the dead line), SHIELD equipped = deploy with 40 armor. Also now call
ex_loadout_update each frame so the system ticks.
KNOWN/NEXT: the other selectable perks (hover/turret/dash/cloak) are still cosmetic,
and blink/pulse/resonance aren't menu-selectable at all (enum ids > the picker's
0-6) -- a fuller ability-loadout wiring is the next lead. All 41 tests pass.

## Update 42 -- finish wiring the ability loadout (+ revive the dead turret)
Continued last round's fix so the menu ability picks ALL matter now. Added safe,
additive equipped-perks via ex_loadout_has (none remove the always-on hardcoded
moves):
- DASH   -> +12% move speed
- HOVER  -> +25% jump height
- CLOAK  -> enemies sense you at 0.6x range (run quiet)
(joining OVERCLOCK +25% fire rate and SHIELD +40 deploy armor from Update 41).
BUG (second dead system found): the TURRET was never deployable -- ex_turret_deploy
(the only thing that sets g_turret.active=1) was never called from game.c, so the
entire turret block (auto-fire + 6 ammo types) was dead code and the T key is
actually Pulse Blast, not turret. Wired it: equip TURRET, press U to deploy a 12s
auto-firing sentry (16s redeploy gate); while it's out U still cycles its ammo.
So all 7 selectable abilities (hover/turret/shield/dash/cloak/overclock + none) now
have a real effect. All 41 tests pass; game.c compiles clean.

## Update 43 -- campaign: new mission 6 "the drowned approach" (the Trench)
Wired the Trench map (Update 38) into the campaign and grew the arc 6 -> 7 missions
(EX_MAX_MISSIONS 6->7). Inserted between "the mantle dive" (Rodimus rises) and the
finale, so the descent through the pacific abyss now has its own mission instead of
jumping straight to the seabed:
  5 the mantle dive (foundry)  ->  6 the drowned approach (TRENCH, NEW)  ->  7 beneath the pacific (cybertron)
Mission 6: SURVIVE 70s on the trench with 1 grunt + 4 corrupted converging on the
rift -- thematically Jhiaxus's freed Corrupted massing on the drowned wound (ties to
mission 4). Five story slides bridge "the tide turned" to the reassembler stirring;
the finale renumbered to "7 _ beneath the pacific". No champion on trench (a hold-
the-line swarm, boss is saved for the finale). All 41 tests pass; game.c clean.

## Update 44 -- weapon addons: FLAK / PLASMA / SUPPRESS conversion cores
Completed the "any form -> any fire mode" customization. The addon roster already
had PIERCE_CORE (->rail), RIPPLE_COIL (->ricochet), SCATTER_BARREL (->burst); added
the three missing conversion cores (weaponmod.h/.c):
- FLAK_CORE     -> EX_FIRE_FLAK    (airburst splash; +cooldown, -10% dmg)
- PLASMA_CORE   -> EX_FIRE_PLASMA  (heavy electric blast; +15% dmg, +chain, slow refire)
- SUPPRESS_CORE -> EX_FIRE_SUPPRESS(long freeze-beam; +30% range, -30% dmg, heavy slow)
Now any of the 12 forms can be reshaped into any of the AoE/utility fire modes via a
loadout slot. The per-mode gun MODEL and the menu weapon-stat readout already key off
the resulting mode, so equipping a core also swaps the on-back cannon + updates the
readout automatically -- no extra wiring.
Menu fixes: the addon picker was hardcoded to 13 (WMOD_NAMES[14], %14, bound v>13) so
the new cores were unreachable -- bumped to 17 names / %17 / bound 16 across all three
addon slots. Bonus: fixed ex_map_desc's stale i<9 guard (reclaim's description was
being dropped) and gave the Trench a map description. test_weapon covers all three
cores. All 41 tests pass; game.c clean. Addon roster now 16 (+none).

## Update 45 -- bug hunt: chassis transform-speed multiplier was dead
Dead-data sweep of the chassis system. Of its fields, the six proportion muls +
fold_stagger + fold_spin are all applied, but xform_dur_mul (transform-duration
multiplier, per chassis: Standard 1.0 / Angular 0.78 / Juggernaut 1.35 / Strider
0.92) had ZERO references -- chassis choice didn't change how fast you fold, even
though a Juggernaut is meant to transform ponderously and an Angular frame snaps
fast. Same class of bug as the dead class hp_mul.
FIX: threaded a dur_mul through the transform system -- new float on ex_xform_t
(default 1.0, set from ex_chassis_info(chassis)->xform_dur_mul at deploy);
ex_xform_request now starts blend_timer = EX_XFORM_BLEND_SEC * dur_mul, and both
morph-progress calcs in game.c divide by the scaled duration so the animation stays
0..1. Enemies keep the 1.0 default. So the chassis now genuinely tunes fold speed.
NOTE: greeble_boost is also unreferenced, but the body greeble pass already requests
the full DETAIL_MODELS(420) pool, so it's capped out -- can't add detail without
enlarging the pool; logged as a known cosmetic dead field. All 41 tests pass.

## Update 46 -- IQM pipeline abstractions: scale (non-uniform sections) + bend + arrow
Expanded gen_spine.py again (force multiplier for every model). Three new ops:
- scale SX SY : non-uniform cross-section scaling -- flatten a tube into a plate/fin
  (scale 1 0.3) or stretch it into a tall oval (scale 0.5 1). Applied per-ring after
  the twist, inherited by branches.
- bend DEG    : curve the growth direction a little every grow step -- horns, tusks,
  hooks, ram arcs, swept blades (pairs with taper/blade for organic curves).
- arrow       : an 11th cross-section (a chevron pointing +x) for fins / blade heads
  / arrowed spikes.
Help/--list-presets updated; tools/test_gen_spine.py now also generates scale-flat,
scale-oval, bend-horn and arrow parts and asserts each bakes to a valid IQM -- all
pass, no regression on the existing sections/radial/presets. No shipped assets or
game code changed this round, so the 41-test suite is unaffected (re-run: still 41/41).

## Update 47 -- skirmish: new STALKERS mode + mode descriptions
Skirmish had two modes (arena fight = 8 waves, survival = endless). Added a third by
surfacing a rich system that was locked to a single campaign mission:
- EX_MODE_STALKERS: the horror/stalker ruleset (heavy fog, a dread meter, and
  enemies that FREEZE while in your view cone and only advance when unwatched) was
  previously reachable ONLY in campaign mission 2. Now it's a skirmish mode playable
  on any of the 11 maps -- endless waves, wired via g_horror_mode when
  !in_campaign && mode==STALKERS.
- Mode-select screen now shows a one-line DESCRIPTION per mode (like the map screen):
  arena "8 escalating waves - clear them all", survival "endless waves - last as long
  as you can", stalkers "endless, in fog - they freeze when watched, close in when
  not" (new ex_mode_desc()). EX_GS_MODE row count 2->3.
Wired: menu.h/.c (enum, name, desc), game.c (endless-waves + horror-mode gating).
All 41 tests pass; game.c clean.

## Update 48 -- more structure variation (runtime cover + map primitives)
Two layers of new structure variety:
1. RUNTIME COVER (affects every map, every match). The flow-graph blueprint volumes
   used only 3 meshes (rock / spire / box). Now:
   - cover blocks alternate rock <-> boulder (both centre-origin) so adjacent cover
     no longer looks stamped from one mould;
   - vantage masses alternate tapered spire <-> standing column (both base-origin,
     ground-anchored);
   - CONCEAL "solid mass" volumes route to the varied rock meshes instead of plain
     boxes -- fewer featureless cubes.
   (Checked mesh origins first: dome/column are base-origin, rock/boulder centre-
   origin, so only origin-compatible meshes were paired to avoid float/sink.)
2. MAP BUILDER primitives (gen_world.py): new arch(w,h,d) gateway (two legs + lintel)
   and ring(r,n,ph) pillar henge/colonnade -- both composite of collidable boxes.
   Applied: wastes gains a ruined 9-pillar henge around the dead relay; cybertron
   gains two monumental arch gateways on the approach. Maps regenerated + validated
   (wastes 24 colliders, cybertron 39); data archive repacks them.
All 41 tests pass; game.c compiles clean.

## Update 49 -- fix: greebles now move with the body + film: Unicron's gravity well
BUG ("parts of the bots don't move with the rest"): the base body parts get a
motion LEAN (upper parts shift proportional to height, pivoting at the feet) and a
fold-SPIN during transform, but the greeble surface-detail pass only inherited the
body yaw -- so the little panels stayed pinned to the un-leaned origin and visibly
slid off the body when it moved, turned, or transformed. Fixed: the greeble pass now
applies the SAME height-proportional lean (g_lean_side/g_lean_fwd, zeroed during the
fold like the parts) and the same xf_spin to its yaw. Detail now rides the body.
FILM (from docs/blackstar_chat_digest.txt -- "Unicron's gravity well begins tearing
apart Earth's orbiting satellites"): added UNICRON'S GRAVITY WELL to the finale.
While the planet-god boss lives, the well periodically surges (~every 9s, 2.6s each):
it drags the player toward the boss core (24 u/s pull, falls off inside 4u), shakes
the camera, and announces "UNICRON'S PULL -- the world tips inward". Timers reset per
match; only fires while the boss is present. All 41 tests pass; game.c clean.

## Update 50 -- more map assets + richness + gunplay spread
ASSETS + MAP RICHNESS:
- 4 new decorative prop meshes via the gen_spine pipeline (scale/bend): deco_slab
  (flat debris), deco_pipe (bent conduit), deco_antenna (tapered mast), deco_crystal
  (diamond shard). All structurally validated.
- Denser far skyline: DECO_ROCK 14->18, DECO_TALL 10->14.
- NEW mid-ground prop pool (DECO_PROP=20): scatters the 4 props in the empty band
  (r 48..94) between the play cover and the far skyline -- slabs lie low, antennae
  stand tall, crystals catch the sky tint. Purely visual (no colliders) so it adds
  richness without cluttering the fight. Data archive repacks the meshes.
GUNPLAY: movement-based accuracy. Fire now opens up with your speed -- move_spread =
min(speed*0.006, 0.052 rad) (~0 standing, ~3 deg sprinting), applied as a per-pellet
random yaw/pitch jitter (own LCG). Standing still is now pinpoint; run-and-gun
trades accuracy. Layers on top of the existing per-mode fan spread.
All 41 tests pass; game.c compiles clean.

## Update 51 -- more character frames (size shapes) + frame-driven detail
CHARACTER SIZE SHAPES: added 3 new chassis frames (4 -> 7), each a visibly distinct
build via its proportion multipliers (h/bulk/arm/leg/head/shoulder):
- RAPTOR   -- lean, tall, long-legged predator (bulk 0.78, leg 1.32); fast fold (0.80)
- BRUISER  -- short, wide, huge arms/shoulders brawler (bulk 1.38, arm 1.40, shoulder
              1.45, leg 0.84); heavy fold (1.28)
- SENTINEL -- very tall, spindly watcher (h 1.34, bulk 0.76, thin limbs); tall glow
The builder already reads name from ex_chassis_info and cycles by EX_CHASSIS_COUNT,
so all three appear in the "frame" selector automatically. Their xform_dur_mul feeds
the transform-speed system wired in Update 45.
MODEL DETAIL: revived the dead greeble_boost chassis field -- the body greeble pass
now scales its surface-detail density by the frame's greeble_boost (base ~300 panels,
up to the full 420 pool at boost 2). Bruiser wears the most panels, lean frames
fewer. test_class now reports the frame count dynamically. All 41 tests pass.

## Update 52 -- [script push 1/many] new map: PLUTO STATION (the pluto line)
Pluto is the script's single most-referenced location (the film opens "securing the
solar system's defenses out on the space bases of Pluto" / the Solar Defense Grid),
but campaign mission 1 "the pluto line" was borrowing the frostline map. Built a
dedicated map: the outer containment station at the dark edge of the system --
a ring of 12 defense PYLONS (new ring primitive), north/south gate ARCHES (arch
primitive), a central command dais + containment PILLAR sealing a glowing crystal
CORE (the thing being held), and scattered sensor masts. 12th theme.
Wired: gen_world theme -> pluto.iqm (valid) + pluto.col (17 blockers); NUM_THEMES
11->12 + g_theme_ids; MAP_IDS/NAMES/DESC ("pluto station"); a cold, near-black
far-sun atmosphere; and campaign mission 1 now deploys on pluto. Moved CRYOTEK (the
ice-base opening villain) to designate on pluto too, so mission 1 still has its boss.
All 41 tests pass; game.c clean. Data archive repacks it.

## Update 53 -- [script push 2/many] new map: THE BEACH (the beach opens)
Mission 3 "the beach opens" -- the film's signature disaster shot, Earth's shore
tearing apart as energon breaches the mantle -- was borrowing the wastes map. Built
it: a sand shore (new procedural sand.png texture) split by a jagged glowing ENERGON
TEAR down the middle (9 non-colliding glow segments, sine-jagged) flanked by HEAVED
broken ground (colliding slabs shoved up on both sides of the rip), a COLLAPSING PIER
of pilings marching out to the edge at progressively shorter heights, and scattered
beach debris. 13th theme; warm dusk-over-ash atmosphere. Wired NUM_THEMES 12->13 +
g_theme_ids, MAP_IDS/NAMES/DESC ("the beach"), atmosphere, and campaign mission 3 now
deploys here. beach.iqm valid + beach.col (30 blockers). All 41 tests pass.

## Update 54 -- [script push 3/many] new activity: DEFENSE (hold the line)
New skirmish mode EX_MODE_DEFEND -- the film's Solar Defense Grid as a gameplay
activity. Endless waves converge on a central CORE (1000 HP); any enemy that breaches
within 12u of it drains its health (grunts 9/s, an apex 26/s), pulsing distress
shockrings from the core as it's hit. You lose the moment the core falls ("THE LINE
IS BROKEN"), win by holding as long as you can -- score/wave driven like the other
endless modes. HUD shows a live CORE % readout (turns red on hit). Pairs naturally
with the new Pluto map (its containment core is literally the thing you defend).
Wired: menu.h enum + menu.c name "defense"/desc "hold the line - defend the core
against endless waves"; game.c globals (g_defend_mode, g_core_hp/max, hurt+ring
timers), deploy init, per-frame drain+defeat pass, and the HUD line. All 41 tests
pass; game.c clean.

## Update 55 -- [script push 4/many] gameplay: ENERGON CACHE SURGES
The mantle energon that woke Unicron, as a live environmental hazard. On energon-rich
maps (beach, emberpit, foundry, pluto) raw energon periodically erupts from the
ground: it telegraphs with a growing warning ring (~1.3s), then bursts. The burst
BURNS the player if caught in it (26 dmg + knockback) -- but energon is the bots'
lifeblood, so any enemies within the blast get REFUELLED (+25% max HP). This creates
a real tactical read: don't stand on a surge, and don't let the swarm cluster on one
to heal. Fires every ~8-12s at a random point in the play area, telegraph + erupt +
VFX, announced "ENERGON SURGE -- the cache stirs". Globals + deploy gate (set once the
map id is known) + a self-contained update cycle using the existing player_hit and
shockring/vfx systems. All 41 tests pass; game.c clean.

## Update 56 -- [script push 5/many] new map + campaign beat: MARS (the red extraction)
Added the film's Mars fragment site and threaded a mission through the campaign.
MAP (14th theme): a Cybertron FRAGMENT half-buried at center (tilted metallic mass +
jutting shard + a cracked glowing seam), EXTRACTION derricks + rig pads ringing the
dig, and red rock mesas for cover on a new procedural mars.png (red rock). Cold-to-
warm rusty daylight + red dust sky. mars.iqm valid + mars.col (16 blockers).
CAMPAIGN (7 -> 8 missions, EX_MAX_MISSIONS 7->8): inserted "3 _ the red extraction"
between the scavenger king (outer system) and the beach (Earth) -- the scavengers'
trail leads sunward to a Cybertron shard on Mars; objective BREACH (tear the
extraction rig apart), but the shard turns out hollow and already gone, its
trajectory burning sunward to Earth -- handing straight off to "the beach opens".
Renumbered missions 3-7 -> 4-8. Wired map (NUM_THEMES 13->14, ids, menu, atmosphere).
All 41 tests pass; game.c clean. Data archive repacks it.

## Update 57 -- back-end: multi-relational, data-driven content system
Big developer-facing back-end: content (troops, maps, modes, missions, abilities,
weapons, chassis, signatures, animations) is now describable as JSON RELATIONS in
content/, resolved + validated by a compiler -- so variations can be authored by
recombination without touching C or art.
- content/part_groups.json exposes the IQM meshes as named GROUPS (head, cannon,
  backpiece, structure, prop...) with slot + origin. Everything else references
  meshes by group+index, so re-skinning is a one-line change.
- content/{chassis,signatures,weapons,abilities}.json = the building blocks (mirror
  the C tables); content/troops.json = the rearrange layer (17 troops incl. 10 player
  classes, 4 enemies, and 3 NEW variant troops composed purely by reference:
  ashfall_lancer, hoarfrost_sentinel, magma_breaker); content/{maps,modes,missions,
  animations}.json = the world/arc/feel data.
- tools/content_build.py resolves the graph and VALIDATES every relation: meshes
  exist on disk, troop refs resolve, backpiece/cannon indices in range, addon
  converts_to real, mission map/objective/champion valid, mission order clean 1..N,
  and every 'enum' cross-checked against the C headers (drift guard -- already caught
  EX_SIG_WING -> EX_SIG_WING_CANNON). Emits content/_manifest.json (flattened graph).
- tools/test_content.py self-tests (valid passes + broken refs caught); wired as a
  ctest 'content_validate' so a dangling reference now FAILS THE BUILD. Suite 41->42.
- content/README.md documents the graph + how to add content + the art/campaign/
  weapon/vehicle design directions the system opens up.
Pure additive: no C gameplay changed. 42/42 tests pass.

## Update 58 -- content system: JSON->C codegen + verified single-source-of-truth
Pushed the content layer from "validated mirror" to "VERIFIED mirror" of the C tables.
- content_build.py gained --emit-c: it generates engine_src/exengine/content_gen.h
  from content/chassis.json (a real C table: ex_gen_chassis[7]).
- tests/test_content_gen.c compiles that generated header alongside the hand-written
  chassis.c table and asserts they match value-for-value (proportions, fold_spin,
  xform_dur, greeble). Registered as a ctest -> if the JSON and the C table ever drift
  apart, the build FAILS. Suite 42->43.
- Reconciling for this exposed real drift: content/chassis.json's angular + juggernaut
  rows were slightly off the C values (authored from memory); corrected to match
  chassis.c exactly, so the mirror is now faithful.
This is the first step to single-source-of-truth: the JSON is provably in sync with C,
and the same --emit-c path can later emit the tables the game actually compiles. Pure
additive, no gameplay change. 43/43 tests pass.

## Update 59 -- graphics: shader fidelity pass (lighting + post)
Pushed the deferred pipeline (it already has g-buffer + SSAO + shadow-mapped point
lights) toward a richer look via two contained shader upgrades. Both are runtime-
loaded GLSL, packed into data.ex; the C side is untouched.
- gmain.glsl (deferred lighting resolve): replaced the flat 6% ambient with
  HEMISPHERE AMBIENT -- a cool sky fill from above graduating to a warm-dark ground
  bounce from below, tinted by albedo and driven by the world-space normal -- plus a
  soft FRESNEL RIM that catches every silhouette edge. Far more form-reading depth
  than a constant term; average brightness kept near the old value to avoid blowout.
- fboshader.glsl (post): after the existing exposure + ACES tonemap, added a filmic
  GRADE (gentle contrast lift + ~12% saturation), a subtle VIGNETTE, and ordered
  DITHER to break up 8-bit gradient banding.
Validated GLSL syntax headlessly (balanced braces/parens code-only; intact VS/FS
structure; only standard funcs + already-present uniforms). HONEST LIMITS: no GPU
here, so these are verified to be well-formed and packed, but NOT visually confirmed;
"AAA" is a direction, not a one-shot -- values (ambient 0.34, rim 0.35, sat 1.12,
vignette 0.5) are tuning constants for a live pass. 43/43 tests pass.

## Update 60 -- graphics wiring: normal mapping made real (tangents + TBN + normal maps)
The engine already had the normal-map plumbing (gbuffer samples u_norm, does TBN; the
loader auto-derives norm_<diffuse>; renderer binds unit 6) -- but it did NOTHING,
because (a) our procedurally-generated IQMs carry NO tangents, so the TBN was
degenerate, and (b) no normal maps existed to sample. It only looked fine because a
flat default normal collapses TBN*(0,0,1) back to the geometric normal. Wired all
three missing links:
1. TANGENTS AT LOAD (util/iqm.c): when an IQM ships without a tangent array (all of
   ours), derive per-vertex tangents from the UV gradient (Lengyel accumulation +
   Gram-Schmidt against the normal). Fixes every mesh in one place, no re-export.
2. CORRECT TBN (gbuffer.glsl VS): the bitangent misused tangent.w in a point
   transform; replaced with re-orthonormalized T and B = cross(N,T)*handedness.
3. NORMAL MAPS (gen_texture.py): added to_normal_map (tileable Sobel height->normal)
   + a --norm flag, and batch-derived norm_<name>.png for all 58 diffuse textures
   (auto-picked up by the loader). 59 now pack into data.ex.
Safe by construction: with a flat/near-flat normal map the result is identical to
before; real relief only shows where the derived map has detail. C compiles, shaders
balanced, 43/43 tests pass. HONEST: not GPU-verified -- validated structurally.

## Update 61 -- graphics: bloom (HDR glow on bright/emissive pixels)
Added bloom to the post pass. Before the existing exposure+ACES tonemap, the shader
now gathers bright HDR energy from a 7x7 neighborhood (spread 2.5 texels, Gaussian
weights) through a soft knee (smoothstep 0.75..1.7 on max channel) so only genuinely
bright/emissive pixels bleed -- energon seams + surges, containment/fragment cores,
muzzle flash, the new fresnel rim highlights -- and adds it back at 0.7 strength IN
HDR so it blooms and tonemaps naturally.
This is a single-pass approximation living entirely in fboshader.glsl (no renderer
changes): a true bloom would downsample + ping-pong blur across FBOs, but that's a
risky renderer rewrite I can't GPU-verify, whereas this needs zero C changes and can
only add glow. 49 taps/pixel, one fullscreen pass -- fine for a stylized target.
Shader balanced, C untouched, 43/43 tests pass. HONEST: not GPU-verified; strength
0.7 / knee 0.75..1.7 / spread 2.5 are one-line tuning constants.

## Update 62 -- graphics: contact-hardening soft shadows (PCSS-lite)
Upgraded the point-light shadows from a flat 20-tap PCF to PCSS-lite. First a blocker
search averages the depth of the occluders in front of the receiver; the penumbra
then widens with the gap between receiver and blocker (radius *= 1 + penumbra*6), so
shadows stay crisp where a bot's feet meet the ground and soften realistically with
distance -- the contact-hardening look, instead of one uniform blur. Early-outs to
fully lit when no blocker is found (skips the filter entirely). Contained to the
shadow term in gmain.glsl's calc_point_light; shader balanced, C untouched.
43/43 tests pass. HONEST: not GPU-verified; penumbra*6 + blocker-search radius*2 are
tuning constants.

## Update 63 -- graphics: FXAA (edge anti-aliasing)
Closed out the AA item. Switched the post-source framebuffer color buffer from
GL_NEAREST to GL_LINEAR (renderer.c) so sub-texel taps interpolate -- needed by both
FXAA and bloom. Added FXAA to the post pass: a 5-tap luma edge-detect (center + NSEW)
gates on a real luma-range edge (>0.055), then a directional blend along the gradient
with two sample pairs, rejecting sub-pixel overshoot by clamping the blend to the
local luma range. Softens geometry jaggies before tonemap.
Single-pass approximation (runs on the pre-tonemap buffer rather than a dedicated
post-tonemap FBO pass) -- chosen over renderer surgery I can't GPU-verify; it's
contained to fboshader.glsl + a one-line filter change. Shader balanced, C builds,
43/43 tests pass. HONEST: not GPU-verified; edge threshold 0.055 / blend 1.5 texel
are tuning constants.
This completes the requested graphics wiring/upgrades: normal mapping, bloom, soft
shadows, FXAA -- atop hemisphere ambient + fresnel rim and the filmic post grade.

## Update 64 -- IMPORTANT: test suite was running with assertions stripped (fixed)
While adding a headless verification for the new tangent code, discovered the build
defaults to CMAKE_BUILD_TYPE=Release, which defines NDEBUG -- and NDEBUG strips every
assert(). So the assert-based checks across the whole test suite have been compiled
OUT: the tests compiled and ran (catching crashes + compile errors) but their actual
assertions never fired. Prior "N tests pass" was therefore much weaker than presented
for any assert-based test. This is now fixed.
- FIX: target_compile_options(... -UNDEBUG) on every test target so assertions run
  regardless of build type. Verified they now genuinely fire (a deliberate false
  assert aborts).
- Enabling asserts surfaced 5 real hidden failures; fixed all:
  * test_tangents (NEW, below) caught a REAL bug in the new tangent generator: the
    degenerate-UV fallback (1,0,0) wasn't perpendicular to the normal, and non-unit
    normals broke the Gram-Schmidt. Fixed ex_compute_tangents to normalize the normal
    and pick an orthogonal fallback; now finite+unit+orthogonal on all meshes.
  * test_gait: demanded zero idle bob, but idle breath/sway is a real feature ->
    assert moving bobs >2x more than idle instead.
  * test_tsunami: measured distance from the front position, which resets to origin
    once the wave spends -> assert the tracked .travelled field reached range.
  * test_weapon: summon window exactly matched the 12s timer; float accum just missed
    -> widened to 14s.
  * test_menu: hardcoded camp.count==4, but the arc legitimately grew to 8 missions
    -> assert 0 < count <= EX_MAX_MISSIONS.
NEW: engine_src/exengine/util/tangents.h (pure, testable ex_compute_tangents, now
used by iqm.c) + tests/test_tangents.c (synthetic quad + real IQMs). Suite 43->44,
all passing WITH assertions live.

## Update 65 -- automate the recurring "dead data" bug + fix 2 more it found
The most-repeated bug in this project is config fields set in a data table but never
read (dead class hp_mul, chassis xform_dur_mul, greeble_boost -- all found by hand).
Turned that manual hunt into a tool.
- NEW tools/lint_dead_fields.py: for each config struct (ex_chassis_info_t,
  ex_class_info_t, ex_sig_stats_t, ex_sig_combat_t) it extracts the fields and checks
  each is actually read via ->field/.field somewhere in the C sources; a field with
  zero reads is flagged as dead. Registered as a ctest (lint_dead_fields) so dead
  data now FAILS THE BUILD.
- It immediately caught 2 real dead fields: ex_class_info_t.base_bulk and
  base_height -- each class defines its default proportions (Vanguard bulky ~140,
  Scout lean ~75, Titan tall 115) but they were never applied; the loadout just
  defaulted everyone to 100 and only clamped bulk on class switch. FIXED: switching
  class now seeds loadout.bulk (clamped) + loadout.height from the class's base_bulk/
  base_height, so each class starts at its characteristic build.
Now that assertions actually fire (Update 64), these gates are real. Suite 44->45,
all passing; lint clean (26 config fields, all applied).

## Update 66 -- FIX: "default cube" attachments (zero-scale greebles on collapsed parts)
User-reported: strange default cubes stuck to models (character back/chest + world),
unmoving while the body animates. Root cause traced:
- The renderer applies scale3 only if it's non-zero (renderer.c:265); otherwise it
  falls back to model->scale (default 1.0) -> the mesh renders at its native full
  2-unit size (a default cube).
- EX_PART_BACK is deliberately zeroed (sx=sy=sz=0) because the signature draws it
  separately -- but ex_detail_generate forced >=1 greeble per part (if quota<1
  quota=1), even on a zero-area part. That greeble got 0 dimensions -> scale3=(0,0,0)
  -> rendered as a full 2-unit box stuck on the back. Greeble copies keep box's
  scale=1.0, so the fallback is full size. Base parts already guarded this; greebles
  never did.
FIX (two layers): (1) detail.c skips zero-area parts (no greebles on a collapsed
part); (2) the greeble render hides any greeble whose scale3 collapses <0.02 on all
axes, exactly like the base parts. NEW tests/test_detail.c locks it: no greeble on a
zeroed part, none zero-scale. Suite 45->46, all pass. game.c clean.
NOTE: image viewer was down this session, so this was diagnosed purely from the user's
description + code trace; the fix is verified logically + by the new test, but not
visually confirmed here.

## Update 67 -- graphics QUALITY TIERS (mobile <-> ultra) + honest shader-path audit
Added a graphics-quality preset to Settings so low-end (30fps phone) and high-end
(240fps desktop) can each be served: mobile / balanced / high / ultra.
- Settings: new "quality" row (menu.h field + menu.c row/label/adjust, save/load
  persistence). EX_SETTINGS_ROWS 10->11.
- Plumbing: engine global ex_gfx_quality (mirrors ex_fog_density), driven from the
  setting each frame, passed to the post shader as u_gfx_quality.
- Shader LOD (fboshader.glsl, the ACTIVE post shader): bloom (49-tap gather) and FXAA
  are now GATED -- skipped entirely on mobile (quality 0), full on balanced+. Biggest
  per-pixel post cost drops on phones.
- Detail LOD (game.c): the greeble budget scales with tier (mobile 0.28x ~= 84 panels,
  balanced 0.65x, high/ultra full ~300-420) -- large draw-call reduction on mobile.
AUDIT (honest correction): while wiring uniforms I found the renderer loads
forward.glsl for lighting, NOT gmain.glsl/gbuffer.glsl. So:
  * LIVE: normal mapping (my load-time tangents + generated norm maps feed forward.glsl's
    existing u_norm+TBN), bloom, FXAA, grade/vignette/dither, and the new quality gating.
  * DEAD/redundant: the U59 hemisphere-ambient and U62 PCSS soft-shadows I put in
    gmain.glsl were never loaded; forward.glsl has its own (simpler) ambient + shadows,
    and already had a fresnel rim. Porting the ambient upgrade into forward.glsl next.
All 46 tests pass; shaders balanced; game.c clean. HONEST: quality deltas are logic-/
compile-verified, not GPU/perf-measured here.

## Update 68 -- port hemisphere ambient into the ACTIVE shader (forward.glsl) + quality-gate it
Made the U59 ambient upgrade actually take effect by moving it into forward.glsl (the
shader the renderer really loads). forward.glsl's ambient was a flat 0.12 floor + a
directional sun; replaced the flat floor with a HEMISPHERE fill -- cool sky from above
graduating to a warm-dark ground bounce below, from the view-space normal vs world-up
(no new matrix needed). Gated by u_gfx_quality via step(0.5,...): mobile keeps the
cheap flat floor (identical to before), balanced+ get the hemisphere -- exactly the
"up fidelity on desktop, drop cost on mobile" split. Added u_gfx_quality to
forward.glsl + set it on forward_shader in the renderer (beside u_fog_density).
Now genuinely live in the active pipeline: normal mapping, hemisphere ambient
(quality-gated), fresnel rim, bloom + FXAA (quality-gated), filmic grade/vignette/
dither. Still dead: the gmain PCSS soft-shadows (forward.glsl has its own shadows) --
optional future port. 46/46 tests pass; shaders balanced; game.c clean.

## Update 69 -- wire render-scale (dynamic resolution) + dynamic shadow-map resolution
Two engine-level perf levers, the strongest ones for the 30fps-phone end.
- DYNAMIC RESOLUTION: the render_scale setting (100/75/50%) was inert -- ex_render_resize
  hard-set the internal framebuffer to the full display size. Now it scales the buffer
  by a new ex_render_scale global (display * scale); the scene renders to that smaller
  buffer and the existing post pass upscales it to the window (the buffer is already
  GL_LINEAR from U63, so the upscale is smooth). game.c drives ex_render_scale from the
  setting and re-runs ex_render_resize when it changes. 50% scale = ~1/4 the pixels
  shaded -- the single biggest mobile win.
- DYNAMIC SHADOW RES: SHADOW_MAP_SIZE was a compile-time 1024. Made it a runtime
  ex_shadow_map_size (used by the point-light depth cubemap creation + the shadow-pass
  viewport), driven by the quality tier: mobile 512 / balanced 1024 / high 1536 /
  ultra 2048. Takes effect on the next deploy (when a map's lights + depth maps are
  created). pointlight.c now includes renderer.h for the extern.
Both compile clean; 46/46 tests pass. HONEST: not GPU/FPS-measured here -- the scaling
is standard and wired correctly, but the actual fps deltas + upscale sharpness are for
on-device tuning.

## Update 70 -- quality preset defaults + CINEMATIC STYLING layer (JSON) + first wire
Two things.
(1) SENSIBLE DEFAULTS: changing the quality tier now also sets a sensible render scale
    (mobile 50 / balanced 75 / high+ultra 100) -- one-tap preset, still overridable on
    the render-scale row.
(2) CINEMATIC STYLING: reasoned each troop's archetype (from class taglines + film
    canon) into concrete look/feel numbers so a Reaver prowls hunched + predatory and
    a Prime stands noble + upright WITHOUT new meshes -- pure recombination on the base
    IQM parts. New content/styling.json (17 troops): posture_lean, shoulder_hunch,
    gait_cadence, gait_swagger, arm_len, leg_len, visor_narrow, visor_angle,
    idle_restless, aggression -- each documented with the "read" it's going for.
    - content_build.py validates styling (every entry maps to a real troop; all values
      in sane ranges) and --emit-c now emits a per-class ex_gen_style[10] table into
      content_gen.h (player-class enum order), beside the chassis table.
    - FIRST APPLICATION wired: gait_cadence drives the player's step rhythm
      (ex_gait_advance dt *= cadence) -- Scout skitters (1.40), Siege plods (0.70),
      Vanguard is heavy (0.82). game.c now includes content_gen.h.
Still to wire (next): posture lean/hunch, limb proportions (arm_len/leg_len), visor
expression (narrow/angle), idle restlessness + aggression behaviours. 46/46 tests pass.

## Update 71 -- cinematic styling: posture + limb proportions wired into the model
Wired three more styling attributes into the actual body layout (robot_parts.c).
- New ex_body_style_t + global ex_active_body_style (neutral default), set per-entity
  before ex_robot_layout_ex.
- LIMB PROPORTIONS: arm_len/leg_len now scale the arm (AR) and leg (LG) lengths in the
  layout -- lithe long-legged predators (Reaver/Scout leg 1.18-1.20) vs heavy short
  bruisers (Siege/Titan leg 0.88-0.95), on top of the chassis muls.
- POSTURE: a forward LEAN tilts the whole upper body from the waist (parts above the
  waist shift forward by height*tan(lean)) -- aggressive/predatory bots lean in
  (Reaver 13deg, Blackstar 12deg), noble/stoic stand upright (Warden 0, Prime 3). A
  styling HUNCH rounds the shoulders + head forward, layered on any kind-based hunch.
- Fed from the generated ex_gen_style table: the player gets its class styling before
  layout; pose_body (enemies/allies) resets to neutral so they don't inherit it (enemy
  styling is a later step).
46/46 tests pass (neutral default = no change to test bodies). Still to wire: visor
expression (narrow/angle -> the "face"), idle restlessness + aggression behaviours.

## Update 72 -- cinematic styling COMPLETE (visor/face, fidget, swagger, enemies) [big patch]
Finished the styling system in one pass, for player AND enemies.
- STRUCT: ex_body_style_t extended with idle_restless + swagger.
- VISOR / FACE (robot_parts.c): visor_narrow squeezes the eye-band into a menacing
  slit + pulls it in; visor_angle tips the brow down (angry) or up (open/noble). This
  is the most legible read -- Reaver/Blackstar wear near-slits angled down, Prime/Scout
  wear open brows.
- IDLE RESTLESSNESS: idle fidget (bob + sway) scales with idle_restless -- twitchy
  scouts/predators vs statue-still wardens/sentinels.
- SWAGGER: heavy bots (swagger>1) roll shoulder-to-shoulder as they stride; the whole
  upper body sways with the walk phase.
- ENEMY STYLING (framework): --emit-c now also emits ex_gen_style_body[6] indexed by
  ex_body_kind (grunt/corrupted->ravager/hijack->slipstream/apex->ravager/human->
  neutral). New ex_style_for_body() feeds pose_body, so enemies read with intent too;
  per-enemy gait cadence drives their step rhythm.
- TEST: test_content_gen now asserts both styling tables (10 classes + 6 body kinds)
  are well-formed.
Full styling stack now live end-to-end (JSON -> validated -> codegen -> in-engine):
limb proportions, posture lean/hunch, visor expression, idle fidget, walk swagger,
gait cadence -- for player + enemies. 46/46 tests pass. Remaining (behavioural, balance
-sensitive, deferred): aggression driving actual AI movement speed + attack timing.

## Update 73 -- [WFC + script] the CORE map (the Forge) + ESCALATION mode [big patch]
Pulled from the script's molten "core"/"Forge"/Matrix beats (84/59/35 mentions) and
War-for-Cybertron's underworld + Escalation horde.
- NEW MAP "core" (15th theme): Cybertron's molten heart -- dark metal floor cut by a
  glowing cross of energon/molten channels, a central FORGE (pillar ring + dais + the
  anvil column + a glowing Matrix core), and 6 industrial descent-shaft towers. New
  molten.png (+ normal map). Molten forge-glow atmosphere; energon-rich (surges on).
  Wired NUM_THEMES 14->15 + ids/menu/desc/atmosphere; campaign MISSION 6 "the mantle
  dive" (Hot Rod -> Rodimus, the Matrix) now deploys here instead of borrowing foundry.
- NEW MODE "escalation" (WFC's signature horde loop): endless escalating waves with an
  ENERGON ECONOMY. Kills bank energon credits (grunt 20 / corrupted 40 / apex 120);
  each wave you clear FORGES the banked energon into permanent +max integrity and a
  full repair (+armor floor), so surviving compounds your strength. HUD shows live
  ENERGON credits + total forged. Globals + deploy init + kill hook + wave-clear forge
  + HUD, mirroring the DEFEND wiring.
46/46 tests pass; game.c clean; core.iqm valid (17 colliders); data archive repacks.

## Update 74 -- [WFC x CoD Zombies] Escalation STATION FRAMEWORK: buy-a-door map expansion
Took Escalation to BO3-Transit scale, Transformers-themed. New station system (game.c):
- 5 station types: GATE, WALLBUY, PERK, REPAIR, BRIDGE. Data-driven layout table
  (escalation_setup_stations) placed in ZONES radiating from spawn.
- BUY-A-DOOR MAP EXPANSION: GATE stations cost energon; opening one unlocks the next
  zone and GROWS the enemy spawn box (g_esc_box 26 -> 44 -> 62), so the playable/threat
  area literally expands through doors -- the core CoD-Zombies loop.
- ECONOMY: kills bank energon (U73); spend at stations. REPAIR (repeatable heal+armor),
  WALLBUY (installs a weapon core: flak/plasma), PERK (JUGGERNAUT +80 max hp / OVERDRIVE
  +20% move / RAPID COIL +28% fire -- wired into the speed + cooldown paths), and the
  SPACE BRIDGE (5000: unlocks all zones + big fortify -- "the outer rim opens", the
  gateway to the moon-hopping expansion to come).
- Interaction: proximity + [E], energon check, HUD buy-prompt ("[E] LABEL  N energon"),
  glowing emissive station beacons tinted by type, only reachable stations shown
  (GATE reachability = next zone only, so it's a real progression).
Compiles clean; 46/46 tests pass. HONEST: big gameplay system, logic-verified + built
but NOT playtested here (headless) -- costs/zone sizes/perk values are tuning targets.
Next: multiple escalation maps + the space-bridge actually TRAVELLING to moon/outer-rim
sub-arenas.

## Update 75 -- [WFC x BO3-Transit] Escalation SPACE-BRIDGE TRAVEL: the world opens up
The Space Bridge now actually TRAVELS. Buying it jumps the whole escalation to the next
arena -- a moon / outer-rim location -- keeping your run's energon + perks + forged max
-HP. All theme maps are pre-loaded, so escalation_do_travel() swaps arenas cheaply:
retargets the visible theme model, swaps colliders (load_map_colliders) + atmosphere
(apply_theme_lighting) + energon-cache flag, rebuilds fresh zones/stations, and drops
the player at the new spawn. Arena chain: START map -> outer_rim -> pluto (moon) ->
mars (fragment) -> trench (deep) -> loops, escalating deeper into space (BO3-Transit's
"world expands" moment, Transformers-styled). Bridge purchase sets a pending flag that
the update resolves where the player entity is in scope. HUD shows "sector N". Each new
arena resets zones to 0 (buy your way back out through its doors) but you keep
everything you earned. Compiles clean; 46/46 tests pass. HONEST: built + logic-verified,
NOT playtested (headless) -- arena chain/pacing are tuning targets.

## Update 76 -- [script] Escalation SECTOR WARLORDS + boss-gated bridge + escalating swarm
Adapted the film's premise -- the space bridge scattered Cybertron's fragments across
the solar system, each fragment held by a warlord -- into the escalation loop.
- SECTOR WARLORDS: each arena is a named warlord's domain, drawn from the script and
  the existing champion system: CRYOTEK (frost/pluto), BLACKSTAR (outer_rim/trench),
  JHIAXUS (vault/mars/core). The champion promotion mapping was extended to cover every
  escalation arena; travel resets g_champion_spawned so each new sector spawns its own
  warlord (8x-hp mini-boss with a boss bar).
- BOSS-GATED BRIDGE: the Space Bridge is now LOCKED until the sector's warlord falls
  (station_reachable(BRIDGE) requires g_esc_boss_down). HUD warns "a warlord holds this
  sector -- destroy it to open the bridge." map_has_warlord() sets the gate on
  deploy + on each jump; the warlord's death (existing kill hook) opens it and pays a
  +400 energon bounty. So the run is: expand zones -> the warlord rises -> kill it ->
  bridge deeper.
- ESCALATING SWARM: each space-bridge jump enlarges every wave (+2 grunts, +1 corrupted,
  +0.5 hijack per sector), so bridging deeper into the rim genuinely gets harder.
Compiles clean; 46/46 tests pass. HONEST: built + logic-verified, not playtested
(headless) -- warlord hp, bounty, and swarm scaling are tuning targets.

## Update 77 -- [escalation] ENERGON FORGE (Pack-a-Punch) + deepening gravity-well hazard
Two more escalation systems.
- ENERGON FORGE (STN_FORGE, Pack-a-Punch analog): a station in zone 1 that upgrades your
  EQUIPPED weapon, repeatable up to tier 3 with escalating cost (2500 -> 4000 -> 5500).
  Each tier: +35% damage and faster refire (cooldown *= 0.86^tier), applied in the
  weapon build beside the perks. A deep energon sink that keeps the economy meaningful
  late. Reset per run; carries across space-bridge jumps. New station type + orange-gold
  beacon tint.
- UNICRON'S PULL DEEPENS (script: "the well tears apart everything in orbit"): in deep
  escalation sectors (2+), even after the warlord falls, a gravity well SURGES and drags
  the player toward the arena centre -- and it grows stronger + more frequent the deeper
  you bridge (pull 14+7*depth, cadence tightens 11->5s). The rim literally tips inward
  as you near the core. Runs only when no boss is present (the boss well takes over then),
  so the two never fight over the shared timers.
Compiles clean; 46/46 tests pass. HONEST: built + logic-verified, not playtested
(headless) -- forge tiers/costs and pull strength are tuning targets.

## Update 78 -- [script] Escalation FISSURES: the mantle cracks open (film hazard, 65x)
Adapted the script's most-cited environmental beat -- the mantle FISSURES splitting the
ground under the fighting -- into an escalation area-denial hazard.
- Telegraphed molten rifts spawn near the player in batches; the telegraph glows a dim
  ember (1.6s warning), then ERUPTS: a shockring + molten burst, and if you're standing
  on it (~4.2u) it burns you (20 + 4*sector damage). Then it cools and closes.
- Deeper sectors crack faster + wider: batch grows and the cadence tightens (8s -> 4s)
  the further you've bridged toward the core.
- Pairs with the deepening gravity well (U77): the well drags you toward the centre while
  fissures split it open -- pulled in, but the ground you're pulled onto may erupt.
- Own model pool (emissive rift markers, ember->molten tint by state), spawned in the
  escalation setup, hidden in other modes. New ex_fissure_t pool + escalation_fissures().
Compiles clean; 46/46 tests pass. HONEST: built + logic-verified, not playtested
(headless) -- fissure damage/cadence/radius are tuning targets.

## Update 79 -- [script] Escalation: MEGATRON'S TRUCE -- a Decepticon fights beside you
Adapted the film's Megatron-truce beat into escalation, reusing the Forgeheart ally
system + the existing truce logic.
- When a sector warlord is beaten down below 40% but still deadly, Megatron sends a
  Decepticon to help finish it: g_ally_present flips on, the ally body (already spawned
  + hidden at deploy) is reset beside the player with 300 hp, and its existing AI
  (ex_ally_think: follow, engage, revive) takes over -- it fights the warlord + the
  swarm alongside you.
- The pact HOLDS for the rest of the run (fires once), and since travel doesn't clear
  g_ally_present the Decepticon rides the space bridge with you into the next sector --
  the film's temporary alliance, extended across the horde. HUD: "MEGATRON'S TRUCE -- a
  Decepticon fights beside you."
Not campaign-gated, so it only arms in escalation warlord fights. Compiles clean;
46/46 tests pass. HONEST: built + logic-verified, not playtested (headless) -- the 40%
trigger + ally hp are tuning targets.

## Update 80 -- FOSS modelling pipeline: Blender -> glTF/GLB -> IQM hooks
Added a hand-modelling path so art can scale past procedural generation using FOSS tools.
- NEW tools/gltf_to_iqm.py: converts glTF 2.0 / GLB (Blender's cleanest export) into the
  engine's IQM part format, reusing obj_to_iqm's canonical write_iqm. Flattens the whole
  node hierarchy (bakes transforms to world space), merges all mesh primitives, handles
  .glb (binary) + .gltf (external/embedded buffer), POSITION/NORMAL/TEXCOORD_0 + indexed
  or non-indexed prims, Y-up (Blender default) with a --z-up flag, and normalises into
  the engine's 2-unit part slot (or --raw). obj_to_iqm.py already covered OBJ.
- THE OVERRIDE HOOK: export a Blender mesh to a part filename (phead.iqm, pchest.iqm,
  plimb.iqm, spine_*.iqm, ...) and it replaces that procedural part everywhere, for every
  troop + chassis, with the cinematic styling layer still applied on top -- no code change.
  New models can also be registered via content/part_groups.json.
- NEW docs/blender_pipeline.md: the full workflow (model -> export glTF -> convert ->
  drop in data/ -> validate), the override table, axis/slot/material conventions, and the
  auto norm_/spec_/emit_ map convention.
- NEW tools/test_gltf.py (gltf_roundtrip ctest): builds a minimal glTF + GLB cube,
  converts both, asserts a load-compatible unit-slot IQM. Guards the importer.
Suite 46 -> 47, all pass.
(Process note: the headless build's target list must filter to C targets that have a
tests/<name>.c -- python-test names like test_gltf are ctest COMMANDs, not build targets.)

## Update 81 -- MARKOV BEHAVIOUR FSM: simulated + tuned per mode/mission/vehicle, wired live
A complex, data-driven enemy-AI behaviour system built by headless simulation + tuning.
- FSM: a Markov chain over 8 combat behaviours (ADVANCE, STRAFE, HOLD, FLANK, CHARGE,
  RETREAT, HARASS, REGROUP). Each decision tick an enemy samples its next behaviour from
  a transition matrix picked by (game context, vehicle archetype).
- TUNER (tools/tune_fsm.py): a headless abstract-combat sim (player kites, K enemies run
  the FSM) with a per-context fitness (skirmish = encircling pressure banded so enemies
  are dangerous-not-instakill; defend/beach = rush the objective; stalkers/trench =
  patient survival; escalation/finale = surround + grind), then an EVOLUTIONARY search
  (elitism + mutation) fine-tunes a matrix per (context, archetype). Contexts = the 5
  modes + all 8 campaign missions (13); archetypes = flyer/heavy/fast/skitter (the 21
  vehicle types bucket onto these). 52 matrices tuned in ~9s; deterministic/reproducible.
  --emit-c writes engine_src/exengine/behavior_gen.h (matrices + vehicle->archetype map).
- MODULE (behavior.h/.c): ex_behavior_ctx(mode,campaign,mission), ex_behavior_arch(
  vehicle_type), ex_behavior_step(ctx,arch,cur,rng) (xorshift-sampled). Registered in
  cybertron_modules.
- LIVE: every enemy now runs the FSM -- stepped ~every 0.6s by (current mode/mission
  context, its archetype: apex/corrupted->heavy, hijack->fast, grunts vary by id), and
  its behaviour adds a gentle movement flavour over the base chase AI (strafe circles,
  flank arcs in, harass oscillates, retreat/hold peel off). Kept small so it enriches
  motion without overriding tuned encounter balance.
- content/behavior_fsm.json (the tuned data) + tests/test_behavior.c (row-stochastic,
  vehicle-map coverage, context map, sampled distribution matches the matrix, contexts
  actually differ). Suite 47 -> 48, all pass.
HONEST: the sim is an ABSTRACT proxy (kiting player, simplified combat) -- it produces
sensible, context-differentiated matrices (flyers->STRAFE, heavies->HOLD/CHARGE,
fast->HARASS), but the fitness model + nudge strengths are tuning targets against real
play, not playtested here.

## Update 77 -- vehicle/camera/dropship fixes (from user screenshot report)
Five fixes from a user screenshot (viewer down here, so worked from the description):
- VEHICLE MESH CLUTTER: greebles now hidden in vehicle mode (if(in_vehicle) ng=0) --
  they were the blocky "stuff" obscuring the clean vehicle mesh, same detail pass as
  the robot but wrong for the vehicle silhouette.
- VEHICLE TOO BIG: vehicle visual scale reduced ~18% (size_scale*0.82f at both
  ex_vehicle_layout calls -- the in-vehicle layout + the transform-blend layout).
- CAMERA ZOOM: third-person boom 8.5 -> 7.0 (tighter framing; vehicle still pulls back
  at speed).
- RAISE MODELS IN FRAME: camera focus target lowered (player+2.2 -> +1.6) so the
  character sits higher in the frame with less headroom.
- LOADING DROPSHIP OFF-SCREEN: the deploy dropship arc started at +38 (above the top
  of frame); lowered to +20 with a gentler descent (-13) so it stays visible through
  the drop-in.
48/48 tests pass. HONEST: camera/scale values are framing tweaks reasoned from the
description, not seen here -- easy to nudge further if any overshoot.

## Update 78 -- [big batch] greebles down, default-cube root fix, map scorer + natural terrain
Worked from a user screenshot + the monorepo bundle (games/soldier as the map-design
reference). Five things:
- GREEBLES TONED DOWN ~35%: detail.c plate/ridge/vent/etc. shrunk (ridge 0.55->0.34 of
  the part unit, plate 0.38->0.24, antenna 0.42->0.26, etc.) -- less chunky surface.
- DEFAULT-CUBE ROOT FIX: the renderer falls back to model->scale (default 1.0) when
  scale3 collapses to (0,0,0) -> a full 2-unit "default cube". Every scale3-DRIVEN
  character model (all body parts via spawn_body, greebles, the cape, the signature
  back-piece) now has ->scale=0, so a collapse renders INVISIBLE instead of a giant
  textured box on the bot/vehicle. General fix for the whole class, both silhouettes.
- MAP SCORING HARNESS (tools/map_score.py, inspired by soldier's mapeval): scores each
  map's collider layout on cover / spread / verticality / openness / scale (0..100 +
  grade), --all to rank, --check to gate. Registered as a ctest. Baseline mean was 61.
- NATURAL TERRAIN: new Builder.mound / .ramp / .natural_terrain -- scatters low varied
  -height mounds + a ramped vantage plateau for bumpiness/verticality, applied to 8
  outdoor maps (wastes, frostline, outer_rim, reclaim, trench, mars, beach, mining).
  Each gets real AABB colliders (better bounding boxes for traversal/collision). Result
  (scored): wastes 69->79, frostline 54->71, mars 65->76, beach 54->70, outer_rim
  65->76, trench 69->78; mean 61->67. All maps still validate + within the 120-collider
  cap. map_score --check gates them above a quality floor.
49/49 tests pass (map_score gate added). HONEST: greeble/cube/terrain verified by
build + scorer + validators, NOT seen on a GPU here. Still queued from the request:
more script pulls + escalation expansion (next batch).

## Update 79 -- per-arena escalation shops + lift the last weak maps (scorer-guided)
Continuing escalation + map quality (gravity-well-per-sector, per-warlord attacks, and
the Pack-a-Punch Energon Forge were already in from earlier turns).
- PER-ARENA STATION LAYOUTS: escalation_setup_stations now varies by sector -- costs
  scale with depth (x1 + 0.45/sector), the two wall-buy cores + two perks rotate per
  sector (FLAK/PLASMA -> SCATTER/RIPPLE -> CRYO/SUPPRESS -> PLASMA/PIERCE, matching the
  warlord turf), and the shop mirrors on odd sectors. Each bridge now feels like a
  fresh economy, not the same template.
- WEAK MAPS LIFTED (using the U78 map scorer as the guide): added natural rubble terrain
  to the two D-grade maps -- emberpit 52->76 (spread 0.27->1.00, scale 0.46->0.84) and
  foundry 60->75 (scale 0.33->0.90). Both validate + within the 120-collider cap. Map
  mean 61 (start of session) -> 69; no map is below C now.
49/49 tests pass. HONEST: escalation variety + map scores verified by build/scorer/
validators, not playtested on GPU.

## Update 80 -- [script] THE WATCH: elemental super-boss in deep escalation sectors
Pulled the film's Watch (the 12+1 elemental god-forms) into escalation as a rare deep
-sector super-boss, extending the champion framework with kind 4.
- From sector 4 onward, the fragment's keeper is no warlord but one of THE WATCH -- an
  elemental god-form whose element ROTATES per sector (ICE/FIRE/STORM/THUNDER/BRAMBLE/
  STONE/TIDE), announced "THE WATCH RISES -- the <element> form".
- Tougher: 13x base hp (vs 8x for Cryotek/Blackstar/Jhiaxus), body TINTED to its
  element (element_color), boss bar reads "THE WATCH".
- ELEMENTAL NOVA special: a big twin-shockring radial blast themed to its element; ice/
  tide forms also FREEZE you (g_player_slow) in place. On a 3.8s cadence.
- Still gates the space bridge (kill it to bridge on) and pays a bigger bounty (700 vs
  400 energon). Reuses the existing champion promotion / attack / death / HUD / tint
  paths -- one new kind threaded through cleanly.
So escalation now escalates in KIND, not just number: warlords hold the near fragments,
the Watch holds the deep ones. 49/49 tests pass. HONEST: built + logic-verified, not
playtested on GPU.

## Update 81 -- [script] The Watch, deepened: distinct elemental novas + kill-surge reward
Made each Watch form fight and reward differently.
- DISTINCT ELEMENTAL NOVAS (kind-4 special now branches on g_watch_elem):
  * FIRE  -- higher damage, sets you BURNING (toxic DOT), and erupts fissures around you
  * AIR/STORM -- wider blast + violent hurl-back (kb 62 vs 34)
  * ROCK/STONE -- heavy short shove + higher damage
  * ELECTRIC/THUNDER -- brief stun + more damage, fires FASTER (3.2s vs 3.8s)
  * VINE/BRAMBLE -- roots you (3.4s slow)
  * ICE/WATER -- freeze/mire (2.2s slow)
  Each reads with its element_color VFX + shockrings.
- KILL REWARD LOOP: defeating a Watch now seizes its "ELEMENTAL SURGE" -- +60 heal, a
  big energon bounty (700), and 8s of surge (weapon x1.5 damage / x0.7 cooldown), with
  a HUD countdown. Turns the super-boss kill into a power spike, not just a gate open.
  Wired into the firing path beside the forge/matrix boosts; reset on deploy.
So the deep sectors now vary in FEEL (which element you're fighting) and pay off with a
rush. 49/49 tests pass. HONEST: built + logic-verified, not playtested on GPU -- nova
effects, surge strength, and cadences are tuning targets.

## Update 82 -- [CoD-Zombies] escalation POWER-UP DROPS
Added the classic Zombies power-up loop to Escalation: ~3% of kills drop a floating,
spinning, color-coded pickup you walk over to grab (15s before it fades).
- DOUBLE ENERGON (gold): 30s of 2x credit gain -- HUD flags "ENERGON ... (x2!)".
- FULL REPAIR (green): heal to full + armor floor.
- NOVA BLAST (red): scours the whole rank-and-file horde (leaves warlords/Watch/apex).
- OVERCHARGE (violet): 10s weapon surge (reuses the Watch-kill surge: x1.5 dmg/x0.7 cd).
Self-contained system (ex_powerup_t x6 + emissive beacon models), dropped from the
escalation kill hook, updated beside the stations, reset on deploy. NOVA kills still
route through the normal death hook so they pay energon + can chain more drops.
So escalation now has that Zombies rhythm: chase kills -> pray for a drop -> cash a
Double Energon into a Pack-a-Punch/perk spree. 49/49 tests pass. HONEST: built + logic-
verified, not playtested on GPU (drop rate + effect strengths are tuning targets).

## Update 83 -- [script] the KUIPER BELT map (Blackstar's fleet), scorer-designed
Built the film's most-referenced unbuilt location -- Blackstar (471 mentions), the
Kuiper/asteroid belt -- using the U78 map scorer in a generate-score-iterate loop.
- KUIPER map (17th theme): an asteroid-belt salvage yard -- 16 drifting asteroids of
  varied height (cover + verticality), 4 rusted salvage platforms at varied heights
  reached by gantry ramps, strewn wreckage, and Blackstar's flagship deck + tall mast
  at the center. Modelled on the A-grade cybertron layout; iterated with the scorer
  (74.2 -> 77.9 B: raised asteroid height 9->13 and spread 78->90 to lift verticality
  0.38->0.51 and scale 0.67->0.81). Validates; 108 colliders (under the 120 cap).
- FULLY WIRED: theme roster (NUM_THEMES 15->16) + ids + menu name/desc + cold-starlight
  atmosphere. It's Blackstar's warlord turf (champion kind 2 + map_has_warlord), and
  the FIRST escalation arena in the bridge chain (kuiper -> outer_rim -> pluto -> mars
  -> trench), so escalation now opens in the belt where the scavenger king holds court.
17 maps, mean score 69.8. 49/49 tests pass. HONEST: layout scored + validated headless,
not seen on GPU.

## Update 84 -- [script] UNICRON's interior: the finale map (god-machine's heart)
Built the film's climax location -- Unicron (398 mentions), the god-machine -- as a
dedicated finale arena (18th theme).
- UNICRON map: inside the god-machine's chest -- the HEART pulsing on a central dais
  (glowing energon core + column), 8 energon veins radiating outward, 14 ribbed
  machinery pillars of varied height (5-16u), and 3 concentric descent tiers. Scores
  77.7 B (verticality 0.62 -- the pillars/tiers), validates, 71 colliders.
- WIRED: theme roster (16->17) + ids + menu name/desc + a red-violet heart-glow
  atmosphere + energon-rich (surges on). CAMPAIGN MISSION 8 "beneath the pacific" (the
  Unicron boss) now deploys here instead of borrowing cybertron -- the finale finally
  has its own stage. Also added as the DEEPEST escalation arena (kuiper -> outer_rim ->
  pluto -> mars -> trench -> UNICRON) and warlord turf (kind 3).
18 maps, mean score 70.2. 49/49 tests pass. HONEST: scored + validated headless, not
seen on GPU.

## Update 85 -- [script] UNICRON boss: the god-machine wakes (finale + deep escalation)
Gave the finale a named super-boss instead of a plain elimination. Extended the champion
framework with kind 5 = UNICRON (mirrors the Watch kind 4), active on the unicron map.
- On the unicron map the first grunt rises as UNICRON: 18x hp (vs Watch 13x / warlord
  8x), deep-red body, boss bar "UNICRON", "UNICRON WAKES -- the god-machine stirs".
- Attack: a huge elemental nova (radius 64 vs 46, 30 dmg, 3.0s cadence -- the widest,
  hardest, fastest boss blast) on top of the gravity-well pull already tied to the boss.
- Death: "UNICRON FALLS -- the god-machine goes still" + the big escalation bounty.
- CAMPAIGN MISSION 8 "beneath the pacific" beefed to Unicron + 4 corrupted antibodies
  (5 grunts, first = Unicron) on the new unicron map -- a real finale. Unicron also
  holds the DEEPEST escalation arena.
So the campaign now ends against the god-machine itself on its own stage, and the
longest escalation runs do too. 49/49 tests pass. HONEST: built + logic-verified, not
playtested on GPU -- Unicron's hp/nova/cadence are tuning targets.

## Update 86 -- [WFC/FOC] Grimlock-style GROUND SLAM + aerial DIVE-BOMB melee
Drew from Fall of Cybertron's Grimlock melee (and WFC's close-combat) for a new, always
-available player tool -- the thing the game lacked for when the horde closes in.
- GROUND SLAM (Q): a radial melee shockwave centred on you -- ~11u radius, 34 dmg,
  hurls enemies back. 3.5s cooldown. Uses the existing apply_shockwave AOE + shockrings
  + a camera-kick + landing squash so it lands with weight.
- AERIAL DIVE-BOMB: slam while airborne and you CRASH straight down (velocity y=-34)
  into a bigger stomp -- 15u radius, 46 dmg, harder camera kick, 4.5s cooldown. Rewards
  jumping in before the slam, very Grimlock.
- Guarded: disabled at human scale + in vehicle mode. Low-hp enemies caught in the ring
  just die (a natural finisher). Bound to the free Q key.
Gives every class a panic-clear / space-maker independent of the Watch-form abilities,
which fits the horde density escalation now throws at you. 49/49 tests pass. HONEST:
built + logic-verified, not playtested on GPU -- radius/damage/cooldowns are tuning
targets.

## Update 87 -- [FOC] COMBINERS: break up the Combaticons before BRUTICUS forms
Built Fall of Cybertron's signature mechanic -- the combiner -- as an escalation threat
with real "kill them fast or else" tension.
- THREAT: periodically, when >=4 grunts are massing, a warning fires ("COMBATICONS
  MASSING -- destroy them before they merge!") and a 14s merge countdown starts (shown
  on the HUD).
- BREAK IT: drop the squad below 3 alive before the timer and "the combine is broken."
- OR ELSE: if 3+ survive when it hits zero, BRUTICUS FORMS -- a survivor becomes a
  GIANT (2.7x scale, 18x hp), the rest fold into it, and it stomps a huge slam
  shockwave (radius 20) on a 4.5s cadence. Its own "BRUTICUS" boss bar (priority over
  the warlord bar). Killing it pays +500 energon.
Self-contained combiner_update() beside the stations/power-ups (tracks alive grunts,
the warn window, the merge, the giant's slam + death); giant scale via the enemy render
(i==g_combiner_slot -> ss 2.7); reset on deploy. 49/49 tests pass. HONEST: built +
logic-verified, not playtested on GPU -- timings/scale/hp are tuning targets.

## Update 88 -- [WFC/FOC] TITAN STRIKE: call Metroplex's orbital bombardment
Built the WFC/FOC "call the giant" fantasy (Omega Supreme / Metroplex) into the energon
economy as a buyable escalation station.
- New STN_TITAN station "TITAN STRIKE" in the zone-2 shop (4000 energon, repeatable).
- On purchase: "METROPLEX ANSWERS -- orbital bombardment inbound" -> 5 seconds of
  raining strikes (~3/sec), each a heavy shockwave (12u radius, 60 dmg + knockback) +
  explosion + shockring, aimed at live enemy clusters (scatters near you if none),
  with a building camera shake and a "METROPLEX BOMBARDMENT" HUD banner.
- Wired: station enum + layout + purchase + titan_bombardment() beside the stations/
  power-ups/combiner, marker colors extended to cover FORGE+TITAN, reset on deploy.
So the deep-sector economy now buys a screen-clearing cinematic titan strike -- a
different tool from the random Nova drop (this one you SAVE FOR and call on demand).
49/49 tests pass. HONEST: built + logic-verified, not playtested on GPU -- cost, duration,
strike damage/cadence are tuning targets.

## Update 89 -- weapon feel + textures + map fix (graphics/physics/maps pass)
A feel-and-polish pass across the requested areas:
- WEAPON PHYSICS -- MUZZLE CLIMB: recoil was a lone scalar that never touched the aim.
  Added a centralized view-kick on each shot's recoil rising-edge -- the camera pitches
  up (scaled to that shot's recoil) with a small horizontal drift, and the accumulated
  climb settles back down (~50 deg/s). Rapid fire now walks the aim up (hold it or fight
  it); heavy single shots kick once and recover. Weapon-agnostic (every fire path that
  sets g_recoil gets it).
- ANIMATION -- weapon recoil: the barrel now pitches up with the kick (w->rotation[0] =
  g_recoil*22) on top of the existing pull-back + lift, so firing reads on the gun.
- TEXTURES: generated brushmetal.png + norm_brushmetal.png (brushed-panel plating, seam
  ~4.8) as a new deck/platform material.
- MAP DESIGN + BUG: reclaim 68 C -> 72 B. Its three "lanes" were huge FLAT collider
  strips (14x108) -- both tanking openness (0.35) AND acting as phantom collision you'd
  bump into on open floor. Made them non-colliding visual floor (openness -> 0.94) and
  thinned the jungle cover 60 -> 32 for flow. Map mean 70.2 -> 70.5.
49/49 tests pass. HONEST: recoil/animation are logic-correct + compile, but the FEEL is
GPU/input-unverified here -- climb scale (2.6), recovery (50/s), barrel kick (22) are
tuning targets.

## Update 90 -- animations: enemy DEATH TOPPLE + HIT FLINCH
Enemies were snapping out of existence the instant they died and only flashing white
when hit. Gave them physical reactions.
- DEATH TOPPLE: a per-enemy g_enemy_death timer (0.9s) starts on the kill transition.
  The render loop now lets a dying body through instead of hiding it immediately, and
  over the timer it SINKS into the ground, keels over sideways, and crumples (scale
  collapses, gait freezes) before it's hidden. Deaths now read as a fall, not a
  vanish.
- HIT FLINCH: on top of the existing white damage-flash, a hurt enemy now physically
  jerks -- knocked up a touch + a brief recoil scale-pop scaled by the hit -- so shots
  land with weight.
Both are pure-render (the AI/damage loops are untouched), reset on deploy. 49/49 tests
pass. HONEST: logic-correct + compiles, but the motion is GPU-unverified here -- topple
sink/keel (1.7/0.9u), duration (0.9s), and flinch pop (0.4) are tuning targets.

## Update 91 -- projectiles: the LOB is now a REAL arcing shell (travel + gravity)
The lob fire mode was an instant hitscan-to-landing detonation. Made it a genuine
physical projectile.
- New lob projectile pool (ex_lob_t x8 + emissive models). Firing LOB launches a shell
  with forward+up velocity (dir*42 + 15 up) that TRAVELS and falls under gravity
  (scaled by g_map_gravity, so low-g maps give long floaty arcs and high-g maps drop
  it fast). It's drawn as a glowing shell in the weapon's color the whole flight.
- On impact -- ground, a foe (2.4u proximity), or 3.5s fuse -- it bursts: the same
  splash damage + falloff + explosion + shockring + lingering scorch, now at the real
  landing point instead of a computed one.
- lob_update() runs every frame in ALL modes (LOB is a loadout choice), reset on deploy.
This turns the lob from a point-and-detonate into a SKILL shot -- lead moving targets,
arc it over cover, bank the map's gravity. 49/49 tests pass. HONEST: logic + ballistics
verified by build, arc feel is GPU-unverified (launch speed 42, up-kick 15, gravity
26*g are tuning targets).

## Update 92 -- UI: readability + stylization pass on the HUD
Reworked the in-game HUD for legibility over busy scenes and a consistent Cybertron
tech look (the vga glyph renderer fills each cell's background, so setbg gives real
panels).
- TRANSLUCENT TECH PANELS: replaced the harsh opaque-black text background with a
  translucent dark-blue panel (8,12,20,a168) applied across the whole HUD, so every
  readout stays legible against explosions/energon glow without looking like solid
  boxes.
- HEALTH COLOR-CODING: HP now reads green (>60%) -> amber (30-60%) -> red (<30%) at a
  glance, and shows as "HP 240" (was a lowercase "hp" with an awkward trailing shield
  string; shield stays on its own energon-blue readout).
- BOSS BAR STYLIZED: boss name in warning gold; the bar now REDDENS as the boss bleeds
  (crimson at low hp -> hot orange at full) instead of a flat blue.
- CLEAN CROSSHAIR: the center "+" is drawn with a transparent cell so it stays crisp.
Preview (hud_restyle_preview.png) approximates the new look. 49/49 tests pass. HONEST:
colors/alpha are chosen from the code, not seen on GPU -- panel alpha (168) and the HP
thresholds are easy to nudge.

## Update 93 -- vehicle mode: RAM + NITRO BOOST (make the vehicle a weapon)
Vehicle mode had a great weighted g-force camera but nothing to DO -- it was just fast
movement. Made it a combat + mobility tool.
- VEHICLE RAM: driving into enemies above ~7 speed now damages them (scaling with
  velocity: 8 + vmag*1.6/s) and HURLS them aside, but only the ones you're actually
  driving toward (velocity-dot-direction gate). Bosses (warlord/Watch/Unicron/Bruticus)
  are immune to the hurl. The vehicle becomes a battering ram through the horde, with a
  contact camera-jolt.
- NITRO BOOST (hold Shift in vehicle): 1.7x top speed while a boost meter drains
  (~2s of full boost), recharging when off (~3.5s to full). Powers rams and outruns
  swarms. A 10-segment BOOST gauge shows on the HUD in vehicle mode, brightening while
  lit. Reset full on deploy.
Now transforming to vehicle is a tactical choice -- charge a cluster, boost to reposition
-- not just a travel button. Pairs with the g-force camera (boost/ram feed the trauma).
49/49 tests pass. HONEST: built + logic-verified, not driven on GPU -- ram damage/knock,
boost multiplier (1.7) + drain/recharge rates are tuning targets.

## Update 94 -- combat AI: separation, aggression press, wounded self-preservation
The AI already had a solid behavior FSM (strafe/flank/harass/retreat/hold). Three
additions that make the horde read as smarter without touching the tuned balance:
- SEPARATION (boids): each chasing enemy pushes gently off neighbors within ~3.2u
  (clamped shove), so a crowd SURROUNDS the player and fans across cover instead of
  collapsing into one stacked column on your face. O(n^2) but n is tiny.
- AGGRESSION PRESS: predatory kinds (apex/corrupted) get a forward closing nudge when
  beyond ~6u, so they commit to the attack instead of loitering at range with the
  skirmishers -- the heavies feel relentless.
- SELF-PRESERVATION: a badly wounded rank grunt/hijack (<28% hp, not a boss) is now
  likely to peel off -- a charge/advance flips to RETREAT, otherwise to FLANK -- so the
  fight has ebb and flow instead of every enemy suiciding straight in.
All layered on the existing chase AI as small dt-scaled nudges. 49/49 tests pass.
HONEST: logic-correct + compiles, crowd behaviour is GPU-unverified -- separation radius
(3.2u/strength 1.6), press (1.3), and the wound threshold (28%) are tuning targets.

## Update 95 -- NEW MODE: ORB RUSH (Halo Grifball x Rocket League x the Blackstar Orb)
A melee possession brawl. Synthesized the film's most-referenced relic -- the ORB (75
mentions) -- into a grifball-styled mode. NO ranged weapons here.
- THE ORB: a glowing, pulsing relic sphere spawns at centre court. CARRY it (walk up
  slow and it rides in front of you) or DRIBBLE it Rocket-League style -- bump it at
  speed on foot OR ram it in vehicle mode. Click to HURL it forward when carrying.
- Loose orb has real physics: rolls with friction, bounces off the side walls.
- FORGES: get the Orb past the enemy forge line (+Z) to score; enemies shove it back
  toward YOUR forge (-Z). First to 5 -> new match.
- POWER AXES: two on the court (respawn on an 18s timer) -- grab one for 12s of a
  DEVASTATING ground-slam (x3 damage, x1.6 radius) -- the grifball hammer.
- MELEE ONLY: guns are disabled (fire_held forced off); the slam + vehicle ram + axe
  are your whole kit. Vehicle ram is fully legal (Rocket-League car energy).
- Wired as EX_MODE_ORBRUSH (menu "orb rush"), its own HUD (score, carry state, forge
  direction, axe timer), orb + axe models, reset on deploy.
49/49 tests pass. HONEST: mechanics logic-verified + compile; ball feel (dribble impulse,
friction 0.28, throw 36) and scoring are GPU-unverified tuning targets. A dedicated
rectangular court map would sharpen it further (currently plays on the selected arena).

## Update 96 -- menus dressed up + graphics upped (in-style)
MENUS:
- Animated title: "CYBERTRON _ ARENA OF GODS" now pulses in energon-blue (sinusoidal
  glow on g_anim_t) instead of flat white -- the front-end breathes.
- Tech panels: menu text uses the same translucent dark-blue panel as the restyled HUD
  (was harsh opaque black) for a consistent interface language.
- More scenery: added 8 tumbling debris shards drifting/parallaxing past the camera on
  top of the existing cruising ship + 5 spinning planets (Earth/Jupiter/Cybertron/Mars/
  Pluto) + starfield -- the space backdrop has foreground depth + motion now.
GRAPHICS (up it, keep the style):
- SUPERSAMPLING: render-scale setting now goes 50 -> 150% (was capped at 100). >100%
  renders the scene to a larger internal buffer and downsamples = free anti-aliasing on
  capable hardware; labeled "SSAA". Already plumbed through ex_render_scale -> framebuffer.
- Richer energon bloom: post-bloom strength 0.7 -> 0.85 with a slightly lower bright-pass
  threshold (0.75 -> 0.68), so glow reads stronger without changing the filmic look. FS
  brace/paren balance verified.
49/49 tests pass. HONEST: menu logic + shader balance verified by build; the actual look
(title pulse, bloom richness, SSAA sharpness) is GPU-unverified -- bloom 0.85 and the
150% cap are easy dials.

## Update 97 -- particles + impact physics + texture grit (juice pass)
Big VFX/feel pass across the requested areas.
- LANDING IMPACTS: every landing now kicks up dust + sparks + a shock ring scaled by
  fall speed (was only big >22 drops). Heavy drops still crater. Gravity reads on touchdown.
- GUN SMOKE: a smoke puff spits from the muzzle on each shot.
- SKID SPARKS: the vehicle throws friction sparks off its treads above ~12 speed --
  the ram/boost play now leaves a trail.
- RICHER EXPLOSIONS: spark count 18->26, a grey SMOKE PLUME added, +3 debris chunks per
  blast -- missiles/lobs/titan strikes hit with more debris.
- HIT REACTION (brawl): enemies now stagger harder on hit -- knocked up ~1.9 + ROCKED
  BACK away from the blow (lateral shove in the render pose), a real "took a hit" jolt
  on top of the death topple. (True toss-ragdoll deferred: enemies are ground-locked
  with no vertical velocity to integrate -- that's a physics-model change, noted.)
- TEXTURE GRIT: gen_texture plating/hexmesh grain bumped (0.12->0.17 / 0.15->0.20) plus a
  fine wear/scratch layer; brushmetal regenerated (surface variation up, seam still low).
  Existing textures need their original params to re-grit (not stored) -- the grit is
  banked for new textures + the ones we can reproduce.
49/49 tests pass; gen_texture self-test green. HONEST: particle/impact logic verified by
build; look (density, smoke, grit) GPU-unverified. Full PBR (roughness/metallic maps +
shader) not attempted headless -- richer bloom (U96) + grit are the in-style substitute.

## Update 98 -- enemy VERTICAL PHYSICS: launch-and-tumble ragdolls
Gave enemies the vertical-velocity state they lacked, so heavy impacts now throw them.
- Enemies were ground-locked (y lerped to 0). Added a ballistic AIRBORNE state
  (g_enemy_air timer + g_enemy_vel[.][1] vertical velocity + gravity 32/s + a tumble
  spin g_enemy_spin/spinv). While launched, the ground-lerp is bypassed: they arc up,
  spin, fall, and hit the deck with a dust puff, then skid to a stop.
- enemy_launch(i, vy) helper. HARD impacts now toss LIGHT, non-boss troops:
  * apply_shockwave (ground-slam, explosions, lob bursts, TITAN STRIKE) launches anyone
    caught with a strong-enough impulse (im>7), scaled by force.
  * a fast VEHICLE RAM (>16 speed) sends them flying.
  Apex + the promoted champion/Watch/Unicron/Bruticus stay planted (mass/slot gated).
- Render reads the toss: the body spin-tumbles (yaw) through its arc.
So the slam, the ram, missiles, and the titan barrage now physically scatter the horde
into the air -- the "hammer impact" feel. 49/49 tests pass. HONEST: physics + build
verified; the tumble is a fast yaw-spin approximation (true pitch/roll would need
pose_body rotation support) and the arc is GPU-unverified -- gravity (32), launch caps
(24-26), and the im>7 threshold are tuning targets.

## Update 99 -- NEW MODE: SKY DUEL (Starscream free-fall brawl) [research-informed]
Researched the fan-loved "Free-Fall Fight" trope (God of War II's "beat him before you
hit the bottom", Bayonetta's falling platform, the film-1 tackle-off-the-tower + G1's
"thrown into the void") and built the mechanic almost no game has: a brawl while
PLUMMETING.
- EX_MODE_SKYDUEL "sky duel": you + STARSCREAM are launched to ~86m and plunge together
  (terminal velocity capped so the fall lasts ~6s). He drifts to stay in your face and
  LUNGES on a timer (strikes + knocks you); you STRIKE back with click/Q when close --
  each blow chunks his health, knocks him off, throws a shockring + sparks.
- The clock is the GROUND: an ALTITUDE bar counts down (reddens as the deck rushes up)
  alongside his health bar. Break him before you land -> he's HURLED down and you hover
  to a safe landing (WIN). Hit the deck with him alive -> you crater for 55 (LOSE).
- Melee only (guns disabled). Starscream is drawn from the spare ally body -- crimson,
  hit-flashing, spinning through the dive. Reuses player_hit / pose_body / shockrings.
Novel mechanic, self-contained (skyduel_update + its own HUD + deploy launch). 49/49
tests pass. HONEST: logic + build verified; the fall/brawl FEEL (terminal velocity 20,
foe spacing, strike/lunge cadence) is GPU-unverified and the prime tuning target. A
dedicated tall skybox arena + a proper mid-air camera would elevate it further.

## Update 100 -- sky brawl in skirmish + turret nerf
SKY BRAWL INTEGRATED INTO SKIRMISH (peak Transformers -- a duel over a live battle):
- Decoupled the free-fall brawl from its mode via g_skyduel_active. In any skirmish
  (arena/survival/stalkers/defend/escalation), Starscream now periodically CHALLENGES
  you (~every 60s, only when you're grounded): you're yanked to ~84m and the plummeting
  duel plays out ABOVE the arena while the horde keeps fighting below. Win -> a +40
  adrenaline heal and you drop back into the fight; lose -> you crater. The dedicated
  SKY DUEL mode still works (active set at deploy). No guns + the altitude/foe HUD key
  off g_skyduel_active now, so both paths share one implementation.
TURRET NERF (they were pinpoint machine-guns that zoned even vehicle aces):
- Fire cadence 0.9s -> 2.2s (slow, deliberate). Aim window 0.22 -> 0.16 rad + tracking
  1.1 -> 0.85 rad/s, so a fast strafer breaks the lock. Crucially, hitscan now MISSES
  moving targets: hit chance = 1 - min(speed/22, 0.78), so a ~20-speed vehicle is only
  ~22% to be hit and the bolt visibly sails wide. Skilled maneuvering runs the gauntlet.
49/49 tests pass. HONEST: logic + build verified; the challenge cadence, brawl-over-
battle framing, and turret hit-curve are GPU-unverified tuning targets.

## Update 101 -- ground bots throw ANTI-AIR flak at jets overhead
Completes the sky-brawl-over-a-battle fantasy: when a ground troop is aware but has
nothing to melee -- the player is a jet high above (a sky brawl, or just airborne >14u)
-- it now fires FLAK upward. Staggered volleys across the horde (g_enemy_aa cooldown,
1.3-2.4s offset per unit), real tracers streaking up from the ground fight, and a ~20%
chance of a small 3-dmg chip per bolt (a maneuvering flyer is hard to tag from below).
Gated so flyers (mode 2) and mid-launch enemies don't do it. So during a Starscream sky
duel the arena below now lights up with tracer fire chasing you -- the battle and the
brawl are one scene. 49/49 tests pass. HONEST: logic + build verified; flak rate, chip
(3), and hit chance (20%) are GPU-unverified tuning targets.

## Update 102 -- [script] refined ICE + GLYPH textures (Cryotek frost, Cybertronian runes)
Script-guided material pass. The digest leans hard on ICE (295 mentions -- Cryotek) and
GLYPHS/Cybertronian script, so I added two new procedural patterns to gen_texture (each
tileable, with matching normal + emission maps):
- ICE: a frosted fbm sheet fractured by sharp toroidal-voronoi seams -- the cracks
  darken (deep ice) and carry a faint glow (frozen energon). Seam h0.4/v1.0, detail std
  18.4. Applied across frostline (floor + spires + terrain + rock).
- GLYPH: dark Cybertron plating etched with angular rune strokes that GLOW (energon-lit
  script); strokes stay inside cell margins so it tiles cleanly (seam 0.1). Applied to
  cybertron's ruin walls (film's Cybertronian writing).
Both emit maps mean the cracks/glyphs self-illuminate where the renderer reads emit_*.
gen_texture --check/self-test green; maps regenerated + validate (cybertron holds 86.6
A, frostline 70.6 B), 18-map mean 70.5. 49/49 tests pass. Previews (texture_ice/glyph)
attached. HONEST: tiling + detail verified programmatically (low seams), in-engine look
GPU-unverified. Refined character models/animations are the natural next step.

## Update 103 -- camera polish: smooth boom, air animation, screen-door recede
Refined the third-person camera + air animations (asks 1 & 2 of a 3-part request).
- SMOOTH BOOM: the collision pull-in was snapping to each new clip distance. Now it
  eases -- pulls in FAST (never clips) but glides back out slowly (boom_smooth lerp), so
  the camera flows around cover instead of popping.
- AIR ANIMATION: the body now STRETCHES reaching up on the launch and BRACES/compresses
  on the way down, scaled by vertical velocity, layered over the landing squash -- the
  jump and fall read on the bot itself.
- SCREEN-DOOR (transformer-style): a tall tower standing directly between the camera and
  the bot now RECEDES (shrinks to ~50%) and springs back once clear, so it never walls
  off your view. Collider untouched -- only the silhouette gives way. NOTE: this is a
  shrink-recede approximation; a true dither/alpha screen-door needs per-model alpha in
  the renderer (not present), so that's a GPU-side follow-up.
49/49 tests pass. HONEST: logic + build verified, feel GPU-unverified (boom ease rates,
air stretch 0.16/-0.10, tower recede 0.5 are tuning targets). ASK 3 (Destiny-style
open-world planets/events/loot/raids) is a large separate build -- starting it next.

## Update 104 -- NEW MODE: PATROL (Destiny-style open roam) [MVP; raid tier later]
Built the explore loop on top of the space-bridge travel + wave + pickup systems.
- EX_MODE_PATROL "patrol": roam a planet with continuous roaming foes (survival-style
  spawns), on any of the 6 bridge worlds.
- PUBLIC EVENTS: every ~15-28s a beacon lights up (a pulsing pillar, colour-coded) with
  an objective -- SURGE (purge a scaling kill-quota) or HOLD (bank 18s in the beacon
  ring) against a 32s clock. Kills during a surge are counted off the existing kill hook.
- LOOT: completing an event drops a CACHE -- 4 pickups (energon/shield/overcharge) plus
  a rare Matrix every 3rd cache -- and +300 energon, with a shockring + spark fanfare.
  Loot rides the existing auto-collect pickup system.
- PLANET TRAVEL: [B] space-bridges to the next world (kuiper -> outer_rim -> pluto ->
  mars -> trench -> unicron) via escalation_do_travel -- the world literally swaps.
- HUD: planet, energon, caches cleared, live event objective + timer, bridge prompt.
Reuses travel/waves/pickups so it's lean. 49/49 tests pass. RAID TIER deferred to a
later turn (a gated multi-phase finale) as agreed. HONEST: logic + build verified; event
pacing (quota +1/event, 32s clock, 300 energon) + feel are GPU-unverified tuning targets.

## Update 105 -- OPENWORLD LIBRARY (agnostic helper, built on soldier's infra) + raid plan
Turned the monorepo games/soldier design infra (procgen noise+activation net, mapeval
scoring) into a standalone, GAME-AGNOSTIC open-world helper library at tools/openworld/,
then used it to author the raid tier's layout (data-driven, per the plan).
- world.py: agnostic World / POI / Obstacle model + multi-octave fBm substrate (the
  "raw potential" field everything reads).
- palette.py: paletting -- named biome palettes (materials, key/fill/accent colours,
  per-palette archetype allow-masks + spawn weights). Ships 5 generic ones.
- score.py: soldier's mapeval ported faithfully -- openness/flow/cover/verticality +
  connectivity HARD GATE + landmark + scale -> 0..100, S..F. Exact weights
  (0.30/0.22/0.16/0.16/0.06/0.10, x0.25 if disconnected).
- activation.py: the "little net with activators" -- feature vector (height/slope/
  roughness/openness/depth) -> per-archetype perceptron -> activations.
- goals.py: GOAL SETTING -- Poisson-spaced candidate sampling, net picks each site's
  archetype (gated by palette mask), guarantees a landmark.
- path.py: PATHING -- grid A*, point routing, POI patrol tours, navigability check.
- optimize.py: OPTIMIZATION -- hill-climb that mutates cover + re-scores, keeping only
  connected improvements (demo: 76.9 -> 82.7).
- test_openworld.py: self-test, registered as ctest #46 (now 50/50 pass).
- gen_raid.py: uses the library to design a 3-phase RAID (Descent[void] 80.8A ->
  Gauntlet[frost] 82.6A -> Unicron's Heart[ember] 73.9B w/ a boss), writing
  engine_src/data/raid_plan.json for the in-game raid to consume NEXT turn.
50/50 tests pass. The raid tier will read raid_plan.json and instantiate the phases --
that's the next build. HONEST: library is unit-tested + deterministic; the in-game raid
wiring is the follow-up.

## Update 106 -- RAID MODE (the endgame, consuming the library-authored plan)
Wired the in-game raid tier to the data the openworld library generated last update.
- game.c now #includes the generated engine_src/exengine/raid_plan.h (phases -> maps +
  encounter positions/kinds, baked by tools/gen_raid.py). Regenerate the raid by re-
  running that script -- level design is data, not hand-placement.
- EX_MODE_RAID "raid": 3 GATED PHASES -- The Descent (kuiper) -> The Gauntlet (frostline)
  -> Unicron's Heart (unicron). Each phase swaps to its map and lights the plan's
  encounter BEACONS; clear every encounter to advance.
- Encounters: objective beacons clear on a 6-kill purge in their zone (kills banked to
  the nearest active beacon off the shared kill hook); the phase-3 BOSS beacon clears
  when the map's warlord dies -- and the warlords already spawn per-map (Blackstar on
  kuiper, Cryotek on frostline, UNICRON on unicron), so the ladder self-populates.
- Clear phase 3 -> RAID COMPLETE (full heal + fanfare). HUD shows phase name + encounters
  cleared. Continuous waves feed each phase.
50/50 tests pass (openworld lib test included). HONEST: logic + build verified; pacing
(6-kill zones, phase flow) + feel GPU-unverified. The raid is fully re-generatable --
change a seed/palette in gen_raid.py and the whole endgame relays out.

## Update 107 -- NEW MODE: TIME TRIAL (calm run + parkour) [openworld-authored]
The quiet, scenic counterpoint to the raid -- an opening RUN RACE with mid-course
PARKOUR through a beautiful landscape, no combat.
- tools/gen_race.py uses the openworld library to author the course: goal-setting drops
  checkpoints, the patrol tour flows them into a racing line, and the height substrate
  picks which gates sit UP HIGH as parkour. Bakes engine_src/exengine/race_plan.h (13
  checkpoints on the beach, 2 elevated parkour gates). Regenerate = new course.
- EX_MODE_RACE "time trial": loads the calm map ENEMY-FREE (waves suppressed), spawns you
  at the start, and runs a clock. Hit each checkpoint gate in order; a shockring + chime
  banks it. Finish all -> your time.
- PARKOUR gates are elevated: race_start builds a 3-step stepping-platform stair (real
  map colliders + models) under each, so you jump/climb/jet up to them -- true traversal,
  not a floating marker. Gate rings show current (bright) + next two (dim), parkour gates
  tinted apart.
- HUD: running clock, checkpoint x/N, and a PARKOUR cue when the next gate is up high.
Reuses the raid beacon models for gate rings; data-driven like the raid. 50/50 tests
pass. HONEST: logic + build verified; course feel + parkour reachability (step spacing,
gate reach radii) are GPU-unverified tuning targets. Parkour-gate count follows the
height field -- lower the threshold in gen_race.py for more.

## Update 108 -- raid ROLE MECHANICS: force flyers down, grapple up, min-3 SYNC (team-first)
Made raid encounters demand different roles + team comp, per the design brief. Each
encounter now carries a MECH + min_units, assigned by the openworld library from the
substrate (gen_raid.py -> raid_plan.h: high ground -> AIR, broken -> GROUND, some -> SYNC).
- GROUND field (mech 1): an anti-air ceiling over the zone -- fly inside and you're
  SLAMMED to the deck + chipped, so flyers must come down and fight it grounded.
- AIR node (mech 2): an ELEVATED node (14u up). Reach it by flying OR, for a ground
  unit, GRAPPLE -- [G] near it swings you up and in (creative traversal: grab/cable/climb).
- SYNC (mech 3): needs >= min_units on the plates AT ONCE. Solo you can't stand on three
  plates -- so up to 2 ALLY DRONES field the seats (you + 2 = the required three; six is
  the multiplayer ceiling). Drones trail you and man the outer plates near an active SYNC.
- PURGE (mech 0) + boss unchanged. HUD now teaches the active encounter's mechanic
  ("GROUNDING FIELD -- stay grounded", "AIR NODE -- fly up or [G] grapple", "SYNC -- hold
  plates", "KILL THE WARLORD"). Beacon tint reads the mechanic.
Varying comps get through by different means; certain parts (SYNC) genuinely need three.
50/50 tests pass; raid fully re-generatable. HONEST: single-player with drones filling
team seats -- true 6-player co-op needs netcode (not built); the mechanics are designed
team-first and are GPU-unverified for feel (field radius 22u, grapple impulse, plate
spacing 8u are tuning targets).

## Update 109 -- bugfixes from live playthrough: degenerate-part tile glitch + airborne gate
Two fixes off a real run (Mac, campaign on pluto):
- TILE GLITCH (bot mode): the renderer treats an all-zero scale3 as "use uniform scale
  1.0" -> a full-size default CUBE, and a single-zero axis as a flat sliver. The old
  guard only hid FULLY-collapsed parts, so a part with one collapsed axis rendered as a
  stray box/tile. Now: fully-collapsed parts are hidden (as before) AND any visible part
  with a near-zero axis has that axis clamped to 0.02, so no default cube or flat sliver
  can appear. (Vehicle mode already avoided this via its own pose path -- matches the
  report that only bot mode glitched.)
- AIRBORNE STRETCH now robot-only: the jump/fall vertical stretch (U103) was also firing
  in vehicle mode (vehicles are "airborne" while flying), stretching the flying vehicle.
  Gated to !in_vehicle -- correctness + removes an odd vehicle deform.
- The "Texture does not exist spec_*/emit_*" lines in the log are harmless: the engine
  falls back when a map lacks those channels (many maps ship without them).
50/50 tests pass. NOTE: the "bent backwards in bot mode after vehicle" report isn't
reproducible headlessly -- traced the pose, and normal robot mode can only droop FORWARD
(damage sag) or lean toward MOVEMENT; nothing bends it back when idle. Likely the
transform FOLD animation (parts hinge up to ~200 deg mid-fold) or the head tracking your
up-look. Need one detail to fix precisely (see chat).

## Update 110 -- FIX: stuck "human-scale vehicle" (flat/upside-down, mode-locked)
Root-caused the report: robot->vehicle, then the "player key" (H = human/titan scale
toggle) -> body goes feet-first/flat/upside-down and you can't change modes.
- CAUSE: H toggles g_human_scale but left g_avatar_xform->mode = VEHICLE. So you became a
  VEHICLE posed at human scale (ex_vehicle_layout shrunk to human_k -> a flat, inverted
  mess), and the transform key is gated `&& !g_human_scale`, so mode changes were locked
  -- stranded.
- FIX: the H toggle now snaps the transform cleanly back to ROBOT + IDLE (mode, state,
  blend_timer) every time. Human scale is always the on-foot robot, never a vehicle.
  Doubles as a recovery -- tapping H clears ANY stuck transform.
- HARDENING: the pose's in_vehicle is now `(mode==VEHICLE) && !g_human_scale`, so human
  scale can never render the vehicle layout even if mode were somehow stuck.
50/50 tests pass. This one is confidently fixed (exact cause traced), unlike the earlier
"bent backwards" report which still needs the during-transform-vs-standing detail.

## Update 111 -- ambient METEORS + Unicron-fragment structures + terrain relief (Ultra feedback)
Off live Ultra feedback ("barren, flat, nothing moving, structures too basic, no Unicron
read"):
- AMBIENT METEORS: on barren/space maps (outer_rim/kuiper/wastes/pluto/mars/unicron/
  trench/reclaim/shield) meteors now streak down every ~4-11s with a fiery trail and
  IMPACT -- a crater + blast + shockwave that scars the ground and rocks you if it lands
  near. 5-strong pool, ambient (doesn't hunt you). Sky + ground are alive now.
- UNICRON FRAGMENTS on outer_rim: a huge segmented shell-rib RING (10 tall pillars @ r74),
  two broken machine PORTALS (arches), three jagged hull-TEETH spires, and 6 terrain
  MOUNDS for relief. Reads as the planet-eater's wreckage, not flat asteroid boxes.
  outer_rim 77.9 B -> 86.3 A (verticality 0.92); map mean 70.5 -> 71.0.
- (Floating motes already drift on every map; meteors add the big motion the space maps
  were missing.)
50/50 tests pass; outer_rim validates (65 colliders, under the 120 cap). HONEST: logic +
structure verified; the look (meteor scale/rate, Unicron silhouette) is GPU-unverified.
Same treatment can extend to kuiper/wastes/mars next.

## Update 112 -- more map atmosphere: mars/wastes/kuiper relief + themed meteors + gravity
Rolled the atmosphere pass through the rest of the space maps + added variety:
- VERTICAL STRUCTURES (the flat-map fix): mars gets a colossal crashed-fragment
  superstructure + fractured spars + gantry arch + taller mesas (75.7 B -> 85.6 A, vert
  0.35->0.72); wastes gets reared-up hull slabs + snapped tower spars + a broken gateway
  (79.4 B -> 85.2 A, vert 0.36->0.60); kuiper gets towering Blackstar ship-hull spars
  (77.9 B -> 80.6 A, vert 0.51->0.63, 114 colliders, under cap). Map mean 71.0 -> 72.0;
  all 4 space maps now A-grade.
- THEMED METEORS: colour by world -- ICE shards (cyan) over frost/pluto/kuiper/shield,
  MOLTEN (deep orange) over core/ember/mars/foundry, rock elsewhere. Trail matches.
- GRAVITY VARIATION: meteors now fall by a per-map gravity (deep space kuiper/outer_rim/
  pluto 0.55 = slow fall + wide lateral drift; core/unicron 1.35 = heavy fast plunge).
  Your "gravity variations for objects" note, on the ambient debris.
- MOTES boosted: bigger (0.16-0.26) + 35% brighter so the floating particles actually read.
50/50 tests pass; all maps validate. HONEST: structure + logic verified; the look (meteor
tints, silhouettes, mote density) is GPU-unverified.

## Update 113 -- STRUCTURAL SKELETON: designed ribs + limb joints, varied by archetype
Addressed "uniqueness should affect the skeleton of the limbs and the ribs, not just
scaling." The body was distinct part-meshes (pchest/plimb) scaled per class + random
greeble panels -- no designed structure. Added a dedicated STRUCTURAL pass (separate from
greebles):
- RIBS: horizontal slats stacked down the chest into the abdomen, front-facing, tapering
  to the waist. Tinted a darker recessed chest tone so they read as structure.
- JOINTS: sphere knuckles at shoulders, elbows, and knees -- the limb skeleton's pivots --
  tinted the energon accent so the frame reads as a POWERED skeleton.
- VARIES BY FRAME ARCHETYPE (from alt_mode): FLYER = 5 caged thin ribs + slim exposed
  joints; HEAVY/SIEGE = 3 thick armour bands + big reinforced knuckles; AGILE/BEAST = 4
  ribs; BALANCED = 4. So two builds now differ in skeletal STRUCTURE, not only proportions.
- Rides the body yaw + gait lean exactly like the base parts; hidden in vehicle/human/
  transform/menu/death; degenerate-scale guarded (no default cubes). 30-model pool
  (12 sphere joints + 18 rib slats).
50/50 tests pass. HONEST: geometry + placement verified; the actual on-screen read (rib
spacing, joint size, whether it declutters the "box jumble") is GPU-unverified -- please
eyeball a flyer vs a tank on Ultra.

## Update 114 -- freefall animation + patrol events/sounds + frostline/pluto relief
Three threads in one pass:
- FREEFALL ANIMATION: a sustained drop (airborne + falling faster than ~10u/s) eases the
  body into a splayed SKYDIVE FLAIL -- arms flung up + out, hands high, legs splayed and
  trailing -- ramping with fall speed and easing out on landing, with a one-shot whoosh on
  entry. Reads the fall on the whole silhouette (beyond the older vertical stretch).
- PATROL -- new event + more sound: added ONSLAUGHT (a heavy timed purge, 12+ scaling
  quota, 42s, richer loot + 500 energon, red-alarm beacon, camera-shake klaxon on spawn)
  alongside SURGE + HOLD. Per-kind START sounds (launch / transform / explosion), a soft
  chirp as the next event spins up, urgent COUNTDOWN beeps under 6s, and a fail thud.
  HUD + kill-hook + beacon tint all handle the new type.
- MAPS: frostline was nearly flat (vert 0.15) -> titanic sheared-ice monoliths + tall
  jagged spires + a frozen arch + an ice-pillar ring -> 70.6 B -> 89.1 A (vert 0.87);
  pluto got tall corner containment towers + two more gates + a taller command spire ->
  62.8 C -> 71.9 B. Map mean 72.0 -> 73.6.
50/50 tests pass; maps validate under the collider cap. HONEST: logic/structure verified;
the freefall read + event feel + silhouettes are GPU-unverified.

## Update 115 -- mantle fix + climb/mantle animations + physically-based (slower) gait
Three locomotion fixes:
- MANTLE (the "climb to the top but never mount" bug): at the crest the climb used to keep
  HUGGING the wall face (velocity into the wall), so you pinned at the top and never got
  up. Now, once you pass wtop-0.55, a real MANTLE fires -- a short pop up + a firm FORWARD
  carry (fdx/fdz * 8.5) that vaults you over the lip onto the top surface. Tagged with a
  0.42s g_mantle_t for the animation.
- CLIMB / MANTLE ANIMATIONS: wall-climb now plays an arm-over-arm reach (one arm reaches
  high while the other pulls, alternating knee drive, body hugging the wall); the mantle
  plays a pull-up (both arms plant forward + down, chest heaves over, knees tuck onto the
  ledge). Climb TYPE varies the stroke rate by frame archetype -- heavy hauls slow +
  powerful (4.2), flyer scrambles fast (8.5), others mid (6.2).
- GAIT -- slower + physically based: stride FREQUENCY now rises with sqrt(speed) (the
  Froude relation for legged locomotion) and saturates, so legs no longer WHIRL at sprint
  (~0.6 Hz at run vs the old ~0.9, ~1.1 Hz at sprint vs old ~2.7). Stride LENGTH grows with
  speed (up to 1.5x, was capped at 6 u/s) so faster = LONGER strides carrying the ground,
  and a 2nd harmonic breaks the clean sine into a quick plant + slow lift (organic, weighted).
  Applies to enemies too (shared ex_gait_advance).
50/50 tests pass. HONEST: logic + math verified; the mantle reliability, climb pose, and
new stride feel (incl. any foot-slide from the slower cadence) are GPU-unverified.

## Update 116 -- transforms respect limb attachment: joint-hinged mechanical origami fold
Reworked the transform morph to be faithful to the lore -- limbs now FOLD at their joints
and stay connected, instead of detaching and floating outward.
- OLD: each part was pushed radially OUTWARD (detach*1.5), arced, and spun up to 200 deg --
  so joints came apart and parts flew around the body mid-transform.
- NEW: each limb bone HINGES about its attachment joint (pivot compensation keeps the joint
  fixed while the bone rotates), and every child inherits its parent's tip displacement, so
  the kinematic chain stays connected end-to-end: SHOULDER->ARM->HAND and
  HIP->THIGH->SHIN->FOOT. Knees jackknife, elbows close, ankles fold under, torso panels
  rotate on their edges -- a mechanism articulating, not a cloud of parts.
- No more free yaw spin (a hinge, not a spin); per-chassis fold intensity now drives off the
  chassis fold_spin field (140..300 -> ~0.88..1.08x), which also re-homed that field (the
  dead-field lint had flagged it once the old spin was removed).
- Kept the staggered sequencing, the scale/colour morph, the collapse-to-zero fade, and the
  damaged-joint judder.
50/50 tests pass. HONEST: the FK chain (pivot comp + parent-tip inheritance) is geometry I
can't see run -- the fold should read connected now, but the exact joint alignment + whether
any bone over/under-rotates is GPU-unverified. Please watch a robot<->vehicle transform.

## Update 117 -- openworld library deepened: biomes, cost-pathing, roads, annealing, tiers
Pushed the agnostic open-world helper (tools/openworld/) toward a real toolkit across all
four pillars:
- PALETTING -> BIOMES (regions.py): a World can now wear MANY palettes at once. A RegionMap
  assigns biomes by nearest-seed Voronoi cells; goal placement reads the LOCAL biome's
  allow-mask, so a boss only grows where its biome permits and every POI is tagged with its
  region. build(biomes=[...]) paints a multi-biome world in one call.
- GOAL SETTING -> tiers + constraints: POIs get a difficulty TIER from distance-to-spawn
  (near=easy, deep=hard), bosses are kept a min distance off spawn, landmark still
  guaranteed. POI gained tier + region fields.
- PATHING -> cost-aware + roads + reachability: cost_at (slope+roughness terrain cost),
  astar_cost (least-COST routes that hug valleys/flats), build_roads (a min-spanning ROAD
  NETWORK linking every POI over cost-aware trails), components (label reachable islands).
- OPTIMIZATION -> simulated annealing: anneal() accepts some worse moves (temp schedule) to
  escape the hill-climb's local optima, snapshots + restores the BEST config, and can
  optimise toward a TARGET PROFILE (e.g. {"cover":0.9}) via profile_distance -- optimise for
  a FEEL, not just the blended total.
- ANALYSIS (analyze.py): a headless report -- score/grade, connectivity + island count, POI
  mix by kind/tier/biome, mean spacing, road segments + length. BuiltWorld.report().
Self-test (ctest #46) extended to cover biomes, tiers, cost-pathing, roads, components,
annealing (incl. target profile), and the report. 50/50 tests pass. Library ~940 lines.
Still fully game-agnostic + deterministic; Cybertron's raid/race generators can adopt
biomes + roads next.

## Update 118 -- ambiance: per-biome ambient particles + drifting orbital debris
Made the air + skyline of each world feel distinct and alive.
- THEMED AMBIENT PARTICLES: the drifting motes are no longer a generic grey rise. A
  per-biome style (mote_style) now drives colour AND motion: SNOW falls (pale blue) on
  frost/pluto/kuiper/trench, EMBERS rise + flicker (orange) on core/ember/foundry, DUST
  wind-drifts sideways (tan) on wastes/mars/reclaim, ENERGON SPARKS rise fast + flicker
  (cyan) on cybertron/vault/shield/unicron, neutral elsewhere. Snow reads a touch fatter;
  fire + sparks flicker.
- ORBITAL DEBRIS: 6 big chunks now slowly ORBIT + tumble around the skyline of barren/space
  arenas (radius 62-112, height 22-62, alternating orbit direction, gentle bob), so the
  space maps have constant ambient motion overhead. Hidden on non-space maps.
- (Distant meteor impacts already supply the ambient audio booms; held off on a looping
  ambient sound to avoid repetition.)
50/50 tests pass. HONEST: logic verified; the particle read (snow vs embers vs dust) + the
orbital-debris scale/height are GPU-unverified -- worth an Ultra eyeball per biome.

## Update 119 -- ambiance: energon/ion LIGHTNING storms on charged biomes
Dynamic weather to go with the themed particles + orbital debris.
- On charged worlds (cybertron/vault/shield/unicron/frostline/core/pluto) LIGHTNING now
  strikes every ~7-16s: a jagged 5-segment bolt cracks from the sky (62u) down to the
  ground at a random point near you, a bright flash-glow lights the strike, and a scorch +
  spark burst + a small shockwave land at the base.
- THUNDER rolls in a beat AFTER the flash (0.6-1.2s, scaled by the strike's distance --
  the light-then-sound gap sells the scale), reusing the explosion sfx.
- A close strike (<32u) RATTLES the camera (trauma scaled by nearness), so a nearby bolt
  is felt, not just seen.
- Inactive on non-charged maps. Bolt segments jitter per strike so no two look alike.
50/50 tests pass. HONEST: logic verified; the bolt read (jaggedness, brightness, flash
size) + strike cadence are GPU-unverified -- worth an Ultra look on cybertron/unicron.

## Update 120 -- ambiance: ambient FAUNA (energon wisps + scrap-drone flocks)
Life drifting through the world, so it feels inhabited rather than just decorated.
- ENERGON WISPS: on Cybertronian/charged/calm ground (cybertron/vault/unicron/shield/core/
  beach/frost), ~14 small cyan orbs WANDER on a damped random-walk, bob gently, and twinkle
  -- will-o'-the-wisps of raw energon. Emissive, no shadow.
- SCRAP-DRONES: on the salvage + space worlds (wastes/mars/kuiper/reclaim/outer_rim/trench),
  the same pool becomes a loose FLOCK of small dark mechanical fliers crossing the sky on a
  shared heading that drifts slowly, each with its own lane + wingbeat bob. Lit, flat flier
  shape, tinted apart.
- Reuses one 14-model pool (glowtpl), styled per biome via fauna_style; hidden on other maps
  + in menus. Sits alongside the themed motes, orbital debris, meteors, and lightning.
50/50 tests pass. HONEST: logic verified; the wisp glow + drone read/scale/altitude are
GPU-unverified -- worth an Ultra look on cybertron (wisps) and kuiper/wastes (drones).

## Update 121 -- terrain noise UPGRADE: gradient/fBm/ridged heightfield ground
Replaced the flat floor + random box-bumps with real procedural terrain.
- NEW NOISE: proper gradient (Perlin) noise with a smootherstep fade, fBm (5-octave summed),
  and ridged multifractal (sharp mountain ridges) -- module-level _perlin/_fbm/_ridged in
  gen_world.py.
- terrain_field(): a rolling ground MESH (40x40 grid) driven by DOMAIN-WARPED fBm blended
  with ridged noise, so the ground has dunes, ridges, and hollows instead of a dead-flat
  plane. Kept gentle (amp ~2.8) + mostly positive + FLATTENED toward spawn so you deploy on
  level ground. Per-quad normals for correct lighting.
- natural_terrain() rebuilt: lays the noise ground, then CLUSTERS mounds on the high-ground
  ridges (fBm gate) instead of scattering them randomly -- coherent hills, not confetti.
- Applied to all 10 outdoor maps (wastes/frostline/emberpit/foundry/mining/outer_rim/
  reclaim/trench/beach/mars): each now carries a ~3200-tri rolling heightfield.
50/50 tests pass; every map validates; data ~6.5MB total. HONEST: the terrain MESH is
visual (the flat base hull + box structures still drive collision), so raised ground clips
a little under the gentle amp -- looks far better, and gameplay collision is unchanged.
Making it fully walkable would need a heightfield collider (a bigger job).

## Update 122 -- procedural, connected, DYNAMIC fancy ground (fragment shader)
Added a live procedural ground overlay in forward.glsl -- seamless, animated, and it breaks
the old tiled-texture look.
- NEW u_time uniform: plumbed ex_render_time (renderer.c/.h) fed by the game's anim clock,
  set alongside u_gfx_quality each frame.
- GLSL value-noise + fBm added to the FS. On UP-FACING ground (world normal.y>0.6, recovered
  via u_inverse_view so it's seamless world-space -- no UV tiling seams), the shader overlays:
  energon VEINS (domain-warped fBm ridge lines) that a PULSE of energy travels along; fine
  procedural MOTTLING that breaks the texture repeat; and a slow breathing SHIMMER. Veins are
  energon-cyan biased toward the map's element tint. Gated to quality>=1 (mobile skips it).
- Everything animates off u_time -> the ground flows + pulses continuously; world-space coords
  keep it connected across the whole map.
FS brace/paren balance verified; 50/50 tests pass. HONEST: this is a SHADER effect I can't
run headlessly -- compiles + balances, but the on-screen look (vein density, flow speed, glow
strength) is GPU-unverified. Dials: the 0.06 vein frequency, 0.05 flow, and 0.30/0.55 glow.

## Update 123 -- character animation: articulated gait (knee/elbow flex) + firing arm
Legs + arms now bend at their joints instead of swinging as stiff pendulums.
- GAIT ARTICULATION: through a stride the KNEE now flexes -- the shin bends back as the foot
  lifts on the forward swing (peak ~49 deg) and straightens as it plants -- and the elbows
  flex subtly counter-phase. Joints stay attached: a new limb_bend() pivots each bone about
  its joint (position-compensated) and carries the foot/hand along, so the leg reads as a
  real 2-bone limb. Only while walking on the ground (not air/climb/transform).
- FIRING ARM: the weapon (right) arm now raises to AIM and RECOILS on each shot (driven by
  g_recoil) -- the arm levels forward then jerks back with the kick, so shooting reads on the
  body, not just the muzzle.
- Both layer cleanly over the existing structural skeleton, gait, freefall, climb, and
  mechanical-fold work via g_part_pitch + base_poses.
50/50 tests pass. HONEST: geometry verified; the knee-flex read (amount/timing) + firing-arm
raise are GPU-unverified -- worth watching a run and a firefight. Dials: KMAX 0.85 (knee),
the 0.4 elbow, and the firing-arm 0.22/0.42 raise + 0.55 recoil.

## Update 124 -- character: IDLE animation + per-archetype PAULDRONS/guards
Both sides of the ask -- a living idle + more distinct armour silhouette.
- IDLE ANIMATION: standing still, the bot now shifts its weight foot to foot, BREATHES (the
  chest rises + swells on the cycle), the head DRIFTS/scans, the loaded leg takes a soft knee
  bend, and the arms settle + sway. Fades in as you slow to a stop (full at standstill), so
  it never fights the walk. No more frozen mannequin between fights.
- PAULDRONS + GUARDS: the structural skeleton pass now adds ARMOUR PLATES -- shoulder
  pauldrons that jut outboard + up, plus knee + elbow guards -- sized by frame archetype:
  a HEAVY frame wears big slab pauldrons (0.62), a FLYER slim swept ones (0.34), agile/
  balanced between. Body-armour tone (not the energon-joint glow). 6 new box models in the
  skel pool (now 36), hidden in vehicle/human/transform/menu like the rest.
50/50 tests pass. HONEST: geometry verified; the idle read (amount/rate) + pauldron scale/
placement are GPU-unverified -- worth standing still + comparing a tank vs a jet on Ultra.

## Update 125 -- procedural ground v2: cracked energy-PLATES + flowing veins
Deepened the procedural ground shader (U122) into a full material.
- NEW ex_cell(): a Voronoi/cellular noise in the FS (nearest-feature distance + per-cell id).
- The ground now layers, all seamless (world-space) + animated (u_time) + element-tinted:
  (1) cracked energy PLATES -- warped Voronoi cells read as metal plates, each with its own
      brightness, and the SEAMS between them glow + pulse on a per-plate cycle;
  (2) flowing VEINS threading across the plates (kept from v1);
  (3) fine mottling + a slow breathing sheen.
- Warped so the plates aren't a rigid grid; biased toward the map's element tint so fire maps
  crack orange, ice blue, Cybertron cyan.
FS code braces/parens balance (comment-only paren split is harmless); 50/50 tests pass.
HONEST: shader can't run headlessly -- compiles + balances, but the on-screen look (plate
size 0.09, seam width 0.07, glow 0.22/0.40) is GPU-unverified. Easy dials once seen on Ultra.

## Update 126 -- FIX robot-mode artifacts + Cybertron buildings + Voyager outer_rim
Two things: fixed the animation artifacts I'd introduced, then pushed the maps.
- ARTIFACT FIX: my recent per-joint bends (knee/elbow articulation + idle) rotate the limbs
  via g_part_pitch, but the STRUCTURAL SKELETON (joints, ribs, pauldrons) was still placed
  from the LIVE base_poses -- so the glowing joints + plates detached from the bent limbs.
  Now the skeleton is placed from a SNAPSHOT taken before the fine bends (it still rides the
  gross gait/freefall/climb), so joints stay seated. Also softened the knee flex (0.85->0.70)
  and made idle + gait mutually exclusive (no double-bend in the 2-3 speed overlap).
- CYBERTRON now has a CITY: 17 proper buildings -- wide towers of varied footprint + height,
  many stepped with rooftop antenna spires -- a metropolis around the citadel (the old thin
  columns now read as toppled ruins among them). 86.6 -> 90.1 S (vert 1.00).
- OUTER_RIM -- Voyager's outer system: pathable asteroid SPACEWALKS (rising stepping
  platforms bridged by catwalks), icy MINI PLANETS (a dwarf world + crystalline moon + rock
  moon), GIANT ICE SPIKES, and rare DIAMOND crystals. Charted after the Voyager probes' Kuiper
  frontier. 84.8 A.
- KUIPER: added decorative ice spikes + diamonds (non-blocking, kept under the 120 collider
  cap). Added a `collide=` option to spire().
50/50 tests pass; all maps validate under the collider cap. Map work is collider-verified;
the building/spacewalk READ is GPU-unverified.

## Update 127 -- weapon projectiles: thin PULSING energy streaks (not static blobs)
Reworked the projectile look -- they were fat static spheres, now they're animated energy.
- SHAPE: bolts are now THIN + LONG tapering streaks (width ~0.6x, length ~4.6x the base,
  base trimmed 0.45->0.38), oriented along travel in BOTH yaw and pitch so arcing shots lie
  along their arc instead of facing flat. No more round blob.
- ANIMATED ENERGY: each bolt now PULSES its brightness (fast energy throb), FLICKERS its
  width (lightning-fast shimmer), and TAPERS as it travels -- so even the emissive-solid
  bolts read as live energy, not a static ball.
- LIGHTNING CRACKLE: a fast lateral (perpendicular-to-travel) jitter gives an electric wobble
  down the streak, so energy shots crackle rather than glide dead-straight.
- All driven off g_anim_t + per-bolt phase so no two look identical frame to frame.
50/50 tests pass. HONEST: this is a render/visual change I can't run headlessly -- logic +
math verified, but the on-screen read (streak length, pulse rate, crackle amount) is
GPU-unverified. Dials: 0.6/4.6 scale, 42/95 Hz pulse/flicker, 0.10 crackle. NOTE: distinct
missile-pod / shell / per-weapon projectile MODELS are a larger follow-up (the vfx bolt is
one generic type today) -- this pass makes the shared bolt read as animated energy.

## Update 128 -- drivable terrain: baked heightmaps + vehicle SUSPENSION + slope tilt
The rolling terrain was visual-only (entities drove on the flat base). Now it's DRIVABLE.
- HEIGHTMAP BAKE: terrain_field now writes a .hgt heightmap (res, half, grid) beside each
  map, packed into data.ex. The game loads it (load_map_heightmap) + samples it bilinearly
  (terrain_sample), matching the visual mesh exactly (both scaled by THEME_VIS_SCALE=16).
- SUSPENSION: entities now RIDE the heightfield via a damped spring (g_susp_y) so bumps
  smooth out instead of jolting -- SOFTER in a vehicle (stiffness 8 = smooth racing) and
  firmer on foot (15). Never sinks through platforms; you can still jump off.
- SLOPE TILT: the vehicle samples terrain fore/aft + left/right along its OWN axes and tilts
  the chassis to the slope (nose up a rise, bank across a crossfall), blended into the body
  lean so it pivots at the wheels.
- DUST: driving fast on dirt kicks up biome-tinted dust behind the wheels (deformation feel).
- Terrain amp raised (2.8->5.2, ~67 scaled units) + the flat spawn zone shrunk so far more of
  the map has relief -- it read too flat before.
50/50 tests pass; all maps validate. HONEST: the SUSPENSION FEEL (stiffness, tilt amount,
whether it's smooth vs floaty) + the new relief are play/GPU-unverified -- this is physics I
can't feel headlessly. Dials: stiffness 8/15, tilt 0.75, amp 5.2. NOTE: true carved RUTS
(persistent mesh deformation) aren't done -- that needs dynamic mesh editing; this delivers
the suspension + ride + dust, and the terrain is now genuinely drivable.

## Update 129 -- creative terrain variation: carved paths, dunes, ice slabs, sky-highways
Per feedback (build varied DRIVING SURFACES as geometry, not lean on suspension physics):
- CARVED FLAT PATHS: terrain_field now carves flat drivable ribbons toward a road network (a
  ring road + two crossing routes) -- the heightfield flattens to a low road height near the
  paths (smootherstep shoulders), so vehicles get smooth flat routes threading the hills, and
  mounds are kept OFF the paths. This is the "flat sections + carved paths".
- DUNES: wastes / mars / beach now use a stretched ridged-wave dune field (long wind-blown
  crests) instead of generic rolling -- proper desert dunes with drivable faces.
- ICE SLABS: new ice_slab() -- big frozen chunks with FLAT drivable tops + an ice ramp up;
  added 3 to frostline so there are broad flat surfaces to race between the spikes.
- CYBERTRON SKY-HIGHWAYS: new skyway() -- elevated flat roadways between the towers. Cybertron
  now has a 4-point elevated ring highway (height 12) + a high cross-span (15) you can DRIVE in
  vehicle mode in the sky, plus a ground ring road below. Both registers, as asked.
- Terrain-follow STIFFENED (13/20) so it reads as direct ground contact, not a floaty spring.
All maps validate UNDER the 120 collider cap (cybertron trimmed to 109: longer skyway
segments, non-colliding pylons + ground road). 50/50 tests pass. Cybertron 87.4 A, frostline
87.3 A, mars 86.6 A. HONEST: the DRIVE FEEL over paths/dunes/slabs + the sky-highway routes
are play/GPU-unverified. NOTE: the skyway decks DO collide (you can drive on them); the
ground road + pylons are visual.

## Update 130 -- SIX NEW WORLDS: NASA-archive moons + the clone homeworlds (+ lore drip)
The space bridge now reaches a real outer system and the clone lines' homeworlds. 24 maps.
- NASA ARCHIVE WORLDS (from the Voyager / Galileo / Cassini-Huygens / Juno records):
  * EUROPA (75.0 B): cracked ice shell scored by segmented double-ridge LINEAE (steppable,
    with route gaps), tilted chaos-terrain rafts, a south plume field, and the ancient
    listening post over the warm ocean -- "something answered from below the ice".
  * IO (79.6 B): sulfur plains, glowing LAVA LAKES ringed by calderas (Loki Patera and kin),
    volcano cones with towering plume vents, sheer Boosaule-class mountain blocks -- the
    tide-forges where the magma-clone fires never cool.
  * TITAN (78.6 B): Kraken + Ligeia methane seas with shoreline reefs, equatorial DUNE belts,
    a cryovolcano, labyrinth mesas, scavenger fuel-tithe rigs, and the Huygens lander kept as
    a shrine -- the only world where fuel falls as rain.
- CLONE HOMEWORLDS (the warlords are CLONE LINES; each line's civilization grew strange
  around its element):
  * GLACIEM (74.1 B, Cryotek): a civilization that lives SLOW -- vault-ziggurats carved DOWN
    into the glacier, lattice pools where clones grow over centuries (half-formed bodies
    visible in the ice), aurora crystal groves, archive slabs, a glacier high road.
  * CARRION (85.0 A, Blackstar): a wreck-pile world lived FAST -- leaning hull-stack cities
    of mismatched salvage, clan masts, a gantry high-road, shipbreak trenches, the trade-pyre
    meet-ring. Every body a patchwork; status is the rarest wreck worn openly.
  * HELIX (84.0 A, Jhiaxus): the architect's garden-world -- twin spiral helix towers, ordered
    gene-vat rows with bodies mid-growth, a mirrored obelisk avenue, organic-metal groves.
    Clones re-engineered between lives, perfected past identity.
- HOMEWORLD PRIMES: fight a warlord ON ITS HOMEWORLD and the line's PRIME rises instead --
  12x HP (vs 8x) and a body-detail intro (glacier-grown / welded-wreckage / seamless
  organic-metal bodies in their respective elements).
- SPACE BRIDGE: patrol/escalation circuit extended 6 -> 12 worlds (kuiper, outer_rim, europa,
  io, pluto, titan, mars, glaciem, carrion, trench, helix, unicron).
- LORE DRIP: patrol now surfaces ARCHIVE FRAGMENTS every ~55-85s -- 14 rotating lines mixing
  Blackstar-script beats (mantle cache, the bunker over the ancient site, Hot Rod's fate),
  older strata (Age of Foundries, First Bridge era, the Silent Century), and the clone
  civilizations. Menu map table extended w/ names + lore descs; full table now selectable.
- Per-world atmosphere: lighting, sky tint, fog (Titan's thick orange haze), gravity
  (europa/titan 0.6, io 0.7), particles (snow/embers/dust/sparks), meteors on the bombarded
  moons, storms on helix/glaciem, wisps/drones per world.
All 6 validate under the collider cap; map mean 73.6 -> 74.7 over 24 maps; 50/50 tests pass.
HONEST: layouts + wiring verified headlessly; the READ of each world is GPU-unverified.

## Update 131 -- shake ease, faster vehicles, leg-whirl FIX, robotic understructure
Feel fixes + a big skeleton detail pass:
- CAMERA SHAKE: an attack/decay ENVELOPE now chases the trauma (attack 10/s, release 6/s),
  so a hit ramps the shake in over ~0.1s instead of snapping on the first frame (the
  jerkiness), and swing amplitude trimmed 10% (1.1 -> 0.99).
- VEHICLE SPEED: ground racer 20 -> 24, jet 34 -> 41 (~20% faster; they felt slow).
- LEG-WHIRL FIX (dinos/specialists/fast classes): the class/body gait cadence multiplied
  AFTER the physical frequency clamp, so quick kinds (slipstream 1.45, scout 1.40) spun
  their legs absurdly fast. Cadence is now sqrt-compressed at both call sites (1.45x ->
  ~1.20x) and the cap lowered 8.5 -> 7.4 -- character stays (quick kinds still step
  quicker) but nobody whirls.
- ROBOTIC UNDERSTRUCTURE (skeleton pool 36 -> 54; the same parts you'd hand-craft in
  tools/part_editor.html, composed from the pool):
  * SPINAL COLUMN -- 4 vertebra blocks chest -> abdomen down the back, alternating
    armour / energon-lit (a powered mechanical spine);
  * HOLLOW LIMB STRUT-FRAMES -- each thigh + upper arm carries a pair of thin outer rails
    running its length, so the bone reads as an open strut frame (hollow, panelled,
    cybertronian) instead of one solid slab;
  * ANGLED CHEST PANELS -- two plates flaring from the sternum (the classic chest);
  * HIP RING -- two pelvis plates bridging abdomen -> thighs (a girdle grounding the legs);
  * SHIN FINS -- a thin blade fin off each calf (bigger on flyers -- speed-line silhouette).
  All ride the pre-articulation snapshot (no artifact regression), hidden in
  vehicle/human/transform/menu like the rest of the pool.
50/50 tests pass. HONEST: shake ease + speeds + cadence are feel changes and the
understructure is silhouette work -- all GPU/play-unverified. Dials: attack/release 10/6,
swing 0.99, speeds 24/41, cadence sqrt + 7.4 cap, rail thickness 0.05.

## Update 132 -- the SIX-FILM CAMPAIGN: 12 chapters, both trilogies, the emotional arc
Rebuilt the campaign (EX_MAX_MISSIONS 8 -> 12) as the full Blackstar saga -- two chapters
per film, start to finish, with the cutscene slides carrying the EMOTIONAL side of every
beat (all wording original to this game):
- TRILOGY ONE: I.1 the pluto line (Witwicky, the old soldier -- "quiet is what safe sounds
  like"); I.2 the beach opens (Aiden, a nobody who doesn't run); II.1 the truce (Megatron
  returns CHANGED and asks to talk; Witwicky: "never thought i'd cover HIS flank"); II.2
  the mantle dive (Hot Rod catches the fall he once caused -- the Matrix opens for the one
  who reached without wanting it); III.1 the drowned approach (Rodimus leads, and hates how
  the word sounds); III.2 beneath the pacific (Megatron under the collapsing plates:
  "go." -- they never find him; Prime marks the spot anyway).
- TRILOGY TWO: IV.1 the scavenger king (Blackstar finds hope FUNNY); IV.2 the carrion court
  (the clone clans at home -- "he was the first of us to laugh"); V.1 what jhiaxus grew (an
  architect who cannot admit he is afraid); V.2 the garden of helix (a clone asks Rodimus
  "which of us is the copy?" -- "a spark isn't the shape it wears"); VI.1 the vault of ice
  (Cryotek's line are not conquerors -- they are MOURNERS; defend the memory of the dead);
  VI.2 the heart of the god-machine (everyone comes -- even the ashes' king; the Matrix
  pours QUIET into the heart; full circle to Pluto: "now THIS is what safe sounds like").
- MID-MISSION COMMS: once per chapter, ~28s in, a character speaks INSIDE the fight (12
  lines -- Witwicky, Aiden, Megatron, Hot Rod/Rodimus, Blackstar, the clones, Cryotek
  Prime) so the emotional thread runs through the missions, not just between them.
- Chapters route through the new worlds (carrion, helix, glaciem) -- the clone homeworlds
  are now story beats, not just arenas. Objectives vary per chapter (protect/control/
  survive/breach/eliminate); menu campaign flow + boss-mission logic intact.
50/50 tests pass (menu test exercises the campaign + cutscene flow headlessly). HONEST:
the flow is verified; the PACING of slides + the comm timing (28s) are feel, unverified.

## Update 133 -- campaign ch. I.3 "outrun the sea": tsunami-parkour on the beach
A high-intensity early-Earth chapter (inserted after the beach opens; 13 chapters total,
EX_MAX_MISSIONS -> 14):
- REFUGE TOWERS (beach map): three stepped wreck-platforms -- first deck reachable at a
  run, a second deck stepped up + across, a crow's deck safe over any wave, and a spotting
  mast. The map's parkour line. Beach 68.5 C -> 75.6 B (col 54, under cap).
- ENVIRONMENTAL TSUNAMI (chapter-gated): the fissure's exhale drags the ocean out -- a
  3.5s WARNING ("THE SEA PULLS BACK -- GET HIGH", countdown knocks, rumble), then a WALL
  OF OCEAN (glowing crest, foam vfx, bobbing) sweeps the whole arena at 58 u/s,
  alternating shores each set. Caught below ~5.5 units: 22 damage + a violent throw +
  camera trauma. Up a tower: it passes beneath you. Enemies get swept too. Next set
  ~12-19s later -- the survive-75s objective is a drill of sprint-climb-hold cycles.
- CUTSCENES: intro/outro slides carry Aiden's arc -- the kid who ran tsunami drills nobody
  believed in, hoarse from counting heads: "the drill works. tell everyone. THE DRILL
  WORKS." Mid-mission comm added (COMMS now 13, indices shifted).
50/50 tests pass; beach validates. HONEST: the wave's SPEED/height/damage + the tower
climb difficulty are exactly the play-feel I can't verify -- dials: 58 u/s sweep, 5.5
safe-height, 3.5s warn, 12-19s period, 22 dmg. Tell me if the climb window is too tight.

## Update 134 -- AIDEN: skateboard vehicle-mode, salvage, rescue bonuses, LA in ruins
Aiden's kit + his city, per direction:
- SKATEBOARD (human vehicle mode): at human scale the TRANSFORM key now hops Aiden on/off
  a salvaged deck with energon trucks -- near-double pace (0.45x -> 0.85x of robot speed),
  a worn board rendered underfoot that BANKS into carves (rides g_lean_side), toggle
  message in-voice ("kick, push, roll"). Board hidden off-scale.
- RESOURCEFUL: campaign beach chapters scatter 9 SALVAGE pickups across the ruined coast;
  gathering energon at human scale pays a +60 energon "salvaged supplies" bonus (small
  hands, sharp eyes -- salvage pays double).
- SAVING PEOPLE PAYS: rescuing a downed survivor now grants +80 energon +10 hp -- and
  +160 when the rescue is made at human scale ("one of their own pulling them up counts
  double"), with a pickup chime.
- LOS ANGELES, BREAKING (beach map): the destruction around Aiden made visible -- three
  gutted boardwalk buildings (standing/sheared walls, half-fallen tilted roofs), a SNAPPED
  OVERPASS stepping down into the sand with a surviving pylon, 5 burning wreck piles,
  7 listing palm trunks, abandoned lifeguard posts. Slides now name LA and the skateboard
  ("a kid with a skateboard on a los angeles beach").
Beach 70 colliders (under cap), B 75.0; 50/50 tests pass. HONEST: the board FEEL (speed,
bank), pickup pacing, and the LA ruin read are play/GPU-unverified. Dials: 0.85 board
speed, +60/+80/+160 bonuses, ruin density.

## Update 135 -- ANIMATED FACES: robot eyes + human faces + Witwicky named
Faces now carry state, for robots and humans both:
- ROBOT EYES (skel pool 54 -> 56): two emissive eyes on the head that BLINK on a ~3.2s
  mechanical shutter, WINCE narrow under fire (g_hit_flash), FLARE with weapon recoil,
  FLICKER while comms are up (talking), DIM as power fails (hp fraction, floor 0.30), and
  ride the head's look pitch. Eye colour = the build's energon tone.
- HUMAN FACES (survivors): each head now BLINKS on its own phase, SCANS the horizon
  uneasily (slow head-turn), and varies skin tone person to person (stable per-human hash)
  -- a crowd of individuals, not clones of one extra. Downed red-pulse state kept.
- WITWICKY NAMED: the campaign flank ally announces on deploy -- "WITWICKY ON THE FLANK --
  the old suit still flies."
Design note (per discussion): faces stay STYLIZED + animated rather than photoreal
actor-likenesses -- ages the character through design + voice, not through a real
person's face.
50/50 tests pass. HONEST: eye size/placement + blink cadence + the survivor scan are
GPU-unverified -- dials: eye 0.24/0.30 scale, 3.2s blink, 28-deg scan.

## Update 136 -- FACE micro-FSM + LOD + DEPTH PLATES ("true models, not flat + texture")
Per direction: expression as a state machine, LOD-gated detail, and geometric depth --
the part-detail fans loved in the films, not cheap animation.
- EXPRESSION micro-FSM (6 states, priority + 0.6s dwell so expressions READ, never
  flicker): CALM / ALERT (enemy < 28u: eyes wide, brows raised) / COMBAT (hit, firing, or
  a champion up: eyes narrowed, brows down + in -- anger) / WOUNDED (hp < 35%: heavy
  lids, jaw slack) / SEARCH (no enemy 6s: a slow eye sweep scanning) / TALK (comms up:
  the jaw WORKS to the line's rhythm). Signals: nearest-enemy distance, hp fraction,
  hit-flash, recoil, champion slot, comm timer.
- FACE LOD by camera distance: <18u FULL RIG (eyes + brows + jaw + FSM nuance);
  18-40u eyes only; >40u face hidden (the visor carries it). Detail where the camera is.
- NEW FACE PARTS (pool 56 -> 66): a JAW BAR that drops when slack + works while talking;
  two BROW PLATES whose height + inward shift fake the angle (down+in = angry, raised =
  alert, drooped = wounded).
- DEPTH PLATES ("true models"): 7 inset secondary panels riding PROUD of the major faces
  -- chest inset, abdomen inset, head faceplate frame, 2 pauldron rim plates, 2 thigh
  outer plates -- in a darker machined tone, so the big parts read as LAYERED THICKNESS
  with real depth, not flat boxes wearing a texture. LOD-gated with the face.
All ride the pre-articulation snapshot; hidden in vehicle/human/transform/menu. 50/50
tests pass. HONEST: expression readability, LOD pop distances (18/40), and the depth-
plate offsets are exactly the GPU look I can't verify -- every one is a dial.

## Update 137 -- true-model detailing continued: SEGMENTED LIMBS + the WEAPON DETAIL RIG
Model depth pass extended to the limbs and, per direction, the WEAPONS:
- WEAPON DETAIL RIG (new 6-model pool riding the held cannon every frame): a BARREL
  SLEEVE extending the bore, a MUZZLE BRAKE at the tip, an ENERGY CELL slung under the
  receiver that PULSES with heat + flashes with each shot (the gun's heartbeat), a TOP
  SIGHT RAIL, an UNDERBARREL GRIP, and a REAR EXHAUST VANE whose vents ignite orange once
  the barrel runs hot (heat > 0.35). Proportions vary per fire mode (barrel reach + girth
  from the mode index) AND stack with the existing mod reshaping (long barrel, scatter,
  cells) -- ten modes x mods all read as different machines. Metal parts follow the gun
  body tint; everything kicks with the recoil rotation.
- SEGMENTED LIMB PLATING (skel pool 66 -> 78, LOD 0-1): two armour BANDS wrap each
  forearm + shin with a visible GAP between them -- the limbs read as assembled machined
  sections, not single slabs -- plus KNUCKLE GUARDS on both hands and TOE CAPS on both
  feet. Rides the pre-articulation snapshot; brighter armour tone so the banding reads.
50/50 tests pass. HONEST: attachment placement (whether the cell/sight/brake sit right on
each of the 10 gun meshes) + band offsets are GPU-unverified -- the per-mode variation is
math-verified, the LOOK needs your eyes. Dials: every offset/scale in the two rigs.

## Update 138 -- DEFENSE OF EARTH: a 3-phase squad raid (new mode, EX_MODE_EARTHDEF)
The second raid, per direction + the Blackstar-era frame: the hijack fleet comes for
Earth; the system's line holds it at the moons, at Mars, and then someone drinks the SUN.
- 3 PHASES: LUNA PICKET (europa, 8 kills) -> THE MARS LINE (mars, 10) -> THE SUN GAMBIT
  (core, 8 + the channel). Waves refill while the quota is unmet (an onslaught).
- SQUAD OF SIX: you + WITWICKY (the wingman ally flies this mode) + RODIMUS and THE WATCH
  BEARER (escort orbs flanking you, flame + pale-light tinted) + BILLY (support AI: patches
  your plating on a cadence + routes the star) + AIDEN (spotter: warp-signature warnings).
- CLONE vs TRANSFORMER: fight as a CLONE by default (balanced). In the opening 10s window,
  TRANSFORM to declare as a TRANSFORMER -- thinner hull (-20%% max hp) but a unique squad
  role: your spark AURA burns hijacks within 14u (2 dps) and the squad support cadence
  quickens 40%% around you (Billy heals more + more often).
- HIJACK CLONES fight like hijackers: short-range WARP -- when far, they BLINK to 8-12u off
  your shoulder (violet flash out + in, a jolt) -- and FINGERGUNS: full-auto 4-round tracer
  hoses (light hits, fast cadence) inside 26u.
- THE SUN GAMBIT: clear the third quota and Billy opens the bridge -- "I'm routing you THE
  SUN. STAND IN THE LIGHT." A pulsing solar column lands at the arena heart; hold inside it
  5s (percent readout, drains if you step out) -> SOLAR ABSORPTION: full heal, 20s
  overcharge, and the ENTIRE hijack fleet detonates in a cascade. +1500 energon.
  "THE SUN ANSWERS -- the hijack fleet BURNS. EARTH STANDS."
Mode registered in the menu ("defense of earth" + description). 50/50 tests pass. HONEST:
phase pacing, warp/fingergun pressure, aura strength, and the channel drama are
play-unverified -- dials: quotas 8/10/8, warp 6-10s + 8-12u, fingergun 8 dmg/burst @1.5-2.5s,
aura 2dps/14u, channel 5s.

## Update 139 -- PLANETARY LIGHTING: per-world day cycles + the sun DIMS in the gambit
A planet-scale lighting system across the worlds, and the last raid's star drain made
visible in the sky itself.
- DYNAMIC SUN (new u_sun_dir / u_sun_strength / u_sun_color uniforms, plumbed like
  u_time; shader defaults reproduce the old fixed look exactly): the directional sun now
  ARCS across the sky through a day cycle -- amber at the horizons, white at noon, its
  strength breathing dawn -> day -> dusk -> dim night, with the hemisphere ambient dying
  alongside it (night floor kept playable). The key/fill light pair breathes with the day.
- PER-WORLD DAYS: each world keeps its own clock + brightness -- Io's fast sulfur
  tide-days (140s, bright); Mars 260s; Earth-line maps 240s; the far worlds long + dim
  (Europa 300s @ 0.45, Titan 320s hazy, Kuiper/outer-rim 340s, Glaciem 360s, Pluto 400s
  at a faint 0.30 -- the far sun barely more than a bright star). Interiors (core, vault,
  foundry, trench, mining, unicron) keep the fixed studio look.
- THE SUN DIMS (Defense of Earth, phase 3): from the moment the gambit opens the star is
  being tapped -- the WHOLE WORLD'S light drains toward ember dark (sun strength -85%,
  colour collapsing to dying red, key light sinking), deepest while you channel, with the
  beat called out ("AIDEN: the sun is going OUT-- BILLY: that's ME."). At absorption the
  star ROARS back through you -- a 3s white-gold SOLAR FLARE - then the sky steadies.
50/50 tests pass; FS braces/parens balance; uniform defaults guard unset state (no
regression on other render paths). HONEST: this is the most shader-dependent feature yet
-- day-cycle pacing, night darkness floor, and the drain's drama are all GPU-unverified.
Dials: day lengths, smin/smax per world, the 0.85 drain, 3s flare.

## Update 140 -- NIGHT: stars + aurora, hunt-lights, the dawn assault, dark patrols
The planetary day cycle now has consequences -- four systems that make night MEAN something:
- STARS COME OUT: as the sun falls, the ambient motes WHITEN, sharpen, and brighten (up
  to 2.2x) -- snowfields and dust-hazes become starfields; the night sky glitters.
- AURORA over the ice worlds (frostline, glaciem, pluto, europa): night doesn't go black,
  it goes GREEN -- the fill light breathes slow curtains of green-cyan aurora colour on
  two drifting sine periods while the sun is down.
- HUNT-LIGHTS: energon burns brighter in the dark. The procedural ground's seams + veins
  glow up to +90%% as the sun dies (at night they're the light you navigate by), and the
  player's eyes brighten +40%% -- a machine watching from the dark.
- DAWN ASSAULT (campaign ch. II.1, the truce): the parley now opens in the DARK before
  dawn ("MEGATRON: they wait for first light. so do we. HOLD.") -- roughly 45s of night
  defense beside your oldest enemy -- then the sun crests and the trigger fires: "DAWN --
  THE CORRUPTED CHARGE" + a fresh assault wave + Megatron's line. The chapter's emotional
  beat is now told by the sky.
- DARK PATROLS: patrol events spin up ~60%% faster at night -- the dark brings them out;
  day is for travel, night is for trouble.
50/50 tests pass; FS validates. HONEST: all four are look/pacing features on top of the
(already unverified) day cycle -- star brightness 2.2x, aurora amplitudes, seam boost 0.9,
eye boost 0.4, dawn timing ~45s, night patrol 1.6x are the dials.

## Update 141 -- ASSET PASS: six dedicated world TEXTURES (+normals) + three maps rebuilt
Models/textures/assets/maps polish, per direction:
- SIX DEDICATED WORLD TEXTURES (gen_texture, each with a tangent-space NORMAL map; emit
  maps where the pattern glows): europa_ice (pale shell, rust-red linea cracking, glowing
  fractures), io_sulfur (yellow sulfur sediment), titan_dune (orange hydrocarbon sand),
  glaciem_vault (deep-blue carved-glyph archive ice, aurora-cyan glow), carrion_hull
  (rust-brown mismatched plating), helix_bio (deep-green organic-metal circuit weave,
  pale bio-light glow). All six worlds rewired to use them -- each new world now has its
  OWN surface identity instead of borrowing ice/sand/rust/reclaim.
- THREE WEAK MAPS REBUILT (the bottom of the ranking):
  * VAULT 66.4 C -> 71.7 B: a 4-tier crystal ZIGGURAT with a crowning shard, two rows of
    tall archive shards, perimeter arches, and a colonnade ring around the heart.
  * CORE 68.9 C -> 75.1 B: a central CRUCIBLE (obsidian rim + molten pour glow-floor), a
    raised steppable catwalk ring, and four great forge columns with pour-light spires.
  * PLUTO 71.9 B -> 74.4 B: the containment LINE itself now stands -- four rampart
    sections with watch bastions atop, twin relay pylons, and a drivable ice-dock gantry.
- Map mean 75.0 -> 75.6 over 24; all maps validate under the collider cap; normals follow
  the engine's norm_/emit_ naming convention (63 existing norm maps confirmed the pattern).
50/50 tests pass; the archive packs the new textures automatically. HONEST: the six
surfaces + normal-map relief + the three rebuilt silhouettes are exactly the GPU look I
can't check -- colors/seeds/pattern choices are one-line dials in the gen commands.

## Update 142 -- CRITICAL FIX: runaway body shear in robot mode (terrain slope tilt bug)
Player-reported (with screenshot): in robot mode the body parts fanned out sideways,
progressively worse from the ground up, slowly worsening over time. ROOT CAUSE: the
terrain slope tilt (U129) ADDED the tilt into the persistent smoothed lean state
(g_lean_fwd/g_lean_side) every frame instead of into the smoothing TARGET -- on any
sloped heightfield the state accumulated toward a huge equilibrium (tilt*0.75/(dt*6), an
order of magnitude past sane), and the per-part lean shear (which offsets parts
proportionally to their height, pivoting at the feet) scattered the body exactly as
photographed. It also ran in ROBOT mode, not just vehicle.
THE FIX: slope tilt now (1) folds into the lean TARGET before smoothing (no
accumulation possible), (2) is VEHICLE-ONLY -- the robot's legs handle slopes with
articulation; its chassis stays upright, (3) clamps the tilt +/-0.35 and the final
targets +/-0.55/0.65, and (4) decays the tilt state when not applicable.
50/50 tests pass. The fix is structural (accumulation is now impossible by
construction); the remaining unverified bit is only whether the vehicle's slope bank
AMOUNT still looks right on hills.

## Update 143 -- readability + puffy-slope fix + unlock-all + animation life
Four player-reported items:
- PUFFY SLOPES FIXED: terrain_sample used BILINEAR interpolation while the mesh draws
  each quad as two flat TRIANGLES (diagonal (0,0)->(1,1)) -- bilinear bows away from the
  triangle faces, so the rendered slope rode outside the collision ("texture rides
  outside the bounding box"). The sampler now evaluates the SAME two triangles the mesh
  draws: the surface you stand on IS the surface you see, exactly.
- CAMPAIGN UNLOCK-ALL (settings): new settings row "campaign < n/13 open >" -- press it
  to open every chapter for testing. No more being gated by chapter 2.
- READABILITY (the "where do I go / what is shooting me" pass):
  * THREAT MARKER -- whoever hits you gets a spinning RED DIAMOND overhead for 2s;
  * DAMAGE BEARING -- a live HUD readout ("damage: << LEFT / RIGHT >> / AHEAD / BEHIND")
    computed against the camera each frame while the threat is lit;
  * OBJECTIVE CHEVRON -- a gold marker bobbing over where you should be: a DOWNED
    SURVIVOR (urgent red), an active patrol event, the sun channel, or -- the classic
    pain -- the LAST 1-3 enemies in a hunt.
- ANIMATION LIFE: landing squash now bends the KNEES (drive + squash, capped), with
  depth scaled by impact ("harder fall = deeper crouch") on top of the existing body
  cushion; and IDLE WEIGHT SHIFT -- standing still, the body slowly sways + breathes
  (0.035/0.018 lean amplitudes) -- never a statue. (Caught + removed a duplicate landing
  detector before it double-fired; the original impact system owns detection.)
50/50 tests pass; maps regenerated. HONEST: marker sizes/heights, HUD row position, sway
amplitudes are GPU-unverified; the triangulation fix is exact math (verified per-corner).

## Update 144 -- AUDIO PASS: the constant drone tamed + movement/world sounds
Player-reported: a constant annoying sound + wanting footsteps/driving/flying/boost/ice.
- THE CONSTANT SOUND: data/ambient.ogg was a looping stream started at FULL source gain
  on init -- the reported drone. Now a quiet bed at gain 0.16 (atmosphere, not noise).
- FIVE NEW SYNTHESIZED SFX (clean PCM WAVs; both loops are integer-cycle SEAMLESS):
  * FOOTSTEPS -- one weighty thump per leg plant, synced to the actual gait phase (sign
    flip of sin(phase) = a plant), gain scaling with speed; quiet patter at human scale.
  * ICE CRACKS -- a snap + descending ping + crackle tail: every 4th step on the frozen
    worlds (frost/glaciem/europa/pluto/kuiper) cracks the shell underfoot, and the ice
    worlds also settle + crack ON THEIR OWN every 7-15s at low gain (distance).
  * ENGINE loop -- a harmonic motor hum riding a gain throttle: 0 out of the seat,
    0.10-0.40 with ground speed (+0.14 under boost); the skateboard hums it quietly too.
  * JET loop -- a periodic sine-cloud whoosh for flight, same throttle treatment.
  * BOOST -- an ignition pop + rising whoosh one-shot on the nitro rising edge.
  Loops start once at init and live at gain 0 (the API has no stop; gain IS the switch),
  smoothed at 7/s so throttle changes never click.
50/50 tests pass; all five WAVs pack into the archive. HONEST: the MIX (bed 0.16, step
0.28-0.48, engine 0.40 ceiling, jet 0.46, crack cadence) is exactly what I can't hear --
every level is one number, tell me what's too loud or too quiet.

## Update 145 -- AUDIO FOLLOW-UPS: landing thuds, wave crash, warp blink, hit ticks, world beds
The approved follow-up set, all synthesized + wired:
- LANDING THUDS: a deep double-impact (body hit, armor settle, metallic ring) scaled by
  fall force -- gain 0.22 + impact*0.018, capped 0.75. Harder fall = heavier thud, paired
  with the impact-scaled knee squash.
- WAVE CRASH (tsunami chapter): a 1.3s ocean roar (lowpassed noise + low surge + hissing
  tail) at wave LAUNCH (0.6) and louder when it CATCHES you (0.8) -- no more silent wall.
- WARP BLINK: the hijack teleport now has its OWN voice -- a rising glassy FM shimmer
  snapping into a pop-in (0.32s) -- distinct from the transform sound it borrowed.
- HIT CONFIRM: every real chunk of player damage answers with a tight 60ms tick, routed
  through a game-side damage wrapper (all 7+ damage call sites), rate-limited to ~14/s
  and filtering per-frame DoT crumbs (< 1 dmg) -- "am I damaging it", answered by ear.
- WORLD BEDS (two seamless loops, gain-ridden like the engine): deep tectonic RUMBLE on
  io/core/emberpit/unicron/foundry (0.12) + the trench (0.08, the ocean presses); airy
  WIND with a slow swell on titan/wastes/mars/beach/carrion (0.10). Crossfaded at 2/s on
  map change.
50/50 tests pass; all six WAVs pack. (Caught mid-wire: ex_enemy_damage returns a kill
flag some sites read -- the wrapper now forwards it.) HONEST: the MIX again -- thud curve,
crash 0.6/0.8, tick 0.22 @70ms, bed levels -- all ear-unverifiable here; every one a number.

## Update 146 -- smooth terrain, bots respect heights, hero parts + CRITICAL shader crash fix
Three requested threads + two bug fixes (one crash the player hit):
- CRITICAL SHADER CRASH FIXED: the night-hunt-light glow (U140) referenced `sunS` in the
  procedural-ground block, but `sunS` was declared inside the LIT-shading block -- a
  different scope. On many drivers the FS failed to compile ("`sunS' undeclared") and the
  game segfaulted on launch for anyone whose GL rejected it. `sunS` is now hoisted to FS
  main scope (declared once, before both uses); the inner re-declaration became a plain
  read. FS braces/parens balance.
- SMOOTH TERRAIN (no more big facets): terrain_field now emits PER-VERTEX smooth normals
  from the height gradient (central differences) instead of one flat face-normal per quad
  -- lighting blends across the surface so slopes read as continuous rolling ground, not
  faceted panels. Mesh resolution also up 44 -> 60 (finer geometry under the smooth shading).
- BOTS RESPECT TERRAIN HEIGHTS (reported bug): enemies were pinned to y=0 while the ground
  rolled. Now every bot rides the heightfield -- walkers follow the surface (lerp), nothing
  ever sinks below it, knockback ragdolls LAND on the terrain (not sea level), survivors
  stand ON the dunes, and salvage pickups spawn on the surface.
- HERO PARTS (character builder): new customizer row "hero parts < ... >" -- wear a
  campaign character's loadout: their signature COMPONENT + accent element + proportion
  silhouette. 7 donors: Witwicky suit (recon fins, nimble), Rodimus (solar core, tall +
  flame), the Watch bearer (sentinel halo, broad), Megatron-truce (heavy cannon, hulking),
  Cryotek Prime (crest, glacier-massive), Blackstar (harpoon, long-armed), Jhiaxus (cable,
  tall-elegant). Bulk stays class-clamped.
50/50 tests pass; map mean 75.6; all maps under cap. HONEST: the smooth-shading LOOK,
terrain-follow feel for bots, and the donor silhouettes are GPU/play-unverified -- but the
shader fix is the important one: it should launch cleanly again.

## Update 147 -- GRAPHICAL CEILING: god rays, height fog, reflections, sun glint, particles
Engine/graphics pass per direction (the post chain -- bloom/ACES/grade/vignette/dither --
already existed; the missing layer was ATMOSPHERE + MATERIALS in the forward pass):
- GOD RAYS (analytic sun in-scatter): looking toward the sun, the FOG ITSELF catches
  light -- a broad halo plus a tight core along the sun line, strongest at DAWN/DUSK when
  the sun sits low (long shafts), scaled by the planetary sun's live strength + colour.
  Ties directly into the day cycle: sunrise on the wastes should now throw visible light
  through the haze, and the sun-drain finale dims the shafts with the star.
- HEIGHT FOG (ground mist, balanced+): fog thickens exponentially toward low world
  height, breathing slowly -- mist POOLS in valleys and dune hollows, thins on ridges.
  Uses the world position already reconstructed for the procedural ground.
- ENVIRONMENT REFLECTIONS (fake IBL, balanced+): the reflection vector samples a
  sky/ground hemisphere weighted by FRESNEL -- armour catches cool sky at grazing angles
  and a warm bounce underneath, scaled by daylight (night stays matte-dark). Plus a SUN
  GLINT: a tight specular highlight from the planetary sun itself riding every lit
  surface -- the key reflection that sells metal.
- PARTICLES: MUZZLE SMOKE -- a hot barrel (heat > 0.55) sheds soft grey wisps between
  bursts, pairing with the exhaust-vane glow; ICE-CRACK BURSTS -- the ambient ice-crack
  audio on frozen worlds now has a matching frost burst somewhere nearby ON the terrain
  (audio + visual agree about the world settling).
All effects quality-gated (mobile skips the new taps); sun state hoisted once to FS main
scope (same discipline as the crash fix). 50/50 tests pass; FS braces/parens balance.
HONEST: this is pure GPU-look work -- shaft intensity (24-pow core), mist density 0.030,
fresnel env 0.30-0.65, glint 64-pow are the dials; screenshots at dawn/dusk will tell all.

## Update 148 -- DUST + WATER PHYSICS: wind, devils, step/skid dust, the methane seas
Per direction, the world's loose matter now behaves:
- WIND: every map has a live airflow -- direction wandering, strength GUSTING on slow
  noise -- and the ambient dust motes now RIDE it (direction + gust speed) instead of
  drifting generically. The whole dust field visibly leans with the weather.
- DUST DEVILS (wastes/mars/titan/beach/carrion): two wandering VORTEX COLUMNS -- stacked
  spinning dust puffs widening with height -- that walk the wind line, wrap across the
  arena, shed ground dust, and SHOVE: caught in the funnel you're flung outward + upward
  with a swirl (26 u/s core push). Real hazard, real spectacle.
- STEP DUST: on the dusty worlds every footfall KICKS a puff at the plant (synced to the
  same gait beat as the footstep audio -- see it and hear it land together).
- SKID DUST: hard lateral drift in a ground vehicle (lat speed > 7) throws SIDE plumes
  off the sliding edge -- drifting now smokes.
- WATER PHYSICS (Titan's Kraken + Ligeia methane seas, generic region system for more):
  * ENTRY SPLASH -- spray burst + an expanding surface ring + a speed-scaled crash sound;
  * DRAG -- liquid resistance pulls ground speed down hard while you're in;
  * BUOYANCY -- sink below the surface and the sea pushes you gently back up;
  * WAKE -- moving fast leaves a trail of expanding ripple rings + spray behind you;
  * enemies WADE -- the seas drag them too (kite fights into the shallows).
50/50 tests pass. HONEST: devil READ (5 stacked puffs as a funnel), wind lean, splash/
wake feel, drag strength (1.8/s) are GPU/feel-unverified -- devil push 26, drag 1.8,
buoyancy 22, wake 0.22s are the dials. Water is Titan-only for now; the region table
takes one line per new sea.

## Update 149 -- MODELED MENU FONT: 14-segment mechanical glyphs that transform + animate
Menu visual upgrade per direction: every menu now carries a 3D MODELED HEADER built from
14-SEGMENT mechanical glyphs (bars + diagonals -- a cybertronian display face rendered as
real emissive geometry floating in the menu scene, over the cruising ship).
- TRANSFORM ASSEMBLY: on every menu-state change the word REBUILDS -- each segment flies
  in from a deterministic scatter (burning hot in flight, tumbling on two axes), snapping
  home on a smoothstep with per-letter + per-segment stagger. The word literally
  transforms into place, left to right, in ~1s.
- LIVING TYPE: assembled letters bob individually, pulse with an energon glow offset per
  letter, and every ~6.5s a SPIN WAVE ripples down the word -- each glyph does a full
  360 Y-turn in sequence.
- PER-MENU WORDS: CYBERTRON (title) / MODES / WORLDS / CAMPAIGN / BUILDER / CUSTOMIZE /
  SETTINGS / CONTROLS / CODEX / RESULTS. Font covers A-Z minus Q/J (all header letters +
  spares); 10-glyph x 14-segment model pool (140 boxes), menu-only, hidden on deploy and
  re-assembled fresh on every menu return.
50/50 tests pass. HONEST: glyph legibility at the menu camera distance, the assembly
timing (0.55s + stagger), and the scale/placement (y 5.6, z 4.0, letter pitch 1.35) are
GPU-unverified -- if the word sits wrong in frame those three numbers move it.

## Update 150 -- MODEL + TEXTURE DETAIL: 4 new rig mechanisms + 5 older-map surfaces
Per direction, more graphics + detail in the models and textures:
- CHARACTER RIG 78 -> 86 skel models, four new mechanisms (all LOD-gated with the bands):
  * ANKLE PISTONS (78-79): a thin actuator rod up the back of each shin that COMPRESSES
    with the landing squash (shorter + dropped) -- hard landings now read hydraulically.
  * BACK BOOST-STACKS (80-81): twin exhaust stacks on the upper back that HEAT-GLOW with
    thrust (boost/jet/engine gain drives an armour->ember colour blend), cooling at rest.
  * FOREARM INSET PANELS (82-83): dark machined recesses on the outer forearms -- another
    depth layer against the segmented bands.
  * HEAD ANTENNAE (84-85): two thin energon sensor rods, swept back, swaying faintly
    with speed.
- FIVE DEDICATED SURFACES for the older maps still sharing textures (all with normal
  maps; emit where the pattern glows): wastes_scrap (rust-orange scoured plating, sand in
  the seams), mining_strata (banded ore sediment), reclaim_growth (teal-patina circuit
  REGROWTH, glowing -- the reclamation is literally alive underfoot), trench_abyss (dark
  pressure-hull plating, pale biolum accents), shield_wall (grey-blue hex armour mesh for
  the shield bastion walls). Wastes/mining/reclaim/trench/shield_edge rewired.
50/50 tests pass; all 5 maps validate; 11 new texture files pack; map mean holds 75.6.
HONEST: piston compression read, stack glow blend, antenna sway, and all five surface
looks are GPU-unverified; every anchor offset + colour is a dial.

## Update 150 -- MULTI-BODY GRAVITY + ASSIST LANES + the codex CLONE-HYBRID easter egg
Two systems per direction:
- MULTI-BODY GRAVITY (kuiper + outer_rim): gravity is now tied to OBJECTS, not the
  ground. 4-5 visible floating rock bodies per belt map (real rock meshes, radius 7-11,
  hanging at y 18-36), each an inverse-square gravity well (mass-scaled, pull clamped
  at 60 u/s^2 -- "sort of realistic"): drift near one and it takes you. LAND on a body:
  the shell clamps your feet, kills inward radial speed, gives friction footing -- you
  can stand on, walk around, and jump off small worlds. The old deck below keeps a weak
  residual pull that fades with height, so ground play still works underneath the belt.
- ASSIST LANES (the overlay for non-flyers): each body links to its nearest neighbour
  with a VISIBLE dotted green transit lane (pulse travelling along the dots). Grounded
  bots caught adrift between rocks get guided acceleration easing them along to the
  nearest shell (9 u/s^2), and bots near a shell STICK to it (footing at rad+1.6) --
  they fight you ON the bodies. Hijacks (true flyers) are exempt; stranded humans get
  eased gently down to the deck (they can't fly either).
- CLONE-HYBRID EASTER EGG (codex): read the archive to the LAST entry and the final
  page grows a switch: "[HYBRID PROTOCOL < dormant > -- enter]". Toggle it and the
  player's build becomes a CLONE HYBRID: the energon regrafts BIO-GREEN and BREATHES
  like tissue (jc feeds eyes/seams/joints downstream -- the whole rig inherits it), and
  armour accents drift toward grown olive-flesh alloy. Machine outside, clone inside --
  the Watch bearer's secret, for archivists who read to the end.
50/50 tests pass. HONEST: well strengths (3200 mass constant, 60 clamp), shell footing
feel, lane legibility, and the hybrid's green are all play/GPU-unverified dials. The
gravity replaces the uniform pull ONLY when bodies are live; every other map untouched.

## Update 151 -- FROM THE SCRIPTS: BLACKSTAR'S FLEET (new mode) + THE SYMBOL WAR
Mined straight from the trilogy screenplays -- the Kuiper warlord's raid doctrine and the
insignia war, both made playable/visible:
- NEW MODE "blackstar's fleet" (EX_MODE_FLEET, 12th skirmish mode): survive THREE
  BOARDING WAVES of his fleet, each role from the script's fleet page:
  * CORSAIRS -- boarders with magnetic HARPOONS: a corsair in range tethers you (glowing
    line, reeled toward it at 30 u/s^2) until you KILL IT or BOOST to cut the line;
  * DRIFTCUTTERS -- fast interceptors that dart in and RAID your energon reserve (60 a
    cut) then burn away at escape speed;
  * HULLBREAKERS -- siege bots that SLAM the deck open: shockwave + camera hit, and the
    MINED ENERGON physically spills out as pickups (loot the crack they made);
  * THE DREADNOUGHT -- Blackstar himself drifting a slow predator orbit at altitude,
    dark hull + two violet solar sails, firing PLASMA HARPOONS: a burning warning line
    marks the strike point for 1.2s, then the hammer (explosion + shockwave).
  Victory after wave 3: "BLACKSTAR WITHDRAWS -- 'a costly salvage.'" (+800 energon).
- THE SYMBOL WAR (from the insignia design pages): both faction marks rendered as living
  emblems over the battle -- the NULL MERIDIAN (broken rotating ring with its gap, four
  inward lines, a breathing central VOID, black-indigo-violet) hangs beneath the
  dreadnought; the HELION MARK (gold orbital ring, deep-red rising spark, two white
  heritage wings) burns over the arena heart. The ideology fight, visible in the sky.
50/50 tests pass. (Two ordering/name collisions caught mid-wire: fleet helpers relocated
below g_enc; fleet slam cd renamed off the melee slam.) HONEST: harpoon feel (reel 30,
break-by-boost), steal pacing, dreadnought read, emblem legibility all play-unverified.

## Update 151 -- SOLAR FLARES + OUTER RIM CAPES
Player-suggested pair (neither existed yet -- the sun-drain had a lighting-only flare,
and the game had exactly one cape):
- NATURAL SOLAR FLARES: every 45-90s while the sun is up, the star ERUPTS -- a 2-3s
  surge in sun strength with the white-gold shift (reusing the absorption flare
  machinery, so the whole lighting stack responds): god-ray shafts visibly flare, the
  fake-IBL reflections + sun glint brighten on every surface, key light lifts, and the
  ambient rumble bed GROWLS underneath (+0.10 gain during the surge). The sky now has
  weather.
- OUTER RIM CAPES: on the belt maps (outer_rim, kuiper, carrion) the rim clans cloak
  their biggest -- apex units + the champion first, then heavy brutes (up to 4) wear
  tattered salvage capes hung off the back: rust-brown cloth with a per-wearer accent
  stripe, trailing lift when they run, fluttering on body phase, and SNAPPING in the
  live wind gusts. The player's own cape goes full DRIFTER on the rim: longer (1.18x)
  and wind-snapped. Capes drop hidden when the wearer dies.
50/50 tests pass. (One self-caught slip: a python str.replace hit BOTH occurrences of
an anchor and spliced the cape-owner block into a global declaration -- repaired via
targeted str_replace before build.) HONEST: flare cadence 45-90s + surge 2-3s, cape
sizes/tints/flutter amplitudes, and clan-stripe hues are GPU-unverified dials.

## Update 152 -- MAPS: ENCELADUS (25th world, geyser launches) + reclaim/foundry rebuilt
Map pass per direction -- one NEW world + the two weakest rebuilt:
- ENCELADUS, the geyser moon (25th map, debuts 81.6 A): blinding-bright ice over a buried
  ocean, its own texture (enceladus_ice + emit + norm: white shell, glowing teal
  tiger-stripe fissures). Geometry: four TIGER STRIPES (glowing fissure lines flanked by
  ice ridges), three GEYSER MOUNDS with vent-throat glows, shard-cluster cover, twin ice
  arches, a survey outpost with relay mast + perimeter ring, and a drivable survey skyway.
- GEYSER LAUNCHES (the moon's mechanic): the three vents run a staggered 8.5s cycle --
  6s dormant, then a 2.5s ERUPTION: the vent snaps open with a ring, a white plume column
  climbs, and anyone standing in it is TAKEN UP the plume (capped 34 u/s vertical) --
  free altitude for jets, a repositioning tool for walkers, a hazard mid-fight. Fully
  integrated: snow motes, aurora nights, ice-crack audio underfoot, 330s dim day, teal
  saturn-shadow sky, menu row "enceladus geysers".
- RECLAIM rebuilt 70.3 B -> 80.0 A (vert 0.19 -> 0.47): the reclamation GROWS UPWARD --
  six regrowth spires + an elevated tram line crossing the district (drivable) on pylons.
- FOUNDRY rebuilt 73.2 B -> 82.9 A (vert 0.24 -> 0.61): the works move into the AIR --
  two crossing crane gantries (drivable) + four pour towers crowned in molten glow.
Map mean 75.6 -> 76.6 over 25 maps; all colliders under cap; 50/50 tests pass.
HONEST: geyser plume read + launch feel (55 accel, 34 cap, 5.5u radius) and the new
world's brightness are play-unverified dials.

## Update 152 -- ENEMY VARIATION + MODEL DETAIL: individuals, eyes, spikes
Per direction, the opposition stops being clones:
- INDIVIDUALS (variation through the existing genome + build machinery): every enemy now
  carries its own PAINT SCHEME (8, hashed per slot) and FINISH (mostly metallic, some
  matte/gloss) on top of the existing per-slot proportion genome -- a squad now reads as
  a squad of individuals, not copies. Plain grunts additionally spread across THREE rigs
  by slot hash: standard wing-cannon, siege MORTAR shoulders, and AEGIS guardian arches
  (30%%/20%% splits) -- squad silhouette texture at a glance.
- ENEMY EYES (detail + readability): the nearest 8 living enemies (<70u, LOD-budgeted)
  get a pair of emissive hunt-light dots at head height riding their facing --
  kind-coloured: corrupted CRIMSON, hijack VIOLET, apex WHITE-GOLD, grunt AMBER -- and
  +50%% brighter in the dark (they pair with the player's own night hunt-lights). You can
  now read WHAT is looking at you by eye colour alone.
- SHOULDER SPIKES: the cape wearers (apex/champion/brutes on the rim maps) also mount
  angled pauldron horns -- the biggest threats get the biggest silhouettes.
50/50 tests pass. (Two self-caught slips this round: a nonexistent EX_SIG_FINS enum ->
EX_SIG_AEGIS, and an eye-budget reset that landed on a false-positive anchor -- re-placed
at the true pose-loop head with hide->reset->claim->pose ordering verified.) HONEST:
paint spread, eye size 0.085*ss + colours, spike angles are GPU-unverified dials.

## Update 153 -- FIVE PLAYER REPORTS: whine killed, terrain mesh restored, enemies grounded,
## SANTA MONICA, kibble curated
- THE WHINE (report 1): the ambient.ogg loop is RETIRED entirely -- even at 0.16 its
  ringing character read as a constant whine. The per-world rumble/wind beds + SFX carry
  atmosphere now. amb_wind also resynthesized without its 875-1750Hz band (whine risk).
- TERRAIN MESH RESTORED (report 2, CRITICAL): "textures flat but I walk in the air" --
  the U146 smooth-normal rewrite appended terrain verts/tris but DROPPED the material
  BUCKET registration; the IQM writer exports meshes from buckets, so the entire terrain
  mesh silently vanished (wastes: 1,708 verts -- boxes only). Collision (.hgt) still had
  hills -> air-walking over a flat floor. Fixed: the terrain quads register their bucket;
  wastes now 16,108 verts, y-range 15.2 -- hills are BACK, now smooth-shaded as intended.
- ENEMIES GROUNDED (report 3): the earlier terrain-respect lerp was overridden by later
  y-writers (flee/run pulls toward 0). Those pulls now target the TERRAIN height, and a
  FINAL-WORD snap in the pose loop stands every grounded bot on the heightfield
  (knockback air + hijack flyers keep their own y; nothing sinks).
- SANTA MONICA, UNMISTAKABLY (report 4, mission accuracy): the wave chapter is scripted
  in LA -- the beach map now SAYS so: a stepped downtown SKYLINE wall with lit rooflines
  (one tower collapsed to rubble), the PIER as a drivable deck on the old pilings, the
  FERRIS WHEEL (A-frame + hub + 10 lit cars in a vertical ring -- the Santa Monica
  silhouette), palm lines with frond crowns along the boardwalk, and the snapped PCH
  with its center divider. Beach 75.0 B -> 81.6 A, 90/120 colliders.
- KIBBLE CURATED (report 5): alt-mode remnants were random %21 -- incoherent vehicle
  junk on robot bodies ("reads as incorrect, not flair"). Each kind now draws from a
  small curated set that fits its fiction: grunts wear ground-vehicle kibble (tank/car/
  cycle), corrupted wear beast frames (dino/spider), hijacks wear flight surfaces
  (jet/dragon), apex heavy iron (mech/tank).
50/50 tests pass; all maps validate. The terrain regression fix is verified in the IQM
bytes; the whine's absence + LA read need your ears/eyes.

## Update 154 -- space radio, transform floor fix, the TOWER FALLS, de-blockify pass
Four items from playtest feedback:
- SPACE RADIO: the retired ambient.ogg returns the way the player suggested -- as an
  occasional TRANSMISSION: every 60-140s it fades in slowly (0.8/s) to 0.12, holds
  ~5-13s, fades back out. Distant radio drifting in and out of range over the void,
  never a constant bed.
- TRANSFORM FALL-THROUGH FIXED: the terrain suspension/floor block excluded the
  TRANSFORMING state -- mid-fold the player had no heightfield floor and dropped
  through to the flat base. The floor now holds through the transform.
- THE TOWER FALLS (mission I.2 opening): a 9s in-engine cinematic replaces the combat
  drop: the camera holds on a standing Ocean Avenue tower (3 stacked segments, live
  models); at 3.0s it LETS GO -- shears sideways, keels, accelerating fail, shockring +
  debris bursts + heavy impact audio + 0.55 trauma ("the tower on ocean avenue lets
  go.") -- then the camera FALLS with the dust in one long pan down to the sand ("and
  below all of it: a kid with a skateboard"), landing behind the player: "AIDEN. you."
  The human-scale reveal is now a shot, not a menu fact. The fallen segments vanish
  into the map's static rubble at the same spot (continuity).
- DE-BLOCKIFY (map design pass, per "cheap arcade" note): two composite Builder methods
  replace naked primitives -- city_tower (tapered setback tiers, lit window bands,
  roof cap, antenna + beacon; tiers 0-1 collide) and pylon (base collar, trunk, twin
  crossarms with lit insulators, beacon). Retrofit: LA skyline (7 towers), core descent
  shafts (6, molten-lit), pluto relay pylons + all 9 sensor masts. Pluto 74.4 B ->
  85.1 A (real vert + cover from dressed masts); mean 77.0.
50/50 tests pass; all maps valid + under cap. HONEST: radio fade feel, the cinematic's
pacing/framing (a 260u orbit + 4.6s pan -- numbers movable), and whether the composite
structures kill the arcade read are play-unverified.

## Update 155 -- de-blockify sweep II (wastes/mining/foundry) + champion WAR-PAINT
Continuing the "not a cheap arcade game" pass:
- TWO MORE COMPOSITE METHODS: stack (refinery chimney -- base drum, tapered trunk, lit
  vent ring, top lip) and derrick (splayed collidable legs, cap platform, wheel-house
  glow, cable mast -- machinery, not a spike).
- RETROFITS: WASTES' fourteen naked 0.5-girth perimeter poles -> dressed dead-relay
  pylons with hazard-lit insulators (85.1 A, was ~78); MINING's bare rust spires -> real
  derricks over the pits; FOUNDRY's bare wall slabs -> caution-capped barriers with end
  posts, plus three molten-vented SMOKING STACKS on the north line (83.9 A).
- CHAMPION WAR-PAINT: the promoted warlord now wears its master's colours in GLOSS --
  Cryotek cobalt, Blackstar gunmetal, Jhiaxus emerald -- and apex units arrive in pearl.
  With the cape + spikes + white-gold eyes, the biggest threat on the field reads in
  half a second from any distance.
50/50 tests pass; all maps valid + under cap; mean 76.9. Remaining naked-primitive
sweeps if wanted: emberpit obsidian tiers, shield_edge, reclaim scaffolds.

## Update 156 -- methane rain + wet sheen, ORBITING belt, hybrid appendix, de-blockify III
- TITAN METHANE RAIN: showers schedule on/off (18-32s showers, 30-80s gaps); 40 thin
  amber-grey streaks fall slant-wise ON THE LIVE WIND around the player, ticking ground
  splashes; and the whole world WETS -- a new u_wetness uniform (plumbed like the sun)
  boosts the fake-IBL env reflections (+90%) and the sun glint (+~85%) while raining or
  swimming, soaking in fast and drying out slow. Rain on the dunes at dusk under god
  rays should be the map's best moment.
- THE BELT ORBITS: the gravity bodies now DRIFT on slow individual circles (70-158s
  periods, gentle vertical bob) around their anchors -- rocks visibly wander, the player
  gravity/landing shell, the bot assist, and the dotted transit lanes all read the LIVE
  positions every frame, so the whole system moves as one. Stand on a rock and it
  carries you.
- HYBRID APPENDIX (codex): a final lore entry -- "appendix: hybrid protocol" -- an
  unsigned fragment threaded through the watch's code, sitting on the same last page as
  the toggle. The reward now explains itself.
- DE-BLOCKIFY III: emberpit's dais spire -> a broken monument (collar + shaft + ember
  ring) with ember-lit rims on every obsidian tier; shield_edge's bare energon pickets ->
  emitter columns with base collars + cap rails (87.2 A, second-highest map). Mean 77.3.
50/50 tests pass; caps ok. HONEST: rain streak read (thin boxes at 55 u/s), wet-sheen
strength, orbit speeds vs landing feel (the shell moves ~0.5 u/s under your feet), and
the appendix's tone are unverified. Every number is a dial.

## Update 156 -- SWIM STROKES (+ a duplication audit)
The genuinely new feature this pass:
- SWIM STROKES: shoulder-deep in Titan's seas on foot, the walk articulation hands over
  to a STROKE -- alternating overarm sweeps (0.85 rad, opposed phases), a faster flutter
  kick underneath (0.30 rad), and the body LIES FORWARD into the water (+0.24 lean,
  capped) -- the titan swims instead of moonwalking through methane.
DUPLICATION AUDIT (important honesty note): while adding "titan rain + wet sheen +
orbiting gravity bodies" this pass, I discovered ALL THREE already exist, fully wired,
from an earlier stretch of this session that had been compacted out of my working
summary (ex_wetness/u_wetness renderer+shader plumbing, a complete NUM_RAIN weather
system with wind-slanted streaks, and g_gbody_pos live-orbit updates feeding wells,
lanes, shells, and models). My duplicates (ex_wet uniform, RAIN_N system, g_gb_pos
orbit updater -- the last double-writing body models against the original) were fully
identified and STRIPPED; the pre-existing implementations stand untouched. Lesson
applied: before implementing any "new" system, grep for cousins first -- the transcript
holds more than the summary does.
50/50 tests pass; the shipped delta is exactly: swim strokes + swim lean, nothing else.

## Update 157 -- de-blockify III (emberpit/shield_edge/reclaim) + the SEALED appendix
Verified-first pass (per the U156 lesson: grepped before building -- and found gait
personality + the hybrid lore entry ALREADY exist; only these were genuinely missing):
- SEALED RECORD: the codex's final entry ("appendix: hybrid protocol") now displays as
  "[ SEALED RECORD ] -- the archive keeps its last page for those who read every other"
  in both the menu row and the reader pane until the unlock condition is met. The
  easter egg now WITHHOLDS properly instead of spoiling its own title.
- DE-BLOCKIFY III:
  * EMBERPIT: four obsidian VENT STACKS breathing molten light on the outer rim (78.6).
  * SHIELD_EDGE: the nine floating combat platforms get lit energon deck rims + support
    stalks beneath (they FLOAT for a reason now), plus three perimeter relay pylons
    watching the line -- 87.5 A, the highest-scored map in the game.
  * RECLAIM: LANE GATES -- each of the three corridors passes under an arch at both
    ends; the lanes read as ROUTES with thresholds, not paint strips -- 80.7 A.
- Map mean 77.5 over 25; all valid + under cap. 50/50 tests pass.

## Update 158 -- LENS FLARE (post pass) + the word wears its outcome
Two verified-missing features (grepped first; both genuinely absent):
- LENS FLARE: the renderer now projects the planetary sun through view+projection each
  frame (screen uv + a visibility factor that fades at screen edges, dies below the
  horizon, and scales with LIVE sun strength -- so solar flares flare the lens too, and
  the sun-drain finale kills it with the star). The post pass adds: a warm double halo
  at the sun, a thin ANAMORPHIC horizontal streak, and three cool ghost dots marching
  toward screen centre. Quality-gated (balanced+); zero cost when the sun is offscreen.
- THE WORD WEARS ITS OUTCOME (modeled menu font): the results header is no longer
  "RESULTS" -- victory assembles "VICTORY" in triumphal GOLD; defeat assembles "FALLEN"
  arriving DENTED: ~30%% of segments torn away, the rest guttering ember-red with
  hash-timed flicker drops and bent rotations. The pause screen also gets its own
  "PAUSED" header.
50/50 tests pass; FS braces/parens balance in both edited shaders. HONEST: flare halo
sizes (38/5 falloffs), ghost spacing 0.7, streak tightness, and whether FALLEN's dents
read as damage or noise are GPU-unverified dials.

## Update 159 -- THE SUN ITSELF + chapter titles assemble in-world
Two coherence features (both verified absent before building):
- THE SUN DISC: the sky finally HAS a sun -- an emissive disc parked 950u along the live
  sun direction from the camera, colour riding the day cycle (amber horizons, dying
  ember in the drain), size breathing with strength (natural flares visibly SWELL it),
  setting below the horizon, absent indoors. The god rays, sun glints, height-fog light,
  and the new lens flare all now point at a real object instead of empty sky -- the
  whole lighting stack agrees about where the star is.
- CHAPTER TITLES ASSEMBLE IN-WORLD: at campaign match start the mission's key word
  (first word after the "I.3 _" tag, uppercased; Q/J font gaps mapped to O) builds
  itself from flying 14-segment glyphs 13u over the spawn, FACING THE CAMERA, holds,
  then SCATTERS BACK OUT -- the same transform run in reverse (~6s total, letter+segment
  staggered both directions). "OUTRUN" hangs over the doomed beach; "DAWN" over the
  truce. Reuses the menu glyph pool (menu is gone in-match; no new models).
50/50 tests pass. HONEST: disc size 26 x strength, title height 13u / scale 2x /
6s timing, and the scatter-out read are GPU-unverified dials.

## Update 159 -- flare OCCLUSION + the bridge glitch (+ a scene-capacity audit)
- SCENE-CAPACITY AUDIT (stability, verified clean): with the model pools grown large
  (140 font segments, eyes, capes, rain, lanes, devils, bodies...), checked whether the
  scene has a fixed model cap that silent adds could overflow -- it does not: renderables
  are an unbounded linked list (ex_rendernode_push). No risk; no change needed.
- FLARE OCCLUSION (no depth buffer needed): the lens flare now samples the RENDERED
  FRAME in a 5-tap cross at the sun's screen uv -- dark pixels there mean geometry is in
  the way, and the flare dims through a smoothstep. Walk behind a tower and the flare
  dies; step out and it blooms back. The visible sun spot is reliably the brightest
  thing on screen (god-ray core), so frame luma is an honest proxy.
- THE BRIDGE GLITCH (modeled font): browsing the WORLDS menu now makes the header
  STUTTER on every selection change -- 0.28s of per-segment positional jitter + a
  chromatic red/blue split cycling across segments -- the space bridge hunting for a
  lock on the next world. Purely cosmetic, resets cleanly, never fires on first entry.
50/50 tests pass; post FS validates. Dials: occlusion thresholds (0.10/0.42), glitch
duration 0.28 + jitter amplitude.

## Update 160 -- CLING (sideways/inverted climbing w/ grip safety) + CRAWLER AMBUSHES
The requested climbing system, built on the existing wall-climb + the map collider AABBs:
- CLING (player): JUMP is GRIP. Airborne near any collider face, hold jump WITHOUT
  driving forward (forward+jump stays the legacy climb-up/mantle) to LATCH: sideways on
  wall faces, fully INVERTED under box undersides. Move keys traverse the face plane at
  6.5 u/s; walls clamp inside the face bounds; cresting a wall's top edge auto-MANTLES
  you over. Release = drop with a push-off.
- GRIP SAFETY (position x type): a grip meter drains while latched -- vertical faces
  0.045/s, UNDERSIDES 0.20/s (brutal); scaled by build BULK (heavy frames struggle,
  0.55+bulk*0.55), quartered for crawler ALT-MODES (spider/insect/scorpion/crab: born to
  cling, x0.40), doubled-ish on slick ice ((2-surface_grip)). Empty grip = "GRIP FAILED"
  + a 1.2s regrab lockout + the fall. HUD shows "grip N% [INV]" while latched (red under
  30%%). Refills on the ground.
- CRAWLER AMBUSH AI (bounding-box work): ~40%% of corrupted are CRAWLERS. Mid-range
  (22-65u), off cooldown, max 2 live: they pick a collider rising 6u+ overhead within
  14u, SCUTTLE up its near face over 2.4s (accelerating, position-glued -- "something is
  CRAWLING the structure--"), PERCH coiled on the crest (staggered 0.6-1.35s), then
  POUNCE: a solved ballistic dive at you ("IT POUNCES", tumbling) -- landing handled by
  the existing ragdoll system. 12s per-enemy cooldown. The terrain final-word snap is
  gated off during climbs so nothing yanks a crawler off its spire. The frostline
  arches + ice spikes are exactly the ambush architecture this was asked for.
50/50 tests pass. HONEST: cling traverse feel (6.5 u/s), the drain rates, whether the
scuttle READS as crawling (position-glue, no full body reorientation -- pose limits),
and pounce ballistics are play-unverified. All dials.

## Update 161 -- AAA GRAPHICS PASS: contact shadows, back-light, per-world grade, grain + CA
Four verified-missing cinematography layers (ssao/gbuffer confirmed orphaned files;
point-shadow cubemaps exist; none of the below did):
- CONTACT SHADOWS: a soft dark blob glued to the ground under the player + the 10
  nearest bots, shrinking + fading with height (gone past 9u) -- the cheap half of
  ambient occlusion, and the single biggest "characters exist in the world" grounding
  win. Terrain-aware; sized by build bulk / enemy scale.
- BACK-LIGHT RIM (forward): the film kicker -- a cool edge light from OPPOSITE the sun
  (pow 2.5 fresnel x facing-away) lifts silhouettes off dark backgrounds. Key (sun) +
  fill (hemisphere) + back-light: the three-point rig is complete, and it rides the
  live sun so it dims at night and dies in the drain.
- PER-WORLD SPLIT-TONE GRADE (post): shadows lean into the world's FILL colour,
  highlights into its KEY colour (derived automatically from each theme's own lighting,
  normalized + softened 0.6+0.4) at 30%% mix -- every map now carries a coherent
  colour-grade: teal-orange on the ember worlds, blue-gold ice, green-veined helix.
- FILM GRAIN + CHROMATIC ABERRATION (post): animated luma-weighted grain (stronger in
  shadow, 0.028, balanced+) and radial channel-split at the frame edges (0.030 * r^2,
  high+ only, centre stays sharp) -- the anamorphic-lens finish.
50/50 tests pass; both shaders validate. HONEST: blob darkness 0.055 (too dark reads as
holes), rim 0.55 strength, grade 30%% mix, grain 0.028, CA 0.030 -- five numbers, all
screaming for a screenshot.

## Update 162 -- SUN SHADOW MAP: real cast shadows from the planetary star
The biggest single AAA jump available in this engine, built on the existing point-shadow
infrastructure (whose pass loop + depth-shader pattern it mirrors):
- THE PASS: a 2048x2048 orthographic depth cascade rendered every frame along the LIVE
  sun direction, centred 45u ahead of the camera (300u ortho box, front-face culling to
  fight acne, clamp-to-border depth 1.0 so off-map reads lit). New sunshadow.glsl depth
  shader (skeleton-aware VS, empty FS). Camera position/forward extracted from the
  inverse-view matrix (the renderer's camera struct is matrices-only -- the lens-flare
  block was quietly relying on a nonexistent field and got fixed in the same stroke).
- THE SAMPLE (forward FS, high+ quality): each fragment projects into the sun's map;
  3x3 PCF with slope-scaled bias; the result multiplies EVERY sun contribution --
  direct diffuse and the sun glint -- floored at 0.28 so shadows stay readable, never
  pure black (the hemisphere + fill still live there).
- WHAT IT MEANS: buildings cast long dawn shadows that SWING through the day cycle;
  the LA skyline throws evening shade across the sand; bots shadow each other; you can
  stand in a tower's shade at noon on Mars; and during the drain finale the shadows die
  WITH the light. Every prior lighting feature (day cycle, god rays, disc, flares)
  gains physical grounding. Quality-gated high+; pass skipped entirely at night.
50/50 tests pass; all shaders validate. HONEST: this is the highest-risk GPU feature
yet -- acne/peter-panning (bias 0.0022/0.0007 + front-culling are the knobs), cascade
coverage (150u half-extent), and the 0.28 floor all need real eyes. If shadows look
wrong, quality 'balanced' turns them off while we tune.

## Update 162 -- AAA GRAPHICS II: cloud shadows + far-image stability
Verified-first pass, and the audit mattered again: the DIRECTIONAL SUN SHADOW MAP is
already fully implemented (depth pass, 3x3 PCF, 0.28 floor, quality-gated) from the
compacted-out session -- so shadows needed nothing. Genuinely missing + now built:
- CLOUD SHADOWS: broad soft light-patches DRIFT across the outdoor worlds -- the sun
  through a moving cloud deck (world-space fbm, slow diagonal drift, never darker than
  0.55 so it reads as weather). Per-world decks: Titan 1.0 (the haze world IS cloud),
  earth-line skies 0.85, Mars dust 0.55, helix/carrion 0.6, ice worlds 0.35, io/ember
  0.25; interiors AND the vacuum maps (kuiper/outer_rim/shield_edge) stay hard-lit at 0
  -- airless worlds correctly have knife-edge light. Smoothed 0.5/s on map change.
- DISTANCE NORMAL FADE: beyond ~45u surfaces relax toward the smooth vertex normal
  (85%% by ~175u) so normal-map detail cannot SHIMMER at distance -- near detail stays
  crisp, the far image goes clean and stable. The quiet mark of a AAA frame.
50/50 tests pass; FS validates. HONEST: cloud scale 0.0045 / drift speed / 0.55 floor
and the fade band 45-175 are the dials; a wastes panorama screenshot judges both.

## Update 163 -- SSAO: screen-space ambient occlusion (depth-only)
The second pillar of the AAA push, unlocked by one structural change:
- DEPTH AS A TEXTURE: the canvas framebuffer's depth attachment converts from a
  renderbuffer to a DEPTH24_STENCIL8 texture (same attachment point, resize-aware) and
  binds into the post pass on unit 1 -- the post can finally SEE the scene's shape.
  This also opens the door to any future depth-aware effect (soft particles, DOF,
  depth fog) for free.
- SSAO (high+): depth-only ambient occlusion in the post -- a 10-tap golden-angle
  spiral around each pixel; neighbours meaningfully NEARER the camera occlude it, with
  a smoothstep window (0.10-0.55 in, fading 2.2-4.5 out) so distant silhouettes don't
  halo, and a radius that shrinks with distance (620/z px, clamped 3-42) so it stays a
  crease-and-contact effect, not a smog. Skips the sky (z > 900). Applied before
  exposure at 0.55 max darkening: panel seams, armour joints, building corners, and
  the ground under every bot gain natural weight -- the "everything is slightly lifted
  render" look dies here. Linearization matches camera.c's 0.05/4000 planes exactly.
- Stacks multiplicatively with the contact blobs (analytic wide + SSAO tight = layered
  grounding, the standard AAA recipe).
50/50 tests pass; renderer + shader validate. HONEST: SSAO is artifact-prone territory
(halos, over-darkening, shimmer under motion) -- the 0.55 strength, the range window,
and the radius curve are THE dials, and 'balanced' quality bypasses it entirely.

## Update 163 -- AAA GRAPHICS III: cinematic far-field DOF + eye adaptation
Fourth audit first (and again it saved a mountain of duplicate work: SSAO is ALREADY
fully implemented -- golden-spiral depth-only AO, range-checked, radius-scaled,
pre-exposure -- plus the depth-texture conversion it rides on). Genuinely absent + built:
- CINEMATIC FAR-FIELD DOF (high+): beyond ~120u the frame SOFTENS gently, full by
  ~500u -- a long-lens atmosphere blur. The whole gameplay range stays tack-sharp;
  only the horizon breathes like a photographed one. Depth-WEIGHTED taps (only
  far-field samples contribute) prevent sharp foreground silhouettes bleeding into
  the blurred distance -- the artifact that betrays cheap DOF.
- EYE ADAPTATION: exposure opens as the light dies (up to +55%% at full night --
  night stays dark but READABLE) and it LAGS at 0.4/s: step out of a vault into
  noon and the frame overbrightens for a beat before the pupils close; watch a
  sunset and the eye slowly gives you the dark. Interiors settle to neutral. The
  film-camera behaviour AAA lighting is built around, driven by the live sun.
50/50 tests pass; FS validates; unset-uniform guard keeps every other render path
identical. Post chain now: CA -> DOF -> SSAO -> bloom -> ADAPTIVE exposure -> ACES ->
split-tone -> flare -> vignette -> grain -> dither. HONEST: the 120-500u DOF band,
blur radius 3.2px, +0.55 night exposure, 0.4/s adaptation lag -- four dials, one dusk
screenshot.

## Update 164 -- CRITICAL CRASH FIX: uniform-cache overflow (segfault in ex_uniform)
Player-reported with a perfect gdb trace: SIGSEGV inside ex_uniform() from
ex_render_forward(), first frame after "forward.glsl successfully compiled".
ROOT CAUSE: the engine's uniform-location cache is a FIXED 256x256 table with NO bounds
checks -- neither on the shader handle row nor, critically, on the entry count: the
257th distinct uniform name for a shader wrote out of bounds. Two pressures filled it:
(1) per-index point-light names ("u_point_lights[N].xxx") mint NEW permanent entries
for every light index ever used, and (2) the recent graphics passes added ~15 uniforms
(sun trio, wetness, clouds, grade pair, sun uv/vis, time, exposure...). This build
crossed the 256th entry -- overflow, corruption, segfault.
THE FIX (structural): bounds guards on BOTH axes -- an out-of-range shader handle or a
full cache now falls back to a direct glGetUniformLocation (correct, merely uncached)
instead of writing out of bounds; capacity also doubled to 512 columns for headroom.
A crash is now impossible by construction regardless of how many uniforms future
graphics work adds.
50/50 tests pass. (Same lesson-family as the sunS scope crash: C compiles fine, only a
live GL run walks these paths -- the player's gdb trace was the diagnosis.)

## Update 165 -- BLACK SCREEN FIXED: shader shadowing bug (+ real GLSL validation adopted)
Player-reported: black frame, UI rendering over it ("probably a bad filter" -- exactly
right). DIAGNOSIS -- with a REAL compiler this time: installed glslangValidator and
compiled every stage of every shader the way the engine assembles them. fboshader's
fragment stage FAILED: my split-tone grade (U161) declared a local variable
`float luma = ...` inside main -- which SHADOWED the pre-existing `luma()` helper
FUNCTION for the rest of the shader, so the (compacted-session) lens-flare occlusion
block downstream -- which CALLS luma() -- no longer compiled ("can't use function
syntax on variable"). Post shader failed -> post pass drew nothing -> black frame with
UI (drawn separately) on top.
THE FIX: renamed my local to lgy (un-shadowing the helper); all four of my luma uses
follow. Every stage of every shader now compiles clean under glslang.
METHOD UPGRADE (the bigger win): headless GLSL validation via glslangValidator is now
part of the standing build gate -- the brace-counting heuristic missed BOTH this bug
and the earlier sunS scope crash; a real compiler catches the entire class. Two of the
last three player crashes would have been caught before shipping.
50/50 tests pass; ALL SHADERS COMPILE (glslang-verified).

## Update 166 -- SIX-ITEM PLAYTEST BATCH: WFC vehicles, world reverb, terrain precision,
## the checker boxes, roll-not-shear, perf trims
- WFC VEHICLE FEEL (per direction + screenshot): vehicles now HOVER cybertronian-style --
  ground modes ride a 0.55u cushion with a two-frequency bob (treads/mech low 0.18,
  beast modes planted); banking gain nearly doubled in vehicle (0.028, clamp 0.62) so
  turns LEAN hard, War-for-Cybertron style.
- EXPAND/CONTRACT FIXED (roll, not shear): the body lean was a per-part HEIGHT-SHEAR --
  fine for a tall robot pivoting at its feet, but on a wide low hull it slid parts
  sideways by their height, visibly stretching the body in turns (the reported bug).
  Vehicles now take a TRUE RIGID ROLL+PITCH about the chassis centre (rotated offsets +
  rotation[2] so the hull banks as ONE piece). Robot keeps the shear it looks good with.
- THE CHECKER BOXES (blue-marked): the skeleton pool's 74 box-based parts (pauldrons,
  guards, plates, bands...) used the raw box template -- whose DEFAULT CHECKER-GRID
  texture is exactly the "extra default texture boxes" in the screenshot at light tints.
  New smooth brushed-metal plate.iqm template generated + swapped in for every non-joint
  skel part. The grid is gone from the body.
- WORLD REVERB (physically-motivated): pre-rendered Schroeder comb-tail WET versions of
  the percussive one-shots (footstep, land, boost, icecrack, warp); enclosed worlds
  (core/vault/foundry/trench/mining/unicron -- the planetary system's interior set)
  play the wet takes: the hall answers back. Open sky stays dry.
- TERRAIN PRECISION: mesh + heightmap res 60 -> 72 (finer triangles = less curvature
  error underfoot) and .hgt heights now written at 5 decimals (was 3) -- the sampler
  and the mesh agree to ~0.0002u.
- PERF (reported 240 -> 170 fps): SSAO 10->8 taps + far cutoff 900 -> 320u (fog owns
  the distance anyway), DOF 8->6 taps, cloud fbm 4 -> 2 octaves. Roughly a third of the
  new per-pixel cost trimmed with near-identical output.
50/50 tests pass; ALL SHADERS COMPILE (glslang gate); rev_* + plate.iqm pack (6 files).
HONEST: hover height/bob, bank clamp 0.62, reverb wet 0.55, and the recovered fps are
all play-unverified dials.

## Update 167 -- craters surface + REAL dynamic character shadows (blobs retired)
Two playtest items:
- CRATERS ON THE TERRAIN: impact craters/scorch marks spawned at y~0.12 flat-world
  height -- the new heightfield buried them (exactly the player's diagnosis: "y bound
  underneath new terrain"). They now sample the terrain and sit ON the surface, dust
  plume included. Battle scars are visible again on every rolling map.
- REAL SHAPED DYNAMIC SHADOWS: the sun shadow map has been rendering all along -- but
  the CHARACTER models were created with is_shadow=0, so bodies never cast into it (the
  contact blobs were compensating). Now the entire body casts: all robot parts (player +
  every enemy via spawn_body), the 86-part skeleton pool (pauldrons, plates, fins -- the
  greebles SHAPE the silhouette), and the capes. Articulated, animated, properly-shaped
  shadows from the live sun -- stretching long at dawn, swinging with the day cycle,
  vanishing in the drain.
- CONTACT BLOBS RETIRED (player-requested: "sort of distracting"): the fake blobs are
  disabled now that the real thing exists.
50/50 tests pass; all shaders compile (glslang gate). HONEST: shadow-map resolution vs
~100 new casters (acne/peter-panning at the existing bias), and the depth-pass cost of
those casters, are GPU-unverified -- if shadows shimmer or fps dips, the bias + a
skel-pool caster cut (plates only, skip small greebles) are the dials.

## Update 168 -- unendlich REVERB DSP + upscale architecture, Aiden starts HUMAN,
## autobot courtesy, NASA terrain
Five items, two mined from the monorepo:
- REAL-TIME REVERB DSP (ported from mono/games/unendlich audio.rs r185/r186): the
  Schroeder-lite chain -- two feedback combs (34.7/48.3ms, fb 0.74/0.68) + one 5ms
  allpass, soft-knee saturation on the dry sum, brick limiter -- now runs INSIDE
  MojoAL's mix bus (reverb_bus after mix_context), processing the ENTIRE mix
  phase-coherently: footsteps, engines, weapons, radio, everything. Driven by world
  ENCLOSURE per unendlich's sound_resolve doctrine: sealed interiors wash at 0.20,
  trench canyon whispers at 0.08, open sky decays to 0 (chain fully bypassed -- zero
  cost when dry). The pre-rendered rev_ WAV path is superseded (wetpick returns dry;
  assets retained).
- NEURAL UPSCALER (honest port): unendlich's neural upscale is Vulkan compute + Q8
  weights TRAINED IN-ENGINE -- the bundle ships only .spv.hash files, no sources or
  weights, so the network itself CANNOT be ported. What shipped instead is its
  ARCHITECTURE faithfully: render-scale modes (the settings' render scale now feeds
  u_upscale = 1/scale) + a contrast-adaptive reconstruction SHARPEN in the post pass
  that recovers edge definition from the bilinear upsample -- av1tuner's own doctrine:
  "copies real pixels or smooths, does NOT hallucinate". Rendering at 0.5-0.75 scale
  now looks far closer to native = the fps lever the 170fps report wanted.
- AIDEN STARTS HUMAN (campaign bug): missions never set g_human_scale (it was only the
  debug H toggle) -- so "the beach opens" and "outrun the sea" started you as a robot
  in an orbital drop. Both Aiden chapters now force human scale, skip the deploy drop,
  and spawn ON THE SAND (I.3 opens: "the sea is pulling back. you know this drill.
  RUN."). All other missions reset to robot scale.
- AUTOBOT COURTESY (requested bonus): near unguarded civilians (<18u, active, not
  downed) the vehicle hover cushion SETTLES smoothly to wheels-down (90%% reduction,
  2/s ease) -- a titan doesn't blast dust over the people it protects. Lifts again
  when clear.
- NASA TERRAIN (imaging-derived landforms per world, composited into the heightfield):
  PLUTO Sputnik Planitia polygonal convection cells (raised rims, 26u cells); EUROPA
  parallel double-ridge LINEAE bands; MARS three rimmed crater bowls + transverse dune
  ripples; TITAN longitudinal dune seas + a meandering carved channel; IO two rimmed
  caldera pits in volcanic plains. All five regenerate valid + under collider caps.
50/50 tests pass; ALL SHADERS COMPILE; mojoal.c syntax-clean. HONEST: reverb wet levels,
sharpen strength (-0.22), civ-settle radius 18u, and every NASA landform amplitude are
play-unverified dials; the reverb DSP runs on the audio thread (float math, ~40 ops/
sample when wet -- fine on any modern CPU, but unmeasured here).

## Update 169 -- film-weight movement retune + six new body parts + carbon weave
Player direction: "up the speed of the cars and running, tune down the leg speed,
bigger ones hitting slower strides -- film-inspired, bots feel bigger."
- SPEED RETUNE: ground vehicles 24 -> 31 (cars properly QUICK now), on-foot run
  13 -> 16. Jets hold 41.
- FILM-WEIGHT GAIT: cadence now scales with MASS on top of the Froude relation --
  the player's stride slows with build bulk (up to ~38% slower at full bulk, stride
  LENGTH grows to carry the speed); enemy cadence divides by kind size (apex 1.55x
  slower strides, corrupted 1.18x, champions a further 1.22x). Faster ground speed +
  slower, longer strides = the film look: mass reads in the WALK, speed reads in the
  travel.
- SIX NEW BODY PARTS (skeleton pool 86 -> 92, high-detail tier): hip skirt armour
  plates that swing opposite the stepping leg (86-87), back radiator fins above the
  boost stacks that heat-glow with thrust (88-89), a chin-guard jaw plate squaring
  the head profile (90), and a tail spoiler off the lower back -- vehicle heritage
  showing through the robot (91). All brushed-metal, all cast shadows, all hidden
  at low detail.
- NEW TEXTURES: carbon twill WEAVE (warp/weft sheen blocks + normal map) added to the
  generator and applied to the mining-rig deck; a hazard-chevron generator pattern was
  also added for future use. (NOTE: an existing hand-authored hazard.png already ships
  -- it was accidentally overwritten during this update and restored byte-identical
  from the previous build; the new hazard GENERATOR remains available under a
  different invocation only.)
50/50 tests pass; all maps valid. HONEST: the new speeds, cadence factors, and part
proportions are play-unverified; if the run cadence now mismatches footstep audio
feel, the sync reads gait phase so it should follow -- but the THUMP RATE will be
audibly slower, which is intended (heavier), tell me if it reads as sluggish instead.

## Update 170 -- RAZOR signature, kind-true enemy guns, girder + tank props
Asset/model pass:
- 12th SIGNATURE: RAZOR FANS (Executioner) -- twin swept dorsal blade fans, three
  blades per side stepped in pitch, generated from a new gen_spine "razor" preset
  (sec_blade sections, mirrored; 344 verts). Full engine wiring: enum, mesh path,
  menu label ("razor fans"), mount transform, stat identity (harder-hitting, quicker,
  thinner armour) and on-hit identity (0.3s slow -- the blades hamstring). Selectable
  in customization; in the field, ~30%% of non-Ravager corrupted now spawn as the
  CUTTER CASTE carrying them.
- ENEMIES CARRY THEIR OWN GUNS (was: every enemy held the same weapon[0] mesh):
  grunts spread bolt/burst/flak by slot hash at spawn; on rig re-sync the mesh
  follows the kind -- corrupted carry the jagged PIERCE, hijack the RICOCHET, apex
  shoulder the PLASMA launcher. Enemy weapons + signature backs now also CAST
  SHADOWS (U167 consistency -- the silhouette includes the kit).
- TWO NEW DECO PROPS (gen_props): a bent I-BEAM GIRDER, snapped mid-span and kinked
  upward (leans 22 degrees in the field like fallen wreckage), and a ribbed STORAGE
  TANK with dome cap + valve stub. Skyline scatter widened from 4 prop kinds to 6.
50/50 tests pass. HONEST: razor blade proportions, the cutter-caste rate, enemy gun
mix rates, and prop scatter balance are all play-unverified; the razor's 0.3s slow
stacks with the tether cable's slow if both ever apply -- flag if that feels sticky.

## Update 171 -- screenplay ports: THE EYE, THE HORN, THE MATRIX OPENS + film lines
Straight from the digest's biggest unfilmed beats:
- THE COLOSSAL EYE (I.1-I.3, beach): "a colossal eye -- miles wide -- flickers beneath
  the crust." A vast red glow-disc lies offshore under the shallows (radius ~20 map
  units), breathing and FLICKERING -- and every ~7 seconds it BLINKS. The dark iris
  drifts toward the player's position: it is looking at YOU. Gone outside the
  campaign beach chapters.
- UNICRON'S HORN (III.1, trench): a colossal curved tusk (new gen_props model: seven
  stacked tapered segments sweeping through ~64 degrees, obsidian-dark) RISES through
  the survive timer -- buried at mission start, breaching the sky by the end,
  curving over the arena from the horizon. When it crosses half-risen, prime's line
  lands: "i had hoped you would never awaken."
- THE MATRIX OPENS (VI.2 victory): winning the final mission fires the film's pivot
  -- "the matrix responds. it OPENS." -- with an accelerating golden ring flooding
  outward from where you stand for six seconds and a single hard camera shake, then
  quiet. The epilogue slide now closes the campaign with prime's line: "leadership
  is not the absence of fear... but the will to rise beyond it."
- SCRIPTED FILM LINES (one-shot, timed into missions): I.1 at 26s -- ratchet:
  "that's not solar radiation... that's energon. and it isn't coming from space.";
  I.2 after the collapse cine -- aiden: "that's not a fault line..."
50/50 tests pass. HONEST: eye placement (offshore at +55 map-units z), horn position/
scale (~660u tall at full rise) and the matrix ring pacing are tuned blind to the
actual sightlines -- screenshots will tell us if the eye reads through the water and
whether the horn breaches inside the skybox.

## Update 172 -- boot cutscene, cave maps, weapon meshes, LIVING projectiles
Four player asks:
- BOOT CUTSCENE (prime's voiceover): the game now opens on a six-line optimus VO
  built from the script documents ("before the first city... there was a hunger older
  than light" / "unicron does not invade. unicron WAKES"), auto-advancing every 7s
  over a staged backdrop: EARTH close and wounded-red, THE EYE burning and blinking
  on its face, THE HORN rising black through the entire VO. Skippable with ANY key;
  the "any key _ skip" tooltip FADES IN at 2.2s instead of popping. Falls through to
  the title screen.
- TWO CAVE MAPS (fights UNDER the surface): THE HOLLOW -- rolling cave floor beneath
  a slab ceiling pierced by three light shafts, fat support columns, fourteen hanging
  stalactites, rubble ramps under the shafts; and THE ROOTS -- jhiaxus's under-garden:
  bio-metal cave, grown ceiling membrane with two torn light rents, paired thorn
  columns like ribcage architecture, sixteen glowing spore pods, sunken digestion
  pools. Both are ENCLOSED: full interior light + the 0.20 reverb wash -- and the
  ceilings feed the cling/crawler systems, so jhiaxus's crawlers genuinely ambush
  from overhead. Theme table 24 -> 26.
- WEAPON MESHES: the five modes that shared old placeholder shapes now have distinct
  silhouettes -- PIERCE rail (long core between side rails + capacitor rings), FLAK
  (underslung drum + flared bell muzzle), PLASMA (bulb chamber + four coil rings),
  RICOCHET (twin angled deflector prongs), SUPPRESS (heavy vented shroud + long
  suppressor tube).
- PROJECTILES LIVE NOW: per-mode identity end to end. Shapes: pierce needles (8.5x),
  wave crescents (wide + flat), plasma comets (fat, slow at 62u/s -- watchable),
  flak slugs that TUMBLE in flight, burst/suppress snap at 150u/s as light tracers.
  Trails: flak sheds grey powder smoke, plasma leaves a glow wake in its own colour,
  and mortar lobs SMOKE THEIR WHOLE ARC (a puff every 0.05s). New SMOKE particle kind:
  grows 3.4x, rolls lazily, fades hard (smoke, not light).
50/50 tests pass; all shaders compile; both cave maps validate. HONEST: cave ceiling
sightlines and shaft lighting, the eye/horn staging angles in the boot scene (the
glow-disc's facing axis is my best guess), and every projectile speed/trail cadence
are play-unverified. If the boot glow disc renders edge-on instead of facing you,
that's the rotation[0]=90 guess -- one line to fix with a screenshot.

## Update 173 -- THE FRONTIER CLOAK (outer rim, from the source material)
Player flag: capes were never given their outer-rim ROLE. The digest's "Autobot
Frontier Cloak & Suit Systems" concept (cowboy/ice-explorer nanotech weave,
shape-memory alloy, reactive energon threads) is now a live system on the cold
outer-system worlds (pluto, kuiper, outer rim, mining, shield edge, enceladus,
europa, glaciem, frostline):
- PROTECTION: the weave sheds 10%% of ALL incoming damage on frontier maps
  ("protects from cosmic radiation and solar flares... shields from ice shards").
- CLOAKING: the "stealth drape" -- resonant-hunter sense range x0.72 on frontier
  worlds (stacks with the CLOAK perk's x0.6), and spark loudness decays 1.67x
  faster: going quiet actually WORKS out there.
- THE LOOK: on frontier maps the cape wears the concept palette -- muted icy
  steel with energon-blue seams that BREATHE -- and drapes heavier (parka weight,
  the prime ceremonial read). One-shot HUD line on first wear: "frontier cloak
  sealed: radiation shielded _ sensor-dim".
50/50 tests pass. HONEST: the 0.90 damage factor, 0.72 sensor factor and decay
rate are first-pass balance; the icy tint replaces whatever accent tint the cape
carried on those maps -- flag if the champion-paint capes lose identity out there.

## Update 174 -- driving x terrain: res 88, surface grain, gravity assist, dust wakes
Driving-feel + terrain-quality pass:
- TERRAIN RES 72 -> 88 (bound-checked: loader cap TERR_MAX=96 holds 89 samples): ~50%%
  more heightfield triangles per map, plus a fine SURFACE-GRAIN micro-octave (0.14
  amp, dual high frequencies) composited under the NASA landforms -- the ground reads
  high-res up close instead of faceted. All 26 maps regenerated; every map validates
  and every collider file stays under cap. Collision stays EXACT: the sampler already
  walks the same two triangles the mesh draws, so the higher-res surface is the
  collision surface, bit for bit.
- GRAVITY ASSIST (new physics): a grounded ground-vehicle now feels the slope --
  velocity gains 7.5 x gravity x gradient downhill, loses it climbing. Crest a dune
  and the run-out pulls; charge a rise and it costs. Scales with per-map gravity, so
  low-g moons let you carry speed uphill in a way Earth doesn't.
- DUST WAKES: over gritty ground (grit > 0.3: sand, regolith, rust) at speed, the
  wheel line sheds a rolling dust plume BEHIND the travel direction -- cadence
  tightens with speed (0.14s -> 0.05s), tinted from the ground material's own spark
  palette. The new smoke particles grow, roll and fade; a fast run across the wastes
  finally LOOKS fast from behind.
- CAUGHT LIVE (verify-first lesson, again): slope body-tilt ALREADY existed (folded
  into the lean targets, with a comment explaining a runaway-shear bug the current
  design fixes) -- my first draft re-derived it and double-tilted the hull. Stripped
  before shipping; the new block adds only what did not exist: the gradient physics
  and the dirt.
50/50 tests pass. HONEST: the 7.5 assist constant, dust thresholds/cadence, and how
res-88 terrain hits framerate (~50%% more terrain tris + denser SSAO/shadow work)
are play-unverified -- if fps dips, res 80 is a one-number fallback.

## Update 174 -- villain VOICES, PALE VECTOR, the data-core beat
The named warlords existed (novas, banners, homeworld prime forms) -- what they
lacked was PRESENCE between hits. Ported from the character docs:
- VILLAIN VOICES: while a named champion stands and knows where you are, he TALKS
  -- every ~13-18s, three rotating lines each, written in their documented registers.
  Cryotek speaks in clipped logic ("objective: cores. you: obstacle."), Blackstar in
  ashes-philosophy ("empires burn. i collect the ashes." -- his signature line,
  verbatim from the profile), Jhiaxus clinically ("you are a draft. i publish final
  forms."). Lines never clobber urgent combat messages.
- PALE VECTOR (the relic-keeper, from Blackstar's crew sheet): a thin, pale, hooded
  APPARITION now haunts the scavenger worlds (outer rim, kuiper, carrion). He stands
  far off, always facing you, barely breathing, one cold cyan optic pulsing. Come
  within 30u and he whispers ("the lattice is stirring." / "the array saw it first.
  no one wanted to look."). Press within 12u and he WARPS OUT in a burst of cold
  light -- "somewhere, a relic hums" -- returning elsewhere 40s later. Non-combat,
  pure mystique, exactly his documented role: Memory.
- THE DATA-CORE BEAT (campaign I.1): killing Cryotek on the pluto opener now lands
  the script's whiplash transition -- "data core recovered. its trajectory log burns
  sunward -- toward earth." The line that aims film one at the beach.
50/50 tests pass. HONEST: taunt cadence (13-18s) and Pale Vector's distances
(30u whisper / 12u warp) are feel-dials; his body is assembled from thin armour
plates rather than a bespoke mesh -- if he reads as a post instead of a person in
your screenshots, a dedicated model is the upgrade path.

## Update 175 -- campaign set-pieces: the under-garden, the patchwork king, ice quakes
Three chapters get their identity:
- V.1 "WHAT JHIAXUS GREW" MOVED UNDERGROUND: the mission now plays in THE ROOTS --
  the bio-metal under-garden cave built for exactly this -- and runs the FULL horror
  kit there (campaign horror was previously only I.2): stalkers that advance only
  while unseen, heavy fog, the dread meter -- in a cave whose ceiling his crawlers
  own. Fighting things that freeze when watched, under a roof that drops things on
  you when you look away.
- IV.1 "THE SCAVENGER KING" WEARS PATCHWORK: the outer-rim champion is now visually
  the ashes' king -- every body part tinted from a different salvage lot (five junk
  palettes hashed per-part: rust-bronze, gunmetal, oxblood, verdigris, bone-steel).
  A body built of other bots' parts, crowned in mismatch.
- VI.1 "THE VAULT OF ICE" QUAKES: every ~18-32 seconds the shelf fails somewhere
  nearby -- ice-crack report, a staggering shockwave (16u radius, knocks everyone),
  hard camera jolt, and a running crack-line of ice sparks toward you. First quake:
  "the shelf is failing. the vault knows you are here."
50/50 tests pass. HONEST: horror pacing in an enclosed cave (fog + darkness + 0.20
reverb may compound heavier than intended), the patchwork palette read at distance,
and quake frequency are play-unverified. If V.1 is TOO oppressive, the dial is which
horror components run there (fog can drop while stalkers stay).

## Update 175 -- THE SEA RETURNS (the mission wave) + PHOTO MODE
Two systems:
- THE SEA RETURNS: mission I.3 finally has its tsunami. The existing water-form
  ability was the only "tsunami" in the code -- the outrun-the-sea mission was a
  bare survive timer. Now, on a rolling cadence (~26s), the horizon LIFTS ("the
  sea returns -- HIGH GROUND. NOW."), the ground hums for 4 warning seconds, and a
  34-unit wave wall sweeps the whole beach west-to-east at 92u/s: six curling
  segments with a boiling white crest, leaning forward as they run. The drill is
  REAL: when the front passes through you, RELATIVE height decides -- standing on
  sand (even a dune crest: sand does not stop the sea) takes 22 damage and gets
  thrown east off your feet; standing on a refuge deck or any structure 4u+ above
  the local ground, the wave breaks BELOW you ("aiden whoops, terrified"). Grounded
  enemies get swept wholesale -- the sea does not aim. The refuge towers built into
  the map three updates ago finally have their purpose.
- PHOTO MODE: pause, press P -- every overlay vanishes, and a free camera detaches
  over the frozen world (wasd + mouse, space/ctrl for height, shift for 90u/s).
  A faint hint line fades away after five seconds and republishes every 30 so the
  frame stays clean for screenshots. P (or leaving pause) returns to the menu.
  Given this collaboration runs on your screenshots, aim it at anything suspicious.
50/50 tests pass. HONEST: wave damage/cadence/speed are first-pass; the 4u relative
safety threshold assumes decks register as player height above terrain (true for
box colliders); wave visuals are lit boxes + a glow crest -- if it reads as a wall
of slabs rather than water in motion, foam particles along the crest are the next
dial. Photo mode framerate honors whatever the frozen scene costs.

## Update 176 -- snow, crawlers that CRAWL, campaign progress persists
Three quality passes:
- SNOW ON THE ICE WORLDS: the precipitation pool now runs two modes -- titan keeps
  its scheduled methane showers (streaks, splashes, world-wetting), while frostline,
  glaciem, enceladus and pluto carry a CONSTANT slow snow: square flakes that float
  down at 4.5u/s, sway on a per-flake phase, ride 40%% of the wind, never splash and
  never wet the world. (Suppressed in horror mode -- the dread fog owns that air.)
- CRAWLERS FINALLY CRAWL: the long-pending pose problem solved through the transform
  system itself -- while climbing or perched, ambush crawlers now pose as their
  SPIDER alt-mode (the body folds into a scuttling spider on the wall, legs
  cycling at climb cadence), unfolding back to robot mid-pounce. The wall-crawl
  finally LOOKS like a wall-crawl.
- CAMPAIGN PROGRESS PERSISTS: mission unlocks now save to cybertron_campaign.dat on
  every completion and restore at boot (clamped to the mission count) -- closing the
  gap where a 13-mission campaign forgot everything on exit while the high score
  remembered.
50/50 tests pass. HONEST: snow density (40 shared particles), the spider fold-pose
read on a vertical face (the pose system was built for ground planes -- the fold is
correct but the wall orientation comes from position-glue, not a surface-normal
rotation), and flake scale are play-unverified.

## Update 176 -- geysers erupt, crawlers hunt, the codex knows the villains
Three items:
- ENCELADUS GEYSERS: the map's three vent mounds ("game erupts these" -- promised in
  the map comment, never delivered) now ERUPT on a staggered ~9-15s cadence: a 1s
  pre-hiss of gathering mist at the throat, then a 46-unit column of flickering
  ice-bright plume for ~3.2s (four stacked narrowing segments swaying as they rise),
  shedding drifting ice mist. Stand in the throat and the moon THROWS you 34u/s
  skyward -- "the vent FIRES -- ride it" -- a traversal toy, not a hazard; enemies
  caught in it go ballistic too.
- CRAWLER READ: discovered the crawlers already FOLD into spider-form while climbing
  (better than remembered) -- what they lacked was menace. Clinging crawlers now
  carry a fine predatory TREMBLE and drop to 60%% value: a shadow with joints on the
  ceiling until the pounce. Applied after posing so it survives the frame.
- CODEX DOSSIERS: four villain entries written from the character docs, inserted
  before the sealed appendix (which stays last, preserving the hybrid unlock):
  CRYOTEK (clipped logic, purity, the buried cores), BLACKSTAR ("empires burn.
  i collect the ashes." -- the name was earned, not given), JHIAXUS (clinical,
  "his gardens grow bodies perfected past identity. the crawlers in the roots are
  his drafts"), and PALE VECTOR ("he is the crew's memory. do not press him.").
50/50 tests pass. HONEST: geyser height/cadence/launch velocity are feel-dials;
the crawler tremble amplitude (0.045) may read as jitter at distance -- one
screenshot decides.

## Update 177 -- engine RPM, speedometer, landing dust, three codex entries
Feel + polish batch:
- ENGINE RPM (new ex_sound_pitch API into OpenAL): the engine/jet loop's PITCH now
  rides speed -- idle burble at 0.82x, flat-out ground at ~1.37x (+0.12 under boost),
  jets 0.90-1.35x -- spooling at 5/s rather than snapping. The new 31-speed cars now
  SOUND faster, not just move faster.
- VEHICLE SPEEDOMETER: a plain "NNN u/s" readout (plus a BOOST tag) on the HUD while
  in any vehicle. Speed sells speed.
- LANDING BURST: a grounded touchdown on gritty terrain kicks a six-puff dust ring
  around the chassis -- jumps over dunes now land with weight.
- THREE CODEX ENTRIES for the new systems, in the game's voice: "the frontier weave"
  (the cloak), "the under-garden" (the roots + its ceiling), "the eye beneath"
  ("...and sometimes, it blinks.").
50/50 tests pass. HONEST: pitch ranges and spool rate are ear-untested (AL_PITCH on
MojoAL resamples -- quality at 1.4x unknown to me); the speedometer row (vga row 6)
could collide with other HUD text on small gui scales.

## Update 177 -- molten avatar, crest spume, geyser roar, THE BOT SURVIVES RESTARTS
Four items (and two systems verified already-done before touching them: aiden's
skateboard and the settings/keybind config both fully exist):
- MOLTEN AVATAR: unicron's champion no longer wears a flat red glow -- it THROBS
  between deep red and orange-white on a slow furnace breath with a fast surface
  shimmer, DRIPS embers (gravity sparks falling off the body every 0.16s, dying
  where they land) and breathes heat-smoke off the shoulders. "a towering molten
  metal titan", per the script.
- WAVE CREST SPUME: the mission wave's boiling crest now TEARS white spume off
  as it runs -- drifting puffs shed along the whole front (the fix flagged in
  U175 for "wall of slabs" risk).
- GEYSER ROAR: each enceladus vent fires its steam roar the instant it erupts.
- THE BOT SURVIVES RESTARTS: the entire loadout -- size class, element, class,
  height/bulk/head/shoulders/arm/leg, accent, vehicle, paint, finish, signature --
  now persists in config.ini, saved automatically the moment you commit to a
  match. Previously only keybinds + graphics persisted: players rebuilt their
  transformer every launch. All values are range-clamped on load so a stale or
  hand-edited config can't produce an out-of-range build.
50/50 tests pass. HONEST: molten drip cadence (0.16s) is a particle-budget guess
-- if the vfx pool starves during big fights on the unicron map (shared pool),
the drip is the first thing to thin.

## Update 178 -- crawlers CLING (wall alignment) + pounce telegraph
Closing the last flagged crawler gap:
- WALL ALIGNMENT: while climbing, the whole posed spider body now ROLLS 90 degrees
  onto the climbed face -- every part rotated about a pivot at the body centre
  (clean Rz(+/-90) toward the approach side, parts' roll following) -- belly to the
  wall, back to the room. Combined with U176's spider fold-pose, the wall-crawl now
  reads as an actual wall-crawl: a folded spider glued flat to the face, scuttling
  UP.
- POUNCE TELEGRAPH: in the final 0.45s of the perch, the crawler's eye-line GLINTS
  red in three quick pulses ("it has CHOSEN") -- enough warning to dodge if you're
  watching the ceiling, which is the whole point of the horror: you have to keep
  watching the ceiling.
- (Checked and closed without changes: terrain normal-map coverage is already
  complete -- all 75 diffuse textures carry norm_ variants.)
50/50 tests pass. HONEST: the 90-degree roll composes with each part's existing
euler rotations in whatever order the renderer applies them -- box-like parts will
read correctly, but the visor/face plates may end up angled oddly on the wall; a
screenshot decides whether the pivot height (+1.1) needs a nudge.

## Update 178 -- transforms have VERBS: jump into vehicle, explode into robot
Player direction, implemented as layers OVER the mechanical origami (the hinged
part-by-part fold stays as the skeleton of the move -- it is what keeps limbs
connected; these give it character):
- ROBOT -> VEHICLE = a JUMP-TUCK: triggering the transform on the ground fires a
  real hop (9u/s + up to 6 more when battered), and through the blend every part
  GATHERS toward the core (up to 32%% pull mid-leap) -- a leap into a ball that
  lands rolling.
- VEHICLE -> ROBOT = a DETONATION: panels blow OUTWARD along their own radials
  (each part flies its own hashed distance, biased upward -- it erupts), then the
  fold snaps them home. An element-coloured spark burst fires at the trigger.
- RECENT DAMAGE SCALES THE VIOLENCE: a new accumulator remembers damage taken
  (caps at 60, decays 7/s). Pristine bot: crisp, tight, professional. Battered
  bot: the jump is a desperate scramble (+66%% height), the detonation is nearly
  3x wider, sheds grit smoke, and kicks the camera harder. The transform TELLS
  you how the fight is going.
50/50 tests pass. HONEST: tuck/burst magnitudes (0.22-0.32 gather, 1.1-3.2 radial)
are tuned blind -- the burst especially may need clamping if wide-shouldered
builds throw panels through nearby walls visually; screenshots decide.

## Update 179 -- enemies transform with verbs, killstreaks, the ashes' throne
Three items:
- ENEMY TRANSFORM VERBS (U178 parity): every bot in the world now transforms the
  way the player does -- a 6.5u/s HOP into the fold when shifting robot->drive/fly,
  and an element-coloured panel DETONATION when snapping back to robot. The whole
  battlefield speaks the same movement language.
- KILLSTREAKS: kills chained inside a rolling 3.5s window now call out -- DOUBLE
  KILL / TRIPLE -- scrap rain / QUAD -- they can't stop you / RAMPAGE xN. Counted
  at the damage wrapper so every fire path feeds it.
- THE ASHES' THRONE (carrion): blackstar's court finally has its seat -- a two-tier
  salvage dais at the arena's south edge, a canted seat-back of torn dreadnought
  plate ("every plate torn from a rival's hull"), twin obsidian harpoon standards,
  and cold energon banner plates. The IV.2 BREACH mission now has a visual center
  of power to breach toward.
50/50 tests pass; carrion validates under collider caps. HONEST: streak window
(3.5s) and the throne's exact sightline from the meet-ring are unverified; the
enemy hop fires on AI mode flips which can be frequent for hijack flyers --
if the skies look like popcorn, the hop gets a per-enemy cooldown.

## Update 180 -- CINEMATIC GUNPLAY: names, streak trails, the lance, the redline
The gunplay pass, per direction ("neutron blaster... get creative and epic"):
- WEAPONS HAVE NAMES: the ten fire modes are now weapons with identity, on the HUD
  and announced with a title card when mods change your mode -- NEUTRON BLASTER
  (bolt), TRIVOLLEY (burst), SOLAR LANCE (beam), TIDEBREAKER (wave), ORBITAL MORTAR
  (lob), RAILSPIKE (pierce), SCRAPSTORM (flak), SUNSPITTER (plasma), THE JACKAL
  (ricochet), HAILDRIVER (suppress). "SOLAR LANCE online."
- STREAK TRAILS (new EX_VFX_STREAK kind): the bright weapons leave their LIGHT
  hanging in the air -- stationary afterimage segments shed every 3u along the
  flight line, oriented with the shot, brightest young, dead in 0.2s (0.3s for the
  lance, whose ghost also runs longer). Neutron, trivolley, lance, railspike,
  jackal, and haildriver all draw lines of light across the fight now.
- SPEED RETUNE: the SOLAR LANCE moves at 340u/s (a near-instant spear of light, 13x
  long and thread-thin), the neutron blaster at an authoritative 135, the jackal
  whipcracks at 165. Muzzle presence scales per weapon: the sunspitter BLOOMS
  (1.7x flash, longer-lived), the scrapstorm flares wide, the haildriver spits.
- THE REDLINE (the ammo rhythm): heat now MEANS something -- redlining the barrel
  (heat 1.0) locks the gun and it VENTS: steam jets off the muzzle, cooling at
  nearly double rate, until 0.35 -- "REDLINE -- venting" / "vents closed -- weapon
  hot and ready". Sustained spray has a cost; bursts of discipline don't.
50/50 tests pass. HONEST: streak cadence (3u) roughly doubles bolt-related particle
load -- the shared pool may thin impacts in heavy fights (streaks are the first
dial); redline thresholds assume current per-mode heat gains (flak redlines in ~5
fast shots, neutron needs sustained hosing) -- tell me which weapons hit the wall
too early.

## Update 181 -- VEHICLE MODE CUSTOMIZATION: spoiler, exhaust, stance, rim glow
Four new customize rows (menu 21 -> 25 rows), all persisted in config.ini, all
posed live in vehicle mode onto the greeble slots vehicle mode never used:
- SPOILER (none / low wing / GT wing / whale tail): a carbon-dark tail wing on
  struts -- the GT stands 0.95u proud and rakes -8 degrees; the whale tail runs
  35%% wider than the hull. Banks with the chassis in turns.
- EXHAUST (hidden / twin side / quad stack / big single): heat-reactive pipes --
  idle they smoulder faint amber, under engine load + boost they glow toward
  white-orange. Twin sides run the rocker line, stacks stand up off the tail
  deck, the big single is one fat bore center-rear.
- STANCE (0-100, step 10): true ride height -- slammed drops the hull 0.27u
  toward the deck, raised lifts it 0.27u, folded straight into the hover base so
  it composes with the bob, courtesy-settle, and banking.
- RIM GLOW (off / accent-lit): two thin running-light lines in your ACCENT
  element colour along the rocker panels, breathing at 2.1hz -- the film-street
  underglow read.
All four are vehicle-mode only (slots hand back to the robot greebles on
transform) and cost nothing new: they reuse skel 86-91.
50/50 tests pass. HONEST: geometry offsets assume base part 0 approximates the
hull footprint (true for wheeled forms; beast alt-modes may wear a spoiler
oddly -- gating those is one condition if screenshots object); stance extremes
are visual only and don't change the collider.

## Update 182 -- campaign: BREACH implemented, trilogy two finds its voice
Two campaign systems:
- THE BREACH OBJECTIVE WAS NEVER IMPLEMENTED: EX_OBJ_BREACH existed in the enum,
  the IV.2 "carrion court" mission declared it -- and the game silently fell back
  to plain eliminate. Now it is real: a SEALED WARD burns over the ashes' throne
  (placed at the U179 throne dais), 600hp, pulsing faster and shifting colour as
  it fails. Your fire landing within 10u chips it (integrity callouts every 25%%),
  and shattering it ("THE WARD SHATTERS -- the court is open") wins the mission
  outright -- whatever still stands. Mission opens with the order: "BREACH the
  throne ward -- the court falls with its seat."
- TRILOGY TWO FINDS ITS VOICE: six new timed in-mission film lines (the U171
  system, extended): II.1 at 40s -- megatron: "i did not come to save you. i came
  to not die alone." / II.2 -- ratchet: "the mantle is singing. that is not
  geology." / IV.1 -- "the array is WATCHING. blackstar knows your pattern --
  break it." / V.1 -- prime: "these were people, once. carry that. then put them
  down." / VI.1 -- "the vault doors are FAILING." / VI.2 -- witwicky: "twenty
  years i flew this run in my head. WATCH ME THREAD IT."
50/50 tests pass. HONEST: the ward's 600hp vs weapon DPS is untested (railspike
may delete it in seconds -- the hp scales in one line); the ward position assumes
the throne sits where U179 placed it and hitscan impacts near it register within
the 10u sphere -- lob splash does not chip it yet, flagged as a known gap.

## Update 183 -- map quality pass: the five weakest, metric-targeted
Ran the map scorer, took the bottom five, read the scorer's actual metric
definitions, and fixed each map on its measured weak axes over three rounds:
- MINING 74.0 -> 85.3 A: HEADFRAMES -- paired winch-tower legs, crossbeams and
  wheel-houses over three shafts (the mining silhouette it never had) + an ore
  conveyor high-road between them. Verticality 0.25 -> 0.67.
- EUROPA 75.0 -> 83.8 A: CHAOS TERRAIN delivered at last (the comment promised
  it) -- eight rafted, canted refrozen ice blocks, three upthrust shell towers,
  two frozen plume columns, six low refrozen slabs. Vert 0.22 -> 0.51, cover
  0.84 -> 0.96.
- ROOTS 67.5 C -> 78.2 B: bio shelves at height, a tight thorn grove + a tall
  loner (fixing the SPREAD metric, which rewards varied neighbour distances --
  my first regular ring made it worse), two grown pillar-trunks, low pod
  clusters.
- VAULT 71.7 -> 77.8: six broken monoliths pushed to a wider span, two archive
  towers at the far corners, five low shards. Scale 0.55 -> 0.67.
- CORE 71.5 -> 77.9: a slag field at mixed scales widened across the floor, two
  frozen flow towers, and -- decisive -- eight TRUE-LOW slag lumps under eye
  height (the cover metric wants a low/high MIX; my first "low" pieces were
  secretly high). Cover 0.63 -> 0.75.
Suite mean 77.3 -> 78.9 over 27 maps; all five validate under collider caps;
50/50 tests pass. HONEST: the scorer measures geometry, not fun -- the numbers
say the fights have more room to be interesting, your hands will say whether
they are.

## Update 184 -- maps: the cage breaks, glaciem stands taller, THE DREADNOUGHT
- UNICRON 77.7 -> 81.7 A: the concentric descent tiers were perfectly regular
  (18 identical segments x3 rings = the suite's worst spread). Now they are
  BREAKING as he stirs: a third of every ring is gone, the rest jittered in
  angle and radius, some segments heaved to 1.6x height. Fiction and metric
  agree: the cage is failing.
- GLACIEM 74.1 -> 78.6: three sheared ICE MONOLITHS with aurora crowns at the
  far reach (the civilization's tall bones), five calved true-low ice chunks.
- NEW MAP: THE DREADNOUGHT (80.3 A on first tuning) -- fighting ON blackstar's
  flagship, straight from the profile: five hull-rib arches over the deck,
  lashed cargo in uneven clusters, four corsair docking cradles with guide-
  lights on the rails, three raked solar-sail masts with carbon membranes, twin
  harpoon batteries, and the BRIDGE TOWER aft, its viewport burning cold.
  Registered everywhere it matters: 27th theme; FRONTIER CLOAK active (deep
  space); PALE VECTOR haunts it (his master's deck); raiders field here;
  the champion that rises is BLACKSTAR HIMSELF -- fight him on his own ship.
  Lighting: hard starlight over dark hull, energon fill, near-black void sky.
  (First cut scored 60.2: a full-footprint deck plate zeroed openness; the map
  scorer caught it before any human had to.)
Suite mean 79.2 over 28 maps; all validate under caps; 50/50 tests pass.

## Update 185 -- HYDRODYNAMICS: real buoyancy, swells, diving, planing hulls
Water physics rework (the old model was binary: below a fixed depth -> constant
+22 shove, which popped and jittered):
- SPRING BUOYANCY TO A WATERLINE: proportional restoring force + heavy vertical
  damping = the body SETTLES onto its equilibrium and bobs there. No popping.
- MASS SETS THE DRAFT: the waterline depends on the build -- a light scout rides
  0.55u deep, a full-bulk siege frame wallows at ~1.7u, mass adds more. In
  vehicle mode, hover hulls draft a skimming 0.35u while tanks/mechs sink 1.9u.
- THE SURFACE BREATHES: two crossed long-period SWELLS (phased by world position)
  ride through every sea -- the waterline you float on rises and falls, and
  everyone floats on the same swell field.
- DIVE CONTROL: hold crouch to swim DOWN (thrust to -8u/s, thin bubble trail);
  release and buoyancy finds your waterline again. Robot scale only.
- REAL DRAG: quadratic-ish -- gentle when drifting, punishing at speed... EXCEPT
  planing: a shallow-draft hull above 10u/s runs at 45%% of the drag (hydroplane
  physics) with bow spray torn off the nose and a wake cadence that tightens
  into a V as speed climbs.
- BREACH HOP: jumping off the surface throws a spray ring.
- ENEMIES SWIM: they previously seabed-walked under the surface with drag only;
  now the same spring buoyancy floats each kind at its own mass-set waterline
  (apex ride low), on the same swells, shedding wading wakes.
50/50 tests pass. HONEST: spring constants (9.0 lift / 3.4 damp) are tuned for
settle-without-oscillation on paper; if the bob resonates with the swell period
in practice it will look like a metronome -- one damping number fixes it. Dive
uses LCTRL (only otherwise bound in photo mode, which is pause-only -- no
conflict).

## Update 186 -- DUST STORM FRONTS: planets get weather with a will
Rolling storm events on the five dust worlds (mars, wastes, titan, io, carrion),
each wearing its OWN dust colour (mars ochre, wastes rust, titan orange haze, io
sulfur-yellow, carrion grey-brown):
- THE FRONT: calm -> "the horizon goes RED -- storm front inbound" (4.5s build)
  -> RAGING 18-28s -> clearing ("the storm passes. the world coughs.") -> calm
  45-90s. Intensity is a smoothed envelope, so everything below fades in and out
  together.
- FOG CLOSES IN: the storm re-shapes the per-world fog every frame -- colour
  pulled 85%% toward the dust palette, density +0.034 at full rage. The horizon
  genuinely disappears.
- WIND SHOVES: strength up to 3.2x, direction drifting slowly; airborne bots get
  pushed 3.5x harder than grounded (flying through a storm is a fight). Dust
  devils spin up automatically at full rage. Grit rattles the camera faintly.
- STORM GRIT: the precipitation streak pool becomes near-HORIZONTAL flying
  filaments in the dust colour, respawning upwind, wind-stretched up to 1.7u
  long, flickering as they tear past.
- SENSORS CHOKE -- both ways: enemy sight capped as low as 33u at full rage, and
  resonance-hunting range cut 40%%. A storm is COVER: break contact, reposition,
  ambush out of the brown-out. (Stacks with the frontier cloak on wastes.)
- NO DOUBLE WEATHER: an active storm suppresses titan's methane rain -- the
  storm owns the sky.
50/50 tests pass. HONEST: fog density delta, wind shove, and the 33u sight floor
are feel-dials; the streak pool is shared with rain/snow (40 models) so storm
grit density is capped by it -- if the brown-out reads sparse, NUM_RAIN is the
one number to raise.

## Update 187 -- PATROL EXPANSION: the far signal, the space bridge, campaign set VII
Patrol mode grows from public events into a place where things HAPPEN, and the
space bridge ties it to the campaign:
- SIDE QUEST -- THE FAR SIGNAL (patrol only, chain): ~20s in, three RELAY
  FRAGMENTS scatter across the field (green pulsing pickups, "it is still
  TRANSMITTING"). All three -> the relay decodes -> the WARDEN OF THE GATE: the
  nearest hostile is promoted to a 4x-hp bounty burning gate-green. Kill it ->
  "a SPACE BRIDGE tears open."
- THE SPACE BRIDGE: two counter-spinning glow rings standing on the field,
  shedding mist. WALK THROUGH IT: flash, the bridge throws you 55u somewhere
  else -- and the first time, it CHARTS A NEW REGION:
- CAMPAIGN SET VII -- BEYOND THE BRIDGE (2 new missions, count 12 -> 14): VII.1
  "the far gate" (the hollow -- clear the corrupted dig, pale vector's voice at
  the end: "there are more") and VII.2 "the ashes court" (BREACH blackstar's
  command ward ON HIS OWN DREADNOUGHT -- the ward is map-aware now, burning at
  the bridge tower). Unlock rides the existing progression field
  (campaign.unlocked jumps to 12, persisted by the existing save) -- no new
  save machinery. Repeat bridge runs pay +300 energon credits instead.
- SALVAGE CACHES: four amber-glinting lashed crates scatter at every patrol
  start; crack one for +40 credits.
50/50 tests pass. HONEST: fragment/bridge distances (32-72u, walk-in 6.5u) are
feel-dials; the warden promotion picks the nearest living non-champion, so on a
sparse field it may crown a lowly grunt -- fictionally fine (the gate chooses a
vessel), flag it if it feels cheap. EX_MAX_MISSIONS is now exactly full at 14;
any future set needs the cap raised. Mission select will show the VII titles
even while locked -- acceptable as a teaser, one line to mask if not.

## Update 187 -- PATROL EXPANSION: the far signal, the space bridge, campaign set VII
Patrol grows from public events into a place where things HAPPEN, and the space
bridge ties it back to the campaign:
- SIDE QUEST -- THE FAR SIGNAL (patrol, chained): ~20s in, three RELAY FRAGMENTS
  scatter across the field (green pulsing pickups -- "it is still TRANSMITTING").
  All three -> the relay decodes -> the WARDEN OF THE GATE: the nearest hostile
  is promoted to a 4x-hp bounty burning gate-green. Kill it -> "a SPACE BRIDGE
  tears open."
- THE SPACE BRIDGE: two counter-spinning vertical glow rings shedding mist. WALK
  THROUGH: flash, thrown 55u across the field -- and the FIRST crossing CHARTS A
  NEW REGION. Repeat crossings pay +300 energon credits (the bridge's toll).
- CAMPAIGN SET VII -- BEYOND THE BRIDGE: two new missions (13 -> 15 total;
  EX_MAX_MISSIONS raised 14 -> 16): VII.1 "the far gate" (the hollow: clear the
  corrupted dig; pale vector: "now you have seen a far gate. there are more.")
  and VII.2 "the ashes court" (BREACH blackstar's command ward ON HIS OWN
  DREADNOUGHT -- the ward is map-aware now, burning at the bridge tower;
  blackstar's parting line: "kept. every plate of you."). The unlock rides the
  existing progression field (unlocked -> 13, persisted by the existing save):
  no new save machinery.
- SALVAGE CACHES: four amber-glinting lashed crates scatter at every patrol
  start; crack one for +40 credits.
- BUGS THE FEATURE EXPOSED (both fixed): the campaign actually had THIRTEEN
  missions, not twelve -- adding two blew the EX_MAX_MISSIONS=14 array and
  test_menu caught the overflow (raised to 16); and the U182 trilogy-two film
  lines were off by one mission for the same mis-count (array/people/vault/
  witwicky lines realigned to their intended missions).
50/50 tests pass. HONEST: fragment/bridge distances are feel-dials; the warden
promotion picks the nearest living non-champion, so a sparse field may crown a
lowly grunt -- fictionally fine (the gate chooses a vessel), flag it if it feels
cheap; mission select shows the VII titles even while locked -- a teaser, one
line to mask if unwanted.

## Update 188 -- gap-closing: splash breaches, no popcorn skies, the mystery set
Four flagged items closed:
- SPLASH CHIPS THE WARD (U182 known gap): apply_shockwave is the funnel every
  explosive path runs through -- mortars, lob bursts, explosions near the breach
  ward now chip it with distance falloff (80%% efficiency vs direct fire). The
  ORBITAL MORTAR is finally a siege weapon in breach missions, as it should be.
- NO POPCORN SKIES (U179 flagged risk): enemy transform-hops now carry a
  per-enemy 4s cooldown -- hijack flyers that flip modes constantly no longer
  bounce like kettle corn.
- THE MYSTERY SET: locked bridge missions in mission select now show
  "?? _ beyond the bridge (chart it from patrol)" instead of their real titles --
  the unlock stays a discovery AND the menu tells you where to find it.
- CODEX: "the space bridge" entry, before the sealed appendix -- "the network
  was never destroyed -- only unlisted... kill a warden and the gate reads you
  as worthy. that is not a malfunction."
50/50 tests pass. HONEST: the splash-vs-ward falloff (radius+8u reach, 0.8x)
means a max-forge mortar may still two-shot the ward -- the breach hp (600) is
the balancing number if so; the shatter burst from splash kills uses a plain
impact flare rather than the full explosion helper (declaration order), so a
splash finish looks slightly leaner than a direct-fire finish.

## Update 189 -- patrol: THE HUNTING PARTY + a persistent quest line
Two additions to the patrol loop:
- SECOND SIDE-QUEST KIND -- THE HUNTING PARTY (40%% of chain starts; the far
  signal keeps 60%%): up to three hostiles are MARKED as hunters -- 1.6x hp,
  permanently aware (they never lose the scent), pulsing blood-red so you know
  exactly who is coming. Kill all three -> +150 credits ("the hunting party is
  SCRAP"). Outlast the 45s clock instead -> +90. The far-signal bridge chain is
  untouched -- patrol now alternates between GO-AND-GET and THEY-COME-TO-YOU.
- PERSISTENT QUEST LINE: active quests now hold a cyan status row on the HUD
  ("QUEST: relay fragments 2/3" / "kill the WARDEN OF THE GATE" / "hunting
  party -- 2 left, 31s" / "the SPACE BRIDGE stands -- walk through") instead of
  living only in fading announcement text. You can always glance down and know
  what you are doing.
50/50 tests pass. HONEST: the hunt marks up to 3 LIVING hostiles at chain start
-- on a nearly-cleared field it may mark only one (paying full 150 for one kill;
the count is announced so it reads fair); hunter red overrides other identity
tints (raider/champion glow) for the hunt's duration by design -- the mark is
supposed to own the silhouette.

## Update 190 -- blizzards, a photographer's lens, credits on the tally
Three additions:
- BLIZZARD FRONTS: the storm system (U186) reaches the atmospheric ice worlds --
  glaciem and the frostline get white-out fronts ("the sky goes WHITE -- blizzard
  front inbound"): the same building/raging/clearing cycle wearing ice colours
  (0.72,0.80,0.92 white-blue fog, pale horizontal snow-grit on the wind), the
  same sensor choke -- a white-out is COVER on cryotek's turf, and it stacks
  with the frontier cloak there. Vacuum ice worlds (pluto, europa, enceladus)
  correctly stay stormless: no atmosphere, no weather.
- PHOTO MODE LENS: Q/E rack the fov 30-100 degrees (tele to wide), R resets to
  70. Compression shots of a champion against the horn, or wide venue shots of
  the dreadnought deck, are now composable.
- THE TALLY COUNTS CREDITS: the results screen shows "energon earned +N" for the
  match (baseline captured at match start, so patrol quest payouts, caches, and
  bridge tolls all show up). Score moved from row 8 to 9 to make room -- caught
  a row collision before it shipped.
50/50 tests pass. HONEST: blizzard palette on glaciem may wash out against its
already-pale ground fog -- if the white-out reads as "slightly milky" instead of
"lost in it", the fog density term is the dial; photo zoom persists only while
in photo mode (combat fov logic reclaims the lens on exit, by design).

## Update 191 -- WEATHER FROM THE MISSION ARCHIVES: six worlds, real physics
Dynamic weather grounded in what the probes actually saw:
- TITAN RAIN, PHYSICALLY TRUE (Cassini-Huygens): 0.14g + an atmosphere 1.5x
  denser than Earth's means methane drops fall at ~1.6 m/s and grow toward
  centimeter size. Titan's rain now DRIFTS down at 9u/s (vs 55 elsewhere) as
  fat slow drops -- you can watch a single drop fall the whole way.
- MARS: PLANET-ENCIRCLING DUST EVENT (the 2018 storm that ended Opportunity):
  ~8%% of mars fronts escalate -- "the sun is going out" -- 45-60s of rage,
  fog palette dimmed to 45%% (the tau spike browning out the light itself),
  half again the density.
- IO: PELE-CLASS UMBRELLA PLUMES (Voyager 1 caught nine erupting at once): two
  calderas take turns throwing 38u fountain columns that spread into a sulfur-
  yellow fallout canopy -- the umbrella shape is the real ballistic form these
  plumes take in vacuum.
- ENCELADUS: THE PLUME FALLS HOME (E-ring physics): after each tiger-stripe
  eruption, 6s of fine ice-snow sifts down across the field -- what goes up
  comes back as the freshest snow in the solar system.
- EUROPA: TRANSIENT VAPOR PLUMES (Hubble 2013/2016): rarely (~75-135s), a thin
  silent 42u ghost-column stands on a lineae for 7s -- "the ocean, breathing."
  No sound, no launch: vacuum. Just evidence.
- PLUTO: BLUE HAZE BANDS (New Horizons' ~20 tholin layers): the fog breathes
  gently blue on a slow ~57s cycle.
50/50 tests pass. HONEST: plume/fallout rates share the global vfx pool -- io
during a dust storm is the worst case and may thin other effects; titan's slow
rain means drops linger ~3x longer on screen, so the 40-streak pool reads
denser there (intended, but flag if it reads as fog); europa/enceladus plume
positions are fixed picks, not sourced from the actual DEM vent coordinates.

## Update 192 -- vents on the TRUE fissures: sulci + lineae alignment
Closing U191's honesty note ("positions are fixed picks, not sourced from the
actual vent geometry"):
- ENCELADUS: the geyser mounds moved ONTO the four tiger stripes -- one vent per
  sulcus at z=-62/-45/-28/-11 (Damascus, Baghdad, Cairo, Alexandria, as on the
  real moon, where every plume erupts FROM a stripe). The map's mound props and
  the game's eruption coordinates both updated, still perfectly matched; a
  FOURTH vent joins the cycle (cadence widened 11-17s so four vents share the
  sky without constant noise).
- EUROPA: the transient plume now stands ON the map's actual lineae -- four
  candidate points computed from the builder's three ridge-line equations, one
  picked at random per event, base height sampled from the DEM terrain.
  "transient plume on a lineae -- the ocean, breathing."
Map revalidates (59 colliders, under cap); 50/50 tests pass. HONEST: with four
enceladus vents the launch-toy coverage is denser -- if eruptions chain-launch
too often during fights there, the cadence floor (11s) is the dial.

## Update 193 -- MICRO-GRAVITY MAPS: the driftfield + the derelict
Two new maps (30 total, suite mean holds 79.2) in a NEW gravity tier -- 0.16x,
far below the belt maps' 0.55: jumps sail, falls are suggestions, jets are king.
- THE DRIFTFIELD (78.6): fighting IN an asteroid stream. The swarm travels in
  FAMILIES, as real belt objects do -- three tight clusters, four far loners --
  with rocks hanging at every height from deck-level boulders to 12u drifters.
  Centerpiece: a shattered ore-hauler in three tumbled sections, spilled energon
  glowing across her midship, a snapped mast, loose ore glittering between the
  rocks.
- THE DERELICT (79.4): a generation ship that never arrived. Her keel runs 168u
  stem to stern; six hull ribs arch over it, two snapped at the breach; cryo
  racks amidships still glow faint; the reactor drum sits dark aft with one
  heartbeat of energon left; the bridge viewport forward is dead but lit. Torn
  hull plates drift at height. Inside her ribs the sealed-space REVERB wash
  applies -- she still has acoustics, even dead.
Both: frontier cloak active (deep space), naked-sun vacuum lighting, deep belt
black skies. Registered as themes 28 + 29.
50/50 tests pass; both validate under caps. HONEST: 0.16g is chosen for play,
not realism (true belt micro-g would be ~0.0001g -- unplayable); enemy AI
ground-pathing may look floaty-odd on driftfield's high rocks since walkers
can't reach them -- flyers and the crawler-climbers are the natural predators
there, which is either emergent map identity or a complaint, your screenshots
will say which.

## Update 194 -- LOW-G LOCOMOTION: any orientation, auto-align, moon-bounding
Player direction: "controls and engine adapt to auto-align on land, low-g
animations, any orientation possible." Implemented as a whole-body pass on the
assembled player (runs after every part is placed, before the enemy section):
- ANY ORIENTATION IN FLIGHT (maps with g <= 0.6): the body's PITCH follows the
  velocity vector (dive nose-down, climb pitched back, up to 70 degrees) and it
  BANKS into turns like a diver. Applied by rotating every body part about a
  chest-height pivot (proper Rodrigues rotation about the yaw-right and
  yaw-forward axes) with the angles folded into part orientation -- the meshes
  lean with their new positions. Smoothed at 6.5/s: continuous, never snapped.
- AUTO-ALIGN ON LANDING: while descending, time-to-touchdown is estimated from
  altitude over the DEM terrain; inside 0.55s the body RIGHTS itself
  progressively, arriving level exactly at the deck -- flight ends in a strut,
  not a faceplant. Touchdown absorb rides the existing landing-squash system.
- AUTO-ALIGN ON LAND: standing on terrain in low-g, the stance tilts to the
  local slope (terrain sampled fore/aft + side, clamped 14 degrees) -- the bot
  stands ON the asteroid, not through it.
- DRIFT POSE: floating in micro-g the arms SPREAD from the body with a slow
  flutter -- a diver holding position, not a statue falling slowly.
- MOON-BOUNDING GAIT: stride cadence now scales with sqrt(gravity) (real
  pendulum-gait physics) -- on 0.16g maps strides slow to ~40%% rate while
  ground speed holds, producing the long-loping Apollo bound.
50/50 tests pass. HONEST: the euler fold (rotation[0]+=pitch, [2]+=roll) is an
approximation on top of exact position rotation -- at combined extreme pitch+
roll individual meshes may twist slightly off their true axis (positions stay
exactly right); the camera deliberately stays y-up -- full camera reorientation
is a sickness machine and the engine's collision is y-up AABB, so "any
orientation" is the BODY's, which is what reads on screen.

## Update 195 -- content pass: pods + dishes, cryoglass + tholin
(One thing verified before touching: the beach ALREADY generates with dunes=True
-- no new terrain style needed.)
- TWO NEW PROPS in the world scatter rotation (6 kinds -> 8, so every map's
  mid-ground furniture gets fresher): DECO_POD -- an escape/cryo pod lying on
  its skids: tapered capsule hull, domed aft, nose cap, and a frosted viewport
  bulge ("the solar system is littered with these"); DECO_DISH -- a comms dish:
  mast, elbow, squashed parabolic bowl pitched at the sky with a feed horn
  ("every frontier map has someone listening").
- TWO NEW GENERATED TEXTURES (+ normal maps): CRYOGLASS -- frost-fern
  crystallization over cold blue with a faint inner glow ellipse (the sleeper's
  silhouette light); THOLIN -- Pluto's red-brown photochemical fallout in soft
  drifts with darker pooled crevices (Cthulhu Regio's paint).
- APPLIED IN-WORLD: the derelict's cryo-rack caps now wear CRYOGLASS (each rack
  reads as a lit, frosted pod -- someone might still be inside); PLUTO gains
  four THOLIN DRIFT patches across Sputnik's cells -- the New Horizons
  red-brown, finally on the ground. The pod prop itself is skinned in
  cryoglass; the dish in plate-blue.
50/50 tests pass; derelict + pluto revalidate. HONEST: prop scatter placement
is hash-driven per map, so which maps draw pods vs dishes is luck of the
rotation; the pod's frosted viewport uses the texture's glow ellipse rather
than true emission (the prop pipeline has no emission channel) -- it reads lit
in ambient, not in darkness.

## Update 196 -- THE RIP: doom-inspired finishers where WHO you are decides HOW
Melee execution system, per direction ("optimus less violent... others brutal...
others clean... dinos different than scouts"):
- STAGGER WINDOWS: any non-champion under 24%% hp REELS -- movement damped, the
  whole frame pulsing slow amber. Within 7.5u: "staggered -- FINISH [E]".
- THE EXECUTION: [E] kills through the normal damage path (streaks, waves,
  score all count) and the enemy's ACTUAL body parts -- the same models that
  walked at you -- go ballistic: per-part velocities, tumbling euler spin,
  terrain-aware landing with one small bounce, parts LYING where they fell,
  fading out over the last 0.4s. A ripped frame does not topple; it scatters.
- STYLE BELONGS TO WHO YOU ARE (class-mapped, 6 animations):
    prime            -> MERCIFUL: parts slump gently off the frame ("a clean
                        end. prime would nod."), softest camera, no smoke
    reaver / pyre    -> BRUTAL: "TORN TO SCRAP" -- everything flies at 13u/s +
                        jitter, oil-black smoke, hardest camera hit
    scout/interceptor-> PRECISION: the player BLINKS THROUGH to the flank
                        (positioning, not force) and the parts pop in one tight
                        radial ring
    specialist/warden-> SURGICAL: parts lift in ordered anti-grav float, then
                        drop -- "disassembled. every bolt accounted for."
    vanguard/titan/siege -> HEAVY power-rip, strong radial burst
    DINO / DRAGON alt-mode OVERRIDES all -> THRASH: savage 17u/s all-direction
                        double-arc scatter -- "the beast takes its due"
- DOOM'S BARGAIN: every finisher feeds you -- +12 hp, +15 credits. Wounded and
  surrounded, diving INTO the crowd is the play, exactly as the inspiration
  intends. In micro-g the ballistics use map gravity: rips in the driftfield
  drift, gloriously.
50/50 tests pass. HONEST: [E] is shared with the interact key -- contexts should
not overlap but a staggered enemy next to a pickup will prefer whichever
handler runs first (the rip runs earlier in the frame); stagger excludes
champions by design (bosses deserve their own cinematic deaths, unbuilt); part
fade-out stands in for persistent wreckage -- true persistent gibs would need
model pool growth.

## Update 197 -- HALO ASSASSINATIONS + THE SNEAK: stealth becomes a build
Building on U196's rip system, per direction:
- SNEAK STANCE (hold LCTRL on land): creep at 8.5u/s. While sneaking your spark
  runs COLD -- resonance decay 1.6x faster, corrupted sense range x0.62, and
  raw sight capped at 42u. Stacks with the frontier cloak AND storm brown-outs:
  a sneaking, cloaked bot inside a blizzard is functionally a ghost.
- ASSASSINATION WINDOWS: any UNAWARE enemy -- full health included -- can be
  executed with [E] within reach: "unaware -- ASSASSINATE [E]". Aware enemies
  face you, so you cannot backstab attention: you must CATCH them, exactly as
  halo demands. The kill uses the full rip ballistics (class style + beast
  override all apply).
- THE APPROACH WRITES THE ANIMATION LINE:
    descending from the air -> "DEATH FROM ABOVE -- aerial assassination"
    sneaking               -> "GHOSTED -- the pack never knew" (+25 bonus
                              credits, +6 bonus hp, NO sound, NO spark flare)
    on foot, from behind   -> "BACKBREAKER -- it never saw you"
- STEALTH ECONOMY: a loud RIP now flares your spark (+0.5 resonance -- every
  corrupted nearby feels it); a ghost kill flares NOTHING, and the victim never
  alerts the pack either way. Chain-clearing a patrol silently is now a real
  playstyle with its own income (+40 credits per ghost).
- APEX EXCEPTION: "you can't assassinate a GOD -- but you can wound one":
  an unaware apex takes a 45% max-hp strike instead of the instakill.
50/50 tests pass. HONEST: unaware-only backstab means facing geometry is
implicit, not measured -- an enemy mid-turn who has not flagged aware is still
assassinatable (reads as catching them off guard; flag if it feels cheaty);
sneak shares LCTRL with the water dive, cleanly separated by the g_in_water
gate.

## Update 198 -- THE JAZZ RIP: "you want a piece of me? NO. i want TWO."
The 2007 film's most infamous kill, as a size-gated seventh execution style:
- THE GATE: a BIG frame (size-class titan or larger, or bulk past 0.72) ripping
  a SMALL bot (grunt or hijack swarm frame). Prime never does it -- his mercy
  style holds regardless of size. The beast never does it -- it thrashes.
  Everyone else with the mass to do it: megatron's move.
- THE TEAR: instead of scattering, the victim separates into TWO COHERENT
  HALVES split at the waist -- chest/head/shoulders/arms/hands/back hoisted and
  thrown along your shoulder line at 8.5u/s while the hips/thighs/shins/feet
  drop away the opposite direction. Each half moves as ONE (per-part jitter
  nearly zero, 0.9-1.1 coherence) -- the unified motion is what sells "torn in
  half" rather than "exploded." The tear line smokes.
- THE LINE: the kill card quotes him -- "'you want a piece of me? NO. i want
  TWO.'" Camera hit sits between heavy and brutal.
Works through both paths: staggered finishers AND halo assassinations (ghosting
a patrol as a titan-class and jazz-ripping the sentry is now a sentence that
parses). 50/50 tests pass. HONEST: the waist split is by part enum, so back-
mounted signature gear travels with the upper half (correct); the halves share
YOUR facing (torn across your grip) not the victim's -- watch a screenshot to
confirm it reads; corrupted/ravager victims are excluded (too big to be jazz).

## Update 199 -- MAPS: CITYFALL + LUNA (32 total, mean 79.4)
Two new maps filling the biggest remaining fiction gaps:
- CITYFALL, 82.7 A on first cut (Earth downtown -- the mission-city fight the
  films live in): tower blocks in four irregular skyline families + two tall
  loners (tallest 17 units at the east edge), the ELEVATED FREEWAY cracked
  across downtown on pylons, stalled traffic as low cover, plaza planters, an
  overturned energon hauler cracked open at the meet, buckled street terrain.
  Late-afternoon sun between towers, smoke-hazed dusk sky. Built as aerial-
  assassination country: drop off the freeway onto a sentry.
- LUNA, 79.4 (Earth's moon + THE ARK): four crater rims, far-flung ejecta
  boulders, two sheared crater-wall pinnacles -- and the ship that started
  everything half-buried nose-down in the far wall: buried bow mass, torn
  midship, sheared engine block, the dark viewport line, a snapped antenna
  mast, crash debris fanned south, and one abandoned survey lander. Gravity
  0.30 (Apollo bounds -- the moon-bounding gait + landing auto-align built in
  U194 get their signature stage). Lighting is apollo photography: BLAZING key
  light, near-black fill, black noon sky.
Both validate under caps; 50/50 tests pass. HONEST: cityfall's freeway skyway
is one long collider -- fighting ON it works, fighting UNDER it depends on the
pylon spacing reading fair (screenshot call); luna wears no atmosphere so
storms correctly skip it, but motes default neutral -- lunar dust kicked by
footfalls would be a lovely later touch.

## Update 200 -- THE GATE RAID: what the far gate kept (DbD-inspired, blackstar lore)
An asymmetric-horror raid, seeded by VII.1's outro ("at its bottom: a door,
older than the war, still sealed") and gated exactly as directed -- patrol, on
the hollow, only after the bridge set is charted (unlocked >= 13):
- THE DOOR: a dark-red seal turns slowly at the cave's south wall. Walk in.
- THE KILLER -- THE AMALGAM OF THE GATE: the devastator lesson applied to our
  own lore: a warden-gestalt "fused from every frame that ever failed the
  crossing -- many bodies, one hunger." The biggest living frame in the cave is
  TAKEN: 2400 hp, permanently aware (it NEVER loses you), massed to 1.55x
  scale, rust-and-gate-green shifting skin. It cannot be staggered or
  assassinated. Two other frames DEFECT to your side when the door opens --
  allies who draw its eye (the 6v1 spirit at our scale).
- THE OBJECTIVE -- FORGE 4 RELICS: four forge-stones ring the cave at the
  cardinal-diagonals. Channel by standing close (6s each); forging pins your
  spark resonance to MAXIMUM ("it can HEAR this") and the forge STALLS whenever
  the amalgam is within 15u -- the DbD generator loop: work, get hunted, break
  away, return. Each relic forged ENRAGES it (+35% press per relic) -- "the
  amalgam SCREAMS."
- THE BANISHMENT: 4/4 -> the door reopens gate-green -- carry the relics to it
  and the amalgam is BANISHED (+500 credits, counts as the kill for streaks/
  waves). OR: kill 2400hp with guns alone -- "the amalgam falls to force
  alone" -- for +800. Horror rewards craft; force pays better and costs more.
- Persistent red HUD raid line; repeatable per patrol visit.
50/50 tests pass. HONEST: the amalgam reuses the host kind's AI (relentless
pursuit comes from pinned awareness + enrage, not bespoke hunting patterns --
mind-game depth is a later layer); its 1.55x massing is a per-frame visual
scale, so its hitbox stays host-sized (shots at the visual edges may miss --
flag if it reads wrong); ally defectors use the truce brain and may die early,
which is authentically dead-by-daylight.

## Update 201 -- DISGUISE PROTOCOL: robots in disguise, literally (raid prop hunt)
Per direction ("let transforming into objects be allowed temporarily sometimes
in this mode") -- during the gate raid, crouch + transform [LCTRL+T] and you
BECOME A ROCK:
- THE MOVE: the entire robot vanishes -- every body part, the whole skeleton
  pool, cape, weapon -- replaced by a single cave boulder sized to your build
  (your mass is your boulder; full-bulk frames make convincing big rocks),
  tinted cave-grey, rotated 37 degrees ("it fell here long ago, obviously").
- THE BLINDNESS: the amalgam -- whose defining trait is that it NEVER loses
  you -- goes completely blind to a rock: its awareness pin releases, and your
  spark resonance reads zero ("a rock has no spark to hear"). It wanders. You
  breathe.
- THE RULES: 8 seconds maximum. Moving (above a drift threshold) or firing
  BREAKS it instantly -- "the disguise BREAKS". Full expiry warns you: "it
  will remember the shape." 30s recharge -- "the gate notices patterns" -- with
  a CHARGED callout when it's back.
- THE PLAY PATTERN it creates: forge until the amalgam closes -> break away ->
  rock up in a boulder field as it storms past -> resume. The DbD locker/
  stealth juke, but the transformers version -- hiding as an object is the
  franchise's founding joke, and here it is load-bearing.
50/50 tests pass. HONEST: the boulder is a tinted box, not a sculpted rock
mesh -- among the hollow's angular geometry it should pass, but a sculpted
deco-rock prop is a one-liner upgrade if it reads too clean; enemies other
than the amalgam also naturally lose a vanished body (their sense still works
on proximity), which is generous but consistent with the fantasy.

## Update 202 -- CHICAGO: the aftermath, destructible, where the record begins
The dark-of-the-moon aftermath map, 83.2 A on first cut (the strongest new-map
debut yet), in skirmish rotation as theme 32:
- THE RIVER: a real WATER REGION wall-to-wall across the map's waist (the full
  hydrodynamics apply -- swells, wakes, diving under the spans), between rust
  embankment walls.
- THE BASCULE BRIDGES: the west span crosses level on pylons; the east span's
  leaf has been JAMMED OPEN since the battle -- approach deck, then the raised
  leaf canted skyward with its warning-striped tip, flanked by counterweight
  towers. Jumping the open leaf at speed in vehicle mode is the obvious dare.
- THE SKYLINE: three tower families on the north bank + the 18-unit SENTINEL
  standing tallest at the east edge; on the south bank THE FALLEN TOWER -- a
  leaning slab stack with its crown lying in the street and a rubble fan.
- THE RECONSTRUCTION: two cranes (mast, jib, hanging hoist cable), scaffolding
  sheets with top walkboards, staged girder stacks -- the city rebuilding
  itself around the fight.
- DESTRUCTIBLE: chicago carries the engine's destructible chunk-cover layer
  (shootable, breachable rubble cover -- "cover destroyed") dressed by the
  ruin; the IQM skyline itself is permanent, as skylines are.
- WHERE THE RECORD BEGINS: a memorial obelisk with a cold energon flame near
  the river, and a new codex entry -- "chicago: the first page": the blackstar
  material opens HERE, in the reconstruction, where a naval commander read the
  salvage economy in the ruins and drew his first conclusion: empires burn.
50/50 tests pass; validates at 60 colliders. HONEST: the river's water plane is
a dark visual sheet at y=0.06 while the physics surface sits at 0.55 -- if the
mismatch shows at the embankment line, one number aligns them; the raised
bridge leaf is climbable geometry but its jump-clearance at vehicle speed is
untested math.

## Update 203 -- CHICAGO: closing both honest caveats from Update 202
Went back and fixed the two flagged gaps rather than leaving them as debt:
- **Waterline alignment:** the river's dark visual plane was floating at
  y=0.06/0.12 while the physics surface (WATER_REGIONS surf) sits at y=0.55 --
  a visible mismatch at the embankment line. Raised the visual plane to sit on
  the physics surf (cy=0.55, thin hy=0.04) so what you see IS where you float.
- **Bridge leaf reachability, checked by the numbers:** worked the jump math
  the caveat asked for. Vertical jump height caps at jv^2/(2*gravity); with
  gravity=52 and wheeled/tracked baseline jv~9 (before per-build multipliers,
  which top out modest for heavy vehicles), max clearable height lands around
  0.8-1.9m -- well short of the leaf's 2.6m rise from the approach deck. So the
  "obvious dare" was actually IMPOSSIBLE for most vehicles, not just untested.
  Added a 4-step launch ramp closing the gap so every vehicle can cross for
  real; skipping steps to jump it directly is still there for style, now as an
  optional shortcut instead of the only route.
Regenerated world data (tools/gen_world.py), rebuilt clean, 50/50 ctest green.
map_score: chicago 82.4 A (was 83.2 A -- the 4 extra ramp colliders trimmed
vert/scale slightly; still comfortably an A, still the strongest new-map debut).

## Update 204 -- Sculpted limb mesh + the disguise boulder fix
Two backlog items closed, both flagged by name in earlier updates:
- **Limb mesh detail** (README's "Next: sculpted limbs/shoulders"): plimb.iqm
  was a plain tapered tube shared by every arm and leg on every bot. Added
  robot_limb() (tools/gen_props.py, same P-list/comb pattern as robot_head/
  robot_chest): a shoulder/hip cuff ring and a wrist/ankle cuff ring (short
  tapered bands, radius interpolated to the core's local taper so they hug the
  limb instead of floating), plus an elbow/knee hinge knob and a raised front
  armor strip. 469 verts, up from the plain tube -- lifts every bot at once,
  same lever as the head/chest passes.
- **Disguise boulder** (HONEST note in Update 201: "a tinted box, not a
  sculpted rock mesh"): the raid's disguise-protocol prop was ex_model_copy'd
  from g_boxtpl. Swapped it to boultpl (the same sculpted boulder.iqm already
  used for cover-block variety elsewhere in this same init function, so no new
  asset needed) -- the disguise now reads as an actual rock, not a cube.
Regenerated props + world data, rebuilt clean, 50/50 ctest green.

## Update 205 -- Per-vehicle head variants
Closed the last item on the README's alt-mode-remnant backlog ("Next: sculpted
limbs/shoulders + per-vehicle head variants" -- limbs landed in Update 204,
heads land now). The alt-mode remnant switch in robot_parts.c already gave
CAR/JET/DINO/TANK families distinct BODY silhouettes but only DINO touched the
head (a snout stretch); the other three shared one head shape.
- **CAR/CYCLE** -> compact rounded helm (sx*1.08, sy*0.92).
- **JET/GUNSHIP/HAWK/WRAITH/MANTA/BULWARK** -> sleek aero head, narrowed and
  swept forward (sx*0.85, sz*1.25, offset.z+0.05).
- **DINO/RAPTOR/DRAGON/SCORPION/WOLF** -> existing snout stretch gets a
  forward jut + lower brow (offset.z+0.04, sy*0.94) so it reads as a jaw, not
  just a longer box.
- **TANK/MECH/SENTRY** -> wide flat siege helm (sx*1.25, sy*0.85).
By hand: the resulting head aspect ratios (width/height) come out ~1.25 (car),
~1.42 (jet, on the depth axis), ~1.25 (dino, but stretched on depth not
width), ~1.56 (tank) -- four genuinely different silhouettes, not just a
shared box at different sizes. Runtime-parametric (robot_parts.c), so it
applies to every existing and future bot with no new assets. 50/50 ctest
green, runs clean.

## Update 206 -- Raised the two weakest real maps (vault + core), by the numbers
Ran map_score.py --all across all 33 maps to find the weakest spots instead of
guessing. arena_base scored 0/F but that's by design (collision-hull-only, no
visual geometry -- see its doc comment); the weakest actual ARENAS were vault
(77.8 B) and core (77.9 B), both dragged down by the same two sub-scores:
- **scale** (~0.66-0.67): both maps' collider spread topped out around
  ±65 units, well short of the S=85 wall boundary and the scorer's ~200u
  ideal span. Added two outer landmark spires per map, placed near the wall
  (centers at |x|~82-83, safely inside S=85) -- vault gets sentinel spires,
  core gets vent spires with their own molten glow base, matching each map's
  existing vocabulary rather than bolting on generic props.
- **cover** (~0.72-0.75): both maps were almost entirely HIGH cover (vault
  7 low vs 57 high; core 8 low vs 49 high) against the scorer's ~55%-high
  ideal, so low shoot-over cover was badly underrepresented. Added ~18-20
  scattered low pieces to each (crystal shards for vault, slag lumps for
  core), keeping the central landmark clear so it's still the focal point.
Result: vault 77.8 B -> 85.2 A, core 77.9 B -> 84.0 A. Mean across all 33 maps
79.4 -> 79.9. Regenerated world data, rebuilt clean, 50/50 ctest green,
map_score --check still passes (all 8 gated terrain maps above the floor).

## Update 207 -- Swept every remaining B-grade map to A
After vault + core (Update 206), ran map_score.py --all again and fixed every
other map still sitting at B: trench, roots, beach, driftfield, emberpit,
glaciem, titan, derelict, io, kuiper -- ten maps, same by-the-numbers method
each time (extend span toward the wall boundary where scale was low, add low
shoot-over cover where the high/low balance was skewed, add real height where
verticality was low), always matching each map's own texture/prop vocabulary
rather than bolting on generic filler.
Caught a real mistake mid-pass: my first attempt at beach + emberpit used
THIN spires (radius ~1.1-1.3) for height variety, but write_colliders() skips
any collider under 1.2 half-extent as a "greeble" -- so those additions barely
registered as real collision despite looking right in the generator code. Ran
the score numbers, saw beach/emberpit hadn't moved, checked the actual .col
output, found the filter, and redid both with properly-sized pillars (radius
>=1.6-2.3). Worth remembering: verify the generated data, not just the code
that's supposed to produce it.
One honest limit: kuiper's cover was the most skewed of any map (12 low vs
104 high) and closing it fully would need ~70 more low pieces -- but the
engine caps maps at 150 colliders (already flagged in a comment in that
block). Added 30, landing at 146, well under the cap: a big improvement
(cover 0.72->0.86) but not a perfect 55/45 balance. Noting the limit rather
than claiming it's fully fixed.
Results: trench 78.1->85.9, roots 78.2->80.1, beach 78.6->80.2, driftfield
78.6->88.7, emberpit 78.6->82.7, glaciem 78.6->85.4, titan 78.6->83.9,
derelict 79.4->84.3, io 79.6->81.8, kuiper 79.9->84.9 -- every map in the
game is now A-grade or better. Mean across all 33 maps: 79.4 (start of
Update 206) -> 81.4. Regenerated world data, rebuilt clean, 50/50 ctest
green, map_score --check still passes.

## Update 208 -- Fixed dozens of truncated HUD/story messages (a real, silent bug)
Every build of this project has been printing ~50 -Wformat-truncation warnings
without them ever getting addressed. Actually read through them this pass
instead of letting the build noise sit there: they weren't compiler pedantry,
they were g_shot_msg (the main HUD message line) being sized at 48 bytes
while dozens of the game's own mission dialogue, boss taunts, and story beats
are 60-113 characters long -- meaning lines like the SQUAD OF SIX declaration
prompt, Ratchet's/Megatron's over-the-wire lines in the dust-event sequence,
and multiple raid/boss VO lines were being silently CUT OFF mid-sentence at
runtime. This has apparently been happening since g_shot_msg was first sized,
undetected because nothing ever fails a test over display text.
Checked ex_vga_print() before touching anything: it already wraps at the
screen edge and wraps vertically too, so there was no reason for the buffer
to be this tight -- enlarging it carries no overflow risk, confirmed by
reading the renderer, not assumed.
- g_shot_msg: 48 -> 160 bytes (longest real message is 113 chars; margin for
  future lines).
- g_event_msg: 64 -> 96 bytes (one message needed 70).
- Three smaller HUD buffers (survivor count, elemental-surge timer, the
  combine-warning line whose static text alone was 47 chars against its
  48-byte buffer) bumped for the same reason.
Result: zero format-truncation warnings on a clean build (was ~50). Every
story/mission/taunt line in the game now actually displays in full instead of
losing its ending. Rebuilt clean, 50/50 ctest green.

## Update 209 -- THE ART BIBLE + first IMAX detail passes (film-sourced)
New reference layer: docs/ART_BIBLE.md -- an art-direction bible that quotes
the DOOMSDAY/DOMINION/DESTINY treatments directly and translates each cited
film moment into buildable geometry/light targets. Core rules distilled from
the storyboard's own language ("Minimal dialogue. Maximum gravity."): one
enormous readable idea per map; three registers (geological scale, human
foreground, a single wound of light); detail adds gravity, not clutter.
First passes implemented from it, all cited to source:
- **BEACH ("Earth Splits")**: THE SEAM -- one continuous molten line running
  the full seaward edge ("a glowing red seam splits the horizon"); THE EYE --
  an ember disc at its midpoint ringed by dark crust plates ("a colossal eye
  ... flickers beneath the crust"); the DRAINED SEA -- a pale receded
  waterline band ("the ocean drains before the LA fissure"). All
  collide=False; beach's collider score verified unchanged (80.2 A).
- **KUIPER (the Blackstar Array)**: built the storyboard's Array aft of the
  flagship mast -- a shattered crystal sphere (jagged 9-shard riftcrystal
  cluster + fractured heart), deep-red lens beams piercing upward, and
  pale-blue Chicago transport-event fragments socketed into the surface, per
  the "Visualizing The Blackstar Array" section verbatim. One deliberate
  collider (the plinth, standable on the deck); 146 boxes, still under cap.
- **CHICAGO (the scar)**: a pillar-shard field around the memorial + one tall
  leaning flagpole shard with glow trapped in its tip -- the physical debris
  of the transport event the Array carries fragments of. Non-colliding;
  collider score verified unchanged (82.4 A).
- **CHARACTERS (ART_BIBLE rung 4)**: the shoulder was the last part slot
  still wearing a bare primitive (plain sphere). New robot_pauldron() --
  dome core + two stepped armor plates + rim ridge + triple vent slats
  (1065 verts) -- wired into part_template() with sphere/box fallbacks.
  Every bot in the game now wears layered film-style pauldrons at its most
  silhouette-defining slot.
HONEST: all of this is geometry-verified (vert counts, collider neutrality,
50/50 ctest) but look-unverified -- seam glow intensity, Array beam read at
distance, and pauldron proportions at gameplay camera are screenshot calls.
The bible's remaining direction (outer-rim defense lattice, weathering rung
5) is written but unimplemented.

## Update 210 -- Bible complete: the Solar Defense Grid + rung-5 weathering
Closed both remaining sections of docs/ART_BIBLE.md's first edition:
- **PLUTO (2.4, the Solar Defense Grid)**: the film's "massive
  Autobot-constructed weapons lattice" -- a 3x3 grid of thin truss beams
  16.5u over the station with energon weapon-node cubes at all nine
  crossings, plus the "retrofitted Cybertronian and human tech" read: bolted
  rust patch-plates and live energon seams on the plate_blue corner towers.
  All collide=False; pluto's collider score verified IDENTICAL before/after
  (85.1 A, 47 boxes) -- the lattice is pure sky.
- **CHARACTERS (rung 5, weathering)**: deterministic per-panel paint wear in
  robot_parts.c -- each part's coat scales 0.94-1.06x by a hash of part slot
  + alt-mode, so a body reads as plated panels with individual history
  instead of one molded coat, and each alt-mode family wears its own
  pattern. Deliberately placed BEFORE the two-tone and accent overrides, so
  accents stay clean: fresh paint over worn plate, the film read. Zero
  per-frame cost (computed in the same layout pass as everything else).
HONEST: weathering intensity (the 0.12 spread) and lattice beam thickness at
distance are look-unverified screenshot calls, same caveat as Update 209.
Rebuilt clean, 50/50 ctest green. The ART_BIBLE first edition is now fully
implemented; its listed futures (outer_rim/shield_edge retrofit extension,
faction decals needing texture work) are the next art frontier.

## Update 211 -- BOTS AS DATA: content/bots.json + the complexity pass
The characters are now authored the way the content layer always intended --
as JSON, not hardcoded geometry. Following the Blender CAD-reference
structure (coordinate system, materials, layered assemblies, joints,
robot<->vehicle transform map), content/bots.json specifies all five shared
part templates as component lists in four layers: frame -> actuator ->
armor -> detail. tools/gen_props.py gained a JSON assembly compiler
(bevel_box / cyl / sphere / limb_core / radius-relative ring + local-box --
preserving the Update-204 taper-relative rule -- with mirror/rows/cols/quad
replication). The old sculpt functions remain in the file as the reference
geometry the JSON was transcribed from; the JSON builds are what ship.
The transcription is a verified superset: every existing component carried
over 1:1, then each part gained a MECHANICAL sub-layer it never had --
the reference doc's core insight ("skeleton -> frame -> actuators -> armor
-> detail; a vehicle chassis whose components unfold into a machine"):
- phead 2144 -> 2628 verts: helmet crown plate, jaw hinge bolts, neck bearing
- pchest 1800 -> 2800: truck-grille bars, headlight nubs, spine strip, twin
  exhaust stacks, visible waist hydraulics
- ppauldron 1065 -> 1690: four bolt studs, under-skirt shoulder servo
- phand 900 -> 1084: wrist servo ring, knuckle guard plate
- plimb 469 -> 813: twin rear hydraulic pistons, calf vent slats
MAPS: new Builder.trim() -- one-call landmark dressing (four crown parapet
ridges + a lit service strip, all collide=False) -- applied inside existing
tower loops on vault (2 towers), core (2), pluto (4), glaciem (1). All four
maps' collider scores verified IDENTICAL before/after.
HONEST: same look-unverified caveat as 209/210 -- vert counts and collision
neutrality are proven, on-screen reads are screenshot calls. And the JSON is
the five SHARED templates, not yet per-bot variants (an optimus.json with
its own dims would need per-troop template routing in part_template()).
Rebuilt clean, 50/50 ctest green.

## Update 212 -- NAMED FRAMES: the variant system (prime + corsair)
Closed Update 211's honest limit: bots.json now has a "variants" section and
the engine routes named frames per body -- the reference doc's "Optimus G1 /
Movie / Heavy Armor from the same generator" made real.
DATA: each variant = base assembly - drops + adds, compiled by gen_props.py
into p<part>_<variant>.iqm alongside the shared templates:
- **prime** (heroic hero frame): tall crown fin + twin helm side fins + face
  mask on the head; twin CHEST WINDOWS (the reference doc's chest_windows,
  the truck identity worn proudly) + bumper band; a shoulder SMOKESTACK on
  the pauldron. 17/15/8 components.
- **corsair** (Blackstar warlord: "a body of welded wreckage, every plate
  torn from a rival's hull"): drops the crest + crown plate for twin HORN
  SPIKES and a wide visor slit; welded naval armor bands + collar spikes on
  the chest; a two-spike pauldron. 14/16/9 components.
ENGINE: part_template_var(i,var) routes head/chest/pauldrons per variant
with clean fallback to shared templates; swap_body_variant() re-frames an
already-spawned body using the exact remove/copy/add mechanic the signature
-backpiece swap established. Wired: the PLAYER wears prime at spawn; when a
warlord champion is promoted from the horde (the existing g_champion hook),
its body swaps to corsair on the spot -- the moment "the first grunt rises
as that warlord" now has a visible frame change to match its line.
Verified: all 6 variant meshes compile, engine builds clean (the only 2
warnings are pre-existing fread/system unused-result), 50/50 ctest green
including openworld (full init + mesh loading). HONEST: same screenshot
caveat as 209-211 for the on-screen read; and enemies besides champions all
still share the base frame -- per-troop routing through content/troops.json
(chassis -> variant column) is the natural next system.

## Update 213 -- FRAMES AS CONTENT: per-troop frame routing through the graph
Built Update 212's "natural next system": the frame is now a validated column
in the content graph, not two hardcoded engine swaps.
- **content/troops.json** gains an optional `frame` column; authored it on
  the prime player class (-> prime) and the six enemy warlord/variant troops
  (blackstar_raider, ravager, slipstream, ashfall_lancer, hoarfrost_sentinel,
  magma_breaker -> corsair).
- **content_build.py** loads bots.json and VALIDATES every frame reference
  against its variants list (verified: authoring an unknown frame now fails
  the content build with a clear error, same as any other bad reference).
- **codegen**: the compiler emits a new `ex_gen_frame[]` table into
  content_gen.h -- frame variant index per player class, in enum order
  (single source of truth: edit the JSON, the C table follows).
- **engine**: the player's frame is now data-routed from ex_gen_frame[class]
  instead of the hardcoded prime swap -- selecting the prime class in the
  loadout gives the prime frame; other classes stay base; all authored in
  JSON. (Champions still swap to corsair via the existing promotion hook.)
- **test_content_gen** extended to assert the frame table is well-formed and
  that the prime class routes to a real frame -- a broken frame column is now
  caught at test time, not discovered on screen.
50/50 ctest green (incl. the extended content_gen + openworld full-init).
HONEST: the enemy-side frame column is authored + validated but the horde
spawn path still frames rank-and-file enemies as base (only champions swap);
wiring each enemy troop's frame through the spawn path is the follow-on. And
the on-screen read stays a screenshot call, as with 209-212.

## Update 214 -- THE WATCH: a third frame + the horde wears its frames
Did all three open threads at once.
1. **Third frame -- watch** (the film's elemental god-form, "the 12+1"):
   authored in bots.json from the same generator -- a triple-crown halo of
   crystalline spikes + a third eye on the head, a radiant core ring/gem +
   shoulder mantle wings on the chest, a forked antler-crown pauldron. Drops
   the mundane crest/antennae for the halo. Compiles to 2544/3290/1750-vert
   meshes. Engine variant arrays widened 2->3; part_template_var guard now
   accepts index 2.
2. **Full per-enemy frame routing** (not just the one champion): new
   g_enemy_frame[] tracker mirrors g_enemy_sig's sync discipline -- the frame
   swap only re-copies the 4 variant slots (head/chest/L+R pauldron) when a
   slot's frame actually changes. Routing: champions get watch (elemental
   knd 4/5) or corsair (warlords 1/2/3); Blackstar raider grunts wear
   corsair. Resets to base on slot recycle (spawn_wave_makeup + fresh
   spawn_body). Removed the now-redundant promotion-time swap -- the
   per-enemy block owns all champion framing, routed from g_champion_kind.
3. **Watch reachable from the boss hook**: the existing knd==4 (The Watch
   rises) and knd==5 (Unicron) elemental-god promotions now put the boss in
   the watch frame automatically -- the "god-form" line finally has a
   god-form silhouette.
Codegen frame_idx + content validator + test_content_gen all widened to know
watch=2. 50/50 ctest green (content_gen, behavior, openworld full-init all
pass). HONEST: on-screen read still unverified (screenshot call, as 209-213);
the enemy-troop `frame` column in troops.json is validated but the horde
routes by engine kind/champion rather than reading that column per-spawn --
unifying those two paths (troop-column -> spawn frame) is the last seam.

## Update 215 -- Closing the seam: horde frames come from the data now
The one remaining seam from Update 214: enemy base frames were a hardcoded
engine constant while the troop `frame` column sat validated-but-unread.
Unified them. content_build.py now emits a second table, ex_gen_frame_body[]
indexed by ex_body_kind, whose values come from the troops' frame columns
(via frame_idx). The engine's per-enemy frame block reads its base from
ex_gen_frame_body[bk] instead of a literal -1; champion (watch/corsair) and
raider (corsair) upgrades still layer on top as the elite escalation.
Design call, made explicit in codegen rather than inherited: the frame
body-map is DECOUPLED from the styling body-map (whose CORRUPTED->ravager
borrow is a cinematic-read choice, not a frame choice). Baseline
grunt/corrupted/hijack stay on the shared base frame; only APEX -- the
standing elite/mini-boss tier -- wears a named frame (corsair) by default,
so rank-and-file read plain and elites read distinct: the correct hierarchy.
test_content_gen extended to assert the body-frame table is well-formed, that
the PLAYER slot carries no enemy frame, and that APEX wears a named one.
Now a designer changing a troop's frame column, or the APEX mapping, changes
the on-screen horde with no C edit. 50/50 ctest green (content_gen +
openworld full-init). HONEST: still screenshot-unverified visually (as
209-214). With all four frame paths (player class, champion kind, raider
caste, body-kind base) now sourced from content, the frame system is
feature-complete; what's left is pure look-tuning, which needs a render.

## Update 216 -- CLOSING THE VERIFICATION GAP: an offline silhouette renderer
For seven updates every art pass ended "look-unverified, screenshot call."
Built the thing that removes that caveat instead of stacking more blind
geometry: an offline silhouette renderer.
- **tools/pose_dump.c**: a tiny harness linking the engine's REAL
  ex_robot_layout_ex, printing each part's offset+scale as TSV -- so the
  transforms are the game's, not a reimplementation that could drift.
- **tools/silhouette.py**: reads that layout, loads the JSON-compiled part
  meshes (incl. frame variants), transforms verts to world space, and
  software-rasterizes a depth-shaded orthographic FRONT+SIDE view to PNG.
  Pure stdlib+zlib, no GPU. Now bot proportions are VISUALLY inspectable.
And then actually USED it -- rendered base/prime/corsair/watch and LOOKED:
the watch halo spikes and corsair horns were too THIN to read at silhouette
scale (r0 ~0.10-0.12). The "grander god-form" and "horned warlord" reads
weren't landing. Thickened them (watch halo r0 0.12->0.20/0.16/0.13 and
taller; corsair horns r0 0.10->0.17, +longer) and re-rendered to CONFIRM the
crown/horns now reshape the profile. This is the first art change this whole
arc made from evidence rather than intuition.
- **tools/test_silhouette.py** + CTest registration: guards the pipeline --
  builds the harness, asserts 17 valid parts x 6 body kinds, and that all 4
  frames composite >5000 tris at a plausible bot height. Suite now 51 tests.
The renderer is a static rest-pose silhouette (no gait, no shader/material/
lighting), so it verifies PROPORTION and PROFILE, not final surface look --
but that's exactly the layer the last seven updates couldn't check. 51/51
ctest green.

## Update 217 -- Silhouette verification swept across the whole cast
Now that the renderer exists (216), pointed it at everything it hadn't seen
and confirmed each reads correctly -- evidence, not assumption:
- **All 6 body archetypes** (grunt/player, corrupted, hijack, APEX, human):
  rendered and confirmed the robot_parts.c archetype reshaping actually
  lands -- APEX is a hunched hulk with huge pauldrons, human is slender and
  upright, base is balanced. The proportion system works as written.
- **Alt-mode remnants** (never visually checked before): extended pose_dump.c
  + silhouette.py with an --alt argument, then rendered car/dino/jet/tank.
  Confirmed the beast remnant's enlarged claws, raised spine ridge (clear in
  side view), and jutting snout all read; the siege remnant's wide flat helm
  + heavy pauldrons read distinctly different. The alt-mode reshaping from the
  earlier updates is verified for the first time.
- **test_silhouette.py** extended: asserts alt-mode remnants (dino, tank)
  measurably reshape the silhouette vs base without changing tri count (pure
  reshape, not add/remove). Guards the remnant path too.
Everything the renderer can reach -- 6 kinds x 3 frames x the alt-mode
families -- is now spot-checked and regression-guarded. 51/51 ctest green.

## Update 218 -- The surface layer, finally verified: color in the renderer
The silhouette renderer (216-217) checked proportion but ignored color -- the
last "shader-side, can't verify" frontier. Closed it by teaching the offline
path to read the engine's real per-part colors:
- **pose_dump.c** now emits each part's computed color[3] (the engine's base
  tint -> rung-5 weathering -> two-tone lower body -> accent-element parts,
  the full pipeline from robot_parts.c) alongside its transform.
- **silhouette.py** gained a --color mode: shades each triangle by its part's
  actual engine color instead of a flat blue, so the tint/weathering/two-tone/
  accent layers are now visually AND numerically inspectable.
Found + fixed a real bug in the process: colored mode first rendered nearly
blank (6 non-bg pixels) because the bounding-box/depth scans iterated over the
triangle's trailing rgb tuple as if it were a 4th vertex. Restricted vertex
iteration to the first 3 elements; re-rendered to a correct 343-distinct-color
image. Confirmed the tiers are right: accent parts brightest (~126,176,206),
weathered body panels mid, two-tone lower body darkest (~62,86,101 = 0.62x).
- **test_silhouette.py** extended: asserts colored mode yields >=4 distinct
  part tints and that the darkest (two-tone lower body) is <0.75x the
  brightest -- guarding the whole color pipeline.
Every layer the game builds a bot from -- proportion, profile, alt-mode
reshape, AND surface color -- is now offline-verifiable and regression-locked.
The renderer still omits gait, materials (metallic/gloss), and scene lighting,
which are true GPU-only; but the per-part color model is now proven. 51/51.

## Update 219 -- Test coverage for combat.c: the core hitscan math
Shifted to verification-independent work (no render needed) by testing the
untested. Audited which gameplay modules lacked dedicated tests and found the
most fundamental one uncovered: combat.c -- the ray-resolution EVERY ranged
shot in the game runs through. Wrote tests/test_combat.c pinning down the
geometry that's easy to get subtly wrong:
- ray/AABB (slab method): straight hit, miss-above, box-behind-origin,
  beyond-max-dist, ray parallel to a slab (inside vs outside the slab -- the
  fabsf(dir)<1e-6 branch), and a diagonal grazing corner hit.
- ray/sphere: near-root entry, offset miss, exact tangent, origin-INSIDE the
  sphere (must use the far/exit root -- the -b+sq fallback), sphere-behind,
  and out-of-range.
- combined raycast: nearest-of resolution (enemy behind cover -> cover hit
  first; enemy in front -> enemy hit first), dead targets skipped, impact
  point == origin+dir*dist, empty-scene EX_HIT_NONE at max range, and --
  the real gameplay mechanic -- a DESTROYED cover chunk no longer blocking,
  so a shot passes through the broken wall to the enemy behind it.
All pass first try (the math was sound; now it's locked). Suite: 52 tests.
This is the kind of core-mechanic coverage that catches a regression the
moment someone refactors the raycast -- independent of any render.

## Update 220 -- Two more untested systems covered: weaponmod + ability
Continued down the untested-module list with two player-facing systems, no
render needed.
- **test_weaponmod.c** (weapon addon balance math): slot-cap enforcement,
  stat multipliers stacking multiplicatively (amplifier 1.30 x energon 1.30),
  NULL-modset == base, all five fire-mode conversions, the BOLT->BURST
  promotion when a pellet mod is applied, and -- the important one -- every
  output clamp holding under adversarial mod stacks (three cooldown-cutters
  can't break the 0.08 floor; spread-adders can't pass 40deg; amplifier +
  plasma can't overrun the 3.0 cooldown ceiling). These are the invariants
  that keep any addon combo from producing a degenerate weapon.
- **test_ability.c** (ability cooldown/duration state machine): the full
  ready->trigger->active->cooldown->ready lifecycle stepped over dt, slot cap,
  double-trigger prevention while active, instant (duration 0) vs sustained
  abilities, active_any-by-id, and set/bounds guards.
Caught a REAL float-boundary property writing the ability test: 50 exact
0.1f steps from a 5.0s timer leaves it at +2.5e-6 (verified by probe), so the
active window hasn't quite closed on the exact boundary. Not an engine bug --
real gameplay uses variable dt and never lands exactly on the line -- but the
test now documents it (step a hair past, never assume exact boundary
landings). A good property to have pinned down.
Suite: 54 tests, all green. Untested gameplay modules remaining: enemy,
turret, vehicle.

## Update 221 -- vehicle + turret coverage: the untested-module list nearly cleared
Two more, both pure logic, no render:
- **test_vehicle.c** (21 alt-mode archetypes): name coverage, flies()
  self-consistency (documented flyers fly, ground types don't, every type
  returns clean 0/1), extra_parts sanity (0..8, 8-legged crawlers highest,
  tank lowest), and the handling stats' DESIGN INTENT -- every archetype has
  positive speed/accel and grip in (0,1]; the rooted SENTRY is slowest to
  accelerate and top out; the nimble CYCLE out-runs and out-accelerates the
  heavy TANK; walkers grip harder than the drifty CAR. Plus ex_vehicle_layout
  fills a valid in-cap part count that scales with the archetype (an 8-legged
  spider places more parts than a wheeled car: 11 vs 7), all parts sized.
- **test_turret.c** (the deployable auto-gun): deploy defaults + the +1 muzzle
  lift + spin-up delay; target pick (nearest ALIVE in range -- far/dead/empty/
  exact-boundary cases); the update lifecycle (no shot during spin-up, then a
  fire-rate-gated shot returning 1 and acquiring the target id, no back-to-back
  shots); no-target clears the acquisition; and lifetime expiry deactivating
  the turret so it never fires again.
Both passed first try. Suite: 56 tests. Of the original untested gameplay
modules only enemy.c remains -- it's the big stateful one (horde AI), a
natural next target.

## Update 222 -- WFC/FOC fan research + the shoulder-swap they loved most
Web-researched what fans actually praised about War for / Fall of Cybertron
(Metacritic, Steam, GameWatcher, Push Square, PopMatters, MP1st, the FOC
wiki), then mapped it against the build to fix a REAL gap, not rebuild what's
here. Findings + audit written up in docs/WFC_FOC_RESEARCH.md.
Most of the beloved mechanics were already present and deep: Escalation with
an energon buy-economy + map-expanding buy-gates, purchasable perks + a
weapon FORGE (Pack-a-Punch), 9 class abilities, and genuine vehicle-mode
combat (dino fire breath, spider web, scorpion cannons, velocity-scaled ram).
The one signature feature fans named repeatedly that we entirely LACKED: the
Fall-of-Cybertron **weapon-hand / shoulder swap** ("switch which hand holds
the weapon -- a great advantage turning a corner"). The cannon was hardcoded
to the right hand.
- Added g_wep_hand (+1 right / -1 left) toggled by Z, with a smoothed
  g_wep_hand_lerp so the cannon SWINGS across the body instead of popping.
  The weapon mount now blends between the right/left hand-part offsets; the
  muzzle flash already keyed off the weapon position so it follows for free.
  On foot only (guards out vehicle + human scale). Aim/shot origin unchanged
  (center) -- this is the peek-around-cover FEEL, exactly what fans valued.
Built clean, 56/56 ctest green. HONEST: the swap's on-screen feel is a
play-test call (like all render-side work); and it's cosmetic-tactical for
now -- docs/WFC_FOC_RESEARCH.md lists the follow-ons (air-snipers for "they
could be anywhere" tension, transform-dodge i-frames, a cover-side peek
bonus that makes the swap mechanically matter).

## Update 223 -- StarCraft 2 map-craft research applied to the maps
Researched pro SC2 map-making (Blizzard's official "Best Practices in Art and
Performance", Envato's SC2 Level Design series, Liquipedia, TerranCraft) and
distilled the engine-agnostic principles into docs/SC2_MAPCRAFT.md, then
applied the two most impactful ones the maps weren't yet embodying:
- **LEAD THE EYE** (Blizzard: trails tell players where to go, else the map is
  "a confusing mess"): new Builder.eye_trail() lays a line of lit floor
  markers from a hub toward a key area. Applied on pluto (command dais -> each
  of the 4 gates) and beach (down the pier deck -> the drained sea-line).
- **DETAIL IN THE MARGINS, GROUNDED** (Blizzard: keep gameplay unobstructed,
  put doodads in non-playable areas, and paint texture UNDER them so they
  don't float): new Builder.ground_cluster() places a SPACED cluster (rejects
  props within 2u -- the doodad-density/no-overlap rule) on a painted ground
  patch. Applied to pluto's 4 corners (tholin/abyssrock debris) and beach's
  inland margins (sand/rust beach-wrack).
Both helpers are collide=False by construction, so the dressing is pure
visual -- VERIFIED: pluto (85.1 A / 47 boxes) and beach (80.2 A / 99 boxes)
collider scores byte-identical before/after; mesh vert counts grew (pluto
5860, beach 37196) confirming the geometry rendered. map_score --check: all 8
maps still pass the quality floor. Rebuilt clean, 56/56 ctest green (incl.
openworld, which loads every map mesh).
HONEST: on-screen look is a screenshot call (geometry + collision-neutrality
are proven, the visual read isn't). docs/SC2_MAPCRAFT.md lists the researched
follow-ons not yet done: value-contrast elevation tiers for legibility,
extending the two helpers to the remaining maps, and a palette-readability
audit of the fighting space.

## Update 223 -- SC2 map-craft research: readable height + landmark reinforcement
Web-researched how StarCraft 2 mappers make artistic, readable terrain
(Blizzard's "Mastering Mapmaking", the Envato aesthetic tutorial, Hive
Workshop, SC2Mapster, community tip threads), distilled the principles that
transfer to our box-collider maps, and applied them. Full writeup in
docs/SC2_MAP_CRAFT.md.
The core SC2 lessons: height levels must READ (contrasting cliff colour
values so players see high/low ground), LEAD THE EYE with trails to important
areas, REINFORCE landmarks (dark ground under key objects so they pop), and
every placement has a MOTIVATION (never random).
- New Builder.tier_pad(): a raised collidable deck + a contrasting glowing RIM
  around the top edge (the SC2 height-readability rule) + a darker ground
  decal ringing the base (the landmark-reinforcement rule). Encodes three
  principles in one call.
- enceladus (our flattest map): two raised survey tier-pads with riftcrystal
  rims as genuine high-ground over the ice + an eye_trail to the outpost.
- io: the central forge-plinth rebuilt as a readable tier_pad (hazard rim so
  the raised forge reads, dark obsidian decal so it pops from the sulfur) with
  two eye_trails leading inward to it.
All dressing is collide=False reusing existing textures (no overdraw); the
tier_pad decks collide but map scores held (io 81.8 A, enceladus 80.9 A, all
8 terrain maps above the quality floor). Rebuilt clean, 56/56 ctest green.
HONEST: the map scorer measures gameplay geometry, not aesthetic readability,
so these wins are score-NEUTRAL by design -- the value is on-screen (a
screenshot/play-test call). And not every flat map should be forced vertical:
enceladus is thematically an ice field, so we added readability, not fake
height. docs/SC2_MAP_CRAFT.md lists extending the landmark treatment to other
maps' key objects as the next pass.

## Update 224 -- SC2 map-craft follow-ons: value-contrast + 3 more maps
Executed the follow-ons logged in Update 223 / docs/SC2_MAPCRAFT.md.
- New **Builder.height_bands()**: the researched value-contrast-elevation
  principle (Blizzard: height tiers need contrasting color VALUE so players
  read high vs low ground). Our mesh has no per-vertex colour, so it applies
  value contrast via MATERIAL -- rings a structure with thin lit bands whose
  texture steps dim->bright with height, so elevation reads as brightness.
- **vault**: eye-trails from all 4 map edges inward to the central relic
  plinth; value-bands up the two archive towers (spec_rock -> riftcrystal ->
  white); grounded shard clusters in bare margins.
- **core**: value-bands up the two flow towers (obsidian -> rust -> molten,
  matching "heat rises"); grounded slag clusters in 3 corners. (Its molten
  energon rivers already lead the eye to the forge, so no trail needed.)
- **chicago**: eye-trails to both river-bridge crossings (the key routes);
  grounded reconstruction-rubble clusters in the far-south margins.
VERIFIED collision-neutral: vault (85.2 A/86), core (84.0 A/76), chicago
(82.4 A/64) collider scores byte-identical before/after; mesh vert counts
grew (dressing rendered); map_score --check: all 8 maps pass floor; rebuilt
clean, 56/56 ctest green (incl. openworld loading every mesh).
Caught + fixed a mistake mid-edit: my first chicago str_replace inserted a
premature b.write() in the MIDDLE of the map (before the fallen-tower /
reconstruction / memorial content), which would have truncated it -- moved
the additions to the real end and confirmed a single chicago write remains.
HONEST: on-screen look is still a screenshot call. Remaining SC2 follow-ons
(doc): extend the helpers to the ice/space maps, and a palette-readability
audit of the fighting space.

## Update 224 -- Destiny-informed map design: varied + interactive + legible
Pushed past the SC2 competitive-RTS advice (right that it's only half the
picture for an open-world 3rd-person combat game) into Destiny/open-world
craft. Researched Bungie env-design coverage + Max Nichols' ex-Bungie
encounter-design writeup (owning space, anchors that project danger,
directing attention, and combat LEGIBILITY). Full findings appended to
docs/SC2_MAP_CRAFT.md.
- New Builder.varied_scatter(): the old ground_cluster repeated ONE prop
  (the monotony flagged); this mixes a LIST of prop meshes at varied
  sizes/footprints/caps with enforced spacing (Destiny legibility -- no
  overlap clumps) and makes a fraction SOLID destructible-style cover
  (interactivity -- weave through it, use it), not just decoration.
- wastes (scrapyard): three varied_scatter clusters of mixed scrap
  (rust/wall/brushmetal/plate/rock), ~40% solid -- a flat rubble field becomes
  a varied, interactive, legible junk battlefield for patrol. Score 85.0 A.
- New tools/map_view.py: a top-down height-shaded collider renderer so map
  layouts can be INSPECTED for legibility/spacing (the map analogue of the bot
  silhouette renderer). USED it to verify wastes reads as well-spaced with
  clear lanes and distinct cover -- not guessed.
Rebuilt clean, 56/56 ctest green (map_score + openworld pass).
HONEST: the biggest Destiny idea -- owned-space front/middle/back firing
zones -- is a horde-AI change, not a map-builder change, so it's deliberately
NOT in this pass; it's flagged as the highest-value future direction. And
"destructible cover" here means solid colliders used tactically; full
shoot-to-destroy on themed-map props needs wiring the .col loader into the
existing arena material/chunk system -- a clean follow-on. varied_scatter is
on wastes so far; rolling it across io/derelict/driftfield is the next sweep.

## Update 225 -- varied_scatter refined: solid props are real eye-height cover
Continued the Destiny map-craft thread by improving the varied_scatter helper
and learning where it does/doesn't fit.
- Refined varied_scatter: SOLID props are now proper eye-height cover you hide
  behind (height 1.0-3.0, tops reach the 1.5 sightline-break line) with a real
  footprint, while non-solid props stay low decorative debris you shoot over.
  This makes "solid" mean tactical cover (better design) and reads correctly
  on the top-down map_view renderer.
- wastes holds a strong 84.2 A with the improved cover; verified via map_view
  that it reads as a well-spaced field (~12% collider coverage, open lanes
  preserved -- Destiny legibility, not clutter).
HONEST / a disciplined negative result: I tried rolling varied_scatter onto
io (sparse volcanic field) and it REGRESSED the score 81.8 A -> ~79 B. io's
original 16 low spread obsidian boxes genuinely scored better -- the map's
large sparse layout + the scorer's density/balance metrics preferred them.
Rather than overfit tuning to force a worse result, I REVERTED io to its
original scatter. varied_scatter is a clear win on dense, monotone-prop maps
(wastes scrapyard) but not on sparse fields with existing tall-structure
contrast (io). Lesson recorded in docs/SC2_MAP_CRAFT.md: apply the helper
where the map has repeated-single-prop monotony, not everywhere.
Rebuilt clean, 56/56 ctest green; all 8 terrain maps above the quality floor.

## Update 225 -- Game-feel: hitstop + hit markers (surfacing the depth)
Direct response to playtest feedback: the systems are deep on the backend but
the moment-to-moment PLAYING experience read as lower quality -- the depth
wasn't surfacing. Started the feedback-layer work that makes existing depth
FELT. (docs/GAME_FEEL.md)
- **Hitstop**: a heavy hit briefly crunches time. hitstop(strength) sets a
  freeze timer that ticks on REAL dt while scaling the GAMEPLAY dt everything
  downstream uses -- a KILL near-freezes (~0.09s, squeeze->~0), a solid
  non-killing hit gets a tiny weight-bump (~0.05s), throttled with the
  existing damage-tick gate so sustained/rapid fire and DoT don't stutter.
  Camera + mouse-look run on real dt ABOVE the squeeze, so aiming stays
  responsive through the freeze. The single highest-leverage "juice"
  technique -- it's what makes a hit read as connecting.
- **Hit markers**: the center crosshair flashes from "+" to a bright white
  "X" on a landed hit and a hot-orange "X" on a kill (decays on real frame
  time so the flash length is stable even inside a hitstop). Crisp shot
  confirmation the game had entirely lacked.
Order verified: g_hitstop decrements with the REAL dt before the scale, so
the freeze self-terminates in real time regardless of squeeze. Forward-
declared the two hitmark statics so enemy_damage_fx (defined early) can set
them. Built clean, 56/56 ctest green.
HONEST: both are math/timing-verified but the on-screen FEEL is a play-test
call (I can't see/feel the running game -- this is exactly the blind spot the
playtest addresses). Tuning knobs: hitstop squeeze/duration, marker flash
length. docs/GAME_FEEL.md lists the next feedback layers (floating damage
numbers, directional damage vignette, streak-kill VFX escalation, per-weapon
recoil curves, a HUD-hierarchy pass so the important beat always wins the eye).

## Update 226 -- Round + hollow map geometry with fitted collision
Addressed the "better collisions for round and hollowed geometries of big map
elements" feedback. Researched pro collision approaches (capsule-vs-triangle
character controllers with AABB-tree broadphase) and confirmed the pragmatic
consensus (computerBear/Wicked Engine): a box is cheaper + more robust than
the 12 triangles it's made of, so full mesh collision is only worth it where
curvature is actually felt. Per the user's steer ("precise round collision
only when needed -- driving, bullets -- not on foot"), chose a TIERED design
rather than a full capsule controller:
- New Builder primitives: _prism (N-sided vertical prism mesh), cyl_tower (a
  genuinely ROUND tower: prism mesh + an INSET RING of collider boxes hugging
  the circle, so no invisible square corners poke past the visible edge --
  verified the ring reaches only 3.64u for r=4 vs a naive box's 4.0+ corner),
  tunnel (a HOLLOW passage: two side walls + a solid roof, OPEN bore you walk/
  drive through), and dome (stacked shrinking prism rings + curve-following
  collision, or collide=False for decorative shells).
- The inset box-ring is the elegant part: because every system (bullets,
  camera boom, vehicles, player step-resolve) already tests boxes, they ALL
  get the rounder profile for free -- raycasts included -- with no separate
  code path. Full triangle collision would be marginal gain for real cost,
  exactly what the research cautioned against.
- Converted THREE maps: cybertron (a third of the skyline towers -> round
  cyl_towers; the two monumental gateways -> walkable tunnels), helix (organic
  groves -> decorative bio-domes), enceladus (ice arches -> walkable ice tubes
  over the fissures). Scores held: cybertron 86.9 A, helix 84.2 A (up from
  84.0), enceladus 80.0 B; all 8 gated terrain maps above the quality floor.
- Disciplined tuning: a first pass made helix's domes + dais fully collidable
  and it regressed to 75.4 B (collider bloat flattened verticality); made the
  groves decorative and kept the dais solid -> back to 84.2 A. Visual win, no
  gameplay cost.
Also fixed a pre-existing build break: a prior turn's encounter-zone edit had
dropped the ex_encounter_targets function signature, leaving a headerless
block. Restored it. Rebuilt clean, 56/56 ctest green.

## Update 227 -- encounter.c wiring: the Destiny owned-space zones actually work now
Finished wiring the Destiny front/middle/back "owned space" zone system that
had been sitting partly-dormant in encounter.c. Traced it end to end and found
it was structurally complete -- ex_encounter_update calls assign_zones ->
ex_enemy_update (which DOES read en->standoff to hold at zone distance) ->
update_zones, and the game's enemy-physics layer derives velocity from the
AI's intended move, so a held enemy correctly decelerates. BUT the core
mechanic was silently broken:
- BUG: assign_zones re-derived every enemy's zone from its CURRENT distance
  every single frame. So the collapse-advance computed by update_zones (the
  central "claim space" payoff: clear a line, survivors behind advance) was
  instantly overwritten the next frame. The advance never stuck -- the whole
  point of the system did nothing.
- FIX: made zones STICKY. assign_zones now only sets an INITIAL zone (once)
  for enemies marked unassigned (zone < 0, set at spawn); after that the zone
  persists and is changed only by update_zones' collapse-advance. Every spawn
  path routes through ex_encounter_spawn, so wave reinforcements correctly
  join at their spawn-distance line and then hold/advance.
Proven with new zone assertions in test_encounter: zones assign by distance
(front/mid/back = 3/3/3), a back enemy yanked forward in space KEEPS its back
zone (sticky), and after wiping the front line 3 survivors ADVANCE into the
front zone (the collapse-advance now actually takes effect). 57/57 ctest green.
Result: enemies now hold front/mid/back lines at standoff distances (4/16/30u)
instead of all swarming the player -- back-line anchors project danger from
range, the front brawls close, and clearing a line pushes the fight forward.

## Update 228 -- enemy engagement ranges: they shoot back now (+ a clobber bug fixed)
The user was right: I had NOT touched enemy ranges (the zone work only changed
HOLD distance). The underlying detection/engagement ranges were tiny -- the
standard GRUNT had sight 40 / attack 22 while the player's weapons reach ~290-
470u effective, a 7-10x advantage. You could poke enemies freely from just
outside their bubble.
- Raised GRUNT to sight 140 / attack 90, HIJACK to sight 130 / attack 70, so
  ranged enemies notice + open fire from real distance instead of being free
  targets. Also separated "where I stop walking" (zone standoff) from "how far
  I can shoot" (attack_range) in enemy.c's CHASE logic, so a back-line anchor
  holds at 30u but still fires at range instead of walking in silently first.
- Writing the test for this surfaced a PRE-EXISTING bug: the APEX stat block's
  catch-all `else` was firing for CORRUPTED and HIJACK too (they aren't APEX),
  overwriting the melee-hunter + swarm profiles those kinds set in their own
  if-blocks with grunt ranges. So corrupted enemies had been getting sight
  140/attack 90 instead of their 1e9-sight melee-hunter profile. Fixed to an
  explicit GRUNT-only block; each kind's stats now stick.
- New tests/test_enemy.c -- the FIRST dedicated test for enemy.c (the last
  untested gameplay module): engagement ranges (proving the fix), the
  IDLE->CHASE->ATTACK->DEAD lifecycle, out-of-sight staying idle, and the
  corrupted/apex hunt-from-anywhere behavior (which is what caught the clobber
  bug). 58/58 ctest green.
Result: ranged enemies engage from distance and return fire; corrupted are
proper long-sight melee hunters again; no more free poking from outside a tiny
bubble.

## Update 229 -- combat_json: a JSON boundary for (future) LLM/Blackstar integration
Built the clean JSON seam the user wants for later LLM roleplay -- Blackstar
reading how the player engages and issuing "funky" directives back -- as a
self-contained, dependency-free, fully-tested module (combat_json.{h,c}).
Two directions, both implemented + tested:
- SNAPSHOT (game -> brain): serializes the combat experience -- player, allies
  FOLLOWING the player (pos/hp/mode/target), and troops/enemies with their
  owned-space ZONES + kind/element, plus the ENGAGEMENT SIGNAL input variables
  a roleplaying brain reacts to: aggression, pressure, recent_dmg, kills,
  enemies_alive, player_hp_frac. ex_cj_signal_tracker smooths per-frame fire/
  closing/near-count into 0..1 aggression+pressure; ex_cj_build_snapshot fills
  a snapshot from the game's parallel arrays in one call.
- ORDERS (brain -> game): flock/move directives (flock_to, flock_spread, hold,
  focus, retreat) + ally_follow, each targeting a group by zone (or all). The
  parser is deliberately TOLERANT -- a chatty LLM reply with prose, unknown
  keys, and unknown verbs still decodes the valid commands, with per-command
  field windowing so values don't bleed between commands. Observe-only replies
  (no commands) are valid.
The loop is COMPLETABLE: game arrays -> snapshot -> JSON -> (brain) -> parse ->
commands, all covered by tests/test_combat_json.c (round-trip, tolerant messy-
reply parse, live tracker climbing/falling, snapshot-from-arrays). Caught +
fixed a real parser bug: the commands-array end was found with strchr(']')
which stopped at an inner point-array bracket, truncating later commands;
replaced with bracket-depth matching. Schema + intended integration documented
in docs/COMBAT_JSON.md. 59/59 ctest green.
Nothing is wired into the live game loop yet (by design -- this is the format +
codecs); the commands map cleanly onto existing systems (focus/hold/retreat =
zone standoff changes, flock_* = rally points over the existing boid
separation, ally_follow = g_ally.mode), noted for the integration pass.

## Update 230 -- commanders: killing leaders scatters troops (a deterministic pre-LLM layer)
Per the user's steer, added a deterministic "commander" tactical layer before
the LLM Blackstar work -- it gives directed-squad texture and the satisfying
payoff of decapitating a squad, and slots into the zone system + combat_json
seam already built.
- New enemy fields: is_commander (leads a squad), squad (leader id a troop
  answers to, -1 independent, defaulted so a zeroed field doesn't bind to
  commander id 0), scatter_t (leaderless panic countdown), demoralized.
- ex_encounter_update_command (run each frame after the zone update): when a
  squad's commander dies, its surviving troops SCATTER; a squad that still has
  a live commander stays disciplined; a squad that finishes panicking with no
  commander is left demoralized.
- Behavior wired into enemy.c CHASE: a scattering troop FLEES the player
  erratically and won't fire (formation visibly breaks); a demoralized troop
  holds ~18u wider standoff (timid, disorganized) instead of a coordinated line.
- ex_encounter_spawn_squad convenience: spawns a commander + a ring of troops
  bound to it, so killing that commander scatters exactly that group.
- Exposed is_commander/squad/scattering in the combat_json unit snapshot, so
  the future Blackstar brain can see squad structure and reason about
  decapitation tactics.
Proven in test_encounter: a commander + 5 bound troops are disciplined while it
lives, all 5 scatter the frame it dies, and all 5 survivors become demoralized
after the panic window. Fixed the ex_encounter_targets signature (dropped again
during insertion, same headerless-block gotcha as U227). 59/59 ctest green.
NOT yet wired into live game spawns (by design -- which encounters get
commanders + commander body models is a deliberate design choice, not an
end-of-turn bolt-on); the mechanic + spawn helper + JSON exposure are complete
and tested, ready to drop into chosen encounters.

## Update 231 -- commander doctrine corpus + FSMs (the pre-LLM brain layer)
Built the layered command architecture the user sketched: military doctrine ->
distilled archetype corpus -> deterministic FSMs now -> (future) LLM brains for
Blackstar/Prime, all speaking ONE command vocabulary (the combat_json verbs) so
FSM + LLM commanders are interchangeable behind one interface.
- Sourcing/copyright: doctrine drawn from Sun Tzu's Art of War in the PUBLIC-
  DOMAIN 1910 Lionel Giles translation (confirmed via Gutenberg/Standard
  Ebooks/Wikisource). Did NOT store raw passages or touch the copyrighted modern
  translations (Griffith/Delphi). Raw quotes make flavor not behavior, so each
  archetype is doctrine DISTILLED into params + triggers, with at most a short
  attributed Giles fragment as an optional callout.
- content/commanders.json: 6 principles (concentration/envelopment/evasion/
  tempo/baiting/cohesion) distilled into 5 archetypes (aggressor/surrounder/
  skirmisher/baiter/warden), each a param set (aggression/caution/encircle/
  regroup) + flavor. Validated: archetypes reference known principles, params in
  range, order + values match the C built-ins exactly.
- commander_ai.{h,c}: deterministic doctrine FSMs. ex_commander_think runs on a
  ~1.6/s cadence, surveys its own squad in the combat_json snapshot (alive/
  losses/hp/centroid), picks a posture (PROBE/PRESS/ENCIRCLE/FALLBACK/RALLY)
  from doctrine + battlefield, and emits the SAME ex_cj_command_t the LLM will
  (PRESS->focus, ENCIRCLE->flock_spread, FALLBACK->retreat, RALLY->flock_to,
  PROBE->hold). Dead-leader/wiped-squad -> no orders (ties into the scatter
  mechanic). This is the key design payoff: an FSM commander and an LLM
  commander are drop-in interchangeable.
- test_commander_ai proves archetypes are behaviorally DISTINCT from the same
  snapshot: aggressor presses, surrounder encircles with numbers, warden holds
  fresh / rallies hurt, skirmisher disengages hurt, baiter dangles bait then
  springs to focus when the player over-commits; plus think-cadence gating and
  dead-commander silence. 60/60 ctest green.
- docs/COMMANDER_AI.md maps the full corpus->archetype->FSM->LLM roadmap
  (Blackstar moods, Prime roster-aware life-preserving play) and the wiring
  status.
NOT yet wired to live spawns (same deliberate integration-pass boundary as the
commander-spawn work); FSM output applies through the same path the LLM orders
will (focus/hold/retreat=zone standoff, flock_*=rally points, ally_follow=
g_ally.mode).

## Update 232 -- infected hive: a sneakier swarm decision engine
Added a decision engine for the infected/Corrupted that's distinct IN KIND from
the military commander FSMs, per the user's steer (crawl + organize before
feeding, waggle-dance to allies). Where a commander issues open top-down orders,
the hive is patient + indirect -- swarm intelligence:
- hive_ai.{h,c} with a 3-state swarm machine:
  - CRAWL: below critical mass the infected stay dispersed + creep, massing
    quietly (emits a loose hold) so a lone scout doesn't give the swarm away.
  - WAGGLE DANCE: a scout that gets close to prey recruits nearby infected
    toward that point (emits flock_to the scout). Recruitment spreads
    stigmergically by proximity (bee-inspired), not a broadcast order.
  - FEED: once enough answer the dance (critical mass = 4 by default), the swarm
    tips + converges (focus). Thinned below mass -> disperses back to crawl.
- Reads infected from the combat_json snapshot (units of kind CORRUPTED), and
  emits the SAME ex_cj_command_t vocabulary as commander_ai + the future LLMs,
  so hive/commander/LLM brains are all interchangeable behind one interface --
  the hive just reaches those commands by a sneakier policy.
- test_hive_ai proves the crawl->dance->feed progression: a far swarm crawls
  quietly, a close scout starts dancing (flock_to, recruits counted), critical
  mass tips to feed (focus), a thinned swarm disperses back to crawl, plus think-
  cadence gating. 61/61 ctest green.
- docs/COMMANDER_AI.md updated with the hive section contrasting it with the
  commanders.
NOT yet wired to live spawns (same integration-pass boundary); output applies
through the same path as commander/LLM orders.

## Update 233 -- infected hive: multiple sneaky STRAINS (variety)
The base hive engine (crawl->waggle-dance->feed) already landed in U232; this
adds the VARIETY the "engines" (plural) ask implies -- distinct sneaky styles,
the way commanders have five archetypes. Each strain is a doctrine preset over
the existing crawl/dance/feed skeleton (no new state-machine code), so they FEEL
different without duplicated logic:
- STALKER: patient ambusher -- lurks long (only dances when nearly on top of
  prey), tight compact pack, sudden feed.
- CREEPER: slow encircler -- dances from farther out, waits for a BIG mass over
  a wide recruit radius before committing (surrounds before it feeds).
- LURE: a few dance EARLY and from range to draw prey toward the pack; small
  feed mass so once prey's close the rest tip fast.
- RAVENER: impatient -- short lurk, tiny mass, quick to swarm (barely sneaky).
New: ex_hive_strain enum + ex_hive_strain_name/from_name + ex_hive_doctrine_for
+ ex_hive_init_strain, and a strain field on the runtime struct.
test_hive_ai extended + proves the variety is REAL not cosmetic: a ravener FEEDS
on a 3-infected swarm where a creeper is still only RECRUITING, and at 26u a lure
already DANCES to draw prey while a stalker still LURKS unseen. 61/61 ctest green.
(Base mechanic was a duplicate ask; flagged it and delivered the additive part.)

## Update 234 -- controls page: document the hidden ability keys
The controls screen only listed the 10 rebindable movement/basic actions
(forward/left/back/right/jump/sprint/crouch/transform/fire/reload), but the game
had grown a whole set of combat + traversal keys that were HARDCODED and
undocumented -- players had no way to discover them. Added a read-only ABILITIES
reference panel to the controls page:
- ABILITIES: Q ground slam, Z swap gun hand, SHIFT vehicle boost, E interact,
  G grapple (raid).
- FORM SPECIALS (element-gated): C cyclone (air), X thorns (vine), F arc
  (electric), V tsunami (water).
Layout: the VGA grid is 32 cols x 16 rows. Tightened the rebindable list's
format ("%-7s %s" + key names clamped to 8 chars, so the left column ends by
col 20) and placed the reference panel at col 21, rows 1-12, clear of the left
list and the row-2 subtitle. These specials are form-gated + hardcoded (not in
the rebind enum), so a reference is the right call vs. rebindable rows. Build
clean, 61/61 ctest green. (UI render -- verified by layout math + build, not an
on-screen capture; the visual fit is a play-test confirmation.)

## Update 235 -- FIX: match-start freeze (infinite angle-wrap loop on non-finite input)
Diagnosed the reported hang. The gdb backtrace (parked in Wayland_GLES_SwapWindow
-> wl_display_flush -> futex_wait) was a RED HERRING -- that's just where a
vsynced frame blocks; the real freeze was a game-side infinite loop that stopped
feeding frames, leaving the last one on screen.
Root cause: the angle-normalization idiom used ~8 times in game.c --
  while(a>PI) a-=2PI;  while(a<-PI) a+=2PI;
spins FOREVER if `a` is +/-Inf, because Inf minus a finite value is still Inf.
Some aim/heading angle (turret tracking t->angle, or body yaw) went non-finite
and locked the frame at match start.
Fix: added ex_wrap_pi() -- a bounded fmodf-based wrap that returns 0 for any
non-finite input, so a corrupted angle can never lock a frame. Replaced every
unbounded while-wrap in game.c (turret aim x2, body-yaw x4, plus two others)
with it. Added <math.h> for isfinite/fmodf.
Verified with a standalone test: ex_wrap_pi is safe + terminating for Inf/NaN
(returns 0) and correct for normal values, AND the old loop is confirmed to hang
on +Inf (hit a 100k-iteration cap). Full game builds clean, 61/61 ctest green.
NOTE: this hardens every wrap SITE so the game can't freeze there again
regardless of source. The upstream reason an angle went non-finite (likely a
zero-vector normalize / divide somewhere feeding a yaw) isn't independently
traced -- if a specific unit still visibly misbehaves, that's the next thread,
but the freeze itself is now impossible.

## Update 236 -- maps: verticality pass on the three flattest maps
"Keep working on the maps." Surveyed all 33: mean 81.3, and the weak axis on the
low scorers was consistently VERTICALITY (the scorer measures std-dev of
collider top-heights, ~6u spread = full). enceladus (the only B, 80.0) plus luna
(80.1) and roots (80.1) were all dragged down by flat fields of uniform low
cover. Fixed all three thematically -- the lesson: a few genuinely TALL
landmarks stretch the height distribution far more than many mid bumps.
- enceladus 80.0 B -> 88.0 A (vert 0.43 -> 0.86): geyser mounds became towering
  tiered ICE PLUMES on the fissure lines; tiger-stripe ridges raised into fissure
  WALLS; shard heights widened (3-16u); + 3 tall round ice-column landmarks
  (cyl_tower 14-17u).
- luna 80.1 -> 88.2 A (vert 0.44 -> 0.85): crater rims raised into real walls so
  craters read as pits; sheared pinnacles became towering round columns (15-16u).
- roots 80.1 -> 94.3 S (vert 0.45 -> 0.93): grown pillar-trunks became towering
  round organic columns (15-16u); spore chimneys taller. First S-grade map.
Verified each with map_view renders (readable high-ground landmarks, center
stays open -- not cluttered). All 8 gated terrain maps still above floor; set
mean 81.3 -> 82.2. Full game builds clean, 61/61 ctest green. Reused the
cyl_tower round-column primitive from U226, so the tall landmarks also carry the
round-matched collision.

## Update 237 -- maps: verticality pass continued (beach, reclaim)
Continued the verticality work on the next two low scorers, refining the method
from U236. A useful diagnostic emerged: dump the collider top-height histogram
first, because "low vert" has two distinct causes needing different fixes.
- beach 80.2 -> 83.0 A (vert 0.47 -> 0.61): the tall city towers were all walled
  off at the x=76 edge, leaving the whole center/inland FLAT (confirmed by
  render). Added a distributed field of collapsed-building RUINS across the empty
  play space. First pass with mid-height stumps barely moved it (they fed an
  already-huge mid-height pile); switching to BIMODAL ruins (tall 9-18u husk +
  low rubble slab, no mid) widened the std-dev properly. Thematically a drowned
  city would have broken structures everywhere, and it puts high-ground cover
  where the fight is. Left the pier/PCH/seam low detail intact (intentional).
- reclaim 80.7 -> 88.0 A (vert 0.51 -> 0.87): it already had a "verticality pass"
  but every tall feature capped at ~9.6u, so the range was compressed (good
  distribution, no genuine height). Raised the regrowth spires to 12-19u and
  added two tall round regrowth-tower landmarks (cyl_tower), stretching the top.
Verified both with renders (ruins fill beach's dead center; reclaim's towers
frame the lanes -- readable, spawn stays clear). All 8 gated terrain maps above
floor; set mean 82.2 -> 82.5. Build clean, 61/61 ctest green.
Method note for next time: histogram the tops. A low MAX means "add taller
landmarks"; a fat MID pile means "go bimodal / thin the middle". Two different
fixes for the same 0.5-ish vert score.

## Update 238 -- maps: cover fix (dreadnought) + verticality (io, emberpit)
Kept working down the low scorers, and this turn the histogram-first method paid
off by revealing the lowest map needed a DIFFERENT fix than verticality.
- dreadnought 80.3 -> 84.5 A (cover 0.69 -> 0.85): NOT a vert problem -- it was
  86% high cover (ideal ~55%) and low-density (37 pieces over a 160x150 deck).
  Expanded the low deck-clutter scatter (5 -> 16 low shoot-over pieces: crates,
  hatches, tie-down blocks). Raised density AND pulled the high/low balance
  toward ideal. A warship deck is covered in this gear anyway.
- io 81.8 -> 93.8 S (vert 0.55 -> 1.00): low-dominated (26 low / 12 mid / 5 high,
  max only 15). Raised the sheer mountain blocks to Boosaule-class 10-18u (Io has
  the solar system's tallest peaks -- thematically exact) and gave volcano cones
  a SOLID tall vent core (the plumes were decorative/non-colliding). Now S-grade.
- emberpit 82.7 -> 83.2 A (vert 0.50 -> 0.55): DELIBERATELY modest. Its 72 stepped
  obsidian tiers are the fat mid-pile but they ARE the amphitheater's identity;
  maxing vert would mean bulldozing the map's signature, which is bad design.
  Added a jagged crown of more rim spires (better silhouette) and accepted the
  small gain -- a case where the metric shouldn't be maximized.
All 8 gated terrain maps above floor; set mean 82.5 -> 83.0. Build clean, 61/61
ctest green. Reinforces the method: histogram tops AND check the cover balance --
low vert vs low cover vs a signature-mid-pile are three different situations.

## Update 239 -- bot design: the Corrupted now reads as WRONG (asymmetry pass)
Worked on bot design. Rendered the whole roster (base + prime/corsair/watch
variants + all 6 body kinds) via the offline silhouette tool and verified via raw
pose_dump diffs that the per-kind silhouettes ARE genuinely differentiated
(apex chest scale 1.12 vs player 0.59, hunched + dark; hijack small/spindly with
a bulbous head; etc.) -- my first-glance "they look identical" was wrong, misled
by small thumbnails; the engine's per-kind reshaping is solid.
The one real weakness: the CORRUPTED read as a hunched BRUISER, not something
infected/WRONG. The code had asymmetry (big arm / withered arm) but it was too
subtle to land. Strengthened it into a genuine infected silhouette:
- one bloated dragging arm (sx*1.85 sy*2.0) vs a withered stump (sx*0.55)
- a TWISTED torso line: heavy shoulder sags, light shoulder hikes up
- legs splay lopsided both directions; head lolls harder aside
- the over-grown back mass shoved off to one side
Verified with the silhouette render (now clearly lopsided + wrong, distinct from
the symmetric armored Apex). Isolated to the CORRUPTED kind block -- player
re-render confirms it's unchanged (still clean + symmetric).
Added test_bodykind.c locking in the roster's read: player arm-asymmetry 0.000
(symmetric), corrupted 1.791 (>>4x player -- the infection tell), apex 1.6x
player height, hijack shorter. Guards against a refactor silently flattening the
kinds to one body. Build clean, 62/62 ctest green.
NOTE: verified in offline silhouette (proportion/pose/asymmetry), not shaded
on-screen -- the in-engine look with materials + animation is a play-test call.

## Update 240 -- bot design: the Corrupted infection now VISIBLY PROGRESSES by stage
Continued bot design on the thread flagged last turn: the Corrupted tracked an
infection stage (1..3) in the enemy struct, and the game escalates it, but the
BODY always rendered at one fixed distortion -- a just-turned Corrupted looked
identical to a fully-consumed one. Fixed:
- Added corrupt_stage to ex_build_params (defaults 0 via the positional init, so
  existing callers unaffected).
- The Corrupted distortion block now scales EVERY asymmetry amount by a stage
  factor sf: 0.40 at stage 1 (subtly wrong, freshly turned) -> 0.70 stage 2 ->
  1.00 stage 3 (the full lopsided horror from U239). 0/unknown -> 0.85 mid-heavy
  default so unstaged corrupted still read wrong.
- Wired en->stage into the game's enemy build_params for Corrupted.
- Threaded a stage arg through pose_dump + silhouette.py so the progression is
  renderable/inspectable offline.
Verified numerically: arm asymmetry climbs monotonically 1.135 (s1) -> 1.463
(s2) -> 1.791 (s3). Extended test_bodykind to LOCK the escalation (asserts s1 <
s2 < s3 and even s1 > player). Caught + fixed a stub-signature break in the
silhouette test from the new pose_dump arg. Build clean, 62/62 ctest green.
NOTE: offline-verified (proportion/asymmetry by stage), not shaded on-screen --
the in-motion look with the corrupted twitch anim is a play-test call.

## Update 241 -- WIRED LIVE: commanded squads in real matches (the AI arc goes playable)
"Keep working on the game." Surveyed the changelog: the last big arc (U229-233)
built a whole enemy-AI stack -- combat_json seam, commander scatter, commander
FSMs, hive engine + strains -- and EVERY update ended "NOT yet wired to live
spawns". Confirmed: grep found ZERO references to any of it in game.c. So the
highest-value move was the deferred integration pass: make some of it actually
run in matches.
Started surgically with the lowest-risk, highest-feel piece -- COMMANDED SQUADS:
- In regular EX_WIN_ELIMINATE matches with >=4 grunts, part of the grunt budget
  now spawns as squads (1 squad, or 2 if >=8 grunts): a commander ringed by 2-3
  grunts bound to it via ex_encounter_spawn_squad. Remainder spawn independent
  via the normal makeup.
- Commanders are GRUNT-kind (NOT apex) so they don't trigger boss mechanics
  (slam/summon/telegraph) -- a mid-fight officer, not a boss -- with 300hp and a
  1.35x render scale as the visual tell ("the big one leads; kill it to scatter").
- The scatter/zone systems already run inside ex_encounter_update (which the game
  calls every frame), so once squads exist the whole U230 mechanic activates live:
  kill the leader -> its troops panic-scatter -> then fight on demoralized.
Verified the exact match-spawn logic in a harness linked against the real module
lib: 9 grunts -> 2 commanders + 6 bound + 4 independent, and killing a commander
scattered exactly its 3 troops. Build clean, 62/62 ctest green.
This is the first live wiring of the AI arc. Still dormant (next integration
steps): commander_ai FSMs (deterministic doctrine orders), hive_ai (infected
swarm), combat_json emit/apply. They apply through the same encounter path now
that squads exist in-match.

## Update 242 -- WIRED LIVE: commanders now COMMAND (doctrine FSMs drive squads)
Continued the integration pass from U241. Last turn squads spawned live but their
leaders didn't think (ex_commander_* still 0 refs in game.c). Now they do:
- Per-commander-slot ex_commander_t brain array (g_cmd_brain), re-init per match.
  Each commander gets a doctrine archetype deterministically from its id
  (aggressor/surrounder/skirmisher/baiter/warden).
- After ex_encounter_update each frame (throttled ~3/sec): build a combat_json
  snapshot of the live squads + player signals (fire + nearby-count feed the
  aggression/pressure tracker), run each live commander's ex_commander_think, and
  apply its order to that commander's squad by modulating troop zone-standoff:
  FOCUS->0.55x (press in), RETREAT->1.6x (fall back), FLOCK_TO->1.25x (rally off),
  FLOCK_SPREAD->0.9x (encircle), HOLD->1.0x. Panicking (scattered) troops ignore
  orders.
- This is the payoff of the shared combat_json vocabulary: the SAME snapshot->
  think->command path an LLM will use, now driving deterministic FSMs live.
Verified end-to-end in a harness vs the real module lib: an aggressor commander
issued FOCUS, dropping its squad's standoff 16.0->8.8 (presses in). Different
archetypes yield different orders (skirmisher retreats, warden holds), so squads
now fight by their leader's doctrine. Fixed two symbol issues found at wire time
(EX_ZONE_STANDOFF is static in encounter.c -> local table; g_run_t doesn't exist
-> frame counter). Build clean, 62/62 ctest green.
Now live: squad spawn (U241) + scatter-on-death (U230) + doctrine orders (this).
Still dormant: hive_ai (infected swarm), combat_json external transport (the LLM
seam itself). Commander archetypes are currently id-hashed; could be authored.

## Update 243 -- WIRED LIVE: the infected hive (Corrupted now swarm)
Continued the integration pass. Commanders think live (U242); the parallel HIVE
was still 0 refs in game.c. Now the Corrupted behave as one sneaky collective:
- Single per-encounter g_hive brain (the infected are a collective, unlike the
  per-squad commanders), strain chosen deterministically from the match seed
  (stalker/creeper/lure/ravener), re-init per match.
- Shares the commander block's combat_json snapshot. After the commander loop,
  ex_hive_think runs and its state reshapes INDEPENDENT Corrupted standoffs:
  CRAWL->1.7x (hang back, mass quietly), DANCE(FLOCK_TO)->1.1x (gather, ease in),
  FEED(FOCUS)->0.5x (converge + swarm). Corrupted in a commanded squad follow
  their commander instead; scattered ones ignore the hive.
- So the roster now has TWO contrasting live AIs: top-down doctrine commanders
  vs the stigmergic crawl->dance->feed swarm -- both via the same combat_json
  path an LLM will use.
Verified end-to-end vs the real module lib in two harnesses: (1) a ravener with
6 clustered Corrupted feeds immediately, pulling standoff to 15; (2) a patient
stalker with distant Corrupted CRAWLS (masses quietly), then progresses to FEED
once they close. Build clean, 62/62 ctest green.
Now live: squads (U241) + scatter (U230) + commander doctrine (U242) + hive
swarm (this). Still dormant: combat_json EXTERNAL transport (the actual LLM
seam). Strain/archetype are seed-hashed; could be authored per encounter.

## Update 244 -- turrets no longer clutter every map (a real backlog complaint)
Deliberately PAUSED the AI-wiring arc (4 systems landed in a row; play-testing is
the blocker, not more machinery) and returned to the user's original feedback
batch. Unaddressed item: "all the maps have turrets in the exact same spot even
though those are supposed to only be at large bases or the conquest gamemode",
plus "grifball doesn't change the turret in the map center".
Root cause: the turret spawn had reclaim (lanes) as a special case and an ELSE
that put 4 turrets in a ring at r=30 around the centre of EVERY other map, every
mode. That's the identical-center-turrets the player saw.
Fix -- gate by map + mode:
- reclaim (conquest-style lanes): keeps its 3x3 lane turrets.
- base-defense modes (DEFEND, EARTHDEF): the guard ring (defending a fixed base
  with emplaced turrets fits).
- everything else (arena/patrol/stalkers/skirmish AND grifball/orbrush): NO
  turrets -> clean gunfights, exactly as asked. This also fixes the grifball
  center-turret complaint in the same stroke.
Verified the gating decision table in a standalone check (reclaim=9, DEFEND/
EARTHDEF=4, all other modes incl. ORB=0). Build clean, 62/62 ctest green.

## Update 245 -- textures: detailed the flat GROUND (the most-seen, least-detailed surface)
"Keep detailing textures and models." Measured detail across all 77 base textures
via edge energy (mean laplacian). Finding: the flattest textures were the ones
you see MOST -- the terrain floors you walk on. seabed 0.31, sand 0.33, mars
0.36, io_sulfur 0.42 edge energy (vs rust 33, asteroid 36) -- near-featureless,
reading plasticky underfoot. Root cause: they used plain FBM (pat_rock) with
base~=accent (only ~9 rgb apart), so the noise had no contrast to work with.
Fix -- new pat_ground pattern: normalized coarse undulation + large-scale
patchiness + pebble specks (bright + dark flecks) + fine erosion RILLS + micro-
grain. First attempt caught ZERO pebbles (fixed 0.80 thresholds vs fbm's narrow
0.25-0.79 range) -> switched to PERCENTILE thresholds so specks fire on the
actual distribution. Regenerated sand/mars/seabed/io_sulfur with pat_ground +
widened palettes for contrast + fresh normal maps.
Result: edge energy ~3x (sand 0.33->0.97, mars ->1.01, seabed ->1.08, io_sulfur
->1.13), std doubled. Normal maps carry the pebble/rill RELIEF (rg_var ~2) so the
ground bumps under lighting, not just a flatter albedo. Tileable: seams (0.4-0.5)
are <= interior pixel steps (0.47-0.56), i.e. seamless. Added ground to the
texture self-test (tiles clean). Build clean, 62/62 ctest green.
NOTE: verified by edge-energy + tiling numbers + normal-map validity, NOT shaded
on-screen (image viewer wouldn't render the PNGs this session) -- the in-engine
look under real lighting is a play-test confirmation.

## Update 246 -- models: faceted the rocks (+ a lit prop renderer to verify meshes)
"Keep detailing textures and models" -- balanced to MODELS this turn (verifiable
via mesh renders even though texture PNGs won't display this session). Built a new
verification tool first: tools/prop_view.py -- loads a prop IQM and renders it with
DIRECTIONAL lighting (n.l over vertex normals), unlike silhouette.py which shades
by depth (blind to surface relief). It also reports a local_faceting metric (mean
dihedral angle between edge-adjacent triangles) that measures FINE relief, not
overall shape.
Target found: faceted_rock (the rock/boulder scattered across every map) used a
single low-frequency sine lump -> a smooth BLOB, not rock. Rewrote it multi-scale:
coarse lumps (boulder shape) + medium bumps (knobs/hollows) + fine facets (sharp
small relief, incl. a per-direction hash) + per-rock squash variety. Bumped mesh
res so the fine detail has vertices (rock 20x14->28x20, boulder 24x18->32x24).
Verified fairly at IDENTICAL resolution (the naive dihedral metric is confounded
by tri size, so I compared old-vs-new formula on the same 28x20 mesh): local
faceting 9.00 -> 10.46 deg, +16.2% genuine surface relief. AABBs stay sane
(~1.04 x/z, displacement capped ~4%), so collision is unaffected. Build clean,
62/62 ctest green. The bot part meshes are already dense (2000-4000 verts,
components); the rocks were the real low-detail prop.
NOTE: faceting increase is measured, not eyeballed -- prop_view PNGs didn't
display this session either. The lit in-engine look is a play-test confirmation,
but the metric + sane AABBs make the improvement objective.

## Update 247 -- models: carved relief on spire + pillar (were smooth tubes)
Continued model detailing from U246 (rocks). Next-smoothest props were the
tapered-cylinder columns: spire.iqm (rock) + pillar.iqm (metal) were perfectly
smooth revolution surfaces -- plastic tubes. Added optional surface RELIEF to
tapered_cyl: `flutes` vertical carved ridges (cos of angle) + fine radial noise,
fading toward the tip, controlled by a `relief` param that DEFAULTS TO 0 so the
~30 other callers (weapon barrels, ship/tank/robot parts, crystal) are untouched.
When relief>0, normals are recomputed from the perturbed faces so lighting
actually shows the flutes (analytic normals would ignore them).
Applied: spire relief=0.12 flutes=7 (rugged rock spike), pillar relief=0.06
flutes=12 (fluted carved column); crystal column left smooth (angular gem).
Verified at identical resolution: spire faceting 9.64->14.06 deg (+46%), pillar
9.42->13.81 (+47%) -- genuine carved relief. Mesh stays healthy (spire y 0..3,
x/z ~+-1.05, relief within ~7% of radius) so collision/footprint barely change.
Confirmed all other tapered_cyl callers use relief=0 (unaffected). Build clean,
62/62 ctest green.
NOTE: faceting measured, not eyeballed (prop PNGs still won't display this
session); lit in-engine look is a play-test confirmation. Prop_view tool from
U246 reused for the metric.

## Update 248 -- AI observability overlay (F2): make the wired AI legible in play
Deliberate choice on an open "keep working": instead of building/detailing more
that can't be validated without play-testing, made the ALREADY-WIRED enemy AI
observable, so a live match can actually be evaluated. Over the last arc I wired
squads + scatter + commander doctrine + hive but flagged repeatedly that whether
they READ in a firefight is unknowable without seeing them. This closes that.
- F2 toggles g_ai_debug (mirrors the F1 tuner pattern).
- Left-column VGA panel shows: each live commander's doctrine archetype + last
  order it issued (e.g. "CMD aggre:focus"); the infected hive's strain + state
  ("HIVE stalk:feed"); and a squad/independent/scattering tally ("sq6 ind3
  scat2"). Colour-coded (commanders cyan, hive purple, tally green).
- Captured each commander's last order in a new g_cmd_last_order[] parallel array,
  set in the commander update loop.
Verified the label strings in a harness vs the real lib: "CMD aggre:focus",
"HIVE stalk:feed", "CMD warde:none" (no order yet). Build clean, 62/62 ctest
green. Kept out of the player controls page (dev overlay, not a game control).
This is the tool that makes U230/241/242/243 evaluable in an actual match --
press F2 and watch which doctrine each squad runs and how the hive progresses.

## Update 249 -- grifball: the Orb is findable + grabbable (a backlog UX fix)
Another concrete, verifiable item from the original feedback batch: the grifball
(Orb Rush) orb was "hard to locate and pick up". Both were real:
1. PICKUP was backwards -- the old rule grabbed only when d2<4.5 AND player speed
   <3.5, but dribble-bumped the orb away whenever speed>3.5. So chasing a loose
   orb PUSHED it away repeatedly; you could only grab it by creeping. Fixed: a
   forgiving grab radius (d2<7 ~2.6u) picks it up at any normal speed; only a
   genuine fast charge (pv>8) still dribbles.
2. LOCATE -- no cue where a loose orb was on the 92u-long court. Added: (a) a
   loose orb is now a BEACON (bigger throbbing scale 1.15+0.35sin + brighter
   glow) vs a calm carried orb; (b) a HUD locator line "ORB 34m  > right" with
   distance + a facing-relative arrow (ahead/behind/left/right) from camera yaw
   vs bearing to the orb.
Verified pickup + locator logic in a harness: grabs at speed within radius (the
old frustration), and arrows resolve ahead/behind/lateral correctly. Build clean,
62/62 ctest green. Pairs with U244 (grifball no longer spawns the stray center
turret).
NOTE: the pickup radius + beacon pulse magnitudes are play-test tuning calls; the
logic (grab-at-speed, correct direction) is harness-verified.

## Update 250 -- bot design: the Hijack swarm gets its focused insectile pass
Gave the Hijack the focused design care the Corrupted got (U239-240); it was
differentiated but modest -- a scaled-down humanoid with a slightly big head,
not clearly a swarm BUG. Pushed the insectile read hard:
- proportions: thinner body (BK 0.60->0.52), bigger sensor head (HD 1.35->1.60),
  narrower shoulders (0.65->0.55), lower scuttling crouch (H 0.72->0.66, hunch
  0.22->0.34), longer limbs.
- pose: raised mantis-like forelimbs splayed wide, arms + shins thinned to
  spindly insect limbs, wide scuttle leg stance, oversized sensor head craned
  forward with jitter.
Measured (pose_dump): head/chest ratio 1.88->2.67, arm spindliness 4.51->7.89,
head width 0.35->0.49. Still shorter than the player (1.66 vs 2.09) so the
roster-height invariant holds. Extended test_bodykind to LOCK the insectile
look: hijack head/chest (2.67) > 1.5x player's (0.77), and hijack arm aspect
(7.89) > player's (2.35). Build clean, 62/62 ctest green.
Roster now: player (balanced), apex (towering armoured), corrupted (asymmetric,
stage-escalating), hijack (spindly big-headed bug) -- four clearly distinct reads.
NOTE: proportions/pose verified numerically via pose_dump (image viewer down all
session); the shaded, animated in-engine look is a play-test confirmation.

## Update 251 -- models: rounded ball-joint limb ends (parts read less separate)
User: "the body part meshes are obviously separate." Investigated the joints
numerically first: parts don't FLOAT (all adjacent joints overlap -- neck/hip/
knee gaps are negative). The "separate" read comes from limbs being flat-capped
cylinders that butt into the torso/each other as distinct blocks with hard seams.
Fix: gave the limb() core rounded BALL-JOINT ends instead of flat caps -- a
shallow dome at each end (radius-scaled depth) so an arm/leg nestles into its
shoulder/hip socket and into the next segment like an articulated joint. Kept
overall length IDENTICAL by shortening the cylinder core by the dome depth so the
dome tips land exactly at the old flat-cap positions -> no joint shift, limb still
fits its [-1,1] slot (verified: plimb y half-extent still 1.0, AABB stable).
Verified the rounding: top verts now spread 0.27 in y (dome, not flat) and radius
tapers 0.57->0.31->0.15 toward the tip. Feeds through robot_limb -> the shipped
plimb + all bots.json limb variants. Build clean, 62/62 ctest green.
Scope honesty: this is ONE concrete step. Investigated adding a neck bridge too,
but phead already has a neck cyl (bots.json), and robot_head() is overwritten by
the bots.json compiler -- so I reverted a dead-code neck edit rather than ship it.
The head/chest/pauldron connections live in content/bots.json (data-driven) and
are a separate, larger pass if more connection work is wanted.
NOTE: rounding verified by geometry (vert spread, radius taper, stable bounds);
the shaded in-engine look is a play-test confirmation (image viewer down).

## Update 252 -- CTEX compressed texture format (unpacks on load) + adaptive verified
Two user asks + a correction. (1) Verified the adaptive model pipeline on the new
01/02 samples, which turned out to be the mixed case I needed: Scorponok is 87663
tris (GEOMETRY-heavy -> decimated to 8000) while Ravage is 10k tris + 19M textures
(TEXTURE-heavy -> geometry untouched, textures slimmed). 91M -> 2M IQM (2.5%) /
19M slim (20%). The measure-then-compress design does the right thing per model.
(2) Built CTEX -- a compressed texture format that UNPACKS ON LOAD:
- tools/ctex.py: encode PNG -> .ctex. Colour/emissive maps: downscale + palette-
  index + zlib the index stream. Normal maps: zlib'd raw RGB (NOT palettized --
  would band normals). Measured: detailed 1024 baseColor -> 9.9% size @ 33 dB
  (near-lossless), or 2.7% @ 256. Normal map 59% @ 49.8 dB.
- engine_src/exengine/render/ctex.h: C decoder using the engine's EXISTING tinfl
  (miniz) inflate -- no new dependency. Hooked into ex_texture_load ahead of
  stb_image (CTEX blobs unpack to RGBA; .png/.jpg fall through). Build clean,
  62/62 ctest green.
- Verified the C decoder is BYTE-IDENTICAL to the python reference on a real
  texture (256x256 palette round-trip), so on-load unpack is proven correct.
On the "update the FSMs to compress" idea: the enemy-AI FSMs (commander/hive) are
behaviour engines, unrelated to asset bytes -- I did NOT force that. The coherent
version of "patterns -> compression" is CTEX's palette+dictionary approach (and a
future shared cross-texture dictionary), which is what actually shrinks these.

## Update 253 -- pipeline hardened for the HUGE no-color sculpts (up to 19M tris)
The no-color models are film/ZBrush-density: estimated ~19M tris (531MB) for
Unicron's Horn, ~97M tris across 13 models, ~2.7GB raw geometry. Engine bots are
3-8k tris, so these need 99.9%+ decimation. Three problems at this scale, fixed:
- MEMORY: trimesh's scene-graph load took 2.4GB for just 87k faces (extrapolates
  to ~234GB for the 531MB models -- infeasible). Replaced decimation with a LEAN
  direct-glTF-parse path (pull positions+indices from accessors, feed
  fast_simplification): 51MB vs 2400MB at 87k faces (47x less), 0.1s vs 14s. Makes
  the huge models actually processable. trimesh kept only as export helper.
- BUFFER NAMING: tf5_hot_rod's buffer is 'e.bin' not 'scene.bin' -- the loader
  already follows the glTF-declared uri (verified), and mesh_tris now also parses
  GLB JSON chunks.
- MISSING/DEGENERATE: tf1_ironhide has no .bin -- _load_gltf_geometry returns
  None on a missing buffer so the batch skips it gracefully instead of crashing.
Quality at extreme reduction (measured on scorponok 87k faces): even a 97.7% cut
to ~3900 faces deviates only 0.12% of model size -- these dense sculpts decimate
almost losslessly. Build clean, 62/62 ctest green.
CAVEAT: verified on the largest REAL model I have (87k tris). The multi-million-
tri ones (up to 19M) are validated by extrapolation of the lean path's memory
profile + the deviation curve, not run directly (I don't have those files). A
real run on one 500MB model is the final confirmation; RAM will be the thing to
watch even on the lean path.

## Update 254 -- FIX: process_models.py was not self-contained (user's 48 failures)
User ran the tool and got "IQM FAIL" on all 48 models. Root cause (my bug, two
parts): (1) process_models.py shelled out to gltf_to_iqm.py, which I NEVER
delivered -- it only existed in the engine repo, so every convert failed with
file-not-found. (2) The tool swallowed the real error behind a generic "IQM FAIL",
so the log gave no diagnosis.
Fix: made process_models.py FULLY SELF-CONTAINED -- inlined the IQM writer +
slot-normalize + vertex-normal computation (it already parsed glTF geometry
directly for decimation), so it writes IQMs itself with no external script. And
failures now print the REAL error ("FAIL: <reason>") per model.
Verified by reproducing the user's exact setup (only process_models.py + models,
no gltf_to_iqm.py): all convert, 0 fail, IQMs valid (correct bounds + non-
degenerate normals). Only deps now: numpy + fast_simplification (geometry),
pillow (archive) -- all pip-installable, no repo files needed.

## Update 255 -- FIX: decimation didn't hit budget on huge meshes + name collisions
User's re-run worked (48 ok, 0 fail) but exposed two bugs the real data revealed:
1. DECIMATION UNDERSHOT badly on the big models -- flying_optimus stayed at
   2.35M tris, mixmaster 1.5M, etc (budget was 8000). Cause: single-pass
   fast_simplification preserves topology and STALLS on disconnected/boundary-
   heavy ripped game meshes (measured: tf2_demolishor 23899 asked->8000 got
   18938, then plateaued at 17787 no matter how many passes). Fix: two-stage
   _decimate() -- iterate quadric for quality, then if it plateaus above budget
   fall back to VERTEX CLUSTERING (grid-snap, binary-searched) which guarantees
   reduction regardless of topology. tf2_demolishor now 23899->7910. Verified on
   the real mesh that plateaued.
2. NAME COLLISION -- two 'transformers_5_optimus_prime' folders (main + no-color)
   wrote the same .iqm; the second silently overwrote the first (47 files for 48
   models). Fix: find_models now prefixes the parent dir on duplicate base names.
Impact: the 13 under-decimated models were 560 MB of the 575 MB total; corrected
they're ~0.3 MB each -> full set ~575 MB drops to ~18 MB, and the lost 48th model
is recovered. Local re-run: 12/12 ok, all ~8000 tris, IQMs valid. 62/62 ctest.

## Update 256 -- rig_components.py: split imported models into engine part slots
While the batch converts, started the "components for animation" work. Key finding:
the glTF still HAS part structure the merge throws away -- each mesh is tagged with
a material whose name encodes the body part (RB_Jazz_Head/Chest/ArmUP/ArmLo/Legs/
Wheel), and the models are rigged (60-124 joints). So components are recoverable.
Built tools/rig_components.py: groups geometry by material, maps part-name ->
engine slot (phead/pchest/parm_upper/parm_lower/plegs/...), and for PAIRED limbs
that share one material, splits L/R by face-centroid x-sign (verified clean 240/240
on Jazz's arms). Emits one IQM per slot. Jazz -> head, chest, upper-arm L/R,
lower-arm L/R, legs L/R, wheel, logo -- a full articulated set, all IQMs valid
(bounds + normals ok).
HONEST SCOPE: this is the FOUNDATION, not finished animation. Reported limits:
(1) only works where material names are semantic -- War-for-Cybertron soldier uses
opaque names (Cust_Mat/INST) and won't map (falls to unmapped). (2) parts sit at
the MODEL's proportions, not the engine skeleton's joint positions -- posing them
needs a retarget pass (align each part pivot to its slot origin) = the next stage.
(3) texture-fitting: each part keeps its own UVs + we know its source material, so
per-part texture assignment is now possible but not yet wired. No engine change;
62/62 ctest still green.

## Update 257 -- model import pipeline COMPLETE: 3GB -> 13.6MB, 48/48, verified
User's run with the fixed tool: all 48 models converted, 0 fail, every one hitting
~8000 tris including the monsters (flying_optimus 19M->7233, unicron's_horn
19M->7324). 3GB source -> 13.6MB IQM (0.4%). Both fixes confirmed on the real
output: 48 files (was 47 -- collision fix recovered the lost model; 04b_ and
no-color-b_transformers_5_optimus_prime now distinct).
Verified the actual output archive (not just extrapolation this time):
- all 48 IQMs load, in-bounds, healthy normals, 0 suspect/degenerate.
- shape coherence: the clustered huge models have FEWER slivers (unicron 0.5%,
  flying_optimus 0.0%) than quadric-decimated Jazz (4.0%) -- clustering makes
  uniform well-formed triangles, so extreme reduction didn't fragment them.
- size distribution tight (126-498K, median 296K) = decimation uniform.
- vehicle proportions sane (wheeljack 1.2x0.65x2.0 = car-shaped, etc).
Remaining honest gap: could NOT eyeball renders (image viewer down all session),
so "looks like the character" is still a play-test/visual check; geometry is
verified sound by metrics. Vehicles are drop-in ready (single rigid mesh = what
the engine wants); robots need the rig_components split + retarget (U256) to pose.

## Update 258 -- character viewer: fixed centering + imported models now viewable
Two asks. (1) CENTERING: the viewer framed the bot ABOVE screen-center. Cause: the
camera sat at focus_y + dist*0.55 + 2.0 (very high) but aimed at focus_y*0.7 (below
the subject's middle), so the bot rode high. Fix: softened camera elevation to
dist*0.28 and aimed the look-at at the subject's true vertical centre (focus_y*
0.875 ~ 2.1 for the whole-bot view). Verified geometrically: subject span midpoint
moved from +2.2deg (high) to +0.6deg (~dead centre).
(2) IMPORTED MODELS IN THE VIEWER: placed the decimated Jazz + wheeljack IQMs in
data/, load imported_jazz as a template, and added F3 to swap the viewer between
the built bot and the imported real model -- centred on the plinth at the look
height (scale 2.8, y=2.1), spinning on the turntable, with a HUD hint. Build clean,
62/62 ctest.
CAVEAT (image viewer down all session): centering is verified by camera-projection
math and the imported model by load+bounds, not eyeballed -- the on-screen framing
+ how the decimated Jazz reads is the play-test confirmation. This is also the
first real imported model wired into the running engine.
NOTE: environment-scale assets (unicron's horn etc.) shouldn't use the character
slot-fit -- normalizing a map-sized horn into a 2-unit slot squashes it. A separate
scale class for props/set-pieces is a follow-up.

## Update 259 -- imported-model BROWSER in viewer: fixed F3 + framing + multi-model
Three issues from play-test feedback:
1. F3 DID NOTHING: the toggle was handled in the in-MATCH update path (line
   ~11200), which doesn't run during the customize menu. Moved the handling into
   menu_input() (F3 toggle + [ / ] cycle), which runs in EX_GS_CUSTOMIZE/BUILDER.
2. MODEL MISFRAMED (too large ~3x tall, camera inside it, centred over the menu):
   rewrote the display + gave the imported view its own camera. Model now scaled
   to ~4 units (s=2.0, feet on plinth), positioned at x=3 (right of origin), and
   the camera sits at (-2,3.6,11) -- well OUTSIDE the model -- aiming left of it so
   it frames on the RIGHT half beside the menu. Verified by projection math:
   camera depth ~12 (model only reaches z=2, so never inside), model spans screen
   sx +0.09..+0.52 (right half), sy -0.38..+0.43 (centred, ~80% height).
3. ONE MODEL -> a BROWSABLE LIST of 6 (Jazz, Grimlock, Megatron, Scorponok,
   Wheeljack, Optimus truck); [ / ] cycle, HUD shows the current name. Each loaded
   as a template, parked offscreen until shown. All 6 validated (load + tris).
Build clean, 62/62 ctest.
CAVEAT (image viewer down all session): framing verified by camera-projection
math + model bounds, not eyeballed. The exact on-screen placement is a play-test
check -- but F3 now runs in the right code path, which was the core bug.

## Update 260 -- imported models swap IN-MATCH (robust path) + 12-model browser
Play-test: the VIEWER showed a solid non-rotating fill on F3 (camera jammed inside/
against the model -- my custom viewer-camera framing was wrong, and I can't eyeball
it to tune). User suggested wiring F3 in-match instead. Did that -- it's the robust
path because the in-match camera already frames the player correctly:
- In-match F3 swaps the player's built bot for the selected imported model at the
  player's position + g_body_yaw, scaled to stature. [ / ] cycle. Handled next to
  F1/F2 (which DO run in-match). HUD shows "MODEL: <name>".
- Also SIMPLIFIED the viewer: dropped the broken custom camera; the imported model
  now sits at origin where the built bot goes, so the existing (working) builder
  camera frames it. (Confirmed the models carry plate_blue, not yellow -- the
  "yellow" was the camera pressed against a surface, i.e. inside-the-model.)
- Expanded the browser 6 -> 12 models (Jazz, Grimlock, Megatron, Scorponok,
  Wheeljack, Optimus truck, Bumblebee, Optimus Prime, Soundwave, Shockwave,
  Devastator, Laserbeak). All 12 load-verified.
Build clean, 62/62 ctest.
NOTE: in-match swap reuses the proven game camera, so this should finally be
visible/correct. Model is static (no per-limb animation yet -- that's the
rig_components + retarget path). Image viewer still down my end, so this is a
play-test confirmation.

## Update 261 -- full 48-model browser (all imported models selectable in-match)
"Keep adding models": expanded the imported browser from 12 to ALL 48 converted
models. Copied every decimated IQM into data/ and GENERATED the C file/name arrays
from the filenames (script, not hand-typed -- avoids fat-fingering 48 paths).
Display names cleaned (FOC JAZZ, WFC ZETA PRIME, etc.). Verified all 48 referenced
files exist; 14MB total in data/ (largest ~500K -- all properly decimated). F3
in-match cycles through the whole roster with [ / ]. Build clean, 62/62 ctest.
NOTE: env-scale assets (unicron's horn, etc.) are in the list too but will look
squashed until the separate prop scale-class lands (flagged U258). Still static
meshes (animation = rig_components + retarget, not yet).

## Update 262 -- per-model correction HARNESS (live tune) + hole-fixing weld
Play-test feedback: some imported models flipped/rotated/wrong-size (different
authoring conventions), and some have holes from decimation. Two fixes:
1. CORRECTION HARNESS (engine): per-model rx/ry/rz rotation offsets + scale mult
   (g_imp_*[EX_N_IMPORTED]), default identity so nothing worsens. Applied in the
   in-match swap (rx=pitch/lay-down fix, ry+facing, rz=roll/flip, scale). LIVE
   TUNING: F4 in imported mode -> IJKL rotate yaw/pitch, U/O roll, +/- scale,
   0 reset; HUD prints the values to bake back into the table. Verified the
   renderer's Euler order (Y,X,Z degrees) so the axis labels are correct.
2. HOLE FIX (tool): the holes come from source meshes splitting vertices at UV/
   material seams -> decimation opens them into gaps. Added _weld() before
   decimation: merges near-coincident verts. Measured on scorponok: boundary
   edges (holes) in the final 8000-tri mesh dropped 3244 -> 431 (-87%), and
   welding lets quadric decimate further (fewer clustering fallbacks). Re-run the
   batch to get hole-fixed models.
Build clean, 62/62 ctest.
NOTE: harness values are empty (identity) until tuned -- the user reads them off
the HUD per model and I bake them in, since only they can see which needs what.

## Update 263 -- hole fix (weld verified + adaptive quality guard) + shape-match R&D
Holes: diagnosed on the user's ACTUAL output -- worst offenders 44-67% boundary-
edge ratio, across many models (not just TF1; that was the visible subset). Root
cause = seam-split source verts (last turn's _weld). VERIFIED weld fixes it:
shockwave 44%->0.6%, optimus_truck 60%->4.5% holes when re-decimated. Added a
QUALITY GUARD: after decimating, if hole-ratio still >25%, auto-retry at 4x budget
(where more surface survives) and keep the better result -- catches the stubborn
clustering-torn cases the user saw without hand-picking budgets. Re-run the batch
to get the repaired models.
Shape-match parts (R&D toward the parts/animation vision): prototyped classifying
mesh parts by GEOMETRY not material name (so it works on models with opaque names).
Roundness (radial std/mean) + elongation + flatness. Correctly IDs Jazz's wheel
(roundness 0.28) and body parts, BUT misclassifies straight arms as wheels (round
cross-section) -- so shape alone isn't enough; needs POSITION context (wheels low,
arms high/lateral). Honest first step, not finished. No engine change; 62/62 ctest.

## Update 264 -- sandbox mode + UI log + layout-neutral tuning + Dvorak menu fix
Fixed the whole model-tuning WORKFLOW (was: tune while being chased by Jhiaxus,
read values by hand). Four parts:
1. SANDBOX (F5): sets g_sandbox + restarts the arena with ZERO enemies (gates
   n_grunt/corr/hijack=0 and forces wave_delay huge). A calm space to inspect/tune.
2. UI LOG (D in sandbox): game_log() ring buffer (14 lines) shown on-screen +
   mirrored to stdout. EVERY change is logged -- each tune adjustment writes
   "JAZZ rx=90 ry=0 rz=180 scale=1.100", model cycles, mode toggles. So the user
   reads changes from the log, never by hand. Solves "read them by hand".
3. LAYOUT-NEUTRAL TUNING: replaced the QWERTY IJKL/UO/+- tune keys (wrong on
   Dvorak) with arrows + PageUp/Dn + Home/End + Delete -- identical on every layout.
4. DVORAK FIX: menu navigation hardcoded physical W/A/S/D scancodes (wrong keys on
   a Dvorak preset). Now uses the ACTIVE layout's movement bindings
   (ex_keybind_get FORWARD/BACK/LEFT/RIGHT); arrows still work as neutral fallback.
Build clean, 62/62 ctest.
Controls now: F5 sandbox, F3 imported view, [ ] cycle, F4 tune, arrows/pgup-dn/
home-end tune + del reset, D log. Read the log, send me the values, I bake them.

## Update 265 -- auto_orient.py: best-guess orientation + scale (measured accuracy)
Toward "auto-align/size with pose+shape detection": did the empirical work first
rather than ship blind heuristics. Findings on known models:
- UP-AXIS / STANDING detection is RELIABLE: robots come in thin front-to-back
  (short axis = Z), so up = taller of the two big axes; vehicles are low (short
  axis = height). Verified: jazz comes in lying on X -> suggested rz=90 stands it
  up in Y correctly; grimlock already upright -> no rotation. This fixes the most
  common complaint (flipped/lying-down) automatically.
- VEHICLE vs ROBOT is a ~70% BEST GUESS (5/7): shape-only can't separate a
  scorpion (reads long like a vehicle) from a compact car (reads square like a
  robot). Honestly labeled as a suggestion, not truth.
- FACING is not reliably detectable from shape -> left to manual (F4).
Built tools/auto_orient.py: analyzes each model, prints suggested rx/ry/rz + kind.
These are a HEAD START -- the user confirms/overrides in-game (the manual harness
value wins). Kept as a tool output (not auto-baked) since review is needed. No
engine change; 62/62 ctest.

## Update 266 -- baked auto-orientation defaults into the harness (test-ready)
Ran auto_orient.py on all 48 decimated IQMs and baked the suggested orientation
corrections in as the harness DEFAULTS (g_imp_rx_def/g_imp_rz_def, copied into
g_imp_rx/rz at init). Result: 34/48 get a non-zero correction; after applying,
35/48 stand tallest-in-Y (the other 13 are legitimately wider-than-tall --
vehicles, Laserbeak's wingspan, Devastator -- so that's correct, not a miss). So
most models now START upright instead of needing hand-tuning; the user only
adjusts the ~30% the classifier missed + facing.
Also fixed a bug this exposed: rotated models floated/sank (feet-at-ground assumed
the UN-rotated bounds; worst offset 0.81u on Laserbeak). Now the swap centres the
model at mid-body height (orientation-independent), so auto-rotated models sit
right. Still overridable in-game (F4); manual value wins. Build clean, 62/62 ctest.

## Update 267 -- part-detection R&D (honest ceiling found) + env-scale prop scale
Pushed on shape+position part detection (the animation keystone). Real finding via
measurement: it's blocked by orientation. To classify parts by position (head high,
legs low, wheels outboard) the model must be STANDING HEAD-UP -- but bounding-box
orientation only makes the longest axis vertical, which for a T-posed robot can be
the arm-span, and "which end is the head" resists shape heuristics (robots aren't
narrower at the head like humans -- measured Jazz ends 3.9 vs 3.9 width, no signal).
Honest conclusion: fully-auto character orientation (head-up + facing) is NOT
reliably solvable from raw geometry for these stylized models. So the manual F4
pass (now easy: sandbox + log + neutral keys + auto up-axis) stays the primary
tool; auto-detect is a partial assist. Documented rather than shipping a wrong
heuristic.
Pivoted to a solvable one flagged since U258: ENV-SCALE set-pieces squashed by the
character slot-fit. Added g_imp_scl_def -- models with extreme flatness (unicron's
horn, flatness 0.09) or known set-pieces (battleship) get a bigger default scale
(3.0x / 2.5x) so they read as environment, not crushed characters. Overridable in
-game. Build clean, 62/62 ctest.

## Update 268 -- Dvorak tuning cluster + sandbox-interruption fix
Three items from feedback:
1. SANDBOX INTERRUPTED: with 0 enemies the wave system marked itself 'finished'
   and called end_wave_run(1) -> results screen, kicking the user out mid-tune.
   Fixed: gated the whole wave update/completion loop with !g_sandbox (line 8004),
   so sandbox never triggers victory/results.
2. TUNING KEYS -> Dvorak cluster: user wants them around htns/gcrl. Those Dvorak
   letters sit at physical scancodes J K L ; (home row) and U I O P (top row).
   Mapped: h/t (J/K)=yaw, n/s (L/;)=pitch, g/c (U/I)=roll, r/l (O/P)=scale;
   [ ] still cycle; Del/Backspace reset. Arrows kept as a fallback. Every change
   still logs to the D-log.
3. CONFIRM KEY: it's Enter or Space (both physical, layout-independent).
Build clean, 62/62 ctest.

## Update 269 -- baked the user's tuned values (19 models) + pattern defaults
User tuned models in the sandbox and sent the final per-model values (captured via
rg -B1 = last value before each model-switch). Baked them in:
- 19 models get their EXACT tuned rx/ry/rz/scale (Jazz, Grimlock, both Optimus
  variants, Brawl robot/tank, Bumblebee camaro/missiles/veh, Devastator, battleship,
  Gimlock dino, Zeta Prime, WFC soldier/optimus/bumblebee, FOC optimus/optimus-cy).
- STRONG PATTERN found: nearly every model needed ry=-90 (the whole set faced 90
  degrees wrong) and scale ~0.31 (my base stature mult was ~3x too big). Added the
  missing g_imp_ry_def array + seed it at init. The 29 untuned models now default
  to ry=-90 + scale=0.33 + their auto-orient rx/rz -- a much better starting point.
- Angles normalized to (-180,180]. Note: idx 0 and 19 both display "TRANSFORMERS 5
  OPTIM" (name truncation collision) -- applied the tuned value to both.
All overridable in-game; manual still wins. Build clean, 62/62 ctest.

## Update 270 -- quality tiers (indie/high/ultra) for AAA/IMAX fidelity
User wants a higher-fidelity tier (2-4x tri + texture) for AAA/IMAX vs the current
indie 8k. Measured the payoff first on scorponok: 8k->16k->32k drops mean edge
length 1.69->0.99 (41% finer detail = more curvature captured), holes stay 3-4%,
size scales linearly (~250K->~930K per model). So the tier is real, not just bigger.
Added --quality {indie,high,ultra}: indie=8k tri/512 tex, high=16k/1024, ultra=
32k/2048; scales tri budget + texture cap + palette together. Explicit --tri-budget
/--tex-cap still override. Tested ultra: scorponok -> 31998 tris/933K clean.
HONEST on the TEXTURE half (the real fidelity gap): the IQMs currently carry a
'plate_blue' PLACEHOLDER (the writer hardcodes tex="plate_blue.png"), so raising
tex-cap only helps once real textures are wired. The models' actual baseColor/
normal/emissive maps exist (archive mode extracts+CTEX-compresses them), but the
single-mesh merge collapses a model's several materials (head/chest/arms each have
their own texture) into one slot -- so real texturing needs per-material mesh
groups (the rig_components split) + writing each group's texture name into the IQM
+ shipping the CTEX. That's the next real chunk, not a flag. No engine change here.

## Update 271 -- baked 13 more hand-tuned models (from the same build)
Second batch of user-tuned values (same build). Baked 13 more: FOC Wheeljack +
vehicle, Laserbeak, both Megatrons (head-tf2 + tf1), Optimus Prime truck, Ravage,
BOTH Shockwaves, both Skywarps, TF1 Barricade, and the idx-19 Transformers-5-
Optimus (updated scale 0.577). Used the log's CYCLE ORDER to disambiguate the two
"SHOCKWAVE" name-collision entries (idx23 shockwave_-_devastation vs idx24
shockwave_human_alliance) and to pin "TRANSFORMERS 5 OPTIM" to idx19 specifically.
Angles normalized (e.g. ry=990 -> -90, rx=-270 -> 90). Now ~32/48 models carry
hand-tuned corrections; the rest still use the ry=-90/scale=0.33 pattern defaults.
Build clean, 62/62 ctest.

## Update 272 -- baked 3 more tuned + swapped in ULTRA (32k) models
Third tuning batch baked: TF2 Demolishor (rx=-90 s0.878), TF4 Crosshairs (rx=-75
ry=90 rz=90 s1.378), TF5 Barricade (rx=-90 ry=90 rz=180 s0.352, last value). ~35/48
now hand-tuned.
Swapped the ULTRA-tier models (user's --quality ultra run, 32k tris) into data/,
replacing the 8k indie ones -- orientation/scale corrections are tri-count-
independent so the baked tuning still applies. Verified all 48 load valid, 0 bad;
48 refs all present. Package grows ~14MB->31MB in data/ (higher fidelity; browser
shows one model at a time so no perf hit). Build clean, 62/62 ctest.
CAVEAT: a few ultra models still show ~60% holes (FoC Optimus pair) -- the user's
process_models_v4 may predate the weld+quality-guard fix; re-running with the
latest tool should clean those. Flagged, not blocking.

## Update 273 -- REAL TEXTURES: per-part textured export + Jazz proof in-engine
Started the textures work (chosen first over animation: higher visual payoff, more
tractable; both share the per-part foundation). Confirmed only 1/12 sample models
is single-material -- proper texturing genuinely needs the per-part split, same as
animation, so it's a convergent investment.
Built tools/texture_export.py: splits a model by material KEEPING each part's real
UVs, extracts each part's real baseColor (+normal) texture, converts them into the
engine texture dir, and writes one IQM per part whose material field points at its
real texture (engine's IQM loader auto-derives norm_/spec_ lookups). Verified end
-to-end on Jazz: 7 textured parts, real 1024 textures (chest mean RGB 83,90,98 --
real data, not placeholder blue), part IQMs correctly reference them.
Wired the engine PROOF: Jazz (idx 12) now renders as 7 real-textured parts posed
together (same transform, normalized as a group) instead of the flat plate_blue
placeholder; other models unchanged. Build clean, 62/62 ctest.
SCOPE: this proves the path on one model. Rolling out to all multi-material models
+ generalizing the N-parts engine path (currently Jazz-specific arrays) is the next
step. ANIMATION is still ahead -- but the parts now exist separated + textured,
which is exactly what posing them needs.

## Update 274 -- material maps (normal + emissive + spec) for depth, not just diffuse
User: Jazz textures landed well (the red arm is REAL Jazz detail, not a bug --
confirmed the arm uses its proper baseColorTexture, not an emissive fallback). But
tires/face/shoulders look FLAT -> wants PBR. Found the engine's forward shader
ALREADY does normal mapping + specular(metallic) + Fresnel reflection + texture
emission; the flat look was purely because texture_export shipped ONLY baseColor.
The IQM loader auto-binds norm_<tex>, spec_<tex>, emit_<tex>. So: extended
texture_export.py to also export normal -> norm_<tex>, emissive -> emit_<tex>,
metallicRoughness -> spec_<tex>. Re-exported Jazz: all 7 parts now carry base +
normal + emissive (verified: normals are valid blueish maps ~127,128,250; emissive
has real content). No shader/engine change needed -- the loader picks them up. This
gives normal-mapped surface relief on the tires/face + glowing accents = the depth
the flat diffuse lacked. Build clean, 62/62 ctest.
NOTE: Jazz's set has no metallicRoughness maps (shader's metallic baseline covers
it); other models that DO have them will now get spec_ too. Full PBR metal/rough
control would be a shader tweak on top; the normal+emissive is the big flat->
dimensional win and needs no engine change.

## Update 275 -- per-model material boost: bumpier + more reflective imported models
User: textured look is "between indie and IMAX, closer to indie"; shader isn't
interacting enough -- wants more bump + reflections. The forward shader is actually
rich (fake-IBL env reflection, fresnel, sun glint, rim, normal mapping) but tuned
conservatively + shared by ALL models, so I couldn't just crank it globally.
Added a PER-MODEL control: model->mat_boost -> u_mat_boost uniform (default 1.0, so
existing models unchanged). It amplifies (a) normal-map bump strength (nrm.xy scaled
-> more surface relief on tires/face), (b) env reflection + fresnel (catches sky
more), (c) sun glint (shinier metal). Imported models (single mesh + Jazz parts)
set mat_boost=1.8 so they read more dimensional/reflective than the built bots
without touching the rest of the game's look.
model.h field + model.c init (0->treated as 1.0) + renderer uniform + 3 shader
uses. Build clean, 62/62 ctest; edited GLSL syntactically validated.
NOTE: 1.8 is a starting value -- easy to dial per taste. Bigger IMAX gains beyond
this would be real roughness/metalness maps driving the spec (a metal-rough shader
path), but this is the low-risk "more bump + shine" the user asked for.

## Update 276 -- fixed the "hollow box" (weld) + textured 4 models + generalized
User: Jazz feels like a hollow box; wants more models/textures. DIAGNOSED the
hollow feel with data: the textured parts had 24-46% open boundary edges -- the
texture_export path never welded, so seam-split source verts left the parts full
of cracks you see into. FIX: position-only weld in texture_export (UVs averaged at
seams -- minor seam artifact, big solidity win). Jazz parts 33% avg holes -> 9%.
Honest: some boxiness is the 2012 FoC design itself (open panels assembled to look
solid from the front) -- welding closes the cracks, can't re-sculpt the era.
ROLLED OUT: exported Jazz(7 parts)+Grimlock(6)+Scorponok(1)+WFC-Optimus(1) as real
-textured parts, and GENERALIZED the engine from Jazz-only to a g_tmodels[] table
(idx -> part files); any listed model shows textured parts, others use the single
mesh. Adding more = one table row + an export. All carry mat_boost 1.8. Build
clean, 62/62 ctest.

## Update 277 -- fix texture regression (my weld bug) + Grimlock legs + shine
Feedback caught real bugs:
1. JAZZ PURPLE / red-glow reverted: my position-only weld (U276) averaged UVs
   across genuine seams -- 131 seam-UVs merged on Jazz's arm alone -> corrupted
   texture sampling. REVERTED to UV-aware weld (merge only same-pos-AND-same-uv
   true duplicates). Textures correct again. Tradeoff: seam-boundary cracks remain
   (the era's open-panel design; welding them corrupts textures -- honest limit).
2. GRIMLOCK LOST LEGS: the g_tmodels table listed 6 parts but Grimlock exports 7
   (part6 = legs). Fixed count -> 7; legs load now.
3. UNIFORM SHINE (Grimlock head): softened the sun-glint's mat_boost to sqrt() so
   bump+reflection keep full boost but the mirror-glint doesn't blow out flat;
   lowered textured-parts mat_boost 1.8 -> 1.5.
Re-exported all 4 textured models with the fixed weld. Build clean, 62/62 ctest.
SCORPONOK FILL-SYSTEM IDEA (user): Scorponok is 19 separate body sub-meshes (ribs/
hollow-body) under 2 materials -- genuinely rich detail. Noted as a real R&D
direction: catalogue those sub-meshes as a reusable greeble/detail library for a
generic fill system. Exploratory, not built yet -- flagged for a focused pass.

## Update 278 -- more textured models + FIRST animation layer
Textures: user flagged Skywarp/Soundwave/Brawl robots as detailed -- but I DON'T
have those sources (only the 12 samples; the rest are in the user's 3GB set).
Honest: textured the detailed ones I DO have instead -- Shockwave(4 parts), WFC
Zeta Prime(3), WFC Soldier(3), Ravage(1). Now 10 models render real per-part
textures via the g_tmodels table.
ANIMATION (first layer): imported models were static. Added a procedural gait:
idle = gentle breathing bob + sway; moving = faster stride-bob + forward lean,
intensity scaled by player speed. Computed once, inherited by both the single mesh
and the textured parts so they move together. g_imp_anim_t phase accumulator.
Build clean, 62/62 ctest.
HONEST SCOPE: this is WHOLE-BODY procedural motion (bob/sway/lean) -- makes them
feel alive + weighted, but it is NOT per-limb walking yet. Real limb articulation
(legs stride, arms swing) needs the L/R part split + per-limb pivots -- the next
animation step. Starting with body motion is the robust first pass (can't look
broken) before limbs.

## Update 279 -- texture_export.py: BATCH mode (whole folder, like process_models)
User wants to run it on their full 3GB set and send back all the textured parts.
Refactored texture_export.py: point it at a PARENT FOLDER and it finds every model
under it (recursive, same find_models + duplicate-name disambiguation as
process_models.py) and exports each -> per-part IQMs + manifest + all textures.
Auto-names from the folder (imported_<name>_partN.iqm convention). Single-model dir
+ --name still works. Per-model FAIL lines + ok/fail/parts summary. Tested batch on
9-model folder: 9 ok, 0 fail, 22 parts. Output = one flat dir of IQMs+manifests +
a textures/ subdir -- exactly what the engine consumes.
Usage:
  python3 texture_export.py MODELS_DIR --out-iqm parts --out-tex parts/textures
(then send back the parts/ dir; I map each model's parts into the g_tmodels table.)

## Update 280 -- FIX: textured models vanished (animation ordering bug, my fault)
Jazz/Grimlock/Ravage/Scorponok (all textured-parts models) stopped showing. Cause:
the U278 animation refactor made the parts copy im->position[1] -- but that copy
ran AFTER "im->position[1] = -100000" (the line that hides the placeholder mesh),
so every part inherited the hidden sentinel and vanished. Fix: save the animated
body height (anim_y) BEFORE hiding the placeholder, and position the parts from
anim_y. Single-mesh (non-textured) models were unaffected (they never hit that
branch), which is why only the textured ones disappeared. Build clean, 62/62 ctest.

## Update 281 -- part SCANNER (tire/fan auto-spin) + Grimlock white-legs fix
Grimlock white legs: texture+emissive were fine (dark) -- it was mat_boost 1.5
blowing out the sun glint/reflection on the leg panels (they face the sky). Lowered
textured mat_boost 1.5 -> 1.25.
SCANNER (user's idea -- detect tires/fans/doors for auto motion): added scan_anim()
to texture_export -- classifies each part by NAME (wheel/tire/fan/rotor/door/armor,
reliable) confirmed by SHAPE (roundness), writes tag+axis into the manifest. Jazz
wheel -> "tire" spin-axis Z (arms no longer misfire since name-first). Engine reads
per-part atag/aaxis: TIRES roll proportional to movement speed (ENGAGED = rolls
when moving, still when stopped -- respects the user's lock-vs-engaged note; a wheel
that only spins with motion reads correct for both), FANS spin freely. Jazz wheel
now rolls when you move.
Build clean, 62/62 ctest.
SCOPE: scanner tags tires/fans now; door/armor hinge + bullet/collision response +
suspension is the bigger mechanical-animation layer ahead (the user's movie-model
vision). Foundations (part split, tags, per-part transform) are in place for it.

## Update 282 -- OBJ support in BOTH pipelines (unblocks the user's new library)
User's new 48GB library is .obj (they flagged the gap). Added OBJ across both tools:
- process_models.py: _load_obj_geometry (v/f parse, polygon fan-triangulation,
  negative-index support) + load_geometry() dispatcher; find_models detects .obj.
  Tested: quad cube -> triangulated valid IQM; dense sphere (6962 tri) -> decimated
  2052 through the full weld+quality-guard path. Valid bounds+normals.
- texture_export.py: load_obj_by_material (usemtl groups + per-corner UVs) +
  _parse_mtl (map_Kd baseColor); export_one routes .obj -> obj loader / mtl
  textures, .gltf -> gltf path. Tested: OBJ+MTL cube -> textured part, texture
  extracted, scanner ran. Both tools still handle glTF (no regression: Jazz 7 parts,
  sample geometry ok).
Note: OBJ carries only baseColor via map_Kd (no normal/emissive/spec in basic MTL);
those models get diffuse-only unless the MTL has map_Bump etc (future). Geometry +
diffuse is the core need to bring the library in. Engine unchanged; tools only.

## Update 283 -- model_metadata.py: zip-name classification -> structured routing
User: "the names of the zip files contain classification metadata; route that to a
rich metadata system." Built tools/model_metadata.py: parses the library's PATH
(parent dir -> category: bot/vehicle/animal/raid_boss/weapon/...) + ZIP NAME
(prefixes: both-=robot+alt modes, animated/idle, detailed, +weapon, low-poly, pbr,
set; subtype: spiderbot/hawk-flyer/dragonflyer/flyer/raidboss; '--name'=character;
'aka ...'=author note) into per-model JSON records. Reconstructs full paths from an
indented `tree` listing (recovers category even from a text tree, not just a walk).
Tested on the user's actual plan.md tree: 487 models parsed -> 189 vehicle, 84 bot,
57 raid_boss, 28 weapon, 27 terrain, 13 structure; 19 both-modes; subtypes incl 60
flyer, 7 spider, dragon/avian/beast. This is the routing backbone for patrol/raid
placement per bot. Standalone tool.

## Update 284 -- extract_models.py: recursive name-preserving unzip
User's models are still inside .zip files (process_models found nothing: "no models
found under ."). Built tools/extract_models.py: recursively finds every .zip and
extracts each into a sibling folder named after the ZIP (stripping .zip, keeping
spaces/--/aka), so the classification metadata in zip NAMES becomes folder names
that find_models/process_models/texture_export/model_metadata all route on. Handles
spaces + special chars, NESTED zips (multi-pass: a zip extracted from a zip gets
extracted too), path-traversal safety, skips already-done folders (idempotent), and
prints a recursive tree marking [model] folders (--tree-only / --tree-out too).
Tested on a tree with spaces/nested-zip/special-chars: names preserved end-to-end
into find_models + metadata (Grimlock=both-modes bot, nullbite=spider, gigadrill=
raid_boss). CORRECT WORKFLOW is now: extract FIRST, then process/texture/metadata.

## Update 285 -- parallelize all heavy tools (--jobs, default all cores)
User: make processing multithreaded on all fronts. Each model/zip is independent,
so added parallelism to the 3 heavy tools (metadata is us-per-name, left serial):
- process_models.py: _process_one() picklable worker + ProcessPoolExecutor over
  models (CPU-bound decimation -> processes, not threads, to beat the GIL). --jobs
  (default os.cpu_count(), 1=serial). Results stream as they complete.
- texture_export.py: ProcessPoolExecutor over export_one. --jobs.
- extract_models.py: ThreadPoolExecutor per pass (I/O+decompress; threads fine,
  no ZipFile pickling), nested-zip multi-pass preserved. --jobs.
Verified parallel and serial paths give identical results (process_models 9 ok
1022K both ways; texture_export 9 ok 22 parts; extract incl nested pass). Sandbox
is 1-core so no speedup shown here, but scales on the user's machine. Tools only,
engine untouched.

## Update 286 -- raid_content.py: raid boss lore -> structured game content
While the user prepares data, built out the docs' raid system. raids-and-relics.md
frames every raid boss as an ancient Cybertronian relic (archaeological find, not
just a big enemy) + hand-writes lore for ~17. Built tools/raid_content.py:
- PARSES the authored lore into structured records (name, epithet "The X",
  classification, backstory paragraphs) -- anchors on the quoted-epithet-under-name
  pattern + Classification: lines. Tested: 17 parsed clean (Gigadrill "The Core
  Piercer" 12 paras, Cherno Alpha "The Walking Reactor", etc.).
- GENERATES name-themed stub lore for bosses without written lore, in the same
  relic style, driven by name keywords (titan/drill/reactor/scorpion...) -> desc +
  class + combat TAGS (radiation/burrow/swarm/artillery...) the encounter system
  can use.
- Outputs raid_content.json for the game to load into raid locations.
KNOWN NIT: the tree filter is a bit over-inclusive (pulled 1 raid-goon in as a boss,
mis-themed). Authored-lore parsing (the valuable part) is solid; tighten the
boss-vs-goon filter when wiring the real extracted raid-bosses/ dir. Standalone tool.

## Update 287 -- game-model-tools/: repo cleanup + plans + Python + RUST ports
User: clean up, add plans to repo, and add the python tools + RUST counterparts
under a shared "game model tools" (for reuse in games/soldier + apps/logscope).
- CLEANUP: removed tools/__pycache__, stray jaztest artifacts.
- game-model-tools/plans/: plan.md, raids-and-relics.md, plan5.zip.
- game-model-tools/python/: the 5 pipeline tools (extract_models, process_models,
  texture_export, model_metadata, raid_content).
- game-model-tools/rust/: a Cargo workspace with native ports -- ALL COMPILE
  (cargo 1.75, release):
    * model-metadata -- FULL parity (output matches Python exactly: 468 models,
      identical category counts + 19 both-modes). Fixed a UTF-8 char-boundary
      panic on a curly-apostrophe name.
    * raid-content   -- lore-parse parity (authored entries).
    * extract-models -- full parity (recursive, name-preserving, nested zips; uses
      the `zip` crate).
    * obj-iqm        -- OBJ load+normalize+normals+IQM write; HONEST scope: quadric
      decimation + glTF + texture/scanner remain Python-only (documented in README).
- README.md documents pipeline order, both toolsets, and Rust parity status.
- .gitignore for rust/target; build artifacts not committed (388K total).
Engine unchanged, still 62/62 ctest.

## Update 288 -- real terrain craters (physics) + Rust decimation port
Two tracks the user asked to keep pushing:
1. TERRAIN/PHYSICS: explosions now DEFORM the collision heightfield, not just decal.
   deform_terrain_crater() digs a smooth parabolic bowl into g_terr[][] at each
   blast (radius/depth from blast size, clamped, grid-bounded). Craters are now
   physically real -- vehicles/bots sink in + slopes shift movement, the groundwork
   imported models will interact with. Wired into spawn_crater (forward-decl +
   real def below the terrain grid). Build clean, 62/62 ctest.
   HONEST: this deforms the COLLISION field (gameplay-real); the VISUAL terrain mesh
   is a separate prebuilt model, so the crater's visual is still the decal -- true
   visual mesh deformation is a bigger GPU-rebuild task, noted for later.
2. RUST PORT: added vertex-clustering decimation to obj-iqm (--tri-budget N): grid-
   snap verts -> collapse cells to centroids -> remap/drop-degenerate faces, coarsen
   until under budget. Tested: 12482-tri sphere -> 526t valid IQM (bounds+normals ok).
   Port of the Python _cluster_decimate fallback. Full quadric still Python-only
   (documented). Rust obj-iqm is now a real load+decimate+write tool.

## Update 289 -- per-corner suspension (the plan's "expanded suspension")
Picked up the suspension expansion the plan calls for, and it lands right on top of
the new craters. WAS: one centre ride-height spring + an AVERAGED terrain gradient
for tilt -- so a crater under one wheel just shifted the average slope.
NOW: 4 independent spring-damper corners (FL/FR/RL/RR). Each samples its OWN ground
height and runs its own spring; body pitch comes from front-vs-rear compression and
roll from left-vs-right. A crater or rock under ONE wheel compresses that corner and
tilts the body diagonally -- real independent wheel travel.
VERIFIED NUMERICALLY (can't play-test): replicated the exact integrator offline --
k=26, damp=9 gives zeta=0.88 (underdamped: quick settle, ~no overshoot), natural
freq 0.81Hz (realistic vehicle range), wn*dt=0.085 so explicit-Euler stable. Crater
test: tracks a 1m drop to 0.989m, rebound settles in 0.9s, never hits the +/-1.6
travel clamp. Attitude table: FL-in-crater -> -4.8 pitch/-4.8 roll (diagonal),
both-front -> -9.5 pitch/0 roll, left-side -> 0/-9.5, ALL-FOUR-in-crater -> 0/0
(body drops LEVEL) -- that last case is the one the old gradient couldn't get right.
Kept the critical existing rule (fold into the lean TARGET, never the smoothed
state) that fixed the old runaway-shear bug. Added g_susp_travel (max corner
compression) as the hook the armor/shin-guard reaction layer will read next.
Build clean, 62/62 ctest.

## Update 290 -- ARMOR REACTION layer (bullets + suspension drive the panels)
Built the layer the plan calls for and that U289's g_susp_travel was the hook for:
armor/shin-guard parts now REACT instead of riding rigid.
- g_armor_jolt spring: spikes when the player is hit (impulse scaled by damage,
  carrying the hit DIRECTION), and also kicks when the suspension takes a real slam
  (travel > 0.55, i.e. crater/bump hits -- rough ground rattles the panels).
- Applied per-part in the textured-model loop: atag 3 (armor) shoves along the hit
  direction, lifts off the body, and tips on its mount; atag 4 (door/hatch) hinges
  open a little under the same jolt. Tires/fans unchanged.
- VERIFIED NUMERICALLY: k=55/damp=7.5 -> 1.18Hz, zeta=0.51 (underdamped: visibly
  rattles then settles -- right for a panel), wn*dt=0.124 stable. A 30-dmg hit
  shoves the panel 65% of max travel, rattles ~6 times, back to rest in 1.1s; never
  reaches the clamp on normal hits (safety net, not the normal path).
- Manifest now carries the ENGINE atag INT (ATAG_CODE map) so wiring a model needs
  no string->int guesswork.
- SCANNER FIX: the shape-only fallback tagged Grimlock's LOGO as a wheel (flat disc)
  -- a spinning logo. Now excludes logo/badge/decal/emblem/insignia names outright
  and requires a shape-guessed tire to have meaningful span. Re-exported all 8
  textured models; Grimlock's real wheel wired into the table (part2, axis Y).
Build clean, 62/62 ctest.
HONEST: NONE of my 8 sample models have armor-tagged parts (all body/tire) -- their
materials are Chest/Head/Legs/ArmUp/ArmLo, no shin-guard/pauldron/armor names. So
the reaction is wired + verified but has nothing to act on until the movie-based
detailed models (which DO have separate armor pieces) come in. Not dead code -- it
fires the moment such a model is added -- but I can't show it working yet.

## Update 291 -- weapons/physics: projectiles + debris now respect the terrain
Went after the weapons item on the user's list and found a real class of bug: three
systems assumed a FLAT ground plane and ignored the heightfield entirely.
- LOB projectiles burst at y<=0.3 -- on terrain maps they punched THROUGH hills and
  only detonated at the flat base height. Now they test the real terrain surface and
  the blast (and its crater) lands where they actually hit.
- MISSILES had the same bug at y<=0.6, plus a hardcoded explosion height of y=1.0
  and shockwave at y=0.5. All three now sit on the sampled ground.
- DEBRIS bounced off a flat plane at its own radius, so chunks sank through hills.
  Now it rests/bounces on the terrain surface.
Each keeps the OLD behaviour exactly when there's no terrain (sample returns the
-9999 sentinel -> falls back to the original constant), so flat maps are unchanged.
This closes the loop with U288/289: shoot the ground -> real crater -> per-corner
suspension feels it when you drive through.
Also moved the terrain_sample/g_terr_res forward decls above the debris code (they
sat below it, so the first attempt failed to compile).
Build clean, 62/62 ctest.

## Update 292 -- procedural raid arenas (world gen) + lore-parser precision fix
Took up the procedural world-gen item. raid_content.py gave every boss lore + combat
TAGS but no battlefield; the engine already loads .hgt heightfields. So:
tools/gen_raid_terrain.py generates an arena per boss, DETERMINISTICALLY SEEDED by
the boss name (same boss = same ground every time) and THEMED by its tags:
  burrow/armor_pierce -> pitted crater field | quake/aoe -> fractured ridges + a
  fault line | artillery/armor -> cover ridges with firing lanes | colossus/siege ->
  wide open low-relief arena | swarm/web -> broken mounds + hollows | else dunes.
Layered value noise (pure numpy), soft rim so the arena reads enclosed, and a FLAT
LANDING ZONE carved at the centre so the drop-in is never on a cliff.
VERIFIED BY NUMBERS (no eyes on it): every arena reports slope mean/p95, traversable
fraction (<=35deg climbable), impassable fraction, landing-zone max slope, relief --
and flags any arena that's hard to cross or has a steep drop zone.
TWO REAL BUGS THIS SURFACED AND FIXED:
1. Landing zones on crater/fault themes came out at 14-16deg (dropping onto a
   slope) -- the soft LZ mask let steep features survive at its edge. Now a true
   plateau (hard flat core + wide feather). Tremor 16.1 -> 10.4deg; all 19 pass.
2. The lore parser was catching quoted DIALOGUE as boss entries -- 3 bogus bosses
   named "Let's map the nebula." Now an entry must have substance (a Classification
   line or 3+ paragraphs) and a name that reads like a name, not a sentence.
   22 "bosses" -> 19 real ones.
Output validated against the engine's own parser: 18 files parse clean (19 bosses,
2 share a cleaned name so one overwrites -- minor, noted), avg 51KB vs the 400KB cap.
Tools only; engine untouched, still 62/62.

## Update 293 -- preview drag-orbit + drag-aware menu clicks
MOUSE DRAG SPACE: the customize/builder preview was a fixed auto-turntable -- you
could only watch the bot rotate at one speed, never look at a specific side. Now:
- LEFT-DRAG orbits the model: horizontal = yaw, vertical = tilt (clamped +/-28deg so
  it never flips under or over the bot).
- MOMENTUM: releasing mid-drag throws the spin, which coasts and decays (friction
  e^-2.6t) instead of stopping dead -- a flick spins it round.
- The auto-turntable FADES BACK IN after ~2.5s idle (ramped over 1.5s) so it never
  fights the user mid-inspection, and a big tilt settles back toward eye level.
- Applies to BOTH the built bot and the imported-model preview (same orbit state),
  so drag works on whatever is on the plinth.
FIXED A CONFLICT I'D HAVE INTRODUCED: left-click also acts as Enter on the current
menu selection, so a drag would have fired a menu confirm. Clicks now resolve on
RELEASE and only when the mouse moved <6px total -- which is also just the correct
standard behaviour (press, change your mind, drag away, release = no action).
Left the mouse WHEEL alone: it already adjusts the highlighted menu value, which is
more useful than preview zoom. Dropped the zoom variable rather than ship dead code.
Build clean, 62/62 ctest.

## Update 294 -- 3D mechanical menu title, letterforms derived from the models
User: pull from Shockwave + Scorponok to build 3D text for the menu, grey, detailed
font characteristics, living idle. Did NOT invent the proportions -- MEASURED them:
  Shockwave panel thin/long ratio  0.430 -> stroke width 0.129 em
  Shockwave hand/accent ratio      0.310 -> thin stroke  0.093 em
  Scorponok 19 body sub-meshes           -> rib band count basis
  Scorponok detail ratio 0.0048          -> chamfer 0.058 em
tools/gen_text3d.py builds each glyph from beveled slab STROKES (plates, not an
extruded outline), then cuts every stroke into Scorponok-style RIB BANDS with
recessed gaps and alternating depth -- so letters read as assembled ribbed panelling.
Full A-Z 0-9 -.: set. Validated: 0 degenerate tris, all normals present, correct em
extents, ~252 tris/glyph.
Grey as asked (the models' own palette is PURPLE -- Shockwave measured RGB 170,138,197
-- so tone comes from a new plate_grey.png brushed-metal map, not the source colour).
ENGINE: one model per letter of "CYBERTRON" over the front-end menu, with a LIVING
IDLE -- a ripple travelling down the word (per-letter phase offset) lifting and
tipping each letter in turn, a slower whole-word breathe, a settle sway, and tint
brightening on the ripple crest. Hidden in the builder (bot preview owns that view).
mat_boost 1.35 so the bevels catch light like panelling.
Build clean, 62/62 ctest.
HONEST: BRAWL was not available (never in my sample set) -- user swapped to Scorponok,
which is what shipped. And I cannot SEE any of this: geometry is verified by numbers
(degenerate/normal/extent checks), but whether the letterforms actually read well and
the idle feels alive is a play-test call.

## Update 285 -- glTF primitive MODES + batch resilience (446-model run crashed)
User extracted (446 models found) but process_models died mid-run:
"cannot reshape array of size 2 into shape (3)". Cause: BOTH tools blindly did
idx.reshape(-1,3), assuming every primitive is TRIANGLES. glTF primitives carry a
mode: 0 POINTS, 1 LINES, 2 LINE_LOOP, 3 LINE_STRIP, 4 TRIANGLES(default),
5 TRIANGLE_STRIP, 6 TRIANGLE_FAN. A 2-index LINES primitive blew up the reshape.
FIX (both process_models.py + texture_export.py):
 - skip modes 0-3 (no surface geometry)
 - convert mode 5 TRIANGLE_STRIP -> triangles (with odd-triangle winding flip)
 - convert mode 6 TRIANGLE_FAN -> triangles
 - truncate ragged remainder on mode 4; guard len<3 and missing POSITION
BATCH RESILIENCE: one bad model was killing a 446-model run. _process_one and a new
module-level _export_safe now catch every exception per model -> "FAIL: <type>: msg"
row, batch continues. (Module-level so it stays picklable for the process pools.)
Tested a primitive zoo: lines/points -> clean FAIL (no crash), strip 4idx -> 2 tris,
fan 4idx -> 2 tris, ragged 4idx -> 1 tri, corrupt JSON + bad accessor -> clean FAIL,
run completes. No regression on real models (samples identical, Jazz/Grimlock 7 parts).
LIKELY BONUS: the suspiciously tiny outputs in the user's run (Elita One 32M->2K,
car--rumbleshock 55M->1K) are probably strip/fan meshes the old blind reshape
mangled into garbage triangles -- those should come out much better now.

## Update 286 -- validate-then-skip: resumable runs that don't trust bad output
User: on re-runs, skip already-done models -- but only after VALIDATING them.
Important because some existing outputs are GARBAGE from the U285 strip/fan bug
(Elita One 32M->2K, car--rumbleshock 55M->1K); a naive "file exists" skip would
freeze those broken files in place forever.
process_models.py -- validate_iqm() checks: magic, version, header-filesize vs
actual (catches truncation from an interrupted run), non-zero verts/tris, triangle
data within EOF, normalized bounds (|p|<=1.05, finite), output older than source
(stale), and SUSPICIOUSLY SPARSE (>5MB source yielding <200 tris = the old bug's
signature). Good -> "SKIP (ok, Nt)"; bad -> reprocessed with "[redone: <reason>]".
texture_export.py -- validate_export(): a model is done only if its manifest exists
AND every part it lists validates (_validate_part). Any missing/truncated/corrupt
part re-exports the whole model.
Both: --force to reprocess regardless; skipped count in the summary.
Tested: fresh run -> all processed; re-run -> all SKIP; then truncated / bad-magic /
valid-but-2-tris-from-24MB / missing-part / truncated-part all correctly REDONE with
exact reasons, and the sparse one self-healed 2t -> 7999t. --force ignores skips.
Tools-only change; engine untouched.

## Update 287 -- texture_audit.py: measure which size lever actually pays
User asked whether to switch to a texture ATLAS to exploit assets "sharing
details". Separated two conflated things: an atlas packs textures into one image
-> saves DRAW CALLS, not bytes (same pixels stored). "Sharing details" is really
DEDUPLICATION, which does save bytes. Built tools/texture_audit.py to measure
rather than guess: resolution histogram + cap projection, exact-duplicate
detection (sha1 content hash), near-duplicate detection (perceptual dHash, catches
same art re-saved at another size/encoding), then ranks the levers by measured
payoff.
Tested on the engine's 344 real textures: cap-1024 18.6%, exact dupes 5.6% (12
groups incl norm_space_back x4), near dupes 1.5%.
KEY FINDING for the user's library: their bulk is 4K source maps (the 25-40MB PNGs
in the dust output). Capping those to 1024 is 16x fewer pixels -- the three tanks
in their paste alone go 552MB -> 34MB (94%). Dedup is real but secondary.
BIGGER POINT: the 49G is SOURCE, not runtime. Their pipeline output is already
parts/ 1.8G + iqm 230M ~= 2G. The source doesn't ship.

## Update 288 -- texture FORMAT lever (measured), jpg colour + png normals
User's audit on the real library: 19.1G textures; cap-1024 saves 15.0G (79%),
exact dupes 236M (1.2%), near dupes 680M (3.5%). So the "assets share details"
hypothesis is REAL but small (4.7% combined) -- resolution dominates. Atlas would
show 0 here (draw calls, not bytes).
Measured a lever the audit didn't cover: FORMAT. On real textures, opaque maps as
JPEG q88 are 6.4x smaller than PNG (29.6MB -> 4.6MB, 84%). Quality check (PSNR):
colour 34.8dB@q88 / 37.9dB@q95 = fine; NORMAL maps 32.5-34.3dB = NOT fine (lossy
artifacts on normals cause visible shading errors). So: colour+emissive -> JPEG,
normal+spec -> PNG, anything with real alpha -> PNG.
Problem: the engine derives companion names by prefixing the diffuse name, which
assumes a shared extension. Added ex_cache_texture_alt() in iqm.c -- tries the
derived name, falls back to the other extension (.jpg<->.png). norm_/spec_/emit_
lookups now use it.
texture_export.py: --tex-format {auto,png,jpg} (auto = the split above) and
--tex-quality (default 92); convert_texture returns the real filename so the IQM
material field records the right extension; companions named off its stem.
Tested: sample export 17M -> 8.2M textures (2.1x) with correct naming. Re-exported
the 8 in-game textured models in auto format + removed 50 orphaned PNG colour maps
(-19.4MB); data/textures 85M -> 71M; all 29 parts resolve, 0 broken. Build clean,
62/62 ctest.

## Update 289 -- library_shrink.py: 51G -> target, keeping models re-processable
User wants the extracted library at ~15G (zips already wiped). Measured the split:
of 51G, parts/ output is 1.5G, source textures ~18.4G, source GEOMETRY ~31G. So
capping textures alone lands at 36.5G -- MISSES. User confirmed why: sculpting
tools emit 10-100M-tri meshes for things that never needed it.
Built tools/library_shrink.py -- two levers, dry-run by default, --target/--apply:
 1. GEOMETRY: decimate each model's source to a working budget (default 250k tris;
    ultra export only uses 32k) and rewrite as a single slim .glb that PRESERVES
    material groups, UVs and texture refs, then delete the fat .gltf/.bin/.obj.
 2. TEXTURES: downscale to --cap, colour->JPEG, normal/spec stay PNG.
Never touches parts/ iqm/ or folder names (classification metadata safe).
Bugs found + fixed during testing (round-trip verification caught all three):
 - texture_export couldn't read .glb at all (UnicodeDecodeError) -> added binary
   glTF container parsing (JSON+BIN chunks). Affects any .glb in the library too.
 - slim GLB referenced pre-conversion .png names after the texture pass renamed
   them .jpg -> resolve every map slot against what's actually on disk.
 - the GLB writer only recorded baseColor, silently DROPPING normal+emissive ->
   now writes normalTexture + emissiveTexture too.
Verified round-trip: 31.2M -> 3.6M (88%), then re-export from the SLIMMED library
gives all 7 Jazz parts with their real textures + 15 norm_/emit_ maps, scanner
still tags the tire. process_models also reads the slim .glb fine.
GUIDANCE: --tri-budget must EXCEED the top export tier -- a 20k budget measurably
caps ultra (32k); 250k leaves 7.8x headroom.

## Update 290 -- FIX: geometry pass silently did nothing (versioned-filename import)
User's run reported "geometry 0B -> 0B (saves 0B)" and missed the 15G target.
NOT a result -- a bug. library_shrink imported `texture_export` / `process_models`
by plain name, but the user keeps versioned copies (texture_export_v10.py,
process_models_v7.py, ...). The ImportError was swallowed per-model and returned
0 bytes, and since the tool only prints models >20MB, nothing surfaced. So the
LARGER of the two levers never ran.
Fixes:
 - load_helpers(): resolves helpers from beside the script, tolerating _vN
   suffixes and picking the highest version NUMERICALLY (v10 > v9, which a string
   sort gets wrong). Verified against the user's exact layout -> picks v10 + v7.
 - failure is now LOUD and up-front ("GEOMETRY SKIPPED -- ...", plus a note that
   geometry is usually the larger lever) instead of a silent zero.
 - indentation fix so the geometry loop can't run when helpers are missing.
Also found a MISSED WIN while testing: geometry was only rewritten when a model
exceeded the tri budget. But source .bin files carry attributes the pipeline never
uses (tangents, extra UV sets, joints/weights, vertex colours) -- Jazz is 13k tris
yet 6.3MB of geometry. Now ALWAYS rewrites to the slim form (POSITION + TEXCOORD_0
+ u32 indices only), and never claims a saving if the rewrite would be larger.
Dry-run estimate replaced with an EXACT payload computation (predicted 2.6M,
actual 2.5M). Result on samples: geometry 6.3M -> 2.5M (59%) with NO decimation at
all, purely attribute stripping. Round-trip re-verified: all 7 Jazz parts keep real
textures, 15 norm_/emit_ maps, scanner still tags the tire.

## Update 291 -- SAFETY: library_shrink was about to destroy geometry it never read
User's dry run hit the target (49.1G -> 5.8G) but the output contained data-loss
warnings I should not have shipped past. Implausible rows: Beverly Hills 656.8M ->
108 tris, bot--yukon-brawler 989.1M -> 76 tris, raidboss-starlord 182.2M -> 2 tris,
flyerF-117 1.8G -> 16.7k tris. A real mesh is ~40-80 bytes/triangle; these implied
100k-90M bytes/triangle, i.e. the parse captured a FRACTION of the geometry.
Why that was dangerous: `before` summed EVERY geometry file in the folder and
--apply deleted them all, while only what a single parse captured was preserved.
Fixes:
 - find_models now picks the LARGEST model file per folder (was: first match in
   os.listdir order -- a small preview/LOD .gltf could be parsed while the real
   geometry was deleted). Verified against a decoy: correctly picks the real file.
 - _consumed_files(): only the parsed model file + the buffers/.bin (or .mtl) it
   actually references are counted AND deleted. Other files in the folder are
   never ours to remove.
 - PLAUSIBILITY GUARD: if >8MB consumed and >4000 bytes/triangle parsed, refuse to
   replace and leave the source untouched. Replayed against the user's real rows:
   all 12 implausible models REFUSED, all legitimately-decimated ones proceed.
 - Suspects are now reported loudly in a dedicated block with their sizes, not
   buried in the per-model list.
 - Diagnoses likely causes: Draco / meshopt compression, sparse accessors.
Round-trip regression re-verified after the changes: 31.2M -> 12.2M, re-export
gives all 7 Jazz parts with REAL textures.
NOTE: the refused models are exactly the ones whose parse we cannot trust -- they
likely use Draco compression, which the direct accessor reader can't decode. They
keep their full source until that's handled.

## Update 292 -- library_shrink parallelized + EXACT dry run
User: shrink takes a while, parallelize it.
Both passes are per-item independent, so they now run in a ProcessPoolExecutor.
Constraint kept: textures must fully complete BEFORE geometry (geometry records
final texture names), so parallelism is WITHIN each pass, not across them.
 - module-level _tex_job / _geo_job wrappers (picklable for the pool) that carry
   the item path back with the result, since futures complete out of order.
 - _run_pool() helper: -j/--jobs (default = all cores, 1 = serial), progress as
   futures land, and a serial fallback if the pool can't start.
Verified parallel == serial: identical totals (textures 99.7M->25.8M, geometry
25.1M->10.2M) at -j1 and -j4. Parallel --apply also verified: 8 models, every
folder got its slim.glb, round-trip re-export gives 7/7 real-textured Jazz parts
and 60 norm_/emit_ maps. (Container has 1 core so no wall-clock speedup to show
here; correctness is what was testable.)
ALSO FIXED a real planning bug found while testing: the dry-run texture estimate
used a fixed ratio and was off by 34%. Tried recalibrating (0.16 -> 0.25 -> using
measured 0.154 jpeg / 0.62 png) and it kept missing, because compression depends
on IMAGE CONTENT -- no single ratio is right. Since this number drives an
IRREVERSIBLE decision, the dry run now ENCODES TO MEMORY and reports the true
size. Predicted 38.8M vs actual 39.5M (within 2%). --fast keeps the old rough
heuristic for a quick look. Parallelism is what makes exact affordable.

## Update 293 -- found the missing 27G: geometry NOTHING reads
User's v4 run: geometry counted only 3.4G, down from 30.8G in v2, and the 15G
target was missed (31.6G projected). That drop is NOT shrinkage -- it is the U291
safety fix working. v2 counted (and would have DELETED) every geometry file in a
folder; v4 counts only the parsed model file + the buffers it references. So ~27G
of geometry lives in files no parse touches, and the tool had gone silent about it.
Added report_unconsumed(): after the geometry pass it walks the tree for geometry
files not consumed by any model parse, and reports them by extension with the
biggest offenders and a ready-made find/delete command. When that pool is large
enough to close the gap, the target advice now says so instead of suggesting a
harsher --tri-budget/--cap that would degrade quality for no reason.
Almost certainly duplicate ALTERNATE-FORMAT exports (.fbx/.obj/.dae shipped beside
the .gltf by the same download) -- invisible to the pipeline, dominant on disk.
Deliberately REPORT-ONLY: it prints the command, never auto-deletes, because
"unconsumed" means "this tool can't read it", not "worthless" -- an .fbx could be
the only copy of a model whose .gltf is broken. Tested on a library seeded with
redundant .fbx/.dae: correctly attributes 17.2M, lists offenders, ties it to the
target gap.

## Update 294 -- MAJOR: multi-part OBJ models were being read one piece at a time
My U293 guess ("unconsumed geometry = duplicate alternate-format exports, safe to
delete") was WRONG and the suggested `find . -name "*.obj" -delete` would have
destroyed real model data. RETRACTED.
The 27.4G is 19,524 .obj files named model_0.obj ... model_68.obj sitting INSIDE
individual model folders -- ~45 pieces per model. They are not duplicates; they are
the models, split into parts. Proof from the user's own run: boat--Super
Portaviones consumed 22.7M == model_68.obj (22.5M) + mtl, i.e. find_models picked
the LARGEST single .obj and parsed only that; the folder has no .gltf at all.
So the pipeline has been processing ONE piece of ~69 for every multi-part OBJ
model. That also explains the many "1 textured parts" rows in the earlier
texture_export run and several SUSPECT tiny-triangle parses.
Fixed in all three tools:
 - texture_export.py: sibling_objs() gathers every .obj in the folder;
   load_obj_by_material loads them all and merges by material (renamed the
   single-file parser to _load_one_obj).
 - process_models.py: load_geometry merges all sibling .obj pieces with index
   offsetting before decimation.
 - library_shrink.py: _consumed_files and report_unconsumed treat all sibling
   .obj + their .mtl as consumed model data, so they are processed rather than
   reported as dead weight -- which also moves the 27.4G into the shrinkable pool
   and makes the 15G target reachable.
 - The unconsumed advice no longer says "delete"; it says INSPECT, and notes that
   unconsumed can mean "this tool cannot parse it" (.fbx) rather than "worthless".
Tested on a synthetic 7-piece OBJ model (10660 tris): texture_export now yields 7
textured parts (was 1), process_models sees the full mesh, library_shrink counts
it as geometry with no unconsumed report, and after --apply the model consolidates
to one slim.glb that still re-exports all 7 parts.
CONSEQUENCE FOR THE USER: parts/ produced before this fix is INCOMPLETE for every
multi-part OBJ model and should be regenerated with --force.

## Update 295 -- shared-vertex-block handling + obj_diagnose.py
User sha256'd two Sea Harrier .obj files; the hashes differ. That does NOT settle
the question: if the files share a vertex block and differ only in face lines, the
whole-file hashes differ too. Both hypotheses predict that result. The decisive
test is hashing ONLY the 'v ' lines.
Context: Sea Harrier = 442 .obj files, dozens at exactly ~16M, folder 1.9G, and it
tripped the SUSPECT guard (249964 tris from 1.9G ~= 8000 bytes/tri). Identical file
sizes across dozens of "parts" is the signature of every file re-declaring the same
vertex list -- in which case U294's sibling-merge would concatenate the same mesh
~442 times.
Implemented shared-block handling in texture_export.load_obj_by_material:
 - _obj_vertex_signature(): sha1 of the v/vt block only, no retention
 - siblings grouped by signature; each block's vertices loaded ONCE, then only
   faces merged from every file in that group (per-corner v/t keying preserved)
 - falls back to plain merging when signatures genuinely differ, so it is correct
   under BOTH structures
 - prints when de-duplication happened
Verified on a synthesised 12-file shared-block model: exports 10660 tris (correct);
naive concatenation would have produced ~127,920.
Added tools/obj_diagnose.py (READ-ONLY): hashes vertex blocks per file and reports
file count, distinct blocks, largest shared group, true unique vertices/triangles,
and a VERDICT of SHARED VERTEX BLOCK vs TRUE PARTS. --all sweeps a whole tree.
Tested against both synthetic cases: correctly identifies each.

## Update 296 -- faces were duplicated too; library-wide diagnosis
User ran obj_diagnose across the library (297 multi-obj folders). Results:
  210 TRUE PARTS  -- faces/vertex median 1.8 (healthy; merging all pieces is right)
   87 SHARED BLOCK -- faces/vertex median 36.8, max 122.9 (anomalous)
47 folders exceed 6 faces/vertex, claiming 131,017,253 faces between them.
A healthy mesh is ~2 faces per vertex, so shared-block siblings duplicate their
FACE lines as well as their vertex block -- U295 only de-duplicated vertices, and
would still have merged ~26 copies of the same triangles.
Sea Harrier concretely: 442 files, 17 distinct vertex blocks, 13,538,278 claimed
faces on 295,823 unique vertices (45.8 f/v). True size is nearer ~590k triangles.
texture_export now:
 - _obj_signature() hashes the vertex block AND the face block per file
 - files whose (vhash, fhash) pair was already seen are skipped unparsed (cheap:
   hashing beats parsing), collapsing 442 files to the handful that differ
 - _dedupe_faces() then removes triangles repeated across partially-overlapping
   files (np.unique on row-sorted indices)
 - TRUE PARTS models are unaffected -- they have distinct signatures, so every
   piece is still merged
Verified: 12 fully-duplicate files -> 10,660 tris (naive merge: 127,920); case A
7-piece model -> 10,660 tris across 7 parts; glTF path unchanged (17 parts).
obj_diagnose now reports faces/vertex and calls out duplicated faces with a true
size estimate. Also repaired a botched edit that had left two loader definitions
and an empty _load_one_obj stub in texture_export.

## Update 297 -- process_models given the same duplicate handling (was a real gap)
texture_export v13 dedupes shared vertex/face blocks, but process_models v8 still
CONCATENATED every sibling .obj -- so the standalone geometry path would have
rebuilt the Sea Harrier 442 times while the texture path was correct. Closed it:
_obj_block_signature() hashes each sibling's vertex block and face block; files
whose (vhash, fhash) pair repeats are skipped unparsed; after merging, vertices are
welded and repeated triangles removed. Verified: case B (12 fully-duplicate files)
-> 10,529 tris, case A (7 true parts) -> 10,529 tris, both correct where naive
concatenation gave 127,920. glTF path unchanged (3 ok, 725K).
Note library_shrink's geometry pass already routed through texture_export's loader
via load_helpers(), so it was correct; this was specifically the standalone
process_models path.
RUN ORDER for the user's latest set (helpers auto-resolve to highest _vN):
  1. library_shrink_vN . --target 15G          (exact dry run, review suspects)
  2. library_shrink_vN . --apply -j <cores>    (destructive; try one category first)
  3. texture_export_vN . --out-iqm parts --out-tex parts/textures --cap 512 \
       --tex-format auto --force                (--force: prior parts/ is wrong)
  4. process_models_vN . --quality ultra --out-iqm iqm
  5. model_metadata_v1.py . --out metadata.json

## Update 298 -- guard false-positives: judged on decimated tris and skipped bytes
User's v6 run hit target (49.1G -> 14.0G) and the dedup worked dramatically (e.g.
"merged 4290 .obj pieces; 4127 duplicate mesh file(s) skipped, 163 distinct vertex
blocks"). But suspects grew 5 -> 11 (8.2G held back), and three read exactly
"250000 tris" -- which IS the --tri-budget, i.e. those models parsed MORE than the
budget and were decimated on purpose. Two compounding errors in the guard:
 1. it divided by tris KEPT (post-decimation), so ANY model hitting the budget
    looked starved;
 2. it divided by ALL consumed bytes, including the sibling .obj files the loader
    deliberately SKIPPED as duplicates -- 425 of the Sea Harrier's 442 files were
    skipped yet their 1.9G stayed in the denominator.
Fixed: the guard now uses triangles actually PARSED (pre-decimation) over the bytes
actually READ (unique-signature files only, recomputed with te._obj_signature).
Message updated to "N tris parsed from X read".
Verified: duplicate-heavy case B now passes (8.5M -> 305.6K, no suspect), while a
synthetic genuinely-partial parse (30MB .bin claiming 1 triangle) is still refused.

## Update 299 -- clean plan (49.1G -> 4.5G, 0 suspects) + undershoot warning
User's v7 run: all 11 suspects cleared, geometry 30.8G -> 1.0G, projected
49.1G -> 4.5G (91%), target met with large margin. Sea Harrier now parses
516,825 tris from its 17 real vertex blocks -- matching the diagnostic's estimate
and confirming the dedup chain end to end.
Reviewed the rows for quality risk. Important distinction: rows reading "N tris,
attrs stripped" were never decimated (that IS the model's true size -- skynok 7588,
gigadrill 9292 are simply small models, nothing lost). But SciFi-Corridor went
1,510,373 -> 23,322 against a 250k budget: a genuine 10x undershoot, landing below
even the 32k ultra export tier, which would silently cap quality downstream.
Cause is proportional per-material-group budgeting combined with _decimate's
plateau/vertex-clustering fallback returning fewer triangles than requested.
Added an UNDERSHOT flag (kept < 50% of budget) plus a summary block listing
affected models, separate from the SUSPECT block, with the suggestion to re-run
those with a higher --tri-budget. Not a refusal -- the result is still usable, it
just should not pass silently.
NOTE: the user's run used --fast, so its 3.3G texture figure is the rough estimate;
the exact figure was 4.6G. Real total lands near 5.6G, still far inside target.

## Update 300 -- install_models.py: model import is now a command, not a C edit
User: "we're going to have to do a pipeline where I can always import models from
folders and merge new changes into the game" -- plus they can't ship hundreds of MB
to me and can't verify in-game themselves. The bottleneck was that every import
required ME to hand-edit g_imported_files/names, g_tmodels and the four default
arrays in game.c.
Built tools/install_models.py: takes process_models output (--iqm-dir) and/or
texture_export output (--parts-dir), copies meshes + textures into engine_src/data,
and GENERATES engine_src/imported_models.h containing EX_N_IMPORTED, the file/name
arrays, the four orientation/scale default arrays, and the full g_tmodels table
(with scanner anim tags/axes carried through from the manifests).
KEY PROPERTY -- TUNING SURVIVES RE-IMPORT: per-model rx/ry/rz/scale live in
model_tuning.json keyed by MODEL NAME, not array index, so re-importing, adding or
reordering models never loses hand-tuned values. Unknown models fall back to the
measured library defaults (ry=-90, scale=0.33). Verified: seeded jazz=0.185 and
grimlock=0.314, re-ran, both preserved while the third model took the default.
Engine change: game.c no longer holds model tables. The ex_tmodel_t typedef moved
above a new #include "imported_models.h". To guarantee zero behaviour change, the
header was first generated FROM the existing 48-model tables -- build clean,
62/62 ctest, identical data.
Also: accepts textures/ or tex/ beside the parts dir (was silently copying 0
textures), and clamps models exceeding EX_MAX_TPARTS=8 with a loud warning naming
them, since many library models split into far more parts than 8.
Display names widened 20 -> 28 chars, which fixes the long-standing
"TRANSFORMERS 5 OPTIM" browser collision.

## Update 301 -- measured the Scorponok "detail" structure; part-library findings
User redirected: compression is done, the goal is procedurally raising models to
Scorponok-level detail using low-poly shells + bot-mode constraints + scanner
metadata. Went to the data before proposing anything.
WHAT SCORPONOK ACTUALLY IS (measured):
 - 19 baked submeshes resolve to 9 UNIQUE PART TYPES under rigid transform; one leg
   segment appears 6x (mirrored/rotated radially), another 6x. Not glTF instancing
   -- transforms are baked into the vertices.
 - So the detail aesthetic IS a part library + placement rules, exactly the model
   the user proposed. Confirmed rather than assumed.
Built tools/part_harvest.py: splits models into connected components, computes a
rotation/mirror/translation-invariant radial signature, clusters into part types,
reports within- vs cross-model reuse, optionally emits partlib.json.
RESULTS across 9 sample models (193 components -> 70 unique types):
 - reused WITHIN a model: 40 types; 56% of placed triangles are repeated geometry
 - shared ACROSS models: **0**. Honest negative -- a shared part library gives NO
   further compression, because different models genuinely have different parts.
   That answers the user's "any more compression wins" question: no, not this way.
DETAIL SIGNATURE (what a generator must actually hit) -- first attempt was WRONG:
component density is inverted (arcee 105 comps/1k vs scorponok 22) because low
-detail models are a cloud of 2-triangle fragments. The real metric is SUBSTANTIAL
structural members (>=50 tris):
   scorponok 479 members / 75% of tris structural   <- target
   shockwave  58 / 40%,  jazz 45 / 52%,  arcee 32 / 31%,  soldier 13 / 27%
So raising a low-detail model means adding ~450 structural members, not subdividing
its shell. The harvest catalog (70 types, incl. Scorponok's 36) is the raw material;
plan.md's Rule 3 (vehicle topology -> skeleton), Rule 7 (hollow interiors) and
Rule 9 (skeleton first) are the placement rules; the scanner's tire/fan/thruster
tags give the anchor points.

## Update 302 -- CORRECTION: cross-model reuse finding was sample-biased + too strict
User pushed back: the harvest ran on 9 mostly-character models, not the tank/flyer/
car classes where fins, tires and thrusters would actually recur -- and their goal
is parts with VARIATIONS, not exact duplicates. Both points land.
Ran a tolerance sweep on the same 9 models:
   tol=3 -> 75 types, 0 shared      tol=1 -> 66 types, 0 shared
   tol=2 -> 70 types, 0 shared      tol=0 -> 21 types, 13 SHARED across models
So "0 cross-model reuse" was partly an artifact of strict exact-shape matching.
Exact reuse across models really is ~0, but at family-level tolerance parts DO
cluster across models -- which is the granularity a variation-based generator wants
anyway. Corrected the claim; --tol default lowered 2 -> 1 and documented as an
explicit exact..family dial.
Caveat retained: 21 types at tol=0 is likely over-merging on a 9-model sample; the
meaningful experiment is per-CLASS on the real library (vehicles/tanks,
vehicles/flyers) where same-function parts should genuinely cluster.

## Update 303 -- continuous tolerance + sweep; my U302 correction was over-stated
User's tanks run was going with the DEFAULT tolerance, which was the setting that
reports ~0 cross-model sharing -- flagged before they drew a conclusion from it.
Then found the dial itself was broken: --tol was an integer decimal applied to a
0..1 quantile vector, so tol=1 left 11 distinct values, tol=0 left only 2. There
was NO usable setting between "near-identical" and "everything merges" -- which
means U302's "13 shared at tol=0" was OVER-MERGING, not evidence of families.
Replaced with a continuous quantisation step (--tol float) and added --sweep.
Sweep on the 9-model sample:
   0.01 -> 70 types, 0 shared      0.12 -> 63 types, 0 shared
   0.03 -> 68, 0                   0.20 -> 64, 0
   0.05 -> 68, 0                   0.30 -> 54, 5 shared (9%)
So cross-model sharing stays at ZERO right up to "same rough silhouette" (0.30).
The original U301 finding was more robust than my U302 correction claimed; I
retract the over-correction. Honest position now: on CHARACTER models there is no
cross-model part reuse at any meaningful tolerance. The user's real point still
stands untested -- same-class vehicles (tanks/flyers/cars) are where same-function
parts should cluster, and that experiment is running on the real library.
Guidance: use --sweep per class and pick the tolerance from the data rather than
defaulting.

## Update 304 -- part naming + residual measurement
User asked the two right design questions: can parts be NAMED by how they match,
and can unique detail be preserved as residual bits on top of a shared family part.
Both built into part_harvest.
NAMING: describe() extracts measurable features (roundness, flatness, elongation,
dominant axis, relative height in the model, repetition count); name_part() maps
them to semantic labels -- wheel, roadwheel, disc, tread_link, barrel, strut,
panel, cylinder, turret_greeble, underside_greeble, block. Deliberately falls back
to "block" rather than inventing a label it cannot justify. On the samples: 65
types named across 9 categories, with "wheel" at 6 types / 77 placements.
RESIDUALS: residual() measures how far an instance deviates from its family
exemplar using the same rotation/mirror-invariant radial profile the clustering
uses -- i.e. it measures exactly what the family signature discards. That is the
per-instance delta a generator must keep to preserve unique detail.
Sanity-checked the metric before trusting it: identical=0.0000, two different
parts=0.0454, 2% noise=0.0343, 15% squash=0.0053. It responds.
FINDING: every current family has residual 0.000 -- because within a model the
repeats are EXACT rigid copies (consistent with Scorponok's 6 identical legs).
So residual storage costs nothing today; it only starts to matter once families
span multiple models, which is exactly the tanks/flyers experiment.
partlib.json now carries name, instances, residual_mean and residual_max per part
type alongside the placements.

## Update 305 -- part_harvest optimised (81x on fragmented models)
User reported it was super slow. Profiled rather than guessed: load 0.27s,
signatures 0.24s, components() 1.31s -- and components() was the one that scaled
badly. Two real defects:
 1. pure-Python union-find looping over every triangle with a find() call per corner
 2. worse: a full O(len(V)) remap array allocated INSIDE the per-component loop, so
    cost grew as components x vertices. A tank with 500k verts and 15k components
    was doing billions of operations.
Rewrote: labels from scipy connected_components (vectorised, with a vectorised
pointer-jumping union-find fallback if scipy is missing), grouping via a single
argsort + searchsorted, and ONE reusable remap buffer written only at each
component's own vertices.
Also pushed min_tris DOWN into the splitter so components destined to be discarded
are never built, and added -j parallel workers across models.
Measured (synthetic fragmented meshes, scipy import warmed):
   1,200 comps: 0.13s -> 0.010s   13x
   3,000 comps: 0.66s -> 0.027s   24x
   6,000 comps: 2.43s -> 0.044s   55x
  12,000 comps: 8.90s -> 0.110s   81x     <- old cost grows quadratically
Scorponok components(): 1.31s -> 0.78s, and 0.06s with min_tris pushed down (22x).
Full 9-model sample run now completes in under a second.
Correctness preserved: -j1 and -j4 both give 193 components -> 65 types, 37 reused
within, 0 shared across, 59% repeated geometry -- identical to pre-optimisation.

## Update 306 -- tank harvest settles the architecture: functional vs detail parts
User ran the harvester across 26 real tanks (3625 components -> 879 types) and sent
the library back. The result answers the shared-parts question cleanly, and it is
NOT the flat "no reuse" I reported from the character sample.
REUSE SPLITS BY FUNCTION -- the naming turned out to be the discriminator:
    tread_link   41 types -> 1253 placements   30.6x reuse
    wheel        62 types ->  701 placements   11.3x
    strut        41 types ->  404 placements    9.9x
    panel        75 ->  283   3.8x
    block       201 ->  313   1.6x
    disc        158 ->  212   1.3x
    greebles    198 ->  305   ~1.5x
  functional (tread/wheel/strut): 144 types, 2358 placements = 16.4x reuse
  detail (block/disc/greebles)  : 557 types,  830 placements =  1.5x reuse
CROSS-MODEL: 5 shared types at tol 0.08 -- and ALL FIVE are wheels and tread_links.
Not one block or greeble. Sweep: 5 shared @0.08, 48 @0.20 (7%), 145 @0.30 (36%).
So the architecture the user proposed is confirmed, with a sharp boundary:
  - FUNCTIONAL parts (running gear, struts, barrels) are genuinely shareable across
    vehicles of a class -> a shared library pays here, and reuse is 16x within
    models on top of that.
  - DETAIL parts (greebles, blocks, panels) are ~unique per model at 1.5x -> these
    cannot be shared and must be harvested per-model or generated. They are exactly
    the "residual bits" the user asked about.
48% of placed triangles are repeated geometry (4.53M -> 2.37M unique).
Residuals still read 0.000 because at tol 0.08 families are exact repeats; the
variation lives BETWEEN models, so the meaningful residual measurement is at
tol 0.20-0.30 where functional families span vehicles.

## Update 307 -- 178-vehicle harvest: shared library CONFIRMED at scale
User ran the harvester across the full vehicle set (178 models), bots (81) and tanks
(25) at tol 0.20 and sent all three libraries. My earlier "no cross-model reuse"
conclusion was an artefact of a 9-model character sample; at scale it inverts.
VEHICLES: 26,680 components -> 5,873 types; 3,478 reused within a model;
1,776 SHARED ACROSS MODELS (30% of types). 74% of placed triangles are repeats
(29.2M -> 7.5M unique). One wheel type appears in 61 different vehicles across 691
placements -- the same signature that showed in 6 tanks.
CROSS-CLASS (the important one for bot generation):
  656 of 1108 bot part types (59%) ALSO occur in vehicles.
  100% of tank types occur in the vehicle library (sanity check -- signature stable).
  149 types occur in bots AND vehicles AND tanks.
That is direct evidence for the user's claim that bot modes are skeletons with
vehicle detail extrapolated onto them -- the parts genuinely are the same stock.
FUNCTIONAL vs DETAIL holds at scale:
  functional (wheel/tread/strut/roadwheel/barrel): 2,233 types, 18,795 placements,
    8.4x reuse, present in 2.1 models on average
  detail (block/disc/panel/greebles): 3,640 types, 7,885 placements, 2.2x reuse,
    1.3 models -- essentially per-model
CATALOG COVERAGE (how big a shared library must be):
  top 25 types -> 19% of all placements      top 500 -> 53%
  top 100      -> 32%                        top 1000 -> 65%
  top 250      -> 43%                        top 2000 -> 78%
So a ~500-part catalog covers half of everything placed across 178 vehicles.
RESIDUALS finally non-zero at tol 0.20, and they rank exactly as theory predicts:
  wheel 0.0181 (221 types >0.02), tread_link 0.0154 (152), then greebles ~0.007,
  down to barrel 0.0031. Running gear is the same part with real per-vehicle
  variation -- the family+residual case the user described.

## Update 308 -- part_harvest emits real geometry + learns placement rules
The library was an index with no meshes, so the catalog could not actually build
anything. Extended part_harvest:
 --emit N  writes the exemplar MESH for the top N part types (ranked by placement
   count) as BOTH .obj (portable) and .iqm (engine-ready, normalised into the unit
   slot so parts are comparable). Files are named part_<name>_<sig8>.* so they are
   addressable by function, and partlib.json now carries a "mesh" field per type.
 PLACEMENT RULES: placement_rules() derives, per part category, the 25-75%
   relative position within the model bounding box across every placement -- the
   rules a generator needs, learned from data rather than hand-authored. Stored in
   partlib.json under "rules" and printed. Sample output ordered by height:
     underside_greeble y 0.14, roadwheel 0.18-0.21, block 0.43-0.68,
     wheel 0.70-0.74, disc 0.79-0.83, turret_greeble 0.87-0.89
   i.e. it independently recovers "greebles underneath sit low, turret detail sits
   high", which is exactly the placement logic the generator would otherwise guess.
Verified: 20/20 emitted IQMs load with valid normalised bounds, 20/20 OBJs
re-parse, 20 types carry mesh references, rules produced for 9 categories.
CAVEAT recorded: naming is shape-based, so Scorponok's high body discs classify as
"wheel" (y 0.70) rather than running gear. The category is a shape family, not a
guaranteed semantic role -- position stats should be read per class, not globally.

## Update 309 -- both catalogs harvested; greeble pass is the more useful one
User ran both emit passes over 178-188 vehicles and uploaded the result (55MB,
1000 .obj + 1000 .iqm exemplars, two indexes).
  vehiclecat (--min-tris 200):  5,873 types, 178 models,  26,680 placements
  greeblecat (--min-tris  50): 10,375 types, 188 models,  88,618 placements
The greeble pass is a strict SUPERSET (all 5,873 structural types present) plus
4,502 NEW small parts the 200 threshold discarded -- and it is better on every
axis that matters:
  coverage per catalog entry: top500 = 59% (greeble) vs 53% (structural)
  placements: 3.3x more (88,618 vs 26,680)
  median exemplar: 108 tris vs 391 -- cheap to place in bulk
  top-500 exemplars account for 9.7M placed triangles vs 7.8M
That matches the U301 finding that Scorponok's detail is many SUBSTANTIAL small
members, not a denser shell -- the greeble catalog is the detail library.
Emitted 500 greeble exemplars: 226 wheel, 194 tread_link, 32 strut, 29 panel,
19 barrel. Workhorses: part_strut_03e37adb placed 8,625 times, part_wheel_ffcea5d6
7,252 times (the signature previously seen in 61 vehicles).
327 of 500 exemplars have residual > 0.02, i.e. genuine per-instance variation
needing residual storage; 173 are true repeats. The family+residual architecture
is now backed by counts.
PLACEMENT RULES over 188 vehicles came out physically sensible and ordered by
height: strut 0.10-0.32 (chassis), underside_greeble 0.15-0.38, roadwheel
0.21-0.37, tread/wheel 0.26-0.67, barrel 0.35-0.76, turret_greeble 0.58-0.87.
Validation: 60/60 randomly sampled emitted IQMs load with finite, normalised
bounds; tris 52..2360.

## Update 310 -- greeble UVs fixed; catalog is installable in-game today
User asked whether models/textures can go in the game yet, and correctly predicted
two blockers. Checked both:
 1. CONFIRMED: emitted part exemplars had UV range 0.000..0.000 -- no unwrap, so any
    texture collapses to one texel. Fixed with box_uvs(): per-face dominant-axis
    projection with per-face vertex duplication for clean seams, written through
    texture_export's UV-aware IQM writer. Emitted parts now carry UV 0.000..1.000.
 2. CONFIRMED: robot mode is not derivable from the vehicle library -- that remains
    the generator's job, not something the harvest can supply.
IN-GAME PATH WORKS TODAY, no new tooling needed: install_models.py --iqm-dir points
at any folder of IQMs, so the harvested catalog installs directly as browsable
models. Verified end to end on a copy: 40 greeble exemplars installed, header
generated (EX_N_IMPORTED 40), engine builds clean.
Agreed with the user's sequencing: stop processing, get results on screen, and
return to the processor once there is visual confirmation. Noted their combat
"relic" concept (rule-driven part swapping + transform animation on terrain/damage
events, and LLM-generated bots from player text) as a design direction that the
part catalog + placement rules would directly feed -- recorded, not started.

## Update 311 -- install_models refuses engines that predate the generated header
User asked what paths to use going from models/round-2-extras to their
cybertron-cmake-project. Spotted that their checkout is from Jul 20 (signatures-88
era) -- it still has hardcoded model tables in game.c and NO
#include "imported_models.h" (that landed in signatures-92). Running the installer
against it would have written a header nothing reads: models copied, tables
generated, zero visible change, no error. A silent no-op that would be painful to
diagnose.
Added a pre-flight check: install_models now reads engine_src/game.c and refuses
with an explicit message + exit 2 if the include is missing, naming signatures-92
as the minimum. Also warns if game.c is absent (wrong --engine path).
Verified both directions: refuses a synthetic old engine, proceeds on the current one.

## Update 312 -- bundle: engine + 24 installed greebles + full harvested catalog
User asked for a package like the old signature exports, with the greebles included.
Two REAL bugs found while assembling it (both now fixed in install_models.py):
 1. SameFileError when --parts-dir/--iqm-dir already points inside engine_src/data
    (a legitimate re-install). Guarded with a samefile check.
 2. Names were derived from the filename WITHOUT stripping our own "imported_"
    prefix, so re-installing this tool's own output produced keys like
    "imported_foo" that never matched tuning keyed on "foo" -- tuning silently
    reverted to defaults ("0 match models being installed"). Now stripped on
    ingest, making re-install idempotent.
Rather than risk the tested 48-model + 10-textured setup on a full reinstall, I
restored imported_models.h from signatures-92 and APPENDED 24 curated greeble
exemplars (5 each of strut/wheel/tread_link/panel, 4 barrel -- a category spread,
picked by placement count). EX_N_IMPORTED 48 -> 72, all six arrays verified at 72
entries, greeble meshes copied into data/, defaults rx/ry/rz=0 scale=0.6.
Build clean, 62/62 ctest.
Bundled catalog/ (62MB): greeblecat + vehiclecat with their 500 exemplar meshes
each, plus the tank/vehicle/bot indexes, and a README explaining partlib.json's
name/instances/residual/mesh fields and the learned `rules` block.

## Update 313 -- orbiting-tire pivot fix + first BOT GENERATOR
User feedback on the bundle: greeble parts are "very basic" (expected -- they are
harvested primitives), existing models survived, but tires ORBIT instead of
rotating on their own axes, plus some over-glowy bits.
PIVOT BUG (real, and it would have poisoned the generator too): parts are
normalised as a GROUP, so each part carries its offset inside its own vertices and
all parts are positioned at the model centre. Rotating a part therefore swung it
around the model origin. Fixed properly:
 - texture_export now CENTRES any part tagged tire/fan on its own centroid and
   writes the offset (3 floats) into the manifest
 - ex_tmodel_t gained float off[EX_MAX_TPARTS][3]; the engine re-applies the offset
   rotated by body yaw, so a spun part turns on its own axis
 - re-exported all 8 textured models at --cap 1024; rebuilt g_tmodels with pivots
   using the known index map (a name-similarity match had silently dropped 2 of 8)
EX_MAX_TPARTS raised 8 -> 24: a humanoid needs 17 slots and was being clamped.
BOT GENERATOR (tools/bot_generate.py): assembles a robot from catalog parts using
skeleton templates that follow plan.md Rule 3 -- human, knight (tank: wider,
heavier, lower), spider (radial 6-leg ring replacing the leg chain). Each slot
declares a box and preferred part categories; parts are uniformly scaled to fit
(never distorted), box-UV'd, and written as per-part IQMs + a manifest that
install_models consumes directly. Verified end to end: 17 parts generated from the
real greeble catalog, installed, engine builds clean, all 17 present in the table.
FINDING for the next catalog run: the emitted 500 exemplars are only 5 categories
(226 wheel, 194 tread_link, 32 strut, 29 panel, 19 barrel) because --emit ranks by
placement count -- no block/disc reached the cut, so the head slot fell back. A
category-balanced emit (or --emit 2000) would give the generator better material.

## Update 314 -- HONEST: part-assembly will not reach reference quality; fixes
User asked directly whether this is getting close to the reference models or is
just building something else. Answer given plainly: it is something else. Bolting
harvested primitives together produces a proportioned MANNEQUIN, not a character.
Reference models (Jazz, Grimlock, Scorponok) are continuous sculpted surfaces with
a coherent silhouette; a pile of struts and panels has no surface continuity and
more parts will not fix that. Recorded so the approach is not oversold.
BETTER PATH, which is also closer to the user's own original framing: do not
generate from scratch -- START from a real bot model (jazz, grimlock, soldier,
zeta prime, wfc optimus are all real bots already in the library) and ADD greeble
detail onto it. That inherits the silhouette for free and only needs the detail
density raised (U301 target: ~450 structural members, 75% of tris structural).
Fixed this turn:
 - VIEWER CAMERA INSIDE THE MODEL: my bug, not the camera. Generated bots were laid
   out in a 0..1 body box and written unnormalised, so they sat at half scale and
   offset upward (bounds y 0..0.97, centre 0.48). Now the assembly is normalised as
   a GROUP into the engine's unit slot -- bounds -1..1, centre 0,0,0.
 - SPIDER LEGS LOOKED LIKE MISSILES: the spider skeleton listed "barrel" as a leg
   category and barrels are missile-shaped. Changed leg slots to strut/panel.
CONFIRMED for the user: bot selection already works from the customize screen --
F3 toggles the imported browser, [ and ] cycle, handled in menu_input so it works
in-menu as well as in-match. No new menu needed.

## Update 315 -- category-balanced emission (--emit-per-category)
User wants to harvest ribs/spine from a human skeleton reference and detail from
the best-detail donors (Brawl, Scorponok, Shockwave) to convert WFC hollow-body
bots (Jazz, Optimus, Sentinel, Grimlock) to full detail.
Blocker found first: --emit ranks purely by placement count, so the 500 emitted
exemplars were 226 wheel / 194 tread_link / 32 strut / 29 panel / 19 barrel and
contained ZERO block or disc -- which is why the generator put a tread_link on a
head. Added --emit-per-category N: emits up to N exemplars of EACH named category.
Verified on the samples: balanced draw across all 10 categories (block 20, disc 20,
tread_link 20, turret_greeble 20, wheel 16, roadwheel 13, strut 12,
underside_greeble 12, panel 8, barrel 5) instead of five wheel-dominated ones.
Noted for the skeleton harvest: a human skeleton model's ribs and vertebrae ARE its
geometry, so harvesting it directly yields rib/spine parts -- exactly the material
needed to convert a hollow WFC chest into a ribbed one. Use a lower --min-tris
(~30) there since individual ribs are small.

## Update 316 -- obj_diagnose silently printed nothing on non-.obj folders
User ran obj_diagnose on references/animation-reference--male_skeleton and got NO
output at all. Cause: it only inspected .obj files directly inside the named
folder, and their models sit one level deeper (and many are .gltf, not .obj), so it
hit the "<2 objs" branch and returned. Worse, that branch's message was easy to
miss and said nothing about what WAS there.
Now it walks the folder and reports exactly what it found and where -- counts of
.obj / .gltf-.glb / .fbx-.dae-.stl per subfolder, plus either "re-run on <path>"
when a multi-obj model is found deeper, or "single-file model -- harvest it
directly" when there is nothing to diagnose. Also guards a non-directory argument.
Noted: the user's find turned up no scorponok under round-2-extras (their copy is
spelled "scorponok_trnasformers..." with a transposition, and may sit outside that
tree), so donor curation needs a spelling-tolerant search.

## Update 317 -- UNWELDED meshes silently yielded zero parts (skeleton harvest)
User's skeleton harvest returned "115,386 comps, 0 kept, 0 unique types" -- total
failure with no error. Diagnosed: the mesh is UNWELDED (every triangle carries its
own three vertices), so no triangles share a vertex index, connected-components
sees each triangle as its own island, and nothing survives any min-tris filter.
Reproduced exactly by exploding Jazz (8,846 verts -> 31,980, same 10,660 tris):
components() returned 0 kept where the welded original returns 80.
Fixed: components() now welds by position (tol 1e-5) BEFORE labelling, collapsing
duplicate vertices and dropping the degenerate faces that welding creates. Verified
the exploded mesh now returns 80 kept -- identical to the welded control. weld=True
is the default; pass weld=False only for meshes known to be welded.
This affects any export with split vertices, not just the skeleton, so earlier
harvests may have under-reported parts on some models.
User's two working catalogs analysed:
  detailcat   293 types / 222 meshes (scorponok 479 kept + shockwave 159 +
              yukon-brawler 185); exemplars 50..15,847 tris, median 95
  roboskelcat 215 types / 141 meshes from the Terminator endoskeleton;
              exemplars 30..663 tris, median 58 -- small, dense, mechanical,
              which is the right character for chest-cavity ribbing
Notable: yukon-brawler parsed fine here (185 kept, 106 types) despite tripping the
suspect guard during library_shrink -- different threshold, different pass.

## Update 318 -- detail_apply.py: ribs/spine INTO an existing bot (the right approach)
Skeleton harvest works after the weld fix: 228 kept, 115 types, 110 exemplars.
With all three catalogs in hand, built the pass that abandons from-scratch
generation and instead adds structure to a REAL bot, keeping its silhouette.
tools/detail_apply.py:
 - loads the target, normalises to the engine unit slot, and MEASURES its current
   structural members (>=50 tris) against the Scorponok reference (479 / 75%)
 - CAVITY FINDING is geometric, not guessed: slices the chest band and measures
   the interior half-width/half-depth per slice from the actual shell walls, so
   ribs are sized to the body instead of poking through it
 - builds a spine column up the cavity centre, then mirrored rib PAIRS curving out
   (sin-weighted so the cage is widest mid-chest, rotated on Z per side)
 - scatters surface greebles anchored to real shell vertices, choosing category by
   the catalog's own learned height rules, pulled 6% toward the centre so they sit
   ON the shell
 - KEEPS THE SHELL as part 0 -- the silhouette is inherited, not invented
BATCHING: 127 placed pieces would blow EX_MAX_TPARTS=24 and cost 127 draw calls.
Everything shares one texture, so pieces accumulate into buckets (shell/frame/
greeble) and merge into a handful of meshes, splitting only past a 60k-vertex
budget. 127 pieces -> 3 meshes.
Verified on Jazz + roboskelcat: 42 -> ~168 structural members, 10,660 -> 24,080
tris, frame bounds (-0.50..0.53, -0.09..0.56) sit INSIDE shell bounds (-1..1),
combined bounds equal shell bounds (nothing protrudes), installs as 3 parts,
engine builds clean.
HONEST: 168 of the 479 target. Raising --greebles closes the gap numerically but
risks visual noise, and I cannot see the result -- that call needs the user's eyes.

## Update 319 -- showcase camera framing + part limit raised to 128
User reported (twice) the character viewer camera is too high and too close, and
that Jazz showed no change. The last one had a simple cause: their command used my
literal placeholder path "bots/.../fall_of_cybertron_jazz/scene.gltf" and pointed
--engine at the PROJECT ROOT rather than engine_src, so detail_apply errored and
install_models refused -- nothing was ever installed. My tool warned correctly about
the engine path; the placeholder path was my fault for writing it that way.
CAMERA (real bug): the preview scales imported models to ~4 units tall centred at
y=2, but the camera used the BUILT-BOT framing (focus_y 2.4, dist 9, elevation
0.28), putting the eye at y=4.92 looking down at y=2.1 -- above and too near.
Added adaptive framing: when the imported showcase is active, focus_y = half the
model height, dist = 3x model height, and a new g_prev_elev drops from 0.28 to 0.06
so the view is near eye level. Eye 4.92 -> 2.72, dist 9 -> 12, target 2.10 -> 2.00.
Built-bot framing is untouched.
EX_MAX_TPARTS 24 -> 128, since the user wants full-fledged showcase models and the
chest pass alone places 127 pieces. Cost measured: 28 bytes/part = 3.5KB per
textured model, 0.3MB for 100 models. Build clean, 62/62.
Batching in detail_apply stays (it is still the right default for draw calls), but
unbatched detail now fits the table if a showcase model wants every piece separate.

## Update 320 -- detail_apply now fails helpfully on a missing target
Third consecutive run died on a raw FileNotFoundError deep in the glTF loader:
their `find . -iname "*fall_of_cybertron*jazz*"` matched nothing, so $JAZZ was empty
and the target became "/scene.gltf". The traceback said nothing actionable.
Added a pre-flight check: detail_apply verifies the target file exists and, if not,
names the problem (specifically calling out the empty-shell-variable signature when
the path starts with "/"), prints ready-to-run find commands including a parent-dir
search, and lists any model files that DO sit beside the given path. Exits 2.
Verified both directions: "/scene.gltf" now explains itself, a real target still
runs (Jazz 42 -> ~168 members, 127 pieces, 3 meshes).
NOTE: Jazz is evidently not under round-2-extras -- same situation as scorponok,
which turned out to live one level up. The user's library layout differs from the
sample set I hold, so path discovery has to come from their filesystem, not my
assumptions.

## Update 321 -- my find command was wrong (-iname vs -ipath)
User's searches kept returning nothing for Jazz. The model IS there:
  .../blackstar/models/fall_of_cybertron_jazz/scene.gltf
My find was the bug: `-iname "*jazz*" -name "*.gltf"` requires the FILENAME to
contain both "jazz" and ".gltf", but the file is scene.gltf -- only the DIRECTORY
carries the character name. Correct form uses -ipath, which matches the whole path:
    find ~ -ipath '*jazz*' \( -name '*.gltf' -o -name '*.glb' -o -name '*.obj' \)
Demonstrated both against a synthetic tree: -iname finds nothing, -ipath finds it.
Corrected the hint printed by detail_apply's missing-target guard so it teaches the
right pattern rather than repeating my mistake.
Also worth noting the models/ dir (one level above round-2-extras) holds the
unpacked originals -- that is where the WFC-era bots live, not in round-2-extras.

## Update 322 -- the ribs were INVISIBLE: shell was never gutted
User's screenshot: the model looks like stock Jazz plus scattered floating
fragments -- no gutted chest, no ribs. Diagnosed both faults, both mine:
 1. I kept the shell 100% INTACT and built ribs inside it. An opaque chest means
    nothing inside is ever visible. The "detail" was there and unseeable. Measured:
    1,166 front-facing faces sit in the chest band, covering the whole cavity.
 2. The only visible additions were the 90 scattered greebles -- unoriented, small,
    anchored but not aligned to the surface, so they read as floating debris. That
    is exactly what the screenshot shows.
Fixed:
 - gut_shell(): removes the outward-facing shell panel over the cavity so the
   interior actually reads. --gut front (default) opens the chest; 'both' opens
   front and back; 'none' restores the old closed behaviour.
 - --greebles now defaults to 0. The scatter was the debris; it should be enabled
   deliberately after the cavity work looks right, and it needs normal-alignment
   before it will look attached rather than sprinkled.
Verified on Jazz: 1,166 of 10,660 faces removed (11%), cavity open; frame occupies
y -0.09..0.56 with 100% of its vertices inside the cut band, reaching z=0.14 behind
the shell front at z=0.36 -- i.e. sitting in the opening, visible, not protruding.
43 pieces (shell + 14 spine + 28 ribs), 2 meshes, 42 -> ~84 structural members.
Fewer members than the 168 of the greeble version, but they are members you can SEE.

## Update 323 -- built a VISIBILITY test; it disproves my own approach
User: patched but still cannot see into the chest; try a new approach and verify on
my own. Two findings, both from measurement rather than guessing.
INSTALL BUG (why nothing ever changed in game): install_models only WARNED when
game.c was missing, then wrote anyway. The user ran --engine engine_src from
round-2-extras, so it created a stray engine_src/imported_models.h there and the
real bundle was never touched -- twice. Missing game.c is now a hard error (exit 2).
Then built a real check instead of shipping blind: a front-facing visibility raster
that asks, per grid cell over the chest, WHICH mesh owns the frontmost surface.
It disproved my own work:
  closed shell : shell frontmost 97.6% of the torso window
  after gutting: shell frontmost 97.6%  -- no change whatsoever
Chased that and found TWO more of my bugs plus a fatal assumption:
 1. The chest band spanned the whole ARM SPAN (body 2.00 wide in X), so the
    "cavity" I measured (half-width 0.36..0.94) was arms, not torso, and the cut
    was removing shell off the limbs. Added torso_extent(): densest cluster around
    the centre line -> Jazz torso half-width 0.17 vs body 1.00.
 2. Even correctly targeted, gutting achieved nothing, because JAZZ HAS NO HOLLOW
    INTERIOR. Measured the torso window: 1,550 faces spread z -0.31..0.34, with 728
    at z 0.10..0.40 and 714 MORE behind them. The chest is layered panels, not a
    shell around a void. Cutting the outer skin just exposes the next skin.
CONCLUSION recorded honestly: "gut the chest and show ribs" cannot work on these
models by subtraction. There is no cavity to open. Getting a ribbed chest requires
BUILDING the rib structure proud of the surface, or replacing the chest volume
outright -- not carving into it.

## Update 324 -- EXTERNAL rib bands, verified visible; kit of 4 ribbed bots
Acting on the U323 disproof (no cavity exists to open), switched to building ribs
ON the surface and VERIFIED every step with the visibility raster rather than
shipping blind.
surface_ring(): measures the body's outer radius per angle at a given height, so
ribs can be laid just proud of the real silhouette.
Two measured findings that made the difference:
 1. rib OFFSET barely matters (1.02/1.10/1.20 -> 10.1/10.6/10.2% frontmost). Not
    the limiter.
 2. UNIFORM SCALING was the limiter. Fitting a part into a flat rib slot
    (0.095, 0.021, 0.095) scales by the SMALLEST axis ratio, shrinking the rib to
    nothing. Added --rib-stretch (bounded anisotropy):
        stretch 1.0 -> ribs frontmost 10.4%
        stretch 4.0 -> 44.6%    <- default
        stretch 8.0 -> 57.0%
So the ribs now genuinely band the chest: ~45% of the torso window is rib, the rest
shell showing between bands, versus 1.4% for the old interior approach.
Built the kit and installed it: jazz 42->236, grimlock 52->231, soldier 31->169,
zetaprime 50->202 structural members (target 479). Combined with the existing 48
tuned models + 24 greeble exemplars + 8 original textured models: EX_N_IMPORTED 84,
12 textured, 76 textures copied, 152 tuning entries preserved. Build clean, 62/62.
Browser entries JAZZRIB / GRIMLOCKRIB / SOLDIERRIB / ZETAPRIMERIB.

## Update 325 -- catalog comparison measured; full 12-model ribbed kit
Answered the open question with the visibility raster instead of taste:
    roboskelcat (Terminator endoskeleton): 44.6% frontmost,  35,610 tris
    detailcat   (Scorponok/Shockwave)    : 27.5%           , 156,306 tris
    skelcat     (human skeleton)         :  6.7%           , 245,721 tris
The endoskeleton wins decisively -- best coverage at 1/7th the triangle cost of the
human skeleton, whose parts (median 542 tris) are too large to fit rib slots. My
earlier guess that Scorponok parts would read better on outer surfaces was WRONG on
the measurement; they cover less at 4x the cost.
Applied ribs across all 12 sample models with the winning settings. Structural
members before -> after:
    scorponok 384->594 (already past the 479 reference), grimlock 52->231,
    jazz 42->236, wfc-optimus 50->225, arcee 31->210, zeta 50->202,
    foc/wfc bumblebee 5->185, shockwave 25->174, optimus-truck 45->170,
    soldier 31->169, ravage 38->160
Installed the whole kit alongside the existing library: EX_N_IMPORTED 92, 12 ribbed
entries, 156 tuning values preserved. Build clean, 62/62.
Note bumblebee started at just 5 structural members -- the WFC vehicle models are
nearly featureless, so they gain the most proportionally (5 -> 185).

## Update 326 -- limb splitting + real walk cycle (the long-standing animation blocker)
Every animation so far moved the WHOLE body, because nothing produced left/right
parts. Built tools/limb_split.py: segments a bot by CONNECTED COMPONENT (never
cutting triangles) into body / head / arm_l / arm_r / leg_l / leg_r from each
component's centroid, and exports each limb centred on its own PIVOT -- shoulder,
hip or neck -- so rotations swing the limb instead of orbiting it.
Jazz split: body 17 comps/3,783 tris, head 20, arm_l 11/912, arm_r 11/912,
leg_l 24/1,053, leg_r 24/1,053. The exact L/R symmetry in both component and
triangle counts is strong evidence the segmentation is correct.
Engine: atags 5..9 (arm_l/arm_r/leg_l/leg_r/head) now drive a walk cycle sharing the
existing body gait phase -- legs in opposition, arms counter-swinging at 0.65x, head
drifting slightly. Verified the phase math: at t=0.25 leg_l +70, leg_r -70, arm_l
-45.5, i.e. correctly opposed.
Caught my own amplitude bug before shipping: amp = 6 + spd*16 reached 102 degrees of
hip swing, which is cartwheeling not walking. Clamped to fminf(34, 5 + spd*7):
5 deg idle, 16 at walking pace, 33-34 at full speed.
Also fixed install_models: it was NOT emitting the pivot offset array, so every
off[] was zero-initialised and limbs would have orbited the model origin. Manifest
offsets (fields 5-7) now flow through to the generated table. Verified jazzlimb:
atags 0,9,5,6,7,8 with symmetric shoulder pivots at x -0.764 / +0.755.
Build clean, 62/62.

## Update 327 -- knees and elbows: hierarchical two-segment limbs
Single-pivot limbs give a stiff marching gait. Adding a second joint needs a
transform HIERARCHY, which the engine did not have -- quantified the need first: at
a 34-degree thigh swing the knee should travel 0.263 units, so a shin rotating about
a fixed pivot would visibly detach from its thigh.
Engine: ex_tmodel_t gained int parent[EX_MAX_TPARTS]. Limb angles are now computed
into g_limb_ang[] BEFORE being applied, so a child segment composes its transform
from its parent -- swinging the child's pivot about the parent's pivot by the
parent's angle, then adding the parent's rotation. Table order guarantees parents
are processed before children.
limb_split.py --joints (default on): splits each arm/leg at its median component
height into upper + lower, pivoting the lower at its top (elbow/knee) and recording
the parent index. Jazz: arm_l 6 comps/696 tris + arm_l_lo 5/216, leg_l 12/525 +
leg_l_lo 12/528, mirrored exactly on the right. Pivots are anatomically ordered --
shoulder y+0.72, elbow y+0.13, hip y-0.03, knee y-0.31.
install_models carries tags 10..13 and the parent column through to the generated
table. Verified: atags 0,9,5,10,6,11,7,12,8,13 with parents -1,-1,-1,2,-1,4,-1,6,
-1,8 (each lower segment pointing at its upper).
Gait: the knee is RECTIFIED (fmaxf against 0, so it never bends backward) and lags
the thigh by a quarter cycle -- straight through the plant, flexing as the leg
swings through. Checked across the cycle: thigh +34/knee 0 at the reach, thigh 0/
knee +37 mid-swing. Build clean, 62/62.

## Update 328 -- feet that stay level + limb kit across the sample set
Fixed the flaw I flagged last turn: with only thigh+shin the foot points wherever
the shin does, so the bot walks on its toes.
limb_split --feet (default on) takes the lowest 35% of each shin as a FOOT segment,
pivoted at the ankle and parented to the shin. Jazz: leg_l_ft 4 comps/98 tris at
y-0.80, mirrored exactly. Engine tags 14/15 counter-rotate the foot against
everything the leg has accumulated (-0.85x), so:
    phase 0.38: thigh +24, knee +26 -> leg total +50.5, foot tilt only +7.6
The sole now stays within about +/-8 degrees of level instead of following the shin
to +/-50.
Applied limb splitting across the sample set: 11 models (scorponok 699 components,
grimlock 162, optimus-truck 120, jazz 107, zeta 97, ravage 78, soldier 73,
wfc-optimus 72, arcee 42, shockwave 29, bumblebee 6).
Installed everything together: EX_N_IMPORTED 103 -- 12 RIB detail models, 11 LMB
articulated models, plus the existing library. Build clean, 62/62.
KNOWN ISSUE: model names truncate to 16 chars before the suffix, so
war_for_cybertron_arcee and war_for_cybertron_autobot_soldier both became
"warforcybertronalmb" and one overwrote the other -- 11 models from 12 inputs.
Needs longer names or a disambiguating suffix.
Also worth noting the vehicle models (bumblebee, 6 components) get limb-split too,
which is meaningless for a car; the tool should skip models classified as vehicles.

## Update 329 -- batch limb splitting, vehicle skip, and unique names everywhere
Fixed the name-collision class of bug properly this time, in three places.
1. limb_split --batch DIR: walks a tree, disambiguates duplicate folder names by
   parent directory, then GUARANTEES uniqueness with a counter. Replaces the
   hand-rolled shell truncation that silently merged arcee and autobot_soldier.
2. --skip-vehicles (default on): limb-splitting a car produces meaningless "legs",
   so paths matching car/tank/flyer/truck/boat/spaceship/hover/racer/heli/jet are
   skipped. On the samples: 7 bots split, 5 vehicles skipped.
3. install_models unique_display_names(): the collision had MOVED to the display
   layer -- 28-char truncation made distinct models share a browser label, so LMB
   models appeared as 3 instead of 7 and you could not tell which you were viewing.
   Collisions now keep the TAIL of the name (where the LMB/RIB suffix lives) and
   fall back to a ~N counter.
Result: 99 models, 99 unique names, 0 duplicates, RIB=12 LMB=7. Build clean, 62/62.
The lesson recurring across this project: every place a name is shortened is a
place two models can silently become one. That has now bitten in the browser list
(U269), the batch shell command (U328) and the display layer (here).

## Update 330 -- detail AND articulation in one model
Until now a model was either detailed (RIB) or articulated (LMB), never both.
limb_split --detail CATALOG now runs the rib pass AFTER splitting, banding the
TORSO while leaving the limb hierarchy untouched. Jazz: 12 limb parts + 1 rib part.
Visibility raster on the combined model: ribs frontmost 50.6% of the torso window
(vs 44.6% for rib-only), with thigh/shin/foot articulation intact -- so combining
them did not cost detail.
Two batch bugs found and fixed while doing it:
 - --batch was NOT forwarding --detail to the per-model runs, so batch output was
   silently limbs-only while single runs were correct.
 - display_name truncated from the HEAD, so long names lost their suffix:
   "SCORPONOK...HUMANLMB" became "...HUMANLM" and articulated variants were
   unrecognisable in the browser (counted 3 instead of 7). Long names now keep the
   TAIL with a leading "~", since the suffix is the distinguishing part.
Batched across both sample sets: 7 bots with combined detail+articulation, 5
vehicles skipped. Installed: 99 models, 99 unique names, RIB=12 LMB=7.
Build clean, 62/62.

## Update 331 -- vehicle_rig.py: wheels that actually roll
limb_split skips vehicles by design, which left them completely rigid -- sliding
along the ground with painted-on wheels. Built the counterpart.
vehicle_rig.py detects wheels by SHAPE and PLACEMENT (these meshes label nothing):
roundness of the cross-section about the thin axis, height in the lower body, and
crucially a MIRROR TWIN across the centre line. The pairing test is what rejects
round-but-not-wheel parts. Each wheel is split out, centred on its own hub, and
tagged tire so the engine's existing roll animation drives it.
First pass over-detected badly -- the Optimus truck reported 36 wheels (pipes,
lights, vents all pass roundness + mirroring). Added dominant-radius clustering:
real wheels on one vehicle share a radius, so keep only the largest cluster and
drop an unpaired odd one out. Truck 36 -> 12 (plausible for a semi), Arcee holds at
4 symmetric wheels at identical radius/height, Bumblebee correctly reports 0 and
warns -- its wheels are merged into the body, which the tool cannot split.
Installed the two genuine vehicles (Optimus truck, WFC Optimus cyber truck), each
13 parts / 12 wheels, atags 0,1,1,1... Build clean, 62/62. 101 models, 101 unique.
HONEST: run across everything, the detector also finds "wheels" on bots -- Jazz 4,
Grimlock 8, Scorponok 10. For a car-robot like Jazz those may be real wheels on the
body and rolling them would be correct; on Scorponok they are probably round
greebles. Did not install those, since I cannot tell which is which without seeing
them.

## Update 332 -- torso motion in the walk + PIPELINE.md consolidation
WALK: the gait had legs swinging under a static box. Added torso counter-rotation
(-0.16x the leg angle on Y) and hip roll (+0.10x on Z), applied to the body part of
limbed models only when moving. Verified subtle: at speed 4 the twist peaks at
+/-5.4 deg and the roll at +/-3.4, opposing the forward leg -- the weight-shift cue,
not a wobble. Build clean, 62/62.
PIPELINE.md: single reference for the now-eleven-tool pipeline. Documents the run
order and what each tool consumes/produces, a typical end-to-end run, and -- most
usefully -- a table of settings that were MEASURED rather than guessed (tri-budget
250k because 20k capped ultra; tol 0.20 because 0.01-0.12 finds no cross-model
reuse and 0.30 over-merges; rib-stretch 4.0 because uniform scaling gave 10%
coverage vs 45%; roboskelcat over detailcat on measured coverage-per-triangle;
stride clamp 34 deg because unclamped hit 102).
Also records the known limits (no cavity exists to open; detail parts do not
transfer between models; greebles are not normal-aligned; limbs are rigid
segments; wheel detection finds round greebles on bots) and the recurring failure
modes with their symptoms -- truncated names silently merging models, wrong
--engine path, unwelded meshes yielding zero parts, multi-part OBJ duplication.
Closes with the honest caveat that everything is verified by measurement and
NOTHING by eye, and that the one time visual feedback arrived it caught two faults
no measurement had.

## Update 333 -- derived models were losing their source's orientation
User tested 103 and reported models "flipped on their backs with feet forward",
plus skeletons reading as faces not solid parts, disconnected/oddly-rotated parts,
and greebles too small to fill space. Took the flipped-models report first since it
is unambiguous, and it was three stacked bugs:
 1. Derived models (jazzlmb, jazzrib) fell back to the LIBRARY DEFAULT orientation
    instead of their source's hand-tuned values -- Jazz is scale 0.185 rx 0, the
    default is scale 0.33, and models whose source needed rx=180 or -90 ended up
    lying down. Added inherit_tuning(): strips lmb/rib/veh/limb/full and matches
    the derived name against known models.
 2. Earlier runs had SEEDED a default entry for every derived model, so the
    inheritance never fired -- an entry existed, it was just meaningless. A tuning
    entry that exactly equals the library default now defers to inheritance.
 3. Inheritance then picked the WRONG source: stale "imported_*" keys from an older
    naming scheme match longer than the real key, so pure longest-match chose an
    entry that itself held defaults. Now prefers a genuinely-tuned source, then
    longest match.
Also restored the "imported_" prefix strip in collect(), which a later edit had
silently removed -- staged files were producing imported_imported_*.iqm and no
tuning key could ever match.
Result: 34 derived models now inherit (jazz-derived 0.185, grimlock 0.314,
optimus-truck 0.531, ravage rx=90 0.127); the 48 hand-tuned originals verified
intact (megatron_tf1 rx=180 0.534, tf5_barricade rx=-90 0.352). Build clean, 62/62.
STILL OPEN from the same report: parts read as faces not solids, limb parts detach
and rotate oddly, greebles too small. Not yet addressed.

## Update 334 -- "faces not solid parts": measured, confirmed, and fixed by generating
User's sharpest observation from testing 103. Measured the harvested catalog:
    0 of 120 exemplars are closed. Median 31% open edges, mean 31%.
    distribution: <10% open: 2,  10-40%: 83,  40-70%: 35
So a harvested "rib" is a curved SHEET with no thickness -- it reads as a shard.
Filtering cannot help; there is nothing closed to filter to. The user was exactly
right and the fix is to generate solids, not harvest them.
Built tools/solid_parts.py: watertight mechanical primitives -- box, plate,
tapered strut, capped cylinder, I-beam, ring segment (a rib with real
cross-section, not a sheet), hex bolt. Generated 140 parts across the same seven
catalog categories the placer expects, so it is a drop-in --catalog replacement.
Verified: worst openness across all 140 is 0.0%, against the harvested 31% median.
Solid parts are more compact than sprawling fragments, so coverage initially fell
(29.1% vs the harvested 44.6%). Swept it back: stretch 5.0/18 segments -> 48.0%,
stretch 6.0/22 -> 60.7%. Rebuilt the kit at 5.0/18.
NOTE for later: openness measured on the FINAL iqm reads 100% for everything,
because box_uvs splits vertices per face for UV seams -- every edge becomes a
boundary. That metric is only meaningful on the pre-UV geometry. Nearly misread it
as a regression.
Installed: 101 models, 34 inheriting tuning, build clean, 62/62.
STILL OPEN: limb parts detach and rotate oddly (child transform ignores Z offset
and does not rotate pivots by the model's own orientation); greebles too small and
not normal-aligned.

## Update 335 -- limb detachment fixed; greebles resized and normal-aligned
Two more items from the user's test report.
LIMB DETACHMENT (real math bug): the child-transform rotated the joint offset about
X as y'=y*cos, z'=y*sin -- dropping BOTH dz terms. Correct is y'=y*cos - z*sin,
z'=y*sin + z*cos. Measured the error on a real joint (dy -0.28, dz 0.05, 34 deg):
0.050 units of separation, which is exactly the "parts are not always connected"
report. Also the composed delta was applied in unrotated space while the base
offset was yaw-rotated, so the two disagreed as the model turned; the delta is now
yawed to match.
GREEBLES: were 2.5-6% of body extent (specks that sat in negative space, as
reported) and had no orientation. Now --greeble-size (default 0.09, so 6-14%) and
aligned to the surface NORMAL -- computed per-vertex from the shell -- so each
piece stands proud along the normal instead of lying at an arbitrary angle.
Verified on Jazz with solid parts + 120 greebles: 397 of the 479 target structural
members (was 236 without greebles), torso window 44.9% ribs + 6.7% greebles, and
greeble bounds sit INSIDE the shell bounds (protrusion -0.055/-0.166/-0.041, i.e.
sitting on the surface rather than floating off it).
limb_split now forwards --rib-segments/--greebles/--greeble-size through batch, so
batch and single runs agree (that had bitten once before with --detail).
Installed 101 models, build clean, 62/62.

## Update 336 -- rig_test.py: the animation checks the user asked for
User: "your animations could use some more rig tests because parts are not always
connected and they are being rotated strangely". Built one rather than eyeballing.
tools/rig_test.py replicates the ENGINE's transform composition in Python (same
atag switch, same parent inheritance) and sweeps the full gait cycle at four
speeds, checking:
  JOINT CONTINUITY -- a rotated bone must preserve its LENGTH exactly
  NO INVERSION     -- knees may not bend backwards
  RANGE            -- no joint beyond a plausible limit
  PIVOT SANITY     -- ordered down the limb, mirrored across the body
First run immediately proved the U335 math fix: max drift 0.000000 on every model,
where the dropped-dz bug would have shown ~0.05. But it also caught 12 asymmetry
failures I had no idea about.
Root cause found by measuring symmetry directly: THE MIRROR AXIS IS NOT ALWAYS X.
    jazz      X:0.0100  Z:0.0583  -> X
    shockwave X:0.1547  Z:0.3434  -> X
    zetaprime X:0.1226  Z:0.0860  -> Z
Zeta Prime mirrors about Z, so classifying left/right by X put its limbs on the
wrong sides. limb_split now detects the mirror axis per model (mirror the point
cloud about each candidate and keep the better reconstruction) and classifies
against it; rig_test infers the same axis from the pivots rather than assuming.
Result: 15 -> 8 failures, and 4 of 7 models now fully OK (grimlock, jazz, ravage,
scorponok). Remaining 8 are all "L/R at different heights" on shockwave, soldier
and zetaprime -- shockwave is a genuinely asymmetric character (cannon arm), the
other two are real misclassification from centroid-based assignment on non-T-pose
models. Recorded, not yet fixed.
Installed 101 models, build clean, 62/62.

## Update 337 -- tried mirror-pair classification, it REGRESSED, reverted
Attempted to fix the remaining 8 rig failures (L/R limbs at different heights on
shockwave/soldier/zetaprime) by classifying limbs via mirror PAIRING -- match each
component to its reflected twin, then hand the pair to left/right by construction
so heights cannot disagree.
It made things worse: 8 -> 10 failures. Pairing introduced new "on the SAME side"
errors, and shifting which components landed in which limb changed the axis
rig_test inferred from the pivots, cascading further. Reverted rather than ship a
regression, and removed the unused helper.
Back to the U336 state, re-verified: 4 of 7 models fully OK (grimlock, jazz,
ravage, scorponok), max drift 0.000000 on all seven, 8 failures confined to
shockwave (genuinely asymmetric -- cannon arm), soldier and zetaprime.
Recording the negative result so it is not retried blind: component-level mirror
matching is not sufficient here, because these models' left and right halves are
NOT reflections at component granularity -- the mesh is split differently on each
side, so twins often do not exist to match. A fix would need to work on the
geometry before component splitting, not after.
Installed 101 models, build clean, 62/62.

## Update 338 -- symmetrising the PIVOTS clears all rig failures
After the U337 mirror-pairing attempt regressed, tried the opposite approach: stop
trying to make the component ASSIGNMENT symmetric, and instead make the RIG
symmetric regardless of how the mesh happens to be split.
Pivots are now pre-computed for every limb, then left/right pairs are reconciled:
shared joint height (the average), mirrored magnitude on the model's own mirror
axis, shared depth. The mesh split can stay lopsided -- a left leg with 12
components and a right with 9 still articulates evenly, because the joint they
rotate about is symmetric by construction.
Result: 7 of 7 models pass, 0 failures (was 8, and 10 with the pairing attempt).
Verified the pivots stayed sensible rather than being dragged somewhere arbitrary:
    grimlock  arm +/-0.50 y+0.69   leg +/-0.87 y+0.01
    jazz      arm +/-0.72 y+0.72   leg +/-0.46 y-0.03
    ravage    arm +/-0.18 y+0.81   leg +/-0.20 y-0.04
Exactly mirrored, shoulders above hips, plausible spans.
Worth noting WHY this worked where pairing failed: pairing tried to fix the input
(which cannot be made symmetric -- the meshes genuinely are not), whereas this
fixes the output (which can be, and is the only part the animation touches).
Installed 101 models, build clean, 62/62, rig 7/7.

## Update 339 -- limb-split models were rendering FLAT PLACEHOLDER BLUE
Chasing the user's "not great visually" further and found something large that had
nothing to do with the rig: limb_split loaded geometry POSITIONS ONLY, so every
emitted part referenced plate_blue.png. A limb-split Jazz lost all seven of his
textures and rendered as untextured placeholder -- while the RIB models beside him
kept theirs. That alone would make the articulated models look far worse than the
static ones, independent of anything else.
Fixed: limb_split now loads BY MATERIAL (via texture_export), splits components
while carrying their UVs (components_uv -- ph.components drops everything but
positions, which is what forced the placeholder), and emits one part per
(limb, texture) so each limb keeps its real materials. Source textures are copied
out beside the model, converted and capped, under model-prefixed names so
install_models picks them up.
Jazz now emits 19 parts across 7 real textures instead of 12 flat-blue ones.
Across the kit: 9 of 92 parts (10%) still on placeholder -- those are the
generated RIB detail parts, which come from solid_parts and legitimately have no
source texture. Every part derived from the original MODEL now carries its real
material. (Corrected: an earlier draft of this entry said 0 of 118, which was
wrong on both numbers.)
Rig unaffected: still 7/7 with 0.000000 drift. Installed 101 models, 100 textures
(was 76), 20 max parts. Build clean, 62/62.

## Update 340 -- rib models were untextured too; same fault, same fix
Having found limb-split models rendering placeholder blue, checked detail_apply and
found the identical fault: the SHELL was emitted with the default texture, so every
RIB model was untextured as well. Both families of generated model were therefore
losing the source materials, which is a large part of "not great visually" and had
nothing to do with ribs, greebles or joints.
detail_apply now loads the shell BY MATERIAL and writes one shell part per
material, copying and capping the source maps out beside the model. Generated rib
geometry legitimately keeps the placeholder -- it has no source texture -- but the
model underneath no longer does.
Jazz rib model: 7 real textures + placeholder for the ribs, where it previously had
placeholder for everything. Across the rebuilt rib kit: 38 of 51 parts carry real
materials.
Installed: 101 models, 137 textures in data/ (was 76 two updates ago, 100 last
update). Build clean, 62/62, rig 7/7.
Reflection worth keeping: three separate visual faults in a row (flipped
orientation, surface-fragment parts, missing textures) were all invisible to every
measurement I had, and all three were implied by a single round of the user
actually looking at the result. The visibility raster and rig test catch geometry
and articulation errors; nothing I have catches "it renders wrong".

## Update 341 -- REGRESSION fixed: my texture change had deleted the joints
User reported joints "more disconnected". Investigated and it was far worse than a
degradation: the U339 per-material change built the texture buckets BEFORE the
joint split, so the emitted model had NO _lo or _ft segments at all and every
parent index was -1. Articulation was silently deleted while I reported textures
restored. The rig test did not catch it because it only checks the joints that
exist -- a model with no child segments trivially passes.
Fixed: texture bucketing now runs AFTER the upper/lower/foot split and keys on the
FINAL limb names; parent indices refer to the emitted part list rather than the
limb list (they no longer correspond once a limb spans several materials).
Jazz: 25 parts, 8 lower/foot segments, 8 valid parents -- was 20 parts, 0, 0.
Rig 7/7, 0.000000 drift. Installed 101 models, build clean, 62/62.
LESSON: rig_test passes vacuously on a model with no hierarchy. It needs a check
that the EXPECTED segments are present, not just that present ones behave.

## Update 342 -- shape_pairs.py: self-supervised (blob -> detail) training data
User proposed the right architecture for the detail problem: downsample the models
that already look good into blobs, and train a generator to invert that. Degrading
is free and exact, so the pairs are self-supervised and the learned detail comes
from real Transformers geometry rather than foreign parts -- which is precisely the
"has nothing to do with the models" complaint.
They also supplied kBM3 (Rust DCGAN, burn framework): Generator latent -> Linear ->
(B,f,8,16) -> 4x ConvTranspose2d -> (B,1,128,256); Discriminator strided-conv
critic; MelCache of zlib'd f32 crops in [-1,1]; BCE-with-logits + Adam.
Built the data half to match EXACTLY. A 3D component encodes as six orthographic
depth views at 64x64, tiled 2 rows x 4 cols = 128x256 single channel in [-1,1] --
deliberately the same tensor shape and range as the mel cache, so data.rs needs no
change. blobify() degrades a component by voxelising at grid=6 and keeping occupied
cells as boxes: proportions survive, surface structure does not.
Verified on scorponok + grimlock: 436 pairs, tensors (128,256) in [-1.00,1.00],
detail maps 61% occupied. Mean |blob-detail| median 0.361 (a real gap, not a
trivial identity) while silhouette IoU is 0.79 (outline preserved, detail removed)
-- i.e. exactly the mapping to learn. 2.5MB compressed, plus PGM previews.
HONEST LIMITS recorded for the next step: only 623 structural components exist
across every high-detail donor I hold, 384 of them Scorponok -- a DCGAN at that
scale memorises. And kBM3's generator is UNCONDITIONAL (latent -> image); upscaling
needs conditioning on the blob, i.e. a pix2pix-style encoder-decoder with skips,
which is a real but bounded change to model.rs.

## Update 343 -- 4,824 real pairs; degradation was going the WRONG WAY
User extracted the 901MB high-quality Grimlock (42 true-parts OBJ files, 4.2M
triangles, 1.88M vertices) and ran the encoder over six donors: 4,824 pairs, up
from 436. Grimlock alone contributed 3,767 components.
Checked the encoding before building on it and caught a real fault: the voxel
degradation carried MORE depth variation than the detail it was meant to degrade
(blob 0.506 vs detail 0.415). Voxelising adds hard cube edges, so training on those
pairs would have taught the generator to REMOVE blockiness rather than to ADD
detail -- the opposite of the goal, and it would have looked like progress because
the loss would still fall.
Replaced the degradation with DECIMATION, which is what a low-poly model actually
is. Re-measured:
    depth variation   blob 0.344   detail 0.391   -> detail +14% relief (correct)
    |blob-detail|     median 0.287
    silhouette IoU    median 0.82  (outline preserved, structure removed)
--degrade voxel kept for comparison; decimate is now the default.
ON POSE (user flagged idle vs action between the low-poly and high-quality
Grimlock): it does not block this approach, because BOTH sides of every pair come
from the SAME mesh -- degradation cannot change the pose. Pose would only matter if
the real low-poly model were used as the input side. At inference each component is
normalised to unit scale independently, so a thigh is a thigh regardless of where
the limb is swung to.

## Update 344 -- conditional GAN for the Rust trainer + raw pair export
Wrote tools/model_cond.rs: the conditional half kBM3 lacks. Their Generator maps a
LATENT to an image, which can only invent a sample -- upscaling needs the generator
to see the low-detail shape and add to it, and the critic to judge the PAIR (a
critic seeing only the output accepts realistic detail belonging to a different
part, which is the failure mode the whole approach exists to avoid).
Design, in the same burn idiom so it drops into the existing Vulkan trainer:
 - CondGenerator: U-Net. Four stride-2 encoder convs 1 -> f/8 -> f/4 -> f/2 -> f
   down to (B,f,8,16), four transposed convs back up, with the matching encoder
   level CONCATENATED at each step. The skips are the important part: without them
   the bottleneck discards the silhouette and the output drifts off the input.
 - CondDiscriminator: identical strided-conv critic except c1 takes 2 channels,
   (condition, candidate) stacked.
 - l1_loss + the same bce_with_logits; l1_weight default 100 as in pix2pix. Pure
   adversarial loss on a few thousand samples collapses onto whichever donor
   dominates -- exactly the "one model's parts on everything" failure.
Verified the tensor arithmetic before it reaches a compiler: encoder 128x256 ->
8x16, decoder back to 128x256 with skip concatenations 128+128=256, 64+64=128,
32+32=64 all consistent, output channels 1. Documented the training step as a
comment mirroring main.rs.
Also added a raw export to shape_pairs: pairs.f32 (blob,detail interleaved little
-endian f32) + pairs.json with count/height/width, so the Rust side needs no numpy
reader. 436 pairs = 114MB raw.
NOT DONE: cannot compile or train here (no cargo, no GPU). The Rust is written to
match their idiom and the shapes are checked arithmetically, but it has never been
built.

## Update 345 -- shape_baseline.py: is the task learnable? Measured before training
A GAN always produces something, so it is a poor instrument for deciding whether a
task is well posed. Built a training-free test instead.
On the user's 4,824 real pairs, random split:
    IDENTITY  (return the blob)      L1 0.4081
    MEAN      (ignore the input)     L1 0.4286
    RETRIEVAL (nearest blob's detail)L1 0.1590   -61% vs identity, -63% vs mean
So the mapping genuinely carries signal, and retrieval is a strong no-GPU baseline
any trained model must beat.
But 86% of nearest neighbours came from the SAME donor, and Grimlock is 78% of the
set, so the random split flatters it. Added LEAVE-ONE-DONOR-OUT, which is the
question that actually matters -- will this work on a model it has never seen:
    brawl      -37%   grimlock  -10%   soundwave -10%
    scorponok   -8%   megatron   -2%   laserbeak +17% (WORSE than identity)
    mean -8%
Honest reading: detail transfers only WEAKLY across models. The -61% headline is
mostly within-model matching -- a held-out component usually has a near-twin
elsewhere in the same mesh. Laserbeak actively fails, which fits: it is a small
bird-form model unlike any other donor.
CONSEQUENCE for the GAN: on this data it would learn largely the same within-model
matching, i.e. memorisation, which is the "one model's parts on everything" failure
in a more expensive form. The fix is donor DIVERSITY, not architecture -- the six
current donors are three humanoid Decepticons, a dinobot, a bird and a tank.

## Update 346 -- shape_transfer.py: retrieval-based detail, real geometry
Turned the validated retrieval baseline into something that produces geometry.
For each component of a target model, find the donor component whose coarse shape
matches (principal-axis-aligned occupancy grid + aspect ratios) and transplant the
DONOR'S ACTUAL MESH fitted to the target component's box.
Transplanting rather than decoding a generated depth map is deliberate: the six-view
encoding loses concavities and thin features, and reconstructing from it reintroduces
exactly the blobbiness the whole exercise is trying to remove. The donor mesh is
already correct geometry.
On Jazz with a 226-component bank (grimlock + scorponok):
    swapped 47 of 56 components
    triangles 8,728 -> 29,033 (3.3x)
    donors drawn 26 grimlock / 21 scorponok -- NOT collapsing onto one donor
HONEST LIMITS, both measured:
 1. Structural member count is UNCHANGED at 56. This raises surface DENSITY, not
    part count -- it cannot approach Scorponok's 479 members, because it swaps
    components one-for-one rather than adding new ones. It is a different axis of
    detail than the rib work, not a replacement for it.
 2. Silhouette drifts. IoU against the original falls to 0.67 at the default
    --gain 2.0, i.e. a third of the outline changed -- the donors partly overwrite
    the character's shape. Swept it: gain 4.0 -> IoU 0.74 at 24.5k tris, gain 8.0
    -> IoU 0.80 at 18.1k. Higher gain swaps only where the donor is much denser,
    trading added detail for fidelity to the original.
No GPU, no training, and it is the baseline a trained model must beat.

## Update 347 -- component swapping trades fidelity for detail; no free setting
Added --min-sim (only swap on a confident shape match) to try to keep the
silhouette while still adding detail, and swept it on Jazz at --gain 4.0:
    --min-sim 0.00   28 swaps   24,529 tris   IoU 0.74
    --min-sim 0.90    4 swaps   11,475 tris   IoU 0.79
    --min-sim 0.97    0 swaps    8,728 tris   IoU 0.82  (unchanged model)
Match confidences run p10 0.833 / median 0.880 / p90 0.917, so 0.90 admits only
the top quarter.
The two axes move in opposite directions with no crossover, and the reason is
structural rather than a tuning failure: every swap that adds triangles also
changes the outline, BECAUSE THE DONOR IS A DIFFERENT SHAPE. Component swapping
cannot add detail without altering form.
So the two approaches are complementary, not competing:
  shape_transfer  raises surface DENSITY (3.3x tris) but drifts the silhouette and
                  leaves member count unchanged
  detail_apply    adds structural MEMBERS (42 -> 397) on top of the existing shell,
                  keeping the silhouette intact by construction
If the goal is "same character, more detail", detail_apply is the right tool and
shape_transfer is wrong for it. If the goal is "denser surfaces, character may
shift", the reverse. Recorded so the choice is explicit rather than rediscovered.

## Update 348 -- ascii_view.py, and it immediately found the "weird arm plates"
The binding constraint all along has been that I cannot see the output. Built a
text renderer to close part of that loop: depth-shaded ASCII silhouettes from
front/side/top, applying the engine's own rx/ry/rz and scale from
model_tuning.json, so what it prints is what the game shows.
It paid for itself immediately. Rendering a ribbed model showed the ribs as mere
speckle at the silhouette edge rather than banding across the chest. Measuring the
rib frame explained why:
    rib frame spanned x -1.03..1.03 on a torso 0.34 wide -- SIX TIMES too far
surface_ring() measured the body radius using EVERY vertex at chest height, and on
a T-posed bot that means the ARMS. Ribs were being placed out at the arm span.
That is exactly the user's "weird arm plates that have nothing to do with the
models" -- reported from looking at it, invisible to every numeric check I had.
Fixed in two steps, the first of which I got wrong:
 1. Clipped the ring RADIALLY to the torso -> ribs landed at x +/-0.30, correct
    laterally, but the same clip cut the chest DEPTH so the ribs sat inside the
    shell and were hidden again.
 2. Clip LATERALLY only (|dx| <= max_r). Arms are far in x; the chest is
    legitimately deep in z. Ribs now x -0.33..0.31, reaching z 0.39 against a
    shell front of 0.26 -- proud of the surface and visible.
Visibility raster confirms: ribs frontmost over 50.4% of the torso window.

## Update 349 -- used the viewer to fix the greebles the user called "random"
With ascii_view working, rendered the greeble layer in isolation for the first
time. It looked exactly as reported: scattered specks with no structure. Uniform
random placement cannot read as mechanical detail, because real detail is
ORGANISED -- a panel of bolts, a bank of vents, a row of louvres.
Replaced the scatter with CLUSTERED placement: seed a few sites, build a local
frame from the surface normal (up + two tangents), and lay an aligned grid at each,
sharing one part type and size per cluster so a group reads as one feature.
First attempt overcorrected -- rendering showed the clusters merging into solid
masses, because the spacing (1.35x element width) was under the element size.
Added --cluster-pitch and swept it: at 2.4x with --greeble-size 0.05 the elements
stay distinct and read as patches of detail rather than noise or lumps. Defaults
changed to 0.05 / 2.4 / 6-per-cluster.
Rebuilt the kit: 7 models, rig 7/7, 0 drift. Installed 101 models, build clean,
62/62.
Worth noting all three greeble faults (too small, unoriented, unstructured) were
reported by the user from looking at the game, and none were visible to the
coverage metrics -- but all three were obvious within seconds of rendering the
layer as text.

## Update 350 -- orientation audit found a genuinely lying-down model
Used ascii_view to audit every model in the kit rather than spot-check. Found
Shockwave (devastation) lying flat: after tuning, height 0.94 against x 2.00 and
z 1.35 -- height was its SMALLEST dimension. The OTHER Shockwave
(human_alliance) had been hand-tuned to rx=90; this one never was, and nothing in
the pipeline noticed for ~80 updates.
Automated the check, and got it wrong first: flagging "tallest axis is not Y"
marked EVERY model, because a T-posed bot is legitimately wider than it is tall,
and the height-maximising fix then stands it on an arm. Tightened to: lying down
iff height is the SMALLEST dimension by a clear margin (<0.75x the largest), and
only suggest a rotation that stops height being smallest.
Re-audit is then clean and specific -- 6 of 7 upright, Shockwave alone flagged with
"suggested fix: rx = +90". Applied it; he now stands at height 1.35.
The viewer has now caught, in one session: ribs placed out at the arm span,
greebles as unstructured noise, greeble clusters merging into lumps, and a model
lying on its back. None were visible to any numeric check; all were obvious on
sight. That asymmetry is the main lesson of this project.

## Update 351 -- limbs were flying off; now the split refuses rather than ships broken
Measured joint gaps through the walk cycle instead of trusting rig_test, which only
ever checked that a bone PRESERVES ITS LENGTH under rotation -- never that the
child is anywhere near the parent. The gaps were severe:
    zetaprime 1.2982   soldier 1.1419   ravage 0.5069   grimlock 0.4546
    scorponok 0.2846   shockwave 0.2602   jazz 0.0742
on a model normalised to 2 units. Zeta Prime's arms were 6,818 triangles on one
side and 100 on the other, with pieces sitting a full body-width from their own
pivot. That is the user's "joints are more disconnected", and I had been shipping it
for many updates behind a passing test.
Two attempted fixes that did NOT work, recorded so they are not retried: snapping
the child's pivot onto the parent surface (no measurable change), and rejecting
outlier components by distance from the limb median (no change -- when most of a
limb is outliers the median is already wrong).
What did work: VALIDATE and refuse. If the two sides differ by more than 4x in
triangles, or a limb sits further than --max-gap (0.12) from the body, emit the
model UNSPLIT. A static but correct model beats an articulated broken one.
Result: zetaprime, soldier, ravage and shockwave now fall back to static (gap
0.0000); jazz rigs cleanly at 0.084; grimlock 0.45 and scorponok 0.28 still pass
the body check but gap at the KNEE, which is created after validation runs and so
is not yet covered.
Installed 101 models, build clean, 62/62, rig 3/3 on the models that do rig.

## Update 352 -- voxel_morph.py: reassembly swarm (the user's idea, and it works)
User's insight: shared parts look too basic ON THEIR OWN, so never show them alone.
Break the model into many clusters that fly apart and REASSEMBLE -- the eye reads
density and motion instead of any single piece's shape. That also sidesteps the
problem that defeated three rounds of procedural detail: a cluster does not have to
look like anything, because it is never seen in isolation.
tools/voxel_morph.py partitions a model by triangle centroid (Lloyd relaxation, 8
iterations -- compactness matters, a stringy cluster reads as debris while a compact
one reads as plating). Each cluster becomes a part carrying TWO positions: its
assembled pivot, and a dispersed position pushed out along its own radial direction
with a tangential swirl and vertical scatter. Manifest gains fields 9-11 for the
dispersed position.
Engine: atag 20 interpolates between them with smoothstep, a per-part STAGGER so
pieces arrive at different times (uniform arrival looks mechanical), and a spin that
decays as each piece lands. F4 in the showcase runs it either way.
Grimlock: 90 clusters, 58/142/329 tris min/median/max. Installed as a 90-part model
(EX_MAX_TPARTS 128 was already high enough). Build clean, 62/62.
Verified both states with ascii_view: dispersed renders as a wide scattered cloud,
assembled renders as coherent structure -- and because clusters are cut from the
model's own surface, the assembled state IS the original mesh exactly.
Incidental: the container filled to 100% mid-session and silently truncated
voxel_morph.py to 0 bytes, which then "ran" with no output and exit 0. Freed 2GB
and rewrote it. Worth remembering that a 0-byte Python file is a successful no-op.

## Update 353 -- library_score.rs: Rust donor scorer, and it COMPILES and RUNS
User asked for a Rust-side tool to sweep the whole library, bucket by form class
(flyer/tank/etc) and score bony vs hollow. Rust turned out to be available in this
container, so this one is actually compiled and run rather than written blind.
SCORING CHOSEN BY MEASUREMENT, against models whose character is already known:
    model       compactness  bbox_fill  members
    scorponok         95.7      0.016      384   <- the bony reference
    grimlock          30.4      0.229       52
    jazz              29.2      0.150       42
    ravage            20.5      0.077       38
    shockwave         17.5      0.039       25
COMPACTNESS = area / volume^(2/3) is the discriminator -- 3x separation on
Scorponok. A skeletal model has huge surface area for its enclosed volume. bbox_fill
alone is misleading: Shockwave scores low purely from a sprawling pose.
Final score = compactness * (1 + ln(members)), the log so a 4,000-part model does
not swamp a genuinely bony 400.
Compiling and running found TWO bugs I would otherwise have shipped:
 1. IQM header fields transposed -- I used offsets 48/52/56/60 as
    tris/ofs_tris/verts/ofs_verts when they are verts/ofs_va/tris/ofs_tris. It
    panicked with triangle indices past the end of the vertex array. Verified the
    real layout against prop_view.py and corrected it; added index guards so a
    malformed file is skipped rather than killing the run.
 2. Signed volume is MEANINGLESS on an open surface, so a flat 80-triangle panel
    encloses nothing and compactness explodes. The first run ranked generated
    panels at 24,630 -- above Scorponok. Gated on open_frac < 0.25 and >= 500
    triangles rather than clamping, since the metric genuinely does not apply.
Run over the 674 installed IQMs the ranking is now sensible: Scorponok variants
take 3 of the top 7, and the per-class summary reads bot-bony 98.8 mean compactness
against bot-shell 18.1 -- a 5x separation between the classes it is meant to tell
apart. CSV output for feeding donor selection.

## Update 354 -- user was right: "bony" rewards NOISE. Measured and partly fixed.
User's warning: "boxy vs bony seems rewarding but can degrade into the same issue of
noisy over sensible box quickly". Tested it directly by building an adversarial mesh
-- a slab of jittered boxes, pure noise with no structure:
    scorponok (real bony)   compactness  95.7
    clean grid of boxes                 111.6   <- already beats it
    jittered amp 0.06                   270.5
    jittered amp 0.15                   881.0   <- 9x the real thing
So compactness alone would have selected noise as the best donors. The critique was
exactly right and the metric was unusable as a selector.
Added a COHERENCE term: mean |dot| between the normals of edge-sharing faces. Real
plating is locally smooth and meets at clean edges; high-frequency junk has
neighbours pointing everywhere. Score is now compactness * coherence^3 * ln(members).
    clean grid of boxes:  111.6 -> 4.1     (correctly destroyed)
    scorponok:             95.7 -> 46.9
HONEST: this is a PARTIAL fix and I have recorded it as such in the source.
Coarsely jittered boxes keep each face planar, so they keep decent coherence (0.64)
and can still outrank real models. On the real library the new ranking is also
mixed: Scorponok variants hold 4 of the top 8, but several smooth terrain meshes
(roots, hollow, enceladus) top the list on near-perfect coherence (0.99) with only
2-9 members. Those are not bony at all -- they are just clean.
The remaining hole is that no cheap geometric metric distinguishes "purposeful
varied structure" from "clean repetitive structure". The honest use is as a
CANDIDATE FILTER to look at, never as proof of quality -- which is the same lesson
as the coverage metrics earlier in this project.

## Update 355 -- assetkit: egui asset manager on the monorepo's uikit
User supplied logscope-monorepo. Their shared/uikit pins egui 0.29 (not 0.34), which
needs rustc >= 1.76 -- this container has 1.75 and static.rust-lang.org is blocked,
so egui cannot be built here. Confirmed by trying: 0.29 fails on ecolor/emath, 0.28
pins resolve back to 0.29 transitively, and eframe 0.27 fails on edition2024.
Rather than write blind, read their ACTUAL API out of the bundle and wrote against
it: Skin::carved(), carved::{button,button_accent,pill,chip,panel}, apply_palette ->
PaletteColors {bg_base,bg_card,accent,text,text_weak,danger,border,...},
uikit_egui::row, snapclone_theme::palette::DARK.
rust/assetkit/
  src/mesh.rs   IQM + OBJ loaders and the FULL scoring stack (compactness,
                coherence, welded connected components, openness, form class).
                COMPILES CLEAN standalone -- this is the real port of the
                measurement half of the python pipeline, and it is the part that
                benefits from Rust (library-wide sweeps).
  src/main.rs   four pages -- Library (scan/score/sort/filter), Viewer (wireframe
                with the lying-down check carried over from ascii_view), Export
                (shells out to the tested python tools, output streamed into the
                panel), GAN (retrieval baseline + training launch).
                NOT COMPILED, and the file says so in its header.
Deliberate decisions recorded in the README: exports shell out to python rather
than being reimplemented, because those tools are tested and a second
implementation is a second thing to keep correct. The viewer is edge-only so it
needs no GPU path, and it caps drawn triangles (every Nth) so a 4M-triangle donor
does not freeze the UI.
The Library page prints the caveat next to the score itself: "a filter to look at,
not a verdict", because the user's noise critique is real and a big number in a
table implies more than the metric can support.

## Update 356 -- python pipeline ported to Rust, COMPILED and CROSS-CHECKED
User overrode my hesitation about porting, and they were right: dependency-free
Rust compiles fine here (it was only egui that needed rustc 1.76). So this port is
verified rather than written blind.
rust/assetkit/src/
  mesh.rs     IQM + OBJ loaders, area/volume, welded components, openness,
              coherence, form class, bony score
  iqm.rs      IQM WRITER + box-projection UVs + vertex normals (offsets computed,
              never hardcoded, so a change in one section cannot desynchronise)
  harvest.rs  connected components (welds first -- unwelded meshes otherwise give
              one component per triangle), mirror-axis detection, limb
              classification, split validation
  ascii.rs    depth-shaded text renderer + the lying-down test
  cli.rs      score / view / harvest / morph / info
CROSS-CHECKED against the python it replaces, which is the point:
  * info on imported_jazz_part1.iqm: 398 verts, 480 tris, 2 components,
    compactness 12.5 -- IDENTICAL to python on the same file
  * score matches library_score.rs row for row across 674 models
  * IQM writer round-trips: 106 tris written, 106 read back by its own reader
  * 60 morph clusters written by Rust all load in the PYTHON reader -- the two
    implementations are interoperable, not merely similar
  * `view` is strictly BETTER than the python original, which cannot open IQM at
    all (ascii_view routes through process_models.load_geometry: glTF/OBJ only)
Kept in python deliberately: texture conversion (PIL), glTF parsing (json+base64),
and the GAN data encoding -- all heavily dependency-bound, and none of them is the
part that benefits from Rust. The measurement and geometry core, which is what
sweeps a large library, is now native.

## Update 357 -- glTF, JSON and the GAN encoder ported; one honest shortfall
Pushed the port through the "challenging" remainder.
json.rs   hand-written JSON reader + base64. No dependencies, which is why the
          whole crate still compiles on any rustc.
gltf.rs   full glTF/GLB loader: external + embedded + GLB buffers, byte-strided
          accessors, all five index component types, primitive modes (skips
          points/lines, converts strips and fans), percent-decoded URIs, and the
          NODE HIERARCHY with matrix or TRS -- without the node transforms every
          part lands at the origin in a pile.
          CROSS-CHECK, exact on all four: jazz 8846v/10660t, grimlock
          12512v/13768t, scorponok 63551v/87663t, ravage 7088v/10004t.
pairs.rs  six-view depth encoder (128x256 in [-1,1], the kBM3 cache shape),
          vertex-clustering decimation, interleaved f32 writer, PGM previews, and
          a built-in check that the blob really is less detailed than the target.
HONEST SHORTFALL, documented in the README rather than buried: the Rust `pairs`
produces a +3% depth-relief gap where python gets +14%. Vertex clustering reduces
only 2-4x per pass and cannot hit an arbitrary target; iterating it helped
marginally (2% -> 3%). The pairs are correctly ORDERED but the gap is narrow,
which makes the learning task easier and less useful. Recommendation recorded: use
the python shape_pairs.py for real training data until this is replaced with an
edge-collapse decimator.
Texture conversion stays in python: it needs PNG/JPEG codecs and is I/O bound, so
Rust buys nothing.

## Update 358 -- raids built from the lore; two phases pointed at nothing
Returned to plan5. Audited the raid content and found a real defect: raid_plan.h
referenced maps "kuiper" and "unicron" which DO NOT EXIST in maps.json, so 2 of 3
raid phases loaded an undefined theme. Also, raids-and-relics.md has 19 bosses with
substantial fiction and none of it reached the game -- the raid was three generic
phases with no boss at all.
tools/gen_raids.py:
 * adds the two missing maps (kuiper: void_edge, shepherd moons + frozen debris
   braid; unicron: devourer_red, "the maw; horizon curves the wrong way")
 * turns each boss's FICTION INTO A MECHANIC rather than flavour text, mapping onto
   the engine's existing raid_enc_t.mech field:
     Cherno Alpha  "if it ever stops walking the reactor reaches criticality"
                   -> GROUND, the fight is about keeping it moving
     Gigadrill     "it drilled through moons"        -> AIR, it leaves the floor
     Trenchmaster  "the water rises to meet it"      -> SYNC, high ground shrinks
     Speakerman    "fractures planets by resonance"  -> SYNC, plate mechanics
   Each raid records the premise line the mechanic came from, so the reasoning is
   inspectable rather than asserted.
 * scatters encounters with the boss's own mechanic guaranteed on at least one --
   a phase where every encounter is mech 0 plays identically to no mechanic at all
 * VALIDATES that every map a phase references actually exists, and exits non-zero
   if not. That check is what the old hand-written header lacked.
Output: content/raids.json (19 raids x 3 phases), maps.json 14 -> 16, and
raid_plan.h regenerated with Cherno Alpha baked in as the default.
Mechanic spread across the 19: 5 sync, 5 purge, 5 ground, 4 air.
Build clean, 62/62.

## Update 359 -- raid generation ported to Rust and wired into the egui app
Ported gen_raids.py to rust/assetkit/src/raids.rs and added a Raids page to the
egui front-end.
Needed a JSON WRITER first (json.rs): the generator patches maps.json in place, and
clobbering a hand-edited content file with a lossy round-trip would be worse than
not writing it. Emits sorted keys so the file stays diffable.
raids.rs carries the boss table, the derived-mechanic mapping, a small xorshift
RNG, encounter scatter (boss mechanic guaranteed on at least one -- otherwise the
phase plays as if it had none), maps.json patching, raids.json + raid_plan.h
emission, and the reference validation.
CROSS-CHECKED against the python: same 19 ids, IDENTICAL mechanic assignments and
premise lines. Phase map choices differ because the RNGs differ; both are valid and
both pass the reference check. Rebuilt raid_plan.h from the Rust with gigadrill
baked in -- cmake clean, 62/62.
GUI Raids page: generate with seed/phase controls, browse all 19, inspect each
raid's phases and per-encounter mechanics, write json + header. The PREMISE line is
shown next to the mechanic it produced, so the derivation is on screen rather than
buried -- the same reasoning as putting "a filter, not a verdict" beside the bony
score on the Library page.

## Update 360 -- CPU falsifier for the GAN: the task is NOT currently learnable
Built rust/assetkit/src/nn.rs -- a dependency-free CPU conv net (residual
generator + paired patch critic, Adam, the same BCE-with-logits as kBM3) that
trains on the real pairs at 64x32 in seconds.
The point is that it is a FALSIFIER, not a training rig. A GAN on a GPU always
produces something and the loss always falls, so it cannot answer "is this task
learnable". This can, before any GPU time is spent.
Also fixed my own too-generous verdict: the first version reported "the
architecture LEARNS" at a 1% margin over identity, and claimed beating MEAN was
evidence. It is not -- returning the input beats the mean trivially. IDENTITY is
the only real bar, and it has to be beaten by a margin rather than by noise.
RESULT, on both pair sets:
    rust pairs   (+3% relief gap)    vs IDENTITY  -0.1%
    python pairs (+14% relief gap)   vs IDENTITY  +0.4%
The net has NOT learned. It reproduces its input, which is exactly what a residual
generator does when the target is nearly the input. That is a DATA problem, not an
architecture problem -- the blob is not degraded enough for a mapping to exist.
This is a genuinely useful negative result: it says do not run the GPU trainer yet,
and it says the fix is the decimator (widen the blob/detail gap), not the model.
Consistent with U345's leave-one-donor-out finding (-8% mean), which pointed the
same way from a different direction.
Wired into the GUI's GAN page as "CPU check (does it learn?)" beside Train, so the
question gets asked before the expensive answer.

## Update 361 -- the two-stage proposal fixes the DATA problem; verdict logic fixed twice
User proposed splitting the GAN into two pairs: one for asset position/scale/
orientation, a second that takes exploded parts vs "big empty box equivalents" and
fits the geometry. Tested the second half immediately because it attacks exactly
what U360 measured as broken.
IT FIXES THE DATA PROBLEM. On the same 437 components:
    degradation   mean |blob-detail|   depth relief gap
    decimate                   0.15               +3%
    box                        0.58             +150%
A box shares only the part's extent, so "what fills a volume of this shape" is a
real question. Decimation produced blobs so close to the target that a residual net
could just copy its input -- which is precisely what it did.
Added pairs::boxify() and pairs::placement() (9 numbers -- position, scale, aspect
-- for stage 1, a far better-posed problem than pixel-to-pixel).
HAD TO FIX MY OWN VERDICT LOGIC TWICE, in opposite directions:
 * U360's version called a 1% margin over identity "LEARNS", and cited beating MEAN
   as evidence. Beating the mean is trivial when the input is nearly the target.
 * The tightened version then called the box result "LEARNS a real mapping" at +35%
   over identity -- while the net was 2.6% WORSE than the mean. With box inputs
   identity is trivially bad, so MEAN becomes the hard bar.
Which baseline is hard DEPENDS ON THE DEGRADATION. The verdict now requires beating
BOTH, and names which one is the real bar for the data at hand.
CURRENT STATE of stage 2: +34% vs identity, -1.3% vs mean. The net has found the
output DISTRIBUTION without conditioning on its input. Swept lr 0.001-0.004 and
12-24 filters; stable, and 0.004 diverges. Reading: this is now a capacity problem
rather than a data problem -- three same-size convs with no bottleneck cannot encode
"which box is this", so the U-Net in model_cond.rs is the right next thing to try,
and this is the first point where GPU time looks justified.

## Update 362 -- two-stage GAN wired into the GUI end to end
Added `assetkit-cli place` (stage 1): exports position/scale/aspect per part as CSV
+ raw f32. Verified on 436 parts that the descriptors VARY (position 0.02..0.98,
scale 0.002..0.839) rather than being constant -- a constant descriptor would mean
stage 1 learns nothing, and that check is cheap enough to always run.
GUI GAN page rebuilt around the two stages:
 * stage toggle (placement / fitting)
 * stage 1: donor path + export placements
 * stage 2: degradation toggle, build pairs, then THREE buttons in escalating cost
   -- CPU check, retrieval baseline, GPU train. Cheap questions first.
 * the degradation toggle states its measured consequence on screen
   (decimate +3% relief, box +150%), because it is the single setting that decides
   whether the task is learnable at all
 * log colouring is meaningful: red HAS NOT LEARNED, amber PARTIAL, green LEARNS
 * the standing result is printed as a chip rather than left to rediscovery
Verified the whole CLI path the GUI shells to: pairs --degrade box -> 437 pairs at
+150% relief -> train -> "PARTIAL, +30.5% vs identity, -4.7% vs mean". CLI compiles
with 0 errors; the egui layer remains unverified (rustc 1.75 vs egui's 1.76).

## Update 363 -- ASCII checkpoints in the trainer; they corroborate the verdict
Added --show N to `assetkit-cli train`: renders held-out examples as a triptych --
input box, target part, net output -- in text.
It immediately paid for itself. The numbers said PARTIAL (+34% vs identity, +1% vs
mean); the pictures show WHY. Across two held-out examples with structurally
different targets, the net emits the SAME mushy texture:
    INPUT (box)    TARGET (part)          NET OUTPUT
    @@@@@@@@@@     %#####*---+*#%%%###*   +##******##*****#***
    @@@@@@              #%%*    #%%*      *****######**###
That is "found the output distribution without conditioning on the input" made
visible, and no loss value would have shown it. Same lesson as the ascii_view work:
this project's failures have consistently been invisible to metrics and obvious on
sight.
Also wrote rust/assetkit/GAN_RUN.md with the concrete commands for the user's
library path, including the warning not to point `score` at the whole tree (tens of
GB, every mesh loaded) and the reminder that --degrade box vs decimate is the
difference between +150% and +3% relief gap, i.e. between a learnable task and an
unlearnable one.

## Update 364 -- context conditioning: the user's AABB-in-context point, tested
User's point: the batch has to represent AABBs INSIDE a subset of models -- a lone
box is ambiguous, so the net cannot do better than the average part. That is
exactly the failure mode measured in U361-363.
Implemented pairs::context_plane(): the part's position/scale/aspect within its
model, laid out as horizontal bands so a convolution can read it locally rather
than integrating a constant. Written as a third plane via `pairs --context`
(triples.f32), and consumed as a real second input channel by a generator that now
takes N channels.
MEASURED, same 436 components:
    box only                      +34.2% vs identity   -1.5% vs mean
    box + context (blended)       +25.7%               -1.8%   WORSE
    box + context (real channel)  +34.8%               -0.6%   best
Two findings. The conditioning DOES help and moves the hard baseline (mean) the
right way. But my first attempt -- blending context into the box plane because the
generator was 1-channel -- measured WORSE than not using context at all: the two
planes mean different things and averaging destroys both. Recorded in the source so
the shortcut is not retried.
Still PARTIAL. The ascii checkpoints show the net now varies its output with the
input more than before but still emits a broadly uniform texture. Reading unchanged:
three same-size convs have no bottleneck, so there is nowhere to encode "which box
is this" even when the information IS in the input. The U-Net in model_cond.rs is
the missing piece, and that is a GPU-scale change rather than something to
brute-force on CPU.

## Update 365 -- genetic fill: search beats the net, and it actually works
User: "make a genetic pool, bigger box regions as strides or multi-pass with linker,
that is the scoring, and the population does hunting random walks". Built it, and it
succeeds where the GAN stalled.
rust/assetkit/src/evolve.rs:
 * GENOME: list of (part index, position, scale, yaw) drawn from a real catalog, so
   a candidate cannot be a smear -- it is always genuine geometry
 * FITNESS: IoU against a 16^3 target occupancy grid MINUS a spill penalty. Overlap
   alone is gamed by one enormous part; the spill term forces an actual fit
 * MULTI-PASS STRIDES: each pass halves the box scale and doubles the gene count, so
   coarse structure settles before detail
 * HUNTING RANDOM WALK: mutation step decays with generation -- large jumps explore,
   small ones refine. A fixed step either misses the basin or never settles in it
 * LINKER: adjacent parts pulled a quarter of the way into contact after evolution,
   so the result reads as connected structure rather than a swarm -- the most
   reported fault of the earlier placement work
 * tournament-of-3 selection, uniform crossover, elitism
MEASURED on Jazz against a 280-part solid catalog:
     pass   genes    scale      best
        1       6     0.55    0.1297
        2      12     0.28    0.1980
        3      24     0.18    0.2487
        4      48     0.14    0.2856
Fitness climbs monotonically across passes exactly as the stride design intends, and
scales with compute (pop 50/gens 30 reached 0.1999; pop 90/gens 70 reached 0.2856).
BASELINE BUILT IN, because "it improved" is not evidence: every run scores the best
of 40 RANDOM assemblies and reports the margin. Evolution beat random by +875% and
+1424% on the two runs. If it ever fails to, the tool says the fitness function is
not guiding the search rather than reporting a number.
This is the first approach in the whole generative thread that clearly beats its own
null hypothesis. No GPU, no training data, and the output is real catalog geometry.

## Update 366 -- tuned the genetic fill; found where it plateaus and where it turns
Added seeded initialisation (genes placed onto cells the TARGET occupies rather than
anywhere in its bounding box -- Jazz fills 880 of 4096 cells, so uniform seeding
starts 79% of every gene in empty space) and coverage-directed mutation (a gene can
jump onto a cell the current best is MISSING, refreshed every 5 generations).
MEASURED, and the honest reading is more interesting than the headline:
 * Seeding lifted pass-4 STARTING fitness 0.056 -> 0.187, but final fitness barely
   moved (0.2856 -> 0.2942). The search finds the basin either way; seeding just
   arrives sooner. Recorded so the knob is not over-credited.
 * GENE COUNT is the real ceiling, up to a point:
       48 genes 0.2836 | 96 genes 0.3342 | 192 genes 0.3852 | 384 genes 0.3661
   384 is WORSE -- the search space grows faster than the budget to explore it. The
   tool now keeps the best pass rather than the last, and prints a line when a pass
   fails to improve, so the regression is visible instead of hidden.
The pass-6 result is genuine geometry, scored by the same tool used to rank the
library: 19,800 verts / 6,600 tris / 24 structural components, compactness 116.4,
0% open edges, class bot-bony. Watertight, structured, and classified BONY -- which
is what the entire detail effort has been trying to reach since the user first said
the parts read as faces rather than solids.

## Update 367 -- region + interior filling; the cavity question is now settled
Added --region (chest|legs|head|y0,y1,lateral) and --interior to `fill`.
REGION uses the DETECTED mirror axis. Assuming X first gave Jazz a chest region
0.14 wide x 2.00 deep -- a thin slice through the body with the full arm span still
in it -- because Jazz mirrors about Z. Third time this exact assumption has bitten
(rib bands, limb classification, now regions), and it is now detected everywhere.
With the axis right the region is a sane 0.68 x 0.66 x 0.40.
INTERIOR flood-fills from the grid boundary; whatever the flood cannot reach is
enclosed. This handles concave shapes properly where a convex hull would swallow the
gaps between limbs.
AND IT SETTLES SOMETHING THIS PROJECT KEPT CIRCLING. Enclosed cells, whole model,
16^3 grid:
    jazz 3 of 880 | grimlock 36 of 1056 | scorponok 10 of 713 | ravage 0 of 668
These models have essentially NO INTERIOR. They are layered panels, not shells
around voids. That is an independent confirmation of U323 (measured then by ray
depth: 728 front faces with 714 more behind them) arrived at by a completely
different method, and it explains every failed attempt to gut a chest and reveal
ribs. The tool now DETECTS this and falls back with an explanation rather than
optimising against a two-cell target.
Consequence recorded: `fill` is a whole-model or large-region tool on this library.
Cavity filling is unavailable and no tuning will change that -- the geometry is not
there.

## Update 368 -- harvested parts beat generated primitives, decisively
Tested the remaining quality question: does filling with REAL harvested parts beat
generated solid primitives? Same target, same budget:
    catalog                    IoU     tris  comps    compact   coher   open
    solid primitives        0.3342     3888     17      687.0   0.565     0%
    harvested real parts    0.3770    29869     96       55.2   0.814     2%
Harvested wins on fit (+13%), on STRUCTURE (96 components against 17 -- Scorponok
is 479), and on coherence (0.814 vs 0.565: smoother, more organised surfaces rather
than a heap of boxes). Cost is triangles, 30k vs 4k, which is the right trade for a
showcase model and is controlled by --passes.
NOTABLE: the primitives scored compactness 687 against the harvested 55. That is the
bony metric being GAMED -- disconnected boxes maximise area-per-volume without being
structured, exactly the noise failure the user predicted when the score was first
proposed. Coherence is what exposes it. Second confirmation that the score is a
candidate filter and not a verdict, which is why the Library page says so on screen.
Practical consequence: harvest a catalog from the best donors and fill with THAT,
not with solid_parts primitives. The primitives were built to fix "faces not solid
parts"; the fill does not need them, because occupancy does not care about
watertightness and real parts bring real character.

## Update 369 -- additive fill: 42 -> 138 structural members, silhouette intact
Installed an evolved model into the engine and compared it against the original by
eye (ascii), which changed the recommendation.
REPLACEMENT IS A DOWNGRADE. At IoU ~0.38 the evolved geometry occupies roughly the
right space but its silhouette is visibly coarser than the original -- swapping a
good model for it loses more than it gains. The numbers alone (96 components vs 42,
coherence 0.814 vs 0.753) would have suggested otherwise; the render did not.
Added --keep-shell so a whole-model fill is ADDITIVE: original plus evolved
structure.
    model                          tris   comps  compact   coher
    original jazz                 10660      42     29.2   0.753
    evolved (replacement)         27848      96     55.2   0.814
    additive (shell+evolved)      38508     138     35.3   0.792
138 structural members against the original 42, with the silhouette intact because
the shell is still present. That is precisely the combination this project has been
chasing since the first rib attempt -- more structure, same character -- and it is
the first version that achieves both rather than trading one for the other.
Installed into the engine: 81 models, build clean, 62/62.
Default is additive for --region, opt-in for whole models via --keep-shell.

## Update 370 -- batch fill; one model reached the Scorponok reference level
Added --batch to `fill`: walks a tree, takes the largest file per model folder, and
generates unique names itself. Doing that in a shell loop is how two models silently
became one earlier in this project, so the tool owns naming now.
Batched across two sample sets with --keep-shell (additive):
    model     tris   comps  compact  coher  class
    fill00   57459      53     38.8  0.804  other
    fill01   54486      66     33.1  0.770  bot-shell
    fill02   80586     100     33.9  0.779  bot-shell
    xfill00 308037     432     92.3  0.793  bot-bony   <-- Scorponok reference is 479
    xfill01  76236      93     33.3  0.764  tank/car
    xfill02  67836      86     27.8  0.772  bot-shell
    xfill03  88242      73     27.5  0.831  tank/car
xfill00 reaching 432 structural members at compactness 92.3 and classified bot-bony
is the first time anything in this project has approached the reference density that
has been the target since Update 301.
Installed: 87 models, build clean, 62/62.
INCIDENT: the container filled to 100% mid-run, which failed the build and dropped
ctest to 66% (21 failures) -- and the failure looked like a code regression rather
than a disk problem. Freed 1.4GB and it returned to 62/62 with no code change.
Second time disk exhaustion has produced a misleading failure this session; the
first silently truncated a source file to 0 bytes.

## Update 371 -- Fill page in the GUI, with its measured caveats on screen
Added a Fill page to the egui app, promoted out of the GAN page because genetic fill
is now the approach that actually works.
Controls: target (with "use Library selection"), catalog (plus a Harvest button that
builds one from the Library root), region whole/chest/legs/head, keep-shell toggle,
and passes/pop/gens.
Three measured facts are stated ON THE PAGE rather than left to be discovered, each
because it is easy to get wrong and expensive to get wrong:
 * harvested parts beat generated primitives (+13% fit, 5.6x components,
   coherence 0.81 vs 0.57) -- otherwise the catalog choice looks arbitrary
 * unticking keep-shell REPLACES the model, and at IoU ~0.38 that is a rougher
   silhouette rather than a better one -- warned in amber, because the numbers
   alone say the opposite
 * 7 passes measured WORSE than 6 (0.366 vs 0.385) -- warned at the point of
   choosing rather than after a long run
That pattern is now consistent across the app: the Library page labels the bony
score "a filter, not a verdict", the Raids page shows the premise line beside the
mechanic it produced, and the GAN page prints its standing PARTIAL result. Where a
tool has made a judgement call or a metric can mislead, the UI says so.
CLI still compiles clean; the egui layer remains the one unverified piece.

## Update 372 -- rigscore: humanoid riggability, validated against known failures
Added rust/assetkit/src/rigscore.rs and `assetkit-cli rigscore`. Scores a model for
humanoid motion BEFORE rigging it -- the limb splitter's failures were previously
only discovered by measuring joint gaps through a walk cycle, after shipping.
Four components: SEPARABILITY (is geometry actually in limbs), SYMMETRY (worst
left/right pair by mass and mirrored position), PROPORTION (limb spans against
anthropometric fractions of stature -- head .130, shoulder .259, thigh .245,
shin .246, leg .530), ARTICULATION (is there a waist to pivot about).
VALIDATED AGAINST OUTCOMES ALREADY MEASURED, which caught a real flaw:
    model       sep   sym  prop  artic  TOTAL  verdict   what happened
    jazz       0.73  0.99  0.65   0.08   0.71  RIGGABLE  rigged, gap 0.084
    grimlock   0.96  1.00  0.72   0.00   0.78  RIGGABLE  rigged, gap 0.45
    zetaprime  1.00  0.49  0.56   0.00   0.58  MARGINAL  FAILED, arms 6818 v 100
    soldier    0.74  0.00  0.46   0.52   0.39  MARGINAL  FAILED, fell back
The first version averaged the limb pairs and scored Zeta Prime 0.59 symmetry ->
RIGGABLE, a false positive: its legs were fine and the mean hid arms of 44 and 5,874
triangles. Changed symmetry to the WORST pair -- an animation fails at its worst
joint, not its average one -- and it drops to 0.49 MARGINAL.
Worth noting the correction came from checking against outcomes already on record
rather than from reasoning about the metric. The bony score, the GAN verdict
thresholds and now this have each needed the same treatment.

## Update 373 -- ship hub: rooms, travel modes and console destinations
User asked for a homebase RPG structure -- upgrade room, spacewalk, vendors, quests,
a console that picks the destination, with driving/flying/landing to get there.
Surveyed the engine first: it already has a landing sequence (g_landing_active,
g_landing_ring, g_land_impact), flight pitch/roll, a ship model and consoles -- but
the consoles are arena props and there is no hub.
Built the hub as DATA, matching the raids pattern: rust/assetkit/src/hub.rs +
`assetkit-cli hub` emitting content/hub.json and engine_src/exengine/hub_plan.h.
8 rooms along the ship's spine (bridge forward, hangar/airlock aft), 10 console
destinations. Travel is a MODE not a loading screen -- drive 3, fly 3, land 4 --
because the engine already has the systems and arrival should use them.
ROOMS ARE A GRAPH, NOT A LIST, and three checks enforce what that implies, each for
a bug invisible until someone walks into it:
  * every room reachable from the bridge (an upgrade bay nobody can reach is worse
    than no upgrade bay)
  * every exit mutual (a one-way door looks entirely reasonable in a table)
  * every destination map exists in maps.json (a raid phase pointing at an undefined
    map was a real bug that shipped in this project)
It refuses to write if any fails.
Verified the generated header compiles and reads correctly from C with a standalone
harness: 8 rooms with correct exit indices, 10 destinations with travel codes.
NOT DONE: the engine does not yet consume hub_plan.h -- no EX_GS_HUB state, no room
rendering, no console interaction. This is the data and the validation; the runtime
is the next piece and it is substantially larger.

## Update 374 -- hub fitted to arbitrary hulls, with overlap and containment checks
`assetkit-cli hub --ship HULL` fits the room graph into a real hull instead of using
authored positions. Rooms keep their ORDER along the spine and their exits, but
centre and half-extents come from the hull -- so one layout serves a stubby lander
and a long cruiser.
Long axis detected rather than assumed. Cross-sections taken at the 15th/85th
percentile so one antenna or stray vertex does not define a room.
Two more validations, both for faults invisible until someone walks the corridor:
no room overlaps (a hub you can fall between), and no room outside the hull (which
shows the outside from inside -- a hole, not a window).
Verified on two very different hulls: ship.iqm (1,206 tris) and Grimlock (13,768) --
both fit 8 rooms, no overlaps, nothing outside. Generated hub_fit.h compiles and
reads correctly from C with exit indices resolving to the right room names.
BUG WORTH RECORDING: the first version floored room size at 0.6, a figure written
for metres. Meshes are normalised to -1..1, so on a 2-unit hull EVERY half-extent
clamped to 0.6 -- all eight rooms became the same oversized box, overlapped each
other, and the containment check reported all eight outside the hull. The floor is
now a fraction of hull size. Absolute constants and normalised coordinates do not
mix, and the symptom (everything fails at once) looked like a much deeper problem
than it was.

## Update 375 -- engine port: scoped honestly, with a differentially-tested example
User asked whether the engine can be ported from C to Rust. Measured it rather than
guessing:
    game.c            11,932 lines, 745 static globals, 171 SDL calls
    exengine (logic)   6,123 lines, no SDL/GL/Vulkan
    exengine (gfx/os)    628 lines
ANSWER: the 6,123 pure-logic lines are worth porting and can be VERIFIED. game.c is
not -- 745 interdependent globals driving SDL and a Vulkan backend is an
architectural rewrite, not a translation, and none of it is checkable without a
display. "It compiles" would be the only evidence, and this project has repeatedly
shown what unverified graphics code is worth.
Demonstrated the method rather than asserting it. Ported combat.c (ray_aabb slab
method + ray_sphere) and DIFFERENTIALLY TESTED it: C and Rust each run 200 random
ray casts from a shared xorshift stream and print results.
    C lines: 400  Rust lines: 400  -- IDENTICAL, bit for bit
including the edge cases: rays parallel to a slab, roots behind the origin, hits
beyond max_dist. Set up rust/enginecore/ with the module, unit tests, and PORTING.md
recording the scope, the method, and a ranked list of the next ten candidates
(mapgen 332, robot_parts 348, signature 234, encounter 253, enemy 288, lore 299...).
Each is self-contained enough to port and diff-test in isolation and can be called
from the C via FFI once ported, so the engine keeps running throughout -- no
big-bang switchover.
Rule recorded: no module counts as ported until it passes a differential test.
Reading C and writing equivalent-looking Rust is exactly how float-ordering and
early-exit differences survive into a "faithful" port.

<!-- The two entries below originated on the monorepo line (games/blackstar)
     while the standalone cybertron line produced Updates 35-375. They were
     both numbered 35/36 there; renumbered to 376/377 on merge. Content
     unchanged. -->

## Update 376 -- ESCALATION mode (WFC's signature horde loop) + energon economy
Adds Escalation as a third skirmish mode, built on the existing wave-loop
framework (waves.h/.c), the same one Survival already used:
- NEW MODE `EX_MODE_ESCALATION` (menu.h/menu.c): endless waves, selectable by
  cycling the skirmish mode picker like Survival/Arena Fight.
- ENERGON ECONOMY (waves.h/.c): kills bank energon credits (grunt 20 /
  hijack 30 / corrupted 40 / apex 120, `ex_waves_energon_value`). When a wave
  clears, the whole bank is FORGED into a permanent +max-integrity increase
  (0.4x the banked total) plus a full repair and a 30-armor floor -- so
  surviving compounds your strength, and playing greedy (banking more before
  the wave clears) pays off. `forge_event`/`forge_hp_amount` fire for one tick
  on the wave-clear frame; `forged_total` tracks the lifetime forge for the HUD.
- game.c wiring: Escalation dispatches like Survival (endless waves, 60s
  survive floor) but additionally applies the forge to g_player_max_hp/hp/armor
  on `forge_event`, with a "FORGED +N integrity" toast. HUD gains an
  `ENERGON n  forged n  integrity n` line, shown only in Escalation.
test_waves.c extended: energon values, banking across kill kinds, forge math
(banked*0.4), forged_total accumulation, and the event flag clearing next tick.
All 41 tests pass; game.c compiles clean (link needs a system SDL2 runtime,
unrelated to this change -- not present in this sandbox).

## Update 377 -- Escalation triple: titan-boss waves, buy-a-door gates, energon perks
The three follow-ups queued after Escalation's energon economy landed:

1. TITAN-CLASS BOSS WAVES: every 5th wave is now a titan wave (`ex_wave_makeup_t.
   titan`, waves.h/.c). The regular grunt/corrupted/hijack makeup is thinned so a
   titan-class apex boss (hp = 900 + 120*wave, scaling with progress) always gets
   a guaranteed spawn slot inside the 12-enemy encounter budget. Reuses the
   existing apex boss kit wholesale -- phase/slam/summon AI, the boss health bar,
   the render styling -- all of which was already generic, not campaign-only, so
   this needed no new combat code, just a `ex_encounter_boss()` call in
   spawn_wave_makeup + a "TITAN AWAKENS" toast.

2. BUY-A-DOOR MAP EXPANSION: new pure-logic module `gates.c/.h` -- a 3-gate chain
   (causeway 300 energon -> foundry wing 750 -> outer rim causeway 1500), each
   permanently expanding the arena's spawn half-extent (60 -> 100 -> 150 -> 220)
   once opened. Keys 1/2/3 attempt to open gate 0/1/2 straight from the banked
   energon wallet (spends before it would otherwise auto-forge). HUD shows the
   next affordable gate + its cost; spawn_wave_makeup now spawns into
   `ex_gates_extent(g_gates)` instead of a fixed 60 in Escalation.

3. ENERGON PERK SHOP: `ex_waves_spend()` (waves.h/.c) is the shared spend gate
   used by both gates and perks. Keys 4-7 buy: repair (+50 hp, 150), shield
   (+40 armor, 200), overcharge surge (12s, 250, reuses the existing overcharge
   pickup's buff), and +80 repair energy for the part-repair system (100). All
   mid-run, competing with the gates for the same wallet that forges on wave
   clear -- so spending vs. banking-for-the-forge is now a real tradeoff.

HUD: ENERGON/forged/integrity line moved to row 9 (was colliding with the
corruption-stage line at row 8); added a "[n] open <gate> -- cost" hint row.
test_waves.c extended (titan escalation + budget-fit assertions, ex_waves_spend);
new test_gates.c (afford/deny, no double-spend, extent tracks the largest open
gate, next-locked index). gates.c wired into CMakeLists' module sources + test
list. All 42 tests pass; game.c compiles clean (SDL2 runtime link unavailable
in this sandbox, as before -- unrelated to these changes).
