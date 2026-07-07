# Genome Loci for Behaviour & Perception — the missing heritable channels

> Status: design (r287). Consolidates the gene gap flagged three times —
> `perception_and_population_behavior.md` §3 (temperament/sociality),
> `acoustic_perception_and_poi.md` §2 (hearing acuity / innate sound classes / POI
> disposition), `sim_vectors_and_correction_deltas.md` §4.5 (heritable temperament
> vs learned habit). One coherent set of loci to add to the genome, how they
> inherit, how they express into the built mechanism modules, and why they're the
> Class-C channels the deep-time population sim evolves.

---

## 0. Why this is the keystone

The perception + behaviour mechanism is built and pure: `acoustics` (hear), `poi`
(score → `Drives`), `behavior_arbiter` (choose), plus aggro + `combat_habit`. Every
one of them has a **disposition input** currently hard-coded or faked:
- `poi::PoiWeights` — how much food/shelter/kin/foe pull (faked: all 1.0);
- the arbiter's effective aggression — currently aggro/habit only, no baseline;
- `acoustics` `acuity` + the innate `SoundClass` set — faked: a fixed per-species
  table;
- the help-vs-flee tip a witnessed attack produces — currently aggro/habit only.

All of these *should be heritable* and *vary per individual*, then evolve per
population under selection. That's the genome's job. Add the loci once and four
systems stop faking their disposition — which is why the same gap kept surfacing.

The genome today carries body/sense traits (colour, size, eyesight, hearing, speed,
turn-rate). It has **no behavioural or dispositional loci**. This doc adds them.

---

## 1. The loci to add

All are scalar genes in `[0,1]` (the genome's existing trait representation),
expressed through small deterministic mappings. Grouped by what they feed:

**Temperament (feeds the arbiter + witnessed reaction + aggro):**
- `boldness` — baseline tip between fight and flee. High → the arbiter's Fight
  needs less provocation, a witnessed attack tips to *help*, aggro has a higher
  floor; low → flees early, scatters when kin are attacked. This is the literal
  "habitual animals fight the genetic trend" axis: `combat_habit` (learned, fast)
  bends a creature *away* from its `boldness` (genetic, slow) within a lifetime.
- `aggression_base` — resting aggression the aggro decay relaxes *toward* (instead
  of toward 0). A naturally aggressive lineage idles hostile.

**Sociality (feeds POI weights + flocking):**
- `sociality` — `poi::PoiWeights.friend` and the cohesion pull. High → herds
  tightly, comes to help kin; low → solitary, ignores the flock.
- `caution` — `poi::PoiWeights.shelter` + `.foe`. High → values cover, treats foes
  as more salient (skittish); low → exposed, bold.
- `foraging_drive` — `poi::PoiWeights.food` + how early hunger dominates.

**Senses (promote existing + add acuity properly):**
- `hearing_acuity` — the `acuity` arg to `acoustics::effective_hearing` (today read
  from the `senses` stat; make it a gene alongside eyesight). High → hears faint /
  distant / masked sounds; low → easily deafened by ambient.
- `eyesight` — already a gene; the witness sight-range + detection tier scale with
  it (so the perception range in the witnessed-attack reaction becomes per-individual
  rather than a constant).

**Innate acoustic knowledge (the "sound DNA" a creature is born with):**
- `innate_classes: small fixed-length table of SoundClass prototypes` — the sounds a
  creature recognises from birth (e.g. a prey lineage born knowing the local
  predator-roar prototype recognises the threat the first time it hears it; another
  must *learn* it by association). Stored as quantised prototype vectors so they
  inherit + mutate like any gene. Selection tunes the innate set per species over
  deep time — a population under heavy predation evolves a sharper innate predator
  prototype.

---

## 2. Inheritance

These ride the **existing breeding/crossover path** (`genetics.rs` +
`world/dragon_breeding`/offspring). Scalar loci: per-gene crossover + bounded
Gaussian mutation, same as colour/size. The `innate_classes` table: inherit
per-slot (each prototype slot crossed independently) with small vector mutation
(jitter a coefficient), then re-`l2_normalize` (so a mutated prototype stays a unit
vector `acoustics::classify` can use). Determinism preserved — offspring genome is a
pure function of (parents, seed), so the whole lineage is replayable and
multiplayer-safe (the deep-time invariant).

No temperament gene is "dominant/recessive" specially; the existing blend model is
enough to start. Heritability + variance is what matters, not a Mendelian model.

---

## 3. Expression (genome → the built modules)

A single `express(genome) -> Disposition` produces the inputs the mechanism modules
already take — nothing in those modules changes:

```
Disposition {
    poi_weights: poi::PoiWeights {           // §1 sociality/caution/foraging
        food:    f(foraging_drive),
        shelter: f(caution),
        friend:  f(sociality),
        foe:     f(caution),
    },
    aggression_base: f(aggression_base),     // aggro relaxes toward this
    boldness:        f(boldness),            // arbiter Fight bias + witness tip
    hearing_acuity:  f(hearing_acuity),      // acoustics::effective_hearing acuity
    sight_range:     f(eyesight),            // witness + detection range
    innate_classes:  Vec<acoustics::SoundClass>,  // born-known sounds
}
```

So the wiring becomes: `express` the creature's genome once (at spawn/hatch, cache
on the roamer) → feed `poi_weights` to `poi::to_drives`, `aggression_base` to the
aggro decay floor, `boldness` to the arbiter/witness tip, `hearing_acuity` +
`innate_classes` to the acoustic classification. The disposition is per-individual
and inherited; **the population's behaviour is the distribution of dispositions**,
which is exactly what evolves.

---

## 4. Why these are the deep-time Class-C channels

Each locus is a **path-dependent population channel** (`sim_vectors §4`): under
selection the *distribution* of `boldness`/`sociality`/`hearing_acuity`/innate
prototypes drifts (a region under heavy predation selects for caution + sharper
predator prototypes; an empty region drifts bold). A deep-time jump advances that
distribution statistically (the 8³ genome-summary redistribution, now including the
behavioural loci), while individual `combat_habit` is the fast within-lifetime layer
that runs against it (§4.5). The microsim that batches POI dynamics
(`acoustic_perception_and_poi.md` §5) reads these distributions as its disposition
priors and emits the sim-vector.

So adding the loci is also what makes the **population-behaviour half of the
deep-time sim real**: without heritable disposition there's nothing for selection to
act on and nothing for the jump to evolve; with it, "genetics works out the
population behaviour" becomes a literal computation.

---

## 5. Build order (when it happens)

1. Add the scalar loci to `Genome` + the `express(genome) -> Disposition` mapping
   (pure, testable now — no GPU). Default mapping reproduces today's constants so
   nothing changes until wired.
2. Cache `Disposition` on the roamer at spawn/hatch (one field, like the others).
3. Wire expression into the mechanism calls (`poi::to_drives` weights, aggro floor,
   arbiter/witness boldness, `acoustics` acuity). Behind a kill switch; behavioural
   → needs the M2.
4. `innate_classes` inherit + mutate in the breeding path.
5. Population sim (deep-time) reads the disposition distribution — gated on Step 0 +
   the fill.

Step 1 (the loci + `express`) is the next pure-CPU buildable piece and unblocks the
genetic half of `poi`, `acoustics`, the arbiter, and the witnessed reaction at once.

> Honest status: nothing here is built yet — it's the consolidated spec for the gene
> gap. The mechanism modules it feeds ARE built (`poi`, `acoustics`,
> `behavior_arbiter`, aggro/habit); they're waiting on exactly these loci to stop
> faking their disposition inputs.
