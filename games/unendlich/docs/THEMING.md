# In-game UI theming — material- and light-driven panels

Goal: an in-game panel inherits the look of *what it belongs to*. A wooden
workbench reads warm/brown; an iron anvil reads cold/grey; a torch-lit cave panel
is dimmer than the same panel in daylight. The dashboard keeps its fixed dark
palette; only diegetic game panels theme.

## What ships now (`brick/ui-theme-system`)
- `Theme` (kit): the structural tokens as data — `bg/panel/panel2/line/ink/dim/
  faint/accent/accent_soft`. Status colours (`CYAN/GREEN/RED/VIOLET`) stay global;
  they mean fixed things (good/bad/info), independent of the panel's material.
- `Theme::DARK` = the exact current dashboard palette. **This is the default**, so
  anything that doesn't opt in renders byte-identically to before.
- `Theme::from_material(albedo)` — derive a palette from a material colour
  (`block_color`): accent IS the albedo; surfaces are the albedo crushed toward
  near-black; ink is a near-white tinted by the albedo (warm on wood, cool on
  iron). Validated previews: OakLog→panel (28,23,19) warm; IronOre→(31,32,35)
  cool; Grass→(19,27,20) green.
- `Theme::lit(light)` — modulate surface tones by ambient `light ∈ [0,1]`
  (text/accent kept readable). The lighting environment bleeds into the UI.
- `set_theme(ctx, t)` / `current_theme(ctx)` — context-memory propagation, mirrors
  the `Surface` seam. Default `DARK`.
- `card_themed(ui, theme, title, body)` — `card` honouring an explicit theme.
- **Demo:** the creative picker themes its frame by the in-hand material
  (`card_themed(from_material(block_color(selected)))`). Static at the default
  selection until pointer input lands, then shifts per selection.

## Full propagation (DONE, `brick/ui-theme-propagation`)
Every kit component (`card`, `card_themed`, `slot`, `slot_swatch`, `pill`,
`button`, `radar`) + `surface_fill` now reads `current_theme(ui.ctx())` for its
structural tokens instead of the global consts. Because `DARK == the consts`,
anything that never calls `set_theme` (the whole dashboard) is unchanged; only
themed panels differ. `card_themed` sets the theme so a panel's inner widgets
inherit it; the overlay HUD sets `Theme::DARK` explicitly and the picker themes
fully by the in-hand material.

### The one discipline: per-panel `set_theme`
`set_theme` writes context memory that persists across panels AND frames (like
`Surface`). So EACH top-level panel must set its own theme at its start, or a
themed panel will bleed into the next panel / next frame. Pattern:
```
// HUD panel:        set_theme(c, Theme::DARK);            // explicit
// Workbench panel:  set_theme(c, Theme::from_material(oak).lit(ambient));
// Anvil panel:      set_theme(c, Theme::from_material(iron).lit(ambient));
```
This ordering/persistence is the part to verify on screen (panels drawn in
sequence, memory carrying over) — hence it's staged for the build loop rather than
done blind.

## Where the SOURCE comes from
- **Material:** the block/entity the panel was opened from (a workbench → its log
  albedo; an anvil → iron). For the creative picker we use the in-hand item's
  `block_color`. A station-opened UI passes the station block's albedo.
- **Light:** sample the engine's lighting at the panel's anchor — the froxel
  volume (`froxel_fill.comp` output) or the atmosphere/sky term at the player —
  and pass it to `.lit(light)`. A single scalar (0=pitch dark, 1=full daylight) is
  enough for the first cut; per-channel light tint is a later refinement.

## Risks
- Component conversion: map consts → theme fields exactly (compile), and ensure
  per-panel `set_theme` so themes don't bleed (visual; test on screen).
- `from_material` on near-black or near-white materials yields low-contrast
  panels; clamp/raise minimum contrast if a material is too dark/bright.
