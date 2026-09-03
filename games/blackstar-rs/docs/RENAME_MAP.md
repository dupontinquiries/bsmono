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
* ~~`autobot`/`decepticon` map onto vocabulary that already exists and ships:
  **Coalition** and **Blackstar Clans / Corrupted**. Mostly mechanical.~~
  **WRONG — corrected after an audit.** `content.rs` disproves it in one line:
  *"elites — they are the thing the coalition and the Autobots both"*. The code
  treats Coalition and Autobots as DIFFERENT factions, and `patrol::Faction`
  agrees — `Coalition` is annotated "the elemental coalition", i.e. the clone
  races, not the robots. The robot factions need names of their own, exactly
  like the two characters do. **The whole prose half is one authoring decision,
  not four mechanical ones.**

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
* **The guard exists now: `src/vocab.rs`.** It walks `src/*.rs` and asserts an
  exact count per term, so a removal FAILS and prompts you to lower the number
  while an addition fails too. Modelled on `saga.rs`'s
  `the_inherited_contamination_is_counted_not_hidden`, which had the idea first
  for one file.

  A count rather than a ban, because the cleanup is blocked on AUTHORING, not
  on effort, and a test demanding zero would simply get disabled. Update
  `CENSUS` in `vocab.rs` as terms come down; that edit is the record.

  `nothing_new_has_crept_in_beside_the_terms_being_tracked` covers the failure
  mode a fixed list has — other marks arriving — for Bumblebee, Starscream,
  Soundwave and Primus.
* **`AllSpark` (2) should STAY** and is counted so a well-meaning sweep fails.
  Both are doc comments citing the reference material as the reason for a
  rendering technique; deleting them removes the reasoning that makes
  `compact.rs` reviewable.
