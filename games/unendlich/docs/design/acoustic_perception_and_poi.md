# Acoustic Perception & POI Scoring: Hearing as Genetics, POIs as the Decision Layer

> Status: design + first build (r285). Captures the operator's direction: *sound
> should be part of genetics; a sound has an acoustic FINGERPRINT (adapted from the
> voiceterm voice-fingerprinting code) that classifies it against a "sound DNA"
> database; environment changes hearing (mobs at a waterfall/windy area don't use
> their ears the same); ECHO and volume change reactions; and creatures balance
> POIs (food / shelter / friends / foes) by score over a distance radius for the
> near-term decision, while the long-term runs a MICROSIM that batches these and
> emits a sim-vector.* Extends `perception_and_population_behavior.md` (sight) to
> hearing, and ties both to the deep-time machinery.

---

## 0. The shape

Same two-regime split as everything else, now for hearing + goals:

- **Real time = PERCEPTION + LOCAL SCORING.** A creature hears sounds (classified by
  fingerprint, attenuated by ambient noise + distance, coloured by echo) and scores
  nearby POIs (food/shelter/kin/foe) by type × distance. The argmax pull drives the
  near-term move. Local, cheap, per-creature.
- **Deep time = GENETICS + MICROSIM → SIM-VECTOR.** Hearing acuity, the innate sound
  classes a creature is born knowing, and the POI-weighting disposition are
  heritable (Class-C genetic channels). The long-term world doesn't score POIs
  per-frame for off-screen regions — it runs a **microsim** that batches the POI
  dynamics over a span and emits a **sim-vector** that guides the jump (the
  `sim_vector`/`sim_chain` machinery already built).

---

## 1. Sound has a fingerprint → classify against a "sound DNA" database (built core, r285)

Adapted directly from `voiceterm`'s `speaker_embed`: **identity is a region in
feature space, not a scalar.** `sim::acoustics` provides the pure mechanism:
- a **fingerprint** = a short L2-normalised vector of pooled spectral stats
  (`FINGERPRINT_DIM`);
- a **sound class** = a stored prototype vector + an id (predator-roar, conspecific
  call, waterfall, wind, footstep…) — the prototypes ARE the "sound DNA database",
  a fingerprint is the key into it;
- **`classify`** = nearest prototype by `cosine_distance`, gated by a confidence
  **margin** (distance to 2nd-nearest − nearest). Large margin = confident; small =
  the sound sits between classes → the creature stays **wary** instead of guessing
  (the same "refuse to identify when ambiguous" guard the voice code uses).

This is why acoustic classification beats a hard-coded "is this sound a threat?"
flag: a creature can mishear, can be uncertain, and can *learn* new prototypes —
and (below) the prototype set is partly **inherited**.

## 2. Hearing is genetic

Heritable acoustic traits (Class-C channels, `sim_vectors §4`):
- **acuity** — how far/faint a creature can hear (the `acuity` arg to
  `effective_hearing`); already mirrors the existing `senses` hearing stat, but
  belongs in the genome alongside eyesight.
- **innate sound classes** — a prey species born knowing the predator-roar
  prototype (recognises the threat the first time it hears it) vs one that must
  learn it. Selection tunes the innate set per species over deep time.
- **POI-weighting disposition** (§4) — how much this lineage values shelter vs food
  vs flocking, the heritable bias the arbiter expresses.

This is the same "perception in real time, genetics in deep time" split, and it
needs the same missing piece: **a genome with sense + temperament loci** (filed in
`perception_and_population_behavior.md` §3). Until then acuity comes from the
`senses` stat and the innate classes are a fixed per-species table.

## 3. Environment changes hearing — masking + echo (built core, r285)

- **Masking** (`effective_hearing(volume, ambient, acuity)`): a creature's usable
  hearing is its acuity scaled by how far a sound rises above the **local ambient
  noise floor**. At a waterfall or in high wind (`ambient` high) even loud sounds
  are lost — *mobs there literally don't use their ears the same*, exactly as the
  operator describes. A sound at/below the ambient floor is inaudible regardless of
  acuity. This makes **acoustic environment a tactical dimension**: a predator
  approaching along a noisy stream is harder to hear; prey near a waterfall is
  jumpier (it knows it can't hear, so it leans on sight — a behaviour the arbiter
  can encode).
- **Echo** (`spatial_impression(HeardSound{volume, echo})`): echo + volume give a
  coarse `(perceived_distance, enclosure)`. Reverb reads as "far / large space";
  dry + loud reads as "right here". Creatures react differently to an echoing
  predator call from a canyon (ambush ahead → avoid the canyon) vs a near dry one
  (flee now). The reaction policy lives with the caller; the module turns the
  physical cues into perceptual scalars.

**Ambient sources (to add, not yet built):** wind, dust, surf, and waterfalls
should emit an ambient noise field (a per-region scalar the masking reads), and the
audio side should play matching ambient beds. Echo should come from local geometry
(enclosed/canyon vs open) — a coarse per-region "enclosure" scalar to start, real
geometry later. These are the inputs §3's math consumes; filed.

## 4. POIs as the near-term decision layer (design; partial primitives exist)

Creatures **balance Points Of Interest by score over a distance radius**:
food, shelter, friends (kin/flock), foes (predator/player). Each POI has a position,
a type, and a salience; a creature scores them by **type-weight × distance-falloff**
and the dominant pull "underfoots" the near-term prediction — i.e. it sets the
drives the **behaviour arbiter** then resolves (a strong foe-POI nearby → threat
drive up → Flee/Fight; a food-POI when hungry → Forage; a shelter-POI at night or
under threat → move to it). This is the unifying substrate the witnessed-attack
reaction (a foe/distress POI), the predator hunt (a food POI), and flocking (a
friend POI) all reduce to.

Cost model (the operator's "underfoot the near-term"): POIs are **few and local**
(the handful of salient things near a creature), discovered by perception (sight +
the §1–3 hearing), so scoring is cheap per creature — unlike the omniscient herd
scan. The POI set is the perception output; the score is the utility input to the
arbiter. (Primitives that already exist and fold in here: `behavior_arbiter` =
the scorer/selector; `feeding_select`/`predator_prey` = food-POI logic;
`herd_cohesion` = friend-POI logic. The missing piece is a unified `Poi { pos,
kind, salience }` + a `score_pois` that produces the arbiter's `Drives`.)

## 5. Long-term: microsim batches POIs → a sim-vector (design)

Off-screen / deep-time, you do **not** score POIs per creature per frame. The
operator's model: run a **microsim** that **batches** the POI dynamics for a region
over a span — aggregate flows (this herd drifts toward the water POI, predators
concentrate where prey density is high, a shelter POI anchors a population) — and
emit a **sim-vector** (`sim_vector.rs`) that guides the long jump. The microsim is
the cheap statistical stand-in for "what all these POI-driven decisions add up to",
and its output is exactly a checkpoint-director with waypoints + corrections. This
is the behavioural analogue of the convergence-class story: the per-creature POI
scoring is the divergent near-field; the batched microsim is the convergent /
statistical far-field; the sim-vector carries the landing.

Concretely it slots into the built machinery: the microsim produces the
`quantized_state` + class hints that `SimVector::plan` / `plan_chain` consume, and
the POI-aggregate digest feeds `region_digest` (so two regions whose POI dynamics
converge share/merge). The acoustic environment (ambient/echo per region) is part
of that region state — a noisy region genuinely plays out differently.

---

## 6. Status + what's built vs filed

| Piece | Status |
|---|---|
| sound fingerprint + classify (nearest-region + margin) | **built (r285)** `sim::acoustics` |
| environmental masking (waterfall/wind floods ears) | **built (r285)** `effective_hearing` |
| echo → distance/enclosure cue | **built (r285)** `spatial_impression` |
| hearing acuity + innate classes + POI disposition as GENES | filed — needs genome loci |
| ambient noise + echo source fields (wind/dust/surf/geometry) + audio beds | filed |
| unified `Poi {pos,kind,salience}` + `score_pois → Drives` | filed (primitives exist) |
| microsim batching POIs → `SimVector` | filed — needs Step 0 + the fill |

Invariant, restated for hearing + goals: **perception and POI scoring are local and
real-time; acoustic genetics and the POI microsim are deep-time statistical
computations.** A creature hears what its ears (masked by its environment) deliver
and chases the POIs it can perceive; the population's acoustic adaptation and
long-range POI flows are computed in the jump, never per-frame.


> See also: `light_field_architecture.md` (r298) — the sibling sense. Hearing has
> `acoustics`; sight has the light field (sky+block, fractional transmittance, baked
> hemispherical exposure). Both are local real-time reads of stored/propagated state,
> sampled through `sim::world_prober`; the population-scale adaptation is deep-time.
