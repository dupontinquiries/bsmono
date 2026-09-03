# The filter pass — DONE, and what the counts became

**Status: complete except for floors.** ~680 licensed marks down to 30, none of
which are in-fiction vocabulary. `src/vocab.rs` is the live version of this
document: it counts every mark across `src/` and fails when a number moves, so
what follows is the record of how it went rather than a plan to follow.

The names are in `docs/films/CANON_DECISIONS.md` under "The cast, named".

## What the original plan got wrong

**"autobot/decepticon map onto vocabulary that already exists and ships:
Coalition and Blackstar Clans / Corrupted. Mostly mechanical."** Wrong.
`content.rs` disproves it in one line — *"the thing the coalition and the
Autobots both"* — and `patrol::Faction` annotates `Coalition` as "the elemental
coalition", the clone races, not the robots. The robot factions needed names of
their own. The prose half was one authoring decision, not four mechanical ones.

**The census was short by 146.** Unicron (70), Jhiaxus (37) and Rodimus (39)
were never counted. `docs/films/README.md` named Rodimus inside the very
sentence stating the vocabulary rule.

**"Cybertron is a rename."** It was three things: an adjective and a type
(`Cybertronian` → `Body::Machine`), a place (`Cybertron` → the Foundry), and
asset filenames that must not move at all.

## Five things a vocabulary sweep does that nobody expects

1. **It leaves `src/`.** Boss ids in `assets/raids/bosses.json`, catalogue ids
   in `tools/meta/library.json`, relic prose. `raidcon`'s roster-matches-json
   test was twice the only thing between a rename and silent data drift.
2. **It corrupts code that reasons ABOUT the names.** `saga.rs`'s banned-word
   list and contamination counter had their needles rewritten five separate
   times, turning a licensed-mark detector into a detector of the replacement
   names — and the symptom was always a count going UP, reading like a
   reintroduction. Now guarded by `vocab::no_sweep_has_eaten_a_guard`.
3. **It falsifies citations.** `grandeur.rs` cites real production
   measurements; renaming a label attributed a published 28 ft figure to a
   fictional character. Only the ungrammatical half of the same sweep ("a the
   Foundry spire") made anyone look.
4. **Writing about a mark creates an occurrence of it.** Guard needles, policy
   notes, this document. A census cannot distinguish a use from a mention, so
   every explanation raises a floor. Expected, not alarming.
5. **`\b` does not work.** Underscores are word characters, so `\benergon\b`
   misses `energon_value` and `at_energon`. The first pass left 30 occurrences
   and looked like it had worked.

## The 30 that remain, and why each cannot go lower

| kind | count | where |
|---|---|---|
| citations of real reference material | ~15 | `grandeur.rs` production figures, `compact.rs` / `xform.rs` technique attribution |
| guard needles | 6 | `saga.rs`'s two guards must contain what they detect |
| asset filenames | 3 | real files on disk naming a real game |
| policy notes | ~6 | comments explaining the above, which necessarily name it |

## Rules that still apply

* **`DO_NOT_SWEEP`**: `saga.rs`, `grandeur.rs`, `vocab.rs`. Each has been
  damaged by a sweep at least once; the list exists because remembering failed
  five times.
* **Update `CENSUS` in `vocab.rs`** as anything moves. That edit is the record.
* **Regenerate `archive_gen.h`** after touching `archive.rs` — and rebuild the
  binary FIRST, or `--arch emit-c` writes a header that looks updated and is
  not.
