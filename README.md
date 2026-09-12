# Clawbound — Battleground · Rust + wgpu Microvoxel Edition

A 5v5 three-lane arena for the **thirteen legendary ancestors of Clawbound**,
built as a native Linux game in Rust + wgpu. No engine, no assets — every
ancestor, minion, tower, Elder, projectile and ability effect is procedurally
generated microvoxels: tens of thousands of instanced cubes with baked ambient
occlusion, real-time sun shadow mapping, HDR rendering, single-pass bloom and an
ACES-tonemapped post pass. The sky is alive: weather drifts on its own, and
timed map events reshape the match.

The roster and its kits come straight from `monorepo/games/clawbound/src/ancestor.rs`
(names, legends, philosophies, ability names, crest colours). This crate contains
no third-party trademarks or assets.

## Features

- **5v5, three lanes** — TOP / MID / BOT with waypoint pathing, per-lane minion
  waves (melee + casters), 12 towers, two Elders gated behind tower kills.
- **13 playable ancestors** — Goldenmane, Duskstripe, Rainspots, Nightpelt,
  Windclaw, Faryowl, Oldroar, Twofang, Suneye, Snowhush, Riptail, Stillsmoke,
  Stonewhisper — across 5 roles (Warrior / Guardian / Mage / Hunter / Assassin).
  Each is a quadruped feline voxel body with a pelt signature (mane, stripes,
  rosettes, saber fangs, antlers, sun disc, ear tufts…) and its crest colour,
  plus **4 mechanical abilities** (52 total) built from 13 archetype systems.
  Clawbound kits are a passive + two techniques + an ultimate; here the passive
  is reforged as a third short-cooldown technique (see `src/gods.rs`).
- **Dynamic weather** (`src/weather.rs`) — Clear / Overcast / Rain / Storm /
  Fog / Snow, crossfading every 70–140 s, with lightning, wind, ground wetness
  (puddles + specular), cloud cover that greys the sky, rain streaks on the
  lens. Fog shortens AI sight. Deterministic through the sim rng.
- **Map events** (`src/map_events.rs`) — every ~90 s, one of:
  **Sweetrain** (both teams heal in a contested ring at centre),
  **The Long Night** (night sky; Assassins +25 % speed; sight closes in),
  **Thunder Front** (lightning strikes the lanes, hurts whoever it lands on),
  **The Long Winter** (snow; everyone slows 12 % except Snowhush),
  **Elder's Bounty** (kill gold ×2). A HUD chip shows the sky + active event.
- **100-item shop** in 8 stat categories, three tiers, 8 stacking items. AI
  ancestors auto-buy role-appropriate builds.
- **Character select** with live offscreen-rendered voxel portraits, legend,
  philosophy, stat bars and ability descriptions.
- **Full HUD**: ability bar with cooldown sweeps, HP/MP/XP bars, gold + 6 item
  slots, kill feed, announcements, floating damage numbers, world-space HP
  bars, 3-lane minimap, hold-Tab scoreboard, death/victory/defeat screens.
- **Rendering**: MSAA×4, PCF shadow maps that follow the (weather-driven) sun,
  heightmap cliff bowl, decorative props, weather sky with stars and moon,
  fog, bloom, ACES tonemap + vignette + grain. Ancestor bodies are meshed at
  0.05 u voxels (2× the linear density of the previous humanoids).

## Build & Run

Requirements: Rust 1.75+, a GPU with Vulkan (or GL).

```bash
cargo run --release          # windowed game
cargo test                   # headless game-logic suite (roster, items, lanes, sim, weather, map events)
cargo run -- --screenshot    # headless demo: renders PNG screenshots (no window)
```

On minimal-X systems winit may also need `libxkbcommon-x11` + `libxcb-xkb`.

On a headless server (no GPU), install Mesa's software rasterizer and point the
Vulkan loader at it — no root required:

```bash
mkdir -p ~/.local/mesa && cd ~/.local/mesa
apt-get download mesa-vulkan-drivers libegl-mesa0 libegl1 libglx-mesa0
for f in *.deb; do dpkg -x "$f" .; done
sed -i "s|\"library_path\": \"libvulkan_lvp.so\"|\"library_path\": \"$HOME/.local/mesa/usr/lib/x86_64-linux-gnu/libvulkan_lvp.so\"|" \
  usr/share/vulkan/icd.d/lvp_icd.json
export VK_ICD_FILENAMES=~/.local/mesa/usr/share/vulkan/icd.d/lvp_icd.json
export LD_LIBRARY_PATH=~/.local/mesa/usr/lib/x86_64-linux-gnu:$LD_LIBRARY_PATH
cargo run -- --screenshot    # renders via lavapipe (CPU)
```

## Controls

| Input | Action |
|---|---|
| Mouse (click to lock) | Third-person camera, crosshair aim |
| `W A S D` | Move (camera-relative) |
| `LMB` | Basic attack (auto-assist target) |
| `1 2 3 4` | Cast abilities at crosshair ground point (4 = ultimate) |
| `B` | Toggle the 100-item shop |
| `Tab` / `C` (hold) | Scoreboard |
| Mouse wheel | Zoom |
| `Esc` | Release cursor / close shop |

## Architecture

Headless core first, GPU shell on top — everything above the line runs under
`cargo test` with no GPU.

```
src/
├── consts.rs      arena layout (3-lane polylines, 12 towers), tuning, XP curve
├── gods.rs        13-ancestor roster (from Clawbound), 52 ability defs over 13 archetypes
├── items.rs       the 100-item catalog, stat model, stacking rules, AI builds
├── weather.rs     weather state machine → `Atmosphere` sample (plain numbers)
├── map_events.rs  event scheduler + effects; `speed_mul` / `gold_mul` hooks
├── sim.rs         entities, economy, damage pipeline, kills
├── systems.rs     tick: waves, movement, auto-attacks, projectiles, zones, sky
├── abilities.rs   cast execution for all 13 archetypes
├── ai.rs          9 AI ancestors: lane push, orbit fights, sieging, retreat, buys
── ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─
├── voxel.rs       microvoxel core: dense grids → instanced cubes + baked AO
├── models.rs      procedural feline ancestor / minion / tower / Elder builders
├── props.rs       trees, rocks, crystals, gates, fountains, pedestals
├── vfx.rs         events → voxel particles; stateless rain/snow; event visuals
├── scene.rs       uploads meshes once, records per-frame draw calls
├── render.rs      wgpu pipelines: shadow → HDR MSAA scene → bloom/ACES post
├── ui.rs          egui: menus, ancestor select, HUD, sky chip, minimap, shop
├── app.rs         winit window + pointer-lock controls + fixed-step sim
└── headless.rs    windowless screenshot harness (CI / software Vulkan)
shaders/           voxel, shadow, terrain, sky, post (WGSL) — all read the
                   weather block of the frame uniform
tests/             catalog, roster, lanes, mitigation, Elder gate, 120s sim
```

Rendering pipeline per frame:

1. **Shadow pass** — every opaque voxel draw into a 2048² depth map from the
   current sun direction.
2. **Scene pass** (HDR, RGBA16F, MSAA×4) — sky → terrain (wet-ground specular,
   puddles) → opaque voxels → transparent voxels + up to 48k dynamic VFX cubes
   (including the weather field around the camera).
3. **Post pass** — 24-tap single-pass bloom, lightning flash, ACES tonemap,
   night grade, lens rain, vignette, grain.
4. **egui pass** — HUD/menus over the frame.

## Legal

All characters, names and lore are from the Clawbound project in this
monorepo. No third-party trademarks, assets or code are used.
