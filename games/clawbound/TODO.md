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
- [ ] Textual HUD (timer/cats-left). ~~netcode transport for a human hunter~~ — c21: `sim::Game::set_hunter_input` + `hunt_net` (host-authoritative state over the fleet socket; the joiner plays the hunter). Unplayed on two machines.


- [x] SHADOW enemy #6 (afterlife dark-warrior; Silent Hill + Freddy + Tigerstar): teleport-blink to its quarry, otherworld SHIFT that reveals hiders/reaches climbers/empowers it; dark spectral model + otherworld screen tint; playable via key 6 (nightmare tier: catches 5-6/6)


- [x] AI vision cone hidden by default (debug-only, toggle key V / Settings); terrain given fuzzy high-freq colour noise so translucent overlays melt into the ground
- [x] egui menu (egui 0.28 + egui-winit 0.28 + hand-rolled wgpu renderer, unendlich-style): Title (Single Player/Multiplayer/Settings/Keybinds/Quit), hunter+mode pick, settings (cone/volume/difficulty), rebindable keys; menu.rs logic unit-tested; Esc opens, game pauses


- [x] Nightmare/otherworld atmospherics (Shadow shift): near-black violet sky, stars snuffed out (robust hash), long shadows crawling up from the trees, spectral cats drifting in the underworld, sickly desaturation + closing vignette. `tod=nightmare` render + driven live by the shift in --play (post fx uniform)

## Battleground of the Ancestors (clan-war mode)
- [x] `ancestor.rs` — six legendary ancestors (Goldenmane / Duskstripe / Rainspots of the great
  clans of legend; Nightpelt / Windclaw / Stonewhisper of the far wild), each a passive + two
  techniques + an ultimate expressed as generic verbs (Strike/Dash/Aoe/Shield/Stealth/Heal/Fear/Slow)
  so the sim stays kit-agnostic. Budget-band balance test in the clan.rs style.
- [x] `battleground.rs` — headless deterministic sim: large map (camps a >200 m march apart, three
  trails), Trees of Healing (heal + guard auras; roots harden the Great Tree while any stand),
  freshkill economy (hunt at prey camps / the contention ground, carry home, saturating growth —
  no shop), cat AI patrols (2 per wave, strong, retreat past 45% hp to the medic cat, rejoin at
  90%; last-stand only at the Great Tree), herb patches (gather + deliver → faster healing and
  respawns). 13 unit tests + determinism digest; `examples/battleground_report.rs` prints match
  timelines per ancestor.
- [x] Passives made real (they were flavor text): Pride's Warmth aura, Night's Hunger +
  Silent Footfall in the dark or from the veil, Riverborn water-mending, Skyward Hunger
  kill-stacks, Sure-Footed half-slows and shallow-water striding. Day/night watches
  (90s each) so the ambush ancestors have a rhythm; unit-tested per passive.
- [x] Patrol duties from the books: **border patrols** walk the trail, **hunting patrols**
  (every third wave) work the prey camps and carry freshkill home on their own. Prey has
  kinds now (mouse/rabbit/fish/rich) — land prey bolts from a close hunter on a 12m leash,
  fish hold to mirrored fishing runs.
- [x] Leaf-bare late game (300s): auras thin, border patrols march 3 strong and press 1.5x —
  plus MOBA death timers that grow with match age. Together these turned the 8-minute
  full-time stalemate into real fells (4/6 headless matches end at a Great Tree now).
- [x] Breach-lane massing: once an enemy Heal Tree falls, patrol spawns and warriors
  rejoining the trail funnel onto the breach — the three-lane stalemate breaker.
- [x] Balance debugging via the report harness: found and fixed a *systematic* economy bias —
  patrol duty was derived from the global tick (`tick/7 + team`), which handed clan B its
  hunting patrols ~78s before clan A every cycle; per-team wave counters fixed it
  (was 6-0 sweeps, now 4-2 with piles comparable). Also mirrored the fish runs.
- [x] Render mode in main.rs (`battle`): runs the sim ~2 min, prints the war report, stages
  camps/Great Trees (canopy scales with hp), Heal Trees + aura rings, prey camps, herb
  patches, warriors with crests/collars/carries. Verified headless via llvmpipe GL → PNG.
- [x] `new_draft(picks_a, picks_b)` — engine-side draft API for a future pick screen
- [x] The deep roster (6 → 10): **Faryowl** the serval — the first *ranged* ancestor, yowl-bolts
  at 11 reach with kiting AI and the Far Song distance bonus; **Oldroar** the cave lion
  (Panthera spelaea) — the hulk, Deep Wounds Deep Fury scaling damage with missing hp,
  Boulder Paw knockback, Earthshaker; **Twofang** the Smilodon — The Saber Finishes vs
  wounded targets and a Killing Bite execute scaling with missing strength; **Suneye** the
  desert sun-priest — The Sun's Favor (the day-side mirror of Night's Hunger), Sunspear at
  12 reach, and scorched-ground Zones whose light sears stealth (the built-in counter to
  the veil kits). Egypt/Ra fantasy kept wild-born: sun-priests, no temples, no two-legs.
- [x] Three new verbs in the sim: Knockback (fling + hurt), Execute (missing-hp scaling),
  Zone (persistent burn that strips stealth; rendered as glowing ground in `battle`).
  Rule kept deliberately: *no cat ever yowled down a tree* — attack reach applies to
  warriors and prey only, structures must be gnawed at the claw (tested by that name).
- [x] Default rosters now rotate through the 10-pool from the player's pick, so every
  choice faces a different five. Report: 10/10 headless matches end by felling
  (491–658s, no timeouts), outcomes 5–5.
- [x] The control pair (roster 10 → 12), filling the two missing CC classes — before this,
  all CC/AoE was ult-locked (55-62s): **Snowhush** the deep-snow lynx owns *hard CC* —
  Snowbind and The Long Winter (Root verb: can claw, cannot move; fear cannot override it),
  White Silence, and Hush of Snows chilling on every ordinary blow; **Riptail** the
  drowned-forest jaguar owns *pulls* — Undertow and The River Takes drag scattered
  enemies to the claw (never inside the caster), The River Remembers biting +20% into
  the slowed/rooted/feared. Controller identity enforced by test: their techniques
  cycle ≤10s, faster than any ultimate. Root/slow timers join the determinism digest.
- [x] The jungle layer (`junglemap.rs`) — the map's natural blockers, every feature mirrored
  under a half-turn by construction: **thickets** (brush that conceals a cat from outside
  beyond 6m — one-way ambush sight, the hidden cat sees out; two cats in the same brush see
  each other), **stone walls** with a carved pass (sight never crosses the solid spans;
  crossing off-pass is a 0.45x scramble), **mistfalls** (drifting concealment near water).
  Wired into the sim: nearest_enemy respects sight, prey isn't spooked by hunters it cannot
  see (brush-hunting is now genuinely better), the ambush kits' Night's Hunger feeds in
  cover, walls slow underfoot. Kept pure and battleground-local — world's global terrain
  untouched, so parallel tests and other modes are unaffected. The layout test
  `the_trails_themselves_stay_open` caught two real wall placements crossing/grazing lanes.
- [x] Land adjustment: the battleground bakes **craggier, drier country** — mountain_bias
  1.6 (the per-mode pool knob prowl already rolls) paired with a new `world::set_land_lift`
  (+1.3m). Lift is additive with default 0.0; choosing a pool resets it, because a global
  lift leaking into the next mode's bake broke two sim/prowl water tests exactly as
  predicted — the reset is the contract, learned the measured way.
- [x] Battle render fullness: bushes/rocks/logs/litter added to the scene, thicket stands
  as dense dark brush, wall slab lines with their pass, mistfall columns + spray domes.
- [x] **Stillsmoke, the ninth son of the storm-dragon** (roster 13) — the dragon-cat, from
  the wild retelling of far-east lore (Suanni, the dragon's cat-shaped son who loved smoke
  and stillness; the seated-colossus image). The open niche he takes: **storm support /
  siege defense** — the roster's only mender of wood. New Rain verb: Sweetrain zones heal
  packmates AND drink into Heal Trees / the Great Tree (enemies in the same rain gain
  nothing, by test). Passive The Seated Mountain: stillness gathers up to +50% strike,
  spent the moment he moves (per-tick seat tracking). Cloud Breath slow, The Ninth Thunder
  fear ult. Ranged breath (8 reach, thin pelt) — ranged-kit count test now 3.
- [x] The river re-routed off the camp line (Smite-conquest shape): `world::set_river_course`
  joins the terrain-pool opt-ins (pool selection resets it, same contract as lift). The
  battleground's `RIVER_U_EAST`: a U-shaped bend bowed around the solo lane's territory,
  centred on the **anti-diagonal** — the map's second clan-swapping symmetry, which keeps
  the clans equal while letting the lanes differ (river solo side, dry duo side). Two
  paw-deep **fords** (gravel bars, rendered) where the east trail crosses; fishing runs
  moved to the U's two mirrored arms. The centre-lake bowl in raw_height turned out to be
  meander-course geometry and moves with the course; the camp line is dry **by landform**
  — a raised causeway, "the old high trail" — not by luck of the noise (a −4.5m pocket
  proved lift alone can't guarantee it). Locked-terrain test asserts dry camp line, wet U,
  dry fords.
- [x] Biomes classify on the land's *shape* (lift-corrected height), shoreline on the true
  waterline — without this, +1.2m of lift reclassified most of the map as bare mountain
  stone (two renders' worth of moonscape to find it).
- [x] Walls carved by wind and water: stacked strata narrowing as they rise, grey/rust
  banding, hoodoo spires with capstones off the crest, the pass a worn notch with
  shoulder stones — roughly double the height, true sight blockers in silhouette.
- [x] The tilted country: the U course now also raises a **one-sided slope** along the
  anti-diagonal — the duo lane climbs ~5.5m to a mountain shoulder (naturally prey-thin
  MOUNTAINS biome), mid holds the causeway, the river keeps the valley floor. One-sided
  because a full-plane tilt would drown the solo side's prey camps; anti-diagonal so the
  clan-swap reflection fixes it (lanes differ in height, clans do not).
- [x] High ground is a real sense, not scenery: looking DOWN a slope spots up to 40%
  farther (pure `downhill_bonus`, capped), and from 4m+ above within 30m a watcher sees
  down INTO brush and mist — cover is a roof, not a dome; stone still blocks absolutely.
- [x] The **Skyrock**: a claimable lookout spire on the ridge crest, the reflection's own
  fixed point (equidistant from both camps, asserted). Holding it (presence, like the
  contention ground) widens the whole clan's spotting range by a quarter — the reason to
  climb a lane with no freshkill. Rendered as spire + watchers' ledge + holder-coloured
  beacon.
- [x] Report skew resolved, and it was not variance after all. `world::set_terrain_seed` +
  `Battleground::new_on` let the harness re-roll the land; `examples/battleground_balance.rs`
  plays random drafts, both sides of every draft, across several rolls (140 games).
  - First version scored each kit leading its *default* draft — worthless, because the
    default fills teammates and opponents by pool order, so a kit's record mostly reported
    which trio it faced (Twofang 88% while facing the bottom three). Random drafts with
    presence-based scoring collapsed the spread from 12-88% to 33-59%.
  - `examples/mirror_probe.rs`: identical drafts both sides on folded terrain, asserting the
    match stays a perfect mirror image. It found four real asymmetries (below) and now holds
    until first contact, which is irreducibly symmetry-breaking.
- [x] **The map mixed two symmetry conventions.** River/causeway/tilt were anti-diagonal
  (mirror, each lane fixed); jungle/prey/herbs were half-turn (swaps duo and solo lanes).
  Each fair alone, fair under neither together. Unified on the mirror via
  `junglemap::reflect`; prey camps and herb spots are now base-plus-generated-reflection
  rather than typed pairs that can drift.
- [x] **Terrain noise was never symmetric** — cosmetic until downhill spotting and the
  Skyrock made height grant advantage. The battleground folds its land onto its own
  reflection: `height_at(x,z) == height_at(-z,-x)` is now an identity. Other modes keep
  their asymmetric wild land.
- [x] **Spawns, camps, medics reflected rather than copied/re-searched** (both clans fanned
  along +x: a 2.8m mirror break at tick zero; `land_near`'s one-sided slope probe settles
  differently on mirrored ground).
- [x] **Per-index randomness** — the economy peel-off roll hashed on the index into
  `warriors` (A = 0..2, B = 3..5), so the clans made different farming decisions from
  identical states; now hashed on a per-clan `seat`. Same class: `is_player` gated the AI
  economy detour even with nobody driving, so clan A's seat 0 never farmed.
  Net: clan A 37% -> 46% of 140 games.
- [x] Wounded warriors retreat to the nearest living **Heal Tree** instead of the far camp —
  what those trees are for. Found by diagnostic tally (`examples/battleground_diag.rs`):
  Oldroar spent 79s retreating, 7s healing, 67s dead and *zero* seconds on the economy.
  Took the Ice Age hulk from 31% to 48%.
- [x] `siege` kit trait (data-driven, budget-costed): heavy kits bite wood harder. Honest
  negative result — it moved Oldroar 33% -> 31% on its own; the retreat loop was the cure.
  Kept because the archetype should have the job, and it is tested.
- [x] Laning is doctrine now, not draft order: `ancestor::sociability` scores how much of a
  kit is spent on other cats (heal/rain +3, shield +2, control +1, **stealth -3** — a veil
  shared is a veil wasted) plus +2 for a thin pelt (under 150 hp needs a bodyguard), and
  the least sociable of a clan's three walks the river alone. Plus a duo doctrine: only one
  lane-mate peels off to the economy at a time, because a lane held by nobody is given away.
- [x] Settled by A/B on **identical drafts and lands** (`LanePolicy::Seat` vs `Kit`, 160
  games each) rather than by comparing two runs that differed in more than one thing:
  kit laning is decisively flatter — **sd 4.2 against 6.9**, range 18 against 24. The
  intermediate result that looked like a regression (spread 36-67) was two non-identical
  runs being compared, exactly the noise-chasing this switch exists to prevent.
- [x] The frailty term was itself found by measurement: sorting on ally-facing tools alone
  sent the 130-hp yowler down the river and cost it 19 points (55% -> 36%) in 168 games.
- [x] Faryowl's 61% traced to a mechanism, not a number: the kite in the Fight branch let a
  ranged cat give ground *and* land its blow in the same tick, so at speed 6.4 it outran
  every kit slower than itself while still hitting — free damage against half the roster.
  Backing away and biting are not the same motion, so giving ground now costs the strike.
- [x] Three arms measured on identical drafts and lands (160 games each), because the first
  fix overshot: kite free -> Faryowl 61%, sd 4.2, 1 kit outside +-7; kite **silent** ->
  Faryowl 42% and Stillsmoke 37%, sd 6.2, 3 outside; kite at **half** -> Faryowl 53%,
  sd 4.3, 1 outside. Half keeps the trade real without deleting the archetype, and a yowl
  thrown over the shoulder being a weaker yowl is the version that was true anyway.
  Abilities still land at full while kiting — spacing is what a yowler's abilities are paid
  for; it was the free auto-attack that was wrong.
- [x] Nightpelt's 62% was the same class of bug as Faryowl's kite, found by asking what the
  kit got for free rather than by tuning: **stealth did not break on striking**. A veiled
  cat was skipped by `nearest_enemy` entirely while still landing every blow — untargetable
  and undiminished. Now the blow ends the ambush. The bonus is banked first (`strike_of`
  reads the veil when the damage is computed), so a kit still gets its whole opening strike
  from the dark and then has to be somewhere; shields, heals and Veilwalker itself do not
  give a cat away.
  Result on the same 160 games: Nightpelt 62% -> 46%, and the **tightest roster yet** —
  sd 4.2, range 15, every kit within 8 points of even (was range 18, worst 12).
- [x] The balance harness now separates side bias from draft-set luck: it reports how many
  draft pairs split 1-1 (composition decided it) against how many one side took twice.
  Latest: 33 split, 18 to clan A twice, 29 to clan B twice — a 1.6 SD lean, i.e. not
  enough to act on, and the mirror probe's 51% over identical drafts remains the authority
  on side fairness. The harness's 43% is mostly orientation-decided pairs, which is chaos.
- [x] Shields honour their declared `dur` now — every kit's shield spec carried a duration
  the sim ignored, so guard-hair held until something broke it and a lion could raise Mane
  of Dawn once and wear it all match. Third free lunch of the week after the kite and the
  veil, found the same way: asking what a kit gets without paying.
- [x] **A correction to how the last few passes were argued.** Two independent draft sets
  (same code, same 160-game shape, different draft seed) disagree wildly per kit: Riptail
  66% then 39%, Suneye 55% then 40%, Faryowl 47% then 64%. At n≈70-90 games a kit's
  standard error is ±6, and *which drafts it lands in* swamps that again. So the per-kit
  table cannot resolve a kit-sized effect on one arm, and the rate improvements quoted for
  the kite and veil fixes were weaker evidence than they read as. Those changes stand on
  their mechanism — a cat that backs away while biting, a cat that strikes while
  untargetable — which is why it was worth finding mechanisms rather than tuning numbers.
- [x] Proof by counter-example, kept because it is the clearest one: *lengthening* the
  shield durations to compensate for the nerf measured the buffed kit eleven points WORSE
  (Goldenmane 49 -> 38). No buff does that; noise does. The durations went back to their
  authored values and the lesson went into the harness instead — it now prints ± standard
  error per kit and flags only rates outside two of them, and takes a draft-set seed so
  independent sets can be pooled.
- [x] Protocol written down and *run*: `docs/BALANCE.md` holds the pooled baseline — five
  independent draft sets, **800 matches, 4,800 kit appearances**, ±2se ≈ ±5 points.
  Result: **sd 3.3, range 12, ten of thirteen kits statistically even.** The three outside
  two standard errors are Windclaw 55%, Rainspots 55%, Snowhush 44% — the weakest kind of
  real, and each worth a mechanism hunt rather than a number nudge.
- [x] The 800-game run is affordable because the jungle stopped reallocating: `thickets()`,
  `walls()` and `mists()` rebuilt three Vecs on every call, and their callers are
  `concealment_at` and `sees`, which run once per warrior pair per tick. Cached behind a
  OnceLock with slice accessors for the hot path: **12,209 -> 27,883 sim steps/s (2.3x)**,
  and the pooled protocol went from nineteen minutes to eight.
- [x] Mechanism hunt done, and all three hypotheses were right (`docs/BALANCE.md` has the
  before/after): Windclaw stacked Skyward Hunger off **prey** because the prey rework
  credited mice as kills (55% -> 49%); Riverborn mended with no cost, cooldown or cap so
  Rainspots could outlast anything that came to the water, and now mends only between
  fights (55% -> 53%); Snowhush's chill was value only a packmate could spend, so the lynx
  collects on it itself now, +25% into the slowed or rooted (44% -> 48%).
- [x] Found while testing those: `Mind::AtMedic` healed **unconditionally**, so anything
  that put a warrior in that state healed it wherever it stood. `think` never does that
  from far away, so it never showed in play — two earlier tests were written around the
  quirk before it was understood as a bug. Now the den heals only at the den.
- [x] That hypothesis — spread dominated by AI quality rather than kit power — was testable
  and true. The harness already measured it: only **44%** of draft pairs were decided by
  the composition rather than by which orientation the chaos favoured. Warriors picked the
  nearest visible enemy, so three cats chewed three healthy targets and killed nobody.
- [x] Focus fire: a warrior now prefers the enemy closest to falling, trading up to eight
  lengths for it. Composition-decided pairs **44% -> 54%** over 800 games. Dispersion rose
  (3.3 -> 4.5) and that is the point: the old flatness was partly the AI hiding the roster,
  and every kit number taken before this was taken through a duller instrument.
- [ ] Re-run the mechanism hunt under focus fire — Duskstripe -6 and Oldroar at 41% are the
  new candidates, but they are one arm-set old and the instrument just changed
- [x] Odds doctrine: a warrior breaks off when outmatched, not only when wounded — counts
  what it can *see* within 16 lengths, margin of two (one flaps as packmates cross the
  radius), veiled cats uncounted. **The flattest roster yet: sd 4.5 -> 2.9, range 17 -> 10,
  kits outside 2se 3 -> 1**, while composition-determinism *held* at 54% — so unlike the
  old flatness this is not the AI hiding the roster, it is the roster being closer together
  when played competently. Biggest gainers are the kits that were dying for nothing:
  Oldroar +7, Snowhush +8, Riptail +6.
- [x] Collapse-on-breach: healthy warriors abandon an intact trail for the lane whose Heal
  Tree fell (before, only patrol spawns and respawns funnelled there, so a clan could break
  a border and keep walking past the hole). Correct play, and it **measured worse** —
  dispersion 2.9 -> 4.2, kits outside 2se 1 -> 2, composition-determinism 54% -> 52%. Lives
  behind `collapse_on_breach`, default off. Suspected cause is in the rule: *every* healthy
  warrior goes, so the clan trades one broken border for two empty lanes.
- [x] Holder-push tested: everyone collapses *except* the lowest seat on each lane. The
  abandonment diagnosis was right — composition-determinism back to 54%, dispersion 4.2 ->
  3.5 — but pushing still does not beat not pushing (sd 2.9). Default stays off.
- [x] New metric, because both existing ones measure the roster and none measured the AI:
  **does the clan that breaks the first border win?** Pooled answer **43% over 320 games** —
  taking the first objective was slightly a *liability*.
- [x] First explanation found and fixed: the Great Tree's root guard was all-or-nothing, so
  two standing Heal Trees hardened it exactly as much as three and a felled border bought
  nothing. It scales now, a third of the guard per border. Tested.
- [x] Instrumented it (`examples/breach_probe.rs`) and the overextension theory was **wrong**:
  deaths after a break were even (21 v 25) and at identical depth (103m from each cat's own
  camp), and the breacher was *ahead* on freshkill at the moment of the break in 17/29.
  What the probe found instead: the breacher ended with fewer of its **own** borders
  standing (-0.28), because `breach_lane` funnelled *every* patrol wave into the hole while
  the defender kept covering three trails. Half the waves press the breach now; mechanism
  metric -0.28 -> -0.07, probe conversion 34% -> 41%.
- [x] Tried paying the attacker (`spoils`, default off — a design switch, not a bug fix): a
  felled border spills 12 freshkill. It **did not help**: breacher wins 41% -> 34%, and the
  reward evaporates — a +14.3 pile lead at the break is +0.7 by match end while the
  defender out-farms it back. The attacker's problem is not that borders pay too little.
- [x] Probed it (`examples/economy_probe.rs`) and the premise was half wrong: an *earned*
  lead does compound (slope 1.21 over two minutes); it was the *granted* spoils lead that
  regressed away. What was actually broken: `GROWTH_HALF` was 16 while real piles hover
  around **three**, so the whole interesting range of the growth curve sat above anything a
  match reached and a clan's entire food advantage bought under 3% damage (1.138 v 1.108).
  Half-point is 6 now, with a test that a pile of nine beats a pile of three by more than a
  tenth of a multiplier.
- [ ] The deeper finding, unresolved and more interesting than the constant: **the economy
  self-equalises.** The trailing clan hunts 1.9x as much in every run, because the leading
  clan is busy fighting — an emergent rubber band nobody designed. Composition-determinism
  did not move (53%) and the leader/trailer growth gap barely did (0.030 -> 0.028), because
  leads stay small no matter how the curve is shaped. Whether a clan war *wants* an economy
  that refuses to let anyone pull ahead is a design question.
- [ ] Conversion still isn't fixed: **39.8% pooled over 793 games (se 1.8)**, five standard
  errors below even. Two explanations eliminated, one repaired, and defending is still
  stronger than attacking. The remaining candidates are structural rather than buggy — the
  defender fights beside its medic den, inside its own auras, respawns closer, and has the
  last-stand rule. Deciding whether attacking should pay (and what with) is a design call,
  not a bug hunt.
- [ ] Remaining levers: hold an ult for a structure push; call a packmate *to* a fight worth
  winning rather than only fleeing a lost one
- [x] Chased the pooled 46.9% clan-A rate to ground with `examples/side_probe.rs` rebuilt as
  a real instrument: 40 **mirror matches** (identical drafts both sides, folded terrain, so
  every remaining asymmetry is in the sim). Result **A 20 / B 19 / 1 undecided — 51%**.
  There is no side bias in the simulation. The harness number was partly draw accounting
  (draws sat in the denominator but never in clan A's column) and the rest is sample noise
  across differing draft sets; the balance harness now reports decided games only.
- [x] The probe's own first-blood metric caught a lesson worth keeping: scanning for "the
  first wounded warrior" reported a 78/22 skew that was **pure artefact** — clan A holds
  the low indices and wins every simultaneous tie in the iteration order. Counting both
  clans' wounds in the first tick any appear gives A 9, simultaneous 23, B 8. An
  instrument that shares the thing it measures' index order will measure its own index
  order.
- [x] Faryowl's 61% traced to a mechanism, not a number: the kite in the Fight branch let a
  ranged cat give ground *and* land its blow in the same tick, so at speed 6.4 it outran
  every kit slower than itself while still hitting — free damage against half the roster.
  Backing away and biting are not the same motion, so giving ground now costs the strike.
- [x] Three arms measured on identical drafts and lands (160 games each), because the first
  fix overshot: kite free -> Faryowl 61%, sd 4.2, 1 kit outside +-7; kite **silent** ->
  Faryowl 42% and Stillsmoke 37%, sd 6.2, 3 outside; kite at **half** -> Faryowl 53%,
  sd 4.3, 1 outside. Half keeps the trade real without deleting the archetype, and a yowl
  thrown over the shoulder being a weaker yowl is the version that was true anyway.
  Abilities still land at full while kiting — spacing is what a yowler's abilities are paid
  for; it was the free auto-attack that was wrong.
- [x] Nightpelt's 62% was the same class of bug as Faryowl's kite, found by asking what the
  kit got for free rather than by tuning: **stealth did not break on striking**. A veiled
  cat was skipped by `nearest_enemy` entirely while still landing every blow — untargetable
  and undiminished. Now the blow ends the ambush. The bonus is banked first (`strike_of`
  reads the veil when the damage is computed), so a kit still gets its whole opening strike
  from the dark and then has to be somewhere; shields, heals and Veilwalker itself do not
  give a cat away.
  Result on the same 160 games: Nightpelt 62% -> 46%, and the **tightest roster yet** —
  sd 4.2, range 15, every kit within 8 points of even (was range 18, worst 12).
- [x] The balance harness now separates side bias from draft-set luck: it reports how many
  draft pairs split 1-1 (composition decided it) against how many one side took twice.
  Latest: 33 split, 18 to clan A twice, 29 to clan B twice — a 1.6 SD lean, i.e. not
  enough to act on, and the mirror probe's 51% over identical drafts remains the authority
  on side fairness. The harness's 43% is mostly orientation-decided pairs, which is chaos.
- [x] Shields honour their declared `dur` now — every kit's shield spec carried a duration
  the sim ignored, so guard-hair held until something broke it and a lion could raise Mane
  of Dawn once and wear it all match. Third free lunch of the week after the kite and the
  veil, found the same way: asking what a kit gets without paying.
- [x] **A correction to how the last few passes were argued.** Two independent draft sets
  (same code, same 160-game shape, different draft seed) disagree wildly per kit: Riptail
  66% then 39%, Suneye 55% then 40%, Faryowl 47% then 64%. At n≈70-90 games a kit's
  standard error is ±6, and *which drafts it lands in* swamps that again. So the per-kit
  table cannot resolve a kit-sized effect on one arm, and the rate improvements quoted for
  the kite and veil fixes were weaker evidence than they read as. Those changes stand on
  their mechanism — a cat that backs away while biting, a cat that strikes while
  untargetable — which is why it was worth finding mechanisms rather than tuning numbers.
- [x] Proof by counter-example, kept because it is the clearest one: *lengthening* the
  shield durations to compensate for the nerf measured the buffed kit eleven points WORSE
  (Goldenmane 49 -> 38). No buff does that; noise does. The durations went back to their
  authored values and the lesson went into the harness instead — it now prints ± standard
  error per kit and flags only rates outside two of them, and takes a draft-set seed so
  independent sets can be pooled.
- [x] Protocol written down and *run*: `docs/BALANCE.md` holds the pooled baseline — five
  independent draft sets, **800 matches, 4,800 kit appearances**, ±2se ≈ ±5 points.
  Result: **sd 3.3, range 12, ten of thirteen kits statistically even.** The three outside
  two standard errors are Windclaw 55%, Rainspots 55%, Snowhush 44% — the weakest kind of
  real, and each worth a mechanism hunt rather than a number nudge.
- [x] The 800-game run is affordable because the jungle stopped reallocating: `thickets()`,
  `walls()` and `mists()` rebuilt three Vecs on every call, and their callers are
  `concealment_at` and `sees`, which run once per warrior pair per tick. Cached behind a
  OnceLock with slice accessors for the hot path: **12,209 -> 27,883 sim steps/s (2.3x)**,
  and the pooled protocol went from nineteen minutes to eight.
- [x] Mechanism hunt done, and all three hypotheses were right (`docs/BALANCE.md` has the
  before/after): Windclaw stacked Skyward Hunger off **prey** because the prey rework
  credited mice as kills (55% -> 49%); Riverborn mended with no cost, cooldown or cap so
  Rainspots could outlast anything that came to the water, and now mends only between
  fights (55% -> 53%); Snowhush's chill was value only a packmate could spend, so the lynx
  collects on it itself now, +25% into the slowed or rooted (44% -> 48%).
- [x] Found while testing those: `Mind::AtMedic` healed **unconditionally**, so anything
  that put a warrior in that state healed it wherever it stood. `think` never does that
  from far away, so it never showed in play — two earlier tests were written around the
  quirk before it was understood as a bug. Now the den heals only at the den.
- [x] That hypothesis — spread dominated by AI quality rather than kit power — was testable
  and true. The harness already measured it: only **44%** of draft pairs were decided by
  the composition rather than by which orientation the chaos favoured. Warriors picked the
  nearest visible enemy, so three cats chewed three healthy targets and killed nobody.
- [x] Focus fire: a warrior now prefers the enemy closest to falling, trading up to eight
  lengths for it. Composition-decided pairs **44% -> 54%** over 800 games. Dispersion rose
  (3.3 -> 4.5) and that is the point: the old flatness was partly the AI hiding the roster,
  and every kit number taken before this was taken through a duller instrument.
- [ ] Re-run the mechanism hunt under focus fire — Duskstripe -6 and Oldroar at 41% are the
  new candidates, but they are one arm-set old and the instrument just changed
- [x] Odds doctrine: a warrior breaks off when outmatched, not only when wounded — counts
  what it can *see* within 16 lengths, margin of two (one flaps as packmates cross the
  radius), veiled cats uncounted. **The flattest roster yet: sd 4.5 -> 2.9, range 17 -> 10,
  kits outside 2se 3 -> 1**, while composition-determinism *held* at 54% — so unlike the
  old flatness this is not the AI hiding the roster, it is the roster being closer together
  when played competently. Biggest gainers are the kits that were dying for nothing:
  Oldroar +7, Snowhush +8, Riptail +6.
- [x] Collapse-on-breach: healthy warriors abandon an intact trail for the lane whose Heal
  Tree fell (before, only patrol spawns and respawns funnelled there, so a clan could break
  a border and keep walking past the hole). Correct play, and it **measured worse** —
  dispersion 2.9 -> 4.2, kits outside 2se 1 -> 2, composition-determinism 54% -> 52%. Lives
  behind `collapse_on_breach`, default off. Suspected cause is in the rule: *every* healthy
  warrior goes, so the clan trades one broken border for two empty lanes.
- [x] Holder-push tested: everyone collapses *except* the lowest seat on each lane. The
  abandonment diagnosis was right — composition-determinism back to 54%, dispersion 4.2 ->
  3.5 — but pushing still does not beat not pushing (sd 2.9). Default stays off.
- [x] New metric, because both existing ones measure the roster and none measured the AI:
  **does the clan that breaks the first border win?** Pooled answer **43% over 320 games** —
  taking the first objective was slightly a *liability*.
- [x] First explanation found and fixed: the Great Tree's root guard was all-or-nothing, so
  two standing Heal Trees hardened it exactly as much as three and a felled border bought
  nothing. It scales now, a third of the guard per border. Tested.
- [x] Instrumented it (`examples/breach_probe.rs`) and the overextension theory was **wrong**:
  deaths after a break were even (21 v 25) and at identical depth (103m from each cat's own
  camp), and the breacher was *ahead* on freshkill at the moment of the break in 17/29.
  What the probe found instead: the breacher ended with fewer of its **own** borders
  standing (-0.28), because `breach_lane` funnelled *every* patrol wave into the hole while
  the defender kept covering three trails. Half the waves press the breach now; mechanism
  metric -0.28 -> -0.07, probe conversion 34% -> 41%.
- [ ] Remaining levers: hold an ult for a structure push; call a packmate *to* a fight worth
  winning rather than only fleeing a lost one
- [x] `--play` wiring: the `app` feature's rustc >= 1.76 requirement (emath 0.28) was the
  blocker in earlier sessions, pinned to a 1.75 sandbox toolchain. Resolved this session
  with a real rustc 1.96 — `cargo check --features app --bin clawbound-rs` compiles clean
  (confirmed twice this session: once verifying `Screen::Draft`'s wiring, once verifying
  the ancestor crest-colour change). Compiling is not the same as playtesting on a real
  display (this sandbox is still headless), but the glue this note used to say "can't be
  compiled or verified" now compiles — the remaining gap is a human at a screen, not a
  toolchain.
## Asset compression (the parts-web pipeline, from blackstar-rs)
- [x] Measured first, per `blackstar-rs/docs/COMPRESSION.md` doctrine. 17 IQMs = 19.0 MB;
  `iqm.rs` reads positions + indices only (10.7 MB) and throws away texcoords/normals on
  every load — 44% of the drop is shipped to be discarded.
- [x] Two of the described techniques do NOT pay here, and the negative results are worth
  keeping: cross-model topology dedup finds **0 of 17** (no two models share an index
  array) and float-exact vertex welding finds **0 of 413,307** (already welded). Lattice
  welding only starts paying at 12 bits (0.2%). Same shape as blackstar's own finding that
  dedup found nothing on fold strips.
- [x] What pays, cumulative, one zstd -19 pass: 14-bit quantisation per model box (8.2 MB,
  worst error 0.0031% of span) + Morton reorder (worth 17%: 3.86 -> 3.20 MB) + planar
  delta streams + varints -> **2.81 MB, 6.8x**. Planar positions beat BSWEB's existing
  fixed-width layout by 21% (3.63 -> 2.85 MB at equal quantisation).
- [x] `games/shared/bsweb` gains its missing encoder half (`pack.rs`, 15 tests) plus two
  additive reader changes: `pmode` (0 = the original fixed AoS, the default when absent,
  so every previously built web still reads) and generalised bit widths
  (`maxq = (1<<bits)-1`, identical for the 8 and 16 that existed). blackstar benefits too.
- [x] `assetpack.rs` + `examples/pack_animals.rs` + `examples/pack_verify.rs`; `Model::load`
  reads the pack with IQM fallback; real-asset integration test in `tests/`.
- [x] Load-time trap found and fixed by measurement: unzstd costs ~37 ms while decoding one
  model costs ~1.3 ms, so a per-call decompress would have made seventeen loads ~700 ms —
  *slower* than the IQMs it replaced. Decompress once into a `OnceLock`, decode per model.
- [x] Which artifact to ship, measured rather than assumed: the .zst saves 0.93 MB but an
  outer archive recompresses the raw web to within 79 bytes of it (2,811,201 vs
  2,811,122), so the container only earns its keep when files ship loose. Loader prefers
  the raw web; both are gitignored (committing a derived pack beside its source is exactly
  the trap FLEET_COMPRESSION.md records: a 904 MB bundle).
- [x] The loose IQMs are gone: `assets/` 19.0 MB -> 6.3 MB (the committed `animals.web` is
  3.7 MB, 2.97 MB as a git object). `Model::load` reads the pack as its normal path, the
  `.iqm` branch survives as the way to re-run the round-trip test against restored source,
  and the shipped-asset tests now assert against the pack. `docs/ASSET_ARCHIVE.md` records
  what went, the exact restore commands, and the `git filter-repo` invocation.
- [x] The raw web is committed and the `.zst` is gitignored, measured: as a git object the
  raw web costs 2.97 MB against the container's 2.81 MB (155 KB apart) while the container
  costs ~37 ms of ruzstd on first touch against ~4 ms. For a repo file read at startup that
  trade goes the other way.
- [ ] **History rewrite deliberately NOT done here.** `git filter-repo` renames every commit
  hash from the first that touched the IQMs, which would make this bundle unappliable to
  the base it was built on — a rewrite cannot be delivered as a patch. Run it locally when
  clone size matters, after archiving the originals outside git; commands in
  docs/ASSET_ARCHIVE.md.
- [ ] Decode speed: 1.3 ms/model (21.9 ms for all 17) against 5.6 ms for parsing the loose
  IQMs — 4x slower, invisible for statues, worth revisiting if a mode ever streams these
- [ ] The triangle stream could be ~8% smaller with per-triangle (b-a, c-a) deltas instead
  of one running cursor, but bsweb's decoder is a single cursor and changing it is a
  format break, not an additive one. Left as measured, not done.

- [ ] Landform-carved heightfield (walls/crags as real terrain relief rather than placed
  slabs) would need a per-mode heightfield overlay in world — a bigger seam, deferred
- [x] Crowd-control coordination in `auto_cast`: a controller saves its pull/root for a
  crowd, an area ult holds a few seconds for a gather that is coming, and the pulled carry
  a `pulled_t` mark so the follow-up can see the moment. Ults also now want a bigger crowd
  than techniques (3 vs 2) — without that the gather kits burned The River Takes on any
  two scouts. Behind `Battleground::wombo` so it can be ablated, not just believed.
- [x] Ablation says the coordination alone was worth **nothing**: 168 games each way, same
  drafts, same lands — clan A 48% off vs 49% on, every kit inside its own standard error
  (Riptail 52% -> 49%). Recorded rather than quietly kept.
- [x] The diagnosis was the real finding: ancestors were assigned `slot % LANES`, one per
  trail, so **the map had a duo lane with nobody duoing** and the coordination had no
  partner. Ancestors now lane like the country is shaped — seat 0 walks the river solo,
  seats 1-2 climb the mountain together, the middle is the patrols' with a weighted wave
  rotation. Roster spread went 37-65% -> **42-57%**, flatter than any kit tuning achieved.
- [x] Re-ran the wombo ablation with duos in place: still nothing (47% clan A either way,
  spread 43-58 off against 42-57 on), and Riptail — the kit it exists for — came out worse
  in *both* ablations (52 -> 49, then 56 -> 50). Two independent runs agreeing on the
  direction is weak but real evidence that holding an ability costs more than it buys in
  fights this short, so **`wombo` now defaults off**. The code, the switch and the tests
  stay: the behaviour wants an AI that can promise the follow-up rather than hope for it.
- [x] Playable *now*, without the blocked GUI stack: `examples/battleground_play.rs` — a
  terminal client with a top-down ASCII view (terrain, brush, stone, water, high ground,
  prey, herbs, zones, both clans, structures), a HUD (ability cooldowns by name, pile,
  herbs, Great Tree hp, contention and Skyrock holders), and one warrior under your paw.
  Interactive by line, or scripted (`-- lynx "wwwd1f" 60`) which is deterministic and
  diffable. Keystroke and script share one `key()` mapping, so the scripted path exercises
  exactly what a human's press does.
- [x] The `set_input` path had **no test at all** — everything in battleground.rs drove the
  AI. Now covered: the player moves where told and only where told (an idle player is idle,
  not auto-piloted), a root stops them, slots cast and pay cooldowns, an out-of-range slot
  clamps rather than panics, and the player's claw is the *same blow* an ancestor throws —
  target riders, on-hit effects (Hush of Snows fires from a human's paw) and kill credit,
  which it previously skipped.
## Sound: a hybrid soundscape that lives with the world

- [x] `soundscape.rs` — two halves, because ambience and information are different jobs.
  **Beds** are the AmbientSounds idea kept: layers chosen by *sampling* the world around the
  listener (canopy, water fraction, height, night, leaf-bare) on a ring rather than a single
  point — one sample under the paws says "in a puddle", nine say "at the river" — each
  crossfading so a treeline is a slope rather than a cut. Nothing is authored twice: the
  terrain already answers these questions.
- [x] **Nodes** are the half a MOBA needs and a block game does not: emitters that live at a
  place *and carry state*. A hunting camp murmurs while its prey is alive and goes quiet
  once taken, so a player learns to hear a camp somebody else has cleared; a Heal Tree hums
  while it stands and **creaks while it is being gnawed**, which is a warning arriving
  through the ears from a lane you cannot see. Nodes are not faded in — an emitter you are
  standing beside should be audible the moment you are beside it.
- [x] **Events** are one-shots that duck the beds and are not ducked themselves: a border
  falling, the Skyrock changing paws, night, a herb delivered, your own death, victory.
  They fire on the frame they happen and only then — the first version kept re-emitting for
  the length of the duck, which for a fire-and-forget backend is the same thunderclap
  played thirty-six times.
- [x] The mix is scaled if the looping layers sum past 1.0, so a backend is never handed a
  clipping frame; events are exempt, because a cue that matters should not be quietened by
  a busy field. Pan is computed against the listener's facing.
- [x] Pure and testable: it takes a world and a listener and returns `(cue, gain, pan)`. No
  audio device, no file paths — a missing asset is a silent cue rather than a crash.
  `examples/soundscape_walk.rs` walks the map printing the mix, then plays a match printing
  the cues as they fire.
- [ ] Backend: map `Cue::id()` to files and play them (the app's audio path). The one design
  note worth carrying over from AmbientSounds is per-sound `length`/`transition` for
  variation within a bed — several files per cue, chosen round-robin, so a loop does not
  become a tick.

## Graphics: camera, ability guides, casting

- [x] The camera came in: the `battle` render mode was framed two hundred units out on the
  whole territory, which is a map screenshot rather than a game. It now follows the player's
  warrior at ~30 units, and `castguide::camera_dist` gives the app and the menu one shared
  answer for what each zoom step means (0 closest 20m .. 4 widest 34m, default 24 — nearer
  than the 34 capture mode used).
- [x] Zooming in broke the ground markers, which is the part worth recording: the contention
  ground, the Heal Tree auras, the Skyrock and the camp pads were **filled translucent
  discs**, fine as gentle washes at two hundred units and opaque slabs across the whole
  frame at thirty. They are **rings** now — an outline says "this far" and still shows the
  ground it describes.
- [x] `castguide.rs`: placeholder ability guides, headless. Every kit's verb already implies
  a shape — a ring around you (Aoe/Fear/Shield/Heal), a placed disc (Zone/Rain), a travel
  line (Dash), a wedge (Root/Pull), a reticle (Strike/Execute) — and the aim is clamped into
  range, because a guide that shows a reach the ability does not have is a guide that lies.
  Cooling abilities are greyed rather than hidden: a range you cannot see is a range you
  cannot learn.
- [x] Tooltips assembled in one place (`castguide::tooltip`) so two screens cannot describe
  the same ability differently: name, what it does, then "range 13 · 2.2x claw · 1.8s ·
  cooldown 58s" — power in *claws*, the unit a player can feel. The ultimate's tooltip
  carries the kit's passive, where there is room for it.
- [x] Casting options: Quick (press to cast), Normal (press, aim, press, with a cancel that
  spends no cooldown, and a different slot re-aims rather than firing both), Instant (quick
  and no guide). Guides toggle **independently** of cast mode, because wanting a reticle and
  wanting to press once are two different preferences.
- [x] Menu rows for all three (`Ability guides`, `Casting`, `Camera`), with Back now found at
  the end of the list rather than hard-coded to row 6.
- [ ] App-side: the guides need drawing from `battleview` + `castguide` in `render_battle`,
  and the tooltip wants an egui panel. Same rustc >= 1.76 blocker; the shapes, the words and
  the state machine are done and tested, and the `battle` render mode is a working reference
  implementation of exactly this drawing.

## Quick play and the 5v5 select page

- [x] The battleground fields **up to five a side** (`Battleground::new_roster`, `MAX_CLAN`),
  and five is where the map's lanes finally make sense: a river solo, a mountain duo, a
  middle that is somebody's rather than nobody's, and a **roamer** with no lane at all.
  Rosters of one to five all build and run; the three-a-side constructors are thin wrappers,
  so every existing test and balance number still means what it meant.
- [x] `lobby.rs` — quick play and the select page, headless like `draft` and `battleview`.
  One `LobbyInput` set for three sets of hands, with a test that a **controller** (walk the
  grid, wrapping so a stick never finds a wall), a **keyboard** (type "still", land on
  Stillsmoke) and a **thumb** (tap a grid cell, layout supplied by `Lobby::grid` so the
  hit-test cannot disagree with the render) all reach the same cat and lock it.
- [x] The clock decides: 30s to pick, bots choosing with rising urgency so the column
  resolves before the buzzer rather than on it, anyone unlocked auto-picked from what they
  were hovering, then a 5s ready beat and launch. Everyone locking early skips the rest of
  the clock; nobody locking still starts a legal match.
- [x] `MenuCmd::QuickPlay` is the first row of the title screen — one press to a match — and
  the rows beneath it are asserted to still go where they went.
- [x] `examples/lobby_demo.rs` runs the page and the match it produces end to end.
- [ ] The app-side page itself (portraits, clan columns, the countdown ring, thumb targets
  sized for a phone) — same blocker as the render work below: it needs the `app` feature and
  rustc >= 1.76. The state machine it drives is done and tested, so this is layout.

## The battleground in the app: what is done, what is left

The mode is simulated, balanced, playable in a terminal and recorded/replayed —
and until this pass it could not be reached from the menu, which is the only door
most players use. The remaining work is now precisely bounded.

- [x] `menu.rs`: `GameMode::Battleground` exists, appears in `GAME_MODES`, and brings its own
  rows — an ancestor with its passive and three ability names, instead of hunter/class/clan,
  because those are choices the mode does not have. Left/right walks the thirteen.
  `MenuCmd::StartBattleground(Ancestor)` is its own command rather than being squeezed
  through `StartMode`'s three arguments, two of which it would ignore. Tested, including
  that the other four modes' rows and commands are unchanged.
- [x] `battleview.rs`: the pure view model — `marks()` (every drawable thing with its ground
  height, team, fill fraction, carried burden and whether it is *you*, culled to a radius),
  `hud()` (one consistent read of ability cooldowns by name, piles, herbs, Great Tree hp,
  borders standing, contention and Skyrock holders, respawn timer, outcome), and `focus()`
  (follow the driven warrior; fall back to the centre of the living clan rather than
  staring at a corpse). Tested, so what is left in `app.rs` is mapping rather than logic.
- [ ] `app.rs`, mirroring `render_capture` (~1001-1135) exactly:
  1. `battle: Option<Battleground>` beside `capture`/`prowl` on the app state, cleared in
     the other `MenuCmd` arms the same way they clear each other;
  2. a `MenuCmd::StartBattleground(a)` arm: `state.battle = Some(Battleground::new(a))`;
  3. `render_battle(dt)`: `keys.move_vec()` → `move_world(yaw, ..)` → `set_input`, with
     `keys.ability` mapped to slots 1/2/3 and left-click/dodge to the claw;
  4. `build_battle_entities()` from `battleview::marks` — the shapes already exist in
     `main.rs`'s `battle` mode (canopies scaled by `fill`, aura rings, crests, carries);
  5. an egui panel from `battleview::hud`, laid out like the capture HUD;
  6. `self.focus = v3(fx, height_at(fx,fz), fz)` from `battleview::focus`, `write_camera(34.0)`;
  7. the `render()` dispatch line: `if self.battle.is_some() { self.render_battle(dt); return; }`.
  **Unwritten because it cannot be compiled here**: the `app` feature needs rustc >= 1.76
  (egui 0.28 / emath) and this sandbox is pinned at 1.75. Writing renderer glue that has
  never been through a compiler is how a codebase acquires code nobody trusts.

  MERGE NOTE on the blocker above: the incoming series was written in a sandbox
  pinned to rustc 1.75, which is why the `app.rs` glue is specified but unwritten.
  THAT CONSTRAINT DOES NOT HOLD HERE — this tree builds on 1.96. The seven steps
  stay exactly as written because they are still the plan; only the reason for
  their being unwritten has expired, and someone can now actually compile them.

- [x] Player-pick draft UI in menu.rs — `Screen::Draft`, 6 pick slots (3 per
  clan) cycling through the 13-ancestor roster with no-duplicate cycling,
  `MenuCmd::StartBattleground([Ancestor;3], [Ancestor;3])`. Reachable from
  Title. `app.rs` wiring is intentionally the honest fallback (log the
  confirmed draft, reopen the menu) rather than a fake live match — the
  separate "--play wiring in app.rs" item below is what a real Battleground
  match needs, and isn't done yet.
- [x] `draft.rs` — the pick screen's logic, headless so the TTY client, a future menu and the
  tests share one set of rules: names resolve by warrior *or* species ("stillsmoke" or
  "suanni"), no duplicates within a clan, undo, autofill, and a deterministic counter-pick
  that scores each remaining ancestor against what you brought (reach or a gap-closer for
  thin pelts, the sun-priest's light for veils, speed against siege, burst against menders).
  Deterministic on purpose: an opponent you cannot predict is only unfair, and a rule you
  can read is a rule you can learn to beat.
- [x] `battleground_play --roster` prints the thirteen with their stats and passives;
  `--draft` runs the pick screen and starts the match against the counter-picked clan.
- [x] The counter excludes your own picks. The *rules* allow a mirror (side_probe needs it),
  but answering a sun-priest by bringing the same sun-priest reads as a shrug.
  MERGE NOTE: the draft exists TWICE after the split — `menu.rs`'s `Screen::Draft` above
  (this side, GUI-facing) and `draft.rs` here (incoming, headless + TTY). Both kept: the
  headless one is where the rules belong and the menu one is already wired to `app.rs`.
  Collapsing menu.rs onto draft.rs is the obvious follow-up and is not done.
- [x] Map fairness: the mid trail crosses the wide river basin (a genuine "river lane" —
  Rainspots country) — extend mapeval to grade lane water-fractions per clan side.
  `mapeval::lane_water_report(camp_a, camp_b, bends, samples)`: per lane, the water
  fraction of each clan's own home-side half (camp → bend) and their `asymmetry`.
  Generic over any 3-waypoint lane, no dependency on battleground's types (matches
  the module's existing "adapted, not coupled" posture). Wired into
  `examples/battleground_report.rs` as a one-time printout before the per-ancestor
  match loop. Empirical result on the current default map/seed: **0% asymmetry on
  all 3 lanes** (lane 2 has a small 3% water fraction, identical on both sides) —
  `RIVER_U_EAST`'s own doc comment ("the U on the solo-lane side; the camp line
  stays dry") already explains why: the river is placed to keep both camps' home
  approaches dry by construction, not routed through the mid lane the way this
  TODO's description assumed. The grading tool this item asked for now exists and
  runs cleanly; if a future terrain change (different river course, different seed)
  reintroduces a lopsided lane, this is what will catch it — nothing to fix on the
  current map by this metric. 3 tests.
- [x] `mapeval::lane_score` — the SAME question answered a second time, from the other
  side of the split, and kept because it answers more of it. Where `lane_water_report`
  grades water fairness per clan half, this profiles each trail for height, wet fraction,
  cover and climb along the walk the patrols actually take, and reports both
  **distinctness** (are the trails different from each other, or the country was shaped
  for nothing) and **clan_gap** (are they the same for both clans).
  `examples/lane_report.rs` prints it per land in about a second.
  Measured across three lands: clan gap **0.0000** every time (the reflection holds),
  distinctness 0.35-0.41, and the shape the terrain work aimed at is visible in the
  numbers — duo sits 3-5 units above solo, solo takes the water and the most climb.
  This is the standing guard that would have caught the two-symmetry-conventions bug in
  one call instead of a mirror probe and three commits, and it is a unit test now.
  MERGE NOTE: both live. They do not collide (`LaneWaterReport` vs `LaneProfile`/
  `LaneScore`) and they agree on the finding that matters — the map is symmetric — which
  is a genuine independent confirmation rather than duplication to tidy away. If one
  should go it is a later decision with both sets of numbers in hand.
- [x] Ancestor crest colours per-kit in the battle render — `ancestor::crest_color`
  (13 distinct hex colours, one per legend, tested for uniqueness), wired into
  `build_battle`'s ancestor-crest branch in place of the team-colour fallback
  that was there before.

- [x] `replay.rs` — a match is its setup plus the keys pressed, so a replay is a header and
  the ticks where input *changed* (780 ticks recorded as 11 events). `--record FILE` writes
  one, `--replay FILE` re-runs it and compares digests. Three things this buys: a bug report
  that runs, whole-match regression fixtures nobody had to hand-build
  (`tests/replays/*.replay`, checked by `tests/replay_regression.rs`), and an honest check
  on the determinism every other part of this crate assumes — including any future netcode.
- [x] The tamper test is asserted where it is *true* rather than where it reads well: a
  different land, enemy clan or laning doctrine all fail to reproduce a recording, but
  flipping `wombo` on a ten-second recording reproduces it exactly, because the clans have
  not met yet. That assertion says so.

- [x] `lockstep.rs` — the deterministic-lockstep **protocol** for two-clan matches, sitting on
  the determinism `replay.rs` proves every test run: peers exchange inputs (a frame each per
  tick, a digest each second) rather than state, both run the same sim, nobody is the
  server. Input delay of 6 ticks (200ms), a tick advances only when both sides' inputs are
  in, digests compared on arrival, and a quiet peer stalls then loses the match rather than
  hanging forever. Protocol-then-transport: it knows nothing about sockets or
  `shared/netcode`, and the tests run a 600-tick match between two in-process peers with no
  port opened.
- [x] Writing it caught a genuine protocol bug of the kind that only shows in a real
  exchange: comparing digests at the moment of *hashing* catches nothing, because the other
  peer's claim for that tick is still in flight. Whichever half arrives second does the
  comparing now, and the desync test proves it by nudging one warrior half a length in one
  peer's world — which is exactly what a real desync looks like from the inside.
- [x] Transport: `wire.rs` — a hand-written codec (an input is **8 bytes**, a digest check 13,
  so a tick of two players costs 16 bytes of payload) plus `UdpLink`, a deliberately dumb
  non-blocking UDP carrier. Movement travels as two i8 axes rather than f32: the sim
  normalises the vector anyway, it is a quarter of the bytes, and two peers that agree on
  the *bytes* agree on the input with no float parsing in between. Tested end to end — two
  peers, two real loopback sockets, 240 ticks, same digest.
- [x] No hard dependency on `shared/netcode`. That crate's job ends with two peers holding
  each other's socket address, which is exactly what `UdpLink::connect` wants; keeping the
  seam there means a LAN match between two handsets on the same wifi does not have to pay
  for discovery it does not need. Match setup can still be agreed by exchanging a
  `replay::Replay` header, which already carries draft, seed and every doctrine switch.
- [ ] What a deployment needs on top, and where it belongs: resend-until-acked for the last
  few input frames. A lost packet only *stalls* lockstep (a tick will not run without both
  inputs), so this is a latency fix rather than a correctness one, and it is a transport
  concern — it goes in `wire.rs`, not in the protocol.
- [x] Both clans' leaders are drivable now (`set_input_for(side, ..)`, `driven(side)`), so
  lockstep applies both frames instead of carrying one it could not spend. A peer that drove
  only its own leader and left the other clan's to the AI would be running a different match
  from the one across the link — the classic move-one desync — so both peers apply both.
  `set_input` still means clan A, and an undriven clan still thinks for itself (tested:
  driving one side must not switch the other side's AI off).
- [x] The recorded-match regression passed unchanged through that refactor, which is exactly
  what the fixture is for: clan B's seat 0 became `is_player` but nobody drives it, so the
  match must play identically — and it does, to the digest.

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
