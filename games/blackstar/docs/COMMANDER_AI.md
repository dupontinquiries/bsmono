# Commander AI -- doctrine FSMs now, LLM personalities later

A layered command architecture. The whole point: **deterministic FSMs and the
future LLMs (Blackstar, Prime) speak the SAME command vocabulary** -- the
`combat_json` verbs -- so they're interchangeable behind one interface and the
work compounds instead of being thrown away.

```
  military doctrine (Sun Tzu, Giles 1910, public domain)
        │  distilled, NOT quoted
        ▼
  content/commanders.json     -- archetype corpus (params + triggers + flavor)
        │
        ▼
  commander_ai.{h,c}          -- deterministic doctrine FSMs  [DONE]
        │  emits ex_cj_command_t
        ▼
  combat_json.{h,c}           -- the one command vocabulary   [DONE]
        ▲  same ex_cj_command_t
        │
  (future) LLM brains         -- Blackstar (moods), Prime (roster-aware)
```

## Sourcing + copyright stance

The doctrine is drawn from Sun Tzu's *Art of War* in the **public-domain 1910
Lionel Giles translation** (Project Gutenberg / Standard Ebooks / Wikisource).
We do **not** store raw passages or use the copyrighted modern translations
(Griffith, Delphi). Raw quotes make flavor, not behavior -- so each archetype is
the doctrine **distilled into parameters + tactical triggers**, with at most a
short attributed Giles fragment as an optional in-game callout (`flavor`).

## The archetypes (content/commanders.json)

Each distills one or more classical principles into a parameter set
(`aggression`, `caution`, `encircle_bias`, `regroup_bias`) and an FSM policy:

| archetype    | doctrine                       | signature behavior                    |
|--------------|--------------------------------|---------------------------------------|
| `aggressor`  | concentrate force + tempo      | presses/focuses relentlessly          |
| `surrounder` | envelop with numbers           | spreads to encircle when 3+ alive     |
| `skirmisher` | evade, hit-and-run             | harasses, disengages when hurt        |
| `baiter`     | feign weakness, then collapse  | holds bait; springs to focus on press |
| `warden`     | cohesion, hold the line        | disciplined standoff; rallies if hurt |

The JSON params mirror the C built-ins exactly (`ex_cmd_doctrine_for`), so the
engine has sane defaults with or without the content file; a test guards that
they stay in sync.

## The FSM (commander_ai.c)

`ex_commander_think` runs on a decision cadence (~1.6/s), surveys its own squad
in the snapshot (alive count, losses, avg hp, centroid), picks a posture
(PROBE / PRESS / ENCIRCLE / FALLBACK / RALLY) from doctrine + battlefield, and
emits the matching `ex_cj_command_t`:

- PRESS -> `focus`, ENCIRCLE -> `flock_spread`, FALLBACK -> `retreat`,
  RALLY -> `flock_to` (squad centroid), PROBE -> `hold`.

A commander with a dead leader or wiped squad emits nothing. Ties into the
existing scatter mechanic: kill the commander and its troops scatter
(encounter.c); the FSM simply stops issuing orders for that dead squad.

## The infected hive -- a sneakier brain (hive_ai.c)

The Corrupted get their OWN engine, distinct in KIND from the military
commanders. A commander issues open, top-down orders; the hive is patient and
indirect -- swarm intelligence, not doctrine:

- **CRAWL & organize** -- below critical mass the infected stay dispersed and
  creep, massing quietly (`hold`) so a lone scout doesn't give the swarm away.
- **WAGGLE DANCE** -- a scout that gets close to prey recruits nearby infected
  toward that point (`flock_to` the scout). Excitement spreads stigmergically
  through proximity, not a broadcast order.
- **FEED** -- once enough answer the dance (critical mass), the swarm converges
  (`focus`). Thinned below mass -> disperses back to crawl.

Same `ex_cj_command_t` vocabulary as the commanders + future LLMs, so all brains
are interchangeable -- the hive just reaches those commands by a sneakier policy.
A future LLM Blackstar could drive a hive with mood the same way it'd drive a
commander.

## Roadmap to the LLM layers

Because FSM and LLM both emit `ex_cj_command_t`, the LLM slots in as a drop-in
alternative "brain" per commander -- or per faction:

- **Blackstar (moods):** an LLM commander whose archetype + a mood axis
  (amused / enraged / cautious) shifts which commands it favors, reacting to the
  player's engagement signals (aggression, pressure, kills). Same snapshot in,
  same commands out -- just nuanced, with in-character reasoning.
- **Prime (roster-aware):** an LLM that reads a top-players roster (likes /
  dislikes) and leans on more dramatic, life-preserving choices -- pulling allies
  back (`ally_follow`), rallying (`flock_to`), refusing hopeless pushes. Reads
  the same snapshot, adds `retreat`/`ally_follow` weighted by who's on the field.
- **Team/squad JSON actions:** both already covered -- commands target a squad by
  id/zone, so an LLM can coordinate multiple squads by emitting several commands.

## Wiring status

- DONE + tested: corpus, FSMs, the shared command type, the scatter tie-in.
- NOT yet wired to live spawns: which encounters field commanded squads, and
  giving each commander a brain instance, is the integration pass (same
  deliberate choice as the commander-spawn work). `ex_commander_think` output
  applies through the same path the LLM orders will: translate `focus`/`hold`/
  `retreat` into zone standoff changes, `flock_*` into rally points over the
  boid separation, `ally_follow` into `g_ally.mode`.
```
