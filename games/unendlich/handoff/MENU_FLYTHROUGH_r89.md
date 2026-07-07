# r89 — menu flythrough wired into engine::app

`App` now owns the title cinematic and exposes it; `menu_view` renders the fly-in.
Verified headless: `examples/flythrough_snapshot.rs` renders the pieces assembling
over time (see menu_flythrough.png) + prints the camera pose at each step.

## App surface (new)
- `app.advance(dt)` — call EVERY frame with real dt (advances the cinematic clock).
- `app.camera_pose() -> CameraPose { eye, target, fov_deg }` — the orbiting title camera.
- `app.cinematic_elapsed() -> f32` — drives the menu fly-in.

## Renderer adoption (compile-on-end)
On the Title screen, each frame:
```rust
app.advance(dt);
let pose = app.camera_pose();                 // fly the title camera through the scene
camera.set_eye_target_fov(pose.eye, pose.target, pose.fov_deg);
// draw the menu pieces flying in (replaces the static inline title panel):
let click = unendlich_engine::app::menu_view::title_flythrough_ui(
    c, app.cinematic_elapsed(), 0.6 /* fly_secs */);
// map `click` (MenuClick::Select(id)) into AppInput and feed App::tick as today.
```
Other screens (NewGame/Load/Settings/…) keep using `menu_view::menu_ui`. The fly-in
pieces are the 4 MenuPiece anchors (Singleplayer/Multiplayer/Settings/Quit); Load/
Continue can be added as static pieces or extra MenuPiece variants later.

NOTE: the snapshot is a CPU preview (fonts/atlas rasterized) — faithful for layout +
timing; the real frame still renders through the Vulkan overlay on-device.
