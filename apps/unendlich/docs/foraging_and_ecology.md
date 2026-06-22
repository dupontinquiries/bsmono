# Foraging, mating, and a living ecology — design notes

Status banner (2026-06-02): the **mechanisms** below are landing as tested pure
models wired into the roamers one at a time. Built so far: senses (vision +
hearing from genome), landscape memory, feeding/energy + food-gated breeding,
biological sex + mate-picking, the detent + nonlinear turn map, and the
population-dynamics + cliff-march test harnesses. This doc records the intended
*behaviour* these compose into, and the longer-horizon ecology ideas, so the
wiring order stays clear.

## Foraging by memory, not by omniscience

The fauna must not do "world detection" — no scanning the whole map for grass.
They forage the way they already survey: **by looking around.** The loop:

1. **Looking is already valuable.** A creature gains liveliness and builds world
   knowledge by surveying (the awareness/landscape-memory layer). That same act
   is how it *discovers food*.
2. **Notice → remember with coordinates.** When a creature's gaze falls on grass
   (a food block within its view cone and range), it records that location in a
   **forage memory** — a small, fading set of known food sites, like landscape
   memory but tagged as food.
3. **Hunger sets a goal.** While well-fed it wanders idly (detent + nonlinear
   turn map — looks alive). When [`Vitality`] drops, it switches to **goal
   mode**: pick the nearest remembered food site and set a position/direction
   goal toward it. Goal pathing turns *crisply* (plain rate cap), unlike idle
   wandering.
4. **Eat → feed → maybe breed.** Reaching the grass consumes it (world wiring)
   and calls `feed()`. Well-fed, opposite-sex neighbours can then mate
   (`try_mate`), producing an offspring genome (Mendelian cross).
5. **Forget.** Food memory fades; eaten grass is gone, so stale sites get
   re-checked by looking again. Knowledge is never global or permanent.

Pieces still to build/wire for this: a `forage_memory` model (food sites with
freshness + nearest-when-hungry); the bin step where a surveying creature tests
the blocks in its view for grass and remembers them; the hunger→goal switch and
goal pathing; the world step that consumes a grass block and spawns offspring.

## Mating

Already modelled: each creature has a [`Sex`] (≈50/50, fixed at birth) and mates
only with a fed, opposite-sex partner. Wiring: assign each roamer its sex; when a
fed creature is near a fed opposite-sex one, run `try_mate` and spawn the child at
a parent's position with a fresh (low) energy reserve.

## Turning that looks alive

Idle wandering uses a per-creature `TurnProfile` (a detent so it doesn't
micro-vacillate, and a nonlinear response so small corrections are gentle and big
ones near-instant). Fleeing and goal pathing bypass this and turn crisply. This
is the "nonlinear turn maps stored in the brain, applied to wandering not
pathing" idea.

## Longer-horizon ecology (musings — not yet built)

A dependency-ordered sketch so these don't get built backwards:

- **Plant genetics & growth.** Leaves carry shape/colour genes. A plant's energy
  budget: **light → food**, **water → food**, food → **trunk/limb growth**.
  Colour could trade off against light capture. Growth and survival then feed
  **genetic selection** on the plants themselves — forests that adapt to their
  patch (sunlight, moisture, altitude).
- **Pollination by insects.** Bugs carry pollen between plants, mixing leaf/
  colour/shape genes — the cross operator for flora, giving diverse, drifting
  forests instead of clones. (Depends on plant genetics existing first.)
- **Fire → soil.** Fires don't just destroy: ash adds **soil food**, which plants
  draw from for growth. Couples the existing fire/weather sim into the plant
  energy budget. (Depends on the plant food/growth model.)

Build order if/when we turn to flora: plant energy/growth model (light+water→
growth) → plant genetics + selection → insect pollination (flora cross) → fire→
soil coupling. Each is a pure, testable model first, wired second — same cadence
as the fauna stack.

## Sequencing (what comes before monkeys)

A new arboreal species ("monkeys") is wanted, but it should land *after* the
groundwork so it has somewhere to live and something to do:

1. **Finish wiring the current fauna section** — give roamers `Vitality` +
   `ForageMemory` + `Sex`, wire grass-spotting-while-surveying, hunger→goal
   pathing, eating, and mate-spawning into the world. (All the models exist and
   are proven; this is the bin/world integration, likely starting with a
   roamer-tuple → `Roamer` struct refactor since the tuple is at 7 fields.)
2. **Basic plant genetics** — start the flora stack above (leaf/canopy traits,
   light+water→growth, selection), so forests vary and form **distinct
   habitats**.
3. **Monkeys** — an arboreal species suited to those varied, canopied habitats
   (climbing, foraging fruit/leaves, troop behaviour). They'll reuse the fauna
   stack (senses, steering, feeding, mating, forage memory) with arboreal
   movement on top.

Rationale: monkeys are most interesting when there are real forests with
structure to inhabit; building the habitat-shaping plant genetics first gives
them a world worth living in.

## Rest cycle (all mobs, not just dragons)

Every creature now keeps a daily rhythm, read from the sky's day phase
(0 sunrise · 0.25 noon · 0.5 sunset · 0.75 midnight) in `sim::rest_cycle`.
`daily_rest_urge(rhythm, day_phase)` returns 0 (fully active) … 1 (wants to
rest); `rest_urge(.., fatigue, chill)` layers the body's tiredness and the
cold on top so either can tip a creature into resting off-schedule, and
`wants_rest(..)` is the threshold the brain checks before settling into a
`liveliness::MotionState::Resting` pose (slack limpness + slow, deep breathing)
and suppressing its wander.

Rhythms by species (`rhythm_for` / `rhythm_for_mob`):

- **Diurnal** — birds (Späher, Flyer) and big grazers (Dinosaur): awake by day,
  fold up at night. **Bees** are diurnal too but cold-sensitive: their `chill`
  term is weighted high, so a cold afternoon sends them home before dusk.
- **Crepuscular** — general Fauna / Grazers: stir at dawn and dusk, lull through
  midday heat and the dead of night.
- **Nocturnal** — the Lauscher and ambush Seekers: rest by day, wake to hunt and
  listen in the dark.
- **Cathemeral** — dragons: a low territorial watch around the clock, with a
  gentle midday basking rise (never forced to rest by the clock alone, only by
  fatigue or injury).

The model is pure and deterministic; wiring `rest_urge` into the per-tick brain
(and a `chill` source from local temperature for bees) is the bin/sim-loop step.
