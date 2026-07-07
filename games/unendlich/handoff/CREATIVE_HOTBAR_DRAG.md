# Drag items to the creative hotbar (r655)

## What's wired (UI)
- `world_renderer.creative_hotbar: [Option<ItemKind>; 10]` — per-slot assignments.
- `unendlich_ui::menu::cell_grid` now senses `click_and_drag` and, on drag-start,
  publishes the dragged cell index as `menu::CellDrag(i)` via `dnd_set_drag_payload`.
  (Click behaviour is unchanged — `click_and_drag` still reports clicks.)
- The bottom creative hotbar renders `creative_hotbar[i]` if assigned, else the
  palette item in that column, and each slot is a drop target:
  `resp.dnd_release_payload::<CellDrag>()` → `creative_hotbar[i] = items[payload.0]`.

So: open creative → drag a palette cell onto a hotbar slot → it sticks there.

## The one remaining piece (bin) — make assigned items PLACE
Right now placement reads the *palette* selection (`items[creative_selected]`).
To actually place an assigned item when its hotbar slot is selected:
1. When a hotbar slot `s` is selected (home-row key or a click you add on the
   slot's `slot_swatch` Response), and `creative_hotbar[s].is_some()`, the block/
   item the bin places should be `creative_hotbar[s]` rather than the palette item.
2. Concretely, wherever the bin resolves the creative "held item" from
   `renderer.creative_selected`, prefer `renderer.creative_hotbar[sel]` when it's
   `Some`, else fall back to the palette item.

That keeps the palette-select flow working while letting drag-assigned slots place.

## Verify on build
egui 0.34 dnd methods used: `Response::dnd_set_drag_payload`,
`Response::dnd_release_payload::<T>()`. If a name differs in your egui pin, those
are the two call sites (menu.rs:498, world_renderer.rs:4948). Delimiter-checked;
needs your build to compile-confirm + a playtest of the drag feel.
