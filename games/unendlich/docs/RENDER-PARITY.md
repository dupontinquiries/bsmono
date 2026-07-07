# Render parity — CPU reference vs GPU capture

`render_parity` (in `unendlich-engine`) gives a CPU oracle and a fail-fast diff
so you can answer "is the render still broken?" without loading a world and
eyeballing it. The CPU half runs in CI today (5 tests). This doc is the GPU
half — the part that needs your hardware.

## What it does and doesn't catch

- **Catches:** gross emptiness (inverted density, all-air worlds), holes /
  missing chunks (terrain reads as sky), and CPU↔GPU *structural* divergence
  (the mesh isn't where the world model says it should be).
- **Does NOT catch:** cave over-carving (a top-down ray hits the first solid
  per column regardless of internal cave volume — measured: identical coverage
  at correct and buggy cave thresholds), or fine shading/lighting differences
  (the luma threshold is deliberately loose). Caves are guarded separately by
  `world_model::tests::cave_volume_fraction_is_sane` and
  `cpu_generator::tests::surface_y_finds_terrain`.

## Wiring the GPU capture (your hardware)

1. **Capture the swapchain after stabilization.** Once the chunk stream has
   settled — e.g. N frames after the last upload, or when `tick_apply` and
   mesher activity have gone quiet — copy the presented color image into a
   host-visible buffer and read it back as row-major RGB8. A
   `vkCmdCopyImageToBuffer` into a `HOST_VISIBLE` buffer after the frame's
   fence is the simplest path. Capturing "during" vs "after" stabilize is just
   *when* you snapshot — the harness doesn't care.

2. **Use the same camera for the CPU reference.** Read the live camera's
   position + yaw/pitch/fov and build a `render_parity::Camera`. The reference
   is cheap at low resolution (64×48 is plenty for a structural check).

3. **Compare.**

   ```rust
   use unendlich_engine::render_parity::{self, Camera, RefImage, SKY_LUM};

   let cam = Camera { pos, yaw, pitch, fov_deg };
   let reference = render_parity::cpu_reference(&world_gen, &cam, w, h, 360.0, 1.0);
   let captured  = RefImage::from_rgb8_coverage(&gpu_rgb8, w, h, SKY_LUM, 16);
   match render_parity::assert_parity(&reference, &captured, 0.15, 40.0) {
       Ok(r)  => log::info!("render parity OK: {:?}", r),
       Err(r) => log::warn!("render parity DIVERGED: {:?}", r), // fires warn_snipe
   }
   ```

   Thresholds (`max_coverage_mismatch=0.15`, `max_lum_mean=40`) are starting
   points: loose enough to tolerate that the CPU oracle doesn't model GPU
   lighting, tight enough that a missing chunk or a swiss-cheesed region trips
   it. Tighten once you see the steady-state numbers on real captures.

4. **Make it a flag.** Gate the capture+compare behind an env var (e.g.
   `UNENDLICH_RENDER_PARITY=1`) so it only runs when you're hunting. On
   divergence, log at WARN so `UNENDLICH_WARN_SNIPE=1` rolls it into the
   shutdown warning summary automatically.

## Why coverage is the primary signal

A hole or missing mesh shows up as sky where the world model says terrain —
that's a coverage mismatch, and it survives any difference in how the GPU
shades the surface. Luma diff is the secondary, looser signal. The CPU
reference deliberately does not try to reproduce GPU lighting; matching the
*silhouette* is what isolates the "mesh not showing" class of bug.
