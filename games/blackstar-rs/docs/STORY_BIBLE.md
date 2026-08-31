# STORY BIBLE

## What this document is for

This is the canonical account of the fiction, and it exists to end a specific
habit: pasting film-script drafts into a chat at the start of every session to
re-establish context. The scripts are the *drafting* surface. This is the
*settled* surface. If the two disagree, one of them is out of date, and the
rule is that whichever was decided later wins and this file gets edited to say
so.

A session should be able to start with "read `docs/STORY_BIBLE.md`" and nothing
else.

## The game is the medium

Nine films were written and it is now unlikely any of them get made — game
production is the tractable path and film production is not. So this is not a
tie-in bible, and the scripts are not a production plan waiting on a green
light. **`blackstar-rs` is where this story ships**, the screenplays are the
most recent half-finished iteration of it, and half-finished is the expected
state rather than a defect.

Two things follow, and they sharpen the rule above rather than replace it:

* **When a doc and the code disagree, the code wins.** The general rule is
  "whichever was decided later", and for story the code is almost always the
  later decision, because it is the only version anyone can play.
  `content.rs` runs the missions, `archive.rs` holds the strata. A beat no
  player can reach is a note, not canon.
* **A thin beat is an invitation.** Where the fiction is unfinished, the game
  is free to be the place it gets decided — and this file then gets edited to
  match what shipped.

The surviving screenplays live in [`docs/films/`](films/README.md), with their
provenance and what is still missing recorded there. Films 5 and 6 are current;
their predecessors were a different plot that contradicted this bible and were
replaced.

**It is machine-checked.** The fenced ```` ```canon:… ```` blocks below are
cross-checked against the code tables by `archive::tests::the_story_bible_is_current`.
Add a claim to `archive.rs` without listing it here and the suite fails; list
something here that no longer exists and it fails too. That is deliberate:
documentation rots because nothing breaks when it does, and this repo already
treats content consistency as a test (`content_validate`, `lint_dead_fields`).

Prose is not checked, only ids. Keeping the prose honest is still a human job.

---

## Canon status — read this before treating anything below as fixed

| status | meaning |
|---|---|
| **LOCKED** | shipped in code and played. Changing it breaks saves or tests. |
| **SETTLED** | decided, written here, not all of it built yet. Safe to build on. |
| **PROVISIONAL** | a good answer that has not been stress-tested. Argue with it. |
| **OPEN** | named, deliberately undecided. Do not invent an answer silently. |

Most of the human-side material is SETTLED, not LOCKED — it exists as tables
and prose but very little of it has been played.

---

## The shape of the whole thing

There are two histories, and the trick is that they are the same history seen
from two ends.

- **The Transformer present.** Six films, LOCKED, in `content::FILMS`. Ships,
  space bridges, raids, the Watch, the clones. This is what the game currently
  is.
- **The human prehistory.** Twenty films, SETTLED. Secret societies on Earth
  fighting over technology none of them understand, across roughly a century.

They meet on one fact: the energy architecture that produces Dragon Pulse,
Voltage and the kinetic operative was not invented by humans, and was not
invented by the Asteri either. It is machine-native, and every biological
version is a misreading of it. That is why the human interface eats its host.

The audience is not told this. They *excavate* it — which is the design of the
whole archaeology layer (see `provenance.rs`, `archive.rs`).

### The one line the franchise is about

> **There isn't one future.**

Not a tagline. A structural commitment: every faction below has a defensible
answer to "what should civilisation become", and the story never awards the
argument to one of them.

---

## The six strata

History is layered, and the player meets the layers **out of order**. A find
from Layer 4 can surface before anything from Layer 1; the map survives that
because a claim's standing depends on corroboration, never on depth.

```canon:strata
Present
Recent
Blackstar
BlackArchive
Asteri
Machine
```

| layer | name | what it is |
|---|---|---|
| 0 | THE PRESENT | ships, bridges, raids, and who is shooting at whom |
| 1 | RECENT HISTORY | the wars and colonies this generation inherited |
| 2 | THE BLACKSTAR ERA | a god in the Earth, and the first time the two histories touched |
| 3 | THE BLACK ARCHIVE | humans held this technology first, and hid it from each other |
| 4 | ASTERI | a civilisation that had it all before, and made one arithmetic error |
| 5 | BEFORE ASTERI | the Asteri did not invent it either |

Layers 0–2 are the Transformer game. Layers 3–4 are the human franchise.
Layer 5 is where they turn out to be one story.

---

## THE TRANSFORMER ARC — six films (LOCKED)

Authored in `content::FILMS`, with missions in `content::MISSIONS`. Do not edit
the titles here without editing the table; the test will catch you.

```canon:films
DOOMSDAY
DOMINION
DESTINY
FIRST LIGHT
SOLAR
PHOTON
```

Trilogy one (DOOMSDAY / DOMINION / DESTINY) is Earth, Unicron, and the shield.
Trilogy two (FIRST LIGHT / SOLAR / PHOTON) leaves the shield and finds out how
large the cosmos is. The Watch's ceiling is the climax; `watch.rs` owns the
mechanic and `chi.rs` owns the attunement.

**The Array logs** are Blackstar's own instrument readings, gated behind
`the_ashes_court` — the mission that puts the player inside his systems. They
are the first hard evidence of Layer 5, and they *cannot corroborate
themselves*: three readings, one instrument, one source. The player needs the
Carrion wreck to make them mean anything. (Formerly `intel.rs`, now findings in
`archive.rs`.)

---

## THE HUMAN ARC — twenty films (SETTLED, mostly unbuilt)

Umbrella title: **THE BLACK ARCHIVE**. "Black" as in black-budget, black site,
black operation, and information deliberately erased. And eventually: the Black
Archive is not a database, it is a *method*.

**Tone.** 1998–2005 visual language. Not retro pastiche — the world simply has
not become visually saturated with technology yet. Warehouses, ports, casinos,
CRT monitors, black sedans, practical stunt choreography. The public world is
almost completely normal: no known superhumans, no agency admitting anything.
The audience learns the hidden world at the protagonists' speed.

The escalation is deliberately *not* superhero → alien → cosmic. It is:

> crime → conspiracy → intelligence war → technological arms race → societal
> conflict → historical mystery → existential technological threat.

| phase | films | what happens |
|---|---|---|
| I — THE BLACK OPERATIONS | 1–4 | warehouses, assassinations, stolen tech. The protagonists discover their own organisations are lying. |
| II — THE SHADOW WAR | 5–8 | they become targets. One defects. One learns his abilities were engineered on purpose. |
| III — THE OPENING | 9–12 | the technology leaks. Ordinary criminals get crude versions. Governments panic. The orbital object is detected. |
| IV — THE FIRST ARCHIVE | 13–16 | the original research is found. It is far older than anyone believed, and it is not a weapon. The object arrives. |
| V — THE BLACK WAR | 17–20 | all three societies fracture. Film 18: the machine civilisation is real. Film 19: the satellite network exposes everyone. Film 20: can humans and machines coexist openly. |

**The crossover happens late and this is load-bearing.** Transformers do not
appear before Film 17. Introducing them earlier destroys the mystery — the
audience must spend sixteen films believing this is a terrestrial story.

Nothing in this section is in code yet. There is no `HUMAN_FILMS` table, so
there is no canon block for it; when one lands, add it here and to the test.

### The three societies

None of them is the villain. All three arguments are defensible, which is the
point.

- **THE DIRECTORATE** — extraordinary technology must belong to legitimate
  governments. Centralised control, secrecy regrettable but necessary. If one
  person can drop a power grid, someone needs to be able to stop them. Their
  headquarters is offices that do not officially exist.
- **THE FREE STATE** — no government should hold technology powerful enough to
  control everyone else. Decentralised, leaks, sabotage. Some members are
  liberating humanity; some just hate governments. That difference is where the
  trouble starts.
- **THE COVENANT** — humanity cannot be trusted with power it does not
  understand, so society must be managed from above. They destroy research,
  kill scientists, manufacture wars as cover. The twist: **they do not want to
  rule.** They want to prevent discovery — and they may be the only people who
  correctly understand how dangerous the technology is.

### The powers, as code (SETTLED)

`powers.rs`. The Watch is one ability with a form selector, mastered before
the story starts — costed, limited, **not** degrading. The human interface is
the opposite, and the archaeology layer had already established what:
`human_interface_unstable`, corroborated by the autopsy file (tissue damaged
by working *correctly*, for years) and by `rotation_ledger` (four revisions of
a schedule cycling staff out sooner each time — nine years, then four).

So a human power is **a withdrawal against a body, and the balance never
refills.** Not a cooldown, not a stamina bar: a per-mission meter would make
the interface a resource system and quietly retire the fiction's central
claim.

Costs are quoted in **months of remaining service**, because the fiction
quotes them in years and the rotation ledger is a calendar. A player who can
convert a cost into "how many more jobs" can decide with it.

**A failing interface costs more** — 1.5× past 80% spent — which is the trap
the ledger describes and why the revisions kept shortening.

| who | verbs | starts at |
|---|---|---|
| Dragon Pulse | discharge, absorb | 62% spent — eleven years against a nine-year schedule, by his own line |
| Voltage | sense, discharge | 15% |
| Kinetic | surge, sense | 15% |
| The Archivist | sever, absorb | **86% — failing** |

### The Archivist (the real antagonist)

**His readout is his motive.** 1.3 years left, and one use of `unmake` — the
most expensive power in the table — takes him to 0.3. He has done the same
arithmetic on everyone else. That is the argument for destroying all of it,
stated as a number rather than a monologue, and a player running the villain
campaign is playing a man whose meter is nearly full.


Not the head of any society. A person who has spent decades collecting every
fragment, and has concluded that humanity cannot be trusted with any of it — so
he intends to destroy all of it, *including every human body modified to
interface with it*.

Which means the protagonists eventually learn: the thing that made them special
is the reason he intends to kill them. The final question is not who is
strongest. It is **should this knowledge exist at all.**

He is already in the C engine's codex, in the `appendix: hybrid protocol` entry
(`exengine/lore.c`) — a fragment addressed directly to "archivist". That
predates this document and is now canon.

### The Asteri, and the mistake

PROVISIONAL on the dates, SETTLED on the shape.

A terrestrial civilisation, thousands of years ago, that held this technology.
Not gods — brilliant people who made catastrophic errors, whose records show
failed experiments, political arguments, and researchers who warned them and
were ignored.

At the end, two artifacts were separated: one buried, one put into a long
orbit. Nobody knows why. One returns naturally. The other is deliberately
retrieved by the antagonist. **The audience will assume both were sent on
purpose. They were not.**

Their final astronomical record says `RETURN EXPECTED: 417 YEARS`. It is wrong.
The true period is 3,812 years.

**This is computed, not asserted** — see `orbits.rs`, which is a real
long-period orbit solver tested against Halley's published elements. Run
`skydribble --arch orbit`. The result:

- the difference between 417 and 3,812 years is a **0.312%** error in the
  perihelion speed
- at that precision, their own data could not exclude *never returns*

So they did not get a number wrong. They published the optimistic end of a band
that was open at the other end, and carved it in stone. That is the franchise's
thesis in one calculation: they were a hundred and four years of good
measurements away from a question that needed a thousand.

Do not restate these numbers anywhere else. Cite `orbits.rs`.

---

## The claims — the propositions the history consists of

Every one of these is something the player can come to believe, disbelieve, or
find contested. They live in `archive::CLAIMS`.

```canon:claims
bridge_predates_coalition
unicron_in_earth
convergence_was_not_first
societies_existed
facility_built_2003
continuous_earth
watchers_observed
archivist_collecting
human_interface_unstable
asteri_existed
asteri_orbital_error
two_artifacts
asteri_were_not_first
architecture_is_machine
edge_lattice
cold_war_debris
later_forgery
instrument_artefact
```

Three of these are **mundane answers to false leads** — `cold_war_debris`,
`later_forgery`, `instrument_artefact`. Chasing `satellite_fragment_17`,
`asteri_signal_beacon` or `unicron_detection_scan` is not punished and not
rewarded. It is *answered*. A universe where every glowing object means
something is a theme park.

One is **an assertion rather than a fact**: `facility_built_2003` is the
Directorate's own excavation record, and three unrelated finds put the dig in
1998, 1996 and 1994. The map reports SOURCES DISAGREE and refuses to average
them. This is the moment the player learns the official history is something
somebody chose to write.

---

## Who is playing

```canon:lenses
patrol
dragon
voltage
kinetic
archivist
```

The same evidence reads differently depending on who is holding it. The
**finding** is what is physically there and never changes; the **reading** is
what somebody said about it. A new playable character is a new column, not a
new evidence table.

- **PATROL** — the Transformer. The ship's chartroom. The voice everything is
  authored in by default.
- **DRAGON PULSE** — discipline and pulse architecture; a body that pays for
  what it does.
- **VOLTAGE** — charm and electromagnetics; reads rooms and power draw first.
- **KINETIC** — reads movement, distance, and what left.
- **THE ARCHIVIST** — the villain campaign. He already knows; he is checking
  whether you do. (A nod to War for Cybertron / Fall of Cybertron letting you
  run the other side; see `docs/WFC_FOC_RESEARCH.md` in the C tree.)

Reading coverage is **deliberately partial**. Four voices across every finding
would be a hundred and forty pieces of writing and most would be filler. A lens
with nothing to add falls back to the object's own description, and that is a
tested behaviour, not a gap.

---

## Where the fiction lives in code

The governing rule, and the reason the architecture looks like this:

> **Never make a code breadcrumb exist solely because it is a lore breadcrumb.**

If you need an orbital solver, write a good orbital solver. If you need
artifact provenance, write a good provenance system. Then attach the story to
them. The lore is not hidden in the code; it is embedded in the architecture
because the architecture describes the world.

| story element | where it actually lives |
|---|---|
| the six films, missions, troops | `content.rs` |
| the historical claims and evidence | `archive.rs` |
| how belief is computed from evidence | `provenance.rs` (content-free) |
| the Asteri orbital error | `orbits.rs` |
| the codex the player reads | `exengine/lore.c` + `EX_GS_CODEX` (C engine) |
| the ship's archaeological map | `stations.rs` → `Station::Chartroom` |
| the C engine's copy of the tables | `exengine/archive_gen.h` (**generated**) |
| destinations and their flavour | `patrol.rs` |
| the Watch and its ceiling | `watch.rs`, `chi.rs` |
| clones | `clones.rs` |

The C header is generated by `skydribble --arch emit-c`. Never hand-edit it.

### The mystery anchor

A main-menu dial (`cfg story_anchor`, 0–1) for how much of this the game hands
you already believing. It is **not** a tutorial slider: received knowledge
enters the inference as one contradictable source, and the received account
includes the official record — which is wrong. So HOMEY gives you a lie to
catch and FRESH gives you a world with nothing yet to disbelieve. Four stops:
FRESH / SPARSE / FAMILIAR / HOMEY. Default 0.45 — you know what you lived
through and nothing past it.

Whether 0.45 is right is a question for someone with a screen. It is a
placeholder.

---

## THE CAMPAIGN'S SHAPE (SETTLED)

Not a mystery game. Not a shooter with lore bolted on.

**The campaign is Call of Duty's structure, and then it inverts.**

Authored set-piece missions introduce an extended mechanic set one piece at a
time, each in a place built to make that one thing legible — the fold in a
corridor with nothing else to do, flight when the shore tears open and the
ground stops being an option. That shape is well understood and it works: fun
dialogue over escalating action, scaling up to side-by-side Transformers, the
Watch-bearer and the clones.

Then the graduation runs the other way. A Call of Duty campaign releases you
into multiplayer, which is a **subset** — smaller maps, fewer verbs, most of
the campaign's toys gone. Here the campaign releases you into a **strict
superset**: Destiny's patrol-and-raid loop, except the destinations are
procedurally generated, so it is closer to Ark or Rust's infinite worlds than
to Destiny's fixed handful. Everything you were taught still works, in more
places, forever.

`the_far_gate` (mission 10, Film 4) is the structural hinge. Before it, authored
corridors. After it, the procedural sky. Most of the kit is deliberately taught
before that mission, or the funnel opens onto a player who cannot use it.

**This is enforced, not asserted.** `curriculum.rs` is the ledger — every
mechanic, the one mission that introduces it, and where it can be used
afterwards — and its lints hold the shape:

- every mechanic is introduced by **exactly one** mission (re-tutorialising is
  the specific sin the table exists to prevent)
- the table is in teaching order, monotone with mission order
- **nothing is campaign-only.** A row that fails this is either a bug or an
  admission that the campaign is a demo
- raids may *deepen* a mechanic but never own one, because a verb you need five
  other people for is not a verb the game has
- the kit accumulates and never shrinks across the mission list

One row is honestly marked unbuilt. `combine` is reachable now (`--combine`);
`clinch` is not, and is deliberately left alone — its brief is that grabs must
not feel random, and that is not verifiable without playing it. A test pins
the unbuilt list so it cannot drift from `docs/PLAN.md`.

### What this means for the human campaign

Same shape, different verbs. Hitman-meets-Clancy is the movement and infiltration
register rather than a genre change: authored missions that teach a human kit —
pulse architecture, electromagnetics, kinetic reading — which then has to survive
contact with the Transformer-scale open world. The human side is not a smaller
game bolted on; it is the narrow end of the same funnel.

The alien interaction arrives through the clones, and the Watch-bearer is where
the two kits genuinely overlap in play rather than only in story.

---

## THE RED DWARF STRIKE (SETTLED, and one part decided itself)

Billy does not release fully. He holds the charge short of a nova — dims the
star toward a red dwarf — and Photon Man **aims** it, away from the airspace
the arriving Clone worlds are leaping into. Restraint is not his instinct,
which is why doing it once, in front of everyone, counts as his completed arc.

`finale.rs` states the fiction through what it REFUSES:

- **a true nova** — nothing in the radius survives, and the arriving allies
  are standing in it
- **too small a draw** — a shrug does not touch a fleet
- **nobody aiming it** — without Photon Man the held draw goes out broad

### The endings are not symmetric, and nobody decided that

The treatment leaves Teleport and NewSun deliberately open. The mechanics do
not: **NewSun is always available, and Teleport is refused straight after the
strike.** Photon Man costs almost the whole resolve pool — 5% left against a
16% shift — so the leap only becomes possible if the Morpher survives about
**two more seconds**.

Nobody wrote that rule. It falls out of two costs in `watch.rs`, and it is
better than a decision: the ending depends on whether the scene gives him a
beat. NewSun is what happens if it does not. A test pins the two-second gap,
because a tweak to either cost would silently rewrite the ending.

---

## LOS ANGELES — one city, two eras (SETTLED)

The strongest overlap between the two campaigns is physical, not thematic.
The human films are set in LA in the archive era. `the_beach_opens` (Film 1,
mission 4) is set there in the present, when the shore tears open under
**Aiden** — a kid with a skateboard who does not run, and starts pulling people
up the dunes. Same coastline, two casts, fifteen years apart.

So a place in `groundside.rs` carries **two states**, and the test suite
refuses a place that reads the same in both. Standing somewhere in one campaign
and recognising it from the other is the archaeology theme made spatial, and it
costs nothing extra because the evidence was already written.

### The thing the geography explains

`farmhouse_photograph`, `trench_excavation_scar` and `thermal_plate` all
describe THE RIDGE, and together they date the Directorate's dig to 1998, 1996
and 1994 against an official record of 2003. The ridge overlooks the shore
where Unicron's fissure opens.

Which answers something the tables raised and never resolved: **the record is
not falsified to hide a building. It is falsified because somebody was digging
toward the thing in the mantle a decade early**, and 2003 is the smallest lie
that stops anyone asking what they were digging for.

The game never says this. `RIDGE_OVERLOOKS_BEACH` keeps the two places adjacent
by test; the inference is the player's.

### The home base — present only (SETTLED, and this was a correction)

This started as one building held by both casts, and the source overruled it.
The scene the human team is built out of is explicit: *old car, three people
inside, **no secret base**, no holograms.* Chris with a foot on the dashboard,
Pietro looking out the window, Jackie driving, nobody completely trusting
anyone yet. A safehouse with a wall of monitors is exactly what that scene
refuses, and it is the scene where the audience is meant to fall in love with
the team before the mythology gets enormous.

So **the archive era has no home base**, and a test asserts it stays that way —
if one ever appears, that scene is gone and somebody should have to argue for
it.

The **present** keeps one. The bunker is Aiden's foothold, and a fixed
GTA-style home is right for him: he is a kid who stayed at a fissure, not an
operative with a network. Its Transformer-side twin is the ship, which is why
`Station::Chartroom` and the bunker do the same job.

The asymmetry is the design. What the archive era has instead is below.

### The nine places

`the_bunker` (home), `the_ridge`, `the_beach`, `the_farmhouse`,
`the_ground_station`, `the_museum`, `the_port`, `the_clinic`, `the_observatory`.

Every Earth finding now has an address — a lint fails on any that only says
"earth" — and nothing is held in two places at once. Walk it with
`--arch city [place] [then|now]`.

---

## RAIDS CARRY HISTORY (SETTLED)

A raid should teach something about the universe through its mechanics rather
than only pay out. The roster's own frame already supports this — the nineteen
wardens are described as *"wardens, judges, archivists, seals"*, a distributed
lock rather than a monster list.

`judgementor` (Executor of Empires) is the first one wired: *ancient defendants
still orbit as asteroid belts, and one verdict was never delivered.* Clearing
it opens `undelivered_verdict` — a docket left open on a defendant named in a
script the machine has no entry for. It was trying a civilisation older than
its own jurisdiction. That is Layer 5 evidence reachable nowhere else, and a
test asserts it does not share a source with any campaign finding, so it
genuinely adds rather than duplicating.

**Gate vocabulary.** `Finding::gate` is opaque to the engine; `archive::gate_open`
is the content layer's reading of it. A bare id is a campaign mission (so every
pre-existing row keeps working untouched); `warden:<id>` is a raid clear. A
prefix rather than a second field, because a gate is one precondition with
several kinds and two nullable fields invite the state where both are set.

---

## TRADECRAFT — the archive era's method (SETTLED)

No base. A car you stage out of, a **case** you carry, and a scattered set of
ways to get information to whoever you answer to. Period-correct for
1998–2005, and it turned out to be a better mechanic than a base for one
reason:

**Finding stops being free.** In the ship campaign, scanning something puts it
on the map. Here it only puts it in the case. Nothing reaches the
archaeological map until it is RELAYED, and every relay costs something —
*reach* (a payphone call is one thing, a hideout is an afternoon with a
photocopier), *exposure* (who else hears), and *fidelity* (a contact retelling
a serial number is not a photocopy of it).

So the case is an inventory decision rather than a bag: you are carrying
evidence that is not safe, and the map does not know it yet. That is the Hitman
half of the register — the problem is rarely the shooting, it is walking out
with the thing.

### Exposure is not a penalty, it is a plot

A loud relay tells rival societies what you found. **That is how the official
record comes to be written by somebody who was not there** — and the payoff is
already built: `facility_built_2003` is a claim the Directorate *asserted*,
contradicted by three dig dates. Exposure is the mechanism by which such
records exist. Reporting is a move in an argument, not a save button.

| relay | channel | reach | exposure | whose |
|---|---|---|---|---|
| the car | staging only | 0 | none | anyone |
| port payphone | a call | 1 | low | anyone |
| museum dead drop | serviced Thursdays | 3 | none | Directorate |
| newsstand contact | a person, lossy | 1 | high | Free State |
| ridge lamp | morse off the high ground | 2 | total | anyone |
| observatory room | a photocopier and a lock | 8 | low | Covenant |

The rooftop lamp is the interesting one: it reaches everyone who matters and
everyone who does not. The ridge is also where the excavation record was
falsified, which is not a coincidence anybody needs to point out.

### This is the human campaign's first mechanic

`curriculum.rs` is saturated for the Transformer arc — fifteen missions, fifteen
mechanics, and its lint refuses a sixteenth without a new mission. Tradecraft
belongs to the human ledger, which does not exist yet. When it does, this is
row one, and it must obey the same rule: **nothing is campaign-only.**

---

## THE CODEX (SETTLED — this was an open question)

Fifty-four entries, carried out of the C engine's `lore.c` into `codex.rs`
before that tree is retired. Original writing on the six-film arc, and
**source rather than derivative**: the Archivist is addressed directly in
`appendix: hybrid protocol`, which predates everything in `archive.rs`.

The open question was whether the codex should become something you excavate,
dimming until earned, or stay a reference. **Reference** — and the third
option, which I did not see when I first wrote the question, is that the two
are not exclusive. Entries are ANNOTATED, never gated: five of the
fifty-four name a claim, and when you read one the codex prints what the
evidence currently says beside what the entry says. The entry is readable
either way.

Five deliberately, with a test refusing more than a quarter. An annotation
that fires on everything is decoration, and the case worth building for is
the single entry the evidence ends up disputing.

`--codex [entry]`.

---

## THE BOTS (SETTLED)

259 placed models, and **most of them should not have lore.** A world with
259 named personalities has no crowd in it, and the crowd is what makes the
named ones legible. `tools/meta/bots.json`'s own documentation worries that
178 of them "read as their class, which is placement metadata wearing a
costume" — the tempting fix is 178 more hooks and it is the wrong one.

They are a CENSUS instead (`census.rs`, `--roster`). Individuals mostly mean
nothing; the distribution means a great deal, because the placement rules were
written against the film material's geography and reading them back gives
statements nobody typed.

- **All 56 unrecorded shapes are on Cybertron, and every one is Corrupted.**
  Their shared hook — *"a shape the coalition has no record of"* — is already
  a provenance statement. `substrate_sample` says from the other direction
  that Cybertron's architecture is older than its own record of inventing it.
- **All 46 on Earth are Coalition, and all road vehicles.** Cars, trucks,
  racers, a few helicopters. Nothing that could not be parked. Concealment as
  a demographic fact, and the first thing a human investigator notices.
- **Ten of eleven worlds are monocultures.** Only the outer rim is contested,
  which is why the boundary is where the story keeps happening.

The census reaches the map as `Bearing::Supports` from ONE source. A headcount
is a real observation, unlike Pale Vector's testimony which only ever
`Mentions` — but it is still one body of evidence, so it lands a claim at
UNKNOWN and waits for a dig. Counting bots is suggestive; it is not evidence
twice over.

---

## OPEN questions — do not answer these silently

- **The human campaign's UI surface.** The SHAPE is settled (above); the screens
  are not. `Medium::Terminal` marks which evidence belongs on world terminals
  and `Lens` supplies the voice, but nothing renders them. Note that the
  Division-style terminal is a *reading* surface inside a CoD-shaped mission —
  those are compatible, and the terminal must never stop the mission dead.
- **The human curriculum.** `curriculum.rs` covers the Transformer kit only.
  The human mechanic ledger does not exist yet, and it needs the same lints —
  above all "nothing is campaign-only", which is harder to honour when the
  open world is Transformer-scale and the character is a person.
- **Whether Layer 0–2 claims should be knowable at all**, or whether the map
  should be reserved for what is *below* the present. The anchor makes this
  adjustable rather than deciding it.

---

## Editing rules

1. Change the code table and this file in the same commit. The test enforces
   it.
2. Numbers that can be computed do not get restated here. Cite the module.
3. Mark new material with a status. PROVISIONAL is not an admission of
   weakness; an unmarked guess that later reads as canon is much worse.
4. Original prose only. The film-script drafts are reference, and their text is
   not reproduced here or in `content.rs`, `lore.c`, or `archive.rs` — the
   house style is our own terse voice, paraphrasing beats.
