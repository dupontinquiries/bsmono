# CLAWBOUND (native Rust + wgpu)

A papercraft **4-cats-vs-1-predator** stalk/hide/chase game — rebuilt on a native
Rust + wgpu renderer (Vulkan on Linux, Metal on macOS, WebGPU on the web). Same
style as the Paperbound-inspired look: flat "origami" facets, warm key light,
paper texture + ink outlines + grain + vignette.

## Build & run
```bash
cargo run --release -- [OUT.png] [MODE] [TOD]
```
- `OUT.png` — output image (default `shot.png`)
- `MODE` — `world` (default) · `map` (high angle w/ water) · `goals` (objective
  beacons + a printed fairness grade) · `poses` (cat rig pose sheet) ·
  `seq` (walk→bound→climb motion strip) · `senses` (top-down hunter sense chart)
- `TOD` — `day` (default) · `dusk` · `night`

Examples:
```bash
cargo run --release -- map.png    map   night
cargo run --release -- goals.png  goals day     # also prints the MapScore
cargo run --release -- poses.png  poses
cargo test                                       # netcode + core unit tests
```
The renderer is offscreen → PNG (works headless, e.g. CI with Mesa lavapipe).

## Play it (live window)
```bash
cargo run --release --features app -- --play
```
A **6-cats-vs-1-hunter** match on your GPU. You are cat 0; the other five cats and
the hunter are AI. Move for the objective, break line-of-sight, dive into brambles.
- **WASD / arrows** — move
- **Shift / Ctrl / C** — crouch (in a bush = hidden)
- **Esc** — quit

The window runs the exact same authoritative `sim` as the headless `game` mode and
the netcode server, so single-player, spectating, and multiplayer share one model.
For an interactive window / phone web client, see **BUILD_WEB.md**.

## Architecture (`src/`)
Core lives in the **library** (`lib.rs`) so the native renderer and a web build
share the exact same game:
- `math` — Vec3 + column-major Mat4 (wgpu conventions)
- `mesh` — flat-shaded solids (box/cone/tapered segment) + screen-door opacity
- `world` — domain-warp terrain, Whittaker biomes, genetic foliage, **talus
  erosion** + **water** (baked heightfield), grass/rocks/bushes
- `physics` — branch mass + bending **snap-risk** (climb limits)
- `cat` — posable two-bone-legged rig: gaits, leap, coil, startle, climb
- `hunter` — five archetypes (fox/dog/badger/hawk/snake), **cascading vision
  cone** + omni + scent/vibration, and a micro-FSM (patrol→alert→track→chase→search)
- `mapeval` — map scoring (openness/flow/cover/verticality → S–F grade) + a
  hunter-vs-cat **fairness** axis
- `objectives` — Reach / Rendezvous / Survive / Forage, placed by terrain
- `net` — server-authoritative netcode (≤6 players), **snapshot delta**
  encode/apply, wire (de)serialize, interpolation (unit-tested)

`main.rs` is the offscreen renderer (scene build + wgpu pipelines + PNG readback).

## Provenance
Algorithms adapted from the accompanying Rust/C sources: terrain warp, biome
temp-map, plant genetics, thermal erosion + water field (unendlich); map
scoring + enemy FSM (early-fps); netcode wire/reliability model (cybertron).

## Status
See `TODO.md`. Core systems are built and verified headless; the remaining work
is the live window loop (input → rig → FSM → net) and wrapping `net` in a
transport (WebRTC/UDP) — both are your-hardware steps.
