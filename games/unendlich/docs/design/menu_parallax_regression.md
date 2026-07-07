# Menu Parallax Regression (r245 fix)

> Part of the menu system — see `MENU_SYSTEM.md`.

## Symptom
Operator (r241/r244): **the main-menu buttons stopped working** — couldn't
create or load worlds. The buttons rendered but clicks did nothing.

## Root cause
The cone-parallax effect (r229, generalised in r233) was applied as the egui
`Window::anchor()` **offset**, computed every frame from the cursor position:

```rust
let menu_par = cone_par(c, "unendlich_menu", 0.018); // cursor-driven, per frame
egui::Window::new("unendlich_menu")
    .anchor(egui::Align2::CENTER_CENTER, menu_par)   // ← moves the whole window
    .show(c, |ui| { /* buttons */ });
```

Anchoring the window by a cursor-driven offset makes the panel **drift toward
the cursor**, which moves the buttons' interaction rects out from under the
pointer: as you move to click a button, the button slides, and the click lands
where the button *was*, not where it's drawn. The r234 repaint-gating (only
repaint while the parallax is still settling) compounds it — egui may not
process the click on a frame where the layout matches what's on screen.

Net: a cosmetic effect broke core navigation. On a main menu, that means the
game is unplayable (no way into a world).

## Fix
`cone_par` is forced to return `Vec2::ZERO`, restoring the proven pre-r229
**fixed-center anchor** for all five menu windows (main / pause / settings /
crafting / wizard). The helper and the `menu::cone_offset` / `parallax_lerp`
kit (still unit-tested) are kept for a future safe re-enable:

```rust
let cone_par = |_c: &egui::Context, _id: &str, _z: f32| -> egui::Vec2 {
    egui::Vec2::ZERO
};
```

Everything else from the menu arc (fonts, blocky titles, diamond toggles,
sliders, the cell-grid inventory, border-fill, frosted surface) is unaffected —
those don't move interaction rects.

## Safe re-enable path (future)
The lesson: **parallax must be a visual-only transform that does NOT move the
window's interaction rects.** Options, in order of safety:
1. **Translate the painted content, not the window.** Inside the window, push a
   small `ui.painter()` translation / offset the drawn shapes by the parallax
   while leaving the layout (and thus the click rects) centered. The pixels
   shift; the hit-testing doesn't.
2. **Parallax only the backdrop layers** (the frosted glass / cone background /
   history panes) — never the layer that contains buttons.
3. If the whole window must move, move it by a **fixed** offset that does not
   track the cursor (e.g. a gentle idle sway), so the buttons don't slide out
   from under a deliberate click — and do not gate the repaint while a click
   could be in flight.

Any re-enable should be validated on a real build by actually clicking every
main-menu button while moving the cursor toward each one.

## Also fixed in r245 (unrelated, same release)
The frame profiler env var collision: `UNENDLICH_PROFILE` was already the
flag-bundle **preset** selector, so the r237 percentile profiler never fired
(and `UNENDLICH_PROFILE=1` printed "unknown profile '1'"). The profiler now
reads `UNENDLICH_FRAME_PROFILE=1`. See `PERF_PROFILING.md`.
