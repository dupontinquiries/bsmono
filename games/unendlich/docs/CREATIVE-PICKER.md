# Creative item picker — status + wiring plan

Goal: pull up a menu, click an item (block/spawn-egg), put it in hand, place/use
it to test. Built in safe layers; step A ships now, B/C want the build loop.

## A — picker UI (SHIPPED, contained to `ui-overlay`)
A top-right `card` titled "creative · items" with a `grid_fit` of kit `slot`s,
one per entry in a curated `ITEMS: &[BlockId]` list (sparse discriminants → an
explicit list, not a range). The selected slot is highlighted; the HUD shows
`in hand: <Block>`. Selection lives on `WorldRenderer::creative_selected` and is
edited through a local copy (the closure mutates a local, never `self`, which
`prepare` borrows via `self.egui_overlay`).

`slot(..).clicked()` is already wired to set the selection — it just never fires
yet because no pointer input reaches the overlay. So the picker RENDERS now;
clicking is step B. (It's always-visible for now; step B adds a toggle key.)

Spawn eggs are listed conceptually but not included: spawning a mob needs the
entity render+spawn pipeline (currently unbound). Blocks are placeable today, so
the list is blocks; add spawn-egg entries when the mob path lands.

## B — pointer input (build loop; touches the bin)
egui needs pointer events in `RawInput.events`. Plan, lowest-churn:

1. Renderer (contained, `ui-overlay`): add
   `pub fn set_overlay_pointer(&mut self, pos: (f32,f32), down: bool)` storing
   `(pos, down)` + the previous `down` on a new field. In `prepare`, build
   `raw_input.events`:
   - always push `egui::Event::PointerMoved(pos2(pos.0, pos.1))`;
   - on a press edge (`down && !prev_down`) push `Event::PointerButton{ pos,
     button: PointerButton::Primary, pressed: true, modifiers: Default }`;
   - on a release edge push the same with `pressed: false`.
   Also set `raw_input.pixels_per_point`/screen_rect as today. Edge detection
   (not just held state) is what makes egui register a *click* (press+release).
2. Bin (the one blast-radius line, `ui-overlay`-gated): before
   `draw_frame_with_view_and_atmos`, call
   `renderer.set_overlay_pointer(input.cursor_pos(), input.is_mouse_button_pressed(Button1))`
   — BUT only when the cursor is in Normal mode (UI mode), not Disabled (camera
   look). The bin already toggles `CursorMode` (search `set_cursor_mode`); gate a
   menu key (e.g. `Tab`/`E`) to flip Normal⇄Disabled and show/hide the picker.
   When Disabled (camera mode), pass `down=false` so the world doesn't see UI
   clicks and egui doesn't see look-clicks.

Test: open the menu, click a slot, watch the AMBER highlight + `in hand:` change.

## C — held item → placement (build loop; touches gameplay)
`creative_selected` → a `BlockId`. The bin's placement path already places blocks
on right-click (search the main loop for `is_mouse_button_pressed(Button2)` /
the raycast-place code). Feed it `ITEMS[renderer.creative_selected]` as the block
to place instead of a hardcoded one. For a fuller version, route through
`items::Inventory` (it has `held()` + a hotbar) so number keys pick hotbar slots
and the creative grid fills the inventory — `items/debug_ui.rs` already sketches
a click-to-add palette + hotbar if you'd rather mount that (it's gated behind
`egui_debug`; would need an egui-0.34 nudge pass).

## Risks
- Step B's edge detection: feed press AND release, or clicks won't register.
- Cursor-mode coupling: UI clicks and camera-look share the mouse; gate by mode.
- `items/debug_ui.rs` is blind-written against an unknown egui version — expect to
  nudge a widget name if you mount it instead of the kit grid.
