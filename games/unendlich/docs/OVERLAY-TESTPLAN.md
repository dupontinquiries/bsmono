# In-game egui overlay — test plan (3 parts)

Run this when the build loop is back. It validates the overlay end-to-end and
gives known-good vs failure signatures so a regression is obvious.

## Part 1 — dashboard (kit) visual test
The kit (`unendlich-ui`) renders in the eframe dashboard, which is fast to build
and doesn't need a GPU/world. This isolates the *kit* from the *engine renderer*.

    cargo run -p unendlich-dash   # or the dashboard's usual run command

Check:
- `card` / `pill` / `slot` / `button` render with the right tokens (amber lead-bar,
  rounded corners, glow). Pills sit inline and WRAP (never overflow/overlap) when
  the row is narrow — resize the window to force a wrap.
- The Legend dialog renders **frosted** (translucent) while the rest stays Solid.
- Radar + scatter compare panels draw.

Failure signatures: vertical glyph-column pills (old measure bug), doubled pill
borders, panels overflowing their box.

## Part 2 — engine overlay test (the HUD)
    UNENDLICH_ADAPTIVE_UPLOADS=1 UNENDLICH_INDIRECT=1 \
      cargo run --release --bin unendlich-world \
      --features render,hot-shaders,window,gpu,ui-overlay

Check (top-left, fixed at 16,16):
- An amber-bordered **unendlich** card over the world (already confirmed working).
- `frame N` increments every frame.
- `chunks loaded: K` is non-zero and changes as you fly.
- The `wait …ms` pill is GREEN normally, flips RED when wait-fences > 10ms (stall).
- The `fif N` pill cycles through 0..MAX_FRAMES_IN_FLIGHT-1.
- `xyz` tracks the camera as you move (uses `view.position`).

Failure signatures:
- No card → overlay not recorded (check `record` is before `cmd_end_render_pass`).
- Card flickers / garbled text every other frame → FIF ring slot mismatch (the
  ring fix should prevent this; if seen, confirm `prepare` runs AFTER the fence
  wait and `slot == frame_idx`).
- Validation sync-layer "buffer written while in use" → ring not engaged.
- xyz frozen → `view.position` not updating (unlikely; it's the rendered eye).

## Part 3 — both together
Run the engine (Part 2) and the dashboard (Part 1) side by side. The SAME kit code
draws in both; they should look identical (same tokens, same pill behavior). Any
divergence is a renderer issue (the hand-rolled egui-on-ash path), not the kit:
- text crisp in dashboard but blurry/offset in-engine → font-atlas upload or the
  vertex `screen_size`→NDC mapping in `egui.vert`.
- colors right in dashboard but washed/oversaturated in-engine → sRGB handling in
  `egui.frag` (straight-alpha vs premultiply; deferred sRGB).
- clipping wrong in-engine only → scissor math in `record`.

## After this passes
Proceed to the frosted box (overlay step 3: swapchain TRANSFER_SRC + copy + the
3 blur dispatches whose shaders already ship) and then input wiring (GLFW→egui
RawInput through the bin) to make the overlay interactive.
