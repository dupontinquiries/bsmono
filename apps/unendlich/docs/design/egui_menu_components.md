# egui Menu Components — `unendlich-ui::menu`

The dialed-in `menu_mockup.html` (v3) design language, ported to reusable
egui 0.34 components in `crates/unendlich-ui/src/menu.rs`. Built and tested
standalone; **not yet wired into the engine** (integration is the next
step — see below).

## Transparent-first

Every component composites over the live 3D scene, so all fills use alpha
(`from_rgba_unmultiplied` / the local `a()` helper) and never assume an
opaque backdrop. Pairing:
- `unendlich_ui::set_surface(ctx, Surface::frosted_blur(tex, tint))` +
  `unendlich_ui::frosted_backdrop(ui, rect, corner)` gives the blurred-scene
  look behind a panel;
- with no blur texture, the translucent tints alone still read over the
  world (no opaque box).

## Components

- `glass_panel(ui, rect, accent, light)` — frosted panel chrome: translucent
  fill, edge-weighted inner glow (thicker at the rim), top rim, and a global
  specular gleam centred on `light` (pass the scene light / cursor / biggest-
  element centre). Paint before content.
- `toggle(ui, on, accent) -> Response` — the `<…>` holder whose 45° caps
  shrink-wrap a diamond knob; fractal-dither infill fills the whole holder,
  revealing left→right when on; press squish. Flip your own bool on
  `.clicked()`.
- `slider(ui, &mut value, min, max, detents, accent) -> Response` — dither
  fill that **wraps a square knob**; unfilled right side is a dim grayscale-
  faded hairline; push/pull stretch on grab; lerp-to-detents on release.
- `cell(ui, rect, accent, state)` + `crosshair_state(cols, rows, hovered,
  idx) -> (CellState, dist)` — neutral hollow cells (clear interior, sharp
  inner hairline) with a row×column crosshair; stagger per-cell animation by
  the returned manhattan `dist` (the propagating sweep).
- `history_point(ui, pane, marker_center, accent, warm, hov)` — a screenshot
  pane frame (caller paints the thumbnail texture into `pane`) + a diamond
  marker whose inner expands slightly on hover while a decoupled border ring
  expands more and sharpens.
- `dither_fill(ui, rect, color, reveal_x, alpha)` — the shared multi-octave
  (fractal/downsampled) horizontal fill used by the toggle and slider.

## Fonts

- `install_fonts(ctx, blocks_bytes, glyphs_bytes)` — register the two TTFs.
  Embed the bytes at the call site:
  ```rust
  unendlich_ui::menu::install_fonts(
      ctx,
      include_bytes!(".../docs/design/UnendlichBlocks-Regular.ttf"),
      include_bytes!(".../docs/design/UnendlichGlyphs-Regular.ttf"),
  );
  ```
- `blocks_font(size)` / `glyph_font(size)` — `FontId`s for the blocky game
  font (titles/nav/headings) and the UI/bending glyphs. Body text stays on
  egui's default proportional font.

## Helpers / tokens

`CORNER` (3px sharp), `SQUARE`, `spring(t)` (overshoot ease ≈ the mockup's
cubic-bezier), `a(color, alpha)`, `desaturate(color, amount)`,
`cone_offset(cursor, screen, z)` + `parallax_lerp(cur, tgt, smooth)` (the
cone-projection parallax: cursor-relative layer offset + weighty smoothing),
`border_fill(ui, rect, accent, t01)` + `border_fill_active(t01)` (the
outer→inner "frame draws itself in" ring; drive t01 from an animation clock).

## Tests

`cargo test -p unendlich-ui menu::` — covers the crosshair row/column logic,
desaturate endpoints, and the spring settling to 1.0.

## Integration status

**Wired (r223–r224):**
- `menu::install_fonts` is called once in `EguiOverlay::new`
  (`render/egui_overlay.rs`), embedding both TTFs — the fonts load in-game.
- Main-menu title (`render/world_renderer.rs`) and every `menu_view.rs`
  screen heading + the title flythrough render in `menu::blocks_font`
  (feature-gated with a plain-heading fallback in `menu_view`).
- Settings → Video: the Fog/Shadows checkboxes are now `menu::toggle`
  (diamond toggles); Settings → Audio: music volume is `menu::slider`
  (fill-wraps-square-knob).

**Wired (r225–r226):**
- All remaining settings sliders (day length, perf stride/budget/distance/
  fps) → `menu::slider` (integers via an `int_slider!` round-trip); autotune
  → `menu::toggle`; a blocky SETTINGS title.
- Creative inventory → `menu::cell_grid` (neutral hollow cells + row×col
  crosshair). Hotbar stays on `slot_swatch`.

**Wired (r227–r228):**
- World-history (rollback) panel got the menu language: blocky header +
  accent diamond markers per row (full horizontal history_point timeline
  awaits the screenshot-capture pipeline). Crafting + wizard titles → blocky.
- Ambient `Surface::Frosted` set for all menu UI; the mockup's edge-weighted
  glass glow folded into the shared `frosted_backdrop` (activates with the
  blur texture).

**Wired (r229–r235):**
- Cone-parallax drift on all five menu windows (main/settings/crafting/
  wizard/pause) via a shared `cone_par` closure (idle when static).
- The self-drawing `border_fill` ring on the main menu.
- The egui external-texture bridge (`register_external_texture`) for the blur
  backdrop (the scene-blur pass itself is still pending — see below).
- Server game-speed log slider on the Gameplay tab + a per-world settings page
  (settings tab 5, ⚙🌐 two-column button).
- Toggle render fix (fill clipped to the holder shape) + a parallax repaint-
  storm FPS fix (r234).

**Still to wire** (each blocked on a runtime/netcode step — see
`MENU_SYSTEM.md` for the canonical list):
- The scene-blur PostFx pass (bridge is ready; needs a GPU test loop) —
  recipe in `blur_backdrop_integration.md`.
- Screenshot-timeline capture → the horizontal `history_point` timeline with
  real thumbnails (the rows currently show diamond markers).
- GUI-scale slider has a staged release-commit — left on the native widget.
- Game-speed tick-binding and timeline sim-binding (see their docs).

> See `MENU_SYSTEM.md` for the system-wide overview, status table, and the
> component quick-reference.

## Integration recipe (for the remaining widgets)

The engine already consumes the kit from
`crates/unendlich-engine/src/render/world_renderer.rs` (it calls
`unendlich_ui::dest_button`, `glass_at`, `fresnel`, `set_theme`). To wire
the menu:
1. Call `menu::install_fonts(ctx, …)` once at overlay init (highest-impact
   single change — gives the build the blocky character immediately).
2. In the menu window builders, set `Surface::Frosted` with the engine's
   existing blurred-scene texture, then draw `menu::glass_panel` for chrome.
3. Replace the menu's toggles/sliders/cells with `menu::toggle` /
   `menu::slider` / `menu::cell` (+ `crosshair_state`).
4. For world-history, feed real per-snapshot thumbnail textures into
   `history_point`'s `pane` (the noise placeholder in the mockup becomes the
   actual screenshot capture).
5. The cone-parallax offsets + the border-fill remain to port; the parallax
   maps to per-layer translation in the menu draw, and the border-fill is
   cleaner as a true animated-radius painter stroke than the CSS version.
