# CLAWBOUND (native Rust + wgpu)

A papercraft stalk / hide / chase game built on a native Rust + wgpu renderer
(Vulkan on Linux, Metal on macOS, WebGPU on the web). Flat "origami" facets, warm
key light, paper texture, ink outlines, grain and vignette. The whole game lives in
a shared **library** (`lib.rs`) so the offscreen renderer, the live window, and the
web build all run the exact same deterministic logic.

## Four game modes
| Mode | Players | Goal |
|------|---------|------|
| **Rendezvous** | 6 cats vs 1 hunter | Reach the beacon to escape; hunter wins by taking 3 of 6 |
| **Survive** | 6 cats vs 1 hunter | Outlast the hunter for the round using cover |
| **Prowl** | 1 cat vs 1 stalker | Clear rotating objectives (Reach / Rendezvous / Survive / Forage), hunt prey, survive a shrinking ring; respawn on catch |
| **Pop Island** | 3 vs 3 teams | "Yarn Ball Capture" — carry yarn to your base; most captures over two periods wins (sudden-death overtime on a tie) |

Prowl and Pop Island are ports of two browser prototypes onto the native engine,
sharing the `class` / `powerup` / `ring` primitives.

## Build & run (headless renderer → PNG)
```bash
cargo run --release -- [OUT.png] [MODE] [TOD] [ARG4] [ARG5]
```
- `MODE` — `world` (default) · `map` · `goals` (+ prints a fairness grade) ·
  `poses` · `seq` · `senses` · `wildlife` · `game` (ARG4 = hunter) ·
  `prowl` (ARG4 = hunter, ARG5 = class) · `capture` (ARG4 = class)
- `TOD` — `day` (default) · `dusk` · `night` · `nightmare`
- hunters: `fox dog badger hawk snake shadow` · classes: `cat squirrel otter frog goat hawk`

```bash
cargo run --release -- prowl.png   prowl   day  badger otter
cargo run --release -- island.png  capture day  squirrel
cargo test                                          # 69 unit/property/soak tests
```
The renderer is offscreen → PNG, so it works headless (e.g. CI with Mesa lavapipe).

## Play it (live window)
```bash
cargo run --release --features app -- --play
```
Opens the menu: pick **Game** (Rendezvous / Survive / Prowl / Pop Island), a
**Hunter**, and a **Class**, then Start. Settings has an **Easy / Normal / Hard**
difficulty that scales the hunter's speed, how fast it grows suspicious, and how far
it roots hiders out of cover.

Controls (mouse buttons are bindable alongside the keys):
- **WASD / arrows** — move · **mouse** — look
- **Shift** — hide (crouch in a bush) / climb a trunk
- 6v1 & Prowl: **Space / middle-click** — roll (i-frames) · **F / left-click** — leap (airborne, uncatchable)
- Pop Island: **Space / left-click** — throw a firecracker · **Shift / right-click** — jump
- **R** — restart · **Esc** — menu

For the phone/web canvas client see **BUILD_WEB.md** (`web` feature, wasm32 → WebGPU).

## Architecture (`src/`)
Shared library (`lib.rs`):
- `math` — Vec3 + column-major Mat4 (wgpu conventions)
- `mesh` — flat-shaded solids (box / cone / tapered segment) + screen-door opacity
- `world` — domain-warp terrain, Whittaker biomes, genetic foliage, talus erosion + baked water, grass/rocks/bushes/reeds
- `physics` — branch mass + bending snap-risk (climb limits)
- `cat` — posable two-bone-legged rig: gaits, leap, coil, startle, climb
- `hunter` — six archetypes, cascading vision cone + omni + scent/vibration, micro-FSM (patrol→alert→track→chase→search)
- `ecology` — eight ambient critter species with a flee/wander FSM
- `class` — six terrain-specialised creature classes (land/water/mtn speeds, jump, stun-resist, fly); `speed_at` reads the heightfield **(shared)**
- `powerup` — Speed / Shield / Triple-Toss buffs **(shared)**
- `ring` — a shrinking safe-zone / storm dome **(shared)**
- `sim` — the 6v1 match (leap, roll, hawk grab-and-carry, prey pounce, difficulty)
- `prowl` — the Prowl mode (rotating objectives, prey pounce, exposure ring, respawn)
- `capture` — the Pop Island mode (teams, yarn, firecracker stuns, power-ups, periods + overtime, role-based AI)
- `objectives` — Reach / Rendezvous / Survive / Forage, placed by terrain
- `mapeval` — map scoring (openness/flow/cover/verticality → S–F) + hunter-vs-cat fairness
- `menu` — screen/cursor/settings UI logic + mode/hunter/class selection (tested)
- `net` — server-authoritative netcode (≤6 players), snapshot delta encode/apply, wire (de)serialize (fuzz-tested)

`main.rs` is the offscreen renderer (scene builders + wgpu pipelines + PNG readback).
`app.rs` is the live winit/wgpu window (per-mode input, HUD, cameras); `egui_render.rs`
is a hand-rolled wgpu egui renderer.

## Testing
`cargo test` runs 69 tests: sim/prowl/capture mechanics, difficulty, yarn-conservation
and invariant soaks, class/powerup/ring units, menu logic, and the netcode fuzz tests.
Render configs are swept headless for panics.

## Provenance
Algorithms adapted from the accompanying Rust/C sources: terrain warp, biome temp-map,
plant genetics, thermal erosion + water field (unendlich); map scoring + enemy FSM
(early-fps); netcode wire/reliability model (cybertron). Prowl and Pop Island port two
in-house browser prototypes.

## Status
See `TODO.md`. All four modes are playable in the live window and verified headless.
Sim/mode logic is unit- and soak-tested; the app-side presentation (cameras, HUD,
timing, mouse binds) is compile-verified and awaits on-hardware playtest tuning.
