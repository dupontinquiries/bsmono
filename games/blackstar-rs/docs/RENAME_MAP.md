# The filter pass — licensed marks out of `src/`

`docs/films/README.md` states the vocabulary rule and says the two screenplays
were checked clean by direct grep. They are. **`src/` is not**, and nothing in
the tree says so. This file is the map for fixing that, and it exists because
the rule is currently enforced on 2 files out of ~110.

Counts as of this writing (`grep -rniE ... src/ --include=*.rs -o | wc`):

| mark | hits | files (partial) | kind |
|---|---|---|---|
| cybertron / cybertronian | 276 | `raidgen`, `gear`, `gates`, `archive`, `mapsim`, `saga`, `district`, `procgen`, `loot`, `campaign` | **prose + identifiers + a module + an enum variant** |
| energon | 152 | `energon.rs`, `gear`, `gates`, `content`, `weapons`, `nightfall`, `campsim`, `escalation`, `stations` | **live identifiers throughout** |
| optimus | 45 | `mapsim`, `saga`, `content`, `grandeur`, `pulse`, `scale`, `codex`, `casting` | prose + content strings |
| megatron | 32 | `saga`, `content`, `codex`, `casting`, `main`, `cybertronian`, `stations` | prose + content strings |
| decepticon | 15 | `saga`, `content`, `census`, `codex`, `tradecraft`, `corruption` | prose |
| autobot | 15 | `saga`, `content`, `campaign`, `pulse`, `codex`, `duel` | prose |
| allspark | 2 | `xform`, `compact` | doc comments only |

Also outside `src/`: `Rodimus Prime` is a speaking role in
`docs/films/FILM_05_SOLAR_SCREENPLAY.md`, and `docs/films/README.md`'s own
allowed-vocabulary sentence ends with "Rodimus" — the rule has a hole in the
sentence that states it. Fix the rule and the screenplay in the same pass.

---

## Order of work, and why

**1. `allspark` (2 hits) — leave as-is, deliberately.** Both are doc comments
citing the reference material as the *reason for a rendering technique*
(`compact.rs`: matter compacting into cubes as the justification for the
displacement field over a vertex lerp). That is developer attribution, not
in-fiction vocabulary, and removing it would delete the reasoning that makes
the module reviewable. If they go, they go last and they go by rewording to
"the reference material" rather than by deletion. **The in-fiction object is
the Lattice** (`docs/films/CANON_DECISIONS.md` §4) and was never called the
Cube in the game.

**2. `autobot`, `decepticon`, `optimus`, `megatron` (107 hits) — prose and
content strings.** No identifiers. Safe to do first because nothing compiles
differently. Two of these are not pure renames:

* `optimus`/`megatron` appear in `casting.rs` and `codex.rs`, i.e. in things
  the player reads. Those need *characters*, and per
  `docs/films/CANON_DECISIONS.md` §6 the Rodimus replacement also inherits the
  mid-battle "leadership is not the promise of survival" speech. **Do the
  character naming as one decision, not per-file.**
* `autobot`/`decepticon` map onto vocabulary that already exists and ships:
  **Coalition** and **Blackstar Clans / Corrupted**. Mostly mechanical.

**3. `energon` (152) — the only live-identifier rename.** `src/energon.rs` is a
module; `energon()`, `rec_energon()`, `energon_value()`, `at_energon`, and
several test names carry it. This is a compiler-checked rename, which makes it
the *safest* large one — nothing silently survives — but it touches ~9 files
and `main.rs`'s `mod` list.

Proposed replacement: **`ichor`**. It pairs against `chi.rs` (what a human body
holds) as the machine-side counterpart, and bot fuel as a body-fluid word is
already the register `energon.rs` is written in. Alternatives if that reads
wrong: `plasm`, `vitae`. Decide before starting; a half-done identifier rename
is worse than none.

**4. `cybertron` / `cybertronian` (276) — do this LAST, and separately.** It is
not a rename, it is a lore decision: the home planet needs a name, and
`Body::Cybertronian` in `loot.rs` plus `src/cybertronian.rs` plus `mod
cybertronian;` in `main.rs` mean the chosen word becomes a type and a module.
`campaign.rs::the_shield_needs_both_slipstreams_coordinates_and_the_cybertron_archive`
is a test name that encodes it too.

Note the interaction with the prologue: the monologue's third line ("Cybertron
stood for millions of cycles") is **not** renamed to the new planet — per
`CANON_DECISIONS.md` §6 it is reassigned to the ice clones and becomes a better
line. Do not sweep it with the rest.

---

## Rules for whoever runs the pass

* **One mechanical commit per mark, no behaviour change**, so the diff is
  reviewable. Do not fold the rename into a feature commit.
* **Decide the replacement word before the first edit.** Every one of these
  appears in prose that explains *why* code is shaped the way it is; a
  find-and-replace with a bad word damages the reasoning as well as the noun.
* **Check test names.** Several tests encode marks in their identifiers
  (`insufficient_energon_never_partially_spends`,
  `low_energon_is_a_wall`,`storyline_energon_meets_every_recommendation`,
  `the_shield_needs_both_slipstreams_coordinates_and_the_cybertron_archive`).
  These are documentation and should be renamed with the thing they describe.
* **Re-grep after each commit** and record the new count here, so the file stays
  a status board rather than a plan that went stale. `docs/films/README.md`
  should point at this file instead of implying the tree is clean.
* **Add a guard.** Once a mark reaches zero, a test that greps the tree for it
  is cheap and stops it coming back through a future content string. This is
  the only way the rule survives contact with new writing.
