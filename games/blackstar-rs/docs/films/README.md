# The films, as source material

**These screenplays are not a production plan. The game is the work.**

Nine films were written for the Blackstar saga and it is now unlikely any of
them get made. Game production is the tractable path — the tooling, the
expertise and the resources are here, and film production is none of those
things. So the relationship between these documents and `blackstar-rs` is the
reverse of a tie-in: **the game is the medium this story ships in, and these
are the most recent iteration of the storyline, half-finished.**

That has three consequences worth stating rather than leaving implied.

**Half-done is the expected state, not a defect.** Nothing here is waiting on a
polish pass before it counts. Where a beat is thin, the game is free to be the
place it gets decided, and the doc should then be updated to match the game
rather than the other way round.

**Where a doc and the code disagree, the CODE is canon.** This inverts the
usual rule and it is deliberate: `content.rs` runs the missions people actually
play, `archive.rs` holds the strata they actually dig through, and a screenplay
beat nobody can reach is a note. `CLAUDE.md` §5 already says to verify claims
against code rather than documentation; for story this is the same rule with
the reason spelled out.

**Vocabulary is the one hard constraint.** This universe is Coalition /
Hijacks / Blackstar Clans / Corrupted / Rodimus. It is NOT Autobot, Decepticon,
Optimus Prime or Cybertron — those are licensed marks belonging to
Paramount/Hasbro, and earlier drafts of every one of the thirteen original
screenplay files used them. The two files here were checked clean by direct
grep, and anything added beside them should be.

## The Unicron material is a reference, not a claim

The Unicron thread is fan-made homage. It is not a claim on the official film,
and the vocabulary rule above applies to it exactly as to everything else — the
homage is carried by structure and by what the story does with the idea, not by
borrowed names.

That matters for Films 7–9 specifically, which are **held rather than pending**.
The operator has not worked on their content in two decades and it wants real
deliberation, not a writing sprint; filling them in from their titles would be
inventing the least-decided part of the saga. The one settled constraint is
ORDER: Convergence can only slot in after the Unicron payoff has actually been
delivered, which lands in the Doomsday trilogy.
`saga::order_respected` enforces that, and
`saga::join_tests::convergence_cannot_ship_before_the_debt_it_is_downstream_of`
exercises it against synthetic inputs — because with Films 7–9 unwritten the
rule is vacuous against live data and would otherwise pass while measuring
nothing.

## What is here

| file | status |
|---|---|
| `FILM_05_SOLAR_SCREENPLAY.md` | current. Replaced the abandoned `FILM_05_EMBER_DEEP`. |
| `FILM_06_PHOTON_SCREENPLAY.md` | current. Replaced the abandoned `FILM_06_CONVERGENCE`. |

Films 5 and 6 were rewritten because the originals were **a different plot
entirely** — a Matter Transformation Planet, an Ice Mirror clone vessel, a
singular "Hijak Commander" — that contradicted the shipped codex and the
mechanics the bible names as Trilogy 2's climax (`watch.rs`, `chi.rs`). The
replacements were written against confirmed canon only: the shield-crossing
aftermath of Film 4, and the Hijacks as data-gatherers rather than conquerors,
which `content.rs` implements and the codex states outright.

Their climax is the sequence to protect if anything here is ever revised: clone
reinforcements arriving **by gravity warp, materialising inside the Hijack
formations** rather than approaching conventionally, the first jump nearly
silent; then Billy's Sun Gun, which requires a human call on the final shot —
"You have one shot." / "I know." / "Let's go." — discharging not as an explosion
but as an artificial solar event that lights the whole battlefield and shows its
true scale for the first time.

## Also here

| file | status |
|---|---|
| `SCREENPLAY_INVENTORY.md` | **stale** — predates the 5–6 rewrite; still useful for Films 1–4 and 7–9 |
| `SCREENPLAY_TO_GAME_INTEGRATION.md` | **a proposal** — its pipeline is sound, two of its claims about the code are not true |

Each carries a status header saying exactly which parts to distrust.

## The Optimus / Megatron question, measured

An open creative decision, recorded so that whenever it is picked up the cost
is known rather than guessed at. Three findings, and they make the decision
smaller than it looks.

**The team-up already exists and already ships.** Missions 8 and 9 —
`the_ashen_truce` and `beneath_the_pacific`, both Film 3 — are exactly that
beat, playable today. Megatron crosses the shield with his transponder open and
his weapons cold ("the devourer eats decepticons too"); Prime meets him at the
airlock, no speech, "two old soldiers, one war left"; and under the closing
plates Megatron plants himself and says "go", nothing else. "They never find
megatron. prime marks the spot anyway. the war's oldest debt, paid last."
Whatever is decided about names, that is written and it is not at risk.

**Neither is a playable or fieldable unit.** Zero `Troop` entries, nothing in
`roster.rs`, `bots.rs`, `loadout.rs` or `census.rs`. They exist as two
`codex.rs` entries and as prose inside mission intro/outro lines. So "renaming
would leave them as non-playables" is not a cost of renaming — it is already
the case, for unrelated reasons.

**The rename is prose, not structure.** 25 mentions of Optimus and 15 of
Megatron across `src/`, concentrated in `content.rs` mission text and two codex
entries, plus 14 and 8 in `docs/`. No system depends on either name; nothing
keys off them. `saga::join_tests::the_inherited_contamination_is_counted_not_hidden`
already counts them, so progress on it is visible either way.

What that leaves is the narrow question rather than the broad one: not whether
independence costs the team-up, but what those two characters are called. The
beat is structural — two enemies whose war defined an era, made small by
something neither can fight alone, one of them paying for it — and it survives
any names at all.

## One character the newest canon introduces and the game has never heard of

**Meridian.** She speaks in both Films 5 and 6 — the sensor and tactical
voice, and the one who says "Billy. This is it. You have one shot before
their formation adapts." She has no `codex.rs` entry and no `content.rs`
troop. Morpher, Billy, Aiden and Rodimus all do.

That is the clearest single piece of work these documents imply: the current
screenplays introduce a named character the shipped game cannot refer to.

## Provenance, and what is still missing

Both files were extracted from a session transcript
(`post_c12unified_--_Completing_movie_saga_scripts_for_blackstar-rs_game.md`,
2026-08-25), which was their **only surviving copy** — no screenplay existed
anywhere in this monorepo before this commit. Verbatim, minus the artifact
header line the transcript wrapped them in.

Not recovered, and known to exist somewhere:

* the other eleven screenplays, including the `renamed_screenplays/`
  set (name changes only, original stories intact)
* ~~`src/saga.rs`~~ — **recovered.** It is in the tree now, reconciled: it had
  never compiled (`vec![]` in a `static`), and its Films 5 and 6 were the
  abandoned Ember Deep / Convergence plot. Films 1–4 and 7–9 still carry
  inherited licensed vocabulary and abandoned-plot references, counted by
  `saga::join_tests::the_inherited_contamination_is_counted_not_hidden` rather
  than silently rewritten — renaming those characters is authoring, not
  cleanup.
* `CANON_DISCREPANCY_NOTE.md`, which tracked exactly this supersession
* the superseded `FILM_05_EMBER_DEEP` / `FILM_06_CONVERGENCE`, deliberately
  kept rather than deleted in case anything in them is still wanted

If those turn up, they belong here beside these two.
