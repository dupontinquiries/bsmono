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
-29. [x] **Scent on the wind — reading the air like a cat does.** Scent is no longer just an
   invisible trail the hunter tracks; it's something you sense and *see*.
   * **`Game::read_wind`** — what your nose catches this instant. Scent drifts downwind, so
     you smell prey and the predator that lie **upwind**: approach prey from downwind and
     you scent it before it bolts; drop downwind of the hunter and its reek warns you early.
     Returns the strongest prey's direction/distance/strength + its name, the predator's
     reek (0..1), and the wind vector. Keen-nosed clans and a strong wind carry scent
     farther. Tested (a hunter upwind reeks; one downwind you can't smell at all).
   * **Visible scent** — `build_scent_wisps` renders faint motes drifting downwind: your own
     pale trail lifting and thinning behind you, a warm thread from the nearest prey you can
     smell (follow it upwind to the catch), and the predator's red reek drifting to you when
     you're downwind. In the headless game view and the app.
   * **HUD readout** — the world panel now whispers what the wind carries: "rabbit-scent on
     the breeze — upwind", or "the reek of the fox is on the wind".
   1 new test → 125 total. Together with -28 (cover, snag, freeze-and-taste-air) a crossing
   of the territory is now read through the nose as much as the eyes. (All original.)
-28. [x] **Tactile traversal — making crossing the world feel like the writing.** The world
   now pushes back the way a cat's does: undergrowth to slip through half-seen, brush that
   drags, a bramble that snags, and stillness that sharpens the senses.
   * **Cover field** — `world::cover_at(x,z)` gives a patchy undergrowth density (thick in
     the oak floor / reeds / pines, thin on the moor and scree, near-zero on open water).
     Tested that the woods hold more cover than the open.
   * **Slip through half-seen** — a cat in cover is harder to spot: the hunter's suspicion
     builds up to 50% slower on a cat standing in thick growth. Crossing open ground is
     fast but exposed; threading the brush is slow but hidden.
   * **Brush drag + bramble snag** — pushing through cover shaves a little speed, and now
     and then (only when running a real thicket) a trailing bramble **snags** you for a
     brief tug you pull free of (`Cat.snag_t`). Creeping crouched slips clean.
   * **Freeze and taste the air** — a *still* cat reads the wind ~35% farther than a moving
     one, so stopping to scent genuinely extends your danger-sense (tested).
   * **Animation** — a `wade` gait (hunched, shouldering into the brush, head dipped) when
     moving through thick cover, and a `taste_air` pose (muzzle lifted, casting side to
     side, quick scenting breaths) when the player freezes with danger about.
   4 new tests → 124 total. (All original systems; the book was reference only, for the
   *feel* of how cats move through undergrowth — no text or names taken from it.)
-27. [x] **Flick controls + a combo move-set (`combat.rs`).** A stylish, chainable fighting
   layer, continuing the "rich journey" thread into confrontation:
   * **Moves** — seven: Pounce, Jump, Roll, Dodge, Underbelly Swipe, Hind Kick, Swat, each
     with windup/active/recovery timings, i-frames, power and lunge (`combat::spec`).
   * **Flick controls** — `resolve_flick(dir_local, mag, low)` maps a fast aim-flick to a
     move by direction: forward→Pounce, back→Hind Kick, sideways→Roll, sharp/directionless
     →Swat, low-modifier→Underbelly Swipe. In-app a fast mouse-flick + left-click performs
     it (right-click = low modifier); a bare click swats.
   * **Combos** — `combo(prev, next)` chains two moves inside a 0.55s window into named
     finishers (Ambush Pounce, Counter Gut, Rake-Off, Pin & Bat, Rising Pounce, Flurry,
     Kick Away) that hit 1.5× harder. HUD flashes the combo name.
   * **Sim integration** — `Game::player_flick` performs a move (pounce/roll reuse the
     leap/dodge mechanics); an attack that connects within reach **staggers the hunter**
     (it's dazed, skips its chase/catch), and a Hind Kick or Underbelly Swipe **wrenches
     you free of the hawk's grab**. New poses (underbelly swipe, hind kick, jump) + a
     `move_pose` bridge, rendered for the player's active move.
   9 new tests (4 combat-module + 2 cat-pose + 3 sim-integration) → 117 total.
   (System + integration verified in sandbox; the flick feel/combo HUD are compile-only.)
-26. [x] **Traversal variety — "every trip A→B is a rich journey."** Movement was one flat
   sprint; now it's a continuum of gaits and events:
   * **Gaits by speed** — `cat::gait`/`gait_name` pick a walk that quickens to a trot and
     breaks into a bounding gallop, keyed to each cat's *actual* travelled speed
     (`Cat.speed`, measured per step), with a per-cat phase offset. Rendered in the
     headless game view and the app.
   * **Paced by danger** — the player no longer moves at one speed: it ambles across safe
     ground and accelerates toward a sprint as the hunter closes (pace = f(distance to
     hunter)); only a real bolt drains stamina, and a winded cat can't gallop. Verified a
     lone journey shifts walk↔trot as the hunter nears/recedes.
   * **Bounding over logs** — the fallen logs are now in the sim (`Game.logs` via
     `world::log_positions`); a running cat that crosses one auto-leaps over it (a short
     bound), so obstacles punctuate a run. Tested.
   3 new tests (gait shifts with speed, bounds over a log, + the pace threading) → 109
   total. Together with -25 (grass/litter/twig-snap/paw-prints), a crossing of the
   territory now reads as amble → trot → bound over a log → snap a twig → bolt.
-25. [x] **Ground-level immersion pass ("think like a cat").**
   * **Grass, everywhere** — grass was gated to only grow under trees, so the whole open
     moor was bare. Rewrote `build_grass`: it now covers open/temperate ground (moor, oak,
     reeds, mangrove, shore), 3–5 taller blades per tuft, green-tinted and leaning with a
     prevailing wind, ~4× denser. Coverage went from sparse to ~29% of the frame.
   * **Forest-floor litter + logs (art)** — new `build_litter` (fallen twigs + scattered
     autumn leaves, thickest under the trees) and `build_logs` (thick bark cylinders to
     bound over / hide behind), density following each biome's tree cover. Wired into the
     headless game view and the app world.
   * **Twig-snap stealth (system)** — a cat *running* (not crouched) through the leaf-litter
     can crack a twig underfoot (`world::twig_positions` → `Game.twigs`); the twig is
     consumed and, if a hunter is within earshot (30u), the sharp sound spikes its suspicion
     and turns its head toward the noise. Crouched/creeping cats, airborne leapers and
     climbers stay silent. An on-screen "*SNAP!*" cue fires (`noise_flash`). Tested both
     ways (running snaps + is heard; crouched leaves twigs intact and silent).
   * **Paw prints (art)** — soft dark paw marks pressed into the ground along the pack's
     scent trail, fading with it, in both the headless and app renders.
   2 new tests → 107 total. Still on the wishlist for later passes: scent-marking borders,
   grass that swishes as you brush past, mud that soils your paws, ambient sound.
-24. [x] **Two new clans, a mangrove biome, and an AAA graphics mode.**
   * **Scent trait + clans** — `ClanTrait` gained a fifth attribute, **scent** (a keen
     nose). Added **Fenclan** (the misted fen — best nose, "trackers") and **Mangroveclan**
     (the tangled mangroves — fish-eating ocelots that swim and scent). `CLAN_COUNT` is now
     6; keen-nosed clans feel the predator's danger from further off (danger sense range
     ×scent) — tested (Fenclan senses at 55u where Amberclan can't). Overlays extended to
     5 bars (showcase + HUD, scent = orange); the clan showcase now lines up all six.
   * **Mangrove biome** — added `MANGROVE` ("Mangrove Shallows") to `BIOMES` (now 7);
     `biome_at` slots it into the wettest shoreline band (moisture > 0.72). Verified
     reachable on the map; the seam-blend test still passes.
   * **AAA hi-res mode** — `CLAW_HIRES=1` (headless) supersamples 1.85x (capped to the
     2048 texture limit) and scales geometry ×1.8: verified 1100x680/398k verts →
     2035x1258/788k verts. In-app it's a **Settings → Graphics: Standard/High (AAA)**
     toggle that rebuilds the scene target at 1.85x supersampling on the fly (surface stays
     at window size). 3 new tests → 105 total.
-23. [x] **Suspense system for the hunted (6v1) mode.** Added a `danger` level (0..1) on
   the Game that swells with the hunter's nearness weighted by how switched-on it is
   (point-blank chase ≈ 1, distant patrol ≈ 0) and eases — spiking fast, fading slow.
   Verified it reaches 0.99 at close encounters in a real hunt. It drives three things:
   (a) a **heartbeat vignette** in the post shader — the edges pulse and close in on a
   beat that pounds faster/harder as danger rises, bleeding red near a catch (fx.w);
   (b) a **directional THREAT marker** at the screen edge pointing to where the hunter is
   closing from (camera-relative bearing); and (c) a **stalking creep** — when the hunter
   is nosing around right where it last had you (searching/tracking near its last-seen
   point) it slows to 0.4× and casts about, a patient menace instead of a flat sprint.
   4 new tests (danger spikes on approach, hunter creeps on the trail, + module) → 104
   total. (danger + creep verified in sandbox; heartbeat/marker compile-only.)
-22. [x] **Clan traits + overlays** (Warriors-inspired). The four clans (Ember/Tide/Moss/
   Amber) were identical teams; now each is adapted to its home terrain via `clan.rs`
   (shared) with four diverging attributes — **climb** (Mossclan, the oak canopy),
   **sprint** (Amberclan, the open moor), **swim** (Tideclan, the reed river), **stealth**
   (Emberclan, the shadowed pines) — as balanced trade-off multipliers. Wired into the
   6v1 sim: the pack's clan scales climb ascent/stamina, water speed, open-ground running,
   and how tightly it hides (flush radius). Overlays to read the values: a headless
   **`clans` showcase** with floating per-trait bars (climb=green/sprint=gold/swim=blue/
   stealth=violet — verified all four render), an in-game **HUD clan panel** with the four
   trait bars + specialty tag, and a **clan-colour collar** on every cat. Menu gains a
   **Clan** selector row. 8 new tests (3 clan-module + 4 sim-trait + menu) → 102 total.
   (Sim traits + showcase verified in sandbox; HUD panel/menu compile-only.)
-21. [x] **Refined tree climbing.** It used to be instant and binary — press crouch at a
   trunk and the cat teleported up (+4.2) with full safety, no cost. Now it's a real
   climb: the cat **scrambles up gradually** (`climb_t` 0→1 over ~½s, rendered at that
   height), **hugs the actual trunk** (x/z snap onto the tree), and **clinging costs
   stamina** (net drain), so you can't roost forever. Safety is keyed to *height*
   (`climb_t > CLIMB_SAFE`) rather than the on/off flag, and a **hysteresis** means once
   you slip below safe height you need real stamina to re-commit — so an **exhausted cat
   slips out of the tree** and is briefly catchable by the ground hunter. The hawk still
   plucks a climber (aerial), and reaching the beacon / being grabbed resets the height.
   3 tests (gradual+trunk-hug+stamina, exhaustion-slips-you, and the original
   escapes-ground-but-not-hawk still green). 96 tests total.
-20. [x] **Dithering + biome blending.** (a) Added ordered **Bayer 4x4 dithering** as the
   final post-process step: the colour is quantised along a 4x4 threshold pattern, which
   breaks up smooth-gradient banding (sky, fog, shadow) and lays down a printed-paper
   texture that suits the papercraft look. Verified active — a smooth sky band shows
   ~747/1099 adjacent pixels alternating (the dither signature; a flat gradient would be
   near-uniform). (b) Widened the **biome-colour blend** kernel from a single 9-tap ring
   (band ≈ 2r, r=3.6) to a two-ring 13-tap kernel (centre + inner 8 + outer 4, r=4.2,
   reach 2r) with distance-falloff weights, so biomes crossfade over a wider, softer band.
   The seam-smoothing test still passes. 94 tests.
-19. [x] **Wind + weather system** (`weather.rs`, shared) — Warriors + SMITE-2 flavoured.
   Four **seasons** (Newleaf … Leaf-bare) set prey abundance and which skies roll in;
   the **sky shifts during a match** (clear → rain → fog → storm → snow) on a timer, and
   a gusty, veering **wind** carries scent. Modifiers: rain/storm wash scent away and
   scatter it downwind while fog/snow let it linger (tracks in snow); fog and storm blind
   the hunter; season + weather set how much prey is about. Integrated into the sim —
   dynamic wind now drives the scent drift, weather scales scent decay and the hunter's
   suspicion build-up, and the season sizes the wildlife population. Rendered as rain /
   snow / storm **particles** (slanted by wind) in the headless game view and the app sim
   path (verified: 0 rain-px clear, 589 storm, 1159 snow); a HUD line shows
   "Greenleaf · rain · brisk wind", and `CLAW_SKY=rain|snow|storm|fog` forces the sky for
   headless previews. 7 new tests (5 weather unit + 2 sim integration) → 94 total.
   (Weather logic + particles verified in sandbox; live feel on the M2 still to confirm.
   Prowl/Pop Island don't share the weather yet — a clean follow-up since it's one field.)
-18. [x] Animation overhaul from playtest feedback:
   * **Model rotation (bug)** — the cat rig's `build` only *flipped* along z (every caller
     passed 1.0), so cats never faced their heading. Added `mesh::boxx_yaw` (an oriented
     box) and rewrote `cat::build(at, yaw, pose)` to rotate the whole rig — body, head,
     ears, tail, legs — about the vertical axis. Every gameplay caller now passes the
     entity's `heading` (sim cats, prowler, minions, rivals, capture mobs). Unit test
     proves it (facing +z the cat is longer along z; a quarter-turn makes it longer along
     x). This also gives "shoulders turning when turning" for free.
   * **Walk too fast** — the phase was `frame*0.06`, i.e. ~7 strides/sec at 120Hz. Added a
     real-time `anim_t` accumulator; walk now runs a framerate-independent ~1.35 strides/sec
     with a per-cat gait offset so they don't march in lockstep.
   * **Naturalization** — new `cat::idle(t)` breathing pose (chest expansion, body rise,
     slow head glances, lazy tail sway) used whenever a creature is standing; `Pose` gained
     `head_yaw` + `breath`.
   * **Combat groundwork** — added `stance(t)` (low ready-crouch, tail lashing) and
     `strike(t)` (a swipe that peaks mid-swing) poses, with tests. NOTE: these are the
     animation vocabulary; full combat *gameplay* (attack action, damage, hit reactions,
     stances wired into the loop) is still to do. 87 tests. (Rig math + poses verified in
     sandbox; the live look on the M2 is still to confirm.)
-27. [x] **Signature class abilities** (abilities + character design + a new control). Each of
   the six classes now has a distinct active ability in the shared `class` module, on its own
   cooldown: Cat **Second Wind** (refill stamina + briefly untouchable), Squirrel **Scamper**
   (blazing sprint), Otter **Dive** (fast + untouchable getaway), Frog **Bound** (huge airborne
   leap), Goat **Charge** (unstoppable rush), Hawk **Stoop** (screaming dive). Each has tuned
   cooldown/duration/speed-multiplier/invuln/refill params. Wired into Prowl: press **Q** (or
   right-click) to fire it — activation, cooldown, speed burst, i-frames, and stamina refill all
   handled in `step`. The HUD shows the ability name, READY/cooldown, and a blurb; controls hint
   updated. 3 tests (every class has a unique ability; Second Wind refills + shields a catch;
   Scamper covers more ground). 84 tests. So classes finally play distinctly, not just move at
   different terrain speeds. (World interactions + more map design are the continuing direction.)
-26. [x] **Win condition + rivals flee the stalker** — the four-way match now has a proper
   end. A clan wins outright at **600 points**, otherwise the **leader when the ring finishes
   closing (~120s)** takes it (ties → stalemate). `outcome: Option<u8>` freezes the sim once
   decided. Rivals now **break off to flee** the stalker when it's hunting them nearby (rather
   than only reacting once caught). The app shows a result banner ("YOUR CLAN WINS" / "TIDE
   CLAN WINS" / "STALEMATE") with R to restart. Verified integrated: a full match ran to the
   120s ring-close and awarded Amber the win on a [0,120,160,240] score. 2 tests (score
   threshold wins + freezes; ring-close awards the leader). 81 tests.
-25. [x] **Rival AI prowlers → a live 1v1v1v1 match.** Refactored ancestor ownership from a
   single bool to `claimed_by: Option<u8>` (which clan owns each tree) and the score from one
   int to `clan_scores: [i32; 4]`. Added one **AI prowler per rival clan**: each roams to the
   nearest tree it doesn't own, dwells to claim it (flipping ownership + banking +40 for its
   clan), and rallies home (losing points) if the stalker catches it — so the four clans
   genuinely fight over the ancestors. Trees glow in their **owning clan's colour**; rivals
   render as full-size clan-tinted cats; the HUD shows a four-clan scoreboard (Ember/Tide/
   Moss/Amber). Verified integrated: over 120s the rivals claimed and flipped trees to a
   [0,120,160,240] score spread with two rivals contesting the same tree. 2 tests (a rival
   claims a wild tree for its clan + scores; one prowler per rival clan). 79 tests.
-24. [x] **Four clans + border-patrol minions** in Prowl — a 1v1v1v1 territorial map. The
   four ancestor trees are now four **clan homes** (Ember / Tide / Moss / Amber), each
   defended by a ring of small **cat minions** that walk a border-patrol loop around their
   territory. The player prowls as clan 0. Trespass near a **rival** clan's minions and they
   break off to shoo you — and a rival spotting you **raises the hunt alarm** (bumps the
   stalker's suspicion), so raiding enemy ground is risky. Minions render as small clan-tinted
   cats (patrol animation) in both the live and headless renders. 2 tests (patrol stays near
   the clan border + moves; rival minions aggro on an intruder, advance, and raise the alarm).
   78 tests. (Rival AI prowlers per clan would be the next step toward a full 4-way match.)
-23. [x] **Lanes** in Prowl — a MOBA-style path network tying the map together. A spoke runs
   from the middle out to each ancestor tree plus a ring linking neighbours, so the staging
   grounds are connected by deliberate routes. **Travel is faster on a lane** (×1.25), giving
   a fast-but-open rotation option vs slower-but-hideable off-lane movement. `on_lane_at`
   (point-to-segment test) drives the bonus and the render. Drawn as worn dirt-path ribbons
   hugging the terrain in both the live and headless renders (~8% of the showcase frame).
   Test: a spoke midpoint is on a lane, and travel there beats the same move off-lane. 76 tests.
-22. [x] **Ancestor trees** in Prowl — SMITE-style buff camps themed as defensible staging
   grounds, spread far out across the map (radius ~95, on dry land) so the arena plays big:
   * **Claim by dwelling** — hold near a tree for a few seconds to claim it; it bestows its
     stored buff (Speed or Shield) and rests on cooldown. Banks +40 score.
   * **Staging ground** — a claimed tree shelters you: standing in its grounds mends stamina
     and cools the hunt (suspicion decays faster), and getting caught **rallies you at the
     nearest claimed ancestor** instead of the origin (a forward respawn).
   * **Render + animation + texture** — a grand tiered papercraft tree on a heavy flared-root
     trunk with a **swaying canopy** and an aura ring (gold = claimable, green = claimed
     staging, dim = resting). Drawn in both the live window and headless renders; HUD shows a
     "STAGING GROUND" indicator.
   2 new tests (dwelling claims + grants the buff + is a staging ground; a catch rallies you
   at a claimed tree). 75 tests, both builds clean.
-21. [x] Built out the **Prowl (SMITE-2) mode** with MOBA-flavored depth, reusing the
   shared `powerup` module:
   * **Power-up buffs in the field** — Speed and Shield pickups scatter on dry land; grab
     one for a boost. A **Shield soaks a catch** (you're spared instead of respawning, the
     shield pops) — a defensive-item mechanic. Buffs clear when you're actually caught.
   * **Hunt score + escalation** — clearing an objective banks +100 and ramps difficulty
     (+0.05, capped) like waves; pouncing prey is +25; getting caught costs −50 (floored
     at 0). So the hunt gets harder the better you do.
   * **Wiring** — the app HUD shows score + active buffs (SPD/SHLD), a shield-soak shows no
     damage tint, and both the live and headless renders draw the bobbing pickups.
   4 new tests (shield soaks a catch, pickup grants a buff, clear banks score + escalates,
   plus the existing respawn path). Verified integrated (buffs grabbed, clears escalate,
   score arithmetic correct). 73 tests.
-20. [x] **Match seeding for replayability.** `new_match` was fully deterministic — every
   Rendezvous-vs-Fox match spawned cats at the same angles, picked obj[0], and placed the
   hunter identically, so every match played out the same. Added `new_match_seeded(kind,
   mode, seed)`: the seed rotates the objective choice and scatters the cats' spawn
   positions, so repeat matches differ (verified: seeds 0–5 give caught/escaped spreads of
   0–3 / 1–4). `seed == 0` reproduces the legacy fixed layout exactly, so determinism and
   all existing tests are untouched. The live window seeds each new match/restart from the
   frame counter. Test covers both the variation and the seed-0 legacy guarantee. 70 tests.
-19. [x] Consolidation pass. (a) **Fixed a web-build regression before it shipped**: last
   turn's real-dt fix used `std::time::Instant`, which panics at runtime on wasm32 (no
   clock without a shim). Swapped in the `web-time` crate — a drop-in `Instant` that's
   plain std on native (so native behavior is identical and verified) and uses
   `performance.now()` on wasm. (b) **Rewrote the README** to match reality: it still
   described a single 6v1 game with three controls; it now documents all four modes, the
   full headless-render arg matrix, the live-window menu + per-mode controls (incl. the
   new mouse binds), difficulty, the shared-lib architecture (all 17 modules), and the
   test suite. Verified the arg/mode claims against main.rs. 69 tests, both builds clean.
-18. [x] Completed the difficulty system — it scaled hunter speed + suspicion but a hider
   was equally safe at every setting, which was inconsistent. Now the **flush radius**
   (how close a hidden cat must be for the hunter to sniff it out) also scales with
   difficulty, so hard mode makes cover genuinely riskier while normal play is unchanged
   (×1.0 leaves it as-is). Test: a hider just outside the normal flush distance stays safe
   on easy but is rooted out on hard. Fixed the now-stale "affects aggression later"
   comment. 69 tests.
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
