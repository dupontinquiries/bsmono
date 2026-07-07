# Interactive survival inventory — 3×9 + hotbar, click-to-move (r662)

## What changed (both playtest complaints)
- **Layout (was 2×9):** `cell_grid` auto-sized columns from panel width, so 27
  slots became ~2 rows. New `unendlich_ui::menu::cell_grid_cols` forces a fixed
  column count; the survival panel now draws a **3×9 backpack** over a separate
  **9-slot hotbar row**.
- **Move items (was read-only):** a reliable **click-cursor** model (not egui
  drag, which never worked):
  - Click a filled slot → its stack lifts onto the **cursor** (drawn at the
    pointer).
  - Click another slot → the held stack drops in, swapping back whatever was
    there. So: click a backpack item, click a hotbar slot → it's in your hotbar.

## Data flow
- Bin → renderer: `inv_slots: Vec<Option<InvSlotView>>` (all 36 slots
  POSITIONALLY, `[0..hotbar_size)` = hotbar, rest = backpack), `inv_hotbar_size`,
  `inv_cursor` (the held stack, for drawing at the pointer).
- Renderer → bin: `inv_click: i32` (clicked GLOBAL slot index, -1 = none).
- Bin: `fg_inv_cursor: Option<ItemStack>`; each frame, empty cursor + filled slot
  → `inv.take(i)`; held cursor → `inv.set(i, held)` swapping the previous stack
  back. Uses the real `items::Inventory` (`take`/`slot`/`set`/`len`/`hotbar_size`).

## Notes / follow-ups
- Same-item stacks **swap** rather than merge (simple + correct for v1); a
  merge-on-same-item pass is a nice follow-up.
- The cursor is drawn with the panel's painter, so it shows while the pointer is
  over the inventory (which is when it matters). A top-layer painter would let it
  render anywhere.
- A held cursor stack **persists** if you close the inventory (reopen to keep
  moving it) — chosen over Minecraft's "drop on close" to avoid item loss.
- A fresh survival world starts empty, so mine/craft something first to have a
  stack to move.
