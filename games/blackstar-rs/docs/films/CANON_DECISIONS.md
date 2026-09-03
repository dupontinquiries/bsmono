# Canon decisions — the Watch, the Lattice, the prologue

**Status: settled unless marked OPEN.** These are decisions, not proposals.
`SCREENPLAY_TO_GAME_INTEGRATION.md` is a proposal and says so; this file is
not. Where something is undecided it says OPEN and says what the branches are,
so a session can pick one deliberately instead of inventing quietly.

The rule from `README.md` in this directory still holds and this file is
subordinate to it: **where a doc and the code disagree, the CODE is canon.**
Every decision below names the shipped code it rests on. If that code has
moved, the code won.

---

## 1. The Watch is a receiver, not an engine

The authority over matter lives in a machine on the makers' world, barren now,
and reaches the Watch down a quantum tunnel. The Watch is an interface.

Rests on: `watch.rs` (eleven mapped forms, a twelfth that is not chosen),
`link.rs` (tethered/severed, reservoir, fare home).

Consequences that are also settled:

* **ICE is the carrier, not a favourite.** `watch.rs` already makes ICE the
  only form that opens `Verb::Blink`. That is not a bonus attached to a form;
  it is the tunnel showing through. The bearer found the Watch in the ice clone
  archives and is himself an ice clone because the pairing needed a body
  already on that channel.
* **Two modes, one device, opposite aesthetics.** Tethered: a dial, a sapphire,
  fingers rippling into another material, then the wrist, then the arm.
  Unhurried, because the machine is doing the work. Severed: a stored
  superliquid and a contained ring of energy, crude and bright and finite.
  When the Watch stops being graceful, he is alone. This is the player-facing
  tell for `Tether::Severed` and should never be conveyed by a HUD label alone.
* **The tether is quantum-gravitational, not spark resonance.** This matters
  because `corruption.rs` builds a mechanic on the Corrupted registering
  neither humans nor dead machinery, and names the watch-bearer as structurally
  safe from them. Different channel, so the invariant holds. A stranded bearer
  in a non-native form is still not carrying a spark and is still invisible to
  them. **Do not** give any form a spark without treating it as a major
  decision that reopens `corruption.rs`.

## 2. The cost is specificity, and nothing is ever stored

The Watch does not copy, save, upload or back up a mind. In a form, the
bearer's mind is being emulated over a body that is not his, and what returns
is a translation. Translations lose.

* What survives a shift: the facts. He knows what happened, accurately, in
  order.
* What does not survive: the first person. He remembers it the way you remember
  something you were told. **Suggested rendering, cheap:** log/codex entries
  written during a long shift render in third person. No stat, no debuff.
* `SHIFT_COST` (0.16) is a translation out and back. `PHOTON_COST` (0.95) is
  the form furthest from any human substrate, which is why the round trip
  nearly fails and why he is under for three days.

**This idea pulls toward storage constantly. Resist it.** Storage makes the
question *is the copy me*, which is answerable and heavily precedented.
Emulation makes it *how much of me is left*, which has no answer, no
experiment, and no control case — the previous bearers are gone.

* **Previous bearers: yes.** They existed, all of them ended, and what the
  Watch carries forward is the specificity it took from them. Anomalies near it
  are bleed from people it already spent. This is the inheritance corner of the
  three-technology idea, arrived at through our own mechanics.
* **Forms have dispositions, not personalities.** `clones.rs` derives seven
  natures as principled mutations of one anatomy. What bleeds through is a
  bias — ICE waits, FIRE commits early — not a passenger. Eleven characters is
  a content cost we are not paying, and possession is a different genre. This
  is also the quiet reason he keeps returning to ICE: it is the one whose
  instincts he cannot tell apart from his own.
* **OPEN — the thermal gradient.** The operator's own reasoning for why ice
  clones pair with the Watch is that cold is nearest the quantum world, and it
  is close enough to real physics to say in-game without hedging:
  decoherence is driven by thermal noise, which is why real quantum machines
  sit at millikelvin. It makes the pairing necessary rather than lucky — a
  warm body is a noisy receiver, and the Watch bonded to the only kind of body
  that could hold the channel.

  The consequence, if taken: the eleven forms are not thermally equal, and
  FIRE/MAGMA are the far end. That cost belongs on the DEVICE side —
  `link.rs`'s reservoir, which pays for holding the channel open — and must
  stay away from `watch.rs`'s flat `SHIFT_COST`, because the man's cost is
  translation and translation costs the same in both directions.

  If built, do it as WHICH FORMS THE FARE CAN BE SPENT FROM, not as fractional
  charges: from ICE the fare is one, from FIRE it takes two. The reservoir is
  deliberately a small integer you can hold in your head; a multiplier turns
  it into a bar.

  It also retro-justifies `link.rs` refusing Photon Man off the tether. That
  was written so the ceiling could not come out of a tank; under this rule
  Photon is the thermal extreme, the least coherent thing the Watch can do,
  and therefore precisely the form that cannot be held without the machine.

  **Do not** extend this to aging. `chi.rs` makes the clone races' longevity a
  species trait; making it a coherence consequence hands someone a lever.

* **OPEN:** the ledger. Specificity loss wants a monotone counter alongside
  resolve, because resolve regenerates (`REGEN_PER_SEC`) and this must not. It
  is deliberately NOT in `link.rs` — one idea per module. Constraint on
  whoever builds it: it must cost detail, never power. A debuff turns it into a
  resource to optimise; loss of detail cannot be optimised around.

## 3. The fare home

Severed, the reservoir spends down to one charge, and the last one only buys
the return to the native form. The question is not "how many can I spend", it
is "can I still get back".

Stranding — dry, in a form that is not home — is **playable, not a loss**. A
failure state everyone successfully avoids is a rule nobody experiences. The
way out is diegetic: restore the tunnel, or reach something that can refill the
reservoir. Two different missions; `link.rs::reconnect` deliberately does not
refill, because signal is not fuel.

What stranding costs is not combat power — stuck in ICE is not even a penalty.
It costs the human campaign. `tradecraft.rs` is built on a man being
unremarkable enough to work a payphone, a newsstand, a dead drop, and none of
that is available to someone who cannot turn back. It also inverts the party:
normally the human is the fragile one being covered; here the Morpher is what
has to be walked home and Billy leads.

Keep stranding clear of Photon Man, which already has a defined consequence.

## 4. The Lattice (do not call it the Cube)

The vast perfect solid is ours; the name is not. **The Lattice** — the word is
already the codebase's own (`compact.rs`: matter goes somewhere on the way, and
the somewhere is a lattice), so the name describes the silhouette honestly
instead of borrowing recognition. **The Anvil** is its closed/defensive state.

* **Closed = perfect, invulnerable, inert.** It must open to exert authority
  over anything. Armour and agency are the same resource; the telegraph is
  geometry, not a wind-up animation. Same logic as `compact.rs`'s curve — the
  endpoints are untouched, everything happens mid-fold.
* **Striking the closed Anvil reshapes the attacker.** An anvil does not move
  when struck; it shapes what strikes it. `corruption.rs` already has the
  mechanic (continuous transformation, targeted hits missing because the
  component shifted). Players will try brute force on the first pull. That is
  the fight's first lesson.
* **Damage is dispossession, not HP.** What you take is cells it no longer
  governs. Each fragment prised out is loot AND permanent visible subtraction
  from the boss body. Avoids inventing a health bar for a geometric absolute.
* **The bearer's role falls out of canon.** During the closed phase every bot
  is a target and the human is not (`corruption.rs`, spark resonance). That is
  a real reason for a human to be in a robot raid.
* **Voxel grammar, and it is a tell.** A bot's local voxel authority is
  bounded, which is why it must fold to get within reach of its target form and
  why forms are few and fixed. The Lattice's authority is unbounded — it
  changes a thing without the thing folding. `compact.rs::Style::Grow` already
  renders the signature (mass reading as added rather than rearranged);
  `Style::Shrink` is what a bot does on its own.

**Fragments of the Lattice** are the raid artifact: a fragment grants
unbounded local voxel authority, i.e. `Grow`'s curve held at peak instead of
relaxing to zero.

**Cost to know before building it:** `compact.rs` is safe to layer onto the
shipped morph precisely because `c(t)` returns to 0 at both ends and a body at
rest is bit-identical to what it was. Sustaining the peak gives that up. Super
mode therefore wants to be a **separate state**, not a parameter on the fold.

**Super bodies are derived, not authored.** Follow `clones.rs`: one measured
silhouette, eight height-bands of mass fraction plus width and depth, others
generated as principled mutations with a test holding each to the reference.
Two costs that are the actual work: non-uniform band scaling changes skeleton
proportions, so animation needs **retargeting**, not a scale multiplier; and
sustaining a per-vertex displacement field for a raid's duration is a different
budget from a half-second fold — **measure on ARM before the design hardens
around everyone popping super in the last phase.**

**Raid slotting.** SYNC is the right mechanic (plates needing units together =
the solid only opens when matter outside it is arranged correctly), and
`raidgen.rs` derives mechanics from lore by design. **OPEN, and it has a
cost:** `raidgen.rs` says the finale is the boss's own arena and that "SYNC
finishes at `unicron`" falls out for free because every SYNC boss's arena is
`unicron` — a roster fact `raidcon` tests. Finishing at `unicron` fits the
Earth-remnant thread and needs no changes. Anywhere else breaks that test.
Phases: The Approach → The Gauntlet → **The Anvil**.

## 5. The thing under Earth

What the makers left is not a mind and not a weapon. It is **unbounded
transformation authority** — the same principle as the Watch, without the
governor. `corruption.rs` already says the remnant hijacks a transformation
protocol and reissues it forever with no defined endpoint. The Watch is the
governed twin: eleven mapped forms, costs, a ceiling. Photon Man is the
governor coming off for forty seconds; the thing under Earth is what happens
when nothing puts you under.

The bearer is invisible to it and carries its governed twin. That is why he can
approach it and nobody else can.

**The replica.** Someone else tried to build one and got it wrong — a rod and
wiring up the arm into the skull, surgical, invasive, period-correct for
`tradecraft.rs`'s 1998–2005 register. It is wrong *because* its builders
believed the Watch stores a brain. It doesn't. So the replica is a monument to
a misunderstanding — the archive's own theme (the official record written by
someone who wasn't there) in hardware. The gore is evidence of an error, not
shock. Jhiaxus' work is the natural site: a room, a chair, cabling, and
something that has been mid-transformation for years.

**Do not state the genealogy as fact in the fiction.** `archive.rs` is built on
corroboration and on false leads resolving to nothing (`unicron_detection_scan`
is an instrument artefact). "The Corrupted are failed bearers" is exactly the
grand unification that module distrusts. Enter it as a claim, single-sourced,
unable to corroborate itself — the shape of `array_reading_one/two/three`
reaching UNKNOWN and stopping. Players who assemble it will believe it harder.

## 6. The prologue

The "Consequence" monologue exists complete in exactly one place in the source
archive: **`🎬 TRANSFORMERS BLACKSTAR.md`**. It is the only copy that reaches
the payoff. The two most polished-looking variants in
`TRANSFORMERS_DOOMSDAY+DOMINION+DESTINY.md` (the "Cold, Calculated Tone"
version and the storyboard VO) both truncate at "We call it coincidence." Work
from the complete one.

**Speaker: the Morpher, unrevealed.** Not a leader. The cosmological middle is
speaker-neutral. The payoff gains: for a leader, coincidence-versus-consequence
is a warning about history; for him it is about the pairing — the Watch found
him, that was not an accident, and something is owed. Same words, two readings,
both complete on their own.

* **The line that changes is the one that had to change anyway.** "We once
  believed ourselves eternal. Cybertron stood for millions of cycles, and we
  mistook endurance for destiny." Give it to the ice clones: `chi.rs` states
  they do not age as the rest of the cast does, that a native who has stood
  watch a thousand years is not old by their own reckoning. Endurance mistaken
  for destiny is their species condition, and from a man now spending himself
  in increments it stops being a metaphor. This is the only line that gets more
  precise when the licensed noun comes out.
* **Keep the ledger out of the text.** Anything within reach of *am I still
  myself* is a non-sequitur to a first-time player and spends the reveal in
  minute one. The source's own direction — cold and calculated until the end,
  when he relays his feelings about consequence — already carries it, and now
  re-reads: the flatness is not composure, it is what is left of him. The
  direction survives verbatim.
* **Unfixed in time.** The VO speaks from an unspecified later present, so he
  can know everything without contradicting Film 4 introducing him as already a
  master. Naming the Hijacks at a distance is fine; implying he has already
  fought them anchors the VO and reopens the chronology.
* **One verbatim echo, and only one.** "It believes in consequence," said again
  at the Photon moment in Film 6. Minute one it was about the universe; there
  it is about him. **Do not** hang the echo on the Sun Gun exchange — that beat
  is Billy's and `README.md` names it as the sequence to protect.

## 6b. The hub is Pluto, and the menu is the hub

Not a new location — the game's front door already is one. `casting.rs` uses
`the_pluto_line` as `introduced_by` for the first cast member, so it is the
opening mission, and `archive.rs` already carries a `pluto` site with a
boundary marker set into the ice (`pluto_marker`) and a Pluto outpost's last
log buried under a reactor-fault report (`whiteout_incident`). The prologue's
hard cut on "consequence" lands on a place the campaign already starts at.

* **The menu IS the hub, live.** First load drops the player into the outer
  rim looking like a menu, and the calm is false. This is a deliberate
  jarring, and it has an engine consequence: there is no separate menu render
  path, so the hub scene is running at menu time. That makes the menu another
  consumer of the world, and it is therefore the strongest argument for the
  menu going through `worldhost::WorldHost` rather than reaching into `World`
  — otherwise the first thing that loads is also the thing most tangled with
  the engine we are trying to make swappable.
* **Returning to the marker is the payoff, and the mechanic exists.**
  `pluto_marker` is a physical object at a location that `archive.rs` can hang
  a finding on. Standing there much later, with a crew and a ship, and getting
  something that was not available the first time is that module working as
  designed. Do not build a new system for it.

**Engineering flag, and it is not optional.** `chi.rs` maps
frostline/kuiper/pluto to Ice, and KUIPER is a FIELD world — the one with the
phantom floor. `footing.rs`'s `the_phantom_floor_is_in_the_sdf_itself` is a
characterisation test asserting that a march down through open field hits an
invisible near-flat plane, because `World::sdf`'s first term IS that plane.
Cheap to render is true; open and floorless is not, yet. A serene hub where
you drift and look out is exactly where that floor shows up as bodies resting
on nothing. Removing it means giving the SDF a terrain toggle — a change to
the one function physics, collision and the raymarcher all share. Decide it
before the hub art is built around it, not after.

## 6c. Two readings, and only one echo

"Worlds are born. Worlds perish. Stars ignite… and fade." reads literally and
completely on first hearing. The second reading — worlds as people's worlds,
stars as souls — must be AVAILABLE, never signposted. Nothing is withheld on
the first pass; that is what makes the line work at all.

The way the second reading arrives is one small later use of the same word in
the personal sense: someone says *world* meaning a person's world, or *went
out* about a star and about a man in the same scene. Once. The opening then
re-reads on its own and nobody has to be told.

**One echo, not two.** Same discipline as "it believes in consequence"
returning at the Photon moment (§6). A second echo makes it a motif and kills
it.

## 6d. The Watch's origin world — free, and partly prepared

**Not in the source archive.** All 13 screenplay files were grepped: there is
no watch planet, no sphere, no green structure, no conduit and no recharge
anywhere in them. `TF_QUANTUM_GRAVITY_LEAP.md` establishes the Ice-only leap
and the faithful-but-not-native reproduction — which `watch.rs` implements —
and stops there. The device's origin was never written, so nothing has to be
reconciled.

Two slots already exist in shipped code and should be used rather than
duplicated:

* **`glaciem`** — an ice clone OUTPOST, not the homeworld. `archive.rs`'s
  section order runs LUNA → EUROPA → IO → GLACIEM → PLUTO → OUTER RIM, which
  is solar-system order by distance, so Glaciem is an in-system body between
  the Jovian moons and Pluto. `chi.rs` already permits this: each people
  "mostly keeps to its own world **or outpost**."

  That resolves the crowding problem without inventing anything. Three
  separate places, none jostling: the ice clone HOMEWORLD (inhabited, outside
  the system, still unwritten and free), GLACIEM (a small in-system outpost
  with one vault — far from everything by default, because that is what an
  outpost is), and the MAKERS' WORLD (barren, `uncharted`). The separation is
  by function, not distance: a homeworld has a society, an outpost has a store
  room.

  The vault finding — *opened cleanly and emptied. the inventory plate is
  still bolted on. every line is struck through in the same hand, and the hand
  is older than the vault* — is where he found the Watch. **The reading to
  take: whatever was left behind was never inventoried.** The Archivist
  (`archivist_collecting`) emptied it and took everything they recognised; the
  Watch stayed because they did not recognise it. That is a far better reason
  than luck, and it makes the Archivist's own thoroughness the reason the
  Morpher has it at all.

  **SETTLED: the outpost is STAFFED, and he walked out with it in front of
  them.** (This overrides an earlier note in this file that leaned abandoned.)
  Nobody gave it to him and he did not take it — it ANSWERED him, where they
  could see. `patrol.rs` describes the place itself: *the ice-clones grow slow
  in lattice pools; nothing is hurried, nothing thrown away.* A culture that
  discards nothing also does not hoard, because both decide an object's worth
  in advance; what it has instead is a sense that a thing belongs where it is
  useful. A people with no procedure for an object that has chosen does the
  only thing consistent with nine hundred years of not hurrying: records it on
  the plate and lets him leave. Gifted, earned and shown-useful at once, with
  no ceremony, no test and NO PROPHECY — the register here is a clerk writing
  a line in a book.

  Two consequences. The clones do not age, so whoever let him leave is
  probably still there: Glaciem is a returnable place with a WITNESS in it,
  which is stronger than ruins and the same shape as the Pluto marker. And the
  darker reading stays available without being stated — four names that rotate
  and never retire may have let him go BECAUSE they had seen a bearer before
  and knew the cost.
* **`uncharted`** (`archive.rs`) — a site for what nobody has plotted. If the
  makers' world is meant to be unlocated, the slot is there.

**BUILT (this session).** The unconfirmed-claim design is now in
`archive.rs`, and the archive's own integrity tests pass on it:

* `machine_answers_the_watch` (Layer 5, `Stratum::Machine`) — *the carrier has
  a source, and the source is what the Watch is answering to.* Supported by
  `carrier_bearing` and `carrier_null` at `uncharted`, which are TWO findings
  from ONE source (`carrier_drift`). By the module's own rule that is not
  corroboration — it reaches UNKNOWN and stops, exactly like
  `array_reading_one/two/three`. The claim cannot resolve from listening. Only
  standing there could move it.
* `glaciem_was_an_outpost` — supported by `glaciem_rotation_roster`, a
  rotation board with four names and a return date nobody came back for.
* `vault_plate_is_complete` — `asserted: true`, because a plate that says
  *nothing further held* is somebody's record rather than a fact. Supported by
  `vault_plate_inventory` (the plate itself, forty-one entries and forty-one
  strikes) and CONTRADICTED by the roster's hand-off note, which lists an item
  that is not on the plate. SOURCES DISAGREE, over the exact gap the Watch
  came out of.

**The recharge conduit serves both audiences with one system.** Show the
button and the meter — that is `link.rs::recharge`, which clamps and reports
what it took on, and needs no lore to operate. The connection is available to
whoever looks, because `archive.rs` is built for exactly that. A conduit that
refills the reservoir is a FINDING as much as a mechanic.

**Ambiguity: the Quarry model, applied correctly.** The Quarry lets the player
determine what HAPPENS, never what is TRUE — its monster rules are fixed and
acting on a wrong belief gets someone killed. `archive.rs` depends on the same
thing: SOURCES DISAGREE has stakes only because one side is right, and
`facility_built_2003` is meaningfully wrong. If truth were player-assigned,
corroboration would become theatre and a false lead could not be false.

So: the fact stays fixed and UNCONFIRMED, and what the player decides is
whether to ACT on it — travel there or not, stake the crew, commit the fare
home on a single-source claim. The game never confirms what the structure is;
it is precise about what the trip cost. `Stratum::Machine` (Layer 5, "whatever
the Asteri were themselves borrowing from") is described as the part nobody
has finished understanding, so a claim that never resolves belongs there by
design rather than by omission.

Two conditions on that. Ambiguity is expensive to do well: if nothing in the
game ever commits, players correctly read it as the authors not having
decided, so be merciless about outcomes even while the metaphysics stays open.
And **decide it privately anyway** — an unstated fact still has to exist, or
the findings written toward it drift and stop pointing at the same thing.

**Source gap worth noting:** `chi.rs` cites
`TF_CLONE_CIVILIZATIONS_AND_CONVERGENCE.md` for the clone races and their
worlds. That file is NOT in the uploaded archive. If it exists it likely
answers this question; if it does not, then shipped code is citing something
that was never written.

## 6e. The visitor — an ice clone who is off-shift

A Xur-shaped idea, corrected in three places so it fits what is already built.

**Not gated on being good.** There is no morality meter and adding one would
be a fifth fence (§9 says do not). The signal that already exists, and is
thematically exact for a people who grow slowly and discard nothing, is HOW
YOU TREAT EVIDENCE: corroborated versus asserted, whether false leads were
chased and what you did when they resolved to nothing, and `tradecraft.rs`
exposure — whether you let the record be written by somebody who was not
there.

**The visit is not the reward; he comes either way.** What exposure changes is
what he READ about you first. High exposure hands the pen to the Directorate
(`facility_built_2003` is that mechanism already shipped), so the account that
reached him is theirs, with their framing and your name on it. He arrives with
the wrong picture and behaves accordingly — not hostile, a people who do not
hurry do not lecture — he simply asks different questions, and the line he
brings is chosen against what he believes you already know. The loud player's
visit can be STRANGER, not worse. And what he read is itself a claim: single
sourced, uncorroborated, sitting in the same map as everything else, and
contestable with evidence like any other. The archive's thesis is that
official history is something somebody chose to write; this is the entry where
the subject is you.

**He is a PLACE, not a vendor UI.** `stations.rs`: the ship is the menu, a
station is a screen with an address in a room, and `aboard.rs` makes the ship
walkable. He stands in a room. No marker, no indicator, no "visitor aboard"
prompt — you find him because you walked through, or you do not.

**Most visits contain nothing.** He is off-shift and curious; the default
encounter is that he is there, he asks something, and he leaves. An offering
is the exception, or he is a reward dispenser wearing a mystery. When he does
bring something, the strongest thing is a STRUCK-THROUGH LINE from the vault
plate explained — the Archivist took what they recognised, and a people who do
not hurry have been quietly recovering it since. Resolves three threads,
inflates no stat.

**OPEN — a fourth `Medium`.** `Artifact`/`Terminal`/`Site` all mean FOUND. A
finding that is GIVEN changes the epistemics: a person can be partial,
mistaken or selective in a way a cut wall cannot. `Source` is already the
independence key so a person as a source fits; the medium is the gap. Touches
`provenance.rs` and regenerates the C header — do it deliberately, not by
adding a variant in passing.

### His rhythm belongs to him

**He is off-shift.** `glaciem_rotation_roster` is the schedule: four names
rotating out and back. Not a reward timer — an NPC with a life, which is what
makes a world feel inhabited (Majora's Mask, not Xur).

**Give him a POSITION, not a probability.** Being in the solar system should
raise encounters without guaranteeing them — but modelled as two people moving
through the same system, never as a chance multiplier. A multiplier turns
every non-visit into a bad roll and the drive around Chicago into a spin. A
position makes MISSING him informative: if he was at the Pluto marker last
cycle, that is a fact you can learn. Leave evidence when he came and you were
out — something moved, a line added, an object that was not there.

**Deterministic means learnable, and learnable means corroborable.** Three
sightings are three observations from ONE source (you), which by this game's
own rule cannot confirm itself. The roster plate is the independent document
that does. The visitor becomes archaeology assembled by paying attention, with
no new system.

**The rota is astronomical, not arithmetic.** A people with no aging have no
reason to prefer a round number of days; 360 is a human convenience. Shifts
change when the sun reaches a MARK. On an eccentric orbit a quarter by angle
is not a quarter by time — Kepler's second law means the body crawls near
aphelion and hurries through perihelion, so four equal arcs give visibly
unequal shifts, in a fixed pattern, forever. Visits cluster during short
shifts and thin during long ones with no randomness anywhere. This is
COMPUTABLE today: `orbits.rs` is a real Kepler solver (vis-viva,
`period_years`, `semi_major_au`, `perihelion_speed`), and `archive.rs` states
that every number claiming to be physics comes from it.

**Do not publish the mark.** A "next shift change in 40 days" readout is a
timer and players will wait it out. The sky is the only clock.

**REJECTED: shift length as biological adaptation.** Bodies are not shaped by
rosters, and selection pressure over generations fights directly with a people
who do not age and grow slowly in lattice pools. The unequal shifts need no
biological justification; the pattern comes from the sky.

### Their time sense is not ours

The consequence that makes the visitor necessary rather than decorative. If a
decade reads to him the way an afternoon reads to a human, then:

* `patrol.rs`'s *nothing is hurried* is a PERCEPTION, not a virtue, and nine
  hundred years on one roster is an unremarkable career rather than devotion;
* he is vague about human intervals and exact about long ones — "recently"
  meaning two centuries, but the shift mark to the hour;
* and the Morpher is not an old acquaintance he checks on. He is something
  visibly coming apart FAST. The ledger runs in increments that read as
  gradual to us and alarming to him. That is the curiosity, and it needs no
  fondness and no prophecy: he wants to see how far the thing they let walk
  out of the vault gets.

## 7. Billy

Billy does not get the Watch as a loadout choice. If it comes off freely then
*is he still himself* has an obvious answer — take it off — and the ledger
becomes optional. Removal is a failure state or a last resort, once, late.

The one plausible route is local mode: severed, the device runs on stored
matter rather than a paired nervous system, which is the only way it does
anything for someone it never paired with — badly, at enormous cost per shift,
until the reservoir is dry. This does not undo pairing.

**Guard the Sun Gun.** Billy's weight there is that a *human* makes the call.
Give him the Watch too freely and the one person who matters as a human becomes
a second Morpher.

## 8. Voices

The Morpher sounds unplaceable for reasons already in the code: `watch.rs` says
his reproduction of a form is faithful but not native and costs him more than
it costs a native, and `chi.rs` makes him an ice clone. A body that has been
eleven things and is currently doing an imitation of the one it was born as
would not sound quite like anything. That is what he is, not an effect applied
to him.

Billy is unusual for a **different** reason. Two unusual people with one shared
cause is a smaller universe than two who arrived separately, and it makes the
Watch the explanation for everything within a hundred metres of it.

**If a proximity effect is used, the seam is hard: proximity touches the body,
shifting touches the person.** A trace in a recording is fine. Costing anyone
else specificity is not — that stays exclusive to the bearer and stays earned
per shift, or the ledger becomes ambient, which means nobody's fault, which
means it is not tragedy.

**Implementation note:** there is no voice-effect path in the tree.
`shared/voiceprint-kit` is MFCC fingerprinting for matching; `shared/audiomix-kit`
is channel routing and mixing. Both pure sample math, neither a filter chain. A
per-character treatment is new DSP. Prefer direction over processing — a filter
reads as "robot effect", which is the opposite of what this character needs.

## 9. What bounds the player

The Quarry/Destiny property — an open world bounded by knowledge and
consequence rather than walls — is **already shipped here**, as four separate
systems that were never named as one:

| fence | module | what it withholds |
|---|---|---|
| relay cost | `tradecraft.rs` | finding is not knowing until you pay to report it |
| corroboration | `archive.rs` | one source can never confirm itself |
| attunement | `chi.rs` | serious kit gated behind standing in a place |
| the fare home | `link.rs` | movement bounded by the cost of getting back |

**Do not add a fifth.** If a future design wants to bound the player, it should
tighten one of these rather than introduce a new mechanism.
