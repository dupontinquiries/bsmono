# Social Behaviour: Perception in Real Time, Genetics in Deep Time

> Extended to HEARING in `acoustic_perception_and_poi.md` — sound has an acoustic FINGERPRINT (classified nearest-region + margin, adapted from voiceterm); hearing is masked by ambient noise (waterfall/wind floods ears) and coloured by echo; acuity + innate sound-classes are genetic; and creatures balance POIs (food/shelter/kin/foe) by score for the near-term while a microsim batches POIs into a sim-vector for the long-term.

> Status: design + first build (r284). Captures the operator's architectural
> correction: *the omniscient per-frame herd scan is the wrong model for real
> time — animals should react to what they can SEE (a herdmate attacked → help /
> attack / flee / other), and genetics works out the population-level behaviour
> over generations. The global herd computation belongs to a deep-time sim-vector
> jump, not the frame loop.* Ties together `awareness_blend` (sight), the
> behaviour arbiter + aggro + combat-habit, and the deep-time genetic channel
> (`sim_vectors_and_correction_deltas.md` §4–4.5).

---

## 0. The principle

Two regimes, two cost models — the same split as the whole deep-time design:

- **Real time = PERCEPTION.** A creature knows only what its senses deliver this
  moment. Social reactions are **local and event-driven**: it witnesses something
  within sight/earshot and responds. Cost ∝ events × local neighbourhood, which is
  tiny because events (a kill, an attack) are rare and perception range is short.
- **Deep time = GENETICS + STATISTICS.** Population-level behaviour — does this
  species mob predators, scatter, or ignore the weak being culled? — is the
  *aggregate* of many individual perception-driven reactions across generations,
  under selection. That aggregate is a **statistical population computation**, and
  it belongs in a sim-vector jump (the omniscient herd math), evaluated over a long
  span, **not** recomputed every frame.

The omniscient O(n²) per-frame herd scan conflates the two: it does deep-time
population reasoning (every creature consults global herd state) on the real-time
clock. That is both **too expensive** (n² per frame) and **wrong** (animals are
not omniscient). Split them.

---

## 1. Real time: witnessed-attack reaction (built, r284, `UNENDLICH_WITNESS`)

The canonical case the operator named — *see a herdmate attacked → help, attack,
flee, or something else* — is now event-driven and perception-gated:
- When a creature is harmed (the combat/bending harm points), it records an
  **attack event** `(victim_position, victim_kind)` into a per-frame buffer.
- In the roamer loop, each creature scans that buffer (usually empty → free) for an
  attack within **witness sight range** on a **same-kind herdmate**. Seeing one
  raises its `aggro` toward help/defend.
- The **behaviour arbiter** then resolves the actual response from the creature's
  drives: a bold individual (high aggro/combat-habit) tips to **Fight** → moves to
  help/defend; a timid or wounded one's **Flee** dominates → it scatters. "Help vs
  flee" is *emergent from the scoring*, not a hand-coded branch — exactly what the
  operator asked for ("help, attack, do something else, or run").

Cost: O(events × creatures) where events ≈ 0 most frames, so it's an empty-Vec
check per creature when nothing is happening — the opposite of the n²-every-frame
herd scan. Perception range bounds propagation (the deep-time through-line, applied
to behaviour).

**What's still coarse (honest):** the "can see it" test is currently a range check
(witness radius), not a full line-of-sight raycast or sight-cone — occlusion and
facing aren't modelled yet. Hearing (a distress call carrying past line-of-sight)
and smell aren't wired into witnessing yet, though `awareness_blend` already blends
all three for player-detection and is the natural home for it. And the reaction
only keys on the player as attacker; creature-on-creature attacks reuse the same
buffer but predator prey-selection (below) is still the old scan.

---

## 2. The other half: perception-bounded prey selection (next, not yet built)

The remaining O(n²) is **prey selection** — a hungry predator scanning the whole
herd to pick a target (and the dragon hunt doing the same). Same fix, same
principle: a predator hunts what it can **see**, so it should consult only nearby
creatures, not the global herd. The mechanism is a **spatial grid** rebuilt once
per frame (O(n)); each predator queries its local cells (≈O(1)) — which *is* a
perception index (you only see what's in range). That turns the n² tail into ≈O(n)
and makes hunting sight-bounded at the same time. Filed as the follow-up; it needs
the r283 sub-zone numbers first to confirm herd-scan is actually a big share of the
17 ms roamer cost before investing in the grid.

---

## 3. Deep time: genetics works out the population behaviour

Whether a species *tends* to mob, scatter, or ignore is a **heritable disposition**
shaped by selection — a Class-C path-dependent genetic channel
(`sim_vectors_and_correction_deltas.md` §4). Over a deep-time jump:
- individual reactions (§1) are the fast, per-lifetime layer (and `combat_habit` is
  the learned within-lifetime bias on top — §4.5, the habit-vs-genome opposition);
- the **population's** behavioural tendency is the slow genetic layer the sim-vector
  advances statistically (the omniscient herd math, run once over the span, not per
  frame);
- selection couples them: dispositions that survive (mobbing works here, scattering
  works there) become more common — emergent population behaviour, computed in the
  jump, materialised on load.

**The missing gene (filed twice now).** The genome carries
colour/size/senses/speed — there is **no temperament/sociality locus** yet. Adding
a `boldness`/`sociality` gene is what makes "genetics works out the population
behaviour" literal: it would set each creature's baseline help-vs-flee tip (which
§1's witnessed reaction then expresses, and `combat_habit` then bends within a
life). Until then, §1's disposition comes from aggro/habit alone — a proxy for the
gene, not the gene.

---

## 4. How the pieces line up

| Timescale | Mechanism | Status |
|---|---|---|
| seconds (this moment) | witnessed-attack reaction via sight + arbiter (`UNENDLICH_WITNESS`) | **built (r284)**, sight = range check |
| seconds (hunting) | perception-bounded prey selection via spatial grid | filed (§2) |
| a lifetime | `combat_habit` learned bias (`UNENDLICH_ARBITER`) | built (r272) |
| generations | heritable temperament/sociality gene → population tendency | needs the gene (§3) |
| deep-time jump | population behaviour as a statistical sim-vector op (the herd math) | needs the fill (Step 0) |

The invariant: **real-time social behaviour is local and perception-gated; global
herd/population behaviour is a deep-time statistical computation.** Never run the
population math on the frame clock; never make a real-time creature omniscient.


> See also: `light_field_architecture.md` (r298) — sight's environmental field.
> Mob perception ("eyes") reads the light field for a visibility/contrast term; like
> hearing, it is a local real-time read of a stored field, never a global recompute.
