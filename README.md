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
- **The living jungle** (`src/wild.rs`) — six **Heal Trees** (rest 4 s out of
  combat beneath one to mend HP + mana; Sweetrain doubles it; the fruit glows
  while it works). **Hares and stags** graze in groups, bolt when noticed, and
  are *caught* by getting within pounce range — dashes are the hunting tool —
  for gold and a mouthful of health. AI ancestors hunt when hurt and
  unthreatened. The **glowfish** surfaces once a minute per river; catch it for
  the **Riverblessing** (+12 % speed, +10 % attack speed, mana regen, and free
  passage through the rivers for 45 s) — this map's jungle-camp buff.
- **Rivers you can feel** — a stylized liquid surface (`shaders/water.wgsl`):
  flow-aligned wave trains, depth-shaded from clear shallows to a dark
  channel, bank foam, sparkling sun glints, rain rings. Voxel "gelatin" beads
  ride the surface, waders throw splash rings, fish leap, dragonflies stitch
  the air. Wading costs up to 40 % speed by depth; Rainspots and Riptail are
  riverborn and wade free.
- **Wind you can see and hear** — gust fronts (weather.rs) rise in ~1.5 s
  and fall over ~6 s, more often and harder in storms, with a wandering
  heading. Every flora voxel carries a sway weight; the voxel and shadow
  shaders bend the canopy by height with a travelling gust wave and tip
  flutter, so trees, palms, bamboo, ferns and vine curtains lean and shiver
  together (rocks and ruins stay put). Big gusts tear leaves loose.
- **Procedural ambience** (`src/audio.rs`, cpal, feature `audio`, on by
  default) — no samples: a wind body, a place-shaped howl (the resonant band
  the wind excites depends on where you stand — a whistle over the river, a
  hollow moan under the stone gates, a broad hiss in the canopy, a keening on
  the cliff rim), pitch rising with each gust, a high keen only the strongest
  fronts reach, rain, water lapping by the channel, thunder rolls keyed to the
  actual lightning strikes and their distance, and a Schroeder reverb that
  opens up at the bases, on the bridges and along the cliffs. The howl pans
  with the wind relative to the camera. `--no-default-features` builds without
  cpal; the DSP core is tested either way.
- **Lighting** — warm wrap-lit key, sky-coloured fill in the shadows (no dead
  grey), ground bounce tinted by the haze, a soft back/rim light from the
  anti-sun side, a broad enamel specular on all voxels, contact darkening near
  the ground, sun light shafts in post when the sun is low and in frame, a
  gentle S-curve after ACES. Shadows follow the swaying canopy.
- **Ambient life** — ground-bird flocks that flush when you come near and
  settle elsewhere, glow-moths around the heal trees at night, fireflies that
  come out under cloud, sky birds that scatter at lightning and roost at night.
- **100-item shop** in 8 stat categories, three tiers, 8 stacking items. AI
  ancestors auto-buy role-appropriate builds.
- **Character select** with live offscreen-rendered voxel portraits, legend,
  philosophy, stat bars and ability descriptions.
- **Full HUD**: ability bar with cooldown sweeps, HP/MP/XP bars, gold + 6 item
  slots, kill feed, announcements, floating damage numbers, world-space HP
  bars, 3-lane minimap, hold-Tab scoreboard, death/victory/defeat screens.
- **Rendering**: MSAA×4, PCF shadow maps that follow the (weather-driven,
  slowly arcing) sun, a 1.6 u heightmap with per-material fragment detail
  (grass tufts, cracked dirt with pebbles and ruts, pebbled riverbed with
  algae, rippled sand, strata rock), snow cover, boulder clusters and vine
  curtains among the flora, weather sky with stars and moon, fog, bloom, ACES
  tonemap + vignette + grain. Ancestor bodies are meshed at 0.05 u voxels
  (2× the linear density of the previous humanoids).

## Build & Run

Requirements: Rust 1.75+, a GPU with Vulkan (or GL). For audio on Linux,
ALSA headers (`libasound2-dev`) at build time — or build with
`--no-default-features` for a silent binary.

```bash
cargo run --release          # windowed game
cargo test                   # 30 tests: roster, items, lanes, sim, weather, map events, wildlife,
                             #   plus offline WGSL validation of every shader with naga
cargo run -- --screenshot    # headless demo: renders PNG screenshots (no window)
cargo run --example simdbg   # 120 s AI-only match trace (economy, events, weather, hunt tally)
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
├── wild.rs        heal trees, prey (hare / stag / glowfish), the hunt, Riverblessing
├── audio.rs       acoustics_at(pos) → howl/reverb voicing; WindSynth DSP; cpal shell
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
├── render.rs      wgpu pipelines: shadow → HDR MSAA scene (terrain, water) → post
├── ui.rs          egui: menus, ancestor select, HUD, sky chip, minimap, shop
├── app.rs         winit window + pointer-lock controls + fixed-step sim
└── headless.rs    windowless screenshot harness (CI / software Vulkan)
shaders/           voxel, shadow, terrain, water, sky, post (WGSL) — all read
                   the weather block of the frame uniform
tests/             catalog, roster, lanes, mitigation, Elder gate, 120s sim,
                   naga validation of every shader + uniform layout sizes
```

Rendering pipeline per frame:

1. **Shadow pass** — every opaque voxel draw into a 2048² depth map from the
   current sun direction.
2. **Scene pass** (HDR, RGBA16F, MSAA×4) — sky → terrain (material detail,
   wet-ground specular, puddles, snow) → opaque voxels → water surface (alpha,
   depth read-only) → transparent voxels + up to 48k dynamic VFX cubes
   (weather field, river life, birds, moths).
3. **Post pass** — 24-tap single-pass bloom, lightning flash, ACES tonemap,
   night grade, lens rain, vignette, grain.
4. **egui pass** — HUD/menus over the frame.

## Legal

All characters, names and lore are from the Clawbound project in this
monorepo. No third-party trademarks, assets or code are used.
