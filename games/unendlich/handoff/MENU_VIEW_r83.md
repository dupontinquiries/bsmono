# r83 — menu drawing is now one testable lib function

`app::menu_view::menu_ui(ctx, menu) -> MenuClick` is the canonical menu drawing,
compiled + tested headlessly (egui is backend-agnostic; no glfw). Two tests run it
on a headless `egui::Context` and confirm it draws each screen.

## Renderer adoption (replaces the inline panel in world_renderer.rs ~2803)
Inside the existing `#[cfg(feature="ui-overlay")]` egui closure, replace the hand-
inlined menu/wizard `egui::Window` blocks with one call:
```rust
let click = unendlich_engine::app::menu_view::menu_ui(c, app.menu());
// stash `click` to a closure-captured local, apply after prepare()
```
## Bin adoption (the thin shell)
```rust
let click = /* from the renderer */;
let input = match click {
    MenuClick::Select(id)      => AppInput::Select(id),
    MenuClick::SelectSlot(s)   => AppInput::SelectSlot(s),
    MenuClick::Back            => AppInput::Back,
    MenuClick::Confirm         => AppInput::Confirm,
    MenuClick::WizardNext      => AppInput::WizardNext,
    MenuClick::WizardBack      => AppInput::WizardBack,
    MenuClick::WizardCycleMode => { /* cycle then None */ AppInput::None }
    MenuClick::WizardToggleSeed=> { /* toggle then None */ AppInput::None }
    MenuClick::None            => AppInput::None,
};
match app.tick(input, frame_entropy) {
    AppStep::Menu => {}                                   // keep drawing the menu
    AppStep::Launch(plan) => begin_world_from(plan),      // seed → real worldgen (deferred TODO)
    AppStep::Quit => window.set_should_close(true),
}
```
This deletes the bin's loose menu_open/menu_choice/wizard_action flag-driving
(lines ~3626-3722) and the four overlapping menu abstractions collapse to one path:
egui draw (`menu_view`) → `App` (composition) → `MenuController` (state).

## Next: render menu_view to a PNG headlessly
`menu_ui` produces `egui::FullOutput`; tessellate (`ctx.tessellate`) + rasterize the
clipped primitives with the font atlas to an image so the menu can be pixel-verified
(and shown) without the GUI. That closes the "Claude can't see visible output" gap.

## DONE (r84): headless menu → PNG works
`examples/menu_snapshot.rs` renders `menu_view::menu_ui` to PNGs in-sandbox with no
glfw/GPU: run the egui frame, capture the font atlas from the FIRST frame's
textures_delta (the second frame's is empty), `ctx.tessellate`, then a CPU
barycentric rasterizer samples the atlas coverage (bilinear) × vertex color and
alpha-blends onto a backdrop. Dependency-free PNG encoder (stored zlib + CRC/Adler).
Run: `cargo run --example menu_snapshot --features egui` → menu_title.png, menu_wizard.png.
This is the UI verification loop; the same path will render bending VFX once the
r82 shader threads land (rasterize their output to an image instead of the menu).
