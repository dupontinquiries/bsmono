# Menu System — Overview & Index (r219–r235)

The in-game menu, redesigned from a browser mockup, ported to reusable egui
components, and wired into the engine — plus two systems that surfaced
alongside it (timeline branching, server game-speed). This is the map; each
piece has its own doc, linked below.

All Rust referenced here is build-verified (`cargo check` + tests under
`render,window,gpu,ui-overlay,audio`). Items marked **pending** compile and
have their model/bridge in place but await a step that needs a GPU/runtime or
netcode loop — called out explicitly so nothing reads as done that isn't.

---

## The pieces

| Area | Code | Doc | Status |
|---|---|---|---|
| Design playground | `docs/design/menu_mockup.html` (v3) | `menu_mockup_README.md` | the dialed-in reference |
| egui components | `crates/unendlich-ui/src/menu.rs` | `egui_menu_components.md` | **wired** + 6 unit tests |
| Custom fonts | `docs/design/Unendlich*.ttf` + `fonts/build_fonts.py` | `menu_mockup_README.md` | **wired** (embedded) |
| Frosted blur backdrop | `egui_overlay.rs` bridge | `blur_backdrop_integration.md` | bridge **wired**; scene-blur PASS **pending** |
| Timeline branching | `crates/unendlich-engine/src/sim/timeline.rs` | `timeline_branching.md` | model **done** + 5 tests; sim-bind **pending** |
| Server game-speed | `crates/unendlich-engine/src/sim/game_speed.rs` | `game_speed.md` | model **done** + 8 tests; tick-bind **pending** |

---

## What's wired in-game (verified)

**Fonts.** `menu::install_fonts` runs once in `EguiOverlay::new` (embedded
TTFs). Titles/nav opt into the blocky voxel face via `menu::blocks_font`.

**Blocky titles** — main menu ("UNENDLICH"), and in `menu_view.rs` every
screen heading + the title flythrough; plus Settings, Crafting, the new-world
Wizard, and the World-History header in `world_renderer.rs`.

**Settings widgets** — Video Fog/Shadows → `menu::toggle` (diamond toggles);
Audio music + Gameplay day-length + game-speed + the Performance integer levers
→ `menu::slider` (integers via an `int_slider!` round-trip); autotune →
`menu::toggle`.

**Creative inventory** → `menu::cell_grid` (neutral hollow cells, clear
interiors, row×column crosshair on hover). Hotbar stays on `slot_swatch`.

**World-history rows** → blocky header + accent diamond markers (the full
horizontal `history_point` timeline awaits screenshot capture — see pending).

**Ambient frosted surface** set for all menu UI; the mockup's edge-weighted
glass glow folded into the shared `frosted_backdrop` (activates with the blur
texture).

**Motion** — cone-parallax drift on all five menu windows (main/settings/
crafting/wizard/pause via a shared `cone_par` closure, idle when static); the
self-drawing `border_fill` ring on the main menu.

**Server game-speed** — a log slider (0x/1x/10x) on the Gameplay tab and on a
new **per-world settings page** (settings tab 5, reached by a ⚙🌐 two-column
button); reconciles to closest-to-1x across peers.

---

## Component kit (`unendlich-ui::menu`) — quick reference

Transparent-first (composites over the live scene). 21 public items:
`glass_panel` · `toggle` · `slider` · `cell` + `cell_grid` + `crosshair_state`
+ `CellState` · `history_point` · `dither_fill` · `border_fill` +
`border_fill_active` · `cone_offset` + `parallax_lerp` · `install_fonts` +
`blocks_font` + `glyph_font` · tokens (`CORNER`, `SQUARE`, `spring`, `a`,
`desaturate`). Full API in `egui_menu_components.md`.

---

## Pending (each blocked on a specific runtime/netcode step)

1. **Scene-blur render pass** — the egui external-texture bridge
   (`register_external_texture`) is built; remaining is a half-res scene blur
   (reuse `bloom.rs`'s separable gaussian) → register it → flip the menu's
   `Surface::frosted()` to `frosted_blur(tex, tint)`. Needs a GPU test loop.
   Recipe: `blur_backdrop_integration.md`.
2. **Screenshot-timeline capture** — a capture hook in the render loop to
   snapshot each world-history point; unlocks swapping the diamond-marker rows
   for the horizontal `history_point` timeline with real thumbnails.
3. **Timeline sim-binding** — bind a branch head to live `SimState`/chunks so
   switching the live branch swaps the world; region-scoped resim via
   `rollback_closure`; stamp eroded structures into voxels. `timeline_branching.md`.
4. **Game-speed tick-binding** — gather peer prefs on the netcode path,
   `reconcile` per tick in sorted order, scale how many *fixed* sim steps run
   per wall-second (a rate change, not a dt change, to keep determinism).
   `game_speed.md`.

---

## History

Per-release detail is in `docs/WIRING_STATUS.md` (entries r219–r235): r219–222
mockup→components, r223–229 engine wiring (fonts/titles/widgets/inventory/
history/parallax), r230 blur bridge, r231 timeline, r232 border-fill, r233
all-window parallax, r234 toggle + FPS fixes, r235 game-speed + world page.

---

## Backlog: title-screen world seed + spawn position (note, rNNN)

New-world flow on the title screen, before entering the world:

- **Seed field.** A text/number input for `world_seed`. Worldgen is already fully
  deterministic from `(world_seed, region)` (see `STRUCTURES_worldgen.md`), so the
  field just feeds that seed instead of the current default/random one — same seed
  reproduces the same world. Accept a typed integer and a "randomize" button;
  optionally accept a string and hash it to the u64 seed so players can share
  word-seeds.
- **Spawn position.** Inputs for the initial player/camera world coordinates. Sensible
  form is X / Z with the height resolved from the surface column at that column
  (the freeze/heightmap seed already gives a base height — see
  `light_field_architecture.md` build-order step 0), with an optional explicit Y
  override for spawning underground/in-air. Default = the world's normal spawn.
- **Wiring.** Lives in the existing menu system (`app/menu_view.rs`,
  `menu_layout.rs`) as a panel on the title/new-world screen; the two values flow
  into world creation (the seed into the generator, the position into the initial
  camera/player transform). No worldgen changes needed — both are existing inputs,
  this is just surfacing them in the UI.
