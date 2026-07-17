# Game editors — the family

Every game in `games/` now has an authoring surface, all cut from the same two
cloth patterns the repo already proved out:

- **The forge pattern** (`soldier/src/buildscript.rs`): a stable, documented
  EXTERNAL op format — names not IDs, ops as a sequence not a snapshot,
  per-op error tolerance (one typo never voids the script), and an
  `export()` that round-trips so tooling can read → propose → hand back a
  full document. Authorable by a human, a GUI, or an LLM shown the schema.
- **The editor pattern** (`paperbound/rts_editor.py`): a standalone GUI with a
  `--selftest` headless mode, editing the same data files the game loads.

| game | editor | format | entry point |
|------|--------|--------|-------------|
| **soldier** | Forge mode (in-game) + buildscript — *the reference* | JSON op pipeline → `CustomMap` | `src/buildscript.rs`, `game::Editor` |
| **paperbound** (RTS) | Foldworks map editor — *the reference* | `rts_data.json` maps/events | `python rts_editor.py` |
| **paperbound** (battler) | Pack editor — cards + origami model designer with live preview | `expansions/<pack>.json` (merged by `expansions.py`) | `python pack_editor.py [pack]` · `--selftest` |
| **gyroball** | Maze editor — draw walls, place start/goal, live reachability check | maze packs `v15/mazes/*.json` | `python v15/maze_editor.py [pack]` · `--selftest`; play with `python v15.py [name] --pack mazes/starter.json` |
| **blackstar** | Arena forge — JSON ops → IQM + obstacle/spawn sidecar, plus a top-down GUI | build doc → `engine_src/data/<name>.iqm` + `.arena.json` | `python tools/arena_forge.py {example\|build doc.json\|edit\|--selftest}` |
| **clawbound** | Worldscript — layers hand-tuning over the procedural world: density multipliers, cleared arenas, pinned cover, beacon/spawns | sectioned text (no serde, `db.rs` spirit) | `src/worldscript.rs`; `worldscript::load_and_activate(path)` before world build |
| **bsgames** | — none: it's the games-centre *launcher*, not a game; there is no level content to edit | | |
| **unendlich** | **Deliberately NOT a map editor.** World packs (parametric generation data) + an LLM terrain-modification HTTP endpoint. Fixed maps are rejected by policy — including for server sub-lobbies — because peers must regenerate identical terrain from `(seed, pack)` | world-pack JSON (biomes + climate offsets + seed-anchored field brushes) | `worldpacks/`, `crates/unendlich-engine/src/world/worldpack.rs`, `…/terrain_llm.rs` (`GET /schema`, `POST /terrain?lobby=L`) |

Shared properties, on purpose:

- **LLM-authorable.** Every format is self-describing JSON or plain text with
  names not IDs; blackstar's forge and unendlich's endpoint serve/print their
  own schema. This is the buildscript header's promise ("including an LLM
  given the schema below") carried across the fleet.
- **Error-tolerant application.** Bad ops/lines/mods are collected and
  reported; the rest of the document still lands.
- **Round-trips.** arena_forge `export()`, worldscript `export()`,
  maze/pack `save→load`, worldpack serde round-trip — all covered by
  selftests/unit tests.
- **Deterministic.** Same document (+ seed where applicable) → identical
  output bytes/placements. Verified in each selftest.
