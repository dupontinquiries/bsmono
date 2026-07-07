# Dynamic splitscreen — render wiring (sim::player_views)

The pure decision layer is `sim::player_views` (built + tested). The renderer consumes it; the
GPU work below is compile-on-end.

## Per-player view IDs
`ViewRegistry` hands each local player a stable `ViewId` (`add_player()→id`, `remove_player(id)`).
Tag the renderer's per-player resources (the `View`, per-view culling output, any per-view
descriptor offsets) by `ViewId` so they survive joins/leaves without reshuffling.

## Animated slide-in + framebuffer blend
On join/leave, `ViewRegistry` re-targets each view to the new `viewport_split::layout` slot and
runs a `DEFAULT_ANIM_SECS` transition. Each frame: `reg.step(dt)`, then for each `PlayerView`
read `viewport` (current, interpolated rect in [0,1]) and set the scissor/viewport from it. While
`reg.animating()`, the renderer can cross-fade: render each view to its offscreen target and blend
`from → target` rects (the `from`/`target`/`viewport` triple gives the blend endpoints + progress).
A newcomer starts full-screen and shrinks into its slot (a blend/zoom-in); tune the `from` rect in
`add_player` for a different slide direction.

## Shared GPU context + overlap → efficiency
All local players share one `VulkanContext` and the chunk mesh pool — so a chunk visible to two
players must be culled/meshed/uploaded once. Each frame:
1. Extract per-view frustum planes (`frustum_plane_extract::extract(viewproj)` → `[Plane;6]`,
   flattened to `[[f32;4];6]`).
2. `player_views::compute_overlap(&view_planes, &candidate_chunk_aabbs)` →
   - `per_view[i]` — chunks view i sees (drive each view's draw list);
   - `union_count` — the unique work set (mesh/upload these once, not per-view);
   - `shared` — chunks ≥2 players see (guaranteed reuse; never re-cull/re-mesh per player);
   - `overlap_ratio` — 0 (players far apart) … 1 (same view); a live efficiency readout.
3. Mesh/upload the union once; each view's draw pass indexes the shared GPU buffers with its own
   `View` matrices. Co-op (high overlap_ratio) then costs ~one world's worth of meshing, not N.

## Integration points (bin)
`ViewRegistry::new(window_aspect, SplitPref::Auto)`; `add_player()` on join (P2/P3/P4 controller),
`remove_player(id)` on leave, `set_aspect()` on resize, `step(dt)` per frame. The fullgame menu's
Host/local-join path is where players are added; `run_world` iterates `reg.views()` for the passes.
