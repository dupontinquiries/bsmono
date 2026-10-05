# Echelon (M0+M2-core) — deterministic RTS in Rust

A StarCraft-2-style 1v1 RTS built around the three races of the *Scion
Custom Races* mod — **Genetron**, **Xayid**, **Keiron** — reconstructed from
the mod's actual data files. This drop contains the finished **M0
deterministic simulation core**, a headless match runner with scripted
macro bots for all three races, and the transport-agnostic **lockstep
session layer** (the heart of M2), proven by running two full clients
in-process with zero desyncs.

No engine dependencies: the sim is pure Rust (serde + ron only) and builds
on **distro rustc 1.75** (`targets.x86` — "games must build here" — and the
aarch64 hosts alike).

## Quickstart

```sh
cargo test                                   # fx math, determinism, lockstep
cargo run --release -p echelon-runner -- all --verify
cargo run --release -p echelon-runner -- Keiron Genetron --events
cargo run --release -p echelon-runner -- Genetron Xayid --lockstep
cargo run --release -p echelon-runner -- Keiron Genetron \
    --svg out.svg --svg-every 2200          # battlefield snapshots
```

Runner arguments: `[race1 race2 | all]`, `--map NAME` (standard,
confluence, cauldron), `--seed N`, `--ticks N`, `--events`, `--verify`
(re-simulate the recorded command log and compare periodic + final state
hashes), `--lockstep` (two clients, in-memory transport, hash
checkpoints), `--svg PATH [--svg-every N]`, `--png PATH [--png-every N]`
(lit look-dev frames), and `preview MAP OUT.png` (render a map with its
tileset's palette + light rig).

Sample session:

```
== Genetron vs Xayid  (seed 11, data hash 1eeac58464d0f18e) ==
   winner: player 0 (Genetron) at t14885 (11m16s)
   commands: 521   final state hash: b43a53f7a91b00a1
   verify (re-sim from command log): IDENTICAL — deterministic
== Xayid vs Keiron ==      winner: Xayid  at 9m38s   ... IDENTICAL
== Keiron vs Genetron ==   winner: Keiron at 5m00s   ... IDENTICAL

== lockstep Genetron vs Xayid  (turn=4t delay=2 turns) ==
   4620 command turns, 41 hash checkpoints exchanged, desync: none
   client worlds bit-identical at end: YES
```

## Crates

| crate            | what                                                          |
|------------------|---------------------------------------------------------------|
| `echelon-data`   | unit/race definitions, RON loading, validation, FNV pack hash |
| `echelon-sim`    | the tick-locked world: Q32.32 fixed point, grid A*, economy, construction, production, combat, regen, state hashing |
| `echelon-net`    | lockstep sessions: command turns, input delay, deterministic merge, hash exchange, desync detection (no sockets — feed any transport) |
| `echelon-bots`   | the scripted macro AIs, shared by runner and client            |
| `echelon-runner` | headless matches, replay verification, lockstep harness, SVG snapshots |
| `client/`        | **M1 Bevy front-end** (detached workspace — needs current stable Rust; see `client/README.md`). Play any race vs the bot: drag-select, move/attack/harvest, train hotkeys, build-placement ghost, HUD |

`assets/data/races/*.ron` holds every unit and structure — including
vertical `depth` (massive ground units and capital ships fill two
levels, townhalls three) and `fly_layer` for the three air bands.
`assets/data/maps/*.ron` is MapDef v2: four aligned char layers
(`height` 0–5/`#`, `ramps`, `bridges`, `things`) giving six terrain
levels, three air layers above (nine total), and native bridges —
ground units cross decks or pass beneath when their depth clears, air
routes around structures tall enough to reach its band. Authored via
`tools/gen_maps.py`; palettes/light rigs in `assets/data/tilesets/`
(see `docs/ART_DIRECTION.md`); **Phase 2 is live**: `crates/echelon-deck` is the native client — headless
tape mode (select/move/attack/hold/rally/build/train/cast + assertions +
PNG frames) is CI-proof, `--features window` adds the minifb frontend on
a desktop; the sim gained fog (`visible_mask`/`sight`), Hold, and rally
walk-outs, and a long-standing bug fell: produced units were never
charged supply. `crates/echelon-arena` is the Proving Grounds — the bot
refactored into a 27-gene policy, evolved against the coach in resumable
bursts (`evolve gens pop out [init] [base]`), measured by `bench`.
Current state: gen-5 champion sweeps its training battery 3/3 and takes
41.7% of the full cross-map grid — the per-generation config rotation
now in place exists to close that gap. **Phase 2 is live**: `echelon-runner serve R1 R2 --map M` speaks a
JSON-lines protocol on stdio (fog-filtered snapshots out, token commands
in — move/amove/stop/hold/harvest/build/train/cast/rally) and seats the
Command Deck (`tools/playdeck.py`, a playable faction-skinned client
with drag select, smart orders, production hotkeys, ability casting,
rally, fog with last-seen ghosts, and a headless `--script` CI mode),
future agents, and replays alike. `crates/echelon-arena` is the Proving
Grounds: the scripted bot refactored into a 27-gene `Genome` policy
(`coach()` = the hand-tuned play), a GA league with
imitation-by-initialization, the coach as permanent fixed exploiter,
rotating evaluation seeds against memorization, deterministic
entity-budget guards, warm-restart bursts for single-core boxes, and
`bench` — the crush criterion (>=90% vs coach across all pairings, maps,
colors, seeds). Tournaments run through `games/bsgames` (the portfolio tournament host): `echelon-arena play-one` is the registered provider, and the Echelon Invitational — coach vs champion across all six race seats — is the inaugural event. See `docs/`/the phase-2 prompt for the league design.
SC2Map prototypes convert via
`tools/sc2map_extract.py`; `tools/mapedit.py` (built on
`shared/editor-kit`) paints all four layers interactively, wearing
faction skins — `T` cycles Genetron / Xayid / Keiron consoles. Abilities are data too — `assets/data/abilities.ron`
maps every roster power onto the effect-op vocabulary (atlas + fidelity
notes: `docs/ABILITIES.md`); placeholder unit meshes come from
`apps/cadscope` (`cadscope gen-race Keiron …`). All numbers are
integers (milli-units where fractional), so data files contain no floats
and hash identically everywhere; the pack hash is part of the lockstep
handshake.

## Determinism contract (enforced, tested)

- No floating point in `echelon-sim` — positions/HP/energy are Q32.32
  (`Fx`), distances compared squared in i128, integer sqrt for steering.
- No hash-map iteration; entities live in a `Vec` with stable indices and
  tombstones.
- All randomness flows through one seeded PCG32 owned by `World`.
- Fixed system order inside `World::step`; commands are the only input.
- `World::state_hash()` folds every observable field; a replay is exactly
  `seed + data hash + command log`. `--verify` proves it per match; the
  determinism test proves it cross-world; `--lockstep` proves it across
  two clients exchanging checkpoints.

## The races as implemented

**Genetron** (Attend builds — worker stays on site; no innate regen):
ProcessingCore → Node → Manufacturer/Uploader/AnalysisTerminal →
Fabricator → AssemblyArray, with OutfittingStation/TechVault/RelayTower/
Generator support. The signature *Uploader link* system is live:
`@uplink1` / `@uplink2` requirements mean "this facility has ≥N Uploaders
linked" (Fixer, Blaze, Blitzer at 1; Equalizer, Tesla, Inciter, Boar,
Jupiter +OutfittingStation; Aquila, Virtus, Badger at 2 +TechVault). Bots
build and link one Uploader per Manufacturer.

**Xayid** (Consume builds — the worker mutates into the structure; innate
regen): three independent spawner streams instead of one larva pool —
XayidDen breeds **Worms** (Xayidling, Roamer, Scorpalisk, Mendling,
Casnolisk, Eroder), BiomassHatchery breeds **Grubs** (Reviler, Ferrion,
Spitter, Prowler, Massalisk), AvianNest breeds **Wasps** (Raptor — twin
hatch, Assailant, Exterminator, Xayithoan, Kraken). Training consumes one
from the matching stream; pools regrow on a timer.

**Keiron** (Auto builds — worker shapes it and walks away; shields on
everything + innate regen): **no unit-production buildings.** All sixteen
units materialize at Citadels (3 parallel warp slots, 5 s; Titan/Paragon
10 s); tech structures (Edifice, Sanctum, Conduit, Elysium, Foundry,
Outlet, Empyrean, Atrium, Pantheon, Zenith) only unlock the roster. Supply
comes *only* from Citadels (18 each) — expanding is supply, exactly as in
the mod data (`Citadel food=18`, everything else 0).

## What's faithful vs. seeded (BALANCE)

Faithful to the mod's data files: rosters, tech-tree edges, HP/shields,
supply (including 1.5-supply twin Raptors), build times for structures,
race mechanics above. **Seeded placeholders**: alloy/flux costs, weapon
damage/range/cooldowns, and movement speeds (the mod stores these in
files we didn't mine). They live entirely in the RON files — rebalancing
is data-only.

Not yet simulated (tracked for M3): active abilities & energy spenders
(heals, EMP-likes, Aegis), upgrades, transports' load/unload, detectors &
cloak, addon-style Uploader *configuration* effects beyond tech gating,
creep-like spread, high-ground vision.

## Roadmap

- **M0 sim core — done** (this drop): fixed-point lockstep sim, data-driven
  races, bot matches, replay verification.
- **M2 net core — done in-process** (this drop): `echelon-net` sessions.
  The UDP transport landed in c21 (`echelon_net::udp::UdpLink`: the same
  `Msg` values over a datagram socket, re-sent until acknowledged; proven on
  loopback through a relay that drops every third datagram). Remaining: a
  relay server and the client wiring (the Bevy client is still human vs bot),
  reconnect, drift buffering under real latency.
- **M1 client — builds clean** (verified this session, first time ever: a
  real rustc 1.96 finally cleared Bevy 0.16's MSRV, well past distro 1.75,
  hence the detached workspace). `client/`: Bevy 0.16 front-end over the
  same sim (Bevy owns its wgpu; `shared/gpu-kit` policy doesn't apply —
  decision recorded in the project prompt). Human vs bot playable:
  selection, orders, training, placement, HUD, win/lose. `cargo check`
  caught one real pre-existing bug the untested "written" state had been
  carrying: `order_label` (a `#[allow(dead_code)]` function kept "for a
  future rally UI") didn't match `Order::Hold`, a real `E0004` non-
  exhaustive-patterns error that had simply never been compiled before.
  Fixed (labeled `"holding"`, matching `Hold`'s own doc comment — "stand
  and fight: fire in place, never chase"). Not run on a real display
  (headless sandbox) — compiling clean is confirmed, playing it is not.
  Still open: fog, health bars, minimap, replays as spectator input.
- **M3+**: abilities/upgrades, the classic trio, maps, editor.

## Monorepo integration

Lives at `games/echelon` in this monorepo (`apps/echelon` in the standalone repo this README came from). Registered in `manifest/projects.toml` as:

```toml
[apps.echelon]
path = "games/echelon"
kind = "cargo"
role = "app"
daemon = false
flatpak = false          # headless M0; flip when the Bevy client lands
runtime = "none"
targets = ["desktop", "x86"]
notes = "Echelon RTS — deterministic lockstep sim + net core; Scion-inspired races; distro rustc 1.75; workspace (data/sim/net/runner)"
```

## Provenance & IP

Fan project. Race design and names originate from the *Scion Custom
Races* StarCraft II mod by **Solstice245** (data mined from the published
mod files); StarCraft belongs to Blizzard Entertainment. Nothing here
ships Blizzard or mod assets — geometry, code, and numbers are original
or reconstructed. Every name lives in `assets/data/races/*.ron`, so a
full rename pass for public distribution is a data edit, not a code
change.
