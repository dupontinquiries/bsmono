# Cats that move like Warriors cats: what exists, what does not

A design note for clawbound-battleground, written after auditing what
`unendlich` already simulates. The short version: **most of the sensing is
built and tested; almost none of the AUTHORING is.**

## The passage is a spec

```
Hollyleaf's pelt prickled ... suddenly Jayfeather nudged her hard in the side
"ShadowClan!" he hissed. "Hide!"
All three cats dived for cover into a bramble thicket.
"Ivytail, Smokefoot, and Owlpaw. They're patroling the border.
 I hope they didn't scent us."
No challenging yowls came ... the scents gradually died away, leaving only
the smell of the border markers.
```

Read as mechanics, that paragraph asks for eight things:

| beat | what it needs |
|---|---|
| Jayfeather senses the patrol first (he is blind) | hearing and scent as channels **separate from sight** |
| "I hope they didn't scent us" | the player emits scent and can be detected by it |
| dived into a bramble thicket | cover that breaks line of sight but not smell |
| the patrol does not challenge | a detection threshold that can be *missed*, not a boolean |
| scents "gradually died away" | scent decays over time, and the decay is readable |
| "leaving only the smell of the border markers" | **placed, persistent** scent distinct from creature scent |
| racing, weaving around thickets, panting halt | stamina bound to gait |
| stalking a squirrel, claws tore the ground, it bolted | prey with its own perception of *you* |

Six of the eight already exist.

## Already built, and now shared

`games/shared/wildkit` — 21 modules, 3,443 lines, 137 tests, extracted
verbatim from `unendlich-engine/src/sim` because copying would have forked
actively-developed code.

| beat | module | shape |
|---|---|---|
| separate channels | `sight_detect`, `hearing_radius`, `creature_hearing` | range per channel, not one "detect" |
| a detection that can be missed | `perception::perceive`, `PerceptParams` | and `in_blind_spot` / `being_watched` — a mob has a blind arc behind it, which is what makes flanking real |
| scent emitted and found | `scent_strength`, `scent_gradient`, `scent_track::step_toward` | `step_toward(here, [4 neighbours]) -> u8` — following a trail is a gradient walk, already written |
| scent dying away | `scent_decay` | the "gradually died away" line, literally |
| wind turning a stalk | `wind_field::wind_at(x, z, t)` | a field over space AND time, so the wind shifts mid-stalk |
| fog, dusk, light | `fog_model`, `light_physics` | the LOS-vs-ears trade the brief asks for is these two feeding the sight range |
| sound blocked by rock | `audio_occlusion`, `spatial_audio` | |
| prey that flees | `predator_prey::decide`, `prey_scan` | prey scans for you while you scan for it |
| cost of crossing ground | `path_cost`, `step_height`, `neighbor_grid` | |
| remembered routes | `path_memory` | patrols that reuse a path are free |
| streams | `river_flow`, `terrain_variation` | `terrain_variation` already builds channels from flow accumulation |

**`step_height` deserves a callout.** `max_step` / `max_step_on_slope` is the
difference between a cat and a player capsule: a cat leaps a rock a human
vaults. Tuning that one function is most of "moves like a cat" before any
animation exists.

## Not built, and this is the actual work

### 1. Scent MARKERS are not creature scent

`scent_*` models a trail an animal lays as it moves. Border markers are the
other thing: **placed once, refreshed on patrol, decaying over days not
seconds, and owned by a clan rather than an individual.** "Leaving only the
smell of the border markers" is the whole territorial system in one line, and
nothing models it. It wants its own type, not a long-lived creature scent —
the refresh-on-patrol behaviour is a gameplay loop, not a decay constant.

### 2. Cover is not modelled at all

`clawbound-battleground/src/blockers.rs` (added this session) answers "can I
walk through it" and nothing else. A bramble thicket is the opposite of a
wall: **passable, and it hides you.** That is a second property on the same
props, and it is what makes the map readable — you learn where to dive.

Concretely, `blockers::Footprint` grows a `concealment: f32` beside its
`radius`, ferns and bushes go from `passable()` to passable-but-concealing,
and `sight_detect` takes the sum along the ray. The blocker grid is already
the right spatial index for it.

### 3. Stamina did not come across

`unendlich`'s `stamina.rs` reaches into a service registry, so it stayed
behind. It is the module the brief most explicitly asks for — streams that
restore it, a run that must end in a panting halt — and it needs a seam cut
before it can travel. **Next extraction.**

### 4. Fresh-kill as a stockpile

"Hunt early before being chased and stock up fresh kill for later" is an
economy, not a heal: prey caught → carried → cached → eaten. `forage_*` in
unendlich is about a creature feeding itself, which is a different loop.

### 5. Time of day and season

`biome_climate` stayed behind too (it reaches `world::biomes`). Without it
there is no clock, and the brief's "LOS vs ears varies by time of day" has
nothing to vary against. Extracting it is cheap — one enum's worth of seam.

### 6. The spirit sites and the shadow realm

Nothing exists and nothing should be borrowed. A StarClan buff site is an
ordinary zone-with-an-effect; the interesting one is the **shadow realm
spreading into the real world**, which reads as a per-tile overlay that
changes which senses work — sight fails, scent persists, hearing lies. That
is a modifier over `PerceptParams`, which is the right shape precisely
because perception is already parameterised.

## Animation: what the rig needs that it has not got

`games/shared/creature-rig` poses the body and `clawbound/src/anim.rs`
chooses the gait. Between them they cover walk/trot/pace/canter/bound, a
tail that lags a turn, ears that pin with speed, and — added this session —
flinch, guard, cast and death timers.

What the passage asks for that is missing:

* **Stalk is not a gait, it is a posture.** `Gait::Pace` is the creep, but a
  stalking cat also drops its chest, shortens its stride and stops moving its
  tail. That is `Build`-independent and belongs as a posture blend, not a
  sixth footfall pattern.
* **Leap is not a jump.** `MoveMode::{Anticipate, Airborne, Landing}` exists,
  but a cat's leap is aimed at a *place* — a branch, a rock — and the
  interesting part is the gather before it and the balance check after.
  Wants a target, not just a phase.
* **"Dived for cover" is a transition into a state**, not a movement. The
  body has to end up *lower than the terrain height*, which no current pose
  expresses.

## Order I would take this in

1. **Cover on blockers** (`concealment` on `Footprint`). Smallest change,
   biggest effect on how the map plays, and the grid already exists.
2. **Extract `stamina`** — cut the service-registry seam. Unlocks streams,
   the panting halt, and the chase economy.
3. **Scent markers** as their own type, with refresh-on-patrol.
4. **Extract `biome_climate`** for the clock, then time-of-day perception.
5. Stalk posture and aimed leap in the rig.
6. Fresh-kill economy, spirit sites, shadow realm.

Steps 1–4 are mostly wiring existing, tested code. Step 5 is real animation
work. Step 6 is design.
