# An Experience Brain for Unendlich Creatures

*From "eyes observing" to creatures that learn — staged, honest, and built on what already exists.*

This is a design + implementation guide. It proposes a way to make the lauscher
stalk/learn and dragons master their elementals, by reusing a loop we validated on
a separate project (a code-evolution swarm). It is written to be **staged** and
**measured** — the early stages are solid engineering you can ship; the later ones
are research bets you should prototype and ablate before committing the architecture
to them. Where something is aspirational, it says so.

---

## 0. The one idea

A creature that learns is the same loop as a search-and-verify system:

```
observe  →  turn the observation into an objective/test  →  propose behaviour variations
        →  keep what passes the test  →  distill the winner into a cheap, fast policy
```

"Eyes observing → a sim test generated on the spot" is exactly the middle of that
loop: the thing the creature watches becomes the *success predicate* it then tries to
satisfy by trial. Watching the player vault a wall defines "get from A to B over this
gap." Watching a combo defines "reproduce/counter this move sequence." Seeing an item
while unobserved defines "reach it with time-in-view ≈ 0."

The catch — and the rest of this guide hangs on it — is **where** each part of that
loop is allowed to run.

---

## 1. The one constraint that makes or breaks this: you cannot microsim per frame

This section exists to stop the whole thing from becoming architecture cosplay.

A creature "simulating itself with variations" sounds great until you price it. On the
project this came from, a single behaviour evaluation cost ~18 **seconds** (it ran real
tests). A game frame is ~16 **milliseconds**. You cannot run internal simulations,
evolution, or planning rollouts inside the per-creature, per-frame path. If you try, the
frame budget is gone after one creature.

So the architecture is forced into two tiers, and the discipline is non-negotiable:

- **Reactive tier** — cheap, deterministic, runs every (banded) tick for every creature.
  This is essentially what you already have (`goals::choose`) plus the stateful FSM from
  Stage 0. It never simulates anything; it reacts.
- **Experience tier** — expensive (search / sim / planning). Runs **rarely**, for **few**
  creatures, on **surrogate** models (never the real game), **amortised** across many
  ticks, and its only output is to **distill** a result back down into the reactive
  tier's parameters. It is gated by `ai_budget` — and your `should_full_tick` /
  `TickBand` distance banding is the perfect gate: only near-band creatures, on a slow
  period, ever pay for it.

If a stage can't fit this shape, it doesn't ship. Keep that rule and the rest is sound.

---

## 2. Where this plugs into the code you already have

You don't rip anything out. Every piece below is a seam onto existing types.

| Existing piece (`unendlich-engine/src/sim/…`) | Role in the experience brain |
|---|---|
| `goals.rs` — `Goal`, `Signals`, `Disposition`, `score`, `choose` | The reactive vocabulary. The FSM *wraps* `choose`; learning *writes back* into `Disposition`. |
| `mob_memory.rs` — `Memory{pos,salience,last_seen}`, `remember/recall/decay` | Stores objectives (item pos, vantage, last-heard) and the stalk's refuge. Already has salience + decay — the substrate for "over long time." |
| `awareness.rs` — `best_facing` | Steers gaze toward information; reuse for the Orient state and for scanning. |
| `ai_budget.rs` — `TickBand`, `should_full_tick` | The gate for the experience tier. Expensive work only on full-tick, near-band creatures. |
| `Fixed` (fixed-point) | Determinism. Keep every learned value `Fixed`; seed all randomness from the mob's RNG so replays stay bit-exact. |

The reactive tier reads `Signals` + `MobMemory`, runs the FSM (which calls `choose`
inside a state), and moves. The experience tier, when the budget allows, reads the same
state, runs a surrogate search, and edits `Disposition` / FSM weights. Two tiers, one
shared state, clean seam.

---

## 3. Stage 0 — Base: a stateful behaviour FSM (ship this first, no learning yet)

Goal: make the lauscher *temporally coherent* — phases that persist over time — without
any learning. This alone makes it feel alive; everything later is optional on top.

Replace the "argmax every tick" feel with a small per-creature FSM whose states are
behavioural **modes**, and let `goals::score` provide the **transition weights**. The
lauscher (your hearing sentinel) gets six states:

```
Listen → Orient → Shadow → Observe → (Freeze if watched) → Withdraw → Listen…
```

The two registers that create "creepy, over a long time":

1. **`standoff`** — the distance it holds during `Observe`. Each `Observe` cycle it
   completes *uncaught*, shrink it a little (curiosity beats caution). The moment it's
   seen, jump it back out (caution spike). Over minutes it ratchets inward: *"it's closer
   than last time."* That one decaying number is the whole stalking core.
2. **`curiosity`** — an **accumulator**, not your current instantaneous `novelty ×
   curiosity`. Integrate exposure to the target; habituate (decay) once familiar. The arc
   build → peak → lose interest reads as a mind, not a trigger.

See `sketch/lauscher_brain.rs` for a concrete, deterministic sketch grounded in your
types. It is a *sketch to adapt to your tick loop*, not drop-in code.

**New signal you must add:** "am I being looked at." Your `awareness.rs` computes the
*mob's* facing, not whether the *player's* view contains the mob. Add a check from the
player's frustum/facing + the mob's position. It's the linchpin of the Freeze beat.

Ship Stage 0. Tune it. If it isn't compelling by hand, no amount of learning will save
it — learning amplifies a good substrate, it doesn't create one.

---

## 4. Stage 1 — Eyes → objective (observation becomes a spec; still no search)

Now make observation *produce* something. When the lauscher perceives a salient event,
write an **objective** into `MobMemory` (you already have `remember`):

- sees an item → `remember(item_id, pos, value_as_salience, now)`; objective = "reach it".
- hears the player repeatedly from a spot → a vantage objective.
- the FSM's `Shadow`/`Observe` now serve the recalled objective (`recall(from, radius)`).

The "on-the-spot test" is just a **success predicate** attached to the objective:
`reached(pos) && time_in_player_view ≈ 0`. No search yet — the hand-tuned FSM either
satisfies it or doesn't. This stage is cheap and is the seam the experience tier plugs
into next. Ablate: does objective-driven behaviour beat pure patrol/wander? (It should.)

---

## 5. Stage 2 — The experience tier: budgeted micro-search + distill

Now the loop closes, off the hot path. Under `should_full_tick` (near band, slow period),
run **one** trial per budgeted tick:

1. **Propose**: mutate the creature's FSM weights / `Disposition` (reuse the structural
   operators + `evolve_toward` in `reference/fsm.rs`).
2. **Score**: run the candidate against the Stage-1 objective in a **cheap surrogate
   sim** — for stealth/items that's pure *geometry*: visibility cones + a path check, no
   physics. Score = reached ∧ low view-time.
3. **Keep + distill**: if the candidate beats the incumbent, write its params back into
   the creature's reactive policy. The expensive search ran once; the creature now *acts*
   at frame rate with an improved cheap policy.

This is the macro-discovery loop from `reference/macros.rs`, creature-side: harvested
outcomes → promote what recurs/works → run the cheap distilled policy; re-search rarely.
Start with **stealth + item theft only** — it has the cheapest surrogate. Defer anything
needing physics.

**Honesty gate:** amortise hard (one trial/tick, a handful of creatures), and *ablate* —
a lauscher with the experience tier vs one without, measured on "time near player while
unseen." If it doesn't beat the hand-tuned Stage-0 FSM, cut it; the FSM was enough.

---

## 6. Stage 3 — Imitation & fighting-style adaptation (transfer + verifier + search)

Same loop, richer objective. Observe the player's action **sequence** and fit a small FSM
to it — that's a model of the player's *move grammar* (this is literally style transfer:
factor the player's style out of observed traces, then transfer a response into the
creature's own behaviour). Then search a **counter-policy** scored against a sim of that
grammar. "Adapts to your fighting style" = it keeps refitting the grammar as you change.

**Parkour** is the same shape but needs a *physics* surrogate, which is the expensive end.
Treat it as: learn for a few "hero" creatures, offline/heavily-budgeted, distilled into a
reusable traversal policy — not something every mob computes live.

---

## 7. Stage 4 — Dragons: a self-model and mental simulation (research bet — prototype, measure, don't pre-commit)

This is the "experience part of the brain," and it's the most aspirational. The honest
framing is **model-based planning with a surrogate self-model**:

- The dragon does **not** run the real game inside itself. It runs a *cheap surrogate*: a
  small hand-coded or learned forward model that predicts the outcome of an elemental
  choice in an *abstracted* situation (range band, cover, target type, wind/wet/dry).
- "Simulating with variations" = domain-randomised rollouts over that surrogate to find a
  policy that's *robust*, not overfit to one encounter.
- The "experience brain" is the harvest: log real `(situation, elemental, outcome)` tuples
  from actual play (your `MobMemory` + a ring buffer), distill them into a fast policy
  `situation → elemental`, and run **that** at frame rate. The surrogate planning runs
  rarely (budgeted, near-band), to refine the policy and to "dream" variations the dragon
  hasn't met yet.

Why this is a bet: the value of mental simulation depends entirely on the surrogate being
*cheap and faithful enough*. That's the thing to prototype on **one** dragon and measure
(does planning-refined elemental use beat a hand-tuned table?) before letting it shape the
architecture. If the surrogate is expensive or wrong, this collapses to "an expensive way
to get a hand table." Build the harvest + fast policy first (solid); add surrogate
planning only if it earns its keep.

---

## 8. The discipline that keeps this honest: ablate every tier, budget every stage

This is the most important section, ported wholesale from how the source project stayed
real:

- **Each tier must earn its place by measurement.** Creature *with* vs *without* the new
  tier, on a metric that matches the intent (stalk: time-near-while-unseen; combat:
  win-rate vs a fixed player policy; dragon: damage-per-mana or survival). If the added
  tier doesn't beat the simpler one, **cut it** — the simpler tier was enough.
- **Each stage must fit the frame budget before the next begins.** Profile the experience
  tier's amortised cost per frame at your target creature count. If it doesn't fit, it
  doesn't ship.
- **Gate the stages.** Don't start Stage N+1 until Stage N is ablated *and* within budget.
  Stages 0–1 are reliable value. Stage 2 is feasible with care. Stages 3–4 are research —
  prototype small, measure, and be willing to stop at "the hand-tuned version was as good."

Learning systems also produce *degenerate* behaviour (exploits, jitter, standing in a
corner forever). Keep guardrails (clamp learned params, sanity-bound the FSM) and let the
ablation metric catch regressions.

---

## 9. What to port (included in this package)

- **`reference/fsm.rs`** — a weighted generative automaton with memory states
  (capture/recall/backreference), structural operators, and `evolve_toward`. This is the
  behaviour-grammar substrate and the search operator. Port the *ideas* and the
  `evolve_toward` mechanics; the alphabet becomes your move/behaviour vocabulary instead
  of characters.
- **`reference/macros.rs`** — the experience loop: harvest → discover what recurs/works
  (with a generality threshold) → apply as a cheap tier → **ablate**. This is the template
  for the experience tier *and* its validation discipline.
- **`sketch/lauscher_brain.rs`** — a concrete, deterministic Stage-0/Stage-2 sketch
  grounded in your `Goal`/`Signals`/`Disposition`/`MobMemory` types. Adapt, don't paste.

Mapping at a glance:

| Source-project concept | Unendlich creature analogue |
|---|---|
| FSM character alphabet | move / behaviour-primitive vocabulary |
| `evolve_toward(target)` | evolve a behaviour FSM toward a behavioural fingerprint |
| `experience.jsonl` (harvested wins) | harvested `(situation, action, outcome)` from real play |
| `discover` / recurrence threshold | promote behaviours that generalise across encounters |
| `ablate(with vs without)` | measure each tier; cut what doesn't beat the simpler one |
| cheap tier 0 + expensive tier 1 cascade | reactive (frame-rate) tier + budgeted experience tier |
| `git apply --check` validity gate | the surrogate-sim success predicate / guardrails |

---

## 10. Risk register (read before scoping)

1. **Performance is the whole game.** Per-frame microsims are infeasible. The two-tier +
   surrogate + amortise + distill shape is the only viable one. (§1)
2. **Surrogate fidelity** (dragons especially): a cheap, wrong model teaches the wrong
   lesson. Measure the surrogate against real outcomes. (§7)
3. **Determinism / replay**: keep `Fixed`, seed from the mob RNG, keep FSM transitions
   deterministic given (signals, memory, seed). (§2)
4. **"Creepy/fun" is presentation**, not just AI — timing, sound, intermittent reveal. The
   FSM gives the substrate; the felt scare lives in animation/audio. (§3)
5. **Degenerate learned behaviour**: clamp params, bound the FSM, let ablation catch it. (§8)
6. **Scope honesty**: Stages 0–1 are the reliable win. 2 is feasible. 3–4 are research —
   prototype and be willing to stop early. (§8)

---

## Suggested order of work

1. Stage 0 FSM for the lauscher + the "am-I-watched" signal. Ship + tune by hand.
2. Stage 1 objectives in `MobMemory`. Ablate vs patrol.
3. Stage 2 experience tier (stealth/items only), gated by `should_full_tick`. Ablate vs
   Stage 0. Profile the amortised cost.
4. Only then consider Stage 3 (combat-style) and Stage 4 (dragon self-model), each as a
   measured prototype.

Build the base; let each rung earn the next.
