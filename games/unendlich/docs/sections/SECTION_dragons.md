# SECTION — Dragons

## 1. What this section is
Dragons are unendlich's apex creatures: a single heritable **genome** drives a dragon's
body, growth, voice, breath weapon, and behaviour, so every dragon is a unique,
*readable* expression of its genes (and, once tamed, of its owner). The player-facing
goal is dragons that look, sound, fly, fight, and bond believably — you can hear a
dragon's lineage, watch it ride a thermal, tame one over time, and recover legendary
artifacts that dragons themselves create. This section covers the
genome→body→voice→behaviour pipeline, what the brain composes today, and the gaps:
full deterministic **voice** (+ owner-voice mesh + audio backend), taming/owner-bond,
breath-weapon wiring, dragon artifacts/legendary loot, and flight polish.

## 2. Already built (read from the tree)
Body / genome → model:
- `sim/dragon_blueprint_from_genome.rs` — `BodyGenes` (7 normalised BODY genes) →
  `[BlueprintPart; 8]` (`PartRole`, dimensions). The semantic body layout.
- `sim/dragon_forge.rs` — the canonical procedural model: `build(g:&Genome,res)->ForgeModel`
  (CatmullRom spine, legs, wings), `ForgeBox`, `WingPose{Spread,Folded,Beating,Flared,Aggressive}`,
  `skin`/`skin_with_flex` (animate boxes by joint), `is_leg_joint`. Ported from the Three.js viewer.
- `sim/dragon_growth_stage.rs` — `dragon_growth_stage(age_ticks, MaturationGenes)->GrowthState`
  (`GrowthStage` Egg/Hatchling/Juvenile/Adult, size/stat scales; `EGG_TICKS`… constants).
- `sim/egg_genome.rs` — egg `Genome`; `sim/dragon_archetype.rs` — archetype `Genome`;
  `genetics.rs::Genome` (8-gene readout: colour RGB, height, eyesight, hearing, speed, turn rate).
- `sim/mob_limb_girth.rs` — `leg_girth_ratio`/`exceeds_girth` guard against over-thick animated legs.

Behaviour brain (composition layer):
- `sim/dragon_brain.rs` — `think(&mut BrainState,&BrainSenses,dt)->BrainOut` composes the cores
  below into one per-tick decision: `BrainOut{ mode, flight_phase, wing_pose, activity,
  move_heading, move_speed01, wants_sprint, seek_water, flap01, altitude }`. **No breath output yet.**
- `sim/dragon_activity.rs` — grounded loop `Activity{Idle,Forage,Feeding,Sleeping,Alert}` from
  vitality/fatigue/day phase/threat/heard.
- `sim/dragon_mode.rs` — `decide(...)->ModeStep` ground vs air with dwell hold; builds the `AirIntent`.
- `sim/dragon_air.rs` — airborne FSM `FlightPhase{Grounded,TakingOff,Climbing,Cruising,Soaring,
  Gliding,Diving,Landing}`, `step(...)`, `params(g)`; stamina-aware.
- `sim/dragon_flight.rs` — aero per tick: `FlightMode{Stalled,Gliding,Level,Climbing}`,
  `flight_step`, `stall_speed`, `wing_area(g)`, `flap_stamina_cost`, `GRAVITY`.
- `sim/dragon_thirst.rs` — `Hydration` vital (`tick`/`drink`/`is_thirsty`/`is_parched`),
  `drink_urge`, `prefers_water_over_food` (spec: river_systems §Dragon Thirst).
- `sim/dragon_locomotion.rs`, `sim/dragon_hunt.rs` — ground gait + hunt (composed by activity/brain).

Voice / signalling (partial):
- `sim/sound_genome.rs` — `voice_from(genome:[u8;6])->Voice`; `mimic(target,skill)` (Mimic horror).
  A **basic** heritable voice — far short of the dragon_voice spec (see §3).
- `sim/dragon_language.rs` — symbol grammar `encode(intent,mood)->u16`, `decode`, `similar`;
  pairs with sound_genome. `MOOD_BUCKETS=16`.

Breath weapons (mostly built):
- `sim/breath_attack.rs` — `BreathParams`, `params_from(g)`, `dominant_element(g)->u8`,
  `damage_at(angle,distance)`, `recharge`, `can_breathe`/`try_breathe` (fuel).
- `sim/breath_cone.rs`, `breath_damage.rs`, `breath_falloff.rs`, `breath_recharge.rs`,
  `breathe.rs` (chest swell), plus batch variants `batch_breath_{falloff,damage}.rs`, `batch_breathe.rs`.
- `sim/dragon_elemental.rs` — `choose_element(RangeBand,cover,wetness01)->Element` situation policy.

Taming / artifacts (seeds):
- `sim/taming.rs` — `progress(cur,affinity,feed_quality)->f32` taming meter (0 wild .. 1 tamed). No bond state.
- `sim/artifact_imbue.rs` — imbue counter + `field_amplification` (built earlier); the substrate for
  bender/dragon-imbued artifacts (legendary_items spec).
- `sim/dragon_bending.rs`, `dragon_bending_ai.rs` — dragon use of bending disciplines.

## 3. The big sub-sections to build

### A. Dragon Voice — deterministic genome→acoustic
Scope: express the full voice model, not just `[u8;6]→Voice`. Derives from
**dragon_voice.docx §1–2**: F0 from mass×age (elder fundamentals subsonic), harmonic-series
shape + formant peaks + noise-layer character from elemental attunement, harmonic precision from
BLD allele expression, prosodic envelope from disposition alleles, environmental resonance coupling
(BLD-2). Seams: extends `sound_genome::Voice`, reads `dragon_growth_stage` (age/size) +
`breath_attack::dominant_element`/`dragon_elemental::Element` + the BLD/disposition genes (resonance
bending spec). Deps: a canonical dragon genome (see Open Questions). **Pure** (genome+state→acoustic
parameter struct).

### B. Owner Voice Mesh (tamed)
Scope: a tamed dragon's upper harmonics (8th–16th) drift toward the owner's transposed register
while low harmonics stay genetic and middle harmonics are the "mesh zone" (dragon_voice.docx §1,
"Owner voice residuals"). Accumulates from proximity over time. Seams: consumes Voice (A) + an owner
voice descriptor + a bond/proximity accumulator (D). **Pure** (deterministic blend; the residual
accumulator is a seeded/clocked-by-dt state, no wall-clock).

### C. Audio backend (synthesis + playback)
Scope: turn the acoustic parameter set (A/B) into actual sound — additive/subtractive synth from
F0+harmonic series+formants+noise layer+envelope, triggered by brain/language events. Seams: new
audio device/mixer in the bin; render/asset pipeline. Deps: A. **NOT pure** — bin/audio backend,
compile-on-end; biggest external dependency in the section.

### D. Taming & Owner Bond
Scope: promote `taming::progress` into a persistent bond (owner identity, trust/obedience, command
acceptance, feeding/affinity history) that gates owner-voice-mesh (B) and rideability. Derives from
the taming/owner-voice-mesh goal in the brief + dragon_voice "tamed" layer. Seams: `taming.rs`,
`dragon_brain` (bond modulates senses/obedience), bin (player commands). **Pure** core + bin wiring.

### E. Breath-weapon brain wiring
Scope: the brain currently emits no breath. Add a breath intent to the decision: compose
`dragon_elemental::choose_element` + `breath_attack::{params_from,damage_at,try_breathe}` + recharge
into a `BrainOut` breath decision (when/where/which element), respecting fuel and activity. Seams:
extend `dragon_brain::{BrainSenses,BrainOut}` + a small pure breath-intent module. VFX/projectile is
separate render/bin. **Pure** core (+ later render for the visible breath).

### F. Dragon Artifacts & Legendary Loot
Scope: data model + generation rules for the legendary_items_dragon_artifacts.docx catalogue —
Tier I/II/III (RARE/LEGENDARY/MYTHIC), origin & rarity rules (1 per region / per biome / per world),
properties (Field Amplification, Discipline Anchor, Gesture Extension, passives). **Dragon natural
artifacts** are created by Builder dragons with BLD alleles expressed under peak environmental
conditions (tie to `artifact_imbue` + BLD genes). Seams: `artifact_imbue.rs` (field amplification
already exists), worldgen/loot placement (bin), bending disciplines (resonance bending). **Pure**
data+rules core; placement is bin.

### G. Flight polish
Scope: thermals/updraft *sources* from the world (`dragon_mode::decide` already takes an `updraft`
param — needs a real field), landing-site selection, dive-attack targeting, multi-dragon spacing.
Seams: `dragon_air`/`dragon_mode`/`dragon_flight`, a world updraft field, bin spawns. **Pure** cores
+ bin for world coupling.

## 4. Prioritized roadmap (each ≈ one worker thread)
1. **E — breath-intent core** (pure). Smallest, high value: makes built breath weapons actually fire
   from the brain. Unblocks visible-breath render later. *Depends on:* nothing new.
2. **A — dragon_voice acoustic expression** (pure). The section's headline gap; everything voice
   builds on it. *Depends on:* canonical genome decision (Open Q1).
3. **D — owner-bond core** (pure). Needed before B has an owner to mesh with; also gates rideability.
4. **B — owner-voice mesh** (pure). *Depends on:* A + D.
5. **F — artifact data+rules core** (pure). Reuses `artifact_imbue`; independent of voice, can run in
   parallel with 1–4. *Unblocks:* loot placement.
6. **G — updraft field + landing-site core** (pure) then mode/air wiring. Polish; parallelizable.
7. **C — audio backend** (render/bin, compile-on-end). Last and largest external piece; consumes A/B.
   *Depends on:* A (and B for tamed timbre).
8. **F-place / E-VFX / D-commands** (render/bin) — the compile-on-end integration tails of 5/1/3.

Parallelizable now (pure, no cross-deps): **1 (E)**, **5 (F)**, **6 (G core)**. Voice chain is the
critical path: A → (B, C).

## 5. Open questions / decisions
- **Q1 (canonical dragon genome).** Several genome encodings coexist: `genetics::Genome` (8 named
  genes), `sound_genome`'s `[u8;6]`, `dragon_blueprint_from_genome::BodyGenes` (7 BODY genes),
  and `dragon_archetype::Genome`/`egg_genome::Genome`. Voice (A), breath (`dominant_element`), and
  body all index "the genome" differently. Decide the canonical dragon genome layout (and where BLD
  + disposition + elemental-attunement alleles live) before A/B — otherwise each module invents its
  own slice. Operator call.
- **Q2 (audio backend tech).** In-engine synth vs sampled bank vs hybrid for C? Determines whether A
  outputs synth parameters or sample-selection keys.
- **Q3 (artifact authoring).** Data-driven table (authored catalogue from the docx) vs fully
  procedural generation for F? The spec reads as an authored catalogue with procedural placement.
- **Q4 (BLD/disposition allele source).** Voice precision (BLD-3), environmental coupling (BLD-2),
  and prosody (disposition) need those alleles defined — confirm they come from the resonance-bending
  genome and are accessible to a pure dragon module.
- **Q5 (subsonic/“felt” voice).** Elder F0 is subsonic ("felt before heard") — needs a haptic/low-end
  channel decision in C, and possibly a gameplay "presence" cue independent of audio.
