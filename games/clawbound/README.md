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
| **Battleground** | clan vs clan (playable in a terminal: `cargo run --example battleground_play`) | "Battleground of the Ancestors" — legendary First Warriors, three trails, Trees of Healing; fell the enemy Great Tree (`battle` render mode; --play wiring pending) |

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
- **R** — restart · **Esc** — menu · **F11** — fullscreen (also in Settings, remembered)

**A friend as the hunter.** Title → **Multiplayer** hosts the match on the fleet (the
bsgames hub lists it; a friend's "▶ join" launches their clawbound straight into it), or
bsgames hands both ends off itself. The joiner plays the **hunter**: WASD to move, hold
left-click or Space to pounce; the host's six cats (one of them the host) are the prey.
The host runs the match and sends its state thirty times a second; the joiner's stick
rides back (`src/hunt_net.rs`). If the joiner's link drops, the AI takes the hunter back
after two seconds.

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
- `ancestor` — the ten legendary ancestors: the great clans of legend (lion, tiger, leopard), the far wild (shadow-panther, caracal, puma), and the deep roster from the whole Felidae line and further back — the serval yowler (ranged), the Ice Age cave lion (hulk), the Smilodon executioner, the desert sun-priest (ranged, scorched ground), the control pair — the deep-snow lynx (roots, chill-on-hit) and the drowned-forest jaguar (pulls) — and the ninth son of the storm-dragon (the far-east dragon-cat: stillness-charged strikes, healing rain that mends wounded wood, thunder). Kits are pure data: passive + two techniques + an ultimate over generic verbs
- `battleground` — the clan-war mode, laned like the country is shaped (solo on the river, duo on the mountain, patrols holding the middle): Trees of Healing instead of towers, a freshkill economy instead of a shop (border vs hunting patrols; mouse/rabbit/fish prey that flees on a leash; carry kills home for saturating clan growth), cat AI patrols instead of minion waves (fewer, stronger, retreat to the medic cat rather than die — except defending the Great Tree), herb-gathering side objectives, day/night watches feeding the ancestors' passives, a leaf-bare late game with aging death timers, and breach-lane massing once a border falls. A world with **no humans**: no thunderpaths, no twoleg places (a deliberate deviation from novel canon)
- `objectives` — Reach / Rendezvous / Survive / Forage, placed by terrain
- `soundscape` — hybrid ambience: crossfading beds sampled from the terrain, plus world-placed nodes whose *voice is state* (a cleared camp goes quiet, a gnawed Heal Tree creaks), plus ducking one-shot events
- `castguide` — ability guides (ring, placed disc, dash line, cone, reticle), tooltips assembled once, and the quick/normal/instant cast state machine
- `lobby` — quick play and the 5v5 ancestor-select page: one input set for touch, controller and keyboard, a clock that starts the match whether or not you were ready, bots that fill the other nine seats
- `battleview` — the pure view model for the battleground (marks to draw, HUD numbers, camera focus), so a renderer maps rather than decides
- `wire` — the byte codec for lockstep packets (8 bytes an input, 16 a tick of two players) and a UDP carrier; tested by playing a match between two real loopback sockets
- `lockstep` — deterministic-lockstep protocol (inputs on the wire, not state; input delay, both-sides-ready ticks, digest checks, timeouts), transport-agnostic and tested between two in-process peers
- `replay` — record a match as its setup plus input changes; replay it anywhere and compare digests (bug reports that run, whole-match regression fixtures, and a standing check on determinism)
- `draft` — the pick screen's rules: name resolution by warrior or species, legal clans, autofill, and a deterministic counter-pick that answers the kits you brought
- `assetpack` — the 17 animal IQMs packed into the monorepo's parts-web format (`games/shared/bsweb`, shared with blackstar): 14-bit per-model quantisation, Morton-ordered vertices, planar varint deltas, zstd. 19.0 MB -> 2.81 MB (6.8x), worst error 0.0031% of a model's span. `Model::load` reads it as the normal path; the source IQMs were removed once it proved itself (`docs/ASSET_ARCHIVE.md` has the restore and rewrite commands)
- `junglemap` — the battleground's jungle layer: mirrored thickets (one-way ambush concealment), stone walls with passes (sight never crosses stone; off-pass crossing is a slow scramble), and mistfalls — pure `(x,z)` functions, local to the mode
- `mapeval` — map scoring (openness/flow/cover/verticality → S–F) + hunter-vs-cat fairness, plus `lane_score` for the battleground: are the three trails distinct from each other and identical for both clans?
- `menu` — screen/cursor/settings UI logic + mode/hunter/class selection (tested)
- `net` — server-authoritative netcode (≤6 players), snapshot delta encode/apply, wire (de)serialize (fuzz-tested)

`main.rs` is the offscreen renderer (scene builders + wgpu pipelines + PNG readback).
`app.rs` is the live winit/wgpu window (per-mode input, HUD, cameras); `egui_render.rs`
is a hand-rolled wgpu egui renderer.

## Testing
`cargo test` runs the suite (381 in the lib at the time of writing): sim/prowl/capture
mechanics, difficulty, yarn-conservation and invariant soaks, class/powerup/ring units,
menu logic, the netcode fuzz tests, the lockstep and wire tests, and the hunt link over
loopback UDP.
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
