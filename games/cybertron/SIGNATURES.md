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
