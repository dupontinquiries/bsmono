# I1 handoff — wiring `sim::viewmodel` (first-person held item)

Pure module: `crates/unendlich-engine/src/sim/viewmodel.rs`. Data + math only; it
does **not** touch input or the draw path. The bin owns feeding the camera and
issuing the draw.

## What the module gives you
- `ItemForm` (re-used from `sim::elemental`) is the item kind — `Staff | Axe | Blade | Tool`.
- `viewmodel_pose(form, walk_phase, speed01, look_delta) -> ViewmodelPose`
  - `walk_phase`: radians — **reuse the forge gait/anim phase** you already advance
    (the same accumulator `ForgeAnim.anim_time`/`gait_phase` is fine).
  - `speed01`: player planar speed normalized to `[0,1]` (0 ⇒ no bob).
  - `look_delta`: `[dx, dy]` mouse/look delta for this frame (raw is fine; sway is clamped).
  - Returns an offset + small yaw/pitch/roll + scale, all in **view space**.
- `item_model(form) -> Vec<ItemBox>` — the local box model (item space, +Z = length, grip at origin).
- `box_corners_in_view(&pose, &box) -> ([V3;8], color)` — the 8 posed corners in view space.
- `place_in_view(&pose, local) -> V3` and `place_in_world(view, &CameraBasis) -> V3`.

## View-space convention
Camera at origin, looking **−Z**, **+X right**, **+Y up** (standard FPS viewmodel
pass). A held item lives at small −Z (in front), +X (right hand), −Y (below
centre). Bob and sway are bounded (|bob| ≤ ~0.022 m, |sway| ≤ 0.06 m, |roll| ≤
0.18 rad) so the model never leaves its screen corner or clips the near plane.

## Two ways to draw it
1. **Dedicated viewmodel pass (recommended).** Render with an identity view matrix
   and a separate narrow-FOV near projection (so the item never clips world
   geometry). For each `ItemBox` of `item_model(form)`, take `box_corners_in_view`
   and build your vertices; shade with the box `color` (or your material).
2. **Main-pass, attached to the camera.** Fill a `CameraBasis { pos, forward,
   right, up }` from your glm camera (`cam.pos`, `cam.forward()`, `cam.right()`,
   `cam.up()`), then map each view-space corner to world with
   `place_in_world(corner, &basis)` and submit in the normal pass.

```rust
let pose = viewmodel::viewmodel_pose(held_form, anim.anim_time, speed01, look_delta);
for b in viewmodel::item_model(held_form) {
    let (corners, color) = viewmodel::box_corners_in_view(&pose, &b);
    // option 1: use `corners` directly in a viewmodel pass, OR
    // option 2: map to world for the main pass:
    let basis = viewmodel::CameraBasis { pos: cam.pos.into(), forward: cam.forward().into(),
                                         right: cam.right().into(), up: cam.up().into() };
    let world: Vec<_> = corners.iter().map(|c| viewmodel::place_in_world(*c, &basis)).collect();
    // … emit 12 triangles per box (standard box indices), shade with `color` …
}
```

## Notes
- No new dependencies; pure `[f32;3]` math. Deterministic and panic-free.
- Tuning constants (`BOB_*`, `SWAY_*`, per-form `base_pose`) live at the top of the
  module — adjust feel there; the API is stable.
- Proof: `cargo run --example viewmodel_collage` → `viewmodel_collage.png`
  (each form across a walk cycle + a sway frame).
