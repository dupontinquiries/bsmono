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

Headless logic (sim, weather, map events, wildlife, objectives, catastrophes,
input resolution, audio DSP) is under `cargo test` and trustworthy. The
rendering, ambience, and touch-drag visuals have not been seen running on a
real display — see **[TODO.md](TODO.md)** for exactly what that covers and
what else is known-but-unresolved.

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
- **Major objectives** (`src/bosses.rs`) — two neutral beasts in sheltered
  pits (boulder-and-pillar rings with gaps, braziers, a stone floor) in the
  west and east jungle at the map's equator. **The Elder Boar** (up at 3:00,
  respawns 3:30; tusk bites + a ground slam that shoves): the killing team
  takes +250 gold each and **the Wallow's Bounty** (kill/last-hit gold ×1.5,
  90 s). **The Storm Drake** (up at 6:00, respawns 4:00; lightning bites at
  range + a breath cone): **Stormfire** (+18 % to all damage dealt, autos hit
  structures +80, 90 s). Both are neutral, turn on whoever hurt them last,
  and mend fast when left alone — half-done objectives get stolen or reset.
  AI ancestors contest them and rally to a beast their team is already on.
  Minimap diamonds + timers in the sky chip.
- **Catastrophes** (`src/disasters.rs`) — rolled once at 1:30 (45 %), at most
  one per match, with a 12 s warning: **Wildfire** (spreads downwind cell to
  cell through the flora, sears anyone inside, rain douses it, burnt cells
  stay charred — darkened flora, ash, prey avoid them), **Tempest** (a black
  storm, lightning every half-second hunting the ancestors, gale gusts, and
  strikes that can start fires), **Earthquake** (camera shake, global slow,
  fissures tearing open that burn while hot then cool into scars),
  **Flood** (the rivers spill: the whole floor becomes wading water, the
  channels drown, silt-brown water, then it recedes), **Tornado** (a funnel
  walks in from the rim, pulls everything within 16 u, throws debris and
  animals, tears the canopy). All hurt players; all leave the map changed.
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
- **102-item shop** in 8 stat categories (100, plus the two Siphon items that raise every cat's base Siphon defence), three tiers, 8 stacking items,
  searchable (type to filter, arrow keys + Enter to buy), with hover-lift and
  a green purchase flash on item cards, a gold dot marking categories
  recommended for your role, and sell-back (click an owned slot to arm it,
  click again for a 70 % refund). AI ancestors auto-buy role-appropriate
  builds. The search/keyboard-grid pattern is adapted from games/clawbound's
  `lobby.rs` ancestor picker (`LobbyInput::Point/Move/Search`), generalized
  into `src/gridpick.rs` so the same tested logic backs both the shop and
  the ancestor-select screen below.
- **Character select** with live offscreen-rendered voxel portraits, legend,
  philosophy, stat bars, ability descriptions, a search box, keyboard grid
  navigation (arrows + Enter to lock in), and hover/cursor-lift animation.
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

Requirements: Rust (stable, via rustup), a GPU with Metal (macOS), Vulkan
or GL. For audio on Linux, ALSA headers (`libasound2-dev`) at build time —
or build with `--no-default-features` for a silent binary. macOS needs
nothing extra (CoreAudio, Metal).

### Bot styles and the arena

Each match, both sides draw a page from the playbook (Pride Hunt, Shadow
Stalk, Thornwall, ...); the scoreboard and the spectator HUD say which.
Force a pairing with `CLAWBOUND_PLAYBOOK="Pride Hunt,Storm Front"` (blue, red).

```bash
cargo run --release --example arena -- 10 10 --write   # 10 generations, 10-min cap
cargo run --release --example aiprobe -- 12 8          # circling / tower-death / retreat counts
```

`--write` puts the league's best strains into `dev/arena/booklet.txt`,
which the playbook reads at build time.

### A high-end test (e.g. an M2 Ultra)

```bash
cd games/clawbound-battleground
cargo run --release                       # then set things in the game
cargo run --release -- --ultra --map pass # or force Ultra on the 15-v-15 map
```

- **Settings** (Controls & Settings: the button on the main menu, or **Esc**
  during a match): graphics **Ultra (preview)** / High / Medium / Low, fur
  (tape / locks), volumes, **show frame rate**, and **vsync** — turn it off
  to see uncapped frame times. Saved between runs.
- **Ultra** applies from the next match (it builds the smooth, furred
  characters: ~2 s); the rest applies at once. `CLAWBOUND_QUALITY=ultra|
  high|medium|low` and `CLAWBOUND_FUR=tape|locks` override the settings.
- **What Ultra adds:** smooth furred cats with PBR shading; coloured light
  from abilities, channels and lightning, with ray-traced shadows between
  the cats and onto the ground; wet ground as a mirror; god rays the canopy
  blocks. Good things to look at: Stillsmoke's ultimate (lightning), any
  cast in the rain, the pass at a low sun.
- **Numbers:** the frame-rate readout shows fps and ms. For a repeatable
  benchmark: `CLAWBOUND_SHOTS=perf cargo run --release -- --screenshot
  --map pass` (draws, instances and frame times per shot, headless).
- On startup the log prints the GPU backend and the present modes the
  display offers (`[clawbound] present modes ...`).

```bash
cargo run --release          # windowed game
cargo test                   # 79 tests (67 lib + integration + shader validation + doctests):
                             #   roster, items, lanes, sim, weather, map events, wildlife,
                             #   objectives, catastrophes, input/camera math, gridpick, touch,
                             #   plus offline WGSL validation of every shader with naga
cargo run -- --screenshot    # headless demo: renders PNG screenshots (no window)
cargo run --example simdbg 3 # 12-minute AI-only match trace for a seed (economy, events,
                             #   weather, hunt, objectives, catastrophes)
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

Every action is rebindable — open **CONTROLS** from the main menu or press
`Esc` in a match, click a row, press the key / mouse button / pad button you
want. Bindings are *logical* keys (the character a key types on your layout),
so a Dvorak player binding `e` gets the key labelled `e`. Two stock schemes
(QWERTY WASD, Dvorak `.OEU`) plus custom. Saved to
`$XDG_CONFIG_HOME/clawbound/controls.cfg` (a plain text file). This is the
input model the monorepo's other games (skydribble, unendlich) use.

| Input (QWERTY scheme) | Action |
|---|---|
| Mouse (click to lock) | Third-person camera, crosshair aim |
| `W A S D` / arrows | Move (camera-relative) |
| `Shift` (hold) | Auto-run toward the crosshair |
| `LMB` | Basic attack (auto-assist target) |
| `Space` / pad `A` / touch `^` | Leap: onto the rock, log, wall top or pillar cap you are steering at (up to 5.5 u above your paws), else up to 9 u forward. From a perch the next leap is free |
| `1 2 3 4` or `Q E R F` | Cast abilities at crosshair ground point (4 = ultimate) |
| `Z` / pad `LB` / a gentle stick | Stalk: a quarter speed, low, silent, threading between stems. A toggle, and deliberately not a held modifier. Attacking, casting or being hit suspends it for 1.5 s; dashes, blinks and buffs grant a hush at full speed; under 35% HP a cat slinks on its own |
| `B` | Toggle the shop |
| `Tab` / `C` (hold) | Scoreboard |
| Mouse wheel / `PgUp` `PgDn` | Zoom |
| `X` | Camera behind me |
| `Esc` | Pause: release cursor and open the controls panel (one press) |

**Controller** (`--features gamepad`, gilrs; needs `libudev-dev` on Linux):
left stick move, right stick look, `RT` attack, `X Y B RB` abilities 1–4,
D-pad up shop, Select scoreboard, Start menu, `R3` recenter — buttons are
rebindable like keys (press a pad button on a row).

**Touch** (phones / tablets — postmarketOS, Plasma Mobile): enabled
automatically on the first touch. A floating stick appears under your thumb
anywhere in the left 42 % of the screen; attack and ability buttons (with
cooldown sweeps, and a flash the instant one comes off cooldown) cluster
bottom-right; drag in the open area to turn the camera; **tap the ground to
walk there**. Portrait and landscape both work. Size and on/off live in the
controls panel.

**Drag-to-aim a mobility skill**: for whichever ability is a Dash or Blink
(never an ultimate — `AbilityDef::quick_aimable()`), press and drag from
that ability's own button to pick a direction — a line and a confidence
ring show exactly where it'll go — then release to fire; a plain tap still
casts it at the camera crosshair like every other ability. This replaced an
earlier, unsafe design that read a fast swipe *anywhere in the open area* as
a dash: that collided with ordinary fast camera-whipping during real fights,
which crosses the same velocity thresholds a deliberate flick would — a
misread there doesn't cost you a menu selection, it burns your escape at the
worst possible moment. Anchoring the gesture to the button itself (never
where camera-panning starts) and excluding ultimates and every non-
directional archetype (heals, shields, novas...) closes that off structurally
rather than by tuning thresholds. The per-ability-type care this needed —
ultimates get none of it, healing/shield/nova casts aim exactly like they do
on keyboard/mouse/pad — is the actual adaptation of skydribble's flick-to-
dodge for this game, not just a reskinned gesture.

Tap recognition in the open area is `src/touchkit/`, vendored from the
monorepo's `shared/touchkit` — the same gesture recognizer skydribble's
on-screen driving joystick (`shellkit::touch`, a floating stick +
BOOST/JUMP/MENU buttons + tap-to-drive) is built on, vendored the same way
`games/shared/shellkit` vendors it, so this game stays a standalone,
clonable tree with no monorepo path dependencies.

Mouse capture releases automatically if the window loses focus (so a clean
Alt-Tab can't leave the OS grab out of sync with what the game thinks is
locked), and a small crosshair fades in while captured so capture state is
never ambiguous.

Camera look uses raw mouse motion (`DeviceEvent::MouseMotion`), so it works
on Wayland compositors that refuse cursor warping. The camera-relative
movement basis (`input::camera_basis`) is verified against the actual
`Mat4::look_at_rh` view matrix in a unit test, not just derived by hand —
this is what strafe (`A`/`D`, arrow keys) reads, so a sign error there can't
silently drift back in.

## Architecture

Headless core first, GPU shell on top — everything above the line runs under
`cargo test` with no GPU.

```
src/
├── consts.rs      arena layout (3-lane polylines, 12 towers), tuning, XP curve
├── gods.rs        13-ancestor roster (from Clawbound), 52 ability defs over 13 archetypes
├── items.rs       the 102-item catalog, stat model, stacking rules, AI builds
├── weather.rs     weather state machine → `Atmosphere` sample (plain numbers)
├── map_events.rs  event scheduler + effects; `speed_mul` / `gold_mul` hooks
├── wild.rs        heal trees, prey (hare / stag / glowfish), the hunt, Riverblessing
├── bosses.rs      the Elder Boar + Storm Drake: pits, AI, rewards, TargetRef::Boss
├── disasters.rs   wildfire / tempest / earthquake / flood / tornado + map scars
├── audio.rs       acoustics_at(pos) → howl/reverb voicing; WindSynth DSP; cpal shell
├── input.rs       Action / Bind / Bindings (schemes, capture, config file), InputState,
│                  resolve(), camera_basis() (verified against the real view matrix)
├── gamepad.rs     PB_* codes, PadState, gilrs shell (feature `gamepad`)
├── touch.rs       floating stick + button cluster + tap-to-walk + swipe-to-dash
├── touchkit/      vendored gesture recognizer (tap/swipe/fling/pinch), from
│                  the monorepo's shared/touchkit — see Controls above
├── gridpick.rs    search-filtered grid + cursor, shared by shop and select
│                  (adapted from games/clawbound's lobby.rs picker)
├── launch.rs      --quick-match / --ancestor / BSGAMES_HANDOFF_* parsing
├── sim.rs         entities, economy, damage pipeline, kills
├── systems.rs     tick: waves, movement, auto-attacks, projectiles, zones, sky
├── abilities.rs   cast execution for all 13 archetypes
├── ai.rs          AI ancestors: lane push, fights in stances, tower discipline, retreat, buys
├── tactics.rs     fight decisions: dive / hold / regroup / retreat from a Lanchester edge
├── playbook.rs    the booklet of team styles each side draws per match (+ dev/arena/booklet.txt)
├── arena.rs       league: styles play headless, Elo, PFSP matchmaking, mutation
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

## Status

See **[TODO.md](TODO.md)** for what's untested (mainly: anything visual, since
this was built without a display), known gaps (no netcode for the bsgames
handoff, gamepad untested end-to-end, an unexplained Blue-side AI win bias),
and ideas raised but not built.
